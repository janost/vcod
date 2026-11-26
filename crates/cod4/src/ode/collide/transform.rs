//! Contains routines ported from the Open Dynamics Engine 0.5, Copyright (C) 2001-2004
//! Russell L. Smith, under its BSD license (LICENSE-ODE-BSD.txt beside this file). See NOTICE.
//!
//! ODE 0.5's geom transform collider (`dCollideTransform`) as iw3mp.exe 1.7
//! runs it on a class 6 geom (vtable 0x6f823c): the inner geom collided at
//! the transform's final pose (0x5aee10, the pose 0x5aed50); cod4-gsc.md
//! 3.6.6.

use super::ContactGeom;
use super::cylinder::{self, Cylinder};
use super::world::Pieces;
use crate::ode::geom::{Geom, Transform, Unported};
use crate::ode::math::{M3, V3, add, mul0_331, mul0_333};

/// 0x5aed50: the inner geom's final pose off its body's, `R F` and
/// `R p + x` (`F`, `p` the transform's frame at +0x58 and +0x88), each
/// element stored, the turned offset before the body's position is added.
pub fn final_pose(t: &Transform, body_pos: V3, body_r: &M3) -> (V3, M3) {
    let r = mul0_333(body_r, &t.r);
    let pos = add(mul0_331(body_r, t.pos), body_pos);
    (pos, r)
}

/// A cylinder geom or a transform round one (class 6 over class 13)
/// against the world at pose `pos`, `r` on a body at `body`, room for
/// `max`: a transform is 0x5aee10, its inner geom pointed at the final
/// pose (+0xc4, +0x94) and given the transform's body, then collided by
/// 0x5a95a0; `infomode` (+0x54) is 0 on every transform 0x5aeb50 makes,
/// so the contacts keep the inner geom as theirs. No other class has its
/// collider here.
#[allow(clippy::too_many_arguments)]
pub fn collide_geom(
    world: &dyn Pieces,
    g: &Geom,
    pos: V3,
    r: &M3,
    body: V3,
    narrow_max: f32,
    mask: u32,
    max: usize,
) -> Result<Vec<ContactGeom>, Unported> {
    match g {
        &Geom::Cylinder {
            direction,
            radius,
            half_height,
        } => {
            let cy = Cylinder::new(pos, r, direction, radius, half_height, body, narrow_max);
            Ok(cylinder::collide_world(world, &cy, mask, max))
        }
        Geom::Transform(t) => {
            let (pos, r) = final_pose(t, pos, r);
            collide_geom(world, &t.inner, pos, &r, body, narrow_max, mask, max)
        }
        g => Err(Unported(g.class())),
    }
}
