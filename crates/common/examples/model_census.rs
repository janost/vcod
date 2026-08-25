//! Throwaway census: which entity classnames reference inline `*N` models.
//! Run: COD_DIR=~/Games/CoD-Deluxe cargo run --example model_census -p vcod-common

use std::collections::BTreeMap;

fn main() {
    let Some(fs) = vcod_common::testing::game_fs() else {
        eprintln!("no game fs");
        return;
    };
    let is_stock_pak = |p: &std::path::Path| {
        p.file_stem()
            .and_then(|s| s.to_str())
            .and_then(|s| s.strip_prefix("pak"))
            .is_some_and(|rest| !rest.is_empty() && rest.chars().all(|c| c.is_ascii_alphanumeric()))
    };
    let maps: Vec<String> = fs
        .find_maps()
        .into_iter()
        .filter(|m| {
            let entry = fs.resolve_map(m).unwrap();
            fs.source_archive(&entry).is_some_and(is_stock_pak)
                && entry.to_lowercase().contains("maps/mp/")
        })
        .collect();

    // classname -> count of entities referencing *N
    let mut by_class: BTreeMap<String, u32> = BTreeMap::new();
    // classname -> how many of its referenced submodels carry solid/playerclip/water brushes
    let mut by_class_solid: BTreeMap<String, (u32, u32, u32)> = BTreeMap::new();
    for map in &maps {
        let entry = fs.resolve_map(map).unwrap();
        let Ok(bsp) = vcod_common::bsp::parse(&fs.read(&entry).unwrap()) else {
            continue;
        };
        for b in vcod_common::bsp::entity_blocks(&bsp.entities) {
            let Some(model) = b.get("model") else {
                continue;
            };
            let Some(idx) = model
                .strip_prefix('*')
                .and_then(|n| n.parse::<usize>().ok())
            else {
                continue;
            };
            if idx == 0 || idx >= bsp.models.len() {
                continue;
            }
            let class = b.get("classname").cloned().unwrap_or_default();
            *by_class.entry(class.clone()).or_default() += 1;
            let m = &bsp.models[idx];
            let mut solid = 0;
            let mut water = 0;
            for br in &bsp.brushes[m.first_brush as usize..(m.first_brush + m.num_brushes) as usize]
            {
                let c = bsp.materials[br.material as usize].content_flags;
                if c & (0x1 | 0x10000) != 0 {
                    solid += 1;
                }
                if c & 0x20 != 0 {
                    water += 1;
                }
            }
            let e = by_class_solid.entry(class).or_default();
            e.0 += solid;
            e.1 += water;
            e.2 += 1;
        }
    }
    println!(
        "{:<28} {:>8} {:>8} {:>8} {:>8}",
        "classname", "*N refs", "ents", "solidbr", "waterbr"
    );
    for (class, n) in &by_class {
        let (s, w, _) = by_class_solid.get(class).copied().unwrap_or_default();
        println!(
            "{class:<28} {n:>8} {:>8} {s:>8} {w:>8}",
            by_class_solid.get(class).map(|e| e.2).unwrap_or(0)
        );
    }
}
