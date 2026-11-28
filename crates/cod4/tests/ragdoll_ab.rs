//! `ragdoll_ab`: retail 1.7's ragdoll against `vcod_cod4::ragdoll`'s ODE
//! world, on the kill captures (`fixtures/ragdoll/`, cod4-player-anims.md
//! 10.5, 10.7): each capture's start bodies and seed are built into the
//! port's world over mp_crash's clip map, stepped as retail stepped them,
//! and compared step by step; the max-life take's drawn bones are compared
//! frame by frame (10.7.2).

use std::collections::BTreeMap;
use std::sync::Arc;

use glam::{Quat, Vec3};
use vcod_cod4::collision::ClipMap;
use vcod_cod4::map::Map;
use vcod_cod4::ode::Body;
use vcod_cod4::ragdoll::{self, Dvars, RagdollDef, Ragdolls, Record, State, iw};
use vcod_cod4::testing::cod4_install;
use vcod_common::skeleton::Skeleton;

static FIXTURES: vcod_common::manifest::ManifestPath =
    vcod_common::manifest_const!("/tests/fixtures/ragdoll");
/// Asleep on step 79 (cod4-player-anims.md 10.5).
const SLEPT: &str = "kill-mp_crash-1.7.txt";
/// Awake to `ragdoll_max_life`, with the bone, entity and skeleton lines
/// (10.7, 10.7.2).
const MAX_LIFE: &str = "kill-mp_crash-1.7-maxlife.txt";
/// A corpse dropped from 1200 up, its `trace` lines from step 85 to 95
/// (cod4-gsc.md 3.6.9).
const FALL_TRACES: &str = "traces-mp_crash-1.7-fall.txt";
/// A corpse dropped onto mp_carentan's tiled roof at (1165, 1792): the
/// 126 hits of its 3180 sweeps over 266 steps (cod4-gsc.md 3.6.9).
const ROOF_TRACES: &str = "traces-mp_carentan-1.7-roof.txt";
/// The max-life take's corpse, an opfor rifleman's body and head.
const MAX_LIFE_MODELS: [&str; 2] = ["body_mp_arab_regular_assault", "head_mp_arab_regular_suren"];

/// One body as the dump has it.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Rec {
    flags: u32,
    idle: f32,
    pos: [f32; 3],
    q: [f32; 4],
    lvel: [f32; 3],
    avel: [f32; 3],
}

/// One step after the first body: its msec, the seed after it, the
/// slot's state and its bodies by def bone.
#[derive(Clone, Debug)]
struct Step {
    msec: u32,
    seed: u32,
    state: u32,
    bodies: BTreeMap<usize, Rec>,
}

/// A `bone` line: the slot bone's parent and length, the time between
/// the snapshots, the body's `dMass` words and the two snapshots.
#[derive(Clone, Debug)]
struct BoneLine {
    len: f32,
    dt: i32,
    mass: Vec<u32>,
    cur: Record,
    prev: Record,
}

/// The corpse's skeleton after a drawn frame's pose (a `skel` line): the
/// slot's state, the bits of the bones computed, each bone's turn and
/// position in the world, in the DObj's order.
#[derive(Clone, Debug)]
struct Skel {
    frame: u32,
    state: u32,
    bits: [u32; 4],
    world: Vec<(Vec3, Quat)>,
}

impl Skel {
    fn computed(&self, b: usize) -> bool {
        b < 128 && self.bits[b >> 5] & (0x8000_0000 >> (b & 31)) != 0
    }
}

#[derive(Clone, Debug)]
struct Dump {
    seed: u32,
    start: BTreeMap<usize, Rec>,
    bones: BTreeMap<usize, BoneLine>,
    steps: Vec<Step>,
    /// The corpse's entity at the build (`ent`): its origin and turn.
    ent: Option<(Vec3, Quat)>,
    /// The DObj's models and their bones' names (`models`).
    models: Vec<(String, Vec<String>)>,
    skels: Vec<Skel>,
}

fn hex(w: &str) -> u32 {
    u32::from_str_radix(w, 16).expect("hex word")
}

fn floats<const N: usize>(part: &str) -> [f32; N] {
    let v: Vec<f32> = part
        .split_whitespace()
        .map(|w| f32::from_bits(hex(w)))
        .collect();
    v.try_into().expect("field width")
}

