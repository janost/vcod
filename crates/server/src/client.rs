//! One connected client's server-side state (`client_t`).

use crate::engine::slot::Link;
use crate::engine::{Lan, link};
use crate::spectate::ClientSim;
use std::net::SocketAddr;
use std::time::Instant;
use vcod_common::net::msg::{NULL_USERCMD, UserCmd};
use vcod_common::net::netchan::{ClientMessage, MAX_RELIABLE_COMMANDS, ServerNetchan};
use vcod_common::net::snapshot::Snapshot;

/// `MAX_NAME_LENGTH`, a byte cap since the value is remote input.
const MAX_NAME: usize = 32;

/// Retail's `PACKET_BACKUP`; the client's own `SnapshotRing` ring size matches.
pub const SV_PACKET_BACKUP: usize = crate::engine::slot::PACKET_BACKUP;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClientState {
    /// `CS_CONNECTED`, no gamestate sent yet.
    Connected,
    /// `CS_PRIMED`, gamestate sent, no move received yet.
    Primed,
    /// `CS_ACTIVE`, flying.
    Active,
}

/// A usercmd waiting for the tick, stamped with the packet it arrived in:
/// the tick replays every client's packets in that order, which is the
/// order retail's `SV_ExecuteClientMessage` ran them in.
#[derive(Clone, Copy, Debug)]
pub struct QueuedCmd {
    pub packet: u64,
    pub cmd: UserCmd,
}

/// A client command that runs in the move pass, ahead of its packet's
/// usercmds and behind every earlier packet's, as `SV_ExecuteClientMessage`
/// (0x80872ec) runs a packet's client commands before its usercmds.
#[derive(Clone, Debug, PartialEq)]
pub enum PacketCommand {
    /// `Cmd_Kill_f` (combat doc, section 8).
    Kill,
    /// `Cmd_MenuResponse_f`'s notify: the menu and the response.
    MenuResponse(crate::game::script::MenuRef, String),
    /// UO's `Cmd_NextVehSlot_f` / `Cmd_PrevVehSlot_f`, `back` for the
    /// latter (coduo-vehicles.md 11.3).
    CycleSeat { back: bool },
}

impl From<UserCmd> for QueuedCmd {
    fn from(cmd: UserCmd) -> Self {
        QueuedCmd { packet: 0, cmd }
    }
}

/// `svscmd_type`, what `SV_AddServerCommand` may do with a queued command
/// (docs/protocol-1.1.md, "The server command queue").
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CmdKind {
    /// 0: dropped while the client is not active or 32 behind, never
    /// replaced by a later command.
    #[default]
    CanIgnore,
    /// 1: always queued, and replaces a matching command still unsent.
    Reliable,
}

impl CmdKind {
    /// The type retail's call sites pass, which is one per letter: every
    /// print, chat, quick chat, announcement and local sound is 0.
    pub fn of(cmd: &str) -> CmdKind {
        match cmd.as_bytes().first() {
            Some(b'c' | b'e' | b'f' | b'g' | b'h' | b'i' | b'j' | b'k' | b'l' | b's') => {
                CmdKind::CanIgnore
            }
            _ => CmdKind::Reliable,
        }
    }
}

/// What [`Client::queue_server_command`] did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Queued {
    Added,
    /// An unsent match was taken out and this one appended.
    Replaced,
    /// A [`CmdKind::CanIgnore`] command the client is not ready for.
    Dropped,
    /// Queueing would overwrite a slot the client has not acked; nothing
    /// was written, the caller drops the client.
    Overflow,
}

/// `SV_FindPendingCommand` (cod_lnxded 0x808b580): whether `new` makes the
/// pending `old` redundant. Same text, or the same letter for `a b o p q r
/// t`, or the same first argument for `d` and `v`; never a `x y z` big
/// configstring chunk.
fn supersedes(new: &str, old: &str) -> bool {
    let (n, o) = (new.as_bytes(), old.as_bytes());
    let Some(&letter) = n.first() else {
        return false;
    };
    if o.first() != Some(&letter) || (b'x'..=b'z').contains(&letter) {
        return false;
    }
    if n[1..] == o[1..] {
        return true;
    }
    let first_arg = |b: &[u8]| -> Vec<u8> {
        b.get(2..)
            .unwrap_or_default()
            .iter()
            .copied()
            .take_while(|&c| c != b' ')
            .collect()
    };
    match letter {
        b'a' | b'b' | b'o' | b'p' | b'q' | b'r' | b't' => true,
        b'd' | b'v' => first_arg(n) == first_arg(o),
        _ => false,
    }
}

