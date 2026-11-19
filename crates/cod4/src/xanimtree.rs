//! A cgame anim tree's clock and weights as `iw3mp.exe` steps them, for a
//! tree of one root over leaves (`XAnimBlend`, 0x5810c0, flags 0), the
//! shape the view weapon's is (cod4-player-anims.md 12): each live leaf's
//! `XAnimState` (info +0x18..+0x34), `XAnimSetGoalWeight` (0x584fd0,
//! 0x584a80), the restart (0x5827d0), the set time (0x584e90), the
//! finished test (0x583b20) and the frame's step (0x583bf0: the weights
//! 0x582860, then the times 0x582560 and 0x5822c0). The x87 runs at double
//! precision: an `f64` between the binary's stores. [`FlatTree::tree`]
//! hands the walk ([`crate::xanimcalc`]) its leaves.

use crate::skel::{d, f};
use crate::xanimcalc::{Parts, Tree};

/// The float 0.001 widened (0x70af98): a weight starting from 0 starts at
/// this share of its goal.
const START_SHARE: f64 = 0.001_000_000_047_497_451_3;
/// Under it a goal weight is 0 (0x70b278).
const MIN_GOAL: f32 = 0.001;
/// Under it an easing weight jumps to [`START_SHARE`] of its goal
/// (0x70b230).
const MIN_WEIGHT: f32 = 1.000_000_1e-6;
/// The last time a clip that does not loop holds short of its end
/// (0x70b610, the double at 0x70b618 its value).
const NEAR_END: f32 = 0.999_999_9;

/// One leaf's `XAnimState`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct LeafState {
    /// +0x18: the node time, 0 to 1.
    pub time: f32,
    /// +0x1c: the time before this frame's step.
    pub old_time: f32,
    /// +0x20, +0x22: the loops run, before and after the step.
    pub cycle: i16,
    pub old_cycle: i16,
    /// +0x24: seconds left to reach the goal.
    pub goal_time: f32,
    /// +0x28.
    pub goal_weight: f32,
    /// +0x2c.
    pub weight: f32,
    /// +0x30: the clip's speed.
    pub rate: f32,
}

/// One info under the root: its node and state. A restart with a blend
/// leaves the old info fading out beside a fresh one (`ghost`), which the
/// lookup (0x583a20) no longer finds.
#[derive(Clone, Debug, PartialEq)]
struct Info {
    node: usize,
    state: LeafState,
    ghost: bool,
}

/// What the time step reads of a leaf's clip (`XAnimParts`).
#[derive(Clone, Copy, Debug)]
pub struct Clock {
    /// +0x2c: node time a second at rate 1.
    pub frequency: f32,
    /// +0x10.
    pub looped: bool,
}

impl Clock {
    pub fn of(p: &Parts) -> Clock {
        Clock {
            frequency: p.frequency,
            looped: p.looped,
        }
    }
}

/// The root's live children, in node order.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FlatTree {
    infos: Vec<Info>,
}

impl FlatTree {
    fn find(&self, node: usize) -> Option<usize> {
        self.infos.iter().position(|i| i.node == node && !i.ghost)
    }

    /// Node `node`'s state, `None` while it has no info.
    pub fn state(&self, node: usize) -> Option<&LeafState> {
        self.find(node).map(|i| &self.infos[i].state)
    }

    /// `XAnimSetGoalWeight` (0x584fd0): node `node` toward `weight` over
    /// `goal_time` seconds at `rate`, `restart` taking it back to its start
    /// first. A goal under 0.001 is 0; a node with no info and a goal of 0
    /// stays without one.
    pub fn set_goal_weight(
        &mut self,
        node: usize,
        weight: f32,
        goal_time: f32,
        rate: f32,
        restart: bool,
    ) {
        let weight = if MIN_GOAL > weight { 0.0 } else { weight };
        let i = match self.find(node) {
            Some(i) if restart => self.restart(i, goal_time),
            Some(i) => i,
            None if weight == 0.0 => return,
            None => {
                let at = self.infos.partition_point(|i| i.node <= node);
                self.infos.insert(
                    at,
                    Info {
                        node,
                        state: LeafState::default(),
                        ghost: false,
                    },
                );
                at
            }
        };
        goal(&mut self.infos[i].state, weight, goal_time, rate);
    }

    /// The restart (0x5827d0): a node at its start, or one that looped this
    /// frame, stays; one with no blend or no weight goes back to 0 (0x582690);
    /// else the old info fades out over `goal_time` (0x584470) beside a
    /// fresh one, whose index comes back.
    fn restart(&mut self, i: usize, goal_time: f32) -> usize {
        let s = self.infos[i].state;
        if (s.time == 0.0 && s.cycle == 0) || s.cycle != s.old_cycle {
            return i;
        }
        if goal_time == 0.0 || s.weight == 0.0 {
            let s = &mut self.infos[i].state;
            (s.time, s.old_time, s.cycle, s.old_cycle) = (0.0, 0.0, 0, 0);
            return i;
        }
        let mut old = self.infos[i].clone();
        old.ghost = true;
        old.state.goal_time = goal_time;
        old.state.goal_weight = 0.0;
        self.infos[i].state = LeafState::default();
        self.infos.insert(i + 1, old);
        i
    }

