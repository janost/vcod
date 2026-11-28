//! Bots in United Offensive vehicles (`bots::vehicle`): a bot set down
//! beside mp_cassino's tanks walks up to one and boards it with the use
//! key, drives it along the graph, and gets out again. Needs `COD_DIR` at a
//! UO install; each test builds mp_cassino's graph, so they are ignored.

use crate::common;
use std::time::{Duration, Instant};

const MAP: &str = "mp_cassino";
const FRAME: Duration = Duration::from_millis(50);

fn uo_server(bots: usize, shoot: bool) -> Option<(vcod_server::Server, Instant)> {
    uo_server_on(MAP, bots, shoot)
}

fn uo_server_on(map: &str, bots: usize, shoot: bool) -> Option<(vcod_server::Server, Instant)> {
    let fs = vcod_common::testing::uo_fs()?;
    let cfg = vcod_server::ServerConfig {
        bots,
        bots_shoot: shoot,
        ..common::cfg(map, "tdm")
    };
    let (mut sv, now) = common::Boot {
        proto: Some(&vcod_common::net::protocol::PROTOCOL_V22),
        ..common::Boot::on(cfg)
    }
    .start(fs);
    // The bots stay on foot through the warm-up, whatever their spawn has
    // near it; `warm_up` lets them board.
    sv.set_cvar("vc_bot_vehicles", "0");
    Some((sv, now))
}

/// Five seconds for the bots to join and spawn, then vehicles on.
fn warm_up(sv: &mut vcod_server::Server, now: &mut Instant) {
    run(sv, now, 100);
    sv.set_cvar("vc_bot_vehicles", "1");
}

fn run(sv: &mut vcod_server::Server, now: &mut Instant, ticks: usize) {
    for _ in 0..ticks {
        *now += FRAME;
        sv.tick(*now);
    }
}

/// Sets the bot down on the ground beside the hull of `spot`, off its
/// left side.
fn beside(sv: &mut vcod_server::Server, slot: usize, spot: &vcod_server::bots::VehicleSpot) {
    let (s, c) = spot.yaw.to_radians().sin_cos();
    let off = spot.maxs[1] + 70.0;
    let at = [
        spot.origin[0] - s * off,
        spot.origin[1] + c * off,
        spot.origin[2] + 64.0,
    ];
    let ground = sv.test_ground_under(at).expect("ground beside the tank");
    sv.place_client(slot, [ground[0], ground[1], ground[2] + 0.125], spot.yaw);
}

/// The bot's seat, if it sits in one, run tick by tick up to `ticks`.
fn run_until_seated(
    sv: &mut vcod_server::Server,
    now: &mut Instant,
    slot: usize,
    ticks: usize,
) -> Option<(u32, i32)> {
    (0..ticks).find_map(|_| {
        run(sv, now, 1);
        sv.bot_body(slot)?.seat
    })
}

/// A lone bot (nobody to fight) set down beside a tank boards it within
/// 10 s, in the driver's seat.
#[test]
#[ignore = "builds mp_cassino's UO graph, ~4 s; run with --ignored"]
fn a_bot_beside_a_tank_boards_it() {
    let Some((mut sv, mut now)) = uo_server(1, false) else {
        eprintln!("COD_DIR has no uo/: skipping");
        return;
    };
    warm_up(&mut sv, &mut now);
    let slot = sv.bot_slots()[0];
    assert!(sv.bot_body(slot).unwrap().playing, "the bot never spawned");
    let tanks = sv.test_vehicles();
    assert!(!tanks.is_empty(), "mp_cassino's tdm has no vehicle");
    beside(&mut sv, slot, &tanks[0]);
    let seat = run_until_seated(&mut sv, &mut now, slot, 200);
    assert_eq!(
        seat,
        Some((tanks[0].num, 1)),
        "the bot never took the tank's driver's seat"
    );
    assert_eq!(sv.script_aborts(), Vec::<String>::new());
}

