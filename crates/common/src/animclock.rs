//! A player DObj's per-channel clock: when the clip on a channel started, at
//! which phase and rate it plays, and the clips it is still blending out of.
//! Retail keeps it in the client info's two lerp frames (`ci+0x37c` legs,
//! `ci+0x3ac` torso) and the XAnim tree's node times and weights;
//! `BG_PlayerAnimation`'s per-channel updater (`game.mp.i386.so` 0x2aba0) and
//! its anim setter (0x2a600) drive it once an end frame, and the cgame's copy
//! of the two (`cgame_mp_x86.dll` 0x30003be0, 0x300036b0) once a drawn frame
//! (docs/research/cod11-movers.md, section 15.7 and
//! "The drawn body's clip").

use crate::animscript::{movetype_bit, movetype_bit_hi};
use crate::animtree::{AnimMotion, PlayerAnims};
use glam::Vec3;

thread_local! {
    static RESTART_WEIGHTED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Test hook: while on, this thread's 1.5 and UO setter restarts a weighted
/// node as 1.1's does, the rule before "The setter on a weighted node"
/// (cod11-combat.md 3.1), so a gate can show its capture tells the two apart.
#[doc(hidden)]
pub fn restart_weighted_nodes(on: bool) {
    RESTART_WEIGHTED.with(|c| c.set(on));
}

/// The restart toggle on a wire anim value.
const TOGGLE: i32 = 0x200;

/// `eFlags` 0x800: a body `cloneplayer` made under 250 ms ago, whose death
/// anim plays from its start; without it the setter shows the death anim's
/// last frame at once (0x2a600, `cgame_mp_x86.dll` 0x300036b0).
const EF_FRESH_CORPSE: i32 = 0x800;

/// The two clocks an update reads. The server's end frame passes
/// `level.time` for both; the cgame plays its clips and starts its blends on
/// `cg.time` and measures the rate whenever the newest snapshot's
/// `serverTime` (`bg+0x4`, `cg.latestSnapshotTime`) moves.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Times {
    pub clock_ms: i32,
    pub snapshot_ms: i32,
}

impl Times {
    /// An end frame at level time `now_ms`.
    pub fn end_frame(now_ms: i32) -> Times {
        Times {
            clock_ms: now_ms,
            snapshot_ms: now_ms,
        }
    }
}

/// What an update reads off the entity besides its anim: the client
/// number the first phase spreads by (`ci+0x8`), `pos.trBase` and `eFlags`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Seen {
    pub client_num: i32,
    pub tr_base: Vec3,
    pub eflags: i32,
}

/// Which engine's clock a [`ChannelClock`] imitates, as [`update_body`]
/// names it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Flavour {
    /// 1.1d's server and 1.1's cgame: no stance window, and a looping
    /// clip's step keeps its wrap on the stack ([`Play::step`]).
    #[default]
    V1_1,
    /// 1.5's and UO's server: the windowed setter ([`blend_ms_windowed`])
    /// and its spread phase ([`spread_phase_15`]).
    Server15,
    /// 1.5's and UO's cgames: the windowed setter with the cgames' spread
    /// ([`spread_phase_15_cgame`]).
    Cgame15,
    /// `cod2_lnxded` 1.3: the windowed setter, stored increments, weights
    /// and rate ([`Ramp::cod2`], [`rate_cod2`]; cod2-player-anims.md 7),
    /// and CoD2's tree handling ([`Flavour::cod2_tree`]).
    Cod2,
    /// 1.7: the windowed setter, CoD2's tree handling, and the rate over
    /// the float `moveSpeed`.
    Cod4,
}

impl Flavour {
    /// The 1.5, UO, CoD2 and CoD4 setter, whose blends a crouch or prone
    /// change stretches.
    fn windowed(self) -> bool {
        self != Flavour::V1_1
    }

    /// `cod2_lnxded`'s stored arithmetic: each step stores its increment,
    /// each weight its goal time, the rate each quotient.
    fn stored(self) -> bool {
        self == Flavour::Cod2
    }

    /// CoD2's and CoD4's tree handling: a KnobAll clears beside the death's
    /// path at once, and a setter starting a node the other lerp frame
    /// holds starts its own (cod2-player-anims.md 7 and 12 hold those
    /// weights against gdb dumps). CoD1's engine keeps a cleared node's
    /// weight until the next step (cod11-movers.md 15), unread on CoD2.
    fn cod2_tree(self) -> bool {
        matches!(self, Flavour::Cod2 | Flavour::Cod4)
    }
}

/// How many clips a channel keeps blending out at once. A change lands at
/// most once an end frame; with blends of 250 ms at most, a sixth one is at
/// weight 0 by the time it would be pushed out. A blend stretched to 400 ms
/// by 1.5's and UO's crouch window ([`blend_ms_windowed`]) may still carry
/// weight then; changing the count changes poses.
const FADES: usize = 5;
/// The sync groups a clock remembers past their blend out: the aim and
/// fire groups of one stance and the other's.
const HELD: usize = 4;

/// One clip on a channel: the clip's time in seconds at level time `t` is
/// `base_s + (t - base_ms) * rate / 1000`. The engine advances a node by
/// `frametime * rate` at each frame's start, and the rate only changes at an
/// end frame, so the clock rebases there.
///
/// Beside the seconds it keeps the engine's own node time, the fraction of
/// the clip a float sum of per-frame steps has reached ([`Play::time`]),
/// which 1.5's and UO's stored pose samples at (cod11-combat.md 3.1.4).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Play {
    /// The anim index, toggle off.
    index: i32,
    base_ms: i32,
    base_s: f32,
    rate: f32,
    /// The node time `time_t` held at level time `time_ms`.
    time_ms: i32,
    time_t: f32,
    /// [`AnimMotion::frequency`] and looping, which the step reads.
    frequency: f32,
    looping: bool,
    /// A sync group's node (an MG42 gunner's turret anim), whose step
    /// stores its increment before the sum ([`Play::step`]).
    sync: bool,
}

/// The server frame the engine steps a node time by: `level.frametime`
/// times 0.001 at `sv_fps 20` (1.5b `G_DObjUpdateServerTime` 0x7c5b1).
const STEP_MS: i32 = 50;

impl Play {
    fn seconds(&self, now_ms: i32) -> f32 {
        self.base_s + now_ms.wrapping_sub(self.base_ms).max(0) as f32 * 0.001 * self.rate
    }

    /// The node time after every whole frame step up to `now_ms`.
    /// Stops early once a step returns its input bit for bit (rate 0, a
    /// clip held at 1): step is pure, so every later step would too.
    fn time(&self, now_ms: i32, f: Flavour) -> f32 {
        let steps = now_ms.wrapping_sub(self.time_ms).max(0) / STEP_MS;
        let mut t = self.time_t;
        for _ in 0..steps {
            let next = self.step(t, f);
            if next.to_bits() == t.to_bits() {
                break;
            }
            t = next;
        }
        t
    }

    /// One frame of the leaf arm of 1.5b's 0x80c84f0 (UO 0x80bd884): the
    /// node time plus `dtime * rate * frequency` on the x87 stack, stored;
    /// a looping clip wraps by 1, any other stops at 1, and a step that
    /// would leave it at or under 0 is dropped.
    ///
    /// A sync node's arm (1.5b 0x80c86dc..0x80c86f8) stores the increment
    /// `frequency * rate * dtime` as a float first, then the sum. 1.1d's
    /// leaf arm (`cod_lnxded` 0x80bbab0) keeps the sum and a looping clip's
    /// wrap by 1 on the stack and stores the result once.
    fn step(&self, t: f32, f: Flavour) -> f32 {
        let dt = f64::from(STEP_MS as f32 * 0.001);
        let inc = dt * f64::from(self.rate) * f64::from(self.frequency);
        let inc = if self.sync || f.stored() {
            f64::from(inc as f32)
        } else {
            inc
        };
        if f == Flavour::V1_1 && self.looping {
            let mut next = f64::from(t) + inc;
            while next < 0.0 {
                next += 1.0;
            }
            while next >= 1.0 {
                next -= 1.0;
            }
            return next as f32;
        }
        let mut next = (f64::from(t) + inc) as f32;
        if self.looping {
            while next < 0.0 {
                next += 1.0;
            }
            while next >= 1.0 {
                next -= 1.0;
            }
        } else if next >= 1.0 {
            next = 1.0;
        } else if next <= 0.0 {
            return t;
        }
        next
    }

    /// Steps the node time up to `now_ms` and keeps it there, so a long
    /// clip never walks its whole history.
    fn settle(&mut self, now_ms: i32, f: Flavour) {
        let steps = now_ms.wrapping_sub(self.time_ms).max(0) / STEP_MS;
        self.time_t = self.time(now_ms, f);
        self.time_ms = self.time_ms.wrapping_add(steps * STEP_MS);
    }
}

/// A leaf's own node time under a plain (not sync) group, as a UO seat's
/// leaves keep it: 0 when the leaf is first given weight, then stepped by
/// [`Play::step`] at rate 1 each frame (the seat's legs anims carry no
/// `moveSpeed`, and the groups and leaves are set at rate 1), a nested
/// node passing its children `dtime * rate` (cod11-combat.md 3.1.4).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct LeafTime(Play);

/// A seat leaf's step: neither 1.1d's kept wrap nor CoD2's stored increment.
const LEAF: Flavour = Flavour::Cgame15;

impl LeafTime {
    /// A leaf allocated at level time `start_ms`.
    pub fn new(start_ms: i32, frequency: f32, looping: bool) -> LeafTime {
        LeafTime(Play {
            rate: 1.0,
            time_ms: start_ms,
            frequency,
            looping,
            ..Play::default()
        })
    }

    /// The node time after every whole frame step up to `now_ms`.
    pub fn at(&self, now_ms: i32) -> f32 {
        self.0.time(now_ms, LEAF)
    }

    /// Steps the time up to `now_ms` and keeps it there.
    pub fn settle(&mut self, now_ms: i32) {
        self.0.settle(now_ms, LEAF);
    }

    /// Keeps the time where it stopped and steps it again from `now_ms`.
    pub fn resume(&mut self, now_ms: i32) {
        self.0.time_ms = now_ms;
    }
}

