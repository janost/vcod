//! IBSP 59 (CoD 1) loader into a [`MapWorld`]. Layouts:
//! docs/research/bsp-ibsp59-format.md.

use anyhow::{Result, anyhow, ensure};

/// The IBSP 59 records are the [`MapWorld`]'s; they stay reachable here.
pub use crate::mapworld::{
    AabbNode, Brush, BrushSide, Cell, CullGroup, DrawVert, LIGHTMAP_SIZE, Leaf, LeafLists,
    LinkClusters, MapWorld, Material, Model, NO_LIGHTMAP, Node, Occluder, Partition, PatchPart,
    Plane, Portal, Pvs, TerrainPart, TriangleSoup, Visibility, areas_connected,
};

const LUMP_COUNT: usize = 33;
const LUMP_MATERIALS: usize = 0;
const LUMP_LIGHTMAPS: usize = 1;
const LUMP_PLANES: usize = 2;
const LUMP_BRUSHSIDES: usize = 3;
const LUMP_BRUSHES: usize = 4;
const LUMP_SOUPS: usize = 6;
const LUMP_DRAWVERTS: usize = 7;
const LUMP_DRAWINDICES: usize = 8;
const LUMP_MODELS: usize = 27;
const LUMP_ENTITIES: usize = 29;
const LUMP_LIGHTS: usize = 30;
const LUMP_LIGHT_VIS: usize = 32;
const LUMP_CULL_GROUPS: usize = 9;
const LUMP_CULL_INDICES: usize = 10;
const LUMP_PORTAL_VERTS: usize = 11;
const LUMP_OCCLUDERS: usize = 12;
const LUMP_OCCLUDER_PLANES: usize = 13;
const LUMP_OCCLUDER_EDGES: usize = 14;
const LUMP_OCCLUDER_INDICES: usize = 15;
const LUMP_AABB_NODES: usize = 16;
const LUMP_CELLS: usize = 17;
const LUMP_PORTALS: usize = 18;
const LUMP_LIGHT_INDICES: usize = 19;
const LUMP_NODES: usize = 20;
const LUMP_LEAFS: usize = 21;
const LUMP_LEAF_BRUSHES: usize = 22;
const LUMP_LEAF_PARTITIONS: usize = 23;
const LUMP_PVS: usize = 28;
const LUMP_COLLISION_PARTS: usize = 24;
const LUMP_COLLISION_VERTS: usize = 25;
const LUMP_COLLISION_INDICES: usize = 26;

/// Lump 28: `i32 cluster_count, row_bytes` then one row per cluster. An empty
/// lump is a map with no PVS, which the format doc records for mp_neuville.
pub(crate) fn parse_pvs(b: &[u8]) -> Result<Option<Pvs>> {
    if b.is_empty() {
        return Ok(None);
    }
    ensure!(b.len() >= 8, "pvs lump too short for its header");
    let cluster_count = le_i32(b, 0);
    let row_bytes = le_i32(b, 4);
    ensure!(
        cluster_count >= 0 && row_bytes >= 0,
        "pvs header is negative: {cluster_count} clusters, {row_bytes} bytes a row"
    );
    let (cluster_count, row_bytes) = (cluster_count as usize, row_bytes as usize);
    let need = cluster_count * row_bytes;
    ensure!(
        b.len() >= 8 + need,
        "pvs lump holds {} bytes, {need} needed for {cluster_count} rows of {row_bytes}",
        b.len() - 8
    );
    ensure!(
        row_bytes * 8 >= cluster_count,
        "a {row_bytes}-byte row cannot address {cluster_count} clusters"
    );
    Ok(Pvs::new(cluster_count, row_bytes, b[8..8 + need].to_vec()))
}

fn lump<'a>(data: &'a [u8], dir: &[(u32, u32)], i: usize) -> Result<&'a [u8]> {
    let (len, off) = (dir[i].0 as usize, dir[i].1 as usize);
    ensure!(off + len <= data.len(), "lump {i} out of bounds");
    Ok(&data[off..off + len])
}

fn le_u32(b: &[u8], off: usize) -> u32 {
    u32::from_le_bytes(b[off..off + 4].try_into().unwrap())
}

fn le_i32(b: &[u8], off: usize) -> i32 {
    i32::from_le_bytes(b[off..off + 4].try_into().unwrap())
}

fn le_f32(b: &[u8], off: usize) -> f32 {
    f32::from_le_bytes(b[off..off + 4].try_into().unwrap())
}

fn le_vec3(b: &[u8], off: usize) -> [f32; 3] {
    [le_f32(b, off), le_f32(b, off + 4), le_f32(b, off + 8)]
}

/// Fixed-size records; a lump length that is not a multiple is an error.
fn records<T>(lump: &[u8], size: usize, name: &str, f: impl Fn(&[u8]) -> T) -> Result<Vec<T>> {
    ensure!(
        lump.len().is_multiple_of(size),
        "{name} lump size {} not divisible by {size}",
        lump.len()
    );
    Ok(lump.chunks_exact(size).map(f).collect())
}

