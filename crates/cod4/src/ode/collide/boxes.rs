//! Contains routines ported from the Open Dynamics Engine 0.5, Copyright (C) 2001-2004
//! Russell L. Smith, under its BSD license (LICENSE-ODE-BSD.txt beside this file). See NOTICE.
//!
//! A box geom (class 1) against the clip map, IW's colliders in iw3mp.exe
//! 1.7: the world collider's box arm (0x5a8237), a brush (0x59a610 with its
//! clips 0x597fd0, 0x59a2b0 and the narrow pass 0x59a500) and a partition's
//! triangles (0x59c500 into 0x5b1620, ODE 0.5's box against a triangle list,
//! `collision_trimesh_box.cpp`, as IW rebuilt it); cod4-gsc.md 3.6.5.

use glam::Vec3;

use super::ContactGeom;
use super::poly::clip;
use super::world::Pieces;
use crate::collision::{BrushFaces, Piece};
use crate::ode::math::{M3, V3, cross, dot, dot_wide, neg, normalize3};

#[inline]
fn w(x: f32) -> f64 {
    x as f64
}

/// `phys_narrowObjMaxLength`'s default (0xcc54428, registered at 0x5a4408).
pub const NARROW_MAX_LENGTH: f32 = 4.0;

/// A box geom as the world collider's context holds it (0x5a8110,
/// 0x5a8237): its position (+0x38), its axes, the rotation's columns
/// (+0x44, the rows at +0x68 the same matrix), the half sides (+0x8c), its
/// body's position (+0x98) and the narrow flag (+0xa4).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BoxGeom {
    pub centre: V3,
    pub axes: [V3; 3],
    pub half: V3,
    pub body: V3,
    pub narrow: bool,
}

impl BoxGeom {
    /// 0x5a8237..0x5a8368 off a box of full `sides` at `centre` turned by
    /// `r` (rows), on a body at `body`: the halves `sides * 0.5`, narrow
    /// when a side is under `narrow_max`.
    pub fn new(centre: V3, r: &M3, sides: V3, body: V3, narrow_max: f32) -> BoxGeom {
        BoxGeom {
            centre,
            axes: std::array::from_fn(|k| [r[0][k], r[1][k], r[2][k]]),
            half: sides.map(|s| (w(s) * 0.5) as f32),
            body,
            narrow: sides.iter().any(|&s| narrow_max > s),
        }
    }

    /// Axis `k` negated when `neg`.
    fn axis(&self, k: usize, neg: bool) -> V3 {
        if neg {
            self.axes[k].map(|x| -x)
        } else {
            self.axes[k]
        }
    }

    /// 0x5a82b9..0x5a8425: the box the walk takes, the geom's world
    /// extents grown by 1 round its position.
    pub fn bounds(&self) -> (Vec3, Vec3) {
        let a = self.axes.map(|v| v.map(f32::abs));
        let h = self.half;
        let e: [f64; 3] = std::array::from_fn(|j| {
            let ex = ((w(a[1][j]) * w(h[1]) + w(a[0][j]) * w(h[0])) + w(a[2][j]) * w(h[2])) as f32;
            w(ex) + 1.0
        });
        let c = self.centre;
        (
            Vec3::from(std::array::from_fn::<f32, 3, _>(|j| {
                (w(c[j]) - e[j]) as f32
            })),
            Vec3::from(std::array::from_fn::<f32, 3, _>(|j| {
                (w(c[j]) + e[j]) as f32
            })),
        )
    }

    /// The box's extent along world axis `j` (0x599da0's sums).
    fn extent(&self, j: usize) -> f32 {
        let h = self.half;
        let a = |k: usize| w(self.axes[k][j].abs());
        ((a(2) * w(h[2]) + a(0) * w(h[0])) + a(1) * w(h[1])) as f32
    }

