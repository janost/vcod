//! Every stock MP map (`zone/english/mp_*.ff`, `_load` aside) through
//! `vcod_cod4::mapworld::to_mapworld`: the conversion succeeds and every
//! index the renderer and the cell vis follow stays in range (soups'
//! vertices and indices, materials, lightmap pages, cell trees, portals,
//! cull groups, the cell lookup tree, brush models), no surface is reached
//! twice, and the dpvs walk ([`Map::cell_at`]) and the converted tree
//! agree on a sample of points. Prints a line per map.
//! `COD4_MAPS=a,b` keeps only maps whose name holds one of those words.

use std::sync::Arc;
use vcod_cod4::map::Map;
use vcod_cod4::mapworld::{LightmapImages, to_mapworld};
use vcod_cod4::source::Cod4Source;
use vcod_cod4::testing::cod4_install;
use vcod_common::mapworld::{LIGHTMAP_SIZE, MapWorld, NO_LIGHTMAP};
use vcod_common::vis::WorldVis;

/// Every index `w` holds checked against what it names.
fn check(name: &str, w: &MapWorld) {
    for (i, s) in w.soups.iter().enumerate() {
        let (fi, n) = (s.first_index as usize, s.index_count as usize);
        assert!(fi + n <= w.indices.len(), "{name}: soup {i} indices");
        for &k in &w.indices[fi..fi + n] {
            assert!(k < s.vertex_count, "{name}: soup {i} index {k}");
        }
        assert!(
            s.first_vertex as usize + s.vertex_count as usize <= w.verts.len(),
            "{name}: soup {i} vertices"
        );
        assert!(
            (s.material as usize) < w.materials.len(),
            "{name}: soup {i} material"
        );
        assert!(
            s.lightmap == NO_LIGHTMAP || (s.lightmap as usize) < w.lightmap_basis.len(),
            "{name}: soup {i} lightmap {}",
            s.lightmap
        );
    }
    assert_eq!(w.tangents.len(), w.verts.len(), "{name}: tangents");
    assert_eq!(w.lightmaps.len(), w.lightmap_basis.len());
    for p in &w.lightmap_basis {
        assert!(
            p.coeffs
                .iter()
                .all(|c| c.len() == 4 * LIGHTMAP_SIZE * LIGHTMAP_SIZE)
        );
        assert_eq!(p.sun.len(), 4 * LIGHTMAP_SIZE * LIGHTMAP_SIZE);
    }
    let soups = w.soups.len() as u32;
    for (i, n) in w.aabb_nodes.iter().enumerate() {
        assert!(
            n.first_soup + n.soup_count <= soups,
            "{name}: tree node {i}"
        );
    }
    for (i, c) in w.cells.iter().enumerate() {
        assert!(
            (c.first_aabb as usize) < w.aabb_nodes.len(),
            "{name}: cell {i} tree"
        );
        assert!(
            (c.first_portal + c.portal_count) as usize <= w.portals.len(),
            "{name}: cell {i} portals"
        );
        let cull = c.first_cull_index as usize..(c.first_cull_index + c.cull_count) as usize;
        for &g in &w.cull_indices[cull] {
            assert!(
                (g as usize) < w.cull_groups.len(),
                "{name}: cell {i} cull group {g}"
            );
        }
    }
    for g in &w.cull_groups {
        assert!(
            g.first_soup + g.soup_count <= soups,
            "{name}: cull group soups"
        );
    }
    for (i, p) in w.portals.iter().enumerate() {
        assert!((p.cell as usize) < w.cells.len(), "{name}: portal {i} cell");
        assert!(
            (p.plane as usize) < w.planes.len(),
            "{name}: portal {i} plane"
        );
        assert!(
            (p.first_vert + p.vert_count) as usize <= w.portal_verts.len(),
            "{name}: portal {i} vertices"
        );
    }
    for (i, n) in w.nodes.iter().enumerate() {
        assert!(
            (n.plane as usize) < w.planes.len(),
            "{name}: node {i} plane"
        );
        for c in n.children {
            if c < 0 {
                assert!(
                    ((-c - 1) as usize) < w.leafs.len(),
                    "{name}: node {i} leaf {c}"
                );
            } else {
                assert!(
                    c > 0 && (c as usize) < w.nodes.len(),
                    "{name}: node {i} child {c}"
                );
            }
        }
    }
    for l in &w.leafs {
        assert!(
            l.cell < w.cells.len() as i32,
            "{name}: leaf cell {}",
            l.cell
        );
    }
    for m in &w.models {
        assert!(m.first_soup + m.num_soups <= soups, "{name}: model soups");
    }
}

