//! A CoD2 `.d3dbsp` as the engine's neutral [`MapWorld`]: the render
//! geometry, cells and portals, the BSP tree and PVS, brushes, and the
//! collision triangles as one terrain partition per lump-33 partition
//! (`docs/research/cod2-d3dbsp-format.md`). Built on [`D3dBsp::parse`],
//! which has already range-checked every index used here.
//!
//! The light grid (lumps 2, 3) goes in as stored (`MapWorld::light_grid`,
//! sampled by [`crate::lightgrid`]). The collision AABB trees,
//! edges, borders and the leaf brush trees are `crate::collision`'s, which
//! traces the world in place of the terrain partitions here
//! (`crate::collision::collision_world`). Lightmaps go in twice: as an RGB
//! approximation, [`lightmap_rgb`], and as stored
//! (`MapWorld::lightmap_basis`) for the shader path.

use std::collections::HashMap;

use anyhow::Result;
use glam::{DVec3, Vec3};
use vcod_common::mapworld::{
    self, AabbNode, Brush, BrushSide, Cell, CullGroup, DrawVert, LIGHTMAP_SIZE, Leaf, LeafLists,
    LightGrid, LightmapBasis, MapWorld, Material, Model, Node, Occluder, Partition, Plane, Portal,
    Pvs, TerrainPart, TriangleSoup,
};
use vcod_common::net::FogParams;

use crate::d3dbsp::{self, CollAabbTree, CollPartition, CollTri, CollVert, D3dBsp, LightmapPage};

/// Parses a `.d3dbsp` and converts it.
pub fn from_bytes(data: &[u8]) -> Result<MapWorld> {
    Ok(from_bsp(&D3dBsp::parse(data)?))
}

