//! A CoD4 [`Map`] as the engine's neutral [`MapWorld`], what the client's
//! renderer and cell vis read: the soups, the cells' surface trees in
//! preorder, the portals, the cell lookup tree, the static models placed
//! as soups, and the lightmaps in CoD2's page form. What each conversion
//! rests on: `docs/research/cod4-world.md` section 9.

use crate::drawsurf::List;
use crate::map::{Map, NO_LIGHTMAP};
use anyhow::{Result, bail, ensure};
use std::collections::HashMap;
use vcod_common::assets::{Image, ImageData};
use vcod_common::mapworld::{
    self, AabbNode, Cell, CullGroup, DrawVert, LIGHTMAP_SIZE, Leaf, LightmapBasis, MapSun,
    MapWorld, Material, Model, Node, Plane, Portal, StaticLod, SunCaster, TriangleSoup,
};

/// A lightmap's two images, decoded: the L8 primary (as RGBA) and the
/// A8R8G8B8 secondary.
pub struct LightmapImages {
    pub primary: Image,
    pub secondary: Image,
}

/// One 512 x 512 tile of a merged lightmap: which lightmap and where.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct Tile {
    lightmap: usize,
    x: usize,
    y: usize,
}

/// A merged lightmap's tile grid, off its secondary's size: 512 texels a
/// tile across, two 512-texel halves (`A` over `B`) of `high` tiles down.
fn grid(img: &Image) -> Result<(usize, usize)> {
    let (w, h) = (img.width as usize, img.height as usize);
    ensure!(
        w % LIGHTMAP_SIZE == 0 && h % (2 * LIGHTMAP_SIZE) == 0 && w > 0 && h > 0,
        "secondary lightmap {w}x{h} is not 512 x 1024 tiles"
    );
    Ok((w / LIGHTMAP_SIZE, h / (2 * LIGHTMAP_SIZE)))
}

fn rgba(img: &Image) -> Result<&[u8]> {
    match &img.data {
        ImageData::Rgba8(px) => Ok(px),
        _ => bail!("lightmap image is not a bitmap"),
    }
}

/// The light `lm_r0c0` puts on a flat surface at one secondary texel pair:
/// `A.rgb + B.rgb / sqrt(1 + x^2 + y^2)`, the alphas carrying `x = 4.08 A.a
/// - 2.08`, `y = 4.064516 B.a - 2.064516`.
pub fn secondary_light(a: [u8; 4], b: [u8; 4]) -> [f32; 3] {
    let x = f32::from(a[3]) / 255.0 * 4.08 - 2.08;
    let y = f32::from(b[3]) / 255.0 * 4.064_516 - 2.064_516;
    let w = (1.0 / (1.0 + x * x + y * y).sqrt()).min(1.0);
    [0, 1, 2].map(|c| (f32::from(a[c]) + f32::from(b[c]) * w) / 255.0)
}

