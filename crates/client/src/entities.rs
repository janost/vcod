//! Snapshot entities to drawables, plus the per-entity legs/torso anim driver.
//! docs/research/clientstate-wire-format.md, docs/research/player-model-anim-system.md.

use crate::camera;
use crate::fx::sim::TagPose;
use crate::renderer::{DynamicModelInstance, ModelHandle, Renderer};
use crate::turret::{self, ET_TURRET, GunAnim, MsvcRand, TurretEye};
use glam::{Mat4, Quat, Vec3};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::rc::Rc;
use vcod_common::animclock::{ChannelClock, Seen, Times};
use vcod_common::animtree::PlayerAnims;
use vcod_common::asset_source::AssetSource;
use vcod_common::collision::MASK_PLAYERSOLID;
use vcod_common::model::{self, Model};
use vcod_common::movetrace::MoveWorld;
use vcod_common::net::flags::{EF_CROUCH, EF_DEAD, EF_PRONE};
use vcod_common::net::msg::{ClientState, EntityState, PlayerState};
use vcod_common::net::protocol::Protocol;
use vcod_common::net::snapshot::Snapshot;
use vcod_common::net::trajectory::{TR_INTERPOLATE, TR_LINEAR_STOP, Trajectory};
use vcod_common::pk3::Pk3Fs;
use vcod_common::playerpose::{
    BG_SWING_SPEED, BodyAngles, BodyInput, BodySeat, BodySlope, Controllers, PoseInputs,
    SeatWeapon, pose_player_drawn, seat_inputs,
};
use vcod_common::pmove::MoveRules;
use vcod_common::pmove::movers::SnapshotMovers;
use vcod_common::skeleton::{AnimBinding, PoseBuffer, Skeleton};
use vcod_common::static_light::SceneLight;
use vcod_common::turretpose::quat_angles;
use vcod_common::turretpose::{GunnerPlacement, angles_quat, place_gunner, tag_weapon_local};
use vcod_common::vehiclepose::{self, VehicleFields};
use vcod_common::xanim::{self, XAnim};

/// `legsAnim`/`torsoAnim`: anim index in the low 9 bits, restart toggle in bit 512.
const ANIM_INDEX_MASK: i32 = 511;

/// How long a turret's anim state and a pending assembly survive unseen.
const STATE_TTL_MS: i32 = 5000;

/// New xmodel load+upload pairs per [`build_instances`] call. One costs
/// 0.2-7ms, so several new loadouts in one frame would stall it by their sum.
const ASSEMBLY_LOAD_BUDGET_PER_FRAME: u32 = 1;

/// `eFlags` bit that hides an entity's model.
use vcod_common::net::flags::EF_NODRAW;
/// `solid` of a brush model entity, whose `index` is then an inline model
/// number rather than a model configstring slot
/// (docs/research/cod11-movers.md, section 14).
pub use vcod_common::net::flags::SOLID_BMODEL;
use vcod_common::net::flags::{EF_IN_VEHICLE_V22, ET_VEHICLE_CORPSE_V22, ET_VEHICLE_V22};
pub use vcod_common::net::flags::{
    ET_CORPSE, ET_GENERAL, ET_ITEM, ET_MISSILE, ET_MOVER, ET_PLAYER, ET_SCRIPTMOVER,
};

/// A UO rider whose seat holds no weapon of his own: every seat but a
/// four-wheel's seat 3 (`BG_AllowPlayerWeaponAtVehiclePos` 0x30008210, read
/// off the rider's `surfType` by 0x300058f0).
fn seat_unarmed(eflags: i32, surf_type: i32) -> bool {
    eflags & EF_IN_VEHICLE_V22 != 0 && !(surf_type >> 3 & 7 == 1 && surf_type & 7 == 3)
}

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
    /// A United Offensive vehicle or its wreck: its xmodel, posed off the
    /// snapshot pair by `vcod_common::vehiclepose`.
    Vehicle(String),
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

/// A model entity's model slot: `index` on 1.1, `xmodel` on United
/// Offensive, whose `G_DObjUpdate` writes that field instead
/// (docs/research/coduo-diff.md 22).
fn dobj_model(ent: &EntityState, p: &Protocol) -> i32 {
    if EntityState::field_index(p, "xmodel").is_some() {
        ent.field_i32(p, "xmodel")
    } else {
        ent.field_i32(p, "index")
    }
}

pub fn resolve_visual(
    ent: &EntityState,
    clients: &BTreeMap<u32, ClientState>,
    configstrings: &[String],
    p: &Protocol,
) -> EntityVisual {
    let cs = |i: usize| configstrings.get(i).map(String::as_str).unwrap_or("");
    let (models, tags) = (p.cs.models(), p.cs.tags());
    let etype = ent.field_i32(p, "eType");
    match etype {
        ET_PLAYER | ET_CORPSE => {
            let cn = ent.field_i32(p, "clientNum") as u32;
            let Some(client) = clients.get(&cn) else {
                return EntityVisual::None;
            };
            let mi = client.field_i32(p, "modelindex");
            let Some(body) = (mi > 0)
                .then(|| model_name(cs(models + mi as usize)))
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
                let Some(m) = model_name(cs(models + am as usize)) else {
                    continue;
                };
                let at = client.field_i32(p, &format!("attachTagIndex[{i}]"));
                let tag = (at > 0)
                    .then(|| cs(tags + at as usize))
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
            let mi = if etype != ET_MOVER && ent.field_i32(p, "solid") != SOLID_BMODEL {
                dobj_model(ent, p)
            } else {
                ent.field_i32(p, "index")
            };
            if mi <= 0 || (etype == ET_SCRIPTMOVER && ent.field_i32(p, "eFlags") & EF_NODRAW != 0) {
                return EntityVisual::None;
            }
            if ent.field_i32(p, "solid") == SOLID_BMODEL {
                return EntityVisual::Submodel(mi as usize);
            }
            let s = cs(models + mi as usize);
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
            let mi = dobj_model(ent, p);
            if mi <= 0 || ent.field_i32(p, "eFlags") & EF_NODRAW != 0 {
                return EntityVisual::None;
            }
            match model_name(cs(models + mi as usize)) {
                Some(model) => EntityVisual::Turret {
                    model,
                    weapon: ent.field_i32(p, "weapon").max(0) as usize,
                },
                None => EntityVisual::None,
            }
        }
        // A United Offensive vehicle, live or burnt out: its `xmodel`, one
        // DObj callback for both (docs/research/coduo-diff.md, 8, 9, 18).
        ET_VEHICLE_V22 | ET_VEHICLE_CORPSE_V22 if p.et_events > ET_VEHICLE_CORPSE_V22 => {
            let mi = ent.field_i32(p, "xmodel");
            if mi <= 0 || ent.field_i32(p, "eFlags") & EF_NODRAW != 0 {
                return EntityVisual::None;
            }
            match model_name(cs(models + mi as usize)) {
                Some(m) => EntityVisual::Vehicle(m),
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
    /// The skeleton's clip bindings, kept from pose to pose.
    pub binds: vcod_common::skeleton::BindCache,
    #[allow(dead_code)] // handles.len() is what the draw path uses
    pub model_count: usize,
}

/// Loaded+uploaded player parts by xmodel name, shared across assemblies.
/// Holds the `Model` too, because `Skeleton::build_grafted` re-reads bone
/// data for every assembly combination. `None` is a failed part, never retried.
type PartCache = HashMap<String, Option<(Rc<Model>, ModelHandle)>>;

/// One attachment's model, graft tag and GPU handle, kept together so a
/// dropped attachment never desyncs `Assembly::handles` from the model list.
type AttachmentPart = (Rc<Model>, Option<String>, ModelHandle);

/// One assembly's load progress, advanced one part per [`AssemblyCache::resolve`]
/// call. A slot is `None` until attempted, `Some(None)` once attempted and failed.
struct PendingAssembly {
    body: String,
    attachments: Vec<(String, Option<String>)>,
    body_part: Option<Option<(Rc<Model>, ModelHandle)>>,
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
) -> Option<(Rc<Model>, ModelHandle)> {
    let outcome = match model::load(fs, name) {
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

    let mut refs: Vec<(&Model, Option<&str>)> = Vec::with_capacity(1 + parts.len());
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
        binds: Default::default(),
    }))
}

/// A turret's anim channel. `raw` is the last wire value; the server flips
/// the toggle bit on every anim (re)start, the only signal that a repeat of
/// the same anim should restart from frame 0.
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
    /// `bg_swingSpeed`, which the caller keeps in step with the console.
    pub swing_speed: f32,
    /// The view this frame draws, wire degrees, which a ridden UO gun turns
    /// with (uo cgame 0x3001e9f0); the caller sets it before the pass.
    pub view: Option<[f32; 3]>,
    pub stats: SceneStats,
    /// Where each entity with an xmodel drew this frame, for
    /// [`EntityScene::tag_pose`].
    drawn: HashMap<u32, Drawn>,
    /// Bind-pose bones of plain entity models by xmodel name, loaded on the
    /// first tag asked of one. `None` is a failed load.
    model_rigs: HashMap<String, Option<ModelRig>>,
    /// Vehicle models by xmodel name. `None` is a failed load, warned once.
    vehicle_rigs: HashMap<String, Option<VehicleRig>>,
    /// Each vehicle's posed bones this frame, model space.
    vehicle_worlds: HashMap<u32, Vec<(Vec3, Quat)>>,
}

