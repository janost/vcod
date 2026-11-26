//! The client rig the CoD4 map sweep and soak (`vcod::tools::cod4_sweep`)
//! play through, and the live gates on it: a client joined through the
//! stock menus into a headless renderer, as `main` joins.

#[cfg(test)]
use std::net::SocketAddr;
use std::path::Path;
use std::time::{Duration, Instant};

#[cfg(test)]
use glam::Vec3;

use super::cod4::Cod4Session;
use super::cod4_menus::Join as MenuJoin;
#[cfg(test)]
use crate::harness::run_held;
use crate::harness::{clk_now, clk_sleep, since as clk_since};
use crate::play::input::Action;
use vcod_cod4::net::conn::Cp;

/// The client and what it draws and plays through, joined and loaded.
pub(crate) struct Rig {
    pub(crate) c: crate::cod4_client::Cod4Client,
    pub(crate) fs: vcod_common::pk3::Pk3Fs,
    pub(crate) renderer: Option<(crate::renderer::Renderer, crate::renderer::headless::Errors)>,
    pub(crate) shell: crate::console::shell::Shell,
    pub(crate) localized: vcod_common::localize::Localized,
    pub(crate) ui: crate::frontend::Ui,
    pub(crate) audio: crate::audio::AudioSystem,
    pub(crate) fx: crate::fx::sim::FxSystem,
}

/// Joins `addr` through the stock menus and loads `map` as `main` does,
/// into a headless 320 x 240 renderer when there is an adapter. Waits on
/// the thread's `sim` clock under `run_held`, else the wall.
pub(crate) fn rig(root: &Path, map: &str, addr: std::net::SocketAddr) -> anyhow::Result<Rig> {
    use crate::audio::{AudioOpts, AudioSystem, bank};
    use crate::renderer::Renderer;
    let join = MenuJoin {
        team: Some("autoassign".into()),
        class: Some(super::cod4_menus::DEFAULT_CLASS.into()),
    };
    let mut s = Cod4Session::connect(addr, "sweeper", Cp::Bare, join)?;
    let t0 = clk_now();
    while s.gamestate().is_none() && clk_since(t0) < Duration::from_secs(60) {
        s.step(clk_since(t0).as_secs_f64() * 1000.0)?;
        clk_sleep(Duration::from_millis(16));
    }
    anyhow::ensure!(s.gamestate().is_some(), "no gamestate");
    let mut m = crate::cod4_fly::load(root, None, map, &s.entity_assets())?;
    let mut fs = crate::cod4_fly::open_fs(root, None, Some(map))?;
    for (p, w) in std::mem::take(&mut m.sound_files) {
        fs.overlay(&p, w);
    }
    let mut renderer = Renderer::headless_sized(&fs, false, 1, (320, 240));
    if let Some((r, _)) = &mut renderer {
        r.set_gamma(1.0, crate::gamma::overbright_bits(fs.game(), false, true));
        r.set_material_lib(m.materials.clone());
        r.load_world(&m.bsp, &fs)?;
    }
    let shell = crate::console::shell::Shell::new();
    let localized = vcod_common::localize::Localized::load(&fs);
    let ui = crate::frontend::Ui::without_menus(&fs);
    let mut audio = AudioSystem::with_aliases(
        &fs,
        AudioOpts {
            enabled: false,
            volume: shell.cvar_f32("mss_volume"),
            rate: bank::mixer_rate(shell.cvar_f32("mss_khz") as i32),
        },
        Default::default(),
    );
    audio.on_gamestate(map);
    // the sweep's sounds start, play out and reap as on a device
    audio.mock_mixer();
    let mut fx = crate::fx::sim::FxSystem::new();
    let mut c = crate::cod4_client::Cod4Client::new(
        s,
        std::mem::take(&mut m.models),
        std::mem::take(&mut m.parts),
        std::mem::take(&mut m.weapons),
        m.bodies.take(),
        m.av.take(),
        m.world.take(),
        std::mem::take(&mut m.look),
    );
    // The level's first-use builds behind the load, as `main`'s join runs
    // them (`App::cod4_prewarm`).
    if let Some((r, _)) = &mut renderer {
        c.prewarm(&mut crate::session::ClientFrame {
            fs: &fs,
            shell: &shell,
            localized: &localized,
            audio: &mut audio,
            fx: &mut fx,
            ui: &ui,
            r,
            console_open: false,
            time: 0.0,
            dt: 0.0,
        });
    }
    Ok(Rig {
        c,
        fs,
        renderer,
        shell,
        localized,
        ui,
        audio,
        fx,
    })
}

