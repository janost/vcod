//! CoD4 1.7's indexed collision triangles: three vertex indices and no
//! stored plane each, met by a point or a capsule sweep and by the
//! capsule's position test, with a walkable bit per edge
//! (`docs/research/cod4-collision.md` 3). The point and capsule sweeps keep
//! 1.7's float model (what it stores, what stays on the x87 stack), the
//! server's or the client's (`TraceFloats`); the server's border corner is
//! plain `f32`, and both position tests are `pos4`.

// A test written `!(a < b)` keeps the branch a NaN takes.
#![allow(clippy::neg_cmp_op_on_partial_ord)]

use glam::{DVec3, Vec3};

use super::trace::{CLIP_EPS, Cx};
use super::{ClipTrace, CollBorder, CollPartition, IndexedMesh, TraceFloats};

/// Below this share of `b * b` a corner's discriminant is a graze and
/// the corner is passed (doc 3).
const GRAZE: f32 = 0.001;

/// A contact one of the sphere kernels found, for the trace to take.
struct Contact {
    fraction: f32,
    normal: Vec3,
    /// What `startsolid` becomes; `None` leaves it.
    startsolid: Option<bool>,
}

impl Contact {
    /// Writes the contact into `tr`; `walkable` as for `startsolid`.
    fn apply(self, tr: &mut ClipTrace, walkable: Option<bool>) {
        tr.fraction = self.fraction;
        tr.normal = self.normal;
        if let Some(s) = self.startsolid {
            tr.startsolid = s;
        }
        if let Some(w) = walkable {
            tr.walkable = w;
        }
    }
}

/// Where a sphere swept at an edge's line meets it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EdgeSweep {
    /// Not before the trace's fraction, or moving away.
    Clear,
    /// Between the ends: the strike is written into the trace.
    Struck,
    /// On the line past end 0 or end 1: only that corner is left to meet.
    Beyond(usize),
}

fn corners(m: &IndexedMesh, i: usize) -> [Vec3; 3] {
    m.tris[i].map(|v| m.verts[usize::from(v)])
}

/// A value 1.7 stores to a float and loads back.
fn stored(x: f64) -> f64 {
    f64::from(x as f32)
}

