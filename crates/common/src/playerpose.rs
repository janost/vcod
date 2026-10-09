//! Posing a player: the clip a wire `legsAnim`/`torsoAnim` value names, and
//! the aim layer over it. Shared by the client's draw path and the server's
//! locational trace. `docs/research/player-model-anim-system.md`.

use crate::animtree::{AnimTree, PlayerAnims};
use crate::net::flags::{EF_CROUCH, EF_DEAD, EF_FIRING, EF_MOUNTED, EF_PRONE};
use crate::pmove::aim::{
    angle_normalize_180, angle_normalize_360 as angle_mod, angle_subtract, lean_fraction,
};
use crate::skeleton::{PoseBuffer, Skeleton};
use crate::xanim::XAnim;
use glam::{Quat, Vec3};
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

/// `bg_swingSpeed`'s default, the speed the torso and legs yaw swing at.
pub const BG_SWING_SPEED: f32 = 0.2;

/// What the angle updater reads off the entity and the client's record:
/// `ClientEndFrame` (0x41273..0x412a6) copies `ps.viewangles`, the entity's
/// `angles2[1]` (`movementDir`) and its `leanf` into the record.
#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct BodyInput {
    /// `ps.viewangles` (the entity's `apos` on the client): engine degrees,
    /// pitch down positive, yaw left positive.
    pub view: [f32; 3],
    /// `ps.movementDir`: the legs' heading off the view, degrees.
    pub movement_dir: f32,
    /// The entity's `eFlags`.
    pub eflags: i32,
    /// The legs anim's record (`BG_ParseCommands`' flags).
    pub legs: crate::animscript::AnimRecord,
}

impl BodyInput {
    fn dead(&self) -> bool {
        self.eflags & EF_DEAD != 0
    }
    fn mounted(&self) -> bool {
        self.eflags & EF_MOUNTED != 0
    }
    fn prone(&self) -> bool {
        self.eflags & EF_PRONE != 0
    }
}

/// `eFlags` 0x200, which `BG_PlayerStateToEntityState` sets off `pm_flags`
/// 0x20, the sight flag (0x2cdb6).
pub const EF_ADS: i32 = 0x200;

/// The record's `movetype` bits for `climbup` and `climbdown`.
const MOVETYPE_CLIMB: u32 = 0x30000;
/// `idle` and `idlecr`.
const MOVETYPE_IDLE: u32 = 0x6;

/// The body's three swings, kept in the client's record across frames:
/// legs yaw and its flag at `+0x37c`/`+0x380`, torso yaw at `+0x3ac`/`+0x3b0`,
/// torso pitch at `+0x3b4`/`+0x3b8`, and the two conditions the updater
/// reads, `movetype` (`+0x410`) and `firing` (`+0x428`).
/// `docs/research/cod11-combat.md` 16.3 and 16.4.
#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct BodyAngles {
    pub legs_yaw: f32,
    pub legs_yawing: bool,
    pub torso_yaw: f32,
    pub torso_yawing: bool,
    /// Degrees, engine convention (down positive), folded to 0..360.
    pub pitch: f32,
    pub pitching: bool,
    pub movetype: u32,
    pub firing: bool,
}

impl BodyAngles {
    /// The condition half of `BG_PlayerAnimation` (`game.mp.i386.so`
    /// 0x2b328): `movetype` from the legs anim's record when it carries any
    /// bit, `firing` from `eFlags` 0x400. Retail runs it after
    /// [`Self::step`], so the step reads the previous frame's; a server also
    /// writes `movetype` from pmove ahead of the step.
    pub fn update_conditions(&mut self, input: &BodyInput) {
        if input.legs.movetypes != 0 {
            self.movetype = input.legs.movetypes;
        }
        self.firing = input.eflags & EF_FIRING != 0;
    }

