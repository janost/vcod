//! Contains routines ported from the Quake III Arena GPL source, Copyright (C) 1999-2005 Id Software, Inc..
//! See NOTICE.
//!
//! Brush clip planes, render triangles and a BVH from the BSP collision
//! lumps, swept by a Q3-style AABB `box_trace`. Layouts and why triangles
//! are required: docs/research/bsp-ibsp59-format.md, "Terrain has no brushes".

use crate::bsp::Bsp;
use glam::Vec3;

/// Shared with the xmodel collision surfaces, which use the same bits.
pub const CONTENTS_SOLID: u32 = 0x1;
const CONTENTS_PLAYERCLIP: u32 = 0x10000;
const CONTENTS_SKY: u32 = 0x800;

/// A brush as clip planes: point p is inside iff n·p <= d for every plane.
pub struct BrushPlanes {
    pub planes: Vec<(Vec3, f32)>,
}

#[derive(Clone, Copy, Debug)]
pub enum Prim {
    Brush(u32),
    Tri(u32),
}

#[derive(Clone, Copy, Debug)]
pub struct Trace {
    pub fraction: f32, // 1.0 = made it to end
    pub endpos: Vec3,
    pub normal: Vec3, // valid when fraction < 1.0
    pub startsolid: bool,
    pub allsolid: bool,
}

pub const SURFACE_CLIP_EPSILON: f32 = 0.125;

/// Q3 `cm_trace.c` `CM_TraceThroughBrush`, on planes already expanded by the box.
fn clip_segment(trace: &mut Trace, start: Vec3, end: Vec3, planes: &[(Vec3, f32)]) {
    let mut enter = -1.0f32;
    let mut leave = 1.0f32;
    let mut clip_normal = Vec3::ZERO;
    let mut getout = false;
    let mut startout = false;

    for &(n, d) in planes {
        let d1 = n.dot(start) - d;
        let d2 = n.dot(end) - d;
        if d2 > 0.0 {
            getout = true;
        }
        if d1 > 0.0 {
            startout = true;
        }
        // entirely in front of this face
        if d1 > 0.0 && (d2 >= SURFACE_CLIP_EPSILON || d2 >= d1) {
            return;
        }
        if d1 <= 0.0 && d2 <= 0.0 {
            continue;
        }
        if d1 > d2 {
            // entering
            let f = (d1 - SURFACE_CLIP_EPSILON) / (d1 - d2);
            if f > enter {
                enter = f;
                clip_normal = n;
            }
        } else {
            // leaving
            let f = (d1 + SURFACE_CLIP_EPSILON) / (d1 - d2);
            if f < leave {
                leave = f;
            }
        }
    }

    if !startout {
        trace.startsolid = true;
        if !getout {
            trace.allsolid = true;
            trace.fraction = 0.0;
        }
        return;
    }
    // `<=`, not Q3's `<`: a zero-thickness triangle's paired face planes make
    // enter == leave for a grazing ray. Brushes have thickness, so unaffected.
    if enter <= leave && enter > -1.0 && enter < trace.fraction {
        trace.fraction = enter.max(0.0);
        trace.normal = clip_normal;
    }
}

/// Q3 `cm_trace.c` offset trick: planes pushed out by the box so the center
/// segment can be clipped.
fn expand_brush(planes: &[(Vec3, f32)], mins: Vec3, maxs: Vec3, out: &mut Vec<(Vec3, f32)>) {
    out.clear();
    for &(n, d) in planes {
        let ofs = Vec3::new(
            if n.x < 0.0 { maxs.x } else { mins.x },
            if n.y < 0.0 { maxs.y } else { mins.y },
            if n.z < 0.0 { maxs.z } else { mins.z },
        );
        out.push((n, d - n.dot(ofs)));
    }
}

