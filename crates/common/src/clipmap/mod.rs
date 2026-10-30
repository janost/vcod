//! The CoD2-family collision model and its world traces, as
//! `cod2_lnxded` walks them: brushes in each leaf's k-d tree order, the
//! collision AABB trees with their triangles, edges, vertices and borders,
//! and a capsule for every box (`docs/research/cod2-d3dbsp-format.md`,
//! "What the trace reads" and "The trace"). A game's loader fills a
//! [`ClipMap`] (`vcod_cod2::collision::build` from a `.d3dbsp`); the
//! kernels read only the runtime records here. Model 0 only: a submodel
//! clips through its entity, which the server links.
//!
//! The arithmetic follows the Linux build's x87 code: an expression stays
//! extended (`f64` here) and rounds to `f32` where the binary stores a
//! float variable.

mod mesh;
mod mesh4;
mod trace;

use std::cell::RefCell;

use glam::Vec3;

use crate::collision::{WorldClip, WorldHit};

/// `surfaceparm` content bits, from the material compiler's table (Mac
/// `.data` 0x348820; `cod2-d3dbsp-format.md`, "Contents and surface flags").
pub const CONTENTS_SOLID: u32 = 0x1;
pub const CONTENTS_FOLIAGE: u32 = 0x2;
pub const CONTENTS_GLASS: u32 = 0x10;
pub const CONTENTS_WATER: u32 = 0x20;
pub const CONTENTS_CANSHOOTCLIP: u32 = 0x40;
pub const CONTENTS_MISSILECLIP: u32 = 0x80;
pub const CONTENTS_VEHICLECLIP: u32 = 0x200;
pub const CONTENTS_ITEMCLIP: u32 = 0x400;
pub const CONTENTS_SKY: u32 = 0x800;
pub const CONTENTS_AI_NOSIGHT: u32 = 0x1000;
pub const CONTENTS_CLIPSHOT: u32 = 0x2000;
pub const CONTENTS_PLAYERCLIP: u32 = 0x1_0000;
pub const CONTENTS_MONSTERCLIP: u32 = 0x2_0000;
/// `mantleOn` and `mantleOver`.
pub const CONTENTS_MANTLE: u32 = 0x100_0000;
/// A player's own contents; no material carries it.
pub const CONTENTS_BODY: u32 = 0x200_0000;
pub const CONTENTS_DETAIL: u32 = 0x800_0000;
pub const CONTENTS_STRUCTURAL: u32 = 0x1000_0000;
pub const CONTENTS_NODROP: u32 = 0x8000_0000;

/// `surfaceparm` surface bits (same table); the sound material rides bits
/// 20-24.
pub const SURF_NODAMAGE: u32 = 0x1;
pub const SURF_SLICK: u32 = 0x2;
pub const SURF_SKY: u32 = 0x4;
pub const SURF_LADDER: u32 = 0x8;
pub const SURF_NOIMPACT: u32 = 0x10;
pub const SURF_NOMARKS: u32 = 0x20;
pub const SURF_NOSTEPS: u32 = 0x2000;
pub const SURF_NONSOLID: u32 = 0x4000;
pub const SURF_MANTLE_ON: u32 = 0x200_0000;
pub const SURF_MANTLE_OVER: u32 = 0x400_0000;

/// A live player's `pm->tracemask` (`ClientThink_real`, Mac 1.3): SOLID,
/// GLASS, PLAYERCLIP, BODY and 0x800000, which no material carries.
pub const MASK_PLAYERSOLID: u32 = 0x281_0011;
/// A dead player's (`pm_type` 6 and up): the live mask without BODY.
pub const MASK_DEADSOLID: u32 = 0x81_0011;
/// `Bullet_Fire_Extended`'s and `Melee_Trace`'s `G_LocationalTrace` mask:
/// SOLID, GLASS, WATER, SKY, CLIPSHOT, BODY and 0x800000.
pub const MASK_SHOT: u32 = 0x280_2831;
/// `CanDamage`'s blast probes: the shot mask with MISSILECLIP for WATER and
/// no BODY.
pub const MASK_BLAST: u32 = 0x80_2891;

/// What a trace met: `trace_t` without `entityNum` and the hit-part fields,
/// which the entity clip fills.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ClipTrace {
    /// 1 when the sweep reached its end.
    pub fraction: f32,
    pub normal: Vec3,
    pub surface_flags: u32,
    pub contents: u32,
    /// The lump-0 material the contact reported.
    pub material: Option<u16>,
    pub allsolid: bool,
    pub startsolid: bool,
    /// CoD4's `trace_t.walkable`: set by an indexed triangle's walkable
    /// edge bits ([`IndexedMesh::walkable`]), cleared by a brush or a face.
    /// CoD2's kernels never set it; `vcod_cod4::collision` applies
    /// `CM_Trace`'s closing rule over it.
    pub walkable: bool,
}