    /// The box face of axis `k` (`neg` its minus side) as 0x597fd0 and
    /// 0x59a2b0 walk it: the corner at `R hs` (`hs` the halves, `hs[k]`
    /// negated for the minus side), then along `-2 h` of the next axis, of
    /// the one after, and back along `2 h` of the next.
    fn face_steps(&self, k: usize, neg: bool) -> (V3, [(f32, V3); 3]) {
        let k1 = (k + 1) % 3;
        let k2 = (k1 + 1) % 3;
        let mut hs = self.half;
        if neg {
            hs[k] = -hs[k];
        }
        let a = &self.axes;
        let v = std::array::from_fn(|j| {
            ((w(hs[1]) * w(a[1][j]) + w(hs[0]) * w(a[0][j])) + w(hs[2]) * w(a[2][j])) as f32
        });
        let h = self.half;
        (
            v,
            [
                ((w(h[k1]) * -2.0) as f32, a[k1]),
                ((w(h[k2]) * -2.0) as f32, a[k2]),
                ((w(h[k1]) + w(h[k1])) as f32, a[k1]),
            ],
        )
    }
}

/// `p + s a`, each component stored.
fn step(p: V3, s: f32, a: V3) -> V3 {
    std::array::from_fn(|j| (w(p[j]) + w(s) * w(a[j])) as f32)
}

fn sub(a: V3, b: V3) -> V3 {
    std::array::from_fn(|j| (w(a[j]) - w(b[j])) as f32)
}

fn add(a: V3, b: V3) -> V3 {
    std::array::from_fn(|j| (w(a[j]) + w(b[j])) as f32)
}

/// The output a collide writes into (0x5a8110's list of `dContactGeom`s).
struct Out<'a> {
    c: &'a mut Vec<ContactGeom>,
    max: usize,
}

impl Out<'_> {
    fn full(&self) -> bool {
        self.c.len() >= self.max
    }

    fn push(&mut self, pos: V3, normal: V3, depth: f32, surface: u32) {
        self.c.push(ContactGeom {
            pos,
            normal,
            depth,
            surface,
        });
    }
}

/// 0x5974b0: how polygon `p` winds round `n`: 0 when the first corner pair
/// whose cross product with the first edge is longer than 0.1 turns with
/// `n` (dot at least 0), 1 against it, 2 when none is.
fn winding_sense(p: &[V3], n: V3) -> u8 {
    if p.len() < 3 {
        return 2;
    }
    let a = sub(p[1], p[0]);
    for i in 1..p.len() - 1 {
        let b = sub(p[i + 1], p[i]);
        let c = cross(a, b);
        if dot(c, c) > 0.01 {
            return if dot(c, n) >= 0.0 { 0 } else { 1 };
        }
    }
    2
}

/// 0x5975e0: the plane through the edge `a -> b` along `n`, `e x n` (or
/// `n x e` when `flip`), unnormalized; none when the edge or the normal's
/// squared length is under 0.1 (the double at 0x70b040).
fn edge_plane(a: V3, b: V3, n: V3, flip: bool) -> Option<(V3, f32)> {
    const MIN: f64 = 0.100_000_001_490_116_12;
    let e = sub(b, a);
    if w(dot(e, e)) < MIN {
        return None;
    }
    let m = if flip { cross(n, e) } else { cross(e, n) };
    if w(dot(m, m)) < MIN {
        return None;
    }
    Some((m, dot(a, m)))
}

/// 0x597e70: polygon `p` cut by each edge plane of `clipper` along `n`
/// (the edges from the last corner round), empty once fewer than three
/// corners are left.
fn clip_by(mut p: Vec<V3>, clipper: &[V3], n: V3) -> Vec<V3> {
    let sense = winding_sense(clipper, n);
    if sense == 2 {
        return Vec::new();
    }
    let m = clipper.len();
    for i in 0..m {
        let prev = clipper[(i + m - 1) % m];
        if let Some((en, ed)) = edge_plane(prev, clipper[i], n, sense == 1)
            && clip(&mut p, (en, ed)) < 3
        {
            return Vec::new();
        }
    }
    p
}