/// A menu left up past its auto answer (a new gametype's team menu)
/// takes auto-assign, or the first class.
pub(crate) fn answer_menu(c: &mut crate::cod4_client::Cod4Client, since: &mut Option<Instant>) {
    let now = clk_now();
    let at = *since.get_or_insert(now);
    if now.saturating_duration_since(at) > Duration::from_secs(3)
        && let Some(gs) = c.session.gamestate().cloned()
    {
        let responses = c.session.menus.open().unwrap().responses.clone();
        let pick = ["autoassign", super::cod4_menus::DEFAULT_CLASS]
            .iter()
            .find_map(|r| responses.iter().position(|x| x == r))
            .or((!responses.is_empty()).then_some(0));
        if let Some(i) = pick
            && let Some(cmd) = c.session.menus.choose(i, &gs)
        {
            c.session.command(&cmd);
        }
        *since = Some(now);
    }
}

/// What the tools' `Player` reads off and does to a game's client; the
/// gates drive a few of its methods themselves.
#[cfg_attr(test, allow(dead_code))]
pub(crate) trait Pilot {
    /// The gametype the client's gamestate names.
    fn gametype(&self) -> String;
    /// The newest snapshot's own `pm_type`.
    fn pm_type(&self) -> u32;
    /// Playing in the world (not dead, spectating or in a menu's team pick).
    fn spawned(&self) -> bool;
    fn menu_open(&self) -> bool;
    /// A menu left up: answered (or closed) once it was up a while.
    fn answer_menu(&mut self, since: &mut Option<Instant>);
    /// A `+button` bind's edge.
    fn key(&mut self, action: Action, down: bool);
    fn release_all(&mut self);
    /// The scoreboard's key.
    fn set_scores(&mut self, down: bool);
    /// Esc while playing: true when a menu came up.
    fn open_menu(&mut self) -> bool;
    /// Esc on the menu it opened.
    fn close_menu(&mut self);
}

impl Pilot for crate::cod4_client::Cod4Client {
    fn gametype(&self) -> String {
        self.session.gametype().unwrap_or_default()
    }

    fn pm_type(&self) -> u32 {
        self.session
            .client()
            .snapshots
            .last()
            .map(|f| f.ps.word(vcod_cod4::ps_word!("pm_type")))
            .unwrap_or(0)
    }

    fn spawned(&self) -> bool {
        self.session.spawn().is_some()
    }

    fn menu_open(&self) -> bool {
        self.session.menus.open().is_some()
    }

    fn answer_menu(&mut self, since: &mut Option<Instant>) {
        answer_menu(self, since);
    }

    fn key(&mut self, action: Action, down: bool) {
        self.session.input.key(action, down);
    }

    fn release_all(&mut self) {
        self.session.input.release_all();
    }

    fn set_scores(&mut self, down: bool) {
        crate::cod4_client::Cod4Client::set_scores(self, down);
    }

    fn open_menu(&mut self) -> bool {
        self.menu_key(winit::keyboard::KeyCode::Escape);
        self.session.menus.open().is_some()
    }

    fn close_menu(&mut self) {
        self.menu_key(winit::keyboard::KeyCode::Escape);
    }
}

/// The mark materials built and fx images loaded so far.
pub(crate) fn warm_counts(r: &crate::renderer::Renderer) -> (usize, usize) {
    (r.mark_shaders(), r.fx_images())
}

/// vcod-server's war on mp_crash for a live test, loaded: `hostname`
/// names it, `bots` non-shooting bots join.
#[cfg(test)]
fn crash_war(
    install: &vcod_cod4::install::Install,
    hostname: &str,
    cheats: bool,
    bots: usize,
) -> vcod_server::cod4::Cod4Server {
    use vcod_server::cod4::{Cod4Config, Cod4Server};
    let mut sv = Cod4Server::new(
        Cod4Config {
            map: "mp_crash".into(),
            gametype: "war".into(),
            hostname: hostname.into(),
            max_clients: 4,
        },
        7,
    );
    sv.set_cvar("sv_pure", "0");
    if cheats {
        sv.set_cvar("sv_cheats", "1");
    }
    sv.set_cvar("scr_game_matchstarttime", "1");
    sv.set_cvar("scr_game_playerwaittime", "1");
    sv.load(Some(install)).unwrap();
    if bots > 0 {
        sv.add_bots(bots, false);
    }
    sv
}

