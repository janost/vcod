//! `--save-bump`: the walker and the target of the player-bump capture.

use super::fixture::{PLAYERSTATE_FIXTURE_DIR, cmd_fields, serverinfo_key, write_fixture_file};
use super::*;
use vcod_common::net::Msec;

/// The target's feet under `client-probes/probe_bump`'s `probe_teleport 1`; the
/// walker is placed [`BUMP_BACK`] units behind along -x, both facing +x.
const BUMP_SPOT: [f32; 3] = [1132.0, -376.0, -151.875];

/// The target's feet under `probe_bump_spot wall`: the walker's spot, 200
/// behind, stands 8.875 short of a +y wall (cod11-player-clip.md 10.4).
const BUMP_SPOT_WALL: [f32; 3] = [144.0, 448.0, 0.25];

const BUMP_BACK: f32 = 200.0;

/// Standing this close in xy to the start is being placed.
const BUMP_PLACED: f32 = 10.0;

const BUMP_WAIT: Duration = Duration::from_secs(30);

const BUMP_SETTLE: Duration = Duration::from_millis(1000);

/// How long the target's box has to read a stance before the walk starts.
const BUMP_STANCE_HOLD: Duration = Duration::from_millis(1000);

const BUMP_STANCE_WAIT: Duration = Duration::from_secs(60);

const BUMP_HEAD_ON: Duration = Duration::from_millis(2000);

/// The glance runs this far right of the line (-y, facing +x), and stops once
/// this far past the target.
const BUMP_GLANCE_OFFSET: f32 = 20.0;

const BUMP_GLANCE: Duration = Duration::from_millis(2000);

const BUMP_GLANCE_PAST: f32 = 50.0;

/// The jump starts from rest this far behind the target's centre, 10 units
/// short of touching it, with forward and up held until it lands: the air
/// accel drifts it in slowly enough to come down on a prone target's top.
const BUMP_JUMP_FROM: f32 = 40.0;

const BUMP_JUMP_LIMIT: Duration = Duration::from_millis(3500);

const BUMP_LAND: Duration = Duration::from_millis(1500);

/// A walk back to a mark gives up after this and moves on.
const BUMP_NAV_LIMIT: Duration = Duration::from_secs(10);

/// Close enough to a mark: a one-cmd tap moves about 1.2 units.
const BUMP_NAV_TOL: f32 = 1.5;

/// Inside this of a mark the walk moves by single-cmd taps.
const BUMP_NAV_NEAR: f32 = 8.0;

/// How stale the velocity a cmd is chosen from is: a snapshot interval plus
/// the cmd lag, about 70 ms locally.
const BUMP_NAV_LAG: f32 = 0.1;

/// The overlap script, in ms of server time after the placement, around the
/// gsc's two setorigins at 12 s and 30 s.
const BUMP_STILL_UNTIL: i32 = 15_000;

const BUMP_WALK_FROM: i32 = 29_200;

const BUMP_WALK_UNTIL: i32 = 31_000;

const BUMP_OVERLAP_END: i32 = 34_000;

/// The target's schedule from the walker's placement: stand, crouch, stand
/// (a prone straight out of a crouch is sometimes refused), prone, stand.
const BUMP_TARGET_SCHEDULE: [(Duration, BumpStance); 4] = [
    (Duration::from_secs(35), BumpStance::Stand),
    (Duration::from_secs(25), BumpStance::Crouch),
    (Duration::from_millis(1500), BumpStance::Stand),
    (Duration::from_secs(25), BumpStance::Prone),
];

/// The target starts its schedule without a walker after this.
const BUMP_TARGET_WAIT: Duration = Duration::from_secs(120);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum BumpStance {
    Stand,
    Crouch,
    Prone,
}

