//! CoD4 1.7's world collision: the map zone's `clipMap_t` turned into the
//! CoD2-family [`vcod_common::clipmap::ClipMap`] the shared kernels trace,
//! with its submodels (`cmodel_t`) and the static models' point clip
//! through their xmodels' collision. Layouts and evidence:
//! `docs/research/cod4-collision.md`.
//!
//! The leaf brush trees come prebuilt in the zone, in the shape CoD2
//! builds at load ([`BrushNode`]); the triangles carry no planes and go
//! through the kernels' indexed arm ([`IndexedMesh`]).

#![allow(clippy::neg_cmp_op_on_partial_ord)]

use std::sync::Arc;

use anyhow::{Context, Result, bail, ensure};
use glam::Vec3;
use vcod_common::clipmap::{
    self, BrushNode, CBrush, CLeaf, CNode, CPlane, ClipTrace, CollAabbTree, CollBorder,
    CollPartition, IndexedMesh,
};
use vcod_common::collision::{WorldClip, WorldHit};

use crate::assets::Assets;
use crate::assets::model::XModelCollSurf;
use crate::assets::world;
use crate::zone::Pointer;

pub use vcod_common::clipmap::{
    CONTENTS_BODY, CONTENTS_GLASS, CONTENTS_PLAYERCLIP, CONTENTS_SOLID, MASK_PLAYERSOLID,
    MASK_SHOT, SURF_LADDER,
};

/// `CM_Trace`'s closing rule: a contact no kernel called walkable is
/// walkable when its normal climbs at least this steeply.
pub const WALKABLE_NORMAL_Z: f32 = 0.7;
/// `EQUAL_EPSILON`, the static model triangles' barycentric slack.
const EQUAL_EPS: f32 = 0.001;
/// `SURFACE_CLIP_EPSILON`.
const CLIP_EPS: f32 = 0.125;

/// A `cmodel_t` (0x48): bounds, radius and the leaf its brushes and
/// triangles hang off.
#[derive(Clone, Copy, Debug)]
pub struct Submodel {
    pub mins: Vec3,
    pub maxs: Vec3,
    pub radius: f32,
    pub leaf: CLeaf,
}

/// A `cStaticModel_s` (0x50) with its xmodel's collision.
#[derive(Clone, Debug)]
pub struct StaticModel {
    pub origin: Vec3,
    /// `invScaledAxis` rows: world to model is `d.x r0 + d.y r1 + d.z r2`.
    pub inv_axis: [Vec3; 3],
    pub absmin: Vec3,
    pub absmax: Vec3,
    /// The xmodel's `contents`, what the point trace masks first.
    pub contents: u32,
    /// `collLod` below 0: the model clips nothing.
    pub no_collision: bool,
    pub surfs: Arc<Vec<XModelCollSurf>>,
}

/// The world's clip map: model 0 in [`world`](Self::world), the
/// submodels after it, the static models beside.
#[derive(Clone, Debug)]
pub struct ClipMap {
    pub name: String,
    pub world: clipmap::ClipMap,
    /// `cmodels`; index 0 is the world's own and traces as [`Self::world`].
    pub submodels: Vec<Submodel>,
    pub static_models: Vec<StaticModel>,
}

impl ClipMap {
    /// The clip map of a walked map zone (walked with
    /// [`crate::walk::WalkOptions::keep_bulk`]): its one `col_map_mp` or
    /// `col_map_sp`.
    pub fn from_assets(a: &Assets) -> Result<ClipMap> {
        Self::from_zones(a, &[])
    }

    /// [`Self::from_assets`] with the zones loaded before the map's: a
    /// static model whose xmodel the map zone only names (a leading comma)
    /// clips with the model of that name in `refs`, the first that has it
    /// (`DB_FindXAssetHeader`; mp_crash's 219 `fx_me_brick_01` rubble
    /// pieces are common_mp's).
    pub fn from_zones(a: &Assets, refs: &[&Assets]) -> Result<ClipMap> {
        let c = match (&a.clip_maps_pvs[..], &a.clip_maps[..]) {
            ([c], []) => c,
            ([], [c]) => &c.0,
            (p, s) => bail!("{} + {} clip maps", p.len(), s.len()),
        };
        build(c, a, refs).with_context(|| c.name.clone())
    }