/// One entity's drawn frame and the skeleton its tags come off.
#[derive(Clone)]
struct Drawn {
    pos: Vec3,
    rot: Quat,
    rig: Rig,
}

#[derive(Clone)]
enum Rig {
    /// Posed in [`EntityScene::states`].
    Player,
    /// Posed in [`EntityScene::turret_anims`], skeleton in `turret_rigs`.
    Turret(String),
    /// Unanimated: its bind pose (`EntityScene::model_rigs`).
    Model(String),
    /// Posed in [`EntityScene::vehicle_worlds`], skeleton in `vehicle_rigs`.
    Vehicle(String),
}

/// One uploaded vehicle model and its skeleton.
struct VehicleRig {
    handle: ModelHandle,
    skeleton: Skeleton,
}

/// One vehicle this frame: where it draws, its posed bones (model space)
/// and the material clock.
struct VehicleFrame {
    model: String,
    pos: Vec3,
    rot: Quat,
    pose: vehiclepose::VehiclePose,
    worlds: Vec<(Vec3, Quat)>,
    skins: Vec<Mat4>,
}

impl VehicleFrame {
    /// The vehicle at `pos` turned by `angles` (degrees), its controls lerped
    /// from `cur` to `next` by `f`, then `own` (the client's own seat: the
    /// steering wheel's ratio, the own gunner's gun) applied.
    fn posed(
        skel: &Skeleton,
        model: String,
        (pos, angles): (Vec3, Vec3),
        (cur, next, f): (&VehicleFields, &VehicleFields, f32),
        own: impl FnOnce(&mut vehiclepose::VehiclePose),
    ) -> VehicleFrame {
        let mut pose = vehiclepose::lerp(cur, next, f);
        own(&mut pose);
        let mut bones = PoseBuffer::new(skel);
        vehiclepose::apply(&mut bones, skel, &pose);
        VehicleFrame {
            model,
            pos,
            rot: angles_quat(angles.to_array()),
            pose,
            worlds: bones.bone_worlds(skel),
            skins: bones.skin_matrices(skel, 0),
        }
    }
}

/// The steering wheel's roll a degree of steer on vehicle `num`: four
/// times the steer when `cg.snap`'s player (`snap`, the older of the pair)
/// drives it unsighted (cgame 0x30020b8b), 0.75 otherwise.
fn steering_ratio(snap: &PlayerState, num: u32, p: &Protocol) -> f32 {
    let driving = snap.field_i32(p, "eFlags") & EF_IN_VEHICLE_V22 != 0
        && snap.field_i32(p, "vehPos") == 1
        && snap.field_i32(p, "viewlocked_entNum") == num as i32;
    if driving && snap.field_f32(p, "fWeaponPosFrac") == 0.0 {
        vehiclepose::STEERING_RATIO_OWN
    } else {
        vehiclepose::STEERING_RATIO
    }
}

/// Whether `cg.snap`'s player (`snap`, the older of the pair) mans vehicle
/// `num`'s second gun, whose gun then follows the view (cgame 0x300207c0).
fn own_gunner(snap: &PlayerState, num: u32, p: &Protocol) -> bool {
    snap.field_i32(p, "eFlags") & EF_IN_VEHICLE_V22 != 0
        && snap.field_i32(p, "vehPos") == 2
        && snap.field_i32(p, "viewlocked_entNum") == num as i32
}

/// Where a UO rider in seat `surf_type & 7` of `v` draws, and the seat his
/// controllers and seat blend read: the seat tag as the vehicle is posed
/// this frame, whose angles 0x30033a77 writes into the client info's
/// `+0x3f4` (cgame 0x30032fe0). `None` where the seat has no tag.
fn rider_frame(
    skel: &Skeleton,
    v: &VehicleFrame,
    surf_type: i32,
    weapon: i32,
) -> Option<(Vec3, Quat, BodySeat)> {
    let seat = vehiclepose::rider_seat(surf_type);
    let (at, r) = vehiclepose::seat_frame(skel, &v.worlds, (v.pos, v.rot), v.pose.kind, seat)?;
    let body = BodySeat::new(quat_angles(r), v.pose.kind, seat, v.pose.steer)
        .drawn()
        .armed(weapon);
    Some((at, r, body))
}

/// A drawn player's spine and `tag_origin` controls as the hook aims them:
/// a live rider's off his seat (cgame 0x30004da0 on `eFlags` 0x100000),
/// anyone else's off the swung body angles; `None` for a 1.1 gunner.
fn body_controllers(
    rules: MoveRules,
    rider: Option<&BodySeat>,
    body: &BodyAngles,
    input: &BodyInput,
    slope: &BodySlope,
) -> Option<Controllers> {
    match rider {
        Some(seat) if input.eflags & EF_DEAD == 0 => Some(Controllers::rider(input, seat, slope)),
        _ => Controllers::target(rules, body, input, slope),
    }
}

/// A plain model's skeleton and its bind-pose bone frames, model space.
struct ModelRig {
    skeleton: Skeleton,
    worlds: Vec<(Vec3, Quat)>,
}

/// `name`'s bone index under the case fold the server's tag lookup uses.
fn bone_named(skel: &Skeleton, name: &str) -> Option<usize> {
    skel.bones()
        .iter()
        .position(|b| b.name.eq_ignore_ascii_case(name))
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
    /// UO's ridden gun: its drawn pitch and yaw and the arcs the view is
    /// held in.
    uo: Option<([f32; 2], turret::UoArcs)>,
    /// `tag_weapon` turned by the barrel, in model space.
    tag_weapon: Option<(Vec3, Quat)>,
    /// The weapon file's `animHorRotateInc`.
    rotate_inc: f32,
}

struct TurretAnim {
    channel: Channel,
    last_seen_ms: i32,
    /// This frame's posed bones, model space.
    worlds: Vec<(Vec3, Quat)>,
}

/// Debug-overlay counters. `anim_restarts` is cumulative; `pending_assemblies`
/// is a snapshot of the last pass.
#[derive(Default)]
pub struct SceneStats {
    /// Wire anim changes across all entities, legs and torso.
    pub anim_restarts: u64,
    pub pending_assemblies: usize,
}

impl EntityScene {
    /// An effect's debris model (`fx::sim::FxModel`) by its `xmodel/...`
    /// path, through the entity model cache.
    pub fn fx_model(
        &mut self,
        renderer: &mut Renderer,
        fs: &Pk3Fs,
        path: &str,
    ) -> Option<ModelHandle> {
        let name = path.strip_prefix("xmodel/").unwrap_or(path);
        resolve_model(&mut self.model_cache, renderer, fs, name)
    }

    /// `name` on entity `num` as drawn this frame, in the world: the
    /// entity's drawn origin and rotation times the bone's posed frame,
    /// what `PlayEffect`'s bolt reads (CoDMP.exe 0x49e8d0,
    /// docs/research/efx-grammar.md R10). `None` for an entity not drawn
    /// with an xmodel this frame or a model without the tag.
    pub fn tag_pose(&mut self, fs: &dyn AssetSource, num: u32, name: &str) -> Option<TagPose> {
        let d = self.drawn.get(&num)?.clone();
        let (lp, lr) = match &d.rig {
            Rig::Player => {
                let st = self.states.get(&num)?;
                st.pose
                    .bone_world(&st.skeleton, bone_named(&st.skeleton, name)?)
            }
            Rig::Turret(model) => {
                let rig = self.turret_rigs.get(model)?.as_ref()?;
                let bi = bone_named(&rig.skeleton, name)?;
                *self.turret_anims.get(&num)?.worlds.get(bi)?
            }
            Rig::Vehicle(model) => {
                let rig = self.vehicle_rigs.get(model)?.as_ref()?;
                let bi = bone_named(&rig.skeleton, name)?;
                *self.vehicle_worlds.get(&num)?.get(bi)?
            }
            Rig::Model(model) => {
                let parts = &self.parts;
                let rig = self
                    .model_rigs
                    .entry(model.clone())
                    .or_insert_with(|| {
                        // The level prewarm's copy, else read now.
                        let m = match parts.get(model.as_str()) {
                            Some(Some((m, _))) => Ok(m.clone()),
                            _ => model::load(fs, model).map(Rc::new),
                        };
                        match m {
                            Ok(m) => {
                                let skeleton = Skeleton::build(&[&*m]);
                                let worlds = PoseBuffer::new(&skeleton).bone_worlds(&skeleton);
                                Some(ModelRig { skeleton, worlds })
                            }
                            Err(e) => {
                                log::warn!("tag on entity model '{model}': {e:#}");
                                None
                            }
                        }
                    })
                    .as_ref()?;
                rig.worlds[bone_named(&rig.skeleton, name)?]
            }
        };
        let rot = d.rot * lr;
        Some(TagPose {
            origin: d.pos + d.rot * lp,
            axis: [rot * Vec3::X, rot * Vec3::Y, rot * Vec3::Z],
        })
    }

