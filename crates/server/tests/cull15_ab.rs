//! What a 1.5 server sends past the PVS cull, against retail 1.5b's run of
//! `client-probes/probe_cull15` on mp_carentan
//! (`tests/fixtures/entities/cull15-carentan.txt`). A script model out of
//! the viewer's PVS rides for 350 ms after a `playSound` on it (the
//! `r.broadcastTime` stamp of level time plus 300), from the last one when
//! sounded twice, for as long as `playLoopSound` loops and 350 ms past
//! `stopLoopSound`; a sounded `script_origin` never rides, in the PVS or
//! out of it; a 1000-unit cull fog drops everything past it, an entity
//! moved across it follows on its frame, a broadcast one rides through it,
//! and exponential fog brings everything back
//! (docs/research/cod15-protocol-diff.md 15).
//!
//! Per step the gate compares every change of the entity list the viewer
//! was sent, timed from the step's line. `CULL15_REPORT=1` prints both
//! sides. Needs `COD_DIR` at a 1.5 install; on any other it returns early.

use crate::common;

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;
use std::time::{Duration, Instant};
use vcod_common::net::msg::NULL_USERCMD;
use vcod_common::net::protocol::{PROTOCOL_V6, Protocol};

const MAP: &str = "mp_carentan";
const PROBE: &str = "probe_cull15";
const FIXTURE: &str = "tests/fixtures/entities/cull15-carentan.txt";

/// An entity list as `number:eType`, the wire's type.
type Ents = BTreeSet<String>;
/// A side's run: its `PROBE step` lines and every change of its list.
type Run = (Vec<(String, i32)>, Vec<(i32, Ents)>);

/// Each step's changes of list, timed from the step's line: the list in
/// force when the line was logged (at 0), then every different one
/// before the next step.
fn by_step(steps: &[(String, i32)], snaps: &[(i32, Ents)]) -> BTreeMap<String, Vec<(i32, Ents)>> {
    let mut out = BTreeMap::new();
    for (i, (name, t)) in steps.iter().enumerate() {
        let end = steps.get(i + 1).map_or(i32::MAX, |s| s.1);
        let mut run: Vec<(i32, Ents)> = Vec::new();
        let at = snaps
            .iter()
            .rev()
            .find(|(st, _)| st <= t)
            .map(|(_, e)| (*t, e));
        let later = snaps
            .iter()
            .filter(|(st, _)| st > t && *st < end)
            .map(|(st, e)| (*st, e));
        for (st, e) in at.into_iter().chain(later) {
            if run.last().map(|(_, l)| l) != Some(e) {
                run.push((st - t, e.clone()));
            }
        }
        out.insert(name.clone(), run);
    }
    out
}

fn step_line(l: &str) -> Option<(String, i32)> {
    let rest = l.trim().strip_prefix("PROBE step ")?;
    let (name, t) = rest.split_once(' ')?;
    Some((name.to_string(), t.trim().parse().ok()?))
}

fn retail() -> Run {
    let text = std::fs::read_to_string(FIXTURE).expect("read the fixture");
    let steps = text.lines().filter_map(step_line).collect();
    let snaps = text
        .lines()
        .filter_map(|l| {
            let rest = l.strip_prefix("EDGE ents serverTime=")?;
            let (t, list) = rest.split_once(" [")?;
            let ents = list
                .trim_end_matches(']')
                .split_whitespace()
                .map(str::to_string)
                .collect();
            Some((t.parse().ok()?, ents))
        })
        .collect();
    (steps, snaps)
}

