//! Housekeeping against retail (cod2-protocol.md 17): the out-of-band
//! captures in `tests/fixtures/cod2/oob-*.txt` replayed step for step on
//! `Cod2Server`, every reply byte for byte past the challenge number; the
//! pacing, zombie, timeout and download rules on the in-process wire.

use std::net::SocketAddr;
use std::time::{Duration, Instant};

use vcod_cod2::net::Client;
use vcod_cod2::net::download::{DownloadEvent, DownloadMsg, Downloads};
use vcod_cod2::net::oob;

use super::*;

const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/cod2");

fn fixture(name: &str) -> Vec<(String, Vec<String>)> {
    let text = std::fs::read_to_string(format!("{FIXTURES}/{name}")).unwrap();
    let mut steps: Vec<(String, Vec<String>)> = Vec::new();
    for line in text.lines() {
        if let Some(step) = line.strip_prefix("== ") {
            steps.push((step.to_string(), Vec::new()));
        } else if let Some(hex) = line.strip_prefix("< ") {
            let bytes: Vec<u8> = (0..hex.len())
                .step_by(2)
                .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
                .collect();
            steps.last_mut().unwrap().1.push(masked(&bytes));
        }
    }
    steps
}

/// A reply as text, the challenge's random number masked.
fn masked(payload: &[u8]) -> String {
    let text = vcod_cod2::net::latin1(payload);
    if text.starts_with("challengeResponse ") {
        "challengeResponse N".into()
    } else {
        text
    }
}

/// One `oob.py` step: a query or a connect, on or off the LAN.
enum Step {
    Info,
    Status,
    /// Off the LAN, ms between the challenge and the connect, qport, keys.
    Connect(bool, u64, u16, &'static [(&'static str, &'static str)]),
}

struct Harness {
    sv: Cod2Server,
    now: Instant,
    port: u16,
}

impl Harness {
    fn new(max: usize, sets: &[(&str, &str)]) -> Harness {
        let now = Instant::now();
        let cfg = Cod2Config {
            map: "mp_toujane".into(),
            gametype: "dm".into(),
            hostname: "CoD2Host".into(),
            max_clients: max,
            gametype_script: None,
        };
        let mut sv = Cod2Server::new(cfg, 7, now);
        for (k, v) in sets {
            sv.set_cvar(k, v);
        }
        sv.load(None).unwrap();
        Harness {
            sv,
            now,
            port: 40000,
        }
    }

    fn frames(&mut self, n: u64) {
        for _ in 0..n {
            self.now += Duration::from_millis(FRAME_MS as u64);
            self.sv.tick_frames(self.now, 1);
        }
    }

    /// The out-of-band replies to `from` since the last call.
    fn replies(&mut self, from: SocketAddr) -> Vec<String> {
        self.sv
            .take_outgoing()
            .into_iter()
            .filter(|(to, _)| *to == from)
            .filter_map(|(_, p)| oob::payload(&p).map(masked))
            .collect()
    }

