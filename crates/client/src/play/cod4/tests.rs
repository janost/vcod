//! The replay gate: the committed captures through the session's frame
//! path (step, events, scene) at 60 frames a second of the capture's
//! clock, as `cod4_client` runs them.

use super::*;
use std::collections::BTreeMap;

use vcod_cod4::net::conn::Cp;

use super::scene::crosshair_scan;
use crate::play::cod4_menus::Join as MenuJoin;

mod hud_menus;

mod capture;
mod capture_bodies;
mod capture_bullets;
mod capture_view;
mod cod4x;
mod demo;
mod join;
mod live_hud;
mod live_play;
mod live_rounds;
mod live_session;
mod live_spectate;
mod live_streaks;
mod live_view;
mod ragdoll;
mod voice;
mod www;

/// How many live CoD4 servers this test binary runs at once: each holds
/// a loaded map, a client and its session, about 2.5 GB at the peak
/// (docs/dev/testing.md, "CoD4 live tests"). `VCOD_LIVE_SLOTS` sets it.
fn live_slots() -> usize {
    static N: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *N.get_or_init(|| {
        std::env::var("VCOD_LIVE_SLOTS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(3)
            .max(1)
    })
}

static LIVE: (std::sync::Mutex<usize>, std::sync::Condvar) =
    (std::sync::Mutex::new(0), std::sync::Condvar::new());

thread_local! {
    /// This thread holds a slot: a test's second server rides its first's.
    static HOLDS: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// A slot of [`live_slots`], released on drop.
pub(crate) struct LiveSlot(bool);

/// Waits for a free slot before a test builds a live server, so a run
/// on every core holds at most [`live_slots`] maps whatever its
/// `--test-threads`. Taken before the sim clock starts: a wait inside
/// one would trip its stall asserts.
pub(crate) fn live_slot() -> LiveSlot {
    if HOLDS.get() {
        return LiveSlot(false);
    }
    let mut n = LIVE.0.lock().unwrap_or_else(|e| e.into_inner());
    while *n >= live_slots() {
        n = LIVE.1.wait(n).unwrap_or_else(|e| e.into_inner());
    }
    *n += 1;
    HOLDS.set(true);
    LiveSlot(true)
}

impl Drop for LiveSlot {
    fn drop(&mut self) {
        if self.0 {
            HOLDS.set(false);
            *LIVE.0.lock().unwrap_or_else(|e| e.into_inner()) -= 1;
            LIVE.1.notify_one();
        }
    }
}

/// A server command whose sequence leaps past the reliable ring
/// drops the session as `EXE_ERR_RELIABLE_CYCLED_OUT`, the gap never
/// walked.
#[test]
fn a_command_past_the_reliable_ring_drops_the_session() {
    use vcod_cod4::net::parse::{Gamestate, SVC_EOF, SVC_SERVERCOMMAND, UNUSED_HUFF};
    use vcod_common::net::codec::MsgWriter;
    let huff = vcod_cod4::net::huffman::Huffman::new();
    let gs = Gamestate {
        server_command_sequence: 3,
        ..Default::default()
    };
    let opening = vcod_cod4::net::demo::gamestate_message(&huff, 0, 3, &gs);
    let mut demo = vec![0];
    demo.extend(1i32.to_le_bytes());
    demo.extend((opening.len() as i32).to_le_bytes());
    demo.extend(&opening);
    let mut s = Cod4Session::play_demo(demo, false).unwrap();
    let command = |seq: i32| {
        let mut w = MsgWriter::new(&UNUSED_HUFF);
        w.write_byte(SVC_SERVERCOMMAND);
        w.write_long(seq);
        w.write_string_bytes(b"x");
        w.write_byte(SVC_EOF);
        huff.compress(&w.into_ops())
    };
    let feed = |s: &mut Cod4Session, seq: i32| {
        let Source::Demo(d) = &mut s.source else {
            unreachable!()
        };
        d.client.parse(2, &command(seq)).unwrap();
        s.queue_commands()
    };
    assert_eq!(feed(&mut s, 4), [b"x".to_vec()]);
    assert!(s.take_dropped().is_none());
    assert!(feed(&mut s, i32::MAX).is_empty());
    assert_eq!(
        s.take_dropped().as_deref(),
        Some("EXE_ERR_RELIABLE_CYCLED_OUT")
    );
}

/// `CG_ScanForCrosshairEntity`'s trace: a client under 64 the point
/// met, a teammate by team, none while we spectate, nothing on the
/// world.
#[test]
fn the_crosshair_scan_names_the_client_it_meets() {
    struct Hits(u32);
    impl vcod_cod4::pmove::PmWorld for Hits {
        fn trace(&self, start: Vec3, end: Vec3, _: Vec3, _: Vec3, pass: u32, mask: u32) -> pm::Tr {
            assert_eq!((pass, mask), (2, 0x280_3001));
            // straight ahead along +x, 8192 out
            assert!((end - start - Vec3::X * 8192.0).length() < 0.01);
            pm::Tr {
                entity_num: self.0,
                ..Default::default()
            }
        }
    }
    let team = |n: u32| match n {
        2 | 7 => 1,
        _ => 2,
    };
    let at = |_| Some(Vec3::new(300.0, 0.0, 60.0));
    let eye = Vec3::new(0.0, 0.0, 60.0);
    let scan = |hit, own| crosshair_scan(&Hits(hit), eye, Vec3::ZERO, own, team, at);
    let enemy = scan(9, 2).unwrap();
    assert_eq!((enemy.client, enemy.friendly), (9, Some(false)));
    assert_eq!(enemy.dist_sq, 90_000.0);
    assert_eq!(scan(7, 2).unwrap().friendly, Some(true));
    assert_eq!(scan(1022, 2), None);
    assert_eq!(scan(64, 2), None);
    let spectating = |n: u32| if n == 2 { 3 } else { 1 };
    let s = crosshair_scan(&Hits(9), eye, Vec3::ZERO, 2, spectating, at).unwrap();
    assert_eq!(s.friendly, None);
}

/// The clock a live test's client thread and its in-process server
/// share, so the test runs as fast as both compute instead of in real
/// time. Only the client moves it, with every sleep ([`clk_sleep`]),
/// and each sleep waits until [`serve`] has read every packet sent
/// before and run every server frame due by then; the session's own
/// clock reads it ([`clock_now`]), so the two run in lockstep however
/// slow a frame is or however long the client computes between
/// sleeps (an asset load). The join waits and times out on it too, a
/// frame a wait (`conn::set_thread_clock`), so a client wait that
/// does not sleep on it never ends: [`Clock::turn`] fails one that
/// leaves the clock still for [`STALL`].
pub(crate) mod sim {
    use std::cell::RefCell;
    use std::sync::{Arc, Condvar, Mutex};
    use std::time::{Duration, Instant};

    /// The wall time the client may leave the clock still.
    pub(crate) const STALL: Duration = Duration::from_secs(600);

    #[derive(Clone)]
    pub(crate) struct Clock(Arc<(Mutex<State>, Condvar)>);

    struct State {
        /// The clock.
        now: Instant,
        /// What the server has run up to.
        served: Instant,
    }

    thread_local! {
        static CLOCK: RefCell<Option<Clock>> = const { RefCell::new(None) };
    }

    impl Clock {
        pub(crate) fn new() -> Clock {
            let t = Instant::now();
            let st = State { now: t, served: t };
            Clock(Arc::new((Mutex::new(st), Condvar::new())))
        }

        pub(crate) fn now(&self) -> Instant {
            self.0.0.lock().unwrap().now
        }

        /// Moves the clock on by `d` and waits for the server to catch
        /// up to it.
        fn sleep(&self, d: Duration) {
            let (m, cv) = &*self.0;
            let mut st = m.lock().unwrap();
            st.now += d;
            cv.notify_all();
            let want = st.now;
            let real = Instant::now();
            while st.served < want {
                assert!(
                    real.elapsed() < Duration::from_secs(60),
                    "the server stopped serving the clock"
                );
                st = cv.wait_timeout(st, Duration::from_millis(100)).unwrap().0;
            }
        }

        /// The server's turn: waits for the client to move the clock,
        /// and returns the time to run up to, or `None` once
        /// `finished` says the client is done.
        pub(crate) fn turn(&self, finished: impl Fn() -> bool) -> Option<Instant> {
            let (m, cv) = &*self.0;
            let mut st = m.lock().unwrap();
            let real = Instant::now();
            while st.served >= st.now {
                if finished() {
                    return None;
                }
                assert!(
                    real.elapsed() < STALL,
                    "the client left the clock still for {STALL:?}: a wait that does not sleep on it"
                );
                st = cv.wait_timeout(st, Duration::from_millis(50)).unwrap().0;
            }
            Some(st.now)
        }

        /// The server has run up to `t`.
        pub(crate) fn served(&self, t: Instant) {
            let (m, cv) = &*self.0;
            let mut st = m.lock().unwrap();
            st.served = st.served.max(t);
            cv.notify_all();
        }
    }

    /// A client thread on `clock`.
    pub(crate) fn spawn<T: Send + 'static>(
        clock: &Clock,
        f: impl FnOnce() -> T + Send + 'static,
    ) -> std::thread::JoinHandle<T> {
        let c = clock.clone();
        std::thread::spawn(move || {
            install(Some(c));
            f()
        })
    }

    /// Puts this thread on `clock` (`None`: back on the wall), the
    /// join's waits and deadlines too, a frame a wait as a client
    /// reads its packets once a frame.
    pub(crate) fn install(clock: Option<Clock>) {
        CLOCK.with(|x| *x.borrow_mut() = clock);
        vcod_cod4::net::conn::set_thread_clock(super::clk_now, |_| {
            super::clk_sleep(Duration::from_millis(16))
        });
    }

    /// This thread's clock, for a thread it starts to run on too.
    pub(crate) fn current() -> Option<Clock> {
        CLOCK.with(|c| c.borrow().clone())
    }

    /// This thread's clock, if it runs on one.
    pub(crate) fn now() -> Option<Instant> {
        CLOCK.with(|c| c.borrow().as_ref().map(Clock::now))
    }

    pub(crate) fn sleep(d: Duration) -> bool {
        CLOCK.with(|c| c.borrow().as_ref().map(|c| c.sleep(d)).is_some())
    }
}

/// Now on this thread's [`sim`] clock, else the wall clock.
pub(crate) fn clk_now() -> Instant {
    sim::now().unwrap_or_else(Instant::now)
}

/// Sleeps on this thread's [`sim`] clock, else the wall clock.
pub(crate) fn clk_sleep(d: Duration) {
    if !sim::sleep(d) {
        std::thread::sleep(d);
    }
}

/// [`Instant::elapsed`] on [`clk_now`].
pub(crate) fn since(t: Instant) -> Duration {
    clk_now().saturating_duration_since(t)
}

/// What [`serve`] runs a game's in-process server through.
pub(crate) trait Held {
    /// Ahead of a turn's packets.
    fn turn(&mut self) {}
    /// One server frame at `at`.
    fn frame(&mut self, at: Instant);
    fn frame_ms(&self) -> u64;
    fn handle_packet(&mut self, from: std::net::SocketAddr, pkt: &[u8], now: Instant);
    fn take_outgoing(&mut self) -> Vec<(std::net::SocketAddr, Vec<u8>)>;
}

impl Held for vcod_server::cod4::Cod4Server {
    fn turn(&mut self) {
        // The bots' graph lands on the frame that asks for it, not
        // when a build thread is done.
        if let Some(b) = self.bots_mut() {
            b.wait_for_graph();
        }
    }

    fn frame(&mut self, at: Instant) {
        self.tick_frames(at, 1);
    }

    fn frame_ms(&self) -> u64 {
        vcod_server::cod4::Cod4Server::frame_ms(self) as u64
    }

    fn handle_packet(&mut self, from: std::net::SocketAddr, pkt: &[u8], now: Instant) {
        vcod_server::cod4::Cod4Server::handle_packet(self, from, pkt, now);
    }

    fn take_outgoing(&mut self) -> Vec<(std::net::SocketAddr, Vec<u8>)> {
        vcod_server::cod4::Cod4Server::take_outgoing(self)
    }
}

impl Held for vcod_server::cod2::Cod2Server {
    fn turn(&mut self) {
        if let Some(b) = self.bots_mut() {
            b.wait_for_graph();
        }
    }

    /// The binary's frame: `Cbuf_Execute`, then `SV_Frame`.
    fn frame(&mut self, at: Instant) {
        self.drain_console(at);
        self.tick_frames(at, 1);
    }

    fn frame_ms(&self) -> u64 {
        vcod_server::cod2::FRAME_MS as u64
    }

    fn handle_packet(&mut self, from: std::net::SocketAddr, pkt: &[u8], now: Instant) {
        vcod_server::cod2::Cod2Server::handle_packet(self, from, pkt, now);
    }

    fn take_outgoing(&mut self) -> Vec<(std::net::SocketAddr, Vec<u8>)> {
        vcod_server::cod2::Cod2Server::take_outgoing(self)
    }
}

/// An in-process server's loop under `client`, on `clock`: each turn
/// reads the packets the client sent (stamped with the time it sent
/// them), runs every frame ([`Held::frame_ms`]) due by the clock and
/// sends what they queued. `tick` runs ahead of each frame, at its
/// time, between the packets' cmds and the frame (it may move the next
/// frame's time); once it returns false the server goes quiet for
/// good: nothing read, run or sent.
pub(crate) fn serve<S: Held, T>(
    sv: &mut S,
    sock: &std::net::UdpSocket,
    clock: &sim::Clock,
    client: &std::thread::JoinHandle<T>,
    tick: impl FnMut(&mut S, Instant, &mut Instant) -> bool,
) {
    serve_until(sv, sock, clock, || client.is_finished(), tick);
}

/// [`serve`] until `finished`.
pub(crate) fn serve_until<S: Held>(
    sv: &mut S,
    sock: &std::net::UdpSocket,
    clock: &sim::Clock,
    finished: impl Fn() -> bool,
    mut tick: impl FnMut(&mut S, Instant, &mut Instant) -> bool,
) {
    let mut buf = [0u8; 4096];
    let mut quiet = false;
    let mut next = clock.now();
    let mut heard = next;
    while let Some(until) = clock.turn(&finished) {
        sv.turn();
        while !quiet && let Ok((n, from)) = sock.recv_from(&mut buf) {
            sv.handle_packet(from, &buf[..n], heard);
        }
        while !quiet && next <= until {
            let at = next;
            if !tick(sv, at, &mut next) {
                quiet = true;
                break;
            }
            sv.frame(at);
            next += Duration::from_millis(sv.frame_ms());
            for (to, p) in sv.take_outgoing() {
                let _ = sock.send_to(&p, to);
            }
        }
        // The connectionless answers the packets queued.
        for (to, p) in sv.take_outgoing().into_iter().filter(|_| !quiet) {
            let _ = sock.send_to(&p, to);
        }
        heard = until;
        clock.served(until);
    }
}

/// `client` against `sv` on one fresh [`sim`] clock, through
/// [`serve`]: its frames, the server's frames and every packet between
/// them land on the same times run to run whatever the wall does.
/// `tick` is [`serve`]'s.
pub(crate) fn run_held<S: Held, T: Send + 'static>(
    sv: &mut S,
    client: impl FnOnce(std::net::SocketAddr) -> T + Send + 'static,
    tick: impl FnMut(&mut S, Instant, &mut Instant) -> bool,
) -> T {
    let sock = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    sock.set_nonblocking(true).unwrap();
    let addr = sock.local_addr().unwrap();
    let clock = sim::Clock::new();
    let handle = sim::spawn(&clock, move || client(addr));
    serve(sv, &sock, &clock, &handle, tick);
    handle.join().unwrap()
}

