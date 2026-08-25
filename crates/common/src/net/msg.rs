//! Contains routines ported from the RTCW-MP GPL source, Copyright (C) 1999-2010 id Software LLC, a ZeniMax Media company.
//! See NOTICE.
//!
//! Message reader/writer and the delta codecs, ported from RTCW-MP
//! `qcommon/msg.c` and corrected against cod_lnxded 1.1d. Wire layout:
//! `docs/protocol-1.1.md`, "The message-block model" and "Delta field encoding".
//!
//! The body is block-decompressed up front and read as plain bytes with two
//! cursors, `readcount` for whole bytes and `bit` for bit fields, as `msg_t`
//! does. A raw byte read in the middle of a run of bit fields lands after the
//! byte those bits are still filling.

use super::huffman::{add_bit, get_bit, Huffman};
use super::protocol::Protocol;

const MAX_STRING_CHARS: usize = 1024;
const BIG_INFO_STRING: usize = 8192;
/// A float that is integral and inside this range goes out truncated.
const FLOAT_INT_BITS: i32 = 13;
const FLOAT_INT_BIAS: i32 = 1 << (FLOAT_INT_BITS - 1);

/// `svc_ops_e`, the server->client op bytes.
pub const SVC_NOP: u8 = 1;
pub const SVC_GAMESTATE: u8 = 2;
pub const SVC_CONFIGSTRING: u8 = 3;
pub const SVC_BASELINE: u8 = 4;
pub const SVC_SERVER_COMMAND: u8 = 5;
pub const SVC_DOWNLOAD: u8 = 6;
pub const SVC_SNAPSHOT: u8 = 7;
pub const SVC_EOF: u8 = 8;

/// Reader over a message body; `new` block-decompresses the input.
pub struct MsgReader {
    data: Vec<u8>,
    /// `msg_t.bit`, the bit-field cursor.
    bit: usize,
    /// `msg_t.readcount`, the whole-byte cursor.
    readcount: usize,
    overflowed: bool,
}

impl MsgReader {
    pub fn new(data: &[u8], huff: &Huffman) -> Self {
        MsgReader {
            data: huff.decompress_block(data),
            bit: 0,
            readcount: 0,
            overflowed: false,
        }
    }

    /// Bits consumed so far, on the decompressed stream.
    pub fn bits_read(&self) -> usize {
        self.bit.max(self.readcount * 8)
    }

    /// Set once a read ran past the end; every read after that returns 0.
    pub fn is_overflowed(&self) -> bool {
        self.overflowed
    }

    /// `MSG_ReadBits` (cod_lnxded 0x807f18c): one bit at a time, LSB first.
    /// Negative `bits` sign-extends.
    pub fn read_bits(&mut self, bits: i32) -> i32 {
        debug_assert!(bits != 0 && (-31..=32).contains(&bits), "bad bits {bits}");
        if self.overflowed {
            return 0;
        }
        let width = (bits.unsigned_abs() as usize).min(32);
        let mut value: u32 = 0;
        for i in 0..width {
            value |= u32::from(self.next_bit()) << i;
        }
        sign_extend(value, width, bits < 0)
    }

    /// The delta codec's inlined bit read (cod_lnxded 0x807c904): `bits & 7`
    /// loose bits, then whole bytes off `readcount`. Differs from
    /// [`Self::read_bits`] at `bits >= 8`; never swap one for the other.
    pub fn read_packed_bits(&mut self, bits: i32) -> i32 {
        debug_assert!(bits != 0 && (-32..=32).contains(&bits), "bad bits {bits}");
        if self.overflowed {
            return 0;
        }
        let width = (bits.unsigned_abs() as usize).min(32);
        let mut value: u32 = 0;
        let rem = width & 7;
        for i in 0..rem {
            value |= u32::from(self.next_bit()) << i;
        }
        let mut got = rem;
        while got < width {
            value |= u32::from(self.next_byte()) << got;
            got += 8;
        }
        // No sign extension: a negative `bits` only picks the width.
        value as i32
    }

    /// `MSG_ReadByte` (0x807f294): raw at `readcount`, untouched by `bit`.
    pub fn read_byte(&mut self) -> u8 {
        if self.overflowed {
            return 0;
        }
        self.next_byte()
    }

    pub fn read_short(&mut self) -> i16 {
        let lo = u16::from(self.read_byte());
        let hi = u16::from(self.read_byte());
        (lo | (hi << 8)) as i16
    }

    pub fn read_long(&mut self) -> i32 {
        let mut v = 0u32;
        for i in 0..4 {
            v |= u32::from(self.read_byte()) << (i * 8);
        }
        v as i32
    }

    /// `MSG_ReadString`: `%` and high ascii become `.`.
    pub fn read_string(&mut self) -> String {
        self.read_string_capped(MAX_STRING_CHARS, true)
    }

    /// `MSG_ReadBigString`: high ascii kept as latin-1, `%` still neutered.
    pub fn read_big_string(&mut self) -> String {
        self.read_string_capped(BIG_INFO_STRING, false)
    }

    fn read_string_capped(&mut self, cap: usize, strip_high: bool) -> String {
        let mut s = String::new();
        // The cap counts bytes read; a latin-1 char above 127 is two utf-8
        // bytes in `s`.
        let mut n = 0;
        while n < cap - 1 {
            let c = self.read_byte();
            if c == 0 || self.overflowed {
                break;
            }
            let c = if c == b'%' || (strip_high && c > 127) {
                b'.'
            } else {
                c
            };
            s.push(c as char);
            n += 1;
        }
        s
    }

    /// One bit, resyncing onto `readcount` at each byte boundary as the C does.
    fn next_bit(&mut self) -> u8 {
        if self.bit & 7 == 0 {
            if self.readcount >= self.data.len() {
                self.overflowed = true;
                return 0;
            }
            self.bit = self.readcount * 8;
            self.readcount += 1;
        }
        get_bit(&self.data, &mut self.bit)
    }

    fn next_byte(&mut self) -> u8 {
        if self.readcount >= self.data.len() {
            self.overflowed = true;
            return 0;
        }
        let b = self.data[self.readcount];
        self.readcount += 1;
        b
    }
}

fn sign_extend(value: u32, width: usize, signed: bool) -> i32 {
    if signed && width < 32 && value & (1 << (width - 1)) != 0 {
        (value | !((1u32 << width) - 1)) as i32
    } else {
        value as i32
    }
}

/// Raw wire values in `Protocol::entity_fields` order. Float fields
/// (`bits == 0`) hold the f32 bit pattern.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EntityState {
    pub number: u32,
    pub fields: Vec<i32>,
}

impl EntityState {
    pub fn null(p: &Protocol) -> Self {
        EntityState {
            number: 0,
            fields: vec![0; p.entity_fields.len()],
        }
    }

    pub fn field_index(p: &Protocol, name: &str) -> Option<usize> {
        p.entity_fields.iter().position(|f| f.name == name)
    }

    /// Raw wire value of a named field, 0 if the protocol lacks it.
    pub fn field_i32(&self, p: &Protocol, name: &str) -> i32 {
        Self::field_index(p, name).map_or(0, |i| self.fields[i])
    }

    pub fn field_f32(&self, p: &Protocol, name: &str) -> f32 {
        f32::from_bits(self.field_i32(p, name) as u32)
    }

    pub fn origin(&self, p: &Protocol) -> [f32; 3] {
        [
            self.field_f32(p, "pos.trBase[0]"),
            self.field_f32(p, "pos.trBase[1]"),
            self.field_f32(p, "pos.trBase[2]"),
        ]
    }

    /// `apos.trBase`, degrees.
    pub fn angles(&self, p: &Protocol) -> [f32; 3] {
        [
            self.field_f32(p, "apos.trBase[0]"),
            self.field_f32(p, "apos.trBase[1]"),
            self.field_f32(p, "apos.trBase[2]"),
        ]
    }
}

/// `MSG_ReadDeltaEntity`. The entity number is already read; `None` means the
/// delta removed the entity.
pub fn read_delta_entity(
    r: &mut MsgReader,
    p: &Protocol,
    from: &EntityState,
    number: u32,
) -> Option<EntityState> {
    if r.read_bits(1) == 1 {
        return None;
    }
    let mut to = EntityState {
        number,
        fields: from.fields.clone(),
    };
    if r.read_bits(1) == 0 {
        return Some(to);
    }

    // `lc` is a raw byte at `readcount` (cod_lnxded 0x807cca3); fields past
    // it keep `from`.
    let lc = (r.read_byte() as usize).min(p.entity_fields.len());
    for (i, field) in p.entity_fields.iter().take(lc).enumerate() {
        to.fields[i] = read_delta_field(r, to.fields[i], field.bits);
    }
    Some(to)
}

