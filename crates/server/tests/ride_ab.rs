//! A brush model mover carrying, pushing and stalling against a player, and
//! a player linked to a moving `script_origin`, against the retail capture.
//!
//! Two fixtures from one retail run (their headers carry the recipe):
//! `movers/mp_carentan-dm-ride.txt` is the server's `games_mp.log` under
//! `client-probes/probe_ride.gsc`, one `PROBE f` line per server frame of a
//! phase; `-ride-wire.txt` is the `--probe-ride` client's playerstate per
//! snapshot. Ours runs the same probe as its gametype with one client
//! joined, and both halves are compared phase by phase, each sample at the
//! same offset from its phase's start. `docs/research/cod11-movers.md`,
//! sections 11 to 13, is what the run measured.
//!
//! A third, `-ride-ents.txt`, is a later run of the same probe with the
//! `bm_*` phases added: the brush model's own entity as the client was sent
//! it, diffed per snapshot against the entity ours sends (section 14).
//!
//! `RIDE_REPORT=1` prints every compared row.
//!
//! Needs `COD_DIR`; without the paks the test returns early.

mod common;

use common::{ADDR, ClientEnd, FRAME_MS, Join, Queues, holding, step_at};
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;
use std::time::{Duration, Instant};
use vcod_common::net::protocol::PROTOCOL_V1;
use vcod_common::net::trajectory::Trajectory;
use vcod_common::net::{NetClient, NetEvent};

const MAP: &str = "mp_carentan";
const SERVER: &str = "tests/fixtures/movers/mp_carentan-dm-ride.txt";
const WIRE: &str = "tests/fixtures/movers/mp_carentan-dm-ride-wire.txt";
const ENTS: &str = "tests/fixtures/movers/mp_carentan-dm-ride-ents.txt";
/// The slab's brush model, `*5`; retail numbers its entity 177.
const SLAB_MODEL: i32 = 5;

/// What both halves agree to: float formatting and the resting height.
/// Ours rests on the courtyard terrain at z -31.87 where retail rests at
/// -31.84 (the lift, over and crush phases); that is the terrain clip's, not
/// the push's.
const TOL: f32 = 0.05;

/// Known divergences, `(phase, what, tolerance)`; `what` is `player`,
/// `mover` or `wire`. Each names the movers doc section that explains it.
const GAPS: &[(&str, &str, f32)] = &[
    // Retail's pushed player drifts +x by 0.021 a frame, the per-frame yaw
    // residual its pusher keeps after the `rotateyaw` pair (section 12).
    ("push_y", "player", 0.75),
    ("push_y", "wire", 0.75),
    ("push_y_back", "player", 0.75),
    ("push_y_back", "wire", 0.75),
    // The slab resumes its descent a frame later on ours once the player is
    // teleported out from under it (section 12): 2 units at 40 u/s.
    ("reset", "mover", 2.05),
    // The yaw residual again: retail's rider comes back to x -214.94.
    ("ride_yaw_back", "player", 0.1),
    ("ride_yaw_back", "wire", 0.1),
];

fn report() -> bool {
    std::env::var("RIDE_REPORT").is_ok_and(|v| v == "1")
}

/// `(x, y, z)` as retail's `logPrint` renders a vector.
fn vectors(line: &str) -> Vec<[f32; 3]> {
    line.split('(')
        .skip(1)
        .filter_map(|s| {
            let inner = s.split(')').next()?;
            let v: Vec<f32> = inner
                .split(',')
                .filter_map(|t| t.trim().parse().ok())
                .collect();
            (v.len() == 3).then(|| [v[0], v[1], v[2]])
        })
        .collect()
}

/// One phase of the server half: its start, then per frame the player's
/// origin, the mover's origin and its angles.
#[derive(Default, Debug)]
struct Phase {
    start: Option<i32>,
    frames: Vec<[[f32; 3]; 3]>,
}

