//! Map assets: the collision map, the render world, the light list
//! (`com_map`), the game worlds and the entity string.

use super::fx::fx;
use super::material::{Image, Material, image, material};
use super::misc::{LightDef, light_def, phys_preset};
use super::model::{XModel, brush_sides, xmodel};
use super::{Id, asset, push};
use crate::walk::{Follow, Handle, S, W, u16s};
use crate::zone::{Block, Pointer};
use anyhow::Result;
use std::io::BufRead;

/// Raw bytes kept under [`crate::walk::WalkOptions::keep_bulk`].
pub type Bulk = Option<Vec<u8>>;

/// `clipMap_t`, 0x11C: `col_map_mp` and `col_map_sp` share it.
#[derive(Clone, Debug, Default)]
pub struct ClipMap {
    pub name: String,
    pub plane_count: u32,
    pub static_model_count: u32,
    pub material_count: u32,
    pub brush_side_count: u32,
    pub brush_edge_count: u32,
    pub node_count: u32,
    pub leaf_count: u32,
    pub leaf_brush_node_count: u32,
    pub leaf_brush_count: u32,
    pub leaf_surface_count: u32,
    pub vert_count: u32,
    pub tri_count: u32,
    pub border_count: u32,
    pub partition_count: u32,
    pub aabb_tree_count: u32,
    pub submodel_count: u32,
    pub brush_count: u16,
    pub cluster_count: u32,
    pub cluster_bytes: u32,
    pub dyn_ent_count: [u16; 2],
    pub checksum: u32,
    /// `cplane_s`, 0x14 each.
    pub planes: Bulk,
    /// One per `cStaticModel_s` (0x50): its model and the struct.
    pub static_models: Vec<(Option<Id<XModel>>, Vec<u8>)>,
    /// `dmaterial_t`, 0x48 each: a 64-byte name, surface and content flags.
    pub materials: Vec<u8>,
    /// `cbrushside_t`, 0xC each (plane pointers as stored).
    pub brush_sides: Bulk,
    pub brush_edges: Bulk,
    /// `cNode_t`, 8 each.
    pub nodes: Bulk,
    /// `cLeaf_t`, 0x2C each.
    pub leafs: Bulk,
    pub leaf_brushes: Bulk,
    pub leaf_brush_nodes: Bulk,
    /// Per leaf brush node, a leaf's (positive count) brush list, copied
    /// out of `leaf_brushes` when its slot is an offset into them.
    pub leaf_brush_lists: Vec<Option<Vec<u16>>>,
    pub leaf_surfaces: Bulk,
    /// vec3 per vertex.
    pub verts: Bulk,
    /// Three u16 per triangle.
    pub tri_indices: Bulk,
    pub tri_edge_walkable: Bulk,
    /// `CollisionBorder`, 0x1C each.
    pub borders: Bulk,
    /// `CollisionPartition`, 0xC each.
    pub partitions: Bulk,
    /// `CollisionAabbTree`, 0x20 each.
    pub aabb_trees: Bulk,
    /// `cmodel_t`, 0x48 each.
    pub cmodels: Bulk,
    /// `cbrush_t`, 0x50 each (pointers as stored).
    pub brushes: Bulk,
    pub visibility: Bulk,
    pub map_ents: Option<Id<MapEnts>>,
    /// Where the arrays other structs point into landed, for turning
    /// those pointers into indices.
    pub at: ClipAddrs,
}

/// Packed addresses ([`crate::walk::addr`]) of the clip map's arrays that
/// stored pointers name: a brush side's plane, a node's plane, a brush's
/// sides and edges, a partition's borders.
#[derive(Clone, Copy, Debug, Default)]
pub struct ClipAddrs {
    pub planes: u32,
    pub brush_sides: u32,
    pub brush_edges: u32,
    pub borders: u32,
}

/// `col_map_sp`: the same `clipMap_t`.
#[derive(Clone, Debug, Default)]
pub struct ClipMapSp(pub ClipMap);

pub fn clip_map<R: BufRead>(w: &mut W<R>, p: Pointer, slot: u32) -> Result<Option<Id<ClipMapSp>>> {
    asset(w, p, slot, Handle::ASSET, |w| {
        let c = clip_map_t(w)?;
        Ok(push(&mut w.out.clip_maps, ClipMapSp(c)))
    })
}

pub fn clip_map_pvs<R: BufRead>(
    w: &mut W<R>,
    p: Pointer,
    slot: u32,
) -> Result<Option<Id<ClipMap>>> {
    asset(w, p, slot, Handle::ASSET, |w| {
        let c = clip_map_t(w)?;
        Ok(push(&mut w.out.clip_maps_pvs, c))
    })
}

