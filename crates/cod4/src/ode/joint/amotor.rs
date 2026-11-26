//! Contains routines ported from the Open Dynamics Engine 0.5, Copyright (C) 2001-2004
//! Russell L. Smith, under its BSD license (LICENSE-ODE-BSD.txt beside this file). See NOTICE.
//!
//! The angular motor (IW's type 8; `amotorGetInfo1` 0x5b5680,
//! `amotorGetInfo2` 0x5b5750) in Euler mode, the swivel's three limited,
//! frictioned axes (cod4-player-anims.md 10.3).

use super::limot::LimitMotor;
use super::{Info1, Info2};
use crate::ode::body::Body;
use crate::ode::math::{V3, cross, dot, mul0_331, mul1_331, normalize3};
use crate::ode::world::Params;

/// Which frame an axis is given in (`rel`): the world, body 1 or body 2.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rel {
    Global = 0,
    Body1 = 1,
    Body2 = 2,
}

#[derive(Clone, Debug)]
pub struct AMotor {
    pub num: usize,
    pub rel: [Rel; 3],
    /// Axis 0 in body 1's frame, axis 2 in body 2's (+0x54, +0x74).
    pub axis: [V3; 3],
    pub limot: [LimitMotor; 3],
    /// Axis 2 in body 1's frame and axis 0 in body 2's at the start
    /// (+0x108, +0x118).
    pub reference1: V3,
    pub reference2: V3,
}

impl AMotor {
    /// `amotorInit` (0x5b5060) then Euler mode with three axes, as the
    /// swivel's creator sets it (0x5a7510). IW keeps no angles field.
    pub fn euler(world: &Params) -> AMotor {
        AMotor {
            num: 3,
            rel: [Rel::Global; 3],
            axis: [[0.0; 3]; 3],
            limot: [LimitMotor::new(world); 3],
            reference1: [0.0; 3],
            reference2: [0.0; 3],
        }
    }

    /// `dJointSetAMotorAxis` (0x5b5860): the world axis `a` into `rel`'s
    /// frame, normalized; a `Body2` axis without a body 2 reads `Body1`.
    pub fn set_axis(&mut self, b: [Option<&Body>; 2], anum: usize, rel: Rel, a: V3) {
        let anum = anum.min(2);
        let rel = if b[1].is_none() && rel == Rel::Body2 {
            Rel::Body1
        } else {
            rel
        };
        self.rel[anum] = rel;
        let v = match rel {
            Rel::Body1 => mul1_331(&b[0].expect("amotor attached").r, a),
            Rel::Body2 => mul1_331(&b[1].expect("amotor body 2").r, a),
            Rel::Global => a,
        };
        self.axis[anum] = normalize3(v);
        self.set_euler_reference_vectors(b);
    }

    /// `amotorSetEulerReferenceVectors` (0x5b55c0).
    fn set_euler_reference_vectors(&mut self, b: [Option<&Body>; 2]) {
        match b {
            [Some(b1), Some(b2)] => {
                self.reference1 = mul1_331(&b1.r, mul0_331(&b2.r, self.axis[2]));
                self.reference2 = mul1_331(&b2.r, mul0_331(&b1.r, self.axis[0]));
            }
            [Some(b1), None] => {
                self.reference1 = mul1_331(&b1.r, self.axis[2]);
                self.reference2 = mul0_331(&b1.r, self.axis[0]);
            }
            _ => {}
        }
    }

    /// `amotorComputeGlobalAxes` (0x5b5120), Euler mode: axis 1 is
    /// `ax2 x ax0`, normalized.
    fn global_axes(&self, b: [Option<&Body>; 2]) -> [V3; 3] {
        let b1 = b[0].expect("amotor attached");
        let ax0 = mul0_331(&b1.r, self.axis[0]);
        let ax2 = match b[1] {
            Some(b2) => mul0_331(&b2.r, self.axis[2]),
            None => self.axis[2],
        };
        [ax0, normalize3(cross(ax2, ax0)), ax2]
    }

    /// `amotorComputeEulerAngles` (0x5b52e0); each arctangent is stored,
    /// then negated.
    pub fn euler_angles(&self, b: [Option<&Body>; 2]) -> [f32; 3] {
        let ax = self.global_axes(b);
        let b1 = b[0].expect("amotor attached");
        let ref1 = mul0_331(&b1.r, self.reference1);
        let ref2 = match b[1] {
            Some(b2) => mul0_331(&b2.r, self.reference2),
            None => self.reference2,
        };
        let atan = |y: f32, x: f32| -(((y as f64).atan2(x as f64)) as f32);
        let q = cross(ax[0], ref1);
        let a0 = atan(dot(ax[2], q), dot(ax[2], ref1));
        let q = cross(ax[0], ax[1]);
        let a1 = atan(dot(ax[2], ax[0]), dot(ax[2], q));
        let q = cross(ax[1], ax[2]);
        let a2 = atan(dot(ref2, ax[1]), dot(ref2, q));
        [a0, a1, a2]
    }

    /// `amotorGetInfo1` (0x5b5680), its limit test inlined: an angle not
    /// above `lostop` (NaN too) is at the low stop.
    pub(crate) fn info1(&mut self, b: [Option<&Body>; 2]) -> Info1 {
        let angle = self.euler_angles(b);
        let mut m = 0;
        for (l, &a) in self.limot.iter_mut().zip(&angle).take(self.num) {
            if l.lostop.partial_cmp(&a) != Some(std::cmp::Ordering::Less) {
                l.limit = 1;
                l.limit_err = (a as f64 - l.lostop as f64) as f32;
                m += 1;
            } else if l.histop <= a {
                l.limit = 2;
                l.limit_err = (a as f64 - l.histop as f64) as f32;
                m += 1;
            } else {
                l.limit = 0;
                if l.fmax > 0.0 {
                    m += 1;
                }
            }
        }
        Info1 { m, nub: 0 }
    }

    /// `amotorGetInfo2` (0x5b5750): Euler mode holds `ax1 x ax2`, `ax1`
    /// and `ax0 x ax1`.
    pub(crate) fn info2(&self, b: [Option<&Body>; 2], info: &mut Info2) {
        let ax = self.global_axes(b);
        let axes = [cross(ax[1], ax[2]), ax[1], cross(ax[0], ax[1])];
        let mut row = 0;
        for (l, &a) in self.limot.iter().zip(&axes).take(self.num) {
            if l.add_limot(b, info, row, a) {
                row += 1;
            }
        }
    }
}
