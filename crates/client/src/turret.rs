//! A mounted gun on the client: the controller that turns its model by the
//! entity's `angles2`, which of its two anims plays, and the first-person eye
//! a gunner sees from. `cgame_mp_x86.dll` (1.1), docs/research/cod11-turrets.md
//! section 14; UO's view and barrel (uo cgame 0x30041550, 0x3001e9f0),
//! coduo-diff.md 19, "The turret view".

use glam::Vec3;
/// On a playerstate: riding a mounted gun, one bit pair per gun stance.
pub use vcod_common::net::flags::EF_MOUNTED;
/// Flipped on a teleport, and on a turret's first mounted frame (turrets doc 6.2).
use vcod_common::net::flags::EF_TELEPORT_BIT;
use vcod_common::skeleton::{PoseBuffer, Skeleton};
use vcod_common::turretpose::angles_quat;

/// On a turret entity: the server fired it this frame (turrets doc 6.3).
use vcod_common::net::flags::EF_FIRING;
/// `eType` of a `misc_mg42` / `misc_turret`.
pub use vcod_common::net::flags::ET_TURRET;
const ENTITYNUM_NONE: i32 = vcod_common::net::protocol::ENTITYNUM_NONE as i32;
/// The 0.1 s the controller hands its goal-weight call.
pub const ANIM_BLEND_MS: i32 = 100;

/// The gun's two anim slots: the weapon file's `idleAnim` and `fireAnim`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GunAnim {
    Idle = 1,
    Fire = 2,
}

/// 0x3001b110's pick: the gun the view rides in first person always idles,
/// any other plays its fire anim while the server flags a shot. UO's
/// (0x3001e9f0) also idles an overheated gun, `animMovetype` not 0; 1.1
/// passes 0.
pub fn gun_anim(ridden: bool, eflags: i32, anim_movetype: i32) -> GunAnim {
    if !ridden && eflags & EF_FIRING != 0 && anim_movetype == 0 {
        GunAnim::Fire
    } else {
        GunAnim::Idle
    }
}

/// The turret a playerstate rides, by the fields 0x30033220 tests. UO's
/// cgame never reads `viewlocked` (0x30041550, 0x3001e9f0, the reticle's
/// 0x30019ba0 caller), which its server leaves 0 on a gunner.
pub fn ridden(eflags: i32, viewlocked: i32, viewlocked_ent: i32, uo: bool) -> Option<u32> {
    if eflags & EF_MOUNTED == 0 || (viewlocked == 0 && !uo) || viewlocked_ent == ENTITYNUM_NONE {
        return None;
    }
    u32::try_from(viewlocked_ent).ok()
}

/// Whether an entity lerps from its older state: a flipped teleport bit makes
/// the newer state the current one at the snapshot transition (0x3002fc80),
/// so the barrel snaps across a mount.
pub fn interpolates(a_eflags: i32, b_eflags: i32) -> bool {
    (a_eflags ^ b_eflags) & EF_TELEPORT_BIT == 0
}

/// `angles2` between two snapshots, each slot by the shorter way round.
pub fn barrel(a: Option<[f32; 3]>, b: [f32; 3], f: f32) -> [f32; 3] {
    match a {
        Some(a) => std::array::from_fn(|i| crate::camera::lerp_angle(a[i], b[i], f)),
        None => b,
    }
}

/// The controller: `tag_aim` and `tag_aim_animated` turned by the barrel's
/// pitch and yaw, `tag_flash` pitched by `angles2[2]`, each in the bone's
/// own frame on top of whatever the anim left there.
pub fn apply_controller(pose: &mut PoseBuffer, skel: &Skeleton, barrel: [f32; 3]) {
    let aim = angles_quat([barrel[0], barrel[1], 0.0]);
    let flash = angles_quat([barrel[2], 0.0, 0.0]);
    for (tag, q) in [
        ("tag_aim", aim),
        ("tag_aim_animated", aim),
        ("tag_flash", flash),
    ] {
        if let Some(bi) = skel.bone_index(tag) {
            pose.set_local_rot(bi, pose.local_rot(bi) * q);
        }
    }
}