/// The pages of every lightmap's tiles: the basis form, which the CoD4
/// lit sets read as `lm_*_sm3.hlsl` does (its first two coefficient
/// images the secondary's halves `A` and `B` as stored, the third the flat
/// light, the primary as the sun visibility), and the RGB form of the flat
/// light.
fn pages(
    lightmaps: &[LightmapImages],
    tiles: &[Tile],
) -> Result<(Vec<LightmapBasis>, Vec<Vec<u8>>)> {
    let lm = LIGHTMAP_SIZE;
    let mut basis = Vec::with_capacity(tiles.len());
    let mut rgb = Vec::with_capacity(tiles.len());
    for t in tiles {
        let l = &lightmaps[t.lightmap];
        let (wide, high) = grid(&l.secondary)?;
        let sec = rgba(&l.secondary)?;
        let pri = rgba(&l.primary)?;
        let (sw, pw) = (l.secondary.width as usize, l.primary.width as usize);
        ensure!(
            l.primary.width as usize == 2 * lm * wide && l.primary.height as usize == 2 * lm * high,
            "primary lightmap {}x{} for a {wide}x{high} grid",
            l.primary.width,
            l.primary.height
        );
        let mut coeffs: [Vec<u8>; 3] = Default::default();
        coeffs.iter_mut().for_each(|c| c.reserve(lm * lm * 4));
        let mut page = Vec::with_capacity(lm * lm * 3);
        for row in 0..lm {
            for col in 0..lm {
                let px = |r: usize| {
                    let i = 4 * (r * sw + t.x * lm + col);
                    [sec[i], sec[i + 1], sec[i + 2], sec[i + 3]]
                };
                let a = px(t.y * lm + row);
                let b = px((high + t.y) * lm + row);
                let light =
                    secondary_light(a, b).map(|v| (v * 255.0).round().clamp(0.0, 255.0) as u8);
                coeffs[0].extend(a);
                coeffs[1].extend(b);
                coeffs[2].extend([light[0], light[1], light[2], 255]);
                page.extend(light);
            }
        }
        let mut sun = Vec::with_capacity(4 * lm * lm);
        for row in 0..2 * lm {
            let r = t.y * 2 * lm + row;
            for col in 0..2 * lm {
                sun.push(pri[4 * (r * pw + t.x * 2 * lm + col)]);
            }
        }
        basis.push(LightmapBasis { coeffs, sun });
        rgb.push(page);
    }
    Ok((basis, rgb))
}

/// Counts of what [`to_mapworld`] built, for the census.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Built {
    /// Leaf tree ranges and cull group runs that reach a surface a second
    /// time through another `sortedSurfIndex` entry.
    pub repeated_surfaces: usize,
    /// Surfaces no tree, cull group or brush model reaches: drawn as
    /// model 0's own soups after the trees'.
    pub loose_surfaces: usize,
    pub static_soups: usize,
    pub tiles: usize,
}

