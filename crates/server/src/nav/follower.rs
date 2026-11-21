//! The path a bot follows: planning, pulling ahead and the cues it reads
//! off the edge under way.

use super::*;

mod reprove;
mod yields;

/// Horizontal distance at which a waypoint counts as reached.
pub(super) const REACH: f32 = 24.0;

/// A start that reaches fewer nodes than this round the parked vehicles'
/// is hemmed in by them ([`Follower::penned`]): mp_cassino's terrace at
/// (383, -916, 76) reaches 7 with a tank over its stair, of the 7 676 it
/// reaches without.
const PEN_NODES: usize = 64;

/// How many grid steps across [`NavGraph::clear_near`] looks: past a
/// parked tank's reach (`server::bot_vehicles::PARK_REACH`).
pub(super) const CLEAR_STEPS: i32 = 6;

/// Where none is clear within [`CLEAR_STEPS`]: past a burning hull's nodes
/// (`server::bot_vehicles`, `bots::WRECK_REACH`) at the finest spacing.
pub(super) const CLEAR_STEPS_FAR: i32 = 13;

/// A waypoint further than this many grid steps away means the bot has left
/// its path (knocked back, fell off a ledge): plan again.
pub(super) const OFF_PATH_STEPS: f32 = 4.0;

/// Ticks without closing on the waypoint before the follower gives up on
/// it, and the ticks it then leaves the bot to its own unstick.
const STUCK_TICKS: u32 = 40;

const REST_TICKS: u32 = 20;

/// How far, flat, the last waypoint may be for a body to wait on it while a
/// plan is searched for ([`Follower::rest_point`]), and for how many ticks.
const WAIT_REACH: f32 = 48.0;

const WAIT_TICKS: u32 = 40;

/// The most of the tick's plan budget one follower's search takes, so a
/// long search leaves some for the bots after it.
pub(crate) const PLAN_SLICE: u32 = 2000;

/// How long the edge a follower got stuck on stays out of its plans. What
/// blocks a proven edge is mostly a body: a bot guarding the bomb at the
/// foot of mp_rocket's stairs held another there for 11 s.
const AVOID_TICKS: u32 = 200;

/// How far below where its path ended a body walking at a point goal has
/// to drop before the follower plans for the point again: more than a stair
/// or a step off a kerb.
const ARRIVED_DROP: f32 = 48.0;

/// How near a leap's foot, flat, a bot comes to rest before it leaps.
pub const LEAP_FOOT: f32 = 12.0;

/// A jump edge's top counts as reached only within this of its height:
/// under a stair step, over the slack a slope leaves.
pub(super) const JUMP_PASS: f32 = 16.0;

/// A node the next edge drops more than a step from counts as reached only
/// within this of its height (bot-navigation.md, section 3).
const DROP_PASS: f32 = 8.0;

/// What a [`super::Edge::risky`] drop adds to a plan's cost: a long way round
/// is taken first.
pub(super) const RISK_COST: f32 = 4000.0;

/// What a teammate's path costs another plan over its length, on top of
/// the length: a way round up to this much longer is taken instead
/// (bot-objectives.md section 9).
pub const CROWD_COST: f32 = 1.0;

/// [`CROWD_COST`] for an S&D flanker on the main group's paths: a way
/// round up to this much longer again is taken (bot-objectives.md 9.1).
pub const FLANK_COST: f32 = 4.0;

/// [`CROWD_COST`] for a `ctf` carrier on the edges near enemies his side
/// saw of late (bot-objectives.md 14.12).
pub const DANGER_COST: f32 = 2.0;

/// How far aside of a drop's node [`NavGraph::mark_risky`] walks it again.
pub(super) const RISK_ASIDE: f32 = 4.0;

/// How many of the nearest nodes [`NavGraph::nearest_walkable`] walks to.
pub(super) const WALKABLE_TRIES: usize = 8;

/// A body on the ground this far under its waypoint, off a ladder, has
/// fallen off the path.
const FELL_BELOW: f32 = 48.0;

/// Within this of a waypoint flat, a walk on that fails from where the bot
/// stands fails from the node ([`Follower::edge_holds`]).
const ON_NODE: f32 = 12.0;

/// A roam goal is picked at least this far away when it can be.
pub(super) const ROAM_MIN: f32 = 1000.0;

/// A bot this near a ladder, flat, waits aside for a climber on it
/// ([`Follower::ladder_yield`]); a player this near is on its rungs, and
/// this near the bot's end in height about to step off there; the spot
/// the bot waits at is at least this far from the ladder.
const YIELD_NEAR: f32 = 64.0;

const CLIMBER_NEAR: f32 = 32.0;

const CLIMBER_CLOSE: f32 = 64.0;

const YIELD_CLEAR: f32 = 32.0;

/// Two climbers this near in height share the rungs' top abreast
/// ([`Follower::abreast_yield`]): a standing body's height.
const ABREAST_HEIGHT: f32 = 70.0;

/// A climber coming up this far under a body backing down the same rungs
/// sends it back up ([`Follower::head_on_yield`]): two bodies' height.
const HEAD_ON: f32 = 2.0 * ABREAST_HEIGHT;

/// Only on a ladder taller than a fall that kills (`bg_fallDamageMaxHeight`,
/// [`falls`]: 480 on CoD1; a hurt bot's is shorter, [`fall_that_kills`]):
/// off a shorter one a whole body slides off a climber's head and lives,
/// and turning back held two bots at mp_ship's hold ladder (3369, -379)
/// for 12 s (bot-navigation.md section 3).
pub(super) fn deadly_fall() -> f32 {
    falls().max
}

/// The lowest fall a body with `health` of `max_health` does not live
/// through: `PM_CrashLand`'s percent, linear from `h.min` to 100 at
/// `h.max`, of `max_health`, 110 percent past 99 (`pmove::fall_damage`,
/// `Server`'s `fall_damage`, player-clip doc 8.3 and 8.8). A drop no
/// higher lands whatever the health.
pub fn fall_that_kills(health: i32, max_health: i32, h: vcod_common::pmove::FallHeights) -> f32 {
    if h.max <= h.min || h.min < 0.0 || max_health <= 0 {
        return f32::INFINITY;
    }
    // The smallest percent whose share of max health reaches the health.
    let percent = (0..=99)
        .find(|&p| (f64::from(max_health) * f64::from(p) * f64::from(0.01f32)) as i32 >= health)
        .unwrap_or(100);
    // The percent truncates up from the height, so the height that first
    // gives it; past the max the whole 110 lands.
    h.min + (h.max - h.min) * percent as f32 / 100.0
}

/// A body on the ground holds still rather than step within this of a
/// climber rising under it, flat ([`Follower::rising_hold`]): two
/// half-widths are where `StuckInClient` pushes the two apart.
const RISING_REACH: f32 = 2.0 * vcod_common::pmove::HALF_WIDTH + 6.0;

/// How far under the feet the climber's top may be: a climb at the bots'
/// 95 units/s rises this in half a second.
const RISING_BELOW: f32 = 48.0;

/// The longest such hold.
const RISING_TICKS: u32 = 60;

/// Two bodies on the ground pressed together this long, both slow and
/// near their waypoints, and one of them gives way for `GIVE_TICKS`,
/// stepping `GIVE_STEP` back from the other ([`Follower::pressed_yield`]).
const PRESSED_TICKS: u32 = 10;

const GIVE_TICKS: u32 = 20;

const GIVE_STEP: f32 = 24.0;

/// A body on the ground slower than this across, units/s, with another
/// within [`BODY_REACH`] across and [`BODY_RISE`] up or down toward its
/// next node, is held up by it; after [`BODY_TICKS`] its plans keep off
/// the nodes the other stands over for [`SHUN_TICKS`]
/// ([`Follower::bodies_in_way`]).
const BODY_HELD_SPEED: f32 = 60.0;

const BODY_REACH: f32 = 48.0;

const BODY_RISE: f32 = 64.0;

const BODY_TICKS: u32 = 4;

const SHUN_TICKS: u32 = 60;

/// The longest a bot stands aside for an oncoming body, the ticks before
/// it does again, and how far it backs off when no node lies aside
/// ([`Follower::stand_aside`]).
const ASIDE_TICKS: u32 = 60;

const ASIDE_REST: u32 = 60;

const ASIDE_BACK: f32 = 64.0;

/// The longest wait: a climb of mp_ship's 296-unit ladders takes about
/// 110 ticks. With the climber still, the most of it: a climber that hangs
/// on with the way clear is stuck for another reason (mp_ship's ladder at
/// x 3808). A wait that runs out is not taken again for `YIELD_REST`.
const YIELD_TICKS: u32 = 200;

const YIELD_HUNG: u32 = 40;

const YIELD_REST: u32 = 200;

/// How far off the line along a [`Follower::straight_run`] a node may
/// stand, and how many of the path's nodes it looks along.
const SPRINT_SLACK: f32 = 16.0;

const SPRINT_LOOK: usize = 40;

/// A seen enemy has to move this far off the planned destination before the
/// path is planned again.
pub(super) const RETARGET: f32 = 128.0;

/// [`Follower::pull`]: how far, in grid steps, a pulled waypoint may lie
/// flat (under [`OFF_PATH_STEPS`], past which the follower replans), how
/// far a skipped node may stand off the line to it, the floor probe's
/// spacing along that line ([`straight_floor`]), and the ticks between
/// tries after one fails.
const PULL_STEPS: f32 = 3.0;

const PULL_SLACK: f32 = 0.75;

pub(super) const PULL_PROBE: f32 = 16.0;

const PULL_RETRY: u32 = 4;

/// What the body a [`Follower`] steers stands on this tick.
#[derive(Clone, Copy)]
pub struct Footing {
    /// At rest with a jump ready ([`ready_to_leap`]).
    pub still: bool,
    pub on_ground: bool,
    pub on_ladder: bool,
    /// `ps.velocity`: a drop is re-proved at the pace the body comes onto it.
    pub velocity: Vec3,
}

impl Footing {
    #[cfg(test)]
    const STANDING: Footing = Footing {
        still: true,
        on_ground: true,
        on_ladder: false,
        velocity: Vec3::ZERO,
    };
}

/// How far `p` stands across from the segment `a` to `b`, all flat.
fn seg_dist_flat(a: Vec3, b: Vec3, p: Vec3) -> f32 {
    let (a, b, p) = (a.truncate(), b.truncate(), p.truncate());
    let ab = b - a;
    let t = ((p - a).dot(ab) / ab.length_squared().max(1e-6)).clamp(0.0, 1.0);
    p.distance(a + ab * t)
}

/// One bot's walk along the graph: the destination, the planned path and
/// the bookkeeping that notices when it has gone wrong. The server keeps one
/// per bot and asks it for the waypoint every tick.
#[derive(Clone, Default)]
pub struct Follower {
    dest: Option<u32>,
    path: Vec<u32>,
    /// Index into `path` of the current waypoint.
    next: usize,
    /// Closest the bot has come to the current waypoint, and the ticks since
    /// it last closed in.
    best: f32,
    idle: u32,
    rest: u32,
    /// The last waypoint handed out, and the ticks since one was: where
    /// the body waits while a plan is searched for ([`Self::rest_point`]).
    last: Option<Vec3>,
    waited: u32,
    /// A point goal's path is walked out; the bot heads at the point itself.
    arrived: bool,
    /// Where the body stood when the path was walked out. Walking at the
    /// point off the graph can drop it a floor (mp_depot's Retrieval
    /// documents sit by an upper floor's edge), and from there the point is
    /// planned for again.
    arrived_at: Vec3,
    /// Calls to [`Self::waypoint`], one per tick: the clock `avoid` runs on.
    clock: u32,
    /// Edges it got stuck on, left out of its plans until the tick given.
    avoid: Vec<(u32, u32, u32)>,
    /// Edges it got stuck on again while still avoided, planned through
    /// for want of a way round: left out of that plan too, until the tick
    /// given ([`Self::avoid_edge`]).
    dead: Vec<(u32, u32, u32)>,
    /// The plan under way, and whether it leaves out `avoid` (a plain one
    /// follows if it fails).
    search: Option<Search>,
    /// Edges teammates' paths run along, each `(low, high)` and sorted: a
    /// plan pays [`CROWD_COST`] more on them (`bots::team::crowd`). Set by
    /// the server each tick; read when a plan starts.
    pub crowd: Vec<(u32, u32)>,
    /// What [`Self::crowd`] costs: [`CROWD_COST`], or [`FLANK_COST`] for
    /// an S&D flanker. Set with it.
    pub crowd_cost: f32,
    /// The bot's route noise seed ([`Search::noisy`]), 0 for none. Set
    /// with [`Self::crowd`].
    pub noise: u64,
    /// Ticks before a `ctf` carrier's plan may be dropped again for a
    /// sighting on its way ahead (`bots::driver`, bot-objectives.md 14.12).
    pub danger_wait: u32,
    /// Nodes a parked vehicle stands over, sorted
    /// ([`NavGraph::blocked_near`]): a plan runs through none but its own
    /// ends, and a path with one ahead is planned again. Set by the
    /// server each tick.
    pub blocked: Arc<[u32]>,
    /// The path runs through [`Self::blocked`]: no plan went round.
    through: bool,
    /// The last plan found the start hemmed in by [`Self::blocked`]
    /// ([`PEN_NODES`]).
    penned: bool,
    round: bool,
    /// The threat an [`Goal::Away`](crate::bots::Goal::Away) destination
    /// was picked against.
    threat: Option<Vec3>,
    /// The kind of the last goal asked for: a change drops `arrived` and
    /// `threat`, which belong to the goal that set them.
    goal_kind: Option<std::mem::Discriminant<crate::bots::Goal>>,
    /// The body fell off its path: the next plan starts at a node it walks
    /// to ([`NavGraph::nearest_walkable`]).
    fell: bool,
    /// The drop edge under way when the body left the ground, kept through
    /// a plan made in the air ([`Self::landed_partway`]).
    falling_off: Option<(u32, u32)>,
    /// The body was on a ladder when the plan began: it starts at one of
    /// the ladder's ends ([`NavGraph::nearest_ladder_end`]).
    on_rungs: bool,
    /// The plan under way starts at a node a walk from the body reached
    /// or at a ladder's end, not merely the nearest.
    proved_start: bool,
    /// That walk from the body arrived only with a jump
    /// ([`NavGraph::walkable_start`]): the way to the first node is run as
    /// a jump edge.
    jump_start: bool,
    /// The last jump edge waited on, and whether its jump lands from rest
    /// ([`Self::rest_jump_fails`]).
    rest_jump: Option<((u32, u32), bool)>,
    /// The last drop asked about, and whether the graph's walk ran off it
    /// ([`Self::runs_off`]).
    run_off: Option<((u32, u32), bool)>,
    /// The last drop asked about, and whether a miss of it hurts
    /// ([`Self::miss_hurts`]).
    miss_hurt: Option<((u32, u32), bool)>,
    /// A jump edge whose walk on from where the body passed its first node
    /// arrived without the jump ([`Self::edge_holds`]): the bot runs it
    /// as that walk did ([`Self::jump_cued`]).
    walked_jump: Option<(u32, u32)>,
    /// The last edge asked about, and whether only a creep proved it
    /// ([`Self::creeps_off`]).
    creep_off: Option<((u32, u32), bool)>,
    /// The creep whose top the body comes to rest on before it heads on
    /// ([`Self::creep_top`]).
    stop_at: Option<(u32, u32)>,
    /// The edge whose first node the body comes to rest on and heads on
    /// from only at rest: its pace flung it off the way on
    /// ([`Self::flings`]).
    held: Option<(u32, u32)>,
    /// The scale the body runs at, which [`Self::flings`]' walk runs at:
    /// the held weapon's `moveSpeedScale`, cut to the graph's 1 for a
    /// light weapon on the ground (`paced_key`), so only a heavy one's
    /// shows. A body can still pass 190 units/s off a slope or a fall.
    pub move_scale: f32,
    /// Ticks spent waiting aside for a climber ([`Self::ladder_yield`]),
    /// those of them with the climber still, and the ticks before it waits
    /// again once a wait ran out.
    yield_ticks: u32,
    yield_hung: u32,
    yield_rest: u32,
    /// Ticks held still for a climber rising under the body
    /// ([`Self::rising_hold`]).
    rising_ticks: u32,
    /// Ticks held still on the rungs under a body over it
    /// ([`Self::under_hold`]).
    under_ticks: u32,
    /// Ticks the body has been held up by a teammate standing still at
    /// its side, and the nodes it plans round for the ticks left after
    /// (`bots::driver`'s `mates_in_way`).
    pub mate_ticks: u32,
    pub mate_block: (u32, Arc<[u32]>),
    /// Ticks pressed on another body, and the spot and ticks of a give way
    /// ([`Self::pressed_yield`]).
    pressed_ticks: u32,
    give: Option<(Vec3, u32)>,
    /// The lowest fall that kills this body now ([`fall_that_kills`]):
    /// a hurt bot dies off a shorter ladder. `None` reads [`deadly_fall`].
    pub fall_kills: Option<f32>,
    /// pmove's climb is under way: no plan starts until it lands, since a
    /// plan from mid-climb starts at the foot (bot-navigation.md 3.15). Set
    /// by the driver per tick.
    pub climbing: bool,
    /// A mantle edge whose landing lies past [`OFF_PATH_STEPS`] is still
    /// followed: the probe's climb ends where the body comes down, which
    /// on CoD4's mp_crash rubble at (291, -1439, 84) is 162 across
    /// (bot-objectives.md 12.4). Set by the driver per game.
    pub long_mantles: bool,
    /// A waypoint up to [`REACH`] past [`OFF_PATH_STEPS`] is still on the
    /// path: a body passes an edge's foot from up to [`REACH`] short of
    /// it, so an edge nearly [`OFF_PATH_STEPS`] long read as off the path
    /// there. Set by the driver per game.
    pub foot_slack: bool,
    /// A mantle edge down more than a jump is taken from rest at its foot
    /// without the creep's re-proof ([`Self::backs_short`]).
    pub mantle_rests: bool,
    /// What a jump edge adds to a plan's length ([`Search::jumps_costing`]).
    /// Set by the driver per game.
    pub jump_cost: f32,
    /// Off the ground and off the rungs last tick: a landing is read
    /// against the edge under way.
    airborne: bool,
    /// The point goal's plan the last goal move left, for a goal that
    /// comes back.
    parked: Option<Parked>,
    /// The waypoint before [`Self::pull`] took it further along the plan,
    /// while the pulled one is still ahead: a re-check that fails falls
    /// back toward it.
    pull_base: Option<usize>,
    /// Ticks before the next pull is tried, after one failed.
    pull_wait: u32,
    /// Edges given up on, by the stall count or the bot's own unstick
    /// ([`Self::stuck`]): what `bots::driver` tallies as edge stalls.
    pub gave_up: u32,
    /// Ticks held up by another body in the way ([`Self::bodies_in_way`]),
    /// the nodes the plans keep off for it, sorted, until the tick given,
    /// and whether the plan found no way round them.
    body_ticks: u32,
    shun: Arc<[u32]>,
    shun_until: u32,
    shun_through: bool,
    /// Where the bodies [`Self::shun`] keeps off stood.
    shunned: Vec<Vec3>,
    /// A give way to an oncoming body ([`Self::stand_aside`]): the spot, the
    /// way the other came from, the ticks left; and the ticks before the
    /// next.
    aside: Option<(Vec3, Vec3, u32)>,
    aside_rest: u32,
}

/// A [`Follower`] plan set aside when its point goal moved off, and the
/// tick it was.
#[derive(Clone)]
struct Parked {
    dest: u32,
    path: Vec<u32>,
    next: usize,
    best: f32,
    idle: u32,
    at: u32,
}

/// How long a [`Parked`] plan waits for its goal to come back: a second.
const PARK_TICKS: u32 = 20;

