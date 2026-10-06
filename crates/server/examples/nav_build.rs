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
        maps = fs.find_maps().into_iter().filter(|m| m.starts_with("mp_")).collect();
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
        if std::env::var("NAV_DEBUG").is_ok() {
            let big = *count.iter().max_by_key(|(_, v)| **v).unwrap().0;
            for (s, n) in seeds.iter().zip(&nodes) {
                if comp[*n as usize] != big {
                    let c = comp[*n as usize];
                    let size = comp.iter().filter(|x| **x == c).count();
                    println!("  off: seed {s:?} node {:?} comp {c} size {size}", g.nodes[*n as usize]);
                }
            }
        }
        if std::env::var("NAV_DEBUG2").is_ok() {
            let mut sizes = std::collections::BTreeMap::new();
            for c in &comp { *sizes.entry(*c).or_insert(0usize) += 1; }
            let mut by: Vec<_> = sizes.into_iter().collect();
            by.sort_by_key(|(_, s)| std::cmp::Reverse(*s));
            let (a, b) = (by[0].0, by[1].0);
            println!("  biggest {:?} second {:?}", by[0], by[1]);
            let mut shown = 0;
            for (i, out) in g.edges.iter().enumerate() {
                for &j in out {
                    let (ci, cj) = (comp[i], comp[j as usize]);
                    if (ci == a && cj == b) || (ci == b && cj == a) {
                        if shown < 15 { println!("  one-way {ci}->{cj}: {:?} -> {:?}", g.nodes[i], g.nodes[j as usize]); }
                        shown += 1;
                    }
                }
            }
            println!("  one-way edges between: {shown}");
        }
        let reach = count.values().max().copied().unwrap_or(0);
        println!(
            "{map}: {} nodes, {} edges, {ms:.0} ms, spawns {}/{} on graph, {reach} in one component",
            g.len(),
            g.edge_count(),
            nodes.len(),
            seeds.len(),
        );
    }
}
