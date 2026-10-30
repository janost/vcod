//! CoD4's distortion effects on the live path: vcod-server's war on
//! mp_crash with us joined on allies, played through `Cod4Client::pump`
//! and the fx step `main` runs a frame (the session's map off
//! `cod4_fly::load`, a headless renderer at the client's 4 samples). We
//! throw a frag ahead; its explode (`explosions/grenadeexp_concrete_1`)
//! carries `gfx_distortion_heat_far`, whose quads draw through the
//! distortion pipeline over the scene copy and bend the frame, and
//! `r_distortion` 0 draws the same frame without the bend.
//! `VCOD_C4_SHOTS=<dir>` writes the frames. Needs `COD4_DIR` and an
//! adapter with BC textures (lavapipe will do). cod4-effects.md 17.

use super::*;
use crate::play::cod4::Cod4Session;
use crate::play::cod4::tests::{clk_now, clk_sleep, run_held, since as clk_since};
use glam::Vec3;
use std::time::{Duration, Instant};

const SIZE: (u32, u32) = (640, 384);

#[derive(Debug, Default)]
struct Seen {
    thrown: bool,
    /// Frames drawn with the explode's distortion run in the fx pass.
    haze_frames: usize,
    /// The most pixels the bend moved in one frame, against the same
    /// frame under `r_distortion` 0.
    bent_pixels: usize,
    /// The images of the distortion runs.
    images: Vec<String>,
}

#[test]
fn cod4_live_grenade_haze_bends_the_scene() {
    use vcod_server::cod4::{Cod4Config, Cod4Server};
    let Some(install) = vcod_cod4::testing::cod4_install() else {
        return;
    };
    let root = install.root().to_path_buf();
    let _slot = crate::renderer::pipeline_check::live_session_slot();
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
    sv.load(Some(&install)).unwrap();
    let client = run_held(&mut sv, move |addr| play(&root, addr), |_, _, _| true);
    let Some(seen) = client else {
        return;
    };
    println!("{seen:#?}");
    assert!(seen.thrown, "no frag thrown");
    assert!(seen.haze_frames > 0, "the explode drew no distortion run");
    assert!(
        seen.images.iter().any(|m| m.contains("distortion_heat")),
        "{:?}",
        seen.images
    );
    assert!(seen.bent_pixels > 500, "{} pixels bent", seen.bent_pixels);
}

