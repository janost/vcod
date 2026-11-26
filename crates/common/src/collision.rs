//! Contains routines ported from the Quake III Arena GPL source, Copyright (C) 1999-2005 Id Software, Inc..
//! See NOTICE.
//!
//! Brush clip planes, terrain triangles, patch facets and a BVH from the
//! BSP collision lumps, swept by a Q3-style `box_trace`. Layouts and why
//! triangles are required: docs/research/bsp-ibsp59-format.md, "Terrain has
//! no brushes".

use crate::clipmap::ClipTrace;
use crate::leafwalk::LeafWalk;
use crate::patch::{Arith, PatchCollide};
use crate::terrain::Terrain;
use glam::Vec3;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, RwLock};

mod build;
mod clip;
mod entities;
mod masks;
mod query;
mod reach;
mod statics;
mod walk;

pub use build::brush_side_planes;
use build::{BvhNode, Volume};
pub(crate) use clip::{Capsule, note_contact};
pub use clip::{
    ModelSurf, clip_entity_box, clip_model_surfs, clip_model_tris, segment_misses_box,
    sight_meets_brush,
};
pub use entities::UnlinkedInstance;
use entities::{ModelInstance, ModelPose, ModelSpan, WorldCopy};
pub use masks::{
    CONTENTS_GLASS, CONTENTS_NODROP, CONTENTS_SOLID, CONTENTS_VEHICLE_CLIP, CONTENTS_WATER,
    MASK_BLAST, MASK_MISSILE, MASK_MISSILE_MOVE, MASK_PLAYERSOLID, MASK_SHOT, SURF_LADDER,
    SURF_SLICK,
};
pub use reach::{ClipState, Reach, reach, recorded};
pub use statics::{StaticModel, StaticModels};

/// A brush as clip planes: point p is inside iff n·p <= d for every plane.
#[derive(Clone)]
pub struct BrushPlanes {
    pub planes: Vec<(Vec3, f32)>,
    /// Lump-0 surface flags of the brush's material (SURF_LADDER and friends).
    pub surface_flags: u32,
    /// Each side's own material's surface flags, parallel to `planes`: what a
    /// trace that stops on that side reports.
    pub side_surface_flags: Vec<u32>,
    /// Lump-0 content flags, reported by `point_contents`.
    pub content_flags: u32,
    /// The brush's index in lump 4 and its material's name, for `describe`.
    pub bsp_index: u32,
    pub material: String,
    /// The lump-27 model the brush belongs to; 0 is the world.
    pub model: u32,
}

