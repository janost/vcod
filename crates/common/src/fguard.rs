//! Guards for retail's float idioms on values a server or a file supplies.
//! Retail folds an angle with `while a > 180 { a -= 360 }`, which never
//! ends once `a - 360 == a` (from about 2^33, or at infinity), and clamps
//! with two compares, which Rust's `f32::clamp` turns into a panic when the
//! bounds cross or one is NaN. Each guard returns its input untouched for
//! every value a retail game produces, so the ported arithmetic keeps its
//! bits.

/// Past this many degrees (364 turns) an angle fold is reduced by one `%`
/// first. No game angle comes near it.
pub const FOLD_LIMIT: f32 = 131_072.0;

/// An angle a retail `while`-by-360 fold finishes in a turn or two: the
/// angle itself up to [`FOLD_LIMIT`], its remainder by 360 past it, 0 when
/// it is not finite.
pub fn turns(a: f32) -> f32 {
    if a.abs() <= FOLD_LIMIT {
        a
    } else if a.is_finite() {
        a % 360.0
    } else {
        0.0
    }
}

/// [`turns`] for an angle held as a double.
pub fn turns64(a: f64) -> f64 {
    if a.abs() <= f64::from(FOLD_LIMIT) {
        a
    } else if a.is_finite() {
        a % 360.0
    } else {
        0.0
    }
}

/// `target` for a `while target - cur > 180 { target -= 360 }` fold:
/// itself while within [`FOLD_LIMIT`] of `cur`, else `cur` plus the
/// [`turns`] of the gap.
pub fn near(target: f32, cur: f32) -> f32 {
    let gap = target - cur;
    if gap.abs() <= FOLD_LIMIT {
        target
    } else {
        cur + turns(gap)
    }
}

/// `v` held to `lo..=hi` by two compares, low bound first as `f32::clamp`
/// does: its result whenever `lo <= hi`, and no panic when the bounds cross
/// or one is NaN.
pub fn clamp(v: f32, lo: f32, hi: f32) -> f32 {
    let mut v = v;
    if v < lo {
        v = lo;
    }
    if v > hi {
        v = hi;
    }
    v
}

/// `v`, or 0 when it is NaN or infinite: a wire float no retail server
/// sends.
pub fn finite(v: f32) -> f32 {
    if v.is_finite() { v } else { 0.0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fold180(mut a: f32) -> f32 {
        a = turns(a);
        while a > 180.0 {
            a -= 360.0;
        }
        while a < -180.0 {
            a += 360.0;
        }
        a
    }

    #[test]
    fn in_range_angles_keep_their_bits() {
        for a in [
            0.0f32, -0.0, 179.99, 180.0, -180.0, 359.5, 721.3, -1000.25, FOLD_LIMIT,
        ] {
            assert_eq!(turns(a).to_bits(), a.to_bits());
            assert_eq!(near(a, 10.0).to_bits(), a.to_bits());
        }
    }

    #[test]
    fn hostile_angles_fold_at_once() {
        for a in [
            1e10f32,
            -1e30,
            f32::MAX,
            f32::INFINITY,
            f32::NEG_INFINITY,
            f32::NAN,
        ] {
            let f = fold180(a);
            assert!(f.is_finite() && (-180.0..=180.0).contains(&f), "{a} -> {f}");
        }
        assert_eq!(near(f32::INFINITY, 5.0), 5.0);
        assert!(near(1e30, 0.0).abs() <= 360.0);
    }

    #[test]
    fn clamp_matches_std_and_survives_crossed_bounds() {
        for v in [-5.0f32, -1.0, 0.0, 0.5, 1.0, 7.0] {
            assert_eq!(clamp(v, -1.0, 1.0), v.clamp(-1.0, 1.0));
        }
        assert_eq!(clamp(0.5, 1.0, -1.0), -1.0);
        assert_eq!(clamp(0.5, f32::NAN, f32::NAN), 0.5);
        assert!(clamp(f32::NAN, -1.0, 1.0).is_nan());
    }
}
