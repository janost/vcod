//! CoD2 1.3's wire descriptor [`Cod2Wire`], protocol 118
//! (docs/research/cod2-protocol.md; the per-field choices are section 13):
//! its [`Transport`] (version, codec, netchan, the stock iwd names), the
//! netfield tables, the configstring layout, `ET_EVENTS` and the HUD
//! block. CoD2 keeps its own numbering ([`super::events`],
//! [`super::flags`]). [`Cod2Wire::as_cod1_shim`] is the same wire as a
//! CoD1 descriptor with identity maps, for the server code that still
//! takes a `Cod1Wire`: `Defs::COD2` and the `EntityState` CoD2's entities
//! sit in, built in CoD2's numbers, until the engine takes `EntityState2`.

use vcod_common::game::Game;
use vcod_common::net::codec::MsgWriter;
use vcod_common::net::protocol::{
    Cod1Wire, CsLayout, CsRange, CursorHints, FieldCodec, HudWire, NetField, NetchanParams,
    PakRules, SnapshotHead, Transport, WireGame,
};

use super::delta::{HUD_FIELDS, MAX_HUDELEMS};
use super::fields::{CLIENT_STATE_FIELDS, ENTITY_STATE_FIELDS, PLAYER_STATE_FIELDS};
use super::ops::{MAX_CONFIGSTRINGS, PROTOCOL};

/// The configstring blocks of section 6. A block `G_FindConfigstringIndex`
/// or `G_ModelIndex` fills starts one past its base (models at 335, live);
/// the ones the precache builtins fill start at the base (status icons at
/// 23, menus at 1246, live).
pub const CS_LAYOUT_118: CsLayout = CsLayout {
    ranges: [
        (23, 30),     // StatusIcon, GScr_PrecacheStatusIcon 0x81134dc
        (31, 45),     // HeadIcon, GScr_PrecacheHeadIcon 0x811367a
        (111, 141),   // Tag, 0x811d5cc
        (335, 589),   // Model, G_ModelIndex 0x811d49c
        (591, 845),   // SoundAlias, G_SoundAliasIndex 0x811d668
        (847, 909),   // Effect, G_EffectIndex 0x811d600
        (911, 1165),  // FxTag, Scr_PlayFXOnTag 0x81152d6
        (1167, 1181), // ShellShock, G_ShellShockIndex 0x811d634
        (1246, 1277), // Menu, GScr_PrecacheMenu 0x811334a
        (1311, 1565), // Localized, G_LocalizedStringIndex 0x811d3fe
        (1567, 1693), // Shader, G_ShaderIndex 0x811d44a
    ],
    cvar_names: (142, 237),
    cvar_values: 238,
    hint_strings: 1278,
    match_timeout: None,
    vote: 15,
    fog: 12,
    level_start: 13,
    motd: 14,
    winner: 22,
};

/// The axis and allies team scores (section 6).
pub const CS_AXIS_SCORE: usize = 5;
pub const CS_ALLIES_SCORE: usize = 6;
/// Weapon names, space-separated, 1-based: `entityState.weapon` indexes it.
pub const CS_WEAPONS: usize = 7;
pub const CS_NORTHYAW: usize = 11;

/// What every game's wire shares, CoD2's: version, codec, netchan, the
/// stock iwd names.
const TRANSPORT_118: Transport = Transport {
    game: Game::Cod2,
    version: PROTOCOL as u32,
    // `EXE_SERVER_IS_DIFFERENT_VER\x15%s` with `1.3` (0x8149eb7).
    short_version: "1.3",
    own_master: None,
    max_configstrings: MAX_CONFIGSTRINGS,
    field_codec: FieldCodec::COD2,
    netchan: NetchanParams::COD2,
    paks: PakRules {
        // FS_iwIwd 0x8065324: `%s/iw_%02d` for 0..24, and `localized_*`.
        id_pak_numbers: &[
            "00", "01", "02", "03", "04", "05", "06", "07", "08", "09", "10", "11", "12", "13",
            "14", "15", "16", "17", "18", "19", "20", "21", "22", "23", "24",
        ],
        id_pak_dirs: &["main"],
        id_pak_prefixes: &["iw_"],
        skips_svr_paks: false,
    },
};

const HUD_118: HudWire = HudWire {
    count_bits: 5,
    max_elems: MAX_HUDELEMS,
    fields: HUD_FIELDS,
};

/// CoD2's wire descriptor. Its tables and numbers are CoD2's own
/// ([`super::events`], [`super::flags`]); nothing here maps them to 1.1's.
pub struct Cod2Wire {
    pub transport: Transport,
    pub entity_fields: &'static [NetField],
    pub player_fields: &'static [NetField],
    pub client_fields: &'static [NetField],
    /// Where the configstring blocks sit.
    pub cs: CsLayout,
    /// `ET_EVENTS`: an `eType` at or above it is a temp event entity
    /// carrying event `eType - et_events`.
    pub et_events: i32,
    pub hud: HudWire,
    /// What a `map_restart` that drops `pers[]` sends a client.
    pub map_restart_command: &'static str,
    /// The usercmd's `weapon` width.
    pub usercmd_weapon_bits: i32,
    /// Weapon rows `bg_itemlist` leaves room for.
    pub max_weapons: usize,
}

/// Protocol 118, `pc_1.3_1_1`.
pub const PROTOCOL_118: Cod2Wire = Cod2Wire {
    transport: TRANSPORT_118,
    entity_fields: &ENTITY_STATE_FIELDS,
    player_fields: &PLAYER_STATE_FIELDS,
    client_fields: &CLIENT_STATE_FIELDS,
    cs: CS_LAYOUT_118,
    et_events: super::events::ET_EVENTS,
    hud: HUD_118,
    // SV_MapRestart 0x808be3e: `B` when `G_GetSavePersist` is 0, else `n`.
    map_restart_command: "B",
    usercmd_weapon_bits: 7,
    // INFERRED: the 7-bit `weapon` fields and CS 8's 33 hex digits (132
    // items) leave room for 128 weapon rows.
    max_weapons: 128,
};

