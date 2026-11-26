//! A cylinder geom (class 13) against the clip map, IW's colliders in
//! iw3mp.exe 1.7: the world collider's cylinder arm (0x5a86ad), a brush
//! (0x5a1330) and a partition's triangles (0x5a17c0), each face through
//! one polygon test (0x5a0f70: the separating axes 0x59fc90 and the
//! contact arms by the best axis); cod4-gsc.md 3.6.5.

// Each negated compare sends a NaN the way the x87 flags do.
#![allow(clippy::neg_cmp_op_on_partial_ord)]

use glam::Vec3;

use super::ContactGeom;
use super::world::Pieces;
use crate::collision::{BrushFaces, Piece};
use crate::ode::math::{M3, V3, cross, dot, neg, sub};
use crate::ode::phys::normalize;

#[inline]
fn w(x: f32) -> f64 {
    x as f64
}

/// `sqrt(2)` as the double at 0x70ba88.
const SQRT2: f64 = 1.414_214_015_007_019;
/// The floats at 0x70b234 and 0x70b880.
const AXIS_MIN: f32 = 1e-5;
const EDGE_AXIS_DOT: f32 = 0.02;
/// The double at 0x70af08: the normal's arms split at this `|a . n|`.
const CAP_DOT: f64 = 0.9;
/// The double at 0x70b720, the double at 0x70b5c0.
const SIDE_MIN: f64 = 1e-5;
const CAP_MIN: f64 = 1e-10;
/// The double at 0x70b000.
const PARALLEL: f64 = 9.999_999_747_378_752e-5;
/// 0x5a09a0's rim offsets: `r` times 0 (0x70adf0), `sqrt(1/2)` (0x70ae70)
/// and its negation (0x70b718), -1 (0x70b2d8).
const HALF_SQRT2: f64 = 0.707_106_769_084_930_4;
/// The most points 0x59f170 cuts a polygon to (the 0x100 at 0x59f270).
const MAX_CUT: usize = 0x100;
/// The most corners 0x5a1330 lets 0x5998e0 build (the 0x100 it pushes).
const MAX_WINDING: usize = 0x100;

type Plane = (V3, f32);

/// A cylinder geom as the world collider's context holds it (0x5a8110,
/// its arm 0x5a86ad): the geom's position (+0x38), its rotation's
/// columns (+0x44) and the one that is its axis (+4, 1-based), its
/// radius (+0x8c) and half height (+0x94), its body's position (+0x98)
/// and the narrow flag (+0xa4).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Cylinder {
    pub centre: V3,
    pub cols: [V3; 3],
    pub axis: usize,
    pub radius: f32,
    pub half_height: f32,
    pub body: V3,
    pub narrow: bool,
}

impl Cylinder {
    /// The context off a class 13 geom at `pos` turned by `r` (rows), its
    /// axis column `direction` (1..=3), on a body at `body`: narrow when
    /// `2 r` is under `narrow_max` (0x5a8710).
    pub fn new(
        pos: V3,
        r: &M3,
        direction: usize,
        radius: f32,
        half_height: f32,
        body: V3,
        narrow_max: f32,
    ) -> Cylinder {
        Cylinder {
            centre: pos,
            cols: std::array::from_fn(|j| [r[0][j], r[1][j], r[2][j]]),
            axis: direction - 1,
            radius,
            half_height,
            body,
            narrow: w(narrow_max) > w(radius) + w(radius),
        }
    }

    fn a(&self) -> V3 {
        self.cols[self.axis]
    }

    /// 0x5a86ed and 0x5a8370: the box the walk takes, a cube of
    /// `max(r, h)` (0x4093c0) times the double sqrt(2) grown by 1 round the
    /// centre.
    pub fn bounds(&self) -> (Vec3, Vec3) {
        let e = (w(crate::ode::phys::fmax(self.radius, self.half_height)) * SQRT2) as f32;
        let e1 = w(e) + 1.0;
        let c = self.centre;
        (
            Vec3::from(c.map(|x| (w(x) - e1) as f32)),
            Vec3::from(c.map(|x| (e1 + w(x)) as f32)),
        )
    }
}

/// The best axis 0x59fc90 keeps: its overlap (+0), code (+4) and the
/// axis, turned against the face's normal (+8).
#[derive(Clone, Copy, Debug, PartialEq)]
struct Best {
    depth: f32,
    code: usize,
    axis: V3,
}