/// 0x599da0: of the brush's faces whose plane the box's centre lies in
/// front of, none may clear the box (a separation of 0 or more ends the
/// collide); the least deep with edges is returned with its plane and
/// separation, in the order `+x -x +y -y +z -z` then the sides; `-1` when
/// none has edges.
// A NaN distance reads in front, as the compare's flags do.
#[allow(clippy::neg_cmp_op_on_partial_ord)]
fn least_deep_face(b: &BoxGeom, f: &BrushFaces) -> Option<(i32, ([f32; 3], f32), f32)> {
    let (mut face, mut plane, mut best) = (-1i32, ([0.0; 3], 0.0), -f32::MAX);
    let c = b.centre;
    let ext = [b.extent(0), b.extent(1), b.extent(2)];
    for &i in &[1usize, 0, 3, 2, 5, 4] {
        let k = i >> 1;
        let d = f.planes[i].1;
        // The minus faces' `d` is `-mins`.
        let gap = if i & 1 == 1 {
            (w(c[k]) - w(d)) as f32
        } else {
            (w(-d) - w(c[k])) as f32
        };
        if gap > 0.0 {
            let s = (w(gap) - w(ext[k])) as f32;
            if s >= 0.0 {
                return None;
            }
            if !f.adjacent[i].is_empty() && (i == 1 || best < s) {
                (face, plane, best) = (i as i32, f.planes[i], s);
            }
        }
    }
    for i in 6..f.planes.len() {
        let (n, d) = f.planes[i];
        let dist = (w(dot(c, n)) - w(d)) as f32;
        if !(dist <= 0.0) {
            let r = |k: usize| w(dot(b.axes[k], n).abs()) * w(b.half[k]);
            let s = (w(dist) - (r(2) + (r(0) + r(1)))) as f32;
            if s >= 0.0 {
                return None;
            }
            if !f.adjacent[i].is_empty() && best < s {
                (face, plane, best) = (i as i32, f.planes[i], s);
            }
        }
    }
    Some((face, plane, best))
}

/// 0x598d60: the face with a winding whose normal is most against `n`
/// (the least dot, under -0.1, the double at 0x70b490).
fn most_against(f: &BrushFaces, wind: &[Vec<V3>], n: V3) -> Option<usize> {
    const MAX_DOT: f64 = -0.100_000_001_490_116_12;
    let mut best = 2.0f32;
    let mut at = None;
    for (i, &(pn, _)) in f.planes.iter().enumerate() {
        if wind[i].is_empty() {
            continue;
        }
        let d = dot(pn, n);
        if d < best && w(d) < MAX_DOT {
            (best, at) = (d, Some(i));
        }
    }
    at
}

/// 0x597fd0: box face `k` (`neg` its minus side) as the reference: its
/// corners put onto the brush face's plane `(pn, pd)`, the face's winding
/// cut by the edges they make (0x5974b0, 0x5975e0, 0x597800), each corner
/// left behind the box face a contact of its depth, the box face's
/// outward normal and the brush face's surface flags.
fn box_reference(
    out: &mut Out,
    b: &BoxGeom,
    k: usize,
    neg: bool,
    wind: &[V3],
    plane: ([f32; 3], f32),
    surface: u32,
) {
    let (pn, pd) = plane;
    let (v, steps) = b.face_steps(k, neg);
    let mut rel = [v; 4];
    for i in 0..3 {
        rel[i + 1] = step(rel[i], steps[i].0, steps[i].1);
    }
    let proj = rel.map(|r| {
        let p = add(b.centre, r);
        let t = -((w(dot(pn, p)) - w(pd)) as f32);
        step(p, t, pn)
    });
    let sense = winding_sense(&proj, pn);
    if sense == 2 {
        return;
    }
    let mut pts = wind.to_vec();
    for i in 0..4 {
        if let Some((en, ed)) = edge_plane(proj[i], proj[(i + 1) % 4], pn, sense == 1)
            && clip(&mut pts, (en, ed)) < 3
        {
            return;
        }
    }
    let n = b.axis(k, neg);
    let d = (w(dot(n, b.centre)) + w(b.half[k])) as f32;
    for p in pts {
        let depth = (w(d) - w(dot(p, n))) as f32;
        if depth > 0.0 {
            if out.full() {
                return;
            }
            out.push(p, n, depth, surface);
            if out.full() {
                return;
            }
        }
    }
}

