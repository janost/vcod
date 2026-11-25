//! The pmove walks that prove an edge: the walk configuration, the
//! parallel map, the gaits and the jump pin.

use super::*;

thread_local! {
    /// Set on a [`NavJob`]'s thread: its [`par_map`]s leave one core free.
    pub(super) static SPARE_CORE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    /// A [`NavJob`]'s thread's cancel flag, which each [`par_map`] reads.
    pub(super) static CANCEL: std::cell::RefCell<Option<Arc<std::sync::atomic::AtomicBool>>> = const { std::cell::RefCell::new(None) };
    /// The movement rules every walk's body moves by ([`with_rules`]).
    pub(super) static RULES: std::cell::Cell<MoveRules> = const { std::cell::Cell::new(MoveRules::V1_1) };
    /// The fall heights every walk and bot judges a drop by ([`with_falls`]).
    pub(super) static FALLS: std::cell::Cell<FallHeights> = const { std::cell::Cell::new(FallHeights::COD1) };
    /// A game's own step every walk's body moves by in place of CoD1's
    /// pmove ([`with_step`]).
    static STEP: std::cell::RefCell<Option<Arc<dyn WalkStep>>> = const { std::cell::RefCell::new(None) };
    /// Set inside [`walk_to`]: a forward run that stalls bobbing on a steep
    /// face jumps on its next tick on the ground ([`walk_sliding`]).
    static BOB: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    /// Set inside [`as_a_bot`]: a pinned walk jumps without
    /// [`jump_clear`]'s room test, as a bot on a jump edge does.
    static ROOMLESS: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Runs the walk `f` with a bot's jump on a jump edge: out of a bob too
/// ([`BOB`]), and pinned with no room test ([`ROOMLESS`]).
pub(super) fn as_a_bot<R>(f: impl FnOnce() -> R) -> R {
    BOB.set(true);
    ROOMLESS.set(true);
    let r = f();
    BOB.set(false);
    ROOMLESS.set(false);
    r
}

/// Runs `f` with every walk's body moving by `rules`: a graph built for
/// a 1.5 or UO server is proved under that server's pmove, and the
/// follower's walks again from where a bot stands under the same
/// (bot-navigation.md, "Other rules").
pub fn with_rules<R>(rules: MoveRules, f: impl FnOnce() -> R) -> R {
    let was = RULES.replace(rules);
    let r = f();
    RULES.set(was);
    r
}

/// Runs `f` with drops judged by `falls`, the game's
/// `bg_fallDamageMinHeight` and `MaxHeight`: CoD1's 256 and 480 unless a
/// game names its own (CoD4's 128 and 300, `bots::cod4::FALLS`). Read by
/// [`falls`].
pub fn with_falls<R>(falls: FallHeights, f: impl FnOnce() -> R) -> R {
    let was = FALLS.replace(falls);
    let r = f();
    FALLS.set(was);
    r
}

/// The fall heights in force ([`with_falls`]).
pub fn falls() -> FallHeights {
    FALLS.get()
}

/// What a [`WalkStep`] keeps of a walk's body between its cmds, beside the
/// [`PlayerState`] the walk reads: the game's own playerstate.
pub type WalkCarry = Option<Arc<dyn std::any::Any + Send + Sync>>;

/// A game's own move for the walks' bodies, in place of CoD1's pmove
/// under [`MoveRules`]: CoD2's (`crate::bots::cod2::Cod2Walk`), so its
/// graph is proved by a body that moves as its bots do
/// (bot-navigation.md, "Other rules").
pub trait WalkStep: Send + Sync {
    /// Moves `ps` by one cmd over `world`, its clock from `from_ms` to
    /// `cmd.server_time`. The view is `cmd.angles` plus `delta`, which the
    /// step may push as `delta_angles`; `weapons` holds the def of
    /// `ps.weapon`. `carry` is what the step kept after the walk's last
    /// cmd, `None` on its first.
    fn step(
        &self,
        ps: &mut PlayerState,
        delta: &mut [i32; 3],
        carry: &mut WalkCarry,
        from_ms: i32,
        cmd: &UserCmd,
        world: &CollisionWorld,
        weapons: &[Option<vcod_common::weapon::WeaponDef>],
    );

    /// Tells graphs proved by two steps apart in the cache.
    fn key(&self) -> String;
}

/// Runs `f` with every walk's body moving by `step`, `None` for CoD1's
/// pmove under the rules in force ([`with_rules`]).
pub fn with_step<R>(step: Option<Arc<dyn WalkStep>>, f: impl FnOnce() -> R) -> R {
    let was = STEP.replace(step);
    let r = f();
    STEP.set(was);
    r
}

/// The walks' rules, step and fall heights in force on a thread
/// ([`with_rules`], [`with_step`], [`with_falls`]), carried to a worker thread that walks for it: the bots'
/// pass plans on workers under the ticking thread's (`bots::driver::par`).
#[derive(Clone)]
pub(crate) struct WalkRules(MoveRules, Option<Arc<dyn WalkStep>>, FallHeights);

impl WalkRules {
    /// This thread's.
    pub(crate) fn here() -> WalkRules {
        WalkRules(RULES.get(), own_step(), FALLS.get())
    }

    /// Runs `f` under them.
    pub(crate) fn enter<R>(&self, f: impl FnOnce() -> R) -> R {
        with_rules(self.0, || {
            with_falls(self.2, || with_step(self.1.clone(), f))
        })
    }
}

/// The step in force ([`with_step`]).
pub(super) fn own_step() -> Option<Arc<dyn WalkStep>> {
    STEP.with_borrow(Clone::clone)
}

/// A body standing at `p` facing `yaw`, moving by the rules in force.
pub(super) fn body(p: Vec3, yaw: f32) -> PlayerState {
    let mut ps = PlayerState::spawn(p, yaw);
    ps.rules = RULES.get();
    ps
}

/// `f` over `items` on every core (all but one under [`SPARE_CORE`]),
/// results in `items` order. A cancelled [`NavJob`]'s build unwinds here,
/// at its next pass or flood layer.
pub(super) fn par_map<T: Sync, R: Send>(items: &[T], f: impl Fn(&T) -> R + Sync) -> Vec<R> {
    if CANCEL.with_borrow(|c| {
        c.as_ref()
            .is_some_and(|c| c.load(std::sync::atomic::Ordering::Relaxed))
    }) {
        std::panic::resume_unwind(Box::new(Cancelled));
    }
    let cores = std::thread::available_parallelism().map_or(1, |n| n.get());
    let threads = if SPARE_CORE.get() {
        cores.saturating_sub(1).max(1)
    } else {
        cores
    };
    if threads == 1 || items.len() < 16 {
        return items.iter().map(f).collect();
    }
    // Handed out one at a time: a walk into open ground is a few ticks and
    // one along a wall runs out its budget, so even chunks would leave most
    // cores idle behind the slowest.
    let next = std::sync::atomic::AtomicUsize::new(0);
    let mut out: Vec<Option<R>> = (0..items.len()).map(|_| None).collect();
    let rules = RULES.get();
    let falls = FALLS.get();
    let step = own_step();
    let memo = memo::current();
    std::thread::scope(|s| {
        let handles: Vec<_> = (0..threads.min(items.len()))
            .map(|_| {
                let (step, memo, next, f) = (step.clone(), memo.clone(), &next, &f);
                s.spawn(move || {
                    RULES.set(rules);
                    FALLS.set(falls);
                    STEP.set(step);
                    memo::enter(memo);
                    let mut done = Vec::new();
                    loop {
                        let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        let Some(item) = items.get(i) else {
                            memo::enter(None);
                            return done;
                        };
                        done.push((i, f(item)));
                    }
                })
            })
            .collect();
        for h in handles {
            for (i, r) in h.join().expect("a nav walk panicked") {
                out[i] = Some(r);
            }
        }
    });
    out.into_iter()
        .map(|r| r.expect("every item mapped"))
        .collect()
}

/// Idle cmds until a body dropped at `p` stands on something; `None` when it
/// falls out of the world or never lands.
pub(super) fn settle(world: &CollisionWorld, p: Vec3, yaw: f32) -> Option<Vec3> {
    memo::spot(p, yaw, memo::Probe::Settle, || settle_here(world, p, yaw))
}

fn settle_here(world: &CollisionWorld, p: Vec3, yaw: f32) -> Option<Vec3> {
    let mut ps = body(p + Vec3::Z, yaw);
    let idle = UserCmd {
        angles: [0, (yaw * ANGLE2SHORT) as i32, 0],
        ..NULL_USERCMD
    };
    let mut sim = Sim::default();
    for _ in 0..40 {
        sim.tick(world, &mut ps, &idle);
        if ps.on_ground {
            return Some(ps.origin);
        }
    }
    None
}

/// A walk's tick budget for `dist` units: twice the run time, plus slack for
/// a stair or a slide along a wall.
fn run_ticks(dist: f32) -> usize {
    (dist / (vcod_common::pmove::SPEED_RUN * 0.05) * 2.0) as usize + 4
}

/// How far to either side of a node a walk back to it is retried
/// (`NavGraph::link_back`). VERIFIED (measured, `mp_rocket`): the walk west
/// from (11858, 4224, 262) pins on a corner at x 11840 aimed at the node at
/// (11803, 4226), and arrives aimed 8 units north of it.
pub(super) const SIDESTEP: f32 = 8.0;

/// A body on the ground faster than this, units/s, slows onto a creep's
/// top before it backs off ([`Follower::creep_top`]).
pub(super) const CREEP_TOP_PACE: f32 = 100.0;

/// The pace across a creep's way a bot comes onto its top at, off a turn
/// ([`creep_takes_a_turn`]): seed 183's 148 on mp_depot's roof.
const TURN_PACE: f32 = 150.0;

/// How far across from a ladder box a fall is retried backing down.
pub(super) const LADDER_NEAR: f32 = 96.0;

/// The ticks a walk's budget stretches by while the body is in the air:
/// a fall of [`max_drop`] takes 16.
const FALL_TICKS: usize = 18;

/// Inside this of its target, horizontally, a body backing onto a ladder
/// below a ledge creeps: an airborne grab reaches 17 units past the box
/// (`check_ladder_move`, the box shrunk 6 and the trace 8), and at a run the
/// body is past that within the tick it leaves the lip.
const CREEP_RADIUS: f32 = 24.0;

/// The creep's move key, about 16 units/s. The body leaves the lip at 15
/// units past the face and is first low enough to grab (under the ladder's
/// top) a tick later, so two ticks of travel have to stay under 2 units.
pub const CREEP: i8 = 15;

/// The ticks the creep adds to a backing walk's budget.
const CREEP_TICKS: usize = 16;

/// The move key a body backing toward a point `flat` units away sends:
/// full off the ground near it and on a ladder, a creep at the lip.
pub fn back_move(flat: f32, on_ladder: bool) -> i8 {
    if on_ladder || flat > CREEP_RADIUS {
        -127
    } else {
        -CREEP
    }
}

/// The yaw a climb holds: square into the ladder's face, whatever the
/// head's bearing. Aimed at a head off to one side, the body slides along
/// the rungs to the ladder's edge and pins under what overhangs it there
/// (mp_ship's ladder at x 3354, bot-navigation.md "Ladders").
pub fn climb_yaw(ladder_normal: Vec3) -> Option<f32> {
    let n = ladder_normal.truncate();
    (n.length_squared() > 0.01).then(|| (-n.y).atan2(-n.x).to_degrees())
}

/// The heading a creep holds. The lip is a step or two from the point it
/// backs toward, and re-aiming as the body passes over it would turn it
/// round in the air, away from the face its grab traces toward.
#[derive(Default, Clone, Copy)]
pub struct Creep(Option<f32>);

impl Creep {
    /// The yaw to send with move key `forward`: `want` at full speed, the
    /// first creeping tick's yaw for as long as the creep lasts.
    pub fn hold(&mut self, forward: i8, want: f32) -> f32 {
        if forward == -CREEP {
            *self.0.get_or_insert(want)
        } else {
            self.0 = None;
            want
        }
    }

    /// Whether the last key sent was the creep's.
    pub fn creeping(&self) -> bool {
        self.0.is_some()
    }
}

/// How a walk is driven: facing its way, or backing along it facing the
/// other way, which is how a body gets onto a ladder below a ledge (the grab
/// traces along the view, `ladder_move`), at a run or creeping at the lip
/// ([`back_move`]).
#[derive(Clone, Copy, PartialEq)]
pub(super) enum Gait {
    Forward,
    /// [`Gait::Forward`] that also jumps at a lip ([`lip_ahead`]): across a
    /// gap to a ledge (`NavGraph::link_leaps`).
    Leap,
    Backward,
    Creep,
    /// [`Gait::Forward`] that holds crouch from the tick after its jump
    /// until the body lands again: through a window too low for a standing
    /// body (`Edge::crouched`).
    CrouchJump,
}

/// What a walk came to.
#[derive(Clone, Copy)]
pub(super) enum Walked {
    /// Where it came to rest, and whether it took a jump to get there.
    Arrived(Vec3, bool),
    Blocked,
    /// It left the ground and never arrived; a ladder below may still take it.
    Fell,
}

/// [`walk_as`] forward, backward when the forward run fell, and creeping
/// when that fell too. The creep is last because it is slow: a back down
/// off every ledge on a map is most of a build, and only a ladder below
/// needs it, so neither is tried away from one ([`ladder_near`]).
pub(super) fn walk(
    world: &CollisionWorld,
    ladders: &[(Vec3, Vec3)],
    from: Vec3,
    target: glam::Vec2,
) -> Option<(Vec3, bool)> {
    walk_to(world, ladders, from, target, None)
}

/// [`walk`], arriving only on `floor` when given. Only these runs jump
/// out of a bob ([`BOB`]): the flood proves such a way as a jump edge,
/// where a bot's walk on from its body would read one as a way it walks.
pub(super) fn walk_to(
    world: &CollisionWorld,
    ladders: &[(Vec3, Vec3)],
    from: Vec3,
    target: glam::Vec2,
    floor: Option<f32>,
) -> Option<(Vec3, bool)> {
    for gait in [Gait::Forward, Gait::Backward, Gait::Creep] {
        if gait == Gait::Creep && !ladder_near(ladders, from) {
            return None;
        }
        let walked = if gait == Gait::Forward {
            BOB.set(true);
            let w = walk_as(world, from, target, floor, gait);
            BOB.set(false);
            w
        } else {
            walk_as(world, from, target, floor, gait)
        };
        match walked {
            Walked::Arrived(p, jumped) => return Some((p, jumped)),
            Walked::Blocked => return None,
            Walked::Fell => {}
        }
    }
    None
}

/// Whether a creep from `from` also arrives with the body coming onto it
/// off a turn, at [`TURN_PACE`] across the way to `target` either side,
/// where the graph's walk ran straight at the drop. mp_depot's thin ledges
/// at (-2749, -1281, 326) and (-2755, -1150, 326), 182 under the roof, land
/// only the straight run (bot-navigation.md section 3).
pub(super) fn creep_takes_a_turn(
    world: &CollisionWorld,
    from: Vec3,
    target: glam::Vec2,
    floor: Option<f32>,
) -> bool {
    let dir = (target - from.truncate()).normalize_or_zero();
    let across = glam::Vec2::new(-dir.y, dir.x) * TURN_PACE;
    [across, -across].into_iter().all(|v| {
        matches!(
            walk_with(world, from, Some(v.extend(0.0)), target, floor, Gait::Creep),
            Walked::Arrived(..)
        )
    })
}

/// Whether a floor point 32 or 48 units from `q` in any of the eight
/// octants has nothing under it within `bg_fallDamageMinHeight`
/// ([`falls`]).
pub(super) fn beside_a_fall_that_hurts(world: &CollisionWorld, q: Vec3) -> bool {
    memo::flag(q, memo::Probe::BesideAFall, || {
        beside_a_fall_that_hurts_here(world, q)
    })
}

fn beside_a_fall_that_hurts_here(world: &CollisionWorld, q: Vec3) -> bool {
    let hurts = falls().min;
    (0..8).any(|i| {
        let a = (i as f32 * 45.0).to_radians();
        let dir = Vec3::new(a.cos(), a.sin(), 0.0);
        [32.0, 48.0].iter().any(|&r| {
            let top = q + dir * r + Vec3::Z * 16.0;
            let down = top - Vec3::Z * (16.0 + hurts);
            world
                .point_trace(top, down, MASK_PLAYERSOLID, false)
                .fraction
                >= 1.0
        })
    })
}

/// Whether a point 32 to 96 units from a drop's landing `q` in any of the
/// eight octants, in the open from [`MISS_EYE`] over `q`, has no floor
/// within `bg_fallDamageMinHeight` ([`falls`]) under `q`: a run off the drop
/// that misses it carries that far (mp_ship's run onto the ledge at
/// (3521, 289, 760) flew 93 units before it was level with it). From
/// lower, a slope over that ledge hides the way it flew.
pub(super) fn a_miss_hurts(world: &CollisionWorld, q: Vec3) -> bool {
    let hurts = falls().min;
    (0..8).any(|i| {
        let a = (i as f32 * 45.0).to_radians();
        let dir = Vec3::new(a.cos(), a.sin(), 0.0);
        let eye = q + Vec3::Z * MISS_EYE;
        [32.0, 48.0, 64.0, 80.0, 96.0].iter().any(|&r| {
            let top = eye + dir * r;
            let down = top - Vec3::Z * (MISS_EYE + hurts);
            // Not through a wall: past the hull lies nothing at all.
            world
                .point_trace(eye, top, MASK_PLAYERSOLID, false)
                .fraction
                >= 1.0
                && world
                    .point_trace(top, down, MASK_PLAYERSOLID, false)
                    .fraction
                    >= 1.0
        })
    })
}

/// Whether a body at `at` moving at `pace` with no key held leaves the
/// ground and falls more than a jump within [`COAST_TICKS`].
pub(super) fn coasts_off(world: &CollisionWorld, at: Vec3, pace: Vec3) -> bool {
    let mut ps = body(at, 0.0);
    ps.velocity = pace.truncate().extend(0.0);
    let mut sim = Sim::default();
    (0..COAST_TICKS).any(|_| {
        sim.tick(world, &mut ps, &NULL_USERCMD);
        ps.origin.z < at.z - vcod_common::pmove::JUMP_HEIGHT
    })
}

/// How far `p` lies from a ladder box, flat.
pub(super) fn box_flat(b: &(Vec3, Vec3), p: Vec3) -> f32 {
    let q = p.truncate().clamp(b.0.truncate(), b.1.truncate());
    q.distance(p.truncate())
}

/// Whether a ladder box comes within [`LADDER_NEAR`] of `p` across and
/// spans heights from a drop below it to a storey above: the only place
/// a back down off a ledge can find a grab.
pub(super) fn ladder_near(ladders: &[(Vec3, Vec3)], p: Vec3) -> bool {
    ladders.iter().any(|&(lo, hi)| {
        let near = Vec3::new(LADDER_NEAR, LADDER_NEAR, 0.0);
        let (lo, hi) = (lo - near, hi + near);
        (lo.x..=hi.x).contains(&p.x)
            && (lo.y..=hi.y).contains(&p.y)
            && hi.z >= p.z - max_drop()
            && lo.z <= p.z + Z_MERGE
    })
}

/// Runs a body from `from` toward `target`, re-aiming every tick, and
/// reports where it came to rest when it got within [`ARRIVE`] on the
/// ground (and of the height `floor`, when given) without falling past
/// [`max_drop`], and whether it jumped on the way: once where a forward
/// run stalls, or at the lip for a [`Gait::Leap`].
pub(super) fn walk_as(
    world: &CollisionWorld,
    from: Vec3,
    target: glam::Vec2,
    floor: Option<f32>,
    gait: Gait,
) -> Walked {
    walk_with(world, from, None, target, floor, gait)
}

/// [`walk_as`] with the body moving at `pace` to begin with, in place of
/// the gait's own start.
///
/// A run from the gait's own start that slides on ground too steep to stand
/// on arrives only if no spot it slid over drains into a crease: a body
/// left there at rest comes to rest held off the ground. A bot comes onto
/// an edge off a turn and slower than the run, and on such a slope the pace
/// it carries, not its keys, says where it goes (mp_hurtgen's crease,
/// bot-navigation.md section 3).
pub(super) fn walk_with(
    world: &CollisionWorld,
    from: Vec3,
    pace: Option<Vec3>,
    target: glam::Vec2,
    floor: Option<f32>,
    gait: Gait,
) -> Walked {
    memo::walked(
        || memo::Key::walk(from, pace, target, floor, gait, BOB.get(), ROOMLESS.get()),
        || walk_with_here(world, from, pace, target, floor, gait),
    )
}

/// [`walk_with`] walked.
fn walk_with_here(
    world: &CollisionWorld,
    from: Vec3,
    pace: Option<Vec3>,
    target: glam::Vec2,
    floor: Option<f32>,
    gait: Gait,
) -> Walked {
    let mut trail = Trail::default();
    let walked = walk_sliding(world, from, pace, 1.0, target, floor, gait, &mut trail);
    let proving = pace.is_none() && matches!(gait, Gait::Forward | Gait::Backward);
    if proving
        && matches!(walked, Walked::Arrived(..))
        && trail.slid.iter().any(|&p| drains(world, p))
    {
        return Walked::Blocked;
    }
    walked
}

/// Where a body off the ground left to itself comes to rest is off the
/// ground too: it never lands in [`SETTLE_TICKS`] and then stays within a
/// unit of one spot for [`HELD_TICKS`]. A crease, not a bounce on a slope
/// or a fall. A player left on mp_hurtgen's slope slides into its crease
/// and stays (bot-navigation.md section 3).
fn held_off_ground(world: &CollisionWorld, mut ps: PlayerState, mut sim: Sim) -> bool {
    let mut idle = || {
        sim.tick(world, &mut ps, &NULL_USERCMD);
        !ps.on_ground && !ps.on_ladder
    };
    if !(0..SETTLE_TICKS).all(|_| idle()) {
        return false;
    }
    let at = ps.origin;
    (0..HELD_TICKS).all(|_| {
        sim.tick(world, &mut ps, &NULL_USERCMD);
        !ps.on_ground && !ps.on_ladder && ps.origin.distance(at) < 1.0
    })
}

/// Whether a body left at rest at `p` comes to rest held off the ground
/// ([`held_off_ground`]) under a bot's 50 ms cmds or any of a client's
/// three 16/17 ms splits. On mp_hurtgen's slope the answer turns on the
/// cmd split and the last bits of the move: retail holds a body left at
/// (385.42, 2813.30, -29.90) and one a tenth of a unit off it, where one
/// split of ours lets each go (bot-navigation.md section 3, "The drain
/// test's cmd splits").
pub(super) fn drains(world: &CollisionWorld, p: Vec3) -> bool {
    std::iter::once(None)
        .chain(CLIENT_SPLITS.iter().map(Some))
        .any(|split| {
            let sim = Sim {
                split: split.copied(),
                ..Sim::default()
            };
            held_off_ground(world, body(p, 0.0), sim)
        })
}

/// A 60 Hz client's cmds over a 50 ms frame, each phase.
const CLIENT_SPLITS: [[i32; 3]; 3] = [[16, 17, 17], [17, 16, 17], [17, 17, 16]];

const SETTLE_TICKS: usize = 40;

const HELD_TICKS: usize = 10;

/// What a walk passed over: the spots where the body was off the ground
/// against a plane too steep to stand on, and the deepest it went under its
/// start off the ground and off the rungs.
#[derive(Default)]
pub(super) struct Trail {
    pub(super) slid: Vec<Vec3>,
    pub(super) dip: f32,
}

/// [`walk_with`] without the crease test, the body's weapon moving it at
/// `move_scale`; `trail` gathers what it passed over.
pub(super) fn walk_sliding(
    world: &CollisionWorld,
    from: Vec3,
    pace: Option<Vec3>,
    move_scale: f32,
    target: glam::Vec2,
    floor: Option<f32>,
    gait: Gait,
    trail: &mut Trail,
) -> Walked {
    let dist = target.distance(from.truncate());
    let on_floor = |z: f32| floor.is_none_or(|f| (z - f).abs() < ARRIVE);
    if dist < ARRIVE && on_floor(from.z) {
        return Walked::Arrived(from, false);
    }
    // A running start: a following bot never stops at a node, and the
    // spin-up from a standstill was a third of every walk.
    let mut ps = body(from, 0.0);
    let dir = (target - from.truncate()) / dist;
    // A leap starts standing: a bot comes to rest at its foot first.
    if let Some(v) = pace {
        ps.velocity = v.truncate().extend(0.0);
    } else if gait != Gait::Leap {
        ps.velocity = (dir * vcod_common::pmove::SPEED_RUN).extend(0.0);
    }
    let mut sim = Sim::scaled(move_scale);
    let mut budget = run_ticks(dist);
    // A floor-bound walk is a ladder's (`link_ladders`): room to reach it.
    if floor.is_some() {
        budget = budget.max(60);
    }
    if gait == Gait::Creep {
        budget += CREEP_TICKS;
    }
    let mut creep = Creep::default();
    let mut climbed = false;
    // The highest the body held on the rungs: a walk to a floor no higher
    // than its start that goes up a ladder and over the top arrives by a
    // way a bot heading for the target never takes (mp_depot's ladder at
    // (-417, 1112), bot-navigation.md section 3).
    let mut rung_top = f32::MIN;
    let mut fell = false;
    // The highest the body rose since it last stood or held on: a fall is
    // measured from there, as `PM_CrashLand` measures it. A paced walk
    // measures from where the body last stood, in the air only.
    let mut fall_top = from.z;
    let mut support_z = from.z;
    let (max_drop, paced_drop) = (max_drop(), paced_drop());
    let mut tick = 0;
    let mut airborne = false;
    let cap = budget + FALL_TICKS + run_ticks(dist);
    // A forward run pinned on the ground jumps once (`jump_key`); one that
    // still doesn't arrive reports what it came to before the jump.
    let mut jumped = false;
    let mut jump = false;
    let mut fell_before_jump = false;
    let mut pin = PinJump::default();
    let mut crouched = false;
    // A body in the air when the budget runs out is let land: a drop down
    // a steep companionway is slower than the run it replaced.
    while tick < budget || (airborne && tick < budget + FALL_TICKS) {
        tick += 1;
        let before = ps.origin;
        let to = target - ps.origin.truncate();
        let (yaw, forward) = match gait {
            Gait::Forward | Gait::Leap | Gait::CrouchJump => (to.y.atan2(to.x).to_degrees(), 127),
            Gait::Backward => ((-to.y).atan2(-to.x).to_degrees(), -127),
            Gait::Creep => {
                let forward = back_move(to.length(), ps.on_ladder);
                let yaw = (-to.y).atan2(-to.x).to_degrees();
                (creep.hold(forward, yaw), forward)
            }
        };
        // On a ladder the view goes up, where `ladder_move` climbs at full
        // rate (a level view climbs at a third); backing off it climbs down.
        let pitch = if ps.on_ladder { -LADDER_PITCH } else { 0.0 };
        let mut cmd = UserCmd {
            forward,
            up: if std::mem::take(&mut jump) { 127 } else { 0 },
            angles: [(pitch * ANGLE2SHORT) as i32, (yaw * ANGLE2SHORT) as i32, 0],
            ..NULL_USERCMD
        };
        // `PM_CheckJump` refuses a crouched body, so the crouch starts on
        // the cmd after the jump's, held as a retail client holds it.
        crouched &= !ps.on_ground;
        if crouched {
            cmd.wbuttons |= msg::WBUTTON_CROUCH;
            cmd.up = -127;
        }
        crouched |= gait == Gait::CrouchJump && cmd.up > 0;
        if gait == Gait::Leap && !jumped && ps.on_ground {
            // The lip ([`lip_ahead`]): the tick run without the jump, kept
            // when it stays on the ground, run again with it when it does not.
            let (mut trial, mut run) = (ps, sim.clone());
            run.tick(world, &mut trial, &cmd);
            if trial.on_ground || trial.on_ladder {
                (ps, sim) = (trial, run);
            } else {
                jumped = true;
                fell_before_jump = fell;
                budget = budget.max(tick + JUMP_TICKS + run_ticks(to.length()));
                cmd.up = 127;
                sim.tick(world, &mut ps, &cmd);
            }
        } else {
            sim.tick(world, &mut ps, &cmd);
        }
        // Landed past the budget: the run back to the target from there.
        if airborne && ps.on_ground && tick >= budget {
            let rest = run_ticks(target.distance(ps.origin.truncate()));
            budget = budget.max((tick + rest).min(cap));
        }
        airborne = !ps.on_ground && !ps.on_ladder;
        if ps.on_ladder {
            rung_top = rung_top.max(ps.origin.z);
        }
        if ps.sliding() && !ps.on_ladder {
            trail.slid.push(ps.origin);
        }
        let held = ps.on_ground || ps.on_ladder;
        if !held {
            fell = true;
            trail.dip = trail.dip.max(from.z - ps.origin.z);
            fall_top = fall_top.max(ps.origin.z);
        }
        let deep = match pace {
            Some(_) => !held && ps.origin.z < support_z - paced_drop,
            None => ps.origin.z < fall_top - max_drop,
        };
        if deep {
            return Walked::Fell;
        }
        if held {
            fall_top = ps.origin.z;
            support_z = ps.origin.z;
        }
        if ps.on_ground && target.distance(ps.origin.truncate()) < ARRIVE && on_floor(ps.origin.z) {
            if under_ground(world, ps.origin)
                || floor.is_some_and(|f| rung_top > f.max(from.z) + vcod_common::pmove::JUMP_HEIGHT)
            {
                return Walked::Blocked;
            }
            return Walked::Arrived(ps.origin, jumped);
        }
        // Right under a floor more than a step up: re-aimed at a point
        // overhead, the run only jitters and never stalls. Or pinned under
        // a face too steep to stand on, where the step lifts the body onto
        // it and it slides back down, so it is on the ground one tick in a
        // few: it jumps on the next one (mp_berlin's roof gutter,
        // bot-navigation.md "Jumps").
        let armed = matches!(gait, Gait::Forward | Gait::CrouchJump) && !jumped;
        let roomless = ROOMLESS.get();
        // Only a bot's jump waits out a ramp's climb ([`jump_under`]); the
        // flood's walks keep the jump under the top. Taken out of them too,
        // it turned 26 of mp_ship's jump edges plain and dropped a node,
        // and bots stalled and fell at its stairwell and crow's nest
        // (bot-navigation.md, "The jump cue and the re-proof").
        let rose = if roomless {
            ps.origin.z - before.z
        } else {
            0.0
        };
        let under = floor.is_some_and(|f| {
            jump_under(target.distance(ps.origin.truncate()), f - ps.origin.z, rose)
        });
        let clear = || roomless || jump_clear(world, &ps, target, gait == Gait::CrouchJump);
        if pin.before(armed, ps.on_ground, under, clear) {
            jumped = true;
            jump = true;
            fell_before_jump = fell;
            let rest = run_ticks(target.distance(ps.origin.truncate()));
            budget = budget.max(tick + JUMP_TICKS + rest);
            continue;
        }
        // A climb is slower than a run and straight up, so the budget
        // stretches while it goes on, with the run still left past the top,
        // and progress is the height gained.
        let moved = if ps.on_ladder {
            let rest = run_ticks(target.distance(ps.origin.truncate()));
            budget = budget.max(tick + rest).min(LADDER_TICKS + rest);
            (ps.origin.z - before.z).abs() * 4.0
        } else {
            (ps.origin - before).length()
        };
        // Pinned against a wall: two ticks without moving two units (half
        // one, creeping). A run that only slides along a wall, or overshoots
        // off a ledge and turns back, keeps its budget, and so does one
        // hanging in the air at the top of a ladder before it tips over.
        climbed |= ps.on_ladder;
        let least = if forward.abs() < 127 { 0.5 } else { PINNED };
        let hanging = (climbed || jumped) && !ps.on_ground && !ps.on_ladder;
        let bob = gait == Gait::Forward && BOB.get();
        let clear = || roomless || jump_clear(world, &ps, target, gait == Gait::CrouchJump);
        match pin.after(
            armed,
            moved < least && !hanging,
            ps.on_ground,
            ps.sliding(),
            bob,
            clear,
        ) {
            PinTick::Go => {}
            PinTick::Stuck => break,
            PinTick::Jump => {
                jumped = true;
                jump = true;
                fell_before_jump = fell;
                let rest = run_ticks(target.distance(ps.origin.truncate()));
                budget = budget.max(tick + JUMP_TICKS + rest);
            }
        }
    }
    if jumped {
        fell = fell_before_jump;
    }
    if fell { Walked::Fell } else { Walked::Blocked }
}

/// Under this many units a tick a body is pinned: a run against a ledge's
/// face, which a jump may clear.
pub const PINNED: f32 = 2.0;

/// A body this close under a jump edge's top, flat, jumps whatever its pace:
/// past it, a run carries it off the ledge's foot (mp_depot's stair to the
/// documents' step).
const JUMP_UNDER: f32 = 16.0;

/// The ticks a jump stays in the air: 2 * 249.8 / 800 s.
const JUMP_TICKS: usize = 13;

/// Whether a body standing `flat` from a jump edge's top and `rise` under
/// it, having `climbed` the last tick, jumps whatever its pace
/// ([`JUMP_UNDER`]). Not while it still climbs a ramp under the top: off
/// mp_uo_dawnville's ramp at (-195, -16833, 138) that jump flies past the
/// floor at 176 and falls back, where the run on stalls at its foot and
/// jumps onto it (bot-navigation.md, "The jump cue and the re-proof").
pub fn jump_under(flat: f32, rise: f32, climbed: f32) -> bool {
    flat < JUMP_UNDER && rise > vcod_common::pmove::STEPSIZE && climbed < CLIMBING
}

/// A tick's rise at which a body still climbs a ramp ([`jump_under`]).
const CLIMBING: f32 = 1.0;

/// Whether a leap jumps now, its top `flat` away and `rise` above: at the
/// lip ([`lip_ahead`]), as its walk did, and never once on the top.
pub fn leap_cue(lip: bool, flat: f32, rise: f32) -> bool {
    lip && !on_jump_top(flat, rise)
}

/// Whether a body `flat` from a jump edge's top and `rise` under it
/// already stands on it: no jump from there.
pub fn on_jump_top(flat: f32, rise: f32) -> bool {
    flat < REACH && rise.abs() < JUMP_PASS
}

/// The forward walk's jump ([`walk_sliding`]), one tick at a time: on the
/// ground right under a floor above ([`jump_under`]), on the ground out
/// of a bob, or after two ticks pinned (moved under [`PINNED`] units, half
/// one creeping), each only where [`jump_clear`] passes. A bot on a jump
/// edge runs the same rule without the room test (`Bot::think`), and
/// every jump edge but `NavGraph::link_pockets`' is walked again that way
/// ([`NavGraph::reprove_jumps`], [`as_a_bot`]), so it jumps where the walk
/// that proved the edge did.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PinJump {
    stalled: u32,
    bobbing: bool,
}