impl Follower {
    /// The waypoint toward `goal` for a bot standing at `at` on `body`'s
    /// footing. `world`
    /// re-proves a leap from where the bot rests (none in unit tests).
    /// `budget` is the tick's shared A* budget in expanded nodes; a bot whose
    /// plan it does not finish gets no waypoint this tick and resumes the
    /// search next tick, so neither one long plan nor many bots planning at
    /// once (the tick the graph lands) stall a tick.
    /// `rand` picks roam destinations.
    pub fn waypoint(
        &mut self,
        g: &NavGraph,
        world: Option<&CollisionWorld>,
        goal: crate::bots::Goal,
        at: [f32; 3],
        body: Footing,
        budget: &mut u32,
        rand: &mut dyn FnMut() -> i32,
    ) -> Option<[f32; 3]> {
        let w = self.follow(g, world, goal, at, body, budget, rand);
        if let Some(w) = w {
            self.last = Some(w.into());
            self.waited = 0;
        } else {
            self.waited += 1;
        }
        w
    }

    /// Where a body on the ground with no waypoint this tick comes to rest
    /// while the next plan is searched for: the last waypoint, when it
    /// stands within [`WAIT_REACH`] on the body's floor, for up to
    /// [`WAIT_TICKS`] (bot-navigation.md section 3). Not while the follower
    /// rests on purpose: a body stuck before its first node is left to its
    /// own unstick. Round a body in the way, where it stands.
    pub fn rest_point(&self, at: [f32; 3], body: Footing) -> Option<[f32; 3]> {
        // The plan round a body in the way ([`Self::bodies_in_way`]) is
        // waited for where the body stands (bot-navigation.md, "The ship's
        // leftovers", seed 1381).
        if self.rest == 0
            && !self.shun.is_empty()
            && self.path.is_empty()
            && self.waited <= WAIT_TICKS
            && body.on_ground
            && !body.on_ladder
        {
            return Some(at);
        }
        let p = self.last?;
        let at = Vec3::from(at);
        (self.rest == 0
            && self.waited <= WAIT_TICKS
            && body.on_ground
            && !body.on_ladder
            && (p - at).truncate().length() < WAIT_REACH
            && (p.z - at.z).abs() < vcod_common::pmove::STEPSIZE)
            .then_some(p.into())
    }

    /// [`Self::waypoint`] before the bookkeeping.
    fn follow(
        &mut self,
        g: &NavGraph,
        world: Option<&CollisionWorld>,
        goal: crate::bots::Goal,
        at: [f32; 3],
        body: Footing,
        budget: &mut u32,
        rand: &mut dyn FnMut() -> i32,
    ) -> Option<[f32; 3]> {
        use crate::bots::Goal;
        self.clock += 1;
        self.stop_at = None;
        let landed = self.airborne && body.on_ground;
        self.airborne = !body.on_ground && !body.on_ladder;
        let clock = self.clock;
        self.avoid.retain(|e| e.2 > clock);
        self.dead.retain(|e| e.2 > clock);
        if clock >= self.shun_until && !self.shun.is_empty() {
            self.shun = Arc::default();
            self.shun_through = false;
            self.shunned.clear();
        }
        if self.rest > 0 {
            self.rest -= 1;
            return None;
        }
        let here = Vec3::from(at);
        let drop_landed = self.landed_partway(g, here, body, landed);
        let kind = Some(std::mem::discriminant(&goal));
        if self.goal_kind != kind {
            self.goal_kind = kind;
            self.arrived = false;
            self.threat = None;
        }
        match goal {
            Goal::Hold => {
                self.reset();
                return None;
            }
            Goal::Roam => {}
            Goal::Away(t) => {
                let t = Vec3::from(t);
                if self.threat.is_none_or(|p| p.distance(t) > RETARGET) {
                    self.reset();
                    self.threat = Some(t);
                }
            }
            Goal::To(p) => {
                // A destination within reach of the point but a floor off the
                // node a plan to it ends at is no longer the point's
                // (bot-navigation.md, "The ship's leftovers"). A point
                // farther off the graph than `RETARGET` moves only with the
                // node it maps to (bot-navigation.md, "A goal far off the
                // graph, the ladders again, 2026-11-14").
                let target = g.goal_node(p).map(|t| g.nodes[t as usize]);
                let off = |d: u32| {
                    let n = g.nodes[d as usize];
                    (n.distance(Vec3::from(p)) > RETARGET
                        && target.is_none_or(|t| n.distance(t) > RETARGET))
                        || target
                            .is_some_and(|t| (n.z - t.z).abs() > vcod_common::pmove::JUMP_HEIGHT)
                };
                let moved = self.dest.is_none_or(off);
                if moved {
                    // A goal back within `PARK_TICKS` takes up the plan it
                    // left where it was: an enemy in sight for a tick or
                    // two from mp_cassino's stair at (1440, -700, 24)
                    // flipped the goal and back, each replan started at
                    // the node behind the body, and the bot paced the
                    // step for 12 s (seed 17, bot-navigation.md "Jumps").
                    let back = self
                        .parked
                        .take()
                        .filter(|k| clock - k.at <= PARK_TICKS && !off(k.dest));
                    if let Some(d) = self.dest.filter(|_| !self.path.is_empty() && !self.arrived) {
                        self.parked = Some(Parked {
                            dest: d,
                            path: std::mem::take(&mut self.path),
                            next: self.next,
                            best: self.best,
                            idle: self.idle,
                            at: clock,
                        });
                    }
                    self.reset();
                    match back {
                        Some(k) => {
                            self.dest = Some(k.dest);
                            self.path = k.path;
                            self.next = k.next;
                            self.best = k.best;
                            self.idle = k.idle;
                        }
                        None => self.dest = g.goal_node(p),
                    }
                } else if self.arrived {
                    let fell = here.z < self.arrived_at.z - ARRIVED_DROP;
                    if !fell {
                        return Some(self.arrival(g, p));
                    }
                    self.reset();
                    self.dest = g.goal_node(p);
                }
            }
        }
        // Down a drop onto a floor partway: plan round it, and not through
        // it again for `AVOID_TICKS` (a dead edge), where no way round
        // takes the bot. From the floor at 352 the plan to the floor at
        // 204 went back up mp_ship's shelf ladder at (5352, -70) to the
        // same slot, and bots looped ladder, shelf, slot and ledge for 12 s
        // and more; the goal had often moved in the air, so the plan under
        // way on landing was another (bot-objectives.md 14.9).
        if let Some((a, b)) = drop_landed {
            self.avoid_edge(a, b);
            self.avoid_edge(a, b);
            self.reset();
            self.fell = true;
            return None;
        }
        // A vehicle parked over a node ahead, short of the path's end: plan
        // again round it, from a node the body walks to. Its first node
        // may be one: the start a plan falls back to when no walk arrives.
        let ahead = self
            .path
            .get(self.next.max(1)..self.path.len().saturating_sub(1));
        if !self.through && ahead.is_some_and(|a| a.iter().any(|&n| self.blocks(n))) {
            self.path.clear();
            self.held = None;
            self.pull_base = None;
            self.fell = true;
        }
        if self.path.is_empty() {
            if self.climbing {
                return None;
            }
            self.on_rungs = body.on_ladder;
            self.path = self.plan(g, world, goal, here, budget, rand)?;
            self.next = 0;
            self.best = f32::INFINITY;
            self.idle = 0;
        }
        let flat = |p: Vec3| (p - here).truncate().length();
        // A plan made or taken up on the rungs starts at the ladder's end the
        // body is nearer, and when the edge on from there is that ladder the
        // body is already on it: head for the other end. Heading for the
        // start first, a bot halfway up mp_ship's hold ladder at (3370,
        // -380) whose goal flipped between two plans went down for one and
        // up for the other, over and over (bot-combat.md 17.2).
        if body.on_ladder
            && self.next == 0
            && let (Some(&a), Some(&b)) = (self.path.first(), self.path.get(1))
        {
            let (a, b) = (g.nodes[a as usize], g.nodes[b as usize]);
            let (low, high) = if a.z < b.z { (a, b) } else { (b, a) };
            if high.z - low.z > vcod_common::pmove::JUMP_HEIGHT
                && (low.z..high.z).contains(&here.z)
                && flat(low) < g.spacing
            {
                self.next = 1;
                self.best = f32::INFINITY;
                self.idle = 0;
            }
        }
        // Past a waypoint once inside its reach, or once nearer the next one
        // than the waypoint itself is: that keeps a bot from doubling back
        // to touch a node it cut the corner on. Either only on the
        // waypoint's floor: a ladder's head is right above its foot.
        while let Some(&w) = self.path.get(self.next) {
            // The top of a jump is passed only standing on it: from its
            // foot the node is in reach flat and the jump not yet made.
            // So is a node a drop leaves from: from under it the drop is
            // not where the graph proved it.
            let jump = self.jumping(g);
            let rise = if jump {
                JUMP_PASS
            } else if self.dropping(g) {
                DROP_PASS
            } else {
                48.0
            };
            // A leap's foot is passed only at rest on it: the leap was
            // proved from a standstill there. A creep's top is held while
            // the body comes onto it faster than [`CREEP_TOP_PACE`].
            let leap = self
                .path
                .get(self.next + 1)
                .is_some_and(|&b| g.has(w, b, |k| k.leap));
            let wp = g.nodes[w as usize];
            // A node held for a body too fast for the walk on is passed only
            // from at rest on it, like a leap's foot.
            let held = self
                .path
                .get(self.next + 1)
                .is_some_and(|&b| self.held == Some((w, b)));
            let foot = leap
                || held
                || (body.on_ground
                    && !body.on_ladder
                    && body.velocity.truncate().length() > CREEP_TOP_PACE
                    && world.is_some_and(|world| self.creep_top(world, g)));
            let w = wp;
            // From under it, only from its floor: a ladder's head is passed
            // standing at its lip, not from the rungs a storey down.
            let below = rise.min(vcod_common::pmove::STEPSIZE);
            // A ladder's head only standing on it: the climb proved it facing
            // the rungs to the top, and a body that turns for the next node
            // on the rungs or in the air over the lip lets go of them
            // (mp_ship's mast, section 3). A jump's top too: in the air
            // beside the ledge, under its floor, the jump is not yet made
            // (mp_ship's ledge at (3347, 250, 616)).
            // On the rungs, a node below only from its floor: the back
            // down that proved it ran onto that floor, and a body that
            // turns for the next node from above it hangs on the rungs
            // under the floor's lip (mp_ship's hold ladder at z 304).
            let above = if body.on_ladder {
                vcod_common::pmove::STEPSIZE
            } else {
                rise
            };
            // And no node off the rungs in the air: a body that turns for
            // the next node in the air over a narrow shelf flies off its
            // side (mp_ship's shelf at (5384, -161, 396)), and one sliding
            // down terrain too steep to stand on turns off a patch it never
            // landed on and slides on into mp_hurtgen's crease (section 3).
            // On the rungs, not a node more than a step below.
            let high = here.z > w.z + vcod_common::pmove::STEPSIZE;
            // A plan begun on the rungs starts at a ladder's end
            // ([`NavGraph::nearest_ladder_end`]); one over the feet is the
            // head, passed only on the ground like any climb's: from the
            // rungs under mp_ship's crow's nest plate a bot turned for the
            // rim's exit and fell down the hatch.
            let head_first = self.next == 0 && w.z > here.z;
            let level = here.z > w.z - below
                && here.z < w.z + above
                && (body.on_ground
                    || (body.on_ladder && !(self.climbing(g) || jump || high || head_first)));
            // A ladder's foot is passed only on it, past cutting nothing:
            // the climb was proved from there, and a body beside the face
            // heading for the head runs past the rungs (mp_ship, section 3).
            let rung = self.rising(g);
            // Nor is a jump's top cut past: a body still in the air beside
            // it is nearer the next node too, and falls back to the foot.
            // Nor a drop's top: the drop was proved from there, and a body
            // cutting across for the bottom pins on whatever stands between
            // (mp_ship's ladder head at (3298, -380, 352), its foot east
            // behind the wall at y -333).
            // Nor a jump's foot: the jump was proved from there, and a body
            // that cut to its top from a ledge beside it jumped off that
            // ledge (mp_ship's at (3468, -210, 798), section 3).
            let past = !foot
                && !jump
                && !rung
                && self.path.get(self.next + 1).is_some_and(|&n| {
                    if g.has(self.path[self.next], n, |k| k.jump) {
                        return false;
                    }
                    let n = g.nodes[n as usize];
                    n.z >= w.z - vcod_common::pmove::JUMP_HEIGHT
                        && flat(n) < (n - w).truncate().length()
                        // Only with floor the whole way: off mp_ship's mast
                        // ladder at x 3696 a body 27 short of the head's exit
                        // at (3705, 85, 992) read as past it and ran straight
                        // for (3648, 71, 994), over the hatch, and fell back
                        // down it (seed 1031, bot-navigation.md section 3).
                        && world.is_none_or(|world| straight_floor(world, here, n))
                });
            let reached = if foot {
                flat(w) < LEAP_FOOT && body.still
            } else if rung {
                // Square in front of the rungs, the run turns into the face
                // from anywhere near (`face_ahead`); off to the side, only
                // the foot itself proved the climb.
                let square = g.face_ahead(w).is_some();
                flat(w) < if square { REACH } else { ARRIVE }
            } else {
                flat(w) < REACH
            };
            if level && reached && leap && !self.leap_holds(world, here, g) {
                // The leap fails from where this body came to rest: round it.
                let edge = (self.path[self.next], self.path[self.next + 1]);
                self.avoid_edge(edge.0, edge.1);
                self.reset();
                return None;
            }
            // An edge planned while it was avoided had no way round it, and
            // is taken as it is: checked again, it would be avoided again
            // every tick and the bot stand without a waypoint (mp_ship's
            // corner at (5375, -193, 352), where a body from the east jams
            // 2 units past the node on the shelf's slope and its seam).
            let planned_round = self.path.get(self.next + 1).is_some_and(|&b| {
                let a = self.path[self.next];
                self.avoid.iter().any(|e| (e.0, e.1) == (a, b))
            });
            // A body that a light weapon's pace flings off the way on comes
            // to rest on the node first and heads on only from at rest
            // ([`Self::flings`]). Passed on the way to rest at the first
            // pace whose walk arrived, bots came onto mp_hurtgen's slope at
            // (338, 2830, -25) and slid into its crease.
            if level
                && (reached || past)
                && (!foot || held)
                && !planned_round
                && self.flings(world, here, body, g)
            {
                let edge = (self.path[self.next], self.path[self.next + 1]);
                if held {
                    // Flung from at rest too: round it.
                    self.avoid_edge(edge.0, edge.1);
                    self.reset();
                    return None;
                }
                self.held = Some(edge);
                break;
            }
            // So does one whose back down off a drop, at its pace, falls
            // short of the floor it was proved onto ([`Self::backs_short`]).
            if level
                && (reached || past)
                && (!foot || held)
                && !planned_round
                && self.backs_short(world, here, body, g)
            {
                let edge = (self.path[self.next], self.path[self.next + 1]);
                if held {
                    self.avoid_edge(edge.0, edge.1);
                    self.reset();
                    return None;
                }
                self.held = Some(edge);
                break;
            }
            if level
                && (reached || past)
                && (!foot || held)
                && !planned_round
                && !self.edge_holds(world, here, body.velocity, body.on_ladder, g)
            {
                // The walk on was proved from the node, and fails from here:
                // close in on the node first, and from on it, round the edge.
                // A drop more than a step down whose walk on from rest on
                // the node arrives is taken from rest there, as a fling is;
                // any other edge, a level or rising one included, is avoided
                // and planned round (bot-navigation.md section 3).
                let edge = (self.path[self.next], self.path[self.next + 1]);
                if (flat(w) < ON_NODE || held) && here.z < w.z + ARRIVE {
                    let drop = g.nodes[edge.1 as usize].z < w.z - vcod_common::pmove::STEPSIZE;
                    if !held && drop && self.edge_holds(world, w, Vec3::ZERO, false, g) {
                        self.held = Some(edge);
                        break;
                    }
                    self.avoid_edge(edge.0, edge.1);
                    self.reset();
                    return None;
                }
                break;
            }
            // Taken as it is, but from at rest on the node: passed at the
            // pace that had it avoided, bots ran off the crow's nest's top
            // at (4928, 61, 1254) on a turn, over the spar at 1212 and down
            // to the deck at 400 (bot-navigation.md section 3).
            if level
                && (reached || past)
                && planned_round
                && !held
                && !self.edge_holds(world, here, body.velocity, body.on_ladder, g)
            {
                self.held = Some((self.path[self.next], self.path[self.next + 1]));
                break;
            }
            if level && (reached || past) {
                self.held = None;
                self.pull_base = None;
                self.next += 1;
                self.best = f32::INFINITY;
                self.idle = 0;
            } else {
                break;
            }
        }
        // Not on a landing: the landing is read against the edge under way.
        if !landed {
            self.pull(g, world, here, body);
        }
        let Some(&w) = self.path.get(self.next) else {
            // Arrived. A roam picks somewhere new; a point is walked at.
            self.drop_path();
            return match goal {
                Goal::To(p) => {
                    self.arrived = true;
                    self.arrived_at = here;
                    Some(self.arrival(g, p))
                }
                _ => {
                    self.dest = None;
                    None
                }
            };
        };
        let w = g.nodes[w as usize];
        let slack = if self.foot_slack { REACH } else { 0.0 };
        if flat(w) > OFF_PATH_STEPS * g.spacing + slack && !(self.long_mantles && self.mantling(g))
        {
            // Fallen this far off the edge under way, still in the air: round
            // it, as a landing under it below plans. Planned again with it,
            // a bot ran off mp_ship's ledge at (7256, 237, 166) down the
            // roof to the hull floor four times in 20 s (seed 125,
            // bot-navigation.md section 3).
            if self.next > 0 && !self.climbing(g) && w.z - here.z > FELL_BELOW {
                let edge = (self.path[self.next - 1], self.path[self.next]);
                self.avoid_edge(edge.0, edge.1);
                self.fell = true;
            }
            self.drop_path();
            return None;
        }
        // Fallen off the path: a waypoint more than a climb over the feet,
        // and no ladder leads to it. Plan again from where the body landed,
        // round the edge it fell off (mp_ship's lip at (5424, -187, 344),
        // under the shelf at 396).
        if body.on_ground && self.next > 0 && !self.climbing(g) && w.z - here.z > FELL_BELOW {
            let edge = (self.path[self.next - 1], self.path[self.next]);
            self.avoid_edge(edge.0, edge.1);
            self.reset();
            self.fell = true;
            return None;
        }
        // Landed more than a step under both ends of a level edge that is
        // no jump: off its side into a pocket the walk never went
        // through. mp_cassino's roof dent at (4948, -1413, 280), 23 under
        // the edge from (4938, -1393, 303) to (4991, -1440, 303), took a
        // bot whose next plan started at the nearest node, level with
        // the dent's floor behind its wall, and held it 4 s (seed 25,
        // bot-objectives.md 10.6). Plan again round the edge from a node
        // it walks or jumps to.
        if landed && self.next > 0 {
            let (a, b) = (self.path[self.next - 1], self.path[self.next]);
            let (za, zb) = (g.nodes[a as usize].z, g.nodes[b as usize].z);
            let step = vcod_common::pmove::STEPSIZE;
            if !g.has(a, b, |k| k.jump) && (za - zb).abs() < step && here.z < za.min(zb) - step {
                self.avoid_edge(a, b);
                self.reset();
                self.fell = true;
                return None;
            }
        }
        // Down a drop onto a floor partway, the waypoint more than a jump
        // under the feet and within reach flat, the edge's top more than a
        // jump over them: plan again from where it landed, the edge not
        // avoided here (a landing off the fall itself plans round it,
        // above). Within a jump of the top the body may still stand on the
        // drop's own slope (bot-navigation.md section 3, seed 2245).
        if body.on_ground
            && !body.on_ladder
            && self.next > 0
            && here.z - w.z > vcod_common::pmove::JUMP_HEIGHT
            && flat(w) < REACH
            && g.nodes[self.path[self.next - 1] as usize].z - here.z
                > vcod_common::pmove::JUMP_HEIGHT
        {
            self.reset();
            self.fell = true;
            return None;
        }
        // Run off a drop and brought to a stop short of the lip on the
        // edge's top, where the run on from rest no longer lands: plan
        // round the edge, once per edge, or a plan with no way round takes
        // it again from rest (bot-navigation.md section 3, the hold floor's
        // drop).
        if let Some(world) = world
            && body.on_ground
            && !body.on_ladder
            && self.next > 0
            && here.z - w.z > vcod_common::pmove::JUMP_HEIGHT
            && flat(w) > REACH
            && body.velocity.truncate().length() < UNPIN_SPEED
        {
            let edge = (self.path[self.next - 1], self.path[self.next]);
            let top = g.nodes[edge.0 as usize];
            if (here.z - top.z).abs() < vcod_common::pmove::STEPSIZE
                && !g.has(edge.0, edge.1, |k| k.jump)
                && !self.avoid.iter().any(|a| (a.0, a.1) == edge)
                && self.runs_off(world, edge, top, w)
                && !matches!(
                    walk_with(
                        world,
                        here,
                        Some(body.velocity),
                        w.truncate(),
                        Some(w.z),
                        Gait::Forward
                    ),
                    Walked::Arrived(_, false)
                )
            {
                self.avoid_edge(edge.0, edge.1);
                self.reset();
                return None;
            }
        }
        // A plan's first node more than a step over the feet is a floor
        // the body left while the search ran: a bot that wandered off
        // mp_ship's crow's nest top at 1254 onto the hatch's rim at 1219
        // headed back for the top, pinned on its face and slid along it
        // down the hatch. Plan again from a node it walks to.
        if body.on_ground
            && !body.on_ladder
            && self.next == 0
            && !self.proved_start
            && w.z - here.z > vcod_common::pmove::STEPSIZE
        {
            self.reset();
            self.fell = true;
            return None;
        }
        // Progress counts height too: a climb closes on nothing flat.
        let d = flat(w) + (w.z - here.z).abs();
        if d < self.best - 1.0 {
            self.best = d;
            self.idle = 0;
        } else {
            self.idle += 1;
            if self.idle > STUCK_TICKS {
                // Stuck on the way from the last node: plan round that edge
                // next tick. Before the first node, leave the bot to its own
                // unstick for a spell.
                // Either way the next plan starts at a node a walk from
                // the body reaches: the nearest may stand across a wall
                // (mp_cassino's slot at (945, -775, 64), seed 1 with no
                // vehicles; its roof dent, seed 25, bot-objectives.md 10.6).
                self.fell = true;
                self.gave_up += 1;
                if let Some(from) = self.next.checked_sub(1).map(|i| self.path[i]) {
                    let to = self.path[self.next];
                    self.avoid_edge(from, to);
                } else {
                    self.rest = REST_TICKS;
                }
                self.reset();
                return None;
            }
        }
        // Up a ladder from its foot: into the face first, the way the climb
        // that found the head went, then at the head once level with it.
        if self.climbing(g)
            && here.z < w.z - vcod_common::pmove::STEPSIZE
            && let Some(p) = g.face_ahead(g.nodes[self.path[self.next - 1] as usize])
        {
            return Some([p.x, p.y, w.z]);
        }
        Some(w.into())
    }

