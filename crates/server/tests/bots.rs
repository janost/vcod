//! The `--bots` debug feature end to end: two synthetic clients join through
//! the stock menus, wander, and, with shoot on, wound each other.
//!
//! Needs `COD_DIR`; without the paks it returns early.

use std::rc::Rc;
use std::time::{Duration, Instant};

const MAP: &str = "mp_carentan";

fn cfg(bots: usize, shoot: bool, gametype: &str) -> vcod_server::ServerConfig {
    vcod_server::ServerConfig {
        map: MAP.into(),
        hostname: "vcod test".into(),
        max_clients: 8,
        gametype: gametype.into(),
        test_entities: 0,
        trace: false,
        bots,
        bots_shoot: shoot,
    }
}

fn server_with(bots: usize, shoot: bool) -> Option<(vcod_server::Server, Instant)> {
    server_on(bots, shoot, "tdm")
}

fn server_on(bots: usize, shoot: bool, gametype: &str) -> Option<(vcod_server::Server, Instant)> {
    let fs = vcod_common::testing::game_fs()?;
    let bsp_path = fs.resolve_map(MAP).expect("map in the mounted paks");
    let bsp_bytes = fs.read(&bsp_path).expect("read the bsp");
    let bsp = vcod_common::bsp::parse(&bsp_bytes).expect("parse the bsp");
    let now = Instant::now();
    let mut sv = vcod_server::Server::new(cfg(bots, shoot, gametype), now);
    sv.load_world(vcod_server::world::World::from_bsp(&bsp, Some(&fs)));
    sv.load_scripts(Rc::new(fs)).expect("load the scripts");
    Some((sv, now))
}

/// `50 ms` of wall clock per tick, `sv_fps 20`.
const FRAME: Duration = Duration::from_millis(50);

fn run(sv: &mut vcod_server::Server, now: &mut Instant, ticks: usize) {
    for _ in 0..ticks {
        *now += FRAME;
        sv.tick(*now);
    }
}

#[test]
fn two_bots_join_through_the_stock_menus_and_wander() {
    let Some((mut sv, mut now)) = server_with(2, false) else {
        eprintln!("COD_DIR unset or has no main/: skipping");
        return;
    };
    run(&mut sv, &mut now, 100);

    let slots = sv.bot_slots();
    assert_eq!(slots.len(), 2, "two bots took two slots");
    for slot in &slots {
        let body = sv.bot_body(*slot).expect("a bot with no body");
        assert!(body.playing, "bot {slot} never spawned as a player");
        assert!(!body.dead);
    }
    // The stock roster knows them, by name and team, like any client.
    let (t0, t1) = (sv.bot_team(slots[0]), sv.bot_team(slots[1]));
    assert_ne!(t0, t1, "the two bots landed on one team");

    let before: Vec<[f32; 3]> = slots
        .iter()
        .map(|s| sv.bot_body(*s).unwrap().origin)
        .collect();
    run(&mut sv, &mut now, 100);
    let moved = slots
        .iter()
        .zip(&before)
        .filter(|(s, o)| (sv.bot_body(**s).unwrap().origin[0] - o[0]).abs() > 50.0)
        .count();
    assert!(moved >= 1, "no bot wandered in 5 s");
}

#[test]
fn a_bot_with_shoot_on_wounds_the_other() {
    let Some((mut sv, mut now)) = server_with(2, true) else {
        eprintln!("COD_DIR unset or has no main/: skipping");
        return;
    };
    run(&mut sv, &mut now, 100);
    let slots = sv.bot_slots();
    // Eye to eye, 100 units apart, facing each other, so the first taps land.
    let a = sv.bot_body(slots[0]).unwrap().origin;
    sv.place_client(slots[0], a, 0.0);
    sv.place_client(slots[1], [a[0] + 100.0, a[1], a[2]], 180.0);

    let mut hurt = false;
    for tick in 0..600 {
        run(&mut sv, &mut now, 1);
        // They wander apart; pull them eye to eye again while both live.
        if tick % 50 == 0 {
            let (ba, bb) = (
                sv.bot_body(slots[0]).unwrap(),
                sv.bot_body(slots[1]).unwrap(),
            );
            if ba.playing && bb.playing {
                let a = ba.origin;
                sv.place_client(slots[0], a, 0.0);
                sv.place_client(slots[1], [a[0] + 100.0, a[1], a[2]], 180.0);
            }
        }
        let (ha, hb) = (
            sv.bot_body(slots[0]).unwrap().health,
            sv.bot_body(slots[1]).unwrap().health,
        );
        if ha < 100 || hb < 100 || sv.bot_body(slots[0]).unwrap().dead {
            hurt = true;
            break;
        }
    }
    assert!(hurt, "30 s of mutual fire never drew blood");
}