/// `n` structs of `size` at `align` behind the slot at `o`, kept as bulk.
fn array<R: BufRead>(w: &mut W<R>, s: &S, o: usize, align: u32, n: usize) -> Result<Bulk> {
    Ok(if w.inline_only(s.ptr(o), align)? {
        w.bulk(align, n)?
    } else {
        None
    })
}

fn clip_map_t<R: BufRead>(w: &mut W<R>) -> Result<ClipMap> {
    let s = w.load(0x11C)?;
    w.push(Block::Virtual);
    let n = |o: usize| s.u32(o) as usize;
    let mut c = ClipMap {
        name: w.xstring(s.ptr(0))?.unwrap_or_default(),
        plane_count: s.u32(8),
        static_model_count: s.u32(16),
        material_count: s.u32(24),
        brush_side_count: s.u32(32),
        brush_edge_count: s.u32(40),
        node_count: s.u32(48),
        leaf_count: s.u32(56),
        leaf_brush_node_count: s.u32(64),
        leaf_brush_count: s.u32(72),
        leaf_surface_count: s.u32(80),
        vert_count: s.u32(88),
        tri_count: s.u32(96),
        border_count: s.u32(108),
        partition_count: s.u32(116),
        aabb_tree_count: s.u32(124),
        submodel_count: s.u32(132),
        brush_count: s.u16(140),
        cluster_count: s.u32(148),
        cluster_bytes: s.u32(152),
        dyn_ent_count: [s.u16(244), s.u16(246)],
        checksum: s.u32(280),
        ..ClipMap::default()
    };
    // The render world's dpvs planes, when it loaded first.
    (c.at.planes, c.planes) = w.shared_at(s.ptr(12), 4, 0x14 * n(8), "planes", true)?;
    if w.inline_only(s.ptr(20), 4)? {
        let a = w.load(0x50 * n(16))?;
        for i in 0..n(16) {
            let e = a.elem(i, 0x50);
            let m = xmodel(w, e.ptr(4), e.slot(4))?;
            c.static_models.push((m, e.b));
        }
    }
    if w.inline_only(s.ptr(28), 4)? {
        c.materials = w.bytes(4, 0x48 * n(24))?;
    }
    if w.inline_only(s.ptr(36), 4)? {
        c.at.brush_sides = w.here()?;
        let b = brush_sides(w, n(32))?;
        c.brush_sides = w.keep_bulk().then_some(b);
    }
    if w.inline_only(s.ptr(44), 1)? {
        c.at.brush_edges = w.here()?;
        c.brush_edges = w.bulk(1, n(40))?;
    }
    if w.inline_only(s.ptr(52), 4)? {
        let a = w.load(8 * n(48))?;
        for i in 0..n(48) {
            if let Follow::Inline = w.follow(a.elem(i, 8).ptr(0), 4)? {
                w.load(0x14)?;
            }
        }
        c.nodes = w.keep_bulk().then_some(a.b);
    }
    c.leafs = array(w, &s, 60, 4, 0x2C * n(56))?;
    if w.inline_only(s.ptr(76), 2)? {
        let at = w.here()?;
        c.leaf_brushes = w.bulk(2, 2 * n(72))?;
        if let Some(b) = &c.leaf_brushes {
            w.pool("leaf brushes", at, b.clone());
        }
    }
    if w.inline_only(s.ptr(68), 4)? {
        let a = w.load(0x14 * n(64))?;
        for i in 0..n(64) {
            let e = a.elem(i, 0x14);
            let count = e.i16(2);
            let list = if count > 0 {
                w.shared(e.ptr(8), 2, 2 * count as usize, "leaf brushes", true)?
            } else {
                None
            };
            c.leaf_brush_lists.push(list.map(|b| u16s(&b)));
        }
        c.leaf_brush_nodes = w.keep_bulk().then_some(a.b);
    }
    c.leaf_surfaces = array(w, &s, 84, 4, 4 * n(80))?;
    c.verts = array(w, &s, 92, 4, 12 * n(88))?;
    c.tri_indices = array(w, &s, 100, 2, 6 * n(96))?;
    c.tri_edge_walkable = array(w, &s, 104, 1, (3 * n(96)).div_ceil(32) * 4)?;
    if w.inline_only(s.ptr(112), 4)? {
        c.at.borders = w.here()?;
        c.borders = w.bulk(4, 0x1C * n(108))?;
    }
    if w.inline_only(s.ptr(120), 4)? {
        let a = w.load(0xC * n(116))?;
        for i in 0..n(116) {
            if let Follow::Inline = w.follow(a.elem(i, 0xC).ptr(8), 4)? {
                w.load(0x1C)?;
            }
        }
        c.partitions = w.keep_bulk().then_some(a.b);
    }
    c.aabb_trees = array(w, &s, 128, 4, 0x20 * n(124))?;
    c.cmodels = array(w, &s, 136, 4, 0x48 * n(132))?;
    if w.inline_only(s.ptr(144), 16)? {
        let nb = s.u16(140) as usize;
        let a = w.load(0x50 * nb)?;
        for i in 0..nb {
            cbrush(w, &a.elem(i, 0x50))?;
        }
        c.brushes = w.keep_bulk().then_some(a.b);
    }
    c.visibility = array(w, &s, 156, 1, n(148) * n(152))?;
    c.map_ents = map_ents(w, s.ptr(164), s.slot(164))?;
    match w.follow(s.ptr(168), 16)? {
        Follow::Inline => {
            let b = w.load(0x50)?;
            cbrush(w, &b)?;
        }
        Follow::At(_) | Follow::Null => {}
    }
    for k in 0..2 {
        let count = s.u16(244 + 2 * k) as usize;
        if w.inline_only(s.ptr(248 + 4 * k), 4)? {
            let a = w.load(0x60 * count)?;
            for i in 0..count {
                dyn_ent_def(w, &a.elem(i, 0x60))?;
            }
        }
    }
    // DynEntityPose, DynEntityClient, DynEntityColl: runtime only.
    for (o, size) in [(256, 0x20), (264, 0xC), (272, 0x14)] {
        for k in 0..2 {
            let count = s.u16(244 + 2 * k) as usize;
            runtime(w, &s, o + 4 * k, 4, size * count)?;
        }
    }
    w.pop()?;
    Ok(c)
}

