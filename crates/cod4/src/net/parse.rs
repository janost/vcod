//! A CoD4 1.7 server message, decompressed: server commands, the gamestate
//! (configstrings, baselines, clientNum, checksumFeed) and snapshots
//! (docs/research/cod4-netfields.md 4 and 5), and the writer that
//! puts a parsed message back together byte for byte.

use super::constant_cs;
use super::delta::{
    self, CLIENTNUM_BITS, ClientState, Ctx, ENTITYNUM_NONE, EntityState, GENTITYNUM_BITS,
    PlayerState, PsWire, Wire,
};
use super::huffman::Huffman;
use anyhow::{Context, Result, anyhow, bail, ensure};
use std::collections::BTreeMap;
use vcod_common::net::Msec;
use vcod_common::net::codec::{MsgReader, MsgWriter};
use vcod_common::net::huffman::Huffman as Cod1Huffman;

/// `MsgWriter` wants a CoD1 tree for `finish`, which these writers never
/// call (1.7's code is [`Huffman`]); built once, as it costs tens of
/// milliseconds, so a client builds it with the gamestate (`Conn`'s
/// `answer_gamestate`), not in the first frame's send.
pub static UNUSED_HUFF: std::sync::LazyLock<Cod1Huffman> =
    std::sync::LazyLock::new(Cod1Huffman::new);

pub const SVC_NOP: u8 = 0;
pub const SVC_GAMESTATE: u8 = 1;
pub const SVC_CONFIGSTRING: u8 = 2;
pub const SVC_BASELINE: u8 = 3;
pub const SVC_SERVERCOMMAND: u8 = 4;
pub const SVC_SNAPSHOT: u8 = 6;
pub const SVC_EOF: u8 = 7;
/// `SV_SetConfigstring` (0x8173e54) refuses an index above 0x989 (the
/// compare at 0x8173e60, `SV_SetConfigstring: bad index` at 0x82320d4),
/// and `SV_SendClientGameState`'s loops stop at 0x98a. VERIFIED.
pub const MAX_CONFIGSTRINGS: u32 = 2442;
pub const CS_SYSTEMINFO: u16 = 1;
pub const CS_MAPCENTER: u16 = 12;
/// Snapshots a delta can reach back over.
const PACKET_BACKUP: usize = 32;
/// Server commands [`Client::server_commands`] keeps below the newest: twice
/// the 128-command reliable ring.
const KEPT_SERVER_COMMANDS: i32 = 256;

/// What a parse fails with when `CL_ParseServerMessage` would mark the
/// message overflowed (`iw3mp.exe` 0x474874..0x474881): a read past its
/// end, an unknown op, a delta base the ring does not hold.
/// `CL_PacketEvent` then rolls the message's acknowledges back
/// (cod4-handshake.md 10, "An illegible message rolls back"). Context on
/// the error, so `err.downcast_ref::<Illegible>()` finds it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Illegible;

impl std::fmt::Display for Illegible {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("illegible message")
    }
}

impl std::error::Error for Illegible {}

/// `MSG_ReadString`: the bytes up to the NUL, as they are, no `%` or 0x92
/// mapping (INFERRED, no 1.7 address recorded).
/// [`super::download::DownloadMsg::read`] maps 0x92 through CoD2's reader
/// instead; which one 1.7 does is unread.
pub fn read_raw_string(r: &mut MsgReader) -> Vec<u8> {
    read_string_capped(r, usize::MAX)
}

/// [`read_raw_string`] keeping at most `max` bytes, the rest read to the
/// NUL and dropped, as a fixed buffer's read does.
pub fn read_string_capped(r: &mut MsgReader, max: usize) -> Vec<u8> {
    let mut out = Vec::new();
    loop {
        let c = r.read_byte();
        if c == 0 || r.is_overflowed() {
            return out;
        }
        if out.len() < max {
            out.push(c);
        }
    }
}

/// One gamestate op in wire order.
#[derive(Clone, Debug)]
pub enum GamestateOp {
    /// One `svc_configstring`: its slots and strings as sent.
    Configstrings(Vec<(u16, Vec<u8>)>),
    Baseline(u32, Wire),
}

