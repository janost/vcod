//! CoD2 1.3 `maps/mp/*.d3dbsp` (IBSP version 4, 39 lumps), decoded into the
//! raw records of `docs/research/cod2-d3dbsp-format.md` with its field
//! names. [`D3dBsp::parse`] checks every index and cross-reference the
//! census (`tools/re/d3dbsp_census.py`) and the retail loaders check, and
//! errors on bad data. Nothing here interprets the map for the engine.

mod brushtree;

pub use brushtree::{BrushBox, BrushNode, BrushTree, partition_brushes};
pub use vcod_common::clipmap::{CollAabbTree, CollBorder, CollPartition, CollTri};

use anyhow::{Result, bail, ensure};
use glam::{Vec2, Vec3};

/// `IBSP`.
pub const MAGIC: &[u8; 4] = b"IBSP";
pub const VERSION: u32 = 4;
pub const LUMP_COUNT: usize = 39;
/// The header: magic, version and 39 `(length, offset)` pairs.
pub const HEADER_BYTES: usize = 8 + 8 * LUMP_COUNT;
/// One lump 1 page: three 512x512 RGBA images and a 1024x1024 L8 one.
pub const LIGHTMAP_PAGE_BYTES: usize = 0x40_0000;
/// A draw surface's `lightmap` for none.
pub const NO_LIGHTMAP: i16 = 31;
/// `CMod_LoadSubmodels`' "MAX_SUBMODELS exceeded".
pub const MAX_SUBMODELS: usize = 0x400;
/// The bits `CMod_LoadBrushes` clears off a brush's material contents.
pub const BRUSH_CONTENTS_MASK: u32 = 0xdfff_fffb;

/// Lump numbers (doc, "Lump directory").
pub mod lump {
    pub const MATERIALS: usize = 0;
    pub const LIGHTMAPS: usize = 1;
    pub const LIGHT_GRID_ENTRIES: usize = 2;
    pub const LIGHT_GRID_COLOURS: usize = 3;
    pub const PLANES: usize = 4;
    pub const BRUSH_SIDES: usize = 5;
    pub const BRUSHES: usize = 6;
    pub const SURFACES: usize = 7;
    pub const VERTS: usize = 8;
    pub const INDICES: usize = 9;
    pub const CULL_GROUPS: usize = 10;
    pub const CULL_GROUP_INDICES: usize = 11;
    pub const PORTAL_VERTS: usize = 17;
    pub const OCCLUDERS: usize = 18;
    pub const OCCLUDER_PLANES: usize = 19;
    pub const OCCLUDER_EDGES: usize = 20;
    pub const OCCLUDER_INDICES: usize = 21;
    pub const AABB_TREES: usize = 22;
    pub const CELLS: usize = 23;
    pub const PORTALS: usize = 24;
    pub const NODES: usize = 25;
    pub const LEAFS: usize = 26;
    pub const LEAF_BRUSHES: usize = 27;
    pub const LEAF_SURFACES: usize = 28;
    pub const COLL_VERTS: usize = 29;
    pub const COLL_EDGES: usize = 30;
    pub const COLL_TRIS: usize = 31;
    pub const COLL_BORDERS: usize = 32;
    pub const COLL_PARTITIONS: usize = 33;
    pub const COLL_AABB_TREES: usize = 34;
    pub const MODELS: usize = 35;
    pub const VISIBILITY: usize = 36;
    pub const ENTITIES: usize = 37;
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LumpEntry {
    pub length: u32,
    pub offset: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Header {
    pub version: u32,
    pub lumps: [LumpEntry; LUMP_COUNT],
}

/// Lump 0.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Material {
    pub name: String,
    pub surface_flags: u32,
    pub content_flags: u32,
}

/// One lump 1 page, split into its four images.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LightmapPage {
    /// Images 0, 1, 2: 512x512, 4 bytes a texel (A8R8G8B8), the red, green
    /// and blue coefficients (INFERRED in the doc).
    pub rgba: [Vec<u8>; 3],
    /// Image 3: 1024x1024, 1 byte a texel (L8), the sun visibility.
    pub sun: Vec<u8>,
}

impl LightmapPage {
    pub const RGBA_SIZE: usize = 512;
    pub const SUN_SIZE: usize = 1024;
}

/// Lump 2 and lump 3: the light grid's records, shared with the engine's
/// [`vcod_common::mapworld::LightGrid`].
pub use vcod_common::mapworld::{LightGridColour, LightGridEntry};

/// Lump 4.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Plane {
    pub normal: Vec3,
    pub dist: f32,
}

/// Lump 6. Sides are stored brush after brush; `first_side` is derived.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Brush {
    pub num_sides: i16,
    pub material: i16,
    /// Not in the file: the running sum of the earlier brushes' sides.
    pub first_side: u32,
}

/// Lump 5. A brush's first six sides carry an `f32` bound in
/// `plane_or_bound` (x min, x max, y min, y max, z min, z max); the rest a
/// plane index.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BrushSide {
    pub plane_or_bound: u32,
    pub material: i32,
}

