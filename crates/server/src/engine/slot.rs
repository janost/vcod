//! A client slot's connection half, the `client_t` fields both servers
//! keep under the same rules: the netchan and the reliable rings' acks,
//! the frames a delta is taken from, the ping ring, the message pacing and
//! its fragments, and the timeout clock. Each game's client struct embeds
//! one as `link`; its state machine, rates, downloads and game half stay
//! its own.

use std::collections::VecDeque;
use std::net::SocketAddr;
use vcod_common::net::netchan::ServerNetchan;

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
}
