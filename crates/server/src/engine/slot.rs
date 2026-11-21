//! A client slot's connection half, the `client_t` fields the servers
//! keep under the same rules: the netchan and the reliable rings' acks,
//! the frames a delta is taken from, the ping ring, the message pacing and
//! its fragments, and the timeout clock ([`Link`]). Each game's client
//! embeds one as `link`. CoD2 and CoD4 share the rest of the slot too
//! ([`Conn`]): the state machine, the gamestate and delta marks, the pure
//! verdict, the rates and the voice; CoD1's client keeps its own.

use std::collections::VecDeque;
use std::net::SocketAddr;
use vcod_common::net::netchan::{NetchanParams, ServerNetchan};

use super::link::Rates;

/// `PACKET_BACKUP`, the same 32 on every game: the frame and ping rings.
pub const PACKET_BACKUP: usize = 32;

/// `F` is the game's sent frame, filed by message number.
pub struct Link<F> {
    pub addr: SocketAddr,
    pub netchan: ServerNetchan,
    pub userinfo: String,
    /// `lastClientCommand`; goes back out as every message's `reliableAcknowledge`.
    pub last_client_command: i32,
    /// The client's `reliableAcknowledge`, what it has seen of our server commands.
    pub reliable_ack: i32,
    /// `reliableSent`: the last server command a message has carried. Only
    /// the ones past it can still be squashed.
    pub reliable_sent: i32,
    /// `cl->nextReliableTime`: the end of the flood-protection window the
    /// last non-exempt client command opened.
    pub next_reliable_ms: i32,
    /// Frames sent to this client, by message number `% PACKET_BACKUP`;
    /// the delta base for a later frame is picked from here.
    pub frames: Vec<Option<F>>,
    /// `frames[].messageSent` and `.messageAcked` by `outgoingSequence &
    /// 31`: the server clock when the message went out (CoD2's frame
    /// clock), and when the last move message acking it arrived, -1 until
    /// then.
    pub ping_ring: [(i32, i32); PACKET_BACKUP],
    /// `cl->ping`, `SV_CalcPings`' average over [`Self::ping_ring`]; 999
    /// until in the world.
    pub ping: i32,
    /// `nextSnapshotTime`: no message goes out before this server time.
    pub next_message_ms: i32,
    /// `netchan.unsentFragments`: the rest of a fragmented message, one
    /// packet per frame turn.
    pub unsent: VecDeque<Vec<u8>>,
    /// `cl->rateDelayed`: the last off-LAN message's rate interval reached
    /// the snapshot interval; the next snapshot carries it in `snapFlags`.
    pub rate_delayed: bool,
    /// `lastPacketTime` on the server clock: on CoD1 `svs.time` between
    /// frames, the last frame's, when the packet was taken. What frees a
    /// zombie.
    pub last_packet_ms: i32,
    /// `timeoutCount`: consecutive frames past `sv_timeout`; the sixth
    /// drops the client (`SV_CheckTimeouts`, cod_lnxded 0x808cbc0).
    pub timeout_count: u32,
    /// The word at `cl+4` that `SV_WriteSnapshotToClient` (cod_lnxded
    /// 0x808dee0) sets on an active client's snapshot and a zombie keeps: a
    /// zombie never sent one carries `SNAPFLAG_NOT_ACTIVE`
    /// (docs/protocol-1.1.md, snapFlags).
    pub was_active: bool,
}

impl<F> Link<F> {
    /// A fresh connection: nothing acked or sent, the rings empty, every
    /// clock at 0 (a caller on another clock sets its own).
    pub fn new(addr: SocketAddr, netchan: ServerNetchan, userinfo: String) -> Self {
        Link {
            addr,
            netchan,
            userinfo,
            last_client_command: 0,
            reliable_ack: 0,
            reliable_sent: 0,
            next_reliable_ms: 0,
            frames: Self::no_frames(),
            ping_ring: [(0, 0); PACKET_BACKUP],
            ping: 999,
            next_message_ms: 0,
            unsent: VecDeque::new(),
            rate_delayed: false,
            last_packet_ms: 0,
            timeout_count: 0,
            was_active: false,
        }
    }

    /// An empty frame ring.
    pub fn no_frames() -> Vec<Option<F>> {
        (0..PACKET_BACKUP).map(|_| None).collect()
    }

