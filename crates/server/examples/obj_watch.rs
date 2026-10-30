//! Bots on a United Offensive objective gametype, watched: every `EVERY`
//! ticks each bot's body and waypoint and the gametype's objective state,
//! and every `logPrint` objective line (`A;`) as it lands.
//! Run: COD_DIR=<uo install> cargo run --release -p vcod-server --example obj_watch
//! `MAP=` (mp_cassino), `GAMETYPE=` (ctf), `BOTS=` (2), `TICKS=` (3000),
//! `SHOOT=1`, `SEED=` (7), `EVERY=` (40), `SET=name=value,...` cvars.

use std::rc::Rc;
use std::time::{Duration, Instant};

fn main() {
    env_logger::init();
    let Some(fs) = vcod_common::testing::uo_fs() else {
        eprintln!("COD_DIR has no uo/");
        return;
    };
    let env = |k: &str, d: &str| std::env::var(k).unwrap_or_else(|_| d.into());
    let map = env("MAP", "mp_cassino");
    let gametype = env("GAMETYPE", "ctf");
    let bots: usize = env("BOTS", "2").parse().unwrap();
    let ticks: usize = env("TICKS", "3000").parse().unwrap();
    let every: usize = env("EVERY", "40").parse().unwrap();
    let seed: u64 = env("SEED", "7").parse().unwrap();
    let bsp = vcod_common::mapworld::find(&fs, &map).unwrap();
    let mut now = Instant::now();
    let cfg = vcod_server::ServerConfig {
        map: map.clone(),
        hostname: "watch".into(),
        max_clients: 8,
        gametype,
        test_entities: 0,
        trace: false,
        bots,
        bots_shoot: env("SHOOT", "0") == "1",
    };
    let mut sv = vcod_server::Server::new(cfg, now);
    sv.set_protocol(&vcod_common::net::protocol::PROTOCOL_V22);
    sv.test_seed_rng(seed);
    for kv in env("SET", "").split(',').filter(|s| !s.is_empty()) {
        let (k, v) = kv.split_once('=').expect("SET name=value");
        sv.set_cvar(k, v);
    }
    sv.load_world(vcod_server::world::World::from_bsp(&bsp, Some(&fs)));
    sv.load_scripts(Rc::new(fs)).expect("load the scripts");
    // `PROBE=x,y,z;...`: the floor under each point and the route to it
    // from the first bot, once the graph is up.
    let probes: Vec<[f32; 3]> = env("PROBE", "")
        .split(';')
        .filter(|s| !s.is_empty())
        .map(|p| {
            let v: Vec<f32> = p.split(',').map(|x| x.parse().unwrap()).collect();
            [v[0], v[1], v[2]]
        })
        .collect();
    let mut seen = 0;
    let r = |p: [f32; 3]| p.map(|x| x.round() as i32);
    for tick in 0..ticks {
        now += Duration::from_millis(50);
        sv.tick(now);
        let log = sv.script_log();
        if log.len() < seen {
            seen = 0;
        }
        for l in &log[seen..] {
            if l.contains("A;") || l.contains("K;") {
                println!("t{tick} log {}", l.trim_end());
            }
        }
        seen = log.len();
        if tick == 100 {
            let from = sv
                .bot_slots()
                .first()
                .and_then(|s| sv.bot_body(*s))
                .map(|b| b.origin);
            for p in &probes {
                let floor = sv.test_ground_under(*p);
                let route = from.zip(floor).and_then(|(f, g)| sv.test_nav_route(f, g));
                println!("probe {p:?}: floor {floor:?} route from {from:?} {route:?}");
            }
        }
        if tick == 1200
            && let Some(vcod_server::game::script::UoObjectives::Bas(bases)) = sv.bot_uo()
        {
            for slot in sv.bot_slots() {
                let Some(b) = sv.bot_body(slot).filter(|b| b.playing) else {
                    continue;
                };
                println!("self route {:?}", sv.test_nav_route(b.origin, b.origin));
                for x in &bases {
                    if let Some((_, m)) = x.charge {
                        println!(
                            "route bot {slot} {:?} -> charge {:?}: {:?}",
                            r(b.origin),
                            r(m),
                            sv.test_nav_route(b.origin, m)
                        );
                    }
                }
            }
        }
        if let Ok(m) = env("MODEL", "").parse::<usize>()
            && tick % every == 0
        {
            println!("t{tick} model {m} linked {}", sv.test_model_linked(m));
        }
        if tick % every != 0 {
            continue;
        }
        for slot in sv.bot_slots() {
            let team = sv.bot_team(slot);
            let Some(b) = sv.bot_body(slot) else {
                continue;
            };
            let (wp, _) = sv.bot_waypoints(slot);
            let o = b.origin;
            println!(
                "t{tick} bot {slot} team {team} at {:?} play {} unst {} wp {:?} on {:?}",
                r(b.origin),
                b.playing as u8,
                b.unsticking as u8,
                wp.map(r),
                sv.test_floor_under([o[0], o[1], o[2] + 1.0]),
            );
        }
        use vcod_server::game::script::UoObjectives as U;
        match sv.bot_uo() {
            Some(U::Ctf(flags)) => {
                for f in flags {
                    println!(
                        "t{tick}  flag team {} home {:?} dropped {:?} goal {:?} carrier {:?}",
                        f.team,
                        f.home.map(|b| (r(b.0), r(b.1))),
                        f.dropped.map(|b| (r(b.0), r(b.1))),
                        (r(f.goal.0), r(f.goal.1)),
                        f.carrier
                    );
                }
            }
            Some(U::Dom(flags)) => {
                let t: Vec<i32> = flags.iter().map(|f| f.team).collect();
                println!("t{tick}  dom {t:?}");
            }
            Some(U::Hq { radios, next }) => {
                for x in radios {
                    println!(
                        "t{tick}  radio {:?} team {} radius {}",
                        r(x.at),
                        x.team,
                        x.radius
                    );
                }
                println!("t{tick}  next {:?}", next.map(r));
            }
            Some(U::Bas(bases)) => {
                for x in bases {
                    println!(
                        "t{tick}  base team {} at {:?} breached {} planted {} charge {:?}",
                        x.team,
                        r(x.at),
                        x.breached as u8,
                        x.planted as u8,
                        x.charge.map(|(b, m)| (r(b.0), r(b.1), r(m)))
                    );
                }
            }
            None => println!("t{tick}  no objectives"),
        }
    }
    println!("aborts {:?}", sv.script_aborts());
}