/// A runtime array behind the slot at `o`: block-1 space, nothing read.
fn runtime<R: BufRead>(w: &mut W<R>, s: &S, o: usize, align: u32, n: usize) -> Result<()> {
    w.push(Block::Runtime);
    if w.inline_only(s.ptr(o), align)? {
        w.bulk(align, n)?;
    }
    w.pop()
}

/// `cbrush_t`, 0x50: its side and adjacency slots, each one struct inline
/// or (as in stock zones) an offset into the map's arrays.
fn cbrush<R: BufRead>(w: &mut W<R>, b: &S) -> Result<()> {
    if let Follow::Inline = w.follow(b.ptr(32), 4)? {
        brush_sides(w, 1)?;
    }
    if let Follow::Inline = w.follow(b.ptr(48), 1)? {
        w.load(1)?;
    }
    Ok(())
}

/// `DynEntityDef`, 0x60.
fn dyn_ent_def<R: BufRead>(w: &mut W<R>, d: &S) -> Result<()> {
    xmodel(w, d.ptr(32), d.slot(32))?;
    fx(w, d.ptr(40), d.slot(40))?;
    if let Follow::Inline = w.follow(d.ptr(44), 4)? {
        // XModelPieces, 0xC: name, count, 0x10-byte pieces each a model.
        let p = w.load(0xC)?;
        w.xstring(p.ptr(0))?;
        if w.inline_only(p.ptr(8), 4)? {
            let n = p.i32(4).max(0) as usize;
            let a = w.load(0x10 * n)?;
            for i in 0..n {
                xmodel(w, a.ptr(0x10 * i), a.slot(0x10 * i))?;
            }
        }
    }
    phys_preset(w, d.ptr(48), d.slot(48))?;
    Ok(())
}

/// `MapEnts`, 0xC: name, entity string, its length (with the NUL).
#[derive(Clone, Debug)]
pub struct MapEnts {
    pub name: String,
    pub entity_string: Vec<u8>,
}

pub fn map_ents<R: BufRead>(w: &mut W<R>, p: Pointer, slot: u32) -> Result<Option<Id<MapEnts>>> {
    asset(w, p, slot, Handle::ASSET, |w| {
        let s = w.load(0xC)?;
        w.push(Block::Virtual);
        let name = w.xstring(s.ptr(0))?.unwrap_or_default();
        let mut entity_string = Vec::new();
        if w.inline_only(s.ptr(4), 1)? {
            entity_string = w.bytes(1, s.u32(8) as usize)?;
        }
        w.pop()?;
        Ok(push(
            &mut w.out.map_ents,
            MapEnts {
                name,
                entity_string,
            },
        ))
    })
}

/// `ComWorld`, 0x10: name, `isInUse`, the primary lights.
#[derive(Clone, Debug)]
pub struct ComWorld {
    pub name: String,
    /// `ComPrimaryLight`, 0x44 each: the struct and its light def's name.
    pub primary_lights: Vec<(Vec<u8>, Option<String>)>,
}

