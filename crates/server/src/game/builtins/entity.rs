//! Entity builtins the map-load and spawn paths reach: lookup, spawn (both
//! the map-entity form and `ClientSpawn`), delete, visibility/solidity and
//! setModel. Per-family dispatch: `NAMES`/`lookup` is matched against
//! `Cx::resolve_folded`, same shape `host.rs` uses for the env/io names, and
//! Task 9 adds more families beside this one.

use crate::configstrings::CsRange;
use crate::game::entity::{ThinkFn, FIRST_HUD_ELEM};
use crate::game::host::{GameHost, LinkOp, SpawnMode, SpawnRequest};
use crate::server::MAX_CLIENTS;
use glam::Vec3;
use vcod_gsc::{ArrayKey, Cx, EntId, ErrorKind, Host, Target, Value};

/// `delete()`'s deferred-free window: the `delete` entity method (0x5da14,
/// unnamed in either of `game.mp.i386.so`'s symbol tables) sets `think =
/// G_FreeEntity` with `nextthink = level.time + 100` rather than freeing on
/// the spot (docs/research/cod11-gsc-object-model.md section 14, from
/// disassembly). `probe_delete`'s capture is consistent with a defer
/// somewhere in (0, 150] ms but does not pin 100 specifically; see the note
/// on `probe_delete_matches_retail` in `crates/server/tests/semantics_ents.rs`.
const DELETE_DEFER_MS: i32 = 100;

pub type Builtin = fn(&mut GameHost, &mut Cx, Option<Target>, &[Value]) -> Result<Value, ErrorKind>;

pub const NAMES: &[(&str, Builtin)] = &[
    ("getent", get_ent),
    ("getentarray", get_ent_array),
    ("spawn", spawn),
    ("spawnstruct", spawn_struct),
    ("delete", delete),
    ("show", show),
    ("hide", hide),
    ("solid", solid),
    ("notsolid", not_solid),
    ("setmodel", set_model),
    ("getorigin", get_origin),
    ("getentitynumber", get_entity_number),
    ("isplayer", is_player),
    ("isalive", is_alive),
    ("isdefined", is_defined),
    ("istouching", is_touching),
    ("placespawnpoint", place_spawnpoint),
    ("linkto", link_to),
    ("unlink", unlink),
];

pub fn lookup(folded: &str) -> Option<Builtin> {
    NAMES.iter().find(|(n, _)| *n == folded).map(|(_, f)| *f)
}

/// `getEntArray`'s equality. String atoms are interned exactly
/// (`Cx::intern_exact`), so two spellings of one text are two different
/// atoms; comparing resolved text rather than the atom itself is what makes
/// two equal-looking strings match.
fn values_match(cx: &Cx, a: Value, b: Value) -> bool {
    match (a, b) {
        (Value::String(x), Value::String(y)) => cx.resolve(x) == cx.resolve(y),
        _ => a == b,
    }
}

/// The receiver a call like `ent hide()` carries; anything else is a type
/// error, the same shape a field access on a non-entity would raise. Shared
/// with the other families whose builtins are entity methods, so there is
/// one definition of what a valid receiver is.
pub(crate) fn entity_receiver(recv: Option<Target>) -> Result<EntId, ErrorKind> {
    match recv {
        Some(Target::Entity(id)) => Ok(id),
        _ => Err(ErrorKind::BadType("needs an entity receiver")),
    }
}

/// `getEntArray(value, key)`. `Scr_GetEntArray` (0x61980) walks slots
/// 0..level.num_entities, skips a slot whose `inuse` is clear, and appends
/// matches, so the result is ascending entity number
/// (docs/research/cod11-gsc-object-model.md section 10). The key names a
/// field and is resolved exactly as `.name` is: engine table, then client
/// table, then the entity's script struct. The corpus passes six distinct
/// keys and two of them are radiant keys, so a special case for the engine
/// table would be wrong.
///
/// `getEntArray()` with no arguments is the second form: every entity in
/// use, no filter. `Scr_GetEntArray` branches on the parameter count at
/// 0x61989 and runs the same slot walk with the field compare dropped;
/// `_gameobjects::main` is the caller in the stock corpus.
pub fn get_ent_array(
    host: &mut GameHost,
    cx: &mut Cx,
    _recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    if args.is_empty() {
        let ids: Vec<EntId> = host.ents.iter_inuse().map(|(id, _)| id).collect();
        let arr = cx.new_array();
        for (n, id) in ids.into_iter().enumerate() {
            cx.set_index(arr, ArrayKey::Int(n as i32), Value::Entity(id));
        }
        return Ok(Value::Array(arr));
    }
    let [want, Value::String(key)] = args else {
        return Err(ErrorKind::BadType("getEntArray takes a value and a key"));
    };
    let (want, key) = (*want, *key);
    // `cx.resolve(key)` inside `cx.intern_folded(...)`'s argument would
    // double-borrow cx; lowercase into an owned local first.
    let lowered = cx.resolve(key).to_ascii_lowercase();
    let field = cx.intern_folded(&lowered);

    let ids: Vec<EntId> = host.ents.iter_inuse().map(|(id, _)| id).collect();
    let arr = cx.new_array();
    let mut n = 0;
    for id in ids {
        let got = host.get_field(cx, id, field);
        if values_match(cx, got, want) {
            cx.set_index(arr, ArrayKey::Int(n), Value::Entity(id));
            n += 1;
        }
    }
    Ok(Value::Array(arr))
}

/// The single-result form of `getEntArray`: `undefined` on a miss, which is
/// what every `isDefined(getEnt(...))` in the corpus tests. `Scr_GetEnt` is
/// its own function on retail and takes no unfiltered form, so the argument
/// check is here rather than shared with `get_ent_array`.
pub fn get_ent(
    host: &mut GameHost,
    cx: &mut Cx,
    recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    if args.len() != 2 {
        return Err(ErrorKind::BadType("getEnt takes a value and a key"));
    }
    let Value::Array(arr) = get_ent_array(host, cx, recv, args)? else {
        unreachable!("get_ent_array always returns an array");
    };
    if cx.array_len(arr) == 0 {
        Ok(Value::Undefined)
    } else {
        Ok(cx.get_index(arr, ArrayKey::Int(0)))
    }
}

