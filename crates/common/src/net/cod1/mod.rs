//! Call of Duty (2003)'s wire data: the netfield tables, configstring
//! layouts and numbering of patch 1.1 (protocol 1), 1.5 (6) and United
//! Offensive 1.51 (22), and the maps that rewrite 1.5's and UO's numbering
//! into 1.1's, the one the rest of vcod reads. The codec they run through is
//! [`crate::net::codec`]; the descriptor type is [`Cod1Wire`].

pub mod event_ids;
pub mod fields_v1;
pub mod fields_v22;
pub mod fields_v6;
pub mod flags;
mod wire;

pub use wire::{Cod1Wire, CursorHints};

use crate::game::Game;
use crate::net::protocol::{
    CsLayout, CsRange, FieldCodec, HudWire, NetchanParams, PakRules, Transport,
};

const HUD_V1: HudWire = HudWire {
    count_bits: 5,
    max_elems: 31,
    fields: 28,
};

pub const PROTOCOL_V1: Cod1Wire = Cod1Wire {
    transport: Transport {
        game: Game::Cod1,
        version: 1, // reported by cod_lnxded 1.1d getstatus, 2026-08-23
        short_version: "1.1",
        own_master: None,
        max_configstrings: 2048, // codextended/src/shared.h:141
        field_codec: FieldCodec::COD1,
        netchan: NetchanParams::COD1,
        paks: PakRules {
            // `%s/pak%d` for 0..8, CoDMP.exe 0x43b6b0.
            id_pak_numbers: &["0", "1", "2", "3", "4", "5", "6", "7", "8"],
            id_pak_dirs: &["main"],
            id_pak_prefixes: &["pak", "mp_pak", "sp_pak"],
            skips_svr_paks: false,
        },
    },
    entity_fields: fields_v1::ENTITY_FIELDS,
    vehicle_fields: &[],
    vehicle_field_slots: &[],
    player_fields: fields_v1::PLAYER_FIELDS,
    client_fields: fields_v1::CLIENT_FIELDS,
    pm_flags_to_v1: |f| f,
    pm_flags_from_v1: flags::pm_flags_v1_wire,
    map_restart_command: "n",
    eflags_to_v1: |f| f,
    eflags_from_v1: |f| f,
    event_from_v1: |e| e,
    max_weapons: 64, // bg_itemlist, game.mp.i386.so .data 0x7b9d8
    // game.mp.i386.so 0x4f787, 0x4f7b9 (1.5b: 0x4e51c, 0x4e56c).
    cursor_hints: CursorHints {
        health: 7,
        weapon: 9,
    },
    hud: HUD_V1,
    usercmd_weapon_bits: 6,
    cs: CS_LAYOUT_V1,
    et_events: 12,
    event_to_v1: |e| e,
    mod_to_v1: |m| m,
    pure_dlls: ["cgame_mp_x86.dll", "ui_mp_x86.dll"],
    team_colour_codes: false,
};

/// Patch 1.5: 1.1's wire with a wider, renumbered `pm_flags`
/// (docs/research/cod15-protocol-diff.md).
pub const PROTOCOL_V6: Cod1Wire = Cod1Wire {
    transport: Transport {
        game: Game::Cod1,
        version: 6, // cod_lnxded 1.5b getinfo and getstatus, 2026-10-13
        short_version: "1.5",
        own_master: None,
        max_configstrings: 2048,
        field_codec: FieldCodec::COD1,
        netchan: NetchanParams::COD1,
        paks: PakRules {
            // `%s/pak%x` for 0..0xf, 1.5 CoDMP.exe 0x43e310; `_svr_` at 0x43e490.
            id_pak_numbers: HEX_PAK_NUMBERS,
            id_pak_dirs: &["main"],
            id_pak_prefixes: &["pak", "mp_pak", "sp_pak"],
            skips_svr_paks: true,
        },
    },
    entity_fields: fields_v1::ENTITY_FIELDS,
    vehicle_fields: &[],
    vehicle_field_slots: &[],
    player_fields: fields_v6::PLAYER_FIELDS,
    client_fields: fields_v1::CLIENT_FIELDS,
    pm_flags_to_v1: flags::pm_flags_v6_to_v1,
    pm_flags_from_v1: flags::pm_flags_v1_to_v6,
    // cod_lnxded 1.5b map_restart, live (cod15-protocol-diff.md 8).
    map_restart_command: "B",
    eflags_to_v1: |f| f,
    eflags_from_v1: |f| f,
    event_from_v1: |e| e,
    max_weapons: 64,
    cursor_hints: CursorHints {
        health: 7,
        weapon: 9,
    },
    hud: HUD_V1,
    usercmd_weapon_bits: 6,
    cs: CsLayout {
        match_timeout: Some(1628),
        vote: 15,
        ..CS_LAYOUT_V1
    },
    et_events: 12,
    event_to_v1: |e| e,
    mod_to_v1: |m| m,
    pure_dlls: ["cgame_mp_x86.dll", "ui_mp_x86.dll"],
    team_colour_codes: true,
};

