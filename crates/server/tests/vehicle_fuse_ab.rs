//! A United Offensive tank shell that strikes nothing, ours against
//! retail's. `client-probes/probe_shellfuse` first calls `fireTurret` on
//! mp_kharkov's empty Panzer IV, then seats client 0 in it and fires the
//! gun twice, pinning each shell in the air with an `origin` write every
//! frame: the first over the Elefant, the second over the Panzer itself,
//! with client 0 moved to its second seat by then. Each goes off on
//! `fire_rocket`'s 15 s fuse (`G_ExplodeMissile`), and its splash names
//! the vehicle as the attacker, on the Elefant, on the Panzer and on the
//! rider. The fixtures are retail UO 1.51b's: the probe's log
//! (`mp_kharkov-shell-fuse-uo.txt`) and the rider's snapshots
//! (`*-uo-shellfuse.bin`, the explodes). docs/research/cod11-combat.md
//! 13.7.
//!
//! `FUSE_REPORT=1` prints both sides. Needs `COD_DIR` at a UO install;
//! without one it returns early.

use crate::common;

use common::{Join, Queues, step};
use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;
use vcod_common::net::NetEvent;
use vcod_common::net::events::{EventTracker, GameEvent};
use vcod_common::net::msg::{BUTTON_USE, UserCmd};
use vcod_common::net::protocol::PROTOCOL_V22;

const MAP: &str = "mp_kharkov";
/// `vehicle_probe.rs` `SIT`, from the placement: on foot, the use tap,
/// the driver's seat for 12 s, `nextvehslot` and the second seat, which
/// holds client 0 past both fuses.
const SETTLE_MS: i64 = 10_000;
const TAP_MS: i64 = 150;
const SEAT_MS: i64 = 12_000;
const SEAT_B_MS: i64 = 12_000;

/// Events raised on a shell's own entity, with the server time of the
/// snapshot that brought each.
type Explodes = Vec<(i32, GameEvent)>;

/// The pinned shells' numbers, off the `PROBE shell` lines.
fn shell_numbers(log: &[String]) -> Vec<u32> {
    log.iter()
        .filter_map(|l| l.trim().strip_prefix("PROBE shell "))
        .filter_map(|l| l.split(' ').nth(2)?.parse().ok())
        .collect()
}

/// The events on the shells a probe log names, out of `frames`.
fn shell_events<'a>(
    log: &[String],
    frames: impl Iterator<Item = &'a vcod_common::net::snapshot::Snapshot>,
) -> Explodes {
    let p = &PROTOCOL_V22;
    let shells = shell_numbers(log);
    let mut tracker = EventTracker::new();
    let mut out = Vec::new();
    for s in frames {
        for ev in tracker.drain(s, p) {
            if shells.contains(&ev.entity_num) {
                out.push((s.server_time.raw(), ev));
            }
        }
    }
    out
}

