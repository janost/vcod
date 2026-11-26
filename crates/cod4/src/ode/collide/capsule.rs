//! Contains routines ported from the Open Dynamics Engine 0.5, Copyright (C) 2001-2004
//! Russell L. Smith, under its BSD license (LICENSE-ODE-BSD.txt beside this file). See NOTICE.
//!
//! ODE 0.5's triangle against capsule collider (`collision_trimesh_ccylinder.cpp`,
//! Alen Ladavac's) as IW rebuilt it in iw3mp.exe 1.7: the separating axes
//! (0x59ca70, 0x59ccd0) in three sets, the triangle's own (0x59cd70) and two
//! for the triangles of a fanned brush face (0x59d4a0, 0x59d9c0), the clip
//! (0x59c910) and the local contacts (0x59c5a0, 0x59c600, 0x59c710);
//! cod4-player-anims.md 10.6.

use super::ContactGeom;
use crate::ode::math::{V3, add, cross, dot, neg, sub};

#[inline]
fn w(x: f32) -> f64 {
    x as f64
}

/// The capsule as the colliders read it (0x59e870's block): its segment's
/// ends `c + h a` (+0) and `c - h a` (+0xc), the centre (+0x18), `-a`
/// (+0x24), the radius (+0x30), its square (+0x34), `h + r` (+0x38) and
/// the half length `h` (+0x3c).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Seg {
    pub p0: V3,
    pub p1: V3,
    pub c: V3,
    pub dir: V3,
    pub r: f32,
    pub r2: f32,
    pub hr: f32,
    pub h: f32,
}

impl Seg {
    /// 0x59e870 off a capsule centred on `c` along `axis`.
    pub fn new(c: V3, axis: V3, r: f32, h: f32) -> Seg {
        let dir = neg(axis);
        let end = |s: f32| std::array::from_fn(|k| (w(dir[k]) * w(s) + w(c[k])) as f32);
        Seg {
            p0: end(-h),
            p1: end(h),
            c,
            dir,
            r,
            r2: (w(r) * w(r)) as f32,
            hr: (w(h) + w(r)) as f32,
            h,
        }
    }
}

/// The best axis so far (`fBestDepth` and kin): its depth (+0), the
/// triangle's centre along it (+4), its id (+8), the triangle's half
/// extent (+0xc) and the normal (+0x10).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Best {
    pub depth: f32,
    pub center: f32,
    pub id: i32,
    pub tr: f32,
    pub normal: V3,
}

impl Best {
    fn new() -> Best {
        // The float at 0x70af5c.
        Best {
            depth: -f32::MAX,
            center: 0.0,
            id: 0,
            tr: 0.0,
            normal: [0.0; 3],
        }
    }
}

/// A local contact (0x24 bytes at 0xcc53f80): position, normal, depth,
/// the surface flags IW added and the valid flag.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Local {
    pub pos: V3,
    pub normal: V3,
    pub depth: f32,
    pub surface: u32,
    pub valid: bool,
}

/// The local contacts' room (`cmp ecx,0x20` at 0x59c5a6).
pub const MAX_LOCAL: usize = 0x20;

/// 0x59c5a0: a local contact, dropped when the 32 are taken.
fn add_local(locals: &mut Vec<Local>, pos: V3, normal: V3, depth: f32, surface: u32) {
    if locals.len() < MAX_LOCAL {
        locals.push(Local {
            pos,
            normal,
            depth,
            surface,
            valid: true,
        });
    }
}

/// `_ProcessLocalContacts` (0x59c710): with two or more, of each pair whose
/// positions and normals are within 0.001 on every axis (the squared
/// difference against the double at 0x70b300, 0x59c600) the shallower
/// goes, the first on a tie; the rest go out while `out` has room.
// A NaN difference reads near, as the compare's flags do.
#[allow(clippy::neg_cmp_op_on_partial_ord)]
pub fn process(locals: &[Local], out: &mut Vec<ContactGeom>, max: usize) {
    const EPS: f64 = 1.000_000_111_162_080_4e-6;
    let mut l = locals.to_vec();
    if l.len() > 1 {
        let near = |a: V3, b: V3| (0..3).all(|k| !((w(a[k]) - w(b[k])).powi(2) > EPS));
        for i in 0..l.len() - 1 {
            for j in i + 1..l.len() {
                if near(l[i].pos, l[j].pos) && near(l[i].normal, l[j].normal) {
                    if l[j].depth < l[i].depth {
                        l[j].valid = false;
                    } else {
                        l[i].valid = false;
                    }
                }
            }
        }
    }
    for c in l.iter().filter(|c| c.valid) {
        if out.len() < max {
            out.push(ContactGeom {
                pos: c.pos,
                normal: c.normal,
                depth: c.depth,
                surface: c.surface,
            });
        }
    }
}

