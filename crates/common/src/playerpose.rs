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
        if let Some(num) = tok.strip_suffix(word)
            && let Ok(deg) = num.parse::<f32>()
        {
            return Some((axis, sign * deg));
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

const PELVIS_LEAN_DEG: f32 = 12.0;
const BACK_LEAN_DEG: f32 = 8.0;

/// The eased torso pitch the spine controllers bend by:
/// `BG_PlayerAnimation`'s pitch swing (`game.mp.i386.so` 0x2b2ac..0x2b317,
/// cgame 0x30004343), stored in the client record at `+0x3b4` with its
/// swinging flag at `+0x3b8`. `docs/research/cod11-combat.md` 16.3.
#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct PitchSwing {
    /// Degrees, engine convention (down positive), folded to 0..360 by
    /// `AngleMod`.
    pub angle: f32,
    pub swinging: bool,
}

impl PitchSwing {
    /// One `BG_PlayerAnimation` step: `view_pitch` is the record's
    /// `viewangles[0]` (engine degrees), `frametime_ms` the game's frame
    /// length on the server and `cg.frametime` on the client. `rest` is a
    /// dead, mounted or climbing body, whose swing heads for 0.
    pub fn step(&mut self, view_pitch: f32, frametime_ms: i32, rest: bool) {
        let dest = if rest {
            0.0
        } else {
            let p = if view_pitch > 180.0 {
                view_pitch - 360.0
            } else {
                view_pitch
            };
            p * 0.6
        };
        swing_angles(
            dest,
            0.0,
            45.0,
            0.15,
            frametime_ms as f32,
            &mut self.angle,
            &mut self.swinging,
        );
    }
}

/// `BG_SwingAngles` (`game.mp.i386.so` 0x2ae00, cgame 0x30003ec0): Q3's
/// `CG_SwingAngles` with the step scale `max(|swing| * 0.05, 0.5)`.
fn swing_angles(
    dest: f32,
    tolerance: f32,
    clamp: f32,
    speed: f32,
    frametime_ms: f32,
    angle: &mut f32,
    swinging: &mut bool,
) {
    use crate::pmove::aim::{angle_normalize_360 as angle_mod, angle_subtract};
    if !*swinging {
        let swing = angle_subtract(*angle, dest);
        if swing > tolerance || swing < -tolerance {
            *swinging = true;
        }
    }
    if *swinging {
        let swing = angle_subtract(dest, *angle);
        let scale = (swing.abs() * 0.05).max(0.5);
        if swing >= 0.0 {
            let mut step = frametime_ms * scale * speed;
            if step >= swing {
                step = swing;
                *swinging = false;
            }
            *angle = angle_mod(*angle + step);
        } else {
            let mut step = -frametime_ms * scale * speed;
            if step <= swing {
                step = swing;
                *swinging = false;
            }
            *angle = angle_mod(*angle + step);
        }
    }
    let swing = angle_subtract(dest, *angle);
    if swing > clamp {
        *angle = angle_mod(dest - (clamp - 1.0));
    } else if swing < -clamp {
        *angle = angle_mod(dest + (clamp - 1.0));
    }
}

/// The pitch half of `BG_Player_DoControllers` (`game.mp.i386.so` 0x2b7f8):
/// what each control bone is bent by, engine degrees (down positive).
/// `docs/research/cod11-combat.md` 16.3.
#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct AimPitch {
    pub pelvis: f32,
    pub back_low: f32,
    pub back_mid: f32,
    pub back_up: f32,
    pub neck: f32,
    pub head: f32,
}

impl AimPitch {
    /// `view_pitch` the record's `viewangles[0]`, `swing` its eased torso
    /// pitch, `torso`/`waist` the entity's `fTorsoPitch`/`fWaistPitch`.
    /// A mounted body runs no controllers at all; the caller skips it.
    pub fn new(view_pitch: f32, swing: &PitchSwing, prone: bool, torso: f32, waist: f32) -> Self {
        use crate::pmove::aim::{angle_normalize_180, angle_subtract};
        let mut p = swing.angle;
        if prone {
            p = angle_normalize_180(p);
            p *= if p > 0.0 { 0.5 } else { 0.25 };
        }
        // `AnglesSubtract(view, torso)`, then `AnglesSubtract(torso, legs)`
        // with the legs' pitch 0.
        let head = angle_subtract(view_pitch, p);
        let p = angle_subtract(p, 0.0);
        let slope = if torso != 0.0 || waist != 0.0 {
            angle_subtract(torso, waist)
        } else {
            0.0
        };
        let (back_low, back_mid, back_up) = if prone {
            (slope, 0.0, p)
        } else {
            (p * 0.2 + slope, p * 0.3, p * 0.5)
        };
        AimPitch {
            pelvis: -slope,
            back_low,
            back_mid,
            back_up,
            neck: head * 0.3,
            head: head * 0.7,
        }
    }
}