impl BrushSide {
    pub fn bound(&self) -> f32 {
        f32::from_bits(self.plane_or_bound)
    }
}

/// Lump 7.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DrawSurface {
    pub material: i16,
    /// Page into lump 1; [`NO_LIGHTMAP`] for none.
    pub lightmap: i16,
    pub first_vertex: i32,
    pub vertex_count: u16,
    pub index_count: u16,
    pub first_index: i32,
}

/// Lump 8.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DrawVert {
    pub xyz: Vec3,
    pub normal: Vec3,
    /// R, G, B, A (INFERRED in the doc).
    pub color: [u8; 4],
    pub uv: Vec2,
    pub lm_uv: Vec2,
    /// File +0x2c, along dP/du.
    pub tangent: Vec3,
    /// File +0x38, along dP/dv.
    pub binormal: Vec3,
}

/// Lump 10.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CullGroup {
    pub mins: Vec3,
    pub maxs: Vec3,
    pub first_surface: i32,
    pub surface_count: i32,
}

/// Lump 18.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Occluder {
    pub first_plane: u32,
    pub num_planes: u16,
    pub num_edges: u16,
    pub first_edge: u32,
    pub first_vert: u32,
    pub vert_count: u16,
    pub pad: u16,
}

/// Lump 20.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OccluderEdge {
    pub plane0: u8,
    pub plane1: u8,
    pub vert0: u8,
    pub vert1: u8,
}

/// Lump 22, a render AABB tree node.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AabbTree {
    pub first_surface: i32,
    pub surface_count: i32,
    pub child_count: i32,
}

/// Lump 23.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Cell {
    pub mins: Vec3,
    pub maxs: Vec3,
    pub first_aabb_tree: i32,
    pub first_portal: i32,
    pub portal_count: i32,
    pub first_cull_group_index: i32,
    pub cull_group_count: i32,
    pub first_occluder: i32,
    pub occluder_count: i32,
}

/// Lump 24.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Portal {
    pub plane: i32,
    pub cell: i32,
    pub first_vert: i32,
    pub vert_count: i32,
}

/// Lump 25. A negative child `c` is leaf `-(c + 1)`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Node {
    pub plane: i32,
    pub children: [i32; 2],
    pub mins: [i32; 3],
    pub maxs: [i32; 3],
}

/// Lump 26.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Leaf {
    /// -1 in solid.
    pub cluster: i32,
    pub area: i32,
    /// Into lump 34.
    pub first_coll_aabb: i32,
    pub coll_aabb_count: i32,
    /// Into lump 27.
    pub first_leaf_brush: i32,
    pub leaf_brush_count: i32,
    /// Into lump 23; -1 in solid.
    pub cell: i32,
    pub pad7: i32,
    pub pad8: i32,
}

/// Lump 29.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CollVert {
    pub unused: u32,
    pub xyz: Vec3,
}

/// Lump 30.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CollEdge {
    pub unused: u32,
    pub origin: Vec3,
    pub axis: [Vec3; 2],
    pub dir: Vec3,
    pub length: f32,
}

/// Lump 35.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Model {
    pub mins: Vec3,
    pub maxs: Vec3,
    pub first_surface: i32,
    pub surface_count: i32,
    pub first_coll_aabb: i32,
    pub coll_aabb_count: i32,
    pub first_brush: i32,
    pub brush_count: i32,
}

/// Lump 36.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Visibility {
    pub cluster_count: i32,
    pub row_bytes: i32,
    /// `cluster_count` rows of `row_bytes`.
    pub rows: Vec<u8>,
}

impl Visibility {
    /// Whether cluster `to` is in `from`'s PVS row; out of range reads false.
    pub fn sees(&self, from: usize, to: usize) -> bool {
        let row = from * self.row_bytes as usize;
        self.rows
            .get(row + to / 8)
            .filter(|_| to / 8 < self.row_bytes as usize)
            .is_some_and(|b| b & (1 << (to % 8)) != 0)
    }
}

/// A parsed `.d3dbsp`. Every index field is in range once `parse` returns.
#[derive(Clone, Debug)]
pub struct D3dBsp {
    pub header: Header,
    pub materials: Vec<Material>,
    pub lightmaps: Vec<LightmapPage>,
    pub light_grid: Vec<LightGridEntry>,
    pub light_grid_colours: Vec<LightGridColour>,
    pub planes: Vec<Plane>,
    pub brush_sides: Vec<BrushSide>,
    pub brushes: Vec<Brush>,
    pub surfaces: Vec<DrawSurface>,
    pub verts: Vec<DrawVert>,
    pub indices: Vec<u16>,
    pub cull_groups: Vec<CullGroup>,
    pub cull_group_indices: Vec<i32>,
    pub portal_verts: Vec<Vec3>,
    pub occluders: Vec<Occluder>,
    pub occluder_planes: Vec<u32>,
    pub occluder_edges: Vec<OccluderEdge>,
    pub occluder_indices: Vec<i16>,
    pub aabb_trees: Vec<AabbTree>,
    pub cells: Vec<Cell>,
    pub portals: Vec<Portal>,
    pub nodes: Vec<Node>,
    pub leafs: Vec<Leaf>,
    pub leaf_brushes: Vec<i32>,
    pub leaf_surfaces: Vec<i32>,
    pub coll_verts: Vec<CollVert>,
    pub coll_edges: Vec<CollEdge>,
    pub coll_tris: Vec<CollTri>,
    pub coll_borders: Vec<CollBorder>,
    pub coll_partitions: Vec<CollPartition>,
    pub coll_aabb_trees: Vec<CollAabbTree>,
    pub models: Vec<Model>,
    /// `None` when lump 36 is empty.
    pub visibility: Option<Visibility>,
    /// Lump 37 without its trailing NULs.
    pub entities: String,
}

