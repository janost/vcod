//! Contains routines ported from the Open Dynamics Engine 0.5, Copyright (C) 2001-2004
//! Russell L. Smith, under its BSD license (LICENSE-ODE-BSD.txt beside this file). See NOTICE.
//!
//! ODE 0.5's geom transform collider (`dCollideTransform`) as iw3mp.exe 1.7
//! runs it on a class 6 geom (vtable 0x6f823c): the inner geom collided at
//! the transform's final pose (0x5aee10, the pose 0x5aed50); cod4-gsc.md
//! 3.6.6.

use super::ContactGeom;
use super::world::Pieces;
use crate::ode::geom::Transform;
use crate::ode::math::{M3, V3, add, mul0_331, mul0_333};

/// 0x5aed50: the inner geom's final pose off its body's, `R F` and
/// `R p + x` (`F`, `p` the transform's frame at +0x58 and +0x88), each
/// element stored, the turned offset before the body's position is added.
pub fn final_pose(t: &Transform, body_pos: V3, body_r: &M3) -> (V3, M3) {
    let r = mul0_333(body_r, &t.r);
    let pos = add(mul0_331(body_r, t.pos), body_pos);
    (pos, r)
}

/// 0x5aee10 on a transform at its body's pose `pos`, `r`: its inner geom
/// pointed at the final pose (+0xc4, +0x94), given the transform's body
/// and collided by 0x5a95a0 ([`super::collide_geom`]); `infomode` (+0x54)
/// is 0 on every transform 0x5aeb50 makes, so the contacts keep the inner
/// geom as theirs.
#[allow(clippy::too_many_arguments)]
pub fn collide(
    world: &dyn Pieces,
    t: &Transform,
    pos: V3,
    r: &M3,
    body: V3,
    narrow_max: f32,
    mask: u32,
    max: usize,
) -> Vec<ContactGeom> {
    let (pos, r) = final_pose(t, pos, r);
    super::collide_geom(world, &t.inner, pos, &r, body, narrow_max, mask, max)
}