/// `clientState_t`, the snapshot's third stream. Raw wire values in
/// `Protocol::client_fields` order. `docs/research/clientstate-wire-format.md`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClientState {
    pub client_num: u32,
    pub fields: Vec<i32>,
}

impl ClientState {
    pub fn null(p: &Protocol) -> Self {
        ClientState {
            client_num: 0,
            fields: vec![0; p.client_fields.len()],
        }
    }

    pub fn field_index(p: &Protocol, name: &str) -> Option<usize> {
        p.client_fields.iter().position(|f| f.name == name)
    }

    pub fn field_i32(&self, p: &Protocol, name: &str) -> i32 {
        Self::field_index(p, name).map_or(0, |i| self.fields[i])
    }

    /// `name[0..28]` are the 32-byte char array in LE u32 chunks; latin-1 up
    /// to the first NUL.
    pub fn name(&self, p: &Protocol) -> String {
        let mut bytes = Vec::with_capacity(32);
        for off in (0..32).step_by(4) {
            let v = self.field_i32(p, &format!("name[{off}]")) as u32;
            bytes.extend_from_slice(&v.to_le_bytes());
        }
        bytes
            .iter()
            .take_while(|&&b| b != 0)
            .map(|&b| b as char)
            .collect()
    }

    /// A fresh entry: team plus the name packed LE u32 chunk per
    /// `name[0..28]` field, NUL padded, capped at 31 bytes like the C array.
    pub fn named(p: &Protocol, client_num: u32, team: i32, name: &str) -> Self {
        let mut cs = ClientState::null(p);
        cs.client_num = client_num;
        if let Some(i) = Self::field_index(p, "team") {
            cs.fields[i] = team;
        }
        let mut bytes: Vec<u8> = name.bytes().take(31).collect();
        bytes.resize(32, 0);
        for (chunk, off) in bytes.chunks(4).zip((0..32).step_by(4)) {
            let Some(idx) = Self::field_index(p, &format!("name[{off}]")) else {
                break;
            };
            let mut word = [0u8; 4];
            word[..chunk.len()].copy_from_slice(chunk);
            cs.fields[idx] = u32::from_le_bytes(word) as i32;
        }
        cs
    }
}

/// `MSG_ReadDeltaClient` body (cod_lnxded 0x807f758). The presence bit and the
/// 6-bit client index are already read; `None` means the client disconnected.
pub fn read_delta_client(
    r: &mut MsgReader,
    p: &Protocol,
    from: &ClientState,
    client_num: u32,
) -> Option<ClientState> {
    if r.read_bits(1) == 1 {
        return None;
    }
    let mut to = ClientState {
        client_num,
        fields: from.fields.clone(),
    };
    if r.read_bits(1) == 0 {
        return Some(to);
    }
    let lc = (r.read_byte() as usize).min(p.client_fields.len());
    for (i, field) in p.client_fields.iter().take(lc).enumerate() {
        to.fields[i] = read_delta_field(r, to.fields[i], field.bits);
    }
    Some(to)
}

/// Inverse of [`read_delta_client`]. The caller has written the presence bit
/// and the 6-bit client number; `None` writes the removed bit.
pub fn write_delta_client(
    w: &mut MsgWriter,
    p: &Protocol,
    from: &ClientState,
    to: Option<&ClientState>,
) {
    let Some(to) = to else {
        w.write_bits(1, 1); // removed
        return;
    };
    debug_assert_eq!(from.fields.len(), to.fields.len());
    w.write_bits(0, 1);
    let lc = from
        .fields
        .iter()
        .zip(&to.fields)
        .rposition(|(a, b)| a != b)
        .map_or(0, |i| i + 1);
    debug_assert!(lc <= 255);
    if lc == 0 {
        w.write_bits(0, 1); // no delta
        return;
    }
    w.write_bits(1, 1);
    w.write_byte(lc as u8);
    for i in 0..lc {
        write_delta_field(w, from.fields[i], to.fields[i], p.client_fields[i].bits);
    }
}

/// One delta field (cod_lnxded 0x807c904): change bit, zero flag, then the
/// float selector or the packed `|bits|` value. No sign extension.
fn read_delta_field(r: &mut MsgReader, from_val: i32, bits: i32) -> i32 {
    if r.read_bits(1) == 0 {
        return from_val;
    }
    if bits == 0 {
        read_float_field(r, from_val)
    } else if r.read_bits(1) == 0 {
        0
    } else {
        r.read_packed_bits(bits)
    }
}

/// Zero flag, then a 13-bit biased integer or the raw 32 bits. A clear zero
/// flag from a `+0.0` base means the sender held `-0.0` (the C compares slots
/// as ints); keeping it lets [`write_delta_entity`] round-trip a capture byte
/// for byte. docs/protocol-1.1.md, "Entity delta".
fn read_float_field(r: &mut MsgReader, from_val: i32) -> i32 {
    if r.read_bits(1) == 0 {
        return if from_val == 0 {
            (-0f32).to_bits() as i32
        } else {
            0f32.to_bits() as i32
        };
    }
    if r.read_bits(1) == 0 {
        let trunc = r.read_packed_bits(FLOAT_INT_BITS) - FLOAT_INT_BIAS;
        (trunc as f32).to_bits() as i32
    } else {
        // Raw read at `readcount` (0x807ca80).
        r.read_long()
    }
}

pub fn write_server_command(w: &mut MsgWriter, seq: i32, cmd: &str) {
    w.write_byte(SVC_SERVER_COMMAND);
    w.write_long(seq);
    w.write_string(cmd);
}

/// Inverse of [`read_delta_entity`]. The caller has written the entity number;
/// `None` writes a remove. Slots compare as raw ints, as in the C, so `-0.0`
/// is a change from `0.0`.
pub fn write_delta_entity(
    w: &mut MsgWriter,
    p: &Protocol,
    from: &EntityState,
    to: Option<&EntityState>,
) {
    let Some(to) = to else {
        w.write_bits(1, 1); // removed
        return;
    };
    // zip would silently truncate a mismatched pair.
    debug_assert_eq!(from.fields.len(), to.fields.len());
    w.write_bits(0, 1);
    let lc = from
        .fields
        .iter()
        .zip(&to.fields)
        .rposition(|(a, b)| a != b)
        .map_or(0, |i| i + 1);
    debug_assert!(lc <= 255);
    if lc == 0 {
        w.write_bits(0, 1); // no delta
        return;
    }
    w.write_bits(1, 1);
    w.write_byte(lc as u8); // raw byte, as the reader takes it (0x807cca3)
    for i in 0..lc {
        write_delta_field(w, from.fields[i], to.fields[i], p.entity_fields[i].bits);
    }
}

fn write_delta_field(w: &mut MsgWriter, from_val: i32, to_val: i32, bits: i32) {
    if from_val == to_val {
        w.write_bits(0, 1);
        return;
    }
    w.write_bits(1, 1);
    if bits == 0 {
        write_float_field(w, to_val);
    } else if to_val == 0 {
        w.write_bits(0, 1);
    } else {
        w.write_bits(1, 1);
        w.write_packed_bits(to_val, bits);
    }
}

fn write_float_field(w: &mut MsgWriter, raw: i32) {
    let f = f32::from_bits(raw as u32);
    if f == 0.0 {
        w.write_bits(0, 1);
        return;
    }
    w.write_bits(1, 1);
    let trunc = f as i32;
    let biased = trunc.wrapping_add(FLOAT_INT_BIAS);
    if trunc as f32 == f && (0..(1 << FLOAT_INT_BITS)).contains(&biased) {
        w.write_bits(0, 1);
        w.write_packed_bits(biased, FLOAT_INT_BITS);
    } else {
        w.write_bits(1, 1);
        w.write_long(raw);
    }
}

/// Raw wire values in `Protocol::player_fields` order, floats as bit patterns.
/// The trailing stat/HUD arrays are consumed but not stored.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct PlayerState {
    pub fields: Vec<i32>,
}

impl PlayerState {
    pub fn null(p: &Protocol) -> Self {
        PlayerState {
            fields: vec![0; p.player_fields.len()],
        }
    }

    pub fn field_index(p: &Protocol, name: &str) -> Option<usize> {
        p.player_fields.iter().position(|f| f.name == name)
    }

    pub fn field_i32(&self, p: &Protocol, name: &str) -> i32 {
        Self::field_index(p, name).map_or(0, |i| self.fields[i])
    }

    pub fn field_f32(&self, p: &Protocol, name: &str) -> f32 {
        f32::from_bits(self.field_i32(p, name) as u32)
    }

    pub fn origin(&self, p: &Protocol) -> [f32; 3] {
        [
            self.field_f32(p, "origin[0]"),
            self.field_f32(p, "origin[1]"),
            self.field_f32(p, "origin[2]"),
        ]
    }

    /// Degrees.
    pub fn viewangles(&self, p: &Protocol) -> [f32; 3] {
        [
            self.field_f32(p, "viewangles[0]"),
            self.field_f32(p, "viewangles[1]"),
            self.field_f32(p, "viewangles[2]"),
        ]
    }
}