/// `map` as a [`MapWorld`]. Soup `k` below `sortedSurfIndex`'s length is
/// that entry's surface, so a tree's range is a soup range; the surfaces
/// none reaches, the brush models' and the static models' follow.
pub fn to_mapworld(map: &Map, lightmaps: &[LightmapImages]) -> Result<(MapWorld, Built)> {
    let mut built = Built::default();
    ensure!(
        lightmaps.len() == map.lightmaps.len(),
        "{} lightmaps for the map's {}",
        lightmaps.len(),
        map.lightmaps.len()
    );
    let grids: Vec<(usize, usize)> = lightmaps
        .iter()
        .map(|l| grid(&l.secondary))
        .collect::<Result<_>>()?;

    let materials: Vec<Material> = map
        .materials
        .iter()
        .map(|m| Material {
            name: m.name.clone(),
            surface_flags: 0,
            content_flags: 0,
        })
        .collect();

    // Each surface's vertices once: the span its triangles use (a
    // surface's `vertexCount` reaches far past it), its indices rebased on
    // the span and its lightmap coordinates moved into its tile's page.
    let mut verts: Vec<DrawVert> = Vec::new();
    let mut tangents: Vec<[[f32; 3]; 2]> = Vec::new();
    let mut indices = map.indices.clone();
    let mut tile_pages: HashMap<Tile, u16> = HashMap::new();
    let mut tiles: Vec<Tile> = Vec::new();
    let mut blocks: Vec<TriangleSoup> = Vec::with_capacity(map.surfaces.len());
    for s in &map.surfaces {
        let tris = s.base_index as usize..s.base_index as usize + 3 * s.tri_count as usize;
        let (lo, hi) = indices[tris.clone()]
            .iter()
            .fold((u16::MAX, 0u16), |(lo, hi), &i| (lo.min(i), hi.max(i)));
        let lo = lo.min(hi);
        let mut page = mapworld::NO_LIGHTMAP;
        let mut shift = (0.0, 0.0, 1.0, 1.0);
        if s.lightmap != NO_LIGHTMAP {
            let l = s.lightmap as usize;
            let (wide, high) = grids[l];
            // the tile the triangles' lightmap coordinates centre on
            let (mut su, mut sv) = (0.0f64, 0.0f64);
            for &i in &indices[tris.clone()] {
                let v = &map.vertices[s.first_vertex as usize + usize::from(i)];
                su += f64::from(v.lm_uv[0]);
                sv += f64::from(v.lm_uv[1]);
            }
            let n = tris.len().max(1) as f64;
            let x = ((su / n * wide as f64).floor().max(0.0) as usize).min(wide - 1);
            let y = ((sv / n * high as f64).floor().max(0.0) as usize).min(high - 1);
            let tile = Tile { lightmap: l, x, y };
            page = *tile_pages.entry(tile).or_insert_with(|| {
                tiles.push(tile);
                tiles.len() as u16 - 1
            });
            shift = (x as f32, y as f32, wide as f32, high as f32);
        }
        let first_vertex = verts.len() as u32;
        let span =
            s.first_vertex as usize + usize::from(lo)..=s.first_vertex as usize + usize::from(hi);
        for v in &map.vertices[span] {
            verts.push(DrawVert {
                pos: v.pos,
                uv: v.uv,
                lm_uv: [
                    v.lm_uv[0] * shift.2 - shift.0,
                    v.lm_uv[1] * shift.3 - shift.1,
                ],
                normal: v.normal,
                color: v.color,
            });
            let (n, t) = (v.normal, v.tangent);
            let b = [
                n[1] * t[2] - n[2] * t[1],
                n[2] * t[0] - n[0] * t[2],
                n[0] * t[1] - n[1] * t[0],
            ]
            .map(|c| c * v.binormal_sign);
            tangents.push([t, b]);
        }
        indices[tris].iter_mut().for_each(|i| *i -= lo);
        blocks.push(TriangleSoup {
            material: s.material as u16,
            lightmap: page,
            first_vertex,
            vertex_count: if s.tri_count == 0 { 0 } else { hi - lo + 1 },
            index_count: 3 * s.tri_count,
            first_index: s.base_index,
        });
    }
    let soup_of = |si: usize| {
        let b = &blocks[si];
        TriangleSoup {
            material: b.material,
            lightmap: b.lightmap,
            first_vertex: b.first_vertex,
            vertex_count: b.vertex_count,
            index_count: b.index_count,
            first_index: b.first_index,
        }
    };

    let mut soups: Vec<TriangleSoup> = Vec::new();
    // each soup's reflection probe, its surface's or static model's
    let mut soup_probes: Vec<u8> = Vec::new();
    // each soup's primary light, its surface's or static model's, the
    // technique it draws by
    let mut soup_lights: Vec<u8> = Vec::new();
    // each soup's place in 1.7's lit draw order (cod4-world.md 12)
    let mut soup_order: Vec<u64> = Vec::new();
    let ranks = crate::drawsurf::ranks(
        &map.materials
            .iter()
            .map(|m| m.order.clone())
            .collect::<Vec<_>>(),
    );
    let order_of = |list, material: u32, light, probe, id| {
        let m = &map.materials[material as usize];
        crate::drawsurf::soup_key(
            list,
            m.sort_key,
            light,
            ranks[material as usize],
            m.order.custom,
            probe,
            id,
        )
    };
    let surface_order = |list, si: usize| {
        let s = &map.surfaces[si];
        order_of(
            list,
            s.material,
            s.primary_light,
            s.reflection_probe,
            si as u32,
        )
    };
    // Which sorted entries a leaf range or cull group reaches.
    let mut reached = vec![false; map.sorted_surfaces.len()];
    for c in &map.cells {
        for t in c.aabb_trees.iter().filter(|t| t.child_count == 0) {
            let r = t.start_surf_index as usize..(t.start_surf_index + t.surface_count) as usize;
            reached[r].fill(true);
        }
    }
    for g in &map.cull_groups {
        let r = g.start_surf_index as usize..(g.start_surf_index + g.surface_count) as usize;
        reached[r].fill(true);
    }
    let mut seen = vec![false; map.surfaces.len()];
    for (k, &si) in map.sorted_surfaces.iter().enumerate() {
        soups.push(soup_of(si as usize));
        soup_probes.push(map.surfaces[si as usize].reflection_probe);
        soup_order.push(surface_order(List::World, si as usize));
        soup_lights.push(map.surfaces[si as usize].primary_light);
        if reached[k] {
            if seen[si as usize] {
                built.repeated_surfaces += 1;
            }
            seen[si as usize] = true;
        }
    }
    // Model 0's soups: the sorted entries, then the surfaces none of them
    // reaches that no brush model owns either.
    let brush_models = brush_model_ranges(map);
    let mut owned = vec![false; map.surfaces.len()];
    for r in &brush_models {
        owned[r.clone()].fill(true);
    }
    for si in 0..map.surfaces.len() {
        if !seen[si] && !owned[si] {
            soups.push(soup_of(si));
            soup_probes.push(map.surfaces[si].reflection_probe);
            soup_lights.push(map.surfaces[si].primary_light);
            soup_order.push(surface_order(List::World, si));
            built.loose_surfaces += 1;
        }
    }
    let world_soups = soups.len() as u32;
    let mut models = vec![Model {
        mins: map.mins,
        maxs: map.maxs,
        first_soup: 0,
        num_soups: world_soups,
        first_brush: 0,
        num_brushes: 0,
        partitions: (0, 0),
    }];
    for r in &brush_models {
        let first = soups.len() as u32;
        let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
        for si in r.clone() {
            soups.push(soup_of(si));
            soup_probes.push(map.surfaces[si].reflection_probe);
            soup_lights.push(map.surfaces[si].primary_light);
            soup_order.push(surface_order(List::BrushModel, si));
            let s = &map.surfaces[si];
            for k in 0..3 {
                lo[k] = lo[k].min(s.mins[k]);
                hi[k] = hi[k].max(s.maxs[k]);
            }
        }
        models.push(Model {
            mins: lo,
            maxs: hi,
            first_soup: first,
            num_soups: r.len() as u32,
            first_brush: 0,
            num_brushes: 0,
            partitions: (0, 0),
        });
    }

    // The cells: each tree in preorder, an inner node holding no soups of
    // its own (only a leaf's range is drawn), the cull groups after the
    // map's own, one per static model a cell's trees list.
    let mut aabb_nodes = Vec::new();
    let mut cells = Vec::with_capacity(map.cells.len());
    let mut portals = Vec::new();
    let mut portal_verts = Vec::new();
    let mut planes: Vec<Plane> = map
        .dpvs_planes
        .iter()
        .map(|p| Plane {
            normal: p.normal,
            dist: p.dist,
        })
        .collect();
    let mut cull_groups: Vec<CullGroup> = map
        .cull_groups
        .iter()
        .map(|g| CullGroup {
            mins: g.mins,
            maxs: g.maxs,
            first_soup: if g.surface_count == 0 {
                0
            } else {
                g.start_surf_index
            },
            soup_count: g.surface_count,
        })
        .collect();

    // Static models: each placement's LODs as world-space soups, LOD 0's
    // first, one cull group a placement over them all; the frame keeps one
    // LOD's (`StaticLod`).
    let mut smodel_group: Vec<Option<u32>> = vec![None; map.static_models.len()];
    let mut static_lods = Vec::new();
    for (i, sm) in map.static_models.iter().enumerate() {
        let Some(mesh) = sm.mesh.map(|m| &map.models[m]) else {
            continue;
        };
        let first_soup = soups.len() as u32;
        let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
        let mut lods = Vec::with_capacity(mesh.lod_dists.len());
        for (lod, surfaces) in std::iter::once(&mesh.surfaces)
            .chain(&mesh.coarser)
            .enumerate()
        {
            let lod_first = soups.len() as u32;
            for s in surfaces {
                let first_vertex = verts.len() as u32;
                let first_index = indices.len() as u32;
                for v in &s.vertices {
                    let p = v.pos.map(|c| c * sm.scale);
                    let n = v.normal;
                    let mut pos = sm.origin;
                    let mut normal = [0.0; 3];
                    for k in 0..3 {
                        pos[k] +=
                            p[0] * sm.axis[0][k] + p[1] * sm.axis[1][k] + p[2] * sm.axis[2][k];
                        normal[k] =
                            n[0] * sm.axis[0][k] + n[1] * sm.axis[1][k] + n[2] * sm.axis[2][k];
                        lo[k] = lo[k].min(pos[k]);
                        hi[k] = hi[k].max(pos[k]);
                    }
                    verts.push(DrawVert {
                        pos,
                        uv: v.uv,
                        lm_uv: block_centre(i, map.static_models.len()),
                        normal,
                        color: v.color,
                    });
                    let t = v.tangent;
                    let tangent: [f32; 3] = std::array::from_fn(|k| {
                        t[0] * sm.axis[0][k] + t[1] * sm.axis[1][k] + t[2] * sm.axis[2][k]
                    });
                    let b = [
                        normal[1] * tangent[2] - normal[2] * tangent[1],
                        normal[2] * tangent[0] - normal[0] * tangent[2],
                        normal[0] * tangent[1] - normal[1] * tangent[0],
                    ]
                    .map(|c| c * v.binormal_sign);
                    tangents.push([tangent, b]);
                }
                ensure!(
                    s.indices.len() <= usize::from(u16::MAX)
                        && s.vertices.len() <= usize::from(u16::MAX),
                    "{}: a surface of {} indices",
                    mesh.name,
                    s.indices.len()
                );
                indices.extend_from_slice(&s.indices);
                soup_probes.push(sm.reflection_probe);
                soup_lights.push(sm.primary_light);
                soup_order.push(order_of(
                    List::StaticModel,
                    s.material,
                    sm.primary_light,
                    sm.reflection_probe,
                    i as u32,
                ));
                soups.push(TriangleSoup {
                    material: s.material as u16,
                    lightmap: mapworld::NO_LIGHTMAP,
                    first_vertex,
                    vertex_count: s.vertices.len() as u16,
                    index_count: s.indices.len() as u16,
                    first_index,
                });
                built.static_soups += 1;
            }
            lods.push((
                mesh.lod_dists[lod],
                lod_first,
                soups.len() as u32 - lod_first,
            ));
        }
        static_lods.push(StaticLod {
            origin: sm.origin,
            scale: sm.scale,
            cull_dist: sm.cull_dist,
            lods,
        });
        smodel_group[i] = Some(cull_groups.len() as u32);
        cull_groups.push(CullGroup {
            mins: lo,
            maxs: hi,
            first_soup,
            soup_count: soups.len() as u32 - first_soup,
        });
    }
    debug_assert_eq!(tangents.len(), verts.len());

    let mut cull_indices = Vec::new();
    for c in &map.cells {
        let first_aabb = aabb_nodes.len() as u32;
        if !c.aabb_trees.is_empty() {
            preorder(&c.aabb_trees, 0, &mut aabb_nodes, 0)?;
        }
        let first_portal = portals.len() as u32;
        for p in &c.portals {
            let cell = p.cell.expect("checked by the map build");
            // `DpvsPlane` is `coeffs . (p, 1)`, facing out of the owning
            // cell as vis wants.
            planes.push(Plane {
                normal: [p.plane[0], p.plane[1], p.plane[2]],
                dist: -p.plane[3],
            });
            portals.push(Portal {
                plane: planes.len() as u32 - 1,
                cell,
                first_vert: portal_verts.len() as u32,
                vert_count: p.vertices.len() as u32,
            });
            portal_verts.extend_from_slice(&p.vertices);
        }
        let first_cull_index = cull_indices.len() as u32;
        cull_indices.extend(c.cull_groups.iter().map(|&g| g as u32));
        let mut listed: Vec<u32> = c
            .aabb_trees
            .iter()
            .flat_map(|t| t.smodel_indexes.iter())
            .filter_map(|&m| smodel_group.get(m as usize).copied().flatten())
            .collect();
        listed.sort_unstable();
        listed.dedup();
        cull_indices.extend(listed);
        cells.push(Cell {
            mins: c.mins,
            maxs: c.maxs,
            first_aabb,
            first_portal,
            portal_count: portals.len() as u32 - first_portal,
            first_cull_index,
            cull_count: cull_indices.len() as u32 - first_cull_index,
            first_occluder: 0,
            occluder_count: 0,
        });
    }

    let (nodes, leafs) = cell_tree(map)?;
    let (lightmap_basis, lightmaps) = pages(lightmaps, &tiles)?;
    built.tiles = tiles.len();
    let sun = map.sun().map(|s| MapSun {
        dir: s.dir,
        colour: s.color,
    });
    Ok((
        MapWorld {
            materials,
            lightmaps,
            lightmap_basis,
            soups,
            verts,
            tangents,
            indices,
            entities: map.entity_string.clone(),
            cull_groups,
            cull_indices,
            portal_verts,
            aabb_nodes,
            cells,
            portals,
            portals_only: true,
            // `r_portalMinClipArea` and `r_portalMinRecurseDepth` at their
            // 1.7 defaults (cod4-world.md 9.9)
            portal_min_clip_area: 0.02,
            portal_min_recurse_depth: 2,
            static_lods,
            planes,
            nodes,
            leafs,
            models,
            sun,
            soup_lights,
            soup_probes,
            soup_order,
            sun_casters: map
                .materials
                .iter()
                .map(|m| {
                    let st = m.caster_state?;
                    Some(SunCaster {
                        alpha_test: st.alpha_test.is_some(),
                        image: match crate::materials::color_image(m) {
                            vcod_common::material::ImageRef::Path(p) => p,
                            _ => String::new(),
                        },
                    })
                })
                .collect(),
            // square power-of-two spectra only, as `water_t`'s FFT takes
            waters: map
                .materials
                .iter()
                .filter_map(|m| {
                    let w = m.water.as_ref()?;
                    (w.m == w.n && w.m.is_power_of_two()).then(|| {
                        let s = mapworld::WaterSpectrum {
                            width: w.m,
                            h0: w.h0.clone(),
                            omega: w.w_term.clone(),
                        };
                        (m.name.clone(), s)
                    })
                })
                .collect(),
            ..MapWorld::default()
        },
        built,
    ))
}

