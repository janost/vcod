//! Snapshot entities to drawables, plus the per-entity legs/torso anim driver.
//! docs/research/clientstate-wire-format.md, docs/research/player-model-anim-system.md.

use crate::camera;
use crate::renderer::{DynamicModelInstance, ModelHandle, Renderer};
use crate::turret::{self, ET_TURRET, GunAnim, MsvcRand, TurretEye};
use glam::{Mat4, Quat, Vec3};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::rc::Rc;
use vcod_common::animtree::PlayerAnims;
use vcod_common::collision::MASK_PLAYERSOLID;
use vcod_common::movetrace::MoveWorld;
use vcod_common::net::flags::{EF_DEAD, EF_PRONE};
use vcod_common::net::msg::{ClientState, EntityState};
use vcod_common::net::protocol::{CS_MODELS_V1, CS_TAGS_V1, Protocol};
use vcod_common::net::snapshot::Snapshot;
use vcod_common::net::trajectory::{TR_INTERPOLATE, TR_LINEAR_STOP, Trajectory};
use vcod_common::pk3::Pk3Fs;
use vcod_common::playerpose::{AimPitch, PitchSwing, apply_aim, clip_name};
use vcod_common::pmove::movers::SnapshotMovers;
use vcod_common::skeleton::{AnimBinding, PoseBuffer, Skeleton};
use vcod_common::turretpose::{GunnerPlacement, angles_quat, place_gunner, tag_weapon_local};
use vcod_common::weapon_table::STATIC_ITEMS;
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

