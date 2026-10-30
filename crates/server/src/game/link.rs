//! `linkTo` on everything that is not a client: the link record, the
//! per-frame re-anchor and the release. A client's own link pins its
//! playerstate and lives on the sim (`crate::spectate::Link`).
//!
//! `docs/research/cod11-movers.md` section 15 is the measurement. In short:
//! a link stores the child's pose in the parent's frame (or a tag's); the
//! entity pass after the threads (`G_GeneralLink`) re-applies it off the
//! parent each frame, in entity-number order with the parent run first, one
//! level deep; script reads the result a frame later, the wire carries it
//! as `TR_INTERPOLATE` on the frame it was computed. Section 16 adds
//! `enableLinkTo`, turrets and a tag parent's model change.

use crate::game::host::GameHost;
use crate::game::spawn::axis_to_angles;
use glam::{Quat, Vec3};
use std::collections::{BTreeMap, HashSet};
use vcod_common::x87::angles_to_axis;
use vcod_gsc::{Cx, EntId, Host, Value};

/// A placement: `AnglesToAxis`' forward, left and up, and an origin. Retail
/// keeps the same thing as a 4x3 matrix (`record+0x10`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Frame {
    pub axis: [Vec3; 3],
    pub origin: Vec3,
}

impl Frame {
    pub fn from_pose(origin: [f32; 3], angles: [f32; 3]) -> Frame {
        Frame {
            axis: angles_to_axis(angles.into()),
            origin: origin.into(),
        }
    }

    fn from_quat(origin: Vec3, rot: Quat) -> Frame {
        Frame {
            axis: [rot * Vec3::X, rot * Vec3::Y, rot * Vec3::Z],
            origin,
        }
    }

    /// `MatrixMultiply43(self, parent)`: `self`, held in `parent`'s frame,
    /// placed in the world.
    pub fn within(&self, parent: &Frame) -> Frame {
        let to_world = |v: Vec3| parent.axis[0] * v.x + parent.axis[1] * v.y + parent.axis[2] * v.z;
        Frame {
            axis: self.axis.map(to_world),
            origin: parent.origin + to_world(self.origin),
        }
    }

    /// [`Frame::within`] as 1.5b's and UO's `MatrixMultiply43` rounds it:
    /// each element's sum of products rounded once.
    pub fn within_stored(&self, parent: &Frame) -> Frame {
        let [p0, p1, p2] = parent.axis.map(|a| a.as_dvec3());
        let to_world = |v: Vec3| {
            let v = v.as_dvec3();
            (p0 * v.x + p1 * v.y) + p2 * v.z
        };
        Frame {
            axis: self.axis.map(|a| to_world(a).as_vec3()),
            origin: (to_world(self.origin) + parent.origin.as_dvec3()).as_vec3(),
        }
    }

    /// `self` in `parent`'s frame, the inverse of [`Frame::within`]:
    /// `G_CalcTagAxis`' `MatrixInverseOrthogonal43` then `MatrixMultiply43`.
    pub fn relative_to(&self, parent: &Frame) -> Frame {
        let local = |v: Vec3| {
            Vec3::new(
                v.dot(parent.axis[0]),
                v.dot(parent.axis[1]),
                v.dot(parent.axis[2]),
            )
        };
        Frame {
            axis: self.axis.map(local),
            origin: local(self.origin - parent.origin),
        }
    }

    /// `AxisToAngles`, the way the link writes `r.currentAngles`.
    pub fn angles(&self) -> [f32; 3] {
        axis_to_angles(self.axis.map(|v| v.to_array()))
    }
}

/// One link record (`gentity_t+0x2e4`, 0x70 bytes): the parent, the bone the
/// tag named (`None` for the entity's own frame), and the child's placement
/// in that frame.
#[derive(Clone, Debug, PartialEq)]
pub struct Link {
    pub parent: EntId,
    pub tag: Option<String>,
    pub rel: Frame,
}

/// Every linked entity that is not a client, by child, and the entities
/// `enableLinkTo` opened to `linkTo`.
#[derive(Default)]
pub struct Links {
    rows: BTreeMap<EntId, Link>,
    enabled: HashSet<EntId>,
}

impl Links {
    pub fn get(&self, child: EntId) -> Option<&Link> {
        self.rows.get(&child)
    }

    pub fn contains(&self, child: EntId) -> bool {
        self.rows.contains_key(&child)
    }

    pub fn insert(&mut self, child: EntId, link: Link) {
        self.rows.insert(child, link);
    }

    /// `G_EntUnlink`: the child keeps the pose it has. Whether there was a
    /// record to drop.
    pub fn unlink(&mut self, child: EntId) -> bool {
        self.rows.remove(&child).is_some()
    }

    /// `G_FreeEntity`: the record and the `enableLinkTo` bit both go with
    /// the slot.
    pub fn forget(&mut self, id: EntId) {
        self.rows.remove(&id);
        self.enabled.remove(&id);
    }