pub fn parse(data: &[u8]) -> Result<MapWorld> {
    ensure!(
        data.len() >= 8 + LUMP_COUNT * 8,
        "file too small to be a BSP"
    );
    ensure!(
        &data[..4] == b"IBSP",
        "not an IBSP file (magic {:?})",
        &data[..4.min(data.len())]
    );
    let version = u32::from_le_bytes(data[4..8].try_into().unwrap());
    ensure!(
        version == 59,
        "unsupported BSP version {version}, expected 59 (CoD1/UO)"
    );

    let dir: Vec<(u32, u32)> = (0..LUMP_COUNT)
        .map(|i| {
            let b = &data[8 + i * 8..16 + i * 8];
            (
                u32::from_le_bytes(b[0..4].try_into().unwrap()),
                u32::from_le_bytes(b[4..8].try_into().unwrap()),
            )
        })
        .collect();

    let mats = lump(data, &dir, LUMP_MATERIALS)?;
    ensure!(
        mats.len() % 72 == 0,
        "materials lump size {} not divisible by 72",
        mats.len()
    );
    let materials: Vec<Material> = mats
        .as_chunks::<72>()
        .0
        .iter()
        .map(|c| {
            let name_end = c[..64].iter().position(|&b| b == 0).unwrap_or(64);
            Material {
                // some materials use '\' (mp_carride's curtains)
                name: String::from_utf8_lossy(&c[..name_end]).replace('\\', "/"),
                surface_flags: u32::from_le_bytes(c[64..68].try_into().unwrap()),
                content_flags: u32::from_le_bytes(c[68..72].try_into().unwrap()),
            }
        })
        .collect();

    let lm = lump(data, &dir, LUMP_LIGHTMAPS)?;
    const PAGE: usize = LIGHTMAP_SIZE * LIGHTMAP_SIZE * 3;
    ensure!(
        lm.len() % PAGE == 0,
        "lightmap lump size {} not divisible by {PAGE}",
        lm.len()
    );
    let lightmaps = lm
        .as_chunks::<PAGE>()
        .0
        .iter()
        .map(|c| c.to_vec())
        .collect();

    let soup_raw = lump(data, &dir, LUMP_SOUPS)?;
    ensure!(
        soup_raw.len() % 16 == 0,
        "triangle soup lump size {} not divisible by 16",
        soup_raw.len()
    );
    let soups: Vec<TriangleSoup> = soup_raw
        .as_chunks::<16>()
        .0
        .iter()
        .map(|c| TriangleSoup {
            material: u16::from_le_bytes(c[0..2].try_into().unwrap()),
            lightmap: u16::from_le_bytes(c[2..4].try_into().unwrap()),
            first_vertex: u32::from_le_bytes(c[4..8].try_into().unwrap()),
            vertex_count: u16::from_le_bytes(c[8..10].try_into().unwrap()),
            index_count: u16::from_le_bytes(c[10..12].try_into().unwrap()),
            first_index: u32::from_le_bytes(c[12..16].try_into().unwrap()),
        })
        .collect();

    let pl = lump(data, &dir, LUMP_PLANES)?;
    ensure!(
        pl.len() % 16 == 0,
        "plane lump size {} not divisible by 16",
        pl.len()
    );
    let planes: Vec<Plane> = pl
        .as_chunks::<16>()
        .0
        .iter()
        .map(|c| Plane {
            normal: [
                f32::from_le_bytes(c[0..4].try_into().unwrap()),
                f32::from_le_bytes(c[4..8].try_into().unwrap()),
                f32::from_le_bytes(c[8..12].try_into().unwrap()),
            ],
            dist: f32::from_le_bytes(c[12..16].try_into().unwrap()),
        })
        .collect();

    let bs = lump(data, &dir, LUMP_BRUSHSIDES)?;
    ensure!(
        bs.len() % 8 == 0,
        "brushside lump size {} not divisible by 8",
        bs.len()
    );
    let brush_sides: Vec<BrushSide> = bs
        .as_chunks::<8>()
        .0
        .iter()
        .map(|c| BrushSide {
            plane_or_dist: u32::from_le_bytes(c[0..4].try_into().unwrap()),
            material: u32::from_le_bytes(c[4..8].try_into().unwrap()),
        })
        .collect();

    let br = lump(data, &dir, LUMP_BRUSHES)?;
    ensure!(
        br.len() % 4 == 0,
        "brush lump size {} not divisible by 4",
        br.len()
    );
    let mut first_side = 0u32;
    let mut brushes: Vec<Brush> = Vec::with_capacity(br.len() / 4);
    for c in br.as_chunks::<4>().0 {
        let b = Brush {
            first_side,
            num_sides: u16::from_le_bytes(c[0..2].try_into().unwrap()),
            material: u16::from_le_bytes(c[2..4].try_into().unwrap()),
        };
        first_side = first_side
            .checked_add(b.num_sides as u32)
            .ok_or_else(|| anyhow!("brush side total overflows u32"))?;
        brushes.push(b);
    }
    ensure!(
        first_side as usize == brush_sides.len(),
        "brush side counts ({first_side}) do not cover the brushside lump ({})",
        brush_sides.len()
    );

    let mo = lump(data, &dir, LUMP_MODELS)?;
    ensure!(
        mo.len() % 48 == 0,
        "model lump size {} not divisible by 48",
        mo.len()
    );
    let models: Vec<Model> = mo
        .as_chunks::<48>()
        .0
        .iter()
        .map(|c| {
            let f = |o: usize| f32::from_le_bytes(c[o..o + 4].try_into().unwrap());
            let u = |o: usize| u32::from_le_bytes(c[o..o + 4].try_into().unwrap());
            Model {
                mins: [f(0), f(4), f(8)],
                maxs: [f(12), f(16), f(20)],
                first_soup: u(24),
                num_soups: u(28),
                partitions: (u(32), u(36)),
                first_brush: u(40),
                num_brushes: u(44),
            }
        })
        .collect();

    let vert_raw = lump(data, &dir, LUMP_DRAWVERTS)?;
    ensure!(
        vert_raw.len() % 44 == 0,
        "drawvert lump size {} not divisible by 44",
        vert_raw.len()
    );
    let verts: Vec<DrawVert> = vert_raw
        .as_chunks::<44>()
        .0
        .iter()
        .map(|c| DrawVert {
            pos: [
                f32::from_le_bytes(c[0..4].try_into().unwrap()),
                f32::from_le_bytes(c[4..8].try_into().unwrap()),
                f32::from_le_bytes(c[8..12].try_into().unwrap()),
            ],
            uv: [
                f32::from_le_bytes(c[12..16].try_into().unwrap()),
                f32::from_le_bytes(c[16..20].try_into().unwrap()),
            ],
            lm_uv: [
                f32::from_le_bytes(c[20..24].try_into().unwrap()),
                f32::from_le_bytes(c[24..28].try_into().unwrap()),
            ],
            normal: [
                f32::from_le_bytes(c[28..32].try_into().unwrap()),
                f32::from_le_bytes(c[32..36].try_into().unwrap()),
                f32::from_le_bytes(c[36..40].try_into().unwrap()),
            ],
            color: [c[40], c[41], c[42], c[43]],
        })
        .collect();

    let idx_raw = lump(data, &dir, LUMP_DRAWINDICES)?;
    ensure!(idx_raw.len() % 2 == 0, "index lump has odd size");
    let indices: Vec<u16> = idx_raw
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| u16::from_le_bytes(*c))
        .collect();

    for (i, s) in soups.iter().enumerate() {
        ensure!(
            (s.material as usize) < materials.len(),
            "soup {i} references material {} out of range",
            s.material
        );
        ensure!(
            s.first_vertex as usize + s.vertex_count as usize <= verts.len(),
            "soup {i} vertex range out of bounds"
        );
        ensure!(
            s.first_index as usize + s.index_count as usize <= indices.len(),
            "soup {i} index range out of bounds"
        );
        // indices are relative to first_vertex; collision indexes verts by them unchecked
        ensure!(
            indices[s.first_index as usize..][..s.index_count as usize]
                .iter()
                .all(|&v| v < s.vertex_count),
            "soup {i} has an index past its {} vertices",
            s.vertex_count
        );
    }

    ensure!(!models.is_empty(), "BSP has no models");
    for (i, m) in models.iter().enumerate() {
        ensure!(
            m.first_brush as usize + m.num_brushes as usize <= brushes.len(),
            "model {i} brush range out of bounds"
        );
        ensure!(
            m.first_soup as usize + m.num_soups as usize <= soups.len(),
            "model {i} triangle soup range out of bounds"
        );
    }
    for (i, b) in brushes.iter().enumerate() {
        ensure!(b.num_sides >= 6, "brush {i} has fewer than 6 sides");
        ensure!(
            (b.material as usize) < materials.len(),
            "brush {i} material out of range"
        );
        let sides = &brush_sides[b.first_side as usize..][..b.num_sides as usize];
        for s in &sides[6..] {
            ensure!(
                (s.plane_or_dist as usize) < planes.len(),
                "brush {i} references plane out of range"
            );
        }
    }

    let entities = String::from_utf8_lossy(lump(data, &dir, LUMP_ENTITIES)?)
        .trim_end_matches('\0')
        .to_string();

    let cull_groups = records(
        lump(data, &dir, LUMP_CULL_GROUPS)?,
        32,
        "cull groups",
        |b| CullGroup {
            mins: le_vec3(b, 0),
            maxs: le_vec3(b, 12),
            first_soup: le_u32(b, 24),
            soup_count: le_u32(b, 28),
        },
    )?;
    let cull_indices = records(
        lump(data, &dir, LUMP_CULL_INDICES)?,
        4,
        "cull indices",
        |b| le_u32(b, 0),
    )?;
    let portal_verts = records(
        lump(data, &dir, LUMP_PORTAL_VERTS)?,
        12,
        "portal verts",
        |b| le_vec3(b, 0),
    )?;
    let aabb_nodes = records(lump(data, &dir, LUMP_AABB_NODES)?, 12, "aabb nodes", |b| {
        AabbNode {
            first_soup: le_u32(b, 0),
            soup_count: le_u32(b, 4),
            child_count: le_u32(b, 8),
        }
    })?;
    let cells = records(lump(data, &dir, LUMP_CELLS)?, 52, "cells", |b| Cell {
        mins: le_vec3(b, 0),
        maxs: le_vec3(b, 12),
        first_aabb: le_u32(b, 24),
        first_portal: le_u32(b, 28),
        portal_count: le_u32(b, 32),
        first_cull_index: le_u32(b, 36),
        cull_count: le_u32(b, 40),
        first_occluder: le_u32(b, 44),
        occluder_count: le_u32(b, 48),
    })?;
    let occluders = records(lump(data, &dir, LUMP_OCCLUDERS)?, 20, "occluders", |b| {
        Occluder {
            first_plane: le_u32(b, 0),
            num_planes: u16::from_le_bytes([b[4], b[5]]),
            num_edges: u16::from_le_bytes([b[6], b[7]]),
            first_edge: le_u32(b, 8),
            first_vert: le_u32(b, 12),
            vert_count: u16::from_le_bytes([b[16], b[17]]),
        }
    })?;
    let occluder_plane_indices = records(
        lump(data, &dir, LUMP_OCCLUDER_PLANES)?,
        4,
        "occluder planes",
        |b| le_u32(b, 0),
    )?;
    let occluder_edges = records(
        lump(data, &dir, LUMP_OCCLUDER_EDGES)?,
        4,
        "occluder edges",
        |b| [b[0], b[1], b[2], b[3]],
    )?;
    let occluder_indices = records(
        lump(data, &dir, LUMP_OCCLUDER_INDICES)?,
        2,
        "occluder indices",
        |b| u16::from_le_bytes([b[0], b[1]]),
    )?;
    let portals = records(lump(data, &dir, LUMP_PORTALS)?, 16, "portals", |b| Portal {
        plane: le_u32(b, 0),
        cell: le_u32(b, 4),
        first_vert: le_u32(b, 8),
        vert_count: le_u32(b, 12),
    })?;
    let nodes = records(lump(data, &dir, LUMP_NODES)?, 36, "nodes", |b| Node {
        plane: le_u32(b, 0),
        children: [le_i32(b, 4), le_i32(b, 8)],
        mins: [le_i32(b, 12), le_i32(b, 16), le_i32(b, 20)],
        maxs: [le_i32(b, 24), le_i32(b, 28), le_i32(b, 32)],
    })?;
    let leafs = records(lump(data, &dir, LUMP_LEAFS)?, 36, "leafs", |b| Leaf {
        cluster: le_i32(b, 0),
        area: le_i32(b, 4),
        cell: le_i32(b, 24),
    })?;
    let leaf_lights = lump(data, &dir, LUMP_LEAFS)?
        .as_chunks::<36>()
        .0
        .iter()
        .map(|b| (le_u32(b, 28), le_u32(b, 32)))
        .collect();
    let pvs = parse_pvs(lump(data, &dir, LUMP_PVS)?)?;
    // Lighting lumps parse leniently: a bad one costs prop lighting, not
    // the map.
    let light_indices = lump(data, &dir, LUMP_LIGHT_INDICES)?
        .as_chunks::<2>()
        .0
        .iter()
        .map(|b| i16::from_le_bytes(*b))
        .collect();
    let lights = lump(data, &dir, LUMP_LIGHTS)?
        .as_chunks::<72>()
        .0
        .iter()
        .map(|b| std::array::from_fn(|i| le_u32(b, i * 4)))
        .collect();
    let light_vis = lump(data, &dir, LUMP_LIGHT_VIS)?.to_vec();
    let collision_verts = records(
        lump(data, &dir, LUMP_COLLISION_VERTS)?,
        12,
        "collision verts",
        |b| le_vec3(b, 0),
    )?;
    let collision_indices = records(
        lump(data, &dir, LUMP_COLLISION_INDICES)?,
        2,
        "collision indices",
        |b| u16::from_le_bytes([b[0], b[1]]),
    )?;
    let mut terrain = Vec::new();
    let mut patches = Vec::new();
    let mut kinds = Vec::new();
    for (i, b) in lump(data, &dir, LUMP_COLLISION_PARTS)?
        .as_chunks::<16>()
        .0
        .iter()
        .enumerate()
    {
        let material = u16::from_le_bytes([b[0], b[1]]);
        ensure!(
            (material as usize) < materials.len(),
            "collision partition {i} material out of range"
        );
        let (a, c) = (
            u16::from_le_bytes([b[4], b[5]]),
            u16::from_le_bytes([b[6], b[7]]),
        );
        if b[2] == 0 {
            let first_vert = le_u32(b, 12);
            ensure!(
                first_vert as usize + a as usize * c as usize <= collision_verts.len(),
                "patch partition {i} control points out of range"
            );
            kinds.push(Partition::Patch(patches.len() as u32));
            patches.push(PatchPart {
                material,
                width: a,
                height: c,
                tolerance: le_u32(b, 8) as i32,
                first_vert,
            });
        } else {
            let (first_vert, first_index) = (le_u32(b, 8), le_u32(b, 12));
            ensure!(
                first_vert as usize + a as usize <= collision_verts.len()
                    && first_index as usize + c as usize <= collision_indices.len()
                    && c % 3 == 0,
                "terrain partition {i} out of range"
            );
            for &ix in &collision_indices[first_index as usize..][..c as usize] {
                ensure!(ix < a, "terrain partition {i} index past its vertices");
            }
            kinds.push(Partition::Terrain(terrain.len() as u32));
            terrain.push(TerrainPart {
                material,
                first_vert,
                vert_count: a,
                first_index,
                index_count: c,
            });
        }
    }

    let leaf_lists: Vec<LeafLists> = lump(data, &dir, LUMP_LEAFS)?
        .as_chunks::<36>()
        .0
        .iter()
        .map(|b| LeafLists {
            brushes: (le_u32(b, 16), le_u32(b, 20)),
            partitions: (le_u32(b, 8), le_u32(b, 12)),
        })
        .collect();
    let leaf_brushes = records(
        lump(data, &dir, LUMP_LEAF_BRUSHES)?,
        4,
        "leaf brushes",
        |b| le_u32(b, 0),
    )?;
    let leaf_partitions = records(
        lump(data, &dir, LUMP_LEAF_PARTITIONS)?,
        4,
        "leaf partitions",
        |b| le_u32(b, 0),
    )?
    .into_iter()
    .enumerate()
    .map(|(i, r)| {
        kinds
            .get(r as usize)
            .copied()
            .ok_or_else(|| anyhow!("leaf partition {i} past lump 24"))
    })
    .collect::<Result<Vec<_>>>()?;
    let within = |(first, count): (u32, u32), len: usize| first as u64 + count as u64 <= len as u64;
    for (l, lists) in leaf_lists.iter().enumerate() {
        ensure!(
            within(lists.brushes, leaf_brushes.len())
                && within(lists.partitions, leaf_partitions.len()),
            "leaf {l} lists out of bounds"
        );
    }
    ensure!(
        leaf_brushes.iter().all(|&b| (b as usize) < brushes.len()),
        "a leaf brush past lump 4"
    );

    let soup_count = soups.len() as u64;
    let in_soups = |first: u32, count: u32| first as u64 + count as u64 <= soup_count;
    for (i, g) in cull_groups.iter().enumerate() {
        ensure!(
            in_soups(g.first_soup, g.soup_count),
            "cull group {i} soup range out of bounds"
        );
    }
    for (i, &c) in cull_indices.iter().enumerate() {
        ensure!(
            (c as usize) < cull_groups.len(),
            "cull index {i} out of bounds"
        );
    }
    for (i, n) in aabb_nodes.iter().enumerate() {
        ensure!(
            in_soups(n.first_soup, n.soup_count),
            "aabb node {i} soup range out of bounds"
        );
        ensure!(
            i as u64 + 1 + n.child_count as u64 <= aabb_nodes.len() as u64,
            "aabb node {i} child count out of bounds"
        );
    }
    for (i, p) in portals.iter().enumerate() {
        ensure!(
            (p.plane as usize) < planes.len(),
            "portal {i} plane out of bounds"
        );
        ensure!(
            (p.cell as usize) < cells.len(),
            "portal {i} cell out of bounds"
        );
        ensure!(
            p.first_vert as u64 + p.vert_count as u64 <= portal_verts.len() as u64,
            "portal {i} vertex range out of bounds"
        );
    }
    for (i, c) in cells.iter().enumerate() {
        ensure!(
            (c.first_aabb as usize) < aabb_nodes.len() || aabb_nodes.is_empty(),
            "cell {i} aabb tree out of bounds"
        );
        ensure!(
            c.first_portal as u64 + c.portal_count as u64 <= portals.len() as u64,
            "cell {i} portal range out of bounds"
        );
        ensure!(
            c.first_cull_index as u64 + c.cull_count as u64 <= cull_indices.len() as u64,
            "cell {i} cull index range out of bounds"
        );
    }
    for (i, n) in nodes.iter().enumerate() {
        ensure!(
            (n.plane as usize) < planes.len(),
            "node {i} plane out of bounds"
        );
        for c in n.children {
            let ok = if c >= 0 {
                (c as usize) < nodes.len()
            } else {
                ((-(c as i64) - 1) as usize) < leafs.len()
            };
            ensure!(ok, "node {i} child {c} out of bounds");
        }
    }
    for (i, l) in leafs.iter().enumerate() {
        ensure!(l.cell < cells.len() as i32, "leaf {i} cell out of bounds");
    }
    // Occluder range validation: everything references something, per-map.
    for oc in occluders.iter() {
        let prange = oc.first_plane as usize..oc.first_plane as usize + oc.num_planes as usize;
        ensure!(
            prange.end <= occluder_plane_indices.len(),
            "occluder planes out of range"
        );
        let erange = oc.first_edge as usize..oc.first_edge as usize + oc.num_edges as usize;
        ensure!(
            erange.end <= occluder_edges.len(),
            "occluder edges out of range"
        );
        let vrange = oc.first_vert as usize..oc.first_vert as usize + oc.vert_count as usize;
        ensure!(
            vrange.end <= portal_verts.len(),
            "occluder verts out of range"
        );
        for pi in &occluder_plane_indices[prange] {
            let in_range = (*pi as usize) < planes.len();
            ensure!(in_range, "occluder references {pi} out of plane range");
        }
        for e in &occluder_edges[erange] {
            // plane sub-indices are relative to the occluder's own planes.
            // Vertex ones are the occluder's absolute lump-11 slots taken
            // modulo 256 (a u8 in the file): recover the local index as
            // `B - first_vert (mod 256)`; vert_count stays far below 256.
            let plane_ok = |s: &u8| (*s as usize) < oc.num_planes as usize;
            let r = (oc.first_vert % 256) as usize;
            let vert_ok = |s: &u8| (((*s as usize) + 256 - r) % 256) < oc.vert_count as usize;
            ensure!(
                e[..2].iter().all(plane_ok),
                "occluder edge plane sub-index out of range"
            );
            ensure!(
                e[2..].iter().all(vert_ok),
                "occluder edge vertex sub-index out of range"
            );
        }
    }
    for (i, c) in cells.iter().enumerate() {
        let list = c.first_occluder as usize..c.first_occluder as usize + c.occluder_count as usize;
        ensure!(
            list.end <= occluder_indices.len(),
            "cell {i} occluders out of range"
        );
    }

    Ok(MapWorld {
        materials,
        lightmaps,
        lightmap_basis: Vec::new(),
        light_grid: None,
        sun: None,
        model_lighting: None,
        entity_lighting: None,
        model_lights: Vec::new(),
        reflection_probes: Vec::new(),
        waters: Vec::new(),
        soup_probes: Vec::new(),
        soup_order: Vec::new(),
        sun_casters: Vec::new(),
        soup_lights: Vec::new(),
        soups,
        verts,
        tangents: Vec::new(),
        indices,
        entities,
        planes,
        brush_sides,
        brushes,
        models,
        cull_groups,
        cull_indices,
        portal_verts,
        occluders,
        occluder_plane_indices,
        occluder_edges,
        occluder_indices,
        aabb_nodes,
        cells,
        portals,
        portals_only: false,
        portal_min_clip_area: 0.0,
        portal_min_recurse_depth: 0,
        static_lods: Vec::new(),
        nodes,
        leafs,
        pvs,
        terrain,
        patches,
        collision_verts,
        collision_indices,
        light_indices,
        leaf_lights,
        leaf_lists,
        leaf_brushes,
        leaf_partitions,
        lights,
        light_vis,
    })
}