impl BumpStance {
    fn label(self) -> &'static str {
        match self {
            BumpStance::Stand => "stand",
            BumpStance::Crouch => "crouch",
            BumpStance::Prone => "prone",
        }
    }

    /// `solid`'s top byte, `maxs.z + 32` (docs/research/cod11-player-clip.md).
    fn solid_top(self) -> i32 {
        match self {
            BumpStance::Stand => 102,
            BumpStance::Crouch => 82,
            BumpStance::Prone => 62,
        }
    }

    fn wbuttons(self) -> u8 {
        match self {
            BumpStance::Stand => 0,
            BumpStance::Crouch => net::msg::WBUTTON_CROUCH,
            BumpStance::Prone => net::msg::WBUTTON_PRONE,
        }
    }

    /// From `pm_flags`: prone, else ducked.
    fn of_pm_flags(f: i32) -> Self {
        if f & net::flags::PMF_PRONE != 0 {
            BumpStance::Prone
        } else if f & net::flags::PMF_DUCKED != 0 {
            BumpStance::Crouch
        } else {
            BumpStance::Stand
        }
    }
}

/// `--probe-bump-target`: stands on the gsc's spot and runs the stance
/// schedule, clocked from the moment the walker stands on its start mark. It
/// writes no fixture; it prints its own playerstate whenever a push is on it,
/// which is the only view of the stuck half's `pm_time` from this side.
#[derive(Default)]
pub(super) struct BumpTarget {
    first: Option<Instant>,
    started: Option<Instant>,
    stance: Option<BumpStance>,
    traced: Option<u32>,
}

impl BumpTarget {
    pub(super) fn cmd(&self) -> net::msg::UserCmd {
        let stance = self.stance.unwrap_or(BumpStance::Stand);
        net::msg::UserCmd {
            wbuttons: stance.wbuttons(),
            up: if stance == BumpStance::Stand { 0 } else { -127 },
            angles: [0, 0, 0],
            ..net::msg::NULL_USERCMD
        }
    }

    pub(super) fn step(&mut self, now: Instant, snap: &net::snapshot::Snapshot) {
        let p = wire();
        let first = *self.first.get_or_insert(now);
        let me = snap.ps.field_i32(p, "clientNum") as u32;
        let o = snap.ps.origin(p);
        if self.started.is_none() {
            let mark = [o[0] - BUMP_BACK, o[1], o[2]];
            let walker = snap.entities.iter().any(|(&n, e)| {
                n != me
                    && e.field_i32(p, "eType") == crate::entities::ET_PLAYER
                    && horiz_dist(e.origin(p), mark) <= 30.0
            });
            if walker || now.duration_since(first) >= BUMP_TARGET_WAIT {
                println!(
                    "BUMPT: schedule starts at t={} ({})",
                    snap.server_time,
                    if walker { "walker placed" } else { "no walker" }
                );
                self.started = Some(now);
            }
        }
        let stance = self.started.map(|t| {
            let mut left = now.duration_since(t);
            for (d, s) in BUMP_TARGET_SCHEDULE {
                if left < d {
                    return s;
                }
                left -= d;
            }
            BumpStance::Stand
        });
        if stance != self.stance {
            println!(
                "BUMPT: {} at t={}",
                stance.unwrap_or(BumpStance::Stand).label(),
                snap.server_time
            );
            self.stance = stance;
        }
        if self.traced == Some(snap.message_num) {
            return;
        }
        self.traced = Some(snap.message_num);
        let i = |n: &str| snap.ps.field_i32(p, n);
        let f = |n: &str| snap.ps.field_f32(p, n);
        let moving = f("velocity[0]").hypot(f("velocity[1]")) > 1.0;
        if moving
            || i("pm_time") != 0
            || i("pm_flags") & vcod_common::pmove::PMF_TIME_KNOCKBACK != 0
        {
            println!(
                "BUMPT: !snap msg={} t={} ct={} origin={},{},{} vel={},{},{} ground={} pm_flags={} pm_time={}",
                snap.message_num,
                snap.server_time,
                i("commandTime"),
                o[0],
                o[1],
                o[2],
                f("velocity[0]"),
                f("velocity[1]"),
                f("velocity[2]"),
                i("groundEntityNum"),
                i("pm_flags"),
                i("pm_time"),
            );
        }
    }
}