/// A node's weight easing to its goal: `XAnimSetGoalWeight` (`cod_lnxded`
/// 0x080bd674) gives it `|goal - weight| * time` to get there and the
/// per-frame step (0x080bb130) moves it the remaining distance over the
/// remaining goal time, which is a straight line from where the setter found
/// it at a slope of one over the blend time.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Ramp {
    from: f32,
    to: f32,
    start_ms: i32,
    ms: f32,
    /// 1.5's and UO's step, which stores each frame's weight
    /// ([`Ramp::stepped`]); off, the line itself.
    stored: bool,
    /// `cod2_lnxded` 1.3's goal time in seconds, as its setter stores it
    /// ([`Ramp::cod2`]); `None` on the other engines.
    cod2: Option<f32>,
}

impl Ramp {
    fn new(from: f32, to: f32, start_ms: i32, ms: f32) -> Ramp {
        Ramp {
            from,
            to,
            start_ms,
            ms,
            stored: false,
            cod2: None,
        }
    }

    /// `cod2_lnxded` 1.3's `XAnimSetGoalWeight` (0x80c20de) on a node at
    /// `weight` whose goal was `old_goal`, to `to` over `ms`: a node at 0
    /// starts at `0.001 * to`, and the goal time is the part of `ms` the
    /// distance from that weight takes, `(to - w) * time / to` up or `(w -
    /// to) * time / w` down, each on the stack and stored; to 0 it is the
    /// whole time when the old goal was not 0, else the sooner of the two.
    /// cod2-player-anims.md 7.
    fn cod2(weight: f32, old_goal: f32, to: f32, start_ms: i32, ms: f32, gt: f32) -> Ramp {
        let time = (f64::from(ms) * f64::from(0.001f32)) as f32;
        let w = if ms == 0.0 {
            to
        } else if weight == 0.0 {
            (f64::from(to) * f64::from(0.001f32)) as f32
        } else {
            weight
        };
        let (w64, to64, t64) = (f64::from(w), f64::from(to), f64::from(time));
        let gt = if to != 0.0 {
            if to >= w {
                ((to64 - w64) * t64 / to64) as f32
            } else {
                ((w64 - to64) * t64 / w64) as f32
            }
        } else if old_goal != 0.0 || gt > time {
            time
        } else {
            gt
        };
        Ramp {
            cod2: Some(gt),
            ..Ramp::new(w, to, start_ms, gt * 1000.0)
        }
    }

    /// The goal time a [`Ramp::cod2`] ramp has left at `now_ms`.
    fn cod2_left(&self, now_ms: i32) -> f32 {
        let since = now_ms.wrapping_sub(self.start_ms).max(0);
        self.cod2_stepped(since / STEP_MS).1
    }

    /// The weight and goal time after `steps` frames of `cod2_lnxded`
    /// 1.3's step (0x80bf112): the goal once the frame's `dt` reaches the
    /// goal time, else `(goal - w) * dt / gt + w` on the stack, stored (one
    /// under 1e-6 raised to `0.001 * goal`), the goal time less `dt`,
    /// stored.
    fn cod2_stepped(&self, steps: i32) -> (f32, f32) {
        let dt = STEP_MS as f32 * 0.001;
        let (mut w, mut gt) = (self.from, self.cod2.unwrap_or(0.0));
        for _ in 0..steps {
            if dt >= gt {
                return (self.to, 0.0);
            }
            let d = (f64::from(self.to) - f64::from(w)) * f64::from(dt) / f64::from(gt);
            w = (d + f64::from(w)) as f32;
            if 1e-6f32 > w {
                w = (f64::from(self.to) * f64::from(0.001f32)) as f32;
            }
            gt = (f64::from(gt) - f64::from(dt)) as f32;
        }
        (w, gt)
    }

    /// The setter's ramp from a weight of `from` over `ms`: a node at 0 with
    /// a goal starts at `0.001 * to` (1.1d 0x080bd7e9, 1.5b 0x80cb20e, UO
    /// 0x80c05a2), its time still the whole distance's. A body traced before
    /// the next frame's step reads that, not 0 (cod11-turrets.md 13.9).
    fn raised(from: f32, to: f32, start_ms: i32, ms: f32) -> Ramp {
        let start = if from == 0.0 && ms > 0.0 {
            (f64::from(0.001f32) * f64::from(to)) as f32
        } else {
            from
        };
        Ramp::new(start, to, start_ms, ms)
    }

    /// The last frame from the ramp's start at which it reads above 0, or
    /// `i32::MAX` for one that never reaches 0.
    fn last_weighted(&self) -> i32 {
        if self.to > 0.0 {
            return i32::MAX;
        }
        let mut last = self.start_ms;
        while self.at(last + STEP_MS) > 0.0 {
            last += STEP_MS;
        }
        last
    }

    /// A goal of 0 at goal time 0 on a node that still has weight: the
    /// clear writes the goal and its time but not the weight (`cod_lnxded`
    /// 0x080bd35c, the weight kept when 0x080bd0ec reads the node weighted),
    /// so the weight reads `from` until the next frame's step lands it
    /// (0x080bb130). The goal setter's time 0 writes the weight itself
    /// (0x080bd674), so a raised node's time-0 ramp is [`Ramp::new`]'s.
    fn cleared_next_step(from: f32, start_ms: i32) -> Ramp {
        Ramp::new(from, 0.0, start_ms, f32::MIN_POSITIVE)
    }

    fn stored_on(self, stored: bool) -> Ramp {
        Ramp { stored, ..self }
    }

    /// The weight after `steps` frames of 1.5b's 0x80c788a (UO 0x80bcc1e):
    /// it lands on the goal once the goal time is under the step plus
    /// 0.001, else takes `(goal - weight) * dt / goalTime` on the x87
    /// stack, stores the sum, and takes `dt` off the stored goal time.
    fn stepped(&self, steps: i32) -> f32 {
        let dt = STEP_MS as f32 * 0.001;
        let (mut w, mut gt) = (self.from, self.ms * 0.001);
        for _ in 0..steps {
            if f64::from(gt) < f64::from(dt) + f64::from(0.001f32) {
                return self.to;
            }
            let d = f64::from(self.to - w) * f64::from(dt) / f64::from(gt);
            w = (d + f64::from(w)) as f32;
            gt -= dt;
        }
        w
    }

    fn at(&self, now_ms: i32) -> f32 {
        let since = now_ms.wrapping_sub(self.start_ms);
        if self.cod2.is_some() && since >= 0 && since % STEP_MS == 0 {
            return self.cod2_stepped(since / STEP_MS).0;
        }
        if self.ms <= 0.0 {
            return self.to;
        }
        if self.stored && since >= 0 && since % STEP_MS == 0 {
            return self.stepped(since / STEP_MS);
        }
        let s = (now_ms.wrapping_sub(self.start_ms) as f32 / self.ms).max(0.0);
        if s >= 1.0 {
            // 0x080bb130 lands on the goal.
            return self.to;
        }
        self.from + (self.to - self.from) * s
    }
}

/// The legs node's weight while a torso anim plays (`game.mp.i386.so`
/// 0x6eee4).
const LEGS_UNDER_TORSO: f32 = 0.01;

/// The weights of `main`'s two children in the player's anim tree, which
/// only the torso setter moves (0x2aa62..0x2ab5b): a torso anim sends the
/// `torso` node to 1 and the `legs` node to 0.01, no torso anim sends them
/// to 0 and 1, each over the torso's blend.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Nodes {
    torso: Ramp,
    legs: Ramp,
}

impl Default for Nodes {
    /// The legs leaf's first goal weight raises its ancestors to 1 at once
    /// (0x080bdf64 with the legs' blend 0); the torso node waits for a
    /// torso anim.
    fn default() -> Nodes {
        Nodes {
            torso: Ramp::new(0.0, 0.0, 0, 0.0),
            legs: Ramp::new(1.0, 1.0, 0, 0.0),
        }
    }
}

/// `main`'s `torso` and `legs` node weights at one instant.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NodeWeights {
    pub torso: f32,
    pub legs: f32,
}

impl Default for NodeWeights {
    fn default() -> NodeWeights {
        NodeWeights {
            torso: 0.0,
            legs: 1.0,
        }
    }
}

/// One clip of a posed channel: what it plays, how far in, and its share.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Layer {
    /// The anim index, toggle off.
    pub index: i32,
    /// The clip's time in seconds, unwrapped; the pose wraps a looping clip.
    pub seconds: f32,
    /// The engine's node time, the fraction of the clip played, wrapped;
    /// `None` for a node the clock knows no leaf frequency for (a node
    /// with children, as a clip name the group pitch picks under).
    pub time: Option<f32>,
    pub weight: f32,
}

/// One channel's clips and the rate measurement behind its current one.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ChannelClock {
    /// `lf+0x10`: the wire value last set, toggle included.
    anim: i32,
    current: Play,
    weight: Ramp,
    /// Earlier clips still blending out.
    fading: [Option<(Play, Ramp)>; FADES],
    /// The engine the clock imitates, as the last [`update_body`] named it.
    flavour: Flavour,
    /// 1.5's and UO's sync groups (a gunner's turret anims) a blend took
    /// to weight 0, at the node time their last weighted step left: the
    /// group's rows keep their weights, so its record lives on and a later
    /// set plays on from that time (cod11-turrets.md 13.10).
    held: [Option<Play>; HELD],
    /// `lf+0x2c` and `lf+0x1c`: the snapshot time and `pos.trBase` the rate
    /// was last measured from; 0 is unset, as the zeroed lerp frame reads.
    old_ms: i32,
    old_pos: Vec3,
    /// `lf+0x28`: the rate last measured, which every update hands the
    /// current clip.
    rate: f32,
    /// The torso channel's: the two nodes its setter weights.
    nodes: Nodes,
    /// How far ahead of the node times [`Self::layers`] and
    /// [`Self::node_weights`] read every weight ([`Self::set_weight_lead`]).
    lead_ms: i32,
}

impl ChannelClock {
    /// Reads every weight `ms` ahead of the node times: the frame after an
    /// end frame sets a goal, 1.7's server reads its weight a step on and
    /// its new clip at node time 0 (0.5005 into a 100 ms blend,
    /// cod4-player-anims.md 3), so `ms` is `level.frametime`.
    pub fn set_weight_lead(&mut self, ms: i32) {
        self.lead_ms = ms;
    }

    /// A clock already on `anim` at full weight, started at `start_ms` and
    /// playing at rate 1: what a channel reads with no movement anim on it.
    pub fn started(anim: i32, start_ms: i32) -> ChannelClock {
        ChannelClock {
            anim,
            current: Play {
                index: anim & !TOGGLE,
                base_ms: start_ms,
                base_s: 0.0,
                rate: 1.0,
                time_ms: start_ms,
                ..Play::default()
            },
            weight: Ramp::new(1.0, 1.0, start_ms, 0.0),
            rate: 1.0,
            ..ChannelClock::default()
        }
    }

