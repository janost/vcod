//! Contains routines ported from the RTCW-MP GPL source, Copyright (C) 1999-2010 id Software LLC, a ZeniMax Media company.
//! See NOTICE.
//!
//! The engine-neutral message codec: the bit reader and writer and the
//! table-driven delta field codec every game's netfield tables run through.
//! Ported from RTCW-MP `qcommon/msg.c` and corrected against cod_lnxded 1.1d
//! and cod2_lnxded 1.3. Wire layout: `docs/protocol-1.1.md`, "The
//! message-block model" and "Delta field encoding"; CoD2's field codes:
//! docs/research/cod2-protocol.md 5.2.
//!
//! The body is block-decompressed up front and read as plain bytes with two
//! cursors, `readcount` for whole bytes and `bit` for bit fields, as `msg_t`
//! does. A raw byte read in the middle of a run of bit fields lands after the
//! byte those bits are still filling.
//!
//! A game's tables are `&[NetField]`; what a field's `bits` means is
//! [`NetField`]'s doc, and the one rule that differs between games, whether
//! a negative width sign-extends, is the [`FieldCodec`] the caller reads
//! with. Nothing here knows a game's numbering.

use super::huffman::{Huffman, add_bit, get_bit};

const MAX_STRING_CHARS: usize = 1024;
pub(crate) const BIG_INFO_STRING: usize = 8192;
/// A float that is integral and inside this range goes out truncated.
pub(crate) const FLOAT_INT_BITS: i32 = 13;
pub(crate) const FLOAT_INT_BIAS: i32 = 1 << (FLOAT_INT_BITS - 1);
/// [`FLOAT10_BITS`]'s integral range: 2 bits and a byte, biased by 0x200
/// (cod2_lnxded `MSG_ReadDeltaField` 0x8069960).
const FLOAT10_INT_BITS: i32 = 10;

/// One netfield, as retail's `netField_t` `{name, offset, bits}`. `bits`
/// is the table's own code, kept verbatim:
///
/// - `0`: a float. Behind the zero flag, a 13-bit biased integer or the raw
///   32 bits ([`read_field_value`]).
/// - [`ANGLE_BITS`] (-100): a 16-bit angle.
/// - [`FLOAT10_BITS`] (-99): a float with a 10-bit integral form (CoD2).
/// - any other: an integer `|bits|` wide, `|bits| & 7` loose bits then whole
///   bytes. A negative width is unsigned on CoD1 and sign-extends on CoD2
///   ([`FieldCodec`]).
///
/// `offset` is the engine struct offset, used only as a stable identity.
#[derive(Clone, Copy, Debug)]
pub struct NetField {
    pub name: &'static str,
    pub offset: u16,
    pub bits: i32,
}

/// A 16-bit angle field: a zero flag, then a short of `deg * 65536 / 360`
/// truncated toward zero (coduo_lnxded 0x8080815, cod2_lnxded 0x8069960).
/// Decoded, it holds the f32 bit pattern of the degrees, as a float field
/// does.
pub const ANGLE_BITS: i32 = -100;

/// CoD2's small float: a zero flag, then a bit: clear, a 10-bit integer
/// biased by 0x200 (an integral float in [-512, 511]); set, the raw 32 bits
/// (cod2_lnxded `MSG_ReadDeltaField` 0x8069960; cod2-protocol.md 5.2). No
/// CoD1 table uses it. Decoded, it holds the f32 bit pattern.
pub const FLOAT10_BITS: i32 = -99;

/// The rule that differs between games' field codecs: how an integer field
/// with a negative width reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FieldCodec {
    /// CoD1 reads a negative width as a plain unsigned width (cod_lnxded
    /// 0x807c904); CoD2 sign-extends from bit `|bits| - 1` (cod2_lnxded
    /// 0x8069960, the `| -1 << bits` after the read).
    pub signed_widths: bool,
}

impl FieldCodec {
    pub const COD1: FieldCodec = FieldCodec {
        signed_widths: false,
    };
    pub const COD2: FieldCodec = FieldCodec {
        signed_widths: true,
    };

    /// An integer field's packed value, sign-extended where the codec says.
    pub fn read_int(self, r: &mut MsgReader, bits: i32) -> i32 {
        let v = r.read_packed_bits(bits);
        let width = (bits.unsigned_abs() as usize).min(32);
        sign_extend(v as u32, width, self.signed_widths && bits < 0)
    }

    /// One delta field (cod_lnxded 0x807c904, cod2_lnxded 0x8069960): the
    /// change bit, then [`Self::read_field_value`]. The entity, client,
    /// objective and hud element codec.
    pub fn read_delta_field(self, r: &mut MsgReader, from_val: i32, bits: i32) -> i32 {
        if r.read_bits(1) == 0 {
            return from_val;
        }
        self.read_field_value(r, from_val, bits)
    }