/// What a [`PinJump`] makes of a tick.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PinTick {
    /// Jump on the next cmd.
    Jump,
    /// Run on.
    Go,
    /// Pinned with no jump: the walk gives up.
    Stuck,
}

impl PinJump {
    /// The jumps that read before the tick's progress: right `under` the
    /// target floor, or out of a bob. `armed` is false once the walk has
    /// jumped or in a gait that never jumps.
    pub fn before(
        &mut self,
        armed: bool,
        on_ground: bool,
        under: bool,
        clear: impl Fn() -> bool,
    ) -> bool {
        let jump = armed && on_ground && (under || self.bobbing) && clear();
        if jump {
            *self = Self::default();
        }
        jump
    }

    /// The tick's progress: `stalled` when it moved under the pinned
    /// pace, not hanging in the air off a jump or a climb. A stall that
    /// cannot jump turns into a bob once where `bob` allows it and the
    /// body slides on a steep face.
    pub fn after(
        &mut self,
        armed: bool,
        stalled: bool,
        on_ground: bool,
        sliding: bool,
        bob: bool,
        clear: impl Fn() -> bool,
    ) -> PinTick {
        if !stalled {
            self.stalled = 0;
            return PinTick::Go;
        }
        self.stalled += 1;
        if self.stalled < 2 {
            return PinTick::Go;
        }
        if armed && on_ground && clear() {
            *self = Self::default();
            PinTick::Jump
        } else if armed && bob && !self.bobbing && sliding {
            self.bobbing = true;
            self.stalled = 0;
            PinTick::Go
        } else {
            PinTick::Stuck
        }
    }
}

