//! `sv_wwwDownload`: a join's files fetched over HTTP, and its fallbacks
//! to UDP.

use vcod_cod4::net::conn::Cp;
use vcod_cod4::net::download::DownloadProgress;

use super::*;
use crate::play::cod4_menus::Join as MenuJoin;

/// A loopback HTTP server over `root`: each GET's path logged, the
/// file sent (a byte longer for one under `pad`), or 404 for one under
/// `deny` or not there, each answer `slow` in coming. It runs on the wall, so [`live_www`] holds
/// its sim clock on [`HttpServer::busy`] while an exchange is open.
fn http_server(
    root: std::path::PathBuf,
    deny: &'static [&'static str],
    pad: &'static [&'static str],
    slow: Duration,
) -> HttpServer {
    use std::io::{BufRead, Write};
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    l.set_nonblocking(true).unwrap();
    let port = l.local_addr().unwrap().port();
    let log = Arc::new(std::sync::Mutex::new(Vec::new()));
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let busy = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let (lg, st, bz) = (log.clone(), stop.clone(), busy.clone());
    std::thread::spawn(move || {
        while !st.load(std::sync::atomic::Ordering::Relaxed) {
            let Ok((sock, _)) = l.accept() else {
                std::thread::sleep(Duration::from_millis(1));
                continue;
            };
            bz.store(true, std::sync::atomic::Ordering::SeqCst);
            sock.set_nonblocking(false).unwrap();
            sock.set_read_timeout(Some(Duration::from_secs(60)))
                .unwrap();
            let mut r = std::io::BufReader::new(sock);
            let mut first = String::new();
            r.read_line(&mut first).unwrap();
            let mut line = String::new();
            while r.read_line(&mut line).unwrap() > 2 {
                line.clear();
            }
            let path = first.split_whitespace().nth(1).unwrap_or("/").to_string();
            lg.lock().unwrap().push(path.clone());
            std::thread::sleep(slow);
            let mut body = (!deny.contains(&path.as_str()))
                .then(|| std::fs::read(root.join(path.trim_start_matches('/'))).ok())
                .flatten();
            if let Some(b) = body.as_mut().filter(|_| pad.contains(&path.as_str())) {
                b.push(0);
            }
            let mut w = r.into_inner();
            let _ = match body {
                Some(b) => write!(w, "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n", b.len())
                    .and_then(|()| w.write_all(&b)),
                None => write!(w, "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n"),
            };
            // Open until the client has read it all and hung up.
            let _ = w.shutdown(std::net::Shutdown::Write);
            let _ = std::io::Read::read(&mut w, &mut [0u8; 1]);
            bz.store(false, std::sync::atomic::Ordering::SeqCst);
        }
    });
    HttpServer {
        port,
        asked: log,
        stop,
        busy,
    }
}

/// [`http_server`]'s handles.
struct HttpServer {
    port: u16,
    /// The paths asked for.
    pub(super) asked: Arc<std::sync::Mutex<Vec<String>>>,
    pub(super) stop: Arc<std::sync::atomic::AtomicBool>,
    /// An exchange is open: accepted, not yet closed by the client.
    busy: Arc<std::sync::atomic::AtomicBool>,
}

/// What [`live_www`] saw.
struct Www {
    /// The paths the HTTP server was asked for.
    pub(super) asked: Vec<String>,
    /// Every progress report.
    progress: Vec<Option<DownloadProgress>>,
    /// The files as they landed in the client's install, and as served.
    landed: Vec<(Option<Vec<u8>>, Vec<u8>)>,
    pub(super) spawned: Option<String>,
    /// The client's clock from the join's start to the spawn's look.
    took: Duration,
    /// Why the join ended before the world, if it did.
    pub(super) refused: Option<String>,
}

