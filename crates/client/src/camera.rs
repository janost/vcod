use glam::{Mat4, Quat, Vec3};

/// Default (non-ADS) vertical FOV, degrees.
pub const DEFAULT_FOV_DEG: f32 = vcod_common::weapon::DEFAULT_FOV;

/// (forward, right, up) for a yaw/pitch pair. Z-up, yaw 0 = +X, so right is
/// -Y at rest. No roll, since the fire trace and impact billboards must not
/// tilt with a lean.
pub fn basis(yaw: f32, pitch: f32) -> (Vec3, Vec3, Vec3) {
    let forward = Vec3::new(
        pitch.cos() * yaw.cos(),
        pitch.cos() * yaw.sin(),
        pitch.sin(),
    );
    let right = Vec3::new(yaw.sin(), -yaw.cos(), 0.0);
    let up = right.cross(forward).normalize();
    (forward, right, up)
}

/// Positive roll (leaning right) rotates the world counterclockwise, as CoD
/// does. `fov_deg` is vertical.
pub fn view_proj_from(
    pos: Vec3,
    yaw: f32,
    pitch: f32,
    roll: f32,
    fov_deg: f32,
    aspect: f32,
) -> Mat4 {
    let (forward, _, _) = basis(yaw, pitch);
    let up = Quat::from_axis_angle(forward, roll) * Vec3::Z;
    glam::camera::rh::proj::directx::perspective(fov_deg.to_radians(), aspect, 4.0, 60000.0)
        * glam::camera::rh::view::look_to_mat4(pos, forward, up)
}

/// Wrap an angle in degrees to `[-180, 180)`.
fn wrap180(mut d: f32) -> f32 {
    d %= 360.0;
    if d >= 180.0 {
        d -= 360.0;
    } else if d < -180.0 {
        d += 360.0;
    }
    d
}

/// Shortest-arc lerp between angles in degrees, so the 360/0 seam wraps.
pub fn lerp_angle(a: f32, b: f32, f: f32) -> f32 {
    wrap180(a + wrap180(b - a) * f)
}

#[derive(Default)]
pub struct InputState {
    pub forward: bool,
    pub back: bool,
    pub left: bool,
    pub right: bool,
    pub up: bool,
    pub down: bool,
    pub boost: bool,
}

/// Yaw 0 = +X, positive yaw toward +Y (entity "angles" convention). Radians.
pub struct FlyCamera {
    pub pos: Vec3,
    pub yaw: f32,
    pub pitch: f32,
    pub speed: f32,
}

impl FlyCamera {
    pub fn new(pos: Vec3, yaw_deg: f32) -> Self {
        Self {
            pos,
            yaw: yaw_deg.to_radians(),
            pitch: 0.0,
            speed: 500.0,
        }
    }

    pub fn forward(&self) -> Vec3 {
        Vec3::new(
            self.pitch.cos() * self.yaw.cos(),
            self.pitch.cos() * self.yaw.sin(),
            self.pitch.sin(),
        )
    }

    pub fn update(&mut self, input: &InputState, dt: f32) {
        let fwd = self.forward();
        let right = fwd.cross(Vec3::Z).normalize_or_zero();
        let mut dir = Vec3::ZERO;
        if input.forward {
            dir += fwd;
        }
        if input.back {
            dir -= fwd;
        }
        if input.right {
            dir += right;
        }
        if input.left {
            dir -= right;
        }
        if input.up {
            dir += Vec3::Z;
        }
        if input.down {
            dir -= Vec3::Z;
        }
        let boost = if input.boost { 4.0 } else { 1.0 };
        self.pos += dir.normalize_or_zero() * self.speed * boost * dt;
    }

    pub fn mouse_delta(&mut self, dx: f32, dy: f32) {
        const SENS: f32 = 0.003;
        self.yaw -= dx * SENS;
        self.pitch = (self.pitch - dy * SENS).clamp(-89.0f32.to_radians(), 89.0f32.to_radians());
    }

    pub fn adjust_speed(&mut self, scroll: f32) {
        self.speed = (self.speed * 1.2f32.powf(scroll)).clamp(10.0, 20000.0);
    }

