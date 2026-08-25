//! Walk bob and mouse sway for the first-person weapon rig. Pure math, no
//! rendering or windowing types.

use glam::{Mat4, Vec2, Vec3, Vec4};

/// Model space (X forward, Y left, Z up, tag_view at the eye) to view space
/// (X right, Y up, -Z forward). A rotation, so one-sided winding survives.
/// docs/research/xmodel-v14-format.md, "Model space and the view basis".
const BASIS: Mat4 = Mat4::from_cols(
    Vec4::new(0.0, 0.0, -1.0, 0.0), // model +X (forward) -> view -Z
    Vec4::new(-1.0, 0.0, 0.0, 0.0), // model +Y (left)    -> view -X
    Vec4::new(0.0, 1.0, 0.0, 0.0),  // model +Z (up)      -> view +Y
    Vec4::W,
);
/// Residual view-space offset (right, up, forward is -Z). Zero: the idle anim
/// already holds the rifle at the hip.
const OFFSET_POS: Vec3 = Vec3::ZERO;
/// Residual yaw about view up, positive swings the muzzle left.
const OFFSET_YAW_DEG: f32 = 0.0;
const BOB_CYCLE_HZ: f32 = 1.7; // full cycle at run speed
const BOB_LATERAL: f32 = 0.35; // view-space units at full speed
const BOB_VERTICAL: f32 = 0.25;
const BOB_ROLL_DEG: f32 = 0.6;
const BOB_ATTACK: f32 = 8.0; // 1/s, amplitude lerp rate
const SWAY_SCALE: f32 = 0.0015; // units per mouse count
const SWAY_MAX: f32 = 0.6; // clamp, view-space units
const SWAY_DECAY: f32 = 6.0; // 1/s exponential return

/// Walk bob cycle and mouse sway spring; `transform` is the view-space model
/// matrix.
pub struct ViewmodelMotion {
    phase: f32,
    amp: f32,
    sway: Vec2,
}

impl ViewmodelMotion {
    pub fn new() -> Self {
        Self {
            phase: 0.0,
            amp: 0.0,
            sway: Vec2::ZERO,
        }
    }

    /// `ground_speed` is horizontal speed on the ground, else 0. Mouse deltas
    /// are raw counts. `damp` scales bob and sway, 1 full, 0 frozen; ADS lerps
    /// it toward the weapon's `adsViewBobMult`.
    pub fn update(
        &mut self,
        dt: f32,
        ground_speed: f32,
        on_ground: bool,
        mouse_dx: f32,
        mouse_dy: f32,
        damp: f32,
    ) {
        let speed_frac = (ground_speed / 190.0).clamp(0.0, 1.0);

        if on_ground && ground_speed > 1.0 {
            self.phase += dt * std::f32::consts::TAU * BOB_CYCLE_HZ * speed_frac;
            self.phase %= std::f32::consts::TAU;
        }

        let target = if on_ground { speed_frac * damp } else { 0.0 };
        self.amp += (target - self.amp) * (BOB_ATTACK * dt).min(1.0);

        self.sway += Vec2::new(mouse_dx, mouse_dy) * SWAY_SCALE * damp;
        self.sway = self.sway.clamp_length_max(SWAY_MAX);
        self.sway *= (-SWAY_DECAY * dt).exp();
    }

    pub fn transform(&self) -> Mat4 {
        let bob = Vec3::new(
            self.phase.sin() * BOB_LATERAL * self.amp,
            -(self.phase * 2.0).sin().abs() * BOB_VERTICAL * self.amp,
            0.0,
        );
        let roll = self.phase.sin() * BOB_ROLL_DEG.to_radians() * self.amp;
        // The gun lags the turn: mouse right drifts it left, mouse down up.
        let sway_offset = Vec3::new(-self.sway.x, self.sway.y, 0.0);

        Mat4::from_translation(OFFSET_POS + bob + sway_offset)
            * Mat4::from_rotation_y(OFFSET_YAW_DEG.to_radians())
            * Mat4::from_rotation_z(roll)
            * BASIS
    }
}

impl Default for ViewmodelMotion {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bob_advances_only_when_moving_on_ground() {
        let mut m = ViewmodelMotion::new();
        let t0 = m.transform();
        m.update(0.016, 0.0, true, 0.0, 0.0, 1.0);
        assert!(m.transform().abs_diff_eq(t0, 1e-6), "idle must not bob");
        for _ in 0..30 {
            m.update(0.016, 190.0, true, 0.0, 0.0, 1.0);
        }
        assert!(!m.transform().abs_diff_eq(t0, 1e-4), "running must bob");
        for _ in 0..200 {
            m.update(0.016, 0.0, false, 0.0, 0.0, 1.0);
        }
        assert!(m.transform().abs_diff_eq(t0, 1e-3), "bob must decay in air");
    }

    #[test]
    fn sway_follows_mouse_and_decays() {
        let mut m = ViewmodelMotion::new();
        let t0 = m.transform();
        m.update(0.016, 0.0, true, 500.0, 0.0, 1.0);
        let swayed = m.transform();
        assert!(!swayed.abs_diff_eq(t0, 1e-5), "mouse motion must sway");
        for _ in 0..300 {
            m.update(0.016, 0.0, true, 0.0, 0.0, 1.0);
        }
        assert!(
            m.transform().abs_diff_eq(t0, 1e-3),
            "sway must decay to rest"
        );
    }

    #[test]
    fn sway_is_clamped() {
        let mut m = ViewmodelMotion::new();
        for _ in 0..100 {
            m.update(0.016, 0.0, true, 10000.0, 0.0, 1.0);
        }
        let a = m.transform();
        for _ in 0..100 {
            m.update(0.016, 0.0, true, 10000.0, 0.0, 1.0);
        }
        assert!(m.transform().abs_diff_eq(a, 1e-3), "sway must saturate");
    }

    /// Screenshot-verified sign.
    #[test]
    fn sway_direction_is_pinned() {
        let mut right = ViewmodelMotion::new();
        right.update(0.016, 0.0, true, 500.0, 0.0, 1.0);
        assert!(
            right.transform().w_axis.x < OFFSET_POS.x,
            "mouse right must sway the gun toward view -x"
        );

        let mut down = ViewmodelMotion::new();
        down.update(0.016, 0.0, true, 0.0, 500.0, 1.0);
        assert!(
            down.transform().w_axis.y > OFFSET_POS.y,
            "mouse down must sway the gun toward view +y"
        );
    }

    #[test]
    fn damping_freezes_bob_and_sway() {
        let mut m = ViewmodelMotion::new();
        let rest = m.transform();
        for _ in 0..200 {
            m.update(0.016, 190.0, true, 500.0, 500.0, 0.0);
        }
        assert!(
            m.transform().abs_diff_eq(rest, 1e-6),
            "damp 0 must leave the transform at rest, got {:?}",
            m.transform()
        );

        // the same inputs at full damping must move it
        let mut live = ViewmodelMotion::new();
        for _ in 0..200 {
            live.update(0.016, 190.0, true, 500.0, 500.0, 1.0);
        }
        assert!(!live.transform().abs_diff_eq(rest, 1e-4));
    }
}