/// `MSG_ReadDeltaPlayerstate` (cod_lnxded 0x807e2f0). No per-field zero flag,
/// unlike the entity delta; the trailing array blocks are consumed only to
/// stay aligned. docs/protocol-1.1.md, "PlayerState delta".
pub fn read_delta_playerstate(r: &mut MsgReader, p: &Protocol, from: &PlayerState) -> PlayerState {
    let mut to = PlayerState {
        fields: from.fields.clone(),
    };
    let lc = (r.read_byte() as usize).min(p.player_fields.len());
    for (i, field) in p.player_fields.iter().take(lc).enumerate() {
        if r.read_bits(1) == 0 {
            continue;
        }
        to.fields[i] = if field.bits == 0 {
            read_ps_float(r)
        } else {
            // Unsigned, width `|bits|`, no sign extension (0x807e5d7).
            r.read_packed_bits(field.bits)
        };
    }
    consume_ps_arrays(r);
    to
}

/// Integral/full selector only, no zero flag (cod_lnxded 0x807e471).
fn read_ps_float(r: &mut MsgReader) -> i32 {
    if r.read_bits(1) == 0 {
        let trunc = r.read_packed_bits(FLOAT_INT_BITS) - FLOAT_INT_BIAS;
        (trunc as f32).to_bits() as i32
    } else {
        r.read_long()
    }
}

/// Inverse of [`read_ps_float`]: integral selector unless out of the biased
/// 13-bit range.
fn write_ps_float(w: &mut MsgWriter, raw: i32) {
    let f = f32::from_bits(raw as u32);
    let trunc = f as i32;
    let biased = trunc.wrapping_add(FLOAT_INT_BIAS);
    if trunc as f32 == f && (0..(1 << FLOAT_INT_BITS)).contains(&biased) {
        w.write_bits(0, 1);
        w.write_packed_bits(biased, FLOAT_INT_BITS);
    } else {
        w.write_bits(1, 1);
        w.write_long(raw);
    }
}

/// Mirror of [`read_delta_playerstate`]. The trailing array blocks go out
/// empty: the vcod server carries no stats, ammo or HUD state. Unlike the
/// entity codec there is no zero-flag bit here, so a `-0.0` float reads back
/// `+0.0`; retail loses the sign the same way.
pub fn write_delta_playerstate(
    w: &mut MsgWriter,
    p: &Protocol,
    from: &PlayerState,
    to: &PlayerState,
) {
    debug_assert_eq!(from.fields.len(), to.fields.len());
    let lc = from
        .fields
        .iter()
        .zip(&to.fields)
        .rposition(|(a, b)| a != b)
        .map_or(0, |i| i + 1);
    debug_assert!(lc <= 255);
    w.write_byte(lc as u8);
    for i in 0..lc {
        if from.fields[i] == to.fields[i] {
            w.write_bits(0, 1);
            continue;
        }
        w.write_bits(1, 1);
        if p.player_fields[i].bits == 0 {
            write_ps_float(w, to.fields[i]);
        } else {
            // Unsigned width |bits|, matching the reader's plain packed read.
            w.write_packed_bits(to.fields[i], p.player_fields[i].bits);
        }
    }
    // Array-block gates: block 1, block 2, block 3's four sub-arrays,
    // blocks 4 and 5.
    for _ in 0..8 {
        w.write_bits(0, 1);
    }
}

/// Widths of the 34-entry HUD field table (cod_lnxded 0x80de384). 0..6 back
/// the per-weapon block, 6..34 the two HUD arrays.
#[rustfmt::skip]
const HUD_FIELD_BITS: [i32; 34] = [
    0, 0, 0, 12, 10, 4,   // origin[0..2], icon, entNum, teamNum
    32, 4, 0, 10, 10, 2,  // color.rgba, type, fontScale, y, x, alignY
    2, 32, 4, 8, 8, 10, 10, 0, 32, 32, 16, 32, 16, 10, 0, 8, 10, 32, 16, 10, 10, 32,
];

/// The trailing array blocks (cod_lnxded 0x807e7b3..0x807eeb6), values
/// discarded.
fn consume_ps_arrays(r: &mut MsgReader) {
    // Block 1: a 6-bit mask selecting up to six scalars.
    if r.read_bits(1) == 1 {
        let m = r.read_bits(6);
        if m & 0x01 != 0 {
            r.read_short();
        }
        if m & 0x02 != 0 {
            r.read_short();
        }
        if m & 0x04 != 0 {
            r.read_short();
        }
        if m & 0x08 != 0 {
            r.read_bits(6);
        }
        if m & 0x10 != 0 {
            r.read_short();
        }
        if m & 0x20 != 0 {
            r.read_byte();
        }
    }
    // Block 2: gated group of four 16-entry short arrays.
    if r.read_bits(1) == 1 {
        consume_short_array_group(r);
    }
    // Block 3: the same, with no group gate.
    consume_short_array_group(r);
    // Block 4: 16 weapons, each a 3-bit value then six delta fields.
    if r.read_bits(1) == 1 {
        for _ in 0..16 {
            r.read_bits(3);
            if r.read_bits(1) == 1 {
                for &bits in &HUD_FIELD_BITS[0..6] {
                    read_delta_field(r, 0, bits);
                }
            }
        }
    }
    // Block 5: two HUD-element arrays.
    if r.read_bits(1) == 1 {
        consume_hud_array(r);
        consume_hud_array(r);
    }
}

/// Four gated 16-entry short arrays (cod_lnxded 0x807ea4b / 0x807eb49).
fn consume_short_array_group(r: &mut MsgReader) {
    for _ in 0..4 {
        if r.read_bits(1) == 1 {
            let mask = r.read_short() as u16 as u32;
            for i in 0..16 {
                if mask & (1u32 << i) != 0 {
                    r.read_short();
                }
            }
        }
    }
}

/// One HUD-element array (cod_lnxded 0x807cf5c).
fn consume_hud_array(r: &mut MsgReader) {
    let count = r.read_bits(5);
    for _ in 0..count {
        let j = r.read_bits(5);
        for k in 0..=j {
            let idx = (6 + k) as usize;
            let bits = HUD_FIELD_BITS.get(idx).copied().unwrap_or(0);
            read_delta_field(r, 0, bits);
            if r.is_overflowed() {
                return;
            }
        }
    }
}

/// `usercmd_t` (codextended shared.h:811). The compact encoding
/// [`write_delta_usercmd`] emits carries none of `up`, `weapon`, `wbuttons`,
/// `flags` or `angles[2]`. Layout: docs/protocol-1.1.md, "Client to server
/// message body".
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct UserCmd {
    pub server_time: i32,
    pub buttons: u8,
    pub wbuttons: u8,
    pub weapon: u8,
    pub flags: u8,
    /// ANGLE2SHORT units, 16 bits each on the wire.
    pub angles: [i32; 3],
    pub forward: i8,
    pub right: i8,
    pub up: i8,
}

/// The base (all-zero) usercmd a `clc_moveNoDelta` deltas from.
pub const NULL_USERCMD: UserCmd = UserCmd {
    server_time: 0,
    buttons: 0,
    wbuttons: 0,
    weapon: 0,
    flags: 0,
    angles: [0; 3],
    forward: 0,
    right: 0,
    up: 0,
};

/// Forward/right as the wire nibble (cod_lnxded 0x807ba8e): forward in bits
/// 0/1, right in bits 2/3, +/-10 deadzone.
fn fr_bucket(forward: i8, right: i8) -> i32 {
    let mut f = 0;
    if forward as i32 > 10 {
        f |= 1;
    } else if (forward as i32) < -10 {
        f |= 2;
    }
    if right as i32 > 10 {
        f |= 4;
    } else if (right as i32) < -10 {
        f |= 8;
    }
    f
}

/// `MSG_WriteDeltaUsercmdKey`, reconstructed from the reader at cod_lnxded
/// 0x807b7f8. Emits only the compact branch: serverTime, button bit 0, keyed
/// pitch and yaw, a 4-bit forward/right code. Forward/right can only be +127,
/// -127 or 0; `up`, `weapon`, `wbuttons` and roll stay at the base.
///
/// Angles and the forward/right code are always sent, never delta-omitted:
/// the server keeps an omitted field at its stored previous cmd, so a
/// self-contained cmd survives a dropped packet. `from` only feeds the
/// serverTime delta. docs/protocol-1.1.md, "Client to server message body".
pub fn write_delta_usercmd(w: &mut MsgWriter, key: i32, from: &UserCmd, to: &UserCmd) {
    // serverTime: 1 = 8-bit delta from the base, 0 = 32-bit absolute.
    let dt = to.server_time.wrapping_sub(from.server_time);
    if (0..256).contains(&dt) {
        w.write_bits(1, 1);
        w.write_byte(dt as u8);
    } else {
        w.write_bits(0, 1);
        w.write_long(to.server_time);
    }

    // Changed bit (!= key & 1), then the branch bit (== key & 1 picks compact).
    w.write_bits((key & 1) ^ 1, 1);
    w.write_bits(key & 1, 1);

    // The reader mixes the serverTime into the key from here (0x807b95d).
    let key = key ^ to.server_time;

    w.write_bits(((to.buttons as i32) & 1) ^ (key & 1), 1);

    write_keyed_angle(w, key, to.angles[0]);
    write_keyed_angle(w, key, to.angles[1]);

    let flag = fr_bucket(to.forward, to.right);
    w.write_bits(1, 1);
    w.write_bits(flag ^ (key & 0xf), 4);
}