/// The contacts a collide gives (the list 0x5973e0 adds to: room for
/// `max`).
struct Out<'a> {
    v: &'a mut Vec<ContactGeom>,
    max: usize,
}

impl Out<'_> {
    /// 0x5973e0: nothing once full; false once the contact made it full.
    fn add(&mut self, pos: V3, normal: V3, depth: f32, surface: u32) -> bool {
        if self.v.len() >= self.max {
            return false;
        }
        self.v.push(ContactGeom {
            pos,
            normal,
            depth,
            surface,
        });
        self.v.len() < self.max
    }
}

/// `p - q` turned by `t` from `q`, each part stored: `(p - q) t + q`
/// (0x432700, 0x597700's and 0x597800's cuts).
fn lerp(q: V3, p: V3, t: f32) -> V3 {
    std::array::from_fn(|k| ((w(p[k]) - w(q[k])) * w(t) + w(q[k])) as f32)
}

/// `a s + b`, each part stored.
fn mad(a: V3, s: f32, b: V3) -> V3 {
    std::array::from_fn(|k| (w(a[k]) * w(s) + w(b[k])) as f32)
}

/// `dot(p, n) - d`, the dot stored, then the difference.
fn dist(p: V3, (n, d): Plane) -> f32 {
    (w(dot(p, n)) - w(d)) as f32
}

/// 0x5a11f0: a face's depth, `-FLT_MAX` (0x70af5c) when both the centre
/// and the body lie behind its plane, else the centre's height over it
/// less the cylinder's reach toward it, `h |c| + r sqrt(1 - c^2)`, `c` the
/// axis's dot with the normal (clamped to 1).
fn face_depth(cy: &Cylinder, p: Plane) -> f32 {
    let dc = dist(cy.centre, p);
    if dc < 0.0 && dist(cy.body, p) < 0.0 {
        return -f32::MAX;
    }
    let mut c = dot(cy.a(), p.0).abs();
    if ((1.0 - w(c)) as f32) < 0.0 {
        c = 1.0;
    }
    let x = (w(dc) - w(c) * w(cy.half_height)) as f32;
    let s = (1.0 - w(c) * w(c)) as f32;
    let s = w(s).sqrt() as f32;
    (w(x) - w(s) * w(cy.radius)) as f32
}

/// 0x59f5d0: `axis` turned so its dot with `reference` is not positive,
/// the cylinder's reach along it against the polygon's corners off the
/// centre. Apart when the reach is under the nearest corner or the
/// farthest under minus the reach; else, unless `test_only`, the overlap
/// takes the best when the best is over it plus `bias`.
#[allow(clippy::too_many_arguments)]
fn test_axis(
    cy: &Cylinder,
    poly: &[V3],
    axis: V3,
    reference: V3,
    code: usize,
    bias: f32,
    test_only: bool,
    best: &mut Best,
) -> bool {
    let along = -dot(reference, axis);
    let u = if along >= 0.0 { axis } else { neg(axis) };
    let mut c = dot(cy.a(), u).abs();
    if ((1.0 - w(c)) as f32) < 0.0 {
        c = 1.0;
    }
    let c2 = (w(c) * w(c)) as f32;
    let s = w((1.0 - w(c2)) as f32).sqrt() as f32;
    let hc = (w(cy.half_height) * w(c)) as f32;
    let reach = (w(s) * w(cy.radius) + w(hc)) as f32;
    let cd = dot(cy.centre, u);
    let (mut lo, mut hi) = (f32::MAX, -f32::MAX);
    for &p in poly {
        let rel = (w(dot(p, u)) - w(cd)) as f32;
        if ((w(rel) - w(lo)) as f32) < 0.0 {
            lo = rel;
        }
        if ((w(hi) - w(rel)) as f32) < 0.0 {
            hi = rel;
        }
    }
    if reach < lo || -reach > hi {
        return false;
    }
    if test_only {
        return true;
    }
    let ov = (w(reach) - w(lo)) as f32;
    if w(best.depth) > w(ov) + w(bias) {
        *best = Best {
            depth: ov,
            code,
            axis: u,
        };
    }
    true
}

