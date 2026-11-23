//! CoD4x 21's reliable side channel (docs/research/cod4x-protocol.md 7): its
//! packets beside the netchan on the same socket, the stream of
//! length-framed messages their fragments carry, and the stats message.
//! The gamestate it carries is [`super::parse::Client::take_side_gamestate`]'s.
//! No timing here: the resend and the window are the connection's.

use anyhow::{Result, ensure};
use std::collections::BTreeMap;

/// The word a side channel packet opens with, where the netchan's sequence
/// stands (`SV_PacketEvent` routes it, 0x121ee2).
pub const MARK: i32 = -16;
/// A fragment's most data bytes (0xd5aca).
pub const MAX_FRAGMENT: usize = 1200;
/// Selective acknowledges a packet carries at most.
pub const MAX_SACKS: usize = 3;
/// The sequence of a packet that only acknowledges.
pub const BARE_ACK: i32 = -1;
/// A message larger than the gamestate's 0x20000-byte buffer (`MSG_Init` at
/// 0x110aaf) is no message the server writes.
pub const MAX_MESSAGE: usize = 0x20000;
/// How far past the next expected fragment one is kept: the window the
/// captures carry (20) with room.
const MAX_AHEAD: i32 = 256;

/// The message `cmd` words (`SV_ExecuteReliableMessage` 0x110fc0).
pub const CMD_GAMESTATE: i32 = 1;
pub const CMD_DOWNLOAD: i32 = 5;
pub const CMD_STATS: i32 = 9;

/// The stats block's size in modes 1 and 2 (`SV_ReceiveStats_f` 0x10f5b0).
pub const STATS_LEN: usize = 0x2000;

/// One side channel datagram (`ReliableMessagesTransmitNextFragment`
/// 0xd56b4, `ReliableMessagesReceiveNextFragment` 0xd5918).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Packet {
    pub qport: u16,
    /// The fragment's sequence, [`BARE_ACK`] for none.
    pub sequence: i32,
    /// The next sequence the sender expects.
    pub ack: i32,
    /// The byte after the acknowledge: 0 in every capture.
    pub flags: u8,
    /// Selective acknowledges, two shorts each.
    pub sacks: Vec<(i16, i16)>,
    pub window: i16,
    pub data: Vec<u8>,
}

impl Packet {
    /// Whether a datagram is a side channel packet.
    pub fn is_side(d: &[u8]) -> bool {
        d.get(..4) == Some(&MARK.to_le_bytes()[..])
    }

    /// A whole datagram, to its last byte.
    pub fn read(d: &[u8]) -> Result<Packet> {
        let mut r = Bytes(d, 0);
        ensure!(r.long()? == MARK, "not a side channel packet");
        let qport = r.short()? as u16;
        let sequence = r.long()?;
        let ack = r.long()?;
        let flags = r.byte()?;
        let n = r.byte()? as usize;
        ensure!(n <= MAX_SACKS, "{n} selective acknowledges");
        let mut sacks = Vec::with_capacity(n);
        for _ in 0..n {
            sacks.push((r.short()?, r.short()?));
        }
        let window = r.short()?;
        let len = r.short()?;
        ensure!(
            (0..=MAX_FRAGMENT as i16).contains(&len),
            "fragment of {len} bytes"
        );
        let data = r.take(len as usize)?.to_vec();
        ensure!(r.1 == d.len(), "{} bytes past the fragment", d.len() - r.1);
        Ok(Packet {
            qport,
            sequence,
            ack,
            flags,
            sacks,
            window,
            data,
        })
    }

    pub fn write(&self) -> Vec<u8> {
        let mut p = MARK.to_le_bytes().to_vec();
        p.extend_from_slice(&self.qport.to_le_bytes());
        p.extend_from_slice(&self.sequence.to_le_bytes());
        p.extend_from_slice(&self.ack.to_le_bytes());
        p.push(self.flags);
        p.push(self.sacks.len() as u8);
        for (a, b) in &self.sacks {
            p.extend_from_slice(&a.to_le_bytes());
            p.extend_from_slice(&b.to_le_bytes());
        }
        p.extend_from_slice(&self.window.to_le_bytes());
        p.extend_from_slice(&(self.data.len() as i16).to_le_bytes());
        p.extend_from_slice(&self.data);
        p
    }
}

/// A message as the stream holds it: its length word, then the body
/// (`SV_SendReliableServerCommand` 0x10eea0).
pub fn frame(body: &[u8]) -> Vec<u8> {
    let mut m = (body.len() as i32).to_le_bytes().to_vec();
    m.extend_from_slice(body);
    m
}

/// One direction's stream: fragments in sequence order, cut into the
/// message bodies their length words frame.
#[derive(Clone, Debug, Default)]
pub struct Inbound {
    next: i32,
    early: BTreeMap<i32, Vec<u8>>,
    stream: Vec<u8>,
}

