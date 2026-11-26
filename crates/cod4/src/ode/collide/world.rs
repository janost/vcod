//! A capsule geom (class 14) against the clip map, IW's colliders in
//! iw3mp.exe 1.7: the world collider's capsule arm (0x5a8721) and walk
//! (0x5a8110), a brush (0x59ea10) and a partition's triangles (0x59ed10);
//! cod4-player-anims.md 10.6.

use glam::Vec3;

use super::ContactGeom;
use super::capsule::{self, Local, Seg, Tri};
use crate::collision::{BrushFaces, ClipMap, Piece};
use crate::ode::math::{V3, cross, dot, neg, sub};

#[inline]
fn w(x: f32) -> f64 {
    x as f64
}

/// A capsule geom as the world collider's context holds it: the geom's
/// position (context +0x38), the rotation column the geom names
/// (context +4, the column at +0x44 + 12 (k - 1)), the radius (+0x8c)
/// and the half length of its segment (+0x94).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Capsule {
    pub centre: V3,
    pub axis: V3,
    pub radius: f32,
    pub half_len: f32,
}

impl Capsule {
    fn seg(&self) -> Seg {
        Seg::new(self.centre, self.axis, self.radius, self.half_len)
    }

    /// 0x5a8721..0x5a87d6 and 0x5a8370: the box the walk takes, a cube of
    /// the larger of `r` and `2 r + h` (the double 2.0 at 0x70ae50) grown
    /// by 1 round the centre.
    pub fn bounds(&self) -> (Vec3, Vec3) {
        let (r, h) = (self.radius, self.half_len);
        let e = (w(r) * 2.0 + w(h)) as f32;
        let ext = if ((w(r) - w(e)) as f32) < 0.0 { e } else { r };
        let e1 = w(ext) + 1.0;
        let c = self.centre;
        (
            Vec3::from(c.map(|x| (w(x) - e1) as f32)),
            Vec3::from(c.map(|x| (e1 + w(x)) as f32)),
        )
    }
}

/// 0x59e940: the lesser of the segment's ends' heights over the plane
/// less the radius (`n . e - d - r`); `-FLT_MAX` when the centre lies
/// behind it.
fn plane_depth((n, d): ([f32; 3], f32), s: &Seg) -> f32 {
    let sc = (w(dot(s.c, n)) - w(d)) as f32;
    if sc < 0.0 {
        return -f32::MAX;
    }
    let e = |p: V3| ((w(dot(p, n)) - w(d)) - w(s.r)) as f32;
    let (e0, e1) = (e(s.p0), e(s.p1));
    if ((w(e1) - w(e0)) as f32) < 0.0 {
        e1
    } else {
        e0
    }
}

/// 0x59e790: the segment crosses the plane, or its nearer end lies
/// within the radius of it.
fn touches((n, d): ([f32; 3], f32), s: &Seg) -> bool {
    let a0 = (w(dot(s.p0, n)) - w(d)) as f32;
    let a1 = (w(dot(s.p1, n)) - w(d)) as f32;
    if w(a1) * w(a0) < 0.0 {
        return true;
    }
    let m = if ((w(a1.abs()) - w(a0.abs())) as f32) < 0.0 {
        a1.abs()
    } else {
        a0.abs()
    };
    s.r > m
}

/// The most corners 0x59ea10 lets 0x5998e0 build (the 0x100 it pushes).
const MAX_WINDING: usize = 0x100;

/// 0x59ea10: no contact once a face's depth (0x59e940) reads 0 or more;
/// else the deepest face with edges whose plane the centre lies in front
/// of is the reference, and when the capsule touches it (0x59e790) its
/// winding (0x5998e0, clockwise from outside) goes to the triangle
/// collider as `(w0, w2, w1)` when it has three corners, else as a fan:
/// `(w0, w2, w1)` and `(w[n-3], w[n-1], w[n-2])` as ends, `(w0, w[k],
/// w[k-1])` for `k` from 3 to `n - 2` between. The last end starts at
/// `w[n-3]`, not `w0` (0x59ecbc: `eax` is `&w[n-3]`).
pub fn brush(out: &mut Vec<ContactGeom>, max: usize, s: &Seg, f: &BrushFaces) {
    let mut locals: Vec<Local> = Vec::new();
    let mut best = -f32::MAX;
    let mut r = None;
    for (i, &p) in f.planes.iter().enumerate() {
        let d = plane_depth(p, s);
        if d >= 0.0 {
            return;
        }
        if !f.adjacent[i].is_empty() && best < d {
            best = d;
            r = Some(i);
        }
    }
    let Some(r) = r else { return };
    let plane = f.planes[r];
    if !touches(plane, s) {
        return;
    }
    let wv = f.winding(r, MAX_WINDING);
    let n = wv.len();
    if n == 0 {
        return;
    }
    let surf = f.surface[r];
    let tri = |locals: &mut Vec<Local>, kind, v: [V3; 3]| {
        capsule::triangle(kind, locals, s, plane, v, surf);
    };
    if n == 3 {
        tri(&mut locals, Tri::All, [wv[0], wv[2], wv[1]]);
    } else {
        tri(&mut locals, Tri::End, [wv[0], wv[2], wv[1]]);
        for k in 3..n - 1 {
            tri(&mut locals, Tri::Mid, [wv[0], wv[k], wv[k - 1]]);
        }
        tri(&mut locals, Tri::End, [wv[n - 3], wv[n - 1], wv[n - 2]]);
    }
    capsule::process(&locals, out, max);
}

