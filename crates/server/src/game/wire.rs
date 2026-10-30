//! The object table as packet entities.
//!
//! What reaches a client is what retail links and does not mark `NOCLIENT`,
//! and the traces say a stock map's static set is items, scriptmovers and
//! turrets (`docs/protocol-1.1.md`, "Which entities a client is sent"). This
//! module builds that set; the culling that decides which of them one client
//! is sent is the caller's business.
//!
//! # How the entity numbers are split
//!
//! - `0..63`: client slots (`crate::game::entity::ObjectTable::spawn_client`).
//! - `64..71`: the body queue, retail's own numbers
//!   (`crate::game::bodies`, `docs/research/cod11-combat.md` section 5.2).
//! - `72..`: map and script entities and temp entities, from
//!   `entity::FIRST_MAP_ENTITY` up, off `G_Spawn`'s free list
//!   (`crate::game::temp_entity`).
//! - `1022`: `ENTITYNUM_WORLD`, `1023`: `ENTITYNUM_NONE`.
//!
//! Bodies and temp entities are appended by `crate::server`, not here: the
//! A/B gates read `packet_entities` for the map's own static set.

use super::host::GameHost;
use crate::game::entity::{HUD_OWNER_ALL, HudState};
use crate::game::profile::{TriggerWire, WeaponNumbering};
use std::collections::BTreeMap;
use vcod_common::net::flags::{
    EF_FIRING, EF_NODRAW, EF_TELEPORT_BIT, ET_CORPSE, ET_FX, ET_ITEM, ET_MOVER, ET_PLAYER,
    ET_SCRIPTMOVER, ET_TURRET, SOLID_BMODEL,
};
use vcod_common::net::msg::{EntityState, HudElem, MAX_HUD_ELEMS};
use vcod_common::net::protocol::{Cod1Wire, ENTITYNUM_NONE};
use vcod_common::net::trajectory::{TR_INTERPOLATE, Trajectory};
use vcod_gsc::EntId;
use vcod_gsc::{Cx, Host, Value};

/// `eFlags` 2, which every trigger's spawn but `trigger_use`'s ORs in
/// (`InitTrigger` 0x65784). Unnamed in the 1.1 headers we have.
const EF_TRIGGER: i32 = 2;
/// CoD2's: `InitTrigger` (0x811c504) ORs in 1.
const COD2_EF_TRIGGER: i32 = 1;
/// CoD4's: 1 and the nodraw 0x20 of a trigger with no model: the walk
/// capture's triggers read 0x21 in their baselines (VERIFIED live), the
/// bits `InitTrigger` sets.
const COD4_EF_TRIGGER: i32 = 0x21;
/// `eFlags` as the traces carry it on every item, and `clientNum` on one no
/// dropper's lockout holds: 0x3fe through the 8-bit netfield
/// (docs/research/cod11-items.md section 9).
const ITEM_EFLAGS: i32 = 16;
const ITEM_CLIENTNUM: i32 = 254;
/// A turret's `apos.trType` in both maps' traces.
const TURRET_APOS_TRTYPE: i32 = 3;

/// The numbers [`build`] writes an entity in: the game's own `eType`s,
/// `eFlags` bits and `trType`s, and the events of its ring. vcod's shared
/// code keeps 1.1's (a mover's trajectories, the events the builtins
/// raise); a game that numbers them otherwise maps them here, as each
/// entity is built (`HostProfile::entities`; CoD2's are
/// `cod2_wire::COD2_ENTITY_NUMBERS`, cod2-protocol.md 14.1).
pub struct EntityNumbers {
    pub et_item: i32,
    /// A `func_bobbing`, `func_pendulum` or `func_static`.
    pub et_mover: i32,
    pub et_scriptmover: i32,
    pub et_fx: i32,
    pub et_turret: i32,
    pub ef_teleport: i32,
    pub ef_nodraw: i32,
    pub ef_firing: i32,
    /// What every item carries in `eFlags` beside `ef_nodraw`.
    pub item_eflags: i32,
    /// A ring event raised in 1.1's numbering, `None` for one the game has
    /// no event for: it goes out as none, its parm 0.
    pub event: fn(i32) -> Option<i32>,
    /// A `trType` in 1.1's numbering.
    pub tr_type: fn(i32) -> i32,
}

/// 1.1's numbers, which 1.5 and United Offensive build in too
/// ([`entity_to_protocol`] maps what UO renumbered once a snapshot).
pub const COD1_ENTITY_NUMBERS: EntityNumbers = EntityNumbers {
    et_item: ET_ITEM,
    et_mover: ET_MOVER,
    et_scriptmover: ET_SCRIPTMOVER,
    et_fx: ET_FX,
    et_turret: ET_TURRET,
    ef_teleport: EF_TELEPORT_BIT,
    ef_nodraw: EF_NODRAW,
    ef_firing: EF_FIRING,
    item_eflags: ITEM_EFLAGS,
    event: Some,
    tr_type: |t| t,
};

