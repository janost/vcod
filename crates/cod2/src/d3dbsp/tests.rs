//! Synthetic files: record decoding and the validation errors.

use super::*;

/// Builds a file from per-lump byte blobs.
#[derive(Clone)]
struct Map {
    lumps: Vec<Vec<u8>>,
}

fn le(words: &[u32]) -> Vec<u8> {
    words.iter().flat_map(|w| w.to_le_bytes()).collect()
}

fn f(x: f32) -> u32 {
    x.to_bits()
}

impl Map {
    /// The smallest map that passes: one material, plane, brush, surface
    /// (one triangle), node, two leafs, model, a PVS and an entity string.
    fn minimal() -> Self {
        let mut lumps = vec![Vec::new(); LUMP_COUNT];
        let mut mat = vec![0u8; 72];
        mat[..6].copy_from_slice(b"caulk\0");
        mat[0x40..0x44].copy_from_slice(&0x80u32.to_le_bytes());
        mat[0x44..0x48].copy_from_slice(&0x2000_0005u32.to_le_bytes());
        lumps[lump::MATERIALS] = mat;
        lumps[lump::PLANES] = le(&[f(0.0), f(0.0), f(1.0), f(8.0)]);
        // Six axial sides then one plane side.
        let mut sides = Vec::new();
        for b in [-1.0f32, 1.0, -2.0, 2.0, -3.0, 3.0] {
            sides.extend(le(&[f(b), 0]));
        }
        sides.extend(le(&[0, 0]));
        lumps[lump::BRUSH_SIDES] = sides;
        lumps[lump::BRUSHES] = [7i16.to_le_bytes(), 0i16.to_le_bytes()].concat();
        let mut surf = Vec::new();
        surf.extend(0i16.to_le_bytes());
        surf.extend(NO_LIGHTMAP.to_le_bytes());
        surf.extend(0i32.to_le_bytes());
        surf.extend(3u16.to_le_bytes());
        surf.extend(3u16.to_le_bytes());
        surf.extend(0i32.to_le_bytes());
        lumps[lump::SURFACES] = surf;
        let mut verts = Vec::new();
        for k in 0..3 {
            let mut v = vec![0u8; 68];
            v[0..4].copy_from_slice(&(k as f32).to_le_bytes());
            v[0x18..0x1c].copy_from_slice(&[1, 2, 3, 4]);
            v[0x2c..0x30].copy_from_slice(&1.0f32.to_le_bytes());
            v[0x3c..0x40].copy_from_slice(&1.0f32.to_le_bytes());
            verts.extend(v);
        }
        lumps[lump::VERTS] = verts;
        lumps[lump::INDICES] = [0u16, 1, 2].iter().flat_map(|i| i.to_le_bytes()).collect();
        // Node 0: plane 0, children leaf 1 and leaf 1.
        lumps[lump::NODES] = le(&[0, -2i32 as u32, -2i32 as u32, 0, 0, 0, 0, 0, 0]);
        let mut leafs = le(&[-1i32 as u32, 0, 0, 0, 0, 0, -1i32 as u32, 0, 0]);
        leafs.extend(le(&[0, 0, 0, 0, 0, 1, -1i32 as u32, 0, 0]));
        lumps[lump::LEAFS] = leafs;
        lumps[lump::LEAF_BRUSHES] = le(&[0]);
        lumps[lump::MODELS] = le(&[
            f(-1.0),
            f(-2.0),
            f(-3.0),
            f(1.0),
            f(2.0),
            f(3.0),
            0,
            1,
            0,
            0,
            0,
            1,
        ]);
        lumps[lump::VISIBILITY] = le(&[1, 8, 1, 0]);
        lumps[lump::ENTITIES] = b"{\n\"classname\" \"worldspawn\"\n}\n\0".to_vec();
        Self { lumps }
    }

    fn bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend(MAGIC);
        out.extend(VERSION.to_le_bytes());
        let mut at = HEADER_BYTES as u32;
        for l in &self.lumps {
            let off = if l.is_empty() { 0 } else { at };
            out.extend(le(&[l.len() as u32, off]));
            at += l.len() as u32;
        }
        for l in &self.lumps {
            out.extend(l);
        }
        out
    }

    fn with(mut self, i: usize, b: Vec<u8>) -> Self {
        self.lumps[i] = b;
        self
    }

    fn parse(&self) -> Result<D3dBsp> {
        D3dBsp::parse(&self.bytes())
    }
}

fn err(m: &Map) -> String {
    format!("{:#}", m.parse().unwrap_err())
}

