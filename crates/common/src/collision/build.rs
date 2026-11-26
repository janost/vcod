//! Contains routines ported from the Quake III Arena GPL source, Copyright (C) 1999-2005 Id Software, Inc..
//! See NOTICE.
//!
//! `CollisionWorld::build`: the primitives, the volumes and the BVH.

use super::entities::ModelSpan;
use super::masks::{
    CLIP_BRUSH, CONTENTS_NODROP, CONTENTS_PLAYERCLIP, CONTENTS_SOLID, CONTENTS_TERRAIN,
    CONTENTS_WATER, NO_ENTITY_CONTENTS,
};
use super::walk::TraceInput;
use super::{BrushPlanes, CollisionWorld, ModelTri, Prim, StaticModels, WorldClip, WorldPatch};
use crate::leafwalk::{Item, LeafWalk};
use crate::mapworld::{MapWorld, Partition};
use crate::net::protocol::ENTITYNUM_WORLD;
use crate::patch::{Arith, PatchCollide};
use crate::terrain::Terrain;
use glam::Vec3;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, AtomicU64};
use std::sync::{Arc, RwLock};

/// Build-time soup filter. Census over all 49 stock maps (`flag_census`
/// example): the walk-through cutouts (bushwalls, treelines, ground decals,
/// autosprite wire, sfx water) are TRANSLUCENT/WINDOW/DETAIL words with no
/// clip bit; everything collidable carries SOLID or PLAYERCLIP except bare
/// terrain 0x4. Kept tris store only their mask-relevant bits.
fn tri_contents(content_flags: u32) -> Option<u32> {
    if content_flags & (CONTENTS_SOLID | CONTENTS_PLAYERCLIP) != 0 {
        Some(content_flags & (CONTENTS_SOLID | CONTENTS_PLAYERCLIP))
    } else if content_flags == CONTENTS_TERRAIN {
        Some(CONTENTS_SOLID)
    } else {
        None
    }
}

/// Per lump-24 terrain partition and patch of `bsp`, the model whose
/// lump-23 run lists it (lump 27 bytes 32..40); 0 where none does.
pub(super) fn partition_owners(bsp: &MapWorld) -> (Vec<u32>, Vec<u32>) {
    let mut terrain = vec![0; bsp.terrain.len()];
    let mut patches = vec![0; bsp.patches.len()];
    for (mi, model) in bsp.models.iter().enumerate().skip(1) {
        let (first, count) = model.partitions;
        let Some(run) = bsp
            .leaf_partitions
            .get(first as usize..first as usize + count as usize)
        else {
            continue;
        };
        for part in run {
            match *part {
                Partition::Terrain(i) => terrain[i as usize] = mi as u32,
                Partition::Patch(i) => patches[i as usize] = mi as u32,
            }
        }
    }
    (terrain, patches)
}

#[derive(Clone)]
pub(super) struct BvhNode {
    pub(super) lo: Vec3,
    pub(super) hi: Vec3,
    /// leaf: absolute prim index; internal: left child
    pub(super) first: u32,
    /// internal: right child; leaf: unused
    pub(super) second: u32,
    /// 0 = internal, otherwise leaf covering `count` prims starting at `first`
    pub(super) count: u32,
    /// Whether the subtree holds a prim outside model 0, all the walk after
    /// [`CollisionWorld::clip_leaf_walk`] clips.
    pub(super) rest: bool,
    /// Whether the subtree holds a brush or a patch, which
    /// [`TraceInput::misses`] never culls.
    pub(super) loose: bool,
}

/// A brush as clip planes plus its axial bounds for the cheap reject: a
/// water volume, or a pane at build time.
#[derive(Clone)]
pub(super) struct Volume {
    pub(super) planes: Vec<(Vec3, f32)>,
    pub(super) lo: Vec3,
    pub(super) hi: Vec3,
    /// Per side, its material's surface flags: kept for water, which a
    /// missile's splash reads (`water_hit`), empty otherwise.
    pub(super) side_flags: Vec<u32>,
}

const AXES: [Vec3; 3] = [Vec3::X, Vec3::Y, Vec3::Z];

