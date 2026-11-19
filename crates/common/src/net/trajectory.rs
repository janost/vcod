//! Contains routines ported from the RTCW-MP GPL source, Copyright (C) 1999-2010 id Software LLC, a ZeniMax Media company.
//! See NOTICE.
//!
//! `trajectory_t` / `BG_EvaluateTrajectory`. The per-branch arithmetic is
//! RTCW-MP's (`bg_misc.c:3510ff`); the enumeration is not. CoD 1.1 dispatches
//! it through an eleven-entry jump table (`game.mp.i386.so` 0x2c600, cgame
//! `cgame_mp_x86.dll` 0x30005470) and `Com_Error`s on anything above 10.
//! Three forms, named for their rounding: `evaluate_kept` (1.1d's server),
//! `evaluate_stored` (1.5b's, UO's and CoD2's lnxded) and `evaluate_f32`.

use super::msg::EntityState;
use super::protocol::Cod1Wire;
use crate::x87;
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

/// A server build's `dt`: the ms times the float 0.001, left on the x87
/// stack (1.1d).
fn secs_kept(ms: i32) -> f64 {
    f64::from(ms) * f64::from(0.001_f32)
}

/// [`secs_kept`] stored to a float (1.5b 0x1b8f5 on a line, 0x1ba87 on an
/// arc). The `f64` product is exact below 2^29 ms, so this is the `fstp`'s
/// one rounding.
fn secs_stored(ms: i32) -> f64 {
    f64::from(secs_kept(ms) as f32)
}

/// `pos`/`apos` as it comes off the wire. `tr_duration` is 0 for the
/// unbounded types.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Trajectory {
    pub tr_type: i32,
    pub tr_time: i32,
    pub tr_duration: i32,
    pub base: Vec3,
    pub delta: Vec3,
}

/// A trajectory's nine words in [`Trajectory::read`]'s order.
const WORDS: [&str; 9] = [
    "trType",
    "trTime",
    "trDuration",
    "trBase[0]",
    "trBase[1]",
    "trBase[2]",
    "trDelta[0]",
    "trDelta[1]",
    "trDelta[2]",
];

/// [`WORDS`] under `pos.` and `apos.`, spelled out so a read builds no name.
fn names(prefix: &str) -> Option<&'static [&'static str; 9]> {
    const POS: [&str; 9] = [
        "pos.trType",
        "pos.trTime",
        "pos.trDuration",
        "pos.trBase[0]",
        "pos.trBase[1]",
        "pos.trBase[2]",
        "pos.trDelta[0]",
        "pos.trDelta[1]",
        "pos.trDelta[2]",
    ];
    const APOS: [&str; 9] = [
        "apos.trType",
        "apos.trTime",
        "apos.trDuration",
        "apos.trBase[0]",
        "apos.trBase[1]",
        "apos.trBase[2]",
        "apos.trDelta[0]",
        "apos.trDelta[1]",
        "apos.trDelta[2]",
    ];
    match prefix {
        "pos" => Some(&POS),
        "apos" => Some(&APOS),
        _ => None,
    }
}

impl Trajectory {
    /// `prefix` is `"pos"` or `"apos"`.
    pub fn read(ent: &EntityState, p: &Cod1Wire, prefix: &str) -> Trajectory {
        let at = |k: usize| match names(prefix) {
            Some(n) => ent.field_i32(p, n[k]),
            None => ent.field_i32(p, &format!("{prefix}.{}", WORDS[k])),
        };
        let f = |k: usize| f32::from_bits(at(k) as u32);
        Trajectory {
            tr_type: at(0),
            tr_time: at(1),
            tr_duration: at(2),
            base: Vec3::new(f(3), f(4), f(5)),
            delta: Vec3::new(f(6), f(7), f(8)),
        }
    }

