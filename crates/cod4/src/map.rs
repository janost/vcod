//! One map out of its walked zone: the render world's vertices, surfaces,
//! materials, cells and static models, the entity string, and the
//! collision map's counts with every index in it range-checked. Layouts
//! and the evidence for each: `docs/research/cod4-world.md`.

use crate::assets::Assets;
use crate::assets::world::{ClipMap, GfxCell, GfxWorld};
use crate::install::Install;
use crate::state::{GfxState, caster_state, draw_state};
use crate::techset::{TechClass, classify};
use crate::walk::{WalkOptions, Walked, u16s, walk};
use crate::zone::{Pointer, Zone};
use anyhow::{Context, Result, bail, ensure};
use std::collections::{BTreeSet, HashMap};

/// `GfxWorldVertex` decoded (0x2C: xyz, binormal sign, colour, texture
/// and lightmap coordinates, packed normal and tangent).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WorldVertex {
    pub pos: [f32; 3],
    /// The binormal is `cross(normal, tangent) * binormal_sign`.
    pub binormal_sign: f32,
    /// R, G, B, A (stored B, G, R, A).
    pub color: [u8; 4],
    pub uv: [f32; 2],
    pub lm_uv: [f32; 2],
    pub normal: [f32; 3],
    pub tangent: [f32; 3],
}

/// A packed normal or tangent: three bytes centred on 127 and a fourth
/// that scales all three by `(w + 192) / 32385`, the scale stored as a
/// float before the three products (iw3mp.exe 0x624003, cod4-world.md 1).
pub fn unpack_unit_vec([x, y, z, w]: [u8; 4]) -> [f32; 3] {
    let scale = (f32::from(w) + 192.0) / 32385.0;
    [x, y, z].map(|c| (f32::from(c) - 127.0) * scale)
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
    u16::from_le_bytes([b[o], b[o + 1]])
}
fn i16_at(b: &[u8], o: usize) -> i16 {
    u16_at(b, o) as i16
}
fn vec3_at(b: &[u8], o: usize) -> [f32; 3] {
    [f32_at(b, o), f32_at(b, o + 4), f32_at(b, o + 8)]
}

impl WorldVertex {
    fn decode(b: &[u8]) -> Self {
        WorldVertex {
            pos: vec3_at(b, 0),
            binormal_sign: f32_at(b, 12),
            color: [b[18], b[17], b[16], b[19]],
            uv: [f32_at(b, 20), f32_at(b, 24)],
            lm_uv: [f32_at(b, 28), f32_at(b, 32)],
            normal: unpack_unit_vec(b[36..40].try_into().unwrap()),
            tangent: unpack_unit_vec(b[40..44].try_into().unwrap()),
        }
    }
}

/// `GfxSurface` (0x30) with its `srfTriangles_t`: `tri_count` triangles at
/// `base_index` of [`Map::indices`], each index relative to `first_vertex`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Surface {
    /// Index into [`Map::materials`].
    pub material: u32,
    /// Index into [`Map::lightmaps`], or [`NO_LIGHTMAP`].
    pub lightmap: u8,
    pub reflection_probe: u8,
    pub primary_light: u8,
    pub flags: u8,
    pub first_vertex: u32,
    pub vertex_count: u16,
    pub tri_count: u16,
    pub base_index: u32,
    pub mins: [f32; 3],
    pub maxs: [f32; 3],
}

/// A surface's `lightmapIndex` when it has no lightmap.
pub const NO_LIGHTMAP: u8 = 31;

/// A material as the map uses it: its technique set by name and class,
/// and each texture's image by name.
#[derive(Clone, Debug)]
pub struct MapMaterial {
    pub name: String,
    /// `None` only for a material whose techset slot is null. Names here
    /// and in [`MapTexture::image`] drop a stub's `,` ([`stub_name`]).
    pub techset: Option<String>,
    pub class: Option<TechClass>,
    pub sort_key: u8,
    pub textures: Vec<MapTexture>,
    /// `GfxStateBits.loadBits` per entry.
    pub state_bits: Vec<[u32; 2]>,
    /// `stateBitsEntry`: per technique type, its entry in `state_bits`.
    pub state_bits_entry: Vec<u8>,
    /// The lit view's state ([`draw_state`]); `None` when no drawing
    /// technique names an entry.
    pub draw_state: Option<GfxState>,
    /// The sun shadow map's caster state ([`caster_state`]); `None` casts
    /// no shadow.
    pub caster_state: Option<GfxState>,
    /// `MaterialConstantDef`s: the name's [`sampler_hash`] and the literal.
    pub constants: Vec<(u32, [f32; 4])>,
    /// A water map texture's spectrum (`water_t`), when one is kept.
    pub water: Option<WaterSpectrum>,
    /// What 1.7's material order reads ([`crate::drawsurf`]); a stub
    /// technique set's keys come from [`Map::resolve_material_orders`].
    pub order: crate::drawsurf::MaterialOrder,
}

/// `water_t`'s stored spectrum: `H0` and `wTerm` per texel of its `M` x
/// `N` (cod4-world.md 9.8).
#[derive(Clone, Debug, PartialEq)]
pub struct WaterSpectrum {
    pub m: usize,
    pub n: usize,
    pub h0: Vec<[f32; 2]>,
    pub w_term: Vec<f32>,
}

/// `MaterialTextureDef`: the sampler's name hash and the image.
#[derive(Clone, Debug)]
pub struct MapTexture {
    pub semantic: u8,
    pub name_hash: u32,
    /// The sampler's name when the hash is a stock one ([`SAMPLERS`]).
    pub sampler: Option<&'static str>,
    pub image: Option<String>,
}

/// `TS_*` texture semantics a world material uses.
pub mod semantic {
    pub const COLOR_MAP: u8 = 2;
    pub const NORMAL_MAP: u8 = 5;
    pub const SPECULAR_MAP: u8 = 8;
    pub const WATER_MAP: u8 = 11;
}

/// The sampler names a stock material's texture defs carry, hashed by
/// [`sampler_hash`].
pub const SAMPLERS: [&str; 4] = ["colorMap", "detailMap", "normalMap", "specularMap"];

/// `R_HashString` over a sampler or constant name: `h = (c | 0x20) ^ (33 * h)`.
pub fn sampler_hash(name: &str) -> u32 {
    name.bytes()
        .fold(0u32, |h, c| u32::from(c | 0x20) ^ h.wrapping_mul(33))
}