    /// Loads the player animtree and uploads every xmodel the level's
    /// configstrings register, as entity models and as player parts, at
    /// the map load: not in the frame that first draws one
    /// (cod11-client-soak.md). Returns the models uploaded.
    pub fn prewarm(
        &mut self,
        renderer: &mut Renderer,
        fs: &Pk3Fs,
        configstrings: &[String],
        p: &Protocol,
    ) -> usize {
        use vcod_common::net::protocol::CsRange;
        if self.anims.is_none() {
            self.anims = Some(PlayerAnims::load(fs).ok());
        }
        let (first, last) = p.cs.bounds(CsRange::Model);
        let mut names: Vec<String> = configstrings
            .iter()
            .take(last + 1)
            .skip(first)
            .filter_map(|s| model_name(s))
            .collect();
        // A dropped weapon draws its file's world model, an ammo or health
        // row its own (`EntityVisual::Item`).
        let cs7 = configstrings.get(7).map(String::as_str).unwrap_or("");
        for w in split_weapon_list(cs7) {
            if let Some(m) = resolve_weapon_def(&mut self.weapon_cache, fs, &w)
                .and_then(|d| d.world_model.clone())
            {
                names.push(m);
            }
        }
        names.extend(
            (0..p.max_weapons + 8).filter_map(|i| static_item_model(p, i).map(str::to_string)),
        );
        let mut seen = HashSet::new();
        names.retain(|n| {
            !n.starts_with('*') && !self.parts.contains_key(n) && seen.insert(n.clone())
        });
        let loaded: Vec<(String, Model)> = names
            .into_iter()
            .filter_map(|name| Some((name.clone(), model::load(fs, &name).ok()?)))
            .collect();
        let refs: Vec<&Model> = loaded.iter().map(|(_, m)| m).collect();
        let handles = renderer.upload_dynamic_models(fs, &refs);
        let mut n = 0;
        for ((name, m), handle) in loaded.into_iter().zip(handles) {
            self.model_cache.entry(name.clone()).or_insert(handle);
            if let Some(h) = handle {
                self.parts.insert(name, Some((Rc::new(m), h)));
                n += 1;
            }
        }
        n
    }

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
            swing_speed: BG_SWING_SPEED,
            view: None,
            stats: SceneStats::default(),
            drawn: HashMap::new(),
            model_rigs: HashMap::new(),
            vehicle_rigs: HashMap::new(),
            vehicle_worlds: HashMap::new(),
        }
    }
}

impl Default for EntityScene {
    fn default() -> Self {
        Self::new()
    }
}

/// One player's or corpse's playback state, kept per entity number for the
/// cgame's life as retail's `clientInfo` lerp frames are.
struct EntityAnim {
    /// Assembly this pose was sized for; a change rebuilds against the new skeleton.
    key: Vec<String>,
    /// The assembly's skeleton `pose` is shaped for.
    skeleton: Rc<Skeleton>,
    pose: PoseBuffer,
    /// The cgame's two lerp frames (`ci+0x37c`, `ci+0x3ac`) and the DObj's
    /// clips under them (cod11-movers.md 15, "The drawn body's clip").
    legs: ChannelClock,
    torso: ChannelClock,
    /// Roster-resolved visual from the last frame this entity resolved (body
    /// plus attachments, no held weapon). A corpse re-resolves through the
    /// dead client's live roster entry, which clears when they drop to limbo;
    /// the corpse then draws this instead of vanishing.
    visual: EntityVisual,
    /// The record's legs and torso swings (combat doc 16.3, 16.4).
    body: BodyAngles,
    /// 1.5's and UO's controls as eased so far (cgame uo 0x30005730).
    controls: Option<Controllers>,
    /// 1.5's and UO's stance window (`ci+0x4a4` 1.5b, `ci+0x4c0` UO):
    /// `animclock::ChannelClock::update_windowed`.
    stance_window_ms: i32,
    seat: SeatLeaves,
    /// The render time of the last frame the entity was in the snapshot;
    /// its clips hold still while it is not (cgame 0x3001bd00).
    present_ms: i32,
}

/// The cgame's two channel updates (0x30004e40's clears and 0x30003be0;
/// 1.5 0x30003870 and UO 0x30003a90 under the stance window,
/// coduo-vehicles.md 13), through `animclock::update_body`.
fn update_channels(
    (legs, torso, window): (&mut ChannelClock, &mut ChannelClock, &mut i32),
    anims: &PlayerAnims,
    rules: MoveRules,
    (legs_wire, torso_wire): (i32, i32),
    seen: &Seen,
    at: Times,
) {
    let window = (rules != MoveRules::V1_1).then_some(window);
    let wires = (legs_wire, torso_wire);
    vcod_common::animclock::update_body((legs, torso), anims, wires, seen, at, window);
}

/// A rider's seat node and leaves this frame, and those of the node the
/// last seat change left.
#[derive(Clone, Default)]
struct SeatLeaves {
    goal: Option<(usize, Vec<(usize, f32)>)>,
    left: Option<(usize, Vec<(usize, f32)>)>,
}

impl SeatLeaves {
    /// This frame's seat goal, then the leaves of the node the last seat
    /// change left, which the old legs clip fades out on: nothing in the
    /// cgame clears them (coduo-vehicles.md 13, "The glide back").
    fn next(
        &mut self,
        goal: Option<(usize, Vec<(usize, f32)>)>,
    ) -> Vec<(usize, Vec<(usize, f32)>)> {
        let node = goal.as_ref().map(|g| g.0);
        if self.goal.as_ref().map(|g| g.0) != node {
            self.left = self.goal.take();
        }
        if self.left.as_ref().is_some_and(|g| Some(g.0) == node) {
            self.left = None;
        }
        self.goal = goal.clone();
        goal.into_iter().chain(self.left.clone()).collect()
    }
}

impl EntityAnim {
    fn new(key: Vec<String>, skel: &Rc<Skeleton>, now_ms: i32) -> EntityAnim {
        EntityAnim {
            key,
            skeleton: skel.clone(),
            pose: PoseBuffer::drawn(skel),
            legs: ChannelClock::default(),
            torso: ChannelClock::default(),
            visual: EntityVisual::None,
            body: BodyAngles::default(),
            controls: None,
            stance_window_ms: 0,
            seat: SeatLeaves::default(),
            present_ms: now_ms,
        }
    }
}

/// The model of a compiled-in ammo or health item row, without the
/// `xmodel/` prefix the table spells (`weapon_table::STATIC_ITEMS`).
fn static_item_model(
    p: &vcod_common::net::protocol::Protocol,
    index: usize,
) -> Option<&'static str> {
    let (_, model) = vcod_common::weapon_table::static_item(p, index)?;
    Some(model.strip_prefix("xmodel/").unwrap_or(model))
}

/// Loads+uploads an xmodel by name (no `xmodel/` prefix), caching failures as `None`.
pub(crate) fn resolve_model(
    cache: &mut HashMap<String, Option<ModelHandle>>,
    renderer: &mut Renderer,
    fs: &Pk3Fs,
    name: &str,
) -> Option<ModelHandle> {
    if let Some(h) = cache.get(name) {
        return *h;
    }
    let handle = match model::load(fs, name) {
        Ok(m) => renderer.upload_dynamic_model(fs, &m),
        Err(e) => {
            log::warn!("live entity model '{name}': {e:#}, drawing nothing for it");
            None
        }
    };
    cache.insert(name.to_string(), handle);
    handle
}

/// [`resolve_model`] over `names`, the ones not cached loaded first and
/// uploaded as one batch ([`Renderer::upload_dynamic_models`]).
pub(crate) fn resolve_models(
    cache: &mut HashMap<String, Option<ModelHandle>>,
    renderer: &mut Renderer,
    fs: &Pk3Fs,
    names: &[String],
) {
    let mut loaded: Vec<(&str, Model)> = Vec::new();
    for name in names {
        if cache.contains_key(name) || loaded.iter().any(|(n, _)| n == name) {
            continue;
        }
        match model::load(fs, name) {
            Ok(m) => loaded.push((name, m)),
            Err(e) => {
                log::warn!("live entity model '{name}': {e:#}, drawing nothing for it");
                cache.insert(name.to_string(), None);
            }
        }
    }
    let refs: Vec<&Model> = loaded.iter().map(|(_, m)| m).collect();
    let handles = renderer.upload_dynamic_models(fs, &refs);
    for ((name, _), h) in loaded.iter().zip(handles) {
        cache.insert(name.to_string(), h);
    }
}

/// `name` as the level prewarm uploaded it ([`EntityScene::prewarm`]), else
/// loaded and uploaded now.
fn loaded(
    parts: &PartCache,
    renderer: &mut Renderer,
    fs: &Pk3Fs,
    name: &str,
) -> anyhow::Result<(Rc<Model>, Option<ModelHandle>)> {
    if let Some(Some((m, h))) = parts.get(name) {
        return Ok((m.clone(), Some(*h)));
    }
    let m = model::load(fs, name)?;
    let h = renderer.upload_dynamic_model(fs, &m);
    Ok((Rc::new(m), h))
}