/// [`PROTOCOL_118`] dressed as a CoD1 descriptor, for the server code that
/// still takes a `Cod1Wire` (vcod-server's `Defs::COD2`, and the
/// `EntityState` its CoD2 entities, built in CoD2's numbers, sit in until
/// the engine takes `EntityState2`): 118's tables and layout, identity
/// numbering maps,
/// and nothing for what only CoD1 reads (cursor hints, `pure_dlls`,
/// vehicle tables, `^8`/`^9`).
static COD1_SHIM_118: Cod1Wire = Cod1Wire {
    transport: TRANSPORT_118,
    entity_fields: PROTOCOL_118.entity_fields,
    vehicle_fields: &[],
    vehicle_field_slots: &[],
    player_fields: PROTOCOL_118.player_fields,
    client_fields: PROTOCOL_118.client_fields,
    pm_flags_to_v1: |f| f,
    pm_flags_from_v1: |f| f,
    map_restart_command: PROTOCOL_118.map_restart_command,
    eflags_to_v1: |f| f,
    eflags_from_v1: |f| f,
    event_from_v1: |e| e,
    max_weapons: PROTOCOL_118.max_weapons,
    cursor_hints: CursorHints {
        health: 0,
        weapon: 0,
    },
    hud: HUD_118,
    usercmd_weapon_bits: PROTOCOL_118.usercmd_weapon_bits,
    cs: CS_LAYOUT_118,
    et_events: PROTOCOL_118.et_events,
    event_to_v1: |e| e,
    mod_to_v1: |m| m,
    // One executable: no client game or UI module opens a `cp`.
    pure_dlls: ["", ""],
    team_colour_codes: false,
};

impl Cod2Wire {
    /// The event an `eType` carries, `None` for an ordinary entity.
    pub fn event_of(&self, etype: i32) -> Option<i32> {
        (etype >= self.et_events).then(|| etype - self.et_events)
    }

    /// This descriptor as a [`Cod1Wire`] with identity maps
    /// ([`COD1_SHIM_118`]): for code that holds CoD2 entities in CoD1's
    /// `EntityState` over 118's tables. Drop a caller once it builds
    /// [`super::EntityState2`] itself.
    pub const fn as_cod1_shim(&self) -> &'static Cod1Wire {
        &COD1_SHIM_118
    }
}

impl WireGame for Cod2Wire {
    type UserCmd = super::UserCmd2;
    type EntityState = super::EntityState2;
    type PlayerState = super::PlayerState2;
    type Frame = super::write::Frame2;
    type Gamestate = super::Gamestate2;
    type Baselines = std::collections::BTreeMap<u32, super::EntityState2>;

    fn transport(&self) -> &Transport {
        &self.transport
    }

    fn cs(&self) -> &CsLayout {
        &self.cs
    }

    /// Latin-1, a byte a char.
    fn oob_packet(&self, text: &str) -> Vec<u8> {
        super::oob::packet(text)
    }

    fn write_gamestate(&self, w: &mut MsgWriter, gs: &Self::Gamestate) {
        super::write::write_gamestate(w, gs);
    }

    /// CoD2's `snapFlags` is a byte; no `sv_padPackets` nops.
    fn write_snapshot(
        &self,
        w: &mut MsgWriter,
        base: Option<&Self::Frame>,
        frame: &Self::Frame,
        head: SnapshotHead,
        baselines: &Self::Baselines,
    ) {
        super::write::write_snapshot(
            w,
            base,
            head.delta_num,
            head.snap_flags as u8,
            frame,
            |n| baselines.get(&n).copied(),
            0,
        );
    }
}

/// The models' configstring for a 1-based `G_ModelIndex` index.
pub fn model_slot(index: i32) -> Option<usize> {
    CS_LAYOUT_118.slot(CsRange::Model, index)
}

/// The materials' configstring for a 1-based `G_ShaderIndex` index.
pub fn shader_slot(index: i32) -> Option<usize> {
    CS_LAYOUT_118.slot(CsRange::Shader, index)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_matches_the_live_gamestate() {
        // cod2-protocol.md 9: the first model at 335, the first material at 1567.
        assert_eq!(model_slot(1), Some(335));
        assert_eq!(model_slot(255), Some(589));
        assert_eq!(model_slot(256), None);
        assert_eq!(shader_slot(1), Some(1567));
        assert_eq!(CS_LAYOUT_118.bounds(CsRange::Menu).0, 1246);
        assert_eq!(PROTOCOL_118.event_of(10 + 194), Some(194));
        assert_eq!(PROTOCOL_118.entity_fields.len(), 59);
        assert_eq!(PROTOCOL_118.player_fields.len(), 105);
        let shim = PROTOCOL_118.as_cod1_shim();
        // The tables are consts, so compare their rows, not their addresses.
        let rows =
            |t: &[NetField]| -> Vec<_> { t.iter().map(|f| (f.name, f.offset, f.bits)).collect() };
        assert_eq!(rows(shim.entity_fields), rows(PROTOCOL_118.entity_fields));
        assert_eq!(rows(shim.player_fields), rows(PROTOCOL_118.player_fields));
        assert_eq!(shim.event_of(10 + 194), Some(194));
        assert_eq!((shim.eflags_to_v1)(0x40), 0x40);
    }
}
