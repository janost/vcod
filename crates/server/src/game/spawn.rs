//! The map load: BSP entity lump to object table, following `G_CallSpawn`
//! and `G_ParseField` (docs/research/cod11-gsc-object-model.md section 7).

use crate::configstrings::CsRange;
use crate::game::fields::{self, FieldType, Route};
use crate::game::host::GameHost;
use vcod_gsc::{Cx, EntId, ErrorKind, Host, Value};

/// The `spawns` table at 0x7eb30, dumped by `tools/re/dump_builtins.py`.
/// A classname here gets an engine spawn function in retail; one that is not
/// takes `G_CallSpawn`'s fourth case and becomes a live, script-visible
/// entity, which is what keeps the gametype spawn markers alive.
///
/// The only part of an `SP_` function stage 2 reproduces is whether it frees
/// the entity it was handed; see `SPAWN_FREES`.
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

/// The five `spawns` classnames whose `SP_` function is nothing but
/// `G_FreeEntity(self)`, so the block leaves no live entity behind. Four
/// distinct functions cover them: `SP_info_null` 0x531cc (`info_null` and
/// `func_group`), `SP_light` 0x53204, `SP_misc_model` 0x53224 and
/// `SP_corona` 0x53274, each a single unconditional call.
/// docs/research/cod11-gsc-object-model.md section 13 has the evidence and
/// the live per-classname counts.
pub const SPAWN_FREES: &[&str] = &["info_null", "func_group", "light", "misc_model", "corona"];

/// One object per entity block with a classname, in lump order, then
/// `G_CallSpawn`'s third case: a classname whose `SP_` function frees is
/// spawned, keyed and freed again, and `G_Spawn` hands the slot straight
/// back out. The net effect is that such a block consumes no entity number,
/// which is what makes vcod's numbers retail's numbers.
///
/// The classname match is `strcmp` in retail, so it is case-sensitive here
/// too, unlike the key names `parse_field` folds.
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
    host.configstrings[11] = worldspawn_northyaw(&world);
    for block in blocks {
        let Some(classname) = block.get("classname").cloned() else {
            // `G_CallSpawn` warns and creates nothing.
            log::warn!("gsc: entity block with no classname, dropped");
            continue;
        };
        let id = host.ents.spawn(cx)?;
        for (k, v) in &block {
            parse_field(host, cx, id, k, v);
        }
        if classname == "trigger_hurt" {
            register_sound_alias(host, trigger_hurt_sound(&block));
        }
        if let Some(item) = spawn_item_name(host, cx, id, &classname) {
            host.register_item(&item.name);
            if item.turret {
                for alias in turret_sound_aliases(host.fs.as_deref(), &item.name) {
                    register_sound_alias(host, alias);
                }
            }
        }
        if SPAWN_FREES.contains(&classname.as_str()) {
            host.ents.free(id);
        }
    }
    Ok(())
}

/// `SP_worldspawn` (0x61cec): `G_SpawnString("northyaw", "", &out)` then, if
/// the result is empty, the literal `"0"` in its place -- a raw copy of the
/// BSP text, never a rendered number, which is why this does not go through
/// `Cx::format_number`. `mp_carentan`'s own worldspawn block carries no
/// `northyaw` key at all (checked against the shipped BSP), so its capture's
/// `"0"` is that literal, not a formatted zero; `mp_pavlov`'s `"90"` is the
/// key's value verbatim. INFERRED FROM DECOMPILATION for the mechanism,
/// cross-checked against both captures for the outcome.
fn worldspawn_northyaw(world: &std::collections::HashMap<String, String>) -> String {
    match world.get("northyaw") {
        Some(v) if !v.is_empty() => v.clone(),
        _ => "0".to_string(),
    }
}

/// `SP_trigger_hurt` (0x64ef8): `G_SpawnString("sound", "world_hurt_me",
/// &out)`, unconditionally, before any of the entity's other fields are
/// touched (INFERRED FROM DECOMPILATION, reading control flow only). Both
/// gate maps place exactly one `trigger_hurt` with no `sound` key -- VERIFIED,
/// read straight from the shipped BSPs -- so the default is what lands in
/// configstring 525 on both.
///
/// Unlike `worldspawn_northyaw`, this only falls back when the key is
/// absent, not when it is present-but-empty: nothing in `SP_trigger_hurt`'s
/// disassembly showed the post-`G_SpawnString` empty check `SP_worldspawn`
/// has, and neither gate map's `trigger_hurt` carries an empty `sound`
/// value, so that branch is deliberate but unmeasured, not proven either
/// way.
fn trigger_hurt_sound(block: &std::collections::HashMap<String, String>) -> String {
    block
        .get("sound")
        .cloned()
        .unwrap_or_else(|| "world_hurt_me".to_string())
}

