//! The CoD2 1.3 soak, on the CoD4 soak's harness (`super::cod4_sweep`):
//! per stock map, vcod-server `--game cod2` loads it on a thread of the
//! test with six shooting bots and `sv_mapRotation` naming the map once
//! per gametype (dm, tdm, sd, ctf, hq; dm again last), each to its time limit and
//! `endMap`'s intermission, and one client
//! joins it through the session's own team and weapon answers and plays
//! it as `main`'s `Mode::Cod2` arm does: `Session::pump`, `scene`, the
//! events' sounds and effects, the view effects, `draw_view`, `ViewTail`
//! and the HUD quads, rendered at `VCOD_SOAK_RENDER_HZ` (10) into a 320 x
//! 240 target. The player walks the bots' nav graph, fires, throws a
//! frag, holds the scoreboard and opens the Esc menu
//! (`super::cod4_sweep::Player`). `soak.md` under `VCOD_SWEEP_OUT`
//! (default `tmp/cod2-soak`) counts per pair what the CoD4 soak counts
//! (docs/research/cod2-client-perf.md, "The soak").
//!
//! `COD2_DIR=... cargo test -p vcod --bins -- --ignored cod2_soak
//! --nocapture`, run alone (it installs the process's logger), under
//! lavapipe with `VK_DRIVER_FILES`, `WGPU_BACKEND=vulkan`,
//! `GALLIVM_PERF=nopt`. `tools/cod2_soak.sh` runs it a map a process.
//! `VCOD_SWEEP_MAPS`, `VCOD_SWEEP_GAMETYPES`, `VCOD_SOAK_MINUTES` (3),
//! `VCOD_SWEEP_LEVEL_SECS` as the CoD4 soak's.

use std::net::SocketAddr;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use anyhow::Context;
use glam::Vec3;
use vcod_common::pk3::Pk3Fs;

use super::cod2::{Cod2Session, ConnectOpts, Join};
use super::cod4_sweep::{
    BOTS, Game, Opts, Pilot, Player, Sample, Shared, SoakOpts, SoakPair, SoakServer, Tally,
};
use crate::cod2_client::{Cod2Client, Cod2Frame};
use crate::console::shell::{Effect, Shell};
use crate::frontend::Ui;
use crate::play::input::Action;

const GAMETYPES: [&str; 5] = ["dm", "tdm", "sd", "ctf", "hq"];

/// See the module doc.
#[test]
#[ignore = "15 minutes per map; COD2_DIR"]
fn cod2_soak() {
    let Some(fs) = vcod_common::testing::cod2_fs() else {
        return;
    };
    super::cod4_sweep::soak(&Cod2 { fs }, &GAMETYPES, "cod2-soak");
}

/// CoD2 1.3 under the run.
struct Cod2 {
    fs: Pk3Fs,
}

impl Game for Cod2 {
    /// Every `maps/mp/mp_*.d3dbsp` the stock iwds hold.
    fn stock_maps(&self) -> Vec<String> {
        let mut maps: Vec<String> = self
            .fs
            .find_maps()
            .into_iter()
            .filter(|m| m.starts_with("mp_"))
            .collect();
        maps.sort();
        maps
    }

    fn start(
        &self,
        map: &str,
        gametypes: &[String],
        opts: Opts,
    ) -> anyhow::Result<(Box<dyn SoakServer>, Vec<usize>)> {
        use vcod_server::cod2::{Cod2Config, Cod2Server};
        let fs = Rc::new(vcod_common::testing::cod2_fs().context("COD2_DIR")?);
        let mut sv = Cod2Server::new(
            Cod2Config {
                map: map.into(),
                gametype: gametypes[0].clone(),
                hostname: "soak".into(),
                max_clients: 12,
                gametype_script: None,
            },
            7,
            Instant::now(),
        );
        sv.exec_default_cfg(&*fs);
        let minutes = opts.soak.map_or(0.5, |s| s.minutes).to_string();
        let mut sets: Vec<(String, String)> = vec![("sv_pure".into(), "0".into())];
        for g in GAMETYPES {
            sets.push((format!("scr_{g}_timelimit"), minutes.clone()));
        }
        // One round of sd, as long as the level.
        sets.push(("scr_sd_roundlimit".into(), "1".into()));
        sets.push(("scr_sd_roundlength".into(), minutes));
        sets.push((
            "sv_mapRotation".into(),
            super::cod4_sweep::rotation(map, gametypes),
        ));
        for (k, v) in &sets {
            sv.set_cvar(k, v);
        }
        sv.load(Some(fs))?;
        let slots = sv.add_bots(BOTS, true);
        if let Some(b) = sv.bots_mut() {
            b.wait_for_graph();
        }
        Ok((Box::new(sv), slots))
    }

