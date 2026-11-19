//! A United Offensive tank's shell on another vehicle, ours against
//! retail's. `client-probes/probe_vehicle_shell` seats client 0 in
//! mp_kharkov's first Panzer IV, turns its view onto the Elefant 276 units
//! off and fires the gun with `fireTurret` every 4 s from 3 s into the
//! seat. The fixtures are retail UO 1.51b's: the probe's log
//! (`mp_kharkov-tank-shell-uo.txt`, the Elefant's `damage` notifies) and
//! the driver's snapshots (`*-uo-shellveh.bin`, the explodes). The shell's
//! direct hit is the Elefant's only notify: `G_MissileImpact`'s `G_Damage`
//! with the driver as the attacker and the tank as the inflictor, and the
//! splash skips what it struck (docs/research/coduo-vehicles.md 13).
//!
//! `SHELL_REPORT=1` prints both sides. Needs `COD_DIR` at a UO install;
//! without one it returns early.

use crate::common;

use common::{Join, Queues, step};
use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;
use vcod_common::net::NetEvent;
use vcod_common::net::event_ids::EV_ROCKET_EXPLODE_NOMARKS;
use vcod_common::net::events::{EventTracker, GameEvent};
use vcod_common::net::msg::{BUTTON_USE, UserCmd};
use vcod_common::net::protocol::PROTOCOL_V22;

const MAP: &str = "mp_kharkov";
/// `vehicle_probe.rs` `SIT`, from the placement: on foot, the use tap,
/// the driver's seat for 12 s.
const SETTLE_MS: i64 = 10_000;
const TAP_MS: i64 = 150;
const SEAT_MS: i64 = 12_000;

/// Explodes with the server time of the snapshot that brought each.
type Explodes = Vec<(i32, GameEvent)>;

