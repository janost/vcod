//! Snapshot entities to drawables, plus the per-entity legs/torso anim driver.
//! docs/research/clientstate-wire-format.md, docs/research/player-model-anim-system.md.

use crate::camera;
use crate::renderer::{DynamicModelInstance, ModelHandle, Renderer};
use glam::{Mat4, Quat, Vec3};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::rc::Rc;
use vcod_common::animtree::{AnimTree, PlayerAnims};
use vcod_common::bsp::Bsp;
use vcod_common::net::msg::{ClientState, EntityState};
use vcod_common::net::protocol::{Protocol, CS_MODELS_V1, CS_TAGS_V1};
use vcod_common::net::snapshot::Snapshot;
use vcod_common::net::trajectory::{Trajectory, TR_INTERPOLATE, TR_STATIONARY};
use vcod_common::pk3::Pk3Fs;
use vcod_common::skeleton::{AnimBinding, PoseBuffer, Skeleton};
use vcod_common::xanim::{self, XAnim};
use vcod_common::xmodel::{self, XModel};

/// `legsAnim`/`torsoAnim`: anim index in the low 9 bits, restart toggle in bit 512.
const ANIM_INDEX_MASK: i32 = 511;

/// Cross-fade length when a channel switches clips. Retail blends animtree
/// nodes per-transition; one flat engine-scale value covers the visible cases
/// (stance and movement changes).
const ANIM_BLEND_MS: i32 = 200;

/// How long an entity's anim state survives unseen. Long enough to ride out PVS
/// churn, short enough that a player who left drops.
const STATE_TTL_MS: i32 = 5000;

/// New xmodel load+upload pairs per [`build_instances`] call. One costs
/// 0.2-7ms, so several new loadouts in one frame would stall it by their sum.
const ASSEMBLY_LOAD_BUDGET_PER_FRAME: u32 = 1;

pub const ET_GENERAL: i32 = 0;
pub const ET_PLAYER: i32 = 1;
pub const ET_CORPSE: i32 = 2;
pub const ET_ITEM: i32 = 3;
pub const ET_MISSILE: i32 = 4;
pub const ET_MOVER: i32 = 5;
#[cfg_attr(not(test), allow(dead_code))] // only tests name it
pub const ET_PORTAL: i32 = 6;
#[cfg_attr(not(test), allow(dead_code))] // only tests name it
pub const ET_INVISIBLE: i32 = 7;
pub const ET_SCRIPTMOVER: i32 = 8;
/// 12, not Q3's 13 (CoDExtended shared.h:445).
#[cfg_attr(not(test), allow(dead_code))] // only tests name it
pub const ET_EVENTS: i32 = 12;

/// What one snapshot entity draws as.
#[derive(Debug, Clone, PartialEq)]
pub enum EntityVisual {
    /// Body xmodel name plus up to 6 (model name, tag name) attachments.
    Player {
        body: String,
        attachments: Vec<(String, Option<String>)>,
    },
    /// A single unskinned xmodel.
    Model(String),
    /// Dropped weapon: the raw `index`, 1-based into configstring 7. Resolved
    /// to a `worldModel` in `build_instances`, which has `fs`.
    Item(usize),
    /// Inline BSP submodel (`"*N"` configstring).
    Submodel(usize),
    None,
}

/// Strip the `xmodel/` (or backslashed) prefix off a configstring model path.
fn model_name(cs: &str) -> Option<String> {
    let cs = cs.replace('\\', "/");
    let name = cs.strip_prefix("xmodel/").unwrap_or(&cs);
    (!name.is_empty()).then(|| name.to_string())
}

/// Splits configstring 7 into weapon file names. 1-based: `index` names
/// `list[index - 1]`. Empty tokens are dropped so they never shift the numbering.
/// `pub(crate)` for `hud::Hud::weapon_kill_icon`.
pub(crate) fn split_weapon_list(list: &str) -> Vec<String> {
    list.split(' ')
        .filter(|s| !s.is_empty())
        .map(String::from)
        .collect()
}

pub fn resolve_visual(
    ent: &EntityState,
    clients: &BTreeMap<u32, ClientState>,
    configstrings: &[String],
    p: &Protocol,
) -> EntityVisual {
    let cs = |i: usize| configstrings.get(i).map(String::as_str).unwrap_or("");
    let etype = ent.field_i32(p, "eType");
    match etype {
        ET_PLAYER | ET_CORPSE => {
            let cn = ent.field_i32(p, "clientNum") as u32;
            let Some(client) = clients.get(&cn) else {
                return EntityVisual::None;
            };
            let mi = client.field_i32(p, "modelindex");
            let Some(body) = (mi > 0)
                .then(|| model_name(cs(CS_MODELS_V1 + mi as usize)))
                .flatten()
            else {
                return EntityVisual::None; // spectators have no body
            };
            let mut attachments = Vec::new();
            for i in 0..6 {
                let am = client.field_i32(p, &format!("attachModelIndex[{i}]"));
                if am <= 0 {
                    continue;
                }
                let Some(m) = model_name(cs(CS_MODELS_V1 + am as usize)) else {
                    continue;
                };
                let at = client.field_i32(p, &format!("attachTagIndex[{i}]"));
                let tag = (at > 0)
                    .then(|| cs(CS_TAGS_V1 + at as usize))
                    .filter(|t| !t.is_empty())
                    .map(String::from);
                attachments.push((m, tag));
            }
            EntityVisual::Player { body, attachments }
        }
        ET_GENERAL | ET_MISSILE | ET_SCRIPTMOVER | ET_MOVER => {
            let mi = ent.field_i32(p, "index");
            if mi <= 0 {
                return EntityVisual::None;
            }
            let s = cs(CS_MODELS_V1 + mi as usize);
            if let Some(sub) = s.strip_prefix('*').and_then(|n| n.parse::<usize>().ok()) {
                return EntityVisual::Submodel(sub);
            }
            match model_name(s) {
                Some(m) => EntityVisual::Model(m),
                None => EntityVisual::None,
            }
        }
        ET_ITEM => {
            let mi = ent.field_i32(p, "index");
            if mi <= 0 {
                return EntityVisual::None;
            }
            EntityVisual::Item(mi as usize)
        }
        _ => EntityVisual::None, // portal, invisible, turret base, events
    }
}