pub use vcod_common::net::events::ET_EVENTS;
/// `eFlags` bit that hides an entity's model.
use vcod_common::net::flags::EF_NODRAW;
/// `solid` of a brush model entity, whose `index` is then an inline model
/// number rather than a model configstring slot
/// (docs/research/cod11-movers.md, section 14).
pub use vcod_common::net::flags::SOLID_BMODEL;
pub use vcod_common::net::flags::{
    ET_CORPSE, ET_GENERAL, ET_ITEM, ET_MISSILE, ET_MOVER, ET_PLAYER, ET_SCRIPTMOVER,
};

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
    /// Flying missile: its `weapon`, 1-based into configstring 7, resolved to
    /// that weapon's `projectileModel` in `build_instances`. The wire's
    /// `index` is 0 on a retail grenade (docs/research/cod11-combat.md, 11.1).
    Missile(usize),
    /// Inline BSP submodel (`"*N"` configstring).
    Submodel(usize),
    /// A mounted gun: its xmodel, and its `weapon` (1-based into
    /// configstring 7) for the anims and the flash.
    Turret {
        model: String,
        weapon: usize,
    },
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
        ET_MISSILE => {
            let weapon = ent.field_i32(p, "weapon");
            if weapon <= 0 {
                return EntityVisual::None;
            }
            EntityVisual::Missile(weapon as usize)
        }
        ET_GENERAL | ET_SCRIPTMOVER | ET_MOVER => {
            let mi = ent.field_i32(p, "index");
            if mi <= 0 || (etype == ET_SCRIPTMOVER && ent.field_i32(p, "eFlags") & EF_NODRAW != 0) {
                return EntityVisual::None;
            }
            if ent.field_i32(p, "solid") == SOLID_BMODEL {
                return EntityVisual::Submodel(mi as usize);
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
        ET_TURRET => {
            let mi = ent.field_i32(p, "index");
            if mi <= 0 || ent.field_i32(p, "eFlags") & EF_NODRAW != 0 {
                return EntityVisual::None;
            }
            match model_name(cs(CS_MODELS_V1 + mi as usize)) {
                Some(model) => EntityVisual::Turret {
                    model,
                    weapon: ent.field_i32(p, "weapon").max(0) as usize,
                },
                None => EntityVisual::None,
            }
        }
        _ => EntityVisual::None, // portal, invisible, events
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

/// Per-connection draw-path state: caches, the player animtree, clips, and one
/// [`EntityAnim`] per animating entity.
pub struct EntityScene {
    assemblies: AssemblyCache,
    parts: PartCache,
    /// World-entity models by xmodel name. `None` is a failed load, warned once.
    model_cache: HashMap<String, Option<ModelHandle>>,
    /// Weapon defs by CS 7 name. `None` is a failed load, warned once.
    weapon_cache: HashMap<String, Option<vcod_common::weapon::WeaponDef>>,
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
    /// Turret models by xmodel name. `None` is a failed load, warned once.
    turret_rigs: HashMap<String, Option<TurretRig>>,
    /// Per turret entity: which of its two anims plays, and since when.
    turret_anims: HashMap<u32, TurretAnim>,
    /// The firing view's shake.
    shake: MsvcRand,
    /// The last pass's `render_time`: `cg.frametime` for the pitch swing.
    last_render_ms: Option<i32>,
    pub stats: SceneStats,
}

/// One uploaded turret model and its skeleton.
struct TurretRig {
    handle: ModelHandle,
    skeleton: Skeleton,
    bindings: HashMap<String, AnimBinding>,
    /// Bind positions of `tag_aim` and `tag_weapon` in model space, which
    /// the gunner placement reads.
    aim_tags: Option<(Vec3, Vec3)>,
}

/// One gun this frame: its world frame, the barrel's lerped `angles2`, and
/// what the gunner placement needs off it.
struct GunFrame {
    pos: Vec3,
    rot: Quat,
    barrel: [f32; 3],
    /// `tag_weapon` turned by the barrel, in model space.
    tag_weapon: Option<(Vec3, Quat)>,
    /// The weapon file's `animHorRotateInc`.
    rotate_inc: f32,
}

struct TurretAnim {
    channel: Channel,
    last_seen_ms: i32,
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
            warned_items: HashSet::new(),
            anims: None,
            clips: HashMap::new(),
            states: HashMap::new(),
            turret_rigs: HashMap::new(),
            turret_anims: HashMap::new(),
            shake: MsvcRand::default(),
            last_render_ms: None,
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
    /// The torso pitch easing after the view (combat doc 16.3).
    pitch_swing: PitchSwing,
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
            pitch_swing: PitchSwing::default(),
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

/// Loads and uploads a turret's xmodel with its skeleton, caching failures as `None`.
fn resolve_turret_rig<'a>(
    cache: &'a mut HashMap<String, Option<TurretRig>>,
    renderer: &mut Renderer,
    fs: &Pk3Fs,
    name: &str,
) -> Option<&'a mut TurretRig> {
    if !cache.contains_key(name) {
        let rig = match xmodel::load(fs, name) {
            Ok(m) => renderer.upload_dynamic_model(fs, &m).map(|handle| {
                let skeleton = Skeleton::build(&[&m]);
                let bind = PoseBuffer::new(&skeleton);
                let tag = |n: &str| Some(bind.bone_world(&skeleton, skeleton.bone_index(n)?).0);
                let aim_tags = tag("tag_aim").zip(tag("tag_weapon"));
                TurretRig {
                    handle,
                    skeleton,
                    bindings: HashMap::new(),
                    aim_tags,
                }
            }),
            Err(e) => {
                log::warn!("turret model '{name}': {e:#}, drawing nothing for it");
                None
            }
        };
        cache.insert(name.to_string(), rig);
    }
    cache.get_mut(name).unwrap().as_mut()
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
    /// Per drawn `ET_PLAYER`: the world position of its `Bip01 Head` bone,
    /// where the head icon hangs (`hud::head_icon`).
    pub heads: HashMap<u32, Vec3>,
    /// The gun `b.ps` rides, when it was drawn: the first-person eye.
    pub turret_eye: Option<TurretEye>,
    /// Inline models drawn this frame with their entity's pose, the identity
    /// for one where the map put it ([`Renderer::set_submodels`]).
    pub submodels: Vec<(usize, Mat4)>,
}

/// Entity numbers below this are clients (`MAX_CLIENTS`).
const MAX_CLIENTS: u32 = vcod_common::net::protocol::MAX_CLIENTS as u32;

/// The older snapshot of the pair an entity is drawn between, with its
/// movers, which is what retail's `cg.snap` holds.
struct LerpFrom<'a> {
    snap: &'a Snapshot,
    movers: SnapshotMovers,
}