/// Where a gunner looks from (0x30033220): `tag_player` in the world, along
/// the gun's base angles plus the barrel's, shaken on a frame that fired.
/// On 1.5 (1.5 cgame 0x300348c0) and UO (0x30041550) along the view held
/// inside the gun's arcs, jittered on a frame that fired; UO's eye is
/// pulled back then too.
#[derive(Clone, Copy, Debug)]
pub struct TurretEye {
    pub pos: Vec3,
    /// Pitch, yaw, roll in degrees.
    pub angles: [f32; 3],
    /// UO: `tag_player`'s forward, left and up, which the caller's turn
    /// sway moves the eye along ([`sway_offset`]).
    pub axis: Option<[Vec3; 3]>,
    /// 1.5 and UO: the arcs or the jitter moved the view, so the client's
    /// angles go there (trap 0xf2, UO's 0xf7), pitch and yaw degrees, kick
    /// not taken off.
    pub snap: Option<[f32; 2]>,
    /// The damage kick and the sight bob go on top ([`TurretEye::kicked`]):
    /// 1.1 and 1.5, not UO.
    pub kicks: bool,
}

/// The gun's arcs about its drawn angles, as UO's turret entity carries
/// them: `[pitch, yaw]` lows in `origin2[1]`, `origin2[2]` and highs in
/// `leanf`, `origin2[0]`, each lerped as an angle (0x300415e6..0x3004163f).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct UoArcs {
    pub min: [f32; 2],
    pub max: [f32; 2],
}

impl UoArcs {
    /// The arcs off the gun's two snapshot states at fraction `f`.
    pub fn lerp(a: [f32; 4], b: [f32; 4], f: f32) -> UoArcs {
        let l = |i: usize| crate::camera::lerp_angle(a[i], b[i], f);
        UoArcs {
            min: [l(1), l(2)],
            max: [l(3), l(0)],
        }
    }

    /// The fields [`UoArcs::lerp`] reads, `origin2[0..3]` then `leanf`.
    pub const FIELDS: [&str; 4] = ["origin2[0]", "origin2[1]", "origin2[2]", "leanf"];

    /// The barrel the view asks for: each of pitch and yaw off `base`, put
    /// inside the arcs (0x30041646..0x300416a9, 0x3001ea77). Roll is kept.
    pub fn barrel(&self, view: [f32; 3], base: [f32; 2]) -> [f32; 2] {
        std::array::from_fn(|i| {
            let d = angle_subtract(view[i], base[i]);
            if d > self.max[i] {
                self.max[i]
            } else if d < self.min[i] {
                self.min[i]
            } else {
                d
            }
        })
    }
}

/// UO's eye on a ridden gun (0x30041550) before the turn sway: `tag_player`
/// at `tag` (origin and forward, left, up), the view `view` held inside the
/// arcs. `shake` is the firing phase ([`crate::play::seat::shake_phase`])
/// when `cg.snap`'s playerstate fires and the gun has not overheated: the
/// eye backs half a unit at most along the tag, and pitch and yaw each take
/// `2 * rand() / 32768 - 1` of `jitter` (`vertViewJitter`,
/// `horizViewJitter`), pitch drawn first.
pub fn uo_eye(
    tag: (Vec3, [Vec3; 3]),
    view: [f32; 3],
    base: [f32; 2],
    arcs: &UoArcs,
    shake: Option<f32>,
    jitter: [f32; 2],
    rand: &mut MsvcRand,
) -> TurretEye {
    let (origin, axis) = tag;
    let barrel = arcs.barrel(view, base);
    let held = [base[0] + barrel[0], base[1] + barrel[1]];
    let moved = (0..2).any(|i| angle_subtract(held[i], view[i]).abs() > 1e-4);
    let mut angles = [held[0], held[1], view[2]];
    let mut pos = origin;
    if let Some(t) = shake {
        pos += axis[0] * (t * -0.5);
        angles[0] += rand.shake() * jitter[0];
        angles[1] += rand.shake() * jitter[1];
    }
    TurretEye {
        pos,
        angles,
        axis: Some(axis),
        snap: moved.then_some(held),
        kicks: false,
    }
}