#[test]
fn minimal_map_decodes() {
    let b = Map::minimal().parse().unwrap();
    assert_eq!(b.materials[0].name, "caulk");
    assert_eq!(b.materials[0].surface_flags, 0x80);
    assert_eq!(b.planes[0].normal, Vec3::Z);
    assert_eq!(b.planes[0].dist, 8.0);
    assert_eq!(b.brushes[0].num_sides, 7);
    assert_eq!(b.brush_sides[1].bound(), 1.0);
    let bx = b.brush_box(0);
    assert_eq!(bx.mins, [-1.0, -2.0, -3.0]);
    assert_eq!(bx.maxs, [1.0, 2.0, 3.0]);
    assert_eq!(bx.contents, 0x1);
    assert_eq!(b.surfaces[0].lightmap, NO_LIGHTMAP);
    assert_eq!(b.verts[2].xyz.x, 2.0);
    assert_eq!(b.verts[0].color, [1, 2, 3, 4]);
    assert_eq!(b.verts[0].tangent, Vec3::X);
    assert_eq!(b.verts[0].binormal, Vec3::Y);
    assert_eq!(b.indices, [0, 1, 2]);
    assert_eq!(b.nodes[0].children, [-2, -2]);
    assert_eq!(b.leafs[1].leaf_brush_count, 1);
    assert_eq!(b.leafs[1].cell, -1);
    assert_eq!(b.models[0].brush_count, 1);
    let vis = b.visibility.as_ref().unwrap();
    assert!(vis.sees(0, 0));
    assert!(!vis.sees(0, 1));
    assert_eq!(b.entities, "{\n\"classname\" \"worldspawn\"\n}\n");
    let (list, trees) = b.leaf_brush_trees().unwrap();
    assert_eq!(list, [0, 0]);
    assert!(trees[0].is_none());
    assert_eq!(trees[1].as_ref().unwrap().nodes.len(), 1);
}

#[test]
fn collision_records_decode() {
    let mut tri = le(&[f(0.0), f(0.0), f(1.0), f(0.0)]);
    tri.extend(le(&[f(1.0), 0, 0, 0, 0, f(1.0), 0, 0]));
    tri.extend(le(&[0, 1, 2, -1i32 as u32, 0, -1i32 as u32]));
    let mut cverts = Vec::new();
    for k in 0..3 {
        cverts.extend(le(&[0, f(k as f32), 0, 0]));
    }
    let edge = le(&[
        0,
        f(1.0),
        f(2.0),
        f(3.0),
        0,
        0,
        f(1.0),
        0,
        f(1.0),
        0,
        f(1.0),
        0,
        0,
        f(4.0),
    ]);
    let border = le(&[f(1.0), 0, f(5.0), f(6.0), f(0.5), f(-1.0), f(10.0)]);
    let part = [vec![0, 0, 1, 1], le(&[0, 0])].concat();
    let mut aabb = le(&[0, 0, 0, f(1.0), f(1.0), f(1.0)]);
    aabb.extend([0, 0, 1, 0]);
    aabb.extend(le(&[1]));
    aabb.extend(le(&[0, 0, 0, f(1.0), f(1.0), f(1.0)]));
    aabb.extend([0, 0, 0, 0]);
    aabb.extend(le(&[0]));
    let b = Map::minimal()
        .with(lump::COLL_TRIS, tri)
        .with(lump::COLL_VERTS, cverts)
        .with(lump::COLL_EDGES, edge)
        .with(lump::COLL_BORDERS, border)
        .with(lump::COLL_PARTITIONS, part)
        .with(lump::COLL_AABB_TREES, aabb)
        .parse()
        .unwrap();
    assert_eq!(b.coll_tris[0].verts, [0, 1, 2]);
    assert_eq!(b.coll_tris[0].edges, [-1, 0, -1]);
    assert_eq!(b.coll_tris[0].svec, [1.0, 0.0, 0.0, 0.0]);
    assert_eq!(b.coll_verts[2].xyz, Vec3::new(2.0, 0.0, 0.0));
    assert_eq!(b.coll_edges[0].origin, Vec3::new(1.0, 2.0, 3.0));
    assert_eq!(b.coll_edges[0].axis, [Vec3::Z, Vec3::Y]);
    assert_eq!(b.coll_edges[0].dir, Vec3::X);
    assert_eq!(b.coll_edges[0].length, 4.0);
    assert_eq!(b.coll_borders[0].dist_eq, [1.0, 0.0, 5.0]);
    assert_eq!(b.coll_borders[0].length, 10.0);
    assert_eq!(
        b.coll_partitions[0],
        CollPartition {
            unused: 0,
            tri_count: 1,
            border_count: 1,
            first_tri: 0,
            first_border: 0
        }
    );
    assert_eq!(b.coll_aabb_trees[0].child_count, 1);
    assert_eq!(b.coll_aabb_trees[0].child_or_partition, 1);
    assert_eq!(b.coll_aabb_trees[1].half_size, Vec3::ONE);
}