/// Once in, the bot drives the tank off along the graph toward its roam,
/// and backs off and gets out when it is stuck or the ride is long.
#[test]
#[ignore = "builds mp_cassino's UO graph and runs a bot 100 s of game time, ~5 s; run with --ignored"]
fn a_bot_drives_a_tank_off_and_gets_out() {
    let Some((mut sv, mut now)) = uo_server(1, false) else {
        eprintln!("COD_DIR has no uo/: skipping");
        return;
    };
    warm_up(&mut sv, &mut now);
    let tank = sv.test_vehicles()[0];
    drives_off_and_gets_out(&mut sv, &mut now, tank);
}

/// As the tank, a jeep on mp_kharkov's `tdm`: a four-wheel steers only
/// rolling and has no gun for its driver.
#[test]
#[ignore = "builds mp_kharkov's UO graph and runs a bot 100 s of game time, ~13 s; run with --ignored"]
fn a_bot_drives_a_jeep_off_and_gets_out() {
    let Some((mut sv, mut now)) = uo_server_on("mp_kharkov", 1, false) else {
        eprintln!("COD_DIR has no uo/: skipping");
        return;
    };
    warm_up(&mut sv, &mut now);
    let jeep = sv
        .test_vehicles()
        .into_iter()
        .find(|v| sv.test_vehicle_kind(v.num) == Some(1))
        .expect("mp_kharkov's tdm has no four-wheel");
    drives_off_and_gets_out(&mut sv, &mut now, jeep);
}

/// The lone bot set down beside `veh` boards it in the driver's seat,
/// drives it over 500 units off and gets out again.
fn drives_off_and_gets_out(
    sv: &mut vcod_server::Server,
    now: &mut Instant,
    veh: vcod_server::bots::VehicleSpot,
) {
    let slot = sv.bot_slots()[0];
    beside(sv, slot, &veh);
    let Some((num, 1)) = run_until_seated(sv, now, slot, 200) else {
        panic!("the bot never took the driver's seat");
    };
    let mut far = 0.0f32;
    let mut out = None;
    for tick in 0..2000 {
        run(sv, now, 1);
        let at = sv
            .test_vehicles()
            .into_iter()
            .find(|v| v.num == num)
            .expect("the vehicle is gone")
            .origin;
        far = far.max((at[0] - veh.origin[0]).hypot(at[1] - veh.origin[1]));
        if std::env::var("VEH_REPORT").is_ok() && tick % 20 == 0 {
            eprintln!(
                "{tick} vehicle {at:?} seat {:?}",
                sv.bot_body(slot).unwrap().seat
            );
        }
        if sv.bot_body(slot).unwrap().seat.is_none() {
            out = Some(tick);
            break;
        }
    }
    eprintln!("drove {far:.0} units, out at tick {out:?}");
    assert!(far > 500.0, "the vehicle went only {far:.0} units");
    assert!(out.is_some(), "the bot never got out");
    assert!(sv.bot_body(slot).unwrap().playing);
    assert_eq!(sv.script_aborts(), Vec::<String>::new());
}

/// The kills a vehicle's gun scored, from the script log's `K;` lines:
/// (attacker slot, weapon).
fn gun_kills(sv: &vcod_server::Server) -> Vec<(String, String)> {
    sv.script_log()
        .iter()
        .filter_map(|l| {
            // Not the `TANK;` of a `D;` line's `MOD_CRUSH_TANK`.
            l.trim_end()
                .split_once("K;")
                .filter(|(pre, _)| !pre.ends_with(|c: char| c.is_ascii_alphanumeric()))
                .map(|(_, k)| k)
        })
        .map(|k| k.split(';').map(str::to_string).collect::<Vec<_>>())
        .filter(|k| k.len() > 8)
        .map(|k| (k[5].clone(), k[8].clone()))
        .collect()
}

