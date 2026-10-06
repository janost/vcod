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

/// `tag_weapon` in the gun's model space with the barrel's pitch and yaw on
/// `tag_aim`: the bind positions of the two tags and the wire `angles2`.
pub fn tag_weapon_local(aim: Vec3, weapon: Vec3, angles2: [f32; 3]) -> (Vec3, Quat) {
    let barrel = angles_quat([angles2[0], angles2[1], 0.0]);
    (aim + barrel * (weapon - aim), barrel)
}

/// What 0x515a8 (and the client's 0x300279b0) work out for a gunner.
#[derive(Clone, Debug)]
pub struct GunnerPlacement {
    /// World origin, before the trace down.
    pub origin: Vec3,
    /// The body's world rotation: the blend's root yaw plus `tag_weapon`'s,
    /// carried by the gun's axis.
    pub rot: Quat,
    /// The turret anim's leaves (animtree node ids) and the goal weights the
    /// placement gives them, summing to 1.
    pub leaves: Vec<(usize, f32)>,
}

impl GunnerPlacement {
    /// The body's world yaw in degrees, `AxisToAngles`' yaw of [`Self::rot`].
    pub fn yaw(&self) -> f32 {
        let f = self.rot * Vec3::X;
        f.y.atan2(f.x).to_degrees()
    }
}

/// Where 0x515a8 puts a gunner before its trace down, and the leaf blend
/// that puts it there. `None` when the legs anim is not a `turretanim` one
/// or a clip under it will not load, which skips the placement as retail's
/// flag test does.
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
) -> Option<GunnerPlacement> {
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
    Some(GunnerPlacement {
        origin: turret.0 + turret.1 * local,
        rot: turret.1 * Quat::from_rotation_z((rot_yaw + gun_yaw).to_radians()),
        leaves: weights,
    })
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
            let tag_weapon = tag_weapon_local(aim, weapon, [a2[0], a2[1], 0.0]);
            let at = place_gunner(
                &anims,
                |n| crate::xanim::load(&fs, n).ok().map(Rc::new),
                legs,
                tag_weapon,
                turret,
                Vec3::from(want),
                15.0,
            )
            .unwrap_or_else(|| panic!("line {line}: no placement"))
            .origin;
            // Horizontal only: z is the height handed in (turrets doc 7.2).
            let err = (at - Vec3::from(want)).truncate().length();
            assert!(err < 0.25, "line {line}: {at:?} vs {want:?}, off {err}");
        }
    }

    /// The yaw splits each row between the two columns either side of
    /// `n / 2 - yaw / inc` (turrets doc 7.2), and the leaves' root yaw undoes
    /// the barrel's, so the body keeps facing the gun's base and the leaves
    /// swing the arms.
    #[test]
    fn the_barrel_yaw_picks_the_columns_and_the_body_holds_still() {
        let Some(fs) = crate::testing::game_fs() else {
            return;
        };
        let anims = PlayerAnims::load(&fs).unwrap();
        let bones = crate::xmodel::load_bones(&fs, "mg42_bipod").unwrap();
        let tag = |n: &str| bones.iter().find(|b| b.name == n).unwrap().pos;
        let turret = (
            Vec3::new(1712.0, 1830.0, 8.0),
            angles_quat([0.0, 229.0, 0.0]),
        );
        let legs = anims.wire_of("standMG42_aim").unwrap();
        // A height the 40-down barrel puts wholly on the 15up row.
        let player = Vec3::new(1742.2, 1857.1, -23.9);
        for (yaw, cols) in [
            (0.0, ["forward", "15left"]),
            (45.0, ["45right", "30right"]),
            (-40.0, ["45left", "30left"]),
        ] {
            let tag_weapon = tag_weapon_local(tag("tag_aim"), tag("tag_weapon"), [40.0, yaw, 0.0]);
            let g = place_gunner(
                &anims,
                |n| crate::xanim::load(&fs, n).ok().map(Rc::new),
                legs,
                tag_weapon,
                turret,
                player,
                15.0,
            )
            .unwrap();
            let sum: f32 = g.leaves.iter().map(|l| l.1).sum();
            assert!((sum - 1.0).abs() < 1e-4, "yaw {yaw}: weights sum {sum}");
            for (leaf, w) in &g.leaves {
                let name = &anims.tree.nodes[*leaf].name;
                assert!(name.ends_with("_15up"), "yaw {yaw}: {name}");
                assert!(
                    cols.iter().any(|c| name.contains(&format!("_aim_{c}_"))),
                    "yaw {yaw}: {name} at {w}"
                );
            }
            let off = angle_delta(g.yaw(), 229.0);
            assert!(
                (-0.1..7.6).contains(&off),
                "yaw {yaw}: body {off} off the base"
            );
        }
    }

    fn angle_delta(a: f32, b: f32) -> f32 {
        (a - b + 540.0).rem_euclid(360.0) - 180.0
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