    /// The current clip is a sync group's node stepping at `frequency` from
    /// `now_ms` on: the weighted mean its leaves' weights gave at this end
    /// frame (`turretpose::GunnerPlacement::frequency`), which the next
    /// frame's step reads (cod11-turrets.md 13.9); `looping` is the node's
    /// `loopsync`. A clip other than `index` is left alone.
    pub fn set_sync_frequency(&mut self, index: i32, now_ms: i32, frequency: f32, looping: bool) {
        if self.current.index != index & !TOGGLE {
            return;
        }
        self.current.settle(now_ms, self.flavour);
        self.current.frequency = frequency;
        self.current.looping = looping;
        self.current.sync = true;
    }

    /// The ramp a KnobAll leaves on a node beside the death's path that
    /// read `w` at `now_ms`.
    fn knob_cleared(&self, w: f32, now_ms: i32) -> Ramp {
        match self.flavour.cod2_tree() {
            true => Ramp::new(0.0, 0.0, now_ms, 0.0),
            false => Ramp::cleared_next_step(w, now_ms),
        }
    }

    /// The wire value last set, toggle included (`lf+0x10`).
    pub fn anim(&self) -> i32 {
        self.anim
    }

    /// The current clip's time in seconds at level time `now_ms`.
    pub fn seconds(&self, now_ms: i32) -> f32 {
        self.current.seconds(now_ms)
    }

    /// The time in seconds at `now_ms` of the layer playing `wire` (toggle
    /// ignored), 0 when none does: the time a gunner's placement reads his
    /// turret group's leaves at (cod11-turrets.md 7.2).
    pub fn seconds_of(&self, wire: i32, now_ms: i32) -> f32 {
        let index = wire & !TOGGLE;
        self.layers(now_ms)
            .iter()
            .find(|l| l.index == index)
            .map_or(0.0, |l| l.seconds)
    }

    /// The rate the current clip plays at.
    pub fn rate(&self) -> f32 {
        self.current.rate
    }

    /// The torso clock's `torso` and `legs` node weights at `now_ms`; a legs
    /// clock reads the defaults.
    pub fn node_weights(&self, now_ms: i32) -> NodeWeights {
        let lead = now_ms.wrapping_add(self.lead_ms);
        NodeWeights {
            torso: self.nodes.torso.at(lead),
            legs: self.nodes.legs.at(lead),
        }
    }

    /// Every clip with weight at `now_ms`, those blending out first and the
    /// current one last.
    pub fn layers(&self, now_ms: i32) -> Vec<Layer> {
        let layer = |p: &Play, w: &Ramp| Layer {
            index: p.index,
            seconds: p.seconds(now_ms),
            time: (p.frequency > 0.0).then(|| p.time(now_ms, self.flavour)),
            weight: w.at(now_ms.wrapping_add(self.lead_ms)),
        };
        self.fading
            .iter()
            .flatten()
            .map(|(p, w)| layer(p, w))
            .chain(std::iter::once(layer(&self.current, &self.weight)))
            .filter(|l| l.index != 0 && l.weight > 0.0)
            .collect()
    }

    /// One channel's update alone, without [`update_body`]'s clears and
    /// hand-offs between the two lerp frames.
    #[cfg(test)]
    fn update(&mut self, anims: &PlayerAnims, legs: bool, wire: i32, seen: &Seen, at: Times) {
        self.flavour = Flavour::V1_1;
        self.update_in(anims, legs, wire, seen, at, &mut 0, None);
    }

    /// [`Self::update`] under 1.5's and UO's server setter.
    #[cfg(test)]
    fn update_windowed(
        &mut self,
        anims: &PlayerAnims,
        legs: bool,
        wire: i32,
        seen: &Seen,
        at: Times,
        window: &mut i32,
    ) {
        self.flavour = Flavour::Server15;
        self.update_in(anims, legs, wire, seen, at, window, None);
    }

    /// 0x2aba0 (and the cgame's 0x30003be0) for one channel: start the clip
    /// when `wire` differs from the last one set, then, when the snapshot
    /// time moved, measure the rate off how far `seen.tr_base`, the entity's
    /// snapped origin, moved since the last measurement. `legs` picks the
    /// legs lerp frame, whose setter blends by stance (0x2a6ff).
    fn update_in(
        &mut self,
        anims: &PlayerAnims,
        legs: bool,
        wire: i32,
        seen: &Seen,
        at: Times,
        window: &mut i32,
        shared: Option<(Play, Ramp)>,
    ) -> Touches {
        let f = self.flavour;
        self.current.settle(at.clock_ms, f);
        for (p, w) in self.fading.iter_mut().flatten() {
            // The step moves no time on a node its blend has taken to 0.
            p.settle(at.clock_ms.min(w.last_weighted()), f);
        }
        let index = wire & !TOGGLE;
        let touches = if wire != self.anim {
            self.set(anims, wire, legs, seen, at.clock_ms, window, shared)
        } else {
            Touches::default()
        };
        if index == 0 {
            return touches;
        }
        let motion = anims.motion(index).unwrap_or_default();
        let now_ms = at.snapshot_ms;
        let tr_base = seen.tr_base;
        if motion.move_speed == 0 || self.old_ms == 0 {
            // 0x2ada5: rate 1, and the measurement starts here.
            self.rate = 1.0;
            self.old_ms = now_ms;
            self.old_pos = tr_base;
        } else if self.old_ms != now_ms && f.stored() {
            let ms = now_ms.wrapping_sub(self.old_ms);
            self.rate = rate_cod2(&motion, self.old_pos, tr_base, ms);
            self.old_ms = now_ms;
            self.old_pos = tr_base;
        } else if self.old_ms != now_ms {
            // 0x2ac58..0x2ac9d: distance over seconds over `moveSpeed`.
            let dist = if motion.vertical {
                (self.old_pos.z - tr_base.z).abs()
            } else {
                self.old_pos.distance(tr_base)
            };
            let secs = now_ms.wrapping_sub(self.old_ms) as f32 * 0.001;
            let speed = if f == Flavour::Cod4 {
                motion.move_speed_f
            } else {
                motion.move_speed as f32
            };
            let mut rate = dist / secs / speed;
            if rate < 0.1 {
                // 0x2acba..0x2acf6: a ladder anim stops under 0.01.
                rate = if rate < 0.01 && motion.vertical {
                    0.0
                } else {
                    0.1
                };
            } else if rate > 2.0 {
                // 0x2acf8..0x2ad61: a fast clip may run less over.
                let cap = if motion.vertical {
                    4.0
                } else if motion.move_speed > 150 {
                    2.0
                } else if motion.move_speed < 20 {
                    3.0
                } else {
                    3.0 - (motion.move_speed - 20) as f32 / 130.0
                };
                rate = rate.min(cap);
            }
            self.rate = rate;
            self.old_ms = now_ms;
            self.old_pos = tr_base;
        }
        // `trap_XAnimSetAnimRate` with `lf+0x28` on every call (0x2adef,
        // cgame 0x30003e2e), so a clip the setter just started takes the last
        // measured rate even on a frame that measured nothing.
        if self.current.rate != self.rate {
            self.set_rate(at.clock_ms, self.rate);
        }
        touches
    }

