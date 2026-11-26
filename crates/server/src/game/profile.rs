//! What one game version's host does differently from another's: the VM's
//! rules, the game module's tables, the builtin set and every rule the host,
//! spawn, item, vote, field and print code tells the versions apart by,
//! each named. That code reads `host.profile`, never the game. One static
//! per version; the load picks it (`ScriptRuntime::load_game`).

use vcod_common::game::Game;
use vcod_common::net::protocol::{Cod1Wire, PROTOCOL_V1, PROTOCOL_V6, PROTOCOL_V22};
use vcod_gsc::VmProfile;

use crate::defs::{self, Defs};
use crate::game::builtins::math::{CloserRounding, MathRounding};
use crate::game::builtins::set::BuiltinSet;
use crate::game::fields::{self, Route};
use crate::game::trigger::{self, TriggerKind};
use crate::game::vote::VoteRules;

/// One game version's host rules.
pub struct HostProfile {
    pub game: Game,
    pub vm: VmProfile,
    /// The game module's tables; `GameHost::defs` starts as these.
    pub defs: &'static Defs,
    /// The builtins this version's compiler knows (`builtins::set`).
    pub builtins: fn() -> &'static BuiltinSet,
    /// The loader checks a call against [`Self::builtins`]' names while it
    /// compiles (CoD2's `Loader::set_builtins`).
    pub compile_against_builtins: bool,
    /// `G_Spawn` hands a freed slot out again only this long after its free
    /// (CoD2's 500 ms, cod2-protocol.md 15.1); `None` at once.
    pub ent_reuse_after_ms: Option<i32>,
    /// `delete()` puts the number on `G_Spawn`'s free list inside the call
    /// (CoD4 1.7's `Scr_Delete` 0x80c3cd6 ends in `G_FreeEntity`,
    /// cod4-combat.md 14); the entity still goes 100 ms out, as CoD1's.
    pub delete_queues_number: bool,
    /// `delete()` on a client is a script error (`Cannot delete a client
    /// entity`, CoD4 1.7's `Scr_Delete`, cod4-combat.md 14).
    pub delete_refuses_client: bool,
    /// `ClientEndFrame` stores the client's `maxspeed` in `ps.speed`
    /// (1.5b 0x3ada8, UO 0x4329f); 1.1's `(int)g_speed.value` (game.mp
    /// 0x41100).
    pub speed_off_maxspeed: bool,
    /// A client has the `maxspeed` and `sessionsquad` fields (1.5b's and
    /// UO's; cod15-movement.md, cod15-protocol-diff.md "say_squad"); on 1.1
    /// both are plain script variables.
    pub maxspeed_and_squads: bool,
    /// `G_ShaderIndex` lowercases the name (CoD2's `I_strlwr`,
    /// cod2-protocol.md 15.3).
    pub lowercase_shaders: bool,
    /// The load reads `radiant/keys.txt` (`GScr_AddFieldsForRadiant`,
    /// cod2-gsc-diff.md 2.1); else 1.1's compiled-in keys.
    pub reads_radiant_keys: bool,
    /// The damage callback takes `timeOffset`, the killed callback
    /// `timeOffset` and `deathAnimDuration` (cod2-gsc-diff.md 4.1).
    pub callback_extra_args: bool,
    /// The spawn pass keeps `script_struct` blocks, built into
    /// `level.struct` off `codescripts/struct` after it (cod2-gsc-diff.md
    /// 2.4).
    pub script_structs: bool,
    /// The load runs `InitStructs` with no `script_struct` in the map too
    /// (CoD4's `G_LoadStructs`, cod4-gsc.md 4), so `level.struct` is
    /// always an array.
    pub init_structs_always: bool,
    /// How a weapon name gets its wire index.
    pub weapons: WeaponNumbering,
    /// `G_InitGame` writes configstring 22's `\winner\0`.
    pub init_winner: bool,
    /// A link's angle snap keeps the unsnapped angles for the wire's
    /// `apos.trBase` (`Entity::apos_base`, cod2-protocol.md 15.6).
    pub unsnapped_apos: bool,
    /// `SV_LinkEntity`'s snap of `r.currentAngles` on every link:
    /// CoD1's truncating one (`mover::snap_angles`) or 1.7's flooring one
    /// (`mover::snap_angles4`).
    pub link_snap: fn(glam::Vec3) -> glam::Vec3,
    /// A script field's route on an entity and on a HUD element
    /// (`game::fields`).
    pub route_entity: fn(&str) -> Route,
    pub route_hud: fn(&str) -> Route,
    /// The trigger a spawn table classname makes.
    pub trigger_kind: fn(&str) -> Option<TriggerKind>,
    pub spawn: SpawnRules,
    pub items: ItemRules,
    pub votes: VoteRules,
    pub print: PrintRules,
    /// The dvars `setCvar` stores as a colour (CoD2's `g_TeamColor_*`).
    pub color_dvars: &'static [&'static str],
    /// `addTestClient` connects the bot as a real client connects and ends
    /// in `SV_ClientEnterWorld` (CoD2's `SV_AddTestClient`, lnxded
    /// 0x80913da): a fresh client state, the roster connecting then active.
    pub test_client_enters_world: bool,
    /// The loop sounds broadcast through `r.broadcastTime` on 1.1 too
    /// (CoD2's, cod2-protocol.md 15.7).
    pub loop_sounds_broadcast_time: bool,
    /// The snapshot builder takes a set `r.broadcastTime` ahead of the
    /// `SVF_NOCLIENT` test (CoD2's `SV_AddEntitiesVisibleFromPoint`,
    /// lnxded 0x8098b98; cod2-protocol.md 15.7).
    pub broadcast_time_sends: bool,
    /// How a trigger's entity state goes out (`game::wire`).
    pub trigger_wire: TriggerWire,
    /// The `eType`s, `eFlags` bits, `trType`s and ring events
    /// `game::wire` builds an entity in.
    pub entities: &'static crate::game::wire::EntityNumbers,
    /// The console line `map_restart()` runs: CoD2's in-place
    /// `fast_restart`, where its `map_restart` is a full load
    /// (cod2-map-cycle.md 1).
    pub restart_command: &'static str,
    /// `G_Say`'s rules.
    pub say: SayRules,
    /// The combat code's rules (`game::combat`, the damage builtins).
    pub combat: CombatRules,
    /// The missile pool's rules (`game::missile`).
    pub missiles: MissileRules,
    /// The log takes 1.1's line forms (`say: %s: %s`, `Weapon: %i %s`)
    /// rather than the `;` records 1.5, UO and CoD2 write
    /// (cod11-map-cycle.md 9).
    pub log_1_1_lines: bool,
    /// `G_RunFrame`'s one thread pass runs before `G_XAnimUpdate`, on the
    /// last frame's DObjs (1.5b 0x50821, 0x50840; UO 0x59e4d, 0x59e6c);
    /// 1.1's second pass follows `G_DObjUpdateServerTime`.
    pub threads_on_last_dobjs: bool,
    /// A mover's position comes off 1.5b's and UO's stored-float
    /// evaluation (`mover::bg_evaluate`).
    pub stored_movers: bool,
    /// `G_RunFrame` ends in `CheckMatchTimeout` (cod15-protocol-diff.md
    /// 12). Only CoD1's frame reads it.
    pub match_timeout: bool,
    /// `G_InitGame` opens with `G_ParseScrVehicleInfo` (UO's 0x586cf):
    /// the level has vehicles.
    pub vehicles: bool,
    /// `vehicletype` is a read-only entity field (UO's).
    pub readonly_vehicletype: bool,
    /// `headiconteam` takes `spectator` and refuses any other unknown name
    /// (UO's); else an unknown name is 0.
    pub head_icon_spectator: bool,
    /// A client's model change runs `G_UpdateClientInfo` (UO's 0x87df9);
    /// 1.1d's and 1.5b's return.
    pub model_change_updates_client: bool,
    /// Where `closer()` rounds (`builtins::math::closer`).
    pub closer: CloserRounding,
    /// How the distance, length, normalize, `anglesTo*` and trig builtins
    /// round (`builtins::math`).
    pub math: MathRounding,
}

