//! The melee charge, 1.7's lunge: a melee key press whose cmd carries a
//! distance (the client's target pick, `meleeChargeDist`) throws the body
//! along `meleeChargeYaw` at the speed that friction stops in that
//! distance (docs/research/cod4-pmove.md 17).

use glam::Vec3;

use super::{PlayerState, Pml, Pmove, pm_type, pmf};

/// `PM_MeleeChargeStart` (`cod4_lnxded` 0x80561e6): the swing's cmd sets
/// the charge off when it asks for one, else clears it.
pub(crate) fn start(pm: &mut Pmove) {
    if pm.cmd.melee_charge_dist == 0 {
        clear(pm.ps);
        return;
    }
    let ps = &mut *pm.ps;
    ps.pm_flags |= pmf::MELEE_CHARGE;
    ps.melee_charge_yaw = short2angle(pm.cmd.melee_charge_yaw);
    ps.melee_charge_dist = i32::from(pm.cmd.melee_charge_dist);
    ps.melee_charge_time = 0;
}

/// The usercmd's short yaw as `MSG_ReadDeltaUsercmdKey` leaves it.
fn short2angle(s: u16) -> f32 {
    (f64::from(s) * (360.0 / 65536.0)) as f32
}

/// `PM_MeleeChargeClear` (inlined at 0x805bf6c and 0x805c32f).
pub(crate) fn clear(ps: &mut PlayerState) {
    ps.pm_flags &= !pmf::MELEE_CHARGE;
    ps.melee_charge_yaw = 0.0;
    ps.melee_charge_dist = 0;
    ps.melee_charge_time = 0;
}

/// The lunge's step, run in `PmoveSingle` after `Mantle_ClearHint`
/// (0x805bf55; cod4-pmove.md 19). It lives only on a normal player off a
/// turret (`eFlags` 0x300), not mantling and not on a ladder; anywhere
/// else it is cleared. Its first step throws the body (see [`launch`]);
/// every step spends its msec, and the lunge ends at 0.
pub(crate) fn update(pm: &mut Pmove, pml: &Pml) {
    let ps = &mut *pm.ps;
    let live = ps.pm_flags & pmf::MELEE_CHARGE != 0
        && ps.pm_type == pm_type::NORMAL
        && ps.e_flags & 0x300 == 0
        && ps.pm_flags & (pmf::MANTLE | pmf::LADDER) == 0;
    let left = live.then(|| {
        let total = match ps.melee_charge_time {
            0 => launch(ps, pm.rules.melee_charge_friction),
            t => t,
        };
        total - pml.msec
    });
    match left {
        Some(t) if t > 0 => ps.melee_charge_time = t,
        _ => clear(ps),
    }
}

/// The lunge's throw: the flat velocity along the charge yaw at the speed
/// `player_meleeChargeFriction` stops in the charge distance, `sqrt(2 *
/// dist * friction)` (the x87 root unrounded, 0x805d471), and the ms it
/// takes to stop, `(int)(float)(v / friction * 1000)` (0x805c2ef..0x805c354,
/// 0x805d442..0x805d4bd).
fn launch(ps: &mut PlayerState, friction: f32) -> i32 {
    let friction = f64::from(friction);
    let speed = (f64::from(ps.melee_charge_dist) * friction * 2.0).sqrt();
    let [c, s] = yaw_vectors_2d(ps.melee_charge_yaw);
    ps.velocity.x = (speed * f64::from(c)) as f32;
    ps.velocity.y = (speed * f64::from(s)) as f32;
    ((speed / friction * 1000.0) as f32) as i32
}

/// `YawVectors2D` (0x81955e4): `sinf` and `cosf` of the yaw in radians,
/// its product with the double `pi / 180` stored to a float.
fn yaw_vectors_2d(yaw: f32) -> [f32; 2] {
    let r = (f64::from(yaw) * (std::f64::consts::PI / 180.0)) as f32;
    [r.cos(), r.sin()]
}

/// `aim_automelee_range` (128), `aim_automelee_region_width` (320) and
/// `..height` (240) out of 640x480, `player_meleeRange` (64) and
/// `aim_target_sentient_radius` (10): 1.7's defaults (`iw3mp.exe`
/// 0x401c63, 0x401b9f, 0x401c01, 0x40b801, 0x4033b3).
pub const AUTOMELEE_RANGE: f32 = 128.0;
pub const AUTOMELEE_REGION: [f32; 2] = [320.0 / 640.0, 240.0 / 480.0];
pub const MELEE_RANGE: f32 = 64.0;
pub const SENTIENT_RADIUS: f32 = 10.0;

/// The eye the pick projects through: `refdef`'s origin and angles, the
/// tangents of the half fov and the region's scale for a zoomed fov
/// (`fovScaleInv`, 1 at `cg_fov`).
#[derive(Clone, Copy, Debug)]
pub struct AimEye {
    pub origin: Vec3,
    pub angles: Vec3,
    pub tan_half_fov: [f32; 2],
    pub fov_scale_inv: f32,
}

