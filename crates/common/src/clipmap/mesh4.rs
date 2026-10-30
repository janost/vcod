//! CoD4 1.7's indexed collision triangles: three vertex indices and no
//! stored plane each, met by a point or a capsule sweep and by the
//! capsule's position test, with a walkable bit per edge
//! (`docs/research/cod4-collision.md` 3). The point sweep keeps 1.7's
//! float stores; the capsule's arithmetic is plain `f32`.

// A test written `!(a < b)` keeps the branch a NaN takes.
#![allow(clippy::neg_cmp_op_on_partial_ord)]

use glam::{DVec3, Vec3};

use super::trace::{CLIP_EPS, Cx};
use super::{ClipTrace, CollBorder, CollPartition, IndexedMesh};

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
enum EdgeSweep {
    /// Not before the trace's fraction, or moving away.
    Clear,
    /// Between the ends.
    Struck(Contact),
    /// On the line past end 0 or end 1: only that corner is left to meet.
    Beyond(usize),
}

/// What [`Cx::cylinder_entry`] found: the entry's fraction and the
/// measures the rest of the edge test reads again.
struct Entry {
    enter: f32,
    /// The edge's squared length.
    len2: f32,
    across2: f32,
    off: f32,
    mid_scaled: f32,
}

/// The capsule's sphere nearer a triangle, met against the face.
struct Face {
    sphere: Vec3,
    contact: Contact,
    /// Whether the contact lies outside each edge, in the order 1.7 sweeps
    /// them: `v0 v1`, `v0 v2`, `v1 v2`.
    outside: [bool; 3],
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
        for i in first..first + usize::from(part.tri_count) {
            if self.sw.point {
                self.point_tri4(m, i, tr);
            } else {
                self.capsule_tri4(m, i, tr);
            }
        }
        let sideways = self.sw.sweep.x != 0.0 || self.sw.sweep.y != 0.0;
        if self.sw.point || !sideways || self.sw.sphere_z == 0.0 {
            return;
        }
        let map = self.map;
        let first = part.first_border as usize;
        for b in &map.borders[first..first + usize::from(part.border_count)] {
            self.capsule_border(b, true, tr);
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

    /// The capsule against triangle `i` (`CM_TraceCapsuleThroughTriangle`):
    /// its sphere nearer the face against the face 0.125 out; a contact
    /// off the face sweeps the sphere at each edge it lies outside, and
    /// past an edge's end at that corner. An edge contact takes the
    /// edge's walkable bit (`3 i + the opposite corner`), a corner the
    /// AND of the edges swept past (doc 3).
    fn capsule_tri4(&self, m: &IndexedMesh, i: usize, tr: &mut ClipTrace) {
        let tri = corners(m, i);
        let Some(face) = self.face_contact(tri, tr.fraction) else {
            return;
        };
        let [a, b, c] = tri;
        let edges = [
            (a, a - b, [a, b], 2),
            (a, a - c, [a, c], 1),
            (b, b - c, [b, c], 0),
        ];
        let mut crossed = edges
            .into_iter()
            .zip(face.outside)
            .filter_map(|(e, out)| out.then_some(e))
            .peekable();
        if crossed.peek().is_none() {
            return face.contact.apply(tr, Some(false));
        }
        let mut corner = None;
        let mut walk = true;
        for (from, along, ends, opposite) in crossed {
            let bit = m.edge_walkable(3 * i + opposite);
            match self.sweep_edge(face.sphere, from, along, tr.fraction) {
                EdgeSweep::Struck(hit) => return hit.apply(tr, Some(bit)),
                EdgeSweep::Beyond(end) => {
                    corner = Some(ends[end]);
                    walk &= bit;
                }
                EdgeSweep::Clear => {}
            }
        }
        if let Some(hit) = corner.and_then(|v| self.corner_contact(face.sphere, v, tr.fraction)) {
            hit.apply(tr, Some(walk));
        }
    }

    /// The face test of [`Self::capsule_tri4`]: `None` when the sweep
    /// moves off the face or the sphere misses it before `limit`.
    fn face_contact(&self, [a, b, c]: [Vec3; 3], limit: f32) -> Option<Face> {
        let sw = &self.sw;
        let (ba, ca) = (a - b, a - c);
        let scaled = ca.cross(ba);
        let closing = sw.sweep.dot(scaled);
        if !(closing < 0.0) {
            return None;
        }
        let area2 = scaled.length();
        let normal = scaled / area2;
        // The lower sphere, or the upper under a face turned down.
        let mut sphere = sw.start;
        if normal.z < 0.0 {
            sphere.z += sw.sphere_z;
        } else {
            sphere.z -= sw.sphere_z;
        }
        let lead = sphere - sw.radius * normal - a;
        let gap = lead.dot(normal);
        let (fraction, solid, foot) = if gap >= 0.0 {
            let f = -((gap - CLIP_EPS) / closing) * area2;
            if limit <= f {
                return None;
            }
            (f.max(0.0), false, lead)
        } else {
            // Already through the face's plane: solid when the centre is
            // within a radius of it, measured from its foot on the plane.
            let rel = sphere - a;
            let d = rel.dot(normal);
            if sw.radius * sw.radius <= d * d {
                return None;
            }
            (0.0, true, rel - d * normal)
        };
        let m = sw.sweep.cross(foot);
        let (s, t) = (m.dot(ba), m.dot(ca));
        Some(Face {
            sphere,
            contact: Contact {
                fraction,
                normal,
                startsolid: Some(solid),
            },
            outside: [s < 0.0, t > 0.0, closing > t - s],
        })
    }

    /// The sphere at `sphere`, grown by the clip epsilon, swept at the line
    /// `from + s along` (`CM_TraceSphereThroughEdge`, 1.7's 0x81150ae):
    /// where it enters the cylinder round the line, then whether that
    /// point lies between the ends. A sphere that starts inside the
    /// cylinder and closes on the line meets it at fraction 0, solid when
    /// it is within its own radius.
    fn sweep_edge(&self, sphere: Vec3, from: Vec3, along: Vec3, limit: f32) -> EdgeSweep {
        let rel = sphere - from;
        let Some(entry) = self.cylinder_entry(rel, along, limit) else {
            return EdgeSweep::Clear;
        };
        let sw = &self.sw;
        let entering = entry.enter >= 0.0;
        let at = if entering {
            rel + entry.enter * sw.sweep
        } else {
            rel
        };
        let s = -at.dot(along);
        if !(s > 0.0) {
            return EdgeSweep::Beyond(0);
        }
        if !(entry.len2 > s) {
            return EdgeSweep::Beyond(1);
        }
        // From the nearest point on the line out to the sphere's centre.
        let out = at + (s / entry.len2) * along;
        let contact = if entering {
            Contact {
                fraction: entry.enter,
                normal: out * (1.0 / (sw.radius + CLIP_EPS)),
                startsolid: None,
            }
        } else if out.dot(sw.sweep) < 0.0 {
            let inner = sw.radius * sw.radius * entry.across2 - entry.off * entry.off;
            Contact {
                fraction: 0.0,
                normal: out.normalize(),
                startsolid: Some(entry.len2 * inner > entry.mid_scaled * entry.mid_scaled),
            }
        } else {
            return EdgeSweep::Clear;
        };
        EdgeSweep::Struck(contact)
    }

    /// Where the grown sphere `rel` off a point of the line enters the
    /// cylinder round it, the cylinder's sides measured along
    /// `along x sweep` and scaled by its squared length; `None` when it
    /// never does before `limit`.
    fn cylinder_entry(&self, rel: Vec3, along: Vec3, limit: f32) -> Option<Entry> {
        let sw = &self.sw;
        let reach = sw.radius + CLIP_EPS;
        let across = along.cross(sw.sweep);
        let across2 = across.length_squared();
        let off = rel.dot(across);
        let room = reach * reach * across2 - off * off;
        if room <= 0.0 {
            return None;
        }
        let len2 = along.length_squared();
        let mid_scaled = rel.cross(along).dot(across);
        let mid = mid_scaled / across2;
        let half = (room * len2).sqrt() / across2;
        if mid + half < 0.0 || limit <= mid - half {
            return None;
        }
        Some(Entry {
            enter: mid - half,
            len2,
            across2,
            off,
            mid_scaled,
        })
    }

    /// The sphere at `sphere`, grown by the clip epsilon, swept at the
    /// point `v` (`CM_TraceSphereThroughVertex`): the nearer root, its
    /// normal one Newton step of the reciprocal length from unit (doc 3);
    /// a sphere already within reach meets it at fraction 0.
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
        match self.sweep_edge(sphere, ends[0], ends[0] - ends[1], tr.fraction) {
            EdgeSweep::Struck(hit) => hit.apply(tr, None),
            EdgeSweep::Beyond(end) => {
                if let Some(hit) = self.corner_contact(sphere, ends[end], tr.fraction) {
                    hit.apply(tr, Some(false));
                }
            }
            EdgeSweep::Clear => {}
        }
    }