impl ClipTrace {
    /// `CM_BoxTrace`'s fresh trace: zeroed, fraction 1.
    pub fn clear() -> ClipTrace {
        ClipTrace {
            fraction: 1.0,
            normal: Vec3::ZERO,
            surface_flags: 0,
            contents: 0,
            material: None,
            allsolid: false,
            startsolid: false,
            walkable: false,
        }
    }
}

/// A plane with `CMod_LoadPlanes`' type byte: 0..2 when that normal
/// component is exactly 1, else 3; and its sign bits.
#[derive(Clone, Copy, Debug)]
pub struct CPlane {
    pub normal: Vec3,
    pub dist: f32,
    pub kind: u8,
    pub signbits: u8,
}

#[derive(Clone, Copy, Debug)]
pub struct CNode {
    pub plane: u32,
    pub children: [i32; 2],
}

/// The 0x2c-byte runtime leaf `CMod_LoadBrushRelated` builds.
#[derive(Clone, Copy, Debug)]
pub struct CLeaf {
    pub first_aabb: u16,
    pub aabb_count: u16,
    /// OR of its brushes' masked contents.
    pub brush_contents: u32,
    /// OR of its AABB roots' materials' contents.
    pub terrain_contents: u32,
    /// Its brushes' union widened by 0.125.
    pub mins: Vec3,
    pub maxs: Vec3,
    /// Root of its brush tree in [`ClipMap::brush_nodes`]; 0 for none.
    pub brush_node: u32,
}

/// A brush: its axial bounds, masked contents, the sides past the six
/// axial ones, and the axial sides' materials (`[is_max][axis]`).
#[derive(Clone, Debug)]
pub struct CBrush {
    pub mins: Vec3,
    pub maxs: Vec3,
    pub contents: u32,
    pub sides: Vec<(u32, u16)>,
    pub axial_material: [[u16; 3]; 2],
}

/// A lump-30 edge as `CMod_LoadCollisionEdges` keeps it: `dir` scaled by
/// `1 / length`.
#[derive(Clone, Copy, Debug)]
pub struct CEdge {
    pub origin: Vec3,
    pub axis: [Vec3; 2],
    pub dir_scaled: Vec3,
}

/// Lump 31.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CollTri {
    /// Normal, dist.
    pub plane: [f32; 4],
    /// `s = dot(p, svec.xyz) - svec.w`
    pub svec: [f32; 4],
    /// `t = dot(p, tvec.xyz) - tvec.w`
    pub tvec: [f32; 4],
    /// Into lump 29; -1 = none.
    pub verts: [i32; 3],
    /// Into lump 30; -1 = none.
    pub edges: [i32; 3],
}

/// Lump 32.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CollBorder {
    /// a, b, c: `a*x + b*y = c`.
    pub dist_eq: [f32; 3],
    pub z_base: f32,
    pub z_slope: f32,
    pub start: f32,
    pub length: f32,
}

/// Lump 33.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CollPartition {
    pub unused: u16,
    pub tri_count: u8,
    pub border_count: u8,
    pub first_tri: i32,
    pub first_border: i32,
}

/// Lump 34.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CollAabbTree {
    pub origin: Vec3,
    pub half_size: Vec3,
    /// Into lump 0.
    pub material: u16,
    /// 0 for a leaf.
    pub child_count: u16,
    /// First child (into lump 34) with children, else a partition (lump 33).
    pub child_or_partition: i32,
}

/// One 20-byte runtime node, in retail's allocation (preorder) order.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum BrushNode {
    /// Clips `count` brushes of the list from `first`, in order.
    Leaf {
        first: u32,
        count: u16,
        contents: u32,
    },
    /// Splits at `dist` on `axis`. With `straddlers` (retail's count -1) the
    /// next node is the subtree of the brushes crossing `dist`, walked
    /// first. `children` are offsets in nodes from this one: [0] the side
    /// above `dist`, [1] the side below.
    Split {
        axis: u8,
        dist: f32,
        clearance: f32,
        straddlers: bool,
        children: [u16; 2],
        contents: u32,
    },
}

impl BrushNode {
    pub fn contents(&self) -> u32 {
        match *self {
            BrushNode::Leaf { contents, .. } | BrushNode::Split { contents, .. } => contents,
        }
    }
}