fn sampler_name(hash: u32) -> Option<&'static str> {
    SAMPLERS.into_iter().find(|s| sampler_hash(s) == hash)
}

/// `GfxStaticModelDrawInst` (0x4C): a placed model.
#[derive(Clone, Debug, PartialEq)]
pub struct StaticModel {
    pub model: Option<String>,
    /// Index into [`Map::models`], `None` for a model with no geometry kept.
    pub mesh: Option<usize>,
    pub origin: [f32; 3],
    /// Rows: forward, left, up.
    pub axis: [[f32; 3]; 3],
    pub scale: f32,
    pub cull_dist: f32,
    pub reflection_probe: u8,
    pub primary_light: u8,
    pub lighting_handle: u16,
    pub flags: u8,
    /// `GfxStaticModelInst`'s bounds' centre, where its light grid sample
    /// is taken (`R_SetStaticModelLighting`).
    pub lighting_origin: [f32; 3],
    /// `groundLighting`, B, G, R, A as stored; non-zero lights the whole
    /// model with it instead of the grid.
    pub ground_lighting: [u8; 4],
}

/// An xmodel's first LOD: its surfaces in model space.
#[derive(Clone, Debug, PartialEq)]
pub struct ModelMesh {
    pub name: String,
    pub surfaces: Vec<ModelSurface>,
    /// `lodInfo[i].dist` for each of the model's `numLods`, LOD 0 first:
    /// LOD `i` draws below its distance.
    pub lod_dists: Vec<f32>,
    /// LODs 1 and on, in model space as `surfaces` is. Kept for static
    /// models only; [`Map::add_model`] leaves it empty.
    pub coarser: Vec<Vec<ModelSurface>>,
}

/// One `XSurface` of a model's first LOD: `GfxPackedVertex` (0x20)
/// decoded, triangles indexing them, and the surface's material.
#[derive(Clone, Debug, PartialEq)]
pub struct ModelSurface {
    /// Index into [`Map::materials`].
    pub material: u32,
    pub vertices: Vec<ModelVertex>,
    pub indices: Vec<u16>,
}

/// `GfxPackedVertex` decoded.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ModelVertex {
    pub pos: [f32; 3],
    /// R, G, B, A.
    pub color: [u8; 4],
    pub uv: [f32; 2],
    pub normal: [f32; 3],
    pub tangent: [f32; 3],
    /// The binormal is `cross(normal, tangent)` times this.
    pub binormal_sign: f32,
}

/// `GfxCullGroup` (0x20): bounds and a run of `dpvs.sortedSurfIndex`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CullGroup {
    pub mins: [f32; 3],
    pub maxs: [f32; 3],
    pub surface_count: u32,
    pub start_surf_index: u32,
}

/// A `ComPrimaryLight` (0x44): the map's lights, the sun among them.
#[derive(Clone, Debug, PartialEq)]
pub struct PrimaryLight {
    /// `GFX_LIGHT_TYPE_*`: 1 the sun, 2 a spot, 3 an omni.
    pub kind: u8,
    /// `canUseShadowMap` (byte 1): a spot that may take a shadow map
    /// (cod4-world.md 3.5).
    pub can_shadow: bool,
    /// The spot's exponent.
    pub exponent: u8,
    pub color: [f32; 3],
    /// The unit vector toward the light: toward the sun, against a
    /// spot's beam.
    pub dir: [f32; 3],
    pub origin: [f32; 3],
    pub radius: f32,
    pub cos_half_fov_outer: f32,
    pub cos_half_fov_inner: f32,
    pub cos_half_fov_expanded: f32,
    /// Its light def (`GfxLightDef`), whose image is its attenuation.
    pub def: Option<String>,
}

/// [`Map::cell_at`] over the dpvs tree's parts.
fn cell_for_point(cells: usize, n: &[u16], planes: &[Plane], p: [f32; 3]) -> Option<usize> {
    let mut k = 0usize;
    // A malformed tree must not spin.
    for _ in 0..n.len() {
        let v = *n.get(k)? as usize;
        if v <= cells {
            return v.checked_sub(1);
        }
        let pl = planes.get(v - (cells + 1))?;
        let d = p[0] * pl.normal[0] + p[1] * pl.normal[1] + p[2] * pl.normal[2] - pl.dist;
        k += if d <= 0.0 { *n.get(k + 1)? as usize } else { 2 };
    }
    None
}

/// A map's cells, their probe lists and the probes' origins, for
/// [`ProbeFinder::at`].
#[derive(Clone, Debug, Default)]
pub struct ProbeFinder {
    cell_probes: Vec<Vec<u8>>,
    nodes: Vec<u16>,
    planes: Vec<Plane>,
    origins: Vec<[f32; 3]>,
}

impl ProbeFinder {
    /// `R_CalcReflectionProbeIndex` (1.7 0x6077c0, cod4-world.md 9.7):
    /// the probe nearest `p` among its cell's, the first on a tie
    /// (0x607490); outside every cell the nearest of all but probe 0
    /// (0x607710); 0 when none is nearer than `f32::MAX`.
    pub fn at(&self, p: [f32; 3]) -> u8 {
        let dist = |i: u8| {
            self.origins.get(usize::from(i)).map_or(f32::MAX, |o| {
                let d = [p[0] - o[0], p[1] - o[1], p[2] - o[2]];
                d[0] * d[0] + d[1] * d[1] + d[2] * d[2]
            })
        };
        let nearest = |probes: &mut dyn Iterator<Item = u8>| {
            let (mut best, mut best_dist) = (0u8, f32::MAX);
            for i in probes {
                let d = dist(i);
                if best_dist > d {
                    (best, best_dist) = (i, d);
                }
            }
            best
        };
        let cell = cell_for_point(self.cell_probes.len(), &self.nodes, &self.planes, p);
        match cell.and_then(|c| self.cell_probes.get(c)) {
            Some(c) => nearest(&mut c.iter().copied()),
            None => nearest(&mut (1..self.origins.len().min(255) as u8)),
        }
    }
}