/// Marks an `eType` that is United Offensive's own (a vehicle, 12): 1.1's
/// numbering puts its events there, so the type rides above the 8-bit
/// field until [`entity_to_protocol`] takes the marker off.
pub const ET_RAW: i32 = 0x100;
/// United Offensive's `ET_SOUND_BLEND`.
const ET_SOUND_BLEND: i32 = 9;

/// The vehicle a United Offensive sound blend belongs to. The snapshot
/// builder (coduo_lnxded 0x8096c6a) tests a blend whose `otherEntityNum`
/// is not 1023 as that entity: its flags, its broadcast time and its
/// clusters, never the blend's own (coduo-vehicles.md 10.2).
pub fn sound_blend_vehicle(p: &Cod1Wire, es: &EntityState) -> Option<u32> {
    let other = es.field_i32(p, "otherEntityNum");
    (es.field_i32(p, "eType") == (ET_RAW | ET_SOUND_BLEND) && other != ENTITYNUM_NONE as i32)
        .then_some(other as u32)
}

/// An entity the server built in vcod's (1.1's) numbering, put into the
/// protocol's: the `eFlags` layout, the event ids in the ring and an event
/// entity's `eType` (`ET_EVENTS` 16 on United Offensive). Identity on 1.1
/// and 1.5, which share 1.1's numbers. The obituary's means of death is
/// already the protocol's: the builtin indexes the running `Defs`.
pub fn entity_to_protocol(p: &Cod1Wire, es: &mut EntityState) {
    if p.et_events == vcod_common::net::events::ET_EVENTS {
        return;
    }
    // A UO-only type rides above 1.1's event range ([`ET_RAW`]).
    let raw = EntityState::field_index(p, "eType").filter(|&i| es.fields[i] & ET_RAW != 0);
    {
        let mut map = |name: &str, f: &dyn Fn(i32) -> i32| {
            if let Some(i) = EntityState::field_index(p, name) {
                es.fields[i] = f(es.fields[i]);
            }
        };
        map("eFlags", &p.eflags_from_v1);
        for k in 0..4 {
            map(&format!("events[{k}]"), &p.event_from_v1);
        }
    }
    if let Some(i) = raw {
        es.fields[i] &= !ET_RAW;
        return;
    }
    // UO's `G_DObjUpdate` (game.mp.uo 0x77df9) puts a model entity's model in
    // `s.xmodel` where 1.1's (0x66054) writes `s.index`; a brush model keeps
    // its inline number in `index` (coduo-diff.md 22).
    let get = |es: &EntityState, n: &str| EntityState::field_index(p, n).map(|i| es.fields[i]);
    if let (Some(x), Some(i)) = (
        EntityState::field_index(p, "xmodel"),
        EntityState::field_index(p, "index"),
    ) && matches!(
        get(es, "eType"),
        Some(vcod_common::net::flags::ET_SCRIPTMOVER | vcod_common::net::flags::ET_TURRET)
    ) && get(es, "solid") != Some(vcod_common::net::flags::SOLID_BMODEL)
    {
        es.fields[x] = es.fields[i];
        es.fields[i] = 0;
    }
    let mut map = |name: &str, f: &dyn Fn(i32) -> i32| {
        if let Some(i) = EntityState::field_index(p, name) {
            es.fields[i] = f(es.fields[i]);
        }
    };
    map("eType", &|t| {
        if t >= vcod_common::net::events::ET_EVENTS {
            p.et_events + (p.event_from_v1)(t - vcod_common::net::events::ET_EVENTS)
        } else {
            t
        }
    });
}

/// [`entity_to_protocol`] for a playerstate: its `eFlags` and its event
/// ring.
pub fn ps_to_protocol(p: &Cod1Wire, ps: &mut vcod_common::net::msg::PlayerState) {
    if p.et_events == vcod_common::net::events::ET_EVENTS {
        return;
    }
    use vcod_common::net::msg::PlayerState;
    let mut map = |name: &str, f: &dyn Fn(i32) -> i32| {
        if let Some(i) = PlayerState::field_index(p, name) {
            ps.fields[i] = f(ps.fields[i]);
        }
    };
    map("eFlags", &p.eflags_from_v1);
    for k in 0..4 {
        map(&format!("events[{k}]"), &p.event_from_v1);
    }
}