/// 0x59a2b0: brush face `(pn, pd)` as the reference: the box face most
/// against it (0x5994c0, the least of `a . n` and `-a . n` over the axes,
/// from 1), its corners cut by the face's winding's edges (0x5989f0 into
/// 0x597e70), each corner left behind the face a contact of its depth, the
/// face's normal negated and its surface flags.
fn face_reference(out: &mut Out, b: &BoxGeom, plane: ([f32; 3], f32), wind: &[V3], surface: u32) {
    let (pn, pd) = plane;
    let normal = neg(pn);
    let (mut k, mut flip, mut best) = (0, false, 1.0f32);
    for (i, a) in b.axes.iter().enumerate() {
        let d = dot(*a, pn);
        if d < best {
            (k, flip, best) = (i, false, d);
        }
        if -d < best {
            (k, flip, best) = (i, true, -d);
        }
    }
    let (v, steps) = b.face_steps(k, flip);
    let mut poly = vec![add(b.centre, v); 4];
    for i in 0..3 {
        poly[i + 1] = step(poly[i], steps[i].0, steps[i].1);
    }
    for p in clip_by(poly, wind, pn) {
        let depth = (w(pd) - w(dot(pn, p))) as f32;
        if depth > 0.0 {
            if out.full() {
                return;
            }
            out.push(p, normal, depth, surface);
            if out.full() {
                return;
            }
        }
    }
}

/// 0x59a500: the winding cut by the box's six face planes keeps a corner
/// after each cut.
fn meets_box(b: &BoxGeom, wind: &[V3]) -> bool {
    let mut p = wind.to_vec();
    for k in 0..3 {
        let a = b.axes[k];
        let ac = dot(b.centre, a);
        if clip(&mut p, (a, (w(b.half[k]) + w(ac)) as f32)) == 0 {
            return false;
        }
        if clip(&mut p, (neg(a), (w(b.half[k]) - w(ac)) as f32)) == 0 {
            return false;
        }
    }
    true
}

/// The most corners 0x59a610 lets 0x5998e0 build over all the faces.
const MAX_CORNERS: usize = 0x400;

/// 0x59a610: a box against a brush. The faces' windings in the box's
/// frame give each box face's least depth; any face the brush stays clear
/// of (0 or more) ends the collide, as does 0x599da0's. The less deep of
/// the box's best face and the brush's (0x599da0) is the reference: a box
/// face takes the brush face most against it (0x598d60, 0x597fd0), a brush
/// face the box face most against it (0x59a2b0). A narrow box then also
/// takes as references the axial faces whose side of the bounds the centre
/// lies past and the sides whose plane it lies in front of, each that the
/// box's faces leave something of (0x59a500), but the reference.
pub fn brush(out: &mut Vec<ContactGeom>, max: usize, b: &BoxGeom, f: &BrushFaces) {
    let out = &mut Out { c: out, max };
    let Some((mut ref_face, ref_plane, ref_depth)) = least_deep_face(b, f) else {
        return;
    };
    let mut wind: Vec<Vec<V3>> = Vec::with_capacity(f.planes.len());
    let mut used = 0;
    let mut least = [0.0f32; 6];
    let nh = b.half.map(|h| -h);
    for i in 0..f.planes.len() {
        let wv = f.winding(i, MAX_CORNERS - used);
        for &p in &wv {
            let d = sub(p, b.centre);
            let q = b.axes.map(|a| dot(a, d));
            for k in 0..3 {
                let plus = (w(q[k]) + w(nh[k])) as f32;
                let minus = (w(nh[k]) - w(q[k])) as f32;
                if w(plus) - w(least[k]) < 0.0 {
                    least[k] = plus;
                }
                if w(minus) - w(least[k + 3]) < 0.0 {
                    least[k + 3] = minus;
                }
            }
        }
        used += wv.len();
        wind.push(wv);
    }
    if used == 0 {
        return;
    }
    let (mut bk, mut bs) = (0, 0);
    for k in 0..3 {
        for s in 0..2 {
            let v = least[k + 3 * s];
            if v >= 0.0 {
                return;
            }
            if least[bk + 3 * bs] < v {
                (bk, bs) = (k, s);
            }
        }
    }
    let surface = |i: usize| f.surface[i];
    let ref_wound = ref_face >= 0 && !wind[ref_face as usize].is_empty();
    if ref_depth < least[bk + 3 * bs] || !ref_wound {
        let n = b.axis(bk, bs == 1);
        match most_against(f, &wind, n) {
            Some(i) => {
                ref_face = i as i32;
                box_reference(out, b, bk, bs == 1, &wind[i], f.planes[i], surface(i));
            }
            None => ref_face = -1,
        }
    } else {
        let r = ref_face as usize;
        face_reference(out, b, ref_plane, &wind[r], surface(r));
    }
    if !b.narrow {
        return;
    }
    let c = b.centre;
    let take = |i: usize, out: &mut Out| {
        if meets_box(b, &wind[i]) {
            face_reference(out, b, f.planes[i], &wind[i], surface(i));
        }
    };
    for k in 0..3 {
        let (lo, hi) = (2 * k, 2 * k + 1);
        let mins = -f.planes[lo].1;
        let maxs = f.planes[hi].1;
        if wind[lo].is_empty() || mins <= c[k] {
            if !wind[hi].is_empty() && maxs < c[k] && hi as i32 != ref_face {
                take(hi, out);
            }
        } else if lo as i32 != ref_face {
            take(lo, out);
        }
    }
    for i in 6..f.planes.len() {
        if i as i32 != ref_face && !wind[i].is_empty() {
            let (n, d) = f.planes[i];
            if d < dot(c, n) {
                take(i, out);
            }
        }
    }
}

