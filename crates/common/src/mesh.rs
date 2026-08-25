use crate::bsp::{Bsp, Material};

pub struct Batch {
    pub material: u16,
    pub lightmap: u16,
    pub first_index: u32,
    pub index_count: u32,
}

/// Gameplay-only surfaces (clip, caulk) and sky; the sky reads as the clear color.
pub fn should_skip(material: &Material) -> bool {
    let n = material.name.as_str();
    n.starts_with("textures/common/") || n.starts_with("textures/skies/") || n.contains("sky")
}

/// Coplanar surfaces that need a depth bias, marked by the surface-type
/// prefix of the texture basename (`decal@`, `*_masked@`).
pub fn is_overlay(material: &Material) -> bool {
    let base = material.name.rsplit('/').next().unwrap_or("");
    base.starts_with("decal") || base.split('@').next().unwrap_or("").ends_with("_masked")
}

/// One absolute u32 index buffer, one contiguous batch per (material, lightmap).
pub fn build_batches(bsp: &Bsp) -> (Vec<u32>, Vec<Batch>) {
    use std::collections::BTreeMap;
    let mut groups: BTreeMap<(u16, u16), Vec<u32>> = BTreeMap::new();
    for soup in &bsp.soups {
        if should_skip(&bsp.materials[soup.material as usize]) {
            continue;
        }
        let dst = groups.entry((soup.material, soup.lightmap)).or_default();
        let fi = soup.first_index as usize;
        for &rel in &bsp.indices[fi..fi + soup.index_count as usize] {
            dst.push(soup.first_vertex + rel as u32);
        }
    }
    let mut indices = Vec::new();
    let mut batches = Vec::new();
    for ((material, lightmap), idx) in groups {
        batches.push(Batch {
            material,
            lightmap,
            first_index: indices.len() as u32,
            index_count: idx.len() as u32,
        });
        indices.extend(idx);
    }
    (indices, batches)
}

/// Bounds over the drawn soups' vertices; the spawn fallback.
pub fn map_bounds(bsp: &Bsp) -> ([f32; 3], [f32; 3]) {
    let mut min = [f32::INFINITY; 3];
    let mut max = [f32::NEG_INFINITY; 3];
    for soup in &bsp.soups {
        if should_skip(&bsp.materials[soup.material as usize]) {
            continue;
        }
        let fv = soup.first_vertex as usize;
        for v in &bsp.verts[fv..fv + soup.vertex_count as usize] {
            for a in 0..3 {
                min[a] = min[a].min(v.pos[a]);
                max[a] = max[a].max(v.pos[a]);
            }
        }
    }
    (min, max)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bsp::{Bsp, DrawVert, Material, TriangleSoup};

    fn mat(name: &str) -> Material {
        Material {
            name: name.into(),
            surface_flags: 0,
            content_flags: 0,
        }
    }

    fn vert(x: f32) -> DrawVert {
        DrawVert {
            pos: [x, 0.0, 0.0],
            uv: [0.0; 2],
            lm_uv: [0.0; 2],
            normal: [0.0, 0.0, 1.0],
            color: [255; 4],
        }
    }

    fn test_bsp() -> Bsp {
        Bsp {
            materials: vec![
                mat("textures/wall"),
                mat("textures/common/clip"),
                mat("textures/floor"),
            ],
            lightmaps: vec![],
            // soups 0 and 2 share (material 0, lightmap 0); soup 1 is clip
            soups: vec![
                TriangleSoup {
                    material: 0,
                    lightmap: 0,
                    first_vertex: 0,
                    vertex_count: 3,
                    index_count: 3,
                    first_index: 0,
                },
                TriangleSoup {
                    material: 1,
                    lightmap: 0,
                    first_vertex: 3,
                    vertex_count: 3,
                    index_count: 3,
                    first_index: 3,
                },
                TriangleSoup {
                    material: 0,
                    lightmap: 0,
                    first_vertex: 6,
                    vertex_count: 3,
                    index_count: 3,
                    first_index: 6,
                },
                TriangleSoup {
                    material: 2,
                    lightmap: 1,
                    first_vertex: 9,
                    vertex_count: 3,
                    index_count: 3,
                    first_index: 9,
                },
            ],
            // clip verts (3..6) sit far outside the drawn range
            verts: (0..12)
                .map(|i| {
                    vert(if (3..6).contains(&i) {
                        100.0 + (i - 3) as f32
                    } else {
                        i as f32
                    })
                })
                .collect(),
            indices: vec![0, 1, 2, 0, 1, 2, 0, 1, 2, 0, 2, 1],
            entities: String::new(),
            planes: vec![],
            brush_sides: vec![],
            brushes: vec![],
            models: vec![],
        }
    }

    #[test]
    fn skips_common_and_sky() {
        assert!(should_skip(&mat("textures/common/clip_nosight")));
        assert!(should_skip(&mat("textures/common/caulk")));
        assert!(should_skip(&mat("textures/skies/stalingrad_sky")));
        assert!(!should_skip(&mat("textures/normandy/walls/brick")));
    }

    #[test]
    fn overlays_detected_by_surface_type_prefix() {
        assert!(is_overlay(&mat(
            "textures/normandy/windows/decal@churchwindow"
        )));
        assert!(is_overlay(&mat(
            "textures/normandy/uniques/decal_clampx@dirtroad"
        )));
        assert!(is_overlay(&mat(
            "textures/normandy/transparents/wood_masked@rooflattice1"
        )));
        assert!(is_overlay(&mat(
            "textures/decals/metal_masked@churchcross_iron"
        )));
        // the directory does not count
        assert!(!is_overlay(&mat(
            "textures/normandy/windows/wood@shutter4a"
        )));
        assert!(!is_overlay(&mat("textures/decals/wood@crate1")));
    }

    #[test]
    fn merges_batches_and_rebases_indices() {
        let (indices, batches) = build_batches(&test_bsp());
        assert_eq!(batches.len(), 2);
        let b0 = &batches[0];
        assert_eq!((b0.material, b0.lightmap, b0.index_count), (0, 0, 6));
        // soup 0 rebased by 0, soup 2 rebased by 6
        let range0 = &indices[b0.first_index as usize..(b0.first_index + b0.index_count) as usize];
        assert_eq!(range0, &[0, 1, 2, 6, 7, 8]);
        let b1 = &batches[1];
        assert_eq!((b1.material, b1.lightmap, b1.index_count), (2, 1, 3));
        let range1 = &indices[b1.first_index as usize..(b1.first_index + b1.index_count) as usize];
        assert_eq!(range1, &[9, 11, 10]);
        assert_eq!(indices.len(), 9); // clip soup contributed nothing
    }

    #[test]
    fn bounds_ignore_skipped_materials() {
        let (min, max) = map_bounds(&test_bsp());
        assert_eq!(min[0], 0.0);
        assert_eq!(max[0], 11.0);
    }
}