const HEX_PAK_NUMBERS: &[&str] = &[
    "0", "1", "2", "3", "4", "5", "6", "7", "8", "9", "a", "b", "c", "d", "e", "f",
];

/// United Offensive 1.51: 1.1's transport with grown and partly re-encoded
/// netfield tables, a second entity table for vehicles, 63 HUD elements,
/// rebased configstrings and renumbered events (docs/research/coduo-diff.md,
/// sections 3 to 8).
pub const PROTOCOL_V22: Cod1Wire = Cod1Wire {
    transport: Transport {
        game: Game::Cod1,
        version: 22, // coduo_lnxded 1.51b getinfo and getstatus, 2026-10-13
        short_version: "1.51",
        // UO servers list on a master of their own (CoDUOMP.exe, coduo_lnxded),
        // and on its own port of cod.pm's.
        own_master: Some(crate::net::master::UO_MASTER),
        max_configstrings: 2048, // CoDUOMP.exe 0x4014c6
        field_codec: FieldCodec::COD1,
        netchan: NetchanParams::COD1,
        paks: PakRules {
            // `%s/pak%x`, `%s/pakuo%x`, `%s/pakuo0%x`, `%s/mp_pak%x` (coduo_lnxded strings).
            id_pak_numbers: HEX_PAK_NUMBERS,
            id_pak_dirs: &["main", "uo"],
            id_pak_prefixes: &["pak", "pakuo", "pakuo0", "mp_pak"],
            skips_svr_paks: true,
        },
    },
    entity_fields: fields_v22::ENTITY_FIELDS,
    vehicle_fields: fields_v22::VEHICLE_FIELDS,
    vehicle_field_slots: &fields_v22::VEHICLE_FIELD_SLOTS,
    player_fields: fields_v22::PLAYER_FIELDS,
    client_fields: fields_v1::CLIENT_FIELDS,
    pm_flags_to_v1: flags::pm_flags_v22_to_v1,
    pm_flags_from_v1: flags::pm_flags_v1_to_v22,
    // UO's cgame takes 1.5's command set (coduo-diff.md 8).
    map_restart_command: "B",
    eflags_to_v1: flags::eflags_v22_to_v1,
    eflags_from_v1: flags::eflags_v1_to_v22,
    event_from_v1: event_ids::event_v1_to_v22,
    max_weapons: 128, // bg_itemlist, game.mp.uo.i386.so 0xa9880
    // game.mp.uo.i386.so 0x57add, 0x57b2f: `hintLMGMount` took 7.
    cursor_hints: CursorHints {
        health: 8,
        weapon: 10,
    },
    // coduo_lnxded 0x8082550: a 6-bit count, 30 fields (SCOORD, TCOORD).
    hud: HudWire {
        count_bits: 6,
        max_elems: 63,
        fields: 30,
    },
    usercmd_weapon_bits: 7, // coduo_lnxded 0x80814ce
    cs: CS_LAYOUT_V22,
    et_events: 16, // `G_TempEntity` stores `event + 0x10`
    event_to_v1: event_ids::event_v22_to_v1,
    mod_to_v1: mod_v22_to_v1,
    // Spelled obfuscated in both binaries: coduo_lnxded `SV_VerifyPaks_f`
    // 0x808cf23 and CoDUOMP.exe 0x42db37 (coduo-diff.md 23).
    pure_dlls: ["uo_cgame_mp_x86.dll", "uo_ui_mp_x86.dll"],
    team_colour_codes: true,
};

