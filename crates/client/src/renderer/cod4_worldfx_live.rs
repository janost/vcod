//! CoD4's world fx and effect leftovers on the live path: vcod-server's
//! war with us joined on allies, played through `Cod4Client::pump` and
//! the fx step `main` runs a frame (the session's map off
//! `cod4_fly::load`, a headless renderer at the client's 4 samples), as
//! `cod4_distort_live` does. Each test draws one frame pair and holds one
//! against the other: the tracer's corkscrew beam against the frame
//! without it, the vision set's glow against `r_glow` off, the world's
//! distortion glass and a static model's feathered beam against
//! `r_distortion` and `r_zFeather` off. The world tests move the drawn
//! eye in front of their surface; the session, its map and the renderer
//! stay the live ones. `VCOD_C4_SHOTS=<dir>` writes the frames. Needs
//! `COD4_DIR` and an adapter with BC textures (lavapipe will do).

use super::*;
use crate::play::cod4::Cod4Session;
use crate::play::cod4::tests::{clk_now, clk_sleep, run_held, since as clk_since};
use crate::play::input::Action;
use glam::Vec3;
use std::time::{Duration, Instant};

const SIZE: (u32, u32) = (640, 384);

/// What a test's step sees each drawn frame.
pub(super) struct Live<'a> {
    pub r: &'a mut Renderer,
    pub fs: &'a Pk3Fs,
    pub c: &'a mut crate::cod4_client::Cod4Client,
    pub fx: &'a crate::fx::sim::FxSystem,
    pub bsp: &'a MapWorld,
    /// Seconds since the spawn's first drawn frame.
    pub t: f32,
    /// The drawn eye and angles (radians), which a step may move.
    pub eye: Vec3,
    pub yaw: f32,
    pub pitch: f32,
    pub fov: f32,
    pub time: f32,
}

impl Live<'_> {
    fn frame(&self) -> Frame {
        let (fwd, _, up) = crate::camera::basis(self.yaw, self.pitch);
        Frame {
            view_proj: crate::camera::view_proj_from(
                self.eye,
                self.yaw,
                self.pitch,
                0.0,
                self.fov,
                self.r.aspect(),
            ),
            eye: self.eye,
            fwd,
            up,
            time: self.time,
            fov_x: self.fov,
            cull: CullMode::On,
            hud_lines: Vec::new(),
        }
    }

    /// One frame under `options` with this frame's fx quads, its BGRA
    /// bytes, written as `<name>.tga` under `VCOD_C4_SHOTS`.
    fn shot(&mut self, options: Cod4Options, name: &str) -> Vec<u8> {
        let quads = self.quads();
        self.shot_with(options, quads, name)
    }

    /// [`Self::shot`] with the fx quads `quads`.
    pub(super) fn shot_with(
        &mut self,
        options: Cod4Options,
        quads: Vec<crate::fx::sim::FxQuad>,
        name: &str,
    ) -> Vec<u8> {
        // the options first: the quads' feather reads `r_zFeather`
        self.r.set_cod4_options(options);
        self.r.set_fx_quads(self.fs, quads, self.time);
        let frame = self.frame();
        self.r.render(frame, None);
        self.r.set_cod4_options(Cod4Options::default());
        let (w, h, bgra) = self.r.read_offscreen().expect("offscreen target");
        if let Some(dir) = std::env::var_os("VCOD_C4_SHOTS") {
            let rgba: Vec<u8> = bgra
                .chunks(4)
                .flat_map(|p| [p[2], p[1], p[0], 255])
                .collect();
            let path = std::path::Path::new(&dir).join(format!("{name}.tga"));
            image::save_buffer(path, &rgba, w, h, image::ExtendedColorType::Rgba8).unwrap();
        }
        bgra
    }

    /// This frame's fx quads as `main` builds them from the drawn eye.
    pub(super) fn quads(&self) -> Vec<crate::fx::sim::FxQuad> {
        let (_, right, up) = crate::camera::basis(self.yaw, self.pitch);
        self.fx.build_quads(self.eye, right, up, self.time)
    }

    /// The soups drawn with a material `pick` takes, as (centre, normal).
    fn surfaces(&self, pick: impl Fn(&Material) -> bool) -> Vec<(Vec3, Vec3)> {
        self.bsp
            .soups
            .iter()
            .filter(|s| {
                let name = &self.bsp.materials[s.material as usize].name;
                self.r.shader_lib.get(name).is_some_and(&pick)
            })
            .filter_map(|s| {
                let v = &self.bsp.verts
                    [s.first_vertex as usize..s.first_vertex as usize + s.vertex_count as usize];
                let n = v.len().max(1) as f32;
                let c = v.iter().map(|v| Vec3::from(v.pos)).sum::<Vec3>() / n;
                let nrm = v.iter().map(|v| Vec3::from(v.normal)).sum::<Vec3>();
                (nrm.length_squared() > 0.0).then(|| (c, nrm.normalize()))
            })
            .collect()
    }

    /// Puts the eye `dist` out from `at` along `normal`, looking back at it.
    fn face(&mut self, (at, normal): (Vec3, Vec3), dist: f32) {
        self.eye = at + normal * dist;
        let d = -normal;
        self.yaw = d.y.atan2(d.x);
        self.pitch = d.z.clamp(-1.0, 1.0).asin();
    }
}