/// Little-endian field reads off one record.
struct Rec<'a>(&'a [u8]);

impl Rec<'_> {
    fn u8(&self, o: usize) -> u8 {
        self.0[o]
    }
    fn u16(&self, o: usize) -> u16 {
        u16::from_le_bytes([self.0[o], self.0[o + 1]])
    }
    fn i16(&self, o: usize) -> i16 {
        self.u16(o) as i16
    }
    fn u32(&self, o: usize) -> u32 {
        u32::from_le_bytes(self.0[o..o + 4].try_into().unwrap())
    }
    fn i32(&self, o: usize) -> i32 {
        self.u32(o) as i32
    }
    fn f32(&self, o: usize) -> f32 {
        f32::from_bits(self.u32(o))
    }
    fn vec2(&self, o: usize) -> Vec2 {
        Vec2::new(self.f32(o), self.f32(o + 4))
    }
    fn vec3(&self, o: usize) -> Vec3 {
        Vec3::new(self.f32(o), self.f32(o + 4), self.f32(o + 8))
    }
    fn f32s<const N: usize>(&self, o: usize) -> [f32; N] {
        std::array::from_fn(|k| self.f32(o + 4 * k))
    }
    fn i32s<const N: usize>(&self, o: usize) -> [i32; N] {
        std::array::from_fn(|k| self.i32(o + 4 * k))
    }
}

/// The file and its directory, for slicing lumps.
struct File<'a> {
    data: &'a [u8],
    header: &'a Header,
}

impl File<'_> {
    fn bytes(&self, i: usize) -> &[u8] {
        // `Header::parse` checked every non-empty lump against the file.
        let l = self.header.lumps[i];
        if l.length == 0 {
            return &[];
        }
        &self.data[l.offset as usize..][..l.length as usize]
    }

    /// Lump `i` as `size`-byte records, each decoded by `f`.
    fn records<T>(&self, i: usize, size: usize, f: impl Fn(Rec) -> T) -> Result<Vec<T>> {
        let b = self.bytes(i);
        ensure!(
            b.len().is_multiple_of(size),
            "lump {i}: funny lump size {} (record {size})",
            b.len()
        );
        Ok(b.chunks_exact(size).map(|r| f(Rec(r))).collect())
    }
}

/// `first .. first + count` within `len`, both non-negative.
fn run_ok(first: i64, count: i64, len: usize) -> bool {
    first >= 0 && count >= 0 && first + count <= len as i64
}

fn index_ok(i: i64, len: usize) -> bool {
    i >= 0 && i < len as i64
}

/// `-1` or in range.
fn opt_index_ok(i: i64, len: usize) -> bool {
    i == -1 || index_ok(i, len)
}

impl Header {
    /// `Com_LoadBsp`'s checks, plus the census's: every non-empty lump lies
    /// past the header and inside the file.
    pub fn parse(data: &[u8]) -> Result<Self> {
        ensure!(
            data.len() >= HEADER_BYTES,
            "d3dbsp: {} bytes, shorter than the {HEADER_BYTES}-byte header",
            data.len()
        );
        ensure!(&data[..4] == MAGIC, "d3dbsp: magic is not IBSP");
        let r = Rec(data);
        let version = r.u32(4);
        ensure!(
            version == VERSION,
            "d3dbsp: version {version}, want {VERSION}"
        );
        let lumps: [LumpEntry; LUMP_COUNT] = std::array::from_fn(|i| LumpEntry {
            length: r.u32(8 + 8 * i),
            offset: r.u32(12 + 8 * i),
        });
        for (i, l) in lumps.iter().enumerate() {
            if l.length == 0 {
                continue;
            }
            ensure!(
                l.offset as usize >= HEADER_BYTES,
                "lump {i}: funny lump offset {}",
                l.offset
            );
            ensure!(
                l.offset as u64 + l.length as u64 <= data.len() as u64,
                "lump {i}: lump extends past end of file"
            );
        }
        Ok(Self { version, lumps })
    }
}