#[derive(Clone, Debug, Default)]
pub struct Gamestate {
    pub server_command_sequence: i32,
    /// The strings the message carried. A slot left out holds its
    /// `constantConfigStrings` default: [`Gamestate::configstring`] fills it.
    pub configstrings: BTreeMap<u16, Vec<u8>>,
    pub baselines: BTreeMap<u32, EntityState>,
    pub client_num: i32,
    pub checksum_feed: i32,
    pub map_center: [f32; 3],
    pub ops: Vec<GamestateOp>,
}

impl Gamestate {
    /// A slot as the client holds it: the string sent, else the slot's
    /// `constantConfigStrings` default, else `""`.
    pub fn configstring(&self, i: u16) -> &[u8] {
        match self.configstrings.get(&i) {
            Some(s) => s,
            None => constant_cs::default_of(i).map_or(&[], str::as_bytes),
        }
    }

    /// Every slot, `MAX_CONFIGSTRINGS` of them, as [`Self::configstring`].
    pub fn all_configstrings(&self) -> Vec<&[u8]> {
        (0..MAX_CONFIGSTRINGS as u16)
            .map(|i| self.configstring(i))
            .collect()
    }

    /// A key of the systeminfo string (`sv_serverid`).
    pub fn systeminfo(&self, key: &str) -> Option<String> {
        let s = String::from_utf8_lossy(self.configstring(CS_SYSTEMINFO));
        vcod_common::net::info_value_for_key(&s, key).map(str::to_string)
    }
}

/// How a snapshot's deltas went out: the choices its states do not show.
#[derive(Clone, Debug, Default)]
pub struct SnapshotWire {
    pub ps: PsWire,
    /// Every entity the message named, in order.
    pub entities: Vec<(u32, Wire)>,
    pub clients: Vec<(u32, Wire)>,
}

#[derive(Clone, Debug)]
pub struct Snapshot {
    /// The netchan sequence of the message that carried it.
    pub message_num: i32,
    pub server_time: Msec,
    /// The delta byte: how many messages back its base went out, 0 for
    /// none. A byte that reaches message 0 or before has no base either.
    pub delta_num: i32,
    /// Index into [`Client::snapshots`] of the snapshot it deltas from.
    pub base: Option<usize>,
    pub snap_flags: u8,
    pub ps: PlayerState,
    /// By number.
    pub entities: Vec<EntityState>,
    pub clients: Vec<ClientState>,
    pub wire: SnapshotWire,
}

#[derive(Clone, Debug)]
pub enum Op {
    ServerCommand {
        sequence: i32,
        text: Vec<u8>,
    },
    /// [`Client::gamestate`].
    Gamestate,
    /// Index into [`Client::snapshots`].
    Snapshot(usize),
    /// One `svc_download` (cod4-handshake.md 11).
    Download(super::download::DownloadMsg),
    Nop,
}

/// One message's walk: its ops, and where the reader stood at `svc_EOF`.
#[derive(Clone, Debug)]
pub struct Parsed {
    pub ops: Vec<Op>,
    /// `readcount` after the `svc_EOF` byte.
    pub eof_at: usize,
    /// The compressed bits of the bytes up to `svc_EOF` fill the block to
    /// its last byte: the walk used the whole message and no more.
    pub exact: bool,
}

/// The client side's state across messages.
#[derive(Default)]
pub struct Client {
    pub huff: Huffman,
    pub gamestate: Option<Gamestate>,
    /// Every snapshot parsed, in order, but those [`Self::shed`] dropped.
    pub snapshots: Vec<Snapshot>,
    /// How many of the oldest snapshots [`Self::shed`] dropped.
    pub shed_count: usize,
    /// Message number to index in `snapshots`, for deltas.
    ring: [Option<(i32, usize)>; PACKET_BACKUP],
    /// The newest server command and its text (keys the client's messages).
    pub server_command_sequence: i32,
    pub server_commands: BTreeMap<i32, Vec<u8>>,
    /// `cl.clientArchive`: the rows a live client adds as each move goes
    /// out, or a demo's archive records; a playerstate sent without
    /// origin takes its predicted fields off it (cod4-netfields.md 13).
    pub archive: super::demo::ArchiveRing,
}

