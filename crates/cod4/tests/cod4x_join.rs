//! A CoD4x join against the replayed side channel of a public server that
//! authenticates clients (`auth-x21.txt`, cod4x-protocol.md 19.2).

use std::net::{SocketAddr, UdpSocket};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use vcod_cod4::net::capture;
use vcod_cod4::net::conn::{AUTH_REFUSAL, Conn, Join, Refused};
use vcod_cod4::net::reliable::{BARE_ACK, Packet};

static FIXTURE: vcod_common::manifest::ManifestPath =
    vcod_common::manifest_const!("/tests/fixtures/net/auth-x21.txt");

/// Each datagram's tap tag and bytes.
type Datagrams = Vec<(String, Vec<u8>)>;

/// A server on loopback that answers `getchallenge` and `connect` with the
/// fixture's replies, then sends the rest of its server lines at their
/// times past the `connectResponse`.
fn replay_server() -> SocketAddr {
    let text = std::fs::read_to_string(&FIXTURE).unwrap();
    let s2c: Vec<capture::Line> = capture::parse(&text)
        .unwrap()
        .into_iter()
        .filter(|l| l.dir == "s2c")
        .collect();
    let sock = UdpSocket::bind("127.0.0.1:0").unwrap();
    sock.set_read_timeout(Some(Duration::from_millis(20)))
        .unwrap();
    let addr = sock.local_addr().unwrap();
    std::thread::spawn(move || {
        let mut buf = [0u8; 2048];
        let t0 = Instant::now();
        let mut next = 0;
        let mut connected: Option<(Instant, u64, SocketAddr)> = None;
        while t0.elapsed() < Duration::from_secs(10) && next < s2c.len() {
            if let Some((at, base, client)) = connected {
                let due = at + Duration::from_millis(s2c[next].ms - base);
                std::thread::sleep(due.saturating_duration_since(Instant::now()));
                let _ = sock.send_to(&s2c[next].data, client);
                next += 1;
                continue;
            }
            let Ok((n, from)) = sock.recv_from(&mut buf) else {
                continue;
            };
            let d = &buf[..n];
            let wants = if d.starts_with(b"\xff\xff\xff\xffgetchallenge") {
                b"challengeResponse".as_slice()
            } else if d.starts_with(b"\xff\xff\xff\xffconnect ") {
                b"connectResponse"
            } else {
                continue;
            };
            let Some(i) = s2c.iter().position(|l| l.data[4..].starts_with(wants)) else {
                continue;
            };
            let _ = sock.send_to(&s2c[i].data, from);
            next = i + 1;
            if wants == b"connectResponse" {
                connected = Some((Instant::now(), s2c[i].ms, from));
            }
        }
    });
    addr
}

/// The authentication library's `cmd 8` ahead of the stats request ends
/// the join with [`AUTH_REFUSAL`]: no stats go, and `disconnect` goes in
/// three messages right after the first `cmd 8` came.
#[test]
fn an_authenticating_server_is_refused_before_the_stats() {
    let addr = replay_server();
    let seen: Arc<Mutex<Datagrams>> = Arc::default();
    let log = seen.clone();
    let mut conn = Conn::join(
        addr,
        Join {
            tap: Some(Box::new(move |tag, _, d| {
                log.lock().unwrap().push((tag.to_string(), d.to_vec()));
                Ok(())
            })),
            ..Join::default()
        },
    )
    .unwrap();
    let t0 = Instant::now();
    let e = loop {
        assert!(t0.elapsed() < Duration::from_secs(5), "no refusal");
        conn.send(&[], None).unwrap();
        if let Err(e) = conn.poll(Instant::now() + Duration::from_millis(20)) {
            break e;
        }
    };
    let Some(Refused(why)) = e.downcast_ref() else {
        panic!("{e:#}");
    };
    assert_eq!(why, AUTH_REFUSAL);
    assert_eq!(
        conn.commands().last().map(Vec::as_slice),
        Some(&b"disconnect"[..])
    );

    let seen = seen.lock().unwrap();
    let side = |d: &[u8]| Packet::is_side(d).then(|| Packet::read(d).unwrap());
    let first_auth = seen
        .iter()
        .position(|(t, d)| t == "s2c" && side(d).is_some_and(|p| p.sequence == 0))
        .expect("the first cmd 8 came");
    for (t, d) in seen.iter().filter(|(t, _)| t == "c2s") {
        if let Some(p) = side(d) {
            assert_eq!(p.sequence, BARE_ACK, "a side channel fragment went: {t}");
        }
    }
    let netchan_after = seen[first_auth + 1..]
        .iter()
        .filter(|(t, d)| t == "c2s" && side(d).is_none() && !d.starts_with(&[0xff; 4]))
        .count();
    assert_eq!(netchan_after, 3, "the disconnect's three messages");
}
