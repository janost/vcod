//! Contains routines ported from the Open Dynamics Engine 0.5, Copyright (C) 2001-2004
//! Russell L. Smith, under its BSD license (LICENSE-ODE-BSD.txt beside this file). See NOTICE.
//!
//! The limit and motor of one rotational axis (`dxJointLimitMotor`): the
//! hinge's and each angular motor axis's (cod4-player-anims.md 10.4).

use super::Info2;
use crate::ode::body::Body;
use crate::ode::math::{V3, dot};
use crate::ode::quickstep::INFINITY;
use crate::ode::world::Params;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LimitMotor {
    pub vel: f32,
    pub fmax: f32,
    pub lostop: f32,
    pub histop: f32,
    pub fudge_factor: f32,
    pub normal_cfm: f32,
    pub stop_erp: f32,
    pub stop_cfm: f32,
    pub bounce: f32,
    /// 0 free, 1 at `lostop`, 2 at `histop`.
    pub limit: i32,
    pub limit_err: f32,
}

/// `dParamLoStop` and kin, the order `limot.set` (0x5b3140) switches on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Param {
    LoStop,
    HiStop,
    Vel,
    FMax,
    FudgeFactor,
    Bounce,
    Cfm,
    StopErp,
    StopCfm,
}

impl LimitMotor {
    /// `dxJointLimitMotor::init`: free, unpowered, the world's ERP and CFM.
    pub fn new(world: &Params) -> LimitMotor {
        LimitMotor {
            vel: 0.0,
            fmax: 0.0,
            lostop: -INFINITY,
            histop: INFINITY,
            fudge_factor: 1.0,
            normal_cfm: world.cfm,
            stop_erp: world.erp,
            stop_cfm: world.cfm,
            bounce: 0.0,
            limit: 0,
            limit_err: 0.0,
        }
    }

    /// `limot.set` (0x5b3140): IW stores the stops unchecked; `FMax` takes
    /// only `>= 0`, the fudge only `0..=1`.
    pub fn set(&mut self, p: Param, v: f32) {
        match p {
            Param::LoStop => self.lostop = v,
            Param::HiStop => self.histop = v,
            Param::Vel => self.vel = v,
            Param::FMax => {
                if v >= 0.0 {
                    self.fmax = v;
                }
            }
            Param::FudgeFactor => {
                if (0.0..=1.0).contains(&v) {
                    self.fudge_factor = v;
                }
            }
            Param::Bounce => self.bounce = v,
            Param::Cfm => self.normal_cfm = v,
            Param::StopErp => self.stop_erp = v,
            Param::StopCfm => self.stop_cfm = v,
        }
    }

    /// `testRotationalLimit` (0x5b3200): whether `angle` is at a stop.
    pub fn test_rotational_limit(&mut self, angle: f32) -> bool {
        if angle <= self.lostop {
            self.limit = 1;
            self.limit_err = (angle as f64 - self.lostop as f64) as f32;
            true
        } else if angle >= self.histop {
            self.limit = 2;
            self.limit_err = (angle as f64 - self.histop as f64) as f32;
            true
        } else {
            self.limit = 0;
            false
        }
    }

    /// `addLimot` (0x5b3250) for a rotational axis: fills `row` when the
    /// axis is powered or at a stop and says whether it did. Against a stop
    /// the motor turns into a constant torque on the bodies; IW reverses it
    /// at the high stop when `vel` is 0, so it presses into either stop.
    pub(crate) fn add_limot(
        &self,
        b: [Option<&Body>; 2],
        info: &mut Info2,
        row: usize,
        ax1: V3,
    ) -> bool {
        let powered = 0.0 < self.fmax;
        if !powered && self.limit == 0 {
            return false;
        }
        info.rows[row].j1a = ax1;
        if b[1].is_some() {
            info.rows[row].j2a = ax1.map(|x| -x);
        }
        if !(self.limit != 0 && self.lostop == self.histop) && powered {
            info.cfm[row] = self.normal_cfm;
            if self.limit == 0 {
                info.c[row] = self.vel;
                info.rows[row].lo = -self.fmax;
                info.rows[row].hi = self.fmax;
            } else {
                let mut fm = self.fmax;
                if self.vel > 0.0 || (self.vel == 0.0 && self.limit == 2) {
                    fm = -fm;
                }
                if (self.limit == 1 && self.vel > 0.0) || (self.limit == 2 && self.vel < 0.0) {
                    fm = (self.fudge_factor as f64 * fm as f64) as f32;
                }
                let t = |k: f32| ax1.map(|x| (k as f64 * x as f64) as f32);
                info.torque.push((0, t(-fm)));
                if b[1].is_some() {
                    info.torque.push((1, t(fm)));
                }
            }
        }
        if self.limit != 0 {
            let k = (self.stop_erp as f64 * info.fps as f64) as f32;
            info.c[row] = (-(k as f64) * self.limit_err as f64) as f32;
            info.cfm[row] = self.stop_cfm;
            let r = &mut info.rows[row];
            if self.lostop == self.histop {
                r.lo = -INFINITY;
                r.hi = INFINITY;
                return true;
            }
            if self.limit == 1 {
                r.lo = 0.0;
                r.hi = INFINITY;
            } else {
                r.lo = -INFINITY;
                r.hi = 0.0;
            }
            if 0.0 < self.bounce {
                let b1 = b[0].expect("limot body");
                let mut v = dot(b1.avel, ax1);
                if let Some(b2) = b[1] {
                    v = (v as f64 - dot(b2.avel, ax1) as f64) as f32;
                }
                let n = (-(self.bounce as f64) * v as f64) as f32;
                if self.limit == 1 && v < 0.0 && n > info.c[row] {
                    info.c[row] = n;
                }
                if self.limit == 2 && v > 0.0 && n < info.c[row] {
                    info.c[row] = n;
                }
            }
        }
        true
    }
}