/// `G_Say`'s rules (`game::say`).
pub struct SayRules {
    /// A say's status colours the name by side and reads death off the
    /// session state, not the health (1.5b, cod15-protocol-diff.md 13).
    pub side_status: bool,
    /// `g_deadChat` lets the dead and the spectators reach everyone
    /// (1.5b's).
    pub dead_chat: bool,
}

/// The combat code's rules.
pub struct CombatRules {
    /// How far a CoD1 server traces a bullet: 8192 on 1.1 and 1.5, UO's
    /// `BG_Bullet_Endpos` (game.mp.uo 0x3011c) 10000 (combat doc 2.2).
    pub bullet_range: f32,
    /// The players' bodies stop a blast's `CanDamage` traces: 1.1's mask
    /// 0x2802091 holds `CONTENTS_BODY`, UO's 0x802091 (game.mp.uo
    /// `CanDamage` 0x503f8) does not.
    pub bodies_shield_blasts: bool,
    /// A `trigger_damage` reads `teamname` and fires only for a hit from
    /// that team (UO's, combat doc 14.8).
    pub damage_trigger_teams: bool,
    /// `Respond_trigger_damage` reads UO's renumbered means of death and
    /// has no 0x40 arm (`damage_trigger::responds`).
    pub damage_trigger_uo_means: bool,
    /// An entity's `"damage"` notify takes six arguments, the damage
    /// floored at 1 (UO's, `EntityHit::uo_args`).
    pub damage_notify_six_args: bool,
    /// `obituary()` writes nothing under `sv_obituary 0` (UO's).
    pub obituary_cvar: bool,
    /// `obituary()` raises its event at `vec3_origin` (UO's 0x6a819), not
    /// at the victim's origin.
    pub obituary_at_origin: bool,
    /// `radiusDamage()` takes an attacker and an inflictor after its four
    /// arguments (UO's 0x6c229, coduo-vehicles.md 11.2).
    pub radius_damage_attacker_args: bool,
    /// A player's damage takes its direction off `VectorNormalize2` stored
    /// (1.5b 0x3ee76, UO 0x47634); 1.1d's normalizes on the stack
    /// (cod11-combat.md 4.5).
    pub stored_damage_dir: bool,
    /// A turret heats while it fires and overheats (UO's).
    pub turret_heat: bool,
}

