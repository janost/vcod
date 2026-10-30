//! A brush model mover carrying, pushing and stalling against a player, and
//! a player linked to a moving `script_origin`, against the retail capture.
//!
//! Two fixtures from one retail run (the first one's header carries the
//! recipe): `movers/mp_carentan-dm-ride.txt` is the server's `games_mp.log`
//! under `client-probes/probe_ride.gsc`, one `PROBE f` line per server frame
//! of a phase, then the `--probe-ride` client's `CMDS` lines;
//! `-ride-wire.txt` is that client's playerstate per snapshot, the slab's
//! entity changes and every trajectory change. Ours runs the same probe as
//! its gametype with one client joined on the capture's cmds, and both
//! halves are compared phase by phase, each sample at the same offset from
//! its phase's start, and every snapshot from the first placement on.
//! `docs/research/cod11-movers.md`, sections 11 to 14, is what the run
//! measured.
//!
//! `RIDE_REPORT=1` prints every compared row.
//!
//! Needs `COD_DIR`; without the paks the test returns early.

use crate::common;

use common::{ADDR, ClientEnd, FRAME_MS, Join, Queues, holding, step_at};
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;
use std::time::{Duration, Instant};
use vcod_common::net::protocol::{PROTOCOL_V1, PROTOCOL_V6, PROTOCOL_V22, Protocol};
use vcod_common::net::trajectory::Trajectory;
use vcod_common::net::{NetClient, NetEvent};

const MAP: &str = "mp_carentan";
const SERVER: &str = "tests/fixtures/movers/mp_carentan-dm-ride.txt";
const WIRE: &str = "tests/fixtures/movers/mp_carentan-dm-ride-wire.txt";
/// The slab's brush model, `*5`; retail numbers its entity 177.
const SLAB_MODEL: i32 = 5;

/// The script log's two decimals.
const TOL: f32 = 0.006;

/// The `RIDE` line's three decimals.
const WIRE_TOL: f32 = 0.0006;

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
    /// The client's first snapshot's time.
    first_snap: Option<i32>,
}

/// A retail capture's probe: the gametype name, its source, the
/// capture whose cmds ours replays and the cvars the run set.
struct Probe {
    name: &'static str,
    source: &'static str,
    fixture: &'static str,
    cvars: &'static [(&'static str, &'static str)],
}

const RIDE: Probe = Probe {
    name: "probe_ride",
    source: include_str!("../../gsc/tests/fixtures/semantics/client-probes/probe_ride.gsc"),
    fixture: SERVER,
    cvars: &[("probe_teleport", "1")],
};

/// The wire tests read the one run; it takes most of the gate's time.
fn ours() -> Option<&'static Ours> {
    static RUN: std::sync::OnceLock<Option<Ours>> = std::sync::OnceLock::new();
    RUN.get_or_init(|| run_ours(&RIDE)).as_ref()
}

/// [`run_once`], run again once if the client's first snapshot missed
/// retail's.
fn run_ours(probe: &Probe) -> Option<Ours> {
    let ours = run_once(probe, 0)?;
    let text = std::fs::read_to_string(probe.fixture).unwrap();
    let first = common::cmds_ran(&text, "").first()?.0;
    match ours.first_snap {
        Some(t) if t != first => run_once(probe, (t - first) as i64),
        _ => Some(ours),
    }
}

/// How long before its first snapshot the capture's client starts joining.
const FIRST_SNAP_AFTER_JOIN_MS: i64 = 400;