    /// `CM_BoxTrace` on model 0 and `CM_Trace`'s closing walkable rule.
    pub fn trace(&self, start: Vec3, end: Vec3, mins: Vec3, maxs: Vec3, mask: u32) -> ClipTrace {
        close(self.world.trace(start, end, mins, maxs, mask))
    }

    /// `CM_BoxTrace` on clip model `model`: 0 the world, else the
    /// submodel's leaf alone, in the model's frame.
    pub fn trace_model(
        &self,
        model: usize,
        start: Vec3,
        end: Vec3,
        mins: Vec3,
        maxs: Vec3,
        mask: u32,
    ) -> ClipTrace {
        if model == 0 {
            return self.trace(start, end, mins, maxs, mask);
        }
        let leaf = &self.submodels[model].leaf;
        close(self.world.trace_in_leaf(leaf, start, end, mins, maxs, mask))
    }

    /// Every brush under submodel `model`'s leaf as its clip planes
    /// `(normal, dist)`, the six axial ones first: a point is inside when
    /// `normal . p <= dist` on all of them. Empty for the world (0) and a
    /// model with no brushes.
    pub fn submodel_brush_planes(&self, model: usize) -> Vec<Vec<(Vec3, f32)>> {
        let Some(sub) = self.submodels.get(model).filter(|_| model != 0) else {
            return Vec::new();
        };
        let w = &self.world;
        let mut found: Vec<u16> = Vec::new();
        let mut stack = vec![sub.leaf.brush_node as usize];
        while let Some(n) = stack.pop() {
            if n == 0 {
                continue;
            }
            match w.brush_nodes[n] {
                BrushNode::Leaf { first, count, .. } => {
                    found.extend(&w.leaf_brush_list[first as usize..][..count as usize]);
                }
                BrushNode::Split {
                    straddlers,
                    children,
                    ..
                } => {
                    if straddlers {
                        stack.push(n + 1);
                    }
                    stack.extend(children.iter().map(|&c| n + c as usize));
                }
            }
        }
        found.sort_unstable();
        found.dedup();
        found
            .iter()
            .map(|&b| {
                let b = &w.brushes[b as usize];
                let mut planes = Vec::with_capacity(6 + b.sides.len());
                for k in 0..3 {
                    let mut n = Vec3::ZERO;
                    n[k] = 1.0;
                    planes.push((n, b.maxs[k]));
                    planes.push((-n, -b.mins[k]));
                }
                planes.extend(b.sides.iter().map(|&(pi, _)| {
                    let p = w.planes[pi as usize];
                    (p.normal, p.dist)
                }));
                planes
            })
            .collect()
    }

    /// `CM_PointContents` on model 0.
    pub fn point_contents(&self, p: Vec3) -> u32 {
        self.world.point_contents(p)
    }

    /// `CM_PointContents` on clip model `model`.
    pub fn point_contents_model(&self, model: usize, p: Vec3) -> u32 {
        if model == 0 {
            return self.point_contents(p);
        }
        self.world
            .point_contents_in_leaf(&self.submodels[model].leaf, p)
    }