/// The `r.mins`/`r.maxs` an entity's spawn function leaves on it. The clusters
/// it links with come from this grown by the engine's link epsilon, which is
/// the caller's business (`crate::world::visible_entities`).
///
/// VERIFIED from the module: `G_SpawnItem` (0x4e6ed) writes (-1, -1, -1) to
/// (1, 1, 1) for a weapon row (any other row takes (-1, -1, 0) to (1, 1, 2),
/// docs/research/cod11-items.md 14, a unit off for the clusters), and
/// `G_SpawnTurret` (0x52f75) writes (-32, -32, 0) to (32, 32, 56).
pub fn link_box(etype: i32) -> ([f32; 3], [f32; 3]) {
    link_box_of(match etype {
        ET_TURRET => LinkBox::Turret,
        ET_SCRIPTMOVER => LinkBox::ScriptMover,
        ET_PLAYER | ET_CORPSE => LinkBox::Body,
        _ => LinkBox::Point,
    })
}

/// Which spawn's box an entity links with, whatever its game numbers its
/// `eType` ([`link_box_of`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkBox {
    Turret,
    ScriptMover,
    Body,
    Point,
}

/// [`link_box`] by the spawn's box.
pub fn link_box_of(class: LinkBox) -> ([f32; 3], [f32; 3]) {
    match class {
        LinkBox::Turret => ([-32.0, -32.0, 0.0], [32.0, 32.0, 56.0]),
        // A script model is spawned with a zero box and nothing on the
        // `SP_script_model` path ever writes one, so its clusters come from
        // the engine's link epsilon alone. A `script_brushmodel` shares this
        // `eType` but links with its inline model's bounds, which
        // `crate::world` reads off the entity's `solid` and `index`.
        LinkBox::ScriptMover => ([0.0; 3], [0.0; 3]),
        // A player links with its own movement box, the one pmove collides
        // with (`vcod_common::pmove`). A corpse keeps the player's box: the
        // clone copies the dying entity's bounds, which `player_die` has
        // already flattened to 30 units tall (cod11-combat.md 5.2). The
        // extra height only widens the cluster set, so the taller box is
        // the conservative one to cull with.
        LinkBox::Body => (
            [
                -vcod_common::pmove::HALF_WIDTH,
                -vcod_common::pmove::HALF_WIDTH,
                0.0,
            ],
            [
                vcod_common::pmove::HALF_WIDTH,
                vcod_common::pmove::HALF_WIDTH,
                vcod_common::pmove::HEIGHT_STAND,
            ],
        ),
        LinkBox::Point => ([-1.0, -1.0, -1.0], [1.0, 1.0, 1.0]),
    }
}

/// What one client is sent of the HUD element pool: the archived array and
/// the current one, in that order, which is the order block 5 writes them
/// (docs/protocol-1.1.md, "Block 5").
///
/// This is `HudElem_UpdateClient` (`game.mp.i386.so` 0x4be8c), which
/// `G_RunFrame` calls once per in-use client (0x50a80). It walks
/// `g_hudelems` from slot 0, skips a record whose team is set and does not
/// match this client's, skips one whose owner is neither `ENTITYNUM_NONE`
/// nor this client, and appends the rest to the array its `archived` flag
/// picks, 31 elements each.
pub fn hud_elems(host: &GameHost, slot: usize, team: i32) -> (Vec<HudElem>, Vec<HudElem>) {
    let mut archived = Vec::new();
    let mut current = Vec::new();
    let ids: Vec<EntId> = host.ents.iter_hud_elems().map(|(id, _)| id).collect();
    for id in ids {
        let Some(state) = host.ents.get(id).and_then(|e| e.hud) else {
            continue;
        };
        if state.team != 0 && state.team != team {
            continue;
        }
        if state.owner != HUD_OWNER_ALL && state.owner != slot as u32 {
            continue;
        }
        let out = match hud_field_i32(host, id, "archived") {
            0 => &mut current,
            _ => &mut archived,
        };
        if out.len() < MAX_HUD_ELEMS {
            out.push(build_hud_elem(host, id, &state));
        }
    }
    (archived, current)
}