pub fn com_world<R: BufRead>(w: &mut W<R>, p: Pointer, slot: u32) -> Result<Option<Id<ComWorld>>> {
    asset(w, p, slot, Handle::ASSET, |w| {
        let s = w.load(0x10)?;
        w.push(Block::Virtual);
        let name = w.xstring(s.ptr(0))?.unwrap_or_default();
        let mut primary_lights = Vec::new();
        if w.inline_only(s.ptr(12), 4)? {
            let n = s.u32(8) as usize;
            let a = w.load(0x44 * n)?;
            for i in 0..n {
                let e = a.elem(i, 0x44);
                let def = w.xstring(e.ptr(64))?;
                primary_lights.push((e.b, def));
            }
        }
        w.pop()?;
        Ok(push(
            &mut w.out.com_worlds,
            ComWorld {
                name,
                primary_lights,
            },
        ))
    })
}

/// `GameWorldMp`, 4: only a name.
#[derive(Clone, Debug)]
pub struct GameWorldMp {
    pub name: String,
}

pub fn game_world_mp<R: BufRead>(
    w: &mut W<R>,
    p: Pointer,
    slot: u32,
) -> Result<Option<Id<GameWorldMp>>> {
    asset(w, p, slot, Handle::ASSET, |w| {
        let s = w.load(4)?;
        w.push(Block::Virtual);
        let name = w.xstring(s.ptr(0))?.unwrap_or_default();
        w.pop()?;
        Ok(push(&mut w.out.game_worlds_mp, GameWorldMp { name }))
    })
}

/// `GameWorldSp`, 0x2C: name and the AI's `PathData`.
#[derive(Clone, Debug)]
pub struct GameWorldSp {
    pub name: String,
    pub node_count: u32,
}

pub fn game_world_sp<R: BufRead>(
    w: &mut W<R>,
    p: Pointer,
    slot: u32,
) -> Result<Option<Id<GameWorldSp>>> {
    asset(w, p, slot, Handle::ASSET, |w| {
        let s = w.load(0x2C)?;
        w.push(Block::Virtual);
        let name = w.xstring(s.ptr(0))?.unwrap_or_default();
        // PathData at 4.
        let nodes = s.u32(4) as usize;
        if w.inline_only(s.ptr(8), 4)? {
            let a = w.load(0x80 * nodes)?;
            for i in 0..nodes {
                let e = a.elem(i, 0x80);
                if w.inline_only(e.ptr(64), 4)? {
                    w.bulk(4, 0xC * e.u16(62) as usize)?;
                }
            }
        }
        runtime(w, &s, 12, 16, 0x10 * nodes)?;
        for o in [20, 24] {
            if w.inline_only(s.ptr(o), 2)? {
                w.bulk(2, 2 * nodes)?;
            }
        }
        if w.inline_only(s.ptr(32), 1)? {
            w.bulk(1, s.u32(28) as usize)?;
        }
        if w.inline_only(s.ptr(40), 4)? {
            let n = s.u32(36) as usize;
            let a = w.load(0x10 * n)?;
            for i in 0..n {
                path_tree(w, &a.elem(i, 0x10))?;
            }
        }
        w.pop()?;
        Ok(push(
            &mut w.out.game_worlds_sp,
            GameWorldSp {
                name,
                node_count: nodes as u32,
            },
        ))
    })
}

/// `pathnode_tree_t`, 0x10: a split (`axis >= 0`, two child slots) or a
/// leaf (a u16 node list).
fn path_tree<R: BufRead>(w: &mut W<R>, t: &S) -> Result<()> {
    if t.i32(0) >= 0 {
        for o in [8, 12] {
            if let Follow::Inline = w.follow(t.ptr(o), 4)? {
                let c = w.load(0x10)?;
                path_tree(w, &c)?;
            }
        }
    } else if w.inline_only(t.ptr(12), 2)? {
        w.bulk(2, 2 * t.i32(8).max(0) as usize)?;
    }
    Ok(())
}

/// `GfxLightRegionHull`, 0x50: a k-DOP over the axes x, y, z, x + y,
/// x - y, x + z, x - z, y + z, y - z (its mid points and half sizes), and
/// its extra axes (`GfxLightRegionAxis`, 0x14).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LightRegionHull {
    pub kdop_mid: [f32; 9],
    pub kdop_half: [f32; 9],
    pub axes: Vec<LightRegionAxis>,
}

/// `GfxLightRegionAxis`: a direction, the hull's mid point and half size
/// along it.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct LightRegionAxis {
    pub dir: [f32; 3],
    pub mid: f32,
    pub half: f32,
}