/// `_cldTestAxis` (0x59ca70) against the triangle's corners `v`, each off
/// the capsule's centre. An axis under 1e-6 long (the double at 0x70b508)
/// passes; one the two do not overlap on separates them. A deeper overlap
/// than the best (the least negative) takes its place, turned to point at
/// the triangle unless `noflip`.
fn test_axis(best: &mut Best, s: &Seg, v: &[V3; 3], axis: V3, id: i32, noflip: bool) -> bool {
    let len2 =
        ((w(axis[1]) * w(axis[1]) + w(axis[0]) * w(axis[0])) + w(axis[2]) * w(axis[2])) as f32;
    let len = w(len2).sqrt() as f32;
    if w(len) < 1e-6 {
        return true;
    }
    let inv = (1.0 / w(len)) as f32;
    let n = axis.map(|a| (w(a) * w(inv)) as f32);
    let frc = (w(dot(s.dir, n).abs()) * w(s.h) + w(s.r)) as f32;
    let p = v.map(|e| dot(e, n));
    let (mut lo, mut hi) = (p[0], p[0]);
    if p[1] < p[0] {
        lo = p[1];
    }
    if p[1] > p[0] {
        hi = p[1];
    }
    if lo > p[2] {
        lo = p[2];
    }
    if hi < p[2] {
        hi = p[2];
    }
    let center = ((w(hi) + w(lo)) * 0.5) as f32;
    let tr = ((w(hi) - w(lo)) * 0.5) as f32;
    let sum = (w(tr) + w(frc)) as f32;
    if sum < center.abs() {
        return false;
    }
    let depth = (w(center.abs()) - w(sum)) as f32;
    if best.depth < depth {
        *best = Best {
            depth,
            center,
            id,
            tr,
            normal: n,
        };
        if center < 0.0 && !noflip {
            best.normal = neg(n);
            best.center = -center;
        }
    }
    true
}

/// `_CalculateAxis` (0x59ccd0): `((p - q) x a) x a`, each step stored.
fn perp(a: V3, p: V3, q: V3) -> V3 {
    cross(cross(sub(p, q), a), a)
}

/// Which separating axes a triangle takes and which of its edges clip:
/// its own (0x59cd70, 0x59dd90), a fanned face's end (0x59d4a0, 0x59e130,
/// its edge `v0 v1` inside the face) or middle (0x59d9c0, 0x59e4a0, only
/// `v1 v2` on the face's rim).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tri {
    All,
    End,
    Mid,
}

/// The separating axis test, the capsule's ends `q0 = c - h a` and
/// `q1 = c + h a`. IW's order and ids; ODE 0.5's 1e-6 squared length gate
/// before each axis is gone.
fn sat(kind: Tri, best: &mut Best, s: &Seg, n: V3, v: [V3; 3]) -> bool {
    *best = Best::new();
    let (q0, q1) = (s.p1, s.p0);
    let [v0, v1, v2] = v;
    let rel = [sub(v0, s.c), sub(v1, s.c), sub(v2, s.c)];
    let e = [sub(v1, v0), sub(v2, v1), sub(v0, v2)];
    let dir = s.dir;
    let mut axes: Vec<(V3, i32)> = Vec::with_capacity(19);
    match kind {
        Tri::All => {
            axes.push((perp(dir, v0, q0), 11));
            for (k, &ek) in e.iter().enumerate() {
                axes.push((cross(dir, ek), 2 + k as i32));
            }
            for (k, &vk) in v.iter().enumerate() {
                axes.push((perp(e[k], q0, vk), 5 + k as i32));
            }
            for k in (0..3).rev() {
                axes.push((perp(e[k], q1, v[k]), 8 + k as i32));
            }
            axes.push((perp(dir, v1, q0), 12));
            axes.push((perp(dir, v2, q0), 13));
            for (k, &vk) in v.iter().enumerate() {
                axes.push((sub(vk, q0), 14 + k as i32));
            }
            for (k, &vk) in v.iter().enumerate() {
                axes.push((sub(vk, q1), 17 + k as i32));
            }
        }
        Tri::End => {
            if !test_axis(best, s, &rel, neg(n), 1, true) {
                return false;
            }
            axes.extend([
                (cross(dir, e[1]), 3),
                (cross(dir, e[2]), 4),
                (perp(e[1], q0, v1), 6),
                (perp(e[2], q0, v2), 7),
                (perp(e[1], q1, v1), 9),
                (perp(e[2], q1, v2), 10),
                (perp(dir, v1, q0), 12),
                (perp(dir, v2, q0), 13),
                (sub(v1, q0), 15),
                (sub(v2, q0), 16),
                (sub(v1, q1), 18),
                (sub(v2, q1), 19),
            ]);
        }
        Tri::Mid => {
            if !test_axis(best, s, &rel, neg(n), 1, true) {
                return false;
            }
            axes.extend([
                (cross(dir, e[1]), 3),
                (perp(e[1], q0, v1), 6),
                (perp(e[1], q1, v1), 9),
                (perp(dir, v1, q0), 12),
                (sub(v1, q0), 15),
                (sub(v1, q1), 18),
            ]);
        }
    }
    for (a, id) in axes {
        if !test_axis(best, s, &rel, a, id, false) {
            return false;
        }
    }
    kind != Tri::All || test_axis(best, s, &rel, neg(n), 1, true)
}