/// The missile pool's rules (`game::missile::Missiles`).
pub struct MissileRules {
    /// United Offensive's missile rules: its bounce, its explode's type in
    /// `es.scale` and no concussion entity.
    pub uo: bool,
    /// A resting missile falls once its floor is gone (1.5b 0x54ad7, UO
    /// 0x5ee9c; cod11-combat.md 12.6).
    pub falls: bool,
}

/// A trigger's entity state on the wire.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TriggerWire {
    /// 1.1's `InitTrigger` `eFlags` 2; a `trigger_use`'s hint in
    /// `_union.scale`.
    Cod1,
    /// CoD2's `InitTrigger` `eFlags` 1, none on a round or lookat
    /// trigger; a `trigger_use`'s hint in `scale` (cod2-protocol.md 15.6).
    Cod2,
    /// CoD4's: CoD2's with `eFlags` 0x21, the nodraw bit 0x20 of a trigger
    /// with no model on top (cod4-gsc.md 7).
    Cod4,
}

/// The message builtins' rules.
pub struct PrintRules {
    /// `iPrintLn`'s command letter comes off its arguments' `va` buffers
    /// (CoD2's, cod2-protocol.md 15.5); else `f`.
    pub letter_from_args: bool,
    /// The localized-string packer has neither the trailing `\x15` nor the
    /// letters arm (CoD2's 0x8110720).
    pub plain_packer: bool,
}

