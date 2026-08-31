//! One connected client's server-side state (`client_t`).

use crate::spectate::ClientSim;
use std::net::SocketAddr;
use std::time::Instant;
use vcod_common::net::msg::{UserCmd, NULL_USERCMD};
use vcod_common::net::netchan::{ClientMessage, ServerNetchan};
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
    /// The client's `reliableAcknowledge`, what it has seen of our server commands.
    pub reliable_ack: i32,
    pub message_ack: i32,
    /// The serverTime of the last usercmd the sim consumed; cmd-to-cmd
    /// deltas drive the pmove dt, retail-style.
    pub last_processed_st: i32,
    /// Usercmds received but not yet replayed, oldest first. Bounded so a
    /// flooded client cannot build unbounded latency.
    pub pending: Vec<UserCmd>,
    /// The last usercmd successfully decoded from this message stream; the
    /// delta base for the next clc_move (`cl->lastUsercmd`). Omitted fields
    /// decode against it, so it commits only after a whole message parses.
    pub last_cmd: UserCmd,
    /// Set once the client enters the world.
    pub sim: Option<ClientSim>,
    /// Frames sent to this client, indexed message_num % SV_PACKET_BACKUP;
    /// the delta base for a later frame is picked from here by message_ack.
    pub frames: Vec<Option<Snapshot>>,
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
            reliable_ack: 0,
            message_ack: 0,
            last_processed_st: 0,
            pending: Vec::new(),
            last_cmd: NULL_USERCMD,
            sim: None,
            frames: vec![None; SV_PACKET_BACKUP],
        }
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
    use super::sanitize_name;

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