const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../cod4/tests/fixtures/net");

/// What the events of a capture play (`cod4_audio`, `cod4_fx`).
#[derive(Default, Debug)]
struct AvCounts {
    /// Cues by alias, and the aliases no zone holds.
    sounds: BTreeMap<String, usize>,
    sounds_missing: std::collections::BTreeSet<String>,
    /// Effects by name, and the names no zone holds.
    effects: BTreeMap<String, usize>,
    effects_missing: std::collections::BTreeSet<String>,
    quakes: Vec<crate::play::quake::Quake>,
}

/// `name` through the session's frame path with the zones' sounds and
/// effects (`COD4_DIR`; `None` without it): each fired event's cues and
/// effects, counted as `cod4_client` plays them.
fn replay_av(name: &str) -> Option<AvCounts> {
    let install = vcod_cod4::testing::cod4_install()?;
    let ((mut sounds, mut effects), _) =
        crate::cod4_fly::load_av(install.root(), "mp_crash").unwrap();
    let text = std::fs::read_to_string(format!("{FIXTURES}/{name}")).unwrap();
    let mut s = Cod4Session::open_capture(&text).unwrap();
    let mut n = AvCounts::default();
    let mut ms = 0.0;
    while !s.finished() {
        ms += 1000.0 / 60.0;
        s.step(ms).unwrap();
        let fired = s.take_fired();
        let Some(gs) = s.gamestate() else { continue };
        let weapons = weapon_list(gs);
        let cs = |k: u16| String::from_utf8_lossy(gs.configstring(k)).into_owned();
        for f in &fired {
            for c in sounds.cues(f, &weapons, &cs) {
                *n.sounds.entry(c.alias).or_default() += 1;
            }
            for p in effects.plays(f, &weapons, &cs, &HashMap::new(), None) {
                *n.effects.entry(p.key).or_default() += 1;
            }
            n.quakes.extend(crate::cod4_fx::earthquake(f));
        }
    }
    n.sounds_missing = sounds.missing.iter().cloned().collect();
    n.effects_missing = effects.missing.iter().cloned().collect();
    assert_eq!(sounds.started, n.sounds.values().sum::<usize>());
    assert_eq!(effects.spawned, n.effects.values().sum::<usize>());
    println!("{name}: {n:#?}");
    println!("element types left out: {:?}", effects.gaps);
    Some(n)
}

