//! The bots' pass on worker threads moves every bot as the ticking thread
//! alone does (docs/dev/server-tick.md, "The bots' threads"): one seeded
//! match run serial and on four threads gives the same cmds and bodies
//! every tick, on CoD1 (1.1, or UO at a UO install), CoD2 and CoD4. Each
//! needs its install and returns early without it.

use std::rc::Rc;
use std::time::{Duration, Instant};

use vcod_server::bots::with_threads;

const FRAME: Duration = Duration::from_millis(50);

/// FNV-1a 64 over a tick's bots, floats by their bits.
struct Fnv(u64);

impl Fnv {
    fn new() -> Fnv {
        Fnv(0xcbf2_9ce4_8422_2325)
    }

    fn bytes(&mut self, b: &[u8]) {
        for &x in b {
            self.0 = (self.0 ^ u64::from(x)).wrapping_mul(0x0000_0100_0000_01b3);
        }
    }

    fn i32s(&mut self, v: &[i32]) {
        v.iter().for_each(|x| self.bytes(&x.to_le_bytes()));
    }

    fn f32s(&mut self, v: &[f32]) {
        v.iter()
            .for_each(|x| self.bytes(&x.to_bits().to_le_bytes()));
    }
}

/// `run`'s per-tick hashes serial and on four threads, the first tick
/// they part at named.
fn same_on_threads(game: &str, run: impl Fn() -> Vec<u64>) {
    let serial = with_threads(1, &run);
    let parallel = with_threads(4, &run);
    assert!(serial.len() > 100, "{game}: {} ticks", serial.len());
    let parted = serial.iter().zip(&parallel).position(|(a, b)| a != b);
    assert_eq!(parted, None, "{game}: the bots part at tick {parted:?}");
    assert_eq!(serial.len(), parallel.len());
}

/// Each bot's last cmd and body on a CoD1 server, by slot.
fn cod1_tick(sv: &vcod_server::Server) -> u64 {
    let mut h = Fnv::new();
    for slot in sv.bot_slots() {
        h.bytes(&(slot as u32).to_le_bytes());
        let Some((c, ps, health)) = sv.bot_digest_state(slot) else {
            h.bytes(&[0]);
            continue;
        };
        h.i32s(&[c.server_time, health]);
        h.i32s(&c.angles);
        h.bytes(&[
            c.buttons,
            c.wbuttons,
            c.weapon,
            c.forward as u8,
            c.right as u8,
        ]);
        h.f32s(&ps.origin.to_array());
        h.f32s(&ps.velocity.to_array());
    }
    h.0
}

/// `bots` shooting bots on `map` `tdm` for `ticks`, on 1.1's or (with
/// `uo`) UO's protocol.
fn cod1_match(
    fs: vcod_common::pk3::Pk3Fs,
    map: &str,
    uo: bool,
    bots: usize,
    ticks: usize,
) -> Vec<u64> {
    let bsp = vcod_common::bsp::parse(&fs.read(&fs.resolve_map(map).unwrap()).unwrap())
        .expect("parse the bsp");
    let mut now = Instant::now();
    let cfg = vcod_server::ServerConfig {
        map: map.into(),
        hostname: "vcod test".into(),
        max_clients: bots + 2,
        gametype: "tdm".into(),
        test_entities: 0,
        trace: false,
        bots,
        bots_shoot: true,
    };
    let mut sv = vcod_server::Server::new(cfg, now);
    if uo {
        sv.set_protocol(&vcod_common::net::protocol::PROTOCOL_V22);
    }
    sv.set_cvar("sv_pure", "0");
    sv.load_world(vcod_server::world::World::from_bsp(&bsp, Some(&fs)));
    sv.load_scripts(Rc::new(fs)).expect("load the scripts");
    (0..ticks)
        .map(|_| {
            now += FRAME;
            sv.tick(now);
            cod1_tick(&sv)
        })
        .collect()
}

/// 1.1: eight bots on mp_carentan for 60 s. Needs `COD_DIR` at 1.1.
#[test]
fn cod1_bots_move_alike_on_threads() {
    if vcod_common::testing::game_fs_11().is_none() {
        return;
    }
    same_on_threads("1.1", || {
        let fs = vcod_common::testing::game_fs_11().unwrap();
        cod1_match(fs, "mp_carentan", false, 8, 1200)
    });
}

