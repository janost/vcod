//! Merges several xmodel bone hierarchies (hands and gun, body and
//! attachments) into one skeleton and evaluates anim poses into skin matrices.

use crate::xanim::XAnim;
use crate::xmodel::XModel;
use glam::{Mat4, Quat, Vec3};
use std::collections::HashSet;
use std::sync::{Mutex, OnceLock};

/// Graft warnings already printed, keyed `<kind>:<lod>:<bone>`, so a shared
/// attachment warns once per process instead of once per assembly.
fn warned_grafts() -> &'static Mutex<HashSet<String>> {
    static WARNED: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    WARNED.get_or_init(|| Mutex::new(HashSet::new()))
}

/// Bind locals only; world binds live in the per-model inverse binds.
pub struct SkelBone {
    pub name: String,
    pub parent: i32,
    pub local_pos: Vec3,
    pub local_rot: Quat,
}

/// Model 0 is the base hierarchy; later models graft onto an existing bone
/// (the gun's `tag_weapon` rides the hands' animated `tag_weapon`).
pub struct Skeleton {
    bones: Vec<SkelBone>,
    /// Per model: model bone index -> skeleton bone index.
    maps: Vec<Vec<usize>>,
    /// Per model: inverse of the model's own bind world, so
    /// `world * inv_bind` is identity at bind pose.
    inv_binds: Vec<Vec<Mat4>>,
}

/// Track index -> skeleton bone index; `None` for bones the skeleton lacks.
pub struct AnimBinding(pub(crate) Vec<Option<usize>>);

impl Skeleton {
    /// `build_grafted` with no explicit tags.
    pub fn build(models: &[&XModel]) -> Skeleton {
        Skeleton::build_grafted(&models.iter().map(|m| (*m, None)).collect::<Vec<_>>())
    }

    /// Each later model's root aliases the skeleton bone its tag names, or,
    /// absent a tag, the bone with the same name (the engine default). Its
    /// other bones alias same-named base-rig bones too (the engine shares one
    /// bone tree: a head model's own `bip01 neck`/`bip01 head` must ride the
    /// body's animated copies, not unkeyed duplicates); genuinely new bones
    /// append with remapped parents.
    pub fn build_grafted(models: &[(&XModel, Option<&str>)]) -> Skeleton {
        let mut bones: Vec<SkelBone> = Vec::new();
        let mut maps: Vec<Vec<usize>> = Vec::new();
        let mut inv_binds: Vec<Vec<Mat4>> = Vec::new();
        for (mi, (model, graft_tag)) in models.iter().enumerate() {
            // Merges scope to model 0's bones so two attachments that happen to
            // share a bone name never alias each other.
            let base_len = if mi == 0 { 0 } else { maps[0].len() };
            let mut map = Vec::with_capacity(model.bones.len());
            for (bi, b) in model.bones.iter().enumerate() {
                if mi > 0 && bi > 0 {
                    if let Some(si) = bones[..base_len].iter().position(|s| s.name == b.name) {
                        map.push(si);
                        continue;
                    }
                }
                // The graft point: a later model's root aliases an existing bone.
                if mi > 0 && bi == 0 {
                    let target = graft_tag.unwrap_or(b.name.as_str());
                    let mut matches = bones.iter().enumerate().filter(|(_, s)| s.name == target);
                    if let Some((si, _)) = matches.next() {
                        if matches.next().is_some()
                            && warned_grafts()
                                .lock()
                                .unwrap()
                                .insert(format!("ambiguous:{}:{target}", model.lod))
                        {
                            log::warn!(
                                "{}: graft bone '{target}' matches more than one bone in the base skeleton, using the first",
                                model.lod
                            );
                        }
                        map.push(si);
                        continue;
                    }
                    if warned_grafts()
                        .lock()
                        .unwrap()
                        .insert(format!("missing:{}:{target}", model.lod))
                    {
                        log::warn!(
                            "{}: graft bone '{target}' not in the base skeleton, attaching as a new root",
                            model.lod
                        );
                    }
                }
                let parent = match b.parent {
                    p if p >= 0 => map[p as usize] as i32,
                    // A stray root inside a grafted model hangs off the graft point.
                    _ if mi > 0 && bi > 0 => map[0] as i32,
                    _ => -1,
                };
                bones.push(SkelBone {
                    name: b.name.clone(),
                    parent,
                    local_pos: b.local_pos,
                    local_rot: b.local_rot,
                });
                map.push(bones.len() - 1);
            }
            inv_binds.push(
                model
                    .bones
                    .iter()
                    .map(|b| Mat4::from_rotation_translation(b.rot, b.pos).inverse())
                    .collect(),
            );
            maps.push(map);
        }
        Skeleton {
            bones,
            maps,
            inv_binds,
        }
    }