    /// Whether `enableLinkTo` set the receiver's link bit.
    pub fn is_enabled(&self, id: EntId) -> bool {
        self.enabled.contains(&id)
    }

    pub fn enable(&mut self, id: EntId) {
        self.enabled.insert(id);
    }

    /// The children linked to `parent` with a tag, and their tags.
    pub fn tagged_children(&self, parent: EntId) -> Vec<(EntId, String)> {
        self.rows
            .iter()
            .filter(|(_, l)| l.parent == parent)
            .filter_map(|(c, l)| l.tag.clone().map(|t| (*c, t)))
            .collect()
    }

    /// Whether linking `child` to `parent` would close a loop: the helper
    /// behind `G_EntLinkTo` (0x662e0) refuses `parent == child` and a parent
    /// whose chain of parents reaches the child (0x66335..0x66353).
    pub fn would_cycle(&self, child: EntId, parent: EntId) -> bool {
        let mut at = Some(parent);
        while let Some(p) = at {
            if p == child {
                return true;
            }
            at = self.rows.get(&p).map(|l| l.parent);
        }
        false
    }
}

/// The client slot behind `id`, when it is a client's entity.
fn client_slot(host: &GameHost, id: EntId) -> Option<usize> {
    host.ents
        .get(id)
        .and_then(|e| e.client.as_ref())
        .map(|_| id.0 as usize)
}

fn vector(host: &mut GameHost, cx: &mut Cx, id: EntId, name: &str) -> [f32; 3] {
    let atom = cx.intern_folded(name);
    match host.get_field(cx, id, atom) {
        Value::Vector(v) => v,
        _ => [0.0; 3],
    }
}

/// A bone of the client's posed body in the body's own frame: the model
/// faces its `yaw` about the feet (`crate::game::item::tag_start`'s pose),
/// and `tag_origin`'s local tag turns the body from there by the legs' yaw
/// off the view (`docs/research/cod11-combat.md` 16.4).
pub fn client_bone(host: &mut GameHost, slot: usize, tag: &str) -> Option<(Vec3, Quat)> {
    let body = host.client_dobjs.get(slot)?.clone()?;
    let anims = host.anims.clone()?;
    let fs = host.fs.clone()?;
    let skel = host.hit_rigs.rig(&*fs, &body.pose.assembly)?;
    let bone = host.hit_rigs.bone(&*fs, &body.pose.assembly, &skel, tag)?;
    // The bone and its parents, each at the time and with the controls the
    // frame first posed it.
    let mut chain = vec![bone];
    while let Ok(p) = usize::try_from(skel.bones()[*chain.last()?].parent) {
        chain.push(p);
    }
    let now = host.dobj_time_ms;
    let posed = host.dobj_posed.get_mut(slot)?;
    let times = posed.stamp(skel.bones().len(), &chain, now, &body.pose);
    let mut poses: Vec<((i32, usize), vcod_common::skeleton::PoseBuffer)> = Vec::new();
    for &t in &times {
        if poses.iter().all(|(pt, _)| *pt != t) {
            let inputs = posed.inputs(&body.pose, &anims, t.0, t.1);
            let rigs = &mut host.hit_rigs;
            let pose = vcod_common::playerpose::pose_player(&skel, &inputs, |n| rigs.clip(&*fs, n));
            poses.push((t, pose));
        }
    }
    let (mut local, mut rot) = (Vec3::ZERO, Quat::IDENTITY);
    for (&b, t) in chain.iter().zip(&times).rev() {
        let pose = &poses.iter().find(|(pt, _)| pt == t)?.1;
        let (lp, lr) = pose.local(b);
        (local, rot) = (local + rot * lp, rot * lr);
    }
    let frame = body.frame();
    Some((frame * local, frame * rot))
}

/// [`client_bone`] in the model's own frame as 1.5b's and UO's skeleton
/// build leaves it (`PoseBuffer::stored_worlds`): its origin and its
/// matrix's axes, each bone of the chain from the build that first posed
/// it. `None` where the pose kept no engine records.
fn client_bone_stored(host: &mut GameHost, slot: usize, tag: &str) -> Option<(Vec3, [Vec3; 3])> {
    let body = host.client_dobjs.get(slot)?.clone()?;
    let anims = host.anims.clone()?;
    let fs = host.fs.clone()?;
    let skel = host.hit_rigs.rig(&*fs, &body.pose.assembly)?;
    let bone = host.hit_rigs.bone(&*fs, &body.pose.assembly, &skel, tag)?;
    let mut chain = vec![bone];
    while let Ok(p) = usize::try_from(skel.bones()[*chain.last()?].parent) {
        chain.push(p);
    }
    let now = host.dobj_time_ms;
    let posed = host.dobj_posed.get_mut(slot)?;
    let times = posed.stamp(skel.bones().len(), &chain, now, &body.pose);
    let mut merged: Option<vcod_common::skeleton::PoseBuffer> = None;
    let mut done: Vec<(i32, usize)> = Vec::new();
    for &t in &times {
        if done.contains(&t) {
            continue;
        }
        done.push(t);
        let inputs = posed.inputs(&body.pose, &anims, t.0, t.1);
        let rigs = &mut host.hit_rigs;
        let pose = vcod_common::playerpose::pose_player(&skel, &inputs, |n| rigs.clip(&*fs, n));
        match merged.as_mut() {
            None => merged = Some(pose),
            Some(m) => {
                for (&b, _) in chain.iter().zip(&times).filter(|(_, bt)| **bt == t) {
                    m.copy_bone(&pose, b);
                }
            }
        }
    }
    let b = merged?.stored_worlds(&skel)?[bone];
    Some((b.origin, b.axes))
}