/// A lump-4 brush's sides as clip planes, appended to `out` and offset to
/// `origin`, plus the brush's axial bounds. Sides 0..6 are the bit-cast axial
/// bounds and sides 6.. index `MapWorld::planes`
/// (docs/research/bsp-ibsp59-format.md, "Lump 4, brushes"). Point p is inside the
/// brush iff n·p <= d for every plane returned.
pub fn brush_side_planes(
    bsp: &MapWorld,
    brush: &crate::mapworld::Brush,
    origin: Vec3,
    out: &mut Vec<(Vec3, f32)>,
) -> (Vec3, Vec3) {
    let sides = &bsp.brush_sides[brush.first_side as usize..][..brush.num_sides as usize];
    out.reserve(sides.len());
    let mut lo = Vec3::ZERO;
    let mut hi = Vec3::ZERO;
    for axis in 0..3 {
        let axis_lo = f32::from_bits(sides[axis * 2].plane_or_dist) + origin[axis];
        let axis_hi = f32::from_bits(sides[axis * 2 + 1].plane_or_dist) + origin[axis];
        // Retail zeroes the side's normal and stores -1 on the axis
        // (`cod_lnxded` 0x8054fd1), so the other components are +0.0.
        let mut down = Vec3::ZERO;
        down[axis] = -1.0;
        out.push((down, -axis_lo));
        out.push((AXES[axis], axis_hi));
        lo[axis] = axis_lo;
        hi[axis] = axis_hi;
    }
    for s in &sides[6..] {
        let p = &bsp.planes[s.plane_or_dist as usize];
        let n = Vec3::from_array(p.normal);
        out.push((n, p.dist + n.dot(origin)));
    }
    (lo, hi)
}

/// The triangle lists under construction in `build`.
struct Tris {
    tris: Vec<[Vec3; 3]>,
    surf: Vec<u32>,
    contents: Vec<u32>,
    prims: Vec<(Prim, Vec3, Vec3)>,
    terrain: Terrain,
}

impl Tris {
    /// One lump-24 terrain partition: its records for the clip, and each
    /// triangle in lump order wound with `cross(b - a, c - a)` on the
    /// outside, the side the sphere clip faces. The BVH holds the
    /// partition, padded by 0.25 so the query finds one the box merely
    /// touches.
    fn push_partition(
        &mut self,
        points: &[[f32; 3]],
        tris: &[[u16; 3]],
        surface_flags: u32,
        contents: u32,
    ) {
        if tris.is_empty() {
            return;
        }
        let part = self.terrain.add_partition(points, tris);
        let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
        for tri in tris {
            let p = |i: usize| Vec3::from_array(points[tri[i] as usize]);
            let (a, b, c) = (p(0), p(2), p(1));
            lo = lo.min(a).min(b).min(c);
            hi = hi.max(a).max(b).max(c);
            self.tris.push([a, b, c]);
            self.surf.push(surface_flags);
            self.contents.push(contents);
        }
        self.prims.push((
            Prim::Terrain(part),
            lo - Vec3::splat(0.25),
            hi + Vec3::splat(0.25),
        ));
    }
}

impl CollisionWorld {
    /// [`Self::with_statics`] over `model_tris` clipped a triangle at a
    /// time ([`StaticModels::loose`]).
    pub fn build(bsp: &MapWorld, model_tris: &[ModelTri]) -> Self {
        Self::with_statics(bsp, &StaticModels::loose(model_tris))
    }