/// The world's collision model, built from the map's collision lumps.
#[derive(Clone, Debug)]
pub struct ClipMap {
    pub planes: Vec<CPlane>,
    pub nodes: Vec<CNode>,
    pub leafs: Vec<CLeaf>,
    /// Every leaf's brush tree, node 0 a placeholder as retail's pool has.
    pub brush_nodes: Vec<BrushNode>,
    /// Lump 27 as the trees rewrote it.
    pub leaf_brush_list: Vec<u16>,
    pub brushes: Vec<CBrush>,
    /// `(surface_flags, content_flags)` per lump-0 material.
    pub materials: Vec<(u32, u32)>,
    pub verts: Vec<Vec3>,
    pub edges: Vec<CEdge>,
    pub tris: Vec<CollTri>,
    pub borders: Vec<CollBorder>,
    pub partitions: Vec<CollPartition>,
    pub aabbs: Vec<CollAabbTree>,
    /// CoD4's triangles, which carry no planes: with it the AABB trees'
    /// partitions index these, not [`tris`](Self::tris).
    pub indexed: Option<IndexedMesh>,
    /// Per BSP node, the box round everything a sweep can clip under it,
    /// built on the first sweep ([`ClipMap::node_reach`]).
    pub reach: std::sync::OnceLock<Vec<[Vec3; 2]>>,
}

/// CoD4 1.7's collision triangles (`clipMap_t` `verts`, `triIndices`,
/// `triEdgeIsWalkable`; `docs/research/cod4-collision.md`).
#[derive(Clone, Debug, Default)]
pub struct IndexedMesh {
    pub verts: Vec<Vec3>,
    pub tris: Vec<[u16; 3]>,
    /// One bit per triangle edge, bit `3 tri + k` for the edge opposite
    /// vertex `k`, LSB first within each byte.
    pub walkable: Vec<u8>,
}

impl IndexedMesh {
    /// Whether edge bit `bit` (`3 tri + k`) is set.
    pub fn edge_walkable(&self, bit: usize) -> bool {
        self.walkable
            .get(bit >> 3)
            .is_some_and(|b| b >> (bit & 7) & 1 != 0)
    }
}

impl ClipMap {
    /// Per BSP node, the union of its leaves' brush boxes (`CLeaf::mins`,
    /// `maxs`) and AABB tree roots: the leaf tests a sweep runs under it
    /// ([`trace::Cx`]'s brush box cull and `CM_CullBox`) pass only for a
    /// sweep whose box meets it. An empty node's box is inverted.
    pub(crate) fn node_reach(&self) -> &[[Vec3; 2]] {
        self.reach.get_or_init(|| {
            let empty = [Vec3::splat(f32::INFINITY), Vec3::splat(f32::NEG_INFINITY)];
            let leaf = |l: &CLeaf| {
                let mut b = empty;
                if l.brush_contents != 0 {
                    b = [l.mins, l.maxs];
                }
                for t in &self.aabbs[l.first_aabb as usize..][..l.aabb_count as usize] {
                    b = [
                        b[0].min(t.origin - t.half_size),
                        b[1].max(t.origin + t.half_size),
                    ];
                }
                b
            };
            // Children before parents: a node's box waits on both of its.
            fn fill(
                map: &ClipMap,
                n: i32,
                leaf: &dyn Fn(&CLeaf) -> [Vec3; 2],
                out: &mut [[Vec3; 2]],
            ) -> [Vec3; 2] {
                if n < 0 {
                    return leaf(&map.leafs[!n as usize]);
                }
                let c = map.nodes[n as usize].children;
                let (a, b) = (fill(map, c[0], leaf, out), fill(map, c[1], leaf, out));
                let u = [a[0].min(b[0]), a[1].max(b[1])];
                out[n as usize] = u;
                u
            }
            let mut nodes = vec![empty; self.nodes.len()];
            if !self.nodes.is_empty() {
                fill(self, 0, &leaf, &mut nodes);
            }
            nodes
        })
    }

    /// `CM_BoxTrace` on model 0: a sweep of the box `mins..maxs` from
    /// `start` to `end` under `mask`, as the capsule the box makes; a zero
    /// box is a point. `start == end` is a position test.
    pub fn trace(&self, start: Vec3, end: Vec3, mins: Vec3, maxs: Vec3, mask: u32) -> ClipTrace {
        self.with_cx(start, end, mins, maxs, mask, |cx, tr| {
            if start == end {
                cx.position_test(tr);
            } else {
                cx.sweep(tr);
            }
        })
    }