    /// `CM_PointTraceStaticModels`: a point sweep against every static
    /// model whose box it can reach before `tr`'s fraction, through
    /// `XModelTraceLine`. Retail walks them by its world sectors; a flat
    /// walk finds the same nearest contact but for exact ties. Returns the
    /// model hit last, the one the trace now reports.
    pub fn trace_static_models(
        &self,
        start: Vec3,
        end: Vec3,
        mask: u32,
        tr: &mut ClipTrace,
    ) -> Option<usize> {
        let mut hit = None;
        for (i, sm) in self.static_models.iter().enumerate() {
            if sm.no_collision
                || sm.contents & mask == 0
                || box_misses(start, end, sm.absmin, sm.absmax, tr.fraction)
            {
                continue;
            }
            let local = |p: Vec3| {
                // `MatrixTransformVector`: the row vector times the matrix.
                let d = p - sm.origin;
                sm.inv_axis[0] * d.x + sm.inv_axis[1] * d.y + sm.inv_axis[2] * d.z
            };
            if trace_line(&sm.surfs, local(start), local(end), mask, tr) {
                // `MatrixTransposeTransformVector` with each dot kept on
                // the stack and stored, then `Vec3NormalizeTo`'s rounding
                // (cod4-collision.md 4).
                let n = tr.normal.as_dvec3();
                let axis = sm.inv_axis.map(|a| a.as_dvec3());
                let world = Vec3::new(
                    n.dot(axis[0]) as f32,
                    n.dot(axis[1]) as f32,
                    n.dot(axis[2]) as f32,
                );
                tr.normal = vcod_common::x87::normalize_to(world);
                hit = Some(i);
            }
        }
        hit
    }

    /// The bounds of every brush with a [`SURF_LADDER`] side.
    pub fn ladder_boxes(&self) -> Vec<(Vec3, Vec3)> {
        self.world.ladder_boxes()
    }

    /// The leafs the BSP's nodes reach, in index order. The leaf array
    /// ends with one more per submodel, which no node names.
    pub fn world_leafs(&self) -> Vec<usize> {
        let mut seen = vec![false; self.world.leafs.len()];
        let mut stack = vec![0i32];
        while let Some(n) = stack.pop() {
            if n < 0 {
                seen[!n as usize] = true;
            } else {
                stack.extend(self.world.nodes[n as usize].children);
            }
        }
        (0..seen.len()).filter(|&i| seen[i]).collect()
    }

