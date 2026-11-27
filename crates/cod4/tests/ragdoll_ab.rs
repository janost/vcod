//! `ragdoll_ab`: retail 1.7's ragdoll against `vcod_cod4::ragdoll`'s ODE
//! world, on the kill capture (`fixtures/ragdoll/kill-mp_crash-1.7.txt`,
//! cod4-player-anims.md 10.5): the capture's start bodies and seed are
//! built into the port's world over mp_crash's clip map, stepped as
//! retail stepped them, and compared step by step.

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
/// Awake to `ragdoll_max_life`, with the bone lines (10.7).
const MAX_LIFE: &str = "kill-mp_crash-1.7-maxlife.txt";

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

#[derive(Clone, Debug)]
struct Dump {
    seed: u32,
    start: BTreeMap<usize, Rec>,
    bones: BTreeMap<usize, BoneLine>,
    steps: Vec<Step>,
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

/// Every MP body model shares the def bones' lengths; the capture's is a
/// marine's (war, autoassign).
const BODY: &str = "body_mp_usmc_assault";

struct Level {
    clip: ClipMap,
    def: Arc<RagdollDef>,
    skel: Skeleton,
    base: Vec<(Vec3, Quat)>,
}

fn level() -> Option<Level> {
    let install = cod4_install()?;
    let (_, walked) = Map::load(&install, "english", "mp_crash").unwrap();
    let clip = ClipMap::from_assets(&walked.assets).unwrap();
    let x = walked
        .assets
        .xmodels
        .iter()
        .find(|m| m.name == BODY)
        .expect(BODY);
    let model = vcod_cod4::anim::skeleton_model(&walked.assets, x).unwrap();
    let skel = Skeleton::build(&[&model]);
    let base = model.bones.iter().map(|b| (b.pos, b.rot)).collect();
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
    let id = r
        .start_from(&l.skel, &l.base, [snap.clone(), snap], 0, &l.clip)
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
    let d = dump(name);
    let run = run(&l, &d);
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
    Some((d, run))
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

/// The take that ran to `ragdoll_max_life`: awake throughout as retail's,
/// idle on its step 266, within 0.02 units of retail's bodies to step 165
/// (measured 0.012) and 0.5 units and 10 degrees after (0.33, 7.9; its
/// bodies jitter at rest over 1 rad/s), the seed retail's through step
/// 189 (measured), its flags retail's every step,
/// the rest pose's spread within 0.2.
#[test]
fn the_max_life_capture_against_the_port() {
    let Some((d, run)) = report(MAX_LIFE) else {
        return;
    };
    assert_eq!((run.asleep, run.idle), (None, Some(266)));
    let early = worst(&run.diffs[..165]);
    assert!(early.pos <= 0.02, "{early:?}");
    let w = worst(&run.diffs);
    assert!(w.pos <= 0.5 && w.turn_deg <= 10.0, "{w:?}");
    assert_eq!(w.flags, 0, "steps whose flags part");
    assert!(
        run.seed_parts.is_none_or(|k| k >= 190),
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
/// 0x5bc4f0, so it carries that rounding: measured at most 3 ulps on a
/// position, 11 on a quaternion, 9e-4 u/s and 5e-5 rad/s), its mass and
/// inertia within 1e-5 of retail's (the base pose here is the model's,
/// not under the corpse's entity).
#[test]
fn the_build_off_retails_snapshots() {
    let Some(l) = level() else { return };
    let d = dump(MAX_LIFE);
    let cur: Vec<Record> = d.bones.values().map(|b| b.cur).collect();
    let prev: Vec<Record> = d.bones.values().map(|b| b.prev).collect();
    let mut r = Ragdolls::new(l.def.clone(), Dvars::default());
    let id = r
        .start_from(&l.skel, &l.base, [cur, prev], d.bones[&0].dt, &l.clip)
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
        assert!(
            !far(&mass, &theirs, 1e-5),
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
