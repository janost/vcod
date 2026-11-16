//! Launched car parts against mp_backlot's steps and dynamic entities
//! against mp_crash's and the cylinder props' maps' (`vcod_cod4::physics`,
//! cod4-gsc.md 3.6.1): each dropped edge-first across a step's edge comes
//! to rest with no world solid inside its box or cylinder.
//! `COD4_DIR` unset passes vacuously.

use glam::Vec3;
use vcod_cod4::assets::Assets;
use vcod_cod4::collision::ClipMap;
use vcod_cod4::map::Map;
use vcod_cod4::physics::{
    Body, Dvars, Preset, Shape, ShapeBox, ShapeCylinder, clip_solids, clip_trace, part_models,
};
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

/// [`solid_inside`] for a cylinder: point traces both ways along lines
/// through it parallel to its axis (its centre line and three rings of 8
/// out to the inset radius) and across it (8 diameters at 5 heights).
fn solid_inside_cylinder(clip: &ClipMap, cy: &ShapeCylinder) -> f32 {
    let a = cy.axis;
    let t0 = a.any_orthonormal_vector();
    let t1 = a.cross(t0);
    let mut deepest = 0.0;
    for inset in [0.25f32, 0.5, 1.0, 2.0, 4.0] {
        let (r, h) = ((cy.radius - inset).max(0.0), (cy.half - inset).max(0.0));
        let dir = |k: usize| {
            let t = k as f32 * std::f32::consts::TAU / 8.0;
            t0 * t.cos() + t1 * t.sin()
        };
        let mut lines = vec![(cy.centre - a * h, cy.centre + a * h)];
        for ring in [r / 3.0, 2.0 * r / 3.0, r] {
            for k in 0..8 {
                let o = cy.centre + dir(k) * ring;
                lines.push((o - a * h, o + a * h));
            }
        }
        for j in 0..5 {
            let o = cy.centre + a * (h * (j as f32 / 2.0 - 1.0));
            for k in 0..8 {
                lines.push((o - dir(k) * r, o + dir(k) * r));
            }
        }
        let z = Vec3::ZERO;
        let hit = lines.iter().any(|&(p, q)| {
            [(p, q), (q, p)].iter().any(|&(x, y)| {
                let t = clip.trace(x, y, z, z, CLIP_MASK);
                t.startsolid || t.fraction < 1.0
            })
        });
        if !hit {
            break;
        }
        deepest = inset;
    }
    deepest
}

/// How deep the world reaches into any of `b`'s boxes and cylinders.
fn body_depth(clip: &ClipMap, b: &Body) -> f32 {
    b.world_boxes()
        .map(|w| solid_inside(clip, &w))
        .chain(b.world_cylinders().map(|c| solid_inside_cylinder(clip, &c)))
        .fold(0.0f32, f32::max)
}

/// `shape` with `preset` dropped from 24 units over the first six of
/// `edges`, across them, along them and at 45 degrees, tipped 30 degrees
/// so an edge leads, stepped as `advance` steps it to 30 s: a line per
/// drop that does not sleep, ends with a point in the world, or with world
/// solid a quarter unit or more inside its box or cylinder. A cylinder
/// still rolling down a slope at the end passes: a bottle's spin takes
/// past 10 s to fall under `phys_autoDisableAngular` through
/// `phys_dragAngular`, and a slope keeps it up. A shape with no box drops
/// its first cylinder's centre there.
fn drop_on_steps(
    clip: &ClipMap,
    edges: &[(Vec3, Vec3)],
    name: &str,
    shape: &Shape,
    preset: &Preset,
    advance: &mut dyn FnMut(&mut Body, i32),
) -> Vec<String> {
    let mut problems = Vec::new();
    for &(at, up) in &edges[..6] {
        let yaw = up.y.atan2(up.x).to_degrees();
        for (turn, roll) in [(0.0, 30.0), (90.0, 30.0), (45.0, -30.0)] {
            let angles = Vec3::new(0.0, yaw + turn, roll);
            // The box's centre 24 units over the edge.
            let rot = vcod_common::turretpose::angles_quat(angles.to_array());
            let centre = shape
                .boxes
                .first()
                .map_or_else(|| shape.cylinders[0].centre, |b| b.centre);
            let origin = at + Vec3::Z * 24.0 - rot * centre;
            let mut b = Body::new(origin, angles, shape, preset, 0);
            let clear = body_depth(clip, &b) == 0.0;
            assert!(clear, "{name} at {at} turn {turn}: starts in the world");
            let mut t = 0;
            while t < 30_000 && !b.asleep {
                t += 16;
                advance(&mut b, t);
            }
            let depth = body_depth(clip, &b);
            let z = Vec3::ZERO;
            let points_in = b
                .world_points()
                .filter(|&p| clip.trace(p, p, z, z, CLIP_MASK).startsolid)
                .count();
            // Still rolling down a slope: its speed its spin about its
            // axis times its radius.
            let rolling = !b.asleep
                && b.world_cylinders().next().is_some_and(|c| {
                    let spin = b.avel.dot(c.axis).abs() * c.radius;
                    let v = b.vel.length();
                    v > 0.0 && (v - spin).abs() < 0.25 * v
                });
            let line = format!(
                "{name} at {at} turn {turn} roll {roll}: asleep {} rolling {rolling} after {t} ms, {} away, solid {depth} deep, {points_in} points in",
                b.asleep,
                b.origin().distance(origin)
            );
            println!("{line}");
            if !(b.asleep || rolling) || depth > 0.0 || points_in > 0 {
                problems.push(line);
            }
        }
    }
    problems
}