    /// The brushes clip model `model`'s leaf brush tree names, each once:
    /// for the world, those of the leafs its nodes reach.
    pub fn model_brushes(&self, model: usize) -> Vec<u16> {
        let leaf = if model == 0 {
            None
        } else {
            Some(self.submodels[model].leaf)
        };
        let roots: Vec<u32> = match leaf {
            Some(l) => vec![l.brush_node],
            None => self
                .world_leafs()
                .into_iter()
                .map(|l| self.world.leafs[l].brush_node)
                .collect(),
        };
        let mut seen = vec![false; self.world.brushes.len()];
        let mut out = Vec::new();
        for r in roots.into_iter().filter(|&r| r != 0) {
            let mut stack = vec![r as usize];
            while let Some(n) = stack.pop() {
                match self.world.brush_nodes[n] {
                    BrushNode::Leaf { first, count, .. } => {
                        for &b in &self.world.leaf_brush_list[first as usize..][..count as usize] {
                            if !std::mem::replace(&mut seen[b as usize], true) {
                                out.push(b);
                            }
                        }
                    }
                    BrushNode::Split {
                        straddlers,
                        children,
                        ..
                    } => {
                        if straddlers {
                            stack.push(n + 1);
                        }
                        stack.extend(children.map(|c| n + c as usize));
                    }
                }
            }
        }
        out
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

/// `CM_Trace`'s last step: a contact the kernels left unwalkable, not
/// started solid, is walkable on a steep enough normal.
fn close(mut t: ClipTrace) -> ClipTrace {
    if !t.walkable && !t.startsolid {
        t.walkable = t.normal.z >= WALKABLE_NORMAL_Z;
    }
    t
}

/// `CM_TraceBox` for a point: whether the segment misses the box or meets
/// it no nearer than `fraction`.
fn box_misses(start: Vec3, end: Vec3, lo: Vec3, hi: Vec3, fraction: f32) -> bool {
    let (mut enter, mut leave) = (0.0f32, fraction);
    for k in 0..3 {
        let d = end[k] - start[k];
        if d == 0.0 {
            if start[k] < lo[k] || start[k] > hi[k] {
                return true;
            }
            continue;
        }
        let (mut a, mut b) = ((lo[k] - start[k]) / d, (hi[k] - start[k]) / d);
        if a > b {
            std::mem::swap(&mut a, &mut b);
        }
        enter = enter.max(a);
        leave = leave.min(b);
        if leave < enter {
            return true;
        }
    }
    false
}

/// `XModelTraceLine` (cod4_lnxded 0x81c5f14, iw3mp.exe 0x58c5a0): a
/// point sweep against an xmodel's collision surfaces, `start` and `end`
/// in the model's frame at its bind pose. Each surface whose contents
/// meet `mask` and whose box the sweep reaches before `tr`'s fraction is
/// tried triangle by triangle; a crossing from the front face to behind
/// it nearer than the fraction so far, inside the triangle's `s, t`
/// range give or take [`EQUAL_EPS`], becomes the contact, its fraction
/// pulled back [`CLIP_EPS`] (cod4-collision.md 4). Whether any did.
pub fn trace_line(
    surfs: &[XModelCollSurf],
    start: Vec3,
    end: Vec3,
    mask: u32,
    tr: &mut ClipTrace,
) -> bool {
    let mut met = false;
    for s in surfs.iter().filter(|s| mask & s.contents as u32 != 0) {
        let (lo, hi) = (Vec3::from_array(s.mins), Vec3::from_array(s.maxs));
        if box_misses(start, end, lo, hi, tr.fraction) {
            continue;
        }
        for tri in &s.tris {
            let Some(fraction) = tri_crossing(tri, start, end, tr.fraction) else {
                continue;
            };
            *tr = ClipTrace {
                fraction,
                normal: Vec3::new(tri[0], tri[1], tri[2]),
                surface_flags: s.surface_flags as u32,
                contents: s.contents as u32,
                material: None,
                allsolid: false,
                startsolid: false,
                ..*tr
            };
            met = true;
        }
    }
    met
}

/// Where the segment crosses a collision triangle, as a trace fraction
/// under `nearest`. A triangle is twelve floats: its plane (normal and
/// distance), then two planes whose distances are the barycentric `s`
/// and `t` of a point on it.
fn tri_crossing(tri: &[f32; 12], start: Vec3, end: Vec3, nearest: f32) -> Option<f32> {
    let side = |p: Vec3, k: usize| p.dot(Vec3::new(tri[k], tri[k + 1], tri[k + 2])) - tri[k + 3];
    let behind = side(end, 0);
    if !(behind < 0.0) {
        return None;
    }
    let front = side(start, 0);
    if !(front > 0.0) {
        return None;
    }
    let span = front - behind;
    let fraction = ((front - CLIP_EPS) / span).max(0.0);
    if !(nearest > fraction) {
        return None;
    }
    let on = start + (end - start) * (front / span);
    let within = |x: f32| (-EQUAL_EPS..=1.0 + EQUAL_EPS).contains(&x);
    let s = side(on, 4);
    let t = side(on, 8);
    (within(s) && t >= -EQUAL_EPS && s + t <= 1.0 + EQUAL_EPS).then_some(fraction)
}

fn f32_at(b: &[u8], o: usize) -> f32 {
    f32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}

fn u32_at(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}

fn i32_at(b: &[u8], o: usize) -> i32 {
    u32_at(b, o) as i32
}

fn u16_at(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes(b[o..o + 2].try_into().unwrap())
}

fn i16_at(b: &[u8], o: usize) -> i16 {
    u16_at(b, o) as i16
}

fn vec3_at(b: &[u8], o: usize) -> Vec3 {
    Vec3::new(f32_at(b, o), f32_at(b, o + 4), f32_at(b, o + 8))
}

fn bulk<'a>(b: &'a Option<Vec<u8>>, what: &str) -> Result<&'a [u8]> {
    b.as_deref()
        .with_context(|| format!("{what} not kept (walk with keep_bulk)"))
}

