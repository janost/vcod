//! CoD4 1.7's third-person view: `cg.renderingThirdPerson` and
//! `CG_OffsetThirdPersonView` (iw3mp.exe 0x451d70, 0x44fb20;
//! cod4-client-view.md 16).

use glam::Vec3;

/// `cg_thirdPerson`, `cg_thirdPersonRange` and `cg_thirdPersonAngle`
/// (iw3mp.exe 0x43ccb1..0x43cdbf), as the console holds them.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Dvars {
    pub on: bool,
    pub range: f32,
    pub angle: f32,
}

impl Default for Dvars {
    fn default() -> Self {
        Dvars {
            on: false,
            range: 120.0,
            angle: 0.0,
        }
    }
}

impl Dvars {
    /// The console's values, each held to its registered domain (the
    /// range 0..1024, the angle -180..360).
    pub fn read(cvar: impl Fn(&str) -> Option<String>) -> Dvars {
        let num = |name: &str, default: f32| {
            cvar(name)
                .and_then(|v| v.trim().parse::<f32>().ok())
                .unwrap_or(default)
        };
        Dvars {
            on: num("cg_thirdPerson", 0.0) != 0.0,
            range: num("cg_thirdPersonRange", 120.0).clamp(0.0, 1024.0),
            angle: num("cg_thirdPersonAngle", 0.0).clamp(-180.0, 360.0),
        }
    }
}

/// A frame drawn in third person.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ThirdPerson {
    /// The camera `CG_OffsetThirdPersonView` placed.
    pub origin: Vec3,
    /// Its pitch, yaw and roll, degrees.
    pub angles: Vec3,
    /// A dead playerstate's `stats[1]` (`pm_type` 7 and up).
    pub dead_yaw: Option<f32>,
    /// The `viewHeightCurrent` the camera rose by over the view origin.
    pub view_height: f32,
}

/// `cg.renderingThirdPerson` (0x451d70): `cg_thirdPerson`, or a dead
/// playerstate (`pm_type` 7 and up, 0x451d80..0x451d8a). Its other two
/// arms, a client info's vehicle seat (0x451da7..0x451dbc) and a killcam
/// on an entity (0x451dc2..0x451deb), reach nothing 1.7's stock MP sends
/// vcod.
pub fn rendering(dvars: Dvars, pm_type: u32) -> bool {
    dvars.on || pm_type >= 7
}

/// The box the camera's traces sweep (0x6cdda4, 0x6cddb0).
pub const BOX: (Vec3, Vec3) = (Vec3::splat(-4.0), Vec3::splat(4.0));

/// The mask they meet (0x44fa1c).
pub const MASK: u32 = 0x811;

/// `CG_OffsetThirdPersonView` (0x44fb20) from `vieworg` (the view origin,
/// already raised by `viewHeightCurrent`) and `view` (`refdefViewAngles`):
/// the camera's origin and angles. `trace` sweeps [`BOX`] from the first
/// point to the second and gives its fraction.
///
/// - A dead playerstate's yaw is its `dead_yaw`.
/// - The focus is 512 along the view, its pitch held to 45 at most.
/// - The camera starts 8 over the view origin and backs off `range` along
///   the view with half its pitch and `angle` less yaw.
/// - The trace pulls it in (`trace_camera`).
/// - It looks at the focus: its pitch is the focus's over its level
///   distance (at least 1), its yaw the turned one.
pub fn camera(
    vieworg: Vec3,
    view: Vec3,
    dead_yaw: Option<f32>,
    dvars: Dvars,
    trace: &dyn Fn(Vec3, Vec3) -> f32,
) -> (Vec3, Vec3) {
    let mut angles = view;
    let mut focus_angles = [view.x, view.y];
    if let Some(yaw) = dead_yaw {
        angles.y = yaw;
        focus_angles[1] = yaw;
    }
    focus_angles[0] = focus_angles[0].min(45.0);
    // The inline `AngleVectors` (0x44fbab..0x44fc4c): each angle times the
    // double at 0x70b2d0 stored as a float, its `fsincos` stored as floats.
    let sin_cos = |deg: f32| {
        let r = (f64::from(deg) * 0.01745329238474369) as f32;
        let (s, c) = f64::from(r).sin_cos();
        (s as f32, c as f32)
    };
    let (sy, cy) = sin_cos(focus_angles[1]);
    let (sp, cp) = sin_cos(focus_angles[0]);
    let forward = Vec3::new(cp * cy, cp * sy, -sp);
    // 512, the double at 0x70ba78.
    let along = |o: f32, d: f32, s: f64| (f64::from(d) * s + f64::from(o)) as f32;
    let focus = Vec3::new(
        along(vieworg.x, forward.x, 512.0),
        along(vieworg.y, forward.y, 512.0),
        along(vieworg.z, forward.z, 512.0),
    );
    // 8 up (0x70ae88), half the pitch (0x70ace8).
    let start = Vec3::new(vieworg.x, vieworg.y, (f64::from(vieworg.z) + 8.0) as f32);
    angles.x = (f64::from(angles.x) * 0.5) as f32;
    angles.y -= dvars.angle;
    let [back, _, _] = vcod_common::x87::angle_vectors_msvc(angles);
    let range = -f64::from(dvars.range);
    let end = Vec3::new(
        along(start.x, back.x, range),
        along(start.y, back.y, range),
        along(start.z, back.z, range),
    );
    let origin = trace_camera(vieworg, end, trace);
    let to = focus - origin;
    let dist = (to.x * to.x + to.y * to.y).sqrt().max(1.0);
    // -180 / pi, the double at 0x70bb30.
    angles.x = (f64::from(to.z).atan2(f64::from(dist)) * -57.2957763671875) as f32;
    (origin, angles)
}

