use std::collections::BTreeMap;
fn main() {
    let fs = vcod_common::testing::game_fs().expect("no game fs");
    let is_stock = |p: &std::path::Path| {
        let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or("");
        stem.strip_prefix("pak").is_some_and(|r| r.len() == 1 && r.chars().all(|c| c.is_ascii_digit()))
    };
    for map in fs.find_maps() {
        let entry = fs.resolve_map(&map).unwrap();
        let src = fs.source_archive(&entry).unwrap().to_path_buf();
        if !is_stock(&src) { continue; }
        let bsp = vcod_common::bsp::parse(&fs.read(&entry).unwrap()).unwrap();
        let mut classes: BTreeMap<String, u32> = BTreeMap::new();
        for b in vcod_common::bsp::entity_blocks(&bsp.entities) {
            *classes.entry(b.get("classname").cloned().unwrap_or_default()).or_default() += 1;
        }
        let nodes: Vec<_> = classes.iter().filter(|(c, _)| c.starts_with("node") || c.contains("path") || c.contains("cover")).collect();
        let spawns: Vec<_> = classes.iter().filter(|(c, _)| c.contains("spawn") || c.starts_with("info_player")).collect();
        println!("{map} ({}) ents={} nodes={nodes:?} spawns={spawns:?}", src.file_name().unwrap().to_string_lossy(), classes.values().sum::<u32>());
    }
}
