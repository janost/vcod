//! vcod-server in process, played through the client's own frame: the clock
//! (`play::clock`), the cmd ring and its packets (`play::cmds`), the
//! prediction, the event collection and the kick, called in `main.rs`'s
//! order, over a link with optional delay and jitter and a frame rate with
//! hitches. The rounds fired are counted off the server's ammo, what the
//! client played off `events::frame_events`; each round has to play and kick
//! once. Needs `COD_DIR` (UO's install runs UO, else 1.1).

use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::rc::Rc;
use std::time::{Duration, Instant};

use vcod_common::collision::CollisionWorld;
use vcod_common::net::NetEvent;
use vcod_common::net::events::EventTracker;
use vcod_common::net::msg::{
    BUTTON_ADS, BUTTON_ATTACK, BUTTON_SPRINT_V22, NULL_USERCMD, PlayerState, UserCmd,
    WBUTTON_CROUCH, WBUTTON_LEAN_LEFT, WBUTTON_PRONE, WBUTTON_RELOAD,
};
use vcod_common::net::protocol::{PROTOCOL_V1, PROTOCOL_V22, Protocol};
use vcod_common::pk3::Pk3Fs;
use vcod_common::pmove::movers::SnapshotMovers;
use vcod_common::pmove::predict;

use super::clock::ServerClock;
use super::cmds::{CmdClock, CmdRing};
use super::events::{self, PredictedEvents};
use super::predict::{Predictor, solid_bodies};
use super::recoil::{self, Recoil};

// The server tests' in-process client: the queues, the connect and the
// stock menu join.
#[allow(clippy::all, unused)]
#[path = "../../../server/tests/common/mod.rs"]
mod common;

/// `sv_fps` 20.
const SERVER_FRAME_MS: f64 = 50.0;

/// xorshift, uniform in [0, 1).
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> f64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 % 1_000_000) as f64 / 1_000_000.0
    }
}

/// The link and the client's frame pacing.
#[derive(Clone, Copy, Debug)]
struct Conditions {
    /// One-way delay and its spread, ms. Datagrams keep their order.
    delay: f64,
    jitter: f64,
    /// Frame length range, ms, and the percentage of frames that hitch
    /// for 60 to 250 ms instead.
    frame_ms: (f64, f64),
    hitch_pct: f64,
    /// Percentage of datagrams lost, each way.
    loss_pct: f64,
    seed: u64,
}

/// One direction of the link: datagrams with the local ms they come out.
#[derive(Default)]
struct Lane(VecDeque<(f64, Vec<u8>)>);

impl Lane {
    fn push(&mut self, c: &Conditions, rng: &mut Rng, at: f64, data: Vec<u8>) {
        if c.loss_pct > 0.0 && rng.next() * 100.0 < c.loss_pct {
            return;
        }
        let t = at + c.delay + (rng.next() - 0.5) * c.jitter;
        let t = self.0.back().map_or(t, |&(last, _)| t.max(last));
        self.0.push_back((t, data));
    }

    fn due(&mut self, now: f64) -> Vec<Vec<u8>> {
        let mut out = Vec::new();
        while self.0.front().is_some_and(|&(t, _)| t <= now) {
            out.push(self.0.pop_front().unwrap().1);
        }
        out
    }
}

/// Buttons and keys held for `ms`.
#[derive(Clone, Copy)]
struct Hold {
    ms: f64,
    buttons: u8,
    wbuttons: u8,
    forward: i8,
    right: i8,
    up: i8,
}

const fn hold(ms: f64, buttons: u8, wbuttons: u8, forward: i8, right: i8, up: i8) -> Hold {
    Hold {
        ms,
        buttons,
        wbuttons,
        forward,
        right,
        up,
    }
}

const A: u8 = BUTTON_ATTACK;
const S: u8 = BUTTON_ADS;
const SPRINT: u8 = BUTTON_SPRINT_V22;