/// `G_SpawnTurret` (0x52c84), reached from `SP_turret` for `misc_mg42` and
/// `misc_turret`: right after `RegisterItem(weaponinfo)`, it reads two
/// string fields off the weapon info struct `BG_GetInfoForWeapon` returns
/// for that same weapon and calls `G_SoundAliasIndex` on each (INFERRED FROM
/// DECOMPILATION, reading control flow only). VERIFIED which two fields,
/// from the shipped `weapons/mp/mg42_bipod_stand_mp`: its `loopFireSound`
/// and `stopFireSound` keys are `weap_mg42_loop` and `weap_mg42_cooldown`,
/// exactly `mp_carentan`'s captured 526/527. So the alias names come from
/// the weapon file, keyed by the entity's own `weaponinfo` value -- the same
/// string `spawn_item_name` already resolved for `RegisterItem`, reused here
/// rather than re-read.
fn turret_sound_aliases(fs: Option<&vcod_common::pk3::Pk3Fs>, weaponinfo: &str) -> Vec<String> {
    let Some(fs) = fs else {
        return Vec::new();
    };
    let Some(bytes) = fs.read(&format!("weapons/mp/{weaponinfo}")) else {
        return Vec::new();
    };
    let map = vcod_common::xmodel::parse_weapon(&String::from_utf8_lossy(&bytes));
    ["loopFireSound", "stopFireSound"]
        .iter()
        .filter_map(|k| map.get(*k))
        .filter(|v| !v.is_empty())
        .cloned()
        .collect()
}

fn register_sound_alias(host: &mut GameHost, name: String) {
    if let Err(e) = host
        .allocators
        .index(&mut host.configstrings, CsRange::SoundAlias, &name)
    {
        log::warn!("gsc: sound alias {name:?} not indexed: {e:?}");
    }
}

/// A placed weapon's `bg_itemlist` classname is its weapon file's
/// `radiantName`, not its own file name -- and not always that name with an
/// `mpweapon_` prefix stripped, either. VERIFIED: read straight from every
/// `weapons/mp/*_mp` file in the shipped paks. 23 of `WEAPON_LIST`'s 32
/// weapons carry a non-empty `radiantName`; this table is all 23, so a
/// classname not in it is not a placeable weapon, not a transcription gap.
///
/// Three names are a compound losing its underscore
/// (`mosin_nagant_mp`/`mpweapon_mosinnagant`,
/// `mosin_nagant_sniper_mp`/`mpweapon_mosinnagantsniper`,
/// `kar98k_sniper_mp`/`mpweapon_kar98k_scoped`), and a fourth is not a
/// transform of the file's own name at all
/// (`rgd-33russianfrag_mp`/`mpweapon_russiangrenade`), which is why this is
/// a table and not a prefix-and-suffix rule.
///
/// The other nine `WEAPON_LIST` weapons carry no usable `radiantName` and
/// are deliberately absent: five alt-fire files (`bar_slow_mp`,
/// `fg42_semi_mp`, `mp44_semi_mp`, `ppsh_semi_mp`, `thompson_semi_mp`) carry
/// an empty `radiantName`, since they are never placed directly and reach
/// `Items::register` only through the alt-fire link (`alt_weapon_index`);
/// the three `mg42_bipod_*` files carry no `radiantName` key at all, since
/// a mounted mg42 is placed as `misc_mg42` with its item named by the
/// `weaponinfo` key instead (see `spawn_item_name`); `ptrs41_antitank_rifle_mp`
/// has no weapon file under that name in the stock paks.
const RADIANT_NAMES: &[(&str, &str)] = &[
    ("mpweapon_bar", "bar_mp"),
    ("mpweapon_bren", "bren_mp"),
    ("mpweapon_colt", "colt_mp"),
    ("mpweapon_enfield", "enfield_mp"),
    ("mpweapon_fg42", "fg42_mp"),
    ("mpweapon_fraggrenade", "fraggrenade_mp"),
    ("mpweapon_kar98k", "kar98k_mp"),
    ("mpweapon_kar98k_scoped", "kar98k_sniper_mp"),
    ("mpweapon_luger", "luger_mp"),
    ("mpweapon_m1carbine", "m1carbine_mp"),
    ("mpweapon_m1garand", "m1garand_mp"),
    ("mpweapon_mk1britishfrag", "mk1britishfrag_mp"),
    ("mpweapon_mosinnagant", "mosin_nagant_mp"),
    ("mpweapon_mosinnagantsniper", "mosin_nagant_sniper_mp"),
    ("mpweapon_mp40", "mp40_mp"),
    ("mpweapon_mp44", "mp44_mp"),
    ("mpweapon_panzerfaust", "panzerfaust_mp"),
    ("mpweapon_ppsh", "ppsh_mp"),
    ("mpweapon_russiangrenade", "rgd-33russianfrag_mp"),
    ("mpweapon_springfield", "springfield_mp"),
    ("mpweapon_sten", "sten_mp"),
    ("mpweapon_stielhandgranate", "stielhandgranate_mp"),
    ("mpweapon_thompson", "thompson_mp"),
];

