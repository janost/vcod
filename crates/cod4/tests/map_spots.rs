//! mp_crash built into a [`Map`], against values read out of its zone
//! (`docs/research/cod4-world.md` section 7). `--nocapture` prints them.

use std::sync::Arc;
use vcod_cod4::map::{Map, NO_LIGHTMAP};
use vcod_cod4::source::{Cod4Source, ImageOrigin};
use vcod_cod4::techset::{Blend, Family};
use vcod_cod4::testing::cod4_install;
use vcod_common::assets::ImageData;

#[test]
fn mp_crash_spot_values() {
    let Some(install) = cod4_install() else {
        return;
    };
    let (m, walked) = Map::load(&install, "english", "mp_crash").unwrap();
    assert_eq!(m.bsp_name, "maps/mp/mp_crash.d3dbsp");
    assert_eq!(
        (m.vertices.len(), m.indices.len(), m.surfaces.len()),
        (166418, 347493, 5558)
    );
    // the world's 134 materials, then the 145 more its static models' LODs
    // add (one only a coarser LOD names)
    assert_eq!((m.materials.len(), m.cells.len()), (279, 23));
    let portals: usize = m.cells.iter().map(|c| c.portals.len()).sum();
    assert_eq!(portals, 180);
    assert_eq!(m.static_models.len(), 4038);
    assert_eq!(
        (m.mins, m.maxs),
        ([-4160.0, -4736.0, 0.0], [4864.0, 5632.0, 2176.0])
    );

    let v = m.vertices[0];
    println!("vertex 0: {v:?}");
    assert_eq!(v.pos, [613.3779, 1089.5088, 303.0]);
    assert_eq!((v.binormal_sign, v.color), (-1.0, [255; 4]));
    assert_eq!(
        (v.uv, v.lm_uv),
        ([-0.63589096, 5.646454], [0.8318712, 0.23321533])
    );
    let len = |n: [f32; 3]| (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
    for v in &m.vertices {
        assert!((len(v.normal) - 1.0).abs() < 0.01 && (len(v.tangent) - 1.0).abs() < 0.01);
    }

    let s = m.surfaces[0];
    println!("surface 0: {s:?}");
    assert_eq!(
        (s.first_vertex, s.vertex_count, s.tri_count, s.base_index),
        (0, 8083, 14, 214692)
    );
    assert_eq!(s.lightmap, 0);
    let mat = &m.materials[s.material as usize];
    println!("surface 0 material: {mat:?}");
    assert_eq!(mat.name, "wc/me_wire_black");
    assert_eq!(mat.techset.as_deref(), Some("wc_l_sm_r0c0"));
    let class = mat.class.unwrap();
    assert_eq!((class.family, class.blend), (Family::Lit, Blend::Replace));
    let first = m.triangles(&s).next().unwrap();
    assert!(
        first
            .iter()
            .all(|&i| i < s.first_vertex + u32::from(s.vertex_count))
    );
    let unlit = m
        .surfaces
        .iter()
        .filter(|s| s.lightmap == NO_LIGHTMAP)
        .count();
    assert_eq!(unlit, 47);

    let sm = &m.static_models[0];
    println!("static model 0: {sm:?}");
    assert_eq!(sm.model.as_deref(), Some("me_corrugated_metal4x8"));
    assert_eq!((sm.origin, sm.scale), ([1332.0, -14.0, 292.5], 1.0));

    assert_eq!(m.entities.len(), 309);
    let spawns = m
        .entities
        .iter()
        .filter(|e| {
            e.iter()
                .any(|(k, v)| k == "classname" && v == "mp_tdm_spawn")
        })
        .count();
    assert_eq!(spawns, 19);

    let c = &m.collision;
    println!(
        "collision: {} materials, {} leaf brush nodes, {} leaf brushes, {} aabb trees, {} tris, max cluster {}, dyn ents {:?}",
        c.materials.len(),
        c.leaf_brush_nodes,
        c.leaf_brushes,
        c.aabb_trees,
        c.tris,
        c.max_cluster,
        c.dyn_ents
    );
    assert_eq!(
        (c.planes, c.brushes, c.brush_sides, c.nodes, c.leafs),
        (11400, 10978, 21774, 2130, 2165)
    );
    assert_eq!(
        (c.submodels, c.static_models, c.partitions, c.clusters),
        (34, 4038, 1028, 1)
    );
    assert_eq!(
        (c.leaf_brush_nodes, c.leaf_brushes, c.aabb_trees),
        (12075, 19627, 2508)
    );
    assert_eq!(
        (c.verts, c.tris, c.max_cluster, c.dyn_ents),
        (10220, 14977, 947, [516, 0])
    );
    assert_eq!(c.checked, 296199);

    // The lightmap's two images ride the zone; a colour map is an iwi.
    assert_eq!(
        m.lightmaps,
        [[
            Some("*lightmap0_primary".into()),
            Some("*lightmap0_secondary".into())
        ]]
    );
    // The clip map's planes slot is an offset into the render world's.
    let (g, cm) = (
        &walked.assets.gfx_worlds[0],
        &walked.assets.clip_maps_pvs[0],
    );
    assert!(g.planes.is_some() && g.planes == cm.planes);
    let mut src = Cod4Source::open(&install).unwrap();
    src.add_zone(Arc::new(walked.assets));
    let primary = src.image("*lightmap0_primary").unwrap();
    let secondary = src.image("*lightmap0_secondary").unwrap();
    assert_eq!((primary.width, primary.height), (1024, 2048));
    assert_eq!((secondary.width, secondary.height), (512, 2048));
    let colour = mat
        .textures
        .iter()
        .find(|t| t.sampler == Some("colorMap"))
        .unwrap();
    let name = colour.image.as_deref().unwrap();
    assert_eq!(src.image_origin(name), Some(ImageOrigin::Iwd));
    let img = src.image(name).unwrap();
    assert_eq!((name, img.width, img.height), ("me_wire_black", 256, 32));
    assert!(matches!(img.data, ImageData::Bc { .. }));
}

/// mp_crash's doorway lighteners (`wc/hdrportal_lighten`, `distfalloff`):
/// their constants, and texture coordinates that hold one xy point per
/// triangle (its opening's centre), which the falloff measures the eye from
/// (`docs/research/cod4-world.md` 9.4).
#[test]
fn mp_crash_doorway_lighteners() {
    let Some(install) = cod4_install() else {
        return;
    };
    let (m, _) = Map::load(&install, "english", "mp_crash").unwrap();
    let (mi, mat) = m
        .materials
        .iter()
        .enumerate()
        .find(|(_, x)| x.name == "wc/hdrportal_lighten")
        .unwrap();
    assert_eq!(mat.techset.as_deref(), Some("wc_unlit_distfalloff"));
    assert_eq!(mat.sort_key, 43);
    let f = vcod_cod4::materials::compile(mat).falloff.unwrap();
    println!("{f:?}");
    assert_eq!(
        (f.begin, f.end),
        ([0.811765, 0.811765, 0.803922], [0.498039; 3])
    );
    assert_eq!((f.scale, f.bias), (0.005882353, -0.1764706));
    let mut n = 0;
    for s in m.surfaces.iter().filter(|s| s.material == mi as u32) {
        n += 1;
        for t in m.triangles(s) {
            let uv = t.map(|i| m.vertices[i as usize].uv);
            assert!(uv.iter().all(|&p| p == uv[0]), "{s:?}");
            let inside = |k: usize| (s.mins[k]..=s.maxs[k]).contains(&uv[0][k]);
            assert!(inside(0) && inside(1), "{s:?}");
        }
    }
    assert_eq!(n, 30);
}