impl Client {
    pub fn new() -> Client {
        Client::default()
    }

    /// Snapshots parsed so far, those shed included: counts on as
    /// [`Self::snapshots`] stops growing.
    pub fn total_snapshots(&self) -> usize {
        self.shed_count + self.snapshots.len()
    }

    /// Drops all but the newest `keep` snapshots (at least
    /// `PACKET_BACKUP`, which a delta can reach) and returns how many went:
    /// every index into [`Self::snapshots`] moves down by that. A live
    /// connection sheds; a capture keeps its every snapshot.
    pub fn shed(&mut self, keep: usize) -> usize {
        let keep = keep.max(PACKET_BACKUP);
        let n = self.snapshots.len().saturating_sub(keep);
        if n == 0 {
            return 0;
        }
        self.snapshots.drain(..n);
        for s in &mut self.snapshots {
            s.base = s.base.and_then(|b| b.checked_sub(n));
        }
        for r in &mut self.ring {
            *r = r.and_then(|(num, i)| Some((num, i.checked_sub(n)?)));
        }
        self.shed_count += n;
        n
    }

    /// Text of the newest server command, `""` before any.
    pub fn last_server_command(&self) -> &[u8] {
        self.server_command(self.server_command_sequence)
    }

    /// Text of server command `seq`, `""` before any.
    pub fn server_command(&self, seq: i32) -> &[u8] {
        self.server_commands.get(&seq).map_or(&[], Vec::as_slice)
    }

    /// Walks one message's block (the bytes past the plain
    /// `reliableAcknowledge`, unscrambled) as `CL_ParseServerMessage` does.
    /// A message retail marks overflowed fails with [`Illegible`] in its
    /// chain: one read past its end, an unknown op, or a delta base the
    /// ring does not hold.
    pub fn parse(&mut self, sequence: u32, block: &[u8]) -> Result<Parsed> {
        let plain = self.huff.decompress(block);
        let mut r = MsgReader::from_plain(plain);
        let mut ops = Vec::new();
        match self.parse_ops(&mut r, sequence, &mut ops) {
            Err(e) if r.is_overflowed() && e.downcast_ref::<Illegible>().is_none() => {
                return Err(e.context(Illegible));
            }
            done => done?,
        }
        let eof_at = r.readcount();
        let bits: usize = r.plain()[..eof_at]
            .iter()
            .map(|&c| self.huff.code_len(c))
            .sum();
        Ok(Parsed {
            ops,
            eof_at,
            exact: bits.div_ceil(8) == block.len(),
        })
    }

    /// [`Self::parse`]'s op loop, to `svc_EOF`.
    fn parse_ops(&mut self, r: &mut MsgReader, sequence: u32, ops: &mut Vec<Op>) -> Result<()> {
        loop {
            let op = r.read_byte();
            ensure!(!r.is_overflowed(), "message ran out before svc_EOF");
            match op {
                SVC_EOF => return Ok(()),
                SVC_NOP => ops.push(Op::Nop),
                SVC_SERVERCOMMAND => {
                    let seq = r.read_long();
                    let text = read_raw_string(r);
                    if seq > self.server_command_sequence {
                        self.server_command_sequence = seq;
                    }
                    self.server_commands
                        .entry(seq)
                        .or_insert_with(|| text.clone());
                    // Older than two reliable rings: nothing keys or walks it.
                    let floor = self
                        .server_command_sequence
                        .saturating_sub(KEPT_SERVER_COMMANDS);
                    while let Some(e) = self.server_commands.first_entry() {
                        if *e.key() > floor {
                            break;
                        }
                        e.remove();
                    }
                    ops.push(Op::ServerCommand {
                        sequence: seq,
                        text,
                    });
                }
                SVC_GAMESTATE => {
                    let gs = parse_gamestate(r)?;
                    self.server_command_sequence = gs.server_command_sequence;
                    self.gamestate = Some(gs);
                    self.ring = [None; PACKET_BACKUP];
                    ops.push(Op::Gamestate);
                }
                SVC_SNAPSHOT => {
                    let snap = self.parse_snapshot(r, sequence as i32)?;
                    let i = self.snapshots.len();
                    self.ring[snap.message_num as usize % PACKET_BACKUP] =
                        Some((snap.message_num, i));
                    self.snapshots.push(snap);
                    ops.push(Op::Snapshot(i));
                }
                super::download::SVC_DOWNLOAD => {
                    ops.push(Op::Download(super::download::DownloadMsg::read(r)));
                }
                o => return Err(anyhow!("op {o} at byte {}", r.readcount() - 1).context(Illegible)),
            }
        }
    }

