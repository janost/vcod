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

    /// The decompressed message, for spanning a region a parse just consumed.
    pub fn plain(&self) -> &[u8] {
        &self.data
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

    /// `MSG_ReadString` (CoDMP.exe 0x444e60 family).
    pub fn read_string(&mut self) -> String {
        self.read_string_capped(MAX_STRING_CHARS)
    }

    /// `MSG_ReadBigString` (CoDMP.exe 0x444e00): same byte mapping, bigger cap.
    pub fn read_big_string(&mut self) -> String {
        self.read_string_capped(BIG_INFO_STRING)
    }

    /// CoD 1.1 maps `%` -> `.`, 0x92 -> `'` and every other byte over 127 ->
    /// `.` in BOTH readers, unlike Q3/RTCW where the big reader keeps high
    /// bytes. Faithfulness matters beyond display: the usercmd delta key
    /// hashes the stored server-command string (`Com_HashKey` on raw bytes),
    /// so a client keeping high bytes - worse, re-encoded as two-byte UTF-8 -
    /// hashes differently from the server and every move it sends decodes as
    /// garbage until an ASCII-only command rotates into the acked slot
    /// (docs/protocol-1.1.md, divergence list).
    fn read_string_capped(&mut self, cap: usize) -> String {
        let mut s = String::new();
        let mut n = 0;
        while n < cap - 1 {
            let c = self.read_byte();
            if c == 0 || self.overflowed {
                break;
            }
            let c = match c {
                b'%' => b'.',
                0x92 => b'\'',
                c if c > 127 => b'.',
                c => c,
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

/// One primitive read from array blocks 4 and 5, in the order the parse took
/// it. Blocks 1 to 3 decode into [`PsArrays`]' named arrays instead. Those two
/// blocks reach the wire only through the shared field reader, which has no
/// byte or short form, so three variants cover them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PsArrayOp {
    Bits(i32, i32),
    Packed(i32, i32),
    Long(i32),
}

/// The playerState's five trailing array blocks (docs/protocol-1.1.md, "The
/// five array blocks"). Each block is gated against the base frame, so all
/// five are empty once a client is deltaing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PsArrays {
    /// Block 1, `ps.stats[6]`; index 0 health, 2 max health. The widths are
    /// per element: `stats[3]` is 6 unsigned bits and `stats[5]` a byte, so a
    /// retail `-1` in `stats[3]` arrives as 63.
    pub stats: [i32; 6],
    /// Block 2, `ps.ammo[64]`, the reserve, indexed by the weapon def's ammo
    /// index and not by weapon.
    pub ammo: [i16; 64],
    /// Block 3, `ps.ammoclip[64]`, the loaded magazine, by clip index.
    pub ammoclip: [i16; 64],
    /// Blocks 4 and 5 (objectives, and the two HUD-element arrays) as the
    /// primitives the parse consumed, replayed verbatim: nothing in vcod
    /// writes either. Empty means both gates are clear.
    pub tail: Vec<PsArrayOp>,
}

impl Default for PsArrays {
    fn default() -> Self {
        PsArrays {
            stats: [0; 6],
            ammo: [0; 64],
            ammoclip: [0; 64],
            tail: Vec::new(),
        }
    }
}

/// Raw wire values in `Protocol::player_fields` order, floats as bit patterns.
/// The trailing array blocks are not in that table; `arrays` carries them.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct PlayerState {
    pub fields: Vec<i32>,
    pub arrays: PsArrays,
}

impl PlayerState {
    pub fn null(p: &Protocol) -> Self {
        PlayerState {
            fields: vec![0; p.player_fields.len()],
            arrays: PsArrays::default(),
        }
    }

    pub fn health(&self) -> i32 {
        self.arrays.stats[0]
    }

    pub fn set_health(&mut self, v: i32) {
        self.arrays.stats[0] = v;
    }

    pub fn max_health(&self) -> i32 {
        self.arrays.stats[2]
    }

    pub fn set_max_health(&mut self, v: i32) {
        self.arrays.stats[2] = v;
    }

    /// The reserve behind the magazine, by the weapon def's ammo index.
    pub fn ammo(&self, index: usize) -> i16 {
        self.arrays.ammo[index]
    }

    pub fn set_ammo(&mut self, index: usize, v: i16) {
        self.arrays.ammo[index] = v;
    }

    /// The loaded magazine, by the weapon def's clip index. Firing spends
    /// this, not the reserve.
    pub fn clip(&self, index: usize) -> i16 {
        self.arrays.ammoclip[index]
    }

    pub fn set_clip(&mut self, index: usize, v: i16) {
        self.arrays.ammoclip[index] = v;
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
        arrays: PsArrays::default(),
    };
    let lc = (r.read_byte() as usize).min(p.player_fields.len());
    for (i, field) in p.player_fields.iter().take(lc).enumerate() {
        if r.read_bits(1) == 0 {
            continue;
        }
        to.fields[i] = if field.bits == 0 {
            let v = read_ps_float(r);
            // The change bit says the C saw two different ints, but this codec
            // has no zero flag, so a `-0.0` comes back `+0.0` and would
            // re-encode as unchanged. Keep the sign the entity codec keeps
            // (docs/protocol-1.1.md, "PlayerState delta"). The integral path
            // is exact for everything else, so `v == base` can only mean the
            // two zeroes; the guard says so rather than trusting it.
            if v == 0 && to.fields[i] == 0 {
                (-0f32).to_bits() as i32
            } else {
                v
            }
        } else {
            // Unsigned, width `|bits|`, no sign extension (0x807e5d7).
            r.read_packed_bits(field.bits)
        };
    }
    to.arrays = read_ps_arrays(r, &from.arrays);
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

/// Mirror of [`read_delta_playerstate`]. The trailing array blocks are gated
/// against `from` the same way the scalar fields are. Unlike the entity codec
/// there is no zero-flag bit here, so a `-0.0` float goes out as `+0.0`; the
/// reader keeps the sign so the field still counts as changed.
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
    write_ps_arrays(w, &from.arrays, &to.arrays);
}

/// Mirror of [`read_ps_arrays`]. Blocks 1 to 3 carry only what differs from
/// the base; blocks 4 and 5 replay `to.tail`, which is empty for a
/// server-built state and goes out as their two clear gates.
fn write_ps_arrays(w: &mut MsgWriter, from: &PsArrays, to: &PsArrays) {
    // Block 1: the gate, then six raw bits selecting the changed scalars.
    let mut mask = 0i32;
    for i in 0..6 {
        if from.stats[i] != to.stats[i] {
            mask |= 1 << i;
        }
    }
    if mask == 0 {
        w.write_bits(0, 1);
    } else {
        w.write_bits(1, 1);
        w.write_bits(mask, 6);
        if mask & 0x01 != 0 {
            w.write_short(to.stats[0] as i16);
        }
        if mask & 0x02 != 0 {
            w.write_short(to.stats[1] as i16);
        }
        if mask & 0x04 != 0 {
            w.write_short(to.stats[2] as i16);
        }
        if mask & 0x08 != 0 {
            w.write_bits(to.stats[3], 6);
        }
        if mask & 0x10 != 0 {
            w.write_short(to.stats[4] as i16);
        }
        if mask & 0x20 != 0 {
            w.write_byte(to.stats[5] as u8);
        }
    }
    // Block 2 behind its group gate, block 3 without one.
    if from.ammo == to.ammo {
        w.write_bits(0, 1);
    } else {
        w.write_bits(1, 1);
        write_short_array_group(w, &from.ammo, &to.ammo);
    }
    write_short_array_group(w, &from.ammoclip, &to.ammoclip);
    if to.tail.is_empty() {
        w.write_bits(0, 1);
        w.write_bits(0, 1);
        return;
    }
    for &op in &to.tail {
        match op {
            PsArrayOp::Bits(bits, v) => w.write_bits(v, bits),
            PsArrayOp::Packed(bits, v) => w.write_packed_bits(v, bits),
            PsArrayOp::Long(v) => w.write_long(v),
        }
    }
}

/// Mirror of [`read_short_array_group`]: four sub-blocks of 16, each a gate,
/// a byte-aligned 16-bit changed mask and the changed elements.
fn write_short_array_group(w: &mut MsgWriter, from: &[i16; 64], to: &[i16; 64]) {
    for s in 0..4 {
        let base = s * 16;
        let mut mask = 0u16;
        for i in 0..16 {
            if from[base + i] != to[base + i] {
                mask |= 1 << i;
            }
        }
        if mask == 0 {
            w.write_bits(0, 1);
            continue;
        }
        w.write_bits(1, 1);
        w.write_short(mask as i16);
        for i in 0..16 {
            if mask & (1 << i) != 0 {
                w.write_short(to[base + i]);
            }
        }
    }
}

/// Widths of the 34-entry field table at cod_lnxded 0x80de384. 0..6 back the
/// objective block, 6..34 the two HUD-element arrays.
/// docs/protocol-1.1.md, "The five array blocks".
#[rustfmt::skip]
const HUD_FIELD_BITS: [i32; 34] = [
    0, 0, 0, 12, 10, 4,   // origin[0..2], icon, entNum, teamNum
    32, 4, 0, 10, 10, 2,  // color.rgba, type, fontScale, y, x, alignY
    2, 32, 4, 8, 8, 10, 10, 0, 32, 32, 16, 32, 16, 10, 0, 8, 10, 32, 16, 10, 10, 32,
];

/// Records every primitive it reads, so [`write_ps_arrays`] can put blocks 4
/// and 5 back on the wire unchanged.
struct ArrayReader<'a> {
    r: &'a mut MsgReader,
    ops: Vec<PsArrayOp>,
}

