//! Live joins of the CoD4x 21.3 release server (cod4x-protocol.md 18.3 to
//! 18.5): each test runs `tools/cod4x_capture.sh` on `$COD4X_DEDRUN` and
//! itself again inside its namespace, where the second run joins.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use vcod_cod4::net::clmsg::UserCmd;
use vcod_cod4::net::conn::{Conn, Cp, GAMESTATE_WAIT, Join, NO_GAMESTATE, Refused};
use vcod_cod4::net::parse::Op;
use vcod_cod4::net::reliable::{self, BARE_ACK, Packet};

use super::*;
use crate::play::cod4_menus::Join as MenuJoin;

/// Each datagram's tap tag and bytes.
type Datagrams = Vec<(String, Vec<u8>)>;

/// The join's steps in the order `cap-noauth1` shows them
/// (cod4x-protocol.md 6).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Step {
    GetChallenge,
    ChallengeResponse,
    Connect,
    ConnectResponse,
    ServerNetchan,
    ClientNetchan,
    StatsRequest,
    Stats,
    GamestateFragment,
}

/// A datagram's step, `None` for one that marks none (a bare
/// acknowledge, any other out-of-band line).
fn step(tag: &str, d: &[u8]) -> Option<Step> {
    let s2c = tag == "s2c";
    if let Some(text) = d.strip_prefix(b"\xff\xff\xff\xff") {
        let starts = |p: &[u8]| text.starts_with(p);
        return [
            (&b"getchallenge"[..], Step::GetChallenge),
            (b"challengeResponse", Step::ChallengeResponse),
            (b"connectResponse", Step::ConnectResponse),
            (b"connect ", Step::Connect),
        ]
        .into_iter()
        .find(|(p, _)| starts(p))
        .map(|(_, s)| s);
    }
    if Packet::is_side(d) {
        let p = Packet::read(d).ok()?;
        return match (s2c, p.sequence) {
            (_, BARE_ACK) => None,
            (true, 0) => Some(Step::StatsRequest),
            (true, _) => Some(Step::GamestateFragment),
            (false, _) => Some(Step::Stats),
        };
    }
    Some(if s2c {
        Step::ServerNetchan
    } else {
        Step::ClientNetchan
    })
}

/// One move for a raw [`Conn`] once it has the gamestate: the newest
/// snapshot's time run on by the wall since it came. The server sends no
/// snapshot before the first move (cod4x-protocol.md 18.3).
fn move_cmd(conn: &Conn, snap_at: Instant) -> Vec<UserCmd> {
    if conn.client.gamestate.is_none() {
        return Vec::new();
    }
    let t = conn.client.snapshots.last().map_or(0, |s| {
        (s.server_time + snap_at.elapsed().as_millis() as i32).raw()
    });
    vec![UserCmd {
        server_time: t.max(conn.last_cmd().server_time + 1),
        ..UserCmd::default()
    }]
}