    /// The angle updater (`game.mp.i386.so` 0x2af78, cgame 0x300040e0) for
    /// one frame of `frametime_ms`: the torso yaw toward the view (less 0.3
    /// of the legs' heading off it), the legs toward their heading once 40
    /// degrees off it, the torso pitch toward 0.6 of the view's.
    pub fn step(&mut self, input: &BodyInput, frametime_ms: i32, swing_speed: f32) {
        let ft = frametime_ms as f32;
        let m = input.movement_dir;
        let yaw = angle_mod(input.view[1]);
        let climbing = self.movetype & MOVETYPE_CLIMB != 0;
        if !input.mounted() && !climbing && self.movetype & MOVETYPE_IDLE != 0 {
            if self.firing {
                self.torso_yawing = true;
                self.pitching = true;
            }
        } else {
            self.torso_yawing = true;
            self.pitching = true;
            self.legs_yawing = true;
        }

        let (torso_dest, torso_clamp) = if input.dead() {
            (yaw, 90.0)
        } else if climbing {
            (yaw + m, 0.0)
        } else if input.prone() {
            (yaw, 90.0)
        } else if input.eflags & EF_FIRING != 0 {
            (yaw, 45.0)
        } else if input.eflags & EF_ADS != 0 {
            (yaw, 90.0)
        } else {
            (yaw + m * 0.3, 90.0)
        };
        swing_angles(
            torso_dest,
            0.0,
            torso_clamp,
            swing_speed,
            ft,
            &mut self.torso_yaw,
            &mut self.torso_yawing,
        );

        let legs = |dest: f32, tolerance: f32, a: &mut Self| {
            swing_angles(
                dest,
                tolerance,
                150.0,
                swing_speed,
                ft,
                &mut a.legs_yaw,
                &mut a.legs_yawing,
            );
        };
        if input.dead() {
            legs(yaw, 0.0, self);
        } else if input.prone() {
            self.legs_yawing = false;
            self.legs_yaw = m + yaw;
        } else if input.legs.strafe {
            self.legs_yawing = false;
            legs(yaw, 0.0, self);
        } else if self.legs_yawing {
            legs(yaw + m, 0.0, self);
        } else {
            legs(yaw + m, 40.0, self);
        }
        if input.mounted() {
            self.torso_yaw = yaw;
            self.legs_yaw = yaw;
        } else if climbing {
            self.torso_yaw = yaw + m;
            self.legs_yaw = yaw + m;
        }

        let pitch_dest = if input.dead() || input.mounted() || climbing {
            0.0
        } else {
            let p = input.view[0];
            (if p > 180.0 { p - 360.0 } else { p }) * 0.6
        };
        swing_angles(
            pitch_dest,
            0.0,
            45.0,
            0.15,
            ft,
            &mut self.pitch,
            &mut self.pitching,
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

/// What the entity adds to [`BodyInput`] for the controllers: the lean
/// (`leanf`, -1..1, right positive) and the prone slope terms
/// `fTorsoHeight`, `fTorsoPitch`, `fWaistPitch`.
#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct BodySlope {
    pub lean: f32,
    pub torso_height: f32,
    pub torso_pitch: f32,
    pub waist_pitch: f32,
}

/// `BG_Player_DoControllers` (`game.mp.i386.so` 0x2b7f8): the local tag on
/// `tag_origin` and the control angles of the spine, each `[pitch, yaw,
/// roll]` in engine degrees. `docs/research/cod11-combat.md` 16.3 and 16.4.
#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct Controllers {
    /// `tag_origin`'s local translation and angles.
    pub origin_trans: Vec3,
    pub origin: [f32; 3],
    pub pelvis: [f32; 3],
    pub back_low: [f32; 3],
    pub back_mid: [f32; 3],
    pub back_up: [f32; 3],
    pub neck: [f32; 3],
    pub head: [f32; 3],
}

impl Controllers {
    /// `None` for a mounted body, which runs none (0x2b804).
    pub fn new(a: &BodyAngles, input: &BodyInput, slope: &BodySlope) -> Option<Self> {
        if input.mounted() {
            return None;
        }
        let prone = input.prone();
        let crouch = input.eflags & EF_CROUCH != 0;
        let mut t = [0.0, a.torso_yaw, 0.0];
        if a.movetype & MOVETYPE_CLIMB == 0 {
            t[0] = a.pitch;
            if prone {
                let p = angle_normalize_180(t[0]);
                t[0] = p * if p > 0.0 { 0.5 } else { 0.25 };
            }
        }
        let mut h = [0usize, 1, 2].map(|i| angle_subtract(input.view[i], t[i]));
        let mut l = [0.0, a.legs_yaw, 0.0];
        t = [0usize, 1, 2].map(|i| angle_subtract(t[i], l[i]));

        let mut trans = Vec3::new(0.0, 0.0, slope.torso_height);
        let f = lean_fraction(slope.lean);
        if f == 0.0 {
            t[2] = 0.0;
            h[2] = 0.0;
        } else {
            let crouch_scale = if f > 0.0 { 1.5 } else { 1.8 };
            let roll = f * 50.0 * 0.925;
            t[2] = if crouch { roll * crouch_scale } else { roll };
            trans.y = -f
                * if prone {
                    2.5
                } else if crouch {
                    1.5
                } else {
                    1.0
                };
            h[2] = if crouch {
                roll * crouch_scale
            } else if prone {
                roll * 0.5
            } else {
                roll
            };
        }
        if !input.dead() {
            l[1] = angle_subtract(l[1], input.view[1]);
        }
        if prone {
            l[0] += slope.torso_pitch;
        } else {
            l[2] += f * 50.0 * 0.075;
        }
        let sloped = slope.torso_pitch != 0.0 || slope.waist_pitch != 0.0;
        let bend = if sloped {
            angle_subtract(slope.torso_pitch, slope.waist_pitch)
        } else {
            0.0
        };
        let (back_low, back_mid, back_up) = if prone {
            (
                [bend, t[2] * -1.2, t[2] * 0.3],
                [0.0, t[1] * 0.1 - t[2] * 0.2, t[2] * 0.2],
                [t[0], t[1] * 0.8 + t[2], t[2] * -0.2],
            )
        } else {
            (
                [t[0] * 0.2 + bend, t[1] * 0.4, t[2] * 0.5],
                [t[0] * 0.3, t[1] * 0.4, t[2] * 0.5],
                [t[0] * 0.5, t[1] * 0.2, t[2] * -0.6],
            )
        };
        let pelvis = if sloped {
            angle_subtract(slope.waist_pitch, slope.torso_pitch)
        } else {
            0.0
        };
        Some(Controllers {
            origin_trans: trans,
            origin: l,
            pelvis: [pelvis, 0.0, 0.0],
            back_low,
            back_mid,
            back_up,
            neck: [h[0] * 0.3, h[1] * 0.3, 0.0],
            head: [h[0] * 0.7, h[1] * 0.7, h[2] * -0.3],
        })
    }
}

/// `YawToQuaternion` ⊗ `PitchToQuaternion` ⊗ `RollToQuaternion`, the order
/// `G_DObjSetControlTagAngles` and `G_DObjSetLocalTag` compose them in
/// (0x6721c, 0x67128): pitch down positive about Y, yaw left positive about
/// Z, roll right-side-down positive about X.
fn control_quat([p, y, r]: [f32; 3]) -> Quat {
    Quat::from_rotation_z(y.to_radians())
        * Quat::from_rotation_y(p.to_radians())
        * Quat::from_rotation_x(r.to_radians())
}

/// Poses [`Controllers`] onto `pose`. Call after the clips and before
/// `skin_matrices`.
///
/// `tag_origin`'s local tag replaces its local rotation and translation. A
/// control bone's rotation replaces its local one and turns it in model
/// space: its world rotation is the control's times its parent's
/// (`docs/research/cod11-combat.md` 16.3). The bones are walked parent
/// first, so each is measured off the bends above it, and `set_local_rot`
/// overwrites, so nothing accumulates across frames. Missing bones are
/// skipped.
pub fn apply_controllers(pose: &mut PoseBuffer, skel: &Skeleton, c: &Controllers) {
    if let Some(bi) = skel.bone_index("tag_origin") {
        pose.set_local(bi, c.origin_trans, control_quat(c.origin));
    }
    for (name, angles) in [
        ("pelvis", c.pelvis),
        ("back_low", c.back_low),
        ("back_mid", c.back_mid),
        ("back_up", c.back_up),
        ("neck", c.neck),
        ("head", c.head),
    ] {
        let Some(bi) = skel.bone_index(name) else {
            continue;
        };
        let parent = match usize::try_from(skel.bones()[bi].parent) {
            Ok(p) => pose.bone_world(skel, p).1,
            Err(_) => Quat::IDENTITY,
        };
        pose.set_local_rot(bi, parent.inverse() * control_quat(angles) * parent);
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
/// started, and the controllers the spine layer bends by.
pub struct PoseInputs<'a> {
    pub anims: &'a PlayerAnims,
    /// Wire `legsAnim` / `torsoAnim`, restart toggle included.
    pub legs: i32,
    pub torso: i32,
    /// serverTime each channel last (re)started, and the time to pose at.
    pub legs_start_ms: i32,
    pub torso_start_ms: i32,
    pub now_ms: i32,
    /// Engine degrees, down positive: what an MG42 aim group descends by
    /// when no placement blend is given.
    pub group_pitch: f32,
    /// A gunner's legs: the turret anim's leaves and the weights the body
    /// placement gave them (`turretpose::GunnerPlacement::leaves`), in
    /// place of the legs clip.
    pub turret_leaves: Option<&'a [(usize, f32)]>,
    /// `None` for a mounted or dead body.
    pub controllers: Option<Controllers>,
}

/// Legs then torso on one pose buffer, then the controllers: `pl_*` clips
/// key the whole body and `pt_*` only the bones they name, which is what
/// makes the split work (player-model-anim-system.md, "Legs/torso split").
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
    let tree = &inputs.anims.tree;
    for (legs, wire, start_ms) in [
        (true, inputs.legs, inputs.legs_start_ms),
        (false, inputs.torso, inputs.torso_start_ms),
    ] {
        let t = inputs.now_ms.wrapping_sub(start_ms).max(0) as f32 / 1000.0;
        if legs && let Some(leaves) = inputs.turret_leaves {
            // Each leaf lerps in by its share of the weight so far, which
            // leaves the buffer at the weighted mean.
            let mut total = 0.0;
            for &(leaf, w) in leaves {
                let Some(anim) = tree.nodes.get(leaf).and_then(|n| clip(&n.name)) else {
                    continue;
                };
                if w <= 0.0 {
                    continue;
                }
                total += w;
                let binding = skel.bind(&anim);
                pose.apply_weighted(&anim, &binding, anim.frame_pos(t, anim.looping), w / total);
            }
            continue;
        }
        let Some(name) = clip_name(inputs.anims, wire, inputs.group_pitch, 0.0) else {
            continue;
        };
        let Some(anim) = clip(name) else { continue };
        let binding = skel.bind(&anim);
        pose.apply(&anim, &binding, anim.frame_pos(t, anim.looping));
    }
    if let Some(c) = &inputs.controllers {
        apply_controllers(&mut pose, skel, c);
    }
    pose
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skeleton::PoseBuffer;
    use crate::xmodel::{Bone, XModel};

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

    fn near(x: f32, y: f32) -> bool {
        (x - y).abs() < 0.01
    }

    fn idle() -> crate::animscript::AnimRecord {
        crate::animscript::AnimRecord {
            movetypes: 1 << 1,
            strafe: false,
        }
    }

    /// A standing idle body: the record reads `idle`, the view as given.
    fn standing(view: [f32; 3]) -> (BodyAngles, BodyInput) {
        let input = BodyInput {
            view,
            legs: idle(),
            ..Default::default()
        };
        let mut a = BodyAngles::default();
        a.update_conditions(&input);
        (a, input)
    }

    #[test]
    fn controllers_bend_the_spine_in_model_space() {
        let m = spine_fixture();
        let skel = Skeleton::build(&[&m]);
        let mut pose = PoseBuffer::new(&skel);
        // A settled swing: the torso takes 0.6 of a 50-degree view.
        let a = BodyAngles {
            pitch: 30.0,
            ..Default::default()
        };
        let input = BodyInput {
            view: [50.0, 0.0, 0.0],
            ..Default::default()
        };
        let c = Controllers::new(&a, &input, &BodySlope::default()).unwrap();
        apply_controllers(&mut pose, &skel, &c);
        // back_up accumulates all three weights, the full torso pitch, and
        // positive pitch tips the spine's top toward +X, nose down.
        let up = skel.bone_index("back_up").unwrap();
        let v = world_rot_of(&pose, &skel, up) * Vec3::Z;
        assert!(near(v.angle_between(Vec3::Z).to_degrees(), 30.0), "{v}");
        assert!(v.x > 0.0, "{v}");
        // Again: `set_local_rot` overwrites, so nothing accumulates.
        let first = world_rot_of(&pose, &skel, up);
        apply_controllers(&mut pose, &skel, &c);
        assert!(first.abs_diff_eq(world_rot_of(&pose, &skel, up), 1e-5));
    }

    /// `tag_origin`'s local tag turns the whole body by the legs' yaw off
    /// the view and shifts it by the lean; the spine's yaw controllers turn
    /// it back by the torso's, so `back_up` ends at the torso's yaw.
    #[test]
    fn the_local_tag_turns_the_body_to_the_legs_and_the_spine_back() {
        let m = spine_fixture();
        let skel = Skeleton::build(&[&m]);
        let mut pose = PoseBuffer::new(&skel);
        let a = BodyAngles {
            legs_yaw: 30.0,
            torso_yaw: 10.0,
            ..Default::default()
        };
        let input = BodyInput::default();
        let c = Controllers::new(&a, &input, &BodySlope::default()).unwrap();
        apply_controllers(&mut pose, &skel, &c);
        let yaw_of = |name: &str| {
            let f = world_rot_of(&pose, &skel, skel.bone_index(name).unwrap()) * Vec3::X;
            f.y.atan2(f.x).to_degrees()
        };
        assert!(near(yaw_of("tag_origin"), 30.0), "{}", yaw_of("tag_origin"));
        assert!(near(yaw_of("back_low"), 30.0 - 8.0));
        assert!(near(yaw_of("back_mid"), 30.0 - 16.0));
        assert!(near(yaw_of("back_up"), 10.0), "{}", yaw_of("back_up"));
    }

    /// `BG_SwingAngles` on the pitch channel at the server's 50 ms frame:
    /// the first step covers `51 * 0.05 * 50 * 0.15` of a flip to 85, and
    /// the swing lands on `0.6 * 85` seven frames later (combat doc 16.3).
    #[test]
    fn the_torso_pitch_eases_after_the_view() {
        let (mut s, input) = standing([85.0, 0.0, 0.0]);
        s.step(&input, 50, BG_SWING_SPEED);
        assert!(near(s.pitch, 19.125), "{s:?}");
        assert!(s.pitching);
        let mut frames = 1;
        while s.pitching {
            s.step(&input, 50, BG_SWING_SPEED);
            frames += 1;
        }
        assert_eq!(frames, 7);
        assert!(near(s.pitch, 51.0), "{s:?}");
        // Back up: the angle folds through 360 on the way to 0.
        s.step(
            &BodyInput {
                view: [0.0; 3],
                ..input
            },
            50,
            BG_SWING_SPEED,
        );
        assert!(near(s.pitch, 31.875), "{s:?}");
        // A dead body heads for level whatever its view.
        let mut r = BodyAngles::default();
        let dead = BodyInput {
            view: [-60.0, 0.0, 0.0],
            eflags: EF_DEAD,
            ..Default::default()
        };
        r.step(&dead, 50, BG_SWING_SPEED);
        assert_eq!(r.pitch, 0.0);
    }

    /// A short frame cannot leave the torso more than 45 degrees off its
    /// destination: the clamp puts it 44 inside.
    #[test]
    fn the_torso_pitch_is_clamped_to_45_off_the_view() {
        let (mut s, input) = standing([85.0, 0.0, 0.0]);
        s.step(&input, 1, BG_SWING_SPEED);
        assert!(near(s.pitch, 7.0), "{s:?}");
        let (mut up, input) = standing([275.0, 0.0, 0.0]); // -85 as 0..360
        up.step(&input, 1, BG_SWING_SPEED);
        assert!(near(up.pitch, 360.0 - 7.0), "{up:?}");
    }

    /// The torso yaw always swings, at `bg_swingSpeed` 0.2: a 50 ms step is
    /// `max(|d| * 0.5, 5)`. The idle legs hold until the view is 40 off them.
    #[test]
    fn the_torso_turns_with_the_view_and_the_idle_legs_wait_for_40() {
        let (mut s, input) = standing([0.0, 30.0, 0.0]);
        s.step(&input, 50, BG_SWING_SPEED);
        assert!(near(s.torso_yaw, 15.0), "{s:?}");
        assert_eq!(s.legs_yaw, 0.0, "30 off is inside the legs' 40");
        s.step(&input, 50, BG_SWING_SPEED);
        assert!(near(s.torso_yaw, 22.5), "{s:?}");

        let (mut s, input) = standing([0.0, 50.0, 0.0]);
        s.step(&input, 50, BG_SWING_SPEED);
        assert!(near(s.legs_yaw, 25.0), "{s:?}");
        assert!(s.legs_yawing);
        // Once swinging they keep going until they land.
        s.step(
            &BodyInput {
                view: [0.0, 30.0, 0.0],
                ..input
            },
            50,
            BG_SWING_SPEED,
        );
        assert!(near(s.legs_yaw, 30.0), "{s:?}");
        assert!(!s.legs_yawing);
    }

    /// A moving body's legs always follow, toward the view plus
    /// `movementDir`, and its torso toward the view plus 0.3 of it; a strafe
    /// anim's legs face the view; prone legs snap to their heading; a
    /// mounted body snaps everything to the view.
    #[test]
    fn the_legs_follow_their_heading_by_movetype() {
        let run = BodyInput {
            view: [0.0, 0.0, 0.0],
            movement_dir: 20.0,
            legs: crate::animscript::AnimRecord {
                movetypes: 1 << 10,
                strafe: false,
            },
            ..Default::default()
        };
        let mut s = BodyAngles::default();
        s.update_conditions(&run);
        s.step(&run, 50, BG_SWING_SPEED);
        assert!(near(s.legs_yaw, 10.0), "{s:?}");
        assert!(near(s.torso_yaw, 5.0), "{s:?}");

        let strafe = BodyInput {
            movement_dir: 90.0,
            legs: crate::animscript::AnimRecord {
                movetypes: 1 << 10,
                strafe: true,
            },
            ..run
        };
        let mut s = BodyAngles::default();
        s.update_conditions(&strafe);
        s.step(&strafe, 50, BG_SWING_SPEED);
        assert_eq!(s.legs_yaw, 0.0);

        let prone = BodyInput {
            eflags: EF_PRONE,
            movement_dir: 60.0,
            ..run
        };
        let mut s = BodyAngles::default();
        s.step(&prone, 50, BG_SWING_SPEED);
        assert_eq!(s.legs_yaw, 60.0);

        let mounted = BodyInput {
            view: [30.0, 200.0, 0.0],
            eflags: EF_MOUNTED,
            ..run
        };
        let mut s = BodyAngles::default();
        s.step(&mounted, 50, BG_SWING_SPEED);
        assert!(near(s.legs_yaw, 200.0) && near(s.torso_yaw, 200.0));
        assert_eq!(s.pitch, 0.0);
        assert!(Controllers::new(&s, &mounted, &BodySlope::default()).is_none());
    }

    /// The split: the back takes the torso pitch 0.2 / 0.3 / 0.5 and its yaw
    /// off the legs 0.4 / 0.4 / 0.2, the neck and head 0.3 / 0.7 of the view
    /// past the torso, and `tag_origin` the legs' yaw off the view, so the
    /// yaws sum to nothing and the head faces the view.
    #[test]
    fn the_controllers_split_pitch_and_yaw_between_back_and_head() {
        let a = BodyAngles {
            pitch: 30.0,
            torso_yaw: 20.0,
            legs_yaw: 350.0,
            ..Default::default()
        };
        let input = BodyInput {
            view: [50.0, 40.0, 0.0],
            ..Default::default()
        };
        let c = Controllers::new(&a, &input, &BodySlope::default()).unwrap();
        assert!(near(c.back_low[0], 6.0) && near(c.back_mid[0], 9.0) && near(c.back_up[0], 15.0));
        assert!(near(c.neck[0], 6.0) && near(c.head[0], 14.0), "{c:?}");
        assert!(near(c.back_low[1], 12.0) && near(c.back_mid[1], 12.0));
        assert!(near(c.back_up[1], 6.0) && near(c.neck[1], 6.0) && near(c.head[1], 14.0));
        assert!(near(c.origin[1], -50.0), "{c:?}");
        let sum =
            c.origin[1] + c.back_low[1] + c.back_mid[1] + c.back_up[1] + c.neck[1] + c.head[1];
        assert!(near(sum, 0.0), "{sum}");
        assert_eq!(c.pelvis, [0.0; 3]);
        assert_eq!(c.origin_trans, Vec3::ZERO);
    }

    /// The lean through `GetLeanFraction`: 46.25 degrees of roll at full
    /// lean, split 0.5 / 0.5 / -0.6 down the back with the head countering
    /// 0.3, 3.75 on `tag_origin` and a one-unit shift; crouched it is 1.5
    /// times (1.8 to the left) and the shift 1.5, prone the shift 2.5.
    #[test]
    fn the_lean_rolls_the_back_and_shifts_the_body() {
        let a = BodyAngles::default();
        let slope = BodySlope {
            lean: 0.5,
            ..Default::default()
        };
        let f = 0.75; // (2 - 0.5) * 0.5
        let roll = f * 46.25;
        let stand = Controllers::new(&a, &BodyInput::default(), &slope).unwrap();
        assert!(near(stand.back_low[2], roll * 0.5) && near(stand.back_mid[2], roll * 0.5));
        assert!(near(stand.back_up[2], roll * -0.6) && near(stand.head[2], roll * -0.3));
        assert!(near(stand.origin[2], f * 3.75) && near(stand.origin_trans.y, -f));

        let crouch = BodyInput {
            eflags: EF_CROUCH,
            ..Default::default()
        };
        let c = Controllers::new(&a, &crouch, &slope).unwrap();
        assert!(near(c.back_low[2], roll * 1.5 * 0.5) && near(c.origin_trans.y, -f * 1.5));
        let left = BodySlope {
            lean: -0.5,
            ..Default::default()
        };
        let c = Controllers::new(&a, &crouch, &left).unwrap();
        assert!(near(c.head[2], roll * 1.8 * 0.3), "{c:?}");

        let prone = BodyInput {
            eflags: EF_PRONE,
            ..Default::default()
        };
        let c = Controllers::new(&a, &prone, &slope).unwrap();
        assert!(near(c.origin_trans.y, -f * 2.5) && near(c.origin[2], 0.0));
        assert!(near(c.back_low[1], roll * -1.2) && near(c.back_up[1], roll));
        assert!(near(c.head[2], roll * 0.5 * -0.3));
    }

    /// Prone halves (down) or quarters (up) the torso pitch onto `back_up`
    /// alone; the slope bends `back_low` and `pelvis` against each other
    /// and tips `tag_origin` by the torso's pitch, raised by its height.
    #[test]
    fn prone_puts_the_pitch_on_back_up_and_the_slope_on_the_pelvis() {
        let a = BodyAngles {
            pitch: 30.0,
            ..Default::default()
        };
        let prone = BodyInput {
            view: [50.0, 0.0, 0.0],
            eflags: EF_PRONE,
            ..Default::default()
        };
        let slope = BodySlope {
            torso_height: 3.0,
            torso_pitch: 4.0,
            waist_pitch: 1.0,
            ..Default::default()
        };
        let c = Controllers::new(&a, &prone, &slope).unwrap();
        assert!(near(c.back_up[0], 15.0) && near(c.back_mid[0], 0.0));
        assert!(near(c.back_low[0], 3.0) && near(c.pelvis[0], -3.0));
        assert!(near(c.head[0], 35.0 * 0.7), "{c:?}");
        assert!(near(c.origin[0], 4.0) && near(c.origin_trans.z, 3.0));
        let up = BodyAngles {
            pitch: 340.0,
            ..Default::default()
        };
        let c = Controllers::new(&up, &prone, &BodySlope::default()).unwrap();
        assert!(near(c.back_up[0], -5.0));
    }
}