fn rec(line: &str) -> (usize, Rec) {
    let parts: Vec<&str> = line.split('|').collect();
    let head: Vec<&str> = parts[0].split_whitespace().collect();
    (
        head[2].parse().expect("bone"),
        Rec {
            flags: hex(head[3]),
            idle: f32::from_bits(hex(head[4])),
            pos: floats(parts[1]),
            q: floats(parts[2]),
            lvel: floats(parts[3]),
            avel: floats(parts[4]),
        },
    )
}

/// The dump's lines, as crates/cod4/tests/fixtures/README.md gives them.
fn parse(text: &str) -> Dump {
    let mut d = Dump {
        seed: 0,
        start: BTreeMap::new(),
        bones: BTreeMap::new(),
        steps: Vec::new(),
        ent: None,
        models: Vec::new(),
        skels: Vec::new(),
    };
    for line in text.lines() {
        let w: Vec<&str> = line.split_whitespace().collect();
        match w.first().copied() {
            Some("world") => d.seed = hex(w[3]),
            Some("start") => {
                let (b, r) = rec(line);
                d.start.insert(b, r);
            }
            Some("step") => d.steps.push(Step {
                msec: w[3].parse().expect("msec"),
                seed: hex(w[9]),
                state: 0,
                bodies: BTreeMap::new(),
            }),
            Some("bone") => {
                let parts: Vec<&str> = line.split('|').collect();
                let snap = |p: &str| {
                    let v: [f32; 7] = floats(p);
                    Record {
                        pos: [v[0], v[1], v[2]],
                        q: [v[3], v[4], v[5], v[6]],
                    }
                };
                d.bones.insert(
                    w[2].parse().expect("bone"),
                    BoneLine {
                        len: f32::from_bits(hex(w[4])),
                        dt: w[5].parse().expect("dt"),
                        mass: parts[1].split_whitespace().map(hex).collect(),
                        cur: snap(parts[2]),
                        prev: snap(parts[3]),
                    },
                );
            }
            Some("slot") => {
                d.steps.last_mut().expect("a step").state = w[3].parse().expect("state")
            }
            Some("body") => {
                let (b, r) = rec(line);
                d.steps.last_mut().expect("a step").bodies.insert(b, r);
            }
            Some("ent") => {
                let parts: Vec<&str> = line.split('|').collect();
                let a: [f32; 9] = floats(parts[3]);
                let axis = |k: usize| Vec3::new(a[3 * k], a[3 * k + 1], a[3 * k + 2]);
                let m = glam::Mat3::from_cols(axis(0), axis(1), axis(2));
                d.ent = Some((Vec3::from(floats::<3>(parts[1])), Quat::from_mat3(&m)));
            }
            Some("models") => {
                d.models = w[2..]
                    .iter()
                    .map(|m| {
                        let (name, bones) = m.split_once(':').expect("model:bones");
                        (name.into(), bones.split(',').map(String::from).collect())
                    })
                    .collect()
            }
            Some("skel") => {
                let parts: Vec<&str> = line.split('|').collect();
                let origin = Vec3::from(floats::<3>(parts[3]));
                let mats: Vec<f32> = parts[5]
                    .split_whitespace()
                    .map(|w| f32::from_bits(hex(w)))
                    .collect();
                d.skels.push(Skel {
                    frame: w[1].parse().expect("frame"),
                    state: w[4].parse().expect("state"),
                    bits: parts[4]
                        .split_whitespace()
                        .map(hex)
                        .collect::<Vec<u32>>()
                        .try_into()
                        .expect("four words of bits"),
                    world: mats
                        .chunks(8)
                        .map(|m| {
                            let q = Quat::from_xyzw(m[0], m[1], m[2], m[3]);
                            let q = if q.length_squared() > 0.0 {
                                q.normalize()
                            } else {
                                q
                            };
                            (Vec3::new(m[4], m[5], m[6]) + origin, q)
                        })
                        .collect(),
                });
            }
            _ => {}
        }
    }
    d
}