fn phases<'a>(lines: impl Iterator<Item = &'a str>) -> BTreeMap<String, Phase> {
    let mut out: BTreeMap<String, Phase> = BTreeMap::new();
    for line in lines.filter(|l| !l.starts_with('#')) {
        let Some(rest) = line.find("PROBE ").map(|i| &line[i + 6..]) else {
            continue;
        };
        let tokens: Vec<&str> = rest.split_whitespace().collect();
        match tokens.as_slice() {
            ["f", phase, t, ..] => {
                let v = vectors(rest);
                let p = out.entry(phase.to_string()).or_default();
                p.start.get_or_insert(t.parse().unwrap());
                p.frames.push([v[0], v[1], v[2]]);
            }
            ["at", phase, t] => {
                out.entry(phase.to_string()).or_default().start = Some(t.parse().unwrap());
            }
            _ => {}
        }
    }
    out
}

/// The player's origin per snapshot, by serverTime.
type Wire = BTreeMap<i32, [f32; 3]>;

/// `RIDE t=<serverTime> ... origin=x,y,z ...` lines, by serverTime.
fn wire(text: &str) -> Wire {
    text.lines()
        .filter(|l| l.starts_with("RIDE "))
        .map(|l| {
            let field = |k: &str| {
                l.split_whitespace()
                    .find_map(|t| t.strip_prefix(k))
                    .unwrap()
            };
            let o: Vec<f32> = field("origin=")
                .split(',')
                .map(|v| v.parse().unwrap())
                .collect();
            (field("t=").parse().unwrap(), [o[0], o[1], o[2]])
        })
        .collect()
}

fn diff(a: [f32; 3], b: [f32; 3]) -> f32 {
    (0..3).map(|i| (a[i] - b[i]).abs()).fold(0.0, f32::max)
}

/// The slab's entity as one snapshot carried it, `None` when it was not in
/// the snapshot.
type SlabEnt = Option<SlabState>;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct SlabState {
    solid: i32,
    index: i32,
    eflags: i32,
    pos: Trajectory,
    apos: Trajectory,
}

/// One run of ours: the script log, the client's origin per snapshot and the
/// slab's entity per snapshot, both by serverTime.
struct Ours {
    log: Vec<String>,
    wire: Wire,
    slab: BTreeMap<i32, SlabEnt>,
}

/// Both tests read the one run; it takes most of the gate's time.
fn ours() -> Option<&'static Ours> {
    static RUN: std::sync::OnceLock<Option<Ours>> = std::sync::OnceLock::new();
    RUN.get_or_init(run_ours).as_ref()
}

/// The probe on ours with one allied client standing still, run to its
/// `PROBE done`.
fn run_ours() -> Option<Ours> {
    let fs = vcod_common::testing::game_fs()?;
    let bsp_path = fs.resolve_map(MAP).expect("map in the mounted paks");
    let bsp = vcod_common::bsp::parse(&fs.read(&bsp_path).unwrap()).unwrap();
    let mut now = Instant::now();
    let mut sv = vcod_server::Server::new(common::cfg(MAP, "probe_ride"), now);
    sv.overlay_script(
        "maps/mp/gametypes/probe_ride",
        include_str!("../../gsc/tests/fixtures/semantics/client-probes/probe_ride.gsc"),
    );
    sv.set_cvar("probe_teleport", "1");
    sv.load_world(vcod_server::world::World::from_bsp(&bsp, Some(&fs)));
    sv.load_scripts(Rc::new(fs)).expect("load the scripts");
    let q = Rc::new(RefCell::new(Queues::default()));
    let mut cl = NetClient::start_with_qport(ClientEnd(q.clone()), now, 0x2001);
    let mut join = Join::new("allies", "m1carbine_mp");
    let mut seen = BTreeMap::new();
    let mut slab = BTreeMap::new();
    for _ in 0..3000 {
        now += Duration::from_millis(FRAME_MS as u64);
        cl.send_frame(&holding(&cl));
        for e in step_at(&mut sv, ADDR, &q, &mut cl, now) {
            match e {
                NetEvent::ServerCommand(tokens) => join.on_server_command(&tokens, &mut cl, now),
                NetEvent::Dropped(r) => panic!("dropped: {r}"),
                _ => {}
            }
        }
        if let Some(s) = cl.snapshots().newest() {
            let p = &PROTOCOL_V1;
            seen.insert(s.server_time, s.ps.origin(p));
            let ent = s
                .entities
                .values()
                .find(|e| e.field_i32(p, "eType") == 8 && e.field_i32(p, "index") == SLAB_MODEL);
            slab.insert(
                s.server_time,
                ent.map(|e| SlabState {
                    solid: e.field_i32(p, "solid"),
                    index: e.field_i32(p, "index"),
                    eflags: e.field_i32(p, "eFlags"),
                    pos: Trajectory::read(e, p, "pos"),
                    apos: Trajectory::read(e, p, "apos"),
                }),
            );
        }
        if sv.script_log().iter().any(|l| l.contains("PROBE done")) {
            break;
        }
    }
    assert_eq!(sv.script_aborts(), Vec::<String>::new());
    Some(Ours {
        log: sv.script_log().to_vec(),
        wire: seen,
        slab,
    })
}