/// 0x4076d0: `v` divided by its stored length (by 1 when that is not over
/// 0), and the length.
fn normalize_len(v: V3) -> (V3, f32) {
    let l = (w(dot(v, v)).sqrt()) as f32;
    let d = if -l >= 0.0 { 1.0 } else { l };
    let inv = (1.0 / w(d)) as f32;
    (v.map(|x| (w(x) * w(inv)) as f32), l)
}

/// `sqrt` through 0x401050, its argument and result stored.
fn sqrt_st(x: f32) -> f32 {
    w(x).sqrt() as f32
}

/// Of 0x5b1620's collider block, ODE 0.5's globals gathered into one: the
/// best axis so far, the triangle's edges and its normal.
struct Tri {
    best_normal: V3,
    best_depth: f32,
    best_axis: i32,
    e: [V3; 3],
    n: V3,
}

/// 0x5aefa0: the triangle's normal as an axis, its depth scaled by the
/// inverse of the normal's length; kept when strictly less deep.
fn test_normal(t: &mut Tri, depth: f32) {
    let inv = (1.0 / w(sqrt_st(dot(t.n, t.n)))) as f32;
    let d = (w(inv) * w(depth)) as f32;
    if d < t.best_depth {
        t.best_normal = t.n.map(|x| (-w(x) * w(inv)) as f32);
        t.best_axis = 1;
        t.best_depth = d;
    }
}

/// 0x5af030: a box axis `l`: no contact unless both depths `r - min` and
/// `r + max` are at least 0; the axis turned against the triangle's
/// normal when it lies along it (IW's: ODE's takes the lesser depth), its
/// depth `r + max` then, else `r - min`; kept when strictly less deep.
fn test_face(t: &mut Tri, p: [f32; 3], r: f32, mut l: V3, axis: i32) -> bool {
    let min = p[0].min(p[1]).min(p[2]);
    let max = p[0].max(p[1]).max(p[2]);
    let dmin = (w(r) - w(min)) as f32;
    let dmax = (w(r) + w(max)) as f32;
    if dmin < 0.0 || dmax < 0.0 {
        return false;
    }
    let depth = if dot(t.n, l) > 0.0 {
        l = neg(l);
        dmax
    } else {
        dmin
    };
    if depth < t.best_depth {
        (t.best_normal, t.best_axis, t.best_depth) = (l, axis, depth);
    }
    true
}

/// 0x5af170: an edge axis `l`: no contact unless both depths are at least
/// 0; the lesser depth's sense, scaled by the inverse length, kept when 1.5
/// times it (the double at 0x70ada0) is under the best.
fn test_edge(t: &mut Tri, p0: f32, p1: f32, r: f32, mut l: V3, axis: i32) -> bool {
    let (min, max) = if p1 <= p0 { (p1, p0) } else { (p0, p1) };
    let dmin = (w(r) - w(min)) as f32;
    let dmax = (w(r) + w(max)) as f32;
    if dmin < 0.0 || dmax < 0.0 {
        return false;
    }
    let depth = if dmin <= dmax {
        dmin
    } else {
        l = neg(l);
        dmax
    };
    let len = sqrt_st(dot(l, l));
    if len > 0.0 {
        let inv = (1.0 / w(len)) as f32;
        let d = (w(inv) * w(depth)) as f32;
        if 1.5 * w(d) < w(t.best_depth) {
            t.best_normal = l.map(|x| (w(x) * w(inv)) as f32);
            t.best_axis = axis;
            t.best_depth = d;
        }
    }
    true
}