    fn parse_snapshot(&self, r: &mut MsgReader, message_num: i32) -> Result<Snapshot> {
        let gs = self
            .gamestate
            .as_ref()
            .context("snapshot before the gamestate")?;
        let server_time = Msec::from_raw(r.read_long());
        let delta = r.read_byte() as i32;
        let snap_flags = r.read_byte();
        // Retail's wrapping `int` (iw3mp.exe 0x47378a): a demo's sequence
        // is any word. A zero byte reads as -1, and a base of 0 or less
        // takes the no-base arm (`jg` at 0x4737d1).
        let num = if delta == 0 {
            -1
        } else {
            message_num.wrapping_sub(delta)
        };
        let base = if num <= 0 {
            None
        } else {
            match self.ring[num.rem_euclid(PACKET_BACKUP as i32) as usize] {
                Some((n, i)) if n == num => Some(i),
                _ => {
                    return Err(anyhow!(
                        "snapshot {message_num} deltas from {num}, which I don't hold"
                    )
                    .context(Illegible));
                }
            }
        };
        let old = base.map(|i| &self.snapshots[i]);
        let ctx = Ctx {
            time: server_time,
            map_center: gs.map_center,
        };
        let mut wire = SnapshotWire::default();
        let (mut ps, ps_wire) = delta::read_delta_playerstate(r, &ctx, old.map(|o| &o.ps))
            .with_context(|| format!("snapshot {message_num} playerstate"))?;
        if !ps.origin_sent {
            self.archive
                .fill_unsent(&mut ps, old.map_or(&*delta::ZERO_PS, |o| &o.ps));
        }
        wire.ps = ps_wire;

        let mut entities = Vec::new();
        let old_ents: &[EntityState] = old.map_or(&[], |o| &o.entities);
        let mut oi = 0;
        let mut last = -1;
        loop {
            let num = delta::read_entity_index(r, &mut last, GENTITYNUM_BITS)?;
            ensure!(
                !r.is_overflowed(),
                "snapshot {message_num}: entities ran out"
            );
            if num == ENTITYNUM_NONE {
                break;
            }
            ensure!(num < ENTITYNUM_NONE, "entity number {num}");
            while oi < old_ents.len() && old_ents[oi].number() < num {
                entities.push(old_ents[oi]);
                oi += 1;
            }
            let base = if oi < old_ents.len() && old_ents[oi].number() == num {
                oi += 1;
                old_ents[oi - 1]
            } else {
                gs.baselines.get(&num).copied().unwrap_or_default()
            };
            let (e, w) = delta::read_delta_entity(r, &ctx, &base, num)
                .with_context(|| format!("snapshot {message_num} entity {num}"))?;
            entities.extend(e);
            wire.entities.push((num, w));
        }
        entities.extend_from_slice(&old_ents[oi..]);

        let mut clients = Vec::new();
        let old_cls: &[ClientState] = old.map_or(&[], |o| &o.clients);
        let mut oi = 0;
        let mut last = -1;
        while r.read_bits(1) != 0 {
            let num = delta::read_entity_index(r, &mut last, CLIENTNUM_BITS)?;
            ensure!(
                !r.is_overflowed(),
                "snapshot {message_num}: clients ran out"
            );
            while oi < old_cls.len() && old_cls[oi].0[0] < num {
                clients.push(old_cls[oi]);
                oi += 1;
            }
            let base = if oi < old_cls.len() && old_cls[oi].0[0] == num {
                oi += 1;
                old_cls[oi - 1]
            } else {
                ClientState::default()
            };
            let (c, w) = delta::read_delta_client(r, &ctx, &base, num)?;
            clients.extend(c);
            wire.clients.push((num, w));
        }
        clients.extend_from_slice(&old_cls[oi..]);
        ensure!(
            !r.is_overflowed(),
            "snapshot {message_num} read past the message"
        );
        Ok(Snapshot {
            message_num,
            server_time,
            delta_num: delta,
            base,
            snap_flags,
            ps,
            entities,
            clients,
            wire,
        })
    }

