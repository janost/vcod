//! Launched car parts against mp_backlot's steps (`vcod_cod4::physics`,
//! cod4-gsc.md 3.6.1): a sedan's hood and door dropped edge-first across
//! a step's edge come to rest with no world solid inside their boxes.
//! `COD4_DIR` unset passes vacuously.

use glam::Vec3;
use vcod_cod4::assets::Assets;
use vcod_cod4::collision::ClipMap;
use vcod_cod4::map::Map;
use vcod_cod4::physics::{Body, Dvars, ShapeBox, clip_solids, clip_trace, part_models};
use vcod_cod4::ragdoll::CLIP_MASK;
use vcod_cod4::testing::cod4_install;
use vcod_cod4::walk::{WalkOptions, walk};
use vcod_cod4::zone::Zone;

fn zone(name: &str) -> Option<Assets> {
    let install = cod4_install()?;
    let z = Zone::open_path(&install.zone("english", name)).unwrap();
    Some(walk(z, WalkOptions { keep_bulk: true }).unwrap().assets)
}

/// The floor under `(x, y)` from `top` down 160 units: its height and
/// normal, `None` from inside the world or over nothing.
fn floor(clip: &ClipMap, x: f32, y: f32, top: f32) -> Option<(f32, Vec3)> {
    let (a, b) = (Vec3::new(x, y, top), Vec3::new(x, y, top - 160.0));
    let t = clip.trace(a, b, Vec3::ZERO, Vec3::ZERO, CLIP_MASK);
    (!t.startsolid && t.fraction < 1.0).then_some((a.z + (b.z - a.z) * t.fraction, t.normal))
}

/// Steps near the spawns: two floors 8 units apart, both level, the
/// upper 6 to 18 above the lower, with a 72 by 72 by 56 box of room over
/// the edge. Each is the point over the edge at the upper floor's height
/// and the level direction up the step; one per 192 units.
fn steps(m: &Map, clip: &ClipMap) -> Vec<(Vec3, Vec3)> {
    let mut spawns: Vec<Vec3> = m
        .entities
        .iter()
        .filter(|e| {
            e.iter()
                .any(|(k, v)| k == "classname" && v == "mp_tdm_spawn")
        })
        .filter_map(|e| {
            let (_, o) = e.iter().find(|(k, _)| k == "origin")?;
            let v: Vec<f32> = o
                .split_whitespace()
                .filter_map(|s| s.parse().ok())
                .collect();
            (v.len() == 3).then(|| Vec3::new(v[0], v[1], v[2]))
        })
        .collect();
    spawns.sort_by(|a, b| a.to_array().partial_cmp(&b.to_array()).unwrap());
    let mut out: Vec<(Vec3, Vec3)> = Vec::new();
    for s in spawns {
        'grid: for i in -24..=24 {
            for j in -24..=24 {
                let (x, y) = (s.x + i as f32 * 8.0, s.y + j as f32 * 8.0);
                let Some((z0, n0)) = floor(clip, x, y, s.z + 48.0) else {
                    continue;
                };
                for dir in [Vec3::X, Vec3::Y, -Vec3::X, -Vec3::Y] {
                    let Some((z1, n1)) = floor(clip, x + dir.x * 8.0, y + dir.y * 8.0, s.z + 48.0)
                    else {
                        continue;
                    };
                    if n0.z < 0.99 || n1.z < 0.99 || !(6.0..=18.0).contains(&(z1 - z0)) {
                        continue;
                    }
                    let at = Vec3::new(x + dir.x * 4.0, y + dir.y * 4.0, z1);
                    // Room for a door or a hood tipped over it.
                    let room = Vec3::new(36.0, 36.0, 28.0);
                    let over = at + Vec3::Z * (room.z + 1.0);
                    if clip.trace(over, over, -room, room, CLIP_MASK).startsolid {
                        continue;
                    }
                    if out.iter().all(|(o, _)| o.distance(at) > 192.0) {
                        out.push((at, dir));
                    }
                    continue 'grid;
                }
            }
        }
    }
    out
}

