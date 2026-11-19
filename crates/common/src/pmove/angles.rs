//! Contains routines ported from the Quake III Arena GPL source, Copyright (C) 1999-2005 Id Software, Inc.,
//! and the RTCW-MP GPL source, Copyright (C) 1999-2010 id Software LLC, a ZeniMax Media company. See NOTICE.
//!
//! The angle folds and conversions the moves share.

use super::state::MoveRules;
use crate::movetrace::MoveWorld;
/// `AngleNormalize180` (0x3eb70): the float argument's product with
/// `ANGLE2SHORT` kept into the truncation to a short.
pub use crate::x87::angle_normalize180_short as angle_normalize180;
pub use crate::x87::angle_shorts;
pub(crate) use crate::x87::normalize360_accurate;
pub(super) use crate::x87::{
    angle_normalize180_short, angle_normalize360_short, angle_subtract, normalize180_accurate,
};
use glam::Vec3;

/// `vectopitch` (0x3dd7c): wire convention (positive down), in 0..360.
pub(super) fn vectopitch(v: Vec3) -> f32 {
    if v.x == 0.0 && v.y == 0.0 {
        return if v.z > 0.0 { 270.0 } else { 90.0 };
    }
    let len = (v.x * v.x + v.y * v.y).sqrt();
    let p = (-f64::from(v.z).atan2(f64::from(len)) * 180.0 / std::f64::consts::PI) as f32;
    if p < 0.0 { p + 360.0 } else { p }
}

/// A view angle in degrees off the sim's radians. Retail holds degrees,
/// and every view the prone, ladder and entry code reads is on the shorts
/// grid (`SHORT2ANGLE` of the cmd, or a 1.1 cap's
/// `AngleNormalize180/360`), which the radians miss by up to 0.005 of a
/// short near 360 degrees: one under the grid truncates a whole short away
/// in `AngleDelta`. An angle further off the grid is kept.
pub(crate) fn view_deg(rad: f32) -> f32 {
    let deg = rad.to_degrees();
    let shorts = f64::from(deg) * (65536.0 / 360.0);
    let grid = shorts.round();
    if (shorts - grid).abs() < 0.05 {
        (grid * (360.0 / 65536.0)) as f32
    } else {
        deg
    }
}

/// Degrees to the sim's radians as `AngleVectors` takes them, by the double
/// pi / 180 rounded to a float (game.mp 0x3b234), as [`super::cmd::apply_view`]
/// does.
pub(super) fn view_rad(deg: f32) -> f32 {
    (f64::from(deg) * (std::f64::consts::PI / 180.0)) as f32
}

/// `vectoyaw` (1.1 0x3dccc, 1.5 0x338ab): the double atan2's degrees
/// rounded to a float, 360 added to a negative one, 0 straight up or down.
/// 1.1 hands the sum back on the x87 stack, so it is left unrounded here.
pub(super) fn vectoyaw(v: Vec3) -> f64 {
    if v.x == 0.0 && v.y == 0.0 {
        return 0.0;
    }
    let yaw = f64::from(f64::from(v.y).atan2(f64::from(v.x)).to_degrees() as f32);
    if yaw < 0.0 { yaw + 360.0 } else { yaw }
}

/// `AngleDelta`: `a - b` through a truncated 16-bit angle, in -180..180.
/// 1.1d's (0x3ec9c) subtracts on the x87 stack; 1.5b's (0x36c0a) and UO's
/// (0x3de5a) store the float difference and call `AngleNormalize180`, which
/// truncates the same way, and so does every Windows cgame, whose
/// `AngleNormalize180` takes the difference as a float argument
/// ([`delta_stored`]; cod11-mantle.md, "AngleDelta").
pub(crate) fn angle_delta(a: f32, b: f32, stored: bool) -> f32 {
    if stored {
        crate::x87::angle_delta_short(a, b)
    } else {
        crate::x87::angle_delta_kept(a, b)
    }
}

/// Whether this build's `AngleDelta` stores the difference: any but 1.1d's.
pub(super) fn delta_stored(rules: MoveRules, world: &MoveWorld) -> bool {
    rules.v15() || world.world.arith() == crate::patch::Arith::Msvc
}