/// [`client_bone`] in the model's own frame, before the body's yaw: what
/// a link holds in the parent entity's `angles` (`G_CalcTagParentAxis`
/// multiplies the tag's model-space matrix by the parent's axis).
fn client_bone_local(host: &mut GameHost, slot: usize, tag: &str) -> Option<(Vec3, Quat)> {
    let frame = host.client_dobjs.get(slot)?.as_ref()?.frame().inverse();
    let (o, r) = client_bone(host, slot, tag)?;
    Some((frame * o, frame * r))
}

/// Whether the client's body has `tag`, without posing it.
fn client_has_bone(host: &mut GameHost, slot: usize, tag: &str) -> bool {
    let Some(body) = host.client_dobjs.get(slot).cloned().flatten() else {
        return false;
    };
    let Some(fs) = host.fs.clone() else {
        return false;
    };
    let Some(skel) = host.hit_rigs.rig(&*fs, &body.pose.assembly) else {
        return false;
    };
    host.hit_rigs
        .bone(&*fs, &body.pose.assembly, &skel, tag)
        .is_some()
}

/// The models a non-client entity's DObj holds: its `model`, then each
/// `attach`ed one on its tag. `None` with no model.
fn entity_assembly(
    host: &GameHost,
    cx: &mut Cx,
    model: &str,
    id: EntId,
) -> Option<crate::game::hitrig::Assembly> {
    use crate::game::hitrig::{Assembly, model_name};
    if model.is_empty() {
        return None;
    }
    let attachments = host
        .ents
        .get(id)
        .map(|e| {
            e.attachments
                .iter()
                .map(|&(m, t)| {
                    let tag = cx.resolve(t);
                    let tag = (!tag.is_empty()).then(|| tag.to_string());
                    (model_name(cx.resolve(m)), tag)
                })
                .collect()
        })
        .unwrap_or_default();
    Some(Assembly {
        body: model_name(model),
        attachments,
        ignore_collision: 0,
    })
}

/// A bone of a non-client entity's model, attachments included, at its
/// bind pose in model space. A script model plays no animation in MP, so
/// the bind pose is the pose.
fn model_bone(
    host: &mut GameHost,
    cx: &mut Cx,
    model: &str,
    id: EntId,
    tag: &str,
) -> Option<(Vec3, Quat)> {
    if matches!(host.ext, crate::game::host::HostExt::Cod4(_)) {
        return cod4_model_tag(host, cx, model, id, tag);
    }
    let fs = host.fs.clone()?;
    let assembly = entity_assembly(host, cx, model, id)?;
    let skel = host.hit_rigs.rig(&*fs, &assembly)?;
    let bone = host.hit_rigs.bone(&*fs, &assembly, &skel, tag)?;
    Some(vcod_common::skeleton::PoseBuffer::new(&skel).bone_world(&skel, bone))
}

/// [`model_bone`] on CoD4: the zones' bind-pose tags
/// (`Cod4HostState::model_tags`), the body's first, then each `attach`ed
/// model's placed at the body tag it hangs on, as the DObj lists them
/// (cod4-gsc.md 9.4). The pk3 rigs know no CoD4 model.
pub(crate) fn cod4_model_tag(
    host: &GameHost,
    cx: &mut Cx,
    model: &str,
    id: EntId,
    tag: &str,
) -> Option<(Vec3, Quat)> {
    use crate::game::hitrig::model_name;
    let tags = &host.cod4().model_tags;
    let find = |model: &str, name: &str| {
        let t = tags.get(&model_name(model).to_ascii_lowercase())?;
        t.iter()
            .find(|(n, _, _)| n.eq_ignore_ascii_case(name))
            .map(|&(_, p, r)| (p, r))
    };
    if let Some(t) = find(model, tag) {
        return Some(t);
    }
    let attachments = host.ents.get(id).map(|e| e.attachments.clone())?;
    attachments.iter().find_map(|&(m, t)| {
        let (bone_pos, bone_rot) = find(cx.resolve(m), tag)?;
        let graft = cx.resolve(t);
        let (gp, gr) = if graft.is_empty() {
            (Vec3::ZERO, Quat::IDENTITY)
        } else {
            find(model, graft)?
        };
        Some((gp + gr * bone_pos, gr * bone_rot))
    })
}

/// Why `linkTo` refused, as retail words it (strings at 0x76bc0..0x76ca0).
#[derive(Clone, Debug, PartialEq)]
pub enum LinkError {
    NoModel,
    NoTag { tag: String, model: String },
    Cycle,
}

