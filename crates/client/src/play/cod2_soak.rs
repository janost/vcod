//! The client rig the CoD2 soak (`vcod::tools::cod2_soak`) plays
//! through, and the live gates on it: a client joined as `main` joins
//! into a headless renderer, and one frame of `main`'s `Mode::Cod2` arm.

use std::net::SocketAddr;
#[cfg(test)]
use std::rc::Rc;
use std::time::{Duration, Instant};

use anyhow::Context;
use glam::Vec3;
use vcod_common::pk3::Pk3Fs;

use super::cod2::{Cod2Session, ConnectOpts, Join};
use super::cod4_sweep::Pilot;
#[cfg(test)]
use super::cod4_sweep::warm_counts;
use crate::cod2_client::Cod2Client;
use crate::console::shell::{Effect, Shell};
use crate::frontend::Ui;
#[cfg(test)]
use crate::harness::{clk_now, clk_sleep, since};
use crate::play::input::Action;
use crate::session::ClientFrame;

/// The client and what it draws and plays through, joined and loaded.
pub(crate) struct Rig {
    pub(crate) sess: crate::session::Session,
    pub(crate) fs: Pk3Fs,
    pub(crate) r: crate::renderer::Renderer,
    // The soak reads it; the test build leaves the tools out.
    #[cfg_attr(test, allow(dead_code))]
    pub(crate) errors: crate::renderer::headless::Errors,
    pub(crate) shell: Shell,
    pub(crate) localized: vcod_common::localize::Localized,
    pub(crate) ui: Ui,
    pub(crate) audio: crate::audio::AudioSystem,
    pub(crate) fx: crate::fx::sim::FxSystem,
    pub(crate) cam: crate::camera::FlyCamera,
    /// The front end's effects `main` would apply; dropped.
    pub(crate) pending: Vec<crate::frontend::UiEffect>,
}

/// Joins `addr` as `main`'s `--connect --game cod2 --team autoassign`
/// does and loads `map` with the session on its pump, into a headless
/// 320 x 240 renderer.
pub(crate) fn rig(map: &str, addr: SocketAddr) -> anyhow::Result<Rig> {
    use crate::audio::{AudioOpts, AudioSystem, bank};
    use crate::renderer::Renderer;
    let fs = vcod_common::testing::cod2_fs().context("COD2_DIR")?;
    let mut shell = Shell::new();
    shell.use_cod2_cvars();
    shell.use_cod2_binds();
    shell.apply_latched();
    shell.execute("set name soaker");
    let opts = ConnectOpts {
        home: None,
        ..crate::cod2_front::connect_opts(&shell, &fs)
    };
    let mut s = Cod2Session::connect(&addr.to_string(), &opts, Duration::from_secs(60))?;
    s.join = Some(Join {
        team: "autoassign".into(),
        weapon: None,
    });
    let named = s.map_name().unwrap_or_default().to_string();
    anyhow::ensure!(named == map, "the gamestate is for {named:?}");
    let pump = Box::new(s).pump();
    let data = fs
        .read(&format!("maps/mp/{map}.d3dbsp"))
        .context("no .d3dbsp")?;
    let bsp = vcod_cod2::world::from_bytes(&data)?;
    let (mut r, errors) = Renderer::headless_sized(&fs, false, 1, (320, 240))
        .context("the soak draws: no adapter (set VK_DRIVER_FILES for lavapipe)")?;
    r.load_world(&bsp, &fs)?;
    let localized = vcod_common::localize::Localized::load(&fs);
    let ui = Ui::new(&fs, shell.cvar("cl_language").unwrap_or(""));
    let mut audio = AudioSystem::new(
        &fs,
        AudioOpts {
            enabled: false,
            volume: shell.cvar_f32("mss_volume"),
            rate: bank::mixer_rate(shell.cvar_f32("mss_khz") as i32),
        },
    );
    audio.on_gamestate(map);
    // the soak's sounds start, play out and reap as on a device
    audio.mock_mixer();
    let (origin, yaw) = vcod_common::bsp::find_spawn(&bsp.entities).unwrap_or(([0.0; 3], 0.0));
    let cam = crate::camera::FlyCamera::new(Vec3::from(origin) + Vec3::Z * 60.0, yaw);
    Ok(Rig {
        sess: crate::session::Session::Cod2(Box::new(Cod2Client::new(Some(pump)))),
        fs,
        r,
        errors,
        shell,
        localized,
        ui,
        audio,
        fx: crate::fx::sim::FxSystem::new(),
        cam,
        pending: Vec::new(),
    })
}

