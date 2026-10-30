//! The www download redirect on protocol 22 (cod15-protocol-diff.md 14):
//! vcod's client asks the server for a pak, is sent to an HTTP server on
//! loopback, fetches it there and finishes the handshake.

use crate::common;

use common::{Queues, connect, step};
use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};
use vcod_common::net::protocol::{PROTOCOL_V6, PROTOCOL_V22, Protocol};
use vcod_common::net::{NetEvent, NetState};
use vcod_server::Server;

/// Serves `body` to every connection on a loopback port; returns the port
/// and the request lines it read.
fn http_server(body: Vec<u8>) -> (u16, std::sync::Arc<std::sync::Mutex<Vec<String>>>) {
    use std::io::{BufRead, Write};
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let log = seen.clone();
    std::thread::spawn(move || {
        for s in l.incoming() {
            let Ok(mut s) = s else { return };
            let mut line = String::new();
            let _ = std::io::BufReader::new(&s).read_line(&mut line);
            log.lock().unwrap().push(line.trim_end().to_string());
            let head = format!("HTTP/1.0 200 OK\r\nContent-Length: {}\r\n\r\n", body.len());
            let _ = s.write_all(head.as_bytes());
            let _ = s.write_all(&body);
        }
    });
    (port, seen)
}

#[test]
fn a_redirected_download_lands_over_http_and_the_gamestate_follows() {
    let scratch = std::env::temp_dir().join(format!("vcod-www-e2e-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&scratch);
    let served = scratch.join("server/main");
    std::fs::create_dir_all(&served).unwrap();
    let pak = vcod_common::testing::pk3_bytes();
    std::fs::write(served.join("zzz_www.pk3"), &pak).unwrap();
    let (port, requests) = http_server(pak.clone());

    let mut sv = Server::new(common::cfg("mp_carentan", "dm"), Instant::now());
    sv.set_protocol(&PROTOCOL_V22);
    sv.test_mount_paks(Rc::new(
        vcod_common::pk3::Pk3Fs::open(&served).expect("the served pak"),
    ));
    sv.set_cvar("sv_wwwDownload", "1");
    sv.set_cvar("sv_wwwBaseURL", &format!("http://127.0.0.1:{port}"));

    let q = Rc::new(RefCell::new(Queues::default()));
    let mut now = Instant::now();
    let mut cl = connect(&mut sv, &q, &mut now);
    assert_eq!(
        cl.protocol().transport.version,
        PROTOCOL_V22.transport.version
    );
    let dest = scratch.join("client/main/zzz_www.pk3");
    cl.begin_download("main/zzz_www.pk3", &dest).unwrap();

    let deadline = std::time::Instant::now() + Duration::from_secs(60);
    let mut landed = false;
    while !landed {
        assert!(std::time::Instant::now() < deadline, "no download");
        now += Duration::from_millis(50);
        for e in step(&mut sv, &q, &mut cl, now) {
            match e {
                NetEvent::DownloadComplete(n) => {
                    assert_eq!(n, "main/zzz_www.pk3");
                    landed = true;
                }
                NetEvent::Dropped(r) => panic!("dropped: {r}"),
                _ => {}
            }
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(std::fs::read(&dest).unwrap(), pak);
    assert_eq!(
        *requests.lock().unwrap(),
        ["GET /main/zzz_www.pk3 HTTP/1.0"]
    );

    // `wwwdl done`, then `donedl` brings the gamestate again.
    cl.finish_downloads();
    let mut again = false;
    for _ in 0..40 {
        now += Duration::from_millis(50);
        let events = step(&mut sv, &q, &mut cl, now);
        assert!(
            !events.iter().any(|e| matches!(e, NetEvent::Dropped(_))),
            "{events:?}"
        );
        again |= events.contains(&NetEvent::GamestateReady);
    }
    assert!(again, "no gamestate after donedl");
    assert_ne!(cl.state(), NetState::Disconnected);
    let _ = std::fs::remove_dir_all(&scratch);
}

/// A server on `proto` holding `zzz_www.pk3` with `sv_wwwDownload` on and
/// another pak of that name behind its `sv_wwwBaseURL`, and a connected
/// client asking for it. Returns the HTTP request log too.
/// What [`wrong_pak_at_the_url`] sets up.
struct WrongPak {
    sv: Server,
    q: Rc<RefCell<Queues>>,
    cl: vcod_common::net::NetClient<common::ClientEnd>,
    now: Instant,
    requests: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    scratch: std::path::PathBuf,
}

fn wrong_pak_at_the_url(proto: &'static Protocol, tag: &str) -> WrongPak {
    let scratch = std::env::temp_dir().join(format!("vcod-www-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&scratch);
    let served = scratch.join("server/main");
    std::fs::create_dir_all(&served).unwrap();
    std::fs::write(
        served.join("zzz_www.pk3"),
        vcod_common::testing::pk3_bytes(),
    )
    .unwrap();
    let wrong = vcod_common::testing::pk3_with(&[("maps/mp/foo.bsp", b"IBSX")]);
    let (port, requests) = http_server(wrong);

    let mut sv = Server::new(common::cfg("mp_carentan", "dm"), Instant::now());
    sv.set_protocol(proto);
    sv.test_mount_paks(Rc::new(
        vcod_common::pk3::Pk3Fs::open(&served).expect("the served pak"),
    ));
    sv.set_cvar("sv_wwwDownload", "1");
    sv.set_cvar("sv_wwwBaseURL", &format!("http://127.0.0.1:{port}"));
    sv.set_cvar("sv_wwwDlDisconnected", "0");

    let q = Rc::new(RefCell::new(Queues::default()));
    let mut now = Instant::now();
    let mut cl = connect(&mut sv, &q, &mut now);
    assert_eq!(cl.protocol().transport.version, proto.transport.version);
    let dest = scratch.join("client/main/zzz_www.pk3");
    cl.begin_download("main/zzz_www.pk3", &dest).unwrap();
    WrongPak {
        sv,
        q,
        cl,
        now,
        requests,
        scratch,
    }
}

/// 1.5b, connected, the wrong pak behind the URL: the client says `wwwdl
/// chkfail`, the server resends the gamestate and streams the re-request
/// over UDP with no second redirect. Live on cod_lnxded 1.5b, 2026-10-20
/// (`fixtures/handshake/www-chkfail-15.txt`; cod15-protocol-diff.md 14).
#[test]
fn a_chkfail_on_1_5_falls_back_to_udp() {
    let WrongPak {
        mut sv,
        q,
        mut cl,
        mut now,
        requests,
        scratch,
    } = wrong_pak_at_the_url(&PROTOCOL_V6, "chkfail15");
    let deadline = std::time::Instant::now() + Duration::from_secs(60);
    let mut landed = false;
    while !landed {
        assert!(std::time::Instant::now() < deadline, "no download");
        now += Duration::from_millis(50);
        for e in step(&mut sv, &q, &mut cl, now) {
            match e {
                NetEvent::DownloadComplete(n) => {
                    assert_eq!(n, "main/zzz_www.pk3");
                    landed = true;
                }
                NetEvent::Dropped(r) => panic!("dropped: {r}"),
                _ => {}
            }
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    let dest = scratch.join("client/main/zzz_www.pk3");
    assert_eq!(
        std::fs::read(&dest).unwrap(),
        vcod_common::testing::pk3_bytes(),
        "the server's own pak, over UDP"
    );
    assert_eq!(
        *requests.lock().unwrap(),
        ["GET /main/zzz_www.pk3 HTTP/1.0"],
        "one www fetch"
    );
    let _ = std::fs::remove_dir_all(&scratch);
}

/// United Offensive, connected (`sv_wwwDlDisconnected 0`), the wrong pak
/// behind the URL: `chkfail`, the gamestate again, the re-request
/// redirected again, which vcod's client refuses with `wwwdl fail` (the
/// name is on its bad-checksum list); the server redirects a third time
/// and the client leaves. No block ever goes over UDP. Live on coduo_lnxded
/// 1.51b, 2026-10-20 (`fixtures/handshake/www-refuse-uo.txt`; cod15-protocol-diff.md
/// 14).
#[test]
fn a_refused_redirect_on_uo_is_redirected_again_until_the_client_leaves() {
    let WrongPak {
        mut sv,
        q,
        mut cl,
        mut now,
        requests,
        scratch,
    } = wrong_pak_at_the_url(&PROTOCOL_V22, "refuseuo");
    let deadline = std::time::Instant::now() + Duration::from_secs(60);
    let reason = loop {
        assert!(
            std::time::Instant::now() < deadline,
            "the client never left"
        );
        now += Duration::from_millis(50);
        let events = step(&mut sv, &q, &mut cl, now);
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, NetEvent::DownloadComplete(_))),
            "a pak landed: {events:?}"
        );
        if let Some(r) = events.iter().find_map(|e| match e {
            NetEvent::Dropped(r) => Some(r.clone()),
            _ => None,
        }) {
            break r;
        }
        std::thread::sleep(Duration::from_millis(5));
    };
    assert!(reason.contains("keeps redirecting"), "{reason}");
    assert_eq!(
        *requests.lock().unwrap(),
        ["GET /main/zzz_www.pk3 HTTP/1.0"],
        "one www fetch; the refused redirect fetches nothing"
    );
    assert!(!scratch.join("client/main/zzz_www.pk3").exists());
    let _ = std::fs::remove_dir_all(&scratch);
}
