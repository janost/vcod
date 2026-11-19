//! Launched car parts against mp_backlot's steps and dynamic entities
//! against mp_crash's and the cylinder props' maps' (`vcod_cod4::physics`,
//! cod4-gsc.md 3.6.1): each dropped edge-first across a step's edge comes
//! to rest with no world solid inside its box or cylinder; picture frames
//! wedged between two walls on mp_crash's floor rest as a pinched body's
//! contact filter lets them.
//! `COD4_DIR` unset passes vacuously.

use glam::{Mat3, Quat, Vec3};
use vcod_cod4::assets::Assets;
use vcod_cod4::collision::ClipMap;
use vcod_cod4::map::Map;
use vcod_cod4::physics::{
    Body, Dvars, Preset, Shape, ShapeBox, ShapeCylinder, Solid, WorldClock, clip_solids,
    clip_trace, msec_step, part_models,
};
use vcod_cod4::ragdoll::CLIP_MASK;
use vcod_cod4::ragdoll::Sweep;
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
/// so an edge leads, stepped as `advance` steps it to 60 s: a line per
/// drop that does not sleep, ends with a point in the world, or with world
/// solid a quarter unit or more inside its box or cylinder. A bottle
/// rolling off a step keeps its spin over `phys_autoDisableAngular`
/// through `phys_dragAngular` past 30 s (cod4-gsc.md 3.6.1). A shape with
/// no box drops its first cylinder's centre there.
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
            while t < 60_000 && !b.asleep {
                t += 16;
                advance(&mut b, t);
            }
            let depth = body_depth(clip, &b);
            let z = Vec3::ZERO;
            let points_in = b
                .world_points()
                .filter(|&p| clip.trace(p, p, z, z, CLIP_MASK).startsolid)
                .count();
            let line = format!(
                "{name} at {at} turn {turn} roll {roll}: asleep {} after {t} ms, {} away, solid {depth} deep, {points_in} points in",
                b.asleep,
                b.origin().distance(origin)
            );
            println!("{line}");
            if !b.asleep || depth > 0.0 || points_in > 0 {
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

/// The first row's 36 drops as bodies made at time 0: the sedan hood and
/// left front door 24 units over each of the first six step edges, across,
/// along and at 45 degrees, tipped 30. Each with its name and origin.
fn dropped_together(
    parts: &std::collections::HashMap<String, std::sync::Arc<vcod_cod4::physics::PartModel>>,
    edges: &[(Vec3, Vec3)],
) -> Vec<(String, Vec3, Body)> {
    let mut bodies = Vec::new();
    for name in [
        "vehicle_80s_sedan1_green_hood",
        "vehicle_80s_sedan1_green_door_lf",
    ] {
        let part = &parts[name];
        for &(at, up) in &edges[..6] {
            let yaw = up.y.atan2(up.x).to_degrees();
            for (turn, roll) in [(0.0, 30.0), (90.0, 30.0), (45.0, -30.0)] {
                let angles = Vec3::new(0.0, yaw + turn, roll);
                let rot = vcod_common::turretpose::angles_quat(angles.to_array());
                let origin = at + Vec3::Z * 24.0 - rot * part.shape.boxes[0].centre;
                let b = Body::new(origin, angles, &part.shape, &part.preset, 0);
                bodies.push((format!("{name} at {at} turn {turn}"), origin, b));
            }
        }
    }
    assert_eq!(bodies.len(), 36);
    bodies
}

/// The row of [`parts_dropped_edge_first_on_backlots_steps_rest_clear_of_them`]
/// dropped all at once, its 36 bodies in one physics world 1 at 16 ms
/// frames: the world's `msecStep` off the count awake after each step
/// (`msec_step`, 0x5a6738) is 28 ms while all 36 fly and comes down to 17
/// as they sleep, and each still rests clear of the steps.
#[test]
fn parts_dropped_together_step_at_world_ones_msec_step() {
    let (Some(install), Some(common)) = (cod4_install(), zone("common_mp")) else {
        return;
    };
    let (m, walked) = Map::load(&install, "english", "mp_backlot").unwrap();
    let clip = ClipMap::from_assets(&walked.assets).unwrap();
    let parts = part_models(&[&walked.assets, &common]);
    let edges = steps(&m, &clip);
    let d = Dvars::default();
    let mut bodies = dropped_together(&parts, &edges);
    let mut clock = WorldClock::new(1);
    clock.start(0);
    let (mut trace, mut solids) = (clip_trace(&clip), clip_solids(&clip));
    let mut steps: Vec<(i32, usize)> = Vec::new();
    let mut t = 0;
    while t < 60_000 && bodies.iter().any(|(_, _, b)| !b.asleep) {
        t += 16;
        let stepped = clock.advance(t, &d, |ms, until, first, dv| {
            assert_eq!(
                dv.step_ms,
                steps.last().map_or(17, |&(_, n)| msec_step(1, n))
            );
            let mut awake = 0;
            for (_, _, b) in bodies.iter_mut() {
                if first {
                    b.begin_frame();
                }
                b.step_world(ms, until, &mut trace, &mut solids, dv);
                awake += usize::from(!b.asleep);
            }
            steps.push((ms, awake));
            awake
        });
        if stepped {
            for (_, _, b) in bodies.iter_mut() {
                b.end_frame(&mut trace, &d);
            }
        }
    }
    let longest = steps.iter().map(|s| s.0).max().unwrap();
    println!(
        "{} steps to {t} ms, the longest {longest} ms, the first {:?}",
        steps.len(),
        &steps[..4]
    );
    assert_eq!(steps[1], (28, 36));
    let mut problems = Vec::new();
    for (name, origin, b) in &bodies {
        let depth = body_depth(&clip, b);
        let z = Vec3::ZERO;
        let points_in = b
            .world_points()
            .filter(|&p| clip.trace(p, p, z, z, CLIP_MASK).startsolid)
            .count();
        let line = format!(
            "{name}: asleep {}, {} away, solid {depth} deep, {points_in} points in",
            b.asleep,
            b.origin().distance(*origin)
        );
        println!("{line}");
        if !b.asleep || depth > 0.0 || points_in > 0 {
            problems.push(line);
        }
    }
    assert!(problems.is_empty(), "{problems:#?}");
}

/// A round on a launched part names world 0 (0x48e043) while world 1 steps
/// the part: `Phys_ObjAddForce` (0x5a5900) puts its push on as a force at
/// world 0's `msecStep`, and world 1's next step spends it. With the 36
/// parts of [`parts_dropped_together_step_at_world_ones_msec_step`] in
/// flight world 1 steps 28 ms, so a round at world 0's 17 moves a part 28
/// / 17 times what the same round at world 1's own 28 does; as one whole
/// impulse each moved it the same.
#[test]
fn a_round_on_a_flying_part_is_a_force_at_world_zeros_msec_step() {
    let (Some(install), Some(common)) = (cod4_install(), zone("common_mp")) else {
        return;
    };
    let (m, walked) = Map::load(&install, "english", "mp_backlot").unwrap();
    let clip = ClipMap::from_assets(&walked.assets).unwrap();
    let parts = part_models(&[&walked.assets, &common]);
    let edges = steps(&m, &clip);
    let d = Dvars::default();
    let mut bodies = dropped_together(&parts, &edges);
    let mut clock = WorldClock::new(1);
    clock.start(0);
    let (mut trace, mut solids) = (clip_trace(&clip), clip_solids(&clip));
    let mut t = 0;
    while clock.step_ms != 28 {
        t += 16;
        clock.advance(t, &d, |ms, until, first, dv| {
            for (_, _, b) in bodies.iter_mut() {
                if first {
                    b.begin_frame();
                }
                b.step_world(ms, until, &mut trace, &mut solids, dv);
            }
            bodies.iter().filter(|(_, _, b)| !b.asleep).count()
        });
    }
    let (name, _, b) = &bodies[0];
    let scale = parts["vehicle_80s_sedan1_green_hood"]
        .preset
        .bullet_force_scale;
    // A round slanting down at the part's centre along +x, at
    // `dynEnt_bulletForce`'s 1000.
    let dir = Vec3::new(1.0, 0.0, -0.3).normalize();
    let mut moved = |msec: i32| {
        let (mut shot, mut still) = (b.clone(), b.clone());
        shot.launch(b.pos, dir * 1000.0, scale, msec, &d);
        let dv = Dvars {
            step_ms: clock.step_ms,
            ..d
        };
        for x in [&mut shot, &mut still] {
            x.step_world(28, t + 28, &mut trace, &mut solids, &dv);
        }
        shot.vel - still.vel
    };
    let (world0, world1) = (moved(17), moved(28));
    let ratio = world0.length() / world1.length();
    println!("{name}: {world0} at world 0, {world1} at world 1, {ratio}");
    assert!((ratio - 28.0 / 17.0).abs() < 1e-3, "{ratio}");
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
    let maps = vcod_cod4::testing::stock_maps(&install);
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

/// An axial slab `lo..hi` of solid.
struct Slab {
    lo: Vec3,
    hi: Vec3,
}

impl Slab {
    fn planes(&self) -> Vec<(Vec3, f32)> {
        (0..3)
            .flat_map(|k| [(Vec3::AXES[k], self.hi[k]), (-Vec3::AXES[k], -self.lo[k])])
            .collect()
    }

    /// A box of half size `ext` swept `a..b` against it as the clip map's
    /// kernels sweep one: the slab grown by `ext`, the skin kept off it.
    fn trace(&self, a: Vec3, b: Vec3, ext: f32) -> Sweep {
        let (lo, hi) = (self.lo - Vec3::splat(ext), self.hi + Vec3::splat(ext));
        let miss = Sweep {
            fraction: 1.0,
            normal: Vec3::ZERO,
            startsolid: false,
        };
        let (mut enter, mut leave, mut normal, mut out) = (-1.0f32, 1.0f32, Vec3::ZERO, false);
        for k in 0..3 {
            for (n, d) in [(Vec3::AXES[k], hi[k]), (-Vec3::AXES[k], -lo[k])] {
                let (d1, d2) = (n.dot(a) - d, n.dot(b) - d);
                out |= d1 > 0.0;
                if d1 > 0.0 && (d2 >= SKIN || d2 >= d1) {
                    return miss;
                }
                if d1 <= 0.0 && d2 <= 0.0 {
                    continue;
                }
                if d1 > d2 {
                    let f = ((d1 - SKIN) / (d1 - d2)).max(0.0);
                    if f > enter {
                        (enter, normal) = (f, n);
                    }
                } else {
                    leave = leave.min(((d1 + SKIN) / (d1 - d2)).min(1.0));
                }
            }
        }
        if !out {
            Sweep {
                fraction: 0.0,
                normal: Vec3::Z,
                startsolid: true,
            }
        } else if enter > -1.0 && enter < leave {
            Sweep {
                fraction: enter,
                normal,
                startsolid: false,
            }
        } else {
            miss
        }
    }

    /// How far `p` lies inside it, 0 outside.
    fn depth(&self, p: Vec3) -> f32 {
        (p - self.lo).min(self.hi - p).min_element().max(0.0)
    }
}

/// `SURFACE_CLIP_EPSILON`, the skin the clip map's kernels keep.
const SKIN: f32 = 0.125;

/// `clip` with `slabs` added, as the body's trace and its solids round a
/// box.
#[allow(clippy::type_complexity)]
fn with_slabs<'a>(
    clip: &'a ClipMap,
    slabs: &'a [Slab],
) -> (
    impl FnMut(Vec3, Vec3, f32) -> Sweep + 'a,
    impl FnMut(Vec3, Vec3) -> Vec<Solid> + 'a,
) {
    let mut t = clip_trace(clip);
    let mut s = clip_solids(clip);
    let trace = move |a: Vec3, b: Vec3, ext: f32| {
        let mut best = t(a, b, ext);
        for slab in slabs {
            let w = slab.trace(a, b, ext);
            if w.startsolid || (!best.startsolid && w.fraction < best.fraction) {
                best = w;
            }
        }
        best
    };
    let solids = move |lo: Vec3, hi: Vec3| {
        let mut out = s(lo, hi);
        for slab in slabs {
            if slab.lo.cmple(hi).all() && slab.hi.cmpge(lo).all() {
                out.extend(Solid::brush(&slab.planes()));
            }
        }
        out
    };
    (trace, solids)
}

/// The first `mp_tdm_spawn`'s floor with a level 64 by 64 by 96 room
/// over it.
fn open_floor(m: &Map, clip: &ClipMap) -> Vec3 {
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
    spawns
        .into_iter()
        .find_map(|s| {
            let (z, n) = floor(clip, s.x, s.y, s.z + 48.0)?;
            let level = [(-30.0, -30.0), (30.0, -30.0), (-30.0, 30.0), (30.0, 30.0)]
                .iter()
                .all(|&(dx, dy)| {
                    floor(clip, s.x + dx, s.y + dy, s.z + 48.0)
                        .is_some_and(|(fz, fnm)| fnm.z > 0.99 && (fz - z).abs() < 0.01)
                });
            let at = Vec3::new(s.x, s.y, z);
            let room = Vec3::new(32.0, 32.0, 48.0);
            let over = at + Vec3::Z * (room.z + 1.0);
            (n.z > 0.99 && level && !clip.trace(over, over, -room, room, CLIP_MASK).startsolid)
                .then_some(at)
        })
        .unwrap()
}

/// A wedged frame's end: asleep, how many of its steps were pinched, how
/// deep its points went into the nearer and the farther wall and the floor.
struct Wedged {
    line: String,
    asleep: bool,
    pinched: u32,
    near: f32,
    far: f32,
    floor: f32,
}

/// mp_crash's picture frames, each `split` into its geoms, wedged between
/// two facing walls: two slabs stood on the map's floor at its first open
/// `mp_tdm_spawn` a tenth closer than the frame is long, the frame laid
/// flat across them 40 over the floor, tipped about the walls' line until
/// it fits with a unit to spare and turning at 2, 4 or 8 rad/s to lie
/// flatter, so its ends meet both walls and it leans between them.
fn wedge_pictures(split: &dyn Fn(&Shape) -> Shape) -> Vec<Wedged> {
    let (Some(install), Some(common)) = (cod4_install(), zone("common_mp")) else {
        return Vec::new();
    };
    let (m, crash) = Map::load(&install, "english", "mp_crash").unwrap();
    let clip = ClipMap::from_assets(&crash.assets).unwrap();
    let defs = vcod_cod4::dynent::defs(&crash.assets, &[&common]);
    let d = Dvars::default();
    let spot = open_floor(&m, &clip);
    let mut out = Vec::new();
    for model in [4, 5, 6, 7, 8, 10].map(|n| format!("me_picture{n}")) {
        let def = defs.iter().find(|x| x.model == model).unwrap();
        let (shape, preset) = def.body.as_ref().unwrap();
        let bx = shape.boxes[0];
        let shape = split(shape);
        // Its longest side across the walls (x), its thinnest up.
        let dims = bx.half * 2.0;
        let mut idx = [0usize, 1, 2];
        idx.sort_by(|&p, &q| dims[q].total_cmp(&dims[p]));
        let (len, thin) = (dims[idx[0]], dims[idx[2]]);
        let mut cols = [Vec3::X, Vec3::Y, Vec3::Z];
        (cols[idx[0]], cols[idx[1]], cols[idx[2]]) = (Vec3::X, Vec3::Y, Vec3::Z);
        if Mat3::from_cols(cols[0], cols[1], cols[2]).determinant() < 0.0 {
            cols[idx[2]] = -cols[idx[2]];
        }
        let w = 0.9 * len;
        let mut tip = 0.0f32;
        while len * tip.cos() + thin * tip.sin() > w - 1.0 {
            tip += 0.005;
        }
        let rot = Quat::from_axis_angle(Vec3::Y, tip)
            * Quat::from_mat3(&Mat3::from_cols(cols[0], cols[1], cols[2]));
        let wall = |x0: f32, x1: f32| Slab {
            lo: spot + Vec3::new(x0, -24.0, -8.0),
            hi: spot + Vec3::new(x1, 24.0, 96.0),
        };
        let walls = [
            wall(-w * 0.5 - 16.0, -w * 0.5),
            wall(w * 0.5, w * 0.5 + 16.0),
        ];
        for spin in [2.0, 4.0, 8.0] {
            let origin = spot + Vec3::Z * 40.0 - rot * bx.centre;
            let mut b = Body::at(origin, rot, &shape, preset, 0);
            b.avel = -Vec3::Y * spin;
            let (mut trace, mut solids) = with_slabs(&clip, &walls);
            assert!(
                b.world_points().all(|p| !trace(p, p, 0.0).startsolid)
                    && body_depth(&clip, &b) == 0.0
            );
            let mut t = 0;
            while t < 60_000 && !b.asleep {
                t += 16;
                b.advance_in(t, &mut trace, &mut solids, &d);
            }
            let into = |s: &Slab| b.world_points().map(|p| s.depth(p)).fold(0.0f32, f32::max);
            let (near, far) = {
                let (a, c) = (into(&walls[0]), into(&walls[1]));
                (a.min(c), a.max(c))
            };
            let floor = b
                .world_points()
                .map(|p| spot.z - p.z)
                .fold(body_depth(&clip, &b), f32::max);
            let line = format!(
                "{model} spin {spin}: asleep {} after {t} ms, pinched {} of {} steps, walls {near} and {far} in, floor {floor}",
                b.asleep, b.pinched, b.steps
            );
            println!("{line}");
            out.push(Wedged {
                line,
                asleep: b.asleep,
                pinched: b.pinched,
                near,
                far,
                floor,
            });
        }
    }
    out
}

/// [`wedge_pictures`] on the frames as they are, one box geom (cod4-gsc.md
/// 3.6.1). Pinched, it keeps the first wall's contacts and loses the
/// other's and those its points leave (0x5a2760), so it comes to rest with
/// its end inside that far wall, but under a quarter unit, the floor and
/// the kept wall under a quarter too. Without the filter all 18 rest with
/// no point in either wall.
#[test]
fn pictures_wedged_between_two_walls_lose_one_walls_contacts() {
    let problems: Vec<String> = wedge_pictures(&|s| s.clone())
        .into_iter()
        .filter(|w| {
            !w.asleep
                || w.pinched == 0
                || w.far <= 0.0
                || w.far >= 0.25
                || w.near >= 0.25
                || w.floor >= 0.25
        })
        .map(|w| w.line)
        .collect();
    assert!(problems.is_empty(), "{problems:#?}");
}

/// The frame cut across its length into two box geoms, the halves'
/// points each the geom's, its mass as it was.
fn halves(s: &Shape) -> Shape {
    let b = s.boxes[0];
    let long = (0..3)
        .max_by(|&p, &q| b.half[p].total_cmp(&b.half[q]))
        .unwrap();
    let axis = b.axes.col(long) * b.half[long] * 0.5;
    let half = b.half - Vec3::AXES[long] * b.half[long] * 0.5;
    let mut out = Shape {
        points: Vec::new(),
        boxes: Vec::new(),
        cylinders: Vec::new(),
        point_geoms: Vec::new(),
        box_geoms: Vec::new(),
        cylinder_geoms: Vec::new(),
        ..s.clone()
    };
    for (g, c) in [b.centre - axis, b.centre + axis].into_iter().enumerate() {
        let part = Shape::of_bounds(c - b.axes * half, c + b.axes * half);
        out.point_geoms.extend(part.points.iter().map(|_| g));
        out.points.extend(part.points);
        out.boxes.push(ShapeBox {
            axes: b.axes,
            ..part.boxes[0]
        });
        out.box_geoms.push(g);
    }
    out
}

/// [`wedge_pictures`] on each frame as two box geoms, one at each wall:
/// 0x5a5f20 runs the contact pass per geom, and neither geom's contacts
/// face each other, so no step is pinched and each comes to rest with no
/// more than a quarter unit in either wall or the floor. Run as one body's
/// pass, as vcod had it, every frame was pinched and slipped into a wall.
#[test]
fn two_geom_pictures_wedged_between_two_walls_are_never_pinched() {
    let problems: Vec<String> = wedge_pictures(&halves)
        .into_iter()
        .filter(|w| !w.asleep || w.pinched > 0 || w.far >= 0.25 || w.floor >= 0.25)
        .map(|w| w.line)
        .collect();
    assert!(problems.is_empty(), "{problems:#?}");
}

/// mp_crash's plastic crate come to rest flat on its open floor, then
/// woken under one `physicsJitter` region over it whose least and most
/// push are the same, so each contact the pass keeps with a normal z of
/// 0.7 or more lifts it by the same `v1` (cod4-world.md 13.8): the step's
/// lift over `v1` counts them. The box meets the four floor brushes under
/// it in 16 contacts, each brush's top clipped to its face as 0x59a610
/// clips it; 0x5a2320 keeps three of them, all of the face's corners but
/// the one at its most x and least y, and vcod's points, nine on the
/// face, take none. The lift is read against the same step with no
/// region. Pushed on the points, as vcod had it, it read 8.9 pushes; on
/// all 16 contacts, uncut, 12.3.
#[test]
fn a_crate_flat_on_the_floor_takes_a_jitter_at_three_corners() {
    let (Some(install), Some(common)) = (cod4_install(), zone("common_mp")) else {
        return;
    };
    let (m, crash) = Map::load(&install, "english", "mp_crash").unwrap();
    let clip = ClipMap::from_assets(&crash.assets).unwrap();
    let defs = vcod_cod4::dynent::defs(&crash.assets, &[&common]);
    let d = Dvars::default();
    let spot = open_floor(&m, &clip);
    let def = defs
        .iter()
        .find(|x| x.model == "me_plastic_crate1")
        .unwrap();
    let (shape, preset) = def.body.as_ref().unwrap();
    let bx = shape.boxes[0];
    let origin = spot + Vec3::Z * (bx.half.z + 0.5) - bx.centre;
    let mut b = Body::at(origin, Quat::IDENTITY, shape, preset, 0);
    let (mut trace, mut solids) = (clip_trace(&clip), clip_solids(&clip));
    let mut t = 0;
    while t < 10_000 && !b.asleep {
        t += 17;
        b.advance_in(t, &mut trace, &mut solids, &d);
    }
    assert!(b.asleep, "{t}");
    // `(min + u (max - min)) f m' (1 - (m' / 200)^3) 1000 / msecStep`
    // over the mass, `f` 1 within the inner radius.
    let push = 4.0;
    let m = preset.mass.min(200.0);
    let v1 = push * m * (1.0 - (m / 200.0).powi(3)) * 1000.0 / d.step_ms as f32 / preset.mass;
    b.asleep = false;
    // The same step with no jitter, against which the lift is read.
    let mut still = b.clone();
    still.advance_in(t + 17, &mut trace, &mut solids, &d);
    b.jitter.push(vcod_cod4::physics::Jitter {
        origin: b.origin(),
        inner: 100.0,
        outer: 200.0,
        min: push,
        max: push,
    });
    b.advance_in(t + 17, &mut trace, &mut solids, &d);
    let lift = b.vel.z - still.vel.z;
    let count = lift / v1;
    println!("lift {lift} over {v1}: {count}");
    assert_eq!(count.round(), 3.0, "{count}");
}