/// The item code's rules (`game::item`).
pub struct ItemRules {
    /// The module's `rand() / 2^31` reads negated (1.1d's compile, items
    /// doc 14.2; CoD2's `crandom` reads [-1, 1)).
    pub negative_unit_draw: bool,
    /// `G_RunItem` runs CoD2's own flight (`cod2_items::run_flight`).
    pub own_flight: bool,
    /// `BG_TakePlayerWeapon` empties the hand when it takes the weapon in
    /// it (CoD2's, Mac 0x7bec4); 1.1's leaves `ps.weapon` for pmove.
    pub take_empties_hand: bool,
    /// A launched row is a weapon list index: its name the weapon's where
    /// the item rows have none, its classname `weapon_<name>`.
    pub weapon_rows: bool,
    /// `Drop_Item`'s launch reads the `g_drop*` dvars
    /// (`cod2_items::launch_off`).
    pub cvar_launch: bool,
    /// A drop at a tag waits for the body's pose and takes the player's
    /// angles (`cod2_items::TagDrop`).
    pub tag_drops: bool,
    /// The drop ring holds entity numbers (1.1d's, `DropRing::claim`).
    pub drop_ring_by_number: bool,
}

/// [`SpawnRules::weapon_pickup`]'s registration.
pub type WeaponPickup =
    fn(&mut crate::game::host::GameHost, &str) -> Result<(), vcod_gsc::ErrorKind>;

/// The spawn pass's rules.
pub struct SpawnRules {
    /// Where `SP_worldspawn`'s `message` and `northyaw` and `ambientPlay`'s
    /// track go: 4, 11 and 3 on CoD1 and CoD2, 3, 822 and 821 on CoD4
    /// (`game::cod4::CS_LAYOUT_COD4`).
    pub message_slot: usize,
    pub northyaw_slot: usize,
    pub ambient_slot: usize,
    /// A `weapon_<name>` classname registers weapon `<name>` this way
    /// (CoD4's, `builtins::cod4::register_weapon_pickup`); the item itself
    /// is not built yet. `None` reads the item rows.
    pub weapon_pickup: Option<WeaponPickup>,
    /// `SP_worldspawn` opens with configstring 2 at this (CoD2's `cod`,
    /// cod2-protocol.md 15.3).
    pub worldspawn_cs2: Option<&'static str>,
    /// The spawn table has `func_bobbing`, `func_pendulum` and
    /// `misc_spawner` (CoD2's has none, cod2-gsc-diff.md 2.4).
    pub sine_movers_and_spawner: bool,
    /// `SP_trigger_hurt` indexes its sound (CoD2's reads the key and
    /// indexes nothing, cod2-protocol.md 15.3).
    pub hurt_sound: bool,
    /// `trigger_radius` and `trigger_disk` take their round shape
    /// (`spawn::round_trigger`).
    pub round_triggers: bool,
    /// `G_SpawnTurret` takes the weapon's index first and indexes the
    /// player fire sounds too (CoD2, cod2-turrets.md 2).
    pub turret_weapon_and_player_sounds: bool,
    /// `G_FindTeams` chains the map's `teamname`s after the spawn pass
    /// (`crate::game::teams`); CoD2's and CoD4's are not measured.
    pub find_teams: bool,
    /// The spawn table has `trigger_mount` (UO's `SP_trigger_mount`).
    pub trigger_mount: bool,
}

/// How a weapon name gets its 1-based wire index.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum WeaponNumbering {
    /// [`Defs::weapon_index`]'s fixed list.
    Fixed,
    /// `G_GetWeaponIndexForName`: the level's weapons in the order it
    /// named them, `defaultweapon_mp` first at the load, written out after
    /// the bootstrap (`builtins::cod2::register_weapon`, cod2-protocol.md
    /// 15.3).
    Registered,
}