/// The jump key for a cue: pressed on the ground when the key was up last
/// cmd. `PM_CheckJump` refuses a held key, so a second try needs a release.
pub fn jump_key(on_ground: bool, cue: bool, held: bool) -> i8 {
    if on_ground && cue && !held { 127 } else { 0 }
}

/// A body at rest on the ground with its jump off cooldown: what a leap was
/// proved from.
pub fn ready_to_leap(ps: &PlayerState) -> bool {
    ps.on_ground
        && ps.velocity.truncate().length() * (FRAME_MS as f32 / 1000.0) < PINNED
        && jump_ready(ps)
}

/// Whether the body's next jump rises the full 39 units the graph's walks
/// jumped: off `PM_CheckJump`'s 500 ms cooldown and, under 1.5's and UO's
/// rules, clear of the jump penalty, which cuts a jump inside 1.8 s of the
/// last landing to as little as 15.6 (cod15-movement.md 2).
pub fn jump_ready(ps: &PlayerState) -> bool {
    ps.since_jump_ms >= vcod_common::pmove::JUMP_COOLDOWN_MS && !ps.jump_penalty
}

/// Key `k` cut to the graph's pace for a weapon moving the body at
/// `move_scale`: a following bot holds this on the ground (`Bot::think`),
/// so a light weapon runs at the 190 units/s the graph's walks ran at.
pub fn paced_key(k: i8, move_scale: f32) -> i8 {
    if move_scale > 1.0 {
        (f32::from(k) / move_scale).round() as i8
    } else {
        k
    }
}