/// The trigger held from the hip, tapped, held down the sight and on the
/// move, a reload, then UO's sprint into fire and a winded spray standing,
/// jumping, crouched, prone and leaning.
const SCRIPT: &[Hold] = &[
    hold(500.0, 0, 0, 0, 0, 0),
    hold(1500.0, A, 0, 0, 0, 0),
    hold(600.0, 0, 0, 0, 0, 0),
    hold(60.0, A, 0, 0, 0, 0),
    hold(300.0, 0, 0, 0, 0, 0),
    hold(60.0, A, 0, 0, 0, 0),
    hold(300.0, 0, 0, 0, 0, 0),
    hold(800.0, S, 0, 0, 0, 0),
    hold(1200.0, S | A, 0, 0, 0, 0),
    hold(700.0, 0, 0, 0, 0, 0),
    hold(1500.0, A, 0, 127, 0, 0),
    hold(1200.0, A, 0, 0, 127, 0),
    hold(1200.0, A, 0, -127, -127, 0),
    hold(1500.0, 0, 0, 0, 0, 0),
    hold(200.0, 0, WBUTTON_RELOAD, 0, 0, 0),
    hold(3000.0, 0, 0, 0, 0, 0),
    hold(1200.0, SPRINT, 0, 127, 0, 0),
    hold(600.0, SPRINT | A, 0, 127, 0, 0),
    hold(900.0, A, 0, 127, 0, 0),
    hold(300.0, A, 0, 127, 0, 127),
    hold(700.0, A, 0, 127, 0, 0),
    hold(1200.0, A, WBUTTON_CROUCH, 0, 0, 0),
    hold(1500.0, A, WBUTTON_PRONE, 0, 0, 0),
    hold(1500.0, 0, 0, 0, 0, 0),
    hold(900.0, A, WBUTTON_LEAN_LEFT, 0, 0, 0),
    hold(900.0, A | SPRINT, 0, 127, 0, 0),
    hold(1500.0, 0, 0, 0, 0, 0),
];

/// What a run fired and played.
#[derive(Debug, PartialEq)]
struct Outcome {
    /// Rounds the server took off the player's ammo.
    rounds: i32,
    /// Fire events the client played, and the kicks they gave the view.
    fires: usize,
    kicks: usize,
}

/// The paks and protocol: UO's when `COD_DIR` is a UO install, else 1.1's.
fn setup() -> Option<(Pk3Fs, &'static Protocol)> {
    match vcod_common::testing::uo_fs() {
        Some(fs) => Some((fs, &PROTOCOL_V22)),
        None => Some((vcod_common::testing::game_fs()?, &PROTOCOL_V1)),
    }
}