/// `cl->pureAuthentic`: what the last `cp` proved (`SV_VerifyPaks_f`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Pure {
    /// No `cp` since the connect or the last `vdr`.
    #[default]
    Unchecked,
    Authentic,
    Unpure,
}

pub struct Client {
    /// The connection half both servers keep alike (`engine::slot`).
    pub link: Link<Snapshot>,
    pub pure: Pure,
    /// The pak being sent, from `download` until it ends.
    pub download: Option<crate::download::ServerDownload>,
    /// The www redirect's state, which outlives one transfer.
    pub www: crate::download::WwwState,
    pub name: String,
    pub state: ClientState,
    /// `gamestateMessageNum`, -1 until a gamestate has gone out.
    pub gamestate_message_num: i64,
    pub last_packet: Instant,
    pub last_connect: Instant,
    /// Each ring slot's [`CmdKind`], beside `netchan.reliable`.
    pub reliable_kind: [CmdKind; MAX_RELIABLE_COMMANDS],
    pub message_ack: i32,
    /// The serverTime of the last usercmd the sim consumed; cmd-to-cmd
    /// deltas drive the pmove dt, retail-style.
    pub last_processed_st: i32,
    /// Retail's `cl->lastUsercmd.serverTime`: `SV_UserMove` hands the game
    /// only a cmd past it (`cod_lnxded` 0x8087264, cod11-combat.md 3.6), so a
    /// redundant copy never reaches `ClientThink`, where one past it but
    /// not past `commandTime` still rotates `pers.oldcmd`.
    pub last_usercmd_st: i32,
    /// Usercmds received but not yet replayed, oldest first. Bounded so a
    /// flooded client cannot build unbounded latency.
    pub pending: Vec<QueuedCmd>,
    /// The [`PacketCommand`]s this client sent, each with the packet it
    /// came in, in arrival order.
    pub commands_at: Vec<(u64, PacketCommand)>,
    /// UO's `fdc <entnum>` (`Cmd_FlameDamageClient_f`, game.mp.uo 0x4ee2c):
    /// whose flame the client says it stands in and the `level.time` it
    /// said so.
    pub flame: Option<(u32, i32)>,
    /// The last usercmd successfully decoded from this message stream; the
    /// delta base for the next clc_move (`cl->lastUsercmd`). Omitted fields
    /// decode against it, so it commits only after a whole message parses.
    pub last_cmd: UserCmd,
    /// Set once the client enters the world.
    pub sim: Option<ClientSim>,
    /// A server-side bot: no socket traffic, the driver in `server.rs` feeds
    /// its commands and moves.
    pub is_bot: bool,
    /// A bot's netadr port: `SV_AddTestClient` (0x80875cf) connects it from
    /// a type-0 (bot) address, which `status` prints IPX-style and
    /// `Sys_IsLANAddress` does not count as LAN. `None` for a socket client.
    pub bot_port: Option<u16>,
    /// `CS_ZOMBIE`: dropped, and kept only to carry the drop notice. Paced
    /// as a client out of the world ([`Self::pace`]).
    pub zombie: bool,
}

impl Client {
    pub fn new(
        addr: SocketAddr,
        qport: u16,
        challenge: i32,
        userinfo: String,
        now: Instant,
    ) -> Self {
        let name = sanitize_name(
            vcod_common::net::info_value_for_key(&userinfo, "name").unwrap_or("UnnamedPlayer"),
        );
        Client {
            link: Link::new(addr, ServerNetchan::new(qport, challenge), userinfo),
            pure: Pure::Unchecked,
            download: None,
            www: Default::default(),
            name,
            state: ClientState::Connected,
            gamestate_message_num: -1,
            last_packet: now,
            last_connect: now,
            zombie: false,
            reliable_kind: [CmdKind::CanIgnore; MAX_RELIABLE_COMMANDS],
            message_ack: 0,
            last_processed_st: 0,
            last_usercmd_st: 0,
            pending: Vec::new(),
            commands_at: Vec::new(),
            flame: None,
            last_cmd: NULL_USERCMD,
            sim: None,
            is_bot: false,
            bot_port: None,
        }
    }