fn ours(fs: vcod_common::pk3::Pk3Fs) -> Run {
    let p: &'static Protocol = &PROTOCOL_V6;
    let bsp_path = fs.resolve_map(MAP).expect("the map in the paks");
    let bsp = vcod_common::bsp::parse(&fs.read(&bsp_path).unwrap()).unwrap();
    let mut now = Instant::now();
    let mut sv = vcod_server::Server::new(common::cfg(MAP, PROBE), now);
    sv.set_protocol(p);
    sv.overlay_script(
        &format!("maps/mp/gametypes/{PROBE}"),
        include_str!("../../gsc/tests/fixtures/semantics/client-probes/probe_cull15.gsc"),
    );
    sv.load_world(vcod_server::world::World::from_bsp(&bsp, Some(&fs)));
    sv.load_scripts(Rc::new(fs)).expect("load the scripts");
    common::run_alone(&mut sv, &mut now, common::PROBE_JOIN_MS);
    let q = Rc::new(RefCell::new(common::Queues::default()));
    let mut cl = common::connect(&mut sv, &q, &mut now);
    let mut snaps: Vec<(i32, Ents)> = Vec::new();
    let mut seen = i32::MIN;
    for _ in 0..(70_000 / common::FRAME_MS) {
        now += Duration::from_millis(common::FRAME_MS as u64);
        cl.send_frame(&NULL_USERCMD);
        common::step(&mut sv, &q, &mut cl, now);
        if let Some(s) = cl.snapshots().newest()
            && s.server_time != seen
        {
            seen = s.server_time;
            let ents: Ents = s
                .entities
                .iter()
                .map(|(n, e)| (n, e.field_i32(p, "eType")))
                .filter(|&(_, t)| p.event_of(t).is_none())
                .map(|(n, t)| format!("{n}:{t}"))
                .collect();
            if snaps.last().is_none_or(|(_, e)| *e != ents) {
                snaps.push((s.server_time, ents));
            }
        }
        if sv
            .script_log()
            .iter()
            .any(|l| l.contains("PROBE step done"))
        {
            break;
        }
    }
    let steps = sv
        .script_log()
        .iter()
        .filter_map(|l| step_line(l))
        .collect();
    (steps, snaps)
}

#[test]
fn the_15_cull_matches_retail_step_for_step() {
    let Some(fs) = vcod_common::testing::game_fs() else {
        return;
    };
    if vcod_server::server::install_protocol(&fs).transport.version != PROTOCOL_V6.transport.version
    {
        eprintln!("COD_DIR is not a 1.5 install: skipping");
        return;
    }
    let (rs, rsn) = retail();
    let (os, osn) = ours(fs);
    let names: Vec<&String> = rs.iter().map(|(n, _)| n).collect();
    assert_eq!(
        names,
        os.iter().map(|(n, _)| n).collect::<Vec<_>>(),
        "steps"
    );
    let (r, o) = (by_step(&rs, &rsn), by_step(&os, &osn));
    let report = std::env::var_os("CULL15_REPORT").is_some();
    if report {
        for name in &names {
            for (side, run) in [("retail", &r[*name]), ("ours", &o[*name])] {
                for (dt, e) in run {
                    println!(
                        "{name} {side} +{dt}: {}",
                        e.iter().cloned().collect::<Vec<_>>().join(" ")
                    );
                }
            }
        }
    }
    for name in names {
        let (r, o) = (&r[name], &o[name]);
        assert_eq!(r.len(), o.len(), "{name}: lists in the step");
        for ((rt, a), (ot, b)) in r.iter().zip(o) {
            assert!(
                a == b,
                "{name} at +{rt}: retail only {:?}, ours only {:?}",
                a.difference(b).collect::<Vec<_>>(),
                b.difference(a).collect::<Vec<_>>()
            );
            assert_eq!(rt, ot, "{name}: when the list changed");
        }
    }
}

/// 1.5b's `SV_inSnapshot` on `client-probes/probe_snapcull`, as UO's: an
/// eye in a solid leaf is sent nothing, a teammate in no cluster is on the
/// compass, a sounded one is off it until the snapshot after his broadcast
/// time runs out, and one in the PVS past a cull fog is on it and off it
/// again while sounded (docs/research/cod11-hud-protocol.md 9).
#[test]
fn the_15_solid_eye_fog_and_compass_match_retail() {
    let Some(fs) = vcod_common::testing::game_fs() else {
        return;
    };
    if vcod_server::server::install_protocol(&fs).transport.version != PROTOCOL_V6.transport.version
    {
        eprintln!("COD_DIR is not a 1.5 install: skipping");
        return;
    }
    common::snapcull::matches(
        "tests/fixtures/entities/snapcull-carentan-15.txt",
        fs,
        &PROTOCOL_V6,
    );
}