fn dump(name: &str) -> Dump {
    parse(&std::fs::read_to_string(format!("{FIXTURES}/{name}")).expect("the ragdoll fixture"))
}

/// The capture as cod4-player-anims.md 10.5 reads it: 12 bodies from seed
/// 0x89abcdef, 17 ms steps, every body asleep on step 79, the slot idle
/// on step 80.
#[test]
fn the_kill_capture_reads_as_its_summary() {
    let d = dump(SLEPT);
    assert_eq!(
        (d.seed, d.start.len(), d.steps.len()),
        (0x89ab_cdef, 12, 80)
    );
    assert!(d.steps.iter().all(|s| s.msec == 17));
    let asleep = d
        .steps
        .iter()
        .position(|s| !s.bodies.is_empty() && s.bodies.values().all(|r| r.flags & 4 != 0));
    assert_eq!(asleep.map(|i| i + 1), Some(79));
    assert_eq!(d.steps[79].state, 5);
    assert!(d.steps[79].bodies.is_empty());
}

/// A take's after-frame sweeps (0x5a6aea, any world: the ragdolls' and
/// worlds 0 and 1's) replayed through `map`'s clip map: each trace's
/// fraction, normal, `startsolid` and `walkable` (trace_t +0x29, +0x2a)
/// retail's to the bit through the client's trace. `take` is the fixture's
/// path. Returns the sweeps, the hits and how many the server's float
/// model parts on, or `None` with no install.
pub(crate) fn sweeps_as_retails(map: &str, take: &str) -> Option<(usize, usize, usize)> {
    let install = cod4_install()?;
    let (_, walked) = Map::load(&install, "english", map).unwrap();
    let clip = ClipMap::from_assets(&walked.assets).unwrap();
    let text = std::fs::read_to_string(take).unwrap();
    let (mut sweeps, mut hits, mut server_parts) = (0, 0, 0);
    for line in text.lines().filter(|l| l.starts_with("trace ")) {
        let p: Vec<&str> = line.split('|').collect();
        let [start, end, a, b] = [1, 2, 3, 4].map(|k| Vec3::from(floats::<3>(p[k])));
        let mask = hex(p[5].trim());
        let w: Vec<u32> = p[6].split_whitespace().map(hex).collect();
        let theirs = (
            w[0],
            [w[1], w[2], w[3]],
            (w[10] >> 8) & 0xff != 0,
            (w[10] >> 16) & 0xff != 0,
        );
        let read = |t: vcod_common::clipmap::ClipTrace| {
            (
                t.fraction.to_bits(),
                t.normal.to_array().map(f32::to_bits),
                t.startsolid,
                t.walkable,
            )
        };
        // the box's two corners (eax and [esp+0xc] at the call), in either order
        let (mins, maxs) = (a.min(b), a.max(b));
        let ours = read(clip.trace_client(start, end, mins, maxs, mask));
        assert_eq!(ours, theirs, "{take}: {line}");
        server_parts += usize::from(read(clip.trace(start, end, mins, maxs, mask)) != theirs);
        sweeps += 1;
        hits += usize::from(w[0] != 1f32.to_bits());
    }
    eprintln!(
        "{take}: {sweeps} sweeps, {hits} hits, {server_parts} parted under the server's model"
    );
    Some((sweeps, hits, server_parts))
}

/// The fall take's sweeps met only brushes; the server's float model parts
/// on 6 of its 29 hits (measured).
#[test]
fn the_ragdoll_sweeps_trace_as_retails() {
    if let Some(counts) = sweeps_as_retails("mp_crash", &format!("{FIXTURES}/{FALL_TRACES}")) {
        assert_eq!(counts, (132, 29, 6));
    }
}

/// The roof take's corpse slid down mp_carentan's tiled roof, triangles
/// of the clip map, to the street: the client's capsule against them
/// (0x4f05c0) gives retail's every hit, the server's model parts on 94 of
/// the 126 (cod4-gsc.md 3.6.9).
#[test]
fn the_roof_sweeps_trace_as_retails() {
    if let Some(counts) = sweeps_as_retails("mp_carentan", &format!("{FIXTURES}/{ROOF_TRACES}")) {
        assert_eq!(counts, (126, 126, 94));
    }
}