    /// `CM_PositionTestInAabbTree_r` over indexed triangles, depth first,
    /// the children stopping once one leaves the trace solid (doc 2). A
    /// leaf is solid when the capsule's axis, a point when it has no
    /// length, comes within its radius of a triangle
    /// (`CM_PositionTestCapsuleInTriangle`, doc 3).
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
        let up = Vec3::new(0.0, 0.0, self.sw.sphere_z);
        let (top, bottom) = (self.sw.start + up, self.sw.start - up);
        let r2 = self.sw.radius * self.sw.radius;
        let first = part.first_tri as usize;
        let touches = (first..first + usize::from(part.tri_count))
            .any(|i| segment_tri_dist2(top, bottom, corners(m, i)) < r2);
        if touches {
            tr.fraction = 0.0;
            tr.startsolid = true;
            tr.allsolid = true;
        }
    }
}

/// The squared distance from the segment `a..b` to the triangle `t`.
fn segment_tri_dist2(a: Vec3, b: Vec3, t: [Vec3; 3]) -> f32 {
    let [v0, v1, v2] = t;
    let n = (v1 - v0).cross(v2 - v0);
    let (da, db) = ((a - v0).dot(n), (b - v0).dot(n));
    if da * db <= 0.0 && da != db {
        // The segment crosses the plane: through the face is distance 0.
        let p = a + (b - a) * (da / (da - db));
        let inside = [(v0, v1), (v1, v2), (v2, v0)]
            .iter()
            .all(|&(e0, e1)| (e1 - e0).cross(p - e0).dot(n) >= 0.0);
        if inside {
            return 0.0;
        }
    }
    let mut best = point_tri_dist2(a, t).min(point_tri_dist2(b, t));
    for (p, q) in [(v0, v1), (v1, v2), (v2, v0)] {
        best = best.min(segment_segment_dist2(a, b, p, q));
    }
    best
}