/// One record as block 5 carries it: the script-visible fields out of the
/// element's engine slots, the rest out of its [`HudState`]. An unwritten
/// slot reads `undefined`, which stands for the zero retail's `bzero` left.
fn build_hud_elem(host: &GameHost, id: EntId, state: &HudState) -> HudElem {
    use vcod_common::net::msg::hud_field as f;
    let mut e = HudElem::default();
    e.set(f::TYPE, state.elem_type);
    e.set(f::TEXT, state.text);
    e.set(f::TIME, state.time);
    e.set(f::DURATION, state.duration);
    // UO's two texture coordinates; a protocol without them must see 0 there,
    // or the delta's last-changed index points past its fields.
    if host.defs.proto.hud.fields > f::TCOORD {
        e.set_f32(f::SCOORD, state.scoord);
        e.set_f32(f::TCOORD, state.tcoord);
    }
    e.set(f::SHADER, state.shader);
    e.set(f::WIDTH, state.width);
    e.set(f::HEIGHT, state.height);
    e.set_f32(f::VALUE, state.value);
    for (field, name) in [
        (f::X, "x"),
        (f::Y, "y"),
        (f::FONT, "font"),
        (f::ALIGN_X, "alignx"),
        (f::ALIGN_Y, "aligny"),
        (f::COLOR, "color"),
        (f::LABEL, "label"),
    ] {
        e.set(field, hud_field_i32(host, id, name));
    }
    for (field, name) in [(f::FONT_SCALE, "fontscale"), (f::SORT, "sort")] {
        e.set_f32(field, hud_field_f32(host, id, name));
    }
    e.set(f::FROM_COLOR, state.from_color);
    e.set(f::FADE_START_TIME, state.fade_start);
    e.set(f::FADE_TIME, state.fade_ms);
    e.set(f::SCALE_START_TIME, state.scale_start);
    e.set(f::SCALE_TIME, state.scale_ms);
    e.set(f::FROM_WIDTH, state.from_width);
    e.set(f::FROM_HEIGHT, state.from_height);
    e.set(f::MOVE_START_TIME, state.move_start);
    e.set(f::MOVE_TIME, state.move_ms);
    e.set(f::FROM_X, state.from_x);
    e.set(f::FROM_Y, state.from_y);
    e
}

/// One HUD engine slot as the integer the wire wants. The enum fields
/// (`font`, `alignx`, `aligny`) are stored as retail's index and read back
/// as a name, so this takes the slot rather than going through `get_field`.
/// `pub(crate)`: the tween builtins in `builtins::hud` read `color`/`x`/`y`
/// through it before they overwrite the state those slots snapshot.
pub(crate) fn hud_field_i32(host: &GameHost, id: EntId, name: &str) -> i32 {
    match hud_slot_value(host, id, name) {
        Some(Value::Int(i)) => i,
        Some(Value::Float(f)) => f as i32,
        _ => 0,
    }
}

fn hud_field_f32(host: &GameHost, id: EntId, name: &str) -> f32 {
    match hud_slot_value(host, id, name) {
        Some(Value::Int(i)) => i as f32,
        Some(Value::Float(f)) => f,
        _ => 0.0,
    }
}

/// A slot through the game's own hudelem table (CoD4's packs `color` at
/// a slot of its own).
fn hud_slot_value(host: &GameHost, id: EntId, name: &str) -> Option<Value> {
    let crate::game::fields::Route::Engine { slot, .. } = (host.profile.route_hud)(name) else {
        return None;
    };
    host.ents.get(id).map(|e| e.engine[slot])
}

/// Builds one `EntityState` per linked object, keyed by entity number.
pub fn packet_entities(
    host: &mut GameHost,
    cx: &mut Cx,
    p: &Cod1Wire,
) -> BTreeMap<u32, EntityState> {
    linked_entities(host, cx, p, false)
}

/// What `SV_CreateBaseline` writes: [`packet_entities`] plus the linked
/// `SVF_NOCLIENT` entities no snapshot carries, the map's triggers and
/// `script_origin`s. The pass tests `r.linked` alone (map-cycle doc 3
/// step 21).
pub fn baseline_entities(
    host: &mut GameHost,
    cx: &mut Cx,
    p: &Cod1Wire,
) -> BTreeMap<u32, EntityState> {
    linked_entities(host, cx, p, true)
}

fn linked_entities(
    host: &mut GameHost,
    cx: &mut Cx,
    p: &Cod1Wire,
    noclient: bool,
) -> BTreeMap<u32, EntityState> {
    let ids: Vec<EntId> = host.ents.iter_inuse().map(|(id, _)| id).collect();
    let mut out = BTreeMap::new();
    for id in ids {
        if let Some(e) = build(host, cx, p, id, noclient) {
            out.insert(id.0, e);
        }
    }
    out
}