/// `--save-bump`'s phases. The bump script runs `AwaitStance` to `Back3` once
/// per target stance; the overlap script runs `Still` to `After` once.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
enum BumpPhase {
    #[default]
    Wait, // until the gsc has placed us
    Settle,      // still, BUMP_SETTLE
    AwaitStance, // still, until the target's solid reads the stance
    HeadOn,      // forward along +x, BUMP_HEAD_ON
    Back1,       // back to the start mark
    GlanceLine,  // to BUMP_GLANCE_OFFSET right of the start mark
    Glance,      // forward along +x past the target
    Back2,
    JumpLine, // to BUMP_JUMP_FROM behind the target
    Jump,     // forward and up from rest, held until landed
    Land,     // still, BUMP_LAND
    Back3,
    Still, // overlap: still until BUMP_STILL_UNTIL
    Idle,  // overlap: still until BUMP_WALK_FROM
    Walk,  // overlap: forward until BUMP_WALK_UNTIL
    After, // overlap: still until BUMP_OVERLAP_END
    Done,
}

impl BumpPhase {
    fn label(self) -> &'static str {
        match self {
            BumpPhase::Wait => "wait",
            BumpPhase::Settle => "settle",
            BumpPhase::AwaitStance => "await",
            BumpPhase::HeadOn => "headon",
            BumpPhase::Back1 => "back1",
            BumpPhase::GlanceLine => "glance-line",
            BumpPhase::Glance => "glance",
            BumpPhase::Back2 => "back2",
            BumpPhase::JumpLine => "jump-line",
            BumpPhase::Jump => "jump",
            BumpPhase::Land => "land",
            BumpPhase::Back3 => "back3",
            BumpPhase::Still => "still-overlap",
            BumpPhase::Idle => "idle",
            BumpPhase::Walk => "walk-overlap",
            BumpPhase::After => "after",
            BumpPhase::Done => "done",
        }
    }

    /// The phases whose stance tag is the target's, not the run's.
    fn per_stance(self) -> bool {
        matches!(
            self,
            BumpPhase::AwaitStance
                | BumpPhase::HeadOn
                | BumpPhase::Back1
                | BumpPhase::GlanceLine
                | BumpPhase::Glance
                | BumpPhase::Back2
                | BumpPhase::JumpLine
                | BumpPhase::Jump
                | BumpPhase::Land
                | BumpPhase::Back3
        )
    }
}

/// How far a ground mover at `v` along an axis coasts before friction stops
/// it: 5.5 friction with a 100 u/s control floor (`PM_Friction`).
fn bump_stop_distance(v: f32) -> f32 {
    use vcod_common::pmove::{PM_FRICTION, PM_STOPSPEED};
    let decel = PM_STOPSPEED * PM_FRICTION;
    if v > PM_STOPSPEED {
        (v - PM_STOPSPEED) / PM_FRICTION + PM_STOPSPEED * PM_STOPSPEED / (2.0 * decel)
    } else {
        v * v / (2.0 * decel)
    }
}

/// One move axis toward a mark `d` units away, moving at `v` along it, while
/// the mark is far: press until friction plus the lag would carry it to
/// within [`BUMP_NAV_NEAR`]. The compact cmd carries only -127/0/127.
fn bump_axis_far(d: f32, v: f32) -> i8 {
    let toward = v * d > 0.0;
    if !toward && v.abs() > 5.0 {
        return 0;
    }
    let carry = if toward {
        bump_stop_distance(v.abs()) + v.abs() * BUMP_NAV_LAG
    } else {
        0.0
    };
    if d.abs() - carry <= BUMP_NAV_NEAR {
        return 0;
    }
    if d > 0.0 { 127 } else { -127 }
}

