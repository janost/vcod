//! Contains routines ported from the RTCW-MP GPL source, Copyright (C) 1999-2010 id Software LLC, a ZeniMax Media company.
//! See NOTICE.
//!
//! `trajectory_t` / `BG_EvaluateTrajectory`. The per-branch arithmetic is
//! RTCW-MP's (`bg_misc.c:3510ff`); the enumeration is not. CoD 1.1 dispatches
//! it through an eleven-entry jump table (`game.mp.i386.so` 0x2c600, cgame
//! `cgame_mp_x86.dll` 0x30005470) and `Com_Error`s on anything above 10.

use super::msg::EntityState;
use super::protocol::Protocol;
use glam::Vec3;

/// `trType_t`. CoD 1.1 has no `TR_LINEAR_STOP_BACK`, `TR_SPLINE` or
/// `TR_LINEAR_PATH`, so every value from 4 up sits one index below the RTCW
/// header codextended's `shared.h:460` pastes. VERIFIED off the eleven-entry
/// jump tables at `.so` 0x700a8 (evaluate) and 0x7012c (delta), both entered
/// through `cmp $0xa; ja Com_Error` (`docs/protocol-1.1.md`, divergence 8).
/// A `trType` above 10 is fatal in retail; here it falls back to `trBase`.
pub const TR_STATIONARY: i32 = 0;
pub const TR_INTERPOLATE: i32 = 1;
pub const TR_LINEAR: i32 = 2;
pub const TR_LINEAR_STOP: i32 = 3;
pub const TR_SINE: i32 = 4;
pub const TR_GRAVITY: i32 = 5;
pub const TR_GRAVITY_LOW: i32 = 6;
pub const TR_GRAVITY_FLOAT: i32 = 7;
pub const TR_GRAVITY_PAUSED: i32 = 8;
pub const TR_ACCELERATE: i32 = 9;
pub const TR_DECCELERATE: i32 = 10;

/// `g_pmove.c` `DEFAULT_GRAVITY`. No per-entity gravity is on the wire.
/// VERIFIED as the literal the `TR_GRAVITY` delta branch reads
/// (`.so` .rodata 0x70120); evaluate carries the halved 400.0 at 0x70098.
pub const DEFAULT_GRAVITY: f32 = 800.0;

/// `pos`/`apos` as it comes off the wire. `tr_duration` is 0 for the
/// unbounded types.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Trajectory {
    pub tr_type: i32,
    pub tr_time: i32,
    pub tr_duration: i32,
    pub base: Vec3,
    pub delta: Vec3,
}

impl Trajectory {
    /// `prefix` is `"pos"` or `"apos"`.
    pub fn read(ent: &EntityState, p: &Protocol, prefix: &str) -> Trajectory {
        let i32_field = |name: &str| ent.field_i32(p, &format!("{prefix}.{name}"));
        let f32_field = |name: &str| ent.field_f32(p, &format!("{prefix}.{name}"));
        Trajectory {
            tr_type: i32_field("trType"),
            tr_time: i32_field("trTime"),
            tr_duration: i32_field("trDuration"),
            base: Vec3::new(
                f32_field("trBase[0]"),
                f32_field("trBase[1]"),
                f32_field("trBase[2]"),
            ),
            delta: Vec3::new(
                f32_field("trDelta[0]"),
                f32_field("trDelta[1]"),
                f32_field("trDelta[2]"),
            ),
        }
    }