impl ArrayReader<'_> {
    fn bits(&mut self, bits: i32) -> i32 {
        let v = self.r.read_bits(bits);
        self.ops.push(PsArrayOp::Bits(bits, v));
        v
    }

    fn packed(&mut self, bits: i32) {
        let v = self.r.read_packed_bits(bits);
        self.ops.push(PsArrayOp::Packed(bits, v));
    }

    fn long(&mut self) {
        let v = self.r.read_long();
        self.ops.push(PsArrayOp::Long(v));
    }

    /// [`read_delta_field`] with the value thrown away; the base is always 0
    /// inside these blocks.
    fn delta_field(&mut self, bits: i32) {
        if self.bits(1) == 0 {
            return;
        }
        if bits == 0 {
            // Zero flag, then the integral/full selector.
            if self.bits(1) == 0 {
                return;
            }
            if self.bits(1) == 0 {
                self.packed(FLOAT_INT_BITS);
            } else {
                self.long();
            }
        } else if self.bits(1) != 0 {
            self.packed(bits);
        }
    }
}

/// The trailing array blocks (cod_lnxded 0x807e7b3..0x807eeb6). Everything a
/// block does not carry keeps the base frame's value.
fn read_ps_arrays(r: &mut MsgReader, from: &PsArrays) -> PsArrays {
    let mut out = PsArrays {
        stats: from.stats,
        ammo: from.ammo,
        ammoclip: from.ammoclip,
        tail: Vec::new(),
    };
    // Block 1: a 6-bit mask selecting up to six scalars, each its own width.
    if r.read_bits(1) == 1 {
        let m = r.read_bits(6);
        if m & 0x01 != 0 {
            out.stats[0] = i32::from(r.read_short());
        }
        if m & 0x02 != 0 {
            out.stats[1] = i32::from(r.read_short());
        }
        if m & 0x04 != 0 {
            out.stats[2] = i32::from(r.read_short());
        }
        if m & 0x08 != 0 {
            out.stats[3] = r.read_bits(6);
        }
        if m & 0x10 != 0 {
            out.stats[4] = i32::from(r.read_short());
        }
        if m & 0x20 != 0 {
            out.stats[5] = i32::from(r.read_byte());
        }
    }
    // Block 2: ps.ammo[64] behind a group gate.
    if r.read_bits(1) == 1 {
        read_short_array_group(r, &mut out.ammo);
    }
    // Block 3: ps.ammoclip[64], the same four sub-blocks with no group gate.
    read_short_array_group(r, &mut out.ammoclip);

    let mut a = ArrayReader { r, ops: Vec::new() };
    // Block 4: ps.objective[16], each a 3-bit state then six delta fields.
    if a.bits(1) == 1 {
        for _ in 0..16 {
            a.bits(3);
            if a.bits(1) == 1 {
                for &bits in &HUD_FIELD_BITS[0..6] {
                    a.delta_field(bits);
                }
            }
        }
    }
    // Block 5: two HUD-element arrays.
    if a.bits(1) == 1 {
        read_hud_array(&mut a);
        read_hud_array(&mut a);
    }
    // Two clear gates and an empty record mean the same two zero bits, and
    // dropping them lets a parsed state compare equal to a built one.
    if !a.ops.iter().all(|&op| op == PsArrayOp::Bits(1, 0)) {
        out.tail = a.ops;
    }
    out
}