/// 0x59f9f0: the axis from cap centre `cap`'s rim to the edge `e` out of
/// corner `p`, with a bias of 1 (the float 0x3f800000 it passes). None
/// when the edge is shorter than 1e-5, runs across the axis, or lies
/// where the cap's side of it faces away.
fn rim_axis(
    cy: &Cylinder,
    (n, _): Plane,
    poly: &[V3],
    p: V3,
    e: V3,
    cap: V3,
    code: usize,
    best: &mut Best,
) -> bool {
    let a = cy.a();
    let (u, len) = normalize(e);
    if len < AXIS_MIN {
        return true;
    }
    let ua = dot(a, u);
    if ua.abs() < AXIS_MIN {
        return true;
    }
    let wv = cross(u, n);
    let kc = dot(cy.centre, wv);
    let kp = dot(wv, p);
    if kp < kc && dot(wv, a) > 0.0 {
        return true;
    }
    let s = dot(a, sub(cap, p));
    let t = (w(s) / w(ua)) as f32;
    let q = mad(u, t, p);
    let x = cross(sub(cap, q), a);
    let (y, _) = normalize(cross(x, u));
    test_axis(cy, poly, y, n, code, 1.0, false, best)
}

/// 0x59fc90: the separating axes in IW's order, each through 0x59f5d0:
/// the normal negated (1), the axis (2), each edge crossed with the axis
/// (3 on, all skipped when `|a . n|` is at most 0.02; one only tested when
/// its edge's in-plane normal points away from it), the line from the
/// axis to each corner, then each corner's edge against each cap's rim.
fn sat(cy: &Cylinder, plane: Plane, poly: &[V3], best: &mut Best) -> bool {
    let n = plane.0;
    let a = cy.a();
    *best = Best {
        depth: f32::MAX,
        code: 0,
        axis: [0.0; 3],
    };
    if !test_axis(cy, poly, neg(n), n, 1, 0.0, false, best)
        || !test_axis(cy, poly, a, n, 2, 0.0, false, best)
    {
        return false;
    }
    let k = poly.len();
    let e: Vec<V3> = (0..k).map(|i| sub(poly[(i + 1) % k], poly[i])).collect();
    let mut code = 3;
    if dot(a, n).abs() <= EDGE_AXIS_DOT {
        code += k;
    } else {
        for ei in &e {
            let mut m = cross(a, *ei);
            if dot(n, m) > 0.0 {
                m = neg(m);
            }
            let q = cross(*ei, n);
            let (m, len) = normalize(m);
            if len > AXIS_MIN {
                let test_only = dot(q, m) < 0.0;
                if !test_axis(cy, poly, m, n, code, 0.0, test_only, best) {
                    return false;
                }
            }
            code += 1;
        }
    }
    for &p in poly {
        let d = sub(p, cy.centre);
        let s = -dot(a, d);
        let (l, len) = normalize(mad(a, s, d));
        if len > AXIS_MIN && !test_axis(cy, poly, l, n, code, 0.0, false, best) {
            return false;
        }
        code += 1;
    }
    let h = cy.half_height;
    let cp = mad(a, h, cy.centre);
    let cm = mad(a, -h, cy.centre);
    for (i, &p) in poly.iter().enumerate() {
        for cap in [cp, cm] {
            if !rim_axis(cy, plane, poly, p, e[i], cap, code, best) {
                return false;
            }
            code += 1;
        }
    }
    true
}

/// 0x5974b0: 2 when no two edges from the first corner span over 0.01
/// squared, else 1 when they wind clockwise round `n`, 0 otherwise.
fn orient(poly: &[V3], n: V3) -> u8 {
    let k = poly.len();
    if k < 3 {
        return 2;
    }
    let e0 = sub(poly[1], poly[0]);
    for i in 1..k - 1 {
        let c = cross(e0, sub(poly[i + 1], poly[i]));
        if w(dot(c, c)) > w(0.01f32) {
            return u8::from(dot(n, c) < 0.0);
        }
    }
    2
}