    /// `CM_BoxTrace` on a submodel: the same sweep through `leaf` alone
    /// (a `cmodel_t`'s, `CM_TraceThroughLeaf`), or `CM_TestInLeaf` when
    /// `start == end`. The points are in the model's frame.
    pub fn trace_in_leaf(
        &self,
        leaf: &CLeaf,
        start: Vec3,
        end: Vec3,
        mins: Vec3,
        maxs: Vec3,
        mask: u32,
    ) -> ClipTrace {
        self.with_cx(start, end, mins, maxs, mask, |cx, tr| {
            if start == end {
                cx.test_in_leaf(*leaf, tr);
            } else {
                cx.trace_in_leaf(*leaf, tr);
            }
        })
    }

    /// One trace over this map with its stamps.
    fn with_cx(
        &self,
        start: Vec3,
        end: Vec3,
        mins: Vec3,
        maxs: Vec3,
        mask: u32,
        f: impl FnOnce(&mut trace::Cx, &mut ClipTrace),
    ) -> ClipTrace {
        let mut tr = ClipTrace::clear();
        STAMPS.with(|s| {
            let mut stamps = s.borrow_mut();
            stamps.prepare(self);
            let mut cx = trace::Cx::new(self, &mut stamps, start, end, mins, maxs, mask);
            f(&mut cx, &mut tr);
        });
        tr
    }

    /// `CM_PointContents` on model 0: the OR of the contents of every brush
    /// of the point's leaf that holds it. Triangles add nothing.
    pub fn point_contents(&self, p: Vec3) -> u32 {
        trace::point_contents(self, p)
    }

    /// `CM_PointContents` on a submodel: the brushes of its `leaf`.
    pub fn point_contents_in_leaf(&self, leaf: &CLeaf, p: Vec3) -> u32 {
        trace::leaf_contents(self, leaf, p)
    }

    /// The bounds of every brush with a side whose material carries
    /// [`SURF_LADDER`], what a climb's trace reads off the side it meets.
    pub fn ladder_boxes(&self) -> Vec<(Vec3, Vec3)> {
        let ladder = |m: u16| {
            self.materials
                .get(usize::from(m))
                .is_some_and(|&(surf, _)| surf & SURF_LADDER != 0)
        };
        self.brushes
            .iter()
            .filter(|b| {
                b.axial_material.iter().flatten().any(|&m| ladder(m))
                    || b.sides.iter().any(|&(_, m)| ladder(m))
            })
            .map(|b| (b.mins, b.maxs))
            .collect()
    }

    /// The brush contents bits [`trace`](Self::trace) and
    /// [`point_contents`](Self::point_contents) read: the material's word
    /// with 0x4 and 0x20000000 cleared (`CMod_LoadBrushes`).
    pub fn brush_contents(&self, brush: usize) -> u32 {
        self.brushes[brush].contents
    }
}

impl WorldClip for ClipMap {
    fn trace(&self, start: Vec3, end: Vec3, mins: Vec3, maxs: Vec3, mask: u32) -> WorldHit {
        let t = ClipMap::trace(self, start, end, mins, maxs, mask);
        WorldHit {
            fraction: t.fraction,
            normal: t.normal,
            surface_flags: t.surface_flags,
            contents: t.contents,
            startsolid: t.startsolid,
            allsolid: t.allsolid,
        }
    }

    fn point_contents(&self, p: Vec3) -> u32 {
        ClipMap::point_contents(self, p)
    }

    fn ladder_boxes(&self) -> Vec<(Vec3, Vec3)> {
        ClipMap::ladder_boxes(self)
    }
}

/// The per-trace stamps on edges, vertices and partitions that keep a trace
/// from testing one twice (retail's thread-local `checkcount` arrays). A
/// stamp is a trace number, so arrays sized for another map hold nothing
/// current.
#[derive(Default)]
struct Stamps {
    count: u32,
    edges: Vec<u32>,
    verts: Vec<u32>,
    parts: Vec<u32>,
}

impl Stamps {
    fn prepare(&mut self, map: &ClipMap) {
        self.count = self.count.wrapping_add(1);
        if self.count == 0 {
            self.edges.fill(0);
            self.verts.fill(0);
            self.parts.fill(0);
            self.count = 1;
        }
        let grow = |v: &mut Vec<u32>, n: usize| {
            if v.len() < n {
                v.resize(n, 0);
            }
        };
        grow(&mut self.edges, map.edges.len());
        grow(&mut self.verts, map.verts.len());
        grow(&mut self.parts, map.partitions.len());
    }
}

thread_local! {
    static STAMPS: RefCell<Stamps> = RefCell::new(Stamps::default());
}

#[cfg(test)]
mod tests;