/// The inside half: client A joins as a bare [`Conn`] and its datagrams
/// and ops are checked against `cap-noauth1`'s order; client B then
/// joins as the client's session, picks a team and class, spawns and
/// leaves while A counts the `svc 11` entries B's join brings it.
fn join_inside(addr: SocketAddr) {
    let seen: Arc<Mutex<Datagrams>> = Arc::default();
    let log = seen.clone();
    let mut a = Conn::join(
        addr,
        Join {
            name: "vcodxa".into(),
            qport: 1001,
            tap: Some(Box::new(move |tag, _, d| {
                log.lock().unwrap().push((tag.to_string(), d.to_vec()));
                Ok(())
            })),
            ..Join::default()
        },
    )
    .unwrap();
    let mut ops = Vec::new();
    let t0 = Instant::now();
    let mut snap_at = Instant::now();
    while a.client.snapshots.is_empty() {
        assert!(t0.elapsed() < Duration::from_secs(30), "no snapshot");
        let cmds = move_cmd(&a, snap_at);
        a.send(&cmds, None).unwrap();
        for r in a.poll(Instant::now() + Duration::from_millis(50)).unwrap() {
            for op in r.parsed.ops {
                match op {
                    Op::Gamestate => ops.push("gamestate"),
                    Op::Snapshot(_) => {
                        ops.push("snapshot");
                        snap_at = Instant::now();
                    }
                    _ => {}
                }
            }
        }
    }
    assert_eq!(&ops[..2], ["gamestate", "snapshot"], "{ops:?}");
    let gs = a.client.gamestate.as_ref().unwrap();
    let si = gs.systeminfo("sv_referencedFFNames").unwrap_or_default();
    println!(
        "cod4x: A's gamestate in {:?}, sv_referencedFFNames {si:?}",
        t0.elapsed()
    );
    assert!(!si.to_ascii_lowercase().contains("cod4x"), "{si}");

    // The steps' first datagrams, in order; the stats in 7 fragments
    // (0..6), the gamestate in fragments 1.. up to its last.
    let seen = std::mem::take(&mut *seen.lock().unwrap());
    let steps: Vec<Step> = seen.iter().filter_map(|(t, d)| step(t, d)).collect();
    let first = |s: Step| steps.iter().position(|&x| x == s).unwrap_or(usize::MAX);
    let order = [
        Step::GetChallenge,
        Step::ChallengeResponse,
        Step::Connect,
        Step::ConnectResponse,
        Step::StatsRequest,
        Step::Stats,
        Step::GamestateFragment,
    ];
    let at: Vec<usize> = order.iter().map(|&s| first(s)).collect();
    assert!(at.windows(2).all(|w| w[0] < w[1]), "{order:?} at {at:?}");
    assert!(first(Step::ServerNetchan) < first(Step::StatsRequest));
    assert!(first(Step::ClientNetchan) < first(Step::StatsRequest));
    let seqs = |s2c: bool| -> std::collections::BTreeSet<i32> {
        seen.iter()
            .filter(|(t, d)| (t == "s2c") == s2c && Packet::is_side(d))
            .filter_map(|(_, d)| Packet::read(d).ok())
            .map(|p| p.sequence)
            .filter(|&s| s != BARE_ACK)
            .collect()
    };
    assert_eq!(seqs(false), (0..7).collect());
    let gs_frags = seqs(true);
    let last = *gs_frags.last().unwrap();
    assert_eq!(
        gs_frags,
        (0..=last).collect(),
        "the stats request and the gamestate"
    );
    println!("cod4x: A's gamestate in fragments 1..={last}");

    // B: the session, from its own thread; A keeps moving meanwhile.
    let b = std::thread::spawn(move || {
        let join = MenuJoin {
            team: Some("allies".into()),
            class: Some("assault_mp,0".into()),
        };
        let mut s = Cod4Session::connect(addr, "vcodxb", Cp::Bare, join).unwrap();
        let t0 = Instant::now();
        let mut spawned = None;
        while t0.elapsed() < Duration::from_secs(30) && spawned.is_none() {
            s.step(t0.elapsed().as_secs_f64() * 1000.0).unwrap();
            spawned = s.spawn();
            std::thread::sleep(Duration::from_millis(16));
        }
        let spawned = spawned.expect("B never spawned");
        let at_spawn = s.client().total_snapshots();
        let t1 = Instant::now();
        while t1.elapsed() < Duration::from_secs(3) {
            s.step(t0.elapsed().as_secs_f64() * 1000.0).unwrap();
            std::thread::sleep(Duration::from_millis(16));
        }
        let after = s.client().total_snapshots() - at_spawn;
        let dropped = s.take_dropped();
        s.disconnect();
        (spawned, after, dropped)
    });
    // A's messages with an `svc 11` entry, each with the acknowledge A
    // had sent by then.
    let mut entries: Vec<(i32, i32)> = Vec::new();
    let mut left: Option<Instant> = None;
    // A second past B's leave, for the entry it brings.
    while left.is_none_or(|t| t.elapsed() < Duration::from_secs(1)) {
        assert!(t0.elapsed() < Duration::from_secs(90), "B never finished");
        if left.is_none() && b.is_finished() {
            left = Some(Instant::now());
        }
        let cmds = move_cmd(&a, snap_at);
        let sent_ack = a.client.config_data_ack;
        a.send(&cmds, None).unwrap();
        for r in a.poll(Instant::now() + Duration::from_millis(50)).unwrap() {
            for op in &r.parsed.ops {
                match op {
                    Op::Snapshot(_) => snap_at = Instant::now(),
                    Op::ClientConfig { sequence, .. } => entries.push((*sequence, sent_ack)),
                    _ => {}
                }
            }
        }
    }
    let (spawned, after, dropped) = b.join().unwrap();
    println!("cod4x: B spawned: {spawned}; {after} snapshots in the 3 s after");
    assert!(after >= 30, "{after} snapshots in 3 s");
    assert_eq!(dropped, None);
    // Once A's header carries an entry's sequence, at most the messages
    // already on the way carry that entry again.
    let repeats = |seq: i32| entries.iter().filter(|e| e.0 == seq && e.1 >= seq).count();
    let seqs: std::collections::BTreeSet<i32> = entries.iter().map(|e| e.0).collect();
    for &seq in &seqs {
        let all = entries.iter().filter(|e| e.0 == seq).count();
        println!(
            "cod4x: A got entry {seq} in {all} messages, {} after acknowledging it",
            repeats(seq)
        );
        assert!(repeats(seq) <= 3, "entry {seq} kept coming: {entries:?}");
    }
    assert!(!seqs.is_empty(), "B's join brought A no svc 11");
    a.disconnect().unwrap();
}