/// Joins mp_carentan dm as axis with `weapon` and plays [`SCRIPT`] under
/// `c`, one client frame at a time beside a server frame every 50 ms.
fn run(c: Conditions, weapon: &str) -> Option<Outcome> {
    let (fs, p) = setup()?;
    let uo = p.is_uo();
    let bsp_path = fs.resolve_map("mp_carentan").expect("mp_carentan");
    let bsp = vcod_common::bsp::parse(&fs.read(&bsp_path).unwrap()).unwrap();
    let world = CollisionWorld::build(
        &bsp,
        &vcod_common::props::collision_tris(&fs, &bsp.entities),
    );
    world.unlink_script_brushes(&bsp.entities, "dm");
    world.set_arith(vcod_common::patch::Arith::for_client());
    let fs = Rc::new(fs);
    let t0 = Instant::now();
    let mut now = t0;
    let mut sv = vcod_server::Server::new(common::cfg("mp_carentan", "dm"), now);
    sv.set_protocol(p);
    sv.load_world(vcod_server::world::World::from_bsp(&bsp, Some(&fs)));
    sv.load_scripts(fs.clone()).expect("load the scripts");
    let q = Rc::new(RefCell::new(common::Queues::default()));
    let (mut net, _) = common::join(&mut sv, &q, &mut now, "axis", weapon);
    let weapons = vcod_common::weapon_table::from_configstring(&fs, net.configstring(7));
    let ps0 = net.snapshots().newest().unwrap().ps.clone();
    let held = ps0.field_i32(p, "weapon") as u8;
    assert_eq!(
        net.configstring(7).split(' ').nth(usize::from(held) - 1),
        Some(weapon),
        "joined with another weapon"
    );
    let def = weapons[usize::from(held)]
        .as_ref()
        .expect("the weapon loads");
    let ammo = |ps: &PlayerState| {
        i32::from(ps.arrays.ammo[def.ammo_index]) + i32::from(ps.arrays.ammoclip[def.clip_index])
    };

    let mut rng = Rng(c.seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1);
    let (mut up, mut down) = (Lane::default(), Lane::default());
    let mut arrived: Vec<(Instant, Vec<u8>)> = Vec::new();
    let mut clock = ServerClock::default();
    let mut cmd_clock = CmdClock::default();
    let mut ring = CmdRing::default();
    let mut predictor = Predictor::default();
    let mut tracked = PredictedEvents::default();
    let mut tracker = EventTracker::new();
    let mut recoil = Recoil::default();
    let mut out = Outcome {
        rounds: 0,
        fires: 0,
        kicks: 0,
    };

    let start = now.duration_since(t0).as_secs_f64() * 1000.0;
    let (mut local, mut last_local) = (start, start);
    let mut next_frame = start + SERVER_FRAME_MS;
    let mut script = SCRIPT.iter().copied();
    let mut stretch = script.next().unwrap();
    let mut stretch_end = local + stretch.ms;
    // A second and a half of quiet after the script, for the last snapshot.
    let end = start + SCRIPT.iter().map(|h| h.ms).sum::<f64>() + 1500.0;
    while local < end {
        let mut ft = c.frame_ms.0 + rng.next() * (c.frame_ms.1 - c.frame_ms.0);
        if rng.next() * 100.0 < c.hitch_pct {
            ft = 60.0 + rng.next() * 190.0;
        }
        local += ft;
        while local >= stretch_end {
            stretch = script.next().unwrap_or(hold(f64::MAX, 0, 0, 0, 0, 0));
            stretch_end += stretch.ms;
        }
        // Last frame's packets on the wire, then the server's loop: a frame
        // every 50 ms handles what arrived, ticks and sends (`vcod-server`'s
        // main).
        for pkt in q.borrow_mut().to_server.drain(..) {
            up.push(&c, &mut rng, last_local, pkt);
        }
        while next_frame <= local {
            let at = t0 + Duration::from_secs_f64(next_frame / 1000.0);
            arrived.extend(up.due(next_frame).into_iter().map(|p| (at, p)));
            for (when, pkt) in arrived.drain(..) {
                let read = vcod_server::engine::Arrival {
                    now: at,
                    arrived: when,
                    read: when,
                };
                sv.packet_read(common::ADDR, &pkt, read);
            }
            sv.tick(at);
            for (to, pkt) in sv.take_outgoing() {
                assert_eq!(to, common::ADDR);
                down.push(&c, &mut rng, next_frame, pkt);
            }
            sv.frame_sent(at);
            next_frame += SERVER_FRAME_MS;
        }
        q.borrow_mut().to_client.extend(down.due(local));

        // The client's frame.
        now = t0 + Duration::from_secs_f64(local / 1000.0);
        for e in net.pump_at(now) {
            assert!(!matches!(e, NetEvent::Dropped(_)), "{e:?}");
        }
        let snap = net.snapshots().newest().unwrap().clone();
        let realtime = local as i32;
        clock.update(realtime, snap.server_time, snap.message_num, 0);
        let kick = recoil.view_kick();
        for t in cmd_clock.due(clock.cmd_time(snap.server_time)) {
            ring.push(UserCmd {
                server_time: t,
                buttons: if uo {
                    stretch.buttons
                } else {
                    stretch.buttons & !SPRINT
                },
                wbuttons: stretch.wbuttons,
                weapon: held,
                forward: stretch.forward,
                right: stretch.right,
                up: stretch.up,
                angles: std::array::from_fn(|i| (kick[i] * 65536.0 / 360.0) as i32),
                ..NULL_USERCMD
            });
        }
        if ring.packet_due(realtime, true, 30) {
            net.send_cmds(&ring.packet(realtime, 1));
        }
        let client_num = snap.ps.field_i32(p, "clientNum");
        let predicting = predict::predictable(snap.ps.field_i32(p, "pm_type"));
        let predicted = if predicting {
            let bodies = solid_bodies(p, &snap.entities, client_num as u32, &HashMap::new());
            let movers = (
                SnapshotMovers::from_entities(p, &snap.entities),
                snap.server_time,
            );
            predictor.predict(
                p, &snap.ps, &ring, &world, &bodies, &movers, &weapons, local,
            )
        } else {
            None
        };
        let evs = events::frame_events(
            &mut tracked,
            &mut tracker,
            &snap,
            p,
            true,
            predicting,
            predicted.as_ref().map(|v| &v.pred),
            client_num,
        );
        let frac = predicted.as_ref().map_or_else(
            || snap.ps.field_f32(p, "fWeaponPosFrac"),
            |v| v.pred.ps.weapon_pos_frac,
        );
        let kicks = events::view_kicks(&evs, client_num);
        for _ in 0..kicks {
            recoil.fire(def, frac);
        }
        out.kicks += kicks;
        out.fires += evs
            .iter()
            .filter(|e| recoil::fire_calls(e.event) > 0)
            .count();
        recoil.step((local - last_local) as i32, Some(def), frac, true);
        last_local = local;
    }
    out.rounds = ammo(&ps0) - ammo(&net.snapshots().newest().unwrap().ps);
    Some(out)
}