/// Ours: the log and every snapshot the rider received.
fn ours() -> Option<(Vec<String>, Vec<vcod_common::net::snapshot::Snapshot>)> {
    let fs = vcod_common::testing::uo_fs()?;
    let (mut sv, mut now) = common::Boot {
        proto: Some(&PROTOCOL_V22),
        cvars: &[("probe_vehicletype", "panzeriv_mp")],
        script: Some(include_str!(
            "../../gsc/tests/fixtures/semantics/client-probes/probe_shellfuse.gsc"
        )),
        ..common::Boot::new(MAP, "probe_shellfuse")
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
    let mut snaps = Vec::new();
    let mut seen_time = 0;
    let mut said = false;
    for i in 0..4000i64 {
        now += frame;
        let since = placed_at.map(|at| (i - at) * common::FRAME_MS);
        if since.is_some_and(|ms| ms >= SETTLE_MS + TAP_MS + SEAT_MS + SEAT_B_MS) {
            break;
        }
        let tap = since.is_some_and(|ms| (SETTLE_MS..SETTLE_MS + TAP_MS).contains(&ms));
        // The probe holds the view from the use tap on.
        if tap {
            hold.follow = false;
        }
        if !said && since.is_some_and(|ms| ms >= SETTLE_MS + TAP_MS + SEAT_MS) {
            said = true;
            cl.send_reliable("nextvehslot");
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
            snaps.push(s.clone());
        }
        if placed_at.is_none() && sv.script_log().iter().any(|l| l.contains("PROBE place")) {
            placed_at = Some(i);
        }
    }
    Some((sv.script_log().to_vec(), snaps))
}

/// A timed `PROBE` line's time and where it sits among the words:
/// `vdamage <who> <time>`, every other timed line `<kind> <time>`.
fn time_of(l: &str) -> Option<(usize, i32)> {
    let w: Vec<&str> = l.split(' ').collect();
    let at = if w[0] == "vdamage" { 2 } else { 1 };
    Some((at, w.get(at)?.parse().ok()?))
}

/// The absolute times of the log's `PROBE <kind>` lines.
fn times(log: &[String], kind: &str) -> Vec<i32> {
    log.iter()
        .filter_map(|l| l.trim().strip_prefix("PROBE "))
        .filter(|l| l.starts_with(kind))
        .filter_map(|l| Some(time_of(l)?.1))
        .collect()
}

/// The log's `PROBE` lines as `(time, rest)`, the time cut from the words
/// and taken past the last anchor before it: the placement, then the
/// first seat (retail's client joined later than ours, and its use tap
/// landed a frame off ours). Lines before the placement keep the map's
/// clock. Each pinned shell's number reads `shell<k>`: the two servers
/// number their temp entities apart.
fn rebased(log: &[String]) -> Vec<(i32, String)> {
    let lines: Vec<&str> = log
        .iter()
        .filter_map(|l| l.trim().strip_prefix("PROBE "))
        .collect();
    let shells: Vec<(String, String)> = lines
        .iter()
        .filter_map(|l| l.strip_prefix("shell "))
        .filter_map(|l| {
            let w: Vec<&str> = l.split(' ').collect();
            Some((w.get(2)?.to_string(), format!("shell{}", w.get(1)?)))
        })
        .collect();
    let mut base = 0;
    let mut seated = false;
    let mut out = Vec::new();
    for l in lines {
        let untimed = l.starts_with("vehicle ") || l.starts_with("target ");
        let Some((at, t)) = time_of(l).filter(|_| !untimed) else {
            out.push((0, l.to_string()));
            continue;
        };
        if l.starts_with("place ") || (l.starts_with("seat ") && !seated) {
            seated |= l.starts_with("seat ");
            base = t;
        }
        let mut w: Vec<String> = l.split(' ').map(str::to_string).collect();
        w.remove(at);
        for word in &mut w {
            if let Some((_, name)) = shells.iter().find(|(n, _)| n == word) {
                *word = name.clone();
            }
        }
        out.push((t - base, w.join(" ")));
    }
    out
}

fn vecs(s: &str) -> Vec<[f32; 3]> {
    s.split('(')
        .skip(1)
        .map(|v| {
            let v: Vec<f32> = v
                .split(')')
                .next()
                .unwrap()
                .split(',')
                .map(|x| x.trim().parse().unwrap())
                .collect();
            [v[0], v[1], v[2]]
        })
        .collect()
}

/// `rest` with its vectors blanked, for the exact part of a comparison.
fn words(s: &str) -> String {
    let mut out = String::new();
    let mut depth = 0;
    for c in s.chars() {
        match c {
            '(' => {
                depth += 1;
                out.push_str("()");
            }
            ')' => depth -= 1,
            _ if depth == 0 => out.push(c),
            _ => {}
        }
    }
    out
}

#[test]
fn a_shell_that_strikes_nothing_goes_off_on_its_fuse_as_on_uo() {
    let Some((log, snaps)) = ours() else {
        return;
    };
    let fixture = include_str!("fixtures/vehicles/mp_kharkov-shell-fuse-uo.txt");
    let retail: Vec<String> = fixture
        .lines()
        .filter(|l| !l.starts_with('#'))
        .map(str::to_string)
        .collect();
    let frames = vcod_common::testing::fixture_frames(
        &PROTOCOL_V22,
        "net/gamestate-uo-shellfuse.bin",
        "net/snapshots-uo-shellfuse.bin",
    );
    let retail_ex = shell_events(&retail, frames.iter().map(|f| &f.snapshot));
    let ours_ex = shell_events(&log, snaps.iter());
    let (r, o) = (rebased(&retail), rebased(&log));
    if std::env::var_os("FUSE_REPORT").is_some() {
        for l in &r {
            eprintln!("retail {l:?}");
        }
        for l in &o {
            eprintln!("ours   {l:?}");
        }
        for (side, ex) in [("retail", &retail_ex), ("ours", &ours_ex)] {
            for (t, e) in ex {
                eprintln!(
                    "{side} t {t} on {} ev {} parm {} surf {} scale {} weapon {} eFlags {:#x} pos {:?}",
                    e.entity_num, e.event, e.parm, e.surf_type, e.scale, e.weapon, e.eflags, e.pos
                );
            }
        }
    }
    // Every line up to the last splash: the probe's client sits on in the
    // seat cycle after this replay stops.
    let last_hit = r
        .iter()
        .filter(|(_, l)| l.starts_with("vdamage") || l.starts_with("hit"))
        .map(|(t, _)| *t)
        .max()
        .expect("retail's splashes");
    let cut = |v: &[(i32, String)]| -> Vec<(i32, String)> {
        v.iter().filter(|(t, _)| *t <= last_hit).cloned().collect()
    };
    let (r, o) = (cut(&r), cut(&o));
    assert_eq!(
        o.len(),
        r.len(),
        "the PROBE lines: retail {r:#?}, ours {o:#?}"
    );
    for ((rt, rl), (ot, ol)) in r.iter().zip(&o) {
        assert_eq!(
            (ot, words(ol)),
            (rt, words(rl)),
            "the line's time past its anchor and its words"
        );
        for (rv, ov) in vecs(rl).iter().zip(vecs(ol)) {
            for i in 0..3 {
                assert!(
                    (rv[i] - ov[i]).abs() <= 0.02,
                    "{rl}: retail {rv:?}, ours {ov:?}"
                );
            }
        }
    }
    assert_eq!(ours_ex.len(), retail_ex.len(), "the shells' events");
    let (rf, of) = (times(&retail, "fire "), times(&log, "fire "));
    for (k, ((ot, oe), (rt, re))) in ours_ex.iter().zip(&retail_ex).enumerate() {
        assert_eq!(
            (
                oe.event,
                oe.parm,
                oe.surf_type,
                oe.scale,
                oe.weapon,
                oe.eflags
            ),
            (
                re.event,
                re.parm,
                re.surf_type,
                re.scale,
                re.weapon,
                re.eflags
            ),
            "the fuse's explode"
        );
        assert_eq!(
            ot - of[k],
            rt - rf[k],
            "the explode's snapshot past its fire"
        );
        for i in 0..3 {
            assert!((oe.pos[i] - re.pos[i]).abs() <= 1.0, "the explode's place");
        }
    }
}
