//! `CM_Trace`'s set-up, the BSP walk and the brushes
//! (`docs/research/cod2-d3dbsp-format.md`, "The trace").
//! Addresses are `cod2_lnxded`'s.
// The negated float tests keep the binary's branch on a NaN.
#![allow(clippy::neg_cmp_op_on_partial_ord)]

use glam::Vec3;

use super::BrushNode;
use super::{CBrush, CLeaf, ClipMap, ClipTrace, Stamps};

/// The parallel split's epsilon (rodata 0x8140254, 0x8140268).
const SPLIT_EPS: f32 = 4.768_371_6e-7;
/// `SURFACE_CLIP_EPSILON` (0x814024c and its copies).
pub(super) const CLIP_EPS: f32 = 0.125;
/// `BoxLeafnums`' list (0x08059c4c passes 0x400).
const MAX_LEAFS: usize = 0x400;

/// A dot product as the engine's helpers sum it (0x0805d758, 0x080562b2):
/// x plus y, then z, extended.
pub(super) fn dot(a: Vec3, b: Vec3) -> f64 {
    f64::from(a.x) * f64::from(b.x)
        + f64::from(a.y) * f64::from(b.y)
        + f64::from(a.z) * f64::from(b.z)
}

/// The 2D dot (0x08057610): y, then x.
pub(super) fn dot2(a: [f32; 2], b: [f32; 2]) -> f64 {
    f64::from(a[1]) * f64::from(b[1]) + f64::from(a[0]) * f64::from(b[0])
}

/// `a + b * s` per component, stored (0x0805625e).
pub(super) fn ma(a: Vec3, s: f32, b: Vec3) -> Vec3 {
    Vec3::from(std::array::from_fn::<f32, 3, _>(|k| {
        (f64::from(a[k]) + f64::from(s) * f64::from(b[k])) as f32
    }))
}

/// The engine's `fsel`-built min and max (0x0805d5f2, 0x0805d5ce): the
/// difference is passed as a float.
pub(super) fn fmin(a: f32, b: f32) -> f32 {
    if b - a >= 0.0 { a } else { b }
}

pub(super) fn fmax(a: f32, b: f32) -> f32 {
    if a - b >= 0.0 { a } else { b }
}

/// The sweep `CM_Trace` (0x0805b414) sets up for the kernels: shifted to
/// the box's centre, the capsule the box becomes and the bounds it covers.
#[derive(Default)]
pub(super) struct Sweep {
    // What is traced: both ends on the box's centre, and the mask.
    pub start: Vec3,
    pub end: Vec3,
    pub mask: u32,
    // The shape.
    /// The box's half extents about its centre.
    pub half_box: Vec3,
    /// A sweep of no size.
    pub point: bool,
    /// The capsule's radius, and its end spheres' centres' height off its
    /// middle.
    pub radius: f32,
    pub sphere_z: f32,
    /// The capsule's half extents per axis; zero for a position test.
    pub capsule_half: Vec3,
    // The move.
    /// `end - start`, its length and squared length.
    pub sweep: Vec3,
    pub sweep_len: f32,
    pub sweep_len2: f32,
    pub half_sweep: Vec3,
    pub half_sweep_abs: Vec3,
    pub mid: Vec3,
    /// `1 / (start - end)` per axis, 0 where the two are equal.
    pub back_recip: Vec3,
    // What it covers.
    /// The box the capsule sweeps through.
    pub swept_box: [Vec3; 2],
    /// The sweep is short against its box: `CM_CullBox` tests the axes
    /// only.
    pub cull_by_axes: bool,
    /// The box the whole sweep covers, grown by [`REACH_SLACK`].
    pub reach: [Vec3; 2],
    /// The box the capsule's axial extents ([`Sweep::capsule_half`]) cover
    /// along the sweep, grown by [`REACH_SLACK`]: a brush box it misses
    /// has a side both ends clear.
    pub brush_reach: [Vec3; 2],
}

/// How far past a node's reach ([`ClipMap::node_reach`]) a sweep's box
/// must stay for the node to be passed over: well past the last bits the
/// leaf tests' float sums can round by.
const REACH_SLACK: f32 = 1.0;