/// The one entity, or `None` when nothing links it, or when it is
/// `SVF_NOCLIENT` and `noclient` is off.
fn build(
    host: &mut GameHost,
    cx: &mut Cx,
    p: &Cod1Wire,
    id: EntId,
    noclient: bool,
) -> Option<EntityState> {
    if host.ents.get(id).is_some_and(|e| e.unlinked) {
        return None;
    }
    // A vehicle's sound blends carry no classname (`G_SpawnSoundBlend`'s
    // scr_const is never set).
    let classname = match field_string(host, cx, id, "classname") {
        Some(c) => c,
        None if crate::game::vehicle::sound_blend(host, id).is_some() => String::new(),
        None => return None,
    };
    let kind = kind_of(host, cx, id, &classname)?;
    // `playSound` clears `SVF_NOCLIENT` (game.mp 0x5d95d). CoD2's builder
    // takes a set `r.broadcastTime` ahead of the `SVF_NOCLIENT` test
    // (`SV_AddEntitiesVisibleFromPoint`, lnxded 0x8098b98): a looping
    // `script_origin` goes out (cod2-protocol.md 15.7).
    let sends = host.profile.broadcast_time_sends;
    let broadcast = host
        .ents
        .get(id)
        .is_some_and(|e| e.broadcast || sends && e.broadcast_time != 0);
    if !noclient && !broadcast && matches!(kind, Kind::Trigger(_) | Kind::ScriptOrigin) {
        return None;
    }

    let n = host.profile.entities;
    let mut e = EntityState::null(p);
    e.number = id.0;
    let seti = |e: &mut EntityState, name: &str, v: i32| {
        if let Some(i) = EntityState::field_index(p, name) {
            e.fields[i] = v;
        }
    };
    let setf = |e: &mut EntityState, name: &str, v: f32| {
        if let Some(i) = EntityState::field_index(p, name) {
            e.fields[i] = v.to_bits() as i32;
        }
    };

    // A mover's own trajectory groups, which are what the client
    // extrapolates between the two updates a move produces
    // (`crate::game::mover`, docs/research/cod11-movers.md section 9). An
    // entity no verb has touched is stationary at its `origin`/`angles`.
    match host.movers.wire(id, host.level_time_ms) {
        Some((pos, mut apos)) => {
            // A group no verb has run on stands at the entity's own field:
            // a `moveTo` alone leaves `apos` at the angles script set
            // (cod4-gsc.md 3.2, the plane's 90).
            if host
                .movers
                .pose_at(id, host.level_time_ms)
                .is_some_and(|(_, a)| a.is_none())
            {
                let angles = host
                    .ents
                    .get(id)
                    .and_then(|e| e.apos_base)
                    .or_else(|| field_vec(host, cx, id, "angles"))
                    .unwrap_or([0.0; 3]);
                apos.base = glam::Vec3::from_array(angles);
            }
            for (group, tr) in [("pos", pos), ("apos", apos)] {
                seti(&mut e, &format!("{group}.trType"), (n.tr_type)(tr.tr_type));
                seti(&mut e, &format!("{group}.trTime"), tr.tr_time);
                seti(&mut e, &format!("{group}.trDuration"), tr.tr_duration);
                for (axis, v) in tr.base.to_array().iter().enumerate() {
                    setf(&mut e, &format!("{group}.trBase[{axis}]"), *v);
                }
                for (axis, v) in tr.delta.to_array().iter().enumerate() {
                    setf(&mut e, &format!("{group}.trDelta[{axis}]"), *v);
                }
            }
        }
        None => {
            let origin = field_vec(host, cx, id, "origin").unwrap_or([0.0; 3]);
            let angles = host
                .ents
                .get(id)
                .and_then(|e| e.apos_base)
                .or_else(|| field_vec(host, cx, id, "angles"))
                .unwrap_or([0.0; 3]);
            // A link re-anchors the entity every frame and sends both groups
            // `TR_INTERPOLATE` at the pose (`G_GeneralLink` 0x68568,
            // docs/research/cod11-movers.md 15).
            if host.links.contains(id) {
                seti(&mut e, "pos.trType", (n.tr_type)(TR_INTERPOLATE));
                seti(&mut e, "apos.trType", (n.tr_type)(TR_INTERPOLATE));
            }
            for (axis, v) in origin.iter().enumerate() {
                setf(&mut e, &format!("pos.trBase[{axis}]"), *v);
            }
            for (axis, v) in angles.iter().enumerate() {
                setf(&mut e, &format!("apos.trBase[{axis}]"), *v);
            }
        }
    }

    // The entity's own event ring, which `playSound` writes on a receiver
    // that is not a client (sound doc, section 9).
    if let Some(mut ring) = host.ents.get(id).map(|e| e.events) {
        for k in 0..4 {
            if ring.events[k] != 0 {
                match (n.event)(ring.events[k]) {
                    Some(ev) => ring.events[k] = ev,
                    None => (ring.events[k], ring.parms[k]) = (0, 0),
                }
            }
        }
        let mut set = |name: &str, v: i32| seti(&mut e, name, v);
        ring.write(&mut set);
    }

    // `es.loopSound`, what `playLoopSound` left on the entity. State rather
    // than an event, so it rides every snapshot until `stopLoopSound` clears
    // it (sound doc, section 9).
    if let Some(idx) = host.ents.get(id).map(|e| e.loop_sound) {
        seti(&mut e, "loopSound", idx);
    }

    // An item overwrites it with its own below.
    if let Some(g) = host.ents.get(id).map(|e| e.ground) {
        seti(&mut e, "groundEntityNum", g);
    }
    match kind {
        Kind::Item(item) => {
            seti(&mut e, "eType", n.et_item);
            seti(&mut e, "index", i32::from(item.index));
            let nodraw = if item.nodraw { n.ef_nodraw } else { 0 };
            seti(&mut e, "eFlags", n.item_eflags | nodraw);
            seti(&mut e, "groundEntityNum", item.ground);
            seti(
                &mut e,
                "clientNum",
                item.owner.map_or(ITEM_CLIENTNUM, i32::from),
            );
            // In the air the item's own trajectories, which `G_RunItem` and
            // the spin run (docs/research/cod11-items.md 8, 14).
            for (group, tr) in [("pos", item.pos), ("apos", item.apos)] {
                if let Some(tr) = tr {
                    let tr_type = (n.tr_type)(tr.tr_type);
                    Trajectory { tr_type, ..tr }.write(&mut e, p, group);
                }
            }
        }
        Kind::ScriptMover(model) => {
            seti(&mut e, "eType", n.et_scriptmover);
            seti(&mut e, "index", model);
            seti(&mut e, "eFlags", eflags_hidden(host, id));
        }
        Kind::Mover(model) => {
            seti(&mut e, "eType", n.et_mover);
            seti(&mut e, "index", model);
            seti(&mut e, "solid", SOLID_BMODEL);
            seti(&mut e, "eFlags", eflags_hidden(host, id));
        }
        Kind::BrushModel(model) => {
            seti(&mut e, "eType", n.et_scriptmover);
            seti(&mut e, "index", model);
            seti(&mut e, "solid", SOLID_BMODEL);
            seti(&mut e, "eFlags", eflags_hidden(host, id));
        }
        // `InitScriptMover` (0x60214): `eType` 8, `svFlags` 0x80, which
        // `SP_script_origin` links (0x61055) with contents 0, so it has
        // no clusters and rides no snapshot until a `playSound` makes it
        // broadcast.
        Kind::ScriptOrigin => seti(&mut e, "eType", n.et_scriptmover),
        // A trigger's spawn sets `SVF_NOCLIENT` and, but on a
        // `trigger_use`, `eFlags` 2 (`InitTrigger` 0x65784,
        // `SP_trigger_use` 0x5742c, map-cycle doc 3.3).
        Kind::Trigger(t) => {
            if let Some(model) = host.ents.get(id).and_then(|e| e.brush_model) {
                seti(&mut e, "index", model as i32);
                seti(&mut e, "solid", SOLID_BMODEL);
            }
            if host.profile.trigger_wire != TriggerWire::Cod1 {
                // CoD2's `InitTrigger` (0x811c504) ORs in `eFlags` 1;
                // `trigger_radius` never calls it, nor does `SP_trigger_use`
                // (0x810fe50), which stores the hint string slot and the
                // cursor hint in `s+0xd8` and `s+0xdc` (cod2-protocol.md 15.6).
                if t.kind == crate::game::trigger::TriggerKind::Use {
                    seti(&mut e, "scale", t.hint_string & 0xff);
                    seti(&mut e, "dmgFlags", t.cursor_hint);
                } else if t.round.is_none() && t.kind != crate::game::trigger::TriggerKind::LookAt {
                    let flags = if host.profile.trigger_wire == TriggerWire::Cod4 {
                        COD4_EF_TRIGGER
                    } else {
                        COD2_EF_TRIGGER
                    };
                    seti(&mut e, "eFlags", flags);
                }
            } else if t.kind == crate::game::trigger::TriggerKind::Use {
                // `ent+0xd8` and `ent+0xdc` sit inside the entity state.
                seti(&mut e, "_union.scale", t.hint_string & 0xff);
                seti(&mut e, "dmgFlags", t.cursor_hint);
            } else {
                seti(&mut e, "eFlags", EF_TRIGGER);
            }
        }
        // `playLoopedFx` (game.mp 0x5b536..0x5b585).
        Kind::Fx(fx) => {
            seti(&mut e, "eType", n.et_fx);
            seti(&mut e, "_union.scale", fx.id);
            for (axis, v) in fx.forward.iter().enumerate() {
                setf(&mut e, &format!("origin2[{axis}]"), *v);
            }
            setf(&mut e, "angles2[0]", fx.cull);
            setf(&mut e, "angles2[1]", fx.period_ms as f32);
            seti(&mut e, "eventParm", fx.up);
        }
        Kind::SoundBlend(vehicle, (a, b, blend)) => {
            seti(&mut e, "eType", ET_RAW | ET_SOUND_BLEND);
            seti(&mut e, "eventParm", a);
            seti(&mut e, "_union.scale", b);
            setf(&mut e, "angles2[1]", blend);
            seti(&mut e, "otherEntityNum", vehicle as i32);
        }
        Kind::Vehicle(model) => {
            crate::game::vehicle::write_wire(host, id, model, &mut |name, v| seti(&mut e, name, v))
        }
        Kind::Turret(model, weapon) => {
            seti(&mut e, "eType", n.et_turret);
            seti(&mut e, "index", model);
            // The turret's own weapon, from its `weaponinfo` key: both gate
            // maps place `mg42_bipod_stand_mp`, configstring 7's 16.
            seti(&mut e, "weapon", weapon);
            // Transcribed from the traces, not derived: every turret in both
            // captures carries 3 here and nothing in the module has been read
            // that says why. A link writes 1 instead, and its `G_EntUnlink`
            // 0 for good (movers doc 16).
            let angle_set = host.turrets.get(&id).is_some_and(|r| r.angle_set);
            if !host.links.contains(id) && !angle_set {
                seti(&mut e, "apos.trType", (n.tr_type)(TURRET_APOS_TRTYPE));
            }
            // The barrel's own record, built at spawn (`game::turret`). A
            // host with no collision world built no record and sends the
            // entity with everything else at its zeroed default: rest pitch
            // 0, no view lock, no firing bit.
            if let Some(rec) = host.turrets.get(&id) {
                setf(&mut e, "angles2[0]", rec.angles2[0]);
                setf(&mut e, "angles2[1]", rec.angles2[1]);
                if let Some(arcs) = rec.wire_arcs(host.defs.proto) {
                    for (name, v) in ["leanf", "origin2[0]", "origin2[1]", "origin2[2]"]
                        .into_iter()
                        .zip(arcs)
                    {
                        setf(&mut e, name, v);
                    }
                }
                let mut eflags = 0;
                if rec.teleport_bit {
                    eflags |= n.ef_teleport;
                }
                if rec.firing {
                    eflags |= n.ef_firing;
                }
                seti(&mut e, "eFlags", eflags);
                if rec.overheat_mark {
                    seti(&mut e, "animMovetype", 1);
                }
            }
        }
    }
    Some(e)
}