#[test]
fn light_grid_and_lightmaps_decode() {
    let mut page = vec![0u8; LIGHTMAP_PAGE_BYTES];
    page[0x10_0000] = 7;
    page[0x30_0000 + 5] = 9;
    let mut surf = Map::minimal().lumps[lump::SURFACES].clone();
    surf[2..4].copy_from_slice(&0i16.to_le_bytes());
    let grid = [
        le(&[0x0020_0401]),
        vec![0x41, 0x0f],
        1u16.to_le_bytes().to_vec(),
    ]
    .concat();
    let colours: Vec<u8> = (0..48).collect();
    let b = Map::minimal()
        .with(lump::LIGHTMAPS, page)
        .with(lump::SURFACES, surf)
        .with(lump::LIGHT_GRID_ENTRIES, grid)
        .with(lump::LIGHT_GRID_COLOURS, colours)
        .parse()
        .unwrap();
    assert_eq!(b.lightmaps.len(), 1);
    assert_eq!(b.lightmaps[0].rgba[1][0], 7);
    assert_eq!(b.lightmaps[0].rgba[0].len(), 512 * 512 * 4);
    assert_eq!(b.lightmaps[0].sun.len(), 1024 * 1024);
    assert_eq!(b.lightmaps[0].sun[5], 9);
    assert_eq!(
        b.light_grid[0],
        LightGridEntry {
            key: 0x0020_0401,
            low: 0x41,
            occlusion_mask: 0x0f,
            colour: 1
        }
    );
    assert_eq!(b.light_grid_colours[1].rgb[2][7], 47);
}

#[test]
fn header_errors() {
    let good = Map::minimal().bytes();
    assert!(D3dBsp::parse(&good[..100]).is_err());
    let mut magic = good.clone();
    magic[0] = b'X';
    assert!(format!("{:#}", D3dBsp::parse(&magic).unwrap_err()).contains("magic"));
    let mut version = good.clone();
    version[4] = 59;
    assert!(format!("{:#}", D3dBsp::parse(&version).unwrap_err()).contains("version"));
    let mut past = good.clone();
    let o = 8 + 8 * lump::ENTITIES;
    past[o..o + 4].copy_from_slice(&1_000_000u32.to_le_bytes());
    assert!(format!("{:#}", D3dBsp::parse(&past).unwrap_err()).contains("past end"));
    let mut inside = good;
    inside[o + 4..o + 8].copy_from_slice(&4u32.to_le_bytes());
    assert!(format!("{:#}", D3dBsp::parse(&inside).unwrap_err()).contains("offset"));
}

#[test]
fn validation_errors() {
    let m = Map::minimal();
    let cases: Vec<(Map, &str)> = vec![
        (m.clone().with(lump::PLANES, vec![0; 15]), "funny lump size"),
        (m.clone().with(lump::MATERIALS, vec![]), "no materials"),
        (
            m.clone().with(
                lump::BRUSHES,
                [5i16, 0].iter().flat_map(|x| x.to_le_bytes()).collect(),
            ),
            "less than 6",
        ),
        (
            m.clone().with(
                lump::BRUSHES,
                [7i16, 3].iter().flat_map(|x| x.to_le_bytes()).collect(),
            ),
            "materialNum",
        ),
        (m.clone().with(lump::LEAF_BRUSHES, le(&[4])), "brushIndex"),
        (
            m.clone().with(
                lump::INDICES,
                [0u16, 1, 3].iter().flat_map(|i| i.to_le_bytes()).collect(),
            ),
            "index past",
        ),
        (
            m.clone()
                .with(lump::NODES, le(&[0, 5, -2i32 as u32, 0, 0, 0, 0, 0, 0])),
            "child 5",
        ),
        (m.clone().with(lump::VISIBILITY, le(&[2, 8, 1])), "lump 36"),
        (
            m.clone()
                .with(lump::LIGHTMAPS, vec![0; LIGHTMAP_PAGE_BYTES]),
            "lightmap pages",
        ),
        (
            m.clone().with(
                lump::COLL_PARTITIONS,
                [vec![0, 0, 1, 0], le(&[0, 0])].concat(),
            ),
            "triangle run",
        ),
        (
            m.clone().with(lump::CULL_GROUP_INDICES, le(&[0])),
            "cull group index",
        ),
    ];
    for (map, want) in cases {
        let e = err(&map);
        assert!(e.contains(want), "want {want:?}, got {e:?}");
    }
    // A brush plane side out of range.
    let mut sides = m.lumps[lump::BRUSH_SIDES].clone();
    sides[48..52].copy_from_slice(&9u32.to_le_bytes());
    assert!(err(&m.clone().with(lump::BRUSH_SIDES, sides)).contains("plane 9"));
    // Leaf cluster past the PVS.
    let mut leafs = m.lumps[lump::LEAFS].clone();
    leafs[36..40].copy_from_slice(&3i32.to_le_bytes());
    assert!(err(&m.clone().with(lump::LEAFS, leafs)).contains("cluster 3"));
    // A render AABB tree whose children run off the lump.
    let cell = le(&[0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
    let tree = le(&[0, 0, 2]);
    let e = err(&m.with(lump::CELLS, cell).with(lump::AABB_TREES, tree));
    assert!(e.contains("leaves lump 22"), "{e}");
}

/// Garbage after the header never panics.
#[test]
fn noise_never_panics() {
    let good = Map::minimal().bytes();
    let mut x = 0x1234_5678u32;
    for _ in 0..2000 {
        let mut b = good.clone();
        for _ in 0..8 {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            let at = (x as usize) % b.len();
            b[at] = (x >> 8) as u8;
        }
        if let Ok(bsp) = D3dBsp::parse(&b) {
            let _ = bsp.leaf_brush_trees();
        }
    }
}
