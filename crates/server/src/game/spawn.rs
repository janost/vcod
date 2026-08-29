//! The map load: BSP entity lump to object table, following `G_CallSpawn`
//! and `G_ParseField` (docs/research/cod11-gsc-object-model.md section 7).

use crate::game::fields::{self, FieldType, Route};
use crate::game::host::GameHost;
use vcod_gsc::{Cx, EntId, ErrorKind, Host, Value};

/// The `spawns` table at 0x7eb30, dumped by `tools/re/dump_builtins.py`.
/// A classname here gets an engine spawn function in retail; one that is not
/// takes `G_CallSpawn`'s fourth case and becomes a live, script-visible
/// entity, which is what keeps the gametype spawn markers alive.
///
/// Nothing consults this yet: `spawn_entities_from_string` takes the fourth
/// case for every classname. That is a measured divergence, not a harmless
/// one, because an `SP_` function may free the entity it was handed and the
/// numbering then shifts. See docs/research/cod11-gsc-object-model.md
/// section 13.
pub const SPAWN_CLASSNAMES: &[&str] = &[
    "info_null",
    "info_notnull",
    "func_door",
    "func_static",
    "func_rotating",
    "func_bobbing",
    "func_pendulum",
    "func_group",
    "func_door_rotating",
    "trigger_multiple",
    "trigger_hurt",
    "trigger_once",
    "target_location",
    "mp_target_location",
    "light",
    "misc_teleporter_dest",
    "misc_model",
    "misc_mg42",
    "misc_turret",
    "misc_spawner",
    "corona",
    "trigger_use",
    "trigger_damage",
    "trigger_lookat",
    "script_brushmodel",
    "script_model",
    "script_origin",
];

/// One object per entity block with a classname, in lump order. Retail
/// additionally runs the block's `SP_` function when its classname is in
/// `SPAWN_CLASSNAMES`, and two of those functions (`SP_misc_model`,
/// `SP_light`) free the entity again, so retail ends up with fewer entities
/// and different numbers than this does. Measured on mp_pavlov;
/// docs/research/cod11-gsc-object-model.md section 13 has the numbers, and
/// `crates/server/tests/semantics_ents.rs` fails on them today.
pub fn spawn_entities_from_string(
    host: &mut GameHost,
    cx: &mut Cx,
    entities: &str,
) -> Result<(), ErrorKind> {
    let mut blocks = vcod_common::bsp::entity_blocks(entities).into_iter();
    let world = blocks
        .next()
        .ok_or(ErrorKind::BadType("empty entity lump"))?;
    if world.get("classname").map(String::as_str) != Some("worldspawn") {
        return Err(ErrorKind::BadType("first entity block is not worldspawn"));
    }
    for block in blocks {
        if !block.contains_key("classname") {
            // `G_CallSpawn` warns and creates nothing.
            log::warn!("gsc: entity block with no classname, dropped");
            continue;
        }
        let id = host.ents.spawn(cx)?;
        for (k, v) in &block {
            parse_field(host, cx, id, k, v);
        }
    }
    Ok(())
}

/// `G_ParseField`: the entity field table first, case-insensitively, then
/// the radiant key set, then drop. The three cases are what decides which
/// BSP keys script can see at all.
fn parse_field(host: &mut GameHost, cx: &mut Cx, id: EntId, key: &str, raw: &str) {
    let folded = key.to_ascii_lowercase();
    let ty = match fields::route_entity(&folded) {
        Route::Engine { ty, .. } => ty,
        Route::Client(_) => return,
        Route::Script => match fields::radiant_key(&folded) {
            Some(ty) => ty,
            None => return,
        },
    };
    let Some(value) = convert(cx, ty, raw) else {
        return;
    };
    let atom = cx.intern_folded(&folded);
    let _ = host.set_field(cx, id, atom, value);
}