/// One assembled, GPU-resident player model set, shared by loadout.
pub struct Assembly {
    pub handles: Vec<ModelHandle>, // parallel to the models given to the skeleton
    pub skeleton: Rc<Skeleton>,
    #[allow(dead_code)] // handles.len() is what the draw path uses
    pub model_count: usize,
}

/// Loaded+uploaded player parts by xmodel name, shared across assemblies.
/// Holds the `XModel` too, because `Skeleton::build_grafted` re-reads bone
/// data for every assembly combination. `None` is a failed part, never retried.
type PartCache = HashMap<String, Option<(Rc<XModel>, ModelHandle)>>;

/// One attachment's model, graft tag and GPU handle, kept together so a
/// dropped attachment never desyncs `Assembly::handles` from the model list.
type AttachmentPart = (Rc<XModel>, Option<String>, ModelHandle);

/// One assembly's load progress, advanced one part per [`AssemblyCache::resolve`]
/// call. A slot is `None` until attempted, `Some(None)` once attempted and failed.
struct PendingAssembly {
    body: String,
    attachments: Vec<(String, Option<String>)>,
    body_part: Option<Option<(Rc<XModel>, ModelHandle)>>,
    attachment_parts: Vec<Option<Option<AttachmentPart>>>,
    /// Render-clock time `resolve` last touched this; `prune_stale` sweeps on it.
    last_touched_ms: i32,
}

/// Next part to load. A failed body dooms the assembly, so `Done` follows it
/// without spending budget on the attachments.
enum NextPart {
    Body,
    Attachment(usize),
    Done,
}

impl PendingAssembly {
    fn new(body: String, attachments: Vec<(String, Option<String>)>, now_ms: i32) -> Self {
        let attachment_parts = attachments.iter().map(|_| None).collect();
        PendingAssembly {
            body,
            attachments,
            body_part: None,
            attachment_parts,
            last_touched_ms: now_ms,
        }
    }

    fn next_part(&self) -> NextPart {
        match &self.body_part {
            None => return NextPart::Body,
            Some(None) => return NextPart::Done, // body failed: dead assembly
            Some(Some(_)) => {}
        }
        match self.attachment_parts.iter().position(Option::is_none) {
            Some(i) => NextPart::Attachment(i),
            None => NextPart::Done,
        }
    }

    fn is_complete(&self) -> bool {
        match &self.body_part {
            None => false,
            Some(None) => true,
            Some(Some(_)) => self.attachment_parts.iter().all(Option::is_some),
        }
    }
}

/// Assembled player models by (body, attachments) key, so entities sharing a
/// loadout share GPU uploads and a skeleton. `None` is a body that failed to load.
pub struct AssemblyCache {
    complete: HashMap<Vec<String>, Option<Rc<Assembly>>>,
    pending: HashMap<Vec<String>, PendingAssembly>,
}

impl AssemblyCache {
    pub fn new() -> Self {
        AssemblyCache {
            complete: HashMap::new(),
            pending: HashMap::new(),
        }
    }

    /// Body name, then "name@tag" (or the bare name) per attachment, in slot order.
    pub fn key(body: &str, attachments: &[(String, Option<String>)]) -> Vec<String> {
        let mut key = Vec::with_capacity(1 + attachments.len());
        key.push(body.to_string());
        key.extend(attachments.iter().map(|(name, tag)| match tag {
            Some(t) => format!("{name}@{t}"),
            None => name.clone(),
        }));
        key
    }

    /// The cached assembly for `key`, or `None` while parts are outstanding.
    /// A part already in `parts` is adopted for free; a new one costs one unit
    /// of `budget`, and a spent budget ends the call.
    #[allow(clippy::too_many_arguments)] // one call site, all context params
    pub fn resolve(
        &mut self,
        key: &[String],
        body: &str,
        attachments: &[(String, Option<String>)],
        fs: &Pk3Fs,
        renderer: &mut Renderer,
        parts: &mut PartCache,
        budget: &mut u32,
        now_ms: i32,
    ) -> Option<Rc<Assembly>> {
        if let Some(cached) = self.complete.get(key) {
            return cached.clone();
        }

        let pending = self.pending.entry(key.to_vec()).or_insert_with(|| {
            PendingAssembly::new(body.to_string(), attachments.to_vec(), now_ms)
        });
        pending.last_touched_ms = now_ms;

        loop {
            let (name, is_body, attach_idx) = match pending.next_part() {
                NextPart::Body => (pending.body.clone(), true, 0),
                NextPart::Attachment(i) => (pending.attachments[i].0.clone(), false, i),
                NextPart::Done => break,
            };
            let outcome = if let Some(cached) = parts.get(&name) {
                cached.clone()
            } else {
                if *budget == 0 {
                    break; // out of budget
                }
                let outcome = load_part(parts, fs, renderer, &name);
                *budget -= 1;
                outcome
            };
            if is_body {
                pending.body_part = Some(outcome);
            } else {
                let tag = pending.attachments[attach_idx].1.clone();
                pending.attachment_parts[attach_idx] = Some(outcome.map(|(m, h)| (m, tag, h)));
            }
        }

        if !pending.is_complete() {
            return None; // still loading
        }

        let pending = self.pending.remove(key).expect("just checked complete");
        let assembly = finalize_assembly(pending);
        self.complete.insert(key.to_vec(), assembly.clone());
        assembly
    }

    /// Drops pending assemblies untouched for [`STATE_TTL_MS`]: the key changed
    /// under them (weapon switch mid-load) or the entity left. Call once per frame.
    pub fn prune_stale(&mut self, now_ms: i32) {
        self.pending
            .retain(|_, p| (now_ms - p.last_touched_ms).abs() < STATE_TTL_MS);
    }
}

impl Default for AssemblyCache {
    fn default() -> Self {
        Self::new()
    }
}