    /// The path toward the destination, once the search for it is done.
    /// A search starts only while `budget` has some left and runs at most
    /// [`PLAN_SLICE`] of it a tick; a destination the start does not reach
    /// fails before any search, and a point goal then heads for the reached
    /// node nearest it instead.
    fn plan(
        &mut self,
        g: &NavGraph,
        world: Option<&CollisionWorld>,
        goal: crate::bots::Goal,
        here: Vec3,
        budget: &mut u32,
        rand: &mut dyn FnMut() -> i32,
    ) -> Option<Vec<u32>> {
        if *budget == 0 {
            return None;
        }
        if self.search.is_none() {
            self.proved_start = true;
            self.jump_start = false;
            let start = match world {
                _ if self.on_rungs => g.nearest_ladder_end(here),
                // Nor from a node a parked vehicle stands over.
                Some(w)
                    if std::mem::take(&mut self.fell)
                        || g.off_floor(here)
                        || g.nearest(here.into()).is_some_and(|n| self.blocks(n)) =>
                {
                    g.walkable_start(w, here).map(|(n, jump)| {
                        self.jump_start = jump;
                        n
                    })
                }
                _ => {
                    self.proved_start = false;
                    g.nearest(here.into())
                }
            }?;
            if self.dest.is_none() {
                self.dest = match goal {
                    crate::bots::Goal::Roam => g.roam_from(start, here, rand),
                    crate::bots::Goal::Away(t) => g.flee_from(start, here, t.into(), rand),
                    _ => None,
                };
            }
            let to = match goal {
                crate::bots::Goal::To(_) => self.dest.and_then(|d| g.nearest_reached(start, d)),
                _ => self.dest.filter(|&d| g.reaches(start, d)),
            };
            let Some(to) = to else {
                // Unreachable from here: a roam picks again next time.
                self.reset();
                self.rest = REST_TICKS;
                return None;
            };
            // A destination a parked vehicle stands over, as a boarding's
            // is: the clear node nearest it, and from there a point goal
            // walks at its point.
            let to = if self.blocks(to) {
                g.clear_near(to, &self.blocked, |n| g.reaches(start, n))
                    .unwrap_or(to)
            } else {
                to
            };
            // Round the edges it got stuck on when there is a way round,
            // else through them again: whatever blocked one may have moved.
            // Not through one that failed again so ([`Self::avoid_edge`]).
            let avoid: Vec<(u32, u32)> = self.avoid.iter().map(|e| (e.0, e.1)).collect();
            self.round = !avoid.is_empty();
            self.search = Some(
                Search::new(g, start, to, avoid)
                    .crowded(self.crowd.clone(), self.crowd_cost)
                    .noisy(self.noise)
                    .blocking(self.blocked.clone())
                    .shunning(self.shun.clone())
                    .jumps_costing(self.jump_cost),
            );
        }
        let search = self.search.as_mut()?;
        let mut slice = (*budget).min(PLAN_SLICE);
        let before = slice;
        let plan = search.run(g, &mut slice);
        // A plan that finds its path at once still costs one.
        *budget -= (before - slice).max(1).min(*budget);
        match plan {
            Plan::Pending => None,
            Plan::Found(path) => {
                let s = self.search.take()?;
                self.through = s.blocked.is_empty() && !self.blocked.is_empty();
                self.shun_through = !self.shun.is_empty()
                    && (s.shun.is_empty() || self.meets_shunned(g, here, &path));
                self.penned = false;
                Some(path)
            }
            Plan::Failed if self.search.as_ref().is_some_and(|s| !s.shun.is_empty()) => {
                // No way round the bodies in the way: the same search
                // through them, the bot waiting on them meanwhile.
                let s = self.search.take()?;
                self.search = Some(
                    Search::new(g, s.from, s.to, s.avoid)
                        .crowded(s.crowd, s.crowd_cost)
                        .noisy(s.noise)
                        .blocking(s.blocked)
                        .jumps_costing(s.jump_cost),
                );
                None
            }
            Plan::Failed => {
                let s = self.search.take()?;
                // Round the edges it got stuck on, then through all but the
                // dead ones.
                let blocked = if self.round {
                    self.round = false;
                    s.blocked.clone()
                } else if s.blocked.is_empty() {
                    self.reset();
                    self.rest = REST_TICKS;
                    return None;
                } else if s.reached() < PEN_NODES {
                    // Hemmed in by parked vehicles: no plan until one
                    // moves off, and the bot reads as stranded meanwhile.
                    // Planned through them, bots that got out of a tank
                    // onto mp_cassino's terrace at (383, -916, 76), the
                    // tank parked over the terrace's one stair, wedged on
                    // its hull (seeds 23 and 1).
                    self.penned = true;
                    self.reset();
                    self.rest = REST_TICKS;
                    return None;
                } else {
                    // A destination no way round them reaches: through
                    // them, as round an edge it got stuck on.
                    Arc::default()
                };
                let dead = self.dead.iter().map(|e| (e.0, e.1)).collect();
                self.search = Some(
                    Search::new(g, s.from, s.to, dead)
                        .crowded(s.crowd, s.crowd_cost)
                        .noisy(s.noise)
                        .blocking(blocked)
                        .jumps_costing(s.jump_cost)
                        .risking(),
                );
                None
            }
        }
    }

    /// Drops the plan, so the next waypoint plans afresh to the same goal.
    pub fn replan(&mut self) {
        self.reset();
    }

    /// The bot's own stall circle caught the body on the way to the
    /// current waypoint and sent it on an unstick spell: plan round the
    /// edge under way. The spell runs the body off the path, and the plan
    /// from where it ends took the same edge back into the same trap
    /// (mp_ship's hold crates at (2234, 494, -42), bot-navigation.md
    /// section 3).
    pub fn stuck(&mut self) {
        if let Some(from) = self.next.checked_sub(1).and_then(|i| self.path.get(i))
            && let Some(&to) = self.path.get(self.next)
        {
            self.avoid_edge(*from, to);
            self.reset();
            self.fell = true;
            self.gave_up += 1;
        }
    }

    /// Test-facing: the ticks of the wait aside for a climber under way.
    pub fn yielding(&self) -> u32 {
        self.yield_ticks
    }