/// The item a spawned entity registers, if any -- `G_CallSpawn`'s second
/// case, before `spawns` (section 17 of the object model doc): a classname
/// matched against `bg_itemlist` calls `G_SpawnItem` -> `RegisterItem`
/// straight after the entity is built, before any script runs, and neither
/// function frees the entity, so a later script `delete()` still finds it.
///
/// A placed weapon's classname is looked up in `RADIANT_NAMES`; both gate
/// maps place `mpweapon_fg42`/`mpweapon_panzerfaust` entities that
/// `_teams.gsc`'s `restrictPlacedWeapons` later deletes by that same
/// classname (VERIFIED, read from the shipped BSPs and script).
///
/// `misc_mg42`/`misc_turret` reach `RegisterItem` too, through their own
/// `SP_turret` (0x533b0, section 8) rather than `bg_itemlist`: the item
/// name is their `weaponinfo` key's value directly, no lookup needed
/// (VERIFIED: mp_carentan's two mounted mg42s carry
/// `"weaponinfo" "mg42_bipod_stand_mp"` in the shipped BSP).
///
/// One divergence, on section 17's reading of `SP_turret`'s branches: a
/// `misc_mg42`/`misc_turret` with no `weaponinfo` key takes retail's
/// `Com_Error` ("no weaponinfo specified for turret") and fails the map
/// load, where we return `None` and spawn the entity without an item. No
/// stock map has one, so the two differ only on a broken BSP, which ours
/// loads and retail does not.
fn spawn_item_name(
    host: &mut GameHost,
    cx: &mut Cx,
    id: EntId,
    classname: &str,
) -> Option<SpawnItem> {
    if classname.starts_with("mpweapon_") {
        return RADIANT_NAMES
            .iter()
            .find(|(radiant, _)| *radiant == classname)
            .map(|(_, weapon)| SpawnItem {
                name: weapon.to_string(),
                turret: false,
            });
    }
    if classname == "misc_mg42" || classname == "misc_turret" {
        let weaponinfo = cx.intern_folded("weaponinfo");
        if let Value::String(s) = host.get_field(cx, id, weaponinfo) {
            return Some(SpawnItem {
                name: cx.resolve(s).to_string(),
                turret: true,
            });
        }
    }
    None
}

/// What a spawned entity registers. `turret` carries which of the two
/// paths above matched, because only the `SP_turret` one also registers
/// sound aliases: deciding that at the call site would mean repeating the
/// classname test, and a third turret classname added to one copy and not
/// the other would silently lose the aliases.
struct SpawnItem {
    name: String,
    turret: bool,
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
    if ty == FieldType::ModelIndex && !raw.starts_with('*') {
        // `G_ParseField`'s type-8 branch (call at 0x61505) indexes the name
        // and stores the byte at `gentity_t+0x175`; we keep the name as the
        // storage, so only the slot allocation is reproduced here.
        if let Err(e) = host
            .allocators
            .index(&mut host.configstrings, CsRange::Model, raw)
        {
            log::warn!("gsc: model {raw:?} not indexed: {e:?}");
        }
    }
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
        // No entity field is an enum -- the three that are all live on HUD
        // elements, which no BSP entity block builds.
        FieldType::Entity | FieldType::Object | FieldType::Enum(_) => return None,
    })
}