/// One block's key/value pairs in the order the lump lists them, as
/// `G_ParseSpawnVars` collects them (cod11-gsc-object-model.md 7).
fn parse_entity_pairs(block: &str) -> Vec<(String, String)> {
    block
        .lines()
        .filter_map(|line| {
            let toks: Vec<&str> = line.trim().split('"').collect();
            if toks.len() >= 4 {
                Some((toks[1].to_string(), toks[3].to_string()))
            } else {
                None
            }
        })
        .collect()
}

pub fn parse_vec3(s: &str) -> Option<[f32; 3]> {
    let mut it = s.split_whitespace().map(|t| t.parse::<f32>().ok());
    Some([it.next()??, it.next()??, it.next()??])
}

/// Every block's keys as the field pass reads them: `G_ParseField`
/// matches a key with `Q_stricmp` (1.1d 0x6143a) and `G_CallSpawn`
/// applies every pair in order, so a key is stored folded to lower case
/// and a repeated one keeps its last copy (cod11-gsc-object-model.md 7).
/// Look keys up in lower case. A spawn function's own read is
/// `G_SpawnString`'s, which [`spawn_string`] takes over
/// [`entity_block_pairs`].
pub fn entity_blocks(entities: &str) -> Vec<std::collections::HashMap<String, String>> {
    entity_block_pairs(entities)
        .iter()
        .map(|pairs| field_view(pairs))
        .collect()
}

