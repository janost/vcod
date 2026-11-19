//! A `spawnTriggerMount` on a United Offensive server against retail's
//! capture of the same mount: `mp_foy-dm-slope-8ms-uofoot-mount.txt`, a
//! `--save-slope --probe-uofoot` client holding `mg30cal_mp` in front of
//! `client-probes/probe_uofoot`'s crouched script trigger
//! (docs/research/coduo-movement.md 11). Ours runs the same probe gametype,
//! replays the capture's cmds from the placement up to its frag switch, and
//! compares the run of states the snapshots carried: `pm_flags` (the
//! bipod's sight bit and the stance), `serverCursorHint` and its value.
//!
//! The `-mount-fires` capture of the same script under `probe_fires 1`
//! logs each `"trigger"` the mount raised (`mp_foy-uofoot-mount-fires-uo.txt`):
//! one per cmd the server ran, which ours has to match.
//!
//! `MOUNT_REPORT=1` prints both runs. Needs `COD_DIR` at a UO install;
//! without one it returns early.

use crate::common;

use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};
use vcod_common::net::msg::{NULL_USERCMD, UserCmd};
use vcod_common::net::protocol::PROTOCOL_V22;

const MAP: &str = "mp_foy";
const PROBE: &str = "probe_uofoot";
/// `probe_uofoot`'s spots, `probe_spot` and where it puts the body.
const STAND: (&str, [f32; 2]) = ("stand", [7.5, 2686.0]);
const CROUCH: (&str, [f32; 2]) = ("crouch", [-1094.0, -3293.0]);
const MOUNT: (&str, [f32; 2]) = ("mount", [-100.0, 2590.0]);

/// What one snapshot said about the mount.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct State {
    pm_flags: i32,
    hint: i32,
    val: i32,
}

/// One `!cmd` line: its serverTime, the view it asked for in absolute
/// shorts (the wire angles plus the `delta_angles` of the snapshot before
/// it) and the rest of the cmd.
struct Cmd {
    st: i32,
    view: [i32; 3],
    cmd: UserCmd,
}

fn field<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    line.split_whitespace()
        .find_map(|w| w.strip_prefix(key)?.strip_prefix('='))
}

fn wire_field(line: &str, key: &str) -> i32 {
    field(line, "wire")
        .into_iter()
        .flat_map(|w| w.split(','))
        .find_map(|kv| kv.strip_prefix(key)?.strip_prefix(':')?.parse().ok())
        .unwrap_or(0)
}

fn ints<const N: usize>(s: &str) -> [i32; N] {
    let v: Vec<i32> = s.split(',').map(|x| x.parse().unwrap()).collect();
    std::array::from_fn(|i| v[i])
}

/// Retail's states, one per snapshot, and the cmds up to the first that
/// asks for another weapon.
fn retail(spot: &str) -> (Vec<State>, Vec<Cmd>) {
    let (states, cmds, _) = retail_capture(spot);
    (states, cmds)
}

/// [`retail`] off the capture tagged `uofoot-<tag>`, with each snapshot's
/// `(t, ct)`.
fn retail_capture(tag: &str) -> (Vec<State>, Vec<Cmd>, Vec<(i32, i32)>) {
    let path = format!("tests/fixtures/playerstate/mp_foy-dm-slope-8ms-uofoot-{tag}.txt");
    let text = std::fs::read_to_string(path).expect("read the fixture");
    let mut snaps = Vec::new();
    let mut states = Vec::new();
    let mut cmds = Vec::new();
    let mut da = [0; 3];
    let mut weapon = None;
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("!snap ") {
            da = ints(field(rest, "da").unwrap());
            let num = |k| field(rest, k).unwrap().parse::<i32>().unwrap();
            snaps.push((num("t"), num("ct")));
            states.push(State {
                pm_flags: field(rest, "pm_flags").unwrap().parse().unwrap(),
                hint: wire_field(rest, "serverCursorHint"),
                val: wire_field(rest, "serverCursorHintVal"),
            });
        } else if let Some(rest) = line.strip_prefix("!cmd ") {
            let w: u8 = field(rest, "weapon").unwrap().parse().unwrap();
            if *weapon.get_or_insert(w) != w {
                break;
            }
            let angles: [i32; 3] = ints(field(rest, "angles").unwrap());
            let num = |k| field(rest, k).unwrap().parse::<i32>().unwrap();
            cmds.push(Cmd {
                st: num("st"),
                view: std::array::from_fn(|i| (angles[i] + da[i]) & 0xffff),
                cmd: UserCmd {
                    buttons: num("buttons") as u8,
                    wbuttons: num("wbuttons") as u8,
                    forward: num("forward") as i8,
                    right: num("right") as i8,
                    up: num("up") as i8,
                    ..NULL_USERCMD
                },
            });
        }
    }
    (states, cmds, snaps)
}