/// 0x596c70's plane of a partition triangle `t`: `(t2 - t0) x (t1 - t0)`
/// normalized (0x4076d0, a zero vector left zero) and its distance at
/// `t0` (0x5a18c0). Its cull, a sphere of the box's half diagonal round
/// the centre against the triangle, drops no triangle a capsule inside
/// that sphere can touch, so it is not ported.
fn tri_plane(t: &[V3; 3]) -> ([f32; 3], f32) {
    let n = normalize(cross(sub(t[2], t[0]), sub(t[1], t[0])));
    (n, dot(t[0], n))
}

/// 0x4076d0: divides by the stored length, 1 when it is not over 0.
pub(super) fn normalize(v: V3) -> V3 {
    let l2 = ((w(v[0]) * w(v[0]) + w(v[1]) * w(v[1])) + w(v[2]) * w(v[2])) as f32;
    let mut l = w(l2).sqrt() as f32;
    if -l >= 0.0 {
        l = 1.0;
    }
    let inv = (1.0 / w(l)) as f32;
    v.map(|x| (w(x) * w(inv)) as f32)
}

/// 0x59ed10: nothing when `out` is full; else each triangle the capsule
/// touches the plane of (0x59e790) goes to 0x59dd90 as `(t0, t2, t1)`,
/// their contacts through one 0x59c710.
pub fn tris(out: &mut Vec<ContactGeom>, max: usize, s: &Seg, tris: &[[Vec3; 3]], surface: u32) {
    if out.len() >= max {
        return;
    }
    let mut locals: Vec<Local> = Vec::new();
    for t in tris {
        let t = t.map(|v| v.to_array());
        let plane = tri_plane(&t);
        if touches(plane, s) {
            capsule::triangle(Tri::All, &mut locals, s, plane, [t[0], t[2], t[1]], surface);
        }
    }
    capsule::process(&locals, out, max);
}

/// The world a capsule meets: what 0x5a8110 walks.
pub trait Pieces {
    /// [`ClipMap::world_pieces`].
    fn pieces(&self, centre: Vec3, lo: Vec3, hi: Vec3, mask: u32) -> Vec<Piece>;
    /// [`ClipMap::brush_faces`].
    fn brush(&self, b: usize) -> BrushFaces;
}

impl Pieces for ClipMap {
    fn pieces(&self, centre: Vec3, lo: Vec3, hi: Vec3, mask: u32) -> Vec<Piece> {
        self.world_pieces(centre, lo, hi, mask)
    }

    fn brush(&self, b: usize) -> BrushFaces {
        self.brush_faces(b)
    }
}

/// `dCollide(o1 = the capsule's geom, o2 = the world)` (0x5a95a0) as the
/// near callback (0x5a5f20) calls it with room for `max`: 0x5a8110 walks
/// the world pieces round [`Capsule::bounds`] under `mask` (0x5a882c), a
/// brush only while there is room (0x5a79b0), and the reversed collider's
/// normals are turned round (0x5a9651..), so each points out of the world
/// into the capsule.
pub fn collide_world(world: &dyn Pieces, cap: &Capsule, mask: u32, max: usize) -> Vec<ContactGeom> {
    let s = cap.seg();
    let (lo, hi) = cap.bounds();
    let mut out = Vec::new();
    for piece in world.pieces(Vec3::from(cap.centre), lo, hi, mask) {
        match piece {
            Piece::Brush(b) => {
                if out.len() < max {
                    brush(&mut out, max, &s, &world.brush(b));
                }
            }
            Piece::Tris { tris: t, surface } => tris(&mut out, max, &s, &t, surface),
        }
    }
    for c in &mut out {
        c.normal = neg(c.normal);
    }
    out
}