/// One block's [`entity_blocks`] view: keys folded, the last copy kept.
pub fn field_view(pairs: &[(String, String)]) -> std::collections::HashMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_ascii_lowercase(), v.clone()))
        .collect()
}

/// A key's value as the field pass leaves it on the entity: the last pair
/// whose key matches without case, as [`field_view`] keeps it.
pub fn field_value<'a>(pairs: &'a [(String, String)], key: &str) -> Option<&'a str> {
    pairs
        .iter()
        .rev()
        .find(|(k, _)| k.eq_ignore_ascii_case(key))
        .map(|(_, v)| v.as_str())
}

/// The block's `classname` as CoDMP.exe 1.1's renderer entity pass keeps it
/// (0x4dbf50): a key spelled exactly `classname`
/// goes to its own slot, each copy over the last.
pub fn render_classname(pairs: &[(String, String)]) -> Option<&str> {
    pairs
        .iter()
        .rev()
        .find(|(k, _)| k == "classname")
        .map(|(_, v)| v.as_str())
}

/// Any other key as that pass looks it up (0x4dba20): the first pair
/// whose key matches with `_stricmp`.
pub fn render_key<'a>(pairs: &'a [(String, String)], key: &str) -> Option<&'a str> {
    pairs
        .iter()
        .filter(|(k, _)| k != "classname")
        .find(|(k, _)| k.eq_ignore_ascii_case(key))
        .map(|(_, v)| v.as_str())
}