impl D3dBsp {
    /// Decodes and validates a whole file.
    pub fn parse(data: &[u8]) -> Result<Self> {
        let header = Header::parse(data)?;
        let file = File {
            data,
            header: &header,
        };
        let f = &file;
        use lump::*;

        let materials = f.records(MATERIALS, 72, |r| {
            let name = &r.0[..64];
            let end = name.iter().position(|&b| b == 0).unwrap_or(64);
            Material {
                name: String::from_utf8_lossy(&name[..end]).into_owned(),
                surface_flags: r.u32(0x40),
                content_flags: r.u32(0x44),
            }
        })?;
        let lm = f.bytes(LIGHTMAPS);
        ensure!(
            lm.len().is_multiple_of(LIGHTMAP_PAGE_BYTES),
            "lump 1: funny lump size {} (page {LIGHTMAP_PAGE_BYTES})",
            lm.len()
        );
        let lightmaps = lm
            .as_chunks::<LIGHTMAP_PAGE_BYTES>()
            .0
            .iter()
            .map(|p| LightmapPage {
                rgba: std::array::from_fn(|k| p[k * 0x10_0000..(k + 1) * 0x10_0000].to_vec()),
                sun: p[0x30_0000..].to_vec(),
            })
            .collect();
        let light_grid = f.records(LIGHT_GRID_ENTRIES, 8, |r| LightGridEntry {
            key: r.u32(0),
            low: r.u8(4),
            occlusion_mask: r.u8(5),
            colour: r.u16(6),
        })?;
        let light_grid_colours = f.records(LIGHT_GRID_COLOURS, 24, |r| LightGridColour {
            rgb: std::array::from_fn(|c| r.0[c * 8..c * 8 + 8].try_into().unwrap()),
        })?;
        let planes = f.records(PLANES, 16, |r| Plane {
            normal: r.vec3(0),
            dist: r.f32(12),
        })?;
        let brush_sides = f.records(BRUSH_SIDES, 8, |r| BrushSide {
            plane_or_bound: r.u32(0),
            material: r.i32(4),
        })?;
        let mut brushes = f.records(BRUSHES, 4, |r| Brush {
            num_sides: r.i16(0),
            material: r.i16(2),
            first_side: 0,
        })?;
        let mut side = 0u32;
        for b in &mut brushes {
            b.first_side = side;
            side = side.saturating_add(b.num_sides.max(0) as u32);
        }
        let surfaces = f.records(SURFACES, 16, |r| DrawSurface {
            material: r.i16(0),
            lightmap: r.i16(2),
            first_vertex: r.i32(4),
            vertex_count: r.u16(8),
            index_count: r.u16(10),
            first_index: r.i32(12),
        })?;
        let verts = f.records(VERTS, 68, |r| DrawVert {
            xyz: r.vec3(0),
            normal: r.vec3(0x0c),
            color: r.0[0x18..0x1c].try_into().unwrap(),
            uv: r.vec2(0x1c),
            lm_uv: r.vec2(0x24),
            tangent: r.vec3(0x2c),
            binormal: r.vec3(0x38),
        })?;
        let indices = f.records(INDICES, 2, |r| r.u16(0))?;
        let cull_groups = f.records(CULL_GROUPS, 32, |r| CullGroup {
            mins: r.vec3(0),
            maxs: r.vec3(12),
            first_surface: r.i32(24),
            surface_count: r.i32(28),
        })?;
        let cull_group_indices = f.records(CULL_GROUP_INDICES, 4, |r| r.i32(0))?;
        let portal_verts = f.records(PORTAL_VERTS, 12, |r| r.vec3(0))?;
        let occluders = f.records(OCCLUDERS, 20, |r| Occluder {
            first_plane: r.u32(0),
            num_planes: r.u16(4),
            num_edges: r.u16(6),
            first_edge: r.u32(8),
            first_vert: r.u32(12),
            vert_count: r.u16(16),
            pad: r.u16(18),
        })?;
        let occluder_planes = f.records(OCCLUDER_PLANES, 4, |r| r.u32(0))?;
        let occluder_edges = f.records(OCCLUDER_EDGES, 4, |r| OccluderEdge {
            plane0: r.u8(0),
            plane1: r.u8(1),
            vert0: r.u8(2),
            vert1: r.u8(3),
        })?;
        let occluder_indices = f.records(OCCLUDER_INDICES, 2, |r| r.i16(0))?;
        let aabb_trees = f.records(AABB_TREES, 12, |r| AabbTree {
            first_surface: r.i32(0),
            surface_count: r.i32(4),
            child_count: r.i32(8),
        })?;
        let cells = f.records(CELLS, 52, |r| {
            let w: [i32; 7] = r.i32s(24);
            Cell {
                mins: r.vec3(0),
                maxs: r.vec3(12),
                first_aabb_tree: w[0],
                first_portal: w[1],
                portal_count: w[2],
                first_cull_group_index: w[3],
                cull_group_count: w[4],
                first_occluder: w[5],
                occluder_count: w[6],
            }
        })?;
        let portals = f.records(PORTALS, 16, |r| {
            let w: [i32; 4] = r.i32s(0);
            Portal {
                plane: w[0],
                cell: w[1],
                first_vert: w[2],
                vert_count: w[3],
            }
        })?;
        let nodes = f.records(NODES, 36, |r| Node {
            plane: r.i32(0),
            children: r.i32s(4),
            mins: r.i32s(12),
            maxs: r.i32s(24),
        })?;
        let leafs = f.records(LEAFS, 36, |r| {
            let w: [i32; 9] = r.i32s(0);
            Leaf {
                cluster: w[0],
                area: w[1],
                first_coll_aabb: w[2],
                coll_aabb_count: w[3],
                first_leaf_brush: w[4],
                leaf_brush_count: w[5],
                cell: w[6],
                pad7: w[7],
                pad8: w[8],
            }
        })?;
        let leaf_brushes = f.records(LEAF_BRUSHES, 4, |r| r.i32(0))?;
        let leaf_surfaces = f.records(LEAF_SURFACES, 4, |r| r.i32(0))?;
        let coll_verts = f.records(COLL_VERTS, 16, |r| CollVert {
            unused: r.u32(0),
            xyz: r.vec3(4),
        })?;
        let coll_edges = f.records(COLL_EDGES, 56, |r| CollEdge {
            unused: r.u32(0),
            origin: r.vec3(4),
            axis: [r.vec3(16), r.vec3(28)],
            dir: r.vec3(40),
            length: r.f32(52),
        })?;
        let coll_tris = f.records(COLL_TRIS, 72, |r| CollTri {
            plane: r.f32s(0),
            svec: r.f32s(16),
            tvec: r.f32s(32),
            verts: r.i32s(48),
            edges: r.i32s(60),
        })?;
        let coll_borders = f.records(COLL_BORDERS, 28, |r| {
            let w: [f32; 7] = r.f32s(0);
            CollBorder {
                dist_eq: [w[0], w[1], w[2]],
                z_base: w[3],
                z_slope: w[4],
                start: w[5],
                length: w[6],
            }
        })?;
        let coll_partitions = f.records(COLL_PARTITIONS, 12, |r| CollPartition {
            unused: r.u16(0),
            tri_count: r.u8(2),
            border_count: r.u8(3),
            first_tri: r.i32(4),
            first_border: r.i32(8),
        })?;
        let coll_aabb_trees = f.records(COLL_AABB_TREES, 32, |r| CollAabbTree {
            origin: r.vec3(0),
            half_size: r.vec3(12),
            material: r.u16(24),
            child_count: r.u16(26),
            child_or_partition: r.i32(28),
        })?;
        let models = f.records(MODELS, 48, |r| {
            let w: [i32; 6] = r.i32s(24);
            Model {
                mins: r.vec3(0),
                maxs: r.vec3(12),
                first_surface: w[0],
                surface_count: w[1],
                first_coll_aabb: w[2],
                coll_aabb_count: w[3],
                first_brush: w[4],
                brush_count: w[5],
            }
        })?;
        let vis = f.bytes(VISIBILITY);
        let visibility = if vis.is_empty() {
            None
        } else {
            ensure!(vis.len() >= 8, "lump 36: {} bytes, no header", vis.len());
            let r = Rec(vis);
            let (cluster_count, row_bytes) = (r.i32(0), r.i32(4));
            ensure!(
                cluster_count >= 0
                    && row_bytes >= 0
                    && vis.len() as i64 == 8 + cluster_count as i64 * row_bytes as i64,
                "lump 36: {} bytes for {cluster_count} clusters of {row_bytes}",
                vis.len()
            );
            Some(Visibility {
                cluster_count,
                row_bytes,
                rows: vis[8..].to_vec(),
            })
        };
        let ents = f.bytes(ENTITIES);
        let end = ents.iter().rposition(|&b| b != 0).map_or(0, |p| p + 1);
        let entities = String::from_utf8_lossy(&ents[..end]).into_owned();

        let bsp = Self {
            header,
            materials,
            lightmaps,
            light_grid,
            light_grid_colours,
            planes,
            brush_sides,
            brushes,
            surfaces,
            verts,
            indices,
            cull_groups,
            cull_group_indices,
            portal_verts,
            occluders,
            occluder_planes,
            occluder_edges,
            occluder_indices,
            aabb_trees,
            cells,
            portals,
            nodes,
            leafs,
            leaf_brushes,
            leaf_surfaces,
            coll_verts,
            coll_edges,
            coll_tris,
            coll_borders,
            coll_partitions,
            coll_aabb_trees,
            models,
            visibility,
            entities,
        };
        bsp.validate()?;
        Ok(bsp)
    }