    /// The setter 0x2a600: the old clip blends out and the new one in over
    /// [`blend_ms`], the new one restarting at 0 or, for a looping movement
    /// clip, at the phase of the looping movement clip it replaces, else at
    /// one the level time and the client number spread out (0x2a858..0x2a919,
    /// `trap_XAnimSetTime` 0x2aa5a; 1.5's and UO's [`spread_phase_15`]). A death anim on a body not freshly
    /// cloned goes straight to its last frame at full weight, every other
    /// clip on the channel dropped (0x2a97b..0x2aa5a, cgame 0x3000395d..0x300039f5).
    fn set(
        &mut self,
        anims: &PlayerAnims,
        wire: i32,
        legs: bool,
        seen: &Seen,
        now_ms: i32,
        window: &mut i32,
        shared: Option<(Play, Ramp)>,
    ) -> Touches {
        let f = self.flavour;
        let windowed = f.windowed();
        let motion = |i: i32| anims.motion(i).filter(|_| i != 0);
        let (old_index, new_index) = (self.anim & !TOGGLE, wire & !TOGGLE);
        let (old, new) = (motion(old_index), motion(new_index));
        if let Some(n) = new.filter(|n| n.death && seen.eflags & EF_FRESH_CORPSE == 0) {
            // KnobAll clears every clip beside the death's path at goal time
            // 0, which the next frame's step lands ([`Ramp::cleared_next_step`]).
            let old: Vec<(Play, Ramp)> = self
                .fading
                .iter()
                .flatten()
                .copied()
                .chain(std::iter::once((self.current, self.weight)))
                .filter(|(p, w)| p.index != 0 && p.index != new_index && w.at(now_ms) > 0.0)
                .filter(|_| !f.cod2_tree())
                .map(|(p, w)| (p, Ramp::cleared_next_step(w.at(now_ms), now_ms)))
                .collect();
            self.fading = [None; FADES];
            for (slot, f) in self.fading.iter_mut().zip(old) {
                *slot = Some(f);
            }
            self.anim = wire;
            // The death arm's `trap_XAnimSetTime(.., 1.0)`.
            self.current = Play {
                index: new_index,
                base_ms: now_ms,
                base_s: n.duration,
                rate: 1.0,
                time_ms: now_ms,
                time_t: 1.0,
                frequency: n.frequency,
                looping: n.looping,
                sync: false,
            };
            self.weight = Ramp::new(1.0, 1.0, now_ms, 0.0);
            return Touches {
                knob_all: Some(new_index),
                ..Touches::default()
            };
        }
        let client_num = seen.client_num;
        // The windowed setter is 1.5's and UO's, whose spread differs.
        let spread = |now_ms, client_num| {
            let period = old.map_or(1000, |o| o.length_ms + 200);
            match f {
                Flavour::V1_1 => spread_phase(now_ms, client_num),
                Flavour::Cgame15 => spread_phase_15_cgame(now_ms, client_num, period),
                _ => spread_phase_15(now_ms, client_num, period),
            }
        };
        // The phase in seconds' terms and as the old node's float time
        // (`trap_XAnimGetTime`).
        let (phase, phase_t) = match new {
            Some(n) if n.move_speed != 0 && n.looping => match old {
                Some(o) if o.move_speed != 0 && o.looping && o.duration > 0.0 => (
                    (self.current.seconds(now_ms) / o.duration).rem_euclid(1.0),
                    self.current.time(now_ms, f),
                ),
                _ => {
                    let p = spread(now_ms, client_num);
                    (p, p)
                }
            },
            _ => (0.0, 0.0),
        };
        let keeps = windowed && !RESTART_WEIGHTED.with(|c| c.get());
        let blend = match windowed {
            false => blend_ms(anims, old_index, new_index, legs),
            true => blend_ms_windowed(anims, old_index, new_index, legs, now_ms, window),
        } as f32;
        // `XAnimClearGoalWeight` (0x080bf53c) sends the old node to 0 over
        // the whole blend time; the clips already fading keep their own.
        let old_weight = self.weight.at(now_ms);
        let mut fading: Vec<(Play, Ramp)> = self
            .fading
            .iter()
            .flatten()
            .copied()
            .chain(std::iter::once((
                self.current,
                match f.stored() {
                    true => Ramp::cod2(
                        old_weight,
                        self.weight.to,
                        0.0,
                        now_ms,
                        blend,
                        self.weight.cod2_left(now_ms),
                    ),
                    false => Ramp::new(old_weight, 0.0, now_ms, blend).stored_on(windowed),
                },
            )))
            .filter(|(p, w)| p.index != 0 && w.at(now_ms) > 0.0)
            .collect();
        if windowed {
            self.hold_faded(now_ms);
        }
        // A node already blending out comes back from the weight it had, in
        // the part of the blend time that distance takes; one the other lerp
        // frame holds is the same node, at its weight there.
        let back = fading
            .iter()
            .position(|(p, _)| p.index == new_index)
            .map(|i| fading.remove(i))
            .or(shared.filter(|(p, w)| p.index == new_index && w.at(now_ms) > 0.0));
        let from = back.map_or(0.0, |(_, w)| w.at(now_ms));
        fading.sort_by(|a, b| b.1.at(now_ms).total_cmp(&a.1.at(now_ms)));
        fading.truncate(FADES);
        self.fading = [None; FADES];
        for (slot, f) in self.fading.iter_mut().zip(fading) {
            *slot = Some(f);
        }
        self.anim = wire;
        // 1.5's and UO's setter (1.5b 0x28ced, UO 0x2ce5f) sets the phase
        // only on a node whose weight read 0, and restarts a weighted one
        // only from the torso (a fresh corpse's death never), so a weighted
        // legs clip plays on from where it stands (cod11-combat.md 3.1.5).
        let (phase, phase_t) = if keeps && back.is_some() {
            (0.0, 0.0)
        } else {
            (phase, phase_t)
        };
        let held = match keeps && back.is_none() {
            true => self.take_held(new_index),
            false => None,
        };
        self.current = match back {
            Some((p, _)) if keeps && (legs || new.is_some_and(|n| n.death)) => Play {
                base_ms: now_ms,
                base_s: p.seconds(now_ms),
                rate: 1.0,
                ..p
            },
            None if let Some(p) = held => Play {
                base_ms: now_ms,
                base_s: 0.0,
                rate: 1.0,
                time_ms: now_ms,
                ..p
            },
            _ => Play {
                index: new_index,
                base_ms: now_ms,
                base_s: phase * new.map_or(0.0, |n| n.duration),
                // The goal weight call's own rate; the updater sets the real one.
                rate: 1.0,
                time_ms: now_ms,
                time_t: phase_t,
                frequency: new.map_or(0.0, |n| n.frequency),
                looping: new.is_some_and(|n| n.looping),
                sync: false,
            },
        };
        self.weight = match f.stored() {
            true => Ramp::cod2(from, 0.0, 1.0, now_ms, blend, 0.0),
            false => Ramp::raised(from, 1.0, now_ms, (1.0 - from) * blend).stored_on(windowed),
        };
        if !legs {
            let (torso, legs) = if new_index == 0 {
                (0.0, 1.0)
            } else {
                (1.0, LEGS_UNDER_TORSO)
            };
            self.nodes = Nodes {
                torso: goal(self.nodes.torso, torso, now_ms, blend, f.stored()),
                legs: goal(self.nodes.legs, legs, now_ms, blend, f.stored()),
            };
        }
        Touches {
            cleared: (old_index != new_index && old_index != 0).then_some((old_index, blend)),
            raised: (new_index != 0).then_some((new_index, self.weight)),
            knob_all: None,
        }
    }

    /// Keeps each sync group the blends have taken to 0 by `now_ms` in
    /// [`Self::held`], its time frozen at the last frame its weight read
    /// above 0: the step moves a node's time only while it has weight
    /// (UO 0x80bd884, 1.5b 0x80c84f0).
    fn hold_faded(&mut self, now_ms: i32) {
        for (p, w) in self.fading.iter().flatten().copied() {
            if !p.sync || p.index == 0 || w.at(now_ms) > 0.0 {
                continue;
            }
            let last = w.last_weighted();
            let frozen = Play {
                time_t: p.time(last, self.flavour),
                time_ms: last,
                ..p
            };
            match self
                .held
                .iter_mut()
                .find(|h| h.is_some_and(|h| h.index == p.index))
            {
                Some(slot) => *slot = Some(frozen),
                None => match self.held.iter_mut().find(|h| h.is_none()) {
                    Some(slot) => *slot = Some(frozen),
                    None => self.held[0] = Some(frozen),
                },
            }
        }
    }

    /// The held sync group on `index`, taken out of [`Self::held`].
    fn take_held(&mut self, index: i32) -> Option<Play> {
        self.held
            .iter_mut()
            .find(|h| h.is_some_and(|h| h.index == index))
            .and_then(Option::take)
    }

    /// 0x30003e50 (1.5 0x300040b0, UO 0x300042c0; the server's
    /// `BG_PlayerAnimation` 0x2c25d..0x2c287) ahead of the channel's update:
    /// a lerp frame whose anim's node reads weight 0 is zeroed, so the
    /// update sets the wire anim again as if nothing had played. Only the
    /// other channel's setter takes a node this lerp frame holds to 0
    /// ([`ChannelClock::touched`]); a goal weight raises a zero weight to
    /// `0.001 * goal`, so a node this channel set never reads 0.
    fn clear_unweighted(&mut self, now_ms: i32) {
        if self.anim & !TOGGLE != 0 && self.weight.to == 0.0 && self.weight.at(now_ms) == 0.0 {
            self.anim = 0;
            self.current = Play::default();
            self.weight = Ramp::default();
        }
    }

    /// The other channel's setter touched nodes this lerp frame may hold:
    /// both channels' anims live in one tree, so a `both` line's node is
    /// one node under the two lerp frames, and a death's
    /// `XAnimSetCompleteGoalWeightKnobAll` under `root` zeroes every node
    /// beside its path at the next frame's step.
    fn touched(&mut self, anims: &PlayerAnims, t: &Touches, now_ms: i32) {
        let index = self.current.index;
        if let Some((cleared, blend)) = t.cleared
            && cleared == index
        {
            // `XAnimClearGoalWeight` 0x080bf53c: to 0 over the whole time,
            // or the sooner of the two when it was already going there.
            let w = self.weight.at(now_ms);
            if self.flavour.stored() {
                let left = self.weight.cod2_left(now_ms);
                self.weight = Ramp::cod2(w, self.weight.to, 0.0, now_ms, blend, left);
            } else {
                let ramp = Ramp::new(w, 0.0, now_ms, blend);
                let end = |r: &Ramp| r.start_ms as f32 + r.ms;
                if self.weight.to != 0.0 || end(&ramp) < end(&self.weight) {
                    self.weight = ramp;
                }
            }
        }
        if let Some((raised, ramp)) = t.raised
            && raised == index
        {
            self.weight = ramp;
        }
        if let Some(death) = t.knob_all {
            let path = ancestry(anims, death);
            let cut = |i: i32| {
                anims.index.node_id(i).is_some_and(|n| {
                    !path.contains(&n) && parent(anims, n).is_some_and(|p| path.contains(&p))
                })
            };
            if cut(index) {
                self.weight = self.knob_cleared(self.weight.at(now_ms), now_ms);
            }
            if self.flavour.cod2_tree() {
                for f in self.fading.iter_mut() {
                    if f.is_some_and(|(p, _)| cut(p.index)) {
                        *f = None;
                    }
                }
            }
            for (p, w) in self.fading.iter_mut().flatten() {
                if cut(p.index) {
                    *w = Ramp::cleared_next_step(w.at(now_ms), now_ms);
                }
            }
        }
    }

    /// The clip and weight of `index` while it has weight here, current or
    /// blending out.
    fn holding(&self, index: i32, now_ms: i32) -> Option<(Play, Ramp)> {
        self.fading
            .iter()
            .flatten()
            .copied()
            .chain(std::iter::once((self.current, self.weight)))
            .find(|(p, w)| p.index == index && w.at(now_ms) > 0.0)
    }

    /// What `own`'s setter finds of `wire`'s node in `other`'s lerp frame
    /// when it starts it: both channels' anims live in one tree, so a
    /// `both` line's node the other channel holds, or still blends out,
    /// is the node the setter raises, from its weight there.
    fn shared(own: &Self, other: &Self, wire: i32, now_ms: i32) -> Option<(Play, Ramp)> {
        let index = wire & !TOGGLE;
        if own.flavour.cod2_tree()
            || wire == own.anim
            || index == 0
            || own.holding(index, now_ms).is_some()
        {
            return None;
        }
        other.holding(index, now_ms)
    }

    /// Drops this lerp frame's blending-out entry of `index`: the other
    /// channel raised that node, which its own clock now carries.
    fn release(&mut self, index: i32) {
        for f in self.fading.iter_mut() {
            if f.is_some_and(|(p, _)| p.index == index) {
                *f = None;
            }
        }
    }