/// The mp44 held through every stance, the sight and UO's sprint, at a
/// steady 125 fps, at an uneven rate with hitches, over a delayed, jittery
/// link and over one that also loses 3% each way: each round the server
/// fired plays one fire event and one kick. A UO server that left `iFatigueSoundTime` off the wire had the
/// predictor breathe on every snapshot while winded, which pushed the next
/// shot's event to another slot and played it again: 97 fires and kicks for
/// 76 rounds (coduo-movement.md 15).
#[test]
fn each_round_of_held_fire_plays_and_kicks_once() {
    let cases = [
        ("125 fps", (7.9, 8.1), 0.0, 0.0, 0.0, 0.0, 1),
        ("uneven", (4.0, 25.0), 1.0, 0.0, 0.0, 0.0, 2),
        ("jittery link", (6.0, 18.0), 0.5, 60.0, 40.0, 0.0, 3),
        // Seed 14 at a UO install played a shot twice before the snapshot
        // drain kept to the remembered window (`events::filter_snapshot`).
        ("lossy link", (6.0, 18.0), 0.5, 60.0, 40.0, 3.0, 14),
    ];
    for (name, frame_ms, hitch_pct, delay, jitter, loss_pct, seed) in cases {
        let c = Conditions {
            delay,
            jitter,
            frame_ms,
            hitch_pct,
            loss_pct,
            seed,
        };
        let Some(o) = run(c, "mp44_mp") else {
            return;
        };
        println!("{name}: {o:?}");
        assert!(o.rounds > 50, "{name}: only {} rounds", o.rounds);
        assert_eq!(o.fires as i32, o.rounds, "{name}: fire events played");
        assert_eq!(o.kicks as i32, o.rounds, "{name}: kicks");
    }
}

