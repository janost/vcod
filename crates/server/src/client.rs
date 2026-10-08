//! One connected client's server-side state (`client_t`).

use crate::spectate::ClientSim;
use std::net::SocketAddr;
use std::time::Instant;
use vcod_common::net::msg::{NULL_USERCMD, UserCmd};
use vcod_common::net::netchan::{ClientMessage, MAX_RELIABLE_COMMANDS, ServerNetchan};
use vcod_common::net::snapshot::Snapshot;

/// `MAX_NAME_LENGTH`, a byte cap since the value is remote input.
const MAX_NAME: usize = 32;

/// Retail's `PACKET_BACKUP`; the client's own `SnapshotRing` ring size matches.
pub const SV_PACKET_BACKUP: usize = 32;

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

pub struct Client {
    pub addr: SocketAddr,
    pub netchan: ServerNetchan,
    pub userinfo: String,
    pub name: String,
    pub state: ClientState,
    /// `gamestateMessageNum`, -1 until a gamestate has gone out.
    pub gamestate_message_num: i64,
    pub last_packet: Instant,
    pub last_connect: Instant,
    /// `lastClientCommand`; goes back out as every message's `reliableAcknowledge`.
    pub last_client_command: i32,
    /// `cl->nextReliableTime`: the end of the flood-protection window the
    /// last non-exempt client command opened (`Server::client_command`).
    pub next_reliable_ms: i32,
    /// The client's `reliableAcknowledge`, what it has seen of our server commands.
    pub reliable_ack: i32,
    /// `reliableSent`: the last server command a message has carried. Only
    /// the ones past it can still be squashed.
    pub reliable_sent: i32,
    /// Each ring slot's [`CmdKind`], beside `netchan.reliable`.
    pub reliable_kind: [CmdKind; MAX_RELIABLE_COMMANDS],
    pub message_ack: i32,
    /// The serverTime of the last usercmd the sim consumed; cmd-to-cmd
    /// deltas drive the pmove dt, retail-style.
    pub last_processed_st: i32,
    /// Usercmds received but not yet replayed, oldest first. Bounded so a
    /// flooded client cannot build unbounded latency.
    pub pending: Vec<QueuedCmd>,
    /// A `kill` this client sent, as the packet it came in: retail runs a
    /// packet's client commands before its usercmds, so the death lands
    /// ahead of that packet's cmds and behind every earlier packet's.
    pub kill_at: Option<u64>,
    /// The last usercmd successfully decoded from this message stream; the
    /// delta base for the next clc_move (`cl->lastUsercmd`). Omitted fields
    /// decode against it, so it commits only after a whole message parses.
    pub last_cmd: UserCmd,
    /// Set once the client enters the world.
    pub sim: Option<ClientSim>,
    /// A server-side bot: no socket traffic, the driver in `server.rs` feeds
    /// its commands and moves.
    pub is_bot: bool,
    /// Frames sent to this client, indexed message_num % SV_PACKET_BACKUP;
    /// the delta base for a later frame is picked from here by message_ack.
    pub frames: Vec<Option<Snapshot>>,
    /// `frames[].messageSent` and `.messageAcked` by `outgoingSequence &
    /// 31`: `svs.time` when the message went out, and when the last move
    /// message acking it arrived, -1 until then.
    pub ping_ring: [(i32, i32); SV_PACKET_BACKUP],
    /// `cl->ping`, `SV_CalcPings`' average over [`Self::ping_ring`].
    pub ping: i32,
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
            addr,
            netchan: ServerNetchan::new(qport, challenge),
            userinfo,
            name,
            state: ClientState::Connected,
            gamestate_message_num: -1,
            last_packet: now,
            last_connect: now,
            last_client_command: 0,
            next_reliable_ms: 0,
            reliable_ack: 0,
            reliable_sent: 0,
            reliable_kind: [CmdKind::CanIgnore; MAX_RELIABLE_COMMANDS],
            message_ack: 0,
            last_processed_st: 0,
            pending: Vec::new(),
            kill_at: None,
            last_cmd: NULL_USERCMD,
            sim: None,
            is_bot: false,
            frames: vec![None; SV_PACKET_BACKUP],
            ping_ring: [(0, 0); SV_PACKET_BACKUP],
            ping: 999,
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
        self.frames = vec![None; SV_PACKET_BACKUP];
        self.pending.clear();
        self.kill_at = None;
        self.last_cmd = NULL_USERCMD;
        self.last_processed_st = 0;
    }

    /// `SV_MapRestart_f`'s client pass (map-cycle doc, section 4 steps 9 to
    /// 11): the level state goes and the connection state stays, because
    /// the restart re-enters an `CS_ACTIVE` client itself and leaves a
    /// `CS_PRIMED` one to its own next message (4.4). The snapshot ring
    /// clears, which is retail's `deltaMessage = -1`, and `last_cmd` is
    /// kept: step 11 hands it to `SV_ClientEnterWorld`.
    pub fn reset_for_restart(&mut self) {
        self.sim = None;
        self.frames = vec![None; SV_PACKET_BACKUP];
        self.pending.clear();
        self.kill_at = None;
    }

    /// `SV_AddServerCommand` (cod_lnxded 0x808b680) without the overflow's
    /// drop, which is the caller's. A client neither active nor within 32
    /// of its acks first loses its unsent [`CmdKind::CanIgnore`] commands
    /// and takes no new one; then a pending command `cmd` supersedes is
    /// taken out and `cmd` goes on the end. A bot is never sent anything on
    /// retail; here it reads the ring itself, so it gets every command as
    /// is.
    pub fn queue_server_command(&mut self, cmd: &str) -> Queued {
        const RING: i32 = MAX_RELIABLE_COMMANDS as i32;
        let kind = CmdKind::of(cmd);
        let slot = |seq: i32| seq as usize & (MAX_RELIABLE_COMMANDS - 1);
        let mut seq = self.netchan.reliable_sequence as i32;
        let mut queued = Queued::Added;
        if !self.is_bot {
            if seq - self.reliable_ack > 31 || self.state != ClientState::Active {
                let mut to = self.reliable_sent + 1;
                for from in self.reliable_sent + 1..=seq {
                    if self.reliable_kind[slot(from)] == CmdKind::CanIgnore {
                        continue;
                    }
                    if slot(from) != slot(to) {
                        self.netchan.reliable[slot(to)] =
                            std::mem::take(&mut self.netchan.reliable[slot(from)]);
                        self.reliable_kind[slot(to)] = self.reliable_kind[slot(from)];
                    }
                    to += 1;
                }
                seq = to - 1;
                self.netchan.reliable_sequence = seq as u32;
                if kind == CmdKind::CanIgnore {
                    return Queued::Dropped;
                }
            }
            let found = (self.reliable_sent + 1..=seq).find(|&i| {
                self.reliable_kind[slot(i)] == CmdKind::Reliable
                    && supersedes(cmd, &self.netchan.reliable[slot(i)])
            });
            if let Some(at) = found {
                for i in at..seq {
                    self.netchan.reliable[slot(i)] =
                        std::mem::take(&mut self.netchan.reliable[slot(i + 1)]);
                    self.reliable_kind[slot(i)] = self.reliable_kind[slot(i + 1)];
                }
                // The freed last slot takes `cmd` below.
                seq -= 1;
                queued = Queued::Replaced;
            }
        }
        if seq + 1 - self.reliable_ack > RING {
            return Queued::Overflow;
        }
        seq += 1;
        self.netchan.reliable_sequence = seq as u32;
        self.netchan.reliable[slot(seq)] = cmd.to_string();
        self.reliable_kind[slot(seq)] = kind;
        queued
    }

    /// The frame sent as `message_num`, if still in the ring.
    pub fn sent_frame(&self, message_num: u32) -> Option<&Snapshot> {
        self.frames[message_num as usize % SV_PACKET_BACKUP]
            .as_ref()
            .filter(|s| s.message_num == message_num)
    }

    /// File a frame under the sequence its packet will carry.
    pub fn record_frame(&mut self, snap: Snapshot) {
        let idx = snap.message_num as usize % SV_PACKET_BACKUP;
        self.frames[idx] = Some(snap);
    }

    /// `SV_SendMessageToClient` (0x808f680): stamps the ring slot of the
    /// message about to go out as `sequence`.
    pub fn stamp_sent(&mut self, sequence: u32, sv_time_ms: i32) {
        self.ping_ring[sequence as usize % SV_PACKET_BACKUP] = (sv_time_ms, -1);
    }

    /// `SV_UserMove` (0x8086fa4), once the cmds decode: a move message stamps the
    /// slot it acks, overwriting an earlier ack of the same message.
    pub fn stamp_acked(&mut self, sv_time_ms: i32) {
        self.ping_ring[self.message_ack as usize % SV_PACKET_BACKUP].1 = sv_time_ms;
    }

    /// `SV_CalcPings`' average (0x808cab8): every slot with a positive ack
    /// time counts, capped at 999, and 999 with none.
    pub fn calc_ping(&self) -> i32 {
        let (sum, n) = self
            .ping_ring
            .iter()
            .filter(|(_, acked)| *acked > 0)
            .fold((0i32, 0i32), |(s, n), (sent, acked)| {
                (s.wrapping_add(acked - sent), n + 1)
            });
        if n == 0 { 999 } else { (sum / n).min(999) }
    }

    /// `cl->rate` as `SV_UserinfoChanged` (0x8086ab4) sets it: a LAN client
    /// on a server below `dedicated 2` reads 99999, anyone else the
    /// userinfo's `rate` clamped to 1000..90000, or 5000 when it is empty.
    pub fn rate(&self, dedicated: i32) -> i32 {
        if dedicated != 2 && is_lan(self.addr.ip()) {
            return 99999;
        }
        match vcod_common::net::info_value_for_key(&self.userinfo, "rate") {
            None | Some("") => 5000,
            Some(r) => atoi(r).clamp(1000, 90000),
        }
    }

    /// Commits a message that passed every check: the netchan sequence, the
    /// acks and the timeout clock. The address is the caller's call, see
    /// `Server::handle_client_packet`.
    pub fn accept(&mut self, m: &ClientMessage, now: Instant) {
        self.netchan.accept(m);
        self.last_packet = now;
        self.message_ack = m.message_ack;
        self.reliable_ack = m.reliable_ack;
    }
}

