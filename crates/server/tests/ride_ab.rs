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
use vcod_common::net::{NetClient, NetEvent};

const MAP: &str = "mp_carentan";
const SERVER: &str = "tests/fixtures/movers/mp_carentan-dm-ride.txt";
const WIRE: &str = "tests/fixtures/movers/mp_carentan-dm-ride-wire.txt";

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

/// The probe on ours with one allied client standing still, run to its
/// `PROBE done`: the script log and the client's origin per snapshot.
fn run_ours() -> Option<(Vec<String>, Wire)> {
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
            seen.insert(s.server_time, s.ps.origin(&PROTOCOL_V1));
        }
        if sv.script_log().iter().any(|l| l.contains("PROBE done")) {
            break;
        }
    }
    assert_eq!(sv.script_aborts(), Vec::<String>::new());
    Some((sv.script_log().to_vec(), seen))
}

#[test]
fn a_brush_model_mover_carries_and_pushes_players_like_retail() {
    let Some((log, ours_wire)) = run_ours() else {
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