/// `_cldClipEdgeToPlane` (0x59c910): `a` and `b` cut to where `n . p + d`
/// is not negative; false when both are behind. The meet is
/// `a - (a - b) t` with `t = da / (da - db)`, written over `b` when `a` is
/// in front, else over `a`.
fn clip(a: &mut V3, b: &mut V3, (n, d): (V3, f32)) -> bool {
    let da = (w(dot(*a, n)) + w(d)) as f32;
    let db = (w(dot(*b, n)) + w(d)) as f32;
    if da < 0.0 && db < 0.0 {
        return false;
    }
    if (da > 0.0 && db < 0.0) || (da < 0.0 && db > 0.0) {
        let dd = sub(*a, *b);
        let t = (w(da) / (w(da) - w(db))) as f32;
        let p = std::array::from_fn(|k| (w(a[k]) - w(dd[k]) * w(t)) as f32);
        if da >= 0.0 {
            *b = p;
        } else {
            *a = p;
        }
    }
    true
}

/// `_cldTestOneTriangleVSCCylinder` as 0x59dd90 (`Tri::All`), 0x59e130
/// (`End`) and 0x59e4a0 (`Mid`) run it on the triangle `v` of the plane
/// `(n, d)` (`n` its front, `v` counterclockwise round it): with no
/// separating axis, the capsule's segment pushed out by its radius along
/// the best normal (0x59dce0), clipped behind the plane and inside the
/// edges the kind keeps (the edge planes 1e-6 out, the float at 0x70b930
/// and the double at 0x70b508), gives a contact at each end, its depth
/// along the normal past the triangle's near side, at least 0 (0x4093c0).
pub fn triangle(
    kind: Tri,
    locals: &mut Vec<Local>,
    s: &Seg,
    (n, _): (V3, f32),
    v: [V3; 3],
    surface: u32,
) {
    let mut best = Best::new();
    if !sat(kind, &mut best, s, n, v) {
        return;
    }
    let [v0, v1, v2] = v;
    let base: V3 = std::array::from_fn(|k| (w(best.normal[k]) * w(s.r) + w(s.c[k])) as f32);
    let mut a: V3 = std::array::from_fn(|k| (w(s.dir[k]) * w(s.h) + w(base[k])) as f32);
    let mut b: V3 = std::array::from_fn(|k| (w(base[k]) + w(s.dir[k]) * w(-s.h)) as f32);
    a = sub(a, v0);
    b = sub(b, v0);
    const OUT: f32 = 1e-6;
    let e0 = sub(v1, v0);
    let far = |e: V3| {
        let m = cross(n, e);
        (m, (-(w(dot(e0, m)) - 1e-6)) as f32)
    };
    let mut planes = vec![(neg(n), 0.0)];
    match kind {
        Tri::All => planes.extend([
            (cross(n, e0), OUT),
            far(sub(v2, v1)),
            (cross(n, sub(v0, v2)), OUT),
        ]),
        Tri::End => planes.extend([far(sub(v2, v1)), (cross(n, sub(v0, v2)), OUT)]),
        Tri::Mid => planes.push(far(sub(v2, v1))),
    }
    for p in planes {
        if !clip(&mut a, &mut b, p) {
            return;
        }
    }
    a = add(v0, a);
    b = add(b, v0);
    let near = w(best.center) - w(best.tr);
    for p in [a, b] {
        let d = (w(dot(sub(p, s.c), best.normal)) - near) as f32;
        let depth = if d < 0.0 { 0.0 } else { d };
        add_local(locals, p, best.normal, depth, surface);
    }
}