/// 1.5's eye on a ridden gun (1.5 cgame 0x300348c0): `tag_player` at `eye`,
/// the view held inside the arcs as UO's is, then on a `viewlocked` 2 frame
/// pitch and yaw each take `2 * rand() / 32768 - 1` of `jitter`
/// (`vertViewJitter`, `horizViewJitter`), pitch drawn first. The jittered
/// view is what trap 0xf2 hands back, so a firing gunner's own view walks.
/// No pull-back, no turn sway.
pub fn v15_eye(
    eye: Vec3,
    view: [f32; 3],
    base: [f32; 2],
    arcs: &UoArcs,
    jitter: Option<[f32; 2]>,
    rand: &mut MsvcRand,
) -> TurretEye {
    let barrel = arcs.barrel(view, base);
    let mut angles = [base[0] + barrel[0], base[1] + barrel[1], view[2]];
    if let Some([pitch, yaw]) = jitter {
        angles[0] += rand.shake() * pitch;
        angles[1] += rand.shake() * yaw;
    }
    let moved = (0..2).any(|i| angle_subtract(angles[i], view[i]).abs() > 1e-4);
    TurretEye {
        pos: eye,
        angles,
        axis: None,
        snap: moved.then_some([angles[0], angles[1]]),
        kicks: true,
    }
}

/// `AngleSubtract` (uo cgame 0x3004bd70): `a - b` folded into -180..180.
fn angle_subtract(a: f32, b: f32) -> f32 {
    let mut d = a - b;
    while d > 180.0 {
        d -= 360.0;
    }
    while d < -180.0 {
        d += 360.0;
    }
    d
}

/// The turn sway's offset in `tag_player`'s frame (0x300418a7..0x3004194b):
/// `fwd * o[0] + left * o[1] - up * o[2]`.
pub fn sway_offset(axis: [Vec3; 3], o: [f32; 3]) -> Vec3 {
    axis[0] * o[0] + axis[1] * o[1] - axis[2] * o[2]
}

impl TurretEye {
    /// `CG_OffsetFirstPersonView`'s damage kick and sight bob (`kick`, wire
    /// pitch, yaw, roll), which 1.1 and 1.5 add on top of the gun's view
    /// (cgame 0x30032ae0 after 0x30033220; 1.5 0x30034180 after
    /// 0x300348c0). UO's kick and bob (0x30015a10, 0x30015b50) refuse a
    /// mounted gun, so a UO eye keeps its angles.
    pub fn kicked(mut self, kick: [f32; 3]) -> TurretEye {
        if self.kicks {
            for (a, k) in self.angles.iter_mut().zip(kick) {
                *a += k;
            }
        }
        self
    }

    /// The camera's yaw and pitch in radians, pitch up positive.
    pub fn view(&self) -> (f32, f32) {
        let [pitch, yaw, _] = self.angles;
        (yaw.to_radians(), -pitch.to_radians())
    }
}

/// The C runtime's `rand()`, which 0x30033220 draws the shake from.
pub struct MsvcRand(u32);

impl Default for MsvcRand {
    fn default() -> Self {
        MsvcRand(1)
    }
}

impl MsvcRand {
    fn next(&mut self) -> i32 {
        self.0 = self.0.wrapping_mul(214013).wrapping_add(2531011);
        ((self.0 >> 16) & 0x7fff) as i32
    }