impl Sweep {
    /// `kept` is CoD4 1.7's set-up (`CM_Trace`, `cod4_lnxded`
    /// 0x811ce9f..0x811cfb8): the box's centre offset and the shifted end
    /// stay on the x87 stack, so the sweep is the unrounded end less the
    /// stored start, rounded once (cod4-collision.md 2). CoD2 1.3 stores
    /// each (0x0805b414).
    fn new(start: Vec3, end: Vec3, mins: Vec3, maxs: Vec3, mask: u32, kept: bool) -> Sweep {
        let mut sw = Sweep {
            mask,
            ..Sweep::default()
        };
        for k in 0..3 {
            let off = 0.5 * (f64::from(mins[k]) + f64::from(maxs[k]));
            if kept {
                sw.half_box[k] = (f64::from(maxs[k]) - off) as f32;
                sw.start[k] = (f64::from(start[k]) + off) as f32;
                let end_kept = f64::from(end[k]) + off;
                sw.end[k] = end_kept as f32;
                sw.mid[k] = ((f64::from(sw.start[k]) + end_kept) * 0.5) as f32;
                sw.sweep[k] = (end_kept - f64::from(sw.start[k])) as f32;
            } else {
                let off = off as f32;
                sw.half_box[k] = maxs[k] - off;
                sw.start[k] = start[k] + off;
                sw.end[k] = end[k] + off;
                sw.mid[k] = (0.5 * (f64::from(sw.start[k]) + f64::from(sw.end[k]))) as f32;
                sw.sweep[k] = sw.end[k] - sw.start[k];
            }
            sw.half_sweep[k] = 0.5 * sw.sweep[k];
            sw.half_sweep_abs[k] = sw.half_sweep[k].abs();
        }
        // `CM_CalcTraceExtents` (0x0805d86c): the reciprocal of start - end.
        for k in 0..3 {
            let d = sw.start[k] - sw.end[k];
            sw.back_recip[k] = if d != 0.0 {
                (1.0 / f64::from(d)) as f32
            } else {
                0.0
            };
        }
        sw.sweep_len2 = dot(sw.sweep, sw.sweep) as f32;
        sw.sweep_len = f64::from(sw.sweep_len2).sqrt() as f32;
        sw.radius = if sw.half_box.z < sw.half_box.x {
            sw.half_box.z
        } else {
            sw.half_box.x
        };
        sw.sphere_z = sw.half_box.z - sw.radius;
        let r = sw.radius;
        for k in 0..2 {
            let (lo, hi) = if sw.start[k] < sw.end[k] {
                (sw.start[k], sw.end[k])
            } else {
                (sw.end[k], sw.start[k])
            };
            sw.swept_box[0][k] = lo - r;
            sw.swept_box[1][k] = hi + r;
        }
        let (lo, hi) = if sw.end.z <= sw.start.z {
            (sw.end.z, sw.start.z)
        } else {
            (sw.start.z, sw.end.z)
        };
        sw.swept_box[1].z = (f64::from(hi) + f64::from(sw.sphere_z) + f64::from(r)) as f32;
        sw.swept_box[0].z = (f64::from(lo) - f64::from(sw.sphere_z) - f64::from(r)) as f32;
        // 0x0805b39c: a sweep whose bounds are small against its box is
        // culled by axes alone.
        let b = sw.swept_box[1] - sw.swept_box[0];
        sw.cull_by_axes = f64::from(b.x) * f64::from(b.y) * f64::from(b.z)
            < 16.0
                * f64::from(sw.half_box.x)
                * f64::from(sw.half_box.y)
                * f64::from(sw.half_box.z)
                * f64::from(sw.sweep_len);
        if start != end {
            sw.point =
                f64::from(sw.half_box.x) + f64::from(sw.half_box.y) + f64::from(sw.half_box.z)
                    == 0.0;
            sw.capsule_half = Vec3::new(r, r, r + sw.sphere_z);
        }
        let grow = sw.half_box.abs() + Vec3::splat(REACH_SLACK);
        sw.reach = [sw.start.min(sw.end) - grow, sw.start.max(sw.end) + grow];
        let grow = sw.capsule_half.abs() + Vec3::splat(REACH_SLACK);
        sw.brush_reach = [sw.start.min(sw.end) - grow, sw.start.max(sw.end) + grow];
        sw
    }
}

/// One trace in progress.
pub(super) struct Cx<'a> {
    pub map: &'a ClipMap,
    pub stamps: &'a mut Stamps,
    pub sw: Sweep,
}