/// `GfxWorld`, 0x2DC.
#[derive(Clone, Debug, Default)]
pub struct GfxWorld {
    pub name: String,
    pub base_name: Option<String>,
    pub plane_count: u32,
    pub node_count: u32,
    pub index_count: u32,
    pub surface_count: u32,
    pub sky_surf_count: u32,
    pub vertex_count: u32,
    pub vertex_layer_data_size: u32,
    pub sun_primary_light_index: u32,
    pub primary_light_count: u32,
    pub cull_group_count: u32,
    pub reflection_probe_count: u32,
    pub cell_count: u32,
    pub lightmap_count: u32,
    pub model_count: u32,
    pub mins: [f32; 3],
    pub maxs: [f32; 3],
    pub checksum: u32,
    pub sky_image: Option<Id<Image>>,
    pub sun_light: Option<Id<LightDef>>,
    pub outdoor_image: Option<Id<Image>>,
    /// Primary and secondary image per lightmap.
    pub lightmaps: Vec<[Option<Id<Image>>; 2]>,
    pub reflection_probes: Vec<Option<Id<Image>>>,
    /// `GfxReflectionProbe::origin`, probe for probe.
    pub reflection_probe_origins: Vec<[f32; 3]>,
    /// `lightRegion`: each primary light's hulls (`GfxLightRegion`, 8;
    /// none kept when the walk keeps no bulk data).
    pub light_regions: Vec<Vec<LightRegionHull>>,
    /// `materialMemory`: each material the world uses.
    pub materials: Vec<Option<Id<Material>>>,
    /// `GfxSurface`, 0x30 each, and its material.
    pub surfaces: Vec<(Option<Id<Material>>, Vec<u8>)>,
    /// `GfxStaticModelDrawInst`, 0x4C each, and its model.
    pub static_models: Vec<(Option<Id<XModel>>, Vec<u8>)>,
    /// `GfxWorldVertex`, 0x2C each.
    pub vertices: Bulk,
    pub vertex_layer_data: Bulk,
    /// u16 indices.
    pub indices: Bulk,
    pub cells: Vec<GfxCell>,
    /// `dpvsPlanes.planes`: `cplane_s`, 0x14 each.
    pub planes: Bulk,
    /// `dpvsPlanes.nodes`: u16 per node, the cell lookup's tree.
    pub dpvs_nodes: Bulk,
    /// `GfxBrushModel`, 0x38 each.
    pub models: Bulk,
    /// `dpvs.sortedSurfIndex`: u16 surface indices, what a cell tree's
    /// `startSurfIndex` ranges index.
    pub sorted_surf_index: Bulk,
    /// `skySurfIndices`: i32 surface indices, `skySurfCount` of them.
    pub sky_surfaces: Bulk,
    /// `GfxCullGroup`, 0x20 each
    /// (`cullGroupCount`): bounds, `surfaceCount`, `startSurfIndex`.
    pub cull_groups: Bulk,
    /// `GfxLightGrid` at 272.
    pub light_grid: GfxLightGrid,
    /// `dpvs.smodelInsts`: `GfxStaticModelInst`, 0x1C each: bounds and
    /// the packed ground lighting.
    pub smodel_insts: Bulk,
}

/// `GfxLightGrid`, 0x38 at `GfxWorld` + 272 (cod4-world.md 3.1).
#[derive(Clone, Debug, Default)]
pub struct GfxLightGrid {
    pub has_light_regions: bool,
    pub sun_primary_light_index: u32,
    pub mins: [u16; 3],
    pub maxs: [u16; 3],
    pub row_axis: u32,
    pub col_axis: u32,
    /// u16 per row, 0xFFFF for none: the row's offset in `raw_row_data`
    /// over 4.
    pub row_data_start: Bulk,
    pub raw_row_data: Bulk,
    /// `GfxLightGridEntry`, 4 bytes each: `colorsIndex` u16,
    /// `primaryLightIndex` u8, `needsTrace` u8.
    pub entries: Bulk,
    /// `GfxLightGridColors`, 56 RGB triples (0xA8) each.
    pub colors: Bulk,
}

pub fn gfx_world<R: BufRead>(w: &mut W<R>, p: Pointer, slot: u32) -> Result<Option<Id<GfxWorld>>> {
    asset(w, p, slot, Handle::ASSET, |w| {
        let s = w.load(0x2DC)?;
        w.push(Block::Virtual);
        let g = gfx_world_body(w, &s)?;
        w.pop()?;
        Ok(push(&mut w.out.gfx_worlds, g))
    })
}