/// Blocks along a row of the model lighting volume: its 256 texels.
const VOLUME_ROW: usize = 64;

/// The model lighting volume's size for `count` static models: 256
/// texels across, four rows of texels a row of blocks, at least one.
fn volume_size(count: usize) -> (u32, u32) {
    (
        4 * VOLUME_ROW as u32,
        4 * count.div_ceil(VOLUME_ROW).max(1) as u32,
    )
}

/// Static model `i`'s block centre in the volume's x and y, retail's
/// `R_SetModelLightingCoords` for entry `i` (`baseLightingCoords`; its z
/// is 0.5, the slices' middle).
pub fn block_centre(i: usize, count: usize) -> [f32; 2] {
    let (w, h) = volume_size(count);
    [
        ((4 * (i % VOLUME_ROW)) as f32 + 2.0) / w as f32,
        ((4 * (i / VOLUME_ROW)) as f32 + 2.0) / h as f32,
    ]
}

/// The model lighting volume of `map`'s static models, model `i` in
/// block `i` ([`block_centre`]): a ground lit model's packed colour, else
/// the light grid at its lighting origin, a `needsTrace` corner kept only
/// where `blocked` finds the line from the origin clear (cod4-world.md
/// 3.2). `None` without a grid.
pub fn model_lighting(
    map: &Map,
    blocked: &(dyn Fn([f32; 3], [f32; 3]) -> bool + Sync),
) -> Option<mapworld::ModelLightVolume> {
    let grid = map.light_grid.as_ref()?;
    let n = map.static_models.len();
    let (w, h) = volume_size(n);
    let mut rgba = vec![0u8; (w * h * 4 * 4) as usize];
    let blocks: Vec<[[u8; 4]; 64]> =
        vcod_common::laps::par_map(&(0..n).collect::<Vec<_>>(), |&i: &usize| {
            let sm = &map.static_models[i];
            if sm.ground_lighting != [0; 4] {
                crate::lightgrid::ground_texels(sm.ground_lighting)
            } else {
                let l = grid.lighting_at(sm.lighting_origin, sm.primary_light, blocked);
                crate::lightgrid::block_texels(&l)
            }
        });
    for (i, texels) in blocks.iter().enumerate() {
        let (bx, by) = (4 * (i % VOLUME_ROW), 4 * (i / VOLUME_ROW));
        for (t, px) in texels.iter().enumerate() {
            let (x, y, z) = (t & 3, (t >> 2) & 3, t >> 4);
            let row = z * h as usize + by + y;
            let at = 4 * (row * w as usize + bx + x);
            rgba[at..at + 4].copy_from_slice(px);
        }
    }
    Some(mapworld::ModelLightVolume {
        width: w,
        height: h,
        rgba,
    })
}