/// A packed texture coordinate pair, `u` in the high half and `v` in the
/// low, each a 16-bit float (sign, 5-bit exponent, 10-bit mantissa).
/// iw3mp.exe 0x5648e0 rebases the exponent by 112 into an f32's and has
/// no zero, denormal or infinity case but a half that is all zero bits.
pub fn unpack_tex_coords(packed: u32) -> [f32; 2] {
    fn widen(half: u32) -> f32 {
        if half == 0 {
            return 0.0;
        }
        let sign = (half & 0x8000) << 16;
        let exponent = ((half >> 10 & 0x1f) + 112) << 23;
        let mantissa = (half & 0x3ff) << 13;
        f32::from_bits(sign | exponent | mantissa)
    }
    [widen(packed >> 16), widen(packed & 0xffff)]
}

/// A `cplane_s` (0x14): normal and distance.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Plane {
    pub normal: [f32; 3],
    pub dist: f32,
}

/// The collision map's counts, each index in it checked against the
/// array it names by [`check_clip`].
#[derive(Clone, Debug, Default)]
pub struct Collision {
    pub name: String,
    pub planes: usize,
    pub materials: Vec<ClipMaterial>,
    pub brushes: usize,
    pub brush_sides: usize,
    pub brush_edges: usize,
    pub nodes: usize,
    pub leafs: usize,
    pub leaf_brush_nodes: usize,
    pub leaf_brushes: usize,
    pub leaf_surfaces: usize,
    pub verts: usize,
    pub tris: usize,
    pub borders: usize,
    pub partitions: usize,
    pub aabb_trees: usize,
    pub submodels: usize,
    pub static_models: usize,
    /// `numClusters`, and the highest cluster a leaf names.
    pub clusters: usize,
    pub max_cluster: usize,
    pub dyn_ents: [u16; 2],
    /// Pointers and indices checked.
    pub checked: usize,
}

/// `dmaterial_t` (0x48): name, surface and content flags.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClipMaterial {
    pub name: String,
    pub surface_flags: u32,
    pub content_flags: u32,
}

/// One map: what the client draws and what the server needs to know is
/// there. Built from the map zone's walk ([`Map::from_assets`]).
#[derive(Debug)]
pub struct Map {
    /// The gfx world's name, `maps/mp/<map>.d3dbsp`.
    pub bsp_name: String,
    pub mins: [f32; 3],
    pub maxs: [f32; 3],
    pub vertices: Vec<WorldVertex>,
    pub indices: Vec<u16>,
    pub surfaces: Vec<Surface>,
    /// The world's list, then any a surface adds, then the static models'.
    pub materials: Vec<MapMaterial>,
    /// Primary and secondary image per lightmap.
    pub lightmaps: Vec<[Option<String>; 2]>,
    pub reflection_probes: Vec<Option<String>>,
    /// Each probe's origin, as [`Map::reflection_probes`].
    pub reflection_probe_origins: Vec<[f32; 3]>,
    pub sky_image: Option<String>,
    pub outdoor_image: Option<String>,
    /// Portals name cells by index; every one is checked in range.
    pub cells: Vec<GfxCell>,
    /// `dpvs.sortedSurfIndex`: what a cell tree's surface range indexes.
    pub sorted_surfaces: Vec<u16>,
    pub dpvs_planes: Vec<Plane>,
    pub dpvs_nodes: Vec<u16>,
    /// `skySurfIndices`: the surfaces the sky draws, indices into
    /// [`Map::surfaces`].
    pub sky_surfaces: Vec<u32>,
    /// `GfxBrushModel` surface runs (`startSurfIndex`, `surfaceCount`);
    /// model 0 is the world.
    pub brush_models: Vec<(u16, u16)>,
    pub cull_groups: Vec<CullGroup>,
    pub static_models: Vec<StaticModel>,
    /// The first LOD of each static model's xmodel, each once.
    pub models: Vec<ModelMesh>,
    /// The comworld's primary lights; [`Map::sun`] picks the sun.
    pub primary_lights: Vec<PrimaryLight>,
    /// Each primary light's region hulls (`GfxWorld::lightRegion`).
    pub light_regions: Vec<Vec<crate::assets::world::LightRegionHull>>,
    /// `sunPrimaryLightIndex`.
    pub sun_light_index: u32,
    pub entity_string: String,
    /// Each `{ }` block's pairs in order.
    pub entities: Vec<Vec<(String, String)>>,
    pub collision: Collision,
    /// The light grid the static models are lit from.
    pub light_grid: Option<crate::lightgrid::LightGrid>,
}

impl Map {
    /// Walks `zone/<language>/<name>.ff` (or a usermap's,
    /// [`Install::zone_file`]) with the bulk data kept and builds the map;
    /// the walk is returned too, for its images.
    pub fn load(install: &Install, language: &str, name: &str) -> Result<(Map, Walked)> {
        let path = install.zone_path(language, name);
        let zone = Zone::open_path(&path).with_context(|| path.display().to_string())?;
        let walked = walk(zone, WalkOptions { keep_bulk: true })?;
        ensure!(
            walked.stats.unresolved.is_empty(),
            "{name}: unresolved offsets {:?}",
            walked.stats.unresolved
        );
        let map = Map::from_assets(&walked.assets).with_context(|| name.to_string())?;
        Ok((map, walked))
    }

    /// The map in a walked zone: its one gfx world and one clip map, walked
    /// with [`WalkOptions::keep_bulk`].
    pub fn from_assets(a: &Assets) -> Result<Map> {
        let [g] = &a.gfx_worlds[..] else {
            bail!("{} gfx worlds", a.gfx_worlds.len());
        };
        let clip = match (&a.clip_maps_pvs[..], &a.clip_maps[..]) {
            ([c], []) => c,
            ([], [c]) => &c.0,
            (p, s) => bail!("{} + {} clip maps", p.len(), s.len()),
        };
        let mut map = render_world(a, g)?;
        map.collision = check_clip(clip)?;
        if let [cw] = &a.com_worlds[..] {
            map.primary_lights = cw
                .primary_lights
                .iter()
                .map(|(b, def)| PrimaryLight {
                    kind: b[0],
                    can_shadow: b[1] != 0,
                    exponent: b[2],
                    color: vec3_at(b, 4),
                    dir: vec3_at(b, 16),
                    origin: vec3_at(b, 28),
                    radius: f32_at(b, 40),
                    cos_half_fov_outer: f32_at(b, 44),
                    cos_half_fov_inner: f32_at(b, 48),
                    cos_half_fov_expanded: f32_at(b, 52),
                    def: def.clone(),
                })
                .collect();
        }
        let ents = clip
            .map_ents
            .map(|e| &a.map_ents[e.index()])
            .context("clip map without map_ents")?;
        let text = &ents.entity_string;
        let text = text.strip_suffix(&[0]).unwrap_or(text);
        map.entity_string = String::from_utf8_lossy(text).into_owned();
        map.entities = vcod_common::bsp::entity_block_pairs(&map.entity_string);
        Ok(map)
    }

