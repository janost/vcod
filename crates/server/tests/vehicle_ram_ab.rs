//! A United Offensive vehicle driven into another, ours against retail's.
//! `client-probes/probe_vehicle_ram` seats client 0 in mp_kharkov's second
//! Horch, which `--probe-vehicle ram` drives straight ahead for 6 s into
//! the Panzer IV parked 680 units in front; the probe logs both vehicles'
//! poses every frame (`mp_kharkov-jeep-ram-uo.txt`, retail UO 1.51b). The
//! Horch glances off the tank's collmap twice and shoves it a few units
//! (0x827ab's `script_vehicle` arm, docs/research/coduo-vehicles.md 12).
//! Ours matches both shoves think for think: the second lands only because
//! the parked tank's wheel traces end a last bit apart from think to think,
//! as United Offensive's terrain clip rounds them.
//!
//! `mp_kharkov-jeep-ram-link-uo.txt` repeats the ram with a script_model
//! linked over the tank (`probe_link`): the tank's think runs on its own
//! turn in the entity pass, so the model, numbered after it, follows it in
//! the same frame (coduo-vehicles.md 12, "The think's turn").
//!
//! `RAM_REPORT=1` prints both sides' poses, `RAM_DUMP=<path>` writes ours'
//! PROBE lines. Needs `COD_DIR` at a UO install; without one it returns
//! early.

use crate::common;

use common::{Join, Queues, step};
use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;
use vcod_common::net::NetEvent;
use vcod_common::net::msg::{BUTTON_USE, UserCmd};
use vcod_common::net::protocol::PROTOCOL_V22;

const MAP: &str = "mp_kharkov";
/// `vehicle_probe.rs` `RAM`, from the placement.
const SETTLE_MS: i64 = 2000;
const TAP_MS: i64 = 150;
const SEATED_MS: i64 = 2000;
const AHEAD_MS: i64 = 6000;
const STOP_MS: i64 = 3000;