/// [`paced_key`] for the weapon `ps` holds.
pub(super) fn paced_key_for(
    k: i8,
    ps: &PlayerState,
    weapons: &[Option<vcod_common::weapon::WeaponDef>],
) -> i8 {
    let scale = weapons
        .get(ps.weapon as usize)
        .and_then(Option::as_ref)
        .map_or(1.0, |d| d.move_speed_scale);
    paced_key(k, scale)
}

/// The cmd times of one 50 ms tick for a body that starts it at `start`:
/// one, or on the rungs off the ground three to six, the count cycling
/// with the tick, as a client's frames come. Off the top of a ladder the
/// step-up has to clear the lip before `PM_CheckLadderMove`'s box lets go
/// 8 units under the ladder's top, and the step's slide carries the wall
/// glue's 500 units/s: whether it clears depends on the frame's length and
/// phase. In 50 ms frames, or any one length, a body can bob under a lip
/// for good where retail's client, whose frames vary, gets over on the
/// next try (docs/research/bot-navigation.md, "Ladders"). Shared by the
/// bots (`Server::step_bots`) and the graph's walks, so a bot climbs the
/// way the graph proved it.
pub fn tick_times(start: i32, climbing: bool) -> impl Iterator<Item = i32> {
    let n = if climbing {
        LADDER_SUBSTEPS[(start / FRAME_MS).rem_euclid(LADDER_SUBSTEPS.len() as i32) as usize]
    } else {
        1
    };
    (1..=n).map(move |i| start + FRAME_MS * i / n)
}