fn gfx_world_body<R: BufRead>(w: &mut W<R>, s: &S) -> Result<GfxWorld> {
    let n = |o: usize| s.u32(o) as usize;
    let mut g = GfxWorld {
        name: w.xstring(s.ptr(0))?.unwrap_or_default(),
        base_name: w.xstring(s.ptr(4))?,
        plane_count: s.u32(8),
        node_count: s.u32(12),
        index_count: s.u32(16),
        surface_count: s.u32(24),
        sky_surf_count: s.u32(32),
        vertex_count: s.u32(48),
        vertex_layer_data_size: s.u32(60),
        sun_primary_light_index: s.u32(216),
        primary_light_count: s.u32(220),
        cull_group_count: s.u32(224),
        reflection_probe_count: s.u32(228),
        cell_count: s.u32(240),
        lightmap_count: s.u32(264),
        model_count: s.u32(336),
        mins: s.vec3(344),
        maxs: s.vec3(356),
        checksum: s.u32(368),
        ..GfxWorld::default()
    };
    g.indices = array(w, s, 20, 2, 2 * n(16))?;
    g.sky_surfaces = array(w, s, 36, 4, 4 * n(32))?;
    g.sky_image = image(w, s.ptr(40), s.slot(40))?;
    match w.follow(s.ptr(200), 4)? {
        Follow::Inline => {
            let at = w.here()?;
            let l = w.load(0x40)?;
            g.sun_light = light_def(w, l.ptr(60), l.slot(60))?;
            w.remember("sun light", at, g.sun_light);
        }
        Follow::At(a) => g.sun_light = w.recall("sun light", a).flatten(),
        Follow::Null => {}
    }
    let probes = n(228);
    if w.inline_only(s.ptr(232), 4)? {
        let a = w.load(0x10 * probes)?;
        for i in 0..probes {
            g.reflection_probe_origins.push(a.vec3(0x10 * i));
            g.reflection_probes
                .push(image(w, a.ptr(0x10 * i + 12), a.slot(0x10 * i + 12))?);
        }
    }
    runtime(w, s, 236, 4, 4 * probes)?;
    // GfxWorldDpvsPlanes at 240: cellCount, planes, nodes, cell bits.
    let cells = n(240);
    g.planes = w.shared(s.ptr(244), 4, 0x14 * n(8), "planes", true)?;
    g.dpvs_nodes = array(w, s, 248, 2, 2 * n(12))?;
    runtime(w, s, 252, 4, 4 * cells * 256)?;
    if w.inline_only(s.ptr(260), 4)? {
        let a = w.load(0x38 * cells)?;
        for i in 0..cells {
            let c = gfx_cell(w, &a.elem(i, 0x38), a.at, cells)?;
            g.cells.push(c);
        }
    }
    let lightmaps = n(264);
    if w.inline_only(s.ptr(268), 4)? {
        let a = w.load(8 * lightmaps)?;
        for i in 0..lightmaps {
            let p0 = image(w, a.ptr(8 * i), a.slot(8 * i))?;
            let p1 = image(w, a.ptr(8 * i + 4), a.slot(8 * i + 4))?;
            g.lightmaps.push([p0, p1]);
        }
    }
    // GfxLightGrid at 272.
    let row_axis = s.u32(292) as usize;
    let lg = &mut g.light_grid;
    lg.has_light_regions = s.b[272] != 0;
    lg.sun_primary_light_index = s.u32(276);
    lg.mins = [0, 1, 2].map(|k| s.u16(280 + 2 * k));
    lg.maxs = [0, 1, 2].map(|k| s.u16(286 + 2 * k));
    lg.row_axis = s.u32(292);
    lg.col_axis = s.u32(296);
    if row_axis < 3 {
        let span = s.u16(286 + 2 * row_axis) as usize + 1 - s.u16(280 + 2 * row_axis) as usize;
        lg.row_data_start = array(w, s, 300, 2, 2 * span)?;
    }
    lg.raw_row_data = array(w, s, 308, 1, n(304))?;
    lg.entries = array(w, s, 316, 4, 4 * n(312))?;
    lg.colors = array(w, s, 324, 4, 0xA8 * n(320))?;
    runtime(w, s, 328, 4, 4 * lightmaps)?;
    runtime(w, s, 332, 4, 4 * lightmaps)?;
    g.models = array(w, s, 340, 4, 0x38 * n(336))?;
    if w.inline_only(s.ptr(376), 4)? {
        let count = n(372);
        let a = w.load(8 * count)?;
        for i in 0..count {
            g.materials.push(material(w, a.ptr(8 * i), a.slot(8 * i))?);
        }
    }
    g.vertices = array(w, s, 52, 4, 0x2C * n(48))?;
    g.vertex_layer_data = array(w, s, 64, 1, n(60))?;
    material(w, s.ptr(384), s.slot(384))?;
    material(w, s.ptr(388), s.slot(388))?;
    g.outdoor_image = image(w, s.ptr(540), s.slot(540))?;
    let dyn_count = [n(692), n(696)];
    let shadowed = n(220).saturating_sub(n(216) + 1);
    runtime(w, s, 544, 4, 4 * cells * cells.div_ceil(32))?;
    runtime(w, s, 548, 4, 6 * dyn_count[0])?;
    runtime(w, s, 552, 4, 4 * dyn_count[1])?;
    runtime(w, s, 556, 4, 4 * shadowed * 4096)?;
    runtime(w, s, 560, 4, 4 * dyn_count[0] * shadowed)?;
    runtime(w, s, 564, 4, 4 * dyn_count[1] * shadowed)?;
    runtime(w, s, 568, 1, dyn_count[0])?;
    let lights = n(220);
    if w.inline_only(s.ptr(572), 4)? {
        let a = w.load(0xC * lights)?;
        for i in 0..lights {
            let e = a.elem(i, 0xC);
            array(w, &e, 4, 2, 2 * e.u16(0) as usize)?;
            array(w, &e, 8, 2, 2 * e.u16(2) as usize)?;
        }
    }
    if w.inline_only(s.ptr(576), 4)? {
        let a = w.load(8 * lights)?;
        for i in 0..lights {
            let r = a.elem(i, 8);
            let mut region = Vec::new();
            if w.inline_only(r.ptr(4), 4)? {
                let hulls = r.u32(0) as usize;
                let h = w.load(0x50 * hulls)?;
                for k in 0..hulls {
                    let e = h.elem(k, 0x50);
                    let count = e.u32(72) as usize;
                    let axes = array(w, &e, 76, 4, 0x14 * count)?;
                    region.push(LightRegionHull {
                        kdop_mid: std::array::from_fn(|j| e.f32(4 * j)),
                        kdop_half: std::array::from_fn(|j| e.f32(36 + 4 * j)),
                        axes: axes.map_or_else(Vec::new, |b| {
                            let b = S { at: 0, b };
                            (0..count)
                                .map(|j| LightRegionAxis {
                                    dir: b.vec3(0x14 * j),
                                    mid: b.f32(0x14 * j + 12),
                                    half: b.f32(0x14 * j + 16),
                                })
                                .collect()
                        }),
                    });
                }
            }
            g.light_regions.push(region);
        }
    }
    // GfxWorldDpvsStatic at 580.
    let d = s.sub(580, 0x68);
    let smodels = d.u32(0) as usize;
    let static_surfs = d.u32(4) as usize;
    for k in 0..3 {
        runtime(w, &d, 44 + 4 * k, 1, smodels)?;
    }
    for k in 0..3 {
        runtime(w, &d, 56 + 4 * k, 1, static_surfs)?;
    }
    runtime(w, &d, 68, 128, 8 * d.u32(36) as usize)?;
    g.sorted_surf_index = array(w, &d, 72, 2, 2 * (static_surfs + d.u32(8) as usize))?;
    g.smodel_insts = array(w, &d, 76, 4, 0x1C * smodels)?;
    if w.inline_only(d.ptr(80), 4)? {
        let count = n(24);
        let a = w.load(0x30 * count)?;
        for i in 0..count {
            let e = a.elem(i, 0x30);
            let m = material(w, e.ptr(16), e.slot(16))?;
            g.surfaces.push((m, e.b));
        }
    }
    g.cull_groups = array(w, &d, 84, 4, 0x20 * n(224))?;
    if w.inline_only(d.ptr(88), 4)? {
        let a = w.load(0x4C * smodels)?;
        for i in 0..smodels {
            let e = a.elem(i, 0x4C);
            let m = xmodel(w, e.ptr(56), e.slot(56))?;
            g.static_models.push((m, e.b));
        }
    }
    runtime(w, &d, 92, 4, 8 * static_surfs)?;
    runtime(w, &d, 96, 128, 4 * d.u32(40) as usize)?;
    // GfxWorldDpvsDynamic at 684.
    let words = [n(684), n(688)];
    runtime(w, s, 700, 4, 4 * words[0] * cells)?;
    runtime(w, s, 704, 4, 4 * words[1] * cells)?;
    for (k, words) in words.iter().enumerate() {
        for j in 0..3 {
            runtime(w, s, 708 + 12 * k + 4 * j, 16, 32 * words)?;
        }
    }
    Ok(g)
}