/// The probe on ours: joined allies with the 30 cal, placed by the
/// gametype, then the capture's cmds at their own spacing.
fn ours(spot: (&str, [f32; 2]), cmds: &[Cmd]) -> Vec<State> {
    ours_run(spot, cmds, false).0
}

/// [`ours`], and under `fires` (`probe_fires 1`) how many `"trigger"`s
/// the mount raised from the first of `cmds` on.
fn ours_run((spot, at): (&str, [f32; 2]), cmds: &[Cmd], fires: bool) -> (Vec<State>, usize) {
    let fs = vcod_common::testing::uo_fs().unwrap();
    let p = &PROTOCOL_V22;
    let src = format!("../gsc/tests/fixtures/semantics/client-probes/{PROBE}.gsc");
    let probe = std::fs::read_to_string(src).expect("read the probe");
    let mut cvars = vec![("probe_teleport", "1"), ("probe_spot", spot)];
    if fires {
        cvars.push(("probe_fires", "1"));
    }
    let (mut sv, mut now) = common::Boot {
        proto: Some(p),
        cvars: &cvars,
        script: Some(&probe),
        ..common::Boot::new(MAP, PROBE)
    }
    .start(fs);
    common::run_alone(&mut sv, &mut now, common::PROBE_JOIN_MS);
    let q = Rc::new(RefCell::new(common::Queues::default()));
    let mut cl = common::connect(&mut sv, &q, &mut now);
    let mut join = common::Join::new("allies", "mg30cal_mp");
    let mg = vcod_server::defs::UO.weapon_index("mg30cal_mp").unwrap() as u8;
    let state = |s: &vcod_common::net::snapshot::Snapshot| State {
        pm_flags: s.ps.field_i32(p, "pm_flags"),
        hint: s.ps.field_i32(p, "serverCursorHint"),
        val: s.ps.field_i32(p, "serverCursorHintVal"),
    };
    let frame = |sv: &mut vcod_server::Server,
                 cl: &mut vcod_common::net::NetClient<common::ClientEnd>,
                 join: &mut common::Join,
                 now: Instant| {
        for e in common::step(sv, &q, cl, now) {
            if let vcod_common::net::NetEvent::ServerCommand(tokens) = e {
                join.on_server_command(&tokens, cl, now);
            }
        }
    };
    // Until the gametype has put the body on the spot, and a second more
    // for the hint to settle, as retail's capture opens.
    let mut placed_at = None;
    for i in 0..400 {
        now += Duration::from_millis(common::FRAME_MS as u64);
        cl.send_frame(&UserCmd {
            weapon: mg,
            ..NULL_USERCMD
        });
        frame(&mut sv, &mut cl, &mut join, now);
        let on_spot = cl.snapshots().newest().is_some_and(|s| {
            let o = s.ps.origin(p);
            s.ps.field_i32(p, "pm_type") == 0
                && (o[0] - at[0]).abs() < 1.0
                && (o[1] - at[1]).abs() < 1.0
                && s.ps.field_i32(p, "weapon") == mg as i32
        });
        if on_spot && placed_at.is_none() {
            placed_at = Some(i);
        }
        if placed_at.is_some_and(|at| i >= at + 20) {
            break;
        }
    }
    assert!(placed_at.is_some(), "the probe never placed the client");

    let fired = |sv: &vcod_server::Server| {
        sv.script_log()
            .iter()
            .filter(|l| l.starts_with("PROBE fire "))
            .count()
    };
    let fired_before = fired(&sv);
    let mut out = Vec::new();
    let mut next_frame = now + Duration::from_millis(common::FRAME_MS as u64);
    let mut last_st = cmds[0].st;
    for c in cmds {
        now += Duration::from_millis((c.st - last_st).max(0) as u64);
        last_st = c.st;
        while now >= next_frame {
            frame(&mut sv, &mut cl, &mut join, next_frame);
            if let Some(s) = cl.snapshots().newest() {
                out.push(state(s));
            }
            next_frame += Duration::from_millis(common::FRAME_MS as u64);
        }
        cl.pump_at(now);
        cl.send_frame(&UserCmd {
            angles: c.view,
            weapon: mg,
            ..c.cmd
        });
    }
    // The last cmds' notifies go out on the next frame.
    frame(&mut sv, &mut cl, &mut join, next_frame);
    (out, fired(&sv) - fired_before)
}