/// One body the pick may lunge at, already through the target lists'
/// filters (an enemy, or anyone without teams, whose `j_head` the eye
/// sees): its origin and yaw, `j_spine4` (the aim point) and `j_helmet`'s
/// height over the origin (the box's top).
#[derive(Clone, Copy, Debug)]
pub struct AimBody {
    pub origin: Vec3,
    pub yaw: f32,
    pub aim: Vec3,
    pub top: f32,
}

/// The client's melee charge pick (`iw3mp.exe` 0x402b60, docs/research/
/// cod4-pmove.md 19): with the melee key held off prone, the body whose
/// projected box is nearest the crosshair (the nearer one on a tie) among
/// those within `aim_automelee_range` of `own` (the predicted origin) whose
/// box meets the middle region. At or past `player_meleeRange` the cmd
/// carries the flat yaw from the eye to its aim point and the distance,
/// truncated; `(0, 0)` otherwise.
pub fn client_pick(
    buttons: u32,
    pm_flags: u32,
    own: Vec3,
    eye: &AimEye,
    bodies: &[AimBody],
) -> (f32, u8) {
    const NONE: (f32, u8) = (0.0, 0);
    if buttons & super::button::MELEE == 0 || pm_flags & pmf::PRONE != 0 {
        return NONE;
    }
    let view = vcod_common::x87::angle_vectors(eye.angles);
    let region = AUTOMELEE_REGION.map(|r| r * eye.fov_scale_inv);
    let best = bodies
        .iter()
        .filter_map(|body| {
            let off = body.origin - own;
            // At most the sentient radius behind the view's plane.
            if view[0].dot(off) + SENTIENT_RADIUS < 0.0 {
                return None;
            }
            let dist_sq = off.length_squared();
            if dist_sq > AUTOMELEE_RANGE * AUTOMELEE_RANGE {
                return None;
            }
            let rect = ScreenRect::of_body(eye, &view, body)?;
            rect.meets(region)
                .then(|| (rect.centre_dist_sq(), dist_sq, body))
        })
        .min_by(|a, b| a.0.total_cmp(&b.0).then(a.1.total_cmp(&b.1)));
    match best {
        Some((_, dist_sq, body)) if dist_sq >= MELEE_RANGE * MELEE_RANGE => {
            let to_aim = body.aim - eye.origin;
            let yaw = super::fpu::vectoyaw(Vec3::new(to_aim.x, to_aim.y, 0.0));
            // `fistp` under a truncating control word.
            (yaw, dist_sq.sqrt() as i32 as u8)
        }
        _ => NONE,
    }
}

/// A body's box on screen in normalized device units (x right, y down,
/// each -1..1).
#[derive(Clone, Copy, Debug)]
struct ScreenRect {
    min: [f32; 2],
    max: [f32; 2],
}

impl ScreenRect {
    /// The body's box (the sentient radius about its origin, from the
    /// origin to `top`, turned by its yaw) through an infinite perspective
    /// with a near plane of 1. Corners behind the eye drop out; `None`
    /// when what is left has no area or depth, or misses the screen.
    fn of_body(eye: &AimEye, view: &[Vec3; 3], body: &AimBody) -> Option<ScreenRect> {
        let [forward, right, up] = *view;
        let (sin, cos) = body.yaw.to_radians().sin_cos();
        let r = SENTIENT_RADIUS;
        let corners = [0.0, body.top].into_iter().flat_map(|z| {
            [(-r, -r), (r, -r), (-r, r), (r, r)]
                .map(|(x, y)| Vec3::new(cos * x - sin * y, sin * x + cos * y, z))
        });
        // x, y and the depth `1 - 1 / f` of each corner in front.
        let projected = corners.filter_map(|c| {
            let p = body.origin + c - eye.origin;
            let f = forward.dot(p);
            (f > 0.0).then(|| {
                [
                    -(right.dot(p) / (f * eye.tan_half_fov[0])),
                    -(up.dot(p) / (f * eye.tan_half_fov[1])),
                    1.0 - 1.0 / f,
                ]
            })
        });
        let (lo, hi) = projected.fold(([f32::MAX; 3], [-f32::MAX; 3]), |(lo, hi), q| {
            (
                std::array::from_fn(|i| lo[i].min(q[i])),
                std::array::from_fn(|i| hi[i].max(q[i])),
            )
        });
        let flat = (0..3).any(|i| hi[i] <= lo[i]);
        let off = lo[..2].iter().any(|&v| v > 1.0)
            || hi[..2].iter().any(|&v| v < -1.0)
            || lo[2] > 1.0
            || hi[2] < 0.0;
        if flat || off {
            return None;
        }
        let clamp = |v: f32| v.clamp(-1.0, 1.0);
        Some(ScreenRect {
            min: [clamp(lo[0]), clamp(lo[1])],
            max: [clamp(hi[0]), clamp(hi[1])],
        })
    }