/// A parsed map as a [`MapWorld`].
pub fn from_bsp(b: &D3dBsp) -> MapWorld {
    let sun = sun_colour(&b.entities);
    let (collision_verts, collision_indices, terrain) = collision_terrain(b);
    let mut leaf_partitions = Vec::new();
    let mut partition_run = |first: i32, count: i32| -> (u32, u32) {
        let at = leaf_partitions.len() as u32;
        for p in tree_partitions(&b.coll_aabb_trees, first as usize, count as usize) {
            leaf_partitions.push(Partition::Terrain(p));
        }
        (at, leaf_partitions.len() as u32 - at)
    };
    let leaf_lists: Vec<LeafLists> = b
        .leafs
        .iter()
        .map(|l| LeafLists {
            brushes: (l.first_leaf_brush as u32, l.leaf_brush_count as u32),
            partitions: partition_run(l.first_coll_aabb, l.coll_aabb_count),
        })
        .collect();
    // Model 0 reaches lump 34 only through its leafs; a submodel's own run
    // is empty on every stock map but is kept for one that has it.
    let models = b
        .models
        .iter()
        .enumerate()
        .map(|(i, m)| Model {
            mins: m.mins.to_array(),
            maxs: m.maxs.to_array(),
            first_soup: run_start(m.first_surface, m.surface_count),
            num_soups: m.surface_count as u32,
            first_brush: run_start(m.first_brush, m.brush_count),
            num_brushes: m.brush_count as u32,
            partitions: if i == 0 {
                (0, 0)
            } else {
                partition_run(m.first_coll_aabb, m.coll_aabb_count)
            },
        })
        .collect();

    MapWorld {
        materials: b
            .materials
            .iter()
            .map(|m| Material {
                name: m.name.replace('\\', "/"),
                surface_flags: m.surface_flags,
                content_flags: m.content_flags,
            })
            .collect(),
        lightmaps: b.lightmaps.iter().map(|p| lightmap_rgb(p, sun)).collect(),
        lightmap_basis: b
            .lightmaps
            .iter()
            .map(|p| LightmapBasis {
                coeffs: p.rgba.clone(),
                sun: p.sun.clone(),
            })
            .collect(),
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
        light_grid: Some(LightGrid {
            entries: b.light_grid.clone(),
            colours: b.light_grid_colours.clone(),
        }),
        soups: b
            .surfaces
            .iter()
            .map(|s| TriangleSoup {
                material: s.material as u16,
                lightmap: if s.lightmap == d3dbsp::NO_LIGHTMAP {
                    mapworld::NO_LIGHTMAP
                } else {
                    s.lightmap as u16
                },
                first_vertex: s.first_vertex as u32,
                vertex_count: s.vertex_count,
                index_count: s.index_count,
                first_index: s.first_index as u32,
            })
            .collect(),
        verts: b
            .verts
            .iter()
            .map(|v| DrawVert {
                pos: v.xyz.to_array(),
                uv: v.uv.to_array(),
                lm_uv: v.lm_uv.to_array(),
                normal: v.normal.to_array(),
                color: v.color,
            })
            .collect(),
        tangents: b
            .verts
            .iter()
            .map(|v| [v.tangent.to_array(), v.binormal.to_array()])
            .collect(),
        indices: b.indices.clone(),
        entities: b.entities.clone(),

        cull_groups: b
            .cull_groups
            .iter()
            .map(|g| CullGroup {
                mins: g.mins.to_array(),
                maxs: g.maxs.to_array(),
                first_soup: run_start(g.first_surface, g.surface_count),
                soup_count: g.surface_count as u32,
            })
            .collect(),
        cull_indices: b.cull_group_indices.iter().map(|&i| i as u32).collect(),
        portal_verts: b.portal_verts.iter().map(|v| v.to_array()).collect(),
        occluders: b
            .occluders
            .iter()
            .map(|o| Occluder {
                first_plane: o.first_plane,
                num_planes: o.num_planes,
                num_edges: o.num_edges,
                first_edge: o.first_edge,
                first_vert: o.first_vert,
                vert_count: o.vert_count,
            })
            .collect(),
        occluder_plane_indices: b.occluder_planes.clone(),
        occluder_edges: b
            .occluder_edges
            .iter()
            .map(|e| [e.plane0, e.plane1, e.vert0, e.vert1])
            .collect(),
        occluder_indices: b.occluder_indices.iter().map(|&i| i as u16).collect(),
        aabb_nodes: b
            .aabb_trees
            .iter()
            .map(|n| AabbNode {
                first_soup: run_start(n.first_surface, n.surface_count),
                soup_count: n.surface_count as u32,
                child_count: n.child_count as u32,
            })
            .collect(),
        cells: b
            .cells
            .iter()
            .map(|c| Cell {
                mins: c.mins.to_array(),
                maxs: c.maxs.to_array(),
                first_aabb: c.first_aabb_tree as u32,
                first_portal: run_start(c.first_portal, c.portal_count),
                portal_count: c.portal_count as u32,
                first_cull_index: run_start(c.first_cull_group_index, c.cull_group_count),
                cull_count: c.cull_group_count as u32,
                first_occluder: run_start(c.first_occluder, c.occluder_count),
                occluder_count: c.occluder_count as u32,
            })
            .collect(),
        portals: b
            .portals
            .iter()
            .map(|p| Portal {
                plane: p.plane as u32,
                cell: p.cell as u32,
                first_vert: p.first_vert as u32,
                vert_count: p.vert_count as u32,
            })
            .collect(),
        // retail's queued portal walk, CoD4's (cod2-client-perf.md 9)
        portals_only: true,
        // `r_portalMinClipArea`'s default (Mac dvar registration)
        portal_min_clip_area: 0.02,
        portal_min_recurse_depth: 0,
        static_lods: Vec::new(),

        planes: b
            .planes
            .iter()
            .map(|p| Plane {
                normal: p.normal.to_array(),
                dist: p.dist,
            })
            .collect(),
        nodes: b
            .nodes
            .iter()
            .map(|n| Node {
                plane: n.plane as u32,
                children: n.children,
                mins: n.mins,
                maxs: n.maxs,
            })
            .collect(),
        leafs: b
            .leafs
            .iter()
            .map(|l| Leaf {
                cluster: l.cluster,
                area: l.area,
                cell: l.cell,
            })
            .collect(),
        pvs: b.visibility.as_ref().and_then(|v| {
            Pvs::new(
                v.cluster_count as usize,
                v.row_bytes as usize,
                v.rows.clone(),
            )
        }),

        brush_sides: b
            .brush_sides
            .iter()
            .map(|s| BrushSide {
                plane_or_dist: s.plane_or_bound,
                material: s.material as u32,
            })
            .collect(),
        brushes: b
            .brushes
            .iter()
            .map(|br| Brush {
                first_side: br.first_side,
                num_sides: br.num_sides as u16,
                material: br.material as u16,
            })
            .collect(),
        models,
        terrain,
        patches: Vec::new(),
        collision_verts,
        collision_indices,
        leaf_lists,
        leaf_brushes: b.leaf_brushes.iter().map(|&i| i as u32).collect(),
        leaf_partitions,

        // IBSP 4 has no static light lumps (doc, "Lump directory").
        light_indices: Vec::new(),
        leaf_lights: Vec::new(),
        lights: Vec::new(),
        light_vis: Vec::new(),
    }
}

