//! The map every consumer reads, filled by a format loader: the render
//! geometry, the cells and portals, the BSP tree and PVS, collision and the
//! entity text. Nothing past a loader takes a format's own parse.

use crate::asset_source::{Asset, AssetKind, AssetSource};
use crate::game::Game;
use anyhow::{Result, anyhow, bail, ensure};

/// A lightmap page's side in texels.
pub const LIGHTMAP_SIZE: usize = 512;
/// A soup's `lightmap` when it has none.
pub const NO_LIGHTMAP: u16 = 65535;

#[derive(Debug, Clone)]
pub struct Material {
    pub name: String,
    /// Unused; surfaces are skipped by material name.
    #[allow(dead_code)]
    pub surface_flags: u32,
    pub content_flags: u32,
}

#[derive(Debug)]
pub struct TriangleSoup {
    pub material: u16,
    pub lightmap: u16,
    pub first_vertex: u32,
    pub vertex_count: u16,
    pub index_count: u16,
    pub first_index: u32,
}

#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct DrawVert {
    pub pos: [f32; 3],
    pub uv: [f32; 2],
    pub lm_uv: [f32; 2],
    pub normal: [f32; 3],
    pub color: [u8; 4],
}

#[derive(Debug, Clone)]
pub struct Plane {
    pub normal: [f32; 3],
    pub dist: f32,
}

#[derive(Debug, Clone)]
pub struct BrushSide {
    /// Sides 0..6: f32 axial bound, bit-cast. Sides 6..: index into
    /// `MapWorld::planes`. Decoded in collision.rs.
    pub plane_or_dist: u32,
    /// Index into `MapWorld::materials`: the side's own material, whose surface
    /// flags a trace stopping on this side reports.
    pub material: u32,
}

#[derive(Debug, Clone)]
pub struct Brush {
    pub first_side: u32,
    pub num_sides: u16,
    pub material: u16,
}

/// Lump 27 entry. Model 0 is the world; 1.. are the `"model" "*N"`
/// brushmodels. Soup and brush ranges partition their lumps (doc, "Lump 27,
/// models").
#[derive(Debug, Clone)]
pub struct Model {
    /// Relative to the brushmodel's origin brush. Unused outside tests.
    pub mins: [f32; 3],
    pub maxs: [f32; 3],
    pub first_soup: u32,
    pub num_soups: u32,
    pub first_brush: u32,
    pub num_brushes: u32,
    /// The model's run in lump 23 (`leaf_partitions`): the terrain and
    /// patches it owns. A submodel's run is the list of its own leaf, which
    /// no node of model 0's tree reaches.
    pub partitions: (u32, u32),
}

impl Model {
    /// The clip model's bounds, which `trap_SetBrushModel` hands an entity's
    /// `r.mins`/`r.maxs`: the lump's grown a unit a side by the submodel
    /// loader (cod_lnxded 0x804a8e5..0x804a8fa, movers doc 18).
    pub fn clip_bounds(&self) -> ([f32; 3], [f32; 3]) {
        (self.mins.map(|v| v - 1.0), self.maxs.map(|v| v + 1.0))
    }
}

/// A CoD4 static model placement drawn at one of its LODs: its soups, one
/// run per LOD, follow each other inside its cull group, and the frame keeps
/// the run [`StaticLod::pick`] names (cod4-world.md 9.10).
#[derive(Debug, Clone, PartialEq)]
pub struct StaticLod {
    pub origin: [f32; 3],
    pub scale: f32,
    /// `GfxStaticModelDrawInst.cullDist`.
    pub cull_dist: f32,
    /// Per LOD, the model's `lodInfo[].dist` and its soup run `(first,
    /// count)`.
    pub lods: Vec<(f32, u32, u32)>,
}

impl StaticLod {
    /// The LOD 1.7's static model pass draws (iw3mp.exe 0x63adb2..0x63af2d):
    /// the distance from `eye` times the view's `scale` plus `bias` is
    /// culled at `cullDist` or past; under it, divided by the placement's
    /// scale, the first LOD whose distance is beyond it; the pass has no
    /// test for a distance of 0. `None` draws nothing.
    pub fn pick(&self, eye: [f32; 3], scale: f32, bias: f32) -> Option<usize> {
        let d: f32 = (0..3)
            .map(|k| (eye[k] - self.origin[k]).powi(2))
            .sum::<f32>()
            .sqrt()
            * scale
            + bias;
        if d >= self.cull_dist {
            return None;
        }
        let d = d * (1.0 / self.scale);
        self.lods.iter().position(|&(dist, _, _)| dist > d)
    }
}

/// CoD4's `R_UpdateLodParms` rigid ramp (iw3mp.exe 0x5fadf0): `(scale,
/// bias)` for a view whose vertical fov is `fov_y` degrees.
pub fn cod4_lod_ramp(fov_y: f32, r_lod_scale_rigid: f32, r_lod_bias_rigid: f32) -> (f32, f32) {
    let inv_fov = (fov_y.to_radians() * 0.5).tan() * 2.118_673;
    (r_lod_scale_rigid * inv_fov, r_lod_bias_rigid * inv_fov)
}

/// Layouts: docs/research/bsp-ibsp59-format.md, "Visibility and tree lumps".
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CullGroup {
    pub mins: [f32; 3],
    pub maxs: [f32; 3],
    pub first_soup: u32,
    pub soup_count: u32,
}

/// Children follow their parent in preorder; a node carries no bounds.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AabbNode {
    pub first_soup: u32,
    pub soup_count: u32,
    pub child_count: u32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Cell {
    pub mins: [f32; 3],
    pub maxs: [f32; 3],
    pub first_aabb: u32,
    pub first_portal: u32,
    pub portal_count: u32,
    pub first_cull_index: u32,
    pub cull_count: u32,
    /// Into lump 15; the cell's occluder index list.
    pub first_occluder: u32,
    pub occluder_count: u32,
}

