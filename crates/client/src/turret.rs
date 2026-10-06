//! A mounted gun on the client: the controller that turns its model by the
//! entity's `angles2`, which of its two anims plays, and the first-person eye
//! a gunner sees from. `cgame_mp_x86.dll` (1.1), docs/research/cod11-turrets.md
//! section 14.

use glam::Vec3;
/// On a playerstate: riding a mounted gun, one bit pair per gun stance.
pub use vcod_common::net::flags::EF_MOUNTED;
/// Flipped on a teleport, and on a turret's first mounted frame (turrets doc 6.2).
use vcod_common::net::flags::EF_TELEPORT_BIT;
use vcod_common::skeleton::{PoseBuffer, Skeleton};
use vcod_common::turretpose::angles_quat;

/// `eType` of a `misc_mg42` / `misc_turret`.
pub const ET_TURRET: i32 = 11;
/// On a turret entity: the server fired it this frame (turrets doc 6.3).
const EF_FIRING: i32 = 0x400;
const ENTITYNUM_NONE: i32 = 1023;
/// The 0.1 s the controller hands its goal-weight call.
pub const ANIM_BLEND_MS: i32 = 100;

/// The gun's two anim slots: the weapon file's `idleAnim` and `fireAnim`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GunAnim {
    Idle = 1,
    Fire = 2,
}

/// 0x3001b110's pick: the gun the view rides in first person always idles,
/// any other plays its fire anim while the server flags a shot.
pub fn gun_anim(ridden: bool, eflags: i32) -> GunAnim {
    if !ridden && eflags & EF_FIRING != 0 {
        GunAnim::Fire
    } else {
        GunAnim::Idle
    }
}

/// The turret a playerstate rides, by the fields 0x30033220 tests.
pub fn ridden(eflags: i32, viewlocked: i32, viewlocked_ent: i32) -> Option<u32> {
    if eflags & EF_MOUNTED == 0 || viewlocked == 0 || viewlocked_ent == ENTITYNUM_NONE {
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
#[derive(Clone, Copy, Debug)]
pub struct TurretEye {
    pub pos: Vec3,
    /// Pitch, yaw, roll in degrees.
    pub angles: [f32; 3],
}

impl TurretEye {
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
        assert_eq!(gun_anim(false, 0x408), GunAnim::Fire);
        assert_eq!(gun_anim(false, 0x8), GunAnim::Idle);
        assert_eq!(gun_anim(true, 0x408), GunAnim::Idle);
    }

    #[test]
    fn only_a_locked_mount_rides_a_gun() {
        assert_eq!(ridden(0xC018, 1, 298), Some(298));
        assert_eq!(ridden(0x4000, 2, 298), Some(298));
        assert_eq!(ridden(0x18, 1, 298), None);
        assert_eq!(ridden(0xC018, 0, 298), None);
        assert_eq!(ridden(0xC018, 1, ENTITYNUM_NONE), None);
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
        let model = vcod_common::xmodel::load(&fs, "mg42_bipod").unwrap();
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
