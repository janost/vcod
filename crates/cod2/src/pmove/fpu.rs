//! The vector helpers `bg_pmove` and `bg_slidemove` call out of line, as
//! `cod2_lnxded`'s unoptimized gcc 3.3 build rounds them: every expression
//! runs on the x87 stack (here `f64`) and every assignment to a float is
//! stored, so rounded to `f32` (docs/research/cod2-movement.md 2). A
//! function that returns a float through `st(0)` (a dot product, a length)
//! hands its caller the unrounded value; the caller rounds it only where it
//! stores it.

use glam::Vec3;

#[inline]
pub(crate) fn d(x: f32) -> f64 {
    f64::from(x)
}

/// `DotProduct` (0x80e9980): the sum stays on the stack.
pub(crate) fn dot(a: Vec3, b: Vec3) -> f64 {
    d(a.x) * d(b.x) + d(a.y) * d(b.y) + d(a.z) * d(b.z)
}

/// The two-axis dot (0x80e9740 and `PM_StepSlideMove`'s 0x80eae6e).
pub(crate) fn dot2(a: Vec3, b: Vec3) -> f64 {
    d(a.x) * d(b.x) + d(a.y) * d(b.y)
}

/// `VectorMA`: `a + s * b`, each component stored.
pub(crate) fn ma(a: Vec3, s: f32, b: Vec3) -> Vec3 {
    Vec3::new(
        (d(a.x) + d(s) * d(b.x)) as f32,
        (d(a.y) + d(s) * d(b.y)) as f32,
        (d(a.z) + d(s) * d(b.z)) as f32,
    )
}

/// `VectorScale`: `s * v`, each component stored.
pub(crate) fn scale(v: Vec3, s: f32) -> Vec3 {
    Vec3::new(
        (d(s) * d(v.x)) as f32,
        (d(s) * d(v.y)) as f32,
        (d(s) * d(v.z)) as f32,
    )
}

/// `Vec3Lerp` (0x80e129e): `a + (b - a) * f` per component, stored. What
/// every pmove caller rebuilds a trace's end point with: CoD2's `trace_t`
/// has no `endpos` (docs/research/cod2-movement.md 3).
pub(crate) fn lerp(a: Vec3, b: Vec3, f: f32) -> Vec3 {
    let l = |a: f32, b: f32| (d(a) + (d(b) - d(a)) * d(f)) as f32;
    Vec3::new(l(a.x, b.x), l(a.y, b.y), l(a.z, b.z))
}

/// `I_sqrt` (0x80e9590): libc `sqrt` of the float argument widened to a
/// double, stored back to a float.
pub(crate) fn sqrt(x: f64) -> f32 {
    d(x as f32).sqrt() as f32
}

/// `VectorLength` (0x80e99e8): the sum passed to [`sqrt`] as a float.
pub(crate) fn length(v: Vec3) -> f32 {
    sqrt(dot(v, v))
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

/// `Vec2Normalize` (0x80a44c2) on x and y: each component times `1 / len`,
/// the reciprocal kept.
pub(crate) fn normalize2(x: f32, y: f32) -> (f32, f32) {
    let len = sqrt(d(x) * d(x) + d(y) * d(y));
    if len == 0.0 {
        return (x, y);
    }
    let inv = 1.0 / d(len);
    ((d(x) * inv) as f32, (d(y) * inv) as f32)
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
pub(crate) fn normalize360_accurate(mut a: f32) -> f32 {
    while a < 0.0 {
        a += 360.0;
    }
    while a >= 360.0 {
        a -= 360.0;
    }
    a
}

/// `AngleNormalize180Accurate` (0x80a8536).
pub(crate) fn normalize180_accurate(mut a: f32) -> f32 {
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
    let mut x = a - b;
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
    fn the_lerp_rounds_once_per_component() {
        let a = Vec3::new(1.0, 0.1, -3.0);
        let b = Vec3::new(2.0, 0.3, 5.0);
        let f = 0.3f32;
        let p = lerp(a, b, f);
        let x = (1.0f64 + (2.0f64 - 1.0) * f64::from(f)) as f32;
        assert_eq!(p.x.to_bits(), x.to_bits());
        assert_eq!(lerp(a, b, 1.0), b);
        assert_eq!(lerp(a, b, 0.0), a);
    }

    #[test]
    fn the_short_grid_truncates() {
        // 85 * 182.04445 = 15473.78: the pitch cap's 15473.
        assert_eq!(shorts(85.0), 15473);
        assert_eq!(shorts(-1.0), (-182i32) & 0xffff);
        assert_eq!(angle_normalize180(190.0), angle_normalize360(190.0) - 360.0);
    }
}