    /// `BG_EvaluateTrajectory` at `at_ms` with every product and sum rounded
    /// to a float: what the cgame draws, and the server sites that no capture
    /// pins to the bit. Unhandled types fall back to `base`.
    pub fn evaluate_f32(&self, at_ms: i32) -> Vec3 {
        match self.tr_type {
            TR_STATIONARY | TR_INTERPOLATE | TR_GRAVITY_PAUSED => self.base,
            TR_LINEAR => {
                let dt = at_ms.wrapping_sub(self.tr_time) as f32 * 0.001;
                self.base + self.delta * dt
            }
            TR_LINEAR_STOP => {
                let clamped = at_ms.min(self.tr_time.wrapping_add(self.tr_duration));
                let dt = (clamped.wrapping_sub(self.tr_time) as f32 * 0.001).max(0.0);
                self.base + self.delta * dt
            }
            TR_SINE => {
                let dt = at_ms.wrapping_sub(self.tr_time) as f32 / self.tr_duration as f32;
                let phase = (dt * std::f32::consts::TAU).sin();
                self.base + self.delta * phase
            }
            TR_GRAVITY => {
                let dt = at_ms.wrapping_sub(self.tr_time) as f32 * 0.001;
                let mut r = self.base + self.delta * dt;
                r.z -= 0.5 * DEFAULT_GRAVITY * dt * dt;
                r
            }
            TR_GRAVITY_LOW => {
                let dt = at_ms.wrapping_sub(self.tr_time) as f32 * 0.001;
                let mut r = self.base + self.delta * dt;
                r.z -= 0.5 * (DEFAULT_GRAVITY * 0.3) * dt * dt;
                r
            }
            TR_GRAVITY_FLOAT => {
                let dt = at_ms.wrapping_sub(self.tr_time) as f32 * 0.001;
                let mut r = self.base + self.delta * dt;
                // Single `dt`, not `dt * dt`: faithful to RTCW's bg_misc.c.
                r.z -= 0.5 * (DEFAULT_GRAVITY * 0.2) * dt;
                r
            }
            TR_ACCELERATE => {
                let clamped = at_ms.min(self.tr_time.wrapping_add(self.tr_duration));
                let dt = clamped.wrapping_sub(self.tr_time) as f32 * 0.001;
                let dur_s = self.tr_duration as f32 * 0.001;
                if dur_s == 0.0 || self.delta == Vec3::ZERO {
                    return self.base;
                }
                let phase = self.delta.length() / dur_s;
                let dir = self.delta.normalize();
                self.base + dir * (phase * 0.5 * dt * dt)
            }
            TR_DECCELERATE => {
                let clamped = at_ms.min(self.tr_time.wrapping_add(self.tr_duration));
                let dt = clamped.wrapping_sub(self.tr_time) as f32 * 0.001;
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

    /// 1.1d's server `BG_EvaluateTrajectory` (`.so` 0x2c600): `dt` the ms
    /// times the float 0.001 ([`secs_kept`]), each product and sum on the
    /// x87 stack, every component rounded once into its store. A line is
    /// `delta * dt + base` (0x2c640), a stop the same with the time clamped
    /// to its end and `dt` to no less than 0 (0x2c673..0x2c69e), an arc's `z`
    /// less `400 * dt * dt` (0x2c6c0), the ramps [`Trajectory::ramp_kept`]'s
    /// (0x2c767, 0x2c800). Other types are [`Trajectory::evaluate_f32`]'s.
    pub fn evaluate_kept(&self, at_ms: i32) -> Vec3 {
        let dt = |t: i32| secs_kept(t.wrapping_sub(self.tr_time));
        match self.tr_type {
            TR_LINEAR => self.line(dt(at_ms)),
            TR_LINEAR_STOP => self.line(dt(at_ms.min(self.end())).max(0.0)),
            TR_GRAVITY => {
                let dt = dt(at_ms);
                let (b, d) = (self.base.as_dvec3(), self.delta.as_dvec3());
                let half_g = f64::from(0.5 * DEFAULT_GRAVITY);
                Vec3::new(
                    (b.x + d.x * dt) as f32,
                    (b.y + d.y * dt) as f32,
                    ((b.z + d.z * dt) - half_g * dt * dt) as f32,
                )
            }
            TR_ACCELERATE | TR_DECCELERATE => self.ramp_kept(at_ms),
            _ => self.evaluate_f32(at_ms),
        }
    }

    /// 1.5b's and United Offensive's server `BG_EvaluateTrajectory` (1.5b
    /// `.so` 0x1b877, UO 0x1fe63): `dt` stored as a float ([`secs_stored`],
    /// 0x1ba87), each component's `base + delta * dt` rounded once into its
    /// store (0x1b8da, 0x1ba6c), a stop clamped as 1.1d's, then an arc's
    /// stored `z` less `dt * 400 * dt` on the stack, rounded once (0x1bade);
    /// the ramps [`Trajectory::ramp_stored`]'s (0x1bc0a, 0x1bd22). CoD2's
    /// lnxded (0x80df5d4) runs the line, the stop and the arc alike
    /// (cod2-combat.md 9.4). Other types are [`Trajectory::evaluate_f32`]'s.
    pub fn evaluate_stored(&self, at_ms: i32) -> Vec3 {
        let dt = |t: i32| secs_stored(t.wrapping_sub(self.tr_time));
        match self.tr_type {
            TR_LINEAR => self.line(dt(at_ms)),
            TR_LINEAR_STOP => self.line(dt(at_ms.min(self.end())).max(0.0)),
            TR_GRAVITY => {
                let dt = dt(at_ms);
                let r = self.line(dt);
                let half_g = f64::from(0.5 * DEFAULT_GRAVITY);
                r.with_z((f64::from(r.z) - dt * half_g * dt) as f32)
            }
            TR_ACCELERATE | TR_DECCELERATE => self.ramp_stored(at_ms),
            _ => self.evaluate_f32(at_ms),
        }
    }

    /// `trTime + trDuration`, wrapping as the C `int` does.
    fn end(&self) -> i32 {
        self.tr_time.wrapping_add(self.tr_duration)
    }

    /// `delta * dt + base` per component, rounded once.
    fn line(&self, dt: f64) -> Vec3 {
        (self.delta.as_dvec3() * dt + self.base.as_dvec3()).as_vec3()
    }

    /// 1.1d's `TR_ACCELERATE` and `TR_DECCELERATE` arms (0x2c767, 0x2c800):
    /// the time clamped at the segment's end, `phase` the stored length of the
    /// delta over the duration, both kept on the stack, the direction
    /// `VectorNormalize2`'s; `base + dir * (phase/2 dt^2)` accelerating,
    /// `(base + delta dt) - dir * (phase/2 dt^2)` decelerating, one rounding.
    fn ramp_kept(&self, at_ms: i32) -> Vec3 {
        let dt = secs_kept(at_ms.min(self.end()).wrapping_sub(self.tr_time));
        let len = x87::dot(self.delta, self.delta).sqrt() as f32;
        let phase = f64::from(len) / secs_kept(self.tr_duration);
        let dir = x87::normalize(self.delta).as_dvec3();
        let (base, delta) = (self.base.as_dvec3(), self.delta.as_dvec3());
        let out = if self.tr_type == TR_ACCELERATE {
            let s = phase * 0.5 * dt * dt;
            dir * s + base
        } else {
            let s = -phase * 0.5 * dt * dt;
            (delta * dt + base) + dir * s
        };
        out.as_vec3()
    }

    /// 1.5b's ramp arms (game.mp 0x1bc0a, 0x1bd22), which UO shares: `dt`,
    /// the length and `phase` each stored as a float, the decelerating
    /// segment's `base + delta dt` stored before the ramp term is added.
    fn ramp_stored(&self, at_ms: i32) -> Vec3 {
        let dt = secs_stored(at_ms.min(self.end()).wrapping_sub(self.tr_time));
        let len = x87::dot(self.delta, self.delta).sqrt() as f32;
        let phase = f64::from((f64::from(len) / secs_kept(self.tr_duration)) as f32);
        let dir = x87::normalize2_stored(self.delta).as_dvec3();
        let base = self.base.as_dvec3();
        if self.tr_type == TR_ACCELERATE {
            return (phase * 0.5 * dt * dt * dir + base).as_vec3();
        }
        let a = self.line(dt).as_dvec3();
        (-phase * 0.5 * dt * dt * dir + a).as_vec3()
    }

    /// The inverse of [`Trajectory::read`]: every word of the `prefix` trajectory.
    pub fn write(&self, ent: &mut EntityState, p: &Cod1Wire, prefix: &str) {
        let [bx, by, bz] = self.base.to_array().map(f32::to_bits);
        let [dx, dy, dz] = self.delta.to_array().map(f32::to_bits);
        let words = [
            self.tr_type,
            self.tr_time,
            self.tr_duration,
            bx as i32,
            by as i32,
            bz as i32,
            dx as i32,
            dy as i32,
            dz as i32,
        ];
        for (k, v) in words.into_iter().enumerate() {
            let i = match names(prefix) {
                Some(n) => EntityState::field_index(p, n[k]),
                None => EntityState::field_index(p, &format!("{prefix}.{}", WORDS[k])),
            };
            if let Some(i) = i {
                ent.fields[i] = v;
            }
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

    /// Wire times wrap as retail's C ints do; no type panics on them.
    #[test]
    fn extreme_wire_times_wrap_without_a_panic() {
        let d = Vec3::new(1.0, 2.0, 3.0);
        for t in 0..=TR_DECCELERATE {
            for (time, dur, at) in [
                (i32::MIN, i32::MAX, 1000),
                (i32::MAX, i32::MAX, -1000),
                (1000, 0x7fff_ffff, i32::MAX),
            ] {
                let tr = traj(t, time, dur, Vec3::ZERO, d);
                let _ = tr.evaluate_f32(at);
                let _ = tr.evaluate_kept(at);
                let _ = tr.evaluate_stored(at);
            }
        }
    }

    /// 1.5b's `dt` is `fildl`, `fmulp` by the float 0.001 and `fstps`
    /// (0x1b8e6..0x1b8f5): the exact product rounded once, the ms never a
    /// float first. Past 2^24 ms the two part.
    #[test]
    fn stored_dt_rounds_the_ms_product_once() {
        let tr = traj(TR_LINEAR, 0, 0, Vec3::ZERO, Vec3::X);
        let ms = (1 << 24) + 1;
        assert_eq!(tr.evaluate_stored(ms).x, 16_777.219);
        assert_eq!(ms as f32 * 0.001, 16_777.217);
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
        let p = tr.evaluate_f32(1500);
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
        assert_eq!(tr.evaluate_f32(1250).x, 25.0);
        assert_eq!(tr.evaluate_f32(2000).x, 50.0); // clamped at trTime+trDuration
        assert_eq!(tr.evaluate_f32(500).x, 0.0); // before start
    }

    #[test]
    fn sine_oscillates() {
        let tr = traj(TR_SINE, 0, 1000, Vec3::ZERO, Vec3::new(0.0, 0.0, 10.0));
        assert!((tr.evaluate_f32(250).z - 10.0).abs() < 1e-3); // sin(pi/2)
        assert!(tr.evaluate_f32(500).z.abs() < 1e-3);
        assert_eq!(tr.evaluate_f32(0).z, 0.0);
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
        assert_eq!(tr.evaluate_f32(2000), Vec3::new(1.0, 2.0, -397.0));
    }

    #[test]
    fn stationary_and_interpolate_hold_base() {
        for t in [TR_STATIONARY, TR_INTERPOLATE] {
            let tr = traj(t, 0, 0, Vec3::X, Vec3::new(999.0, 0.0, 0.0));
            assert_eq!(tr.evaluate_f32(5000), Vec3::X);
        }
    }

    #[test]
    fn write_then_read_round_trips_a_gravity_arc() {
        let p = &crate::net::protocol::PROTOCOL_V1;
        let t = Trajectory {
            tr_type: TR_GRAVITY,
            tr_time: 12_345,
            tr_duration: 0,
            base: Vec3::new(1.0, 2.0, 3.0),
            delta: Vec3::new(400.0, 0.0, 300.0),
        };
        let mut e = EntityState::null(p);
        t.write(&mut e, p, "pos");
        assert_eq!(Trajectory::read(&e, p, "pos"), t);
        assert_eq!(e.field_i32(p, "pos.trType"), TR_GRAVITY);
    }
}