impl Cx<'_> {
    /// One indexed partition under the AABB tree walk: each triangle
    /// against the point or the capsule; a capsule moving sideways then
    /// meets the partition's borders, end spheres included (doc 2, 3).
    pub(super) fn trace_indexed_partition(
        &mut self,
        m: &IndexedMesh,
        part: &CollPartition,
        tr: &mut ClipTrace,
    ) {
        let first = part.first_tri as usize;
        let client = self.sw.floats == TraceFloats::Cod4Client;
        for i in first..first + usize::from(part.tri_count) {
            match (self.sw.point, client) {
                (true, false) => self.point_tri4(m, i, tr),
                (true, true) => self.point_tri_client(m, i, tr),
                (false, false) => self.capsule_tri4(m, i, tr),
                (false, true) => self.capsule_tri_client(m, i, tr),
            }
        }
        let sideways = self.sw.sweep.x != 0.0 || self.sw.sweep.y != 0.0;
        if self.sw.point || !sideways || self.sw.sphere_z == 0.0 {
            return;
        }
        let map = self.map;
        let first = part.first_border as usize;
        for b in &map.borders[first..first + usize::from(part.border_count)] {
            if client {
                self.capsule_border_client(b, tr);
            } else {
                self.capsule_border(b, true, tr);
            }
        }
    }

    /// The point sweep against triangle `i` (`CM_TracePointThroughTriangle`,
    /// inlined in 1.7's tree walk at `cod4_lnxded` 0x81169e3..0x81179c8):
    /// a front-to-back crossing inside the edges, with no clip epsilon.
    /// Which differences and products are stored floats, and the
    /// `Vec3NormalizeTo` (0x8192a64) normal, are doc 3's.
    fn point_tri4(&self, m: &IndexedMesh, i: usize, tr: &mut ClipTrace) {
        if let Some((normal, fraction)) = self.point_crossing(corners(m, i), tr.fraction) {
            tr.walkable = false;
            tr.normal = normal;
            tr.fraction = fraction;
        }
    }

    fn point_crossing(&self, tri: [Vec3; 3], limit: f32) -> Option<(Vec3, f32)> {
        let [a, b, c] = tri.map(Vec3::as_dvec3);
        let (ba, ca) = (a - b, a - c);
        let n = DVec3::new(
            ba.z * ca.y - stored(ca.z) * ba.y,
            stored(ba.x) * stored(ca.z) - stored(ba.z) * ca.x,
            ca.x * ba.y - stored(ba.x) * stored(ca.y),
        );
        let sweep = self.sw.sweep.as_dvec3();
        let closing = sweep.dot(n);
        if !(closing < 0.0) {
            return None;
        }
        let rel = a - self.sw.start.as_dvec3();
        let rel = DVec3::new(stored(rel.x), stored(rel.y), rel.z);
        let depth = rel.dot(n);
        if !(depth <= 0.0 && depth > f64::from(limit) * closing) {
            return None;
        }
        // The crossing's two edge coordinates, scaled by `closing`: inside
        // when each lies between it and 0 and so does their difference.
        let moment = sweep.cross(rel);
        let (s, t) = (moment.dot(ba), moment.dot(ca));
        let inside = s <= 0.0 && closing <= s && t >= 0.0 && closing <= s - t;
        inside.then(|| {
            let normal = crate::x87::normalize_to(n.as_vec3());
            (normal, (stored(depth) / stored(closing)) as f32)
        })
    }

    /// The capsule against triangle `i` (`CM_TraceCapsuleThroughTriangle`,
    /// inlined in 1.7's tree walk at `cod4_lnxded` 0x8116cd1..0x8117aa6):
    /// its sphere nearer the face against the face 0.125 out; a contact
    /// outside an edge sweeps the sphere at that edge (`v0 v1`, `v0 v2`,
    /// `v1 v2`), and past an edge's end at that corner. An edge contact
    /// takes the edge's walkable bit (`3 i + the opposite corner`), a
    /// corner the AND of the edges swept past. Which values are stored
    /// floats and which stay on the x87 stack (an `f64` here) is the
    /// binary's (doc 3).
    fn capsule_tri4(&self, m: &IndexedMesh, i: usize, tr: &mut ClipTrace) {
        let sw = &self.sw;
        let [a, b, c] = corners(m, i);
        let [a64, c64] = [a, c].map(Vec3::as_dvec3);
        let e01 = a - b;
        let e02k = a64 - c64;
        let e02 = a - c;
        let nzk = e02k.x * f64::from(e01.y) - e02k.y * f64::from(e01.x);
        let n = Vec3::new(
            (f64::from(e01.z) * e02k.y - f64::from(e01.y) * e02k.z) as f32,
            (e02k.z * f64::from(e01.x) - f64::from(e01.z) * e02k.x) as f32,
            nzk as f32,
        );
        let sweep = sw.sweep.as_dvec3();
        let closing_k =
            (sweep.y * f64::from(n.y) + stored(sweep.x * f64::from(n.x))) + nzk * sweep.z;
        if closing_k >= 0.0 {
            return;
        }
        let closing = stored(closing_k);
        let (normal, len) = crate::x87::normalize_to_len(n);
        let area2 = stored(len);
        // The lower sphere, or the upper under a face turned down.
        let off = if normal.z >= 0.0 {
            sw.sphere_z
        } else {
            -sw.sphere_z
        };
        let start = sw.start.as_dvec3();
        let sphere_z = start.z - f64::from(off);
        let sphere = Vec3::new(sw.start.x, sw.start.y, sphere_z as f32);
        let nk = normal.as_dvec3();
        let r = f64::from(sw.radius);
        let lead = DVec3::new(
            (-r * nk.x + start.x) - a64.x,
            (nk.y * -r + start.y) - a64.y,
            (-r * nk.z + sphere_z) - a64.z,
        );
        let gap = (nk.x * lead.x + nk.y * lead.y) + nk.z * lead.z;
        let (fraction, solid, foot) = if 0.0 > gap {
            // Already through the face's plane: solid when the centre is
            // within a radius of it, measured from its foot on the plane.
            let rel = DVec3::new(start.x - a64.x, start.y - a64.y, sphere_z - a64.z);
            let d = (nk.x * rel.x + nk.y * rel.y) + nk.z * rel.z;
            if d * d >= r * r {
                return;
            }
            let foot = DVec3::new(rel.x + -d * nk.x, rel.y + nk.y * -d, rel.z + -d * nk.z);
            (0.0, true, foot)
        } else {
            let g = -stored(gap - f64::from(CLIP_EPS));
            let f = ((area2 * g) / closing) as f32;
            if f >= tr.fraction {
                return;
            }
            (f, false, lead)
        };
        let mut moment = DVec3::new(
            foot.z * sweep.y - sweep.z * foot.y,
            sweep.z * foot.x - foot.z * sweep.x,
            sweep.x * foot.y - sweep.y * foot.x,
        );
        let e01k = e01.as_dvec3();
        let s_k = e01k.z * moment.z + stored(e01k.y * moment.y + stored(e01k.x * moment.x));
        let s = stored(s_k);
        let mut outside = false;
        let mut walk = true;
        let mut corner = None;
        // `None` when the edge is struck (the triangle is done), else the
        // corner past an end, with the edge's walkable bit.
        let edge = |from, along, ends: [Vec3; 2], opposite, tr: &mut ClipTrace| match self
            .sweep_edge(sphere, from, along, tr)
        {
            EdgeSweep::Struck => {
                tr.walkable = m.edge_walkable(3 * i + opposite);
                None
            }
            EdgeSweep::Clear => Some(None),
            EdgeSweep::Beyond(end) => Some(Some((ends[end], m.edge_walkable(3 * i + opposite)))),
        };
        if 0.0 > s_k {
            // The moment goes through a float across the edge's call.
            moment = DVec3::new(stored(moment.x), stored(moment.y), stored(moment.z));
            let Some(beyond) = edge(a, e01, [a, b], 2, tr) else {
                return;
            };
            if let Some((v, bit)) = beyond {
                corner = Some(v);
                walk = bit;
            }
            outside = true;
        }
        let e02s = e02.as_dvec3();
        let t_k = (moment.x * e02s.x + moment.y * e02s.y) + moment.z * e02s.z;
        let mut t = t_k;
        if t_k > 0.0 {
            t = stored(t_k);
            let Some(beyond) = edge(a, e02, [a, c], 1, tr) else {
                return;
            };
            if let Some((v, bit)) = beyond {
                corner = Some(v);
                walk &= bit;
            }
            outside = true;
        }
        if closing > t - s {
            let Some(beyond) = edge(b, b - c, [b, c], 0, tr) else {
                return;
            };
            if let Some((v, bit)) = beyond {
                corner = Some(v);
                walk &= bit;
            }
        } else if !outside {
            tr.normal = normal;
            tr.walkable = false;
            tr.fraction = if -fraction < 0.0 { fraction } else { 0.0 };
            tr.startsolid = solid;
            return;
        }
        if let Some(v) = corner {
            self.vertex4(sphere, v, walk, tr);
        }
    }

    /// The 1.7 client's capsule against triangle `i` (`iw3mp.exe`
    /// 0x4f05c0): [`Self::capsule_tri4`]'s tests in its order, but the
    /// edges, the normal, the closing speed, the sphere's centre, its lead
    /// point, the gap, the foot, the moment and both edge coordinates each
    /// stored, the normal through 0x55f6e0's stored length, and the client's
    /// edge and corner kernels (doc 3).
    fn capsule_tri_client(&self, m: &IndexedMesh, i: usize, tr: &mut ClipTrace) {
        let sw = &self.sw;
        let k = f64::from;
        let [a, b, c] = corners(m, i);
        let (e01, e02) = (a - b, a - c);
        let n = Vec3::new(
            (k(e02.y) * k(e01.z) - k(e02.z) * k(e01.y)) as f32,
            (k(e01.x) * k(e02.z) - k(e01.z) * k(e02.x)) as f32,
            (k(e02.x) * k(e01.y) - k(e01.x) * k(e02.y)) as f32,
        );
        let sweep = sw.sweep;
        let closing = ((k(sweep.y) * k(n.y) + k(n.x) * k(sweep.x)) + k(sweep.z) * k(n.z)) as f32;
        if closing >= 0.0 {
            return;
        }
        let (normal, area2) = crate::x87::normalize_stored_len(n);
        let off = if normal.z >= 0.0 {
            sw.sphere_z
        } else {
            -sw.sphere_z
        };
        let sphere = Vec3::new(sw.start.x, sw.start.y, sw.start.z - off);
        let r = -sw.radius;
        let lead = Vec3::new(
            (k(normal.x) * k(r) + k(sphere.x)) as f32,
            (k(normal.y) * k(r) + k(sphere.y)) as f32,
            (k(r) * k(normal.z) + k(sphere.z)) as f32,
        ) - a;
        // `(y + x) + z`, as the dots here sum.
        let dot = |p: Vec3, q: Vec3| ((k(p.y) * k(q.y) + k(p.x) * k(q.x)) + k(p.z) * k(q.z)) as f32;
        let gap = dot(normal, lead);
        let (fraction, solid, foot) = if !(gap < 0.0) {
            let f = (-(((k(gap) - k(CLIP_EPS)) * k(area2)) / k(closing))) as f32;
            if tr.fraction <= f {
                return;
            }
            (f, false, lead)
        } else {
            let rel = sphere - a;
            let d = dot(rel, normal);
            if (k(sw.radius) * k(sw.radius)) as f32 as f64 <= k(d) * k(d) {
                return;
            }
            let nd = k(-d);
            let foot = Vec3::new(
                (k(rel.x) + nd * k(normal.x)) as f32,
                (k(rel.y) + k(normal.y) * nd) as f32,
                (k(rel.z) + nd * k(normal.z)) as f32,
            );
            (0.0, true, foot)
        };
        let moment = Vec3::new(
            (k(sweep.y) * k(foot.z) - k(foot.y) * k(sweep.z)) as f32,
            (k(foot.x) * k(sweep.z) - k(foot.z) * k(sweep.x)) as f32,
            (k(foot.y) * k(sweep.x) - k(foot.x) * k(sweep.y)) as f32,
        );
        let s = dot(moment, e01);
        let mut outside = false;
        let mut walk = true;
        let mut corner = None;
        let edge = |from, along, ends: [Vec3; 2], opposite, tr: &mut ClipTrace| match self
            .sweep_edge_client(sphere, from, along, tr)
        {
            EdgeSweep::Struck => {
                tr.walkable = m.edge_walkable(3 * i + opposite);
                None
            }
            EdgeSweep::Clear => Some(None),
            EdgeSweep::Beyond(end) => Some(Some((ends[end], m.edge_walkable(3 * i + opposite)))),
        };
        if 0.0 > s {
            outside = true;
            let Some(beyond) = edge(a, e01, [a, b], 2, tr) else {
                return;
            };
            if let Some((v, bit)) = beyond {
                corner = Some(v);
                walk = bit;
            }
        }
        let t = dot(moment, e02);
        if 0.0 < t {
            outside = true;
            let Some(beyond) = edge(a, e02, [a, c], 1, tr) else {
                return;
            };
            if let Some((v, bit)) = beyond {
                corner = Some(v);
                walk &= bit;
            }
        }
        if k(closing) > k(t) - k(s) {
            let Some(beyond) = edge(b, b - c, [b, c], 0, tr) else {
                return;
            };
            if let Some((v, bit)) = beyond {
                corner = Some(v);
                walk &= bit;
            }
        } else if !outside {
            tr.normal = normal;
            tr.walkable = false;
            tr.fraction = if -fraction < 0.0 { fraction } else { 0.0 };
            tr.startsolid = solid;
            return;
        }
        if let Some(v) = corner {
            self.vertex_client(sphere, v, walk, tr);
        }
    }

    /// The 1.7 client's point sweep against triangle `i` (`iw3mp.exe`
    /// 0x4efce0): [`Self::point_tri4`]'s tests with the edges, the normal,
    /// the closing speed, the offset, the depth, the moment and both edge
    /// coordinates each stored, the normal through 0x55f6e0 (doc 3).
    fn point_tri_client(&self, m: &IndexedMesh, i: usize, tr: &mut ClipTrace) {
        let sw = &self.sw;
        let k = f64::from;
        let [a, b, c] = corners(m, i);
        let (e01, e02) = (a - b, a - c);
        let n = Vec3::new(
            (k(e02.y) * k(e01.z) - k(e02.z) * k(e01.y)) as f32,
            (k(e02.z) * k(e01.x) - k(e01.z) * k(e02.x)) as f32,
            (k(e01.y) * k(e02.x) - k(e02.y) * k(e01.x)) as f32,
        );
        let dot = |p: Vec3, q: Vec3| super::trace::dot(p, q) as f32;
        let closing = dot(sw.sweep, n);
        // Each test below lets a NaN through, as the binary's flags do.
        if closing >= 0.0 {
            return;
        }
        let rel = a - sw.start;
        let depth = dot(n, rel);
        if depth > 0.0 || k(tr.fraction) * k(closing) >= k(depth) {
            return;
        }
        let sweep = sw.sweep;
        let moment = Vec3::new(
            (k(sweep.y) * k(rel.z) - k(sweep.z) * k(rel.y)) as f32,
            (k(sweep.z) * k(rel.x) - k(sweep.x) * k(rel.z)) as f32,
            (k(sweep.x) * k(rel.y) - k(sweep.y) * k(rel.x)) as f32,
        );
        let s = dot(moment, e01);
        if s > 0.0 || closing > s {
            return;
        }
        let t = dot(moment, e02);
        if t < 0.0 || k(s) - k(t) < k(closing) {
            return;
        }
        tr.walkable = false;
        tr.normal = crate::x87::normalize_stored_len(n).0;
        tr.fraction = (k(depth) / k(closing)) as f32;
    }

    /// The 1.7 client's `CM_TraceSphereThroughEdge` (`iw3mp.exe`
    /// 0x4eff70): [`Self::sweep_edge`]'s tests in its order, with nearly
    /// every value stored and its own normals (doc 3).
    fn sweep_edge_client(
        &self,
        sphere: Vec3,
        from: Vec3,
        along: Vec3,
        tr: &mut ClipTrace,
    ) -> EdgeSweep {
        let sw = &self.sw;
        let k = f64::from;
        let (rel, al, sweep) = (sphere - from, along, sw.sweep);
        let across = Vec3::new(
            (k(sweep.z) * k(al.y) - k(sweep.y) * k(al.z)) as f32,
            (k(al.z) * k(sweep.x) - k(sweep.z) * k(al.x)) as f32,
            (k(sweep.y) * k(al.x) - k(al.y) * k(sweep.x)) as f32,
        );
        let dot = |p: Vec3, q: Vec3| super::trace::dot(p, q) as f32;
        let off = dot(across, rel);
        let across2 = dot(across, across);
        let reach = (k(sw.radius) + k(CLIP_EPS)) as f32;
        // The square of a stored float is exact in a double.
        let off2 = k(off) * k(off);
        let reach2 = (k(reach) * k(reach)) as f32;
        let room = (k(reach2) * k(across2) - off2) as f32;
        // Each test below lets a NaN through, as the binary's flags do.
        if room <= 0.0 {
            return EdgeSweep::Clear;
        }
        let len2 = dot(al, al);
        let root = crate::x87::sqrt_stored(k(room) * k(len2));
        let half = (k(root) / k(across2)) as f32;
        let w = Vec3::new(
            (k(rel.y) * k(al.z) - k(rel.z) * k(al.y)) as f32,
            (k(rel.z) * k(al.x) - k(rel.x) * k(al.z)) as f32,
            (k(al.y) * k(rel.x) - k(al.x) * k(rel.y)) as f32,
        );
        let mid_scaled = dot(across, w);
        let mid = (k(mid_scaled) / k(across2)) as f32;
        if 0.0 > (k(half) + k(mid)) as f32 {
            return EdgeSweep::Clear;
        }
        let enter = (k(mid) - k(half)) as f32;
        if tr.fraction <= enter {
            return EdgeSweep::Clear;
        }
        let inside = enter < 0.0;
        let at = if inside {
            rel
        } else {
            crate::x87::ma_stored(rel, enter, sweep)
        };
        let s = -dot(al, at);
        if s <= 0.0 {
            return EdgeSweep::Beyond(0);
        }
        if len2 <= s {
            return EdgeSweep::Beyond(1);
        }
        let out = crate::x87::ma_stored(at, (k(s) / k(len2)) as f32, al);
        if inside {
            if dot(sweep, out) >= 0.0 {
                return EdgeSweep::Clear;
            }
            tr.normal = crate::x87::normalize_stored_len(out).0;
            tr.fraction = 0.0;
            let r = k(sw.radius);
            let inner = ((r * r) * k(across2) - off2) as f32;
            tr.startsolid = k(inner) * k(len2) > k(mid_scaled) * k(mid_scaled);
            return EdgeSweep::Struck;
        }
        let inv = (1.0 / k(reach)) as f32;
        tr.normal = Vec3::new(
            (k(inv) * k(out.x)) as f32,
            (k(out.y) * k(inv)) as f32,
            (k(inv) * k(out.z)) as f32,
        );
        tr.fraction = enter;
        EdgeSweep::Struck
    }

    /// The 1.7 client's `CM_TraceSphereThroughVertex` (`iw3mp.exe`
    /// 0x4f0370), which its triangles and its borders call:
    /// [`Self::vertex4`]'s tests with the offset, closing speed, squared
    /// distance, gap, discriminant, fraction and each normal step stored
    /// (doc 3). `walkable` takes `walk`.
    fn vertex_client(&self, sphere: Vec3, v: Vec3, walk: bool, tr: &mut ClipTrace) {
        let sw = &self.sw;
        let k = f64::from;
        let rel = sphere - v;
        let dot = |p: Vec3, q: Vec3| super::trace::dot(p, q) as f32;
        let closing = dot(rel, sw.sweep);
        // Each test below lets a NaN through, as the binary's flags do.
        if closing >= 0.0 {
            return;
        }
        let dist2 = dot(rel, rel);
        let reach = k(sw.radius) + k(CLIP_EPS);
        let gap = (k(dist2) - reach * reach) as f32;
        let r = k(sw.radius);
        if gap <= 0.0 {
            let inv = (1.0 / k(crate::x87::sqrt_stored(k(dist2)))) as f32;
            tr.normal = crate::x87::scale_stored(rel, inv);
            tr.walkable = walk;
            tr.fraction = 0.0;
            if (r * r) as f32 > dist2 {
                tr.startsolid = true;
            }
            return;
        }
        let a = k(sw.sweep_len2);
        let b2 = (k(closing) * k(closing)) as f32;
        let disc = (k(b2) - k(gap) * a) as f32;
        if k(b2) * k(GRAZE) > k(disc) {
            return;
        }
        let root = crate::x87::sqrt_stored(k(disc));
        let fraction = ((-k(root) - k(closing)) / a) as f32;
        if tr.fraction <= fraction {
            return;
        }
        let n = crate::x87::ma_stored(rel, fraction, sw.sweep);
        let n = crate::x87::scale_stored(n, (1.0 / reach) as f32);
        let h = ((3.0 - k(dot(n, n))) * 0.5) as f32;
        tr.normal = crate::x87::scale_stored(n, h);
        tr.walkable = walk;
        tr.fraction = fraction;
    }

    /// The 1.7 client's capsule against a border (`iw3mp.exe` 0x4f1ae0):
    /// [`Self::capsule_border`]'s tests with 1.7's end spheres, but the
    /// distance off the line, `r^2` and the wall top's height over the hit
    /// stored where the server keeps them, the top's height kept where the
    /// server stores it, and a corner contact clearing `walkable` (doc 3).
    fn capsule_border_client(&self, b: &CollBorder, tr: &mut ClipTrace) {
        let sw = &self.sw;
        let k = f64::from;
        let [nx, ny, dist] = b.dist_eq;
        let approach = (k(sw.sweep.y) * k(ny) + k(nx) * k(sw.sweep.x)) as f32;
        // Each test below lets a NaN through, as the binary's flags do.
        if approach >= 0.0 {
            return;
        }
        let reach = (k(sw.radius) + k(CLIP_EPS)) as f32;
        let d = (k(sw.start.x) * k(nx) + k(sw.start.y) * k(ny)) as f32;
        let d = (k(d) - k(dist)) as f32;
        let mut f = ((k(reach) - k(d)) / k(approach)) as f32;
        if tr.fraction <= f || -k(reach) > k(sw.sweep_len) * k(f) {
            return;
        }
        let mut hit = crate::x87::ma_stored(sw.start, f, sw.sweep);
        let mut along = ((k(ny) * k(hit.x) - k(nx) * k(hit.y)) - k(b.start)) as f32;
        // The end the sweep passed: its distance along the line, kept, its
        // `along` and its wall top's height less the start's.
        let end = if along < 0.0 {
            Some((k(b.start), 0.0, (k(b.z_base) - k(sw.start.z)) as f32))
        } else if b.length < along {
            let top = (k(b.z_slope) * k(b.length) + k(b.z_base)) as f32;
            let s = k(b.length) + k(b.start);
            Some((s, b.length, (k(sw.start.z) - k(top)) as f32))
        } else {
            None
        };
        if let Some((s, end_along, dz)) = end {
            let corner = [
                (k(dist) * k(nx) + k(ny) * s) as f32,
                (k(dist) * k(ny) - k(nx) * s) as f32,
            ];
            let v = [sw.start.x - corner[0], sw.start.y - corner[1]];
            let bb = (k(sw.sweep.x) * k(v[0]) + k(sw.sweep.y) * k(v[1])) as f32;
            if bb >= 0.0 {
                return;
            }
            let c0 = (k(v[0]) * k(v[0]) + k(v[1]) * k(v[1])) as f32;
            let c = (k(c0) - k(reach) * k(reach)) as f32;
            if c < 0.0 {
                if sw.sphere_z < dz.abs() {
                    return;
                }
                tr.normal = Vec3::new(nx, ny, 0.0);
                tr.fraction = 0.0;
                tr.walkable = false;
                if (k(sw.radius) * k(sw.radius)) as f32 > c0 {
                    tr.startsolid = true;
                }
                return;
            }
            let disc = (k(bb) * k(bb) - k(c) * k(sw.sweep_len2)) as f32;
            if disc < 0.0 {
                return;
            }
            let root = crate::x87::sqrt_stored(k(disc));
            f = ((k(-bb) - k(root)) / k(sw.sweep_len2)) as f32;
            if tr.fraction <= f || f <= 0.0 {
                return;
            }
            hit = crate::x87::ma_stored(sw.start, f, sw.sweep);
            along = end_along;
        } else if f < 0.0 {
            f = 0.0;
        }
        let above = ((k(b.z_slope) * k(along) + k(b.z_base)) - k(hit.z)) as f32;
        let sz = sw.sphere_z;
        if sz < above {
            if (k(sz) + k(sw.radius)) as f32 > above {
                self.border_sphere_client(b, sz, tr);
            }
        } else if !(-sz > above) {
            tr.walkable = false;
            tr.fraction = f;
            tr.normal = Vec3::new(nx, ny, 0.0);
        } else if ((k(-sz) - k(sw.radius)) as f32) < above {
            self.border_sphere_client(b, -sz, tr);
        }
    }

    /// The 1.7 client's `CM_TraceSphereThroughBorder` (`iw3mp.exe`
    /// 0x4f19f0): [`Self::border_sphere`] with the wall top's ends each
    /// rounded once, the client's edge kernel, then its corner kernel with
    /// `walk` clear (doc 3).
    fn border_sphere_client(&self, b: &CollBorder, dz: f32, tr: &mut ClipTrace) {
        let k = f64::from;
        let [nx, ny, dist] = b.dist_eq;
        let s1 = k(b.length) + k(b.start);
        let ends = [
            Vec3::new(
                (k(ny) * k(b.start) + k(nx) * k(dist)) as f32,
                (k(dist) * k(ny) - k(nx) * k(b.start)) as f32,
                b.z_base,
            ),
            Vec3::new(
                (k(nx) * k(dist) + k(ny) * s1) as f32,
                (k(dist) * k(ny) - k(nx) * s1) as f32,
                (k(b.z_slope) * k(b.length) + k(b.z_base)) as f32,
            ),
        ];
        let mut sphere = self.sw.start;
        sphere.z += dz;
        if let EdgeSweep::Beyond(end) =
            self.sweep_edge_client(sphere, ends[0], ends[0] - ends[1], tr)
        {
            self.vertex_client(sphere, ends[end], false, tr);
        }
    }

    /// `CM_TraceSphereThroughEdge` (1.7's 0x81150ae): the sphere at
    /// `sphere`, grown by the clip epsilon, swept at the line `from + s
    /// along`. Where it enters the cylinder round the line, then whether
    /// that point lies between the ends; a sphere that starts inside the
    /// cylinder and closes on the line meets it at fraction 0, solid when
    /// it is within its own radius. A strike is written into `tr` (its
    /// fraction, normal and, from inside, `startsolid`). Which values are
    /// stored floats is the binary's (doc 3).
    fn sweep_edge(&self, sphere: Vec3, from: Vec3, along: Vec3, tr: &mut ClipTrace) -> EdgeSweep {
        let sw = &self.sw;
        let rel = (sphere - from).as_dvec3();
        let al = along.as_dvec3();
        let sweep = sw.sweep.as_dvec3();
        let across = DVec3::new(
            al.y * sweep.z - al.z * sweep.y,
            al.z * sweep.x - sweep.z * al.x,
            sweep.y * al.x - sweep.x * al.y,
        );
        let off = (rel.x * across.x + rel.y * across.y) + rel.z * across.z;
        let across2 =
            stored(across.z * across.z + stored(across.y * across.y + stored(across.x * across.x)));
        let reach_k = f64::from(CLIP_EPS) + f64::from(sw.radius);
        let reach = stored(reach_k);
        let off2 = off * off;
        let room = reach_k * reach_k * across2 - off2;
        if 0.0 >= room {
            return EdgeSweep::Clear;
        }
        let len2_k = (al.x * al.x + al.y * al.y) + al.z * al.z;
        let len2 = stored(len2_k);
        let half = stored((room * len2_k).sqrt() / across2);
        let w = DVec3::new(
            rel.y * al.z - rel.z * al.y,
            rel.z * al.x - rel.x * al.z,
            rel.x * al.y - rel.y * al.x,
        );
        let mid_scaled = (across.y * w.y + across.x * w.x) + across.z * w.z;
        let mid = mid_scaled / across2;
        if 0.0 > half + mid {
            return EdgeSweep::Clear;
        }
        let enter = mid - half;
        if enter >= f64::from(tr.fraction) {
            return EdgeSweep::Clear;
        }
        if 0.0 > enter {
            // Inside the cylinder already.
            let s = -((rel.x * al.x + al.y * rel.y) + rel.z * al.z);
            if 0.0 >= s {
                return EdgeSweep::Beyond(0);
            }
            if s >= len2 {
                return EdgeSweep::Beyond(1);
            }
            let k = s / len2;
            let out = DVec3::new(al.x * k + rel.x, al.y * k + rel.y, al.z * k + rel.z);
            if (out.x * sweep.x + out.y * sweep.y) + out.z * sweep.z >= 0.0 {
                return EdgeSweep::Clear;
            }
            tr.normal = crate::x87::normalize_to(out.as_vec3());
            tr.fraction = 0.0;
            let inner = f64::from(sw.radius) * f64::from(sw.radius) * across2 - stored(off2);
            let ms = stored(mid_scaled);
            tr.startsolid = len2 * inner > ms * ms;
            return EdgeSweep::Struck;
        }
        let at = rel + enter * sweep;
        let s = -((al.x * at.x + al.y * at.y) + al.z * at.z);
        if 0.0 >= s {
            return EdgeSweep::Beyond(0);
        }
        if s >= len2 {
            return EdgeSweep::Beyond(1);
        }
        let k = s / len2;
        let out = at + al * k;
        let inv = 1.0 / reach;
        tr.normal = Vec3::new(
            (out.x * inv) as f32,
            (stored(out.y) * inv) as f32,
            (inv * stored(out.z)) as f32,
        );
        tr.fraction = enter as f32;
        EdgeSweep::Struck
    }

    /// `CM_TraceSphereThroughVertex` as the triangle arm inlines it
    /// (`cod4_lnxded` 0x8117354..0x8117aa6): the sphere at `sphere`, grown
    /// by the clip epsilon, swept at the corner `v`. The nearer root, its
    /// normal one Newton step of the reciprocal length from unit; a sphere
    /// already within reach meets it at fraction 0. `walkable` takes `walk`.
    fn vertex4(&self, sphere: Vec3, v: Vec3, walk: bool, tr: &mut ClipTrace) {
        let sw = &self.sw;
        let rel = sphere.as_dvec3() - v.as_dvec3();
        let sweep = sw.sweep.as_dvec3();
        let closing = (rel.x * sweep.x + rel.y * sweep.y) + rel.z * sweep.z;
        if closing >= 0.0 {
            return;
        }
        let dist2 = (rel.x * rel.x + rel.y * rel.y) + rel.z * rel.z;
        let reach = f64::from(sw.radius) + f64::from(CLIP_EPS);
        let gap = dist2 - reach * reach;
        if 0.0 >= gap {
            let inv = 1.0 / dist2.sqrt();
            tr.normal = (rel * inv).as_vec3();
            tr.walkable = walk;
            tr.fraction = 0.0;
            if f64::from(sw.radius) * f64::from(sw.radius) > dist2 {
                tr.startsolid = true;
            }
            return;
        }
        let a = f64::from(sw.sweep_len2);
        let b2 = closing * closing;
        let disc = b2 - gap * a;
        if b2 * f64::from(GRAZE) > disc {
            return;
        }
        let fraction = (-disc.sqrt() - closing) / a;
        if fraction >= f64::from(tr.fraction) {
            return;
        }
        let n = (rel + fraction * sweep).as_vec3().as_dvec3();
        let inv = 1.0 / (f64::from(CLIP_EPS) + f64::from(sw.radius));
        let k = n * inv;
        let h = (3.0 - ((k.x * k.x + k.y * k.y) + k.z * k.z)) * 0.5;
        tr.normal = Vec3::new(
            (k.x * h) as f32,
            (stored(k.y) * h) as f32,
            (h * stored(k.z)) as f32,
        );
        tr.walkable = walk;
        tr.fraction = fraction as f32;
    }

    /// The border's corner: the sphere at `sphere`, grown by the clip
    /// epsilon, swept at the point `v`, as [`Self::vertex4`] but in plain
    /// `f32` (1.7's border arm at 0x811633c inlines its own, not yet read).
    /// The nearer root, its normal one Newton step of the reciprocal
    /// length from unit; a sphere already within reach meets it at
    /// fraction 0.
    fn corner_contact(&self, sphere: Vec3, v: Vec3, limit: f32) -> Option<Contact> {
        let sw = &self.sw;
        let rel = sphere - v;
        let closing = sw.sweep.dot(rel);
        if !(closing < 0.0) {
            return None;
        }
        let dist2 = rel.dot(rel);
        let reach = sw.radius + CLIP_EPS;
        let gap = dist2 - reach * reach;
        if !(gap > 0.0) {
            return Some(Contact {
                fraction: 0.0,
                normal: rel * (1.0 / dist2.sqrt()),
                startsolid: (dist2 < sw.radius * sw.radius).then_some(true),
            });
        }
        let a = sw.sweep_len2;
        let b2 = closing * closing;
        let disc = b2 - a * gap;
        if !(disc >= b2 * GRAZE) {
            return None;
        }
        let fraction = (-disc.sqrt() - closing) / a;
        if !(limit > fraction) {
            return None;
        }
        let n = (rel + fraction * sw.sweep) * (1.0 / reach);
        Some(Contact {
            fraction,
            normal: n * ((3.0 - n.length_squared()) * 0.5),
            startsolid: None,
        })
    }

    /// The capsule's end sphere `dz` off its middle against a border's
    /// top edge, from the border's start to its end, then the end it
    /// passed (`CM_TraceSphereThroughBorder`, 1.7's 0x811626a, doc 2).
    /// A corner contact clears `walkable`; an edge one leaves it.
    pub(super) fn border_sphere(&self, b: &CollBorder, dz: f32, tr: &mut ClipTrace) {
        let [nx, ny, dist] = b.dist_eq;
        // The wall top `s` along the border's line, at height `z`.
        let top = |s: f32, z: f32| Vec3::new(ny * s + nx * dist, ny * dist - nx * s, z);
        let ends = [
            top(b.start, b.z_base),
            top(b.start + b.length, b.z_base + b.z_slope * b.length),
        ];
        let mut sphere = self.sw.start;
        sphere.z += dz;
        match self.sweep_edge(sphere, ends[0], ends[0] - ends[1], tr) {
            EdgeSweep::Struck | EdgeSweep::Clear => {}
            EdgeSweep::Beyond(end) => {
                if let Some(hit) = self.corner_contact(sphere, ends[end], tr.fraction) {
                    hit.apply(tr, Some(false));
                }
            }
        }
    }

    /// `CM_PositionTestInAabbTree_r` over indexed triangles, depth first,
    /// the children stopping once one leaves the trace solid (doc 2). A
    /// leaf is solid when the capsule's axis, a point when it has no
    /// length, comes within its radius of a triangle by [`super::pos4`]'s
    /// tests: the client's (`iw3mp.exe` 0x4f2180), which the server's
    /// (`cod4_lnxded` 0x811546a) answers as on every triangle with three
    /// distinct corners (doc 3.1).
    pub(super) fn position_indexed_r(&mut self, m: &IndexedMesh, tree: usize, tr: &mut ClipTrace) {
        let node = self.map.aabbs[tree];
        if self.cull_box(node.origin, node.half_size) {
            return;
        }
        if node.child_count > 0 {
            let first = node.child_or_partition as usize;
            let _ = (first..first + usize::from(node.child_count)).any(|child| {
                self.position_indexed_r(m, child, tr);
                tr.startsolid
            });
            return;
        }
        let Some(part) = self.stamp_partition(node.child_or_partition as usize) else {
            return;
        };
        let first = part.first_tri as usize;
        let touches = (first..first + usize::from(part.tri_count)).any(|i| {
            super::pos4::capsule_in_tri(
                self.sw.start,
                self.sw.radius,
                self.sw.sphere_z,
                corners(m, i),
            )
        });
        if touches {
            tr.fraction = 0.0;
            tr.startsolid = true;
            tr.allsolid = true;
        }
    }
}