/// An entity's origin and `[pitch, yaw, roll]` at `render_time`, after
/// `CG_CalcEntityLerpPositions` (cgame 0x3001d210). A `pos` of
/// `TR_INTERPOLATE`, or a player's `TR_LINEAR_STOP`, lerps both from the
/// older snapshot's state to `ent` (snapping on a teleport over 512 units).
/// Anything else evaluates the older state's `pos` and `apos` at
/// `render_time` and is carried by what its ground mover does between the
/// older snapshot and `render_time` (`CG_AdjustPositionForMover` at
/// 0x3001d2ee, translation only), so an item resting on a moving brush model
/// rides it between snapshots.
fn lerp_pos_angles(
    num: u32,
    ent: &EntityState,
    from: &LerpFrom,
    f: f32,
    render_time: i32,
    p: &Protocol,
) -> (Vec3, Vec3) {
    let prev = from.snap.entities.get(&num);
    let cur = prev.unwrap_or(ent);
    let pos_tr = Trajectory::read(cur, p, "pos");
    let interpolates =
        pos_tr.tr_type == TR_INTERPOLATE || (pos_tr.tr_type == TR_LINEAR_STOP && num < MAX_CLIENTS);
    if !interpolates {
        let pos = from.movers.carry(
            pos_tr.evaluate(render_time),
            cur.field_i32(p, "groundEntityNum"),
            from.snap.server_time,
            render_time,
        );
        return (pos, Trajectory::read(cur, p, "apos").evaluate(render_time));
    }
    let ob = Vec3::from(ent.origin(p));
    let ab = ent.angles(p);
    let Some(ea) = prev else {
        return (ob, Vec3::from(ab));
    };
    let oa = Vec3::from(ea.origin(p));
    let pos = if oa.distance(ob) > 512.0 {
        ob
    } else {
        oa.lerp(ob, f)
    };
    let aa = ea.angles(p);
    let angles = Vec3::new(
        camera::lerp_angle(aa[0], ab[0], f),
        camera::lerp_angle(aa[1], ab[1], f),
        camera::lerp_angle(aa[2], ab[2], f),
    );
    (pos, angles)
}

/// The model rotation an entity draws with. Players are yaw-only; their
/// pitch comes from `apply_aim`. Everything else goes through `AnglesToAxis`
/// (`cgame_mp_x86.dll` 0x3003c770, called by every model draw off
/// `CG_AddCEntity`), so pitch positive tips the nose down and a grenade
/// tumbles forward.
fn model_rotation(visual: &EntityVisual, angles: Vec3) -> Quat {
    match visual {
        EntityVisual::Player { .. } => Quat::from_rotation_z(angles.y.to_radians()),
        _ => angles_quat(angles.to_array()),
    }
}

/// 0x300279b0 (turrets doc 14.5): a mounted player's origin, body rotation
/// and leaf blend for the wire anim `anim`, off the gun its `otherEntityNum`
/// names, and the gun's z for the trace down. `None` leaves the body where
/// the snapshot put it, as the routine's early returns do.
#[allow(clippy::too_many_arguments)]
fn place_body(
    anims: &PlayerAnims,
    anim: i32,
    ent: &EntityState,
    p: &Protocol,
    guns: &HashMap<u32, GunFrame>,
    pos: Vec3,
    clips: &mut HashMap<String, Option<Rc<XAnim>>>,
    fs: &Pk3Fs,
) -> Option<(GunnerPlacement, f32)> {
    if ent.field_i32(p, "eFlags") & turret::EF_MOUNTED == 0 {
        return None;
    }
    // Entity numbers under 64 are clients, 1023 is none (0x300279c0).
    let gun = u32::try_from(ent.field_i32(p, "otherEntityNum"))
        .ok()
        .filter(|n| (64..1023).contains(n))?;
    let gun = guns.get(&gun)?;
    if gun.rotate_inc <= 0.0 {
        return None; // not a turret file: the column split would divide by 0
    }
    place_gunner(
        anims,
        |n| load_clip(clips, fs, n),
        anim,
        gun.tag_weapon?,
        (gun.pos, gun.rot),
        pos,
        gun.rotate_inc,
    )
    .map(|g| (g, gun.pos.z))
}

/// 0x300279b0's trace (turrets doc 14.7): from the gun's height straight
/// down to the placed spot under `MASK_PLAYERSOLID`, every solid but the
/// gunner's own clipping it; the z moves onto whatever it meets.
fn trace_down(world: MoveWorld, gunner: u32, mut at: Vec3, gun_z: f32) -> Vec3 {
    let start = Vec3::new(at.x, at.y, gun_z);
    let world = MoveWorld {
        pass: gunner,
        ..world
    };
    let tr = world.box_trace(start, at, Vec3::ZERO, Vec3::ZERO, MASK_PLAYERSOLID);
    if tr.fraction < 1.0 {
        at.z = tr.endpos.z;
    }
    at
}