/// [`run_held`]'s frame hook for these tests: the console each frame, as
/// the dedicated server's loop drains it.
#[cfg(test)]
fn drain(sv: &mut vcod_server::cod4::Cod4Server, at: Instant, _: &mut Instant) -> bool {
    use vcod_server::engine::GameModule;
    <vcod_server::cod4::Cod4Game as GameModule>::drain_console(sv, at);
    true
}

/// The soak's first-use hitches: a surface's first bullet mark built its
/// material (images, stages, pipelines) in the frame it landed, and an
/// effect's first play loaded its sprite images, 15 to 250 ms each on
/// lavapipe. With the level's effects taken the client builds both up
/// front: a live client on vcod-server's mp_crash then fires into the
/// ground, its tracers fly and marks land, and no mark shader registers
/// and no fx image loads in play. Runs on [`run_held`]'s clock. Needs
/// `COD4_DIR` and an adapter.
#[test]
fn cod4_live_first_impact_marks_build_nothing_in_play() {
    let Some(install) = vcod_cod4::testing::cod4_install() else {
        return;
    };
    let root = install.root().to_path_buf();
    let _slot = crate::renderer::pipeline_check::live_session_slot();
    let mut sv = crash_war(&install, "marks", false, 0);
    let Some(seen) = run_held(&mut sv, move |addr| first_marks(&root, addr), drain) else {
        return;
    };
    let warm = seen.warm.expect("the level's effects were never taken");
    assert!(seen.marks > 0, "no impact mark landed");
    assert_eq!(seen.shaders, warm.0, "a mark shader registered in play");
    assert_eq!(seen.images, warm.1, "an fx image loaded in play");
    assert!(warm.0 > 0 && warm.1 > 0, "nothing built with the level");
}

#[cfg(test)]
struct MarksSeen {
    /// Mark shaders and fx images once the level's effects were taken.
    warm: Option<(usize, usize)>,
    marks: usize,
    /// Mark shaders and fx images at the end.
    shaders: usize,
    images: usize,
}

/// [`cod4_live_first_impact_marks_build_nothing_in_play`]'s client; `None`
/// without an adapter.
#[cfg(test)]
fn first_marks(root: &Path, addr: SocketAddr) -> Option<MarksSeen> {
    use crate::session::ClientFrame;
    let Rig {
        mut c,
        fs,
        renderer,
        shell,
        localized,
        ui,
        mut audio,
        mut fx,
    } = rig(root, "mp_crash", addr).unwrap();
    let Some((mut r, _)) = renderer else {
        c.session.disconnect();
        return None;
    };
    let mut cam = crate::camera::FlyCamera::new(Vec3::ZERO, 0.0);
    let start = clk_now();
    let mut last = start;
    let mut menu_since = None;
    let mut warm: Option<(usize, usize)> = None;
    let mut marks = 0;
    let mut held = false;
    while clk_since(start) < Duration::from_secs(120) && marks == 0 {
        let now = clk_now();
        let dt = now.duration_since(last).as_secs_f32();
        last = now;
        let time = now.duration_since(start).as_secs_f64();
        let frozen = c
            .session
            .client()
            .snapshots
            .last()
            .map(|x| x.ps.word(vcod_cod4::ps_word!("pm_flags")))
            .is_none_or(|f| f & 0x800 != 0);
        if c.session.spawn().is_some() && !frozen {
            // Down at the ground ahead, in bursts.
            cam.pitch -= 0.05;
            let want = (time * 1000.0) as u32 % 800 < 200;
            if want != held {
                c.session.input.key(Action::Attack, want);
                held = want;
            }
        } else if c.session.menus.open().is_some() {
            answer_menu(&mut c, &mut menu_since);
        }
        let mut cx = ClientFrame {
            fs: &fs,
            shell: &shell,
            localized: &localized,
            audio: &mut audio,
            fx: &mut fx,
            ui: &ui,
            r: &mut r,
            console_open: false,
            time,
            dt,
        };
        c.pump(&mut cx, &mut cam);
        let (f, rt, u) = crate::camera::basis(cam.yaw, cam.pitch);
        let marks_due = fx.take_marks(time);
        r.set_marks(&fs, &marks_due, &[], time, true);
        fx.step(dt, time, None);
        audio.set_listener(cam.pos, f, rt, u, u32::MAX);
        c.session.take_commands();
        if warm.is_none() && c.effects_taken() {
            warm = Some((r.mark_shaders(), r.fx_images()));
        }
        if warm.is_some() {
            marks = r.mark_count();
        }
        clk_sleep(Duration::from_millis(16));
    }
    c.session.disconnect();
    Some(MarksSeen {
        warm,
        marks,
        shaders: r.mark_shaders(),
        images: r.fx_images(),
    })
}