/// The string-to-value conversions `G_ParseField` performs, one per field
/// type. `sscanf("%f %f %f")` for a vector, `strtol` for an int, `strtod`
/// for a float, an interned string otherwise.
fn convert(cx: &mut Cx, ty: FieldType, raw: &str) -> Option<Value> {
    Some(match ty {
        FieldType::Int => Value::Int(raw.trim().parse().ok()?),
        FieldType::Float => Value::Float(raw.trim().parse().ok()?),
        FieldType::Vector | FieldType::YawVector => {
            let mut it = raw.split_whitespace().map(str::parse::<f32>);
            let x = it.next()?.ok()?;
            let y = it.next()?.ok()?;
            let z = it.next()?.ok()?;
            Value::Vector([x, y, z])
        }
        // A model name is stored as a model index in retail and read back
        // through `G_ModelName`; here the string is the storage and the
        // index is allocated on demand by `setModel`.
        FieldType::CString | FieldType::IString | FieldType::ModelIndex => {
            Value::String(cx.intern_exact(raw))
        }
        FieldType::Entity | FieldType::Object => return None,
    })
}

#[cfg(test)]
mod tests {
    use crate::game::testing::fixture;
    use vcod_gsc::{EntId, Host, Value};

    /// `G_CallSpawn`'s fourth case: a classname in neither the item list nor
    /// `spawns` still produces a live entity with its keys applied. That is why
    /// `mp_deathmatch_spawn` survives for `_spawnlogic.gsc` to find
    /// (docs/research/cod11-gsc-object-model.md section 7).
    #[test]
    fn an_unknown_classname_still_spawns_a_live_entity() {
        let (mut vm, mut host) = fixture();
        vm.with_cx(|cx| {
            super::spawn_entities_from_string(
                &mut host,
                cx,
                "{\n\"classname\" \"worldspawn\"\n}\n\
                 {\n\"classname\" \"mp_deathmatch_spawn\"\n\"origin\" \"1 2 3\"\n}\n",
            )
            .unwrap();
            let (id, _) = host.ents.iter_inuse().next().unwrap();
            assert_eq!(id, EntId(72));
            let cn = cx.intern_folded("classname");
            let cn_val = host.get_field(cx, id, cn);
            let cn_atom = match cn_val {
                Value::String(a) => a,
                v => panic!("classname is {v:?}"),
            };
            assert_eq!(cx.resolve(cn_atom), "mp_deathmatch_spawn");
            let o = cx.intern_folded("origin");
            assert_eq!(host.get_field(cx, id, o), Value::Vector([1.0, 2.0, 3.0]));
        });
    }

    /// A key that is neither an engine field nor a radiant key is dropped at
    /// load and script never sees it, which is `G_ParseField`'s third case.
    #[test]
    fn an_unknown_key_is_dropped_not_stashed() {
        let (mut vm, mut host) = fixture();
        vm.with_cx(|cx| {
            super::spawn_entities_from_string(
                &mut host,
                cx,
                "{\n\"classname\" \"worldspawn\"\n}\n\
                 {\n\"classname\" \"script_model\"\n\"_lightmapsamplesize\" \"8\"\n\
                 \"script_exploder\" \"4\"\n}\n",
            )
            .unwrap();
            let (id, _) = host.ents.iter_inuse().next().unwrap();
            let junk = cx.intern_folded("_lightmapsamplesize");
            assert_eq!(host.get_field(cx, id, junk), Value::Undefined);
            let sx = cx.intern_folded("script_exploder");
            assert_eq!(host.get_field(cx, id, sx), Value::Int(4));
        });
    }

    /// A radiant key carries its declared type through the conversion, so
    /// `script_exploder` reads as an int and `script_noteworthy` as a string.
    #[test]
    fn radiant_keys_convert_by_their_declared_type() {
        let (mut vm, mut host) = fixture();
        vm.with_cx(|cx| {
            super::spawn_entities_from_string(
                &mut host,
                cx,
                "{\n\"classname\" \"worldspawn\"\n}\n\
                 {\n\"classname\" \"script_model\"\n\"script_exploder\" \"7\"\n\
                 \"script_noteworthy\" \"loud\"\n\"script_delay\" \"1.5\"\n}\n",
            )
            .unwrap();
            let (id, _) = host.ents.iter_inuse().next().unwrap();
            let ex = cx.intern_folded("script_exploder");
            let delay = cx.intern_folded("script_delay");
            let note = cx.intern_folded("script_noteworthy");
            assert_eq!(host.get_field(cx, id, ex), Value::Int(7));
            assert_eq!(host.get_field(cx, id, delay), Value::Float(1.5));
            match host.get_field(cx, id, note) {
                Value::String(a) => assert_eq!(cx.resolve(a), "loud"),
                v => panic!("script_noteworthy is {v:?}"),
            }
        });
    }