impl LinkError {
    pub fn message(&self) -> String {
        match self {
            LinkError::NoModel => "failed to link entity since parent has no model".into(),
            LinkError::NoTag { tag, model } => format!(
                "failed to link entity since tag '{tag}' does not exist in parent model '{model}'"
            ),
            LinkError::Cycle => "failed to link entity due to link cycle".into(),
        }
    }
}

/// The frame `parent` (or its `tag`) has, given the entity's own `base`.
fn tag_frame(
    host: &mut GameHost,
    cx: &mut Cx,
    parent: EntId,
    base: Frame,
    tag: Option<&str>,
    pose: bool,
) -> Result<Frame, LinkError> {
    let Some(tag) = tag else {
        return Ok(base);
    };
    if let Some(slot) = client_slot(host, parent) {
        // The four-argument form only looks the bone up (0x6631b); posing it
        // would fix the frame's skeleton early.
        let stored = pose
            && vcod_common::patch::Arith::for_protocol(host.defs.proto)
                == vcod_common::patch::Arith::Stored;
        let found = if stored && let Some((o, axis)) = client_bone_stored(host, slot, tag) {
            // `G_CalcTagParentAxis`: the tag's matrix times the parent's
            // stored axis, each element rounded once.
            let angles = vector(host, cx, parent, "angles");
            let base = Frame {
                axis: angles_to_axis(angles.into()),
                origin: base.origin,
            };
            Some(Frame { axis, origin: o }.within_stored(&base))
        } else if pose {
            client_bone_local(host, slot, tag).map(|(o, r)| Frame::from_quat(o, r).within(&base))
        } else {
            client_has_bone(host, slot, tag).then_some(base)
        };
        return found.ok_or_else(|| LinkError::NoTag {
            tag: tag.to_string(),
            model: model_of(host, cx, parent),
        });
    }
    let model = model_of(host, cx, parent);
    if model.is_empty() {
        return Err(LinkError::NoModel);
    }
    let (o, r) = model_bone(host, cx, &model, parent, tag).ok_or_else(|| LinkError::NoTag {
        tag: tag.to_string(),
        model: model.clone(),
    })?;
    Ok(Frame::from_quat(o, r).within(&base))
}

/// Whether `id`'s model carries `tag`, the `DObjGetBoneIndex` test
/// `playFxOnTag` makes (game.mp 0x5b338), without posing it.
pub fn tag_check(host: &mut GameHost, cx: &mut Cx, id: EntId, tag: &str) -> Result<(), LinkError> {
    tag_frame(
        host,
        cx,
        id,
        Frame::from_pose([0.0; 3], [0.0; 3]),
        Some(tag),
        false,
    )
    .map(|_| ())
}

fn model_of(host: &mut GameHost, cx: &mut Cx, id: EntId) -> String {
    let atom = cx.intern_folded("model");
    match host.get_field(cx, id, atom) {
        Value::String(s) => cx.resolve(s).to_string(),
        _ => String::new(),
    }
}

/// The entity's own frame off its fields: `r.currentOrigin` and
/// `r.currentAngles`. A client's angles read zero but on the frame of a
/// `setPlayerAngles`, which is what retail's link reads too.
fn field_frame(host: &mut GameHost, cx: &mut Cx, id: EntId) -> Frame {
    Frame::from_pose(
        vector(host, cx, id, "origin"),
        vector(host, cx, id, "angles"),
    )
}

/// `self linkTo(parent [, tag [, originOffset, anglesOffset]])` on an entity
/// that is not a client (`G_EntLinkTo` 0x68034, `G_EntLinkToWithOffset`
/// 0x68074). `offset` is the four-argument form's pair; without it the
/// child keeps the pose it has, held in the parent's frame. A failed link
/// leaves the child unlinked: the helper's `G_EntUnlink` comes first.
pub fn link(
    host: &mut GameHost,
    cx: &mut Cx,
    child: EntId,
    parent: EntId,
    tag: Option<&str>,
    offset: Option<([f32; 3], [f32; 3])>,
) -> Result<(), LinkError> {
    unlink(host, child);
    let tag = tag.filter(|t| !t.is_empty());
    let base = field_frame(host, cx, parent);
    let frame = tag_frame(host, cx, parent, base, tag, offset.is_none());
    if host.links.would_cycle(child, parent) {
        // The builtin's error branch tests the parent's model before it
        // names the cycle (0x59dfd..0x59ea8).
        return Err(match frame {
            Err(e) => e,
            Ok(_)
                if client_slot(host, parent).is_none() && model_of(host, cx, parent).is_empty() =>
            {
                LinkError::NoModel
            }
            Ok(_) => LinkError::Cycle,
        });
    }
    let frame = frame?;
    let rel = match offset {
        Some((origin, angles)) => Frame::from_pose(origin, angles),
        None => field_frame(host, cx, child).relative_to(&frame),
    };
    host.links.insert(
        child,
        Link {
            parent,
            tag: tag.map(str::to_string),
            rel,
        },
    );
    // A verb's plan does not survive the link: `G_RunMover` takes the link
    // arm instead of `G_MoverTeam` (0x57616) and the unlink stops the
    // trajectory, so the notify never comes (movers doc, 15).
    host.movers.forget(child);
    if let Some(i) = host.ents.get_mut(child).and_then(|e| e.item.as_mut()) {
        i.pos = None;
        i.apos = None;
    }
    Ok(())
}