/// The squared length gate on the triangle's edges and normal (the float
/// 0.01 at 0x6df494).
const MIN_EDGE: f32 = 0.01;

/// 0x5af410, ODE's `_cldTestSeparatingAxes` as IW rebuilt it: a triangle
/// whose edges' squared lengths or normal's length is under 0.01 is
/// skipped, as is one the box's centre lies behind or on (`n . (v0 - c)` at
/// least 0) while its body's position lies behind; then the normal (1),
/// the box's axes (2..4) and the axes crossed with the edges (5..13).
fn separating_axes(b: &BoxGeom, v: [V3; 3]) -> Option<Tri> {
    let e0 = sub(v[1], v[0]);
    if dot(e0, e0) < MIN_EDGE {
        return None;
    }
    let e1 = sub(v[2], v[0]);
    if dot(e1, e1) < MIN_EDGE {
        return None;
    }
    let e2 = sub(e1, e0);
    if dot(e2, e2) < MIN_EDGE {
        return None;
    }
    let (n, len) = normalize_len(cross(e0, e1));
    if len < MIN_EDGE {
        return None;
    }
    let mut t = Tri {
        best_normal: [0.0; 3],
        best_depth: f32::MAX,
        best_axis: 0,
        e: [e0, e1, e2],
        n,
    };
    let a = b.axes;
    let h = b.half;
    let d = sub(v[0], b.centre);
    let nd = dot(n, d);
    if nd >= 0.0 && ((w(dot(n, b.body)) - w(dot(n, v[0]))) as f32) < 0.0 {
        return None;
    }
    let r = |k: usize| w(dot(n, a[k]).abs()) * w(h[k]);
    let fr = ((r(1) + r(0)) + r(2)) as f32;
    let depth = (w(fr) + w(nd)) as f32;
    if depth < 0.0 {
        return None;
    }
    test_normal(&mut t, depth);
    for k in 0..3 {
        let p0 = dot(a[k], d);
        let p1 = (w(p0) + w(dot(a[k], e0))) as f32;
        let p2 = (w(p0) + w(dot(a[k], e1))) as f32;
        if !test_face(&mut t, [p0, p1, p2], h[k], a[k], 2 + k as i32) {
            return None;
        }
    }
    for k in 0..3 {
        let (k1, k2) = ((k + 1) % 3, (k + 2) % 3);
        let an = w(dot(a[k], n)) * w(len);
        for (j, ej) in [e0, e1, e2].into_iter().enumerate() {
            let l = cross(a[k], ej);
            let p0 = dot(l, d);
            let p1 = (if j == 0 { w(p0) + an } else { w(p0) - an }) as f32;
            let rr = |m: usize, hm: usize| w(dot(ej, a[m]).abs()) * w(h[hm]);
            let fr = (rr(k2, k1) + rr(k1, k2)) as f32;
            if !test_edge(&mut t, p0, p1, fr, l, 5 + (3 * k + j) as i32) {
                return None;
            }
        }
    }
    Some(t)
}

/// ODE's `_cldClipPolyToPlane` (0x5af2a0): the corners of `p` with
/// `n . x + d` at least 0, and each edge's meet with the plane where it
/// crosses, from the last corner round.
fn clip_poly(p: &[V3], n: V3, d: f32) -> Vec<V3> {
    let mut out = Vec::with_capacity(p.len() + 1);
    let m = p.len();
    for i in 0..m {
        let a = p[(i + m - 1) % m];
        let b = p[i];
        let da = (dot_wide(a, n) + w(d)) as f32;
        let db = (dot_wide(b, n) + w(d)) as f32;
        if da >= 0.0 {
            out.push(a);
        }
        if (da > 0.0 && db < 0.0) || (da < 0.0 && db > 0.0) {
            let den = w(da) - w(db);
            out.push(std::array::from_fn(|j| {
                (w(a[j]) - (w(a[j]) - w(b[j])) * w(da) / den) as f32
            }));
        }
    }
    out
}