#[cfg(test)]
mod vectors;

#[cfg(test)]
mod tests {
    use super::super::trace::Sweep;
    use super::super::{CLeaf, CNode, CPlane, ClipMap, CollAabbTree, MASK_PLAYERSOLID};
    use super::*;

    /// One leaf holding one indexed floor triangle at z 0 with corners
    /// (-200, -100), (-200, 100), (-100, -100); only the long edge
    /// (opposite vertex 0) is walkable.
    pub(super) fn map() -> ClipMap {
        ClipMap {
            planes: vec![CPlane {
                normal: Vec3::Z,
                dist: -1000.0,
                kind: 2,
                signbits: 0,
            }],
            nodes: vec![CNode {
                plane: 0,
                children: [-1, -1],
            }],
            leafs: vec![CLeaf {
                first_aabb: 0,
                aabb_count: 1,
                brush_contents: 0,
                terrain_contents: 1,
                mins: Vec3::ZERO,
                maxs: Vec3::ZERO,
                brush_node: 0,
            }],
            brush_nodes: Vec::new(),
            leaf_brush_list: Vec::new(),
            brushes: Vec::new(),
            materials: vec![(0, 1)],
            verts: Vec::new(),
            edges: Vec::new(),
            tris: Vec::new(),
            borders: Vec::new(),
            partitions: vec![CollPartition {
                unused: 0,
                tri_count: 1,
                border_count: 0,
                first_tri: 0,
                first_border: 0,
            }],
            aabbs: vec![CollAabbTree {
                origin: Vec3::new(-150.0, 0.0, 0.0),
                half_size: Vec3::new(50.0, 100.0, 1.0),
                material: 0,
                child_count: 0,
                child_or_partition: 0,
            }],
            indexed: Some(IndexedMesh {
                verts: vec![
                    Vec3::new(-200.0, -100.0, 0.0),
                    Vec3::new(-200.0, 100.0, 0.0),
                    Vec3::new(-100.0, -100.0, 0.0),
                ],
                tris: vec![[0, 1, 2]],
                walkable: vec![0b001, 0, 0, 0],
            }),
            reach: Default::default(),
        }
    }

