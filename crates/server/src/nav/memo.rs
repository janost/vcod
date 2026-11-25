//! The walks one build proved, kept for the next build of the same map. A
//! walk's answer rides only the collision its queries swept
//! ([`recorded`]), so a rebuild after the script linked or unlinked
//! brushes, or a vehicle moved, takes each of the last build's walks whose
//! box no change meets and walks the rest. The few probes a build runs by
//! the hundred thousand outside a walk ([`settle`],
//! [`beside_a_fall_that_hurts`]) are kept the same way. The graph code
//! runs as it always does, so the graph is the one a full build makes
//! (bot-navigation.md 2.5.1).

use super::*;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};
use vcod_common::collision::{ClipState, Reach, reach, recorded};

/// A memoized call's inputs bit for bit, and which call.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct Key([u32; 10]);

/// What a memoized call answered.
#[derive(Clone, Copy)]
pub(super) enum Answer {
    Walk(Walked),
    Spot(Option<Vec3>),
    Flag(bool),
}

/// The calls besides a walk, in a key's last word past [`Key::walk`]'s flags.
#[derive(Clone, Copy)]
pub(super) enum Probe {
    Settle = 1 << 12,
    BesideAFall = 2 << 12,
}

impl Key {
    /// A walk's ([`walk_with`]), with the thread switches it reads
    /// ([`as_a_bot`]'s and [`walk_to`]'s).
    pub(super) fn walk(
        from: Vec3,
        pace: Option<Vec3>,
        target: glam::Vec2,
        floor: Option<f32>,
        gait: Gait,
        bob: bool,
        roomless: bool,
    ) -> Key {
        let [px, py, pz] = pace.unwrap_or(Vec3::ZERO).to_array().map(f32::to_bits);
        let flags = gait as u32
            | (pace.is_some() as u32) << 8
            | (floor.is_some() as u32) << 9
            | (bob as u32) << 10
            | (roomless as u32) << 11;
        Key([
            from.x.to_bits(),
            from.y.to_bits(),
            from.z.to_bits(),
            px,
            py,
            pz,
            target.x.to_bits(),
            target.y.to_bits(),
            floor.unwrap_or(0.0).to_bits(),
            flags,
        ])
    }

    /// A probe's at `p`, with `arg` (a yaw) where it takes one.
    pub(super) fn probe(probe: Probe, p: Vec3, arg: f32) -> Key {
        let [x, y, z] = p.to_array().map(f32::to_bits);
        Key([x, y, z, arg.to_bits(), 0, 0, 0, 0, 0, probe as u32])
    }

    fn shard(&self) -> usize {
        let h = self
            .0
            .iter()
            .fold(0u32, |h, &x| h.rotate_left(5) ^ x)
            .wrapping_mul(0x9e37_79b1);
        (h >> (32 - SHARD_BITS)) as usize
    }
}

/// A recorded [`Reach`] in whole units, rounded outward, at half its
/// size: an end past `i16` reads as unbounded.
#[derive(Clone, Copy)]
struct Bounds {
    lo: [i16; 3],
    hi: [i16; 3],
}

impl Bounds {
    fn of(r: Reach) -> Bounds {
        if r.is_empty() {
            return Bounds {
                lo: [i16::MAX; 3],
                hi: [i16::MIN; 3],
            };
        }
        if r.lo.is_nan() || r.hi.is_nan() {
            return Bounds {
                lo: [i16::MIN; 3],
                hi: [i16::MAX; 3],
            };
        }
        // `as` saturates, and an end it saturates reads back unbounded.
        Bounds {
            lo: r.lo.to_array().map(|v| v.floor() as i16),
            hi: r.hi.to_array().map(|v| v.ceil() as i16),
        }
    }

    fn reach(self) -> Reach {
        if (0..3).any(|i| self.lo[i] > self.hi[i]) {
            return Reach::EMPTY;
        }
        let f = |v: i16| match v {
            i16::MIN => f32::NEG_INFINITY,
            i16::MAX => f32::INFINITY,
            v => f32::from(v),
        };
        Reach {
            lo: Vec3::from(self.lo.map(f)),
            hi: Vec3::from(self.hi.map(f)),
        }
    }
}