    /// Fills the material order keys of each material whose technique set
    /// the map's zone holds only as a stub, from the first of `zones`
    /// (highest priority first) that holds the set's techniques.
    pub fn resolve_material_orders(&mut self, zones: &[&Assets]) {
        let mut sets: HashMap<&str, (&Assets, &crate::assets::material::TechSet)> = HashMap::new();
        for &z in zones.iter().rev() {
            for t in &z.techsets {
                if t.techniques.iter().any(Option::is_some) {
                    sets.insert(stub_name(&t.name), (z, t));
                }
            }
        }
        for m in &mut self.materials {
            if m.order.has_techset() {
                continue;
            }
            if let Some(&(z, t)) = m.techset.as_deref().and_then(|n| sets.get(n)) {
                m.order.set_techset(z, t);
            }
        }
    }

    /// Every image the map names (materials, lightmaps, probes, sky,
    /// outdoor), each once, sorted.
    pub fn image_names(&self) -> BTreeSet<&str> {
        let mut out = BTreeSet::new();
        for m in &self.materials {
            out.extend(m.textures.iter().filter_map(|t| t.image.as_deref()));
        }
        for l in &self.lightmaps {
            out.extend(l.iter().flatten().map(String::as_str));
        }
        out.extend(self.reflection_probes.iter().flatten().map(String::as_str));
        out.extend(self.sky_image.as_deref());
        out.extend(self.outdoor_image.as_deref());
        out
    }

    /// The sun: primary light `sunPrimaryLightIndex`, when the comworld
    /// has it.
    pub fn sun(&self) -> Option<&PrimaryLight> {
        self.primary_lights.get(self.sun_light_index as usize)
    }

    /// `R_CellForPoint`: the cell `p` is in, walking `dpvsPlanes.nodes`
    /// from the first. An entry below `cells + 1` is a leaf naming cell
    /// `entry - 1` (0 for none); any other tests plane `entry - (cells +
    /// 1)` and steps 2 entries on in front of it, `node[1]` entries on
    /// behind it or on it.
    pub fn cell_at(&self, p: [f32; 3]) -> Option<usize> {
        cell_for_point(self.cells.len(), &self.dpvs_nodes, &self.dpvs_planes, p)
    }

    /// What picks an entity's reflection probe ([`ProbeFinder::at`]).
    pub fn probe_finder(&self) -> ProbeFinder {
        ProbeFinder {
            cell_probes: self
                .cells
                .iter()
                .map(|c| c.reflection_probes.clone())
                .collect(),
            nodes: self.dpvs_nodes.clone(),
            planes: self.dpvs_planes.clone(),
            origins: self.reflection_probe_origins.clone(),
        }
    }

    /// `x`'s first LOD in model space, its materials appended to
    /// [`Map::materials`] (each name once): an entity's model, drawn where
    /// its entity stands. `None` for a model with no geometry kept.
    pub fn add_model(
        &mut self,
        a: &Assets,
        x: &crate::assets::model::XModel,
    ) -> Result<Option<ModelMesh>> {
        let surfaces = model_surfaces(x, 0, &mut |id| self.add_material(a, id))?;
        Ok((!surfaces.is_empty()).then(|| ModelMesh {
            name: x.name.clone(),
            surfaces,
            lod_dists: lod_dists(x),
            coarser: Vec::new(),
        }))
    }

    /// Zone material `id` in [`Map::materials`], appended once by name.
    pub fn add_material(
        &mut self,
        a: &Assets,
        id: crate::assets::Id<crate::assets::material::Material>,
    ) -> u32 {
        let name = &a.materials[id.index()].name;
        match self.materials.iter().position(|m| &m.name == name) {
            Some(i) => i as u32,
            None => {
                self.materials.push(map_material(a, id));
                self.materials.len() as u32 - 1
            }
        }
    }

    /// A surface's triangles as vertex indices into [`Map::vertices`].
    pub fn triangles(&self, s: &Surface) -> impl Iterator<Item = [u32; 3]> + '_ {
        let s = *s;
        let b = s.base_index as usize;
        self.indices[b..b + 3 * s.tri_count as usize]
            .chunks(3)
            .map(move |t| [0, 1, 2].map(|k| s.first_vertex + u32::from(t[k])))
    }
}

/// A name without the leading `,` that marks a stub: an asset the zone
/// names but an earlier zone carries.
pub fn stub_name(name: &str) -> &str {
    name.strip_prefix(',').unwrap_or(name)
}

fn bulk<'a>(b: &'a Option<Vec<u8>>, what: &str) -> Result<&'a [u8]> {
    b.as_deref()
        .with_context(|| format!("{what} not kept (walk without keep_bulk)"))
}

/// One zone material as the map keeps it.
pub fn map_material(
    a: &Assets,
    id: crate::assets::Id<crate::assets::material::Material>,
) -> MapMaterial {
    let image_name = |i: Option<crate::assets::Id<_>>| {
        i.map(|i: crate::assets::Id<crate::assets::material::Image>| {
            stub_name(&a.images[i.index()].name).to_owned()
        })
    };
    let m = &a.materials[id.index()];
    let techset = m
        .techset
        .map(|t| stub_name(&a.techsets[t.index()].name).to_owned());
    let own = m
        .techset
        .map(|t| &a.techsets[t.index()])
        .filter(|t| t.techniques.iter().any(Option::is_some));
    MapMaterial {
        order: crate::drawsurf::MaterialOrder::of(m, own.map(|t| (a, t))),
        name: m.name.clone(),
        class: techset.as_deref().map(classify),
        techset,
        sort_key: m.sort_key,
        textures: m
            .textures
            .iter()
            .map(|t| MapTexture {
                semantic: t.semantic,
                name_hash: t.name_hash,
                sampler: sampler_name(t.name_hash),
                image: image_name(t.image),
            })
            .collect(),
        state_bits: m.state_bits.clone(),
        state_bits_entry: m.state_bits_entry.clone(),
        draw_state: draw_state(&m.state_bits_entry, &m.state_bits),
        caster_state: caster_state(&m.state_bits_entry, &m.state_bits),
        constants: m
            .constants
            .iter()
            .map(|c| (c.name_hash, c.literal))
            .collect(),
        water: m.textures.iter().find_map(|t| {
            let w = t.water.as_ref()?;
            let (m, n) = (w.m as usize, w.n as usize);
            let f = |b: &[u8]| -> Vec<f32> {
                b.as_chunks::<4>()
                    .0
                    .iter()
                    .map(|c| f32::from_le_bytes(*c))
                    .collect()
            };
            let h0 = f(w.h0.as_deref()?);
            let w_term = f(w.w_term.as_deref()?);
            (h0.len() == 2 * m * n && w_term.len() == m * n).then(|| WaterSpectrum {
                m,
                n,
                h0: h0.chunks(2).map(|c| [c[0], c[1]]).collect(),
                w_term,
            })
        }),
    }
}

