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
    let fs = vcod_common::testing::game_fs_11()?;
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
    let Some(fs) = vcod_common::testing::game_fs_11() else {
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
    let Some(fs) = vcod_common::testing::game_fs_11() else {
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
            // Off the graph (no node within a column on its floor) the spell
            // started in a pocket the graph does not cover, which a cornered
            // bot drops out of on purpose: mp_ship's lip under the shelf at
            // 396, (5424, -187, 344), 47 units from the nodes at 352.
            if sv.test_nav_route(from, from).is_none() {
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
    let Some(fs) = vcod_common::testing::game_fs_11() else {
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

/// mp_hurtgen's crease at (436.8, 2824.3, -97.2): terrain on two sides and
/// a clip brush hold a body off the ground at rest under any key, on
/// retail as on ours (bot-navigation.md section 3). Master c7ca821a's seed
/// 43 held a bot there 3 730 bot-ticks; now it types `kill`.
#[test]
#[ignore = "builds mp_hurtgen's graph and runs a bot for 25 s of game time; run with --ignored"]
fn a_bot_held_in_mp_hurtgens_crease_kills_itself() {
    let Some(fs) = vcod_common::testing::game_fs_11() else {
        eprintln!("COD_DIR unset or has no main/: skipping");
        return;
    };
    let bsp = vcod_common::bsp::parse(&fs.read(&fs.resolve_map("mp_hurtgen").unwrap()).unwrap())
        .expect("parse the bsp");
    let mut now = Instant::now();
    let mut cfg = cfg(1, false, "dm");
    cfg.map = "mp_hurtgen".into();
    let mut sv = vcod_server::Server::new(cfg, now);
    sv.load_world(vcod_server::world::World::from_bsp(&bsp, Some(&fs)));
    sv.load_scripts(Rc::new(fs)).expect("load the scripts");
    let slot = (0..400)
        .find_map(|_| {
            run(&mut sv, &mut now, 1);
            sv.bot_slots()
                .into_iter()
                .find(|s| sv.bot_body(*s).is_some_and(|b| b.playing))
        })
        .expect("the bot plays");
    let crease = [436.8, 2824.3, -97.2];
    sv.place_client(slot, crease, 45.0);
    run(&mut sv, &mut now, 20);
    let b = sv.bot_body(slot).expect("the bot");
    assert!(
        !b.on_ground && (b.origin[0] - crease[0]).abs() < 1.0,
        "not held: {:?}",
        b.origin
    );
    let died = (0..140).any(|_| {
        run(&mut sv, &mut now, 1);
        sv.bot_body(slot).is_some_and(|b| b.dead)
    });
    assert!(
        died,
        "still held at {:?}",
        sv.bot_body(slot).map(|b| b.origin)
    );
}

/// mp_ship's boat at (5250, -500, 27): a dish off the graph, walled off
/// from the deck at 56 beside it, whose way out is a drop over its rim to
/// the floor at -64. Master cb5f98d0's seed 297 held a bot in it for 143
/// bot-ticks, planning from the deck nodes beyond the wall.
#[test]
#[ignore = "builds mp_ship's graph and runs a bot for 30 s of game time; run with --ignored"]
fn a_bot_in_mp_ships_boat_drops_out_over_its_rim() {
    let Some(fs) = vcod_common::testing::game_fs_11() else {
        eprintln!("COD_DIR unset or has no main/: skipping");
        return;
    };
    let bsp = vcod_common::bsp::parse(&fs.read(&fs.resolve_map("mp_ship").unwrap()).unwrap())
        .expect("parse the bsp");
    let mut now = Instant::now();
    let mut cfg = cfg(1, false, "dm");
    cfg.map = "mp_ship".into();
    let mut sv = vcod_server::Server::new(cfg, now);
    sv.load_world(vcod_server::world::World::from_bsp(&bsp, Some(&fs)));
    sv.load_scripts(Rc::new(fs)).expect("load the scripts");
    let slot = (0..400)
        .find_map(|_| {
            run(&mut sv, &mut now, 1);
            sv.bot_slots()
                .into_iter()
                .find(|s| sv.bot_body(*s).is_some_and(|b| b.playing))
        })
        .expect("the bot plays");
    for spot in [[5270.0, -450.0, 27.0], [5340.0, -520.0, 28.0]] {
        sv.place_client(slot, spot, 90.0);
        let out = (0..600).any(|_| {
            run(&mut sv, &mut now, 1);
            sv.bot_body(slot).is_some_and(|b| b.origin[2] < -32.0)
        });
        assert!(
            out,
            "from {spot:?} still in the boat at {:?}",
            sv.bot_body(slot).map(|b| b.origin)
        );
    }
}

/// mp_ship's ladder at (3354, -380), rungs on its east face from the floor
/// at 56 to the deck at 352. One bot climbs it while the other stands at
/// its head to back down: master 7de7be23 pinned them head-on (seed 313,
/// bot-navigation.md section 3), the climber hung under the lip and the
/// other stood where it steps off. The one at the head now waits aside.
#[test]
#[ignore = "builds mp_ship's graph and runs two bots for 20 s of game time; run with --ignored"]
fn bots_meeting_head_on_at_a_ladder_take_turns() {
    let Some(fs) = vcod_common::testing::game_fs_11() else {
        eprintln!("COD_DIR unset or has no main/: skipping");
        return;
    };
    let bsp = vcod_common::bsp::parse(&fs.read(&fs.resolve_map("mp_ship").unwrap()).unwrap())
        .expect("parse the bsp");
    let mut now = Instant::now();
    let mut cfg = cfg(2, false, "dm");
    cfg.map = "mp_ship".into();
    let mut sv = vcod_server::Server::new(cfg, now);
    sv.load_world(vcod_server::world::World::from_bsp(&bsp, Some(&fs)));
    sv.load_scripts(Rc::new(fs)).expect("load the scripts");
    let playing = |sv: &vcod_server::Server| {
        let s = sv.bot_slots();
        (s.len() == 2 && s.iter().all(|&s| sv.bot_body(s).is_some_and(|b| b.playing))).then_some(s)
    };
    let slots = (0..600)
        .find_map(|_| {
            run(&mut sv, &mut now, 1);
            playing(&sv).filter(|_| {
                sv.test_nav_route([3370.0, -380.0, 56.1], [3370.0, -380.0, 56.1])
                    .is_some()
            })
        })
        .expect("both bots play on a built graph");
    let (up, down) = (slots[0], slots[1]);
    let (deck, floor) = ([3268.0, -352.0, 352.1], [3391.0, -420.0, 56.1]);
    // Out of the way while the climber starts up.
    sv.place_client(down, [3420.0, -320.0, 352.2], 0.0);
    sv.test_bot_goal(down, Some([3420.0, -320.0, 352.1]));
    sv.place_client(up, [3375.0, -380.0, 56.2], 180.0);
    sv.test_bot_goal(up, Some(deck));
    // High on the rungs, but under the other's feet once it stands at the
    // head: a body placed into the climber's is pushed off it.
    let high = (0..300).any(|_| {
        run(&mut sv, &mut now, 1);
        sv.bot_body(up)
            .is_some_and(|b| b.on_ladder && b.origin[2] > 270.0)
    });
    assert!(
        high,
        "never climbed: {:?}",
        sv.bot_body(up).map(|b| b.origin)
    );
    // Where seed 313's bot stood: where the climber steps off.
    sv.place_client(down, [3346.4, -387.9, 352.2], 0.0);
    sv.test_bot_goal(down, Some(floor));
    let (mut topped, mut landed) = (None, None);
    for tick in 0..400 {
        run(&mut sv, &mut now, 1);
        let (u, d) = (sv.bot_body(up).unwrap(), sv.bot_body(down).unwrap());
        if topped.is_none() && u.on_ground && !u.on_ladder && u.origin[2] > 340.0 {
            topped = Some(tick);
        }
        if landed.is_none() && d.on_ground && d.origin[2] < 100.0 {
            landed = Some(tick);
        }
    }
    // Master: the climber fell back to the floor and was off the rungs at
    // the top at tick 353.
    assert!(
        topped.is_some_and(|t| t < 120) && landed.is_some_and(|t| t < 300),
        "climber off the rungs at {topped:?}, the other down at {landed:?}"
    );
}

/// mp_ship's hold ladder, rungs on the face at y -78 from the floor at 56
/// to 615, its head (4332, -87, 615) on the lip across the face. On the
/// rungs past the head's height, master ce5fdb33 backed off the face, and
/// the climb fell short of the lip, dropped back onto the rungs and lost
/// about 100 ticks to an unstick spell on five of these nine starts (seeds
/// 189, 309, 503 and 631 of bot-navigation.md's sweep stalled there). Now
/// it climbs on over the top.
#[test]
#[ignore = "builds mp_ship's graph and runs a bot up a ladder from 5 starts; run with --ignored"]
fn a_bot_climbs_over_mp_ships_hold_ladder_top() {
    let Some(fs) = vcod_common::testing::game_fs_11() else {
        eprintln!("COD_DIR unset or has no main/: skipping");
        return;
    };
    let fs = Rc::new(fs);
    let bsp = vcod_common::bsp::parse(&fs.read(&fs.resolve_map("mp_ship").unwrap()).unwrap())
        .expect("parse the bsp");
    let (foot, deck) = ([4332.0, -94.0, 56.1], [4352.0, -122.0, 600.1]);
    for x in [4328.0, 4330.0, 4332.8, 4335.0, 4337.0] {
        let mut now = Instant::now();
        let mut cfg = cfg(1, false, "dm");
        cfg.map = "mp_ship".into();
        let mut sv = vcod_server::Server::new(cfg, now);
        sv.load_world(vcod_server::world::World::from_bsp(&bsp, Some(&fs)));
        sv.load_scripts(fs.clone()).expect("load the scripts");
        let slot = (0..600)
            .find_map(|_| {
                run(&mut sv, &mut now, 1);
                sv.bot_slots().into_iter().find(|s| {
                    sv.bot_body(*s).is_some_and(|b| b.playing)
                        && sv.test_nav_route(foot, deck).is_some()
                })
            })
            .expect("the bot plays on a built graph");
        sv.place_client(slot, [x, -94.0, 56.2], 90.0);
        sv.test_bot_goal(slot, Some(deck));
        let top = (0..260).any(|_| {
            run(&mut sv, &mut now, 1);
            sv.bot_body(slot)
                .is_some_and(|b| b.on_ground && b.origin[2] > 599.0)
        });
        assert!(
            top,
            "from x {x}: not over the top, at {:?}",
            sv.bot_body(slot).map(|b| b.origin)
        );
    }
}

/// mp_ship's mast ladder (rungs on the face at y -86.5 from the platform at
/// 896 to the spar at 1145): the head (3675, -95.1, 1147.1) stands on the
/// spar beside the rungs' top, and the creep back down from a running start
/// leaves their reach before it has dropped the 8 they hold from. Before,
/// the graph had no way down there; the creep from rest gets onto the rungs,
/// and a bot sent from the head to the platform climbs down.
#[test]
#[ignore = "builds mp_ship's graph and runs a bot down a ladder; run with --ignored"]
fn a_bot_climbs_down_mp_ships_mast_from_the_spar() {
    let Some(fs) = vcod_common::testing::game_fs_11() else {
        eprintln!("COD_DIR unset or has no main/: skipping");
        return;
    };
    let fs = Rc::new(fs);
    let bsp = vcod_common::bsp::parse(&fs.read(&fs.resolve_map("mp_ship").unwrap()).unwrap())
        .expect("parse the bsp");
    let (head, foot) = ([3675.0, -95.1, 1147.2], [3675.0, -103.0, 896.2]);
    let mut now = Instant::now();
    let mut cfg = cfg(1, false, "dm");
    cfg.map = "mp_ship".into();
    let mut sv = vcod_server::Server::new(cfg, now);
    sv.load_world(vcod_server::world::World::from_bsp(&bsp, Some(&fs)));
    sv.load_scripts(fs.clone()).expect("load the scripts");
    let slot = (0..600)
        .find_map(|_| {
            run(&mut sv, &mut now, 1);
            sv.bot_slots().into_iter().find(|s| {
                sv.bot_body(*s).is_some_and(|b| b.playing)
                    && sv.test_nav_route(head, foot).is_some()
            })
        })
        .expect("a route down from the spar");
    sv.place_client(slot, head, 0.0);
    sv.test_bot_goal(slot, Some(foot));
    let down = (0..200).any(|_| {
        run(&mut sv, &mut now, 1);
        sv.bot_body(slot)
            .is_some_and(|b| b.on_ground && (b.origin[2] - 896.1).abs() < 2.0)
    });
    assert!(
        down,
        "not down on the platform, at {:?}",
        sv.bot_body(slot).map(|b| b.origin)
    );
}

/// mp_depot's roof at 508 by the ladder at x -2750, and the ledges at 326
/// south and north of the ladder's foot (bot-navigation.md section 3, "The
/// lip walk"). Bots sent from across the roof onto either ledge, seed 183's
/// way among them (west along y -1246 with the south ledge below), get
/// down without falling past the ledge to the yard at -24 and stand on it.
/// Off that turn a bot once left the lip of the drop onto the south ledge
/// at 81 units/s to the proving walk's 14 and fell; the drop is now a risky
/// edge and the bots back down the ladder.
#[test]
#[ignore = "builds mp_depot's graph and runs a bot 28 times down off its roof; run with --ignored"]
fn a_bot_gets_down_off_mp_depots_roof_onto_the_ledges() {
    let Some(fs) = vcod_common::testing::game_fs_11() else {
        eprintln!("COD_DIR unset or has no main/: skipping");
        return;
    };
    let fs = Rc::new(fs);
    let bsp = vcod_common::bsp::parse(&fs.read(&fs.resolve_map("mp_depot").unwrap()).unwrap())
        .expect("parse bsp");
    let ledges = [[-2747.6, -1270.8, 326.2], [-2754.6, -1150.3, 326.2]];
    let mut failed = Vec::new();
    // Across the roof east of the ladder's head; (-2600, -1300) and
    // (-2730, -1300) stand on the floor at 344 south of it.
    let mut starts = Vec::new();
    for x in [-2600.0, -2650.0, -2700.0, -2730.0] {
        for y in [-1300.0, -1246.0, -1215.0, -1150.0] {
            if y != -1300.0 || x == -2650.0 || x == -2700.0 {
                starts.push([x, y]);
            }
        }
    }
    for ledge in ledges {
        for &[x, y] in &starts {
            let mut now = Instant::now();
            let mut cfg = cfg(1, false, "dm");
            cfg.map = "mp_depot".into();
            let mut sv = vcod_server::Server::new(cfg, now);
            sv.load_world(vcod_server::world::World::from_bsp(&bsp, Some(&fs)));
            sv.load_scripts(fs.clone()).expect("load scripts");
            let slot = (0..900)
                .find_map(|_| {
                    run(&mut sv, &mut now, 1);
                    sv.bot_slots().into_iter().find(|s| {
                        sv.bot_body(*s).is_some_and(|b| b.playing)
                            && sv
                                .test_nav_route([-2714.2, -1217.0, 527.5], ledge)
                                .is_some()
                    })
                })
                .expect("the bot plays on the built graph");
            let (z, _) = sv.test_floor_under([x, y, 600.0]).expect("the roof");
            assert!(z > 500.0, "({x}, {y}) is on the roof, not at {z}");
            sv.place_client(slot, [x, y, z + 0.2], 180.0);
            sv.test_bot_goal(slot, Some(ledge));
            let mut lowest = f32::MAX;
            let on_ledge = (0..250).any(|_| {
                run(&mut sv, &mut now, 1);
                let b = sv.bot_body(slot).unwrap();
                lowest = lowest.min(b.origin[2]);
                b.on_ground
                    && (b.origin[2] - ledge[2]).abs() < 4.0
                    && (b.origin[0] - ledge[0]).hypot(b.origin[1] - ledge[1]) < 24.0
            });
            if !on_ledge || lowest < 320.0 {
                failed.push(([x, y], ledge, lowest, sv.bot_body(slot).map(|b| b.origin)));
            }
        }
    }
    assert!(
        failed.is_empty(),
        "(start, ledge, lowest z, at): {failed:?}"
    );
}

/// Six shooting bots on a United Offensive server (protocol 22) in stock
/// `tdm` on mp_cassino for 90 s of game time: they fight with the weapons
/// UO's menus offer, never a light machine gun (UO fires one only off its
/// bipod) and never their own frag, sprint most of the long straight legs
/// they run, and no bot spends 12 s of play without an enemy within 150
/// units of one spot (bot-navigation.md, "Other rules"; a sniper holding a long duel stands
/// still on purpose), seated or not: they board, drive and leave the map's
/// tanks (bot-objectives.md 10). Needs `COD_DIR` at a UO install.
#[test]
#[ignore = "builds mp_cassino's UO graph and runs 6 bots for 90 s of game time, ~5 s in release; run with --ignored"]
fn uo_bots_fight_and_sprint_in_tdm() {
    if vcod_common::testing::uo_fs().is_none() {
        eprintln!("COD_DIR has no uo/: skipping");
        return;
    }
    // `SEED=<odd n>` replays another game (bot-navigation.md, "Other rules").
    uo_tdm_game(
        std::env::var("SEED").map_or(1, |v| v.parse().expect("SEED")),
        true,
    );
}

/// mp_cassino's roof dent at (4948, -1413, 280), off the graph between
/// roof nodes at 303 and walled 23 units high on every side: only a jump
/// gets out. Seed 17 on master 973e0e94 dropped a bot into it off the walk
/// from (4938, -1393, 303) to (4991, -1440, 303) and held it there 12 s,
/// pressing at the node at 280 beyond the wall (bot-objectives.md 10.6).
#[test]
#[ignore = "builds mp_cassino's UO graph and runs a bot for 30 s of game time; run with --ignored"]
fn a_uo_bot_in_mp_cassinos_roof_dent_jumps_out() {
    let Some(fs) = vcod_common::testing::uo_fs() else {
        eprintln!("COD_DIR has no uo/: skipping");
        return;
    };
    let bsp = vcod_common::bsp::parse(&fs.read(&fs.resolve_map("mp_cassino").unwrap()).unwrap())
        .expect("parse the bsp");
    let mut now = Instant::now();
    let mut cfg = cfg(1, false, "tdm");
    cfg.map = "mp_cassino".into();
    let mut sv = vcod_server::Server::new(cfg, now);
    sv.set_protocol(&vcod_common::net::protocol::PROTOCOL_V22);
    sv.set_cvar("vc_bot_vehicles", "0");
    sv.load_world(vcod_server::world::World::from_bsp(&bsp, Some(&fs)));
    sv.load_scripts(Rc::new(fs)).expect("load the scripts");
    let goal = [4896.0, -1337.0, 280.0];
    let slot = (0..900)
        .find_map(|_| {
            run(&mut sv, &mut now, 1);
            sv.bot_slots().into_iter().find(|s| {
                sv.bot_body(*s).is_some_and(|b| b.playing)
                    && sv.test_nav_route([4944.0, -1433.0, 303.0], goal).is_some()
            })
        })
        .expect("the bot plays on the built graph");
    for spot in [[4951.7, -1414.5, 280.125], [4943.0, -1418.0, 280.125]] {
        sv.place_client(slot, spot, 180.0);
        sv.test_bot_goal(slot, Some(goal));
        let out = (0..300).any(|_| {
            run(&mut sv, &mut now, 1);
            sv.bot_body(slot).is_some_and(|b| {
                b.on_ground && (b.origin[0] - 4948.0).hypot(b.origin[1] + 1413.0) > 40.0
            })
        });
        assert!(
            out,
            "from {spot:?} still in the dent at {:?}",
            sv.bot_body(slot).map(|b| b.origin)
        );
    }
}

/// The games of [`uo_bots_fight_and_sprint_in_tdm`] that failed it before
/// the fixes they name (bot-navigation.md "Jumps", bot-objectives.md 10.3,
/// 10.4 and 10.7), with vehicles off for two, one thread each.
#[test]
#[ignore = "runs eight 90 s mp_cassino UO games, ~10 s in release on 8 threads; run with --ignored"]
fn uo_tdm_games_that_stalled_or_never_sprinted() {
    if vcod_common::testing::uo_fs().is_none() {
        return;
    }
    // Seed, vehicles: 17 a jump's penalty, then a jump waited on that
    // lands only from a run and a roof dent off the graph, 13 and 19
    // pre-aim off the sprint (19's game also lobbed a stick grenade whose
    // bounce back killed its thrower), 43 two exits on one frame (a driver and a gunner leave one tank a frame
    // apart; seed 15's game lost that pair to the grenade rules' draws and
    // stalls a bot on a ledge instead), 25 a tank's headway, 33 sprint
    // legs; with vehicles off, 1 a plan from across a wall, 31 a stall.
    // Seed 45's rider headway is fixed, but its game now dithers a tank
    // elsewhere.
    let games = [
        (43, true),
        (17, true),
        (13, true),
        (19, true),
        (25, true),
        (33, true),
        (1, false),
        (31, false),
    ];
    std::thread::scope(|s| {
        let runs: Vec<_> = games
            .iter()
            .map(|&(seed, vehicles)| {
                std::thread::Builder::new()
                    .name(format!("seed {seed} vehicles {vehicles}"))
                    .spawn_scoped(s, move || uo_tdm_game(seed, vehicles))
                    .unwrap()
            })
            .collect();
        let failed: Vec<_> = runs
            .into_iter()
            .zip(games)
            .filter_map(|(r, g)| r.join().is_err().then_some(g))
            .collect();
        assert!(failed.is_empty(), "failed (seed, vehicles): {failed:?}");
    });
}

/// One game of [`uo_bots_fight_and_sprint_in_tdm`].
fn uo_tdm_game(seed: u64, vehicles: bool) {
    let fs = vcod_common::testing::uo_fs().expect("uo/");
    let bsp = vcod_common::bsp::parse(&fs.read(&fs.resolve_map("mp_cassino").unwrap()).unwrap())
        .expect("parse the bsp");
    let mut now = Instant::now();
    let mut cfg = cfg(6, true, "tdm");
    cfg.map = "mp_cassino".into();
    let mut sv = vcod_server::Server::new(cfg, now);
    sv.set_protocol(&vcod_common::net::protocol::PROTOCOL_V22);
    sv.test_seed_rng(seed);
    if !vehicles {
        sv.set_cvar("vc_bot_vehicles", "0");
    }
    sv.load_world(vcod_server::world::World::from_bsp(&bsp, Some(&fs)));
    sv.load_scripts(Rc::new(fs)).expect("load the scripts");
    let mut hist: [std::collections::VecDeque<[f32; 3]>; 8] = Default::default();
    let (mut sprints, mut legs) = (0, 0);
    for tick in 0..1800 {
        run(&mut sv, &mut now, 1);
        for slot in sv.bot_slots() {
            // A deployed LMG holds still on purpose.
            let Some(b) = sv.bot_body(slot).filter(|b| b.playing && !b.deployed) else {
                hist[slot].clear();
                continue;
            };
            sprints += usize::from(b.sprinting);
            legs += usize::from(b.sprint_leg);
            // An engaged bot holds still to shoot, and a driver for a
            // teammate walking up to board; the window counts the rest.
            if b.engaged || b.crew_wait {
                continue;
            }
            let h = &mut hist[slot];
            h.push_back(b.origin);
            if h.len() > 240 {
                h.pop_front();
            }
            let first = h[0];
            let still = h.len() == 240
                && h.iter().all(|p| {
                    (0..3).map(|i| (p[i] - first[i]).powi(2)).sum::<f32>() < 150.0 * 150.0
                });
            assert!(
                !still,
                "tick {tick}: bot {slot} 12 s at {first:?} seat {:?}",
                b.seat
            );
        }
    }
    let kills: Vec<Vec<String>> = sv
        .script_log()
        .iter()
        .filter_map(|l| {
            // Not the `TANK;` of a `D;` line's `MOD_CRUSH_TANK`.
            l.trim_end()
                .split_once("K;")
                .filter(|(pre, _)| !pre.ends_with(|c: char| c.is_ascii_alphanumeric()))
                .map(|(_, k)| k)
        })
        .map(|k| k.split(';').map(str::to_string).collect())
        .collect();
    eprintln!(
        "{} kills, {sprints} sprinting bot-ticks, {legs} on sprint legs",
        kills.len()
    );
    assert!(kills.len() >= 5, "only {} kills in 90 s", kills.len());
    for k in &kills {
        // victim guid, num, team, name, attacker guid, num, team, name,
        // weapon, damage, means of death, hit location
        let (weapon, mod_) = (&k[8], &k[10]);
        let frag = ["grenade", "granate", "frag"]
            .iter()
            .any(|g| weapon.contains(g));
        assert!(
            !(k[1] == k[5] && frag),
            "a bot's own frag killed it ({mod_}): {k:?}"
        );
    }
    // How many straight legs a game routes over rides the seed (243 to 1242
    // bot-ticks over seeds 1-23 odd); of those, a bot sprints the ones it
    // has the breath for (82 to 99 per cent).
    assert!(legs > 100, "only {legs} bot-ticks on sprint legs");
    assert!(
        sprints * 2 >= legs,
        "{sprints} sprinting bot-ticks of {legs} on sprint legs"
    );
    // Stock UO tdm reads `level.teamkill_penalty`, which nothing sets, on a
    // death to no player (a tank its own attacker): retail's own error,
    // which it runs on past below `developer 1` (coduo-vehicles.md 11.6,
    // cod11-gsc-language.md "Runtime errors below developer 1").
    let aborts: Vec<String> = sv
        .script_aborts()
        .into_iter()
        .filter(|a| !a.starts_with("maps/mp/gametypes/tdm::Callback_PlayerKilled:815:"))
        .collect();
    assert_eq!(aborts, Vec::<String>::new());
}

/// United Offensive's walled-in spawns: mp_cassino's allied upper floor at
/// (180, 1150, 208), whose way down is a window sill 240 over the street,
/// and mp_streets' allied rooms at (-260, -6900, 8), left by a window
/// whose sill a standing body fits on only off every lattice line, and
/// the three rooms left only by a crouched jump through a window
/// (mp_cassino's axis room, mp_streets' at z 101 and -24). A bot placed in
/// each leaves within 30 s of game time (bot-navigation.md, "Other
/// rules"). Needs `COD_DIR` at a UO install.
#[test]
#[ignore = "builds mp_cassino's and mp_streets' UO graphs and runs a bot in five rooms, ~6 s in release; run with --ignored"]
fn uo_bots_leave_their_walled_in_spawns() {
    let Some(fs) = vcod_common::testing::uo_fs() else {
        return;
    };
    let fs = Rc::new(fs);
    // A map, a spot, and the box the bot has to leave.
    type Case = (&'static str, [f32; 3], [f32; 3], [f32; 3]);
    let cases: [Case; 5] = [
        (
            "mp_cassino",
            [180.0, 1150.0, 224.0],
            [130.0, 990.0, 190.0],
            [240.0, 1350.0, 300.0],
        ),
        (
            "mp_streets",
            [-230.0, -6860.0, 24.0],
            [-370.0, -7080.0, 0.0],
            [-155.0, -6780.0, 100.0],
        ),
        // The rooms left only by a crouched jump through a window: the
        // bot is out once it is below the sill's floor.
        (
            "mp_cassino",
            [5420.0, -1500.0, 144.0],
            [5200.0, -1760.0, 100.0],
            [5620.0, -1220.0, 300.0],
        ),
        (
            "mp_streets",
            [430.0, -6740.0, 117.0],
            [240.0, -6950.0, 90.0],
            [620.0, -6520.0, 300.0],
        ),
        (
            "mp_streets",
            [580.0, -3250.0, -8.0],
            [440.0, -3415.0, -40.0],
            [720.0, -3070.0, 60.0],
        ),
    ];
    for (map, spot, lo, hi) in cases {
        let bsp = vcod_common::bsp::parse(&fs.read(&fs.resolve_map(map).unwrap()).unwrap())
            .expect("parse the bsp");
        let mut now = Instant::now();
        let mut cfg = cfg(1, false, "tdm");
        cfg.map = map.into();
        let mut sv = vcod_server::Server::new(cfg, now);
        sv.set_protocol(&vcod_common::net::protocol::PROTOCOL_V22);
        sv.load_world(vcod_server::world::World::from_bsp(&bsp, Some(&fs)));
        sv.load_scripts(fs.clone()).expect("load the scripts");
        let slot = (0..400)
            .find_map(|_| {
                run(&mut sv, &mut now, 1);
                sv.bot_slots()
                    .into_iter()
                    .find(|s| sv.bot_body(*s).is_some_and(|b| b.playing))
            })
            .expect("the bot plays");
        sv.place_client(slot, spot, 0.0);
        let inside = |p: [f32; 3]| (0..3).all(|i| p[i] > lo[i] && p[i] < hi[i]);
        let out = (0..600).any(|_| {
            run(&mut sv, &mut now, 1);
            sv.bot_body(slot)
                .is_some_and(|b| b.playing && !inside(b.origin))
        });
        assert!(
            out,
            "{map}: from {spot:?} still in at {:?}",
            sv.bot_body(slot).map(|b| b.origin)
        );
    }
}

/// Two bots, one a side, play UO base assault on mp_foy with nobody
/// shooting: each, a lone attacker, fetches rocket launchers, breaches the
/// other side's bases from a stand with a shot at them, plants and sees
/// the charge off, until one side has no base left (bot-objectives.md
/// 7.2). Bases take three direct rockets here (`scr_bas_basehealth 3000`);
/// the stock 24500 is 25, and the round runs half an hour. Needs `COD_DIR`
/// at a UO install.
#[test]
#[ignore = "builds mp_foy's UO graph and its rebuilds, runs 2 bots for up to 29 min of game time, ~4 min in release; run with --ignored"]
fn uo_bots_breach_bases_until_a_bas_round_ends() {
    for map in bas_maps() {
        bas_round(&map, 2, false);
    }
}

/// Four bots, two a side, shooting: an attacker and a defender each. The
/// planter holds its charge's room against the defuse and clears the blast
/// before it goes, and no bot fires a rocket at an enemy close enough to
/// take it too (bot-objectives.md 7.2). Needs `COD_DIR` at a UO install.
#[test]
#[ignore = "builds mp_foy's UO graph and its rebuilds, runs 4 bots for up to 29 min of game time; run with --ignored"]
fn four_shooting_uo_bots_end_a_bas_round() {
    for map in bas_maps() {
        let Some(sv) = bas_round(&map, 4, true) else {
            return;
        };
        own_rocket_kills(&sv);
    }
}

fn own_rocket_kills(sv: &vcod_server::Server) {
    for k in kills(sv) {
        let (weapon, mod_) = (&k[8], &k[10]);
        let rocket = ["bazooka", "panzerschreck", "panzerfaust"]
            .iter()
            .any(|r| weapon.contains(r));
        // UO's lines can log 0 for both numbers; the names tell.
        assert!(
            !(k[1] == k[5] && k[3] == k[7] && rocket),
            "a bot's own rocket killed it ({mod_}): {k:?}"
        );
    }
}

/// Four bots, two a side, nobody shooting: a defender that cannot shoot
/// attacks too, on another base than its partner's, and defuses only a
/// charge it passes near, so a charge goes off while its base's side is
/// away (bot-objectives.md 7.2). Needs `COD_DIR` at a UO install.
#[test]
#[ignore = "builds mp_foy's UO graph and its rebuilds, runs 4 bots for up to 29 min of game time; run with --ignored"]
fn four_uo_bots_end_a_bas_round_without_shooting() {
    for map in bas_maps() {
        bas_round(&map, 4, false);
    }
}

/// The maps the bas gates play: `BAS_MAP`, a comma list or `all` for the
/// five stock bas maps, mp_foy by default.
fn bas_maps() -> Vec<String> {
    match std::env::var("BAS_MAP").as_deref() {
        Ok("all") => [
            "mp_foy",
            "mp_italy",
            "mp_kursk",
            "mp_ponyri",
            "mp_rhinevalley",
        ]
        .map(String::from)
        .to_vec(),
        Ok(list) => list.split(',').map(String::from).collect(),
        Err(_) => vec!["mp_foy".into()],
    }
}

/// The script log's kills: victim guid, num, team, name, attacker guid,
/// num, team, name, weapon, damage, means of death, hit location.
fn kills(sv: &vcod_server::Server) -> Vec<Vec<String>> {
    sv.script_log()
        .iter()
        .filter_map(|l| {
            // Not the `TANK;` of a `D;` line's `MOD_CRUSH_TANK`.
            l.trim_end()
                .split_once("K;")
                .filter(|(pre, _)| !pre.ends_with(|c: char| c.is_ascii_alphanumeric()))
                .map(|(_, k)| k)
        })
        .map(|k| k.split(';').map(str::to_string).collect())
        .collect()
}

/// `bots` bots play `map`'s bas round, seed 7 (`BOTS_SEED`; the seed is
/// made odd), three rockets a base; the round has to end on its bases.
/// `None` without a UO install.
fn bas_round(map: &str, bots: usize, shoot: bool) -> Option<vcod_server::Server> {
    let Some(fs) = vcod_common::testing::uo_fs() else {
        eprintln!("COD_DIR has no uo/: skipping");
        return None;
    };
    let bsp = vcod_common::bsp::parse(&fs.read(&fs.resolve_map(map).unwrap()).unwrap())
        .expect("parse the bsp");
    let mut now = Instant::now();
    let mut cfg = cfg(bots, shoot, "bas");
    cfg.map = map.into();
    let mut sv = vcod_server::Server::new(cfg, now);
    sv.set_protocol(&vcod_common::net::protocol::PROTOCOL_V22);
    sv.test_seed_rng(std::env::var("BOTS_SEED").map_or(7, |v| v.parse().expect("BOTS_SEED")));
    sv.set_cvar("scr_bas_basehealth", "3000");
    sv.load_world(vcod_server::world::World::from_bsp(&bsp, Some(&fs)));
    sv.load_scripts(Rc::new(fs)).expect("load the scripts");
    let count = |sv: &vcod_server::Server, event: &str| {
        sv.script_log()
            .iter()
            .filter(|l| l.trim_end().ends_with(event))
            .count()
    };
    let mut first = None;
    let mut end = None;
    // `BAS_TRACE=1`: the bots (clip, engaged, waypoint) and the bases
    // every 5 s (`BAS_TRACE_EVERY` ticks), and each `A;`, `K;` and `D;` line.
    let trace = std::env::var_os("BAS_TRACE").is_some();
    let mut logged = 0;
    for tick in 0..BAS_ROUND_TICKS {
        run(&mut sv, &mut now, 1);
        if trace {
            let log = sv.script_log();
            // A restart starts a new log.
            if log.len() < logged {
                logged = 0;
            }
            for l in &log[logged..] {
                if l.contains("A;") || l.contains("K;") || l.contains("D;") {
                    eprintln!("{tick}: {}", l.trim_end());
                }
            }
            logged = log.len();
            let every = std::env::var("BAS_TRACE_EVERY").map_or(100, |v| v.parse().unwrap());
            if tick % every == 0 {
                for slot in sv.bot_slots() {
                    let team = sv.bot_team(slot);
                    if let Some(b) = sv.bot_body(slot) {
                        let wp = sv.bot_waypoints(slot).0;
                        eprintln!(
                            "{tick}: bot {slot} team {team} at {:?} dead {} hp {} clip {} engaged {} wp {:?}",
                            b.origin.map(|v| v as i32),
                            b.dead,
                            b.health,
                            b.clip,
                            b.engaged,
                            wp.map(|w| w.map(|v| v as i32))
                        );
                    }
                }
                if let Some(vcod_server::game::script::UoObjectives::Bas(bases)) = sv.bot_uo() {
                    for b in bases {
                        eprintln!(
                            "{tick}: base team {} at {:?} breached {} planted {} blows_in {:?} charge {:?}",
                            b.team,
                            b.at.map(|v| v as i32),
                            b.breached,
                            b.planted,
                            b.blows_in,
                            b.charge.map(|c| c.1.map(|v| v as i32))
                        );
                    }
                }
            }
        }
        if first.is_none() && count(&sv, "bas_breached") > 0 {
            first = Some(tick);
        }
        // `bas.gsc`'s round end logs the winners (`W;`).
        if sv.script_log().iter().any(|l| l.starts_with("W;")) {
            end = Some(tick);
            break;
        }
    }
    eprintln!(
        "{map}: {bots} bots, shoot {shoot}: first breach at tick {first:?}, round over at {end:?}: \
         {} breached, {} planted, {} destroyed, {} defused, {} kills, {} boardings",
        count(&sv, "bas_breached"),
        count(&sv, "bas_planted"),
        count(&sv, "bas_destroyed"),
        count(&sv, "bas_defused"),
        kills(&sv).len(),
        sv.bot_slots()
            .iter()
            .filter_map(|s| sv.bot_body(*s))
            .map(|b| b.boardings)
            .sum::<u32>(),
    );
    assert!(
        first.is_some_and(|t| t < BAS_BREACH_TICKS),
        "no base breached in {} s",
        BAS_BREACH_TICKS / 20
    );
    assert!(end.is_some(), "the round never ended on its bases");
    assert!(
        count(&sv, "bas_destroyed") >= 3,
        "the round ended on its clock"
    );
    // Stock `BaseAssault_bomb_think` decrements a `count` it never sets
    // when a charge's trigger fires outside a round, and at `developer 0`
    // runs on to compare it: a bot looking at a live charge in the
    // end-of-round delay (bot-objectives.md 7.1).
    let aborts: Vec<String> = sv
        .script_aborts()
        .into_iter()
        .filter(|a| {
            !["3152:", "3153:"].iter().any(|l| {
                a.starts_with(&format!(
                    "maps/mp/gametypes/bas::BaseAssault_bomb_think:{l}"
                ))
            })
        })
        .collect();
    assert_eq!(aborts, Vec::<String>::new());
    Some(sv)
}

/// The same seeded bas match twice in one process, the second beside a
/// match without shooting on the map and a busy thread, all three on one
/// nav cache: both play out alike. The cache once keyed a rebuild on the
/// brush links alone, so a gate got the graph whichever test beside it
/// reached those links first had built round its own vehicles
/// (bot-navigation.md, "Build times"). Needs `COD_DIR` at a UO install.
#[test]
#[ignore = "builds mp_foy's UO graph and its rebuilds, runs three 4-bot matches of 4 min of game time, ~10 min in release; run with --ignored"]
fn a_seeded_bas_match_replays_beside_another() {
    if vcod_common::testing::uo_fs().is_none() {
        return;
    }
    vcod_server::nav::clear_cache();
    let first = bas_fingerprint(true);
    vcod_server::nav::clear_cache();
    let done = std::sync::atomic::AtomicBool::new(false);
    let again = std::thread::scope(|s| {
        s.spawn(|| bas_fingerprint(false));
        s.spawn(|| {
            let mut x = 1u64;
            while !done.load(std::sync::atomic::Ordering::Relaxed) {
                x = std::hint::black_box(x.wrapping_mul(6364136223846793005).wrapping_add(1));
            }
        });
        let again = bas_fingerprint(true);
        done.store(true, std::sync::atomic::Ordering::Relaxed);
        again
    });
    assert_eq!(
        first.len(),
        again.len(),
        "the matches ran different lengths"
    );
    let diverged = first.iter().zip(&again).position(|(a, b)| a != b);
    assert_eq!(
        diverged,
        None,
        "the replay diverged at {:?}",
        diverged.map(|i| (&first[i], &again[i]))
    );
}

/// Four bots on mp_foy's bas round, seed 7, shooting or not, for
/// `BAS_FINGERPRINT_TICKS`: every 20 ticks each bot's origin and health,
/// then each new script log line.
fn bas_fingerprint(shoot: bool) -> Vec<String> {
    let fs = vcod_common::testing::uo_fs().expect("a UO install");
    let bsp = vcod_common::bsp::parse(&fs.read(&fs.resolve_map("mp_foy").unwrap()).unwrap())
        .expect("parse the bsp");
    let mut now = Instant::now();
    let mut cfg = cfg(4, shoot, "bas");
    cfg.map = "mp_foy".into();
    let mut sv = vcod_server::Server::new(cfg, now);
    sv.set_protocol(&vcod_common::net::protocol::PROTOCOL_V22);
    sv.test_seed_rng(7);
    sv.set_cvar("scr_bas_basehealth", "3000");
    sv.load_world(vcod_server::world::World::from_bsp(&bsp, Some(&fs)));
    sv.load_scripts(Rc::new(fs)).expect("load the scripts");
    let mut out = Vec::new();
    let mut logged = 0;
    for tick in 0..BAS_FINGERPRINT_TICKS {
        run(&mut sv, &mut now, 1);
        let log = sv.script_log();
        if log.len() < logged {
            logged = 0;
        }
        out.extend(
            log[logged..]
                .iter()
                .map(|l| format!("{tick}: {}", l.trim_end())),
        );
        logged = log.len();
        if tick % 20 == 0 {
            for slot in sv.bot_slots() {
                if let Some(b) = sv.bot_body(slot) {
                    out.push(format!("{tick}: bot {slot} {:?} {}", b.origin, b.health));
                }
            }
        }
    }
    out
}

/// Four minutes: past the first breaches and the rebuilds after them.
const BAS_FINGERPRINT_TICKS: usize = 4 * 60 * 20;

/// `scr_bas_roundlength` is 30 minutes; the round ends by then whatever
/// the bases did, so the gate stops short of it.
const BAS_ROUND_TICKS: usize = 29 * 60 * 20;
const BAS_BREACH_TICKS: usize = 5 * 60 * 20;

/// mp_foy's hollow at (4620, -2225, -14): its rims stand 38 to 58 over its
/// floor, and the graph drops into it with no way out, so a roam from in
/// it picks only its own nodes. Master 447d904f's UO tdm seed 51 held a bot
/// there 751 bot-ticks, jumping at the lip of a marginal edge out that the
/// graph no longer proves; now a bot in it types `kill`
/// (bot-navigation.md section 3).
#[test]
#[ignore = "builds mp_foy's UO graph and runs a bot for 20 s of game time; run with --ignored"]
fn a_bot_in_mp_foys_hollow_kills_itself() {
    let Some(fs) = vcod_common::testing::uo_fs() else {
        eprintln!("COD_DIR has no uo/: skipping");
        return;
    };
    let bsp = vcod_common::bsp::parse(&fs.read(&fs.resolve_map("mp_foy").unwrap()).unwrap())
        .expect("parse the bsp");
    let mut now = Instant::now();
    let mut cfg = cfg(1, false, "tdm");
    cfg.map = "mp_foy".into();
    let mut sv = vcod_server::Server::new(cfg, now);
    sv.set_protocol(&vcod_common::net::protocol::PROTOCOL_V22);
    sv.load_world(vcod_server::world::World::from_bsp(&bsp, Some(&fs)));
    sv.load_scripts(Rc::new(fs)).expect("load the scripts");
    let slot = (0..2000)
        .find_map(|_| {
            run(&mut sv, &mut now, 1);
            sv.bot_slots()
                .into_iter()
                .find(|s| sv.bot_body(*s).is_some_and(|b| b.playing))
        })
        .expect("the bot plays");
    // The graph builds on its own thread; a bot off it does not roam.
    assert!(
        (0..2000).any(|_| {
            run(&mut sv, &mut now, 1);
            sv.test_nav_route([4612.0, -2234.0, -15.0], [4652.0, -2208.0, -13.0])
                .is_some()
        }),
        "no graph"
    );
    sv.place_client(slot, [4635.0, -2241.0, -12.0], 0.0);
    let died = (0..400).any(|_| {
        run(&mut sv, &mut now, 1);
        sv.bot_body(slot).is_some_and(|b| b.dead)
    });
    assert!(
        died,
        "still in the hollow at {:?}",
        sv.bot_body(slot).map(|b| b.origin)
    );
}

/// A fighting bot yells through the stock quick messages: its `mr` on
/// `quickstatements` runs `_teams.gsc`'s `doQuickMessage`, the one script
/// that puts up the quick-message head icon (bots/yell.rs).
#[test]
fn a_fighting_bot_yells_a_stock_quick_message() {
    let Some((mut sv, mut now)) = server_with(2, true) else {
        eprintln!("COD_DIR unset or has no main/: skipping");
        return;
    };
    run(&mut sv, &mut now, 100);
    let slots = sv.bot_slots();
    let pair = [slots[0], slots[1]];
    let spot = fight_spot(&sv, sv.bot_body(slots[0]).unwrap().origin);
    face_off(&mut sv, pair, spot);
    // A yell is a roll (bots/yell.rs); five minutes give a few dozen.
    for tick in 0..6000 {
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
        if slots.iter().any(|&s| {
            sv.client_field(s, "headicon").as_deref() == Some("gfx/hud/headicon@quickmessage")
        }) {
            return;
        }
    }
    panic!("five minutes of fighting and no bot yelled");
}