/// The side a contact came in through: an axial side of the brush, or one
/// of its planes.
#[derive(Clone, Copy)]
enum Lead {
    Axial {
        axis: usize,
        sign: f32,
        material: u16,
    },
    Side(u32, u16),
}

impl<'a> Cx<'a> {
    pub fn new(
        map: &'a ClipMap,
        stamps: &'a mut Stamps,
        start: Vec3,
        end: Vec3,
        mins: Vec3,
        maxs: Vec3,
        mask: u32,
    ) -> Cx<'a> {
        Cx {
            map,
            stamps,
            sw: Sweep::new(start, end, mins, maxs, mask, map.indexed.is_some()),
        }
    }

    /// The moving arm of `CM_Trace` on model 0.
    pub fn sweep(&mut self, tr: &mut ClipTrace) {
        let p1 = [self.sw.start.x, self.sw.start.y, self.sw.start.z, 0.0];
        let p2 = [self.sw.end.x, self.sw.end.y, self.sw.end.z, tr.fraction];
        self.trace_tree(0, p1, p2, tr);
    }

    /// `CM_TraceThroughTree` (0x0805b058): Q3's split with the box's extent
    /// plus 0.125 on axial planes and 2048 (0.125 for a point) on the rest.
    /// A node whose reach the sweep's box misses holds no leaf test that
    /// passes, so it is passed over: the walk under it has no other
    /// effect (cod4-server-frame.md 11).
    fn trace_tree(&mut self, mut num: i32, mut p1: [f32; 4], p2: [f32; 4], tr: &mut ClipTrace) {
        let reach = self.map.node_reach();
        let [sweep_lo, sweep_hi] = self.sw.reach;
        loop {
            if num < 0 {
                self.trace_leaf(!num as usize, tr);
                return;
            }
            let [lo, hi] = reach[num as usize];
            if sweep_hi.cmplt(lo).any() || sweep_lo.cmpgt(hi).any() {
                return;
            }
            let node = self.map.nodes[num as usize];
            let plane = self.map.planes[node.plane as usize];
            let (t1, t2, offset) = if plane.kind < 3 {
                let k = plane.kind as usize;
                (
                    p1[k] - plane.dist,
                    p2[k] - plane.dist,
                    (f64::from(CLIP_EPS) + f64::from(self.sw.half_box[k])) as f32,
                )
            } else {
                let at = |p: [f32; 4]| {
                    (dot(Vec3::new(p[0], p[1], p[2]), plane.normal) - f64::from(plane.dist)) as f32
                };
                (at(p1), at(p2), if self.sw.point { 0.125 } else { 2048.0 })
            };
            match split(t1, t2, offset) {
                Split::Front => num = node.children[0],
                Split::Back => num = node.children[1],
                Split::Cross { side, frac, frac2 } => {
                    if tr.fraction <= p1[3] {
                        return;
                    }
                    let mid = lerp4(p1, p2, frac);
                    self.trace_tree(node.children[side], p1, mid, tr);
                    p1 = lerp4(p1, p2, frac2);
                    num = node.children[side ^ 1];
                }
            }
        }
    }

    /// 0x0805a6ae: a leaf's brushes, then while the fraction is not 0 its
    /// AABB trees in lump order.
    fn trace_leaf(&mut self, leaf: usize, tr: &mut ClipTrace) {
        self.trace_in_leaf(self.map.leafs[leaf], tr);
    }

    /// `CM_TraceThroughLeaf` on a leaf of the tree or a submodel's.
    pub fn trace_in_leaf(&mut self, l: CLeaf, tr: &mut ClipTrace) {
        if tr.fraction == 0.0 {
            return;
        }
        if self.sw.mask & l.brush_contents != 0 && self.trace_leaf_brushes(l, tr) {
            return;
        }
        if self.sw.mask & l.terrain_contents == 0 {
            return;
        }
        for i in 0..l.aabb_count as usize {
            if tr.fraction == 0.0 {
                break;
            }
            self.trace_aabb_tree(l.first_aabb as usize + i, tr);
        }
    }

    /// `CM_TraceThroughLeafBrushNode` (0x0805a5a8): the leaf's brush box
    /// grown by the trace's box culls first. True when the fraction ended
    /// at 0.
    fn trace_leaf_brushes(&mut self, l: CLeaf, tr: &mut ClipTrace) -> bool {
        let lo = l.mins - self.sw.half_box;
        let hi = l.maxs + self.sw.half_box;
        if self.trace_box_culls(lo, hi, tr.fraction) {
            return false;
        }
        let p1 = [self.sw.start.x, self.sw.start.y, self.sw.start.z, 0.0];
        let p2 = [self.sw.end.x, self.sw.end.y, self.sw.end.z, tr.fraction];
        self.trace_brush_node(l.brush_node as usize, p1, p2, tr);
        tr.fraction == 0.0
    }

    /// `CM_TraceBox` (0x0805d8e0): whether the segment misses the box, or
    /// meets it no nearer than `fraction`.
    fn trace_box_culls(&self, lo: Vec3, hi: Vec3, fraction: f32) -> bool {
        let sw = &self.sw;
        let (mut enter, mut leave) = (0.0f32, fraction);
        for (sign, b) in [(-1.0f32, lo), (1.0, hi)] {
            for k in 0..3 {
                let d1 = (sw.start[k] - b[k]) * sign;
                let d2 = (sw.end[k] - b[k]) * sign;
                if d1 > 0.0 {
                    if d2 > 0.0 {
                        return true;
                    }
                    let f = (f64::from(d1) * f64::from(sw.back_recip[k]) * f64::from(sign)) as f32;
                    if leave <= f {
                        return true;
                    }
                    enter = fmax(enter, f);
                } else if d2 > 0.0 {
                    let f = (f64::from(d1) * f64::from(sw.back_recip[k]) * f64::from(sign)) as f32;
                    if f <= enter {
                        return true;
                    }
                    leave = fmin(leave, f);
                }
            }
        }
        false
    }

    /// `CM_TraceThroughLeafBrushNode_r` (0x0805a1e0): the k-d tree, the
    /// straddlers' subtree first, then near side before far.
    fn trace_brush_node(
        &mut self,
        mut n: usize,
        mut p1: [f32; 4],
        p2: [f32; 4],
        tr: &mut ClipTrace,
    ) {
        loop {
            let node = self.map.brush_nodes[n];
            if self.sw.mask & node.contents() == 0 {
                return;
            }
            let BrushNode::Split {
                axis,
                dist,
                clearance,
                straddlers,
                children,
                ..
            } = node
            else {
                let BrushNode::Leaf { first, count, .. } = node else {
                    unreachable!()
                };
                for &b in &self.map.leaf_brush_list[first as usize..][..count as usize] {
                    let brush = &self.map.brushes[b as usize];
                    if self.sw.mask & brush.contents != 0 {
                        self.trace_brush(brush, tr);
                    }
                }
                return;
            };
            if straddlers {
                self.trace_brush_node(n + 1, p1, p2, tr);
            }
            let k = axis as usize;
            let t1 = p1[k] - dist;
            let t2 = p2[k] - dist;
            let offset = (f64::from(CLIP_EPS) + f64::from(self.sw.half_box[k])
                - f64::from(clearance)) as f32;
            let mx = fmax(t1, t2);
            if fmin(t1, t2) >= offset {
                if mx <= -offset {
                    return;
                }
                n += children[0] as usize;
                continue;
            }
            if mx <= -offset {
                n += children[1] as usize;
                continue;
            }
            if tr.fraction <= p1[3] {
                return;
            }
            let Split::Cross { side, frac, frac2 } = cross(t1, t2, offset) else {
                unreachable!()
            };
            let mid = lerp4(p1, p2, frac);
            self.trace_brush_node(n + children[side] as usize, p1, mid, tr);
            p1 = lerp4(p1, p2, frac2);
            n += children[side ^ 1] as usize;
        }
    }

    /// `CM_TraceThroughBrush` (0x08059dc0): the axial sides against the
    /// capsule's box, every other side against the sphere nearer it.
    fn trace_brush(&self, b: &CBrush, tr: &mut ClipTrace) {
        let sw = &self.sw;
        // Both ends more than the slack outside one axial side: the loop
        // below leaves on that side (`fmin(d1, CLIP_EPS) <= d2`), and
        // nothing before its end writes the trace.
        if sw.brush_reach[1].cmplt(b.mins).any() || sw.brush_reach[0].cmpgt(b.maxs).any() {
            return;
        }
        let (mut enter, mut leave) = (0.0f32, tr.fraction);
        let mut allsolid = true;
        let mut lead: Option<Lead> = None;
        for (pass, (sign, bound)) in [(-1.0f32, b.mins), (1.0, b.maxs)].into_iter().enumerate() {
            for k in 0..3 {
                let at = |p: f32| {
                    ((f64::from(p) - f64::from(bound[k])) * f64::from(sign)
                        - f64::from(sw.capsule_half[k])) as f32
                };
                let (d1, d2) = (at(sw.start[k]), at(sw.end[k]));
                if d1 > 0.0 {
                    if fmin(d1, CLIP_EPS) <= d2 {
                        return;
                    }
                    let f = ((f64::from(d1) - f64::from(CLIP_EPS))
                        * f64::from(sw.back_recip[k])
                        * f64::from(sign)) as f32;
                    if leave <= f {
                        return;
                    }
                    if d2 > 0.0 {
                        allsolid = false;
                    }
                    if enter < f || lead.is_none() {
                        enter = enter.max(f);
                        lead = Some(Lead::Axial {
                            axis: k,
                            sign,
                            material: b.axial_material[pass][k],
                        });
                    }
                } else if d2 > 0.0 {
                    let f = (f64::from(d1) * f64::from(sw.back_recip[k]) * f64::from(sign)) as f32;
                    if f <= enter {
                        return;
                    }
                    allsolid = false;
                    leave = fmin(leave, f);
                }
            }
        }
        for &(plane, material) in &b.sides {
            let p = self.map.planes[plane as usize];
            let lift = (p.normal.z * sw.sphere_z).abs();
            let dist = (f64::from(p.dist) + f64::from(sw.radius) + f64::from(lift)) as f32;
            let d1 = (dot(sw.start, p.normal) - f64::from(dist)) as f32;
            let d2 = (dot(sw.end, p.normal) - f64::from(dist)) as f32;
            if d1 > 0.0 {
                if fmin(d1, CLIP_EPS) <= d2 {
                    return;
                }
                if d2 > 0.0 {
                    allsolid = false;
                }
                let denom = d1 - d2;
                let num = d1 - CLIP_EPS;
                if f64::from(enter) * f64::from(denom) < f64::from(num) {
                    enter = (f64::from(num) / f64::from(denom)) as f32;
                    if leave <= enter {
                        return;
                    }
                    lead = Some(Lead::Side(plane, material));
                } else if lead.is_none() {
                    lead = Some(Lead::Side(plane, material));
                }
            } else if d2 > 0.0 {
                let denom = d1 - d2;
                if f64::from(leave) * f64::from(denom) < f64::from(d1) {
                    leave = (f64::from(d1) / f64::from(denom)) as f32;
                    if leave <= enter {
                        return;
                    }
                }
                allsolid = false;
            }
        }
        tr.contents = b.contents;
        let Some(lead) = lead else {
            tr.startsolid = true;
            if allsolid {
                tr.allsolid = true;
                tr.fraction = 0.0;
            }
            return;
        };
        let (normal, material) = match lead {
            Lead::Axial {
                axis,
                sign,
                material,
            } => {
                let mut n = Vec3::ZERO;
                n[axis] = sign;
                (n, material)
            }
            Lead::Side(plane, material) => (self.map.planes[plane as usize].normal, material),
        };
        tr.fraction = enter;
        tr.normal = normal;
        tr.surface_flags = self.map.materials[material as usize].0;
        tr.material = Some(material);
        tr.walkable = false;
    }

    /// The still arm of `CM_Trace` on model 0 (0x08059c4c): the leaves the
    /// box grown by 1 touches, in `CM_BoxLeafnums` order, until one holds
    /// it.
    pub fn position_test(&mut self, tr: &mut ClipTrace) {
        let grow = Vec3::ONE;
        let lo = self.sw.start - self.sw.half_box - grow;
        let hi = self.sw.start + self.sw.half_box + grow;
        let mut leafs = Vec::new();
        box_leafnums(self.map, lo, hi, 0, &mut leafs);
        for leaf in leafs {
            if tr.allsolid {
                break;
            }
            self.test_in_leaf(self.map.leafs[leaf], tr);
        }
    }

    /// 0x08059878 (`CM_TestInLeaf`), on a leaf of the tree or a
    /// submodel's.
    pub fn test_in_leaf(&mut self, l: CLeaf, tr: &mut ClipTrace) {
        if self.sw.mask & l.brush_contents != 0 && self.test_leaf_brushes(l, tr) {
            return;
        }
        if self.sw.mask & l.terrain_contents != 0 {
            self.mesh_test_in_leaf(l, tr);
        }
    }

    /// 0x080597ce: the leaf's brush box must overlap the trace's bounds
    /// strictly. True when the box ended all solid.
    fn test_leaf_brushes(&self, l: CLeaf, tr: &mut ClipTrace) -> bool {
        for k in 0..3 {
            if self.sw.swept_box[1][k] <= l.mins[k] || l.maxs[k] <= self.sw.swept_box[0][k] {
                return false;
            }
        }
        self.test_brush_node(l.brush_node as usize, tr);
        tr.allsolid
    }

    /// `CM_TestInLeafBrushNode_r` (0x08059636).
    fn test_brush_node(&self, mut n: usize, tr: &mut ClipTrace) {
        loop {
            let node = self.map.brush_nodes[n];
            if self.sw.mask & node.contents() == 0 {
                return;
            }
            match node {
                BrushNode::Leaf { first, count, .. } => {
                    for &b in &self.map.leaf_brush_list[first as usize..][..count as usize] {
                        let brush = &self.map.brushes[b as usize];
                        if self.sw.mask & brush.contents != 0 {
                            self.test_brush(brush, tr);
                            if tr.allsolid {
                                return;
                            }
                        }
                    }
                    return;
                }
                BrushNode::Split {
                    axis,
                    dist,
                    straddlers,
                    children,
                    ..
                } => {
                    if straddlers {
                        self.test_brush_node(n + 1, tr);
                        if tr.allsolid {
                            return;
                        }
                    }
                    let k = axis as usize;
                    if dist < self.sw.swept_box[0][k] {
                        n += children[0] as usize;
                    } else {
                        if dist <= self.sw.swept_box[1][k] {
                            self.test_brush_node(n + children[0] as usize, tr);
                            if tr.allsolid {
                                return;
                            }
                        }
                        n += children[1] as usize;
                    }
                }
            }
        }
    }

    /// `CM_TestBoxInBrush` (0x080594f6): the capsule's box against the
    /// brush's bounds, its spheres against the other sides.
    fn test_brush(&self, b: &CBrush, tr: &mut ClipTrace) {
        let sw = &self.sw;
        for k in 0..3 {
            if !(sw.swept_box[0][k] <= b.maxs[k] && b.mins[k] <= sw.swept_box[1][k]) {
                return;
            }
        }
        for &(plane, _) in &b.sides {
            let p = self.map.planes[plane as usize];
            let lift = (p.normal.z * sw.sphere_z).abs();
            let dist = f64::from(p.dist) + f64::from(sw.radius) + f64::from(lift);
            if (dot(sw.start, p.normal) - dist) as f32 > 0.0 {
                return;
            }
        }
        tr.allsolid = true;
        tr.startsolid = true;
        tr.fraction = 0.0;
        tr.contents = b.contents;
    }
}