/// The walker: waits for the gsc's placement, then either walks into the
/// target head-on, at a glance and in a jump once per target stance, or (the
/// overlap script) stands and walks through the gsc's two setorigins.
#[derive(Default)]
pub(super) struct BumpProbe {
    overlap: bool,
    /// [`BUMP_SPOT`], or [`BUMP_SPOT_WALL`] for the wall run.
    spot: [f32; 3],
    phase: BumpPhase,
    phase_started: Option<Instant>,
    /// Wall clock and server time of the snapshot that showed the placement.
    placed: Option<(Instant, i32)>,
    stance_idx: usize,
    /// Since when the target's solid has read the awaited stance.
    stance_since: Option<Instant>,
    /// The target's origin, its entity number and solid on the newest snapshot.
    target: Option<([f32; 3], u32, i32)>,
    origin: [f32; 3],
    vel: [f32; 3],
    ground: i32,
    airborne: bool,
    landed: bool,
    /// The newest snapshot's number, and the one a near-mark tap went out on.
    msg: u32,
    tap_msg: Option<u32>,
    /// Every `[phase]`, `!station`, `!cmd` and `!snap` line in arrival order.
    lines: Vec<String>,
    traced: Option<u32>,
    pub(super) notes: Vec<String>,
    /// Phases a stance change already noted, so each is noted once.
    stance_noted: Option<(usize, BumpPhase)>,
}

const BUMP_STANCES: [BumpStance; 3] = [BumpStance::Stand, BumpStance::Crouch, BumpStance::Prone];

impl BumpProbe {
    pub(super) fn new(overlap: bool, wall: bool) -> Self {
        BumpProbe {
            overlap,
            spot: if wall { BUMP_SPOT_WALL } else { BUMP_SPOT },
            ..Default::default()
        }
    }

    pub(super) fn running(&self) -> bool {
        self.phase != BumpPhase::Done
    }

    fn stance(&self) -> BumpStance {
        BUMP_STANCES[self.stance_idx.min(BUMP_STANCES.len() - 1)]
    }

    pub(super) fn label(&self) -> String {
        if self.phase.per_stance() {
            format!("{}/{}", self.stance().label(), self.phase.label())
        } else {
            self.phase.label().to_string()
        }
    }

    fn ms(&self, now: Instant) -> u128 {
        self.placed
            .map_or(0, |(t, _)| now.saturating_duration_since(t).as_millis())
    }

    fn target_xy(&self) -> [f32; 2] {
        let t = self.target.map_or(self.spot, |t| t.0);
        [t[0], t[1]]
    }

    /// The start mark, [`BUMP_BACK`] behind wherever the target stands now.
    fn start_mark(&self) -> [f32; 2] {
        let t = self.target_xy();
        [t[0] - BUMP_BACK, t[1]]
    }

    /// Where a walk back heads next: round the target if it is in the way,
    /// then to `mark`.
    fn back_waypoint(&self, mark: [f32; 2]) -> [f32; 2] {
        let t = self.target_xy();
        let (x, y) = (self.origin[0], self.origin[1]);
        if x > t[0] - 25.0 && (y - t[1]).abs() < 45.0 {
            let side = if y < t[1] { -1.0 } else { 1.0 };
            [x, t[1] + side * 50.0]
        } else if x > t[0] - 25.0 {
            [t[0] - 60.0, y]
        } else {
            mark
        }
    }

    fn nav_mark(&self) -> Option<[f32; 2]> {
        let s = self.start_mark();
        match self.phase {
            BumpPhase::Back1 | BumpPhase::Back2 | BumpPhase::Back3 => Some(s),
            BumpPhase::GlanceLine => Some([s[0], s[1] - BUMP_GLANCE_OFFSET]),
            BumpPhase::JumpLine => {
                let t = self.target_xy();
                Some([t[0] - BUMP_JUMP_FROM, t[1]])
            }
            _ => None,
        }
    }

    fn at_mark(&self, mark: [f32; 2]) -> bool {
        (self.origin[0] - mark[0]).abs() <= BUMP_NAV_TOL
            && (self.origin[1] - mark[1]).abs() <= BUMP_NAV_TOL
            && self.vel[0].hypot(self.vel[1]) < 1.0
    }