/// Every MP body model shares the def bones' lengths; the first two
/// captures' is a marine's (war, autoassign).
const BODY: &str = "body_mp_usmc_assault";

struct Level {
    clip: ClipMap,
    def: Arc<RagdollDef>,
    skel: Skeleton,
    /// The body's base pose, then each later model's own bones' (unread).
    base: Vec<(Vec3, Quat)>,
}

fn level() -> Option<Level> {
    level_of(&[BODY])
}

/// mp_crash, the stock def and the skeleton of `models` (the body first),
/// a head's bones grafted on by name as the client's rig has them.
fn level_of(models: &[&str]) -> Option<Level> {
    let install = cod4_install()?;
    let (_, walked) = Map::load(&install, "english", "mp_crash").unwrap();
    let clip = ClipMap::from_assets(&walked.assets).unwrap();
    let models: Vec<_> = models
        .iter()
        .map(|name| {
            let x = walked
                .assets
                .xmodels
                .iter()
                .find(|m| m.name == *name)
                .expect(name);
            vcod_cod4::anim::skeleton_model(&walked.assets, x).unwrap()
        })
        .collect();
    let skel = Skeleton::build(&models.iter().collect::<Vec<_>>());
    let mut base = vec![(Vec3::ZERO, Quat::IDENTITY); skel.bone_count()];
    for (m, model) in models.iter().enumerate().rev() {
        for (b, &k) in model.bones.iter().zip(skel.model_map(m)) {
            base[k] = (b.pos, b.rot);
        }
    }
    let fs = vcod_common::pk3::Pk3Fs::open_iwd_layers(&install.root().join("main"), &[]).unwrap();
    let cfg = fs.read("ragdoll.cfg").expect("iw_00.iwd's ragdoll.cfg");
    let mut defs = ragdoll::parse_cfg(&String::from_utf8_lossy(&cfg));
    let def = Arc::new(std::mem::take(&mut defs[ragdoll::MP_DEF]));
    Some(Level {
        clip,
        def,
        skel,
        base,
    })
}

/// ODE's `w x y z` as IW's `x y z w`.
fn iwq(q: [f32; 4]) -> [f32; 4] {
    [q[1], q[2], q[3], q[0]]
}

/// The port's slot built off the capture's start: a snapshot whose bone
/// roots and turns place each body where the capture's stood, then every
/// body set to the capture's bits (position, quaternion, velocities,
/// flags, idle time), the seed the capture's.
fn start(l: &Level, d: &Dump) -> (Ragdolls, ragdoll::SlotId) {
    let mut r = Ragdolls::new(l.def.clone(), Dvars::default());
    let snap: Vec<Record> = (0..l.def.bones.len())
        .map(|b| {
            let s = d.start[&b];
            // the body's turn is the bone's, half round z for a mirrored
            // one: either way round reads the same axes
            let q = iwq(s.q);
            let q = if l.def.bones[b].mirror {
                iw::flip(q)
            } else {
                q
            };
            Record { pos: s.pos, q }
        })
        .collect();
    let ent = d.ent.unwrap_or((Vec3::ZERO, Quat::IDENTITY));
    let id = r
        .start_from(&l.skel, &l.base, ent, [snap.clone(), snap], 0, &l.clip)
        .expect("a slot");
    assert_eq!(
        r.state(id),
        Some(State::Simulating),
        "the tunnel test refused it"
    );
    for (&b, s) in &d.start {
        let body = r.body_mut(id, b).expect("a body per def bone");
        body.pos = s.pos;
        body.q = s.q;
        body.r = vcod_cod4::ode::rotation::q_to_r(s.q);
        body.lvel = s.lvel;
        body.avel = s.avel;
        body.flags = s.flags;
        body.adis_timeleft = s.idle;
    }
    r.set_seed(d.seed);
    (r, id)
}

/// One step's difference from the capture, the worst body's.
#[derive(Clone, Copy, Debug, Default)]
struct Diff {
    pos: f32,
    turn_deg: f32,
    lvel: f32,
    avel: f32,
    flags: usize,
}

fn turn_deg(a: [f32; 4], b: [f32; 4]) -> f32 {
    let qa = Quat::from_array(iwq(a));
    let qb = Quat::from_array(iwq(b));
    qa.angle_between(qb).to_degrees()
}