/// Which CoD1 version [`cod1`] builds.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Cod1Version {
    V11,
    V15,
    Uo,
}

const fn cod1(
    defs: &'static Defs,
    builtins: fn() -> &'static BuiltinSet,
    v: Cod1Version,
) -> HostProfile {
    let v1 = matches!(v, Cod1Version::V11);
    let uo = matches!(v, Cod1Version::Uo);
    HostProfile {
        game: Game::Cod1,
        vm: VmProfile::COD1,
        defs,
        builtins,
        compile_against_builtins: false,
        ent_reuse_after_ms: None,
        delete_queues_number: false,
        delete_refuses_client: false,
        speed_off_maxspeed: !v1,
        maxspeed_and_squads: !v1,
        lowercase_shaders: false,
        reads_radiant_keys: false,
        callback_extra_args: false,
        script_structs: false,
        init_structs_always: false,
        weapons: WeaponNumbering::Fixed,
        init_winner: false,
        unsnapped_apos: false,
        link_snap: crate::game::mover::snap_angles,
        route_entity: fields::route_entity,
        route_hud: fields::route_hud,
        trigger_kind: trigger::kind_of,
        spawn: SpawnRules {
            message_slot: 4,
            northyaw_slot: vcod_common::net::protocol::CS_NORTHYAW,
            ambient_slot: 3,
            weapon_pickup: None,
            worldspawn_cs2: None,
            sine_movers_and_spawner: true,
            hurt_sound: true,
            round_triggers: false,
            turret_weapon_and_player_sounds: false,
            find_teams: true,
            trigger_mount: uo,
        },
        items: ItemRules {
            negative_unit_draw: v1,
            own_flight: false,
            take_empties_hand: false,
            weapon_rows: false,
            cvar_launch: false,
            tag_drops: false,
            drop_ring_by_number: v1,
        },
        votes: if v1 {
            VoteRules::COD1
        } else {
            VoteRules::COD1_15
        },
        print: PrintRules {
            letter_from_args: false,
            plain_packer: false,
        },
        color_dvars: &[],
        test_client_enters_world: false,
        loop_sounds_broadcast_time: false,
        broadcast_time_sends: false,
        trigger_wire: TriggerWire::Cod1,
        entities: &crate::game::wire::COD1_ENTITY_NUMBERS,
        restart_command: "map_restart",
        say: SayRules {
            side_status: !v1,
            dead_chat: !v1,
        },
        combat: CombatRules {
            bullet_range: if uo {
                crate::game::combat::UO_BULLET_RANGE
            } else {
                crate::game::combat::BULLET_RANGE
            },
            bodies_shield_blasts: !uo,
            damage_trigger_teams: uo,
            damage_trigger_uo_means: uo,
            damage_notify_six_args: uo,
            obituary_cvar: uo,
            obituary_at_origin: uo,
            radius_damage_attacker_args: uo,
            stored_damage_dir: !v1,
            turret_heat: uo,
        },
        missiles: MissileRules { uo, falls: !v1 },
        log_1_1_lines: v1,
        threads_on_last_dobjs: !v1,
        stored_movers: !v1,
        match_timeout: !v1,
        vehicles: uo,
        readonly_vehicletype: uo,
        head_icon_spectator: uo,
        model_change_updates_client: uo,
        closer: if v1 {
            CloserRounding::FirstOnly
        } else {
            CloserRounding::Floats
        },
        math: if v1 {
            MathRounding::Kept
        } else {
            MathRounding::Stored
        },
    }
}

/// 1.1.
pub static COD1_11: HostProfile = cod1(&defs::V1, BuiltinSet::v11, Cod1Version::V11);
/// 1.5.
pub static COD1_15: HostProfile = HostProfile {
    vm: VmProfile::COD1_15,
    ..cod1(&defs::V15, BuiltinSet::v15, Cod1Version::V15)
};
/// United Offensive 1.51.
pub static COD1_UO: HostProfile = HostProfile {
    vm: VmProfile::COD1_UO,
    ..cod1(&defs::UO, BuiltinSet::uo, Cod1Version::Uo)
};