/// Runs `tools/cod4x_capture.sh` on the CoD4x release (`$COD4X_DEDRUN`,
/// `sv_noauth 1` and `sets`, mp_crash, the 1.7 data `$COD4_DIR`'s) with
/// this binary's test `test` run again inside as the client, its
/// `COD4X_INNER_OUT` the capture's directory, which `prepare` gets first;
/// returns the server's log once the inside run passed. `$COD4X_OUT` keeps
/// the capture (`lo.txt`, `server.log`, `client.log`), else a temporary
/// directory does.
fn run_in_capture(test: &str, sets: &str, prepare: impl FnOnce(&std::path::Path)) -> String {
    let bin =
        std::env::var("COD4X_DEDRUN").expect("set COD4X_DEDRUN to the release's cod4x18_dedrun");
    assert!(vcod_cod4::testing::cod4_install().is_some(), "set COD4_DIR");
    let tmp = tempfile::tempdir().unwrap();
    let out = std::env::var("COD4X_OUT").map_or_else(|_| tmp.path().to_path_buf(), Into::into);
    let script = vcod_common::manifest_path!("/../../tools/cod4x_capture.sh");
    let name = format!("{}::{test}", module_path!());
    let name = name.split_once("::").unwrap().1;
    let exe = std::env::current_exe().unwrap();
    prepare(&out);
    let inner = format!(
        "COD4X_INNER_ADDR=$ADDR COD4X_INNER_OUT='{}' '{}' --exact {name} --ignored --nocapture \
         --test-threads 1",
        out.display(),
        exe.display()
    );
    let st = std::process::Command::new("bash")
        .args([script, &bin, out.to_str().unwrap(), &inner])
        .env("SERVER_SETS", format!("+set sv_noauth 1 {sets}"))
        .status()
        .unwrap();
    assert!(st.success(), "{st}");
    let client = std::fs::read_to_string(out.join("client.log")).unwrap();
    print!("{client}");
    assert!(
        client.contains("test result: ok. 1 passed"),
        "the inside run failed"
    );
    std::fs::read_to_string(out.join("server.log")).unwrap()
}

