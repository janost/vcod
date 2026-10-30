//! How mp_carentan's and mp_toujane's layered world materials compile
//! (cod2-materials.md 9, "Multiply"): every `effect_multiply*` decal (the
//! dark stains over roofs and walls, toujane's window shadows) multiplies
//! by its colour faded by the vertex alpha, and every `phong_blend*` layer
//! blends SrcAlpha/InvSrcAlpha with the vertex alpha in its source alpha,
//! sorted after the opaque base. Prints the census by technique. Needs
//! `COD2_DIR`; passes vacuously without it.

use std::collections::BTreeMap;
use vcod_cod2::d3dbsp::D3dBsp;
use vcod_cod2::material::{MaterialFile, TechniqueSet};
use vcod_cod2::materials;
use vcod_common::material::{AlphaGen, BlendFactor, SORT_OPAQUE};

#[test]
fn layered_world_materials_blend_by_vertex_alpha() {
    let Some(fs) = vcod_common::testing::cod2_fs() else {
        return;
    };
    let (lib, _) = materials::compile_all(&fs).unwrap();
    for map in ["mp_carentan", "mp_toujane"] {
        let d = fs.read(&format!("maps/mp/{map}.d3dbsp")).unwrap();
        let bsp = D3dBsp::parse(&d).unwrap();
        let mut tris: BTreeMap<usize, usize> = BTreeMap::new();
        for s in &bsp.surfaces {
            *tris.entry(s.material as usize).or_default() += s.index_count as usize / 3;
        }
        // (technique, blend) -> (materials, triangles, reads vertex alpha)
        let mut census: BTreeMap<(String, String), (usize, usize, bool)> = BTreeMap::new();
        let (mut multiply, mut layers) = (0, 0);
        for (&i, &n) in &tris {
            let name = &bsp.materials[i].name;
            let raw = fs
                .read(&format!("materials/{}", name.to_lowercase()))
                .unwrap();
            let file = MaterialFile::parse(&raw).unwrap();
            let set = fs
                .read(&format!(
                    "materials/techniquesets/{}.techset",
                    file.technique_set
                ))
                .unwrap();
            let set = TechniqueSet::parse(&String::from_utf8_lossy(&set)).unwrap();
            let Some((_, technique)) = materials::pick_technique(&set) else {
                continue;
            };
            let m = lib.get(name).unwrap();
            let Some(stage) = m.stages.first() else {
                continue;
            };
            let blend = stage.blend.clone();
            let vertex_alpha = m.vertex_multiply
                || m.phong.is_some_and(|p| p.alpha)
                || (m.phong.is_none()
                    && stage.alpha_gen == AlphaGen::Vertex
                    && blend.as_ref().is_some_and(|b| b.0 == BlendFactor::SrcAlpha));
            if file.technique_set.starts_with("effect_multiply") {
                assert!(m.vertex_multiply, "{map} {name}");
                assert_eq!(
                    blend,
                    Some((BlendFactor::Zero, BlendFactor::SrcColor)),
                    "{map} {name}"
                );
                assert!(m.sort > SORT_OPAQUE, "{map} {name}");
                multiply += n;
            }
            if file.technique_set.starts_with("phong_blend") {
                assert!(technique.starts_with("lmap_a"), "{map} {name} {technique}");
                assert!(m.phong.is_some_and(|p| p.alpha), "{map} {name}");
                assert_eq!(stage.alpha_gen, AlphaGen::Vertex, "{map} {name}");
                assert_eq!(
                    blend,
                    Some((BlendFactor::SrcAlpha, BlendFactor::OneMinusSrcAlpha)),
                    "{map} {name}"
                );
                assert!(m.sort > SORT_OPAQUE && !m.vertex_multiply, "{map} {name}");
                layers += n;
            }
            let e = census
                .entry((technique.to_string(), format!("{blend:?}")))
                .or_insert((0, 0, vertex_alpha));
            assert_eq!(e.2, vertex_alpha, "{map} {name}");
            e.0 += 1;
            e.1 += n;
        }
        println!("{map}:");
        for ((t, blend), (mats, n, va)) in &census {
            println!("  {t:16} {mats:3} materials {n:6} tris  blend {blend}  vertex alpha {va}");
        }
        assert!(multiply > 0 && layers > 0, "{map}: {multiply} {layers}");
    }
}