impl Live<'_> {
    /// The frame at the current view drawn without and with
    /// `r_vc_depthprepass` (cod4-client-perf.md 12): the world fx read the
    /// same depth either way, so the bytes match.
    fn prepass_matches(&mut self, name: &str) -> bool {
        let off = self.shot(Cod4Options::default(), &format!("{name}_prepass0"));
        self.r.set_depth_prepass(true);
        let on = self.shot(Cod4Options::default(), &format!("{name}_prepass1"));
        self.r.set_depth_prepass(false);
        off == on
    }
}

/// The pixels whose colour moved by more than a little.
fn moved(a: &[u8], b: &[u8]) -> usize {
    a.chunks(4)
        .zip(b.chunks(4))
        .filter(|(p, q)| (0..3).map(|i| u32::from(p[i].abs_diff(q[i]))).sum::<u32>() > 8)
        .count()
}

/// vcod-server's war on `map` with the client joined; `step` runs every
/// drawn frame past the spawn, turned to its longest open view, until it returns true or 90 s pass. `None`
/// without an install or an adapter.
fn session(map: &str, step: impl FnMut(&mut Live) -> bool + Send + 'static) -> Option<()> {
    session_with(map, &[], step)
}

/// [`session`] with the server's cvars `sets`.
fn session_with(
    map: &str,
    sets: &[(&str, &str)],
    step: impl FnMut(&mut Live) -> bool + Send + 'static,
) -> Option<()> {
    session_sized(map, sets, (4, SIZE), step)
}