#[test]
fn a_brush_model_mover_carries_and_pushes_players_like_retail() {
    let Some(Ours {
        log,
        wire: ours_wire,
        ..
    }) = ours()
    else {
        eprintln!("COD_DIR unset or has no main/: skipping");
        return;
    };
    assert!(
        log.iter().any(|l| l.contains("PROBE done")),
        "the probe never finished on ours"
    );
    let retail_text = std::fs::read_to_string(SERVER).unwrap();
    let retail = phases(retail_text.lines());
    let ours = phases(log.iter().map(String::as_str));
    let ours_wire = ours_wire.clone();
    let retail_wire = wire(&std::fs::read_to_string(WIRE).unwrap());
    let tol = |phase: &str, what: &str| {
        GAPS.iter()
            .find(|g| g.0 == phase && g.1 == what)
            .map_or(TOL, |g| g.2)
    };

    let mut bad = Vec::new();
    for (name, r) in &retail {
        let o = ours
            .get(name)
            .unwrap_or_else(|| panic!("ours never ran {name}"));
        assert_eq!(r.frames.len(), o.frames.len(), "{name}: sample count");
        for (k, (rf, of)) in r.frames.iter().zip(&o.frames).enumerate() {
            let player = diff(rf[0], of[0]);
            let mover = diff(rf[1], of[1]).max(diff(rf[2], of[2]));
            if report() {
                println!(
                    "{name} {k} player {:?} {:?} mover {:?} {:?}",
                    rf[0], of[0], rf[1], of[1]
                );
            }
            if player > tol(name, "player") {
                bad.push(format!(
                    "{name} frame {k}: player retail {:?} ours {:?}",
                    rf[0], of[0]
                ));
            }
            if mover > tol(name, "mover") {
                bad.push(format!(
                    "{name} frame {k}: mover retail {:?} {:?} ours {:?} {:?}",
                    rf[1], rf[2], of[1], of[2]
                ));
            }
        }
        // The wire half: the snapshot at each frame of the phase, which
        // carries the push a frame before the script's own `.origin` does.
        let (Some(rs), Some(os)) = (r.start, o.start) else {
            continue;
        };
        for k in 0..r.frames.len() as i32 {
            let (Some(rw), Some(ow)) = (
                retail_wire.get(&(rs + k * FRAME_MS as i32)),
                ours_wire.get(&(os + k * FRAME_MS as i32)),
            ) else {
                continue;
            };
            if report() {
                println!("{name} {k} wire {rw:?} {ow:?}");
            }
            if diff(*rw, *ow) > tol(name, "wire") {
                bad.push(format!("{name} snapshot {k}: retail {rw:?} ours {ow:?}"));
            }
        }
    }
    assert!(
        bad.is_empty(),
        "{} rows differ:\n{}",
        bad.len(),
        bad.join("\n")
    );
}