/// `GfxCell`, 0x38.
#[derive(Clone, Debug, Default)]
pub struct GfxCell {
    pub mins: [f32; 3],
    pub maxs: [f32; 3],
    pub aabb_trees: Vec<GfxAabbTree>,
    pub portals: Vec<GfxPortal>,
    pub cull_groups: Vec<i32>,
    pub reflection_probes: Vec<u8>,
}

/// `GfxAabbTree`, 0x2C: a node of a cell's surface tree.
#[derive(Clone, Debug, Default)]
pub struct GfxAabbTree {
    pub mins: [f32; 3],
    pub maxs: [f32; 3],
    pub child_count: u16,
    pub surface_count: u16,
    pub start_surf_index: u16,
    pub surface_count_no_decal: u16,
    pub start_surf_index_no_decal: u16,
    /// Static model indices; a slot that is an offset into an earlier
    /// tree's list is copied out of it (empty when the walk kept no
    /// bulk data).
    pub smodel_indexes: Vec<u16>,
    /// Bytes from this tree to its first child, in the cell's tree array.
    pub children_offset: i32,
}

/// `GfxPortal`, 0x44: a plane facing out of the owning cell (`DpvsPlane`
/// at 0xC), the cell beyond, the hull's vertices.
#[derive(Clone, Debug, Default)]
pub struct GfxPortal {
    pub plane: [f32; 4],
    /// The cell the portal leads into, an index into [`GfxWorld::cells`];
    /// `None` for a cell inlined in the portal (no stock map has one).
    pub cell: Option<u32>,
    pub vertices: Vec<[f32; 3]>,
    pub hull_axis: [[f32; 3]; 2],
}