    fn play(&self, map: &str, addr: SocketAddr, shared: &Shared, opts: Opts) -> anyhow::Result<()> {
        let soak = opts
            .soak
            .context("CoD2 runs the soak only, not the sweep")?;
        play_soak(map, addr, shared, soak)
    }
}

impl SoakServer for vcod_server::cod2::Cod2Server {
    fn handle_packet(&mut self, from: SocketAddr, pkt: &[u8], now: Instant) {
        vcod_server::cod2::Cod2Server::handle_packet(self, from, pkt, now);
    }

    fn take_outgoing(&mut self) -> Vec<(SocketAddr, Vec<u8>)> {
        vcod_server::cod2::Cod2Server::take_outgoing(self)
    }

    fn tick(&mut self, now: Instant) {
        self.drain_console(now);
        self.tick_frames(now, 1);
    }

    fn frame_ms(&self) -> u64 {
        vcod_server::cod2::FRAME_MS as u64
    }

    fn server_id(&self) -> i32 {
        vcod_server::cod2::Cod2Server::server_id(self)
    }

    fn gametype(&self) -> String {
        self.cvar("g_gametype").to_string()
    }

    fn graph(&mut self) -> Option<Arc<vcod_server::nav::NavGraph>> {
        self.bots_mut().and_then(|b| b.graph().cloned())
    }

    fn sample(&mut self, slots: &[usize]) -> Option<Sample> {
        use vcod_cod2::pmove::pm_type;
        let holds: std::collections::HashMap<usize, bool> = self
            .bots_mut()
            .map(|b| {
                b.bots()
                    .iter()
                    .map(|(s, b)| {
                        let h = b.engaged()
                            || b.holds_cover()
                            || b.retreating() != 0
                            || b.on_objective();
                        (*s, h)
                    })
                    .collect()
            })
            .unwrap_or_default();
        let level = self.level()?;
        // `endMap` puts every player in the intermission.
        let ended = level
            .players
            .iter()
            .flatten()
            .any(|p| p.ps.pm_type == pm_type::INTERMISSION);
        let bodies = slots
            .iter()
            .filter_map(|&slot| {
                let p = level.players.get(slot)?.as_ref()?;
                let live = p.ps.pm_type == pm_type::NORMAL && !ended;
                let at = live.then_some(p.ps.origin);
                Some((slot, at, holds.get(&slot).copied().unwrap_or(false)))
            })
            .collect();
        Some(Sample {
            state: if ended { "intermission" } else { "playing" }.into(),
            ended,
            errors: level.rt.aborts(),
            bodies,
        })
    }

    fn rotate(&mut self) {
        self.push_console("map_rotate");
    }
}

/// The CoD2 client as [`Player`] drives it: its binds through
/// `Cod2Client::apply` and the Esc menu through the front end, as `main`
/// takes a key.
struct Cod2Pilot<'a> {
    c: &'a mut Cod2Client,
    ui: &'a mut Ui,
    shell: &'a Shell,
    /// The clock `main` keeps the board's 2 s repeat on.
    start: Instant,
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

/// The client and what it draws and plays through, joined and loaded.
struct Rig {
    sess: crate::session::Session,
    fs: Pk3Fs,
    r: crate::renderer::Renderer,
    errors: crate::renderer::pipeline_check::Errors,
    shell: Shell,
    localized: vcod_common::localize::Localized,
    ui: Ui,
    audio: crate::audio::AudioSystem,
    fx: crate::fx::sim::FxSystem,
    cam: crate::camera::FlyCamera,
    /// The front end's effects `main` would apply; dropped.
    pending: Vec<crate::frontend::UiEffect>,
}