    /// The loaders' and the census's index and cross-reference checks.
    fn validate(&self) -> Result<()> {
        let nm = self.materials.len();
        ensure!(nm > 0, "Map with no materials");
        ensure!(!self.planes.is_empty(), "Map with no planes");
        ensure!(!self.surfaces.is_empty(), "no surfaces");
        ensure!(!self.nodes.is_empty(), "Map has no nodes");
        ensure!(!self.leafs.is_empty(), "Map with no leafs");
        ensure!(!self.models.is_empty(), "Map with no models");
        ensure!(
            self.models.len() < MAX_SUBMODELS,
            "MAX_SUBMODELS exceeded: {}",
            self.models.len()
        );

        // Brushes and sides (CMod_LoadBrushes).
        ensure!(
            self.brushes.len() <= u16::MAX as usize,
            "cm.numBrushes exceeded: {}",
            self.brushes.len()
        );
        for (i, b) in self.brushes.iter().enumerate() {
            ensure!(b.num_sides >= 6, "brush {i} has less than 6 sides");
        }
        let sides: i64 = self.brushes.iter().map(|b| b.num_sides as i64).sum();
        ensure!(
            sides == self.brush_sides.len() as i64,
            "brush sides sum to {sides}, lump 5 holds {}",
            self.brush_sides.len()
        );
        for (i, b) in self.brushes.iter().enumerate() {
            ensure!(
                index_ok(b.material as i64, nm),
                "brush {i}: bad materialNum: {}",
                b.material
            );
            let s = &self.brush_sides[b.first_side as usize..][..b.num_sides as usize];
            for (k, side) in s.iter().enumerate() {
                ensure!(
                    index_ok(side.material as i64, nm),
                    "brush {i} side {k}: bad materialNum: {}",
                    side.material
                );
                if k < 6 {
                    ensure!(
                        side.material <= i16::MAX as i32,
                        "brush {i}: axialMaterialNum exceeded"
                    );
                } else {
                    ensure!(
                        (side.plane_or_bound as usize) < self.planes.len(),
                        "brush {i} side {k}: plane {} out of range",
                        side.plane_or_bound
                    );
                }
            }
            for a in 0..3 {
                ensure!(
                    s[2 * a].bound() <= s[2 * a + 1].bound(),
                    "brush {i}: axis {a} min above max"
                );
            }
        }

        // Draw surfaces (R_LoadSurfaces), lightmap pages.
        let mut lm_max = -1i32;
        for (i, s) in self.surfaces.iter().enumerate() {
            ensure!(
                index_ok(s.material as i64, nm),
                "surface {i}: material {} out of range",
                s.material
            );
            ensure!(
                s.lightmap == NO_LIGHTMAP || index_ok(s.lightmap as i64, self.lightmaps.len()),
                "surface {i}: lightmap {} out of range",
                s.lightmap
            );
            if s.lightmap != NO_LIGHTMAP {
                lm_max = lm_max.max(s.lightmap as i32);
            }
            ensure!(
                run_ok(
                    s.first_vertex as i64,
                    s.vertex_count as i64,
                    self.verts.len()
                ),
                "surface {i}: vertex run out of range"
            );
            ensure!(
                s.index_count % 3 == 0
                    && run_ok(
                        s.first_index as i64,
                        s.index_count as i64,
                        self.indices.len()
                    ),
                "surface {i}: index run out of range"
            );
            let idx = &self.indices[s.first_index as usize..][..s.index_count as usize];
            ensure!(
                idx.iter().all(|&x| x < s.vertex_count),
                "surface {i}: index past its vertex count"
            );
        }
        ensure!(
            (lm_max + 1) as usize == self.lightmaps.len(),
            "funny lump size: {} lightmap pages, surfaces use {}",
            self.lightmaps.len(),
            lm_max + 1
        );

        // Light grid.
        let ncol = self.light_grid_colours.len();
        for (i, g) in self.light_grid.iter().enumerate() {
            ensure!(
                (g.colour as usize) < ncol,
                "light grid entry {i}: colour {} out of range",
                g.colour
            );
        }
        ensure!(
            self.light_grid
                .windows(2)
                .all(|w| (w[0].key, w[0].low & 0xfe) <= (w[1].key, w[1].low & 0xfe)),
            "light grid entries not sorted by (key, low & 0xfe)"
        );

        // Cull groups.
        for (i, c) in self.cull_groups.iter().enumerate() {
            ensure!(
                run_ok(
                    c.first_surface as i64,
                    c.surface_count as i64,
                    self.surfaces.len()
                ),
                "cull group {i}: surface run out of range"
            );
        }
        for &x in &self.cull_group_indices {
            ensure!(
                index_ok(x as i64, self.cull_groups.len()),
                "cull group index {x} out of range"
            );
        }

        // Occluders.
        for (i, o) in self.occluders.iter().enumerate() {
            ensure!(
                run_ok(
                    o.first_plane as i64,
                    o.num_planes as i64,
                    self.occluder_planes.len()
                ) && run_ok(
                    o.first_edge as i64,
                    o.num_edges as i64,
                    self.occluder_edges.len()
                ) && run_ok(
                    o.first_vert as i64,
                    o.vert_count as i64,
                    self.portal_verts.len()
                ),
                "occluder {i}: run out of range"
            );
        }
        for &p in &self.occluder_planes {
            ensure!(
                (p as usize) < self.planes.len(),
                "occluder plane {p} out of range"
            );
        }
        for &x in &self.occluder_indices {
            ensure!(
                index_ok(x as i64, self.occluders.len()),
                "occluder index {x} out of range"
            );
        }

        // Render AABB trees, cells, portals.
        for (i, t) in self.aabb_trees.iter().enumerate() {
            ensure!(
                run_ok(
                    t.first_surface as i64,
                    t.surface_count as i64,
                    self.surfaces.len()
                ),
                "aabb tree {i}: surface run out of range"
            );
            ensure!(
                t.child_count >= 0,
                "aabb tree {i}: child count {}",
                t.child_count
            );
        }
        for (i, c) in self.cells.iter().enumerate() {
            ensure!(
                index_ok(c.first_aabb_tree as i64, self.aabb_trees.len()),
                "cell {i}: aabb tree {} out of range",
                c.first_aabb_tree
            );
            self.aabb_tree_size(c.first_aabb_tree as usize)?;
            ensure!(
                run_ok(
                    c.first_portal as i64,
                    c.portal_count as i64,
                    self.portals.len()
                ),
                "cell {i}: portal run out of range"
            );
            ensure!(
                run_ok(
                    c.first_cull_group_index as i64,
                    c.cull_group_count as i64,
                    self.cull_group_indices.len()
                ),
                "cell {i}: cull group run out of range"
            );
            ensure!(
                run_ok(
                    c.first_occluder as i64,
                    c.occluder_count as i64,
                    self.occluder_indices.len()
                ),
                "cell {i}: occluder run out of range"
            );
        }
        for (i, p) in self.portals.iter().enumerate() {
            ensure!(
                index_ok(p.plane as i64, self.planes.len())
                    && index_ok(p.cell as i64, self.cells.len())
                    && run_ok(
                        p.first_vert as i64,
                        p.vert_count as i64,
                        self.portal_verts.len()
                    ),
                "portal {i}: plane, cell or vertex run out of range"
            );
        }

        // BSP nodes and leafs.
        for (i, n) in self.nodes.iter().enumerate() {
            ensure!(
                index_ok(n.plane as i64, self.planes.len()),
                "node {i}: plane {} out of range",
                n.plane
            );
            for &c in &n.children {
                let ok = if c < 0 {
                    index_ok(-(c as i64 + 1), self.leafs.len())
                } else {
                    index_ok(c as i64, self.nodes.len())
                };
                ensure!(ok, "node {i}: child {c} out of range");
            }
        }
        let clusters = self.visibility.as_ref().map(|v| v.cluster_count as i64);
        for (i, l) in self.leafs.iter().enumerate() {
            ensure!(
                l.cluster >= -1 && clusters.is_none_or(|n| (l.cluster as i64) < n.max(1)),
                "leaf {i}: cluster {} out of range",
                l.cluster
            );
            ensure!(
                run_ok(
                    l.first_coll_aabb as i64,
                    l.coll_aabb_count as i64,
                    self.coll_aabb_trees.len()
                ),
                "leaf {i}: collision aabb run out of range"
            );
            ensure!(
                run_ok(
                    l.first_leaf_brush as i64,
                    l.leaf_brush_count as i64,
                    self.leaf_brushes.len()
                ),
                "leaf {i}: leaf brush run out of range"
            );
            ensure!(
                opt_index_ok(l.cell as i64, self.cells.len()),
                "leaf {i}: cell {} out of range",
                l.cell
            );
        }
        for &b in &self.leaf_brushes {
            ensure!(
                index_ok(b as i64, self.brushes.len()),
                "brushIndex exceeded: leaf brush {b}"
            );
        }

        // Collision geometry.
        for (i, t) in self.coll_tris.iter().enumerate() {
            ensure!(
                t.verts
                    .iter()
                    .all(|&v| opt_index_ok(v as i64, self.coll_verts.len())),
                "collision triangle {i}: vertex out of range"
            );
            ensure!(
                t.edges
                    .iter()
                    .all(|&e| opt_index_ok(e as i64, self.coll_edges.len())),
                "collision triangle {i}: edge out of range"
            );
        }
        for (i, p) in self.coll_partitions.iter().enumerate() {
            ensure!(
                p.tri_count > 0
                    && run_ok(p.first_tri as i64, p.tri_count as i64, self.coll_tris.len()),
                "collision partition {i}: triangle run out of range"
            );
            ensure!(
                run_ok(
                    p.first_border as i64,
                    p.border_count as i64,
                    self.coll_borders.len()
                ),
                "collision partition {i}: border run out of range"
            );
        }
        for (i, a) in self.coll_aabb_trees.iter().enumerate() {
            ensure!(
                (a.material as usize) < nm,
                "collision aabb {i}: material {} out of range",
                a.material
            );
            if a.child_count == 0 {
                ensure!(
                    index_ok(a.child_or_partition as i64, self.coll_partitions.len()),
                    "collision aabb {i}: partition {} out of range",
                    a.child_or_partition
                );
            } else {
                ensure!(
                    a.child_or_partition as i64 > i as i64
                        && run_ok(
                            a.child_or_partition as i64,
                            a.child_count as i64,
                            self.coll_aabb_trees.len()
                        ),
                    "collision aabb {i}: children out of range"
                );
            }
        }

        // Models.
        for (i, m) in self.models.iter().enumerate() {
            ensure!(
                run_ok(
                    m.first_surface as i64,
                    m.surface_count as i64,
                    self.surfaces.len()
                ),
                "model {i}: surface run out of range"
            );
            ensure!(
                run_ok(
                    m.first_coll_aabb as i64,
                    m.coll_aabb_count as i64,
                    self.coll_aabb_trees.len()
                ),
                "model {i}: collision aabb run out of range"
            );
            ensure!(
                run_ok(
                    m.first_brush as i64,
                    m.brush_count as i64,
                    self.brushes.len()
                ),
                "model {i}: brush run out of range"
            );
        }
        Ok(())
    }

