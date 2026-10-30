//! Contains routines ported from the RTCW-MP GPL source, Copyright (C) 1999-2010 id Software LLC, a ZeniMax Media company.
//! See NOTICE.
//!
//! Netchan framing, fragment reassembly and the XOR scramble, both directions.
//! Port of RTCW net_chan.c, cl_net_chan.c and sv_net_chan.c; wire facts in
//! docs/protocol-1.1.md, "Netchan framing" and "XOR scramble".

use super::huffman::Huffman;
use super::msg::SVC_EOF;

/// `MAX_PACKETLEN`, net_chan.c:57.
pub const MAX_PACKETLEN: usize = 1400;
/// `FRAGMENT_SIZE`, net_chan.c:59.
pub const FRAGMENT_SIZE: usize = MAX_PACKETLEN - 100;
/// `FRAGMENT_BIT`, net_chan.c:62.
const FRAGMENT_BIT: u32 = 1 << 31;

/// `MAX_RELIABLE_COMMANDS`, 64 on CoD 1.1, not RTCW's 256 (CoDExtended
/// shared.h:135; `SV_UserMove` cod_lnxded 0x8086fa4, the mask at 0x8087060).
pub const MAX_RELIABLE_COMMANDS: usize = 64;
/// `SV_ENCODE_START` (qcommon.h:1152): past the plain `reliableAcknowledge`.
const SV_ENCODE_START: usize = 4;

/// A client packet's u32 sequence and u16 qport, ahead of the message.
const CLIENT_PACKET_PREFIX: usize = 4 + 2;
/// Plain client header: u8 `serverId`, i32 `messageAcknowledge`, i32
/// `reliableAcknowledge`.
const CLIENT_HEADER_LEN: usize = 1 + 4 + 4;

/// The netchan facts that differ between games. Every CoD1 protocol takes
/// [`Self::COD1`]; CoD2 1.3's are docs/research/cod2-protocol.md 2.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NetchanParams {
    /// Width of a fragment's start offset: 2 on CoD1, 4 on CoD2. The length
    /// after it is 2 on both.
    pub fragment_start_bytes: usize,
    /// `MAX_MSGLEN`, the cap on a reassembled message.
    pub max_msglen: usize,
    /// `MAX_RELIABLE_COMMANDS`, the size of both command rings.
    pub reliable_commands: usize,
    /// Whether the server-to-client key string reads as the server's
    /// `MSG_ReadString` stored it, 0x92 as `'` and every other high byte as
    /// `.` (CoD1), or raw (CoD2).
    pub clean_server_key: bool,
}

impl NetchanParams {
    pub const COD1: NetchanParams = NetchanParams {
        fragment_start_bytes: 2,
        max_msglen: 32768,
        reliable_commands: MAX_RELIABLE_COMMANDS,
        clean_server_key: true,
    };
    /// cod2-protocol.md 2.2-2.4: `Netchan_Process` (cod2_lnxded 0x806be8a)
    /// reads a long start, `MAX_MSGLEN` is 0x20000, the rings 128 slots.
    pub const COD2: NetchanParams = NetchanParams {
        fragment_start_bytes: 4,
        max_msglen: 0x20000,
        reliable_commands: 128,
        clean_server_key: false,
    };
}

pub struct Netchan {
    pub params: NetchanParams,
    pub qport: u16,
    pub challenge: i32,
    pub incoming_sequence: u32,
    pub outgoing_sequence: u32,
    /// Sent reliables, ring at `sequence & (len - 1)`; keys the decode of
    /// server messages.
    pub reliable: Vec<String>,
    pub reliable_sequence: u32,
    pub reliable_acknowledge: u32,
    /// Received server commands, same ring; keys the encode of ours.
    pub server_commands: Vec<String>,
    pub server_command_sequence: u32,
    /// Packets dropped between the last two accepted sequences.
    pub dropped: u32,
    /// Total over the connection.
    pub total_dropped: u64,
    frag_buf: Vec<u8>,
    frag_seq: u32,
}

impl Netchan {
    /// A CoD1 netchan ([`NetchanParams::COD1`]).
    pub fn new(qport: u16, challenge: i32) -> Self {
        Self::with_params(qport, challenge, NetchanParams::COD1)
    }

    pub fn with_params(qport: u16, challenge: i32, params: NetchanParams) -> Self {
        Netchan {
            params,
            qport,
            challenge,
            incoming_sequence: 0,
            outgoing_sequence: 1,
            reliable: vec![String::new(); params.reliable_commands],
            reliable_sequence: 0,
            reliable_acknowledge: 0,
            server_commands: vec![String::new(); params.reliable_commands],
            server_command_sequence: 0,
            dropped: 0,
            total_dropped: 0,
            frag_buf: Vec::new(),
            frag_seq: 0,
        }
    }

