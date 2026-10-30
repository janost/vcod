//! Bots and United Offensive vehicles: per seed, one game with
//! `vc_bot_vehicles 0` and one with 1, each printing the kills, those off a
//! vehicle's guns, the boardings, the bot-ticks spent seated, how far
//! the driven vehicles went, the stuck kills and the drivers stalled 12 s.
//! The measurement behind
//! docs/research/bot-objectives.md section 10.
//! Run: COD_DIR=<UO install> cargo run --release -p vcod-server --example bot_vehicles [seed...]
//! `MAP=` picks the map (mp_cassino), `GAMETYPE=` the gametype (tdm),
//! `BOTS=` how many (6), `TICKS=` the length (3600, 3 min), `ON_ONLY=1`
//! skips the games with vehicles off, `VEH_LOG=1` prints the crush and
//! rocket lines and each stalled driver.

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
    sv.load_world(vcod_server::world::World::from_bsp(bsp, Some(fs)));
    sv.load_scripts(fs.clone()).expect("load the scripts");
    let mut t = Tally::default();
    let mut last: std::collections::HashMap<u32, [f32; 3]> = Default::default();
    let mut spells: std::collections::HashMap<usize, Vec<[f32; 3]>> = Default::default();
    for tick in 0..ticks {
        now += Duration::from_millis(50);
        sv.tick(now);
        let mut driven = Vec::new();
        for slot in sv.bot_slots() {
            let body = sv.bot_body(slot);
            if let Some((v, seat)) = body.as_ref().and_then(|b| b.seat) {
                t.seated += 1;
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
         boardings seated_ticks driven stuck_kills stuck_drivers"
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
                "{seed} {} {} {} {} {} {} {} {} {} {} {} {:.0} {} {}",
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
                t.stuck_drivers
            );
        }
    }
}