/// Minkowski planes for a box-expanded triangle: face, axis bevels, edge x
/// axis bevels. Edge hits report the bevel normal, which lets pmove slide
/// around edges.
fn triangle_planes(tri: &[Vec3; 3], mins: Vec3, maxs: Vec3, out: &mut Vec<(Vec3, f32)>) {
    out.clear();
    let mut push = |n: Vec3| {
        let d_tri = n.dot(tri[0]).max(n.dot(tri[1])).max(n.dot(tri[2]));
        let expand = (-n.x * mins.x).max(-n.x * maxs.x)
            + (-n.y * mins.y).max(-n.y * maxs.y)
            + (-n.z * mins.z).max(-n.z * maxs.z);
        out.push((n, d_tri + expand));
    };
    let face = (tri[1] - tri[0]).cross(tri[2] - tri[0]).normalize();
    push(face);
    push(-face);
    for axis in [Vec3::X, Vec3::Y, Vec3::Z] {
        push(axis);
        push(-axis);
    }
    for k in 0..3 {
        let edge = tri[(k + 1) % 3] - tri[k];
        for axis in [Vec3::X, Vec3::Y, Vec3::Z] {
            let c = edge.cross(axis);
            if c.length_squared() > 1e-8 {
                let c = c.normalize();
                push(c);
                push(-c);
            }
        }
    }
}

struct BvhNode {
    lo: Vec3,
    hi: Vec3,
    /// leaf: absolute prim index; internal: left child
    first: u32,
    /// internal: right child; leaf: unused
    second: u32,
    /// 0 = internal, otherwise leaf covering `count` prims starting at `first`
    count: u32,
}

pub struct CollisionWorld {
    pub brushes: Vec<BrushPlanes>,
    pub tris: Vec<[Vec3; 3]>,
    nodes: Vec<BvhNode>,
    prims: Vec<(Prim, Vec3, Vec3)>,
}

const AXES: [Vec3; 3] = [Vec3::X, Vec3::Y, Vec3::Z];

/// Drops slivers, pads the AABB by 0.25 so the BVH query finds a triangle
/// the box merely touches.
fn push_tri(tris: &mut Vec<[Vec3; 3]>, prims: &mut Vec<(Prim, Vec3, Vec3)>, [a, b, c]: [Vec3; 3]) {
    if (b - a).cross(c - a).length_squared() < 1e-6 {
        return;
    }
    let lo = a.min(b).min(c) - Vec3::splat(0.25);
    let hi = a.max(b).max(c) + Vec3::splat(0.25);
    prims.push((Prim::Tri(tris.len() as u32), lo, hi));
    tris.push([a, b, c]);
}

impl CollisionWorld {
    /// `extra_tris` are world-space triangles from outside the BSP (the props'
    /// collision meshes, `props::collision_tris`); they get the same treatment
    /// as soup triangles.
    pub fn build(bsp: &Bsp, extra_tris: &[[Vec3; 3]]) -> Self {
        let mut brushes = Vec::new();
        let mut prims = Vec::new();

        let model = &bsp.models[0];
        let brush_range =
            model.first_brush as usize..(model.first_brush + model.num_brushes) as usize;
        for b in &bsp.brushes[brush_range] {
            let content = bsp.materials[b.material as usize].content_flags;
            if content & (CONTENTS_SOLID | CONTENTS_PLAYERCLIP) == 0 {
                continue;
            }
            let sides = &bsp.brush_sides[b.first_side as usize..][..b.num_sides as usize];
            let mut planes = Vec::with_capacity(sides.len());
            let mut lo = Vec3::ZERO;
            let mut hi = Vec3::ZERO;
            for axis in 0..3 {
                let axis_lo = f32::from_bits(sides[axis * 2].plane_or_dist);
                let axis_hi = f32::from_bits(sides[axis * 2 + 1].plane_or_dist);
                planes.push((-AXES[axis], -axis_lo));
                planes.push((AXES[axis], axis_hi));
                lo[axis] = axis_lo;
                hi[axis] = axis_hi;
            }
            for s in &sides[6..] {
                let p = &bsp.planes[s.plane_or_dist as usize];
                planes.push((Vec3::from_array(p.normal), p.dist));
            }
            let idx = brushes.len() as u32;
            brushes.push(BrushPlanes { planes });
            prims.push((Prim::Brush(idx), lo, hi));
        }

        let mut tris = Vec::new();
        for soup in &bsp.soups {
            let content = bsp.materials[soup.material as usize].content_flags;
            if content & CONTENTS_SKY != 0 {
                continue;
            }
            let idx = &bsp.indices[soup.first_index as usize..][..soup.index_count as usize];
            for tri in idx.as_chunks::<3>().0 {
                let p = |i: usize| {
                    Vec3::from_array(bsp.verts[soup.first_vertex as usize + tri[i] as usize].pos)
                };
                push_tri(&mut tris, &mut prims, [p(0), p(1), p(2)]);
            }
        }
        for t in extra_tris {
            push_tri(&mut tris, &mut prims, *t);
        }

        let mut nodes = Vec::new();
        if !prims.is_empty() {
            build_bvh(&mut prims, 0, &mut nodes);
        }

        CollisionWorld {
            brushes,
            tris,
            nodes,
            prims,
        }
    }

