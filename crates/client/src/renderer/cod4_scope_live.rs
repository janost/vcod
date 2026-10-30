//! CoD4's sniper scope on the live path: vcod-server's war on mp_crash
//! with us joined on allies as the stock Sniper class (the M40A3),
//! played through `Cod4Client::pump` as `main` runs a frame (the
//! session's map off `cod4_fly::load`, a headless renderer at the
//! client's 4 samples, the HUD's quads set as `main` sets them). Up the
//! sight, the frame `main` draws (the pump's view weapon, depth of field
//! and HUD) shows the world through the overlay's clear centre: its
//! centre matches the same view drawn bare, with no gun, no HUD and no
//! depth of field. `VCOD_C4_SHOTS=<dir>` writes the frames. Needs
//! `COD4_DIR` and an adapter with BC textures (lavapipe will do).
//! cod4-client-view.md 4.6.

use super::*;
use crate::play::cod4::Cod4Session;
use crate::play::cod4::tests::{clk_now, clk_sleep, run_held, since as clk_since};
use glam::Vec3;
use std::time::{Duration, Instant};

const SIZE: (u32, u32) = (640, 384);

#[derive(Debug, Default)]
struct Seen {
    /// The hip frame drew the gun; the scoped one did not.
    hip_gun: bool,
    scoped_gun: bool,
    /// The sight's lerp and fov at the scoped frame.
    pos_frac: f32,
    fov: f32,
    /// The scoped frame's HUD quads (the overlay among them).
    hud_quads: usize,
    /// Of the centre box's pixels, those the live scoped frame draws as
    /// the bare view does, and the box's size.
    centre_same: usize,
    centre: usize,
    /// The centre box's mean brightness, live and bare.
    centre_luma: [f32; 2],
    /// Pixels the overlay darkens out at the frame's corners.
    corner_dark: usize,
}

#[test]
fn cod4_live_sniper_scope_shows_the_world_through_its_centre() {
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
    assert!(seen.hip_gun, "the hip frame draws the gun");
    assert_eq!(seen.pos_frac, 1.0, "fully sighted");
    assert!(!seen.scoped_gun, "the gun hides under the scope's overlay");
    assert!(seen.hud_quads > 0, "the overlay draws");
    assert!(
        seen.centre_same * 10 > seen.centre * 8,
        "{} of {} centre pixels show the world",
        seen.centre_same,
        seen.centre
    );
    assert!(
        seen.corner_dark > 1000,
        "{} dark corner pixels",
        seen.corner_dark
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

fn luma(p: &[u8]) -> f32 {
    (u32::from(p[0]) + u32::from(p[1]) + u32::from(p[2])) as f32 / 3.0
}

/// The centre box (a sixth of the height square) of `live` against
/// `bare`: pixels within 24 summed levels, the box's size, and each
/// box's mean brightness.
fn centre(live: &[u8], bare: &[u8]) -> (usize, usize, [f32; 2]) {
    let (w, h) = (SIZE.0 as usize, SIZE.1 as usize);
    let half = h / 12;
    let (mut same, mut n, mut sum) = (0, 0, [0.0; 2]);
    for y in h / 2 - half..h / 2 + half {
        for x in w / 2 - half..w / 2 + half {
            let i = (y * w + x) * 4;
            let (p, q) = (&live[i..i + 4], &bare[i..i + 4]);
            let d: u32 = (0..3).map(|k| u32::from(p[k].abs_diff(q[k]))).sum();
            same += usize::from(d <= 24);
            n += 1;
            sum[0] += luma(p);
            sum[1] += luma(q);
        }
    }
    (same, n, sum.map(|s| s / n as f32))
}

/// Pixels in the frame's four corner boxes the live frame draws near
/// black where the bare one does not.
fn corner_dark(live: &[u8], bare: &[u8]) -> usize {
    let (w, h) = (SIZE.0 as usize, SIZE.1 as usize);
    let s = h / 6;
    let mut n = 0;
    for (x0, y0) in [(0, 0), (w - s, 0), (0, h - s), (w - s, h - s)] {
        for y in y0..y0 + s {
            for x in x0..x0 + s {
                let i = (y * w + x) * 4;
                n += usize::from(luma(&live[i..i + 4]) < 8.0 && luma(&bare[i..i + 4]) >= 8.0);
            }
        }
    }
    n
}

/// The client's half; `None` without an adapter.
fn play(root: &std::path::Path, addr: std::net::SocketAddr) -> Option<Seen> {
    use crate::audio::{AudioOpts, AudioSystem, bank};
    use crate::cod2_client::Cod2Frame;
    use crate::play::input::Action;
    let join = crate::play::cod4_menus::Join {
        team: Some("allies".into()),
        class: Some("sniper_mp,0".into()),
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
    let mut sighted_at: Option<Instant> = None;
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
        let frame = |r: &Renderer| {
            let (fwd, _, up) = crate::camera::basis(view.yaw, view.pitch);
            Frame {
                view_proj: crate::camera::view_proj_from(
                    cam.pos,
                    view.yaw,
                    view.pitch,
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
        // the frame main draws: the pump's gun, depth of field and HUD
        let hud = c.hud_quads(r.screen_size(), &shell);
        let quads = hud.len();
        r.set_hud_quads(&fs, hud);
        let gun = view.vm.is_some();
        r.render(frame(&r), view.vm);
        if turned_at.is_some_and(|t| clk_since(t) < Duration::from_secs(1)) {
            continue;
        }
        let pos_frac = c.view_ps().map_or(0.0, |p| p.pos_frac);
        match sighted_at {
            None => {
                seen.hip_gun = gun;
                shot(&mut r, "scope_hip");
                c.session.input.key(Action::Ads, true);
                sighted_at = Some(clk_now());
            }
            Some(t) if clk_since(t) < Duration::from_secs(3) => {}
            Some(_) => {
                seen.scoped_gun = gun;
                seen.pos_frac = pos_frac;
                seen.fov = view.fov;
                seen.hud_quads = quads;
                let live = shot(&mut r, "scope_live");
                // the same view bare: no gun, no HUD, no depth of field
                r.set_hud_quads(&fs, Vec::new());
                r.set_cod4_options(Cod4Options {
                    dof: false,
                    ..Default::default()
                });
                r.render(frame(&r), None);
                let bare = shot(&mut r, "scope_bare");
                (seen.centre_same, seen.centre, seen.centre_luma) = centre(&live, &bare);
                seen.corner_dark = corner_dark(&live, &bare);
                break;
            }
        }
    }
    c.session.disconnect();
    let errors = errors.lock().unwrap();
    assert!(errors.is_empty(), "{errors:?}");
    Some(seen)
}