fn render_world(a: &Assets, g: &GfxWorld) -> Result<Map> {
    let image_name = |i: Option<crate::assets::Id<_>>| {
        i.map(|i: crate::assets::Id<crate::assets::material::Image>| {
            stub_name(&a.images[i.index()].name).to_owned()
        })
    };
    let vb = bulk(&g.vertices, "vertices")?;
    let vertices: Vec<WorldVertex> = vb
        .as_chunks::<0x2C>()
        .0
        .iter()
        .map(|b| WorldVertex::decode(b))
        .collect();
    ensure!(vertices.len() == g.vertex_count as usize, "vertex count");
    let indices = u16s(bulk(&g.indices, "indices")?);
    ensure!(indices.len() == g.index_count as usize, "index count");

    // Materials: the world's list first, then any a surface adds.
    let mut materials = Vec::new();
    let mut by_id = HashMap::new();
    let mut add = |id: crate::assets::Id<crate::assets::material::Material>| {
        *by_id.entry(id).or_insert_with(|| {
            materials.push(map_material(a, id));
            materials.len() as u32 - 1
        })
    };
    for id in g.materials.iter().flatten() {
        add(*id);
    }
    let lightmap_count = g.lightmaps.len();
    let mut surfaces = Vec::with_capacity(g.surfaces.len());
    for (i, (m, s)) in g.surfaces.iter().enumerate() {
        let m = m.with_context(|| format!("surface {i} has no material"))?;
        let surf = Surface {
            material: add(m),
            first_vertex: u32_at(s, 4),
            vertex_count: u16_at(s, 8),
            tri_count: u16_at(s, 10),
            base_index: u32_at(s, 12),
            lightmap: s[20],
            reflection_probe: s[21],
            primary_light: s[22],
            flags: s[23],
            mins: vec3_at(s, 24),
            maxs: vec3_at(s, 36),
        };
        let end = surf.first_vertex as usize + surf.vertex_count as usize;
        ensure!(end <= vertices.len(), "surface {i}: vertices end at {end}");
        let tris = surf.base_index as usize..surf.base_index as usize + 3 * surf.tri_count as usize;
        ensure!(
            tris.end <= indices.len(),
            "surface {i}: indices end at {}",
            tris.end
        );
        if let Some(&max) = indices[tris].iter().max() {
            ensure!(
                max < surf.vertex_count,
                "surface {i}: index {max} of {} vertices",
                surf.vertex_count
            );
        }
        ensure!(
            (surf.lightmap as usize) < lightmap_count || surf.lightmap == NO_LIGHTMAP,
            "surface {i}: lightmap {} of {lightmap_count}",
            surf.lightmap
        );
        ensure!(
            (surf.reflection_probe as usize) < g.reflection_probes.len(),
            "surface {i}: probe {}",
            surf.reflection_probe
        );
        surfaces.push(surf);
    }

    let sorted_surfaces = u16s(bulk(&g.sorted_surf_index, "sorted surfaces")?);
    for (k, &s) in sorted_surfaces.iter().enumerate() {
        ensure!((s as usize) < surfaces.len(), "sorted surface {k}: {s}");
    }
    let mut static_models = Vec::with_capacity(g.static_models.len());
    let mut models = Vec::new();
    let mut mesh_of = HashMap::new();
    let insts = g.smodel_insts.as_deref().unwrap_or_default();
    for (smi, (m, b)) in g.static_models.iter().enumerate() {
        let inst = insts.get(0x1C * smi..0x1C * (smi + 1));
        let mesh = match m {
            Some(id) => match mesh_of.get(id) {
                Some(&i) => i,
                None => {
                    let x = &a.xmodels[id.index()];
                    let surfaces = model_surfaces(x, 0, &mut add)?;
                    let coarser = (1..usize::from(x.num_lods).min(x.lods.len()))
                        .map(|l| model_surfaces(x, l, &mut add))
                        .collect::<Result<Vec<_>>>()?;
                    let i = (!surfaces.is_empty()).then(|| {
                        models.push(ModelMesh {
                            name: x.name.clone(),
                            surfaces,
                            lod_dists: lod_dists(x),
                            coarser,
                        });
                        models.len() - 1
                    });
                    mesh_of.insert(*id, i);
                    i
                }
            },
            None => None,
        };
        static_models.push(StaticModel {
            model: m.map(|m| a.xmodels[m.index()].name.clone()),
            mesh,
            cull_dist: f32_at(b, 0),
            origin: vec3_at(b, 4),
            axis: [vec3_at(b, 16), vec3_at(b, 28), vec3_at(b, 40)],
            scale: f32_at(b, 52),
            reflection_probe: b[68],
            primary_light: b[69],
            lighting_handle: u16_at(b, 70),
            flags: b[72],
            lighting_origin: inst.map_or([0.0; 3], |i| {
                let (lo, hi) = (vec3_at(i, 0), vec3_at(i, 12));
                [0, 1, 2].map(|k| (lo[k] + hi[k]) * 0.5)
            }),
            ground_lighting: inst.map_or([0; 4], |i| [i[24], i[25], i[26], i[27]]),
        });
    }
    check_cells(
        g,
        surfaces.len(),
        sorted_surfaces.len(),
        static_models.len(),
    )?;
    let planes: Vec<Plane> = bulk(&g.planes, "dpvs planes")?
        .as_chunks::<0x14>()
        .0
        .iter()
        .map(|p| Plane {
            normal: vec3_at(p, 0),
            dist: f32_at(p, 12),
        })
        .collect();
    let dpvs_nodes = u16s(bulk(&g.dpvs_nodes, "dpvs nodes")?);
    let sky_surfaces: Vec<u32> = g
        .sky_surfaces
        .as_deref()
        .unwrap_or_default()
        .as_chunks::<4>()
        .0
        .iter()
        .map(|b| u32::from_le_bytes(*b))
        .collect();
    for &s in &sky_surfaces {
        ensure!((s as usize) < surfaces.len(), "sky surface {s}");
    }
    let mut cull_groups = Vec::new();
    for (i, c) in g
        .cull_groups
        .as_deref()
        .unwrap_or_default()
        .as_chunks::<0x20>()
        .0
        .iter()
        .enumerate()
    {
        let cg = CullGroup {
            mins: vec3_at(c, 0),
            maxs: vec3_at(c, 12),
            surface_count: u32_at(c, 24),
            start_surf_index: u32_at(c, 28),
        };
        ensure!(
            cg.surface_count == 0
                || (cg.start_surf_index as usize + cg.surface_count as usize)
                    <= sorted_surfaces.len(),
            "cull group {i}: surfaces {}+{}",
            cg.start_surf_index,
            cg.surface_count
        );
        cull_groups.push(cg);
    }

    Ok(Map {
        bsp_name: g.name.clone(),
        mins: g.mins,
        maxs: g.maxs,
        vertices,
        indices,
        surfaces,
        materials,
        lightmaps: g.lightmaps.iter().map(|l| l.map(image_name)).collect(),
        reflection_probes: g.reflection_probes.iter().map(|&p| image_name(p)).collect(),
        reflection_probe_origins: g.reflection_probe_origins.clone(),
        sky_image: image_name(g.sky_image),
        outdoor_image: image_name(g.outdoor_image),
        cells: g.cells.clone(),
        sorted_surfaces,
        dpvs_planes: planes,
        dpvs_nodes,
        sky_surfaces,
        brush_models: g
            .models
            .as_deref()
            .unwrap_or_default()
            .as_chunks::<0x38>()
            .0
            .iter()
            .map(|m| (u16_at(m, 50), u16_at(m, 48)))
            .collect(),
        cull_groups,
        static_models,
        models,
        primary_lights: Vec::new(),
        light_regions: g.light_regions.clone(),
        sun_light_index: g.sun_primary_light_index,
        entity_string: String::new(),
        entities: Vec::new(),
        collision: Collision::default(),
        light_grid: crate::lightgrid::LightGrid::from_world(&g.light_grid)?,
    })
}

