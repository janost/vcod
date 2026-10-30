//! Every stock MP map zone (`zone/english/mp_*.ff`, the `_load` zones
//! aside) built into a [`Map`]: every surface's indices, every cell's
//! portals and trees and every collision index in range (the build fails
//! otherwise), every world material's textures naming an image, and every
//! image the map names decoding from the zone's pixels or the iwds' `.iwi`,
//! or listed in [`CODE_IMAGES`]. Prints a line per map and the techsets
//! the classifier leaves in [`Family::Other`]; `--nocapture` shows them.
//! `COD4_MAPS=a,b` keeps only maps whose name holds one of those words.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;
use vcod_cod4::map::Map;
use vcod_cod4::source::{Cod4Source, ImageOrigin};
use vcod_cod4::techset::Family;
use vcod_cod4::testing::cod4_install;

/// Images a stock map names that neither its zone nor the iwds carry:
/// the water materials' `watersetup*`, made at run time from their
/// `water_t`.
const CODE_IMAGES: [&str; 2] = ["watersetup0", "watersetup1"];

#[test]
fn every_stock_map_builds_and_resolves() {
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
    // Decoded once across maps: name -> what the decode said.
    let mut decoded: HashMap<String, Result<(), String>> = HashMap::new();
    let mut other: BTreeMap<String, usize> = BTreeMap::new();
    let mut missing: BTreeSet<String> = BTreeSet::new();
    let mut failed: Vec<String> = Vec::new();
    for name in &maps {
        let (map, walked) = Map::load(&install, "english", name).unwrap();
        assert_eq!(map.bsp_name, format!("maps/mp/{name}.d3dbsp"));
        assert!(!map.surfaces.is_empty() && !map.cells.is_empty());
        assert_eq!(
            map.entities.first().and_then(|e| {
                e.iter()
                    .find(|(k, _)| k == "classname")
                    .map(|(_, v)| v.as_str())
            }),
            Some("worldspawn"),
            "{name}"
        );
        let mut used = vec![false; map.materials.len()];
        for s in &map.surfaces {
            used[s.material as usize] = true;
        }
        for (m, _) in map.materials.iter().zip(&used).filter(|(_, u)| **u) {
            for t in &m.textures {
                assert!(
                    t.image.is_some(),
                    "{name}: {} has a texture without an image",
                    m.name
                );
            }
            let class = m
                .class
                .unwrap_or_else(|| panic!("{name}: {} has no techset", m.name));
            if class.family == Family::Other {
                *other.entry(m.techset.clone().unwrap()).or_default() += 1;
            }
        }
        src.add_zone(Arc::new(walked.assets));
        let (mut zone, mut iwd) = (0, 0);
        for img in map.image_names() {
            match src.image_origin(img) {
                Some(ImageOrigin::Zone) => zone += 1,
                Some(ImageOrigin::Iwd) => iwd += 1,
                None => {
                    missing.insert(img.to_owned());
                    continue;
                }
            }
            let r = decoded
                .entry(img.to_ascii_lowercase())
                .or_insert_with(|| src.image(img).map(|_| ()).map_err(|e| format!("{e:#}")));
            if let Err(e) = r {
                failed.push(format!("{name}: {img}: {e}"));
            }
        }
        src.pop_zone();
        let c = &map.collision;
        println!(
            "{name}: {} vertices, {} surfaces, {} materials, {} cells, {} portals, {} static models, {} entities; images {zone} zone {iwd} iwd; clip {} brushes {} leafs {} partitions, {} indices checked",
            map.vertices.len(),
            map.surfaces.len(),
            map.materials.len(),
            map.cells.len(),
            map.cells.iter().map(|c| c.portals.len()).sum::<usize>(),
            map.static_models.len(),
            map.entities.len(),
            c.brushes,
            c.leafs,
            c.partitions,
            c.checked
        );
    }
    println!("techsets left in Other (world materials using them): {other:?}");
    println!("images decoded: {}", decoded.len());
    assert!(failed.is_empty(), "{failed:#?}");
    let expected: BTreeSet<String> = CODE_IMAGES.iter().map(|s| s.to_string()).collect();
    assert!(missing.is_subset(&expected), "images nowhere: {missing:?}");
    if filter.is_empty() {
        assert_eq!(missing, expected);
    }
}