/// Joins mp_carentan dm as axis with `weapon`, then presses each of `binds`
/// a second apart through [`super::input::PlayInput`] and the cmds it
/// builds, each gated on `cg_weaponCycleDelay` 0 as `main.rs` gates it, off
/// the weapon name's stamp on the snapshot clock. Returns the held weapon
/// at the start, the selection after each press and the server's
/// `ps.weapon` a second later. `give` is handed out once the join settles.
fn switch_run(weapon: &str, give: &[&str], binds: &[super::input::Action]) -> Option<SwitchRun> {
    use super::input::{Held, PlayInput};
    use crate::hud::player::WeaponNameFade;
    type Client = vcod_common::net::NetClient<common::ClientEnd>;
    let (fs, p) = setup()?;
    let bsp_path = fs.resolve_map("mp_carentan").expect("mp_carentan");
    let bsp = vcod_common::bsp::parse(&fs.read(&bsp_path).unwrap()).unwrap();
    let fs = Rc::new(fs);
    let mut now = Instant::now();
    let mut sv = vcod_server::Server::new(common::cfg("mp_carentan", "dm"), now);
    sv.set_protocol(p);
    sv.load_world(vcod_server::world::World::from_bsp(&bsp, Some(&fs)));
    sv.load_scripts(fs.clone()).expect("load the scripts");
    let q = Rc::new(RefCell::new(common::Queues::default()));
    let (mut net, _) = common::join(&mut sv, &q, &mut now, "axis", weapon);
    let weapons = vcod_common::weapon_table::from_configstring(&fs, net.configstring(7));
    let mut input = PlayInput::default();
    input.new_gamestate();
    let mut name = WeaponNameFade::default();
    let mut frames = |sv: &mut vcod_server::Server,
                      input: &mut PlayInput,
                      name: &mut WeaponNameFade,
                      net: &mut Client,
                      n: usize| {
        for _ in 0..n {
            now += Duration::from_millis(50);
            let s = net.snapshots().newest().unwrap();
            let spawn = (s.ps.field_i32(p, "clientNum"), s.ps.arrays.stats[5]);
            name.step(spawn, s.server_time);
            let cmd = input.build(0, &Held::from_ps(&s.ps, p, &weapons));
            net.send_frame(&cmd);
            common::step(sv, &q, net, now);
        }
        name.cycle_allowed(0)
    };
    let mut allowed = frames(&mut sv, &mut input, &mut name, &mut net, 20);
    for w in give {
        assert!(sv.test_give_weapon(0, w), "give {w}");
    }
    if !give.is_empty() {
        allowed = frames(&mut sv, &mut input, &mut name, &mut net, 10);
    }
    let held = |net: &Client| net.snapshots().newest().unwrap().ps.field_i32(p, "weapon") as u8;
    let start = held(&net);
    let ps = &net.snapshots().newest().unwrap().ps;
    let slots = Held::from_ps(ps, p, &weapons).slots;
    let (mut selected, mut seen) = (Vec::new(), Vec::new());
    for &b in binds {
        assert!(allowed, "cg_weaponCycleDelay 0 refused a select");
        input.key(b, true);
        input.key(b, false);
        name.select();
        selected.push(input.selected());
        allowed = frames(&mut sv, &mut input, &mut name, &mut net, 30);
        seen.push(held(&net));
    }
    Some(SwitchRun {
        uo: p.is_uo(),
        start,
        slots,
        selected,
        seen,
    })
}

/// What [`switch_run`] saw: the protocol, the weapon held and the slots
/// after the join, the selection after each press and the server's weapon.
struct SwitchRun {
    uo: bool,
    start: u8,
    slots: [u8; 8],
    selected: Vec<u8>,
    seen: Vec<u8>,
}

/// The number keys and the wheel switch the held weapon on the server.
#[test]
fn binds_switch_the_held_weapon() {
    use super::input::Action;
    let binds = [
        Action::Slot(3),
        Action::Slot(1),
        Action::NextWeapon,
        Action::PrevWeapon,
    ];
    let Some(SwitchRun {
        start,
        selected,
        seen,
        ..
    }) = switch_run("kar98k_mp", &[], &binds)
    else {
        return;
    };
    assert_ne!(selected[0], start, "weaponslot pistol selects");
    assert_eq!(selected[1], start, "weaponslot primary selects");
    assert_eq!(seen, selected, "the server holds what the client selects");
}