/// The level's first-use builds run with the load, not in its first frames
/// (cod4-client-perf.md 17): a live client joins vcod-server's mp_crash
/// war beside two bots, and once [`rig`]'s prewarm has run the script
/// menus are loaded and every model the play then draws (bodies, heads,
/// weapons) was uploaded already; only a model with parts hidden (a
/// shot-up car) or the stand-in box may upload in play. Runs on
/// [`run_held`]'s clock. Needs `COD4_DIR` and an adapter.
#[test]
fn cod4_live_join_builds_its_models_with_the_load() {
    let Some(install) = vcod_cod4::testing::cod4_install() else {
        return;
    };
    let root = install.root().to_path_buf();
    let _slot = crate::renderer::pipeline_check::live_session_slot();
    let mut sv = crash_war(&install, "prewarm", false, 2);
    let Some(seen) = run_held(&mut sv, move |addr| join_models(&root, addr), drain) else {
        return;
    };
    assert!(seen.menus, "the script menus waited for the first frame");
    assert!(
        seen.bodies >= 2,
        "the bots were never posed ({})",
        seen.bodies
    );
    let late: Vec<_> = seen
        .after
        .difference(&seen.warm)
        .filter(|k| !k.contains('#') && *k != BOX_KEY)
        .collect();
    assert!(late.is_empty(), "uploaded in play: {late:?}");
}

#[cfg(test)]
struct JoinSeen {
    /// The script menus were loaded with the level.
    menus: bool,
    /// The models uploaded with the load, and by the end.
    warm: std::collections::BTreeSet<String>,
    after: std::collections::BTreeSet<String>,
    /// The most bodies one frame posed.
    bodies: usize,
}

/// [`cod4_live_join_builds_its_models_with_the_load`]'s client; `None`
/// without an adapter.
#[cfg(test)]
fn join_models(root: &Path, addr: SocketAddr) -> Option<JoinSeen> {
    use crate::session::ClientFrame;
    let Rig {
        mut c,
        fs,
        renderer,
        shell,
        localized,
        ui,
        mut audio,
        mut fx,
    } = rig(root, "mp_crash", addr).unwrap();
    let Some((mut r, _)) = renderer else {
        c.session.disconnect();
        return None;
    };
    let menus = c.menus_loaded();
    let (warm, _) = c.uploads();
    let mut cam = crate::camera::FlyCamera::new(Vec3::ZERO, 0.0);
    let start = clk_now();
    let mut last = start;
    let mut menu_since = None;
    let mut bodies = 0;
    let mut since_bodies = None;
    // Three seconds of play past the first frame that posed both bots.
    while clk_since(start) < Duration::from_secs(90)
        && since_bodies.is_none_or(|t: Instant| clk_since(t) < Duration::from_secs(3))
    {
        let now = clk_now();
        let dt = now.duration_since(last).as_secs_f32();
        last = now;
        if c.session.spawn().is_none() && c.session.menus.open().is_some() {
            answer_menu(&mut c, &mut menu_since);
        }
        let mut cx = ClientFrame {
            fs: &fs,
            shell: &shell,
            localized: &localized,
            audio: &mut audio,
            fx: &mut fx,
            ui: &ui,
            r: &mut r,
            console_open: false,
            time: now.duration_since(start).as_secs_f64(),
            dt,
        };
        c.pump(&mut cx, &mut cam);
        c.session.take_commands();
        bodies = bodies.max(c.uploads().1);
        if bodies >= 2 && since_bodies.is_none() {
            since_bodies = Some(clk_now());
        }
        clk_sleep(Duration::from_millis(16));
    }
    c.session.disconnect();
    Some(JoinSeen {
        menus,
        warm,
        after: c.uploads().0,
        bodies,
    })
}

