//! Contains routines ported from the Open Dynamics Engine 0.5, Copyright (C) 2001-2004
//! Russell L. Smith, under its BSD license (LICENSE-ODE-BSD.txt beside this file). See NOTICE.
//!
//! A rigid body (`dxBody`, 0x150 bytes in iw3mp.exe) and its position
//! update (`dxStepBody`, 0x5b8380); cod4-player-anims.md 10.4.

use super::mass::Mass;
use super::math::{IDENTITY, M3, Quat, V3};
use super::rotation::{normalize4, q_multiply0, q_to_r, w_to_dq};
use super::world::JointId;

/// `dxBodyFlagFiniteRotation`.
pub const FINITE_ROTATION: u32 = 1;
/// `dxBodyDisabled`.
pub const DISABLED: u32 = 4;
/// `dxBodyNoGravity`.
pub const NO_GRAVITY: u32 = 8;
/// `dxBodyAutoDisable`.
pub const AUTO_DISABLE: u32 = 0x10;

/// The sleep test's thresholds (`dxAutoDisable`, the body's at +0xc8): the
/// speeds are squared.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AutoDisable {
    pub linear_threshold: f32,
    pub angular_threshold: f32,
    pub idle_time: f32,
    pub idle_steps: i32,
}

/// One entry of a body's joint list (`dxJointNode`): the joint and the body
/// at its other end.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Node {
    pub joint: JointId,
    pub other: Option<super::world::BodyId>,
}

#[derive(Clone, Debug)]
pub struct Body {
    pub flags: u32,
    pub mass: Mass,
    pub inv_i: M3,
    pub inv_mass: f32,
    pub pos: V3,
    pub q: Quat,
    pub r: M3,
    pub lvel: V3,
    pub avel: V3,
    pub facc: V3,
    pub tacc: V3,
    pub adis: AutoDisable,
    pub adis_timeleft: f32,
    pub adis_stepsleft: i32,
    /// The island walk's mark, then the body's index in its island.
    pub(crate) tag: i32,
    /// The joints attached, first visited first.
    pub(crate) nodes: Vec<Node>,
}

impl Body {
    pub(crate) fn new(adis: AutoDisable, auto_disable: bool) -> Body {
        Body {
            flags: if auto_disable { AUTO_DISABLE } else { 0 },
            mass: Mass::unit(),
            inv_i: IDENTITY,
            inv_mass: 1.0,
            pos: [0.0; 3],
            q: [1.0, 0.0, 0.0, 0.0],
            r: IDENTITY,
            lvel: [0.0; 3],
            avel: [0.0; 3],
            facc: [0.0; 3],
            tacc: [0.0; 3],
            adis,
            adis_timeleft: adis.idle_time,
            adis_stepsleft: adis.idle_steps,
            tag: 0,
            nodes: Vec::new(),
        }
    }

    pub fn enabled(&self) -> bool {
        self.flags & DISABLED == 0
    }

    /// `dBodyEnable`: awake, its idle counters restarted.
    pub fn enable(&mut self) {
        self.flags &= !DISABLED;
        self.adis_stepsleft = self.adis.idle_steps;
        self.adis_timeleft = self.adis.idle_time;
    }

    pub fn set_finite_rotation(&mut self, on: bool) {
        if on {
            self.flags |= FINITE_ROTATION;
        } else {
            self.flags &= !FINITE_ROTATION;
        }
    }

    pub fn set_gravity(&mut self, on: bool) {
        if on {
            self.flags &= !NO_GRAVITY;
        } else {
            self.flags |= NO_GRAVITY;
        }
    }

    /// `dBodySetMass` (0x5b6f70): `invI` by Cholesky, the identity when
    /// the inertia is not positive definite.
    pub fn set_mass(&mut self, m: Mass) {
        self.mass = m;
        self.inv_i = super::mass::invert_pd3(&m.i).unwrap_or(IDENTITY);
        self.inv_mass = (1.0 / m.mass as f64) as f32;
    }

    /// `dBodySetRotation` (0x5b6ec0): through the quaternion, so `R` comes
    /// back orthonormal.
    pub fn set_rotation(&mut self, r: &M3) {
        self.set_quaternion(super::rotation::r_to_q(r));
    }

    /// `dBodySetQuaternion`: `q` normalized, `R` rebuilt.
    pub fn set_quaternion(&mut self, q: Quat) {
        self.q = q;
        normalize4(&mut self.q);
        self.r = q_to_r(self.q);
    }

    /// `dBodyAddForce`, world frame, at the centre.
    pub fn add_force(&mut self, f: V3) {
        self.facc = super::math::add(self.facc, f);
    }

    /// `dBodyAddForceAtPos` (0x5b7000): `f` onto the force, `(p - pos) x
    /// f` onto the torque, each sum stored (ODE 0.5's `dCROSS`).
    pub fn add_force_at_pos(&mut self, f: V3, p: V3) {
        let w = |x: f32| x as f64;
        self.facc = super::math::add(self.facc, f);
        let q = super::math::sub(p, self.pos);
        let t = &mut self.tacc;
        t[0] = ((w(q[1]) * w(f[2]) - w(q[2]) * w(f[1])) + w(t[0])) as f32;
        t[1] = ((w(q[2]) * w(f[0]) - w(q[0]) * w(f[2])) + w(t[1])) as f32;
        t[2] = ((w(q[0]) * w(f[1]) - w(q[1]) * w(f[0])) + w(t[2])) as f32;
    }

    pub fn add_torque(&mut self, t: V3) {
        self.tacc = super::math::add(self.tacc, t);
    }

    /// `dxStepBody` (0x5b8380): moves the body along its velocities for `h`.
    /// IW's build has no `dGeomMoved` here and no finite rotation axis.
    pub(crate) fn step(&mut self, h: f32) {
        let hw = h as f64;
        for j in 0..3 {
            self.pos[j] = (self.lvel[j] as f64 * hw + self.pos[j] as f64) as f32;
        }
        if self.flags & FINITE_ROTATION != 0 {
            let half = (hw * 0.5) as f32;
            let a = self.avel.map(|x| x as f64);
            let wlen2 = ((a[0] * a[0] + a[1] * a[1]) + a[2] * a[2]) as f32;
            let wlen = (wlen2 as f64).sqrt() as f32;
            let theta = (wlen as f64 * half as f64) as f32;
            let s = (sinc(theta) as f64 * half as f64) as f32;
            let q = [
                (theta as f64).cos() as f32,
                (a[0] * s as f64) as f32,
                (a[1] * s as f64) as f32,
                (a[2] * s as f64) as f32,
            ];
            self.q = q_multiply0(q, self.q);
        } else {
            let dq = w_to_dq(self.avel, self.q);
            for j in 0..4 {
                self.q[j] = (self.q[j] as f64 + dq[j] as f64 * hw) as f32;
            }
        }
        normalize4(&mut self.q);
        self.r = q_to_r(self.q);
    }
}

/// `sin(x) / x` (0x5b8320), its Taylor pair under 1e-4 (the double at
/// 0x70b178) with 1/6 a float widened (0x70b600).
pub(crate) fn sinc(x: f32) -> f32 {
    let xw = x as f64;
    if xw.abs() < 1.0e-4 {
        (1.0 - xw * xw * (1.0f32 / 6.0) as f64) as f32
    } else {
        ((xw.sin() as f32) as f64 / xw) as f32
    }
}
