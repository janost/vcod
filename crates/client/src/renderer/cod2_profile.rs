//! The CoD2 renderer profiles, each a `client_tools` run (`COD2_DIR`, a
//! headless adapter with BC textures: lavapipe will do; maps from
//! `VCOD_PROFILE_MAPS`, space separated). They print what they measured
//! and pass or fail nothing but a wgpu validation error.

use glam::Vec3;

use super::frame_rig::{assert_clean, draw_frames};
use super::*;

/// Times `Renderer::cull` (the cell walk and the static
/// models' cell test, the profile's `cull`) from each map's
/// `mp_global_intermission` view, where `cod4_perf.sh`'s CoD2 spectator
/// stands (cod2-client-perf.md 8). Maps from `VCOD_PROFILE_MAPS`.
pub fn cod2_cull_profile() {
    let Some(fs) = vcod_common::testing::cod2_fs() else {
        return;
    };
    let Some((mut r, _errors)) = Renderer::headless(&fs, false, 1) else {
        eprintln!("no adapter with BC textures; skipped");
        return;
    };
    let maps =
        std::env::var("VCOD_PROFILE_MAPS").unwrap_or_else(|_| "mp_toujane mp_carentan".into());
    for map in maps.split_whitespace() {
        let data = fs.read(&format!("maps/mp/{map}.d3dbsp")).expect(map);
        let bsp = vcod_cod2::world::from_bytes(&data).unwrap();
        r.load_world(&bsp, &fs).unwrap();
        let spot = vcod_common::bsp::entity_blocks(&bsp.entities)
            .into_iter()
            .find(|b| b.get("classname").map(String::as_str) == Some("mp_global_intermission"))
            .expect("an intermission spot");
        let eye = Vec3::from(vcod_common::bsp::parse_vec3(&spot["origin"]).unwrap());
        let ang = spot
            .get("angles")
            .and_then(|a| vcod_common::bsp::parse_vec3(a))
            .unwrap_or([0.0; 3]);
        let view_proj = crate::camera::view_proj_from(
            eye,
            ang[1].to_radians(),
            ang[0].to_radians(),
            0.0,
            80f32.to_radians(),
            640.0 / 372.0,
        );
        let frustum = Frustum::from_view_proj(view_proj);
        let world = r.world.as_ref().unwrap();
        const ROUNDS: u32 = 200;
        let (mut walk, mut props) = (0.0, 0.0);
        let mut seen = (0, 0);
        for _ in 0..ROUNDS {
            let t = std::time::Instant::now();
            let v = world.vis.visible(eye, &frustum, Some(view_proj));
            walk += t.elapsed().as_secs_f64();
            let t = std::time::Instant::now();
            let ok = world
                .prop_bounds
                .iter()
                .filter(|&&(lo, hi)| world.vis.prop_visible(&v, &frustum, lo, hi))
                .count();
            props += t.elapsed().as_secs_f64();
            seen = (v.soups.iter().filter(|&&s| s).count(), ok);
        }
        let ms = |s: f64| s * 1000.0 / f64::from(ROUNDS);
        eprintln!(
            "CULL {map}: walk {:.3} ms, props {:.3} ms ({} placements), {} soups, {} props seen, {} cells visited",
            ms(walk),
            ms(props),
            world.prop_bounds.len(),
            seen.0,
            seen.1,
            world
                .vis
                .visible(eye, &frustum, Some(view_proj))
                .stats
                .cells_visited,
        );
    }
}

