//! The vector helpers `bg_pmove` and `bg_slidemove` call, every expression
//! on the x87 stack (here `f64`). Most round each assignment to a float as
//! CoD2's `cod2_lnxded` does (docs/research/cod2-movement.md 2); the `_kept`
//! forms and `normalize` keep the scalar on the stack, as `cod4_lnxded`
//! does (cod4-pmove.md 1), and round only the stored components.

use glam::Vec3;

#[inline]
pub(crate) fn d(x: f32) -> f64 {
    f64::from(x)
}

/// `DotProduct`: the sum stays on the stack.
pub(crate) fn dot(a: Vec3, b: Vec3) -> f64 {
    d(a.x) * d(b.x) + d(a.y) * d(b.y) + d(a.z) * d(b.z)
}

/// The two-axis dot.
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

/// `a + s * b` with `s` kept on the stack, each component stored.
pub(crate) fn ma_kept(a: Vec3, s: f64, b: Vec3) -> Vec3 {
    Vec3::new(
        (d(a.x) + s * d(b.x)) as f32,
        (d(a.y) + s * d(b.y)) as f32,
        (d(a.z) + s * d(b.z)) as f32,
    )
}

/// `s * v` with `s` kept on the stack, each component stored.
pub(crate) fn scale_kept(v: Vec3, s: f64) -> Vec3 {
    Vec3::new(
        (d(v.x) * s) as f32,
        (d(v.y) * s) as f32,
        (d(v.z) * s) as f32,
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

/// `Vec3Lerp`: `a + (b - a) * f` per component, stored. What
/// every pmove caller rebuilds a trace's end point with: CoD2's `trace_t`
/// has no `endpos` (docs/research/cod2-movement.md 3).
pub(crate) fn lerp(a: Vec3, b: Vec3, f: f32) -> Vec3 {
    let l = |a: f32, b: f32| (d(a) + (d(b) - d(a)) * d(f)) as f32;
    Vec3::new(l(a.x, b.x), l(a.y, b.y), l(a.z, b.z))
}

/// `I_sqrt`: libc `sqrt` of the float argument widened to a
/// double, stored back to a float.
pub(crate) fn sqrt(x: f64) -> f32 {
    d(x as f32).sqrt() as f32
}

/// `VectorLength`: the sum passed to [`sqrt`] as a float.
pub(crate) fn length(v: Vec3) -> f32 {
    sqrt(dot(v, v))
}

/// `Vec3Normalize` (0x81915d8): the squares' sum, the length and its
/// reciprocal kept on the stack, each component times it stored; a zero
/// vector is left alone. Returns the normalized vector and the length as
/// a float.
pub(crate) fn normalize(v: Vec3) -> (Vec3, f32) {
    let len = dot(v, v).sqrt();
    if len == 0.0 {
        return (v, 0.0);
    }
    (scale_kept(v, 1.0 / len), len as f32)
}

/// `Vec3NormalizeTo`: [`normalize`], a zero vector zeroed.
pub(crate) fn normalize_to(v: Vec3) -> (Vec3, f32) {
    let (n, len) = normalize(v);
    if len == 0.0 {
        (Vec3::ZERO, 0.0)
    } else {
        (n, len)
    }
}

/// `Vec2Normalize` on x and y: each component times `1 / len`,
/// the reciprocal kept.
pub(crate) fn normalize2(x: f32, y: f32) -> (f32, f32) {
    let len = sqrt(d(x) * d(x) + d(y) * d(y));
    if len == 0.0 {
        return (x, y);
    }
    let inv = 1.0 / d(len);
    ((d(x) * inv) as f32, (d(y) * inv) as f32)
}

/// 1 / 360 as the double constant 1.7's angle helpers multiply by.
const INV_360: f64 = 0.002777777845039964;

/// `AngleNormalize360`: the turns' fraction off `floor`, 360 taken away
/// once more when the product rounds to a whole turn.
pub(crate) fn angle_normalize360(a: f32) -> f32 {
    let scaled = (d(a) * INV_360) as f32;
    let fl = d(scaled).floor() as f32;
    let r = ((d(scaled) - d(fl)) * 360.0) as f32;
    if ((d(r) - 360.0) as f32) < 0.0 {
        r
    } else {
        (d(r) - 360.0) as f32
    }
}

/// `AngleNormalize180` as the inlined `(x - floor(x + 0.5)) * 360` of the
/// turns, each step stored.
pub(crate) fn angle_normalize180(a: f32) -> f32 {
    let scaled = (d(a) * INV_360) as f32;
    let offset = (d(scaled) + 0.5) as f32;
    let fl = d(offset).floor() as f32;
    ((d(scaled) - d(fl)) * 360.0) as f32
}

/// `AngleDelta`: `AngleNormalize180(a - b)`, the difference stored.
pub(crate) fn angle_delta(a: f32, b: f32) -> f32 {
    angle_normalize180((d(a) - d(b)) as f32)
}

/// `Vec2Normalize` on a vector's x and y, z kept.
pub(crate) fn normalize2v(v: Vec3) -> Vec3 {
    let (x, y) = normalize2(v.x, v.y);
    Vec3::new(x, y, v.z)
}

/// `vectopitch` (`cod4_lnxded` 0x8192510): `-180 / pi * atan2(z, |xy|)`
/// in 0..360, the flat length kept and passed to `atan2` as a double. The
/// stored angle always takes an add, 360 or 0 (`cmovb`, 0x8192571), so a
/// level vector's -0 comes back +0.
pub(crate) fn vectopitch(v: Vec3) -> f32 {
    if v.x == 0.0 && v.y == 0.0 {
        return if v.z > 0.0 { 270.0 } else { 90.0 };
    }
    let flat = (d(v.x) * d(v.x) + d(v.y) * d(v.y)).sqrt();
    let p = (-180.0 * d(v.z).atan2(flat) / std::f64::consts::PI) as f32;
    p + if p < 0.0 { 360.0 } else { 0.0 }
}

/// `vectoyaw` (0x8191c22): `180 / pi * atan2(y, x)` in 0..360, the add
/// as [`vectopitch`]'s.
pub(crate) fn vectoyaw(v: Vec3) -> f32 {
    if v.x == 0.0 && v.y == 0.0 {
        return 0.0;
    }
    let y = (180.0 * d(v.y).atan2(d(v.x)) / std::f64::consts::PI) as f32;
    y + if y < 0.0 { 360.0 } else { 0.0 }
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
    fn angles_wrap_off_floor() {
        assert_eq!(angle_normalize180(190.0), -170.0);
        assert_eq!(angle_normalize180(-190.0), 170.0);
        assert_eq!(angle_normalize360(-90.0), 270.0);
        // The turns' product rounds to a float: not quite 20.
        assert!((angle_delta(10.0, 350.0) - 20.0).abs() < 2e-5);
    }
}