/// The lookup an entity's block comes from (`R_AllocModelLighting`,
/// cod4-world.md 3.3): the grid at its lighting origin, the corners
/// traced through `blocked`, the probe nearest it in its cell and the
/// first spot or omni light reaching the model's box
/// (`primarylight::PrimaryLights::for_box`), which the lookup takes where
/// its corners vote a non-sun light; the entities' mode 1 reads as a
/// static model's mode 0 while `r_showMissingLightGrid` is off.
pub fn entity_lighter(
    map: &Map,
    blocked: std::sync::Arc<
        dyn Fn([f32; 3], [f32; 3]) -> bool + Send + Sync + std::panic::RefUnwindSafe,
    >,
) -> Option<mapworld::EntityLighter> {
    let grid = std::sync::Arc::new(map.light_grid.clone()?);
    let probes = map.probe_finder();
    let lights = crate::primarylight::PrimaryLights::of(map);
    Some(mapworld::EntityLighter(std::sync::Arc::new(
        move |p, [mins, maxs]: [[f32; 3]; 2]| {
            // R_AllocModelLighting_Box (0x62eeb0): the box's mid point and
            // half size
            let mid = [0, 1, 2].map(|k| (mins[k] + maxs[k]) * 0.5);
            let half = [0, 1, 2].map(|k| mid[k] - mins[k]);
            let l = grid.lighting_at(p, lights.for_box(mid, half), &*blocked);
            mapworld::EntityLight {
                texels: crate::lightgrid::block_texels(&l),
                probe: probes.at(p),
                primary: l.primary_light,
            }
        },
    )))
}

