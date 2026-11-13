//! Six roaming bots on a map (`dm`, shoot off, 4000 ticks) per seed: counts
//! the bot-ticks stalled (every position over the 12 s before lies within
//! 150 units of the first) and the falls (unstick spells that start on the
//! ground off a ladder and end 48+ lower) and the deaths, and prints where
//! each stall began and each bot died. A bot with a UO LMG's bipod down
//! holds still on purpose and is not counted. `held` counts the stalled
//! bot-ticks whose 12 s hold a tick `BotBody::holds` names (with
//! `SHOOT=1` mostly fights and cover), as `bot_match` splits its count;
//! each stall line's `ago` is the ticks since the bot last held.
//! `seated` counts the stalled bot-ticks in a UO vehicle's seat, which
//! the driver's route owns, not the walk's follower.
//! The detector behind the tables in docs/research/bot-navigation.md
//! section 3.
//! Run: COD_DIR=<install> cargo run --release -p vcod-server --example ship_stalls [seed...]
//! `MAP=` picks the map (mp_ship), `GAMETYPE=` the gametype (dm), `BOTS=`
//! how many (6), `TICKS=` the length, and `TRACE=<slot,...>:<from>:<to>` prints
//! those bots' every tick in the range. `UO=1` runs a United Offensive
//! server (`uo/` over `main/`, protocol 22) and counts the sprinting
//! bot-ticks too. `SHOOT=1` turns the bots' fire on and adds the kills,
//! the frag kills and the frags that killed their own thrower. Each seed
//! ends with the edges the followers gave up on (40 ticks without
//! closing, or the bot's own unstick) and those with another body within
//! 64 units (`Server::bot_edge_stalls`), and the bot-ticks held up by a
//! body: moving under 60 units/s with another bot within 64 toward the
//! waypoint, not holding on purpose (`held_by_a_body`); `HELD=<ticks>`
//! prints each such spell at least that long.

use std::collections::VecDeque;
use std::rc::Rc;
use std::time::{Duration, Instant};

const WINDOW: usize = 240;
const RADIUS: f32 = 150.0;

