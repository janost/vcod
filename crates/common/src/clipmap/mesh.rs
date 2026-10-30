//! The collision AABB trees: their culls, triangles, edges, vertices and
//! borders, swept and position-tested
//! (`docs/research/cod2-d3dbsp-format.md`, "The trace"). Addresses are
//! `cod2_lnxded`'s.
// The negated float tests keep the binary's branch on a NaN.
#![allow(clippy::neg_cmp_op_on_partial_ord)]

use glam::Vec3;

use super::ClipTrace;
use super::trace::{CLIP_EPS, Cx, dot, dot2, fmax, ma};
use super::{CollBorder, CollTri};

/// A triangle's barycentric slack on the point trace (rodata 0x8140000,
/// 0x8140004).
const BARY_LO: f32 = -0.001;
const BARY_HI: f32 = 1.001;
/// The plane-approach threshold a capsule needs to move by (0x814000c, a
/// float).
const APPROACH_EPS: f32 = 1e-4;
/// A contact normal under this slope on a floor this steep takes the
/// floor's normal on a falling sweep (0x8140014).
const WALKABLE: f32 = 0.7;

fn tri_normal(t: &CollTri) -> Vec3 {
    Vec3::new(t.plane[0], t.plane[1], t.plane[2])
}

/// `dot(p, v.xyz) - v.w`, stored.
fn plane_dist(p: Vec3, v: &[f32; 4]) -> f32 {
    (dot(p, Vec3::new(v[0], v[1], v[2])) - f64::from(v[3])) as f32
}

/// `Vec3Normalize` (0x080a4424): the squared length stored, its root
/// through [`sqrtf`], the reciprocal stored, each component one float
/// product.
fn normalize(v: Vec3) -> Vec3 {
    let len = sqrtf(dot(v, v) as f32);
    if len == 0.0 {
        return v;
    }
    v * (1.0 / f64::from(len)) as f32
}

/// The float square root the trace calls (0x080575a0, 0x080aaebe): libm's
/// `sqrt` on the widened float, stored. The extended root rounds to the
/// same float as a correctly rounded one.
fn sqrtf(x: f32) -> f32 {
    x.sqrt()
}