/// One occluder polyhedron (lumps 12-15; docs/research/bsp-ibsp59-format.md,
/// "Lumps 12-15, occluders"). The `first_*` fields index the plane-index,
/// edge and vertex lumps 13/14/11.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Occluder {
    pub first_plane: u32,
    pub num_planes: u16,
    pub num_edges: u16,
    pub first_edge: u32,
    pub first_vert: u32,
    pub vert_count: u16,
}

/// `plane` faces out of the owning cell, into `cell`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Portal {
    pub plane: u32,
    pub cell: u32,
    pub first_vert: u32,
    pub vert_count: u32,
}

/// `children[0]` is the front side; a negative child is leaf `-(c + 1)`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Node {
    pub plane: u32,
    pub children: [i32; 2],
    pub mins: [i32; 3],
    pub maxs: [i32; 3],
}

/// The other six ints (leaf surface/brush/light ranges) are skipped.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Leaf {
    pub cluster: i32,
    /// -1 in solid leafs, where `cluster` is -1 too on every stock map.
    /// Areas join only through area portals, which no stock map opens, so
    /// two leafs connect when they share it ([`areas_connected`]).
    pub area: i32,
    /// -1 in solid leafs.
    pub cell: i32,
}

/// Lump 28: one bit row per cluster, saying which clusters that one can see.
/// A map can ship without it (mp_neuville does), and then nothing is hidden.
#[derive(Debug)]
pub struct Pvs {
    pub cluster_count: usize,
    pub row_bytes: usize,
    rows: Vec<u8>,
}

impl Pvs {
    /// `cluster_count` rows of `row_bytes` each, a set bit per cluster that
    /// row's cluster sees. `None` when `rows` is too short or a row cannot
    /// address every cluster.
    pub fn new(cluster_count: usize, row_bytes: usize, rows: Vec<u8>) -> Option<Pvs> {
        (rows.len() >= cluster_count * row_bytes && row_bytes * 8 >= cluster_count).then_some(Pvs {
            cluster_count,
            row_bytes,
            rows,
        })
    }

    /// The rows again, for a `Visibility` that outlives the parsed map.
    fn clone_rows(&self) -> Pvs {
        Pvs {
            cluster_count: self.cluster_count,
            row_bytes: self.row_bytes,
            rows: self.rows.clone(),
        }
    }

    /// Whether a viewer in `from` can be shown something in `to`. A cluster
    /// outside the table -- which is what a solid leaf's -1 is -- is visible,
    /// matching the engine: its own test only ever runs on real clusters.
    pub fn visible(&self, from: i32, to: i32) -> bool {
        let (Ok(f), Ok(t)) = (usize::try_from(from), usize::try_from(to)) else {
            return true;
        };
        if f >= self.cluster_count || t >= self.cluster_count {
            return true;
        }
        let byte = self.rows[f * self.row_bytes + (t >> 3)];
        byte & (1 << (t & 7)) != 0
    }
}

/// One lump-24 terrain partition: a triangle list the engine hands
/// `CM_GenerateTerrainCollide`, indices relative to `first_vert`
/// (docs/research/bsp-ibsp59-format.md, "Lump 24, collision partitions").
#[derive(Debug, Clone)]
pub struct TerrainPart {
    pub material: u16,
    pub first_vert: u32,
    pub vert_count: u16,
    pub first_index: u32,
    pub index_count: u16,
}

/// One lump-24 patch partition: a `width x height` grid of bezier control
/// points at `first_vert` of lump 25 and the subdivision tolerance, the
/// engine's `CM_GeneratePatchCollide` input (`patch.rs`).
#[derive(Debug, Clone)]
pub struct PatchPart {
    pub material: u16,
    pub width: u16,
    pub height: u16,
    /// How far a curve may stray from its chord before the grid splits it.
    pub tolerance: i32,
    pub first_vert: u32,
}

/// A lump-23 entry: the lump-24 record a leaf lists, as an index into
/// [`MapWorld::terrain`] or [`MapWorld::patches`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Partition {
    Terrain(u32),
    Patch(u32),
}

/// A leaf's runs, (first, count), in [`MapWorld::leaf_brushes`] and
/// [`MapWorld::leaf_partitions`] (leaf record bytes 16 and 20, 8 and 12).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LeafLists {
    pub brushes: (u32, u32),
    pub partitions: (u32, u32),
}

/// A CoD 2 lightmap page (cod2-d3dbsp-format.md, lump 1): three 512 x 512
/// coefficient images, four basis weights per texel for red, green and
/// blue, in the file's byte order, and the 1024 x 1024 sun visibility.
#[derive(Debug, Clone, Default)]
pub struct LightmapBasis {
    pub coeffs: [Vec<u8>; 3],
    pub sun: Vec<u8>,
}

/// How a CoD4 material casts into the sun's shadow map (its
/// `build_shadowmap_color` technique, docs/research/cod4-world.md 3.6):
/// both faces, alpha-tested at 128 off `image` when `alpha_test`.
#[derive(Clone, Debug, PartialEq)]
pub struct SunCaster {
    pub alpha_test: bool,
    /// The colour map, the image the test reads.
    pub image: String,
}

/// A sun a format stores itself (CoD4's comworld primary light), which the
/// CoD2 lit path takes over worldspawn's keys.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MapSun {
    /// Unit vector toward the sun.
    pub dir: [f32; 3],
    /// Its diffuse colour in display units.
    pub colour: [f32; 3],
}

/// CoD4's model lighting volume (`cod4-world.md` 3.2): a block of 4 x 4 x
/// 4 RGBA8 texels a static model, laid out `width` x `height` a slice,
/// the four z slices one under the other.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ModelLightVolume {
    pub width: u32,
    /// One slice's height; the image is four of them.
    pub height: u32,
    /// R, G, B, the primary light's weight; slice 0's rows first.
    pub rgba: Vec<u8>,
}