/// An xmodel's first LOD's surfaces, each with its material added to the
/// map's through `add`. A surface whose geometry the walk did not keep is
/// left out.
/// `x`'s `lodInfo[].dist`, its `numLods` of them.
fn lod_dists(x: &crate::assets::model::XModel) -> Vec<f32> {
    x.lods
        .iter()
        .take(usize::from(x.num_lods))
        .map(|l| l.dist)
        .collect()
}

/// LOD `lod`'s surfaces of `x` in model space.
fn model_surfaces(
    x: &crate::assets::model::XModel,
    lod: usize,
    add: &mut impl FnMut(crate::assets::Id<crate::assets::material::Material>) -> u32,
) -> Result<Vec<ModelSurface>> {
    let Some(lod) = x.lods.get(lod).filter(|_| usize::from(x.num_lods) > lod) else {
        return Ok(Vec::new());
    };
    let mut out = Vec::new();
    let first = lod.surf_index as usize;
    for si in first..first + lod.num_surfs as usize {
        let (Some(s), Some(Some(m))) = (x.surfaces.get(si), x.materials.get(si)) else {
            continue;
        };
        let (Some(vb), Some(tb)) = (&s.verts, &s.tris) else {
            continue;
        };
        let vertices: Vec<ModelVertex> = vb
            .as_chunks::<0x20>()
            .0
            .iter()
            .map(|b| ModelVertex {
                pos: vec3_at(b, 0),
                color: [b[18], b[17], b[16], b[19]],
                uv: unpack_tex_coords(u32_at(b, 20)),
                normal: unpack_unit_vec(b[24..28].try_into().unwrap()),
                tangent: unpack_unit_vec(b[28..32].try_into().unwrap()),
                binormal_sign: f32_at(b, 12),
            })
            .collect();
        let indices = u16s(tb);
        ensure!(
            indices.len() == 3 * s.tri_count as usize,
            "{} surface {si}: {} indices for {} triangles",
            x.name,
            indices.len(),
            s.tri_count
        );
        if let Some(&max) = indices.iter().max() {
            ensure!(
                (max as usize) < vertices.len(),
                "{} surface {si}: index {max} of {} vertices",
                x.name,
                vertices.len()
            );
        }
        out.push(ModelSurface {
            material: add(*m),
            vertices,
            indices,
        });
    }
    Ok(out)
}

/// Every cell's portals name a cell; its trees' surface ranges stay in
/// `dpvs.sortedSurfIndex`, their children in the cell's tree array and
/// their static models in the map's.
fn check_cells(g: &GfxWorld, surfaces: usize, sorted: usize, smodels: usize) -> Result<()> {
    ensure!(g.cells.len() == g.cell_count as usize, "cell count");
    for (ci, c) in g.cells.iter().enumerate() {
        for (pi, p) in c.portals.iter().enumerate() {
            ensure!(p.cell.is_some(), "cell {ci} portal {pi} names no cell");
            ensure!(
                p.vertices.len() >= 3,
                "cell {ci} portal {pi}: {} vertices",
                p.vertices.len()
            );
        }
        for (ti, t) in c.aabb_trees.iter().enumerate() {
            let end = t.start_surf_index as usize + t.surface_count as usize;
            ensure!(
                end <= sorted,
                "cell {ci} tree {ti}: surfaces end at {end} of {sorted}"
            );
            let end = t.start_surf_index_no_decal as usize + t.surface_count_no_decal as usize;
            ensure!(
                end <= sorted,
                "cell {ci} tree {ti}: no-decal surfaces end at {end}"
            );
            for &m in &t.smodel_indexes {
                ensure!(
                    (m as usize) < smodels,
                    "cell {ci} tree {ti}: static model {m}"
                );
            }
            if t.child_count > 0 {
                ensure!(
                    t.children_offset > 0 && t.children_offset % 0x2C == 0,
                    "cell {ci} tree {ti}: children at {}",
                    t.children_offset
                );
                let first = ti + t.children_offset as usize / 0x2C;
                ensure!(
                    first + t.child_count as usize <= c.aabb_trees.len(),
                    "cell {ci} tree {ti}: children past the tree"
                );
            }
        }
        for &cg in &c.cull_groups {
            ensure!(
                (0..g.cull_group_count as i32).contains(&cg),
                "cell {ci}: cull group {cg}"
            );
        }
    }
    if let Some(models) = &g.models {
        for (i, m) in models.as_chunks::<0x38>().0.iter().enumerate() {
            // A model with no surfaces keeps 0xFFFF as its start.
            let count = u16_at(m, 48) as usize;
            let end = u16_at(m, 50) as usize + count;
            ensure!(
                count == 0 || end <= surfaces,
                "brush model {i}: surfaces end at {end}"
            );
        }
    }
    Ok(())
}