#[cfg(test)]
mod tests {
    use crate::game::testing::fixture;
    use crate::items::Items;
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

    /// A placed weapon registers its item at spawn, which is why retail's cs 8
    /// differs between two maps whose scripts precache the same list. The
    /// registration happens before the script runs, and it survives the
    /// entity's deletion by `_teams::restrictPlacedWeapons`.
    #[test]
    fn a_placed_weapon_registers_its_item() {
        let (mut vm, mut host) = fixture();
        let lump = "{\n\"classname\" \"worldspawn\"\n}\n\
                    {\n\"classname\" \"mpweapon_fg42\"\n\"origin\" \"0 0 0\"\n}\n";
        vm.with_cx(|cx| super::spawn_entities_from_string(&mut host, cx, lump))
            .unwrap();
        assert_ne!(host.items.bitstring(), Items::new().bitstring());
    }

    /// Pins the exact bit: `fg42_mp` is configstring 7 index 6, and
    /// `RegisterItem`'s alt-fire link sets `fg42_semi_mp` (index 7) along
    /// with it, so nibble 1 (bits 4-7) comes out `0b1100` = `c`. A wrong
    /// item index would still flip `assert_ne`'s inequality but land on the
    /// wrong character here.
    #[test]
    fn a_placed_fg42_sets_its_own_bit_and_its_alt_fires() {
        let (mut vm, mut host) = fixture();
        let lump = "{\n\"classname\" \"worldspawn\"\n}\n\
                    {\n\"classname\" \"mpweapon_fg42\"\n\"origin\" \"0 0 0\"\n}\n";
        vm.with_cx(|cx| super::spawn_entities_from_string(&mut host, cx, lump))
            .unwrap();
        assert_eq!(host.items.bitstring().chars().nth(1), Some('c'));
    }

    /// `misc_mg42`/`misc_turret` reach `RegisterItem` through `SP_turret`
    /// instead of `bg_itemlist`: the item comes from the `weaponinfo` key,
    /// not the classname, matching mp_carentan's mounted mg42s.
    #[test]
    fn a_mounted_mg42_registers_the_item_named_by_its_weaponinfo_key() {
        let (mut vm, mut host) = fixture();
        let lump = "{\n\"classname\" \"worldspawn\"\n}\n\
                    {\n\"classname\" \"misc_mg42\"\n\"weaponinfo\" \"mg42_bipod_stand_mp\"\n}\n";
        vm.with_cx(|cx| super::spawn_entities_from_string(&mut host, cx, lump))
            .unwrap();
        // mg42_bipod_stand_mp is configstring 7 index 16, nibble 4 (bits
        // 16-19), bit 16 alone: 0b0001 = 1.
        assert_eq!(host.items.bitstring().chars().nth(4), Some('1'));
    }