    /// `Netchan_Process` framing, then `CL_Netchan_Decode` keyed on
    /// [`Self::reliable`]. Returns the payload past the 4-byte sequence;
    /// `None` for stale, duplicate or non-final fragment packets; `Err` only
    /// when too short to be a netchan packet. The codec is unused here
    /// (CoD1's acknowledge is plain, see [`Self::decode`]) and kept for
    /// symmetry with [`Self::build_out`].
    pub fn process_in(
        &mut self,
        packet: &[u8],
        _huff: &Huffman,
    ) -> anyhow::Result<Option<Vec<u8>>> {
        let Some((sequence, mut payload)) = self.frame(packet)? else {
            return Ok(None);
        };
        let ack = if payload.len() >= 4 {
            i32::from_le_bytes(payload[..4].try_into().unwrap())
        } else {
            0
        };
        let key = self.reliable[ack as usize & (self.reliable.len() - 1)]
            .as_bytes()
            .to_vec();
        self.decode(sequence, &key, &mut payload);
        Ok(Some(payload))
    }

    /// `Netchan_Process` alone: the sequence and the payload past it, the
    /// fragments joined, still scrambled. `None` and `Err` as in
    /// [`Self::process_in`].
    pub fn frame(&mut self, packet: &[u8]) -> anyhow::Result<Option<(u32, Vec<u8>)>> {
        if packet.len() < 4 {
            anyhow::bail!("netchan packet of {} bytes", packet.len());
        }
        let raw = u32::from_le_bytes(packet[..4].try_into().unwrap());
        let fragmented = raw & FRAGMENT_BIT != 0;
        let sequence = raw & !FRAGMENT_BIT;
        // No qport on the server-to-client side.
        let mut read = 4usize;

        let (frag_start, frag_len) = if fragmented {
            let sb = self.params.fragment_start_bytes;
            if packet.len() < read + sb + 2 {
                return Ok(None); // truncated fragment header
            }
            let mut s = [0u8; 8];
            s[..sb].copy_from_slice(&packet[read..read + sb]);
            let s = u64::from_le_bytes(s) as usize;
            let l = u16::from_le_bytes(packet[read + sb..read + sb + 2].try_into().unwrap());
            read += sb + 2;
            (s, l as usize)
        } else {
            (0, 0)
        };

        if sequence <= self.incoming_sequence {
            return Ok(None);
        }
        // Server-supplied; wrap so a hostile value cannot panic in debug.
        self.dropped = sequence.wrapping_sub(self.incoming_sequence.wrapping_add(1));
        self.total_dropped += self.dropped as u64;

        if !fragmented {
            self.incoming_sequence = sequence;
            return Ok(Some((sequence, packet[4..].to_vec())));
        }

        // Fragments must arrive in order; a gap parks the set until the server
        // resends.
        if sequence != self.frag_seq {
            self.frag_seq = sequence;
            self.frag_buf.clear();
        }
        if frag_start != self.frag_buf.len() {
            return Ok(None);
        }
        if read + frag_len > packet.len() || self.frag_buf.len() + frag_len > self.params.max_msglen
        {
            return Ok(None); // illegal fragment length
        }
        self.frag_buf
            .extend_from_slice(&packet[read..read + frag_len]);
        if frag_len == FRAGMENT_SIZE {
            return Ok(None); // more to come
        }
        self.incoming_sequence = sequence;
        Ok(Some((sequence, std::mem::take(&mut self.frag_buf))))
    }

    /// `CL_Netchan_Decode` (cl_net_chan.c:99) over a [`Self::frame`]
    /// payload: bytes 4.. (packet bytes 8..) unscrambled in place, seeded
    /// `challenge ^ sequence` and keyed on `key`, the client's
    /// `reliableCommands[reliableAcknowledge]`. The `reliableAcknowledge`
    /// at 0..4 is plain on CoD1 (`SV_Netchan_Transmit` cod_lnxded
    /// 0x808f680) and CoD2, so it can be read before decoding.
    /// docs/protocol-1.1.md, "Message headers".
    pub fn decode(&self, sequence: u32, key: &[u8], payload: &mut [u8]) {
        if payload.len() <= 4 {
            return;
        }
        let seed = (self.challenge as u32 ^ sequence) as u8;
        scramble_server_block(seed, key, self.params.clean_server_key, &mut payload[4..]);
    }

    /// `CL_WritePacket` + `CL_Netchan_Transmit` keyed on
    /// [`Self::server_commands`]; see [`Self::build_out_keyed`].
    pub fn build_out(
        &mut self,
        server_id: i32,
        message_ack: i32,
        reliable_ack: i32,
        ops: &[u8],
        huff: &Huffman,
    ) -> anyhow::Result<Vec<u8>> {
        let key = self.server_commands[reliable_ack as usize & (self.server_commands.len() - 1)]
            .as_bytes()
            .to_vec();
        self.build_out_keyed(server_id, message_ack, reliable_ack, &key, ops, huff)
    }