/// Loads+uploads one part and caches the outcome in `parts`. `None` on failure,
/// warned once here.
fn load_part(
    parts: &mut PartCache,
    fs: &Pk3Fs,
    renderer: &mut Renderer,
    name: &str,
) -> Option<(Rc<XModel>, ModelHandle)> {
    let outcome = match xmodel::load(fs, name) {
        Ok(m) => match renderer.upload_dynamic_model(fs, &m) {
            Some(handle) => Some((Rc::new(m), handle)),
            None => {
                log::warn!("player part '{name}' uploaded no geometry, drawing without it");
                None
            }
        },
        Err(e) => {
            log::warn!("player part '{name}': {e:#}, drawing without it");
            None
        }
    };
    parts.insert(name.to_string(), outcome.clone());
    outcome
}

/// A failed body fails the assembly; a failed attachment is absent.
/// `handles` stays parallel to the models given to `build_grafted`.
fn finalize_assembly(pending: PendingAssembly) -> Option<Rc<Assembly>> {
    let (body_model, body_handle) = pending.body_part.expect("is_complete checked body_part")?;

    let parts: Vec<AttachmentPart> = pending
        .attachment_parts
        .into_iter()
        .filter_map(|slot| slot.expect("is_complete checked every attachment slot"))
        .collect();

    let mut refs: Vec<(&XModel, Option<&str>)> = Vec::with_capacity(1 + parts.len());
    refs.push((body_model.as_ref(), None));
    refs.extend(parts.iter().map(|(m, t, _)| (m.as_ref(), t.as_deref())));
    let skeleton = Rc::new(Skeleton::build_grafted(&refs));

    let mut handles = Vec::with_capacity(1 + parts.len());
    handles.push(body_handle);
    handles.extend(parts.iter().map(|(_, _, h)| *h));

    Some(Rc::new(Assembly {
        model_count: handles.len(),
        handles,
        skeleton,
    }))
}

/// One anim channel (legs or torso). `raw` is the last wire value; the server
/// flips the toggle bit on every anim (re)start, the only signal that a repeat
/// of the same anim should restart from frame 0.
#[derive(Clone, Copy)]
struct Channel {
    raw: i32,
    /// Render-clock time the current playback started at.
    start_ms: i32,
    /// The clip playing before the last switch, kept for the cross-fade:
    /// `(raw, start_ms)`. `None` once the fade is over, on a same-clip
    /// restart (re-fires stay snappy), and before the first anim.
    prev: Option<(i32, i32)>,
}

impl Channel {
    /// -1 cannot occur on the 10-bit field, so the first update counts as a restart.
    fn new() -> Channel {
        Channel {
            raw: -1,
            start_ms: 0,
            prev: None,
        }
    }

    fn index(&self) -> i32 {
        self.raw & ANIM_INDEX_MASK
    }

    /// Returns whether playback restarted.
    fn update(&mut self, raw: i32, now_ms: i32) -> bool {
        if self.raw == raw {
            return false; // same anim, same toggle: keep the phase
        }
        let switched_clip = self.raw >= 0 && (self.raw ^ raw) & ANIM_INDEX_MASK != 0;
        self.prev = switched_clip.then_some((self.raw, self.start_ms));
        self.raw = raw;
        self.start_ms = now_ms;
        true
    }
}

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
fn descend_aim(tree: &AnimTree, node: usize, pitch_deg: f32, yaw_deg: f32) -> usize {
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

/// Per-connection draw-path state: caches, the player animtree, clips, and one
/// [`EntityAnim`] per animating entity.
pub struct EntityScene {
    assemblies: AssemblyCache,
    parts: PartCache,
    /// World-entity models by xmodel name. `None` is a failed load, warned once.
    model_cache: HashMap<String, Option<ModelHandle>>,
    /// Weapon defs by CS 7 name. `None` is a failed load, warned once.
    weapon_cache: HashMap<String, Option<vcod_common::weapon::WeaponDef>>,
    /// Uploaded inline BSP submodels by index. `None` is a collision-only
    /// submodel, the common case; it must not be re-extracted every frame.
    submodel_cache: HashMap<usize, Option<ModelHandle>>,
    /// `ET_ITEM`/held-weapon failure reasons already logged, keyed by kind
    /// and index or name.
    warned_items: HashSet<String>,
    /// `None` until the first frame with entities; `Some(None)` once the load
    /// failed, so it warns once and every player draws in bind pose.
    anims: Option<Option<PlayerAnims>>,
    /// Player clips by tree-node name. `None` is a failed load (mods can name
    /// anims their pk3s lack).
    clips: HashMap<String, Option<Rc<XAnim>>>,
    states: HashMap<u32, EntityAnim>,
    pub stats: SceneStats,
}

/// Debug-overlay counters. `anim_restarts` is cumulative; `pending_assemblies`
/// is a snapshot of the last pass.
#[derive(Default)]
pub struct SceneStats {
    /// Channel (re)starts across all entities, legs and torso.
    pub anim_restarts: u64,
    pub pending_assemblies: usize,
}

impl EntityScene {
    pub fn new() -> EntityScene {
        EntityScene {
            assemblies: AssemblyCache::new(),
            parts: HashMap::new(),
            model_cache: HashMap::new(),
            weapon_cache: HashMap::new(),
            submodel_cache: HashMap::new(),
            warned_items: HashSet::new(),
            anims: None,
            clips: HashMap::new(),
            states: HashMap::new(),
            stats: SceneStats::default(),
        }
    }
}

impl Default for EntityScene {
    fn default() -> Self {
        Self::new()
    }
}

/// One entity's playback state, dropped once unseen for [`STATE_TTL_MS`].
struct EntityAnim {
    /// Assembly this pose was sized for; a change rebuilds against the new skeleton.
    key: Vec<String>,
    pose: PoseBuffer,
    legs: Channel,
    torso: Channel,
    /// `Skeleton::bind` per clip name. Small enough to keep per entity.
    bindings: HashMap<String, AnimBinding>,
    last_seen_ms: i32,
    /// Roster-resolved visual from the last frame this entity resolved (body
    /// plus attachments, no held weapon). A corpse re-resolves through the
    /// dead client's live roster entry, which clears when they drop to limbo;
    /// the corpse then draws this instead of vanishing.
    visual: EntityVisual,
}

impl EntityAnim {
    fn new(key: Vec<String>, skel: &Skeleton, now_ms: i32) -> EntityAnim {
        EntityAnim {
            key,
            pose: PoseBuffer::new(skel),
            legs: Channel::new(),
            torso: Channel::new(),
            bindings: HashMap::new(),
            last_seen_ms: now_ms,
            visual: EntityVisual::None,
        }
    }
}

/// Loads+uploads an xmodel by name (no `xmodel/` prefix), caching failures as `None`.
fn resolve_model(
    cache: &mut HashMap<String, Option<ModelHandle>>,
    renderer: &mut Renderer,
    fs: &Pk3Fs,
    name: &str,
) -> Option<ModelHandle> {
    if let Some(h) = cache.get(name) {
        return *h;
    }
    let handle = match xmodel::load(fs, name) {
        Ok(m) => renderer.upload_dynamic_model(fs, &m),
        Err(e) => {
            log::warn!("live entity model '{name}': {e:#}, drawing nothing for it");
            None
        }
    };
    cache.insert(name.to_string(), handle);
    handle
}

/// Uploads inline BSP submodel `n`, caching the result. Nothing on stock 1.1 MP
/// exercises this (docs/research/clientstate-wire-format.md, "ET_MOVER and
/// inline BSP submodels"). `mesh::build_batches` already bakes every submodel
/// into the static world, so a mover that did arrive would draw twice.
fn resolve_submodel(
    cache: &mut HashMap<usize, Option<ModelHandle>>,
    renderer: &mut Renderer,
    fs: &Pk3Fs,
    bsp: &Bsp,
    n: usize,
) -> Option<ModelHandle> {
    if let Some(h) = cache.get(&n) {
        return *h;
    }
    // submodel 0 is the whole world: wrong here and expensive to flatten
    let handle = (n > 0)
        .then(|| bsp.submodel_mesh(n))
        .flatten()
        .and_then(|(surfaces, materials)| renderer.upload_dynamic_mesh(fs, &surfaces, &materials));
    cache.insert(n, handle);
    handle
}

/// Loads the weapon file for a CS 7 name, caching failures as `None`.
fn resolve_weapon_def<'a>(
    cache: &'a mut HashMap<String, Option<vcod_common::weapon::WeaponDef>>,
    fs: &Pk3Fs,
    name: &str,
) -> Option<&'a vcod_common::weapon::WeaponDef> {
    if !cache.contains_key(name) {
        let def = match vcod_common::weapon::load(fs, name) {
            Ok(d) => Some(d),
            Err(e) => {
                log::warn!("weapon '{name}': {e:#}, drawing nothing for its dropped world model");
                None
            }
        };
        cache.insert(name.to_string(), def);
    }
    cache.get(name).unwrap().as_ref()
}