/// 0x5975e0: the plane through `b` of the edge `b` to `a` facing out of
/// the polygon (`n x d` when it winds clockwise, else `d x n`, not
/// normalized), none for an edge or a normal under 0.1 squared (the
/// double at 0x70b040).
fn edge_plane(a: V3, b: V3, n: V3, clockwise: bool) -> Option<Plane> {
    const MIN: f64 = 0.100_000_001_490_116_12;
    let d = sub(a, b);
    if w(dot(d, d)) < MIN {
        return None;
    }
    let m = if clockwise { cross(n, d) } else { cross(d, n) };
    if w(dot(m, m)) < MIN {
        return None;
    }
    Some((m, dot(b, m)))
}

/// 0x597700: the segment `q`..`p` cut to the plane's back (`n . x - d` at
/// most 0); false when both ends lie in front.
fn clip_seg(pl: Plane, q: &mut V3, p: &mut V3) -> bool {
    let dq = dist(*q, pl);
    let dp = dist(*p, pl);
    let out = |x: f32| !(x <= 0.0);
    match (out(dq), out(dp)) {
        (false, false) => {}
        (false, true) => {
            let t = (w(dq) / (w(dq) - w(dp))) as f32;
            *p = lerp(*q, *p, t);
        }
        (true, false) => {
            let t = (w(dp) / (w(dp) - w(dq))) as f32;
            *q = lerp(*p, *q, t);
        }
        (true, true) => return false,
    }
    true
}

/// 0x597f30: the segment cut to the polygon's edges in turn (0x5975e0
/// from each corner to the next); false when the polygon has no area or
/// an edge leaves nothing.
fn seg_in_poly(poly: &[V3], n: V3, q: &mut V3, p: &mut V3) -> bool {
    let o = orient(poly, n);
    if o == 2 {
        return false;
    }
    for k in 0..poly.len() {
        let j = (k + 1) % poly.len();
        if let Some(pl) = edge_plane(poly[j], poly[k], n, o == 1)
            && !clip_seg(pl, q, p)
        {
            return false;
        }
    }
    true
}

/// 0x597800 with 0x597440: a convex polygon cut to the plane's back
/// (`n . x` at most `d`) in place, its corners kept in order: the corner
/// leaving the back becomes the exit, the one coming back the entry
/// (one corner out is split in two), those between go. Returns the
/// count, 0 when no corner is behind.
fn clip_poly(pts: &mut Vec<V3>, (m, d): Plane) -> usize {
    let n = pts.len();
    if n == 0 {
        return 0;
    }
    let inside: Vec<bool> = pts.iter().map(|&p| !(d < dot(p, m))).collect();
    let (mut exit, mut entry, mut any) = (None, None, false);
    let mut prev = n - 1;
    for i in 0..n {
        if inside[prev] {
            any = true;
            if !inside[i] {
                exit = Some((prev, i));
            }
        } else if inside[i] {
            entry = Some((prev, i));
        }
        prev = i;
    }
    if !any {
        return 0;
    }
    let (Some((mut xp, xc)), Some((ep, ec))) = (exit, entry) else {
        return n;
    };
    // `p` toward `q` cut at the plane, `p` kept when the edge lies in it.
    let cut = |p: V3, q: V3| {
        let dd = dot(m, sub(p, q));
        (dd != 0.0).then(|| {
            let t = ((w(d) - w(dot(q, m))) / w(dd)) as f32;
            lerp(q, p, t)
        })
    };
    if xc == ep {
        let p = pts[xc];
        pts.insert(xc + 1, p);
        let n1 = n + 1;
        let s = (ep + 1) % n1;
        let q = (s + 1) % n1;
        if xp > xc {
            xp += 1;
        }
        pts[s] = cut(p, pts[q]).unwrap_or(p);
        if let Some(x) = cut(pts[xc], pts[xp]) {
            pts[xc] = x;
        }
        return n1;
    }
    if let Some(x) = cut(pts[xc], pts[xp]) {
        pts[xc] = x;
    }
    if let Some(x) = cut(pts[ep], pts[ec]) {
        pts[ep] = x;
    }
    if (xc + 1) % n == ep {
        n
    } else if xc < ep {
        pts.drain(xc + 1..ep);
        pts.len()
    } else {
        pts.truncate(xc + 1);
        pts.drain(..ep);
        pts.len()
    }
}