    const MINS: Vec3 = Vec3::new(-15.0, -15.0, 0.0);
    const MAXS: Vec3 = Vec3::new(15.0, 15.0, 70.0);

    fn down(x: f32, y: f32) -> (Vec3, Vec3) {
        (Vec3::new(x, y, 100.0), Vec3::new(x, y, -100.0))
    }

    #[test]
    fn a_point_crosses_the_face_with_no_epsilon() {
        let (s, e) = down(-170.0, -50.0);
        let t = map().trace(s, e, Vec3::ZERO, Vec3::ZERO, MASK_PLAYERSOLID);
        assert_eq!((t.fraction, t.normal), (0.5, Vec3::Z));
        let up = map().trace(e, s, Vec3::ZERO, Vec3::ZERO, MASK_PLAYERSOLID);
        assert_eq!(up.fraction, 1.0);
    }

    #[test]
    fn a_capsule_lands_on_the_face_then_on_the_walkable_edge() {
        let (s, e) = down(-170.0, -50.0);
        let t = map().trace(s, e, MINS, MAXS, MASK_PLAYERSOLID);
        assert!((t.fraction - (100.0 - 0.125) / 200.0).abs() < 1e-6, "{t:?}");
        assert!(!t.walkable && !t.startsolid && t.normal == Vec3::Z);
        // 13.4 off the long edge, outside the face: the lower sphere meets
        // the edge, which carries its walkable bit.
        let (s, e) = down(-140.0, 10.0);
        let t = map().trace(s, e, MINS, MAXS, MASK_PLAYERSOLID);
        assert!(t.fraction < 1.0 && t.walkable && t.normal.z < 1.0, "{t:?}");
    }