/// Per shot the hit's distance and surface, the impacts by name, the
/// whizbys.
type Bullets = (Vec<(f32, u32)>, BTreeMap<String, usize>, usize);

/// The first round of [`crate::cod4_fx::client_bullets`].
fn first_bullet(
    f: &Fired,
    weapons: &[String],
    defs: &HashMap<String, crate::cod4_audio::WeaponAv>,
    tags: &HashMap<u32, crate::play::cod4_players::WeaponTags>,
    own: Option<&crate::cod4_fx::OwnView>,
    clip: &vcod_cod4::collision::ClipMap,
    solids: &[Solid],
) -> Option<crate::cod4_fx::BulletHit> {
    let world = crate::cod4_fx::ShotWorld { clip, solids };
    crate::cod4_fx::client_bullets(f, weapons, defs, tags, own, world)
        .into_iter()
        .next()
}

/// mp_crash's prediction world, loaded once.
fn crash_world() -> Option<Arc<Cod4World>> {
    static WORLD: std::sync::OnceLock<Option<Arc<Cod4World>>> = std::sync::OnceLock::new();
    WORLD
        .get_or_init(|| {
            let install = vcod_cod4::testing::cod4_install()?;
            let w = crate::cod4_fly::load_world(install.root(), "mp_crash").unwrap();
            Some(Arc::new(w))
        })
        .clone()
}