    /// `[u32 sequence][u16 qport][9 plain header bytes][scrambled
    /// Huff_Compress(ops)]`. The header is plain with a one-byte `serverId`
    /// (`SV_PacketEvent` cod_lnxded 0x808c9d1) and the scramble covers the
    /// compressed block from its byte 0 (`SV_Netchan_Decode` 0x808de60),
    /// seeded `challenge ^ serverId ^ messageAcknowledge` and keyed on `key`,
    /// the client's `serverCommands[reliableAcknowledge]`.
    /// docs/protocol-1.1.md, "Message headers" and "XOR scramble".
    ///
    /// Outgoing fragmentation is not ported; an oversized message is an error
    /// rather than a packet the server drops silently.
    pub fn build_out_keyed(
        &mut self,
        server_id: i32,
        message_ack: i32,
        reliable_ack: i32,
        key: &[u8],
        ops: &[u8],
        huff: &Huffman,
    ) -> anyhow::Result<Vec<u8>> {
        let mut comp = huff.compress_block(ops);
        anyhow::ensure!(
            CLIENT_HEADER_LEN + comp.len() < FRAGMENT_SIZE,
            "client message of {} bytes needs fragmenting (limit {}); \
             outgoing fragmentation is not ported",
            CLIENT_HEADER_LEN + comp.len(),
            FRAGMENT_SIZE
        );
        let seed = self.challenge as u32 ^ server_id as u32 ^ message_ack as u32;
        scramble_client_ops(seed as u8, key, &mut comp);

        let mut pkt = Vec::with_capacity(CLIENT_PACKET_PREFIX + CLIENT_HEADER_LEN + comp.len());
        pkt.extend_from_slice(&self.outgoing_sequence.to_le_bytes());
        self.outgoing_sequence += 1;
        pkt.extend_from_slice(&self.qport.to_le_bytes());
        pkt.push(server_id as u8);
        pkt.extend_from_slice(&message_ack.to_le_bytes());
        pkt.extend_from_slice(&reliable_ack.to_le_bytes());
        pkt.extend_from_slice(&comp);
        Ok(pkt)
    }
}

/// One client's netchan on the server, the mirror of [`Netchan`]:
/// `SV_Netchan_Decode` (cod_lnxded 0x808de60) and `SV_Netchan_Encode` (RTCW
/// sv_net_chan.c:43).
pub struct ServerNetchan {
    pub params: NetchanParams,
    pub qport: u16,
    pub challenge: i32,
    pub incoming_sequence: u32,
    pub outgoing_sequence: u32,
    /// Packets lost between the last two accepted client sequences.
    pub dropped: u32,
    /// `svc_serverCommand`s sent, ring at `sequence & (len - 1)`; keys the
    /// decode of the client's messages.
    pub reliable: Vec<String>,
    pub reliable_sequence: u32,
    /// `lastClientCommandString`; keys the scramble of our messages.
    pub last_client_command_string: String,
}

/// Plain header plus the still-compressed op block.
pub struct ClientMessage {
    /// The packet's sequence; `incoming_sequence` once [`ServerNetchan::accept`] runs.
    pub sequence: u32,
    pub server_id: u8,
    pub message_ack: i32,
    pub reliable_ack: i32,
    pub ops: Vec<u8>,
}

impl ServerNetchan {
    /// A CoD1 netchan ([`NetchanParams::COD1`]).
    pub fn new(qport: u16, challenge: i32) -> Self {
        Self::with_params(qport, challenge, NetchanParams::COD1)
    }

    pub fn with_params(qport: u16, challenge: i32, params: NetchanParams) -> Self {
        ServerNetchan {
            params,
            qport,
            challenge,
            incoming_sequence: 0,
            outgoing_sequence: 1,
            dropped: 0,
            reliable: vec![String::new(); params.reliable_commands],
            reliable_sequence: 0,
            last_client_command_string: String::new(),
        }
    }

    /// `Netchan_Process` then `SV_Netchan_Decode`. `None` for stale, truncated
    /// or fragmented packets (client messages never fragment). The caller
    /// already matched the qport.
    ///
    /// Unlike `Netchan_Process` this commits nothing: the sequence only
    /// advances in [`Self::accept`], once the caller has checked the message.
    /// A packet forged with the client's address and qport otherwise stalls
    /// the real client behind a huge sequence until it times out.
    pub fn process_in(&self, packet: &[u8]) -> Option<ClientMessage> {
        if packet.len() < CLIENT_PACKET_PREFIX + CLIENT_HEADER_LEN {
            return None;
        }
        let raw = u32::from_le_bytes(packet[..4].try_into().unwrap());
        if raw & FRAGMENT_BIT != 0 {
            return None;
        }
        if raw <= self.incoming_sequence {
            return None;
        }
        let body = &packet[CLIENT_PACKET_PREFIX..];
        let server_id = body[0];
        let message_ack = i32::from_le_bytes(body[1..5].try_into().unwrap());
        let reliable_ack = i32::from_le_bytes(body[5..9].try_into().unwrap());
        let mut ops = body[CLIENT_HEADER_LEN..].to_vec();
        let string = self.reliable[reliable_ack as usize & (self.reliable.len() - 1)].as_bytes();
        let seed = self.challenge as u32 ^ u32::from(server_id) ^ message_ack as u32;
        scramble_client_ops(seed as u8, string, &mut ops);
        Some(ClientMessage {
            sequence: raw,
            server_id,
            message_ack,
            reliable_ack,
            ops,
        })
    }

    /// The sequence half of `Netchan_Process`, run for a message the caller
    /// has accepted.
    pub fn accept(&mut self, m: &ClientMessage) {
        self.dropped = m
            .sequence
            .wrapping_sub(self.incoming_sequence.wrapping_add(1));
        self.incoming_sequence = m.sequence;
    }

    /// `SV_Netchan_Transmit`: `[reliableAcknowledge][Huff_Compress(ops ++ svc_EOF)]`
    /// scrambled from byte 4, fragmented at `FRAGMENT_SIZE` under one sequence
    /// with a zero-length terminator on an exact multiple. Packets in send order.
    /// CoD1's `svc_EOF`; [`Self::transmit_eof`] takes the game's.
    pub fn transmit(&mut self, reliable_ack: i32, ops: &[u8], huff: &Huffman) -> Vec<Vec<u8>> {
        self.transmit_eof(reliable_ack, ops, SVC_EOF, huff)
    }