/// A bot driving a tank holds it still for an enemy in sight, lays the
/// turret on him and fires; a teammate who boards after takes the
/// gunner's seat and fires its MG.
#[test]
#[ignore = "builds mp_cassino's UO graph and runs three bots 40 s of game time, ~7 s; run with --ignored"]
fn tank_bots_lay_their_guns_on_an_enemy() {
    let Some((mut sv, mut now)) = uo_server(3, true) else {
        eprintln!("COD_DIR has no uo/: skipping");
        return;
    };
    warm_up(&mut sv, &mut now);
    let slots = sv.bot_slots();
    let teams: Vec<i32> = slots.iter().map(|s| sv.bot_team(*s)).collect();
    let (crew, enemy): (Vec<usize>, Vec<usize>) = slots
        .iter()
        .partition(|s| teams[slots.iter().position(|x| x == *s).unwrap()] == teams[0]);
    let (crew, enemy) = if crew.len() == 2 {
        (crew, enemy)
    } else {
        (enemy, crew)
    };
    assert_eq!((crew.len(), enemy.len()), (2, 1), "teams {slots:?}");
    let tank = sv.test_vehicles()[0];
    // The enemy waits out of sight at the far end of the map while the
    // crew boards.
    let (s, c) = tank.yaw.to_radians().sin_cos();
    let park = |sv: &mut vcod_server::Server| {
        let p = sv.test_ground_under([0.0, -1250.0, 64.0]).expect("ground");
        sv.place_client(enemy[0], [p[0], p[1], p[2] + 0.125], 0.0);
    };
    park(&mut sv);
    beside(&mut sv, crew[0], &tank);
    assert_eq!(
        run_until_seated(&mut sv, &mut now, crew[0], 200).map(|s| s.1),
        Some(1)
    );
    beside(&mut sv, crew[1], &tank);
    let gunner = (0..200).find_map(|_| {
        park(&mut sv);
        run(&mut sv, &mut now, 1);
        sv.bot_body(crew[1])?.seat
    });
    assert_eq!(
        gunner.map(|s| s.1),
        Some(2),
        "the teammate never took the gun"
    );
    // The enemy in the open ahead of the hull.
    let ahead = [
        tank.origin[0] + c * 700.0,
        tank.origin[1] + s * 700.0,
        tank.origin[2] + 64.0,
    ];
    let p = sv
        .test_ground_under(ahead)
        .expect("ground ahead of the tank");
    sv.place_client(enemy[0], [p[0], p[1], p[2] + 0.125], tank.yaw + 180.0);
    run(&mut sv, &mut now, 800);
    let kills = gun_kills(&sv);
    eprintln!("kills {kills:?}");
    // A tank's guns are its file's `turret_mp` and `gunner_mp` weapons.
    let crew: Vec<String> = crew.iter().map(|s| s.to_string()).collect();
    assert!(
        kills
            .iter()
            .any(|(by, w)| crew.contains(by) && (w.contains("turret") || w.contains("gunner"))),
        "no kill off the tank's guns: {kills:?}"
    );
    assert_eq!(sv.script_aborts(), Vec::<String>::new());
}

/// A bot on foot with a rocket launcher draws it on an enemy driving a
/// tank in its sight and fires (the tank's driver is no target for its
/// rifle: no round reaches him), and the tank is wrecked. The shooter is
/// held dead behind the hull until he fires, so the rocket strikes inside
/// the 20 degree cone where the behind scale applies.
const ROCKET_SEED: u64 = 2;