/// United Offensive's means of death as 1.1's: `modNames`
/// (game.mp.uo.i386.so 0xacf20) drops `KICKED`, `GRABBER`, `AIRSTRIKE`,
/// `SLIME` and `LAVA` and adds the artillery, tank and jeep crush,
/// collision, flame and binoculars kills, which have no 1.1 id
/// (docs/research/coduo-diff.md, section 12). The obituary switch in
/// `uo_cgame_mp_x86.dll` (0x3002231e) reads the same ids.
fn mod_v22_to_v1(m: i32) -> i32 {
    match m {
        0..=10 => m,
        11 | 12 => m + 2, // dynamite and its splash
        15 => 16,         // water
        16 => 19,         // crush
        19..=23 => m + 1, // telefrag, falling, suicide, trigger_hurt, explosive
        _ => -1,
    }
}

/// The protocol an install hosts: 22 when United Offensive's `uo/` paks are
/// mounted, 6 when its `main/pak0.pk3` is 1.5's, whose checksum differs
/// from 1.1's (docs/research/cod15-protocol-diff.md 10), 1 otherwise.
pub fn install_protocol(fs: &crate::pk3::Pk3Fs) -> &'static Cod1Wire {
    install_protocol_of(fs.paks())
}

/// [`install_protocol`] over a search path's paks before they are opened.
pub fn install_protocol_of<'a>(
    paks: impl IntoIterator<Item = &'a crate::pk3::PakInfo>,
) -> &'static Cod1Wire {
    /// `FS_LoadZipFile`'s checksum of the 1.5 install's `main/pak0.pk3`, as
    /// the 1.5b server's `sv_referencedPaks` lists it.
    const PAK0_V6_CHECKSUM: i32 = 1_048_127_331;
    let mut v6 = false;
    for p in paks {
        // A `uo/` with United Offensive's own paks beside `main/` (`--mod-dir
        // uo`, `pk3::base_dirs`) is UO's.
        if p.game == "uo" && p.name.starts_with("pakuo") {
            return &PROTOCOL_V22;
        }
        v6 |= p.game == "main" && p.name == "pak0" && p.checksum == PAK0_V6_CHECKSUM;
    }
    if v6 { &PROTOCOL_V6 } else { &PROTOCOL_V1 }
}

/// Model paths sit at `CS_MODELS_V1 + index`, index 1..=255 (`G_ModelIndex`,
/// game.mp.i386.so 0x66ed8).
pub const CS_MODELS_V1: usize = 268;

/// Tag names sit at `CS_TAGS_V1 + index`, index 1..=31 (`G_TagIndex`,
/// game.mp.i386.so 0x66fda).
pub const CS_TAGS_V1: usize = 108;

/// Global fog parameters, configstring 12. Written by the game dll's
/// `setCullFog` / `setExpFog` builtins; field order per RTCW-MP
/// cg_servercmds.c `CG_ParseFog` and two live captures
/// (docs/research/cod11-server-handshake.md, "Map-dependent, index < 140").
pub const CS_FOG_V1: usize = 12;

/// `setTeamScore("axis", ...)` writes here, "allies" to the next slot
/// (docs/research/cod11-hud-protocol.md 5, "Which team score is which").
pub const CS_AXIS_SCORE: usize = 5;
pub const CS_ALLIES_SCORE: usize = 6;
/// `northyaw`, the compass's north in world yaw degrees, copied verbatim by
/// `SP_worldspawn` (0x61cec).
pub const CS_NORTHYAW: usize = 11;
/// `level.startTime`, server-clock ms.
pub const CS_LEVEL_START_TIME: usize = 13;

/// The mirrored server cvars: names here, values in the next block at the
/// same offset; the client `Cvar_Set`s the pairs.
pub const CS_CVAR_NAMES: std::ops::RangeInclusive<usize> = 140..=203;
pub const CS_CVAR_VALUES: std::ops::RangeInclusive<usize> = 204..=267;