    /// Four placed weapons whose `radiantName` a prefix-and-suffix rule
    /// cannot recover: three drop an underscore from their own file name,
    /// and `rgd-33russianfrag_mp`'s `radiantName` is not any spelling of it
    /// at all. This pins `RADIANT_NAMES` against the four names that
    /// disproved the transform this table replaced -- a regression back to
    /// deriving the name from the classname would silently stop registering
    /// all four.
    #[test]
    fn placed_weapons_whose_radiant_name_is_not_a_simple_transform_still_register() {
        let (mut vm, mut host) = fixture();
        let lump = "{\n\"classname\" \"worldspawn\"\n}\n\
                    {\n\"classname\" \"mpweapon_mosinnagant\"\n}\n\
                    {\n\"classname\" \"mpweapon_mosinnagantsniper\"\n}\n\
                    {\n\"classname\" \"mpweapon_kar98k_scoped\"\n}\n\
                    {\n\"classname\" \"mpweapon_russiangrenade\"\n}\n";
        vm.with_cx(|cx| super::spawn_entities_from_string(&mut host, cx, lump))
            .unwrap();

        let mut expected = Items::new();
        expected.register("mosin_nagant_mp");
        expected.register("mosin_nagant_sniper_mp");
        expected.register("kar98k_sniper_mp");
        expected.register("rgd-33russianfrag_mp");
        assert_eq!(host.items.bitstring(), expected.bitstring());
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

    /// The entity lump fills the model configstring block before any script
    /// runs, in lump order and deduped, and a block whose `SP_` function
    /// frees still takes its slot: the index happens while the fields are
    /// parsed, which is before the classname is dispatched
    /// (docs/research/cod11-gsc-object-model.md section 7). Only one live
    /// entity survives this lump, yet the freed `misc_model` owns slot 269.
    #[test]
    fn entity_model_keys_take_model_slots_in_lump_order() {
        let (mut vm, mut host) = fixture();
        vm.with_cx(|cx| {
            super::spawn_entities_from_string(
                &mut host,
                cx,
                "{\n\"classname\" \"worldspawn\"\n}\n\
                 {\n\"classname\" \"misc_model\"\n\"model\" \"xmodel/barrels\"\n}\n\
                 {\n\"classname\" \"script_model\"\n\"model\" \"xmodel/crate_misc1\"\n}\n\
                 {\n\"classname\" \"misc_model\"\n\"model\" \"xmodel/barrels\"\n}\n",
            )
            .unwrap();
        });
        assert_eq!(host.ents.iter_inuse().count(), 1);
        assert_eq!(host.configstrings[269], "xmodel/barrels");
        assert_eq!(host.configstrings[270], "xmodel/crate_misc1");
        assert_eq!(host.configstrings[271], "");
    }

    /// A `model` value starting with `*` is a brush model: its number goes
    /// straight into the entity and `G_ModelIndex` is never called, so it
    /// takes no configstring slot. Both gate maps' lumps carry them, so
    /// indexing them would offset the whole block.
    #[test]
    fn a_brush_model_takes_no_model_slot() {
        let (mut vm, mut host) = fixture();
        vm.with_cx(|cx| {
            super::spawn_entities_from_string(
                &mut host,
                cx,
                "{\n\"classname\" \"worldspawn\"\n}\n\
                 {\n\"classname\" \"trigger_hurt\"\n\"model\" \"*1\"\n}\n\
                 {\n\"classname\" \"misc_model\"\n\"model\" \"xmodel/barrels\"\n}\n",
            )
            .unwrap();
        });
        assert_eq!(host.configstrings[269], "xmodel/barrels");
    }

    /// `RegisterItem` precaches the item's two model fields as well as
    /// setting its bit; for a weapon those are the weapon file's
    /// `worldModel` and `projectileModel`. The panzerfaust's world model is
    /// already the placed block's own `model` key, so the rocket is the
    /// slot the registration adds -- retail's mp_carentan capture has the
    /// two adjacent.
    #[test]
    fn a_placed_weapon_precaches_its_world_and_projectile_models() {
        let Some(fs) = vcod_common::testing::game_fs() else {
            return;
        };
        let (mut vm, mut host) = fixture();
        host.fs = Some(std::rc::Rc::new(fs));
        let lump = "{\n\"classname\" \"worldspawn\"\n}\n\
                    {\n\"classname\" \"mpweapon_panzerfaust\"\n\
                     \"model\" \"xmodel/weapon_panzerfaust_ammo\"\n}\n";
        vm.with_cx(|cx| super::spawn_entities_from_string(&mut host, cx, lump))
            .unwrap();
        assert_eq!(host.configstrings[269], "xmodel/weapon_panzerfaust_ammo");
        assert_eq!(host.configstrings[270], "xmodel/weapon_panzerfaust_rocket");
        assert_eq!(host.configstrings[271], "");
    }

    /// A block whose `SP_` function frees consumes no entity number: the
    /// slot goes back on the free list and the next block takes it. Measured
    /// on the retail server, where `light` and `misc_model` report zero live
    /// entities on every stock map (docs/research/cod11-gsc-object-model.md
    /// section 13).
    #[test]
    fn a_freeing_classname_consumes_no_entity_number() {
        let (mut vm, mut host) = fixture();
        vm.with_cx(|cx| {
            super::spawn_entities_from_string(
                &mut host,
                cx,
                "{\n\"classname\" \"worldspawn\"\n}\n\
                 {\n\"classname\" \"script_origin\"\n\"targetname\" \"a\"\n}\n\
                 {\n\"classname\" \"misc_model\"\n}\n\
                 {\n\"classname\" \"light\"\n}\n\
                 {\n\"classname\" \"corona\"\n}\n\
                 {\n\"classname\" \"info_null\"\n}\n\
                 {\n\"classname\" \"func_group\"\n}\n\
                 {\n\"classname\" \"script_origin\"\n\"targetname\" \"b\"\n}\n",
            )
            .unwrap();
            let ids: Vec<_> = host.ents.iter_inuse().map(|(i, _)| i).collect();
            assert_eq!(ids, vec![EntId(72), EntId(73)]);
        });
    }

    /// Every classname that frees is also in the `spawns` table, since the
    /// free is the table's `SP_` function running.
    #[test]
    fn the_freeing_classnames_are_a_subset_of_the_spawn_table() {
        assert_eq!(super::SPAWN_FREES.len(), 5);
        for cn in super::SPAWN_FREES {
            assert!(super::SPAWN_CLASSNAMES.contains(cn), "{cn} not in spawns");
        }
        // `misc_teleporter_dest`'s SP_ function is empty, not a free, and
        // `info_notnull` keeps its entity: both measured live.
        assert!(!super::SPAWN_FREES.contains(&"misc_teleporter_dest"));
        assert!(!super::SPAWN_FREES.contains(&"info_notnull"));
    }

    /// The map's own lump opens the model block, before a line of script
    /// runs: slot 269 is mp_pavlov's first `model` key, pinned against the
    /// committed retail capture. Before this, 269 held whatever the
    /// gametype script precached first and the whole block was offset.
    #[test]
    fn mp_pavlov_opens_the_model_block_with_its_lumps_first_model() {
        let Some(fs) = vcod_common::testing::game_fs() else {
            return;
        };
        let capture = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/configstrings/mp_pavlov-dm.txt"
        ))
        .expect("the committed retail capture");
        let retail_269 = capture
            .lines()
            .find_map(|l| l.strip_prefix("269 "))
            .expect("slot 269 in the capture");