    /// `SV_SpawnServer`'s client pass (docs/research/cod11-map-cycle.md,
    /// section 3 step 22): back to `CS_CONNECTED` with the level state gone
    /// and the netchan and both reliable rings kept. `gamestate_message_num`
    /// goes back to -1 so the next message off this client takes the
    /// high-nibble branch and pulls the new gamestate; nothing is pushed
    /// (section 3.1).
    pub fn reset_for_level(&mut self) {
        self.state = ClientState::Connected;
        self.gamestate_message_num = -1;
        self.sim = None;
        self.link.frames = vec![None; SV_PACKET_BACKUP];
        self.pending.clear();
        self.commands_at.clear();
        self.last_cmd = NULL_USERCMD;
        self.last_processed_st = 0;
        self.last_usercmd_st = 0;
    }

    /// `SV_MapRestart_f`'s client pass (map-cycle doc, section 4 steps 9 to
    /// 11): the level state goes and the connection state stays, because
    /// the restart re-enters an `CS_ACTIVE` client itself and leaves a
    /// `CS_PRIMED` one to its own next message (4.4). The snapshot ring
    /// clears, which is retail's `deltaMessage = -1`, and `last_cmd` is
    /// kept: step 11 hands it to `SV_ClientEnterWorld`.
    pub fn reset_for_restart(&mut self) {
        self.sim = None;
        self.link.frames = vec![None; SV_PACKET_BACKUP];
        self.pending.clear();
        self.commands_at.clear();
    }

    /// `SV_AddServerCommand`'s squash: the unsent [`CmdKind::CanIgnore`]
    /// commands leave the ring, the rest close up. What a zombie's drop
    /// notice runs, its state no longer active (0x8085e74).
    pub fn squash_ignorable(&mut self) {
        let slot = |seq: i32| seq as usize & (MAX_RELIABLE_COMMANDS - 1);
        let seq = self.link.netchan.reliable_sequence as i32;
        let mut to = self.link.reliable_sent + 1;
        for from in self.link.reliable_sent + 1..=seq {
            if self.reliable_kind[slot(from)] == CmdKind::CanIgnore {
                continue;
            }
            if slot(from) != slot(to) {
                self.link.netchan.reliable[slot(to)] =
                    std::mem::take(&mut self.link.netchan.reliable[slot(from)]);
                self.reliable_kind[slot(to)] = self.reliable_kind[slot(from)];
            }
            to += 1;
        }
        self.link.netchan.reliable_sequence = (to - 1) as u32;
    }