    /// The torso clock's `main` children after a KnobAll under `root` on
    /// `death`: a node on its path at 1 at once, one beside it at 0 from the
    /// next frame's step ([`Ramp::cleared_next_step`]).
    fn knob_nodes(&mut self, anims: &PlayerAnims, death: i32, now_ms: i32) {
        let path = ancestry(anims, death);
        let at = |name: &str, ramp: Ramp| {
            let Some(n) = anims.tree.index_of(name) else {
                return ramp;
            };
            if path.contains(&n) {
                Ramp::new(1.0, 1.0, now_ms, 0.0)
            } else if parent(anims, n).is_some_and(|p| path.contains(&p)) {
                self.knob_cleared(ramp.at(now_ms), now_ms)
            } else {
                ramp
            }
        };
        self.nodes = Nodes {
            torso: at("torso", self.nodes.torso),
            legs: at("legs", self.nodes.legs),
        };
    }

    /// Holds every clip and weight where it stands for `ms`: the cgame
    /// advances a body's tree only while its entity is in the newest
    /// snapshot (0x3001bd00), so a body back in view resumes where it left.
    pub fn pause(&mut self, ms: i32) {
        let shift = |t: &mut i32| *t = t.wrapping_add(ms);
        shift(&mut self.current.base_ms);
        shift(&mut self.current.time_ms);
        shift(&mut self.weight.start_ms);
        for (p, w) in self.fading.iter_mut().flatten() {
            shift(&mut p.base_ms);
            shift(&mut p.time_ms);
            shift(&mut w.start_ms);
        }
        shift(&mut self.nodes.torso.start_ms);
        shift(&mut self.nodes.legs.start_ms);
    }

    fn set_rate(&mut self, now_ms: i32, rate: f32) {
        self.current.settle(now_ms, self.flavour);
        self.current.base_s = self.current.seconds(now_ms);
        self.current.base_ms = now_ms;
        self.current.rate = rate;
    }
}

/// `cod2_lnxded` 1.3's rate (0x80da52f..0x80da6ec), from `old` to `new`
/// over `ms`: the distance (0x80a40ea: the difference stored, its squares
/// summed on the stack and stored, libm `sqrt` stored), over `ms * 0.001`
/// on the stack, stored, over the float `moveSpeed`, stored; the clamps
/// read the float `moveSpeed`, the cap `3 - (moveSpeed - 20) / 130` on the
/// stack, stored. cod2-player-anims.md 7.
fn rate_cod2(motion: &AnimMotion, old: Vec3, new: Vec3, ms: i32) -> f32 {
    use crate::x87::F80;
    let x = F80::from_f32;
    let dist = if motion.vertical {
        (old.z - new.z).abs()
    } else {
        let d = new - old;
        let len2 = (x(d.x) * x(d.x) + x(d.y) * x(d.y) + x(d.z) * x(d.z)).to_f32();
        f64::from(len2).sqrt() as f32
    };
    let secs = F80::from_f64(f64::from(ms)) * x(0.001);
    let speed = motion.move_speed_f;
    let rate = (x((x(dist) / secs).to_f32()) / x(speed)).to_f32();
    if rate < 0.1 {
        return if rate < 0.01 && motion.vertical {
            0.0
        } else {
            0.1
        };
    }
    if rate <= 2.0 {
        return rate;
    }
    let cap = if motion.vertical {
        4.0
    } else if speed > 150.0 {
        2.0
    } else if speed < 20.0 {
        3.0
    } else {
        (x(3.0) - (x(speed) - x(20.0)) / x(130.0)).to_f32()
    };
    rate.min(cap)
}

/// What a setter did to nodes the other channel's lerp frame may hold.
#[derive(Clone, Copy, Debug, Default)]
struct Touches {
    /// The old anim and the blend its node clears over.
    cleared: Option<(i32, f32)>,
    /// The new anim and its node's weight ramp.
    raised: Option<(i32, Ramp)>,
    /// A death anim loaded under `root` with `KnobAll` (0x2a9fe, cgame
    /// 0x300039e1).
    knob_all: Option<i32>,
}

/// One body's two lerp frames as `BG_PlayerAnimation` (`game.mp.i386.so`
/// 0x2c1f4) and the cgame's 0x30004e40 step them: each lerp frame whose
/// node reads weight 0 cleared, then the legs and the torso updated, each
/// setter's touch on the shared tree passed to the other lerp frame, both
/// under `flavour`'s engine. `window` is the client's end of the stance
/// stretch the windowed setter keeps (`ci+0x4a4` 1.5b, `ci+0x4c0` UO,
/// `ci+0x4ac` CoD2), shared by both channels, the legs first; 1.1 reads
/// none.
pub fn update_body(
    (legs, torso): (&mut ChannelClock, &mut ChannelClock),
    anims: &PlayerAnims,
    (legs_wire, torso_wire): (i32, i32),
    seen: &Seen,
    at: Times,
    flavour: Flavour,
    window: &mut i32,
) {
    let now_ms = at.clock_ms;
    legs.flavour = flavour;
    torso.flavour = flavour;
    legs.clear_unweighted(now_ms);
    torso.clear_unweighted(now_ms);
    let shared = ChannelClock::shared(legs, torso, legs_wire, now_ms);
    let t = legs.update_in(anims, true, legs_wire, seen, at, window, shared);
    if shared.is_some() {
        torso.release(legs_wire & !TOGGLE);
    }
    torso.touched(anims, &t, now_ms);
    if let Some(death) = t.knob_all {
        torso.knob_nodes(anims, death, now_ms);
    }
    // A node both lerp frames hold is one node: the time a setter gives it
    // is the other lerp frame's too.
    if t.raised.is_some_and(|(i, _)| i == torso.current.index) && !flavour.cod2_tree() {
        torso.current = legs.current;
    }
    let shared = ChannelClock::shared(torso, legs, torso_wire, now_ms);
    let t = torso.update_in(anims, false, torso_wire, seen, at, window, shared);
    if shared.is_some() {
        legs.release(torso_wire & !TOGGLE);
    }
    legs.touched(anims, &t, now_ms);
    if let Some(death) = t.knob_all {
        torso.knob_nodes(anims, death, now_ms);
    }
    // The torso's update sets the shared node's rate last.
    if legs.current.index == torso.current.index && legs.current.index != 0 && !flavour.cod2_tree()
    {
        legs.current = torso.current;
    }
}

/// The two lerp frames' clips as the tree holds them, legs then torso: each
/// under the `main` child (`legs` or `torso`) its anim sits under in the
/// tree, whichever lerp frame set it, and a node both carry once, at the
/// copy a lerp frame holds as its current clip (the torso's when both do).
/// CoD2's and CoD4's bodies keep each lerp frame's clips under its own node
/// ([`Flavour::cod2_tree`]). cod11-movers.md 15.8.
pub fn body_layers(
    legs: &ChannelClock,
    torso: &ChannelClock,
    anims: &PlayerAnims,
    now_ms: i32,
) -> (Vec<Layer>, Vec<Layer>) {
    let (mut l, mut t) = (legs.layers(now_ms), torso.layers(now_ms));
    let Some(node) = anims
        .tree
        .index_of("torso")
        .filter(|_| !torso.flavour.cod2_tree())
    else {
        return (l, t);
    };
    // The tree numbers its nodes depth first, so a subtree is a range.
    let mut last = node;
    while let Some(&c) = anims.tree.nodes[last].children.last() {
        last = c;
    }
    // A clip the tree does not place stays on its lerp frame's node.
    let under_torso = |i: i32| {
        anims
            .index
            .node_id(i)
            .is_none_or(|n| (node..=last).contains(&n))
    };
    let current = |c: &ChannelClock, i: i32| c.current.index == i;
    let mut moved = Vec::new();
    t.retain(|x| {
        under_torso(x.index) || {
            moved.push(*x);
            false
        }
    });
    for x in moved {
        match l.iter_mut().find(|y| y.index == x.index) {
            Some(y) if current(torso, x.index) || !current(legs, x.index) => *y = x,
            Some(_) => {}
            None => l.push(x),
        }
    }
    (l, t)
}

/// The tree nodes from `wire`'s up to the root, its own first.
fn ancestry(anims: &PlayerAnims, wire: i32) -> Vec<usize> {
    let mut path = Vec::new();
    let mut n = anims.index.node_id(wire);
    while let Some(id) = n {
        path.push(id);
        n = parent(anims, id);
    }
    path
}

fn parent(anims: &PlayerAnims, id: usize) -> Option<usize> {
    anims
        .tree
        .nodes
        .iter()
        .position(|n| n.children.contains(&id))
}

/// `XAnimSetGoalWeight` (0x080bd674) on a node: from wherever `ramp` stands
/// to `to` over `|to - weight| * blend_ms`.
fn goal(ramp: Ramp, to: f32, now_ms: i32, blend_ms: f32, cod2: bool) -> Ramp {
    let from = ramp.at(now_ms);
    if cod2 {
        return Ramp::cod2(from, ramp.to, to, now_ms, blend_ms, ramp.cod2_left(now_ms));
    }
    Ramp::raised(from, to, now_ms, (to - from).abs() * blend_ms)
}

/// `lf->animationTime`, the blend a new anim gets (0x2a698..0x2a855). The
/// legs blend in at once from no anim at all; a stance change takes 200 ms
/// into a movement or turn clip and 250 into a still one; otherwise a
/// movement clip takes 120 ms, and a still one 250 out of a movement clip
/// and 170 out of anything else. Going to no anim is never under 200.
fn blend_ms(anims: &PlayerAnims, old: i32, new: i32, legs: bool) -> i32 {
    let motion =
        |i: i32| -> Option<AnimMotion> { (i != 0).then(|| anims.motion(i).unwrap_or_default()) };
    let (o, n) = (motion(old), motion(new));
    if o.is_none() && legs {
        return 0;
    }
    // The anim's own `+0x40` wins when it is set (0x2a6ab, cgame 0x3000374c).
    if let Some(b) = n.map(|n| n.blend_ms).filter(|&b| b > 0) {
        return b;
    }
    let still = |o: Option<AnimMotion>| {
        if o.is_some_and(|o| o.move_speed != 0) {
            250
        } else {
            170
        }
    };
    let Some(n) = n else {
        return still(o).max(200);
    };
    // The record's movetype word (`animation_t +0x54`): the crouched and
    // prone classes (0xc4, 0x308), and the two turn movetypes (0xc000).
    let crouched = movetype_bit("idlecr") | movetype_bit("walkcr") | movetype_bit("walkcrbk");
    let prone = movetype_bit("idleprone") | movetype_bit("walkprone") | movetype_bit("walkpronebk");
    let turn = movetype_bit("turnright") | movetype_bit("turnleft");
    let class = |i: i32| {
        let m = anims.record(i).movetypes;
        (m & crouched != 0, m & prone != 0)
    };
    if legs && class(old) != class(new) {
        if n.move_speed != 0 || anims.record(new).movetypes & turn != 0 {
            200
        } else {
            250
        }
    } else if n.move_speed != 0 {
        120
    } else {
        still(o)
    }
}