/// A run's first index, 0 for an empty run: the renderer stores -1 there
/// for a count of 0 (doc, lumps 10 and 35), which a `u32` cannot carry.
fn run_start(first: i32, count: i32) -> u32 {
    if count <= 0 { 0 } else { first as u32 }
}

/// The partitions under `count` collision AABB roots from `first`, tree
/// leaves in preorder, each once: the order `CM_TraceThroughAabbTree` meets
/// them, its per-trace stamp skipping a repeat (doc, lump 34).
fn tree_partitions(trees: &[CollAabbTree], first: usize, count: usize) -> Vec<u32> {
    fn walk(trees: &[CollAabbTree], i: usize, out: &mut Vec<u32>, depth: u32) {
        // Children always follow their parent (census); the cap is for a
        // malformed file.
        let Some(n) = trees.get(i).filter(|_| depth < 256) else {
            return;
        };
        if n.child_count == 0 {
            let p = n.child_or_partition as u32;
            if !out.contains(&p) {
                out.push(p);
            }
            return;
        }
        let c = n.child_or_partition as usize;
        for k in c..c + n.child_count as usize {
            walk(trees, k, out, depth + 1);
        }
    }
    let mut out = Vec::new();
    for root in first..first + count {
        walk(trees, root, &mut out, 0);
    }
    out
}

/// Lumps 29, 31 and 33 as [`TerrainPart`]s, one per partition, with their
/// own vertex and index lists. A partition's material is the first lump-34
/// tree leaf's that names it (0 for one no tree names). Each triangle is
/// wound so that `cross(c - a, b - a)`, the plane the terrain clip builds,
/// faces along the record's plane.
fn collision_terrain(b: &D3dBsp) -> (Vec<[f32; 3]>, Vec<u16>, Vec<TerrainPart>) {
    let mut material = vec![None; b.coll_partitions.len()];
    for n in b.coll_aabb_trees.iter().filter(|n| n.child_count == 0) {
        let m = &mut material[n.child_or_partition as usize];
        m.get_or_insert(n.material);
    }
    let mut verts = Vec::new();
    let mut indices = Vec::new();
    let parts = b
        .coll_partitions
        .iter()
        .zip(material)
        .map(|(p, m)| {
            let (first_vert, first_index) = (verts.len() as u32, indices.len() as u32);
            partition_tris(p, &b.coll_tris, &b.coll_verts, &mut verts, &mut indices);
            TerrainPart {
                material: m.unwrap_or(0),
                first_vert,
                vert_count: (verts.len() as u32 - first_vert) as u16,
                first_index,
                index_count: (indices.len() as u32 - first_index) as u16,
            }
        })
        .collect();
    (verts, indices, parts)
}

