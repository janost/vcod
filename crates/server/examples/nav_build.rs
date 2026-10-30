//! Builds the bot navigation graph on stock maps and prints its size, its
//! build time and how many spawn points reach each other through it.
//! Run: COD_DIR=<install> cargo run --release -p vcod-server --example nav_build [map...]
//! `UO=1` reads United Offensive's `uo/` over `main/` and proves the walks
//! under UO's movement, as a UO server builds the graph; `RULES=1.1`,
//! `1.5` or `uo` picks the movement on its own. `NEAR=x,y,r` lists the
//! nodes within `r` across of `(x, y)`, with their edge counts (`EDGES=1`
//! lists each node's edges, tagged jump, leap, crouch or risky), and
//! `ROUTE=x,y,z,x,y,z` the route between the nodes nearest two points.
//! `GAMETYPE=tdm` unlinks the other gametypes' script brushes as the
//! census does, in place of the `bas` rubble. `LOST=1` lists the spawns
//! that do not reach the census's main component, with their nodes, and
//! `CROUCH=1` every crouched jump edge, `JUMPS=x,y,r` the jump edges out
//! of nodes within `r` across of `(x, y)`, with the aim a bot steers at.
//! `UNLINK=n,...` builds with those submodels out, as a script's
//! `delete` leaves them. The `digest` is an FNV-1a hash over the nodes,
//! the edges and the jump, leap, risky and perch sets: two builds that
//! print one digest built the same graph. `GAME=cod4` builds the graph a
//! CoD4 server's bots walk (`COD4_DIR=<install>`, maps `mp_crash` by
//! default) and adds its mantle count. `GAME=cod2` builds the graph a
//! CoD2 server's bots walk (`COD2_DIR=<install>`, maps `mp_toujane` by
//! default, `GAMETYPE` the level's, `tdm` unset), as its bots' build.

use std::time::Instant;
use vcod_common::pmove::MoveRules;
use vcod_server::nav::{NavGraph, spawn_points};

fn main() {
    if std::env::var("GAME").is_ok_and(|g| g == "cod4") {
        return cod4_main();
    }
    if std::env::var("GAME").is_ok_and(|g| g == "cod2") {
        return cod2_main();
    }
    let uo = std::env::var("UO").is_ok_and(|v| v == "1");
    let fs = if uo {
        vcod_common::testing::uo_fs()
    } else {
        vcod_common::testing::game_fs()
    };
    let Some(fs) = fs else {
        eprintln!("no game fs (set COD_DIR)");
        return;
    };
    let rules = match std::env::var("RULES").as_deref() {
        Ok("1.1") => MoveRules::V1_1,
        Ok("1.5") => MoveRules::V1_5,
        Ok("uo") => MoveRules::Uo,
        _ if uo => MoveRules::Uo,
        _ => MoveRules::V1_1,
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
        if let Ok(gt) = std::env::var("GAMETYPE") {
            world.collision.unlink_script_brushes(&bsp.entities, &gt);
        } else {
            // As a server's build: without the brushes `bas.gsc` lifts away.
            for &m in &world.rubble {
                world.collision.set_model_linked(m, false);
            }
        }
        for m in std::env::var("UNLINK")
            .unwrap_or_default()
            .split(',')
            .filter_map(|m| m.parse().ok())
        {
            world.collision.set_model_linked(m, false);
        }
        let g = NavGraph::build(
            &world.collision,
            &seeds,
            &world.nav_cover,
            &world.hazards,
            rules,
        );
        let ms = t.elapsed().as_secs_f64() * 1000.0;
        let on = seeds.iter().filter(|s| g.nearest(**s).is_some()).count();
        let (reach, leave) = g.spawn_census(&world.collision, &seeds);
        println!(
            "{map}: spacing {}, {} nodes, {} edges, {ms:.0} ms, spawns {}/{} on graph, {reach} in one component, {leave} reach it, {} leaps, {} crouched jumps, {} aimed jumps, digest {:016x}",
            g.spacing,
            g.len(),
            g.edge_count(),
            on,
            seeds.len(),
            g.leaps.len(),
            g.crouch_jumps.len(),
            g.aims.len(),
            digest(&g),
        );
        if std::env::var("CROUCH").is_ok_and(|v| v == "1") {
            let mut edges: Vec<_> = g.crouch_jumps.iter().copied().collect();
            edges.sort_unstable();
            for (a, b) in edges {
                println!(
                    "  crouched jump {a} {:?} -> {b} {:?}",
                    g.nodes[a as usize].round(),
                    g.nodes[b as usize].round()
                );
            }
        }
        if std::env::var("LOST").is_ok_and(|v| v == "1") {
            for (i, n) in g.census_lost(&world.collision, &seeds).1 {
                let s = seeds[i].map(f32::round);
                match n {
                    Some(n) => println!(
                        "  lost spawn {i} {s:?} node {n} {:?} edges {}",
                        g.nodes[n as usize].round(),
                        g.edges[n as usize].len()
                    ),
                    None => println!("  lost spawn {i} {s:?} no node"),
                }
            }
        }
        inspect(&g);
    }
}