/// Where a node puts the segment: wholly on a side, or across it with the
/// near child and the two fractions the split goes on with.
enum Split {
    Front,
    Back,
    Cross { side: usize, frac: f32, frac2: f32 },
}

/// The node test both walks share: front when both ends clear `offset`,
/// back when both clear `-offset`.
fn split(t1: f32, t2: f32, offset: f32) -> Split {
    if fmin(t1, t2) >= offset {
        return Split::Front;
    }
    if fmax(t1, t2) <= -offset {
        return Split::Back;
    }
    cross(t1, t2, offset)
}

/// The crossing's fractions: the near side runs to `frac` (at most 1), the
/// far side from `frac2` (at least 0); a segment nearly parallel to the
/// plane walks the front first, whole.
fn cross(t1: f32, t2: f32, offset: f32) -> Split {
    let diff = t2 - t1;
    let (side, frac, frac2) = if diff.abs() > SPLIT_EPS {
        let d = if diff >= 0.0 { -t1 } else { t1 };
        let inv = (1.0 / f64::from(diff.abs())) as f32;
        (
            usize::from(diff >= 0.0),
            ((f64::from(d) + f64::from(offset)) * f64::from(inv)) as f32,
            ((f64::from(d) - f64::from(offset)) * f64::from(inv)) as f32,
        )
    } else {
        (0, 1.0, 0.0)
    };
    Split::Cross {
        side,
        frac: fmin(frac, 1.0),
        frac2: fmax(frac2, 0.0),
    }
}