/// How deep the world reaches into box `b`: the largest of the insets
/// 0.25, 0.5, 1, 2, 4 whose box still has world solid in it, found by
/// point traces both ways along a 7 by 7 grid of lines through it on
/// each axis; 0 when none has.
fn solid_inside(clip: &ClipMap, b: &ShapeBox) -> f32 {
    let axes = [b.axes.x_axis, b.axes.y_axis, b.axes.z_axis];
    let mut deepest = 0.0;
    for inset in [0.25f32, 0.5, 1.0, 2.0, 4.0] {
        let h = (b.half - Vec3::splat(inset)).max(Vec3::ZERO);
        let mut hit = false;
        for k in 0..3 {
            let (u, v) = ((k + 1) % 3, (k + 2) % 3);
            for i in 0..7 {
                for j in 0..7 {
                    let s = |n: i32| (n as f32 / 3.0) - 1.0;
                    let mid = b.centre + axes[u] * (h[u] * s(i)) + axes[v] * (h[v] * s(j));
                    let (p, q) = (mid - axes[k] * h[k], mid + axes[k] * h[k]);
                    let z = Vec3::ZERO;
                    hit |= [(p, q), (q, p)].iter().any(|&(a, c)| {
                        let t = clip.trace(a, c, z, z, CLIP_MASK);
                        t.startsolid || t.fraction < 1.0
                    });
                }
            }
        }
        if !hit {
            break;
        }
        deepest = inset;
    }
    deepest
}

/// mp_backlot's sedan hood (a brush geom) and left front door (its bounds
/// box), each dropped from 24 units over six steps' edges, across them
/// and along them, tipped 30 degrees so an edge leads: every drop goes
/// to sleep within 10 s, its points out of the world, and no world solid
/// a quarter unit or more inside its box.
#[test]
fn parts_dropped_edge_first_on_backlots_steps_rest_clear_of_them() {
    let (Some(install), Some(common)) = (cod4_install(), zone("common_mp")) else {
        return;
    };
    let (m, walked) = Map::load(&install, "english", "mp_backlot").unwrap();
    let clip = ClipMap::from_assets(&walked.assets).unwrap();
    let parts = part_models(&[&walked.assets, &common]);
    let edges = steps(&m, &clip);
    println!("{} steps", edges.len());
    assert!(edges.len() >= 6, "{edges:?}");
    let d = Dvars::default();
    let mut problems = Vec::new();
    for name in [
        "vehicle_80s_sedan1_green_hood",
        "vehicle_80s_sedan1_green_door_lf",
    ] {
        let part = parts.get(name).unwrap_or_else(|| {
            let mut k: Vec<_> = parts.keys().filter(|k| k.contains("sedan")).collect();
            k.sort();
            panic!("no {name}: {k:?}")
        });
        assert_eq!(part.shape.boxes.len(), 1, "{name}");
        for &(at, up) in &edges[..6] {
            let yaw = up.y.atan2(up.x).to_degrees();
            for (turn, roll) in [(0.0, 30.0), (90.0, 30.0), (45.0, -30.0)] {
                let angles = Vec3::new(0.0, yaw + turn, roll);
                // The box's centre 24 units over the edge.
                let rot = vcod_common::turretpose::angles_quat(angles.to_array());
                let origin = at + Vec3::Z * 24.0 - rot * part.shape.boxes[0].centre;
                let mut b = Body::new(origin, angles, &part.shape, &part.preset, 0);
                let clear = b.world_boxes().all(|w| solid_inside(&clip, &w) == 0.0);
                assert!(clear, "{name} at {at} turn {turn}: starts in the world");
                let mut trace = clip_trace(&clip);
                let mut solids = clip_solids(&clip);
                let mut t = 0;
                while t < 10_000 && !b.asleep {
                    t += 16;
                    b.advance_in(t, &mut trace, &mut solids, &d);
                }
                let depth = b
                    .world_boxes()
                    .map(|w| solid_inside(&clip, &w))
                    .fold(0.0f32, f32::max);
                let z = Vec3::ZERO;
                let points_in = b
                    .world_points()
                    .filter(|&p| clip.trace(p, p, z, z, CLIP_MASK).startsolid)
                    .count();
                let line = format!(
                    "{name} at {at} turn {turn} roll {roll} : asleep {} after {t} ms, {} away, solid {depth} deep, {points_in} points in",
                    b.asleep,
                    b.origin().distance(origin)
                );
                println!("{line}");
                if !b.asleep || depth > 0.0 || points_in > 0 {
                    problems.push(line);
                }
            }
        }
    }
    assert!(problems.is_empty(), "{problems:#?}");
}