/// ODE's `_cldClosestPointOnTwoLines` (0x5b04a0): the parameters along
/// `ua` from `pa` and `ub` from `pb` of the lines' nearest points, both 0
/// when they are parallel.
fn closest(pa: V3, ua: V3, pb: V3, ub: V3) -> (f32, f32) {
    let uaub = dot(ua, ub);
    let vp = sub(pb, pa);
    let q1 = dot(ua, vp);
    let q2 = -dot(ub, vp);
    let d = (1.0 - w(uaub) * w(uaub)) as f32;
    if d <= 0.0 {
        return (0.0, 0.0);
    }
    let inv = w((1.0 / w(d)) as f32);
    (
        ((w(q1) + w(uaub) * w(q2)) * inv) as f32,
        ((w(uaub) * w(q1) + w(q2)) * inv) as f32,
    )
}

/// 0x5b0590, ODE's `_cldClipping` as IW rebuilt it: an edge axis gives one
/// contact between the edges' nearest points (no room check), a box axis
/// the triangle clipped by that face and its four sides; then, whatever
/// the axis, the box face most along the triangle's normal clipped by the
/// triangle's plane and edges, each point behind the triangle a contact
/// along the normal negated (IW's: ODE does this for axis 1 alone, along
/// the best normal).
fn clipping(out: &mut Out, b: &BoxGeom, t: &Tri, v: [V3; 3]) {
    let a = b.axes;
    let h = b.half;
    let c = b.centre;
    let axis = t.best_axis;
    if axis >= 5 {
        // The box's corner most along the best normal, an axis at a time.
        let mut pa = c;
        for k in 0..3 {
            let sign = if dot(a[k], t.best_normal) > 0.0 {
                1.0
            } else {
                -1.0
            };
            pa = step(pa, (w(h[k]) * sign) as f32, a[k]);
        }
        let edge = ((axis - 5) % 3) as usize;
        let col = ((axis - 5) / 3) as usize;
        let (pb, ub) = match edge {
            0 => (v[0], t.e[0]),
            1 => (v[2], t.e[1]),
            _ => (v[1], t.e[2]),
        };
        let ub = normalize3(ub);
        let ua = a[col];
        let (f1, f2) = closest(pa, ua, pb, ub);
        pa = step(pa, f1, ua);
        let pb = step(pb, f2, ub);
        let pos = add(pb, pa).map(|x| (w(x) * 0.5) as f32);
        out.push(pos, t.best_normal, t.best_depth, 0);
    } else if axis >= 2 {
        let i0 = (axis - 2) as usize;
        let (i1, i2) = match i0 {
            0 => (1, 2),
            1 => (0, 2),
            _ => (0, 1),
        };
        let nb = t.best_normal;
        let mut p: Vec<V3> = v.iter().map(|&x| sub(x, c)).collect();
        p = clip_poly(&p, neg(nb), h[i0]);
        p = clip_poly(&p, a[i1], h[i1]);
        p = clip_poly(&p, neg(a[i1]), h[i1]);
        p = clip_poly(&p, a[i2], h[i2]);
        p = clip_poly(&p, neg(a[i2]), h[i2]);
        for q in p {
            let mut dep = (w(dot(nb, q)) - w(h[i0])) as f32;
            if dep > 0.0 {
                dep = 0.0;
            }
            if out.full() {
                break;
            }
            out.push(add(c, q), nb, -dep, 0);
        }
    }
    let n = t.n;
    let vnr = a.map(|x| dot(x, n));
    let ab = vnr.map(f32::abs);
    let (b0, b1, b2) = if ab[1] > ab[0] {
        if ab[1] > ab[2] { (1, 0, 2) } else { (2, 0, 1) }
    } else if ab[0] > ab[2] {
        (0, 1, 2)
    } else {
        (2, 0, 1)
    };
    let hb = h[b0];
    let centre: V3 = std::array::from_fn(|j| {
        let r = w(c[j]) - w(v[0][j]);
        let o = w(hb) * w(a[b0][j]);
        (if vnr[b0] > 0.0 { r - o } else { r + o }) as f32
    });
    let (u, uu) = (a[b1], a[b2]);
    let corner = |su: f64, sv: f64| -> V3 {
        std::array::from_fn(|j| {
            (w(centre[j]) + su * w(u[j]) * w(h[b1]) + sv * w(uu[j]) * w(h[b2])) as f32
        })
    };
    let quad = [
        corner(1.0, -1.0),
        corner(-1.0, -1.0),
        corner(-1.0, 1.0),
        corner(1.0, 1.0),
    ];
    let mut p = clip_poly(&quad, normalize3(neg(n)), 0.0);
    let side = |from: V3, to: V3| normalize3(cross(n, sub(to, from)));
    p = clip_poly(&p, side(v[0], v[1]), 0.0);
    let n2 = side(v[1], v[2]);
    p = clip_poly(&p, n2, dot(sub(v[0], v[2]), n2));
    p = clip_poly(&p, side(v[2], v[0]), 0.0);
    let normal = neg(n);
    for q in p {
        let mut dep = dot(q, n);
        if dep > 0.0 {
            dep = 0.0;
        }
        if out.full() {
            return;
        }
        out.push(add(v[0], q), normal, -dep, 0);
    }
}

