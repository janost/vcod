use glam::{Mat4, Quat, Vec3};

/// Default (non-ADS) horizontal FOV, degrees.
pub const DEFAULT_FOV_DEG: f32 = vcod_common::weapon::CG_FOV;

/// Frame projection planes; the sky pass sizes its box off them
/// (`box_size(Z_FAR, Z_NEAR)`).
pub const Z_NEAR: f32 = 4.0;
pub const Z_FAR: f32 = 60000.0;

/// (forward, right, up) for a yaw/pitch pair. Z-up, yaw 0 = +X, so right is
/// -Y at rest. No roll, since the fire trace and impact billboards must not
/// tilt with the view's roll.
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

/// The vertical fov for a horizontal `fov_x` across a viewport `aspect`
/// wide, both in degrees: retail's refdef keeps `fov_x` and derives this
/// (docs/research/xmodel-v14-format.md, "The view fov").
pub fn fov_y(fov_x: f32, aspect: f32) -> f32 {
    2.0 * ((fov_x / 2.0).to_radians().tan() / aspect)
        .atan()
        .to_degrees()
}

/// The horizontal fov for a vertical `fov_y` across a viewport `aspect`
/// wide, both in degrees: [`fov_y`]'s inverse.
pub fn fov_x(fov_y: f32, aspect: f32) -> f32 {
    2.0 * ((fov_y / 2.0).to_radians().tan() * aspect)
        .atan()
        .to_degrees()
}

/// vcod's Hor+ (`vc_widescreen 1`): the horizontal fov a viewport `aspect`
/// wide draws for `fov_x`, every fov the game names (`cg_fov`, a sight's,
/// a seat's) being the width of a 4:3 view. A wider viewport keeps that
/// view's vertical fov and sees more at the sides; a 4:3 or narrower one
/// keeps `fov_x`, as retail does at any aspect (`widescreen` false).
pub fn view_fov_x(fov_x: f32, aspect: f32, widescreen: bool) -> f32 {
    const FOUR_THREE: f32 = 4.0 / 3.0;
    if !widescreen || aspect <= FOUR_THREE {
        return fov_x;
    }
    self::fov_x(fov_y(fov_x, FOUR_THREE), aspect)
}

/// CoD4's `vidConfig.aspectRatioWindow` (0xcc9d0f8) for `r_aspectRatio`'s
/// `setting`, by name or enum index, in a `w` x `h` window
/// (`R_StoreWindowSettings`, iw3mp.exe 0x5f3500, cod4-front-end.md 9.3,
/// 10.1). A fixed setting names its ratio; `auto`, and any value the enum
/// would refuse, takes the ratio of the window's shape. The view projects
/// at the result over the whole window, so a window of another shape
/// stretches it.
pub fn cod4_aspect(setting: &str, w: f32, h: f32) -> f32 {
    // The enum's values in index order; 0 is `auto` (jump table 0x5f36b8).
    const FIXED: [(&str, &str, f32); 3] = [
        ("standard", "1", ASPECT_4_3),
        ("wide 16:10", "2", ASPECT_16_10),
        ("wide 16:9", "3", ASPECT_16_9),
    ];
    FIXED
        .iter()
        .find(|(name, index, _)| setting.eq_ignore_ascii_case(name) || setting == *index)
        .map_or_else(|| auto_aspect(w, h), |&(_, _, ratio)| ratio)
}

/// The three ratios 1.7 stores (floats at 0x70bb6c, 0x70bb74, 0x70bb70).
const ASPECT_4_3: f32 = 4.0 / 3.0;
const ASPECT_16_10: f32 = 1.6;
const ASPECT_16_9: f32 = 16.0 / 9.0;