    /// 0x584e90: node `node`'s time and the time before it `t`, its loops
    /// none.
    pub fn set_time(&mut self, node: usize, t: f32) {
        if let Some(i) = self.find(node) {
            let s = &mut self.infos[i].state;
            (s.time, s.old_time, s.cycle, s.old_cycle) = (t, t, 0, 0);
        }
    }

    /// 0x583b20: a node with no info, or whose time wrapped, reached 1 or
    /// looped in the last step.
    pub fn finished(&self, node: usize) -> bool {
        self.state(node)
            .is_none_or(|s| s.old_time > s.time || s.time == 1.0 || s.cycle > s.old_cycle)
    }

    /// One frame's step of `dt` seconds (0x583bf0): every weight toward its
    /// goal, then every live leaf's time by its rate and `clock`'s frequency;
    /// a leaf at weight 0 with a goal of 0 goes.
    pub fn update(&mut self, dt: f32, clock: impl Fn(usize) -> Clock) {
        for i in &mut self.infos {
            let s = &mut i.state;
            // 0x582899..0x5828ec.
            if s.goal_time > dt {
                s.weight =
                    f((d(s.goal_weight) - d(s.weight)) * d(dt) / d(s.goal_time) + d(s.weight));
                if s.weight < MIN_WEIGHT {
                    s.weight = f(d(s.goal_weight) * START_SHARE);
                }
                s.goal_time = f(d(s.goal_time) - d(dt));
            } else {
                s.weight = s.goal_weight;
                s.goal_time = 0.0;
            }
            // A leaf of no weight or a clip of no frequency takes the
            // root's clock, at 0 (0x5829f6).
            if s.weight == 0.0 || clock(i.node).frequency == 0.0 {
                (s.time, s.cycle) = (0.0, 0);
            }
            (s.old_time, s.old_cycle) = (s.time, s.cycle);
        }
        // 0x581910: a leaf of no weight and no goal is freed.
        self.infos
            .retain(|i| i.state.weight != 0.0 || i.state.goal_weight != 0.0);
        for i in &mut self.infos {
            let s = &mut i.state;
            if s.weight == 0.0 {
                continue;
            }
            let c = clock(i.node);
            step_time(s, c, dt);
        }
    }

    /// The root over each info's leaf for the walk: `parts` and `remap` of
    /// a node, `None` for a node the tree draws nothing of.
    pub fn tree<'a>(
        &self,
        leaf: impl Fn(usize) -> Option<(&'a Parts, &'a [Option<usize>])>,
    ) -> Tree<'a> {
        Tree::Node {
            weight: 1.0,
            children: self
                .infos
                .iter()
                .filter_map(|i| {
                    let (parts, remap) = leaf(i.node)?;
                    Some(Tree::Leaf {
                        clip: parts,
                        remap,
                        time: i.state.time,
                        weight: i.state.weight,
                    })
                })
                .collect(),
        }
    }
}

/// 0x584a80 on a leaf: the goal and rate set, the seconds to reach it
/// scaled by how far the weight has to go.
fn goal(s: &mut LeafState, weight: f32, goal_time: f32, rate: f32) {
    if goal_time == 0.0 {
        s.weight = weight;
        s.goal_time = 0.0;
    } else {
        if s.weight == 0.0 {
            s.weight = f(START_SHARE * d(weight));
        }
        let diff = d(s.weight) - d(weight);
        let diff_f = f(diff);
        // The same side of the new goal as the old goal: its time anew
        // (0x584b32..0x584b44); else the shorter (0x409400).
        if diff_f == 0.0 || (d(s.goal_weight) - d(weight)) * d(diff_f) > 0.0 {
            s.goal_time = goal_time;
        } else {
            s.goal_time = s.goal_time.min(goal_time);
        }
        if weight != 0.0 {
            // 0x584b8e..0x584bca: at least the share of `goal_time` the
            // weight still has to go (0x4093c0).
            let share = match d(s.weight) > d(weight) {
                true => d(goal_time) * (diff / d(s.weight)),
                false => (d(weight) - d(s.weight)) / d(weight) * d(goal_time),
            };
            s.goal_time = s.goal_time.max(f(share));
        }
    }
    s.goal_weight = weight;
    s.rate = rate;
}

/// The most whole turns [`step_time`] takes off one at a time.
const LOOP_TURNS_MAX: f32 = 65536.0;