impl Inbound {
    /// The next fragment wanted: the acknowledge this side sends.
    pub fn next(&self) -> i32 {
        self.next
    }

    /// Takes one packet's fragment; returns the bodies it completed. A
    /// repeat or a bare acknowledge completes none.
    pub fn take(&mut self, p: &Packet) -> Result<Vec<Vec<u8>>> {
        let ahead = p.sequence.wrapping_sub(self.next);
        if p.sequence != BARE_ACK && (0..MAX_AHEAD).contains(&ahead) {
            self.early
                .entry(p.sequence)
                .or_insert_with(|| p.data.clone());
        }
        while let Some(f) = self.early.remove(&self.next) {
            self.stream.extend_from_slice(&f);
            self.next = self.next.wrapping_add(1);
        }
        let mut out = Vec::new();
        while let Some(head) = self.stream.get(..4) {
            let n = i32::from_le_bytes(head.try_into().unwrap());
            ensure!(
                (0..=MAX_MESSAGE as i32).contains(&n),
                "side channel message of {n} bytes"
            );
            let Some(body) = self.stream.get(4..4 + n as usize) else {
                break;
            };
            out.push(body.to_vec());
            self.stream.drain(..4 + n as usize);
        }
        Ok(out)
    }
}

/// `cmd 9` both ways: the server's request is mode 0 with no block
/// (`SV_RequestStats` 0x10eef0); the client's answer is mode 0 (no stats),
/// or 1 or 2 with a [`STATS_LEN`] block, mode 2's AES-128-CBC
/// (cod4x-protocol.md 7.1).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StatsMsg {
    pub mode: u8,
    pub block: Vec<u8>,
}

impl StatsMsg {
    /// A message body, to its last byte.
    pub fn read(body: &[u8]) -> Result<StatsMsg> {
        let mut r = Bytes(body, 0);
        ensure!(r.long()? == CMD_STATS, "not a stats message");
        let mode = r.byte()?;
        let block = if mode == 0 {
            Vec::new()
        } else {
            let n = r.long()?;
            ensure!(n as usize == STATS_LEN, "stats block of {n} bytes");
            r.take(STATS_LEN)?.to_vec()
        };
        ensure!(
            r.1 == body.len(),
            "{} bytes past the stats",
            body.len() - r.1
        );
        Ok(StatsMsg { mode, block })
    }

    pub fn write(&self) -> Vec<u8> {
        let mut b = CMD_STATS.to_le_bytes().to_vec();
        b.push(self.mode);
        if self.mode != 0 {
            b.extend_from_slice(&(self.block.len() as i32).to_le_bytes());
            b.extend_from_slice(&self.block);
        }
        b
    }
}

/// A little-endian cursor that fails past the end.
struct Bytes<'a>(&'a [u8], usize);

impl<'a> Bytes<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let s = self.0.get(self.1..self.1 + n);
        ensure!(s.is_some(), "short side channel data");
        self.1 += n;
        Ok(s.unwrap())
    }
    fn byte(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }
    fn short(&mut self) -> Result<i16> {
        Ok(i16::from_le_bytes(self.take(2)?.try_into().unwrap()))
    }
    fn long(&mut self) -> Result<i32> {
        Ok(i32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Selective acknowledges, which no capture carries, round-trip, and a
    /// fourth is refused.
    #[test]
    fn selective_acknowledges_round_trip() {
        let p = Packet {
            qport: 0x1234,
            sequence: 7,
            ack: 3,
            sacks: vec![(5, 6), (9, 9)],
            window: 20,
            data: vec![1, 2, 3],
            ..Packet::default()
        };
        let d = p.write();
        assert_eq!(Packet::read(&d).unwrap(), p);
        let mut bad = d.clone();
        bad[15] = 4;
        assert!(Packet::read(&bad).is_err());
        assert!(Packet::read(&d[..d.len() - 1]).is_err());
    }

    /// Fragments out of order and repeated reassemble once; a length word
    /// past the cap fails.
    #[test]
    fn fragments_reassemble_in_sequence() {
        let stream = [frame(&[9, 0, 0, 0, 0]), frame(&[1; 3000])].concat();
        let frags: Vec<Packet> = stream
            .chunks(MAX_FRAGMENT)
            .enumerate()
            .map(|(i, c)| Packet {
                sequence: i as i32,
                data: c.to_vec(),
                ..Packet::default()
            })
            .collect();
        let mut s = Inbound::default();
        let mut got = Vec::new();
        for i in [1, 0, 0, 2, 1] {
            got.extend(s.take(&frags[i]).unwrap());
        }
        assert_eq!(got, vec![vec![9, 0, 0, 0, 0], vec![1; 3000]]);
        assert_eq!(s.next(), 3);
        let huge = Packet {
            sequence: 3,
            data: i32::MAX.to_le_bytes().to_vec(),
            ..Packet::default()
        };
        assert!(s.take(&huge).is_err());
    }
}