    /// [`Self::read_delta_field`] past its change bit: the zero flag, then
    /// the value by `bits` ([`NetField`]).
    pub fn read_field_value(self, r: &mut MsgReader, from_val: i32, bits: i32) -> i32 {
        match bits {
            0 => read_float_field(r, from_val, FLOAT_INT_BITS),
            FLOAT10_BITS => read_float_field(r, from_val, FLOAT10_INT_BITS),
            ANGLE_BITS => read_angle_field(r, from_val),
            _ if r.read_bits(1) == 0 => 0,
            // Behind the set zero flag the C's int was not 0, so a packed 0 or
            // the base's own value says it differed above the width.
            _ => above_width(self.read_int(r, bits), from_val, bits, true),
        }
    }

    /// The playerState's field value past its change bit (cod_lnxded
    /// 0x807e2f0, coduo_lnxded 0x8082826, cod2_lnxded 0x806aaca): floats and
    /// integers carry no zero flag; an angle is [`ANGLE_BITS`]' as anywhere.
    pub fn read_ps_value(self, r: &mut MsgReader, from_val: i32, bits: i32) -> i32 {
        if bits == 0 {
            let v = read_ps_float(r);
            // The change bit says the C saw two different ints, but this codec
            // has no zero flag, so a `-0.0` comes back `+0.0` and would
            // re-encode as unchanged. Keep the sign the entity codec keeps
            // (docs/protocol-1.1.md, "PlayerState delta"). The integral path
            // is exact for everything else, so `v == base` can only mean the
            // two zeroes; the guard says so rather than trusting it.
            if v == 0 && from_val == 0 {
                (-0f32).to_bits() as i32
            } else {
                v
            }
        } else if bits == ANGLE_BITS {
            read_angle_field(r, from_val)
        } else {
            // A `delta_angles` can differ from its base only past its 16 bits.
            above_width(self.read_int(r, bits), from_val, bits, false)
        }
    }

    /// A whole struct delta, `MSG_ReadDeltaStruct`'s shape (cod_lnxded
    /// 0x807f758, cod2_lnxded 0x8069d4a) past whatever the caller read
    /// before it: a remove bit, a no-change bit, an `lc` byte, then `lc`
    /// fields of `table`, the rest kept from `from`. `None` is a remove.
    pub fn read_delta_struct(
        self,
        r: &mut MsgReader,
        table: &[NetField],
        from: &[i32],
    ) -> Option<Vec<i32>> {
        if r.read_bits(1) == 1 {
            return None;
        }
        let mut to = from.to_vec();
        if r.read_bits(1) == 0 {
            return Some(to);
        }
        let lc = (r.read_byte() as usize).min(table.len());
        for (i, field) in table.iter().take(lc).enumerate() {
            to[i] = self.read_delta_field(r, to[i], field.bits);
        }
        Some(to)
    }
}

/// Inverse of [`FieldCodec::read_delta_struct`]; `None` writes the remove
/// bit. Slots compare as raw ints, as in the C.
pub fn write_delta_struct(w: &mut MsgWriter, table: &[NetField], from: &[i32], to: Option<&[i32]>) {
    let Some(to) = to else {
        w.write_bits(1, 1); // removed
        return;
    };
    debug_assert_eq!(from.len(), to.len());
    w.write_bits(0, 1);
    let lc = from
        .iter()
        .zip(to)
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
        write_delta_field(w, from[i], to[i], table[i].bits);
    }
}

/// Inverse of [`FieldCodec::read_delta_field`]. The writer masks to the
/// width, so it needs no [`FieldCodec`].
pub fn write_delta_field(w: &mut MsgWriter, from_val: i32, to_val: i32, bits: i32) {
    if from_val == to_val {
        w.write_bits(0, 1);
        return;
    }
    w.write_bits(1, 1);
    match bits {
        0 => write_float_field(w, to_val, FLOAT_INT_BITS),
        FLOAT10_BITS => write_float_field(w, to_val, FLOAT10_INT_BITS),
        ANGLE_BITS => write_angle_field(w, to_val),
        _ if to_val == 0 => w.write_bits(0, 1),
        _ => {
            w.write_bits(1, 1);
            w.write_packed_bits(to_val, bits);
        }
    }
}

/// Inverse of [`FieldCodec::read_ps_value`], past the change bit.
pub fn write_ps_value(w: &mut MsgWriter, v: i32, bits: i32) {
    if bits == 0 {
        write_ps_float(w, v);
    } else if bits == ANGLE_BITS {
        write_angle_field(w, v);
    } else {
        w.write_packed_bits(v, bits);
    }
}