/// Ours: the log and every explode our snapshots carried.
fn ours() -> Option<(Vec<String>, Explodes)> {
    let fs = vcod_common::testing::uo_fs()?;
    let p = &PROTOCOL_V22;
    let (mut sv, mut now) = common::Boot {
        proto: Some(&PROTOCOL_V22),
        cvars: &[("probe_vehicletype", "panzeriv_mp")],
        script: Some(include_str!(
            "../../gsc/tests/fixtures/semantics/client-probes/probe_vehicle_shell.gsc"
        )),
        ..common::Boot::new(MAP, "probe_vehicle_shell")
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
    let mut tracker = EventTracker::new();
    let mut explodes = Vec::new();
    let mut seen_time = 0;
    for i in 0..3000i64 {
        now += frame;
        let since = placed_at.map(|at| (i - at) * common::FRAME_MS);
        if since.is_some_and(|ms| ms >= SETTLE_MS + TAP_MS + SEAT_MS) {
            break;
        }
        let tap = since.is_some_and(|ms| (SETTLE_MS..SETTLE_MS + TAP_MS).contains(&ms));
        // The probe holds the view from the use tap on.
        if tap {
            hold.follow = false;
        }
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
        if let Some(s) = cl.snapshots().newest()
            && s.server_time.raw() != seen_time
        {
            seen_time = s.server_time.raw();
            for ev in tracker.drain(s, p) {
                if ev.event == EV_ROCKET_EXPLODE_NOMARKS
                    || ev.event == vcod_common::net::event_ids::EV_GRENADE_EXPLODE
                {
                    explodes.push((s.server_time.raw(), ev));
                }
            }
        }
        if placed_at.is_none() && sv.script_log().iter().any(|l| l.contains("PROBE place")) {
            placed_at = Some(i);
        }
    }
    Some((sv.script_log().to_vec(), explodes))
}

/// The retail capture's explodes in 1.1's numbering.
fn retail_explodes() -> Explodes {
    let p = &PROTOCOL_V22;
    let frames = vcod_common::testing::fixture_frames(
        p,
        "net/gamestate-uo-shellveh.bin",
        "net/snapshots-uo-shellveh.bin",
    );
    let mut tracker = EventTracker::new();
    let mut out = Vec::new();
    for f in &frames {
        for ev in tracker.drain(&f.snapshot, p) {
            if ev.event == EV_ROCKET_EXPLODE_NOMARKS
                || ev.event == vcod_common::net::event_ids::EV_GRENADE_EXPLODE
            {
                out.push((f.snapshot.server_time.raw(), ev));
            }
        }
    }
    out
}

/// A `PROBE vdamage` line's amount, means of death, point, direction,
/// attacker and inflictor.
type Notify = (i32, String, [f32; 3], [f32; 3], i32, i32);

fn vec3(s: &str) -> [f32; 3] {
    let v: Vec<f32> = s
        .trim_matches(|c| c == '(' || c == ')')
        .split(',')
        .map(|x| x.trim().parse().unwrap())
        .collect();
    [v[0], v[1], v[2]]
}

/// The `PROBE <tag>` lines, in log order.
fn notifies(log: &[String], tag: &str) -> Vec<Notify> {
    let prefix = format!("PROBE {tag} ");
    log.iter()
        .filter_map(|l| l.trim().strip_prefix(prefix.as_str()).map(str::to_string))
        .map(|l| {
            // `<time> <amount> <mod> (x, y, z) (x, y, z) <attacker> <inflictor> <health>`
            let (head, rest) = l.split_once(" (").unwrap();
            let h: Vec<&str> = head.split(' ').collect();
            let (point, rest) = rest.split_once(") (").unwrap();
            let (dir, tail) = rest.split_once(") ").unwrap();
            let t: Vec<&str> = tail.split(' ').collect();
            (
                h[1].parse().unwrap(),
                h[2].to_string(),
                vec3(point),
                vec3(dir),
                t[0].parse().unwrap(),
                t[1].parse().unwrap(),
            )
        })
        .collect()
}

#[test]
fn a_shell_strikes_a_vehicle_as_on_uo() {
    let Some((log, ours_ex)) = ours() else {
        return;
    };
    let fixture = include_str!("fixtures/vehicles/mp_kharkov-tank-shell-uo.txt");
    let retail: Vec<String> = fixture.lines().map(str::to_string).collect();
    let retail_ex = retail_explodes();
    if std::env::var_os("SHELL_REPORT").is_some() {
        for l in log.iter().filter(|l| l.contains("PROBE")) {
            eprintln!("ours   {}", l.trim());
        }
        for (t, e) in &ours_ex {
            eprintln!(
                "ours   t {t} ev {} parm {} surf {} scale {} weapon {} eFlags {:#x} pos {:?}",
                e.event, e.parm, e.surf_type, e.scale, e.weapon, e.eflags, e.pos
            );
        }
        for (t, e) in &retail_ex {
            eprintln!(
                "retail t {t} ev {} parm {} surf {} scale {} weapon {} eFlags {:#x} pos {:?}",
                e.event, e.parm, e.surf_type, e.scale, e.weapon, e.eflags, e.pos
            );
        }
    }
    let order = |log: &[String]| -> Vec<String> {
        log.iter()
            .filter_map(|l| l.trim().strip_prefix("PROBE "))
            .filter(|l| l.starts_with("vdamage ") || l.starts_with("selfdamage "))
            .map(|l| l.split(' ').take(2).collect::<Vec<_>>().join(" "))
            .collect()
    };
    let rorder = order(&retail);
    let oorder: Vec<String> = order(&log);
    // Times past the first fire: ours seats on its own clock.
    let rebase = |v: Vec<String>| -> Vec<(String, i32)> {
        let t0 = v
            .first()
            .map_or(0, |l| l.split(' ').nth(1).unwrap().parse::<i32>().unwrap());
        v.iter()
            .map(|l| {
                let (tag, t) = l.split_once(' ').unwrap();
                (tag.to_string(), t.parse::<i32>().unwrap() - t0)
            })
            .collect()
    };
    assert_eq!(
        rebase(oorder),
        rebase(rorder),
        "the notifies' order and times past the first"
    );
    for tag in ["vdamage", "selfdamage"] {
        compare(&notifies(&retail, tag), &notifies(&log, tag), tag);
    }
    assert_eq!(ours_ex.len(), retail_ex.len(), "the explodes");
    for ((_, o), (_, r)) in ours_ex.iter().zip(&retail_ex) {
        assert_eq!(
            (o.event, o.parm, o.surf_type, o.scale, o.weapon, o.eflags),
            (r.event, r.parm, r.surf_type, r.scale, r.weapon, r.eflags),
            "the explode on the hull"
        );
    }
}

/// One vehicle's notifies, retail's against ours. The splash's point is the
/// blast 4 off the struck face, which ours puts up to a unit off retail's
/// (docs/research/coduo-vehicles.md 13.3, "The firing vehicle's own splash").
fn compare(rn: &[Notify], on: &[Notify], tag: &str) {
    assert_eq!(on.len(), rn.len(), "{tag}: retail {rn:?}, ours {on:?}");
    for (o, r) in on.iter().zip(rn) {
        assert_eq!(
            (o.0, &o.1, o.4, o.5),
            (r.0, &r.1, r.4, r.5),
            "amount, means, attacker, inflictor"
        );
        for i in 0..3 {
            assert!(
                (o.2[i] - r.2[i]).abs() < 4.0,
                "point: retail {:?}, ours {:?}",
                r.2,
                o.2
            );
            assert!(
                (o.3[i] - r.3[i]).abs() < 0.05,
                "dir: retail {:?}, ours {:?}",
                r.3,
                o.3
            );
        }
    }
}
