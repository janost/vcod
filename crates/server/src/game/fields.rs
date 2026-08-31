//! The retail script field tables, transcribed from
//! `tools/re/dump_script_fields.py`. What each column means, and why `light`
//! is absent, is in docs/research/cod11-gsc-object-model.md section 3.

/// The storage conversion each field goes through, from
/// `Scr_GetGenericField`'s jump table (docs/research/cod11-gsc-object-model.md
/// section 4). Type 6 is unused by all three server tables.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum FieldType {
    Int,
    /// Stored as an int, seen by script as one of a fixed set of strings.
    /// Retail spells this as a set/get hook pair on the field record rather
    /// than a storage type: the setter (0x4af80, shared by every such field)
    /// walks the name table, stores the matching index, and raises a script
    /// error naming the whole table when nothing matches; the getter maps
    /// the index back to its name. The slice is that name table, in index
    /// order.
    Enum(&'static [&'static str]),
    Float,
    CString,
    IString,
    Vector,
    Entity,
    YawVector,
    Object,
    ModelIndex,
}

pub struct EngineField {
    pub name: &'static str,
    /// The retail `gentity_t`/`gclient_t` byte offset. Not dereferenced here;
    /// it is the alias key, since retail aliases two names by sharing one.
    pub offset: u16,
    pub ty: FieldType,
}

use FieldType::*;

pub const ENTITY_FIELDS: &[EngineField] = &[
    EngineField {
        name: "classname",
        offset: 374,
        ty: IString,
    },
    EngineField {
        name: "origin",
        offset: 308,
        ty: Vector,
    },
    EngineField {
        name: "model",
        offset: 373,
        ty: ModelIndex,
    },
    EngineField {
        name: "spawnflags",
        offset: 376,
        ty: Int,
    },
    EngineField {
        name: "speed",
        offset: 480,
        ty: Float,
    },
    EngineField {
        name: "closespeed",
        offset: 484,
        ty: Float,
    },
    EngineField {
        name: "target",
        offset: 468,
        ty: IString,
    },
    EngineField {
        name: "targetname",
        offset: 470,
        ty: IString,
    },
    EngineField {
        name: "message",
        offset: 456,
        ty: IString,
    },
    EngineField {
        name: "teamname",
        offset: 472,
        ty: IString,
    },
    EngineField {
        name: "wait",
        offset: 616,
        ty: Float,
    },
    EngineField {
        name: "random",
        offset: 620,
        ty: Float,
    },
    EngineField {
        name: "count",
        offset: 592,
        ty: Int,
    },
    EngineField {
        name: "health",
        offset: 560,
        ty: Int,
    },
    EngineField {
        name: "dmg",
        offset: 568,
        ty: Int,
    },
    EngineField {
        name: "angles",
        offset: 320,
        ty: Vector,
    },
    EngineField {
        name: "duration",
        offset: 636,
        ty: Float,
    },
    EngineField {
        name: "rotate",
        offset: 640,
        ty: Vector,
    },
    EngineField {
        name: "degrees",
        offset: 464,
        ty: Float,
    },
    EngineField {
        name: "time",
        offset: 480,
        ty: Float,
    },
    EngineField {
        name: "_color",
        offset: 668,
        ty: Vector,
    },
    EngineField {
        name: "color",
        offset: 668,
        ty: Vector,
    },
    EngineField {
        name: "key",
        offset: 680,
        ty: Int,
    },
    EngineField {
        name: "harc",
        offset: 684,
        ty: Float,
    },
    EngineField {
        name: "varc",
        offset: 688,
        ty: Float,
    },
    EngineField {
        name: "delay",
        offset: 628,
        ty: Float,
    },
    EngineField {
        name: "radius",
        offset: 624,
        ty: Int,
    },
    EngineField {
        name: "missionlevel",
        offset: 692,
        ty: Int,
    },
    EngineField {
        name: "start_size",
        offset: 700,
        ty: Int,
    },
    EngineField {
        name: "end_size",
        offset: 704,
        ty: Int,
    },
    EngineField {
        name: "shard",
        offset: 592,
        ty: Int,
    },
    EngineField {
        name: "spawnitem",
        offset: 712,
        ty: IString,
    },
    EngineField {
        name: "track",
        offset: 732,
        ty: IString,
    },
];