/// What one classname puts on the wire, if anything.
enum Kind {
    /// An item.
    Item(crate::game::item::ItemState),
    /// A script model, by its model configstring index, 0 for none.
    ScriptMover(i32),
    /// A `script_brushmodel`, by its inline model number.
    BrushModel(i32),
    /// A `func_bobbing`, `func_pendulum` or `func_static`, by its inline
    /// model number, 0 for a script-spawned one (docs/research/cod11-movers.md
    /// 18 and 20).
    Mover(i32),
    /// A United Offensive vehicle, by its model configstring index.
    Vehicle(i32),
    /// One of its sound-blend entities (`SP_sound_blend` 0x5a5e7,
    /// `G_SetSoundBlend` 0x5a6a9): the vehicle and the two aliases with
    /// their crossfade.
    SoundBlend(u32, (i32, i32, f32)),
    /// A mounted MG: its model configstring index and its weapon index.
    Turret(i32, i32),
    /// A `script_origin`, linked but sent only once it is broadcast.
    ScriptOrigin,
    /// A map trigger, linked `SVF_NOCLIENT`.
    Trigger(crate::game::trigger::Trigger),
    /// A `playLoopedFx` effect.
    Fx(crate::game::builtins::fx::LoopedFx),
}

fn kind_of(host: &mut GameHost, cx: &mut Cx, id: EntId, classname: &str) -> Option<Kind> {
    if let Some(fx) = host.looped_fx.get(&id) {
        return Some(Kind::Fx(*fx));
    }
    if let Some(item) = host.ents.get(id).and_then(|e| e.item) {
        if item.taken {
            return None;
        }
        return Some(Kind::Item(item));
    }
    if host.vehicles.live.contains_key(&id) {
        return Some(Kind::Vehicle(model_index(host, cx, id).unwrap_or(0)));
    }
    if let Some((veh, state)) = crate::game::vehicle::sound_blend(host, id) {
        return Some(Kind::SoundBlend(veh.0, state));
    }
    if classname == "misc_mg42" || classname == "misc_turret" {
        // A `spawnTurret` gun has no key; its record names the weapon.
        let weapon = field_string(host, cx, id, "weaponinfo")
            .or_else(|| host.turrets.get(&id).map(|r| r.weapon.clone()))
            .and_then(|w| match host.profile.weapons {
                // The index `G_SpawnTurret` registered.
                WeaponNumbering::Registered => host
                    .cod2()
                    .weapons
                    .iter()
                    .position(|n| n.eq_ignore_ascii_case(&w))
                    .map(|i| i + 1),
                WeaponNumbering::Fixed => host.defs.weapon_index(&w),
            })
            .unwrap_or(0) as i32;
        return Some(Kind::Turret(model_index(host, cx, id)?, weapon));
    }
    // One with no model goes out too, `index` 0 (sound doc, section 9).
    if classname == "script_model" {
        return Some(Kind::ScriptMover(model_index(host, cx, id).unwrap_or(0)));
    }
    if classname == "script_brushmodel" {
        return brush_model(host, id).map(Kind::BrushModel);
    }
    if crate::game::mover::is_init_mover(host, id) {
        let e = host.ents.get(id)?;
        if e.think == Some(crate::game::entity::ThinkFn::Free) {
            return None;
        }
        return Some(Kind::Mover(e.brush_model.unwrap_or(0) as i32));
    }
    if classname == "script_origin" {
        return Some(Kind::ScriptOrigin);
    }
    if let Some(t) = host.triggers.get(id).copied() {
        let freeing = host
            .ents
            .get(id)
            .is_some_and(|e| e.think == Some(crate::game::entity::ThinkFn::Free));
        return (!freeing).then_some(Kind::Trigger(t));
    }
    None
}