/// Draws each map's spawn views (eye 60 up, four yaws) under
/// the CoD1 recursive cell walk and under retail's queued one
/// (`MapWorld::portals_only`, cod2-client-perf.md 9) and counts the pixels
/// that differ and the soups each marked: a soup the queued walk drops
/// that a view shows is a hole. Maps from `VCOD_PROFILE_MAPS`; at most
/// `VCOD_WALK_SPAWNS` (default 8) spawns a map.
pub fn cod2_cell_walks_draw_alike() {
    let Some(fs) = vcod_common::testing::cod2_fs() else {
        return;
    };
    let Some((mut r, errors)) = Renderer::headless(&fs, false, 1) else {
        eprintln!("no adapter with BC textures; skipped");
        return;
    };
    r.set_gamma(1.0, crate::gamma::overbright_bits(fs.game(), false, true));
    vcod_cod2::model::register();
    let maps =
        std::env::var("VCOD_PROFILE_MAPS").unwrap_or_else(|_| "mp_toujane mp_carentan".into());
    let spawns: usize = std::env::var("VCOD_WALK_SPAWNS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(8);
    for map in maps.split_whitespace() {
        let data = fs.read(&format!("maps/mp/{map}.d3dbsp")).expect(map);
        let mut bsp = vcod_cod2::world::from_bytes(&data).unwrap();
        let views: Vec<(Vec3, f32)> = vcod_common::bsp::entity_blocks(&bsp.entities)
            .into_iter()
            .filter(|b| {
                b.get("classname")
                    .is_some_and(|c| c.starts_with("mp_") && c.ends_with("_spawn"))
            })
            .filter_map(|b| vcod_common::bsp::parse_vec3(b.get("origin")?))
            .take(spawns)
            .flat_map(|o| (0..4).map(move |k| (Vec3::from(o) + Vec3::Z * 60.0, 90.0 * k as f32)))
            .collect();
        let mut drawn: Vec<Vec<Vec<u8>>> = Vec::new();
        for queued in [false, true] {
            bsp.portals_only = queued;
            r.load_world(&bsp, &fs).unwrap();
            let mut frames = Vec::new();
            for &at in &views {
                draw_frames(&mut r, &fs, at, &[], || None);
                assert_clean(&errors, &format!("CoD2 {map} walk frame"));
                frames.push(r.read_offscreen().expect("offscreen target").2);
            }
            drawn.push(frames);
        }
        let mut worst = (0usize, (Vec3::ZERO, 0.0));
        let mut total = 0usize;
        let mut views_differ = 0usize;
        for (i, (a, b)) in drawn[0].iter().zip(&drawn[1]).enumerate() {
            let n = a
                .chunks(4)
                .zip(b.chunks(4))
                .filter(|(p, q)| p[..3] != q[..3])
                .count();
            total += n;
            views_differ += usize::from(n > 0);
            if let (true, Some(dir)) = (n > 0, std::env::var_os("VCOD_C4_SHOTS")) {
                let (w, h, _) = r.read_offscreen().expect("offscreen target");
                for (walk, px) in [("recursive", a), ("queued", b)] {
                    let mut ppm = format!("P6\n{w} {h}\n255\n").into_bytes();
                    ppm.extend(px.chunks(4).flat_map(|p| [p[2], p[1], p[0]]));
                    let name = format!("{map}_walk{i}_{walk}.ppm");
                    std::fs::write(std::path::Path::new(&dir).join(name), ppm).unwrap();
                }
            }
            if n > worst.0 {
                worst = (n, views[i]);
            }
        }
        eprintln!(
            "WALKDRAW {map}: {} views, {views_differ} differ, {total} pixels differ, most {} at {:?} yaw {}",
            views.len(),
            worst.0,
            worst.1.0,
            worst.1.1
        );
    }
}

/// Times CoD2 map loads through the renderer, the session's
/// collision and the first draw's models, each map twice (the second
/// load reuses what the first cached), with a digest of every texture
/// uploaded so a faster load can show it uploads the same. Maps from
/// `VCOD_PROFILE_MAPS` (space-separated). Stage lines log at info.
pub fn cod2_load_profile() {
    let _ = env_logger::builder()
        .is_test(true)
        .filter_level(log::LevelFilter::Info)
        .try_init();
    let Some(fs) = vcod_common::testing::cod2_fs() else {
        return;
    };
    let t = std::time::Instant::now();
    let Some((mut r, _errors)) = Renderer::headless(&fs, false, 1) else {
        eprintln!("no adapter with BC textures; skipped");
        return;
    };
    eprintln!(
        "PROFILE renderer build {:.0} ms",
        t.elapsed().as_secs_f64() * 1000.0
    );
    vcod_cod2::model::register();
    let maps = std::env::var("VCOD_PROFILE_MAPS")
        .unwrap_or_else(|_| "mp_toujane mp_carentan mp_burgundy".into());
    let view = crate::cod2_scene::fixture::replay();
    let f = crate::cod2_scene::fixture::mid(&view);
    let ms = |t: std::time::Instant| t.elapsed().as_secs_f64() * 1000.0;
    let mut draw = crate::cod2_scene::Cod2Draw::new();
    let mut audio = crate::audio::AudioSystem::new(
        &fs,
        crate::audio::AudioOpts {
            enabled: false,
            volume: 1.0,
            rate: 0,
        },
    );
    for round in 0..2 {
        for map in maps.split_whitespace() {
            let total = std::time::Instant::now();
            let t = std::time::Instant::now();
            let data = fs.read(&format!("maps/mp/{map}.d3dbsp")).expect(map);
            let file = vcod_cod2::map::MapFile::parse(&data).unwrap();
            let bsp = file.world();
            let parse = ms(t);
            // as `load_cod2_map` loads: the prediction's collision beside
            let t = std::time::Instant::now();
            super::upload_digest::start();
            fs.clear_references();
            let collision = std::thread::scope(|sc| {
                let cw = sc.spawn(|| {
                    let t = std::time::Instant::now();
                    let clip = std::sync::Arc::new(file.clip().unwrap());
                    crate::play::cod2::prediction_world(&bsp, clip);
                    ms(t)
                });
                r.load_world(&bsp, &fs).unwrap();
                vcod_common::laps::joined(cw)
            });
            let (digest, textures) = super::upload_digest::take().unwrap();
            let paks: Vec<String> = fs
                .referenced_paks()
                .iter()
                .map(|p| p.name.clone())
                .collect();
            let world = ms(t);
            let t = std::time::Instant::now();
            // one draw across maps, as `load_cod2_map` carries it
            draw.next_map();
            draw.draw(&f, &fs, &mut r);
            let scene = ms(t);
            let t = std::time::Instant::now();
            audio.on_gamestate(map);
            let sounds = ms(t);
            let t = std::time::Instant::now();
            let rig = crate::cod2_viewmodel::load_rig(&fs, "kar98k_mp", None).unwrap();
            r.set_viewmodel(&fs, &rig.models);
            let gun = ms(t);
            eprintln!(
                "PROFILE round {round} {map}: total {:.0} ms: read+parse {parse:.0}, \
                 renderer with collision {world:.0} (collision {collision:.0}), first scene {scene:.0}, \
                 view weapon {gun:.0}, sound aliases {sounds:.0}; {textures} textures digest {digest:016x}, paks {}",
                ms(total),
                paks.join(" ")
            );
        }
    }
}