/// The entry a stored pointer names in an array of `size`-byte entries
/// that landed at packed address `base`.
fn index_of(token: u32, base: u32, size: u32, count: usize, what: &str) -> Result<usize> {
    let Pointer::Offset { block, offset } = Pointer::from_token(token) else {
        bail!("{what}: pointer token {token:#x} is not an offset");
    };
    let o = (u32::from(block) << 28 | offset).wrapping_sub(base);
    ensure!(o % size == 0, "{what}: pointer inside an entry");
    let i = (o / size) as usize;
    ensure!(i < count, "{what}: {i} past {count}");
    Ok(i)
}

/// A `cLeaf_t` (0x2C).
fn leaf_at(l: &[u8]) -> CLeaf {
    CLeaf {
        first_aabb: u16_at(l, 0),
        aabb_count: u16_at(l, 2),
        brush_contents: u32_at(l, 4),
        terrain_contents: u32_at(l, 8),
        mins: vec3_at(l, 12),
        maxs: vec3_at(l, 24),
        brush_node: u32_at(l, 36),
    }
}

fn build(c: &world::ClipMap, a: &Assets, refs: &[&Assets]) -> Result<ClipMap> {
    let n = |v: u32| v as usize;
    let planes: Vec<CPlane> = bulk(&c.planes, "planes")?
        .as_chunks::<0x14>()
        .0
        .iter()
        .map(|p| CPlane {
            normal: vec3_at(p, 0),
            dist: f32_at(p, 12),
            kind: p[16],
            signbits: p[17],
        })
        .collect();
    ensure!(planes.len() == n(c.plane_count), "plane count");
    let plane = |token: u32, what: &str| {
        index_of(token, c.at.planes, 0x14, planes.len(), what).map(|i| i as u32)
    };
    let materials: Vec<(u32, u32)> = c
        .materials
        .as_chunks::<0x48>()
        .0
        .iter()
        .map(|m| (u32_at(m, 64), u32_at(m, 68)))
        .collect();
    let nodes = bulk(&c.nodes, "nodes")?
        .as_chunks::<8>()
        .0
        .iter()
        .map(|nd| {
            Ok(CNode {
                plane: plane(u32_at(nd, 0), "node plane")?,
                children: [i32::from(i16_at(nd, 4)), i32::from(i16_at(nd, 6))],
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let leafs: Vec<CLeaf> = bulk(&c.leafs, "leafs")?
        .as_chunks::<0x2C>()
        .0
        .iter()
        .map(|l| leaf_at(l))
        .collect();

    // The prebuilt `cLeafBrushNode_s` (0x14) pool: a positive count is a
    // leaf with its list, a negative one tests the next node first.
    let mut leaf_brush_list = Vec::new();
    let mut brush_nodes = Vec::new();
    for (i, e) in bulk(&c.leaf_brush_nodes, "leaf brush nodes")?
        .as_chunks::<0x14>()
        .0
        .iter()
        .enumerate()
    {
        let count = i16_at(e, 2);
        let contents = u32_at(e, 4);
        brush_nodes.push(if count > 0 {
            let list = c.leaf_brush_lists[i]
                .as_ref()
                .with_context(|| format!("leaf brush node {i}: list not copied"))?;
            let first = leaf_brush_list.len() as u32;
            leaf_brush_list.extend_from_slice(list);
            BrushNode::Leaf {
                first,
                count: count as u16,
                contents,
            }
        } else {
            BrushNode::Split {
                axis: e[0],
                dist: f32_at(e, 8),
                clearance: f32_at(e, 12),
                straddlers: count < 0,
                children: [u16_at(e, 16), u16_at(e, 18)],
                contents,
            }
        });
    }

    let sides = bulk(&c.brush_sides, "brush sides")?;
    let side_count = n(c.brush_side_count);
    let mut brushes = Vec::with_capacity(usize::from(c.brush_count) + 1);
    for (i, b) in bulk(&c.brushes, "brushes")?
        .as_chunks::<0x50>()
        .0
        .iter()
        .enumerate()
    {
        let numsides = n(u32_at(b, 28));
        let mut list = Vec::with_capacity(numsides);
        if numsides > 0 {
            let first = index_of(
                u32_at(b, 32),
                c.at.brush_sides,
                0xC,
                side_count,
                "brush sides",
            )
            .with_context(|| format!("brush {i}"))?;
            ensure!(
                first + numsides <= side_count,
                "brush {i}: sides past the array"
            );
            for s in sides[0xC * first..]
                .as_chunks::<0xC>()
                .0
                .iter()
                .take(numsides)
            {
                list.push((
                    plane(u32_at(s, 0), "brush side plane")?,
                    u32_at(s, 4) as u16,
                ));
            }
        }
        let mut axial_material = [[0u16; 3]; 2];
        for (k, row) in axial_material.iter_mut().enumerate() {
            for (j, m) in row.iter_mut().enumerate() {
                *m = u16_at(b, 36 + 2 * (3 * k + j));
            }
        }
        brushes.push(CBrush {
            mins: vec3_at(b, 0),
            maxs: vec3_at(b, 16),
            contents: u32_at(b, 12),
            sides: list,
            axial_material,
        });
    }
    // The box model's leaf names brush `numBrushes`, the temp box brush
    // `CM_TempBoxModel` fills at run time: a placeholder that clips nothing.
    brushes.push(CBrush {
        mins: Vec3::ZERO,
        maxs: Vec3::ZERO,
        contents: 0,
        sides: Vec::new(),
        axial_material: [[0; 3]; 2],
    });

    let verts: Vec<Vec3> = bulk(&c.verts, "verts")?
        .as_chunks::<12>()
        .0
        .iter()
        .map(|v| vec3_at(v, 0))
        .collect();
    let tris: Vec<[u16; 3]> = bulk(&c.tri_indices, "tri indices")?
        .as_chunks::<6>()
        .0
        .iter()
        .map(|t| [u16_at(t, 0), u16_at(t, 2), u16_at(t, 4)])
        .collect();
    let borders: Vec<CollBorder> = bulk(&c.borders, "borders")?
        .as_chunks::<0x1C>()
        .0
        .iter()
        .map(|b| CollBorder {
            dist_eq: [f32_at(b, 0), f32_at(b, 4), f32_at(b, 8)],
            z_base: f32_at(b, 12),
            z_slope: f32_at(b, 16),
            start: f32_at(b, 20),
            length: f32_at(b, 24),
        })
        .collect();
    let partitions = bulk(&c.partitions, "partitions")?
        .as_chunks::<0xC>()
        .0
        .iter()
        .map(|p| {
            let border_count = p[1];
            let first_border = if border_count > 0 {
                index_of(
                    u32_at(p, 8),
                    c.at.borders,
                    0x1C,
                    borders.len(),
                    "partition borders",
                )? as i32
            } else {
                0
            };
            Ok(CollPartition {
                unused: 0,
                tri_count: p[0],
                border_count,
                first_tri: i32_at(p, 4),
                first_border,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let aabbs: Vec<CollAabbTree> = bulk(&c.aabb_trees, "aabb trees")?
        .as_chunks::<0x20>()
        .0
        .iter()
        .map(|t| CollAabbTree {
            origin: vec3_at(t, 0),
            half_size: vec3_at(t, 12),
            material: u16_at(t, 24),
            child_count: u16_at(t, 26),
            child_or_partition: i32_at(t, 28),
        })
        .collect();
    let submodels = bulk(&c.cmodels, "submodels")?
        .as_chunks::<0x48>()
        .0
        .iter()
        .map(|m| Submodel {
            mins: vec3_at(m, 0),
            maxs: vec3_at(m, 12),
            radius: f32_at(m, 24),
            leaf: leaf_at(&m[28..]),
        })
        .collect();
    let mut surfs_by_model: Vec<Option<Arc<Vec<XModelCollSurf>>>> = vec![None; a.xmodels.len()];
    let static_models = c
        .static_models
        .iter()
        .enumerate()
        .map(|(i, (m, b))| {
            let id = m.with_context(|| format!("static model {i} has no xmodel"))?;
            let mut x = &a.xmodels[id.index()];
            if let Some(name) = x.name.strip_prefix(',')
                && let Some(r) = refs
                    .iter()
                    .find_map(|r| r.xmodels.iter().find(|m| m.name == name))
            {
                x = r;
            }
            let surfs = surfs_by_model[id.index()]
                .get_or_insert_with(|| Arc::new(x.coll.clone()))
                .clone();
            Ok(StaticModel {
                origin: vec3_at(b, 8),
                inv_axis: [vec3_at(b, 20), vec3_at(b, 32), vec3_at(b, 44)],
                absmin: vec3_at(b, 56),
                absmax: vec3_at(b, 68),
                contents: x.contents as u32,
                no_collision: x.coll_lod < 0,
                surfs,
            })
        })
        .collect::<Result<Vec<_>>>()?;

    Ok(ClipMap {
        name: c.name.clone(),
        world: clipmap::ClipMap {
            planes,
            nodes,
            leafs,
            brush_nodes,
            leaf_brush_list,
            brushes,
            materials,
            verts: Vec::new(),
            edges: Vec::new(),
            tris: Vec::new(),
            borders,
            partitions,
            aabbs,
            indexed: Some(IndexedMesh {
                verts,
                tris,
                walkable: bulk(&c.tri_edge_walkable, "walkable bits")?.to_vec(),
            }),
            reach: Default::default(),
        },
        submodels,
        static_models,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fnv(words: impl IntoIterator<Item = u32>) -> u64 {
        words.into_iter().fold(0xcbf2_9ce4_8422_2325, |h, w| {
            (h ^ u64::from(w)).wrapping_mul(0x100_0000_01b3)
        })
    }

    /// A triangle (0,0,0) (64,0,0) (0,64,0) facing +z: its plane, and the
    /// planes whose distances are its barycentric `s` and `t`.
    fn surf(z: f32, contents: i32) -> XModelCollSurf {
        let tri = [
            0.0,
            0.0,
            1.0,
            z,
            1.0 / 64.0,
            0.0,
            0.0,
            0.0,
            0.0,
            1.0 / 64.0,
            0.0,
            0.0,
        ];
        let back = [
            0.0,
            0.0,
            -1.0,
            -z - 4.0,
            0.0,
            1.0 / 64.0,
            0.0,
            0.0,
            1.0 / 64.0,
            0.0,
            0.0,
            0.0,
        ];
        XModelCollSurf {
            tris: vec![tri, back],
            mins: [-1.0, -1.0, z - 5.0],
            maxs: [65.0, 65.0, z + 1.0],
            bone: 0,
            contents,
            surface_flags: 0x40 | contents,
        }
    }

    /// Segments through two stacked triangles and their back faces, bit
    /// for bit: whether each hit, and the trace it left.
    #[test]
    fn pins_the_model_line_trace() {
        let surfs = [surf(0.0, 1), surf(10.0, 2), surf(20.0, 1)];
        let mut out = Vec::new();
        for (mask, frac) in [(1, 1.0), (3, 1.0), (3, 0.4), (2, 1.0)] {
            for x in (-8..=72).step_by(5) {
                for y in (-8..=72).step_by(7) {
                    for (z0, z1) in [(30.0, -10.0), (15.0, 5.0), (-10.0, 30.0), (12.0, 12.0)] {
                        let start = Vec3::new(x as f32 * 0.9, y as f32, z0);
                        let end = Vec3::new(x as f32, y as f32 * 1.1 - 3.0, z1);
                        let mut tr = ClipTrace::clear();
                        tr.fraction = frac;
                        let hit = trace_line(&surfs, start, end, mask, &mut tr);
                        out.extend([
                            u32::from(hit),
                            tr.fraction.to_bits(),
                            tr.contents,
                            tr.surface_flags,
                        ]);
                        out.extend(tr.normal.to_array().map(f32::to_bits));
                    }
                }
            }
        }
        let hits = out.chunks(7).filter(|c| c[0] == 1).count();
        assert!(hits > 100 && hits < out.len() / 7, "{hits} hits");
        assert_eq!(fnv(out), PIN_TRACE);
    }
    const PIN_TRACE: u64 = 12001850731110781703;
}
