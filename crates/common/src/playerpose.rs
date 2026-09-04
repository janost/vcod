//! Posing a player: the clip a wire `legsAnim`/`torsoAnim` value names, and
//! the aim layer over it. Shared by the client's draw path and the server's
//! locational trace. `docs/research/player-model-anim-system.md`.

use crate::animtree::{AnimTree, PlayerAnims};
use crate::skeleton::{PoseBuffer, Skeleton};
use crate::xanim::XAnim;
use glam::Quat;
use std::rc::Rc;

#[derive(Clone, Copy, PartialEq)]
enum Axis {
    Pitch,
    Yaw,
}

/// `15down` -> pitch +15, `30left` -> yaw -30. Pitch is down-positive (engine
/// view pitch), yaw right-positive (offset from the body facing).
fn suffix_angle(tok: &str) -> Option<(Axis, f32)> {
    match tok {
        "level" => return Some((Axis::Pitch, 0.0)),
        "forward" => return Some((Axis::Yaw, 0.0)),
        _ => {}
    }
    for (word, axis, sign) in [
        ("down", Axis::Pitch, 1.0),
        ("up", Axis::Pitch, -1.0),
        ("left", Axis::Yaw, -1.0),
        ("right", Axis::Yaw, 1.0),
    ] {
        if let Some(num) = tok.strip_suffix(word) {
            if let Ok(deg) = num.parse::<f32>() {
                return Some((axis, sign * deg));
            }
        }
    }
    None
}

/// The angle `name`'s last token on `axis` annotates (`..._30right_15down`:
/// pitch +15, yaw +30).
fn name_angle(name: &str, axis: Axis) -> Option<f32> {
    name.split('_').rev().find_map(|tok| {
        suffix_angle(tok)
            .filter(|&(a, _)| a == axis)
            .map(|(_, v)| v)
    })
}

/// Picks the child nearest the requested angle on whichever axis every child
/// annotates and disagrees on (MG42: pitch rows, then yaw columns). Falls back
/// to the middle child.
fn pick_child(tree: &AnimTree, children: &[usize], pitch_deg: f32, yaw_deg: f32) -> usize {
    for (axis, want) in [(Axis::Pitch, pitch_deg), (Axis::Yaw, yaw_deg)] {
        let angles: Option<Vec<f32>> = children
            .iter()
            .map(|&c| name_angle(&tree.nodes[c].name, axis))
            .collect();
        let Some(angles) = angles else { continue };
        if angles.iter().all(|a| *a == angles[0]) {
            continue; // shared annotation, carries no choice
        }
        let best = angles
            .iter()
            .enumerate()
            .min_by(|(_, a), (_, b)| (*a - want).abs().total_cmp(&(*b - want).abs()))
            .map(|(i, _)| i)
            .unwrap_or(0);
        return children[best];
    }
    children[children.len() / 2]
}

/// Walks an aim group down to a leaf. Only the MG42 gunner groups are
/// non-leaves in the shipped tree.
pub fn descend_aim(tree: &AnimTree, node: usize, pitch_deg: f32, yaw_deg: f32) -> usize {
    let mut node = node;
    // cycle guard; the shipped tree nests two levels
    for _ in 0..8 {
        let children = &tree.nodes[node].children;
        if children.is_empty() {
            return node;
        }
        node = pick_child(tree, children, pitch_deg, yaw_deg);
    }
    node
}

/// Per-bone weight of `fTorsoPitch` on the back control bones; must sum to 1.0.
/// `BG_Player_DoControllers` (game.mp.i386.so 0x2b7f8) also mixes lean and
/// torso-height terms; its constants are not decoded. See
/// docs/research/player-model-anim-system.md, "Legs/torso split and aim layer".
const BACK_PITCH_WEIGHTS: [f32; 3] = [0.2, 0.3, 0.5]; // back_low, back_mid, back_up
const PELVIS_LEAN_DEG: f32 = 12.0;
const BACK_LEAN_DEG: f32 = 8.0;

