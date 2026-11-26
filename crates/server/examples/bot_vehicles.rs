//! Bots and United Offensive vehicles: per seed, one game with
//! `vc_bot_vehicles 0` and one with 1, each printing the kills, those off a
//! vehicle's guns, the boardings, the bot-ticks spent seated, how far
//! the driven vehicles went, the stuck kills, the drivers stalled 12 s and
//! the bot-ticks a tank's driver held its fire for want of a shell's line.
//! The measurement behind
//! docs/research/bot-objectives.md section 10.
//! Run: COD_DIR=<UO install> cargo run --release -p vcod-server --example bot_vehicles [seed...]
//! `MAP=` picks the map (mp_cassino), `GAMETYPE=` the gametype (tdm),
//! `BOTS=` how many (6), `TICKS=` the length (3600, 3 min), `SKILL=` every
//! bot's `vc_bot_skill` (1), `ON_ONLY=1` skips the games with vehicles off,
//! `VEH_LOG=1` prints the crush and rocket lines and each stalled driver.
//! The vehicle kills split into crushes, tank shells and vehicle MGs, and
//! `foot_kills` counts those whose victim was on foot.

use std::rc::Rc;
use std::time::{Duration, Instant};

#[derive(Default)]
struct Tally {
    kills: usize,
    gun_kills: usize,
    /// `D;` lines off a vehicle's guns.
    gun_hits: usize,
    boardings: u32,
    seated: usize,
    driven: f32,
    /// `D;` lines of a vehicle's crush on the driver's own team, their
    /// damage, and the lines on the other team.
    crush_team: usize,
    crush_team_dmg: i32,
    crush_enemy: usize,
    /// Rockets fired on foot at enemy vehicles, and vehicles gone.
    rockets: u32,
    wrecks: usize,
    /// Vehicles the stuck kill destroyed, and the 12 s spells a driver's
    /// hull stayed within 150 units of one spot with no enemy in range.
    stuck_kills: u32,
    stuck_drivers: usize,
    /// Bot-ticks a tank's driver held its fire on an enemy hull for want
    /// of a shell's line on it.
    shell_held: u32,
    /// `K;` lines of a crush, a tank's main gun and a vehicle's MG, and of
    /// those three the victims on foot.
    crush_kills: usize,
    shell_kills: usize,
    mg_kills: usize,
    foot_kills: usize,
}

/// A vehicle's kill by its `K;` weapon and means: a crush, a tank's main
/// gun (`<tank>_turret_mp`, not a mounted MG's) or a vehicle's MG
/// (`<mg>_tank_mp`).
#[derive(Clone, Copy, PartialEq)]
enum VehicleKill {
    Crush,
    Shell,
    Mg,
}

fn vehicle_kill(weapon: &str, means: &str) -> Option<VehicleKill> {
    const SHELLS: [&str; 5] = ["elefant", "panzeriv", "sherman", "su152", "t34"];
    let w = weapon.to_ascii_lowercase();
    if means.contains("CRUSH") {
        Some(VehicleKill::Crush)
    } else if w.ends_with("_turret_mp") && SHELLS.iter().any(|s| w.starts_with(s)) {
        Some(VehicleKill::Shell)
    } else if w.ends_with("_tank_mp") {
        Some(VehicleKill::Mg)
    } else {
        None
    }
}