/// 0x44fa00: the camera's two traces from the view origin. A first that
/// stops short is lifted by 32 (0x70b120) times what it missed and traced
/// again, so a low ceiling does not hold the camera in it; the camera is
/// each trace's fraction of its way.
fn trace_camera(start: Vec3, end: Vec3, trace: &dyn Fn(Vec3, Vec3) -> f32) -> Vec3 {
    let lerp = |a: Vec3, b: Vec3, f: f32| {
        let k =
            |s: f32, e: f32| ((f64::from(e) - f64::from(s)) * f64::from(f) + f64::from(s)) as f32;
        Vec3::new(k(a.x, b.x), k(a.y, b.y), k(a.z, b.z))
    };
    let f = trace(start, end);
    if f == 1.0 {
        return end;
    }
    let mut mid = lerp(start, end, f);
    mid.z = (f64::from(mid.z) + (1.0 - f64::from(f)) * 32.0) as f32;
    lerp(start, mid, trace(start, mid))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn open(_: Vec3, _: Vec3) -> f32 {
        1.0
    }

    fn near(a: Vec3, b: Vec3) -> bool {
        (a - b).abs().max_element() < 1e-3
    }

    /// Level, nothing behind: 8 up and `cg_thirdPersonRange` back, looking
    /// down at the focus 512 ahead of the eye.
    #[test]
    fn the_camera_backs_off_the_range_and_looks_at_the_focus() {
        let eye = Vec3::new(100.0, 200.0, 60.0);
        let (o, a) = camera(eye, Vec3::ZERO, None, Dvars::default(), &open);
        assert!(near(o, Vec3::new(-20.0, 200.0, 68.0)), "{o}");
        // atan2(-8, 632) down.
        let pitch = (8.0f64 / 632.0).atan().to_degrees() as f32;
        assert!((a.x - pitch).abs() < 1e-4, "{a}");
        assert_eq!(a.y, 0.0);
    }

    /// `cg_thirdPersonAngle` turns the camera about the eye; the yaw drawn
    /// is the view's less the angle.
    #[test]
    fn the_angle_turns_the_camera_about_the_eye() {
        let dvars = Dvars {
            angle: 90.0,
            ..Dvars::default()
        };
        let (o, a) = camera(Vec3::ZERO, Vec3::ZERO, None, dvars, &open);
        // Backing off along yaw -90 puts it 120 to the left (+y).
        assert!(near(o, Vec3::new(0.0, 120.0, 8.0)), "{o}");
        assert_eq!(a.y, -90.0);
    }

    /// The offset takes half the view's pitch; the focus holds it at 45.
    #[test]
    fn the_offset_halves_the_pitch_and_the_focus_holds_it_at_45() {
        let view = Vec3::new(80.0, 0.0, 0.0);
        let (o, a) = camera(Vec3::ZERO, view, None, Dvars::default(), &open);
        let (s, c) = 40f32.to_radians().sin_cos();
        // Back along pitch 40 (down): behind and up.
        assert!(near(o, Vec3::new(-120.0 * c, 0.0, 8.0 + 120.0 * s)), "{o}");
        let (fs, fc) = 45f32.to_radians().sin_cos();
        let to = Vec3::new(512.0 * fc, 0.0, -512.0 * fs) - o;
        let want = (-to.z).atan2(to.x).to_degrees();
        assert!((a.x - want).abs() < 1e-3, "{a} {want}");
    }

    /// A dead playerstate faces its `stats[1]`.
    #[test]
    fn the_dead_view_faces_the_dead_yaw() {
        let (o, a) = camera(
            Vec3::ZERO,
            Vec3::new(0.0, 30.0, 0.0),
            Some(90.0),
            Dvars::default(),
            &open,
        );
        assert!(near(o, Vec3::new(0.0, -120.0, 8.0)), "{o}");
        assert_eq!(a.y, 90.0);
    }

    /// A wall halfway pulls the camera in to the trace's fraction, lifted
    /// 32 times what it missed, and traced again from the eye.
    #[test]
    fn a_wall_pulls_the_camera_in_and_lifts_it() {
        let calls = std::cell::RefCell::new(Vec::new());
        let half = |s: Vec3, e: Vec3| {
            calls.borrow_mut().push((s, e));
            if calls.borrow().len() == 1 { 0.5 } else { 1.0 }
        };
        let (o, _) = camera(Vec3::ZERO, Vec3::ZERO, None, Dvars::default(), &half);
        let calls = calls.into_inner();
        assert_eq!(calls.len(), 2);
        // The first from the eye, not 8 over it.
        assert_eq!(calls[0].0, Vec3::ZERO);
        assert!(near(calls[0].1, Vec3::new(-120.0, 0.0, 8.0)));
        assert!(near(calls[1].1, Vec3::new(-60.0, 0.0, 4.0 + 16.0)));
        assert!(near(o, calls[1].1));
    }

    /// The console's values, held to their domains.
    #[test]
    fn the_dvars_read_within_their_domains() {
        let d = Dvars::read(|n| match n {
            "cg_thirdPerson" => Some("1".into()),
            "cg_thirdPersonRange" => Some("5000".into()),
            "cg_thirdPersonAngle" => Some("-400".into()),
            _ => None,
        });
        assert_eq!(
            d,
            Dvars {
                on: true,
                range: 1024.0,
                angle: -180.0
            }
        );
        assert_eq!(Dvars::read(|_| None), Dvars::default());
        assert!(rendering(Dvars::default(), 7));
        assert!(!rendering(Dvars::default(), 6));
    }
}