/// Bends the spine control bones by the transmitted aim: `waist_pitch` on the
/// pelvis, `torso_pitch` split by [`BACK_PITCH_WEIGHTS`], `lean` (a fraction,
/// positive presumed right) as sideways roll. Call after the clips and before
/// `skin_matrices`.
///
/// No clip keys these bones, so each bend starts from the bind rotation, not
/// the previous frame's pose. `set_local_rot` overwrites; composing would spin
/// further every frame.
///
/// Assumes the bind keeps the lateral axis as local Y (true on USAirborne3),
/// so pitch is local Y and lean local X. Missing bones are skipped.
pub fn apply_aim(
    pose: &mut PoseBuffer,
    skel: &Skeleton,
    torso_pitch: f32,
    waist_pitch: f32,
    lean: f32,
) {
    let mut bend = |name: &str, pitch_deg: f32, roll_deg: f32| {
        if let Some(bi) = skel.bone_index(name) {
            let bind_rot = skel.bones()[bi].local_rot;
            let q = Quat::from_rotation_y(pitch_deg.to_radians())
                * Quat::from_rotation_x(roll_deg.to_radians());
            pose.set_local_rot(bi, q * bind_rot);
        }
    };
    bend("pelvis", waist_pitch, lean * PELVIS_LEAN_DEG);
    for (w, name) in BACK_PITCH_WEIGHTS
        .iter()
        .zip(["back_low", "back_mid", "back_up"])
    {
        bend(name, torso_pitch * w, lean * BACK_LEAN_DEG / 3.0);
    }
}

/// True when every child carries an aim annotation (an MG42 aim group, not a
/// container like `main` or `legs`).
fn is_aim_group(tree: &AnimTree, node: usize) -> bool {
    let children = &tree.nodes[node].children;
    !children.is_empty()
        && children.iter().all(|&c| {
            let name = &tree.nodes[c].name;
            name_angle(name, Axis::Pitch).is_some() || name_angle(name, Axis::Yaw).is_some()
        })
}

/// The clip a wire `legsAnim`/`torsoAnim` value names, descending MG42 aim
/// groups by aim. `None` for out of range or a container node.
pub fn clip_name(anims: &PlayerAnims, wire: i32, pitch_deg: f32, yaw_deg: f32) -> Option<&str> {
    let tree = &anims.tree;
    let node = anims.index.node_id(wire)?;
    let leaf = if tree.nodes[node].children.is_empty() {
        node
    } else if is_aim_group(tree, node) {
        descend_aim(tree, node, pitch_deg, yaw_deg)
    } else {
        return None;
    };
    Some(&tree.nodes[leaf].name)
}

/// What a player's pose is made of: the two wire anim indices, when each
/// started, and the aim the spine layer bends by.
pub struct PoseInputs<'a> {
    pub anims: &'a PlayerAnims,
    /// Wire `legsAnim` / `torsoAnim`, restart toggle included.
    pub legs: i32,
    pub torso: i32,
    /// serverTime each channel last (re)started, and the time to pose at.
    pub legs_start_ms: i32,
    pub torso_start_ms: i32,
    pub now_ms: i32,
    /// Degrees, and `lean` a fraction of full lean.
    pub torso_pitch: f32,
    pub waist_pitch: f32,
    pub lean: f32,
}