/// A `[x,y,z]` the probe prints.
fn bracket(t: &str) -> glam::Vec3 {
    let v: Vec<f32> = t
        .trim_matches(|c| c == '[' || c == ']')
        .split(',')
        .map(|v| v.parse().unwrap())
        .collect();
    glam::Vec3::new(v[0], v[1], v[2])
}

/// `trType a trTime b trDuration c base [..] delta [..]`, from `tokens[0]`.
fn trajectory(tokens: &[&str]) -> Trajectory {
    Trajectory {
        tr_type: tokens[1].parse().unwrap(),
        tr_time: tokens[3].parse().unwrap(),
        tr_duration: tokens[5].parse().unwrap(),
        base: bracket(tokens[7]),
        delta: bracket(tokens[9]),
    }
}

/// The ents fixture: each phase's start, and the slab's entity at `t` as the
/// latest change line at or before it reads.
struct RetailSlab {
    starts: BTreeMap<String, i32>,
    done: i32,
    /// `(serverTime, state)` per change, in order.
    changes: Vec<(i32, SlabEnt)>,
}

impl RetailSlab {
    fn parse(text: &str) -> Self {
        let mut starts = BTreeMap::new();
        let mut done = 0;
        let mut changes: Vec<(i32, SlabEnt)> = Vec::new();
        let mut cur: SlabEnt = None;
        let kv = |l: &str, k: &str| -> i64 {
            let v = l
                .split_whitespace()
                .find_map(|t| t.strip_prefix(k))
                .unwrap();
            match v.strip_prefix("0x") {
                Some(h) => i64::from_str_radix(h, 16).unwrap(),
                None => v.parse().unwrap(),
            }
        };
        for l in text.lines().filter(|l| !l.starts_with('#')) {
            let tokens: Vec<&str> = l.split_whitespace().collect();
            match tokens.as_slice() {
                ["PROBE", "at", phase, t] => {
                    starts.insert(phase.to_string(), t.parse().unwrap());
                }
                ["PROBE", "done", t] => done = t.parse().unwrap(),
                ["RIDE_ENT", ..] if kv(l, "num=") == 177 => {
                    let mut s = cur.unwrap_or_default();
                    s.solid = kv(l, "solid=") as i32;
                    s.index = kv(l, "index=") as i32;
                    s.eflags = kv(l, "eFlags=") as i32;
                    cur = Some(s);
                    changes.push((kv(l, "t=") as i32, cur));
                }
                ["RIDE_GONE", ..] if kv(l, "num=") == 177 => {
                    cur = None;
                    changes.push((kv(l, "t=") as i32, cur));
                }
                [
                    "entity",
                    "177",
                    "serverTime",
                    t,
                    "eType",
                    _,
                    "pos",
                    rest @ ..,
                ] => {
                    let mut s = cur.expect("a trajectory line before RIDE_ENT");
                    s.pos = trajectory(&rest[..10]);
                    s.apos = trajectory(&rest[11..21]);
                    cur = Some(s);
                    changes.push((t.parse().unwrap(), cur));
                }
                _ => {}
            }
        }
        RetailSlab {
            starts,
            done,
            changes,
        }
    }

    fn at(&self, t: i32) -> SlabEnt {
        self.changes
            .iter()
            .take_while(|(ct, _)| *ct <= t)
            .last()
            .and_then(|(_, s)| *s)
    }
}

/// `t` moved by `shift`, unless it is the 0 of a group no verb has touched.
fn shifted(t: i32, shift: i32) -> i32 {
    if t == 0 { 0 } else { t + shift }
}

/// How far ours may sit from retail: a trajectory's `trTime` in ms, its
/// `trBase` and `trDelta` in units.
#[derive(Clone, Copy)]
struct SlabTol {
    time: i32,
    base: f32,
    delta: f32,
}