/// Whether the server's log has `who` connecting and leaving with
/// `disconnect` before the server's own exit drops every client the same
/// way.
fn connected_and_left(server: &str, who: &str) -> bool {
    let up = server.split("Received signal: Terminated").next().unwrap();
    up.contains(&format!("Going from CS_FREE to CS_CONNECTED for {who}"))
        && up.lines().any(|l| {
            // A print with no newline can run into the line.
            l.contains(&format!("Player {who}^7, ")) && l.ends_with("dropped: EXE_DISCONNECTED")
        })
}

/// The CoD4x release under `tools/cod4x_capture.sh` ([`run_in_capture`]):
/// two clients join as protocol 21 clients, the first's datagrams in
/// `cap-noauth1`'s order and its `svc 11` entries stopping once
/// acknowledged, the second through the session to its spawn; both leave.
#[test]
#[ignore = "live: the CoD4x release under tools/cod4x_capture.sh; set COD4X_DEDRUN and COD4_DIR, ~30 s"]
fn cod4x_live_join_spawns_and_leaves() {
    if let Ok(addr) = std::env::var("COD4X_INNER_ADDR") {
        join_inside(addr.parse().unwrap());
        return;
    }
    let server = run_in_capture("cod4x_live_join_spawns_and_leaves", "", |_| {});
    for bad in ["reliable acknowledge", "Unknown clientcommand", "kicked"] {
        assert!(!server.contains(bad), "server.log: {bad}");
    }
    for who in ["vcodxa", "vcodxb"] {
        assert!(
            connected_and_left(&server, who),
            "{who} never connected and left"
        );
    }
}

/// A relay on loopback between a client and `server` that drops every
/// server side channel fragment past the stats request's: on the release
/// those are the gamestate's (cod4x-protocol.md 7.3). Returns the relay's
/// address and the count of fragments it dropped.
fn withholding_relay(server: SocketAddr) -> (SocketAddr, Arc<AtomicUsize>) {
    let front = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    let back = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    front.set_nonblocking(true).unwrap();
    back.set_nonblocking(true).unwrap();
    let addr = front.local_addr().unwrap();
    let dropped = Arc::new(AtomicUsize::new(0));
    let count = dropped.clone();
    let stats_request = reliable::frame(&[9, 0, 0, 0, 0]);
    std::thread::spawn(move || {
        let mut buf = vec![0u8; 65536];
        let mut client = None;
        let mut stats_seq = None;
        loop {
            let mut idle = true;
            if let Ok((n, from)) = front.recv_from(&mut buf) {
                client = Some(from);
                let _ = back.send_to(&buf[..n], server);
                idle = false;
            }
            if let Ok((n, _)) = back.recv_from(&mut buf) {
                idle = false;
                let d = &buf[..n];
                let p = Packet::is_side(d).then(|| Packet::read(d).ok()).flatten();
                if let Some(p) = p.filter(|p| p.sequence != BARE_ACK) {
                    if p.data == stats_request {
                        stats_seq = Some(p.sequence);
                    } else if stats_seq.is_some_and(|s| p.sequence > s) {
                        count.fetch_add(1, Ordering::Relaxed);
                        continue;
                    }
                }
                if let Some(c) = client {
                    let _ = front.send_to(d, c);
                }
            }
            if idle {
                std::thread::sleep(Duration::from_millis(1));
            }
        }
    });
    (addr, dropped)
}