/// A 60 Hz frame grid for a live test's loop: [`FrameClock::wait`]
/// sleeps to the next 16 ms mark, so a frame's own work does not
/// stretch the frame as a fixed sleep after it would.
struct FrameClock(Instant);

impl FrameClock {
    fn new() -> FrameClock {
        FrameClock(clk_now())
    }

    fn wait(&mut self) {
        self.0 += Duration::from_millis(16);
        let now = clk_now();
        match self.0.checked_duration_since(now) {
            Some(d) => clk_sleep(d),
            // Behind: the next frame at once, the grid from here.
            None => self.0 = now,
        }
    }
}

/// vcod-server's CoD4 `war` on mp_crash in process (`COD4_DIR`;
/// `None` without), on a loopback socket: the client joins with
/// `--team allies --class assault_mp,0` and steps until it spawns (or
/// 60 s pass), then `play` runs on the session with the spawn's
/// weapons and the clock its steps read; the client disconnects after.
fn live_join<T: Send + 'static>(
    play: impl FnOnce(&mut Cod4Session, Option<String>, Instant) -> T + Send + 'static,
) -> Option<T> {
    live_join_ctl(move |s, spawned, t0, _| play(s, spawned, t0))
}

/// What a [`live_join_ctl`] client asks of the server's loop.
enum Ctl {
    /// A server command to slot 0.
    Command(String),
    /// The server stops: no packet read, frame run or sent.
    Silence,
    /// A live frag of slot 0's lies at the point with the fuse left.
    DropFrag([f32; 3], i32),
    /// An axis bot joins, alone and held still, and is kept standing at
    /// the point (its feet), facing west, at full health.
    PinBot([f32; 3]),
    /// [`Ctl::PinBot`]'s bot at his own health, let go once dead.
    PinVictim([f32; 3]),
    /// [`Ctl::PinBot`]'s bot, fighting.
    PinShooter([f32; 3]),
    /// An axis bot joins, is put with his eye at the point and uses
    /// the gun entity, as the use key would, until he is on it.
    MountBot([f32; 3], u32),
    /// Slot 0's rounds the server fired so far: game time, start, the
    /// unspread end along the view, end.
    Shots(std::sync::mpsc::Sender<Vec<(i32, Vec3, Vec3, Vec3)>>),
    /// Slot 0's `commandTime` and view angles on the server, sent back.
    Probe(std::sync::mpsc::Sender<(i32, [f32; 3])>),
    /// Slot 0's stats block on the server, sent back.
    Stats(std::sync::mpsc::Sender<vcod_cod4::net::stats::Stats>),
    /// A bot joins; its name, sent back.
    AddBot(std::sync::mpsc::Sender<String>),
    /// A line for the server console.
    Console(String),
    /// Slot 0 takes the damage along the direction (none: from no
    /// direction), without knockback and his health left alone.
    Hurt(i32, Option<[f32; 3]>),
    /// The script models a round meets: number, origin, open to damage.
    Models(std::sync::mpsc::Sender<ModelList>),
    /// Where each line first meets the world or a script model on the
    /// server, and the model's number.
    ModelTrace(Vec<(Vec3, Vec3)>, std::sync::mpsc::Sender<ModelHits>),
}