/// `CS_SOUNDS`: a sound-alias index (`EV_SOUND_ALIAS`'s parm, `loopSound`,
/// `s <idx>`) counts from here, so the alias range's first slot travels as 1
/// and 0 means no alias (docs/protocol-1.1.md, `s <idx>`).
pub const CS_SOUNDS: usize = CsRange::SoundAlias.bounds().0 - 1;
/// A hudelem's `text`/`label` index counts from here, 1-based like
/// `CS_SOUNDS`.
pub const CS_LOCALIZED: usize = CsRange::Localized.bounds().0 - 1;
/// A hudelem's `shaderIndex` and the objective icons count from here, 1-based.
pub const CS_SHADERS: usize = CsRange::Shader.bounds().0 - 1;
/// An effect id (`loadFx`'s return, `EV_PLAY_FX`'s parm, an `ET_FX`
/// entity's `scale`) counts from here, 1..63 (`CG_PlayFx`, cgame
/// 0x3001db20, docs/research/cod11-events-and-fx.md 5c).
pub const CS_EFFECTS: usize = CsRange::Effect.bounds().0 - 1;
/// `EV_PLAY_FX_ON_TAG`'s parm counts from here: configstring `844 + parm`
/// holds `"%02d%s"` of the effect id and the tag (cgame 0x3001db80).
pub const CS_FX_TAGS: usize = CsRange::FxTag.bounds().0 - 1;
/// The first of the 32 hint-string configstrings `G_GetHintStringIndex`
/// (0x5a238) fills; `serverCursorHintString` indexes from here.
pub const CS_HINT_STRINGS: usize = 1212;
/// 1.5's match timeout: the ms left (`%i`, empty once over) and the message
/// under it, written by `Cmd_MatchTimeout_f`, `Cmd_MatchTimein_f` and
/// `CheckMatchTimeout` (game.mp.i386.so 1.5b 0x4fea8). No 1.1 module writes
/// either (docs/research/cod15-protocol-diff.md, "Match timeout").
pub const CS_MATCH_TIMEOUT_V6: usize = 1628;
pub const CS_MATCH_TIMEOUT_TEXT_V6: usize = 1629;

/// Configstring block layout: docs/research/clientstate-wire-format.md,
/// "Configstring map".
pub const CS_LAYOUT_V1: CsLayout = CsLayout {
    ranges: [
        (21, 28),     // StatusIcon
        (29, 43),     // HeadIcon
        (109, 139),   // Tag
        (269, 523),   // Model
        (525, 779),   // SoundAlias
        (781, 843),   // Effect
        (845, 1099),  // FxTag
        (1101, 1115), // ShellShock
        (1180, 1211), // Menu
        (1245, 1499), // Localized
        (1501, 1627), // Shader
    ],
    cvar_names: (140, 203),
    cvar_values: 204,
    hint_strings: 1212,
    match_timeout: None,
    vote: 15,
    fog: CS_FOG_V1,
    level_start: CS_LEVEL_START_TIME,
    motd: 14, // SP_worldspawn 0x61dde (1.5b 0x66a3d)
    winner: 20,
};