/// The probe on ours with one allied client standing still, run to its
/// `PROBE done`. The client joins so its first snapshot falls on retail's
/// and replays the capture's cmds, each on retail's side of a frame
/// (`ProbeHold::framed`): where a fall lands, and whether a frame ran with
/// no cmd from the player, rides on them (movers doc 12, "A blocked
/// mover", and 21). `late_ms` starts the client that much earlier.
fn run_once(probe: &Probe, late_ms: i64) -> Option<Ours> {
    let fs = vcod_common::testing::game_fs_11()?;
    let bsp_path = fs.resolve_map(MAP).expect("map in the mounted paks");
    let bsp = vcod_common::bsp::parse(&fs.read(&bsp_path).unwrap()).unwrap();
    let mut now = Instant::now();
    let mut sv = vcod_server::Server::new(common::cfg(MAP, probe.name), now);
    sv.overlay_script(&format!("maps/mp/gametypes/{}", probe.name), probe.source);
    for (k, v) in probe.cvars {
        sv.set_cvar(k, v);
    }
    sv.load_world(vcod_server::world::World::from_bsp(&bsp, Some(&fs)));
    sv.load_scripts(Rc::new(fs)).expect("load the scripts");
    let retail_text = std::fs::read_to_string(probe.fixture).unwrap();
    let mut hold = common::ProbeHold {
        base: Some([0.0; 3]),
        framed: true,
        times: common::cmds_times(&retail_text, ""),
        ran: common::cmds_ran(&retail_text, ""),
        ..Default::default()
    };
    let first = hold.ran.first().expect("the capture's CMDS lines").0;
    while (sv.game_time_ms() as i64) < first as i64 - FIRST_SNAP_AFTER_JOIN_MS - late_ms {
        now += Duration::from_millis(FRAME_MS as u64);
        sv.tick(now);
        sv.take_outgoing();
    }
    let q = Rc::new(RefCell::new(Queues::default()));
    let mut cl = NetClient::start_with_qport(ClientEnd(q.clone()), now, 0x2001);
    let mut join = Join::new("allies", "m1carbine_mp");
    let mut seen = BTreeMap::new();
    let mut slab = BTreeMap::new();
    let mut first_snap = None;
    for _ in 0..3000 {
        now += Duration::from_millis(FRAME_MS as u64);
        // Playing, linked (`pm_type` 1) or not.
        let live = cl
            .snapshots()
            .newest()
            .is_some_and(|s| s.ps.field_i32(&PROTOCOL_V1, "pm_type") <= 1);
        if live {
            let cmds = hold.cmds(&cl, 0);
            cl.send_cmds(&cmds);
        } else if let Some(c) = cl.send_frame(&holding(&cl)) {
            hold.last = c.server_time;
        }
        for e in step_at(&mut sv, ADDR, &q, &mut cl, now) {
            match e {
                NetEvent::ServerCommand(tokens) => join.on_server_command(&tokens, &mut cl, now),
                NetEvent::Dropped(r) => panic!("dropped: {r}"),
                _ => {}
            }
        }
        if let Some(s) = cl.snapshots().newest() {
            first_snap.get_or_insert(s.server_time);
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
        first_snap,
    })
}