/// The inside half of [`cod4x_live_withheld_gamestate_times_out`].
fn withheld_inside(addr: SocketAddr) {
    let (relay, dropped) = withholding_relay(addr);
    let t0 = Instant::now();
    let e = Cod4Session::connect(relay, "vcodxw", Cp::Bare, MenuJoin::default())
        .err()
        .expect("the join went on without a gamestate");
    let took = t0.elapsed();
    println!("cod4x: the join ended after {took:?}: {e:#}");
    assert_eq!(e.to_string(), NO_GAMESTATE);
    let dropped = dropped.load(Ordering::Relaxed);
    println!("cod4x: the relay dropped {dropped} gamestate fragments");
    assert!(dropped > 0, "no gamestate fragment came");
    assert!(
        took > GAMESTATE_WAIT && took < Duration::from_secs(25),
        "{took:?}"
    );
    // The relay passes the disconnect on before the run ends.
    std::thread::sleep(Duration::from_millis(500));
}

/// The CoD4x release under `tools/cod4x_capture.sh` ([`run_in_capture`])
/// behind [`withholding_relay`], a server that never sends the gamestate
/// after the stats, as an authenticating one would (cod4x-protocol.md 5):
/// the session's join ends with [`NO_GAMESTATE`] once [`GAMESTATE_WAIT`]
/// passed. The server keeps the slot past the client's `disconnect`
/// (cod4x-protocol.md 18.4).
#[test]
#[ignore = "live: the CoD4x release under tools/cod4x_capture.sh; set COD4X_DEDRUN and COD4_DIR, ~25 s"]
fn cod4x_live_withheld_gamestate_times_out() {
    if let Ok(addr) = std::env::var("COD4X_INNER_ADDR") {
        withheld_inside(addr.parse().unwrap());
        return;
    }
    let server = run_in_capture("cod4x_live_withheld_gamestate_times_out", "", |_| {});
    let connected = "Going from CS_FREE to CS_CONNECTED for vcodxw";
    assert!(server.contains(connected), "{server}");
}

/// The download gate's mod, under the server's homepath.
const X_MOD: &str = "mods/vcodx";
/// The loopback HTTP server's port inside the capture's namespace.
const X_WWW_PORT: u16 = 28991;

/// The mod's files by name: a generated iwd, and a stock fastfile as its
/// `mod.ff`, since the release with a mod and no `mod.ff` stops on its
/// missing `cod4x_patchv2` zone (cod4x-protocol.md 7.2.4).
fn x_mod_files() -> Vec<(&'static str, Vec<u8>)> {
    let install = vcod_cod4::testing::cod4_install().expect("set COD4_DIR");
    let filler: Vec<u8> = (0..30000u32)
        .map(|i| (i.wrapping_mul(2654435761) >> 24) as u8)
        .collect();
    let iwd = vcod_common::testing::pk3_with(&[
        ("vcodx/readme.txt", b"vcod CoD4x download gate\n"),
        ("vcodx/filler.bin", &filler),
    ]);
    let ff = std::fs::read(install.zone_dir("english").join("code_post_gfx_mp.ff")).unwrap();
    vec![("z_vcodx.iwd", iwd), ("mod.ff", ff)]
}

/// The mod's files written under `dir` (a homepath or the HTTP root).
fn write_x_mod(dir: &std::path::Path) {
    let dir = dir.join(X_MOD);
    std::fs::create_dir_all(&dir).unwrap();
    for (f, b) in x_mod_files() {
        std::fs::write(dir.join(f), b).unwrap();
    }
}

/// The gates' downloads into `root`, over HTTP with `www`.
fn x_fetch(root: &std::path::Path, www: bool) -> Fetch<'_> {
    Fetch {
        root,
        allow: true,
        www,
        progress: None,
        cancel: None,
        notes: None,
        max_packets: vcod_cod4::net::conn::MAX_PACKETS,
    }
}