/// The squared distance from `p` to the triangle `t`, by the region of
/// the triangle's plane the projection falls in.
fn point_tri_dist2(p: Vec3, t: [Vec3; 3]) -> f32 {
    let [a, b, c] = t;
    let (ab, ac, ap) = (b - a, c - a, p - a);
    let (d1, d2) = (ab.dot(ap), ac.dot(ap));
    if d1 <= 0.0 && d2 <= 0.0 {
        return ap.length_squared();
    }
    let bp = p - b;
    let (d3, d4) = (ab.dot(bp), ac.dot(bp));
    if d3 >= 0.0 && d4 <= d3 {
        return bp.length_squared();
    }
    let vc = d1 * d4 - d3 * d2;
    if vc <= 0.0 && d1 >= 0.0 && d3 <= 0.0 {
        return (p - (a + ab * (d1 / (d1 - d3)))).length_squared();
    }
    let cp = p - c;
    let (d5, d6) = (ab.dot(cp), ac.dot(cp));
    if d6 >= 0.0 && d5 <= d6 {
        return cp.length_squared();
    }
    let vb = d5 * d2 - d1 * d6;
    if vb <= 0.0 && d2 >= 0.0 && d6 <= 0.0 {
        return (p - (a + ac * (d2 / (d2 - d6)))).length_squared();
    }
    let va = d3 * d6 - d5 * d4;
    if va <= 0.0 && d4 - d3 >= 0.0 && d5 - d6 >= 0.0 {
        let w = (d4 - d3) / ((d4 - d3) + (d5 - d6));
        return (p - (b + (c - b) * w)).length_squared();
    }
    let denom = 1.0 / (va + vb + vc);
    let q = a + ab * (vb * denom) + ac * (vc * denom);
    (p - q).length_squared()
}