/// 0x597e70: `pts` cut by each edge of `poly` in turn (0x5975e0 from the
/// corner before to each), empty once fewer than three corners stay.
fn poly_in_poly(mut pts: Vec<V3>, poly: &[V3], n: V3) -> Vec<V3> {
    let o = orient(poly, n);
    if o == 2 {
        return Vec::new();
    }
    let k = poly.len();
    for i in 0..k {
        let prev = if i == 0 { k - 1 } else { i - 1 };
        if let Some(pl) = edge_plane(poly[i], poly[prev], n, o == 1) {
            let c = clip_poly(&mut pts, pl);
            pts.truncate(c);
            if c < 3 {
                return Vec::new();
            }
        }
    }
    pts
}

/// 0x59ee60: a contact at `p` when it lies on the plane's back.
fn plane_contact(out: &mut Out, pl: Plane, p: V3, normal: V3, surface: u32) {
    let depth = (w(pl.1) - w(dot(p, pl.0))) as f32;
    if depth >= 0.0 {
        out.add(p, normal, depth, surface);
    }
}

/// 0x5a02b0: the cylinder's side line toward the face, its axis moved
/// out by `r` along the best axis less its part along the axis, clipped
/// to the polygon (0x597f30); each end behind the plane a contact.
fn side(cy: &Cylinder, pl: Plane, poly: &[V3], best: &Best, surface: u32, out: &mut Out) {
    let a = cy.a();
    let b = best.axis;
    let s = -dot(a, b);
    let (v, len) = normalize(mad(a, s, b));
    if w(len) < SIDE_MIN {
        return;
    }
    let m = mad(v, cy.radius, cy.centre);
    let h = cy.half_height;
    let mut p0 = mad(a, h, m);
    let mut p1 = mad(a, -h, m);
    if seg_in_poly(poly, pl.0, &mut p0, &mut p1) {
        plane_contact(out, pl, p0, b, surface);
        plane_contact(out, pl, p1, b, surface);
    }
}

/// 0x5a09a0: the cap facing the face as eight points round its rim, cut
/// to the polygon (0x5989f0: 0x597e70), each point behind the plane a
/// contact along the normal negated.
fn cap(cy: &Cylinder, (n, d): Plane, poly: &[V3], surface: u32, out: &mut Out) {
    let a = cy.a();
    let s = dot(a, n);
    let (wv, k) = if s <= 0.0 || s.is_nan() {
        (a, s)
    } else {
        (neg(a), -s)
    };
    let nn = neg(n);
    let mut v = mad(wv, k, nn);
    let l2 = dot(v, v);
    if w(l2) >= CAP_MIN || l2.is_nan() {
        let inv = (1.0 / w(w(l2).sqrt() as f32)) as f32;
        v = v.map(|x| (w(x) * w(inv)) as f32);
    } else {
        v = cy.cols[(cy.axis + 1) % 3];
    }
    let x = cross(v, wv);
    let cc = mad(wv, cy.half_height, cy.centre);
    let r = cy.radius;
    let z = (w(r) * 0.0) as f32;
    let p = (w(r) * HALF_SQRT2) as f32;
    let m = (w(r) * -HALF_SQRT2) as f32;
    let mr = -r;
    let rim: Vec<V3> = [
        (r, z),
        (p, p),
        (z, r),
        (m, p),
        (mr, z),
        (m, m),
        (z, mr),
        (p, m),
    ]
    .iter()
    .map(|&(cs, sn)| {
        std::array::from_fn(|k| ((w(cs) * w(x[k]) + w(cc[k])) + w(sn) * w(v[k])) as f32)
    })
    .collect();
    for q in poly_in_poly(rim, poly, n) {
        let depth = (w(d) - w(dot(n, q))) as f32;
        if depth > 0.0 && !out.add(q, nn, depth, surface) {
            return;
        }
    }
}