/// `ROUTE=`, `JUMPS=` and `NEAR=` (`EDGES=1`) over a built graph.
fn inspect(g: &NavGraph) {
    if let Ok(route) = std::env::var("ROUTE") {
        let v: Vec<f32> = route
            .split(',')
            .map(|x| x.parse().expect("ROUTE"))
            .collect();
        let (a, b) = (g.nearest([v[0], v[1], v[2]]), g.nearest([v[3], v[4], v[5]]));
        let path = a.zip(b).and_then(|(a, b)| g.path(a, b));
        println!(
            "  route {a:?} -> {b:?}: {:?}",
            path.map(|p| p
                .iter()
                .map(|n| g.nodes[*n as usize].round())
                .collect::<Vec<_>>())
        );
    }
    if let Ok(near) = std::env::var("JUMPS") {
        let v: Vec<f32> = near.split(',').map(|x| x.parse().expect("JUMPS")).collect();
        for &(a, b) in &g.jumps {
            let n = g.nodes[a as usize];
            if (n.x - v[0]).hypot(n.y - v[1]) < v[2] {
                println!(
                    "  jump {:?} -> {:?} leap {} aim {:?}",
                    n.round(),
                    g.nodes[b as usize].round(),
                    g.leaps.contains(&(a, b)),
                    g.aims.get(&(a, b))
                );
            }
        }
    }
    if let Ok(near) = std::env::var("NEAR") {
        let v: Vec<f32> = near.split(',').map(|x| x.parse().expect("NEAR")).collect();
        for (i, n) in g.nodes.iter().enumerate() {
            if (n.x - v[0]).hypot(n.y - v[1]) < v[2] {
                println!("  node {i} {:?} edges {}", n.round(), g.edges[i].len());
                if std::env::var("EDGES").is_ok_and(|v| v == "1") {
                    for &b in &g.edges[i] {
                        let e = (i as u32, b);
                        let tag = [
                            (&g.jumps, "jump"),
                            (&g.leaps, "leap"),
                            (&g.crouch_jumps, "crouch"),
                            (&g.risky, "risky"),
                            (&g.mantles, "mantle"),
                        ]
                        .iter()
                        .filter(|(s, _)| s.contains(&e))
                        .map(|(_, t)| *t)
                        .collect::<Vec<_>>()
                        .join(",");
                        println!("    -> {b} {:?} {tag}", g.nodes[b as usize].round());
                    }
                }
            }
        }
    }
}

fn digest(g: &NavGraph) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    let mut eat = |x: u32| {
        for b in x.to_le_bytes() {
            h = (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3);
        }
    };
    for n in &g.nodes {
        n.to_array().iter().for_each(|c| eat(c.to_bits()));
    }
    for e in &g.edges {
        eat(e.len() as u32);
        e.iter().for_each(|&b| eat(b));
    }
    for set in [&g.jumps, &g.leaps, &g.risky] {
        let mut v: Vec<_> = set.iter().copied().collect();
        v.sort_unstable();
        eat(v.len() as u32);
        v.iter().for_each(|&(a, b)| {
            eat(a);
            eat(b);
        });
    }
    let mut p: Vec<_> = g.perches.iter().copied().collect();
    p.sort_unstable();
    p.iter().for_each(|&x| eat(x));
    h
}

