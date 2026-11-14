//! Merges several xmodel bone hierarchies (hands and gun, body and
//! attachments) into one skeleton and evaluates anim poses into skin matrices.

use crate::model::Model;
use crate::xanim::XAnim;
use glam::{Mat4, Quat, Vec3};
use std::collections::{HashMap, HashSet};
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
    /// The bone's hit box in its own frame, all-zero on a bone no shot can
    /// score, and its effective hit location after the graft's inheritance.
    /// docs/research/cod11-combat.md section 3.
    pub hit_mins: Vec3,
    pub hit_maxs: Vec3,
    pub hit_location: u8,
}

impl SkelBone {
    /// A zero-size box is how the artist turns a bone off; the trace skips it.
    pub fn has_hit_box(&self) -> bool {
        (self.hit_maxs - self.hit_mins).max_element() > 0.0
    }
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
    /// Bone name -> the first bone of that name, what `bind` and
    /// `bone_index` look up.
    index: HashMap<String, usize>,
}

/// Track index -> skeleton bone index; `None` for bones the skeleton lacks.
pub struct AnimBinding(pub(crate) Vec<Option<usize>>);

/// One skeleton's [`Skeleton::bind`]s, made once per clip: a body poses
/// every frame off the same few clips. Kept beside the skeleton it binds
/// for; each clip is held, so its address stays its own.
#[derive(Default)]
pub struct BindCache(std::cell::RefCell<HashMap<usize, KeptBinding>>);

type KeptBinding = (std::rc::Rc<XAnim>, std::rc::Rc<AnimBinding>);

impl BindCache {
    /// `skel.bind(anim)`, from the cache once made.
    pub fn bind(&self, skel: &Skeleton, anim: &std::rc::Rc<XAnim>) -> std::rc::Rc<AnimBinding> {
        let key = std::rc::Rc::as_ptr(anim) as usize;
        self.0
            .borrow_mut()
            .entry(key)
            .or_insert_with(|| (anim.clone(), std::rc::Rc::new(skel.bind(anim))))
            .1
            .clone()
    }
}

/// A merged bone takes the box of the model that brings one. The
/// playerbody's own `bip01 head` and `bip01 neck` boxes are all-zero and the
/// real ones ride in the attached head model, so the head is only hittable
/// once the attachment hands its box to the body bone it merges onto. A
/// second box for a bone that has one already (the helmet's root on the head
/// model's `bip01 head`) is its own part's bone on retail and traced as well
/// (docs/research/cod11-combat.md 3.1): it comes back as a box-only bone to
/// hang off `slot` at its origin.
fn adopt_hit_box(slot: &mut SkelBone, from: &crate::model::Bone) -> Option<SkelBone> {
    if (from.hit_maxs - from.hit_mins).max_element() <= 0.0 {
        return None;
    }
    if slot.has_hit_box() {
        return Some(SkelBone {
            name: from.name.clone(),
            parent: -1,
            local_pos: Vec3::ZERO,
            local_rot: Quat::IDENTITY,
            hit_mins: from.hit_mins,
            hit_maxs: from.hit_maxs,
            hit_location: from.hit_location,
        });
    }
    slot.hit_mins = from.hit_mins;
    slot.hit_maxs = from.hit_maxs;
    slot.hit_location = from.hit_location;
    None
}

/// [`adopt_hit_box`] onto `bones[si]`, a second box appended as a child of
/// it. A name lookup (a clip's track, a tag) finds `bones[si]` first, so the
/// extra bone keeps its identity local.
fn extra_box(bones: &mut Vec<SkelBone>, si: usize, from: &crate::model::Bone) {
    if let Some(mut extra) = adopt_hit_box(&mut bones[si], from) {
        extra.parent = si as i32;
        bones.push(extra);
    }
}

/// Code 0 (`none`) means "inherit", which retail resolves per bone against
/// the parent within the part and against the graft point for a part's roots
/// (docs/research/cod11-combat.md section 3). Both collapse to the parent in
/// the grafted tree, where an attachment's root *is* the bone it hangs off.
/// Parents precede children, so one pass resolves the chain.
fn inherit_hit_locations(bones: &mut [SkelBone]) {
    for i in 0..bones.len() {
        if bones[i].hit_location != 0 {
            continue;
        }
        let parent = bones[i].parent;
        if parent >= 0 {
            bones[i].hit_location = bones[parent as usize].hit_location;
        }
    }
}

impl Skeleton {
    /// `build_grafted` with no explicit tags.
    pub fn build(models: &[&Model]) -> Skeleton {
        Skeleton::build_grafted(&models.iter().map(|m| (*m, None)).collect::<Vec<_>>())
    }