type ModelList = Vec<(u32, Vec3, bool)>;
type ModelHits = Vec<(Vec3, Option<u32>)>;

/// [`live_join`] with a line to the server's loop.
fn live_join_ctl<T: Send + 'static>(
    play: impl FnOnce(&mut Cod4Session, Option<String>, Instant, std::sync::mpsc::Sender<Ctl>) -> T
    + Send
    + 'static,
) -> Option<T> {
    live_join_on("mp_crash", crash_world(), &[], play)
}

/// [`live_join_ctl`] on `map` with `sets`, predicting through `world`
/// when given.
fn live_join_on<T: Send + 'static>(
    map: &str,
    world: Option<Arc<Cod4World>>,
    sets: &[(&str, &str)],
    play: impl FnOnce(&mut Cod4Session, Option<String>, Instant, std::sync::mpsc::Sender<Ctl>) -> T
    + Send
    + 'static,
) -> Option<T> {
    live_join_as(map, world, sets, "assault_mp,0", play)
}

/// [`live_join_on`] picking `class` from the class menu.
fn live_join_as<T: Send + 'static>(
    map: &str,
    world: Option<Arc<Cod4World>>,
    sets: &[(&str, &str)],
    class: &'static str,
    play: impl FnOnce(&mut Cod4Session, Option<String>, Instant, std::sync::mpsc::Sender<Ctl>) -> T
    + Send
    + 'static,
) -> Option<T> {
    live_join_with(map, world, sets, class, None, play)
}

/// [`live_join`] on mp_crash as `class`, the stats packets carrying
/// the active profile's under `players` (the directory holding
/// `players/profiles/`), whose file the server's `N`s go back to.
pub(crate) fn live_join_profile<T: Send + 'static>(
    players: std::path::PathBuf,
    class: &'static str,
    play: impl FnOnce(&mut Cod4Session, Option<String>) -> T + Send + 'static,
) -> Option<T> {
    live_join_with(
        "mp_crash",
        crash_world(),
        &[],
        class,
        Some(players),
        |s, sp, _, _| play(s, sp),
    )
}

