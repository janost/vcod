//! Contains routines ported from the Open Dynamics Engine 0.5, Copyright (C) 2001-2004
//! Russell L. Smith, under its BSD license (LICENSE-ODE-BSD.txt beside this file). See NOTICE.
//!
//! The hinge (type 2; `hingeGetInfo1` 0x5b3840, `hingeGetInfo2` 0x5b38e0),
//! with one limit and motor about its axis (cod4-player-anims.md 10.4).

use super::ball::{set_anchors, set_ball};
use super::limot::LimitMotor;
use super::{Info1, Info2};
use crate::ode::body::Body;
use crate::ode::math::{Quat, neg};
use crate::ode::math::{V3, cross, dot, mul0_331, mul1_331, normalize3, plane_space};
use crate::ode::rotation::{q_multiply1, q_multiply2, q_multiply3};
use crate::ode::world::Params;

/// `(double)(float)M_PI` (0x70b080) and twice it (0x70b078).
const PI: f64 = std::f32::consts::PI as f64;
const TWO_PI: f64 = 2.0 * PI;

#[derive(Clone, Debug)]
pub struct Hinge {
    pub anchor1: V3,
    pub anchor2: V3,
    /// The axis in each body's frame (+0x60, +0x70).
    pub axis1: V3,
    pub axis2: V3,
    /// Body 1's turn into body 2's at the start (+0x80).
    pub qrel: Quat,
    pub limot: LimitMotor,
}

impl Hinge {
    /// `hingeInit` (0x5b3780): both axes `x`, `qrel` zero as upstream's.
    pub fn new(world: &Params) -> Hinge {
        Hinge {
            anchor1: [0.0; 3],
            anchor2: [0.0; 3],
            axis1: [1.0, 0.0, 0.0],
            axis2: [1.0, 0.0, 0.0],
            qrel: [0.0; 4],
            limot: LimitMotor::new(world),
        }
    }

    /// `dJointSetHingeAnchor` (0x5b3bb0).
    pub fn set_anchor(&mut self, b: [Option<&Body>; 2], p: V3) {
        set_anchors(b, p, &mut self.anchor1, &mut self.anchor2);
        self.qrel = initial_relative_rotation(b, self.qrel);
    }

    /// `dJointSetHingeAxis`, which IW inlines (`setAxes` 0x5b2f60, then
    /// 0x5b3b40): the world axis normalized into each body's frame; a world
    /// end keeps it as given.
    pub fn set_axis(&mut self, b: [Option<&Body>; 2], axis: V3) {
        let Some(b1) = b[0] else { return };
        let q = normalize3(axis);
        self.axis1 = mul1_331(&b1.r, q);
        self.axis2 = match b[1] {
            Some(b2) => mul1_331(&b2.r, q),
            None => axis,
        };
        self.qrel = initial_relative_rotation(b, self.qrel);
    }

    /// `dJointGetHingeAngle`'s value.
    pub fn angle(&self, b: [Option<&Body>; 2]) -> f32 {
        hinge_angle(b, self.axis1, self.qrel)
    }

    pub(crate) fn info1(&mut self, b: [Option<&Body>; 2]) -> Info1 {
        let l = &self.limot;
        let mut m = if 0.0 < l.fmax { 6 } else { 5 };
        if ((l.lostop as f64) >= -PI || (l.histop as f64) <= PI) && l.lostop <= l.histop {
            let angle = hinge_angle(b, self.axis1, self.qrel);
            if self.limot.test_rotational_limit(angle) {
                m = 6;
            }
        }
        Info1 { m, nub: 5 }
    }

    pub(crate) fn info2(&self, b: [Option<&Body>; 2], info: &mut Info2) {
        set_ball(b, info, self.anchor1, self.anchor2);
        let b1 = b[0].expect("hinge attached");
        let ax1 = mul0_331(&b1.r, self.axis1);
        let (p, q) = plane_space(ax1);
        info.rows[3].j1a = p;
        info.rows[4].j1a = q;
        if b[1].is_some() {
            info.rows[3].j2a = neg(p);
            info.rows[4].j2a = neg(q);
        }
        let ax2 = match b[1] {
            Some(b2) => mul0_331(&b2.r, self.axis2),
            None => self.axis2,
        };
        let bv = cross(ax1, ax2);
        let k = (info.fps as f64 * info.erp as f64) as f32;
        info.c[3] = (dot(bv, p) as f64 * k as f64) as f32;
        info.c[4] = (k as f64 * dot(bv, q) as f64) as f32;
        self.limot.add_limot(b, info, 5, ax1);
    }
}

/// `hingeComputeInitialRelativeRotation` (0x5b3b40).
pub(crate) fn initial_relative_rotation(b: [Option<&Body>; 2], old: Quat) -> Quat {
    match b {
        [Some(b1), Some(b2)] => q_multiply1(b1.q, b2.q),
        [Some(b1), None] => [b1.q[0], -b1.q[1], -b1.q[2], -b1.q[3]],
        _ => old,
    }
}

/// `getHingeAngle` (0x5b30d0).
fn hinge_angle(b: [Option<&Body>; 2], axis: V3, q_initial: Quat) -> f32 {
    let b1 = b[0].expect("hinge attached");
    let qrel = match b[1] {
        Some(b2) => q_multiply2(q_multiply1(b1.q, b2.q), q_initial),
        None => q_multiply3(b1.q, q_initial),
    };
    angle_from_relative_quat(qrel, axis)
}

/// `getHingeAngleFromRelativeQuat` (0x5b3010): the turn about `axis`,
/// `-pi..pi`, IW storing the sine, the arctangent and the angle as floats.
fn angle_from_relative_quat(q: Quat, axis: V3) -> f32 {
    let cost2 = q[0];
    let w = |x: f32| x as f64;
    let s2 = ((w(q[1]) * w(q[1]) + w(q[2]) * w(q[2])) + w(q[3]) * w(q[3])) as f32;
    let sint2 = (w(s2).sqrt()) as f32;
    let d = dot([q[1], q[2], q[3]], axis);
    // `d < 0` takes the negative cosine; so does a NaN.
    let c = if d >= 0.0 { cost2 } else { -cost2 };
    let h = (w(sint2).atan2(w(c))) as f32;
    let mut theta = (w(h) + w(h)) as f32;
    if w(theta) > PI {
        theta = (w(theta) - TWO_PI) as f32;
    }
    -theta
}
