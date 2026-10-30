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
    server_from(
        vcod_common::testing::game_fs_11()?,
        None,
        map,
        gametype,
        bots,
    )
}

fn server_from(
    fs: vcod_common::pk3::Pk3Fs,
    protocol: Option<&'static vcod_common::net::protocol::Protocol>,
    map: &str,
    gametype: &str,
    bots: usize,
) -> Option<(vcod_server::Server, Instant)> {
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
    if let Some(p) = protocol {
        sv.set_protocol(p);
    }
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
    // Seed 7 plants at tick 641 (32 s).
    let planted = wait_for(&mut sv, &mut now, "bomb_plant", 1200);
    eprintln!("planted at tick {planted:?}");
    assert!(planted.is_some(), "no bomb planted in 60 s");
    // The fuse is 60 s; seed 7 defuses 226 ticks after the plant.
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
    // Seed 7 picks the documents up at tick 416 and delivers them 346
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
    // Seed 7 picks the documents up at tick 483 and delivers them 272
    // ticks later.
    let picked = wait_for(&mut sv, &mut now, "re_pickup", 1200);
    eprintln!("picked up at tick {picked:?}");
    assert!(picked.is_some(), "nothing picked up in 60 s");
    let captured = wait_for(&mut sv, &mut now, "re_capture", 1200);
    eprintln!("captured {captured:?} ticks after the pickup");
    assert!(captured.is_some(), "the objective never reached its goal");
    assert_eq!(sv.script_aborts(), Vec::<String>::new());
}

/// The same on a United Offensive server, whose `sd.gsc` asks the same of
/// a planter and a defuser (`canPlantGMI` adds only ground under the
/// feet): mp_cassino under UO's movement. Needs `COD_DIR` at a UO install.
#[test]
fn uo_bots_plant_and_defuse_on_cassino() {
    let Some(fs) = vcod_common::testing::uo_fs() else {
        eprintln!("COD_DIR has no uo/: skipping");
        return;
    };
    let p = &vcod_common::net::protocol::PROTOCOL_V22;
    let (mut sv, mut now) = server_from(fs, Some(p), "mp_cassino", "sd", 2).unwrap();
    let planted = wait_for(&mut sv, &mut now, "bomb_plant", 2400);
    eprintln!("planted at tick {planted:?}");
    assert!(planted.is_some(), "no bomb planted in 120 s");
    let defused = wait_for(&mut sv, &mut now, "bomb_defuse", 1200);
    eprintln!("defused {defused:?} ticks after the plant");
    assert!(
        defused.is_some(),
        "the bomb was not defused before its fuse"
    );
    assert_eq!(sv.script_aborts(), Vec::<String>::new());
}

/// A UO server on `map` running `gametype` with two bots, one a side, and
/// `cvars` set before the scripts load, the RNG seeded with `seed`. `None`
/// without a UO install.
fn uo_server(
    map: &str,
    gametype: &str,
    shoot: bool,
    cvars: &[(&str, &str)],
    seed: u64,
) -> Option<(vcod_server::Server, Instant)> {
    let Some(fs) = vcod_common::testing::uo_fs() else {
        eprintln!("COD_DIR has no uo/: skipping");
        return None;
    };
    let p = &vcod_common::net::protocol::PROTOCOL_V22;
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
        bots: 2,
        bots_shoot: shoot,
    };
    let mut sv = vcod_server::Server::new(cfg, now);
    sv.set_protocol(p);
    sv.test_seed_rng(seed);
    for (k, v) in cvars {
        sv.set_cvar(k, v);
    }
    sv.load_world(vcod_server::world::World::from_bsp(&bsp, Some(&fs)));
    sv.load_scripts(Rc::new(fs)).expect("load the scripts");
    Some((sv, now))
}