#[test]
#[ignore = "builds mp_cassino's UO graph and runs two bots 20 s of game time, ~5 s; run with --ignored"]
fn a_bot_fires_rockets_at_an_enemy_tank() {
    const BEHIND: f32 = 600.0;
    let Some((mut sv, mut now)) = uo_server(2, true) else {
        eprintln!("COD_DIR has no uo/: skipping");
        return;
    };
    warm_up(&mut sv, &mut now);
    let slots = sv.bot_slots();
    let (driver, shooter) = (slots[0], slots[1]);
    assert_ne!(sv.bot_team(driver), sv.bot_team(shooter));
    let tank = sv.test_vehicles()[0];
    let park = |sv: &mut vcod_server::Server| {
        let p = sv.test_ground_under([0.0, -1250.0, 64.0]).expect("ground");
        sv.place_client(shooter, [p[0], p[1], p[2] + 0.125], 0.0);
    };
    park(&mut sv);
    beside(&mut sv, driver, &tank);
    let seated = (0..200).find_map(|_| {
        park(&mut sv);
        run(&mut sv, &mut now, 1);
        sv.bot_body(driver)?.seat
    });
    assert_eq!(seated.map(|s| s.1), Some(1), "the driver never sat");
    // The shooter stands behind the tank, which holds still for him while
    // its turret turns round.
    assert!(sv.test_give_weapon(shooter, "bazooka_mp"));
    assert!(sv.test_give_max_ammo(shooter, "bazooka_mp"));
    let at = sv.test_vehicles()[0];
    let (s, c) = at.yaw.to_radians().sin_cos();
    let behind = [
        at.origin[0] - c * BEHIND,
        at.origin[1] - s * BEHIND,
        at.origin[2] + 64.0,
    ];
    let p = sv
        .test_ground_under(behind)
        .expect("ground behind the tank");
    let spot = [p[0], p[1], p[2] + 0.125];
    sv.place_client(shooter, spot, at.yaw);
    // `bazooka_mp`'s spread (4 sighted, 7 to 12 from the hip) carries the
    // rocket 50 units across the rear face, so where it strikes is the
    // `gunrandom` draw's: a seed whose draw strikes inside the cone (2 of
    // seeds 1-5 do).
    sv.test_seed_rng(ROCKET_SEED);
    // Held on the hull's back axis until the rocket goes, by a script
    // `setOrigin` (no reset of the launcher coming up):
    // `vehicle::damage_scale` takes the behind scale only within 20 degrees
    // of that axis, and left to walk aside while the launcher came up he
    // struck the rear face 19.99 degrees off it on one build and 20.17 on
    // the next, the side's half, and the tank lived (bot-combat.md 11.3).
    let fired = (0..400).find(|_| {
        run(&mut sv, &mut now, 1);
        let shot = sv.bot_body(shooter).is_some_and(|b| b.rockets_at_armor > 0);
        if !shot {
            sv.test_script_set_origin(shooter, spot);
        }
        shot
    });
    eprintln!("first rocket at tick {fired:?}");
    assert!(fired.is_some(), "no rocket at the enemy tank");
    // The hit sets the hull burning; it blows up a few seconds on.
    let wrecked = (0..600).find(|_| {
        run(&mut sv, &mut now, 1);
        !sv.test_vehicles().iter().any(|v| v.num == at.num)
    });
    eprintln!("the tank blew up {wrecked:?} ticks after");
    assert!(wrecked.is_some(), "the tank outlived the rocket");
    assert_eq!(sv.script_aborts(), Vec::<String>::new());
}

/// A wrecked tank's duplicate, spawned where it stood and struck by it
/// before `init_tank` gives it health, is no wreck once the first is gone:
/// no bot keeps clear of a live tank (bot-objectives.md 10.10.1).
#[test]
#[ignore = "runs mp_cassino UO through a tank's wreck and respawn, 30 s of game time; run with --ignored"]
fn a_wrecked_tanks_duplicate_is_no_wreck() {
    let Some((mut sv, mut now)) = uo_server(1, false) else {
        eprintln!("COD_DIR has no uo/: skipping");
        return;
    };
    run(&mut sv, &mut now, 100);
    let tank = sv.test_vehicles()[0];
    assert!(sv.test_damage_vehicle(tank.num, 10000));
    run(&mut sv, &mut now, 20);
    assert_eq!(sv.test_wrecks().len(), 1, "the tank burns");
    run(&mut sv, &mut now, 600);
    assert!(
        !sv.test_vehicles().iter().any(|v| v.num == tank.num),
        "the first tank is gone"
    );
    assert_eq!(sv.test_wrecks(), Vec::<[f32; 3]>::new());
    assert_eq!(sv.script_aborts(), Vec::<String>::new());
}