/// mp_backlot's sedan hood (a brush geom) and left front door (its bounds
/// box) through [`drop_on_steps`] on the client's launched-part path.
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
        let part = &parts[name];
        assert_eq!(part.shape.boxes.len(), 1, "{name}");
        let mut trace = clip_trace(&clip);
        let mut solids = clip_solids(&clip);
        problems.extend(drop_on_steps(
            &clip,
            &edges,
            name,
            &part.shape,
            &part.preset,
            &mut |b, t| b.advance_in(t, &mut trace, &mut solids, &d),
        ));
    }
    assert!(problems.is_empty(), "{problems:#?}");
}

/// mp_crash's dynamic entities, one def of each model whose shape has a
/// box (16 models; the cylinders' are [`cylinder_props_dropped_edge_first_rest_clear_of_steps`]'s), through
/// [`drop_on_steps`] on mp_crash's steps under the mask
/// `DynEnts::advance_in` gives a body that starts in no clip (cod4-world.md
/// 13).
#[test]
fn dynents_dropped_edge_first_on_crashs_steps_rest_clear_of_them() {
    let (Some(install), Some(common)) = (cod4_install(), zone("common_mp")) else {
        return;
    };
    let (m, walked) = Map::load(&install, "english", "mp_crash").unwrap();
    let clip = ClipMap::from_assets(&walked.assets).unwrap();
    let defs = vcod_cod4::dynent::defs(&walked.assets, &[&common]);
    let edges = steps(&m, &clip);
    println!("{} steps", edges.len());
    assert!(edges.len() >= 6, "{edges:?}");
    let mut seen = std::collections::BTreeMap::new();
    for d in &defs {
        let Some((shape, preset)) = &d.body else {
            continue;
        };
        let Some(b) = shape.boxes.first() else {
            continue;
        };
        println!("{} {:?} mass {}", d.model, b.half * 2.0, preset.mass);
        seen.entry(d.model.clone())
            .or_insert(((**shape).clone(), *preset));
    }
    let d = Dvars::default();
    let mut problems = Vec::new();
    for (name, (shape, preset)) in &seen {
        let mut trace = clip_trace(&clip);
        let mut solids = clip_solids(&clip);
        problems.extend(drop_on_steps(
            &clip,
            &edges,
            name,
            shape,
            preset,
            &mut |b, t| b.advance_in(t, &mut trace, &mut solids, &d),
        ));
    }
    assert!(problems.is_empty(), "{problems:#?}");
}

/// The stock dynamic entities whose shape has a cylinder (barrels,
/// cans, bottles, pots, tires, hub caps, fruit), one def of each model
/// from the first `mp_` zone placing it, through [`drop_on_steps`] on
/// mp_crash's steps.
#[test]
fn cylinder_props_dropped_edge_first_rest_clear_of_steps() {
    let (Some(install), Some(common)) = (cod4_install(), zone("common_mp")) else {
        return;
    };
    let mut maps: Vec<String> = std::fs::read_dir(install.zone_dir("english"))
        .unwrap()
        .filter_map(|e| {
            let name = e.ok()?.file_name().into_string().ok()?;
            let map = name.strip_suffix(".ff")?;
            (map.starts_with("mp_") && !map.ends_with("_load")).then(|| map.to_string())
        })
        .collect();
    maps.sort();
    let mut seen = std::collections::BTreeMap::new();
    for map in &maps {
        let Some(walked) = zone(map) else { continue };
        for def in vcod_cod4::dynent::defs(&walked, &[&common]) {
            let Some((shape, preset)) = &def.body else {
                continue;
            };
            if shape.cylinders.is_empty() || seen.contains_key(&def.model) {
                continue;
            }
            let c = &shape.cylinders[0];
            println!(
                "{map} {}: {} cylinders, {} boxes, radius {} half {} mass {}",
                def.model,
                shape.cylinders.len(),
                shape.boxes.len(),
                c.radius,
                c.half,
                preset.mass
            );
            seen.insert(def.model.clone(), ((**shape).clone(), *preset));
        }
    }
    assert!(!seen.is_empty());
    let (m, walked) = Map::load(&install, "english", "mp_crash").unwrap();
    let clip = ClipMap::from_assets(&walked.assets).unwrap();
    let edges = steps(&m, &clip);
    assert!(edges.len() >= 6, "{edges:?}");
    let d = Dvars::default();
    let mut problems = Vec::new();
    for (name, (shape, preset)) in &seen {
        let mut trace = clip_trace(&clip);
        let mut solids = clip_solids(&clip);
        problems.extend(drop_on_steps(
            &clip,
            &edges,
            name,
            shape,
            preset,
            &mut |b, t| b.advance_in(t, &mut trace, &mut solids, &d),
        ));
    }
    println!("{} models, {} problems", seen.len(), problems.len());
    assert!(problems.is_empty(), "{problems:#?}");
}