    pub fn bone_count(&self) -> usize {
        self.bones.len()
    }

    pub fn bone_index(&self, name: &str) -> Option<usize> {
        self.bones.iter().position(|b| b.name == name)
    }

    pub fn bones(&self) -> &[SkelBone] {
        &self.bones
    }

    pub fn bind(&self, anim: &XAnim) -> AnimBinding {
        AnimBinding(
            anim.tracks
                .iter()
                .map(|t| self.bones.iter().position(|b| b.name == t.bone))
                .collect(),
        )
    }
}

/// Per-bone locals, starting at bind pose. `apply` overwrites only keyed
/// bones, so bones outside a clip hold their last pose (the fingers during
/// the `tag_torso`-only ADS clips).
pub struct PoseBuffer {
    locals: Vec<(Vec3, Quat)>,
    /// Translation keys are offsets from these.
    bind_pos: Vec<Vec3>,
}

impl PoseBuffer {
    pub fn new(skel: &Skeleton) -> PoseBuffer {
        PoseBuffer {
            locals: skel
                .bones
                .iter()
                .map(|b| (b.local_pos, b.local_rot))
                .collect(),
            bind_pos: skel.bones.iter().map(|b| b.local_pos).collect(),
        }
    }

    /// Replaces the rotation outright. The aim layer's control bones are
    /// keyed by no clip, so composing onto the previous frame would accumulate.
    pub fn set_local_rot(&mut self, bone: usize, rot: Quat) {
        self.locals[bone].1 = rot;
    }

    /// Rotation keys replace the local rotation; translation keys are offsets
    /// from the bind local, not absolutes. An unkeyed channel keeps its local.
    /// docs/research/xanim-v14-format.md, "Sampling and the translation-key gotcha".
    pub fn apply(&mut self, anim: &XAnim, binding: &AnimBinding, frame_pos: f32) {
        self.apply_weighted(anim, binding, frame_pos, 1.0);
    }

    /// `apply` cross-faded onto the current pose: the clip's keyed channels
    /// lerp from whatever the buffer holds toward the sampled pose. Weight 1
    /// replaces outright; the anim-switch blend ramps it 0 -> 1.
    pub fn apply_weighted(
        &mut self,
        anim: &XAnim,
        binding: &AnimBinding,
        frame_pos: f32,
        weight: f32,
    ) {
        for (track, slot) in anim.tracks.iter().zip(&binding.0) {
            let Some(bi) = *slot else { continue };
            let (p, q) = track.sample(frame_pos);
            if let Some(p) = p {
                let target = self.bind_pos[bi] + p;
                self.locals[bi].0 = self.locals[bi].0.lerp(target, weight);
            }
            if let Some(q) = q {
                self.locals[bi].1 = self.locals[bi].1.slerp(q, weight);
            }
        }
    }

    /// Parents precede children by construction, so one pass composes.
    fn worlds(&self, skel: &Skeleton) -> Vec<(Vec3, Quat)> {
        let mut worlds: Vec<(Vec3, Quat)> = Vec::with_capacity(self.locals.len());
        for (b, &(lp, lr)) in skel.bones.iter().zip(&self.locals) {
            worlds.push(if b.parent >= 0 {
                let (pp, pr) = worlds[b.parent as usize];
                (pp + pr * lp, pr * lr)
            } else {
                (lp, lr)
            });
        }
        worlds
    }

    /// Posed world transform of one bone, before any inverse bind.
    pub fn bone_world(&self, skel: &Skeleton, bone: usize) -> (Vec3, Quat) {
        self.worlds(skel)[bone]
    }