    /// `BG_EvaluateTrajectory` at server time `at_ms`. Unhandled types fall
    /// back to `base`.
    pub fn evaluate(&self, at_ms: i32) -> Vec3 {
        match self.tr_type {
            TR_STATIONARY | TR_INTERPOLATE | TR_GRAVITY_PAUSED => self.base,
            TR_LINEAR => {
                let dt = (at_ms - self.tr_time) as f32 * 0.001;
                self.base + self.delta * dt
            }
            TR_LINEAR_STOP => {
                let clamped = at_ms.min(self.tr_time + self.tr_duration);
                let dt = ((clamped - self.tr_time) as f32 * 0.001).max(0.0);
                self.base + self.delta * dt
            }
            TR_SINE => {
                let dt = (at_ms - self.tr_time) as f32 / self.tr_duration as f32;
                let phase = (dt * std::f32::consts::TAU).sin();
                self.base + self.delta * phase
            }
            TR_GRAVITY => {
                let dt = (at_ms - self.tr_time) as f32 * 0.001;
                let mut r = self.base + self.delta * dt;
                r.z -= 0.5 * DEFAULT_GRAVITY * dt * dt;
                r
            }
            TR_GRAVITY_LOW => {
                let dt = (at_ms - self.tr_time) as f32 * 0.001;
                let mut r = self.base + self.delta * dt;
                r.z -= 0.5 * (DEFAULT_GRAVITY * 0.3) * dt * dt;
                r
            }
            TR_GRAVITY_FLOAT => {
                let dt = (at_ms - self.tr_time) as f32 * 0.001;
                let mut r = self.base + self.delta * dt;
                // Single `dt`, not `dt * dt`: faithful to RTCW's bg_misc.c.
                r.z -= 0.5 * (DEFAULT_GRAVITY * 0.2) * dt;
                r
            }
            TR_ACCELERATE => {
                let clamped = at_ms.min(self.tr_time + self.tr_duration);
                let dt = (clamped - self.tr_time) as f32 * 0.001;
                let dur_s = self.tr_duration as f32 * 0.001;
                if dur_s == 0.0 || self.delta == Vec3::ZERO {
                    return self.base;
                }
                let phase = self.delta.length() / dur_s;
                let dir = self.delta.normalize();
                self.base + dir * (phase * 0.5 * dt * dt)
            }
            TR_DECCELERATE => {
                let clamped = at_ms.min(self.tr_time + self.tr_duration);
                let dt = (clamped - self.tr_time) as f32 * 0.001;
                let dur_s = self.tr_duration as f32 * 0.001;
                if dur_s == 0.0 || self.delta == Vec3::ZERO {
                    return self.base;
                }
                let phase = self.delta.length() / dur_s;
                let dir = self.delta.normalize();
                let v = self.base + self.delta * dt;
                v - dir * (phase * 0.5 * dt * dt)
            }
            _ => self.base,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::Vec3;

    fn traj(t: i32, time: i32, dur: i32, base: Vec3, delta: Vec3) -> Trajectory {
        Trajectory {
            tr_type: t,
            tr_time: time,
            tr_duration: dur,
            base,
            delta,
        }
    }

    #[test]
    fn gravity_arc() {
        // thrown at t=1000 with vz=+200: at t=1500 (dt=0.5s)
        // z = 100 + 200*0.5 - 0.5*800*0.25 = 100
        let tr = traj(
            TR_GRAVITY,
            1000,
            0,
            Vec3::new(0.0, 0.0, 100.0),
            Vec3::new(50.0, 0.0, 200.0),
        );
        let p = tr.evaluate(1500);
        assert!(
            (p.x - 25.0).abs() < 1e-3 && (p.z - 100.0).abs() < 1e-3,
            "{p}"
        );
    }

    #[test]
    fn linear_stop_clamps_at_duration() {
        let tr = traj(
            TR_LINEAR_STOP,
            1000,
            500,
            Vec3::ZERO,
            Vec3::new(100.0, 0.0, 0.0),
        );
        assert_eq!(tr.evaluate(1250).x, 25.0);
        assert_eq!(tr.evaluate(2000).x, 50.0); // clamped at trTime+trDuration
        assert_eq!(tr.evaluate(500).x, 0.0); // before start
    }

    #[test]
    fn sine_oscillates() {
        let tr = traj(TR_SINE, 0, 1000, Vec3::ZERO, Vec3::new(0.0, 0.0, 10.0));
        assert!((tr.evaluate(250).z - 10.0).abs() < 1e-3); // sin(pi/2)
        assert!(tr.evaluate(500).z.abs() < 1e-3);
        assert_eq!(tr.evaluate(0).z, 0.0);
    }

    /// The numbering retail's eleven-entry jump tables give, and the one
    /// value a corpse and a launched item are sent under: 5 is gravity, and
    /// it reads no `trDuration`. Retail's own `z` term is `-400*dt*dt`
    /// (cgame 0x30005470, `.rodata` 0x30069528).
    #[test]
    fn five_is_gravity_and_reads_no_duration() {
        assert_eq!(
            (
                TR_SINE,
                TR_GRAVITY,
                TR_GRAVITY_LOW,
                TR_GRAVITY_FLOAT,
                TR_GRAVITY_PAUSED,
                TR_ACCELERATE,
                TR_DECCELERATE
            ),
            (4, 5, 6, 7, 8, 9, 10)
        );
        // trDuration 0 is what a launched item carries; a sine reading of 5
        // would divide by it.
        let tr = traj(5, 1000, 0, Vec3::new(1.0, 2.0, 3.0), Vec3::ZERO);
        assert_eq!(tr.evaluate(2000), Vec3::new(1.0, 2.0, -397.0));
    }

    #[test]
    fn stationary_and_interpolate_hold_base() {
        for t in [TR_STATIONARY, TR_INTERPOLATE] {
            let tr = traj(t, 0, 0, Vec3::X, Vec3::new(999.0, 0.0, 0.0));
            assert_eq!(tr.evaluate(5000), Vec3::X);
        }
    }
}
