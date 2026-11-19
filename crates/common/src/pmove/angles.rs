//! The angle folds and conversions the moves share.

use super::cmd;
use super::state::MoveRules;
use crate::movetrace::MoveWorld;
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

/// `AngleSubtract`: `a - b` folded into -180..180.
pub(super) fn angle_subtract(a: f32, b: f32) -> f32 {
    let mut d = crate::fguard::turns(a - b);
    while d > 180.0 {
        d -= 360.0;
    }
    while d < -180.0 {
        d += 360.0;
    }
    d
}

/// `AngleNormalize180` (0x3eb70): through a truncated 16-bit angle first.
pub fn angle_normalize180(deg: f32) -> f32 {
    let short = ((deg * (65536.0 / 360.0)) as i32) & 0xffff;
    let a = short as f32 * (360.0 / 65536.0);
    if a > 180.0 { a - 360.0 } else { a }
}

/// [`angle_normalize180`] on the x87's unrounded product: the float
/// argument times rodata's 182.04445 truncated to a short, as 1.1d's
/// `AngleNormalize180` (0x3eb70) and `AngleDelta` (0x3ec9c, on `a - b`) do.
pub(super) fn short_normalize180(deg: f64) -> f32 {
    let short = ((deg * f64::from(cmd::ANGLE2SHORT)) as i32) & 0xffff;
    let a = short as f32 * (360.0 / 65536.0);
    if a > 180.0 { a - 360.0 } else { a }
}

/// `AngleNormalize360` (1.1 0x3eb30, 1.5 0x36a3c, UO 0x3dc8c) on the x87's
/// unrounded product: 0..360 on the shorts grid.
pub(super) fn short_normalize360(deg: f64) -> f32 {
    let short = ((deg * f64::from(cmd::ANGLE2SHORT)) as i32) & 0xffff;
    short as f32 * (360.0 / 65536.0)
}

/// `AngleNormalize180Accurate` (1.5 0x36b75): whole turns added while the
/// angle is at or under -180 and taken away while it is over 180, each sum
/// stored, so an angle already in range comes back as it went in.
pub(super) fn normalize180_accurate(deg: f32) -> f32 {
    let mut deg = crate::fguard::turns(deg);
    while deg <= -180.0 {
        deg += 360.0;
    }
    while deg > 180.0 {
        deg -= 360.0;
    }
    deg
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
/// pi / 180 rounded to a float (game.mp 0x3b234), as [`cmd::apply_view`]
/// does.
pub(super) fn view_rad(deg: f32) -> f32 {
    (f64::from(deg) * (std::f64::consts::PI / 180.0)) as f32
}

/// `AngleNormalize360Accurate` (1.5 0x36ae0): whole turns added or taken
/// away until the angle is in 0..360.
pub(crate) fn normalize360_accurate(deg: f32) -> f32 {
    let mut deg = crate::fguard::turns(deg);
    while deg < 0.0 {
        deg += 360.0;
    }
    while deg >= 360.0 {
        deg -= 360.0;
    }
    deg
}

/// One `delta_angles` push: `(int)(deg * 182.04445)`, truncated (1.5
/// 0x26368 and three more, control word 0xc00).
pub fn angle_shorts(deg: f32) -> i32 {
    (f64::from(deg) * f64::from(cmd::ANGLE2SHORT)) as i32 & 0xffff
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
        short_normalize180(f64::from(a - b))
    } else {
        short_normalize180(f64::from(a) - f64::from(b))
    }
}

/// Whether this build's `AngleDelta` stores the difference: any but 1.1d's.
pub(super) fn delta_stored(rules: MoveRules, world: &MoveWorld) -> bool {
    rules.v15() || world.world.arith() == crate::patch::Arith::Msvc
}