/// Two retail builtins share the name `spawn`, in two different tables:
/// `spawn(classname, origin)` is free function 8 (0x5d268) and
/// `self spawn(origin, angles)` is player method 40 (0x455cc). Retail picks
/// by call form; one name reaches dispatch here, so the receiver is what
/// tells them apart. `Op::CallBuiltin` only supplies a receiver for an actual
/// method call, so a free `spawn(...)` inside a client's own thread still
/// lands on the free form. Object model doc, section 20.
pub fn spawn(
    host: &mut GameHost,
    cx: &mut Cx,
    recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    match recv {
        Some(Target::Entity(id)) => client_spawn(host, cx, id, args),
        _ => spawn_entity(host, cx, args),
    }
}

/// `self spawn(origin, angles)` on a client: the wrapper at 0x455cc, which
/// rejects a receiver with no `ent->client` and then calls `ClientSpawn`
/// (0x4268c). Moves the client to the spawn point and restarts its movement
/// in whatever mode the script's `sessionstate` put it in. Object model doc,
/// section 20.
///
/// Clearing the weapon set here is inferred from the stock scripts' ordering,
/// not read out of `ClientSpawn`; the doc section says why it has to happen.
/// The sim itself lives in `Server`, out of a builtin's reach, so the move
/// leaves as a queued `SpawnRequest`.
fn client_spawn(
    host: &mut GameHost,
    cx: &mut Cx,
    id: EntId,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    let slot = match host.ents.get(id) {
        Some(e) if e.client.is_some() => id.0 as usize,
        _ => return Err(ErrorKind::BadType("that entity is not a player")),
    };
    let [Value::Vector(origin), Value::Vector(angles)] = args else {
        return Err(ErrorKind::BadType("spawn takes an origin and angles"));
    };
    let (origin, angles) = (*origin, *angles);

    let state = cx.intern_folded("sessionstate");
    let mode = match host.get_field(cx, id, state) {
        Value::String(s) => match cx.resolve(s) {
            "playing" => SpawnMode::Player,
            "intermission" => SpawnMode::Intermission,
            // A dead client is not simulated either, and no death goes
            // through this builtin.
            _ => SpawnMode::Spectator,
        },
        _ => SpawnMode::Spectator,
    };

    let origin_field = cx.intern_folded("origin");
    host.set_field(cx, id, origin_field, Value::Vector(origin))?;
    let angles_field = cx.intern_folded("angles");
    host.set_field(cx, id, angles_field, Value::Vector(angles))?;

    host.client_weapons[slot] = crate::weapons::PlayerWeapons::default();
    // `ClientSpawn` gives the sim a fresh playerstate, ammo included; the
    // script's gives that follow land on this.
    host.client_ammo[slot] = crate::game::host::AmmoArrays::default();
    // `ClientSpawn` re-stores `sess.maxHealth` into the fresh playerstate
    // (docs/protocol-1.1.md, "Block 1"); the gametype writes both fields
    // again right after, so this is what a spawn without that script does.
    let v = &mut host.client_vitals[slot];
    v.health = v.max_health;
    v.dead = false;
    host.client_spawns.push(SpawnRequest {
        slot,
        origin,
        yaw_deg: angles[1],
        mode,
    });
    Ok(Value::Undefined)
}

/// `spawn(classname, origin)`: a live entity with both fields set, numbered
/// after everything already in the table. An item classname makes an item
/// (`dm.gsc`'s `dropHealth` spawns `item_health`).
fn spawn_entity(host: &mut GameHost, cx: &mut Cx, args: &[Value]) -> Result<Value, ErrorKind> {
    let [Value::String(cls), Value::Vector(at)] = args else {
        return Err(ErrorKind::BadType("spawn takes a classname and an origin"));
    };
    let (cls, at) = (*cls, *at);
    let id = host.ents.spawn(cx)?;
    let cn = cx.intern_folded("classname");
    host.set_field(cx, id, cn, Value::String(cls))?;
    let og = cx.intern_folded("origin");
    host.set_field(cx, id, og, Value::Vector(at))?;
    // `G_SpawnItem` for a `bg_itemlist` classname: registered, an item, and
    // on the floor, where retail's `G_RunItem` would drop it a frame later.
    let name = cx.resolve(cls).to_string();
    if let Some(index) = crate::items::classname_index(&name) {
        let item = crate::items::item_name(index).unwrap_or(&name).to_string();
        host.register_item(&item);
        crate::game::item::attach(host, id, index);
        let weapon = crate::game::spawn::is_weapon_row(index);
        crate::game::spawn::drop_item_to_floor(host, cx, id, weapon);
    }
    Ok(Value::Entity(id))
}

/// `spawnStruct()`: a bare struct, unrelated to the entity table.
pub fn spawn_struct(
    _host: &mut GameHost,
    cx: &mut Cx,
    _recv: Option<Target>,
    _args: &[Value],
) -> Result<Value, ErrorKind> {
    Ok(Value::Struct(cx.new_struct()))
}

/// `delete()` defers the free rather than performing it now: it arms the
/// entity's think for `DELETE_DEFER_MS` out, so a later `getEntArray` still
/// sees it until that think comes due (`ScriptRuntime::run_frame`'s think
/// pass). `_load.gsc`'s exploder threads end with one. The clip goes now:
/// a `script_brushmodel`'s brushes are in the world only through its link
/// (`SP_script_brushmodel`, game.mp 0x60fb8), and retail's `G_FreeEntity`
/// unlinks, which is how `_gameobjects::main` takes carentan's bombzone
/// clips out of every gametype but sd (docs/research/cod11-mantle.md, "A
/// submodel's brushes are its entity's").
pub fn delete(
    host: &mut GameHost,
    cx: &mut Cx,
    recv: Option<Target>,
    _args: &[Value],
) -> Result<Value, ErrorKind> {
    let id = entity_receiver(recv)?;
    link_submodel(host, cx, id, false);
    host.ents
        .schedule(id, ThinkFn::Free, host.level_time_ms + DELETE_DEFER_MS);
    Ok(Value::Undefined)
}

fn set_hidden(host: &mut GameHost, recv: Option<Target>, hidden: bool) -> Result<Value, ErrorKind> {
    let id = entity_receiver(recv)?;
    let e = host
        .ents
        .get_mut(id)
        .ok_or(ErrorKind::BadType("no such entity"))?;
    e.hidden = hidden;
    Ok(Value::Undefined)
}

