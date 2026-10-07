//! Bots playing stock `sd.gsc`: an attacker walks to a bombzone and plants,
//! a defender walks to the bomb and defuses it; and stock `re.gsc`: an
//! attacker picks an objective up and carries it to its goal, on
//! mp_carentan and on mp_depot, whose objective takes a leap
//! (docs/research/bot-objectives.md).
//!
//! Needs `COD_DIR`; without the paks it returns early.

use std::rc::Rc;
use std::time::{Duration, Instant};

const FRAME: Duration = Duration::from_millis(50);

fn server(bots: usize) -> Option<(vcod_server::Server, Instant)> {
    server_on("mp_carentan", "sd", bots)
}

fn server_on(map: &str, gametype: &str, bots: usize) -> Option<(vcod_server::Server, Instant)> {
    let fs = vcod_common::testing::game_fs()?;
    let bsp_path = fs.resolve_map(map).expect("map in the mounted paks");
    let bsp = vcod_common::bsp::parse(&fs.read(&bsp_path).unwrap()).unwrap();
    let now = Instant::now();
    let cfg = vcod_server::ServerConfig {
        map: map.into(),
        hostname: "vcod test".into(),
        max_clients: 8,
        gametype: gametype.into(),
        test_entities: 0,
        trace: false,
        bots,
        // Nobody dies, so the round runs until the bomb settles it.
        bots_shoot: false,
    };
    let mut sv = vcod_server::Server::new(cfg, now);
    // The seed decides the bots' weapons, sites and roam picks; a fixed
    // one keeps the run the same every time.
    sv.test_seed_rng(7);
    sv.load_world(vcod_server::world::World::from_bsp(&bsp, Some(&fs)));
    sv.load_scripts(Rc::new(fs)).expect("load the scripts");
    Some((sv, now))
}

/// The tick a `logPrint` line ending in `event` (`sd.gsc`'s
/// `A;<num>;<team>;<name>;bomb_plant`) first appears, within `limit` ticks.
fn wait_for(
    sv: &mut vcod_server::Server,
    now: &mut Instant,
    event: &str,
    limit: usize,
) -> Option<usize> {
    for tick in 0..limit {
        *now += FRAME;
        sv.tick(*now);
        if sv
            .script_log()
            .iter()
            .any(|l| l.trim_end().ends_with(event))
        {
            return Some(tick);
        }
    }
    None
}

#[test]
fn an_attacker_bot_plants_and_a_defender_bot_defuses() {
    let Some((mut sv, mut now)) = server(2) else {
        eprintln!("COD_DIR unset or has no main/: skipping");
        return;
    };
    // The match-start restart comes once both teams have a player; the
    // round after it is the one the bomb can be planted in.
    // Seed 7 plants at tick 731 (37 s, about 10 s of it the walk).
    let planted = wait_for(&mut sv, &mut now, "bomb_plant", 1200);
    eprintln!("planted at tick {planted:?}");
    assert!(planted.is_some(), "no bomb planted in 60 s");
    // The fuse is 60 s; seed 7 defuses 225 ticks after the plant.
    let defused = wait_for(&mut sv, &mut now, "bomb_defuse", 1200);
    eprintln!("defused {defused:?} ticks after the plant");
    assert!(
        defused.is_some(),
        "the bomb was not defused before its fuse"
    );
    assert_eq!(sv.script_aborts(), Vec::<String>::new());
}

#[test]
fn an_attacker_bot_carries_a_retrieval_objective_home() {
    let Some((mut sv, mut now)) = server_on("mp_carentan", "re", 2) else {
        eprintln!("COD_DIR unset or has no main/: skipping");
        return;
    };
    // Seed 7 picks the documents up at tick 480 and delivers them 411
    // ticks later; the lone defender guards and never shoots.
    let picked = wait_for(&mut sv, &mut now, "re_pickup", 1200);
    eprintln!("picked up at tick {picked:?}");
    assert!(picked.is_some(), "nothing picked up in 60 s");
    let captured = wait_for(&mut sv, &mut now, "re_capture", 1200);
    eprintln!("captured {captured:?} ticks after the pickup");
    assert!(captured.is_some(), "the objective never reached its goal");
    assert_eq!(sv.script_aborts(), Vec::<String>::new());
}

/// mp_depot's documents lie on a crate top a jump across a gap from the
/// step below it: the attacker gets there only by the graph's leap, coming
/// to rest at its foot and jumping at the lip (bot-navigation.md, "Jumps").
#[test]
fn an_attacker_bot_leaps_onto_depots_crate_for_the_documents() {
    let Some((mut sv, mut now)) = server_on("mp_depot", "re", 2) else {
        eprintln!("COD_DIR unset or has no main/: skipping");
        return;
    };
    // Seed 7 picks the documents up at tick 814 and delivers them 224
    // ticks later.
    let picked = wait_for(&mut sv, &mut now, "re_pickup", 1200);
    eprintln!("picked up at tick {picked:?}");
    assert!(picked.is_some(), "nothing picked up in 60 s");
    let captured = wait_for(&mut sv, &mut now, "re_capture", 1200);
    eprintln!("captured {captured:?} ticks after the pickup");
    assert!(captured.is_some(), "the objective never reached its goal");
    assert_eq!(sv.script_aborts(), Vec::<String>::new());
}