/// Every snapshot of the run, falls between phases included, against
/// retail's to its 3-decimal print: replayed on the capture's cmds, a fall
/// lands where retail's did (movers doc 21, "The slide down the slope").
#[test]
fn every_snapshot_matches_retail() {
    let Some(Ours { wire: ours, .. }) = ours() else {
        eprintln!("COD_DIR unset or has no main/: skipping");
        return;
    };
    let retail = wire(&std::fs::read_to_string(WIRE).unwrap());
    // The spawn is a random draw; the probe places the player a second
    // before its first phase.
    let text = std::fs::read_to_string(SERVER).unwrap();
    let from = phases(text.lines())["ride_up"].start.unwrap() - 1000;
    let mut bad = Vec::new();
    let mut compared = 0;
    for (t, r) in retail.range(from..) {
        let Some(o) = ours.get(t) else { continue };
        compared += 1;
        if diff(*r, *o) > WIRE_TOL {
            bad.push(format!("t={t}: retail {r:?} ours {o:?}"));
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

    let mut bad = Vec::new();
    for (name, r) in &retail {
        let o = ours
            .get(name)
            .unwrap_or_else(|| panic!("ours never ran {name}"));
        assert_eq!(r.frames.len(), o.frames.len(), "{name}: sample count");
        for (k, (rf, of)) in r.frames.iter().zip(&o.frames).enumerate() {
            let mover = diff(rf[1], of[1]).max(diff(rf[2], of[2]));
            if report() {
                println!(
                    "{name} {k} player {:?} {:?} mover {:?} {:?}",
                    rf[0], of[0], rf[1], of[1]
                );
            }
            if diff(rf[0], of[0]) > TOL {
                bad.push(format!(
                    "{name} frame {k}: player retail {:?} ours {:?}",
                    rf[0], of[0]
                ));
            }
            if mover > TOL {
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
            if diff(*rw, *ow) > WIRE_TOL {
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

fn slab_diff(retail: &SlabState, ours: &SlabState, shift: i32) -> Option<String> {
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
            && shifted(r.tr_time, shift) == o.tr_time
            && r.tr_duration == o.tr_duration
            && (r.base - o.base).abs().max_element() < TOL
            && (r.delta - o.delta).abs().max_element() < TOL;
        if !same {
            out.push(format!("{group} retail {r:?} (time +{shift}) ours {o:?}"));
        }
    }
    (!out.is_empty()).then(|| out.join("; "))
}

#[test]
fn a_brush_model_mover_goes_on_the_wire_like_retail() {
    let Some(Ours { log, slab, .. }) = ours() else {
        eprintln!("COD_DIR unset or has no main/: skipping");
        return;
    };
    let text = [SERVER, WIRE].map(|f| std::fs::read_to_string(f).unwrap());
    let retail = RetailSlab::parse(&text.concat());
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
        let row = match (r, o) {
            (None, None) => None,
            (Some(_), None) => Some("retail sends it, ours does not".to_string()),
            (None, Some(_)) => Some("ours sends it, retail does not".to_string()),
            (Some(r), Some(o)) => slab_diff(&r, o, shift),
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

const YAWRES: &str = "tests/fixtures/movers/mp_carentan-dm-yawres.txt";

/// `probe_yawres`'s `PROBE y` rows by phase, the level time dropped: the
/// angles as printed (times 1e9, `%g`) and `getorigin()`.
fn yaw_rows<'a>(lines: impl Iterator<Item = &'a str>) -> BTreeMap<String, Vec<String>> {
    let mut out: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for l in lines.filter(|l| !l.starts_with('#')) {
        let Some(rest) = l.split("PROBE y ").nth(1) else {
            continue;
        };
        let mut t = rest.splitn(3, ' ');
        let (phase, _time, row) = (t.next().unwrap(), t.next(), t.next().unwrap());
        out.entry(phase.to_string())
            .or_default()
            .push(row.trim().to_string());
    }
    out
}

/// A finished rotate leaves `r.currentAngles` quantized to 65536ths
/// (`Reached_ScriptMover`) and every link snaps a component within a
/// thousandth of an integer (`SV_LinkEntity`), so `.angles` reads
/// 1.99951 after `rotateyaw(2, 1)` and the next move turns the slab by
/// what is left (movers doc 12, "The residual yaw"). Server alone, no
/// client.
#[test]
fn a_rotated_brush_model_keeps_retails_quantized_angles() {
    let Some(fs) = vcod_common::testing::game_fs_11() else {
        eprintln!("COD_DIR unset or has no main/: skipping");
        return;
    };
    let bsp_path = fs.resolve_map(MAP).expect("map in mounted paks");
    let bsp = vcod_common::bsp::parse(&fs.read(&bsp_path).unwrap()).unwrap();
    let mut now = Instant::now();
    let mut sv = vcod_server::Server::new(common::cfg(MAP, "probe_yawres"), now);
    sv.overlay_script(
        "maps/mp/gametypes/probe_yawres",
        include_str!("../../gsc/tests/fixtures/semantics/client-probes/probe_yawres.gsc"),
    );
    sv.load_world(vcod_server::world::World::from_bsp(&bsp, Some(&fs)));
    sv.load_scripts(Rc::new(fs)).expect("load scripts");
    for _ in 0..600 {
        common::run_alone(&mut sv, &mut now, FRAME_MS);
        if sv.script_log().iter().any(|l| l.contains("PROBE done")) {
            break;
        }
    }
    assert_eq!(sv.script_aborts(), Vec::<String>::new());
    let ours = yaw_rows(sv.script_log().iter().map(String::as_str));
    let text = std::fs::read_to_string(YAWRES).unwrap();
    let retail = yaw_rows(text.lines());
    assert_eq!(
        retail.keys().collect::<Vec<_>>(),
        ours.keys().collect::<Vec<_>>()
    );
    let mut bad = Vec::new();
    for (phase, r) in &retail {
        let o = &ours[phase];
        assert_eq!(r.len(), o.len(), "{phase}: sample count");
        for (k, (r, o)) in r.iter().zip(o).enumerate() {
            if r != o {
                bad.push(format!("{phase} {k}: retail {r} ours {o}"));
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

const CRUSH: Probe = Probe {
    name: "probe_crush",
    source: include_str!("../../gsc/tests/fixtures/semantics/client-probes/probe_crush.gsc"),
    fixture: "tests/fixtures/movers/mp_carentan-dm-crush.txt",
    cvars: &[],
};

/// `probe_crush`'s script log: per phase the start and every frame's
/// `(phase, level time, player origin, slab origin)`.
struct Crush {
    starts: BTreeMap<String, i32>,
    frames: Vec<(String, i32, [f32; 3], [f32; 3])>,
}

impl Crush {
    fn parse<'a>(lines: impl Iterator<Item = &'a str>) -> Crush {
        let mut starts = BTreeMap::new();
        let mut frames = Vec::new();
        for l in lines.filter(|l| !l.starts_with('#')) {
            let Some(l) = l.find("PROBE ").map(|i| &l[i..]) else {
                continue;
            };
            match l.split_whitespace().collect::<Vec<_>>().as_slice() {
                ["PROBE", "at", phase, t] => {
                    starts.insert(phase.to_string(), t.parse().unwrap());
                }
                ["PROBE", "f", phase, t, ..] => {
                    let v = vectors(l);
                    frames.push((phase.to_string(), t.parse().unwrap(), v[0], v[1]));
                }
                _ => {}
            }
        }
        Crush { starts, frames }
    }
}

/// A blocked push leaves the player out of the area tree until his next
/// cmd links him (`G_MoverPush` 0x55621), so a frame that runs with no cmd
/// from him lets the slab move one frame's 2 units; and a `moveto` on the
/// stalled slab puts `r.currentOrigin` where its trajectory has it at the
/// call, a frame further on (0x5fb38). Ours replays the capture's cmds on
/// its jittered link, the frames that ran none included, and every frame
/// of every phase, the fall after the teleport too, matches to the log's
/// two decimals. Movers doc 12, "A blocked mover".
#[test]
fn a_blocked_slab_moves_on_a_frame_without_a_cmd_like_retail() {
    let retail = Crush::parse(std::fs::read_to_string(CRUSH.fixture).unwrap().lines());
    // The capture's slab slips under the player at least once while held.
    let held: Vec<f32> = retail
        .frames
        .iter()
        .filter(|f| f.0 == "crush")
        .map(|f| f.3[2])
        .collect();
    assert!(held.windows(2).any(|w| w[0] != w[1] && w[1] != held[0]));
    // The probe's phases run off the player's spawn, and ours' client takes
    // its own time through the team menu: it joins late or early by
    // however far the first run's lift missed retail's, so the frames that
    // run no cmd fall on the same phase frames.
    let mut late = 0;
    let mut ours = None;
    for _ in 0..3 {
        let Some(run) = run_once(&CRUSH, late) else {
            eprintln!("COD_DIR unset or has no main/: skipping");
            return;
        };
        let parsed = Crush::parse(run.log.iter().map(String::as_str));
        let shift = parsed.starts.get("lift").copied().unwrap_or(0) - retail.starts["lift"];
        ours = Some(parsed);
        if shift == 0 {
            break;
        }
        late += shift as i64;
    }
    let ours = ours.unwrap();
    assert_eq!(
        ours.starts, retail.starts,
        "ours' phases start on retail's frames"
    );
    let mut bad = Vec::new();
    for (phase, t, player, slab) in &retail.frames {
        let o = ours.frames.iter().find(|f| f.1 == *t);
        if report() {
            println!("{phase} {t}: retail {player:?} {slab:?} ours {o:?}");
        }
        let row = match o {
            Some((p, _, op, os))
                if p == phase && diff(*player, *op).max(diff(*slab, *os)) < TOL =>
            {
                continue;
            }
            Some((_, _, op, os)) => format!("player {op:?} slab {os:?}"),
            None => "no frame".to_string(),
        };
        bad.push(format!(
            "{phase} {t}: retail player {player:?} slab {slab:?}, ours {row}"
        ));
    }
    assert!(
        bad.is_empty(),
        "{} of {} frames differ:\n{}",
        bad.len(),
        retail.frames.len(),
        bad.join("\n")
    );
}

const ANGLESNAP: &str = "tests/fixtures/movers/mp_carentan-dm-anglesnap.txt";

/// `SV_LinkEntity` snaps `r.currentAngles` on every link, not only a
/// mover's: a script model, a script origin, an item and a brush model
/// each read the `.angles` written until an `.origin` write links them,
/// then the snapped set (movers doc 12, "The snap on every link"). Server
/// alone, no client.
#[test]
fn every_link_snaps_angles_like_retail() {
    let Some(fs) = vcod_common::testing::game_fs_11() else {
        eprintln!("COD_DIR unset or has no main/: skipping");
        return;
    };
    let bsp_path = fs.resolve_map(MAP).expect("map in mounted paks");
    let bsp = vcod_common::bsp::parse(&fs.read(&bsp_path).unwrap()).unwrap();
    let mut now = Instant::now();
    let mut sv = vcod_server::Server::new(common::cfg(MAP, "probe_anglesnap"), now);
    sv.overlay_script(
        "maps/mp/gametypes/probe_anglesnap",
        include_str!("../../gsc/tests/fixtures/semantics/client-probes/probe_anglesnap.gsc"),
    );
    sv.load_world(vcod_server::world::World::from_bsp(&bsp, Some(&fs)));
    sv.load_scripts(Rc::new(fs)).expect("load scripts");
    for _ in 0..100 {
        common::run_alone(&mut sv, &mut now, FRAME_MS);
        if sv.script_log().iter().any(|l| l.contains("PROBE done")) {
            break;
        }
    }
    assert_eq!(sv.script_aborts(), Vec::<String>::new());
    let rows = |lines: Vec<&str>| -> Vec<String> {
        lines
            .into_iter()
            .filter_map(|l| l.find("PROBE a ").map(|i| l[i..].trim().to_string()))
            .collect()
    };
    let ours = rows(sv.script_log().iter().map(String::as_str).collect());
    let text = std::fs::read_to_string(ANGLESNAP).unwrap();
    let retail = rows(text.lines().collect());
    assert_eq!(retail, ours);
}

const MOVERREADS: &str = "tests/fixtures/movers/mp_carentan-dm-moverreads.txt";
const MOVERREADS_15: &str = "tests/fixtures/movers/mp_carentan-dm-moverreads-1.5.txt";
const MOVERREADS_UO: &str = "tests/fixtures/movers/mp_carentan-dm-moverreads-uo.txt";

/// Mover script reads (movers doc 23): `.speed` and `.dmg` after a sine
/// mover's spawn, `.origin` across a `moveGravity` started on a moving
/// mover and `.angles` through rotates that cross 0 and 360. Server alone,
/// every line exact. `MOVERREADS_REPORT=1` prints both sides.
#[test]
fn mover_script_reads_match_retail() {
    let Some(fs) = vcod_common::testing::game_fs_11() else {
        eprintln!("COD_DIR unset or has no main/: skipping");
        return;
    };
    mover_reads(&PROTOCOL_V1, fs, MOVERREADS);
}

#[test]
fn mover_script_reads_match_retail_on_1_5() {
    let Some(fs) = vcod_common::testing::game_fs() else {
        eprintln!("COD_DIR unset or has no main/: skipping");
        return;
    };
    if vcod_server::server::install_protocol(&fs).transport.version != PROTOCOL_V6.transport.version
    {
        eprintln!("COD_DIR is not a 1.5 install: skipping");
        return;
    }
    mover_reads(&PROTOCOL_V6, fs, MOVERREADS_15);
}

#[test]
fn mover_script_reads_match_retail_on_uo() {
    let Some(fs) = vcod_common::testing::uo_fs() else {
        eprintln!("COD_DIR is not a UO install: skipping");
        return;
    };
    mover_reads(&PROTOCOL_V22, fs, MOVERREADS_UO);
}

fn mover_reads(proto: &'static Protocol, fs: vcod_common::pk3::Pk3Fs, fixture: &str) {
    let bsp_path = fs.resolve_map(MAP).expect("map in mounted paks");
    let bsp = vcod_common::bsp::parse(&fs.read(&bsp_path).unwrap()).unwrap();
    let mut now = Instant::now();
    let mut sv = vcod_server::Server::new(common::cfg(MAP, "probe_moverreads"), now);
    sv.set_protocol(proto);
    sv.overlay_script(
        "maps/mp/gametypes/probe_moverreads",
        include_str!("../../gsc/tests/fixtures/semantics/client-probes/probe_moverreads.gsc"),
    );
    sv.load_world(vcod_server::world::World::from_bsp(&bsp, Some(&fs)));
    sv.load_scripts(Rc::new(fs)).expect("load scripts");
    for _ in 0..400 {
        common::run_alone(&mut sv, &mut now, FRAME_MS);
        if sv.script_log().iter().any(|l| l.contains("PROBE done")) {
            break;
        }
    }
    assert_eq!(sv.script_aborts(), Vec::<String>::new());
    let rows = |lines: Vec<&str>| -> Vec<String> {
        lines
            .into_iter()
            .filter(|l| !l.starts_with('#'))
            .filter_map(|l| l.find("PROBE ").map(|i| l[i..].trim().to_string()))
            .collect()
    };
    let ours = rows(sv.script_log().iter().map(String::as_str).collect());
    let text = std::fs::read_to_string(fixture).unwrap();
    let retail = rows(text.lines().collect());
    if std::env::var("MOVERREADS_REPORT").is_ok_and(|v| v == "1") {
        for (r, o) in retail.iter().zip(&ours) {
            println!(
                "{} retail {r}\n  ours   {o}",
                if r == o { " " } else { "!" }
            );
        }
    }
    let bad: Vec<String> = retail
        .iter()
        .zip(&ours)
        .filter(|(r, o)| r != o)
        .map(|(r, o)| format!("retail {r}\nours   {o}"))
        .collect();
    assert!(
        bad.is_empty(),
        "{} rows differ:\n{}",
        bad.len(),
        bad.join("\n")
    );
    assert_eq!(retail.len(), ours.len(), "row count");
}
