//! `linkTo` on script entities against the retail capture.
//!
//! `movers/mp_carentan-dm-linkto.txt` is the server's `games_mp.log` under
//! `client-probes/probe_linkto.gsc` (the header carries the recipe), one
//! `PROBE f`/`g`/`h` line per server frame of a phase; `-linkto-wire.txt` is
//! the client's view of the linked script_model, entity 171 on retail. Ours
//! runs the same probe as its gametype with one client joined, and both are
//! compared phase by phase, each sample at the same offset from its phase's
//! start. `docs/research/cod11-movers.md` section 15 is what the run
//! measured.
//!
//! `LINKTO_REPORT=1` prints every compared row.
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
const SERVER: &str = "tests/fixtures/movers/mp_carentan-dm-linkto.txt";
const WIRE: &str = "tests/fixtures/movers/mp_carentan-dm-linkto-wire.txt";

/// Float formatting.
const TOL: f32 = 0.02;

/// How far a child on a player's bone may sit from retail's.
const TAG_TOL: f32 = 3.0;

/// The frames after `setPlayerAngles((0, 180, 0))`: retail's body swings
/// round and back over three frames (the tag's yaw reads 179, 270, 45, 22),
/// ours turns only its entity axis for the one frame (movers doc, 15).
const TAG_GAPS: &[(&str, f32)] = &[("pl_turn", 10.0)];

/// Known divergences, `(phase, tolerance)`, each naming the movers doc
/// section that explains it.
const GAPS: &[(&str, f32)] = &[
    // Retail's brush model keeps a sliver of yaw after `rotateyaw(10)` and
    // `rotateyaw(-10)` (section 12); at the children's 2470-unit radius it is
    // 0.1 units, and it outlives the pair.
    ("bm_yaw", 0.15),
    ("bm_yaw_back", 0.15),
    ("bm_x", 0.15),
    ("verb_linked", 0.15),
    ("unlink", 0.15),
    ("bm_x_back", 0.15),
];

fn report() -> bool {
    std::env::var("LINKTO_REPORT").is_ok_and(|v| v == "1")
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

/// One sampled line: its kind (`f`, `g`, `h`) and its vectors, origins and
/// angles alternating.
type Row = (String, Vec<[f32; 3]>);

/// The capture by phase, plus the one-off lines (`linked`, `ents`, ...) by
/// their first token.
#[derive(Default, Debug)]
struct Capture {
    starts: BTreeMap<String, i32>,
    rows: BTreeMap<String, Vec<Row>>,
    once: BTreeMap<String, String>,
}

fn capture<'a>(lines: impl Iterator<Item = &'a str>) -> Capture {
    let mut out = Capture::default();
    for line in lines.filter(|l| !l.starts_with('#')) {
        let Some(rest) = line.find("PROBE ").map(|i| &line[i + 6..]) else {
            continue;
        };
        let tokens: Vec<&str> = rest.split_whitespace().collect();
        match tokens.as_slice() {
            [kind @ ("f" | "g" | "h"), phase, ..] => {
                out.rows
                    .entry(phase.to_string())
                    .or_default()
                    .push((kind.to_string(), vectors(rest)));
            }
            ["at", phase, t] => {
                out.starts.insert(phase.to_string(), t.parse().unwrap());
            }
            [first, ..] => {
                out.once.insert(first.to_string(), rest.to_string());
            }
            [] => {}
        }
    }
    out
}

/// Positions to the unit's hundredth, angles modulo 360.
fn diff(retail: &[[f32; 3]], ours: &[[f32; 3]]) -> f32 {
    let mut worst = 0.0f32;
    for (k, (r, o)) in retail.iter().zip(ours).enumerate() {
        for i in 0..3 {
            let d = (r[i] - o[i]).abs();
            let d = if k % 2 == 1 { d.min(360.0 - d) } else { d };
            worst = worst.max(d);
        }
    }
    worst
}

struct Ours {
    log: Vec<String>,
    aborts: Vec<String>,
    /// The linked script_model's `pos` per snapshot, by serverTime.
    wire: BTreeMap<i32, (Trajectory, Trajectory)>,
}