/// A packed integer the change bit (and for an entity the zero flag) says
/// the sender's int held differently from the base and from 0, read back
/// as the base or as 0 because the difference sat above the field's width:
/// flip bits above the width until it differs, so the writer, which masks to
/// the width, sends the same bits for it. A flip, not a set: a sign-extended
/// value already has every bit above its width set.
fn above_width(v: i32, from_val: i32, bits: i32, nonzero: bool) -> i32 {
    let width = bits.unsigned_abs();
    if width > 28 {
        return v;
    }
    let mut out = v;
    let mut k = 0;
    while out == from_val || (nonzero && out == 0) {
        k += 1;
        out = v ^ (k << width);
    }
    out
}

/// Degrees per unit of a 16-bit wire angle.
const ANGLE_UNIT: f32 = 360.0 / 65536.0;

/// [`ANGLE_BITS`]' reader (coduo_lnxded 0x8081850): a zero flag, then a raw
/// short. The change bit says the sender's two floats differed, which two
/// angles one short apart need not after quantizing; a value equal to the
/// base is nudged off it in its last mantissa bit, which the writer still
/// rounds to the same short, so a capture re-encodes byte for byte. A short
/// of 0 behind a set zero flag (an angle under half a unit) decodes to the
/// smallest float above zero for the same reason.
pub(crate) fn read_angle_field(r: &mut MsgReader, from_val: i32) -> i32 {
    let v = if r.read_bits(1) == 0 {
        0
    } else {
        match r.read_short() {
            0 => 1,
            s => (f32::from(s) * ANGLE_UNIT).to_bits() as i32,
        }
    };
    if v != from_val {
        v
    } else if v & 0x7fff_ffff == 0 {
        v ^ i32::MIN // the other zero
    } else if v == 1 {
        2
    } else {
        v ^ 1
    }
}

/// Inverse of [`read_angle_field`]: `deg * 65536 / 360` truncated toward
/// zero, the constant the f32 at coduo_lnxded 0x80e292c, stored under
/// control word 0xc00 (0x8080815). A short the reader decoded comes back
/// as itself: the f32 constant is a hair over 65536 / 360.
pub(crate) fn write_angle_field(w: &mut MsgWriter, raw: i32) {
    let f = f32::from_bits(raw as u32);
    if f == 0.0 {
        w.write_bits(0, 1);
        return;
    }
    w.write_bits(1, 1);
    let units = (f64::from(f) * f64::from(65536.0f32 / 360.0)) as i64;
    w.write_short(units as u16 as i16);
}

/// Zero flag, then an `int_bits` biased integer or the raw 32 bits. A clear
/// zero flag from a `+0.0` base means the sender held `-0.0` (the C compares
/// slots as ints); keeping it lets a writer round-trip a capture byte for
/// byte. docs/protocol-1.1.md, "Entity delta".
fn read_float_field(r: &mut MsgReader, from_val: i32, int_bits: i32) -> i32 {
    if r.read_bits(1) == 0 {
        return if from_val == 0 {
            (-0f32).to_bits() as i32
        } else {
            0f32.to_bits() as i32
        };
    }
    if r.read_bits(1) == 0 {
        let trunc = r.read_packed_bits(int_bits) - (1 << (int_bits - 1));
        (trunc as f32).to_bits() as i32
    } else {
        // Raw read at `readcount` (0x807ca80).
        r.read_long()
    }
}