/// [`live_join_as`] with the profile under `players`, else none.
fn live_join_with<T: Send + 'static>(
    map: &str,
    world: Option<Arc<Cod4World>>,
    sets: &[(&str, &str)],
    class: &'static str,
    players: Option<std::path::PathBuf>,
    play: impl FnOnce(&mut Cod4Session, Option<String>, Instant, std::sync::mpsc::Sender<Ctl>) -> T
    + Send
    + 'static,
) -> Option<T> {
    use vcod_server::cod4::{Cod4Config, Cod4Server};
    let install = vcod_cod4::testing::cod4_install()?;
    let _live = live_slot();
    let mut sv = Cod4Server::new(
        Cod4Config {
            map: map.into(),
            gametype: "war".into(),
            hostname: "vcod".into(),
            max_clients: 4,
        },
        7,
    );
    for (k, v) in sets {
        sv.set_cvar(k, v);
    }
    sv.load(Some(&install)).unwrap();
    let sock = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    sock.set_nonblocking(true).unwrap();
    let addr = sock.local_addr().unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    let clock = sim::Clock::new();
    let client = sim::spawn(&clock, move || {
        let join = MenuJoin {
            team: Some("allies".into()),
            class: Some(class.into()),
        };
        let mut s = match players {
            Some(root) => {
                let profile = crate::play::cod4_profile::ProfileStats::load_from(&root, "");
                Cod4Session::connect_profile(addr, "joiner", "", Cp::Bare, join, None, profile)
            }
            None => Cod4Session::connect(addr, "joiner", Cp::Bare, join),
        }
        .unwrap();
        if let Some(w) = world {
            s.set_world(w);
        }
        let t0 = clk_now();
        let mut spawned = None;
        while since(t0) < Duration::from_secs(60) && spawned.is_none() {
            s.step(since(t0).as_secs_f64() * 1000.0).unwrap();
            s.take_commands();
            spawned = s.spawn();
            clk_sleep(Duration::from_millis(16));
        }
        let out = play(&mut s, spawned, t0, tx);
        s.disconnect();
        out
    });
    let mut silent = false;
    let mut pinned: Option<(usize, [f32; 3], bool)> = None;
    // A victim's pin: mortal, and whether he has stood up yet.
    let (mut mortal, mut up) = (false, false);
    let mut mounting: Option<(usize, [f32; 3], u32)> = None;
    serve(&mut sv, &sock, &clock, &client, |sv, now, _| {
        let mut pin = None;
        match rx.try_recv() {
            Ok(Ctl::Command(text)) => sv.send_server_command(0, &text),
            Ok(Ctl::Silence) => silent = true,
            Ok(Ctl::DropFrag(at, fuse)) => {
                let l = sv.game.level.as_mut().unwrap();
                assert!(l.drop_frag_for_test(0, Vec3::from(at), fuse));
            }
            Ok(Ctl::PinBot(at)) => pin = Some((at, false)),
            Ok(Ctl::PinVictim(at)) => {
                pin = Some((at, false));
                mortal = true;
            }
            Ok(Ctl::PinShooter(at)) => pin = Some((at, true)),
            Ok(Ctl::MountBot(eye, gun)) => {
                mounting = Some((sv.add_bots(2, false)[1], eye, gun));
            }
            Ok(Ctl::Shots(tx)) => {
                let l = sv.game.level.as_ref().unwrap();
                let shots = l.combat_shots().into_iter().filter(|r| r.0 == 0);
                let _ = tx.send(shots.map(|r| (r.1, r.2, r.3, r.4)).collect());
            }
            Ok(Ctl::Probe(back)) => {
                let l = sv.game.level.as_ref().unwrap();
                if let Some(ps) = l.pm_state(0) {
                    let _ = back.send((ps.command_time, ps.view_angles));
                }
            }
            Ok(Ctl::Stats(back)) => {
                let l = sv.game.level.as_ref().unwrap();
                let _ = back.send(l.stats(0).unwrap().clone());
            }
            Ok(Ctl::AddBot(back)) => {
                let bot = sv.add_bots(1, false)[0];
                let l = sv.game.level.as_ref().unwrap();
                let _ = back.send(l.client_name(bot).to_string());
            }
            Ok(Ctl::Models(back)) => {
                let l = sv.game.level.as_mut().unwrap();
                let _ = back.send(l.shot_models_for_test());
            }
            Ok(Ctl::ModelTrace(lines, back)) => {
                let l = sv.game.level.as_mut().unwrap();
                let hits = lines
                    .into_iter()
                    .map(|(a, b)| l.shot_model_trace_for_test(a, b))
                    .collect();
                let _ = back.send(hits);
            }
            Ok(Ctl::Hurt(damage, dir)) => {
                let l = sv.game.level.as_mut().unwrap();
                l.hurt_for_test(0, damage, dir);
            }
            Ok(Ctl::Console(line)) => {
                use vcod_server::engine::GameModule;
                sv.core.console.push_back(line);
                vcod_server::cod4::Cod4Game::drain_console(sv, now);
            }
            Err(_) => {}
        }
        if let Some((at, shoot)) = pin {
            // The second bot of a team game is axis. A bot that only
            // stands there is the one axis bot, held still: a walking
            // one stepped a frame off its pin before every shot, and an
            // allied one crossed the line of fire.
            let add = |sv: &mut vcod_server::cod4::Cod4Server| match shoot {
                true => sv.add_bots(2, true)[1],
                false => sv.add_held_bot_for_test(true).unwrap(),
            };
            let bot = pinned.map_or_else(|| add(sv), |p| p.0);
            pinned = Some((bot, at, shoot));
        }
        if let Some((bot, at, shoot)) = pinned
            && let Some(l) = sv.game.level.as_mut()
            && l.playerstate(bot).is_some()
        {
            // Healed every frame past what any frame's rounds take.
            if !shoot && !mortal {
                l.heal_for_test(bot);
            }
            let Some(ps) = l.playerstate(bot) else {
                return !silent;
            };
            // A victim's body stays where he fell, and so does his
            // next life.
            let alive = ps.get("pm_type") == Some(0);
            if mortal && up && !alive {
                pinned = None;
                return !silent;
            }
            up |= alive;
            let o = [0, 1].map(|k| f32::from_bits(ps.get(&format!("origin[{k}]")).unwrap_or(0)));
            if (o[0] - at[0]).hypot(o[1] - at[1]) > 2.0 {
                l.set_view_pos(bot, [at[0], at[1], at[2] + 60.0], [0.0, 180.0, 0.0]);
            }
        }
        if let Some((bot, eye, gun)) = mounting
            && let Some(l) = sv.game.level.as_mut()
            && let Some(ps) = l.playerstate(bot)
            && ps.get("pm_type") == Some(0)
        {
            let o = [0, 1].map(|k| f32::from_bits(ps.get(&format!("origin[{k}]")).unwrap_or(0)));
            if (o[0] - eye[0]).hypot(o[1] - eye[1]) > 2.0 {
                l.set_view_pos(bot, eye, [0.0, 0.0, 0.0]);
            } else if l.use_turret_for_test(bot, gun) {
                mounting = None;
            }
        }
        !silent
    });
    Some(client.join().unwrap())
}

