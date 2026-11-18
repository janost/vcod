//! The bot navigation graph on every stock MP map: how many spawn points
//! share one strongly connected component. The floors are the counts in
//! docs/research/bot-navigation.md, "Build times"; a change that loses a
//! spawn fails here, one that joins more passes and should raise the floor.
//!
//! Needs `COD_DIR`; without the paks it returns early. Ignored by default
//! (about 3 min, 2300 s of CPU). After touching nav, run it with
//! `cargo test -p vcod-server --test server -- --ignored nav_census::`.
//!
//! The United Offensive census runs only with `COD_DIR` at a UO install
//! (`uo/` beside `main/`) and returns at once otherwise, so it costs the
//! 1.1 run nothing.

use vcod_common::pmove::MoveRules;
use vcod_server::nav::NavGraph;

const FLOORS: [(&str, usize, usize); 12] = [
    ("mp_brecourt", 161, 161),
    ("mp_carentan", 185, 185),
    ("mp_chateau", 112, 113),
    ("mp_dawnville", 178, 185),
    ("mp_depot", 158, 161),
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
        let g = NavGraph::build(
            &world.collision,
            &world.spawn_points,
            &world.nav_cover,
            &world.hazards,
            MoveRules::V1_1,
        );
        let reach = g.spawns_in_one_component(&world.collision, &world.spawn_points);
        eprintln!("{map}: {reach} / {total} spawns in one component");
        if reach < floor {
            lost.push(format!("{map}: {reach} < {floor}"));
        }
    }
    assert!(lost.is_empty(), "spawns lost: {lost:?}");
}

/// United Offensive's maps, `pakuo00`/`pakuo01` and the map pack
/// (`uomappack00`), proved under UO's movement: `(map, in one component,
/// reach it, total)`, docs/research/bot-navigation.md, "Other rules". A
/// base's spawns stand behind drops out of it, so the floor that matters
/// to a bot is the second: the spawns whose way leads into the fight.
const UO_FLOORS: [(&str, usize, usize, usize); 19] = [
    ("mp_arnhem", 265, 332, 332),
    ("mp_berlin", 202, 303, 303),
    ("mp_cassino", 94, 180, 180),
    ("mp_foy", 386, 391, 391),
    ("mp_italy", 371, 371, 371),
    ("mp_kharkov", 187, 402, 402),
    ("mp_kursk", 273, 282, 282),
    ("mp_ponyri", 265, 272, 273),
    ("mp_rhinevalley", 362, 386, 386),
    ("mp_sicily", 236, 241, 241),
    ("mp_uo_stanjel", 366, 380, 380),
    ("mp_peaks", 113, 128, 128),
    ("mp_streets", 180, 226, 226),
    ("mp_uo_carentan", 285, 285, 285),
    ("mp_uo_dawnville", 195, 249, 249),
    ("mp_uo_depot", 275, 351, 351),
    ("mp_uo_harbor", 113, 292, 292),
    ("mp_uo_hurtgen", 282, 286, 286),
    ("mp_uo_powcamp", 226, 258, 258),
];

#[test]
#[ignore = "builds all 19 United Offensive nav graphs; run with --ignored and COD_DIR at a UO install"]
fn every_uo_maps_spawns_reach_the_fight() {
    let Some(fs) = vcod_common::testing::uo_fs() else {
        eprintln!("COD_DIR has no uo/: skipping");
        return;
    };
    let mut lost = Vec::new();
    for (map, one, reach, total) in UO_FLOORS {
        let Some(entry) = fs.resolve_map(map) else {
            eprintln!("{map}: not in the paks, skipped");
            continue;
        };
        let bsp = vcod_common::bsp::parse(&fs.read(&entry).unwrap()).unwrap();
        let world = vcod_server::world::World::from_bsp(&bsp, Some(&fs));
        assert_eq!(world.spawn_points.len(), total, "{map}: spawn count");
        // As a tdm server's graph stands: the other gametypes' pieces, such
        // as mp_peaks' one-way spawn walls for dom and ctf, are deleted.
        world.collision.unlink_script_brushes(&bsp.entities, "tdm");
        let g = NavGraph::build(
            &world.collision,
            &world.spawn_points,
            &world.nav_cover,
            &world.hazards,
            MoveRules::Uo,
        );
        let (o, r) = g.spawn_census(&world.collision, &world.spawn_points);
        eprintln!("{map}: {o} / {total} spawns in one component, {r} reach it");
        if o < one || r < reach {
            lost.push(format!("{map}: {o} < {one} or {r} < {reach}"));
        }
    }
    assert!(lost.is_empty(), "spawns lost: {lost:?}");
}

/// Every `bas` charge room on United Offensive's base assault maps is on
/// the graph and reached from a spawn: the graph as a tdm server builds it,
/// every base's walls and rubble gone, which is how a breached base's
/// room stands. mp_italy's western axis bunker lay past the spawns' box,
/// its charge room off the graph, and the round never ended on it
/// (`World::nav_cover`, bot-objectives.md 7.2).
#[test]
#[ignore = "builds 5 United Offensive nav graphs; run with --ignored and COD_DIR at a UO install"]
fn every_bas_charge_room_is_on_the_graph() {
    let Some(fs) = vcod_common::testing::uo_fs() else {
        eprintln!("COD_DIR has no uo/: skipping");
        return;
    };
    let mut lost = Vec::new();
    for map in [
        "mp_foy",
        "mp_italy",
        "mp_kursk",
        "mp_ponyri",
        "mp_rhinevalley",
    ] {
        let entry = fs.resolve_map(map).expect("a stock bas map");
        let bsp = vcod_common::bsp::parse(&fs.read(&entry).unwrap()).unwrap();
        let world = vcod_server::world::World::from_bsp(&bsp, Some(&fs));
        world.collision.unlink_script_brushes(&bsp.entities, "tdm");
        let g = NavGraph::build(
            &world.collision,
            &world.spawn_points,
            &world.nav_cover,
            &world.hazards,
            MoveRules::Uo,
        );
        let charges: Vec<[f32; 3]> = vcod_common::bsp::entity_blocks(&bsp.entities)
            .iter()
            .filter(|e| e.get("model").is_some_and(|m| m == "xmodel/mp_bomb"))
            .filter_map(|e| vcod_common::bsp::parse_vec3(e.get("origin")?))
            .collect();
        assert_eq!(charges.len(), 6, "{map}: six charges");
        let from = world
            .spawn_points
            .iter()
            .filter_map(|s| g.nearest(*s))
            .collect::<Vec<_>>();
        for c in charges {
            let near = g.nearest(c).filter(|&n| {
                let d = g.nodes[n as usize] - glam::Vec3::from(c);
                d.truncate().length() <= g.spacing && d.z.abs() < 18.0
            });
            let reached = near.is_some_and(|n| from.iter().any(|&s| g.reaches(s, n)));
            eprintln!("{map}: charge {c:?} node {near:?} reached {reached}");
            if !reached {
                lost.push(format!("{map}: {c:?}"));
            }
        }
    }
    assert!(lost.is_empty(), "charges off the graph: {lost:?}");
}
