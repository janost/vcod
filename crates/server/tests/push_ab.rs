//! A brush model mover pushing and carrying items, against the retail
//! capture `movers/mp_carentan-dm-push.txt` (its header carries the recipe):
//! the server's `PROBE` lines under `client-probes/probe_push.gsc`, one
//! `PROBE f` line per server frame of a phase with the three items' origins
//! and the mover's, then the `--probe-items` client's `ITEM` lines, each
//! item's ground and trajectories as sent. Ours runs the same probe with one
//! client joined, and both halves are compared phase by phase, each sample at
//! the same offset from its phase's start. `docs/research/cod11-movers.md`,
//! section 12, is what the run measured.
//!
//! `PUSH_REPORT=1` prints every compared row.
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
const FIXTURE: &str = "tests/fixtures/movers/mp_carentan-dm-push.txt";
const ET_ITEM: i32 = 3;

/// Float formatting, and the landing's random lift: `G_BounceItem` rests an
/// item up to half a unit above its sweep's end (items doc, section 14).
const TOL: f32 = 0.55;

/// The rider's known drift: after the `rotateyaw` pair retail's slab keeps a
/// yaw residual, and a rider it carries from `push_y` on creeps +x about
/// 0.021 a frame (movers doc, section 12; `ride_ab.rs` has the same gap for
/// a player). Ours has no residual, so `a`'s tolerance grows by this much a
/// frame from the start of `push_y`.
const YAW_RESIDUAL_PER_FRAME: f32 = 0.022;

/// How far `a` may sit from retail at retail level time `t`.
fn rider_tol(t: i32, push_y: i32) -> f32 {
    TOL + YAW_RESIDUAL_PER_FRAME * ((t - push_y).max(0) / FRAME_MS as i32) as f32
}

fn report() -> bool {
    std::env::var("PUSH_REPORT").is_ok_and(|v| v == "1")
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

/// One phase of the server half: its start, then per frame the items' and
/// the mover's origins.
#[derive(Default, Debug)]
struct Phase {
    start: Option<i32>,
    frames: Vec<[[f32; 3]; 4]>,
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
                p.frames.push([v[0], v[1], v[2], v[3]]);
            }
            ["at", phase, t] => {
                out.entry(phase.to_string()).or_default().start = Some(t.parse().unwrap());
            }
            _ => {}
        }
    }
    out
}

/// The level time of `PROBE at ride_up`, the first verb.
fn ride_up<'a>(mut lines: impl Iterator<Item = &'a str>) -> i32 {
    lines
        .find_map(|l| l.split("PROBE at ride_up ").nth(1))
        .and_then(|t| t.trim().parse().ok())
        .expect("a ride_up start")
}

fn diff(a: [f32; 3], b: [f32; 3]) -> f32 {
    (0..3).map(|i| (a[i] - b[i]).abs()).fold(0.0, f32::max)
}

/// An item entity as one snapshot carried it: its ground and `pos.trBase`.
type Sent = (i32, [f32; 3]);

/// One run of ours: the script log, and per item entity number what each
/// snapshot carried, by serverTime.
struct Ours {
    log: Vec<String>,
    items: BTreeMap<u32, BTreeMap<i32, Sent>>,
}

/// The probe on ours with one allied client standing still, run to its
/// `PROBE done`.
fn run_ours() -> Option<Ours> {
    let fs = vcod_common::testing::game_fs()?;
    let bsp_path = fs.resolve_map(MAP).expect("map in the mounted paks");
    let bsp = vcod_common::bsp::parse(&fs.read(&bsp_path).unwrap()).unwrap();
    let mut now = Instant::now();
    let mut sv = vcod_server::Server::new(common::cfg(MAP, "probe_push"), now);
    sv.overlay_script(
        "maps/mp/gametypes/probe_push",
        include_str!("../../gsc/tests/fixtures/semantics/client-probes/probe_push.gsc"),
    );
    sv.load_world(vcod_server::world::World::from_bsp(&bsp, Some(&fs)));
    sv.load_scripts(Rc::new(fs)).expect("load the scripts");
    let q = Rc::new(RefCell::new(Queues::default()));
    let mut cl = NetClient::start_with_qport(ClientEnd(q.clone()), now, 0x2001);
    let mut join = Join::new("allies", "m1carbine_mp");
    let mut items: BTreeMap<u32, BTreeMap<i32, Sent>> = BTreeMap::new();
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
            for (num, e) in &s.entities {
                if e.field_i32(p, "eType") == ET_ITEM {
                    let pos = Trajectory::read(e, p, "pos");
                    items.entry(*num).or_default().insert(
                        s.server_time,
                        (e.field_i32(p, "groundEntityNum"), pos.base.into()),
                    );
                }
            }
        }
        if sv.script_log().iter().any(|l| l.contains("PROBE done")) {
            break;
        }
    }
    assert_eq!(sv.script_aborts(), Vec::<String>::new());
    Some(Ours {
        log: sv.script_log().to_vec(),
        items,
    })
}

