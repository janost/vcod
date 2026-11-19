//! The vector helpers `bg_pmove` and `bg_slidemove` call, every expression
//! on the x87 stack (here `f64`). Most round each assignment to a float as
//! CoD2's `cod2_lnxded` does (docs/research/cod2-movement.md 2); the `_kept`
//! forms, `normalize`, `vectopitch` and `vectoyaw` keep the scalar on the
//! stack, as `cod4_lnxded` does (cod4-pmove.md 1), and round only the stored
//! components. The angle folds are 1.7's own, off `floor`.

use glam::Vec3;
pub(crate) use vcod_common::x87::{
    angle_delta_floor as angle_delta, angle_normalize180_floor as angle_normalize180,
    angle_normalize360_floor as angle_normalize360, dot, length_stored as length,
    lerp_stored as lerp, ma_stored as ma, normalize2_kept as normalize2, scale as scale_kept,
    scale_stored as scale, sqrt_stored as sqrt, vec3_normalize_kept as normalize,
    vec3_normalize_to_kept as normalize_to, vectopitch_kept as vectopitch,
    vectoyaw_kept as vectoyaw,
};

#[inline]
pub(crate) fn d(x: f32) -> f64 {
    f64::from(x)
}

/// `a + s * b` with `s` kept on the stack, each component stored.
pub(crate) fn ma_kept(a: Vec3, s: f64, b: Vec3) -> Vec3 {
    Vec3::new(
        (d(a.x) + s * d(b.x)) as f32,
        (d(a.y) + s * d(b.y)) as f32,
        (d(a.z) + s * d(b.z)) as f32,
    )
}

/// `Vec2Normalize` on a vector's x and y, z kept.
pub(crate) fn normalize2v(v: Vec3) -> Vec3 {
    let (x, y) = normalize2(v.x, v.y);
    Vec3::new(x, y, v.z)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn angles_wrap_off_floor() {
        assert_eq!(angle_normalize180(190.0), -170.0);
        assert_eq!(angle_normalize180(-190.0), 170.0);
        assert_eq!(angle_normalize360(-90.0), 270.0);
        // The turns' product rounds to a float: not quite 20.
        assert!((angle_delta(10.0, 350.0) - 20.0).abs() < 2e-5);
    }
}