/// `p1 + (p2 - p1) * f` per component, the fraction word included.
fn lerp4(p1: [f32; 4], p2: [f32; 4], f: f32) -> [f32; 4] {
    std::array::from_fn(|k| {
        (f64::from(p1[k]) + (f64::from(p2[k]) - f64::from(p1[k])) * f64::from(f)) as f32
    })
}

/// `BoxOnPlaneSide` (0x080aa17e): 1 in front, 2 behind, 3 across.
fn box_on_plane_side(lo: Vec3, hi: Vec3, p: &super::CPlane) -> u8 {
    if p.kind < 3 {
        let k = p.kind as usize;
        return if p.dist <= lo[k] {
            1
        } else if hi[k] <= p.dist {
            2
        } else {
            3
        };
    }
    let n = p.normal;
    let pick = |bit: u8, k: usize, near: bool| {
        // Sign bit set: the far corner on that axis is the min.
        if (p.signbits >> bit & 1 == 1) == near {
            hi[k]
        } else {
            lo[k]
        }
    };
    let corner = |near: bool| {
        (f64::from(n.z) * f64::from(pick(2, 2, near))
            + f64::from(n.y) * f64::from(pick(1, 1, near))
            + f64::from(n.x) * f64::from(pick(0, 0, near))) as f32
    };
    let (far, near) = (corner(false), corner(true));
    u8::from(p.dist <= far) | if near < p.dist { 2 } else { 0 }
}