/// A shot-up car builds no GPU data in play (cod4-client-perf.md 18): a
/// live client on vcod-server's mp_crash war walks (`setviewpos`) to
/// 160 units off the nearest destructible sedan and fires bursts at its
/// doors until its `partBits` take new states, and no dynamic model
/// uploads past the prewarm's, the stand-in box aside: each hidden-part
/// state draws the prewarmed upload less its hidden surfaces. Prints the
/// client frame of each new state. Runs on [`run_held`]'s clock. Needs
/// `COD4_DIR` and an adapter.
#[test]
fn cod4_live_shot_car_uploads_nothing_in_play() {
    let Some(install) = vcod_cod4::testing::cod4_install() else {
        return;
    };
    let root = install.root().to_path_buf();
    let _slot = crate::renderer::pipeline_check::live_session_slot();
    let mut sv = crash_war(&install, "carparts", true, 0);
    let Some(seen) = run_held(&mut sv, move |addr| shoot_car(&root, addr), drain) else {
        return;
    };
    for (bits, ms) in &seen.firsts {
        println!("car state {bits:08x?}: its first frame's pump {ms:.1} ms");
    }
    assert!(seen.target, "no car spot found");
    assert!(
        seen.firsts.len() >= 2,
        "the car never broke: {:x?}",
        seen.firsts
    );
    let boxed = usize::from(!seen.warm_keys.contains(BOX_KEY) && seen.keys.contains(BOX_KEY));
    assert_eq!(
        seen.uploads,
        seen.warm + boxed,
        "a model uploaded in play: {:?}",
        seen.keys.difference(&seen.warm_keys).collect::<Vec<_>>()
    );
}

#[cfg(test)]
struct CarSeen {
    /// A car spot was found.
    target: bool,
    /// (state, the frame's pump wall ms) per new state drawn.
    firsts: Vec<([u32; 4], f64)>,
    /// Dynamic uploads and model keys after the prewarm, and at the end.
    warm: usize,
    warm_keys: std::collections::BTreeSet<String>,
    uploads: usize,
    keys: std::collections::BTreeSet<String>,
}