    pub(super) fn cmd(&mut self) -> net::msg::UserCmd {
        let mut cmd = net::msg::UserCmd {
            angles: [0, 0, 0],
            ..net::msg::NULL_USERCMD
        };
        match self.phase {
            BumpPhase::HeadOn | BumpPhase::Glance | BumpPhase::Walk => cmd.forward = 127,
            BumpPhase::Jump => {
                cmd.forward = 127;
                cmd.up = 127;
            }
            _ => {
                if let Some(mark) = self.nav_mark() {
                    let w = self.back_waypoint(mark);
                    // Facing +x: forward is +x, right is -y.
                    let d = [w[0] - self.origin[0], self.origin[1] - w[1]];
                    let v = [self.vel[0], -self.vel[1]];
                    cmd.forward = bump_axis_far(d[0], v[0]);
                    cmd.right = bump_axis_far(d[1], v[1]);
                    // Near the mark: one cmd, then wait for a snapshot that
                    // shows it and for the walker to stop, then the next.
                    let still = v[0].hypot(v[1]) < 2.0;
                    let tap_seen = self.tap_msg.is_none_or(|m| self.msg >= m + 2);
                    if cmd.forward == 0 && cmd.right == 0 && still && tap_seen {
                        let tap = |d: f32| match d {
                            d if d > BUMP_NAV_TOL => 127,
                            d if d < -BUMP_NAV_TOL => -127,
                            _ => 0,
                        };
                        cmd.forward = tap(d[0]);
                        cmd.right = tap(d[1]);
                        if cmd.forward != 0 || cmd.right != 0 {
                            self.tap_msg = Some(self.msg);
                        }
                    }
                }
            }
        }
        cmd
    }

    pub(super) fn record(&mut self, now: Instant, c: net::msg::UserCmd) {
        if matches!(self.phase, BumpPhase::Wait | BumpPhase::Done) {
            return;
        }
        let ms = self.ms(now);
        self.lines.push(format!("!cmd ms={ms} {}", cmd_fields(&c)));
    }

    fn enter(&mut self, now: Instant, next: BumpPhase) {
        if next == self.phase {
            return;
        }
        println!(
            "BUMP: {} -> {} at +{}ms",
            self.label(),
            next.label(),
            self.ms(now)
        );
        if self.phase == BumpPhase::Back3 {
            self.stance_idx += 1;
        }
        self.phase = next;
        self.phase_started = Some(now);
        self.stance_since = None;
        self.airborne = false;
        self.landed = false;
        if next != BumpPhase::Done {
            self.lines.push(format!("[phase {}]", self.label()));
            self.lines.push(format!(
                "!station ms={} origin={},{},{} target={}",
                self.ms(now),
                self.origin[0],
                self.origin[1],
                self.origin[2],
                self.target.map_or("none".to_string(), |t| format!(
                    "{},{},{}",
                    t.0[0], t.0[1], t.0[2]
                ))
            ));
        }
    }

    /// The other player nearest the spot: `(origin, entity, solid)`.
    fn read_target(
        snap: &net::snapshot::Snapshot,
        me: u32,
        spot: [f32; 3],
    ) -> Option<([f32; 3], u32, i32)> {
        let p = wire();
        snap.entities
            .iter()
            .filter(|&(&n, e)| n != me && e.field_i32(p, "eType") == crate::entities::ET_PLAYER)
            .map(|(&n, e)| (e.origin(p), n, e.field_i32(p, "solid")))
            .min_by(|a, b| horiz_dist(a.0, spot).total_cmp(&horiz_dist(b.0, spot)))
    }

    fn snap_line(&self, snap: &net::snapshot::Snapshot) -> String {
        let p = wire();
        let i = |n: &str| snap.ps.field_i32(p, n);
        let f = |n: &str| snap.ps.field_f32(p, n);
        let target = match self.target {
            Some((o, n, solid)) => {
                let e = &snap.entities[&n];
                format!(
                    "target={},{},{} target_solid={solid} target_num={n} target_vel={},{},{} target_ground={}",
                    o[0],
                    o[1],
                    o[2],
                    e.field_f32(p, "pos.trDelta[0]"),
                    e.field_f32(p, "pos.trDelta[1]"),
                    e.field_f32(p, "pos.trDelta[2]"),
                    e.field_i32(p, "groundEntityNum"),
                )
            }
            None => "target=none".to_string(),
        };
        format!(
            "!snap msg={} t={} ct={} origin={},{},{} vel={},{},{} ground={} pm_type={} pm_flags={} pm_time={} jump_peak={} speed={} stance={} {target}",
            snap.message_num,
            snap.server_time,
            i("commandTime"),
            self.origin[0],
            self.origin[1],
            self.origin[2],
            f("velocity[0]"),
            f("velocity[1]"),
            f("velocity[2]"),
            i("groundEntityNum"),
            i("pm_type"),
            i("pm_flags"),
            i("pm_time"),
            f("fJumpPeak"),
            i("speed"),
            BumpStance::of_pm_flags(i("pm_flags")).label(),
        )
    }