/// Four gated 16-entry sub-blocks (cod_lnxded 0x807ea4b / 0x807eb49): a gate,
/// a byte-aligned 16-bit changed mask, then the changed elements low to high.
fn read_short_array_group(r: &mut MsgReader, out: &mut [i16; 64]) {
    for s in 0..4 {
        if r.read_bits(1) == 0 {
            continue;
        }
        let mask = r.read_short() as u16;
        for i in 0..16 {
            if mask & (1 << i) != 0 {
                out[s * 16 + i] = r.read_short();
            }
        }
    }
}

/// One HUD-element array (cod_lnxded 0x807cf5c).
fn read_hud_array(a: &mut ArrayReader) {
    let count = a.bits(5);
    for _ in 0..count {
        let j = a.bits(5);
        for k in 0..=j {
            let idx = (6 + k) as usize;
            let bits = HUD_FIELD_BITS.get(idx).copied().unwrap_or(0);
            a.delta_field(bits);
            if a.r.is_overflowed() {
                return;
            }
        }
    }
}

/// `usercmd_t` (codextended shared.h:811). The writer's compact branch
/// carries none of `up`, `weapon`, `wbuttons`, `flags` or `angles[2]`; a
/// change in the first three (or button bits above 0) selects the
/// full-field branch. Layout:
/// docs/protocol-1.1.md, "Client to server message body".
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

