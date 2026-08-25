//! Listener-relative gain and pan (docs/research/cod11-sound-system.md,
//! section 3).

use glam::Vec3;

pub struct Listener {
    pub pos: Vec3,
    /// [`pan`] reads `right` alone; `forward` and `up` only serve the tests.
    #[cfg_attr(not(test), allow(dead_code))]
    pub forward: Vec3,
    pub right: Vec3,
    #[cfg_attr(not(test), allow(dead_code))]
    pub up: Vec3,
}

impl Default for Listener {
    /// Matches `camera::basis(0.0, 0.0)`, +X forward, -Y right, +Z up.
    fn default() -> Self {
        Listener {
            pos: Vec3::ZERO,
            forward: Vec3::X,
            right: -Vec3::Y,
            up: Vec3::Z,
        }
    }
}

/// `1` inside `dist_min`, linear to `0` at `dist_max`, `0` beyond. Linear in
/// amplitude, `FUN_0044cb90` @ `CoDMP.exe 0x44cb90` (research doc, section
/// 3). `dist_max <= 0` is defensive; the parser never yields it.
pub fn falloff(dist: f32, dist_min: f32, dist_max: f32) -> f32 {
    // A NaN wire position; keep it out of the gain.
    if !dist.is_finite() {
        return 0.0;
    }
    if dist_max <= 0.0 || dist <= dist_min {
        return 1.0;
    }
    if dist >= dist_max {
        return 0.0;
    }
    1.0 - (dist - dist_min) / (dist_max - dist_min)
}

/// -1 left, 0 centre, 1 right. The engine's stream pan `(1 - u . left) * 0.5`,
/// `FUN_0044c240` @ `CoDMP.exe 0x44c240` (research doc, section 3), remapped
/// from `0..1`.
pub fn pan(listener: &Listener, emitter: Vec3) -> f32 {
    let d = emitter - listener.pos;
    let len = d.length();
    // On the listener, or non-finite; a NaN pan poisons kira's mix.
    if !len.is_finite() || len <= 1e-3 {
        return 0.0;
    }
    (d / len).dot(listener.right).clamp(-1.0, 1.0)
}

/// Linear amplitude to dB, floored at kira's -60 dB silence.
pub fn amplitude_db(a: f32) -> f32 {
    if a <= 0.001 {
        return -60.0;
    }
    (20.0 * a.log10()).max(-60.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn falloff_worked_examples() {
        // Defaults, 120 / 600 (research doc, section 3 table).
        assert_eq!(falloff(0.0, 120.0, 600.0), 1.0);
        assert_eq!(falloff(120.0, 120.0, 600.0), 1.0);
        assert!((falloff(360.0, 120.0, 600.0) - 0.5).abs() < 1e-5);
        assert_eq!(falloff(600.0, 120.0, 600.0), 0.0);
        assert_eq!(falloff(5000.0, 120.0, 600.0), 0.0);

        // step_run_dirt, 50 / 1000.
        assert!((falloff(525.0, 50.0, 1000.0) - 0.5).abs() < 1e-5);

        // bullet_small_dirt, 150 / 700.
        assert!((falloff(425.0, 150.0, 700.0) - 0.5).abs() < 1e-5);

        // land_dirt, 80 / 120.
        assert!((falloff(100.0, 80.0, 120.0) - 0.5).abs() < 1e-5);
    }

    #[test]
    fn falloff_without_cutoff_is_full_everywhere() {
        assert_eq!(falloff(99999.0, 120.0, 0.0), 1.0);
    }

    #[test]
    fn falloff_degenerate_range() {
        assert_eq!(falloff(50.0, 100.0, 100.0), 1.0);
        assert_eq!(falloff(150.0, 100.0, 100.0), 0.0);
    }

    #[test]
    fn listener_default_matches_camera_basis_at_yaw_pitch_zero() {
        let l = Listener::default();
        assert_eq!(l.pos, Vec3::ZERO);
        assert_eq!(l.forward, Vec3::X);
        assert_eq!(l.right, -Vec3::Y);
        assert_eq!(l.up, Vec3::Z);
    }

    #[test]
    fn pan_follows_the_listener_right_vector() {
        let l = Listener::default();
        assert!((pan(&l, l.right * 100.0) - 1.0).abs() < 1e-5);
        assert!((pan(&l, -l.right * 100.0) + 1.0).abs() < 1e-5);
        assert!(pan(&l, l.forward * 100.0).abs() < 1e-5);
        assert_eq!(pan(&l, l.pos), 0.0); // on top of the listener: centered
    }

    #[test]
    fn pan_reproduces_the_engine_pan01_formula() {
        // pan01 = (1 + pan) / 2: 0.0 left, 1.0 right, 0.5 ahead/behind.
        let l = Listener::default();
        assert!(((1.0 + pan(&l, l.right * 100.0)) / 2.0 - 1.0).abs() < 1e-5);
        assert!(((1.0 + pan(&l, -l.right * 100.0)) / 2.0).abs() < 1e-5);
        assert!(((1.0 + pan(&l, l.forward * 100.0)) / 2.0 - 0.5).abs() < 1e-5);
    }

    #[test]
    fn non_finite_positions_are_silenced_and_centered() {
        assert_eq!(falloff(f32::NAN, 120.0, 600.0), 0.0);
        assert_eq!(falloff(f32::INFINITY, 120.0, 600.0), 0.0);
        assert_eq!(falloff(f32::NAN, 120.0, 0.0), 0.0); // before the no-cutoff branch
        let l = Listener::default();
        assert_eq!(pan(&l, Vec3::NAN), 0.0);
        assert_eq!(pan(&l, Vec3::new(f32::INFINITY, 0.0, 0.0)), 0.0);
    }

    #[test]
    fn amplitude_db_conversions() {
        assert_eq!(amplitude_db(1.0), 0.0);
        assert!((amplitude_db(0.5) + 6.0206).abs() < 1e-3);
        assert_eq!(amplitude_db(0.0), -60.0);
        assert!((amplitude_db(1.5) - 3.5218).abs() < 1e-3);
    }
}
