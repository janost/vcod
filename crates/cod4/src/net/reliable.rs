//! CoD4x 21's reliable side channel (docs/research/cod4x-protocol.md 7): its
//! packets beside the netchan on the same socket, the stream of
//! length-framed messages their fragments carry, and the stats message.
//! The gamestate it carries is [`super::parse::Client::take_side_gamestate`]'s.
//! [`Link`] is one end's driver (the window, the resend and the
//! acknowledge), timed on the caller's millisecond clock.

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

/// Fragments a second a sender sends, and how far past the oldest
/// unacknowledged one it goes before it starts over there: `+0x14`, set
/// to 20 by `ReliableMessageSetup` (0xd5f7b) and written as the packet's
/// window.
pub const WINDOW: i32 = 20;
/// How far past the next expected fragment a receiver takes a packet:
/// `+0x52c`, 4 (0xd5f82); one further on is dropped whole, its
/// acknowledge too (0xd5960..0xd5971).
pub const RECEIVE_AHEAD: i32 = 4;
/// With nothing in flight, a bare acknowledge goes this long after the
/// last packet (`add eax,0x15e` at 0xd57ec, 0xd58f7).
pub const IDLE_ACK_MS: i32 = 350;
/// A frame further than this from the last sends nothing and only
/// restarts the clock (`cmp ecx,0xfa` at 0xd623c).
pub const MAX_FRAME_GAP_MS: i32 = 250;

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

/// One end of the side channel: the stream it reads ([`Inbound`]) and
/// the fragments it sends. The sender is `ReliableMessageSend` (0xd5d98:
/// the message cut into [`MAX_FRAGMENT`]-byte fragments, 0xd5e37..0xd5ebf),
/// `ReliableMessagesFrame` (0xd621b: [`WINDOW`] fragments a second off the
/// frame time, 0xd6244..0xd6291) and `ReliableMessagesTransmitNextFragment`
/// (0xd56b4: one packet a call, the cursor walking from the oldest
/// unacknowledged fragment up to [`WINDOW`] past it and starting over,
/// 0xd56f2..0xd572f, so an unacknowledged fragment goes again each lap;
/// with none in flight a bare acknowledge [`IDLE_ACK_MS`] after the last
/// packet, 0xd574a..0xd57f1). The receiver's checks are
/// `ReliableMessagesReceiveNextFragment`'s (0xd5918). VERIFIED (the
/// instructions); that CoD4x's client runs the same code is INFERRED.
#[derive(Clone, Debug, Default)]
pub struct Link {
    pub qport: u16,
    pub inbound: Inbound,
    /// Fragments not yet acknowledged, by sequence, each with its
    /// selective acknowledge mark (`+0x4b4`).
    out: BTreeMap<i32, (Vec<u8>, bool)>,
    /// The next sequence a fragment takes (`+0x0`).
    head: i32,
    /// The oldest unacknowledged sequence (`+0x8`).
    acked: i32,
    /// The next sequence to transmit (`+0x10`).
    cursor: i32,
    /// The last frame's time (`+0xa54`), `None` before the first.
    last: Option<i32>,
    /// The send credit left over, in fragment-milliseconds (`+0x514`).
    credit: i32,
    /// When a bare acknowledge may go (`+0xa58`).
    ack_due: i32,
}

impl Link {
    pub fn new(qport: u16) -> Link {
        Link {
            qport,
            ..Link::default()
        }
    }

    /// Queues one message body, framed and cut into fragments.
    pub fn send(&mut self, body: &[u8]) {
        for c in frame(body).chunks(MAX_FRAGMENT) {
            self.out.insert(self.head, (c.to_vec(), false));
            self.head += 1;
        }
    }

    /// Whether every fragment sent was acknowledged.
    pub fn is_idle(&self) -> bool {
        self.acked == self.head
    }

    /// Takes one packet from the other end; returns the message bodies it
    /// completed. A packet past [`RECEIVE_AHEAD`], or whose acknowledge is
    /// behind the last one, past the window or past what was sent, is
    /// dropped whole, as the receiver drops it (0xd596f..0xd59e3).
    pub fn receive(&mut self, p: &Packet) -> Result<Vec<Vec<u8>>> {
        if p.sequence != BARE_ACK && p.sequence.wrapping_sub(self.inbound.next()) >= RECEIVE_AHEAD {
            return Ok(Vec::new());
        }
        if p.ack > self.acked + WINDOW || p.ack < self.acked || p.ack > self.head {
            return Ok(Vec::new());
        }
        // (offset from the acknowledge, count), each within the window
        // (0xd5a59..0xd5aad).
        for &(off, n) in &p.sacks {
            let (start, n) = (p.ack + i32::from(off), i32::from(n));
            if !(0..=WINDOW).contains(&n) || start > p.ack + WINDOW - n {
                return Ok(Vec::new());
            }
            for s in start..start + n {
                if let Some(f) = self.out.get_mut(&s) {
                    f.1 = true;
                }
            }
        }
        if p.ack > self.acked {
            self.acked = p.ack;
            self.out.retain(|s, _| *s >= p.ack);
        }
        self.inbound.take(p)
    }