/// Loads and uploads a turret's xmodel with its skeleton, caching failures as `None`.
fn resolve_turret_rig<'a>(
    cache: &'a mut HashMap<String, Option<TurretRig>>,
    parts: &PartCache,
    renderer: &mut Renderer,
    fs: &Pk3Fs,
    name: &str,
) -> Option<&'a mut TurretRig> {
    if !cache.contains_key(name) {
        let rig = match loaded(parts, renderer, fs, name) {
            Ok((m, handle)) => handle.map(|handle| {
                let skeleton = Skeleton::build(&[&*m]);
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

/// Loads and uploads a vehicle's xmodel with its skeleton, caching failures
/// as `None`.
fn resolve_vehicle_rig<'a>(
    cache: &'a mut HashMap<String, Option<VehicleRig>>,
    parts: &PartCache,
    renderer: &mut Renderer,
    fs: &Pk3Fs,
    name: &str,
) -> Option<&'a VehicleRig> {
    if !cache.contains_key(name) {
        let rig = match loaded(parts, renderer, fs, name) {
            Ok((m, handle)) => handle.map(|handle| VehicleRig {
                handle,
                skeleton: Skeleton::build(&[&*m]),
            }),
            Err(e) => {
                log::warn!("vehicle model '{name}': {e:#}, drawing nothing for it");
                None
            }
        };
        cache.insert(name.to_string(), rig);
    }
    cache.get(name).unwrap().as_ref()
}

/// Loads the weapon file for a CS 7 name, caching failures as `None`.
fn resolve_weapon_def<'a>(
    cache: &'a mut HashMap<String, Option<vcod_common::weapon::WeaponDef>>,
    fs: &dyn AssetSource,
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
    fs: &dyn AssetSource,
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
    fs: &dyn AssetSource,
    weapons: &[String],
    weapon_index: i32,
) -> Option<&'a str> {
    let name = weapon_name_for_index(weapons, weapon_index)?;
    let def = resolve_weapon_def(weapon_cache, fs, name)?;
    def.world_flash_effect.as_deref()
}

/// A player anim clip, cached by name.
fn load_clip(
    cache: &mut HashMap<String, Option<Rc<XAnim>>>,
    fs: &dyn AssetSource,
    name: &str,
) -> Option<Rc<XAnim>> {
    xanim::load_cached(cache, fs, name, "player anim")
}

/// [`build_instances`]'s result.
pub struct BuiltScene {
    pub instances: Vec<DynamicModelInstance>,
    /// Per player entity number: world-space muzzle position and forward, from
    /// the held weapon's `tag_flash`. Only entities that drew a held weapon
    /// with that tag this frame.
    pub muzzles: HashMap<u32, (Vec3, Vec3)>,
    /// Per drawn player body with no `tag_flash`: where `CG_CalcMuzzlePoint`
    /// starts its tracers instead ([`crate::fx::registry::muzzle_fallback`]).
    /// It plays no flash there.
    pub shot_fallbacks: HashMap<u32, Vec3>,
    /// Per player entity number: the held weapon's `tag_brass` and its
    /// forward, world space, where `CG_EjectBrass` plays the spent case.
    pub brass: HashMap<u32, (Vec3, Vec3)>,
    /// Per 1-based CS7 weapon index: its `worldFlashEffect` path, for
    /// `fx::registry::ResolveCtx`. Only weapons held by a drawn entity this
    /// frame, and only when the weapon file sets the key.
    pub weapon_flash: HashMap<i32, String>,
    /// Per drawn entity number: interpolated world position, for
    /// entity-attached sound voices. Every drawn entity, not only players.
    pub entity_pos: HashMap<u32, Vec3>,
    /// Per entry of `entity_pos`: its interpolated angles, the centity's
    /// `lerpAngles` (+0x204), the frame its tag voices turn with. A player's
    /// are its view's, pitch included, not the yaw it draws with.
    pub entity_angles: HashMap<u32, Vec3>,
    /// Per drawn `ET_PLAYER`: the world position of its `Bip01 Head` bone,
    /// where the head icon hangs (`hud::head_icon`).
    pub heads: HashMap<u32, Vec3>,
    /// The gun `b.ps` rides, when it was drawn: the first-person eye.
    pub turret_eye: Option<TurretEye>,
    /// Inline models drawn this frame with their entity's pose, the identity
    /// for one where the map put it ([`Renderer::set_submodels`]).
    pub submodels: Vec<(usize, Mat4)>,
    /// The cgame's scene lights in entity order: `constantLight` and a
    /// missile's `projectileDLight` ([`Renderer::set_entity_lights`]).
    pub scene_lights: Vec<SceneLight>,
    /// Where each drawn live `ET_PLAYER` stands, for `CG_PlayerShadow`
    /// (`crate::blob_shadow`): `CG_Player` (0x30028210) draws none with
    /// `EF_NODRAW` or `EF_DEAD`, and a corpse never casts one.
    pub shadow_casters: Vec<Vec3>,
    /// Per drawn UO vehicle: its interpolated angles, the centity's
    /// `lerpAngles` (+0x214) a seat's prediction and turn read.
    pub vehicle_angles: HashMap<u32, [f32; 3]>,
    /// Per drawn UO vehicle: its wheels, for the tread and wheel dust
    /// (`fx::vehicle_trails`).
    pub vehicle_wheels: Vec<crate::fx::vehicle_trails::Wheels>,
}

/// An entity's `constantLight` as `CG_AddCEntity` (0x3001af20) adds it:
/// intensity the top byte times 4, red, green, blue the low bytes / 255.
pub fn constant_light(cl: u32, origin: Vec3) -> Option<SceneLight> {
    (cl != 0).then(|| SceneLight {
        origin,
        color: Vec3::new(
            (cl & 0xff) as f32,
            (cl >> 8 & 0xff) as f32,
            (cl >> 16 & 0xff) as f32,
        ) / 255.0,
        intensity: ((cl >> 24) * 4) as f32,
    })
}

/// Entity numbers below this are clients (`MAX_CLIENTS`).
const MAX_CLIENTS: u32 = vcod_common::net::protocol::MAX_CLIENTS as u32;

/// The older snapshot of the pair an entity is drawn between, with its
/// movers, which is what retail's `cg.snap` holds.
struct LerpFrom<'a> {
    snap: &'a Snapshot,
    movers: SnapshotMovers,
}