    /// The plain bytes of a message just parsed, `svc_EOF` included: the
    /// ops of `parsed` written back from the states and wire choices they
    /// recorded. Call it before the next message's parse, which may replace
    /// the gamestate. Compressed with [`Self::huff`], it is the block.
    pub fn write(&self, parsed: &Parsed) -> Result<Vec<u8>> {
        let mut w = MsgWriter::new(&UNUSED_HUFF);
        for op in &parsed.ops {
            match op {
                Op::Nop => w.write_byte(SVC_NOP),
                Op::ServerCommand { sequence, text } => {
                    w.write_byte(SVC_SERVERCOMMAND);
                    w.write_long(*sequence);
                    w.write_string_bytes(text);
                }
                Op::Gamestate => {
                    let gs = self.gamestate.as_ref().context("no gamestate")?;
                    write_gamestate(&mut w, gs);
                }
                Op::Snapshot(i) => self.write_snapshot(&mut w, &self.snapshots[*i])?,
                Op::Download(d) => d.write(&mut w),
            }
        }
        w.write_byte(SVC_EOF);
        Ok(w.into_ops())
    }

    fn write_snapshot(&self, w: &mut MsgWriter, s: &Snapshot) -> Result<()> {
        let gs = self.gamestate.as_ref().context("no gamestate")?;
        let old = s.base.map(|i| &self.snapshots[i]);
        let ctx = Ctx {
            time: s.server_time,
            map_center: gs.map_center,
        };
        w.write_byte(SVC_SNAPSHOT);
        w.write_long(s.server_time.raw());
        w.write_byte(s.delta_num as u8);
        w.write_byte(s.snap_flags);
        delta::write_delta_playerstate(w, &ctx, old.map(|o| &o.ps), &s.ps, &s.wire.ps);

        let old_ents: &[EntityState] = old.map_or(&[], |o| &o.entities);
        let mut last = -1;
        for (num, wire) in &s.wire.entities {
            delta::write_entity_index(w, &mut last, *num, GENTITYNUM_BITS);
            let from = old_ents
                .iter()
                .find(|e| e.number() == *num)
                .or_else(|| gs.baselines.get(num))
                .copied()
                .unwrap_or_default();
            let to = s
                .entities
                .iter()
                .find(|e| e.number() == *num)
                .copied()
                .unwrap_or(from);
            delta::write_delta_entity(w, &ctx, &from, &to, wire);
        }
        delta::write_entity_index(w, &mut last, ENTITYNUM_NONE, GENTITYNUM_BITS);

        let old_cls: &[ClientState] = old.map_or(&[], |o| &o.clients);
        let mut last = -1;
        for (num, wire) in &s.wire.clients {
            w.write_bits(1, 1);
            delta::write_entity_index(w, &mut last, *num, CLIENTNUM_BITS);
            let from = old_cls
                .iter()
                .find(|c| c.0[0] == *num)
                .copied()
                .unwrap_or_default();
            let to = s
                .clients
                .iter()
                .find(|c| c.0[0] == *num)
                .copied()
                .unwrap_or(from);
            delta::write_delta_client(w, &ctx, &from, &to, wire);
        }
        w.write_bits(0, 1);
        Ok(())
    }
}