/// The CoD2 client as [`Player`] drives it: its binds through
/// `Cod2Client::apply` and the Esc menu through the front end, as `main`
/// takes a key.
pub(crate) struct Cod2Pilot<'a> {
    pub(crate) c: &'a mut Cod2Client,
    #[cfg_attr(test, allow(dead_code))]
    pub(crate) ui: &'a mut Ui,
    pub(crate) shell: &'a Shell,
    /// The clock `main` keeps the board's 2 s repeat on.
    pub(crate) start: Instant,
}

impl Cod2Pilot<'_> {
    fn ps(&self) -> Option<(i32, bool)> {
        let s = self.c.session.as_deref()?;
        let f = s.frame()?;
        Some((f.snap.ps.pm_type, f.snap.ps.followed().is_some()))
    }
}

impl Pilot for Cod2Pilot<'_> {
    fn gametype(&self) -> String {
        self.c
            .session
            .as_deref()
            .and_then(|s| {
                let info = s.configstrings().first()?;
                vcod_cod2::net::oob::info_value_for_key(info, "g_gametype").map(str::to_string)
            })
            .unwrap_or_default()
    }

    fn pm_type(&self) -> u32 {
        self.ps().map_or(0, |(t, _)| t as u32)
    }

    fn spawned(&self) -> bool {
        self.ps().is_some_and(|(t, followed)| t == 0 && !followed)
    }

    fn menu_open(&self) -> bool {
        self.ui.active()
    }

    /// The session answers the team and weapon menus itself (`Join`); a
    /// menu still up after 3 s is closed.
    fn answer_menu(&mut self, since: &mut Option<Instant>) {
        let at = *since.get_or_insert_with(Instant::now);
        if at.elapsed() > Duration::from_secs(3) {
            self.ui.close_all();
            *since = None;
        }
    }

    fn key(&mut self, action: Action, down: bool) {
        self.c
            .apply(&Effect::Button(action, down), self.shell, self.start);
    }

    fn release_all(&mut self) {
        self.c.release_keys();
    }

    fn set_scores(&mut self, down: bool) {
        self.c.apply(&Effect::Scores(down), self.shell, self.start);
    }

    fn open_menu(&mut self) -> bool {
        let mut out = Vec::new();
        self.c.escape(self.ui, self.shell, &mut out) && self.ui.active()
    }

    fn close_menu(&mut self) {
        self.ui.close_all();
    }
}

impl Rig {
    /// [`Player`]'s hands on the client.
    pub(crate) fn pilot(&mut self, start: Instant) -> Cod2Pilot<'_> {
        Cod2Pilot {
            c: self.sess.cod2_mut().unwrap(),
            ui: &mut self.ui,
            shell: &self.shell,
            start,
        }
    }
}

