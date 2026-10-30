//! A bare level on a loopback socket, joined by `vcod_cod4::net::conn` (the
//! client the probe and `vcod --game cod4` use): every datagram both ways
//! must decode and re-encode byte for byte through `vcod_cod4::net`, and a
//! pure server drops a client that sends no `cp` with retail's text
//! (cod4-handshake.md 8).

use super::*;
use std::net::UdpSocket;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use vcod_cod4::net::capture::{self, Item, Session};
use vcod_cod4::net::conn::{Conn, Cp, Join};
use vcod_cod4::net::parse::Op;

type Log = Arc<Mutex<Vec<capture::Line>>>;

/// What the client saw: the snapshots, the server commands, the log.
struct Seen {
    snapshots: usize,
    commands: Vec<String>,
    gamestate: Option<Gamestate>,
}

/// Runs a bare server on loopback while a client joins with `cp` and
/// moves until `done` holds of what it saw (or 60 s pass: the wall
/// clock bounds the wait, never what is counted).
fn join(cp: Cp, done: fn(&Seen) -> bool) -> (Seen, Vec<capture::Line>) {
    let sock = UdpSocket::bind("127.0.0.1:0").unwrap();
    sock.set_nonblocking(true).unwrap();
    let addr = sock.local_addr().unwrap();
    let log: Log = Arc::default();
    let tap_log = log.clone();
    let client = std::thread::spawn(move || {
        let t0 = Instant::now();
        let tap = Box::new(move |tag: &str, _: SocketAddr, data: &[u8]| {
            tap_log.lock().unwrap().push(capture::Line {
                ms: t0.elapsed().as_millis() as u64,
                dir: tag.to_string(),
                data: data.to_vec(),
            });
            Ok(())
        });
        let mut conn = Conn::join(
            addr,
            Join {
                cp,
                tap: Some(tap),
                ..Join::default()
            },
        )
        .unwrap();
        let mut seen = Seen {
            snapshots: 0,
            commands: Vec::new(),
            gamestate: None,
        };
        let end = Instant::now() + Duration::from_secs(60);
        while Instant::now() < end && !done(&seen) {
            // A cmd a message once the gamestate is in, as the client
            // sends from its first frame in the level.
            let cmds = match &conn.client.gamestate {
                Some(_) => {
                    let snap = conn.client.snapshots.last().map_or(0, |s| s.server_time);
                    vec![UserCmd {
                        server_time: (snap + 10).max(conn.last_cmd().server_time + 1),
                        ..UserCmd::default()
                    }]
                }
                None => Vec::new(),
            };
            conn.send(&cmds).unwrap();
            for got in conn
                .poll(Instant::now() + Duration::from_millis(50))
                .unwrap()
            {
                for op in &got.parsed.ops {
                    match op {
                        Op::Snapshot(_) => seen.snapshots += 1,
                        Op::ServerCommand { text, .. } => seen
                            .commands
                            .push(String::from_utf8_lossy(text).into_owned()),
                        _ => {}
                    }
                }
            }
        }
        seen.gamestate = conn.client.gamestate.clone();
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
    let seen = client.join().unwrap();
    let lines = std::mem::take(&mut *log.lock().unwrap());
    if std::env::var_os("COD4_JOIN_TRACE").is_some() {
        for l in &lines {
            eprintln!(
                "{} {} {} {:02x?}",
                l.ms,
                l.dir,
                l.data.len(),
                &l.data[..l.data.len().min(12)]
            );
        }
    }
    (seen, lines)
}

/// Every message of `lines` decodes to its block's last bit and re-encodes
/// to the same bytes, both directions (the capture gate's own walk,
/// `crates/cod4/tests/net_decode.rs`). Returns the server messages walked.
pub fn reencodes(lines: &[capture::Line]) -> usize {
    let mut session = Session::new();
    let mut server = 0;
    for l in lines.iter().filter(|l| l.dir != "auth") {
        match session.feed(l).unwrap() {
            Some(Item::Server { msg, parsed }) => {
                assert!(
                    parsed.exact,
                    "message {}: svc_EOF short of the block",
                    msg.sequence
                );
                let plain = session.client.write(&parsed).unwrap();
                assert_eq!(
                    session.client.huff.compress(&plain),
                    msg.block,
                    "message {}",
                    msg.sequence
                );
                server += 1;
            }
            Some(Item::Client { msg, block, key }) => {
                let again =
                    clmsg::compress_ops(&session.client.huff, &msg.ops, key, &UserCmd::default());
                assert_eq!(again, block, "client message {}", msg.sequence);
            }
            None => {}
        }
    }
    server
}

#[test]
fn a_probe_joins_a_bare_level_and_every_message_reencodes() {
    let (seen, lines) = join(Cp::Bare, |s| s.snapshots > 40);
    let gs = seen.gamestate.expect("a gamestate");
    assert_eq!(gs.systeminfo("sv_serverid").as_deref(), Some("16"));
    assert_eq!(gs.configstring(2), b"cod");
    assert!(seen.snapshots > 10, "{} snapshots", seen.snapshots);
    assert!(seen.commands.is_empty(), "{:?}", seen.commands);
    let walked = reencodes(&lines);
    assert!(walked > seen.snapshots, "{walked} messages");
    // The stats answered 126 down to 0, as retail does.
    let masks: Vec<String> = lines
        .iter()
        .filter_map(|l| l.data.strip_prefix(b"\xff\xff\xff\xffstatResponse "))
        .map(|m| String::from_utf8_lossy(m).into_owned())
        .collect();
    assert_eq!(masks, ["126", "124", "120", "112", "96", "64", "0"]);
}

#[test]
fn a_pure_server_drops_a_client_without_cp_with_retails_text() {
    let (seen, lines) = join(Cp::None, |s| !s.commands.is_empty());
    assert_eq!(
        seen.commands.first().map(String::as_str),
        Some("w \"EXE_CANNOTVALIDATEPURECLIENT\""),
        "{:?}",
        seen.commands
    );
    reencodes(&lines);
}