/// Links or unlinks the brushes of an entity whose `model` is the `*N`
/// spelling of a BSP submodel; any other model, and any host with no map
/// loaded, is a no-op.
fn link_submodel(host: &mut GameHost, cx: &mut Cx, id: EntId, linked: bool) {
    let model = cx.intern_folded("model");
    let Value::String(m) = host.get_field(cx, id, model) else {
        return;
    };
    let Some(n) = cx
        .resolve(m)
        .strip_prefix('*')
        .and_then(|n| n.parse::<usize>().ok())
    else {
        return;
    };
    // Model 0 is the world clip itself, not a submodel; unlinking it would
    // take every world brush out of every trace, and a stock `notsolid()` on
    // a `"*0"` entity would do exactly that.
    if n == 0 {
        return;
    }
    if let Some(world) = &host.world {
        world.collision.set_model_linked(n, linked);
    }
}

/// The flag is what the wire build reads; the link is what the clip reads,
/// since a submodel's brushes are in the world only through its entity
/// (docs/research/cod11-mantle.md, "A submodel's brushes are its entity's").
/// `_load.gsc` `notsolid()`s every `exploder` brush model at map load, which
/// is the only place three stock maps lose that collision.
fn set_solid(
    host: &mut GameHost,
    cx: &mut Cx,
    recv: Option<Target>,
    solid: bool,
) -> Result<Value, ErrorKind> {
    let id = entity_receiver(recv)?;
    let e = host
        .ents
        .get_mut(id)
        .ok_or(ErrorKind::BadType("no such entity"))?;
    e.solid = solid;
    link_submodel(host, cx, id, solid);
    Ok(Value::Undefined)
}

/// `hide()`/`show()` and `solid()`/`notSolid()` flip real flags on the
/// entity, not script-struct keys: `_load.gsc` hides every exploder model
/// at load and stage 5 reads these when it builds entity states.
pub fn hide(
    host: &mut GameHost,
    _cx: &mut Cx,
    recv: Option<Target>,
    _args: &[Value],
) -> Result<Value, ErrorKind> {
    set_hidden(host, recv, true)
}

pub fn show(
    host: &mut GameHost,
    _cx: &mut Cx,
    recv: Option<Target>,
    _args: &[Value],
) -> Result<Value, ErrorKind> {
    set_hidden(host, recv, false)
}

pub fn solid(
    host: &mut GameHost,
    cx: &mut Cx,
    recv: Option<Target>,
    _args: &[Value],
) -> Result<Value, ErrorKind> {
    set_solid(host, cx, recv, true)
}

pub fn not_solid(
    host: &mut GameHost,
    cx: &mut Cx,
    recv: Option<Target>,
    _args: &[Value],
) -> Result<Value, ErrorKind> {
    set_solid(host, cx, recv, false)
}

/// `setModel(name)` allocates a model configstring slot and stores the name,
/// so `.model` reads back what was set.
pub fn set_model(
    host: &mut GameHost,
    cx: &mut Cx,
    recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    let id = entity_receiver(recv)?;
    let Some(Value::String(name)) = args.first() else {
        return Err(ErrorKind::BadType("setModel takes a model name"));
    };
    let name = *name;
    let text = cx.resolve(name).to_string();
    host.allocators
        .index(&mut host.configstrings, CsRange::Model, &text)?;
    let field = cx.intern_folded("model");
    host.set_field(cx, id, field, Value::String(name))?;
    Ok(Value::Undefined)
}

/// `getOrigin()`, the receiver's origin slot, read the way a `.origin`
/// script access is.
pub fn get_origin(
    host: &mut GameHost,
    cx: &mut Cx,
    recv: Option<Target>,
    _args: &[Value],
) -> Result<Value, ErrorKind> {
    let id = entity_receiver(recv)?;
    let origin = cx.intern_folded("origin");
    Ok(host.get_field(cx, id, origin))
}

/// `getEntityNumber()`. A HUD element is not a gentity, it has its own field
/// table, so asking for its entity number is a type error
/// (docs/research/cod11-gsc-object-model.md section 3).
pub fn get_entity_number(
    _host: &mut GameHost,
    _cx: &mut Cx,
    recv: Option<Target>,
    _args: &[Value],
) -> Result<Value, ErrorKind> {
    let id = entity_receiver(recv)?;
    if id.0 >= FIRST_HUD_ELEM {
        return Err(ErrorKind::BadType(
            "getEntityNumber on a HUD element, not a gentity",
        ));
    }
    Ok(Value::Int(id.0 as i32))
}

/// `self placeSpawnpoint()` (entity method 37, `game.mp.i386.so` 0x5bedc):
/// drops a spawnpoint onto the floor at map load. Retail point-traces from
/// the entity origin up 128 units, then from there straight down 262144,
/// moves the entity to the endpoint, keeps one word of the second trace's
/// result in `gentity_t+0x7c`, and prints "Spawn point entity %i is in
/// solid" when a third trace at the new origin starts solid.
///
/// Both traces and the move are faithful; two things are not. Retail traces
/// with contents mask 0x2810011, ours takes solid and playerclip
/// (`CollisionWorld::box_trace`), so the two disagree over any brush whose
/// contents are in one mask and not the other. And our trace carries no
/// entity identity, so the `+0x7c` word goes unrecorded. Neither is
/// observable until players spawn, which is a later stage.
pub fn place_spawnpoint(
    host: &mut GameHost,
    cx: &mut Cx,
    recv: Option<Target>,
    _args: &[Value],
) -> Result<Value, ErrorKind> {
    let id = entity_receiver(recv)?;
    let origin_field = cx.intern_folded("origin");
    let Value::Vector(origin) = host.get_field(cx, id, origin_field) else {
        return Err(ErrorKind::BadType("a spawnpoint needs an origin"));
    };
    // No collision loaded (every unit test, and any host built without a
    // map): the entity stays where the entity lump put it.
    let Some(world) = host.world.clone() else {
        return Ok(Value::Undefined);
    };
    let start = Vec3::from(origin);
    let up = world.collision.box_trace(
        start,
        start + Vec3::new(0.0, 0.0, CEILING_CHECK),
        Vec3::ZERO,
        Vec3::ZERO,
    );
    let down = world.collision.box_trace(
        up.endpos,
        up.endpos - Vec3::new(0.0, 0.0, DROP_DISTANCE),
        Vec3::ZERO,
        Vec3::ZERO,
    );
    // Retail's third trace is a point test at the placed position, not a
    // reading off the drop: `placeSpawnpoint` warns about where the
    // spawnpoint ended up.
    let placed_in_solid = world
        .collision
        .box_trace(down.endpos, down.endpos, Vec3::ZERO, Vec3::ZERO)
        .startsolid;
    if placed_in_solid {
        log::warn!(
            "gsc: spawn point entity {} is in solid at ({}, {}, {})",
            id.0,
            down.endpos.x as i32,
            down.endpos.y as i32,
            down.endpos.z as i32
        );
    }
    let placed = Value::Vector(down.endpos.into());
    host.set_field(cx, id, origin_field, placed)?;
    Ok(Value::Undefined)
}

