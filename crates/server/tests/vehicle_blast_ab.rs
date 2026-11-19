//! A rocket's and a frag's explode on an empty United Offensive vehicle,
//! ours against retail's. `client-probes/probe_vehblast` puts client 0 off
//! a Panzer IV on mp_cassino or a Willys on mp_peaks, turns its view onto a
//! point by the hull every frame and presses the trigger through
//! `PROBE_IN` lines (`--probe-uo-weapon direct`): a bazooka round strikes
//! the hull (`G_MissileImpact`'s direct `G_Damage`, an explode 186 with
//! `eFlags` 0x100000) and a frag goes off by it or on its deck
//! (`G_RadiusDamage` on the hull, scaled by the frag's `vehiclescale`),
//! its strikes on the hull bouncing it with no event.
//! The fixtures are retail UO 1.51b's: the probe's log
//! (`fixtures/vehicles/*-blast-*-uo.txt`, recipe in each header) and the
//! shooter's snapshots (`*-uo-blasttank.bin` and kin, the explodes).
//! docs/research/coduo-vehicles.md 13.3, "A blast on a hull".
//!
//! `BLAST_REPORT=1` prints both sides. Needs `COD_DIR` at a UO install;
//! without one it returns early.

use crate::common;

use common::{Join, Queues, step};
use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};
use vcod_common::net::NetEvent;
use vcod_common::net::event_ids::{
    EV_GRENADE_BOUNCE, EV_UO_PROJECTILE_EXPLODE, EV_UO_PROJECTILE_EXPLODE_NOMARKS,
};
use vcod_common::net::events::{EventTracker, GameEvent};
use vcod_common::net::protocol::PROTOCOL_V22;

const SCRIPT: &str =
    include_str!("../../gsc/tests/fixtures/semantics/client-probes/probe_vehblast.gsc");

/// How far an explode or a notify's point may sit from retail's, units on
/// each axis: retail logs the point to the hundredth.
const POINT_NEAR: f32 = 0.02;
/// How far a notify's direction may sit from retail's, on each axis.
const DIR_NEAR: f32 = 0.006;
/// The bazooka's hip spread (`hipSpreadStandMin` 7 degrees) is a random
/// draw on each server, so a round's contact, its direction, the struck
/// face's normal and the splash on the shooter only sit inside the cone:
/// these bound it at the probe's 200 to 300 units.
const SPREAD_POINT_NEAR: f32 = 32.0;
const SPREAD_DIR_NEAR: f32 = 0.25;

/// One retail capture: the map, the log and the snapshot pair's tag.
struct Case {
    map: &'static str,
    fixture: &'static str,
    tag: &'static str,
    /// The weapon draws a spread (the bazooka; a thrown frag does not).
    spread: bool,
    /// Shots whose aim is not compared (points, directions, the explode's
    /// position), each with why; every other field still is.
    gaps: &'static [(usize, &'static str)],
    /// How long ours runs alone before the shooter connects: retail's
    /// clock when its client joined, where the fixture carries that
    /// client's `CMDS` lines and ours replays their cmd times.
    join_ms: i64,
}

/// mp_cassino's spot 300 ahead of the Panzer IV, (4376, -1979), is on a
/// slope the placed shooter slides down for the 1.5 s before the press;
/// where he stops rides the split of his cmds, so this capture carries
/// the retail client's `CMDS` lines, replayed on retail's clock
/// (docs/research/coduo-vehicles.md 13).
const ROCKET_TANK_GAPS: &[(usize, &str)] = &[];

const ROCKET_TANK: Case = Case {
    map: "mp_cassino",
    fixture: include_str!("fixtures/vehicles/mp_cassino-blast-tank-rocket-uo.txt"),
    tag: "uo-blasttank",
    spread: true,
    gaps: ROCKET_TANK_GAPS,
    // The recipe's client started 18 s in and its first cmd ran at 20500.
    join_ms: 19450,
};
const FRAG_TANK: Case = Case {
    map: "mp_cassino",
    fixture: include_str!("fixtures/vehicles/mp_cassino-blast-tank-frag-uo.txt"),
    tag: "uo-fragtank",
    spread: false,
    gaps: &[],
    join_ms: common::PROBE_JOIN_MS,
};
const ROCKET_JEEP: Case = Case {
    map: "mp_peaks",
    fixture: include_str!("fixtures/vehicles/mp_peaks-blast-jeep-rocket-uo.txt"),
    tag: "uo-blastjeep",
    spread: true,
    gaps: &[],
    join_ms: common::PROBE_JOIN_MS,
};
const FRAG_JEEP: Case = Case {
    map: "mp_peaks",
    fixture: include_str!("fixtures/vehicles/mp_peaks-blast-jeep-frag-uo.txt"),
    tag: "uo-fragjeep",
    spread: false,
    gaps: &[],
    join_ms: common::PROBE_JOIN_MS,
};