/// One client of the download gates, on a thread of its own: joins `addr`
/// with an empty install and `www`, fetching the mod's files, and spawns;
/// returns the spawn and the files as they landed. The session stays in
/// the world, stepped, until `stop`, then leaves: an idle one would time
/// out while the next client fetches.
fn fetch_and_spawn(
    addr: SocketAddr,
    name: &'static str,
    www: bool,
    stop: Arc<std::sync::atomic::AtomicBool>,
) -> (std::thread::JoinHandle<()>, Option<String>, Vec<Vec<u8>>) {
    let (tx, rx) = std::sync::mpsc::channel();
    let client = std::thread::spawn(move || {
        let root = tempfile::tempdir().unwrap();
        let join = MenuJoin {
            team: Some("allies".into()),
            class: Some("assault_mp,0".into()),
        };
        let t0 = Instant::now();
        let fetch = Some(x_fetch(root.path(), www));
        let mut s = Cod4Session::connect_with(addr, name, Cp::Bare, join, fetch).unwrap();
        println!("cod4x: {name} joined in {:?}", t0.elapsed());
        let t1 = Instant::now();
        let mut spawned = None;
        while t1.elapsed() < Duration::from_secs(30) && spawned.is_none() {
            s.step(t1.elapsed().as_secs_f64() * 1000.0).unwrap();
            spawned = s.spawn();
            std::thread::sleep(Duration::from_millis(16));
        }
        let landed: Vec<Vec<u8>> = x_mod_files()
            .iter()
            .map(|(f, _)| std::fs::read(root.path().join(X_MOD).join(f)).unwrap_or_default())
            .collect();
        tx.send((spawned, landed)).unwrap();
        while !stop.load(Ordering::Relaxed) {
            s.step(t1.elapsed().as_secs_f64() * 1000.0).unwrap();
            std::thread::sleep(Duration::from_millis(16));
        }
        s.disconnect();
    });
    let (spawned, landed) = rx.recv().unwrap_or_else(|_| panic!("{name}'s join failed"));
    (client, spawned, landed)
}

/// The inside half of [`cod4x_live_download_fetches_the_mods_files`]: a
/// client with `cl_wwwDownload` fetches the files over HTTP, then one
/// without it answers the redirect with `www fail` and takes them over
/// the side channel, then one whose HTTP iwd is off by a byte answers it
/// with op 7 and takes it over the side channel; each lands them byte for
/// byte and spawns. Each stays in until the last is done: the server
/// refuses a connect from an address one of whose clients is still
/// connecting or a zombie (cod4x-protocol.md 7.2.4).
fn download_inside(addr: SocketAddr, out: &std::path::Path) {
    let www_root = out.join("www");
    let http = super::www::http_server(X_WWW_PORT, www_root.clone(), &[], &[], Duration::ZERO);
    let want: Vec<Vec<u8>> = x_mod_files().into_iter().map(|(_, b)| b).collect();
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let mut joined = Vec::new();
    for (name, www) in [("vcodxh", true), ("vcodxs", false), ("vcodxc", true)] {
        if name == "vcodxc" {
            let iwd = www_root.join(X_MOD).join("z_vcodx.iwd");
            let mut b = std::fs::read(&iwd).unwrap();
            let mid = b.len() / 2;
            b[mid] ^= 0xff;
            std::fs::write(&iwd, b).unwrap();
        }
        let (client, spawned, landed) = fetch_and_spawn(addr, name, www, stop.clone());
        joined.push(client);
        println!("cod4x: {name} spawned: {spawned:?}");
        assert!(spawned.is_some(), "{name} never spawned");
        for (got, want) in landed.iter().zip(&want) {
            assert!(got == want, "{name}: a file did not land whole");
        }
        if www {
            let asked = std::mem::take(&mut *http.asked.lock().unwrap());
            assert_eq!(
                asked,
                ["/mods/vcodx/z_vcodx.iwd", "/mods/vcodx/mod.ff"],
                "{name}"
            );
        }
    }
    assert!(http.asked.lock().unwrap().is_empty(), "the side channel's");
    http.stop.store(true, Ordering::Relaxed);
    stop.store(true, Ordering::Relaxed);
    for c in joined {
        c.join().unwrap();
    }
}

