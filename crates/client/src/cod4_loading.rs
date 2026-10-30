//! A CoD4 join off the window's thread: the handshake, the downloads, the
//! gamestate and the map's load run on a worker while the window draws
//! 1.7's connect screen (`UI_DrawConnectScreen` over the `connect` menu,
//! docs/research/cod4-hud.md 18) off the [`LoadProgress`] the worker
//! writes.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Instant;

use anyhow::{Context, Result};

use crate::cod4_hud::Cod4Hud;
use crate::hud::HudQuad;
use crate::play::cod4::Cod4Session;
use vcod_cod4::net::conn::JoinNote;

/// `connstate_t` as far as the screen tells them apart.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Stage {
    /// `CA_CHALLENGING`: `getchallenge` and `connect` sent.
    #[default]
    Challenging,
    /// `CA_CONNECTED`: waiting for the gamestate (or downloading).
    AwaitingGamestate,
    /// `CA_LOADING`: the gamestate came; the map loads.
    Loading,
    /// Loaded; the window takes the session.
    Done,
}

/// A file coming down: its name, the bytes so far and of the whole, and
/// when it started (`UI_DisplayDownloadInfo`'s rate and time left).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Download {
    pub name: String,
    pub got: u64,
    pub size: u64,
    pub started: Instant,
}

/// What the connect screen draws.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Status {
    pub stage: Stage,
    /// The server as given (`cstate.servername`).
    pub server: String,
    /// `g_mapname` and `g_gametype`, once the gamestate names them.
    pub map: Option<String>,
    pub gametype: Option<String>,
    pub download: Option<Download>,
    /// `DB_GetLoadedFraction`: 0 to 1 over the map's load.
    pub loaded: f32,
    /// `clc.serverMessage`: a `print`'s text or `EXE_AWAITINGCDKEYAUTH`,
    /// drawn in yellow while the join is challenging.
    pub message: String,
}

/// The worker's status, shared with the window. A downloader feeds
/// [`LoadProgress::set_download`] as bytes come.
#[derive(Clone, Debug, Default)]
pub(crate) struct LoadProgress(Arc<Mutex<Status>>);

impl LoadProgress {
    pub fn status(&self) -> Status {
        self.0.lock().map(|s| s.clone()).unwrap_or_default()
    }

    fn with(&self, f: impl FnOnce(&mut Status)) {
        if let Ok(mut s) = self.0.lock() {
            f(&mut s);
        }
    }

    pub fn set_stage(&self, stage: Stage) {
        self.with(|s| s.stage = stage);
    }

    /// `name` has `got` of `size` bytes down; a new name starts its clock.
    // Fed by C4-45's `Fetch::progress` once that branch and this one meet.
    pub fn set_download(&self, name: &str, got: u64, size: u64) {
        self.with(|s| {
            let started = match &s.download {
                Some(d) if d.name == name => d.started,
                _ => Instant::now(),
            };
            s.download = Some(Download {
                name: name.to_string(),
                got,
                size,
                started,
            });
        });
    }

    pub fn clear_download(&self) {
        self.with(|s| s.download = None);
    }

    pub fn set_level(&self, map: String, gametype: Option<String>) {
        self.with(|s| {
            s.map = Some(map);
            s.gametype = gametype;
            s.stage = Stage::Loading;
        });
    }

    pub fn set_loaded(&self, f: f32) {
        self.with(|s| s.loaded = f.clamp(0.0, 1.0));
    }

    /// What the join tells the screen ([`JoinNote`]): a new join clears
    /// the message, `connectResponse` moves on to the gamestate's wait.
    pub fn note(&self, n: JoinNote) {
        self.with(|s| match n {
            JoinNote::Challenging => {
                s.stage = Stage::Challenging;
                s.message.clear();
            }
            JoinNote::Message(m) => s.message = m,
            JoinNote::Connected => s.stage = Stage::AwaitingGamestate,
        });
    }
}