/// `Route::Client(i)` indexes this table by raw position, not through
/// `slot_of`'s offset dedup the way `route_entity` does. That is
/// deliberate: five of these entries (`sessionteam`, `sessionstate`,
/// `statusicon`, `headicon`, `headiconteam`) carry `offset: 0` because
/// retail's dump marks them "fully custom get and set"
/// (docs/research/cod11-gsc-object-model.md, "Client fields, 0x72ed4") —
/// they have no struct offset to alias on, not a shared one. Deduping by
/// offset the way `ENTITY_FIELDS` does would silently merge those five
/// distinct fields into one storage cell.
pub const CLIENT_FIELDS: &[EngineField] = &[
    EngineField {
        name: "name",
        offset: 8628,
        ty: CString,
    },
    EngineField {
        name: "sessionteam",
        offset: 0,
        ty: IString,
    },
    EngineField {
        name: "sessionstate",
        offset: 0,
        ty: IString,
    },
    EngineField {
        name: "maxhealth",
        offset: 8528,
        ty: Int,
    },
    EngineField {
        name: "handicap",
        offset: 8524,
        ty: Int,
    },
    EngineField {
        name: "score",
        offset: 8416,
        ty: Int,
    },
    EngineField {
        name: "deaths",
        offset: 8420,
        ty: Int,
    },
    EngineField {
        name: "statusicon",
        offset: 0,
        ty: IString,
    },
    EngineField {
        name: "headicon",
        offset: 0,
        ty: IString,
    },
    EngineField {
        name: "headiconteam",
        offset: 0,
        ty: IString,
    },
    EngineField {
        name: "spectatorclient",
        offset: 8404,
        ty: Int,
    },
    EngineField {
        name: "archivetime",
        offset: 8412,
        ty: Float,
    },
    EngineField {
        name: "pers",
        offset: 8424,
        ty: Object,
    },
];

pub const HUD_FIELDS: &[EngineField] = &[
    EngineField {
        name: "x",
        offset: 4,
        ty: Int,
    },
    EngineField {
        name: "y",
        offset: 8,
        ty: Int,
    },
    EngineField {
        name: "fontscale",
        offset: 12,
        ty: Float,
    },
    EngineField {
        name: "font",
        offset: 16,
        ty: Enum(FONTS),
    },
    EngineField {
        name: "alignx",
        offset: 20,
        ty: Enum(ALIGN_X),
    },
    EngineField {
        name: "aligny",
        offset: 24,
        ty: Enum(ALIGN_Y),
    },
    EngineField {
        name: "color",
        offset: 28,
        ty: Int,
    },
    EngineField {
        name: "alpha",
        offset: 28,
        ty: Int,
    },
    EngineField {
        name: "label",
        offset: 44,
        ty: Int,
    },
    EngineField {
        name: "sort",
        offset: 108,
        ty: Float,
    },
    EngineField {
        name: "archived",
        offset: 120,
        ty: Int,
    },
];

/// The three HUD enum tables, each read in index order from the pointer
/// table its field's setter passes to the shared name-to-index helper:
/// `font` 0x7de04, `alignx` 0x7de10, `aligny` 0x7de1c, three names each.
const FONTS: &[&str] = &["default", "bigfixed", "smallfixed"];
const ALIGN_X: &[&str] = &["left", "center", "right"];
const ALIGN_Y: &[&str] = &["top", "middle", "bottom"];

pub enum Route {
    /// An engine-backed field. `slot` is a dense index over the table's
    /// distinct offsets, so two names sharing a retail offset share a slot.
    Engine { slot: usize, ty: FieldType },
    /// A client field. Index into `CLIENT_FIELDS`. Reaching one needs a
    /// `gclient_t`, which stage 4 brings.
    Client(usize),
    /// Everything else: a radiant key, or a name a script invented. Both go
    /// to the entity's own struct in the VM heap.
    Script,
}

/// A dense slot per distinct retail offset, in first-appearance order, so
/// two names sharing an offset in retail share a cell here.
fn slot_of(table: &[EngineField], i: usize) -> usize {
    let off = table[i].offset;
    let first = table.iter().position(|f| f.offset == off).unwrap();
    table[..first]
        .iter()
        .map(|f| f.offset)
        .collect::<std::collections::BTreeSet<_>>()
        .len()
}

pub fn engine_slot_count() -> usize {
    slot_count(ENTITY_FIELDS)
}

/// `Route::Client`'s index for `.pers`, the one client field the engine
/// fills in itself rather than leaving for script. Panics if the name ever
/// leaves the table, which is the right answer: `spawn_client` cannot make
/// a usable client entity without it.
pub fn pers_index() -> usize {
    CLIENT_FIELDS
        .iter()
        .position(|f| f.name == "pers")
        .expect("CLIENT_FIELDS carries pers")
}

/// The same count over the HUD table, which is one shorter than the table:
/// `color` and `alpha` share an offset and so share a slot.
pub fn hud_slot_count() -> usize {
    slot_count(HUD_FIELDS)
}