/// 0x59eeb0: the segment `s`..`e` cut to the infinite cylinder round the
/// axis; none when it runs along the axis (`|a x d|^2` under 0.001) or
/// misses.
fn seg_in_tube(cy: &Cylinder, s: V3, e: V3) -> Option<[V3; 2]> {
    let a = cy.a();
    let d = sub(e, s);
    let m = cross(a, sub(s, cy.centre));
    let k = cross(a, d);
    let kk = dot(k, k);
    if w(kk) < w(0.001f32) {
        return None;
    }
    let b2 = (w(dot(k, m)) * 2.0) as f32;
    let cc = (w(dot(m, m)) - w((w(cy.radius) * w(cy.radius)) as f32)) as f32;
    let disc = (w(b2) * w(b2) - w(cc) * (w(kk) * 4.0)) as f32;
    if disc < 0.0 {
        return None;
    }
    let root = w(disc).sqrt() as f32;
    let inv = (0.5 / w(kk)) as f32;
    let t1 = ((-w(b2) - w(root)) * w(inv)) as f32;
    let t2 = ((w(root) - w(b2)) * w(inv)) as f32;
    if t1 > 1.0 || t2 < 0.0 {
        return None;
    }
    Some([
        if t1 > 0.0 { lerp(s, e, t1) } else { s },
        if t2 < 1.0 { lerp(s, e, t2) } else { e },
    ])
}

/// 0x59f170: the polygon cut to the cylinder's circle (inside when
/// `|a x (p - c)|^2` is at most `r^2`), up to 256 points. A crossing whose
/// cut misses (none seen) keeps the last cut's points, as the binary's
/// stack does.
fn circle_cut(cy: &Cylinder, poly: &[V3]) -> Vec<V3> {
    let a = cy.a();
    let rr = (w(cy.radius) * w(cy.radius)) as f32;
    let inside = |p: V3| {
        let m = cross(a, sub(p, cy.centre));
        !(rr < dot(m, m))
    };
    let mut out = Vec::new();
    let mut tmp = [[0.0; 3]; 2];
    let Some(&last) = poly.last() else {
        return out;
    };
    let (mut prev, mut prev_in) = (last, inside(last));
    for &cur in poly {
        if out.len() >= MAX_CUT {
            return out;
        }
        let cur_in = inside(cur);
        let cut = if prev_in && cur_in {
            None
        } else {
            seg_in_tube(cy, prev, cur)
        };
        if let Some(c) = cut {
            tmp = c;
        }
        match (prev_in, cur_in) {
            (true, true) => out.push(cur),
            (true, false) => out.push(tmp[1]),
            (false, true) => {
                out.push(tmp[0]);
                if out.len() >= MAX_CUT {
                    return out;
                }
                out.push(cur);
            }
            (false, false) => {
                if cut.is_some() {
                    out.push(tmp[0]);
                    if out.len() >= MAX_CUT {
                        return out;
                    }
                    out.push(tmp[1]);
                }
            }
        }
        (prev, prev_in) = (cur, cur_in);
    }
    out
}

/// 0x59f460: the polygon cut to the cylinder's circle, each point within
/// the cap facing the face a contact along the axis, that deep.
fn axis_arm(cy: &Cylinder, (n, _): Plane, poly: &[V3], surface: u32, out: &mut Out) {
    let a = cy.a();
    let h = cy.half_height;
    let ac = dot(a, cy.centre);
    let t = dot(a, n);
    for p in circle_cut(cy, poly) {
        let s = (w(dot(p, a)) - w(ac)) as f32;
        if t >= 0.0 || t.is_nan() {
            if -h <= s || s.is_nan() {
                out.add(p, neg(a), (w(s) + w(h)) as f32, surface);
            }
        } else if !(h < s) {
            out.add(p, a, (w(h) - w(s)) as f32, surface);
        }
    }
}

/// 0x560050: the parameters of the nearest points of the lines
/// `p1 + s1 d1` and `p2 + s2 d2`; along `d1` alone when they run parallel
/// (`|a b| 1e-4` at least the determinant squared).
fn nearest(d1: V3, p1: V3, d2: V3, p2: V3) -> (f32, f32) {
    let wv = sub(p2, p1);
    let a = dot(d1, d1);
    let b2 = dot(d2, d2);
    let b = -dot(d1, d2);
    let dw = dot(d1, wv);
    let det = (w(b2) * w(a) - w(b) * w(b)) as f32;
    let ab = ((w(b) * w(a)) as f32).abs();
    if w((w(ab) * PARALLEL) as f32) < w(det) * w(det) {
        let f = -dot(d2, wv);
        let inv = (1.0 / w(det)) as f32;
        let s1 = ((w(f) * w(b) - w(dw) * w(b2)) * w(inv)) as f32;
        let s2 = ((w(b) * w(dw) - w(f) * w(a)) * w(inv)) as f32;
        (s1, s2)
    } else {
        ((-w(dw) / w(a)) as f32, 0.0)
    }
}