/// How far above its origin `placeSpawnpoint` looks for a ceiling, and how
/// far below the result it looks for the floor. Both measured off the
/// literals at 0x5bf45 and 0x5bf91.
const CEILING_CHECK: f32 = 128.0;
const DROP_DISTANCE: f32 = 262144.0;

/// `isPlayer(ent)` reads the entity number against `MAX_CLIENTS`, which is
/// what makes it answerable before stage 4 gives clients any state. Retail's
/// `isPlayer` reads `ent->client`; stage 4 replaces this with the real check.
pub fn is_player(
    _host: &mut GameHost,
    _cx: &mut Cx,
    _recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    // A non-entity answers false rather than refusing: retail's `isPlayer`
    // (0x5efd4) opens with `Scr_GetType(0)` and jumps straight to
    // `Scr_AddInt(0)` for anything but type 7, so `isPlayer(undefined)` is 0.
    // VERIFIED, the type compare and both `Scr_AddInt` arms. It matters
    // because a death with no attacker hands the callbacks `undefined`
    // (`Scr_PlayerKilled` 0x5cb30) and `dm.gsc:492` calls this on it unguarded.
    let Some(Value::Entity(id)) = args.first() else {
        return Ok(Value::Int(0));
    };
    Ok(Value::Int((id.0 < MAX_CLIENTS as u32) as i32))
}

/// `isAlive(ent)` (`.so` 0x5cf8c): 0 for an argument that is not an entity,
/// otherwise `health > 0` (docs/research/cod11-gsc-object-model.md, 23.5). A
/// client's health lives on the host's vitals array, not the generic field
/// table; any other entity reads its `health` field as an int.
pub fn is_alive(
    host: &mut GameHost,
    cx: &mut Cx,
    _recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    let Some(Value::Entity(id)) = args.first() else {
        return Ok(Value::Int(0));
    };
    let id = *id;
    let alive = match host.ents.get(id) {
        Some(e) if e.client.is_some() => host.client_vitals[id.0 as usize].health > 0,
        Some(_) => {
            let health = cx.intern_folded("health");
            matches!(host.get_field(cx, id, health), Value::Int(n) if n > 0)
        }
        None => false,
    };
    Ok(Value::Int(alive as i32))
}

/// `isDefined(x)`: false for a missing argument, `undefined` itself, and a
/// handle whose slot is free. A reused slot reads live again, where retail's
/// entity generation counter would not; the S&D gate hit the HUD element case
/// at `sd.gsc:1850`/`2019` (gsc-language doc, section 10).
pub fn is_defined(
    host: &mut GameHost,
    _cx: &mut Cx,
    _recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    let defined = match args.first() {
        None | Some(Value::Undefined) => false,
        Some(Value::Entity(id)) => host.ents.get(*id).is_some(),
        Some(_) => true,
    };
    Ok(Value::Int(defined as i32))
}

/// `isTouching(other)`: a real box overlap between the receiver's absolute
/// bounds and the argument's, the same test `trap_EntitiesInBox` performs.
pub fn is_touching(
    host: &mut GameHost,
    cx: &mut Cx,
    recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    let a = entity_receiver(recv)?;
    let Some(Value::Entity(b)) = args.first() else {
        return Err(ErrorKind::BadType("isTouching takes an entity"));
    };
    let b = *b;
    let ba = crate::game::trigger::entity_abs_bounds(host, cx, a);
    let bb = crate::game::trigger::entity_abs_bounds(host, cx, b);
    Ok(Value::Int(
        crate::game::trigger::boxes_overlap(ba, bb) as i32
    ))
}

/// `self linkTo(parent [, tag, originOffset, anglesOffset])` (0x59cc4). The
/// offset is the gap the receiver already stands at, which is what retail's
/// fixed-link arm re-applies off the parent every frame; the sim owns the
/// playerstate, so this only queues the edge
/// (docs/research/cod11-gsc-object-model.md, 23.2).
///
/// The receiver must be a client: retail gates on its svFlags 0x20 and
/// `ClientSpawn` is the only writer of that bit a stock MP script reaches,
/// since neither `sd.gsc` nor `re.gsc` calls `enableLinkTo`.
pub fn link_to(
    host: &mut GameHost,
    cx: &mut Cx,
    recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    let slot = super::client::client_receiver(host, recv)?;
    let Some(&Value::Entity(parent)) = args.first() else {
        return Err(ErrorKind::BadType("linkTo needs an entity to link to"));
    };
    // The tag and the two offset vectors retail's four-argument form takes
    // are accepted and ignored: no stock script passes them.
    let origin = cx.intern_folded("origin");
    let at = |host: &mut GameHost, cx: &mut Cx, id| match host.get_field(cx, id, origin) {
        Value::Vector(v) => v,
        _ => [0.0; 3],
    };
    let child = at(host, cx, entity_receiver(recv)?);
    let anchor = at(host, cx, parent);
    let offset = [
        child[0] - anchor[0],
        child[1] - anchor[1],
        child[2] - anchor[2],
    ];
    host.client_link_ops
        .push((slot, LinkOp::Link { parent, offset }));
    Ok(Value::Undefined)
}