/// `Cod4Session::connect_password` for the worker: past it the gamestate
/// has come. A download's progress goes to [`LoadProgress::set_download`]
/// through `Fetch::progress`.
fn connect(a: &JoinArgs, p: &LoadProgress, cancel: &Arc<AtomicBool>) -> Result<Cod4Session> {
    p.set_stage(Stage::Challenging);
    let mut report = |d: Option<vcod_cod4::net::download::DownloadProgress>| match d {
        Some(d) => {
            p.set_stage(Stage::AwaitingGamestate);
            p.set_download(&d.file, d.bytes, d.total);
        }
        None => p.clear_download(),
    };
    let s = Cod4Session::connect_password(
        a.addr,
        &a.name,
        &a.password,
        vcod_cod4::net::conn::Cp::Iwds(a.game_dir.join("main")),
        a.join.clone(),
        Some(crate::play::cod4::Fetch {
            root: &a.game_dir,
            allow: a.allow_download,
            www: a.www_download,
            progress: Some(&mut report),
            cancel: Some(cancel.clone()),
            notes: Some({
                let p = p.clone();
                Arc::new(move |n| p.note(n))
            }),
        }),
    )
    .map_err(|e| crate::play::cod4::localize_refusal(e, &a.game_dir))?;
    p.clear_download();
    Ok(s)
}

/// What the worker hands the window: the session and its loaded map.
pub(crate) struct Joined {
    pub session: Cod4Session,
    pub map: crate::cod4_fly::Cod4Map,
    pub map_name: String,
    pub fs_game: Option<String>,
    /// The level's HUD zones, read beside the map.
    pub hud: crate::cod4_hud::LevelZones,
}

/// The join in flight and the screen's fonts.
pub(crate) struct Loader {
    pub progress: LoadProgress,
    job: Option<JoinHandle<Result<Joined>>>,
    hud: Option<JoinHandle<Cod4Hud>>,
    loaded_hud: Option<Cod4Hud>,
    /// The levelshot's image, looked up once the map is known.
    levelshot: Option<(String, Option<String>)>,
    /// The download's time-left estimates.
    time_left: crate::cod4_hud::connect::TimeLeft,
    game_dir: PathBuf,
    /// Set by [`Loader::cancel`]; the worker's reads stop on it.
    cancel: Arc<AtomicBool>,
}

/// The join's parameters.
pub(crate) struct JoinArgs {
    pub addr: SocketAddr,
    pub server: String,
    pub name: String,
    /// The userinfo's `password`: the browser's `password` dvar, the
    /// console's `password` cvar.
    pub password: String,
    pub join: crate::play::cod4_menus::Join,
    pub game_dir: PathBuf,
    /// `cl_allowDownload` and `cl_wwwDownload`.
    pub allow_download: bool,
    pub www_download: bool,
}

impl Loader {
    /// Starts the join, and the connect screen's zones beside it.
    pub fn start(a: JoinArgs) -> Loader {
        let (server, game_dir) = (a.server.clone(), a.game_dir.clone());
        Self::start_job(server, game_dir, move |p, flag| join(a, p, flag))
    }

    /// `demo` / `timedemo` (cod4-demos.md 5): the demo read to its
    /// gamestate and its map loaded behind the same screen, `name` in the
    /// server's place.
    pub fn start_demo(name: String, data: Vec<u8>, timedemo: bool, game_dir: PathBuf) -> Loader {
        let dir = game_dir.clone();
        Self::start_job(name, game_dir, move |p, flag| {
            let s = Cod4Session::play_demo(data, timedemo)?;
            load_level(s, &dir, p, flag, None)
        })
    }