/// Appends one partition's triangles. A vertex lump 29 names is shared
/// within the partition; one given as -1 is solved off the triangle's
/// plane and barycentric vectors ([`tri_corner`]).
fn partition_tris(
    p: &CollPartition,
    tris: &[CollTri],
    coll_verts: &[CollVert],
    verts: &mut Vec<[f32; 3]>,
    indices: &mut Vec<u16>,
) {
    let base = verts.len();
    let mut shared: HashMap<i32, u16> = HashMap::new();
    let first = p.first_tri as usize;
    for t in &tris[first..first + p.tri_count as usize] {
        let mut corner = |k: usize| -> (u16, Vec3) {
            let v = t.verts[k];
            if v >= 0 {
                let pos = coll_verts[v as usize].xyz;
                let i = *shared.entry(v).or_insert_with(|| {
                    verts.push(pos.to_array());
                    (verts.len() - 1 - base) as u16
                });
                (i, pos)
            } else {
                let pos = tri_corner(t, k);
                verts.push(pos.to_array());
                ((verts.len() - 1 - base) as u16, pos)
            }
        };
        let (a, b, c) = (corner(0), corner(1), corner(2));
        let normal = Vec3::new(t.plane[0], t.plane[1], t.plane[2]);
        let faces = (c.1 - a.1).cross(b.1 - a.1).dot(normal) >= 0.0;
        let (b, c) = if faces { (b, c) } else { (c, b) };
        indices.extend([a.0, b.0, c.0]);
    }
}

/// Corner `k` of a collision triangle from its plane and `(s, t)` vectors:
/// the point on the plane where `(s, t)` reads (0, 0), (1, 0) or (0, 1)
/// (doc, lump 31). For a vertex slot lump 29 leaves as -1.
fn tri_corner(t: &CollTri, k: usize) -> Vec3 {
    let (s, tt) = [(0.0, 0.0), (1.0, 0.0), (0.0, 1.0)][k];
    let row = |v: &[f32; 4]| DVec3::new(v[0].into(), v[1].into(), v[2].into());
    let (n, sv, tv) = (row(&t.plane), row(&t.svec), row(&t.tvec));
    let rhs = DVec3::new(
        t.plane[3].into(),
        f64::from(t.svec[3]) + s,
        f64::from(t.tvec[3]) + tt,
    );
    // Cramer's rule on the rows n, sv, tv.
    let det = n.dot(sv.cross(tv));
    if det.abs() < 1e-12 {
        return Vec3::ZERO;
    }
    let x = rhs.x * sv.cross(tv) + rhs.y * tv.cross(n) + rhs.z * n.cross(sv);
    (x / det).as_vec3()
}

/// The sun the shader path lights with, `GfxLight` as
/// `R_InterpretSunLightParseParamsIntoLights` (mac 0xe25de) fills it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SunLight {
    /// Unit vector toward the sun: `AngleVectors` of `sundirection` (pitch,
    /// yaw), (1, 0, 0) without the key. The shaders' `lightPosition0`.
    pub dir: [f32; 3],
    /// [`sun_colour`], the shaders' `lightColor0` and, under the default
    /// `r_specularColorScale` 1, `lightSpecular0` (`R_AddCmdLightProperties`
    /// scales the colour by the dvar).
    pub colour: [f32; 3],
}

/// worldspawn's value for `key`, matched without case (`I_stricmp`).
fn worldspawn_key(entities: &str, key: &str) -> Option<String> {
    let world = vcod_common::bsp::entity_blocks(entities)
        .into_iter()
        .find(|b| b.get("classname").is_some_and(|c| c == "worldspawn"))?;
    world
        .into_iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(key))
        .map(|(_, v)| v)
}

/// The map's sun light off worldspawn's keys (`R_ParseSunLight` mac 0xe22e4).
pub fn sun_light(entities: &str) -> SunLight {
    let angles = worldspawn_key(entities, "sundirection")
        .and_then(|v| vcod_common::bsp::parse_vec3(&v))
        .unwrap_or([0.0; 3]);
    let (sp, cp) = angles[0].to_radians().sin_cos();
    let (sy, cy) = angles[1].to_radians().sin_cos();
    SunLight {
        dir: [cp * cy, cp * sy, -sp],
        colour: sun_colour(entities),
    }
}