/// Each retail round's `trDelta`, rebuilt from its direct hit's `vdamage`
/// direction and the weapon's `projectileSpeed` as `fire_rocket`
/// truncates it: the hip spread is a draw of each server's own, so ours
/// flies retail's round. None unless every round struck the hull (a
/// round that missed logs no direction).
fn retail_deltas(c: &Case) -> Vec<[f32; 3]> {
    let fs = vcod_common::testing::uo_fs().expect("a UO install");
    let weapon = c
        .fixture
        .lines()
        .filter(|l| l.starts_with('#'))
        .find_map(|l| l.split("+set probe_weapon ").nth(1))
        .and_then(|r| r.split_whitespace().next())
        .expect("the recipe's probe_weapon");
    let speed = vcod_common::weapon::load(&fs, weapon)
        .expect("the weapon file")
        .projectile_speed;
    let log: Vec<String> = c.fixture.lines().map(str::to_string).collect();
    let deltas: Vec<[f32; 3]> = notifies(&log)
        .iter()
        .filter(|n| n.2 == "MOD_PROJECTILE")
        .map(|n| n.4.map(|d| (d * speed).trunc()))
        .collect();
    match deltas.len() == fires(&log).len() {
        true => deltas,
        false => Vec::new(),
    }
}

/// Explodes with the server time of the snapshot that brought each.
type Explodes = Vec<(i32, GameEvent)>;

fn is_explode(ev: &GameEvent) -> bool {
    ev.event == EV_UO_PROJECTILE_EXPLODE || ev.event == EV_UO_PROJECTILE_EXPLODE_NOMARKS
}

/// The explodes and a frag's bounces (UO's 181), each a gate holds.
fn is_held(ev: &GameEvent) -> bool {
    is_explode(ev) || ev.event == EV_GRENADE_BOUNCE
}

/// `BLAST_MISSILES=1`: each missile in a snapshot, its trajectory and
/// ground, on stderr.
fn print_missiles(side: &str, s: &vcod_common::net::snapshot::Snapshot) {
    if std::env::var_os("BLAST_MISSILES").is_none() {
        return;
    }
    let p = &PROTOCOL_V22;
    for (num, e) in &s.entities {
        let i = |n: &str| e.field_i32(p, n);
        let f = |n: &str| e.field_f32(p, n);
        if i("eType") == 4 {
            eprintln!(
                "{side} {} missile {num} tr {} time {} base {:.2} {:.2} {:.2} delta {:.1} {:.1} {:.1} ground {}",
                s.server_time,
                i("pos.trType"),
                i("pos.trTime"),
                f("pos.trBase[0]"),
                f("pos.trBase[1]"),
                f("pos.trBase[2]"),
                f("pos.trDelta[0]"),
                f("pos.trDelta[1]"),
                f("pos.trDelta[2]"),
                i("groundEntityNum"),
            );
        }
    }
}

/// `BLAST_PS=1`: the shooter's playerstate each snapshot, on stderr.
fn print_ps(side: &str, s: &vcod_common::net::snapshot::Snapshot) {
    if std::env::var_os("BLAST_PS").is_none() {
        return;
    }
    let p = &PROTOCOL_V22;
    let i = |n: &str| s.ps.field_i32(p, n);
    let f = |n: &str| s.ps.field_f32(p, n);
    eprintln!(
        "{side} {} ps o {:.3} {:.3} {:.3} v {:.2} {:.2} {:.2} ground {} pmf {:#x} pmt {} ct {}",
        s.server_time,
        f("origin[0]"),
        f("origin[1]"),
        f("origin[2]"),
        f("velocity[0]"),
        f("velocity[1]"),
        f("velocity[2]"),
        i("groundEntityNum"),
        i("pm_flags"),
        i("pm_type"),
        i("commandTime"),
    );
}