    /// Whether the rect overlaps the middle region `[-w, w] x [-h, h]`.
    fn meets(&self, [w, h]: [f32; 2]) -> bool {
        self.min[0] <= w && self.max[0] >= -w && self.min[1] <= h && self.max[1] >= -h
    }

    /// The rect's centre's squared distance from the crosshair.
    fn centre_dist_sq(&self) -> f32 {
        let [x, y] = std::array::from_fn(|i| 0.5 * (self.min[i] + self.max[i]));
        x * x + y * y
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pmove::button;

    fn eye() -> AimEye {
        let t = 32.5f32.to_radians().tan();
        AimEye {
            origin: Vec3::new(0.0, 0.0, 60.0),
            angles: Vec3::ZERO,
            tan_half_fov: [t, t * 0.75],
            fov_scale_inv: 1.0,
        }
    }

    fn body(x: f32, y: f32) -> AimBody {
        AimBody {
            origin: Vec3::new(x, y, 0.0),
            yaw: 180.0,
            aim: Vec3::new(x, y, 48.0),
            top: 70.0,
        }
    }

    #[test]
    fn the_pick_lunges_between_melee_range_and_automelee_range() {
        let pick =
            |x: f32, y: f32| client_pick(button::MELEE, 0, Vec3::ZERO, &eye(), &[body(x, y)]);
        assert_eq!(pick(50.0, 0.0), (0.0, 0), "inside player_meleeRange");
        assert_eq!(pick(96.5, 0.0), (0.0, 96));
        assert_eq!(pick(140.0, 0.0), (0.0, 0), "past aim_automelee_range");
        let (yaw, dist) = pick(90.0, 30.0);
        assert_eq!(dist, 94);
        assert!((yaw - 18.43).abs() < 0.01, "{yaw}");
        // Off the region: a body at 60 degrees is on screen but outside
        // the middle half.
        let a = 60f32.to_radians();
        assert_eq!(pick(100.0 * a.cos(), 100.0 * a.sin()).1, 0);
        // Prone, or no melee key: nothing.
        let none = client_pick(0, 0, Vec3::ZERO, &eye(), &[body(96.0, 0.0)]);
        assert_eq!(none, (0.0, 0));
        let prone = client_pick(
            button::MELEE,
            pmf::PRONE,
            Vec3::ZERO,
            &eye(),
            &[body(96.0, 0.0)],
        );
        assert_eq!(prone, (0.0, 0));
    }

    /// The pick over a grid of views, zooms, body spots, yaws and heights,
    /// one body and pairs, pinned by an FNV-1a digest of `(yaw bits, dist)`.
    #[test]
    fn the_pick_holds_over_a_grid() {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        let mut lunges = 0;
        let mut eat = |(yaw, dist): (f32, u8)| {
            lunges += usize::from(dist != 0);
            for b in yaw.to_bits().to_le_bytes().into_iter().chain([dist]) {
                h = (h ^ u64::from(b)).wrapping_mul(0x100_0000_01b3);
            }
        };
        let own = Vec3::new(3.0, -2.0, 0.0);
        let mut spots = Vec::new();
        for r in [40.0f32, 70.0, 100.0, 127.0, 135.0] {
            for deg in (-60..=60).step_by(8) {
                let a = (deg as f32).to_radians();
                for z in [-40.0f32, 0.0, 30.0] {
                    spots.push(Vec3::new(r * a.cos(), r * a.sin(), z));
                }
            }
        }
        for pitch in [-20.0f32, 0.0, 15.0, 40.0] {
            for yaw in [0.0f32, 12.0, -25.0] {
                for fov_scale_inv in [1.0f32, 0.55] {
                    let e = AimEye {
                        angles: Vec3::new(pitch, yaw, 0.0),
                        fov_scale_inv,
                        ..eye()
                    };
                    for (i, &p) in spots.iter().enumerate() {
                        let b = AimBody {
                            origin: own + p,
                            yaw: (i * 37 % 360) as f32,
                            aim: own + p + Vec3::new(0.0, 0.0, 48.0),
                            top: if i % 2 == 0 { 70.0 } else { 30.0 },
                        };
                        eat(client_pick(button::MELEE, 0, own, &e, &[b]));
                        let other = spots[(i * 7 + 3) % spots.len()];
                        let c = AimBody {
                            origin: own + other,
                            aim: own + other + Vec3::new(0.0, 0.0, 40.0),
                            ..b
                        };
                        eat(client_pick(button::MELEE, 0, own, &e, &[b, c]));
                    }
                }
            }
        }
        assert!(lunges > 500 && lunges < 5000, "{lunges}");
        assert_eq!(h, 0xfef1_e593_0c88_c6f0, "digest {h:#x}");
    }

    #[test]
    fn the_pick_takes_the_body_nearest_the_crosshair() {
        let near_edge = body(70.0, 30.0);
        let centred = body(110.0, 0.0);
        let (yaw, dist) = client_pick(button::MELEE, 0, Vec3::ZERO, &eye(), &[near_edge, centred]);
        assert_eq!((yaw, dist), (0.0, 110));
    }
}