/// The CS7 name a 1-based `weapon`/`index` field names; `None` for 0 or out
/// of range. `pub(crate)` for `hud::Hud::weapon_kill_icon`.
pub(crate) fn weapon_name_for_index(weapons: &[String], weapon_index: i32) -> Option<&str> {
    if weapon_index <= 0 {
        return None;
    }
    weapons.get(weapon_index as usize - 1).map(String::as_str)
}

/// World xmodel for a player's `weapon` field (1-based CS7 index, 0 = none),
/// grafted onto `tag_weapon_right` as one more attachment. `None` never blocks
/// the rest of the assembly. Out-of-range and missing `worldModel` warn once
/// via `warned_items`; load failures are warned by `resolve_weapon_def`.
fn resolve_held_weapon(
    weapon_cache: &mut HashMap<String, Option<vcod_common::weapon::WeaponDef>>,
    warned_items: &mut HashSet<String>,
    fs: &Pk3Fs,
    weapons: &[String],
    weapon_index: i32,
) -> Option<String> {
    let Some(name) = weapon_name_for_index(weapons, weapon_index) else {
        if weapon_index > 0 && warned_items.insert(format!("held-oob:{weapon_index}")) {
            log::warn!(
                "held weapon index {weapon_index} out of range of the {}-entry CS7 weapon list",
                weapons.len()
            );
        }
        return None;
    };
    let def = resolve_weapon_def(weapon_cache, fs, name)?;
    let Some(world_model) = def.world_model.clone() else {
        if warned_items.insert(format!("held-no-world-model:{name}")) {
            log::warn!("weapon '{name}' has no worldModel, drawing it without a held weapon");
        }
        return None;
    };
    Some(world_model)
}

/// `worldFlashEffect` for a 1-based CS7 index (docs/research/cod11-events-and-fx.md,
/// section 5b). Silent on `None`: `fx::registry::resolve` falls back to a
/// class-based flash, and index and load failures are warned elsewhere.
fn resolve_weapon_flash<'a>(
    weapon_cache: &'a mut HashMap<String, Option<vcod_common::weapon::WeaponDef>>,
    fs: &Pk3Fs,
    weapons: &[String],
    weapon_index: i32,
) -> Option<&'a str> {
    let name = weapon_name_for_index(weapons, weapon_index)?;
    let def = resolve_weapon_def(weapon_cache, fs, name)?;
    def.world_flash_effect.as_deref()
}