    /// `mins == maxs == ZERO` is a ray trace.
    pub fn box_trace(&self, start: Vec3, end: Vec3, mins: Vec3, maxs: Vec3) -> Trace {
        let mut trace = Trace {
            fraction: 1.0,
            endpos: end,
            normal: Vec3::ZERO,
            startsolid: false,
            allsolid: false,
        };

        if !self.nodes.is_empty() {
            let mut scratch = Vec::new();
            self.trace_node(0, start, end, mins, maxs, &mut trace, &mut scratch);
        }
        trace.endpos = start + (end - start) * trace.fraction;
        trace
    }

    /// Nearest child first; the sweep box shrinks with `trace.fraction`, so
    /// prims past the current hit are never clipped.
    #[allow(clippy::too_many_arguments)]
    fn trace_node(
        &self,
        i: u32,
        start: Vec3,
        end: Vec3,
        mins: Vec3,
        maxs: Vec3,
        trace: &mut Trace,
        scratch: &mut Vec<(Vec3, f32)>,
    ) {
        let node = &self.nodes[i as usize];
        let cur_end = start + (end - start) * trace.fraction;
        let lo = start.min(cur_end) + mins - Vec3::ONE;
        let hi = start.max(cur_end) + maxs + Vec3::ONE;
        if !(lo.cmple(node.hi).all() && hi.cmpge(node.lo).all()) {
            return;
        }
        if node.count > 0 {
            let first = node.first as usize;
            for (prim, _, _) in &self.prims[first..first + node.count as usize] {
                match *prim {
                    Prim::Brush(b) => {
                        expand_brush(&self.brushes[b as usize].planes, mins, maxs, scratch);
                        clip_segment(trace, start, end, scratch);
                    }
                    Prim::Tri(t) => {
                        triangle_planes(&self.tris[t as usize], mins, maxs, scratch);
                        clip_segment(trace, start, end, scratch);
                    }
                }
            }
            return;
        }
        let dir = end - start;
        let along = |n: u32| {
            let node = &self.nodes[n as usize];
            ((node.lo + node.hi) * 0.5 - start).dot(dir)
        };
        let (near, far) = if along(node.first) <= along(node.second) {
            (node.first, node.second)
        } else {
            (node.second, node.first)
        };
        self.trace_node(near, start, end, mins, maxs, trace, scratch);
        self.trace_node(far, start, end, mins, maxs, trace, scratch);
    }

    /// Only the tests call this now; `box_trace` walks the BVH itself via `trace_node`.
    #[cfg(test)]
    fn candidates(&self, lo: Vec3, hi: Vec3, out: &mut Vec<Prim>) {
        if self.nodes.is_empty() {
            return;
        }
        // the root is nodes[0]: build() makes one top-level build_bvh call
        let mut stack = vec![0u32];
        while let Some(i) = stack.pop() {
            let node = &self.nodes[i as usize];
            if !(lo.cmple(node.hi).all() && hi.cmpge(node.lo).all()) {
                continue;
            }
            if node.count > 0 {
                let start = node.first as usize;
                for (prim, _, _) in &self.prims[start..start + node.count as usize] {
                    out.push(*prim);
                }
            } else {
                stack.push(node.first);
                stack.push(node.second);
            }
        }
    }
}