/// `G_EntUnlink` (0x680d4): the record goes and the child stays where it
/// is. Its `G_SetAngle` leaves a turret's `apos` stationary for good
/// (movers doc 16).
pub fn unlink(host: &mut GameHost, child: EntId) {
    if host.links.unlink(child) {
        angle_set(host, child);
    }
}

fn angle_set(host: &mut GameHost, id: EntId) {
    if let Some(r) = host.turrets.get_mut(&id) {
        r.angle_set = true;
    }
}

/// `G_UpdateTagInfoOfChildren` (0x68294), which `G_DObjUpdate` runs after
/// `setModel`, `attach`, `detach` and `detachAll` on an entity that is not
/// a client: a child on a tag is unlinked where it stands when the
/// parent's new models have no such bone, or there is no model at all. A child on the entity's own frame keeps its link.
/// A bone the new model has is looked up again on every pass, so a kept
/// link needs nothing more here.
pub fn model_changed(host: &mut GameHost, cx: &mut Cx, parent: EntId) {
    if client_slot(host, parent).is_some() {
        return;
    }
    let model = model_of(host, cx, parent);
    for (child, tag) in host.links.tagged_children(parent) {
        if model_bone(host, cx, &model, parent, &tag).is_none() {
            unlink(host, child);
        }
    }
}

/// `self enableLinkTo()` (0x5d5d0): opens a trigger, or any other plain
/// entity with no think, to `linkTo`, with `Think_GeneralLink` as its think
/// (object-model doc 23.2). Errors as retail words them.
pub fn enable(host: &mut GameHost, cx: &mut Cx, id: EntId) -> Result<(), String> {
    let classname = classname(host, cx, id);
    if has_link_bit(host, id, &classname) {
        return Err("entity already has linkTo enabled".into());
    }
    // `eType` and `physicsObject` both 0: none of ours is, past the
    // receivers the bit test above already took, but a missile or a body.
    let thinks = host
        .ents
        .get(id)
        .is_some_and(|e| e.think.is_some() || e.nextthink != 0);
    let general = host.ents.get(id).is_some_and(|e| e.hud.is_none());
    if !general || (thinks && !classname.eq_ignore_ascii_case("trigger_multiple")) {
        return Err(format!(
            "entity (classname: '{classname}') does not currently support enableLinkTo"
        ));
    }
    host.links.enable(id);
    Ok(())
}

/// `ent+0x17d` bit 0x20, which `linkTo` gates its receiver on: set by
/// `G_SpawnItem`, `G_SpawnTurret`, `InitScriptMover`, `ClientSpawn` and
/// `enableLinkTo` (object-model doc 23.2).
pub fn has_link_bit(host: &GameHost, id: EntId, classname: &str) -> bool {
    let Some(e) = host.ents.get(id) else {
        return false;
    };
    e.client.is_some()
        || e.item.is_some()
        || host.turrets.contains_key(&id)
        || host.links.is_enabled(id)
        || matches!(
            classname,
            "script_model" | "script_origin" | "script_brushmodel"
        )
}

/// `G_RunFrame`'s entity loop, the link half: every linked entity re-anchored
/// off its parent, ascending by entity number, each one's parent run first
/// (0x50939..0x50955). A parent's parent is not, so a chain whose parents
/// sit above their children in number lags a frame per level. Runs after
/// the threads, like retail's pass; the result is what script reads on the
/// next frame and what this frame's snapshot carries.
pub fn run(host: &mut GameHost, cx: &mut Cx) {
    prune(host);
    let level_ms = host.level_time_ms;
    let links = std::mem::take(&mut host.links);
    let children: Vec<EntId> = links.rows.keys().copied().collect();
    // Every entity the loop has run this frame, below `child.0` or not.
    let mut ran: HashSet<EntId> = HashSet::new();
    for child in children {
        let Some(parent) = links.rows.get(&child).map(|l| l.parent) else {
            continue;
        };
        // The parent first. Its own parent is current only if the loop
        // has already passed it.
        let current = |g: EntId, ran: &HashSet<EntId>| g.0 < child.0 || ran.contains(&g);
        if links.contains(parent) && !ran.contains(&parent) {
            let grand = links.rows[&parent].parent;
            let fresh = current(grand, &ran);
            run_one(host, cx, &links, parent, fresh, level_ms);
        }
        ran.insert(parent);
        if ran.insert(child) {
            run_one(host, cx, &links, child, true, level_ms);
        }
    }
    host.links = links;
}