/// `CL_ParseGamestate`, after its `svc_gamestate` byte.
pub fn parse_gamestate(r: &mut MsgReader) -> Result<Gamestate> {
    let mut gs = Gamestate {
        server_command_sequence: r.read_long(),
        ..Gamestate::default()
    };
    let mut last_entity = -1;
    loop {
        let op = r.read_byte();
        ensure!(!r.is_overflowed(), "gamestate ran out");
        match op {
            SVC_EOF => break,
            SVC_CONFIGSTRING => {
                let count = r.read_short() as u16;
                let mut last: i32 = -1;
                let mut sent = Vec::with_capacity(count as usize);
                for _ in 0..count {
                    let i = if r.read_bits(1) != 0 {
                        last + 1
                    } else {
                        r.read_bits(12)
                    };
                    ensure!((i as u32) < MAX_CONFIGSTRINGS, "configstring {i}");
                    let s = read_raw_string(r);
                    gs.configstrings.insert(i as u16, s.clone());
                    sent.push((i as u16, s));
                    last = i;
                }
                gs.map_center = super::wire::map_center(gs.configstring(CS_MAPCENTER));
                gs.ops.push(GamestateOp::Configstrings(sent));
            }
            SVC_BASELINE => {
                let num = delta::read_entity_index(r, &mut last_entity, GENTITYNUM_BITS)?;
                ensure!(num < 1024, "baseline {num}");
                let ctx = Ctx {
                    time: Msec::ZERO,
                    map_center: gs.map_center,
                };
                let (e, wire) = delta::read_delta_entity(r, &ctx, &EntityState::default(), num)?;
                let e = e.with_context(|| format!("baseline {num} removed"))?;
                gs.baselines.insert(num, e);
                gs.ops.push(GamestateOp::Baseline(num, wire));
            }
            o => bail!("gamestate op {o} at byte {}", r.readcount() - 1),
        }
    }
    gs.client_num = r.read_long();
    gs.checksum_feed = r.read_long();
    ensure!(!r.is_overflowed(), "gamestate ran out before checksumFeed");
    Ok(gs)
}

