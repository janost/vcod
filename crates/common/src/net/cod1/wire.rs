//! [`Cod1Wire`], CoD1's wire descriptor: [`Transport`] plus the netfield
//! tables, the configstring layout and the maps between each protocol's
//! numbering and 1.1's.

use super::{PROTOCOL_V1, PROTOCOL_V6, PROTOCOL_V22};
use crate::game::Game;
use crate::net::codec::NetField;
use crate::net::msg::MsgWriter;
use crate::net::protocol::{CsLayout, HudWire};
use crate::net::wire::{SnapshotHead, Transport, WireGame};

/// `G_CheckForCursorHints`' item values: health's, and an unowned
/// weapon's base (`base + giTag`); an owned weapon or an ammo row adds
/// `max_weapons` more (cod11-items.md 2.3, coduo-diff.md 12).
pub struct CursorHints {
    pub health: i32,
    pub weapon: i32,
}

/// CoD1's wire descriptor: patch 1.1 (protocol 1), 1.5 (6) and United
/// Offensive 1.51 (22). A connection holds one; everything that decodes
/// reads it from there. Its tables carry each protocol's numbering, and the
/// `*_to_v1` / `*_from_v1` maps rewrite it to and from 1.1's, the one the
/// rest of vcod reads.
pub struct Cod1Wire {
    /// What every game's wire shares: version, master, codec, netchan, paks.
    pub transport: Transport,
    pub entity_fields: &'static [NetField],
    /// Protocol 22's second entity table: an entity whose `eType` is
    /// [`crate::net::flags::ET_VEHICLE_V22`] travels in it, behind a select
    /// bit after its number (coduo-diff.md, section 3). Empty where the
    /// protocol has no select bit.
    pub vehicle_fields: &'static [NetField],
    /// Each `vehicle_fields` entry's index into `entity_fields`; a decoded
    /// entity keeps `entity_fields` order whichever table carried it.
    pub vehicle_field_slots: &'static [u8],
    pub player_fields: &'static [NetField],
    pub client_fields: &'static [NetField],
    /// Rewrites a decoded `pm_flags` into 1.1's bit layout, the one the rest
    /// of vcod reads ([`crate::net::flags`]).
    pub pm_flags_to_v1: fn(i32) -> i32,
    /// The writer's inverse: vcod's `pm_flags` in this protocol's layout.
    pub pm_flags_from_v1: fn(i32) -> i32,
    /// What a `map_restart` that drops `pers[]` sends a client (1.1 `n`, 1.5
    /// and UO `B`); one that keeps it sends `n` on all three.
    pub map_restart_command: &'static str,
    /// The same for an entity's or the playerState's `eFlags`.
    pub eflags_to_v1: fn(i32) -> i32,
    /// A server's inverses of `eflags_to_v1` and `event_to_v1`: vcod's
    /// (1.1's) value in this protocol's layout. The snapshot writer leaves
    /// both alone; the server maps what it builds.
    pub eflags_from_v1: fn(i32) -> i32,
    pub event_from_v1: fn(i32) -> i32,
    /// `bg_itemlist`'s weapon rows: a weapon is item 1..=`max_weapons`, its
    /// configstring 7 index, and the compiled-in ammo and health rows
    /// follow (1.1 64, UO 128; coduo-diff.md 12).
    pub max_weapons: usize,
    pub cursor_hints: CursorHints,
    pub hud: HudWire,
    /// The usercmd's full-branch `weapon` width.
    pub usercmd_weapon_bits: i32,
    /// Where the configstring blocks sit.
    pub cs: CsLayout,
    /// `ET_EVENTS`: an `eType` at or above it is a temp event entity
    /// carrying event `eType - et_events`.
    pub et_events: i32,
    /// Maps one of this protocol's `EV_*` ids to vcod's (1.1's,
    /// [`crate::net::event_ids`]); an id with no 1.1 counterpart comes back
    /// as [`crate::net::event_ids::EV_FOREIGN`] `| id`.
    pub event_to_v1: fn(i32) -> i32,
    /// The same for a means of death (`EV_OBITUARY`'s parm with bit 7 set);
    /// one with no 1.1 counterpart comes back -1.
    pub mod_to_v1: fn(i32) -> i32,
    /// The client game and UI modules whose paks open a pure `cp`
    /// (`CL_SendPureChecksums`, `SV_VerifyPaks_f`).
    pub pure_dlls: [&'static str; 2],
    /// The client's text renderer takes `^8` and `^9` as colour codes, in
    /// `g_TeamColor_Allies` and `g_TeamColor_Axis` (CoDMP.exe 1.5
    /// 0x4ba8e0, CoDUOMP.exe 0x4c02f0); 1.1's prints them
    /// (cod11-hud-protocol.md 7).
    pub team_colour_codes: bool,
}

impl Cod1Wire {
    /// Every protocol vcod speaks, the one it tries first leading.
    pub const ALL: [&'static Cod1Wire; 3] = [&PROTOCOL_V1, &PROTOCOL_V6, &PROTOCOL_V22];

    /// The event an `eType` carries, `None` for an ordinary entity.
    pub fn event_of(&self, etype: i32) -> Option<i32> {
        (etype >= self.et_events).then(|| (self.event_to_v1)(etype - self.et_events))
    }

    /// The protocol of `game` an `infoResponse`'s `protocol` key names. The
    /// game keys it: another game's protocol numbers may collide with these.
    pub fn by_version(game: Game, version: u32) -> Option<&'static Cod1Wire> {
        Self::ALL
            .into_iter()
            .find(|p| p.transport.game == game && p.transport.version == version)
    }

    /// The protocol a server's `shortversion` belongs to.
    pub fn by_short_version(v: &str) -> Option<&'static Cod1Wire> {
        Self::ALL
            .into_iter()
            .find(|p| p.transport.short_version == v)
    }

    /// United Offensive 1.51's protocol 22. Every UO-only rule keys on this.
    pub fn is_uo(&self) -> bool {
        self.transport.version == PROTOCOL_V22.transport.version
    }

    /// Patch 1.1's protocol 1.
    pub fn is_v1(&self) -> bool {
        self.transport.version == PROTOCOL_V1.transport.version
    }
}

impl WireGame for Cod1Wire {
    type UserCmd = crate::net::msg::UserCmd;
    type EntityState = crate::net::msg::EntityState;
    type PlayerState = crate::net::msg::PlayerState;
    type Frame = crate::net::snapshot::Snapshot;
    type Gamestate = crate::net::gamestate::Gamestate;
    type Baselines = std::collections::HashMap<u32, crate::net::msg::EntityState>;

    fn transport(&self) -> &Transport {
        &self.transport
    }

    fn cs(&self) -> &CsLayout {
        &self.cs
    }

    fn oob_packet(&self, text: &str) -> Vec<u8> {
        crate::net::connectionless::build_oob(text)
    }

    fn write_gamestate(&self, w: &mut MsgWriter, gs: &Self::Gamestate) {
        crate::net::gamestate::write(w, self, gs);
    }

    /// A CoD1 frame carries its own header: `head` is what its message
    /// numbers and `snap_flags` already say.
    fn write_snapshot(
        &self,
        w: &mut MsgWriter,
        base: Option<&Self::Frame>,
        frame: &Self::Frame,
        head: SnapshotHead,
        baselines: &Self::Baselines,
    ) {
        debug_assert_eq!(head, frame.head(base));
        w.write_byte(crate::net::snapshot::SVC_SNAPSHOT);
        crate::net::snapshot::write(w, self, base, frame, baselines);
    }
}