/// One collision triangle of a static xmodel, in its model's frame (in the
/// world for [`StaticModels::loose`]). Retail's server clips
/// static models as the bare start-to-end segment of a trace, whatever box
/// or capsule the trace carries, one surface at a time against
/// `contents & mask` (docs/research/cod11-mantle.md, "Static models are
/// clipped as a segment"); the sweep shape never touches these.
#[derive(Clone, Copy, Debug)]
pub struct ModelTri {
    /// Wound so `cross(b - a, c - a)` is the surface's outward normal.
    pub tri: [Vec3; 3],
    /// The xmodel collision surface's `contents` word.
    pub contents: u32,
    /// The surface's `surf_flags`, the sound material in bits 20-24.
    pub surface_flags: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Prim {
    Brush(u32),
    /// A lump-26 terrain triangle: what a trace reports it stopped on.
    Tri(u32),
    /// A lump-24 terrain partition, the unit the BVH holds and the clip
    /// walks, its triangles in lump order.
    Terrain(u32),
    /// A lump-24 patch, all its facets.
    Patch(u32),
    /// A static model's collision triangle, into
    /// [`CollisionWorld::model_tris`]: what a trace reports it stopped on.
    Model(u32),
    /// A placed static model, the unit the BVH holds and the static pass
    /// walks a surface at a time.
    Static(u32),
    /// A player capsule hit, carrying its entity number (`movetrace.rs`).
    Body(u32),
    /// A contact a [`WorldClip`] reported, carrying its contents word.
    Clip(u32),
}

/// A game's own clip of the world (model 0), traced in place of the BVH and
/// leaf walk this module builds from a [`MapWorld`](crate::mapworld::MapWorld): CoD2's
/// `vcod_cod2::collision::ClipMap`. The world's other models still clip
/// here.
pub trait WorldClip: Send + Sync {
    /// A sweep of the box `mins..maxs` from `start` to `end` under `mask`;
    /// `start == end` is a position test.
    fn trace(&self, start: Vec3, end: Vec3, mins: Vec3, maxs: Vec3, mask: u32) -> ClipTrace;
    /// The contents of the world's brushes at `p`.
    fn point_contents(&self, p: Vec3) -> u32;
    /// The game's bullet mask: what [`CollisionWorld::shot_trace`] traces
    /// under on a world with this clip.
    fn shot_mask(&self) -> u32;
    /// The world's ladder brushes as axial boxes, for the bots' graph
    /// (`nav::ladders` reads [`CollisionWorld::brushes`] for the rest).
    fn ladder_boxes(&self) -> Vec<(Vec3, Vec3)> {
        Vec::new()
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Trace {
    pub fraction: f32, // 1.0 = made it to end
    pub endpos: Vec3,
    pub normal: Vec3, // valid when fraction < 1.0
    pub surface_flags: u32,
    pub startsolid: bool,
    pub allsolid: bool,
    /// What the reported contact is against, for diagnostics (`describe`).
    pub hit: Option<Prim>,
    /// The hit's unclamped enter fraction. Only the posed brushes' clip
    /// ([`clip_segment`]) reads it: of two surfaces a box touches at fraction
    /// 0 it reports the one it sits closest to. The world's clips settle a
    /// tie by retail's walk order instead.
    enter: f32,
    /// The entity of the world copy the contact is on
    /// ([`CollisionWorld::set_world_copy`]); `hit` is then the copy's prim.
    copy: Option<u32>,
}

impl Trace {
    /// A trace that reached `end` untouched.
    pub fn clear(end: Vec3) -> Trace {
        Trace {
            fraction: 1.0,
            endpos: end,
            normal: Vec3::ZERO,
            surface_flags: 0,
            startsolid: false,
            allsolid: false,
            hit: None,
            enter: -1.0,
            copy: None,
        }
    }
}

pub const SURFACE_CLIP_EPSILON: f32 = 0.125;

/// What a mover's clip is, for the push's traces: a submodel's brushes, or
/// the world copy an entity with inline model 0 carries (movers doc 20).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pusher {
    Model(usize),
    Copy(u32),
}

/// Where a world trace ends, as `CM_BoxTrace` (`cod_lnxded` 0x8056310)
/// rounds it: the box is centred on `(mins + maxs) / 2`, the start and the
/// end each shifted by it, the delta taken between the shifted points with
/// the start stored as a float (0x80563d1-0x8056420), and the end point
/// `start + fraction * delta` off the unshifted start, rounded once
/// (0x8056887-0x80568c4). On a clear trace that is not `end` in the last
/// bit (docs/research/cod11-player-clip.md 8.9). 1.5b's (0x805aba1) also
/// stores the shifted end (0x805acdd), under [`Arith::Stored`]; the Windows
/// clients keep it (CoDMP15.exe 0x4264d6, cod15-movement.md 8).
pub fn cm_endpos(
    start: Vec3,
    end: Vec3,
    mins: Vec3,
    maxs: Vec3,
    fraction: f32,
    arith: Arith,
) -> Vec3 {
    let off = (mins + maxs) * 0.5;
    let f = f64::from(fraction);
    let axis = |s: f32, e: f32, o: f32| {
        let shifted = s + o;
        let delta = (arith.gcc_var(f64::from(e) + f64::from(o)) - f64::from(shifted)) as f32;
        (f64::from(s) + f * f64::from(delta)) as f32
    };
    Vec3::new(
        axis(start.x, end.x, off.x),
        axis(start.y, end.y, off.y),
        axis(start.z, end.z, off.z),
    )
}

/// Where `SV_Trace` puts a trace an entity's clip cut short of the
/// world's, over the end point the entity's clip left: `start + fraction *
/// (end - start)` off the unshifted points, each component rounded once.
/// 1.1d keeps the delta on the x87 stack (`cod_lnxded` 0x8091a06..
/// 0x8091a51); 1.5b and UO store it as a float (1.5b 0x809a45f..0x809a4d4,
/// UO `coduo_lnxded` 0x809b457..0x809b4cc). Movers doc 22, "On 1.5b and
/// United Offensive".
fn sv_trace_endpos(start: Vec3, end: Vec3, fraction: f32, arith: Arith) -> Vec3 {
    let f = f64::from(fraction);
    Vec3::from(std::array::from_fn::<f32, 3, _>(|i| {
        let delta = arith.gcc_var(f64::from(end[i]) - f64::from(start[i]));
        (f64::from(start[i]) + delta * f) as f32
    }))
}

/// The 5-bit sound-surface index every trace consumer reads
/// (`cod11-events-and-fx.md`, section 4). 0 means the material carries no
/// surfaceparm and retail emits no footstep for it.
pub fn sound_material(surface_flags: u32) -> i32 {
    ((surface_flags >> 20) & 0x1f) as i32
}

pub struct CollisionWorld {
    pub brushes: Vec<BrushPlanes>,
    pub tris: Vec<[Vec3; 3]>,
    /// The static models' collision triangles, each model's once in its
    /// own frame, clipped as a segment only.
    pub model_tris: Vec<ModelTri>,
    /// Their surfaces, runs into `model_tris`.
    static_surfs: Vec<clip::ModelSurf>,
    /// The placed static models, indexed by [`Prim::Static`].
    statics: Vec<statics::StaticPlacement>,
    /// Per-triangle material `surface_flags`, parallel to `tris`. The sound
    /// surface rides bits 20-24 (`cod11-events-and-fx.md`, section 4).
    tris_surf: Vec<u32>,
    /// Per-triangle mask-relevant content flags, parallel to `tris`.
    tris_contents: Vec<u32>,
    /// Lump 24's terrain partitions as retail's records, one triangle record
    /// per entry of `tris`.
    terrain: Terrain,
    /// Per terrain partition, the lump-27 model that owns it.
    terrain_model: Vec<u32>,
    /// Lump 24's patches as retail's facet grids.
    pub patches: Vec<WorldPatch>,
    /// Model 0's BSP tree and what its leaves list, which a trace that
    /// meets a patch walks in retail's order. `None` without a tree.
    leaf_walk: Option<LeafWalk>,
    /// Per terrain triangle, whether every leaf its box reaches lists its
    /// partition, so a trace that stops on it was met on retail's walk.
    tri_listed: Vec<bool>,
    nodes: Vec<BvhNode>,
    /// The subtree holding every [`Prim::Static`] and nothing else, which
    /// the world's walk skips whole and the static pass walks; `u32::MAX`
    /// when the map has none.
    models_root: u32,
    prims: Vec<(Prim, Vec3, Vec3)>,
    water: Vec<Volume>,
    /// Brushes whose material carries [`CONTENTS_NODROP`], for
    /// [`Self::point_contents`] only: nothing clips against them.
    nodrop: Vec<Volume>,
    /// Per lump-27 model, whether its brushes are in the clip. Retail holds
    /// a submodel's brushes only through the entity that links them, so a
    /// deleted `script_brushmodel` takes its brushes out; `set_model_linked`
    /// is that unlink.
    model_linked: Vec<AtomicBool>,
    /// Per lump-27 model, the entity number a trace that ends on its brushes
    /// reports: the world's for model 0 and for any submodel the server has
    /// not named an entity for.
    model_entity: Vec<AtomicU32>,
    /// Per lump-27 model, the `r.contents` its entity linked with, or
    /// [`NO_ENTITY_CONTENTS`]: what a round's trace reads as the contents
    /// of a hit on its brushes ([`Self::shot_contents`]).
    model_contents: Vec<AtomicU64>,
    /// Per lump-27 model, its brushes' run in `brushes`, the entity origin
    /// the stored planes are offset by, and the run's bounds about that
    /// origin: what a [`ModelPose`] moves.
    model_span: Vec<ModelSpan>,
    /// Per lump-27 model, whether a pose has taken its brushes off their
    /// spawn placement: the BVH, built at spawn, then skips them and the
    /// posed pass clips them where the pose puts them.
    model_posed: Vec<AtomicBool>,
    /// The posed models, few and short-lived: a script mover's, while its
    /// entity is anywhere but where the map placed it.
    poses: RwLock<Vec<(usize, ModelPose)>>,
    /// Whether `poses` holds any: a trace reads this and skips the lock,
    /// whose reader count every thread of a parallel nav build otherwise
    /// writes on every trace.
    any_posed: AtomicBool,
    /// Brush entities linked with inline model 0 (a script `spawn` of a
    /// `script_brushmodel` or sine mover): each a copy of the world's own
    /// clip where its entity stands (docs/research/cod11-movers.md 19).
    copies: RwLock<Vec<WorldCopy>>,
    /// Whether `copies` holds any, read like `any_posed`.
    any_copy: AtomicBool,
    /// Entities that each clip their own copy of a submodel where they
    /// stand: a UO vehicle and its collmap, one inline model serving every
    /// vehicle of a model (docs/research/coduo-vehicles.md 10.3).
    instances: RwLock<Vec<ModelInstance>>,
    /// Whether `instances` holds any.
    any_instance: AtomicBool,
    /// Terrain point traces and patches take 1.5b's and United
    /// Offensive's arithmetic ([`Self::set_arith`]).
    arith: AtomicU8,
    /// The world's own clip, when the game brings one
    /// ([`Self::set_world_clip`]).
    world_clip: Option<Arc<dyn WorldClip>>,
    /// The map file's checksum, 0 when unknown ([`Self::set_source`]).
    source: u64,
}

/// A snapshot: the links, poses and entity numbers as they stand now, which
/// the copy then keeps whatever the original does.
impl Clone for CollisionWorld {
    fn clone(&self) -> Self {
        let bools = |v: &[AtomicBool]| {
            v.iter()
                .map(|b| AtomicBool::new(b.load(Ordering::Relaxed)))
                .collect()
        };
        CollisionWorld {
            brushes: self.brushes.clone(),
            tris: self.tris.clone(),
            model_tris: self.model_tris.clone(),
            static_surfs: self.static_surfs.clone(),
            statics: self.statics.clone(),
            tris_surf: self.tris_surf.clone(),
            tris_contents: self.tris_contents.clone(),
            terrain: self.terrain.clone(),
            terrain_model: self.terrain_model.clone(),
            patches: self.patches.clone(),
            leaf_walk: self.leaf_walk.clone(),
            tri_listed: self.tri_listed.clone(),
            nodes: self.nodes.clone(),
            models_root: self.models_root,
            prims: self.prims.clone(),
            water: self.water.clone(),
            nodrop: self.nodrop.clone(),
            model_linked: bools(&self.model_linked),
            model_entity: self
                .model_entity
                .iter()
                .map(|e| AtomicU32::new(e.load(Ordering::Relaxed)))
                .collect(),
            model_contents: self
                .model_contents
                .iter()
                .map(|e| AtomicU64::new(e.load(Ordering::Relaxed)))
                .collect(),
            model_span: self.model_span.clone(),
            model_posed: bools(&self.model_posed),
            poses: RwLock::new(self.poses.read().unwrap_or_else(|e| e.into_inner()).clone()),
            any_posed: AtomicBool::new(self.any_posed.load(Ordering::Acquire)),
            copies: RwLock::new(
                self.copies
                    .read()
                    .unwrap_or_else(|e| e.into_inner())
                    .clone(),
            ),
            any_copy: AtomicBool::new(self.any_copy.load(Ordering::Acquire)),
            instances: RwLock::new(
                self.instances
                    .read()
                    .unwrap_or_else(|e| e.into_inner())
                    .clone(),
            ),
            any_instance: AtomicBool::new(self.any_instance.load(Ordering::Acquire)),
            arith: AtomicU8::new(self.arith.load(Ordering::Acquire)),
            world_clip: self.world_clip.clone(),
            source: self.source,
        }
    }
}

/// A lump-24 patch with its material's words, which the mask test and a
/// hit report read.
#[derive(Clone, Debug)]
pub struct WorldPatch {
    pub collide: PatchCollide,
    /// The same patch in 1.5b's and United Offensive's arithmetic
    /// ([`Arith::Stored`]), where that builds it differently.
    pub stored: Option<PatchCollide>,
    /// The same patch as the Windows clients build it ([`Arith::Msvc`]),
    /// where that differs from `collide`.
    pub msvc: Option<PatchCollide>,
    pub content_flags: u32,
    pub surface_flags: u32,
    /// Its record's index among lump 24's patches, for `describe`.
    pub index: u32,
    /// The lump-27 model that owns it: 0 for the world's.
    pub model: u32,
}

impl WorldPatch {
    /// The build a trace under `arith` clips.
    pub fn clip(&self, arith: Arith) -> &PatchCollide {
        let other = match arith {
            Arith::Kept => None,
            Arith::Stored => self.stored.as_ref(),
            Arith::Msvc => self.msvc.as_ref(),
        };
        other.unwrap_or(&self.collide)
    }
}

impl CollisionWorld {
    /// Which engine build's arithmetic every trace follows: 1.1d's, 1.5b's
    /// and United Offensive's ([`Arith::Stored`]), whose unoptimized code
    /// stores every float variable, or the Windows clients'
    /// ([`Arith::Msvc`]). Under `Stored` a terrain point trace takes
    /// `Terrain::point_clip_stored`, and every trace clips each patch as
    /// its build makes it, which leaves cracks along other facet seams
    /// (docs/research/bsp-ibsp59-format.md, "Patch collision on 1.5 and
    /// UO").
    pub fn set_arith(&self, arith: Arith) {
        self.arith.store(arith as u8, Ordering::Release);
    }

