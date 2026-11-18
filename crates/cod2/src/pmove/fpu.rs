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
    dot, dot2, length_stored as length, lerp_stored as lerp, ma_stored as ma,
    normalize2_kept as normalize2, scale_stored as scale, sqrt_stored as sqrt,
};

#[inline]
pub(crate) fn d(x: f32) -> f64 {
    f64::from(x)
}

/// `VectorLengthSquared` (0x80e99b4), kept.
pub(crate) fn length_sq(v: Vec3) -> f64 {
    dot(v, v)
}

/// `Vec3Normalize` (0x80a4424): the length stored, its reciprocal stored,
/// each component times it stored; a zero vector is left alone. Returns the
/// normalized vector and the length.
pub(crate) fn normalize(v: Vec3) -> (Vec3, f32) {
    let len = length(v);
    if len == 0.0 {
        return (v, 0.0);
    }
    let inv = (1.0 / d(len)) as f32;
    (scale(v, inv), len)
}

/// `Vec3NormalizeTo` (0x80a45fc): [`normalize`], a zero vector zeroed.
pub(crate) fn normalize_to(v: Vec3) -> (Vec3, f32) {
    let (n, len) = normalize(v);
    if len == 0.0 {
        (Vec3::ZERO, 0.0)
    } else {
        (n, len)
    }
}

/// `AngleNormalize360` through the short grid (0x80a8432): `182.04445 * a`
/// truncated, masked to 16 bits, times 360 / 65536.
pub(crate) fn angle_normalize360(a: f32) -> f32 {
    let short = (d(SHORT_PER_DEG) * d(a)) as i32 & 0xffff;
    (f64::from(short) * (360.0 / 65536.0)) as f32
}

/// `AngleNormalize180` (0x80a8476): [`angle_normalize360`] stored, less 360
/// over 180.
pub(crate) fn angle_normalize180(a: f32) -> f32 {
    let a = angle_normalize360(a);
    if a > 180.0 { a - 360.0 } else { a }
}

/// `AngleDelta` (0x80a85bc): `AngleNormalize180(a - b)`, the difference
/// stored as the argument.
pub(crate) fn angle_delta(a: f32, b: f32) -> f32 {
    angle_normalize180(a - b)
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

/// rodata 0x8155124: 65536 / 360 as a float, every `delta_angles` push's
/// factor.
pub(crate) const SHORT_PER_DEG: f32 = f32::from_bits(0x4336_0b61);

/// One `delta_angles` push (`PM_UpdateViewAngles` 0x80e6eb8): `(int)(deg *
/// 182.04445)` under a truncating control word, masked to 16 bits.
pub(crate) fn shorts(deg: f32) -> i32 {
    (d(SHORT_PER_DEG) * d(deg)) as i32 & 0xffff
}

/// `vectopitch` (0x80a4e5a): `-180 / pi * atan2(z, |xy|)` in 0..360, the
/// flat length through [`sqrt`].
pub(crate) fn vectopitch(v: Vec3) -> f32 {
    if v.x == 0.0 && v.y == 0.0 {
        return if v.z > 0.0 { 270.0 } else { 90.0 };
    }
    let flat = sqrt(d(v.y) * d(v.y) + d(v.x) * d(v.x));
    let p = (-180.0 * d(v.z).atan2(d(flat)) / std::f64::consts::PI) as f32;
    if p < 0.0 { p + 360.0 } else { p }
}

/// `vectoyaw` (0x80a4d5a): `180 / pi * atan2(y, x)` in 0..360.
pub(crate) fn vectoyaw(v: Vec3) -> f32 {
    if v.x == 0.0 && v.y == 0.0 {
        return 0.0;
    }
    let y = (180.0 * d(v.y).atan2(d(v.x)) / std::f64::consts::PI) as f32;
    if y < 0.0 { y + 360.0 } else { y }
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

    #[test]
    fn the_short_grid_truncates() {
        // 85 * 182.04445 = 15473.78: the pitch cap's 15473.
        assert_eq!(shorts(85.0), 15473);
        assert_eq!(shorts(-1.0), (-182i32) & 0xffff);
        assert_eq!(angle_normalize180(190.0), angle_normalize360(190.0) - 360.0);
    }
}