/// United Offensive's rebased blocks, each read off its indexer in
/// `game.mp.uo.i386.so` (docs/research/coduo-diff.md, section 7).
pub const CS_LAYOUT_V22: CsLayout = CsLayout {
    ranges: [
        (22, 37),     // StatusIcon, i + 0x16
        (38, 52),     // HeadIcon, i + 0x26
        (118, 148),   // Tag, G_TagIndex 0x75
        (406, 660),   // Model, G_ModelIndex 0x195
        (662, 916),   // SoundAlias, G_SoundAliasIndex 0x295
        (918, 996),   // Effect, G_EffectIndex 0x395
        (998, 1252),  // FxTag, playFxOnTag 0x3e5
        (1254, 1268), // ShellShock, G_ShellShockIndex 0x4e5
        (1333, 1364), // Menu, i + 0x535
        (1398, 1652), // Localized, G_LocalizedStringIndex 0x575
        (1654, 1908), // Shader, G_ShaderIndex 0x675
    ],
    cvar_names: (149, 276),
    cvar_values: 277,
    hint_strings: 1365,        // trigger_use, i + 0x555
    match_timeout: Some(1909), // Cmd_MatchTimeout_f 0x775
    // UO put `setWind` at 12 (0x6ca27) and moved 12..20 up one: `G_setfog`
    // writes 13 (0x4a59a), `SP_worldspawn` the start time 14, `G_InitGame`
    // the winner 21 (coduo-diff.md 7).
    vote: 16,
    fog: 13,
    level_start: 14,
    motd: 15, // SP_worldspawn 0x72c38
    winner: 21,
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_version_comes_from_its_paks() {
        let pak = |game: &str, name: &str, checksum: i32| crate::pk3::PakInfo {
            path: Default::default(),
            game: game.into(),
            name: name.into(),
            localized: false,
            checksum,
            crcs: Vec::new(),
        };
        let v1 = [pak("main", "pak0", 7)];
        let v6 = [pak("main", "pak0", 1_048_127_331)];
        let uo = [pak("main", "pak0", 1_048_127_331), pak("uo", "pakuo00", 3)];
        assert!(install_protocol_of(&v1).is_v1());
        assert_eq!(
            install_protocol_of(&v6).transport.version,
            PROTOCOL_V6.transport.version
        );
        assert!(install_protocol_of(&uo).is_uo());
        assert!(!PROTOCOL_V6.is_uo() && !PROTOCOL_V6.is_v1());
    }

    #[test]
    fn games_and_masters() {
        assert_eq!(
            PROTOCOL_V1.transport.master(),
            crate::net::master::COD1_MASTER
        );
        assert_eq!(
            PROTOCOL_V6.transport.master(),
            PROTOCOL_V1.transport.master()
        );
        assert_eq!(
            PROTOCOL_V22.transport.master(),
            crate::net::master::UO_MASTER
        );
        assert!(PROTOCOL_V1.transport.same_game(&PROTOCOL_V6.transport));
        assert!(!PROTOCOL_V1.transport.same_game(&PROTOCOL_V22.transport));
        assert!(!PROTOCOL_V6.transport.same_game(&PROTOCOL_V22.transport));
        assert!(Cod1Wire::ALL.iter().all(|p| p.transport.game == Game::Cod1));
    }

    #[test]
    fn range_bases_sit_one_below_their_ranges() {
        assert_eq!((CS_SOUNDS, CS_LOCALIZED, CS_SHADERS), (524, 1244, 1500));
    }

    #[test]
    fn v1_tables_look_sane() {
        let p = &PROTOCOL_V1;
        assert_eq!(p.entity_fields.len(), 59);
        assert_eq!(p.player_fields.len(), 103);
        // Array order is the wire order; pin both ends against a reordering
        // regeneration.
        assert_eq!(p.entity_fields[0].name, "pos.trTime");
        assert_eq!(p.entity_fields[58].name, "dmgFlags");
        assert_eq!(p.player_fields[0].name, "commandTime");
        assert_eq!(p.player_fields[102].name, "gunfx");
        assert_eq!(p.client_fields.len(), 22);
        assert_eq!(p.client_fields[0].name, "team");
        assert_eq!(p.client_fields[21].name, "name[28]");
        for f in p
            .entity_fields
            .iter()
            .chain(p.player_fields)
            .chain(p.client_fields)
        {
            assert!((-32..=32).contains(&f.bits), "{} bits {}", f.name, f.bits);
        }
        // Offsets identify fields, so a duplicate means a mis-parsed table.
        for table in [p.entity_fields, p.player_fields, p.client_fields] {
            let mut offsets: Vec<u16> = table.iter().map(|f| f.offset).collect();
            offsets.sort_unstable();
            let len = offsets.len();
            offsets.dedup();
            assert_eq!(offsets.len(), len, "duplicate offset in field table");
        }
    }

    #[test]
    fn v6_is_v1_with_a_wider_pm_flags() {
        let (v1, v6) = (&PROTOCOL_V1, &PROTOCOL_V6);
        assert!(std::ptr::eq(v1.entity_fields, v6.entity_fields));
        assert!(std::ptr::eq(v1.client_fields, v6.client_fields));
        assert_eq!(v1.player_fields.len(), v6.player_fields.len());
        for (i, (a, b)) in v1.player_fields.iter().zip(v6.player_fields).enumerate() {
            assert_eq!((a.name, a.offset), (b.name, b.offset), "entry {i}");
            if a.name == "pm_flags" {
                assert_eq!((i, a.bits, b.bits), (12, 19, 21));
            } else {
                assert_eq!(a.bits, b.bits, "{}", a.name);
            }
        }
    }

    #[test]
    fn protocols_by_version_and_short_version() {
        assert_eq!(
            Cod1Wire::by_version(Game::Cod1, 1).map(|p| p.transport.short_version),
            Some("1.1")
        );
        assert_eq!(
            Cod1Wire::by_version(Game::Cod1, 6).map(|p| p.transport.short_version),
            Some("1.5")
        );
        assert!(Cod1Wire::by_version(Game::Cod1, 5).is_none());
        assert!(Cod1Wire::by_version(Game::Cod2, 1).is_none());
        assert_eq!(
            Cod1Wire::by_short_version("1.5").map(|p| p.transport.version),
            Some(6)
        );
        assert_eq!(
            Cod1Wire::by_short_version("1.51").map(|p| p.transport.version),
            Some(22)
        );
        assert!(Cod1Wire::by_short_version("1.4").is_none());
    }

    #[test]
    fn v22_tables_look_sane() {
        let p = &PROTOCOL_V22;
        assert_eq!(p.entity_fields.len(), 60);
        assert_eq!(p.vehicle_fields.len(), 60);
        assert_eq!(p.player_fields.len(), 114);
        assert_eq!(p.entity_fields[0].name, "pos.trTime");
        assert_eq!(p.vehicle_fields[0].name, "pos.trBase[2]");
        assert_eq!(p.player_fields[113].name, "weaponrechamber[3]");
        // `pm_flags` keeps its slot, which the reader's map keys on.
        assert_eq!(p.player_fields[12].name, "pm_flags");
        // The vehicle table is a permutation of the main one.
        let mut slots: Vec<u8> = p.vehicle_field_slots.to_vec();
        for (k, &i) in p.vehicle_field_slots.iter().enumerate() {
            let (a, b) = (&p.vehicle_fields[k], &p.entity_fields[usize::from(i)]);
            assert_eq!((a.name, a.offset), (b.name, b.offset));
        }
        slots.sort_unstable();
        assert_eq!(slots, (0..60).collect::<Vec<u8>>());
        // The names consumers look up are there.
        for name in [
            "eType",
            "eFlags",
            "_union.scale",
            "xmodel",
            "weapon",
            "events[3]",
        ] {
            assert!(p.entity_fields.iter().any(|f| f.name == name), "{name}");
        }
        for name in ["fJumpPeak", "viewangles[0]", "delta_angles[1]", "clientNum"] {
            assert!(p.player_fields.iter().any(|f| f.name == name), "{name}");
        }
    }

    #[test]
    fn v22_means_of_death() {
        let map = PROTOCOL_V22.mod_to_v1;
        // Headshot and melee keep their ids; drown, crush, falling and
        // suicide moved; tank crush and the artillery have none on 1.1.
        for (uo, v1) in [
            (7, 7),
            (8, 8),
            (15, 16),
            (16, 19),
            (20, 21),
            (21, 22),
            (17, -1),
            (13, -1),
        ] {
            assert_eq!(map(uo), v1, "UO MOD {uo}");
        }
    }

    #[test]
    fn v22_configstring_blocks() {
        let cs = &PROTOCOL_V22.cs;
        // The live mp_cassino gamestate: a Sherman's `xmodel` 36 is
        // configstring 441, and the first script menu sits at 1333.
        assert_eq!(cs.slot(CsRange::Model, 36), Some(441));
        assert_eq!(cs.bounds(CsRange::Menu).0, 1333);
        assert_eq!(cs.slot(CsRange::Effect, 80), None);
        assert_eq!(PROTOCOL_V1.cs.slot(CsRange::Model, 1), Some(269));
        // Blocks never overlap.
        let mut r = cs.ranges.to_vec();
        r.sort_unstable();
        assert!(r.windows(2).all(|w| w[0].1 < w[1].0), "{r:?}");
    }
}