    fn step(&mut self, step: &Step) -> Vec<String> {
        self.port += 1;
        let off = matches!(step, Step::Connect(true, ..));
        let ip = if off {
            [198, 51, 100, 2]
        } else {
            [127, 0, 0, 1]
        };
        let from = SocketAddr::from((ip, self.port));
        match step {
            Step::Info => {
                self.sv
                    .handle_packet(from, &oob::packet("getinfo xyz"), self.now);
                self.replies(from)
            }
            Step::Status => {
                self.sv
                    .handle_packet(from, &oob::packet("getstatus xyz"), self.now);
                self.replies(from)
            }
            Step::Connect(_, delay, qport, keys) => {
                self.sv
                    .handle_packet(from, &oob::packet("getchallenge"), self.now);
                let mut out = self.replies(from);
                let challenge = self.sv.core.challenges.open(from).unwrap();
                self.frames(delay / FRAME_MS as u64);
                let mut ui = format!("\\protocol\\118\\challenge\\{challenge}\\qport\\{qport}");
                if !keys.iter().any(|(k, _)| *k == "name") {
                    ui += "\\name\\oobprobe";
                }
                for (k, v) in *keys {
                    ui += &format!("\\{k}\\{v}");
                }
                self.sv
                    .handle_packet(from, &oob::packet(&format!("connect \"{ui}\"")), self.now);
                out.extend(self.replies(from));
                out
            }
        }
    }
}

fn replay(h: &mut Harness, fixture_name: &str, steps: &[Step]) {
    let want = fixture(fixture_name);
    assert_eq!(want.len(), steps.len());
    for ((name, want), step) in want.iter().zip(steps) {
        assert_eq!(&h.step(step), want, "{fixture_name}: {name}");
        h.frames(1);
    }
}

#[test]
fn g_password_refusals_match_retail() {
    let mut h = Harness::new(8, &[("g_password", "secret"), ("sv_hostname", "PassHost")]);
    replay(
        &mut h,
        "oob-pass.txt",
        &[
            Step::Info,
            Step::Status,
            Step::Connect(false, 0, 1001, &[]),
            Step::Connect(false, 0, 1002, &[("password", "nope")]),
            Step::Connect(false, 0, 1003, &[("password", "SECRET")]),
            Step::Connect(false, 0, 1004, &[("password", "secret"), ("name", "alpha")]),
            Step::Status,
        ],
    );
}

#[test]
fn private_slots_ping_limits_and_a_full_server_match_retail() {
    let mut h = Harness::new(
        2,
        &[
            ("sv_privateClients", "1"),
            ("sv_privatePassword", "priv"),
            ("sv_minPing", "100"),
            ("sv_maxPing", "300"),
            ("sv_hostname", "SlotHost"),
        ],
    );
    replay(
        &mut h,
        "oob-slots.txt",
        &[
            Step::Info,
            Step::Status,
            Step::Info,
            Step::Status,
            Step::Connect(true, 0, 1006, &[]),
            Step::Connect(true, 400, 1007, &[]),
            Step::Connect(true, 200, 1008, &[("name", "alpha")]),
            Step::Info,
            Step::Connect(false, 0, 1009, &[("name", "bravo")]),
            Step::Connect(false, 0, 1010, &[("password", "priv"), ("name", "charlie")]),
            Step::Connect(false, 0, 1011, &[("password", "priv"), ("name", "delta")]),
            Step::Info,
            Step::Status,
        ],
    );
}

/// A client of the in-process wire: its netchan, its address, whether it
/// sends cmds yet (`CA_PRIMED`) and every packet the server sent it, with
/// the frame clock it went at.
struct Peer {
    cl: Client,
    from: SocketAddr,
    primed: bool,
    sent: Vec<(i32, usize)>,
    commands: Vec<String>,
    downloads: Vec<DownloadMsg>,
    /// The download ops each message carried, for the messages with any.
    dl_per_message: Vec<usize>,
    gamestates: usize,
    dls: Option<Downloads>,
    finished: Vec<std::path::PathBuf>,
}

impl Harness {
    fn join(&mut self, ip: [u8; 4], userinfo: &str) -> Peer {
        self.port += 1;
        let from = SocketAddr::from((ip, self.port));
        self.sv
            .handle_packet(from, &oob::packet("getchallenge"), self.now);
        self.replies(from);
        let challenge = self.sv.core.challenges.open(from).unwrap();
        let qport = self.port;
        let ui = format!("\\protocol\\118\\challenge\\{challenge}\\qport\\{qport}{userinfo}");
        self.sv
            .handle_packet(from, &oob::packet(&format!("connect \"{ui}\"")), self.now);
        assert_eq!(self.replies(from), vec!["connectResponse".to_string()]);
        Peer {
            cl: Client::new(qport, challenge),
            from,
            primed: false,
            sent: Vec::new(),
            commands: Vec::new(),
            downloads: Vec::new(),
            dl_per_message: Vec::new(),
            gamestates: 0,
            dls: None,
            finished: Vec::new(),
        }
    }