    /// `statics` are the placed static models (`props::static_models`),
    /// clipped the way retail clips them: by a locational trace's segment,
    /// a surface at a time, never by a movement trace.
    ///
    /// Every model's brushes enter (submodels translated by the entity origin
    /// from the entities lump), except those of `trigger*` entities: their
    /// brushes carry plain CONTENTS_SOLID in the lump, but retail leaves them
    /// hollow to movement. The terrain partitions of lump 24 enter as the
    /// engine's own triangles, swept as a sphere, and its patches as
    /// `CM_GeneratePatchCollide`'s facets. No render soup collides: retail
    /// clips model 0 through its brushes and lump 24 alone.
    pub fn with_statics(bsp: &MapWorld, statics: &StaticModels) -> Self {
        let mut brushes = Vec::new();
        let mut water = Vec::new();
        let mut nodrop = Vec::new();
        let mut t = Tris {
            tris: Vec::new(),
            surf: Vec::new(),
            contents: Vec::new(),
            prims: Vec::new(),
            terrain: Terrain::default(),
        };

        #[derive(Clone, Copy)]
        struct Placement {
            origin: Vec3,
            trigger: bool,
        }
        let mut placements = vec![
            Placement {
                origin: Vec3::ZERO,
                trigger: false,
            };
            bsp.models.len()
        ];
        for block in crate::bsp::entity_blocks(&bsp.entities) {
            let Some(idx) = block
                .get("model")
                .and_then(|m| m.strip_prefix('*'))
                .and_then(|n| n.parse::<usize>().ok())
            else {
                continue;
            };
            if idx == 0 || idx >= placements.len() {
                continue;
            }
            let p = &mut placements[idx];
            if let Some([x, y, z]) = block.get("origin").and_then(|o| crate::bsp::parse_vec3(o)) {
                p.origin = Vec3::new(x, y, z);
            }
            if block
                .get("classname")
                .is_some_and(|c| c.starts_with("trigger"))
            {
                p.trigger = true;
            }
        }

        let mut model_span = vec![ModelSpan::default(); bsp.models.len()];
        for (mi, model) in bsp.models.iter().enumerate() {
            let placement = &placements[mi];
            let span = &mut model_span[mi];
            span.origin = placement.origin;
            span.brushes = (brushes.len() as u32, brushes.len() as u32);
            let (mut span_lo, mut span_hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
            let brush_range =
                model.first_brush as usize..(model.first_brush + model.num_brushes) as usize;
            for (bi, b) in bsp
                .brushes
                .iter()
                .enumerate()
                .take(brush_range.end)
                .skip(brush_range.start)
            {
                let mat = &bsp.materials[b.material as usize];
                if placement.trigger
                    || mat.content_flags & (CLIP_BRUSH | CONTENTS_WATER | CONTENTS_NODROP) == 0
                {
                    continue;
                }
                let mut planes = Vec::new();
                let (lo, hi) = brush_side_planes(bsp, b, placement.origin, &mut planes);
                if mi == 0 && mat.content_flags & CONTENTS_NODROP != 0 {
                    nodrop.push(Volume {
                        planes: planes.clone(),
                        lo,
                        hi,
                        side_flags: Vec::new(),
                    });
                }
                if mat.content_flags & CONTENTS_WATER != 0 {
                    let sides = &bsp.brush_sides[b.first_side as usize..][..b.num_sides as usize];
                    water.push(Volume {
                        planes: planes.clone(),
                        lo,
                        hi,
                        side_flags: sides
                            .iter()
                            .map(|s| {
                                bsp.materials
                                    .get(s.material as usize)
                                    .map_or(mat.surface_flags, |m| m.surface_flags)
                            })
                            .collect(),
                    });
                }
                if mat.content_flags & CLIP_BRUSH != 0 {
                    let idx = brushes.len() as u32;
                    let sides = &bsp.brush_sides[b.first_side as usize..][..b.num_sides as usize];
                    brushes.push(BrushPlanes {
                        planes,
                        surface_flags: mat.surface_flags,
                        side_surface_flags: sides
                            .iter()
                            .map(|s| {
                                bsp.materials
                                    .get(s.material as usize)
                                    .map_or(mat.surface_flags, |m| m.surface_flags)
                            })
                            .collect(),
                        content_flags: mat.content_flags,
                        bsp_index: bi as u32,
                        material: mat.name.clone(),
                        model: mi as u32,
                    });
                    t.prims.push((Prim::Brush(idx), lo, hi));
                    span_lo = span_lo.min(lo);
                    span_hi = span_hi.max(hi);
                }
            }
            span.brushes.1 = brushes.len() as u32;
            if span.brushes.1 > span.brushes.0 {
                span.lo = span_lo - placement.origin;
                span.hi = span_hi - placement.origin;
            }
        }

        // The engine's terrain, wound the way `CM_GenerateTerrainCollide`
        // takes its plane (Q3's `PlaneFromPoints`: `cross(c - a, b - a)`).
        // A model-0 partition no leaf lists never clips
        // (`CM_TraceThroughTree`). A submodel's partitions are its entity's,
        // placed and linked with its brushes.
        let listed = LeafWalk::listed(bsp);
        let (terrain_owner, patch_owner) = partition_owners(bsp);
        let offset = |model: u32, points: &[[f32; 3]]| -> Vec<[f32; 3]> {
            let o = placements[model as usize].origin;
            points
                .iter()
                .map(|p| (Vec3::from_array(*p) + o).to_array())
                .collect()
        };
        let mut world_terrain = vec![None; bsp.terrain.len()];
        let mut terrain_model = Vec::new();
        for (i, part) in bsp.terrain.iter().enumerate() {
            let model = terrain_owner[i];
            if (model == 0 && listed.as_ref().is_some_and(|l| !l.terrain[i]))
                || placements[model as usize].trigger
            {
                continue;
            }
            let mat = &bsp.materials[part.material as usize];
            let Some(contents) = tri_contents(mat.content_flags) else {
                continue;
            };
            let idx =
                &bsp.collision_indices[part.first_index as usize..][..part.index_count as usize];
            let points = offset(
                model,
                &bsp.collision_verts[part.first_vert as usize..][..part.vert_count as usize],
            );
            let next = t.terrain.parts.len() as u32;
            t.push_partition(&points, idx.as_chunks::<3>().0, mat.surface_flags, contents);
            if t.terrain.parts.len() as u32 > next {
                terrain_model.push(model);
                if model == 0 {
                    world_terrain[i] = Some(next);
                }
            }
        }
        // `CM_LoadMap` builds every patch whatever its contents; the trace's
        // mask test skips the ones it does not meet.
        let mut patches = Vec::new();
        let mut world_patch = vec![None; bsp.patches.len()];
        for (index, part) in bsp.patches.iter().enumerate() {
            let model = patch_owner[index];
            if (model == 0 && listed.as_ref().is_some_and(|l| !l.patches[index]))
                || placements[model as usize].trigger
            {
                continue;
            }
            let mat = &bsp.materials[part.material as usize];
            let (w, h) = (part.width as usize, part.height as usize);
            let points = offset(
                model,
                &bsp.collision_verts[part.first_vert as usize..][..w * h],
            );
            let Some(collide) = PatchCollide::generate(w, h, part.tolerance, &points) else {
                log::warn!("lump 24 patch {index} ({w}x{h}) builds no collision");
                continue;
            };
            let stored = PatchCollide::generate_as(w, h, part.tolerance, &points, Arith::Stored)
                .filter(|s| !s.same_clip(&collide));
            let msvc = PatchCollide::generate_as(w, h, part.tolerance, &points, Arith::Msvc)
                .filter(|s| !s.same_clip(&collide));
            let (mut lo, mut hi) = (collide.mins, collide.maxs);
            for s in stored.iter().chain(&msvc) {
                for k in 0..3 {
                    lo[k] = lo[k].min(s.mins[k]);
                    hi[k] = hi[k].max(s.maxs[k]);
                }
            }
            t.prims.push((
                Prim::Patch(patches.len() as u32),
                Vec3::from_array(lo),
                Vec3::from_array(hi),
            ));
            if model == 0 {
                world_patch[index] = Some(patches.len() as u32);
            }
            patches.push(WorldPatch {
                collide,
                stored,
                msvc,
                content_flags: mat.content_flags,
                surface_flags: mat.surface_flags,
                index: index as u32,
                model,
            });
        }
        let (static_prims, placed) = super::statics::static_prims(statics);
        let split = t.prims.len();
        t.prims.extend(static_prims);

        // The static models sit in a subtree of their own under the root,
        // which the world's walk never enters: the static pass clips them
        // after it, and mixed in they widen every box a player's sweep
        // enters. A world of static models alone is that subtree.
        let mut nodes = Vec::new();
        let models_root = if split == t.prims.len() || split == 0 {
            if !t.prims.is_empty() {
                build_bvh(&mut t.prims, 0, &mut nodes);
            }
            if split == 0 && !t.prims.is_empty() {
                0
            } else {
                u32::MAX
            }
        } else {
            nodes.push(BvhNode {
                lo: Vec3::ZERO,
                hi: Vec3::ZERO,
                first: 0,
                second: 0,
                count: 0,
                rest: true,
                loose: true,
            });
            let (world_prims, model_prims) = t.prims.split_at_mut(split);
            let wi = build_bvh(world_prims, 0, &mut nodes);
            let mi = build_bvh(model_prims, split as u32, &mut nodes);
            let (w, m) = (&nodes[wi as usize], &nodes[mi as usize]);
            nodes[0] = BvhNode {
                lo: w.lo.min(m.lo),
                hi: w.hi.max(m.hi),
                first: wi,
                second: mi,
                count: 0,
                rest: true,
                loose: true,
            };
            mi
        };
        // Children sit after their parent, so one pass from the back sees
        // both before it.
        for i in (0..nodes.len()).rev() {
            let n = &nodes[i];
            nodes[i].rest = if n.count > 0 {
                t.prims[n.first as usize..][..n.count as usize]
                    .iter()
                    .any(|(prim, _, _)| match *prim {
                        Prim::Brush(b) => brushes[b as usize].model != 0,
                        Prim::Terrain(p) => terrain_model[p as usize] != 0,
                        Prim::Patch(p) => patches[p as usize].model != 0,
                        _ => true,
                    })
            } else {
                nodes[n.first as usize].rest || nodes[n.second as usize].rest
            };
            let n = &nodes[i];
            nodes[i].loose = if n.count > 0 {
                t.prims[n.first as usize..][..n.count as usize]
                    .iter()
                    .any(|(prim, _, _)| !TraceInput::tight(*prim))
            } else {
                nodes[n.first as usize].loose || nodes[n.second as usize].loose
            };
        }

        let mut world_brush = vec![None; bsp.brushes.len()];
        for (i, b) in brushes.iter().enumerate() {
            if b.model == 0 {
                world_brush[b.bsp_index as usize] = Some(i as u32);
            }
        }
        // Each listing carries its BVH prim's box.
        let (mut brush_box, mut terrain_box, mut patch_box) = (
            vec![Default::default(); brushes.len()],
            vec![Default::default(); terrain_model.len()],
            vec![Default::default(); patches.len()],
        );
        for &(prim, lo, hi) in &t.prims {
            let b = (lo.to_array(), hi.to_array());
            match prim {
                Prim::Brush(i) => brush_box[i as usize] = b,
                Prim::Terrain(i) => terrain_box[i as usize] = b,
                Prim::Patch(i) => patch_box[i as usize] = b,
                _ => {}
            }
        }
        let leaf_walk =
            LeafWalk::new(
                bsp,
                &world_brush,
                &world_terrain,
                &world_patch,
                |item| match item {
                    Item::Brush(i) => brush_box[i as usize],
                    Item::Terrain(i) => terrain_box[i as usize],
                    Item::Patch(i) => patch_box[i as usize],
                },
            );
        let tri_listed = (0..t.terrain.parts.len())
            .flat_map(|p| {
                let part = t.terrain.parts[p];
                let (terrain, leaf_walk) = (&t.terrain, &leaf_walk);
                let model = terrain_model[p];
                (part.first..part.first + part.count).map(move |i| {
                    let (lo, hi) = terrain.tri_bounds(i as usize);
                    let grow = |v: [f32; 3], by: f32| v.map(|x| x + by);
                    model == 0
                        && leaf_walk.as_ref().is_some_and(|w| {
                            w.box_lists(grow(lo, -1.0), grow(hi, 1.0), Item::Terrain(p as u32))
                        })
                })
            })
            .collect();
        CollisionWorld {
            brushes,
            tris: t.tris,
            model_tris: statics.tris.clone(),
            static_surfs: statics.surfs.clone(),
            statics: placed,
            tris_surf: t.surf,
            tris_contents: t.contents,
            terrain: t.terrain,
            terrain_model,
            patches,
            leaf_walk,
            tri_listed,
            nodes,
            models_root,
            prims: t.prims,
            water,
            nodrop,
            model_linked: bsp.models.iter().map(|_| AtomicBool::new(true)).collect(),
            model_entity: bsp
                .models
                .iter()
                .map(|_| AtomicU32::new(ENTITYNUM_WORLD))
                .collect(),
            model_contents: bsp
                .models
                .iter()
                .map(|_| AtomicU64::new(NO_ENTITY_CONTENTS))
                .collect(),
            model_posed: bsp.models.iter().map(|_| AtomicBool::new(false)).collect(),
            model_span,
            poses: RwLock::new(Vec::new()),
            any_posed: AtomicBool::new(false),
            copies: RwLock::new(Vec::new()),
            any_copy: AtomicBool::new(false),
            instances: RwLock::new(Vec::new()),
            any_instance: AtomicBool::new(false),
            arith: AtomicU8::new(Arith::Kept as u8),
            world_clip: None,
            source: 0,
        }
    }

    /// Hands the world's clip to `clip`: every trace, position test and
    /// contents query meets it first. The [`MapWorld`] this was built from
    /// must leave model 0's brushes and partitions out.
    pub fn set_world_clip(&mut self, clip: Arc<dyn WorldClip>) {
        self.world_clip = Some(clip);
    }

    /// Names the map file the world was built from by a checksum of its
    /// bytes, which the server's bots key their graph on disk by.
    pub fn set_source(&mut self, checksum: u64) {
        self.source = checksum;
    }

    /// [`Self::set_source`]'s checksum, 0 when none was set.
    pub fn source(&self) -> u64 {
        self.source
    }

    /// The world clip's ladder boxes ([`WorldClip::ladder_boxes`]); none
    /// without a world clip.
    pub fn world_clip_ladders(&self) -> Vec<(Vec3, Vec3)> {
        self.world_clip
            .as_ref()
            .map_or_else(Vec::new, |c| c.ladder_boxes())
    }
}

/// `base` is the absolute index of prims[0] within CollisionWorld::prims.
/// Splits by the surface area heuristic over binned centroids: a few large
/// brushes or terrain sheets otherwise widen a median split's boxes until a
/// small sweep enters most of the tree.
fn build_bvh(prims: &mut [(Prim, Vec3, Vec3)], base: u32, nodes: &mut Vec<BvhNode>) -> u32 {
    let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
    let (mut clo, mut chi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
    for (_, plo, phi) in prims.iter() {
        lo = lo.min(*plo);
        hi = hi.max(*phi);
        let c = (*plo + *phi) * 0.5;
        clo = clo.min(c);
        chi = chi.max(c);
    }
    let idx = nodes.len() as u32;
    nodes.push(BvhNode {
        lo,
        hi,
        first: base,
        second: 0,
        count: prims.len() as u32,
        rest: true,
        loose: true,
    });
    if prims.len() <= 4 {
        return idx;
    }
    const BINS: usize = 16;
    let area = |lo: Vec3, hi: Vec3| {
        let e = (hi - lo).max(Vec3::ZERO);
        e.x * e.y + e.y * e.z + e.z * e.x
    };
    let span = chi - clo;
    let bin_of = |axis: usize, p: &(Prim, Vec3, Vec3)| {
        let c = (p.1[axis] + p.2[axis]) * 0.5;
        (((c - clo[axis]) / span[axis] * BINS as f32) as usize).min(BINS - 1)
    };
    // (cost, axis, first bin of the right half)
    let mut best: Option<(f32, usize, usize)> = None;
    for axis in 0..3 {
        if span[axis] <= 0.0 {
            continue;
        }
        let mut bins = [(0usize, Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)); BINS];
        for p in prims.iter() {
            let b = &mut bins[bin_of(axis, p)];
            b.0 += 1;
            b.1 = b.1.min(p.1);
            b.2 = b.2.max(p.2);
        }
        // Right-to-left sweep first, so the left one can price each split.
        let mut right = [0.0f32; BINS];
        let (mut n, mut rlo, mut rhi) = (0, Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
        for i in (1..BINS).rev() {
            n += bins[i].0;
            rlo = rlo.min(bins[i].1);
            rhi = rhi.max(bins[i].2);
            right[i] = n as f32 * area(rlo, rhi);
        }
        let (mut n, mut llo, mut lhi) = (0, Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
        for i in 1..BINS {
            n += bins[i - 1].0;
            llo = llo.min(bins[i - 1].1);
            lhi = lhi.max(bins[i - 1].2);
            if n == 0 || n == prims.len() {
                continue;
            }
            let cost = n as f32 * area(llo, lhi) + right[i];
            if best.is_none_or(|(c, _, _)| cost < c) {
                best = Some((cost, axis, i));
            }
        }
    }
    let mid = match best {
        Some((_, axis, split)) => {
            // In-place partition: bins below `split` to the front.
            let mut mid = 0;
            for i in 0..prims.len() {
                if bin_of(axis, &prims[i]) < split {
                    prims.swap(i, mid);
                    mid += 1;
                }
            }
            mid
        }
        // Every centroid in one spot: any halving will do.
        None => prims.len() / 2,
    };
    let (l, r) = prims.split_at_mut(mid);
    let li = build_bvh(l, base, nodes);
    let ri = build_bvh(r, base + mid as u32, nodes);
    nodes[idx as usize].first = li;
    nodes[idx as usize].second = ri;
    nodes[idx as usize].count = 0;
    idx
}