/// `GfxCell`, 0x38: its AABB trees, portals (each naming a cell of
/// `cells_at`'s array, or carrying one inline), cull groups and probe
/// indices.
fn gfx_cell<R: BufRead>(w: &mut W<R>, c: &S, cells_at: u32, cells: usize) -> Result<GfxCell> {
    let mut out = GfxCell {
        mins: c.vec3(0),
        maxs: c.vec3(12),
        ..GfxCell::default()
    };
    if w.inline_only(c.ptr(28), 4)? {
        let n = c.i32(24).max(0) as usize;
        let a = w.load(0x2C * n)?;
        for i in 0..n {
            let t = a.elem(i, 0x2C);
            let list = w.shared(t.ptr(36), 2, 2 * t.u16(34) as usize, "smodel indexes", true)?;
            out.aabb_trees.push(GfxAabbTree {
                mins: t.vec3(0),
                maxs: t.vec3(12),
                child_count: t.u16(24),
                surface_count: t.u16(26),
                start_surf_index: t.u16(28),
                surface_count_no_decal: t.u16(30),
                start_surf_index_no_decal: t.u16(32),
                smodel_indexes: list.map(|b| u16s(&b)).unwrap_or_default(),
                children_offset: t.i32(40),
            });
        }
    }
    if w.inline_only(c.ptr(36), 4)? {
        let n = c.i32(32).max(0) as usize;
        let a = w.load(0x44 * n)?;
        for i in 0..n {
            let p = a.elem(i, 0x44);
            let cell = match w.follow(p.ptr(32), 4)? {
                Follow::Inline => {
                    let cell = w.load(0x38)?;
                    gfx_cell(w, &cell, cells_at, cells)?;
                    None
                }
                Follow::At(at) => {
                    let o = at.wrapping_sub(cells_at);
                    let i = o / 0x38;
                    (o % 0x38 == 0 && (i as usize) < cells).then_some(i)
                }
                Follow::Null => None,
            };
            let mut vertices = Vec::new();
            if w.inline_only(p.ptr(36), 4)? {
                let v = w.bytes(4, 12 * p.u8(40) as usize)?;
                let v = S { at: 0, b: v };
                vertices = (0..p.u8(40) as usize).map(|k| v.vec3(12 * k)).collect();
            }
            out.portals.push(GfxPortal {
                plane: [p.f32(12), p.f32(16), p.f32(20), p.f32(24)],
                cell,
                vertices,
                hull_axis: [p.vec3(44), p.vec3(56)],
            });
        }
    }
    if w.inline_only(c.ptr(44), 4)? {
        let b = w.bytes(4, 4 * c.i32(40).max(0) as usize)?;
        out.cull_groups = b
            .as_chunks::<4>()
            .0
            .iter()
            .map(|c| i32::from_le_bytes(*c))
            .collect();
    }
    if w.inline_only(c.ptr(52), 1)? {
        out.reflection_probes = w.bytes(1, c.u8(48) as usize)?;
    }
    Ok(out)
}