        let bytes = fs.read("maps/mp/mp_pavlov.bsp").expect("mp_pavlov.bsp");
        let bsp = vcod_common::bsp::parse(&bytes).unwrap();
        let (mut vm, mut host) = fixture();
        vm.with_cx(|cx| super::spawn_entities_from_string(&mut host, cx, &bsp.entities))
            .unwrap();
        assert_eq!(host.configstrings[269], retail_269);
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

    /// `SP_worldspawn` (0x61cec) copies `northyaw` verbatim into configstring
    /// 11: a `G_SpawnString` read with an empty default, and if the result is
    /// the empty string it substitutes the literal `"0"` -- no numeric parse
    /// at any point, so a present key reproduces retail byte for byte
    /// whatever text it holds.
    #[test]
    fn worldspawns_northyaw_is_copied_verbatim_into_cs_11() {
        let (mut vm, mut host) = fixture();
        vm.with_cx(|cx| {
            super::spawn_entities_from_string(
                &mut host,
                cx,
                "{\n\"classname\" \"worldspawn\"\n\"northyaw\" \"90\"\n}\n",
            )
        })
        .unwrap();
        assert_eq!(host.configstrings[11], "90");
    }

    /// A worldspawn block with no `northyaw` key at all is `mp_carentan`'s
    /// own case (checked against its shipped BSP): retail's capture reads
    /// `"0"` there, which the disassembly shows is a literal substituted
    /// for the empty `G_SpawnString` result, not a rendered zero.
    #[test]
    fn a_missing_northyaw_key_writes_the_literal_zero() {
        let (mut vm, mut host) = fixture();
        vm.with_cx(|cx| {
            super::spawn_entities_from_string(&mut host, cx, "{\n\"classname\" \"worldspawn\"\n}\n")
        })
        .unwrap();
        assert_eq!(host.configstrings[11], "0");
    }