fn ours() -> Option<&'static Ours> {
    static RUN: std::sync::OnceLock<Option<Ours>> = std::sync::OnceLock::new();
    RUN.get_or_init(run_ours).as_ref()
}

/// The probe on ours with one allied client standing still, run until the
/// cycle's fatal has stopped it.
fn run_ours() -> Option<Ours> {
    let fs = vcod_common::testing::game_fs()?;
    let bsp_path = fs.resolve_map(MAP).expect("map in the mounted paks");
    let bsp = vcod_common::bsp::parse(&fs.read(&bsp_path).unwrap()).unwrap();
    let mut now = Instant::now();
    let mut sv = vcod_server::Server::new(common::cfg(MAP, "probe_linkto"), now);
    sv.overlay_script(
        "maps/mp/gametypes/probe_linkto",
        include_str!("../../gsc/tests/fixtures/semantics/client-probes/probe_linkto.gsc"),
    );
    sv.load_world(vcod_server::world::World::from_bsp(&bsp, Some(&fs)));
    sv.load_scripts(Rc::new(fs)).expect("load the scripts");
    let q = Rc::new(RefCell::new(Queues::default()));
    let mut cl = NetClient::start_with_qport(ClientEnd(q.clone()), now, 0x2001);
    let mut join = Join::new("allies", "m1carbine_mp");
    let mut wire = BTreeMap::new();
    let mut model: Option<u32> = None;
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
        if model.is_none() {
            model = sv.script_log().iter().find_map(|l| {
                let t: Vec<&str> = l.split_whitespace().collect();
                let i = t.iter().position(|w| *w == "b")?;
                (t.get(1) == Some(&"ents") && t.get(2) == Some(&"bz"))
                    .then(|| t[i + 1].parse().ok())?
            });
        }
        if let (Some(n), Some(s)) = (model, cl.snapshots().newest())
            && let Some(e) = s.entities.get(&n)
        {
            let p = &PROTOCOL_V1;
            wire.insert(
                s.server_time,
                (
                    Trajectory::read(e, p, "pos"),
                    Trajectory::read(e, p, "apos"),
                ),
            );
        }
        if !sv.script_aborts().is_empty() {
            break;
        }
    }
    Some(Ours {
        log: sv.script_log().to_vec(),
        aborts: sv.script_aborts(),
        wire,
    })
}

#[test]
fn linked_entities_follow_their_parents_like_retail() {
    let Some(run) = ours() else {
        eprintln!("COD_DIR unset or has no main/: skipping");
        return;
    };
    let retail_text = std::fs::read_to_string(SERVER).unwrap();
    let retail = capture(retail_text.lines());
    let ours = capture(run.log.iter().map(String::as_str));
    let tol = |phase: &str| GAPS.iter().find(|g| g.0 == phase).map_or(TOL, |g| g.1);

    let mut bad = Vec::new();
    // The link itself moves nothing: the pose after the call is the one
    // before it.
    for key in ["linked", "linked_pl"] {
        let (r, o) = (&retail.once[key], &ours.once[key]);
        let d = diff(&vectors(r), &vectors(o));
        if report() {
            println!("{key}: retail {r} ours {o}");
        }
        if d > TOL && key == "linked" {
            bad.push(format!("{key}: retail {r} ours {o}"));
        }
    }
    let mut tag_worst = 0.0f32;
    for (phase, rows) in &retail.rows {
        let o = ours
            .rows
            .get(phase)
            .unwrap_or_else(|| panic!("ours never ran {phase}"));
        assert_eq!(rows.len(), o.len(), "{phase}: sample count");
        for (k, ((rk, rv), (_, ov))) in rows.iter().zip(o).enumerate() {
            // The player phases: a child on a tag rides a bone of the posed
            // body, which ours poses a few degrees off retail's (combat doc,
            // 3), so its origin is held to `TAG_TOL` and its angles not at
            // all; the entity-frame links are held to the unit.
            let tagged = match (phase.starts_with("pl_"), rk.as_str()) {
                (true, "f") => Some(4),
                (true, "h") => Some(0),
                _ => None,
            };
            let d = match tagged {
                Some(i) => {
                    let t = diff(&rv[i..i + 1], &ov[i..i + 1]);
                    tag_worst = tag_worst.max(t);
                    let tol = TAG_GAPS
                        .iter()
                        .find(|g| g.0 == phase)
                        .map_or(TAG_TOL, |g| g.1);
                    if t > tol {
                        bad.push(format!(
                            "{phase} {k} tag: retail {:?} ours {:?}",
                            rv[i], ov[i]
                        ));
                    }
                    let rest = |v: &[[f32; 3]]| -> Vec<[f32; 3]> {
                        v.iter()
                            .enumerate()
                            .filter(|(j, _)| *j != i && *j != i + 1)
                            .map(|(_, x)| *x)
                            .collect()
                    };
                    diff(&rest(rv), &rest(ov))
                }
                None => diff(rv, ov),
            };
            if report() {
                println!("{phase} {k} {rk} retail {rv:?}\n{phase} {k} {rk} ours   {ov:?}");
            }
            if d > tol(phase) {
                bad.push(format!("{phase} {k}: retail {rv:?} ours {ov:?}"));
            }
        }
    }
    if report() {
        println!("worst tag origin {tag_worst}");
    }
    assert!(
        bad.is_empty(),
        "{} rows differ:\n{}",
        bad.len(),
        bad.join("\n")
    );
}

