use glam::Vec3;

use super::selfcol::{Bone, closest, self_pair};
use super::world::{Pieces, brush, tris};
use super::*;
use crate::collision::{BrushFaces, Piece};
use crate::ode::body::DISABLED;
use crate::ode::joint::Kind;
use crate::ode::mass::Mass;
use crate::ode::math::IDENTITY;
use crate::ode::world::Params;

/// An axial box brush as the zones carry one: each face's neighbours in
/// the order whose winding (0x5998e0) runs clockwise seen from outside,
/// as every winding of mp_crash's does (`capsule_world`).
pub(crate) fn cuboid(mins: [f32; 3], maxs: [f32; 3]) -> BrushFaces {
    let planes = BrushFaces::axial_planes(mins, maxs).to_vec();
    let n = |i: usize| Vec3::from(planes[i].0);
    let adjacent = (0..6)
        .map(|i| {
            let others: Vec<usize> = (0..6).filter(|&j| j >> 1 != i >> 1).collect();
            let mut ring = vec![others[0]];
            while ring.len() < 4 {
                let cur = *ring.last().unwrap();
                let next = others
                    .iter()
                    .copied()
                    .find(|&j| j >> 1 != cur >> 1 && n(cur).cross(n(j)).dot(n(i)) < 0.0)
                    .unwrap();
                ring.push(next);
            }
            ring.iter().map(|&j| j as u8).collect()
        })
        .collect();
    BrushFaces {
        planes,
        adjacent,
        surface: vec![0; 6],
    }
}

/// A world of brushes alone, every one in reach.
pub(crate) struct Brushes(pub(crate) Vec<BrushFaces>);

impl Pieces for Brushes {
    fn pieces(&self, _: Vec3, _: Vec3, _: Vec3, _: u32) -> Vec<Piece> {
        (0..self.0.len()).map(Piece::Brush).collect()
    }
    fn brush(&self, b: usize) -> BrushFaces {
        self.0[b].clone()
    }
}

fn flat(cap: &Capsule, f: &BrushFaces) -> Vec<ContactGeom> {
    let mut out = Vec::new();
    brush(
        &mut out,
        MAX_CONTACTS,
        &capsule::Seg::new(cap.centre, cap.axis, cap.radius, cap.half_len),
        f,
    );
    out.iter_mut().for_each(|c| c.normal = c.normal.map(|x| -x));
    out
}

#[test]
fn a_box_brushs_windings_run_clockwise_from_outside() {
    let f = cuboid([-64.0, -32.0, -16.0], [64.0, 32.0, 0.0]);
    for i in 0..6 {
        let v: Vec<Vec3> = f.winding(i, 0x100).into_iter().map(Vec3::from).collect();
        assert_eq!(v.len(), 4, "face {i}");
        let area = (v[1] - v[0]).cross(v[2] - v[0]);
        assert!(area.dot(Vec3::from(f.planes[i].0)) < 0.0, "face {i}");
    }
}

/// A level capsule half a unit into a box's top face: the face is the
/// reference, its two fan triangles give the segment's ends twice, the
/// local filter keeps one of each, and each is 0.5 deep along +z (after
/// 0x5a95a0 turns the normals out of the world).
#[test]
fn a_capsule_resting_on_a_flat_brush() {
    let f = cuboid([-64.0, -64.0, -16.0], [64.0, 64.0, 0.0]);
    let cap = Capsule {
        centre: [3.0, 2.0, 4.5],
        axis: [1.0, 0.0, 0.0],
        radius: 5.0,
        half_len: 10.0,
    };
    let c = flat(&cap, &f);
    assert_eq!(c.len(), 2, "{c:?}");
    for (g, x) in c.iter().zip([-7.0, 13.0]) {
        assert_eq!(g.normal, [0.0, 0.0, 1.0]);
        assert_eq!(g.depth, 0.5);
        assert_eq!(g.pos, [x, 2.0, -0.5]);
    }
    // Clear of the face by a unit: no contact.
    let up = Capsule {
        centre: [3.0, 2.0, 6.0],
        ..cap
    };
    assert!(flat(&up, &f).is_empty());
}