/// One memoized call: its key, its answer and the box its queries swept.
type Entry = (Key, Answer, Bounds);

const SHARD_BITS: u32 = 6;

/// How far a change's box reaches past a call's before it can touch it:
/// the traces' own slack (`SURFACE_CLIP_EPSILON`, the instances' 1-unit
/// broad phase) with room to spare.
const PAD: f32 = 2.0;

/// A memo past this many calls is not kept: mp_rhinevalley's UO build,
/// the most, makes 2.1 million.
const MAX_CALLS: usize = 8_000_000;

/// What one build's calls answered over the collision as it stood
/// ([`ClipState`]), under its rules and fall heights, sorted by key.
pub(super) struct WalkMemo {
    rules: MoveRules,
    falls: FallHeights,
    state: ClipState,
    calls: Vec<Entry>,
    /// Of the build that made it: the calls taken from the last memo, and
    /// those run.
    pub(super) reused: usize,
    pub(super) walked: usize,
}

impl WalkMemo {
    pub(super) fn len(&self) -> usize {
        self.calls.len()
    }

    /// The memo's heap.
    pub(super) fn bytes(&self) -> usize {
        self.calls.capacity() * std::mem::size_of::<Entry>()
    }

    fn get(&self, key: &Key) -> Option<&Entry> {
        let i = self.calls.binary_search_by(|e| e.0.cmp(key)).ok()?;
        Some(&self.calls[i])
    }
}

/// The memo a build runs under: the last build's calls, with the boxes
/// the collision changed in since, and this build's so far.
pub(super) struct BuildMemo {
    old: Option<(Arc<WalkMemo>, Vec<Reach>)>,
    fresh: Vec<Mutex<Vec<Entry>>>,
    reused: AtomicUsize,
    walked: AtomicUsize,
}

thread_local! {
    /// The memo a build's calls on this thread go through
    /// ([`build_reusing`]), carried to [`par_map`]'s workers.
    static MEMO: std::cell::RefCell<Option<Arc<BuildMemo>>> = const { std::cell::RefCell::new(None) };
}

/// This thread's build memo, for a worker to [`enter`].
pub(super) fn current() -> Option<Arc<BuildMemo>> {
    MEMO.with_borrow(Clone::clone)
}

/// Sets this thread's build memo.
pub(super) fn enter(memo: Option<Arc<BuildMemo>>) {
    MEMO.set(memo);
}

/// `call`'s answer under the build memo in force, else `call` run.
fn memoized(key: impl FnOnce() -> Key, call: impl FnOnce() -> Answer) -> Answer {
    match current() {
        Some(m) => m.answer(key(), call),
        None => call(),
    }
}

/// A walk's answer ([`memoized`]).
pub(super) fn walked(key: impl FnOnce() -> Key, walk: impl FnOnce() -> Walked) -> Walked {
    match memoized(key, || Answer::Walk(walk())) {
        Answer::Walk(w) => w,
        _ => unreachable!("a walk's key answers a walk"),
    }
}

/// A spot probe's answer ([`memoized`]).
pub(super) fn spot(
    p: Vec3,
    arg: f32,
    probe: Probe,
    f: impl FnOnce() -> Option<Vec3>,
) -> Option<Vec3> {
    match memoized(|| Key::probe(probe, p, arg), || Answer::Spot(f())) {
        Answer::Spot(s) => s,
        _ => unreachable!("a spot probe's key answers a spot"),
    }
}

/// A yes-or-no probe's answer ([`memoized`]).
pub(super) fn flag(p: Vec3, probe: Probe, f: impl FnOnce() -> bool) -> bool {
    match memoized(|| Key::probe(probe, p, 0.0), || Answer::Flag(f())) {
        Answer::Flag(b) => b,
        _ => unreachable!("a flag probe's key answers a flag"),
    }
}

impl BuildMemo {
    fn keep(&self, e: Entry) {
        self.fresh[e.0.shard()]
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(e);
    }

