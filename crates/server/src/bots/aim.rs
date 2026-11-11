//! The aim a bot's hand puts on its target (bot-combat.md 18): a flick
//! with a bell-shaped speed that lands short or past and a smaller one
//! after it, a slow drift in place of a jitter, a trail behind a crossing
//! target read a beat late, recoil pulled back late and in part, and a
//! point on the upper body drawn per engagement.
//!
//! [`Hand`] holds one engagement's state; `Bot::engage` drives it. All
//! draws come off the bot's own generator, so a seeded match replays.

use super::{EnemyView, PITCH_MOST, Skill, yaw_diff};

/// Ticks of the target's track kept: twice the longest `track_lag`
/// (0.09 s), and one to interpolate past it.
const SEEN: usize = 5;
/// The drift's time constant, ticks, for each of its two stages.
const DRIFT_TICKS: f32 = 8.0;
/// A hand further off its want than this, degrees, flicks; nearer, it
/// closes [`PURSUIT`]'s share of the gap a tick.
const FLICK_MIN_DEG: f32 = 1.5;
/// Of the gap, the share a smooth correction leaves each tick.
const PURSUIT: f32 = 0.5;
/// A flick lands this share of its angle short, on the mean.
const FLICK_SHORT: f32 = 0.04;
/// The view read off a flick's course by more than this, degrees (a
/// respawn, a script's angles, a retreat's look): the flick is dropped.
const FLICK_RESET_DEG: f32 = 2.0;
/// The point on the body off the aimed point, in [`Skill::error_floor`]s:
/// across the line either way, and down or up (normal: 3 across, 3 down
/// to 6 up, in units).
const POINT_ACROSS: f32 = 0.75;
const POINT_LOW: f32 = -0.75;
const POINT_HIGH: f32 = 1.5;
/// Gun offsets kept for [`Skill::recoil_ticks`]: its most plus one.
pub(super) const RECOIL_KEEP: usize = 5;

/// One engagement's aim state.
#[derive(Clone, Debug)]
pub(super) struct Hand {
    /// The target as seen, newest first: origin and velocity.
    seen: [([f32; 3], [f32; 3]); SEEN],
    /// The drift's two stages: noise low-passed, then low-passed again.
    drift_in: [f32; 2],
    drift: [f32; 2],
    flick: Option<Flick>,
    /// Last tick's want and the bot's clock then, and the offset off it
    /// the hand was sent to.
    last: Option<([f32; 2], u32)>,
    off: [f32; 2],
    /// (across, up) units off the aimed point, for this engagement.
    point: [f32; 2],
}

/// A ballistic move of the hand's offset from `from` to `land`, degrees,
/// over `ticks` on the minimum-jerk curve; `tick` the ticks run.
#[derive(Clone, Copy, Debug)]
struct Flick {
    from: [f32; 2],
    land: [f32; 2],
    tick: u32,
    ticks: u32,
}

impl Flick {
    fn at(&self) -> [f32; 2] {
        let x = (self.tick as f32 / self.ticks as f32).min(1.0);
        let s = x * x * x * (10.0 - 15.0 * x + 6.0 * x * x);
        [0, 1].map(|i| self.from[i] + (self.land[i] - self.from[i]) * s)
    }
}

/// A pair of standard normals (Box-Muller) off the bot's generator.
pub(super) fn gauss2(rng: &mut u64) -> [f32; 2] {
    let mut u = || crate::game::host::rand_int(rng) as f32 / 2_147_483_648.0;
    let (u1, u2) = (1.0 - u(), u());
    let r = (-2.0 * u1.ln()).sqrt();
    let (s, c) = (std::f32::consts::TAU * u2).sin_cos();
    [r * c, r * s]
}

fn unit(rng: &mut u64) -> f32 {
    crate::game::host::rand_int(rng) as f32 / 2_147_483_648.0
}

fn len(v: [f32; 2]) -> f32 {
    v[0].hypot(v[1])
}