/// Bends the control bones: the pitch [`AimPitch`] gives each, and `lean`
/// (a fraction, positive presumed right) as a roll on the pelvis and back.
/// Call after the clips and before `skin_matrices`.
///
/// A control bone's rotation replaces its local one and turns it in model
/// space: its world rotation is the control's times its parent's
/// (`docs/research/cod11-combat.md` 16.3). Pitch is about the model's Y,
/// down positive, roll about its X. The bones are walked parent first, so
/// each is measured off the bends above it.
///
/// `set_local_rot` overwrites, so nothing accumulates across frames. The
/// lean split is not retail's, which is not decoded. Missing bones are
/// skipped.
pub fn apply_aim(pose: &mut PoseBuffer, skel: &Skeleton, aim: &AimPitch, lean: f32) {
    let mut bend = |name: &str, pitch_deg: f32, roll_deg: f32| {
        let Some(bi) = skel.bone_index(name) else {
            return;
        };
        let parent = match usize::try_from(skel.bones()[bi].parent) {
            Ok(p) => pose.bone_world(skel, p).1,
            Err(_) => Quat::IDENTITY,
        };
        let control = Quat::from_rotation_y(pitch_deg.to_radians())
            * Quat::from_rotation_x(roll_deg.to_radians());
        pose.set_local_rot(bi, parent.inverse() * control * parent);
    };
    bend("pelvis", aim.pelvis, lean * PELVIS_LEAN_DEG);
    bend("back_low", aim.back_low, lean * BACK_LEAN_DEG / 3.0);
    bend("back_mid", aim.back_mid, lean * BACK_LEAN_DEG / 3.0);
    bend("back_up", aim.back_up, lean * BACK_LEAN_DEG / 3.0);
    bend("neck", aim.neck, 0.0);
    bend("head", aim.head, 0.0);
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
    /// Engine degrees, down positive: what an MG42 aim group descends by.
    pub group_pitch: f32,
    /// The controllers' pitch, `None` for a mounted body, which runs none.
    pub aim: Option<AimPitch>,
    /// A fraction of full lean.
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
        let Some(name) = clip_name(inputs.anims, wire, inputs.group_pitch, 0.0) else {
            continue;
        };
        let Some(anim) = clip(name) else { continue };
        let t = inputs.now_ms.wrapping_sub(start_ms).max(0) as f32 / 1000.0;
        let binding = skel.bind(&anim);
        pose.apply(&anim, &binding, anim.frame_pos(t, anim.looping));
    }
    if let Some(aim) = &inputs.aim {
        apply_aim(&mut pose, skel, aim, inputs.lean);
    }
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
        // A settled swing: the torso takes 0.6 of a 50-degree view.
        let swing = PitchSwing {
            angle: 30.0,
            swinging: false,
        };
        apply_aim(
            &mut pose,
            &skel,
            &AimPitch::new(50.0, &swing, false, 0.0, 0.0),
            0.0,
        );
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

        let aim = AimPitch::new(20.0, &PitchSwing::default(), false, 5.0, 0.0);
        apply_aim(&mut pose, &skel, &aim, 0.0);
        let first = world_rot_of(&pose, &skel, up);
        apply_aim(&mut pose, &skel, &aim, 0.0);
        let second = world_rot_of(&pose, &skel, up);

        assert!(
            first.abs_diff_eq(second, 1e-5),
            "aim pose drifted across identical frames: {first} != {second}"
        );
    }

    /// `BG_SwingAngles` on the pitch channel at the server's 50 ms frame:
    /// the first step covers `51 * 0.05 * 50 * 0.15` of a flip to 85, and
    /// the swing lands on `0.6 * 85` seven frames later (combat doc 16.3).
    #[test]
    fn the_torso_pitch_eases_after_the_view() {
        let mut s = PitchSwing::default();
        s.step(85.0, 50, false);
        assert!((s.angle - 19.125).abs() < 0.01, "{s:?}");
        assert!(s.swinging);
        let mut frames = 1;
        while s.swinging {
            s.step(85.0, 50, false);
            frames += 1;
        }
        assert_eq!(frames, 7);
        assert!((s.angle - 51.0).abs() < 0.01, "{s:?}");
        // Back up: the angle folds through 360 on the way to 0.
        s.step(0.0, 50, false);
        assert!((s.angle - 31.875).abs() < 0.01, "{s:?}");
        // A dead, mounted or climbing body heads for level whatever its view.
        let mut r = PitchSwing::default();
        r.step(-60.0, 50, true);
        assert_eq!(r, PitchSwing::default());
    }

    /// A short frame cannot leave the torso more than 45 degrees off its
    /// destination: the clamp puts it 44 inside.
    #[test]
    fn the_torso_pitch_is_clamped_to_45_off_the_view() {
        let mut s = PitchSwing::default();
        s.step(85.0, 1, false);
        assert!((s.angle - 7.0).abs() < 0.01, "{s:?}");
        let mut up = PitchSwing::default();
        up.step(275.0, 1, false); // -85 as 0..360
        assert!((up.angle - (360.0 - 7.0)).abs() < 0.01, "{up:?}");
    }

    /// `BG_Player_DoControllers`' pitch split: the back takes the eased
    /// torso pitch 0.2 / 0.3 / 0.5 and the neck and head 0.3 / 0.7 of what
    /// the view is past it; prone puts the halved (down) or quartered (up)
    /// torso pitch on `back_up` alone.
    #[test]
    fn the_controllers_split_the_view_between_back_and_head() {
        let swing = PitchSwing {
            angle: 30.0,
            swinging: false,
        };
        let a = AimPitch::new(50.0, &swing, false, 0.0, 0.0);
        let near = |x: f32, y: f32| (x - y).abs() < 0.01;
        assert!(near(a.back_low, 6.0) && near(a.back_mid, 9.0) && near(a.back_up, 15.0));
        assert!(near(a.neck, 6.0) && near(a.head, 14.0), "{a:?}");
        let pr = AimPitch::new(50.0, &swing, true, 4.0, 1.0);
        assert!(near(pr.back_up, 15.0) && near(pr.back_mid, 0.0));
        assert!(near(pr.back_low, 3.0) && near(pr.pelvis, -3.0));
        assert!(near(pr.head, 35.0 * 0.7), "{pr:?}");
        let up = PitchSwing {
            angle: 340.0,
            swinging: false,
        };
        assert!(near(AimPitch::new(0.0, &up, true, 0.0, 0.0).back_up, -5.0));
    }
}
