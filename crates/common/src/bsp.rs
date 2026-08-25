//! IBSP 59 parser. Layouts: docs/research/bsp-ibsp59-format.md.

use anyhow::{anyhow, ensure, Result};

pub const LIGHTMAP_SIZE: usize = 512;
pub const NO_LIGHTMAP: u16 = 65535;

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

#[derive(Debug)]
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

#[derive(Debug)]
pub struct Plane {
    pub normal: [f32; 3],
    pub dist: f32,
}

#[derive(Debug)]
pub struct BrushSide {
    /// Sides 0..6: f32 axial bound, bit-cast. Sides 6..: index into
    /// `Bsp::planes`. Decoded in collision.rs.
    pub plane_or_dist: u32,
    /// Unused.
    #[allow(dead_code)]
    pub material: u32,
}

#[derive(Debug)]
pub struct Brush {
    pub first_side: u32,
    pub num_sides: u16,
    pub material: u16,
}

/// Lump 27 entry. Model 0 is the world; 1.. are the `"model" "*N"`
/// brushmodels. Soup and brush ranges partition their lumps (doc, "Lump 27,
/// models").
#[derive(Debug)]
pub struct Model {
    /// Relative to the brushmodel's origin brush. Unused outside tests.
    pub mins: [f32; 3],
    pub maxs: [f32; 3],
    pub first_soup: u32,
    pub num_soups: u32,
    pub first_brush: u32,
    pub num_brushes: u32,
}

#[derive(Debug)]
pub struct Bsp {
    pub materials: Vec<Material>,
    pub lightmaps: Vec<Vec<u8>>, // 512*512*3 RGB pages
    pub soups: Vec<TriangleSoup>,
    pub verts: Vec<DrawVert>,
    pub indices: Vec<u16>,
    pub entities: String,
    pub planes: Vec<Plane>,
    pub brush_sides: Vec<BrushSide>,
    pub brushes: Vec<Brush>,
    pub models: Vec<Model>,
}

fn lump<'a>(data: &'a [u8], dir: &[(u32, u32)], i: usize) -> Result<&'a [u8]> {
    let (len, off) = (dir[i].0 as usize, dir[i].1 as usize);
    ensure!(off + len <= data.len(), "lump {i} out of bounds");
    Ok(&data[off..off + len])
}

pub fn parse(data: &[u8]) -> Result<Bsp> {
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
    // bytes 32..40 (collision-aabb range) are unused
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

    Ok(Bsp {
        materials,
        lightmaps,
        soups,
        verts,
        indices,
        entities,
        planes,
        brush_sides,
        brushes,
        models,
    })
}

impl Bsp {
    /// Submodel `n`'s drawable soups as dynamic-pass surfaces plus their
    /// material names. `None` when out of range or collision-only (most stock
    /// maps have no drawable submodels). Vertices are submodel-local; the
    /// entity's origin places them (doc, "Lump 27, models"). Bone data is
    /// inert (index 0, weight 1) so the skinned pipeline draws the mesh
    /// against its identity bone block.
    pub fn submodel_mesh(&self, n: usize) -> Option<(Vec<crate::xmodel::Surface>, Vec<String>)> {
        let model = self.models.get(n)?;
        let mut surfaces = Vec::new();
        let mut materials = Vec::new();
        for soup in &self.soups[model.first_soup as usize..][..model.num_soups as usize] {
            let material = &self.materials[soup.material as usize];
            if crate::mesh::should_skip(material) {
                continue;
            }
            let fv = soup.first_vertex as usize;
            let verts: Vec<crate::xmodel::VmVert> = self.verts[fv..fv + soup.vertex_count as usize]
                .iter()
                .map(|v| crate::xmodel::VmVert {
                    pos: v.pos,
                    normal: v.normal,
                    uv: v.uv,
                    bone_indices: [0; 4],
                    bone_weights: [1.0, 0.0, 0.0, 0.0],
                })
                .collect();
            // soup indices are relative to `first_vertex`
            let fi = soup.first_index as usize;
            let indices = self.indices[fi..fi + soup.index_count as usize].to_vec();
            if verts.is_empty() || indices.is_empty() {
                continue;
            }
            surfaces.push(crate::xmodel::Surface {
                verts,
                indices,
                material: materials.len(),
            });
            materials.push(material.name.clone());
        }
        (!surfaces.is_empty()).then_some((surfaces, materials))
    }
}

fn parse_entity_block(block: &str) -> std::collections::HashMap<String, String> {
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

pub fn entity_blocks(entities: &str) -> Vec<std::collections::HashMap<String, String>> {
    entities
        .split('{')
        .skip(1)
        .map(|b| b.split('}').next().unwrap_or(""))
        .map(parse_entity_block)
        .collect()
}

/// `(origin, yaw degrees)` of the first spawn in classname priority order.
pub fn find_spawn(entities: &str) -> Option<([f32; 3], f32)> {
    const CLASSES: [&str; 4] = [
        "mp_deathmatch_spawn",
        "mp_teamdeathmatch_spawn",
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
            assert!(chunk
                .iter()
                .all(|&i| (i as usize) < s.vertex_count as usize));
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

    /// mp_harbor is the smallest stock map with drawable submodels.
    #[test]
    fn extracts_submodel_meshes_in_mp_harbor() {
        let Some(fs) = crate::testing::game_fs() else {
            return;
        };
        let bsp = parse(&fs.read("maps/mp/mp_harbor.bsp").unwrap()).unwrap();
        assert_eq!(bsp.models[0].first_soup, 0);
        assert_eq!(bsp.models[0].num_soups, 1492);
        // submodel 1 is the map-wide trigger_hurt volume: brushes, no surfaces
        assert!(bsp.submodel_mesh(1).is_none());

        let (surfaces, materials) = bsp.submodel_mesh(2).expect("submodel 2");
        assert_eq!(
            materials,
            [
                "textures/industrial/metal@armoreddoor1",
                "textures/industrial/metal@baseboard1lit"
            ]
        );
        assert_eq!(surfaces.len(), 2);
        for (i, s) in surfaces.iter().enumerate() {
            assert_eq!(s.material, i);
            assert!(!s.verts.is_empty());
            assert!(!s.indices.is_empty() && s.indices.len() % 3 == 0);
            assert!(s.indices.iter().all(|&i| (i as usize) < s.verts.len()));
            for v in &s.verts {
                assert_eq!(v.bone_indices, [0; 4]);
                assert_eq!(v.bone_weights, [1.0, 0.0, 0.0, 0.0]);
            }
        }
        assert!(bsp.models[1..].iter().all(|m| m.first_soup >= 1492));
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

        let (surfaces, _) = bsp.submodel_mesh(58).expect("submodel 58");
        for s in &surfaces {
            for v in &s.verts {
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
}