/// One frame of `main`'s `Mode::Cod2` arm on `rig`, the keys already
/// set: `App::cod2_level`, `App::cod2_script_menus`, then the frame
/// (`render`ed when asked). Returns the CPU side's parts in ms and the
/// render's.
pub(crate) fn step(
    rig: &mut Rig,
    time: f64,
    dt: f32,
    render: bool,
    fx_ms: &mut f32,
) -> anyhow::Result<([(&'static str, f32); 6], f32)> {
    let Rig {
        sess,
        fs,
        r,
        shell,
        localized,
        ui,
        audio,
        fx,
        cam,
        pending,
        ..
    } = rig;
    let t0 = Instant::now();
    sess.finish_pump();
    // A drop ends the game, another map's gamestate loads it.
    if let Some(new) = sess.take_level_change()? {
        log::warn!("sweep: the client changed map to {new}");
    }
    if let Some(c) = sess.cod2_mut() {
        c.script_menus(ui, fs, shell, pending);
    }
    pending.clear();
    let mut hud_quads = Vec::new();
    let mut vm = None;
    let mut view_fx = None;
    let mut cx = ClientFrame {
        fs,
        shell,
        localized,
        audio,
        fx,
        ui,
        r,
        console_open: false,
        time,
        dt,
    };
    let pumped = sess.pump(&mut cx, cam);
    let t1 = Instant::now();
    if pumped {
        (vm, hud_quads) = sess.scene(&mut cx, cam);
    }
    let t2 = Instant::now();
    if pumped {
        let events = sess.events(&mut cx);
        cx.audio.pump_mixer(f64::from(dt));
        sess.audio(&mut cx, &events, cam.pos);
        sess.effects(&mut cx, &events, cam.pos);
        view_fx = sess.view(&mut cx, &events, cam);
    } else {
        sess.idle();
    }
    let t2e = Instant::now();
    let aspect = r.aspect();
    let widescreen = shell.cvar_f32("vc_widescreen") != 0.0;
    let fly_fov = crate::camera::view_fov_x(crate::camera::DEFAULT_FOV_DEG, aspect, widescreen);
    let view = sess.draw_view(
        fs,
        r,
        cam,
        view_fx.as_ref(),
        vm,
        (fly_fov, aspect, widescreen),
    );
    let t2a = Instant::now();
    let (drawn, vm) = crate::ViewTail {
        fs,
        shell,
        audio,
        fx,
        session: sess,
        r,
        time,
        dt,
        aspect,
        cull: crate::renderer::CullMode::On,
    }
    .run(cam, view, fly_fov, &mut hud_quads, fx_ms);
    let t2b = Instant::now();
    r.set_hud_quads(fs, hud_quads);
    let t3 = Instant::now();
    let mut render_ms = 0.0;
    if render {
        r.render(drawn, vm);
        render_ms = t3.elapsed().as_secs_f32() * 1000.0;
    }
    crate::frame_prof::end_frame();
    let ms = |a: Instant, b: Instant| b.duration_since(a).as_secs_f32() * 1000.0;
    Ok((
        [
            ("pump", ms(t0, t1)),
            ("hud", ms(t1, t2)),
            ("events", ms(t2, t2e)),
            ("view weapon", ms(t2e, t2a)),
            ("fx and sound", ms(t2a, t2b)),
            ("hud upload", ms(t2b, t3)),
        ],
        render_ms,
    ))
}

/// vcod-server `--game cod2` on `map` under dm, no bots, with
/// `sv_mapRotation` set to `rotation`, under `client` on one test clock
/// (`crate::harness::run_held`): `client` gets its address and a
/// channel whose lines run on the server's console (`map_rotate`,
/// `addbot`).
#[cfg(test)]
fn held<T: Send + 'static>(
    map: &str,
    rotation: &str,
    client: impl FnOnce(SocketAddr, std::sync::mpsc::Sender<String>) -> T + Send + 'static,
) -> T {
    use vcod_server::cod2::{Cod2Config, Cod2Server};
    let fs = Rc::new(vcod_common::testing::cod2_fs().unwrap());
    let mut sv = Cod2Server::new(
        Cod2Config {
            map: map.into(),
            gametype: "dm".into(),
            hostname: "marks".into(),
            max_clients: 4,
            gametype_script: None,
        },
        7,
        Instant::now(),
    );
    sv.exec_default_cfg(&*fs);
    sv.set_cvar("sv_pure", "0");
    sv.set_cvar("sv_mapRotation", rotation);
    sv.load(Some(fs)).unwrap();
    let (console, lines) = std::sync::mpsc::channel::<String>();
    crate::harness::run_held(
        &mut sv,
        move |addr| client(addr, console),
        |sv, _, _| {
            while let Ok(line) = lines.try_recv() {
                sv.push_console(&line);
            }
            true
        },
    )
}

/// The soak's first-use builds: a surface's first bullet mark built its
/// material in the frame it landed, an effect's first play read its file
/// and loaded its sprite images, and an entity's first showing loaded its
/// model (the sd bomb, 115 ms). With the level's effects and models
/// listed at the gamestate the client builds them up front: by the spawn
/// every configstring model is loaded, and a live client on vcod-server's
/// mp_toujane then fires into the ground until marks land with no mark
/// material registered and no fx image loaded past the spawn. Needs
/// `COD2_DIR` and an adapter (lavapipe will do).
#[test]
#[ignore = "a headless renderer each; COD2_DIR and a Vulkan adapter"]
fn cod2_live_level_effects_and_models_build_nothing_in_play() {
    if vcod_common::testing::cod2_fs().is_none() {
        return;
    }
    held("mp_toujane", "", |addr, _| {
        let mut rig = match rig("mp_toujane", addr) {
            Ok(r) => r,
            Err(e) => {
                // No adapter: nothing to measure.
                assert!(format!("{e:#}").contains("no adapter"), "{e:#}");
                return;
            }
        };
        let start = clk_now();
        let mut last = start;
        let mut fx_ms = 0.0;
        // (mark materials, fx images) at the first spawn, marks seen
        let mut warm: Option<(usize, usize)> = None;
        let mut marks = 0;
        while since(start) < Duration::from_secs(120) && marks == 0 {
            let now = clk_now();
            let dt = now.duration_since(last).as_secs_f32();
            last = now;
            let time = since(start).as_secs_f64();
            let spawned = rig.pilot(start).spawned();
            if spawned {
                if warm.is_none() {
                    let c = rig.sess.cod2().unwrap();
                    let s = c.session.as_deref().unwrap();
                    let f = s.frame().unwrap();
                    let named: Vec<&str> =
                        (1..256).map_while(|i| f.configstrings.model(i)).collect();
                    let missing: Vec<&&str> =
                        named.iter().filter(|n| !c.draw.model_loaded(n)).collect();
                    assert!(named.len() > 10, "{} configstring models", named.len());
                    assert!(missing.is_empty(), "not loaded by the spawn: {missing:?}");
                }
                warm.get_or_insert_with(|| warm_counts(&rig.r));
                // Down at the ground ahead, in bursts.
                rig.cam.pitch = -1.2;
                let want = (time * 1000.0) as u32 % 800 < 200;
                rig.pilot(start).key(Action::Attack, want);
            }
            step(&mut rig, time, dt, false, &mut fx_ms).unwrap();
            if warm.is_some() {
                marks = rig.r.mark_count();
            }
            clk_sleep(Duration::from_millis(16));
        }
        rig.sess.shutdown();
        let warm = warm.expect("the client never spawned");
        assert!(marks > 0, "no impact mark landed");
        assert_eq!(
            rig.r.mark_shaders(),
            warm.0,
            "a mark material built in play"
        );
        assert_eq!(rig.r.fx_images(), warm.1, "an fx image loaded in play");
        assert!(warm.0 > 0 && warm.1 > 0, "nothing built with the level");
    });
}

/// sd's first frame: a gamestate for the map already loaded (the rotation
/// from dm to sd on mp_toujane) brings `mp_tntbomb_obj`, the first model
/// with a `SrcAlpha One` stage, whose pipeline took 110 to 125 ms on
/// lavapipe in the frame that brought the gamestate. With every stock
/// model pipeline built at the level's first models
/// (`entity_material::COD2_MODEL_PIPELINES`), a live client on
/// vcod-server builds none after its first spawn, through the sd level's
/// gamestate and its models (cod2-client-perf.md, "The sd level's first
/// frame"). Needs `COD2_DIR` and an adapter (lavapipe will do).
#[test]
#[ignore = "a headless renderer each; COD2_DIR and a Vulkan adapter"]
fn cod2_live_same_map_gamestate_builds_no_model_pipeline() {
    if vcod_common::testing::cod2_fs().is_none() {
        return;
    }
    held(
        "mp_toujane",
        "gametype sd map mp_toujane",
        |addr, console| {
            let mut rig = match rig("mp_toujane", addr) {
                Ok(r) => r,
                Err(e) => {
                    assert!(format!("{e:#}").contains("no adapter"), "{e:#}");
                    return;
                }
            };
            let start = clk_now();
            let mut last = start;
            let mut fx_ms = 0.0;
            // pipelines at the first spawn; the sd gamestate's arrival
            let mut warm: Option<usize> = None;
            let mut sd_at: Option<Instant> = None;
            let mut worst = 0.0f32;
            while since(start) < Duration::from_secs(150)
                && sd_at.is_none_or(|t| since(t) < Duration::from_secs(3))
            {
                let now = clk_now();
                let dt = now.duration_since(last).as_secs_f32();
                last = now;
                let time = since(start).as_secs_f64();
                if warm.is_none() && rig.pilot(start).spawned() {
                    warm = Some(rig.r.entity_pipelines());
                    console.send("map_rotate".into()).unwrap();
                }
                let gamestates = rig
                    .sess
                    .cod2()
                    .and_then(|c| c.session.as_deref())
                    .map_or(0, |s| s.gamestates());
                let (parts, _) = step(&mut rig, time, dt, false, &mut fx_ms).unwrap();
                if gamestates >= 2 {
                    sd_at.get_or_insert(now);
                    worst = worst.max(parts.iter().map(|p| p.1).sum());
                }
                clk_sleep(Duration::from_millis(16));
            }
            let c = rig.sess.cod2().unwrap();
            let bomb = c.draw.model_loaded("xmodel/mp_tntbomb_obj");
            let built = rig.r.entity_pipelines();
            rig.sess.shutdown();
            let warm = warm.expect("the client never spawned");
            assert!(sd_at.is_some(), "no sd gamestate");
            assert!(bomb, "the sd level's mp_tntbomb_obj never loaded");
            eprintln!("sd gamestate: worst frame {worst:.1} ms");
            assert_eq!(built, warm, "a model pipeline built after the spawn");
        },
    );
}

/// The view weapon's first draw: a weapon's first showing (the spawn's
/// rifle, a switch to the pistol, the frag's throw) uploaded its view
/// models in `Renderer::set_viewmodel`, 22 to 53 ms on lavapipe in that
/// frame. With every configstring 7 weapon's view models and the level's
/// hands uploaded at the gamestate (`cod2_viewmodel::level_view_models`),
/// a live client on vcod-server's mp_toujane spawns, switches weapons and
/// throws a frag with nothing uploaded in the draw (cod2-client-perf.md,
/// "The view weapon's first draw"). Needs `COD2_DIR` and an adapter
/// (lavapipe will do).
#[test]
#[ignore = "a headless renderer each; COD2_DIR and a Vulkan adapter"]
fn cod2_live_view_weapons_build_nothing_in_play() {
    if vcod_common::testing::cod2_fs().is_none() {
        return;
    }
    held("mp_toujane", "", |addr, _| {
        let mut rig = match rig("mp_toujane", addr) {
            Ok(r) => r,
            Err(e) => {
                assert!(format!("{e:#}").contains("no adapter"), "{e:#}");
                return;
            }
        };
        let start = clk_now();
        let mut last = start;
        let mut fx_ms = 0.0;
        let mut spawned_at: Option<f64> = None;
        let mut drawn: Vec<String> = Vec::new();
        // the worst `view weapon` and `hud` parts past the first spawn
        let (mut worst_vw, mut worst_hud) = (0.0f32, 0.0f32);
        let mut rigs_at_spawn = 0;
        while since(start) < Duration::from_secs(120)
            && spawned_at.is_none_or(|t| since(start).as_secs_f64() - t < 9.0)
        {
            let now = clk_now();
            let dt = now.duration_since(last).as_secs_f32();
            last = now;
            let time = since(start).as_secs_f64();
            if spawned_at.is_none() && rig.pilot(start).spawned() {
                spawned_at = Some(time);
                rigs_at_spawn = rig.sess.cod2().unwrap().view.rigs_loaded();
            }
            if let Some(at) = spawned_at {
                // the other slot at 1 s and 6 s, a frag at 3 s
                let t = time - at;
                let mut pilot = rig.pilot(start);
                for edge in [1.0, 6.0] {
                    if t >= edge && t - f64::from(dt) < edge {
                        pilot
                            .c
                            .apply(&Effect::Impulse(Action::NextWeapon), pilot.shell, start);
                    }
                }
                pilot.key(Action::Frag, (3.0..3.3).contains(&t));
            }
            let (parts, _) = step(&mut rig, time, dt, false, &mut fx_ms).unwrap();
            if let Some(w) = rig.sess.cod2().unwrap().view.drawn()
                && drawn.last().is_none_or(|l| l != w)
            {
                drawn.push(w.to_string());
            }
            if spawned_at.is_some() {
                worst_vw = worst_vw.max(parts[3].1);
                worst_hud = worst_hud.max(parts[1].1);
            }
            clk_sleep(Duration::from_millis(16));
        }
        let c = rig.sess.cod2().unwrap();
        let rigs = c.view.rigs_loaded();
        let first_uses = rig.r.viewmodel_first_uses();
        rig.sess.shutdown();
        assert!(spawned_at.is_some(), "the client never spawned");
        eprintln!(
            "view weapons drawn {drawn:?}: worst view weapon {worst_vw:.1} ms, hud {worst_hud:.1} ms; \
         rigs {rigs_at_spawn} at the spawn, {rigs} at the end; {first_uses} first uses in the draw"
        );
        let mut kinds = drawn.clone();
        kinds.sort();
        kinds.dedup();
        assert!(kinds.len() >= 3, "three view weapons not drawn: {drawn:?}");
        assert_eq!(first_uses, 0, "a view model uploaded in the draw");
    });
}

/// Muzzle flashes and brass on the guns, live: a client on vcod-server's
/// mp_toujane fires in bursts, three bots joining once it spawns. Its own shots
/// play the view weapon's flash on the drawn gun's `tag_flash`, ahead of
/// the eye, and its brass on `tag_brass` beside the eye; the bots' shots
/// play their world flash on their bodies' `tag_flash`, at a gun's height
/// over their origins. Then `cg_drawGun 0` hides the gun and its flash
/// while its brass still flies (cod2-efx.md 6.8). Needs `COD2_DIR` and an
/// adapter (lavapipe will do).
#[test]
#[ignore = "a headless renderer each; COD2_DIR and a Vulkan adapter"]
fn cod2_live_shots_flash_and_eject_on_their_guns() {
    use crate::play::cod2_fx::ViewTag;
    if vcod_common::testing::cod2_fs().is_none() {
        return;
    }
    held("mp_toujane", "", |addr, console| {
        let mut rig = match rig("mp_toujane", addr) {
            Ok(r) => r,
            Err(e) => {
                assert!(format!("{e:#}").contains("no adapter"), "{e:#}");
                return;
            }
        };
        let start = clk_now();
        let mut last = start;
        let mut fx_ms = 0.0;
        let (mut own_flash, mut own_brass, mut bot_flash) = (0, 0, 0);
        // under `cg_drawGun 0`: the view flashes, the brass, frames the gun drew
        let mut hidden: Option<(u32, u32, u32)> = None;
        let mut bots = false;
        while since(start) < Duration::from_secs(180)
            && hidden.is_none_or(|(_, brass, _)| brass < 3)
        {
            if hidden.is_none() && own_flash > 0 && own_brass > 0 && bot_flash > 0 {
                rig.shell.force("cg_drawGun", "0");
                hidden = Some((0, 0, 0));
            }
            let now = clk_now();
            let dt = now.duration_since(last).as_secs_f32();
            last = now;
            let time = since(start).as_secs_f64();
            if rig.pilot(start).spawned() {
                if !bots {
                    console.send("addbot 3".into()).unwrap();
                    bots = true;
                }
                let want = (time * 1000.0) as u32 % 1200 < 300;
                rig.pilot(start).key(Action::Attack, want);
            }
            step(&mut rig, time, dt, false, &mut fx_ms).unwrap();
            let eye = rig.cam.pos;
            let forward = crate::camera::basis(rig.cam.yaw, rig.cam.pitch).0;
            let c = rig.sess.cod2_mut().unwrap();
            if let Some((flash, brass, drawn)) = hidden.as_mut() {
                *drawn += u32::from(c.gun_drawn(None));
                for (_, tag, _) in c.view_spawned.drain(..) {
                    match tag {
                        ViewTag::Flash => *flash += 1,
                        ViewTag::Brass => *brass += 1,
                    }
                }
            }
            for (path, tag, pose) in c.view_spawned.drain(..) {
                let d = pose.origin - eye;
                match tag {
                    ViewTag::Flash => {
                        assert!(
                            d.length() < 60.0 && d.dot(forward) > 5.0,
                            "{path}: the view flash {d} off the eye"
                        );
                        own_flash += 1;
                    }
                    ViewTag::Brass => {
                        assert!(d.length() < 40.0, "{path}: the view brass {d} off the eye");
                        own_brass += 1;
                    }
                }
            }
            let f = c.session.as_deref().and_then(|s| s.frame());
            for (path, bolt, pose) in c.tag_spawned.drain(..) {
                if bolt.slot != ViewTag::Flash.slot() {
                    continue;
                }
                let Some(e) = f
                    .as_ref()
                    .and_then(|f| f.snap.entities.iter().find(|e| e.number == bolt.entity))
                else {
                    continue;
                };
                let off = pose.origin - f.as_ref().unwrap().entity_origin(e);
                assert!(
                    off.truncate().length() < 80.0 && (5.0..75.0).contains(&off.z),
                    "{path}: entity {}'s flash {off} off his origin",
                    bolt.entity
                );
                bot_flash += 1;
            }
            clk_sleep(Duration::from_millis(16));
        }
        rig.sess.shutdown();
        eprintln!(
            "own flashes {own_flash}, own brass {own_brass}, bot flashes {bot_flash}; \
         under cg_drawGun 0 (flashes, brass, frames drawn) {hidden:?}"
        );
        assert!(own_flash > 0, "no view flash");
        assert!(own_brass > 0, "no view brass");
        assert!(bot_flash > 0, "no bot's world flash");
        assert_eq!(hidden.map(|(f, b, d)| (f, b >= 3, d)), Some((0, true, 0)));
    });
}