fn diff(ours: &[Option<&Body>], theirs: &BTreeMap<usize, Rec>) -> Diff {
    let mut d = Diff::default();
    for (&b, t) in theirs {
        let Some(o) = ours.get(b).copied().flatten() else {
            d.flags += 1;
            continue;
        };
        let dist = |a: [f32; 3], b: [f32; 3]| Vec3::from(a).distance(Vec3::from(b));
        d.pos = d.pos.max(dist(o.pos, t.pos));
        d.turn_deg = d.turn_deg.max(turn_deg(o.q, t.q));
        d.lvel = d.lvel.max(dist(o.lvel, t.lvel));
        d.avel = d.avel.max(dist(o.avel, t.avel));
        d.flags += usize::from(o.flags != t.flags);
    }
    d
}

/// What a run shows against the capture: each step's [`Diff`], the steps
/// on which every body slept and the slot went idle, and the rest pose.
struct Run {
    diffs: Vec<Diff>,
    /// The first step after which the reorder's seed parts from
    /// retail's: the steps' row counts parted (10.4).
    seed_parts: Option<usize>,
    asleep: Option<usize>,
    idle: Option<usize>,
    rest: Vec<[f32; 3]>,
}

fn run(l: &Level, d: &Dump) -> Run {
    let (mut r, id) = start(l, d);
    let mut out = Run {
        diffs: Vec::new(),
        seed_parts: None,
        asleep: None,
        idle: None,
        rest: Vec::new(),
    };
    for (k, step) in d.steps.iter().enumerate() {
        r.update(step.msec as i32, &l.clip);
        if r.world().seed != step.seed {
            out.seed_parts.get_or_insert(k + 1);
        }
        let bodies = r.bodies(id);
        if r.state(id) == Some(State::Idle) {
            out.idle.get_or_insert(k + 1);
            continue;
        }
        if out.asleep.is_none() && bodies.iter().flatten().all(|b| b.flags & 4 != 0) {
            out.asleep = Some(k + 1);
        }
        if out.asleep.is_none() || out.asleep == Some(k + 1) {
            out.rest = bodies.iter().flatten().map(|b| b.pos).collect();
        }
        if !step.bodies.is_empty() {
            out.diffs.push(diff(&bodies, &step.bodies));
        }
    }
    // past the capture's end, to the port's own rest
    let mut k = d.steps.len();
    while out.idle.is_none() && k < 400 {
        r.update(17, &l.clip);
        k += 1;
        let bodies = r.bodies(id);
        if out.asleep.is_none() && bodies.iter().flatten().all(|b| b.flags & 4 != 0) {
            out.asleep = Some(k);
            out.rest = bodies.iter().flatten().map(|b| b.pos).collect();
        }
        if r.state(id) == Some(State::Idle) {
            out.idle = Some(k);
        }
    }
    out
}

fn spread(ps: &[[f32; 3]]) -> [f32; 3] {
    std::array::from_fn(|k| {
        let (lo, hi) = ps.iter().fold((f32::MAX, f32::MIN), |(lo, hi), p| {
            (lo.min(p[k]), hi.max(p[k]))
        });
        hi - lo
    })
}

fn report(name: &str) -> Option<(Dump, Run)> {
    let l = level()?;
    Some(report_in(&l, name))
}

fn report_in(l: &Level, name: &str) -> (Dump, Run) {
    let d = dump(name);
    let run = run(l, &d);
    for (k, x) in run.diffs.iter().enumerate() {
        eprintln!(
            "{name} step {:3}: pos {:8.4} turn {:7.3} deg lvel {:8.3} avel {:7.3} flags {}",
            k + 1,
            x.pos,
            x.turn_deg,
            x.lvel,
            x.avel,
            x.flags
        );
    }
    let last = d.steps.iter().rev().find(|s| !s.bodies.is_empty()).unwrap();
    let theirs: Vec<[f32; 3]> = last.bodies.values().map(|r| r.pos).collect();
    eprintln!(
        "{name}: seed parts after step {:?}; asleep {:?} idle {:?}; rest root z {:?} (retail {}), spread {:?} (retail {:?})",
        run.seed_parts,
        run.asleep,
        run.idle,
        run.rest.first().map(|p| p[2]),
        last.bodies[&0].pos[2],
        spread(&run.rest),
        spread(&theirs),
    );
    (d, run)
}