impl Hand {
    /// A fresh engagement on `e`, drawn off the generator with the bot's
    /// `error_floor`. `draw` off, nothing is drawn (the old aim's arm,
    /// whose draws must not move).
    pub(super) fn new(e: &EnemyView, draw: Option<(&mut u64, f32)>) -> Hand {
        let (dir, point) = match draw {
            Some((rng, floor)) => {
                let a = unit(rng) * std::f32::consts::TAU;
                let across = (unit(rng) * 2.0 - 1.0) * POINT_ACROSS * floor;
                let up = (POINT_LOW + unit(rng) * (POINT_HIGH - POINT_LOW)) * floor;
                ([a.sin(), a.cos()], [across, up])
            }
            None => ([0.0, 1.0], [0.0; 2]),
        };
        // Started at the first error in a random direction, as the old
        // aim's first tick was.
        let raw = dir.map(|d| d / drift_norm());
        Hand {
            seen: [(e.origin, e.velocity); SEEN],
            drift_in: raw,
            drift: raw,
            flick: None,
            last: None,
            off: [0.0; 2],
            point,
        }
    }

    /// One tick's sight of the target.
    pub(super) fn see(&mut self, e: &EnemyView) {
        self.seen.rotate_right(1);
        self.seen[0] = (e.origin, e.velocity);
    }

    /// Where the bot puts the target from `eye`: seen twice `track_lag`
    /// late and led on by half that along the velocity it had then, so a
    /// crossing target is trailed by his speed times `track_lag` and the
    /// aim runs as far past the spot where he turns. Plus this
    /// engagement's point on the body.
    pub(super) fn perceived(&self, eye: [f32; 3], track_lag: f32) -> [f32; 3] {
        let k = (2.0 * track_lag / 0.05).clamp(0.0, (SEEN - 1) as f32);
        let (i, f) = (k.floor() as usize, k.fract());
        let (a, b) = (self.seen[i], self.seen[(i + 1).min(SEEN - 1)]);
        let mix = |x: [f32; 3], y: [f32; 3]| [0, 1, 2].map(|j| x[j] + (y[j] - x[j]) * f);
        let (o, v, lead) = (mix(a.0, b.0), mix(a.1, b.1), track_lag);
        let (dx, dy) = (o[0] - eye[0], o[1] - eye[1]);
        let h = dx.hypot(dy).max(1.0);
        let side = [-dy / h, dx / h];
        [
            o[0] + v[0] * lead + side[0] * self.point[0],
            o[1] + v[1] * lead + side[1] * self.point[0],
            o[2] + v[2] * lead + self.point[1],
        ]
    }

    /// The drift one tick on: (pitch, yaw) in units of the error, 1 on the
    /// mean as the old aim's miss always was, changing smoothly.
    pub(super) fn drift(&mut self, rng: &mut u64) -> [f32; 2] {
        let a = drift_pole();
        let s = (1.0 - a * a).sqrt() * std::f32::consts::FRAC_1_SQRT_2;
        let n = gauss2(rng);
        for (i, n) in n.into_iter().enumerate() {
            self.drift_in[i] = a * self.drift_in[i] + s * n;
            self.drift[i] = a * self.drift[i] + (1.0 - a) * self.drift_in[i];
        }
        self.drift.map(|d| d * drift_norm())
    }