/// Every `weaponslot` name the stock binds use, through the console: a slot
/// that holds a weapon switches the server's `ps.weapon` to it, an empty one
/// or one past the protocol's last leaves the weapon held. On UO the
/// player also carries a satchel and binoculars, which dm hands nobody.
#[test]
fn every_slot_name_switches_the_held_weapon() {
    use crate::console::shell::{Effect, Shell};
    use vcod_common::weapon::SLOT_NAMES;
    let mut shell = Shell::new();
    let mut binds = Vec::new();
    for name in &SLOT_NAMES[1..] {
        // Back to the rifle between names, so each one is a switch.
        for line in [format!("weaponslot {name}"), "weaponslot primary".into()] {
            match shell.execute(&line).as_slice() {
                [Effect::Impulse(a)] => binds.push(*a),
                other => panic!("{line}: {other:?}"),
            }
        }
    }
    let uo = vcod_common::testing::uo_fs().is_some();
    let give: &[&str] = if uo {
        &["satchelcharge_mp", "binoculars_mp"]
    } else {
        &[]
    };
    let Some(r) = switch_run("kar98k_mp", give, &binds) else {
        return;
    };
    assert_eq!(r.uo, uo);
    let last = if r.uo { 7 } else { 5 };
    for (n, name) in SLOT_NAMES.iter().enumerate().skip(1) {
        let i = 2 * (n - 1);
        let want = if n <= last && r.slots[n] != 0 {
            r.slots[n]
        } else {
            r.start
        };
        println!(
            "{name}: slot {} selected {} held {}",
            r.slots[n], r.selected[i], r.seen[i]
        );
        assert_eq!(r.selected[i], want, "weaponslot {name} selects");
        assert_eq!(r.seen[i], want, "weaponslot {name} reaches the server");
        assert_eq!(r.seen[i + 1], r.start, "weaponslot primary after {name}");
    }
    let held: Vec<_> = (1..=last).filter(|&n| r.slots[n] != 0).collect();
    println!("slots {:?}", r.slots);
    // dm hands 1.1 a rifle, a pistol and grenades; UO adds smoke, and the
    // test the satchel and binoculars.
    assert_eq!(
        held,
        if r.uo {
            vec![1, 3, 4, 5, 6, 7]
        } else {
            vec![1, 3, 4]
        }
    );
}