/// `G_SpawnString` (1.1d 0x61390, UO game.mp.uo 0x812e4, CoD2 0x80f415e), under
/// `G_SpawnInt`, `G_SpawnFloat` and `G_SpawnVector`: `strcmp` from pair 0,
/// the first match (cod11-gsc-object-model.md 7). Case-sensitive, unlike
/// the field pass.
pub fn spawn_string<'a>(pairs: &'a [(String, String)], key: &str) -> Option<&'a str> {
    pairs
        .iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.as_str())
}

/// Every block's key/value pairs in lump order, repeats kept.
pub fn entity_block_pairs(entities: &str) -> Vec<Vec<(String, String)>> {
    entities
        .split('{')
        .skip(1)
        .map(|b| b.split('}').next().unwrap_or(""))
        .map(parse_entity_pairs)
        .collect()
}

/// The unit vector toward the sun the sky draws its sprite and flare along:
/// worldspawn `sundirection` read as pitch, yaw, roll through AngleVectors
/// (CoDMP.exe 0x4dc2c8..0x4dc306 store the forward at 0x16c57d0), or
/// retail's default (0.45, 0.3, 0.9) normalised when the key is absent
/// (0x4dd040). mp_ship.bsp carries `-45 110 0`, 45 degrees up.
pub fn sun_direction(entities: &str) -> [f32; 3] {
    let angles = entity_blocks(entities)
        .into_iter()
        .find(|b| b.get("classname").map(String::as_str) == Some("worldspawn"))
        .and_then(|b| b.get("sundirection").and_then(|v| parse_vec3(v)));
    let v = match angles {
        Some([pitch, yaw, _]) => {
            let (sp, cp) = pitch.to_radians().sin_cos();
            let (sy, cy) = yaw.to_radians().sin_cos();
            glam::Vec3::new(cp * cy, cp * sy, -sp)
        }
        None => glam::Vec3::new(0.45, 0.3, 0.9).normalize(),
    };
    v.to_array()
}

/// A `corona` entity as the renderer's world load keeps it (CoDMP.exe
/// 0x4dbcd0, docs/research/cod11-shader-scripts.md section 8, "Coronas").
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Corona {
    pub origin: [f32; 3],
    /// The sprite's half-width: `scale` times 25.5.
    pub radius: f32,
    /// Drawn only while the unit vector from the corona to the eye has a z
    /// below this, fully opaque at or below `zfadeout`.
    pub zcutoff: f32,
    pub zfadeout: f32,
    /// `dl_color` bytes, alpha 255.
    pub rgba: [u8; 4],
}

/// Retail's cap, a fatal `MAX_MAP_CORONAS` error past it (0x4dbcdb).
pub const MAX_MAP_CORONAS: usize = 512;

