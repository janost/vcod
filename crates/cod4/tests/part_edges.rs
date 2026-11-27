//! Launched car parts against mp_backlot's steps and dynamic entities
//! against mp_crash's in worlds 1 and 0 on the ODE port (`vcod_cod4::ode`,
//! cod4-gsc.md 3.6.7): each dropped edge-first across a step's edge
//! sleeps within 60 s with its centre of mass out of the world and over
//! a floor, on the step or down the stairs it rolled off, sunk into what
//! it meets less than 1.5 units: `phys_csl`'s unit lets a resting geom
//! sink about that far (cod4-gsc.md 3.6.9 has the table). `COD4_DIR`
//! unset passes vacuously.

use glam::{Mat3, Quat, Vec3};
use vcod_cod4::assets::Assets;
use vcod_cod4::collision::ClipMap;
use vcod_cod4::map::Map;
use vcod_cod4::ode::collide::{self, JitterRegion};
use vcod_cod4::ode::math::M3;
use vcod_cod4::ode::obj::{PhysModel, Presets, part_models};
use vcod_cod4::ode::phys::{PhysWorld, WorldId, msec_step};
use vcod_cod4::ode::world::{BodyId, PhysDvars};
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

/// Each `mp_tdm_spawn`'s origin, sorted.
fn spawns(m: &Map) -> Vec<Vec3> {
    let mut out: Vec<Vec3> = m
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
    out.sort_by(|a, b| a.to_array().partial_cmp(&b.to_array()).unwrap());
    out
}