/// Drops the rows of freed children, and unlinks where they stand the
/// children of a freed parent (`G_FreeEntity` 0x669f1..0x66a9e). The entity
/// pass runs this once ahead of its loop.
pub fn prune(host: &mut GameHost) {
    let mut links = std::mem::take(&mut host.links);
    links.rows.retain(|id, _| host.ents.get(*id).is_some());
    let orphans: Vec<EntId> = links
        .rows
        .iter()
        .filter(|(_, l)| host.ents.get(l.parent).is_none())
        .map(|(c, _)| *c)
        .collect();
    for child in orphans {
        links.rows.remove(&child);
        angle_set(host, child);
    }
    host.links = links;
}

/// `G_RunEntity`'s link arm for one entity (0x50385, and `G_RunMover`'s
/// 0x57616): [`run_one`] when `id` is linked. `fresh` as there. Returns
/// whether it ran.
pub fn run_linked(host: &mut GameHost, cx: &mut Cx, id: EntId, fresh: bool) -> bool {
    if !host.links.contains(id) {
        return false;
    }
    let level_ms = host.level_time_ms;
    let links = std::mem::take(&mut host.links);
    run_one(host, cx, &links, id, fresh, level_ms);
    host.links = links;
    true
}

/// One `G_GeneralLink` (0x68530): `G_SetFixedLink`'s mode 0, the origin and
/// angles written, both trajectories `TR_INTERPOLATE`, a relink. `fresh`
/// says whether the parent has run this frame: a mover parent that has is
/// on its trajectory at the level time, `G_MoverTeam`'s clock, and one that
/// has not is still where script reads it.
fn run_one(host: &mut GameHost, cx: &mut Cx, links: &Links, id: EntId, fresh: bool, level_ms: i32) {
    let Some(link) = links.rows.get(&id) else {
        return;
    };
    let parent = link.parent;
    let mut base = field_frame(host, cx, parent);
    if fresh
        && !links.contains(parent)
        && let Some((Some(p), _)) = host.movers.pose_at(parent, level_ms)
    {
        // The angles stay the field's, `r.currentAngles`, which can sit a
        // quantization off `apos` (movers doc 12, "The residual yaw").
        base.origin = p;
    }
    let frame = tag_frame(host, cx, parent, base, link.tag.as_deref(), true).unwrap_or(base);
    let placed = link.rel.within(&frame);
    let origin = placed.origin.to_array();
    let angles = placed.angles();
    for (name, v) in [("origin", origin), ("angles", angles)] {
        let atom = cx.intern_folded(name);
        let _ = host.write_field(cx, id, atom, Value::Vector(v));
    }
    host.movers.forget(id);
    host.link_entity_at(cx, id, Some((origin, angles)));
    // A brush model's clip goes where the link puts it, with no push.
    if let (Some(model), Some(world)) = (
        host.ents
            .get(id)
            .and_then(|e| e.brush_model)
            .filter(|&n| n > 0),
        host.world.clone(),
    ) {
        world.collision.set_model_pose(
            model as usize,
            placed.origin,
            crate::game::mover::snap_angles(Vec3::from(angles)),
        );
    }
}