fn slab_diff(retail: &SlabState, ours: &SlabState, shift: i32, tol: SlabTol) -> Option<String> {
    let mut out = Vec::new();
    for (name, r, o) in [
        ("solid", retail.solid, ours.solid),
        ("index", retail.index, ours.index),
        ("eFlags", retail.eflags, ours.eflags),
    ] {
        if r != o {
            out.push(format!("{name} retail {r:#x} ours {o:#x}"));
        }
    }
    for (group, r, o) in [
        ("pos", retail.pos, ours.pos),
        ("apos", retail.apos, ours.apos),
    ] {
        let same = r.tr_type == o.tr_type
            && (shifted(r.tr_time, shift) - o.tr_time).abs() <= tol.time
            && r.tr_duration == o.tr_duration
            && (r.base - o.base).abs().max_element() < TOL.max(tol.base)
            && (r.delta - o.delta).abs().max_element() < TOL.max(tol.delta);
        if !same {
            out.push(format!("{group} retail {r:?} (time +{shift}) ours {o:?}"));
        }
    }
    (!out.is_empty()).then(|| out.join("; "))
}

/// Known divergences of the slab's entity, `(phase, tolerance)`, each from
/// that phase to the end of the run: a stationary trajectory keeps its
/// `trTime` and `trDelta` until the next verb, so a difference outlives the
/// phase that made it.
const SLAB_GAPS: &[(&str, SlabTol)] = &[
    // Retail's lowered slab moves 2 units once more after it first stalls
    // (the frame its push's keep-in-place fallback clears the player and
    // writes ground 1023), ours holds: from there every `trTime` the stall
    // shifts is a frame apart, and the `moveto` that resets it starts 2
    // units higher, in its `trBase` and its `trDelta` (section 12; the
    // `reset` row of `GAPS`).
    (
        "crush",
        SlabTol {
            time: 50,
            base: 2.05,
            delta: 2.05,
        },
    ),
];

#[test]
fn a_brush_model_mover_goes_on_the_wire_like_retail() {
    let Some(Ours { log, slab, .. }) = ours() else {
        eprintln!("COD_DIR unset or has no main/: skipping");
        return;
    };
    let retail = RetailSlab::parse(&std::fs::read_to_string(ENTS).unwrap());
    let mut ours_starts = BTreeMap::new();
    for l in log {
        if let Some(rest) = l.split("PROBE at ").nth(1) {
            let mut t = rest.split_whitespace();
            let (phase, at) = (t.next().unwrap(), t.next().unwrap());
            ours_starts.insert(phase.to_string(), at.parse::<i32>().unwrap());
        }
    }
    let shift = ours_starts["ride_up"] - retail.starts["ride_up"];
    for (phase, rs) in &retail.starts {
        assert_eq!(
            ours_starts.get(phase).map(|o| o - shift),
            Some(*rs),
            "{phase} starts at another offset on ours"
        );
    }
    let mut bad = Vec::new();
    let mut compared = 0;
    for t in (retail.starts["ride_up"]..=retail.done).step_by(FRAME_MS as usize) {
        let Some(o) = slab.get(&(t + shift)) else {
            continue;
        };
        let phase = retail
            .starts
            .iter()
            .filter(|(_, s)| **s <= t)
            .max_by_key(|(_, s)| **s)
            .map_or("", |(p, _)| p.as_str());
        let r = retail.at(t);
        if report() {
            println!("slab {phase} {t} retail {r:?} ours {o:?}");
        }
        compared += 1;
        let tol = SLAB_GAPS
            .iter()
            .filter(|(from, _)| retail.starts[*from] <= t)
            .map(|(_, tol)| *tol)
            .next_back()
            .unwrap_or(SlabTol {
                time: 0,
                base: 0.0,
                delta: 0.0,
            });
        let row = match (r, o) {
            (None, None) => None,
            (Some(_), None) => Some("retail sends it, ours does not".to_string()),
            (None, Some(_)) => Some("ours sends it, retail does not".to_string()),
            (Some(r), Some(o)) => slab_diff(&r, o, shift, tol),
        };
        if let Some(row) = row {
            bad.push(format!("{phase} t={t}: {row}"));
        }
    }
    assert!(compared > 900, "only {compared} snapshots compared");
    assert!(
        bad.is_empty(),
        "{} snapshots differ:\n{}",
        bad.len(),
        bad.join("\n")
    );
}