/// A capsule lying across the box's +x edge: the top face is still the
/// deepest with the centre in front of it, and the segment is clipped at
/// the face's rim, so the outer contact sits on the edge.
#[test]
fn a_capsule_crossing_a_brush_edge() {
    let f = cuboid([-64.0, -64.0, -16.0], [64.0, 64.0, 0.0]);
    let cap = Capsule {
        centre: [60.0, 0.0, 4.0],
        axis: [1.0, 0.0, 0.0],
        radius: 5.0,
        half_len: 10.0,
    };
    let c = flat(&cap, &f);
    assert_eq!(c.len(), 2, "{c:?}");
    let xs: Vec<f32> = c.iter().map(|g| g.pos[0]).collect();
    assert!(xs.contains(&50.0) && xs.contains(&64.0), "{xs:?}");
    assert!(
        c.iter()
            .all(|g| g.normal == [0.0, 0.0, 1.0] && g.depth == 1.0)
    );
}

/// Two triangles of a 30 degree slope rising along +x, a level capsule
/// lying on it across the slope 0.25 deep: along the slope's normal, its
/// segment pushed down by the radius and cut where it crosses the shared
/// edge, so each triangle gives a half and the two contacts on the edge
/// are one: three.
#[test]
fn a_capsule_on_a_slopes_triangles() {
    let (s, c30) = (0.5f32, 3f32.sqrt() / 2.0);
    let up = |x: f32, y: f32| Vec3::new(x * c30, y, x * s);
    let (a, b, cc, d) = (
        up(-50.0, -50.0),
        up(50.0, -50.0),
        up(50.0, 50.0),
        up(-50.0, 50.0),
    );
    // Fronts `(t2 - t0) x (t1 - t0)` up the slope's normal.
    let t = [[a, cc, b], [a, d, cc]];
    let n = Vec3::new(-s, 0.0, c30);
    let r = 4.0;
    let centre = n * (r - 0.25);
    let cap = Capsule {
        centre: centre.to_array(),
        axis: [0.0, 1.0, 0.0],
        radius: r,
        half_len: 8.0,
    };
    let mut out = Vec::new();
    tris(
        &mut out,
        MAX_CONTACTS,
        &capsule::Seg::new(cap.centre, cap.axis, r, 8.0),
        &t,
        7,
    );
    assert_eq!(out.len(), 3, "{out:?}");
    let ys: Vec<f32> = out.iter().map(|g| g.pos[1]).collect();
    assert_eq!(ys, [-8.0, 0.0, 8.0]);
    for g in &out {
        let gn = -Vec3::from(g.normal);
        assert!(gn.abs_diff_eq(n, 1e-6), "{gn}");
        assert!((g.depth - 0.25).abs() < 1e-4, "{}", g.depth);
        assert_eq!(g.surface, 7);
        let p = Vec3::from(g.pos);
        assert!((p.dot(n) + 0.25).abs() < 1e-4, "{p}");
    }
}

/// 0x5a2320 on 18 contacts in three clusters (floor, a -x wall, a +y
/// wall), each a diamond of four with two inside, the middle one deepest:
/// out come each cluster's least and most along its two tangents and its
/// deepest, clusters in seed order (contact 104729 mod 18 = 5's first),
/// 15 in all.
#[test]
fn the_cut_keeps_retails_fifteen() {
    let diamond = [
        (0.0, 10.0),
        (10.0, 0.0),
        (20.0, 10.0),
        (10.0, 20.0),
        (10.0, 10.0),
        (10.0, 5.0),
    ];
    let mut c = Vec::new();
    type At = fn(f32, f32) -> [f32; 3];
    let walls: [(At, [f32; 3]); 3] = [
        (|x, y| [x, y, 0.0], [0.0, 0.0, 1.0]),
        (|y, z| [40.0, y, z], [-1.0, 0.0, 0.0]),
        (|x, z| [x, -40.0, z], [0.0, 1.0, 0.0]),
    ];
    for (at, normal) in walls {
        for (i, &(p, q)) in diamond.iter().enumerate() {
            c.push(ContactGeom {
                pos: at(p, q),
                normal,
                depth: if i == 4 { 2.0 } else { 1.0 },
                surface: 0,
            });
        }
    }
    let got = cut(&c);
    let idx: Vec<usize> = got
        .iter()
        .map(|g| c.iter().position(|h| h == g).unwrap())
        .collect();
    // Floor: T1 = z x y = -x, T2 = z x T1 = -y. The -x wall: T1 = -x x z
    // = y, T2 = -z. The +y wall: T1 = y x z = x, T2 = -z.
    assert_eq!(idx, [2, 3, 0, 1, 4, 6, 9, 8, 7, 10, 12, 15, 14, 13, 16]);
}