/// The blend of 1.5's and UO's setter (1.5b 0x28ced, UO 0x2ce5f): the
/// anim's own, else 170 into a still clip, 250 into one out of a movement
/// clip and 120 into a movement clip, 200 into no anim, and never under
/// what is left of `window`. A legs change of crouch or prone class opens a
/// new 400 ms one, which the torso's change in the same end frame reads
/// too. The legs blend in at once from no anim at all.
fn blend_ms_windowed(
    anims: &PlayerAnims,
    old: i32,
    new: i32,
    legs: bool,
    now_ms: i32,
    window: &mut i32,
) -> i32 {
    let record = |i: i32| (i != 0).then(|| anims.motion(i).unwrap_or_default());
    let (o, n) = (record(old), record(new));
    if n.is_some() && legs {
        let crouched = movetype_bit("idlecr") | movetype_bit("walkcr") | movetype_bit("walkcrbk");
        let prone =
            movetype_bit("idleprone") | movetype_bit("walkprone") | movetype_bit("walkpronebk");
        // CoD2 counts its crouched stumbles crouched too (lnxded 0x80d9c02:
        // `+0x58 & 0x180`), so an anim a crouched stumble block also
        // plays, `pb_stumble_forward` among them, opens the window from a
        // standing idle (cod2-combat.md 9.3).
        let crouched_hi =
            movetype_bit_hi("stumble_crouch_forward") | movetype_bit_hi("stumble_crouch_backward");
        let class = |i: i32| {
            let r = anims.record(i);
            (
                r.movetypes & crouched != 0 || r.movetypes_hi & crouched_hi != 0,
                r.movetypes & prone != 0,
            )
        };
        if class(old) != class(new) {
            *window = now_ms.wrapping_add(400);
        }
    }
    if o.is_none() && legs {
        return 0;
    }
    let own = n.map_or(200, |n| n.blend_ms);
    let rule = match n {
        Some(n) if n.blend_ms >= 1 => -1,
        Some(n) if n.move_speed != 0 => 120,
        _ if o.is_some_and(|o| o.move_speed != 0) => 250,
        _ => 170,
    };
    own.max(rule.max(window.wrapping_sub(now_ms)))
}

/// `((bg.time / 100 + clientNum * 360) % 1000) * 0.001`, the normalized time
/// a movement clip starts at when nothing it replaces gives it one.
fn spread_phase(now_ms: i32, client_num: i32) -> f32 {
    ((now_ms / 100 + client_num * 360) % 1000) as f32 * 0.001
}

/// 1.5b's and UO's spread (`game.mp` 1.5b 0x18fa2..0x19036, UO
/// 0x1d114..0x1d1a8): `clientNum * 0.36f + (bg.time % period) / period`
/// stored as a float, less its integer part. `period` is the old anim's
/// `XAnimGetLength` + 200, or 1000 when the old anim is no clip (none).
/// The sum runs on the x87 stack; `f64` here.
fn spread_phase_15(now_ms: i32, client_num: i32, period: i32) -> f32 {
    let x = ((now_ms % period) as f64 / period as f64 + client_num as f64 * 0.36f32 as f64) as f32;
    x - x.trunc()
}