/// A lone bot set down 600 units behind a tank as it is wrecked hears the
/// hull's first blast and walks to it, and stays out of the second one
/// `tank_death` sets off ten seconds on (bot-objectives.md 10.10): no
/// damage, never inside the blast's reach while the hull burns.
#[test]
#[ignore = "builds mp_cassino's UO graph and runs a bot 16 s of game time, ~4 s; run with --ignored"]
fn a_bot_keeps_out_of_a_burning_hull() {
    // The second blast's 300 from the origin's 25 up, met by the box.
    const BLAST: f32 = 315.0;
    let Some((mut sv, mut now)) = uo_server(1, true) else {
        eprintln!("COD_DIR has no uo/: skipping");
        return;
    };
    run(&mut sv, &mut now, 100);
    let slot = sv.bot_slots()[0];
    let tank = sv.test_vehicles()[0];
    let (s, c) = tank.yaw.to_radians().sin_cos();
    let behind = [
        tank.origin[0] - c * 600.0,
        tank.origin[1] - s * 600.0,
        tank.origin[2] + 64.0,
    ];
    let p = sv
        .test_ground_under(behind)
        .expect("ground behind the tank");
    sv.place_client(slot, [p[0], p[1], p[2] + 0.125], tank.yaw);
    run(&mut sv, &mut now, 1);
    assert!(sv.test_damage_vehicle(tank.num, 10000));
    let health = sv.bot_body(slot).unwrap().health;
    let mut nearest = f32::MAX;
    let mut gone = None;
    let mut after_blast = None;
    for tick in 0..320 {
        run(&mut sv, &mut now, 1);
        let b = sv.bot_body(slot).unwrap();
        let burning = sv.test_vehicles().iter().any(|v| v.num == tank.num);
        if burning {
            nearest =
                nearest.min((b.origin[0] - tank.origin[0]).hypot(b.origin[1] - tank.origin[1]));
        } else if gone.is_none() {
            gone = Some(tick);
            after_blast = Some(b.health);
        }
        if std::env::var("VEH_REPORT").is_ok() && tick % 10 == 0 {
            eprintln!(
                "{tick} {:?} health {} burning {burning}",
                b.origin, b.health
            );
        }
    }
    let b = sv.bot_body(slot).unwrap();
    eprintln!(
        "hull at {:?} gone at {gone:?}, nearest {nearest:.0}, health {health} -> {after_blast:?}",
        tank.origin
    );
    assert!(gone.is_some(), "the hull never went off a second time");
    assert!(
        nearest > BLAST,
        "the bot came {nearest:.0} from the burning hull"
    );
    // The blast and the delete share a frame. Later, the duplicate's 0.4 u/s
    // that never rests may knock a bot that walks into it, as on retail
    // (coduo-vehicles.md 17.6).
    assert_eq!(after_blast, Some(health), "the bot took a blast");
    assert!(!b.dead);
    assert_eq!(sv.script_aborts(), Vec::<String>::new());
}

/// A hull burning when the round restarts is no wreck for the bots on the
/// next tick: the restart reissues the map's vehicles under the same
/// handles, alive.
#[test]
#[ignore = "builds mp_cassino's UO graph, ~5 s; run with --ignored"]
fn a_restart_clears_a_burning_hull_s_wreck() {
    let Some((mut sv, mut now)) = uo_server(1, false) else {
        eprintln!("COD_DIR has no uo/: skipping");
        return;
    };
    run(&mut sv, &mut now, 100);
    let tank = sv.test_vehicles()[0];
    assert!(sv.test_damage_vehicle(tank.num, 10000));
    run(&mut sv, &mut now, 5);
    assert!(
        !sv.test_wrecks().is_empty(),
        "the hull never read as a wreck"
    );
    sv.push_console("map_restart");
    run(&mut sv, &mut now, 2);
    assert!(sv.test_vehicles().iter().any(|v| v.num == tank.num));
    assert_eq!(sv.test_wrecks(), Vec::<[f32; 3]>::new());
}
