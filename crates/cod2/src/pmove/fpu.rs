//! The vector helpers `bg_pmove` and `bg_slidemove` call out of line, as
//! `cod2_lnxded`'s unoptimized gcc 3.3 build rounds them: every expression
//! runs on the x87 stack (here `f64`) and every assignment to a float is
//! stored, so rounded to `f32` (docs/research/cod2-movement.md 2). A
//! function that returns a float through `st(0)` (a dot product, a length)
//! hands its caller the unrounded value; the caller rounds it only where it
//! stores it.

use glam::Vec3;
use vcod_common::fguard::turns;
pub(crate) use vcod_common::x87::{
    SHORT_PER_DEG, angle_delta_short as angle_delta,
    angle_normalize180_short as angle_normalize180, dot, dot2, length_stored as length,
    lerp_stored as lerp, ma_stored as ma, normalize2_kept as normalize2, scale_stored as scale,
    sqrt_stored as sqrt, vec3_normalize_stored as normalize,
    vec3_normalize_to_stored as normalize_to, vectopitch_stored as vectopitch,
    vectoyaw_stored as vectoyaw,
};

#[inline]
pub(crate) fn d(x: f32) -> f64 {
    f64::from(x)
}

/// `VectorLengthSquared` (0x80e99b4), kept.
pub(crate) fn length_sq(v: Vec3) -> f64 {
    dot(v, v)
}

/// `AngleNormalize360Accurate` (0x80a84b8): whole turns added or taken
/// away, each sum stored.
pub(crate) fn normalize360_accurate(a: f32) -> f32 {
    let mut a = turns(a);
    while a < 0.0 {
        a += 360.0;
    }
    while a >= 360.0 {
        a -= 360.0;
    }
    a
}

/// `AngleNormalize180Accurate` (0x80a8536).
pub(crate) fn normalize180_accurate(a: f32) -> f32 {
    let mut a = turns(a);
    while a <= -180.0 {
        a += 360.0;
    }
    while a > 180.0 {
        a -= 360.0;
    }
    a
}

/// One `delta_angles` push (`PM_UpdateViewAngles` 0x80e6eb8): `(int)(deg *
/// 182.04445)` under a truncating control word, masked to 16 bits.
pub(crate) fn shorts(deg: f32) -> i32 {
    (d(SHORT_PER_DEG) * d(deg)) as i32 & 0xffff
}

/// `AngleSubtract`: the difference wrapped by whole turns.
pub(crate) fn angle_subtract(a: f32, b: f32) -> f32 {
    let mut x = turns(a - b);
    while x > 180.0 {
        x -= 360.0;
    }
    while x < -180.0 {
        x += 360.0;
    }
    x
}

#[cfg(test)]
mod tests {
    use super::*;
    use vcod_common::x87::angle_normalize360_short as angle_normalize360;

    #[test]
    fn the_short_grid_truncates() {
        // 85 * 182.04445 = 15473.78: the pitch cap's 15473.
        assert_eq!(shorts(85.0), 15473);
        assert_eq!(shorts(-1.0), (-182i32) & 0xffff);
        assert_eq!(angle_normalize180(190.0), angle_normalize360(190.0) - 360.0);
    }
}