    /// `world(pose) * inverse_bind`, in the model's own bone order.
    pub fn skin_matrices(&self, skel: &Skeleton, model: usize) -> Vec<Mat4> {
        let worlds = self.worlds(skel);
        skel.maps[model]
            .iter()
            .zip(&skel.inv_binds[model])
            .map(|(&si, inv)| {
                let (p, r) = worlds[si];
                Mat4::from_rotation_translation(r, p) * *inv
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::xanim::{Track, XAnim};
    use crate::xmodel::{Bone, XModel};
    use glam::{Mat4, Quat, Vec3};

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

    fn model(bones: Vec<Bone>) -> XModel {
        XModel {
            lod: "t".into(),
            surfaces: vec![],
            materials: vec![],
            bones,
            collision: Vec::new(),
        }
    }

    /// hands-like: a > tag_weapon; gun-like: tag_weapon > c.
    fn two_model_skel() -> (XModel, XModel) {
        let mut hb = vec![bone("a", -1, Vec3::ZERO, Quat::IDENTITY, &[])];
        hb.push(bone(
            "tag_weapon",
            0,
            Vec3::X,
            Quat::from_rotation_z(0.5),
            &hb,
        ));
        let mut gb = vec![bone("tag_weapon", -1, Vec3::ZERO, Quat::IDENTITY, &[])];
        gb.push(bone("c", 0, Vec3::Y, Quat::IDENTITY, &gb));
        (model(hb), model(gb))
    }

    fn one_track_anim(bone: &str, pos: Vec3, rot: Quat) -> XAnim {
        XAnim {
            name: "t".into(),
            frame_count: 1,
            framerate: 30.0,
            looping: false,
            notes: vec![],
            tracks: vec![Track {
                bone: bone.into(),
                rot_keys: vec![(0, rot)],
                trans_keys: vec![(0, pos)],
            }],
        }
    }

    #[test]
    fn grafts_second_model_onto_matching_root() {
        let (h, g) = two_model_skel();
        let skel = Skeleton::build(&[&h, &g]);
        assert_eq!(skel.bone_count(), 3); // a, tag_weapon, c; root aliased
    }

    #[test]
    fn bind_pose_skins_are_identity_for_base_and_attach_for_graft() {
        let (h, g) = two_model_skel();
        let skel = Skeleton::build(&[&h, &g]);
        let pose = PoseBuffer::new(&skel);
        for sm in pose.skin_matrices(&skel, 0) {
            assert!(sm.abs_diff_eq(Mat4::IDENTITY, 1e-5), "{sm}");
        }
        // the grafted model's skins equal the graft bone's bind transform
        let attach = Mat4::from_rotation_translation(Quat::from_rotation_z(0.5), Vec3::X);
        for sm in pose.skin_matrices(&skel, 1) {
            assert!(sm.abs_diff_eq(attach, 1e-5), "{sm}");
        }
    }

    #[test]
    fn anim_on_shared_bone_moves_grafted_children() {
        let (h, g) = two_model_skel();
        let skel = Skeleton::build(&[&h, &g]);
        let anim = one_track_anim("tag_weapon", Vec3::new(0.0, 0.0, 5.0), Quat::IDENTITY);
        let binding = skel.bind(&anim);
        let mut pose = PoseBuffer::new(&skel);
        pose.apply(&anim, &binding, 0.0);
        // tag_weapon: bind X plus offset (0,0,5), rotation replaced by
        // identity, so c's world is (1,1,5)
        let mats = pose.skin_matrices(&skel, 1);
        let p = mats[1].transform_point3(Vec3::Y); // vert at c's bind world
        assert!(p.abs_diff_eq(Vec3::new(1.0, 1.0, 5.0), 1e-4), "{p}");
    }

    #[test]
    fn unkeyed_bones_hold_their_last_pose() {
        let (h, g) = two_model_skel();
        let skel = Skeleton::build(&[&h, &g]);
        let move_c = one_track_anim("c", Vec3::new(0.0, 7.0, 0.0), Quat::IDENTITY);
        let move_tag = one_track_anim("tag_weapon", Vec3::new(0.0, 0.0, 5.0), Quat::IDENTITY);
        let mut pose = PoseBuffer::new(&skel);
        pose.apply(&move_c, &skel.bind(&move_c), 0.0);
        pose.apply(&move_tag, &skel.bind(&move_tag), 0.0);
        // c keeps (0,8,0) from the first apply; tag_weapon moves to (1,0,5)
        let mats = pose.skin_matrices(&skel, 1);
        let p = mats[1].transform_point3(Vec3::Y);
        assert!(p.abs_diff_eq(Vec3::new(1.0, 8.0, 5.0), 1e-4), "{p}");
    }

    #[test]
    fn grafts_at_explicit_tag() {
        // base: a > tag_helmet; the attachment roots at "lid"
        let mut hb = vec![bone("a", -1, Vec3::ZERO, Quat::IDENTITY, &[])];
        hb.push(bone("tag_helmet", 0, Vec3::Z, Quat::IDENTITY, &hb));
        let base = model(hb);
        let gb = vec![bone("lid", -1, Vec3::ZERO, Quat::IDENTITY, &[])];
        let attach = model(gb);
        let skel = Skeleton::build_grafted(&[(&base, None), (&attach, Some("tag_helmet"))]);
        assert_eq!(skel.bone_count(), 2);
        let pose = PoseBuffer::new(&skel);
        let sm = pose.skin_matrices(&skel, 1);
        let p = sm[0].transform_point3(Vec3::ZERO);
        assert!(p.abs_diff_eq(Vec3::Z, 1e-5), "{p}");
    }

    /// head-model shape: the attachment's root and part of its chain duplicate
    /// base bones; only genuinely new bones (jaw) may append, or the duplicates
    /// hold bind pose while the base copy animates (helmet-in-skull).
    #[test]
    fn shared_nonroot_bones_merge_onto_the_base_rig() {
        let mut bb = vec![bone("spine", -1, Vec3::ZERO, Quat::IDENTITY, &[])];
        bb.push(bone("neck", 0, Vec3::Z, Quat::IDENTITY, &bb));
        bb.push(bone("head", 1, Vec3::Z, Quat::IDENTITY, &bb));
        let base = model(bb);
        let mut ab = vec![bone("spine", -1, Vec3::ZERO, Quat::IDENTITY, &[])];
        ab.push(bone("neck", 0, Vec3::Z, Quat::IDENTITY, &ab));
        ab.push(bone("head", 1, Vec3::Z, Quat::IDENTITY, &ab));
        ab.push(bone("jaw", 2, Vec3::X, Quat::IDENTITY, &ab));
        let attach = model(ab);

        let skel = Skeleton::build_grafted(&[(&base, None), (&attach, None)]);
        assert_eq!(skel.bone_count(), 4); // spine, neck, head, jaw

        // a clip keying the shared bone must move the attachment with the base
        let anim = one_track_anim("neck", Vec3::new(5.0, 0.0, 0.0), Quat::IDENTITY);
        let mut pose = PoseBuffer::new(&skel);
        pose.apply(&anim, &skel.bind(&anim), 0.0);
        let sm = pose.skin_matrices(&skel, 1);
        // attachment's head bone (index 2), bind world (0,0,2), offset +5 in x
        let p = sm[2].transform_point3(Vec3::new(0.0, 0.0, 2.0));
        assert!(p.abs_diff_eq(Vec3::new(5.0, 0.0, 2.0), 1e-4), "{p}");
    }

    #[test]
    fn missing_tag_attaches_as_new_root_with_warning() {
        let hb = vec![bone("a", -1, Vec3::ZERO, Quat::IDENTITY, &[])];
        let gb = vec![bone("lid", -1, Vec3::ZERO, Quat::IDENTITY, &[])];
        let (base, attach) = (model(hb), model(gb));
        let skel = Skeleton::build_grafted(&[(&base, None), (&attach, Some("tag_nope"))]);
        assert_eq!(skel.bone_count(), 2); // fell back to appending
    }

    /// Cross-fade support: weight 0 keeps the current pose, 1 equals `apply`,
    /// between lerps - the stance/anim switch blend.
    #[test]
    fn apply_weighted_lerps_between_poses() {
        let (h, g) = two_model_skel();
        let skel = Skeleton::build(&[&h, &g]);
        let from = one_track_anim("tag_weapon", Vec3::new(0.0, 0.0, 2.0), Quat::IDENTITY);
        let to = one_track_anim("tag_weapon", Vec3::new(0.0, 0.0, 6.0), Quat::IDENTITY);
        let mut pose = PoseBuffer::new(&skel);
        pose.apply(&from, &skel.bind(&from), 0.0);
        pose.apply_weighted(&to, &skel.bind(&to), 0.0, 0.5);
        // tag_weapon local z blends 2 -> 6 at half weight: 4 (+ bind X)
        let (p, _) = pose.bone_world(&skel, 1);
        assert!(p.abs_diff_eq(Vec3::new(1.0, 0.0, 4.0), 1e-4), "{p}");

        pose.apply_weighted(&to, &skel.bind(&to), 0.0, 1.0);
        let (p, _) = pose.bone_world(&skel, 1);
        assert!(p.abs_diff_eq(Vec3::new(1.0, 0.0, 6.0), 1e-4), "{p}");
    }

    fn two_bone_chain() -> Skeleton {
        let mut bones = vec![bone("root", -1, Vec3::ZERO, Quat::IDENTITY, &[])];
        bones.push(bone(
            "child",
            0,
            Vec3::new(0.0, 10.0, 0.0),
            Quat::IDENTITY,
            &bones,
        ));
        let m = model(bones);
        Skeleton::build(&[&m])
    }

    #[test]
    fn bone_world_composes_parent_chain() {
        let skel = two_bone_chain();
        let pose = PoseBuffer::new(&skel);
        let (p, _q) = pose.bone_world(&skel, 1);
        assert!((p - Vec3::new(0.0, 10.0, 0.0)).length() < 1e-5);
    }

    #[test]
    fn bone_index_finds_by_name() {
        let (h, g) = two_model_skel();
        let skel = Skeleton::build(&[&h, &g]);
        assert_eq!(skel.bone_index("tag_weapon"), Some(1));
        assert_eq!(skel.bone_index("nope"), None);
    }

    #[test]
    fn real_models_pose_sanely() {
        let Some(fs) = crate::testing::game_fs() else {
            return;
        };
        // The 1.5 paks repackaged the view anims; the pose bind needs one.
        if fs.read("xanim/viewmodel_kar98mp_idle").is_none() {
            return;
        };
        let hands = crate::xmodel::load(&fs, "viewmodel_hands_new").unwrap();
        let gun = crate::xmodel::load(&fs, "viewmodel_kar98k").unwrap();
        let skel = Skeleton::build(&[&hands, &gun]);
        assert_eq!(skel.bone_count(), 60); // 55 hands + 5 gun children (root aliased)

        let idle = crate::xanim::load(&fs, "viewmodel_kar98mp_idle").unwrap();
        let binding = skel.bind(&idle);
        assert_eq!(binding.0.len(), 48);
        assert!(binding.0.iter().all(Option::is_some));
        let mut pose = PoseBuffer::new(&skel);
        pose.apply(&idle, &binding, 0.0);
        for m in 0..2 {
            for sm in pose.skin_matrices(&skel, m) {
                assert!(sm.is_finite());
            }
        }

        let re = crate::xanim::load(&fs, "viewmodel_kar98mp_rechamber").unwrap();
        let rb = skel.bind(&re);
        let at = |fp: f32| {
            let mut p = PoseBuffer::new(&skel);
            p.apply(&re, &rb, fp);
            p.skin_matrices(&skel, 1)
        };
        let (a, b) = (at(0.0), at(12.0));
        let gi = gun
            .bones
            .iter()
            .position(|b| b.name == "kar98_bolt")
            .unwrap();
        assert!(
            !a[gi].abs_diff_eq(b[gi], 1e-3),
            "bolt should move mid-rechamber"
        );

        // bind-pose skins are identity for both models; if this fails the gun
        // grafts at a non-identity tag
        let bind_pose = PoseBuffer::new(&skel);
        for m in 0..2 {
            for sm in bind_pose.skin_matrices(&skel, m) {
                assert!(sm.abs_diff_eq(glam::Mat4::IDENTITY, 1e-3), "{sm}");
            }
        }

        // Pins the simple-key axis: decoded as Z, the idle finger keys equal
        // the bind locals; decoded as X they land 6 to 114 degrees off.
        let mut checked = 0usize;
        for (ti, t) in idle.tracks.iter().enumerate() {
            if !t.bone.contains("finger") || t.rot_keys.is_empty() {
                continue;
            }
            let bind = skel.bones()[binding.0[ti].unwrap()].local_rot;
            let key = t.rot_keys[0].1;
            let d = bind.dot(key).abs();
            assert!(d > 0.999, "{}: idle key {key} != bind {bind}", t.bone);
            checked += 1;
        }
        assert!(checked >= 10, "only {checked} finger tracks checked");

        // every shipped viewmodel rig has zero bind locals, so bind + key == key here
        for m in [&hands, &gun] {
            for b in &m.bones {
                assert!(
                    b.local_pos.length() < 1e-4,
                    "{}: {} {}",
                    m.lod,
                    b.name,
                    b.local_pos
                );
            }
        }
    }

    #[test]
    fn real_weapon_grafts_onto_body_tag_weapon_right() {
        let Some(fs) = crate::testing::game_fs() else {
            return;
        };
        let body = crate::xmodel::load(&fs, "playerbody_american_airborne").unwrap();
        let weapon = crate::xmodel::load(&fs, "weapon_kar98").unwrap();
        let (body_bones, weapon_bones) = (body.bones.len(), weapon.bones.len());

        let skel = Skeleton::build_grafted(&[(&body, None), (&weapon, Some("tag_weapon_right"))]);
        // the weapon's root aliases onto tag_weapon_right
        assert_eq!(skel.bone_count(), body_bones + weapon_bones - 1);

        let tag_idx = skel
            .bone_index("tag_weapon_right")
            .expect("player body rig has tag_weapon_right");
        let tag_bind =
            Mat4::from_rotation_translation(body.bones[tag_idx].rot, body.bones[tag_idx].pos);

        let pose = PoseBuffer::new(&skel);
        let weapon_root_skin = pose.skin_matrices(&skel, 1)[0];
        assert!(
            weapon_root_skin.abs_diff_eq(tag_bind, 1e-2),
            "weapon root skin {weapon_root_skin:?} != tag_weapon_right bind {tag_bind:?}"
        );
    }

    /// The head xmodel carries its own copy of the body's spine/neck/head
    /// chain; those must merge onto the body rig so the face tracks the
    /// animated head bone the helmet rides (else the head clips the helmet).
    #[test]
    fn real_head_shares_the_body_neck_chain() {
        let Some(fs) = crate::testing::game_fs() else {
            return;
        };
        let body = crate::xmodel::load(&fs, "playerbody_american_airborne").unwrap();
        let head = crate::xmodel::load(&fs, "basehead2").unwrap();
        let shared = head
            .bones
            .iter()
            .filter(|hb| body.bones.iter().any(|bb| bb.name == hb.name))
            .count();
        assert!(shared >= 3, "expected a shared spine/neck/head chain");

        let skel = Skeleton::build_grafted(&[(&body, None), (&head, None)]);
        assert_eq!(
            skel.bone_count(),
            body.bones.len() + head.bones.len() - shared
        );

        // posed head bone must land in the same place through either model
        let clip = crate::xanim::load(&fs, "pb_stand_alert").unwrap();
        let mut pose = PoseBuffer::new(&skel);
        pose.apply(&clip, &skel.bind(&clip), 0.0);
        let world_via = |model: usize, bones: &[Bone]| {
            let bi = bones
                .iter()
                .position(|b| b.name == "bip01 head")
                .expect("both rigs have bip01 head");
            pose.skin_matrices(&skel, model)[bi].transform_point3(bones[bi].pos)
        };
        let (via_body, via_head) = (world_via(0, &body.bones), world_via(1, &head.bones));
        assert!(
            via_body.abs_diff_eq(via_head, 1e-3),
            "body {via_body} vs head {via_head}"
        );
    }

    /// Translation keys read as offsets put the toes at z = 0; read as
    /// absolutes they sink the body about 37 units.
    #[test]
    fn player_clips_ground_the_feet() {
        let Some(fs) = crate::testing::game_fs() else {
            return;
        };
        for body_name in [
            "playerbody_german_fallschirmjagergrey",
            "playerbody_american_airborne",
        ] {
            let body = crate::xmodel::load(&fs, body_name).unwrap();
            let skel = Skeleton::build(&[&body]);
            let mut pelvis_z = Vec::new();
            for clip_name in ["pb_stand_alert", "pb_crouch_alert", "pb_prone_aim"] {
                let clip = crate::xanim::load(&fs, clip_name).unwrap();
                let binding = skel.bind(&clip);
                let mut pose = PoseBuffer::new(&skel);
                pose.apply(&clip, &binding, 0.0);
                // skin * bind_world is the posed world; model 0's bone order is the skeleton's
                let sm = pose.skin_matrices(&skel, 0);
                let world_z = |name: &str| {
                    let bi = body.bones.iter().position(|b| b.name == name).unwrap();
                    sm[bi].transform_point3(body.bones[bi].pos).z
                };
                let toes = world_z("bip01 l toe0").min(world_z("bip01 r toe0"));
                assert!(
                    toes.abs() < 2.0,
                    "{body_name}/{clip_name}: lowest toe at z={toes:.2}, expected ~0"
                );
                pelvis_z.push(world_z("bip01 pelvis"));
            }
            let [stand, crouch, prone] = pelvis_z[..] else {
                unreachable!()
            };
            assert!(stand > 30.0, "{body_name}: standing pelvis z={stand:.2}");
            assert!(
                (10.0..20.0).contains(&crouch),
                "{body_name}: crouched pelvis z={crouch:.2}"
            );
            assert!(prone < 10.0, "{body_name}: prone pelvis z={prone:.2}");
        }
    }
}