    /// The first block must be worldspawn and does not consume an entity
    /// number; the map's entities start at 72 in lump order.
    #[test]
    fn worldspawn_is_first_and_consumes_no_number() {
        let (mut vm, mut host) = fixture();
        vm.with_cx(|cx| {
            super::spawn_entities_from_string(
                &mut host,
                cx,
                "{\n\"classname\" \"worldspawn\"\n}\n\
                 {\n\"classname\" \"script_origin\"\n\"targetname\" \"a\"\n}\n\
                 {\n\"classname\" \"script_origin\"\n\"targetname\" \"b\"\n}\n",
            )
            .unwrap();
            let ids: Vec<_> = host.ents.iter_inuse().map(|(i, _)| i).collect();
            assert_eq!(ids, vec![EntId(72), EntId(73)]);
        });
    }

    /// A first block that is not worldspawn is a hard error, matching
    /// `G_SpawnEntitiesFromString`'s `G_Error`.
    #[test]
    fn a_missing_worldspawn_is_an_error() {
        let (mut vm, mut host) = fixture();
        vm.with_cx(|cx| {
            assert!(super::spawn_entities_from_string(
                &mut host,
                cx,
                "{\n\"classname\" \"script_origin\"\n}\n",
            )
            .is_err());
        });
    }

    /// The dumped classname table, so a transcription slip is caught.
    #[test]
    fn the_spawn_classname_table_is_the_dumped_one() {
        assert_eq!(super::SPAWN_CLASSNAMES.len(), 27);
        assert!(super::SPAWN_CLASSNAMES.contains(&"script_origin"));
        assert!(super::SPAWN_CLASSNAMES.contains(&"trigger_multiple"));
        assert!(!super::SPAWN_CLASSNAMES.contains(&"mp_deathmatch_spawn"));
        assert!(!super::SPAWN_CLASSNAMES.contains(&"worldspawn"));
    }

    /// mp_pavlov's lump has 345 blocks, four of them `script_origin` at lump
    /// indices 2, 3, 4 and 344 named auto5, auto4, auto3 and auto6. That order
    /// is what retail's `getEntArray` returned, and it is the whole basis of
    /// `probe_ents` (docs/research/cod11-gsc-object-model.md section 10).
    #[test]
    fn mp_pavlov_loads_its_entities_in_lump_order() {
        let Some(fs) = vcod_common::testing::game_fs() else {
            return;
        };
        let bytes = fs.read("maps/mp/mp_pavlov.bsp").expect("mp_pavlov.bsp");
        let bsp = vcod_common::bsp::parse(&bytes).unwrap();
        let (mut vm, mut host) = fixture();
        vm.with_cx(|cx| {
            super::spawn_entities_from_string(&mut host, cx, &bsp.entities).unwrap();
            let cn = cx.intern_folded("classname");
            let tn = cx.intern_folded("targetname");
            let ids: Vec<EntId> = host.ents.iter_inuse().map(|(id, _)| id).collect();
            let mut names = Vec::new();
            for id in ids {
                let is_origin = matches!(host.get_field(cx, id, cn), Value::String(a)
                    if cx.resolve(a) == "script_origin");
                if !is_origin {
                    continue;
                }
                names.push(match host.get_field(cx, id, tn) {
                    Value::String(a) => cx.resolve(a).to_string(),
                    v => format!("{v:?}"),
                });
            }
            assert_eq!(names, ["auto5", "auto4", "auto3", "auto6"]);
        });
    }
}
