//! A gun on foot against an empty United Offensive vehicle, ours against
//! retail's. `client-probes/probe_vehicle_hits` puts client 0 off the
//! vehicle's flank and turns its sighted, held trigger onto a point 40 above
//! the vehicle's origin every frame; the vehicle's `damage` notifies are the
//! `PROBE vdamage` lines. The fixtures are retail UO 1.51b's `PROBE` lines
//! (recipe in their headers, docs/research/coduo-vehicles.md 13): the hull
//! stops the rounds, a jeep takes each through `Scr_Vehicle_DamageScale`
//! truncated, a tank none.
//!
//! `HITS_REPORT=1` prints both logs. Needs `COD_DIR` at a UO install;
//! without one it returns early.

use crate::common;

use common::{Join, Queues, step};
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;
use std::time::{Duration, Instant};
use vcod_common::net::NetClient;
use vcod_common::net::NetEvent;
use vcod_common::net::msg::{BUTTON_ADS, BUTTON_ATTACK, UserCmd};
use vcod_common::net::protocol::PROTOCOL_V22;

/// How long after the placement both logs are compared.
const WINDOW_MS: i32 = 8000;

/// The capture's shooter, `--probe-uo-weapon hold`, waits this long from
/// its spawn before it sights and fires (`vehicle_probe::HOLD`'s
/// "settle"): retail's first notify comes 1050-1100 ms after the
/// placement, which the gsc makes on the spawn frame.
const SETTLE_MS: i64 = 1000;

/// Our run of the probe on `map` against a `vehicletype`; the script log.
fn ours(map: &str, vehicletype: &str, cvars: &[(&str, &str)]) -> Option<Vec<String>> {
    let _ = env_logger::builder().is_test(true).try_init();
    let fs = vcod_common::testing::uo_fs()?;
    let bsp_path = fs.resolve_map(map).expect("map in the UO paks");
    let bsp = vcod_common::bsp::parse(&fs.read(&bsp_path).unwrap()).unwrap();
    let mut now = Instant::now();
    let mut sv = vcod_server::Server::new(common::cfg(map, "probe_vehicle_hits"), now);
    sv.set_protocol(&PROTOCOL_V22);
    sv.overlay_script(
        "maps/mp/gametypes/probe_vehicle_hits",
        include_str!("../../gsc/tests/fixtures/semantics/client-probes/probe_vehicle_hits.gsc"),
    );
    sv.set_cvar("probe_vehicletype", vehicletype);
    for (k, v) in cvars {
        sv.set_cvar(k, v);
    }
    sv.load_world(vcod_server::world::World::from_bsp(&bsp, Some(&fs)));
    sv.load_scripts(Rc::new(fs)).expect("load the scripts");
    common::run_alone(&mut sv, &mut now, common::PROBE_JOIN_MS);
    let q = Rc::new(RefCell::new(Queues::default()));
    let mut cl = NetClient::start_with_qport(common::ClientEnd(q.clone()), now, 0x2001);
    let mut join = Join::new("allies", "thompson_mp");
    let frame = Duration::from_millis(common::FRAME_MS as u64);
    // The capture's net probe sends its cmds on a 60 Hz grid, so a stock
    // thompson's kick settles between its 100 ms rounds
    // (cod15-movement.md 6).
    let mut hold = common::ProbeHold {
        sixty: true,
        ..Default::default()
    };
    let mut placed_at: Option<i64> = None;
    let mut pressed = false;
    for i in 0..2000i64 {
        now += frame;
        if placed_at.is_some_and(|at| (i - at) * common::FRAME_MS > i64::from(WINDOW_MS) + 1000) {
            break;
        }
        // The probe holds the view of its first spawned snapshot, which is
        // the placement's.
        let placed_seen = placed_at.is_some_and(|at| i > at);
        let settled = placed_at.is_some_and(|at| (i - at) * common::FRAME_MS > SETTLE_MS);
        if placed_seen && cl.state() == vcod_common::net::NetState::Active {
            let buttons = if settled {
                BUTTON_ATTACK | BUTTON_ADS
            } else {
                0
            };
            let mut cmds = hold.cmds(&cl, buttons);
            // The trigger goes down on the second cmd of its batch. The
            // first after a frame aims along the view the gsc's
            // `setPlayerAngles` left, the rest along the held view the cmd
            // rebases back to, and every sixth cmd fires on the press's
            // phase: retail's rounds left along the held view
            // (coduo-vehicles.md 13, "The shooter's phase").
            if settled && !pressed {
                if let Some(c) = cmds.first_mut() {
                    c.buttons = 0;
                }
                pressed = !cmds.is_empty();
            }
            cl.send_cmds(&cmds);
        } else if let Some(c) = cl.send_frame(&UserCmd::default()) {
            hold.last = c.server_time;
        }
        for e in step(&mut sv, &q, &mut cl, now) {
            if let NetEvent::ServerCommand(tokens) = e {
                join.on_server_command(&tokens, &mut cl, now);
            }
        }
        if placed_at.is_none() && sv.script_log().iter().any(|l| l.contains("PROBE place")) {
            placed_at = Some(i);
        }
    }
    Some(sv.script_log().to_vec())
}