/// Every `corona` entity in file order. A scale of 0 or less is a fatal
/// error in retail; vcod skips the entity.
pub fn coronas(entities: &str) -> Vec<Corona> {
    let mut out = Vec::new();
    for pairs in entity_block_pairs(entities) {
        if !render_classname(&pairs).is_some_and(|c| c.eq_ignore_ascii_case("corona")) {
            continue;
        }
        let b = |k: &str| render_key(&pairs, k);
        let num = |k: &str, default: f32| {
            b(k).and_then(|v| v.trim().parse::<f32>().ok())
                .unwrap_or(default)
        };
        let scale = num("scale", 1.0);
        if scale <= 0.0 {
            log::warn!("corona scale must be > 0, skipping one");
            continue;
        }
        if out.len() == MAX_MAP_CORONAS {
            log::warn!("MAX_MAP_CORONAS ({MAX_MAP_CORONAS}) exceeded, dropping the rest");
            break;
        }
        let color = b("dl_color").and_then(parse_vec3).unwrap_or([1.0; 3]);
        // clamped, then `_ftol2`'s truncation (0x4dbe2f..0x4dbf15)
        let byte = |c: f32| (c * 255.0).clamp(0.0, 255.0) as u8;
        out.push(Corona {
            origin: b("origin").and_then(parse_vec3).unwrap_or([0.0; 3]),
            radius: scale * 25.5,
            zcutoff: num("zcutoff", -0.15),
            zfadeout: num("zfadeout", -0.25),
            rgba: [byte(color[0]), byte(color[1]), byte(color[2]), 255],
        });
    }
    out
}