    /// [`Self::transmit`] closing the block with `eof`, the game's `svc_EOF`
    /// (CoD2's is 7, cod2-protocol.md 3.1).
    pub fn transmit_eof(
        &mut self,
        reliable_ack: i32,
        ops: &[u8],
        eof: u8,
        huff: &Huffman,
    ) -> Vec<Vec<u8>> {
        // The retail client needs the trailing `svc_EOF`; without it, it reads
        // the decompressor's pad symbol as an op and drops with "Illegible
        // server message" (docs/protocol-1.1.md, svc_gamestate).
        let mut body = Vec::with_capacity(ops.len() + 1);
        body.extend_from_slice(ops);
        body.push(eof);
        self.transmit_block(reliable_ack, &huff.compress_block(&body))
    }

    /// [`Self::transmit`] of a block the caller compressed itself, its
    /// `svc_EOF` inside: CoD4 1.7's Huffman code is not [`Huffman`]'s
    /// (cod4-handshake.md 4).
    pub fn transmit_block(&mut self, reliable_ack: i32, block: &[u8]) -> Vec<Vec<u8>> {
        let mut msg = reliable_ack.to_le_bytes().to_vec();
        msg.extend_from_slice(block);
        let string = self.last_client_command_string.as_bytes().to_vec();
        let mut key = (self.challenge as u32 ^ self.outgoing_sequence) as u8;
        let mut index = 0;
        // The 4-byte sequence keeps message and packet parity equal, so `i` from
        // SV_ENCODE_START matches the client's decode.
        for (i, byte) in msg.iter_mut().enumerate().skip(SV_ENCODE_START) {
            advance_key(
                &mut key,
                &string,
                &mut index,
                i,
                self.params.clean_server_key,
            );
            *byte ^= key;
        }

        let seq = self.outgoing_sequence;
        self.outgoing_sequence += 1;
        if msg.len() < FRAGMENT_SIZE {
            let mut pkt = seq.to_le_bytes().to_vec();
            pkt.extend_from_slice(&msg);
            return vec![pkt];
        }
        let mut pkts = Vec::new();
        let mut start = 0usize;
        loop {
            let len = FRAGMENT_SIZE.min(msg.len() - start);
            let mut pkt = (seq | FRAGMENT_BIT).to_le_bytes().to_vec();
            if self.params.fragment_start_bytes == 4 {
                pkt.extend_from_slice(&(start as u32).to_le_bytes());
            } else {
                pkt.extend_from_slice(&(start as u16).to_le_bytes());
            }
            pkt.extend_from_slice(&(len as u16).to_le_bytes());
            pkt.extend_from_slice(&msg[start..start + len]);
            pkts.push(pkt);
            start += len;
            if start == msg.len() && len != FRAGMENT_SIZE {
                break;
            }
        }
        pkts
    }
}

/// One scramble key step over the server's copy of the keyed client command.
/// Retail CoD1's `SV_Netchan_Transmit` walks `lastClientCommandString` raw
/// (cod_lnxded 0x808dc74), and that copy came through `MSG_ReadString`
/// (0x807f320), which maps 0x92 to `'` and every other byte over 127 to `.`
/// but keeps `%`. Mapping the client's raw copy the same way here gives both
/// ends one key stream; `clean` off walks it raw. An empty ring slot is all
/// NULs in the C and contributes nothing.
fn advance_key(key: &mut u8, string: &[u8], index: &mut usize, i: usize, clean: bool) {
    if *index >= string.len() {
        *index = 0;
    }
    let c = match string.get(*index).copied().unwrap_or(0) {
        0x92 if clean => b'\'',
        c if clean && c > 127 => b'.',
        c => c,
    };
    *key ^= c << (i & 1);
    *index += 1;
}

/// The server-to-client XOR walk over a message's block, the bytes past its
/// `reliableAcknowledge`, its own inverse: `CL_Netchan_Decode` and
/// `SV_Netchan_Transmit` both. Parity counts from the block's byte 0, which
/// is packet byte 8, so it matches the C's packet index. `clean` is
/// [`NetchanParams::clean_server_key`].
pub fn scramble_server_block(mut key: u8, string: &[u8], clean: bool, block: &mut [u8]) {
    let mut index = 0;
    for (i, b) in block.iter_mut().enumerate() {
        advance_key(&mut key, string, &mut index, i, clean);
        *b ^= key;
    }
}