/// Counts and range checks over a clip map's arrays.
struct Checker {
    checked: usize,
    max_cluster: usize,
}

impl Checker {
    fn index(&mut self, what: &str, i: i64, count: usize) -> Result<usize> {
        self.checked += 1;
        ensure!(i >= 0 && (i as usize) < count, "{what}: {i} of {count}");
        Ok(i as usize)
    }

    /// A stored pointer into the array of `size`-byte entries at packed
    /// address `base`: the entry's index.
    fn pointer(
        &mut self,
        what: &str,
        token: u32,
        base: u32,
        size: u32,
        count: usize,
    ) -> Result<usize> {
        let Pointer::Offset { block, offset } = Pointer::from_token(token) else {
            bail!("{what}: pointer token {token:#x} is not an offset");
        };
        let at = u32::from(block) << 28 | offset;
        let o = at.wrapping_sub(base);
        ensure!(
            o % size == 0,
            "{what}: {at:#x} inside an entry of {base:#x}"
        );
        self.index(what, i64::from(o / size), count)
    }

    /// A `cLeaf_t` (0x2C), in the leaf array or a submodel.
    fn leaf(&mut self, what: &str, l: &[u8], c: &ClipMap) -> Result<()> {
        let first = u16_at(l, 0) as usize;
        let count = u16_at(l, 2) as usize;
        self.checked += 1;
        ensure!(
            first + count <= c.aabb_tree_count as usize,
            "{what}: aabb trees {first}+{count}"
        );
        let node = i64::from(i32_at(l, 36));
        self.index(
            &format!("{what} brush node"),
            node,
            c.leaf_brush_node_count as usize,
        )?;
        let cluster = i16_at(l, 40);
        // Stock MP maps ship one PVS row (`numClusters` 1) whose bits span
        // every leaf cluster: a cluster indexes a bit of the row.
        if cluster >= 0 {
            let bits = 8 * c.cluster_bytes as usize;
            self.index(&format!("{what} cluster"), i64::from(cluster), bits)?;
            self.max_cluster = self.max_cluster.max(cluster as usize);
        }
        Ok(())
    }
}