/// The entity's `classname`, `""` when it has none.
pub fn classname(host: &mut GameHost, cx: &mut Cx, id: EntId) -> String {
    let atom = cx.intern_folded("classname");
    match host.get_field(cx, id, atom) {
        Value::String(s) => cx.resolve(s).to_string(),
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::builtins::entity::{link_to, unlink};
    use crate::game::testing::fixture;
    use vcod_gsc::{Target, Vm};

    fn origin(host: &mut GameHost, cx: &mut Cx, id: EntId) -> [f32; 3] {
        vector(host, cx, id, "origin")
    }

    fn spawn(
        host: &mut GameHost,
        cx: &mut Cx,
        class: &str,
        at: [f32; 3],
        angles: [f32; 3],
    ) -> EntId {
        let id = host.ents.spawn(cx).unwrap();
        let c = cx.intern_exact(class);
        for (name, v) in [
            ("classname", Value::String(c)),
            ("origin", Value::Vector(at)),
            ("angles", Value::Vector(angles)),
        ] {
            let atom = cx.intern_folded(name);
            host.set_field(cx, id, atom, v).unwrap();
        }
        id
    }

    fn close(a: [f32; 3], b: [f32; 3]) -> bool {
        (0..3).all(|i| (a[i] - b[i]).abs() < 1e-3)
    }

    /// The child keeps its pose in the parent's frame: a parent that moves
    /// and turns carries it round, the pass writes the new pose, and an
    /// unlink leaves it where it is.
    #[test]
    fn a_child_rides_its_parents_frame_until_unlinked() {
        let (mut vm, mut host): (Vm, GameHost) = fixture();
        vm.with_cx(|cx| {
            let parent = spawn(&mut host, cx, "script_origin", [100.0, 0.0, 0.0], [0.0; 3]);
            let child = spawn(
                &mut host,
                cx,
                "script_model",
                [132.0, 0.0, 8.0],
                [0.0, 30.0, 0.0],
            );
            link_to(
                &mut host,
                cx,
                Some(Target::Entity(child)),
                &[Value::Entity(parent)],
            )
            .unwrap();
            run(&mut host, cx);
            assert!(
                close(origin(&mut host, cx, child), [132.0, 0.0, 8.0]),
                "the link moves nothing"
            );

            let o = cx.intern_folded("origin");
            let a = cx.intern_folded("angles");
            host.set_field(cx, parent, o, Value::Vector([0.0, 0.0, 0.0]))
                .unwrap();
            host.set_field(cx, parent, a, Value::Vector([0.0, 90.0, 0.0]))
                .unwrap();
            run(&mut host, cx);
            assert!(close(origin(&mut host, cx, child), [0.0, 32.0, 8.0]));
            let angles = vector(&mut host, cx, child, "angles");
            assert!((angles[1] - 120.0).abs() < 1e-3, "{angles:?}");

            unlink(&mut host, cx, Some(Target::Entity(child)), &[]).unwrap();
            host.set_field(cx, parent, o, Value::Vector([500.0, 0.0, 0.0]))
                .unwrap();
            run(&mut host, cx);
            assert!(
                close(origin(&mut host, cx, child), [0.0, 32.0, 8.0]),
                "unlinked"
            );
        });
    }

    /// The receiver gate and the cycle, in retail's words: a model-less
    /// parent names the model, not the cycle.
    #[test]
    fn link_refuses_what_retail_refuses() {
        let (mut vm, mut host): (Vm, GameHost) = fixture();
        vm.with_cx(|cx| {
            let x = spawn(&mut host, cx, "script_origin", [0.0; 3], [0.0; 3]);
            let y = spawn(&mut host, cx, "script_origin", [32.0, 0.0, 0.0], [0.0; 3]);
            let t = spawn(&mut host, cx, "trigger_radius", [0.0; 3], [0.0; 3]);
            let err = |r: Result<Value, vcod_gsc::ErrorKind>| match r {
                Err(vcod_gsc::ErrorKind::Custom(m)) => m,
                other => panic!("{other:?}"),
            };
            assert_eq!(
                err(link_to(
                    &mut host,
                    cx,
                    Some(Target::Entity(t)),
                    &[Value::Entity(x)]
                )),
                "entity (classname: 'trigger_radius') does not currently support linkTo"
            );
            link_to(&mut host, cx, Some(Target::Entity(x)), &[Value::Entity(y)]).unwrap();
            assert_eq!(
                err(link_to(
                    &mut host,
                    cx,
                    Some(Target::Entity(y)),
                    &[Value::Entity(x)]
                )),
                "failed to link entity since parent has no model"
            );
            assert!(!host.links.contains(y), "the failed link left y unlinked");
        });
    }

    /// `enableLinkTo` opens a trigger to `linkTo` once; an entity that has
    /// the bit already, or a think on anything but a `trigger_multiple`,
    /// is refused in retail's words.
    #[test]
    fn enable_link_to_opens_a_trigger_once() {
        let (mut vm, mut host): (Vm, GameHost) = fixture();
        vm.with_cx(|cx| {
            let p = spawn(&mut host, cx, "script_origin", [0.0; 3], [0.0; 3]);
            let t = spawn(
                &mut host,
                cx,
                "trigger_multiple",
                [16.0, 0.0, 0.0],
                [0.0; 3],
            );
            let u = spawn(&mut host, cx, "trigger_use", [0.0; 3], [0.0; 3]);
            let msg = |r: Result<(), String>| r.unwrap_err();
            assert_eq!(
                msg(enable(&mut host, cx, p)),
                "entity already has linkTo enabled"
            );
            host.ents.get_mut(t).unwrap().think = Some(crate::game::entity::ThinkFn::Free);
            host.ents.get_mut(u).unwrap().think = Some(crate::game::entity::ThinkFn::Free);
            assert_eq!(
                msg(enable(&mut host, cx, u)),
                "entity (classname: 'trigger_use') does not currently support enableLinkTo"
            );
            // A `trigger_multiple` passes with a think (its wait's).
            enable(&mut host, cx, t).unwrap();
            assert_eq!(
                msg(enable(&mut host, cx, t)),
                "entity already has linkTo enabled"
            );
            link_to(&mut host, cx, Some(Target::Entity(t)), &[Value::Entity(p)]).unwrap();
            let o = cx.intern_folded("origin");
            host.set_field(cx, p, o, Value::Vector([0.0, 0.0, 32.0]))
                .unwrap();
            run(&mut host, cx);
            assert!(close(origin(&mut host, cx, t), [16.0, 0.0, 32.0]));
        });
    }

    /// A parent above its child in number runs first; a grandparent above
    /// both does not, and the chain reads it a frame late (movers doc, 15).
    #[test]
    fn a_chain_lags_a_frame_where_the_grandparent_numbers_high() {
        use crate::game::mover::Ramp;
        let (mut vm, mut host): (Vm, GameHost) = fixture();
        vm.with_cx(|cx| {
            // Forward: the mover lowest, each child above its parent.
            let f0 = spawn(&mut host, cx, "script_origin", [0.0; 3], [0.0; 3]);
            let f1 = spawn(&mut host, cx, "script_origin", [32.0, 0.0, 0.0], [0.0; 3]);
            let f2 = spawn(&mut host, cx, "script_origin", [64.0, 0.0, 0.0], [0.0; 3]);
            // Reversed: the mover highest.
            let r2 = spawn(&mut host, cx, "script_origin", [64.0, 0.0, 0.0], [0.0; 3]);
            let r1 = spawn(&mut host, cx, "script_origin", [32.0, 0.0, 0.0], [0.0; 3]);
            let r0 = spawn(&mut host, cx, "script_origin", [0.0; 3], [0.0; 3]);
            for (c, p) in [(f1, f0), (f2, f1), (r2, r1), (r1, r0)] {
                link_to(&mut host, cx, Some(Target::Entity(c)), &[Value::Entity(p)]).unwrap();
            }
            host.level_time_ms = 1000;
            for m in [f0, r0] {
                host.movers.move_to(
                    m,
                    1000,
                    Vec3::ZERO,
                    Vec3::new(0.0, 0.0, 100.0),
                    Ramp::new(1.0, 0.0, 0.0),
                );
            }
            host.level_time_ms = 1050;
            // f0, numbered below its chain, has had its turn; r0, numbered
            // above, has not, and still reads 0.
            crate::game::mover::run_one(&mut host, cx, f0);
            run(&mut host, cx);
            assert!(close(origin(&mut host, cx, f1), [32.0, 0.0, 5.0]));
            assert!(close(origin(&mut host, cx, f2), [64.0, 0.0, 5.0]));
            assert!(
                close(origin(&mut host, cx, r1), [32.0, 0.0, 0.0]),
                "r1 read r0 before it ran"
            );
            assert!(close(origin(&mut host, cx, r2), [64.0, 0.0, 0.0]));
        });
    }

    /// A CoD4 parent's tag is the zones' bind-pose tag (`model_tags`), the
    /// body model's or an attached one's on the body tag it hangs on: the
    /// destructible sedans' `play_sound_on_tag` links a sound to
    /// `tag_glass_back_d` on the body and a door's `tag_glass_left_front`
    /// rides the door attached at `tag_door_left_front`.
    #[test]
    fn a_cod4_link_finds_the_zone_tags_of_the_body_and_its_attachments() {
        let mut host = GameHost::new(&crate::game::profile::COD4, vec![String::new(); 2048]);
        let mut vm = Vm::new();
        let q = Quat::from_rotation_z(std::f32::consts::FRAC_PI_2);
        let tags = |t: &[(&str, Vec3, Quat)]| {
            std::rc::Rc::new(t.iter().map(|&(n, p, r)| (n.to_string(), p, r)).collect())
        };
        let c4 = host.cod4_mut();
        c4.model_tags.insert(
            "car".into(),
            tags(&[
                (
                    "tag_glass_back_d",
                    Vec3::new(10.0, 0.0, 5.0),
                    Quat::IDENTITY,
                ),
                ("tag_door_left_front", Vec3::new(0.0, 20.0, 0.0), q),
            ]),
        );
        c4.model_tags.insert(
            "door".into(),
            tags(&[(
                "tag_glass_left_front",
                Vec3::new(4.0, 0.0, 0.0),
                Quat::IDENTITY,
            )]),
        );
        vm.with_cx(|cx| {
            let car = spawn(&mut host, cx, "script_model", [100.0, 0.0, 0.0], [0.0; 3]);
            let m = cx.intern_folded("model");
            let car_name = cx.intern_exact("car");
            host.set_field(cx, car, m, Value::String(car_name)).unwrap();
            let (door, at) = (
                cx.intern_exact("door"),
                cx.intern_exact("tag_door_left_front"),
            );
            host.ents.get_mut(car).unwrap().attachments.push((door, at));
            let glass = spawn(&mut host, cx, "script_origin", [0.0; 3], [0.0; 3]);
            let side = spawn(&mut host, cx, "script_origin", [0.0; 3], [0.0; 3]);
            let none = Some(([0.0; 3], [0.0; 3]));
            link(&mut host, cx, glass, car, Some("TAG_GLASS_BACK_D"), none).unwrap();
            link(&mut host, cx, side, car, Some("tag_glass_left_front"), none).unwrap();
            assert_eq!(
                link(&mut host, cx, side, car, Some("tag_nothing"), none),
                Err(LinkError::NoTag {
                    tag: "tag_nothing".into(),
                    model: "car".into()
                })
            );
            link(&mut host, cx, side, car, Some("tag_glass_left_front"), none).unwrap();
            run(&mut host, cx);
            assert!(close(origin(&mut host, cx, glass), [110.0, 0.0, 5.0]));
            // The door turned a quarter at its tag: its +x is the car's +y.
            assert!(close(origin(&mut host, cx, side), [100.0, 24.0, 0.0]));
        });
    }
}