/// What CoD4's `R_AllocModelLighting` picks for a dynamic model
/// (`cod4-world.md` 3.3): its 4 x 4 x 4 block of the model lighting
/// volume, R, G, B and the primary light's weight by `x + 4 y + 16 z`,
/// its reflection probe and the primary light the lookup settled on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EntityLight {
    pub texels: [[u8; 4]; 64],
    /// Into [`MapWorld::reflection_probes`].
    pub probe: u8,
    /// Into [`MapWorld::model_lights`]: the sun, a spot or omni light, or
    /// 0 for none.
    pub primary: u8,
}

/// CoD4's entity lighting: an [`EntityLight`] for a lighting origin and
/// the world box (mins, maxs) the model spans, as `R_AllocModelLighting`
/// fills one each frame.
#[derive(Clone)]
pub struct EntityLighter(pub std::sync::Arc<EntityLightFn>);

/// [`EntityLighter`]'s lookup.
pub type EntityLightFn =
    dyn Fn([f32; 3], [[f32; 3]; 2]) -> EntityLight + Send + Sync + std::panic::RefUnwindSafe;

/// A CoD4 spot or omni primary light as `R_SetLightProperties` hands it
/// to a model's `lp_spot_*` or `lp_omni_*` technique (`cod4-world.md`
/// 3.5).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ModelLightSource {
    pub spot: bool,
    /// The light may take a shadow map (`canUseShadowMap`).
    pub can_shadow: bool,
    pub origin: [f32; 3],
    /// `lightPosition.w`, one over the radius.
    pub inv_radius: f32,
    /// `lightDiffuse` and `lightSpecular` (both colour scales 1).
    pub colour: [f32; 3],
    /// `lightSpotDir`: back at the light.
    pub dir: [f32; 3],
    /// `lightSpotFactors`: the cone's scale, bias and exponent.
    pub spot_factors: [f32; 3],
    /// The light def's attenuation image, its first row (32 texels on
    /// every stock def), RGBA.
    pub attenuation: Vec<[u8; 4]>,
}

impl std::fmt::Debug for EntityLighter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("EntityLighter")
    }
}

/// A CoD4 water material's stored spectrum (`water_t`'s `H0` and `wTerm`)
/// on its `width` square, row-major.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct WaterSpectrum {
    pub width: usize,
    pub h0: Vec<[f32; 2]>,
    pub omega: Vec<f32>,
}

/// A CoD4 reflection probe's cube: `faces[face][level]` RGBA, largest
/// level first, faces in D3D's order.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ProbeCube {
    pub size: u32,
    pub faces: Vec<Vec<Vec<u8>>>,
}

/// A CoD 2 light grid corner (cod2-d3dbsp-format.md, lump 2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LightGridEntry {
    /// `(x >> 2) << 21 | (y >> 2) << 10 | (z >> 2)`
    pub key: u32,
    /// `(x & 3) << 6 | (y & 3) << 4 | (z & 3) << 2 | sun bit`
    pub low: u8,
    /// Bit `c`: a query's cell corner `c` is used only past a sight trace.
    pub occlusion_mask: u8,
    /// Into [`LightGrid::colours`].
    pub colour: u16,
}

/// A CoD 2 light grid colour (lump 3): eight values per channel,
/// channel-major, one per corner of the probe basis.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LightGridColour {
    pub rgb: [[u8; 8]; 3],
}

/// A CoD 2 map's light grid (lumps 2 and 3) as stored; sampled by
/// `vcod_cod2::lightgrid`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LightGrid {
    /// Sorted by `(key, low & 0xfe)`.
    pub entries: Vec<LightGridEntry>,
    pub colours: Vec<LightGridColour>,
}

/// One loaded map, whatever file it came from: what the collision model,
/// the server's link and snapshot cull, the cell and portal vis, the static
/// lighting and the renderer read. A format loader fills it; IBSP 59 (CoD 1)
/// is [`crate::bsp::parse`], and [`load`] picks the loader by the header.
///
/// The record types keep IBSP 59's shapes where a later format shares them
/// (CoD 2's IBSP 4 renumbers the same records, `cod2-d3dbsp-format.md`). A
/// part only one format carries is an empty vector or `None` elsewhere, and
/// its doc names the format.
#[derive(Debug, Default)]
pub struct MapWorld {
    // Materials and render geometry.
    pub materials: Vec<Material>,
    /// 512 x 512 RGB pages ([`LIGHTMAP_SIZE`]), what a soup's `lightmap`
    /// indexes.
    pub lightmaps: Vec<Vec<u8>>,
    /// IBSP 4 (CoD 2) only, index-aligned with `lightmaps`: each page as
    /// the shader path samples it. Empty on IBSP 59.
    pub lightmap_basis: Vec<LightmapBasis>,
    /// IBSP 4 (CoD 2) only: the light grid models are lit from.
    pub light_grid: Option<LightGrid>,
    /// CoD4 only: the sun the lit path draws with; `None` reads worldspawn.
    pub sun: Option<MapSun>,
    /// CoD4 only: the static models' lighting volume, which their
    /// vertices' `lm_uv` index.
    pub model_lighting: Option<ModelLightVolume>,
    /// CoD4 only: the light grid lookup entities and the view model are
    /// lit from.
    pub entity_lighting: Option<EntityLighter>,
    /// CoD4 only: each primary light's shader constants, `None` for the
    /// sun and the empty light 0.
    pub model_lights: Vec<Option<ModelLightSource>>,
    /// CoD4 only, index-aligned with `soups`: each soup's primary light
    /// (its surface's or static model's `primaryLightIndex`, into
    /// `model_lights`; 0 none). Empty elsewhere.
    pub soup_lights: Vec<u8>,
    /// CoD4 only, index-aligned with `materials`: how each casts into the
    /// sun's shadow map; `None` casts nothing. Empty elsewhere.
    pub sun_casters: Vec<Option<SunCaster>>,
    /// CoD4 only: the reflection probes' cubes.
    pub reflection_probes: Vec<ProbeCube>,
    /// CoD4 only: each water material's spectrum, by material name.
    pub waters: Vec<(String, WaterSpectrum)>,
    /// CoD4 only, index-aligned with `soups`: each soup's reflection probe
    /// into `reflection_probes`. Empty elsewhere (every soup probe 0).
    pub soup_probes: Vec<u8>,
    /// CoD4 only, index-aligned with `soups`: each soup's place in 1.7's
    /// lit draw order, a key that sorts as retail draws (`vcod_cod4::
    /// drawsurf::soup_key`). Empty elsewhere.
    pub soup_order: Vec<u64>,
    pub soups: Vec<TriangleSoup>,
    pub verts: Vec<DrawVert>,
    /// Per vertex, index-aligned with `verts` when the format stores a
    /// tangent frame: (tangent, binormal). Empty on IBSP 59, which has none.
    pub tangents: Vec<[[f32; 3]; 2]>,
    /// Relative to each soup's `first_vertex`.
    pub indices: Vec<u16>,
    /// The entity text, `{ "key" "value" }` blocks.
    pub entities: String,

