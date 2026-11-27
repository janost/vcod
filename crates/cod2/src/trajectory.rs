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
    /// `prefix` is `"pos"` or `"apos"`; any other panics.
    pub fn read(es: &EntityState2, prefix: &str) -> Self {
        match prefix {
            "pos" => Self::pos(es),
            "apos" => Self::apos(es),
            _ => panic!("no trajectory {prefix}"),
        }
    }

    /// `pos`.
    pub fn pos(es: &EntityState2) -> Self {
        use crate::net::fields::ENTITY_STATE_FIELDS as T;
        Self::at(
            es,
            [
                crate::field!(T, "pos.trType"),
                crate::field!(T, "pos.trTime"),
                crate::field!(T, "pos.trDuration"),
            ],
            crate::field!(T, 3 "pos.trBase"),
            crate::field!(T, 3 "pos.trDelta"),
        )
    }

    /// `apos`.
    pub fn apos(es: &EntityState2) -> Self {
        use crate::net::fields::ENTITY_STATE_FIELDS as T;
        Self::at(
            es,
            [
                crate::field!(T, "apos.trType"),
                crate::field!(T, "apos.trTime"),
                crate::field!(T, "apos.trDuration"),
            ],
            crate::field!(T, 3 "apos.trBase"),
            crate::field!(T, 3 "apos.trDelta"),
        )
    }

    fn at(
        es: &EntityState2,
        [ty, time, dur]: [usize; 3],
        base: [usize; 3],
        delta: [usize; 3],
    ) -> Self {
        Trajectory2 {
            tr_type: es.i32_at(ty),
            tr_time: es.i32_at(time),
            tr_duration: es.i32_at(dur),
            base: Vec3::from_array(es.vec3_at(base)),
            delta: Vec3::from_array(es.vec3_at(delta)),
        }
    }

    /// `BG_EvaluateTrajectory` at `at_ms` (lnxded 0x80df5d4): the stack-order
    /// arithmetic of `vcod_common`'s `Trajectory::evaluate_stored`, which
    /// CoD2's numbering is mapped onto. Retail `Com_Error`s on a type past 8;
    /// this returns `base`. The clocks wrap as the C `int`s do: CoD4's
    /// `physicsLaunch` sends `trDuration` 0x7fffffff (cod4-gsc.md).
    pub fn evaluate_f32(&self, at_ms: i32) -> Vec3 {
        use vcod_common::net::trajectory as v1;
        let tr_type = match self.tr_type {
            TR_STATIONARY..=TR_GRAVITY => self.tr_type,
            TR_GRAVITY_PAUSED => v1::TR_GRAVITY_PAUSED,
            TR_ACCELERATE => v1::TR_ACCELERATE,
            TR_DECELERATE => v1::TR_DECCELERATE,
            _ => return self.base,
        };
        // A ramp with no duration or no pace stays at its base.
        if matches!(tr_type, v1::TR_ACCELERATE | v1::TR_DECCELERATE)
            && (self.tr_duration == 0 || self.delta == Vec3::ZERO)
        {
            return self.base;
        }
        v1::Trajectory {
            tr_type,
            tr_time: self.tr_time,
            tr_duration: self.tr_duration,
            base: self.base,
            delta: self.delta,
        }
        .evaluate_stored(at_ms)
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
        tr(tr_type).evaluate_f32(at).x
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
        let g = tr(TR_GRAVITY).evaluate_f32(2000);
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
            assert!(t.evaluate_f32(95_000).is_finite());
        }
    }
}
