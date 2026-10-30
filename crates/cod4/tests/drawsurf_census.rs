//! 1.7's lit draw order against every stock MP map's zone
//! (cod4-world.md 12). The zone stores the world's surfaces sorted by
//! `R_CompareSurfaces` (iw3mp.exe 0x5ff060): within one primary sort key
//! and primary light the surfaces of a material sit together, the
//! materials in `materialSortedIndex` order, and within a material the
//! reflection probe, the lightmap and the first vertex rise (the world's
//! surfaces, model 0's run; the brush models' follow). So the order
//! `vcod_cod4::drawsurf::MaterialOrder` gives (`Material_Compare`,
//! 0x621250) must name the zone's material runs in the zone's order, with
//! the stub technique sets resolved from `common_mp`. Prints a line per
//! map. `COD4_MAPS=a,b` keeps only maps whose name holds one of those
//! words.

use std::collections::BTreeMap;
use vcod_cod4::drawsurf::{MaterialOrder, ranks};
use vcod_cod4::map::{Map, Surface};
use vcod_cod4::testing::cod4_install;
use vcod_cod4::walk::{WalkOptions, walk};
use vcod_cod4::zone::Zone;

#[test]
fn every_stock_map_stores_its_surfaces_in_the_material_order() {
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
    let common = Zone::open_path(&install.zone("english", "common_mp"))
        .and_then(|z| walk(z, WalkOptions { keep_bulk: false }))
        .unwrap();
    for name in &maps {
        let (mut map, walked) = Map::load(&install, "english", name).unwrap();
        map.resolve_material_orders(&[&walked.assets, &common.assets]);
        let orders: Vec<_> = map.materials.iter().map(|m| m.order.clone()).collect();
        let rank = ranks(&orders);
        let key = |s: &Surface| (map.materials[s.material as usize].sort_key, s.primary_light);
        // each (sort key, light) group's material runs, in surface order
        let mut runs: BTreeMap<(u8, u8), Vec<u32>> = BTreeMap::new();
        let mut within = 0;
        // brush models' surfaces follow the world's (model 0's run)
        let (first, count) = map.brush_models[0];
        let world = first as usize..(first + count) as usize;
        for (i, s) in map
            .surfaces
            .iter()
            .enumerate()
            .take(world.end)
            .skip(world.start)
        {
            let v = runs.entry(key(s)).or_default();
            if v.last() != Some(&s.material) {
                v.push(s.material);
            } else {
                let p = &map.surfaces[i - 1];
                let k = |s: &Surface| (s.reflection_probe, s.lightmap, s.first_vertex);
                assert!(k(p) <= k(s), "{name}: surface {i} after {}", i - 1);
                within += 1;
            }
        }
        let (mut pairs, mut repeats) = (0, 0);
        let mut out = Vec::new();
        for v in runs.values() {
            let mut seen = v.clone();
            seen.sort_unstable();
            seen.dedup();
            repeats += v.len() - seen.len();
            for w in v.windows(2) {
                let (a, b) = (&orders[w[0] as usize], &orders[w[1] as usize]);
                if rank[w[0] as usize] < rank[w[1] as usize] {
                    pairs += 1;
                } else {
                    out.push(format!(
                        "{} before {}: {}",
                        a.name,
                        b.name,
                        first_split(a, b)
                    ));
                }
            }
        }
        let unresolved = orders.iter().filter(|o| !o.has_techset()).count();
        println!(
            "{name}: {} world surfaces, {} groups, {pairs} material pairs in order, {} out, {within} \
             neighbours within a material, {repeats} runs repeated, {unresolved} of {} \
             materials without techniques",
            world.len(),
            runs.len(),
            out.len(),
            orders.len()
        );
        for l in &out {
            println!("  {l}");
        }
        assert_eq!(repeats, 0, "{name}: a material's surfaces split in a group");
        assert!(out.is_empty(), "{name}: materials out of the zone's order");
    }
}

/// The first key two materials' orders part on.
fn first_split(a: &MaterialOrder, b: &MaterialOrder) -> String {
    let keys = [
        ("lit", a.lit != b.lit),
        ("emissive", a.emissive != b.emissive),
        ("sort key", a.sort_key != b.sort_key),
        ("lightmap", a.lightmap != b.lightmap),
        ("prepass", a.prepass != b.prepass),
        ("depth", a.writes_depth != b.writes_depth),
        ("pixel shader", a.pixel != b.pixel),
        ("constants", a.consts != b.consts),
        ("vertex shader", a.vertex != b.vertex),
        ("technique set", a.techset != b.techset),
    ];
    keys.iter()
        .find(|k| k.1)
        .map_or("name", |k| k.0)
        .to_string()
}