    // Cells, portals and cull groups: the renderer's vis (`vis.rs`).
    pub cull_groups: Vec<CullGroup>,
    pub cull_indices: Vec<u32>,
    pub portal_verts: Vec<[f32; 3]>,
    pub occluders: Vec<Occluder>,
    pub occluder_plane_indices: Vec<u32>,
    pub occluder_edges: Vec<[u8; 4]>,
    pub occluder_indices: Vec<u16>,
    pub aabb_nodes: Vec<AabbNode>,
    pub cells: Vec<Cell>,
    pub portals: Vec<Portal>,
    /// CoD2 and CoD4: the cell walk follows the portals alone, as 1.7's
    /// `R_AddWorldSurfacesPortalWalk` (iw3mp.exe 0x60afd0) and CoD2's
    /// `R_VisitPortals` (Mac 0xf14c0) do, through the nearest-first
    /// portal queue (`WorldVis::walk_queued`); none of `vis.rs`'s
    /// over-marking rules for CoD1's mp_ship decks run.
    pub portals_only: bool,
    /// CoD2's `r_portalMinClipArea` (0.02): a queued portal whose hull
    /// covers less of the screen visits its cell without queueing that
    /// cell's portals and adds the cells beyond under the same frustum
    /// (`WorldVis::further_cells`). CoD4 1.7 has the same rule at the
    /// same default (iw3mp.exe 0x605fd0). 0 (CoD1) always queues.
    pub portal_min_clip_area: f32,
    /// CoD4's `r_portalMinRecurseDepth` (2, iw3mp.exe 0x62b80a): a portal
    /// queued from a visit shallower than this queues its cell's portals
    /// whatever its area. 0 (CoD1, CoD2) leaves the area alone to decide.
    pub portal_min_recurse_depth: u8,
    /// CoD4 only: each static model placement's LOD runs.
    pub static_lods: Vec<StaticLod>,

    // The BSP tree and the PVS.
    pub planes: Vec<Plane>,
    pub nodes: Vec<Node>,
    pub leafs: Vec<Leaf>,
    /// `None` when the map ships no PVS.
    pub pvs: Option<Pvs>,

    // Collision: brushes, inline models and what each leaf lists.
    pub brush_sides: Vec<BrushSide>,
    pub brushes: Vec<Brush>,
    pub models: Vec<Model>,
    /// IBSP 59 lumps 24-26: the engine's own terrain and patch collision
    /// input, built into facets by the collision model at load.
    pub terrain: Vec<TerrainPart>,
    pub patches: Vec<PatchPart>,
    pub collision_verts: Vec<[f32; 3]>,
    pub collision_indices: Vec<u16>,
    /// Per leaf, index-aligned with `leafs`: its runs in `leaf_brushes`
    /// and `leaf_partitions`. Empty when the format has no such lists; the
    /// collision model then has no leaf walk (`leafwalk.rs`).
    pub leaf_lists: Vec<LeafLists>,
    /// The brushes each leaf lists, as `brushes` indices.
    pub leaf_brushes: Vec<u32>,
    /// IBSP 59 lump 23: the partitions each leaf lists. A trace clips only
    /// what the leaves it walks list (bsp-ibsp59-format.md, "The leaf
    /// walk").
    pub leaf_partitions: Vec<Partition>,

    // IBSP 59's static lights (`static_light.rs`, `sun.rs`).
    /// Lump 19: indices into `lights`, per leaf through `leaf_lights`. A
    /// leaf's list may open with a negative entry, the sky flag.
    pub light_indices: Vec<i16>,
    /// Per leaf, index-aligned with `leafs`: (first, count) into
    /// `light_indices` (leaf record bytes 28 and 32).
    pub leaf_lights: Vec<(u32, u32)>,
    /// Lump 30, 72-byte records as raw words; decoded per light type in
    /// `static_light.rs`.
    pub lights: Vec<[u32; 18]>,
    /// Lump 32, the precomputed light visibility cache, raw.
    pub light_vis: Vec<u8>,
}

impl MapWorld {
    /// The tree and table a server needs to decide who can see what, without
    /// keeping the whole map: the lightmaps alone are megabytes.
    pub fn visibility(&self) -> Visibility {
        Visibility {
            planes: self.planes.clone(),
            nodes: self.nodes.clone(),
            leafs: self.leafs.clone(),
            pvs: self.pvs.as_ref().map(Pvs::clone_rows),
            models: self.models.iter().map(Model::clip_bounds).collect(),
        }
    }
}