    /// `SP_trigger_hurt` (0x64ef8) always registers a sound alias, its own
    /// `sound` key or the compiled-in default `"world_hurt_me"` when the key
    /// is absent -- both gate maps place exactly one `trigger_hurt` with no
    /// `sound` key, which is why `world_hurt_me` takes slot 525 on both, one
    /// with no `misc_mg42` at all.
    #[test]
    fn a_trigger_hurt_with_no_sound_key_registers_the_default_alias() {
        let (mut vm, mut host) = fixture();
        vm.with_cx(|cx| {
            super::spawn_entities_from_string(
                &mut host,
                cx,
                "{\n\"classname\" \"worldspawn\"\n}\n\
                 {\n\"classname\" \"trigger_hurt\"\n}\n",
            )
        })
        .unwrap();
        assert_eq!(host.configstrings[525], "world_hurt_me");
    }

    /// A `trigger_hurt` with its own `sound` key registers that alias
    /// instead of the default, matching `G_SpawnString`'s key-found branch.
    #[test]
    fn a_trigger_hurts_own_sound_key_overrides_the_default() {
        let (mut vm, mut host) = fixture();
        vm.with_cx(|cx| {
            super::spawn_entities_from_string(
                &mut host,
                cx,
                "{\n\"classname\" \"worldspawn\"\n}\n\
                 {\n\"classname\" \"trigger_hurt\"\n\"sound\" \"custom_hurt\"\n}\n",
            )
        })
        .unwrap();
        assert_eq!(host.configstrings[525], "custom_hurt");
        assert_eq!(host.configstrings[526], "");
    }

    /// `G_SpawnTurret` (0x52c84), reached from `SP_turret` for both
    /// `misc_mg42` and `misc_turret`, reads its weapon file's
    /// `loopFireSound`/`stopFireSound` keys and registers each as a sound
    /// alias right after `RegisterItem`. Two `misc_mg42` blocks naming the
    /// same weapon file intern onto the same pair of slots, matching
    /// `mp_carentan`'s two mounted mg42s landing on exactly 526 and 527.
    #[test]
    fn a_mounted_mg42_registers_its_weapon_files_loop_and_cooldown_aliases() {
        let Some(fs) = vcod_common::testing::game_fs() else {
            return;
        };
        let (mut vm, mut host) = fixture();
        host.fs = Some(std::rc::Rc::new(fs));
        let lump = "{\n\"classname\" \"worldspawn\"\n}\n\
                    {\n\"classname\" \"trigger_hurt\"\n}\n\
                    {\n\"classname\" \"misc_mg42\"\n\"weaponinfo\" \"mg42_bipod_stand_mp\"\n}\n\
                    {\n\"classname\" \"misc_mg42\"\n\"weaponinfo\" \"mg42_bipod_stand_mp\"\n}\n";
        vm.with_cx(|cx| super::spawn_entities_from_string(&mut host, cx, lump))
            .unwrap();
        assert_eq!(host.configstrings[525], "world_hurt_me");
        assert_eq!(host.configstrings[526], "weap_mg42_loop");
        assert_eq!(host.configstrings[527], "weap_mg42_cooldown");
        assert_eq!(host.configstrings[528], "");
    }

    /// The full pipeline against both gate maps' own BSPs, pinned to the
    /// exact retail values in the committed captures -- the residual four
    /// slots `configstrings_ab` measures.
    #[test]
    fn both_gate_maps_reproduce_the_residual_configstrings() {
        let Some(fs) = vcod_common::testing::game_fs() else {
            return;
        };
        let fs = std::rc::Rc::new(fs);
        let cases: &[(&str, &[(usize, &str)])] = &[
            ("mp_pavlov", &[(11, "90"), (525, "world_hurt_me")]),
            (
                "mp_carentan",
                &[
                    (11, "0"),
                    (525, "world_hurt_me"),
                    (526, "weap_mg42_loop"),
                    (527, "weap_mg42_cooldown"),
                ],
            ),
        ];
        for (map, expected) in cases {
            let (mut vm, mut host) = fixture();
            host.fs = Some(fs.clone());
            let bytes = fs.read(&format!("maps/mp/{map}.bsp")).unwrap();
            let bsp = vcod_common::bsp::parse(&bytes).unwrap();
            vm.with_cx(|cx| super::spawn_entities_from_string(&mut host, cx, &bsp.entities))
                .unwrap();
            for (slot, value) in *expected {
                assert_eq!(host.configstrings[*slot], *value, "{map} cs[{slot}]");
            }
        }
    }
}