/// The fog a map's own script sets: the first `setExpFog(density, r, g, b,
/// time)` or `setCullFog(near, far, r, g, b, time)` in `maps/mp/<map>.gsc`
/// outside comments, as the fog configstring would carry it (exp fog's
/// near and far unused). What fly mode, which runs no script, draws with.
pub fn script_fog(gsc: &str) -> Option<FogParams> {
    let code = strip_comments(gsc);
    let lower = code.to_ascii_lowercase();
    let call = |name: &str| -> Option<(usize, Vec<f32>)> {
        let at = lower.find(&format!("{name}("))?;
        let args = &code[at + name.len() + 1..];
        let args = &args[..args.find(')')?];
        let nums: Option<Vec<f32>> = args.split(',').map(|a| a.trim().parse().ok()).collect();
        Some((at, nums?))
    };
    let exp = call("setexpfog").filter(|(_, a)| a.len() == 5);
    let cull = call("setcullfog").filter(|(_, a)| a.len() == 6);
    let first = match (exp, cull) {
        (Some(e), Some(c)) => {
            if e.0 < c.0 {
                Ok(e.1)
            } else {
                Err(c.1)
            }
        }
        (Some(e), None) => Ok(e.1),
        (None, Some(c)) => Err(c.1),
        (None, None) => return None,
    };
    Some(match first {
        Ok(a) => FogParams {
            near: 0.0,
            far: 1.0,
            density: a[0],
            color: [a[1], a[2], a[3]],
            fade_ms: 0,
            start: 0.0,
        },
        // linear: the configstring's density slot reads 1 (`R_SetFog` mac
        // 0xd286c takes 1 or more as linear)
        Err(a) => FogParams {
            near: a[0],
            far: a[1],
            density: 1.0,
            color: [a[2], a[3], a[4]],
            fade_ms: 0,
            start: 0.0,
        },
    })
}

/// `src` with `//` and `/* */` comments blanked out.
fn strip_comments(src: &str) -> String {
    let mut out = String::with_capacity(src.len());
    let mut rest = src;
    while !rest.is_empty() {
        if let Some(r) = rest.strip_prefix("//") {
            rest = r.find('\n').map_or("", |i| &r[i..]);
        } else if let Some(r) = rest.strip_prefix("/*") {
            rest = r.find("*/").map_or("", |i| &r[i + 2..]);
        } else {
            let c = rest.chars().next().unwrap();
            out.push(c);
            rest = &rest[c.len_utf8()..];
        }
    }
    out
}

/// The sun colour the fixed-function lightmap path adds per texel:
/// worldspawn's `(sunlight - ambient) * (1 - diffuseFraction)` times
/// `suncolor` scaled to a largest component of 1 (`R_ParseSunLight` mac
/// 0xe22e4, `R_InterpretSunLightParseParamsIntoLights` mac 0xe25de;
/// `diffuseFraction` defaults to 0.5, an `ambient` over 2 is read as 0-255
/// and scaled by 4/255).
pub fn sun_colour(entities: &str) -> [f32; 3] {
    let world = vcod_common::bsp::entity_blocks(entities)
        .into_iter()
        .find(|b| b.get("classname").is_some_and(|c| c == "worldspawn"))
        .unwrap_or_default();
    // Keys match without case (`I_stricmp`); stock maps spell
    // `diffusefraction`.
    let get = |k: &str| {
        world
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(k))
            .map(|(_, v)| v.as_str())
    };
    let num = |k: &str, d: f32| {
        get(k)
            .and_then(|v| v.trim().parse::<f32>().ok())
            .unwrap_or(d)
    };
    let mut ambient = num("ambient", 0.0);
    if ambient > 2.0 {
        ambient *= 4.0 / 255.0;
    }
    let scale = (num("sunlight", 0.0) - ambient) * (1.0 - num("diffuseFraction", 0.5));
    let colour = get("suncolor")
        .and_then(vcod_common::bsp::parse_vec3)
        .unwrap_or([0.0; 3]);
    let max = colour[0].max(colour[1]).max(colour[2]);
    let norm = if max == 0.0 {
        [1.0; 3]
    } else {
        colour.map(|c| c / max)
    };
    norm.map(|c| c * scale)
}