/// A map file's world, by its header: `IBSP` version 59 is CoD 1's. Other
/// versions are refused here; a game crate with its own loader (CoD 2's
/// IBSP 4) fills a [`MapWorld`] itself.
pub fn load(data: &[u8]) -> Result<MapWorld> {
    ensure!(data.len() >= 8, "file too small to be a map");
    ensure!(&data[..4] == b"IBSP", "not an IBSP file");
    let version = u32::from_le_bytes(data[4..8].try_into().unwrap());
    match version {
        59 => crate::bsp::parse(data),
        v => Err(anyhow!(
            "IBSP version {v} has no loader here (59 is CoD 1's)"
        )),
    }
}

/// A CoD1 map by its name (`mp_carentan`), `maps/<name>.bsp` before
/// `maps/mp/<name>.bsp` as `Pk3Fs::resolve_map` looks, read through
/// [`AssetSource::read`] (so a `Pk3Fs` marks the map's pak) and loaded by
/// [`load`]. Built through [`Asset::find`], uncached: a level owns its map
/// and the source's cache would keep the last one resident beside it.
/// CoD2's `.d3dbsp` is `vcod_cod2::map::MapFile`'s kind.
impl Asset for MapWorld {
    const KIND: AssetKind = AssetKind::Map;

    fn find(src: &dyn AssetSource, name: &str) -> Result<Self> {
        if src.game() != Game::Cod1 {
            bail!("map {name}: {:?} maps are not IBSP 59", src.game());
        }
        let name = name.to_lowercase();
        let Some(path) = [format!("maps/{name}.bsp"), format!("maps/mp/{name}.bsp")]
            .into_iter()
            .find(|p| src.exists(p))
        else {
            bail!("map {name} not found in the mounted paks");
        };
        let data = src.read(&path).ok_or_else(|| anyhow!("reading {path}"))?;
        load(&data)
    }
}

/// [`MapWorld::find`]: the CoD1 map `name` off `src`.
pub fn find(src: &dyn AssetSource, name: &str) -> Result<MapWorld> {
    MapWorld::find(src, name)
}

/// Point-to-cluster lookup plus the PVS row test: what a server culls a
/// client's entity list with (`docs/protocol-1.1.md`, "Which entities a client
/// is sent").
#[derive(Debug)]
pub struct Visibility {
    planes: Vec<Plane>,
    nodes: Vec<Node>,
    leafs: Vec<Leaf>,
    pvs: Option<Pvs>,
    /// Each inline model's clip bounds ([`Model::clip_bounds`]), what a
    /// brush model entity links with.
    models: Vec<([f32; 3], [f32; 3])>,
}

impl Visibility {
    /// An empty tree: every point reads as a solid leaf and nothing is
    /// hidden. For a server with no map loaded, and for tests that only need
    /// collision.
    pub fn none() -> Self {
        Visibility {
            planes: Vec::new(),
            nodes: Vec::new(),
            leafs: Vec::new(),
            pvs: None,
            models: Vec::new(),
        }
    }

    /// Test helper: one open cluster holding every point, so every linked
    /// entity is sent to everyone.
    #[doc(hidden)]
    pub fn single_cluster() -> Self {
        Visibility {
            planes: vec![Plane {
                normal: [0.0, 0.0, 1.0],
                dist: 0.0,
            }],
            nodes: vec![Node {
                plane: 0,
                children: [-1, -1],
                mins: [0; 3],
                maxs: [0; 3],
            }],
            leafs: vec![Leaf {
                cluster: 0,
                area: 0,
                cell: 0,
            }],
            pvs: None,
            models: Vec::new(),
        }
    }

    /// Inline model `n`'s bounds, `None` past the model count.
    pub fn model_bounds(&self, n: usize) -> Option<([f32; 3], [f32; 3])> {
        self.models.get(n).copied()
    }

    /// The leaf a point falls in, by walking the tree from node 0. The engine
    /// descends the same way for its camera cell (`0x4e2be0`): a point on the
    /// plane takes the back child.
    pub fn leaf_at(&self, p: [f32; 3]) -> Option<&Leaf> {
        if self.nodes.is_empty() {
            return self.leafs.first();
        }
        let mut i: i32 = 0;
        // A malformed tree must not spin; no stock map comes close to this.
        for _ in 0..self.nodes.len() + 1 {
            if i < 0 {
                return self.leafs.get((-(i + 1)) as usize);
            }
            let n = self.nodes.get(i as usize)?;
            let pl = self.planes.get(n.plane as usize)?;
            let d = p[0] * pl.normal[0] + p[1] * pl.normal[1] + p[2] * pl.normal[2] - pl.dist;
            i = if d > 0.0 {
                n.children[0]
            } else {
                n.children[1]
            };
        }
        None
    }

    /// The cluster a point falls in, or -1 for a solid leaf or no tree.
    pub fn cluster_at(&self, p: [f32; 3]) -> i32 {
        self.leaf_at(p).map_or(-1, |l| l.cluster)
    }

    /// The area a point falls in, -1 for a solid leaf. A missing tree reads
    /// as area 0, so a server with no map culls nothing.
    pub fn area_at(&self, p: [f32; 3]) -> i32 {
        self.leaf_at(p).map_or(0, |l| l.area)
    }

    /// Whether a viewer in `from` is shown something in `to`. A map with no
    /// lump 28 hides nothing.
    pub fn visible(&self, from: i32, to: i32) -> bool {
        self.pvs.as_ref().is_none_or(|p| p.visible(from, to))
    }