/// Input bits in [`UserCmd::buttons`] and [`UserCmd::wbuttons`]. The two
/// stances are level states the client holds for as long as it is down, not
/// key edges, and they are mutually exclusive; `reload` is in `wbuttons`
/// because it is the one bit there that moves nothing. Bit table and
/// evidence: docs/protocol-1.1.md, "Usercmd input bits".
pub const BUTTON_ATTACK: u8 = 0x01;
pub const BUTTON_ADS: u8 = 0x10;
pub const BUTTON_MELEE: u8 = 0x20;
pub const BUTTON_USE: u8 = 0x40;
pub const WBUTTON_RELOAD: u8 = 0x08;
pub const WBUTTON_LEAN_LEFT: u8 = 0x10;
pub const WBUTTON_LEAN_RIGHT: u8 = 0x20;
pub const WBUTTON_PRONE: u8 = 0x40;
pub const WBUTTON_CROUCH: u8 = 0x80;

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
/// 0x807b7f8. Emits the compact branch unless a field it cannot carry
/// (`up`, `weapon`, `wbuttons`, button bits above 0) differs from `from`:
/// the compact encoding has no slot for them, so the receiver would keep
/// its stored value. Forward/right can only be +127, -127 or 0; `flags`
/// is never sent.
///
/// Angles and the forward/right code are always announced; every other
/// field rides a change bit against `from`, so an omitted field decodes
/// against the receiver's stored previous cmd — retail's outCmd chaining,
/// where `from` is the client's last *sent* cmd. A lost packet degrades
/// one cmd, which retail also lives with.
/// docs/protocol-1.1.md, "Client to server message body".
pub fn write_delta_usercmd(w: &mut MsgWriter, key: i32, from: &UserCmd, to: &UserCmd) {
    // serverTime preamble: 1 = 8-bit delta from the base, 0 = 32-bit absolute.
    // The server decodes the 8-bit delta against its last *received* cmd, while
    // `from` here is our last *sent* one. We send a single usercmd per message
    // with no backup copies, so one dropped or reordered packet desyncs the two
    // chains and the server then rejects every later cmd (commandTime freezes,
    // spectator flight stops until reconnect). Retail's redundant backup cmds
    // heal such a gap; lacking those, always send the absolute serverTime.
    let _ = from;
    w.write_bits(0, 1);
    w.write_long(to.server_time);

    // Changed bit (!= key & 1), then the branch bit (== key & 1 picks compact).
    let full = to.up != from.up
        || to.weapon != from.weapon
        || to.wbuttons != from.wbuttons
        || (to.buttons & !1) != (from.buttons & !1);
    w.write_bits((key & 1) ^ 1, 1);
    w.write_bits(if full { (key & 1) ^ 1 } else { key & 1 }, 1);

    if full {
        write_full_usercmd(w, key, to.server_time, to);
        return;
    }

    // The reader mixes the serverTime into the key from here (0x807b95d).
    let key = key ^ to.server_time;

    w.write_bits(((to.buttons as i32) & 1) ^ (key & 1), 1);

    write_keyed_angle(w, key, to.angles[0]);
    write_keyed_angle(w, key, to.angles[1]);

    let flag = fr_bucket(to.forward, to.right);
    w.write_bits(1, 1);
    w.write_bits(flag ^ (key & 0xf), 4);
}