/// The CoD4x release under `tools/cod4x_capture.sh` ([`run_in_capture`])
/// running `fs_game mods/vcodx` with `sv_wwwDownload` on: one client fetches
/// the mod's iwd and `mod.ff` over the www redirect, one over the side
/// channel after `www fail`, one the iwd over the side channel after op 7,
/// each byte for byte against the server's files, then spawns
/// (cod4x-protocol.md 7.2, 18.5).
#[test]
#[ignore = "live: the CoD4x release under tools/cod4x_capture.sh; set COD4X_DEDRUN and COD4_DIR, ~55 s"]
fn cod4x_live_download_fetches_the_mods_files() {
    if let Ok(addr) = std::env::var("COD4X_INNER_ADDR") {
        let out = std::env::var("COD4X_INNER_OUT").unwrap();
        download_inside(addr.parse().unwrap(), out.as_ref());
        return;
    }
    // Quoted: the engine's command line reads `//` as a comment.
    let sets = format!(
        "+set fs_game {X_MOD} +set sv_wwwDownload 1 \
         +set sv_wwwBaseURL \"http://127.0.0.1:{X_WWW_PORT}\""
    );
    let server = run_in_capture("cod4x_live_download_fetches_the_mods_files", &sets, |out| {
        write_x_mod(&out.join("home"));
        write_x_mod(&out.join("www"));
    });
    for bad in ["illegal download request", "Unexpected www", "kicked"] {
        assert!(!server.contains(bad), "server.log: {bad}");
    }
    for f in ["z_vcodx.iwd", "mod.ff"] {
        let file = format!("{X_MOD}/{f}");
        for who in ["vcodxh", "vcodxs", "vcodxc"] {
            let to = format!("Redirecting client '{who}' to http://127.0.0.1:{X_WWW_PORT}/{file}");
            assert!(server.contains(&to), "server.log: no {to:?}");
        }
        let fell = format!("Client 'vcodxs' reported that the http download of '{file}' failed");
        assert!(server.contains(&fell), "server.log: no {fell:?}");
    }
    let chk = |f: &str| {
        let line = format!(
            "Client 'vcodxc' reports that the redirect download for '{X_MOD}/{f}' had wrong checksum."
        );
        server.contains(&line)
    };
    assert!(chk("z_vcodx.iwd") && !chk("mod.ff"), "server.log: op 7");
    for who in ["vcodxh", "vcodxs", "vcodxc"] {
        assert!(
            connected_and_left(&server, who),
            "{who} never connected and left"
        );
    }
}

/// The inside half of [`cod4x_live_download_refused_ends_the_join`].
fn refused_inside(addr: SocketAddr) {
    let root = tempfile::tempdir().unwrap();
    let t0 = Instant::now();
    let fetch = Some(x_fetch(root.path(), true));
    let e = Cod4Session::connect_with(addr, "vcodxr", Cp::Bare, MenuJoin::default(), fetch)
        .err()
        .expect("joined without the mod's files");
    let took = t0.elapsed();
    println!("cod4x: the join ended after {took:?}: {e:#}");
    let why = e.downcast_ref::<Refused>().map(|r| r.0.as_str());
    let want = format!("EXE_AUTODL_SERVERDISABLED\u{15}{X_MOD}/z_vcodx.iwd");
    assert_eq!(why, Some(want.as_str()));
    assert!(took < Duration::from_secs(15), "{took:?}");
    let dir = root.path().join(X_MOD);
    let left = std::fs::read_dir(&dir).map_or(0, |d| d.count());
    assert_eq!(left, 0, "{} holds a spool", dir.display());
    // The disconnect reaches the server before the run ends.
    std::thread::sleep(Duration::from_millis(500));
}