impl Cx<'_> {
    /// 0x08055e78: a root whose material meets the mask; a contact nearer
    /// than the trace's takes that material's flags.
    pub(super) fn trace_aabb_tree(&mut self, tree: usize, tr: &mut ClipTrace) {
        let material = self.map.aabbs[tree].material;
        let (surface, contents) = self.map.materials[material as usize];
        if contents & self.sw.mask == 0 {
            return;
        }
        let before = tr.fraction;
        self.trace_aabb_r(tree, tr);
        if tr.fraction < before {
            tr.surface_flags = surface;
            tr.contents = contents;
            tr.material = Some(material);
        }
    }

    /// `CM_TraceThroughAabbTree_r` (0x08055af0).
    fn trace_aabb_r(&mut self, tree: usize, tr: &mut ClipTrace) {
        let t = self.map.aabbs[tree];
        if self.cull_box(t.origin, t.half_size) {
            return;
        }
        if t.child_count > 0 {
            let first = t.child_or_partition as usize;
            for c in first..first + t.child_count as usize {
                self.trace_aabb_r(c, tr);
            }
            return;
        }
        let Some(part) = self.stamp_partition(t.child_or_partition as usize) else {
            return;
        };
        let map = self.map;
        if let Some(m) = &map.indexed {
            self.trace_indexed_partition(m, &part, tr);
            return;
        }
        let tris = part.first_tri as usize..part.first_tri as usize + part.tri_count as usize;
        if self.sw.point {
            for i in tris {
                self.point_tri(i, tr);
            }
            return;
        }
        for i in tris {
            let oz = self.sw.sphere_z;
            self.capsule_tri(i, oz, tr);
            if self.map.tris[i].plane[2] < 0.0 {
                self.capsule_tri(i, -oz, tr);
            }
        }
        if (self.sw.sweep.x != 0.0 || self.sw.sweep.y != 0.0) && self.sw.sphere_z != 0.0 {
            let first = part.first_border as usize;
            for i in first..first + part.border_count as usize {
                self.capsule_border(&self.map.borders[i], false, tr);
            }
        }
    }

    /// The partition behind a tree leaf, unless this trace clipped it
    /// already. Retail stamps a `u16` and compares it signed against the
    /// trace count (0x08055af0); a full count here keeps the intent.
    pub(super) fn stamp_partition(&mut self, p: usize) -> Option<super::CollPartition> {
        if self.stamps.parts[p] == self.stamps.count {
            return None;
        }
        self.stamps.parts[p] = self.stamps.count;
        Some(self.map.partitions[p])
    }

    /// `CM_CullBox` (0x08055404): the sweep's box against the node's,
    /// on the axes and, unless the sweep is short against its box, the
    /// three cross axes of the sweep.
    pub(super) fn cull_box(&self, origin: Vec3, half: Vec3) -> bool {
        let sw = &self.sw;
        let d = sw.mid - origin;
        let e = half + sw.half_box;
        for k in 0..3 {
            if f64::from(e[k]) + f64::from(sw.half_sweep_abs[k]) < f64::from(d[k].abs()) {
                return true;
            }
        }
        if sw.cull_by_axes {
            return false;
        }
        let (hd, ha) = (sw.half_sweep, sw.half_sweep_abs);
        let cross = |a: f32, b: f32, c: f32, e2: f32| {
            (f64::from(a) * f64::from(b) - f64::from(c) * f64::from(e2)) as f32
        };
        let sum = |a: f32, b: f32, c: f32, e2: f32| {
            f64::from(a) * f64::from(b) + f64::from(c) * f64::from(e2)
        };
        sum(e.z, ha.y, e.y, ha.z) < f64::from(cross(hd.y, d.z, hd.z, d.y).abs())
            || sum(e.x, ha.z, e.z, ha.x) < f64::from(cross(hd.z, d.x, hd.x, d.z).abs())
            || sum(e.y, ha.x, e.x, ha.y) < f64::from(cross(hd.x, d.y, hd.y, d.x).abs())
    }

    /// The point arm (0x0805631a): a one-sided crossing from front to back
    /// inside the triangle's slackened barycentric range.
    fn point_tri(&self, i: usize, tr: &mut ClipTrace) {
        let t = &self.map.tris[i];
        let sw = &self.sw;
        let n = tri_normal(t);
        let d_end = (dot(sw.end, n) - f64::from(t.plane[3])) as f32;
        if !(d_end < 0.0) {
            return;
        }
        let d_start = (dot(sw.start, n) - f64::from(t.plane[3])) as f32;
        if !(d_start > 0.0) {
            return;
        }
        let f = fmax(
            ((f64::from(d_start) - f64::from(CLIP_EPS)) / (f64::from(d_start) - f64::from(d_end)))
                as f32,
            0.0,
        );
        if !(f < tr.fraction) {
            return;
        }
        let g = (f64::from(d_start) / (f64::from(d_start) - f64::from(d_end))) as f32;
        let hit = ma(sw.start, g, sw.sweep);
        let s = plane_dist(hit, &t.svec);
        if !(BARY_LO..=BARY_HI).contains(&s) {
            return;
        }
        let u = plane_dist(hit, &t.tvec);
        if BARY_LO <= u && f64::from(s) + f64::from(u) <= f64::from(BARY_HI) {
            tr.fraction = f;
            tr.normal = n;
        }
    }

    /// `CM_TraceCapsuleThroughTriangle` (0x08056490): the sphere `oz` below
    /// the capsule's centre against the face, then, off the face, its
    /// edges and vertices. A downward face is met by the upper sphere too
    /// (`-oz`).
    fn capsule_tri(&mut self, i: usize, oz: f32, tr: &mut ClipTrace) {
        let t = self.map.tris[i];
        let n = tri_normal(&t);
        let (radius, delta) = (self.sw.radius, self.sw.sweep);
        let mut endp = self.sw.end;
        endp.z -= oz;
        let reach = (f64::from(CLIP_EPS) + f64::from(radius)) as f32;
        let d_end = (dot(endp, n) - f64::from(t.plane[3])) as f32;
        if !(d_end < reach) {
            return;
        }
        let mut sp = self.sw.start;
        sp.z -= oz;
        let d_start = (dot(sp, n) - f64::from(t.plane[3])) as f32;
        // The approach is stored (0x0805654d) and divides the fraction below.
        let approach = d_start - d_end;
        if !(APPROACH_EPS < approach) {
            return;
        }
        let neg = -reach;
        if d_start <= neg {
            // Already behind: solid only if the other sphere still reaches
            // in front within the face.
            let h = (f64::from(d_start) + (f64::from(oz) + f64::from(oz)) * f64::from(n.z)) as f32;
            if !(neg < h) {
                return;
            }
            let s0 = plane_dist(sp, &t.svec);
            let t0 = plane_dist(sp, &t.tvec);
            let inside = |up: f32| {
                let s = (f64::from(s0) + f64::from(up) * f64::from(t.svec[2])) as f32;
                let u = (f64::from(t0) + f64::from(up) * f64::from(t.tvec[2])) as f32;
                (s, u)
            };
            let up = ((f64::from(neg) - f64::from(d_start)) / f64::from(n.z)) as f32;
            let (s, u) = inside(up);
            let solid = if s < 0.0 || u < 0.0 || 1.0 < f64::from(s) + f64::from(u) {
                let up = if reach <= h {
                    ((f64::from(reach) - f64::from(d_start)) / f64::from(n.z)) as f32
                } else {
                    oz + oz
                };
                let (s, u) = inside(up);
                0.0 <= s && 0.0 <= u && f64::from(s) + f64::from(u) <= 1.0
            } else {
                true
            };
            if solid {
                tr.normal = n;
                tr.fraction = 0.0;
                tr.startsolid = true;
            }
            return;
        }
        let (frac, hit) = if d_start - reach <= 0.0 {
            (0.0, sp)
        } else {
            let f = ((f64::from(d_start) - f64::from(reach)) / f64::from(approach)) as f32;
            if tr.fraction < f {
                return;
            }
            (f, ma(sp, f, delta))
        };
        let s = plane_dist(hit, &t.svec);
        let u = plane_dist(hit, &t.tvec);
        let code = u8::from(1.0 < f64::from(s) + f64::from(u))
            | if s < 0.0 { 2 } else { 0 }
            | if u < 0.0 { 4 } else { 0 };
        if code == 0 {
            tr.normal = n;
            tr.fraction = frac;
            if d_start < radius {
                tr.startsolid = true;
            }
            return;
        }
        // Off the face: vertex k where its outcode bit is clear, edge k
        // where it is set.
        let ground = |hit_n: Vec3| {
            if WALKABLE <= n.z && 0.0 <= hit_n.z && hit_n.z < WALKABLE && endp.z < sp.z {
                n
            } else {
                hit_n
            }
        };
        let r2 = f64::from(reach) * f64::from(reach);
        for k in 0..3 {
            if code >> k & 1 == 0 {
                let vi = t.verts[k];
                if vi < 0 || !self.stamp(VERT, vi as usize) {
                    continue;
                }
                let v = sp - self.map.verts[vi as usize];
                let b = dot(delta, v) as f32;
                if !(b < 0.0) {
                    continue;
                }
                let c0 = dot(v, v) as f32;
                let c = (f64::from(c0) - r2) as f32;
                if c <= 0.0 {
                    let inv = (1.0 / f64::from(sqrtf(c0))) as f32;
                    tr.normal = ground(v * inv);
                    tr.fraction = 0.0;
                    if f64::from(radius) * f64::from(radius) <= f64::from(c0) {
                        return;
                    }
                    tr.startsolid = true;
                    return;
                }
                let a = self.sw.sweep_len2;
                let disc = (f64::from(b) * f64::from(b) - f64::from(a) * f64::from(c)) as f32;
                if 0.0 <= disc {
                    let f = ((-f64::from(sqrtf(disc)) - f64::from(b)) / f64::from(a)) as f32;
                    if f < tr.fraction {
                        let scale = (1.0 / f64::from(reach)) as f32;
                        tr.normal = ground(ma(v, f, delta) * scale);
                        tr.fraction = f;
                    }
                }
            } else {
                let ei = t.edges[k];
                if ei < 0 || !self.stamp(EDGE, ei as usize) {
                    continue;
                }
                let e = self.map.edges[ei as usize];
                let v = sp - e.origin;
                let p = [dot(v, e.axis[0]) as f32, dot(v, e.axis[1]) as f32];
                let q = [dot(delta, e.axis[0]) as f32, dot(delta, e.axis[1]) as f32];
                let b = dot2(q, p) as f32;
                if !(b < 0.0) {
                    continue;
                }
                let c0 = dot2(p, p) as f32;
                let c = (f64::from(c0) - r2) as f32;
                let along_ok = |w: Vec3| ((dot(w, e.dir_scaled) as f32) - 0.5).abs() <= 0.5;
                let frame = |a: f32, b: f32| ma(e.axis[0] * a, b, e.axis[1]);
                if c <= 0.0 {
                    if along_ok(v) {
                        tr.normal = ground(normalize(frame(p[0], p[1])));
                        tr.fraction = 0.0;
                        if f64::from(radius) * f64::from(radius) <= f64::from(c0) {
                            return;
                        }
                        tr.startsolid = true;
                        return;
                    }
                    continue;
                }
                let a = dot2(q, q) as f32;
                let disc = (f64::from(b) * f64::from(b) - f64::from(a) * f64::from(c)) as f32;
                if 0.0 < disc {
                    let f = ((-f64::from(sqrtf(disc)) - f64::from(b)) / f64::from(a)) as f32;
                    if f < tr.fraction && along_ok(ma(v, f, delta)) {
                        let n0 = ((f64::from(f) * f64::from(q[0]) + f64::from(p[0]))
                            / f64::from(reach)) as f32;
                        let n1 = ((f64::from(f) * f64::from(q[1]) + f64::from(p[1]))
                            / f64::from(reach)) as f32;
                        tr.normal = ground(frame(n0, n1));
                        tr.fraction = f;
                    }
                }
            }
        }
    }

    /// `CM_TraceCapsuleThroughBorder` (0x08057046): a sideways capsule
    /// against the vertical wall along a terrain partition's outer edge,
    /// its two ends rounded. With `spheres` (CoD4 1.7, 0x81173c4), a wall
    /// top that passes beside the capsule's straight part but within a
    /// radius of an end sphere's centre is met by that sphere
    /// ([`Self::border_sphere`]); CoD2's kernel lets it through.
    pub(super) fn capsule_border(&self, b: &CollBorder, spheres: bool, tr: &mut ClipTrace) {
        let sw = &self.sw;
        let ab = [b.dist_eq[0], b.dist_eq[1]];
        let approach = dot2([sw.sweep.x, sw.sweep.y], ab) as f32;
        if !(approach < 0.0) {
            return;
        }
        let reach = (f64::from(CLIP_EPS) + f64::from(sw.radius)) as f32;
        let d = (dot2([sw.start.x, sw.start.y], ab) - f64::from(b.dist_eq[2])) as f32;
        let mut f = ((f64::from(reach) - f64::from(d)) / f64::from(approach)) as f32;
        if !(f < tr.fraction && -f64::from(reach) <= f64::from(f) * f64::from(sw.sweep_len)) {
            return;
        }
        let mut hit = ma(sw.start, f, sw.sweep);
        let mut along = ((f64::from(ab[1]) * f64::from(hit.x)
            - f64::from(ab[0]) * f64::from(hit.y))
            - f64::from(b.start)) as f32;
        // The end the sweep passed: its distance along the line (extended,
        // as retail sums it), its `along` and its height.
        let end = if along < 0.0 {
            Some((f64::from(b.start), 0.0, b.z_base))
        } else if b.length < along {
            let at = (f64::from(b.z_base) + f64::from(b.z_slope) * f64::from(b.length)) as f32;
            Some((f64::from(b.start) + f64::from(b.length), b.length, at))
        } else {
            None
        };
        if let Some((s, end_along, end_z)) = end {
            // Past an end: the sweep meets the end's vertical line.
            let corner = [
                (f64::from(ab[0]) * f64::from(b.dist_eq[2]) + f64::from(ab[1]) * s) as f32,
                (f64::from(ab[1]) * f64::from(b.dist_eq[2]) - f64::from(ab[0]) * s) as f32,
            ];
            let v = [sw.start.x - corner[0], sw.start.y - corner[1]];
            let bb = dot2([sw.sweep.x, sw.sweep.y], v) as f32;
            if !(bb < 0.0) {
                return;
            }
            let c0 = dot2(v, v) as f32;
            let c = (f64::from(c0) - f64::from(reach) * f64::from(reach)) as f32;
            if c < 0.0 {
                if f64::from(sw.sphere_z) < f64::from((end_z - sw.start.z).abs()) {
                    return;
                }
                tr.normal = Vec3::new(ab[0], ab[1], 0.0);
                tr.fraction = 0.0;
                if f64::from(c0) < f64::from(sw.radius) * f64::from(sw.radius) {
                    tr.startsolid = true;
                }
                return;
            }
            let disc =
                (f64::from(bb) * f64::from(bb) - f64::from(sw.sweep_len2) * f64::from(c)) as f32;
            if disc < 0.0 {
                return;
            }
            f = ((-f64::from(bb) - f64::from(sqrtf(disc))) / f64::from(sw.sweep_len2)) as f32;
            if tr.fraction <= f || f <= 0.0 {
                return;
            }
            hit = ma(sw.start, f, sw.sweep);
            along = end_along;
        } else if f < 0.0 {
            f = 0.0;
        }
        let z = (f64::from(b.z_base) + f64::from(along) * f64::from(b.z_slope)) as f32;
        if ((hit.z - z).abs()) <= sw.sphere_z {
            tr.fraction = f;
            tr.normal = Vec3::new(ab[0], ab[1], 0.0);
        } else if spheres {
            // The top passes above or below the straight part: the end
            // sphere on that side meets it when it is within a radius of
            // that sphere's centre (cod4-collision.md 2).
            let above = z - hit.z;
            if above.abs() < sw.sphere_z + sw.radius {
                let dz = if above > 0.0 {
                    sw.sphere_z
                } else {
                    -sw.sphere_z
                };
                self.border_sphere(b, dz, tr);
            }
        }
    }

    /// Marks vertex or edge `i` for this trace; false when it was already.
    fn stamp(&mut self, kind: u8, i: usize) -> bool {
        let slot = if kind == VERT {
            &mut self.stamps.verts[i]
        } else {
            &mut self.stamps.edges[i]
        };
        if *slot == self.stamps.count {
            return false;
        }
        *slot = self.stamps.count;
        true
    }

    /// `CM_MeshTestInLeaf` (0x08055f50): the leaf's roots in order until
    /// one holds the capsule, which then reports that root's material.
    pub(super) fn mesh_test_in_leaf(&mut self, l: super::CLeaf, tr: &mut ClipTrace) {
        for i in 0..l.aabb_count as usize {
            let tree = l.first_aabb as usize + i;
            let material = self.map.aabbs[tree].material;
            let (surface, contents) = self.map.materials[material as usize];
            if contents & self.sw.mask == 0 {
                continue;
            }
            match &self.map.indexed {
                Some(m) => self.position_indexed_r(m, tree, tr),
                None => self.position_aabb_r(tree, tr),
            }
            if tr.allsolid {
                tr.surface_flags = surface;
                tr.contents = contents;
                tr.material = Some(material);
                return;
            }
        }
    }

    /// `CM_PositionTestInAabbTree_r` (0x08055d4c).
    fn position_aabb_r(&mut self, tree: usize, tr: &mut ClipTrace) {
        let t = self.map.aabbs[tree];
        if self.cull_box(t.origin, t.half_size) {
            return;
        }
        if t.child_count > 0 {
            let first = t.child_or_partition as usize;
            for c in first..first + t.child_count as usize {
                self.position_aabb_r(c, tr);
            }
            return;
        }
        let Some(part) = self.stamp_partition(t.child_or_partition as usize) else {
            return;
        };
        for i in part.first_tri as usize..part.first_tri as usize + part.tri_count as usize {
            self.position_tri(&self.map.tris[i], tr);
        }
    }

    /// `CM_PositionTestCapsuleInTriangle` (0x080555b6): the sphere nearer
    /// the face (the lower one, the upper under a downward face) within the
    /// radius of the face, an edge or a vertex.
    fn position_tri(&self, t: &CollTri, tr: &mut ClipTrace) {
        let sw = &self.sw;
        let n = tri_normal(t);
        let oz = if n.z < 0.0 { -sw.sphere_z } else { sw.sphere_z };
        let mut p = sw.start;
        p.z -= oz;
        let r = sw.radius;
        let d = (dot(p, n) - f64::from(t.plane[3])) as f32;
        if !(d < r) {
            return;
        }
        let solid = |tr: &mut ClipTrace| {
            tr.startsolid = true;
            tr.allsolid = true;
            tr.fraction = 0.0;
        };
        let neg = -r;
        if d <= neg {
            let h = (f64::from(d) + (f64::from(oz) + f64::from(oz)) * f64::from(n.z)) as f32;
            if !(neg < h) {
                return;
            }
            let s0 = plane_dist(p, &t.svec);
            let t0 = plane_dist(p, &t.tvec);
            let at = |up: f32| {
                (
                    (f64::from(s0) + f64::from(up) * f64::from(t.svec[2])) as f32,
                    (f64::from(t0) + f64::from(up) * f64::from(t.tvec[2])) as f32,
                )
            };
            let up = ((f64::from(neg) - f64::from(d)) / f64::from(n.z)) as f32;
            let (s, u) = at(up);
            if s < 0.0 || u < 0.0 || 1.0 < f64::from(s) + f64::from(u) {
                let up = if r <= h {
                    ((f64::from(r) - f64::from(d)) / f64::from(n.z)) as f32
                } else {
                    oz + oz
                };
                let (s, u) = at(up);
                if 0.0 <= s && 0.0 <= u && f64::from(s) + f64::from(u) <= 1.0 {
                    solid(tr);
                }
            } else {
                solid(tr);
            }
            return;
        }
        let q = ma(p, -d, n);
        let s = plane_dist(q, &t.svec);
        let u = plane_dist(q, &t.tvec);
        let code = u8::from(1.0 < f64::from(s) + f64::from(u))
            | if s < 0.0 { 2 } else { 0 }
            | if u < 0.0 { 4 } else { 0 };
        if code == 0 {
            solid(tr);
            return;
        }
        let r2 = f64::from(r) * f64::from(r);
        for k in 0..3 {
            if code >> k & 1 == 0 {
                let vi = t.verts[k];
                if vi < 0 {
                    continue;
                }
                let v = p - self.map.verts[vi as usize];
                if dot(v, v) < r2 {
                    solid(tr);
                    return;
                }
            } else {
                let ei = t.edges[k];
                if ei < 0 {
                    continue;
                }
                let e = self.map.edges[ei as usize];
                let v = p - e.origin;
                let along = dot(v, e.dir_scaled) as f32;
                if (along - 0.5).abs() <= 0.5 {
                    let along = (f64::from(along) / dot(e.dir_scaled, e.dir_scaled)) as f32;
                    let w = ma(v, -along, e.dir_scaled);
                    if dot(w, w) < r2 {
                        solid(tr);
                        return;
                    }
                }
            }
        }
    }
}

const VERT: u8 = 0;
const EDGE: u8 = 1;