    /// `2 * rand() / 32768 - 1` degrees.
    pub fn shake(&mut self) -> f32 {
        let r = self.next() as f32 / 32768.0;
        r + r - 1.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_ridden_gun_idles_and_any_other_fires_on_the_flag() {
        assert_eq!(gun_anim(false, 0x408, 0), GunAnim::Fire);
        assert_eq!(gun_anim(false, 0x8, 0), GunAnim::Idle);
        assert_eq!(gun_anim(true, 0x408, 0), GunAnim::Idle);
        // UO: an overheated gun idles.
        assert_eq!(gun_anim(false, 0x408, 1), GunAnim::Idle);
    }

    #[test]
    fn only_a_locked_mount_rides_a_gun() {
        assert_eq!(ridden(0xC018, 1, 298, false), Some(298));
        assert_eq!(ridden(0x4000, 2, 298, false), Some(298));
        assert_eq!(ridden(0x18, 1, 298, false), None);
        assert_eq!(ridden(0xC018, 0, 298, false), None);
        assert_eq!(ridden(0xC018, 1, ENTITYNUM_NONE, false), None);
        // UO's gunner rides with `viewlocked` 0 (`*-uo-turret.bin`).
        assert_eq!(ridden(0xC018, 0, 298, true), Some(298));
        assert_eq!(ridden(0xC018, 0, ENTITYNUM_NONE, true), None);
    }

    /// The capture's gun: base yaw -131, arcs 45 left and right, 40 up and
    /// down (`*-uo-turret.bin`).
    fn capture_arcs() -> UoArcs {
        let at = [45.0, -40.0, -45.0, 40.0];
        UoArcs::lerp(at, at, 0.5)
    }

    #[test]
    fn the_uo_view_is_held_inside_the_arcs() {
        let arcs = capture_arcs();
        assert_eq!(arcs.min, [-40.0, -45.0]);
        assert_eq!(arcs.max, [40.0, 45.0]);
        let base = [0.0, -131.0];
        assert_eq!(arcs.barrel([10.0, -101.0, 0.0], base), [10.0, 30.0]);
        // Past the left arc, and across the 180 seam.
        assert_eq!(arcs.barrel([60.0, -60.0, 0.0], base), [40.0, 45.0]);
        assert_eq!(arcs.barrel([-50.0, 160.0, 0.0], base), [-40.0, -45.0]);
        let tag = (Vec3::new(1.0, 2.0, 3.0), [Vec3::X, Vec3::Y, Vec3::Z]);
        let mut rng = MsvcRand::default();
        let inside = uo_eye(
            tag,
            [10.0, -101.0, 2.0],
            base,
            &arcs,
            None,
            [0.0; 2],
            &mut rng,
        );
        assert_eq!(inside.angles, [10.0, -101.0, 2.0]);
        assert_eq!(inside.snap, None);
        assert_eq!(inside.pos, tag.0);
        let out = uo_eye(
            tag,
            [60.0, -60.0, 0.0],
            base,
            &arcs,
            None,
            [0.0; 2],
            &mut rng,
        );
        assert_eq!(out.snap, Some([40.0, -86.0]));
    }

    #[test]
    fn the_damage_kick_turns_a_1_1_gunner_and_not_a_uo_one() {
        let eye = |axis: Option<[Vec3; 3]>| TurretEye {
            pos: Vec3::ZERO,
            angles: [10.0, -131.0, 0.0],
            axis,
            snap: None,
            kicks: axis.is_none(),
        };
        let kick = [2.5, 0.0, -1.0];
        assert_eq!(eye(None).kicked(kick).angles, [12.5, -131.0, -1.0]);
        let uo = eye(Some([Vec3::X, Vec3::Y, Vec3::Z])).kicked(kick);
        assert_eq!(uo.angles, [10.0, -131.0, 0.0]);
        let mut rng = MsvcRand::default();
        let v15 = v15_eye(
            Vec3::ZERO,
            [10.0, -131.0, 0.0],
            [0.0, -131.0],
            &capture_arcs(),
            None,
            &mut rng,
        );
        assert_eq!(v15.kicked(kick).angles, [12.5, -131.0, -1.0]);
    }

    /// 1.5's view (1.5 cgame 0x300348c0) is held inside the arcs as UO's
    /// is, at the tag itself; a `viewlocked` 2 frame jitters it by the
    /// stock `mg42_bipod_*_mp`'s 0.15 and hands the jittered view back.
    #[test]
    fn a_1_5_gunner_is_held_inside_the_arcs_and_jitters_on_a_shot() {
        let arcs = capture_arcs();
        let base = [0.0, -131.0];
        let at = Vec3::new(1.0, 2.0, 3.0);
        let mut rng = MsvcRand::default();
        let inside = v15_eye(at, [10.0, -101.0, 2.0], base, &arcs, None, &mut rng);
        assert_eq!(
            (inside.pos, inside.angles, inside.snap),
            (at, [10.0, -101.0, 2.0], None)
        );
        assert!(inside.axis.is_none(), "no turn sway");
        let out = v15_eye(at, [60.0, -60.0, 0.0], base, &arcs, None, &mut rng);
        assert_eq!(out.snap, Some([40.0, -86.0]));

        let mut rng = MsvcRand::default();
        let mut expect = MsvcRand::default();
        let fired = v15_eye(
            at,
            [10.0, -101.0, 0.0],
            base,
            &arcs,
            Some([0.15; 2]),
            &mut rng,
        );
        let [p, y] = [expect.shake() * 0.15, expect.shake() * 0.15];
        assert_eq!(fired.angles, [10.0 + p, -101.0 + y, 0.0]);
        assert!(p.abs() <= 0.15 && y.abs() <= 0.15);
        assert_eq!(fired.snap, Some([10.0 + p, -101.0 + y]));
        assert_eq!(fired.pos, at);
    }

    #[test]
    fn a_firing_uo_gun_pulls_the_eye_back_and_jitters() {
        let arcs = capture_arcs();
        let tag = (Vec3::ZERO, [Vec3::X, Vec3::Y, Vec3::Z]);
        let mut rng = MsvcRand::default();
        // No stock UO turret sets the jitter: only the eye moves.
        let e = uo_eye(
            tag,
            [0.0, -131.0, 0.0],
            [0.0, -131.0],
            &arcs,
            Some(1.0),
            [0.0; 2],
            &mut rng,
        );
        assert_eq!(e.pos, Vec3::new(-0.5, 0.0, 0.0));
        assert_eq!(e.angles, [0.0, -131.0, 0.0]);
        // 1.5's MG42 file's 0.15: msvcrt's first two draws from seed 1.
        let mut rng = MsvcRand::default();
        let e = uo_eye(
            tag,
            [0.0, -131.0, 0.0],
            [0.0, -131.0],
            &arcs,
            Some(1.0),
            [0.15; 2],
            &mut rng,
        );
        let r = |n: f32| (2.0 * n / 32768.0 - 1.0) * 0.15;
        assert!((e.angles[0] - r(41.0)).abs() < 1e-6, "{e:?}");
        assert!((e.angles[1] + 131.0 - r(18467.0)).abs() < 1e-4, "{e:?}");
        assert_eq!(
            sway_offset([Vec3::X, Vec3::Y, Vec3::Z], [1.0, 2.0, 3.0]),
            Vec3::new(1.0, 2.0, -3.0)
        );
    }

    #[test]
    fn the_barrel_lerps_the_short_way_round() {
        let b = barrel(Some([0.0, 170.0, 0.0]), [10.0, -170.0, 0.0], 0.5);
        assert!((b[0] - 5.0).abs() < 1e-5);
        assert!((b[1].abs() - 180.0).abs() < 1e-4, "{b:?}");
        assert_eq!(barrel(None, [1.0, 2.0, 3.0], 0.5), [1.0, 2.0, 3.0]);
    }

    #[test]
    fn a_flipped_teleport_bit_snaps() {
        assert!(interpolates(0x400, 0x0));
        assert!(!interpolates(0x0, 0x8));
        assert!(!interpolates(0x408, 0x400));
    }

    #[test]
    fn the_shake_stays_inside_a_degree() {
        let mut rng = MsvcRand::default();
        // msvcrt's first three draws from seed 1.
        assert_eq!([rng.next(), rng.next(), rng.next()], [41, 18467, 6334]);
        for _ in 0..1000 {
            let s = rng.shake();
            assert!((-1.0..1.0).contains(&s), "{s}");
        }
    }

    /// The controller turns `tag_player` and `tag_flash` about `tag_aim` the
    /// way the server's `tag_weapon_local` and `muzzle` read the bind pose.
    #[test]
    fn the_controller_swings_the_tags_about_tag_aim() {
        let Some(fs) = vcod_common::testing::game_fs() else {
            return;
        };
        let model = vcod_common::model::load(&fs, "mg42_bipod").unwrap();
        let skel = Skeleton::build(&[&model]);
        let at =
            |pose: &PoseBuffer, tag: &str| pose.bone_world(&skel, skel.bone_index(tag).unwrap());
        let bind = PoseBuffer::new(&skel);
        let (aim, _) = at(&bind, "tag_aim");
        let (player, _) = at(&bind, "tag_player");
        let (flash, _) = at(&bind, "tag_flash");
        let a2 = [20.0, -30.0, 0.0];
        let mut pose = PoseBuffer::new(&skel);
        apply_controller(&mut pose, &skel, a2);
        let turn = angles_quat(a2);
        let (p, _) = at(&pose, "tag_player");
        assert!(p.abs_diff_eq(aim + turn * (player - aim), 1e-3), "{p:?}");
        // The gun's mesh rides tag_aim_animated, whose origin sits on tag_aim's
        // to a tenth of a unit, so the flash swings the same way.
        let (f, r) = at(&pose, "tag_flash");
        assert!(f.abs_diff_eq(aim + turn * (flash - aim), 0.1), "{f:?}");
        assert!((r * Vec3::X).abs_diff_eq(turn * Vec3::X, 1e-3));
        // Pitch positive is down, as AnglesToAxis reads it.
        assert!((r * Vec3::X).z < 0.0);
    }
}
