//! Builds the bot navigation graph on stock maps and prints its size, its
//! build time and how many spawn points reach each other through it.
//! Run: COD_DIR=<install> cargo run --release -p vcod-server --example nav_build [map...]

use std::time::Instant;
use vcod_server::nav::{NavGraph, spawn_points};

fn main() {
    let Some(fs) = vcod_common::testing::game_fs() else {
        eprintln!("no game fs (set COD_DIR)");
        return;
    };
    let mut maps: Vec<String> = std::env::args().skip(1).collect();
    if maps.is_empty() {
        maps = fs
            .find_maps()
            .into_iter()
            .filter(|m| m.starts_with("mp_"))
            .collect();
    }
    for map in maps {
        let Some(entry) = fs.resolve_map(&map) else {
            eprintln!("{map}: not found");
            continue;
        };
        let bsp = vcod_common::bsp::parse(&fs.read(&entry).unwrap()).unwrap();
        let world = vcod_server::world::World::from_bsp(&bsp, Some(&fs));
        let seeds = spawn_points(&bsp.entities);
        let t = Instant::now();
        let g = NavGraph::build(&world.collision, &seeds);
        let ms = t.elapsed().as_secs_f64() * 1000.0;
        // The component holding the most spawns, and how many it holds.
        let comp = g.components();
        let nodes: Vec<u32> = seeds.iter().filter_map(|s| g.nearest(*s)).collect();
        let mut count = std::collections::BTreeMap::new();
        for n in &nodes {
            *count.entry(comp[*n as usize]).or_insert(0) += 1;
        }
        let reach = count.values().max().copied().unwrap_or(0);
        println!(
            "{map}: spacing {}, {} nodes, {} edges, {ms:.0} ms, spawns {}/{} on graph, {reach} in one component",
            g.spacing,
            g.len(),
            g.edge_count(),
            nodes.len(),
            seeds.len(),
        );
    }
}