    /// The (pitch, yaw) the hand takes the view to this tick, on its way
    /// to `want`. `view` is where the view is; `clock` the bot's tick
    /// count, so a want older than last tick is not trusted. Off by more
    /// than [`FLICK_MIN_DEG`] the hand flicks: on the minimum-jerk curve,
    /// `turn_deg` a tick on the mean (1.875 times that at its peak), to a
    /// landing off by [`Skill::flick_spread`] of the angle; then flicks
    /// again or closes the rest smoothly.
    pub(super) fn steer(
        &mut self,
        view: [f32; 2],
        want: [f32; 2],
        clock: u32,
        skill: &Skill,
        rng: &mut u64,
    ) -> [f32; 2] {
        let off_from = |w: [f32; 2]| [yaw_diff(view[0], w[0]), yaw_diff(view[1], w[1])];
        // Off last tick's want: a hand that held its course reads the
        // offset it was sent to while the want moves on under it.
        let meas = match self.last.filter(|l| l.1.wrapping_add(1) == clock) {
            Some((w, _)) => off_from(w),
            None => {
                self.flick = None;
                off_from(want)
            }
        };
        if self.flick.is_some()
            && len([meas[0] - self.off[0], meas[1] - self.off[1]]) > FLICK_RESET_DEG
        {
            self.flick = None;
        }
        let off = if let Some(f) = self.flick.as_mut() {
            f.tick += 1;
            let o = f.at();
            if f.tick >= f.ticks {
                self.flick = None;
            }
            o
        } else if len(meas) > FLICK_MIN_DEG {
            let f = plan(meas, skill, rng);
            let o = f.at();
            self.flick = (f.tick < f.ticks).then_some(f);
            o
        } else {
            meas.map(|m| m * PURSUIT)
        };
        self.off = off;
        self.last = Some((want, clock));
        [
            (want[0] + off[0]).clamp(-PITCH_MOST, PITCH_MOST),
            want[1] + off[1],
        ]
    }
}

/// A flick from offset `from` toward none: lands [`FLICK_SHORT`] short on
/// the mean, spread by `flick_spread` of the angle along it and half that
/// across, in as many ticks as `turn_deg` a tick needs: one under it,
/// a bell-shaped run of ticks over it.
fn plan(from: [f32; 2], skill: &Skill, rng: &mut u64) -> Flick {
    let a = len(from);
    let u = [from[0] / a, from[1] / a];
    let n = gauss2(rng);
    let along = a * (FLICK_SHORT + skill.flick_spread * n[0]);
    let side = a * skill.flick_spread * 0.5 * n[1];
    let land = [u[0] * along - u[1] * side, u[1] * along + u[0] * side];
    let travel = len([land[0] - from[0], land[1] - from[1]]);
    let ticks = ((travel / skill.turn_deg).ceil() as u32).max(1);
    Flick {
        from,
        land,
        tick: 1,
        ticks,
    }
}

/// The drift's per-tick pole.
fn drift_pole() -> f32 {
    (-1.0 / DRIFT_TICKS).exp()
}

/// Scales the twice-filtered drift to a mean length of 1: two equal
/// one-pole stages of pole `a` over unit noise leave `(1 - a) * sqrt(1 +
/// a^2) / (1 - a^2)` of its root mean square, and a 2D normal's mean
/// length is `sqrt(pi) / 2` of that.
fn drift_norm() -> f32 {
    let a = drift_pole();
    (1.0 + a) / (1.0 + a * a).sqrt() * 2.0 / std::f32::consts::PI.sqrt()
}