    /// Whether a client message's `reliableAcknowledge` is a ring or more
    /// behind `reliable_sequence`: retail then takes every command as
    /// acknowledged and drops the message (cod2-protocol.md 3.4,
    /// "Acknowledge"). An ack past the sequence passes, as retail's signed
    /// test lets it. Taken in i64, unlike retail's i32, so an ack near
    /// `i32::MIN` counts as behind instead of wrapping into the window and
    /// sending `write_pending_commands` walking 2^31 commands (SEC-001).
    pub fn reliable_ack_behind(&self, ack: i32) -> bool {
        let seq = i64::from(self.netchan.reliable_sequence);
        seq - i64::from(ack) >= self.netchan.reliable.len() as i64
    }
}

/// How long a CoD1 or CoD2 slot that has not had one netchan message
/// taken keeps it, seconds, when `sv_timeout` is longer: a client sends
/// its first one as soon as `connectResponse` lands, so only one that died
/// or a flood that never answers waits this out. vcod's (SEC-066); retail holds
/// such a slot the whole `sv_timeout`, and CoD4's `sv_connectTimeout`
/// already covers it there.
pub const NO_NETCHAN_TIMEOUT_S: i32 = 15;

/// The timeout, in ms, `SV_CheckTimeouts` holds a CoD1 or CoD2 slot to:
/// `sv_timeout`, capped at [`NO_NETCHAN_TIMEOUT_S`] until a netchan
/// message from the client has been taken.
pub fn timeout_ms<F>(link: &Link<F>, sv_timeout_s: i32) -> i32 {
    let s = if link.netchan.incoming_sequence == 0 {
        sv_timeout_s.min(NO_NETCHAN_TIMEOUT_S)
    } else {
        sv_timeout_s
    };
    s.wrapping_mul(1000)
}

/// `MAX_RELIABLE_COMMANDS` of CoD2's and CoD4's netchan: the reliable
/// rings' length.
pub const RELIABLE_COMMANDS: usize = NetchanParams::COD2.reliable_commands;

/// A CoD2 or CoD4 slot's `clientState_t`, in order: a zombie is below
/// every live state.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum State {
    /// `CS_ZOMBIE`: dropped, still sent its last commands until
    /// `sv_zombietime` passes without a packet.
    Zombie,
    /// `connectResponse` sent; bare messages until the client asks (CoD4:
    /// the stats come in).
    Connected,
    /// The gamestate sent, not yet in the world (`CS_PRIMED`, CoD4's
    /// `CS_CLIENTLOADING`).
    Primed,
    Active,
}

/// A CoD2 or CoD4 client slot: the connection both engines keep alike,
/// `extra` the game's own half. `F` is the game's sent frame.
pub struct Conn<F, X> {
    pub link: Link<F>,
    pub state: State,
    /// The message the gamestate went in; an older acknowledge is ignored.
    pub gamestate_message: i32,
    /// The frame the client's last `clc_move` acknowledged, -1 for none.
    pub delta_message: i32,
    /// `lastUsercmd.serverTime`: a cmd at or before it is not thought again.
    pub last_cmd_time: i32,
    /// `pureAuthentic`, `SV_VerifyIwds_f`'s verdict: `None` until a `cp`.
    pub pure: Option<bool>,
    /// `SV_UserinfoChanged`'s rate, snapshot interval and www flag.
    pub rates: Rates,
    /// `Sys_IsLANAddress` of the address: its messages go out every frame.
    pub lan: bool,
    /// One of vcod's bots or a test client: no address, no messages.
    pub bot: bool,
    /// `lastConnectTime`, on `svs.time`.
    pub last_connect_ms: i32,
    /// Each reliable ring slot's type: false for type 0
    /// (`SV_CMD_CAN_IGNORE`), the commands a client not ready loses
    /// ([`super::commands::add`]).
    pub reliable_kept: [bool; RELIABLE_COMMANDS],
    /// Voice chat: the frames queued for the client this frame, its mute
    /// list and `sendVoice`.
    pub voice: crate::voice::ClientVoice,
    pub extra: X,
}

impl<F, X> Conn<F, X> {
    /// A client `connectResponse` just went to at `svs_ms`: nothing sent
    /// or acknowledged, not pure yet.
    pub fn connected(
        link: Link<F>,
        svs_ms: i32,
        rates: Rates,
        lan: bool,
        voice: crate::voice::ClientVoice,
        extra: X,
    ) -> Self {
        Conn {
            link,
            state: State::Connected,
            gamestate_message: -1,
            delta_message: -1,
            last_cmd_time: 0,
            pure: None,
            rates,
            lan,
            bot: false,
            last_connect_ms: svs_ms,
            reliable_kept: [true; RELIABLE_COMMANDS],
            voice,
            extra,
        }
    }
}