/// UO capture the flag on mp_cassino: the lone bot of a side takes the
/// other side's flag and carries it to the goal (`ctf.gsc`
/// `flag_carrier_atgoal_wait`).
#[test]
fn uo_bots_take_and_capture_a_flag_on_cassino() {
    // A seed draw: any bot change reshuffles which seeds take a flag
    // in 150 s and capture one in 200 s more. Over 80 seed pairs in 1-161
    // (2n and 2n + 1 play alike), 79 took one and 54 captured before the
    // jump cue (24cff5ab), 77 and 47 after (2026-10-21), so one seed fails
    // two runs in five: the first of these to capture passes.
    let mut runs = Vec::new();
    for seed in [14, 16, 18, 20, 22] {
        let Some((mut sv, mut now)) = uo_server("mp_cassino", "ctf", true, &[], seed) else {
            return;
        };
        // Both sides take the other's flag, and a capture waits on a kill
        // that drops one and a touch that returns it.
        let taken = wait_for(&mut sv, &mut now, "ctf_pickup", 3000);
        let captured = taken.and_then(|_| wait_for(&mut sv, &mut now, "ctf_captured", 4000));
        eprintln!("seed {seed}: taken at tick {taken:?}, captured {captured:?} ticks after");
        assert_eq!(sv.script_aborts(), Vec::<String>::new());
        if captured.is_some() {
            return;
        }
        runs.push((seed, taken));
    }
    panic!("no flag captured on any seed (seed, pickup tick): {runs:?}");
}

/// UO domination on mp_cassino: the bots stand in flag zones until they
/// turn (`dom.gsc` `Flag_ZoneThink`).
#[test]
fn uo_bots_capture_domination_flags_on_cassino() {
    let Some((mut sv, mut now)) = uo_server("mp_cassino", "dom", false, &[], 7) else {
        return;
    };
    let first = wait_for(&mut sv, &mut now, "dom_captured", 3000);
    eprintln!("first flag at tick {first:?}");
    assert!(first.is_some(), "no flag captured in 150 s");
    let mut count = 0;
    for _ in 0..1200 {
        now += FRAME;
        sv.tick(now);
    }
    count += sv
        .script_log()
        .iter()
        .filter(|l| l.trim_end().ends_with("dom_captured"))
        .count();
    eprintln!("{count} captures by 60 s after the first");
    assert!(count >= 3, "only {count} captures");
    assert_eq!(sv.script_aborts(), Vec::<String>::new());
}

/// UO headquarters on mp_cassino: once a radio shows, a bot stands at it
/// until its side holds it (`hq.gsc` `hq_radio_think`).
#[test]
fn uo_bots_set_up_a_headquarters_on_cassino() {
    let Some((mut sv, mut now)) = uo_server("mp_cassino", "hq", true, &[], 7) else {
        return;
    };
    let held = (0..4000).find(|_| {
        now += FRAME;
        sv.tick(now);
        matches!(sv.bot_uo(), Some(vcod_server::game::script::UoObjectives::Hq { radios, .. })
            if radios.iter().any(|r| r.team != vcod_server::game::script::TEAM_NONE))
    });
    eprintln!("radio held at tick {held:?}");
    assert!(held.is_some(), "no radio captured in 200 s");
    assert_eq!(sv.script_aborts(), Vec::<String>::new());
}

/// UO base assault on mp_foy with the bases breached from the start
/// (`scr_bas_basehealth 0`; bots drive no tanks): an attacker plants a
/// charge on a base and the defender goes to defuse it (`bas.gsc`
/// `BaseAssault_ObjectiveZone_think`, `BaseAssault_bomb_think`).
#[test]
fn uo_bots_plant_on_a_breached_base_on_foy() {
    let Some((mut sv, mut now)) =
        uo_server("mp_foy", "bas", false, &[("scr_bas_basehealth", "0")], 7)
    else {
        return;
    };
    let planted = wait_for(&mut sv, &mut now, "bas_planted", 4000);
    eprintln!("planted at tick {planted:?}");
    assert!(planted.is_some(), "no charge planted in 200 s");
    // The countdown is `scr_bas_bombtime`; the lone defender is off at the
    // other side's bases, so the charge goes off.
    let settled = (0..2400).find(|_| {
        now += FRAME;
        sv.tick(now);
        sv.script_log().iter().any(|l| {
            let l = l.trim_end();
            l.ends_with("bas_destroyed") || l.ends_with("bas_defused")
        })
    });
    eprintln!("settled {settled:?} ticks after the plant");
    assert!(
        settled.is_some(),
        "the charge neither went off nor was defused"
    );
    assert_eq!(sv.script_aborts(), Vec::<String>::new());
}
