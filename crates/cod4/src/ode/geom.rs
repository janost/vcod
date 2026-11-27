//! Contains routines ported from the Open Dynamics Engine 0.5, Copyright (C) 2001-2004
//! Russell L. Smith, under its BSD license (LICENSE-ODE-BSD.txt beside this file). See NOTICE.
//!
//! The geoms a world's bodies carry, as iw3mp.exe 1.7 builds them
//! (cod4-gsc.md 3.6.3): ODE's box (class 1) and geom transform (class 6),
//! and IW's own classes 12..14 (one vtable, 0x6f81c4, with class 11, a
//! brush model's, which no world 0 or 1 body here takes). Every placeable
//! geom reads its body's position and rotation (+0x18, +0x1c); the fields
//! here are the class's own, from +0x4c.

use std::sync::Arc;

use super::math::{M3, V3};
use crate::assets::model::PhysBrush;

#[inline]
fn w(x: f32) -> f64 {
    x as f64
}

/// `sqrt(1/2)` as a float widened (the double at 0x70ae70).
const HALF_SQRT2: f64 = 0.707_106_769_084_930_4;
/// A brush's bounds' share in its sweep size (the double at 0x70afb8, 0.01f).
const BRUSH_SWEEP_SHARE: f64 = 0.009_999_999_776_482_582;

/// A geom's class (+4).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GeomClass {
    Box = 1,
    Transform = 6,
    Brush = 12,
    Cylinder = 13,
    Capsule = 14,
}

/// A geom of a body. A body's geoms are newest first, as ODE links them
/// (body +0x1c, then geom +0x14).
#[derive(Clone, Debug, PartialEq)]
pub enum Geom {
    /// `dCreateBox` (0x5aa500, vtable 0x6f822c): the full side lengths
    /// (+0x4c..+0x54).
    Box { sides: V3 },
    /// 0x5a8d90: the model's brush (its `BrushWrapper`, +0x4c) and the
    /// mass centre the body sits at, in the model's frame (+0x50).
    Brush { brush: Arc<PhysBrush>, offset: V3 },
    /// 0x5a8dd0: its axis, body axis 1..=3 (+0x4c), radius (+0x50) and
    /// half height (+0x54), which the mass (0x5b5dc0) takes as the length.
    Cylinder {
        direction: usize,
        radius: f32,
        half_height: f32,
    },
    /// 0x5a8e10: radius (+0x50), half length (+0x54), along body x.
    Capsule { radius: f32, half_len: f32 },
    /// 0x5aeb50 (vtable 0x6f823c): another geom in a frame of its own
    /// within the body's.
    Transform(Box<Transform>),
}

/// The class 6 geom's fields: ODE 0.5's `dxGeomTransform` with a frame
/// IW put before the final pose.
#[derive(Clone, Debug, PartialEq)]
pub struct Transform {
    /// +0x4c: no body of its own (+0x10 0), its position and rotation
    /// pointers the body's; the transform owns it (`cleanup` +0x50 1,
    /// `infomode` +0x54 0).
    pub inner: Geom,
    /// +0x58: the inner geom's rotation in the body's frame, 3x4 by rows.
    pub r: M3,
    /// +0x88: its position in the body's frame.
    pub pos: V3,
}

impl Geom {
    pub fn class(&self) -> GeomClass {
        match self {
            Geom::Box { .. } => GeomClass::Box,
            Geom::Brush { .. } => GeomClass::Brush,
            Geom::Cylinder { .. } => GeomClass::Cylinder,
            Geom::Capsule { .. } => GeomClass::Capsule,
            Geom::Transform(_) => GeomClass::Transform,
        }
    }

    /// 0x5a9a20's box about the body, `-m..m` on each axis: a box's half
    /// its least side (0x5a9a44, ties keep the earlier), a cylinder's
    /// `min(r sqrt(1/2), +0x54)` (0x5a688b), a capsule's `r sqrt(1/2)`
    /// unless `2 r + h` is less (0x5a68b7), a transform its inner geom's
    /// (0x5aed10); a brush's [`brush_bounds`].
    pub fn bounds(&self) -> (V3, V3) {
        let m = match self {
            Geom::Box { sides } => {
                let mut m = sides[0];
                for &s in &sides[1..] {
                    if s < m {
                        m = s;
                    }
                }
                (w(m) * 0.5) as f32
            }
            Geom::Cylinder {
                radius,
                half_height,
                ..
            } => {
                let t = (w(*radius) * HALF_SQRT2) as f32;
                if *half_height < t { *half_height } else { t }
            }
            &Geom::Capsule { radius, half_len } => {
                let t = (w(radius) * HALF_SQRT2) as f32;
                let s = w(radius) * 2.0 + w(half_len);
                if s < w(t) { s as f32 } else { t }
            }
            Geom::Transform(t) => return t.inner.bounds(),
            Geom::Brush { brush, .. } => return brush_bounds(brush),
        };
        ([-m; 3], [m; 3])
    }
}

/// 0x5a684f: a brush's bounds times 0.01, each stored, then 0x561910 cuts
/// x to z's size about its middle when z is the shorter and y when y is
/// the longer, by half the stored difference of the stored sizes.
fn brush_bounds(b: &PhysBrush) -> (V3, V3) {
    let mut lo = b.mins.map(|x| (w(x) * BRUSH_SWEEP_SHARE) as f32);
    let mut hi = b.maxs.map(|x| (w(x) * BRUSH_SWEEP_SHARE) as f32);
    let s: V3 = std::array::from_fn(|k| (w(hi[k]) - w(lo[k])) as f32);
    let mut cut = |k: usize, by: f32| {
        let h = ((w(s[k]) - w(by)) * 0.5) as f32;
        lo[k] = (w(lo[k]) + w(h)) as f32;
        hi[k] = (w(hi[k]) - w(h)) as f32;
    };
    if s[2] < s[0] {
        cut(0, s[2]);
    }
    if s[1] > s[2] {
        cut(1, s[2]);
    }
    (lo, hi)
}