/// Each primary light's shader constants (`R_SetLightProperties`, 1.7
/// 0x649200, cod4-world.md 3.5): the spot and omni lights past the sun,
/// `attenuation` naming a light def's attenuation row; `None` for the
/// sun, light 0 and a light of another type.
pub fn model_lights(
    map: &Map,
    attenuation: &dyn Fn(&str) -> Option<Vec<[u8; 4]>>,
) -> Vec<Option<mapworld::ModelLightSource>> {
    map.primary_lights
        .iter()
        .enumerate()
        .map(|(i, l)| {
            let spot = l.kind == crate::primarylight::SPOT;
            if i as u32 <= map.sun_light_index || !(spot || l.kind == 3) {
                return None;
            }
            let scale = 1.0 / (l.cos_half_fov_inner - l.cos_half_fov_outer);
            Some(mapworld::ModelLightSource {
                spot,
                can_shadow: l.can_shadow,
                origin: l.origin,
                inv_radius: 1.0 / l.radius,
                colour: l.color,
                dir: l.dir,
                spot_factors: [scale, -scale * l.cos_half_fov_outer, f32::from(l.exponent)],
                attenuation: l.def.as_deref().and_then(attenuation).unwrap_or_default(),
            })
        })
        .collect()
}

/// Each brush model's surface range past the world's (`GfxBrushModel`
/// `startSurfIndex`, `surfaceCount`); model 0 is the world itself.
fn brush_model_ranges(map: &Map) -> Vec<std::ops::Range<usize>> {
    map.brush_models
        .iter()
        .skip(1)
        .map(|&(start, count)| {
            if count == 0 {
                0..0
            } else {
                start as usize..start as usize + count as usize
            }
        })
        .collect()
}