/// Joins `addr` as `main`'s `--connect --game cod2 --team autoassign`
/// does and loads `map` with the session on its pump, into a headless
/// 320 x 240 renderer.
fn rig(map: &str, addr: SocketAddr) -> anyhow::Result<Rig> {
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

/// One frame of `main`'s `Mode::Cod2` arm on `rig`, the keys already
/// set: `App::cod2_level`, `App::cod2_script_menus`, then the frame
/// (`render`ed when asked). Returns the CPU side's parts in ms and the
/// render's.
fn step(
    rig: &mut Rig,
    time: f32,
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
    let mut cx = Cod2Frame {
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

impl Rig {
    /// [`Player`]'s hands on the client.
    fn pilot(&mut self, start: Instant) -> Cod2Pilot<'_> {
        Cod2Pilot {
            c: self.sess.cod2_mut().unwrap(),
            ui: &mut self.ui,
            shell: &self.shell,
            start,
        }
    }
}

/// The soak's client: [`Player`] decides the frame's input, then [`step`]
/// runs the frame, `render` at most `render_hz` times a second.
fn play_soak(map: &str, addr: SocketAddr, shared: &Shared, opts: SoakOpts) -> anyhow::Result<()> {
    let mut rig = rig(map, addr)?;
    crate::frame_prof::init();
    let seed = map.bytes().fold(0x9e37_79b9_7f4a_7c15u64, |h, b| {
        (h ^ b as u64).wrapping_mul(0x100_0000_01b3)
    });
    let mut me = Player::new(seed);
    let start = Instant::now();
    let mut last = start;
    let every = Duration::from_secs_f32(1.0 / opts.render_hz.max(0.1));
    let mut next_render = start;
    let mut fx_ms = 0.0;
    let mut audio_was = rig.audio.stats();
    while !shared.done.load(Ordering::Relaxed) {
        let now = Instant::now();
        let dt = now.duration_since(last).as_secs_f32();
        last = now;
        let time = start.elapsed().as_secs_f32();
        let graph = shared.graph.lock().unwrap().clone();
        let mut frame = SoakPair::default();
        let Rig {
            sess,
            ui,
            shell,
            cam,
            ..
        } = &mut rig;
        let mut pilot = Cod2Pilot {
            c: sess.cod2_mut().unwrap(),
            ui,
            shell,
            start,
        };
        me.act(&mut pilot, cam, time, graph.as_deref(), &mut frame);
        let warm = Tally::warm(&rig.r);
        let render = now >= next_render;
        if render {
            next_render = now + every;
            frame.renders = 1;
        }
        let (parts, render_ms) = step(&mut rig, time, dt, render, &mut fx_ms)?;
        let gametype = rig.pilot(start).gametype();
        Tally {
            parts: &parts,
            render_ms,
            time,
            level_at: me.level_at(&gametype, time),
            errors: &rig.errors,
            audio: &rig.audio,
            warm: [warm, Tally::warm(&rig.r)],
        }
        .add(&mut frame, &mut audio_was, shared, &gametype);
        let spent = now.elapsed();
        std::thread::sleep(Duration::from_millis(16).saturating_sub(spent));
    }
    crate::frame_prof::flush();
    rig.sess.shutdown();
    Ok(())
}

/// vcod-server `--game cod2` on `map` under dm, no bots, on a thread
/// until the flag is set: its address.
fn serve_bare(
    map: &str,
) -> (
    std::thread::JoinHandle<()>,
    SocketAddr,
    Arc<std::sync::atomic::AtomicBool>,
) {
    let (handle, addr, done, _) = serve_rotating(map, "");
    (handle, addr, done)
}

/// [`serve_bare`] with `sv_mapRotation` set to `rotation`; the second flag,
/// once set, runs `map_rotate` on the server's console.
fn serve_rotating(
    map: &str,
    rotation: &str,
) -> (
    std::thread::JoinHandle<()>,
    SocketAddr,
    Arc<std::sync::atomic::AtomicBool>,
    Arc<std::sync::atomic::AtomicBool>,
) {
    let done = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let rotate = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let (tx, rx) = std::sync::mpsc::channel();
    let (map, stop, rotate_flag) = (map.to_string(), done.clone(), rotate.clone());
    let rotation = rotation.to_string();
    let handle = std::thread::spawn(move || {
        use vcod_server::cod2::{Cod2Config, Cod2Server, FRAME_MS};
        let fs = Rc::new(vcod_common::testing::cod2_fs().unwrap());
        let mut sv = Cod2Server::new(
            Cod2Config {
                map,
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
        sv.set_cvar("sv_mapRotation", &rotation);
        sv.load(Some(fs)).unwrap();
        let sock = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        sock.set_nonblocking(true).unwrap();
        tx.send(sock.local_addr().unwrap()).unwrap();
        let mut buf = [0u8; 4096];
        let mut next = Instant::now();
        while !stop.load(Ordering::Relaxed) {
            if rotate_flag.swap(false, Ordering::Relaxed) {
                sv.push_console("map_rotate");
            }
            while let Ok((n, from)) = sock.recv_from(&mut buf) {
                sv.handle_packet(from, &buf[..n], Instant::now());
            }
            let now = Instant::now();
            if now >= next {
                next = now + Duration::from_millis(FRAME_MS as u64);
                SoakServer::tick(&mut sv, now);
            }
            for (to, p) in sv.take_outgoing() {
                let _ = sock.send_to(&p, to);
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    });
    let addr = rx.recv_timeout(Duration::from_secs(300)).unwrap();
    (handle, addr, done, rotate)
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
fn cod2_live_level_effects_and_models_build_nothing_in_play() {
    if vcod_common::testing::cod2_fs().is_none() {
        return;
    }
    let (server, addr, done) = serve_bare("mp_toujane");
    let mut rig = match rig("mp_toujane", addr) {
        Ok(r) => r,
        Err(e) => {
            done.store(true, Ordering::Relaxed);
            server.join().unwrap();
            // No adapter: nothing to measure.
            assert!(format!("{e:#}").contains("no adapter"), "{e:#}");
            return;
        }
    };
    let start = Instant::now();
    let mut last = start;
    let mut fx_ms = 0.0;
    // (mark materials, fx images) at the first spawn, marks seen
    let mut warm: Option<(usize, usize)> = None;
    let mut marks = 0;
    while start.elapsed() < Duration::from_secs(120) && marks == 0 {
        let now = Instant::now();
        let dt = now.duration_since(last).as_secs_f32();
        last = now;
        let time = start.elapsed().as_secs_f32();
        let spawned = rig.pilot(start).spawned();
        if spawned {
            if warm.is_none() {
                let c = rig.sess.cod2().unwrap();
                let s = c.session.as_deref().unwrap();
                let f = s.frame().unwrap();
                let named: Vec<&str> = (1..256).map_while(|i| f.configstrings.model(i)).collect();
                let missing: Vec<&&str> =
                    named.iter().filter(|n| !c.draw.model_loaded(n)).collect();
                assert!(named.len() > 10, "{} configstring models", named.len());
                assert!(missing.is_empty(), "not loaded by the spawn: {missing:?}");
            }
            warm.get_or_insert_with(|| Tally::warm(&rig.r));
            // Down at the ground ahead, in bursts.
            rig.cam.pitch = -1.2;
            let want = (time * 1000.0) as u32 % 800 < 200;
            rig.pilot(start).key(Action::Attack, want);
        }
        step(&mut rig, time, dt, false, &mut fx_ms).unwrap();
        if warm.is_some() {
            marks = rig.r.mark_count();
        }
        std::thread::sleep(Duration::from_millis(16).saturating_sub(now.elapsed()));
    }
    rig.sess.shutdown();
    done.store(true, Ordering::Relaxed);
    server.join().unwrap();
    let warm = warm.expect("the client never spawned");
    assert!(marks > 0, "no impact mark landed");
    assert_eq!(
        rig.r.mark_shaders(),
        warm.0,
        "a mark material built in play"
    );
    assert_eq!(rig.r.fx_images(), warm.1, "an fx image loaded in play");
    assert!(warm.0 > 0 && warm.1 > 0, "nothing built with the level");
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
fn cod2_live_same_map_gamestate_builds_no_model_pipeline() {
    if vcod_common::testing::cod2_fs().is_none() {
        return;
    }
    let (server, addr, done, rotate) = serve_rotating("mp_toujane", "gametype sd map mp_toujane");
    let mut rig = match rig("mp_toujane", addr) {
        Ok(r) => r,
        Err(e) => {
            done.store(true, Ordering::Relaxed);
            server.join().unwrap();
            assert!(format!("{e:#}").contains("no adapter"), "{e:#}");
            return;
        }
    };
    let start = Instant::now();
    let mut last = start;
    let mut fx_ms = 0.0;
    // pipelines at the first spawn; the sd gamestate's arrival
    let mut warm: Option<usize> = None;
    let mut sd_at: Option<Instant> = None;
    let mut worst = 0.0f32;
    while start.elapsed() < Duration::from_secs(150)
        && sd_at.is_none_or(|t| t.elapsed() < Duration::from_secs(3))
    {
        let now = Instant::now();
        let dt = now.duration_since(last).as_secs_f32();
        last = now;
        let time = start.elapsed().as_secs_f32();
        if warm.is_none() && rig.pilot(start).spawned() {
            warm = Some(rig.r.entity_pipelines());
            rotate.store(true, Ordering::Relaxed);
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
        std::thread::sleep(Duration::from_millis(16).saturating_sub(now.elapsed()));
    }
    let c = rig.sess.cod2().unwrap();
    let bomb = c.draw.model_loaded("xmodel/mp_tntbomb_obj");
    let built = rig.r.entity_pipelines();
    rig.sess.shutdown();
    done.store(true, Ordering::Relaxed);
    server.join().unwrap();
    let warm = warm.expect("the client never spawned");
    assert!(sd_at.is_some(), "no sd gamestate");
    assert!(bomb, "the sd level's mp_tntbomb_obj never loaded");
    eprintln!("sd gamestate: worst frame {worst:.1} ms");
    assert_eq!(built, warm, "a model pipeline built after the spawn");
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
fn cod2_live_view_weapons_build_nothing_in_play() {
    if vcod_common::testing::cod2_fs().is_none() {
        return;
    }
    let (server, addr, done) = serve_bare("mp_toujane");
    let mut rig = match rig("mp_toujane", addr) {
        Ok(r) => r,
        Err(e) => {
            done.store(true, Ordering::Relaxed);
            server.join().unwrap();
            assert!(format!("{e:#}").contains("no adapter"), "{e:#}");
            return;
        }
    };
    let start = Instant::now();
    let mut last = start;
    let mut fx_ms = 0.0;
    let mut spawned_at: Option<f32> = None;
    let mut drawn: Vec<String> = Vec::new();
    // the worst `view weapon` and `hud` parts past the first spawn
    let (mut worst_vw, mut worst_hud) = (0.0f32, 0.0f32);
    let mut rigs_at_spawn = 0;
    while start.elapsed() < Duration::from_secs(120)
        && spawned_at.is_none_or(|t| start.elapsed().as_secs_f32() - t < 9.0)
    {
        let now = Instant::now();
        let dt = now.duration_since(last).as_secs_f32();
        last = now;
        let time = start.elapsed().as_secs_f32();
        if spawned_at.is_none() && rig.pilot(start).spawned() {
            spawned_at = Some(time);
            rigs_at_spawn = rig.sess.cod2().unwrap().view.rigs_loaded();
        }
        if let Some(at) = spawned_at {
            // the other slot at 1 s and 6 s, a frag at 3 s
            let t = time - at;
            let mut pilot = rig.pilot(start);
            for edge in [1.0, 6.0] {
                if t >= edge && t - dt < edge {
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
        std::thread::sleep(Duration::from_millis(16).saturating_sub(now.elapsed()));
    }
    let c = rig.sess.cod2().unwrap();
    let rigs = c.view.rigs_loaded();
    let first_uses = rig.r.viewmodel_first_uses();
    rig.sess.shutdown();
    done.store(true, Ordering::Relaxed);
    server.join().unwrap();
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
}