/// `SV_SendClientGameState`'s body, after the server commands: each
/// configstring's index explicit when it does not follow the last one or
/// when its string is empty (the server writes an emptied default that
/// way, INFERRED), then the baselines.
pub fn write_gamestate(w: &mut MsgWriter, gs: &Gamestate) {
    w.write_byte(SVC_GAMESTATE);
    w.write_long(gs.server_command_sequence);
    let mut last_entity = -1;
    let ctx = Ctx {
        time: Msec::ZERO,
        map_center: gs.map_center,
    };
    for op in &gs.ops {
        match op {
            GamestateOp::Configstrings(sent) => {
                w.write_byte(SVC_CONFIGSTRING);
                w.write_short(sent.len() as i16);
                let mut last: i32 = -1;
                for (i, s) in sent {
                    let i = *i as i32;
                    if i == last + 1 && !s.is_empty() {
                        w.write_bits(1, 1);
                    } else {
                        w.write_bits(0, 1);
                        w.write_bits(i, 12);
                    }
                    w.write_string_bytes(s);
                    last = i;
                }
            }
            GamestateOp::Baseline(num, wire) => {
                w.write_byte(SVC_BASELINE);
                delta::write_entity_index(w, &mut last_entity, *num, GENTITYNUM_BITS);
                let to = gs.baselines.get(num).copied().unwrap_or_default();
                delta::write_delta_entity(w, &ctx, &EntityState::default(), &to, wire);
            }
        }
    }
    w.write_byte(SVC_EOF);
    w.write_long(gs.client_num);
    w.write_long(gs.checksum_feed);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A delta snapshot sent without origin (its playerstate's first bit
    /// 0) takes origin, velocity, view, `bobCycle` and `movementDir` off
    /// the archive row for its `commandTime`, as 1.7's reader does
    /// (`iw3mp.exe` 0x507252; cod4-netfields.md 13), not the base's.
    #[test]
    fn a_playerstate_sent_without_origin_takes_the_archive_row() {
        use super::super::demo::Archive;
        use super::super::wire::{EOF, Frame4, write_snapshot};
        use crate::ps_word;
        use vcod_common::net::protocol::SnapshotHead;
        let mut c = Client::new();
        c.gamestate = Some(Gamestate::default());
        let mut ps1 = delta::PlayerState::default();
        ps1.words[ps_word!("commandTime")] = 1000;
        for (w, v) in ps_word!(3 "origin").into_iter().zip([10.0f32, 20.0, 30.0]) {
            ps1.words[w] = v.to_bits();
        }
        let huff = Huffman::new();
        let message = |w: MsgWriter| huff.compress(&w.into_ops());
        let mut w = MsgWriter::new(&UNUSED_HUFF);
        let frame = Frame4 {
            server_time: Msec::from_raw(1000),
            map_center: Default::default(),
            ps: ps1.clone(),
            entities: Vec::new(),
            clients: Vec::new(),
        };
        let head = SnapshotHead {
            delta_num: 0,
            snap_flags: 0,
        };
        write_snapshot(&mut w, None, &frame, head, &BTreeMap::new());
        w.write_byte(EOF);
        let first = message(w);
        c.parse(1, &first).unwrap();
        // The row the client sent with its move at 1050, between two
        // others.
        let row = |t: i32, x: f32| Archive {
            server_time: t,
            origin: [x, 2.0, 3.0],
            velocity: [4.0, 5.0, 6.0],
            bob_cycle: 7,
            movement_dir: -8,
            view_angles: [9.0, 10.0, 11.0],
        };
        for (t, x) in [(1000, 1.0), (1050, 100.0), (1100, 200.0)] {
            c.archive.add(row(t, x));
        }
        let mut ps2 = ps1.clone();
        ps2.words[ps_word!("commandTime")] = 1050;
        ps2.words[ps_word!("bobCycle")] = 99;
        ps2.origin_sent = false;
        let ctx = Ctx {
            time: Msec::from_raw(1050),
            map_center: Default::default(),
        };
        let mut w = MsgWriter::new(&UNUSED_HUFF);
        w.write_byte(SVC_SNAPSHOT);
        w.write_long(1050);
        w.write_byte(1);
        w.write_byte(0);
        let wire = delta::PsWire::natural(&ps1, &ps2);
        delta::write_delta_playerstate(&mut w, &ctx, Some(&ps1), &ps2, &wire);
        delta::write_entity_index(&mut w, &mut -1, delta::ENTITYNUM_NONE, GENTITYNUM_BITS);
        w.write_bits(0, 1);
        w.write_byte(EOF);
        let second = message(w);
        c.parse(2, &second).unwrap();
        let ps = &c.snapshots[1].ps;
        assert!(!ps.origin_sent);
        let f = |w: usize| ps.word_f32(w);
        assert_eq!(ps_word!(3 "origin").map(f), [100.0, 2.0, 3.0]);
        assert_eq!(ps_word!(3 "velocity").map(f), [4.0, 5.0, 6.0]);
        assert_eq!(ps_word!(3 "viewangles").map(f), [9.0, 10.0, 11.0]);
        assert_eq!(ps.word(ps_word!("bobCycle")), 7);
        assert_eq!(ps.word(ps_word!("movementDir")) as i32, -8);
        assert_eq!(ps.word(ps_word!("commandTime")), 1050);
    }

    /// A long connection keeps a bounded window of server commands, and a
    /// `d` past the configstring table changes nothing.
    #[test]
    fn server_commands_and_configstrings_stay_bounded() {
        let mut c = Client::new();
        c.gamestate = Some(Gamestate::default());
        let mut applied = 0;
        for seq in 1..=100_000 {
            let parsed = Parsed {
                ops: vec![Op::ServerCommand {
                    sequence: seq,
                    text: Vec::new(),
                }],
                eof_at: 0,
                exact: true,
            };
            let block = c.huff.compress(&c.write(&parsed).unwrap());
            let parsed = c.parse(seq as u32, &block).unwrap();
            super::super::conn::apply_configstrings(&mut c, &mut applied, &parsed);
        }
        assert_eq!(c.server_command_sequence, 100_000);
        assert!(c.server_commands.len() <= KEPT_SERVER_COMMANDS as usize);
        assert!(c.server_commands.contains_key(&100_000));

        let d = |seq: i32, text: &str| Parsed {
            ops: vec![Op::ServerCommand {
                sequence: seq,
                text: text.as_bytes().to_vec(),
            }],
            eof_at: 0,
            exact: true,
        };
        super::super::conn::apply_configstrings(&mut c, &mut applied, &d(100_001, "d 65535 x"));
        super::super::conn::apply_configstrings(&mut c, &mut applied, &d(100_002, "d 2441 y"));
        let gs = c.gamestate.as_ref().unwrap();
        assert!(!gs.configstrings.contains_key(&65535));
        assert_eq!(gs.configstring(2441), b"y");
    }
}
