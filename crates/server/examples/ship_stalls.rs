//! Six roaming bots on a map (`dm`, shoot off, 4000 ticks) per seed: counts
//! the bot-ticks stalled (every position over the 12 s before lies within
//! 150 units of the first) and the falls (unstick spells that start on the
//! ground off a ladder and end 48+ lower), and prints where each stall began.
//! The detector behind the tables in docs/research/bot-navigation.md
//! section 3.
//! Run: COD_DIR=<install> cargo run --release -p vcod-server --example ship_stalls [seed...]
//! `MAP=` picks the map (mp_ship), `TICKS=` the length, and
//! `TRACE=<slot>:<from>:<to>` prints that bot's every tick in the range.

use std::collections::VecDeque;
use std::rc::Rc;
use std::time::{Duration, Instant};

const WINDOW: usize = 240;
const RADIUS: f32 = 150.0;

fn main() {
    let Some(fs) = vcod_common::testing::game_fs() else {
        eprintln!("no game fs (set COD_DIR)");
        return;
    };
    let fs = Rc::new(fs);
    let map = std::env::var("MAP").unwrap_or_else(|_| "mp_ship".into());
    let ticks: usize = std::env::var("TICKS").map_or(4000, |t| t.parse().expect("TICKS"));
    let trace: Option<(usize, usize, usize)> = std::env::var("TRACE").ok().map(|t| {
        let v: Vec<usize> = t.split(':').map(|x| x.parse().expect("TRACE")).collect();
        (v[0], v[1], v[2])
    });
    let mut seeds: Vec<u64> = std::env::args()
        .skip(1)
        .map(|s| s.parse().unwrap())
        .collect();
    if seeds.is_empty() {
        seeds = (1..32).step_by(2).collect();
    }
    let bsp = vcod_common::bsp::parse(&fs.read(&fs.resolve_map(&map).unwrap()).unwrap()).unwrap();
    let (mut total, mut total_falls) = (0, 0);
    for seed in seeds {
        let mut now = Instant::now();
        let cfg = vcod_server::ServerConfig {
            map: map.clone(),
            hostname: "stalls".into(),
            max_clients: 8,
            gametype: "dm".into(),
            test_entities: 0,
            trace: false,
            bots: 6,
            bots_shoot: false,
        };
        let mut sv = vcod_server::Server::new(cfg, now);
        sv.test_seed_rng(seed);
        sv.load_world(vcod_server::world::World::from_bsp(&bsp, Some(&fs)));
        sv.load_scripts(fs.clone()).expect("load the scripts");
        let mut hist: [VecDeque<[f32; 3]>; 8] = Default::default();
        let mut stalled = [false; 8];
        let mut spell: [Option<[f32; 3]>; 8] = [None; 8];
        let mut was = [false; 8];
        let (mut count, mut falls) = (0, 0);
        for tick in 0..ticks {
            now += Duration::from_millis(50);
            sv.tick(now);
            for slot in sv.bot_slots() {
                let Some(b) = sv.bot_body(slot).filter(|b| b.playing) else {
                    hist[slot].clear();
                    stalled[slot] = false;
                    spell[slot] = None;
                    continue;
                };
                let (wp, next) = sv.bot_waypoints(slot);
                if trace.is_some_and(|(s, a, z)| s == slot && (a..=z).contains(&tick)) {
                    println!(
                        "  t{tick} {:?} gnd {} lad {} unst {} wp {:?} next {:?}",
                        b.origin.map(|x| (x * 10.0).round() / 10.0),
                        b.on_ground as u8,
                        b.on_ladder as u8,
                        b.unsticking as u8,
                        wp.map(|p| p.map(|x| x.round())),
                        next.map(|p| p.map(|x| x.round())),
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
                if near {
                    count += 1;
                    if !stalled[slot] {
                        println!(
                            "  seed {seed} t{tick} bot {slot} stalled at {:?} (12 s at {:?}) lad {} wp {:?} next {:?}",
                            b.origin.map(|x| x.round()),
                            first.map(|x| x.round()),
                            b.on_ladder as u8,
                            wp.map(|p| p.map(|x| x.round())),
                            next.map(|p| p.map(|x| x.round())),
                        );
                    }
                }
                stalled[slot] = near;
            }
        }
        println!("seed {seed}: stalled {count}, falls {falls}");
        total += count;
        total_falls += falls;
    }
    println!("total stalled {total}, falls {total_falls}");
}