fn ours(extra: &[(&str, &str)]) -> Option<Vec<String>> {
    let fs = vcod_common::testing::uo_fs()?;
    let mut cvars = vec![
        ("probe_vehicletype", "horch_mp"),
        ("probe_index", "1"),
        ("probe_ahead", "680"),
    ];
    cvars.extend_from_slice(extra);
    let (mut sv, mut now) = common::Boot {
        proto: Some(&PROTOCOL_V22),
        cvars: &cvars,
        script: Some(include_str!(
            "../../gsc/tests/fixtures/semantics/client-probes/probe_vehicle_ram.gsc"
        )),
        ..common::Boot::new(MAP, "probe_vehicle_ram")
    }
    .start(fs);
    common::run_alone(&mut sv, &mut now, common::PROBE_JOIN_MS);
    let q = Rc::new(RefCell::new(Queues::default()));
    let mut cl =
        vcod_common::net::NetClient::start_with_qport(common::ClientEnd(q.clone()), now, 0x2001);
    let mut join = Join::new("axis", "kar98k_mp");
    let frame = Duration::from_millis(common::FRAME_MS as u64);
    let mut hold = common::ProbeHold {
        follow: true,
        ..Default::default()
    };
    let mut placed_at: Option<i64> = None;
    for i in 0..3000i64 {
        now += frame;
        let since = placed_at.map(|at| (i - at) * common::FRAME_MS);
        let ahead = SETTLE_MS + TAP_MS + SEATED_MS;
        if since.is_some_and(|ms| ms >= ahead + AHEAD_MS + STOP_MS) {
            break;
        }
        let tap = since.is_some_and(|ms| (SETTLE_MS..SETTLE_MS + TAP_MS).contains(&ms));
        if tap {
            hold.follow = false;
        }
        hold.forward = if since.is_some_and(|ms| (ahead..ahead + AHEAD_MS).contains(&ms)) {
            127
        } else {
            0
        };
        if placed_at.is_some() && cl.state() == vcod_common::net::NetState::Active {
            let buttons = if tap { BUTTON_USE } else { 0 };
            cl.send_cmds(&hold.cmds(&cl, buttons));
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

fn vec3(s: &str) -> [f32; 3] {
    let v: Vec<f32> = s
        .trim_matches(|c| c == '(' || c == ')')
        .split(',')
        .map(|x| x.trim().parse().unwrap())
        .collect();
    [v[0], v[1], v[2]]
}

/// One vehicle's `(time past the placement, origin, angles)` lines.
fn poses(log: &[String], which: &str) -> Vec<(i32, [f32; 3], [f32; 3])> {
    let placed: i32 = log
        .iter()
        .find_map(|l| l.trim().strip_prefix("PROBE place "))
        .and_then(|r| r.split(' ').next()?.parse().ok())
        .expect("the driver was placed");
    log.iter()
        .filter_map(|l| {
            l.trim()
                .strip_prefix(&format!("PROBE {which} "))
                .map(str::to_string)
        })
        .map(|l| {
            let (t, rest) = l.split_once(" (").unwrap();
            let (o, rest) = rest.split_once(") (").unwrap();
            let a = rest.split(')').next().unwrap();
            (t.parse::<i32>().unwrap() - placed, vec3(o), vec3(a))
        })
        .collect()
}

/// The struck tank's `(time past the placement, speed)` per think.
fn tank_speeds(log: &[String]) -> Vec<(i32, f32)> {
    let placed: i32 = log
        .iter()
        .find_map(|l| l.trim().strip_prefix("PROBE place "))
        .and_then(|r| r.split(' ').next()?.parse().ok())
        .expect("the driver was placed");
    log.iter()
        .filter_map(|l| l.trim().strip_prefix("PROBE b "))
        .map(|l| {
            let t: i32 = l.split(' ').next().unwrap().parse().unwrap();
            let v: f32 = l.rsplit(' ').next().unwrap().parse().unwrap();
            (t - placed, v)
        })
        .collect()
}

/// The time past the placement of the driver's first hit: the first think
/// whose `speed` falls under half the last one's.
fn first_hit(log: &[String]) -> i32 {
    let placed: i32 = log
        .iter()
        .find_map(|l| l.trim().strip_prefix("PROBE place "))
        .and_then(|r| r.split(' ').next()?.parse().ok())
        .expect("the driver was placed");
    let speeds: Vec<(i32, f32)> = log
        .iter()
        .filter_map(|l| l.trim().strip_prefix("PROBE a "))
        .map(|l| {
            let t: i32 = l.split(' ').next().unwrap().parse().unwrap();
            let v: f32 = l.rsplit(' ').next().unwrap().parse().unwrap();
            (t - placed, v)
        })
        .collect();
    speeds
        .windows(2)
        .find(|w| w[0].1 > 100.0 && w[1].1 < 0.5 * w[0].1)
        .map(|w| w[1].0)
        .expect("the driver struck something")
}

#[test]
fn a_jeep_driven_into_a_tank_shoves_it_as_on_uo() {
    let Some(log) = ours(&[]) else {
        return;
    };
    let fixture = include_str!("fixtures/vehicles/mp_kharkov-jeep-ram-uo.txt");
    let retail: Vec<String> = fixture.lines().map(str::to_string).collect();
    let (ra, oa) = (poses(&retail, "a"), poses(&log, "a"));
    let (rb, ob) = (poses(&retail, "b"), poses(&log, "b"));
    if let Some(path) = std::env::var_os("RAM_DUMP") {
        std::fs::write(path, log.join("\n")).expect("write RAM_DUMP");
    }
    if std::env::var_os("RAM_REPORT").is_some() {
        for (r, o) in ra.iter().zip(&oa).step_by(5) {
            eprintln!("a retail {:?} ours {:?}", r, o);
        }
        for (r, o) in rb.iter().zip(&ob).step_by(5) {
            eprintln!("b retail {:?} ours {:?}", r, o);
        }
    }
    let end = |v: &[(i32, [f32; 3], [f32; 3])]| v.last().copied().expect("poses");
    // The tank parked, then shoved: how far it went.
    let moved = |v: &[(i32, [f32; 3], [f32; 3])]| {
        let (a, b) = (v[0].1, end(v).1);
        ((b[0] - a[0]).powi(2) + (b[1] - a[1]).powi(2)).sqrt()
    };
    // The tank's own think runs in the entity pass, ahead of the driver's
    // step: it first moves on the think after the hit (retail: hit at
    // 24250, the tank off at 24300).
    let shoved = |v: &[(i32, [f32; 3], [f32; 3])]| {
        v.iter()
            .find(|p| p.1 != v[0].1)
            .map(|p| p.0)
            .expect("the tank moved")
    };
    let (rl, ol) = (
        shoved(&rb) - first_hit(&retail),
        shoved(&ob) - first_hit(&log),
    );
    assert_eq!(
        ol, rl,
        "the tank's first move past the hit: retail {rl}, ours {ol}"
    );
    // The first hit's shove, settled two thinks later: the whole pushed
    // velocity on the first think, less the tank's accel on the second.
    let settled = |v: &[(i32, [f32; 3], [f32; 3])], at: i32| {
        v.iter()
            .find(|p| p.0 >= at)
            .expect("a pose past the shove")
            .1
    };
    let (rs, os) = (
        settled(&rb, first_hit(&retail) + 150),
        settled(&ob, first_hit(&log) + 150),
    );
    assert!(
        (0..3).all(|i| (rs[i] - os[i]).abs() < 0.05),
        "the first shove: retail {rs:?}, ours {os:?}"
    );
    let (rm, om) = (moved(&rb), moved(&ob));
    assert!(rm > 1.0, "retail's tank moved {rm}");
    // Think for think from the first hit on, through the second: the
    // tank's pose to 0.02 and its speed to 0.01 (the pushed velocity, then
    // less its accel).
    let (rh, oh) = (first_hit(&retail), first_hit(&log));
    let (rs, os) = (tank_speeds(&retail), tank_speeds(&log));
    let upto = second_hit(&retail) - rh + 1000;
    let mut checked = 0;
    for (r, rv) in rs.iter().filter(|(t, _)| (0..upto).contains(&(t - rh))) {
        let at = r - rh + oh;
        let o = ob
            .iter()
            .find(|p| p.0 == at)
            .expect("ours logged the think");
        let p = rb
            .iter()
            .find(|p| p.0 == *r)
            .expect("retail logged the think");
        let ov = os.iter().find(|s| s.0 == at).expect("ours' speed").1;
        assert!(
            (0..3).all(|i| (p.1[i] - o.1[i]).abs() < 0.02) && (rv - ov).abs() < 0.01,
            "the tank {} ms past the first hit: retail {:?} {rv}, ours {:?} {ov}",
            r - rh,
            p.1,
            o.1
        );
        checked += 1;
    }
    assert!(checked > 25, "only {checked} thinks past the first hit");
    // The second shove lands: retail's total, which the first alone falls
    // a unit short of.
    let first = ((rs_pose(&rb, rh + 150)[0] - rb[0].1[0]).powi(2)
        + (rs_pose(&rb, rh + 150)[1] - rb[0].1[1]).powi(2))
    .sqrt();
    assert!(
        rm > first + 0.5 && (om - rm).abs() < 0.01,
        "the shove past the second hit: retail {rm}, ours {om}, the first {first}"
    );
}

/// A pose's origin at or past `at`.
fn rs_pose(v: &[(i32, [f32; 3], [f32; 3])], at: i32) -> [f32; 3] {
    v.iter()
        .find(|p| p.0 >= at)
        .expect("a pose past the time")
        .1
}

/// The driver's `(time past the placement, speed)` per think.
fn log_speeds(log: &[String]) -> Vec<(i32, f32)> {
    let placed: i32 = log
        .iter()
        .find_map(|l| l.trim().strip_prefix("PROBE place "))
        .and_then(|r| r.split(' ').next()?.parse().ok())
        .expect("the driver was placed");
    log.iter()
        .filter_map(|l| l.trim().strip_prefix("PROBE a "))
        .map(|l| {
            let t: i32 = l.split(' ').next().unwrap().parse().unwrap();
            let v: f32 = l.rsplit(' ').next().unwrap().parse().unwrap();
            (t - placed, v)
        })
        .collect()
}

/// The driver's second hit: the first think past the first hit whose speed
/// falls under half the last one's.
fn second_hit(log: &[String]) -> i32 {
    let first = first_hit(log);
    log_speeds(log)
        .windows(2)
        .find(|w| w[1].0 > first + 100 && w[1].1 < 0.5 * w[0].1)
        .map(|w| w[1].0)
        .expect("the driver struck twice")
}

/// The linked model's `(time past the placement, origin)` per frame.
fn link_origins(log: &[String]) -> Vec<(i32, [f32; 3])> {
    let placed: i32 = log
        .iter()
        .find_map(|l| l.trim().strip_prefix("PROBE place "))
        .and_then(|r| r.split(' ').next()?.parse().ok())
        .expect("the driver was placed");
    log.iter()
        .filter_map(|l| l.trim().strip_prefix("PROBE l "))
        .map(|l| {
            let v: Vec<f32> = l.split(' ').map(|x| x.parse().unwrap()).collect();
            (v[0] as i32 - placed, [v[1], v[2], v[3]])
        })
        .collect()
}

#[test]
fn a_model_linked_to_a_shoved_tank_moves_on_the_tanks_frame() {
    let Some(log) = ours(&[("probe_link", "1")]) else {
        return;
    };
    let fixture = include_str!("fixtures/vehicles/mp_kharkov-jeep-ram-link-uo.txt");
    let retail: Vec<String> = fixture.lines().map(str::to_string).collect();
    let (rl, ol) = (link_origins(&retail), link_origins(&log));
    let (rh, oh) = (first_hit(&retail), first_hit(&log));
    let upto = second_hit(&retail) - rh + 1000;
    let mut checked = 0;
    for (t, r) in rl.iter().filter(|(t, _)| (-200..upto).contains(&(t - rh))) {
        let at = t - rh + oh;
        let o = ol
            .iter()
            .find(|p| p.0 == at)
            .expect("ours logged the frame")
            .1;
        assert!(
            (0..3).all(|i| (r[i] - o[i]).abs() < 0.02),
            "the linked model {} ms past the first hit: retail {r:?}, ours {o:?}",
            t - rh
        );
        checked += 1;
    }
    assert!(checked > 25, "only {checked} frames past the first hit");
}