fn main() {
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
    let fs = Rc::new(fs);
    let map = std::env::var("MAP").unwrap_or_else(|_| "mp_ship".into());
    let ticks: usize = std::env::var("TICKS").map_or(4000, |t| t.parse().expect("TICKS"));
    let gametype = std::env::var("GAMETYPE").unwrap_or_else(|_| "dm".into());
    let bots: usize = std::env::var("BOTS").map_or(6, |t| t.parse().expect("BOTS"));
    let shoot = std::env::var("SHOOT").is_ok_and(|v| v == "1");
    let held_spell: Option<usize> = std::env::var("HELD").ok().map(|t| t.parse().expect("HELD"));
    // The server's 8 client slots, more when more bots are asked for.
    let slots = bots.max(8);
    let trace: Option<(Vec<usize>, usize, usize)> = std::env::var("TRACE").ok().map(|t| {
        let v: Vec<&str> = t.split(':').collect();
        let slots = v[0].split(',').map(|x| x.parse().expect("TRACE")).collect();
        (
            slots,
            v[1].parse().expect("TRACE"),
            v[2].parse().expect("TRACE"),
        )
    });
    let mut seeds: Vec<u64> = std::env::args()
        .skip(1)
        .map(|s| s.parse().unwrap())
        .collect();
    if seeds.is_empty() {
        seeds = (1..32).step_by(2).collect();
    }
    let bsp = vcod_common::mapworld::find(&*fs, &map).unwrap();
    let (mut total, mut total_falls, mut total_edges, mut total_body) = (0, 0, [0; 2], 0);
    for seed in seeds {
        let mut now = Instant::now();
        let cfg = vcod_server::ServerConfig {
            map: map.clone(),
            hostname: "stalls".into(),
            max_clients: slots,
            gametype: gametype.clone(),
            test_entities: 0,
            trace: false,
            bots,
            bots_shoot: shoot,
        };
        let mut sv = vcod_server::Server::new(cfg, now);
        if uo {
            sv.set_protocol(&vcod_common::net::protocol::PROTOCOL_V22);
        }
        sv.test_seed_rng(seed);
        for kv in std::env::var("SET")
            .unwrap_or_default()
            .split(',')
            .filter(|s| !s.is_empty())
        {
            let (k, v) = kv.split_once('=').expect("SET name=value");
            sv.set_cvar(k, v);
        }
        sv.load_world(vcod_server::world::World::from_bsp(&bsp, Some(&fs)));
        sv.load_scripts(fs.clone()).expect("load the scripts");
        let mut hist: Vec<VecDeque<[f32; 3]>> = vec![VecDeque::new(); slots];
        let mut stalled = vec![false; slots];
        let mut was_held = vec![false; slots];
        let mut since_held = vec![usize::MAX; slots];
        let mut spell: Vec<Option<[f32; 3]>> = vec![None; slots];
        let mut was = vec![false; slots];
        let (mut count, mut held, mut falls, mut deaths, mut sprints) = (0, 0, 0, 0, 0);
        let (mut seated, mut body_held) = (0, 0);
        let mut held_since: Vec<Option<(usize, [f32; 3], bool)>> = vec![None; slots];
        let mut died = vec![false; slots];
        for tick in 0..ticks {
            now += Duration::from_millis(50);
            sv.tick(now);
            let bodies: Vec<(usize, [f32; 3])> = sv
                .bot_slots()
                .into_iter()
                .filter_map(|s| sv.bot_body(s).filter(|b| b.playing).map(|b| (s, b.origin)))
                .collect();
            for slot in sv.bot_slots() {
                // Shoot is off: a death is a `kill`, a minefield or a fall.
                let dead = sv.bot_body(slot).filter(|b| b.dead);
                if !died[slot]
                    && let Some(b) = &dead
                {
                    deaths += 1;
                    println!(
                        "  seed {seed} t{tick} bot {slot} died at {:?}",
                        b.origin.map(|x| x.round())
                    );
                }
                died[slot] = dead.is_some();
                let Some(b) = sv.bot_body(slot).filter(|b| b.playing && !b.deployed) else {
                    hist[slot].clear();
                    stalled[slot] = false;
                    spell[slot] = None;
                    continue;
                };
                sprints += usize::from(b.sprinting);
                let (wp, next) = sv.bot_waypoints(slot);
                if trace
                    .as_ref()
                    .is_some_and(|(s, a, z)| s.contains(&slot) && (*a..=*z).contains(&tick))
                {
                    let near = bodies
                        .iter()
                        .filter(|(s, _)| *s != slot)
                        .map(|(s, o)| {
                            (
                                *s,
                                *o,
                                (0..3)
                                    .map(|i| (o[i] - b.origin[i]).powi(2))
                                    .sum::<f32>()
                                    .sqrt(),
                            )
                        })
                        .min_by(|a, b| a.2.total_cmp(&b.2));
                    println!(
                        "  t{tick} b{slot} {:?} gnd {} lad {} unst {} spr {} yld {} wp {:?} next {:?} near {:?}",
                        b.origin.map(|x| (x * 10.0).round() / 10.0),
                        b.on_ground as u8,
                        b.on_ladder as u8,
                        b.unsticking as u8,
                        b.sprinting as u8,
                        sv.bot_yielding(slot),
                        wp.map(|p| p.map(|x| x.round())),
                        next.map(|p| p.map(|x| x.round())),
                        near.filter(|n| n.2 < 100.0)
                            .map(|(s, o, _)| (s, o.map(|x| x.round()))),
                    );
                }
                // Falls.
                if b.unsticking && !was[slot] {
                    spell[slot] = (b.on_ground && !b.on_ladder).then_some(b.origin);
                }
                if b.unsticking && b.on_ladder {
                    spell[slot] = None;
                }
                if !b.unsticking
                    && was[slot]
                    && let Some(from) = spell[slot].take()
                    && b.origin[2] < from[2] - 48.0
                {
                    falls += 1;
                    println!(
                        "  seed {seed} t{tick} bot {slot} fell {:?} -> {:?}",
                        from.map(|x| x.round()),
                        b.origin.map(|x| x.round())
                    );
                }
                was[slot] = b.unsticking;
                let by_body = !b.holds()
                    && hist[slot].back().zip(wp).is_some_and(|(last, wp)| {
                        held_by_a_body(b.origin, *last, wp, slot, &bodies)
                    });
                body_held += usize::from(by_body);
                // `HELD=<ticks>` prints each spell held up by a body that
                // long, where it began.
                match (by_body, held_since[slot]) {
                    (true, None) => held_since[slot] = Some((tick, b.origin, b.on_ladder)),
                    (false, Some((t0, at, lad))) => {
                        held_since[slot] = None;
                        if held_spell.is_some_and(|n| tick - t0 >= n) {
                            println!(
                                "  seed {seed} t{t0} bot {slot} held by a body {} ticks at {:?} lad {}",
                                tick - t0,
                                at.map(|x| x.round()),
                                lad as u8
                            );
                        }
                    }
                    _ => {}
                }
                // Stalls.
                let h = &mut hist[slot];
                h.push_back(b.origin);
                if h.len() > WINDOW {
                    h.pop_front();
                }
                let first = h[0];
                let near = h.len() == WINDOW
                    && h.iter().all(|p| {
                        (0..3).map(|i| (p[i] - first[i]).powi(2)).sum::<f32>() < RADIUS * RADIUS
                    });
                since_held[slot] = if b.holds() {
                    0
                } else {
                    since_held[slot].saturating_add(1)
                };
                if near {
                    count += 1;
                    seated += usize::from(b.seat.is_some());
                    held += usize::from(since_held[slot] < WINDOW);
                    // A held stall that lets go is printed again.
                    if !stalled[slot] || (was_held[slot] && !b.holds()) {
                        // The nearest other bot: two on one ladder pin each other.
                        let other = bodies
                            .iter()
                            .filter(|(s, _)| *s != slot)
                            .map(|(_, o)| {
                                (0..3)
                                    .map(|i| (o[i] - b.origin[i]).powi(2))
                                    .sum::<f32>()
                                    .sqrt()
                            })
                            .fold(f32::INFINITY, f32::min);
                        println!(
                            "  seed {seed} t{tick} bot {slot} stalled at {:?} (12 s at {:?}) lad {} seat {} held {} ago {} wp {:?} next {:?} bot {:.0} away",
                            b.origin.map(|x| x.round()),
                            first.map(|x| x.round()),
                            b.on_ladder as u8,
                            b.seat.map_or(0, |s| s.1),
                            b.holds() as u8,
                            since_held[slot].min(9999),
                            wp.map(|p| p.map(|x| x.round())),
                            next.map(|p| p.map(|x| x.round())),
                            other,
                        );
                    }
                }
                stalled[slot] = near;
                was_held[slot] = b.holds();
            }
        }
        let edges = sv.bot_edge_stalls();
        println!(
            "seed {seed}: stalled {count}, falls {falls}, deaths {deaths}, sprinting {sprints}, held {held}, seated {seated}, edges given up {} (by a body {}), held by a body {body_held}",
            edges[0], edges[1]
        );
        if shoot {
            // victim guid, num, team, name, attacker guid, num, team, name,
            // weapon, damage, means of death, hit location; `dm` logs no
            // teams, so its lines are two fields short.
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
                .map(|k| k.split(';').map(str::to_string).collect::<Vec<_>>())
                .filter(|k| k.len() >= 10)
                .collect();
            let frag = |k: &Vec<String>| k[k.len() - 2].starts_with("MOD_GRENADE");
            // The attacker's number: after the victim's guid, number,
            // team (`tdm`) and name and the attacker's guid.
            let own: Vec<&Vec<String>> = kills
                .iter()
                .filter(|k| frag(k) && k[1] == k[k.len() / 2 - 1])
                .collect();
            for k in &own {
                println!("  seed {seed} own frag: {}", k.join(";"));
            }
            println!(
                "seed {seed}: kills {}, frag kills {}, own frags {}",
                kills.len(),
                kills.iter().filter(|k| frag(k)).count(),
                own.len()
            );
        }
        total += count;
        total_falls += falls;
        total_edges[0] += edges[0];
        total_edges[1] += edges[1];
        total_body += body_held;
    }
    println!(
        "total stalled {total}, falls {total_falls}, edges given up {} (by a body {}), held by a body {total_body}",
        total_edges[0], total_edges[1]
    );
}

/// Whether a bot at `at`, last tick at `last`, is held up by another body
/// on its way to `wp`: it moved under 3 units across (60 units/s) and
/// another body stands within 64 across and 64 up or down toward the
/// waypoint (within 60 degrees of it).
fn held_by_a_body(
    at: [f32; 3],
    last: [f32; 3],
    wp: [f32; 3],
    slot: usize,
    bodies: &[(usize, [f32; 3])],
) -> bool {
    let flat = |a: [f32; 3], b: [f32; 3]| [b[0] - a[0], b[1] - a[1]];
    let len = |d: [f32; 2]| d[0].hypot(d[1]);
    let way = flat(at, wp);
    len(flat(last, at)) < 3.0
        && len(way) > 1.0
        && bodies.iter().any(|&(s, o)| {
            let d = flat(at, o);
            s != slot
                && len(d) < 64.0
                && (o[2] - at[2]).abs() < 64.0
                && d[0] * way[0] + d[1] * way[1] > 0.5 * len(d) * len(way)
        })
}