/// The same spread in the 1.5 and UO cgames (1.5 0x30003acc..0x30003af6,
/// UO 0x30003cce..0x30003cf8): the sum stays on the x87 stack, `_ftol`
/// takes its integer part there, and only the fraction is stored, so it
/// rounds to a float once where the server's rounds the sum first.
fn spread_phase_15_cgame(now_ms: i32, client_num: i32, period: i32) -> f32 {
    let x = (now_ms % period) as f64 / period as f64 + client_num as f64 * 0.36f32 as f64;
    (x - x.trunc()) as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A default clock settled at the end of time takes no step walk: rate
    /// 0 leaves the time where it is on the first step.
    #[test]
    fn a_default_play_settles_at_once() {
        let mut p = Play::default();
        let start = std::time::Instant::now();
        p.settle(i32::MAX, Flavour::Server15);
        assert_eq!(p.time_t.to_bits(), 0f32.to_bits());
        assert_eq!(p.time_ms, i32::MAX / STEP_MS * STEP_MS);
        let held = Play {
            rate: 1.0,
            frequency: 1.0,
            time_t: 1.0,
            ..Play::default()
        };
        assert_eq!(held.time(i32::MAX, Flavour::Server15), 1.0);
        assert!(start.elapsed() < std::time::Duration::from_millis(50));
    }

    /// `cod2_lnxded`'s rates for `pb_combatrun_left_loop` (`moveSpeed`
    /// 0x43363ead) off three snapped steps of 50 ms, read under gdb at the
    /// legs lerp frame's +0x28 (cod2-player-anims.md 7). The seconds kept
    /// on the stack move the first and the last a word from a stored
    /// `0.05`'s.
    #[test]
    fn a_cod2_rate_divides_by_the_seconds_on_the_stack() {
        let motion = AnimMotion {
            move_speed_f: f32::from_bits(0x4336_3ead),
            ..AnimMotion::default()
        };
        let at = |x: f32, y: f32| Vec3::new(x, y, 64.0);
        let steps = [
            (at(1940.0, 1771.0), at(1937.0, 1771.0), 0x3ea8_907f),
            (at(1937.0, 1771.0), at(1933.0, 1771.0), 0x3ee0_c0ab),
            (at(1933.0, 1771.0), at(1922.0, 1772.0), 0x3f9b_2793),
        ];
        for (old, new, rate) in steps {
            assert_eq!(rate_cod2(&motion, old, new, 50).to_bits(), rate);
        }
    }

    #[test]
    fn the_node_time_sums_float_frame_steps_and_wraps() {
        let mut p = Play {
            rate: 1.3,
            frequency: 30.0 / 29.0,
            looping: true,
            time_ms: 1000,
            ..Play::default()
        };
        let d = 0.05f32 as f64 * 1.3f32 as f64 * (30.0f32 / 29.0) as f64;
        let f = Flavour::Server15;
        let mut t = 0.0f32;
        for k in 1..40 {
            t = (t as f64 + d) as f32;
            if t >= 1.0 {
                t -= 1.0;
            }
            // A step lands only on a whole frame.
            assert_eq!(p.time(1000 + 50 * k + 49, f).to_bits(), t.to_bits(), "{k}");
        }
        let at = p.time(2950, f);
        p.settle(2960, f);
        assert_eq!((p.time_ms, p.time(2999, f)), (2950, at));
        p.looping = false;
        assert_eq!(p.time(9000, f), 1.0);
    }

    #[test]
    fn a_cod2_node_set_from_0_takes_the_goal_time_off_its_raised_weight() {
        // The weights `cod2_lnxded` stepped after a strafe's clip set at
        // 10200 (120 ms) and an idle set out of a run at 10450 (250 ms),
        // read under gdb (cod2-player-anims.md 7).
        let words = |blend: f32, n: i32| -> Vec<u32> {
            let r = Ramp::cod2(0.0, 0.0, 1.0, 0, blend, 0.0);
            (0..n).map(|i| r.at(i * STEP_MS).to_bits()).collect()
        };
        assert_eq!(
            words(120.0, 4),
            [0x3a83_126f, 0x3ed5_d867, 0x3f55_96de, 0x3f80_0000]
        );
        assert_eq!(
            words(250.0, 6),
            [
                0x3a83_126f,
                0x3e4d_d2f2,
                0x3ecd_4fdf,
                0x3f19_db23,
                0x3f4d_0e56,
                0x3f80_0000
            ]
        );
        // Out to 0 from a full weight: the whole time.
        let out = Ramp::cod2(1.0, 1.0, 0.0, 0, 120.0, 0.0);
        let w: Vec<u32> = (0..4).map(|i| out.at(i * STEP_MS).to_bits()).collect();
        assert_eq!(w, [0x3f80_0000, 0x3f15_5556, 0x3e2a_aaae, 0]);
    }

    #[test]
    fn a_cod2_step_stores_its_increment() {
        // `pb_stand_alert` (100 frames at 30 fps): retail's 17th step,
        // read under gdb (cod2-player-anims.md 7), against the stack sum's.
        let play = Play {
            rate: 1.0,
            frequency: 30.0 / 99.0,
            looping: true,
            ..Play::default()
        };
        let (mut a, mut b) = (0.0f32, 0.0f32);
        for _ in 0..17 {
            a = play.step(a, Flavour::Cod2);
            b = play.step(b, Flavour::Server15);
        }
        assert_eq!((a.to_bits(), b.to_bits()), (0x3e83_e0f8, 0x3e83_e0f9));
    }

    #[test]
    fn a_sync_group_steps_its_mean_frequency_with_a_stored_increment() {
        let mut c = ChannelClock::started(95, 0);
        // Another clip's frequency leaves this one alone.
        c.set_sync_frequency(608, 100, 6.0, true);
        assert_eq!(c.layers(400)[0].time, None);
        let f = 5.9999995f32;
        c.set_sync_frequency(95, 100, f, true);
        // Steps land at 150, 200 and 250, each adding the stored product.
        let inc = (f64::from(0.05f32) * f64::from(f)) as f32;
        let mut t = 0.0f32;
        for _ in 0..3 {
            t = (f64::from(t) + f64::from(inc)) as f32;
        }
        assert_eq!(c.layers(299)[0].time.map(f32::to_bits), Some(t.to_bits()));
        // A loopsync group wraps.
        assert!(c.layers(1000)[0].time.unwrap() < 1.0);
    }

    #[test]
    fn the_spread_phase_steps_a_tenth_of_a_second_and_a_client() {
        assert_eq!(spread_phase(12_345, 0), 0.123);
        assert_eq!(spread_phase(12_345, 1), 0.483);
        assert_eq!(
            spread_phase(99_999, 3),
            (999 + 1080) as f32 % 1000.0 * 0.001
        );
    }

    #[test]
    fn the_15_spread_phase_runs_on_the_old_clips_period() {
        // Values the sum gives rounded to a float once, off exact rationals.
        let cases = [
            (12_345, 0, 1000, 0x3eb0a3d7),
            (12_345, 3, 1000, 0x3ed9999c),
            (25_050, 5, 1466, 0x3f6326ea),
            (99_999, 63, 2133, 0x3f0fd5e0),
        ];
        for (now, client, period, bits) in cases {
            let p = spread_phase_15(now, client, period);
            assert_eq!(p.to_bits(), bits, "{now} {client} {period}: {p}");
        }
    }

    /// The cgames' fraction rounds once: past the first client the sum's
    /// own float rounding is gone, so the two forms part by an ulp of the
    /// sum while agreeing where the sum is below 1.
    #[test]
    fn the_cgame_spread_rounds_the_fraction_once() {
        assert_eq!(
            spread_phase_15_cgame(12_345, 0, 1000).to_bits(),
            spread_phase_15(12_345, 0, 1000).to_bits()
        );
        let (server, cgame) = (
            spread_phase_15(25_050, 5, 1466),
            spread_phase_15_cgame(25_050, 5, 1466),
        );
        let exact = (25_050 % 1466) as f64 / 1466.0 + 5.0 * 0.36f32 as f64;
        let exact = exact - exact.trunc();
        assert_ne!(server.to_bits(), cgame.to_bits());
        assert_eq!(cgame, exact as f32);
        assert!((f64::from(server) - exact).abs() < 1e-6);
    }

    #[test]
    fn a_rate_change_rebases_without_a_jump() {
        let mut c = ChannelClock::started(122, 1000);
        assert_eq!(c.seconds(1500), 0.5);
        c.set_rate(1500, 2.0);
        assert_eq!(c.seconds(1500), 0.5);
        assert_eq!(c.seconds(1550), 0.6);
    }

    #[test]
    fn a_weight_ramp_is_a_straight_line_to_its_goal() {
        let r = Ramp::new(0.25, 1.0, 1000, 200.0);
        assert_eq!(r.at(1000), 0.25);
        assert_eq!(r.at(1100), 0.625);
        assert_eq!(r.at(1300), 1.0);
        assert_eq!(Ramp::new(0.0, 1.0, 1000, 0.0).at(1000), 1.0);
        // The setter raises a node at 0 to 0.001 of its goal, its time the
        // whole distance's; at once with no blend.
        let r = Ramp::raised(0.0, 1.0, 1000, 200.0);
        assert_eq!(r.at(1000), 0.001);
        assert!((r.at(1100) - 0.5005).abs() < 1e-6);
        assert_eq!(r.at(1200), 1.0);
        assert_eq!(Ramp::raised(0.0, 1.0, 1000, 0.0).at(1000), 1.0);
    }

    /// The first anim a stock event clause names on `torso` or the legs.
    fn event_anim(anims: &PlayerAnims, event: &str, torso: bool) -> i32 {
        let block = anims.script.event(event).expect(event);
        let name = block
            .clauses
            .iter()
            .flat_map(|c| if torso { &c.torso } else { &c.legs })
            .next()
            .expect("an anim line")
            .name
            .clone();
        anims.wire_of(&name).expect(&name)
    }

    fn seen(x: f32, eflags: i32) -> Seen {
        Seen {
            client_num: 3,
            tr_base: Vec3::new(x, 0.0, 0.0),
            eflags,
        }
    }

    /// The cgame's update runs every drawn frame but measures only when the
    /// newest snapshot moved, and the clip's time runs on the drawn clock
    /// without a jump across the rate change.
    #[test]
    fn a_drawn_clock_measures_once_a_snapshot() {
        let Some(fs) = crate::testing::game_fs() else {
            return;
        };
        let anims = PlayerAnims::load(&fs).unwrap();
        let run = anims.wire_of("pb_combatrun_forward_loop").unwrap();
        let speed = anims.motion(run).unwrap().move_speed as f32;
        let mut c = ChannelClock::default();
        let mut last = None;
        // 10 units a snapshot is 200 u/s; snapshots every 50 ms, drawn
        // frames every 7, the newest snapshot 40 ms ahead of the drawn clock.
        for r in (10_000..10_500).step_by(7) {
            let snap = (r + 40) / 50 * 50;
            let at = Times {
                clock_ms: r,
                snapshot_ms: snap,
            };
            let before = c.seconds(r);
            c.update(&anims, true, run, &seen(snap as f32 * 0.2, 0), at);
            if let Some(prev) = last {
                // The rate change rebases on this frame's clock.
                assert_eq!(c.seconds(r), before, "{r}");
                assert!(c.seconds(r) > prev);
            }
            last = Some(c.seconds(r));
        }
        assert_eq!(c.rate(), 200.0 / speed);
        // A drawn frame with no newer snapshot keeps the rate.
        c.update(
            &anims,
            true,
            run,
            &seen(0.0, 0),
            Times {
                clock_ms: 10_500,
                snapshot_ms: 10_500,
            },
        );
        c.update(
            &anims,
            true,
            run,
            &seen(500.0, 0),
            Times {
                clock_ms: 10_507,
                snapshot_ms: 10_500,
            },
        );
        assert_eq!(c.rate(), 200.0 / speed);
    }

    /// A run out of the idle starts at the spread phase: 1.1's off the
    /// tenth of a second, 1.5's and UO's off the idle's own length.
    #[test]
    fn a_run_out_of_the_idle_starts_at_the_protocols_spread() {
        let Some(fs) = crate::testing::game_fs() else {
            return;
        };
        let anims = PlayerAnims::load(&fs).unwrap();
        let idle = anims.wire_of("pb_stand_alert").unwrap();
        let run = anims.wire_of("pb_combatrun_forward_loop").unwrap();
        let period = anims.motion(idle).unwrap().length_ms + 200;
        for windowed in [false, true] {
            let mut c = ChannelClock::default();
            let mut window = 0;
            for (t, wire) in [(1000, idle), (12_350, run)] {
                let at = Times::end_frame(t);
                match windowed {
                    false => c.update(&anims, true, wire, &seen(0.0, 0), at),
                    true => c.update_windowed(&anims, true, wire, &seen(0.0, 0), at, &mut window),
                }
            }
            let want = match windowed {
                false => spread_phase(12_350, 3),
                true => spread_phase_15(12_350, 3, period),
            };
            assert_eq!(c.current.time_t, want, "{windowed}");
        }
    }

    /// A death anim plays from its start on a body `cloneplayer` just made
    /// and shows its last frame, alone, on any other.
    #[test]
    fn a_death_anim_starts_only_on_a_fresh_body() {
        let Some(fs) = crate::testing::game_fs() else {
            return;
        };
        let anims = PlayerAnims::load(&fs).unwrap();
        let death = event_anim(&anims, "death", false);
        let m = anims.motion(death).unwrap();
        assert!(m.death && m.move_speed == 0 && !m.looping);
        let idle = anims.wire_of("pb_stand_alert").unwrap();
        for (eflags, start, layers) in [(EF_FRESH_CORPSE, 0.0, 2), (0, m.duration, 1)] {
            let mut c = ChannelClock::default();
            c.update(
                &anims,
                true,
                idle,
                &seen(0.0, eflags),
                Times::end_frame(1000),
            );
            c.update(
                &anims,
                true,
                death,
                &seen(0.0, eflags),
                Times::end_frame(1050),
            );
            // Halfway through the fresh body's 170 ms blend out of the idle.
            let l = c.layers(1135);
            assert_eq!(l.len(), layers);
            assert!((l.last().unwrap().seconds - start - 0.085).abs() < 1e-4);
        }
    }

    /// A torso anim sends the torso node to 1 and the legs node to 0.01 over
    /// its blend, each from where it stood; clearing it sends them back.
    #[test]
    fn a_torso_anim_weights_the_torso_node_over_the_legs() {
        let Some(fs) = crate::testing::game_fs() else {
            return;
        };
        let anims = PlayerAnims::load(&fs).unwrap();
        let fire = event_anim(&anims, "fireweapon", true);
        let mut c = ChannelClock::default();
        let at = |t| Times::end_frame(t);
        c.update(&anims, false, 0, &seen(0.0, 0), at(1000));
        assert_eq!(c.node_weights(1000), NodeWeights::default());
        c.update(&anims, false, fire, &seen(0.0, 0), at(1000));
        // The torso node starts from the setter's 0.001.
        let w = c.node_weights(1015);
        assert!(
            (w.torso - 0.5005).abs() < 1e-6 && (w.legs - 0.5).abs() < 1e-6,
            "{w:?}"
        );
        assert_eq!(
            c.node_weights(1030),
            NodeWeights {
                torso: 1.0,
                legs: LEGS_UNDER_TORSO
            }
        );
        // Back to no anim halfway: the torso node from 1 over the whole
        // blend, the legs node from 0.01 over 0.99 of it.
        c.update(&anims, false, 0, &seen(0.0, 0), at(2000));
        let blend = blend_ms(&anims, fire, 0, false) as f32;
        assert_eq!(blend % 2.0, 0.0);
        let w = c.node_weights(2000 + (blend / 2.0) as i32);
        assert!((w.torso - 0.5).abs() < 1e-6, "{w:?}");
        assert!((w.legs - 0.51).abs() < 1e-6, "{w:?}");
    }

    /// 1.5's and UO's setter: a legs change into a crouch opens a 400 ms
    /// window, and a fire anim the torso starts inside it blends in over
    /// what is left of it instead of its own 30 ms (UO 0x2ce5f).
    #[test]
    fn a_crouch_stretches_the_next_blends_to_400_ms() {
        let Some(fs) = crate::testing::game_fs() else {
            return;
        };
        let anims = PlayerAnims::load(&fs).unwrap();
        let (stand, crouch) = (
            anims.wire_of("pb_stand_alert").unwrap(),
            anims.wire_of("pb_crouch_alert").unwrap(),
        );
        let fire = event_anim(&anims, "fireweapon", true);
        let at = |t| Times::end_frame(t);
        let weight = |c: &ChannelClock, t| c.layers(t).last().unwrap().weight;
        let (mut legs, mut torso, mut window) =
            (ChannelClock::default(), ChannelClock::default(), 0);
        legs.update_windowed(&anims, true, stand, &seen(0.0, 0), at(1000), &mut window);
        legs.update_windowed(&anims, true, crouch, &seen(0.0, 0), at(2000), &mut window);
        assert_eq!(window, 2400);
        torso.update_windowed(&anims, false, fire, &seen(0.0, 0), at(2100), &mut window);
        assert!((weight(&torso, 2250) - 0.5005).abs() < 1e-6);
        // 1.1's setter keeps the fire anim's own blend.
        let mut plain = ChannelClock::default();
        plain.update(&anims, false, fire, &seen(0.0, 0), at(2100));
        assert_eq!(weight(&plain, 2130), 1.0);
    }

    /// 1.5's and UO's setter keeps the time of a legs node that still has
    /// weight, where 1.1's restarts it at the spread phase; the torso's
    /// restarts it at 0 (1.5b 0x28ced, UO 0x2ce5f).
    #[test]
    fn a_weighted_node_keeps_its_time_on_15() {
        let Some(fs) = crate::testing::game_fs() else {
            return;
        };
        let anims = PlayerAnims::load(&fs).unwrap();
        let (run, stand) = (
            anims.wire_of("pb_combatrun_forward_loop").unwrap(),
            anims.wire_of("pb_stand_alert").unwrap(),
        );
        let at = |t| Times::end_frame(t);
        let time_of = |c: &ChannelClock, i: i32, t| {
            c.layers(t)
                .iter()
                .find(|l| l.index == i)
                .and_then(|l| l.time)
                .unwrap()
        };
        let back = |windowed: bool, legs: bool| {
            let (mut c, mut window) = (ChannelClock::default(), 0);
            let mut set = |c: &mut ChannelClock, wire, t| {
                if windowed {
                    c.update_windowed(&anims, legs, wire, &seen(0.0, 0), at(t), &mut window);
                } else {
                    c.update(&anims, legs, wire, &seen(0.0, 0), at(t));
                }
            };
            set(&mut c, run, 1000);
            set(&mut c, stand, 2000);
            // The run is still blending out.
            let fading = time_of(&c, run, 2100);
            set(&mut c, run, 2100);
            (fading, time_of(&c, run, 2100))
        };
        let (fading, kept) = back(true, true);
        assert!(fading > 0.0);
        assert_eq!(kept.to_bits(), fading.to_bits());
        let (fading, spread) = back(false, true);
        assert_eq!(spread, spread_phase(2100, 3));
        assert_ne!(spread, fading);
        assert_eq!(back(true, false).1, 0.0);
    }

    /// A `FIREWEAPON` anim carries its own 30 ms blend (0x2888d).
    #[test]
    fn a_fire_anim_blends_in_over_30_ms() {
        let Some(fs) = crate::testing::game_fs() else {
            return;
        };
        let anims = PlayerAnims::load(&fs).unwrap();
        let fire = event_anim(&anims, "fireweapon", true);
        assert_eq!(anims.motion(fire).unwrap().blend_ms, 30);
        assert_eq!(blend_ms(&anims, 0, fire, false), 30);
    }

    /// A death shown at its last frame loads under `root` with `KnobAll`
    /// (cgame 0x300039e1, `cod_lnxded` 0x080bf8ac): the legs node comes
    /// back to 1 at once, and the clear of the torso node beside it at goal
    /// time 0 keeps its weight (0x080bd35c) until the next frame's step.
    /// A torso setter in the same update sets the node's goal again, its
    /// time off that kept weight (0x080bd674), so a torso going to no anim
    /// with the death fades the node over its blend.
    #[test]
    fn a_shown_death_clears_the_torso_node_at_the_next_step() {
        let Some(fs) = crate::testing::game_fs() else {
            return;
        };
        let anims = PlayerAnims::load(&fs).unwrap();
        let (idle, fire) = (
            anims.wire_of("pb_stand_alert").unwrap(),
            event_anim(&anims, "fireweapon", true),
        );
        let death = event_anim(&anims, "death", false);
        let step = |l: &mut ChannelClock, t: &mut ChannelClock, w, eflags, ms| {
            update_body(
                (l, t),
                &anims,
                w,
                &seen(0.0, eflags),
                Times::end_frame(ms),
                Flavour::V1_1,
                &mut 0,
            )
        };
        // The torso keeps its fire anim: the clear lands at the next step.
        let (mut legs, mut torso) = (ChannelClock::default(), ChannelClock::default());
        step(&mut legs, &mut torso, (idle, fire), 0, 1000);
        assert_eq!(torso.node_weights(1100).torso, 1.0);
        step(&mut legs, &mut torso, (death, fire), 0, 2000);
        let set = torso.node_weights(2000);
        assert_eq!((set.torso, set.legs), (1.0, 1.0));
        // The idle beside the death still reads its weight in the set's frame.
        assert!(legs.layers(2000).iter().any(|l| l.index == idle & !TOGGLE));
        assert_eq!(torso.node_weights(2001), NodeWeights::default());
        let after: Vec<i32> = legs.layers(2050).iter().map(|l| l.index).collect();
        assert_eq!(after, [death & !TOGGLE]);
        // The torso drops its anim with the death: its own blend.
        let (mut legs, mut torso) = (ChannelClock::default(), ChannelClock::default());
        step(&mut legs, &mut torso, (idle, fire), 0, 1000);
        step(&mut legs, &mut torso, (death, TOGGLE), 0, 2000);
        let blend = blend_ms(&anims, fire, 0, false);
        let mid = torso.node_weights(2000 + blend / 2);
        assert!((mid.torso - 0.5).abs() < 1e-3, "{mid:?}");
        assert_eq!(mid.legs, 1.0);
        assert_eq!(torso.node_weights(2000 + blend), NodeWeights::default());
    }

    /// A `both` line puts one node under the two lerp frames. The torso
    /// leaving it clears the node over its blend; once the node reads 0 the
    /// legs lerp frame is cleared (0x30003e50) and the next update starts
    /// the clip again from its start.
    #[test]
    fn a_shared_node_cleared_by_the_torso_restarts_the_legs_clip() {
        let Some(fs) = crate::testing::game_fs() else {
            return;
        };
        let anims = PlayerAnims::load(&fs).unwrap();
        let throw = anims.wire_of("pb_stand_grenade_throw").unwrap();
        assert_eq!(anims.motion(throw).unwrap().move_speed, 0);
        let (mut legs, mut torso) = (ChannelClock::default(), ChannelClock::default());
        let mut step = |w, ms| {
            update_body(
                (&mut legs, &mut torso),
                &anims,
                w,
                &seen(0.0, 0),
                Times::end_frame(ms),
                Flavour::V1_1,
                &mut 0,
            );
            (legs.layers(ms), legs.seconds(ms))
        };
        step((throw, throw), 1000);
        let blend = blend_ms(&anims, throw, 0, false);
        let (l, _) = step((throw, 0), 1100);
        assert_eq!(l.last().unwrap().weight, 1.0);
        let (l, secs) = step((throw, 0), 1100 + blend / 2);
        assert!((l.last().unwrap().weight - 0.5).abs() < 1e-3, "{l:?}");
        assert!((secs - (100 + blend / 2) as f32 * 0.001).abs() < 1e-4);
        // At 0 the next update clears the lerp frame and sets it again.
        let (l, secs) = step((throw, 0), 1100 + blend + 10);
        assert_eq!(secs, 0.0);
        assert_eq!(l.last().unwrap().weight, 1.0);
    }

    /// A node the legs still blend out is the node a `both` line's torso
    /// raises: the torso's setter takes it from its weight there, not from
    /// 0, and the legs lerp frame no longer carries it.
    #[test]
    fn a_node_the_other_channel_fades_is_raised_from_its_weight() {
        let Some(fs) = crate::testing::game_fs() else {
            return;
        };
        let anims = PlayerAnims::load(&fs).unwrap();
        let throw = anims.wire_of("pb_stand_grenade_throw").unwrap();
        let idle = anims.wire_of("pb_stand_alert").unwrap();
        let (mut legs, mut torso) = (ChannelClock::default(), ChannelClock::default());
        let mut step = |w, ms| {
            update_body(
                (&mut legs, &mut torso),
                &anims,
                w,
                &seen(0.0, 0),
                Times::end_frame(ms),
                Flavour::V1_1,
                &mut 0,
            );
            (legs.layers(ms), torso.layers(ms))
        };
        step((throw, 0), 1000);
        step((idle, 0), 1100);
        let blend = blend_ms(&anims, throw, idle, true);
        let mid = 1100 + blend / 2;
        let (l, t) = step((idle, throw), mid);
        let fading = 1.0 - (blend / 2) as f32 / blend as f32;
        let raised = t.iter().find(|x| x.index == throw).unwrap().weight;
        assert!((raised - fading).abs() < 1e-3, "{t:?}");
        assert!(l.iter().all(|x| x.index != throw), "{l:?}");
    }

    /// A `both` line's node is one node under the two lerp frames: it
    /// poses once, under `legs` where the tree has it, and a restart from
    /// either lerp frame moves the time the other carries.
    #[test]
    fn a_node_both_lerp_frames_hold_poses_once_under_its_own_node() {
        let Some(fs) = crate::testing::game_fs() else {
            return;
        };
        let anims = PlayerAnims::load(&fs).unwrap();
        let throw = anims.wire_of("pb_stand_grenade_throw").unwrap();
        let idle = anims.wire_of("pb_stand_alert").unwrap();
        let fire = anims.wire_of("pt_stand_shoot").unwrap();
        let (mut legs, mut torso) = (ChannelClock::default(), ChannelClock::default());
        let mut step = |w, ms| {
            update_body(
                (&mut legs, &mut torso),
                &anims,
                w,
                &seen(0.0, 0),
                Times::end_frame(ms),
                Flavour::V1_1,
                &mut 0,
            );
            body_layers(&legs, &torso, &anims, ms)
        };
        step((idle, fire), 1000);
        let (l, t) = step((throw, throw), 1100);
        assert_eq!(l.iter().filter(|x| x.index == throw).count(), 1, "{l:?}");
        assert!(t.iter().all(|x| x.index == fire), "{t:?}");
        // The legs restart the node; the torso's copy restarts with it.
        step((throw, throw), 1600);
        let (l, _) = step((throw ^ TOGGLE, throw), 1700);
        assert_eq!(legs.seconds(1750), torso.seconds(1750));
        assert!(legs.seconds(1750) < 0.1, "{}", legs.seconds(1750));
        assert_eq!(l.iter().filter(|x| x.index == throw).count(), 1, "{l:?}");
    }

    /// A paused clock's clips and ramps stand still for the pause.
    #[test]
    fn a_pause_holds_the_clips_where_they_stood() {
        let mut c = ChannelClock::started(122, 1000);
        c.weight = Ramp::new(0.0, 1.0, 1000, 200.0);
        c.pause(5000);
        assert_eq!(c.seconds(6100), 0.1);
        assert_eq!(c.layers(6100)[0].weight, 0.5);
    }
}