fn slot_count(table: &[EngineField]) -> usize {
    table
        .iter()
        .map(|f| f.offset)
        .collect::<std::collections::BTreeSet<_>>()
        .len()
}

pub fn route_entity(folded: &str) -> Route {
    debug_assert!(
        !folded.chars().any(|c| c.is_ascii_uppercase()),
        "field names arrive folded; caller should use Cx::resolve_folded"
    );
    if let Some(i) = ENTITY_FIELDS.iter().position(|f| f.name == folded) {
        return Route::Engine {
            slot: slot_of(ENTITY_FIELDS, i),
            ty: ENTITY_FIELDS[i].ty,
        };
    }
    if let Some(i) = CLIENT_FIELDS.iter().position(|f| f.name == folded) {
        return Route::Client(i);
    }
    Route::Script
}

pub fn route_hud(folded: &str) -> Route {
    debug_assert!(
        !folded.chars().any(|c| c.is_ascii_uppercase()),
        "field names arrive folded; caller should use Cx::resolve_folded"
    );
    match HUD_FIELDS.iter().position(|f| f.name == folded) {
        Some(i) => Route::Engine {
            slot: slot_of(HUD_FIELDS, i),
            ty: HUD_FIELDS[i].ty,
        },
        None => Route::Script,
    }
}

/// `radiant/keys.txt` (pak4.pk3), the keys `Scr_FindField` answers for so
/// `G_ParseField`'s second stage stores them on the entity's script struct.
/// A BSP key in neither this list nor `ENTITY_FIELDS` is dropped at load and
/// script never sees it (docs/research/cod11-gsc-object-model.md section 7).
const RADIANT_KEYS: &[(&str, FieldType)] = &[
    ("script_wait", Float),
    ("script_additive_delay", Float),
    ("script_delay", Float),
    ("script_delay_min", Float),
    ("script_delay_max", Float),
    ("script_burst_min", Float),
    ("script_burst_max", Float),
    ("delay", Float),
    ("script_prespawn_delay", Float),
    ("script_accuracy", Float),
    ("script_accuracystationarymod", Float),
    ("script_suppression", Float),
    ("script_firefxdelay", Float),
    ("script_firefxtimeout", Float),
    ("script_health", Int),
    ("script_health_easy", Int),
    ("script_ignoreme", Int),
    ("script_fxstart", Int),
    ("script_fxstop", Int),
    ("script_delete", Int),
    ("script_increment", Int),
    ("script_patroller", Int),
    ("script_offtime", Int),
    ("script_offradius", Int),
    ("script_autosave", Int),
    ("count", Int),
    ("script_timer", Int),
    ("script_delayed_playerseek", Int),
    ("script_playerseek", Int),
    ("script_seekgoal", Int),
    ("radius", Int),
    ("script_start", Int),
    ("script_radius", Int),
    ("script_followmin", Int),
    ("script_followmax", Int),
    ("script_startinghealth", Int),
    ("script_fallback", Int),
    ("script_grenades", Int),
    ("script_moveoverride", Int),
    ("script_killspawner", Int),
    ("script_mg42auto", Int),
    ("script_turret", Int),
    ("script_min_friendlies", Int),
    ("script_requires_player", Int),
    ("script_sightrange", Int),
    ("spawnflags", Int),
    ("script_fallback_group", Int),
    ("script_vehiclegroup", Int),
    ("script_exploder", Int),
    ("script_balcony", Int),
    ("export", Int),
    ("script_mg42", Int),
    ("script_plane", Int),
    ("script_explode", Int),
    ("script_speed", Int),
    ("dontdropweapon", Int),
    ("dontdrawoncompass", Int),
    ("script_nodeathmessage", Int),
    ("script_order", Int),
    ("script_usemg42", Int),
    ("script_pacifist", Int),
    ("script_parachutegroup", Int),
    ("script_damage", Int),
    ("script_idnumber", Int),
    ("script_dawnville_fast", Int),
    ("script_fixbasepose", Int),
    ("script_tree", CString),
    ("target", CString),
    ("targetname", CString),
    ("groupname", CString),
    ("name", CString),
    ("script_objective", CString),
    ("script_friendname", CString),
    ("script_noteworthy", CString),
    ("script_path", CString),
    ("script_uniquename", CString),
    ("script_chain", CString),
    ("script_triggername", CString),
    ("script_kill_chain", CString),
    ("script_hint", CString),
    ("script_fxcommand", CString),
    ("script_fxid", CString),
    ("weaponinfo", CString),
    ("script_hidden", CString),
    ("vehicletype", CString),
    ("script_personality", CString),
    ("script_squadname", CString),
    ("script_namenumber", CString),
    ("script_commonname", CString),
    ("script_nodestate", CString),
    ("script_assaultnode", CString),
    ("script_team", CString),
    ("script_mortargroup", CString),
    ("ambient", CString),
    ("script_flaktype", CString),
    ("script_waittill", CString),
    ("script_animation", CString),
    ("script_favoriteenemy", CString),
    ("script_gameobjectname", CString),
    ("script_objective_name", CString),
    ("script_topfloor", CString),
    ("script_bottomfloor", CString),
    ("script_sound", CString),
    ("script_animname", CString),
    ("script_firefx", CString),
    ("script_earthquake", CString),
    ("script_presound", CString),
    ("script_ender", CString),
    ("script_firefxsound", CString),
    ("script_scatter", Int),
    ("script_linked", Int),
    ("script_hillgroup", Int),
    ("script_chaintarget", Int),
];