/// [`Collision`] for `c`, failing on the first index out of its array.
pub fn check_clip(c: &ClipMap) -> Result<Collision> {
    let mut k = Checker {
        checked: 0,
        max_cluster: 0,
    };
    let n = |v: u32| v as usize;
    let materials: Vec<ClipMaterial> = c
        .materials
        .as_chunks::<0x48>()
        .0
        .iter()
        .map(|m| ClipMaterial {
            name: String::from_utf8_lossy(m[..64].split(|&b| b == 0).next().unwrap()).into_owned(),
            surface_flags: u32_at(m, 64),
            content_flags: u32_at(m, 68),
        })
        .collect();
    ensure!(materials.len() == n(c.material_count), "material count");
    let mats = materials.len();
    let planes = bulk(&c.planes, "planes")?;
    ensure!(planes.len() == 0x14 * n(c.plane_count), "plane count");

    let sides = bulk(&c.brush_sides, "brush sides")?;
    for s in sides.as_chunks::<0xC>().0.iter() {
        k.pointer(
            "brush side plane",
            u32_at(s, 0),
            c.at.planes,
            0x14,
            n(c.plane_count),
        )?;
        k.index("brush side material", i64::from(u32_at(s, 4)), mats)?;
    }
    let nodes = bulk(&c.nodes, "nodes")?;
    for nd in nodes.as_chunks::<8>().0.iter() {
        k.pointer(
            "node plane",
            u32_at(nd, 0),
            c.at.planes,
            0x14,
            n(c.plane_count),
        )?;
        for o in [4, 6] {
            let ch = i64::from(i16_at(nd, o));
            if ch >= 0 {
                k.index("node child", ch, n(c.node_count))?;
            } else {
                k.index("node leaf", -1 - ch, n(c.leaf_count))?;
            }
        }
    }
    for l in bulk(&c.leafs, "leafs")?.as_chunks::<0x2C>().0.iter() {
        k.leaf("leaf", l, c)?;
    }
    for &b in &u16s(bulk(&c.leaf_brushes, "leaf brushes")?) {
        k.index("leaf brush", i64::from(b), usize::from(c.brush_count))?;
    }
    let lbn = bulk(&c.leaf_brush_nodes, "leaf brush nodes")?;
    let node_count = n(c.leaf_brush_node_count);
    for (i, e) in lbn.as_chunks::<0x14>().0.iter().enumerate() {
        let count = i16_at(e, 2);
        if count > 0 {
            let list = c.leaf_brush_lists[i]
                .as_ref()
                .with_context(|| format!("leaf brush node {i}: list not copied"))?;
            ensure!(
                list.len() == count as usize,
                "leaf brush node {i}: list length"
            );
            // The last node is the box model's: its one brush is the box
            // brush, numbered past the map's.
            let brushes = usize::from(c.brush_count) + usize::from(i + 1 == node_count);
            for &b in list {
                k.index("leaf brush node brush", i64::from(b), brushes)?;
            }
            continue;
        }
        k.index("leaf brush node axis", i64::from(e[0]), 3)?;
        if count < 0 {
            k.index("leaf brush node next", i as i64 + 1, node_count)?;
        }
        for o in [16, 18] {
            k.index(
                "leaf brush node child",
                i as i64 + i64::from(u16_at(e, o)),
                node_count,
            )?;
        }
    }
    let verts = n(c.vert_count);
    for &v in &u16s(bulk(&c.tri_indices, "tri indices")?) {
        k.index("tri vertex", i64::from(v), verts)?;
    }
    for p in bulk(&c.partitions, "partitions")?
        .as_chunks::<0xC>()
        .0
        .iter()
    {
        let (tris, borders) = (p[0] as usize, p[1] as usize);
        let first = i32_at(p, 4);
        k.checked += 1;
        ensure!(
            first >= 0 && first as usize + tris <= n(c.tri_count),
            "partition tris {first}+{tris}"
        );
        if borders > 0 {
            let b = k.pointer(
                "partition borders",
                u32_at(p, 8),
                c.at.borders,
                0x1C,
                n(c.border_count),
            )?;
            ensure!(
                b + borders <= n(c.border_count),
                "partition borders {b}+{borders}"
            );
        }
    }
    let trees = n(c.aabb_tree_count);
    for t in bulk(&c.aabb_trees, "aabb trees")?
        .as_chunks::<0x20>()
        .0
        .iter()
    {
        k.index("aabb tree material", i64::from(u16_at(t, 24)), mats)?;
        let children = u16_at(t, 26) as usize;
        let u = i32_at(t, 28);
        if children > 0 {
            k.checked += 1;
            ensure!(
                u >= 0 && u as usize + children <= trees,
                "aabb tree children {u}+{children}"
            );
        } else {
            k.index("aabb tree partition", i64::from(u), n(c.partition_count))?;
        }
    }
    for m in bulk(&c.cmodels, "submodels")?.as_chunks::<0x48>().0.iter() {
        k.leaf("submodel leaf", &m[28..], c)?;
    }
    let edges = n(c.brush_edge_count);
    for (i, b) in bulk(&c.brushes, "brushes")?
        .as_chunks::<0x50>()
        .0
        .iter()
        .enumerate()
    {
        let what = || format!("brush {i}");
        let numsides = u32_at(b, 28) as usize;
        if numsides > 0 {
            let s = k
                .pointer(
                    "brush sides",
                    u32_at(b, 32),
                    c.at.brush_sides,
                    0xC,
                    n(c.brush_side_count),
                )
                .with_context(what)?;
            ensure!(
                s + numsides <= n(c.brush_side_count),
                "{}: sides past the array",
                what()
            );
        }
        for o in (36..48).step_by(2) {
            k.index("brush axial material", i64::from(i16_at(b, o)), mats)
                .with_context(what)?;
        }
        // A brush with no edges may point anywhere (stock ones point at the
        // brush sides' start); nothing reads through it.
        if b[64..70].iter().any(|&n| n > 0) {
            let e = k
                .pointer(
                    "brush edges",
                    u32_at(b, 48),
                    c.at.brush_edges,
                    1,
                    edges.max(1),
                )
                .with_context(what)?;
            for j in 0..6 {
                let first = i64::from(i16_at(b, 52 + 2 * j));
                let count = i64::from(b[64 + j]);
                k.checked += 1;
                ensure!(
                    first >= 0 && e as i64 + first + count <= edges as i64,
                    "{}: axial side {j} edges {first}+{count}",
                    what()
                );
            }
        }
    }
    for (i, (m, _)) in c.static_models.iter().enumerate() {
        ensure!(m.is_some(), "static model {i} has no xmodel");
        k.checked += 1;
    }
    Ok(Collision {
        name: c.name.clone(),
        planes: n(c.plane_count),
        materials,
        brushes: usize::from(c.brush_count),
        brush_sides: n(c.brush_side_count),
        brush_edges: edges,
        nodes: n(c.node_count),
        leafs: n(c.leaf_count),
        leaf_brush_nodes: node_count,
        leaf_brushes: n(c.leaf_brush_count),
        leaf_surfaces: n(c.leaf_surface_count),
        verts,
        tris: n(c.tri_count),
        borders: n(c.border_count),
        partitions: n(c.partition_count),
        aabb_trees: trees,
        submodels: n(c.submodel_count),
        static_models: c.static_models.len(),
        clusters: n(c.cluster_count),
        max_cluster: k.max_cluster,
        dyn_ents: c.dyn_ent_count,
        checked: k.checked,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unpacks_unit_vectors() {
        // 127 is zero; the fourth byte scales: (63 + 192) / 32385 * 127 = 1.
        assert_eq!(unpack_unit_vec([254, 127, 127, 63]), [1.0, 0.0, 0.0]);
        let [x, ..] = unpack_unit_vec([0, 127, 127, 63]);
        assert_eq!(x, -1.0);
    }

    #[test]
    fn hashes_sampler_names() {
        assert_eq!(sampler_hash("colorMap"), 0xA0AB_1041);
        assert_eq!(sampler_name(0x59D3_0D0F), Some("normalMap"));
    }

    /// FNV-1a over a stream of words: the pins below hash every output's bits.
    fn fnv(words: impl IntoIterator<Item = u32>) -> u64 {
        words.into_iter().fold(0xcbf2_9ce4_8422_2325, |h, w| {
            (h ^ u64::from(w)).wrapping_mul(0x100_0000_01b3)
        })
    }

    /// Every byte pair through the unit vector unpack, bit for bit.
    #[test]
    fn pins_the_unit_vector_unpack() {
        let bits = (0..=255u8).flat_map(|c| {
            (0..=255u8).flat_map(move |w| unpack_unit_vec([c, !c, c ^ 0x55, w]).map(f32::to_bits))
        });
        assert_eq!(fnv(bits), PIN_UNIT);
    }

    /// Every half through the texture coordinate unpack, both words.
    #[test]
    fn pins_the_tex_coord_unpack() {
        assert_eq!(unpack_tex_coords(0x3c00_0000), [1.0, 0.0]);
        assert_eq!(unpack_tex_coords(0x0000_c000), [0.0, -2.0]);
        let bits = (0..=0xffffu32)
            .flat_map(|h| unpack_tex_coords(h << 16 | (h ^ 0x8001)).map(f32::to_bits));
        assert_eq!(fnv(bits), PIN_TEX);
    }
    const PIN_UNIT: u64 = 3097659338910776972;
    const PIN_TEX: u64 = 5623642612305634085;
}