/// Loads a player anim clip, caching both hits and failures by name.
fn load_clip(
    cache: &mut HashMap<String, Option<Rc<XAnim>>>,
    fs: &Pk3Fs,
    name: &str,
) -> Option<Rc<XAnim>> {
    if let Some(c) = cache.get(name) {
        return c.clone();
    }
    let clip = match xanim::load(fs, name) {
        Ok(a) => Some(Rc::new(a)),
        Err(e) => {
            log::warn!("player anim '{name}': {e:#}, skipping that clip");
            None
        }
    };
    cache.insert(name.to_string(), clip.clone());
    clip
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
fn clip_name(anims: &PlayerAnims, wire: i32, pitch_deg: f32, yaw_deg: f32) -> Option<&str> {
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

/// [`build_instances`]'s result.
pub struct BuiltScene {
    pub instances: Vec<DynamicModelInstance>,
    /// Per player entity number: world-space muzzle position and forward, from
    /// the held weapon's `tag_flash` (fallback `tag_weapon_right`). Only
    /// entities that drew a held weapon this frame.
    pub muzzles: HashMap<u32, (Vec3, Vec3)>,
    /// Per 1-based CS7 weapon index: its `worldFlashEffect` path, for
    /// `fx::registry::ResolveCtx`. Only weapons held by a drawn entity this
    /// frame, and only when the weapon file sets the key.
    pub weapon_flash: HashMap<i32, String>,
    /// Per drawn entity number: interpolated world position, for
    /// entity-attached sound voices. Every drawn entity, not only players.
    pub entity_pos: HashMap<u32, Vec3>,
}

/// Builds this frame's live-entity draw list from the interpolation pair
/// `(a, b, f)` the camera uses. `skip_num` is the entity the camera is inside
/// (our own, or the followed player's; docs/research/cod11-events-and-fx.md,
/// section 7). Positions lerp `a`->`b`, snapping on a teleport over 512 units.
///
/// `weapon_flash` also gets `b.ps.weapon` unconditionally: that player's body
/// is the skipped entity, and playerState-ring fire events
/// (`entity_num == u32::MAX`) need its exact `worldFlashEffect`.
#[allow(clippy::too_many_arguments)]
pub fn build_instances(
    scene: &mut EntityScene,
    (a, b, f): (&Snapshot, &Snapshot, f32),
    render_time: i32,
    skip_num: i32,
    configstrings: &[String],
    fs: &Pk3Fs,
    bsp: &Bsp,
    renderer: &mut Renderer,
    p: &Protocol,
) -> BuiltScene {
    // Destructure so `anims` can be read while the other fields are written.
    let EntityScene {
        assemblies,
        parts,
        model_cache,
        weapon_cache,
        submodel_cache,
        warned_items,
        anims,
        clips,
        states,
        stats,
    } = scene;
    let anims = anims
        .get_or_insert_with(|| match PlayerAnims::load(fs) {
            Ok(a) => Some(a),
            Err(e) => {
                log::warn!("player animtree: {e:#}, players will draw in bind pose");
                None
            }
        })
        .as_ref();

    // Shared across the whole pass, see ASSEMBLY_LOAD_BUDGET_PER_FRAME.
    let mut load_budget = ASSEMBLY_LOAD_BUDGET_PER_FRAME;

    // split once per frame, shared by the ET_ITEM and held-weapon lookups
    let weapon_names = split_weapon_list(configstrings.get(7).map(String::as_str).unwrap_or(""));

    let mut out = Vec::new();
    let mut muzzles: HashMap<u32, (Vec3, Vec3)> = HashMap::new();
    let mut weapon_flash: HashMap<i32, String> = HashMap::new();
    let mut entity_pos: HashMap<u32, Vec3> = HashMap::new();
    for (&num, ent) in &b.entities {
        if num as i32 == skip_num {
            continue; // the body the camera is inside, if the server sends it
        }
        let etype = ent.field_i32(p, "eType");
        let mut visual = resolve_visual(ent, &b.clients, configstrings, p);
        // A corpse carries no model of its own; it resolves through the dead
        // client's roster entry, which clears when they enter limbo. Fall back
        // to the visual cached while the corpse (or the player it copies) was
        // still resolvable, instead of dropping the body with it.
        if matches!(visual, EntityVisual::None) && etype == ET_CORPSE {
            let cn = ent.field_i32(p, "clientNum") as u32;
            if let Some(st) = states.get(&num).or_else(|| states.get(&cn)) {
                visual = st.visual.clone();
            }
        }
        if matches!(visual, EntityVisual::None) {
            continue;
        }

        let prev = a.entities.get(&num);

        // STATIONARY/INTERPOLATE aren't parametric, so a->b interpolation is
        // the evaluation. Every other trType is closed-form at `render_time`.
        let pos_tr = Trajectory::read(ent, p, "pos");
        let pos = if matches!(pos_tr.tr_type, TR_STATIONARY | TR_INTERPOLATE) {
            let ob = Vec3::from(ent.origin(p));
            match prev {
                Some(ea) => {
                    let oa = Vec3::from(ea.origin(p));
                    if oa.distance(ob) > 512.0 {
                        ob
                    } else {
                        oa.lerp(ob, f)
                    }
                }
                None => ob,
            }
        } else {
            pos_tr.evaluate(render_time)
        };

        // Same split. `[pitch, yaw, roll]`, matching `ent.angles` and the trajectory layout.
        let apos_tr = Trajectory::read(ent, p, "apos");
        let angles = if matches!(apos_tr.tr_type, TR_STATIONARY | TR_INTERPOLATE) {
            let ab = ent.angles(p);
            match prev {
                Some(ea) => {
                    let aa = ea.angles(p);
                    Vec3::new(
                        camera::lerp_angle(aa[0], ab[0], f),
                        camera::lerp_angle(aa[1], ab[1], f),
                        camera::lerp_angle(aa[2], ab[2], f),
                    )
                }
                None => Vec3::from(ab),
            }
        } else {
            apos_tr.evaluate(render_time)
        };
        if !pos.is_finite() || !angles.is_finite() {
            continue; // never feed a NaN transform to the GPU
        }
        entity_pos.insert(num, pos);
        let [pitch, yaw, roll] = angles.to_array();
        // Players are yaw-only; their pitch comes from `apply_aim`. Everything
        // else gets the full `AnglesToAxis` order so a grenade spins.
        let transform = match &visual {
            EntityVisual::Player { .. } => {
                Mat4::from_rotation_translation(Quat::from_rotation_z(yaw.to_radians()), pos)
            }
            _ => {
                let rot = Quat::from_rotation_z(yaw.to_radians())
                    * Quat::from_rotation_y(-pitch.to_radians())
                    * Quat::from_rotation_x(roll.to_radians());
                Mat4::from_rotation_translation(rot, pos)
            }
        };

        match visual {
            EntityVisual::Player {
                body,
                mut attachments,
            } => {
                // Kept before the held weapon joins: the corpse fallback
                // re-resolves the weapon from its own entityState.
                let roster_visual = EntityVisual::Player {
                    body: body.clone(),
                    attachments: attachments.clone(),
                };
                // The held weapon is one more attachment on tag_weapon_right,
                // so a weapon switch changes the assembly key like a gear change.
                let weapon_index = ent.field_i32(p, "weapon");
                let held_weapon = resolve_held_weapon(
                    weapon_cache,
                    warned_items,
                    fs,
                    &weapon_names,
                    weapon_index,
                );
                if let Some(world_model) = &held_weapon {
                    attachments.push((world_model.clone(), Some("tag_weapon_right".to_string())));
                }
                if let Some(path) =
                    resolve_weapon_flash(weapon_cache, fs, &weapon_names, weapon_index)
                {
                    weapon_flash
                        .entry(weapon_index)
                        .or_insert_with(|| path.to_string());
                }
                let key = AssemblyCache::key(&body, &attachments);
                let Some(assembly) = assemblies.resolve(
                    &key,
                    &body,
                    &attachments,
                    fs,
                    renderer,
                    parts,
                    &mut load_budget,
                    render_time,
                ) else {
                    continue;
                };
                if !states.contains_key(&num) {
                    let mut ea = EntityAnim::new(key.clone(), &assembly.skeleton, render_time);
                    // A corpse is the dead player's entityState copied to a
                    // fresh number; carry that entity's channels so the death
                    // clip keeps its phase instead of replaying from frame 0.
                    if etype == ET_CORPSE {
                        let cn = ent.field_i32(p, "clientNum") as u32;
                        if let Some(src) = states.get(&cn) {
                            ea.legs = src.legs;
                            ea.torso = src.torso;
                        }
                    }
                    states.insert(num, ea);
                }
                let st = states.get_mut(&num).expect("inserted above");
                if st.key != key {
                    // Channels hold only the wire index and start time, so
                    // carry them across a loadout change instead of restarting
                    // the clip. Only the pose buffer and bindings are skeleton-shaped.
                    let legs = std::mem::replace(&mut st.legs, Channel::new());
                    let torso = std::mem::replace(&mut st.torso, Channel::new());
                    *st = EntityAnim {
                        key: key.clone(),
                        pose: PoseBuffer::new(&assembly.skeleton),
                        legs,
                        torso,
                        bindings: HashMap::new(),
                        last_seen_ms: st.last_seen_ms,
                        visual: EntityVisual::None,
                    };
                }
                st.visual = roster_visual;
                st.last_seen_ms = render_time;
                stats.anim_restarts +=
                    u64::from(st.legs.update(ent.field_i32(p, "legsAnim"), render_time));
                stats.anim_restarts +=
                    u64::from(st.torso.update(ent.field_i32(p, "torsoAnim"), render_time));

                if let Some(anims) = anims {
                    // The yaw offset to clip_name is 0: the wire carries torso
                    // pitch, no torso yaw, so only the MG42 pitch rows ever
                    // pick a non-middle child.
                    let lerp_field = |name: &str| {
                        let vb = ent.field_f32(p, name);
                        match prev {
                            Some(ea) => {
                                let va = ea.field_f32(p, name);
                                va + (vb - va) * f
                            }
                            None => vb,
                        }
                    };
                    let pitch = lerp_field("fTorsoPitch");
                    let waist_pitch = lerp_field("fWaistPitch");
                    let lean = lerp_field("leanf");

                    // Legs first: `pb_*` keys the whole body, then `pt_*`
                    // overwrites only the bones it keys. A clip switch
                    // cross-fades from the outgoing clip: retail smooths
                    // stance/movement changes by blending animtree nodes,
                    // there are no transition clips in multiplayer.atr.
                    for ch in [&mut st.legs, &mut st.torso] {
                        let Some(name) = clip_name(anims, ch.index(), pitch, 0.0) else {
                            continue;
                        };
                        let Some(clip) = load_clip(clips, fs, name) else {
                            continue;
                        };
                        let fade = (render_time - ch.start_ms) as f32 / ANIM_BLEND_MS as f32;
                        if fade >= 1.0 {
                            ch.prev = None;
                        }
                        if let Some((praw, pstart)) = ch.prev {
                            let pclip = clip_name(anims, praw & ANIM_INDEX_MASK, pitch, 0.0)
                                .map(|n| (n.to_string(), load_clip(clips, fs, n)));
                            if let Some((pname, Some(pclip))) = pclip {
                                let pb = st
                                    .bindings
                                    .entry(pname)
                                    .or_insert_with(|| assembly.skeleton.bind(&pclip));
                                let pt = (render_time - pstart).max(0) as f32 / 1000.0;
                                st.pose
                                    .apply(&pclip, pb, pclip.frame_pos(pt, pclip.looping));
                            }
                        }
                        let binding = st
                            .bindings
                            .entry(name.to_string())
                            .or_insert_with(|| assembly.skeleton.bind(&clip));
                        let t = (render_time - ch.start_ms).max(0) as f32 / 1000.0;
                        let w = if ch.prev.is_some() {
                            fade.max(0.0)
                        } else {
                            1.0
                        };
                        st.pose
                            .apply_weighted(&clip, binding, clip.frame_pos(t, clip.looping), w);
                    }

                    // Corpses keep their death-clip pose; their aim fields are
                    // stale and would twist the body forever.
                    if ent.field_i32(p, "eType") == ET_PLAYER {
                        apply_aim(&mut st.pose, &assembly.skeleton, pitch, waist_pitch, lean);
                    }
                }

                // Prefer the weapon's `tag_flash`; fall back to the
                // `tag_weapon_right` graft point with the entity yaw as forward.
                if held_weapon.is_some() {
                    let muzzle = assembly
                        .skeleton
                        .bone_index("tag_flash")
                        .map(|bi| st.pose.bone_world(&assembly.skeleton, bi))
                        .or_else(|| {
                            assembly.skeleton.bone_index("tag_weapon_right").map(|bi| {
                                let (pos, _) = st.pose.bone_world(&assembly.skeleton, bi);
                                (pos, Quat::from_rotation_z(yaw.to_radians()))
                            })
                        });
                    if let Some((local_pos, local_rot)) = muzzle {
                        // Tags point +X forward (docs/research/xmodel-v14-format.md,
                        // "Model space and the view basis"). Flip to -X if a
                        // visual check shows the flash pointing backwards.
                        let world_pos = transform.transform_point3(local_pos);
                        let world_dir = transform.transform_vector3(local_rot * Vec3::X);
                        muzzles.insert(num, (world_pos, world_dir));
                    }
                }

                for (m, &handle) in assembly.handles.iter().enumerate() {
                    out.push(DynamicModelInstance {
                        model: handle,
                        transform,
                        bones: Some(st.pose.skin_matrices(&assembly.skeleton, m)),
                    });
                }
            }
            EntityVisual::Model(m) => {
                let Some(handle) = resolve_model(model_cache, renderer, fs, &m) else {
                    continue;
                };
                out.push(DynamicModelInstance {
                    model: handle,
                    transform,
                    bones: None,
                });
            }
            EntityVisual::Item(index) => {
                let Some(name) = weapon_name_for_index(&weapon_names, index as i32) else {
                    if warned_items.insert(format!("oob:{index}")) {
                        log::warn!(
                            "ET_ITEM index {index} out of range of the {}-entry CS7 weapon list",
                            weapon_names.len()
                        );
                    }
                    continue;
                };
                let Some(def) = resolve_weapon_def(weapon_cache, fs, name) else {
                    continue; // warned by resolve_weapon_def
                };
                let Some(world_model) = def.world_model.clone() else {
                    if warned_items.insert(format!("no-world-model:{name}")) {
                        log::warn!(
                            "weapon '{name}' has no worldModel, drawing nothing for its dropped item"
                        );
                    }
                    continue;
                };
                let Some(handle) = resolve_model(model_cache, renderer, fs, &world_model) else {
                    continue;
                };
                out.push(DynamicModelInstance {
                    model: handle,
                    transform,
                    bones: None,
                });
            }
            EntityVisual::Submodel(n) => {
                let Some(handle) = resolve_submodel(submodel_cache, renderer, fs, bsp, n) else {
                    continue; // collision-only submodel: nothing to draw
                };
                out.push(DynamicModelInstance {
                    model: handle,
                    transform,
                    bones: None,
                });
            }
            EntityVisual::None => unreachable!("skipped above"),
        }
    }
    // `b.ps`'s owner is always `skip_num`, so its weapon never reaches the
    // insert in the `Player` arm above.
    let ps_weapon = b.ps.field_i32(p, "weapon");
    if let Some(path) = resolve_weapon_flash(weapon_cache, fs, &weapon_names, ps_weapon) {
        weapon_flash
            .entry(ps_weapon)
            .or_insert_with(|| path.to_string());
    }
    // `abs` so a render clock that jumps backwards prunes instead of keeping everything.
    states.retain(|_, s| (render_time - s.last_seen_ms).abs() < STATE_TTL_MS);
    assemblies.prune_stale(render_time);
    stats.pending_assemblies = assemblies.pending.len();
    BuiltScene {
        instances: out,
        muzzles,
        weapon_flash,
        entity_pos,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use vcod_common::net::msg::{ClientState, EntityState};
    use vcod_common::net::protocol::{CS_MODELS_V1, CS_TAGS_V1, PROTOCOL_V1};

    #[test]
    fn weapon_list_splits_from_captured_gamestate() {
        let data = vcod_common::testing::fixture("net/gamestate.bin");
        let h = vcod_common::net::huffman::Huffman::new();
        let mut r = vcod_common::net::msg::MsgReader::new(&data[4..], &h);
        let gs = vcod_common::net::gamestate::parse(&mut r, &PROTOCOL_V1).unwrap();
        let list = split_weapon_list(&gs.configstrings[7]);
        assert!(list.iter().any(|w| w.contains("kar98k")), "{list:?}");
    }

    fn cs_table() -> Vec<String> {
        let mut cs = vec![String::new(); 2048];
        cs[CS_MODELS_V1 + 5] = "xmodel/playerbody_american_airborne".into();
        cs[CS_MODELS_V1 + 6] = "xmodel/basehead2".into();
        cs[CS_MODELS_V1 + 7] = "xmodel/USAirborneHelmet".into();
        cs[CS_MODELS_V1 + 9] = "xmodel/crate_misc1".into();
        cs[CS_MODELS_V1 + 10] = "*2".into();
        cs[CS_TAGS_V1 + 3] = "tag_helmet".into();
        cs
    }

    fn ent(etype: i32, client_num: i32, index: i32) -> EntityState {
        let p = &PROTOCOL_V1;
        let mut e = EntityState::null(p);
        let put = |e: &mut EntityState, n: &str, v: i32| {
            e.fields[EntityState::field_index(p, n).unwrap()] = v;
        };
        put(&mut e, "eType", etype);
        put(&mut e, "clientNum", client_num);
        put(&mut e, "index", index);
        e
    }

    fn client(modelindex: i32, attach: &[(i32, i32)]) -> ClientState {
        let p = &PROTOCOL_V1;
        let mut c = ClientState::null(p);
        let put = |c: &mut ClientState, n: &str, v: i32| {
            c.fields[ClientState::field_index(p, n).unwrap()] = v;
        };
        put(&mut c, "modelindex", modelindex);
        for (i, &(m, t)) in attach.iter().enumerate() {
            put(&mut c, &format!("attachModelIndex[{i}]"), m);
            put(&mut c, &format!("attachTagIndex[{i}]"), t);
        }
        c
    }

    #[test]
    fn player_resolves_body_and_attachments() {
        let p = &PROTOCOL_V1;
        let cs = cs_table();
        let mut clients = BTreeMap::new();
        clients.insert(4u32, client(5, &[(6, 0), (7, 3)]));
        let v = resolve_visual(&ent(ET_PLAYER, 4, 0), &clients, &cs, p);
        assert_eq!(
            v,
            EntityVisual::Player {
                body: "playerbody_american_airborne".into(),
                attachments: vec![
                    ("basehead2".into(), None),
                    ("USAirborneHelmet".into(), Some("tag_helmet".into())),
                ],
            }
        );
        // Corpses resolve through the same clientState.
        assert!(matches!(
            resolve_visual(&ent(ET_CORPSE, 4, 0), &clients, &cs, p),
            EntityVisual::Player { .. }
        ));
        // Unknown client -> nothing (never a fallback body).
        assert_eq!(
            resolve_visual(&ent(ET_PLAYER, 9, 0), &clients, &cs, p),
            EntityVisual::None
        );
    }

    #[test]
    fn world_entities_route_by_etype() {
        let p = &PROTOCOL_V1;
        let cs = cs_table();
        let none = BTreeMap::new();
        assert_eq!(
            resolve_visual(&ent(ET_MISSILE, 0, 9), &none, &cs, p),
            EntityVisual::Model("crate_misc1".into())
        );
        assert_eq!(
            resolve_visual(&ent(ET_MOVER, 0, 10), &none, &cs, p),
            EntityVisual::Submodel(2)
        );
        for skip in [ET_PORTAL, ET_INVISIBLE, ET_EVENTS, ET_EVENTS + 5] {
            assert_eq!(
                resolve_visual(&ent(skip, 0, 9), &none, &cs, p),
                EntityVisual::None
            );
        }
        // index 0 = no model.
        assert_eq!(
            resolve_visual(&ent(ET_GENERAL, 0, 0), &none, &cs, p),
            EntityVisual::None
        );
    }

    #[test]
    fn item_resolves_to_raw_index_not_cs_models() {
        let p = &PROTOCOL_V1;
        let cs = cs_table();
        let none = BTreeMap::new();
        assert_eq!(
            resolve_visual(&ent(ET_ITEM, 0, 9), &none, &cs, p),
            EntityVisual::Item(9)
        );
        // index 0 = no item.
        assert_eq!(
            resolve_visual(&ent(ET_ITEM, 0, 0), &none, &cs, p),
            EntityVisual::None
        );
    }

    #[test]
    fn toggle_bit_restarts_channel() {
        let mut ch = Channel::new();
        assert!(ch.update(5, 1000)); // first sight: (re)start
        assert_eq!(ch.prev, None); // nothing to fade from
        assert!(!ch.update(5, 1500)); // same raw: keep phase
        assert!(ch.update(5 | 512, 2000)); // toggle flip: restart
        assert_eq!(ch.prev, None); // same clip re-trigger: no fade
        assert!(ch.update(6 | 512, 2500)); // index change: restart
        assert_eq!(ch.prev, Some((5 | 512, 2000))); // fade from the old clip
        assert_eq!(ch.index(), 6);
        assert_eq!(ch.start_ms, 2500);
    }

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

    #[test]
    fn assembly_key_is_stable_and_tag_sensitive() {
        let a = AssemblyCache::key("body", &[("head".into(), None)]);
        let b = AssemblyCache::key("body", &[("head".into(), None)]);
        let c = AssemblyCache::key("body", &[("head".into(), Some("tag_x".into()))]);
        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn weapon_name_for_index_resolves_1_based_cs7() {
        let weapons = split_weapon_list("knife_mp kar98k_mp mp40_mp");
        assert_eq!(weapon_name_for_index(&weapons, 0), None); // 0 = no weapon
        assert_eq!(weapon_name_for_index(&weapons, -1), None);
        assert_eq!(weapon_name_for_index(&weapons, 2), Some("kar98k_mp"));
        assert_eq!(weapon_name_for_index(&weapons, 99), None); // out of range
    }

    #[test]
    fn assembly_key_differs_by_weapon() {
        let gear = [("helmet".into(), Some("tag_helmet".into()))];
        let key_of = |weapon: Option<&str>| {
            let mut attachments = gear.to_vec();
            if let Some(w) = weapon {
                attachments.push((w.into(), Some("tag_weapon_right".into())));
            }
            AssemblyCache::key("body", &attachments)
        };
        let none = key_of(None);
        let kar98 = key_of(Some("weapon_kar98"));
        let mp40 = key_of(Some("weapon_mp40"));
        assert_ne!(none, kar98);
        assert_ne!(kar98, mp40);
    }

    /// No real GPU upload; `ModelHandle`'s index is `pub(crate)` so tests can build one.
    fn fake_part() -> (Rc<XModel>, ModelHandle) {
        let m = XModel {
            lod: "x".into(),
            surfaces: vec![],
            materials: vec![],
            bones: vec![],
            collision: Vec::new(),
        };
        (Rc::new(m), ModelHandle(0))
    }

    #[test]
    fn pending_assembly_loads_one_part_at_a_time() {
        let mut pending = PendingAssembly::new(
            "body".into(),
            vec![("head".into(), None), ("helmet".into(), None)],
            0,
        );
        assert!(!pending.is_complete());
        assert!(matches!(pending.next_part(), NextPart::Body));

        pending.body_part = Some(Some(fake_part())); // tick: body attempted (succeeded)
        assert!(!pending.is_complete());
        assert!(matches!(pending.next_part(), NextPart::Attachment(0)));

        pending.attachment_parts[0] = Some(None); // tick: attachment 0 attempted (failed)
        assert!(!pending.is_complete());
        assert!(matches!(pending.next_part(), NextPart::Attachment(1)));

        let (model, handle) = fake_part();
        pending.attachment_parts[1] = Some(Some((model, None, handle))); // tick: attachment 1 attempted
        assert!(pending.is_complete());
        assert!(matches!(pending.next_part(), NextPart::Done));
    }

    #[test]
    fn pending_assembly_stops_at_a_failed_body() {
        let mut pending = PendingAssembly::new(
            "body".into(),
            vec![("head".into(), None), ("helmet".into(), None)],
            0,
        );
        pending.body_part = Some(None); // body attempted and failed
        assert!(pending.is_complete());
        assert!(matches!(pending.next_part(), NextPart::Done));
        assert!(pending.attachment_parts.iter().all(Option::is_none));
    }

    use vcod_common::xmodel::Bone;

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