/// `auto`: the window's height in sixteenths of its width, kept as a
/// float (0x5f358c), nudged up by 2^-30 (the double at 0x70b088) and
/// rounded to nearest (`fistp`, 0x5f35a2), sorts the window: exactly 10
/// sixteenths is 16:10, fewer 16:9, more 4:3.
fn auto_aspect(w: f32, h: f32) -> f32 {
    use std::cmp::Ordering;
    let sixteenths = (f64::from(h) * 16.0 / f64::from(w.max(1.0))) as f32;
    let nearest = (f64::from(sixteenths) + 2f64.powi(-30)).round_ties_even();
    match nearest.total_cmp(&10.0) {
        Ordering::Less => ASPECT_16_9,
        Ordering::Equal => ASPECT_16_10,
        Ordering::Greater => ASPECT_4_3,
    }
}

/// Where a CoD4 frame projects its views and places its 2D layer: 1.7's
/// `vidConfig` (`R_StoreWindowSettings`, iw3mp.exe 0x5f3500) or, under
/// `vc_windowAspect 1`, vcod's (cod4-front-end.md 10).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Cod4Screen {
    /// `aspectRatioWindow`: every view projects at it across the whole
    /// window, its fov a 4:3 view's widened to it (`CG_UpdateFov`).
    pub aspect: f32,
    /// The display's pixel aspect (`vidConfig` +0x20, 0xcc9d100): a
    /// pixel's width over its height, which
    /// `ScrPlace_SetupFloatViewport` (0x477420) divides the HUD's 4:3
    /// column by.
    pub pixel_aspect: f32,
    /// `wideScreen` (`R_StoreWindowSettings` 0x5f35e5): set off 4:3, so
    /// the HUD menus draw their 854-wide pieces instead of the 640 ones.
    pub wide_screen: bool,
    /// What a HUD strip spanning the picked aspect's screen scales across
    /// by to span the window: vcod's, above 1 only under
    /// `vc_windowAspect 1` on a window wider than the picked aspect.
    pub strip_scale: f32,
}

impl Cod4Screen {
    /// 1.7's for `r_aspectRatio`'s `setting` in a `w` x `h` window: the
    /// picked aspect stretched over the window, and in fullscreen the HUD
    /// stretched with it (`aspect * h / w`; 1.0 windowed). With
    /// `window_aspect` (`vc_windowAspect 1`) the view projects at the
    /// window's own aspect, Hor+ by the same `CG_UpdateFov`, and the HUD
    /// keeps square pixels: its 4:3 column centred, its `LEFT` and `RIGHT`
    /// pieces at the window's edges, as 1.7 places them in a window of the
    /// picked aspect.
    pub fn of(setting: &str, w: f32, h: f32, fullscreen: bool, window_aspect: bool) -> Self {
        let picked = cod4_aspect(setting, w, h);
        let wide_screen = picked != ASPECT_4_3;
        if window_aspect {
            let aspect = w / h.max(1.0);
            // The strip's grid width: 640 at 4:3, 854 (480 * 16/9) otherwise.
            let authored = if wide_screen { 16.0 / 9.0 } else { 4.0 / 3.0 };
            return Cod4Screen {
                aspect,
                pixel_aspect: 1.0,
                wide_screen,
                strip_scale: (aspect / authored).max(1.0),
            };
        }
        Cod4Screen {
            aspect: picked,
            pixel_aspect: if fullscreen {
                picked * h / w.max(1.0)
            } else {
                1.0
            },
            wide_screen,
            strip_scale: 1.0,
        }
    }
}

/// A projection `fov_x` degrees wide across a viewport `aspect` wide.
pub fn perspective(fov_x: f32, aspect: f32, near: f32, far: f32) -> Mat4 {
    glam::camera::rh::proj::directx::perspective(
        fov_y(fov_x, aspect).to_radians(),
        aspect,
        near,
        far,
    )
}