/// Steps near the spawns: two floors 8 units apart, both level, the
/// upper 6 to 18 above the lower, with a 72 by 72 by 56 box of room over
/// the edge. Each is the point over the edge at the upper floor's height
/// and the level direction up the step; one per 192 units.
fn steps(m: &Map, clip: &ClipMap) -> Vec<(Vec3, Vec3)> {
    let mut out: Vec<(Vec3, Vec3)> = Vec::new();
    for s in spawns(m) {
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

/// One drop: `m`'s body, its mass centre 24 units over `at`, turned by
/// `rot`.
fn make(pw: &mut PhysWorld, m: &PhysModel, at: Vec3, rot: Quat, dynent: bool) -> BodyId {
    let r = Mat3::from_quat(rot);
    let axes: M3 = [r.x_axis.into(), r.y_axis.into(), r.z_axis.into()];
    let id = pw
        .create_from(m, [0.0; 3], &axes, dynent, &PhysDvars::default())
        .unwrap();
    // The body sits at its mass centre: that 24 over the edge.
    let pos = (at + Vec3::Z * 24.0).into();
    pw.world.body_mut(id).pos = pos;
    pw.obj_mut(id).unwrap().prev.0 = pos;
    id
}

/// The six turns a drop takes over an edge facing `up`: across, along
/// and at 45 degrees, tipped 30 so an edge leads.
fn turns(up: Vec3) -> [(f32, Quat); 3] {
    let yaw = up.y.atan2(up.x).to_degrees();
    [(0.0, 30.0), (90.0, 30.0), (45.0, -30.0)].map(|(turn, roll)| {
        let a = Vec3::new(0.0, yaw + turn, roll);
        (turn, vcod_common::turretpose::angles_quat(a.to_array()))
    })
}

/// World `w` run in 16 ms frames to 60 s or until every body sleeps;
/// each step's length and the bodies awake after it.
fn settle(pw: &mut PhysWorld, clip: &ClipMap) -> (i32, Vec<(i32, i32)>) {
    let (dv, contact) = (PhysDvars::default(), collide::Dvars::default());
    let mut steps = Vec::new();
    let mut t = 0;
    pw.set_time(0);
    let awake = |pw: &PhysWorld| pw.world.bodies().iter().filter(|&&b| !pw.asleep(b)).count();
    while t < 60_000 && awake(pw) > 0 {
        t += 16;
        let mut last = pw.clock.now;
        pw.run_to_time_with(t, &dv, &contact, clip, &mut |_| {}, &mut |pw| {
            steps.push((pw.clock.now - last, awake(pw) as i32));
            last = pw.clock.now;
        });
    }
    (t, steps)
}

/// Whether point `p` is in the world the bodies meet.
fn in_world(clip: &ClipMap, p: Vec3) -> bool {
    clip.trace(p, p, Vec3::ZERO, Vec3::ZERO, CLIP_MASK)
        .startsolid
}

/// A geom in the world: a point of the body's frame to the world, and
/// the geom's inside in that frame (`None` outside, else how far in).
struct Solid {
    to_world: Box<dyn Fn(Vec3) -> Vec3>,
    lo: Vec3,
    hi: Vec3,
    inside: Box<dyn Fn(Vec3) -> Option<f32>>,
}

/// Body `id`'s geoms as [`Solid`]s.
fn solids(pw: &PhysWorld, id: BodyId) -> Vec<Solid> {
    use vcod_cod4::ode::geom::Geom;
    let b = pw.world.body(id);
    let r = Mat3::from_cols(
        Vec3::new(b.r[0][0], b.r[1][0], b.r[2][0]),
        Vec3::new(b.r[0][1], b.r[1][1], b.r[2][1]),
        Vec3::new(b.r[0][2], b.r[1][2], b.r[2][2]),
    );
    let pos = Vec3::from(b.pos);
    let of = |g: &Geom, r: Mat3, pos: Vec3| -> Option<Solid> {
        match g {
            Geom::Box { sides } => {
                let h = Vec3::from(*sides) * 0.5;
                Some(Solid {
                    to_world: Box::new(move |x| pos + r * x),
                    lo: -h,
                    hi: h,
                    inside: Box::new(move |x| {
                        let d = (h - x.abs()).min_element();
                        (d >= 0.0).then_some(d)
                    }),
                })
            }
            Geom::Brush { brush, offset } => {
                let (o, br) = (Vec3::from(*offset), brush.clone());
                let (lo, hi) = (Vec3::from(br.mins) - o, Vec3::from(br.maxs) - o);
                Some(Solid {
                    to_world: Box::new(move |x| pos + r * x),
                    lo,
                    hi,
                    inside: Box::new(move |x| {
                        let m = x + o;
                        let mut d = (Vec3::from(br.maxs) - m)
                            .min(m - Vec3::from(br.mins))
                            .min_element();
                        for p in &br.side_planes {
                            d = d.min(p[3] - Vec3::new(p[0], p[1], p[2]).dot(m));
                        }
                        (d >= 0.0).then_some(d)
                    }),
                })
            }
            &Geom::Cylinder {
                direction,
                radius,
                half_height,
            } => {
                let k = direction - 1;
                let mut h = Vec3::splat(radius);
                h[k] = half_height;
                Some(Solid {
                    to_world: Box::new(move |x| pos + r * x),
                    lo: -h,
                    hi: h,
                    inside: Box::new(move |x| {
                        let mut q = x;
                        q[k] = 0.0;
                        let d = (radius - q.length()).min(half_height - x[k].abs());
                        (d >= 0.0).then_some(d)
                    }),
                })
            }
            _ => None,
        }
    };
    let Some(o) = pw.obj(id) else {
        return Vec::new();
    };
    o.geoms
        .iter()
        .filter_map(|g| match g {
            Geom::Transform(t) => {
                let tr = Mat3::from_cols(
                    Vec3::new(t.r[0][0], t.r[1][0], t.r[2][0]),
                    Vec3::new(t.r[0][1], t.r[1][1], t.r[2][1]),
                    Vec3::new(t.r[0][2], t.r[1][2], t.r[2][2]),
                );
                of(&t.inner, r * tr, pos + r * Vec3::from(t.pos))
            }
            g => of(g, r, pos),
        })
        .collect()
}

/// How far the world reaches into body `id`: over a grid through each
/// geom's bounds (nine even planes an axis, and planes 0.1 to 4 in from
/// each side), the deepest point inside the geom (its distance to the
/// geom's surface) that is in the world, and the deepest any point
/// inside lies, the most the first can read.
fn sink(clip: &ClipMap, pw: &PhysWorld, id: BodyId) -> (f32, f32) {
    let (mut deepest, mut most) = (0.0f32, 0.0f32);
    for s in solids(pw, id) {
        let axis = |k: usize| {
            let (lo, hi) = (s.lo[k], s.hi[k]);
            let mut v: Vec<f32> = (0..9).map(|i| lo + (hi - lo) * i as f32 / 8.0).collect();
            for d in [0.1f32, 0.25, 0.5, 0.75, 1.0, 1.5, 2.0, 3.0, 4.0] {
                v.extend(
                    [lo + d, hi - d]
                        .into_iter()
                        .filter(|x| (lo..=hi).contains(x)),
                );
            }
            v
        };
        let (xs, ys, zs) = (axis(0), axis(1), axis(2));
        for &x in &xs {
            for &y in &ys {
                for &z in &zs {
                    let p = Vec3::new(x, y, z);
                    let Some(d) = (s.inside)(p) else { continue };
                    most = most.max(d);
                    if d > deepest && in_world(clip, (s.to_world)(p)) {
                        deepest = d;
                    }
                }
            }
        }
    }
    (deepest, most)
}

/// Body `id`'s end: a line, and whether it is a problem (awake, its
/// centre in the world, no floor within 64 under it: through the map, or
/// the world 1.5 units or more inside it).
fn judge(
    clip: &ClipMap,
    pw: &PhysWorld,
    id: BodyId,
    name: &str,
    at: Vec3,
    t: i32,
) -> Option<String> {
    let p = Vec3::from(pw.world.body(id).pos);
    let inside = in_world(clip, p);
    let z = Vec3::ZERO;
    let over = clip.trace(p, p - Vec3::Z * 64.0, z, z, CLIP_MASK).fraction < 1.0;
    let asleep = pw.asleep(id);
    let (deep, most) = sink(clip, pw, id);
    let line = format!(
        "{name} over {at}: asleep {asleep} by {t} ms, centre {p} ({} off), sunk {deep} of {most}{}{}",
        p.distance(at),
        if inside { ", in the world" } else { "" },
        if over { "" } else { ", over nothing" }
    );
    println!("{line}");
    (!asleep || inside || !over || deep >= 1.5).then_some(line)
}

/// `m` dropped alone in a world of `world` over the first six of `edges`
/// in each of [`turns`]: the problem lines.
fn drop_on_steps(
    clip: &ClipMap,
    edges: &[(Vec3, Vec3)],
    name: &str,
    m: &PhysModel,
    world: WorldId,
) -> Vec<String> {
    let mut problems = Vec::new();
    for &(at, up) in &edges[..6] {
        for (turn, rot) in turns(up) {
            let mut pw = PhysWorld::new(world);
            let id = make(&mut pw, m, at, rot, world == WorldId::Dynent);
            let p = Vec3::from(pw.world.body(id).pos);
            assert!(
                !in_world(clip, p),
                "{name} at {at} turn {turn}: starts in the world"
            );
            let (t, _) = settle(&mut pw, clip);
            let named = format!("{name} turn {turn}");
            problems.extend(judge(clip, &pw, id, &named, at, t));
        }
    }
    problems
}

/// mp_backlot's parts and the presets they launch with.
fn backlot() -> Option<(
    Map,
    ClipMap,
    std::collections::HashMap<String, std::sync::Arc<PhysModel>>,
)> {
    let (install, common) = (cod4_install()?, zone("common_mp")?);
    let (m, walked) = Map::load(&install, "english", "mp_backlot").unwrap();
    let clip = ClipMap::from_assets(&walked.assets).unwrap();
    let zones = [&walked.assets, &common];
    let parts = part_models(&zones, &Presets::of(&zones));
    Some((m, clip, parts))
}

const HOOD: &str = "vehicle_80s_sedan1_green_hood";
const DOOR: &str = "vehicle_80s_sedan1_green_door_lf";

/// mp_backlot's sedan hood (a brush geom) and left front door (its bounds
/// box) through [`drop_on_steps`] in world 1, as the client launches them.
#[test]
fn parts_dropped_edge_first_on_backlots_steps_come_to_rest() {
    let Some((m, clip, parts)) = backlot() else {
        return;
    };
    let edges = steps(&m, &clip);
    println!("{} steps", edges.len());
    assert!(edges.len() >= 6, "{edges:?}");
    let mut problems = Vec::new();
    for name in [HOOD, DOOR] {
        problems.extend(drop_on_steps(
            &clip,
            &edges,
            name,
            &parts[name],
            WorldId::Fx,
        ));
    }
    assert!(problems.is_empty(), "{problems:#?}");
}

/// The 36 drops of [`parts_dropped_edge_first_on_backlots_steps_come_to_rest`]
/// in one world 1, made at time 0, with their names and edges.
fn dropped_together(
    pw: &mut PhysWorld,
    parts: &std::collections::HashMap<String, std::sync::Arc<PhysModel>>,
    edges: &[(Vec3, Vec3)],
) -> Vec<(String, Vec3, BodyId)> {
    let mut bodies = Vec::new();
    for name in [HOOD, DOOR] {
        for &(at, up) in &edges[..6] {
            for (turn, rot) in turns(up) {
                let id = make(pw, &parts[name], at, rot, false);
                bodies.push((format!("{name} turn {turn}"), at, id));
            }
        }
    }
    assert_eq!(bodies.len(), 36);
    bodies
}

/// The 36 drops in one world 1 at 16 ms frames: the world's `msecStep`
/// off the count awake after each step (0x5a6738) is 28 ms while all 36
/// fly and comes down to 17 as they sleep, and each comes to rest as it
/// did alone.
#[test]
fn parts_dropped_together_step_at_world_ones_msec_step() {
    let Some((m, clip, parts)) = backlot() else {
        return;
    };
    let edges = steps(&m, &clip);
    let mut pw = PhysWorld::new(WorldId::Fx);
    let bodies = dropped_together(&mut pw, &parts, &edges);
    let (t, steps) = settle(&mut pw, &clip);
    let longest = steps.iter().map(|s| s.0).max().unwrap();
    println!(
        "{} steps to {t} ms, the longest {longest} ms, the first {:?}",
        steps.len(),
        &steps[..4]
    );
    assert_eq!(steps[0], (17, 36));
    assert_eq!(steps[1].0, msec_step(WorldId::Fx, 36));
    assert_eq!(steps[1].0, 28);
    assert_eq!(steps.last().unwrap().1, 0);
    let problems: Vec<String> = bodies
        .iter()
        .filter_map(|(name, at, id)| judge(&clip, &pw, *id, name, *at, t))
        .collect();
    assert!(problems.is_empty(), "{problems:#?}");
}

/// A round on a launched part names world 0 (0x48e043) while world 1 steps
/// the part: `Phys_ObjBulletImpact` (0x5a5a20) puts its push on as a force
/// at world 0's `msecStep`. With the 36 parts in flight world 1 steps 28
/// ms, so a round at world 0's 17 moves a part 28 / 17 times what the
/// same round at world 1's own 28 does.
#[test]
fn a_round_on_a_flying_part_is_a_force_at_world_zeros_msec_step() {
    let Some((m, clip, parts)) = backlot() else {
        return;
    };
    let edges = steps(&m, &clip);
    let mut pw = PhysWorld::new(WorldId::Fx);
    let bodies = dropped_together(&mut pw, &parts, &edges);
    let (dv, contact) = (PhysDvars::default(), collide::Dvars::default());
    pw.set_time(0);
    let mut t = 0;
    while pw.clock.msec_step != 28 {
        t += 16;
        pw.run_to_time(t, &dv, &contact, &clip, &mut |_| {});
    }
    let (name, _, id) = &bodies[0];
    let scale = parts[HOOD].preset.bullet_force_scale;
    // A round slanting down at the part's centre along +x, at
    // `dynEnt_bulletForce`'s 1000.
    let dir = Vec3::new(1.0, 0.0, -0.3).normalize().to_array();
    let lvel = |pw: &PhysWorld| Vec3::from(pw.world.body(*id).lvel);
    let moved = |msec: i32| {
        let (mut shot, mut still) = (pw.clone(), pw.clone());
        let b = shot.world.body_mut(*id);
        let at = b.pos;
        vcod_cod4::ode::phys::obj_bullet_impact(b, msec, at, dir, 1000.0, scale, &dv);
        for x in [&mut shot, &mut still] {
            x.run_to_time(t + 28, &dv, &contact, &clip, &mut |_| {});
        }
        lvel(&shot) - lvel(&still)
    };
    let (world0, world1) = (moved(17), moved(28));
    let ratio = world0.length() / world1.length();
    println!("{name}: {world0} at world 0, {world1} at world 1, {ratio}");
    assert!((ratio - 28.0 / 17.0).abs() < 1e-3, "{ratio}");
}

/// One def's model of each name in `defs` that `keep` takes.
fn models_of(
    defs: &[vcod_cod4::dynent::DynDef],
    keep: impl Fn(&PhysModel) -> bool,
) -> std::collections::BTreeMap<String, std::sync::Arc<PhysModel>> {
    let mut seen = std::collections::BTreeMap::new();
    for d in defs {
        if let Some(m) = d.body.as_ref().filter(|m| keep(m)) {
            seen.entry(d.model.clone()).or_insert_with(|| m.clone());
        }
    }
    seen
}

/// Whether a dynent model's body is a cylinder: a `physGeoms` geom of
/// another type than the box, or the preset's cylinder byte.
fn cylinder(m: &PhysModel) -> bool {
    m.preset.obj.cylinder
        || m.model
            .phys_geoms
            .as_ref()
            .is_some_and(|g| g.geoms.iter().any(|g| g.brush.is_none() && g.kind != 1))
}

/// mp_crash's dynamic entities, one def of each model with no cylinder,
/// through [`drop_on_steps`] in world 0 on mp_crash's steps (cod4-world.md
/// 13).
#[test]
fn dynents_dropped_edge_first_on_crashs_steps_come_to_rest() {
    let (Some(install), Some(common)) = (cod4_install(), zone("common_mp")) else {
        return;
    };
    let (m, walked) = Map::load(&install, "english", "mp_crash").unwrap();
    let clip = ClipMap::from_assets(&walked.assets).unwrap();
    let defs = vcod_cod4::dynent::defs(&walked.assets, &[&common]);
    let edges = steps(&m, &clip);
    assert!(edges.len() >= 6, "{edges:?}");
    let seen = models_of(&defs, |m| !cylinder(m));
    println!("{} models", seen.len());
    let mut problems = Vec::new();
    for (name, m) in &seen {
        problems.extend(drop_on_steps(&clip, &edges, name, m, WorldId::Dynent));
    }
    assert!(problems.is_empty(), "{problems:#?}");
}

/// The stock dynamic entities with a cylinder (barrels, cans, bottles,
/// pots, tires, hub caps, fruit), one def of each model from the first
/// `mp_` zone placing it, through [`drop_on_steps`] on mp_crash's steps.
#[test]
fn cylinder_props_dropped_edge_first_come_to_rest() {
    let (Some(install), Some(common)) = (cod4_install(), zone("common_mp")) else {
        return;
    };
    let mut seen = std::collections::BTreeMap::new();
    for map in &vcod_cod4::testing::stock_maps(&install) {
        let Some(walked) = zone(map) else { continue };
        let defs = vcod_cod4::dynent::defs(&walked, &[&common]);
        for (name, m) in models_of(&defs, cylinder) {
            seen.entry(name).or_insert(m);
        }
    }
    assert!(!seen.is_empty());
    let (m, walked) = Map::load(&install, "english", "mp_crash").unwrap();
    let clip = ClipMap::from_assets(&walked.assets).unwrap();
    let edges = steps(&m, &clip);
    assert!(edges.len() >= 6, "{edges:?}");
    let mut problems = Vec::new();
    for (name, m) in &seen {
        problems.extend(drop_on_steps(&clip, &edges, name, m, WorldId::Dynent));
    }
    println!("{} models, {} problems", seen.len(), problems.len());
    assert!(problems.is_empty(), "{problems:#?}");
}

/// The first `mp_tdm_spawn`'s floor with a level 64 by 64 by 96 room
/// over it.
fn open_floor(m: &Map, clip: &ClipMap) -> Vec3 {
    spawns(m)
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

/// mp_crash's plastic crate come to rest flat on its open floor in world
/// 0, then woken under one `physicsJitter` region over it whose least and
/// most push are the same, so each contact the pass keeps with a normal z
/// of 0.7 or more lifts it by the same `v1` (cod4-world.md 13.8): the
/// step's lift over `v1`, against the same step with no region, counts
/// them. On the box collider (0x59a610) and 0x5a2320's cut it reads 4.
#[test]
fn a_crate_flat_on_the_floor_takes_a_jitter_at_its_kept_contacts() {
    let (Some(install), Some(common)) = (cod4_install(), zone("common_mp")) else {
        return;
    };
    let (m, crash) = Map::load(&install, "english", "mp_crash").unwrap();
    let clip = ClipMap::from_assets(&crash.assets).unwrap();
    let defs = vcod_cod4::dynent::defs(&crash.assets, &[&common]);
    let spot = open_floor(&m, &clip);
    let model = defs
        .iter()
        .find(|x| x.model == "me_plastic_crate1")
        .and_then(|d| d.body.clone())
        .unwrap();
    let mut pw = PhysWorld::new(WorldId::Dynent);
    let id = make(&mut pw, &model, spot, Quat::IDENTITY, true);
    let half = (model.model.maxs[2] - model.model.mins[2]) * 0.5;
    pw.world.body_mut(id).pos = (spot + Vec3::Z * (half + 0.5)).into();
    let (t, _) = settle(&mut pw, &clip);
    assert!(pw.asleep(id), "{t}");
    // `(min + u (max - min)) f m' (1 - (m' / 200)^3) 1000 / msecStep`
    // over the mass, `f` 1 within the inner radius, spent over 17 ms.
    let push = 4.0;
    let mass = model.preset.obj.mass;
    let m = mass.min(200.0);
    let v1 = push * m * (1.0 - (m / 200.0).powi(3)) * 1000.0 / 17.0 / mass;
    let (dv, contact) = (PhysDvars::default(), collide::Dvars::default());
    pw.world.body_mut(id).enable();
    let mut still = pw.clone();
    still.run_to_time(t + 17, &dv, &contact, &clip, &mut |_| {});
    let p = pw.world.body(id).pos;
    pw.add_jitter(JitterRegion::new(p, 100.0, 200.0, push, push));
    pw.run_to_time(t + 17, &dv, &contact, &clip, &mut |_| {});
    let lift = pw.world.body(id).lvel[2] - still.world.body(id).lvel[2];
    let count = lift / v1;
    println!("lift {lift} over {v1}: {count}");
    assert_eq!(count.round(), 4.0, "{count}");
}