    pub fn view_proj(&self, aspect: f32) -> Mat4 {
        view_proj_from(self.pos, self.yaw, self.pitch, 0.0, DEFAULT_FOV_DEG, aspect)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::Vec3;

    #[test]
    fn lerp_angle_takes_the_short_way_across_the_seam() {
        // 350 -> 10 is a 20-degree step forward through 0, not 340 back.
        assert!((lerp_angle(350.0, 10.0, 0.5) - 0.0).abs() < 1e-4);
        assert!((lerp_angle(10.0, 350.0, 0.5) - 0.0).abs() < 1e-4);
        assert!((lerp_angle(20.0, 80.0, 0.0) - 20.0).abs() < 1e-4);
        assert!((lerp_angle(20.0, 80.0, 1.0) - 80.0).abs() < 1e-4);
        assert!((lerp_angle(20.0, 80.0, 0.5) - 50.0).abs() < 1e-4);
    }

    #[test]
    fn yaw_zero_looks_along_plus_x() {
        let cam = FlyCamera::new(Vec3::ZERO, 0.0);
        assert!(cam.forward().abs_diff_eq(Vec3::X, 1e-5));
        let cam = FlyCamera::new(Vec3::ZERO, 90.0);
        assert!(cam.forward().abs_diff_eq(Vec3::Y, 1e-5));
    }

    #[test]
    fn moves_along_look_direction() {
        let mut cam = FlyCamera::new(Vec3::ZERO, 0.0);
        cam.speed = 100.0;
        let input = InputState {
            forward: true,
            ..Default::default()
        };
        cam.update(&input, 1.0);
        assert!(cam.pos.abs_diff_eq(Vec3::new(100.0, 0.0, 0.0), 1e-4));
    }

    #[test]
    fn boost_multiplies_speed() {
        let mut cam = FlyCamera::new(Vec3::ZERO, 0.0);
        cam.speed = 100.0;
        let input = InputState {
            forward: true,
            boost: true,
            ..Default::default()
        };
        cam.update(&input, 1.0);
        assert!(cam.pos.x > 350.0);
    }

    #[test]
    fn pitch_clamps() {
        let mut cam = FlyCamera::new(Vec3::ZERO, 0.0);
        cam.mouse_delta(0.0, -100000.0);
        assert!(cam.pitch <= 89.0f32.to_radians() + 1e-6);
        cam.mouse_delta(0.0, 100000.0);
        assert!(cam.pitch >= -89.0f32.to_radians() - 1e-6);
    }

    #[test]
    fn view_proj_puts_point_ahead_in_front() {
        let cam = FlyCamera::new(Vec3::ZERO, 0.0);
        let clip = cam.view_proj(16.0 / 9.0) * glam::Vec4::new(1000.0, 0.0, 0.0, 1.0);
        let ndc = clip / clip.w;
        assert!(
            ndc.z > 0.0 && ndc.z < 1.0,
            "point ahead should be inside depth range, got {}",
            ndc.z
        );
        assert!(ndc.x.abs() < 0.01 && ndc.y.abs() < 0.01);
    }

    /// The one lean sign nobody can eyeball in a test run.
    #[test]
    fn leaning_right_rolls_the_world_counterclockwise() {
        let ndc = |vp: Mat4, p: Vec3| {
            let c = vp * p.extend(1.0);
            c / c.w
        };
        let flat = view_proj_from(Vec3::ZERO, 0.0, 0.0, 0.0, DEFAULT_FOV_DEG, 16.0 / 9.0);
        assert!(
            ndc(flat, Vec3::new(1000.0, -100.0, 0.0)).x > 0.0,
            "-Y is right"
        );
        assert!(ndc(flat, Vec3::new(1000.0, 0.0, 100.0)).y > 0.0, "+Z is up");

        let lean_right = view_proj_from(
            Vec3::ZERO,
            0.0,
            0.0,
            14.0f32.to_radians(),
            DEFAULT_FOV_DEG,
            16.0 / 9.0,
        );
        let above = ndc(lean_right, Vec3::new(1000.0, 0.0, 100.0));
        assert!(
            above.x < 0.0 && above.y > 0.0,
            "leaning right should swing the point above the centre to the left, got {above}"
        );
    }

    #[test]
    fn basis_is_right_handed_and_orthonormal() {
        let (f, r, u) = basis(0.0, 0.0);
        assert!(f.abs_diff_eq(Vec3::X, 1e-6), "forward {f}");
        assert!(r.abs_diff_eq(-Vec3::Y, 1e-6), "right {r}");
        assert!(u.abs_diff_eq(Vec3::Z, 1e-6), "up {u}");

        let (f, r, u) = basis(0.7, -0.4);
        for v in [f, r, u] {
            assert!((v.length() - 1.0).abs() < 1e-5, "{v} is not unit length");
        }
        assert!(f.dot(r).abs() < 1e-5);
        assert!(f.dot(u).abs() < 1e-5);
        assert!(r.dot(u).abs() < 1e-5);
    }

    #[test]
    fn view_proj_from_matches_flycamera_when_roll_is_zero() {
        let cam = FlyCamera::new(Vec3::new(10.0, 20.0, 30.0), 90.0);
        let a = cam.view_proj(16.0 / 9.0);
        let b = view_proj_from(
            cam.pos,
            cam.yaw,
            cam.pitch,
            0.0,
            DEFAULT_FOV_DEG,
            16.0 / 9.0,
        );
        assert!(a.abs_diff_eq(b, 1e-5));
    }

    #[test]
    fn a_narrower_fov_magnifies() {
        let ndc_x = |fov: f32| {
            let vp = view_proj_from(Vec3::ZERO, 0.0, 0.0, 0.0, fov, 16.0 / 9.0);
            let c = vp * glam::Vec4::new(1000.0, -100.0, 0.0, 1.0);
            (c / c.w).x
        };
        let wide = ndc_x(DEFAULT_FOV_DEG);
        let zoomed = ndc_x(50.0);
        assert!(wide > 0.0, "-Y is right, got {wide}");
        assert!(
            zoomed > wide,
            "fov 50 must magnify vs fov 75, got {zoomed} vs {wide}"
        );
    }
}