/// The buttons of a `f "PROBE_IN ..."` command.
fn direct_input(tokens: &[String]) -> Option<u8> {
    if tokens.first().map(String::as_str) != Some("f") {
        return None;
    }
    let mut t = tokens.get(1)?.strip_prefix("PROBE_IN ")?.split_whitespace();
    t.next()?.parse().ok()
}

/// Ours: the log and every explode the shooter's snapshots carried.
fn ours(c: &Case) -> Option<(Vec<String>, Explodes)> {
    let _ = env_logger::builder().is_test(true).try_init();
    let fs = vcod_common::testing::uo_fs()?;
    let p = &PROTOCOL_V22;
    let bsp_path = fs.resolve_map(c.map).expect("map in the UO paks");
    let bsp = vcod_common::bsp::parse(&fs.read(&bsp_path).unwrap()).unwrap();
    let mut now = Instant::now();
    let mut sv = vcod_server::Server::new(common::cfg(c.map, "probe_vehblast"), now);
    sv.set_protocol(p);
    sv.overlay_script("maps/mp/gametypes/probe_vehblast", SCRIPT);
    // The recipe's `+set`s.
    for l in c.fixture.lines().filter(|l| l.starts_with('#')) {
        let mut t = l.split_whitespace();
        while let Some(w) = t.next() {
            if w == "+set"
                && let (Some(name), Some(value)) = (t.next(), t.next())
            {
                sv.set_cvar(name, value);
            }
        }
    }
    sv.load_world(vcod_server::world::World::from_bsp(&bsp, Some(&fs)));
    sv.load_scripts(Rc::new(fs)).expect("load the scripts");
    if c.spread {
        sv.pin_rocket_deltas(retail_deltas(c));
    }
    common::run_alone(&mut sv, &mut now, c.join_ms);
    let q = Rc::new(RefCell::new(Queues::default()));
    let mut cl =
        vcod_common::net::NetClient::start_with_qport(common::ClientEnd(q.clone()), now, 0x2001);
    let mut join = Join::new("allies", "thompson_mp");
    let frame = Duration::from_millis(common::FRAME_MS as u64);
    let mut hold = common::ProbeHold {
        aim: true,
        ..Default::default()
    };
    hold.times = common::cmds_times(c.fixture, "");
    hold.ran = common::cmds_ran(c.fixture, "");
    hold.framed = !hold.times.is_empty();
    let (mut buttons, mut selected) = (0u8, None::<u8>);
    let mut tracker = EventTracker::new();
    let mut explodes = Vec::new();
    let mut seen_time = 0;
    for _ in 0..4000 {
        if sv.script_log().iter().any(|l| l.contains("PROBE done")) {
            break;
        }
        now += frame;
        let live = cl
            .snapshots()
            .newest()
            .is_some_and(|s| s.ps.field_i32(p, "pm_type") == 0);
        if live {
            let mut cmds = hold.cmds(&cl, buttons);
            if let Some(w) = selected {
                for c in &mut cmds {
                    c.weapon = w;
                }
            }
            cl.send_cmds(&cmds);
        } else if let Some(c) = cl.send_frame(&vcod_common::net::msg::UserCmd::default()) {
            hold.last = c.server_time;
        }
        for e in step(&mut sv, &q, &mut cl, now) {
            if let NetEvent::ServerCommand(tokens) = e {
                if tokens.first().map(String::as_str) == Some("a") {
                    selected = tokens.get(1).and_then(|w| w.parse().ok());
                }
                if let Some(b) = direct_input(&tokens) {
                    buttons = b;
                }
                join.on_server_command(&tokens, &mut cl, now);
            }
        }
        if let Some(s) = cl.snapshots().newest()
            && s.server_time != seen_time
        {
            seen_time = s.server_time;
            print_missiles("ours  ", s);
            print_ps("ours  ", s);
            for ev in tracker.drain(s, p) {
                if is_held(&ev) {
                    explodes.push((s.server_time, ev));
                }
            }
        }
    }
    Some((sv.script_log().to_vec(), explodes))
}

