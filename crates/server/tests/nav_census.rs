//! The bot navigation graph on every stock MP map: how many spawn points
//! share one strongly connected component. The floors are the counts in
//! docs/research/bot-navigation.md, "Build times"; a change that loses a
//! spawn fails here, one that joins more passes and should raise the floor.
//!
//! Needs `COD_DIR`; without the paks it returns early. Ignored by default
//! (about 3 min, 2300 s of CPU): run it with `cargo test -p vcod-server
//! --test nav_census -- --ignored` after touching nav.

use vcod_server::nav::NavGraph;

const FLOORS: [(&str, usize, usize); 12] = [
    ("mp_brecourt", 161, 161),
    ("mp_carentan", 185, 185),
    ("mp_chateau", 112, 113),
    ("mp_dawnville", 178, 185),
    ("mp_depot", 154, 161),
    ("mp_harbor", 160, 161),
    ("mp_hurtgen", 190, 193),
    ("mp_pavlov", 161, 161),
    ("mp_powcamp", 157, 161),
    ("mp_railyard", 161, 161),
    ("mp_rocket", 148, 153),
    ("mp_ship", 112, 112),
];

#[test]
#[ignore = "builds all 12 stock nav graphs, ~3 min; run with --ignored"]
fn every_stock_maps_spawns_stay_in_one_component() {
    let Some(fs) = vcod_common::testing::game_fs() else {
        eprintln!("COD_DIR unset or has no main/: skipping");
        return;
    };
    let mut lost = Vec::new();
    for (map, floor, total) in FLOORS {
        let Some(entry) = fs.resolve_map(map) else {
            eprintln!("{map}: not in the paks, skipped");
            continue;
        };
        let bsp = vcod_common::bsp::parse(&fs.read(&entry).unwrap()).unwrap();
        let world = vcod_server::world::World::from_bsp(&bsp, Some(&fs));
        assert_eq!(world.spawn_points.len(), total, "{map}: spawn count");
        let g = NavGraph::build(&world.collision, &world.spawn_points, &world.hazards);
        let reach = g.spawns_in_one_component(&world.collision, &world.spawn_points);
        eprintln!("{map}: {reach} / {total} spawns in one component");
        if reach < floor {
            lost.push(format!("{map}: {reach} < {floor}"));
        }
    }
    assert!(lost.is_empty(), "spawns lost: {lost:?}");
}