    /// `SV_AddServerCommand` (cod_lnxded 0x808b680) without the overflow's
    /// drop, which is the caller's. A client neither active nor within 32
    /// of its acks first loses its unsent [`CmdKind::CanIgnore`] commands
    /// and takes no new one; then a pending command `cmd` supersedes is
    /// taken out and `cmd` goes on the end. A bot is never sent anything on
    /// retail (0x808b68c); here it reads the ring itself, so it gets every
    /// [`CmdKind::Reliable`] command as is and no ignorable one, which it
    /// never reads: one frame's prints to 61 or more bots would overflow
    /// the ring before their acks (protocol-1.1.md, "The server command
    /// queue").
    pub fn queue_server_command(&mut self, cmd: &str) -> Queued {
        const RING: i32 = MAX_RELIABLE_COMMANDS as i32;
        let kind = CmdKind::of(cmd);
        if self.is_bot && kind == CmdKind::CanIgnore {
            return Queued::Dropped;
        }
        let slot = |seq: i32| seq as usize & (MAX_RELIABLE_COMMANDS - 1);
        let mut seq = self.link.netchan.reliable_sequence as i32;
        let mut queued = Queued::Added;
        if !self.is_bot {
            if seq - self.link.reliable_ack > 31 || self.state != ClientState::Active {
                self.squash_ignorable();
                seq = self.link.netchan.reliable_sequence as i32;
                if kind == CmdKind::CanIgnore {
                    return Queued::Dropped;
                }
            }
            let found = (self.link.reliable_sent + 1..=seq).find(|&i| {
                self.reliable_kind[slot(i)] == CmdKind::Reliable
                    && supersedes(cmd, &self.link.netchan.reliable[slot(i)])
            });
            if let Some(at) = found {
                for i in at..seq {
                    self.link.netchan.reliable[slot(i)] =
                        std::mem::take(&mut self.link.netchan.reliable[slot(i + 1)]);
                    self.reliable_kind[slot(i)] = self.reliable_kind[slot(i + 1)];
                }
                // The freed last slot takes `cmd` below.
                seq -= 1;
                queued = Queued::Replaced;
            }
        }
        if seq + 1 - self.link.reliable_ack > RING {
            return Queued::Overflow;
        }
        seq += 1;
        self.link.netchan.reliable_sequence = seq as u32;
        self.link.netchan.reliable[slot(seq)] = cmd.to_string();
        self.reliable_kind[slot(seq)] = kind;
        queued
    }

    /// The frame sent as `message_num`, if still in the ring.
    pub fn sent_frame(&self, message_num: u32) -> Option<&Snapshot> {
        self.link.frames[message_num as usize % SV_PACKET_BACKUP]
            .as_ref()
            .filter(|s| s.message_num == message_num)
    }

    /// File a frame under the sequence its packet will carry.
    pub fn record_frame(&mut self, snap: Snapshot) {
        let idx = snap.message_num as usize % SV_PACKET_BACKUP;
        self.link.frames[idx] = Some(snap);
    }

    /// `SV_SendMessageToClient` (0x808f680): stamps the ring slot of the
    /// message about to go out as `sequence`.
    pub fn stamp_sent(&mut self, sequence: u32, sv_time_ms: i32) {
        self.link.ping_ring[sequence as usize % SV_PACKET_BACKUP] = (sv_time_ms, -1);
    }

    /// `SV_UserMove` (0x8086fa4), once the cmds decode: a move message stamps the
    /// slot it acks, overwriting an earlier ack of the same message.
    pub fn stamp_acked(&mut self, sv_time_ms: i32) {
        self.link.ping_ring[self.message_ack as usize % SV_PACKET_BACKUP].1 = sv_time_ms;
    }

    /// `SV_CalcPings`' average (0x808cab8): every slot with a positive ack
    /// time counts, capped at 999, and 999 with none.
    pub fn calc_ping(&self) -> i32 {
        link::calc_ping(&self.link.ping_ring)
    }

    /// `cl->rate` as `SV_UserinfoChanged` (0x8086ab4) sets it: a LAN client
    /// on a server below `dedicated 2` reads 99999, anyone else the
    /// userinfo's `rate` clamped to 1000..90000, or 5000 when it is empty.
    pub fn rate(&self, dedicated: i32) -> i32 {
        let lan = self.bot_port.is_none() && Lan::Cod1.is_lan(self.link.addr.ip());
        link::rate(&self.link.userinfo, lan, dedicated)
    }

    /// `cl->snapshotMsec` as `SV_UserinfoChanged` sets it (0x8086cad..0x8086d44):
    /// 50 when `snaps` is empty or missing, else 1000 over `snaps` clamped
    /// to 1..30. It floors an off-LAN client's message interval and sizes a
    /// download message.
    pub fn snapshot_msec(&self) -> i32 {
        link::snapshot_msec(&self.link.userinfo)
    }

    /// `cl_wwwDownload` read as `SV_UserinfoChanged` reads it (UO
    /// 0x808d2c7): non-empty and its `atoi` not 0.
    pub fn wants_www(&self) -> bool {
        link::wants_www(&self.link.userinfo)
    }