/// `self unlink()` (0x5d594). A no-op on an unlinked player: retail's
/// `G_EntUnlink` returns having done nothing without a link record.
pub fn unlink(
    host: &mut GameHost,
    _cx: &mut Cx,
    recv: Option<Target>,
    _args: &[Value],
) -> Result<Value, ErrorKind> {
    let slot = super::client::client_receiver(host, recv)?;
    host.client_link_ops.push((slot, LinkOp::Unlink));
    Ok(Value::Undefined)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::testing::fixture;
    use crate::world::World;
    use std::rc::Rc;

    /// The two `spawn`s the one name dispatches. A receiver picks the
    /// `ClientSpawn` form: it moves the client, queues the sim's move with
    /// the mode `sessionstate` names, and clears the weapon set the loadout
    /// is about to refill. No receiver still allocates a map entity.
    #[test]
    fn a_receiver_picks_client_spawn_and_no_receiver_the_map_entity() {
        let (mut vm, mut host) = fixture();
        vm.with_cx(|cx| {
            let e = host.ents.spawn_client(cx, 2, None).unwrap();
            let t = Some(Target::Entity(e));
            let state = cx.intern_folded("sessionstate");
            let playing = Value::String(cx.intern_exact("playing"));
            host.set_field(cx, e, state, playing).unwrap();
            host.client_weapons[2].give(4, 3);

            let at = Value::Vector([10.0, 20.0, 30.0]);
            let angles = Value::Vector([0.0, 90.0, 0.0]);
            spawn(&mut host, cx, t, &[at, angles]).unwrap();

            assert_eq!(host.client_spawns.len(), 1);
            let s = &host.client_spawns[0];
            assert_eq!(
                (s.slot, s.origin, s.yaw_deg, s.mode),
                (2, [10.0, 20.0, 30.0], 90.0, SpawnMode::Player)
            );
            assert_eq!(
                host.client_weapons[2],
                crate::weapons::PlayerWeapons::default()
            );
            let origin = cx.intern_folded("origin");
            assert_eq!(host.get_field(cx, e, origin), at);

            // The other two modes go through the same builtin: the
            // `sessionstate` string is the whole of what tells them apart
            // (map-cycle doc, 6.1).
            for (state_name, mode) in [
                ("spectator", SpawnMode::Spectator),
                ("intermission", SpawnMode::Intermission),
            ] {
                let v = Value::String(cx.intern_exact(state_name));
                host.set_field(cx, e, state, v).unwrap();
                spawn(&mut host, cx, t, &[at, angles]).unwrap();
                assert_eq!(host.client_spawns.last().unwrap().mode, mode);
            }

            // No receiver is still the free function.
            let cls = Value::String(cx.intern_exact("script_model"));
            let Value::Entity(made) = spawn(&mut host, cx, None, &[cls, at]).unwrap() else {
                panic!("the free form returns the entity it made");
            };
            assert_eq!(made, EntId(crate::game::entity::FIRST_MAP_ENTITY, 0));
            assert_eq!(host.client_spawns.len(), 3);
        });
    }

    /// `dropHealth()`'s `spawn("item_health", origin)` makes an item: row 68
    /// registered and on the wire as an `ET_ITEM` no dropper owns.
    #[test]
    fn a_script_spawned_item_health_is_an_item_on_the_wire() {
        let (mut vm, mut host) = fixture();
        let id = vm.with_cx(|cx| {
            let cls = Value::String(cx.intern_exact("item_health"));
            let at = Value::Vector([10.0, 20.0, 30.0]);
            let Value::Entity(id) = spawn(&mut host, cx, None, &[cls, at]).unwrap() else {
                panic!("spawn returns the entity");
            };
            id
        });
        assert_eq!(host.ents.get(id).unwrap().item.unwrap().index, 68);
        assert_eq!(&host.configstrings[8][17..], "1");
        let p = &vcod_common::net::protocol::PROTOCOL_V1;
        let ents = vm.with_cx(|cx| crate::game::wire::packet_entities(&mut host, cx, p));
        let e = &ents[&id.0];
        assert_eq!(e.field_i32(p, "eType"), 3);
        assert_eq!(e.field_i32(p, "index"), 68);
        assert_eq!(e.field_i32(p, "clientNum"), 254);
        assert_eq!(e.field_i32(p, "groundEntityNum"), 1022);
    }

    /// `spawn` on an entity that is not a player is retail's
    /// `entity %i is not a player`, not a silently ignored move.
    #[test]
    fn client_spawn_refuses_a_non_client_receiver() {
        let (mut vm, mut host) = fixture();
        vm.with_cx(|cx| {
            let prop = host.ents.spawn(cx).unwrap();
            let at = Value::Vector([0.0; 3]);
            assert!(spawn(&mut host, cx, Some(Target::Entity(prop)), &[at, at]).is_err());
            assert!(host.client_spawns.is_empty());
        });
    }

    /// `placeSpawnpoint` drops the entity onto the floor: the test world's
    /// floor is at z = 0, so a spawnpoint hovering at 100 lands there.
    #[test]
    fn placespawnpoint_drops_the_spawnpoint_onto_the_floor() {
        let (mut vm, mut host) = fixture();
        host.world = Some(Rc::new(World {
            collision: vcod_common::collision::test_world(&[]),
            vis: vcod_common::bsp::Visibility::none(),
            spawn: ([0.0, 0.0, 64.0], 0.0),
        }));
        vm.with_cx(|cx| {
            let e = host.ents.spawn(cx).unwrap();
            let origin = cx.intern_folded("origin");
            host.set_field(cx, e, origin, Value::Vector([0.0, 0.0, 100.0]))
                .unwrap();
            place_spawnpoint(&mut host, cx, Some(Target::Entity(e)), &[]).unwrap();
            let Value::Vector(placed) = host.get_field(cx, e, origin) else {
                panic!("a spawnpoint keeps a vector origin");
            };
            assert!(
                placed[2].abs() < 1.0,
                "expected the floor at z = 0, got {placed:?}"
            );
        });
    }

    /// The submodel test world: model 0's floor plus a door brush local
    /// (-8..8)^2 x 0..64 placed by a `script_brushmodel` at (200, 0, 0), so
    /// its solid face is at x = 192.
    fn brushmodel_world() -> World {
        World {
            collision: vcod_common::collision::submodel_test_world(
                "{\n\"classname\" \"script_brushmodel\"\n\"model\" \"*1\"\n\"origin\" \"200 0 0\"\n}",
                &[([-8.0, -8.0, 0.0], [8.0, 8.0, 64.0])],
            ),
            vis: vcod_common::bsp::Visibility::none(),
            spawn: ([0.0, 0.0, 64.0], 0.0),
        }
    }

    /// A point sweep at door height that only the submodel can stop.
    fn brushmodel_clips(world: &World) -> bool {
        world
            .collision
            .box_trace(
                Vec3::new(150.0, 0.0, 32.0),
                Vec3::new(250.0, 0.0, 32.0),
                Vec3::ZERO,
                Vec3::ZERO,
            )
            .fraction
            < 1.0
    }

    /// `notSolid()` has to reach the clip, not just the flag: a submodel's
    /// brushes are in the world only through its entity, so this is the
    /// unlink `_load.gsc` performs on every `exploder` brush model and
    /// `_utility.gsc`'s `brush_show` undoes.
    #[test]
    fn notsolid_takes_a_brush_models_brushes_out_of_the_clip() {
        let (mut vm, mut host) = fixture();
        host.world = Some(Rc::new(brushmodel_world()));
        let world = host.world.clone().unwrap();
        vm.with_cx(|cx| {
            let e = host.ents.spawn(cx).unwrap();
            let model = cx.intern_folded("model");
            let star = Value::String(cx.intern_exact("*1"));
            host.set_field(cx, e, model, star).unwrap();
            let t = Some(Target::Entity(e));

            assert!(brushmodel_clips(&world), "linked at load");

            not_solid(&mut host, cx, t, &[]).unwrap();
            assert!(!host.ents.get(e).unwrap().solid);
            assert!(!brushmodel_clips(&world), "notSolid must unlink");

            solid(&mut host, cx, t, &[]).unwrap();
            assert!(host.ents.get(e).unwrap().solid);
            assert!(brushmodel_clips(&world), "solid must relink");
        });
    }

    /// An entity whose `model` is an xmodel name owns no brushes, so
    /// `notSolid()` on one must not unlink submodel 1 -- or any script model
    /// standing near a door would take the door out of the clip.
    #[test]
    fn notsolid_on_an_xmodel_entity_leaves_the_submodels_alone() {
        let (mut vm, mut host) = fixture();
        host.world = Some(Rc::new(brushmodel_world()));
        let world = host.world.clone().unwrap();
        vm.with_cx(|cx| {
            let e = host.ents.spawn(cx).unwrap();
            let model = cx.intern_folded("model");
            let name = Value::String(cx.intern_exact("xmodel/fx"));
            host.set_field(cx, e, model, name).unwrap();
            not_solid(&mut host, cx, Some(Target::Entity(e)), &[]).unwrap();
            assert!(brushmodel_clips(&world));
        });
    }

    /// No collision loaded is every unit test and any host built without a
    /// map, and it must leave the entity where the entity lump put it rather
    /// than dropping it 262144 units into nothing.
    #[test]
    fn placespawnpoint_without_a_world_leaves_the_origin_alone() {
        let (mut vm, mut host) = fixture();
        vm.with_cx(|cx| {
            let e = host.ents.spawn(cx).unwrap();
            let origin = cx.intern_folded("origin");
            let at = Value::Vector([1.0, 2.0, 3.0]);
            host.set_field(cx, e, origin, at).unwrap();
            place_spawnpoint(&mut host, cx, Some(Target::Entity(e)), &[]).unwrap();
            assert_eq!(host.get_field(cx, e, origin), at);
        });
    }

    /// `getEntArray(value, key)` walks slots in ascending entity number and
    /// keeps the ones whose field equals the value: `Scr_GetEntArray`
    /// 0x61980, which loops 0..level.num_entities filtering on `inuse`.
    #[test]
    fn get_ent_array_returns_ascending_entity_number() {
        let (mut vm, mut host) = fixture();
        vm.with_cx(|cx| {
            for name in ["b", "a", "c"] {
                let id = host.ents.spawn(cx).unwrap();
                let f = cx.intern_folded("classname");
                let v = cx.intern_exact("script_origin");
                host.set_field(cx, id, f, Value::String(v)).unwrap();
                let t = cx.intern_folded("targetname");
                let n = cx.intern_exact(name);
                host.set_field(cx, id, t, Value::String(n)).unwrap();
            }
            let cls = Value::String(cx.intern_exact("script_origin"));
            let key = Value::String(cx.intern_exact("classname"));
            let Value::Array(arr) = get_ent_array(&mut host, cx, None, &[cls, key]).unwrap() else {
                panic!("not an array");
            };
            assert_eq!(cx.array_len(arr), 3);
            let t = cx.intern_folded("targetname");
            let mut got = Vec::new();
            for i in 0..3 {
                let Value::Entity(e) = cx.get_index(arr, ArrayKey::Int(i)) else {
                    panic!()
                };
                let field = host.get_field(cx, e, t);
                let name = match field {
                    Value::String(a) => cx.resolve(a).to_string(),
                    v => panic!("{v:?}"),
                };
                got.push(name);
            }
            // Spawn order, not alphabetical: entity number decides.
            assert_eq!(got, ["b", "a", "c"]);
        });
    }

    /// `getEntArray()` with no arguments is every entity in use, in the same
    /// ascending order, no filter: `_gameobjects::main` opens with it.
    #[test]
    fn get_ent_array_with_no_arguments_is_every_entity() {
        let (mut vm, mut host) = fixture();
        vm.with_cx(|cx| {
            let want: Vec<EntId> = (0..3).map(|_| host.ents.spawn(cx).unwrap()).collect();
            let Value::Array(arr) = get_ent_array(&mut host, cx, None, &[]).unwrap() else {
                panic!("not an array");
            };
            assert_eq!(cx.array_len(arr), want.len());
            let got: Vec<Value> = (0..want.len() as i32)
                .map(|i| cx.get_index(arr, ArrayKey::Int(i)))
                .collect();
            let want: Vec<Value> = want.into_iter().map(Value::Entity).collect();
            assert_eq!(got, want);
        });
    }

    /// The key is resolved the same way a plain `.name` read is, so a radiant
    /// key works as a key. The corpus passes six: targetname, classname,
    /// script_noteworthy, target, export and team.
    #[test]
    fn get_ent_array_keys_on_script_defined_fields_too() {
        let (mut vm, mut host) = fixture();
        vm.with_cx(|cx| {
            let a = host.ents.spawn(cx).unwrap();
            let b = host.ents.spawn(cx).unwrap();
            let k = cx.intern_folded("script_noteworthy");
            let v = cx.intern_exact("loud");
            host.set_field(cx, a, k, Value::String(v)).unwrap();
            let _ = b;
            let want = Value::String(cx.intern_exact("loud"));
            let key = Value::String(cx.intern_exact("script_noteworthy"));
            let Value::Array(arr) = get_ent_array(&mut host, cx, None, &[want, key]).unwrap()
            else {
                panic!()
            };
            assert_eq!(cx.array_len(arr), 1);
        });
    }

    /// The key argument is a string value, so it is matched exactly, not
    /// folded; the field name it names is then folded like any field name.
    #[test]
    fn get_ent_array_returns_an_empty_array_when_nothing_matches() {
        let (mut vm, mut host) = fixture();
        vm.with_cx(|cx| {
            let want = Value::String(cx.intern_exact("nothing"));
            let key = Value::String(cx.intern_exact("classname"));
            let Value::Array(arr) = get_ent_array(&mut host, cx, None, &[want, key]).unwrap()
            else {
                panic!()
            };
            assert_eq!(cx.array_len(arr), 0);
        });
    }

    /// `getEnt` is the single-result form and yields `undefined` on a miss,
    /// which is what every `isDefined(getEnt(...))` in the corpus tests.
    #[test]
    fn get_ent_yields_undefined_on_a_miss() {
        let (mut vm, mut host) = fixture();
        vm.with_cx(|cx| {
            let want = Value::String(cx.intern_exact("nope"));
            let key = Value::String(cx.intern_exact("targetname"));
            assert_eq!(
                get_ent(&mut host, cx, None, &[want, key]).unwrap(),
                Value::Undefined
            );
        });
    }

    /// `spawn(classname, origin)` makes a live entity with both fields set,
    /// numbered after everything already in the table.
    #[test]
    fn spawn_sets_classname_and_origin() {
        let (mut vm, mut host) = fixture();
        vm.with_cx(|cx| {
            let cls = Value::String(cx.intern_exact("script_origin"));
            let at = Value::Vector([64.0, 0.0, 0.0]);
            let Value::Entity(e) = spawn(&mut host, cx, None, &[cls, at]).unwrap() else {
                panic!()
            };
            assert_eq!(e, EntId(72, 0));
            let o = cx.intern_folded("origin");
            assert_eq!(host.get_field(cx, e, o), Value::Vector([64.0, 0.0, 0.0]));
        });
    }

    /// `delete()` defers the free: the entity stays in iteration until its
    /// think comes due, `DELETE_DEFER_MS` past the call, and only then does
    /// a later `getEntArray` stop seeing it. `_load.gsc`'s exploder threads
    /// end with one.
    #[test]
    fn delete_defers_the_free_until_its_think_is_due() {
        let (mut vm, mut host) = fixture();
        vm.with_cx(|cx| {
            let e = host.ents.spawn(cx).unwrap();
            delete(&mut host, cx, Some(Target::Entity(e)), &[]).unwrap();
            assert_eq!(
                host.ents.iter_inuse().count(),
                1,
                "delete() must not free immediately"
            );
            host.run_entity_thinks(host.level_time_ms + DELETE_DEFER_MS);
            assert_eq!(host.ents.iter_inuse().count(), 0);
        });
    }

    /// `hide`/`show` and `solid`/`notSolid` flip real flags on the entity, not
    /// script-struct keys: `_load.gsc` hides every exploder model at load and
    /// stage 5 reads these when it builds entity states.
    #[test]
    fn hide_and_notsolid_flip_entity_flags() {
        let (mut vm, mut host) = fixture();
        vm.with_cx(|cx| {
            let e = host.ents.spawn(cx).unwrap();
            let t = Some(Target::Entity(e));
            assert!(!host.ents.get(e).unwrap().hidden);
            hide(&mut host, cx, t, &[]).unwrap();
            assert!(host.ents.get(e).unwrap().hidden);
            show(&mut host, cx, t, &[]).unwrap();
            assert!(!host.ents.get(e).unwrap().hidden);
            not_solid(&mut host, cx, t, &[]).unwrap();
            assert!(!host.ents.get(e).unwrap().solid);
        });
    }

    /// `isPlayer` reads the entity number against `MAX_CLIENTS`, which is
    /// what makes it answerable before stage 4 gives clients any state.
    #[test]
    fn is_player_reads_the_entity_number() {
        let (mut vm, mut host) = fixture();
        vm.with_cx(|cx| {
            let map_ent = Value::Entity(host.ents.spawn(cx).unwrap());
            assert_eq!(
                is_player(&mut host, cx, None, &[map_ent]).unwrap(),
                Value::Int(0)
            );
            let client = Value::Entity(EntId(3, 0));
            assert_eq!(
                is_player(&mut host, cx, None, &[client]).unwrap(),
                Value::Int(1)
            );
        });
    }

    /// `isAlive` reads a client's health off the host's vitals array, a
    /// non-entity argument answers 0, and any other entity reads its own
    /// `health` field.
    #[test]
    fn is_alive_reads_health_and_refuses_nothing() {
        let (mut vm, mut host) = fixture();
        vm.with_cx(|cx| {
            let player = host.ents.spawn_client(cx, 0, None).unwrap();
            host.client_vitals[0].health = 100;
            assert_eq!(
                is_alive(&mut host, cx, None, &[Value::Entity(player)]).unwrap(),
                Value::Int(1)
            );
            host.client_vitals[0].health = 0;
            assert_eq!(
                is_alive(&mut host, cx, None, &[Value::Entity(player)]).unwrap(),
                Value::Int(0)
            );
            assert_eq!(
                is_alive(&mut host, cx, None, &[Value::Undefined]).unwrap(),
                Value::Int(0)
            );
            let prop = host.ents.spawn(cx).unwrap();
            let h = cx.intern_folded("health");
            host.set_field(cx, prop, h, Value::Int(50)).unwrap();
            assert_eq!(
                is_alive(&mut host, cx, None, &[Value::Entity(prop)]).unwrap(),
                Value::Int(1)
            );
        });
    }

    /// A handle to a freed slot reads undefined: `sd.gsc:1850` (the plant) and
    /// `2019` (the defuse) test `isDefined(other.progressbackground)` on the
    /// handle an abort `destroy()`ed, and make a fresh element only on false.
    /// The same for a `delete()`d entity once its deferred free has run.
    #[test]
    fn is_defined_reads_zero_on_a_freed_hud_elem_and_a_freed_entity() {
        let (mut vm, mut host) = fixture();
        vm.with_cx(|cx| {
            let hud = super::super::hud::new_hud_elem(&mut host, cx, None, &[]).unwrap();
            assert_eq!(
                is_defined(&mut host, cx, None, &[hud]).unwrap(),
                Value::Int(1)
            );
            let Value::Entity(hud_id) = hud else { panic!() };
            super::super::hud::destroy(&mut host, cx, Some(Target::Entity(hud_id)), &[]).unwrap();
            assert_eq!(
                is_defined(&mut host, cx, None, &[hud]).unwrap(),
                Value::Int(0)
            );

            let e = host.ents.spawn(cx).unwrap();
            let ent = Value::Entity(e);
            delete(&mut host, cx, Some(Target::Entity(e)), &[]).unwrap();
            assert_eq!(
                is_defined(&mut host, cx, None, &[ent]).unwrap(),
                Value::Int(1),
                "delete() defers the free, so the handle still reads defined"
            );
            host.run_entity_thinks(host.level_time_ms + DELETE_DEFER_MS);
            assert_eq!(
                is_defined(&mut host, cx, None, &[ent]).unwrap(),
                Value::Int(0)
            );
        });
    }

    /// `getEntityNumber` on a HUD element is a type error: HUD elements have
    /// their own field table and are not gentities
    /// (docs/research/cod11-gsc-object-model.md section 3).
    #[test]
    fn get_entity_number_refuses_a_hud_element() {
        let (mut vm, mut host) = fixture();
        vm.with_cx(|cx| {
            let hud = Some(Target::Entity(EntId(FIRST_HUD_ELEM, 0)));
            assert!(get_entity_number(&mut host, cx, hud, &[]).is_err());
        });
    }

    /// `setModel` allocates a model configstring slot and stores the name,
    /// so `.model` reads back what was set.
    #[test]
    fn set_model_allocates_a_configstring_and_stores_the_name() {
        let (mut vm, mut host) = fixture();
        vm.with_cx(|cx| {
            let e = host.ents.spawn(cx).unwrap();
            let name = Value::String(cx.intern_exact("xmodel/fx"));
            set_model(&mut host, cx, Some(Target::Entity(e)), &[name]).unwrap();
            assert_eq!(host.configstrings[269], "xmodel/fx");
            let m = cx.intern_folded("model");
            match host.get_field(cx, e, m) {
                Value::String(a) => assert_eq!(cx.resolve(a), "xmodel/fx"),
                v => panic!("{v:?}"),
            }
        });
    }

    /// `istouching` is a box overlap, not a distance: a player standing in a
    /// bombzone 200 units wide is touching it well past the old 32-unit
    /// origin comparison's reach, and one clear of the zone's edge plus the
    /// player's own half-width is not. The boundary is the sum of the two
    /// boxes' half-extents (100 + 15 = 115), not the zone's edge alone.
    #[test]
    fn is_touching_overlaps_boxes() {
        let (mut vm, mut host) = crate::game::testing_world_fixture();
        vm.with_cx(|cx| {
            let zone = host.ents.spawn(cx).unwrap();
            let origin = cx.intern_folded("origin");
            host.set_field(cx, zone, origin, Value::Vector([0.0, 0.0, 0.0]))
                .unwrap();
            host.triggers.register(
                zone,
                crate::game::trigger::TriggerKind::Multiple,
                crate::game::trigger::TriggerShape::boxed(
                    [-100.0, -100.0, 0.0],
                    [100.0, 100.0, 64.0],
                ),
                0,
                0,
            );

            let player = host.ents.spawn_client(cx, 0, None).unwrap();
            let inside = Some(Target::Entity(player));
            host.set_field(cx, player, origin, Value::Vector([90.0, 0.0, 0.0]))
                .unwrap();
            assert_eq!(
                is_touching(&mut host, cx, inside, &[Value::Entity(zone)]),
                Ok(Value::Int(1)),
                "90 is inside a box that reaches 100, well past the old 32-unit test"
            );

            host.set_field(cx, player, origin, Value::Vector([116.0, 0.0, 0.0]))
                .unwrap();
            assert_eq!(
                is_touching(&mut host, cx, inside, &[Value::Entity(zone)]),
                Ok(Value::Int(0)),
                "116 clears the zone's 100 reach plus the player's own 15"
            );
        });
    }

    /// `linkTo` takes the offset the client already stands at and `unlink`
    /// releases it; both are edges the sim applies, and only a client links
    /// in stock MP (object-model doc, 23.2).
    #[test]
    fn link_to_queues_the_offset_and_unlink_queues_the_release() {
        let (mut vm, mut host) = fixture();
        vm.with_cx(|cx| {
            let player = host.ents.spawn_client(cx, 0, None).unwrap();
            let o = cx.intern_folded("origin");
            host.set_field(cx, player, o, Value::Vector([10.0, 0.0, 0.0]))
                .unwrap();
            let zone = host.ents.spawn(cx).unwrap();
            host.set_field(cx, zone, o, Value::Vector([0.0, 0.0, 0.0]))
                .unwrap();
            link_to(
                &mut host,
                cx,
                Some(Target::Entity(player)),
                &[Value::Entity(zone)],
            )
            .unwrap();
            assert_eq!(
                host.client_link_ops,
                vec![(
                    0,
                    LinkOp::Link {
                        parent: zone,
                        offset: [10.0, 0.0, 0.0]
                    }
                )]
            );
            unlink(&mut host, cx, Some(Target::Entity(player)), &[]).unwrap();
            assert_eq!(host.client_link_ops[1], (0, LinkOp::Unlink));
            assert!(
                link_to(
                    &mut host,
                    cx,
                    Some(Target::Entity(player)),
                    &[Value::Undefined]
                )
                .is_err(),
                "linkTo needs an entity to link to"
            );
            // Only a client links: retail gates on the receiver's svFlags
            // 0x20, which `ClientSpawn` is what sets in stock MP (23.2).
            assert!(link_to(
                &mut host,
                cx,
                Some(Target::Entity(zone)),
                &[Value::Entity(zone)]
            )
            .is_err());
            // `unlink()` on an unlinked player is a no-op, not an error.
            assert!(unlink(&mut host, cx, Some(Target::Entity(player)), &[]).is_ok());
        });
    }
}