    /// The edges of the path still ahead, the one under way first.
    pub fn ahead(&self) -> impl Iterator<Item = (u32, u32)> + '_ {
        self.path
            .get(self.next.saturating_sub(1)..)
            .unwrap_or(&[])
            .windows(2)
            .map(|w| (w[0], w[1]))
    }

    /// Test-facing: the current waypoint's node and the one after it.
    pub fn current(&self) -> (Option<u32>, Option<u32>) {
        (
            self.path.get(self.next).copied(),
            self.path.get(self.next + 1).copied(),
        )
    }

    /// Whether the current waypoint is a leap's foot, or the node of an edge
    /// the body comes onto too fast ([`Self::stop_at`], [`Self::held`]): the bot comes to
    /// rest on it before it heads on.
    pub fn holding(&self, g: &NavGraph) -> bool {
        match (self.path.get(self.next), self.path.get(self.next + 1)) {
            (Some(&a), Some(&b)) => {
                g.has(a, b, |k| k.leap) || self.stop_at == Some((a, b)) || self.held == Some((a, b))
            }
            _ => false,
        }
    }

    /// Whether the current waypoint is the top of a creep that misses off a
    /// turn ([`Self::creeps_off`]), which the bot slows onto before it backs
    /// off: the graph's walk crept from the node with a run straight at the
    /// drop behind it, and a bot that came onto mp_depot's roof drop off a
    /// turn left the lip at 81 units/s to the walk's 14 and fell past the
    /// ledge. Re-proved at the body's pace each tick instead, the creep
    /// read as arriving by a floor 3 under the ledge that the bot's own
    /// fall missed; held at every creep's top, bots slowed onto mp_ship's
    /// lip node (5402, 321, 344) from 8 over it and bounced off the stair
    /// below (bot-navigation.md section 3).
    fn creep_top(&mut self, world: &CollisionWorld, g: &NavGraph) -> bool {
        if !self.creeps_off(world, g) {
            return false;
        }
        self.stop_at = Some((self.path[self.next], self.path[self.next + 1]));
        true
    }

    /// Whether the way on from the current waypoint is a drop more than a
    /// jump deep that neither a run nor a back down from the node arrives
    /// at, so the graph's walk crept it ([`Gait::Creep`]), that misses
    /// coming onto it off a turn ([`creep_takes_a_turn`]) and arrives from
    /// rest: where a stop helps. Held at every creep that misses off a
    /// turn, mp_ship's bots slowed onto its lip node (5402, 321, 344) and
    /// bounced off the stair below, and stalled about twice as long over
    /// the sweep (bot-navigation.md section 3). A body
    /// that comes onto it off a turn carries its pace over the lip and past
    /// the grab: mp_depot's thin ledges at (-2749, -1281, 326) and
    /// (-2755, -1150, 326), 182 under the roof (bot-navigation.md
    /// section 3). So is a creep that arrives only from rest: the back down
    /// off mp_ship's mast head (3675, -95.1, 1147.1) on the spar beside the
    /// rungs' top. Walked once per edge.
    fn creeps_off(&mut self, world: &CollisionWorld, g: &NavGraph) -> bool {
        let (Some(&a), Some(&b)) = (self.path.get(self.next), self.path.get(self.next + 1)) else {
            return false;
        };
        if let Some((e, creep)) = self.creep_off
            && e == (a, b)
        {
            return creep;
        }
        let (from, to) = (g.nodes[a as usize], g.nodes[b as usize]);
        let arrives = |pace, gait| {
            matches!(
                walk_with(world, from, pace, to.truncate(), Some(to.z), gait),
                Walked::Arrived(..)
            )
        };
        // Or only from rest: a ladder's back down `rung` proved that way.
        let creep = to.z < from.z - vcod_common::pmove::JUMP_HEIGHT
            && !self.runs_off(world, (a, b), from, to)
            && arrives(Some(Vec3::ZERO), Gait::Creep)
            && ((!arrives(None, Gait::Backward)
                && !creep_takes_a_turn(world, from, to.truncate(), Some(to.z)))
                || !arrives(None, Gait::Creep));
        self.creep_off = Some(((a, b), creep));
        creep
    }

    /// Whether the way to the current waypoint is a leap.
    pub fn leaping(&self, g: &NavGraph) -> bool {
        let Some(i) = self.next.checked_sub(1) else {
            return false;
        };
        match (self.path.get(i), self.path.get(self.next)) {
            (Some(&a), Some(&b)) => g.has(a, b, |k| k.leap),
            _ => false,
        }
    }

    /// Up a ladder from a foot square in front of the rungs, the foot: the
    /// climb that proved the head went up level with it along the face,
    /// and a body that grabbed the rungs off that line strafes back to it
    /// (`bots::Bot::steer`).
    pub fn climb_line(&self, g: &NavGraph) -> Option<glam::Vec2> {
        if !self.climbing(g) {
            return None;
        }
        let foot = g.nodes[self.path[self.next - 1] as usize];
        g.face_ahead(foot).map(|_| foot.truncate())
    }

    /// Down a ladder whose fall kills, from its head: the yaw square into
    /// the rungs' face and the middle of the rungs along it. The creep off
    /// the head backs along that yaw strafing to the middle
    /// (`bots::Bot::steer`). Backed toward the foot, as the graph's walk
    /// did, it grabs mp_ship's crow's nest rungs only from a diamond about
    /// 4 units across round the head; a bot that came onto the head off
    /// the line, or along the rim, fell down the hatch past them
    /// (bot-navigation.md section 3).
    pub fn back_face(&self, g: &NavGraph) -> Option<(f32, glam::Vec2)> {
        let i = self.next.checked_sub(1)?;
        let (head, foot) = (
            g.nodes[*self.path.get(i)? as usize],
            g.nodes[*self.path.get(self.next)? as usize],
        );
        if foot.z > head.z - self.deadly_fall() {
            return None;
        }
        let (lo, hi) = *g
            .ladder_boxes
            .iter()
            .filter(|b| box_flat(b, head) < LADDER_NEAR && box_flat(b, foot) < LADDER_NEAR)
            .min_by(|a, b| {
                (box_flat(a, head) + box_flat(a, foot))
                    .total_cmp(&(box_flat(b, head) + box_flat(b, foot)))
            })?;
        let ext = hi - lo;
        // The thin axis is across the face; the foot stands on the
        // climber's side of it.
        let (a, b) = if ext.x < ext.y { (0, 1) } else { (1, 0) };
        let into = ((lo[a] + hi[a]) * 0.5 - foot[a]).signum();
        let yaw = match (a, into > 0.0) {
            (0, true) => 0.0,
            (0, false) => 180.0,
            (_, true) => 90.0,
            (_, false) => -90.0,
        };
        let mut middle = foot.truncate();
        middle[b] = (lo[b] + hi[b]) * 0.5;
        Some((yaw, middle))
    }

    /// Whether the edge on from the current waypoint climbs more than a
    /// jump: the waypoint is a ladder's foot.
    fn rising(&self, g: &NavGraph) -> bool {
        match (self.path.get(self.next), self.path.get(self.next + 1)) {
            (Some(&a), Some(&b)) => {
                g.nodes[b as usize].z > g.nodes[a as usize].z + vcod_common::pmove::JUMP_HEIGHT
            }
            _ => false,
        }
    }

    /// Whether the edge on from the current waypoint drops more than a step.
    fn dropping(&self, g: &NavGraph) -> bool {
        match (self.path.get(self.next), self.path.get(self.next + 1)) {
            (Some(&a), Some(&b)) => {
                g.nodes[b as usize].z < g.nodes[a as usize].z - vcod_common::pmove::STEPSIZE
            }
            _ => false,
        }
    }

    /// Whether the way to the current waypoint climbs more than a jump: a
    /// ladder's foot to its head.
    fn climbing(&self, g: &NavGraph) -> bool {
        let Some(i) = self.next.checked_sub(1) else {
            return false;
        };
        match (self.path.get(i), self.path.get(self.next)) {
            (Some(&a), Some(&b)) => {
                g.nodes[b as usize].z > g.nodes[a as usize].z + vcod_common::pmove::JUMP_HEIGHT
            }
            _ => false,
        }
    }

    /// How far the plan runs on from `at` along a straight, level line of
    /// plain edges: every node within [`SPRINT_SLACK`] of the line from
    /// `at` to the last, each step under a stair's rise, no jump, leap or
    /// risky edge. Where a United Offensive bot sprints (bot-navigation.md,
    /// "Other rules"): the graph's walks ran at 190 units/s, and a sprint's
    /// 304 off a turn or a drop flies a body past what they proved.
    pub fn straight_run(&self, g: &NavGraph, at: Vec3) -> f32 {
        if self.arrived {
            return 0.0;
        }
        let ahead = &self.path[self.next.min(self.path.len())..];
        let mut pts = vec![at];
        let mut prev = self
            .next
            .checked_sub(1)
            .and_then(|i| self.path.get(i).copied());
        for &n in ahead.iter().take(SPRINT_LOOK) {
            let p = g.nodes[n as usize];
            let plain = prev.is_none_or(|m| {
                !g.has(m, n, |k| k.jump) && !g.has(m, n, |k| k.leap) && !g.has(m, n, |k| k.risky)
            });
            if !plain || (p.z - pts[pts.len() - 1].z).abs() > vcod_common::pmove::STEPSIZE {
                break;
            }
            let line = (p - at).truncate();
            let len = line.length();
            let across = |q: &Vec3| {
                let d = (*q - at).truncate();
                if len < 1.0 {
                    d.length()
                } else {
                    (d.x * line.y - d.y * line.x).abs() / len
                }
            };
            if pts.iter().any(|q| across(q) > SPRINT_SLACK) {
                break;
            }
            pts.push(p);
            prev = Some(n);
        }
        pts.windows(2)
            .map(|w| (w[1] - w[0]).truncate().length())
            .sum()
    }

    /// Whether the way to the current waypoint, a drop of more than
    /// `deep`, is taken at a run rather than backed onto: whether the
    /// graph's walk proved it running off ([`Self::runs_off`]). The pass
    /// onto it re-proved the run at the body's pace (`edge_holds`).
    pub fn running_off(
        &mut self,
        g: &NavGraph,
        world: Option<&CollisionWorld>,
        at: Vec3,
        deep: f32,
    ) -> bool {
        let (Some(world), Some(i)) = (world, self.next.checked_sub(1)) else {
            return false;
        };
        let (Some(&a), Some(&b)) = (self.path.get(i), self.path.get(self.next)) else {
            return false;
        };
        let (from, to) = (g.nodes[a as usize], g.nodes[b as usize]);
        to.z < at.z - deep && self.runs_off(world, (a, b), from, to)
    }

    /// Whether the way to the current waypoint is a drop more than a jump
    /// under `at` that a run off misses: the graph's walk backed down it
    /// ([`Self::runs_off`]), and the bot backs down it too.
    pub fn backs_off(&mut self, g: &NavGraph, world: Option<&CollisionWorld>, at: Vec3) -> bool {
        let (Some(world), Some(i)) = (world, self.next.checked_sub(1)) else {
            return false;
        };
        let (Some(&a), Some(&b)) = (self.path.get(i), self.path.get(self.next)) else {
            return false;
        };
        let (from, to) = (g.nodes[a as usize], g.nodes[b as usize]);
        to.z < at.z - vcod_common::pmove::JUMP_HEIGHT
            && !g.has(a, b, |k| k.jump)
            && !self.runs_off(world, (a, b), from, to)
    }

    /// Whether a run that misses the drop `edge` onto `to` can fall far
    /// enough to hurt ([`a_miss_hurts`]). Probed once per edge.
    fn miss_hurts(&mut self, world: &CollisionWorld, edge: (u32, u32), to: Vec3) -> bool {
        if let Some((e, hurts)) = self.miss_hurt
            && e == edge
        {
            return hurts;
        }
        let hurts = a_miss_hurts(world, to);
        self.miss_hurt = Some((edge, hurts));
        hurts
    }

    /// Whether a forward walk from `from` arrives on `to`'s floor without a
    /// jump: the first gait the graph's walk tries (`walk_to`), so a drop
    /// it misses was backed down. Walked once per edge.
    fn runs_off(&mut self, world: &CollisionWorld, edge: (u32, u32), from: Vec3, to: Vec3) -> bool {
        if let Some((e, run)) = self.run_off
            && e == edge
        {
            return run;
        }
        let run = matches!(
            walk_as(world, from, to.truncate(), Some(to.z), Gait::Forward),
            Walked::Arrived(_, false)
        );
        self.run_off = Some((edge, run));
        run
    }

    /// Whether the way to the current waypoint is a jump edge, or the way
    /// from the body to a plan's first node that only a jump walked
    /// ([`Self::jump_start`]).
    pub fn jumping(&self, g: &NavGraph) -> bool {
        let Some(i) = self.next.checked_sub(1) else {
            return self.jump_start && !self.path.is_empty();
        };
        match (self.path.get(i), self.path.get(self.next)) {
            (Some(&a), Some(&b)) => g.has(a, b, |k| k.jump),
            _ => false,
        }
    }

    /// Whether the way to the current waypoint is a mantle edge
    /// ([`super::Edge::mantle`]).
    pub fn mantling(&self, g: &NavGraph) -> bool {
        self.mantle_edge(g).is_some()
    }

    /// The mantle edge the current waypoint ends, if it is one.
    pub fn mantle_edge(&self, g: &NavGraph) -> Option<(u32, u32)> {
        let i = self.next.checked_sub(1)?;
        let e = (*self.path.get(i)?, *self.path.get(self.next)?);
        g.has(e.0, e.1, |k| k.mantle).then_some(e)
    }

    /// On a mantle edge, where its probe landed ([`super::Edge::land`]).
    pub fn mantle_land(&self, g: &NavGraph) -> Option<Vec3> {
        let (a, b) = self.mantle_edge(g)?;
        g.edge(a, b)?.land
    }

    /// Whether the bot jumps on the way to the current waypoint: a jump
    /// edge, unless its walk on from where the body passed the edge's
    /// first node ran it without the jump ([`Self::walked_jump`]).
    pub fn jump_cued(&self, g: &NavGraph) -> bool {
        let into = self
            .next
            .checked_sub(1)
            .and_then(|i| Some((*self.path.get(i)?, *self.path.get(self.next)?)));
        self.jumping(g) && (self.walked_jump.is_none() || self.walked_jump != into)
    }

    /// On a jump edge proved aimed beside its top ([`super::Edge::aim`]),
    /// the point to steer at in place of the top.
    pub fn jump_aim(&self, g: &NavGraph) -> Option<glam::Vec2> {
        let i = self.next.checked_sub(1)?;
        let (a, b) = (*self.path.get(i)?, *self.path.get(self.next)?);
        g.edge(a, b)?.aim
    }

    /// Whether the way to the current waypoint is a crouched jump edge: the
    /// bot holds crouch from the cmd after its jump until it lands, as
    /// [`Gait::CrouchJump`] proved it.
    pub fn crouch_jumping(&self, g: &NavGraph) -> bool {
        let Some(i) = self.next.checked_sub(1) else {
            return false;
        };
        match (self.path.get(i), self.path.get(self.next)) {
            (Some(&a), Some(&b)) => g.has(a, b, |k| k.crouched),
            _ => false,
        }
    }

    /// On a jump edge with the body's jump still cooling down
    /// (`PM_CheckJump` refuses one inside `JUMP_COOLDOWN_MS` of the last)
    /// or under 1.5's penalty ([`jump_ready`]), the edge's foot to come to
    /// rest on until it is ready: the graph's walk proved the edge with a
    /// full jump to hand. A bot that jumped onto
    /// mp_depot's crate at (-2275, 131, 46) ran on for the rim at 66 with
    /// the jump refused and off the crate's edge into the open crate below
    /// (bot-navigation.md section 3).
    pub fn jump_wait(&self, g: &NavGraph, at: [f32; 3], ps: &PlayerState) -> Option<[f32; 3]> {
        if !self.jumping(g) || !ps.on_ground || ps.on_ladder || jump_ready(ps) {
            return None;
        }
        // A jump start's foot is where the body stands.
        let Some(i) = self.next.checked_sub(1) else {
            return Some(at);
        };
        let foot = g.nodes[self.path[i] as usize];
        let rise = at[2] - foot.z;
        if rise.abs() >= vcod_common::pmove::STEPSIZE {
            return None;
        }
        // Stepped up off the foot's floor onto a ledge the top still needs
        // a jump from, it waits where it stands: the walk jumped from up
        // there too, and one sent back down off mp_cassino's ledge at
        // (3667, -1240, 128) jumped again from the foot 53 under the top,
        // into the penalty, and back for good (bot-navigation.md "Jumps").
        let top = g.nodes[self.path[self.next] as usize];
        let ledge = rise > ARRIVE && top.z - at[2] > vcod_common::pmove::STEPSIZE;
        Some(if ledge { at } else { foot.into() })
    }

    /// Whether the jump edge [`Self::jump_wait`] holds the body at `spot`
    /// for fails from there at rest, where the wait leaves it: then the
    /// plan is dropped and the edge avoided. The graph proved the edge
    /// with a running start, and mp_cassino's from the roof at (4992,
    /// -1338, 295) to (4938, -1393, 303) clears the trench at 256 between
    /// them only so: a UO bot that waited out the penalty there walked
    /// into the trench and jumped at the roof's face 47 over it until it
    /// gave up (bot-objectives.md 10.6). An edge already avoided had no
    /// way round and is waited on. A jump start has no edge and is kept.
    pub fn rest_jump_fails(
        &mut self,
        g: &NavGraph,
        world: &CollisionWorld,
        spot: [f32; 3],
    ) -> bool {
        let (Some(i), Some(&b)) = (self.next.checked_sub(1), self.path.get(self.next)) else {
            return false;
        };
        let edge = (self.path[i], b);
        if self.avoid.iter().any(|e| (e.0, e.1) == edge) {
            return false;
        }
        let lands = match self.rest_jump {
            Some((e, lands)) if e == edge => lands,
            _ => {
                let top = g.nodes[b as usize];
                let gait = if g.has(edge.0, edge.1, |k| k.crouched) {
                    Gait::CrouchJump
                } else {
                    Gait::Forward
                };
                let walked = as_a_bot(|| {
                    walk_with(
                        world,
                        spot.into(),
                        Some(Vec3::ZERO),
                        g.aim(edge.0, edge.1),
                        Some(top.z),
                        gait,
                    )
                });
                let lands = matches!(walked, Walked::Arrived(..));
                self.rest_jump = Some((edge, lands));
                lands
            }
        };
        if lands {
            return false;
        }
        self.avoid_edge(edge.0, edge.1);
        self.reset();
        true
    }

    /// String-pulling: takes the waypoint on to the next node of the plan
    /// when a body walks there straight from where it stands
    /// ([`straight_floor`], then a walk from the body at its pace that
    /// arrives without a jump, a dip or a slide), so a bot cuts the grid's
    /// 45 degree kinks. Only along a run of plain edges
    /// ([`NavGraph::plain_edge`]) whose last node leaves by one too: a
    /// jump, leap, ladder, drop or risky edge is still come onto along the
    /// edge the graph proved. One node a tick, no further than
    /// [`PULL_STEPS`] grid steps and a step's height away, every node
    /// skipped within [`PULL_SLACK`] steps of the line, on the ground
    /// past the plan's first node. While a pulled waypoint is ahead the
    /// straight line to it is checked again every tick; a failed check
    /// falls back to the furthest skipped node it still reaches
    /// (bot-navigation.md, "String-pulling").
    fn pull(&mut self, g: &NavGraph, world: Option<&CollisionWorld>, here: Vec3, body: Footing) {
        let Some(world) = world else {
            return;
        };
        if !body.on_ground || body.on_ladder || self.arrived || self.next >= self.path.len() {
            return;
        }
        let node = |i: usize| g.nodes[self.path[i] as usize];
        if let Some(base) = self.pull_base
            && !straight_floor(world, here, node(self.next))
        {
            let back = (base + 1..self.next)
                .rev()
                .find(|&j| straight_floor(world, here, node(j)));
            self.next = back.unwrap_or(base);
            if back.is_none() {
                self.pull_base = None;
            }
            self.best = f32::INFINITY;
            self.idle = 0;
            self.pull_wait = PULL_RETRY;
            return;
        }
        if self.pull_wait > 0 {
            self.pull_wait -= 1;
            return;
        }
        let k = self.next + 1;
        if self.next == 0 || k >= self.path.len() || self.held.is_some() {
            return;
        }
        let plain = |i: usize| {
            i + 1 >= self.path.len()
                || (g.plain_edge(self.path[i], self.path[i + 1])
                    && !self
                        .avoid
                        .iter()
                        .any(|e| (e.0, e.1) == (self.path[i], self.path[i + 1])))
        };
        if !(plain(self.next - 1) && plain(self.next) && plain(k)) {
            return;
        }
        let to = node(k);
        let line = (to - here).truncate();
        if line.length() > PULL_STEPS * g.spacing
            || (to.z - here.z).abs() > vcod_common::pmove::STEPSIZE
        {
            return;
        }
        // The node skipped stays near the line: off it the grid went round
        // something no trace sees (a hazard's edge, a fall beside a run).
        let skipped = (node(self.next) - here).truncate();
        let t = (skipped.dot(line) / line.length_squared().max(1.0)).clamp(0.0, 1.0);
        if (skipped - line * t).length() > PULL_SLACK * g.spacing {
            return;
        }
        let mut trail = Trail::default();
        let walked = straight_floor(world, here, to)
            && matches!(
                walk_sliding(
                    world,
                    here,
                    Some(body.velocity),
                    self.move_scale,
                    to.truncate(),
                    Some(to.z),
                    Gait::Forward,
                    &mut trail,
                ),
                Walked::Arrived(_, false)
            )
            && trail.dip <= vcod_common::pmove::STEPSIZE
            && trail.slid.is_empty();
        if !walked {
            self.pull_wait = PULL_RETRY;
            return;
        }
        self.pull_base.get_or_insert(self.next);
        self.next = k;
        self.best = f32::INFINITY;
        self.idle = 0;
    }

    /// Where a walked-out point goal is headed: the point, or the
    /// destination node when [`NavGraph::goal_node`] kept it off a perch
    /// the point stands on.
    fn arrival(&self, g: &NavGraph, p: [f32; 3]) -> [f32; 3] {
        match (self.dest, g.nearest(p)) {
            (Some(d), Some(n)) if d != n && g.perches.contains(&n) => g.nodes[d as usize].into(),
            _ => p,
        }
    }

    /// The drop edge the body just landed partway down: in the air off a
    /// drop more than a jump high (its edge kept in [`Self::falling_off`]),
    /// it came down more than a jump over the drop's foot, within
    /// [`REACH`] of it across and more than a jump under its top.
    fn landed_partway(
        &mut self,
        g: &NavGraph,
        here: Vec3,
        body: Footing,
        landed: bool,
    ) -> Option<(u32, u32)> {
        let jump = vcod_common::pmove::JUMP_HEIGHT;
        if body.on_ground || body.on_ladder {
            let edge = self.falling_off.take().filter(|_| landed)?;
            let (top, foot) = (g.nodes[edge.0 as usize], g.nodes[edge.1 as usize]);
            return (here.z - foot.z > jump
                && (foot - here).truncate().length() < REACH
                && top.z - here.z > jump)
                .then_some(edge);
        }
        if self.next > 0
            && let (Some(&a), Some(&b)) = (self.path.get(self.next - 1), self.path.get(self.next))
            && g.nodes[a as usize].z - g.nodes[b as usize].z > jump
            && self.falling_off.is_none()
        {
            self.falling_off = Some((a, b));
        }
        None
    }

    /// Whether the plan to the point goal is walked out: the body heads
    /// straight at the point ([`Self::arrival`]).
    pub fn walked_out(&self) -> bool {
        self.arrived
    }

    /// Whether the last plan found the body hemmed in by parked vehicles
    /// ([`Self::blocked`]) with no way out round them: what the bot reads
    /// as stranded (`bots::BotView::stranded`).
    pub fn penned(&self) -> bool {
        self.penned
    }

    /// Whether a parked vehicle stands over node `n` ([`Self::blocked`]).
    fn blocks(&self, n: u32) -> bool {
        self.blocked.binary_search(&n).is_ok()
    }

    /// Plans round the edge `a` to `b` for [`AVOID_TICKS`]. One stuck on
    /// again while still avoided was planned through for want of a way
    /// round and failed again: no plan takes it until it expires. Taken
    /// again and again, a bot paced the slalom on mp_ship's deck at (3565,
    /// 95, 352) for most of a UO match: its way out north is proved only
    /// with a running start, and no turn into it gives one
    /// (bot-combat.md 17.1).
    fn avoid_edge(&mut self, a: u32, b: u32) {
        // A plan parked through it is no plan round it: taken up when the
        // goal came back, it led a bot down mp_ship's slot drop again 5 s
        // after the drop was avoided (bot-objectives.md 14.9).
        if self
            .parked
            .as_ref()
            .is_some_and(|k| k.path.windows(2).any(|w| (w[0], w[1]) == (a, b)))
        {
            self.parked = None;
        }
        let until = self.clock + AVOID_TICKS;
        if self.avoid.iter().any(|e| (e.0, e.1) == (a, b)) {
            self.dead.push((a, b, until));
        }
        self.avoid.push((a, b, until));
    }

    fn reset(&mut self) {
        self.dest = None;
        self.search = None;
        self.drop_path();
    }

    /// The path gone with what indexes or follows it: the pulled base, the
    /// held waypoint and the walked-out mark.
    fn drop_path(&mut self) {
        self.path.clear();
        self.held = None;
        self.pull_base = None;
        self.arrived = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nav::build::tests::map_world;
    use crate::nav::graph::tests::grid_and_island;
    use crate::nav::graph::tests::row;
    use crate::nav::graph::tests::tiny;
    use crate::nav::graph::tests::zigzag;

    /// A vehicle parked over node 1 of [`tiny`]: a path under way through
    /// it is planned again round by node 3, and a goal on it is still
    /// walked to. Parked over node 3 as well, node 0 is hemmed in until
    /// the vehicle moves off.
    #[test]
    fn a_plan_runs_round_the_nodes_a_parked_vehicle_covers() {
        use crate::bots::Goal;
        let g = tiny();
        let mut f = Follower::default();
        let mut budget = 100;
        let mut step = |f: &mut Follower, goal| {
            let at = [-16.0, 16.0, 0.0];
            f.waypoint(
                &g,
                None,
                goal,
                at,
                Footing::STANDING,
                &mut budget,
                &mut || 0,
            )
        };
        let goal = Goal::To([64.0, 0.0, 0.0]);
        assert_eq!(step(&mut f, goal), Some([32.0, 0.0, 0.0]));
        f.blocked = Arc::new([1]);
        assert_eq!(step(&mut f, goal), Some([32.0, 64.0, 0.0]));
        assert_eq!(f.path, [0, 3, 2]);
        let mut f = Follower {
            blocked: Arc::new([1]),
            ..Follower::default()
        };
        assert_eq!(
            step(&mut f, Goal::To([32.0, 0.0, 0.0])),
            Some([32.0, 0.0, 0.0])
        );
        // Both ways covered: hemmed in, no plan.
        let mut f = Follower {
            blocked: Arc::new([1, 3]),
            ..Follower::default()
        };
        assert_eq!((0..3).find_map(|_| step(&mut f, goal)), None);
        assert!(f.penned());
        f.blocked = Arc::default();
        let w = (0..REST_TICKS + 2).find_map(|_| step(&mut f, goal));
        assert_eq!(w, Some([32.0, 0.0, 0.0]));
        assert!(!f.penned());
    }

    /// No plan starts while pmove's climb is under way: from mid-climb the
    /// nearest node is the foot, and a plan from there walked the bot back
    /// off mp_crash's ledge (bot-navigation.md 3.15). Landed on top, it
    /// plans from there.
    #[test]
    fn no_plan_starts_mid_climb() {
        use crate::bots::Goal;
        let nodes = vec![
            Vec3::ZERO,
            Vec3::new(40.0, 0.0, 60.0),
            Vec3::new(120.0, 0.0, 60.0),
        ];
        let mut g = NavGraph::from_parts(nodes, vec![vec![1], vec![0, 2], vec![1]]);
        g.edge_mut(0, 1).unwrap().mantle = true;
        let goal = Goal::To([120.0, 0.0, 60.0]);
        let air = Footing {
            still: false,
            on_ground: false,
            ..Footing::STANDING
        };
        let mut f = Follower {
            climbing: true,
            ..Follower::default()
        };
        let mid = [8.0, 0.0, 30.0];
        assert_eq!(
            f.waypoint(&g, None, goal, mid, air, &mut 100, &mut || 0),
            None
        );
        assert!(f.path.is_empty());
        f.climbing = false;
        let top = [40.0, 0.0, 60.0];
        f.waypoint(&g, None, goal, top, Footing::STANDING, &mut 100, &mut || 0);
        assert_eq!(f.path.first(), Some(&1));
    }

    /// A body standing on node 1 of [`tiny`], in the way of a bot bound
    /// along 0, 1, 2: after [`BODY_TICKS`] held up the plan goes round by
    /// node 3. On [`row`] there is no way round: the plan goes through him,
    /// and is not dropped again until the nodes he stood over are let go.
    #[test]
    fn a_body_in_the_way_is_planned_round() {
        use crate::bots::Goal;
        let held = |f: &mut Follower, g: &NavGraph, at: [f32; 3], others: &[(Vec3, Vec3)]| {
            f.bodies_in_way(g, at.into(), Footing::STANDING, others)
        };
        let g = tiny();
        let mut f = Follower::default();
        let at = [-16.0, 16.0, 0.0];
        let goal = Goal::To([64.0, 0.0, 0.0]);
        let step = |f: &mut Follower| {
            f.waypoint(&g, None, goal, at, Footing::STANDING, &mut 100, &mut || 0)
        };
        assert_eq!(step(&mut f), Some([32.0, 0.0, 0.0]));
        let other = [(Vec3::new(28.0, 4.0, 0.0), Vec3::ZERO)];
        for _ in 1..BODY_TICKS {
            assert!(!held(&mut f, &g, at, &other));
        }
        // One behind the bot holds nothing up.
        let behind = [(Vec3::new(-40.0, 16.0, 0.0), Vec3::ZERO)];
        assert!(!held(&mut f, &g, at, &behind));
        for _ in 1..BODY_TICKS {
            assert!(!held(&mut f, &g, at, &other));
        }
        assert!(held(&mut f, &g, at, &other));
        assert_eq!(step(&mut f), Some([32.0, 64.0, 0.0]));
        assert_eq!(f.path, [0, 3, 2]);
        // A corridor: through him, once.
        let g = row();
        let mut f = Follower::default();
        let at = [8.0, 0.0, 0.0];
        let goal = Goal::To([160.0, 0.0, 0.0]);
        let step = |f: &mut Follower| {
            f.waypoint(&g, None, goal, at, Footing::STANDING, &mut 100, &mut || 0)
        };
        let other = [(Vec3::new(36.0, 0.0, 0.0), Vec3::ZERO)];
        assert_eq!(step(&mut f), Some([32.0, 0.0, 0.0]));
        for _ in 1..BODY_TICKS {
            assert!(!held(&mut f, &g, at, &other));
        }
        assert!(held(&mut f, &g, at, &other));
        let w = (0..3).find_map(|_| step(&mut f));
        assert_eq!(w, Some([32.0, 0.0, 0.0]));
        assert_eq!(f.path, [0, 1, 2, 3, 4, 5]);
        for _ in 0..2 * BODY_TICKS {
            assert!(!held(&mut f, &g, at, &other));
            step(&mut f);
        }
    }

    /// Off the side of a level edge into a pocket a step and more under it:
    /// the landing drops the plan and avoids the edge, and the next plan
    /// starts at a node a walk reaches.
    #[test]
    fn a_landing_under_a_level_edge_plans_again_round_it() {
        use crate::bots::Goal;
        let g = row();
        let mut f = Follower::default();
        let goal = Goal::To([160.0, 0.0, 0.0]);
        let mut budget = 100;
        let mut step = |f: &mut Follower, at: [f32; 3], body: Footing| {
            f.waypoint(&g, None, goal, at, body, &mut budget, &mut || 0)
        };
        assert!(step(&mut f, [40.0, 0.0, 0.0], Footing::STANDING).is_some());
        let air = Footing {
            on_ground: false,
            ..Footing::STANDING
        };
        assert!(step(&mut f, [70.0, 0.0, -10.0], air).is_some());
        assert_eq!(step(&mut f, [72.0, 0.0, -23.0], Footing::STANDING), None);
        assert!(f.fell && f.avoid.len() == 1);
    }

    /// Off the side of an edge and still falling past the waypoint's reach:
    /// the plan is dropped round the edge, as a landing under it drops it.
    /// Knocked as far off on its floor, the plan is only dropped.
    #[test]
    fn a_fall_off_the_path_past_its_reach_plans_round_the_edge() {
        use crate::bots::Goal;
        let g = row();
        let goal = Goal::To([160.0, 0.0, 0.0]);
        let air = Footing {
            on_ground: false,
            ..Footing::STANDING
        };
        for (z, avoided) in [(-100.0, true), (0.0, false)] {
            let mut f = Follower::default();
            let mut budget = 100;
            let mut step = |f: &mut Follower, at: [f32; 3], body: Footing| {
                f.waypoint(&g, None, goal, at, body, &mut budget, &mut || 0)
            };
            assert!(step(&mut f, [40.0, 0.0, 0.0], Footing::STANDING).is_some());
            assert_eq!(step(&mut f, [70.0, 200.0, z], air), None);
            assert_eq!((f.fell, f.avoid.len() == 1), (avoided, avoided), "z {z}");
        }
    }

    /// A plan round a stuck edge takes no risky drop: with the only plain
    /// way avoided, the plan goes through the stuck edge again, as
    /// mp_ship's crow's nest's ladder after a meeting at its head, not
    /// down the hatch beside it. With nothing avoided, the risky drop is
    /// still the way when it is the only one.
    #[test]
    fn a_plan_round_a_stuck_edge_takes_no_risky_drop() {
        use crate::bots::Goal;
        // 0 -> 1 -> 2 plain, 0 -> 2 a risky drop.
        let nodes = vec![
            Vec3::ZERO,
            Vec3::new(64.0, 64.0, 0.0),
            Vec3::new(128.0, 0.0, 0.0),
        ];
        let mut g = NavGraph::from_parts(nodes, vec![vec![1, 2], vec![2], vec![]]);
        g.edge_mut(0, 2).unwrap().risky = true;
        g.index_components();
        let goal = Goal::To([128.0, 0.0, 0.0]);
        let mut f = Follower::default();
        f.avoid.push((0, 1, u32::MAX));
        let mut budget = 100;
        let at = [0.0, 0.0, 0.0];
        let plan = (0..4).find_map(|_| {
            f.waypoint(
                &g,
                None,
                goal,
                at,
                Footing::STANDING,
                &mut budget,
                &mut || 0,
            )
        });
        assert_eq!(plan, Some([64.0, 64.0, 0.0]), "through the stuck edge");
        let mut s = Search::new(&g, 0, 2, vec![(0, 1)]);
        assert!(matches!(s.run(&g, &mut 100), Plan::Failed));
        let mut s = Search::new(&g, 0, 2, Vec::new());
        assert!(matches!(s.run(&g, &mut 100), Plan::Found(p) if p == [0, 1, 2]));
        g.edges[0] = vec![Edge::to(2)];
        let mut s = Search::new(&g, 0, 2, Vec::new());
        assert!(matches!(s.run(&g, &mut 100), Plan::Found(p) if p == [0, 2]));
    }

    /// Round only the edges that failed twice, the plan may take a risky
    /// drop: with the one plain way out of mp_ship's deck pocket at (3560,
    /// -60, 352) dead, plans with no drop failed and the bots stood there
    /// 10 s a time (bot-navigation.md, "The ship's leftovers").
    #[test]
    fn a_plan_round_a_dead_edge_takes_a_risky_drop() {
        use crate::bots::Goal;
        let nodes = vec![
            Vec3::ZERO,
            Vec3::new(64.0, 64.0, 0.0),
            Vec3::new(128.0, 0.0, 0.0),
        ];
        let mut g = NavGraph::from_parts(nodes, vec![vec![1, 2], vec![2], vec![]]);
        g.edge_mut(0, 2).unwrap().risky = true;
        g.index_components();
        let mut f = Follower::default();
        f.avoid.push((0, 1, u32::MAX));
        f.dead.push((0, 1, u32::MAX));
        let mut budget = 100;
        let plan = (0..4).find_map(|_| {
            f.waypoint(
                &g,
                None,
                Goal::To([128.0, 0.0, 0.0]),
                [0.0, 0.0, 0.0],
                Footing::STANDING,
                &mut budget,
                &mut || 0,
            )
        });
        assert_eq!(plan, Some([128.0, 0.0, 0.0]), "down the drop");
    }

    /// A point goal within [`RETARGET`] of the destination, a floor over
    /// it, is planned for again: a bot kept its hold floor node for a
    /// health item on mp_ship's ledge 116 over it and paced under it.
    #[test]
    fn a_goal_a_floor_off_its_destination_is_planned_for_again() {
        use crate::bots::Goal;
        let nodes = vec![
            Vec3::ZERO,
            Vec3::new(64.0, 0.0, 0.0),
            Vec3::new(64.0, 32.0, 100.0),
        ];
        let mut g = NavGraph::from_parts(nodes, vec![vec![1], vec![0, 2], vec![1]]);
        g.index_components();
        let mut f = Follower::default();
        let wp = |f: &mut Follower, p| {
            f.waypoint(
                &g,
                None,
                Goal::To(p),
                [0.0, 0.0, 0.0],
                Footing::STANDING,
                &mut 100,
                &mut || 0,
            )
        };
        wp(&mut f, [64.0, 0.0, 0.0]);
        assert_eq!(f.dest, Some(1));
        wp(&mut f, [64.0, 32.0, 100.0]);
        assert_eq!(f.dest, Some(2));
    }

    /// A point farther off the graph than `RETARGET`, a noise in the air
    /// over a floor, keeps the plan to the node it maps to: read off the
    /// point it moved every tick and the plan was parked and swapped for
    /// the last one each tick (bot-navigation.md "The ship's leftovers").
    #[test]
    fn a_goal_far_off_the_graph_keeps_its_plan() {
        use crate::bots::Goal;
        let nodes = vec![
            Vec3::ZERO,
            Vec3::new(64.0, 0.0, 0.0),
            Vec3::new(128.0, 0.0, 0.0),
        ];
        let mut g = NavGraph::from_parts(nodes, vec![vec![1], vec![0, 2], vec![1]]);
        g.index_components();
        let mut f = Follower::default();
        let p = [128.0, 0.0, 220.0];
        for _ in 0..3 {
            f.waypoint(
                &g,
                None,
                Goal::To(p),
                [0.0, 0.0, 0.0],
                Footing::STANDING,
                &mut 100,
                &mut || 0,
            );
            assert_eq!(f.dest, Some(2));
            assert!(
                f.parked.is_none(),
                "the plan was parked as if the goal moved"
            );
        }
    }

    /// An edge stuck on again while it is avoided, planned through for want
    /// of a way round, is planned through no more until it expires: the
    /// bot gets no waypoint rather than the same edge again.
    #[test]
    fn an_edge_stuck_on_again_while_avoided_is_not_planned_through() {
        use crate::bots::Goal;
        let nodes = vec![Vec3::ZERO, Vec3::new(64.0, 0.0, 0.0)];
        let mut g = NavGraph::from_parts(nodes, vec![vec![1], vec![]]);
        g.index_components();
        let goal = Goal::To([64.0, 0.0, 0.0]);
        let plan = |f: &mut Follower| {
            let mut budget = 100;
            (0..4).find_map(|_| {
                f.waypoint(
                    &g,
                    None,
                    goal,
                    [0.0; 3],
                    Footing::STANDING,
                    &mut budget,
                    &mut || 0,
                )
            })
        };
        let mut f = Follower::default();
        f.avoid_edge(0, 1);
        assert_eq!(plan(&mut f), Some([64.0, 0.0, 0.0]), "through it once");
        f.reset();
        f.avoid_edge(0, 1);
        assert_eq!(plan(&mut f), None, "not again");
        f.clock += AVOID_TICKS;
        f.rest = 0;
        assert_eq!(plan(&mut f), Some([64.0, 0.0, 0.0]), "once it expires");
    }

    /// A point goal that moves off and comes back within `PARK_TICKS`
    /// takes up its old plan where it was, with no new search; later, it
    /// plans afresh.
    #[test]
    fn a_goal_back_within_a_second_takes_up_its_old_plan() {
        use crate::bots::Goal;
        let g = row();
        let mut f = Follower::default();
        let wp = |f: &mut Follower, goal, mut budget| {
            f.waypoint(
                &g,
                None,
                goal,
                [40.0, 0.0, 0.0],
                Footing::STANDING,
                &mut budget,
                &mut || 0,
            )
        };
        let far = Goal::To([160.0, 0.0, 0.0]);
        let first = wp(&mut f, far, 100);
        assert!(first.is_some());
        assert!(wp(&mut f, Goal::To([0.0, 0.0, 0.0]), 100).is_some());
        assert_eq!(wp(&mut f, far, 0), first, "no budget, the old plan");
        // Flipped away again and left there past the wait: no plan without
        // a search.
        wp(&mut f, Goal::To([0.0, 0.0, 0.0]), 100);
        for _ in 0..=PARK_TICKS {
            wp(&mut f, Goal::To([0.0, 0.0, 0.0]), 100);
        }
        assert_eq!(wp(&mut f, far, 0), None);
    }

    /// With no waypoint while the next plan is searched for, the body waits
    /// on the last one handed out, near it on its floor, for a while.
    #[test]
    fn a_body_waits_on_its_last_waypoint_while_a_plan_is_searched_for() {
        use crate::bots::Goal;
        let g = row();
        let mut f = Follower::default();
        let wp = |f: &mut Follower, goal, mut budget| {
            f.waypoint(
                &g,
                None,
                goal,
                [0.0; 3],
                Footing::STANDING,
                &mut budget,
                &mut || 0,
            )
        };
        assert_eq!(
            wp(&mut f, Goal::To([160.0, 0.0, 0.0]), 100),
            Some([32.0, 0.0, 0.0])
        );
        // A new goal, and no budget left this tick to plan for it.
        assert_eq!(wp(&mut f, Goal::To([0.0, 0.0, 0.0]), 0), None);
        let at = [10.0, 0.0, 0.0];
        assert_eq!(f.rest_point(at, Footing::STANDING), Some([32.0, 0.0, 0.0]));
        let rungs = Footing {
            on_ground: false,
            on_ladder: true,
            ..Footing::STANDING
        };
        assert_eq!(f.rest_point(at, rungs), None, "not on a ladder");
        assert_eq!(
            f.rest_point([10.0, 0.0, 40.0], Footing::STANDING),
            None,
            "a floor up"
        );
        for _ in 0..WAIT_TICKS {
            wp(&mut f, Goal::To([0.0, 0.0, 0.0]), 0);
        }
        assert_eq!(f.rest_point(at, Footing::STANDING), None, "waited out");
    }

    /// The plan round a body in the way is waited for where the body
    /// stands, however far its last waypoint: on mp_ship's spar a bot ran
    /// a random heading meanwhile and fell off it (seed 1381).
    #[test]
    fn a_body_waits_where_it_stands_for_the_plan_round_a_body() {
        let mut f = Follower::default();
        let at = [500.0, 0.0, 0.0];
        assert_eq!(f.rest_point(at, Footing::STANDING), None);
        f.shun = Arc::from([3u32]);
        assert_eq!(f.rest_point(at, Footing::STANDING), Some(at));
        let rungs = Footing {
            on_ground: false,
            on_ladder: true,
            ..Footing::STANDING
        };
        assert_eq!(f.rest_point(at, rungs), None, "not on a ladder");
    }

    #[test]
    fn a_follower_hands_out_the_path_then_the_point() {
        use crate::bots::Goal;
        let g = row();
        let mut f = Follower::default();
        let goal = Goal::To([170.0, 0.0, 40.0]);
        let mut budget = 100;
        let mut rand = || 0;
        let mut at = [0.0, 0.0, 0.0];
        let mut seen = Vec::new();
        let mut planned = None;
        for _ in 0..10 {
            let Some(w) = f.waypoint(
                &g,
                None,
                goal,
                at,
                Footing::STANDING,
                &mut budget,
                &mut rand,
            ) else {
                panic!("no waypoint at {at:?}");
            };
            seen.push(w[0]);
            planned.get_or_insert(budget);
            if w == [170.0, 0.0, 40.0] {
                break;
            }
            at = w;
        }
        assert_eq!(seen, [32.0, 64.0, 96.0, 128.0, 160.0, 170.0]);
        assert_eq!(Some(budget), planned, "one plan for the whole walk");
    }

    /// Along plain edges the waypoint is pulled on to the furthest node a
    /// straight walk reaches, one a tick: a pillar on the line holds it at
    /// the grid's next node, and so does a jump edge.
    #[test]
    fn a_waypoint_is_pulled_along_a_clear_straight_line() {
        use crate::bots::Goal;
        let goal = Goal::To([160.0, 64.0, 0.125]);
        let run = |g: &NavGraph, world: &CollisionWorld, at: [[f32; 2]; 2]| {
            let mut f = Follower::default();
            let mut budget = 100;
            at.map(|[x, y]| {
                f.waypoint(
                    g,
                    Some(world),
                    goal,
                    [x, y, 0.125],
                    Footing::STANDING,
                    &mut budget,
                    &mut || 0,
                )
                .map(|w| [w[0], w[1]])
            })
        };
        let open = vcod_common::collision::test_world(&[]);
        let at = [[0.0, 0.0], [40.0, 10.0]];
        assert_eq!(
            run(&zigzag(), &open, at),
            [Some([64.0, 32.0]), Some([96.0, 32.0])]
        );
        // Clear of node 1's capsule, across the line from node 0 to node 2.
        let pillar = vcod_common::collision::test_world(&[(
            Vec3::new(24.0, 20.0, 0.0),
            Vec3::new(40.0, 28.0, 100.0),
        )]);
        assert_eq!(run(&zigzag(), &pillar, at)[0], Some([32.0, 0.0]));
        let mut g = zigzag();
        g.edge_mut(1, 2).unwrap().jump = true;
        assert_eq!(run(&g, &open, at)[0], Some([32.0, 0.0]));
    }

    /// The island is ruled out by the component table, not by a search of
    /// all 40000 nodes: the plan heads for the grid's corner nearest it at
    /// the cost of the walk there, a handful of expansions.
    #[test]
    fn an_unreachable_goal_fails_without_a_search() {
        let g = grid_and_island(200);
        let island = g.nodes.len() as u32 - 1;
        assert!(!g.reaches(0, island) && !g.reaches(island, 0));
        assert_eq!(g.path(5, island), None);
        let mut f = Follower::default();
        let mut budget = 4000;
        let at = [64.0, 64.0, 0.0];
        let w = f.waypoint(
            &g,
            None,
            crate::bots::Goal::To([-1000.0, -1000.0, 0.0]),
            at,
            Footing::STANDING,
            &mut budget,
            &mut || 0,
        );
        assert!(w.is_some(), "heads for the corner");
        assert!(4000 - budget < 10, "spent {}", 4000 - budget);
        assert_eq!(f.path.last(), Some(&0), "ends at the corner");
        // From the island nothing is reached: no search and no waypoint.
        let mut f = Follower::default();
        let mut budget = 4000;
        let w = f.waypoint(
            &g,
            None,
            crate::bots::Goal::To([0.0; 3]),
            [-1000.0, -1000.0, 0.0],
            Footing::STANDING,
            &mut budget,
            &mut || 0,
        );
        assert_eq!((w, budget), (None, 4000));
    }

    /// A plan longer than one tick's slice resumes where it stopped: no
    /// tick spends more than [`PLAN_SLICE`], and the path still arrives.
    #[test]
    fn a_long_plan_spreads_over_ticks() {
        // A serpentine: rows joined at alternate ends, so A*'s straight-line
        // guess is wrong nearly everywhere.
        let side = 120u32;
        let nodes = (0..side * side)
            .map(|i| Vec3::new((i % side) as f32 * 32.0, (i / side) as f32 * 32.0, 0.0))
            .collect();
        let edges = (0..side * side)
            .map(|i| {
                let (x, y) = (i % side, i / side);
                let mut out = Vec::new();
                if x > 0 {
                    out.push(i - 1);
                }
                if x + 1 < side {
                    out.push(i + 1);
                }
                let open = if y % 2 == 0 { side - 1 } else { 0 };
                if x == open && y + 1 < side {
                    out.push(i + side);
                }
                let below = if y % 2 == 1 { side - 1 } else { 0 };
                if x == below && y > 0 {
                    out.push(i - side);
                }
                out
            })
            .collect();
        let g = NavGraph::from_parts(nodes, edges);
        let goal = crate::bots::Goal::To([0.0, (side - 1) as f32 * 32.0, 0.0]);
        let mut f = Follower::default();
        let mut ticks = 0;
        let w = loop {
            let mut budget = 4000;
            let w = f.waypoint(
                &g,
                None,
                goal,
                [0.0; 3],
                Footing::STANDING,
                &mut budget,
                &mut || 0,
            );
            assert!(
                4000 - budget <= PLAN_SLICE,
                "tick {ticks} spent {}",
                4000 - budget
            );
            ticks += 1;
            if w.is_some() || ticks > 100 {
                break w;
            }
        };
        assert!(ticks > 1, "one tick held the whole plan");
        assert_eq!(w, Some([32.0, 0.0, 0.0]));
        assert_eq!(f.path.len() as u32, side * side);
    }

    #[test]
    fn a_spent_plan_budget_leaves_the_bot_without_a_waypoint() {
        let g = row();
        let mut f = Follower::default();
        let w = f.waypoint(
            &g,
            None,
            crate::bots::Goal::Roam,
            [0.0; 3],
            Footing::STANDING,
            &mut 0,
            &mut || 0,
        );
        assert_eq!(w, None);
    }

    #[test]
    fn a_roam_heads_for_a_far_node() {
        let g = row();
        let mut f = Follower::default();
        let mut picks = [1, 5].into_iter();
        let mut rand = || picks.next().unwrap_or(0);
        let w = f.waypoint(
            &g,
            None,
            crate::bots::Goal::Roam,
            [0.0; 3],
            Footing::STANDING,
            &mut 100,
            &mut rand,
        );
        assert_eq!(w, Some([32.0, 0.0, 0.0]));
        // Nothing is ROAM_MIN away on a 160-unit row: the farthest pick wins.
        assert_eq!(f.dest, Some(5));
    }

    /// Away from a threat at one end of the row, the farthest pick that is
    /// nearer the bot than the threat wins.
    #[test]
    fn a_flight_heads_away_from_the_threat() {
        let g = row();
        let mut f = Follower::default();
        let mut picks = [0, 4, 1, 5, 3].into_iter().cycle();
        let mut rand = || picks.next().unwrap();
        let goal = crate::bots::Goal::Away([0.0, 0.0, 0.0]);
        let w = f.waypoint(
            &g,
            None,
            goal,
            [64.0, 0.0, 0.0],
            Footing::STANDING,
            &mut 100,
            &mut rand,
        );
        assert_eq!(f.dest, Some(5));
        assert_eq!(w, Some([96.0, 0.0, 0.0]));
        // The threat moves past the far end: the flight turns round.
        let goal = crate::bots::Goal::Away([300.0, 0.0, 0.0]);
        f.waypoint(
            &g,
            None,
            goal,
            [64.0, 0.0, 0.0],
            Footing::STANDING,
            &mut 100,
            &mut rand,
        );
        assert_eq!(f.dest, Some(0));
    }

    /// A point goal walked out, then a roam: the roam's legs are no longer
    /// "arrived", so they string-pull and report a straight run.
    #[test]
    fn a_roam_after_a_walked_out_point_runs_its_legs() {
        use crate::bots::Goal;
        let g = row();
        let mut f = Follower::default();
        let mut at = [0.0f32; 3];
        let goal = Goal::To([96.0, 0.0, 0.0]);
        for _ in 0..20 {
            match f.waypoint(&g, None, goal, at, Footing::STANDING, &mut 100, &mut || 0) {
                Some(w) => at = w,
                None => break,
            }
            if f.arrived {
                break;
            }
        }
        assert!(f.arrived, "the point was never walked out");
        let mut picks = [5].into_iter().cycle();
        let mut rand = || picks.next().unwrap();
        // The first roam tick may walk out the point's node; the next picks.
        for _ in 0..3 {
            let roam = Goal::Roam;
            f.waypoint(&g, None, roam, at, Footing::STANDING, &mut 100, &mut rand);
            if f.dest == Some(5) {
                break;
            }
        }
        assert_eq!(f.dest, Some(5));
        assert!(!f.arrived);
        assert!(f.straight_run(&g, Vec3::from(at)) > 0.0);
    }

    /// Away, a roam, then Away from the same spot: the flight picks its
    /// destination again instead of following the roam's.
    #[test]
    fn a_flight_after_a_roam_picks_a_flight_destination() {
        use crate::bots::Goal;
        let g = row();
        let mut f = Follower::default();
        let mut picks = [0, 4, 1, 5, 3].into_iter().cycle();
        let mut rand = || picks.next().unwrap();
        let away = Goal::Away([0.0, 0.0, 0.0]);
        let at = [64.0, 0.0, 0.0];
        f.waypoint(&g, None, away, at, Footing::STANDING, &mut 100, &mut rand);
        assert_eq!(f.dest, Some(5));
        f.waypoint(
            &g,
            None,
            Goal::Roam,
            at,
            Footing::STANDING,
            &mut 100,
            &mut rand,
        );
        // The roam's own pick, toward the threat.
        f.dest = Some(0);
        f.path.clear();
        f.waypoint(&g, None, away, at, Footing::STANDING, &mut 100, &mut rand);
        assert_eq!(f.dest, Some(5));
    }

    /// The off-path clear drops the pulled base and the held waypoint with
    /// the path, so the next plan starts at its own first node.
    #[test]
    fn an_off_path_clear_drops_the_pulled_base() {
        let g = row();
        let mut f = Follower::default();
        let goal = crate::bots::Goal::To([160.0, 0.0, 0.0]);
        f.waypoint(
            &g,
            None,
            goal,
            [0.0; 3],
            Footing::STANDING,
            &mut 100,
            &mut || 0,
        );
        assert!(!f.path.is_empty());
        f.next = 4;
        f.pull_base = Some(3);
        // Far off the waypoint: the path is dropped.
        let off = [32.0, 2000.0, 0.0];
        f.waypoint(&g, None, goal, off, Footing::STANDING, &mut 0, &mut || 0);
        assert!(f.path.is_empty());
        assert_eq!(f.pull_base, None);
        assert_eq!(f.held, None);
    }

    /// A bot pinned on its way from node 1 to node 2 (by a body guarding the
    /// stairs, say) plans round that edge, through node 3, once it gives up.
    #[test]
    fn a_follower_stuck_on_an_edge_plans_round_it() {
        let g = NavGraph::from_parts(
            vec![
                Vec3::new(0.0, 0.0, 0.0),
                Vec3::new(32.0, 0.0, 0.0),
                Vec3::new(64.0, 0.0, 0.0),
                Vec3::new(60.0, 30.0, 0.0),
            ],
            vec![vec![1], vec![0, 2, 3], vec![1, 3], vec![1, 2]],
        );
        let mut f = Follower::default();
        let goal = crate::bots::Goal::To([64.0, 0.0, 0.0]);
        let mut budget = 1000;
        let at = [32.0, 0.0, 0.0];
        let mut last = None;
        for _ in 0..=STUCK_TICKS + 1 {
            last = f.waypoint(
                &g,
                None,
                goal,
                at,
                Footing::STANDING,
                &mut budget,
                &mut || 0,
            );
        }
        assert_eq!(last, None, "still steering at a waypoint it never nears");
        let w = f.waypoint(
            &g,
            None,
            goal,
            at,
            Footing::STANDING,
            &mut budget,
            &mut || 0,
        );
        assert_eq!(
            w,
            Some([60.0, 30.0, 0.0]),
            "planned through the blocked edge"
        );
    }

    /// The bot's own stall circle sends it on a spell: the edge under way
    /// is planned round, and the plan from wherever the spell leaves it
    /// does not take it again.
    #[test]
    fn a_spell_plans_round_the_edge_it_began_on() {
        let g = NavGraph::from_parts(
            vec![
                Vec3::new(0.0, 0.0, 0.0),
                Vec3::new(32.0, 0.0, 0.0),
                Vec3::new(64.0, 0.0, 0.0),
                Vec3::new(60.0, 30.0, 0.0),
            ],
            vec![vec![1], vec![0, 2, 3], vec![1, 3], vec![1, 2]],
        );
        let mut f = Follower::default();
        let goal = crate::bots::Goal::To([64.0, 0.0, 0.0]);
        let mut budget = 1000;
        let mut at = |f: &mut Follower, p: [f32; 3]| {
            f.waypoint(&g, None, goal, p, Footing::STANDING, &mut budget, &mut || 0)
        };
        assert_eq!(at(&mut f, [32.0, 0.0, 0.0]), Some([64.0, 0.0, 0.0]));
        f.stuck();
        assert_eq!(f.current(), (None, None), "the path is dropped");
        assert_eq!(
            at(&mut f, [32.0, 0.0, 0.0]),
            Some([60.0, 30.0, 0.0]),
            "round the edge it was stuck on"
        );
    }

    /// mp_ship's crow's nest: a bot planned from the top at 1254, walked off
    /// it onto the hatch's rim at 1219 while the search ran, and headed
    /// back for the top's node 35 over its feet with no way up from there;
    /// it pinned on the top's face and slid down the hatch. A plan whose
    /// first node is a floor over the feet is planned again from a node a
    /// walk reaches.
    #[test]
    fn a_plan_that_starts_a_floor_up_is_planned_again() {
        let Some(world) = map_world("mp_ship") else {
            return;
        };
        let g = graph_for("mp_ship", &world);
        let w = Some(&world.collision);
        // Down on the deck: a point goal on the rail itself is walked to
        // the rim now (`NavGraph::goal_node`).
        let goal = crate::bots::Goal::To([4929.0, 68.0, 692.125]);
        let mut f = Follower::default();
        let top = [4928.5, 60.75, 1254.125];
        let rim = [4961.2, 38.7, 1219.125];
        let mut budget = 1;
        assert_eq!(
            f.waypoint(&g, w, goal, top, Footing::STANDING, &mut budget, &mut || 0),
            None,
            "the search runs on"
        );
        let at = |f: &mut Follower| {
            let mut budget = 100_000;
            f.waypoint(&g, w, goal, rim, Footing::STANDING, &mut budget, &mut || 0)
        };
        assert_eq!(at(&mut f), None, "the plan from the top is dropped");
        let wp = at(&mut f).expect("a plan from the rim");
        assert!(wp[2] < rim[2] + 1.0, "{wp:?}");
    }

    /// Walking at a point past the path's end can drop the body a floor;
    /// from there the point is planned for again, not walked at blind.
    #[test]
    fn a_follower_that_falls_off_its_point_plans_again() {
        let g = NavGraph::from_parts(
            vec![
                Vec3::new(0.0, 0.0, 100.0),
                Vec3::new(32.0, 0.0, 100.0),
                Vec3::new(64.0, 0.0, 0.0),
                Vec3::new(32.0, 0.0, 0.0),
            ],
            vec![vec![1], vec![0, 2], vec![3, 1], vec![2]],
        );
        let mut f = Follower::default();
        let goal = crate::bots::Goal::To([40.0, 0.0, 120.0]);
        let mut budget = 1000;
        // Standing on the path's last node: walked out, head at the point.
        let at = [32.0, 0.0, 100.0];
        assert_eq!(
            f.waypoint(
                &g,
                None,
                goal,
                at,
                Footing::STANDING,
                &mut budget,
                &mut || 0
            ),
            Some([40.0, 0.0, 120.0])
        );
        assert_eq!(
            f.waypoint(
                &g,
                None,
                goal,
                at,
                Footing::STANDING,
                &mut budget,
                &mut || 0
            ),
            Some([40.0, 0.0, 120.0])
        );
        // Fell to the floor below: a new plan back up, its waypoints nodes.
        let below = [34.0, 0.0, 0.0];
        let before = budget;
        let w = f.waypoint(
            &g,
            None,
            goal,
            below,
            Footing::STANDING,
            &mut budget,
            &mut || 0,
        );
        assert!(budget < before, "planned again");
        assert!(
            w.is_some_and(|w| g.nodes.contains(&Vec3::from(w))),
            "still walking at the point: {w:?}"
        );
    }

    /// Floor, a ledge 16 up, a top 32 up, the floor beyond: from the ledge,
    /// beside the top and in reach of it flat, the bot still heads for the
    /// top, where the drop on was proved.
    #[test]
    fn a_node_a_drop_leaves_from_is_passed_only_on_it() {
        let g = NavGraph::from_parts(
            vec![
                Vec3::new(64.0, 0.0, -64.0),
                Vec3::new(32.0, 0.0, -48.0),
                Vec3::new(0.0, 0.0, -32.0),
                Vec3::new(-48.0, 0.0, -64.0),
            ],
            vec![vec![1], vec![0, 2], vec![1, 3], vec![]],
        );
        let mut f = Follower::default();
        let goal = crate::bots::Goal::To([-48.0, 0.0, -64.0]);
        let mut budget = 1000;
        let mut at = |p: [f32; 3]| {
            f.waypoint(
                &g,
                None,
                goal,
                p,
                Footing {
                    still: false,
                    on_ground: true,
                    on_ladder: false,
                    velocity: Vec3::ZERO,
                },
                &mut budget,
                &mut || 0,
            )
        };
        assert_eq!(at([64.0, 0.0, -64.0]), Some([32.0, 0.0, -48.0]));
        assert_eq!(at([12.0, 0.0, -48.0]), Some([0.0, 0.0, -32.0]));
        assert_eq!(at([2.0, 0.0, -32.0]), Some([-48.0, 0.0, -64.0]));
    }

    /// A body in the air beside a jump's top is nearer the next node than
    /// the top is, and still not on it.
    #[test]
    fn a_jumps_top_is_not_cut_past_in_the_air() {
        let mut g = NavGraph::from_parts(
            vec![
                Vec3::new(0.0, 0.0, 0.0),
                Vec3::new(32.0, 0.0, 36.0),
                Vec3::new(32.0, 64.0, 36.0),
            ],
            vec![vec![1], vec![0, 2], vec![1]],
        );
        g.edge_mut(0, 1).unwrap().jump = true;
        let mut f = Follower::default();
        let goal = crate::bots::Goal::To([32.0, 64.0, 36.0]);
        let mut budget = 100;
        let mut at = |p: [f32; 3]| {
            f.waypoint(
                &g,
                None,
                goal,
                p,
                Footing {
                    still: false,
                    on_ground: true,
                    on_ladder: false,
                    velocity: Vec3::ZERO,
                },
                &mut budget,
                &mut || 0,
            )
        };
        assert_eq!(at([0.0, 0.0, 0.0]), Some([32.0, 0.0, 36.0]));
        // 25 flat from the top, 15 under it, mid-jump.
        assert_eq!(at([20.0, 22.0, 30.0]), Some([32.0, 0.0, 36.0]));
        assert_eq!(at([32.0, 2.0, 36.0]), Some([32.0, 64.0, 36.0]));
    }

    /// mp_ship's ledge at (3347, 250, 616): a bot passed the jump's top at
    /// 652 from 15 under it in the air, within reach flat, headed on for
    /// the floor at 664 and fell back to the deck. The top is passed only
    /// on the ground.
    #[test]
    fn a_jumps_top_is_passed_only_on_the_ground() {
        let mut g = NavGraph::from_parts(
            vec![
                Vec3::new(3361.3, 256.0, 616.125),
                Vec3::new(3366.2, 224.0, 652.125),
                Vec3::new(3328.2, 252.4, 664.125),
            ],
            vec![vec![1], vec![0, 2], vec![1]],
        );
        g.edge_mut(0, 1).unwrap().jump = true;
        let mut f = Follower::default();
        let goal = crate::bots::Goal::To([3328.2, 252.4, 664.125]);
        let mut budget = 100;
        let top = Some([3366.2, 224.0, 652.125]);
        let mut at = |p: [f32; 3], on_ground: bool| {
            let body = Footing {
                still: false,
                on_ground,
                on_ladder: false,
                velocity: Vec3::ZERO,
            };
            f.waypoint(&g, None, goal, p, body, &mut budget, &mut || 0)
        };
        assert_eq!(at([3367.5, 239.1, 616.125], true), top);
        assert_eq!(at([3349.2, 239.1, 637.1], false), top, "in the air");
        assert_eq!(
            at([3366.0, 230.0, 652.125], true),
            Some([3328.2, 252.4, 664.125])
        );
    }

    /// mp_hurtgen, seed 43: a bot slid down the terrain west of the crease
    /// at (437, 2824, -97), in the air from (379, 2807, -34) on, turned for
    /// the node after (381.7, 2790.8) 12 units past it and for the next
    /// 22 past that, never landing on either patch, and slid into the
    /// crease. A node level with the body is passed only on the ground.
    #[test]
    fn a_node_is_not_passed_sliding_past_it_in_the_air() {
        let g = NavGraph::from_parts(
            vec![
                Vec3::new(381.7, 2790.8, -53.6),
                Vec3::new(428.2, 2783.2, -60.3),
                Vec3::new(480.1, 2833.2, -123.2),
            ],
            vec![vec![1], vec![2], vec![]],
        );
        let mut f = Follower::default();
        let goal = crate::bots::Goal::To([480.1, 2833.2, -123.2]);
        let mut budget = 100;
        let mut at = |p: [f32; 3], on_ground: bool| {
            let body = Footing {
                still: false,
                on_ground,
                on_ladder: false,
                velocity: Vec3::ZERO,
            };
            f.waypoint(&g, None, goal, p, body, &mut budget, &mut || 0)
        };
        let slid = [392.1, 2797.3, -55.1];
        assert_eq!(at(slid, false), Some([381.7, 2790.8, -53.6]));
        assert_eq!(at(slid, true), Some([428.2, 2783.2, -60.3]));
    }

    /// mp_ship, seed 693: from (3365, -323, 352) the bot was nearer the
    /// ladder's foot (3370, -380, 56) than the head (3298, -380, 352) it
    /// backs down from, cut past the head and pinned on the wall at
    /// y -333 above the foot. A drop's top is passed only on reaching it.
    #[test]
    fn a_drops_top_is_not_cut_past() {
        let g = NavGraph::from_parts(
            vec![
                Vec3::new(3353.0, -321.0, 352.0),
                Vec3::new(3298.0, -380.0, 352.0),
                Vec3::new(3370.0, -380.0, 56.0),
            ],
            vec![vec![1], vec![2], vec![]],
        );
        let mut f = Follower::default();
        let goal = crate::bots::Goal::To([3370.0, -380.0, 56.0]);
        let mut budget = 100;
        let mut at =
            |p: [f32; 3]| f.waypoint(&g, None, goal, p, Footing::STANDING, &mut budget, &mut || 0);
        assert_eq!(at([3353.0, -321.0, 352.0]), Some([3298.0, -380.0, 352.0]));
        assert_eq!(at([3365.4, -324.2, 352.1]), Some([3298.0, -380.0, 352.0]));
    }

    /// mp_ship's hold ladder: a bot backing down the rungs passed the node
    /// of the floor at 304 beside it from 42 above, headed level for the
    /// next node and hung on the rungs under the floor's lip. On a ladder a
    /// node below is passed only from a step over its floor.
    #[test]
    fn a_node_below_the_rungs_is_passed_only_from_its_floor() {
        let g = NavGraph::from_parts(
            vec![
                Vec3::new(4349.5, -93.1, 600.125),
                Vec3::new(4318.5, -96.0, 304.125),
                Vec3::new(4280.5, -96.1, 304.125),
            ],
            vec![vec![1], vec![2], vec![1]],
        );
        let mut f = Follower::default();
        let goal = crate::bots::Goal::To([4280.5, -96.1, 304.125]);
        let mut budget = 100;
        let floor = Some([4318.5, -96.0, 304.125]);
        let mut at = |p: [f32; 3]| {
            let body = Footing {
                still: false,
                on_ground: false,
                on_ladder: true,
                velocity: Vec3::ZERO,
            };
            f.waypoint(&g, None, goal, p, body, &mut budget, &mut || 0)
        };
        assert_eq!(at([4349.5, -93.1, 600.125]), floor);
        assert_eq!(at([4328.5, -93.1, 346.3]), floor, "42 above");
        assert_eq!(at([4322.0, -93.1, 315.0]), Some([4280.5, -96.1, 304.125]));
    }

    /// mp_ship's hull beam: from its edge node a walk over the beam to the
    /// floor beyond arrives, from 16 units north along the edge it slides
    /// back off the slope. A bot that passed the node there stalled 120
    /// ticks.
    #[test]
    fn a_walk_on_proved_from_the_node_is_proved_again_from_the_body() {
        let Some(world) = map_world("mp_ship") else {
            return;
        };
        let w = &world.collision;
        let edge = Vec3::new(2247.1, 511.2, -47.875);
        let beyond = Vec3::new(2203.5, 511.4, -63.875);
        let g = NavGraph::from_parts(vec![edge, beyond], vec![vec![1], vec![0]]);
        let mut f = Follower::default();
        let goal = crate::bots::Goal::To(beyond.into());
        let mut budget = 100;
        // Running at the floor beyond, as a follower on the path does.
        let mut at = |p: Vec3| {
            let run = (beyond - p).truncate().normalize() * vcod_common::pmove::SPEED_RUN;
            f.waypoint(
                &g,
                Some(w),
                goal,
                p.into(),
                Footing {
                    still: false,
                    on_ground: true,
                    on_ladder: false,
                    velocity: run.extend(0.0),
                },
                &mut budget,
                &mut || 0,
            )
        };
        // In reach of the edge node along the ledge, but the walk on slides
        // back from here: close in on the node.
        let along = Vec3::new(2256.2498, 523.75616, -47.87189);
        assert!(!matches!(
            walk_as(w, along, beyond.truncate(), Some(beyond.z), Gait::Forward),
            Walked::Arrived(..)
        ));
        assert_eq!(at(along), Some(edge.into()));
        assert_eq!(at(edge), Some(beyond.into()));
    }

    #[test]
    fn a_follower_climbs_to_a_head_above_its_foot() {
        let g = NavGraph::from_parts(
            vec![
                Vec3::new(0.0, 0.0, 0.0),
                Vec3::new(10.0, 0.0, 200.0),
                Vec3::new(40.0, 0.0, 200.0),
            ],
            vec![vec![1], vec![0, 2], vec![1]],
        );
        let mut f = Follower::default();
        let goal = crate::bots::Goal::To([40.0, 0.0, 200.0]);
        let mut budget = 100;
        let mut at = [0.0, 0.0, 0.0];
        // 2.5 units a tick, the climb rate looking up, until the head's
        // floor is a step off: from further down the rungs the next node
        // is no way on.
        while at[2] <= 182.0 {
            let w = f.waypoint(
                &g,
                None,
                goal,
                at,
                Footing::STANDING,
                &mut budget,
                &mut || 0,
            );
            assert_eq!(w, Some([10.0, 0.0, 200.0]), "at z {}", at[2]);
            at[2] += 2.5;
        }
        at[0] = 10.0;
        at[2] = 200.0;
        // Level with it but still on the rungs: turning for the next node
        // there lets go of them (mp_ship's mast).
        let climbing = Footing {
            still: false,
            on_ground: false,
            on_ladder: true,
            velocity: Vec3::ZERO,
        };
        let w = f.waypoint(&g, None, goal, at, climbing, &mut budget, &mut || 0);
        assert_eq!(w, Some([10.0, 0.0, 200.0]), "on the rungs");
        let w = f.waypoint(
            &g,
            None,
            goal,
            at,
            Footing::STANDING,
            &mut budget,
            &mut || 0,
        );
        assert_eq!(w, Some([40.0, 0.0, 200.0]), "on the head's floor");
    }

    /// A climb from a foot square in front of a ladder runs into the face
    /// until it is level with the head; from a foot off to the side it runs
    /// at the head, the way the walk that linked it went.
    #[test]
    fn a_climb_runs_into_the_face_from_a_foot_in_front_of_it() {
        let mut g = NavGraph::from_parts(
            vec![
                Vec3::new(0.0, 0.0, 0.0),
                Vec3::new(-40.0, 10.0, 200.0),
                Vec3::new(0.0, 40.0, 0.0),
            ],
            vec![vec![1], vec![0], vec![1]],
        );
        // A ladder plate 1 thick at x -17, 24 wide along y.
        g.ladder_boxes = vec![(Vec3::new(-18.0, -12.0, -8.0), Vec3::new(-17.0, 12.0, 200.0))];
        let goal = crate::bots::Goal::To([-40.0, 10.0, 200.0]);
        let mut f = Follower::default();
        let mut budget = 100;
        let w = |f: &mut Follower, budget: &mut u32, at: [f32; 3]| {
            f.waypoint(&g, None, goal, at, Footing::STANDING, budget, &mut || 0)
        };
        assert_eq!(w(&mut f, &mut budget, [0.0; 3]), Some([-17.0, 0.0, 200.0]));
        assert_eq!(
            w(&mut f, &mut budget, [-2.0, 0.0, 190.0]),
            Some([-40.0, 10.0, 200.0]),
            "level with the head"
        );
        let mut f = Follower::default();
        let at = [0.0, 40.0, 0.0];
        assert_eq!(w(&mut f, &mut budget, at), Some([-40.0, 10.0, 200.0]));
        assert_eq!(
            g.ladder_middle(Vec3::new(-2.0, 9.0, 100.0)),
            Some(glam::Vec2::new(-2.0, 0.0))
        );
    }

    /// A leap's foot is held until the bot stands still on it, then the
    /// top is the waypoint, flagged a leap.
    #[test]
    fn a_follower_comes_to_rest_at_a_leaps_foot() {
        let mut g = NavGraph::from_parts(
            vec![
                Vec3::new(0.0, 0.0, 0.0),
                Vec3::new(32.0, 0.0, 0.0),
                Vec3::new(96.0, 0.0, 32.0),
            ],
            vec![vec![1], vec![0, 2], vec![1]],
        );
        g.edge_mut(1, 2).unwrap().jump = true;
        g.edge_mut(1, 2).unwrap().leap = true;
        let mut f = Follower::default();
        let goal = crate::bots::Goal::To([96.0, 0.0, 32.0]);
        let mut budget = 100;
        let foot = Some([32.0, 0.0, 0.0]);
        let at = [30.0, 0.0, 0.0];
        assert_eq!(
            f.waypoint(
                &g,
                None,
                goal,
                at,
                Footing {
                    still: false,
                    on_ground: true,
                    on_ladder: false,
                    velocity: Vec3::ZERO,
                },
                &mut budget,
                &mut || 0
            ),
            foot
        );
        assert!(f.holding(&g) && !f.jumping(&g));
        // Running past the foot, nearer the top than the foot is: held.
        let at = [40.0, 0.0, 0.0];
        assert_eq!(
            f.waypoint(
                &g,
                None,
                goal,
                at,
                Footing {
                    still: false,
                    on_ground: true,
                    on_ladder: false,
                    velocity: Vec3::ZERO,
                },
                &mut budget,
                &mut || 0
            ),
            foot
        );
        let at = [33.0, 0.0, 0.0];
        assert_eq!(
            f.waypoint(
                &g,
                None,
                goal,
                at,
                Footing::STANDING,
                &mut budget,
                &mut || 0
            ),
            Some([96.0, 0.0, 32.0])
        );
        assert!(f.jumping(&g) && f.leaping(&g) && !f.holding(&g));
    }

    /// A point goal on a perch is walked to the nearest node off the
    /// perches within [`RETARGET`], and the walked-out path heads at that
    /// node, not at the point: a chase onto mp_ship's crow's nest rails
    /// ended at a run on the rail as a roam once did.
    #[test]
    fn a_point_goal_on_a_perch_ends_beside_it() {
        let nodes = (0..3)
            .map(|i| Vec3::new(i as f32 * 32.0, 0.0, 0.0))
            .collect();
        let mut g = NavGraph::from_parts(nodes, vec![vec![1], vec![0, 2], vec![1]]);
        g.index_components();
        assert_eq!(g.goal_node([64.0, 0.0, 0.0]), Some(2));
        g.perches.insert(2);
        assert_eq!(g.goal_node([64.0, 0.0, 0.0]), Some(1));
        let f = Follower {
            dest: g.goal_node([64.0, 0.0, 0.0]),
            ..Follower::default()
        };
        assert_eq!(f.arrival(&g, [64.0, 0.0, 0.0]), [32.0, 0.0, 0.0]);
        assert_eq!(f.arrival(&g, [0.0, 0.0, 0.0]), [0.0, 0.0, 0.0]);
        let Some(world) = map_world("mp_ship") else {
            return;
        };
        let g = graph_for("mp_ship", &world);
        for rail in [[5023.8, 123.7, 1254.125], [4928.5, 60.8, 1254.125]] {
            let n = g.goal_node(rail).unwrap();
            assert!(!g.perches.contains(&n), "{rail:?}");
            assert!(g.nodes[n as usize].distance(Vec3::from(rail)) <= RETARGET);
        }
    }

    /// mp_ship, seed 245: the drop from the crow's nest's top at
    /// (4928.5, 60.8, 1254) to the spar at 1212 had been avoided, and with
    /// no way round it the plan took it again. Passed as it was at the
    /// pace of a turn onto the node, the run flew past the spar to the
    /// deck at 400. It is held: the body comes to rest on the node first.
    #[test]
    fn an_edge_with_no_way_round_is_taken_from_rest() {
        let Some(world) = map_world("mp_ship") else {
            return;
        };
        let top = Vec3::new(4928.5, 60.8, 1254.125);
        let spar = Vec3::new(4888.3, 64.6, 1211.65);
        let g = NavGraph::from_parts(vec![top, spar], vec![vec![1], vec![]]);
        let mut f = Follower {
            path: vec![0, 1],
            avoid: vec![(0, 1, u32::MAX)],
            ..Follower::default()
        };
        let mut body = Footing {
            still: false,
            on_ground: true,
            on_ladder: false,
            velocity: Vec3::new(-118.0, 120.0, 0.0),
        };
        let at = [4940.6, 48.3, 1254.125];
        let mut budget = 100;
        let mut rand = || 0;
        let mut wp = |f: &mut Follower, at: [f32; 3], body: Footing| {
            f.waypoint(
                &g,
                Some(&world.collision),
                crate::bots::Goal::Roam,
                at,
                body,
                &mut budget,
                &mut rand,
            )
        };
        assert_eq!(
            wp(&mut f, at, body),
            Some(top.into()),
            "held at the turn's pace"
        );
        assert!(f.holding(&g));
        body.velocity = Vec3::ZERO;
        body.still = true;
        assert_eq!(
            wp(&mut f, (top + Vec3::new(3.0, -2.0, 0.0)).into(), body),
            Some(spar.into()),
            "from rest"
        );
    }

    /// mp_depot, seed 9: a bot jumped onto the crate at (-2275, 131, 46)
    /// and ran on for the open crate's rim at 66, 20 over it, with its
    /// jump still cooling down. The jump under the rim was refused and the
    /// run went off the crate's edge into the open crate at 23, where the
    /// rim is out of a jump's reach. The bot waits on the foot until the
    /// jump is ready, as the graph's walk had it.
    #[test]
    fn a_jump_edge_waits_on_its_foot_for_the_jump() {
        let Some(world) = map_world("mp_depot") else {
            return;
        };
        let w = &world.collision;
        let mw = MoveWorld::bare(w);
        let foot = Vec3::new(-2275.2, 131.3, 46.125);
        let top = Vec3::new(-2277.2, 95.5, 66.125);
        // The run from the foot with the jump key on the cue under the top.
        let run = |since_jump_ms: f32| {
            let mut ps = PlayerState::spawn(foot, 0.0);
            ps.since_jump_ms = since_jump_ms;
            ps.velocity = ((top - foot).truncate().normalize() * 190.0).extend(0.0);
            for tick in 1..30 {
                let to = top.truncate() - ps.origin.truncate();
                let cue = jump_under(to.length(), top.z - ps.origin.z, 0.0);
                let cmd = UserCmd {
                    server_time: tick * FRAME_MS,
                    forward: 127,
                    up: jump_key(ps.on_ground, cue, false),
                    angles: [0, (to.y.atan2(to.x).to_degrees() * ANGLE2SHORT) as i32, 0],
                    ..NULL_USERCMD
                };
                let dt = FRAME_MS as f32 / 1000.0;
                player_step(
                    &mut ps,
                    &mut [0; 3],
                    &mut EventRing::default(),
                    &cmd,
                    dt,
                    &mw,
                    &[],
                );
                if ps.on_ground && (ps.origin.z - top.z).abs() < 1.0 {
                    return true;
                }
            }
            false
        };
        assert!(run(f32::INFINITY), "a jump to hand lands on the rim");
        assert!(!run(150.0), "a jump refused runs off the crate");
        let mut g = NavGraph::from_parts(vec![foot, top], vec![vec![1], vec![]]);
        g.edge_mut(0, 1).unwrap().jump = true;
        let f = Follower {
            path: vec![0, 1],
            next: 1,
            ..Follower::default()
        };
        let mut ps = PlayerState::spawn(foot + Vec3::new(0.0, -6.0, 0.0), 0.0);
        ps.since_jump_ms = 150.0;
        ps.on_ground = true;
        let at = ps.origin.into();
        assert_eq!(f.jump_wait(&g, at, &ps), Some(foot.into()));
        ps.since_jump_ms = vcod_common::pmove::JUMP_COOLDOWN_MS;
        assert_eq!(f.jump_wait(&g, at, &ps), None, "ready");
    }

    /// Under 1.5's and UO's rules a jump inside the penalty after a landing
    /// rises as little as 15.6 units: the bot waits on the foot for it to
    /// run out, as for the cooldown.
    #[test]
    fn a_jump_edge_waits_out_the_jump_penalty() {
        let (foot, top) = (Vec3::ZERO, Vec3::new(32.0, 0.0, 30.0));
        let mut g = NavGraph::from_parts(vec![foot, top], vec![vec![1], vec![]]);
        g.edge_mut(0, 1).unwrap().jump = true;
        let f = Follower {
            path: vec![0, 1],
            next: 1,
            ..Follower::default()
        };
        let mut ps = PlayerState::spawn(foot + Vec3::new(-4.0, 0.0, 0.0), 0.0);
        ps.rules = MoveRules::Uo;
        ps.on_ground = true;
        ps.jump_penalty = true;
        let at = ps.origin.into();
        assert_eq!(f.jump_wait(&g, at, &ps), Some(foot.into()));
        assert!(!ready_to_leap(&ps));
        ps.jump_penalty = false;
        assert_eq!(f.jump_wait(&g, at, &ps), None, "ready");
        assert!(ready_to_leap(&ps));
    }

    /// A body that stepped up off a jump edge's foot onto a ledge under the
    /// top waits out the penalty there, where the walk jumped from: sent
    /// back down off mp_cassino's ledge at (3667, -1240, 128) it jumped
    /// from the foot 53 under the top and came back for good (seed 17).
    #[test]
    fn a_jump_edge_is_waited_on_from_a_ledge_above_its_foot() {
        let (foot, top) = (Vec3::ZERO, Vec3::new(48.0, 0.0, 53.0));
        let mut g = NavGraph::from_parts(vec![foot, top], vec![vec![1], vec![]]);
        g.edge_mut(0, 1).unwrap().jump = true;
        let f = Follower {
            path: vec![0, 1],
            next: 1,
            ..Follower::default()
        };
        let mut ps = PlayerState::spawn(Vec3::new(18.0, 0.0, 16.7), 0.0);
        ps.rules = MoveRules::Uo;
        ps.on_ground = true;
        ps.jump_penalty = true;
        let at: [f32; 3] = ps.origin.into();
        assert_eq!(f.jump_wait(&g, at, &ps), Some(at));
        let at = [-4.0, 0.0, 2.0];
        assert_eq!(f.jump_wait(&g, at, &ps), Some(foot.into()));
        // On the top's own floor, beside it: back to the foot, as before
        // (mp_cassino's ctf at (5065, -1274, 256), a top 16 over its foot).
        let mut low = NavGraph::from_parts(
            vec![foot, Vec3::new(48.0, 0.0, 16.0)],
            vec![vec![1], vec![]],
        );
        low.edge_mut(0, 1).unwrap().jump = true;
        assert_eq!(
            f.jump_wait(&low, [-20.0, 0.0, 16.0], &ps),
            Some(foot.into())
        );
    }

    /// The sprint's room runs along a straight, level line of plain edges
    /// and stops at a turn, a rise past a stair's and a jump edge.
    #[test]
    fn the_straight_run_stops_at_a_turn_a_rise_and_a_jump() {
        let row = |pts: &[(f32, f32, f32)]| {
            let nodes: Vec<Vec3> = pts.iter().map(|&(x, y, z)| Vec3::new(x, y, z)).collect();
            let n = nodes.len() as u32;
            let edges = (0..n)
                .map(|i| if i + 1 < n { vec![i + 1] } else { vec![] })
                .collect();
            NavGraph::from_parts(nodes, edges)
        };
        let follow = |n: u32| Follower {
            path: (0..n).collect(),
            next: 1,
            ..Follower::default()
        };
        // 0..4 straight along x, then a right angle.
        let g = row(&[
            (0.0, 0.0, 0.0),
            (48.0, 0.0, 0.0),
            (96.0, 0.0, 8.0),
            (144.0, 0.0, 8.0),
            (192.0, 0.0, 8.0),
            (192.0, 48.0, 8.0),
            (192.0, 96.0, 8.0),
        ]);
        let run = follow(7).straight_run(&g, Vec3::ZERO);
        assert!((run - 192.0).abs() < 1.0, "to the corner: {run}");
        // A zigzag within the slack counts as straight.
        let g = row(&[
            (0.0, 0.0, 0.0),
            (48.0, 8.0, 0.0),
            (96.0, 0.0, 0.0),
            (144.0, 8.0, 0.0),
        ]);
        assert!(follow(4).straight_run(&g, Vec3::ZERO) > 140.0);
        // A rise past a stair's: the run ends under it.
        let g = row(&[
            (0.0, 0.0, 0.0),
            (48.0, 0.0, 0.0),
            (96.0, 0.0, 30.0),
            (144.0, 0.0, 30.0),
        ]);
        let run = follow(4).straight_run(&g, Vec3::ZERO);
        assert!((run - 48.0).abs() < 1.0, "to the rise: {run}");
        // A jump edge, level or not, ends it too.
        let mut g = row(&[
            (0.0, 0.0, 0.0),
            (48.0, 0.0, 0.0),
            (96.0, 0.0, 0.0),
            (144.0, 0.0, 0.0),
        ]);
        g.edge_mut(2, 3).unwrap().jump = true;
        let run = follow(4).straight_run(&g, Vec3::ZERO);
        assert!((run - 96.0).abs() < 1.0, "to the jump: {run}");
    }

    /// mp_ship's ledge at (3685.5, -354.3, 400.1), 211 under the deck
    /// node (3647, -352.9, 611.1) west of it and narrow over the hold at
    /// 92: a run off the node misses it, a back down lands. A bot that came
    /// along the deck from the north and turned onto it carried its pace
    /// south over the ledge and died in the hold; the node is held and
    /// passed from at rest on it.
    #[test]
    fn a_backed_drop_is_proved_again_at_the_pace_the_bot_comes_onto_it() {
        let Some(world) = map_world("mp_ship") else {
            return;
        };
        let w = &world.collision;
        let deck = Vec3::new(3647.0, -352.9, 611.125);
        let ledge = Vec3::new(3685.5, -354.3, 400.125);
        let to = ledge.truncate();
        assert!(matches!(
            walk_as(w, deck, to, Some(ledge.z), Gait::Forward),
            Walked::Fell
        ));
        let g = NavGraph::from_parts(vec![deck, ledge], vec![vec![1], vec![]]);
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
        let mut f = Follower::default();
        assert_eq!(
            at(&mut f, deck, Vec3::new(0.0, -190.0, 0.0)),
            (Some(deck.into()), true),
            "coming along the deck from the north"
        );
        assert_eq!(
            at(&mut f, deck, Vec3::ZERO),
            (Some(ledge.into()), false),
            "from at rest on the node"
        );
    }

    /// mp_ship's mast: the head (3675, -95.1, 1147.1) stands on the spar
    /// beside the rungs' top, and only a creep from rest gets back onto
    /// them. A bot that comes onto the head at a run is held there until it
    /// is slow.
    #[test]
    fn a_creep_down_proved_only_from_rest_is_passed_only_slow() {
        let Some(world) = map_world("mp_ship") else {
            return;
        };
        let w = &world.collision;
        let head = Vec3::new(3675.0, -95.13204, 1147.125);
        let foot = Vec3::new(3675.0, -103.0, 896.125);
        let g = NavGraph::from_parts(vec![head, foot], vec![vec![1], vec![]]);
        let goal = crate::bots::Goal::To(foot.into());
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
        let mut f = Follower::default();
        let along = head + Vec3::new(-10.0, 0.0, 0.0);
        assert_eq!(
            at(&mut f, along, Vec3::new(190.0, 0.0, 0.0)),
            (Some(head.into()), true),
            "along the spar at a run"
        );
        assert_eq!(
            at(&mut f, head, Vec3::ZERO),
            (Some(foot.into()), false),
            "from at rest on the head"
        );
    }

    /// mp_depot's roof peak (-2660, -1216, 552): a run east over it flies
    /// off the crest and lands 67 units on at 190 units/s, on the narrow
    /// flat at 508 inside the roof's edge at x -2570. Seed 45's bot held a
    /// weapon at 1.16, came over at 220 and flew past the edge to the yard.
    /// The walk at its weapon's scale falls: the peak is held, and from at
    /// rest on it, where the same walk falls again, the edge is rounded.
    #[test]
    fn a_run_over_a_crest_too_fast_for_the_weapon_is_held_at_the_top() {
        let Some(world) = map_world("mp_depot") else {
            return;
        };
        let w = &world.collision;
        let peak = Vec3::new(-2660.245, -1215.9744, 552.13025);
        let east = Vec3::new(-2626.9204, -1215.9722, 531.5182);
        let g = NavGraph::from_parts(vec![peak, east], vec![vec![1], vec![]]);
        let goal = crate::bots::Goal::To(east.into());
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
        let scaled = |move_scale: f32| Follower {
            move_scale,
            ..Follower::default()
        };
        let seed_45 = Vec3::new(-2663.9, -1216.1, 552.125);
        // Where seed 45's bot, held on the peak, came to rest: 6 short.
        let rest = Vec3::new(-2666.2, -1215.7, 552.125);
        assert_eq!(
            at(&mut scaled(1.0), seed_45, Vec3::new(190.0, 0.0, 0.0)),
            (Some(east.into()), false),
            "at the walk's own pace"
        );
        let mut f = scaled(1.16);
        assert_eq!(
            at(&mut f, seed_45, Vec3::new(220.0, 0.0, 0.0)),
            (Some(peak.into()), true),
            "with a light weapon"
        );
        assert_eq!(
            at(&mut f, rest, Vec3::ZERO),
            (None, false),
            "flung from at rest on the peak too: round it"
        );
        assert!(f.avoid.iter().any(|e| (e.0, e.1) == (0, 1)));
        assert_eq!(
            at(&mut scaled(1.0), rest, Vec3::ZERO),
            (Some(east.into()), false),
            "at the walk's own scale from rest"
        );
    }

    /// mp_depot's step at (-1731, 32, 11): the jump to it from (-1760, 71,
    /// -24) lands only aimed 8 units aside of the node, and the re-proof
    /// keeps that aim, which a bot on the edge steers at.
    #[test]
    fn a_jump_edge_that_lands_only_aimed_aside_keeps_its_aim() {
        let Some(fs) = vcod_common::testing::game_fs_11() else {
            return;
        };
        let bsp = vcod_common::bsp::parse(&fs.read(&fs.resolve_map("mp_depot").unwrap()).unwrap())
            .unwrap();
        let world = crate::world::World::from_bsp(&bsp, Some(&fs));
        let w = &world.collision;
        let foot = settle(w, Vec3::new(-1760.0, 71.0, 0.0), 0.0).unwrap();
        let top = settle(w, Vec3::new(-1731.0, 32.0, 30.0), 0.0).unwrap();
        let mut g = NavGraph::from_parts(vec![foot, top], vec![vec![1], vec![]]);
        g.edge_mut(0, 1).unwrap().jump = true;
        g.reprove_jumps(w);
        let aim = g.edge(0, 1).and_then(|e| e.aim).expect("aimed aside");
        assert!((aim.distance(top.truncate()) - SIDESTEP).abs() < 0.01);
        assert!(g.next(0).eq([1]) && g.has(0, 1, |k| k.jump));
        let f = Follower {
            path: vec![0, 1],
            next: 1,
            ..Follower::default()
        };
        assert_eq!(f.jump_aim(&g), Some(aim));
    }

    /// mp_ship's stair stringer by the hull crates: the jump edge from
    /// (5829, 544, 92) to (5792, 576, 76) was proved with a jump from its
    /// node, and a bot passed the node on the stringer's top at (5823,
    /// 567, 92), where the walk on slides off without one. Pinned on the
    /// stringer, the bot jumped on its cue and fell back for 140 ticks
    /// (seed 2553 on 1080e30d). From there it is not cued to jump; run
    /// from the node, as the graph's walk proved it, it still is.
    #[test]
    fn a_jump_edge_walked_on_without_its_jump_cues_no_jump() {
        let Some(world) = map_world("mp_ship") else {
            return;
        };
        let w = &world.collision;
        let a = Vec3::new(5829.127, 544.00085, 92.125);
        let b = Vec3::new(5792.3965, 576.17664, 76.125);
        let mut g = NavGraph::from_parts(vec![a, b], vec![vec![1], vec![]]);
        g.edge_mut(0, 1).unwrap().jump = true;
        let mut f = Follower {
            path: vec![0, 1],
            ..Follower::default()
        };
        let at = Vec3::new(5823.3916, 567.12866, 92.125);
        assert!(f.edge_holds(Some(w), at, Vec3::new(22.0, 0.0, 0.0), false, &g));
        f.next = 1;
        assert!(f.jumping(&g) && !f.jump_cued(&g), "from the stringer");
        f.next = 0;
        let run = (b - a).truncate().normalize().extend(0.0) * vcod_common::pmove::SPEED_RUN;
        assert!(f.edge_holds(Some(w), a, run, false, &g));
        f.next = 1;
        assert!(f.jump_cued(&g), "from the node at a run");
    }

    /// mp_cassino's terrace at (383, -916, 76), whose one way down to the
    /// street at 11 is a stair: seed 23 left a Panzer IV parked at
    /// (450, -1099, 10.5) facing -79.6 with its collmap's rear over the
    /// stair, and a bot on the stair wedged between the stair's side and
    /// the hull at (425.7, -957.5, 60.1) (bot-navigation.md, "Parked
    /// vehicles"). The stair's nodes read as blocked, and the terrace is
    /// hemmed in.
    #[test]
    fn a_tank_parked_over_cassinos_stair_blocks_its_nodes() {
        use crate::bots::Goal;
        let Some(fs) = vcod_common::testing::uo_fs() else {
            return;
        };
        let entry = fs.resolve_map("mp_cassino").unwrap();
        let bsp = vcod_common::bsp::parse(&fs.read(&entry).unwrap()).unwrap();
        let world = crate::world::World::from_bsp(&bsp, Some(&fs));
        let c = &world.collision;
        let g = with_rules(MoveRules::Uo, || graph_for("mp_cassino", &world));
        let tank = Vec3::new(450.0, -1099.0, 10.5);
        assert!(g.blocked_near(c, tank, 192.0).is_empty());
        let contents = crate::game::vehicle::CONTENTS_COLLMAP as u32;
        c.set_model_instance(181, 28, Some((tank, Vec3::new(0.0, -79.6, 0.0), contents)));
        let mut blocked = g.blocked_near(c, tank, 192.0);
        c.set_model_instance(181, 28, None);
        blocked.sort_unstable();
        let blocked: Arc<[u32]> = blocked.into();
        let node = |p: [f32; 3]| g.nearest(p).unwrap();
        let (top, street) = ([383.0, -916.0, 76.0], [576.0, -1052.0, 17.0]);
        assert!(
            blocked.contains(&node([431.0, -964.0, 57.0])),
            "{blocked:?}"
        );
        assert!(!blocked.contains(&node(top)));
        let mut search =
            Search::new(&g, node(top), node(street), Vec::new()).blocking(blocked.clone());
        assert!(matches!(search.run(&g, &mut 100_000), Plan::Failed));
        let mut f = Follower {
            blocked,
            ..Follower::default()
        };
        let mut budget = 100_000;
        let w = (0..3).find_map(|_| {
            f.waypoint(
                &g,
                None,
                Goal::To(street),
                top,
                Footing::STANDING,
                &mut budget,
                &mut || 0,
            )
        });
        assert!(w.is_none() && f.penned());
    }

    /// A bot that ran off the side of mp_ship's shelf at 396 landed on a
    /// lip at 344 and pushed at the shelf node 52 over it until the stuck
    /// count ran out. On the ground more than a climb under its waypoint,
    /// off a ladder, it plans again, round the edge it fell off.
    #[test]
    fn a_follower_that_fell_off_its_path_plans_again() {
        let g = NavGraph::from_parts(
            vec![
                Vec3::new(5343.4, -155.3, 448.125),
                Vec3::new(5383.6, -161.1, 396.125),
                Vec3::new(5402.6, -160.2, 396.125),
            ],
            vec![vec![1], vec![2], vec![1]],
        );
        let mut f = Follower::default();
        let goal = crate::bots::Goal::To([5402.6, -160.2, 396.125]);
        let mut budget = 100;
        let mut at =
            |p: [f32; 3]| f.waypoint(&g, None, goal, p, Footing::STANDING, &mut budget, &mut || 0);
        assert_eq!(
            at([5343.4, -155.3, 448.125]),
            Some([5383.6, -161.1, 396.125])
        );
        assert_eq!(at([5453.9, -181.9, 344.125]), None);
        assert!(
            f.avoid.iter().any(|e| (e.0, e.1) == (0, 1)),
            "the drop it fell off"
        );
    }

    /// mp_ship's slot at (5403, -160, 396): a run off its drop for the
    /// floor at 204 landed on the ledge at 336, 8 units from straight over
    /// the waypoint, and turned there until an unstick spell threw it off
    /// (seeds 73, 149, 183, 317, 1217, 1913, 3151, 4177, 4333). It plans
    /// again from the ledge.
    #[test]
    fn a_drop_that_lands_partway_down_is_planned_again() {
        let g = NavGraph::from_parts(
            vec![
                Vec3::new(5384.0, -161.0, 396.125),
                Vec3::new(5403.0, -160.0, 396.125),
                Vec3::new(5405.0, -192.0, 204.125),
                Vec3::new(5376.0, -196.0, 204.125),
            ],
            vec![vec![1], vec![2], vec![3], vec![2]],
        );
        let mut f = Follower::default();
        let goal = crate::bots::Goal::To([5376.0, -196.0, 204.125]);
        let mut budget = 100;
        let mut at =
            |p: [f32; 3]| f.waypoint(&g, None, goal, p, Footing::STANDING, &mut budget, &mut || 0);
        assert_eq!(
            at([5384.0, -161.0, 396.125]),
            Some([5405.0, -192.0, 204.125])
        );
        // On the top still, the drop is under way.
        assert_eq!(
            at([5398.0, -184.0, 396.125]),
            Some([5405.0, -192.0, 204.125])
        );
        assert_eq!(at([5404.4, -200.4, 336.125]), None);
        assert!(f.fell, "planned from where it landed");
        // Standing on the ledge with no fall read, the edge is kept: the
        // body may have walked there.
        assert!(f.avoid.is_empty());
    }

    /// Landed partway down mp_ship's slot drop, on the ledge at 336, the
    /// bot plans round the drop and not through it again for a while, even
    /// when its goal moved in the air and the plan under way on landing
    /// was another: from the ledge the plan went back up the shelf ladder
    /// to the same slot (bot-objectives.md 14.9).
    #[test]
    fn a_drop_landed_partway_down_is_not_taken_again() {
        let g = NavGraph::from_parts(
            vec![
                Vec3::new(5384.0, -161.0, 396.125),
                Vec3::new(5403.0, -160.0, 396.125),
                Vec3::new(5405.0, -192.0, 204.125),
                Vec3::new(5376.0, -196.0, 204.125),
                Vec3::new(5000.0, -196.0, 204.125),
            ],
            vec![vec![1], vec![2], vec![3, 4], vec![2, 4], vec![3]],
        );
        let air = Footing {
            on_ground: false,
            still: false,
            ..Footing::STANDING
        };
        for moved in [false, true] {
            let mut f = Follower::default();
            let goal = crate::bots::Goal::To([5376.0, -196.0, 204.125]);
            let mut budget = 100;
            let mut at = |p: [f32; 3], goal, body| {
                f.waypoint(&g, None, goal, p, body, &mut budget, &mut || 0)
            };
            let st = Footing::STANDING;
            assert_eq!(
                at([5384.0, -161.0, 396.125], goal, st),
                Some([5405.0, -192.0, 204.125])
            );
            assert_eq!(
                at([5398.0, -184.0, 396.125], goal, st),
                Some([5405.0, -192.0, 204.125])
            );
            let fall = if moved {
                crate::bots::Goal::To([5000.0, -196.0, 204.125])
            } else {
                goal
            };
            at([5402.0, -198.0, 380.0], fall, air);
            assert_eq!(
                at([5404.4, -200.4, 336.125], fall, st),
                None,
                "moved {moved}"
            );
            assert!(f.fell, "moved {moved}");
            assert!(
                f.avoid.iter().any(|e| (e.0, e.1) == (1, 2)),
                "moved {moved}"
            );
            assert!(f.dead.iter().any(|e| (e.0, e.1) == (1, 2)), "moved {moved}");
        }
    }

    /// mp_ship's hold floor at 56: a bot run along the wall at y -400 for
    /// the drop to (3264, -378, -64) was stopped short of the lip by a bot
    /// dropping ahead, pressed at its waypoint behind the wall and was run
    /// off the floor's south side by an unstick spell (seed 2195). Stopped
    /// there, it plans round the drop; on the move it runs on.
    #[test]
    fn a_run_off_a_drop_stopped_short_of_the_lip_plans_round_it() {
        let Some(world) = map_world("mp_ship") else {
            return;
        };
        let w = &world.collision;
        let g = NavGraph::from_parts(
            vec![
                Vec3::new(3329.0, -419.0, 56.125),
                Vec3::new(3296.0635, -419.05017, 56.125),
                Vec3::new(3264.001, -378.19662, -63.875),
                Vec3::new(3232.0, -351.0, -63.875),
            ],
            vec![vec![1], vec![2], vec![3], vec![2]],
        );
        let goal = crate::bots::Goal::To([3232.0, -351.0, -63.875]);
        let mut budget = 100;
        let run = |v: Vec3| Footing {
            still: false,
            velocity: v,
            ..Footing::STANDING
        };
        let bottom = Some([3264.001, -378.19662, -63.875]);
        let mut f = Follower::default();
        let mut at =
            |p: [f32; 3], v: Vec3| f.waypoint(&g, Some(w), goal, p, run(v), &mut budget, &mut || 0);
        let west = Vec3::new(-190.0, 0.0, 0.0);
        assert_eq!(
            at([3329.0, -419.0, 56.125], west),
            Some([3296.0635, -419.05017, 56.125])
        );
        assert_eq!(at([3299.8, -415.13, 56.125], west), bottom);
        assert_eq!(at([3280.3, -415.13, 56.125], west), bottom, "on the move");
        assert_eq!(
            at([3266.6, -415.13, 56.125], Vec3::new(-6.0, 0.0, 0.0)),
            None,
            "stopped"
        );
        assert_eq!(f.avoid.len(), 1);
    }

    /// mp_ship's mast hatch, seed 2171: off the rungs at (3705, 58, 992)
    /// the walk on west to (3648, 71, 994) arrives at the graph's scale and
    /// falls back down the hatch at a weapon's 0.9. The bot heading on at
    /// 0.9 climbed the ladder four times. Now the node is not passed.
    #[test]
    fn a_walk_on_that_falls_at_the_weapons_scale_is_not_taken() {
        let Some(world) = map_world("mp_ship") else {
            return;
        };
        let w = &world.collision;
        let (side, west) = (
            Vec3::new(3704.8745, 84.5, 992.125),
            Vec3::new(3648.0496, 71.05647, 994.125),
        );
        // Up the ladder from the deck at 760 to its head, then on.
        let (foot, head) = (
            Vec3::new(3695.0, 56.0, 760.125),
            Vec3::new(3704.8745, 56.0, 992.125),
        );
        let g = NavGraph::from_parts(
            vec![foot, head, side, west],
            vec![vec![1], vec![2], vec![3], vec![]],
        );
        let goal = crate::bots::Goal::To(west.into());
        let body = Footing {
            still: false,
            on_ground: true,
            on_ladder: false,
            velocity: Vec3::new(0.0, 85.0, 0.0),
        };
        // From where the body steps off, the straight line west crosses
        // the hatch: `side` is not passed early whatever the scale (seed
        // 1031), and the walk on from there falls at 0.9 only.
        let off_rungs = [3704.875, 57.676594, 992.125];
        for (move_scale, holds) in [(1.0, true), (0.9, false)] {
            let mut f = Follower {
                move_scale,
                path: vec![0, 1, 2, 3],
                next: 2,
                dest: Some(3),
                ..Follower::default()
            };
            let at = Vec3::from(off_rungs);
            assert_eq!(
                f.edge_holds(Some(w), at, body.velocity, false, &g),
                holds,
                "walk on at {move_scale}"
            );
            let mut budget = 100;
            let wp = f.waypoint(&g, Some(w), goal, off_rungs, body, &mut budget, &mut || 0);
            assert_eq!(wp, Some(side.into()), "at {move_scale}");
        }
    }

    /// The fall a body lives through shrinks with its health: whole, only
    /// past `bg_fallDamageMaxHeight` (the 110 percent); at 20 of 100, past
    /// 21 percent of the way (20 percent's share truncates to 19).
    #[test]
    fn the_fall_that_kills_follows_the_health() {
        let h = vcod_common::pmove::FallHeights::default();
        assert_eq!(fall_that_kills(100, 100, h), 480.0);
        let at_20 = fall_that_kills(20, 100, h);
        assert!((at_20 - (256.0 + 224.0 * 0.21)).abs() < 0.01, "{at_20}");
        assert!(fall_that_kills(1, 100, h) < 262.0);
        let bad = vcod_common::pmove::FallHeights {
            min: 480.0,
            max: 256.0,
        };
        assert_eq!(fall_that_kills(1, 100, bad), f32::INFINITY);
    }

    /// A plan begun on the rungs under mp_ship's crow's nest plate starts
    /// at the head; passed from the rungs 8 under it, the bot turned for
    /// the rim's exit, let go and fell down the hatch. It is passed only
    /// on the ground.
    #[test]
    fn a_plan_from_the_rungs_passes_its_head_only_on_the_ground() {
        let head = Vec3::new(4963.9, 64.0, 1219.125);
        let exit = Vec3::new(4963.9, 35.5, 1219.125);
        let g = NavGraph::from_parts(vec![head, exit], vec![vec![1], vec![0]]);
        let mut f = Follower {
            path: vec![0, 1],
            ..Follower::default()
        };
        let rungs = Footing {
            still: false,
            on_ground: false,
            on_ladder: true,
            velocity: Vec3::new(0.0, 0.0, 120.0),
        };
        let mut budget = 100;
        let mut rand = || 0;
        let goal = crate::bots::Goal::Roam;
        let at = [4959.9, 62.4, 1210.9];
        assert_eq!(
            f.waypoint(&g, None, goal, at, rungs, &mut budget, &mut rand),
            Some(head.into())
        );
        let ground = Footing {
            on_ground: true,
            on_ladder: false,
            ..rungs
        };
        let at = [4962.0, 63.0, 1219.125];
        assert_eq!(
            f.waypoint(&g, None, goal, at, ground, &mut budget, &mut rand),
            Some(exit.into())
        );
    }

    /// mp_ship's crow's nest: the creep off the head faces the rungs (they
    /// face -x at x 4975) and strafes to their middle at y 64; at the head
    /// of the ladder at x 3793, whose fall does not kill, it backs toward
    /// the foot as the graph's walk did.
    #[test]
    fn the_crows_nest_back_down_faces_the_rungs() {
        let Some(world) = map_world("mp_ship") else {
            return;
        };
        let g = graph_for("mp_ship", &world);
        let head = g.nearest([4963.9, 64.0, 1219.125]).unwrap();
        let foot = g.nearest([4959.0, 64.0, 692.125]).unwrap();
        let f = Follower {
            path: vec![head, foot],
            next: 1,
            ..Default::default()
        };
        let (yaw, middle) = f.back_face(&g).expect("a face");
        assert_eq!(yaw, 0.0);
        assert!((middle.y - 64.0).abs() < 0.5, "{middle}");
        let f = Follower {
            path: vec![
                g.nearest([3760.0, 65.0, 616.0]).unwrap(),
                g.nearest([3809.0, 65.0, 304.0]).unwrap(),
            ],
            next: 1,
            ..Default::default()
        };
        assert_eq!(f.back_face(&g), None);
    }

    /// mp_ship's dome at (3648, -317, 618): backing off it for the floor
    /// at 352, a body stands on the dome's 45 degree face 23 under the
    /// node, still on the drop. Planned again from there, it walked off
    /// the dome's south side into the hold (seed 2245). It backs on.
    #[test]
    fn a_body_on_a_drops_own_slope_keeps_the_drop() {
        let top = Vec3::new(3648.0232, -316.91226, 618.36273);
        let bottom = Vec3::new(3679.998, -318.20682, 352.125);
        let g = NavGraph::from_parts(vec![top, bottom], vec![vec![1], vec![]]);
        let mut f = Follower::default();
        let goal = crate::bots::Goal::To(bottom.into());
        let mut budget = 100;
        let mut at =
            |p: [f32; 3]| f.waypoint(&g, None, goal, p, Footing::STANDING, &mut budget, &mut || 0);
        assert_eq!(at(top.into()), Some(bottom.into()));
        assert_eq!(at([3666.7, -317.2, 595.6]), Some(bottom.into()));
        assert!(!f.fell);
    }
}
