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
//! - `72..`: map and script entities, from `entity::FIRST_MAP_ENTITY` up.
//! - `958..1021`: temp entities, walked by a rolling cursor
//!   (`crate::game::temp_entity`). Retail's `G_TempEntity` takes whatever
//!   free slot it finds instead; vcod reserves a block at the top so a
//!   one-frame event can never take a number the object table is about to
//!   hand out. A map whose script spawns past 958 would collide, which no
//!   stock map comes near.
//! - `1022`: `ENTITYNUM_WORLD`, `1023`: `ENTITYNUM_NONE`.
//!
//! Bodies and temp entities are appended by `crate::server`, not here: the
//! A/B gates read `packet_entities` for the map's own static set.

use super::host::GameHost;
use crate::configstrings::CsRange;
use crate::game::entity::{HudState, HUD_OWNER_ALL};
use std::collections::BTreeMap;
use vcod_common::net::msg::{EntityState, HudElem, MAX_HUD_ELEMS};
use vcod_common::net::protocol::{Protocol, ENTITYNUM_WORLD};
use vcod_gsc::EntId;
use vcod_gsc::{Cx, Host, Value};

/// `ET_ITEM`: a placed weapon, `index` a 1-based index into configstring 7.
const ET_ITEM: i32 = 3;
/// `ET_SCRIPTMOVER`: a script model, `index` a model configstring index.
const ET_SCRIPTMOVER: i32 = 8;
/// `ET_PLAYER`: another client.
const ET_PLAYER: i32 = 1;
/// A mounted MG. Not in CoDExtended's `entityType_t`, read off the traces:
/// carentan's and pavlov's `misc_mg42`s arrive as 11 with the `mg42_bipod`
/// model index.
const ET_TURRET: i32 = 11;

/// `eFlags` and `clientNum` as the traces carry them on a placed weapon. Both
/// are constants of the spawn path rather than anything we compute, so they
/// are transcribed with the capture as their evidence.
const ITEM_EFLAGS: i32 = 16;
const ITEM_CLIENTNUM: i32 = 254;
/// A turret's `apos.trType` in both maps' traces.
const TURRET_APOS_TRTYPE: i32 = 3;