/// Two bones side by side exactly their scaled radii apart touch at depth
/// 0 (0x5bdb99 takes equal); a hair farther they do not. The contact
/// pushes bone `b`'s body (`b1`) away from `a`, `mu` `a`'s friction.
#[test]
fn two_bones_self_collide_at_the_threshold() {
    let dv = Dvars::default();
    // 3.6 + 2.4 reads 6.0000002 in the double sum, so 6 is just inside.
    for (dy, contacts) in [(6.0, 1), (6.01, 0)] {
        let mut world = World::new();
        world.params = Params::ragdoll();
        let ba = world.create_body();
        let bb = world.create_body();
        let a = Bone {
            body: ba,
            seg: [[0.0, 0.0, 0.0], [10.0, 0.0, 0.0]],
            radius: 3.0,
            friction: 0.4,
        };
        let b = Bone {
            body: bb,
            seg: [[2.0, dy, 0.0], [12.0, dy, 0.0]],
            radius: 2.0,
            friction: 0.9,
        };
        let (mut oa, mut ob) = (Obj::default(), Obj::default());
        self_pair(&mut world, &a, &mut oa, &b, &mut ob, &dv);
        assert_eq!(world.contact_count(), contacts, "dy {dy}");
        if contacts == 1 {
            let j = world.joint(world.contacts()[0]);
            assert_eq!(j.body, [Some(bb), Some(ba)]);
            let Kind::Contact(c) = &j.kind else { panic!() };
            assert_eq!(c.normal, [0.0, 1.0, 0.0]);
            assert!(c.depth.abs() < 1e-6, "{}", c.depth);
            assert!((c.pos[1] - 3.6).abs() < 1e-6, "{:?}", c.pos);
            assert_eq!(c.surface.mu, 0.4);
            assert_eq!((c.surface.soft_erp, c.surface.soft_cfm), (0.8, 1e-5));
        }
    }
    // Crossed segments meet at their middles.
    assert_eq!(
        closest(
            [[-5.0, 0.0, 0.0], [5.0, 0.0, 0.0]],
            [[0.0, -5.0, 1.0], [0.0, 5.0, 1.0]]
        ),
        (0.5, 0.5)
    );
}

/// A ragdoll capsule body (`ragdoll_bone` 5 radius, 20 long, mass 3)
/// let go level 20 units over a floor brush in the ragdoll world at 17 ms
/// steps: it lands, comes to rest a little over a unit under its radius
/// over the floor (`phys_csl`'s 1 unit surface layer and the soft CFM let
/// it sink: 3.93), and
/// sleeps on the 53rd step whose sleep test (run first in the step) found
/// it under the sleep speeds.
#[test]
fn a_capsule_dropped_on_a_floor_rests_and_sleeps() {
    let floor = Brushes(vec![cuboid([-256.0, -256.0, -32.0], [256.0, 256.0, 0.0])]);
    let mut world = World::new();
    world.params = Params::ragdoll();
    let b = world.create_body();
    let (r, h) = (5.0f32, 5.0f32);
    let mut m = Mass::capped_cylinder(1.0, 1, r, h * 2.0);
    m.adjust(3.0);
    world.body_mut(b).pos = [0.0, 0.0, 25.0];
    world.body_mut(b).set_mass(m);
    world.body_mut(b).set_finite_rotation(true);
    world.body_mut(b).set_rotation(&IDENTITY);
    let mut obj = Obj {
        friction: 0.3,
        ..Obj::default()
    };
    let dv = Dvars::default();
    let adis = world.params.adis;
    let mut asleep = None;
    let mut last_moving = 0;
    for step in 1..=400 {
        let body = world.body(b);
        let cap = Capsule {
            centre: body.pos,
            axis: [body.r[0][0], body.r[1][0], body.r[2][0]],
            radius: r,
            half_len: h,
        };
        near_world(&mut world, b, &mut obj, &cap, &floor, &dv, RAGDOLL_WORLD);
        world.quick_step(0.017);
        world.zero_forces();
        world.empty_contacts();
        let body = world.body_mut(b);
        body.avel = body.avel.map(|x| (w(x) + w(x) * -0.01) as f32);
        let speed = |v: V3| dot_yx(v, v);
        if speed(body.lvel) > adis.linear_threshold || speed(body.avel) > adis.angular_threshold {
            last_moving = step;
        }
        if body.flags & DISABLED != 0 {
            asleep = Some(step);
            break;
        }
    }
    let at = asleep.expect("never slept");
    assert_eq!(
        at - (last_moving + 1),
        53,
        "moved last at {last_moving}, asleep at {at}"
    );
    let z = world.body(b).pos[2];
    assert!((3.9..4.0).contains(&z), "rests at {z}");
}