    #[test]
    fn a_capsule_on_the_face_is_all_solid() {
        let m = map();
        let p = Vec3::new(-170.0, -50.0, -5.0);
        assert!(m.trace(p, p, MINS, MAXS, MASK_PLAYERSOLID).allsolid);
        let clear = Vec3::new(-170.0, -50.0, 0.5);
        assert!(
            !m.trace(clear, clear, MINS, MAXS, MASK_PLAYERSOLID)
                .startsolid
        );
    }

    /// One kernel call on the sweep `start..end` in each model, the
    /// server's then the client's: what it returned and the trace after.
    fn both<R>(
        start: Vec3,
        end: Vec3,
        mins: Vec3,
        maxs: Vec3,
        f: impl Fn(&mut Cx, bool, &mut ClipTrace) -> R,
    ) -> [(R, Bits); 2] {
        let m = map();
        [TraceFloats::Cod4Server, TraceFloats::Cod4Client].map(|floats| {
            let sw = Sweep::new(start, end, mins, maxs, MASK_PLAYERSOLID, floats);
            let mut out = None;
            let tr = m.with_cx(sw, |cx, tr| {
                out = Some(f(cx, floats == TraceFloats::Cod4Client, tr))
            });
            (out.unwrap(), bits(&tr))
        })
    }