/// The cycle at the end is fatal on both, with retail's message: a failed
/// link on a model-less parent names the model, not the cycle.
#[test]
fn a_link_cycle_on_script_origins_dies_like_retail() {
    let Some(run) = ours() else {
        return;
    };
    assert!(
        run.aborts
            .iter()
            .any(|a| a.contains("failed to link entity since parent has no model")),
        "{:?}",
        run.aborts
    );
    assert!(
        !run.log.iter().any(|l| l.contains("cycle_survived")),
        "the cycle did not stop the thread"
    );
}

/// The linked script_model's own entity, snapshot by snapshot: both groups
/// `TR_INTERPOLATE` at the pose the link computed on that frame, a frame
/// ahead of what script reads.
#[test]
fn the_linked_model_goes_out_interpolated_at_the_frames_pose() {
    let Some(run) = ours() else {
        return;
    };
    let retail_text = std::fs::read_to_string(SERVER).unwrap();
    let retail = capture(retail_text.lines());
    let ours = capture(run.log.iter().map(String::as_str));
    let (rs, os) = (retail.starts["bm_up"], ours.starts["bm_up"]);
    let mut bad = Vec::new();
    let mut seen = 0;
    for line in std::fs::read_to_string(WIRE).unwrap().lines() {
        let t: Vec<&str> = line.split_whitespace().collect();
        if t.first() != Some(&"entity") {
            continue;
        }
        let st: i32 = t[3].parse().unwrap();
        // The wire's own vectors are `[x,y,z]`.
        let v: Vec<[f32; 3]> = line
            .split('[')
            .skip(1)
            .filter_map(|s| {
                let n: Vec<f32> = s
                    .split(']')
                    .next()?
                    .split(',')
                    .filter_map(|x| x.parse().ok())
                    .collect();
                (n.len() == 3).then(|| [n[0], n[1], n[2]])
            })
            .collect();
        let tr_pos: i32 = t[t.iter().position(|w| *w == "pos").unwrap() + 2]
            .parse()
            .unwrap();
        let Some((pos, apos)) = run.wire.get(&(os + st - rs)) else {
            continue;
        };
        seen += 1;
        let d = (pos.base - glam::Vec3::from(v[0])).abs().max_element();
        if report() {
            println!("wire {st} retail {:?} ours {:?}", v[0], pos.base);
        }
        // The yaw residual of `GAPS`, plus the wire's one decimal.
        if pos.tr_type != tr_pos || apos.tr_type != tr_pos || d > 0.2 {
            bad.push(format!("{st}: retail {line}\nours {pos:?} {apos:?}"));
        }
    }
    assert!(seen > 100, "only {seen} snapshots compared");
    assert!(bad.is_empty(), "{} differ:\n{}", bad.len(), bad.join("\n"));
}