    fn start_job(
        server: String,
        game_dir: PathBuf,
        f: impl FnOnce(&LoadProgress, &Arc<AtomicBool>) -> Result<Joined> + Send + 'static,
    ) -> Loader {
        let progress = LoadProgress::default();
        progress.with(|s| s.server = server);
        let p = progress.clone();
        let cancel = Arc::new(AtomicBool::new(false));
        let flag = cancel.clone();
        let job = std::thread::spawn(move || f(&p, &flag));
        let dir = game_dir.join("main");
        let hud = std::thread::spawn(move || {
            let mut h = Cod4Hud::default();
            h.load_connect(&dir);
            h
        });
        Loader {
            progress,
            job: Some(job),
            hud: Some(hud),
            loaded_hud: None,
            levelshot: None,
            time_left: Default::default(),
            game_dir,
            cancel,
        }
    }

    /// Esc on the connect screen (`CL_Disconnect`): the worker's next read
    /// fails, and a connection it already holds sends `disconnect` and
    /// closes. The map's load is not cut short: the connection is let go
    /// once it ends (cod4-front-end.md 7). [`Loader::poll`] then reads
    /// `conn::Cancelled`; the window drops the loader without waiting.
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    /// The finished join, once.
    pub fn poll(&mut self) -> Option<Result<Joined>> {
        if !self.job.as_ref()?.is_finished() {
            return None;
        }
        let job = self.job.take()?;
        Some(
            job.join()
                .map_err(|_| anyhow::anyhow!("cod4: the join's thread panicked"))
                .and_then(|r| r),
        )
    }

    /// The connect screen for a `size` window at wall-clock ms `now`.
    pub fn quads(&mut self, size: (f32, f32), now: i32) -> Vec<HudQuad> {
        if self.hud.as_ref().is_some_and(|h| h.is_finished())
            && let Some(h) = self.hud.take()
        {
            self.loaded_hud = h.join().ok();
        }
        let st = self.progress.status();
        if let Some(map) = &st.map
            && self.levelshot.as_ref().is_none_or(|(m, _)| m != map)
        {
            let shot = crate::cod4_fly::levelshot(&self.game_dir, map);
            self.levelshot = Some((map.clone(), shot));
        }
        let shot = self.levelshot.as_ref().and_then(|(_, s)| s.as_deref());
        match &self.loaded_hud {
            Some(h) => h.connect_screen(&st, shot, size, now, &mut self.time_left),
            None => Vec::new(),
        }
    }
}

/// The worker: the handshake and gamestate ([`connect`]), then the map
/// named in it.
/// The stock zones every map loads are walked meanwhile: the gamestate
/// is seconds on the wire (its fragments rate-paced, cod4-handshake.md 3).
fn join(a: JoinArgs, p: &LoadProgress, cancel: &Arc<AtomicBool>) -> Result<Joined> {
    let t0 = std::time::Instant::now();
    let dir = a.game_dir.clone();
    let stock = std::thread::spawn(move || {
        let t0 = std::time::Instant::now();
        let z = crate::cod4_fly::walk_stock(&dir);
        vcod_common::laps::span(t0, || "cod4 stock zones".into());
        z
    });
    let session = connect(&a, p, cancel)?;
    vcod_common::laps::span(t0, || "cod4 handshake to the gamestate".into());
    // A failed walk walks again inside the load, which says why.
    let stock = stock.join().ok().and_then(Result::ok);
    load_level(session, &a.game_dir, p, cancel, stock)
}