    /// The clusters `SV_LinkEntity` keeps for a box, which is what the
    /// engine culls an entity by. A point lookup is not enough: a turret's or
    /// a prop's origin often sits inside the geometry it stands on and reads
    /// as a solid leaf. The engine keeps a capped list rather than every
    /// cluster the box touches (docs/protocol-1.1.md, "Which entities a
    /// client is sent"): `CM_BoxLeafnums` lists at most 128 leafs, front
    /// child first, solid ones included, and the link takes the first 16
    /// clusters among them; past 16 it keeps the cluster of the last
    /// non-solid leaf the walk reached, listed or not, as `last_cluster`.
    pub fn link_clusters(&self, mins: [f32; 3], maxs: [f32; 3]) -> LinkClusters {
        let mut walk = LeafWalk {
            leafs: Vec::new(),
            last_leaf: 0,
        };
        self.box_leafs(0, mins, maxs, &mut walk, 0);
        let mut out = LinkClusters {
            areas: [-1; 2],
            ..LinkClusters::default()
        };
        // A box over a third area keeps the last as its second (`Object %i
        // touching 3 areas`, coduo_lnxded 0x8099dee).
        for leaf in walk.leafs.iter().filter_map(|&l| self.leafs.get(l)) {
            match leaf.area {
                -1 => {}
                a if out.areas[0] == -1 || out.areas[0] == a => out.areas[0] = a,
                a => out.areas[1] = a,
            }
        }
        let cluster = |i: usize| self.leafs.get(i).map_or(-1, |l| l.cluster);
        for &leaf in &walk.leafs {
            let c = cluster(leaf);
            if c == -1 {
                continue;
            }
            out.clusters.push(c);
            if out.clusters.len() == MAX_ENT_CLUSTERS {
                out.last_cluster = cluster(walk.last_leaf);
                break;
            }
        }
        out.linked = !walk.leafs.is_empty();
        out
    }

    /// `SV_AddEntitiesVisibleFromPoint`'s cluster test of a linked entity
    /// from a viewer in `from`. Past 16 clusters it scans the cluster numbers
    /// from the 16th up to `last_cluster` and, as Q3's does, reads the scan
    /// ending on `last_cluster` itself as hidden and running past it as seen.
    pub fn link_visible(&self, from: i32, link: &LinkClusters) -> bool {
        if link.clusters.iter().any(|&c| self.visible(from, c)) {
            return true;
        }
        let (Some(&last), true) = (link.clusters.last(), link.last_cluster != 0) else {
            return false;
        };
        let mut l = last;
        while l <= link.last_cluster && !self.visible(from, l) {
            l += 1;
        }
        l != link.last_cluster
    }

    /// `CM_BoxLeafnums_r`: a box straddling a plane walks the front child
    /// first. Every leaf counts against the 128, and a non-solid one is the
    /// last leaf even when the list is already full.
    fn box_leafs(&self, i: i32, mins: [f32; 3], maxs: [f32; 3], walk: &mut LeafWalk, depth: u32) {
        // The tree is a few dozen deep on stock maps; the cap is for a
        // malformed one, not for them.
        if depth > 128 {
            return;
        }
        if i < 0 {
            let leaf = (-(i + 1)) as usize;
            if self.leafs.get(leaf).is_some_and(|l| l.cluster != -1) {
                walk.last_leaf = leaf;
            }
            if walk.leafs.len() < MAX_BOX_LEAFS {
                walk.leafs.push(leaf);
            }
            return;
        }
        let Some(n) = self.nodes.get(i as usize) else {
            return;
        };
        let Some(pl) = self.planes.get(n.plane as usize) else {
            return;
        };
        let (front, back) = box_on_plane_side(pl, mins, maxs);
        if front {
            self.box_leafs(n.children[0], mins, maxs, walk, depth + 1);
        }
        if back {
            self.box_leafs(n.children[1], mins, maxs, walk, depth + 1);
        }
    }
}

/// `MAX_ENT_CLUSTERS`: the clusters a linked entity keeps.
const MAX_ENT_CLUSTERS: usize = 16;
/// The leaf list `SV_LinkEntity` hands `CM_BoxLeafnums`.
const MAX_BOX_LEAFS: usize = 128;

/// An entity's link as the snapshot cull reads it ([`Visibility::link_clusters`]).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LinkClusters {
    /// At most 16, in the leaf walk's order, repeats kept.
    pub clusters: Vec<i32>,
    /// The cluster of the walk's last non-solid leaf when `clusters` filled
    /// up, else 0 (the engine's unset value).
    pub last_cluster: i32,
    /// The box reached a leaf at all; the engine unlinks an entity whose box
    /// lists none.
    pub linked: bool,
    /// `areanum` and `areanum2`: the first two areas the listed leafs carry,
    /// -1 for none. The snapshot cull sends an entity only when one of them
    /// connects to the viewer's ([`areas_connected`]).
    pub areas: [i32; 2],
}

/// `CM_AreasConnected` (cod_lnxded 0x8053f44, coduo_lnxded 0x8057abd): no
/// area -1, and both in one flood. Areas flood only through a portal
/// `trap_AdjustAreaPortalState` opened, which only a binary mover calls
/// (`Reached_BinaryMover`, game.mp.uo 0x721ae), and no stock MP map has one,
/// so a flood is one area.
pub fn areas_connected(a: i32, b: i32) -> bool {
    a >= 0 && b >= 0 && a == b
}

impl LinkClusters {
    /// Whether one of the entity's areas connects to `area`.
    pub fn in_area(&self, area: i32) -> bool {
        self.areas.iter().any(|&a| areas_connected(area, a))
    }
}

struct LeafWalk {
    leafs: Vec<usize>,
    last_leaf: usize,
}

