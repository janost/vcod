//! CoD2's `trajectory_t` and `BG_EvaluateTrajectory` (Mac 0x6b262), read off
//! an [`EntityState2`]. Nine types where 1.1 has eleven: no
//! `TR_GRAVITY_LOW` or `TR_GRAVITY_FLOAT`, so the accelerations sit at 7
//! and 8 (docs/research/cod2-protocol.md 12.2).

use glam::Vec3;

use crate::net::EntityState2;

pub const TR_STATIONARY: i32 = 0;
pub const TR_INTERPOLATE: i32 = 1;
pub const TR_LINEAR: i32 = 2;
pub const TR_LINEAR_STOP: i32 = 3;
pub const TR_SINE: i32 = 4;
pub const TR_GRAVITY: i32 = 5;
pub const TR_GRAVITY_PAUSED: i32 = 6;
pub const TR_ACCELERATE: i32 = 7;
pub const TR_DECELERATE: i32 = 8;

/// `pos` or `apos` as it came off the wire.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Trajectory2 {
    pub tr_type: i32,
    pub tr_time: i32,
    pub tr_duration: i32,
    pub base: Vec3,
    pub delta: Vec3,
}

impl Trajectory2 {
    /// `prefix` is `"pos"` or `"apos"`.
    pub fn read(es: &EntityState2, prefix: &str) -> Self {
        let v = |name: &str| {
            Vec3::from_array([0, 1, 2].map(|i| es.f32(&format!("{prefix}.{name}[{i}]"))))
        };
        Trajectory2 {
            tr_type: es.i32(&format!("{prefix}.trType")),
            tr_time: es.i32(&format!("{prefix}.trTime")),
            tr_duration: es.i32(&format!("{prefix}.trDuration")),
            base: v("trBase"),
            delta: v("trDelta"),
        }
    }

    /// `BG_EvaluateTrajectory` at `at_ms`. Retail `Com_Error`s on a type past
    /// 8; this returns `base`. The clocks wrap as the C `int`s do: CoD4's
    /// `physicsLaunch` sends `trDuration` 0x7fffffff (cod4-gsc.md).
    pub fn evaluate(&self, at_ms: i32) -> Vec3 {
        let secs = |t: i32| t.wrapping_sub(self.tr_time) as f32 * 0.001;
        let end = self.tr_time.wrapping_add(self.tr_duration);
        match self.tr_type {
            TR_LINEAR => self.base + self.delta * secs(at_ms),
            TR_LINEAR_STOP => self.base + self.delta * secs(at_ms.min(end)).max(0.0),
            TR_SINE => {
                let frac = at_ms.wrapping_sub(self.tr_time) as f32 / self.tr_duration as f32;
                let phase = (f64::from(frac) * std::f64::consts::PI * 2.0).sin() as f32;
                self.base + self.delta * phase
            }
            TR_GRAVITY => {
                let t = secs(at_ms);
                let mut r = self.base + self.delta * t;
                r.z += t * t * -400.0;
                r
            }
            TR_ACCELERATE | TR_DECELERATE => {
                let t = secs(at_ms.min(end));
                let dur = self.tr_duration as f32 * 0.001;
                if dur == 0.0 {
                    return self.base;
                }
                let dir = self.delta.normalize_or_zero();
                let accel = self.delta.length() / dur;
                if self.tr_type == TR_ACCELERATE {
                    self.base + dir * (accel * 0.5 * t * t)
                } else {
                    self.base + self.delta * t + dir * (accel * -0.5 * t * t)
                }
            }
            _ => self.base, // stationary, interpolate, gravity paused
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tr(tr_type: i32) -> Trajectory2 {
        Trajectory2 {
            tr_type,
            tr_time: 1000,
            tr_duration: 2000,
            base: Vec3::new(10.0, 0.0, 0.0),
            delta: Vec3::new(100.0, 0.0, 0.0),
        }
    }

    fn x(tr_type: i32, at: i32) -> f32 {
        tr(tr_type).evaluate(at).x
    }

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-3
    }

    #[test]
    fn types_follow_the_cod2_numbering() {
        assert!(close(x(TR_STATIONARY, 5000), 10.0));
        assert!(close(x(TR_GRAVITY_PAUSED, 5000), 10.0));
        assert!(close(x(TR_LINEAR, 4000), 310.0));
        // Clamped to the duration and never before the start.
        assert!(close(x(TR_LINEAR_STOP, 4000), 210.0));
        assert!(close(x(TR_LINEAR_STOP, 500), 10.0));
        assert!(close(x(TR_SINE, 1500), 110.0));
        let g = tr(TR_GRAVITY).evaluate(2000);
        assert!(close(g.x, 110.0) && close(g.z, -400.0));
        // From rest a full duration covers half of `delta`'s pace; to rest,
        // the pace less that half.
        assert!(close(x(TR_ACCELERATE, 3000), 10.0 + 100.0));
        assert!(close(x(TR_DECELERATE, 9000), 10.0 + 200.0 - 100.0));
    }

    /// A `physicsLaunch`ed entity's `trDuration` 0x7fffffff past a late
    /// `trTime` wraps its end, as retail's `int` does, instead of panicking
    /// the client (the map sweep's mp_backlot `sab` car).
    #[test]
    fn a_forever_duration_wraps() {
        for tr_type in [TR_LINEAR_STOP, TR_ACCELERATE, 10] {
            let t = Trajectory2 {
                tr_type,
                tr_time: 90_000,
                tr_duration: 0x7fff_ffff,
                base: Vec3::ONE,
                delta: Vec3::X,
            };
            assert!(t.evaluate(95_000).is_finite());
        }
    }
}
