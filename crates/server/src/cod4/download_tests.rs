//! A `vcod_cod4::net::conn` client fetches a mod's iwd and mod.ff from a
//! bare level over `svc_download` and joins after the gamestate `donedl`
//! brings (cod4-handshake.md 11): temp directories only, generated files.

use super::*;
use std::net::UdpSocket;
use std::time::Duration;
use vcod_cod4::net::conn::{Conn, Cp, Join};
use vcod_cod4::net::download::{DownloadEvent, DownloadMsg, Downloads, Wanted, iwd_file_checksum};
use vcod_cod4::net::parse::Op;

/// A scratch directory under the temp dir, removed on drop.
struct Dir(PathBuf);

impl Dir {
    fn new(tag: &str) -> Dir {
        let p = std::env::temp_dir().join(format!("vcod-cod4-dl-{tag}-{}", std::process::id()));
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

/// What the client saw.
#[derive(Default)]
struct Seen {
    events: Vec<DownloadEvent>,
    blocks: usize,
    gamestates: usize,
    snapshots: usize,
}

/// A server install at `base` holding `mods/test/z_test.iwd` and
/// `mods/test/mod.ff`; a client fetches `wanted` into `home`, then moves.
fn fetch(base: &Path, home: &Path, wanted: Vec<Wanted>, sets: &[(&str, &str)]) -> Seen {
    let sock = UdpSocket::bind("127.0.0.1:0").unwrap();
    sock.set_nonblocking(true).unwrap();
    let addr = sock.local_addr().unwrap();
    let home = home.to_path_buf();
    let client = std::thread::spawn(move || {
        let mut conn = Conn::join(
            addr,
            Join {
                cp: Cp::Bare,
                ..Join::default()
            },
        )
        .unwrap();
        let mut seen = Seen::default();
        let mut dls: Option<Downloads> = None;
        let mut wanted = Some(wanted);
        let mut waiting = false;
        let end = Instant::now() + Duration::from_secs(60);
        while Instant::now() < end && seen.snapshots < 10 {
            let busy = waiting || dls.as_ref().is_some_and(|d| !d.is_done());
            let cmds = match (&conn.client.gamestate, busy) {
                (Some(_), false) => {
                    let snap = conn.client.snapshots.last().map_or(0, |s| s.server_time);
                    vec![UserCmd {
                        server_time: (snap + 10).max(conn.last_cmd().server_time + 1),
                        ..UserCmd::default()
                    }]
                }
                _ => Vec::new(),
            };
            conn.send(&cmds).unwrap();
            for got in conn
                .poll(Instant::now() + Duration::from_millis(20))
                .unwrap()
            {
                for op in &got.parsed.ops {
                    let events = match op {
                        Op::Gamestate => {
                            seen.gamestates += 1;
                            waiting = false;
                            match wanted.take() {
                                Some(w) => {
                                    let d = dls.insert(Downloads::new(&home, w));
                                    d.next_download()
                                }
                                None => Vec::new(),
                            }
                        }
                        Op::Download(m) => {
                            if matches!(m, DownloadMsg::Block { .. }) {
                                seen.blocks += 1;
                            }
                            dls.as_mut().map_or_else(Vec::new, |d| d.on_message(m))
                        }
                        Op::Snapshot(_) => {
                            seen.snapshots += 1;
                            Vec::new()
                        }
                        _ => Vec::new(),
                    };
                    for e in events {
                        match &e {
                            DownloadEvent::Command(c) if c == "donedl" => {
                                waiting = true;
                                conn.done_downloading();
                            }
                            DownloadEvent::Command(c) => {
                                conn.command(c.clone());
                            }
                            _ => {}
                        }
                        seen.events.push(e);
                    }
                }
            }
        }
        seen
    });
    let mut server = Cod4Server::new(
        Cod4Config {
            map: "mp_crash".into(),
            gametype: "war".into(),
            hostname: "vcod".into(),
            max_clients: 4,
        },
        7,
    );
    server.load(None).unwrap();
    server.game.install = Install::at(base);
    for (k, v) in sets {
        server.game.cvars_mut().set(k, v);
    }
    let mut next = Instant::now();
    let mut buf = [0u8; 2048];
    while !client.is_finished() {
        while let Ok((n, from)) = sock.recv_from(&mut buf) {
            server.handle_packet(from, &buf[..n], Instant::now());
            for (to, p) in server.take_outgoing() {
                sock.send_to(&p, to).unwrap();
            }
        }
        let now = Instant::now();
        if now >= next {
            server.tick_frames(now, 1);
            next += Duration::from_millis(FRAME_MS as u64);
            for (to, p) in server.take_outgoing() {
                sock.send_to(&p, to).unwrap();
            }
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    client.join().unwrap()
}

/// The served install: an iwd of generated filler and a fastfile of bytes.
fn base() -> (Dir, Vec<u8>, Vec<u8>) {
    let dir = Dir::new(&format!("base-{:?}", std::thread::current().id()));
    std::fs::create_dir_all(dir.0.join("zone")).unwrap();
    std::fs::create_dir_all(dir.0.join("mods/test")).unwrap();
    let filler: Vec<u8> = (0..9000u32)
        .map(|i| (i.wrapping_mul(2654435761) >> 24) as u8)
        .collect();
    let iwd = vcod_common::testing::pk3_with(&[("vcod/filler.bin", &filler)]);
    let ff: Vec<u8> = (0..3000u32).map(|i| (i * 7) as u8).collect();
    std::fs::write(dir.0.join("mods/test/z_test.iwd"), &iwd).unwrap();
    std::fs::write(dir.0.join("mods/test/mod.ff"), &ff).unwrap();
    (dir, iwd, ff)
}

fn wanted(base: &Path, ff: &[u8]) -> Vec<Wanted> {
    vec![
        Wanted {
            remote: "mods/test/z_test.iwd".into(),
            checksum: iwd_file_checksum(&base.join("mods/test/z_test.iwd")),
            size: None,
        },
        Wanted {
            remote: "mods/test/mod.ff".into(),
            checksum: None,
            size: Some(ff.len() as i64),
        },
    ]
}

#[test]
fn a_mod_downloads_and_the_client_joins_on_the_next_gamestate() {
    let (base, iwd, ff) = base();
    let home = Dir::new(&format!("home-{:?}", std::thread::current().id()));
    let seen = fetch(&base.0, &home.0, wanted(&base.0, &ff), &[]);
    let finished: Vec<&PathBuf> = seen
        .events
        .iter()
        .filter_map(|e| match e {
            DownloadEvent::Finished(_, p, _) => Some(p),
            _ => None,
        })
        .collect();
    assert_eq!(finished.len(), 2, "{:?}", seen.events);
    assert_eq!(
        std::fs::read(home.0.join("mods/test/z_test.iwd")).unwrap(),
        iwd
    );
    assert_eq!(std::fs::read(home.0.join("mods/test/mod.ff")).unwrap(), ff);
    // Block 0..n of each file and the empty block past each.
    let want = iwd.len().div_ceil(2048) + 1 + ff.len().div_ceil(2048) + 1;
    assert_eq!(seen.blocks, want, "one send a block, no retransmit");
    assert_eq!(seen.gamestates, 2);
    assert!(seen.snapshots >= 10, "{} snapshots", seen.snapshots);
}

#[test]
fn a_server_without_downloads_refuses_with_retails_key() {
    let (base, _, ff) = base();
    let home = Dir::new(&format!("home-{:?}", std::thread::current().id()));
    let seen = fetch(
        &base.0,
        &home.0,
        wanted(&base.0, &ff),
        &[("sv_allowDownload", "0")],
    );
    assert_eq!(
        seen.events.last(),
        Some(&DownloadEvent::Failed(
            "EXE_AUTODL_SERVERDISABLED_PURE\x15mods/test/z_test.iwd".into()
        )),
        "{:?}",
        seen.events
    );
    assert!(!home.0.join("mods/test/z_test.iwd").exists());
}
