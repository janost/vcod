//! Throwaway census: tally material content/surface flags across stock paks.
//! Run: COD_DIR=~/Games/CoD-Deluxe cargo run --example flag_census -p vcod-common

use std::collections::BTreeMap;

fn main() {
    let Some(fs) = vcod_common::testing::game_fs() else {
        eprintln!("no game fs");
        return;
    };
    let is_stock_pak = |p: &std::path::Path| {
        let Some(stem) = p.file_stem().and_then(|s| s.to_str()) else {
            return false;
        };
        let Some(rest) = stem.strip_prefix("pak") else {
            return false;
        };
        !rest.is_empty() && rest.chars().all(|c| c.is_ascii_alphanumeric())
    };
    let maps: Vec<String> = fs
        .find_maps()
        .into_iter()
        .filter(|m| {
            let entry = fs.resolve_map(m).unwrap();
            fs.source_archive(&entry).is_some_and(is_stock_pak)
        })
        .collect();
    println!("maps: {}", maps.len());

    // (content_flags, surface_flags) -> (brush uses, soup uses, sample names)
    let mut by_flags: BTreeMap<(u32, u32), (u32, u32, Vec<String>)> = BTreeMap::new();
    let mut brush_mats: std::collections::BTreeSet<u16> = Default::default();
    for map in &maps {
        let entry = fs.resolve_map(map).unwrap();
        let bsp = match vcod_common::bsp::parse(&fs.read(&entry).unwrap()) {
            Ok(b) => b,
            Err(e) => {
                eprintln!("{map}: {e}");
                continue;
            }
        };
        brush_mats.clear();
        let mut soup_mats: std::collections::BTreeSet<u16> = Default::default();
        for m in &bsp.models {
            for b in &bsp.brushes[m.first_brush as usize..(m.first_brush + m.num_brushes) as usize]
            {
                brush_mats.insert(b.material);
            }
            for s in &bsp.soups[m.first_soup as usize..(m.first_soup + m.num_soups) as usize] {
                soup_mats.insert(s.material);
            }
        }
        for (mi, mat) in bsp.materials.iter().enumerate() {
            let mi = mi as u16;
            let e = by_flags
                .entry((mat.content_flags, mat.surface_flags))
                .or_default();
            if brush_mats.contains(&mi) {
                e.0 += 1;
                if e.2.len() < 6 && !e.2.iter().any(|n| n == &mat.name) {
                    e.2.push(mat.name.clone());
                }
            }
            if soup_mats.contains(&mi) {
                e.1 += 1;
            }
        }
    }

    println!(
        "{:>12} {:>12} {:>7} {:>7}  names",
        "content", "surface", "brush", "soup"
    );
    for ((c, s), (b, so, names)) in &by_flags {
        println!("{c:>12} {s:>12} {b:>7} {so:>7}  {}", names.join(", "));
    }

    println!("\n-- materials whose name mentions water/ladder/liquid --");
    for map in &maps {
        let entry = fs.resolve_map(map).unwrap();
        let bsp = match vcod_common::bsp::parse(&fs.read(&entry).unwrap()) {
            Ok(b) => b,
            Err(_) => continue,
        };
        let mut seen: std::collections::BTreeSet<&str> = Default::default();
        for mat in &bsp.materials {
            let n = mat.name.to_lowercase();
            if n.contains("water") || n.contains("ladder") || n.contains("liquid") {
                if seen.insert(&mat.name) {
                    println!(
                        "{map:>14}: {:<40} content={:#x} surface={:#x}",
                        mat.name, mat.content_flags, mat.surface_flags
                    );
                }
            }
        }
    }
}