/// CoD2 1.3.
pub static COD2: HostProfile = HostProfile {
    game: Game::Cod2,
    vm: VmProfile::COD2,
    defs: &defs::COD2,
    builtins: BuiltinSet::cod2,
    compile_against_builtins: true,
    ent_reuse_after_ms: Some(500),
    delete_queues_number: false,
    delete_refuses_client: false,
    // 1.1's, unmeasured (cod2-gsc-diff.md 5.1).
    speed_off_maxspeed: false,
    maxspeed_and_squads: false,
    lowercase_shaders: true,
    reads_radiant_keys: true,
    callback_extra_args: true,
    script_structs: true,
    init_structs_always: false,
    weapons: WeaponNumbering::Registered,
    init_winner: true,
    unsnapped_apos: true,
    link_snap: crate::game::mover::snap_angles,
    route_entity: fields::route_entity_cod2,
    route_hud: fields::route_hud_cod2,
    trigger_kind: trigger::kind_of_cod2,
    spawn: SpawnRules {
        message_slot: 4,
        northyaw_slot: vcod_common::net::protocol::CS_NORTHYAW,
        ambient_slot: 3,
        weapon_pickup: None,
        worldspawn_cs2: Some("cod"),
        sine_movers_and_spawner: false,
        hurt_sound: false,
        round_triggers: true,
        turret_weapon_and_player_sounds: true,
        find_teams: false,
        trigger_mount: false,
    },
    items: ItemRules {
        negative_unit_draw: false,
        own_flight: true,
        take_empties_hand: true,
        weapon_rows: true,
        cvar_launch: true,
        tag_drops: true,
        drop_ring_by_number: true,
    },
    votes: VoteRules::COD2,
    print: PrintRules {
        letter_from_args: true,
        plain_packer: true,
    },
    color_dvars: crate::game::builtins::cvar::COD2_COLOR_DVARS,
    test_client_enters_world: true,
    loop_sounds_broadcast_time: true,
    broadcast_time_sends: true,
    trigger_wire: TriggerWire::Cod2,
    entities: &crate::game::cod2_wire::COD2_ENTITY_NUMBERS,
    restart_command: "fast_restart",
    // 1.1's say and log rules, unmeasured (cod2-gsc-diff.md 5.1).
    say: SayRules {
        side_status: false,
        dead_chat: false,
    },
    combat: COD2_COMBAT,
    missiles: MissileRules {
        uo: false,
        falls: false,
    },
    log_1_1_lines: false,
    threads_on_last_dobjs: false,
    stored_movers: false,
    match_timeout: false,
    vehicles: false,
    readonly_vehicletype: false,
    head_icon_spectator: false,
    model_change_updates_client: false,
    closer: CloserRounding::Floats,
    math: MathRounding::Cod2,
};

/// CoD2's and CoD4's combat rules: 1.1's where the CoD1 code reads them.
const COD2_COMBAT: CombatRules = CombatRules {
    bullet_range: crate::game::combat::BULLET_RANGE,
    bodies_shield_blasts: true,
    damage_trigger_teams: false,
    damage_trigger_uo_means: false,
    damage_notify_six_args: false,
    obituary_cvar: false,
    obituary_at_origin: false,
    radius_damage_attacker_args: false,
    stored_damage_dir: false,
    turret_heat: false,
};

