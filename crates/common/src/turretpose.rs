//! The gunner's body placement, `game.mp.i386.so` 0x515a8
//! (docs/research/cod11-turrets.md section 7): the mounted anim's blended
//! root delta, hung off the turret's `tag_weapon`, is where the player
//! stands.

use crate::animtree::PlayerAnims;
use crate::xanim::XAnim;
use glam::{EulerRot, Quat, Vec3};
use std::rc::Rc;

/// Wire angles (pitch, yaw, roll, degrees) as the rotation `AnglesToAxis`
/// builds: its forward, left and up are this rotation's X, Y and Z.
pub fn angles_quat(angles: [f32; 3]) -> Quat {
    let [p, y, r] = angles.map(f32::to_radians);
    Quat::from_euler(EulerRot::ZYX, y, p, r)
}

/// Where 0x515a8 puts a gunner before its trace down: the world origin and
/// the body's world yaw in degrees. `None` when the legs anim is not a
/// `turretanim` one or a clip under it will not load, which skips the
/// placement as retail's flag test does.
///
/// `tag_weapon` is the tag in the turret's model space with the barrel's
/// `angles2` already on `tag_aim`; `turret` is the gun's world origin and
/// rotation; `player_origin` the gunner's current origin, whose height above
/// the gun picks the pitch row.
pub fn place_gunner(
    anims: &PlayerAnims,
    mut clip: impl FnMut(&str) -> Option<Rc<XAnim>>,
    legs_anim: i32,
    tag_weapon: (Vec3, Quat),
    turret: (Vec3, Quat),
    player_origin: Vec3,
    rotate_inc: f32,
) -> Option<(Vec3, f32)> {
    let tree = &anims.tree;
    let node = anims.index.node_id(legs_anim)?;
    if !anims.script.is_turret_anim(&tree.nodes[node].name) {
        return None;
    }
    // The engine's child order is the reverse of the file's (research doc
    // player-model-anim-system.md, "Animation indices: the animtree").
    let children = |n: usize| tree.nodes[n].children.iter().rev().copied();
    let forward = tag_weapon.1 * Vec3::X;
    let gun_yaw = if forward.x == 0.0 && forward.y == 0.0 {
        0.0
    } else {
        forward.y.atan2(forward.x).to_degrees()
    };
    let up = turret.1 * Vec3::Z;
    let height = (player_origin - turret.0).dot(up);
    let target = height - tag_weapon.0.z;

    // One row's yaw blend: the two columns either side of the gun's yaw.
    let row_weights = |row: usize| -> Option<Vec<(usize, f32)>> {
        let cols: Vec<usize> = children(row).collect();
        let last = cols.len().checked_sub(1)?;
        let x = (cols.len() as f32 * 0.5 - gun_yaw / rotate_inc).clamp(0.0, last as f32);
        let i = x as usize;
        let f = x - i as f32;
        let mut w = vec![(cols[i], 1.0 - f)];
        if f != 0.0 {
            w.push((cols[i + 1], f));
        }
        Some(w)
    };
    let mut delta = |weights: &[(usize, f32)]| -> Option<(Vec3, f32)> {
        let (mut trans, mut rot) = (Vec3::ZERO, Quat::from_xyzw(0.0, 0.0, 0.0, 0.0));
        for &(leaf, w) in weights {
            let anim = clip(&tree.nodes[leaf].name)?;
            let (t, r) = anim.root.as_ref().map_or((None, None), |r| r.sample(0.0));
            trans += w * t.unwrap_or(Vec3::ZERO);
            rot += r.unwrap_or(Quat::IDENTITY) * w;
        }
        Some((trans, 2.0 * rot.z.atan2(rot.w).to_degrees()))
    };

    // The rows are searched for the first whose delta sits at or above the
    // gun's height and blended with the one before it (turrets doc 7.1).
    let mut below: Option<(Vec<(usize, f32)>, f32)> = None;
    let mut weights = None;
    for row in children(node) {
        let w = row_weights(row)?;
        let z = delta(&w)?.0.z;
        if z >= target {
            weights = Some(match below.take() {
                None => w,
                Some((bw, bz)) => {
                    let t = (target - bz) / (z - bz);
                    let mut mix: Vec<_> = w.into_iter().map(|(l, x)| (l, x * t)).collect();
                    mix.extend(bw.into_iter().map(|(l, x)| (l, x * (1.0 - t))));
                    mix
                }
            });
            break;
        }
        below = Some((w, z));
    }
    let weights = weights.or(below.map(|(w, _)| w))?;
    let (trans, rot_yaw) = delta(&weights)?;

    let local = Quat::from_rotation_z(gun_yaw.to_radians()) * trans + tag_weapon.0;
    let local = Vec3::new(local.x, local.y, height);
    let body = turret.1 * Quat::from_rotation_z((rot_yaw + gun_yaw).to_radians()) * Vec3::X;
    Some((
        turret.0 + turret.1 * local,
        body.y.atan2(body.x).to_degrees(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn angles_quat_matches_angles_to_axis() {
        let a = [20.0, 229.0, -10.0];
        let axis = crate::pmove::aim::angles_to_axis(a);
        let q = angles_quat(a);
        for (v, want) in [Vec3::X, Vec3::Y, Vec3::Z].into_iter().zip(axis) {
            assert!((q * v).abs_diff_eq(Vec3::from(want), 1e-5), "{:?}", q * v);
        }
    }

    /// Three snapshots of the retail capture
    /// (`crates/server/tests/fixtures/turret/mp_carentan-dm-turret.txt`,
    /// turrets doc 12.2): the barrel centred, at the left arc, and pitched
    /// fully down. The gun is at (1712, 1830, 8) facing 229.
    #[test]
    fn the_captured_gunner_origins_replay() {
        let Some(fs) = crate::testing::game_fs() else {
            return;
        };
        let anims = PlayerAnims::load(&fs).unwrap();
        let bones = crate::xmodel::load_bones(&fs, "mg42_bipod").unwrap();
        let tag = |n: &str| bones.iter().find(|b| b.name == n).unwrap().pos;
        let (aim, weapon) = (tag("tag_aim"), tag("tag_weapon"));
        let turret = (
            Vec3::new(1712.0, 1830.0, 8.0),
            angles_quat([0.0, 229.0, 0.0]),
        );
        let legs = anims.wire_of("standMG42_aim").unwrap();
        // (fixture line, angles2, captured origin)
        for (line, a2, want) in [
            (533, [-2.0, 0.0], [1746.5, 1862.0, -23.9]),
            (250, [0.0, 45.0], [1715.4, 1877.3, -23.9]),
            (568, [40.0, 0.0], [1742.2, 1857.1, -23.9]),
        ] {
            let barrel = angles_quat([a2[0], a2[1], 0.0]);
            let tag_weapon = (aim + barrel * (weapon - aim), barrel);
            let (at, _) = place_gunner(
                &anims,
                |n| crate::xanim::load(&fs, n).ok().map(Rc::new),
                legs,
                tag_weapon,
                turret,
                Vec3::from(want),
                15.0,
            )
            .unwrap_or_else(|| panic!("line {line}: no placement"));
            let err = (at - Vec3::from(want)).length();
            assert!(err < 0.25, "line {line}: {at:?} vs {want:?}, off {err}");
        }
    }

    #[test]
    fn a_leaf_anim_without_turretanim_places_nothing() {
        let Some(fs) = crate::testing::game_fs() else {
            return;
        };
        let anims = PlayerAnims::load(&fs).unwrap();
        let legs = anims.wire_of("pb_crouch_alert").unwrap();
        let id = (Vec3::ZERO, Quat::IDENTITY);
        let placed = place_gunner(&anims, |_| None, legs, id, id, Vec3::ZERO, 15.0);
        assert!(placed.is_none());
    }
}