/// Change bit set, then `value ^ key` as a short (cod_lnxded 0x807b9bd).
fn write_keyed_angle(w: &mut MsgWriter, key: i32, to: i32) {
    w.write_bits(1, 1);
    w.write_short(((to ^ key) & 0xffff) as u16 as i16);
}

/// Two-bit twin of [`fr_bucket`] (cod_lnxded 0x807bf58).
fn up_bucket(up: i8) -> i32 {
    if up as i32 > 10 {
        1
    } else if (up as i32) < -10 {
        2
    } else {
        0
    }
}

/// Axes are never analog on the wire (0x807bb52, 0x807c013).
fn move_axis(code: i32, pos: i32, neg: i32) -> i8 {
    if code & pos != 0 {
        127
    } else if code & neg != 0 {
        -127
    } else {
        0
    }
}

/// `MSG_ReadDeltaKey` for a bit field; mask table at cod_lnxded 0x80de300.
fn read_keyed_bits(r: &mut MsgReader, key: i32, from_val: i32, bits: i32) -> i32 {
    if r.read_bits(1) == 1 {
        r.read_bits(bits) ^ (key & ((1 << bits) - 1))
    } else {
        from_val
    }
}

/// Inverse of [`write_keyed_angle`]; the short comes off the byte cursor
/// (0x807bc06).
fn read_keyed_angle(r: &mut MsgReader, key: i32, from_val: i32) -> i32 {
    if r.read_bits(1) == 1 {
        (i32::from(r.read_short()) ^ key) & 0xffff
    } else {
        from_val & 0xffff
    }
}

/// `MSG_ReadDeltaUsercmdKey` (cod_lnxded 0x807b7f8), both branches. Fields a
/// branch does not carry keep the base value. The serverTime is mixed into the
/// key right away in the compact branch (0x807b95d) and only after the
/// forward/right code in the full one (0x807bde1). Field tables in
/// docs/protocol-1.1.md, "Client to server message body".
///
/// Errs when the message ended inside the cmd, naming the branch; that is the
/// only visible symptom of a wrong field layout.
pub fn read_delta_usercmd(r: &mut MsgReader, key: i32, from: &UserCmd) -> anyhow::Result<UserCmd> {
    let mut to = *from;
    if r.read_bits(1) == 1 {
        to.server_time = from.server_time.wrapping_add(i32::from(r.read_byte()));
    } else {
        to.server_time = r.read_long();
    }
    if r.read_bits(1) == (key & 1) {
        return finish_usercmd(r, to, "unchanged");
    }
    if r.read_bits(1) == (key & 1) {
        let key = key ^ to.server_time;
        let b0 = (r.read_bits(1) ^ (key & 1)) as u8 & 1;
        to.buttons = (to.buttons & !1) | b0;
        to.angles[0] = read_keyed_angle(r, key, to.angles[0]);
        to.angles[1] = read_keyed_angle(r, key, to.angles[1]);
        if r.read_bits(1) == 1 {
            let n = r.read_bits(4) ^ (key & 0xf);
            to.forward = move_axis(n, 1, 2);
            to.right = move_axis(n, 4, 8);
        }
        return finish_usercmd(r, to, "compact");
    }
    read_full_usercmd(r, key, from, &mut to);
    finish_usercmd(r, to, "full")
}

/// Overflow is latched, not returned, so surface it here as an error.
fn finish_usercmd(r: &MsgReader, to: UserCmd, branch: &str) -> anyhow::Result<UserCmd> {
    anyhow::ensure!(
        !r.is_overflowed(),
        "message ended inside the usercmd ({branch} branch)"
    );
    Ok(to)
}

/// The full-field branch (cod_lnxded 0x807bba0). `buttons` is rebuilt: bit 0
/// under the raw key, bits 1..6 under the mixed key. `flags` is never sent.
fn read_full_usercmd(r: &mut MsgReader, key: i32, from: &UserCmd, to: &mut UserCmd) {
    to.buttons = (r.read_bits(1) ^ (key & 1)) as u8 & 1;
    to.angles[0] = read_keyed_angle(r, key, from.angles[0]);
    to.angles[1] = read_keyed_angle(r, key, from.angles[1]);
    let fr = read_keyed_bits(r, key, fr_bucket(from.forward, from.right), 4);
    to.forward = move_axis(fr, 1, 2);
    to.right = move_axis(fr, 4, 8);

    let key = key ^ to.server_time;
    to.angles[2] = read_keyed_angle(r, key, from.angles[2]);
    let hi = read_keyed_bits(r, key, i32::from(from.buttons >> 1), 6);
    to.buttons |= (hi as u8) << 1;
    to.wbuttons = if r.read_bits(1) == 1 {
        r.read_byte() ^ (key as u8)
    } else {
        from.wbuttons
    };
    to.up = move_axis(read_keyed_bits(r, key, up_bucket(from.up), 2), 1, 2);
    to.weapon = read_keyed_bits(r, key, i32::from(from.weapon), 6) as u8;
}

/// Mirror of [`MsgReader`]: assembled plain, block-compressed by
/// [`Self::finish`].
pub struct MsgWriter<'a> {
    huff: &'a Huffman,
    out: Vec<u8>,
    /// `msg_t.bit`, the bit-field cursor.
    bit: usize,
}

impl<'a> MsgWriter<'a> {
    pub fn new(huff: &'a Huffman) -> Self {
        MsgWriter {
            huff,
            out: Vec::new(),
            bit: 0,
        }
    }

    /// See [`MsgReader::bits_read`].
    pub fn bits_written(&self) -> usize {
        self.bit.max(self.out.len() * 8)
    }

    /// Mirror of [`MsgReader::read_bits`].
    pub fn write_bits(&mut self, value: i32, bits: i32) {
        debug_assert!(bits != 0 && (-31..=32).contains(&bits), "bad bits {bits}");
        let width = (bits.unsigned_abs() as usize).min(32);
        let mut value = (value as u32) & (u32::MAX >> (32 - width));
        for _ in 0..width {
            self.put_bit((value & 1) as u8);
            value >>= 1;
        }
    }

    /// Mirror of [`MsgReader::read_packed_bits`].
    pub fn write_packed_bits(&mut self, value: i32, bits: i32) {
        debug_assert!(bits != 0 && (-32..=32).contains(&bits), "bad bits {bits}");
        let width = (bits.unsigned_abs() as usize).min(32);
        let mut value = (value as u32) & (u32::MAX >> (32 - width));
        let rem = width & 7;
        for _ in 0..rem {
            self.put_bit((value & 1) as u8);
            value >>= 1;
        }
        for _ in 0..(width - rem) / 8 {
            self.out.push((value & 0xff) as u8);
            value >>= 8;
        }
    }

    pub fn write_byte(&mut self, c: u8) {
        self.out.push(c);
    }

    pub fn write_short(&mut self, c: i16) {
        let c = c as u16;
        self.write_byte(c as u8);
        self.write_byte((c >> 8) as u8);
    }

    pub fn write_long(&mut self, c: i32) {
        let c = c as u32;
        for i in 0..4 {
            self.write_byte((c >> (i * 8)) as u8);
        }
    }

    /// At each byte boundary the next bit byte goes at the end of the message,
    /// as `MSG_WriteBits` does.
    fn put_bit(&mut self, bit: u8) {
        if self.bit & 7 == 0 {
            self.bit = self.out.len() * 8;
            self.out.push(0);
        }
        add_bit(bit, &mut self.out, &mut self.bit);
    }

    pub fn write_string(&mut self, s: &str) {
        for &b in s.as_bytes().iter().take(MAX_STRING_CHARS - 1) {
            // "get rid of 0xff chars, because old clients don't like them"
            self.write_byte(if b > 127 { b'.' } else { b });
        }
        self.write_byte(0);
    }

    /// `MSG_WriteBigString`: each latin-1 char goes out as its byte.
    pub fn write_big_string(&mut self, s: &str) {
        for c in s.chars().take(BIG_INFO_STRING - 1) {
            let b = u32::from(c);
            self.write_byte(if b > 255 { b'.' } else { b as u8 });
        }
        self.write_byte(0);
    }