/// The squared distance between the segments `p1..q1` and `p2..q2`.
fn segment_segment_dist2(p1: Vec3, q1: Vec3, p2: Vec3, q2: Vec3) -> f32 {
    let (d1, d2, r) = (q1 - p1, q2 - p2, p1 - p2);
    let (a, e, f) = (d1.dot(d1), d2.dot(d2), d2.dot(r));
    let (s, t) = if a <= f32::EPSILON && e <= f32::EPSILON {
        (0.0, 0.0)
    } else if a <= f32::EPSILON {
        (0.0, (f / e).clamp(0.0, 1.0))
    } else {
        let c = d1.dot(r);
        if e <= f32::EPSILON {
            ((-c / a).clamp(0.0, 1.0), 0.0)
        } else {
            let b = d1.dot(d2);
            let denom = a * e - b * b;
            let mut s = if denom != 0.0 {
                ((b * f - c * e) / denom).clamp(0.0, 1.0)
            } else {
                0.0
            };
            let mut t = (b * s + f) / e;
            if t < 0.0 {
                t = 0.0;
                s = (-c / a).clamp(0.0, 1.0);
            } else if t > 1.0 {
                t = 1.0;
                s = ((b - c) / a).clamp(0.0, 1.0);
            }
            (s, t)
        }
    };
    ((p1 + d1 * s) - (p2 + d2 * t)).length_squared()
}

#[cfg(test)]
mod tests {
    use super::super::{CLeaf, CNode, CPlane, ClipMap, CollAabbTree, MASK_PLAYERSOLID};
    use super::*;

    /// One leaf holding one indexed floor triangle at z 0 with corners
    /// (-200, -100), (-200, 100), (-100, -100); only the long edge
    /// (opposite vertex 0) is walkable.
    fn map() -> ClipMap {
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

    const TRI: [Vec3; 3] = [
        Vec3::ZERO,
        Vec3::new(10.0, 0.0, 0.0),
        Vec3::new(0.0, 10.0, 0.0),
    ];

    #[test]
    fn distances_to_a_triangle() {
        assert_eq!(point_tri_dist2(Vec3::new(2.0, 2.0, 3.0), TRI), 9.0);
        assert_eq!(point_tri_dist2(Vec3::new(-3.0, -4.0, 0.0), TRI), 25.0);
        assert_eq!(point_tri_dist2(Vec3::new(5.0, -2.0, 0.0), TRI), 4.0);
        let down = segment_tri_dist2(Vec3::new(2.0, 2.0, 5.0), Vec3::new(2.0, 2.0, -5.0), TRI);
        assert_eq!(down, 0.0);
        let beside = segment_tri_dist2(Vec3::new(-2.0, 5.0, 5.0), Vec3::new(-2.0, 5.0, -5.0), TRI);
        assert!((beside - 4.0).abs() < 1e-5, "{beside}");
    }
}
