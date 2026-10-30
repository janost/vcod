//! What every game's wire shares: the version and master a connection
//! speaks to, the codec and netchan framing, and the stock pak rules
//! ([`Transport`]), and the [`WireGame`] trait each game's descriptor
//! implements ([`crate::net::cod1::Cod1Wire`] for CoD1, `vcod_cod2`'s
//! `Cod2Wire` for CoD2). The netchan, the master query, the pak lists and
//! the browser read only the transport.

use super::codec::{FieldCodec, MsgWriter};
use super::netchan::NetchanParams;
use super::protocol::CsLayout;
use crate::game::Game;

/// One game's wire descriptor: its [`Transport`], its configstring layout,
/// and the types its snapshots and usercmds decode into. Each game keeps
/// its own numbering in those types.
pub trait WireGame: Sync + 'static {
    /// The client's move command.
    type UserCmd;
    /// An entity in a snapshot.
    type EntityState;
    /// The playerstate a snapshot carries.
    type PlayerState;
    /// One snapshot frame, the unit a delta is taken against.
    type Frame;
    /// What `SV_SendClientGameState` sends: the configstrings, the
    /// baselines, the client's number and the checksum feed.
    type Gamestate;
    /// The entity baselines a snapshot deltas a new entity from.
    type Baselines;

    fn transport(&self) -> &Transport;
    fn cs(&self) -> &CsLayout;

    /// Which wire this is: the game and its version, never the version
    /// alone (CoD4 1.7 and CoD1 1.5 both speak protocol 6).
    fn id(&self) -> WireId {
        self.transport().id()
    }

    /// A connectionless packet: `ff ff ff ff` and `text` in the game's
    /// encoding.
    fn oob_packet(&self, text: &str) -> Vec<u8>;

    /// `svc_gamestate` to its `svc_EOF`. The caller writes the
    /// `reliableAcknowledge` and the pending server commands ahead of it.
    fn write_gamestate(&self, w: &mut MsgWriter, gs: &Self::Gamestate);

    /// `SV_WriteSnapshotToClient` from `svc_snapshot` on: `frame` deltaed
    /// from `base`, which went out `head.delta_num` messages back (`None`
    /// for a full frame); an entity new to the frame deltas from its
    /// baseline.
    fn write_snapshot(
        &self,
        w: &mut MsgWriter,
        base: Option<&Self::Frame>,
        frame: &Self::Frame,
        head: SnapshotHead,
        baselines: &Self::Baselines,
    );
}

/// A wire's identity: game and version together. Anything keyed on a wire
/// keys on this, not on [`Transport::version`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WireId {
    pub game: Game,
    pub version: u32,
}

/// The snapshot header's bytes beside the server time: the offset back to
/// the base frame (0 for a full one) and `snapFlags`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SnapshotHead {
    pub delta_num: u8,
    pub snap_flags: u32,
}

pub struct Transport {
    /// The game this protocol belongs to: its master server and which other
    /// protocols a client may fall back to ([`Self::same_game`]).
    pub game: Game,
    pub version: u32,
    /// The server's `shortversion`, which a connect refused for the wrong
    /// protocol names (`EXE_SERVER_IS_DIFFERENT_VER\x15<this>`).
    pub short_version: &'static str,
    /// An expansion that lists its servers on a master of its own; `None`
    /// takes the game's ([`Self::master`]).
    pub own_master: Option<crate::net::master::Master>,
    pub max_configstrings: usize,
    /// How the field tables' negative widths read.
    pub field_codec: FieldCodec,
    /// Fragment framing, message cap and command ring size.
    pub netchan: NetchanParams,
    pub paks: PakRules,
}

/// The stock paks `FS_ComparePaks` never fetches
/// ([`crate::net::download::is_stock_pak`]).
pub struct PakRules {
    /// `FS_idPak`'s pak numbers, as its format prints them: `main/pak<n>`,
    /// `mp_pak<n>`, `sp_pak<n>` and `localized_*_pak<n>`.
    pub id_pak_numbers: &'static [&'static str],
    /// The directories and name prefixes of those stock paks.
    pub id_pak_dirs: &'static [&'static str],
    pub id_pak_prefixes: &'static [&'static str],
    /// 1.5's `FS_ComparePaks` also skips a pak whose name holds `_svr_`.
    pub skips_svr_paks: bool,
}

impl Transport {
    /// This wire's [`WireId`].
    pub const fn id(&self) -> WireId {
        WireId {
            game: self.game,
            version: self.version,
        }
    }

    /// The master server the browser asks and a heartbeat goes to.
    pub fn master(&self) -> crate::net::master::Master {
        self.own_master.unwrap_or(self.game.master())
    }

    /// Whether a client on one's install can play the other's server: one
    /// game, listed on one master. 1.1 and 1.5 share `pak1`-`pak4` and cross
    /// over; United Offensive's `uo/` paks override `main/`'s weapons and
    /// menus and its servers list apart, so it plays only itself.
    pub fn same_game(&self, other: &Transport) -> bool {
        self.game == other.game && self.master() == other.master()
    }
}