    /// A trace's fraction and normal bits, `startsolid`, `walkable`.
    type Bits = (u32, [u32; 3], bool, bool);

    fn bits(t: &ClipTrace) -> Bits {
        let n = t.normal.to_array().map(f32::to_bits);
        (t.fraction.to_bits(), n, t.startsolid, t.walkable)
    }

    fn v(x: f32, y: f32, z: f32) -> Vec3 {
        Vec3::new(x, y, z)
    }

    // The client's bits in the tests below are its port's, read off the
    // instructions (doc 3); each input is one the two models part on.

    #[test]
    fn the_clients_edge_kernel_stores_what_the_servers_keeps() {
        let edge = |start, end, from, along| {
            both(start, end, MINS, MAXS, |cx, client, tr| {
                let sphere = cx.sw.start;
                if client {
                    cx.sweep_edge_client(sphere, from, along, tr)
                } else {
                    cx.sweep_edge(sphere, from, along, tr)
                }
            })
        };
        // struck from outside the cylinder: its fraction and normal
        let [(srv, s), (cl, c)] = edge(
            v(-33.69728, 7.2417984, -49.376637),
            v(30.9794, -58.248, -98.250404),
            v(-25.127277, -7.4672775, -8.450911),
            v(-51.838074, -27.435295, -1.9985809),
        );
        assert_eq!((srv, cl), (EdgeSweep::Struck, EdgeSweep::Struck));
        assert_eq!(
            s,
            (
                0x3d4df0c9,
                [0xbebe9c7a, 0x3f3e6d81, 0xbf0e1788],
                false,
                false
            )
        );
        assert_eq!(
            c,
            (
                0x3d4df0ca,
                [0xbebe9c79, 0x3f3e6d81, 0xbf0e1788],
                false,
                false
            )
        );
        // from inside, solid: the normal off the foot
        let [(srv, s), (cl, c)] = edge(
            v(18.302322, 7.0408936, -58.477543),
            v(83.80708, 12.852058, -62.49416),
            v(0.118349075, 21.553085, 15.26366),
            v(-26.777023, 30.108574, 44.343323),
        );
        assert_eq!((srv, cl), (EdgeSweep::Struck, EdgeSweep::Struck));
        assert_eq!(s, (0, [0xbe1d48fd, 0x3f459cdb, 0xbf1debc9], true, false));
        assert_eq!(c, (0, [0xbe1d4901, 0x3f459cdc, 0xbf1debc8], true, false));
    }