/// `CM_BoxLeafnums_r` (0x08058cc2): the leaves the box touches, front
/// child first, at most [`MAX_LEAFS`].
fn box_leafnums(map: &ClipMap, lo: Vec3, hi: Vec3, mut num: i32, out: &mut Vec<usize>) {
    while num >= 0 {
        let node = map.nodes[num as usize];
        match box_on_plane_side(lo, hi, &map.planes[node.plane as usize]) {
            1 => num = node.children[0],
            2 => num = node.children[1],
            _ => {
                box_leafnums(map, lo, hi, node.children[0], out);
                num = node.children[1];
            }
        }
    }
    if out.len() < MAX_LEAFS {
        out.push(!num as usize);
    }
}

/// `CM_PointContents` on model 0 (0x08058f8c, 0x08058ddc).
pub(super) fn point_contents(map: &ClipMap, p: Vec3) -> u32 {
    let mut num = 0i32;
    while num >= 0 {
        let node = map.nodes[num as usize];
        let plane = map.planes[node.plane as usize];
        let d = if plane.kind < 3 {
            p[plane.kind as usize] - plane.dist
        } else {
            (dot(plane.normal, p) - f64::from(plane.dist)) as f32
        };
        num = node.children[usize::from(d < 0.0)];
    }
    leaf_contents(map, &map.leafs[!num as usize], p)
}