/// A spectator's newest playerstate as the spectate tests read it:
/// `otherFlags`, `clientNum`, `pm_type` and the origin.
fn spec_ps(s: &Cod4Session) -> Option<(u32, u32, u32, [f32; 3])> {
    let ps = &s.client().snapshots.last()?.ps;
    let u = |n: &str| ps.get(n).unwrap_or(0);
    let o = [0, 1, 2].map(|i| ps_f32(ps, &format!("origin[{i}]")));
    Some((u("otherFlags"), u("clientNum"), u("pm_type"), o))
}

/// vcod-server's `gametype` on mp_crash with `sets` and one bot, which
/// we join through the team menu's Spectator (then `assault_mp,0`,
/// should a team pick follow); `each` runs every frame with our
/// session, the bot's slot and the ms since the join until it returns
/// `Some` or 30 s pass. `None` without an install.
fn live_spectate<T: Send + 'static>(
    gametype: &str,
    sets: &[(&str, &str)],
    mut each: impl FnMut(&mut Cod4Session, usize, f64) -> Option<T> + Send + 'static,
) -> Option<Option<T>> {
    use vcod_server::cod4::{Cod4Config, Cod4Server};
    let install = vcod_cod4::testing::cod4_install()?;
    let world = crash_world();
    let _live = live_slot();
    let mut sv = Cod4Server::new(
        Cod4Config {
            map: "mp_crash".into(),
            gametype: gametype.into(),
            hostname: "vcod".into(),
            max_clients: 4,
        },
        7,
    );
    sv.set_cvar("scr_game_playerwaittime", "1");
    sv.set_cvar("scr_game_matchstarttime", "1");
    for (k, v) in sets {
        sv.set_cvar(k, v);
    }
    sv.load(Some(&install)).unwrap();
    let bot = sv.add_bots(1, false)[0];
    let sock = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    sock.set_nonblocking(true).unwrap();
    let addr = sock.local_addr().unwrap();
    let clock = sim::Clock::new();
    let client = sim::spawn(&clock, move || {
        let join = MenuJoin {
            team: Some("spectator".into()),
            class: Some("assault_mp,0".into()),
        };
        let mut s = Cod4Session::connect(addr, "watcher", Cp::Bare, join).unwrap();
        if let Some(w) = world {
            s.set_world(w);
        }
        let t0 = clk_now();
        let mut out = None;
        while since(t0) < Duration::from_secs(30) && out.is_none() {
            let now = since(t0).as_secs_f64() * 1000.0;
            s.step(now).unwrap();
            s.take_commands();
            out = each(&mut s, bot, now);
            clk_sleep(Duration::from_millis(16));
        }
        s.disconnect();
        out
    });
    serve(&mut sv, &sock, &clock, &client, |_, _, _| true);
    Some(client.join().unwrap())
}

/// What one frame of [`live_war_pair`] hands its test.
struct PairFrame<'a> {
    /// Us, on allies, the HUD's session.
    s: &'a mut Cod4Session,
    /// The enemy, on axis.
    e: &'a mut Cod4Session,
    hud: &'a mut crate::cod4_hud::Cod4Hud,
    quads: &'a [crate::hud::HudQuad],
    /// Our session's events this frame, as the HUD took them.
    fired: &'a [Fired],
    /// Our drawn playerstate, as the crosshair reads it, the eye and
    /// its angles.
    view: &'a crate::cod4_viewmodel::ViewPs,
    eye: Vec3,
    angles: Vec3,
    shell: &'a crate::console::shell::Shell,
    /// Both have spawned, and our newest snapshot's time.
    both: bool,
    level: i32,
    /// The `playLocalSound` / `stopLocalSound` aliases the HUD took
    /// this frame.
    local_sounds: &'a [(String, bool)],
    /// A script on a slot's player, run between the frame's packets
    /// and its tick.
    tx: &'a std::sync::mpsc::Sender<(usize, &'static str, &'static str, &'static str)>,
}