/// Legs then torso on one pose buffer, then the aim layer: `pl_*` clips key
/// the whole body and `pt_*` only the bones they name, which is what makes
/// the split work (player-model-anim-system.md, "Legs/torso split").
///
/// `clip` resolves a clip name to the loaded xanim. There is no cross-fade
/// between an outgoing and an incoming clip here, unlike the client's draw
/// path: this poses one instant, for one shot.
pub fn pose_player(
    skel: &Skeleton,
    inputs: &PoseInputs,
    mut clip: impl FnMut(&str) -> Option<Rc<XAnim>>,
) -> PoseBuffer {
    let mut pose = PoseBuffer::new(skel);
    for (wire, start_ms) in [
        (inputs.legs, inputs.legs_start_ms),
        (inputs.torso, inputs.torso_start_ms),
    ] {
        let Some(name) = clip_name(inputs.anims, wire, inputs.torso_pitch, 0.0) else {
            continue;
        };
        let Some(anim) = clip(name) else { continue };
        let t = inputs.now_ms.wrapping_sub(start_ms).max(0) as f32 / 1000.0;
        let binding = skel.bind(&anim);
        pose.apply(&anim, &binding, anim.frame_pos(t, anim.looping));
    }
    apply_aim(
        &mut pose,
        skel,
        inputs.torso_pitch,
        inputs.waist_pitch,
        inputs.lean,
    );
    pose
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skeleton::PoseBuffer;
    use crate::xmodel::{Bone, XModel};
    use glam::Vec3;

    const MG42_SAMPLE: &str = r#"
main
{
    legs
    {
        standMG42_aim : complete nonloopsync
        {
            standMG42_aim_15down
            {
                pb_standMG42gunner_aim_30left_15down
                pb_standMG42gunner_aim_forward_15down
                pb_standMG42gunner_aim_30right_15down
            }
            standMG42_aim_level
            {
                pb_standMG42gunner_aim_30left_level
                pb_standMG42gunner_aim_forward_level
                pb_standMG42gunner_aim_30right_level
            }
            standMG42_aim_15up
            {
                pb_standMG42gunner_aim_30left_15up
                pb_standMG42gunner_aim_forward_15up
                pb_standMG42gunner_aim_30right_15up
            }
        }
    }
}
"#;

    #[test]
    fn aim_group_descends_by_suffix() {
        let t = AnimTree::parse(MG42_SAMPLE).unwrap();
        let group = t.index_of("standMG42_aim").unwrap();
        let leaf_name = |leaf: usize| t.nodes[leaf].name.as_str();
        // Pitch -4 deg is nearest "level"; yaw offset +25 deg nearest 30right.
        assert_eq!(
            leaf_name(descend_aim(&t, group, -4.0, 25.0)),
            "pb_standMG42gunner_aim_30right_level"
        );
        // Pitch is down-positive (engine view pitch), so looking up picks _15up.
        assert_eq!(
            leaf_name(descend_aim(&t, group, -20.0, -25.0)),
            "pb_standMG42gunner_aim_30left_15up"
        );
        assert_eq!(
            leaf_name(descend_aim(&t, group, 40.0, 0.0)),
            "pb_standMG42gunner_aim_forward_15down"
        );
        // A leaf descends to itself.
        let leaf = t.index_of("pb_standMG42gunner_aim_forward_level").unwrap();
        assert_eq!(descend_aim(&t, leaf, 0.0, 0.0), leaf);
        // Unannotated levels (main > legs) fall back to the middle child and keep descending.
        let main = t.index_of("main").unwrap();
        assert_eq!(
            leaf_name(descend_aim(&t, main, 0.0, 0.0)),
            "pb_standMG42gunner_aim_forward_level"
        );
    }

    /// Each push composes against the bones before it, so `world` holds the
    /// full ancestor chain.
    fn bone(name: &str, parent: i32, local_pos: Vec3, local_rot: Quat, world: &[Bone]) -> Bone {
        let (pos, rot) = if parent >= 0 {
            let p = &world[parent as usize];
            (p.pos + p.rot * local_pos, p.rot * local_rot)
        } else {
            (local_pos, local_rot)
        };
        Bone {
            name: name.into(),
            parent,
            pos,
            rot,
            local_pos,
            local_rot,
            hit_mins: Vec3::ZERO,
            hit_maxs: Vec3::ZERO,
            hit_location: 0,
        }
    }

    /// tag_origin > pelvis > back_low > back_mid > back_up on +Z with identity
    /// binds, so local Y is the lateral axis.
    fn spine_fixture() -> XModel {
        let mut bones = vec![bone("tag_origin", -1, Vec3::ZERO, Quat::IDENTITY, &[])];
        for (name, parent) in [
            ("pelvis", 0i32),
            ("back_low", 1),
            ("back_mid", 2),
            ("back_up", 3),
        ] {
            let b = bone(name, parent, Vec3::Z, Quat::IDENTITY, &bones);
            bones.push(b);
        }
        XModel {
            lod: "spine".into(),
            surfaces: vec![],
            materials: vec![],
            bones,
            collision: Vec::new(),
        }
    }

    /// Bind rotations are identity, so the skin matrix's rotation is the posed
    /// world rotation.
    fn world_rot_of(pose: &PoseBuffer, skel: &Skeleton, bone: usize) -> Quat {
        Quat::from_mat4(&pose.skin_matrices(skel, 0)[bone])
    }

    #[test]
    fn aim_pitch_rotates_spine_chain() {
        let m = spine_fixture();
        let skel = Skeleton::build(&[&m]);
        let mut pose = PoseBuffer::new(&skel);
        apply_aim(&mut pose, &skel, 30.0, 0.0, 0.0);
        // back_up accumulates all three weights, the full torso pitch
        let up = skel.bone_index("back_up").unwrap();
        let w = world_rot_of(&pose, &skel, up);
        let v = w * glam::Vec3::Z;
        assert!(
            (v.angle_between(glam::Vec3::Z).to_degrees() - 30.0).abs() < 1.0,
            "{v}"
        );
    }

    #[test]
    fn aim_does_not_accumulate_across_frames() {
        let m = spine_fixture();
        let skel = Skeleton::build(&[&m]);
        let mut pose = PoseBuffer::new(&skel);
        let up = skel.bone_index("back_up").unwrap();

        apply_aim(&mut pose, &skel, 20.0, 5.0, 0.0);
        let first = world_rot_of(&pose, &skel, up);
        apply_aim(&mut pose, &skel, 20.0, 5.0, 0.0);
        let second = world_rot_of(&pose, &skel, up);

        assert!(
            first.abs_diff_eq(second, 1e-5),
            "aim pose drifted across identical frames: {first} != {second}"
        );
    }
}