    /// `ReliableMessagesFrame` at `now_ms`: the packets due since the
    /// last frame.
    pub fn frame(&mut self, now_ms: i32) -> Vec<Packet> {
        let Some(last) = self.last.replace(now_ms) else {
            return Vec::new();
        };
        let dt = now_ms.wrapping_sub(last);
        if dt > MAX_FRAME_GAP_MS {
            return Vec::new();
        }
        let credit = dt.max(0) * WINDOW + self.credit;
        self.credit = credit % 1000;
        (0..credit / 1000)
            .filter_map(|_| self.transmit(now_ms))
            .collect()
    }

    /// `ReliableMessagesTransmitNextFragment`: the packet at the cursor, a
    /// bare acknowledge when nothing is in flight and one is due, or none.
    fn transmit(&mut self, now_ms: i32) -> Option<Packet> {
        let packet = |sequence, data: &[u8]| Packet {
            qport: self.qport,
            sequence,
            ack: self.inbound.next(),
            window: WINDOW as i16,
            data: data.to_vec(),
            ..Packet::default()
        };
        if self.acked == self.head {
            if self.ack_due >= now_ms {
                return None;
            }
            self.ack_due = now_ms + IDLE_ACK_MS;
            return Some(packet(BARE_ACK, &[]));
        }
        self.cursor = self.cursor.max(self.acked);
        let out = match self.out.get(&self.cursor) {
            Some((data, false)) => {
                self.ack_due = now_ms + IDLE_ACK_MS;
                Some(packet(self.cursor, data))
            }
            _ => None,
        };
        self.cursor = if self.cursor + 1 < self.acked + WINDOW {
            self.cursor + 1
        } else {
            self.acked
        };
        out
    }
}

/// Mode 2's IV words (cod4x-protocol.md 7.1).
pub const STATS_IV: [u32; 4] = [0xeb62114f, 0x66996144, 0x7341cfa4, 0xb9551299];

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

    /// Mode 2: `block` under AES-128-CBC as `SV_ReceiveStats_f` takes it
    /// (0x10f5eb..0x10f81a): the key the connection's challenge four
    /// times, the IV [`STATS_IV`]'s words, each little-endian, no padding.
    pub fn mode2(block: &[u8; STATS_LEN], challenge: i32) -> StatsMsg {
        use aes::cipher::{BlockEncryptMut, KeyIvInit, block_padding::NoPadding};
        let key: Vec<u8> = (0..4).flat_map(|_| challenge.to_le_bytes()).collect();
        let iv: Vec<u8> = STATS_IV.iter().flat_map(|w| w.to_le_bytes()).collect();
        let mut buf = block.to_vec();
        cbc::Encryptor::<aes::Aes128>::new(key[..].into(), iv[..].into())
            .encrypt_padded_mut::<NoPadding>(&mut buf, STATS_LEN)
            .expect("a whole number of blocks");
        StatsMsg {
            mode: 2,
            block: buf,
        }
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

    /// The sender's pace and laps: a fragment each 50 ms (20 a second), the
    /// cursor running 20 past the oldest unacknowledged fragment and back,
    /// an acknowledge moving the lap's start; idle, a bare acknowledge each
    /// 350 ms (0xd621b, 0xd56b4).
    #[test]
    fn the_sender_laps_its_window_at_twenty_a_second() {
        let mut l = Link::new(7);
        l.send(&vec![9; 5000]);
        assert_eq!(l.head, 5);
        let mut sent = Vec::new();
        for t in (0..=1500).step_by(50) {
            sent.extend(l.frame(t).iter().map(|p| p.sequence));
            if t == 700 {
                // The other end has 0..2.
                let ack = Packet {
                    sequence: BARE_ACK,
                    ack: 3,
                    ..Packet::default()
                };
                l.receive(&ack).unwrap();
            }
        }
        // 0..4, then the empty slots of the lap; the acknowledge at 700
        // stretches it to 22, and it starts over at 3.
        assert_eq!(sent, [0, 1, 2, 3, 4, 3, 4]);
        let all = Packet {
            sequence: BARE_ACK,
            ack: 5,
            ..Packet::default()
        };
        l.receive(&all).unwrap();
        assert!(l.is_idle());
        let acks: Vec<i32> = (1550..=2600)
            .step_by(50)
            .filter(|&t| !l.frame(t).is_empty())
            .collect();
        assert_eq!(acks, [1650, 2050, 2450]);
        // A frame 251 ms after the last sends nothing.
        assert!(l.frame(2851).is_empty());
    }

    /// The receiver drops a packet four or more past the next fragment,
    /// and one whose acknowledge is behind, past the window or past what
    /// was sent, acknowledge and all (0xd5918).
    #[test]
    fn the_receiver_drops_what_retail_drops() {
        let mut l = Link::new(7);
        l.send(&[1; 10]);
        let frag = |sequence, ack| Packet {
            sequence,
            ack,
            data: frame(&[5]),
            ..Packet::default()
        };
        assert!(l.receive(&frag(4, 1)).unwrap().is_empty());
        assert_eq!((l.inbound.next(), l.acked), (0, 0));
        assert!(l.receive(&frag(0, 2)).unwrap().is_empty());
        assert_eq!(l.acked, 0, "past what was sent");
        assert_eq!(l.receive(&frag(0, 1)).unwrap(), [vec![5]]);
        assert!(l.is_idle());
        assert!(l.receive(&frag(1, 0)).unwrap().is_empty());
        assert_eq!(l.inbound.next(), 1, "an acknowledge behind");
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
