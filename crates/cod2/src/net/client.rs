//! A CoD2 client connection's wire state, no socket: feed it the server's
//! packets, ask it for the next one to send (docs/research/cod2-protocol.md
//! 2-4; `CL_WritePacket`, Mac 0x188154).

use super::demo::Recording;
use super::ops::{CLC_BITS, Clc, MAX_RELIABLE_COMMANDS};
use super::parse::{Parsed, Parser};
use super::usercmd::{MAX_PACKET_USERCMDS, UserCmd2, default_usercmd, write_delta_usercmd};
use anyhow::Context;
use vcod_common::net::codec::MsgWriter;
use vcod_common::net::com_hash_key_bytes;
use vcod_common::net::huffman::Huffman;
use vcod_common::net::netchan::{Netchan, NetchanParams};

pub struct Client {
    pub netchan: Netchan,
    pub parser: Parser,
    huff: Huffman,
    /// `clc.reliableCommands`: what the client sent, keyed on by the server's
    /// scramble of its replies.
    reliable_commands: Vec<Vec<u8>>,
    pub reliable_sequence: i32,
    /// The server's `reliableAcknowledge` of our commands.
    pub reliable_acknowledge: i32,
    /// `cl.serverId`: the gamestate's `sv_serverid`, then each systeminfo
    /// update's ([`Self::set_systeminfo`]).
    server_id: u8,
    /// `record`'s file while one runs.
    recording: Option<Recording>,
}

impl Client {
    /// After `connectResponse`: the challenge and our qport key the netchan.
    pub fn new(qport: u16, challenge: i32) -> Self {
        Client {
            netchan: Netchan::with_params(qport, challenge, NetchanParams::COD2),
            parser: Parser::new(),
            huff: Huffman::new(),
            reliable_commands: vec![Vec::new(); MAX_RELIABLE_COMMANDS],
            reliable_sequence: 0,
            reliable_acknowledge: 0,
            server_id: 0,
            recording: None,
        }
    }

    /// A sequenced packet from the server. `None` while fragments are joined
    /// or for a stale packet.
    pub fn receive(&mut self, packet: &[u8]) -> anyhow::Result<Option<Parsed>> {
        if packet.len() < 4 {
            return Ok(None);
        }
        let Some((sequence, mut body)) = self.netchan.frame(packet)? else {
            return Ok(None);
        };
        let ack = i32::from_le_bytes(body.get(..4).context("short message")?.try_into()?);
        let cmd = &self.reliable_commands[ack as usize % MAX_RELIABLE_COMMANDS];
        self.netchan.decode(sequence, cmd, &mut body);
        self.server_message(sequence as i32, &body).map(Some)
    }

    /// A live server message past the netchan (the acknowledge, then the
    /// unscrambled, still compressed op block), parsed, then written to
    /// the demo being recorded unless it still waits for a full frame
    /// (`CL_PacketEvent`, Mac 0x14ced6).
    pub fn server_message(&mut self, sequence: i32, msg: &[u8]) -> anyhow::Result<Parsed> {
        let parsed = self.message(sequence, msg)?;
        if let Some(rec) = self.recording.as_mut().filter(|r| !r.waiting)
            && let Err(e) = rec.write(sequence, msg)
        {
            log::warn!("demo {}: {e}, recording stopped", rec.name);
            self.recording = None;
        }
        Ok(parsed)
    }

    /// A demo record's message (`CL_ReadDemoMessage`, Mac 0x14c132): one
    /// whose acknowledge is more than 128 behind our commands is skipped,
    /// as a live one is.
    pub fn demo_message(&mut self, sequence: i32, msg: &[u8]) -> anyhow::Result<Option<Parsed>> {
        let ack = i32::from_le_bytes(msg.get(..4).context("short message")?.try_into()?);
        if ack < self.reliable_sequence - MAX_RELIABLE_COMMANDS as i32 {
            return Ok(None);
        }
        self.netchan.incoming_sequence = sequence as u32;
        self.message(sequence, msg).map(Some)
    }

    /// One server message past the netchan, parsed.
    fn message(&mut self, sequence: i32, msg: &[u8]) -> anyhow::Result<Parsed> {
        let ack = i32::from_le_bytes(msg.get(..4).context("short message")?.try_into()?);
        let plain = self.huff.decompress_block(&msg[4..]);
        self.reliable_acknowledge = ack;
        let parsed = self.parser.parse(sequence, &plain)?;
        if parsed.gamestate
            && let Some(gs) = &self.parser.gamestate
        {
            let si = gs.configstring(1).to_string();
            self.set_systeminfo(&si);
        }
        // `CL_ParseSnapshot` (Mac 0x1d5cb2) ends the wait on a frame
        // that deltas from nothing.
        if let Some(n) = parsed.snapshot
            && self.parser.frame(n).is_some_and(|f| f.delta_num < 0)
            && let Some(rec) = self.recording.as_mut()
        {
            rec.waiting = false;
        }
        Ok(parsed)
    }

