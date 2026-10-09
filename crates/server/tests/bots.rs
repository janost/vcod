//! The `--bots` debug feature end to end: two synthetic clients join through
//! the stock menus, wander, and, with shoot on, wound each other.
//!
//! Needs `COD_DIR`; without the paks it returns early. `BOTS_SEED=<n>`
//! replaces the server's fixed seed, for sweeping a scenario over many games.

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
    let cfg = cfg(bots, shoot, gametype);
    let mut sv = match std::env::var("BOTS_SEED") {
        Ok(seed) => vcod_server::Server::with_seed(cfg, now, seed.parse().expect("BOTS_SEED")),
        Err(_) => vcod_server::Server::new(cfg, now),
    };
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

/// Two floors 100 units apart along +x near `at` with a clear eye line
/// between them, for a fight whose first taps land. Where a bot stands
/// after the warm-up depends on its roam, and a wall, a pillar or a brush
/// at the second spot leaves the pair blind and the fight silent.
fn fight_spot(sv: &vcod_server::Server, at: [f32; 3]) -> ([f32; 3], [f32; 3]) {
    (0..49)
        .find_map(|i| {
            let (dx, dy) = ((i % 7 - 3) as f32 * 96.0, (i / 7 - 3) as f32 * 96.0);
            let a = sv.test_ground_under([at[0] + dx, at[1] + dy, at[2] + 64.0])?;
            let b = sv.test_ground_under([a[0] + 100.0, a[1], a[2] + 32.0])?;
            ((b[2] - a[2]).abs() < 8.0 && sv.test_clear_line(a, 0.0, 100.0)).then_some((a, b))
        })
        .unwrap_or_else(|| panic!("no floor with a sightline for a fight round {at:?}"))
}