fn shot(r: &mut Renderer, name: &str) -> Vec<u8> {
    let (w, h, bgra) = r.read_offscreen().expect("offscreen target");
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

fn moved(a: &[u8], b: &[u8]) -> usize {
    a.chunks(4)
        .zip(b.chunks(4))
        .filter(|(p, q)| (0..3).map(|i| u32::from(p[i].abs_diff(q[i]))).sum::<u32>() > 8)
        .count()
}

/// The client's half; `None` without an adapter.
fn play(root: &std::path::Path, addr: std::net::SocketAddr) -> Option<Seen> {
    use crate::audio::{AudioOpts, AudioSystem, bank};
    use crate::cod2_client::Cod2Frame;
    use crate::play::input::Action;
    let join = crate::play::cod4_menus::Join {
        team: Some("allies".into()),
        class: Some("assault_mp,0".into()),
    };
    let mut s = Cod4Session::connect(addr, "joiner", vcod_cod4::net::conn::Cp::Bare, join).unwrap();
    let t0 = clk_now();
    while s.spawn().is_none() && clk_since(t0) < Duration::from_secs(60) {
        s.step(clk_since(t0).as_secs_f64() * 1000.0).unwrap();
        s.take_commands();
        clk_sleep(Duration::from_millis(16));
    }
    assert!(s.spawn().is_some(), "never spawned");
    let map = crate::cod4_fly::load(root, None, "mp_crash", &s.entity_assets()).unwrap();
    let fs = crate::cod4_fly::open_fs(root, None, None).unwrap();
    let Some((mut r, errors)) = Renderer::headless_sized(&fs, false, 4, SIZE) else {
        s.disconnect();
        return None;
    };
    r.set_gamma(1.0, crate::gamma::overbright_bits(fs.game(), false, true));
    r.set_material_lib(map.materials.clone());
    r.load_world(&map.bsp, &fs).unwrap();
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
    let mut seen = Seen::default();
    let start = clk_now();
    let mut last = start;
    let mut turned_at: Option<Instant> = None;
    let mut thrown_at: Option<Instant> = None;
    let mut explode_at: Option<Instant> = None;
    let mut pressed_at: Option<Instant> = None;
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
        let vm = c.take_view();
        let aspect = r.aspect();
        let view = c.draw_view(&fs, &mut r, &cam, vm, (80.0, aspect, false));
        // the fx step and draw `main`'s view tail runs
        let (fwd, right, up) = crate::camera::basis(view.yaw, view.pitch);
        fx.step(dt, time, None);
        fx.spawn_chained(&fs, time);
        let quads = fx.build_quads(cam.pos, right, up, time);
        // the explode's haze; mp_crash's fires draw `gfx_distortion_heat`
        // from the start, bending little (their red and green keys sit
        // under 15 / 255)
        let explode = quads
            .iter()
            .any(|q| &*q.shader == "gfx_distortion_heat_far");
        r.set_fx_quads(&fs, quads, time);
        let aspect = r.aspect();
        let frame = || Frame {
            view_proj: crate::camera::view_proj_from(
                cam.pos, view.yaw, view.pitch, 0.0, view.fov, aspect,
            ),
            eye: cam.pos,
            fwd,
            up,
            time,
            fov_x: view.fov,
            cull: CullMode::On,
            hud_lines: Vec::new(),
        };
        if !drawn || clk_since(start) < Duration::from_secs(2) {
            continue;
        }
        let Some(turned) = turned_at else {
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
            turned_at = Some(clk_now());
            continue;
        };
        // past the prematch freeze, a frag held 600 ms and let go, the
        // press and the release a frame apart at least
        if thrown_at.is_none() && clk_since(turned) > Duration::from_secs(3) {
            match pressed_at {
                None => {
                    c.session.input.key(Action::Frag, true);
                    pressed_at = Some(clk_now());
                }
                Some(p) if clk_since(p) > Duration::from_millis(600) => {
                    c.session.input.key(Action::Frag, false);
                    thrown_at = Some(clk_now());
                }
                Some(_) => {}
            }
        }
        if let Some(t) = thrown_at {
            seen.thrown |= c.session.client().snapshots.last().is_some_and(|f| {
                f.entities
                    .iter()
                    .any(|e| e.etype() == crate::play::cod4::ET_MISSILE)
            });
            if clk_since(t) > Duration::from_secs(9) {
                break;
            }
        }
        // The haze lives 500 ms and grows from nothing: one pair of
        // frames 200 ms into it, as a pair takes lavapipe a while.
        if explode && explode_at.is_none() {
            explode_at = Some(clk_now());
        }
        let due = explode_at.is_some_and(|t| clk_since(t) >= Duration::from_millis(200));
        if explode && due && seen.haze_frames == 0 {
            seen.haze_frames += 1;
            for run in r.fx.runs.iter().filter(|run| run.distort) {
                if !seen.images.contains(&run.image) {
                    seen.images.push(run.image.clone());
                }
            }
            // the frame as drawn, then the same one under r_distortion 0
            r.set_cod4_options(Cod4Options::default());
            r.render(frame(), None);
            let bent = shot(&mut r, "haze_on");
            r.set_cod4_options(Cod4Options {
                distortion: false,
                ..Default::default()
            });
            r.render(frame(), None);
            let flat = shot(&mut r, "haze_off");
            r.set_cod4_options(Cod4Options::default());
            seen.bent_pixels = moved(&bent, &flat);
            continue;
        }
        // only the pair is drawn, so the fx step keeps pace on a slow
        // adapter
    }
    c.session.disconnect();
    let errors = errors.lock().unwrap();
    assert!(errors.is_empty(), "{errors:?}");
    Some(seen)
}