pub fn radiant_key(folded: &str) -> Option<FieldType> {
    RADIANT_KEYS
        .iter()
        .find(|(n, _)| *n == folded)
        .map(|(_, t)| *t)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The dumped counts. `light` is type 9, which
    /// `GScr_AddFieldsForEntity` rejects, so it is not a script field and is
    /// not in the table below: 34 records, 33 fields.
    #[test]
    fn the_tables_have_the_dumped_shape() {
        assert_eq!(ENTITY_FIELDS.len(), 33);
        assert_eq!(CLIENT_FIELDS.len(), 13);
        assert_eq!(HUD_FIELDS.len(), 11);
        assert!(!ENTITY_FIELDS.iter().any(|f| f.name == "light"));
    }

    /// Retail aliases fields by sharing a struct offset: `speed`/`time`,
    /// `count`/`shard` and `_color`/`color` are three pairs of names over
    /// three storage slots, so a write through one name is visible through
    /// the other. Slots are dense indices over the distinct offsets.
    #[test]
    fn aliased_names_share_one_storage_slot() {
        let slot = |n: &str| match route_entity(n) {
            Route::Engine { slot, .. } => slot,
            _ => panic!("{n} is not an engine field"),
        };
        assert_eq!(slot("speed"), slot("time"));
        assert_eq!(slot("count"), slot("shard"));
        assert_eq!(slot("color"), slot("_color"));
        assert_ne!(slot("origin"), slot("angles"));
        assert_eq!(engine_slot_count(), 30);
    }

    /// Field names arrive already folded: `Cx::resolve_folded` lowercases,
    /// and so does the map load's key walk, so the table compares exactly.
    /// The engine-level folding this rests on is covered where it happens,
    /// in `host.rs`'s `field_access_folds_case`.
    #[test]
    fn the_table_matches_folded_names_exactly() {
        assert!(matches!(route_entity("targetname"), Route::Engine { .. }));
        assert!(matches!(route_entity("classname"), Route::Engine { .. }));
        assert!(matches!(route_hud("fontscale"), Route::Engine { .. }));
    }

    /// The three-way split. `sessionteam` is a client field, so it routes to
    /// the client table; `script_exploder` is in radiant/keys.txt and every
    /// unknown name is script-defined, and both land on the entity's own
    /// struct.
    #[test]
    fn the_split_is_three_ways() {
        assert!(matches!(route_entity("origin"), Route::Engine { .. }));
        assert!(matches!(route_entity("sessionteam"), Route::Client(_)));
        assert!(matches!(route_entity("script_exploder"), Route::Script));
        assert!(matches!(route_entity("no_such_field"), Route::Script));
    }

    /// The radiant keys that decide what survives a map load. `light` is not
    /// one: it is an entity-table record the engine rejects, so a `light`
    /// key on a BSP block reaches neither store.
    #[test]
    fn radiant_keys_carry_their_declared_type() {
        assert!(matches!(
            radiant_key("script_exploder"),
            Some(FieldType::Int)
        ));
        assert!(matches!(
            radiant_key("script_noteworthy"),
            Some(FieldType::CString)
        ));
        assert!(matches!(
            radiant_key("script_delay"),
            Some(FieldType::Float)
        ));
        assert!(radiant_key("light").is_none());
        assert!(radiant_key("no_such_key").is_none());
    }

    /// 113 keys: 56 int, 43 string, 14 float
    /// (docs/research/cod11-gsc-object-model.md section 6).
    #[test]
    fn the_radiant_key_set_is_complete() {
        assert_eq!(RADIANT_KEYS.len(), 113);
        let n = |t: FieldType| RADIANT_KEYS.iter().filter(|(_, x)| *x == t).count();
        assert_eq!(n(FieldType::Int), 56);
        assert_eq!(n(FieldType::CString), 43);
        assert_eq!(n(FieldType::Float), 14);
    }
}