    /// `frames` frames: the peer sends a packet a frame (a cmd once
    /// primed) unless `silent`, the server ticks, the peer reads.
    fn pump(&mut self, p: &mut Peer, frames: u32, silent: bool) {
        for _ in 0..frames {
            if !silent {
                let pkt = if p.primed {
                    let cmd = p.cl.idle_cmd(self.sv.time_ms() + FRAME_MS);
                    p.cl.build_packet(&[cmd]).unwrap()
                } else {
                    p.cl.build_packet(&[]).unwrap()
                };
                self.sv.handle_packet(p.from, &pkt, self.now);
            }
            self.now += Duration::from_millis(FRAME_MS as u64);
            self.sv.tick_frames(self.now, 1);
            for (to, pkt) in self.sv.take_outgoing() {
                if to != p.from || oob::payload(&pkt).is_some() {
                    continue;
                }
                p.sent.push((self.sv.time_ms(), pkt.len()));
                let Ok(Some(parsed)) = p.cl.receive(&pkt) else {
                    continue;
                };
                for (_, t) in &parsed.commands {
                    p.commands.push(vcod_cod2::net::latin1(t));
                }
                if !parsed.downloads.is_empty() {
                    p.dl_per_message.push(parsed.downloads.len());
                }
                for m in &parsed.downloads {
                    p.downloads.push(m.clone());
                    let evs = p.dls.as_mut().map(|d| d.on_message(m)).unwrap_or_default();
                    p.apply(evs);
                }
                if parsed.gamestate {
                    p.gamestates += 1;
                    let feed = p.cl.parser.gamestate.as_ref().unwrap().checksum_feed;
                    let si =
                        p.cl.parser
                            .gamestate
                            .as_ref()
                            .unwrap()
                            .configstring(1)
                            .to_string();
                    let mut evs = Vec::new();
                    if let Some(d) = p.dls.as_mut() {
                        evs = d.next_download();
                    }
                    p.primed = !evs.iter().any(|e| matches!(e, DownloadEvent::Command(_)));
                    p.apply(evs);
                    let _ = si;
                    if p.primed {
                        p.cl.add_reliable(&vcod_cod2::net::pure::cp_command(&[], feed));
                    }
                }
            }
        }
    }
}

impl Peer {
    fn apply(&mut self, evs: Vec<DownloadEvent>) {
        for e in evs {
            match e {
                DownloadEvent::Command(c) => self.cl.add_reliable(&c),
                DownloadEvent::Finished(path) => self.finished.push(path),
                DownloadEvent::Failed(why) => self.commands.push(format!("failed {why}")),
                DownloadEvent::Skipped(why) => self.commands.push(format!("skipped {why}")),
            }
        }
    }

