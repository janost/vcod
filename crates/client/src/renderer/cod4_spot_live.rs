//! CoD4's spot light shadow map on the live path (cod4-world.md 3.5):
//! vcod-server's war on mp_crash with us joined on allies, played through
//! `Cod4Client::pump` as `main` runs a frame (the session's map off
//! `cod4_fly::load`, a headless renderer at the client's 4 samples). A
//! view from just under mp_crash's spot light 3 (60 -684 255, its beam
//! straight down, radius 350, over a tiled room) picks that light to
//! shadow, and a bot held on the tiles there casts a shadow onto them
//! that `r_spotLightEntityShadows` 0 does not draw (`r_spotLightShadows`
//! 0 draws the baked visibility in the map's place). `VCOD_C4_SHOTS=<dir>` writes the frames. Needs
//! `COD4_DIR` and an adapter with BC textures (lavapipe will do).

use super::*;
use crate::play::cod4::Cod4Session;
use crate::play::cod4::tests::{clk_now, clk_sleep, run_held, since as clk_since};
use glam::Vec3;
use std::time::Duration;

const SIZE: (u32, u32) = (640, 384);
/// mp_crash's spot light 3 (`--example spot_lights`).
const LIGHT: usize = 3;
const LIGHT_AT: Vec3 = Vec3::new(60.0, -684.0, 255.0);
/// The tiles 70 units off the light toward the room's open side (yaw
/// 67.5), where the view looks.
const BODY_AT: Vec3 = Vec3::new(86.8, -619.3, 185.125);
/// Behind the light, on the room's other side.
const OUR_AT: Vec3 = Vec3::new(37.0, -739.4, 185.125);

#[derive(Debug, Default)]
struct Seen {
    chosen: Option<usize>,
    /// Pixels the body's shadow darkened against
    /// `r_spotLightEntityShadows` 0, and brightened (none expected).
    darker: usize,
    brighter: usize,
    /// Pixels the map moved against `r_spotLightShadows` 0: the body's
    /// shadow and the baked visibility the map stands in for.
    changed: usize,
}

#[test]
fn cod4_live_spot_light_shadows_its_ground() {
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
    // a body standing on the tiles in the light, its shadow cast away from
    // the light into the view
    let bot = sv.add_bots(1, false)[0];
    let client = run_held(
        &mut sv,
        move |addr| play(&root, addr),
        |sv, _, _| {
            // the bot on the tiles, us behind the light (in the room, so the
            // snapshots carry the bot)
            if let Some(l) = sv.game.level.as_mut() {
                for slot in 0..4 {
                    let at = if slot == bot { BODY_AT } else { OUR_AT };
                    let Some(ps) = l.playerstate(slot) else {
                        continue;
                    };
                    let o = [0, 1]
                        .map(|k| f32::from_bits(ps.get(&format!("origin[{k}]")).unwrap_or(0)));
                    if (o[0] - at.x).hypot(o[1] - at.y) > 2.0 {
                        l.set_view_pos(slot, [at.x, at.y, at.z + 60.0], [0.0, 67.5, 0.0]);
                    }
                }
            }
            true
        },
    );
    let Some(seen) = client else {
        return;
    };
    println!("{seen:#?}");
    assert_eq!(seen.chosen, Some(LIGHT));
    assert!(seen.darker > 300, "{seen:?}");
    assert!(seen.brighter < seen.darker / 20, "{seen:?}");
    assert!(seen.changed > seen.darker, "{seen:?}");
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
        // from 30 under the light, 40 degrees down along its most open side
        let world = c.session.world().expect("the session's world");
        let eye = LIGHT_AT - Vec3::Z * 30.0;
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
        let pitch = (-40f32).to_radians();
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
        println!("eye {eye} yaw {}", yaw.to_degrees());
        // the pick, then past its 1 s fade
        r.set_spot_shadow(crate::cod4_spotshadow::Settings::default());
        r.render(frame(time), None);
        r.render(frame(time + 2.0), None);
        seen.chosen = r.shadowed_spots().iter().map(|s| s.0).find(|&l| l == LIGHT);
        let on = shot(&mut r, "spot_on");
        let render = |r: &mut Renderer, settings, name| {
            r.set_spot_shadow(settings);
            r.render(frame(time + 2.0), None);
            shot(r, name)
        };
        // the same light kept, its fade run: then the map off
        let no_ents = render(
            &mut r,
            crate::cod4_spotshadow::Settings {
                entities: false,
                ..Default::default()
            },
            "spot_no_entities",
        );
        let off = render(
            &mut r,
            crate::cod4_spotshadow::Settings {
                on: false,
                ..Default::default()
            },
            "spot_off",
        );
        let count = |a: &[u8], b: &[u8]| {
            a.chunks(4).zip(b.chunks(4)).fold((0, 0), |(d, l), (p, q)| {
                let k = luma(p) - luma(q);
                (d + usize::from(k < -8), l + usize::from(k > 8))
            })
        };
        (seen.darker, seen.brighter) = count(&on, &no_ents);
        seen.changed = {
            let (d, l) = count(&on, &off);
            d + l
        };
        break;
    }
    c.session.disconnect();
    let errors = errors.lock().unwrap();
    assert!(errors.is_empty(), "{errors:?}");
    Some(seen)
}