fn game(
    fs: &Rc<vcod_common::pk3::Pk3Fs>,
    bsp: &vcod_common::mapworld::MapWorld,
    seed: u64,
    on: bool,
) -> Tally {
    let map = std::env::var("MAP").unwrap_or_else(|_| "mp_cassino".into());
    let gametype = std::env::var("GAMETYPE").unwrap_or_else(|_| "tdm".into());
    let bots: usize = std::env::var("BOTS").map_or(6, |t| t.parse().expect("BOTS"));
    let ticks: usize = std::env::var("TICKS").map_or(3600, |t| t.parse().expect("TICKS"));
    let mut now = Instant::now();
    let cfg = vcod_server::ServerConfig {
        map,
        hostname: "vehicles".into(),
        max_clients: 8,
        gametype,
        test_entities: 0,
        trace: false,
        bots,
        bots_shoot: true,
    };
    let mut sv = vcod_server::Server::new(cfg, now);
    sv.set_protocol(&vcod_common::net::protocol::PROTOCOL_V22);
    sv.test_seed_rng(seed);
    sv.set_cvar("vc_bot_vehicles", if on { "1" } else { "0" });
    if let Ok(level) = std::env::var("SKILL") {
        sv.set_cvar("vc_bot_skill", &level);
    }
    sv.load_world(vcod_server::world::World::from_bsp(bsp, Some(&**fs)));
    sv.load_scripts(fs.clone()).expect("load the scripts");
    let mut t = Tally::default();
    let mut last: std::collections::HashMap<u32, [f32; 3]> = Default::default();
    let mut spells: std::collections::HashMap<usize, Vec<[f32; 3]>> = Default::default();
    // The slots seated as the last tick ended, and the log lines read.
    let mut seated = std::collections::HashSet::new();
    let mut read = 0;
    for tick in 0..ticks {
        now += Duration::from_millis(50);
        sv.tick(now);
        let log = sv.script_log();
        for l in &log[read.min(log.len())..] {
            let Some((_, k)) = l
                .trim_end()
                .split_once("K;")
                .filter(|(pre, _)| !pre.ends_with(|c: char| c.is_ascii_alphanumeric()))
            else {
                continue;
            };
            let k: Vec<&str> = k.split(';').collect();
            let (Some(w), Some(m)) = (k.get(8), k.get(10)) else {
                continue;
            };
            let Some(kind) = vehicle_kill(w, m) else {
                continue;
            };
            match kind {
                VehicleKill::Crush => t.crush_kills += 1,
                VehicleKill::Shell => t.shell_kills += 1,
                VehicleKill::Mg => t.mg_kills += 1,
            }
            if k.get(1)
                .and_then(|s| s.parse::<usize>().ok())
                .is_some_and(|v| !seated.contains(&v))
            {
                t.foot_kills += 1;
            }
        }
        read = log.len();
        seated.clear();
        let mut driven = Vec::new();
        for slot in sv.bot_slots() {
            let body = sv.bot_body(slot);
            if let Some((v, seat)) = body.as_ref().and_then(|b| b.seat) {
                t.seated += 1;
                seated.insert(slot);
                if seat == 1 {
                    driven.push(v);
                }
            }
            // `uo_bots_fight_and_sprint_in_tdm`'s stall, for drivers.
            let h = spells.entry(slot).or_default();
            match body.filter(|b| b.seat.is_some_and(|s| s.1 == 1)) {
                Some(b) if !b.engaged && !b.crew_wait => {
                    h.push(b.origin);
                    if h.len() == 240 {
                        let first = h[0];
                        if h.iter().all(|p| {
                            (p[0] - first[0])
                                .hypot(p[1] - first[1])
                                .hypot(p[2] - first[2])
                                < 150.0
                        }) {
                            t.stuck_drivers += 1;
                            if std::env::var("VEH_LOG").is_ok() {
                                eprintln!("stuck driver: tick {tick} slot {slot} at {first:?}");
                            }
                            h.clear();
                        } else {
                            h.remove(0);
                        }
                    }
                }
                Some(_) => {}
                None => h.clear(),
            }
        }
        let now_live = sv.test_vehicles();
        t.wrecks += last
            .keys()
            .filter(|n| !now_live.iter().any(|v| v.num == **n))
            .count();
        last.retain(|n, _| now_live.iter().any(|v| v.num == *n));
        for v in now_live {
            if let Some(p) = last.insert(v.num, v.origin)
                && driven.contains(&v.num)
            {
                t.driven += (v.origin[0] - p[0]).hypot(v.origin[1] - p[1]);
            }
        }
    }
    t.stuck_kills = sv.test_stuck_kills();
    for slot in sv.bot_slots() {
        t.boardings += sv.bot_body(slot).map_or(0, |b| b.boardings);
        t.rockets += sv.bot_body(slot).map_or(0, |b| b.rockets_at_armor);
        t.shell_held += sv.bot_body(slot).map_or(0, |b| b.shell_held);
    }
    for l in sv.script_log() {
        let d: Vec<&str> = l.trim_end().split(';').collect();
        let gun = d
            .get(9)
            .is_some_and(|w| w.contains("turret") || w.contains("gunner"));
        if d.first() == Some(&"D") && gun && d.get(11).is_some_and(|m| !m.contains("CRUSH")) {
            t.gun_hits += 1;
        }
        if d.first() == Some(&"D") && d.get(11).is_some_and(|m| m.contains("CRUSH")) {
            if d[3] == d[7] {
                t.crush_team += 1;
                t.crush_team_dmg += d[10].parse::<i32>().unwrap_or(0);
            } else {
                t.crush_enemy += 1;
            }
        }
    }
    for k in sv.script_log().iter().filter_map(|l| {
        // Not the `TANK;` of a `D;` line's `MOD_CRUSH_TANK`.
        l.trim_end()
            .split_once("K;")
            .filter(|(pre, _)| !pre.ends_with(|c: char| c.is_ascii_alphanumeric()))
            .map(|(_, k)| k)
    }) {
        let k: Vec<&str> = k.split(';').collect();
        t.kills += 1;
        let w = k.get(8).copied().unwrap_or("");
        if w.contains("turret") || w.contains("gunner") {
            t.gun_kills += 1;
        }
    }
    if std::env::var("VEH_LOG").is_ok() {
        for l in sv.script_log().iter().filter(|l| {
            l.contains("CRUSH")
                || l.contains("bazooka")
                || l.contains("panzerschreck")
                || l.contains("panzerfaust")
        }) {
            eprintln!("{}", l.trim_end());
        }
    }
    t
}

