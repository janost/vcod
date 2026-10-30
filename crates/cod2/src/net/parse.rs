//! The client's server-message parser (`CL_ParseServerMessage`, Mac
//! 0x1d69fa): gamestate, server commands, downloads and delta snapshots
//! against a 32-frame ring (docs/research/cod2-protocol.md 3).

use std::collections::BTreeMap;

use anyhow::{Context, bail, ensure};
use vcod_common::net::codec::MsgReader;

use super::delta::{
    ClientState2, Delta, EntityState2, PlayerState2, read_delta_client, read_delta_entity,
    read_delta_playerstate,
};
use super::latin1;
use super::ops::{
    CLIENTNUM_BITS, ENTITYNUM_NONE, GENTITYNUM_BITS, MAX_CONFIGSTRINGS, MAX_GENTITIES,
    MAX_RELIABLE_COMMANDS, PACKET_BACKUP, Svc,
};

/// `svc_gamestate`'s body (3.2).
#[derive(Clone, Debug, PartialEq)]
pub struct Gamestate2 {
    pub server_command_sequence: i32,
    /// Every non-empty configstring by index, as Latin-1.
    pub configstrings: BTreeMap<u16, String>,
    /// Baselines by entity number.
    pub baselines: BTreeMap<u32, EntityState2>,
    pub client_num: i32,
    pub checksum_feed: i32,
}

impl Gamestate2 {
    pub fn configstring(&self, i: u16) -> &str {
        self.configstrings.get(&i).map_or("", String::as_str)
    }

    /// `sv_serverid` out of the systeminfo (configstring 1).
    pub fn server_id(&self) -> Option<i32> {
        super::oob::info_value_for_key(self.configstring(1), "sv_serverid")?
            .parse()
            .ok()
    }
}

/// One decoded `svc_snapshot` (3.3).
#[derive(Clone, Debug, PartialEq)]
pub struct Snapshot2 {
    /// The netchan sequence that carried it.
    pub message_num: i32,
    pub server_time: i32,
    /// The frame it deltas from, -1 for none.
    pub delta_num: i32,
    pub snap_flags: u8,
    /// False when the delta frame was not the one the server meant.
    pub valid: bool,
    pub ps_lc: u8,
    pub ps: PlayerState2,
    /// Ascending by number.
    pub entities: Vec<EntityState2>,
    /// Ascending by number.
    pub clients: Vec<ClientState2>,
}

/// What one server message held.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Parsed {
    /// New server commands, `(sequence, text)`.
    pub commands: Vec<(i32, Vec<u8>)>,
    pub gamestate: bool,
    pub snapshot: Option<i32>,
    /// The `svc_download`s, in order (`net::download`).
    pub downloads: Vec<super::download::DownloadMsg>,
    /// `svc_nop` bytes (`sv_padPackets`).
    pub nops: usize,
    /// Bytes the parse consumed and the decompressed length. The block's last
    /// byte holds padding bits the Huffman decode reads as up to one symbol.
    pub consumed: usize,
    pub plain_len: usize,
}

/// The client's parse state: gamestate, command ring and frame ring.
pub struct Parser {
    pub gamestate: Option<Gamestate2>,
    pub server_commands: Vec<Vec<u8>>,
    pub server_command_sequence: i32,
    frames: Vec<Option<Snapshot2>>,
    /// The newest valid snapshot's message number.
    pub last_snapshot: Option<i32>,
}

impl Default for Parser {
    fn default() -> Self {
        Parser {
            gamestate: None,
            server_commands: vec![Vec::new(); MAX_RELIABLE_COMMANDS],
            server_command_sequence: 0,
            frames: vec![None; PACKET_BACKUP],
            last_snapshot: None,
        }
    }
}

impl Parser {
    pub fn new() -> Self {
        Self::default()
    }

    /// The frame stored for `message_num`, if it is still in the ring.
    pub fn frame(&self, message_num: i32) -> Option<&Snapshot2> {
        self.frames[message_num as usize % PACKET_BACKUP]
            .as_ref()
            .filter(|s| s.message_num == message_num)
    }

    /// The newest valid snapshot.
    pub fn snapshot(&self) -> Option<&Snapshot2> {
        self.frame(self.last_snapshot?)
    }

    /// `serverCommands[seq & 127]`, the string the client keys its packets on.
    pub fn server_command(&self, seq: i32) -> &[u8] {
        &self.server_commands[seq as usize % MAX_RELIABLE_COMMANDS]
    }

    /// Parse one decompressed message carried by netchan `sequence`.
    pub fn parse(&mut self, sequence: i32, plain: &[u8]) -> anyhow::Result<Parsed> {
        let mut r = MsgReader::from_plain(plain.to_vec());
        let mut out = Parsed {
            plain_len: plain.len(),
            ..Parsed::default()
        };
        loop {
            ensure!(!r.is_overflowed(), "read past the end of the message");
            let op = r.read_byte();
            match Svc::from_u8(op) {
                Some(Svc::Eof) => break,
                Some(Svc::Nop) => out.nops += 1,
                Some(Svc::ServerCommand) => {
                    let seq = r.read_long();
                    let text = r.read_string_bytes();
                    if seq > self.server_command_sequence {
                        self.server_command_sequence = seq;
                        self.server_commands[seq as usize % MAX_RELIABLE_COMMANDS] = text.clone();
                        out.commands.push((seq, text));
                    }
                }
                Some(Svc::Gamestate) => {
                    self.parse_gamestate(&mut r).context("gamestate")?;
                    out.gamestate = true;
                }
                Some(Svc::Snapshot) => {
                    let s = self.parse_snapshot(&mut r, sequence).context("snapshot")?;
                    out.snapshot = Some(s);
                }
                Some(Svc::Download) => {
                    out.downloads
                        .push(super::download::DownloadMsg::read(&mut r));
                }
                _ => bail!("illegible server message {op}"),
            }
        }
        ensure!(!r.is_overflowed(), "read past the end of the message");
        out.consumed = r.readcount();
        Ok(out)
    }