/// The retail capture's explodes in 1.1's numbering.
fn retail_explodes(tag: &str) -> Explodes {
    let p = &PROTOCOL_V22;
    let frames = vcod_common::testing::fixture_frames(
        p,
        &format!("net/gamestate-{tag}.bin"),
        &format!("net/snapshots-{tag}.bin"),
    );
    let mut tracker = EventTracker::new();
    let mut out = Vec::new();
    for f in &frames {
        print_missiles("retail", &f.snapshot);
        print_ps("retail", &f.snapshot);
        for ev in tracker.drain(&f.snapshot, p) {
            if is_held(&ev) {
                out.push((f.snapshot.server_time, ev));
            }
        }
    }
    out
}

fn vec3(s: &str) -> [f32; 3] {
    let v: Vec<f32> = s
        .trim_matches(|c| c == '(' || c == ')')
        .split(',')
        .map(|x| x.trim().parse().unwrap())
        .collect();
    [v[0], v[1], v[2]]
}

/// The `PROBE fire` times, by shot.
fn fires(log: &[String]) -> Vec<i32> {
    log.iter()
        .filter_map(|l| l.trim().strip_prefix("PROBE fire "))
        .map(|l| l.split(' ').next().unwrap().parse().unwrap())
        .collect()
}

/// Which shot a time falls in, and the time since its press.
fn since_fire(fires: &[i32], t: i32) -> (usize, i32) {
    let k = fires.partition_point(|&f| f <= t).max(1) - 1;
    (k, t - fires[k])
}

/// A `PROBE vdamage` line: the shot and the time since its press, the
/// amount, the means of death, the point, the direction, the attacker
/// and the health left.
type Notify = ((usize, i32), i32, String, [f32; 3], [f32; 3], i32, i32);

fn notifies(log: &[String]) -> Vec<Notify> {
    let fires = fires(log);
    log.iter()
        .filter_map(|l| l.trim().strip_prefix("PROBE vdamage ").map(str::to_string))
        .map(|l| {
            // `<time> <amount> <mod> (x, y, z) (x, y, z) <attacker> <inflictor> <health>`
            let (head, rest) = l.split_once(" (").unwrap();
            let h: Vec<&str> = head.split(' ').collect();
            let (point, rest) = rest.split_once(") (").unwrap();
            let (dir, tail) = rest.split_once(") ").unwrap();
            let t: Vec<&str> = tail.split(' ').collect();
            (
                since_fire(&fires, h[0].parse().unwrap()),
                h[1].parse().unwrap(),
                h[2].to_string(),
                vec3(point),
                vec3(dir),
                t[0].parse().unwrap(),
                t[2].parse().unwrap(),
            )
        })
        .collect()
}

/// The shooter's own `PROBE hit` lines: the shot, the time since its
/// press and the rest of the line.
fn hits(log: &[String]) -> Vec<((usize, i32), String)> {
    let fires = fires(log);
    log.iter()
        .filter_map(|l| l.trim().strip_prefix("PROBE hit "))
        .map(|l| {
            let (t, rest) = l.split_once(' ').unwrap();
            (since_fire(&fires, t.parse().unwrap()), rest.to_string())
        })
        .collect()
}

fn near(a: [f32; 3], b: [f32; 3], tol: f32) -> bool {
    (0..3).all(|i| (a[i] - b[i]).abs() <= tol)
}

