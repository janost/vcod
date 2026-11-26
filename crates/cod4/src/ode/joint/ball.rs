//! Contains routines ported from the Open Dynamics Engine 0.5, Copyright (C) 2001-2004
//! Russell L. Smith, under its BSD license (LICENSE-ODE-BSD.txt beside this file). See NOTICE.
//!
//! The ball and socket joint (type 1; `setBall`, 0x5b2ab0).

use super::Info2;
use crate::ode::body::Body;
use crate::ode::math::{V3, mul0_331, mul1_331, sub};

#[derive(Clone, Debug, Default)]
pub struct Ball {
    /// The anchor in each body's frame (+0x40, +0x50); for a world end, in
    /// the world's.
    pub anchor1: V3,
    pub anchor2: V3,
}

impl Ball {
    /// The ball's anchor at world point `p` (`setAnchors`, 0x5b2e30, which
    /// the swivel's creator calls).
    pub fn set_anchor(&mut self, b: [Option<&Body>; 2], p: V3) {
        set_anchors(b, p, &mut self.anchor1, &mut self.anchor2);
    }

    pub(crate) fn info2(&self, b: [Option<&Body>; 2], info: &mut Info2) {
        set_ball(b, info, self.anchor1, self.anchor2);
    }
}

/// `setAnchors` (0x5b2e30): `p` in each end's frame; a world end keeps it.
pub(crate) fn set_anchors(b: [Option<&Body>; 2], p: V3, anchor1: &mut V3, anchor2: &mut V3) {
    let Some(b1) = b[0] else { return };
    *anchor1 = mul1_331(&b1.r, sub(p, b1.pos));
    *anchor2 = match b[1] {
        Some(b2) => mul1_331(&b2.r, sub(p, b2.pos)),
        None => p,
    };
}

/// The three rows holding `anchor1` on body 1 to `anchor2` on body 2
/// (`setBall`, 0x5b2ab0).
pub(crate) fn set_ball(b: [Option<&Body>; 2], info: &mut Info2, anchor1: V3, anchor2: V3) {
    let b1 = b[0].expect("ball joint attached");
    let a1 = mul0_331(&b1.r, anchor1);
    for (k, row) in info.rows.iter_mut().take(3).enumerate() {
        row.j1l[k] = 1.0;
    }
    cross_mat(info, a1, true);
    let k = (info.fps as f64 * info.erp as f64) as f32 as f64;
    match b[1] {
        Some(b2) => {
            let a2 = mul0_331(&b2.r, anchor2);
            for (k, row) in info.rows.iter_mut().take(3).enumerate() {
                row.j2l[k] = -1.0;
            }
            cross_mat(info, a2, false);
            for j in 0..3 {
                info.c[j] = ((a2[j] as f64 + b2.pos[j] as f64 - a1[j] as f64 - b1.pos[j] as f64)
                    * k) as f32;
            }
        }
        None => {
            for j in 0..3 {
                info.c[j] = ((anchor2[j] as f64 - a1[j] as f64 - b1.pos[j] as f64) * k) as f32;
            }
        }
    }
}

/// `dCROSSMAT` into the first three rows' angular block: body 1 takes
/// `-[a]x`, body 2 `[a]x`.
fn cross_mat(info: &mut Info2, a: V3, first: bool) {
    let s = if first { -1.0f32 } else { 1.0 };
    let m = [
        [0.0, -a[2] * s, a[1] * s],
        [a[2] * s, 0.0, -a[0] * s],
        [-a[1] * s, a[0] * s, 0.0],
    ];
    for (k, row) in info.rows.iter_mut().take(3).enumerate() {
        if first {
            row.j1a = m[k];
        } else {
            row.j2a = m[k];
        }
    }
}