/// `Sys_IsLANAddress` (0x80c72f8) compares against the host's own
/// interface addresses by class; this takes loopback and the private ranges
/// instead.
fn is_lan(ip: std::net::IpAddr) -> bool {
    match ip {
        std::net::IpAddr::V4(v4) => v4.is_loopback() || v4.is_private(),
        std::net::IpAddr::V6(v6) => v6.is_loopback(),
    }
}

/// `strtol(s, 0, 10)` saturated to `i32`: leading blanks and a sign, then
/// digits until the first non-digit; 0 with none.
fn atoi(s: &str) -> i32 {
    let s = s.trim_start();
    let (neg, digits) = match s.as_bytes().first() {
        Some(b'-') => (true, &s[1..]),
        Some(b'+') => (false, &s[1..]),
        _ => (false, s),
    };
    let mut n: i64 = 0;
    for b in digits.bytes().take_while(u8::is_ascii_digit) {
        n = (n * 10 + i64::from(b - b'0')).min(i64::from(i32::MAX) + 1);
    }
    let n = if neg { -n } else { n };
    n.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32
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
    fn rate_is_clamped_as_retail_does() {
        let mut c = active();
        c.userinfo = "\\rate\\25000".into();
        assert_eq!(c.rate(1), 99999);
        assert_eq!(c.rate(2), 25000);
        c.addr = "8.8.8.8:1".parse().unwrap();
        assert_eq!(c.rate(1), 25000);
        for (rate, want) in [("100", 1000), ("200000", 90000), ("", 5000), ("abc", 1000)] {
            c.userinfo = format!("\\rate\\{rate}");
            assert_eq!(c.rate(1), want, "{rate:?}");
        }
        c.userinfo = String::new();
        assert_eq!(c.rate(1), 5000);
    }

    /// What the next message would carry, oldest first.
    fn unsent(c: &Client) -> Vec<&str> {
        (c.reliable_sent + 1..=c.netchan.reliable_sequence as i32)
            .map(|s| c.netchan.reliable[s as usize & 63].as_str())
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
        c.reliable_sent = c.netchan.reliable_sequence as i32;
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
        assert_eq!(c.netchan.reliable_sequence, 64);
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
