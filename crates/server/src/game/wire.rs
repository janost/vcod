//! The object table as packet entities.
//!
//! What reaches a client is what retail links and does not mark `NOCLIENT`,
//! and the traces say a stock map's static set is items, scriptmovers and
//! turrets (`docs/protocol-1.1.md`, "Which entities a client is sent"). This
//! module builds that set; the culling that decides which of them one client
//! is sent is the caller's business.

use super::host::GameHost;
use crate::configstrings::CsRange;
use std::collections::BTreeMap;
use vcod_common::net::msg::EntityState;
use vcod_common::net::protocol::{Protocol, ENTITYNUM_WORLD};
use vcod_gsc::EntId;
use vcod_gsc::{Cx, Host, Value};

/// `ET_ITEM`: a placed weapon, `index` a 1-based index into configstring 7.
const ET_ITEM: i32 = 3;
/// `ET_SCRIPTMOVER`: a script model, `index` a model configstring index.
const ET_SCRIPTMOVER: i32 = 8;
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