/// CoD4 1.7: CoD2's rules wherever vcod has not measured 1.7's own
/// (docs/research/cod4-gsc.md 4). Its weapons take the fixed (empty) list
/// (the level keeps its own table, `game::cod4::weapons`), and the damage
/// and killed callbacks take CoD2's tenth and later arguments (the stock
/// `_callbacksetup.gsc` names them `timeOffset`, `deathAnimDuration`).
pub static COD4: HostProfile = HostProfile {
    game: Game::Cod4,
    vm: VmProfile::COD4,
    defs: &crate::game::cod4::COD4_DEFS,
    builtins: BuiltinSet::cod4,
    compile_against_builtins: true,
    ent_reuse_after_ms: Some(500),
    delete_queues_number: true,
    delete_refuses_client: true,
    // 1.1's, unmeasured (cod2-gsc-diff.md 5.1).
    speed_off_maxspeed: false,
    maxspeed_and_squads: false,
    lowercase_shaders: true,
    reads_radiant_keys: true,
    callback_extra_args: true,
    script_structs: true,
    init_structs_always: true,
    weapons: WeaponNumbering::Fixed,
    init_winner: true,
    unsnapped_apos: true,
    link_snap: crate::game::mover::snap_angles4,
    route_entity: fields::route_entity_cod2,
    route_hud: fields::route_hud_cod4,
    trigger_kind: trigger::kind_of_cod2,
    spawn: SpawnRules {
        message_slot: 3,
        northyaw_slot: crate::game::cod4::CS_NORTHYAW,
        ambient_slot: crate::game::cod4::CS_AMBIENT,
        weapon_pickup: Some(crate::game::builtins::cod4::register_weapon_pickup),
        worldspawn_cs2: Some("cod"),
        sine_movers_and_spawner: false,
        hurt_sound: false,
        round_triggers: true,
        turret_weapon_and_player_sounds: true,
        find_teams: false,
        trigger_mount: false,
    },
    items: ItemRules {
        negative_unit_draw: false,
        own_flight: true,
        take_empties_hand: true,
        weapon_rows: true,
        cvar_launch: true,
        tag_drops: true,
        drop_ring_by_number: true,
    },
    votes: VoteRules::COD4,
    print: PrintRules {
        letter_from_args: true,
        plain_packer: true,
    },
    color_dvars: crate::game::builtins::cvar::COD4_COLOR_DVARS,
    test_client_enters_world: true,
    loop_sounds_broadcast_time: true,
    broadcast_time_sends: true,
    trigger_wire: TriggerWire::Cod4,
    entities: &crate::game::cod2_wire::COD2_ENTITY_NUMBERS,
    restart_command: "fast_restart",
    // `side_status`, `dead_chat`, `threads_on_last_dobjs` and
    // `stored_movers` are 1.5's: the host took 1.5's arms off the
    // `protocol` dvar, 6 on 1.7 as on 1.5. Unmeasured on 1.7; the say's
    // colours are (`GameHost::say`).
    say: SayRules {
        side_status: true,
        dead_chat: true,
    },
    combat: COD2_COMBAT,
    missiles: MissileRules {
        uo: false,
        falls: false,
    },
    log_1_1_lines: false,
    threads_on_last_dobjs: true,
    stored_movers: true,
    match_timeout: false,
    vehicles: false,
    readonly_vehicletype: false,
    head_icon_spectator: false,
    model_change_updates_client: false,
    closer: CloserRounding::None,
    math: MathRounding::Float,
};

impl HostProfile {
    /// CoD1's profile for `proto`, as [`Defs::of`] picks its tables.
    pub fn cod1(proto: &Cod1Wire) -> &'static HostProfile {
        match proto.transport.version {
            v if v == PROTOCOL_V22.transport.version => &COD1_UO,
            v if v == PROTOCOL_V6.transport.version => &COD1_15,
            _ => &COD1_11,
        }
    }

    /// CoD1's profile for a `protocol` cvar, 1.1's for one no version has.
    pub fn of_protocol_cvar(protocol: &str) -> &'static HostProfile {
        let proto = protocol
            .parse()
            .ok()
            .and_then(|v| Cod1Wire::by_version(Game::Cod1, v))
            .unwrap_or(&PROTOCOL_V1);
        Self::cod1(proto)
    }
}