/// `BoxOnPlaneSide`, as (front, back): the box's farthest corner along the
/// normal at or past the plane is in front, its nearest corner short of it
/// behind. An axial plane takes the same test: the engines' routine is Q3's
/// x87 assembly, which has no axial case, and no caller checks the plane's
/// type first, so a box whose high side lies on an axial plane is on both
/// sides (`cod_lnxded` 0x80c8634, `coduo_lnxded` 0x80cb294;
/// protocol-1.1.md, "The clusters a link keeps"). The corners follow the
/// plane's sign bits, set for a component below 0 (0x8067204). The sums run
/// on the x87 stack, so they are taken in f64 here.
fn box_on_plane_side(pl: &Plane, mins: [f32; 3], maxs: [f32; 3]) -> (bool, bool) {
    let (mut near, mut far) = (0.0f64, 0.0f64);
    for axis in 0..3 {
        let c = f64::from(pl.normal[axis]);
        let (lo, hi) = (f64::from(mins[axis]), f64::from(maxs[axis]));
        if pl.normal[axis] < 0.0 {
            near += c * hi;
            far += c * lo;
        } else {
            near += c * lo;
            far += c * hi;
        }
    }
    let dist = f64::from(pl.dist);
    (far >= dist, near < dist)
}

#[cfg(test)]
mod pvs_tests {
    use super::*;
    use crate::bsp::{parse, parse_pvs};

    /// The engines' `BoxOnPlaneSide` has no axial case (`cod_lnxded`
    /// 0x80c8634): a box whose high side lies on an axial plane is on both
    /// sides, one whose low side lies on it in front only.
    #[test]
    fn a_box_touching_an_axial_plane_from_behind_is_on_both_sides() {
        let x = Plane {
            normal: [1.0, 0.0, 0.0],
            dist: 100.0,
        };
        let side = |lo: f32, hi: f32| box_on_plane_side(&x, [lo, -1.0, -1.0], [hi, 1.0, 1.0]);
        assert_eq!(side(90.0, 100.0), (true, true));
        assert_eq!(side(100.0, 110.0), (true, false));
        assert_eq!(side(80.0, 90.0), (false, true));
        assert_eq!(side(100.0, 100.0), (true, false));
        let back = Plane {
            normal: [-1.0, 0.0, 0.0],
            dist: -100.0,
        };
        let side = |lo: f32, hi: f32| box_on_plane_side(&back, [lo, -1.0, -1.0], [hi, 1.0, 1.0]);
        assert_eq!(side(100.0, 110.0), (true, true));
        assert_eq!(side(90.0, 100.0), (true, false));
    }

    fn lump(cluster_count: i32, row_bytes: i32, rows: &[u8]) -> Vec<u8> {
        let mut b = cluster_count.to_le_bytes().to_vec();
        b.extend(row_bytes.to_le_bytes());
        b.extend(rows);
        b
    }

    #[test]
    fn a_row_is_read_bit_by_bit() {
        // Cluster 0 sees 0 and 9; cluster 1 sees only itself.
        let p = parse_pvs(&lump(2, 2, &[0b0000_0001, 0b0000_0010, 0b0000_0010, 0]))
            .expect("parse")
            .expect("a table");
        assert!(p.visible(0, 0));
        assert!(!p.visible(0, 1));
        assert!(p.visible(0, 9));
        assert!(p.visible(1, 1));
        assert!(!p.visible(1, 0));
    }

    /// A solid leaf's -1 and anything past the table are visible: the engine's
    /// own test only ever runs on real clusters, and `clusters_in_box` is what
    /// keeps a solid origin from meaning "everywhere".
    #[test]
    fn a_cluster_outside_the_table_is_visible() {
        let p = parse_pvs(&lump(1, 1, &[0]))
            .expect("parse")
            .expect("a table");
        assert!(p.visible(-1, 0));
        assert!(p.visible(0, -1));
        assert!(p.visible(0, 7));
    }

    /// A box over 21 leafs in a row, leaf `i` in cluster `i + 1`, seen
    /// from cluster 22, which sees only `sees`.
    fn leaf_row(sees: &[usize]) -> (Visibility, LinkClusters) {
        let n = 20usize;
        let planes = (0..n)
            .map(|i| Plane {
                normal: [1.0, 0.0, 0.0],
                dist: 10.0 * (i + 1) as f32,
            })
            .collect();
        // Node i: its plane's front is leaf i, its back node i + 1; the last
        // node's back is leaf 20. A box over every plane walks the leafs in
        // order.
        let nodes = (0..n)
            .map(|i| Node {
                plane: i as u32,
                children: [
                    -(i as i32) - 1,
                    if i + 1 < n {
                        i as i32 + 1
                    } else {
                        -(n as i32) - 1
                    },
                ],
                mins: [0; 3],
                maxs: [0; 3],
            })
            .collect();
        let leafs = (0..=n)
            .map(|i| Leaf {
                cluster: i as i32 + 1,
                area: 0,
                cell: 0,
            })
            .collect();
        let (count, row_bytes) = (23, 3);
        let mut rows = vec![0u8; count * row_bytes];
        for &c in sees {
            rows[22 * row_bytes + c / 8] |= 1 << (c % 8);
        }
        let vis = Visibility {
            planes,
            nodes,
            leafs,
            pvs: Some(Pvs {
                cluster_count: count,
                row_bytes,
                rows,
            }),
            models: Vec::new(),
        };
        let link = vis.link_clusters([0.0, -1.0, -1.0], [250.0, 1.0, 1.0]);
        (vis, link)
    }