    /// The build every trace runs as ([`Self::set_arith`]).
    pub fn arith(&self) -> Arith {
        match self.arith.load(Ordering::Acquire) {
            x if x == Arith::Stored as u8 => Arith::Stored,
            x if x == Arith::Msvc as u8 => Arith::Msvc,
            _ => Arith::Kept,
        }
    }
}

/// Test helper: flat solid floor (-1024..1024, top at z=0) plus extra axial
/// solid brushes. Always compiled: the client's fx tests use it.
#[doc(hidden)]
#[cfg(any(test, feature = "testing"))]
pub fn test_world(extra: &[(Vec3, Vec3)]) -> CollisionWorld {
    let mut specs: Vec<(usize, [f32; 3], [f32; 3])> =
        vec![(0, [-1024.0, -1024.0, -16.0], [1024.0, 1024.0, 0.0])];
    specs.extend(
        extra
            .iter()
            .map(|(lo, hi)| (0, lo.to_array(), hi.to_array())),
    );
    synthetic_world(&[("textures/test/solid", CONTENTS_SOLID, 0)], &specs)
}

/// Test helper: [`test_world`]'s floor with a ramp rising `deg` degrees along
/// +x from `x0` to `x1`, and level ground at the ramp's height beyond it. The
/// slope arrives as loose triangles, which is the only way this module takes
/// geometry that is not axis-aligned.
#[doc(hidden)]
#[cfg(any(test, feature = "testing"))]
pub fn ramp_test_world(deg: f32, x0: f32, x1: f32) -> CollisionWorld {
    let h = (x1 - x0) * deg.to_radians().tan();
    let (y0, y1) = (-512.0, 512.0);
    let quad = |a: Vec3, b: Vec3, c: Vec3, d: Vec3| [[a, b, c], [a, c, d]];
    let mut tris = Vec::new();
    tris.extend(quad(
        Vec3::new(x0, y0, 0.0),
        Vec3::new(x1, y0, h),
        Vec3::new(x1, y1, h),
        Vec3::new(x0, y1, 0.0),
    ));
    tris.extend(quad(
        Vec3::new(x1, y0, h),
        Vec3::new(1024.0, y0, h),
        Vec3::new(1024.0, y1, h),
        Vec3::new(x1, y1, h),
    ));
    synthetic_world_tris(
        &[("textures/test/solid", CONTENTS_SOLID, 0)],
        &[(0, [-1024.0, -1024.0, -16.0], [1024.0, 1024.0, 0.0])],
        &tris,
    )
}

/// Test helper: axial brushes with named materials `(name, contents, surface)`.
#[doc(hidden)]
#[cfg(any(test, feature = "testing"))]
pub fn synthetic_world(
    materials: &[(&str, u32, u32)],
    brushes: &[(usize, [f32; 3], [f32; 3])],
) -> CollisionWorld {
    synthetic_world_tris(materials, brushes, &[])
}

/// Test helper: [`test_world`]'s floor as model 0 plus one submodel per
/// `submodels` box, each holding a single brush in the model's own local
/// space. `entities` is the entity lump verbatim, since a submodel's
/// classname and `"model" "*N"` are what place its brushes and decide
/// whether they are solid at all. The only shape in which
/// [`CollisionWorld::set_model_linked`] is observable.
#[doc(hidden)]
#[cfg(any(test, feature = "testing"))]
pub fn submodel_test_world(entities: &str, submodels: &[([f32; 3], [f32; 3])]) -> CollisionWorld {
    let side = |v: f32| crate::mapworld::BrushSide {
        plane_or_dist: v.to_bits(),
        material: 0,
    };
    let floor = ([-1024.0, -1024.0, -16.0], [1024.0, 1024.0, 0.0]);
    let mut brush_sides = Vec::new();
    let mut brushes = Vec::new();
    let mut models = Vec::new();
    for (i, (lo, hi)) in std::iter::once(&floor).chain(submodels).enumerate() {
        brushes.push(crate::mapworld::Brush {
            first_side: (i * 6) as u32,
            num_sides: 6,
            material: 0,
        });
        for axis in 0..3 {
            brush_sides.push(side(lo[axis]));
            brush_sides.push(side(hi[axis]));
        }
        models.push(crate::mapworld::Model {
            mins: *lo,
            maxs: *hi,
            first_soup: 0,
            num_soups: 0,
            first_brush: i as u32,
            partitions: (0, 0),
            num_brushes: 1,
        });
    }
    CollisionWorld::build(
        &crate::mapworld::MapWorld {
            materials: vec![crate::mapworld::Material {
                name: "textures/test/solid".into(),
                surface_flags: 0,
                content_flags: CONTENTS_SOLID,
            }],
            entities: entities.to_string(),
            brush_sides,
            brushes,
            models,
            ..Default::default()
        },
        &[],
    )
}

/// A test map's lump 24: each triangle `[a, b, c]` as a flat 3x3 patch of
/// `material` whose last row collapses onto `c`, its front the side
/// `cross(b - a, c - a)` points to.
#[doc(hidden)]
#[cfg(any(test, feature = "testing"))]
pub fn push_tri_patches(bsp: &mut crate::mapworld::MapWorld, tris: &[[Vec3; 3]], material: u16) {
    for &[a, b, c] in tris {
        let mid = |p: Vec3, q: Vec3| (p + q) * 0.5;
        let ab = mid(a, b);
        let rows = [[a, ab, b], [mid(a, c), mid(ab, c), mid(b, c)], [c, c, c]];
        let first_vert = bsp.collision_verts.len() as u32;
        bsp.collision_verts
            .extend(rows.iter().flatten().map(|v| v.to_array()));
        bsp.patches.push(crate::mapworld::PatchPart {
            material,
            width: 3,
            height: 3,
            tolerance: 4,
            first_vert,
        });
    }
}

/// A test map's lump 24: `tris` as one terrain partition of `material`,
/// each front the side `cross(b - a, c - a)` points to.
#[doc(hidden)]
#[cfg(any(test, feature = "testing"))]
pub fn push_tri_terrain(bsp: &mut crate::mapworld::MapWorld, tris: &[[Vec3; 3]], material: u16) {
    let first_vert = bsp.collision_verts.len() as u32;
    let first_index = bsp.collision_indices.len() as u32;
    for (i, tri) in tris.iter().enumerate() {
        bsp.collision_verts.extend(tri.iter().map(|v| v.to_array()));
        let i = (i * 3) as u16;
        // The build reads a lump-26 triangle `a, c, b`.
        bsp.collision_indices.extend([i, i + 2, i + 1]);
    }
    bsp.terrain.push(crate::mapworld::TerrainPart {
        material,
        first_vert,
        vert_count: (tris.len() * 3) as u16,
        first_index,
        index_count: (tris.len() * 3) as u16,
    });
}

/// [`synthetic_world`] plus world-space triangles as terrain of material 0
/// ([`push_tri_terrain`]), the only way this module takes swept geometry
/// that is not axis-aligned.
#[doc(hidden)]
#[cfg(any(test, feature = "testing"))]
pub fn synthetic_world_tris(
    materials: &[(&str, u32, u32)],
    brushes: &[(usize, [f32; 3], [f32; 3])],
    tris: &[[Vec3; 3]],
) -> CollisionWorld {
    let mut bsp = synthetic_bsp(materials, brushes);
    push_tri_terrain(&mut bsp, tris, 0);
    CollisionWorld::build(&bsp, &[])
}

/// [`synthetic_world`] plus world-space triangles as patches of material 0
/// ([`push_tri_patches`]).
#[doc(hidden)]
#[cfg(any(test, feature = "testing"))]
pub fn synthetic_world_patches(
    materials: &[(&str, u32, u32)],
    brushes: &[(usize, [f32; 3], [f32; 3])],
    tris: &[[Vec3; 3]],
) -> CollisionWorld {
    let mut bsp = synthetic_bsp(materials, brushes);
    push_tri_patches(&mut bsp, tris, 0);
    CollisionWorld::build(&bsp, &[])
}

/// A model-0 map of named materials and axial brushes.
#[cfg(any(test, feature = "testing"))]
fn synthetic_bsp(
    materials: &[(&str, u32, u32)],
    brushes: &[(usize, [f32; 3], [f32; 3])],
) -> crate::mapworld::MapWorld {
    let side = |m: u32, v: f32| crate::mapworld::BrushSide {
        plane_or_dist: v.to_bits(),
        material: m,
    };
    let mut brush_sides = Vec::new();
    let mut bsp_brushes = Vec::new();
    for (i, (mat, lo, hi)) in brushes.iter().enumerate() {
        bsp_brushes.push(crate::mapworld::Brush {
            first_side: (i * 6) as u32,
            num_sides: 6,
            material: *mat as u16,
        });
        for axis in 0..3 {
            brush_sides.push(side(*mat as u32, lo[axis]));
            brush_sides.push(side(*mat as u32, hi[axis]));
        }
    }
    let n = bsp_brushes.len() as u32;
    let (mut mins, mut maxs) = ([0.0f32; 3], [0.0f32; 3]);
    for (_, lo, hi) in brushes {
        for axis in 0..3 {
            mins[axis] = mins[axis].min(lo[axis]);
            maxs[axis] = maxs[axis].max(hi[axis]);
        }
    }
    crate::mapworld::MapWorld {
        materials: materials
            .iter()
            .map(|(name, content, surface)| crate::mapworld::Material {
                name: name.to_string(),
                surface_flags: *surface,
                content_flags: *content,
            })
            .collect(),
        brush_sides,
        brushes: bsp_brushes,
        models: vec![crate::mapworld::Model {
            mins,
            maxs,
            first_soup: 0,
            num_soups: 0,
            first_brush: 0,
            partitions: (0, 0),
            num_brushes: n,
        }],
        ..Default::default()
    }
}

#[cfg(test)]
mod tests;