/// Two frags thrown on vcod-server through `PlayInput` (the grenade slot,
/// the trigger held and let go), watched until both blow: each frag's
/// explode plays once off the snapshot drain, as each of retail's does
/// (`replay::each_retail_explode_plays_one_effect`). UO's battle rank 0
/// hands out one frag, so the test fills the grenade's clip first; UO's
/// explode arrives as its own `EV_PROJECTILE_EXPLODE`.
#[test]
fn each_thrown_frag_explodes_once() {
    use super::input::{Action, Held, PlayInput};
    use vcod_common::net::event_ids::{EV_GRENADE_EXPLODE, EV_UO_PROJECTILE_EXPLODE};
    use vcod_common::net::flags::ET_MISSILE;
    let Some((fs, p)) = setup() else {
        return;
    };
    let bsp_path = fs.resolve_map("mp_carentan").expect("mp_carentan");
    let bsp = vcod_common::bsp::parse(&fs.read(&bsp_path).unwrap()).unwrap();
    let fs = Rc::new(fs);
    let mut now = Instant::now();
    let mut sv = vcod_server::Server::new(common::cfg("mp_carentan", "dm"), now);
    sv.set_protocol(p);
    sv.load_world(vcod_server::world::World::from_bsp(&bsp, Some(&fs)));
    sv.load_scripts(fs.clone()).expect("load the scripts");
    let q = Rc::new(RefCell::new(common::Queues::default()));
    let (mut net, _) = common::join(&mut sv, &q, &mut now, "axis", "kar98k_mp");
    let weapons = vcod_common::weapon_table::from_configstring(&fs, net.configstring(7));
    let ps = &net.snapshots().newest().unwrap().ps;
    let frag = Held::from_ps(ps, p, &weapons).slots[4] as usize;
    let frag = net
        .configstring(7)
        .split(' ')
        .nth(frag - 1)
        .unwrap()
        .to_owned();
    assert!(sv.test_give_max_ammo(0, &frag), "fill {frag}");
    let mut input = PlayInput::default();
    input.new_gamestate();
    let mut tracker = EventTracker::new();
    let (mut missiles, mut played) = (std::collections::BTreeSet::new(), 0);
    let mut frames = |input: &mut PlayInput, n: usize| {
        for _ in 0..n {
            now += Duration::from_millis(50);
            let s = net.snapshots().newest().unwrap();
            let cmd = input.build(0, &Held::from_ps(&s.ps, p, &weapons));
            net.send_frame(&cmd);
            common::step(&mut sv, &q, &mut net, now);
            let s = net.snapshots().newest().unwrap();
            for (&num, e) in &s.entities {
                if e.field_i32(p, "eType") == ET_MISSILE {
                    missiles.insert(num);
                }
            }
            played += tracker
                .drain(s, p)
                .iter()
                .filter(|e| matches!(e.event, EV_GRENADE_EXPLODE | EV_UO_PROJECTILE_EXPLODE))
                .count();
        }
    };
    frames(&mut input, 20);
    input.key(Action::Slot(4), true);
    input.key(Action::Slot(4), false);
    frames(&mut input, 30);
    for _ in 0..2 {
        input.key(Action::Attack, true);
        frames(&mut input, 10);
        input.key(Action::Attack, false);
        frames(&mut input, 40);
    }
    frames(&mut input, 100);
    println!("{} frags thrown, {played} explodes played", missiles.len());
    assert_eq!(missiles.len(), 2, "frags thrown");
    assert_eq!(played, missiles.len(), "explodes played");
}

/// A cmd built under a key catcher carries `BUTTON_TALK`, and the server
/// lights `EF_TALK` on the player for it, clearing it on the next cmd
/// without (cod15-movement.md 5, "The talk button").
#[test]
fn a_key_catcher_lights_the_talk_flag() {
    use super::input::{Held, PlayInput};
    use vcod_common::net::flags::EF_TALK;
    let Some((fs, p)) = setup() else {
        return;
    };
    let bsp_path = fs.resolve_map("mp_carentan").expect("mp_carentan");
    let bsp = vcod_common::bsp::parse(&fs.read(&bsp_path).unwrap()).unwrap();
    let fs = Rc::new(fs);
    let mut now = Instant::now();
    let mut sv = vcod_server::Server::new(common::cfg("mp_carentan", "dm"), now);
    sv.set_protocol(p);
    sv.load_world(vcod_server::world::World::from_bsp(&bsp, Some(&fs)));
    sv.load_scripts(fs.clone()).expect("load the scripts");
    let q = Rc::new(RefCell::new(common::Queues::default()));
    let (mut net, _) = common::join(&mut sv, &q, &mut now, "axis", "kar98k_mp");
    let weapons = vcod_common::weapon_table::from_configstring(&fs, net.configstring(7));
    let mut input = PlayInput::default();
    input.new_gamestate();
    let mut talk = |talking: bool| {
        input.talking = talking;
        for _ in 0..10 {
            now += Duration::from_millis(50);
            let s = net.snapshots().newest().unwrap();
            let cmd = input.build(0, &Held::from_ps(&s.ps, p, &weapons));
            net.send_frame(&cmd);
            common::step(&mut sv, &q, &mut net, now);
        }
        net.snapshots().newest().unwrap().ps.field_i32(p, "eFlags") & EF_TALK
    };
    assert_eq!(talk(false), 0, "no catcher, no talk flag");
    assert_eq!(talk(true), EF_TALK, "a catcher up lights the talk flag");
    assert_eq!(talk(false), 0, "the flag clears once it is down");
}