    /// `record` (`CL_Record_f`, Mac 0x149856): opens `out` on a gamestate
    /// of the client's own, `configstrings` the live table. Refused while
    /// recording or before a gamestate, with retail's line.
    pub fn start_recording(
        &mut self,
        out: Box<dyn std::io::Write + Send>,
        name: &str,
        configstrings: &[String],
    ) -> anyhow::Result<()> {
        anyhow::ensure!(self.recording.is_none(), "Already recording.");
        let gs = self
            .parser
            .gamestate
            .as_ref()
            .context("You must be in a level to record.")?;
        let msg =
            super::demo::gamestate_message(&self.huff, self.reliable_sequence, gs, configstrings);
        let sequence = self.netchan.incoming_sequence as i32;
        self.recording = Some(Recording::start(out, name, sequence, &msg)?);
        Ok(())
    }

    /// `stoprecord`: the trailer written and the file closed; `None` when
    /// nothing records, else the bytes written.
    pub fn stop_recording(&mut self) -> Option<std::io::Result<u64>> {
        self.recording.take().map(Recording::finish)
    }

    /// The demo's name and the bytes written so far, while recording.
    pub fn recording_status(&self) -> Option<(&str, u64)> {
        self.recording
            .as_ref()
            .map(|r| (r.name.as_str(), r.written()))
    }

    /// `CL_SystemInfoChanged`'s `sv_serverid`: a `d 1` after a
    /// `fast_restart` carries the new id, and a move under the old one is
    /// not read (cod2-protocol.md 4.1, 1.9).
    pub fn set_systeminfo(&mut self, systeminfo: &str) {
        if let Some(id) = super::oob::info_value_for_key(systeminfo, "sv_serverid")
            .and_then(|v| v.parse::<i32>().ok())
        {
            self.server_id = id as u8;
        }
    }

    /// Queue a client command (`CL_AddReliableCommand`).
    pub fn add_reliable(&mut self, text: &str) {
        self.reliable_sequence += 1;
        self.reliable_commands[self.reliable_sequence as usize % MAX_RELIABLE_COMMANDS] =
            text.bytes().collect();
    }

    /// The reliable command numbered `seq`, while the ring still holds it.
    pub fn reliable_command(&self, seq: i32) -> &[u8] {
        &self.reliable_commands[seq as usize % MAX_RELIABLE_COMMANDS]
    }

    /// `sv_serverid`, as the header's byte; 0 before the gamestate.
    pub fn server_id(&self) -> u8 {
        self.server_id
    }

    /// Whether the next move can delta: the newest message carried a valid
    /// snapshot (`cl.snap.valid && serverMessageSequence == cl.snap.messageNum`).
    /// A record waiting for its full frame asks for one (`CL_WritePacket`,
    /// Mac 0x188154, `clc` +0x40190).
    pub fn can_delta(&self) -> bool {
        self.parser.last_snapshot == Some(self.netchan.incoming_sequence as i32)
            && !self.recording.as_ref().is_some_and(|r| r.waiting)
    }

    /// The next packet: unacknowledged commands, `cmds` as `clc_move` or
    /// `clc_moveNoDelta` (none: no move op), then `clc_EOF`.
    pub fn build_packet(&mut self, cmds: &[UserCmd2]) -> anyhow::Result<Vec<u8>> {
        let mut w = MsgWriter::new(&self.huff);
        for seq in self.reliable_acknowledge + 1..=self.reliable_sequence {
            w.write_bits(Clc::ClientCommand as i32, CLC_BITS);
            w.write_long(seq);
            w.write_string_bytes(&self.reliable_commands[seq as usize % MAX_RELIABLE_COMMANDS]);
        }
        let message_ack = self.netchan.incoming_sequence as i32;
        let rel_ack = self.parser.server_command_sequence;
        let server_cmd = self.parser.server_command(rel_ack).to_vec();
        if !cmds.is_empty() {
            anyhow::ensure!(cmds.len() <= MAX_PACKET_USERCMDS, "{} cmds", cmds.len());
            let op = if self.can_delta() {
                Clc::Move
            } else {
                Clc::MoveNoDelta
            };
            w.write_bits(op as i32, CLC_BITS);
            w.write_byte(cmds.len() as u8);
            let feed = self
                .parser
                .gamestate
                .as_ref()
                .map_or(0, |g| g.checksum_feed);
            let key = (feed ^ message_ack ^ com_hash_key_bytes(&server_cmd, 32)) as u32;
            let mut from = self
                .parser
                .snapshot()
                .map(|s| default_usercmd(&s.ps))
                .unwrap_or_default();
            for c in cmds {
                write_delta_usercmd(&mut w, key, &from, c);
                from = *c;
            }
        }
        w.write_bits(Clc::Eof as i32, CLC_BITS);
        let server_id = self.server_id();
        let ops = w.into_ops();
        self.netchan.build_out_keyed(
            i32::from(server_id),
            message_ack,
            rel_ack,
            &server_cmd,
            &ops,
            &self.huff,
        )
    }

    /// The cmd a still spectator sends: the default cmd at `server_time`, so
    /// it deltas as unchanged.
    pub fn idle_cmd(&self, server_time: i32) -> UserCmd2 {
        let base = self
            .parser
            .snapshot()
            .map(|s| default_usercmd(&s.ps))
            .unwrap_or_default();
        UserCmd2 {
            server_time,
            ..base
        }
    }
}