    fn parse_gamestate(&mut self, r: &mut MsgReader) -> anyhow::Result<()> {
        let server_command_sequence = r.read_long();
        self.server_command_sequence = server_command_sequence;
        let mut configstrings = BTreeMap::new();
        let mut baselines = BTreeMap::new();
        loop {
            ensure!(!r.is_overflowed(), "read past the end");
            match r.read_byte() {
                2 => {
                    let i = r.read_short() as u16;
                    ensure!(usize::from(i) < MAX_CONFIGSTRINGS, "configstring {i}");
                    configstrings.insert(i, latin1(&r.read_big_string_bytes()));
                }
                3 => {
                    let n = r.read_bits(GENTITYNUM_BITS) as u32;
                    ensure!((n as usize) < MAX_GENTITIES, "baseline {n}");
                    let (_, es) = read_delta_entity(r, &EntityState2::null(n), n)?;
                    baselines.insert(n, es);
                }
                7 => break,
                op => bail!("gamestate op {op}"),
            }
        }
        let client_num = r.read_long();
        let checksum_feed = r.read_long();
        self.gamestate = Some(Gamestate2 {
            server_command_sequence,
            configstrings,
            baselines,
            client_num,
            checksum_feed,
        });
        self.frames.iter_mut().for_each(|f| *f = None);
        self.last_snapshot = None;
        Ok(())
    }

    /// `CL_ParseSnapshot` (Mac 0x1d5cb2). A delta whose frame left the ring
    /// is an error here; retail parses it against the stale slot and drops
    /// the result, which vcod does when the slot holds a different frame.
    /// Retail's 0x780-entity age check has no analogue: frames hold their
    /// own entity lists.
    fn parse_snapshot(&mut self, r: &mut MsgReader, message_num: i32) -> anyhow::Result<i32> {
        let server_time = r.read_long();
        let back = i32::from(r.read_byte());
        let snap_flags = r.read_byte();
        let delta_num = if back == 0 { -1 } else { message_num - back };
        let (from, valid) = if delta_num < 0 {
            (None, true)
        } else {
            let slot = self.frames[delta_num as usize % PACKET_BACKUP]
                .as_ref()
                .with_context(|| format!("delta from empty frame {delta_num}"))?;
            (Some(slot), slot.message_num == delta_num && slot.valid)
        };
        let (ps_lc, ps) = read_delta_playerstate(r, from.map(|f| &f.ps))?;
        let gs = self
            .gamestate
            .as_ref()
            .context("snapshot before gamestate")?;

        // packet entities: 10-bit numbers, 1023 ends.
        let old_ents: &[EntityState2] = from.map_or(&[], |f| &f.entities);
        let mut entities = Vec::with_capacity(old_ents.len() + 8);
        let mut oi = 0;
        loop {
            let n = r.read_bits(GENTITYNUM_BITS) as u32;
            ensure!(!r.is_overflowed(), "packet entities: end of message");
            if n == ENTITYNUM_NONE {
                break;
            }
            while oi < old_ents.len() && old_ents[oi].number < n {
                entities.push(old_ents[oi]);
                oi += 1;
            }
            let (d, es) = if oi < old_ents.len() && old_ents[oi].number == n {
                oi += 1;
                read_delta_entity(r, &old_ents[oi - 1], n)?
            } else {
                let base = gs
                    .baselines
                    .get(&n)
                    .copied()
                    .unwrap_or(EntityState2::null(n));
                read_delta_entity(r, &base, n)?
            };
            if d != Delta::Removed {
                entities.push(es);
            }
        }
        entities.extend_from_slice(&old_ents[oi..]);

        // packet clients: a bit, a 6-bit number; a new one deltas from zero.
        let old_cls: &[ClientState2] = from.map_or(&[], |f| &f.clients);
        let mut clients = Vec::with_capacity(old_cls.len() + 1);
        let mut oi = 0;
        while r.read_bits(1) == 1 {
            let n = r.read_bits(CLIENTNUM_BITS) as u32;
            ensure!(!r.is_overflowed(), "packet clients: end of message");
            while oi < old_cls.len() && old_cls[oi].number < n {
                clients.push(old_cls[oi]);
                oi += 1;
            }
            let (d, cs) = if oi < old_cls.len() && old_cls[oi].number == n {
                oi += 1;
                read_delta_client(r, &old_cls[oi - 1], n)?
            } else {
                read_delta_client(r, &ClientState2::null(n), n)?
            };
            if d != Delta::Removed {
                clients.push(cs);
            }
        }
        clients.extend_from_slice(&old_cls[oi..]);
        ensure!(!r.is_overflowed(), "snapshot: end of message");

        let snap = Snapshot2 {
            message_num,
            server_time,
            delta_num,
            snap_flags,
            valid,
            ps_lc,
            ps,
            entities,
            clients,
        };
        self.frames[message_num as usize % PACKET_BACKUP] = Some(snap);
        if valid {
            self.last_snapshot = Some(message_num);
        }
        Ok(message_num)
    }
}
