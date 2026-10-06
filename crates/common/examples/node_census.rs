//! Census of AI path-node entities (`node_*`) per map in the stock paks,
//! with the spawn classes beside them (docs/research/bot-navigation.md).
//! Run: COD_DIR=<install> cargo run --example node_census -p vcod-common

use std::collections::BTreeMap;

fn main() {
    let Some(fs) = vcod_common::testing::game_fs() else {
        eprintln!("no game fs (set COD_DIR)");
        return;
    };
    let is_stock = |p: &std::path::Path| {
        let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or("");
        stem.strip_prefix("pak")
            .is_some_and(|r| r.len() == 1 && r.chars().all(|c| c.is_ascii_digit()))
    };
    for map in fs.find_maps() {
        let entry = fs.resolve_map(&map).unwrap();
        let src = fs.source_archive(&entry).unwrap().to_path_buf();
        if !is_stock(&src) {
            continue;
        }
        let bsp = vcod_common::bsp::parse(&fs.read(&entry).unwrap()).unwrap();
        let mut classes: BTreeMap<String, u32> = BTreeMap::new();
        for b in vcod_common::bsp::entity_blocks(&bsp.entities) {
            let class = b.get("classname").cloned().unwrap_or_default();
            *classes.entry(class).or_default() += 1;
        }
        let pick = |f: &dyn Fn(&str) -> bool| {
            classes
                .iter()
                .filter(|(c, _)| f(c))
                .map(|(c, n)| format!("{c} {n}"))
                .collect::<Vec<_>>()
                .join(", ")
        };
        println!(
            "{map} ({}): nodes [{}] spawns [{}]",
            src.file_name().unwrap().to_string_lossy(),
            pick(&|c| c.starts_with("node_")),
            pick(&|c| c.contains("_spawn") || c == "info_player_start"),
        );
    }
}