/// The CoD4x release under `tools/cod4x_capture.sh` ([`run_in_capture`])
/// running `fs_game mods/vcodx` with `sv_allowDownload 0`: a client
/// without the mod's files asks for the first, the server refuses it
/// once, and the join ends with the refusal's text and a `disconnect`
/// (cod4x-protocol.md 7.2.7).
#[test]
#[ignore = "live: the CoD4x release under tools/cod4x_capture.sh; set COD4X_DEDRUN and COD4_DIR, ~15 s"]
fn cod4x_live_download_refused_ends_the_join() {
    if let Ok(addr) = std::env::var("COD4X_INNER_ADDR") {
        refused_inside(addr.parse().unwrap());
        return;
    }
    let sets = format!("+set fs_game {X_MOD} +set sv_allowDownload 0");
    let server = run_in_capture("cod4x_live_download_refused_ends_the_join", &sets, |out| {
        write_x_mod(&out.join("home"));
    });
    let refusal = format!("\"{X_MOD}/z_vcodx.iwd\" download disabled");
    assert_eq!(
        server.matches(&refusal).count(),
        1,
        "server.log: {refusal:?}"
    );
    assert!(!server.contains("mod.ff\" download disabled"), "server.log");
    assert!(connected_and_left(&server, "vcodxr"), "{server}");
}

/// The inside half of [`cod4x_live_www_disconnected_reconnects`].
fn disconnected_inside(addr: SocketAddr, out: &std::path::Path) {
    let http = super::www::http_server(X_WWW_PORT, out.join("www"), &[], &[], Duration::ZERO);
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let (client, spawned, landed) = fetch_and_spawn(addr, "vcodxd", true, stop);
    println!("cod4x: vcodxd spawned: {spawned:?}");
    assert!(spawned.is_some(), "vcodxd never spawned");
    let want: Vec<Vec<u8>> = x_mod_files().into_iter().map(|(_, b)| b).collect();
    assert!(landed == want, "a file did not land whole");
    let asked = std::mem::take(&mut *http.asked.lock().unwrap());
    assert_eq!(asked, ["/mods/vcodx/z_vcodx.iwd", "/mods/vcodx/mod.ff"]);
    http.stop.store(true, Ordering::Relaxed);
    client.join().unwrap();
}

/// The CoD4x release under `tools/cod4x_capture.sh` ([`run_in_capture`])
/// running `fs_game mods/vcodx` with `sv_wwwDownload` and
/// `sv_wwwDlDisconnected` on: the client answers each redirect with op 6,
/// the server drops it, the file comes over HTTP and the client joins
/// again on its qport, until it has both files and spawns
/// (cod4x-protocol.md 7.2.8).
#[test]
#[ignore = "live: the CoD4x release under tools/cod4x_capture.sh; set COD4X_DEDRUN and COD4_DIR, ~20 s"]
fn cod4x_live_www_disconnected_reconnects() {
    if let Ok(addr) = std::env::var("COD4X_INNER_ADDR") {
        let out = std::env::var("COD4X_INNER_OUT").unwrap();
        disconnected_inside(addr.parse().unwrap(), out.as_ref());
        return;
    }
    let sets = format!(
        "+set fs_game {X_MOD} +set sv_wwwDownload 1 +set sv_wwwDlDisconnected 1 \
         +set sv_wwwBaseURL \"http://127.0.0.1:{X_WWW_PORT}\""
    );
    let server = run_in_capture("cod4x_live_www_disconnected_reconnects", &sets, |out| {
        write_x_mod(&out.join("home"));
        write_x_mod(&out.join("www"));
    });
    for bad in ["uncompleted connection", "Unexpected www", "kicked"] {
        assert!(!server.contains(bad), "server.log: {bad}");
    }
    let dropped = server
        .lines()
        .filter(|l| {
            l.starts_with("Player vcodxd^7, ") && l.ends_with("Client dropped to download files")
        })
        .count();
    assert_eq!(dropped, 2, "server.log: the drops to download");
    assert!(connected_and_left(&server, "vcodxd"), "{server}");
}