/// Appends tree `i` of a cell's array and its children in preorder.
fn preorder(
    trees: &[crate::assets::world::GfxAabbTree],
    i: usize,
    out: &mut Vec<AabbNode>,
    depth: usize,
) -> Result<()> {
    ensure!(depth < 128, "cell tree deeper than 128");
    let t = &trees[i];
    let leaf = t.child_count == 0;
    out.push(AabbNode {
        first_soup: if leaf {
            u32::from(t.start_surf_index)
        } else {
            0
        },
        soup_count: if leaf { u32::from(t.surface_count) } else { 0 },
        child_count: u32::from(t.child_count),
    });
    if !leaf {
        let first = i + t.children_offset as usize / 0x2C;
        for c in first..first + t.child_count as usize {
            preorder(trees, c, out, depth + 1)?;
        }
    }
    Ok(())
}

/// `dpvsPlanes.nodes` as a BSP tree of [`Node`]s over the dpvs planes,
/// each leaf a [`Leaf`] naming its cell (-1 for none): what
/// `WorldVis::cell_for_point` walks, the same descent as
/// [`Map::cell_at`].
fn cell_tree(map: &Map) -> Result<(Vec<Node>, Vec<Leaf>)> {
    let cells = map.cells.len();
    let n = &map.dpvs_nodes;
    let mut nodes: Vec<Node> = Vec::new();
    let mut leafs: Vec<Leaf> = Vec::new();
    let mut leaf_of: HashMap<i32, i32> = HashMap::new();
    let mut leaf = |cell: i32, leafs: &mut Vec<Leaf>| -> i32 {
        *leaf_of.entry(cell).or_insert_with(|| {
            leafs.push(Leaf {
                cluster: if cell < 0 { -1 } else { 0 },
                area: if cell < 0 { -1 } else { 0 },
                cell,
            });
            -(leafs.len() as i32)
        })
    };
    // (entry, parent node, side) still to place
    let Some(&root) = n.first() else {
        return Ok((nodes, leafs));
    };
    if usize::from(root) <= cells {
        // a tree that is one leaf: a node whose sides both name it
        let l = leaf(i32::from(root) - 1, &mut leafs);
        nodes.push(Node {
            plane: 0,
            children: [l, l],
            mins: [0; 3],
            maxs: [0; 3],
        });
        return Ok((nodes, leafs));
    }
    let mut stack: Vec<(usize, Option<(usize, usize)>)> = vec![(0, None)];
    let mut steps = 0;
    while let Some((k, parent)) = stack.pop() {
        steps += 1;
        ensure!(steps <= 2 * n.len() + 2, "cell tree revisits its nodes");
        let Some(&v) = n.get(k) else {
            bail!("cell tree entry {k} past {}", n.len());
        };
        let v = usize::from(v);
        let child = if v <= cells {
            leaf(v as i32 - 1, &mut leafs)
        } else {
            let plane = v - (cells + 1);
            ensure!(plane < map.dpvs_planes.len(), "cell tree plane {plane}");
            let off = *n
                .get(k + 1)
                .ok_or_else(|| anyhow::anyhow!("cell tree node at the end"))?;
            nodes.push(Node {
                plane: plane as u32,
                children: [0, 0],
                mins: [0; 3],
                maxs: [0; 3],
            });
            let me = nodes.len() - 1;
            stack.push((k + usize::from(off), Some((me, 1))));
            stack.push((k + 2, Some((me, 0))));
            me as i32
        };
        if let Some((p, side)) = parent {
            nodes[p].children[side] = child;
        }
    }
    Ok((nodes, leafs))
}