/// `CM_PointContents`' leaf half: the brushes of `l` that hold `p`.
pub(super) fn leaf_contents(map: &ClipMap, l: &CLeaf, p: Vec3) -> u32 {
    if l.brush_node == 0 {
        return 0;
    }
    for k in 0..3 {
        if p[k] <= l.mins[k] || l.maxs[k] <= p[k] {
            return 0;
        }
    }
    brush_node_contents(map, l.brush_node as usize, p)
}

fn brush_node_contents(map: &ClipMap, mut n: usize, p: Vec3) -> u32 {
    let mut out = 0;
    loop {
        match map.brush_nodes[n] {
            BrushNode::Leaf { first, count, .. } => {
                for &b in &map.leaf_brush_list[first as usize..][..count as usize] {
                    let brush = &map.brushes[b as usize];
                    let inside = (0..3).all(|k| brush.mins[k] <= p[k] && p[k] <= brush.maxs[k])
                        && brush.sides.iter().all(|&(plane, _)| {
                            let pl = map.planes[plane as usize];
                            dot(p, pl.normal) <= f64::from(pl.dist)
                        });
                    if inside {
                        out |= brush.contents;
                    }
                }
                return out;
            }
            BrushNode::Split {
                axis,
                dist,
                straddlers,
                children,
                ..
            } => {
                if straddlers {
                    out |= brush_node_contents(map, n + 1, p);
                }
                let side = usize::from(p[axis as usize] <= dist);
                n += children[side] as usize;
            }
        }
    }
}