/// The worst of `diffs[range]`.
fn worst(diffs: &[Diff]) -> Diff {
    diffs.iter().fold(Diff::default(), |a, d| Diff {
        pos: a.pos.max(d.pos),
        turn_deg: a.turn_deg.max(d.turn_deg),
        lvel: a.lvel.max(d.lvel),
        avel: a.avel.max(d.avel),
        flags: a.flags + d.flags,
    })
}

fn close(ours: [f32; 3], theirs: [f32; 3], tol: f32) -> bool {
    (0..3).all(|k| (ours[k] - theirs[k]).abs() <= tol)
}

/// The take that slept, from its start's bits: no step's body more than
/// 0.25 units or 5 degrees from retail's (measured 0.20 and 4.1, from the
/// first step's 2e-4, cod4-player-anims.md 10.7), the reorder's seed
/// retail's through step 15 (measured), every body's flags
/// retail's on every step (the sleep timers' bit included), asleep on
/// retail's step 79 and idle on its 80 within 2 steps (measured: on
/// them), at rest the root within 0.05 of retail's height and the
/// centres' spread within 0.1 on each axis.
#[test]
fn the_kill_capture_against_the_port() {
    let Some((d, run)) = report(SLEPT) else {
        return;
    };
    let w = worst(&run.diffs);
    assert!(w.pos <= 0.25 && w.turn_deg <= 5.0, "{w:?}");
    assert_eq!(w.flags, 0, "steps whose flags part");
    // the seed holds retail's through step 15 (measured): every step's
    // contact and joint rows as many as retail's
    assert!(
        run.seed_parts.is_none_or(|k| k >= 16),
        "{:?}",
        run.seed_parts
    );
    let near = |a: Option<usize>, b: usize| a.is_some_and(|a| a.abs_diff(b) <= 2);
    assert!(
        near(run.asleep, 79) && near(run.idle, 80),
        "{:?} {:?}",
        run.asleep,
        run.idle
    );
    let theirs: Vec<[f32; 3]> = d.steps[78].bodies.values().map(|r| r.pos).collect();
    assert!((run.rest[0][2] - theirs[0][2]).abs() <= 0.05);
    assert!(close(spread(&run.rest), spread(&theirs), 0.1));
}

/// The take that ran to `ragdoll_max_life`, its joints built in the base
/// pose under its corpse's entity (`ent`, 0x5bc930) as 1.7 builds them
/// (10.7.2): the first step's bodies within 5e-5 units of retail's
/// (measured 1.5e-5; 2e-4 built at the origin) and 0.003 through step 50
/// (0.0015; 0.0041 at the origin), awake throughout as retail's, idle on
/// its step 266, within 0.02 units to step 165 (0.015) and 0.5 units and
/// 10 degrees after (0.31, 7.4; its bodies jitter at rest over 1 rad/s),
/// the seed retail's through step 195 (measured), its flags retail's every
/// step, the rest pose's spread within 0.2.
#[test]
fn the_max_life_capture_against_the_port() {
    let Some(l) = level_of(&MAX_LIFE_MODELS) else {
        return;
    };
    let (d, run) = report_in(&l, MAX_LIFE);
    assert!(d.ent.is_some());
    assert!(run.diffs[0].pos <= 5e-5, "{:?}", run.diffs[0]);
    let first = worst(&run.diffs[..50]);
    assert!(first.pos <= 0.003, "{first:?}");
    assert_eq!((run.asleep, run.idle), (None, Some(266)));
    let early = worst(&run.diffs[..165]);
    assert!(early.pos <= 0.02, "{early:?}");
    let w = worst(&run.diffs);
    assert!(w.pos <= 0.5 && w.turn_deg <= 10.0, "{w:?}");
    assert_eq!(w.flags, 0, "steps whose flags part");
    assert!(
        run.seed_parts.is_none_or(|k| k >= 196),
        "{:?}",
        run.seed_parts
    );
    let last = d.steps.iter().rev().find(|s| !s.bodies.is_empty()).unwrap();
    let theirs: Vec<[f32; 3]> = last.bodies.values().map(|r| r.pos).collect();
    assert!(close(spread(&run.rest), spread(&theirs), 0.2));
}