/// `GAME=cod4`: the graph CoD4's bots build (`vcod_server::bots::cod4`)
/// over each map's clip map, from its spawn points, with the mantles, in
/// the same line as CoD1's plus the mantle count.
fn cod4_main() {
    let Some(install) = vcod_cod4::testing::cod4_install() else {
        eprintln!("no CoD4 install (set COD4_DIR)");
        return;
    };
    let mut maps: Vec<String> = std::env::args().skip(1).collect();
    if maps.is_empty() {
        maps = vec!["mp_crash".into()];
    }
    for map in maps {
        let zones = match vcod_server::game::cod4::Cod4Zones::load(&install, &map) {
            Ok(z) => z,
            Err(e) => {
                eprintln!("{map}: {e:#}");
                continue;
            }
        };
        let Some(clip) = zones.clip.clone() else {
            eprintln!("{map}: no clip map");
            continue;
        };
        use vcod_server::bots::cod4;
        let world = cod4::bot_world(clip.clone(), &zones.map.entities, Vec::new());
        let seeds = world.spawn_points.clone();
        let step = cod4::walk_step(
            clip.clone(),
            vcod_cod4::pmove::Rules::default(),
            &zones.map.entities,
        );
        let mantle = cod4::mantle_probe(clip, vcod_cod4::pmove::Rules::default());
        let t = Instant::now();
        let g = NavGraph::build_opts(
            &world.collision,
            &seeds,
            &[],
            &[],
            cod4::GRAPH_RULES,
            Some(&mantle),
            step,
            cod4::LIP,
        );
        let ms = t.elapsed().as_secs_f64() * 1000.0;
        let on = seeds.iter().filter(|s| g.nearest(**s).is_some()).count();
        let (reach, leave) = g.spawn_census(&world.collision, &seeds);
        println!(
            "{map}: spacing {}, {} nodes, {} edges, {ms:.0} ms, spawns {}/{} on graph, {reach} in one component, {leave} reach it, {} leaps, {} crouched jumps, {} aimed jumps, {} mantles, digest {:016x}",
            g.spacing,
            g.len(),
            g.edge_count(),
            on,
            seeds.len(),
            g.leaps.len(),
            g.crouch_jumps.len(),
            g.aims.len(),
            g.mantles.len(),
            digest(&g),
        );
        inspect(&g);
    }
}

/// `GAME=cod2`: the graph CoD2's bots build (`vcod_server::bots::cod2`)
/// over each level's world as the server loads it for `GAMETYPE`, with the
/// mantles, in the same line as CoD4's.
fn cod2_main() {
    use std::rc::Rc;
    use vcod_server::bots::cod2;
    use vcod_server::cod2::{Cod2Config, Cod2Server};
    let Some(fs) = vcod_common::testing::cod2_fs() else {
        eprintln!("no CoD2 install (set COD2_DIR)");
        return;
    };
    let fs = Rc::new(fs);
    let mut maps: Vec<String> = std::env::args().skip(1).collect();
    if maps.is_empty() {
        maps = vec!["mp_toujane".into()];
    }
    let gametype = std::env::var("GAMETYPE").unwrap_or_else(|_| "tdm".into());
    for map in maps {
        let mut sv = Cod2Server::new(
            Cod2Config {
                map: map.clone(),
                gametype: gametype.clone(),
                hostname: "nav".into(),
                max_clients: 8,
                gametype_script: None,
            },
            1,
            Instant::now(),
        );
        if let Err(e) = sv.load(Some(fs.clone())) {
            eprintln!("{map}: {e:#}");
            continue;
        }
        let level = sv.level().expect("a loaded level");
        let world = &level.world;
        let collision = world.collision.clone();
        for &m in &world.rubble {
            collision.set_model_linked(m, false);
        }
        let seeds = world.spawn_points.clone();
        let mantle = cod2::mantle_probe(level.rules);
        let t = Instant::now();
        let g = NavGraph::build_opts(
            &collision,
            &seeds,
            &world.nav_cover,
            &world.hazards,
            cod2::GRAPH_RULES,
            Some(&mantle),
            None,
            cod2::LIP,
        );
        let ms = t.elapsed().as_secs_f64() * 1000.0;
        let on = seeds.iter().filter(|s| g.nearest(**s).is_some()).count();
        let (reach, leave) = g.spawn_census(&collision, &seeds);
        println!(
            "{map}: spacing {}, {} nodes, {} edges, {ms:.0} ms, spawns {}/{} on graph, {reach} in one component, {leave} reach it, {} leaps, {} crouched jumps, {} aimed jumps, {} mantles, digest {:016x}",
            g.spacing,
            g.len(),
            g.edge_count(),
            on,
            seeds.len(),
            g.leaps.len(),
            g.crouch_jumps.len(),
            g.aims.len(),
            g.mantles.len(),
            digest(&g),
        );
        inspect(&g);
    }
}