/// The share of the gun's offset the view takes back: `recoil_share` of
/// it as it was `recoil_ticks` ago (`kept` newest first).
pub(super) fn pulled(kept: &[[f32; 2]; RECOIL_KEEP], skill: &Skill) -> [f32; 2] {
    let g = kept[(skill.recoil_ticks as usize).min(RECOIL_KEEP - 1)];
    g.map(|x| x * skill.recoil_share)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(origin: [f32; 3], velocity: [f32; 3]) -> EnemyView {
        EnemyView {
            slot: 3,
            origin,
            velocity,
        }
    }

    /// A large turn: the speed rises and falls (one peak, no step past
    /// 1.875 `turn_deg`), lands off the want, then corrects and settles
    /// on it.
    #[test]
    fn a_flick_is_bell_shaped_lands_off_and_corrects() {
        let skill = Skill::default();
        let mut landed_off = 0;
        for seed in 1..=40u64 {
            let mut rng = seed * 2 + 1;
            let mut hand = Hand::new(&e([0.0; 3], [0.0; 3]), None);
            let want = [0.0, 0.0];
            let mut view = [0.0, 90.0];
            let (mut first, mut primary) = (Vec::new(), None);
            for clock in 0..30 {
                let next = hand.steer(view, want, clock, &skill, &mut rng);
                let step = len([next[0] - view[0], yaw_diff(next[1], view[1])]);
                assert!(step <= 1.875 * skill.turn_deg + 0.01, "step {step}");
                view = next;
                let ticks = *primary.get_or_insert_with(|| hand.flick.map_or(1, |f| f.ticks));
                if first.len() < ticks as usize {
                    first.push(step);
                    if first.len() == ticks as usize && len(view) > 0.5 {
                        landed_off += 1;
                    }
                }
            }
            let peak = first.iter().copied().fold(0.0, f32::max);
            let top = first.iter().position(|&s| s == peak).unwrap();
            assert!(first[..=top].windows(2).all(|w| w[0] <= w[1]), "{first:?}");
            assert!(first[top..].windows(2).all(|w| w[0] >= w[1]), "{first:?}");
            assert!(first[0] < peak / 2.0, "no slow start: {first:?}");
            assert!(len(view) < 0.05, "seed {seed}: never settled: {view:?}");
        }
        assert!(landed_off > 30, "only {landed_off} of 40 flicks landed off");
    }

    /// The offset off the target changes a little a tick: never a jump
    /// to a new direction, as the old 4-8 tick resample made.
    #[test]
    fn the_drift_moves_smoothly() {
        let mut hand = Hand::new(&e([0.0; 3], [0.0; 3]), Some((&mut 7, 4.0)));
        let mut rng = 11;
        let mut prev = hand.drift(&mut rng);
        let (mut sum, mut most) = (0.0, 0.0f32);
        let n = 8000;
        for _ in 0..n {
            let d = hand.drift(&mut rng);
            most = most.max(len([d[0] - prev[0], d[1] - prev[1]]));
            sum += len(d);
            prev = d;
        }
        let mean = sum / n as f32;
        assert!((0.85..1.15).contains(&mean), "mean length {mean}");
        assert!(most < 0.45, "a tick moved the drift {most}");
    }

    /// A target crossing at a steady 190 is aimed behind it, by its
    /// speed times `track_lag`; on a reversed strafe the aim runs on past
    /// where it turned before it comes back.
    #[test]
    fn a_crossing_target_is_trailed_and_a_reversal_overshot() {
        let lag = Skill::default().track_lag;
        let eye = [0.0; 3];
        let mut pos = [500.0, 0.0, 0.0];
        let v = 190.0;
        let mut hand = Hand::new(&e(pos, [0.0, v, 0.0]), None);
        for _ in 0..10 {
            pos[1] += v * 0.05;
            hand.see(&e(pos, [0.0, v, 0.0]));
        }
        let aim = hand.perceived(eye, lag);
        assert!(
            ((pos[1] - aim[1]) - v * lag).abs() < 0.5,
            "trail {} want {}",
            pos[1] - aim[1],
            v * lag
        );
        let turn = pos[1];
        let mut most = f32::MIN;
        for _ in 0..10 {
            pos[1] -= v * 0.05;
            hand.see(&e(pos, [0.0, -v, 0.0]));
            most = most.max(hand.perceived(eye, lag)[1]);
        }
        assert!(
            most > turn + 0.5 * v * lag,
            "aim ran {} past the turn",
            most - turn
        );
        assert!(hand.perceived(eye, lag)[1] > pos[1], "trails the new way");
    }

    /// Recoil comes back late and in part: the share of the offset
    /// `recoil_ticks` ago.
    #[test]
    fn recoil_is_pulled_back_late_and_in_part() {
        let skill = Skill::default();
        let mut kept = [[0.0; 2]; RECOIL_KEEP];
        kept[0] = [-2.0, 0.5];
        assert_eq!(pulled(&kept, &skill), [0.0, 0.0]);
        kept[skill.recoil_ticks as usize] = [-2.0, 0.5];
        let p = pulled(&kept, &skill);
        assert!(p[0] < 0.0 && p[0] > -2.0, "{p:?}");
        assert_eq!(p[0] / -2.0, skill.recoil_share);
    }
}