/// The build off the max-life take's two snapshots (17 ms apart): each
/// body against the capture's start within a few float steps (the dump's
/// latest snapshot is the pose state 4 saved over it that frame,
/// 0x5bc4f0, so it carries that rounding: measured at most 1 ulp on a
/// position, 11 on a quaternion, 9e-4 u/s and 5e-5 rad/s), its mass and
/// inertia retail's bit for bit, built under the corpse's entity as 1.7
/// builds it (built at the origin, within 1e-5: 10.7.2).
#[test]
fn the_build_off_retails_snapshots() {
    let Some(l) = level_of(&MAX_LIFE_MODELS) else {
        return;
    };
    let d = dump(MAX_LIFE);
    let cur: Vec<Record> = d.bones.values().map(|b| b.cur).collect();
    let prev: Vec<Record> = d.bones.values().map(|b| b.prev).collect();
    let mut r = Ragdolls::new(l.def.clone(), Dvars::default());
    let id = r
        .start_from(
            &l.skel,
            &l.base,
            d.ent.expect("the entity at the build"),
            [cur, prev],
            d.bones[&0].dt,
            &l.clip,
        )
        .expect("a slot");
    let bodies = r.bodies(id);
    let caps = r.capsules(id);
    for (&b, s) in &d.start {
        let len = caps[b].0.distance(caps[b].1);
        assert!(
            (len - d.bones[&b].len).abs() < 1e-4,
            "bone {b}: length {len}"
        );
        let o = bodies[b].expect("a body");
        let ulps = |a: &[f32], b: &[f32]| {
            a.iter()
                .zip(b)
                .map(|(x, y)| (x.to_bits() as i64 - y.to_bits() as i64).unsigned_abs())
                .max()
                .unwrap_or(0)
        };
        let m = &d.bones[&b].mass;
        let mass: Vec<f32> = std::iter::once(o.mass.mass)
            .chain(o.mass.c.iter().copied().chain([0.0]))
            .chain(o.mass.i.iter().flat_map(|r| r.iter().copied().chain([0.0])))
            .collect();
        let theirs: Vec<f32> = m.iter().map(|&w| f32::from_bits(w)).collect();
        let far = |a: &[f32], b: &[f32], tol: f32| {
            a.iter()
                .zip(b)
                .any(|(x, y)| (x - y).abs() > tol * y.abs().max(1.0))
        };
        assert!(
            !far(&o.pos, &s.pos, 1e-6) && !far(&o.q, &s.q, 1e-5),
            "bone {b}: {:?} {:?} against {:?} {:?}",
            o.pos,
            o.q,
            s.pos,
            s.q
        );
        assert!(
            !far(&o.lvel, &s.lvel, 2e-3) && !far(&o.avel, &s.avel, 1e-4),
            "bone {b}"
        );
        assert_eq!(
            mass.iter().map(|x| x.to_bits()).collect::<Vec<_>>(),
            *m,
            "bone {b}: mass {mass:?} against {theirs:?}"
        );
        eprintln!(
            "bone {b:2}: pos {} q {} lvel {} avel {} mass {} ulps",
            ulps(&o.pos, &s.pos),
            ulps(&o.q, &s.q),
            ulps(&o.lvel, &s.lvel),
            ulps(&o.avel, &s.avel),
            ulps(&mass, &theirs),
        );
    }
}