fn check(c: &Case) {
    let Some((log, ours_ex)) = ours(c) else {
        return;
    };
    let retail: Vec<String> = c.fixture.lines().map(str::to_string).collect();
    let retail_ex = retail_explodes(c.tag);
    if std::env::var_os("BLAST_REPORT").is_some() {
        for l in retail.iter().filter(|l| l.starts_with("PROBE")) {
            eprintln!("retail {l}");
        }
        for l in log.iter().filter(|l| l.contains("PROBE")) {
            eprintln!("ours   {}", l.trim());
        }
        for (side, ex) in [("retail", &retail_ex), ("ours  ", &ours_ex)] {
            for (t, e) in ex {
                eprintln!(
                    "{side} t {t} ev {} parm {} surf {} scale {} weapon {} eFlags {:#x} pos {:?}",
                    e.event, e.parm, e.surf_type, e.scale, e.weapon, e.eflags, e.pos
                );
            }
        }
    }
    let (point_near, dir_near) = match c.spread {
        true => (SPREAD_POINT_NEAR, SPREAD_DIR_NEAR),
        false => (POINT_NEAR, DIR_NEAR),
    };
    let (rn, on) = (notifies(&retail), notifies(&log));
    assert_eq!(
        on.len(),
        rn.len(),
        "the vehicle's notifies: retail {rn:?}, ours {on:?}"
    );
    let gap = |k: usize| c.gaps.iter().any(|g| g.0 == k);
    for (o, r) in on.iter().zip(&rn) {
        assert_eq!(
            (o.0, o.1, &o.2, o.5, o.6),
            (r.0, r.1, &r.2, r.5, r.6),
            "shot and time, amount, means, attacker, health"
        );
        if gap(r.0.0) {
            continue;
        }
        assert!(
            near(o.3, r.3, point_near),
            "point: retail {:?}, ours {:?}",
            r.3,
            o.3
        );
        assert!(
            near(o.4, r.4, dir_near),
            "dir: retail {:?}, ours {:?}",
            r.4,
            o.4
        );
    }
    let (rh, oh) = (hits(&retail), hits(&log));
    if c.spread {
        // The round's splash on the shooter rides where it struck; any
        // other blast (a wreck's) is compared whole.
        let means = |h: &[((usize, i32), String)]| -> Vec<((usize, i32), String)> {
            h.iter()
                .map(|(k, l)| match l.contains("MOD_PROJECTILE_SPLASH") {
                    true => (*k, l.split(" mod ").nth(1).unwrap_or("").to_string()),
                    false => (*k, l.clone()),
                })
                .collect()
        };
        assert_eq!(means(&oh), means(&rh), "the shooter's own blast");
    } else {
        assert_eq!(oh, rh, "the shooter's own blast");
    }
    let fires_r = fires(&retail);
    let fires_o = fires(&log);
    // A frag's bounces, by its throw and the ms since it: none off a hull,
    // which takes damage while the frag's `damage` is 0 (`G_MissileImpact`,
    // coduo-vehicles.md 13).
    let bounces = |ex: &Explodes, fires: &[i32]| -> Vec<((usize, i32), i32)> {
        ex.iter()
            .filter(|(_, e)| e.event == EV_GRENADE_BOUNCE)
            .map(|(t, e)| (since_fire(fires, *t), e.parm))
            .collect()
    };
    assert_eq!(
        bounces(&ours_ex, &fires_o),
        bounces(&retail_ex, &fires_r),
        "the bounces"
    );
    let ours_ex: Explodes = ours_ex.into_iter().filter(|(_, e)| is_explode(e)).collect();
    let retail_ex: Explodes = retail_ex
        .into_iter()
        .filter(|(_, e)| is_explode(e))
        .collect();
    assert_eq!(ours_ex.len(), retail_ex.len(), "the explodes");
    for ((ot, o), (rt, r)) in ours_ex.iter().zip(&retail_ex) {
        // A round's struck face is where the spread put it.
        let parm = |e: &GameEvent| if c.spread { 0 } else { e.parm };
        assert_eq!(
            (
                since_fire(&fires_o, *ot),
                o.event,
                parm(o),
                o.surf_type,
                o.scale,
                o.weapon,
                o.eflags
            ),
            (
                since_fire(&fires_r, *rt),
                r.event,
                parm(r),
                r.surf_type,
                r.scale,
                r.weapon,
                r.eflags
            ),
            "the explode"
        );
        if gap(since_fire(&fires_r, *rt).0) {
            continue;
        }
        assert!(
            near(o.pos, r.pos, point_near),
            "explode at: retail {:?}, ours {:?}",
            r.pos,
            o.pos
        );
    }
}

#[test]
fn a_rocket_strikes_a_tank_as_on_uo() {
    check(&ROCKET_TANK);
}

#[test]
fn a_frag_goes_off_by_a_tank_as_on_uo() {
    check(&FRAG_TANK);
}

#[test]
fn a_rocket_strikes_a_jeep_as_on_uo() {
    check(&ROCKET_JEEP);
}

#[test]
fn a_frag_goes_off_by_a_jeep_as_on_uo() {
    check(&FRAG_JEEP);
}