    /// Each later model's root aliases the skeleton bone its tag names, or,
    /// absent a tag, the bone with the same name (the engine default). Its
    /// other bones alias same-named base-rig bones too (the engine shares one
    /// bone tree: a head model's own `bip01 neck`/`bip01 head` must ride the
    /// body's animated copies, not unkeyed duplicates); genuinely new bones
    /// append with remapped parents.
    pub fn build_grafted(models: &[(&Model, Option<&str>)]) -> Skeleton {
        let mut bones: Vec<SkelBone> = Vec::new();
        let mut maps: Vec<Vec<usize>> = Vec::new();
        let mut inv_binds: Vec<Vec<Mat4>> = Vec::new();
        for (mi, (model, graft_tag)) in models.iter().enumerate() {
            // Merges scope to model 0's bones so two attachments that happen to
            // share a bone name never alias each other.
            let base_len = if mi == 0 { 0 } else { maps[0].len() };
            let mut map = Vec::with_capacity(model.bones.len());
            for (bi, b) in model.bones.iter().enumerate() {
                if mi > 0
                    && bi > 0
                    && let Some(si) = bones[..base_len].iter().position(|s| s.name == b.name)
                {
                    extra_box(&mut bones, si, b);
                    map.push(si);
                    continue;
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
                                .insert(format!("ambiguous:{}:{target}", model.name))
                        {
                            log::warn!(
                                "{}: graft bone '{target}' matches more than one bone in the base skeleton, using the first",
                                model.name
                            );
                        }
                        extra_box(&mut bones, si, b);
                        map.push(si);
                        continue;
                    }
                    if warned_grafts()
                        .lock()
                        .unwrap()
                        .insert(format!("missing:{}:{target}", model.name))
                    {
                        log::warn!(
                            "{}: graft bone '{target}' not in the base skeleton, attaching as a new root",
                            model.name
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
                    hit_mins: b.hit_mins,
                    hit_maxs: b.hit_maxs,
                    hit_location: b.hit_location,
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
        inherit_hit_locations(&mut bones);
        let mut index = HashMap::with_capacity(bones.len());
        for (i, b) in bones.iter().enumerate() {
            index.entry(b.name.clone()).or_insert(i);
        }
        Skeleton {
            bones,
            maps,
            inv_binds,
            index,
        }
    }

    /// Model `model`'s skin matrices for the skeleton posed as `worlds`
    /// (each bone's transform before any inverse bind, in skeleton order):
    /// `world * inverse_bind` in the model's own bone order.
    pub fn skin_matrices(&self, worlds: &[(Vec3, Quat)], model: usize) -> Vec<Mat4> {
        self.maps[model]
            .iter()
            .zip(&self.inv_binds[model])
            .map(|(&si, inv)| {
                let (p, r) = worlds[si];
                Mat4::from_rotation_translation(r, p) * *inv
            })
            .collect()
    }

    pub fn bone_count(&self) -> usize {
        self.bones.len()
    }

    pub fn bone_index(&self, name: &str) -> Option<usize> {
        self.index.get(name).copied()
    }

    pub fn bones(&self) -> &[SkelBone] {
        &self.bones
    }

    pub fn bind(&self, anim: &XAnim) -> AnimBinding {
        AnimBinding(
            anim.tracks
                .iter()
                .map(|t| self.bone_index(&t.bone))
                .collect(),
        )
    }
}

/// One clip of a blend: the clip, its bone binding, the frame to sample and
/// its weight.
pub type ClipAt<'a> = (&'a XAnim, &'a AnimBinding, f32, f32);

/// A child of an anim tree node in a blend: a clip at its weight, or a node
/// of its own at a weight over its children (an MG42 group over its rows, a
/// row over its leaves), which the walk normalizes to that weight before its
/// parent sums it (cod11-combat.md 3.1, "The pose on 1.5 and UO";
/// cod11-turrets.md 13.9).
#[derive(Clone)]
pub enum Blend<'a> {
    Clip(ClipAt<'a>),
    Node(f32, Vec<Blend<'a>>),
}

impl Blend<'_> {
    fn weight(&self) -> f32 {
        match self {
            Blend::Clip(c) => c.3,
            Blend::Node(w, _) => *w,
        }
    }
}

/// The sampler's per-bone sums over a node's `children` (0x080b8370):
/// rotation, offset and weight, each clip with a track for the bone adding
/// its keys times its weight, each child node its own sums' unit rotation
/// and mean offset times its weight.
fn node_sums(n: usize, children: &[Blend]) -> (Vec<glam::Vec4>, Vec<Vec3>, Vec<f32>) {
    let mut rot = vec![glam::Vec4::ZERO; n];
    let mut pos = vec![Vec3::ZERO; n];
    let mut weight = vec![0.0f32; n];
    for child in children {
        let w = child.weight();
        if w <= 0.0 {
            continue;
        }
        match child {
            &Blend::Clip((anim, binding, frame_pos, _)) => {
                for (track, slot) in anim.tracks.iter().zip(&binding.0) {
                    let Some(bi) = *slot else { continue };
                    let (p, q) = track.sample(frame_pos);
                    // Signed as the loader left the track (`xanim::parse`):
                    // the sampler adds the raw keys.
                    let q = glam::Vec4::from(q.unwrap_or(Quat::IDENTITY));
                    rot[bi] += q * w;
                    pos[bi] += p.unwrap_or(Vec3::ZERO) * w;
                    weight[bi] += w;
                }
            }
            Blend::Node(_, kids) => {
                let (r, p, k) = node_sums(n, kids);
                for bi in 0..n {
                    if k[bi] <= 0.0 || r[bi] == glam::Vec4::ZERO {
                        continue;
                    }
                    // Raw signed sums, no hemisphere alignment: two nodes on
                    // opposite hemispheres swing the long way round, as
                    // retail's reload blend-out does (cod11-movers.md 15,
                    // "Torso over legs").
                    rot[bi] += r[bi].normalize() * w;
                    pos[bi] += p[bi] / k[bi] * w;
                    weight[bi] += w;
                }
            }
        }
    }
    (rot, pos, weight)
}

/// Per-bone locals, starting at bind pose. `apply` overwrites only keyed
/// bones, so bones outside a clip hold their last pose (the fingers during
/// the `tag_torso`-only ADS clips).
pub struct PoseBuffer {
    locals: Vec<(Vec3, Quat)>,
    /// Translation keys are offsets from these.
    bind_pos: Vec<Vec3>,
    /// The same pose as the 1.5b and United Offensive engines hold it for
    /// their skeleton build ([`Record`]), kept by [`PoseBuffer::blend_nodes`]
    /// and the controllers; `None` once anything else has set a local.
    records: Option<Vec<Record>>,
}

impl PoseBuffer {
    pub fn new(skel: &Skeleton) -> PoseBuffer {
        PoseBuffer {
            records: Some(skel.bones.iter().map(Record::bind).collect()),
            ..PoseBuffer::drawn(skel)
        }
    }

    /// [`PoseBuffer::new`] without the 1.5/UO engine records: for a pose
    /// only drawn, never traced, whose blends then skip the records' walk.
    pub fn drawn(skel: &Skeleton) -> PoseBuffer {
        PoseBuffer {
            locals: skel
                .bones
                .iter()
                .map(|b| (b.local_pos, b.local_rot))
                .collect(),
            bind_pos: skel.bones.iter().map(|b| b.local_pos).collect(),
            records: None,
        }
    }

    /// The bone's current local rotation.
    pub fn local_rot(&self, bone: usize) -> Quat {
        self.locals[bone].1
    }

    /// The bone's current local translation and rotation.
    pub fn local(&self, bone: usize) -> (Vec3, Quat) {
        self.locals[bone]
    }

    /// Replaces the rotation outright. The aim layer's control bones are
    /// keyed by no clip, so composing onto the previous frame would accumulate.
    pub fn set_local_rot(&mut self, bone: usize, rot: Quat) {
        self.locals[bone].1 = rot;
        self.records = None;
    }

    /// Replaces the local translation and rotation outright, as a local tag
    /// does.
    pub fn set_local(&mut self, bone: usize, pos: Vec3, rot: Quat) {
        self.locals[bone] = (pos, rot);
        self.records = None;
    }

    /// A controller's local and the engine's [`Record`] for it together.
    pub fn set_control(&mut self, bone: usize, pos: Vec3, rot: Quat, record: Record) {
        self.locals[bone] = (pos, rot);
        if let Some(r) = self.records.as_mut() {
            r[bone] = record;
        }
    }

    /// `bone` as `other` holds it, local and record.
    pub fn copy_bone(&mut self, other: &PoseBuffer, bone: usize) {
        self.locals[bone] = other.locals[bone];
        match (self.records.as_mut(), other.records.as_ref()) {
            (Some(r), Some(o)) => r[bone] = o[bone],
            _ => self.records = None,
        }
    }

    /// The engine-side records, when every local came through a path that
    /// keeps them.
    pub fn records(&self) -> Option<&[Record]> {
        self.records.as_deref()
    }

    /// Rotation keys replace the local rotation; translation keys are offsets
    /// from the bind local, not absolutes. A track with no keys on a channel
    /// poses it at identity or the bind local, as the engine's sampler does
    /// ([`PoseBuffer::blend`]); a bone the clip has no track for keeps its
    /// local. docs/research/xanim-v14-format.md, "Sampling and the
    /// translation-key gotcha".
    pub fn apply(&mut self, anim: &XAnim, binding: &AnimBinding, frame_pos: f32) {
        self.apply_weighted(anim, binding, frame_pos, 1.0);
    }

    /// Several clips at once, as the engine's sampler sums them
    /// (`cod_lnxded` 0x080b8370): per bone a clip with a track adds its
    /// rotation (identity without keys) and its translation offset (none
    /// without keys) times its weight, and the bone takes the normalized
    /// rotation sum and the offset sum over the weight sum. Bones no clip
    /// tracks keep their local. docs/research/xanim-v14-format.md,
    /// "Rotation keys as the engine rebuilds them".
    pub fn blend(&mut self, layers: &[ClipAt]) {
        self.blend_nodes(&[(layers, 1.0)]);
    }

    /// Sibling tree nodes, each a set of clips at the node's weight, as the
    /// tree walk 0x080bbea8 blends them: per bone, each node that keys it
    /// adds its own blend ([`PoseBuffer::blend`]'s rule) as a unit rotation
    /// and an offset times the node's weight, and the bone takes the
    /// normalized sum and the offset sum over the weights of the nodes that
    /// key it. A bone one node alone keys takes that node's pose whatever its
    /// weight. cod11-movers.md 15, "Torso over legs".
    pub fn blend_nodes(&mut self, nodes: &[(&[ClipAt], f32)]) {
        let nodes: Vec<Blend> = nodes
            .iter()
            .map(|&(clips, w)| Blend::Node(w, clips.iter().map(|&c| Blend::Clip(c)).collect()))
            .collect();
        self.blend_tree(&nodes);
    }

    /// [`PoseBuffer::blend_nodes`] over the root's children as a tree, where
    /// a node may hold nodes of its own ([`Blend`]).
    pub fn blend_tree(&mut self, nodes: &[Blend]) {
        if let Some(r) = self.records.as_mut() {
            walk_records(r, nodes);
        }
        let n = self.locals.len();
        let (rot, pos, weight) = node_sums(n, nodes);
        for bi in 0..n {
            if weight[bi] > 0.0 {
                self.locals[bi] = (
                    self.bind_pos[bi] + pos[bi] / weight[bi],
                    Quat::from_vec4(rot[bi]).normalize(),
                );
            }
        }
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
            let target = self.bind_pos[bi] + p.unwrap_or(Vec3::ZERO);
            self.locals[bi].0 = self.locals[bi].0.lerp(target, weight);
            let q = q.unwrap_or(Quat::IDENTITY);
            self.locals[bi].1 = self.locals[bi].1.slerp(q, weight);
        }
        self.records = None;
    }

    /// Posed world transforms of every bone, before any inverse bind. Parents
    /// precede children by construction, so one pass composes.
    pub fn bone_worlds(&self, skel: &Skeleton) -> Vec<(Vec3, Quat)> {
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
    /// [`PoseBuffer::bone_worlds`]' entry for `bone`, composed down its
    /// parent chain alone, in the same order.
    pub fn bone_world(&self, skel: &Skeleton, bone: usize) -> (Vec3, Quat) {
        let mut chain = Vec::with_capacity(16);
        let mut b = bone as i32;
        while b >= 0 {
            chain.push(b as usize);
            b = skel.bones[b as usize].parent;
        }
        let mut world: Option<(Vec3, Quat)> = None;
        for &b in chain.iter().rev() {
            let (lp, lr) = self.locals[b];
            world = Some(match world {
                Some((pp, pr)) => (pp + pr * lp, pr * lr),
                None => (lp, lr),
            });
        }
        world.expect("the chain holds the bone itself")
    }

    /// `world(pose) * inverse_bind`, in the model's own bone order.
    pub fn skin_matrices(&self, skel: &Skeleton, model: usize) -> Vec<Mat4> {
        skel.skin_matrices(&self.bone_worlds(skel), model)
    }
}

/// A float variable the -O0 engines store.
fn st(x: f64) -> f32 {
    x as f32
}

/// `1 / 32767` as the engines keep it (UO `coduo_lnxded` 0x80ea1ac).
const SHORT_SCALE: f32 = 1.0 / 32767.0;

/// One bone's slot as the 1.5b and United Offensive engines leave it for
/// the skeleton build: the walk's rotation sum, unnormalized, in the keys'
/// 1/32767 units times the weights, the summed weight, the translation
/// offset sum, and whether a controller turns it in model space
/// (cod11-combat.md 3.1, "The pose on 1.5 and UO").
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Record {
    pub q: [f32; 4],
    pub weight: f32,
    pub t: [f32; 3],
    pub control: bool,
}

impl Record {
    const ZERO: Record = Record {
        q: [0.0; 4],
        weight: 0.0,
        t: [0.0; 3],
        control: false,
    };

    /// A bone no clip keys (UO 0x80bf2a8): a root at identity and no
    /// offset (an xmodel's roots carry no bind; a hand-built rig's keep
    /// theirs), any other bone at its bind rotation's shorts over 32767 and
    /// no offset from its bind position.
    fn bind(b: &SkelBone) -> Record {
        if b.parent < 0 {
            return Record {
                q: b.local_rot.to_array(),
                t: b.local_pos.to_array(),
                ..Record::ZERO
            };
        }
        Record {
            q: bind_raw(b.local_rot).map(|c| st(f64::from(SHORT_SCALE) * f64::from(c))),
            ..Record::ZERO
        }
    }

    /// A local tag or a control angle set (game.mp.uo `G_DObjSetLocalTag`,
    /// `G_DObjSetControlTagAngles`, both through 0x88cf6): roll times pitch
    /// times yaw, each half angle's sine and cosine stored, each product
    /// component stored, and the translation as given.
    pub fn tag(angles: [f32; 3], t: Vec3, control: bool) -> Record {
        // `(float)(pi / 360)` (game.mp.uo 0xadb30) times the angle, passed
        // as a float.
        let half = |a: f32| {
            let r = f64::from(st(
                f64::from(st(std::f64::consts::PI / 360.0)) * f64::from(a)
            ));
            (st(r.sin()), st(r.cos()))
        };
        let (ps, pc) = half(angles[0]);
        let (ys, yc) = half(angles[1]);
        let (rs, rc) = half(angles[2]);
        let pitch = [0.0, ps, 0.0, pc];
        let yaw = [0.0, 0.0, ys, yc];
        let roll = [rs, 0.0, 0.0, rc];
        Record {
            q: quat_multiply(roll, quat_multiply(pitch, yaw)),
            weight: 0.0,
            t: t.to_array(),
            control,
        }
    }
}

/// `QuatMultiply` (game.mp.uo 0x4d25f) of `a` and `b`, each component
/// stored.
fn quat_multiply(a: [f32; 4], b: [f32; 4]) -> [f32; 4] {
    let (a, b) = (a.map(f64::from), b.map(f64::from));
    [
        st((a[2] * b[1] + a[3] * b[0] + a[0] * b[3]) - a[1] * b[2]),
        st(a[0] * b[2] + a[3] * b[1] + (a[1] * b[3] - a[2] * b[0])),
        st(a[3] * b[2] + ((a[1] * b[0] + a[2] * b[3]) - a[0] * b[1])),
        st(((a[3] * b[3] - a[0] * b[0]) - a[1] * b[1]) - a[2] * b[2]),
    ]
}

/// A bind rotation's shorts: the xmodel's `x`, `y` and `z` over 32768
/// (`xmodel::parse_parts`), `w` rebuilt as the engines rebuild a key's.
fn bind_raw(q: Quat) -> [i16; 4] {
    let c = |v: f32| (v * 32768.0).round().clamp(-32768.0, 32767.0) as i16;
    crate::xanim::raw_key(c(q.x), c(q.y), c(q.z))
}

/// A track's key `k` as shorts: its own, or a hand-built track's rounded.
fn raw_key_at(track: &crate::xanim::Track, k: usize) -> [i16; 4] {
    if track.rot_raw.len() == track.rot_keys.len() {
        return track.rot_raw[k];
    }
    track.rot_keys[k]
        .1
        .to_array()
        .map(|c| (c * 32767.0).round() as i16)
}

/// The two keys a frame falls between and how far, stored, as UO's
/// sampler 0x80b9b72 finds them; the first or last key alone outside them.
fn key_span<T>(frames: &[(u16, T)], frame_pos: f32) -> Option<(usize, usize, f32)> {
    let (first, last) = (frames.first()?, frames.last()?);
    if frames.len() == 1 || frame_pos <= f32::from(first.0) {
        return Some((0, 0, 0.0));
    }
    if frame_pos >= f32::from(last.0) {
        let k = frames.len() - 1;
        return Some((k, k, 0.0));
    }
    let k = frames.partition_point(|(f, _)| f32::from(*f) <= frame_pos) - 1;
    let (f0, f1) = (f32::from(frames[k].0), f32::from(frames[k + 1].0));
    let frac = st((f64::from(frame_pos) - f64::from(f0)) / f64::from(f1 - f0));
    Some((k, k + 1, frac))
}

/// One clip summed into `dest` at `weight` (UO 0x80b9b72): each key's
/// shorts lerped and times `weight / 32767` (stored), each translation key
/// lerped and times `weight`, every sum stored, the weight added.
fn sample_records(
    dest: &mut [Record],
    anim: &XAnim,
    binding: &AnimBinding,
    frame_pos: f32,
    weight: f32,
) {
    let w = f64::from(weight);
    let scale = f64::from(st(f64::from(SHORT_SCALE) * w));
    for (track, slot) in anim.tracks.iter().zip(&binding.0) {
        let Some(bi) = *slot else { continue };
        let r = &mut dest[bi];
        match key_span(&track.rot_keys, frame_pos) {
            None => r.q[3] = st(f64::from(r.q[3]) + w),
            Some((a, b, frac)) => {
                let (k0, k1) = (raw_key_at(track, a), raw_key_at(track, b));
                for c in 0..4 {
                    let s0 = f64::from(k0[c]);
                    let v = if a == b {
                        s0
                    } else {
                        f64::from(i32::from(k1[c]) - i32::from(k0[c])) * f64::from(frac) + s0
                    };
                    r.q[c] = st(f64::from(r.q[c]) + v * scale);
                }
            }
        }
        if let Some((a, b, frac)) = key_span(&track.trans_keys, frame_pos) {
            let (v0, v1) = (track.trans_keys[a].1, track.trans_keys[b].1);
            for c in 0..3 {
                let v = if a == b {
                    f64::from(v0[c])
                } else {
                    (f64::from(v1[c]) - f64::from(v0[c])) * f64::from(frac) + f64::from(v0[c])
                };
                r.t[c] = st(f64::from(r.t[c]) + v * w);
            }
        }
        r.weight = st(f64::from(r.weight) + w);
    }
}

/// UO's tree walk 0x80bdbdc on `node` at `weight` into `dest`: `first`
/// clears it, `nested` keeps a node with two or more weighted children
/// from dividing by its weight sum. One weighted child passes the call
/// through at the same weight; two or more blend, the first into the
/// buffer and the rest added, and then a nested node normalizes the
/// rotation sum to its weight, scales the offsets to its weight over their
/// sum and (into `dest` when it had a buffer of its own) adds them, while
/// the root divides every sum by the summed weight.
fn walk(node: &Blend, weight: f32, dest: &mut [Record], first: bool, nested: bool) {
    let children = match node {
        &Blend::Clip((anim, binding, frame, _)) => {
            if first {
                dest.fill(Record::ZERO);
            }
            sample_records(dest, anim, binding, frame, weight);
            return;
        }
        Blend::Node(_, c) => c,
    };
    let active: Vec<&Blend> = children.iter().filter(|c| c.weight() > 0.0).collect();
    match active.as_slice() {
        [] => {
            if first {
                dest.fill(Record::ZERO);
            }
            return;
        }
        [only] => return walk(only, weight, dest, first, nested),
        _ => {}
    }
    let mut own = if first {
        None
    } else {
        Some(vec![Record::ZERO; dest.len()])
    };
    {
        let buf: &mut [Record] = match own.as_mut() {
            Some(b) => b,
            None => dest,
        };
        for (i, child) in active.iter().enumerate() {
            walk(child, child.weight(), buf, i == 0, true);
        }
    }
    let w = f64::from(weight);
    // `((q0² + q1²) + q2²) + q3²` on the x87 stack, stored once (UO
    // 0x80be0f3..0x80be121, 1.5b 0x80c8d5f..0x80c8d8d).
    let len = |q: &[f32; 4]| {
        let q = q.map(f64::from);
        st(q[0] * q[0] + q[1] * q[1] + q[2] * q[2] + q[3] * q[3])
    };
    match (nested, own) {
        (false, _) => {
            for r in dest.iter_mut().filter(|r| r.weight != 0.0) {
                let inv = f64::from(st(1.0 / f64::from(r.weight)));
                r.q = r.q.map(|c| st(f64::from(c) * inv));
                r.t = r.t.map(|c| st(f64::from(c) * inv));
            }
        }
        (true, Some(buf)) => {
            for (r, b) in dest.iter_mut().zip(&buf) {
                let n = len(&b.q);
                if n != 0.0 {
                    let s = f64::from(st(w / f64::from(st(f64::from(n).sqrt()))));
                    for c in 0..4 {
                        r.q[c] = st(f64::from(r.q[c]) + f64::from(b.q[c]) * s);
                    }
                }
                if b.weight != 0.0 {
                    let s = f64::from(st(w / f64::from(b.weight)));
                    r.weight = st(f64::from(r.weight) + w);
                    for c in 0..3 {
                        r.t[c] = st(f64::from(r.t[c]) + f64::from(b.t[c]) * s);
                    }
                }
            }
        }
        (true, None) => {
            for r in dest.iter_mut() {
                let n = len(&r.q);
                if n != 0.0 {
                    let s = f64::from(st(w / f64::from(st(f64::from(n).sqrt()))));
                    r.q = r.q.map(|c| st(f64::from(c) * s));
                }
                if r.weight != 0.0 {
                    let s = f64::from(st(w / f64::from(r.weight)));
                    r.weight = weight;
                    r.t = r.t.map(|c| st(f64::from(c) * s));
                }
            }
        }
    }
}

/// [`PoseBuffer::blend_tree`]'s nodes through UO's anim calc (0x80bf2a8):
/// the root's walk at weight 1 over the nodes and their clips, then every
/// bone no clip keys back at its bind record.
fn walk_records(records: &mut [Record], nodes: &[Blend]) {
    let bind: Vec<Record> = records.to_vec();
    let tree = Blend::Node(1.0, nodes.to_vec());
    // The bones a walked clip carries a track for (UO 0x80bab1c's mask).
    fn mark(node: &Blend, keyed: &mut [bool]) {
        match node {
            Blend::Clip((_, binding, _, _)) => {
                for bi in binding.0.iter().flatten() {
                    keyed[*bi] = true;
                }
            }
            Blend::Node(_, kids) => {
                for k in kids.iter().filter(|k| k.weight() > 0.0) {
                    mark(k, keyed);
                }
            }
        }
    }
    let mut keyed = vec![false; records.len()];
    mark(&tree, &mut keyed);
    walk(&tree, 1.0, records, true, false);
    for (i, r) in records.iter_mut().enumerate() {
        if !keyed[i] {
            *r = Record {
                control: false,
                ..bind[i]
            };
        }
    }
}

/// `p`'s quaternion times the child's `l` as UO's build composes a bone
/// (0x80c8056), each component stored.
fn compose(l: [f32; 4], p: [f32; 4]) -> [f32; 4] {
    let ([lx, ly, lz, lw], [px, py, pz, pw]) = (l.map(f64::from), p.map(f64::from));
    [
        st(((lz * py + lw * px) + lx * pw) - ly * pz),
        st((lx * pz + lw * py) + (ly * pw - lz * px)),
        st(lw * pz + ((ly * px + lz * pw) - lx * py)),
        st(((lw * pw - lx * px) - ly * py) - lz * pz),
    ]
}

/// A control bone's `c` times its parent's `p` in model space (UO
/// 0x80c818e), each component stored.
fn compose_control(p: [f32; 4], c: [f32; 4]) -> [f32; 4] {
    let ([px, py, pz, pw], [cx, cy, cz, cw]) = (p.map(f64::from), c.map(f64::from));
    [
        st((pz * cy + pw * cx + px * cw) - py * cz),
        st(px * cz + pw * cy + (py * cw - pz * cx)),
        st(pw * cz + ((py * cx + pz * cw) - px * cy)),
        st(((pw * cw - px * cx) - py * cy) - pz * cz),
    ]
}

/// A bone's axes off its quaternion as UO's build writes its matrix
/// (0x80c7d82): `2 / |q|^2` and every product stored, each element rounded
/// once.
pub fn axes_stored(q: [f32; 4]) -> [Vec3; 3] {
    let f = f64::from;
    let [x, y, z, w] = q;
    let n = st(f(st(f(x) * f(x))) + f(st(f(y) * f(y))) + f(st(f(z) * f(z))) + f(w) * f(w));
    if n == 0.0 {
        return [Vec3::X, Vec3::Y, Vec3::Z];
    }
    let s = f(st(2.0 / f(n)));
    let xx = f(st(f(st(f(x) * f(x))) * s));
    let yy = f(st(f(st(f(y) * f(y))) * s));
    let zz = f(st(f(st(f(z) * f(z))) * s));
    let xs = f(st(f(x) * s));
    let (xy, xz, xw) = (f(st(xs * f(y))), f(st(xs * f(z))), f(st(xs * f(w))));
    let ys = f(st(f(y) * s));
    let (yz, yw) = (f(st(ys * f(z))), f(st(ys * f(w))));
    let zw = f(st(f(z) * f(w) * s));
    [
        Vec3::new(st(1.0 - (yy + zz)), st(xy + zw), st(xz - yw)),
        Vec3::new(st(xy - zw), st(1.0 - (xx + zz)), st(yz + xw)),
        Vec3::new(st(xz + yw), st(yz - xw), st(1.0 - (xx + yy))),
    ]
}

/// One bone's world as UO's build leaves it: the quaternion, unnormalized,
/// its axes and its origin.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StoredBone {
    pub q: [f32; 4],
    pub axes: [Vec3; 3],
    pub origin: Vec3,
}

impl PoseBuffer {
    /// Every bone's world as the 1.5b and UO skeleton build (UO 0x80c5c96)
    /// composes the [`Record`]s: a root as recorded; any other bone's
    /// quaternion composed onto its parent's, a control bone's in model
    /// space, and its offset plus its bind position turned through the
    /// parent's stored matrix (0x80c7f9a). `None` when the records are not
    /// kept.
    pub fn stored_worlds(&self, skel: &Skeleton) -> Option<Vec<StoredBone>> {
        let records = self.records.as_ref()?;
        let mut out: Vec<StoredBone> = Vec::with_capacity(records.len());
        for (b, r) in skel.bones.iter().zip(records) {
            let bone = match usize::try_from(b.parent) {
                Err(_) => StoredBone {
                    q: r.q,
                    axes: axes_stored(r.q),
                    origin: Vec3::from(r.t),
                },
                Ok(p) => {
                    let pb = out[p];
                    let q = if r.control {
                        compose_control(pb.q, r.q)
                    } else {
                        compose(r.q, pb.q)
                    };
                    let t = [0, 1, 2]
                        .map(|c| f64::from(st(f64::from(r.t[c]) + f64::from(b.local_pos[c]))));
                    let [m0, m1, m2] = pb.axes.map(|a| a.as_dvec3());
                    let o = pb.origin.as_dvec3();
                    let origin = Vec3::new(
                        st(((t[2] * m2.x + t[1] * m1.x) + t[0] * m0.x) + o.x),
                        st(((t[2] * m2.y + t[1] * m1.y) + t[0] * m0.y) + o.y),
                        st(((t[2] * m2.z + t[1] * m1.z) + t[0] * m0.z) + o.z),
                    );
                    StoredBone {
                        q,
                        axes: axes_stored(q),
                        origin,
                    }
                }
            };
            out.push(bone);
        }
        Some(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Bone, Model};
    use crate::xanim::{Track, XAnim};
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

    fn model(bones: Vec<Bone>) -> Model {
        Model::rig("t", bones)
    }

    /// hands-like: a > tag_weapon; gun-like: tag_weapon > c.
    fn two_model_skel() -> (Model, Model) {
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
            root: None,
            tracks: vec![Track {
                bone: bone.into(),
                rot_keys: vec![(0, rot)],
                trans_keys: vec![(0, pos)],
                rot_raw: vec![],
            }],
        }
    }

    /// `bone` plus a hit box and a hit-location code.
    fn hit_bone(
        name: &str,
        parent: i32,
        local_pos: Vec3,
        world: &[Bone],
        box_half: f32,
        hit_location: u8,
    ) -> Bone {
        let mut b = bone(name, parent, local_pos, Quat::IDENTITY, world);
        b.hit_mins = Vec3::splat(-box_half);
        b.hit_maxs = Vec3::splat(box_half);
        b.hit_location = hit_location;
        b
    }

    /// The player shape: the body's head/neck/helmet boxes are zero and the
    /// real ones ride in the attachments, and a bone whose code is 0 inherits
    /// the one above it.
    #[test]
    fn graft_carries_hit_boxes_and_inherits_zero_codes() {
        let mut bb = vec![bone("tag_origin", -1, Vec3::ZERO, Quat::IDENTITY, &[])];
        for (name, parent, half, code) in [
            ("spine", 0i32, 5.0f32, 4u8),
            ("neck", 1, 0.0, 0),
            ("head", 2, 0.0, 0),
            ("tag_helmet", 3, 0.0, 0),
        ] {
            let b = hit_bone(name, parent, Vec3::Z, &bb, half, code);
            bb.push(b);
        }
        let base = model(bb);

        // head attachment: its own neck/head copies carry the real boxes
        let mut hb = vec![hit_bone("neck", -1, Vec3::ZERO, &[], 3.0, 3)];
        for (name, parent, code) in [("head", 0i32, 2u8), ("jaw", 1, 0)] {
            let b = hit_bone(name, parent, Vec3::Z, &hb, 3.0, code);
            hb.push(b);
        }
        let head = model(hb);
        let helmet = model(vec![hit_bone("lid", -1, Vec3::ZERO, &[], 4.0, 1)]);

        let skel =
            Skeleton::build_grafted(&[(&base, None), (&head, None), (&helmet, Some("tag_helmet"))]);
        let of = |name: &str| {
            let b = &skel.bones()[skel.bone_index(name).unwrap()];
            (b.has_hit_box(), b.hit_location)
        };
        assert_eq!(of("tag_origin"), (false, 0));
        assert_eq!(of("spine"), (true, 4));
        // merged: the attachment's box and code land on the body's bone
        assert_eq!(of("neck"), (true, 3));
        assert_eq!(of("head"), (true, 2));
        // grafted at a tag: the helmet's root box lands on tag_helmet
        assert_eq!(of("tag_helmet"), (true, 1));
        // a new bone with code 0 inherits its parent's
        assert_eq!(of("jaw"), (true, 2));
        assert!(skel.bone_index("lid").is_none());
    }

    /// The helmet's root merges onto the `head` the head model already gave
    /// a box: both boxes stay, the helmet's on a box-only child that rides
    /// the head and no clip binds.
    #[test]
    fn a_second_box_on_a_merged_bone_is_kept() {
        let mut bb = vec![bone("tag_origin", -1, Vec3::ZERO, Quat::IDENTITY, &[])];
        let b = hit_bone("head", 0, Vec3::Z, &bb, 0.0, 0);
        bb.push(b);
        let base = model(bb);
        let head = model(vec![hit_bone("head", -1, Vec3::ZERO, &[], 3.0, 2)]);
        let helmet = model(vec![hit_bone("head", -1, Vec3::ZERO, &[], 4.0, 2)]);
        let skel = Skeleton::build(&[&base, &head, &helmet]);
        let heads: Vec<(usize, &SkelBone)> = skel
            .bones()
            .iter()
            .enumerate()
            .filter(|(_, b)| b.name == "head")
            .collect();
        assert_eq!(heads.len(), 2);
        let ((first, a), (_, b)) = (heads[0], heads[1]);
        assert_eq!((a.hit_maxs.x, a.hit_location), (3.0, 2));
        assert_eq!((b.hit_maxs.x, b.hit_location), (4.0, 2));
        assert_eq!(b.parent, first as i32);
        assert_eq!((b.local_pos, b.local_rot), (Vec3::ZERO, Quat::IDENTITY));
        let anim = one_track_anim("head", Vec3::X, Quat::IDENTITY);
        assert_eq!(skel.bind(&anim).0, vec![Some(first)]);
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

    /// A track with an empty channel poses it at identity or bind, as the
    /// engine sums it: a turn an earlier clip left on the bone does not stay.
    #[test]
    fn an_empty_channel_resets_its_bone() {
        let (h, g) = two_model_skel();
        let skel = Skeleton::build(&[&h, &g]);
        let turned = Quat::from_rotation_z(0.3);
        let turn = one_track_anim("tag_weapon", Vec3::new(0.0, 0.0, 5.0), turned);
        let mut bare = one_track_anim("tag_weapon", Vec3::ZERO, Quat::IDENTITY);
        bare.tracks[0].rot_keys.clear();
        bare.tracks[0].trans_keys.clear();
        let mut pose = PoseBuffer::new(&skel);
        pose.apply(&turn, &skel.bind(&turn), 0.0);
        pose.apply(&bare, &skel.bind(&bare), 0.0);
        let (p, r) = pose.local(1);
        assert!(p.abs_diff_eq(Vec3::X, 1e-5), "{p}");
        assert!(r.abs_diff_eq(Quat::IDENTITY, 1e-5), "{r}");
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

    #[test]
    fn blend_sums_clips_and_an_unkeyed_offset_counts_as_none() {
        let (h, g) = two_model_skel();
        let skel = Skeleton::build(&[&h, &g]);
        let turned = Quat::from_rotation_z(std::f32::consts::FRAC_PI_2);
        let a = one_track_anim("tag_weapon", Vec3::new(0.0, 0.0, 4.0), Quat::IDENTITY);
        let mut b = one_track_anim("tag_weapon", Vec3::ZERO, turned);
        b.tracks[0].trans_keys.clear();
        let (ba, bb) = (skel.bind(&a), skel.bind(&b));
        let mut pose = PoseBuffer::new(&skel);
        pose.blend(&[(&a, &ba, 0.0, 0.75), (&b, &bb, 0.0, 0.25)]);
        let (p, r) = pose.bone_world(&skel, 1);
        // Offset 4 at weight 0.75 over a total of 1; bind X stays.
        assert!(p.abs_diff_eq(Vec3::new(1.0, 0.0, 3.0), 1e-4), "{p}");
        // The normalized sum, not a slerp at 0.25: atan(0.25 / 0.75) on the
        // half angle.
        let want = Quat::from_rotation_z(
            2.0 * (0.25 * (0.5f32).sqrt()).atan2(0.75 + 0.25 * (0.5f32).sqrt()),
        );
        assert!(r.abs_diff_eq(want, 1e-4), "{r} {want}");
    }

    #[test]
    fn a_bone_one_node_keys_takes_that_nodes_pose_whatever_its_weight() {
        let skel = two_bone_chain();
        let turned = Quat::from_rotation_z(std::f32::consts::FRAC_PI_2);
        // The legs clip keys both bones; the torso clip only the child.
        let mut legs = one_track_anim("root", Vec3::new(0.0, 0.0, 2.0), turned);
        legs.tracks.push(Track {
            bone: "child".into(),
            rot_keys: vec![(0, Quat::IDENTITY)],
            trans_keys: vec![],
            rot_raw: vec![],
        });
        let torso = one_track_anim("child", Vec3::ZERO, turned);
        let (bl, bt) = (skel.bind(&legs), skel.bind(&torso));
        let mut pose = PoseBuffer::new(&skel);
        pose.blend_nodes(&[
            (&[(&legs, &bl, 0.0, 1.0)], 0.01),
            (&[(&torso, &bt, 0.0, 0.4)], 1.0),
        ]);
        // The root is the legs' alone, at the legs node's 0.01.
        let (p, r) = pose.local(0);
        assert!(p.abs_diff_eq(Vec3::new(0.0, 0.0, 2.0), 1e-5), "{p}");
        assert!(r.abs_diff_eq(turned, 1e-5), "{r}");
        // The child sums the torso's clip, full at its node's 1 whatever the
        // clip's own 0.4, with the legs' at 0.01.
        let want = (turned * 1.0 + Quat::IDENTITY * 0.01).normalize();
        assert!(pose.local(1).1.abs_diff_eq(want, 1e-5));
    }

    /// A drawn pose blends to the same locals bit for bit without the
    /// records, and one bone's world off its chain is `bone_worlds`' entry.
    #[test]
    fn a_drawn_pose_matches_the_recorded_one() {
        let skel = two_bone_chain();
        let turned = Quat::from_rotation_z(0.7);
        let legs = one_track_anim("root", Vec3::new(1.0, 0.5, 2.0), turned);
        let torso = one_track_anim("child", Vec3::new(0.0, 3.0, 0.0), turned.inverse());
        let (bl, bt) = (skel.bind(&legs), skel.bind(&torso));
        let blend = |mut pose: PoseBuffer| {
            pose.blend_nodes(&[
                (&[(&legs, &bl, 0.0, 1.0)], 0.3),
                (&[(&torso, &bt, 0.0, 0.4)], 1.0),
            ]);
            pose
        };
        let (kept, drawn) = (
            blend(PoseBuffer::new(&skel)),
            blend(PoseBuffer::drawn(&skel)),
        );
        assert!(kept.records().is_some() && drawn.records().is_none());
        let worlds = drawn.bone_worlds(&skel);
        assert_eq!(kept.bone_worlds(&skel), worlds);
        for (b, w) in worlds.iter().enumerate() {
            assert_eq!(drawn.bone_world(&skel, b), *w);
        }
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
        let Some(fs) = crate::testing::game_fs_11() else {
            return;
        };
        // The 1.5 paks repackaged the view anims; the pose bind needs one.
        if fs.read("xanim/viewmodel_kar98mp_idle").is_none() {
            return;
        };
        let hands = crate::model::load(&fs, "viewmodel_hands_new").unwrap();
        let gun = crate::model::load(&fs, "viewmodel_kar98k").unwrap();
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
                    m.name,
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
        let body = crate::model::load(&fs, "playerbody_american_airborne").unwrap();
        let weapon = crate::model::load(&fs, "weapon_kar98").unwrap();
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
        let body = crate::model::load(&fs, "playerbody_american_airborne").unwrap();
        let head = crate::model::load(&fs, "basehead2").unwrap();
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
            let body = crate::model::load(&fs, body_name).unwrap();
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

    /// The 1.5/UO records: one clip alone leaves its key's shorts over
    /// 32767, unnormalized; two nodes leave each node's sum at its weight,
    /// over the weights' sum; the build composes the child onto its parent
    /// and turns its bind offset through the parent's matrix.
    #[test]
    fn the_engine_records_keep_the_shorts_unnormalized() {
        let skel = two_bone_chain();
        let mut clip = one_track_anim("child", Vec3::ZERO, Quat::IDENTITY);
        clip.tracks[0].rot_raw = vec![[0, 0, 23170, 23170]];
        let b = skel.bind(&clip);
        let mut pose = PoseBuffer::new(&skel);
        pose.blend_nodes(&[(&[(&clip, &b, 0.0, 1.0)], 1.0)]);
        let r = pose.records().unwrap()[1];
        let s = 23170.0f32 * (1.0 / 32767.0);
        assert_eq!(r.q, [0.0, 0.0, s, s]);
        assert_ne!(Quat::from_array(r.q).length(), 1.0);
        let w = pose.stored_worlds(&skel).unwrap();
        // A quarter turn about z off a root at identity: the child sits 10
        // up y, its x axis along y to the matrix's rounding.
        assert_eq!(w[1].origin, Vec3::new(0.0, 10.0, 0.0));
        assert!((w[1].axes[0] - Vec3::Y).length() < 1e-6, "{:?}", w[1].axes);

        let mut half = one_track_anim("child", Vec3::ZERO, Quat::IDENTITY);
        half.tracks[0].rot_raw = vec![[0, 0, 0, 32767]];
        let hb = skel.bind(&half);
        let mut pose = PoseBuffer::new(&skel);
        pose.blend_nodes(&[
            (&[(&clip, &b, 0.0, 1.0)], 0.25),
            (&[(&half, &hb, 0.0, 1.0)], 0.75),
        ]);
        let r = pose.records().unwrap()[1];
        assert_eq!(r.weight, 1.0);
        assert!(
            (r.q[2] - 0.25 * s).abs() < 1e-7 && (r.q[3] - (0.25 * s + 0.75)).abs() < 1e-6,
            "{r:?}"
        );
    }
}