/// Poses `set`, clips and their blend weights, onto `pose` at `t` seconds,
/// the whole set at `weight` over what the buffer holds. Each clip after the
/// first lerps by its share of the running total, which averages the set
/// exactly at `weight` 1; under it (a 0.2 s switch fade) the mix is close.
#[allow(clippy::too_many_arguments)]
fn apply_clip_set(
    pose: &mut PoseBuffer,
    skel: &Skeleton,
    bindings: &mut HashMap<String, AnimBinding>,
    clips: &mut HashMap<String, Option<Rc<XAnim>>>,
    fs: &Pk3Fs,
    set: &[(&str, f32)],
    t: f32,
    weight: f32,
) {
    let mut total = 0.0;
    for &(name, w) in set {
        if w <= 0.0 {
            continue;
        }
        let Some(clip) = load_clip(clips, fs, name) else {
            continue;
        };
        let binding = bindings
            .entry(name.to_string())
            .or_insert_with(|| skel.bind(&clip));
        let k = if total == 0.0 {
            weight
        } else {
            w / (total + w)
        };
        total += w;
        pose.apply_weighted(&clip, binding, clip.frame_pos(t, clip.looping), k);
    }
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
    trace: Option<MoveWorld>,
    renderer: &mut Renderer,
    p: &Protocol,
) -> BuiltScene {
    // Destructure so `anims` can be read while the other fields are written.
    let EntityScene {
        assemblies,
        parts,
        model_cache,
        weapon_cache,
        warned_items,
        anims,
        clips,
        states,
        turret_rigs,
        turret_anims,
        shake,
        last_render_ms,
        stats,
    } = scene;
    let frametime_ms = last_render_ms.map_or(0, |t| (render_time - t).max(0));
    *last_render_ms = Some(render_time);
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
    let mut heads: HashMap<u32, Vec3> = HashMap::new();
    let mut turret_eye = None;
    let mut submodels = Vec::new();
    let ps_int = |name: &str| b.ps.field_i32(p, name);
    let ridden = turret::ridden(
        ps_int("eFlags"),
        ps_int("viewlocked"),
        ps_int("viewlocked_entNum"),
    );
    let from = LerpFrom {
        snap: a,
        movers: SnapshotMovers::from_entities(p, &a.entities),
    };
    // The guns first: a gunner's body is placed off its gun, whose number is
    // always above the gunner's.
    let mut guns: HashMap<u32, GunFrame> = HashMap::new();
    for (&num, ent) in &b.entities {
        let EntityVisual::Turret { model, weapon } =
            resolve_visual(ent, &b.clients, configstrings, p)
        else {
            continue;
        };
        let Some(rig) = resolve_turret_rig(turret_rigs, renderer, fs, &model) else {
            continue;
        };
        let prev = a.entities.get(&num);
        let (pos, angles) = lerp_pos_angles(num, ent, &from, f, render_time, p);
        let a2 =
            |e: &EntityState| ["angles2[0]", "angles2[1]", "angles2[2]"].map(|n| e.field_f32(p, n));
        let eflags = |e: &EntityState| e.field_i32(p, "eFlags");
        let from = prev.filter(|ea| turret::interpolates(eflags(ea), eflags(ent)));
        let barrel = turret::barrel(from.map(a2), a2(ent), f);
        let rotate_inc = weapon_name_for_index(&weapon_names, weapon as i32)
            .and_then(|name| resolve_weapon_def(weapon_cache, fs, name))
            .map_or(0.0, |d| d.anim_hor_rotate_inc);
        guns.insert(
            num,
            GunFrame {
                pos,
                rot: angles_quat(angles.to_array()),
                barrel,
                tag_weapon: rig
                    .aim_tags
                    .map(|(aim, weapon)| tag_weapon_local(aim, weapon, barrel)),
                rotate_inc,
            },
        );
    }
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
        let (mut pos, angles) = lerp_pos_angles(num, ent, &from, f, render_time, p);
        if !pos.is_finite() || !angles.is_finite() {
            continue; // never feed a NaN transform to the GPU
        }
        let mut rot = model_rotation(&visual, angles);
        let mut yaw = angles.y;
        // A gunner stands and turns where its gun puts it, traced down.
        let snap_pos = pos;
        let placed = match (&visual, anims) {
            (EntityVisual::Player { .. }, Some(anims)) if etype == ET_PLAYER => place_body(
                anims,
                ent.field_i32(p, "legsAnim"),
                ent,
                p,
                &guns,
                pos,
                clips,
                fs,
            ),
            _ => None,
        };
        let placed = placed.map(|(g, gun_z)| {
            let traced = match trace {
                Some(world) if g.origin.is_finite() => trace_down(world, num, g.origin, gun_z),
                _ => g.origin,
            };
            GunnerPlacement {
                origin: traced,
                ..g
            }
        });
        if let Some(g) = placed.as_ref().filter(|g| g.origin.is_finite()) {
            // Yaw only, as every player draws; a stock gun stands level.
            pos = g.origin;
            yaw = g.yaw();
            rot = Quat::from_rotation_z(yaw.to_radians());
        }
        entity_pos.insert(num, pos);
        let transform = Mat4::from_rotation_translation(rot, pos);

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
                // A gunner's is not drawn (0x30004eb0 zeroes it on `eFlags & 0xc000`).
                let weapon_index = if ent.field_i32(p, "eFlags") & turret::EF_MOUNTED != 0 {
                    0
                } else {
                    ent.field_i32(p, "weapon")
                };
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
                        pitch_swing: st.pitch_swing,
                    };
                }
                st.visual = roster_visual;
                st.last_seen_ms = render_time;
                stats.anim_restarts +=
                    u64::from(st.legs.update(ent.field_i32(p, "legsAnim"), render_time));
                stats.anim_restarts +=
                    u64::from(st.torso.update(ent.field_i32(p, "torsoAnim"), render_time));

                if let Some(anims) = anims {
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
                    let eflags = ent.field_i32(p, "eFlags");

                    // The clips a wire anim poses: a gunner's turret anim is
                    // the placement's leaf blend (0x300279b0 sets the goal
                    // weights on the gunner's own tree); anything else is
                    // one clip, an MG42 aim group descending by the torso
                    // pitch with no yaw, which the wire does not carry.
                    let clip_set = |raw: i32, clips: &mut _| -> Vec<(&str, f32)> {
                        if placed.is_some()
                            && let Some((g, _)) =
                                place_body(anims, raw, ent, p, &guns, snap_pos, clips, fs)
                        {
                            return g
                                .leaves
                                .iter()
                                .map(|&(n, w)| (anims.tree.nodes[n].name.as_str(), w))
                                .collect();
                        }
                        clip_name(anims, raw & ANIM_INDEX_MASK, pitch, 0.0)
                            .map(|n| vec![(n, 1.0)])
                            .unwrap_or_default()
                    };

                    // Legs first: `pb_*` keys the whole body, then `pt_*`
                    // overwrites only the bones it keys. A clip switch
                    // cross-fades from the outgoing clip: retail smooths
                    // stance/movement changes by blending animtree nodes,
                    // there are no transition clips in multiplayer.atr.
                    for ch in [&mut st.legs, &mut st.torso] {
                        let set = clip_set(ch.index(), clips);
                        if set.is_empty() {
                            continue;
                        }
                        let fade = (render_time - ch.start_ms) as f32 / ANIM_BLEND_MS as f32;
                        if fade >= 1.0 {
                            ch.prev = None;
                        }
                        let secs = |start: i32| (render_time - start).max(0) as f32 / 1000.0;
                        let skel = &assembly.skeleton;
                        if let Some((praw, pstart)) = ch.prev {
                            let pset = clip_set(praw, clips);
                            apply_clip_set(
                                &mut st.pose,
                                skel,
                                &mut st.bindings,
                                clips,
                                fs,
                                &pset,
                                secs(pstart),
                                1.0,
                            );
                        }
                        let w = if ch.prev.is_some() {
                            fade.max(0.0)
                        } else {
                            1.0
                        };
                        apply_clip_set(
                            &mut st.pose,
                            skel,
                            &mut st.bindings,
                            clips,
                            fs,
                            &set,
                            secs(ch.start_ms),
                            w,
                        );
                    }

                    // Corpses keep their death-clip pose; their aim fields are
                    // stale and would twist the body forever. A gunner runs no
                    // controllers (cgame 0x30004710).
                    if etype == ET_PLAYER {
                        // cgame's `CG_PlayerAnimation` (0x30004e40) eases the
                        // torso after the lerped view pitch every frame; a
                        // dead, mounted or climbing body eases back to level.
                        let climbing = anims
                            .name(st.legs.index())
                            .is_some_and(|n| n.starts_with("pb_climb"));
                        let mounted = eflags & turret::EF_MOUNTED != 0;
                        st.pitch_swing.step(
                            angles.x,
                            frametime_ms,
                            eflags & EF_DEAD != 0 || mounted || climbing,
                        );
                        if !mounted {
                            let aim = AimPitch::new(
                                angles.x,
                                &st.pitch_swing,
                                eflags & EF_PRONE != 0,
                                pitch,
                                waist_pitch,
                            );
                            apply_aim(&mut st.pose, &assembly.skeleton, &aim, lean);
                        }
                    }
                }

                if etype == ET_PLAYER
                    && let Some(bi) = assembly.skeleton.bone_index("Bip01 Head")
                {
                    let (local, _) = st.pose.bone_world(&assembly.skeleton, bi);
                    heads.insert(num, transform.transform_point3(local));
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
            EntityVisual::Missile(index) => {
                let Some(def) = weapon_name_for_index(&weapon_names, index as i32)
                    .and_then(|name| resolve_weapon_def(weapon_cache, fs, name))
                else {
                    continue;
                };
                let Some(model) = def.projectile_model.clone() else {
                    continue;
                };
                let Some(handle) = resolve_model(model_cache, renderer, fs, &model) else {
                    continue;
                };
                out.push(DynamicModelInstance {
                    model: handle,
                    transform,
                    bones: None,
                });
            }
            EntityVisual::Item(index) => {
                // Ammo and health rows (65-69) carry their own world model; a
                // mod's dropped health pack is one.
                if let Some((.., model)) = STATIC_ITEMS.iter().find(|(i, ..)| *i == index) {
                    if let Some(handle) = resolve_model(model_cache, renderer, fs, model) {
                        out.push(DynamicModelInstance {
                            model: handle,
                            transform,
                            bones: None,
                        });
                    }
                    continue;
                }
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
                // At rest where the map put it, which is the zero pose: no
                // stock `script_brushmodel` carries an `origin` key, so its
                // brushes and surfaces are in world space.
                let at_rest = pos.abs().max_element() < 0.01 && angles.abs().max_element() < 0.01;
                submodels.push((n, if at_rest { Mat4::IDENTITY } else { transform }));
            }
            EntityVisual::Turret { model, weapon } => {
                let (Some(rig), Some(gun)) = (
                    resolve_turret_rig(turret_rigs, renderer, fs, &model),
                    guns.get(&num),
                ) else {
                    continue;
                };
                let def = weapon_name_for_index(&weapon_names, weapon as i32)
                    .and_then(|name| resolve_weapon_def(weapon_cache, fs, name));
                let (idle, fire, flash) = def.map_or((None, None, None), |d| {
                    (
                        d.idle_anim.clone(),
                        d.fire_anim.clone(),
                        d.world_flash_effect.clone(),
                    )
                });
                let rides = ridden == Some(num);
                let st = turret_anims.entry(num).or_insert_with(|| TurretAnim {
                    channel: Channel::new(),
                    last_seen_ms: render_time,
                });
                st.last_seen_ms = render_time;
                let slot = turret::gun_anim(rides, ent.field_i32(p, "eFlags"));
                st.channel.update(slot as i32, render_time);
                let clip_of = |raw: i32| {
                    if raw & ANIM_INDEX_MASK == GunAnim::Fire as i32 {
                        fire.as_deref()
                    } else {
                        idle.as_deref()
                    }
                };

                // The previous anim at full weight, then the goal blended over it.
                let mut pose = PoseBuffer::new(&rig.skeleton);
                let ch = &mut st.channel;
                let fade = (render_time - ch.start_ms) as f32 / turret::ANIM_BLEND_MS as f32;
                if fade >= 1.0 {
                    ch.prev = None;
                }
                let layers = ch
                    .prev
                    .map(|(raw, start)| (raw, start, 1.0))
                    .into_iter()
                    .chain([(
                        ch.raw,
                        ch.start_ms,
                        if ch.prev.is_some() {
                            fade.max(0.0)
                        } else {
                            1.0
                        },
                    )]);
                for (raw, start, w) in layers {
                    let Some(name) = clip_of(raw) else { continue };
                    let Some(clip) = load_clip(clips, fs, name) else {
                        continue;
                    };
                    let binding = rig
                        .bindings
                        .entry(name.to_string())
                        .or_insert_with(|| rig.skeleton.bind(&clip));
                    let t = (render_time - start).max(0) as f32 / 1000.0;
                    pose.apply_weighted(&clip, binding, clip.frame_pos(t, clip.looping), w);
                }

                let barrel = gun.barrel;
                turret::apply_controller(&mut pose, &rig.skeleton, barrel);
                let worlds = pose.bone_worlds(&rig.skeleton);
                let tag = |name: &str| {
                    let (lp, lr) = worlds[rig.skeleton.bone_index(name)?];
                    Some((pos + rot * lp, rot * lr))
                };
                // The flash plays on the gun's own `tag_flash`, world effect
                // even for the gunner (`CG_FireWeapon` 0x30038bdd).
                if let Some((at, r)) = tag("tag_flash") {
                    muzzles.insert(num, (at, r * Vec3::X));
                }
                if let Some(path) = flash {
                    weapon_flash.entry(weapon as i32).or_insert(path);
                }
                if rides && let Some((eye, _)) = tag("tag_player") {
                    let [mut pitch, mut yaw, roll] = angles.to_array();
                    pitch += barrel[0];
                    yaw += barrel[1];
                    // `viewlocked` 2 is a frame that fired.
                    if ps_int("viewlocked") == 2 {
                        pitch += shake.shake();
                        yaw += shake.shake();
                    }
                    turret_eye = Some(TurretEye {
                        pos: eye,
                        angles: [pitch, yaw, roll],
                    });
                }
                out.push(DynamicModelInstance {
                    model: rig.handle,
                    transform,
                    bones: Some(pose.skin_matrices(&rig.skeleton, 0)),
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
    turret_anims.retain(|_, s| (render_time - s.last_seen_ms).abs() < STATE_TTL_MS);
    assemblies.prune_stale(render_time);
    stats.pending_assemblies = assemblies.pending.len();
    BuiltScene {
        instances: out,
        muzzles,
        weapon_flash,
        entity_pos,
        heads,
        turret_eye,
        submodels,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use vcod_common::net::msg::{ClientState, EntityState};
    use vcod_common::net::protocol::{CS_MODELS_V1, CS_TAGS_V1, PROTOCOL_V1};

    /// An item resting on a moving brush model is drawn at its own
    /// stationary `trBase` from the older snapshot, carried by what the mover
    /// does from that snapshot's time to the drawn time; one on no mover
    /// holds that `trBase` rather than lerping toward the newer snapshot.
    #[test]
    fn a_stationary_entity_rides_its_ground_mover_between_snapshots() {
        let p = &PROTOCOL_V1;
        let ent = |fields: &[(&str, i32)]| {
            let mut e = EntityState::null(p);
            for &(name, v) in fields {
                e.fields[EntityState::field_index(p, name).unwrap()] = v;
            }
            e
        };
        let bits = |v: f32| v.to_bits() as i32;
        // Rising 40 u/s from z 0 since 1000.
        let mover = ent(&[
            ("eType", 8),
            ("pos.trType", TR_LINEAR_STOP),
            ("pos.trTime", 1000),
            ("pos.trDuration", 10_000),
            ("pos.trDelta[2]", bits(40.0)),
        ]);
        let item = |z: f32, ground: i32| {
            ent(&[
                ("eType", 3),
                ("pos.trBase[2]", bits(z)),
                ("groundEntityNum", ground),
            ])
        };
        let snap = |t: i32, ents: Vec<(u32, EntityState)>| Snapshot {
            server_time: t,
            entities: ents.into_iter().collect(),
            ..Snapshot::default()
        };
        let a = snap(
            2000,
            vec![
                (100, mover.clone()),
                (101, item(40.0, 100)),
                (102, item(40.0, 1022)),
            ],
        );
        let b = snap(
            2050,
            vec![
                (100, mover),
                (101, item(42.0, 100)),
                (102, item(42.0, 1022)),
            ],
        );
        let from = LerpFrom {
            snap: &a,
            movers: SnapshotMovers::from_entities(p, &a.entities),
        };
        let at = |n: u32| lerp_pos_angles(n, &b.entities[&n], &from, 0.5, 2025, p).0.z;
        assert_eq!(at(101), 41.0, "40 plus the mover's 1 unit since 2000");
        assert_eq!(at(102), 40.0, "no mover under it: the older trBase");
    }

    /// A model's axis is `AnglesToAxis` of its angles: pitch positive puts
    /// the nose down, and the left axis is `-right`. A player only yaws.
    #[test]
    fn model_rotation_is_angles_to_axis() {
        let model = EntityVisual::Model("xmodel/prop".into());
        for a in [[30.0, 0.0, 0.0], [-20.0, 135.0, 0.0], [715.0, 40.0, 370.0]] {
            let q = model_rotation(&model, Vec3::from(a));
            let axis = vcod_common::pmove::aim::angles_to_axis(a);
            for (v, want) in [Vec3::X, Vec3::Y, Vec3::Z].into_iter().zip(axis) {
                assert!(
                    (q * v).abs_diff_eq(Vec3::from(want), 1e-4),
                    "{a:?}: {:?}",
                    q * v
                );
            }
        }
        assert!((model_rotation(&model, Vec3::new(30.0, 0.0, 0.0)) * Vec3::X).z < 0.0);
        let player = EntityVisual::Player {
            body: Default::default(),
            attachments: Vec::new(),
        };
        let q = model_rotation(&player, Vec3::new(30.0, 90.0, 10.0));
        assert!((q * Vec3::X).abs_diff_eq(Vec3::Y, 1e-5));
        assert!((q * Vec3::Z).abs_diff_eq(Vec3::Z, 1e-5));
    }

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
        // A missile draws its weapon's projectile, whatever `index` says.
        let mut grenade = ent(ET_MISSILE, 0, 0);
        grenade.fields[EntityState::field_index(p, "weapon").unwrap()] = 8;
        assert_eq!(
            resolve_visual(&grenade, &none, &cs, p),
            EntityVisual::Missile(8)
        );
        assert_eq!(
            resolve_visual(&ent(ET_MISSILE, 0, 9), &none, &cs, p),
            EntityVisual::None,
            "no weapon, no projectile"
        );
        assert_eq!(
            resolve_visual(&ent(ET_GENERAL, 0, 9), &none, &cs, p),
            EntityVisual::Model("crate_misc1".into())
        );
        assert_eq!(
            resolve_visual(&ent(ET_MOVER, 0, 10), &none, &cs, p),
            EntityVisual::Submodel(2)
        );
        for skip in [
            vcod_common::net::flags::ET_PORTAL,
            vcod_common::net::flags::ET_INVISIBLE,
            ET_EVENTS,
            ET_EVENTS + 5,
        ] {
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
    fn turret_resolves_its_model_and_weapon_unless_nodraw() {
        let p = &PROTOCOL_V1;
        let cs = cs_table();
        let none = BTreeMap::new();
        let mut gun = ent(ET_TURRET, 0, 9);
        gun.fields[EntityState::field_index(p, "weapon").unwrap()] = 16;
        assert_eq!(
            resolve_visual(&gun, &none, &cs, p),
            EntityVisual::Turret {
                model: "crate_misc1".into(),
                weapon: 16
            }
        );
        gun.fields[EntityState::field_index(p, "eFlags").unwrap()] = EF_NODRAW;
        assert_eq!(resolve_visual(&gun, &none, &cs, p), EntityVisual::None);
    }

    /// A `0xffffff` script mover's `index` is its inline model, not the
    /// model configstring slot of the same number, and `EF_NODRAW` hides it
    /// (docs/research/cod11-movers.md, section 14).
    #[test]
    fn a_brush_model_resolves_to_its_inline_model_unless_nodraw() {
        let p = &PROTOCOL_V1;
        let cs = cs_table();
        let none = BTreeMap::new();
        let mut slab = ent(ET_SCRIPTMOVER, 0, 9);
        assert_eq!(
            resolve_visual(&slab, &none, &cs, p),
            EntityVisual::Model("crate_misc1".into())
        );
        slab.fields[EntityState::field_index(p, "solid").unwrap()] = SOLID_BMODEL;
        assert_eq!(
            resolve_visual(&slab, &none, &cs, p),
            EntityVisual::Submodel(9)
        );
        slab.fields[EntityState::field_index(p, "eFlags").unwrap()] = EF_NODRAW;
        assert_eq!(resolve_visual(&slab, &none, &cs, p), EntityVisual::None);
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
}