/// The client op block's XOR walk, its own inverse, so
/// [`Netchan::build_out_keyed`] and [`ServerNetchan::process_in`] share it:
/// bare NUL wrap, no substitution, parity from the block's first byte
/// (`SV_Netchan_Decode` cod_lnxded 0x808de60, cod2-protocol.md 2.4).
pub fn scramble_client_ops(mut key: u8, string: &[u8], ops: &mut [u8]) {
    let mut index = 0usize;
    for (i, byte) in ops.iter_mut().enumerate() {
        if index >= string.len() {
            index = 0;
        }
        let c = string.get(index).copied().unwrap_or(0);
        key ^= c << (i & 1);
        index += 1;
        *byte ^= key;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::huffman::Huffman;
    /// `CL_DECODE_START` (qcommon.h:1155): payload byte 4, packet byte 8.
    const CL_DECODE_START: usize = 4;

    fn server_packet(seq: u32, payload: &[u8]) -> Vec<u8> {
        let mut p = seq.to_le_bytes().to_vec();
        p.extend_from_slice(payload);
        p
    }

    /// With an empty ring and challenge 0 the decode key is the constant
    /// `seq as u8`; pre-applying it lets the framing tests assert on the exact
    /// payload.
    fn scramble(seq: u32, payload: &mut [u8]) {
        let key = seq as u8;
        for b in payload.iter_mut().skip(CL_DECODE_START) {
            *b ^= key;
        }
    }

    #[test]
    fn in_order_sequences_pass_stale_dropped() {
        let h = Huffman::new();
        let mut nc = Netchan::new(1234, 0);
        // 2-byte payloads sit entirely inside the unscrambled prefix.
        assert!(
            nc.process_in(&server_packet(1, b"aa"), &h)
                .unwrap()
                .is_some()
        );
        assert!(
            nc.process_in(&server_packet(1, b"bb"), &h)
                .unwrap()
                .is_none()
        ); // dup
        assert!(
            nc.process_in(&server_packet(3, b"cc"), &h)
                .unwrap()
                .is_some()
        ); // gap ok
        assert_eq!(nc.dropped, 1);
        assert!(
            nc.process_in(&server_packet(2, b"dd"), &h)
                .unwrap()
                .is_none()
        ); // stale
        assert_eq!(nc.incoming_sequence, 3);
    }

    #[test]
    fn fragments_reassemble() {
        let h = Huffman::new();
        let mut nc = Netchan::new(1234, 0);
        let big = vec![7u8; FRAGMENT_SIZE + 100];
        let mut wire = big.clone();
        scramble(10, &mut wire);
        // frag 1: full size
        let mut p1 = (10u32 | FRAGMENT_BIT).to_le_bytes().to_vec();
        p1.extend_from_slice(&0u16.to_le_bytes());
        p1.extend_from_slice(&(FRAGMENT_SIZE as u16).to_le_bytes());
        p1.extend_from_slice(&wire[..FRAGMENT_SIZE]);
        // frag 2: short = terminal
        let mut p2 = (10u32 | FRAGMENT_BIT).to_le_bytes().to_vec();
        p2.extend_from_slice(&(FRAGMENT_SIZE as u16).to_le_bytes());
        p2.extend_from_slice(&100u16.to_le_bytes());
        p2.extend_from_slice(&wire[FRAGMENT_SIZE..]);
        assert!(nc.process_in(&p1, &h).unwrap().is_none());
        let out = nc.process_in(&p2, &h).unwrap().unwrap();
        assert_eq!(out, big);
        assert_eq!(nc.incoming_sequence, 10);
    }

    /// CoD2's fragments carry a long start (cod2-protocol.md 2.2), and its
    /// key string reads raw where CoD1's maps high bytes.
    #[test]
    fn cod2_fragments_take_long_starts_and_a_raw_key() {
        let mut nc = Netchan::with_params(1, 0x55, NetchanParams::COD2);
        assert_eq!(nc.reliable.len(), 128);
        let frag = |start: u32, data: &[u8]| {
            let mut p = (5u32 | FRAGMENT_BIT).to_le_bytes().to_vec();
            p.extend_from_slice(&start.to_le_bytes());
            p.extend_from_slice(&(data.len() as u16).to_le_bytes());
            p.extend_from_slice(data);
            p
        };
        let a = vec![1u8; FRAGMENT_SIZE];
        assert!(nc.frame(&frag(0, &a)).unwrap().is_none());
        let (seq, mut body) = nc
            .frame(&frag(FRAGMENT_SIZE as u32, &[2; 7]))
            .unwrap()
            .unwrap();
        assert_eq!((seq, body.len()), (5, FRAGMENT_SIZE + 7));
        let plain = body.clone();
        let key = b"say \xe9";
        nc.decode(seq, key, &mut body);
        let mut raw = plain[4..].to_vec();
        scramble_server_block((0x55 ^ 5) as u8, key, false, &mut raw);
        assert_eq!(body[4..], raw[..]);
        let mut cleaned = plain[4..].to_vec();
        scramble_server_block((0x55 ^ 5) as u8, key, true, &mut cleaned);
        assert_ne!(raw, cleaned);
        assert!(nc.frame(&6u32.to_le_bytes()).unwrap().is_some());
        assert!(nc.frame(&6u32.to_le_bytes()).unwrap().is_none());
    }

    /// An exactly-`FRAGMENT_SIZE` message still needs the zero-length terminator.
    #[test]
    fn zero_length_terminal_fragment() {
        let h = Huffman::new();
        let mut nc = Netchan::new(1234, 0);
        let big = vec![0u8; FRAGMENT_SIZE];
        let mut wire = big.clone();
        scramble(4, &mut wire);
        let mut p1 = (4u32 | FRAGMENT_BIT).to_le_bytes().to_vec();
        p1.extend_from_slice(&0u16.to_le_bytes());
        p1.extend_from_slice(&(FRAGMENT_SIZE as u16).to_le_bytes());
        p1.extend_from_slice(&wire);
        let mut p2 = (4u32 | FRAGMENT_BIT).to_le_bytes().to_vec();
        p2.extend_from_slice(&(FRAGMENT_SIZE as u16).to_le_bytes());
        p2.extend_from_slice(&0u16.to_le_bytes());
        assert!(nc.process_in(&p1, &h).unwrap().is_none());
        assert_eq!(nc.process_in(&p2, &h).unwrap().unwrap(), big);
    }

    #[test]
    fn out_of_order_fragment_dropped() {
        let h = Huffman::new();
        let mut nc = Netchan::new(1234, 0);
        let mut p = (10u32 | FRAGMENT_BIT).to_le_bytes().to_vec();
        p.extend_from_slice(&(FRAGMENT_SIZE as u16).to_le_bytes()); // start != 0
        p.extend_from_slice(&10u16.to_le_bytes());
        p.extend_from_slice(&[1u8; 10]);
        assert!(nc.process_in(&p, &h).unwrap().is_none());
    }

    #[test]
    fn truncated_packets_rejected() {
        let h = Huffman::new();
        let mut nc = Netchan::new(1234, 0);
        for n in 0..4 {
            assert!(nc.process_in(&vec![0u8; n], &h).is_err(), "len {n}");
        }
        // Fragment header claims more bytes than the packet carries.
        let mut p = (1u32 | FRAGMENT_BIT).to_le_bytes().to_vec();
        p.extend_from_slice(&0u16.to_le_bytes());
        p.extend_from_slice(&500u16.to_le_bytes());
        p.extend_from_slice(&[0u8; 4]);
        assert!(nc.process_in(&p, &h).unwrap().is_none());
        // Fragment bit set but no fragment header.
        let p = (2u32 | FRAGMENT_BIT).to_le_bytes().to_vec();
        assert!(nc.process_in(&p, &h).unwrap().is_none());
    }

    /// Retail's `SV_Netchan_Transmit` scramble (cod_lnxded 0x808dc74)
    /// transcribed: a raw walk over the server's copy of the command.
    fn reference_encode(challenge: i32, seq: u32, server_copy: &[u8], payload: &[u8]) -> Vec<u8> {
        let mut pkt = seq.to_le_bytes().to_vec();
        pkt.extend_from_slice(payload);
        let mut key = (challenge as u32 ^ seq) as u8;
        let mut index = 0;
        for (i, b) in pkt.iter_mut().enumerate().skip(4 + CL_DECODE_START) {
            if index >= server_copy.len() {
                index = 0;
            }
            // An empty ring slot is a bare NUL in the C, so it contributes 0.
            let c = server_copy.get(index).copied().unwrap_or(0);
            key ^= c << (i & 1);
            index += 1;
            *b ^= key;
        }
        pkt
    }

    /// `MSG_ReadString`'s byte map (cod_lnxded 0x8083394), which is what the
    /// server's copy of a client command went through.
    fn server_read(raw: &[u8]) -> Vec<u8> {
        raw.iter()
            .map(|&c| match c {
                0x92 => b'\'',
                c if c > 127 => b'.',
                c => c,
            })
            .collect()
    }

    #[test]
    fn decode_matches_hand_rolled_encode() {
        let h = Huffman::new();
        // `%` stays in the server's copy, so it keys as itself: a chat line
        // with one used to desync every later message. A high byte keys as
        // the `.` the server read it as.
        for key_string in [
            "vstr nextmap",
            "",
            "say ^1x",
            "say 100% sure",
            "say caf\u{e9}",
        ] {
            let mut nc = Netchan::new(1234, 0x1234_5678);
            nc.reliable[3] = key_string.to_string();

            // Payload: reliableAcknowledge = 3 as a plain long, then filler.
            let mut payload = 3i32.to_le_bytes().to_vec();
            payload.extend((0..200u32).map(|i| (i * 7) as u8));

            let seq = 42u32;
            let copy = server_read(key_string.as_bytes());
            let pkt = reference_encode(nc.challenge, seq, &copy, &payload);
            let out = nc.process_in(&pkt, &h).unwrap().unwrap();
            assert_eq!(out, payload, "key string {key_string:?}");
        }
    }

    /// Guards the bytes on disk; `gamestate::parse` does the real parse.
    #[test]
    fn gamestate_fixture_is_decoded() {
        let h = Huffman::new();
        let data = crate::testing::fixture("net/gamestate.bin");
        assert_eq!(i32::from_le_bytes(data[..4].try_into().unwrap()), 0);
        let mut r = crate::net::msg::MsgReader::new(&data[4..], &h);
        assert_eq!(r.read_byte(), 5, "svc_serverCommand");
        assert_eq!(r.read_long(), 0);
        assert_eq!(r.read_string(), "");
        assert_eq!(r.read_byte(), 2, "svc_gamestate");
        assert_eq!(r.read_long(), 0, "serverCommandSequence");
        assert_eq!(r.read_byte(), 3, "svc_configstring");
        assert_eq!(r.read_short(), 0, "CS_SERVERINFO");
        let cs = r.read_big_string();
        assert!(cs.contains("\\mapname\\mp_carentan"), "{cs}");
        assert!(!r.is_overflowed());
    }

    /// The server's receive path transcribed; must return exactly the ops handed
    /// to [`Netchan::build_out`].
    fn server_receives(nc: &Netchan, huff: &Huffman, pkt: &[u8]) -> (i32, i32, i32, Vec<u8>) {
        assert_eq!(u16::from_le_bytes(pkt[4..6].try_into().unwrap()), nc.qport);
        let body = &pkt[6..];
        let server_id = body[0] as i32;
        let message_ack = i32::from_le_bytes(body[1..5].try_into().unwrap());
        let reliable_ack = i32::from_le_bytes(body[5..9].try_into().unwrap());
        let mut comp = body[9..].to_vec();
        let string = nc.server_commands[reliable_ack as usize & (MAX_RELIABLE_COMMANDS - 1)]
            .as_bytes()
            .to_vec();
        let mut key = (nc.challenge as u32 ^ server_id as u32 ^ message_ack as u32) as u8;
        let mut index = 0usize;
        for (i, byte) in comp.iter_mut().enumerate() {
            if index >= string.len() {
                index = 0;
            }
            let c = string.get(index).copied().unwrap_or(0);
            key ^= c << (i & 1);
            index += 1;
            *byte ^= key;
        }
        (
            server_id,
            message_ack,
            reliable_ack,
            huff.decompress_block(&comp),
        )
    }

    #[test]
    fn build_out_frames_and_round_trips_through_server_decode() {
        let h = Huffman::new();
        let mut nc = Netchan::new(0xbeef, 0x1234_5678);
        let ops = [1u8, 2, 3, 4, 5, 6, 7, 8, 9, 10];
        let pkt = nc.build_out(16, 7, 0, &ops, &h).unwrap();
        assert_eq!(u32::from_le_bytes(pkt[..4].try_into().unwrap()), 1);
        assert_eq!(u16::from_le_bytes(pkt[4..6].try_into().unwrap()), 0xbeef);
        assert_eq!(nc.outgoing_sequence, 2);
        let (sid, mack, rack, got) = server_receives(&nc, &h, &pkt);
        assert_eq!((sid, mack, rack), (16, 7, 0));
        // Block decompression appends one padding byte the reader never reaches.
        assert_eq!(
            got[..ops.len()],
            ops,
            "server decode did not recover the ops"
        );
    }

    #[test]
    fn build_out_scrambles_with_the_command_ring() {
        let h = Huffman::new();
        let ops: Vec<u8> = (0..40u8).collect();

        let mut plain = Netchan::new(1, 0x0bad_f00du32 as i32);
        let clear = plain.build_out(16, 7, 2, &ops, &h).unwrap();

        let mut keyed = Netchan::new(1, 0x0bad_f00du32 as i32);
        keyed.server_commands[2] = "cs 5 \"x\"".to_string();
        let scrambled = keyed.build_out(16, 7, 2, &ops, &h).unwrap();

        assert_ne!(clear[6..], scrambled[6..], "the command ring did nothing");
        let (_, _, _, got) = server_receives(&keyed, &h, &scrambled);
        assert_eq!(got[..ops.len()], ops);
    }

    /// An oversized packet is dropped by the server with no diagnostic.
    #[test]
    fn build_out_refuses_oversized_payload() {
        let h = Huffman::new();
        let mut nc = Netchan::new(1, 0);
        // Incompressible random-ish bytes so the compressed block stays large.
        let big: Vec<u8> = (0..FRAGMENT_SIZE as u32)
            .map(|i| i.wrapping_mul(2654435761) as u8)
            .collect();
        let err = nc
            .build_out(0, 0, 0, &big, &h)
            .expect_err("oversized payload must not be framed as one packet");
        assert!(err.to_string().contains("needs fragmenting"), "{err}");
        // The rejected message must not have burned a sequence number.
        assert_eq!(nc.outgoing_sequence, 1);
    }

    #[test]
    fn server_transmit_reaches_client_process_in() {
        let h = Huffman::new();
        let (qport, challenge) = (0x2abc, 0x1357_9bdf);
        let mut sv = ServerNetchan::new(qport, challenge);
        let mut cl = Netchan::new(qport, challenge);
        cl.reliable[1] = "userinfo \"\\name\\vcod\"".into();
        sv.last_client_command_string = cl.reliable[1].clone();
        let ops: Vec<u8> = (0..6900u32).map(|i| (i * 31 + 7) as u8).collect();

        let pkts = sv.transmit(1, &ops, &h);
        assert!(pkts.len() >= 2, "expected fragments, got {}", pkts.len());
        assert_eq!(sv.outgoing_sequence, 2);
        let mut got = None;
        for p in &pkts {
            if let Some(m) = cl.process_in(p, &h).unwrap() {
                got = Some(m);
            }
        }
        let m = got.expect("client never completed the message");
        assert_eq!(i32::from_le_bytes(m[..4].try_into().unwrap()), 1);
        let plain = h.decompress_block(&m[4..]);
        assert_eq!(&plain[..ops.len()], &ops[..]);
        assert_eq!(plain[ops.len()], SVC_EOF, "the netchan terminator");
    }

    /// CoD2's server side: 4-byte fragment starts, a raw key with a high
    /// byte in it, the game's `svc_EOF`, and a 128-slot ring keying the
    /// client's messages.
    #[test]
    fn cod2_server_transmit_reaches_the_cod2_client() {
        let h = Huffman::new();
        let (qport, challenge) = (0x2abc, 0x1357_9bdf);
        let mut sv = ServerNetchan::with_params(qport, challenge, NetchanParams::COD2);
        let mut cl = Netchan::with_params(qport, challenge, NetchanParams::COD2);
        let key = "say \u{e9}t\u{e9}";
        cl.reliable[100] = key.into();
        sv.last_client_command_string = key.into();
        let ops: Vec<u8> = (0..6900u32).map(|i| (i * 31 + 7) as u8).collect();
        let pkts = sv.transmit_eof(100, &ops, 7, &h);
        assert!(pkts.len() >= 2);
        // `[seq][u32 start][u16 len]`: the second fragment starts at 1300.
        assert_eq!(
            u32::from_le_bytes(pkts[1][4..8].try_into().unwrap()) as usize,
            FRAGMENT_SIZE
        );
        let m = pkts
            .iter()
            .filter_map(|p| cl.process_in(p, &h).unwrap())
            .next()
            .expect("message");
        let plain = h.decompress_block(&m[4..]);
        assert_eq!(&plain[..ops.len()], &ops[..]);
        assert_eq!(plain[ops.len()], 7);

        sv.reliable[100] = "t 4".into();
        cl.server_commands[100] = "t 4".into();
        let pkt = cl.build_out(16, 9, 100, &[1, 2, 3], &h).unwrap();
        let m = sv.process_in(&pkt).unwrap();
        assert_eq!(&h.decompress_block(&m.ops)[..3], &[1, 2, 3]);
    }

    #[test]
    fn server_transmit_fragment_shapes() {
        let h = Huffman::new();
        let mut sv = ServerNetchan::new(1, 0);
        let one = sv.transmit(0, &[8u8, 8, 8], &h);
        assert_eq!(one.len(), 1);
        assert_eq!(
            u32::from_le_bytes(one[0][..4].try_into().unwrap()) & FRAGMENT_BIT,
            0
        );

        // Find an ops length whose compressed message is exactly 2 * FRAGMENT_SIZE.
        let mut n = 2 * FRAGMENT_SIZE;
        let ops = loop {
            let ops: Vec<u8> = (0..n as u32)
                .map(|i| i.wrapping_mul(2654435761) as u8)
                .collect();
            // Mirror `transmit`: the terminator is part of the compressed body.
            let mut body = ops.clone();
            body.push(SVC_EOF);
            let len = 4 + h.compress_block(&body).len();
            if len == 2 * FRAGMENT_SIZE {
                break ops;
            }
            n = if len > 2 * FRAGMENT_SIZE {
                n - 1
            } else {
                n + 1
            };
        };
        let pkts = sv.transmit(0, &ops, &h);
        assert_eq!(pkts.len(), 3);
        let last = &pkts[2];
        assert_eq!(
            u16::from_le_bytes(last[4..6].try_into().unwrap()) as usize,
            2 * FRAGMENT_SIZE
        );
        assert_eq!(u16::from_le_bytes(last[6..8].try_into().unwrap()), 0);
        assert_eq!(last.len(), 8);
        let mut cl = Netchan::new(1, 0);
        let mut got = None;
        for p in &pkts {
            if let Some(m) = cl.process_in(p, &h).unwrap() {
                got = Some(m);
            }
        }
        assert_eq!(got.unwrap().len(), 2 * FRAGMENT_SIZE);
    }

    #[test]
    fn client_build_out_reaches_server_process_in() {
        let h = Huffman::new();
        let (qport, challenge) = (0x2001, -77);
        let mut cl = Netchan::new(qport, challenge);
        let mut sv = ServerNetchan::new(qport, challenge);
        sv.reliable[5] = "d 1 \\sv_serverid\\16".into();
        sv.reliable_sequence = 5;
        cl.server_commands[5] = sv.reliable[5].clone();
        let ops: Vec<u8> = (0..60u8).collect();
        let pkt = cl.build_out(16, 9, 5, &ops, &h).unwrap();
        let m = sv.process_in(&pkt).unwrap();
        assert_eq!((m.server_id, m.message_ack, m.reliable_ack), (16, 9, 5));
        assert_eq!(&h.decompress_block(&m.ops)[..ops.len()], &ops[..]);
        // Nothing moves until the caller accepts the message.
        assert_eq!(sv.incoming_sequence, 0);
        assert!(sv.process_in(&pkt).is_some());
        sv.accept(&m);
        assert_eq!(sv.incoming_sequence, 1);
        // Replay and stale sequences are dropped.
        assert!(sv.process_in(&pkt).is_none());
        assert!(sv.process_in(&pkt[..8]).is_none());
    }

    #[test]
    fn server_accept_counts_the_gap() {
        let h = Huffman::new();
        let mut cl = Netchan::new(7, 0);
        let mut sv = ServerNetchan::new(7, 0);
        let first = cl.build_out(0, 0, 0, &[3], &h).unwrap();
        cl.outgoing_sequence = 5;
        let fifth = cl.build_out(0, 0, 0, &[3], &h).unwrap();
        let m = sv.process_in(&first).unwrap();
        sv.accept(&m);
        let m = sv.process_in(&fifth).unwrap();
        sv.accept(&m);
        assert_eq!((sv.incoming_sequence, sv.dropped), (5, 3));
    }
}