    /// Block-compressed, as `SV_Netchan_Transmit` sends it.
    pub fn finish(self) -> Vec<u8> {
        self.huff.compress_block(&self.out)
    }

    /// The plain bytes; the client netchan compresses its op block itself.
    pub fn into_ops(self) -> Vec<u8> {
        self.out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::huffman::Huffman;
    use crate::net::protocol::PROTOCOL_V1;

    fn rt(f: impl Fn(&mut MsgWriter), g: impl Fn(&mut MsgReader)) {
        let h = Huffman::new();
        let mut w = MsgWriter::new(&h);
        f(&mut w);
        let data = w.finish();
        let mut r = MsgReader::new(&data, &h);
        g(&mut r);
        assert!(!r.is_overflowed());
    }

    #[test]
    fn scalar_roundtrip() {
        rt(
            |w| {
                w.write_bits(1, 1);
                w.write_bits(-3, -5); // signed 5-bit
                w.write_byte(200);
                w.write_short(-12345);
                w.write_long(0x1234_5678);
            },
            |r| {
                assert_eq!(r.read_bits(1), 1);
                assert_eq!(r.read_bits(-5), -3);
                assert_eq!(r.read_byte(), 200);
                assert_eq!(r.read_short(), -12345);
                assert_eq!(r.read_long(), 0x1234_5678);
            },
        );
    }

    #[test]
    fn string_roundtrip() {
        rt(
            |w| w.write_string("mp_carentan"),
            |r| assert_eq!(r.read_string(), "mp_carentan"),
        );
    }

    #[test]
    fn overflow_flags_not_panics() {
        let h = Huffman::new();
        let mut r = MsgReader::new(&[0x00], &h);
        for _ in 0..100 {
            r.read_long();
        }
        assert!(r.is_overflowed());
    }

    #[test]
    fn every_bit_width_roundtrip() {
        let h = Huffman::new();
        let check = |v: i32, bits: i32| {
            let mut w = MsgWriter::new(&h);
            w.write_bits(v, bits);
            let data = w.finish();
            let mut r = MsgReader::new(&data, &h);
            assert_eq!(r.read_bits(bits), v, "{bits} bits, value {v:#x}");
            assert!(!r.is_overflowed());
        };
        for bits in 1..=32i32 {
            let mask = if bits == 32 {
                u32::MAX
            } else {
                (1u32 << bits) - 1
            };
            for &v in &[0u32, 1, mask, mask >> 1, 0x5555_5555 & mask] {
                check(v as i32, bits);
            }
            if bits > 31 {
                continue; // the C caps signed values at 31 bits
            }
            let lo = -(1i32 << (bits - 1));
            let hi = (1i32 << (bits - 1)) - 1;
            for &v in &[lo, -1, 0, hi] {
                check(v, -bits);
            }
        }
    }

    #[test]
    fn big_string_keeps_high_bytes() {
        let h = Huffman::new();
        let mut w = MsgWriter::new(&h);
        for &b in b"caf\xe9%s" {
            w.write_byte(b);
        }
        w.write_byte(0);
        let data = w.finish();
        let mut r = MsgReader::new(&data, &h);
        // '%' still neutered, 0xe9 kept as latin-1.
        assert_eq!(r.read_big_string(), "caf\u{e9}.s");
        assert!(!r.is_overflowed());
    }

    #[test]
    fn string_strips_high_bytes() {
        let h = Huffman::new();
        let mut w = MsgWriter::new(&h);
        for &b in b"a\xe9b" {
            w.write_byte(b);
        }
        w.write_byte(0);
        let data = w.finish();
        let mut r = MsgReader::new(&data, &h);
        assert_eq!(r.read_string(), "a.b");
    }

    /// Latin-1 chars above 127 are two utf-8 bytes; the cap counts bytes read.
    #[test]
    fn big_string_caps_at_8191_chars() {
        let h = Huffman::new();
        let mut w = MsgWriter::new(&h);
        for _ in 0..9000 {
            w.write_byte(0xe9);
        }
        w.write_byte(0);
        let data = w.finish();
        let mut r = MsgReader::new(&data, &h);
        assert_eq!(r.read_big_string().chars().count(), BIG_INFO_STRING - 1);
    }

    #[test]
    fn empty_writer_is_empty() {
        let h = Huffman::new();
        assert!(MsgWriter::new(&h).finish().is_empty());
    }

    #[test]
    fn truncated_string_overflows() {
        let h = Huffman::new();
        let mut w = MsgWriter::new(&h);
        w.write_string(&"abcdefgh".repeat(32));
        let mut data = w.finish();
        data.truncate(4);
        let mut r = MsgReader::new(&data, &h);
        let s = r.read_string();
        assert!(s.len() < 256, "read {} chars from 4 bytes", s.len());
        assert!(r.is_overflowed());
    }

    #[test]
    fn overflow_stops_all_reads() {
        let h = Huffman::new();
        let mut r = MsgReader::new(&[0x00], &h);
        r.read_long();
        assert!(r.is_overflowed());
        let at = r.bits_read();
        assert_eq!(r.read_bits(1), 0);
        assert_eq!(r.read_bits(-5), 0);
        assert_eq!(r.read_byte(), 0);
        assert_eq!(r.read_short(), 0);
        assert_eq!(r.read_long(), 0);
        assert_eq!(r.read_string(), "");
        assert_eq!(r.read_big_string(), "");
        assert_eq!(r.bits_read(), at, "reads kept consuming after overflow");
    }

    #[test]
    fn empty_message_overflows() {
        let h = Huffman::new();
        let mut r = MsgReader::new(&[], &h);
        assert_eq!(r.read_byte(), 0);
        assert!(r.is_overflowed());
    }

    #[test]
    fn mixed_stream_roundtrip() {
        rt(
            |w| {
                w.write_bits(5, 3);
                w.write_string("connect");
                w.write_long(-1);
                w.write_bits(-1, -8);
                w.write_short(0x7fff);
                w.write_bits(0x1ff, 9);
            },
            |r| {
                assert_eq!(r.read_bits(3), 5);
                assert_eq!(r.read_string(), "connect");
                assert_eq!(r.read_long(), -1);
                assert_eq!(r.read_bits(-8), -1);
                assert_eq!(r.read_short(), 0x7fff);
                assert_eq!(r.read_bits(9), 0x1ff);
            },
        );
    }

    /// A whole byte written mid-run lands after the byte those bits fill; this
    /// is what makes the delta codec's raw `lc` byte legible.
    #[test]
    fn bit_and_byte_cursors_interleave() {
        let h = Huffman::new();
        let mut w = MsgWriter::new(&h);
        w.write_bits(0b101, 3);
        w.write_byte(0xab);
        w.write_bits(0b11010, 5); // finishes the byte the first 3 bits started
        w.write_bits(1, 1); // spills into a fresh byte after 0xab
        let data = w.finish();
        let mut r = MsgReader::new(&data, &h);
        assert_eq!(r.read_bits(3), 0b101);
        assert_eq!(r.read_byte(), 0xab);
        assert_eq!(r.read_bits(5), 0b11010);
        assert_eq!(r.read_bits(1), 1);
        assert!(!r.is_overflowed());
        // The plain bytes underneath: the bit byte, the raw byte, the spill.
        assert_eq!(&h.decompress_block(&data)[..3], &[0xd5, 0xab, 0x01]);
    }

    #[test]
    fn packed_bits_differ_from_loose_bits() {
        let h = Huffman::new();
        let loose = {
            let mut w = MsgWriter::new(&h);
            w.write_bits(0x2a5, 10);
            h.decompress_block(&w.finish())
        };
        let packed = {
            let mut w = MsgWriter::new(&h);
            w.write_packed_bits(0x2a5, 10);
            h.decompress_block(&w.finish())
        };
        assert_ne!(loose, packed);
    }

    #[test]
    fn packed_bits_roundtrip() {
        let h = Huffman::new();
        for &bits in &[1i32, 5, 8, 9, 10, 13, 16, 24, 32] {
            let mask = if bits == 32 {
                u32::MAX
            } else {
                (1u32 << bits) - 1
            };
            for &v in &[0u32, 1, mask, mask >> 1, 0x5555_5555 & mask] {
                let mut w = MsgWriter::new(&h);
                w.write_bits(1, 1); // start off a byte boundary
                w.write_packed_bits(v as i32, bits);
                let data = w.finish();
                let mut r = MsgReader::new(&data, &h);
                assert_eq!(r.read_bits(1), 1);
                assert_eq!(r.read_packed_bits(bits) as u32, v, "{bits} bits, {v:#x}");
                assert!(!r.is_overflowed());
            }
        }
    }

    #[test]
    fn delta_entity_roundtrip() {
        use crate::net::protocol::PROTOCOL_V1;
        let p = &PROTOCOL_V1;
        let h = Huffman::new();
        let null = EntityState::null(p);

        let mut w = MsgWriter::new(&h);
        // 1. a full float, an integral float, a zero float and an int field.
        w.write_bits(0, 1); // not removed
        w.write_bits(1, 1); // delta follows
        w.write_byte(12); // lc: fields 0..12
        for (i, f) in p.entity_fields.iter().take(12).enumerate() {
            match i {
                1 => {
                    w.write_bits(1, 1); // changed
                    w.write_bits(1, 1); // non-zero
                    w.write_bits(1, 1); // full float
                    w.write_long(1234.5f32.to_bits() as i32);
                }
                2 => {
                    w.write_bits(1, 1);
                    w.write_bits(1, 1);
                    w.write_bits(0, 1); // integral float
                    w.write_packed_bits(-64 + FLOAT_INT_BIAS, FLOAT_INT_BITS);
                }
                8 => {
                    w.write_bits(1, 1);
                    w.write_bits(0, 1); // zero float
                }
                11 => {
                    w.write_bits(1, 1);
                    w.write_bits(1, 1); // non-zero int
                    w.write_packed_bits(7, f.bits);
                }
                _ => w.write_bits(0, 1), // unchanged
            }
        }
        // 2. no-delta, 3. removed.
        w.write_bits(0, 1);
        w.write_bits(0, 1);
        w.write_bits(1, 1);
        let data = w.finish();

        let mut r = MsgReader::new(&data, &h);
        let a = read_delta_entity(&mut r, p, &null, 5).unwrap();
        assert_eq!(a.number, 5);
        assert_eq!(a.field_f32(p, "pos.trBase[0]"), 1234.5);
        assert_eq!(a.field_f32(p, "pos.trBase[1]"), -64.0);
        assert_eq!(a.field_f32(p, "pos.trBase[2]"), 0.0);
        assert_eq!(a.field_i32(p, "eType"), 7);
        // Past `lc` everything is copied from `from`.
        assert_eq!(a.field_i32(p, "dmgFlags"), 0);

        let b = read_delta_entity(&mut r, p, &a, 6).unwrap();
        assert_eq!(b.number, 6);
        assert_eq!(b.fields, a.fields);

        assert!(read_delta_entity(&mut r, p, &a, 7).is_none());
        assert!(!r.is_overflowed());
    }

    #[test]
    fn write_delta_entity_round_trips_capture_baselines() {
        use crate::net::protocol::PROTOCOL_V1;
        let p = &PROTOCOL_V1;
        let h = Huffman::new();
        let data = crate::testing::fixture("net/gamestate.bin");
        let mut r = MsgReader::new(&data[4..], &h);
        let gs = crate::net::gamestate::parse(&mut r, p).unwrap();
        let null = EntityState::null(p);

        let mut w = MsgWriter::new(&h);
        let mut nums: Vec<u32> = gs.baselines.keys().copied().collect();
        nums.sort();
        for &n in &nums {
            write_delta_entity(&mut w, p, &null, Some(&gs.baselines[&n]));
        }
        write_delta_entity(
            &mut w,
            p,
            &gs.baselines[&nums[0]],
            Some(&gs.baselines[&nums[0]]),
        ); // no delta
        write_delta_entity(&mut w, p, &null, None); // remove
        let mut r = MsgReader::new(&w.finish(), &h);
        for &n in &nums {
            assert_eq!(
                read_delta_entity(&mut r, p, &null, n).unwrap(),
                gs.baselines[&n],
                "entity {n}"
            );
        }
        let same = read_delta_entity(&mut r, p, &gs.baselines[&nums[0]], nums[0]).unwrap();
        assert_eq!(same.fields, gs.baselines[&nums[0]].fields);
        assert!(read_delta_entity(&mut r, p, &null, 9).is_none());
        assert!(!r.is_overflowed());
    }

    // ---- clientState delta ----

    #[test]
    fn delta_client_roundtrip() {
        use crate::net::protocol::PROTOCOL_V1;
        let p = &PROTOCOL_V1;
        let h = Huffman::new();
        let null = ClientState::null(p);

        let team = ClientState::field_index(p, "team").unwrap();
        let mi = ClientState::field_index(p, "modelindex").unwrap();
        let lc = team.max(mi) + 1;

        let mut w = MsgWriter::new(&h);
        // 1. changed team + modelindex
        w.write_bits(0, 1); // not removed
        w.write_bits(1, 1); // delta follows
        w.write_byte(lc as u8);
        for (i, f) in p.client_fields.iter().take(lc).enumerate() {
            if i == team {
                w.write_bits(1, 1);
                w.write_bits(1, 1); // non-zero
                w.write_packed_bits(2, f.bits);
            } else if i == mi {
                w.write_bits(1, 1);
                w.write_bits(1, 1);
                w.write_packed_bits(37, f.bits);
            } else {
                w.write_bits(0, 1);
            }
        }
        // 2. no-delta, 3. removed
        w.write_bits(0, 1);
        w.write_bits(0, 1);
        w.write_bits(1, 1);
        let data = w.finish();

        let mut r = MsgReader::new(&data, &h);
        let a = read_delta_client(&mut r, p, &null, 3).unwrap();
        assert_eq!(a.client_num, 3);
        assert_eq!(a.field_i32(p, "team"), 2);
        assert_eq!(a.field_i32(p, "modelindex"), 37);
        assert_eq!(a.field_i32(p, "attachModelIndex[0]"), 0);

        let b = read_delta_client(&mut r, p, &a, 4).unwrap();
        assert_eq!(b.fields, a.fields);

        assert!(read_delta_client(&mut r, p, &a, 5).is_none());
        assert!(!r.is_overflowed());
    }

    #[test]
    fn client_name_decodes() {
        use crate::net::protocol::PROTOCOL_V1;
        let p = &PROTOCOL_V1;
        let mut cs = ClientState::null(p);
        let put = |cs: &mut ClientState, field: &str, v: u32| {
            let i = ClientState::field_index(p, field).unwrap();
            cs.fields[i] = v as i32;
        };
        put(&mut cs, "name[0]", u32::from_le_bytes(*b"kilr"));
        put(&mut cs, "name[4]", u32::from_le_bytes(*b"oy\0\0"));
        assert_eq!(cs.name(p), "kilroy");
        assert_eq!(ClientState::null(p).name(p), "");
    }

    #[test]
    fn named_client_state_round_trips_through_the_writer() {
        let p = &PROTOCOL_V1;
        let h = Huffman::new();
        let cs = ClientState::named(p, 0, 3, "vcod");
        assert_eq!(cs.name(p), "vcod");
        assert_eq!(cs.field_i32(p, "team"), 3);

        let null = ClientState::null(p);
        let mut w = MsgWriter::new(&h);
        w.write_bits(1, 1); // a client follows
        w.write_bits(0, 6); // index 0
        write_delta_client(&mut w, p, &null, Some(&cs));
        let bits = w.bits_written();
        let d = w.finish();
        let mut r = MsgReader::new(&d, &h);
        assert_eq!(r.read_bits(1), 1);
        assert_eq!(r.read_bits(6), 0);
        assert_eq!(read_delta_client(&mut r, p, &null, 0), Some(cs));
        assert_eq!(r.bits_read(), bits);
    }

    #[test]
    fn long_names_are_capped_and_the_removed_bit_round_trips() {
        let p = &PROTOCOL_V1;
        let cs = ClientState::named(p, 5, 3, &"x".repeat(40));
        assert_eq!(cs.name(p), "x".repeat(31));

        let h = Huffman::new();
        let null = ClientState::null(p);
        let mut w = MsgWriter::new(&h);
        w.write_bits(1, 1);
        w.write_bits(5, 6);
        write_delta_client(&mut w, p, &null, None); // disconnect
        let d = w.finish();
        let mut r = MsgReader::new(&d, &h);
        assert_eq!(r.read_bits(1), 1);
        assert_eq!(r.read_bits(6), 5);
        assert_eq!(read_delta_client(&mut r, p, &null, 5), None);
    }

    #[test]
    fn bit_cursors_match() {
        let h = Huffman::new();
        let mut w = MsgWriter::new(&h);
        w.write_bits(3, 2);
        w.write_long(0x0bad_f00du32 as i32);
        w.write_string("x");
        let written = w.bits_written();
        let data = w.finish();
        let mut r = MsgReader::new(&data, &h);
        r.read_bits(2);
        r.read_long();
        r.read_string();
        assert_eq!(r.bits_read(), written);
        assert!(!r.is_overflowed());
    }

    // ---- playerState delta ----

    fn ps_float_integral(w: &mut MsgWriter, v: f32) {
        w.write_bits(1, 1); // changed
        w.write_bits(0, 1); // integral
        w.write_packed_bits(v as i32 + FLOAT_INT_BIAS, FLOAT_INT_BITS);
    }
    fn ps_float_full(w: &mut MsgWriter, v: f32) {
        w.write_bits(1, 1);
        w.write_bits(1, 1);
        w.write_long(v.to_bits() as i32);
    }
    /// No zero flag: the playerState main loop has none.
    fn ps_int(w: &mut MsgWriter, v: i32, bits: i32) {
        w.write_bits(1, 1);
        w.write_packed_bits(v, bits);
    }
    /// Eight clear gates: blocks 1, 2, 4, 5 and block 3's four sub-gates.
    fn ps_empty_arrays(w: &mut MsgWriter) {
        for _ in 0..8 {
            w.write_bits(0, 1);
        }
    }

    /// A negative-width field stores the raw unsigned value; the 10-bit marker
    /// after the array section proves the delta consumed to the exact bit.
    #[test]
    fn playerstate_delta_roundtrip() {
        use crate::net::protocol::PROTOCOL_V1;
        let p = &PROTOCOL_V1;
        let h = Huffman::new();
        let from = PlayerState::null(p);

        let wt = PlayerState::field_index(p, "weaponTime").unwrap();
        assert!(p.player_fields[wt].bits < 0, "weaponTime is negative-width");
        let lc = wt + 1;

        let idx = |name: &str| PlayerState::field_index(p, name).unwrap();
        let (o0, o1, o2) = (idx("origin[0]"), idx("origin[1]"), idx("origin[2]"));
        let (va0, va1) = (idx("viewangles[0]"), idx("viewangles[1]"));
        let weapon = idx("weapon");

        let mut w = MsgWriter::new(&h);
        w.write_byte(lc as u8);
        for (i, f) in p.player_fields.iter().take(lc).enumerate() {
            if i == o0 {
                ps_float_integral(&mut w, 200.0);
            } else if i == o1 {
                ps_float_integral(&mut w, -96.0);
            } else if i == o2 {
                ps_float_integral(&mut w, 48.0);
            } else if i == va0 {
                ps_float_full(&mut w, 12.5);
            } else if i == va1 {
                ps_float_integral(&mut w, 90.0);
            } else if i == weapon {
                ps_int(&mut w, 5, f.bits);
            } else if i == wt {
                // High bit set: unsigned it is 0xffff, sign-extended it is -1.
                ps_int(&mut w, 0xffff, f.bits);
            } else {
                w.write_bits(0, 1); // unchanged
            }
        }
        ps_empty_arrays(&mut w);
        w.write_bits(0x2aa, 10); // alignment marker
        let data = w.finish();

        let mut r = MsgReader::new(&data, &h);
        let ps = read_delta_playerstate(&mut r, p, &from);
        assert_eq!(ps.origin(p), [200.0, -96.0, 48.0]);
        assert_eq!(ps.viewangles(p), [12.5, 90.0, 0.0]); // [2] kept from null
        assert_eq!(ps.field_i32(p, "weapon"), 5);
        assert_eq!(ps.field_i32(p, "weaponTime"), 0xffff);
        // Past `lc`.
        assert_eq!(ps.field_i32(p, "gunfx"), 0);
        assert_eq!(r.read_bits(10), 0x2aa);
        assert!(!r.is_overflowed());
    }

    /// Every block populated; the trailing marker survives only if each one
    /// consumed exactly.
    #[test]
    fn playerstate_arrays_consume_exactly() {
        use crate::net::protocol::PROTOCOL_V1;
        let p = &PROTOCOL_V1;
        let h = Huffman::new();
        let from = PlayerState::null(p);

        fn field(w: &mut MsgWriter, bits: i32, v: i32) {
            w.write_bits(1, 1); // changed
            if bits == 0 {
                w.write_bits(1, 1); // non-zero
                w.write_bits(1, 1); // full float
                w.write_long(v);
            } else {
                w.write_bits(1, 1); // non-zero int
                w.write_packed_bits(v, bits);
            }
        }

        let mut w = MsgWriter::new(&h);
        w.write_byte(0); // lc = 0: no main fields, straight to the arrays

        // Block 1: mask with all six selectors set.
        w.write_bits(1, 1);
        w.write_bits(0x3f, 6);
        w.write_short(11); // 0x01 short
        w.write_short(22); // 0x02 short
        w.write_short(33); // 0x04 short
        w.write_bits(4, 6); // 0x08 6-bit
        w.write_short(44); // 0x10 short
        w.write_byte(55); // 0x20 byte

        // Block 2: gate, then one of four sub-arrays populated (two shorts).
        w.write_bits(1, 1);
        w.write_bits(1, 1); // sub 0 present
        w.write_short(0b0000_0000_0000_0101); // bits 0 and 2
        w.write_short(1);
        w.write_short(2);
        w.write_bits(0, 1); // sub 1 absent
        w.write_bits(0, 1); // sub 2 absent
        w.write_bits(0, 1); // sub 3 absent

        // Block 3 (ungated): one sub-array with a single short.
        w.write_bits(1, 1); // sub 0 present
        w.write_short(0b0000_0000_0000_0001); // bit 0
        w.write_short(9);
        w.write_bits(0, 1);
        w.write_bits(0, 1);
        w.write_bits(0, 1);

        // Block 4: only weapon 0 carries its six fields.
        w.write_bits(1, 1);
        for wpn in 0..16 {
            w.write_bits(3, 3); // 3-bit value
            if wpn == 0 {
                w.write_bits(1, 1); // fields present
                for &bits in &[0i32, 0, 0, 12, 10, 4] {
                    field(&mut w, bits, if bits == 0 { 1000i32 } else { 3 });
                }
            } else {
                w.write_bits(0, 1); // fields absent
            }
        }

        // Block 5: array 0 has one entry of two fields (table 6, 7), array 1
        // is empty.
        w.write_bits(1, 1);
        w.write_bits(1, 5); // array 0: outer count 1
        w.write_bits(1, 5); // entry: j = 1 -> two fields (entries 6,7)
        field(&mut w, 32, 0x0102_0304); // color.rgba
        field(&mut w, 4, 7); // type
        w.write_bits(0, 5); // array 1: outer count 0

        w.write_bits(0x155, 10); // alignment marker
        let data = w.finish();

        let mut r = MsgReader::new(&data, &h);
        let ps = read_delta_playerstate(&mut r, p, &from);
        assert_eq!(ps.fields, from.fields);
        assert_eq!(r.read_bits(10), 0x155);
        assert!(!r.is_overflowed());
    }

    /// A playerstate holding the pinned retail spectator values (provenance:
    /// docs/design/2026-08-26-server-snapshots-plan.md header). Built by name so
    /// table order cannot break the test silently.
    fn spectator_playerstate(p: &Protocol) -> PlayerState {
        let mut ps = PlayerState::null(p);
        let mut set = |name: &str, v: i32| {
            ps.fields[PlayerState::field_index(p, name).unwrap()] = v;
        };
        set("commandTime", 1_149_798);
        set("origin[0]", 384f32.to_bits() as i32);
        set("origin[1]", (-624f32).to_bits() as i32);
        set("origin[2]", 184f32.to_bits() as i32);
        set("eFlags", 24);
        set("delta_angles[1]", 16_384);
        set("speed", 400);
        set("pm_type", 4);
        set("mins[0]", (-15f32).to_bits() as i32);
        set("mins[1]", (-15f32).to_bits() as i32);
        set("maxs[0]", 15f32.to_bits() as i32);
        set("maxs[1]", 15f32.to_bits() as i32);
        set("maxs[2]", 70f32.to_bits() as i32);
        set("proneViewHeight", 11);
        set("crouchViewHeight", 40);
        set("standViewHeight", 60);
        set("deadViewHeight", 8);
        set("walkSpeedScale", 0.4f32.to_bits() as i32);
        set("runSpeedScale", 1f32.to_bits() as i32);
        set("proneSpeedScale", 0.15f32.to_bits() as i32);
        set("crouchSpeedScale", 0.65f32.to_bits() as i32);
        set("strafeSpeedScale", 0.8f32.to_bits() as i32);
        set("backSpeedScale", 0.7f32.to_bits() as i32);
        set("leanSpeedScale", 0.4f32.to_bits() as i32);
        set("friction", 1f32.to_bits() as i32);
        ps
    }

    #[test]
    fn written_playerstate_round_trips_from_null() {
        let p = &PROTOCOL_V1;
        let h = Huffman::new();
        let to = spectator_playerstate(p);
        let mut w = MsgWriter::new(&h);
        write_delta_playerstate(&mut w, p, &PlayerState::null(p), &to);
        let bits = w.bits_written();
        let d = w.finish();
        let mut r = MsgReader::new(&d, &h);
        assert_eq!(read_delta_playerstate(&mut r, p, &PlayerState::null(p)), to);
        assert_eq!(
            r.bits_read(),
            bits,
            "writer and reader must agree bit for bit"
        );
    }

    #[test]
    fn written_playerstate_deltas_from_a_base() {
        let p = &PROTOCOL_V1;
        let h = Huffman::new();
        let base = spectator_playerstate(p);
        let mut to = base.clone();
        let oi = PlayerState::field_index(p, "origin[0]").unwrap();
        to.fields[oi] = 512f32.to_bits() as i32;
        // An int field at its negative-width boundary (viewheights are -8).
        let vi = PlayerState::field_index(p, "standViewHeight").unwrap();
        to.fields[vi] = 255;

        let mut w = MsgWriter::new(&h);
        write_delta_playerstate(&mut w, p, &base, &to);
        let d = w.finish();
        let mut r = MsgReader::new(&d, &h);
        assert_eq!(read_delta_playerstate(&mut r, p, &base), to);
    }

    #[test]
    fn unchanged_playerstate_writes_only_the_empty_arrays() {
        let p = &PROTOCOL_V1;
        let h = Huffman::new();
        let ps = PlayerState::null(p);
        let mut w = MsgWriter::new(&h);
        write_delta_playerstate(&mut w, p, &ps, &ps);
        let bits = w.bits_written();
        let d = w.finish();
        let mut r = MsgReader::new(&d, &h);
        assert_eq!(read_delta_playerstate(&mut r, p, &ps), ps);
        // lc byte plus eight zero array-gate bits.
        assert_eq!(bits, 8 + 8);
    }

    /// The count byte after a 2-bit clc op lands at the byte cursor, alone and
    /// behind a reliable `clc_clientCommand`.
    #[test]
    fn move_ops_count_reads_back() {
        let h = Huffman::new();
        let mut to = NULL_USERCMD;
        to.server_time = 123456;
        to.forward = 127;

        let mut w = MsgWriter::new(&h);
        w.write_bits(1, 2); // CLC_MOVE_NO_DELTA
        w.write_byte(1); // count
        write_delta_usercmd(&mut w, 0x1122_3344, &NULL_USERCMD, &to);
        w.write_bits(3, 2); // CLC_EOF
        let ops = w.into_ops();
        let mut r = MsgReader::new(&h.compress_block(&ops), &h);
        assert_eq!(r.read_bits(2), 1, "clc op");
        assert_eq!(r.read_byte(), 1, "count");

        let mut w = MsgWriter::new(&h);
        w.write_bits(2, 2); // CLC_CLIENT_COMMAND
        w.write_long(1);
        w.write_string("say hello from vcod");
        w.write_bits(1, 2);
        w.write_byte(1);
        write_delta_usercmd(&mut w, 0x1122_3344, &NULL_USERCMD, &to);
        w.write_bits(3, 2);
        let ops = w.into_ops();
        let mut r = MsgReader::new(&h.compress_block(&ops), &h);
        assert_eq!(r.read_bits(2), 2);
        assert_eq!(r.read_long(), 1);
        assert_eq!(r.read_string(), "say hello from vcod");
        assert_eq!(r.read_bits(2), 1, "clc op after reliable");
        assert_eq!(r.read_byte(), 1, "count after reliable");
    }

    #[test]
    fn read_delta_usercmd_inverts_the_writer() {
        let h = Huffman::new();
        let mut base = NULL_USERCMD;
        base.server_time = 1000;
        let cases = [
            UserCmd {
                server_time: 1100,
                buttons: 1,
                angles: [0x1234, 0xabcd, 0],
                forward: 127,
                right: -127,
                ..NULL_USERCMD
            },
            UserCmd {
                server_time: 1000 + 300,
                buttons: 0,
                angles: [0xffff, 0, 0],
                forward: 0,
                right: 127,
                ..NULL_USERCMD
            },
            UserCmd {
                server_time: 900,
                buttons: 1,
                angles: [7, 8, 0],
                forward: -127,
                right: 0,
                ..NULL_USERCMD
            },
        ];
        for key in [0, 1, 0x1122_3344, -1] {
            let mut w = MsgWriter::new(&h);
            let mut from = base;
            for to in &cases {
                write_delta_usercmd(&mut w, key, &from, to);
                from = *to;
            }
            let mut r = MsgReader::new(&w.finish(), &h);
            let mut from = base;
            for to in &cases {
                let got = read_delta_usercmd(&mut r, key, &from).unwrap();
                assert_eq!(got, *to, "key {key:#x}");
                from = got;
            }
            assert!(!r.is_overflowed());
        }
    }

    /// A changed bit equal to `key & 1` returns the base cmd (0x807c0d9).
    #[test]
    fn read_delta_usercmd_copies_base_when_unchanged() {
        let h = Huffman::new();
        let mut base = NULL_USERCMD;
        base.server_time = 50;
        base.forward = 127;
        for key in [0, 1] {
            let mut w = MsgWriter::new(&h);
            w.write_bits(1, 1);
            w.write_byte(10); // +10 ms
            w.write_bits(key & 1, 1); // unchanged
            w.write_bits(1, 2); // a following clc op must still be readable
            let mut r = MsgReader::new(&w.finish(), &h);
            let got = read_delta_usercmd(&mut r, key, &base).unwrap();
            assert_eq!(
                got,
                UserCmd {
                    server_time: 60,
                    ..base
                }
            );
            assert_eq!(r.read_bits(2), 1);
        }
    }

    /// Hand-encoded off the full-branch table in docs/protocol-1.1.md, "Client
    /// to server message body".
    #[test]
    fn read_delta_usercmd_full_branch_matches_disassembly() {
        let h = Huffman::new();
        let key = 0x1122_3344i32;
        let from = UserCmd {
            server_time: 1000,
            buttons: 0b1010_1011,
            wbuttons: 3,
            weapon: 5,
            flags: 9,
            angles: [0x1111, 0x2222, 0x3333],
            forward: 100,
            right: -100,
            up: 20,
        };

        let mut w = MsgWriter::new(&h);
        w.write_bits(1, 1); // serverTime as an 8-bit delta
        w.write_byte(24); // -> 1024
        w.write_bits((key & 1) ^ 1, 1); // changed
        w.write_bits((key & 1) ^ 1, 1); // full-field branch

        // Keyed with the raw key: the serverTime mix comes later (0x807bde1).
        w.write_bits(1 ^ (key & 1), 1); // buttons bit 0 = 1
        w.write_bits(1, 1); // angles[0] follows
        w.write_short(((0xbeef ^ key) & 0xffff) as u16 as i16);
        w.write_bits(0, 1); // angles[1] keeps 0x2222
        w.write_bits(1, 1); // forward/right follows
        w.write_bits(0b1001 ^ (key & 0xf), 4); // forward +127, right -127

        let key2 = key ^ 1024;
        w.write_bits(1, 1); // angles[2] follows
        w.write_short(((0x4444 ^ key2) & 0xffff) as u16 as i16);
        w.write_bits(1, 1); // buttons bits 1-6 follow
        w.write_bits(0x2a ^ (key2 & 0x3f), 6);
        w.write_bits(1, 1); // wbuttons follows
        w.write_byte(0x77 ^ (key2 as u8));
        w.write_bits(0, 1); // up keeps the base's bucket -> +127
        w.write_bits(1, 1); // weapon follows
        w.write_bits(42 ^ (key2 & 0x3f), 6);
        w.write_bits(3, 2); // CLC_EOF

        let mut r = MsgReader::new(&w.finish(), &h);
        let got = read_delta_usercmd(&mut r, key, &from).unwrap();
        assert_eq!(
            got,
            UserCmd {
                server_time: 1024,
                buttons: 0b0101_0101,
                wbuttons: 0x77,
                weapon: 42,
                flags: 9, // never on the wire, kept from the base
                angles: [0xbeef, 0x2222, 0x4444],
                forward: 127,
                right: -127,
                up: 127,
            }
        );
        assert_eq!(r.read_bits(2), 3, "clc op after the move");
        assert!(!r.is_overflowed());
    }

    #[test]
    fn read_delta_usercmd_errs_on_a_truncated_message() {
        let h = Huffman::new();
        let key = 0x1122_3344i32;
        let mut w = MsgWriter::new(&h);
        w.write_bits(1, 1); // serverTime as an 8-bit delta
        w.write_byte(24);
        w.write_bits((key & 1) ^ 1, 1); // changed
        w.write_bits((key & 1) ^ 1, 1); // full-field branch

        // Only an announced field can run off the end: an absent change bit
        // reads as zero, "keep the base".
        w.write_bits(1 ^ (key & 1), 1); // buttons bit 0
        w.write_bits(1, 1); // angles[0] follows, and then it does not

        let mut r = MsgReader::new(&w.finish(), &h);
        let err = read_delta_usercmd(&mut r, key, &NULL_USERCMD).unwrap_err();
        assert!(err.to_string().contains("full"), "{err}");
        assert!(r.is_overflowed());
    }
}