/// vcod-server on mp_crash under `fs_game mods/test` (a generated iwd
/// holding the stock `weapons/mp/ak47_mp`, which the load reads off it
/// so it is referenced, and a stock fastfile as its `mod.ff`), `sv_wwwDownload`
/// on with `sets` and a loopback HTTP server under `sv_wwwBaseURL` that
/// refuses `deny` and pads `pad`; a client with an empty install joins
/// through `connect_with` and spawns.
fn live_www(
    sets: &[(&str, &str)],
    deny: &'static [&'static str],
    pad: &'static [&'static str],
    slow: Duration,
) -> Option<Www> {
    use vcod_server::cod4::{Cod4Config, Cod4Server};
    let install = vcod_cod4::testing::cod4_install()?;
    let base = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    for d in ["main", "zone"] {
        #[cfg(unix)]
        std::os::unix::fs::symlink(install.root().join(d), base.path().join(d)).unwrap();
        #[cfg(windows)]
        std::os::windows::fs::symlink_dir(install.root().join(d), base.path().join(d)).unwrap();
    }
    let game = base.path().join("mods/test");
    std::fs::create_dir_all(&game).unwrap();
    let stock = vcod_common::pk3::Pk3Fs::open_iwd_layers(&install.root().join("main"), &[])
        .unwrap()
        .read("weapons/mp/ak47_mp")
        .unwrap();
    let filler: Vec<u8> = (0..20000u32)
        .map(|i| (i.wrapping_mul(2654435761) >> 24) as u8)
        .collect();
    let iwd =
        vcod_common::testing::pk3_with(&[("weapons/mp/ak47_mp", &stock), ("vcod/f.bin", &filler)]);
    std::fs::write(game.join("z_test.iwd"), &iwd).unwrap();
    let ff = std::fs::read(install.zone_dir("english").join("code_post_gfx_mp.ff")).unwrap();
    std::fs::write(game.join("mod.ff"), &ff).unwrap();

    let HttpServer {
        port,
        asked,
        stop,
        busy,
    } = http_server(base.path().to_path_buf(), deny, pad, slow);
    let _live = live_slot();
    let mut sv = Cod4Server::new(
        Cod4Config {
            map: "mp_crash".into(),
            gametype: "war".into(),
            hostname: "vcod".into(),
            max_clients: 4,
        },
        7,
    );
    sv.game.cvars_mut().set("fs_game", "mods/test");
    sv.load(Some(&vcod_cod4::install::Install::at(base.path()).unwrap()))
        .unwrap();
    sv.game.cvars_mut().set("sv_wwwDownload", "1");
    sv.game
        .cvars_mut()
        .set("sv_wwwBaseURL", &format!("http://127.0.0.1:{port}"));
    for (k, v) in sets {
        sv.game.cvars_mut().set(k, v);
    }
    let sock = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    sock.set_nonblocking(true).unwrap();
    let addr = sock.local_addr().unwrap();
    let root = home.path().to_path_buf();
    let clock = sim::Clock::new();
    let client = sim::spawn(&clock, move || {
        let join = MenuJoin {
            team: Some("allies".into()),
            class: Some("assault_mp,0".into()),
        };
        let start = clk_now();
        let mut progress = Vec::new();
        let mut report = |p| progress.push(p);
        let joined = Cod4Session::connect_with(
            addr,
            "joiner",
            Cp::Bare,
            join,
            Some(Fetch {
                root: &root,
                allow: true,
                www: true,
                progress: Some(&mut report),
                cancel: None,
                notes: None,
                max_packets: vcod_cod4::net::conn::MAX_PACKETS,
            }),
        );
        let mut s = match joined {
            Ok(s) => s,
            Err(e) => return (progress, None, since(start), Some(format!("{e:#}"))),
        };
        let t0 = clk_now();
        let mut spawned = None;
        while since(t0) < Duration::from_secs(60) && spawned.is_none() {
            s.step(since(t0).as_secs_f64() * 1000.0).unwrap();
            s.take_commands();
            spawned = s.spawn();
            clk_sleep(Duration::from_millis(16));
        }
        let took = since(start);
        s.disconnect();
        (progress, spawned, took, None)
    });
    // The fetch runs on the wall while the client's waits cost it next
    // to nothing: no server frame runs while one is in flight, or a
    // slow machine would see war's time limit pass before the spawn.
    serve(&mut sv, &sock, &clock, &client, |_, _, _| {
        let wall = Instant::now();
        while busy.load(std::sync::atomic::Ordering::SeqCst) {
            assert!(
                wall.elapsed() < Duration::from_secs(50),
                "an HTTP fetch hung"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
        true
    });
    let (progress, spawned, took, refused) = client.join().unwrap();
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    let landed = [("z_test.iwd", iwd), ("mod.ff", ff)]
        .into_iter()
        .map(|(n, b)| (std::fs::read(home.path().join("mods/test").join(n)).ok(), b))
        .collect();
    let asked = asked.lock().unwrap().clone();
    Some(Www {
        asked,
        progress,
        landed,
        spawned,
        took,
        refused,
    })
}

/// A `sv_wwwDownload` redirect is fetched over HTTP: `wwwdl ack`, the
/// GET, `wwwdl done` and the next file, then `donedl` and the join
/// (cod4-handshake.md 11). The connect screen's progress ends at each
/// file's size.
#[test]
fn cod4_live_www_redirect_fetches_over_http_and_joins() {
    let Some(w) = live_www(&[], &[], &[], Duration::ZERO) else {
        return;
    };
    assert_eq!(w.asked, ["/mods/test/z_test.iwd", "/mods/test/mod.ff"]);
    for (got, want) in &w.landed {
        assert!(got.as_ref() == Some(want), "a file did not land whole");
    }
    for (name, (_, want)) in ["mods/test/z_test.iwd", "mods/test/mod.ff"]
        .iter()
        .zip(&w.landed)
    {
        let full = DownloadProgress {
            file: name.to_string(),
            bytes: want.len() as u64,
            total: want.len() as u64,
        };
        assert!(w.progress.contains(&Some(full)), "{:?}", w.progress);
    }
    assert_eq!(w.progress.last(), Some(&None));
    assert!(w.spawned.is_some());
}

/// A fetch slow on the wall costs the join no time on the client's
/// clock: the server runs no frame while it is open, where a few wall
/// seconds once ran out war's time limit before the spawn.
#[test]
fn cod4_live_www_slow_fetch_costs_no_client_time() {
    let Some(w) = live_www(&[], &[], &[], Duration::from_secs(2)) else {
        return;
    };
    assert!(w.spawned.is_some());
    assert!(w.took < Duration::from_secs(20), "{:?}", w.took);
}

/// An HTTP failure answers `wwwdl fail`; the server sends the
/// gamestate again and the file comes over UDP (0x816fc72).
#[test]
fn cod4_live_www_failure_falls_back_to_udp() {
    let Some(w) = live_www(&[], &["/mods/test/z_test.iwd"], &[], Duration::ZERO) else {
        return;
    };
    assert_eq!(w.asked, ["/mods/test/z_test.iwd", "/mods/test/mod.ff"]);
    for (got, want) in &w.landed {
        assert!(got.as_ref() == Some(want), "a file did not land whole");
    }
    assert!(w.spawned.is_some());
}

/// `sv_wwwDlDisconnected`: `wwwdl ack` and `wwwdl bbl8r`, the server
/// drops us, the file lands and the client joins again, once a file.
#[test]
fn cod4_live_disconnected_www_reconnects_after_each_file() {
    let Some(w) = live_www(&[("sv_wwwDlDisconnected", "1")], &[], &[], Duration::ZERO) else {
        return;
    };
    assert_eq!(w.asked, ["/mods/test/z_test.iwd", "/mods/test/mod.ff"]);
    for (got, want) in &w.landed {
        assert!(got.as_ref() == Some(want), "a file did not land whole");
    }
    assert!(w.spawned.is_some());
}

/// A redirected file longer than the size the server announced fails
/// its fetch: `wwwdl fail`, and it comes whole over UDP. 1.7 keeps the
/// long file and fetches it again on every gamestate.
#[test]
fn cod4_live_www_file_past_its_size_falls_back_to_udp() {
    let Some(w) = live_www(&[], &[], &["/mods/test/mod.ff"], Duration::ZERO) else {
        return;
    };
    assert_eq!(w.asked, ["/mods/test/z_test.iwd", "/mods/test/mod.ff"]);
    for (got, want) in &w.landed {
        assert!(got.as_ref() == Some(want), "a file did not land whole");
    }
    assert!(w.spawned.is_some(), "{:?}", w.refused);
}