/// A bot killed in the fight comes back on its own use press and keeps
/// playing; it never sits out the round as a corpse.
#[test]
fn a_dead_bot_respawns_and_keeps_playing() {
    let Some((mut sv, mut now)) = server_with(2, true) else {
        eprintln!("COD_DIR unset or has no main/: skipping");
        return;
    };
    run(&mut sv, &mut now, 100);
    let slots = sv.bot_slots();
    let a = sv.bot_body(slots[0]).unwrap().origin;
    sv.place_client(slots[0], a, 0.0);
    sv.place_client(slots[1], [a[0] + 100.0, a[1], a[2]], 180.0);

    // Fight until someone dies, then give the use press 10 s to work. The
    // two wander apart, so pull them eye to eye again whenever both are
    // alive; only the death itself ends the loop.
    let mut died = None;
    for tick in 0..1200 {
        run(&mut sv, &mut now, 1);
        if tick % 50 == 0 {
            let (ba, bb) = (
                sv.bot_body(slots[0]).unwrap(),
                sv.bot_body(slots[1]).unwrap(),
            );
            if ba.playing && bb.playing {
                let a = ba.origin;
                sv.place_client(slots[0], a, 0.0);
                sv.place_client(slots[1], [a[0] + 100.0, a[1], a[2]], 180.0);
            }
        }
        let dead = slots
            .iter()
            .copied()
            .find(|s| sv.bot_body(*s).unwrap().dead);
        if let Some(d) = dead {
            died = Some(d);
            break;
        }
    }
    let Some(dead) = died else {
        panic!("no bot died in 60 s; the respawn gate has nothing to test");
    };
    for _ in 0..200 {
        run(&mut sv, &mut now, 1);
        let b = sv.bot_body(dead).unwrap();
        if !b.dead && b.playing {
            return;
        }
    }
    panic!("the dead bot never respawned");
}

#[test]
fn a_bot_with_shoot_off_never_presses_the_trigger() {
    let Some((mut sv, mut now)) = server_with(2, false) else {
        eprintln!("COD_DIR unset or has no main/: skipping");
        return;
    };
    run(&mut sv, &mut now, 100);
    let slots = sv.bot_slots();
    let a = sv.bot_body(slots[0]).unwrap().origin;
    sv.place_client(slots[0], a, 0.0);
    sv.place_client(slots[1], [a[0] + 100.0, a[1], a[2]], 180.0);
    let (ha, hb) = (
        sv.bot_body(slots[0]).unwrap().health,
        sv.bot_body(slots[1]).unwrap().health,
    );
    run(&mut sv, &mut now, 200);
    let (ha2, hb2) = (
        sv.bot_body(slots[0]).unwrap().health,
        sv.bot_body(slots[1]).unwrap().health,
    );
    assert_eq!((ha, hb), (ha2, hb2), "an unarmed bot drew blood");
}

/// A restart reruns `ClientConnect` for every bot in one frame, and `dm`'s
/// spawn picker (`_spawnlogic::getSpawnpoint_DM`) compares every other
/// player's `sessionstate` against strings before the first spawn lands.
/// Retail reads "spectator" on a client that nothing has written yet;
/// ours read undefined, the compare aborted every bot's spawn thread, and
/// the bots sat out the rest of the level as spectators.
#[test]
fn bots_spawn_again_after_a_dm_map_restart() {
    let Some((mut sv, mut now)) = server_on(3, false, "dm") else {
        eprintln!("COD_DIR unset or has no main/: skipping");
        return;
    };
    run(&mut sv, &mut now, 100);
    sv.push_console("map_restart");
    run(&mut sv, &mut now, 200);

    let aborts = sv.script_aborts();
    assert!(
        aborts.is_empty(),
        "thread(s) aborted across the restart\n{}",
        aborts.join("\n")
    );
    for slot in sv.bot_slots() {
        let body = sv.bot_body(slot).expect("a bot with no body");
        assert!(
            body.playing,
            "bot {slot} did not spawn again after the restart"
        );
    }
}
