//! Entity builtins the map-load and spawn paths reach: lookup, spawn (both
//! the map-entity form and `ClientSpawn`), delete, visibility/solidity and
//! setModel. Per-family dispatch: `NAMES`/`lookup` is matched against
//! `Cx::resolve_folded`, same shape `host.rs` uses for the env/io names, and
//! Task 9 adds more families beside this one.

use crate::configstrings::CsRange;
use crate::game::entity::{ThinkFn, FIRST_HUD_ELEM};
use crate::game::host::{GameHost, SpawnRequest};
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
    ("isdefined", is_defined),
    ("istouching", is_touching),
    ("placespawnpoint", place_spawnpoint),
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
    let player = match host.get_field(cx, id, state) {
        Value::String(s) => cx.resolve(s) == "playing",
        // `spawnIntermission` and a dead player have their own retail
        // pm_types; neither is simulated, so both fly as spectators.
        _ => false,
    };

    let origin_field = cx.intern_folded("origin");
    host.set_field(cx, id, origin_field, Value::Vector(origin))?;
    let angles_field = cx.intern_folded("angles");
    host.set_field(cx, id, angles_field, Value::Vector(angles))?;

    host.client_weapons[slot] = crate::weapons::PlayerWeapons::default();
    host.client_spawns.push(SpawnRequest {
        slot,
        origin,
        yaw_deg: angles[1],
        player,
    });
    Ok(Value::Undefined)
}

/// `spawn(classname, origin)`: a live entity with both fields set, numbered
/// after everything already in the table.
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
/// pass). `_load.gsc`'s exploder threads end with one.
pub fn delete(
    host: &mut GameHost,
    _cx: &mut Cx,
    recv: Option<Target>,
    _args: &[Value],
) -> Result<Value, ErrorKind> {
    let id = entity_receiver(recv)?;
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

fn set_solid(host: &mut GameHost, recv: Option<Target>, solid: bool) -> Result<Value, ErrorKind> {
    let id = entity_receiver(recv)?;
    let e = host
        .ents
        .get_mut(id)
        .ok_or(ErrorKind::BadType("no such entity"))?;
    e.solid = solid;
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
    _cx: &mut Cx,
    recv: Option<Target>,
    _args: &[Value],
) -> Result<Value, ErrorKind> {
    set_solid(host, recv, true)
}

pub fn not_solid(
    host: &mut GameHost,
    _cx: &mut Cx,
    recv: Option<Target>,
    _args: &[Value],
) -> Result<Value, ErrorKind> {
    set_solid(host, recv, false)
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
    let Some(Value::Entity(id)) = args.first() else {
        return Err(ErrorKind::BadType("isPlayer takes an entity"));
    };
    Ok(Value::Int((id.0 < MAX_CLIENTS as u32) as i32))
}

/// `isDefined(x)`: false only for a missing argument or `undefined` itself.
pub fn is_defined(
    _host: &mut GameHost,
    _cx: &mut Cx,
    _recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    Ok(Value::Int(
        !matches!(args.first(), None | Some(Value::Undefined)) as i32,
    ))
}

/// `isTouching(other)`. Entities gain real bounds in stage 5; until then
/// this compares origins within a small box rather than pretending to be a
/// real intersection test. `BOX` is invented, not measured: nothing has been
/// read out of retail about what its `isTouching` compares, so the number is
/// only a stand-in until entities carry bounds and the real test replaces it.
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
    let origin = cx.intern_folded("origin");
    let oa = host.get_field(cx, a, origin);
    let ob = host.get_field(cx, b, origin);
    let (Value::Vector(oa), Value::Vector(ob)) = (oa, ob) else {
        return Ok(Value::Int(0));
    };
    const BOX: f32 = 32.0;
    let touching = (0..3).all(|i| (oa[i] - ob[i]).abs() <= BOX);
    Ok(Value::Int(touching as i32))
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
            let e = host.ents.spawn_client(cx, 2).unwrap();
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
                (s.slot, s.origin, s.yaw_deg, s.player),
                (2, [10.0, 20.0, 30.0], 90.0, true)
            );
            assert_eq!(
                host.client_weapons[2],
                crate::weapons::PlayerWeapons::default()
            );
            let origin = cx.intern_folded("origin");
            assert_eq!(host.get_field(cx, e, origin), at);

            // A spectator's spawn goes through the same builtin.
            let spectator = Value::String(cx.intern_exact("spectator"));
            host.set_field(cx, e, state, spectator).unwrap();
            spawn(&mut host, cx, t, &[at, angles]).unwrap();
            assert!(!host.client_spawns[1].player);

            // No receiver is still the free function.
            let cls = Value::String(cx.intern_exact("script_model"));
            let Value::Entity(made) = spawn(&mut host, cx, None, &[cls, at]).unwrap() else {
                panic!("the free form returns the entity it made");
            };
            assert_eq!(made, EntId(crate::game::entity::FIRST_MAP_ENTITY));
            assert_eq!(host.client_spawns.len(), 2);
        });
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
            assert_eq!(e, EntId(72));
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
            host.ents.run_thinks(host.level_time_ms + DELETE_DEFER_MS);
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
            let client = Value::Entity(EntId(3));
            assert_eq!(
                is_player(&mut host, cx, None, &[client]).unwrap(),
                Value::Int(1)
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
            let hud = Some(Target::Entity(EntId(FIRST_HUD_ELEM)));
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
}