/// One lump-1 page as the 512x512 RGB page the CoD1 renderer takes. This is
/// retail's own fallback, the fixed-function path's page build (`r_rendererInUse`
/// 2, `R_LoadWorldInternal` mac 0xe4fa0): channel `k` is byte 0 of
/// coefficient image `k`'s texel plus `sun[k]` times the sun image at twice
/// the coordinates, `floor((c + sun * v) * 0.5 + 0.5)` clamped to 255. That
/// halves the light as CoD1's pages are stored (the overbright doubles it
/// back). The shader path's per-normal basis weights are not modelled.
pub fn lightmap_rgb(page: &LightmapPage, sun: [f32; 3]) -> Vec<u8> {
    const S: usize = LightmapPage::SUN_SIZE;
    debug_assert_eq!(LightmapPage::RGBA_SIZE, LIGHTMAP_SIZE);
    let mut out = Vec::with_capacity(LIGHTMAP_SIZE * LIGHTMAP_SIZE * 3);
    for y in 0..LIGHTMAP_SIZE {
        for x in 0..LIGHTMAP_SIZE {
            let texel = (y * LIGHTMAP_SIZE + x) * 4;
            let v = f32::from(page.sun[2 * y * S + 2 * x]);
            for (image, sun) in page.rgba.iter().zip(sun) {
                let c = f32::from(image[texel]);
                out.push(((c + sun * v) * 0.5 + 0.5).floor().min(255.0) as u8);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tri(verts: [[f32; 3]; 3]) -> CollTri {
        // Plane and barycentric vectors as the compiler writes them: s and t
        // read (1, 0) and (0, 1) at vertices 1 and 2.
        let [a, b, c] = verts.map(Vec3::from_array);
        let n = (b - a).cross(c - a).normalize();
        let (e1, e2) = (b - a, c - a);
        // Dual basis of (e1, e2) in the plane.
        let (d11, d12, d22) = (e1.dot(e1), e1.dot(e2), e2.dot(e2));
        let det = d11 * d22 - d12 * d12;
        let sv = (e1 * d22 - e2 * d12) / det;
        let tv = (e2 * d11 - e1 * d12) / det;
        CollTri {
            plane: [n.x, n.y, n.z, n.dot(a)],
            svec: [sv.x, sv.y, sv.z, sv.dot(a)],
            tvec: [tv.x, tv.y, tv.z, tv.dot(a)],
            verts: [-1; 3],
            edges: [-1; 3],
        }
    }

    #[test]
    fn a_missing_corner_is_solved_off_the_plane() {
        let v = [[10.0, 0.0, 5.0], [20.0, 3.0, 6.0], [12.0, 15.0, 4.0]];
        let t = tri(v);
        for (k, want) in v.iter().enumerate() {
            let got = tri_corner(&t, k);
            assert!(got.distance(Vec3::from_array(*want)) < 1e-3, "{k}: {got}");
        }
    }

    /// On every triangle of mp_toujane whose three vertices lump 29 names,
    /// the solved corners land on them. Needs `COD2_DIR`.
    #[test]
    fn solved_corners_match_toujane_verts() {
        let Some(fs) = vcod_common::testing::cod2_fs() else {
            return;
        };
        let b = D3dBsp::parse(&fs.read("maps/mp/mp_toujane.d3dbsp").unwrap()).unwrap();
        let mut checked = 0;
        for t in b
            .coll_tris
            .iter()
            .filter(|t| t.verts.iter().all(|&v| v >= 0))
        {
            for k in 0..3 {
                let want = b.coll_verts[t.verts[k] as usize].xyz;
                let got = tri_corner(t, k);
                assert!(got.distance(want) < 0.05, "{got} vs {want}");
            }
            checked += 1;
        }
        assert!(checked > 8000, "{checked}");
    }

    #[test]
    fn triangles_wind_against_their_plane_and_share_named_verts() {
        let v = [[0.0, 0.0, 0.0], [10.0, 0.0, 0.0], [0.0, 10.0, 0.0]];
        let mut t = tri(v);
        t.verts = [0, 1, -1];
        let coll: Vec<CollVert> = v[..2]
            .iter()
            .map(|p| CollVert {
                unused: 0,
                xyz: Vec3::from_array(*p),
            })
            .collect();
        let p = CollPartition {
            unused: 0,
            tri_count: 2,
            border_count: 0,
            first_tri: 0,
            first_border: 0,
        };
        let (mut verts, mut idx) = (Vec::new(), Vec::new());
        partition_tris(&p, &[t, t], &coll, &mut verts, &mut idx);
        // Two named verts shared, one solved per triangle.
        assert_eq!(verts.len(), 4);
        assert_eq!(idx.len(), 6);
        let n = Vec3::new(t.plane[0], t.plane[1], t.plane[2]);
        for f in idx.chunks(3) {
            let [a, b, c] = [f[0], f[1], f[2]].map(|i| Vec3::from_array(verts[i as usize]));
            assert!((c - a).cross(b - a).dot(n) > 0.0);
        }
    }

    #[test]
    fn tree_partitions_walk_preorder_once() {
        let node = |child_count, child_or_partition| CollAabbTree {
            origin: Vec3::ZERO,
            half_size: Vec3::ONE,
            material: 0,
            child_count,
            child_or_partition,
        };
        // Root 0 has children 1 and 2; 1 has children 3, 4.
        let trees = [
            node(2, 1),
            node(2, 3),
            node(0, 7),
            node(0, 5),
            node(0, 7),
            node(0, 9),
        ];
        assert_eq!(tree_partitions(&trees, 0, 1), vec![5, 7]);
        assert_eq!(tree_partitions(&trees, 0, 0), Vec::<u32>::new());
        assert_eq!(tree_partitions(&trees, 5, 1), vec![9]);
    }

    #[test]
    fn lightmap_takes_byte_zero_and_half_the_sun() {
        let mut page = LightmapPage {
            rgba: std::array::from_fn(|_| vec![0; 512 * 512 * 4]),
            sun: vec![0; 1024 * 1024],
        };
        // Texel (1, 2): images give 100, 50, 0 in byte 0; the sun image is
        // read at (2, 4).
        let t = (2 * 512 + 1) * 4;
        page.rgba[0][t] = 100;
        page.rgba[1][t] = 50;
        page.rgba[1][t + 1] = 255;
        page.sun[4 * 1024 + 2] = 200;
        let rgb = lightmap_rgb(&page, [1.0, 0.5, 2.0]);
        let o = (2 * 512 + 1) * 3;
        assert_eq!(&rgb[o..o + 3], &[150, 75, 200]);
        assert_eq!(rgb.len(), 512 * 512 * 3);
    }

    #[test]
    fn sun_light_points_along_sundirection() {
        let e = "{\n\"classname\" \"worldspawn\"\n\"SunDirection\" \"-90 0 0\"\n}\n";
        let d = sun_light(e).dir;
        assert!(d[0].abs() < 1e-6 && (d[2] - 1.0).abs() < 1e-6, "{d:?}");
        assert_eq!(sun_light("").dir, [1.0, 0.0, 0.0]);
    }

    #[test]
    fn script_fog_takes_the_first_live_call() {
        let gsc = "main()\n{\n//\tsetExpFog(0.9, 1, 1, 1, 0);\n/* setCullFog(0, 1, 0, 0, 0, 0); */\n\tsetExpFog(0.00015, 0.15, 0.14, 0.13, 0);\n\tsetCullFog(0, 16500, 0.5, 0.6, 0.55, 0);\n}\n";
        let f = script_fog(gsc).unwrap();
        assert!(!f.is_linear());
        assert_eq!(f.density, 0.00015);
        assert_eq!(f.color, [0.15, 0.14, 0.13]);
        let f = script_fog("setCullFog(10, 16500, 0.5, 0.6, 0.55, 0);").unwrap();
        assert!(f.is_linear());
        assert_eq!((f.near, f.far, f.color), (10.0, 16500.0, [0.5, 0.6, 0.55]));
        assert_eq!(script_fog("main() { wait 1; }"), None);
    }

    #[test]
    fn sun_colour_follows_worldspawn() {
        let e = "{\n\"classname\" \"worldspawn\"\n\"sunlight\" \"1.5\"\n\"suncolor\" \"0.5 1 0.25\"\n\"ambient\" \"0.5\"\n\"diffusefraction\" \"0.2\"\n}\n";
        let s = sun_colour(e);
        let want = [0.4, 0.8, 0.2];
        for k in 0..3 {
            assert!((s[k] - want[k]).abs() < 1e-5, "{s:?}");
        }
        assert_eq!(sun_colour(""), [0.0; 3]);
    }
}