/// [`session_with`] on a renderer of `samples` and `size`.
pub(super) fn session_sized(
    map: &str,
    sets: &[(&str, &str)],
    (samples, size): (u32, (u32, u32)),
    step: impl FnMut(&mut Live) -> bool + Send + 'static,
) -> Option<()> {
    use vcod_server::cod4::{Cod4Config, Cod4Server};
    let install = vcod_cod4::testing::cod4_install()?;
    let root = install.root().to_path_buf();
    let _slot = crate::renderer::pipeline_check::live_session_slot();
    let mut sv = Cod4Server::new(
        Cod4Config {
            map: map.into(),
            gametype: "war".into(),
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
    let map = map.to_string();
    run_held(
        &mut sv,
        move |addr| play(&root, addr, &map, (samples, size), step),
        |_, _, _| true,
    )
}

/// The client's half.
fn play(
    root: &std::path::Path,
    addr: std::net::SocketAddr,
    map_name: &str,
    (samples, size): (u32, (u32, u32)),
    mut step: impl FnMut(&mut Live) -> bool,
) -> Option<()> {
    use crate::audio::{AudioOpts, AudioSystem, bank};
    use crate::cod2_client::Cod2Frame;
    let join = crate::play::cod4_menus::Join {
        team: Some("allies".into()),
        class: Some("assault_mp,0".into()),
    };
    let mut s = Cod4Session::connect(addr, "joiner", vcod_cod4::net::conn::Cp::Bare, join).unwrap();
    let t0 = clk_now();
    while s.spawn().is_none() && clk_since(t0) < Duration::from_secs(90) {
        s.step(clk_since(t0).as_secs_f64() * 1000.0).unwrap();
        s.take_commands();
        clk_sleep(Duration::from_millis(16));
    }
    assert!(s.spawn().is_some(), "never spawned");
    let map = crate::cod4_fly::load(root, None, map_name, &s.entity_assets()).unwrap();
    let fs = crate::cod4_fly::open_fs(root, None, None).unwrap();
    let Some((mut r, errors)) = Renderer::headless_sized(&fs, false, samples, size) else {
        s.disconnect();
        return None;
    };
    r.set_gamma(1.0, crate::gamma::overbright_bits(fs.game(), false, true));
    r.set_material_lib(map.materials.clone());
    r.load_world(&map.bsp, &fs).unwrap();
    let bsp = map.bsp;
    let shell = crate::console::shell::Shell::new();
    let localized = vcod_common::localize::Localized::load(&fs);
    let ui = crate::frontend::Ui::without_menus(&fs);
    let mut audio = AudioSystem::with_aliases(
        &fs,
        AudioOpts {
            enabled: false,
            volume: 0.0,
            rate: bank::mixer_rate(22),
        },
        Default::default(),
    );
    let mut fx = crate::fx::sim::FxSystem::new();
    let mut c = crate::cod4_client::Cod4Client::new(
        s,
        map.models,
        map.parts,
        map.weapons,
        map.bodies,
        map.av,
        map.world,
        map.look,
    );
    let mut cam = crate::camera::FlyCamera::new(Vec3::ZERO, 0.0);
    let start = clk_now();
    let mut last = start;
    let mut first: Option<Instant> = None;
    while clk_since(start) < Duration::from_secs(90) {
        // the frame first: a pass that skips on must still move the clock
        clk_sleep(Duration::from_millis(16));
        let now = clk_now();
        let dt = now.duration_since(last).as_secs_f32();
        last = now;
        let time = clk_since(start).as_secs_f32();
        let mut cx = Cod2Frame {
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
        let drawn = c.pump(&mut cx, &mut cam);
        // the effects' `spawnRange` culls against the drawn camera
        assert!(!drawn || fx.view() == Some(cam.pos), "no fx view");
        let vm = c.take_view();
        let aspect = r.aspect();
        let view = c.draw_view(&fs, &mut r, &cam, vm, (80.0, aspect, false));
        fx.step(dt, time, None);
        fx.spawn_chained(&fs, time);
        if !drawn || clk_since(start) < Duration::from_secs(2) {
            continue;
        }
        let Some(since) = first else {
            // the spawn may face a wall: turn to the longest open view
            let world = c.session.world().expect("the session's world");
            let open = |k: i32| {
                let yaw = (k as f32 * 22.5).to_radians();
                let (fwd, _, _) = crate::camera::basis(yaw, 0.0);
                let tr =
                    world
                        .clip
                        .trace(cam.pos, cam.pos + fwd * 2000.0, Vec3::ZERO, Vec3::ZERO, 1);
                (tr.fraction, yaw)
            };
            cam.yaw = (0..16)
                .map(open)
                .max_by(|a, b| a.0.total_cmp(&b.0))
                .unwrap()
                .1;
            first = Some(now);
            continue;
        };
        let mut l = Live {
            r: &mut r,
            fs: &fs,
            c: &mut c,
            fx: &fx,
            bsp: &bsp,
            t: clk_since(since).as_secs_f32(),
            eye: cam.pos,
            yaw: view.yaw,
            pitch: view.pitch,
            fov: view.fov,
            time,
        };
        if step(&mut l) {
            break;
        }
    }
    c.session.disconnect();
    let errors = errors.lock().unwrap();
    assert!(errors.is_empty(), "{errors:?}");
    Some(())
}

/// The own shots' tracers draw as 1.7's corkscrew beam
/// (`FX_Beam_GenerateVerts`, cod4-effects.md 12): a cap each end and
/// `len * 8 / 100` segments between, and the beam lights the frame.
/// Before the beam was held against the frame's other fx, one run in
/// two or three read 0 lit pixels: the shot's own flash covered it.
#[test]
fn cod4_live_tracer_draws_the_corkscrew_beam() {
    use std::sync::{Arc, Mutex};
    let seen = Arc::new(Mutex::new((0usize, 0usize, 0usize)));
    let out = seen.clone();
    let mut pressed: Option<f32> = None;
    let ran = session("mp_crash", move |l| {
        // past the prematch's freeze, bursts until a tracer flies
        if l.t > 4.0 {
            let down = pressed.is_some_and(|p| l.t - p < 0.4);
            if pressed.is_none_or(|p| l.t - p > 0.8) {
                l.c.session.input.key(Action::Attack, true);
                pressed = Some(l.t);
            } else if !down {
                l.c.session.input.key(Action::Attack, false);
            }
        }
        let quads = l.quads();
        let beam = quads.iter().filter(|q| &*q.shader == "gfx_tracer").count();
        if beam == 0 {
            return l.t > 40.0;
        }
        // The beam alone against no fx: the muzzle flash's additive white
        // round the crosshair saturates the beam's pixels when it fires
        // the same frame, and then the beam moved none of them.
        let beam_only: Vec<_> = quads
            .into_iter()
            .filter(|q| &*q.shader == "gfx_tracer")
            .collect();
        let on = l.shot_with(Cod4Options::default(), beam_only, "tracer_on");
        let off = l.shot_with(Cod4Options::default(), Vec::new(), "tracer_off");
        *out.lock().unwrap() = (1, beam, moved(&on, &off));
        true
    });
    if ran.is_none() {
        return;
    }
    let (tracers, quads, lit) = *seen.lock().unwrap();
    println!("tracers {tracers} beam quads {quads} lit pixels {lit}");
    assert_eq!(tracers, 1, "no tracer flew");
    // a cap, at least a segment, a cap
    assert!(quads >= 3, "{quads} quads");
    assert!(lit > 20, "{lit} pixels lit");
}

/// mp_bloc's vision set turns the glow on at a cutoff of 0.23
/// (cod4-world.md 11.4; mp_bog's 0.93 blooms only a fire's flicker): a
/// live frame blooms against the same frame under `r_glow` 0, set at the
/// console mid-session as a player would.
#[test]
fn cod4_live_bloc_vision_set_glows() {
    use std::sync::{Arc, Mutex};
    let mut shell = crate::console::shell::Shell::new();
    shell.register_cod4_options();
    shell.apply_systeminfo(r"\sv_cheats\1");
    let seen = Arc::new(Mutex::new(None));
    let out = seen.clone();
    let ran = session("mp_bloc", move |l| {
        // the set is the map's once the prematch ends
        let glow = l.r.level_look.as_ref().and_then(|k| k.glow);
        if glow.is_none() || l.t < 3.0 {
            return l.t > 40.0;
        }
        let on = l.shot(crate::cod4_options(&shell), "glow_on");
        shell.execute("r_glow 0");
        let off = l.shot(crate::cod4_options(&shell), "glow_off");
        *out.lock().unwrap() = Some(moved(&on, &off));
        true
    });
    if ran.is_none() {
        return;
    }
    let lit = seen
        .lock()
        .unwrap()
        .expect("mp_bloc's set never named a glow");
    println!("glow moved {lit} pixels");
    assert!(lit > 10_000, "{lit} pixels");
}

/// mp_overgrown's `wc/office_brickglass` bends the frame's copy behind it
/// (cod4-world.md 9.5): the glass against the same frame under
/// `r_distortion` 0, which draws none of it.
#[test]
fn cod4_live_overgrown_glass_bends_the_scene() {
    use std::sync::{Arc, Mutex};
    let seen = Arc::new(Mutex::new(None));
    let out = seen.clone();
    let ran = session("mp_overgrown", move |l| {
        if l.t < 1.0 {
            return false;
        }
        assert!(l.r.world_fx.is_some(), "no world fx pipelines");
        let glass = l.surfaces(|m| m.distortion.is_some());
        assert!(!glass.is_empty(), "no distortion surface");
        let mut best = 0;
        let mut view = (l.eye, l.yaw, l.pitch);
        // the panes either side, until one shows the bend
        for (i, &(at, n)) in glass.iter().enumerate().take(8) {
            for (side, nrm) in [("front", n), ("back", -n)] {
                l.face((at, nrm), 64.0);
                let on = l.shot(Cod4Options::default(), &format!("glass{i}_{side}_on"));
                let off = l.shot(
                    Cod4Options {
                        distortion: false,
                        ..Default::default()
                    },
                    &format!("glass{i}_{side}_off"),
                );
                let m = moved(&on, &off);
                println!(
                    "glass {i} {side}: eye {:.0} yaw {:.0} pitch {:.0}: {m} bent",
                    l.eye,
                    l.yaw.to_degrees(),
                    l.pitch.to_degrees()
                );
                if m > best {
                    best = m;
                    view = (l.eye, l.yaw, l.pitch);
                }
            }
            if best > 2000 {
                break;
            }
        }
        (l.eye, l.yaw, l.pitch) = view;
        let same = l.prepass_matches("glass");
        *out.lock().unwrap() = Some((glass.len(), best, same));
        true
    });
    if ran.is_none() {
        return;
    }
    let (panes, bent, same) = seen.lock().unwrap().expect("never drew");
    println!("{panes} glass surfaces, {bent} pixels bent");
    assert!(bent > 2000, "{bent} pixels bent");
    assert!(same, "the glass draws differently under the depth prepass");
}

/// cod4-effects.md 17's spot for mp_overgrown's glass bricks, reached
/// with `setviewpos` on the live session as a player would: the
/// predicted eye lands where it says and the glass fills the view. The
/// 2026-10-27 playtest's `setviewpos -406 -95 -108 163 -53` dropped the
/// player 9 units and looked steeply up at a sliver of it.
#[test]
fn cod4_live_overgrown_glass_spot_shows_the_bricks() {
    use std::sync::{Arc, Mutex};
    let seen = Arc::new(Mutex::new(None));
    let out = seen.clone();
    let mut sent = None;
    let sets = [("sv_cheats", "1")];
    let ran = session_with("mp_overgrown", &sets, move |l| {
        if l.t < 3.0 {
            return false;
        }
        let Some(at) = sent else {
            l.c.session.command("setviewpos -525 77 -116 -90 -5");
            sent = Some(l.t);
            return false;
        };
        if l.t - at < 1.5 {
            return l.t > 40.0;
        }
        let on = l.shot(Cod4Options::default(), "bricks_on");
        let off = l.shot(
            Cod4Options {
                distortion: false,
                ..Default::default()
            },
            "bricks_off",
        );
        *out.lock().unwrap() = Some((l.eye, l.yaw.to_degrees(), moved(&on, &off)));
        true
    });
    if ran.is_none() {
        return;
    }
    let (eye, yaw, bent) = seen.lock().unwrap().expect("never drew");
    println!("eye {eye:.1} yaw {yaw:.1}: {bent} pixels bent");
    assert!(
        (eye - Vec3::new(-525.0, 77.0, -116.0)).length() < 2.0,
        "{eye}"
    );
    assert!(bent > 20_000, "{bent} pixels bent");
}

/// mp_crash's floodlight beams (`mc_effect_zfeather_falloff_add_nofog_
/// eyeoffset`, static models) fade where they meet the wall and near the
/// eye (cod4-effects.md 14): the beam against `r_zFeather` 0.
#[test]
fn cod4_live_crash_floodlight_beams_feather() {
    use std::sync::{Arc, Mutex};
    let seen = Arc::new(Mutex::new(None));
    let out = seen.clone();
    let ran = session("mp_crash", move |l| {
        if l.t < 1.0 {
            return false;
        }
        let beams = l.surfaces(|m| {
            m.feather_view_depth && m.feather.is_some_and(|k| k > 0.0) && !m.stages.is_empty()
        });
        assert!(!beams.is_empty(), "no feathered world surface");
        let mut best = 0;
        let mut view = (l.eye, l.yaw, l.pitch);
        for (i, &b) in beams.iter().enumerate().take(6) {
            l.face(b, 160.0);
            let on = l.shot(Cod4Options::default(), &format!("beam{i}_on"));
            let off = l.shot(
                Cod4Options {
                    zfeather: false,
                    ..Default::default()
                },
                &format!("beam{i}_off"),
            );
            let m = moved(&on, &off);
            println!(
                "beam {i}: eye {:.0} yaw {:.0} pitch {:.0}: {m} faded",
                l.eye,
                l.yaw.to_degrees(),
                l.pitch.to_degrees()
            );
            if m > best {
                best = m;
                view = (l.eye, l.yaw, l.pitch);
            }
            if best > 500 {
                break;
            }
        }
        (l.eye, l.yaw, l.pitch) = view;
        let same = l.prepass_matches("beam");
        *out.lock().unwrap() = Some((beams.len(), best, same));
        true
    });
    if ran.is_none() {
        return;
    }
    let (n, faded, same) = seen.lock().unwrap().expect("never drew");
    println!("{n} feathered surfaces, {faded} pixels faded");
    assert!(faded > 500, "{faded} pixels faded");
    assert!(same, "the beam draws differently under the depth prepass");
}

/// The playtest's spot by mp_crash's red sedan (cod4-effects.md 18) on
/// the live session, turned to face the nearest fire (`setviewpos -279
/// 1396 233 -56 10`; at the playtest's yaw -15 it sits on the frustum's
/// edge at 16:9 and outside it at this renderer's 5:3 and 65 degrees):
/// that fire's smoke (`fire/firelp_small_pm` element 2, 1177 units off)
/// draws at 1.7's distance fade, under a fifth of its alpha, and the
/// farther two fires smoke not at all (past the element's 1250 range).
#[test]
fn cod4_live_crash_sedan_spot_fades_the_fire_smoke() {
    use std::sync::{Arc, Mutex};
    // firelp_small_pm's placements (maps/createfx/mp_crash_fx.gsc)
    const FIRES: [Vec3; 3] = [
        Vec3::new(373.468, 419.477, 161.112),
        Vec3::new(786.87, 611.93, 157.891),
        Vec3::new(766.895, 479.279, 148.814),
    ];
    let seen = Arc::new(Mutex::new(None));
    let out = seen.clone();
    let mut sent = None;
    let sets = [("sv_cheats", "1")];
    let ran = session_with("mp_crash", &sets, move |l| {
        if l.t < 3.0 {
            return false;
        }
        let Some(at) = sent else {
            l.c.session.command("setviewpos -279 1396 233 -56 10");
            sent = Some(l.t);
            return false;
        };
        if l.t - at < 6.0 {
            return l.t > 60.0;
        }
        let quads = l.quads();
        let smoke: Vec<(f32, f32)> = quads
            .iter()
            .filter(|q| q.shader.ends_with("gfx_smk_white_atlas"))
            .filter_map(|q| {
                let c = q.verts.iter().map(|v| Vec3::from(*v)).sum::<Vec3>() / 4.0;
                FIRES
                    .iter()
                    .any(|f| (c - *f).truncate().length() < 250.0)
                    .then(|| ((c - l.eye).length(), q.rgba[3]))
            })
            .collect();
        l.shot(Cod4Options::default(), "crash_sedan");
        *out.lock().unwrap() = Some((l.eye, quads.len(), smoke));
        true
    });
    if ran.is_none() {
        return;
    }
    let (eye, n, smoke) = seen.lock().unwrap().take().expect("never drew");
    println!("eye {eye:.1}: {n} quads, fire smoke {smoke:?}");
    assert!(
        (eye.truncate() - glam::Vec2::new(-279.0, 1396.0)).length() < 2.0,
        "{eye}"
    );
    assert!(!smoke.is_empty(), "no fire smoke drawn");
    for (dist, alpha) in &smoke {
        assert!(*dist < 1300.0, "a farther fire smokes: {dist}");
        assert!(*alpha < 0.2, "{dist} {alpha}");
    }
}

/// What [`floatz_against_the_copy`] measured at one view.
#[derive(Debug, Default)]
struct FloatzDiff {
    /// The frame's pixels that moved, and by up to how many levels.
    moved: usize,
    max_level: u8,
    /// `floatZ` texels nearer and farther in the prepass's than in the copy.
    nearer: usize,
    farther: usize,
    /// The prepass's draws with the copy, and writing `floatZ`.
    prepass: (usize, usize),
    /// The pixels the feather moves (`r_zFeather` 0 against 1).
    feathered: usize,
}

/// The frame at the current view, one sample, with `floatZ` from the
/// split's depth copy and from the depth prepass (cod4-client-perf.md 35):
/// the two `floatZ`s texel by texel and the frames pixel by pixel. With
/// `r_zFeather` 0 the two paths draw the same bytes.
fn floatz_against_the_copy(l: &mut Live, name: &str) -> FloatzDiff {
    l.r.set_depth_prepass(true);
    let mut d = FloatzDiff::default();
    let mut shots = Vec::new();
    for floatz in [false, true] {
        l.r.set_floatz_prepass(floatz);
        let off = l.shot(
            Cod4Options {
                zfeather: false,
                ..Default::default()
            },
            &format!("{name}_floatz{}_zfeather0", u8::from(floatz)),
        );
        let frame = l.shot(
            Cod4Options::default(),
            &format!("{name}_floatz{}", u8::from(floatz)),
        );
        let n = l.r.prepass_drawn.get();
        if floatz {
            d.prepass.1 = n;
        } else {
            d.prepass.0 = n;
        }
        shots.push((off, frame, l.r.read_floatz()));
    }
    l.r.set_floatz_prepass(true);
    l.r.set_depth_prepass(false);
    let [(off0, f0, z0), (off1, f1, z1)] = &shots[..] else {
        unreachable!()
    };
    assert!(off0 == off1, "{name}: r_zFeather 0 draws differently");
    d.feathered = moved(off1, f1);
    for (p, q) in f0.chunks(4).zip(f1.chunks(4)) {
        let m = (0..3).map(|i| p[i].abs_diff(q[i])).max().unwrap();
        if m > 0 {
            d.moved += 1;
            d.max_level = d.max_level.max(m);
        }
    }
    for (a, b) in z0.iter().zip(z1) {
        if (a - b).abs() > a.abs() * 1e-4 {
            if b < a {
                d.nearer += 1;
            } else {
                d.farther += 1;
            }
        }
    }
    // the two `floatZ`s as raw little-endian f32 rows
    if let Some(dir) = std::env::var_os("VCOD_C4_SHOTS") {
        for (z, tag) in [(z0, "copy"), (z1, "prepass")] {
            let bytes: Vec<u8> = z.iter().flat_map(|v| v.to_le_bytes()).collect();
            let path = std::path::Path::new(&dir).join(format!("{name}_floatz_{tag}.f32"));
            std::fs::write(path, bytes).unwrap();
        }
    }
    d
}

/// [`floatz_against_the_copy`] at `view` (`setviewpos`'s x y z yaw
/// pitch, the drawn eye there as a spectator's) on `map`, one sample at
/// 960 x 540, with the session's own fx, on the first frame past 6 s
/// where the feather moves 2000 pixels (or at 40 s).
fn floatz_view(map: &str, view: &'static str) -> Option<FloatzDiff> {
    use std::sync::{Arc, Mutex};
    let seen = Arc::new(Mutex::new(None));
    let out = seen.clone();
    let mut sent = None;
    let name = map.to_string();
    let sets = [("sv_cheats", "1")];
    session_sized(map, &sets, (1, (960, 540)), move |l| {
        if l.t < 3.0 {
            return false;
        }
        let Some(at) = sent else {
            l.c.session.command(&format!("setviewpos {view}"));
            sent = Some(l.t);
            return false;
        };
        if l.t - at < 6.0 {
            return l.t > 60.0;
        }
        let v: Vec<f32> = view.split(' ').map(|w| w.parse().unwrap()).collect();
        l.eye = Vec3::new(v[0], v[1], v[2]);
        l.yaw = v[3].to_radians();
        l.pitch = -v[4].to_radians();
        let off = l.shot(
            Cod4Options {
                zfeather: false,
                ..Default::default()
            },
            "probe",
        );
        let on = l.shot(Cod4Options::default(), "probe");
        if moved(&off, &on) < 2000 && l.t - at < 40.0 {
            return false;
        }
        let d = floatz_against_the_copy(l, &name);
        println!(
            "{name} at {:.0} yaw {:.0}: {d:?}",
            l.eye,
            l.yaw.to_degrees()
        );
        *out.lock().unwrap() = Some(d);
        true
    })?;
    seen.lock().unwrap().take()
}

/// [`floatz_against_the_copy`] on `map` facing, from 160 units, whichever
/// of its feathered world surfaces and feathered effect sprites the
/// feather moves most pixels at (six of each tried), one sample.
fn floatz_search(map: &str) -> Option<FloatzDiff> {
    use std::sync::{Arc, Mutex};
    let seen = Arc::new(Mutex::new(None));
    let out = seen.clone();
    let name = format!("{map}_search");
    session_sized(map, &[], (1, (960, 540)), move |l| {
        if l.t < 4.0 {
            return false;
        }
        let feathered = |m: &Material| m.feather.is_some_and(|k| k > 0.0) && !m.stages.is_empty();
        let mut spots = l.surfaces(|m| m.feather_view_depth && feathered(m));
        spots.truncate(6);
        let sprites: Vec<(Vec3, Vec3)> = l
            .quads()
            .iter()
            .filter(|q| l.r.shader_lib.get(&q.shader).is_some_and(feathered))
            .map(|q| {
                let c = q.verts.iter().map(|v| Vec3::from(*v)).sum::<Vec3>() / 4.0;
                let to_eye = (l.eye - c).with_z(0.0).normalize_or(Vec3::X);
                (c, (to_eye + Vec3::Z * 0.4).normalize())
            })
            .step_by(7)
            .take(6)
            .collect();
        spots.extend(sprites);
        let mut best = (0, (l.eye, l.yaw, l.pitch));
        for &spot in &spots {
            l.face(spot, 160.0);
            let on = l.shot(Cod4Options::default(), "probe");
            let off = l.shot(
                Cod4Options {
                    zfeather: false,
                    ..Default::default()
                },
                "probe",
            );
            let m = moved(&on, &off);
            if m > best.0 {
                best = (m, (l.eye, l.yaw, l.pitch));
            }
        }
        (l.eye, l.yaw, l.pitch) = best.1;
        let d = floatz_against_the_copy(l, &name);
        println!(
            "{name} at {:.0} yaw {:.0}: {d:?}",
            l.eye,
            l.yaw.to_degrees()
        );
        *out.lock().unwrap() = Some(d);
        true
    })?;
    seen.lock().unwrap().take()
}

/// The sedan spot of cod4-client-perf.md 32 and mp_crossfire's held view,
/// the depth prepass's `floatZ` against the split's copy, and two views
/// facing a feathered surface or sprite: `r_zFeather` 0 draws the same
/// bytes either way, and the search's feather moves pixels.
#[test]
fn cod4_live_floatz_prepass_against_the_copy() {
    for (map, view) in [
        ("mp_crash", "-279 1396 233 -15 28"),
        ("mp_crossfire", "5232 -888 180 225 0"),
    ] {
        let Some(d) = floatz_view(map, view) else {
            return;
        };
        assert!(d.prepass.1 > 0, "{map}: {d:?}");
    }
    for map in ["mp_crash", "mp_crossfire"] {
        let Some(d) = floatz_search(map) else {
            return;
        };
        assert!(d.feathered > 500, "{map}: {d:?}");
    }
}

/// The main pass's fragment counts by kind ([`crate::frame_prof::FRAG_NAMES`])
/// at the current view, `r_zFeather` as `zfeather`, after a few frames
/// (the counts read back late); `None` without `VCOD_FRAG_STATS`.
fn frag_counts(l: &mut Live, zfeather: bool) -> Option<[u64; crate::frame_prof::FRAG_NAMES.len()]> {
    l.r.frag_stats.as_ref()?;
    l.r.set_cod4_options(Cod4Options {
        zfeather,
        ..Default::default()
    });
    let quads = l.quads();
    l.r.set_fx_quads(l.fs, quads, l.time);
    for _ in 0..8 {
        let frame = l.frame();
        l.r.render(frame, None);
        let _ = l.r.read_offscreen();
    }
    l.r.set_cod4_options(Cod4Options::default());
    l.r.frag_stats.as_ref()?.last.map(|(_, c)| c)
}

/// A real-card spot by mp_crash's sedan at eye height
/// (cod4-client-perf.md 37): every opaque surface of the sedan draws
/// under EQUAL behind the `floatZ` prepass (its materials' stub sets
/// resolved), and with `VCOD_FRAG_STATS=1` the entities' fragments with
/// the feather on and off, then per drawn instance and surface.
#[test]
fn cod4_live_sedan_entity_fill() {
    use std::sync::{Arc, Mutex};
    let seen = Arc::new(Mutex::new(None));
    let out = seen.clone();
    let mut sent = None;
    let sets = [("sv_cheats", "1")];
    let view = "-279 1396 293 -15 28";
    if session_sized("mp_crash", &sets, (1, (1920, 1080)), move |l| {
        if l.t < 3.0 {
            return false;
        }
        let Some(at) = sent else {
            l.c.session.command(&format!("setviewpos {view}"));
            sent = Some(l.t);
            return false;
        };
        if l.t - at < 6.0 {
            return l.t > 60.0;
        }
        let v: Vec<f32> = view.split(' ').map(|w| w.parse().unwrap()).collect();
        l.eye = Vec3::new(v[0], v[1], v[2]);
        l.yaw = v[3].to_radians();
        l.pitch = -v[4].to_radians();
        l.r.set_depth_prepass(true);
        l.shot(Cod4Options::default(), "sedan_fill");
        let names = l.c.drawn_names.borrow().clone();
        let census = l.r.ent_census.borrow().clone();
        let sedan = names
            .iter()
            .find(|x| x.1.starts_with("vehicle_80s_sedan1") && (x.3 - l.eye).length() < 200.0)
            .map(|x| x.0);
        let opaque = |inst| census.iter().filter(move |c| c.0 == inst && c.5);
        *out.lock().unwrap() = sedan.map(|i| {
            (
                opaque(i).count(),
                opaque(i).filter(|c| c.4).count(),
                l.r.floatz_drawn.get(),
            )
        });
        let (Some(on), Some(off)) = (frag_counts(l, true), frag_counts(l, false)) else {
            println!("no fragment counts (VCOD_FRAG_STATS)");
            return true;
        };
        println!("feather on  {on:?}\nfeather off {off:?}");
        let n = l.r.dynamic.instances.len() as u32;
        for i in 0..n {
            let draws: Vec<_> = census.iter().filter(|c| c.0 == i).collect();
            if draws.is_empty() {
                continue;
            }
            l.r.ent_only.set(Some((i, None)));
            let f = frag_counts(l, true).unwrap_or_default();
            let name = names
                .iter()
                .find(|x| x.0 == i)
                .map_or("(body)".to_string(), |x| {
                    format!("{} ent {} at {:.0}", x.1, x.2, x.3)
                });
            let tris: u32 = draws.iter().map(|d| d.2).sum();
            let stages: usize = draws.iter().map(|d| d.3).sum();
            let equal = draws.iter().filter(|d| d.4).count();
            println!(
                "inst {i} {name}: {} surfaces ({equal} under EQUAL), {stages} stages, {tris} tris, ents frags {}",
                draws.len(),
                f[1]
            );
            for d in &draws {
                l.r.ent_only.set(Some((i, Some(d.1))));
                let f = frag_counts(l, true).unwrap_or_default();
                println!(
                    "  surface {} {} tris, {} stages, EQUAL {}: {} frags",
                    d.1, d.2, d.3, d.4, f[1]
                );
            }
        }
        l.r.ent_only.set(None);
        l.r.set_depth_prepass(false);
        true
    })
    .is_none()
    {
        return;
    }
    let (opaque, equal, floatz) = seen.lock().unwrap().expect("no sedan by the eye");
    println!("sedan: {equal} of {opaque} opaque surfaces under EQUAL, floatZ prepass {floatz}");
    assert!(floatz, "the feather drew no floatZ prepass");
    assert!(opaque > 0 && equal == opaque);
}