/// Two clients on vcod-server's war (us on allies, an enemy on axis)
/// with the prematch cut to a second, our HUD loaded and built every
/// frame, `each` run on every frame until it returns `Some` or 120 s
/// pass. `None` without an install.
fn live_war_pair<T: Send + 'static>(
    each: impl FnMut(PairFrame) -> Option<T> + Send + 'static,
) -> Option<Option<T>> {
    live_war_pair_as(["assault_mp,0", "assault_mp,0"], each)
}

/// [`live_war_pair`] with our class and the enemy's.
fn live_war_pair_as<T: Send + 'static>(
    classes: [&'static str; 2],
    each: impl FnMut(PairFrame) -> Option<T> + Send + 'static,
) -> Option<Option<T>> {
    live_war_pair_in(classes, None, each)
}

/// [`live_war_pair_as`] with the loose scripts of `gsc` read over the
/// zones' (`Cod4Server::set_script_overrides`).
fn live_war_pair_in<T: Send + 'static>(
    classes: [&'static str; 2],
    gsc: Option<&'static str>,
    each: impl FnMut(PairFrame) -> Option<T> + Send + 'static,
) -> Option<Option<T>> {
    live_war_pair_on(["allies", "axis"], classes, gsc, each)
}

/// [`live_war_pair_in`] with the two on `teams` (ours first).
fn live_war_pair_on<T: Send + 'static>(
    teams: [&'static str; 2],
    classes: [&'static str; 2],
    gsc: Option<&'static str>,
    mut each: impl FnMut(PairFrame) -> Option<T> + Send + 'static,
) -> Option<Option<T>> {
    use vcod_server::cod4::{Cod4Config, Cod4Server};
    let install = vcod_cod4::testing::cod4_install()?;
    let world = crash_world();
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
    sv.set_cvar("scr_game_playerwaittime", "1");
    sv.set_cvar("scr_game_matchstarttime", "1");
    if let Some(dir) = gsc {
        sv.set_script_overrides(dir.into());
    }
    sv.load(Some(&install)).unwrap();
    let sock = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    sock.set_nonblocking(true).unwrap();
    let addr = sock.local_addr().unwrap();
    let (tx, rx) = std::sync::mpsc::channel::<(usize, &'static str, &'static str, &'static str)>();
    let main = install.root().join("main");
    let clock = sim::Clock::new();
    let client = sim::spawn(&clock, move || {
        let join = |team: &str, class: &str| MenuJoin {
            team: Some(team.into()),
            class: Some(class.into()),
        };
        let mut s =
            Cod4Session::connect(addr, "joiner", Cp::Bare, join(teams[0], classes[0])).unwrap();
        let mut e =
            Cod4Session::connect(addr, "enemy", Cp::Bare, join(teams[1], classes[1])).unwrap();
        if let Some(w) = world {
            s.set_world(w);
        }
        let mut hud = crate::cod4_hud::Cod4Hud::default();
        hud.load(&main, "mp_crash");
        // 1.7's `default_mp_controls.cfg` binds, as a client with no
        // config of its own starts.
        let mut shell = crate::console::shell::Shell::new();
        shell.use_cod4_binds();
        let t0 = clk_now();
        let mut out = None;
        while since(t0) < Duration::from_secs(120) && out.is_none() {
            let now = since(t0).as_secs_f64() * 1000.0;
            for c in [&mut s, &mut e] {
                c.step(now).unwrap();
            }
            e.take_commands();
            e.take_fired();
            let fired = s.take_fired();
            let Some(scene) = s.scene() else {
                clk_sleep(Duration::from_millis(16));
                continue;
            };
            hud.update(&mut s, &fired, scene.server_time);
            let quads = hud.build(
                &s,
                (1280.0, 960.0),
                scene.render_time,
                Some((scene.eye, scene.angles.y)),
                &shell,
            );
            let both = s.spawn().is_some() && e.spawn().is_some();
            let level = s
                .client()
                .snapshots
                .last()
                .map_or(0, |x| x.server_time.raw());
            let local_sounds = hud.take_local_sounds();
            out = each(PairFrame {
                s: &mut s,
                e: &mut e,
                hud: &mut hud,
                quads: &quads,
                fired: &fired,
                view: &scene.view,
                eye: scene.eye,
                angles: scene.angles,
                shell: &shell,
                both,
                level,
                local_sounds: &local_sounds,
                tx: &tx,
            });
            clk_sleep(Duration::from_millis(16));
        }
        s.disconnect();
        e.disconnect();
        out
    });
    serve(&mut sv, &sock, &clock, &client, |sv, _, _| {
        // A script call between the packets' cmds and the frame,
        // where a frame's own threads run; with no path, `srand`
        // reseeds the scripts' draws with the argument.
        if let Ok((slot, path, name, arg)) = rx.try_recv() {
            let l = sv.game.level.as_mut().unwrap();
            match (path, name) {
                ("", "srand") => l.reseed_script_rand(arg.parse().unwrap()),
                _ => l.start_player_script(slot, path, name, &[arg]).unwrap(),
            }
        }
        true
    });
    Some(client.join().unwrap())
}