/// Consecutive repeats folded, anything ahead of the first hint 7 (the
/// placement) dropped, and a state one snapshot long dropped with them:
/// `PmoveSingle` checks the stance before the sight flag, so the first cmd
/// off the bipod still reads ducked and the second stands. Retail's
/// snapshot there carried five such cmds, ours may carry one.
fn runs(states: &[State]) -> Vec<(State, usize)> {
    let fold = |states: &mut dyn Iterator<Item = (State, usize)>| {
        let mut out: Vec<(State, usize)> = Vec::new();
        for (s, n) in states {
            match out.last_mut() {
                Some((last, m)) if *last == s => *m += n,
                _ => out.push((s, n)),
            }
        }
        out
    };
    let all = fold(&mut states.iter().skip_while(|s| s.hint != 7).map(|s| (*s, 1)));
    fold(&mut all.into_iter().filter(|(_, n)| *n > 1))
}

/// Both sides' runs, `MOUNT_REPORT=1` printing them, compared: the same
/// states in the same order, each about as long (the cmds are retail's, the
/// snapshot grid is not).
fn check(spot: (&str, [f32; 2])) {
    if vcod_common::testing::uo_fs().is_none() {
        return;
    }
    let (retail_states, cmds) = retail(spot.0);
    let ours_states = ours(spot, &cmds);
    let (retail, ours) = (runs(&retail_states), runs(&ours_states));
    if std::env::var_os("MOUNT_REPORT").is_some() {
        for (side, r) in [("retail", &retail), ("ours", &ours)] {
            for (s, n) in r {
                println!(
                    "{} {side} pm_flags {:#x} hint {} val {} x{n}",
                    spot.0, s.pm_flags, s.hint, s.val
                );
            }
        }
    }
    let seq = |r: &[(State, usize)]| r.iter().map(|(s, _)| *s).collect::<Vec<_>>();
    assert_eq!(seq(&ours), seq(&retail), "{}", spot.0);
    for ((s, a), (_, b)) in ours.iter().zip(&retail) {
        assert!(
            a.abs_diff(*b) <= 3,
            "{} {s:?}: ours {a} snapshots, retail {b}",
            spot.0
        );
    }
}

/// The deploy on the crouched script mount, the breakdown, the crouched
/// deploy and the stand-up run through the same states on both: hint 7 with
/// value 0 off the trigger's own contents, the bipod down crouched
/// (`pm_flags` 0xa2) with no hint, a crouched hint 7 between.
#[test]
fn a_script_mount_deploys_as_on_uo() {
    check(MOUNT);
}

/// A mount raises `"trigger"` once per cmd the server runs, on the clock
/// of the frame after it: retail's capture holds as many in each frame as
/// the cmds the snapshot's `commandTime` moved past, a cmd stamped at or
/// before the last one run never running (`SV_UserMove`). Ours raises the
/// same count over the replayed cmds (coduo-movement.md 11).
#[test]
fn a_script_mount_fires_once_per_cmd_as_on_uo() {
    if vcod_common::testing::uo_fs().is_none() {
        return;
    }
    let (_, cmds, snaps) = retail_capture("mount-fires");
    let text =
        std::fs::read_to_string("tests/fixtures/playerstate/mp_foy-uofoot-mount-fires-uo.txt")
            .expect("read the fires");
    let fires: std::collections::HashMap<i32, usize> = text
        .lines()
        .filter_map(|l| {
            let mut w = l.strip_prefix("fires ")?.split(' ');
            Some((w.next()?.parse().ok()?, w.next()?.parse().ok()?))
        })
        .collect();
    let mut ran: Vec<i32> = Vec::new();
    for c in &cmds {
        if ran.last().is_none_or(|&last| c.st > last) {
            ran.push(c.st);
        }
    }
    for w in snaps.windows(2) {
        let ((_, c0), (t, c1)) = (w[0], w[1]);
        if c1 > *ran.last().unwrap() {
            break;
        }
        let n = ran.iter().filter(|&&st| c0 < st && st <= c1).count();
        assert_eq!(
            fires.get(&t).copied().unwrap_or(0),
            n,
            "retail's frame at {t}"
        );
    }
    let (_, ours) = ours_run(MOUNT, &cmds, true);
    assert_eq!(ours, ran.len(), "ours' fires over {} cmds", cmds.len());
}

/// The same on the `hmgstanding` sandbags: the hint off the brush, the
/// deploy standing.
#[test]
fn the_standing_mount_brush_deploys_as_on_uo() {
    check(STAND);
}

/// And on the `hmgcrouching` ones, deployed crouched.
#[test]
fn the_crouching_mount_brush_deploys_as_on_uo() {
    check(CROUCH);
}