    fn answer(&self, key: Key, call: impl FnOnce() -> Answer) -> Answer {
        if let Some((old, changed)) = &self.old
            && let Some(&(_, a, b)) = old.get(&key)
            && !changed.iter().any(|c| b.reach().meets(c, PAD))
        {
            self.keep((key, a, b));
            self.reused.fetch_add(1, Relaxed);
            reach(b.reach());
            return a;
        }
        let (a, r) = recorded(call);
        self.keep((key, a, Bounds::of(r)));
        self.walked.fetch_add(1, Relaxed);
        a
    }
}

/// Runs `build` (a graph build over `world` moving by `rules`, under this
/// thread's fall heights) with its calls memoized, taking `prev`'s where
/// nothing that changed since it was made reaches them. Returns the graph
/// and the memo for the next build; none under a game's own step
/// (`stepped`, [`with_step`]), whose queries the memo does not vouch for.
pub(super) fn build_reusing(
    prev: Option<Arc<WalkMemo>>,
    world: &CollisionWorld,
    rules: MoveRules,
    stepped: bool,
    build: impl FnOnce() -> NavGraph,
) -> (NavGraph, Option<WalkMemo>) {
    if stepped {
        return (build(), None);
    }
    let falls = falls();
    let state = world.clip_state();
    let old = prev
        .filter(|m| m.rules == rules && m.falls == falls)
        .map(|m| {
            let changed = m.state.changed(&state);
            (m, changed)
        });
    let memo = Arc::new(BuildMemo {
        old,
        fresh: (0..1 << SHARD_BITS).map(|_| Mutex::default()).collect(),
        reused: AtomicUsize::new(0),
        walked: AtomicUsize::new(0),
    });
    struct Leave;
    impl Drop for Leave {
        fn drop(&mut self) {
            enter(None);
        }
    }
    enter(Some(memo.clone()));
    let leave = Leave;
    let g = build();
    drop(leave);
    let mut calls = Vec::new();
    for s in &memo.fresh {
        calls.append(&mut s.lock().unwrap_or_else(|e| e.into_inner()));
    }
    // A call made twice answers alike both times.
    calls.sort_unstable_by_key(|e| e.0);
    calls.dedup_by_key(|e| e.0);
    calls.shrink_to_fit();
    let out = WalkMemo {
        rules,
        falls,
        state,
        calls,
        reused: memo.reused.load(Relaxed),
        walked: memo.walked.load(Relaxed),
    };
    log::info!(
        "nav memo: {} calls reused, {} run, {} kept in {} MB",
        out.reused,
        out.walked,
        out.len(),
        out.bytes() >> 20
    );
    (g, (out.len() <= MAX_CALLS).then_some(out))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every field a build fills, bit for bit.
    fn assert_same(a: &NavGraph, b: &NavGraph, what: &str) {
        let bits = |g: &NavGraph| -> Vec<[u32; 3]> {
            g.nodes
                .iter()
                .map(|p| p.to_array().map(f32::to_bits))
                .collect()
        };
        assert_eq!(bits(a), bits(b), "{what}: nodes");
        assert_eq!(a.edges, b.edges, "{what}: edges");
        assert_eq!(a.perches, b.perches, "{what}: perches");
        assert_eq!(a.seeds, b.seeds, "{what}: seeds");
        assert_eq!(a.comp, b.comp, "{what}: components");
        assert_eq!(a.members, b.members, "{what}: component members");
        assert_eq!(a.reach, b.reach, "{what}: reach");
        assert_eq!(a.safe, b.safe, "{what}: safe");
        assert_eq!(a.spacing.to_bits(), b.spacing.to_bits(), "{what}: spacing");
    }

    /// mp_foy's UO graph as a `bas` server builds it, rebuilt over the
    /// last build's walks after each change a round makes: a base's walls
    /// deleted, then a second's, a vehicle's collmap parked and moved. Each
    /// is the graph a full build over the same collision makes.
    #[test]
    #[ignore = "builds mp_foy's UO graph nine times, ~30 s in the test profile; needs COD_DIR at a UO install"]
    fn a_rebuild_over_the_last_builds_walks_is_a_full_build() {
        let Some(fs) = vcod_common::testing::uo_fs() else {
            return;
        };
        let entry = fs.resolve_map("mp_foy").expect("mp_foy in the UO paks");
        let bsp = vcod_common::bsp::parse(&fs.read(&entry).unwrap()).unwrap();
        let world = crate::world::World::from_bsp(&bsp, Some(&fs));
        let c = &world.collision;
        c.unlink_script_brushes(&bsp.entities, "bas");
        for &m in &world.rubble {
            c.set_model_linked(m, false);
        }
        // Each base's walls and rubble: the brush models it targets, with a
        // `script_idnumber` and `script_noteworthy rubble`.
        let blocks = vcod_common::bsp::entity_blocks(&bsp.entities);
        let model = |b: &std::collections::HashMap<String, String>| -> Option<usize> {
            b.get("model")?.strip_prefix('*')?.parse().ok()
        };
        let targeted = |key: &str| -> Vec<Vec<usize>> {
            blocks
                .iter()
                .filter(|b| b.get("targetname").is_some_and(|t| t == "gmi_base"))
                .map(|base| {
                    blocks
                        .iter()
                        .filter(|b| b.get("targetname") == base.get("target"))
                        .filter(|b| b.contains_key(key))
                        .filter_map(model)
                        .collect()
                })
                .collect()
        };
        let (walls, rubble) = (targeted("script_idnumber"), targeted("script_noteworthy"));
        assert!(walls.len() >= 2 && walls.iter().all(|w| !w.is_empty()));
        let full = || {
            NavGraph::build_opts(
                c,
                &world.spawn_points,
                &world.nav_cover,
                &world.hazards,
                MoveRules::Uo,
                None,
                None,
                true,
            )
        };
        let t = std::time::Instant::now();
        let (mut last, memo) = build_reusing(None, c, MoveRules::Uo, false, full);
        let mut memo = Arc::new(memo.expect("a memo"));
        eprintln!(
            "first build: {} ms, {} calls, memo {} MB",
            t.elapsed().as_millis(),
            memo.len(),
            memo.bytes() >> 20
        );
        // The breached base's rubble as a vehicle's collmap, parked where
        // the rubble spawned, in the stairwell the breach opened.
        let collmap = rubble[0][0];
        let spawn = blocks
            .iter()
            .find(|b| model(b) == Some(collmap))
            .and_then(|b| vcod_common::bsp::parse_vec3(b.get("origin")?))
            .map_or(Vec3::ZERO, Vec3::from);
        // Each step: the walls it deletes, or where it parks the vehicle.
        let steps: [(&str, &[usize], Option<Vec3>); 4] = [
            ("a base breached", &walls[0], None),
            ("a second base breached", &walls[1], None),
            ("a vehicle parked", &[], Some(spawn)),
            (
                "the vehicle moved",
                &[],
                Some(spawn + Vec3::new(24.0, 0.0, 0.0)),
            ),
        ];
        for (what, unlink, park) in steps {
            unlink.iter().for_each(|&m| c.set_model_linked(m, false));
            if let Some(at) = park {
                c.set_model_instance(900, collmap, Some((at, Vec3::ZERO, 1)));
            }
            let t = std::time::Instant::now();
            let (g, next) = build_reusing(Some(memo.clone()), c, MoveRules::Uo, false, full);
            let quick = t.elapsed();
            let t = std::time::Instant::now();
            let fresh = full();
            let slow = t.elapsed();
            let next = next.expect("a memo");
            eprintln!(
                "{what}: {} nodes, {} edges; rebuild {} ms ({} calls reused, {} run), \
                 full build {} ms",
                g.len(),
                g.edge_count(),
                quick.as_millis(),
                next.reused,
                next.walked,
                slow.as_millis()
            );
            assert_same(&g, &fresh, what);
            assert!(
                next.walked > 0 && next.reused > 0,
                "{what}: the change reached calls"
            );
            assert!(
                (g.len(), g.edge_count()) != (last.len(), last.edge_count()),
                "{what}: the graph moved"
            );
            last = g;
            memo = Arc::new(next);
        }
    }
}