fn write_float_field(w: &mut MsgWriter, raw: i32, int_bits: i32) {
    let f = f32::from_bits(raw as u32);
    if f == 0.0 {
        w.write_bits(0, 1);
        return;
    }
    w.write_bits(1, 1);
    let trunc = f as i32;
    let biased = trunc.wrapping_add(1 << (int_bits - 1));
    if trunc as f32 == f && (0..(1 << int_bits)).contains(&biased) {
        w.write_bits(0, 1);
        w.write_packed_bits(biased, int_bits);
    } else {
        w.write_bits(1, 1);
        w.write_long(raw);
    }
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

    /// A reader over a message already decompressed.
    pub fn from_plain(data: Vec<u8>) -> Self {
        MsgReader {
            data,
            bit: 0,
            readcount: 0,
            overflowed: false,
        }
    }

    /// `msg_t.readcount`: whole bytes taken so far, bit bytes included.
    pub fn readcount(&self) -> usize {
        self.readcount
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

    /// The dedicated server's `MSG_ReadString` (cod_lnxded 0x807f320), what
    /// a client command is read with: the client's byte map less the `%`
    /// one (0x8083394). The netchan keys server-to-client traffic on this
    /// copy, so a `%` mapped here desyncs a retail client that typed one.
    pub fn read_server_string(&mut self) -> String {
        let mut s = String::new();
        while s.len() < MAX_STRING_CHARS - 1 {
            let c = self.read_byte();
            if c == 0 || self.overflowed {
                break;
            }
            s.push(match c {
                0x92 => '\'',
                c if c > 127 => '.',
                c => c as char,
            });
        }
        s
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

    /// CoD2's `MSG_ReadString` (Mac 0x1737fe) as bytes: read to the NUL, the
    /// first 1023 kept, each through `I_CleanChar` (0x92 reads `'`); every
    /// other high byte survives, unlike CoD1's [`Self::read_string`].
    pub fn read_string_bytes(&mut self) -> Vec<u8> {
        self.read_clean_bytes(MAX_STRING_CHARS, false)
    }

    /// CoD2's `MSG_ReadBigString` (Mac 0x173870): 8191 bytes kept, `%` read
    /// as `.` as well.
    pub fn read_big_string_bytes(&mut self) -> Vec<u8> {
        self.read_clean_bytes(BIG_INFO_STRING, true)
    }

    fn read_clean_bytes(&mut self, cap: usize, big: bool) -> Vec<u8> {
        let mut out = Vec::new();
        loop {
            let mut c = self.read_byte();
            if big && c == b'%' {
                c = b'.';
            }
            if c == 0 || self.overflowed {
                return out;
            }
            if out.len() < cap - 1 {
                out.push(if c == 0x92 { b'\'' } else { c });
            }
        }
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

    /// CoD2's `MSG_WriteString`: the bytes as they are, then a NUL.
    pub fn write_string_bytes(&mut self, s: &[u8]) {
        self.out.extend_from_slice(s);
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

    const fn field(name: &'static str, bits: i32) -> NetField {
        NetField {
            name,
            offset: 0,
            bits,
        }
    }

    /// A table with every code a game's table can carry.
    const TABLE: [NetField; 6] = [
        field("float", 0),
        field("small", FLOAT10_BITS),
        field("angle", ANGLE_BITS),
        field("signed", -16),
        field("int", 7),
        field("odd", -3),
    ];

    fn read<T>(
        h: &Huffman,
        f: impl FnOnce(&mut MsgWriter),
        g: impl FnOnce(&mut MsgReader) -> T,
    ) -> T {
        let mut w = MsgWriter::new(h);
        f(&mut w);
        let data = w.finish();
        let mut r = MsgReader::new(&data, h);
        let v = g(&mut r);
        assert!(!r.is_overflowed());
        v
    }

    fn fbits(f: f32) -> i32 {
        f.to_bits() as i32
    }

    /// cod2-protocol.md 5.2: -99 is a zero flag, a selector, then 2 loose
    /// bits and a byte biased by 0x200, or a raw float.
    #[test]
    fn float10_reads_its_integral_and_raw_forms() {
        let h = Huffman::new();
        let got = read(
            &h,
            |w| {
                w.write_bits(1, 1); // changed
                w.write_bits(1, 1); // not zero
                w.write_bits(0, 1); // integral
                w.write_bits((-7 + 0x200) & 3, 2);
                w.write_byte(((-7 + 0x200) >> 2) as u8);
            },
            |r| FieldCodec::COD2.read_delta_field(r, 0, FLOAT10_BITS),
        );
        assert_eq!(f32::from_bits(got as u32), -7.0);
        let got = read(
            &h,
            |w| {
                w.write_bits(0b111, 3); // changed, not zero, raw
                w.write_long(fbits(600.5));
            },
            |r| FieldCodec::COD2.read_delta_field(r, 0, FLOAT10_BITS),
        );
        assert_eq!(f32::from_bits(got as u32), 600.5);
        // 511 still fits the 10 bits, 512 goes raw: 3 bits and 2 bytes
        // against 3 bits and 4 bytes.
        for (v, bytes) in [(511.0f32, 2), (512.0, 5), (-512.0, 2), (-513.0, 5)] {
            let mut w = MsgWriter::new(&h);
            write_delta_field(&mut w, 0, fbits(v), FLOAT10_BITS);
            assert_eq!(w.into_ops().len(), bytes, "{v}");
        }
    }

    /// A negative width sign-extends on CoD2 and reads unsigned on CoD1.
    #[test]
    fn negative_widths_sign_extend_only_on_cod2() {
        let h = Huffman::new();
        let wire = |w: &mut MsgWriter| write_delta_field(w, 0, -5, -16);
        assert_eq!(
            read(&h, wire, |r| FieldCodec::COD2.read_delta_field(r, 0, -16)),
            -5
        );
        assert_eq!(
            read(&h, wire, |r| FieldCodec::COD1.read_delta_field(r, 0, -16)),
            0xfffb
        );
        // A positive width never sign-extends; under a negative one the top
        // bit of the width is the sign.
        let wire = |w: &mut MsgWriter| write_delta_field(w, 0, 0x7f, 7);
        assert_eq!(
            read(&h, wire, |r| FieldCodec::COD2.read_delta_field(r, 0, 7)),
            0x7f
        );
        let wire = |w: &mut MsgWriter| write_delta_field(w, 0, 0b101, -3);
        assert_eq!(
            read(&h, wire, |r| FieldCodec::COD2.read_delta_field(r, 0, -3)),
            -3
        );
        // The playerState path reads integers the same way.
        let wire = |w: &mut MsgWriter| write_ps_value(w, -300, -16);
        assert_eq!(
            read(&h, wire, |r| FieldCodec::COD2.read_ps_value(r, 0, -16)),
            -300
        );
    }

    /// The angle code is a zero flag and a short off the byte cursor.
    #[test]
    fn angle_reads_a_short() {
        let h = Huffman::new();
        let got = read(
            &h,
            |w| {
                w.write_bits(0b11, 2);
                w.write_short(-1821);
            },
            |r| FieldCodec::COD2.read_delta_field(r, 0, ANGLE_BITS),
        );
        assert_eq!(f32::from_bits(got as u32), -1821.0 * 360.0 / 65536.0);
    }

    /// A struct of every code round-trips through the writer, and a value
    /// read back equal to its base (a difference above the width) re-encodes
    /// as the change it was, signed or not.
    #[test]
    fn struct_deltas_round_trip_on_both_codecs() {
        let h = Huffman::new();
        let from = [fbits(1.0), fbits(2.0), fbits(90.0), -2, 5, 1];
        let to = [fbits(-0.25), fbits(-400.0), fbits(-45.0), -32768, 0, -4];
        for codec in [FieldCodec::COD1, FieldCodec::COD2] {
            let mut w = MsgWriter::new(&h);
            write_delta_struct(&mut w, &TABLE, &from, Some(&to));
            let ops = w.into_ops();
            let got = read(
                &h,
                |w| ops.iter().for_each(|&b| w.write_byte(b)),
                |r| codec.read_delta_struct(r, &TABLE, &from).unwrap(),
            );
            let mut again = MsgWriter::new(&h);
            write_delta_struct(&mut again, &TABLE, &from, Some(&got));
            assert_eq!(again.into_ops(), ops, "{codec:?}");
            let want_signed = |v: i32, bits: i32| {
                if codec.signed_widths {
                    v
                } else {
                    v & ((1 << bits) - 1)
                }
            };
            assert_eq!(got[3], want_signed(-32768, 16), "{codec:?}");
            assert_eq!(got[5], want_signed(-4, 3), "{codec:?}");
            assert_eq!(f32::from_bits(got[1] as u32), -400.0);
        }
        // The same bits as the base, sent as changed: -2 under 16 bits from a
        // base of -2 with bits set above the width.
        let base = [0, 0, 0, -2 ^ (1 << 20), 0, 0];
        let mut cur = base;
        cur[3] = -2;
        for codec in [FieldCodec::COD1, FieldCodec::COD2] {
            let mut w = MsgWriter::new(&h);
            write_delta_struct(&mut w, &TABLE, &base, Some(&cur));
            let ops = w.into_ops();
            let got = read(
                &h,
                |w| ops.iter().for_each(|&b| w.write_byte(b)),
                |r| codec.read_delta_struct(r, &TABLE, &base).unwrap(),
            );
            assert_ne!(got[3], base[3], "{codec:?}");
            let mut again = MsgWriter::new(&h);
            write_delta_struct(&mut again, &TABLE, &base, Some(&got));
            assert_eq!(again.into_ops(), ops, "{codec:?}");
        }
        // A remove and an unchanged struct.
        let none = read(
            &h,
            |w| write_delta_struct(w, &TABLE, &from, None),
            |r| FieldCodec::COD2.read_delta_struct(r, &TABLE, &from),
        );
        assert!(none.is_none());
        let same = read(
            &h,
            |w| write_delta_struct(w, &TABLE, &from, Some(&from)),
            |r| FieldCodec::COD2.read_delta_struct(r, &TABLE, &from),
        );
        assert_eq!(same.as_deref(), Some(&from[..]));
    }
}