    #[test]
    fn the_clients_corner_kernel_stores_what_the_servers_keeps() {
        let corner = |start, end, at| {
            both(start, end, MINS, MAXS, |cx, client, tr| {
                let sphere = cx.sw.start;
                if client {
                    cx.vertex_client(sphere, at, true, tr);
                } else {
                    cx.vertex4(sphere, at, true, tr);
                }
            })
        };
        // the nearer root: the normal's Newton step
        let [(_, s), (_, c)] = corner(
            v(-33.69728, 7.2417984, -49.376637),
            v(30.9794, -58.248, -98.250404),
            v(-25.127277, -7.4672775, -8.450911),
        );
        assert_eq!(
            s,
            (
                0x3d4e5136,
                [0xbeb3d331, 0x3f41207a, 0xbf0df6c5],
                false,
                true
            )
        );
        assert_eq!(
            c,
            (
                0x3d4e5136,
                [0xbeb3d331, 0x3f41207b, 0xbf0df6c5],
                false,
                true
            )
        );
        // already within reach: the offset over its stored length
        let [(_, s), (_, c)] = corner(
            v(20.977562, -16.623222, -15.676109),
            v(62.531303, -84.10085, -18.213562),
            v(21.78952, -17.048424, 19.119194),
        );
        assert_eq!(s, (0, [0xbf5d5539, 0x3ee7d004, 0x3e5f31ba], true, true));
        assert_eq!(c, (0, [0xbf5d5539, 0x3ee7d003, 0x3e5f31ba], true, true));
    }