fn main() {
    let Some(fs) = vcod_common::testing::uo_fs() else {
        eprintln!("no UO install (set COD_DIR)");
        return;
    };
    let fs = Rc::new(fs);
    let map = std::env::var("MAP").unwrap_or_else(|_| "mp_cassino".into());
    let bsp = vcod_common::mapworld::find(&*fs, &map).unwrap();
    let mut seeds: Vec<u64> = std::env::args()
        .skip(1)
        .map(|s| s.parse().unwrap())
        .collect();
    if seeds.is_empty() {
        seeds = vec![1, 3, 5, 7];
    }
    println!(
        "seed vehicles kills gun_kills gun_hits crush_team crush_team_dmg crush_enemy rockets wrecks \
         boardings seated_ticks driven stuck_kills stuck_drivers shell_held crush_kills shell_kills \
         mg_kills foot_kills"
    );
    for seed in seeds {
        let modes: &[bool] = if std::env::var("ON_ONLY").is_ok() {
            &[true]
        } else {
            &[false, true]
        };
        for &on in modes {
            let t = game(&fs, &bsp, seed, on);
            println!(
                "{seed} {} {} {} {} {} {} {} {} {} {} {} {:.0} {} {} {} {} {} {} {}",
                u8::from(on),
                t.kills,
                t.gun_kills,
                t.gun_hits,
                t.crush_team,
                t.crush_team_dmg,
                t.crush_enemy,
                t.rockets,
                t.wrecks,
                t.boardings,
                t.seated,
                t.driven,
                t.stuck_kills,
                t.stuck_drivers,
                t.shell_held,
                t.crush_kills,
                t.shell_kills,
                t.mg_kills,
                t.foot_kills
            );
        }
    }
}