/// 0x5a0440: the chord where the cap round `cp` (normal `a`, radius `r`)
/// crosses the plane `(wv, d)`, its rim's nearest point past the plane.
fn chord(a: V3, (wv, d): Plane, cp: V3, r: f32) -> Option<[V3; 2]> {
    let s = dot(wv, a);
    let (a2, s) = if s <= 0.0 || s.is_nan() {
        (a, s)
    } else {
        (neg(a), -s)
    };
    let v = mad(a2, s, neg(wv));
    let l2 = dot(v, v);
    if w(l2) < CAP_MIN {
        return None;
    }
    let inv = (1.0 / w(w(l2).sqrt() as f32)) as f32;
    let v = v.map(|x| (w(x) * w(inv)) as f32);
    let rim = mad(v, r, cp);
    let dc = dist(cp, (wv, d));
    let dr = dist(rim, (wv, d));
    let t = cross(v, a2);
    if !(dr < dc) {
        return None;
    }
    let x = ((w(dc) * w(r)) / (w(dc) - w(dr))) as f32;
    let rr = (w(r) * w(r)) as f32;
    let xx = (w(x) * w(x)) as f32;
    let disc = (w(rr) - w(xx)) as f32;
    if !(disc > 0.0) {
        return None;
    }
    let root = w(disc).sqrt() as f32;
    let mid = mad(v, x, cp);
    let nroot = -root;
    Some([
        std::array::from_fn(|k| (w(t[k]) * w(root) + w(mid[k])) as f32),
        std::array::from_fn(|k| (w(mid[k]) + w(t[k]) * w(nroot)) as f32),
    ])
}

/// 0x5a0740: the cap nearer the face cut by the plane through the edge
/// `p0`..`p1` and the best axis (0x5a0440's chord), the chord cut to the
/// edge's span; each end not above the corner along the best axis a
/// contact that deep.
fn rim_arm(cy: &Cylinder, p0: V3, p1: V3, best: &Best, surface: u32, out: &mut Out) {
    let a = cy.a();
    let b = best.axis;
    let (u, len) = normalize(sub(p1, p0));
    if len < AXIS_MIN {
        return;
    }
    let (wv, _) = normalize(cross(u, b));
    let pd = dot(p0, wv);
    let h = if dot(a, b) < 0.0 {
        -cy.half_height
    } else {
        cy.half_height
    };
    let cp = mad(a, h, cy.centre);
    let Some([mut q, mut p]) = chord(a, (wv, pd), cp, cy.radius) else {
        return;
    };
    let k1 = dot(p0, b);
    if !clip_seg((u, dot(u, p1)), &mut q, &mut p) {
        return;
    }
    let nu = neg(u);
    if !clip_seg((nu, dot(p0, nu)), &mut q, &mut p) {
        return;
    }
    for x in [q, p] {
        let depth = (w(dot(x, b)) - w(k1)) as f32;
        if !(depth < 0.0) {
            out.add(x, b, depth, surface);
        }
    }
}

/// 0x5a0f70: one face of plane `pl` and winding `poly`: nothing unless
/// the centre or the body lies in front of it and no axis separates;
/// then the arm of the best axis: the normal's side line (0x5a02b0) or
/// cap (0x5a09a0) as `|a . axis|` is under 0.9 or not, the axis's cut
/// (0x59f460), an edge's nearest points (0x5a0ef0), a corner (0x5973e0)
/// or a rim against an edge (0x5a0740).
fn polygon(cy: &Cylinder, pl: Plane, poly: &[V3], surface: u32, out: &mut Out) {
    let front = |p: V3| {
        let x = dist(p, pl);
        x > 0.0 || x.is_nan()
    };
    if !front(cy.centre) && !front(cy.body) {
        return;
    }
    let mut best = Best {
        depth: f32::MAX,
        code: 0,
        axis: [0.0; 3],
    };
    if !sat(cy, pl, poly, &mut best) {
        return;
    }
    let k = poly.len();
    match best.code {
        1 => {
            if w(dot(cy.a(), best.axis).abs()) < CAP_DOT {
                side(cy, pl, poly, &best, surface, out);
            } else {
                cap(cy, pl, poly, surface, out);
            }
        }
        2 => axis_arm(cy, pl, poly, surface, out),
        c => {
            let j = c - 3;
            if j < k {
                // 0x5a0ef0: the edge's point nearest the axis.
                let e = sub(poly[(j + 1) % k], poly[j]);
                let (_, s2) = nearest(cy.a(), poly[j], e, cy.centre);
                out.add(mad(e, s2, poly[j]), best.axis, best.depth, surface);
            } else if j - k < k {
                out.add(poly[j - k], best.axis, best.depth, surface);
            } else {
                let i = (j - 2 * k) >> 1;
                rim_arm(cy, poly[i], poly[(i + 1) % k], &best, surface, out);
            }
        }
    }
}