    /// Nodes in the render AABB tree rooted at `root`, walked in preorder
    /// (children follow their parent back to back), or an error when the
    /// walk leaves lump 22.
    pub fn aabb_tree_size(&self, root: usize) -> Result<usize> {
        // Explicit stack: (node, children left to walk).
        let mut next = root;
        let mut pending = vec![1usize];
        while let Some(left) = pending.last_mut() {
            if *left == 0 {
                pending.pop();
                continue;
            }
            *left -= 1;
            let Some(t) = self.aabb_trees.get(next) else {
                bail!("aabb tree walk from {root} leaves lump 22 at {next}");
            };
            next += 1;
            // Each push follows a visited node, so the stack stays under
            // the lump's length.
            if t.child_count > 0 {
                pending.push(t.child_count as usize);
            }
        }
        Ok(next - root)
    }

    /// The bounds the axial sides give brush `i` and its runtime contents
    /// (`materials[material].content_flags & BRUSH_CONTENTS_MASK`).
    pub fn brush_box(&self, i: usize) -> BrushBox {
        let b = &self.brushes[i];
        let s = &self.brush_sides[b.first_side as usize..];
        BrushBox {
            mins: [s[0].bound(), s[2].bound(), s[4].bound()],
            maxs: [s[1].bound(), s[3].bound(), s[5].bound()],
            contents: self.materials[b.material as usize].content_flags & BRUSH_CONTENTS_MASK,
        }
    }