/// UO: eight bots on mp_cassino for 60 s, vehicles and all. Needs
/// `COD_DIR` at a UO install.
#[test]
fn uo_bots_move_alike_on_threads() {
    if vcod_common::testing::uo_fs().is_none() {
        return;
    }
    same_on_threads("UO", || {
        let fs = vcod_common::testing::uo_fs().unwrap();
        cod1_match(fs, "mp_cassino", true, 8, 1200)
    });
}

/// CoD2: ten bots on mp_toujane `tdm` for 60 s. Needs `COD2_DIR`.
#[test]
fn cod2_bots_move_alike_on_threads() {
    use vcod_server::cod2::{Cod2Config, Cod2Server};
    if vcod_common::testing::cod2_fs().is_none() {
        return;
    }
    same_on_threads("CoD2", || {
        let fs = Rc::new(vcod_common::testing::cod2_fs().unwrap());
        let mut now = Instant::now();
        let cfg = Cod2Config {
            map: "mp_toujane".into(),
            gametype: "tdm".into(),
            hostname: "vcod test".into(),
            max_clients: 12,
            gametype_script: None,
        };
        let mut sv = Cod2Server::new(cfg, 3, now);
        sv.load(Some(fs)).expect("load the level");
        let mut slots = sv.add_bots(10, true);
        slots.sort_unstable();
        sv.bots_mut().unwrap().wait_for_graph();
        (0..1200)
            .map(|_| {
                now += FRAME;
                sv.drain_console(now);
                sv.tick_frames(now, 1);
                sv.take_outgoing();
                let mut h = Fnv::new();
                let level = sv.level().unwrap();
                for &slot in &slots {
                    h.bytes(&(slot as u32).to_le_bytes());
                    let Some(p) = level.players.get(slot).and_then(Option::as_ref) else {
                        h.bytes(&[0]);
                        continue;
                    };
                    let c = p.oldcmd;
                    h.i32s(&[
                        c.server_time,
                        c.buttons,
                        level.rt.client_vitals(slot).health,
                    ]);
                    h.i32s(&c.angles);
                    h.bytes(&[c.weapon, c.forward_move as u8, c.right_move as u8]);
                    h.f32s(&p.ps.origin.to_array());
                    h.f32s(&p.ps.velocity.to_array());
                }
                h.0
            })
            .collect()
    });
}

/// CoD4: ten bots on mp_crash `war` for 60 s. Needs `COD4_DIR`.
#[test]
fn cod4_bots_move_alike_on_threads() {
    use vcod_server::cod4::{Cod4Config, Cod4Game, Cod4Server};
    use vcod_server::engine::GameModule;
    let Some(install) = vcod_cod4::testing::cod4_install() else {
        return;
    };
    same_on_threads("CoD4", || {
        let mut now = Instant::now();
        let cfg = Cod4Config {
            map: "mp_crash".into(),
            gametype: "war".into(),
            hostname: "vcod test".into(),
            max_clients: 12,
        };
        let mut sv = Cod4Server::new(cfg, 3);
        sv.load(Some(&install)).expect("load the level");
        let mut slots = sv.add_bots(10, true);
        slots.sort_unstable();
        sv.bots_mut().unwrap().wait_for_graph();
        (0..1200)
            .map(|_| {
                now += FRAME;
                Cod4Game::drain_console(&mut sv, now);
                sv.tick_frames(now, 1);
                sv.take_outgoing();
                let mut h = Fnv::new();
                let level = sv.level().unwrap();
                for &slot in &slots {
                    h.bytes(&(slot as u32).to_le_bytes());
                    let (Some(p), Some(ps)) = (
                        level.players.get(slot).and_then(Option::as_ref),
                        level.pm_state(slot),
                    ) else {
                        h.bytes(&[0]);
                        continue;
                    };
                    let c = p.oldcmd;
                    h.i32s(&[
                        c.server_time,
                        c.buttons as i32,
                        level.rt.client_vitals(slot).health,
                    ]);
                    h.i32s(&c.angles.map(i32::from));
                    h.bytes(&[c.weapon, c.forward_move as u8, c.right_move as u8]);
                    h.f32s(&ps.origin.to_array());
                    h.f32s(&ps.velocity.to_array());
                }
                h.0
            })
            .collect()
    });
}