/// Puts the two bots of `pair` eye to eye at `spot`, facing each other.
fn face_off(sv: &mut vcod_server::Server, pair: [usize; 2], spot: ([f32; 3], [f32; 3])) {
    sv.place_client(pair[0], spot.0, 0.0);
    sv.place_client(pair[1], spot.1, 180.0);
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
    let pair = [slots[0], slots[1]];
    let spot = fight_spot(&sv, sv.bot_body(slots[0]).unwrap().origin);
    face_off(&mut sv, pair, spot);

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
                face_off(&mut sv, pair, spot);
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
    let pair = [slots[0], slots[1]];
    let spot = fight_spot(&sv, sv.bot_body(slots[0]).unwrap().origin);
    face_off(&mut sv, pair, spot);

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
                face_off(&mut sv, pair, spot);
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

/// Roaming follows the navigation graph to destinations at least 1000 units
/// off, so in 20 s each bot gets far from where it spawned; a random
/// heading that turns at every wall mostly does not.
#[test]
fn bots_roam_far_along_the_graph() {
    let Some((mut sv, mut now)) = server_with(2, false) else {
        eprintln!("COD_DIR unset or has no main/: skipping");
        return;
    };
    run(&mut sv, &mut now, 60);
    let slots = sv.bot_slots();
    let start: Vec<[f32; 3]> = slots
        .iter()
        .map(|s| sv.bot_body(*s).unwrap().origin)
        .collect();
    let mut far = vec![0.0f32; slots.len()];
    for _ in 0..400 {
        run(&mut sv, &mut now, 1);
        for (i, s) in slots.iter().enumerate() {
            let o = sv.bot_body(*s).unwrap().origin;
            let d = ((o[0] - start[i][0]).powi(2) + (o[1] - start[i][1]).powi(2)).sqrt();
            far[i] = far[i].max(d);
        }
    }
    for (s, d) in slots.iter().zip(&far) {
        assert!(
            *d > 800.0,
            "bot {s} got no further than {d:.0} from its spawn"
        );
    }
}

/// Gunfire carries through walls: a third bot with no line to a fight
/// 900-1300 units off, and a route to it on the graph, walks toward it.
#[test]
fn a_bot_walks_toward_gunfire_it_cannot_see() {
    let Some((mut sv, mut now)) = server_with(3, true) else {
        eprintln!("COD_DIR unset or has no main/: skipping");
        return;
    };
    run(&mut sv, &mut now, 100);
    let slots = sv.bot_slots();
    let (fighters, listener) = ([slots[0], slots[1]], slots[2]);
    let fight = fight_spot(&sv, sv.bot_body(fighters[0]).unwrap().origin);
    let a = fight.0;
    // A floor in a ring round the fight with the eye line to it blocked and
    // a route to it short enough to walk most of in 20 s. A spot with no
    // route (a roof, a closed yard) tests the graph, not the hearing.
    let spot = (0..48)
        .filter_map(|i| {
            let r = 900.0 + 200.0 * (i / 16) as f32;
            let yaw = (i % 16) as f32 * 22.5;
            let (s, c) = yaw.to_radians().sin_cos();
            let p = sv.test_ground_under([a[0] + c * r, a[1] + s * r, a[2] + 64.0])?;
            let back = yaw + 180.0;
            let route = sv.test_nav_route(p, a)?;
            (!sv.test_clear_line(p, back, r) && route < 1.5 * r).then_some((p, back))
        })
        .next()
        .expect("no walled-off floor with a route to the fight");
    let dist = |p: [f32; 3]| (p[0] - a[0]).hypot(p[1] - a[1]);
    // Facing away, so it does not stumble on the fight by looking.
    sv.place_client(listener, spot.0, spot.1 + 180.0);
    let start = dist(spot.0);

    // 20 s: each death in the fight silences it until the respawn, and the
    // listener heads off on a roam between bursts.
    let mut closest = start;
    for tick in 0..400 {
        if tick % 50 == 0 {
            let (ba, bb) = (
                sv.bot_body(fighters[0]).unwrap(),
                sv.bot_body(fighters[1]).unwrap(),
            );
            if ba.playing && bb.playing {
                face_off(&mut sv, fighters, fight);
            }
        }
        run(&mut sv, &mut now, 1);
        closest = closest.min(dist(sv.bot_body(listener).unwrap().origin));
    }
    assert!(
        closest < start - 500.0,
        "the listener got from {start:.0} to {closest:.0} of the fight"
    );
}

/// mp_ship's ladders, climbed by six roaming bots: none hangs on one for
/// longer than the 560-unit hold ladder takes at the full rate (11 s).
/// Seed 1 used to hold a bot on the hold ladder for a minute, slowed onto
/// a leap's foot at its head, and others for good under a spar on a deck
/// ladder, wandering with a level view.
#[test]
#[ignore = "runs 6 bots on mp_ship for 4 seeds, ~2 min; run with --ignored"]
fn bots_on_mp_ships_ladders_keep_climbing() {
    let Some(fs) = vcod_common::testing::game_fs() else {
        eprintln!("COD_DIR unset or has no main/: skipping");
        return;
    };
    let bsp = vcod_common::bsp::parse(&fs.read(&fs.resolve_map("mp_ship").unwrap()).unwrap())
        .expect("parse the bsp");
    let mut now = Instant::now();
    let mut cfg = cfg(6, false, "dm");
    cfg.map = "mp_ship".into();
    let mut sv = vcod_server::Server::new(cfg, now);
    sv.test_seed_rng(1);
    sv.load_world(vcod_server::world::World::from_bsp(&bsp, Some(&fs)));
    sv.load_scripts(Rc::new(fs)).expect("load the scripts");
    let mut hanging = [0usize; 8];
    let mut climbs = 0;
    for _ in 0..2000 {
        run(&mut sv, &mut now, 1);
        for slot in sv.bot_slots() {
            let on = sv.bot_body(slot).is_some_and(|b| b.playing && b.on_ladder);
            climbs += usize::from(on && hanging[slot] == 0);
            hanging[slot] = if on { hanging[slot] + 1 } else { 0 };
            assert!(
                hanging[slot] < 300,
                "bot {slot} on a ladder for 15 s at {:?}",
                sv.bot_body(slot).map(|b| b.origin)
            );
        }
    }
    assert!(climbs >= 3, "only {climbs} ladder grabs");
}

/// mp_ship, seed 5: a bot pinned on the deck lip at (3288, -455, 56) took
/// its random unstick heading off the deck to the hull 50 below (tick 1671
/// before the drop probe). No unstick spell that starts on the ground off
/// a ladder lands more than a jump below where it started, on a floor the
/// graph does not walk straight back up from. A run down a stair can
/// leave the ground at its top and land a storey down; the stair walks
/// back.
#[test]
#[ignore = "runs 6 bots on mp_ship for 90 s of game time, ~1 min; run with --ignored"]
fn bots_on_mp_ship_unstick_without_walking_off_the_deck() {
    let Some(fs) = vcod_common::testing::game_fs() else {
        eprintln!("COD_DIR unset or has no main/: skipping");
        return;
    };
    let bsp = vcod_common::bsp::parse(&fs.read(&fs.resolve_map("mp_ship").unwrap()).unwrap())
        .expect("parse the bsp");
    let mut now = Instant::now();
    let mut cfg = cfg(6, false, "dm");
    cfg.map = "mp_ship".into();
    let mut sv = vcod_server::Server::new(cfg, now);
    sv.test_seed_rng(5);
    sv.load_world(vcod_server::world::World::from_bsp(&bsp, Some(&fs)));
    sv.load_scripts(Rc::new(fs)).expect("load the scripts");
    // Per slot, where the spell under way started, when it started on the
    // ground off a ladder and has touched no ladder since; it is judged
    // where the body first stands once the spell is over.
    let mut spell: [Option<[f32; 3]>; 8] = [None; 8];
    let mut was = [false; 8];
    for tick in 0..1800 {
        run(&mut sv, &mut now, 1);
        for slot in sv.bot_slots() {
            let Some(b) = sv.bot_body(slot).filter(|b| b.playing) else {
                spell[slot] = None;
                continue;
            };
            if b.unsticking && !was[slot] {
                spell[slot] = (b.on_ground && !b.on_ladder).then_some(b.origin);
            }
            was[slot] = b.unsticking;
            if b.on_ladder {
                spell[slot] = None;
            }
            let Some(from) = spell[slot].filter(|_| !b.unsticking && b.on_ground) else {
                continue;
            };
            spell[slot] = None;
            if b.origin[2] > from[2] - vcod_common::pmove::JUMP_HEIGHT {
                continue;
            }
            let flat = (b.origin[0] - from[0]).hypot(b.origin[1] - from[1]);
            let back = sv.test_nav_route(b.origin, from);
            assert!(
                back.is_some_and(|r| r < 2.0 * flat + 128.0),
                "tick {tick}: bot {slot} fell from {from:?} to {:?} on its unstick heading, \
                 route back {back:?}",
                b.origin
            );
        }
    }
}

/// mp_ship, seed 19: a bot climbed the mast ladder at (3696, 57) to the
/// hatch at 992, turned on the rungs for the platform node 47 units west,
/// let go of them and fell back down the shaft, again and again (tick
/// 1169 before the fix). No bot stays 12 s within 150 units of one spot.
#[test]
#[ignore = "runs 6 bots on mp_ship for 65 s of game time, ~1 min; run with --ignored"]
fn bots_on_mp_ship_get_off_the_mast_ladder() {
    let Some(fs) = vcod_common::testing::game_fs() else {
        eprintln!("COD_DIR unset or has no main/: skipping");
        return;
    };
    let bsp = vcod_common::bsp::parse(&fs.read(&fs.resolve_map("mp_ship").unwrap()).unwrap())
        .expect("parse the bsp");
    let mut now = Instant::now();
    let mut cfg = cfg(6, false, "dm");
    cfg.map = "mp_ship".into();
    let mut sv = vcod_server::Server::new(cfg, now);
    sv.test_seed_rng(19);
    sv.load_world(vcod_server::world::World::from_bsp(&bsp, Some(&fs)));
    sv.load_scripts(Rc::new(fs)).expect("load the scripts");
    let mut hist: [std::collections::VecDeque<[f32; 3]>; 8] = Default::default();
    for tick in 0..1300 {
        run(&mut sv, &mut now, 1);
        for slot in sv.bot_slots() {
            let h = &mut hist[slot];
            let Some(b) = sv.bot_body(slot).filter(|b| b.playing) else {
                h.clear();
                continue;
            };
            h.push_back(b.origin);
            if h.len() > 240 {
                h.pop_front();
            }
            let first = h[0];
            let near = |p: &[f32; 3]| {
                (0..3).map(|i| (p[i] - first[i]).powi(2)).sum::<f32>() < 150.0 * 150.0
            };
            assert!(
                h.len() < 240 || !h.iter().all(near),
                "tick {tick}: bot {slot} 12 s round {first:?}, now at {:?}",
                b.origin
            );
        }
    }
}
