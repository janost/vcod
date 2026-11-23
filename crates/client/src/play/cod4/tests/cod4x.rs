//! Live joins of the CoD4x 21.3 release server (cod4x-protocol.md 18.3,
//! 18.4): each test runs `tools/cod4x_capture.sh` on `$COD4X_DEDRUN` and
//! itself again inside its namespace, where the second run joins.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use vcod_cod4::net::clmsg::UserCmd;
use vcod_cod4::net::conn::{Conn, Cp, GAMESTATE_WAIT, Join, NO_GAMESTATE};
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
/// `sv_noauth 1`, mp_crash, the 1.7 data `$COD4_DIR`'s) with this binary's
/// test `test` run again inside as the client; returns the server's log
/// once the inside run passed. `$COD4X_OUT` keeps the capture (`lo.txt`,
/// `server.log`, `client.log`), else a temporary directory does.
fn run_in_capture(test: &str) -> String {
    let bin =
        std::env::var("COD4X_DEDRUN").expect("set COD4X_DEDRUN to the release's cod4x18_dedrun");
    assert!(vcod_cod4::testing::cod4_install().is_some(), "set COD4_DIR");
    let tmp = tempfile::tempdir().unwrap();
    let out = std::env::var("COD4X_OUT").map_or_else(|_| tmp.path().to_path_buf(), Into::into);
    let script = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tools/cod4x_capture.sh");
    let name = format!("{}::{test}", module_path!());
    let name = name.split_once("::").unwrap().1;
    let exe = std::env::current_exe().unwrap();
    let inner = format!(
        "COD4X_INNER_ADDR=$ADDR '{}' --exact {name} --ignored --nocapture --test-threads 1",
        exe.display()
    );
    let st = std::process::Command::new("bash")
        .args([script, &bin, out.to_str().unwrap(), &inner])
        .env("SERVER_SETS", "+set sv_noauth 1")
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
/// `disconnect`.
fn connected_and_left(server: &str, who: &str) -> bool {
    server.contains(&format!("Going from CS_FREE to CS_CONNECTED for {who}"))
        && server.lines().any(|l| {
            l.starts_with(&format!("Player {who}^7, ")) && l.ends_with("dropped: EXE_DISCONNECTED")
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
    let server = run_in_capture("cod4x_live_join_spawns_and_leaves");
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
/// passed, and the server logs the client leaving.
#[test]
#[ignore = "live: the CoD4x release under tools/cod4x_capture.sh; set COD4X_DEDRUN and COD4_DIR, ~25 s"]
fn cod4x_live_withheld_gamestate_times_out() {
    if let Ok(addr) = std::env::var("COD4X_INNER_ADDR") {
        withheld_inside(addr.parse().unwrap());
        return;
    }
    let server = run_in_capture("cod4x_live_withheld_gamestate_times_out");
    assert!(connected_and_left(&server, "vcodxw"), "{server}");
}