    /// `SV_LinkEntity` keeps the first 16 clusters and the last leaf's, and
    /// the snapshot test scans the cluster numbers between them, reading a
    /// scan that stops on the last cluster as hidden and one that runs past
    /// it as seen (Q3's `SV_AddEntitiesVisibleFromPoint`, coduo_lnxded
    /// 0x8096b97).
    #[test]
    fn a_link_past_sixteen_clusters_keeps_the_last_one() {
        let (vis, link) = leaf_row(&[]);
        assert_eq!(link.clusters, (1..=16).collect::<Vec<_>>());
        assert_eq!(link.last_cluster, 21);
        for (sees, seen) in [
            (vec![3], true),
            (vec![18], true),
            (vec![21], false),
            (vec![16], true),
            (vec![], true),
        ] {
            let (vis, link) = leaf_row(&sees);
            assert_eq!(vis.link_visible(22, &link), seen, "seeing {sees:?}");
        }
        // Under 16 clusters nothing is scanned. Past x 20 the box is behind
        // every plane, which the row files under the last leaf.
        let few = vis.link_clusters([0.0, -1.0, -1.0], [25.0, 1.0, 1.0]);
        assert_eq!((few.clusters, few.last_cluster), (vec![1, 2, 21], 0));
    }

    #[test]
    fn a_map_asset_reads_what_resolve_map_names_and_marks_its_pak() {
        let Some(fs) = crate::testing::stock_fs() else {
            return;
        };
        let path = fs.resolve_map("MP_Carentan").unwrap();
        let direct = load(&fs.read(&path).unwrap()).unwrap();
        fs.clear_references();
        let found = find(&fs, "MP_Carentan").unwrap();
        assert_eq!(found.entities, direct.entities);
        let paks: Vec<_> = fs
            .referenced_paks()
            .iter()
            .map(|p| p.name.clone())
            .collect();
        assert_eq!(paks.len(), 1, "{paks:?}");
        assert!(find(&fs, "mp_nowhere").is_err());
    }

    #[test]
    fn an_empty_lump_is_a_map_without_a_pvs() {
        assert!(parse_pvs(&[]).expect("parse").is_none());
    }

    #[test]
    fn a_short_lump_is_an_error_rather_than_a_panic() {
        assert!(parse_pvs(&lump(4, 8, &[0; 8])).is_err());
        assert!(parse_pvs(&[0, 0, 0]).is_err());
    }

    /// Against a real map: every leaf's cluster has a row, which is what
    /// `visible` indexes with. The format doc records 666 clusters on
    /// mp_pavlov.
    #[test]
    fn every_leaf_cluster_of_a_stock_map_has_a_row() {
        let Some(fs) = crate::testing::game_fs() else {
            return;
        };
        let Some(bytes) = fs.read("maps/mp/mp_pavlov.bsp") else {
            return;
        };
        let b = parse(&bytes).expect("parse the bsp");
        let pvs = b.pvs.as_ref().expect("mp_pavlov ships a pvs");
        let highest = b.leafs.iter().map(|l| l.cluster).max().unwrap_or(-1);
        assert!(
            highest < pvs.cluster_count as i32,
            "leaf cluster {highest} is past the {} rows",
            pvs.cluster_count
        );
        assert!(pvs.row_bytes * 8 >= pvs.cluster_count);
    }

    /// `SV_LinkEntity` keeps the first area its leafs carry and the last
    /// other one, a solid leaf's -1 counting for neither (coduo_lnxded
    /// 0x8099dee), and the cull needs one of them to be the eye's.
    #[test]
    fn a_link_keeps_its_first_area_and_its_last_other() {
        let plane = |dist| Plane {
            normal: [1.0, 0.0, 0.0],
            dist,
        };
        let node = |plane, children| Node {
            plane,
            children,
            mins: [0; 3],
            maxs: [0; 3],
        };
        // Front child first: the walk meets leafs 3, 2, 1, 0.
        let leaf = |cluster, area| Leaf {
            cluster,
            area,
            cell: 0,
        };
        let vis = Visibility {
            planes: vec![plane(0.0), plane(10.0), plane(20.0)],
            nodes: vec![node(0, [1, -1]), node(1, [2, -2]), node(2, [-4, -3])],
            leafs: vec![leaf(0, 2), leaf(1, 0), leaf(2, 1), leaf(-1, -1)],
            pvs: None,
            models: Vec::new(),
        };
        let link = vis.link_clusters([-5.0, -1.0, -1.0], [25.0, 1.0, 1.0]);
        assert_eq!(link.areas, [1, 2]);
        assert!(link.in_area(1) && link.in_area(2));
        assert!(!link.in_area(0) && !link.in_area(-1));
        assert_eq!(vis.area_at([-5.0, 0.0, 0.0]), 2);
        assert_eq!(vis.area_at([25.0, 0.0, 0.0]), -1);
        let solid = vis.link_clusters([21.0, -1.0, -1.0], [25.0, 1.0, 1.0]);
        assert_eq!((solid.areas, solid.clusters.len()), ([-1, -1], 0));
    }

    #[test]
    fn a_static_model_picks_its_lod_and_culls_at_its_cull_dist() {
        let sl = StaticLod {
            origin: [0.0; 3],
            scale: 2.0,
            cull_dist: 2000.0,
            lods: vec![(200.0, 0, 3), (500.0, 3, 2)],
        };
        // the lod distance is the view distance over the placement's scale
        assert_eq!(sl.pick([300.0, 0.0, 0.0], 1.0, 0.0), Some(0));
        assert_eq!(sl.pick([500.0, 0.0, 0.0], 1.0, 0.0), Some(1));
        // past the last LOD, under cullDist: nothing
        assert_eq!(sl.pick([1100.0, 0.0, 0.0], 1.0, 0.0), None);
        assert_eq!(sl.pick([2000.0, 0.0, 0.0], 0.1, 0.0), Some(0));
        // the ramp scales and biases the distance before both tests
        assert_eq!(sl.pick([300.0, 0.0, 0.0], 2.0, 0.0), Some(1));
        assert_eq!(sl.pick([100.0, 0.0, 0.0], 1.0, 1900.0), None);
        // 65 horizontal at 4:3 is about 50.9 vertical: a ramp under 1
        let (scale, bias) = cod4_lod_ramp(50.9, 1.0, 0.0);
        assert!((scale - 1.008).abs() < 0.01 && bias == 0.0, "{scale}");
    }
}