#[test]
fn every_stock_map_converts_with_its_indices_in_range() {
    let Some(install) = cod4_install() else {
        return;
    };
    let filter: Vec<String> = std::env::var("COD4_MAPS")
        .map(|v| v.split(',').map(str::to_owned).collect())
        .unwrap_or_default();
    let mut maps: Vec<String> = install
        .zones("english")
        .unwrap()
        .iter()
        .filter_map(|p| p.file_stem()?.to_str().map(str::to_owned))
        .filter(|n| n.starts_with("mp_") && !n.ends_with("_load"))
        .filter(|n| filter.is_empty() || filter.iter().any(|f| n.contains(f.as_str())))
        .collect();
    maps.sort();
    if filter.is_empty() {
        assert_eq!(maps.len(), 21, "{maps:?}");
    }
    let mut src = Cod4Source::open(&install).unwrap();
    for name in &maps {
        let (map, walked) = Map::load(&install, "english", name).unwrap();
        src.add_zone(Arc::new(walked.assets));
        let lightmaps: Vec<LightmapImages> = map
            .lightmaps
            .iter()
            .map(|[p, s]| LightmapImages {
                primary: src.image(p.as_deref().unwrap()).unwrap(),
                secondary: src.image(s.as_deref().unwrap()).unwrap(),
            })
            .collect();
        // every probe a soup names decodes as a cube of 64 x 64 mipped
        // A8R8G8B8 faces
        for p in &map.reflection_probes {
            let faces = src.zone_cube(p.as_deref().unwrap()).unwrap();
            assert_eq!(faces.len(), 6, "{name}: {p:?}");
            assert!(
                faces.iter().all(|f| f.len() == 7 && f[0].width == 64),
                "{name}: {p:?}"
            );
        }
        src.pop_zone();
        let (w, built) = to_mapworld(&map, &lightmaps).unwrap();
        check(name, &w);
        assert_eq!(w.soup_probes.len(), w.soups.len(), "{name}: soup probes");
        // every water set's material keeps its spectrum
        let water_sets = map
            .materials
            .iter()
            .filter(|m| {
                m.class
                    .is_some_and(|c| c.family == vcod_cod4::techset::Family::Water)
            })
            .count();
        assert_eq!(w.waters.len(), water_sets, "{name}: water spectra");
        for (m, s) in &w.waters {
            assert!(
                s.width >= 4 && s.omega.iter().any(|&o| o > 0.0),
                "{name} {m}"
            );
        }
        let probes = map.reflection_probes.len();
        assert!(
            w.soup_probes.iter().all(|&p| usize::from(p) < probes),
            "{name}: a soup's probe past the map's {probes}"
        );
        assert_eq!(built.repeated_surfaces, 0, "{name}: a surface drawn twice");
        let vis = WorldVis::build(&w);
        let (mut agree, mut sampled) = (0, 0);
        for s in map.surfaces.iter().step_by(5) {
            let p = [0, 1, 2].map(|k| (s.mins[k] + s.maxs[k]) * 0.5);
            sampled += 1;
            if map.cell_at(p) == vis.cell_for_point(p.into()) {
                agree += 1;
            }
        }
        assert_eq!(agree, sampled, "{name}: cell lookups differ");
        let sun = map.sun().map(|s| (s.dir, s.color));
        println!(
            "{name}: {} soups ({} loose, {} static model), {} vertices, {} pages, {} cells, {} portals, {} cell tree nodes, {} cull groups, {probes} probes, sun {sun:?}",
            w.soups.len(),
            built.loose_surfaces,
            built.static_soups,
            w.verts.len(),
            built.tiles,
            w.cells.len(),
            w.portals.len(),
            w.nodes.len(),
            w.cull_groups.len()
        );
    }
}