/// `EF_NODRAW` when script has hidden the entity, else 0.
fn eflags_hidden(host: &GameHost, id: EntId) -> i32 {
    if host.ents.get(id).is_some_and(|e| e.hidden) {
        host.profile.entities.ef_nodraw
    } else {
        0
    }
}

/// The `N` of a brush model's `*N`, which retail sends as its `index`
/// (movers doc, section 14). `None` once `delete()` has unlinked it: the
/// entity leaves the snapshot on the frame of the call, a tenth of a second
/// before the free. A script-spawned one has no `*N` and goes out with
/// index 0, the world's model (movers doc 20).
fn brush_model(host: &GameHost, id: EntId) -> Option<i32> {
    let e = host.ents.get(id)?;
    if e.think == Some(crate::game::entity::ThinkFn::Free) {
        return None;
    }
    match e.brush_model {
        Some(n) if n > 0 => Some(n as i32),
        _ => e.link.is_some().then_some(0),
    }
}

/// The entity's `.model` as a model configstring index, 1-based the way the
/// range's own indexer numbers it.
fn model_index(host: &mut GameHost, cx: &mut Cx, id: EntId) -> Option<i32> {
    let name = field_string(host, cx, id, "model")?;
    match crate::configstrings::model_index(&host.configstrings, host.allocators.layout(), &name) {
        0 => None,
        i => Some(i),
    }
}