/// `base` is the absolute index of prims[0] within CollisionWorld::prims.
fn build_bvh(prims: &mut [(Prim, Vec3, Vec3)], base: u32, nodes: &mut Vec<BvhNode>) -> u32 {
    let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
    for (_, plo, phi) in prims.iter() {
        lo = lo.min(*plo);
        hi = hi.max(*phi);
    }
    let idx = nodes.len() as u32;
    nodes.push(BvhNode {
        lo,
        hi,
        first: base,
        second: 0,
        count: prims.len() as u32,
    });
    if prims.len() <= 4 {
        return idx;
    }
    let extent = hi - lo;
    let axis = if extent.x >= extent.y && extent.x >= extent.z {
        0
    } else if extent.y >= extent.z {
        1
    } else {
        2
    };
    let mid = prims.len() / 2;
    prims.select_nth_unstable_by(mid, |x, y| {
        let cx = (x.1[axis] + x.2[axis]) * 0.5;
        let cy = (y.1[axis] + y.2[axis]) * 0.5;
        cx.total_cmp(&cy)
    });
    let (l, r) = prims.split_at_mut(mid);
    let li = build_bvh(l, base, nodes);
    let ri = build_bvh(r, base + mid as u32, nodes);
    nodes[idx as usize].first = li;
    nodes[idx as usize].second = ri;
    nodes[idx as usize].count = 0;
    idx
}