/// `(origin, yaw degrees)` of the first spawn in classname priority order:
/// CoD1's, then CoD2's (`mp_dm_spawn`, `mp_tdm_spawn`).
pub fn find_spawn(entities: &str) -> Option<([f32; 3], f32)> {
    const CLASSES: [&str; 6] = [
        "mp_deathmatch_spawn",
        "mp_teamdeathmatch_spawn",
        "mp_dm_spawn",
        "mp_tdm_spawn",
        "mp_deathmatch_intermission",
        "info_player_start",
    ];
    let blocks = entity_blocks(entities);

    for class in CLASSES {
        for b in &blocks {
            if b.get("classname").map(String::as_str) == Some(class) {
                let origin = b.get("origin")?;
                let pos = parse_vec3(origin)?;
                let yaw = b
                    .get("angles")
                    .and_then(|a| a.split_whitespace().nth(1))
                    .and_then(|t| t.parse().ok())
                    .unwrap_or(0.0);
                return Some((pos, yaw));
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    /// `spawn_string` is `G_SpawnString`'s strcmp first match, the field
    /// view folds keys and keeps the last copy, the renderer's lookup folds
    /// and takes the first.
    #[test]
    fn a_repeated_key_reads_first_by_spawn_string_and_last_as_a_field() {
        let ents = "{\n\"wait\" \"1\"\n\"Wait\" \"2\"\n\"wait\" \"3\"\n}";
        let pairs = &entity_block_pairs(ents)[0];
        assert_eq!(spawn_string(pairs, "wait"), Some("1"));
        assert_eq!(spawn_string(pairs, "WAIT"), None);
        assert_eq!(field_value(pairs, "WAIT"), Some("3"));
        assert_eq!(render_key(pairs, "WAIT"), Some("1"));
        let view = &entity_blocks(ents)[0];
        assert_eq!(view.len(), 1);
        assert_eq!(view.get("wait").map(String::as_str), Some("3"));
    }

    use super::*;

    /// IBSP 59 with the given lumps; every other lump is empty.
    fn build_bsp(lumps: &[(usize, &[u8])]) -> Vec<u8> {
        let mut dir = vec![(0u32, 0u32); LUMP_COUNT];
        let mut body = Vec::new();
        for &(i, bytes) in lumps {
            dir[i] = (bytes.len() as u32, (8 + LUMP_COUNT * 8 + body.len()) as u32);
            body.extend_from_slice(bytes);
        }
        let mut d = Vec::new();
        d.extend_from_slice(b"IBSP");
        d.extend(59u32.to_le_bytes());
        for (len, off) in dir {
            d.extend(len.to_le_bytes());
            d.extend(off.to_le_bytes());
        }
        d.extend(body);
        d
    }

    /// One material, one model, one soup of 3 verts and `indices`.
    fn soup_bsp(indices: &[u16]) -> Vec<u8> {
        let mut soup = Vec::new();
        soup.extend(0u16.to_le_bytes()); // material
        soup.extend(0u16.to_le_bytes()); // lightmap
        soup.extend(0u32.to_le_bytes()); // first_vertex
        soup.extend(3u16.to_le_bytes()); // vertex_count
        soup.extend((indices.len() as u16).to_le_bytes());
        soup.extend(0u32.to_le_bytes()); // first_index
        let idx: Vec<u8> = indices.iter().flat_map(|i| i.to_le_bytes()).collect();
        build_bsp(&[
            (LUMP_MATERIALS, &[0u8; 72]),
            (LUMP_SOUPS, &soup),
            (LUMP_MODELS, &[0u8; 48]),
            (LUMP_DRAWVERTS, &[0u8; 3 * 44]),
            (LUMP_DRAWINDICES, &idx),
        ])
    }

    #[test]
    fn rejects_soup_index_past_its_vertex_count() {
        assert!(parse(&soup_bsp(&[0, 1, 2])).is_ok());
        let err = parse(&soup_bsp(&[0, 1, 3])).unwrap_err().to_string();
        assert!(err.contains("index"), "got: {err}");
    }

    #[test]
    fn rejects_brush_side_total_that_overflows_u32() {
        // 65538 brushes of 65535 sides sum past u32::MAX
        let mut brushes = Vec::new();
        for _ in 0..65538 {
            brushes.extend(u16::MAX.to_le_bytes());
            brushes.extend(0u16.to_le_bytes());
        }
        let d = build_bsp(&[
            (LUMP_MATERIALS, &[0u8; 72]),
            (LUMP_MODELS, &[0u8; 48]),
            (LUMP_BRUSHES, &brushes),
        ]);
        let err = parse(&d).unwrap_err().to_string();
        assert!(err.contains("overflow"), "got: {err}");
    }

    #[test]
    fn rejects_wrong_magic_and_version() {
        assert!(parse(b"NOPE").is_err());
        let mut fake = Vec::new();
        fake.extend_from_slice(b"IBSP");
        fake.extend_from_slice(&4u32.to_le_bytes()); // Q3 version
        fake.extend_from_slice(&[0u8; 33 * 8]);
        let err = parse(&fake).unwrap_err().to_string();
        assert!(err.contains("59") && err.contains("4"), "got: {err}");
    }

    #[test]
    fn parses_mp_pavlov() {
        let Some(data) = crate::testing::real_bsp() else {
            return;
        };
        let bsp = parse(&data).unwrap();
        assert_eq!(bsp.materials.len(), 191);
        assert_eq!(bsp.materials[0].name, "textures/common/clip_nosight_metal");
        assert_eq!(bsp.lightmaps.len(), 11);
        assert_eq!(bsp.lightmaps[0].len(), 512 * 512 * 3);
        assert_eq!(bsp.soups.len(), 2625);
        assert_eq!(bsp.verts.len(), 94886);
        assert_eq!(bsp.indices.len(), 183840);
        for s in &bsp.soups {
            assert!((s.material as usize) < bsp.materials.len());
            assert!(s.first_vertex as usize + s.vertex_count as usize <= bsp.verts.len());
            let fi = s.first_index as usize;
            let chunk = &bsp.indices[fi..fi + s.index_count as usize];
            assert!(
                chunk
                    .iter()
                    .all(|&i| (i as usize) < s.vertex_count as usize)
            );
        }
        assert!(bsp.entities.contains("classname"));
        // winding the back-face culling relies on: cross(p1-p0, p2-p0)
        // opposes the vertex normal
        for s in &bsp.soups {
            let fv = s.first_vertex as usize;
            for tri in bsp.indices[s.first_index as usize..][..s.index_count as usize].chunks(3) {
                let p = |i: usize| glam::Vec3::from_array(bsp.verts[fv + tri[i] as usize].pos);
                let n = glam::Vec3::from_array(bsp.verts[fv + tri[0] as usize].normal);
                let geo = (p(1) - p(0)).cross(p(2) - p(0));
                if geo.length_squared() > 1e-9 {
                    assert!(geo.dot(n) < 0.0, "unexpected winding in soup");
                }
            }
        }
    }

    #[test]
    fn submodel_vertices_are_local_to_the_entity_origin() {
        let Some(fs) = crate::testing::game_fs() else {
            return;
        };
        let Some(path) = fs.resolve_map("mp_stanjel") else {
            return;
        };
        let bsp = parse(&fs.read(&path).unwrap()).unwrap();
        // *58 is a beam whose entity sits at (-1704, 472, 873)
        let block = entity_blocks(&bsp.entities)
            .into_iter()
            .find(|b| b.get("model").map(String::as_str) == Some("*58"))
            .expect("*58");
        let origin = parse_vec3(block.get("origin").expect("origin")).unwrap();
        assert_eq!(origin, [-1704.0, 472.0, 873.0]);

        let m = &bsp.models[58];
        let soups = &bsp.soups[m.first_soup as usize..][..m.num_soups as usize];
        assert!(!soups.is_empty(), "submodel 58 draws");
        for s in soups {
            let fv = s.first_vertex as usize;
            for v in &bsp.verts[fv..fv + s.vertex_count as usize] {
                for a in 0..3 {
                    assert!(
                        v.pos[a] >= bsp.models[58].mins[a] && v.pos[a] <= bsp.models[58].maxs[a],
                        "vertex outside the submodel's own bounds"
                    );
                    assert!(v.pos[a].abs() < 128.0, "vertex is in map space, not local");
                }
            }
        }
    }

    #[test]
    fn finds_spawn_in_entities() {
        let ents = r#"{
"origin" "-8952 9109 37"
"angles" "0 225 0"
"classname" "mp_deathmatch_spawn"
}"#;
        let (origin, yaw) = find_spawn(ents).unwrap();
        assert_eq!(origin, [-8952.0, 9109.0, 37.0]);
        assert_eq!(yaw, 225.0);
        assert!(find_spawn("{\n\"classname\" \"worldspawn\"\n}").is_none());
    }

    #[test]
    fn finds_spawn_in_mp_pavlov() {
        let Some(data) = crate::testing::real_bsp() else {
            return;
        };
        let bsp = parse(&data).unwrap();
        assert!(find_spawn(&bsp.entities).is_some());
    }

    #[test]
    fn reads_sundirection_from_worldspawn() {
        let ents = r#"{
"sundirection" "-90 0 0"
"suncolor" ".9 .9 .89"
"classname" "worldspawn"
}
{
"classname" "misc_model"
"sundirection" "1 2 3"
}"#;
        // pitch -90 looks straight up; only worldspawn counts
        let d = sun_direction(ents);
        assert!((d[2] - 1.0).abs() < 1e-6 && d[0].abs() < 1e-6, "{d:?}");
        let d = sun_direction("{\n\"classname\" \"worldspawn\"\n}");
        let want = glam::Vec3::new(0.45, 0.3, 0.9).normalize().to_array();
        assert_eq!(d, want);
    }

    #[test]
    fn reads_coronas_with_retail_defaults() {
        let ents = r#"{
"classname" "worldspawn"
}
{
"classname" "Corona"
"origin" "1 2 3"
"scale" ".3"
"zcutoff" "-0.4"
"zfadeout" "-0.6"
"dl_color" "1 0.5 2"
}
{
"classname" "corona"
}
{
"classname" "corona"
"scale" "0"
}"#;
        let c = coronas(ents);
        assert_eq!(c.len(), 2, "{c:?}");
        assert_eq!(c[0].origin, [1.0, 2.0, 3.0]);
        assert!((c[0].radius - 7.65).abs() < 1e-5);
        assert_eq!((c[0].zcutoff, c[0].zfadeout), (-0.4, -0.6));
        // 127.5 truncates, 510 clamps
        assert_eq!(c[0].rgba, [255, 127, 255, 255]);
        assert_eq!(
            c[1],
            Corona {
                origin: [0.0; 3],
                radius: 25.5,
                zcutoff: -0.15,
                zfadeout: -0.25,
                rgba: [255; 4],
            }
        );
    }

    #[test]
    fn mp_ship_has_its_cage_light_coronas() {
        let Some(fs) = crate::testing::game_fs() else {
            return;
        };
        let Some(data) = fs.read("maps/MP/mp_ship.bsp") else {
            return;
        };
        let c = coronas(&parse(&data).unwrap().entities);
        assert_eq!(c.len(), 36);
        assert!(c.iter().all(|c| (c.radius - 7.65).abs() < 1e-4));
    }

    #[test]
    fn mp_ship_sundirection_is_angles_matching_the_sun_light() {
        let Some(fs) = crate::testing::game_fs() else {
            return;
        };
        let Some(data) = fs.read("maps/MP/mp_ship.bsp") else {
            return;
        };
        let bsp = parse(&data).unwrap();
        // `-45 110 0` as angles; the compiler's lump-30 sun (words 7..9)
        // points the same way, which a vector reading of the key would not
        let d = glam::Vec3::from(sun_direction(&bsp.entities));
        let sun = bsp
            .lights
            .iter()
            .rev()
            .find(|w| w[0] == 1)
            .expect("a kind-1 light");
        let lump = glam::Vec3::from_array([7, 8, 9].map(|i| f32::from_bits(sun[i])));
        assert!(d.dot(lump.normalize()) > 0.999, "{d} vs {lump}");
        assert!((d.z - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-3);
    }

    #[test]
    fn parses_collision_lumps_in_mp_pavlov() {
        let Some(data) = crate::testing::real_bsp() else {
            return;
        };
        let bsp = parse(&data).unwrap();
        assert_eq!(bsp.planes.len(), 9132);
        assert_eq!(bsp.brush_sides.len(), 56966);
        assert_eq!(bsp.brushes.len(), 7609);
        assert_eq!(bsp.models.len(), 35);
        // world model owns brushes [0, 7575); submodels own 1 brush each
        assert_eq!(bsp.models[0].first_brush, 0);
        assert_eq!(bsp.models[0].num_brushes, 7575);
        for p in &bsp.planes {
            let n = glam::Vec3::from_array(p.normal);
            assert!((n.length() - 1.0).abs() < 1e-3);
        }
        let mut expect_first = 0u32;
        for b in &bsp.brushes {
            assert_eq!(b.first_side, expect_first);
            assert!(b.num_sides >= 6, "brush with fewer than 6 sides");
            assert!((b.material as usize) < bsp.materials.len());
            expect_first += b.num_sides as u32;
        }
        assert_eq!(expect_first as usize, bsp.brush_sides.len());
        assert_eq!(bsp.terrain.len() + bsp.patches.len(), 619);
        assert_eq!(bsp.collision_verts.len(), 6291);
        assert_eq!(bsp.collision_indices.len(), 4496 * 3);
        for b in &bsp.brushes {
            let s = &bsp.brush_sides[b.first_side as usize..][..b.num_sides as usize];
            for axis in 0..3 {
                let lo = f32::from_bits(s[axis * 2].plane_or_dist);
                let hi = f32::from_bits(s[axis * 2 + 1].plane_or_dist);
                assert!(lo <= hi, "axial bounds out of order");
            }
            for side in &s[6..] {
                assert!((side.plane_or_dist as usize) < bsp.planes.len());
            }
        }
    }

    #[test]
    fn parses_visibility_lumps_in_mp_pavlov() {
        let Some(data) = crate::testing::real_bsp() else {
            return;
        };
        let bsp = parse(&data).unwrap();
        assert_eq!(bsp.cull_groups.len(), 122);
        assert_eq!(bsp.cull_indices.len(), 125);
        assert_eq!(bsp.portal_verts.len(), 568);
        assert_eq!(bsp.occluders.len(), 9);
        assert_eq!(bsp.occluder_plane_indices.len(), 54);
        assert_eq!(bsp.occluder_edges.len(), 108);
        assert_eq!(bsp.occluder_indices.len(), 11);
        assert_eq!(bsp.aabb_nodes.len(), 335);
        assert_eq!(bsp.cells.len(), 20);
        assert_eq!(bsp.portals.len(), 124);
        assert_eq!(bsp.nodes.len(), 1752);
        assert_eq!(bsp.leafs.len(), 1788);
        // occluder ranges chain inside their lumps: all 6-plane boxes
        for w in bsp.occluders.windows(2) {
            assert_eq!(w[0].first_plane + w[0].num_planes as u32, w[1].first_plane);
            assert_eq!(w[0].first_edge + w[0].num_edges as u32, w[1].first_edge);
            assert_eq!(w[0].first_vert + w[0].vert_count as u32, w[1].first_vert);
        }
        // the shared pool leads with the occluders' corners: their runs tile [0,72)
        // and the remaining 568-72 slots belong to the portals
        let last_occ = bsp.occluders.last().unwrap();
        assert_eq!(last_occ.first_vert + last_occ.vert_count as u32, 72);
        assert!(
            bsp.portals
                .iter()
                .all(|p| p.first_vert >= last_occ.first_vert + last_occ.vert_count as u32)
        );
        // cells 0/3/12 carry the 11 occluder references (all of them no. 0)
        let refering: Vec<(u32, u32)> = bsp
            .cells
            .iter()
            .map(|c| (c.occluder_count, c.first_occluder))
            .filter(|&(n, _)| n > 0)
            .collect();
        assert_eq!(
            refering,
            vec![(9, 0), (1, 9), (1, 10)],
            "cell 0 holds nine, cells 3 and 12 one each"
        );
        // cell ranges chain: portals and cull indices are laid out cell by cell
        for w in bsp.cells.windows(2) {
            assert_eq!(w[0].first_portal + w[0].portal_count, w[1].first_portal);
            assert_eq!(
                w[0].first_cull_index + w[0].cull_count,
                w[1].first_cull_index
            );
        }
        let last = bsp.cells.last().unwrap();
        assert_eq!(last.first_portal + last.portal_count, 124);
        assert_eq!(last.first_cull_index + last.cull_count, 125);
        // the soup lump is [cull-group soups][cell-tree soups]
        let cg_end = bsp
            .cull_groups
            .iter()
            .map(|g| g.first_soup + g.soup_count)
            .max()
            .unwrap();
        assert_eq!(cg_end, 437);
        assert_eq!(bsp.aabb_nodes[0].first_soup, 437);
        // node 0 is cell 0's own root, not the whole forest: its range covers only
        // cell 0's share (1873 of the 2188 cell-tree soups; the other 19 cells'
        // roots, found via each cell's first_aabb, chain to cover the rest)
        assert_eq!(
            bsp.aabb_nodes[0].first_soup + bsp.aabb_nodes[0].soup_count,
            2310
        );
        // root node of the tree splits on a plane and both children exist
        assert!(bsp.nodes[0].children.iter().all(|&c| c != 0));
        assert_eq!(bsp.leafs[0].cell, 0);
        assert_eq!(
            bsp.portals[0],
            Portal {
                plane: 58,
                cell: 14,
                first_vert: 72,
                vert_count: 4
            }
        );
    }

    #[test]
    fn rejects_a_truncated_cells_lump() {
        let Some(mut data) = crate::testing::real_bsp() else {
            return;
        };
        // shorten lump 17 by one byte: 8 + 17 * 8 is its length word
        let off = 8 + 17 * 8;
        let len = u32::from_le_bytes(data[off..off + 4].try_into().unwrap());
        data[off..off + 4].copy_from_slice(&(len - 1).to_le_bytes());
        let err = parse(&data).unwrap_err().to_string();
        assert!(err.contains("cells"), "{err}");
    }

    #[test]
    fn rejects_a_portal_past_the_vertex_lump() {
        let Some(mut data) = crate::testing::real_bsp() else {
            return;
        };
        // lump 18 offset; portal 0's first_vert is its third u32
        let dir = 8 + 18 * 8;
        let off = u32::from_le_bytes(data[dir + 4..dir + 8].try_into().unwrap()) as usize;
        data[off + 8..off + 12].copy_from_slice(&u32::MAX.to_le_bytes());
        let err = parse(&data).unwrap_err().to_string();
        assert!(err.contains("portal"), "{err}");
    }
}