/// Cmds a climbing body sends a tick, by tick ([`tick_times`]).
const LADDER_SUBSTEPS: [i32; 4] = [5, 4, 3, 6];

/// The bits of a client a walk carries between cmds.
#[derive(Clone, Default)]
pub(super) struct Sim {
    time: i32,
    delta: [i32; 3],
    ring: EventRing,
    /// Weapon 0's def, when the body moves at a `moveSpeedScale` other
    /// than 1 ([`Sim::scaled`]).
    weapons: Option<Arc<[Option<vcod_common::weapon::WeaponDef>]>>,
    /// Off the rungs, the tick's cmds as a client's lengths instead of one
    /// 50 ms cmd ([`drains`]).
    split: Option<[i32; 3]>,
    /// The [`WalkStep`]'s own body, when one is in force.
    carry: WalkCarry,
}

impl Sim {
    /// A walk whose body holds a weapon moving it at `move_scale`, as weapon
    /// 0: the walk's body holds no other.
    fn scaled(move_scale: f32) -> Sim {
        if move_scale <= 0.0 || move_scale == 1.0 {
            return Sim::default();
        }
        let map = vcod_common::xmodel::WeaponKeys::from_iter([(
            "moveSpeedScale",
            move_scale.to_string(),
        )]);
        Sim {
            weapons: Some(Arc::new([Some(vcod_common::weapon::WeaponDef::from_map(
                &map,
            ))])),
            ..Sim::default()
        }
    }