/// A leaf's time step (0x5822c0): `rate * frequency * dt` stored, added to
/// the time before the step; a looping clip wraps, one that does not holds
/// just short of 1 for a frame, then at 1.
fn step_time(s: &mut LeafState, c: Clock, dt: f32) {
    let step = f(d(s.rate) * d(c.frequency) * d(dt));
    // A zone's infinite or NaN frequency or rate: no step.
    if step == 0.0 || !step.is_finite() {
        return;
    }
    let mut t = f(d(step) + d(s.old_time));
    let mut cycle = s.cycle;
    if t >= 1.0 {
        if c.looped {
            // From 2^24 `t - 1` rounds back to `t`: a step past any clip's
            // drops its whole turns at once.
            if t >= LOOP_TURNS_MAX {
                cycle = cycle.wrapping_add(t.trunc() as i64 as i16);
                t = t.fract();
            }
            while t >= 1.0 {
                t = f(d(t) - 1.0);
                cycle = cycle.wrapping_add(1);
            }
        } else {
            let x = f(d(s.old_time) - d(NEAR_END));
            t = if x < 0.0 { NEAR_END } else { 1.0 };
        }
    }
    // 0x58238c..0x5823a8: a time set past the step keeps.
    if d(s.time) - d(t) > f64::from(cycle.wrapping_sub(s.cycle)) {
        return;
    }
    s.time = t;
    s.cycle = cycle;
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOOP: Clock = Clock {
        frequency: 0.5,
        looped: true,
    };
    const ONCE: Clock = Clock {
        frequency: 0.5,
        looped: false,
    };

    /// A goal over 0.1 s eases the weight in a tenth a 10 ms frame from
    /// 0.001 of it, the time stepping at the rate; the other node's goal 0
    /// with no time drops it at once, and it goes.
    #[test]
    fn a_goal_eases_and_a_dropped_node_goes() {
        let mut t = FlatTree::default();
        t.set_goal_weight(1, 1.0, 0.0, 1.0, false);
        t.update(0.01, |_| LOOP);
        assert_eq!(t.state(1).unwrap().weight, 1.0);
        t.set_goal_weight(2, 1.0, 0.1, 2.0, true);
        t.set_goal_weight(1, 0.0, 0.0, 1.0, false);
        t.update(0.01, |_| LOOP);
        assert!(t.state(1).is_none());
        let s = *t.state(2).unwrap();
        assert!((s.weight - 0.1009).abs() < 1e-6, "{s:?}");
        assert!((s.goal_time - 0.09).abs() < 1e-6, "{s:?}");
        assert!((s.time - 0.01).abs() < 1e-7, "{s:?}");
        // The float seconds left run a hair over a frame at the tenth.
        for _ in 0..9 {
            t.update(0.01, |_| LOOP);
        }
        assert_eq!(t.state(2).unwrap().weight, 0.9999999);
        t.update(0.01, |_| LOOP);
        assert_eq!(t.state(2).unwrap().weight, 1.0);
    }

    /// A zone's infinite, NaN or huge frequency steps a looping leaf at
    /// once (SEC-030).
    #[test]
    fn a_hostile_frequency_steps_at_once() {
        for frequency in [f32::INFINITY, f32::NAN, 1e12, 1.05e9, -1e12] {
            let mut t = FlatTree::default();
            t.set_goal_weight(1, 1.0, 0.0, 1.0, false);
            for _ in 0..3 {
                t.update(0.016, |_| Clock {
                    frequency,
                    looped: true,
                });
            }
            let s = t.state(1).unwrap();
            assert!(
                s.time.is_nan() || (0.0..1.0).contains(&s.time),
                "{frequency}: {s:?}"
            );
        }
    }

    /// A clip that does not loop holds short of 1 for a frame, then reads
    /// finished at 1; a looping one reads finished on the frame it wraps.
    #[test]
    fn a_clip_finishes_at_its_end_or_its_wrap() {
        let mut t = FlatTree::default();
        t.set_goal_weight(3, 1.0, 0.0, 1.0, true);
        let mut frames = 0;
        while !t.finished(3) {
            t.update(0.5, |_| ONCE);
            frames += 1;
        }
        assert_eq!((frames, t.state(3).unwrap().time), (5, 1.0));
        let mut t = FlatTree::default();
        t.set_goal_weight(1, 1.0, 0.0, 1.0, false);
        t.update(1.5, |_| LOOP);
        assert!(!t.finished(1));
        t.update(1.5, |_| LOOP);
        assert!(t.finished(1));
        assert_eq!(t.state(1).unwrap().cycle, 1);
    }

    /// A restart under a blend leaves the old info fading out under the
    /// root beside a fresh one at time 0.
    #[test]
    fn a_blended_restart_fades_the_old_out() {
        let mut t = FlatTree::default();
        t.set_goal_weight(1, 1.0, 0.0, 1.0, false);
        t.update(0.5, |_| LOOP);
        t.set_goal_weight(1, 1.0, 0.5, 1.0, true);
        let s = *t.state(1).unwrap();
        assert_eq!((s.time, s.goal_weight), (0.0, 1.0));
        assert_eq!(t.infos.len(), 2);
        assert!(t.infos[1].ghost && t.infos[1].state.goal_weight == 0.0);
        for _ in 0..6 {
            t.update(0.1, |_| LOOP);
        }
        assert_eq!(t.infos.len(), 1);
    }
}