/// 0x59c500 into 0x5b1620: each triangle `t` of a partition, taken as ODE's
/// `(v0, v1, v2) = (t2, t1, t0)`, while the contacts have room, through the
/// separating axes and the clip; the contacts take the partition's
/// surface flags.
pub fn tris(out: &mut Vec<ContactGeom>, max: usize, b: &BoxGeom, tris: &[[Vec3; 3]], surface: u32) {
    let start = out.len();
    {
        let out = &mut Out { c: out, max };
        for t in tris {
            if out.full() {
                break;
            }
            let v = [t[2].to_array(), t[1].to_array(), t[0].to_array()];
            if let Some(st) = separating_axes(b, v) {
                clipping(out, b, &st, v);
            }
        }
    }
    for c in &mut out[start..] {
        c.surface = surface;
    }
}

/// `dCollide(o1 = the box's geom, o2 = the world)` (0x5a95a0) as the near
/// callback (0x5a5f20) calls it with room for `max`: 0x5a8110 walks the
/// world pieces round [`BoxGeom::bounds`] under `mask` (0x5a882c), a brush
/// only while there is room (0x5a79b0), and the reversed collider's normals
/// are turned round, so each points out of the world into the box.
pub fn collide_world(world: &dyn Pieces, b: &BoxGeom, mask: u32, max: usize) -> Vec<ContactGeom> {
    let (lo, hi) = b.bounds();
    let mut out = Vec::new();
    for piece in world.pieces(Vec3::from(b.centre), lo, hi, mask) {
        match piece {
            Piece::Brush(i) => {
                if out.len() < max {
                    brush(&mut out, max, b, &world.brush(i));
                }
            }
            Piece::Tris { tris: t, surface } => tris(&mut out, max, b, &t, surface),
        }
    }
    for c in &mut out {
        c.normal = neg(c.normal);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ode::collide::tests::{Brushes, cuboid};
    use crate::ode::math::IDENTITY;

    /// A box half a unit into a brush's top and half a unit past its +x
    /// edge: the +x face is the reference; a narrow box (a side under
    /// `phys_narrowObjMaxLength`) also takes the top, whose side of the
    /// bounds its centre lies past.
    #[test]
    fn a_narrow_box_over_a_brushs_edge_takes_its_top_too() {
        let world = Brushes(vec![cuboid([-16.0; 3], [0.0; 3])]);
        let normals = |narrow_max: f32| {
            let c = [0.5, -8.0, 0.5];
            let b = BoxGeom::new(c, &IDENTITY, [2.0; 3], c, narrow_max);
            let mut n: Vec<V3> = collide_world(&world, &b, !0, 0x80)
                .iter()
                .map(|c| c.normal)
                .collect();
            n.dedup();
            (b.narrow, n)
        };
        assert_eq!(normals(4.0), (true, vec![[1.0, 0.0, 0.0], [0.0, 0.0, 1.0]]));
        assert_eq!(normals(1.0), (false, vec![[1.0, 0.0, 0.0]]));
    }
}