    /// Feeds the newest snapshot in. Returns true once the run is done.
    pub(super) fn step(&mut self, now: Instant, snap: &net::snapshot::Snapshot) -> bool {
        let p = wire();
        let phase_started = *self.phase_started.get_or_insert(now);
        let in_phase = now.duration_since(phase_started);
        let me = snap.ps.field_i32(p, "clientNum") as u32;
        self.origin = snap.ps.origin(p);
        self.vel = [
            snap.ps.field_f32(p, "velocity[0]"),
            snap.ps.field_f32(p, "velocity[1]"),
            snap.ps.field_f32(p, "velocity[2]"),
        ];
        self.ground = snap.ps.field_i32(p, "groundEntityNum");
        self.msg = snap.message_num;
        self.target = Self::read_target(snap, me, self.spot);

        if self.phase != BumpPhase::Wait && self.traced != Some(snap.message_num) {
            self.traced = Some(snap.message_num);
            let line = self.snap_line(snap);
            self.lines.push(line);
        }
        if snap.ps.field_i32(p, "pm_type") == PM_DEAD {
            self.notes
                .push(format!("# BROKEN died in {}", self.label()));
            self.enter(now, BumpPhase::Done);
            return true;
        }

        let top = self.target.map(|t| (t.2 >> 16) & 0xff);
        let want = self.stance().solid_top();
        // A 0 is the stuck mark's CORPSE contents, not a stance change.
        if self.phase.per_stance()
            && self.phase != BumpPhase::AwaitStance
            && top != Some(want)
            && self.stance_noted != Some((self.stance_idx, self.phase))
        {
            self.stance_noted = Some((self.stance_idx, self.phase));
            let what = match top {
                Some(0) => "# the target's solid read 0",
                _ => "# BROKEN the target's solid changed stance",
            };
            self.notes.push(format!(
                "{what} during {} (top byte {top:?}, stance {want})",
                self.label()
            ));
        }
        if self.phase == BumpPhase::Jump {
            if self.ground == net::protocol::ENTITYNUM_NONE as i32 {
                self.airborne = true;
            } else if self.airborne {
                self.landed = true;
            }
        }
        let placed_st = self.placed.map_or(0, |(_, st)| st);
        let since_placed = snap.server_time.since(Msec::from_raw(placed_st));
        let nav_done = |s: &Self| s.nav_mark().is_some_and(|m| s.at_mark(m));
        let nav_timeout = in_phase >= BUMP_NAV_LIMIT;
        if self.nav_mark().is_some() && nav_timeout && !nav_done(self) {
            self.notes.push(format!(
                "# BROKEN {} did not reach its mark in {} s; stopped at {},{}",
                self.label(),
                BUMP_NAV_LIMIT.as_secs(),
                self.origin[0],
                self.origin[1]
            ));
        }
        let next = match self.phase {
            BumpPhase::Wait => {
                let start = [self.spot[0] - BUMP_BACK, self.spot[1], self.spot[2]];
                if horiz_dist(self.origin, start) <= BUMP_PLACED {
                    self.placed = Some((now, snap.server_time.raw()));
                    BumpPhase::Settle
                } else if in_phase >= BUMP_WAIT {
                    self.notes.push(
                        "# BROKEN not placed: run client-probes/probe_bump with +set probe_teleport 1"
                            .to_string(),
                    );
                    BumpPhase::Done
                } else {
                    BumpPhase::Wait
                }
            }
            BumpPhase::Settle if in_phase >= BUMP_SETTLE => {
                if self.overlap {
                    BumpPhase::Still
                } else {
                    BumpPhase::AwaitStance
                }
            }
            BumpPhase::AwaitStance => {
                if top == Some(want) {
                    let since = *self.stance_since.get_or_insert(now);
                    if now.duration_since(since) >= BUMP_STANCE_HOLD {
                        BumpPhase::HeadOn
                    } else {
                        BumpPhase::AwaitStance
                    }
                } else {
                    self.stance_since = None;
                    if in_phase >= BUMP_STANCE_WAIT {
                        self.notes.push(format!(
                            "# BROKEN the target's solid never read {} (top byte {want}), last {top:?}",
                            self.stance().label()
                        ));
                        BumpPhase::Done
                    } else {
                        BumpPhase::AwaitStance
                    }
                }
            }
            BumpPhase::HeadOn if in_phase >= BUMP_HEAD_ON => BumpPhase::Back1,
            BumpPhase::Back1 if nav_done(self) || nav_timeout => BumpPhase::GlanceLine,
            BumpPhase::GlanceLine if nav_done(self) || nav_timeout => BumpPhase::Glance,
            BumpPhase::Glance
                if in_phase >= BUMP_GLANCE
                    || self.origin[0] >= self.target_xy()[0] + BUMP_GLANCE_PAST =>
            {
                BumpPhase::Back2
            }
            BumpPhase::Back2 if nav_done(self) || nav_timeout => BumpPhase::JumpLine,
            BumpPhase::JumpLine if nav_done(self) || nav_timeout => BumpPhase::Jump,
            BumpPhase::Jump if self.landed => {
                if self.target.map(|t| t.1 as i32) != Some(self.ground) {
                    self.notes.push(format!(
                        "# BROKEN {} landed on {}, not on the target",
                        self.label(),
                        self.ground
                    ));
                }
                BumpPhase::Land
            }
            BumpPhase::Jump if in_phase >= BUMP_JUMP_LIMIT => {
                self.notes.push(format!(
                    "# BROKEN {} never landed (airborne {})",
                    self.label(),
                    self.airborne
                ));
                BumpPhase::Land
            }
            BumpPhase::Land if in_phase >= BUMP_LAND => BumpPhase::Back3,
            BumpPhase::Back3 if nav_done(self) || nav_timeout => {
                if self.stance_idx + 1 < BUMP_STANCES.len() {
                    BumpPhase::AwaitStance
                } else {
                    BumpPhase::Done
                }
            }
            BumpPhase::Still if since_placed >= BUMP_STILL_UNTIL => BumpPhase::Idle,
            BumpPhase::Idle if since_placed >= BUMP_WALK_FROM => BumpPhase::Walk,
            BumpPhase::Walk if since_placed >= BUMP_WALK_UNTIL => BumpPhase::After,
            BumpPhase::After if since_placed >= BUMP_OVERLAP_END => BumpPhase::Done,
            p => p,
        };
        self.enter(now, next);
        self.phase == BumpPhase::Done
    }
}

