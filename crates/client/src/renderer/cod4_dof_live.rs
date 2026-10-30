//! CoD4's depth of field and effect feather on the live path:
//! vcod-server's war on mp_crash with us joined on allies, played through
//! `Cod4Client::pump` as `main` runs a frame (the session's map off
//! `cod4_fly::load`, a headless renderer at the client's 4 samples).
//! Up the sight, the pump hands the renderer a depth of field that
//! blurs the frame, and `r_dof_enable` 0 draws it sharp; a `zfeather`
//! smoke sprite standing in the ground ahead fades into it, and
//! `r_zFeather` 0 draws it hard. `VCOD_C4_SHOTS=<dir>` writes the frames.
//! Needs `COD4_DIR` and an adapter with BC textures (lavapipe will do).

use super::*;
use crate::play::cod4::Cod4Session;
use crate::play::cod4::tests::{clk_now, clk_sleep, run_held, since as clk_since};
use glam::Vec3;
use std::time::{Duration, Instant};

const SIZE: (u32, u32) = (640, 384);

#[derive(Debug, Default)]
struct Seen {
    hip: Option<crate::cod4_dof::Dof>,
    ads: Option<crate::cod4_dof::Dof>,
    /// Pixels the depth of field moved, and its frame's sum against the
    /// sharp one's.
    dof_pixels: usize,
    /// The sprite's summed difference from the frame without it, drawn
    /// feathered and hard, and the pixels it covers drawn hard.
    feathered: u64,
    hard: u64,
    sprite_pixels: usize,
    material: String,
}

#[test]
fn cod4_live_sight_blurs_and_smoke_feathers() {
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
    let hip = seen.hip.expect("a hip frame");
    assert!(!hip.used(), "{hip:?}");
    let ads = seen.ads.expect("a sighted frame");
    assert!(ads.used(), "{ads:?}");
    assert_eq!((ads.near_start, ads.near_blur), (1.0, 6.0));
    assert!(seen.dof_pixels > 1000, "{} pixels", seen.dof_pixels);
    assert!(seen.sprite_pixels > 1000, "{} pixels", seen.sprite_pixels);
    assert!(
        seen.feathered < seen.hard,
        "{} feathered, {} hard",
        seen.feathered,
        seen.hard
    );
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

fn diff(a: &[u8], b: &[u8]) -> (usize, u64) {
    a.chunks(4).zip(b.chunks(4)).fold((0, 0), |(n, s), (p, q)| {
        let d: u64 = (0..3).map(|i| u64::from(p[i].abs_diff(q[i]))).sum();
        (n + usize::from(d > 8), s + d)
    })
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
    let mut sighted_at = None;
    let mut turned_at: Option<Instant> = None;
    while clk_since(start) < Duration::from_secs(40) {
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
        let frame = |r: &Renderer, pitch: f32| {
            let (fwd, _, up) = crate::camera::basis(view.yaw, pitch);
            Frame {
                view_proj: crate::camera::view_proj_from(
                    cam.pos,
                    view.yaw,
                    pitch,
                    0.0,
                    view.fov,
                    r.aspect(),
                ),
                eye: cam.pos,
                fwd,
                up,
                time,
                fov_x: view.fov,
                cull: CullMode::On,
                hud_lines: Vec::new(),
            }
        };
        if !drawn || clk_since(start) < Duration::from_secs(2) {
            continue;
        }
        let Some(dof) = r.cod4_dof else {
            continue;
        };
        if turned_at.is_none() {
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
        }
        if turned_at.is_some_and(|t| clk_since(t) < Duration::from_secs(1)) {
            r.render(frame(&r, view.pitch), None);
            continue;
        }
        if sighted_at.is_none() {
            seen.hip = Some(dof);
            c.session.input.key(Action::Ads, true);
            sighted_at = Some(clk_now());
        } else if sighted_at.is_some_and(|t| clk_since(t) > Duration::from_secs(3)) {
            seen.ads = Some(dof);
            // the pump's depth of field, then the same frame sharp; the
            // gun stays out of both, as its sway moves it frame to frame
            r.set_cod4_options(Cod4Options::default());
            r.render(frame(&r, view.pitch), None);
            let blurred = shot(&mut r, "dof_on");
            r.set_cod4_dof(dof);
            r.set_cod4_options(Cod4Options {
                dof: false,
                ..Default::default()
            });
            r.render(frame(&r, view.pitch), None);
            let sharp = shot(&mut r, "dof_off");
            seen.dof_pixels = diff(&blurred, &sharp).0;
            feather(&mut r, &c, &fs, cam.pos, view.yaw, time, &mut seen, frame);
            break;
        }
        // the frame main would draw, for the depth of field to read
        r.render(frame(&r, view.pitch), view.vm);
    }
    c.session.disconnect();
    let errors = errors.lock().unwrap();
    assert!(errors.is_empty(), "{errors:?}");
    Some(seen)
}

/// A `zfeather` smoke sprite 80 units square, standing half in the ground
/// 120 ahead of `eye`, seen 30 degrees down: drawn feathered and hard,
/// each against the frame without it.
#[allow(clippy::too_many_arguments)]
fn feather(
    r: &mut Renderer,
    c: &crate::cod4_client::Cod4Client,
    fs: &vcod_common::pk3::Pk3Fs,
    eye: Vec3,
    yaw: f32,
    time: f32,
    seen: &mut Seen,
    frame: impl Fn(&Renderer, f32) -> Frame,
) {
    let lib = &r.shader_lib;
    let mut names: Vec<&str> = lib
        .names()
        .filter(|n| {
            lib.get(n)
                .is_some_and(|m| m.feather_view_depth && m.feather.is_some_and(|f| f > 0.0))
        })
        .collect();
    names.sort();
    let name = names
        .iter()
        .find(|n| **n == "gfx_smk_white_atlas")
        .or(names.first())
        .map(|n| n.to_string())
        .expect("a zfeather effect material in the session's library");
    seen.material.clone_from(&name);
    let (fwd, right, _) = crate::camera::basis(yaw, 0.0);
    let ahead = eye + fwd * 120.0;
    let world = c.session.world().expect("the session's world");
    let down = world
        .clip
        .trace(ahead, ahead - Vec3::Z * 500.0, Vec3::ZERO, Vec3::ZERO, 1);
    let ground = ahead - Vec3::Z * 500.0 * down.fraction;
    let corner = |dx: f32, dz: f32| (ground + right * dx + Vec3::Z * dz).to_array();
    let quad = || crate::fx::sim::FxQuad {
        verts: [
            corner(-40.0, -40.0),
            corner(40.0, -40.0),
            corner(40.0, 40.0),
            corner(-40.0, 40.0),
        ],
        uvs: [[0.0, 1.0], [1.0, 1.0], [1.0, 0.0], [0.0, 0.0]],
        rgba: [1.0; 4],
        shader: name.as_str().into(),
    };
    let pitch = (-30f32).to_radians();
    let draw = |r: &mut Renderer, zfeather: bool, quads: Vec<_>, label: &str| {
        r.set_cod4_options(Cod4Options {
            zfeather,
            dof: false,
            ..Default::default()
        });
        r.set_fx_quads(fs, quads, time);
        r.render(frame(r, pitch), None);
        shot(r, label)
    };
    let none = draw(r, true, Vec::new(), "smoke_none");
    let feathered = draw(r, true, vec![quad()], "smoke_feathered");
    let hard = draw(r, false, vec![quad()], "smoke_hard");
    let (n, hard_sum) = diff(&hard, &none);
    seen.sprite_pixels = n;
    seen.hard = hard_sum;
    seen.feathered = diff(&feathered, &none).1;
}