/// [`cod4_live_shot_car_uploads_nothing_in_play`]'s client; `None` without
/// an adapter.
#[cfg(test)]
fn shoot_car(root: &Path, addr: SocketAddr) -> Option<CarSeen> {
    use super::cod4::Visual;
    use crate::session::ClientFrame;
    let Rig {
        mut c,
        fs,
        renderer,
        shell,
        localized,
        ui,
        mut audio,
        mut fx,
    } = rig(root, "mp_crash", addr).unwrap();
    let Some((mut r, _)) = renderer else {
        c.session.disconnect();
        return None;
    };
    let warm = r.dynamic_uploads();
    let (warm_keys, _) = c.uploads();
    let mut cam = crate::camera::FlyCamera::new(Vec3::ZERO, 0.0);
    let start = clk_now();
    let mut last = start;
    let mut menu_since = None;
    let mut held = false;
    // (car number, its spot's eye, the point aimed at), once placed
    let mut target: Option<(u32, Vec3, Vec3)> = None;
    let mut placed_at = None;
    let mut states: Vec<[u32; 4]> = Vec::new();
    let mut firsts: Vec<([u32; 4], f64)> = Vec::new();
    while clk_since(start) < Duration::from_secs(120) && firsts.len() < 4 {
        if placed_at.is_some_and(|t: Instant| clk_since(t) > Duration::from_secs(20)) {
            break;
        }
        let now = clk_now();
        let dt = now.duration_since(last).as_secs_f32();
        last = now;
        let time = now.duration_since(start).as_secs_f64();
        let frozen = c
            .session
            .client()
            .snapshots
            .last()
            .map(|x| x.ps.word(vcod_cod4::ps_word!("pm_flags")))
            .is_none_or(|f| f & 0x800 != 0);
        if c.session.spawn().is_some() && !frozen {
            match target {
                None => {
                    target = car_spot(&mut c);
                    if let Some((_, eye, aim)) = target {
                        let to = aim - eye;
                        c.session.command(&format!(
                            "setviewpos {} {} {} {}",
                            eye.x,
                            eye.y,
                            eye.z,
                            to.y.atan2(to.x).to_degrees()
                        ));
                        placed_at = Some(now);
                    }
                }
                Some((_, eye, aim))
                    if placed_at
                        .is_some_and(|t: Instant| clk_since(t) > Duration::from_secs(2)) =>
                {
                    let to = aim - eye;
                    cam.yaw = to.y.atan2(to.x);
                    cam.pitch = to.z.atan2(to.truncate().length());
                    let want = (time * 1000.0) as u32 % 800 < 150;
                    if want != held {
                        c.session.input.key(Action::Attack, want);
                        held = want;
                    }
                }
                Some(_) => {}
            }
        } else if c.session.menus.open().is_some() {
            answer_menu(&mut c, &mut menu_since);
        }
        let mut cx = ClientFrame {
            fs: &fs,
            shell: &shell,
            localized: &localized,
            audio: &mut audio,
            fx: &mut fx,
            ui: &ui,
            r: &mut r,
            console_open: false,
            time,
            dt,
        };
        // Wall time, printed only: the frame's cost, not the play's clock.
        let t = Instant::now();
        c.pump(&mut cx, &mut cam);
        let ms = t.elapsed().as_secs_f64() * 1000.0;
        fx.step(dt, time, None);
        c.session.take_commands();
        if let Some((car, _, _)) = target
            && let Some(scene) = c.session.scene()
            && let Some(d) = scene.entities.iter().find(|d| {
                d.number == car
                    && matches!(&d.visual, Visual::Model(m) if m.starts_with("vehicle_80s_sedan1_"))
            })
            && !states.contains(&d.part_bits)
        {
            states.push(d.part_bits);
            firsts.push((d.part_bits, ms));
        }
        clk_sleep(Duration::from_millis(16));
    }
    c.session.disconnect();
    Some(CarSeen {
        target: target.is_some(),
        firsts,
        warm,
        warm_keys,
        uploads: r.dynamic_uploads(),
        keys: c.uploads().0,
    })
}

#[cfg(test)]
const BOX_KEY: &str = "<box>";

/// The nearest destructible sedan the client draws and an eye 160 units
/// off it, on the floor with room to stand, that sees its doors over
/// open air: (its entity number, the eye, the point aimed at).
#[cfg(test)]
fn car_spot(c: &mut crate::cod4_client::Cod4Client) -> Option<(u32, Vec3, Vec3)> {
    use super::cod4::Visual;
    let world = c.session.world()?.clone();
    let scene = c.session.scene()?;
    let mut cars: Vec<_> = scene
        .entities
        .iter()
        .filter(|d| matches!(&d.visual, Visual::Model(m) if m.starts_with("vehicle_80s_sedan1_")))
        .map(|d| (d.number, d.origin))
        .collect();
    cars.sort_by(|a, b| a.1.distance(scene.eye).total_cmp(&b.1.distance(scene.eye)));
    cars.iter().find_map(|&(n, car)| {
        (0..16).find_map(|k| {
            let y = (k as f32 * 22.5).to_radians();
            let spot = car + Vec3::new(y.cos(), y.sin(), 0.0) * 160.0;
            let (top, bottom) = (spot + Vec3::Z * 64.0, spot - Vec3::Z * 128.0);
            let down = world
                .clip
                .trace(top, bottom, Vec3::ZERO, Vec3::ZERO, 0x281_0011);
            if down.startsolid || down.fraction >= 1.0 {
                return None;
            }
            let floor = top.lerp(bottom, down.fraction) + Vec3::Z * 0.125;
            if (floor.z - car.z).abs() > 32.0 {
                return None;
            }
            let (lo, hi) = (Vec3::new(-16.0, -16.0, 0.0), Vec3::new(16.0, 16.0, 70.0));
            if world
                .clip
                .trace(floor, floor, lo, hi, 0x281_0011)
                .startsolid
            {
                return None;
            }
            let aim = car + Vec3::Z * 22.0;
            let eye = floor + Vec3::Z * 60.0;
            let t = world
                .clip
                .trace(eye, aim, Vec3::ZERO, Vec3::ZERO, 0x280_6831);
            (t.fraction == 1.0).then_some((n, eye, aim))
        })
    })
}