    pub(super) fn tick(&mut self, world: &CollisionWorld, ps: &mut PlayerState, cmd: &UserCmd) {
        let mw = MoveWorld::bare(world);
        let weapons = self.weapons.as_deref().unwrap_or(&[]);
        let climbing = ps.on_ladder && !ps.on_ground;
        // At most a climb's six cmds; no allocation on the walks' hot path.
        let mut times = [0; 6];
        let mut n = 0;
        match self.split {
            Some(split) if !climbing => {
                let mut t = self.time;
                for ms in split {
                    t += ms;
                    times[n] = t;
                    n += 1;
                }
            }
            _ => {
                for t in tick_times(self.time, climbing) {
                    times[n] = t;
                    n += 1;
                }
            }
        }
        // The walk's angles are absolute: net of `delta_angles` as the tick
        // starts, as a bot's aim is (`bots::cmd_angles`), so a ladder yaw
        // cap's push holds the view on the cap instead of turning the rest
        // of the walk.
        let d = self.delta;
        let angles = [0, 1, 2].map(|i| cmd.angles[i] - d[i]);
        let own = own_step();
        for &server_time in &times[..n] {
            let cmd = UserCmd {
                server_time,
                angles,
                ..*cmd
            };
            if let Some(own) = &own {
                own.step(
                    ps,
                    &mut self.delta,
                    &mut self.carry,
                    self.time,
                    &cmd,
                    world,
                    weapons,
                );
                self.time = server_time;
                continue;
            }
            chop(self.time, &cmd, |step, dt| {
                player_step(ps, &mut self.delta, &mut self.ring, step, dt, &mw, weapons);
                ps.jump_latched
            });
            self.time = server_time;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nav::build::tests::map_world;

    /// The run off mp_ship's mast platform at (3564, -127, 896) onto the
    /// narrow ledge at 760 lands at the graph's 190 units/s and flies over
    /// it at a light weapon's 228, to the ledge at 480 that killed 15
    /// bots in 384 seeds: why a following bot runs at the graph's pace
    /// (`paced_key`).
    #[test]
    fn the_run_off_the_mast_platform_lands_only_at_the_graphs_pace() {
        let Some(world) = map_world("mp_ship") else {
            return;
        };
        let top = Vec3::new(3564.0, -127.0, 896.125);
        let ledge = Vec3::new(3520.0, -162.0, 760.0);
        let run = |scale: f32| {
            let pace = (ledge - top).truncate().normalize() * 190.0 * scale;
            walk_sliding(
                &world.collision,
                top,
                Some(pace.extend(0.0)),
                scale,
                ledge.truncate(),
                Some(ledge.z),
                Gait::Forward,
                &mut Trail::default(),
            )
        };
        assert!(matches!(run(1.0), Walked::Arrived(..)));
        assert!(matches!(run(1.2), Walked::Fell));
        assert_eq!(paced_key(127, 1.2), 106);
    }

    /// mp_ship's deck at (3515, -323, 631) has an edge to the narrow ledge
    /// at 580 that no gait lands node to node: bots ran it and fell past
    /// the ledge to the hold. It is risky.
    #[test]
    fn a_drop_no_gait_lands_node_to_node_is_risky() {
        let Some(world) = map_world("mp_ship") else {
            return;
        };
        let g = graph_for("mp_ship", &world);
        let deck = g.nearest([3515.0, -323.0, 631.0]).unwrap();
        let ledge = g.nearest([3488.0, -358.0, 580.0]).unwrap();
        assert!(g.linked(deck, ledge));
        let (p, q) = (g.nodes[deck as usize], g.nodes[ledge as usize]);
        for gait in [Gait::Forward, Gait::Backward, Gait::Creep] {
            assert!(!matches!(
                walk_as(&world.collision, p, q.truncate(), Some(q.z), gait),
                Walked::Arrived(..)
            ));
        }
        assert!(g.has(deck, ledge, |k| k.risky));
    }

    /// mp_hurtgen's slope west of the crease at (437, 2824, -97): a body
    /// left at rest there, or on the slope above it where retail's
    /// `probe_wedge` slid into it, comes to rest in the air; on the patch
    /// of the node (428.2, 2783.2, -60.3) it stands. The run from the node
    /// (385.2, 2831.8, -22.2) down to (385.6, 2785.7, -52.6) arrives, but
    /// over spots that drain into the crease, so it proves no edge.
    #[test]
    fn a_slide_over_ground_that_drains_into_a_crease_does_not_hold() {
        let Some(world) = map_world("mp_hurtgen") else {
            return;
        };
        let w = &world.collision;
        let held = |x: f32, y: f32, z: f32| {
            held_off_ground(
                w,
                PlayerState::spawn(Vec3::new(x, y, z), 0.0),
                Sim::default(),
            )
        };
        assert!(held(436.8, 2824.3, -97.2));
        assert!(held(416.7, 2799.0, -60.0));
        assert!(held(392.1, 2797.3, -55.1));
        assert!(!held(428.2, 2783.2, -59.0));
        // Retail's `probe_wedge` holds a body left at the walk's first slid
        // spot and lands one at its second (bot-navigation.md section 3).
        assert!(drains(w, Vec3::new(385.4227, 2813.2952, -29.899721)));
        assert!(!drains(w, Vec3::new(383.21103, 2802.2563, -42.117294)));
        let node = |x: f32, y: f32, z: f32| settle(w, Vec3::new(x, y, z + 2.0), 0.0).unwrap();
        let (top, patch) = (node(385.2, 2831.8, -22.2), node(385.6, 2785.7, -52.6));
        let mut trail = Trail::default();
        let straight = walk_sliding(
            w,
            top,
            None,
            1.0,
            patch.truncate(),
            None,
            Gait::Forward,
            &mut trail,
        );
        assert!(matches!(straight, Walked::Arrived(..)) && !trail.slid.is_empty());
        let walked = walk_with(w, top, None, patch.truncate(), None, Gait::Forward);
        assert!(!matches!(walked, Walked::Arrived(..)));
    }

    /// mp_depot's ladder at (-417, 1112) up to the roof at 133: a walk
    /// from its foot to the floor at (-484, 1051), 7 units below a bracket
    /// and a wall, climbed the rungs, crossed the roof and dropped back to
    /// the floor beside the target. A bot heading there pins in the corner
    /// at (-433, 1129, -26) instead.
    #[test]
    fn a_level_walk_over_a_ladders_top_proves_no_edge() {
        let Some(world) = map_world("mp_depot") else {
            return;
        };
        let w = &world.collision;
        let (foot, floor) = (
            Vec3::new(-416.5, 1130.0, -31.875),
            Vec3::new(-484.1, 1051.2, -31.875),
        );
        let walked = walk_with(
            w,
            foot,
            None,
            floor.truncate(),
            Some(floor.z),
            Gait::Forward,
        );
        assert!(!matches!(walked, Walked::Arrived(..)));
        // The same walk without a floor arrives on the roof, over it.
        let roof = walk_with(w, foot, None, floor.truncate(), None, Gait::Forward);
        assert!(matches!(roof, Walked::Arrived(p, _) if p.z > 100.0));
    }

    /// mp_depot by (-64, 352): a body run off a ledge at z 12 comes to rest
    /// on a step's corner at z 3, right under a floor at z 27 and under a
    /// ceiling 37 over the feet. A flood walk resting there is not merged
    /// into the floor's node: the walk to the node itself jumps.
    #[test]
    fn a_walk_jumps_onto_a_floor_right_overhead_under_a_ceiling() {
        let Some(world) = map_world("mp_depot") else {
            return;
        };
        let w = &world.collision;
        let from = Vec3::new(-28.519163, 352.00345, 12.125);
        let node = Vec3::new(-64.23049, 352.02036, 27.125);
        let free = walk_as(w, from, node.truncate(), None, Gait::Forward);
        assert!(matches!(free, Walked::Arrived(p, false) if (p.z - 3.125).abs() < 0.5));
        let on = walk_as(w, from, node.truncate(), Some(node.z), Gait::Forward);
        assert!(matches!(on, Walked::Arrived(p, true) if (p.z - node.z).abs() < 0.5));
    }

    #[test]
    fn a_creep_holds_its_first_heading() {
        let mut c = Creep::default();
        assert_eq!(back_move(100.0, false), -127);
        assert_eq!(c.hold(-127, 10.0), 10.0);
        assert_eq!(back_move(20.0, false), -CREEP);
        assert_eq!(c.hold(-CREEP, 20.0), 20.0);
        assert_eq!(c.hold(-CREEP, 200.0), 20.0, "turned round over the lip");
        assert_eq!(back_move(20.0, true), -127, "full speed down the ladder");
        assert_eq!(c.hold(-127, 30.0), 30.0);
        assert_eq!(c.hold(-CREEP, 40.0), 40.0, "a new creep, a new heading");
    }

    /// The walk's jump: two pinned ticks where a jump has room, a bob
    /// first where it has none and the body slides, then the next tick on
    /// the ground; right under the top at once.
    #[test]
    fn the_walks_jump_needs_two_pinned_ticks_and_room() {
        let (yes, no) = (|| true, || false);
        let mut p = PinJump::default();
        assert_eq!(p.after(true, true, true, false, false, yes), PinTick::Go);
        assert_eq!(
            p.after(true, false, true, false, false, yes),
            PinTick::Go,
            "moved"
        );
        assert_eq!(p.after(true, true, true, false, false, yes), PinTick::Go);
        assert_eq!(p.after(true, true, true, false, false, yes), PinTick::Jump);
        let mut p = PinJump::default();
        p.after(true, true, true, false, false, no);
        assert_eq!(
            p.after(true, true, true, false, false, no),
            PinTick::Stuck,
            "no room"
        );
        let mut p = PinJump::default();
        p.after(true, true, false, true, true, no);
        assert_eq!(
            p.after(true, true, false, true, true, no),
            PinTick::Go,
            "a bob"
        );
        assert!(!p.before(true, false, false, yes), "off the ground");
        assert!(p.before(true, true, false, yes), "out of the bob");
        let mut p = PinJump::default();
        assert!(p.before(true, true, true, yes), "right under the top");
        assert!(!p.before(false, true, true, yes), "disarmed");
    }

    #[test]
    fn a_leap_jumps_only_at_the_lip() {
        // flat, rise: the top 40 off and 32 up.
        assert!(!leap_cue(false, 40.0, 32.0), "pinned is no cue");
        assert!(leap_cue(true, 40.0, 32.0));
        // Right under a ledge's top, not under a step.
        assert!(jump_under(10.0, 32.0, 0.0));
        assert!(!jump_under(30.0, 32.0, 0.0), "still short of it");
        assert!(!jump_under(10.0, 10.0, 0.0), "under a step");
        assert!(!jump_under(10.0, 32.0, 2.0), "still climbing a ramp");
        // On the top: no more jumping.
        assert!(!leap_cue(true, 10.0, 2.0));
        assert_eq!(jump_key(true, true, false), 127);
        assert_eq!(jump_key(true, true, true), 0, "a held key is refused");
        assert_eq!(jump_key(false, true, false), 0);
    }

    /// mp_depot's documents lie on a crate top at z 148 across a gap from a
    /// step at 116: a run off the step falls, a standing-start leap from it
    /// lands on the crate.
    #[test]
    fn depots_crate_takes_a_leap() {
        let Some(world) = map_world("mp_depot") else {
            return;
        };
        let w = &world.collision;
        let step = settle(w, Vec3::new(-1820.0, -609.0, 120.0), 0.0).expect("the step");
        assert!((step.z - 116.125).abs() < 0.5, "step at {step}");
        let top = glam::Vec2::new(-1888.0, -537.0);
        assert!(matches!(
            walk_as(w, step, top, Some(148.125), Gait::Forward),
            Walked::Fell
        ));
        assert!(matches!(
            walk_as(w, step, top, Some(148.125), Gait::Leap),
            Walked::Arrived(p, true) if (p.z - 148.125).abs() < 0.5
        ));
    }

    /// mp_ship's slot between the stair's top and the shelf: the graph's
    /// walk from the shelf node, a running start straight at the floor
    /// 192 under it, lands. A bot that comes along the shelf at a run and
    /// turns onto the drop falls past the stair: the node is held while the
    /// walk at the body's pace fails, and passed from at rest on it.
    #[test]
    fn a_drop_is_proved_again_at_the_pace_the_bot_comes_onto_it() {
        let Some(world) = map_world("mp_ship") else {
            return;
        };
        let w = &world.collision;
        let shelf = Vec3::new(5402.5635, -160.24321, 396.125);
        let slot = Vec3::new(5404.874, -192.03677, 204.125);
        assert!(matches!(
            walk_as(w, shelf, slot.truncate(), Some(slot.z), Gait::Forward),
            Walked::Arrived(_, false)
        ));
        let g = NavGraph::from_parts(vec![shelf, slot], vec![vec![1], vec![]]);
        let mut f = Follower::default();
        let goal = crate::bots::Goal::To(slot.into());
        let mut budget = 100;
        let mut at = |p: Vec3, velocity: Vec3| {
            let body = Footing {
                still: velocity == Vec3::ZERO,
                on_ground: true,
                on_ladder: false,
                velocity,
            };
            f.waypoint(&g, Some(w), goal, p.into(), body, &mut budget, &mut || 0)
        };
        let run = Vec3::new(-218.0, -2.0, 0.0);
        assert_eq!(
            at(Vec3::new(5417.173, -160.85359, 396.125), run),
            Some(shelf.into())
        );
        assert_eq!(
            at(shelf, Vec3::ZERO),
            Some(slot.into()),
            "from at rest on the node"
        );
    }

    /// mp_depot's thin ledge (-2749, -1281, 326), 182 under the roof's node
    /// (-2750, -1244, 508) by the ladder at x -2750: only a creep from the
    /// node lands on it. Seed 183's bot came onto it off a turn, running
    /// west at 148 units/s, and fell past the ledge to the yard. The node is
    /// held until the body is under [`CREEP_TOP_PACE`].
    #[test]
    fn a_creeps_top_is_passed_only_slow() {
        let Some(world) = map_world("mp_depot") else {
            return;
        };
        let w = &world.collision;
        let roof = Vec3::new(-2750.1372, -1244.103, 508.125);
        let ledge = Vec3::new(-2748.646, -1280.8044, 326.125);
        let to = ledge.truncate();
        for gait in [Gait::Forward, Gait::Backward] {
            assert!(matches!(
                walk_as(w, roof, to, Some(ledge.z), gait),
                Walked::Fell
            ));
        }
        for pace in [None, Some(Vec3::ZERO)] {
            assert!(matches!(
                walk_with(w, roof, pace, to, Some(ledge.z), Gait::Creep),
                Walked::Arrived(..)
            ));
        }
        let g = NavGraph::from_parts(vec![roof, ledge], vec![vec![1], vec![]]);
        let mut f = Follower::default();
        let goal = crate::bots::Goal::To(ledge.into());
        let mut budget = 100;
        let mut at = |f: &mut Follower, p: Vec3, velocity: Vec3| {
            let body = Footing {
                still: velocity == Vec3::ZERO,
                on_ground: true,
                on_ladder: false,
                velocity,
            };
            let wp = f.waypoint(&g, Some(w), goal, p.into(), body, &mut budget, &mut || 0);
            (wp, f.holding(&g))
        };
        let seed_183 = Vec3::new(-2742.7, -1246.2, 508.125);
        assert_eq!(
            at(&mut f, seed_183, Vec3::new(-148.0, -16.0, 0.0)),
            (Some(roof.into()), true),
            "comes to rest on the node"
        );
        assert_eq!(
            at(&mut f, roof, Vec3::ZERO),
            (Some(ledge.into()), false),
            "from at rest on the node"
        );
        assert_eq!(
            at(
                &mut Follower::default(),
                seed_183,
                Vec3::new(-60.0, -10.0, 0.0)
            ),
            (Some(ledge.into()), false),
            "slow enough"
        );
    }

    /// A walk's body holds weapon 0 built from the bot's `moveSpeedScale`
    /// alone ([`Sim::scaled`]). Off the sight button, the attack and the
    /// sprint the move reads no other field of the held weapon's def (the
    /// weapon's scale in `walk_move`; the rest goes through the sight
    /// flag, a grenade's cook or an LMG's deploy). Every stock weapon held
    /// at its own index runs over mp_depot's roof peak and jumps on the
    /// far slope tick for tick as the scaled body does, under 1.1's
    /// rules and UO's (bot-navigation.md section 3).
    #[test]
    fn a_walk_at_a_weapons_scale_moves_as_the_weapon_held() {
        let installs = [
            (
                vcod_common::testing::game_fs_11(),
                &crate::defs::V1,
                MoveRules::V1_1,
            ),
            (
                vcod_common::testing::uo_fs(),
                &crate::defs::UO,
                MoveRules::Uo,
            ),
        ];
        for (fs, list, rules) in installs {
            let Some(fs) = fs else {
                continue;
            };
            let bsp =
                vcod_common::bsp::parse(&fs.read(&fs.resolve_map("mp_depot").unwrap()).unwrap())
                    .unwrap();
            let world = crate::world::World::from_bsp(&bsp, Some(&fs));
            let w = &world.collision;
            let table = crate::weapons::WeaponTable::load(&fs, list);
            let defs: Arc<[Option<vcod_common::weapon::WeaponDef>]> = table.defs().into();
            let mut walked = 0;
            for (i, def) in defs.iter().enumerate() {
                let Some(def) = def else {
                    continue;
                };
                let run = |sim: &mut Sim, weapon: usize| {
                    let mut ps = body(Vec3::new(-2663.9, -1216.1, 552.125), 0.0);
                    ps.rules = rules;
                    ps.weapon = weapon as _;
                    if weapon > 0 {
                        vcod_common::pmove::weapon::give(&mut ps, weapon as _, 0);
                        ps.ammoclip[def.clip_index] = def.clip_size as _;
                    }
                    ps.velocity = Vec3::new(220.0, 0.0, 0.0);
                    (0..60)
                        .map(|tick| {
                            let cmd = UserCmd {
                                forward: 127,
                                up: if tick == 30 { 127 } else { 0 },
                                weapon: weapon as _,
                                ..NULL_USERCMD
                            };
                            sim.tick(w, &mut ps, &cmd);
                            (ps.origin, ps.velocity, ps.on_ground)
                        })
                        .collect::<Vec<_>>()
                };
                let mut held = Sim {
                    weapons: Some(defs.clone()),
                    ..Sim::default()
                };
                let scaled = with_rules(rules, || run(&mut Sim::scaled(def.move_speed_scale), 0));
                let own = with_rules(rules, || run(&mut held, i));
                assert_eq!(
                    own, scaled,
                    "{rules:?} weapon {i} at {}",
                    def.move_speed_scale
                );
                walked += 1;
            }
            assert!(walked > 10, "{rules:?}: {walked} weapons");
        }
    }

    /// mp_ship: a run down the steep stair to the hull's deck at z 104 is in
    /// the air past its run time (the walk lets it land), and the spawns
    /// in a lifeboat at z 394 get out over its 40-unit gunwale only with a
    /// jump.
    #[test]
    fn ships_hull_and_lifeboat_walks() {
        let Some(world) = map_world("mp_ship") else {
            return;
        };
        let w = &world.collision;
        let stair = settle(w, Vec3::new(3430.0, 288.0, 170.0), 0.0).expect("the stair");
        let boxes = ladders(w);
        let down = walk(w, &boxes, stair, glam::Vec2::new(3456.0, 288.0));
        assert!(
            down.is_some_and(|(p, jumped)| p.z < 110.0 && !jumped),
            "down the stair: {down:?}"
        );
        let boat = settle(w, Vec3::new(4224.0, -376.0, 440.0), 0.0).expect("the gunwale");
        let out = walk(w, &boxes, boat, glam::Vec2::new(4192.0, -384.0));
        assert!(
            out.is_some_and(|(p, jumped)| p.z < 360.0 && jumped),
            "out of the boat: {out:?}"
        );
    }

    /// mp_depot's pit, seed 1145: the flood proved the jump from
    /// (-1569, 2087, 212) to the step at (-1533.2, 2112, 239.1) running at
    /// the column's centre (-1536, 2112). Aimed at the node, where a bot
    /// steers, the walk bobs under the step and never lands, nor aimed 8
    /// units either side: the re-proof drops the edge, and the bot goes
    /// round.
    #[test]
    fn a_jump_edge_that_lands_only_off_where_a_bot_steers_is_dropped() {
        let Some(fs) = vcod_common::testing::game_fs_11() else {
            return;
        };
        let bsp = vcod_common::bsp::parse(&fs.read(&fs.resolve_map("mp_depot").unwrap()).unwrap())
            .unwrap();
        let world = crate::world::World::from_bsp(&bsp, Some(&fs));
        let w = &world.collision;
        let pit = settle(w, Vec3::new(-1569.0, 2087.0, 230.0), 0.0).unwrap();
        let step = settle(w, Vec3::new(-1533.2, 2112.0, 250.0), 0.0).unwrap();
        let at =
            |target: glam::Vec2| as_a_bot(|| walk_as(w, pit, target, Some(step.z), Gait::Forward));
        assert!(
            !matches!(at(step.truncate()), Walked::Arrived(..)),
            "lands aimed at the node"
        );
        let flood = walk(w, &[], pit, glam::Vec2::new(-1536.0, 2112.0));
        assert!(
            flood.is_some_and(|(p, jumped)| jumped && (p.z - step.z).abs() < 1.0),
            "the flood's jump"
        );
        let mut g = NavGraph::from_parts(vec![pit, step], vec![vec![1], vec![]]);
        g.edge_mut(0, 1).unwrap().jump = true;
        g.reprove_jumps(w);
        assert!(g.edges[0].is_empty());
    }

    /// United Offensive's mp_uo_dawnville: the jump edge from (-241,
    /// -16802, 103) up the ramp to the floor at (-190, -16848, 176), the
    /// way to spawns 220 and 248. Right under the floor while still
    /// climbing the ramp, a jump flies past it and falls back; the walk
    /// runs on to the ramp's head, stalls and jumps on, and the re-proof
    /// keeps the edge.
    #[test]
    fn a_jump_from_a_ramp_waits_for_its_head() {
        let Some(fs) = vcod_common::testing::uo_fs() else {
            return;
        };
        let bsp = vcod_common::bsp::parse(
            &fs.read(&fs.resolve_map("mp_uo_dawnville").unwrap())
                .unwrap(),
        )
        .unwrap();
        let world = crate::world::World::from_bsp(&bsp, Some(&fs));
        world.collision.unlink_script_brushes(&bsp.entities, "tdm");
        let w = &world.collision;
        with_rules(MoveRules::Uo, || {
            let foot = settle(w, Vec3::new(-241.0, -16802.0, 120.0), 0.0).unwrap();
            let top = settle(w, Vec3::new(-190.0, -16848.0, 190.0), 0.0).unwrap();
            let mut g = NavGraph::from_parts(vec![foot, top], vec![vec![1], vec![]]);
            g.edge_mut(0, 1).unwrap().jump = true;
            g.reprove_jumps(w);
            assert!(g.next(0).eq([1]) && g.has(0, 1, |k| k.jump));
        });
    }

    /// mp_ship's stair from (5792, -423, 124) up to (5792, -382, 204): the
    /// flood's walk jumps under the top while still climbing and proves a
    /// jump edge, as it did before the bot's jump waited out a ramp's
    /// climb. Taken into the flood's walks, that wait turned 26 of
    /// mp_ship's jump edges plain, and bots stalled and fell at its
    /// stairwell and crow's nest.
    #[test]
    fn the_floods_walk_jumps_under_a_top_while_climbing() {
        let Some(world) = map_world("mp_ship") else {
            return;
        };
        let w = &world.collision;
        let foot = settle(w, Vec3::new(5792.0, -423.0, 140.0), 0.0).unwrap();
        let top = settle(w, Vec3::new(5792.0, -382.0, 220.0), 0.0).unwrap();
        let walked = walk_as(w, foot, top.truncate(), Some(top.z), Gait::Forward);
        assert!(matches!(walked, Walked::Arrived(_, true)), "a jump edge");
    }

    /// mp_dawnville's spawns 88 and 154 at (145, -15311, 7) are reached
    /// only by the jump from (-96, -15409, -13) to (-89, -15360, 19), which
    /// `link_back` proved 8 units aside. Aimed at the node the run pins on
    /// the wall at y -15375, where `jump_clear` finds no room; a bot jumps
    /// there anyway and lands, and so does the re-proof, which jumps as a
    /// bot does: the edge stays, aimed at its node.
    #[test]
    fn a_jump_edge_is_proved_again_by_the_bots_jump() {
        let Some(fs) = vcod_common::testing::game_fs_11() else {
            return;
        };
        let bsp =
            vcod_common::bsp::parse(&fs.read(&fs.resolve_map("mp_dawnville").unwrap()).unwrap())
                .unwrap();
        let world = crate::world::World::from_bsp(&bsp, Some(&fs));
        let w = &world.collision;
        let foot = settle(w, Vec3::new(-96.0, -15409.0, 0.0), 0.0).unwrap();
        let top = settle(w, Vec3::new(-89.0, -15360.0, 30.0), 0.0).unwrap();
        let roomed = walk_as(w, foot, top.truncate(), Some(top.z), Gait::Forward);
        assert!(
            !matches!(roomed, Walked::Arrived(..)),
            "the walk with the room test"
        );
        let mut g = NavGraph::from_parts(vec![foot, top], vec![vec![1], vec![]]);
        g.edge_mut(0, 1).unwrap().jump = true;
        g.reprove_jumps(w);
        assert!(g.next(0).eq([1]) && g.has(0, 1, |k| k.jump && k.aim.is_none()));
        assert_eq!(g.aim(0, 1), top.truncate());
    }

    /// mp_ship's hold ladder: its head stands on a lip over the shaft. A
    /// leap from the head to the ledge at 641 arrives, but a climb tips the
    /// body onto the lip 4 units south of it, and from there the leap
    /// falls. A ladder's head is no leap's foot.
    #[test]
    fn a_ladder_head_is_no_leaps_foot() {
        let Some(world) = map_world("mp_ship") else {
            return;
        };
        let w = &world.collision;
        let foot = Vec3::new(4332.0, -94.0, 56.125);
        let head = Vec3::new(4332.0, -87.12833, 615.125);
        let top = Vec3::new(4322.311, -158.08282, 641.125);
        let leap = |from: Vec3| walk_as(w, from, top.truncate(), Some(top.z), Gait::Leap);
        assert!(matches!(leap(head), Walked::Arrived(_, true)));
        let tipped = Vec3::new(4333.2227, -91.545074, 615.125);
        assert!(matches!(leap(tipped), Walked::Fell));
        let mut g = NavGraph::from_parts(vec![foot, head, top], vec![vec![1], vec![0], vec![]]);
        g.link_leaps(w);
        assert!(g.edges_where(|k| k.leap).next().is_none(), "{:?}", g.edges);
        // Not reached by the climb, the same spot is a foot.
        let mut g = NavGraph::from_parts(vec![foot, head, top], vec![vec![], vec![0], vec![]]);
        g.link_leaps(w);
        assert!(g.has(1, 2, |k| k.leap), "{:?}", g.edges);
    }

    /// mp_cassino's allied upper floor at z 208 leaves by a window sill at
    /// 240 over the street at 0: the walk jumps onto the sill and falls 240,
    /// under `bg_fallDamageMinHeight`'s 256 (bot-navigation.md, "Other
    /// rules").
    #[test]
    fn a_walk_off_cassinos_allied_sill_falls_240_and_lands() {
        let Some(fs) = vcod_common::testing::uo_fs() else {
            return;
        };
        let entry = fs.resolve_map("mp_cassino").unwrap();
        let bsp = vcod_common::bsp::parse(&fs.read(&entry).unwrap()).unwrap();
        let world = crate::world::World::from_bsp(&bsp, Some(&fs));
        let floor = Vec3::new(193.0, 1103.0, 208.125);
        with_rules(MoveRules::Uo, || {
            let out = walk_to(
                &world.collision,
                &[],
                floor,
                glam::Vec2::new(16.0, 1104.0),
                None,
            );
            assert!(
                out.is_some_and(|(p, jumped)| jumped && p.z < 40.0),
                "{out:?}"
            );
        });
    }

    /// Node 4 of [`tiny`] is a drop's foot with no way on: cut off from the
    /// component the other four make.
    /// mp_cassino's axis room: from the wall under its south-west window a
    /// forward walk does not get onto the sill, a crouched jump does, as a
    /// retail UO client's does at a bot's 50 ms cmds
    /// (`moves_ab.rs` `CROUCHJUMP_UO`).
    #[test]
    fn a_crouched_jump_gets_onto_cassinos_axis_sill_where_a_run_does_not() {
        let Some(fs) = vcod_common::testing::uo_fs() else {
            return;
        };
        let entry = fs.resolve_map("mp_cassino").unwrap();
        let bsp = vcod_common::bsp::parse(&fs.read(&entry).unwrap()).unwrap();
        let world = crate::world::World::from_bsp(&bsp, Some(&fs));
        let from = Vec3::new(5347.0, -1496.0, 128.125);
        let sill = glam::Vec2::new(5305.0, -1548.0);
        with_rules(MoveRules::Uo, || {
            let walk = |gait| walk_as(&world.collision, from, sill, Some(160.125), gait);
            assert!(matches!(
                walk(Gait::Forward),
                Walked::Blocked | Walked::Fell
            ));
            assert!(matches!(walk(Gait::CrouchJump), Walked::Arrived(_, true)));
        });
    }

    #[test]
    fn a_run_bobbing_under_a_steep_face_jumps_out_of_the_gutter() {
        let Some(fs) = vcod_common::testing::uo_fs() else {
            return;
        };
        let entry = fs.resolve_map("mp_berlin").unwrap();
        let bsp = vcod_common::bsp::parse(&fs.read(&entry).unwrap()).unwrap();
        let world = crate::world::World::from_bsp(&bsp, Some(&fs));
        let w = &world.collision;
        let slope = Vec3::new(-673.7223, 4131.117, 616.0533);
        let ledge = glam::Vec2::new(-720.0, 4080.0);
        with_rules(MoveRules::Uo, || {
            assert!(!matches!(
                walk_as(w, slope, ledge, None, Gait::Forward),
                Walked::Arrived(..)
            ));
            let out = walk_to(w, &[], slope, ledge, None);
            assert!(
                out.is_some_and(|(p, jumped)| jumped && p.z > 645.0),
                "{out:?}"
            );
        });
    }

    /// mp_ship's hull beam: a body stands on its side's edge 16 under the
    /// top. A walk from there aimed 8 units beside a top node comes to rest
    /// on that edge again, which is not the node's floor.
    #[test]
    fn a_walk_back_onto_ships_hull_beam_arrives_only_on_top() {
        let Some(world) = map_world("mp_ship") else {
            return;
        };
        let w = &world.collision;
        let edge = Vec3::new(2208.5088, 444.68066, -47.875);
        let top = Vec3::new(2172.9297, 418.1561, -31.875);
        let aside = top.truncate() + (top - edge).truncate().perp().normalize() * SIDESTEP;
        let walked = walk_as(w, edge, aside, Some(top.z), Gait::Forward);
        assert!(!matches!(walked, Walked::Arrived(..)));
        // The way it went: onto the edge at the beam's foot.
        let free = walk_as(w, edge, aside, None, Gait::Forward);
        assert!(matches!(free, Walked::Arrived(p, false) if (p.z - edge.z).abs() < 0.5));
    }

    /// mp_ship's shelf corners at (5375.1, 321.1, 352.1) and (5375.1,
    /// -193.1, 352.1), a body respawned at rest there and run 2 s at a
    /// yaw. Retail 1.1d (`client-probes/probe_shipcrease`, 2026-10-18,
    /// bot-navigation.md section 3, "the push-off, the unpin and the slope"): the north
    /// one runs free at 200 and 225 to (5343.12, 227.13) and at 140 to
    /// (5343.12, 332.87); the south one stays at x 5375.1 at 140 and runs
    /// at 200 and 225 to (5343.12, -204.87). So the run at the north
    /// node needs no unpin, and the south side's way out is the west axis,
    /// 50 degrees off the way to its node.
    #[test]
    fn a_body_pinned_in_a_crease_runs_aside() {
        let Some(world) = map_world("mp_ship") else {
            return;
        };
        let w = &world.collision;
        let settled = |p: Vec3| {
            let mut ps = PlayerState::spawn(p, 0.0);
            Sim::default().tick(w, &mut ps, &NULL_USERCMD);
            ps
        };
        let run = |ps: PlayerState, yaw: f32, ticks: usize| {
            let mut run = ps;
            let mut sim = Sim::default();
            for _ in 0..ticks {
                let cmd = UserCmd {
                    forward: 127,
                    angles: [0, (yaw * ANGLE2SHORT) as i32, 0],
                    ..NULL_USERCMD
                };
                sim.tick(w, &mut run, &cmd);
            }
            run.origin
        };
        let north = settled(Vec3::new(5375.1, 321.1, 352.125));
        let south = settled(Vec3::new(5375.1, -193.1, 352.125));
        for (ps, yaw, x, y) in [
            (north, 200.0, 5343.12, 227.13),
            (north, 225.0, 5343.12, 227.13),
            (north, 140.0, 5343.12, 332.87),
            (south, 140.0, 5375.1, -193.1),
            (south, 200.0, 5343.12, -204.87),
            (south, 225.0, 5343.12, -204.87),
        ] {
            let at = run(ps, yaw, 40);
            assert!(
                (at.x - x).abs() < 0.05 && (at.y - y).abs() < 0.05,
                "{} at {yaw}: {at}",
                ps.origin
            );
        }
        // The north run at its node is free; nothing to unpin.
        let to = Vec3::new(5344.0938, 312.65878, 352.125);
        assert_eq!(unpin_yaw(w, &[], &north, to, &[]), None);
        // The south side's crease, 50 degrees off the way north-west.
        let to = Vec3::new(5344.4, -156.1, 352.125);
        let yaw = unpin_yaw(w, &[], &south, to, &[]).expect("a way out");
        assert!(run(south, yaw, UNPIN_TICKS).x < 5360.0, "{yaw}");
    }

    #[test]
    fn a_back_down_slides_along_the_face_onto_a_floor_beside_the_ladder() {
        let Some(world) = map_world("mp_ship") else {
            return;
        };
        let w = &world.collision;
        let node = glam::Vec2::new(4318.5, -96.0);
        let down = |slide: bool| {
            let mut ps = PlayerState::spawn(Vec3::new(4328.5, -93.1, 420.0), 90.0);
            let mut sim = Sim::default();
            for _ in 0..200 {
                let yaw = climb_yaw(ps.ladder_normal).unwrap_or(90.0);
                let right = if slide {
                    crate::bots::along_face(yaw, ps.origin.into(), node.into())
                } else {
                    0
                };
                let cmd = UserCmd {
                    forward: -127,
                    right,
                    angles: [
                        (-LADDER_PITCH * ANGLE2SHORT) as i32,
                        (yaw * ANGLE2SHORT) as i32,
                        0,
                    ],
                    ..NULL_USERCMD
                };
                sim.tick(w, &mut ps, &cmd);
                if ps.on_ground {
                    break;
                }
            }
            ps
        };
        let slid = down(true);
        assert!(
            slid.on_ground && (slid.origin.z - 304.125).abs() < 0.5,
            "{}",
            slid.origin
        );
        let plain = down(false);
        assert!(
            !(plain.on_ground && (plain.origin.z - 304.125).abs() < 0.5),
            "{}",
            plain.origin
        );
    }
}