    /// Every brush's [`brush_box`](Self::brush_box), by brush number.
    pub fn brush_boxes(&self) -> Vec<BrushBox> {
        (0..self.brushes.len()).map(|i| self.brush_box(i)).collect()
    }

    /// `CMod_LoadBrushRelated`'s leaf pass (0x08053c84): every leaf's brush
    /// tree, built leaf by leaf on one shared copy of lump 27 as retail
    /// rewrites it in place. Returns that rewritten list (one spare slot at
    /// the end, as the loader allocates it) and each leaf's tree, `None` for
    /// a leaf with no brushes. A tree's [`BrushNode::Leaf`] runs index the
    /// returned list.
    pub fn leaf_brush_trees(&self) -> Result<(Vec<u16>, Vec<Option<BrushTree>>)> {
        let boxes = self.brush_boxes();
        let mut list: Vec<u16> = self.leaf_brushes.iter().map(|&b| b as u16).collect();
        list.push(0);
        let mut trees = Vec::with_capacity(self.leafs.len());
        for (i, l) in self.leafs.iter().enumerate() {
            if l.leaf_brush_count == 0 {
                trees.push(None);
                continue;
            }
            let tree = partition_brushes(
                &boxes,
                &mut list,
                l.first_leaf_brush as usize,
                l.leaf_brush_count as usize,
            )
            .map_err(|e| e.context(format!("leaf {i}")))?;
            trees.push(Some(tree));
        }
        Ok((list, trees))
    }

    /// `CMod_LoadSubmodelBrushNodes` (0x08053404): model `m`'s brush tree
    /// over its own `first_brush .. first_brush + brush_count`, with the
    /// list it rewrote. `None` for a model with no brushes.
    pub fn model_brush_tree(&self, m: usize) -> Result<Option<(Vec<u16>, BrushTree)>> {
        let model = &self.models[m];
        if model.brush_count == 0 {
            return Ok(None);
        }
        let mut list: Vec<u16> = (model.first_brush..model.first_brush + model.brush_count)
            .map(|b| b as u16)
            .collect();
        let tree = partition_brushes(
            &self.brush_boxes(),
            &mut list,
            0,
            model.brush_count as usize,
        )
        .map_err(|e| e.context(format!("model {m}")))?;
        Ok(Some((list, tree)))
    }
}

#[cfg(test)]
mod tests;