    /// The rate a message is paced and filled by: [`Self::rate`] under a
    /// non-zero `sv_maxRate`, which reads 1000 at the least (0x808f7a5).
    pub fn send_rate(&self, dedicated: i32, max_rate: i32) -> i32 {
        link::send_rate(self.rate(dedicated), max_rate)
    }

    /// `SV_SendMessageToClient`'s `nextSnapshotTime` (0x808f680) after a
    /// message of `bytes`: the next frame for a LAN client, else the time
    /// the rate needs to carry it (capped at 1500 bytes, plus 48 of
    /// headers) and never under the snapshot interval. A client that is
    /// neither in the world nor downloading, a zombie included, waits a
    /// second. A LAN client leaves `link.rate_delayed` as it was.
    pub fn pace(&mut self, sv_time_ms: i32, bytes: usize, rate: i32, snapshot_ms: i32) {
        let lan = Lan::Cod1.is_lan(self.link.addr.ip());
        let idle = (self.state != ClientState::Active || self.zombie) && self.download.is_none();
        match link::pace(sv_time_ms, bytes, lan, rate, snapshot_ms, idle) {
            // Retail's `svs.time - 1`: due whatever the clock does next.
            link::Pace::Lan => self.link.next_message_ms = i32::MIN,
            link::Pace::At { next, delayed } => {
                self.link.rate_delayed = delayed;
                self.link.next_message_ms = next;
            }
        }
    }

    /// `SV_SendMessageToClient`'s send: the first packet of a message goes
    /// out now, a fragmented message's others wait in `link.unsent`.
    pub fn first_packet(&mut self, mut packets: Vec<Vec<u8>>) -> Option<Vec<u8>> {
        if packets.is_empty() {
            return None;
        }
        let first = packets.remove(0);
        self.link.unsent = packets.into();
        Some(first)
    }

    /// `SV_SendClientMessages`' fragment turn (0x809045c): the next unsent
    /// packet, paced by the bytes left at the rate with no LAN shortcut and
    /// no snapshot floor.
    pub fn next_fragment(&mut self, sv_time_ms: i32, rate: i32) -> Option<Vec<u8>> {
        let left: usize = self.link.unsent.iter().map(Vec::len).sum();
        let pkt = self.link.unsent.pop_front()?;
        self.link.next_message_ms = sv_time_ms + link::rate_msec(left, rate);
        Some(pkt)
    }

    /// Commits a message that passed every check: the netchan sequence, the
    /// acks and the timeout clock. The address is the caller's call, see
    /// `Server::handle_client_packet`.
    pub fn accept(&mut self, m: &ClientMessage, now: Instant, sv_time_ms: i32) {
        self.link.netchan.accept(m);
        self.last_packet = now;
        self.link.last_packet_ms = sv_time_ms;
        self.message_ack = m.message_ack;
        self.link.reliable_ack = m.reliable_ack;
    }
}