/// The `PROBE` lines, trimmed.
fn probe_lines(log: &[String]) -> Vec<String> {
    log.iter()
        .map(|l| l.trim().to_string())
        .filter(|l| l.starts_with("PROBE"))
        .collect()
}

fn time_of(line: &str) -> i32 {
    line.split(' ').nth(2).and_then(|t| t.parse().ok()).unwrap()
}

/// The damage notifies inside the window past the placement: amount and
/// means of death, counted.
fn damage_counts(lines: &[String]) -> BTreeMap<(i32, String), usize> {
    let placed = lines
        .iter()
        .find(|l| l.starts_with("PROBE place"))
        .map(|l| time_of(l))
        .expect("the shooter was placed");
    let mut out = BTreeMap::new();
    for l in lines.iter().filter(|l| l.starts_with("PROBE vdamage")) {
        if time_of(l) >= placed + WINDOW_MS {
            continue;
        }
        let t: Vec<&str> = l.split(' ').collect();
        *out.entry((t[3].parse().unwrap(), t[4].to_string()))
            .or_insert(0) += 1;
    }
    out
}

/// The first `bulletTrace` the probe logged: entity and fraction.
fn first_trace(lines: &[String]) -> (i32, f32) {
    let l = lines
        .iter()
        .find(|l| l.starts_with("PROBE trace"))
        .expect("a trace line");
    let t: Vec<&str> = l.split(' ').collect();
    (t[3].parse().unwrap(), t[4].parse().unwrap())
}

fn compare(fixture: &str, ours: &[String]) {
    let retail: Vec<String> = fixture
        .lines()
        .filter(|l| l.starts_with("PROBE"))
        .map(str::to_string)
        .collect();
    if std::env::var("HITS_REPORT").is_ok_and(|v| v == "all") {
        for l in ours {
            eprintln!("log    {}", l.trim());
        }
    }
    let ours = probe_lines(ours);
    if std::env::var("HITS_REPORT").is_ok() {
        for l in &ours {
            eprintln!("ours   {l}");
        }
        for l in &retail {
            eprintln!("retail {l}");
        }
    }
    let (re, rf) = first_trace(&retail);
    let (oe, of) = first_trace(&ours);
    assert_eq!(oe, re, "what bulletTrace stopped on");
    assert!(
        (of - rf).abs() < 0.01,
        "trace fraction: retail {rf}, ours {of}"
    );
    let (rd, od) = (damage_counts(&retail), damage_counts(&ours));
    let total = |m: &BTreeMap<(i32, String), usize>| m.values().sum::<usize>();
    let (rt, ot) = (total(&rd), total(&od));
    // Each round the hull stops is a notify; the counts follow the fire
    // rate, which the clocks shift by a round or two.
    assert!(
        rt.abs_diff(ot) * 5 <= rt.max(1),
        "notifies in {WINDOW_MS} ms: retail {rt}, ours {ot} ({rd:?} / {od:?})"
    );
    // The amount retail gave most rounds is ours too.
    let main = |m: &BTreeMap<(i32, String), usize>| {
        m.iter().max_by_key(|(_, n)| **n).map(|(k, _)| k.clone())
    };
    assert_eq!(main(&od), main(&rd), "retail {rd:?}, ours {od:?}");
}

/// mp_peaks' first Willys from its right flank: every round 22, 50 through
/// the side's 90 and `dmgScaleBullet` 50.
#[test]
fn a_jeep_takes_the_rounds_its_side_stops() {
    let Some(log) = ours("mp_peaks", "willyjeep_mp", &[]) else {
        return;
    };
    compare(
        include_str!("fixtures/vehicles/mp_peaks-jeep-hits-side-uo.txt"),
        &log,
    );
}

/// The same from the front: 25.
#[test]
fn a_jeep_takes_the_rounds_its_front_stops() {
    let Some(log) = ours("mp_peaks", "willyjeep_mp", &[("probe_dir", "90")]) else {
        return;
    };
    compare(
        include_str!("fixtures/vehicles/mp_peaks-jeep-hits-front-uo.txt"),
        &log,
    );
}

/// The same from behind: 18, 18.75 truncated.
#[test]
fn a_jeep_takes_the_rounds_its_rear_stops() {
    let Some(log) = ours("mp_peaks", "willyjeep_mp", &[("probe_dir", "-90")]) else {
        return;
    };
    compare(
        include_str!("fixtures/vehicles/mp_peaks-jeep-hits-rear-uo.txt"),
        &log,
    );
}

/// mp_cassino's first Panzer IV: its side stops the trace and the rounds,
/// and takes none of them (`bulletDamage` 0).
#[test]
fn a_tank_stops_rounds_and_takes_none() {
    let Some(log) = ours("mp_cassino", "panzeriv_mp", &[]) else {
        return;
    };
    compare(
        include_str!("fixtures/vehicles/mp_cassino-tank-hits-side-uo.txt"),
        &log,
    );
}