/// The map a session's gamestate names, loaded for it.
fn load_level(
    mut session: Cod4Session,
    game_dir: &std::path::Path,
    p: &LoadProgress,
    cancel: &Arc<AtomicBool>,
    stock: Option<crate::cod4_fly::StockZones>,
) -> Result<Joined> {
    let cancelled = |s: &mut Cod4Session| {
        let gone = cancel.load(Ordering::Relaxed);
        if gone {
            s.disconnect();
        }
        gone
    };
    if cancelled(&mut session) {
        return Err(vcod_cod4::net::conn::Cancelled.into());
    }
    let map_name = session
        .map_name()
        .context("cod4: no mapname in serverinfo")?;
    p.set_level(map_name.clone(), session.gametype());
    let ents = session.entity_assets();
    let fs_game = session.fs_game();
    let report = p.clone();
    crate::cod4_fly::set_load_report(Some(Box::new(move |f| report.set_loaded(f))));
    let loaded =
        crate::cod4_fly::load_with_hud(game_dir, fs_game.as_deref(), &map_name, &ents, stock)
            .with_context(|| format!("cannot load CoD4 map '{map_name}'"));
    crate::cod4_fly::set_load_report(None);
    if cancelled(&mut session) {
        return Err(vcod_cod4::net::conn::Cancelled.into());
    }
    let (map, hud) = loaded?;
    p.set_loaded(1.0);
    p.set_stage(Stage::Done);
    Ok(Joined {
        session,
        map,
        map_name,
        fs_game,
        hud,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// The join against vcod-server's mp_crash war, through [`Loader`] as
    /// the window runs it: the connect screen draws the stage line while
    /// the gamestate is awaited, then the levelshot and a load bar that
    /// fills as the map loads, and the join hands over its session and
    /// map (cod4-hud.md 18).
    #[test]
    fn cod4_live_join_draws_the_connect_screen_then_hands_over() {
        use vcod_server::cod4::{Cod4Config, Cod4Server, FRAME_MS};
        let Some(install) = vcod_cod4::testing::cod4_install() else {
            return;
        };
        let mut sv = Cod4Server::new(
            Cod4Config {
                map: "mp_crash".into(),
                gametype: "war".into(),
                hostname: "vcod".into(),
                max_clients: 4,
            },
            7,
        );
        sv.load(Some(&install)).unwrap();
        let sock = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        sock.set_nonblocking(true).unwrap();
        let addr = sock.local_addr().unwrap();
        let game_dir = install.root().to_path_buf();
        let client = std::thread::spawn(move || {
            let mut l = Loader::start(JoinArgs {
                addr,
                server: addr.to_string(),
                name: "loader".into(),
                password: String::new(),
                join: crate::play::cod4_menus::Join::default(),
                game_dir,
                allow_download: false,
                www_download: false,
            });
            let t0 = Instant::now();
            let (mut stages, mut fills, mut shot) = (Vec::new(), Vec::new(), false);
            let done = loop {
                assert!(t0.elapsed() < Duration::from_secs(60), "no join in 60 s");
                let q = l.quads((1280.0, 960.0), t0.elapsed().as_millis() as i32);
                let st = l.progress.status();
                if stages.last() != Some(&st.stage) {
                    stages.push(st.stage);
                }
                shot |= q
                    .iter()
                    .any(|q| q.texture == "images/loadscreen_mp_crash.iwi");
                // The load bar: white, `loaded` of its texture across.
                if let Some(b) = q.iter().find(|q| q.texture == "white" && q.uvs[1][0] < 1.0) {
                    fills.push(b.uvs[1][0]);
                }
                if let Some(done) = l.poll() {
                    break done;
                }
                std::thread::sleep(Duration::from_millis(5));
            };
            (
                done.map(|j| (j.map_name, j.session.is_live())),
                stages,
                fills,
                shot,
            )
        });
        let mut next = Instant::now();
        let mut buf = [0u8; 2048];
        while !client.is_finished() {
            while let Ok((n, from)) = sock.recv_from(&mut buf) {
                sv.handle_packet(from, &buf[..n], Instant::now());
            }
            let now = Instant::now();
            if now >= next {
                sv.tick_frames(now, 1);
                next += Duration::from_millis(FRAME_MS as u64);
            }
            for (to, p) in sv.take_outgoing() {
                sock.send_to(&p, to).unwrap();
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        let (done, stages, fills, shot) = client.join().unwrap();
        println!("stages {stages:?}, {} bar frames", fills.len());
        assert_eq!(done.unwrap(), ("mp_crash".to_string(), true));
        assert!(stages.contains(&Stage::Loading), "{stages:?}");
        assert!(shot, "the levelshot never drew");
        assert!(
            fills.windows(2).all(|w| w[0] <= w[1]),
            "the bar ran back: {fills:?}"
        );
        assert!(
            fills.iter().any(|f| *f > 0.0 && *f < 1.0),
            "the bar never filled part way"
        );
    }

    fn args(addr: std::net::SocketAddr, game_dir: PathBuf) -> JoinArgs {
        JoinArgs {
            addr,
            server: addr.to_string(),
            name: "canceller".into(),
            password: String::new(),
            join: crate::play::cod4_menus::Join::default(),
            game_dir,
            allow_download: false,
            www_download: false,
        }
    }

    /// The worker's end: what `poll` hands back within `secs`.
    fn finish(l: &mut Loader, secs: u64) -> Option<Result<Joined>> {
        let t0 = Instant::now();
        while t0.elapsed() < Duration::from_secs(secs) {
            if let Some(done) = l.poll() {
                return Some(done);
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        None
    }

    /// A server's `print` while the challenge is out draws in yellow
    /// under the title, wrapped as 1.7 wraps it; a `needcdkey` puts
    /// `EXE_AWAITINGCDKEYAUTH` there (cod4-hud.md 18). The server here
    /// is a socket answering the join's `getchallenge` by hand.
    #[test]
    fn cod4_connect_screen_draws_a_print_in_yellow() {
        let Some(install) = vcod_cod4::testing::cod4_install() else {
            return;
        };
        let server = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        server
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let addr = server.local_addr().unwrap();
        let mut l = Loader::start(args(addr, install.root().to_path_buf()));
        let mut buf = [0u8; 2048];
        let (n, from) = server.recv_from(&mut buf).expect("a getchallenge");
        assert!(buf[4..n].starts_with(b"getchallenge"));
        let text = "This server is for players in the north only, please try another one";
        server
            .send_to(
                &[b"\xff\xff\xff\xffprint\n", format!("{text}\n\0").as_bytes()].concat(),
                from,
            )
            .unwrap();
        let yellow = |q: &HudQuad| q.rgba == [1.0, 1.0, 0.0, 1.0];
        let t0 = Instant::now();
        let quads = loop {
            assert!(t0.elapsed() < Duration::from_secs(20), "no yellow line");
            let q = l.quads((1280.0, 960.0), t0.elapsed().as_millis() as i32);
            if q.iter().any(yellow) {
                break q;
            }
            std::thread::sleep(Duration::from_millis(5));
        };
        assert_eq!(l.progress.status().message, format!("{text}\n"));
        // Two lines, 22 grid units apart (44 pixels at 960 high): the
        // break at the first space past 40 characters.
        // Glyph bottoms: the baselines, give or take a descender.
        let ys: Vec<f32> = quads
            .iter()
            .filter(|q| yellow(q) && q.verts[0][1] != q.verts[2][1])
            .map(|q| q.verts.iter().map(|v| v[1]).fold(f32::MIN, f32::max))
            .collect();
        let (lo, hi) = ys
            .iter()
            .fold((f32::MAX, f32::MIN), |(a, b), y| (a.min(*y), b.max(*y)));
        assert!(hi - lo > 40.0 && hi - lo < 70.0, "{lo} {hi}");
        server.send_to(b"\xff\xff\xff\xffneedcdkey", from).unwrap();
        let t0 = Instant::now();
        while l.progress.status().message != "EXE_AWAITINGCDKEYAUTH" {
            assert!(t0.elapsed() < Duration::from_secs(20), "no key wait line");
            std::thread::sleep(Duration::from_millis(5));
        }
        l.cancel();
        let _ = finish(&mut l, 20).expect("the join ends");
    }

    /// Esc while the challenge goes unanswered: the join ends at once with
    /// `Cancelled` and its socket closes, where it would have resent the
    /// challenge for 30 s.
    #[test]
    fn cod4_connect_cancel_ends_a_join_to_a_silent_server() {
        let silent = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        let addr = silent.local_addr().unwrap();
        let mut l = Loader::start(args(addr, std::env::temp_dir()));
        let mut buf = [0u8; 2048];
        silent
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let (n, from) = silent.recv_from(&mut buf).expect("a getchallenge");
        assert!(buf[4..n].starts_with(b"getchallenge"));
        l.cancel();
        // Well short of the 30 s the challenge would be resent for.
        let done = finish(&mut l, 10).expect("the join ends within 10 s of the cancel");
        let e = done.err().expect("a cancelled join fails");
        assert!(
            e.downcast_ref::<vcod_cod4::net::conn::Cancelled>()
                .is_some(),
            "{e:#}"
        );
        // The client's socket is gone: a datagram to it is refused or
        // dropped, and nothing more comes from it.
        silent
            .set_read_timeout(Some(Duration::from_millis(300)))
            .unwrap();
        let _ = silent.send_to(b"\xff\xff\xff\xffping", from);
        assert!(silent.recv_from(&mut buf).is_err(), "the join kept sending");
    }

    /// Esc once vcod-server has sent the gamestate (the map loading): the
    /// load runs out, then the client says `disconnect` and the server
    /// frees the slot (`CL_Disconnect`), instead of timing it out.
    #[test]
    fn cod4_connect_cancel_after_the_gamestate_frees_the_server_slot() {
        use vcod_server::cod4::{Cod4Config, Cod4Server, FRAME_MS};
        let Some(install) = vcod_cod4::testing::cod4_install() else {
            return;
        };
        let mut sv = Cod4Server::new(
            Cod4Config {
                map: "mp_crash".into(),
                gametype: "war".into(),
                hostname: "vcod".into(),
                max_clients: 4,
            },
            7,
        );
        sv.load(Some(&install)).unwrap();
        let sock = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        sock.set_nonblocking(true).unwrap();
        let addr = sock.local_addr().unwrap();
        let game_dir = install.root().to_path_buf();
        let client = std::thread::spawn(move || {
            let mut l = Loader::start(args(addr, game_dir));
            let t0 = Instant::now();
            while l.progress.status().stage != Stage::Loading {
                assert!(t0.elapsed() < Duration::from_secs(30), "no gamestate");
                std::thread::sleep(Duration::from_millis(2));
            }
            l.cancel();
            finish(&mut l, 60).expect("the join ends")
        });
        let mut next = Instant::now();
        let mut buf = [0u8; 2048];
        let mut most = 0;
        let mut finished_at: Option<Instant> = None;
        let mut freed = false;
        let t0 = Instant::now();
        while t0.elapsed() < Duration::from_secs(90) {
            while let Ok((n, from)) = sock.recv_from(&mut buf) {
                sv.handle_packet(from, &buf[..n], Instant::now());
            }
            let now = Instant::now();
            if now >= next {
                sv.tick_frames(now, 1);
                next += Duration::from_millis(FRAME_MS as u64);
            }
            for (to, p) in sv.take_outgoing() {
                let _ = sock.send_to(&p, to);
            }
            most = most.max(sv.client_count());
            if client.is_finished() {
                let at = *finished_at.get_or_insert_with(Instant::now);
                if sv.client_count() == 0 {
                    freed = true;
                    break;
                }
                // The disconnect lands in a frame or two; a timeout takes
                // `sv_connectTimeout`'s 45 s.
                if at.elapsed() > Duration::from_secs(20) {
                    break;
                }
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        let done = client.join().unwrap();
        let e = done.err().expect("a cancelled join fails");
        assert!(
            e.downcast_ref::<vcod_cod4::net::conn::Cancelled>()
                .is_some(),
            "{e:#}"
        );
        assert_eq!(most, 1, "the client was in the slot table");
        assert!(freed, "the server kept the cancelled client past 20 s");
    }
}