/// Strips the quote that delimits a `statusResponse` player line and any
/// control char, caps the length, falls back to `UnnamedPlayer`.
pub fn sanitize_name(raw: &str) -> String {
    let mut out = String::with_capacity(MAX_NAME);
    for c in raw.chars().filter(|c| *c != '"' && !c.is_control()) {
        if out.len() + c.len_utf8() > MAX_NAME {
            break;
        }
        out.push(c);
    }
    if out.is_empty() {
        out.push_str("UnnamedPlayer");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn active() -> Client {
        let addr = "127.0.0.1:1".parse().unwrap();
        let mut c = Client::new(addr, 1, 1, String::new(), Instant::now());
        c.state = ClientState::Active;
        c
    }

    /// `SV_CalcPings` counts only acked slots, divides as an int, caps at
    /// 999, and reads 999 with nothing acked.
    #[test]
    fn ping_averages_the_acked_slots() {
        let mut c = active();
        assert_eq!(c.calc_ping(), 999);
        c.stamp_sent(0, 1000);
        c.stamp_sent(1, 1050);
        c.stamp_sent(2, 1100);
        c.message_ack = 0;
        c.stamp_acked(1050);
        c.message_ack = 1;
        c.stamp_acked(1050);
        // (50 + 0) / 2; slot 2 is still -1.
        assert_eq!(c.calc_ping(), 25);
        c.message_ack = 2;
        c.stamp_acked(5000);
        assert_eq!(c.calc_ping(), 999);
    }

    /// `SV_UserinfoChanged`'s rate: 99999 for a LAN client below
    /// `dedicated 2` (what retail's `status` printed for a loopback probe),
    /// else clamped, 5000 when empty.
    #[test]
    fn a_message_waits_out_the_rate_off_the_lan() {
        let now = Instant::now();
        let wan = SocketAddr::from(([203, 0, 113, 7], 27960));
        let mut c = Client::new(wan, 1, 2, "\\rate\\5000".into(), now);
        c.state = ClientState::Active;
        // 452 + 48 bytes at 5000 B/s is 100 ms, past the 50 ms interval.
        c.pace(1000, 452, c.send_rate(1, 0), 50);
        assert_eq!(c.link.next_message_ms, 1100);
        // A small message keeps the snapshot interval.
        c.pace(1000, 100, c.send_rate(1, 0), 50);
        assert_eq!(c.link.next_message_ms, 1050);
        // `sv_maxRate` caps the rate, never under 1000.
        assert_eq!(c.send_rate(1, 3000), 3000);
        assert_eq!(c.send_rate(1, 10), 1000);
        // Not in the world and not downloading: a second.
        c.state = ClientState::Primed;
        c.pace(1000, 100, 5000, 50);
        assert_eq!(c.link.next_message_ms, 2000);
        let mut lan = Client::new(
            SocketAddr::from(([127, 0, 0, 1], 1)),
            1,
            2,
            String::new(),
            now,
        );
        lan.pace(1000, 1400, 1000, 50);
        assert!(lan.link.next_message_ms <= 1000);
    }

    #[test]
    fn rate_is_clamped_as_retail_does() {
        let mut c = active();
        c.link.userinfo = "\\rate\\25000".into();
        assert_eq!(c.rate(1), 99999);
        assert_eq!(c.rate(2), 25000);
        c.link.addr = "203.0.113.1:1".parse().unwrap();
        assert_eq!(c.rate(1), 25000);
        for (rate, want) in [("100", 1000), ("200000", 90000), ("", 5000), ("abc", 1000)] {
            c.link.userinfo = format!("\\rate\\{rate}");
            assert_eq!(c.rate(1), want, "{rate:?}");
        }
        c.link.userinfo = String::new();
        assert_eq!(c.rate(1), 5000);
    }

    #[test]
    fn snaps_sets_the_snapshot_interval_as_retail_does() {
        let mut c = active();
        for (snaps, want) in [
            ("20", 50),
            ("30", 33),
            ("10", 100),
            ("100", 33),
            ("0", 1000),
            ("abc", 1000),
            ("", 50),
        ] {
            c.link.userinfo = format!("\\rate\\25000\\snaps\\{snaps}");
            assert_eq!(c.snapshot_msec(), want, "{snaps:?}");
        }
        c.link.userinfo = "\\rate\\25000".into();
        assert_eq!(c.snapshot_msec(), 50);
    }

    /// What the next message would carry, oldest first.
    fn unsent(c: &Client) -> Vec<&str> {
        (c.link.reliable_sent + 1..=c.link.netchan.reliable_sequence as i32)
            .map(|s| c.link.netchan.reliable[s as usize & 63].as_str())
            .collect()
    }

    /// The burst `client-probes/probe_squash` queued in one frame on
    /// retail, in, and the commands its probe received, out.
    #[test]
    fn one_frame_squashes_as_the_retail_probe_did() {
        let mut c = active();
        for cmd in [
            "v sq_a \"1\"",
            "v sq_a \"2\"",
            "v sq_b \"1\"",
            "v sq_c \"1\"",
            "f \"sq dup\"",
            "f \"sq dup\"",
            "u",
            "u",
            "v sq_d \"1\"",
            "f \"sq between\"",
            "v sq_d \"2\"",
        ] {
            c.queue_server_command(cmd);
        }
        assert_eq!(
            unsent(&c),
            [
                "v sq_a \"2\"",
                "v sq_b \"1\"",
                "v sq_c \"1\"",
                "f \"sq dup\"",
                "f \"sq dup\"",
                "u",
                "f \"sq between\"",
                "v sq_d \"2\"",
            ]
        );
    }

    #[test]
    fn a_command_already_sent_is_never_replaced() {
        let mut c = active();
        c.queue_server_command("v sq_a \"3\"");
        c.link.reliable_sent = c.link.netchan.reliable_sequence as i32;
        assert_eq!(c.queue_server_command("v sq_a \"4\""), Queued::Added);
        assert_eq!(unsent(&c), ["v sq_a \"4\""]);
    }

    #[test]
    fn what_supersedes_what() {
        // `d` matches on the index alone, as `v` on the name.
        assert!(supersedes("d 5 118", "d 5 117"));
        assert!(!supersedes("d 5 118", "d 51 118"));
        // These letters on the letter alone.
        assert!(supersedes("t 3", "t 1"));
        assert!(supersedes("b 0 1 2", "b 9"));
        // Anything else on the whole text only.
        assert!(supersedes("n", "n"));
        assert!(!supersedes("m 1", "m 2"));
        // A big configstring's chunks never.
        assert!(!supersedes("x 20 abc", "x 20 abc"));
    }

    /// Before it is active, and once 32 behind its acks, a client loses
    /// its unsent prints and takes no new one; its cvars stay queued.
    #[test]
    fn a_client_not_ready_drops_prints_only() {
        let addr = "127.0.0.1:1".parse().unwrap();
        let mut c = Client::new(addr, 1, 1, String::new(), Instant::now());
        assert_eq!(
            c.queue_server_command("f \"sq early print\""),
            Queued::Dropped
        );
        assert_eq!(c.queue_server_command("v sq_early \"1\""), Queued::Added);
        assert_eq!(unsent(&c), ["v sq_early \"1\""]);

        let mut c = active();
        c.queue_server_command("e \"kept\"");
        for i in 0..30 {
            c.queue_server_command(&format!("v c{i} 1"));
        }
        // 31 behind: still queued.
        assert_eq!(c.queue_server_command("e \"late\""), Queued::Added);
        // 32: both prints go, and the new one with them.
        assert_eq!(c.queue_server_command("e \"later\""), Queued::Dropped);
        assert_eq!(unsent(&c).len(), 30);
        assert!(unsent(&c).iter().all(|cmd| cmd.starts_with('v')));
    }

    #[test]
    fn the_ring_overflows_past_64_unacked() {
        let mut c = active();
        for i in 0..64 {
            assert_eq!(c.queue_server_command(&format!("v c{i} 1")), Queued::Added);
        }
        assert_eq!(c.queue_server_command("v c99 1"), Queued::Overflow);
        assert_eq!(c.link.netchan.reliable_sequence, 64);
        // A replacement takes no new slot.
        assert_eq!(c.queue_server_command("v c3 2"), Queued::Replaced);
    }

    #[test]
    fn a_name_cannot_forge_a_status_line_or_an_escape_sequence() {
        // A quote or newline could invent a `statusResponse` player line.
        assert_eq!(sanitize_name("ab\"\n0 0 \"admin"), "ab0 0 admin");
        // ESC would reach the terminal through the log lines.
        assert_eq!(sanitize_name("\u{1b}[2Jgone"), "[2Jgone");
        assert_eq!(sanitize_name("\"\""), "UnnamedPlayer");
        assert_eq!(sanitize_name(""), "UnnamedPlayer");
    }

    #[test]
    fn the_cap_is_32_bytes_on_a_char_boundary() {
        assert_eq!(sanitize_name(&"x".repeat(64)), "x".repeat(32));
        // 'ä' is two bytes; the twelfth must be dropped whole, not cut in half.
        let name = sanitize_name(&("ä".repeat(10) + &"x".repeat(11) + "ä"));
        assert_eq!(name, "ä".repeat(10) + &"x".repeat(11));
        assert_eq!(name.len(), 31);
    }
}