#[test]
fn a_brush_model_mover_pushes_and_carries_items_like_retail() {
    let Some(ours) = run_ours() else {
        eprintln!("COD_DIR unset or has no main/: skipping");
        return;
    };
    if report() {
        for l in &ours.log {
            println!("ours {l}");
        }
    }
    assert!(
        ours.log.iter().any(|l| l.contains("PROBE done")),
        "the probe never finished on ours"
    );
    let retail_text = std::fs::read_to_string(FIXTURE).unwrap();
    let retail = phases(retail_text.lines());
    let ours_phases = phases(ours.log.iter().map(String::as_str));
    let push_y = retail["push_y"].start.expect("a push_y start");
    // A brush model's `.model` reads "": the BSP's `*N` goes to `s.index`,
    // not to the model byte the field reads (movers doc, section 11).
    let bm = |lines: &mut dyn Iterator<Item = &str>| -> Vec<String> {
        lines
            .filter_map(|l| l.find("PROBE bm ").map(|i| l[i..].to_string()))
            .collect()
    };
    assert_eq!(
        bm(&mut ours.log.iter().map(String::as_str)),
        bm(&mut retail_text.lines())
    );
    let mut bad = Vec::new();
    for (name, r) in &retail {
        let o = ours_phases
            .get(name)
            .unwrap_or_else(|| panic!("ours never ran {name}"));
        assert_eq!(r.frames.len(), o.frames.len(), "{name}: sample count");
        for (k, (rf, of)) in r.frames.iter().zip(&o.frames).enumerate() {
            let t = r.start.unwrap() + k as i32 * FRAME_MS as i32;
            for (what, i) in [("a", 0), ("b", 1), ("c", 2), ("mover", 3)] {
                let tol = if i == 0 { rider_tol(t, push_y) } else { TOL };
                if report() {
                    println!("{name} {k} {what} retail {:?} ours {:?}", rf[i], of[i]);
                }
                if diff(rf[i], of[i]) > tol {
                    bad.push(format!(
                        "{name} frame {k}: {what} retail {:?} ours {:?}",
                        rf[i], of[i]
                    ));
                }
            }
        }
    }
    // The wire half: every ITEM line retail sent for the probe's three items
    // from the first verb on, against what ours sent at the same offset.
    let retail_start = ride_up(retail_text.lines());
    let shift = ride_up(ours.log.iter().map(String::as_str)) - retail_start;
    let probe_items: Vec<u32> = retail_text
        .lines()
        .find_map(|l| l.strip_prefix("PROBE items "))
        .expect("a PROBE items line")
        .split_whitespace()
        .map(|n| n.parse().unwrap())
        .collect();
    let mut compared = 0;
    for l in retail_text.lines().filter(|l| l.starts_with("ITEM ")) {
        let kv = |k: &str| {
            l.split_whitespace()
                .find_map(|t| t.strip_prefix(k))
                .unwrap()
        };
        let (t, num): (i32, u32) = (kv("t=").parse().unwrap(), kv("num=").parse().unwrap());
        if t < retail_start || !probe_items.contains(&num) {
            continue;
        }
        let ground: i32 = kv("ground=").parse().unwrap();
        let pos: Vec<f32> = kv("pos=").split(',').map(|v| v.parse().unwrap()).collect();
        let base = [pos[2], pos[3], pos[4]];
        let Some((og, ob)) = ours.items.get(&num).and_then(|m| m.get(&(t + shift))) else {
            bad.push(format!("item {num} t={t}: not in ours' snapshot"));
            continue;
        };
        compared += 1;
        if report() {
            println!("wire {num} t={t} retail {ground} {base:?} ours {og} {ob:?}");
        }
        let tol = if num == probe_items[0] {
            rider_tol(t, push_y)
        } else {
            TOL
        };
        if *og != ground || diff(*ob, base) > tol {
            bad.push(format!(
                "item {num} t={t}: retail ground {ground} base {base:?} ours {og} {ob:?}"
            ));
        }
    }
    assert!(compared > 300, "only {compared} item snapshots compared");
    assert!(
        bad.is_empty(),
        "{} rows differ:\n{}",
        bad.len(),
        bad.join("\n")
    );
}