fn field_string(host: &mut GameHost, cx: &mut Cx, id: EntId, name: &str) -> Option<String> {
    let atom = cx.intern_folded(name);
    match host.get_field(cx, id, atom) {
        Value::String(s) => Some(cx.resolve(s).to_string()),
        _ => None,
    }
}

fn field_vec(host: &mut GameHost, cx: &mut Cx, id: EntId, name: &str) -> Option<[f32; 3]> {
    let atom = cx.intern_folded(name);
    match host.get_field(cx, id, atom) {
        Value::Vector(v) => Some(v),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::testing::fixture;

    /// The eleven tween fields each land at their own `hud_field` wire index
    /// (docs/research/cod11-gsc-object-model.md 23.4): a round-trip with
    /// eleven distinct values catches a copy-pasted index the way the two
    /// builtin tests, each exercising only their own tween, cannot.
    #[test]
    fn build_hud_elem_writes_every_tween_field_at_its_own_index() {
        let (mut vm, mut host) = fixture();
        vm.with_cx(|cx| {
            use crate::game::builtins::hud::new_hud_elem;
            let Value::Entity(id) = new_hud_elem(&mut host, cx, None, &[]).unwrap() else {
                panic!("newHudElem returns an object");
            };
            let state = HudState {
                from_color: 1,
                fade_start: 2,
                fade_ms: 3,
                scale_start: 4,
                scale_ms: 5,
                from_width: 6,
                from_height: 7,
                move_start: 8,
                move_ms: 9,
                from_x: 10,
                from_y: 11,
                ..host.ents.get(id).unwrap().hud.unwrap()
            };
            let e = build_hud_elem(&host, id, &state);
            use vcod_common::net::msg::hud_field as f;
            assert_eq!(e.get(f::FROM_COLOR), 1);
            assert_eq!(e.get(f::FADE_START_TIME), 2);
            assert_eq!(e.get(f::FADE_TIME), 3);
            assert_eq!(e.get(f::SCALE_START_TIME), 4);
            assert_eq!(e.get(f::SCALE_TIME), 5);
            assert_eq!(e.get(f::FROM_WIDTH), 6);
            assert_eq!(e.get(f::FROM_HEIGHT), 7);
            assert_eq!(e.get(f::MOVE_START_TIME), 8);
            assert_eq!(e.get(f::MOVE_TIME), 9);
            assert_eq!(e.get(f::FROM_X), 10);
            assert_eq!(e.get(f::FROM_Y), 11);
        });
    }
}