/// Where an entity model samples the light grid (`RF_LIGHTING_ORIGIN`). A
/// player or corpse is lit at its lerped origin raised by `fTorsoHeight`
/// and 32 standing, 20 crouched, 12 prone (cgame 0x30028210, 0x30028400);
/// a turret 32 above its origin (0x3001b2e0); anything else at the drawn
/// origin (cod11-light-grid-and-leaf-lights.md, section 13).
fn light_origin(etype: i32, ent: &EntityState, p: &Protocol, lerped: Vec3, drawn: Vec3) -> Vec3 {
    match etype {
        ET_PLAYER | ET_CORPSE => {
            let eflags = ent.field_i32(p, "eFlags");
            let stance = if eflags & EF_PRONE != 0 {
                12.0
            } else if eflags & EF_CROUCH != 0 {
                20.0
            } else {
                32.0
            };
            lerped + Vec3::Z * (ent.field_f32(p, "fTorsoHeight") + stance)
        }
        ET_TURRET => lerped + Vec3::Z * 32.0,
        _ => drawn,
    }
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
    fs: &dyn AssetSource,
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

/// Builds this frame's live-entity draw list from the interpolation pair
/// `(a, b, f)` the camera uses. `latest_snapshot_ms` is the newest received
/// snapshot's `serverTime`, the clock a body's anim rate is measured on. `skip_num` is the entity the camera is inside
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
    latest_snapshot_ms: i32,
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
        swing_speed,
        view,
        stats,
        drawn,
        model_rigs: _,
        vehicle_rigs,
        vehicle_worlds,
    } = scene;
    drawn.clear();
    vehicle_worlds.clear();
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
    let mut shot_fallbacks: HashMap<u32, Vec3> = HashMap::new();
    let mut brass: HashMap<u32, (Vec3, Vec3)> = HashMap::new();
    let mut weapon_flash: HashMap<i32, String> = HashMap::new();
    let mut entity_pos: HashMap<u32, Vec3> = HashMap::new();
    let mut entity_angles: HashMap<u32, Vec3> = HashMap::new();
    let mut heads: HashMap<u32, Vec3> = HashMap::new();
    let mut turret_eye = None;
    let mut submodels = Vec::new();
    let mut scene_lights = Vec::new();
    let mut shadow_casters = Vec::new();
    let ps_int = |name: &str| b.ps.field_i32(p, name);
    let uo = p.is_uo();
    let rules = MoveRules::for_protocol(p);
    let ridden = turret::ridden(
        ps_int("eFlags"),
        ps_int("viewlocked"),
        ps_int("viewlocked_entNum"),
        uo,
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
        let Some(rig) = resolve_turret_rig(turret_rigs, parts, renderer, fs, &model) else {
            continue;
        };
        let prev = a.entities.get(&num);
        let (pos, angles) = lerp_pos_angles(num, ent, &from, f, render_time, p);
        let a2 =
            |e: &EntityState| ["angles2[0]", "angles2[1]", "angles2[2]"].map(|n| e.field_f32(p, n));
        let eflags = |e: &EntityState| e.field_i32(p, "eFlags");
        let from = prev.filter(|ea| turret::interpolates(eflags(ea), eflags(ent)));
        let mut barrel = turret::barrel(from.map(a2), a2(ent), f);
        // 1.5's and UO's ridden gun turns with the view, inside its arcs
        // (1.5 0x3001c2b0, UO 0x3001e9f0).
        let uo_gun = (!p.is_v1() && ridden == Some(num)).then(|| {
            let arcs = |e: &EntityState| turret::UoArcs::FIELDS.map(|n| e.field_f32(p, n));
            let arcs = turret::UoArcs::lerp(arcs(prev.unwrap_or(ent)), arcs(ent), f);
            ([angles.x, angles.y], arcs)
        });
        if let (Some((base, arcs)), Some(v)) = (&uo_gun, *view) {
            [barrel[0], barrel[1]] = arcs.barrel(v, *base);
        }
        let rotate_inc = weapon_name_for_index(&weapon_names, weapon as i32)
            .and_then(|name| resolve_weapon_def(weapon_cache, fs, name))
            .map_or(0.0, |d| d.anim_hor_rotate_inc);
        guns.insert(
            num,
            GunFrame {
                pos,
                rot: angles_quat(angles.to_array()),
                barrel,
                uo: uo_gun,
                tag_weapon: rig
                    .aim_tags
                    .map(|(aim, weapon)| tag_weapon_local(aim, weapon, barrel)),
                rotate_inc,
            },
        );
    }
    // The vehicles next: a seated player draws at its vehicle's posed seat.
    let mut vehicles: HashMap<u32, VehicleFrame> = HashMap::new();
    let mut vehicle_angles = HashMap::new();
    let mut vehicle_wheels = Vec::new();
    for (&num, ent) in &b.entities {
        let EntityVisual::Vehicle(model) = resolve_visual(ent, &b.clients, configstrings, p) else {
            continue;
        };
        let Some(rig) = resolve_vehicle_rig(vehicle_rigs, parts, renderer, fs, &model) else {
            continue;
        };
        let (pos, angles) = lerp_pos_angles(num, ent, &from, f, render_time, p);
        if !pos.is_finite() || !angles.is_finite() {
            continue;
        }
        vehicle_angles.insert(num, angles.to_array());
        let next = VehicleFields::read(ent, p);
        let cur = a
            .entities
            .get(&num)
            .map_or(next, |e| VehicleFields::read(e, p));
        let v = VehicleFrame::posed(
            &rig.skeleton,
            model,
            (pos, angles),
            (&cur, &next, f),
            |pose| {
                pose.steering_ratio = steering_ratio(&a.ps, num, p);
                if let Some(v) = view.filter(|_| own_gunner(&a.ps, num, p)) {
                    vehiclepose::own_gunner(pose, v, angles.to_array(), cur.gunner_spans);
                }
            },
        );
        vehicle_wheels.push(crate::fx::vehicle_trails::Wheels {
            entity: num,
            four_wheel: v.pose.kind == vehiclepose::KIND_FOUR_WHEEL,
            forward: v.rot * Vec3::X,
            up: v.rot * Vec3::Z,
            reach: ent.field_i32(p, "time2") as f32 / 1000.0,
            points: vehiclepose::wheel_points(&rig.skeleton, &v.worlds, (v.pos, v.rot)),
            collmap: (ent.field_i32(p, "solid") == vcod_common::net::flags::SOLID_BMODEL)
                .then(|| ent.field_i32(p, "index"))
                .filter(|&i| i > 0)
                .map(|i| (i as usize, pos, angles)),
        });
        vehicles.insert(num, v);
    }
    for (&num, ent) in &b.entities {
        // The body the camera is inside is posed, for the tags `playFxOnTag`
        // reads, and never drawn (cgame 0x30028210's `renderfx` 2).
        let hidden = num as i32 == skip_num;
        let etype = ent.field_i32(p, "eType");
        if hidden && etype != ET_PLAYER {
            continue;
        }
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
        // every entity's constantLight, drawn or not (0x3001d5f0)
        let cl = ent.field_i32(p, "constantLight") as u32;
        if cl == 0 && matches!(visual, EntityVisual::None) {
            continue;
        }
        let (mut pos, angles) = lerp_pos_angles(num, ent, &from, f, render_time, p);
        if let Some(l) = constant_light(cl, pos).filter(|l| !hidden && l.origin.is_finite()) {
            scene_lights.push(l);
        }
        if matches!(visual, EntityVisual::None) {
            continue;
        }

        let prev = a.entities.get(&num);
        if !pos.is_finite() || !angles.is_finite() {
            continue; // never feed a NaN transform to the GPU
        }
        let mut rot = model_rotation(&visual, angles);
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
            rot = Quat::from_rotation_z(g.yaw().to_radians());
        }
        // A seated player draws at its seat as its vehicle is posed this
        // frame, turned with the seat (cgame 0x30032fe0), or not at all.
        let mut seat_hidden = false;
        // The seat the controllers and the seat blend read (0x30033a77
        // writes the tag's angles into the client info's `+0x3f4`).
        let mut rider = None;
        if etype == ET_PLAYER
            && p.et_events > ET_VEHICLE_CORPSE_V22
            && ent.field_i32(p, "eFlags") & EF_IN_VEHICLE_V22 != 0
            && let Some(v) = vehicles.get(&(ent.field_i32(p, "otherEntityNum") as u32))
            && let Some(rig) = vehicle_rigs.get(&v.model).and_then(Option::as_ref)
        {
            let surf_type = ent.field_i32(p, "surfType");
            seat_hidden = !vehiclepose::seat_drawn(v.pose.kind, vehiclepose::rider_seat(surf_type));
            if let Some((at, r, seat)) =
                rider_frame(&rig.skeleton, v, surf_type, ent.field_i32(p, "weapon"))
            {
                pos = at;
                rot = r;
                rider = Some(seat);
            }
        }
        if !hidden {
            entity_pos.insert(num, pos);
            entity_angles.insert(num, angles);
        }
        if seat_hidden {
            continue;
        }
        // `CG_Player` (0x30028210) draws no player with `EF_NODRAW` or
        // `EF_DEAD`: what a death leaves lying is the corpse `cloneplayer`
        // makes. Its events still sound from where it stands.
        if etype == ET_PLAYER && ent.field_i32(p, "eFlags") & (EF_DEAD | EF_NODRAW) != 0 {
            continue;
        }
        if !hidden && etype == ET_PLAYER && matches!(visual, EntityVisual::Player { .. }) {
            shadow_casters.push(pos);
        }
        let transform = Mat4::from_rotation_translation(rot, pos);
        let light_origin = light_origin(etype, ent, p, snap_pos, pos);

        match visual {
            EntityVisual::Player {
                body,
                mut attachments,
            } => {
                let _bodies = crate::frame_prof::span(crate::frame_prof::Stage::Bodies);
                // Kept before the held weapon joins: the corpse fallback
                // re-resolves the weapon from its own entityState.
                let roster_visual = EntityVisual::Player {
                    body: body.clone(),
                    attachments: attachments.clone(),
                };
                // The held weapon is one more attachment on tag_weapon_right,
                // so a weapon switch changes the assembly key like a gear change.
                // A gunner's is not drawn (0x30004eb0 zeroes it on `eFlags & 0xc000`),
                // nor a UO rider's but in the seat that holds a weapon (0x300058f0).
                let weapon_index = if ent.field_i32(p, "eFlags") & turret::EF_MOUNTED != 0
                    || seat_unarmed(ent.field_i32(p, "eFlags"), ent.field_i32(p, "surfType"))
                {
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
                // A corpse keeps lerp frames of its own (cgame 0x30028400
                // indexes them by body queue slot), so its death clip plays
                // from the start while `eFlags` 0x800 marks it fresh, and
                // shows its last frame otherwise. A player's are his
                // client's. Neither expires while unseen (movers doc 15).
                states.entry(num).or_insert_with(|| {
                    EntityAnim::new(key.clone(), &assembly.skeleton, render_time)
                });
                let st = states.get_mut(&num).expect("inserted above");
                if st.key != key {
                    // The clocks hold only anim indices and times, so carry
                    // them across a loadout change instead of restarting the
                    // clip. Only the pose buffer is skeleton-shaped.
                    *st = EntityAnim {
                        key: key.clone(),
                        skeleton: assembly.skeleton.clone(),
                        pose: PoseBuffer::drawn(&assembly.skeleton),
                        legs: st.legs,
                        torso: st.torso,
                        visual: EntityVisual::None,
                        body: st.body,
                        controls: st.controls,
                        stance_window_ms: st.stance_window_ms,
                        seat: st.seat.clone(),
                        present_ms: st.present_ms,
                    };
                }
                st.visual = roster_visual;
                let (legs_wire, torso_wire) =
                    (ent.field_i32(p, "legsAnim"), ent.field_i32(p, "torsoAnim"));
                stats.anim_restarts += u64::from(legs_wire != st.legs.anim())
                    + u64::from(torso_wire != st.torso.anim());

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

                    // cgame 0x30004e40's two channel updates (0x30003be0):
                    // the clips run on the drawn clock, and the rate is
                    // measured off the newest snapshot's `pos.trBase`
                    // whenever a newer snapshot came in.
                    let at = Times {
                        clock_ms: render_time,
                        snapshot_ms: latest_snapshot_ms,
                    };
                    let seen = Seen {
                        client_num: ent.field_i32(p, "clientNum"),
                        tr_base: Vec3::new(
                            ent.field_f32(p, "pos.trBase[0]"),
                            ent.field_f32(p, "pos.trBase[1]"),
                            ent.field_f32(p, "pos.trBase[2]"),
                        ),
                        eflags,
                    };
                    // The frames since it last was in the snapshot advanced
                    // nothing (0x3001bd00 steps only the newest snapshot's
                    // entities), so a body back in view resumes its clips.
                    let unseen = render_time
                        .wrapping_sub(frametime_ms)
                        .wrapping_sub(st.present_ms);
                    if unseen > 0 {
                        st.legs.pause(unseen);
                        st.torso.pause(unseen);
                    }
                    st.present_ms = render_time;
                    let channels = (&mut st.legs, &mut st.torso, &mut st.stance_window_ms);
                    update_channels(channels, anims, rules, (legs_wire, torso_wire), &seen, at);

                    // Corpses keep their death-clip pose; their aim fields are
                    // stale and would twist the body forever. A gunner runs no
                    // controllers (cgame 0x30004710).
                    let controllers = if etype == ET_PLAYER {
                        // cgame's `CG_PlayerAnimation` (0x30004e40) swings
                        // the legs, torso and torso pitch after the lerped
                        // view every frame, then reads the legs anim's
                        // record, as the server's `BG_PlayerAnimation` does.
                        let surf_type = ent.field_i32(p, "surfType");
                        let input = BodyInput {
                            view: angles.to_array(),
                            movement_dir: ent.field_f32(p, "angles2[1]"),
                            eflags,
                            legs: anims.record(legs_wire),
                            // UO's updater reads the seat off `surfType`;
                            // the record's seat yaw is the seat tag's this
                            // frame (0x30033a77).
                            seat_weapon: if rules == MoveRules::Uo {
                                SeatWeapon::of(
                                    surf_type >> 3 & 7,
                                    vehiclepose::rider_seat(surf_type),
                                    ent.field_i32(p, "weapon"),
                                    rider.as_ref().map_or(0.0, |s| s.angles[1]),
                                )
                            } else {
                                SeatWeapon::Barred
                            },
                        };
                        st.body.step(&input, frametime_ms, *swing_speed);
                        st.body.update_conditions(&input);
                        let slope = BodySlope {
                            lean,
                            torso_height: lerp_field("fTorsoHeight"),
                            torso_pitch: pitch,
                            waist_pitch,
                        };
                        let target =
                            body_controllers(rules, rider.as_ref(), &st.body, &input, &slope);
                        match target {
                            // The cgame's hook steps the controls over the
                            // frame (coduo-vehicles.md 14, "The controllers").
                            Some(t) if Controllers::eased(rules) => {
                                let mut c = st.controls.unwrap_or_default();
                                c.ease_toward(&t, frametime_ms);
                                st.controls = Some(c);
                                Some(c)
                            }
                            _ => target,
                        }
                    } else {
                        None
                    };

                    // The server's pose: the legs and torso clocks' clips
                    // blended as the tree walk blends its nodes. A gunner's
                    // legs are the placement's leaf blend (0x300279b0 sets
                    // the goal weights on the gunner's own tree); an MG42 aim
                    // group descends by the torso pitch with no yaw, which
                    // the wire does not carry.
                    let goal = rider
                        .as_ref()
                        .and_then(|seat| seat_inputs(anims, legs_wire, seat));
                    let inputs = PoseInputs {
                        anims,
                        legs: legs_wire,
                        torso: torso_wire,
                        legs_layers: st.legs.layers(render_time),
                        torso_layers: st.torso.layers(render_time),
                        nodes: st.torso.node_weights(render_time),
                        group_pitch: pitch,
                        turret_leaves: placed.as_ref().map(|g| (g.node, g.rows.as_slice())),
                        turret_left: None,
                        seat_leaves: st.seat.next(goal),
                        controllers,
                        leaf_times: Vec::new(),
                        node_time: false,
                        binds: Some(&assembly.binds),
                    };
                    st.pose =
                        pose_player_drawn(&assembly.skeleton, &inputs, |n| load_clip(clips, fs, n));
                }

                drawn.insert(
                    num,
                    Drawn {
                        pos,
                        rot,
                        rig: Rig::Player,
                    },
                );
                if hidden {
                    continue;
                }
                // Composed once for the tags and every model's skin.
                let worlds = st.pose.bone_worlds(&assembly.skeleton);
                if etype == ET_PLAYER
                    && let Some(bi) = assembly.skeleton.bone_index("Bip01 Head")
                {
                    let (local, _) = worlds[bi];
                    heads.insert(num, transform.transform_point3(local));
                }

                // The weapon's `tag_flash`. A body without one (a gunner,
                // whose weapon is not drawn) flashes nothing and starts its
                // tracers at the stance point (`CG_CalcMuzzlePoint`).
                let flash_tag = held_weapon
                    .as_ref()
                    .and_then(|_| assembly.skeleton.bone_index("tag_flash"));
                match flash_tag {
                    Some(bi) => {
                        let (local_pos, local_rot) = worlds[bi];
                        // Tags point +X forward (docs/research/xmodel-v14-format.md,
                        // "Model space and the view basis").
                        let world_pos = transform.transform_point3(local_pos);
                        let world_dir = transform.transform_vector3(local_rot * Vec3::X);
                        muzzles.insert(num, (world_pos, world_dir));
                    }
                    None if etype == ET_PLAYER && num < MAX_CLIENTS => {
                        let tr_base = Vec3::new(
                            ent.field_f32(p, "pos.trBase[0]"),
                            ent.field_f32(p, "pos.trBase[1]"),
                            ent.field_f32(p, "pos.trBase[2]"),
                        );
                        shot_fallbacks.insert(
                            num,
                            crate::fx::registry::muzzle_fallback(
                                tr_base,
                                ent.field_i32(p, "eFlags"),
                            ),
                        );
                    }
                    None => {}
                }
                if held_weapon.is_some()
                    && let Some(bi) = assembly.skeleton.bone_index("tag_brass")
                {
                    let (local_pos, local_rot) = worlds[bi];
                    brass.insert(
                        num,
                        (
                            transform.transform_point3(local_pos),
                            transform.transform_vector3(local_rot * Vec3::X),
                        ),
                    );
                }

                for (m, &handle) in assembly.handles.iter().enumerate() {
                    out.push(DynamicModelInstance {
                        model: handle,
                        transform,
                        bones: Some(assembly.skeleton.skin_matrices(&worlds, m)),
                        light_origin,
                        shader_time: None,
                    });
                }
            }
            EntityVisual::Model(m) => {
                let Some(handle) = resolve_model(model_cache, renderer, fs, &m) else {
                    continue;
                };
                drawn.insert(
                    num,
                    Drawn {
                        pos,
                        rot,
                        rig: Rig::Model(m),
                    },
                );
                out.push(DynamicModelInstance {
                    model: handle,
                    transform,
                    bones: None,
                    light_origin,
                    shader_time: None,
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
                drawn.insert(
                    num,
                    Drawn {
                        pos,
                        rot,
                        rig: Rig::Model(model),
                    },
                );
                // the panzerfaust's: black (weaponInfo `+0x130` is never
                // written) but it takes a scene light slot
                if def.projectile_dlight != 0.0 {
                    scene_lights.push(SceneLight {
                        origin: pos,
                        color: Vec3::ZERO,
                        intensity: def.projectile_dlight,
                    });
                }
                out.push(DynamicModelInstance {
                    model: handle,
                    transform,
                    bones: None,
                    light_origin,
                    shader_time: None,
                });
            }
            EntityVisual::Item(index) => {
                // Ammo and health rows (1.1 65-69, UO 129-133) carry their
                // own world model; a mod's dropped health pack is one.
                if let Some(model) = static_item_model(p, index) {
                    if let Some(handle) = resolve_model(model_cache, renderer, fs, model) {
                        drawn.insert(
                            num,
                            Drawn {
                                pos,
                                rot,
                                rig: Rig::Model(model.to_string()),
                            },
                        );
                        out.push(DynamicModelInstance {
                            model: handle,
                            transform,
                            bones: None,
                            light_origin,
                            shader_time: None,
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
                drawn.insert(
                    num,
                    Drawn {
                        pos,
                        rot,
                        rig: Rig::Model(world_model),
                    },
                );
                out.push(DynamicModelInstance {
                    model: handle,
                    transform,
                    bones: None,
                    light_origin,
                    shader_time: None,
                });
            }
            EntityVisual::Vehicle(model) => {
                let (Some(v), Some(rig)) = (
                    vehicles.get(&num),
                    vehicle_rigs.get(&model).and_then(Option::as_ref),
                ) else {
                    continue;
                };
                out.push(DynamicModelInstance {
                    model: rig.handle,
                    transform,
                    bones: Some(v.skins.clone()),
                    light_origin,
                    shader_time: Some(v.pose.shader_time),
                });
                vehicle_worlds.insert(num, v.worlds.clone());
                drawn.insert(
                    num,
                    Drawn {
                        pos,
                        rot,
                        rig: Rig::Vehicle(model),
                    },
                );
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
                    resolve_turret_rig(turret_rigs, parts, renderer, fs, &model),
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
                    worlds: Vec::new(),
                });
                st.last_seen_ms = render_time;
                let movetype = if uo {
                    ent.field_i32(p, "animMovetype")
                } else {
                    0
                };
                let slot = turret::gun_anim(rides, ent.field_i32(p, "eFlags"), movetype);
                st.channel.update(slot as i32, render_time);
                let clip_of = |raw: i32| {
                    if raw & ANIM_INDEX_MASK == GunAnim::Fire as i32 {
                        fire.as_deref()
                    } else {
                        idle.as_deref()
                    }
                };

                // The previous anim at full weight, then the goal blended over it.
                let mut pose = PoseBuffer::drawn(&rig.skeleton);
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
                if rides
                    && !uo
                    && let Some((base, arcs)) = &gun.uo
                    && let Some((eye, _)) = tag("tag_player")
                {
                    // `viewlocked` 2 is a frame that fired (0x30034a18).
                    let jitter = (ps_int("viewlocked") == 2).then(|| {
                        def.map_or([0.0; 2], |d| [d.vert_view_jitter, d.horiz_view_jitter])
                    });
                    let view = view.unwrap_or([base[0], base[1], 0.0]);
                    turret_eye = Some(turret::v15_eye(eye, view, *base, arcs, jitter, shake));
                } else if rides
                    && let Some((base, arcs)) = &gun.uo
                    && let Some((eye, r)) = tag("tag_player")
                {
                    // `cg.snap` fires and the gun is not overheated.
                    let fires = crate::play::seat::gunner_shakes(
                        a.ps.field_i32(p, "eFlags"),
                        a.entities
                            .get(&num)
                            .map_or(0, |e| e.field_i32(p, "animMovetype")),
                    );
                    let view = view.unwrap_or([base[0], base[1], 0.0]);
                    turret_eye = Some(turret::uo_eye(
                        (eye, [r * Vec3::X, r * Vec3::Y, r * Vec3::Z]),
                        view,
                        *base,
                        arcs,
                        fires.then(|| crate::play::seat::shake_phase(render_time)),
                        def.map_or([0.0; 2], |d| [d.vert_view_jitter, d.horiz_view_jitter]),
                        shake,
                    ));
                } else if rides && let Some((eye, _)) = tag("tag_player") {
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
                        axis: None,
                        snap: None,
                        kicks: true,
                    });
                }
                out.push(DynamicModelInstance {
                    model: rig.handle,
                    transform,
                    bones: Some(pose.skin_matrices(&rig.skeleton, 0)),
                    light_origin,
                    shader_time: None,
                });
                if let Some(st) = turret_anims.get_mut(&num) {
                    st.worlds = worlds;
                }
                drawn.insert(
                    num,
                    Drawn {
                        pos,
                        rot,
                        rig: Rig::Turret(model),
                    },
                );
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
    turret_anims.retain(|_, s| (render_time - s.last_seen_ms).abs() < STATE_TTL_MS);
    assemblies.prune_stale(render_time);
    stats.pending_assemblies = assemblies.pending.len();
    BuiltScene {
        instances: out,
        muzzles,
        shot_fallbacks,
        brass,
        weapon_flash,
        entity_pos,
        entity_angles,
        heads,
        turret_eye,
        submodels,
        scene_lights,
        shadow_casters,
        vehicle_angles,
        vehicle_wheels,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    /// Every ammo and health row's model loads under the name the item
    /// draw passes; the table's `xmodel/` prefix once made it look up
    /// `xmodel/xmodel/health_medium` and draw nothing.
    #[test]
    fn static_item_models_load() {
        let Some(fs) = vcod_common::testing::game_fs() else {
            return;
        };
        let p = vcod_common::net::protocol::install_protocol(&fs);
        for index in
            p.max_weapons + 1..p.max_weapons + 1 + vcod_common::weapon_table::STATIC_ITEMS.len()
        {
            let model = static_item_model(p, index).expect("a static item row");
            model::load(&fs, model).unwrap_or_else(|e| panic!("{model}: {e:#}"));
        }
    }

    /// A seat change keeps the leaves of the node it left beside the new
    /// seat's, until the rider is back on that node.
    #[test]
    fn a_seat_change_keeps_the_left_nodes_leaves() {
        let mut s = SeatLeaves::default();
        let gunner = (248, vec![(261, 1.0)]);
        assert_eq!(s.next(Some(gunner.clone())), vec![gunner.clone()]);
        assert_eq!(
            s.next(None),
            vec![gunner.clone()],
            "the driver's seat has none"
        );
        assert_eq!(s.next(None), vec![gunner.clone()]);
        let jeep = (273, vec![(276, 1.0)]);
        assert_eq!(
            s.next(Some(jeep.clone())),
            vec![jeep.clone()],
            "the left node moves on"
        );
        assert_eq!(s.next(Some(gunner.clone())), vec![gunner.clone(), jeep]);
        assert_eq!(
            s.next(Some(gunner.clone())),
            vec![gunner.clone(), (273, vec![(276, 1.0)])]
        );
    }

    /// On 1.5 and UO a drawn body's crouch stretches the torso's next
    /// blend to what is left of 400 ms, as the server's does; 1.1's
    /// setter keeps the fire anim's own.
    #[test]
    fn a_drawn_crouch_stretches_the_next_blend_off_1_1() {
        let Some(fs) = vcod_common::testing::game_fs() else {
            return;
        };
        let anims = PlayerAnims::load(&fs).unwrap();
        let (stand, crouch) = (
            anims.wire_of("pb_stand_alert").unwrap(),
            anims.wire_of("pb_crouch_alert").unwrap(),
        );
        // The first `fireweapon` torso anim, which carries its own 30 ms.
        let block = anims.script.event("fireweapon").unwrap();
        let name = &block
            .clauses
            .iter()
            .flat_map(|c| &c.torso)
            .next()
            .unwrap()
            .name;
        let fire = anims.wire_of(name).unwrap();
        let seen = Seen {
            client_num: 0,
            tr_base: Vec3::ZERO,
            eflags: 0,
        };
        let torso_weight = |rules| {
            let (mut legs, mut torso, mut window) = Default::default();
            for (t, l, tr) in [(1000, stand, 0), (2000, crouch, 0), (2100, crouch, fire)] {
                let ch = (&mut legs, &mut torso, &mut window);
                update_channels(ch, &anims, rules, (l, tr), &seen, Times::end_frame(t));
            }
            torso.layers(2150).last().unwrap().weight
        };
        assert!((torso_weight(MoveRules::V1_1) - 1.0).abs() < 1e-6);
        assert!((torso_weight(MoveRules::V1_5) - 50.0 / 300.0).abs() < 1e-3);
    }

    #[test]
    fn constant_light_unpacks_intensity_and_colour_bytes() {
        assert!(constant_light(0, Vec3::ZERO).is_none());
        let l = constant_light(0x32ff_8000, Vec3::X).unwrap();
        assert_eq!(l.intensity, 200.0);
        assert_eq!(l.color, Vec3::new(0.0, 128.0 / 255.0, 1.0));
        assert_eq!(l.origin, Vec3::X);
    }
    use vcod_common::net::msg::{ClientState, EntityState};
    use vcod_common::net::protocol::{CS_MODELS_V1, CS_TAGS_V1, PROTOCOL_V1};

    /// A United Offensive tank draws as the model its `xmodel` names in
    /// UO's model block (the live mp_cassino Sherman: `xmodel` 36,
    /// configstring 441), and its wreck likewise; on 1.1, `eType` 12 is an
    /// event entity and draws nothing.
    #[test]
    fn a_uo_vehicle_draws_its_xmodel() {
        use vcod_common::net::protocol::PROTOCOL_V22;
        let p = &PROTOCOL_V22;
        let mut cs = vec![String::new(); 2048];
        cs[441] = "xmodel/mp_vehicle_sherman".into();
        let mut tank = EntityState::null(p);
        let mut set = |name: &str, v: i32| {
            tank.fields[EntityState::field_index(p, name).unwrap()] = v;
        };
        set("eType", 12);
        set("xmodel", 36);
        let clients = BTreeMap::new();
        let want = EntityVisual::Vehicle("mp_vehicle_sherman".into());
        assert_eq!(resolve_visual(&tank, &clients, &cs, p), want);
        let mut wreck = tank.clone();
        wreck.fields[EntityState::field_index(p, "eType").unwrap()] = 13;
        assert_eq!(resolve_visual(&wreck, &clients, &cs, p), want);

        let v1 = &PROTOCOL_V1;
        let mut event = EntityState::null(v1);
        event.fields[EntityState::field_index(v1, "eType").unwrap()] = 12;
        assert_eq!(
            resolve_visual(&event, &clients, &cs, v1),
            EntityVisual::None
        );
    }

    /// A drawn plain model's tag is its bind-pose bone carried by the
    /// entity's drawn frame, looked up without regard to case; an entity
    /// not drawn, or a tag the model lacks, has none.
    #[test]
    fn a_drawn_models_tag_rides_its_frame() {
        let Some(fs) = vcod_common::testing::game_fs() else {
            return;
        };
        let model = model::load(&fs, "mg42_bipod").unwrap();
        let skel = Skeleton::build(&[&model]);
        let (lp, lr) =
            PoseBuffer::new(&skel).bone_world(&skel, skel.bone_index("tag_flash").unwrap());
        let mut scene = EntityScene::new();
        let (pos, rot) = (Vec3::new(100.0, -20.0, 8.0), angles_quat([10.0, 90.0, 0.0]));
        scene.drawn.insert(
            70,
            Drawn {
                pos,
                rot,
                rig: Rig::Model("mg42_bipod".into()),
            },
        );
        let tag = scene.tag_pose(&fs, 70, "TAG_FLASH").expect("tag_flash");
        assert!(tag.origin.abs_diff_eq(pos + rot * lp, 1e-4), "{tag:?}");
        let want = rot * lr;
        for (got, axis) in tag.axis.iter().zip([Vec3::X, Vec3::Y, Vec3::Z]) {
            assert!(got.abs_diff_eq(want * axis, 1e-5), "{tag:?}");
        }
        assert_eq!(scene.tag_pose(&fs, 70, "tag_nonesuch"), None);
        assert_eq!(scene.tag_pose(&fs, 71, "tag_flash"), None);
    }

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
            let axis = vcod_common::pmove::aim::angles_to_axis_msvc(a);
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
            vcod_common::net::events::ET_EVENTS,
            vcod_common::net::events::ET_EVENTS + 5,
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
        assert_eq!(ch.raw & ANIM_INDEX_MASK, 6);
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
    fn fake_part() -> (Rc<Model>, ModelHandle) {
        let m = Model::rig("x", vec![]);
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

    /// Each rider in the UO seat captures draws as vcod-server poses him:
    /// the seat tag off the drawn vehicle is the one the server places him
    /// at (its hull-only pose, `vehicle::seat::posed_as`), and the
    /// controllers and seat blend the draw path hands `pose_player_drawn` are the
    /// server's `BodyPose` ones but for the cgame's steer scale.
    #[test]
    fn a_drawn_rider_is_posed_as_the_server_poses_him() {
        use vcod_common::animtree::PlayerAnims;
        use vcod_common::net::protocol::PROTOCOL_V22;
        use vcod_common::playerpose::{STEER_FULL_CGAME, seat_inputs};
        let Some(fs) = vcod_common::testing::uo_fs() else {
            return;
        };
        let anims = PlayerAnims::load(&fs).expect("the player anims");
        let p = &PROTOCOL_V22;
        let mut skels: HashMap<String, Skeleton> = HashMap::new();
        let mut seats = std::collections::BTreeMap::new();
        let mut worst_at = 0.0f32;
        for tag in ["-uo-jeep", "-uo-tank", "-uo-overheat"] {
            let gs = vcod_common::testing::fixture_gamestate(p, &format!("net/gamestate{tag}.bin"));
            for frame in vcod_common::testing::fixture_frames(
                p,
                &format!("net/gamestate{tag}.bin"),
                &format!("net/snapshots{tag}.bin"),
            ) {
                let s = &frame.snapshot;
                for e in s.entities.values() {
                    let eflags = e.field_i32(p, "eFlags");
                    if e.field_i32(p, "eType") != ET_PLAYER || eflags & EF_IN_VEHICLE_V22 == 0 {
                        continue;
                    }
                    let Some(ve) = s.entities.get(&(e.field_i32(p, "otherEntityNum") as u32))
                    else {
                        continue;
                    };
                    let EntityVisual::Vehicle(model) =
                        resolve_visual(ve, &BTreeMap::new(), &gs.configstrings, p)
                    else {
                        continue;
                    };
                    let skel = skels.entry(model.clone()).or_insert_with(|| {
                        Skeleton::build(&[&model::load(&fs, &model).expect("the vehicle")])
                    });
                    let v3 = |k: &str| {
                        Vec3::new(
                            e.field_f32(p, &format!("{k}[0]")),
                            e.field_f32(p, &format!("{k}[1]")),
                            e.field_f32(p, &format!("{k}[2]")),
                        )
                    };
                    let vv3 = |k: &str| {
                        Vec3::new(
                            ve.field_f32(p, &format!("{k}[0]")),
                            ve.field_f32(p, &format!("{k}[1]")),
                            ve.field_f32(p, &format!("{k}[2]")),
                        )
                    };
                    let fields = VehicleFields::read(ve, p);
                    let v = VehicleFrame::posed(
                        skel,
                        model,
                        (vv3("pos.trBase"), vv3("apos.trBase")),
                        (&fields, &fields, 0.0),
                        |_| {},
                    );
                    let (surf, weapon) = (e.field_i32(p, "surfType"), e.field_i32(p, "weapon"));
                    let Some((at, _, drawn)) = rider_frame(skel, &v, surf, weapon) else {
                        continue;
                    };
                    let seat = vehiclepose::rider_seat(surf);
                    let mut hull = PoseBuffer::new(skel);
                    vehiclepose::apply_hull(&mut hull, skel, &v.pose);
                    let (served_at, r) = vehiclepose::seat_frame(
                        skel,
                        &hull.bone_worlds(skel),
                        (v.pos, v.rot),
                        v.pose.kind,
                        seat,
                    )
                    .expect("the server's seat");
                    let served = BodySeat::new(quat_angles(r), v.pose.kind, seat, fields.steer)
                        .armed(weapon);
                    assert!((at - served_at).length() < 1e-3, "{at} {served_at}");
                    for i in 0..3 {
                        assert!(
                            vcod_common::pmove::aim::angle_subtract(
                                drawn.angles[i],
                                served.angles[i]
                            )
                            .abs()
                                < 1e-3,
                            "{:?} {:?}",
                            drawn.angles,
                            served.angles
                        );
                    }
                    // Retail sent the origin truncated, off the controls
                    // the frame started from: a turning turret carries the
                    // tank gunner up to 5 units (coduo-vehicles.md 11.4).
                    worst_at = worst_at.max((at - v3("pos.trBase")).abs().max_element());

                    let legs = e.field_i32(p, "legsAnim");
                    let input = BodyInput {
                        view: v3("apos.trBase").to_array(),
                        movement_dir: e.field_f32(p, "angles2[1]"),
                        eflags,
                        legs: anims.record(legs),
                        seat_weapon: SeatWeapon::of(v.pose.kind, seat, weapon, served.angles[1]),
                    };
                    let slope = BodySlope {
                        lean: e.field_f32(p, "leanf"),
                        torso_height: e.field_f32(p, "fTorsoHeight"),
                        torso_pitch: e.field_f32(p, "fTorsoPitch"),
                        waist_pitch: e.field_f32(p, "fWaistPitch"),
                    };
                    let body = vcod_server::game::combat::BodyPose {
                        legs,
                        torso: e.field_i32(p, "torsoAnim"),
                        input,
                        slope,
                        seat: Some(BodySeat {
                            steer_full: STEER_FULL_CGAME,
                            ..served
                        }),
                        es: vcod_server::game::combat::HookEs {
                            seat: Some((v.pose.kind, seat)),
                            weapon,
                        },
                        ci_seat_angles: served.angles,
                        ..Default::default()
                    };
                    let server = body.pose_inputs(&anims, s.server_time);
                    let client = body_controllers(
                        MoveRules::Uo,
                        Some(&drawn),
                        &BodyAngles::default(),
                        &input,
                        &slope,
                    )
                    .expect("a rider's controllers");
                    let c = server.controllers.expect("the server's controllers");
                    let near = |a: [f32; 3], b: [f32; 3]| {
                        a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-3)
                    };
                    for (a, b) in [
                        (client.origin, c.origin),
                        (client.pelvis, c.pelvis),
                        (client.back_low, c.back_low),
                        (client.back_mid, c.back_mid),
                        (client.back_up, c.back_up),
                        (client.neck, c.neck),
                        (client.head, c.head),
                    ] {
                        assert!(near(a, b), "{client:?} {c:?}");
                    }
                    let leaves = seat_inputs(&anims, legs, &drawn);
                    // The server's list carries the node a seat change left
                    // after this seat's.
                    match &leaves {
                        Some(l) => assert_eq!(server.seat_leaves.first(), Some(l)),
                        None => {
                            let node = anims.index.node_id(legs);
                            assert!(server.seat_leaves.iter().all(|l| Some(l.0) != node));
                        }
                    }
                    *seats
                        .entry((v.pose.kind, seat, leaves.is_some()))
                        .or_insert(0) += 1;
                }
            }
        }
        println!("riders by (kind, seat, blended): {seats:?}; origin off retail's by {worst_at}");
        // The jeep's and the tank's gunners, each in his seat blend.
        assert!(seats.contains_key(&(1, 2, true)) && seats.contains_key(&(2, 2, true)));
        assert!(worst_at < 5.0, "{worst_at}");
    }

    /// Only a four-wheel's seat 3 draws the rider's own weapon (0x300058f0).
    #[test]
    fn a_rider_draws_a_weapon_in_the_passengers_seat_only() {
        let seated = EF_IN_VEHICLE_V22;
        assert!(seat_unarmed(seated, 1 << 3 | 1) && seat_unarmed(seated, 2 << 3 | 2));
        assert!(!seat_unarmed(seated, 1 << 3 | 3) && !seat_unarmed(0, 1 << 3 | 1));
    }

    /// `cg.snap`'s own unsighted driver turns his wheel four times the
    /// steer; anyone else's turns 0.75 of it (cgame 0x30020b8b).
    #[test]
    fn the_own_driver_turns_his_wheel_four_times_the_steer() {
        use vcod_common::net::protocol::PROTOCOL_V22;
        let p = &PROTOCOL_V22;
        let mut ps = PlayerState::null(p);
        let mut set = |name: &str, v: i32| {
            let i = PlayerState::field_index(p, name).unwrap();
            ps.fields[i] = v;
        };
        set("eFlags", EF_IN_VEHICLE_V22);
        set("vehPos", 1);
        set("viewlocked_entNum", 90);
        assert_eq!(steering_ratio(&ps, 90, p), 4.0);
        assert_eq!(steering_ratio(&ps, 91, p), 0.75);
        let mut sighted = ps.clone();
        sighted.fields[PlayerState::field_index(p, "fWeaponPosFrac").unwrap()] =
            0.5f32.to_bits() as i32;
        assert_eq!(steering_ratio(&sighted, 90, p), 0.75);
        let mut gunner = ps.clone();
        gunner.fields[PlayerState::field_index(p, "vehPos").unwrap()] = 2;
        assert_eq!(steering_ratio(&gunner, 90, p), 0.75);
        // The own gunner's gun follows the view on his vehicle only.
        assert!(own_gunner(&gunner, 90, p) && !own_gunner(&gunner, 91, p));
        assert!(!own_gunner(&ps, 90, p), "the driver");
    }
}
