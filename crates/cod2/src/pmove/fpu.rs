//! The vector helpers `bg_pmove` and `bg_slidemove` call out of line, as
//! `cod2_lnxded`'s unoptimized gcc 3.3 build rounds them: every expression
//! runs on the x87 stack (here `f64`) and every assignment to a float is
//! stored, so rounded to `f32` (docs/research/cod2-movement.md 2). A
//! function that returns a float through `st(0)` (a dot product, a length)
//! hands its caller the unrounded value; the caller rounds it only where it
//! stores it.

use glam::Vec3;
pub(crate) use vcod_common::x87::{
    SHORT_PER_DEG, angle_delta_short as angle_delta,
    angle_normalize180_short as angle_normalize180, angle_shorts as shorts, angle_subtract, dot,
    dot2, length_stored as length, lerp_stored as lerp, ma_stored as ma,
    normalize2_kept as normalize2, normalize180_accurate, normalize360_accurate,
    scale_stored as scale, sqrt_stored as sqrt, vec3_normalize_stored as normalize,
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