/// The walker's fixture: header, notes, then the lines in arrival order, a
/// `[phase]` and `!station` opening each phase. Named `dm` whatever cs 0
/// says: retail runs the capture as gametype `probe_bump`, which is `dm`
/// underneath.
pub(super) fn write_bump_fixture(
    configstrings: &[String],
    join: &JoinProbe,
    bp: &BumpProbe,
    tag: Option<&str>,
    overwrite: bool,
) -> anyhow::Result<()> {
    let key = |k: &str| serverinfo_key(configstrings, k);
    let (map, gametype) = (key("mapname"), key("g_gametype"));
    let mut out = String::new();
    out.push_str(&format!(
        "# Retail CoD {} dedicated server: one player walking into, round, onto and\n",
        retail_build()
    ));
    out.push_str("# through another.\n");
    out.push_str(&format!(
        "# bump spot={},{},{} yaw=0\n",
        bp.spot[0], bp.spot[1], bp.spot[2]
    ));
    out.push_str(&format!(
        "# map {map}, gametype {gametype}, walker team={} weapon={}, script {}\n",
        join.team,
        join.weapon,
        if bp.overlap { "overlap" } else { "bump" }
    ));
    out.push_str(
        "# client-probes/probe_bump under tools/run_probe.sh with +set probe_teleport 1 puts\n",
    );
    out.push_str(&format!(
        "# the target (axis, --probe-bump-target) on the spot and the walker (allies, this\n\
# probe) {BUMP_BACK} units behind it along -x, both facing +x. The recipe is in\n\
# crates/gsc/tests/fixtures/semantics/client-probes/README.md, probe_bump.\n"
    ));
    if bp.spot == BUMP_SPOT_WALL {
        out.push_str(
            "# Wall run, +set probe_bump_spot wall: the walker stands 8.875 short of a +y wall,\n\
# so the push drives it into the wall.\n",
        );
    }
    if bp.overlap {
        out.push_str(&format!(
            "# Overlap script, +set probe_overlap 1: the gsc setorigins the target onto the walker\n\
# 12 s and 30 s after both are placed. The walker stands still until {} ms of server\n\
# time past its placement, walks forward (+x) from {} to {} ms and stands until {} ms.\n",
            BUMP_STILL_UNTIL, BUMP_WALK_FROM, BUMP_WALK_UNTIL, BUMP_OVERLAP_END
        ));
    } else {
        out.push_str(&format!(
            "# Bump script, once per target stance (stand, crouch, prone), each started once the\n\
# target's solid top byte has read the stance for {} ms: headon walks forward {} ms;\n\
# back* walks back to the start mark {BUMP_BACK} behind the target; glance-line moves\n\
# {BUMP_GLANCE_OFFSET} right (-y) of it; glance walks forward {} ms or until {BUMP_GLANCE_PAST} past the\n\
# target; jump-line moves to {BUMP_JUMP_FROM} behind the target's centre; jump holds\n\
# forward and up from rest until landed; land stands {} ms.\n",
            BUMP_STANCE_HOLD.as_millis(),
            BUMP_HEAD_ON.as_millis(),
            BUMP_GLANCE.as_millis(),
            BUMP_LAND.as_millis(),
        ));
    }
    out.push_str(
        "# !cmd is a usercmd as it went on the wire (angles rebased on delta_angles), ms\n",
    );
    out.push_str(
        "# after the placement. !snap is the walker's playerstate per snapshot (jump_peak\n",
    );
    out.push_str("# its fJumpPeak), stance from pm_flags, with the target's entity: target= its\n");
    out.push_str("# pos.trBase, target_vel its\n");
    out.push_str("# pos.trDelta, target_solid its packed solid. Floats are printed exactly.\n");
    for n in &bp.notes {
        out.push_str(n);
        out.push('\n');
    }
    for l in &bp.lines {
        out.push_str(l);
        out.push('\n');
    }
    let role = match tag {
        Some(t) => format!("{t}-walker"),
        None => "walker".to_string(),
    };
    // The tag goes before the role here, not after it as in every other mode's name.
    let path = write_fixture_file(
        PLAYERSTATE_FIXTURE_DIR,
        &format!("{map}-dm-bump-{role}.txt"),
        &out,
        overwrite,
    )?;
    println!("bump: {} lines -> {path}", bp.lines.len());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The bump walk presses toward a far mark until friction and the
    /// velocity's lag would coast it inside 8 units, and never presses
    /// while it moves away faster than 5 u/s.
    #[test]
    fn the_bump_walk_presses_until_its_coast_reaches_the_mark() {
        // PM_Friction from 300 u/s: (300 - 100) / 5.5 + 100^2 / 1100
        assert!((bump_stop_distance(300.0) - 45.4545).abs() < 1e-3);
        assert!((bump_stop_distance(50.0) - 2.2727).abs() < 1e-3);
        // 300 u/s toward coasts 45.45 + 30 of lag
        assert_eq!(bump_axis_far(90.0, 300.0), 127);
        assert_eq!(bump_axis_far(83.0, 300.0), 0);
        assert_eq!(bump_axis_far(-90.0, -300.0), -127);
        assert_eq!(bump_axis_far(9.0, 0.0), 127);
        assert_eq!(bump_axis_far(8.0, 0.0), 0);
        assert_eq!(bump_axis_far(100.0, -6.0), 0, "moving away");
        assert_eq!(bump_axis_far(100.0, -4.0), 127, "drifting");
    }
}