/// The max-life take drawn frame by frame (10.7.2). Retail's: the bones the
/// slot does not set move under their parents for 20 frames after the
/// build (the death anim playing on: `j_ball_ri` turns 26.8 degrees), then
/// hold. The port's: the corpse built under its entity off the take's
/// start and stepped as retail stepped it, fed each drawn frame's skeleton
/// as its DObj, draws every such bone under its parent within 1e-3 units
/// and 1e-3 degrees of retail's that frame (measured 1.2e-4, 2e-5), the
/// bones it sets its own.
#[test]
fn the_unset_bones_take_the_frames_locals_under_the_ragdoll() {
    let Some(l) = level_of(&MAX_LIFE_MODELS) else {
        return;
    };
    let d = dump(MAX_LIFE);
    let names: Vec<&str> = d
        .models
        .iter()
        .flat_map(|(_, b)| b.iter().map(String::as_str))
        .collect();
    // each skeleton bone's DObj bone, the first of its name (a head's
    // copies of the body's bones read the same)
    let dobj: Vec<usize> = l
        .skel
        .bones()
        .iter()
        .map(|b| {
            names
                .iter()
                .position(|n| *n == b.name)
                .expect("a DObj bone")
        })
        .collect();
    let set: Vec<usize> = l
        .def
        .bones
        .iter()
        .map(|b| b.p0.as_str())
        .chain(l.def.base_lerp.iter().map(|b| b.bone.as_str()))
        .map(|n| l.skel.bone_index(n).expect("a def bone"))
        .collect();
    let parents: Vec<i32> = l.skel.bones().iter().map(|b| b.parent).collect();
    let under = |p: &[(Vec3, Quat)], b: usize| {
        let (pp, pq) = p[parents[b] as usize];
        let inv = pq.inverse();
        (inv * (p[b].0 - pp), (inv * p[b].1).normalize())
    };
    // the turn between two unit quaternions, exact for small ones
    let deg = |a: Quat, b: Quat| {
        let d = a.inverse() * b;
        (2.0 * d.xyz().length().atan2(d.w.abs())).to_degrees()
    };
    let (mut r, id) = start(&l, &d);
    let frames: Vec<&Skel> = d.skels.iter().filter(|s| s.state >= 4).collect();
    // retail's own: how far each unset bone's locals move from the build's
    // frame, and the last frame they move on
    let unset = |sk: &Skel, b: usize| {
        let p = parents[b];
        p >= 0 && !set.contains(&b) && sk.computed(dobj[b]) && sk.computed(dobj[p as usize])
    };
    let worlds = |sk: &Skel| -> Vec<(Vec3, Quat)> { dobj.iter().map(|&k| sk.world[k]).collect() };
    let first = worlds(frames[0]);
    let mut frozen = (0.0f32, String::new());
    let mut last_move = 0;
    for pair in frames.windows(2) {
        let (a, b) = (worlds(pair[0]), worlds(pair[1]));
        for k in 0..parents.len() {
            if !unset(pair[1], k) {
                continue;
            }
            let off = deg(under(&first, k).1, under(&b, k).1);
            if off > frozen.0 {
                frozen = (off, l.skel.bones()[k].name.clone());
            }
            if deg(under(&a, k).1, under(&b, k).1) > 0.01 {
                last_move = pair[1].frame;
            }
        }
    }
    assert!(frozen.0 > 20.0 && frozen.1 == "j_ball_ri", "{frozen:?}");
    assert_eq!(last_move, frames[0].frame + 20);
    let mut worst = (0.0f32, 0.0f32, String::new(), 0);
    for sk in &frames {
        r.update(17, &l.clip);
        if r.state(id) != Some(State::Simulating) {
            break;
        }
        let world: Vec<(Vec3, Quat)> = dobj.iter().map(|&k| sk.world[k]).collect();
        r.feed(
            id,
            &ragdoll::DObj {
                skel: &l.skel,
                base: &l.base,
                origin: d.ent.unwrap().0,
                turn: d.ent.unwrap().1,
                world: &world,
            },
        );
        let pose = r.pose(id).expect("drawn");
        for b in 0..parents.len() {
            let p = parents[b];
            if p < 0 || set.contains(&b) || !sk.computed(dobj[b]) || !sk.computed(dobj[p as usize])
            {
                continue;
            }
            let (ours, theirs) = (under(pose, b), under(&world, b));
            let dist = ours.0.distance(theirs.0);
            let turn = deg(ours.1, theirs.1);
            if dist > worst.0 || turn > worst.1 {
                worst = (
                    worst.0.max(dist),
                    worst.1.max(turn),
                    l.skel.bones()[b].name.clone(),
                    sk.frame,
                );
            }
        }
    }
    assert!(worst.0 <= 1e-3 && worst.1 <= 1e-3, "{worst:?}");
}
