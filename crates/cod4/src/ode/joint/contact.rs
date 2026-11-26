//! Contains routines ported from the Open Dynamics Engine 0.5, Copyright (C) 2001-2004
//! Russell L. Smith, under its BSD license (LICENSE-ODE-BSD.txt beside this file). See NOTICE.
//!
//! The contact joint (type 4; `contactGetInfo1` 0x5b4270,
//! `contactGetInfo2` 0x5b4420): a normal row and up to two friction rows
//! (cod4-player-anims.md 10.3).

use super::{Info1, Info2};
use crate::ode::body::Body;
use crate::ode::math::{V3, cross, dot, neg, plane_space, sub};
use crate::ode::quickstep::INFINITY;
use crate::ode::world::Params;

/// `dContactMu2` and kin, upstream's bits.
pub const MU2: u32 = 0x001;
pub const FDIR1: u32 = 0x002;
pub const BOUNCE: u32 = 0x004;
pub const SOFT_ERP: u32 = 0x008;
pub const SOFT_CFM: u32 = 0x010;
pub const MOTION1: u32 = 0x020;
pub const MOTION2: u32 = 0x040;
pub const SLIP1: u32 = 0x080;
pub const SLIP2: u32 = 0x100;
pub const APPROX1_1: u32 = 0x1000;
pub const APPROX1_2: u32 = 0x2000;

/// `dSurfaceParameters` (joint +0x44).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Surface {
    pub mode: u32,
    pub mu: f32,
    pub mu2: f32,
    pub bounce: f32,
    pub bounce_vel: f32,
    pub soft_erp: f32,
    pub soft_cfm: f32,
    pub motion1: f32,
    pub motion2: f32,
    pub slip1: f32,
    pub slip2: f32,
}

/// A contact (`dContact` without the geoms): where, along which normal
/// (from body 2 into body 1) and how deep.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Contact {
    pub surface: Surface,
    pub pos: V3,
    pub normal: V3,
    pub depth: f32,
    /// The first friction direction under `FDIR1`. IW's create (0x5b7210)
    /// never copies one in.
    pub fdir1: V3,
    /// `the_m`: the rows `info1` counted.
    the_m: usize,
}

impl Contact {
    pub fn new(surface: Surface, pos: V3, normal: V3, depth: f32) -> Contact {
        Contact {
            surface,
            pos,
            normal,
            depth,
            fdir1: [0.0; 3],
            the_m: 0,
        }
    }

    pub(crate) fn info1(&mut self) -> Info1 {
        let s = &mut self.surface;
        let mut m = 1;
        let mut nub = 0;
        if s.mu < 0.0 {
            s.mu = 0.0;
        }
        if s.mode & MU2 != 0 {
            if s.mu > 0.0 {
                m = 2;
            }
            if s.mu2 < 0.0 {
                s.mu2 = 0.0;
            }
            if s.mu2 > 0.0 {
                m += 1;
            }
            if s.mu == INFINITY {
                nub = 1;
            }
            if s.mu2 == INFINITY {
                nub += 1;
            }
        } else {
            if s.mu > 0.0 {
                m = 3;
            }
            if s.mu == INFINITY {
                nub = 2;
            }
        }
        self.the_m = m;
        Info1 { m, nub }
    }

    pub(crate) fn info2(
        &self,
        b: [Option<&Body>; 2],
        reverse: bool,
        params: &Params,
        info: &mut Info2,
    ) {
        let s = &self.surface;
        let b1 = b[0].expect("contact attached");
        let n = if reverse {
            neg(self.normal)
        } else {
            self.normal
        };
        let c1 = sub(self.pos, b1.pos);
        let c2 = b[1].map(|b2| sub(self.pos, b2.pos));
        let row = |info: &mut Info2, k: usize, t: V3| {
            let r = &mut info.rows[k];
            r.j1l = t;
            r.j1a = cross(c1, t);
            if let Some(c2) = c2 {
                r.j2l = neg(t);
                r.j2a = neg(cross(c2, t));
            }
        };
        row(info, 0, n);

        let erp = if s.mode & SOFT_ERP != 0 {
            s.soft_erp
        } else {
            info.erp
        };
        let k = (info.fps as f64 * erp as f64) as f32;
        let mut depth = (self.depth as f64 - params.contact_min_depth as f64) as f32;
        if depth < 0.0 {
            depth = 0.0;
        }
        // The cap compares against the product before it is stored.
        let kd = depth as f64 * k as f64;
        info.c[0] = if (params.contact_max_vel as f64) < kd {
            params.contact_max_vel
        } else {
            kd as f32
        };
        if s.mode & SOFT_CFM != 0 {
            info.cfm[0] = s.soft_cfm;
        }
        if s.mode & BOUNCE != 0 {
            let r = info.rows[0];
            let mut out = (dot(r.j1l, b1.lvel) as f64 + dot(r.j1a, b1.avel) as f64) as f32;
            if let Some(b2) = b[1] {
                out =
                    ((dot(r.j2l, b2.lvel) as f64 + dot(r.j2a, b2.avel) as f64) + out as f64) as f32;
            }
            if s.bounce_vel >= 0.0 && s.bounce_vel < -out {
                let nc = (-(s.bounce as f64) * out as f64) as f32;
                if info.c[0] < nc {
                    info.c[0] = nc;
                }
            }
        }
        info.rows[0].lo = 0.0;
        info.rows[0].hi = INFINITY;

        if self.the_m >= 2 {
            let (t1, t2) = if s.mode & FDIR1 != 0 {
                (self.fdir1, cross(n, self.fdir1))
            } else {
                plane_space(n)
            };
            row(info, 1, t1);
            if s.mode & MOTION1 != 0 {
                info.c[1] = s.motion1;
            }
            info.rows[1].lo = -s.mu;
            info.rows[1].hi = s.mu;
            if s.mode & APPROX1_1 != 0 {
                info.findex[1] = 0;
            }
            if s.mode & SLIP1 != 0 {
                info.cfm[1] = s.slip1;
            }
            if self.the_m >= 3 {
                row(info, 2, t2);
                if s.mode & MOTION2 != 0 {
                    info.c[2] = s.motion2;
                }
                let mu = if s.mode & MU2 != 0 { s.mu2 } else { s.mu };
                info.rows[2].lo = -mu;
                info.rows[2].hi = mu;
                if s.mode & APPROX1_2 != 0 {
                    info.findex[2] = 0;
                }
                if s.mode & SLIP2 != 0 {
                    info.cfm[2] = s.slip2;
                }
            }
        }
    }
}
