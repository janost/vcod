//! The CoD4 live tests' rigs: a client joined to vcod-server's mp_crash
//! in process ([`live_join`], [`live_spectate`]) or a pair at war
//! ([`live_war_pair`]), all on [`crate::harness`]'s clock.

use super::*;
use std::sync::Arc;

use vcod_cod4::net::conn::Cp;

use crate::harness::{clk_now, clk_sleep, live_slot, serve, sim, since};
use crate::play::cod4_menus::Join as MenuJoin;

/// mp_crash's prediction world, loaded once.
pub(crate) fn crash_world() -> Option<Arc<Cod4World>> {
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
pub(crate) struct FrameClock(Instant);

impl FrameClock {
    pub(crate) fn new() -> FrameClock {
        FrameClock(clk_now())
    }

    pub(crate) fn wait(&mut self) {
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
pub(crate) fn live_join<T: Send + 'static>(
    play: impl FnOnce(&mut Cod4Session, Option<String>, Instant) -> T + Send + 'static,
) -> Option<T> {
    live_join_ctl(move |s, spawned, t0, _| play(s, spawned, t0))
}

/// What a [`live_join_ctl`] client asks of the server's loop.
pub(crate) enum Ctl {
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

pub(crate) type ModelList = Vec<(u32, Vec3, bool)>;
pub(crate) type ModelHits = Vec<(Vec3, Option<u32>)>;

/// [`live_join`] with a line to the server's loop.
pub(crate) fn live_join_ctl<T: Send + 'static>(
    play: impl FnOnce(&mut Cod4Session, Option<String>, Instant, std::sync::mpsc::Sender<Ctl>) -> T
    + Send
    + 'static,
) -> Option<T> {
    live_join_on("mp_crash", crash_world(), &[], play)
}

/// [`live_join_ctl`] on `map` with `sets`, predicting through `world`
/// when given.
pub(crate) fn live_join_on<T: Send + 'static>(
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
pub(crate) fn live_join_as<T: Send + 'static>(
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
pub(crate) fn live_join_with<T: Send + 'static>(
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
pub(crate) fn spec_ps(s: &Cod4Session) -> Option<(u32, u32, u32, [f32; 3])> {
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
pub(crate) fn live_spectate<T: Send + 'static>(
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
pub(crate) struct PairFrame<'a> {
    /// Us, on allies, the HUD's session.
    pub(crate) s: &'a mut Cod4Session,
    /// The enemy, on axis.
    pub(crate) e: &'a mut Cod4Session,
    pub(crate) hud: &'a mut crate::cod4_hud::Cod4Hud,
    pub(crate) quads: &'a [crate::hud::HudQuad],
    /// Our session's events this frame, as the HUD took them.
    pub(crate) fired: &'a [Fired],
    /// Our drawn playerstate, as the crosshair reads it, the eye and
    /// its angles.
    pub(crate) view: &'a crate::cod4_viewmodel::ViewPs,
    pub(crate) eye: Vec3,
    pub(crate) angles: Vec3,
    pub(crate) shell: &'a crate::console::shell::Shell,
    /// Both have spawned, and our newest snapshot's time.
    pub(crate) both: bool,
    pub(crate) level: i32,
    /// The `playLocalSound` / `stopLocalSound` aliases the HUD took
    /// this frame.
    pub(crate) local_sounds: &'a [(String, bool)],
    /// A script on a slot's player, run between the frame's packets
    /// and its tick.
    pub(crate) tx: &'a std::sync::mpsc::Sender<(usize, &'static str, &'static str, &'static str)>,
}

/// Two clients on vcod-server's war (us on allies, an enemy on axis)
/// with the prematch cut to a second, our HUD loaded and built every
/// frame, `each` run on every frame until it returns `Some` or 120 s
/// pass. `None` without an install.
pub(crate) fn live_war_pair<T: Send + 'static>(
    each: impl FnMut(PairFrame) -> Option<T> + Send + 'static,
) -> Option<Option<T>> {
    live_war_pair_as(["assault_mp,0", "assault_mp,0"], each)
}

/// [`live_war_pair`] with our class and the enemy's.
pub(crate) fn live_war_pair_as<T: Send + 'static>(
    classes: [&'static str; 2],
    each: impl FnMut(PairFrame) -> Option<T> + Send + 'static,
) -> Option<Option<T>> {
    live_war_pair_in(classes, None, each)
}

/// [`live_war_pair_as`] with the loose scripts of `gsc` read over the
/// zones' (`Cod4Server::set_script_overrides`).
pub(crate) fn live_war_pair_in<T: Send + 'static>(
    classes: [&'static str; 2],
    gsc: Option<&'static str>,
    each: impl FnMut(PairFrame) -> Option<T> + Send + 'static,
) -> Option<Option<T>> {
    live_war_pair_on(["allies", "axis"], classes, gsc, each)
}

/// [`live_war_pair_in`] with the two on `teams` (ours first).
pub(crate) fn live_war_pair_on<T: Send + 'static>(
    teams: [&'static str; 2],
    classes: [&'static str; 2],
    gsc: Option<&'static str>,
    each: impl FnMut(PairFrame) -> Option<T> + Send + 'static,
) -> Option<Option<T>> {
    live_war_pair_with(teams, classes, gsc, |_| {}, each)
}

/// [`live_war_pair_on`] with `setup` run on the loaded server.
pub(crate) fn live_war_pair_with<T: Send + 'static>(
    teams: [&'static str; 2],
    classes: [&'static str; 2],
    gsc: Option<&'static str>,
    setup: impl FnOnce(&mut vcod_server::cod4::Cod4Server),
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
    setup(&mut sv);
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