    /// The gaps between the packets sent at or after `from_ms`.
    fn gaps(&self, from_ms: i32) -> Vec<i32> {
        let times: Vec<i32> = self
            .sent
            .iter()
            .map(|(t, _)| *t)
            .filter(|&t| t >= from_ms)
            .collect();
        times.windows(2).map(|w| w[1] - w[0]).collect()
    }
}

const OFF_LAN: [u8; 4] = [198, 51, 100, 2];
const LAN: [u8; 4] = [127, 0, 0, 1];

/// An off-LAN client is sent a snapshot every `snaps` interval, and a
/// fragmented message a fragment at a time, each waiting what the bytes
/// left take at its rate, rounded up to a frame: 350 ms at rate 5000, as
/// retail's capture shows (cod2-protocol.md 17.2).
#[test]
fn an_off_lan_client_is_paced_by_its_rate_and_snaps() {
    let mut h = Harness::new(4, &[]);
    let mut p = h.join(OFF_LAN, "\\name\\pace\\rate\\5000\\snaps\\10");
    h.pump(&mut p, 40, false);
    assert!(p.primed && p.gamestates == 1);
    let t = h.sv.time_ms();
    h.pump(&mut p, 20, false);
    assert!(p.gaps(t).iter().all(|&g| g == 100), "{:?}", p.gaps(t));
    // A 6000-byte burst of commands: five fragments.
    for i in 0..60 {
        h.sv.send_server_command(0, &format!("print \"{i:03} {}\"", "x".repeat(90)));
    }
    let t = h.sv.time_ms();
    h.pump(&mut p, 60, false);
    let big: Vec<_> = p
        .sent
        .iter()
        .filter(|(at, n)| *at > t && *n > 1000)
        .collect();
    assert!(big.len() >= 4, "{:?}", p.sent);
    assert!(big.windows(2).all(|w| w[1].0 - w[0].0 == 350), "{big:?}");
    assert_eq!(
        p.commands.iter().filter(|c| c.starts_with("print")).count(),
        60
    );
}

/// A dropped client turns zombie: on the LAN it is sent a message every
/// frame, the first carrying its `w`, until `sv_zombietime` (2 s) after its
/// last packet; off the LAN one a second. Retail's: 40 and 2 (17.3).
#[test]
fn a_zombie_is_sent_its_reason_until_sv_zombietime() {
    for (ip, want) in [(LAN, 40), (OFF_LAN, 2)] {
        let mut h = Harness::new(4, &[]);
        let mut p = h.join(ip, "\\name\\zed\\rate\\25000\\snaps\\20");
        h.pump(&mut p, 60, false);
        p.cl.add_reliable("disconnect");
        h.pump(&mut p, 1, false);
        let t = h.sv.time_ms();
        assert_eq!(h.sv.client_count(), 0);
        h.pump(&mut p, 60, true);
        let after = p.sent.iter().filter(|(at, _)| *at > t - FRAME_MS).count();
        assert_eq!(after, want, "{ip:?} {:?}", p.sent);
        assert!(p.commands.iter().any(|c| c == "w \"EXE_DISCONNECTED\""));
        assert!(h.sv.core.clients[0].is_none());
    }
}

/// A silent client is dropped on the sixth frame past `sv_timeout` and
/// freed at once: no zombie messages, the last message 3250 ms after its
/// last packet under `sv_timeout 3` (17.3).
#[test]
fn a_silent_client_times_out_with_no_zombie_turn() {
    let mut h = Harness::new(4, &[("sv_timeout", "3")]);
    let mut p = h.join(LAN, "\\name\\tout\\rate\\25000\\snaps\\20");
    h.pump(&mut p, 40, false);
    // Its last packet came in the frame before.
    let last = h.sv.time_ms() - FRAME_MS;
    h.pump(&mut p, 100, true);
    assert_eq!(p.sent.last().unwrap().0, last + 3250, "{:?}", p.sent);
    assert!(h.sv.core.clients[0].is_none());
}

/// A served iwd under `fs_basepath`.
fn base_with_iwd() -> (tempfile_dir::Dir, Vec<u8>) {
    let dir = tempfile_dir::Dir::new();
    let filler: Vec<u8> = (0..40_000u32)
        .map(|i| (i.wrapping_mul(2654435761) >> 24) as u8)
        .collect();
    let bytes = vcod_common::testing::pk3_with(&[("vcod/filler.bin", &filler)]);
    std::fs::create_dir_all(dir.0.join("main")).unwrap();
    std::fs::write(dir.0.join("main/zzz_test.iwd"), &bytes).unwrap();
    (dir, bytes)
}

/// A scratch directory under the target dir, removed on drop.
mod tempfile_dir {
    pub struct Dir(pub std::path::PathBuf);
    impl Dir {
        pub fn new() -> Dir {
            static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
            let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let p = std::env::temp_dir().join(format!("vcod-cod2-dl-{}-{n}", std::process::id()));
            let _ = std::fs::remove_dir_all(&p);
            std::fs::create_dir_all(&p).unwrap();
            Dir(p)
        }
    }
    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}

/// `download` over UDP: block 0 carries the size, a block a message at
/// rate 25000, `nextdl` acks, the empty block ends it, `donedl` brings the
/// gamestate again; the file lands whole (17.5).
#[test]
fn an_iwd_downloads_over_udp_and_the_gamestate_follows() {
    let (base, bytes) = base_with_iwd();
    let dest = tempfile_dir::Dir::new();
    let mut h = Harness::new(4, &[("fs_basepath", &base.0.to_string_lossy())]);
    let mut p = h.join(LAN, "\\name\\dl\\rate\\25000\\snaps\\20");
    p.dls = Some(Downloads::new(
        &dest.0,
        "",
        vec!["main/zzz_test.iwd".into()],
    ));
    h.pump(&mut p, 200, false);
    assert!(
        p.commands.iter().all(|c| !c.starts_with("failed")),
        "{:?}",
        p.commands
    );
    assert_eq!(p.finished, vec![dest.0.join("main/zzz_test.iwd")]);
    assert_eq!(std::fs::read(&p.finished[0]).unwrap(), bytes);
    let blocks = bytes.len().div_ceil(2048) + 1;
    assert_eq!(p.downloads.len(), blocks, "one send a block, no retransmit");
    assert_eq!(
        p.downloads[0],
        DownloadMsg::Block {
            block: 0,
            size: Some(bytes.len() as i32),
            data: bytes[..2048].to_vec(),
        }
    );
    assert_eq!(p.gamestates, 2);
    assert!(p.primed);
    assert_eq!(h.sv.client_count(), 1);
}

/// A LAN client at `dedicated 1` (rate 99999, `snaps 20`) takes three
/// blocks a message, and the message's fragments go one a frame, as
/// retail's `SV_SendClientMessages` sends them: the transfer is held to a
/// 1300-byte fragment each 50 ms frame, about 26 KB/s, whatever the
/// window allows (17.5).
#[test]
fn a_lan_download_moves_a_fragment_a_frame() {
    let (base, bytes) = base_with_iwd();
    let dest = tempfile_dir::Dir::new();
    let mut h = Harness::new(4, &[("fs_basepath", &base.0.to_string_lossy())]);
    let mut p = h.join(LAN, "\\name\\dl\\snaps\\20");
    p.dls = Some(Downloads::new(
        &dest.0,
        "",
        vec!["main/zzz_test.iwd".into()],
    ));
    h.pump(&mut p, 200, false);
    assert_eq!(p.finished, vec![dest.0.join("main/zzz_test.iwd")]);
    let blocks = bytes.len().div_ceil(2048) + 1;
    assert_eq!(p.dl_per_message.iter().sum::<usize>(), blocks);
    assert_eq!(
        p.dl_per_message.iter().max(),
        Some(&3),
        "{:?}",
        p.dl_per_message
    );
    // From the first block's message to the last one's last fragment.
    let first = p.sent.iter().find(|(_, n)| *n > 1000).unwrap().0;
    let done = p.sent.iter().rev().find(|(_, n)| *n > 1000).unwrap().0;
    let span: Vec<_> = p
        .sent
        .iter()
        .filter(|(t, _)| (first..=done).contains(t))
        .collect();
    assert_eq!(span.len() as i32, (done - first) / FRAME_MS + 1, "{span:?}");
    assert!(span.iter().all(|(_, n)| *n <= 1300 + 10), "{span:?}");
    let frames = (done - first) / FRAME_MS + 1;
    let ceiling = bytes.len() as i32 / 1300;
    assert!(
        frames >= ceiling,
        "{frames} frames for {} bytes",
        bytes.len()
    );
}

/// The refusals retail sends, by reason (17.5).
#[test]
fn download_refusals_match_retail() {
    let (base, _) = base_with_iwd();
    let dest = tempfile_dir::Dir::new();
    let basepath = base.0.to_string_lossy().to_string();
    for (sets, name, want) in [
        (
            vec![],
            "main/nothere.iwd",
            "EXE_AUTODL_FILENOTONSERVER\x15main/nothere.iwd",
        ),
        (
            vec![],
            "main/localized_english_iw11.iwd",
            "EXE_CANTAUTODLGAMEPAK\x15main/localized_english_iw11.iwd",
        ),
        (
            vec![("sv_allowDownload", "0")],
            "main/zzz_test.iwd",
            "EXE_AUTODL_SERVERDISABLED_PURE\x15main/zzz_test.iwd",
        ),
        (
            vec![("sv_allowDownload", "0"), ("sv_pure", "0")],
            "main/zzz_test.iwd",
            "EXE_AUTODL_SERVERDISABLED\x15main/zzz_test.iwd",
        ),
    ] {
        let mut all = vec![("fs_basepath", basepath.as_str())];
        all.extend(sets);
        let mut h = Harness::new(4, &all);
        let mut p = h.join(LAN, "\\name\\dl");
        p.dls = Some(Downloads::new(&dest.0, "", vec![name.into()]));
        h.pump(&mut p, 20, false);
        assert_eq!(
            p.downloads,
            vec![DownloadMsg::Refused(want.into())],
            "{name}"
        );
    }
}

/// `sv_wwwDownload` with a client's `cl_wwwDownload`: the redirect first
/// (base URL, size, the disconnected flag), then after `wwwdl fail` the
/// gamestate and the file over UDP (17.5).
#[test]
fn a_www_redirect_falls_back_to_udp_after_fail() {
    let (base, bytes) = base_with_iwd();
    let dest = tempfile_dir::Dir::new();
    let mut h = Harness::new(
        4,
        &[
            ("fs_basepath", &base.0.to_string_lossy()),
            ("sv_wwwDownload", "1"),
            ("sv_wwwBaseURL", "http:"),
            ("sv_wwwDlDisconnected", "1"),
        ],
    );
    let mut p = h.join(
        LAN,
        "\\name\\www\\rate\\25000\\snaps\\20\\cl_wwwDownload\\1",
    );
    p.dls = Some(Downloads::new(
        &dest.0,
        "",
        vec!["main/zzz_test.iwd".into()],
    ));
    h.pump(&mut p, 10, false);
    assert_eq!(
        p.downloads.first(),
        Some(&DownloadMsg::Redirect {
            url: "http:/main/zzz_test.iwd".into(),
            size: bytes.len() as i32,
            flags: 1,
        })
    );
    // The fallback gamestate starts a new queue, served over UDP.
    assert_eq!(p.gamestates, 2);
    p.dls = Some(Downloads::new(
        &dest.0,
        "",
        vec!["main/zzz_test.iwd".into()],
    ));
    p.primed = false;
    let evs = p.dls.as_mut().unwrap().next_download();
    p.apply(evs);
    h.pump(&mut p, 200, false);
    assert_eq!(p.finished, vec![dest.0.join("main/zzz_test.iwd")]);
}

/// `SV_Heartbeat_f` on a first client, as retail's master log shows
/// (17.6): under `dedicated 2` the frame after the connect beats.
#[test]
fn a_first_client_forces_a_heartbeat() {
    let mut h = Harness::new(4, &[]);
    h.sv.set_master_resolver(|_| Some(SocketAddr::from(([127, 0, 0, 1], 0))));
    let master = SocketAddr::from(([127, 0, 0, 1], 20710));
    let beats = |h: &mut Harness| -> usize {
        h.sv.take_outgoing()
            .into_iter()
            .filter(|(to, p)| *to == master && p[4..].starts_with(b"heartbeat"))
            .count()
    };
    h.sv.set_cvar("dedicated", "2");
    h.frames(1);
    assert_eq!(beats(&mut h), 1);
    h.frames(40);
    assert_eq!(beats(&mut h), 0);
    let _p = h.join(LAN, "\\name\\hb");
    h.frames(1);
    assert_eq!(beats(&mut h), 1);
}

/// `SV_MasterHeartbeat`'s status arm (0x8096ed6): under `dedicated 2` the
/// master gets a `statusResponse` every 600 s whose `challenge` is the last
/// line's first argument, the command line's map at startup (17.6). It
/// waits a second behind the heartbeat, the master's one packet a second.
#[test]
fn the_master_gets_a_status_every_ten_minutes() {
    let mut h = Harness::new(4, &[]);
    h.sv.set_master_resolver(|_| Some(SocketAddr::from(([127, 0, 0, 1], 0))));
    let master = SocketAddr::from(([127, 0, 0, 1], 20710));
    let statuses = |h: &mut Harness| -> Vec<String> {
        h.sv.take_outgoing()
            .into_iter()
            .filter(|(to, _)| *to == master)
            .filter_map(|(_, p)| oob::payload(&p).map(vcod_cod2::net::latin1))
            .filter(|t| t.starts_with("statusResponse"))
            .collect()
    };
    h.sv.set_cvar("dedicated", "2");
    h.frames(1);
    assert!(statuses(&mut h).is_empty(), "held behind the heartbeat");
    h.frames(20);
    let sent = statuses(&mut h);
    assert_eq!(sent.len(), 1);
    let info = sent[0].lines().nth(1).unwrap();
    assert_eq!(
        vcod_common::net::info_value_for_key(info, "challenge"),
        Some("mp_toujane")
    );
    // The next one is 600 s on.
    h.frames(11_950);
    assert!(statuses(&mut h).is_empty());
    h.frames(60);
    assert_eq!(statuses(&mut h).len(), 1);
}

/// rcon-style console run, its printed reply.
fn console(h: &mut Harness, line: &str) -> String {
    let mut out = crate::rcon::Redirect::default();
    h.sv.exec(line, &mut out);
    out.finish()
        .iter()
        .map(|p| p.trim_start_matches("print\n").to_string())
        .collect()
}

fn challenge_reply(h: &mut Harness, ip: [u8; 4]) -> Vec<String> {
    h.port += 1;
    let from = SocketAddr::from((ip, h.port));
    h.sv.handle_packet(from, &oob::packet("getchallenge"), h.now);
    h.replies(from)
}

/// `banUser`, `ban.txt` and `unbanUser` (17.7): an off-LAN client's guid
/// is banned, kicked and refused retail's way until unbanned; a LAN one
/// has guid 0 and cannot be banned.
#[test]
fn ban_txt_refuses_a_banned_guid_until_unbanned() {
    const OFF: [u8; 4] = [198, 51, 100, 2];
    let guid = u32::from_be_bytes(OFF) as i32;
    let mut h = Harness::new(4, &[]);
    let _p = h.join(OFF, "\\name\\^1Bad^7Guy");
    let _q = h.join(LAN, "\\name\\lan");
    assert_eq!(console(&mut h, "banUser"), "Usage: banUser <player name>\n");
    assert_eq!(
        console(&mut h, "banClient 1"),
        "Can't ban user, GUID is 0\n"
    );
    assert_eq!(
        console(&mut h, "banUser badguy"),
        "0:^1Bad^7Guy EXE_PLAYERKICKED\n"
    );
    // The zombie's name is gone (live: `Player badguy is not on the server`).
    assert_eq!(
        console(&mut h, "banUser badguy"),
        "Player badguy is not on the server\n"
    );
    assert_eq!(
        h.sv.core.bans.unban("nobody"),
        Some(0),
        "the ban wrote a file"
    );
    assert!(h.sv.core.bans.is_banned(guid));
    assert_eq!(
        challenge_reply(&mut h, OFF),
        ["error\n\u{15}You are permanently banned from this server"]
    );
    assert_eq!(challenge_reply(&mut h, LAN), ["challengeResponse N"]);
    assert_eq!(
        console(&mut h, "unbanUser ^2BADGUY"),
        "unbanned 1 user(s) named BADGUY\n"
    );
    assert_eq!(
        console(&mut h, "unbanUser badguy"),
        "no banned user has name badguy\n"
    );
    assert_eq!(challenge_reply(&mut h, OFF), ["challengeResponse N"]);
}

/// `kick` is `SV_TempBan_f` (17.7): the guid is refused for
/// `sv_kickBanTime` seconds; `onlykick` and `clientkick` only kick.
#[test]
fn kick_bans_a_guid_for_sv_kick_ban_time() {
    const OFF: [u8; 4] = [198, 51, 100, 3];
    let guid = u32::from_be_bytes(OFF) as i32;
    let mut h = Harness::new(4, &[("sv_kickBanTime", "1")]);
    let _p = h.join(OFF, "\\name\\cheat");
    assert_eq!(
        console(&mut h, "onlykick cheat"),
        "0:cheat EXE_PLAYERKICKED\n"
    );
    assert_eq!(challenge_reply(&mut h, OFF), ["challengeResponse N"]);
    h.frames(1);
    let _p = h.join(OFF, "\\name\\cheat");
    assert_eq!(
        console(&mut h, "kick cheat"),
        format!("1:cheat EXE_PLAYERKICKED\ncheat (guid {guid}) was kicked for cheating\n")
    );
    assert_eq!(
        challenge_reply(&mut h, OFF),
        ["error\n\u{15}You are temporarily banned from this server"]
    );
    h.frames(21);
    assert_eq!(challenge_reply(&mut h, OFF), ["challengeResponse N"]);
}