/// Writer mirror of [`read_full_usercmd`] (cod_lnxded 0x807bba0): every keyed
/// field announced, in the reader's order, first four under the raw key and
/// the rest under the serverTime-mixed one.
fn write_full_usercmd(w: &mut MsgWriter, key: i32, server_time: i32, to: &UserCmd) {
    w.write_bits(((to.buttons as i32) & 1) ^ (key & 1), 1);
    write_keyed_angle(w, key, to.angles[0]);
    write_keyed_angle(w, key, to.angles[1]);
    let flag = fr_bucket(to.forward, to.right);
    w.write_bits(1, 1);
    w.write_bits(flag ^ (key & 0xf), 4);

    let key = key ^ server_time;
    write_keyed_angle(w, key, to.angles[2]);
    w.write_bits(1, 1);
    w.write_bits((i32::from(to.buttons >> 1) & 0x3f) ^ (key & 0x3f), 6);
    w.write_bits(1, 1);
    w.write_byte(to.wbuttons ^ (key as u8));
    w.write_bits(1, 1);
    w.write_bits(up_bucket(to.up) ^ (key & 0x3), 2);
    w.write_bits(1, 1);
    w.write_bits(i32::from(to.weapon) ^ (key & 0x3f), 6);
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

    /// CoDMP.exe 0x444e00: `%` -> `.`, 0x92 -> `'`, other high bytes -> `.`,
    /// in the big reader too (CoD divergence from Q3, which keeps high bytes
    /// there). The usercmd delta key hashes these strings, so the mapping is
    /// wire-compatibility, not cosmetics.
    #[test]
    fn big_string_maps_percent_and_high_bytes_like_retail() {
        let h = Huffman::new();
        let mut w = MsgWriter::new(&h);
        for &b in b"caf\xe9%s\x92\x15" {
            w.write_byte(b);
        }
        w.write_byte(0);
        let data = w.finish();
        let mut r = MsgReader::new(&data, &h);
        assert_eq!(r.read_big_string(), "caf..s'\u{15}");
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

        // Block 4: only objective 0 carries its six fields.
        w.write_bits(1, 1);
        for obj in 0..16 {
            w.write_bits(3, 3); // 3-bit value
            if obj == 0 {
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
        assert_eq!(ps.arrays.stats, [11, 22, 33, 4, 44, 55]);
        assert_eq!((ps.ammo(0), ps.ammo(2)), (1, 2));
        assert_eq!(ps.clip(0), 9);
        assert_eq!((ps.ammo(1), ps.clip(1)), (0, 0), "unmasked elements stay 0");
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

    /// Health, a clip and a reserve set by name go out against a null base and
    /// come back the same numbers.
    #[test]
    fn array_blocks_round_trip_named_fields() {
        let p = &PROTOCOL_V1;
        let h = Huffman::new();
        let from = PlayerState::null(p);
        let mut to = PlayerState::null(p);
        to.set_health(100);
        to.set_max_health(100);
        to.set_clip(3, 15);
        to.set_ammo(1, 300);

        let mut w = MsgWriter::new(&h);
        write_delta_playerstate(&mut w, p, &from, &to);
        let bits = w.bits_written();
        let d = w.finish();
        let mut r = MsgReader::new(&d, &h);
        let back = read_delta_playerstate(&mut r, p, &from);
        assert_eq!(back.health(), 100);
        assert_eq!(back.max_health(), 100);
        assert_eq!(back.clip(3), 15);
        assert_eq!(back.ammo(1), 300);
        assert_eq!(back, to);
        assert_eq!(
            r.bits_read(),
            bits,
            "writer and reader must agree bit for bit"
        );
    }

    /// The changed-mask path: a base that already carries ammo and a clip, and
    /// a frame where one clip element moves. No committed capture sets a block
    /// 2 or 3 gate, so this is the only cover it has.
    #[test]
    fn one_changed_clip_element_deltas_against_a_loaded_base() {
        let p = &PROTOCOL_V1;
        let h = Huffman::new();
        let mut base = PlayerState::null(p);
        base.set_health(100);
        base.set_ammo(1, 300);
        base.set_clip(3, 15);
        // Second sub-block, so an untouched sub-block has to stay gated off.
        base.set_clip(20, 8);
        let mut to = base.clone();
        to.set_clip(3, 14);

        let mut w = MsgWriter::new(&h);
        write_delta_playerstate(&mut w, p, &base, &to);
        let bits = w.bits_written();
        let ops = w.into_ops().len();
        let mut w2 = MsgWriter::new(&h);
        write_delta_playerstate(&mut w2, p, &base, &base);
        let quiet = w2.into_ops().len();
        // The whole cost of the change is the sub-block's 16-bit mask and the
        // one 16-bit element; every gate bit fits the byte the quiet frame
        // already spends.
        assert_eq!(ops, quiet + 4);

        let mut w = MsgWriter::new(&h);
        write_delta_playerstate(&mut w, p, &base, &to);
        let d = w.finish();
        let mut r = MsgReader::new(&d, &h);
        let back = read_delta_playerstate(&mut r, p, &base);
        assert_eq!(back.clip(3), 14);
        assert_eq!(back.clip(20), 8, "an unchanged sub-block keeps the base");
        assert_eq!(back.health(), 100);
        assert_eq!(back.ammo(1), 300);
        assert_eq!(back, to);
        assert_eq!(
            r.bits_read(),
            bits,
            "writer and reader must agree bit for bit"
        );
    }

    /// The count byte after a 2-bit clc op lands at the byte cursor, alone and
    /// behind a reliable `clc_clientCommand`.
    /// The serverTime must survive a decode against a base the server never
    /// received (a dropped usercmd): we send absolute time, so it reconstructs
    /// correctly regardless of the server's stored base. An 8-bit delta anchored
    /// to our last sent cmd would reconstruct wrong here and freeze commandTime.
    #[test]
    fn server_time_survives_a_gap_in_the_received_chain() {
        let h = Huffman::new();
        let key = 0x1122_3344i32;
        let sent = UserCmd {
            server_time: 100_500,
            forward: 127,
            ..NULL_USERCMD
        };
        // Our previous sent cmd was at 100_484 (16 ms back); the server, having
        // dropped everything since NULL, decodes against server_time 0.
        let our_prev = UserCmd {
            server_time: 100_484,
            ..NULL_USERCMD
        };
        let mut w = MsgWriter::new(&h);
        write_delta_usercmd(&mut w, key, &our_prev, &sent);
        let mut r = MsgReader::new(&w.finish(), &h);
        let got = read_delta_usercmd(&mut r, key, &NULL_USERCMD).unwrap();
        assert_eq!(got.server_time, 100_500);
        assert_eq!(got.forward, 127);
    }

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

    /// The full-field branch is the only one that carries `up`; a nonzero
    /// upmove must select it on write and survive the keyed decode exactly,
    /// bit count included.
    #[test]
    fn written_upmove_round_trips_through_the_full_branch() {
        let h = Huffman::new();
        let cases = [
            UserCmd {
                server_time: 1050,
                buttons: 0x15,
                wbuttons: 3,
                weapon: 5,
                angles: [0x1234, 0xabcd, 0xbeef],
                forward: 127,
                right: -127,
                up: 127,
                ..NULL_USERCMD
            },
            UserCmd {
                server_time: 1150,
                buttons: 0x2a,
                wbuttons: 0x81,
                weapon: 42,
                angles: [0xffff, 0, 0x1234],
                forward: -127,
                right: 127,
                up: -127,
                ..NULL_USERCMD
            },
        ];
        for key in [0, 1, 0x1122_3344, -1] {
            let mut w = MsgWriter::new(&h);
            let mut from = NULL_USERCMD;
            for to in &cases {
                write_delta_usercmd(&mut w, key, &from, to);
                from = *to;
            }
            let bits = w.bits_written();
            let mut r = MsgReader::new(&w.finish(), &h);
            let mut from = NULL_USERCMD;
            for to in &cases {
                let got = read_delta_usercmd(&mut r, key, &from).unwrap();
                assert_eq!(got, *to, "key {key:#x}");
                from = got;
            }
            assert_eq!(
                r.bits_read(),
                bits,
                "writer and reader must agree bit for bit (key {key:#x})"
            );
            assert!(!r.is_overflowed());
        }
    }

    /// Press then release: the release (up back to 0) differs from the
    /// previous sent cmd in a field the compact branch cannot carry, so the
    /// writer must take the full branch again. Chained decode must read the
    /// release as 0, not replay the jump off the receiver's stored cmd.
    #[test]
    fn released_upmove_takes_the_full_branch_again_on_the_way_down() {
        let h = Huffman::new();
        let key = 0x1122_3344i32;
        let press = UserCmd {
            server_time: 500,
            up: 127,
            ..NULL_USERCMD
        };
        let release = UserCmd {
            server_time: 550,
            ..NULL_USERCMD
        };

        let mut w = MsgWriter::new(&h);
        write_delta_usercmd(&mut w, key, &NULL_USERCMD, &press);
        write_delta_usercmd(&mut w, key, &press, &release);
        let mut r = MsgReader::new(&w.finish(), &h);
        let got_press = read_delta_usercmd(&mut r, key, &NULL_USERCMD).unwrap();
        assert_eq!(got_press, press);
        let got_release = read_delta_usercmd(&mut r, key, &got_press).unwrap();
        assert_eq!(got_release.up, 0, "the release must not replay the jump");
        assert_eq!(got_release, release);

        // Up never leaves 0 across the pair: both cmds ride the compact branch.
        let quiet_a = UserCmd {
            server_time: 500,
            ..NULL_USERCMD
        };
        let quiet_b = UserCmd {
            server_time: 550,
            ..NULL_USERCMD
        };
        let mut w = MsgWriter::new(&h);
        write_delta_usercmd(&mut w, key, &NULL_USERCMD, &quiet_a);
        write_delta_usercmd(&mut w, key, &quiet_a, &quiet_b);
        let mut r = MsgReader::new(&w.finish(), &h);
        let got_a = read_delta_usercmd(&mut r, key, &NULL_USERCMD).unwrap();
        assert_eq!(got_a, quiet_a);
        let got_b = read_delta_usercmd(&mut r, key, &got_a).unwrap();
        assert_eq!(got_b, quiet_b);
    }

    /// A cmd without upmove must encode exactly as before the full branch
    /// existed, byte for byte.
    #[test]
    fn zero_upmove_writes_the_compact_bytes_unchanged() {
        let h = Huffman::new();
        let to = UserCmd {
            server_time: 1100,
            buttons: 1,
            angles: [0x1234, 0xabcd, 0],
            forward: 127,
            right: -127,
            ..NULL_USERCMD
        };
        let mut w = MsgWriter::new(&h);
        write_delta_usercmd(&mut w, 0x1122_3344, &NULL_USERCMD, &to);
        let d = w.finish();
        assert_eq!(d, [249, 118, 14, 213, 145, 242, 63, 126, 37, 1]);
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
