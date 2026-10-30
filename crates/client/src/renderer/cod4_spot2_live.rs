//! CoD4's spot shadow history on the live path (cod4-world.md 3.5):
//! vcod-server's war on mp_carentan with us joined on allies, played
//! through `Cod4Client::pump` (a headless renderer at the client's 4
//! samples). From among the house lights near (-900 2000), lights 3 to 8,
//! the view's surfaces store several spots, and up to `sm_maxLights` of
//! them take a tile each; dropping `sm_maxLights` to 1 fades the others
//! out over `sm_spotShadowFadeTime` rather than at once, and the tiles
//! past the first change the frame. `VCOD_C4_SHOTS=<dir>` writes the
//! frames. Needs `COD4_DIR` and an adapter with BC textures.

use super::*;
use crate::play::cod4::Cod4Session;
use crate::play::cod4::tests::{clk_now, clk_sleep, run_held, since as clk_since};
use glam::Vec3;
use std::time::Duration;

const SIZE: (u32, u32) = (640, 384);
/// Among mp_carentan's house lights 3 to 8 (`--example spot_lights`).
const EYE: Vec3 = Vec3::new(-760.0, 1900.0, 230.0);

#[derive(Debug, Default)]
struct Seen {
    /// The tiles held with `sm_maxLights` 4, a frame after 1, and past
    /// the fade.
    four: Vec<(usize, f32)>,
    dropping: Vec<(usize, f32)>,
    one: Vec<(usize, f32)>,
    /// Pixels the tiles past the first moved.
    changed: usize,
}

#[test]
fn cod4_live_spot_shadows_hold_several_lights_and_fade_out() {
    use vcod_server::cod4::{Cod4Config, Cod4Server};
    let Some(install) = vcod_cod4::testing::cod4_install() else {
        return;
    };
    let root = install.root().to_path_buf();
    let _slot = crate::renderer::pipeline_check::live_session_slot();
    let mut sv = Cod4Server::new(
        Cod4Config {
            map: "mp_carentan".into(),
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
    assert!(seen.four.len() >= 2, "{seen:?}");
    // one frame on: the dropped lights still held, fading
    assert_eq!(seen.dropping.len(), seen.four.len(), "{seen:?}");
    assert!(seen.dropping[1..].iter().all(|e| e.1 < 1.0), "{seen:?}");
    assert_eq!(seen.one.len(), 1, "{seen:?}");
    assert_eq!(seen.one[0].0, seen.four[0].0, "{seen:?}");
    assert!(seen.changed > 100, "{seen:?}");
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

fn luma(p: &[u8]) -> i32 {
    i32::from(p[0]) + i32::from(p[1]) + i32::from(p[2])
}

/// The client's half; `None` without an adapter.
fn play(root: &std::path::Path, addr: std::net::SocketAddr) -> Option<Seen> {
    use crate::audio::{AudioOpts, AudioSystem, bank};
    use crate::cod2_client::Cod2Frame;
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
    let map = crate::cod4_fly::load(root, None, "mp_carentan", &s.entity_assets()).unwrap();
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
    while clk_since(start) < Duration::from_secs(30) {
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
        if !drawn || clk_since(start) < Duration::from_secs(3) {
            continue;
        }
        // along the room's most open side, 20 degrees down
        let world = c.session.world().expect("the session's world");
        let eye = EYE;
        let open = |k: i32| {
            let yaw = (k as f32 * 22.5).to_radians();
            let (fwd, _, _) = crate::camera::basis(yaw, 0.0);
            let tr = world
                .clip
                .trace(eye, eye + fwd * 1000.0, Vec3::ZERO, Vec3::ZERO, 1);
            (tr.fraction, yaw)
        };
        let yaw = (0..16)
            .map(open)
            .max_by(|a, b| a.0.total_cmp(&b.0))
            .unwrap()
            .1;
        let pitch = (-20f32).to_radians();
        let frame = |t: f32| {
            let (fwd, _, up) = crate::camera::basis(yaw, pitch);
            Frame {
                view_proj: crate::camera::view_proj_from(eye, yaw, pitch, 0.0, view.fov, aspect),
                eye,
                fwd,
                up,
                time: t,
                fov_x: view.fov,
                cull: CullMode::On,
                hud_lines: Vec::new(),
            }
        };
        let s = |n| crate::cod4_spotshadow::Settings {
            max_lights: n,
            ..Default::default()
        };
        r.set_spot_shadow(s(4));
        r.render(frame(time), None);
        r.render(frame(time + 0.5), None);
        seen.four = r.shadowed_spots();
        let many = shot(&mut r, "spots_four");
        r.set_spot_shadow(s(1));
        r.render(frame(time + 0.75), None);
        r.render(frame(time + 1.0), None);
        seen.dropping = r.shadowed_spots();
        for k in 1..=4 {
            r.render(frame(time + 1.0 + k as f32 * 0.5), None);
        }
        seen.one = r.shadowed_spots();
        let one = shot(&mut r, "spots_one");
        seen.changed = many
            .chunks(4)
            .zip(one.chunks(4))
            .filter(|(p, q)| (luma(p) - luma(q)).abs() > 8)
            .count();
        break;
    }
    c.session.disconnect();
    let errors = errors.lock().unwrap();
    assert!(errors.is_empty(), "{errors:?}");
    Some(seen)
}