/// The `r.mins`/`r.maxs` an entity's spawn function leaves on it. The clusters
/// it links with come from this grown by the engine's link epsilon, which is
/// the caller's business (`crate::world::visible_entities`).
///
/// VERIFIED from the module: `G_SpawnItem` (0x4e6ed) writes (-1, -1, -1) to
/// (1, 1, 1), and `G_SpawnTurret` (0x52f75) writes (-32, -32, 0) to
/// (32, 32, 56).
pub fn link_box(etype: i32) -> ([f32; 3], [f32; 3]) {
    match etype {
        ET_TURRET => ([-32.0, -32.0, 0.0], [32.0, 32.0, 56.0]),
        // A script model is spawned with a zero box and nothing on the
        // `SP_script_model` path ever writes one, so its clusters come from
        // the engine's link epsilon alone. A `script_brushmodel` shares this
        // `eType` but takes real bounds from `trap_SetBrushModel`; no trace
        // we hold carries one (docs/protocol-1.1.md).
        ET_SCRIPTMOVER => ([0.0; 3], [0.0; 3]),
        // A player links with its own movement box, the one pmove collides
        // with (`vcod_common::pmove`). A corpse keeps the player's box: the
        // clone copies the dying entity's bounds, which `player_die` has
        // already flattened to 30 units tall (cod11-combat.md 5.2). The
        // extra height only widens the cluster set, so the taller box is
        // the conservative one to cull with.
        ET_PLAYER | crate::game::bodies::ET_CORPSE => (
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
        _ => ([-1.0, -1.0, -1.0], [1.0, 1.0, 1.0]),
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
    e
}

/// One HUD engine slot as the integer the wire wants. The enum fields
/// (`font`, `alignx`, `aligny`) are stored as retail's index and read back
/// as a name, so this takes the slot rather than going through `get_field`.
fn hud_field_i32(host: &GameHost, id: EntId, name: &str) -> i32 {
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

fn hud_slot_value(host: &GameHost, id: EntId, name: &str) -> Option<Value> {
    let crate::game::fields::Route::Engine { slot, .. } = crate::game::fields::route_hud(name)
    else {
        return None;
    };
    host.ents.get(id).map(|e| e.engine[slot])
}

/// Builds one `EntityState` per linked object, keyed by entity number.
pub fn packet_entities(
    host: &mut GameHost,
    cx: &mut Cx,
    p: &Protocol,
) -> BTreeMap<u32, EntityState> {
    let ids: Vec<EntId> = host.ents.iter_inuse().map(|(id, _)| id).collect();
    let mut out = BTreeMap::new();
    for id in ids {
        if let Some(e) = build(host, cx, p, id) {
            out.insert(id.0, e);
        }
    }
    out
}

/// The one entity, or `None` when nothing links it.
fn build(host: &mut GameHost, cx: &mut Cx, p: &Protocol, id: EntId) -> Option<EntityState> {
    let classname = field_string(host, cx, id, "classname")?;
    let kind = kind_of(host, cx, id, &classname)?;

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

    let origin = field_vec(host, cx, id, "origin").unwrap_or([0.0; 3]);
    let angles = field_vec(host, cx, id, "angles").unwrap_or([0.0; 3]);
    for (axis, v) in origin.iter().enumerate() {
        setf(&mut e, &format!("pos.trBase[{axis}]"), *v);
    }
    for (axis, v) in angles.iter().enumerate() {
        setf(&mut e, &format!("apos.trBase[{axis}]"), *v);
    }

    match kind {
        Kind::Item(weapon) => {
            seti(&mut e, "eType", ET_ITEM);
            seti(&mut e, "index", weapon);
            seti(&mut e, "eFlags", ITEM_EFLAGS);
            seti(&mut e, "groundEntityNum", ENTITYNUM_WORLD as i32);
            seti(&mut e, "clientNum", ITEM_CLIENTNUM);
        }
        Kind::ScriptMover(model) => {
            seti(&mut e, "eType", ET_SCRIPTMOVER);
            seti(&mut e, "index", model);
        }
        Kind::Turret(model, weapon) => {
            seti(&mut e, "eType", ET_TURRET);
            seti(&mut e, "index", model);
            // The turret's own weapon, from its `weaponinfo` key: both gate
            // maps place `mg42_bipod_stand_mp`, configstring 7's 16.
            seti(&mut e, "weapon", weapon);
            // Transcribed from the traces, not derived: every turret in both
            // captures carries 3 here and nothing in the module has been read
            // that says why.
            seti(&mut e, "apos.trType", TURRET_APOS_TRTYPE);
            // The barrel's settled pitch, from the sweep
            // `game::spawn::settle_turret_pitch` runs at map load. A host with
            // no collision world ran no sweep and sends zero.
            if let Some(pitch) = host.turret_pitch.get(&id).copied() {
                setf(&mut e, "angles2[0]", pitch);
            }
        }
    }
    Some(e)
}

/// What one classname puts on the wire, if anything.
enum Kind {
    /// A placed weapon, by its 1-based configstring 7 index.
    Item(i32),
    /// A script model, by its model configstring index.
    ScriptMover(i32),
    /// A mounted MG: its model configstring index and its weapon index.
    Turret(i32, i32),
}

fn kind_of(host: &mut GameHost, cx: &mut Cx, id: EntId, classname: &str) -> Option<Kind> {
    if classname == "misc_mg42" || classname == "misc_turret" {
        let weapon = field_string(host, cx, id, "weaponinfo")
            .and_then(|w| crate::configstrings::weapon_index(&w))
            .unwrap_or(0) as i32;
        return Some(Kind::Turret(model_index(host, cx, id)?, weapon));
    }
    if classname.starts_with("mpweapon_") {
        let weapon = field_string(host, cx, id, "weaponinfo")
            .or_else(|| crate::game::spawn::radiant_weapon(classname).map(str::to_string))?;
        return Some(Kind::Item(
            crate::configstrings::weapon_index(&weapon)? as i32
        ));
    }
    if classname == "script_model" {
        return Some(Kind::ScriptMover(model_index(host, cx, id)?));
    }
    // `script_brushmodel` is linked in retail too, but neither gate map has
    // one, and its `.model` is a `*N` submodel rather than a name in the
    // model configstring range, so what its `index` carries is unmeasured.
    // Left out until a map with one says.
    None
}

/// The entity's `.model` as a model configstring index, 1-based the way the
/// range's own indexer numbers it.
fn model_index(host: &mut GameHost, cx: &mut Cx, id: EntId) -> Option<i32> {
    let name = field_string(host, cx, id, "model")?;
    let (first, last) = CsRange::Model.bounds();
    let slot = host.configstrings[first..=last]
        .iter()
        .position(|cs| *cs == name)?;
    Some((slot + 1) as i32)
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