/// Test helper: flat solid floor (-1024..1024, top at z=0) plus extra axial
/// solid brushes. Always compiled: the client's fx tests use it.
#[doc(hidden)]
pub fn test_world(extra: &[(Vec3, Vec3)]) -> CollisionWorld {
    let mut boxes = vec![(
        Vec3::new(-1024.0, -1024.0, -16.0),
        Vec3::new(1024.0, 1024.0, 0.0),
    )];
    boxes.extend_from_slice(extra);
    let side = |v: f32| crate::bsp::BrushSide {
        plane_or_dist: v.to_bits(),
        material: 0,
    };
    let mut brush_sides = Vec::new();
    let mut brushes = Vec::new();
    for (i, (mins, maxs)) in boxes.iter().enumerate() {
        brushes.push(crate::bsp::Brush {
            first_side: (i * 6) as u32,
            num_sides: 6,
            material: 0,
        });
        for axis in 0..3 {
            brush_sides.push(side(mins[axis]));
            brush_sides.push(side(maxs[axis]));
        }
    }
    let n = brushes.len() as u32;
    CollisionWorld::build(
        &crate::bsp::Bsp {
            materials: vec![crate::bsp::Material {
                name: "textures/test/solid".into(),
                surface_flags: 0,
                content_flags: 0x1,
            }],
            lightmaps: vec![],
            soups: vec![],
            verts: vec![],
            indices: vec![],
            entities: String::new(),
            planes: vec![],
            brush_sides,
            brushes,
            models: vec![crate::bsp::Model {
                mins: [-1024.0; 3],
                maxs: [1024.0; 3],
                first_soup: 0,
                num_soups: 0,
                first_brush: 0,
                num_brushes: n,
            }],
            cull_groups: vec![],
            cull_indices: vec![],
            portal_verts: vec![],
            aabb_nodes: vec![],
            cells: vec![],
            portals: vec![],
            nodes: vec![],
            leafs: vec![],
        },
        &[],
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Triangles from outside the BSP (props) clip like soup triangles.
    #[test]
    fn extra_triangles_stop_a_trace() {
        let wall = [
            [
                Vec3::new(5000.0, -50.0, 0.0),
                Vec3::new(5000.0, 50.0, 0.0),
                Vec3::new(5000.0, 50.0, 100.0),
            ],
            [
                Vec3::new(5000.0, -50.0, 0.0),
                Vec3::new(5000.0, 50.0, 100.0),
                Vec3::new(5000.0, -50.0, 100.0),
            ],
        ];
        let (start, end) = (Vec3::new(4900.0, 0.0, 50.0), Vec3::new(5100.0, 0.0, 50.0));
        let bare = CollisionWorld::build(&tiny_world(), &[]);
        assert_eq!(
            bare.box_trace(start, end, Vec3::ZERO, Vec3::ZERO).fraction,
            1.0
        );
        let world = CollisionWorld::build(&tiny_world(), &wall);
        let t = world.box_trace(start, end, Vec3::ZERO, Vec3::ZERO);
        assert!(
            t.fraction < 1.0 && (t.endpos.x - 5000.0).abs() < 0.5,
            "{t:?}"
        );
        assert!(t.normal.abs_diff_eq(-Vec3::X, 1e-4), "{t:?}");
    }
    use crate::bsp::{self, Bsp};
    use glam::Vec3;

    /// One axial brush (-64,-64,-16)..(64,64,0), one triangle at z=10 over x,y in 100..200.
    pub(crate) fn tiny_world() -> Bsp {
        let dist = |v: f32| v.to_bits();
        let side = |v: f32| bsp::BrushSide {
            plane_or_dist: dist(v),
            material: 0,
        };
        Bsp {
            materials: vec![bsp::Material {
                name: "textures/test/solid".into(),
                surface_flags: 0,
                content_flags: 0x1,
            }],
            lightmaps: vec![],
            soups: vec![bsp::TriangleSoup {
                material: 0,
                lightmap: bsp::NO_LIGHTMAP,
                first_vertex: 0,
                vertex_count: 3,
                index_count: 3,
                first_index: 0,
            }],
            verts: vec![
                vert([100.0, 100.0, 10.0]),
                vert([200.0, 100.0, 10.0]),
                vert([100.0, 200.0, 10.0]),
            ],
            indices: vec![0, 1, 2],
            entities: String::new(),
            planes: vec![],
            brush_sides: vec![
                side(-64.0),
                side(64.0), // xmin, xmax
                side(-64.0),
                side(64.0), // ymin, ymax
                side(-16.0),
                side(0.0), // zmin, zmax
            ],
            brushes: vec![bsp::Brush {
                first_side: 0,
                num_sides: 6,
                material: 0,
            }],
            models: vec![bsp::Model {
                mins: [-64.0, -64.0, -16.0],
                maxs: [64.0, 64.0, 0.0],
                first_soup: 0,
                num_soups: 0,
                first_brush: 0,
                num_brushes: 1,
            }],
            cull_groups: vec![],
            cull_indices: vec![],
            portal_verts: vec![],
            aabb_nodes: vec![],
            cells: vec![],
            portals: vec![],
            nodes: vec![],
            leafs: vec![],
        }
    }

    fn vert(pos: [f32; 3]) -> bsp::DrawVert {
        bsp::DrawVert {
            pos,
            uv: [0.0; 2],
            lm_uv: [0.0; 2],
            normal: [0.0, 0.0, 1.0],
            color: [255; 4],
        }
    }

    #[test]
    fn builds_brush_planes_from_axial_bounds() {
        let world = CollisionWorld::build(&tiny_world(), &[]);
        assert_eq!(world.brushes.len(), 1);
        let planes = &world.brushes[0].planes;
        assert_eq!(planes.len(), 6);
        // zmax face: normal +Z, dist 0
        assert!(planes
            .iter()
            .any(|(n, d)| n.abs_diff_eq(Vec3::Z, 1e-6) && *d == 0.0));
        // xmin face: normal -X, dist 64 (inside test: -x <= 64  =>  x >= -64)
        assert!(planes
            .iter()
            .any(|(n, d)| n.abs_diff_eq(-Vec3::X, 1e-6) && *d == 64.0));
    }

    #[test]
    fn harvests_triangles_and_answers_aabb_queries() {
        let world = CollisionWorld::build(&tiny_world(), &[]);
        assert_eq!(world.tris.len(), 1);
        let mut out = Vec::new();
        world.candidates(
            Vec3::new(140.0, 140.0, 0.0),
            Vec3::new(150.0, 150.0, 20.0),
            &mut out,
        );
        assert!(out.iter().any(|p| matches!(p, Prim::Tri(_))));
        out.clear();
        world.candidates(
            Vec3::new(0.0, 0.0, -8.0),
            Vec3::new(1.0, 1.0, 8.0),
            &mut out,
        );
        assert!(out.iter().any(|p| matches!(p, Prim::Brush(_))));
        out.clear();
        world.candidates(
            Vec3::new(9000.0, 9000.0, 0.0),
            Vec3::new(9001.0, 9001.0, 1.0),
            &mut out,
        );
        assert!(out.is_empty());
    }

    #[test]
    fn builds_mp_pavlov_world() {
        let Some(data) = crate::testing::real_bsp() else {
            return;
        };
        let bsp = crate::bsp::parse(&data).unwrap();
        let world = CollisionWorld::build(&bsp, &[]);
        // solid+playerclip subset of model 0's 7575 brushes
        assert!(!world.brushes.is_empty() && world.brushes.len() <= 7575);
        assert!(world.tris.len() > 10_000);
        // a query around a known spawn; only holds if candidates() walks from the root
        let mut out = Vec::new();
        world.candidates(
            Vec3::new(-9100.0, 8950.0, -50.0),
            Vec3::new(-8800.0, 9250.0, 150.0),
            &mut out,
        );
        assert!(!out.is_empty());
    }

    /// Six triangles 1000 units apart along X: the median split yields a
    /// 3-node BVH, so root (0) and last-pushed node (2) differ and a walk
    /// that starts anywhere but the root misses the low-X leaf.
    fn spread_tris_world() -> Bsp {
        const N: usize = 6;
        let mut verts = Vec::with_capacity(N * 3);
        let mut indices = Vec::with_capacity(N * 3);
        let mut soups = Vec::with_capacity(N);
        for i in 0..N {
            let ox = i as f32 * 1000.0;
            verts.push(vert([ox, 0.0, 10.0]));
            verts.push(vert([ox + 10.0, 0.0, 10.0]));
            verts.push(vert([ox, 10.0, 10.0]));
            indices.extend_from_slice(&[0, 1, 2]);
            soups.push(bsp::TriangleSoup {
                material: 0,
                lightmap: bsp::NO_LIGHTMAP,
                first_vertex: (i * 3) as u32,
                vertex_count: 3,
                index_count: 3,
                first_index: (i * 3) as u32,
            });
        }
        Bsp {
            materials: vec![bsp::Material {
                name: "textures/test/solid".into(),
                surface_flags: 0,
                content_flags: 0x1,
            }],
            lightmaps: vec![],
            soups,
            verts,
            indices,
            entities: String::new(),
            planes: vec![],
            brush_sides: vec![],
            brushes: vec![],
            models: vec![bsp::Model {
                mins: [0.0, 0.0, 0.0],
                maxs: [0.0, 0.0, 0.0],
                first_soup: 0,
                num_soups: 0,
                first_brush: 0,
                num_brushes: 0,
            }],
            cull_groups: vec![],
            cull_indices: vec![],
            portal_verts: vec![],
            aabb_nodes: vec![],
            cells: vec![],
            portals: vec![],
            nodes: vec![],
            leafs: vec![],
        }
    }

    fn world() -> CollisionWorld {
        CollisionWorld::build(&tiny_world(), &[])
    }

    #[test]
    fn ray_down_hits_brush_top() {
        let t = world().box_trace(
            Vec3::new(0.0, 0.0, 100.0),
            Vec3::new(0.0, 0.0, -100.0),
            Vec3::ZERO,
            Vec3::ZERO,
        );
        assert!(t.fraction < 1.0 && !t.startsolid);
        assert!(t.normal.abs_diff_eq(Vec3::Z, 1e-5));
        // stops SURFACE_CLIP_EPSILON-ish above the face at z=0
        assert!((t.endpos.z - 0.0).abs() < 0.2, "endpos {}", t.endpos);
    }

    #[test]
    fn box_down_rests_on_brush_by_its_mins() {
        let t = world().box_trace(
            Vec3::new(0.0, 0.0, 100.0),
            Vec3::new(0.0, 0.0, -100.0),
            Vec3::new(-15.0, -15.0, 0.0),
            Vec3::new(15.0, 15.0, 70.0),
        );
        // bbox mins.z = 0 => origin comes to rest at the face, z ~ 0
        assert!(t.fraction < 1.0 && (t.endpos.z - 0.0).abs() < 0.2);
    }

    #[test]
    fn miss_returns_full_fraction() {
        let t = world().box_trace(
            Vec3::new(500.0, 500.0, 100.0),
            Vec3::new(500.0, 500.0, 50.0),
            Vec3::ZERO,
            Vec3::ZERO,
        );
        assert_eq!(t.fraction, 1.0);
        assert_eq!(t.endpos, Vec3::new(500.0, 500.0, 50.0));
    }

    #[test]
    fn start_inside_brush_is_startsolid() {
        let t = world().box_trace(
            Vec3::new(0.0, 0.0, -8.0),
            Vec3::new(0.0, 0.0, -8.0),
            Vec3::ZERO,
            Vec3::ZERO,
        );
        assert!(t.startsolid && t.allsolid && t.fraction == 0.0);
    }

    #[test]
    fn ray_down_hits_triangle() {
        let t = world().box_trace(
            Vec3::new(120.0, 120.0, 100.0),
            Vec3::new(120.0, 120.0, -100.0),
            Vec3::ZERO,
            Vec3::ZERO,
        );
        assert!(t.fraction < 1.0);
        assert!(t.normal.abs_diff_eq(Vec3::Z, 1e-4));
        assert!((t.endpos.z - 10.0).abs() < 0.2);
    }

    #[test]
    fn box_sideways_into_triangle_edge_stops_outside() {
        let t = world().box_trace(
            Vec3::new(50.0, 120.0, 10.0),
            Vec3::new(150.0, 120.0, 10.0),
            Vec3::splat(-5.0),
            Vec3::splat(5.0),
        );
        assert!(t.fraction < 1.0);
        assert!(
            t.endpos.x <= 95.5,
            "box center should stop before the edge, got {}",
            t.endpos.x
        );
    }

    #[test]
    fn answers_aabb_queries_across_a_multi_node_bvh() {
        let world = CollisionWorld::build(&spread_tris_world(), &[]);
        assert_eq!(world.tris.len(), 6);

        // low-X triangle: only reachable from the true root
        let mut out = Vec::new();
        world.candidates(
            Vec3::new(-5.0, -5.0, 0.0),
            Vec3::new(15.0, 15.0, 20.0),
            &mut out,
        );
        assert!(out.iter().any(|p| matches!(p, Prim::Tri(0))));

        // high-X triangle (index 5)
        out.clear();
        let ox = 5.0 * 1000.0;
        world.candidates(
            Vec3::new(ox - 5.0, -5.0, 0.0),
            Vec3::new(ox + 15.0, 15.0, 20.0),
            &mut out,
        );
        assert!(out.iter().any(|p| matches!(p, Prim::Tri(5))));

        // nowhere near any triangle
        out.clear();
        world.candidates(
            Vec3::new(50_000.0, 50_000.0, 0.0),
            Vec3::new(50_001.0, 50_001.0, 1.0),
            &mut out,
        );
        assert!(out.is_empty());
    }

    /// Every prim clipped, no traversal: the oracle for the ordered walk.
    fn brute_trace(
        world: &CollisionWorld,
        start: Vec3,
        end: Vec3,
        mins: Vec3,
        maxs: Vec3,
    ) -> Trace {
        let mut trace = Trace {
            fraction: 1.0,
            endpos: end,
            normal: Vec3::ZERO,
            startsolid: false,
            allsolid: false,
        };
        let mut scratch = Vec::new();
        for (prim, _, _) in &world.prims {
            match *prim {
                Prim::Brush(i) => {
                    expand_brush(&world.brushes[i as usize].planes, mins, maxs, &mut scratch);
                    clip_segment(&mut trace, start, end, &scratch);
                }
                Prim::Tri(i) => {
                    triangle_planes(&world.tris[i as usize], mins, maxs, &mut scratch);
                    clip_segment(&mut trace, start, end, &scratch);
                }
            }
        }
        trace.endpos = start + (end - start) * trace.fraction;
        trace
    }

    /// Deterministic xorshift, so a failure reproduces.
    fn rng(seed: &mut u64) -> f32 {
        *seed ^= *seed << 13;
        *seed ^= *seed >> 7;
        *seed ^= *seed << 17;
        (*seed >> 40) as f32 / (1u64 << 24) as f32
    }

    fn check_against_brute(world: &CollisionWorld, lo: Vec3, hi: Vec3, sweeps: usize, seed: u64) {
        let mut s = seed;
        let mut hits = 0;
        for _ in 0..sweeps {
            let r = |s: &mut u64| Vec3::new(rng(s), rng(s), rng(s));
            let start = lo + (hi - lo) * r(&mut s);
            let end = lo + (hi - lo) * r(&mut s);
            let (mins, maxs) = if rng(&mut s) < 0.5 {
                (Vec3::ZERO, Vec3::ZERO)
            } else {
                (Vec3::new(-15.0, -15.0, 0.0), Vec3::new(15.0, 15.0, 60.0))
            };
            let a = world.box_trace(start, end, mins, maxs);
            let b = brute_trace(world, start, end, mins, maxs);
            assert!(
                (a.fraction - b.fraction).abs() <= 1e-5,
                "{start} -> {end}: {a:?} vs {b:?}"
            );
            assert_eq!(
                (a.startsolid, a.allsolid),
                (b.startsolid, b.allsolid),
                "{start} -> {end}"
            );
            if a.fraction < 1.0 {
                hits += 1;
                assert!(
                    a.normal.abs_diff_eq(b.normal, 1e-4),
                    "{start} -> {end}: {a:?} vs {b:?}"
                );
            }
        }
        assert!(hits > 20, "too few hits to trust the oracle: {hits}");
    }

    #[test]
    fn ordered_walk_matches_brute_force_on_synthetic_worlds() {
        let w = spread_tris_world();
        // bounds hug triangle 0 so 300 sweeps land plenty of hits
        check_against_brute(
            &CollisionWorld::build(&w, &[]),
            Vec3::new(-20.0, -20.0, -20.0),
            Vec3::new(30.0, 30.0, 30.0),
            300,
            0x9E3779B97F4A7C15,
        );
        let w = test_world(&[(Vec3::new(50.0, -400.0, 0.0), Vec3::new(100.0, 400.0, 100.0))]);
        check_against_brute(
            &w,
            Vec3::new(-300.0, -300.0, -50.0),
            Vec3::new(300.0, 300.0, 200.0),
            300,
            0xD1B54A32D192ED03,
        );
    }

    #[test]
    fn ordered_walk_matches_brute_force_on_mp_pavlov() {
        let Some(data) = crate::testing::real_bsp() else {
            return;
        };
        let bsp = crate::bsp::parse(&data).unwrap();
        let world = CollisionWorld::build(&bsp, &[]);
        let (lo, hi) = crate::mesh::map_bounds(&bsp);
        // short sweeps so most of them hit something
        let mut s = 0x2545F4914F6CDD1Du64;
        let mut hits = 0;
        for _ in 0..200 {
            let r = |s: &mut u64| Vec3::new(rng(s), rng(s), rng(s));
            let start = Vec3::from(lo) + (Vec3::from(hi) - Vec3::from(lo)) * r(&mut s);
            let end = start + (r(&mut s) - Vec3::splat(0.5)) * 600.0;
            let a = world.box_trace(start, end, Vec3::ZERO, Vec3::ZERO);
            let b = brute_trace(&world, start, end, Vec3::ZERO, Vec3::ZERO);
            assert!(
                (a.fraction - b.fraction).abs() <= 1e-5,
                "{start} -> {end}: {a:?} vs {b:?}"
            );
            assert_eq!((a.startsolid, a.allsolid), (b.startsolid, b.allsolid));
            hits += (a.fraction < 1.0) as usize;
        }
        assert!(hits > 20, "{hits}");
    }
}