    #[test]
    fn the_clients_point_triangle_stores_what_the_servers_keeps() {
        let mesh = IndexedMesh {
            verts: vec![
                v(19.018867, 47.42945, 40.209267),
                v(-9.361618, -27.26174, -41.67109),
                v(-7.8653793, 17.648804, -15.429317),
            ],
            tris: vec![[0, 1, 2]],
            walkable: vec![0; 4],
        };
        let [(_, s), (_, c)] = both(
            v(12.344925, 39.591454, 37.76329),
            v(25.24379, 46.647255, -39.01197),
            Vec3::ZERO,
            Vec3::ZERO,
            |cx, client, tr| {
                if client {
                    cx.point_tri_client(&mesh, 0, tr);
                } else {
                    cx.point_tri4(&mesh, 0, tr);
                }
            },
        );
        assert_eq!(
            s,
            (
                0x3dee9c4d,
                [0xbf4b087d, 0xbe9322dc, 0x3f097b5e],
                false,
                false
            )
        );
        assert_eq!(
            c,
            (
                0x3dee9c4e,
                [0xbf4b087d, 0xbe9322db, 0x3f097b5e],
                false,
                false
            )
        );
    }

    #[test]
    fn the_clients_border_stores_what_the_servers_keeps() {
        let border = |b: CollBorder, start, end| {
            both(start, end, MINS, MAXS, |cx, client, tr| {
                if client {
                    cx.capsule_border_client(&b, tr);
                } else {
                    cx.capsule_border(&b, true, tr);
                }
            })
        };
        // the wall: the distance off the line
        let wall = CollBorder {
            dist_eq: [-0.99523133, -0.09754295, 20.774883],
            z_base: 24.912933,
            z_slope: 0.119893074,
            start: -20.785244,
            length: 62.813374,
        };
        let [(_, s), (_, c)] = border(
            wall,
            v(-45.986008, -2.0987396, -13.837696),
            v(33.073074, -33.92118, 1.8213844),
        );
        assert_eq!(s, (0x3e087564, [0xbf7ec77b, 0xbdc7c499, 0], false, false));
        assert_eq!(c, (0x3e087563, [0xbf7ec77b, 0xbdc7c499, 0], false, false));
        // the top beside the straight part, met by an end sphere
        let top = CollBorder {
            dist_eq: [0.8676906, -0.49710464, 23.876896],
            z_base: -11.224731,
            z_slope: 0.07241905,
            start: -14.763947,
            length: 42.11246,
        };
        let [(_, s), (_, c)] = border(
            top,
            v(41.254692, -24.032406, -22.882381),
            v(-31.916786, 44.488815, 7.1960907),
        );
        assert_eq!(
            s,
            (
                0x3dcc773d,
                [0x3f52c077, 0xbee2410e, 0x3eb6730e],
                false,
                false
            )
        );
        assert_eq!(
            c,
            (
                0x3dcc773b,
                [0x3f52c076, 0xbee2410c, 0x3eb6730c],
                false,
                false
            )
        );
    }

    /// A bound `1 + 0.75 ulp` against a distance of `1 + 1 ulp`: the
    /// server's kept sum culls the node, the client's stored one rounds up
    /// onto the distance and keeps it.
    #[test]
    fn the_clients_cull_box_stores_its_bounds() {
        let culls = |floats| {
            let sw = Sweep {
                mid: v(f32::from_bits(0x3f80_0001), 0.0, 0.0),
                half_sweep_abs: v(f32::from_bits(0x33c0_0000), 0.0, 0.0),
                cull_by_axes: true,
                floats,
                ..Sweep::default()
            };
            let m = map();
            let mut culled = false;
            m.with_cx(sw, |cx, _| {
                culled = cx.cull_box(Vec3::ZERO, v(1.0, 1.0, 1.0))
            });
            culled
        };
        assert!(culls(TraceFloats::Cod4Server));
        assert!(!culls(TraceFloats::Cod4Client));
    }

    /// A cube's position test is a sphere, which 1.7 meets at the
    /// triangle's point nearest `2 v0 - p` (doc 3.1): 10 over the face's
    /// middle is clear on the server and the client alike, 10 over v0
    /// solid.
    #[test]
    fn the_sphere_position_test_reflects_through_v0() {
        let (mins, maxs) = (Vec3::splat(-15.0), Vec3::splat(15.0));
        let solid = |floats, p| {
            map()
                .trace_as(floats, p, p, mins, maxs, MASK_PLAYERSOLID)
                .startsolid
        };
        for floats in [TraceFloats::Cod4Server, TraceFloats::Cod4Client] {
            assert!(!solid(floats, v(-180.0, 50.0, 10.0)));
            // 10 over v0 itself, where the reflection is the point.
            assert!(solid(floats, v(-199.0, -99.0, 10.0)));
        }
    }
}