/// Positive roll (the wire's `viewangles[2]`, which only the kick moves; a
/// lean never rolls the view) rotates the world counterclockwise, as CoD
/// does. `fov_x` is horizontal.
pub fn view_proj_from(pos: Vec3, yaw: f32, pitch: f32, roll: f32, fov_x: f32, aspect: f32) -> Mat4 {
    let (forward, _, _) = basis(yaw, pitch);
    perspective(fov_x, aspect, Z_NEAR, Z_FAR)
        * glam::camera::rh::view::look_to_mat4(pos, forward, up_hint(yaw, pitch, roll))
}

/// World up turned by `roll` about the view forward: the up the view
/// matrix is built with.
pub fn up_hint(yaw: f32, pitch: f32, roll: f32) -> Vec3 {
    Quat::from_axis_angle(basis(yaw, pitch).0, roll) * Vec3::Z
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

/// The fly and walk cameras' pitch range, radians.
pub const PITCH_MAX: f32 = 89.0 * std::f32::consts::PI / 180.0;
pub const PITCH_MIN: f32 = -PITCH_MAX;

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

    /// Turns by `[pitch, yaw]` degrees, pitch down-positive
    /// (`play::input::MouseLook::degrees`).
    pub fn look(&mut self, [pitch, yaw]: [f32; 2]) {
        self.yaw += yaw.to_radians();
        self.pitch = (self.pitch - pitch.to_radians()).clamp(PITCH_MIN, PITCH_MAX);
    }

    pub fn adjust_speed(&mut self, scroll: f32) {
        self.speed = (self.speed * 1.2f32.powf(scroll)).clamp(10.0, 20000.0);
    }

    /// `fov_x` horizontal degrees across a viewport `aspect` wide.
    pub fn view_proj(&self, fov_x: f32, aspect: f32) -> Mat4 {
        view_proj_from(self.pos, self.yaw, self.pitch, 0.0, fov_x, aspect)
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
        cam.look([-1000.0, 0.0]);
        assert!(cam.pitch <= 89.0f32.to_radians() + 1e-6);
        cam.look([1000.0, 0.0]);
        assert!(cam.pitch >= -89.0f32.to_radians() - 1e-6);
    }

    #[test]
    fn view_proj_puts_point_ahead_in_front() {
        let cam = FlyCamera::new(Vec3::ZERO, 0.0);
        let clip =
            cam.view_proj(DEFAULT_FOV_DEG, 16.0 / 9.0) * glam::Vec4::new(1000.0, 0.0, 0.0, 1.0);
        let ndc = clip / clip.w;
        assert!(
            ndc.z > 0.0 && ndc.z < 1.0,
            "point ahead should be inside depth range, got {}",
            ndc.z
        );
        assert!(ndc.x.abs() < 0.01 && ndc.y.abs() < 0.01);
    }

    /// The one roll sign nobody can eyeball in a test run.
    #[test]
    fn positive_roll_turns_the_world_counterclockwise() {
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

        let rolled = view_proj_from(
            Vec3::ZERO,
            0.0,
            0.0,
            14.0f32.to_radians(),
            DEFAULT_FOV_DEG,
            16.0 / 9.0,
        );
        let above = ndc(rolled, Vec3::new(1000.0, 0.0, 100.0));
        assert!(
            above.x < 0.0 && above.y > 0.0,
            "a positive roll should swing the point above the centre to the left, got {above}"
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
        let a = cam.view_proj(DEFAULT_FOV_DEG, 16.0 / 9.0);
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
            "fov 50 must magnify vs fov 80, got {zoomed} vs {wide}"
        );
    }

    /// Retail's refdef at `cg_fov 80`: 64.4 degrees tall at 4:3, 55.3 at
    /// 16:10, and a 50-degree sight 38.6 tall at 4:3.
    #[test]
    fn fov_y_keeps_the_horizontal_fov() {
        let close = |a: f32, b: f32| (a - b).abs() < 0.05;
        assert!(
            close(fov_y(80.0, 4.0 / 3.0), 64.37),
            "{}",
            fov_y(80.0, 4.0 / 3.0)
        );
        assert!(close(fov_y(80.0, 1.6), 55.35), "{}", fov_y(80.0, 1.6));
        assert!(
            close(fov_y(50.0, 4.0 / 3.0), 38.55),
            "{}",
            fov_y(50.0, 4.0 / 3.0)
        );
        // The edges of the view sit at +-1 in clip space across the width.
        let p = perspective(80.0, 1.6, 1.0, 100.0);
        let edge = p.project_point3(Vec3::new(-(40.0f32.to_radians().tan()), 0.0, -1.0));
        assert!((edge.x + 1.0).abs() < 1e-5, "{edge}");
    }

    /// Hor+: `cg_fov 80` is 96.4 degrees wide at 16:9 and 111.5 at 21:9,
    /// both 64.4 tall as at 4:3; 4:3, 5:4 and `vc_widescreen 0` keep 80.
    #[test]
    fn hor_plus_keeps_the_four_three_vertical_fov() {
        let close = |a: f32, b: f32| (a - b).abs() < 0.05;
        let wide = view_fov_x(80.0, 16.0 / 9.0, true);
        assert!(close(wide, 96.39), "{wide}");
        let ultra = view_fov_x(80.0, 21.0 / 9.0, true);
        assert!(close(ultra, 111.48), "{ultra}");
        for (fov, aspect) in [(80.0, 16.0 / 9.0), (80.0, 21.0 / 9.0), (30.0, 2.4)] {
            let drawn = view_fov_x(fov, aspect, true);
            assert!(close(fov_y(drawn, aspect), fov_y(fov, 4.0 / 3.0)));
        }
        assert_eq!(view_fov_x(80.0, 4.0 / 3.0, true), 80.0);
        assert_eq!(view_fov_x(80.0, 1.25, true), 80.0);
        assert_eq!(view_fov_x(80.0, 16.0 / 9.0, false), 80.0);
        assert!(close(fov_x(fov_y(80.0, 1.6), 1.6), 80.0));
    }

    /// `r_aspectRatio` as 1.7's `R_StoreWindowSettings` reads it
    /// (iw3mp.exe 0x5f3500): `auto` rounds `h * 16 / w` against 10.
    #[test]
    fn cod4_aspect_follows_r_aspect_ratio() {
        assert_eq!(cod4_aspect("auto", 1920.0, 1080.0), 16.0 / 9.0);
        assert_eq!(cod4_aspect("auto", 1680.0, 1050.0), 1.6);
        assert_eq!(cod4_aspect("auto", 1024.0, 768.0), 4.0 / 3.0);
        assert_eq!(cod4_aspect("auto", 1280.0, 1024.0), 4.0 / 3.0);
        // an ultrawide window rounds to 7: 16:9, stretched
        assert_eq!(cod4_aspect("auto", 3440.0, 1440.0), 16.0 / 9.0);
        assert_eq!(cod4_aspect("standard", 1920.0, 1080.0), 4.0 / 3.0);
        assert_eq!(cod4_aspect("wide 16:10", 1024.0, 768.0), 1.6);
        assert_eq!(cod4_aspect("Wide 16:9", 1024.0, 768.0), 16.0 / 9.0);
        // the view's vertical fov stays a 4:3 view's (`CG_UpdateFov`)
        let close = |a: f32, b: f32| (a - b).abs() < 1e-3;
        let wide = view_fov_x(65.0, cod4_aspect("standard", 1920.0, 1080.0), true);
        assert!(close(fov_y(wide, 4.0 / 3.0), fov_y(65.0, 4.0 / 3.0)));
    }

    /// `cod4_aspect` across window sizes and every `r_aspectRatio` value,
    /// ties (`h * 16 / w` at 9.5 and 10.5) and degenerate windows included.
    #[test]
    fn cod4_aspect_table() {
        const S: f32 = 4.0 / 3.0;
        const W10: f32 = 1.6;
        const W9: f32 = 16.0 / 9.0;
        let auto: &[(f32, f32, f32)] = &[
            (640.0, 480.0, S),
            (800.0, 600.0, S),
            (1152.0, 864.0, S),
            (1280.0, 960.0, S),
            (1280.0, 1024.0, S),
            (1080.0, 1920.0, S),
            (1280.0, 800.0, W10),
            (1440.0, 900.0, W10),
            (1680.0, 1050.0, W10),
            (1920.0, 1200.0, W10),
            (1280.0, 720.0, W9),
            (1360.0, 768.0, W9),
            (1366.0, 768.0, W9),
            (1600.0, 900.0, W9),
            (1920.0, 1080.0, W9),
            (2560.0, 1080.0, W9),
            (3440.0, 1440.0, W9),
            (3840.0, 1600.0, W9),
            (5120.0, 1440.0, W9),
            (32.0, 19.0, W10),
            (1920.0, 1140.0, W10),
            (32.0, 21.0, S),
            (1024.0, 672.0, S),
            (1920.0, 1260.0, S),
            (0.0, 768.0, S),
            (0.0, 0.0, W9),
        ];
        for setting in ["auto", "AUTO", "0", "", "bogus"] {
            for &(w, h, want) in auto {
                assert_eq!(cod4_aspect(setting, w, h), want, "{setting} {w}x{h}");
            }
        }
        for (setting, want) in [
            ("standard", S),
            ("Standard", S),
            ("1", S),
            ("wide 16:10", W10),
            ("WIDE 16:10", W10),
            ("2", W10),
            ("wide 16:9", W9),
            ("Wide 16:9", W9),
            ("3", W9),
        ] {
            for &(w, h, _) in auto {
                assert_eq!(cod4_aspect(setting, w, h), want, "{setting} {w}x{h}");
            }
        }
    }

    /// At 3840x1600 (2.4:1) 1.7 picks 16:9 and stretches it; fullscreen
    /// stretches the HUD's 4:3 column with it (`aspect * h / w`).
    /// `vc_windowAspect 1` projects at 2.4 with `cg_fov 65`'s 4:3 vertical
    /// fov and keeps the HUD square.
    #[test]
    fn cod4_screen_at_ultrawide_and_16_9() {
        let close = |a: f32, b: f32| (a - b).abs() < 1e-3;
        let s = Cod4Screen::of("auto", 3840.0, 1600.0, false, false);
        assert_eq!(s.aspect, 16.0 / 9.0);
        assert_eq!(s.pixel_aspect, 1.0);
        let s = Cod4Screen::of("auto", 3840.0, 1600.0, true, false);
        assert!(close(s.pixel_aspect, 16.0 / 9.0 * 1600.0 / 3840.0));
        let s = Cod4Screen::of("standard", 3840.0, 1600.0, false, false);
        assert_eq!(s.aspect, 4.0 / 3.0);
        let s = Cod4Screen::of("auto", 3840.0, 1600.0, true, true);
        assert!(close(s.aspect, 2.4));
        assert_eq!(s.pixel_aspect, 1.0);
        assert!(s.wide_screen && close(s.strip_scale, 2.4 / (16.0 / 9.0)));
        let fx = view_fov_x(65.0, s.aspect, true);
        assert!(close(fov_y(fx, 2.4), fov_y(65.0, 4.0 / 3.0)));
        assert!(Cod4Screen::of("auto", 1920.0, 1080.0, true, false).wide_screen);
        let std = Cod4Screen::of("standard", 3840.0, 1600.0, true, false);
        assert!(!std.wide_screen && std.strip_scale == 1.0);
        // a 16:9 window is the same either way
        let a = Cod4Screen::of("auto", 1920.0, 1080.0, false, false);
        let b = Cod4Screen::of("auto", 1920.0, 1080.0, false, true);
        assert!(close(a.aspect, b.aspect));
    }
}