/// 0x5a1330: no contact once a face's depth (0x5a11f0) reads 0 or more;
/// else the deepest face with edges is the reference and goes to
/// 0x5a0f70; a narrow cylinder also takes each other axial face whose side
/// of the bounds the centre lies past, and each other side whose plane it
/// lies in front of.
pub fn brush(out: &mut Vec<ContactGeom>, max: usize, cy: &Cylinder, f: &BrushFaces) {
    let out = &mut Out { v: out, max };
    let (mut best, mut r) = (-f32::MAX, None);
    for (i, &p) in f.planes.iter().enumerate() {
        let d = face_depth(cy, p);
        if d >= 0.0 {
            return;
        }
        if !f.adjacent[i].is_empty() && best < d {
            (best, r) = (d, Some(i));
        }
    }
    let Some(r) = r else { return };
    let face = |out: &mut Out, i: usize, need: bool| {
        let poly = f.winding(i, MAX_WINDING);
        if !need || !poly.is_empty() {
            polygon(cy, f.planes[i], &poly, f.surface[i], out);
        }
    };
    face(out, r, false);
    if !cy.narrow {
        return;
    }
    for k in 0..3 {
        let (lo, hi) = (-f.planes[2 * k].1, f.planes[2 * k + 1].1);
        let c = cy.centre[k];
        if !(lo <= c) {
            if 2 * k != r {
                face(out, 2 * k, true);
            }
        } else if hi < c && 2 * k + 1 != r {
            face(out, 2 * k + 1, true);
        }
    }
    for i in 6..f.planes.len() {
        let (n, d) = f.planes[i];
        if i != r && d < dot(cy.centre, n) {
            face(out, i, true);
        }
    }
}

/// 0x596c70's plane of a partition triangle `t`: `(t0 - t1) x (t2 - t0)`
/// normalized (0x4076d0) and its distance at `t0` (0x5a18c0). The operand
/// order sets the sign of a zero part.
fn tri_plane(t: &[V3; 3]) -> Plane {
    let (n, _) = normalize(cross(sub(t[0], t[1]), sub(t[2], t[0])));
    (n, dot(t[0], n))
}

/// 0x5a17c0: each triangle of the partition, `(t0, t1, t2)` with its
/// plane, through 0x5a0f70.
pub fn tris(
    out: &mut Vec<ContactGeom>,
    max: usize,
    cy: &Cylinder,
    tris: &[[Vec3; 3]],
    surface: u32,
) {
    let out = &mut Out { v: out, max };
    for t in tris {
        let t = t.map(|v| v.to_array());
        polygon(cy, tri_plane(&t), &t, surface, out);
    }
}

/// The class 13 geom's collide against the world (0x5a95a0 into
/// 0x5a8110, room for `max`): the walk round [`Cylinder::bounds`] under
/// `mask`, a brush only while there is room (0x5a79b0), each normal
/// turned round for the reversed pair, so it points out of the world into
/// the cylinder.
pub fn collide_world(world: &dyn Pieces, cy: &Cylinder, mask: u32, max: usize) -> Vec<ContactGeom> {
    let (lo, hi) = cy.bounds();
    let mut out = Vec::new();
    for piece in world.pieces(Vec3::from(cy.centre), lo, hi, mask) {
        match piece {
            Piece::Brush(b) => {
                if out.len() < max {
                    brush(&mut out, max, cy, &world.brush(b));
                }
            }
            Piece::Tris { tris: t, surface } => tris(&mut out, max, cy, &t, surface),
        }
    }
    for c in &mut out {
        c.normal = neg(c.normal);
    }
    out
}
