//! What a CoD4 frame draws and binds (cod4-client-perf.md 21): vcod-server's
//! war on mp_crossfire with us joined, the session's renderer at 1920 x
//! 1080 and one sample (`r_aaSamples`' CoD4 default) with the depth
//! prepass on, the eye held at the map's heavy views. Per view it prints
//! the [`PassCensus`] and the frame's encode-to-readback time (lavapipe's)
//! with the draws and binds one by one and joined (`draw_joins`), and
//! holds the two frames byte for byte. Needs `COD4_DIR` and an adapter with BC textures
//! (lavapipe will do).

use super::cod4_worldfx_live::session_sized;
use super::*;
use glam::Vec3;

/// mp_crossfire's views: `cod4_perf.sh`'s held one (cod4-client-perf.md
/// 12, 19), the heaviest of `cull_census`' spawn views beside it (the
/// same spot turned to 270: 145k triangles there), and one near the
/// census's median (26k against its 25k).
/// `VCOD_CENSUS_VIEWS="x y z yaw pitch; ..."` replaces them.
const VIEWS: [[f32; 5]; 3] = [
    [5232.0, -888.0, 240.0, 225.0, 0.0],
    [5232.0, -888.0, 240.0, 270.0, 0.0],
    [3990.0, -1110.0, 60.0, 180.0, 0.0],
];

fn views() -> Vec<(Vec3, f32, f32)> {
    let parse = |v: &[f32]| (Vec3::new(v[0], v[1], v[2]), v[3], v[4]);
    match std::env::var("VCOD_CENSUS_VIEWS") {
        Ok(s) => s
            .split(';')
            .map(|v| {
                let v: Vec<f32> = v.split_whitespace().map(|x| x.parse().unwrap()).collect();
                parse(&v)
            })
            .collect(),
        Err(_) => VIEWS.iter().map(|v| parse(v)).collect(),
    }
}

#[test]
fn cod4_live_crossfire_draw_census() {
    use std::sync::{Arc, Mutex};
    let rows = Arc::new(Mutex::new(Vec::new()));
    let out = rows.clone();
    let mut at = 0;
    let views = views();
    let want = views.len();
    let ran = session_sized("mp_crossfire", &[], (1, (1920, 1080)), move |l| {
        if l.t < 2.0 {
            return false;
        }
        let Some(&(eye, yaw, pitch)) = views.get(at) else {
            return true;
        };
        at += 1;
        l.eye = eye;
        l.yaw = yaw.to_radians();
        l.pitch = pitch.to_radians();
        l.r.set_depth_prepass(true);
        let frame = |l: &mut super::cod4_worldfx_live::Live, joins: bool| {
            l.r.draw_joins = joins;
            let name = format!("crossfire_census_{at}_{}", u8::from(joins));
            // the caches settle on the first frame
            let first = {
                let quads = l.quads();
                l.shot_with(Cod4Options::default(), quads, &name)
            };
            let t = std::time::Instant::now();
            const RUNS: u32 = 3;
            for _ in 0..RUNS {
                let again = {
                    let quads = l.quads();
                    l.shot_with(Cod4Options::default(), quads, &name)
                };
                assert!(again == first, "view {at}: the frame holds still");
            }
            let ms = t.elapsed().as_secs_f32() * 1000.0 / RUNS as f32;
            (first, l.r.census(), ms)
        };
        let (one, c1, ms1) = frame(l, false);
        let (joined, c2, ms2) = frame(l, true);
        let tris = l.r.vis_counts().tris.0;
        l.r.draw_joins = true;
        out.lock()
            .unwrap()
            .push((at, tris, c1, ms1, c2, ms2, one == joined));
        false
    });
    if ran.is_none() {
        return;
    }
    let rows = rows.lock().unwrap();
    for (at, tris, c1, ms1, c2, ms2, same) in rows.iter() {
        for (label, c, ms) in [("one by one", c1, ms1), ("joined", c2, ms2)] {
            println!(
                "view {at} ({tris} tris) {label}: {} draws (sky {} world {} ents {} gun {} marks {} fx {} dlights {} prepass world {} ents {}), {} pipelines, {} bind groups, {ms:.0} ms",
                c.draws(),
                c.sky,
                c.world,
                c.ents,
                c.gun,
                c.marks,
                c.fx,
                c.dlights,
                c.prepass_world,
                c.prepass_ents,
                c.pipelines,
                c.bind_groups,
            );
        }
        assert!(same, "view {at}: the joined prepass changed the frame");
        assert!(c2.prepass_world <= c1.prepass_world);
    }
    assert_eq!(rows.len(), want, "every view drew");
}
