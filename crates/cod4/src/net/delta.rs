//! CoD4 1.7's delta codec: the per-field reader (`MSG_ReadDeltaField`,
//! iw3mp.exe 0x5067c0), its writer, and the struct readers and writers
//! built on them, over the tables in [`super::fields`]
//! (docs/research/cod4-netfields.md).
//!
//! Every struct is a word array indexed by `offset / 4`, as the C reads it:
//! a field is one 32-bit word, an int or a float's bits.

use super::fields;
use anyhow::{Result, bail, ensure};
use vcod_common::net::codec::{MsgReader, MsgWriter};

/// One netfield row: `{name, offset, bits, changeHints}`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Field {
    pub name: &'static str,
    pub offset: u16,
    pub bits: i32,
    /// 2: no change bit, always sent. 3: a predicted field (origin,
    /// velocity) the playerstate may send without its base.
    pub hint: u8,
}

impl Field {
    pub const fn new(name: &'static str, offset: i32, bits: i32, hint: u32) -> Field {
        Field {
            name,
            offset: offset as u16,
            bits,
            hint: hint as u8,
        }
    }

    pub fn word(&self) -> usize {
        self.offset as usize / 4
    }
}

/// What a field read needs besides the bits: the snapshot's time (the
/// `-97` delta times) and `CS_MAPCENTER` (the `-92..-90` origins).
#[derive(Clone, Copy, Debug, Default)]
pub struct Ctx {
    pub time: i32,
    pub map_center: [f32; 3],
}

/// `ENTITYNUM_NONE`: ends the snapshot's entity list.
pub const ENTITYNUM_NONE: u32 = 1023;
/// `ENTITYNUM_WORLD`: the -96 code's one-bit ground.
const ENTITYNUM_WORLD: u32 = 1022;
/// -0.0's bits.
const NEG_ZERO: u32 = 0x8000_0000;
/// Bits of a full entity number.
pub const GENTITYNUM_BITS: u32 = 10;
/// Bits of a client number in the snapshot's client list.
pub const CLIENTNUM_BITS: u32 = 6;
/// `entityState_t` is 0xf4 bytes: number, eType, then the fields.
pub const ENTITY_WORDS: usize = 61;
/// `MSG_ReadDeltaEntityStruct` sizes its last-changed index for 61 fields
/// (0x506e8e) whatever the eType's table holds.
const ENTITY_LC_FIELDS: u32 = 61;
pub const CLIENT_WORDS: usize = 25;
pub const HUDELEM_WORDS: usize = 40;
pub const OBJECTIVE_WORDS: usize = 7;
/// Words up to the last netfield (`killCamEntity`, 2208).
pub const PLAYER_WORDS: usize = 553;
pub const MAX_HUDELEMS: usize = 31;
pub const MAX_OBJECTIVES: usize = 16;

/// The minimum bit count: bits to hold `n`.
pub fn min_bits(n: u32) -> u32 {
    32 - n.leading_zeros()
}

/// `n` bits, the reader's `MSG_ReadBits`; 0 bits reads nothing.
fn bits(r: &mut MsgReader, n: u32) -> u32 {
    if n == 0 {
        0
    } else {
        r.read_bits(n as i32) as u32
    }
}

fn bit(r: &mut MsgReader) -> bool {
    r.read_bits(1) != 0
}

/// What a field's delta did: the change bit clear, set, or set with a
/// HUD colour's alpha toggled (the `-85` code's short form, which the
/// server picks off its own unquantized alpha).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Change {
    Kept,
    Sent,
    Toggled,
}

impl Change {
    /// The server's choice between two states it holds.
    pub fn natural(f: &Field, from: u32, to: u32) -> Change {
        if values_equal(f.bits, from, to) {
            Change::Kept
        } else if f.bits == -85 && alpha_toggles(from, to) {
            Change::Toggled
        } else {
            Change::Sent
        }
    }
}

/// `MSG_WriteDeltaField`'s `-85` test: the same RGB and the alpha going
/// from 0xff to 0 or back.
fn alpha_toggles(from: u32, to: u32) -> bool {
    let (a, b) = (from.to_le_bytes(), to.to_le_bytes());
    a[..3] == b[..3] && ((a[3] == 0xff && b[3] == 0) || (a[3] == 0 && b[3] == 0xff))
}

/// `MSG_ReadDeltaField`: the field's word given its base `from` (0 for a
/// `noXor` read, as the C points `from` at a zero there), the base itself
/// when the change bit was clear.
pub fn read_field(r: &mut MsgReader, ctx: &Ctx, from: u32, f: &Field) -> (u32, Change) {
    if f.hint != 2 && !bit(r) {
        return (from, Change::Kept);
    }
    if f.bits == -85 && bit(r) {
        let mut c = from.to_le_bytes();
        c[3] = if c[3] != 0 { 0 } else { 0xff };
        return (u32::from_le_bytes(c), Change::Toggled);
    }
    (read_value(r, ctx, from, f), Change::Sent)
}

/// An origin field's whole value: 1.7's `MSG_WriteOriginFloat` (0x813decc)
/// and the equality test before it (0x813e6b1) both take `floor(f + 0.5f)`
/// on the stack (`frndint` rounding down), so a half goes up: the map's
/// script model at x 1912.5 goes out as 1913, one at -521.5 as -521.
fn snap(word: u32) -> i32 {
    (f64::from(f32::from_bits(word)) + 0.5).floor() as i32
}

/// The 16-bit angle `MSG_ValuesAreEqual` compares (`* 182.0444488525391`).
fn angle_units(word: u32) -> u16 {
    (f32::from_bits(word) as f64 * 182.0444488525391) as i32 as u16
}

/// `MSG_ValuesAreEqual`: the words, or what an angle, a 100 ms timer or an
/// origin quantizes to.
pub fn values_equal(bits: i32, from: u32, to: u32) -> bool {
    if from == to {
        return true;
    }
    match bits {
        -100 | -87 => angle_units(from) == angle_units(to),
        -95 => from as i32 / 100 == to as i32 / 100,
        -92..=-90 => snap(from) == snap(to),
        _ => false,
    }
}

fn put_bit(w: &mut MsgWriter, b: bool) {
    w.write_bits(b as i32, 1);
}

fn put_bits(w: &mut MsgWriter, v: u32, n: u32) {
    w.write_bits(v as i32, n as i32);
}

/// What a server's -100 field sends for the game's angle `word`, as the
/// client decodes it: 1.7 writes `(int)` of the stored float `f *
/// 182.04445f + 0.5f` (`cod4_lnxded` 0x81311aa, the field writer's call
/// at 0x813e8fd), so a negative angle goes out a short nearer zero than
/// `(int)(f * 182.04445f)` whenever its fraction of a short is under a
/// half. The decoded value re-encodes as itself through [`write_delta_entity`]'s
/// plain truncation, which is what a capture's bytes need.
pub fn angle16_sent(word: u32) -> u32 {
    let v = (f64::from(f32::from_bits(word)) * f64::from(182.04445f32) + 0.5) as f32;
    short_angle(v as i32 as i16)
}

/// The value behind a set change bit (for `-85`, past its toggle bit).
fn read_value(r: &mut MsgReader, ctx: &Ctx, from: u32, f: &Field) -> u32 {
    Code::of(f.bits).read(r, ctx, from)
}

/// `MSG_WriteDeltaField` past the change bit: the form follows from the
/// two words alone. Each [`Code`] arm mirrors its reader.
fn write_value(w: &mut MsgWriter, ctx: &Ctx, from: u32, to: u32, f: &Field) {
    Code::of(f.bits).write(w, ctx, from, to);
}

/// A netfield's `bits` as the codec reads it (section 2's table). The
/// field writer's dispatch is `cod4_lnxded` 0x813e287..0x813e339; the
/// reader's `iw3mp.exe` 0x5068a5..0x506ab4.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Code {
    /// 0, -89, -99.
    Float(IntFloat),
    /// -88: the word XOR its base.
    Xor,
    /// -98: 24-bit flags.
    Flags,
    /// -97: a server time.
    Time,
    /// -96: `groundEntityNum`.
    Ground,
    /// -95: a timer in 100 ms steps.
    Tenths,
    /// -94, -93: an event or its parm.
    Byte,
    /// -92, -91, -90: an origin axis against `CS_MAPCENTER`'s.
    Origin(usize),
    /// -100 (`zero_bit`) and -87: an angle in 65536ths of a turn.
    Angle { zero_bit: bool },
    /// -86: 1.4 plus tenths, 5 bits.
    Scale,
    /// -85: a HUD colour's long form.
    Colour,
    /// Any other width: an int, sign-extended when negative.
    Int(i32),
}

impl Code {
    fn of(bits: i32) -> Code {
        match bits {
            0 => Code::Float(IntFloat::FLOAT),
            -89 => Code::Float(IntFloat::INT_FLOAT),
            -99 => Code::Float(IntFloat::SMALL_FLOAT),
            -88 => Code::Xor,
            -98 => Code::Flags,
            -97 => Code::Time,
            -96 => Code::Ground,
            -95 => Code::Tenths,
            -94 | -93 => Code::Byte,
            -92..=-90 => Code::Origin((bits + 92) as usize),
            -100 => Code::Angle { zero_bit: true },
            -87 => Code::Angle { zero_bit: false },
            -86 => Code::Scale,
            -85 => Code::Colour,
            n => Code::Int(n),
        }
    }

    fn read(self, r: &mut MsgReader, ctx: &Ctx, from: u32) -> u32 {
        match self {
            Code::Float(form) => form.read(r, from),
            Code::Xor => r.read_long() as u32 ^ from,
            Code::Flags => {
                if bit(r) {
                    u32::from_le_bytes([r.read_byte(), r.read_byte(), r.read_byte(), 0])
                } else {
                    from ^ (1 << bits(r, 5))
                }
            }
            Code::Time => {
                if bit(r) {
                    r.read_long() as u32
                } else {
                    ctx.time.wrapping_sub(bits(r, 8) as i32) as u32
                }
            }
            Code::Ground => {
                if bit(r) {
                    ENTITYNUM_WORLD
                } else if bit(r) {
                    0
                } else {
                    bits(r, 2) | u32::from(r.read_byte()) << 2
                }
            }
            Code::Tenths => bits(r, 7) * 100,
            Code::Byte => u32::from(r.read_byte()),
            Code::Origin(axis) => Origin::on(ctx, axis).read(r, from),
            Code::Angle { zero_bit } => {
                if zero_bit && !bit(r) {
                    return 0;
                }
                match short_angle(r.read_short()) {
                    // Under a set zero bit a short of 0 (an angle under one
                    // unit, or -0) stands for the smallest float above
                    // zero, which the writer sends the same way.
                    0 if zero_bit => 1,
                    v => v,
                }
            }
            Code::Scale => {
                ((f64::from(bits(r, 5)) / 10.0 + f64::from(SCALE_BASE)) as f32).to_bits()
            }
            Code::Colour => {
                let mut rgba = from.to_le_bytes();
                if !bit(r) {
                    for c in &mut rgba[..3] {
                        *c = r.read_byte();
                    }
                }
                rgba[3] = (bits(r, 5) << 3) as u8;
                u32::from_le_bytes(rgba)
            }
            Code::Int(width) => {
                if !bit(r) {
                    return 0;
                }
                let mask = width_mask(width);
                let v = (r.read_packed_bits(width.abs()) as u32 ^ from) & mask;
                let sign = (mask >> 1) + 1;
                if width < 0 && v & sign != 0 {
                    v | !mask
                } else {
                    v
                }
            }
        }
    }

    fn write(self, w: &mut MsgWriter, ctx: &Ctx, from: u32, to: u32) {
        match self {
            Code::Float(form) => form.write(w, from, to),
            Code::Xor => w.write_long((from ^ to) as i32),
            Code::Flags => {
                let flipped = from ^ to;
                // One flipped bit names itself (0x813e98b); 0 or several
                // send the whole 24 bits.
                if flipped.is_power_of_two() {
                    put_bit(w, false);
                    put_bits(w, flipped.trailing_zeros(), 5);
                } else {
                    put_bit(w, true);
                    for b in &to.to_le_bytes()[..3] {
                        w.write_byte(*b);
                    }
                }
            }
            Code::Time => {
                // Up to 255 ms before the snapshot (0x813e789).
                let ago = ctx.time.wrapping_sub(to as i32);
                if (0..256).contains(&ago) {
                    put_bit(w, false);
                    put_bits(w, ago as u32, 8);
                } else {
                    put_bit(w, true);
                    w.write_long(to as i32);
                }
            }
            Code::Ground => match to {
                ENTITYNUM_WORLD => put_bit(w, true),
                0 => put_bits(w, 0b10, 2),
                // Two clear bits, then the number's low 2 bits and a byte.
                n => {
                    put_bits(w, (n & 3) << 2, 4);
                    w.write_byte((n >> 2) as u8);
                }
            },
            Code::Tenths => put_bits(w, (to as i32 / 100) as u32, 7),
            Code::Byte => w.write_byte(to as u8),
            Code::Origin(axis) => Origin::on(ctx, axis).write(w, from, to),
            Code::Angle { zero_bit } => {
                if zero_bit {
                    put_bit(w, to != 0);
                    if to == 0 {
                        return;
                    }
                }
                w.write_short(angle_short(to));
            }
            Code::Scale => {
                // floor((f - 1.4f) * 10 + 0.5) on the x87 stack (0x813e923).
                let tenths = (f64::from(f32::from_bits(to)) - f64::from(SCALE_BASE)) * 10.0;
                put_bits(w, (tenths + 0.5).floor() as i32 as u32, 5);
            }
            Code::Colour => {
                let (old, new) = (from.to_le_bytes(), to.to_le_bytes());
                // The toggle bit `write_field` leaves to this arm is clear.
                put_bit(w, false);
                let same_rgb = old[..3] == new[..3];
                put_bit(w, same_rgb);
                if !same_rgb {
                    for c in &new[..3] {
                        w.write_byte(*c);
                    }
                }
                put_bits(w, u32::from(new[3] >> 3), 5);
            }
            Code::Int(width) => {
                put_bit(w, to != 0);
                if to != 0 {
                    w.write_packed_bits((from ^ to) as i32, width.abs());
                }
            }
        }
    }
}

/// `cvttss2si` (the server's) and `_ftol2` (the client's, `iw3mp.exe`
/// 0x68b360): toward zero, and 0x80000000 for NaN or a float past an
/// int's range.
fn cvtt(f: f32) -> i32 {
    if (-2_147_483_648.0..2_147_483_648.0).contains(&f) {
        f as i32
    } else {
        i32::MIN
    }
}

/// -86's offset, the f32 `1.4` both sides read (`cod4_lnxded` 0x82154b8).
const SCALE_BASE: f32 = 1.4;

/// The bits an `n`-wide int field keeps.
fn width_mask(n: i32) -> u32 {
    u32::MAX >> (32 - n.unsigned_abs())
}

/// `MSG_WriteAngle16`'s short: `(int)(f * 182.04445f)`, the product in f32.
fn angle_short(word: u32) -> i16 {
    cvtt(f32::from_bits(word) * 182.04445) as i16
}

/// `MSG_ReadAngle16`: a short of 65536ths of a turn back in degrees.
fn short_angle(units: i16) -> u32 {
    ((f64::from(units) * (360.0 / 65536.0)) as f32).to_bits()
}

/// How the float codes treat a zero.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Zero {
    /// -89: no zero form.
    None,
    /// 0: a clear bit, then the sign.
    Signed,
    /// -99: a clear bit for +0; -0 goes out as a long.
    Positive,
}

/// Codes 0, -89 and -99. A float holding a small whole number goes out
/// as `low_bits` bits and a byte of its int XOR the base's, both biased
/// by `1 << (low_bits + 7)`; anything else as a long XOR the base. The
/// server's tests: 0x813e3ab (code 0, which also wants the base in
/// range), 0x813e611 (-89), 0x813e555 (-99).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct IntFloat {
    low_bits: u32,
    zero: Zero,
    base_in_range: bool,
}

impl IntFloat {
    const FLOAT: IntFloat = IntFloat {
        low_bits: 5,
        zero: Zero::Signed,
        base_in_range: true,
    };
    const INT_FLOAT: IntFloat = IntFloat {
        low_bits: 5,
        zero: Zero::None,
        base_in_range: false,
    };
    const SMALL_FLOAT: IntFloat = IntFloat {
        low_bits: 4,
        zero: Zero::Positive,
        base_in_range: false,
    };

    fn bias(self) -> i32 {
        1 << (self.low_bits + 7)
    }

    fn fits(self, n: i32) -> bool {
        (-self.bias()..self.bias()).contains(&n)
    }

    /// `to`'s short form against `from`, when it has one.
    fn packed(self, from: u32, to: u32) -> Option<u32> {
        let new = f32::from_bits(to);
        let n = cvtt(new);
        let whole = to != NEG_ZERO && f64::from(n) == f64::from(new);
        let old = cvtt(f32::from_bits(from));
        let ok = whole && self.fits(n) && (!self.base_in_range || self.fits(old));
        ok.then(|| (n.wrapping_add(self.bias()) ^ old.wrapping_add(self.bias())) as u32)
    }

    fn write(self, w: &mut MsgWriter, from: u32, to: u32) {
        let zero = match self.zero {
            Zero::None => false,
            Zero::Signed => f32::from_bits(to) == 0.0,
            Zero::Positive => to == 0,
        };
        if self.zero != Zero::None {
            put_bit(w, !zero);
        }
        if zero {
            if self.zero == Zero::Signed {
                put_bit(w, to == NEG_ZERO);
            }
            return;
        }
        match self.packed(from, to) {
            Some(v) => {
                put_bit(w, false);
                put_bits(w, v, self.low_bits);
                w.write_byte((v >> self.low_bits) as u8);
            }
            None => {
                put_bit(w, true);
                w.write_long((from ^ to) as i32);
            }
        }
    }

    fn read(self, r: &mut MsgReader, from: u32) -> u32 {
        if self.zero != Zero::None && !bit(r) {
            return match self.zero {
                Zero::Signed => u32::from(bit(r)) << 31,
                _ => 0,
            };
        }
        if bit(r) {
            return r.read_long() as u32 ^ from;
        }
        let low = bits(r, self.low_bits);
        let v = (u32::from(r.read_byte()) << self.low_bits | low) as i32;
        let n =
            (v ^ cvtt(f32::from_bits(from)).wrapping_add(self.bias())).wrapping_sub(self.bias());
        (n as f32).to_bits()
    }
}

/// The origin codes on one axis, in whole units: a 7-bit step off the
/// base, or 16 bits XOR the base, both relative to the map centre's axis
/// rounded (`(int)(c + 0.5f)`, 0x813df9a) and biased by 0x8000. The
/// server rounds both words ([`snap`]); the reader adds the step to the
/// base float and truncates it for the long form.
struct Origin {
    centre: i32,
}

impl Origin {
    fn on(ctx: &Ctx, axis: usize) -> Origin {
        Origin {
            centre: cvtt(ctx.map_center[axis] + 0.5),
        }
    }

    fn biased(&self, units: i32) -> i32 {
        units.wrapping_sub(self.centre).wrapping_add(0x8000)
    }

    fn write(&self, w: &mut MsgWriter, from: u32, to: u32) {
        let (old, new) = (snap(from), snap(to));
        let step = new.wrapping_sub(old);
        let near = (-64..64).contains(&step);
        put_bit(w, !near);
        if near {
            put_bits(w, (step + 64) as u32, 7);
        } else {
            put_bits(w, (self.biased(old) ^ self.biased(new)) as u32, 16);
        }
    }

    fn read(&self, r: &mut MsgReader, from: u32) -> u32 {
        let base = f32::from_bits(from);
        let units = if bit(r) {
            let v = bits(r, 16) as i32 ^ self.biased(cvtt(base));
            f64::from(v - 0x8000 + self.centre)
        } else {
            f64::from(base) + f64::from(bits(r, 7) as i32 - 64)
        };
        (units as f32).to_bits()
    }
}

/// The writer's half of [`read_field`]: the change bit (unless the row has
/// none), then the value as `change` says.
pub fn write_field(w: &mut MsgWriter, ctx: &Ctx, from: u32, to: u32, f: &Field, change: Change) {
    if f.hint != 2 {
        put_bit(w, change != Change::Kept);
        if change == Change::Kept {
            return;
        }
    }
    if change == Change::Toggled {
        put_bit(w, true);
        return;
    }
    write_value(w, ctx, from, to, f);
}

/// The change bits one struct delta wrote, by table row, up to its last
/// changed field (the wire's count is the length). A writer cannot always
/// rebuild them from the two states the reader holds: the server compares
/// its own unquantized state (a HUD alpha that differs below 5 bits still
/// goes out), and a playerstate sends some rows by its own rule. A row with
/// no change bit (`hint` 2) reads [`Change::Sent`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Sent(pub Vec<Change>);

impl Sent {
    /// What the server would send between two states it holds: each row
    /// that differs (`MSG_ValuesAreEqual`), up to the last that does.
    pub fn natural(table: &[Field], from: &[u32], to: &[u32]) -> Sent {
        let mut v: Vec<Change> = table
            .iter()
            .map(|f| Change::natural(f, from[f.word()], to[f.word()]))
            .collect();
        let lc = v
            .iter()
            .rposition(|&c| c != Change::Kept)
            .map_or(0, |i| i + 1);
        v.truncate(lc);
        Sent(v)
    }

    pub fn lc(&self) -> usize {
        self.0.len()
    }
}

/// How one entity or client went out in a snapshot's list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Wire {
    Removed,
    /// Sent with nothing changed: a new entity equal to its baseline.
    Unchanged,
    Changed(Sent),
}

impl Wire {
    /// The server's choice for one slot (`MSG_WriteEntityDelta`): `None`
    /// when an unforced unchanged slot goes unmentioned.
    pub fn natural(table: &[Field], from: &[u32], to: Option<&[u32]>, force: bool) -> Option<Wire> {
        let Some(to) = to else {
            return Some(Wire::Removed);
        };
        let s = Sent::natural(table, from, to);
        if s.lc() > 0 {
            Some(Wire::Changed(s))
        } else {
            force.then_some(Wire::Unchanged)
        }
    }
}

/// `entityState_t` as words: 0 the number, 1 the eType.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EntityState(pub [u32; ENTITY_WORDS]);

impl Default for EntityState {
    fn default() -> Self {
        EntityState([0; ENTITY_WORDS])
    }
}

impl EntityState {
    pub fn number(&self) -> u32 {
        self.0[0]
    }

    pub fn etype(&self) -> u32 {
        self.0[1]
    }

    /// `lerp.pos.trBase`.
    pub fn origin(&self) -> [f32; 3] {
        [6, 7, 8].map(|w| f32::from_bits(self.0[w]))
    }

    /// The table `etype` reads with (eType 17 and up: the event table).
    pub fn table(etype: u32) -> &'static [Field] {
        fields::ENTITY_BY_TYPE[etype.min(17) as usize]
    }

    /// Every angle field as a server sends it ([`angle16_sent`]).
    pub fn sent(mut self) -> Self {
        for f in Self::table(self.etype()) {
            if matches!(f.bits, -100 | -87) {
                self.0[f.word()] = angle16_sent(self.0[f.word()]);
            }
        }
        self
    }
}

/// `clientState_t` as words: 0 the client index.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClientState(pub [u32; CLIENT_WORDS]);

impl Default for ClientState {
    fn default() -> Self {
        ClientState([0; CLIENT_WORDS])
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HudElem(pub [u32; HUDELEM_WORDS]);

impl Default for HudElem {
    fn default() -> Self {
        HudElem([0; HUDELEM_WORDS])
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Objective {
    pub state: u32,
    pub words: [u32; OBJECTIVE_WORDS],
}

/// `playerState_t`: the netfield words plus the blocks sent outside the
/// table.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlayerState {
    pub words: Vec<u32>,
    pub stats: [i32; 5],
    pub ammo: [i32; 64],
    pub ammoclip: [i32; 128],
    pub objectives: [Objective; MAX_OBJECTIVES],
    pub hud_archival: [HudElem; MAX_HUDELEMS],
    pub hud_current: [HudElem; MAX_HUDELEMS],
    pub weapon_models: [u8; 128],
    /// The first bit: origin and velocity came on the wire. A message
    /// without them leaves the client's own prediction in their place,
    /// which a capture reader does not have (the fields keep the base).
    pub origin_sent: bool,
}

impl Default for PlayerState {
    fn default() -> Self {
        PlayerState {
            words: vec![0; PLAYER_WORDS],
            stats: [0; 5],
            ammo: [0; 64],
            ammoclip: [0; 128],
            objectives: [Objective::default(); MAX_OBJECTIVES],
            hud_archival: [HudElem::default(); MAX_HUDELEMS],
            hud_current: [HudElem::default(); MAX_HUDELEMS],
            weapon_models: [0; 128],
            origin_sent: true,
        }
    }
}

impl PlayerState {
    /// A field's word by its table name.
    pub fn get(&self, name: &str) -> Option<u32> {
        fields::PLAYER_STATE
            .iter()
            .find(|f| f.name == name)
            .map(|f| self.words[f.word()])
    }

    pub fn get_f32(&self, name: &str) -> Option<f32> {
        self.get(name).map(f32::from_bits)
    }

    /// Every angle field as a server sends it ([`angle16_sent`]).
    pub fn sent_angles(&mut self) {
        for f in fields::PLAYER_STATE {
            if matches!(f.bits, -100 | -87) {
                self.words[f.word()] = angle16_sent(self.words[f.word()]);
            }
        }
    }
}

/// `MSG_ReadEntityIndex`: +1 in a bit, else (for 10-bit numbers) a 4-bit
/// step or the whole number. `last` is `msg->lastEntityRef`, -1 at a
/// snapshot's or gamestate's start.
pub fn read_entity_index(r: &mut MsgReader, last: &mut i32, index_bits: u32) -> Result<u32> {
    if bit(r) {
        *last += 1;
    } else if index_bits != GENTITYNUM_BITS || bit(r) {
        *last = bits(r, index_bits) as i32;
    } else {
        *last += bits(r, 4) as i32;
    }
    ensure!(*last >= 0, "negative entity number");
    Ok(*last as u32)
}

/// `MSG_WriteEntityIndex`: +1 in a bit, a step under 16 for 10-bit
/// numbers, else the whole number.
pub fn write_entity_index(w: &mut MsgWriter, last: &mut i32, index: u32, index_bits: u32) {
    let step = index as i32 - *last;
    if step == 1 {
        put_bit(w, true);
    } else {
        put_bit(w, false);
        if index_bits == GENTITYNUM_BITS && step < 16 {
            put_bit(w, false);
            put_bits(w, step as u32, 4);
        } else {
            if index_bits == GENTITYNUM_BITS {
                put_bit(w, true);
            }
            put_bits(w, index, index_bits);
        }
    }
    *last = index as i32;
}

/// The remove and change bits and the last-changed count of an entity or
/// client delta; `None` when there are no fields to read.
fn read_wire_head(r: &mut MsgReader, lc_bits: u32) -> std::result::Result<usize, Wire> {
    if bit(r) {
        return Err(Wire::Removed);
    }
    if !bit(r) {
        return Err(Wire::Unchanged);
    }
    Ok(bits(r, lc_bits) as usize)
}

/// `MSG_ReadDeltaEntity`: the state (`None` once removed) and how it was
/// sent.
pub fn read_delta_entity(
    r: &mut MsgReader,
    ctx: &Ctx,
    from: &EntityState,
    number: u32,
) -> Result<(Option<EntityState>, Wire)> {
    let lc = match read_wire_head(r, min_bits(ENTITY_LC_FIELDS)) {
        Err(Wire::Removed) => return Ok((None, Wire::Removed)),
        Err(wire) => return Ok((Some(*from), wire)),
        Ok(lc) => lc,
    };
    let mut to = *from;
    to.0[0] = number;
    let mut sent = Vec::with_capacity(lc);
    // eType always reads through the general table's first row.
    if lc > 0 {
        let ef = &fields::ENTITY_GENERAL[0];
        let (v, c) = read_field(r, ctx, from.0[ef.word()], ef);
        sent.push(c);
        to.0[ef.word()] = v;
    }
    let table = EntityState::table(to.etype());
    ensure!(
        lc <= table.len(),
        "entity {number}: last changed field {lc} past the eType {} table's {}",
        to.etype(),
        table.len()
    );
    for f in &table[1.min(lc)..lc] {
        let (v, c) = read_field(r, ctx, from.0[f.word()], f);
        sent.push(c);
        to.0[f.word()] = v;
    }
    Ok((Some(to), Wire::Changed(Sent(sent))))
}

/// The mirror of [`read_delta_entity`], after the entity number.
pub fn write_delta_entity(
    w: &mut MsgWriter,
    ctx: &Ctx,
    from: &EntityState,
    to: &EntityState,
    wire: &Wire,
) {
    let Wire::Changed(sent) = wire else {
        write_wire_head(w, wire, 0, 0);
        return;
    };
    write_wire_head(w, wire, sent.lc(), min_bits(ENTITY_LC_FIELDS));
    let table = EntityState::table(to.etype());
    for (i, &changed) in sent.0.iter().enumerate() {
        let f = if i == 0 {
            &fields::ENTITY_GENERAL[0]
        } else {
            &table[i]
        };
        write_field(w, ctx, from.0[f.word()], to.0[f.word()], f, changed);
    }
}

fn write_wire_head(w: &mut MsgWriter, wire: &Wire, lc: usize, lc_bits: u32) {
    match wire {
        Wire::Removed => put_bit(w, true),
        Wire::Unchanged => {
            put_bit(w, false);
            put_bit(w, false);
        }
        Wire::Changed(_) => {
            put_bit(w, false);
            put_bit(w, true);
            put_bits(w, lc as u32, lc_bits);
        }
    }
}

/// `MSG_ReadDeltaClient`: the state (`None` once removed) and how it was
/// sent.
pub fn read_delta_client(
    r: &mut MsgReader,
    ctx: &Ctx,
    from: &ClientState,
    number: u32,
) -> Result<(Option<ClientState>, Wire)> {
    let table = &fields::CLIENT_STATE;
    let lc = match read_wire_head(r, min_bits(table.len() as u32)) {
        Err(Wire::Removed) => return Ok((None, Wire::Removed)),
        Err(wire) => return Ok((Some(*from), wire)),
        Ok(lc) => lc,
    };
    ensure!(
        lc <= table.len(),
        "client {number}: last changed field {lc}"
    );
    let mut to = *from;
    to.0[0] = number;
    let mut sent = Vec::with_capacity(lc);
    for f in &table[..lc] {
        let (v, c) = read_field(r, ctx, from.0[f.word()], f);
        sent.push(c);
        to.0[f.word()] = v;
    }
    Ok((Some(to), Wire::Changed(Sent(sent))))
}

/// The mirror of [`read_delta_client`], after the client number.
pub fn write_delta_client(
    w: &mut MsgWriter,
    ctx: &Ctx,
    from: &ClientState,
    to: &ClientState,
    wire: &Wire,
) {
    let table = &fields::CLIENT_STATE;
    let Wire::Changed(sent) = wire else {
        write_wire_head(w, wire, 0, 0);
        return;
    };
    write_wire_head(w, wire, sent.lc(), min_bits(table.len() as u32));
    for (f, &changed) in table.iter().zip(&sent.0) {
        write_field(w, ctx, from.0[f.word()], to.0[f.word()], f, changed);
    }
}

/// `MSG_ReadDeltaHudElems`: a 5-bit count in use, each with a 6-bit last
/// changed field; the slots past the count that were in use are cleared.
/// Returns each elem's change bits (fields 0 to its last changed one).
fn read_hudelems(
    r: &mut MsgReader,
    ctx: &Ctx,
    from: &[HudElem; MAX_HUDELEMS],
    to: &mut [HudElem; MAX_HUDELEMS],
) -> Result<Vec<Sent>> {
    let inuse = bits(r, 5) as usize;
    let mut out = Vec::with_capacity(inuse);
    for i in 0..inuse {
        let lc = bits(r, 6) as usize;
        ensure!(
            lc < fields::HUDELEM.len(),
            "hudelem {i}: last changed field {lc}"
        );
        let mut sent = Vec::with_capacity(lc + 1);
        for f in &fields::HUDELEM[..=lc] {
            let (v, c) = read_field(r, ctx, from[i].0[f.word()], f);
            sent.push(c);
            to[i].0[f.word()] = v;
        }
        out.push(Sent(sent));
    }
    // `type` is word 0.
    for h in to.iter_mut().skip(inuse) {
        if h.0[0] == 0 {
            break;
        }
        *h = HudElem::default();
    }
    Ok(out)
}

fn write_hudelems(
    w: &mut MsgWriter,
    ctx: &Ctx,
    from: &[HudElem; MAX_HUDELEMS],
    to: &[HudElem; MAX_HUDELEMS],
    sent: &[Sent],
) {
    put_bits(w, sent.len() as u32, 5);
    for (i, s) in sent.iter().enumerate() {
        put_bits(w, s.lc().saturating_sub(1) as u32, 6);
        for (f, &changed) in fields::HUDELEM.iter().zip(&s.0) {
            write_field(w, ctx, from[i].0[f.word()], to[i].0[f.word()], f, changed);
        }
    }
}

/// `MSG_WriteDeltaHudElems`' choice: the elems up to the first of type 0,
/// each with fields 0 to its last that differs.
fn natural_hudelems(from: &[HudElem; MAX_HUDELEMS], to: &[HudElem; MAX_HUDELEMS]) -> Vec<Sent> {
    let inuse = to.iter().take_while(|h| h.0[0] != 0).count();
    (0..inuse)
        .map(|i| {
            let changed: Vec<Change> = fields::HUDELEM
                .iter()
                .map(|f| Change::natural(f, from[i].0[f.word()], to[i].0[f.word()]))
                .collect();
            let lc = changed
                .iter()
                .rposition(|&c| c != Change::Kept)
                .unwrap_or(0);
            Sent(changed[..=lc].to_vec())
        })
        .collect()
}

/// The choices a playerstate delta made that its two states do not show.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PsWire {
    /// The netfield rows' change bits.
    pub fields: Sent,
    /// The stats block's mask, 0 when its bit was clear. The server
    /// compares its own ints: `stats[3]` goes out in 6 bits, and one whose
    /// high bits changed is sent with the low 6 the client already holds
    /// (cod4-netfields.md 10).
    pub stats: u8,
    /// `None`: the objectives block's bit was clear. Else per objective,
    /// `None` when its fields were not sent.
    pub objectives: Option<Vec<Option<Sent>>>,
    /// `None`: the HUD block's bit was clear. Else the archival and the
    /// current elems' change bits.
    pub hud: Option<[Vec<Sent>; 2]>,
}

/// `PM_INTERMISSION`, whose view angles always go out. INFERRED.
const PM_INTERMISSION: u32 = 5;

impl PsWire {
    /// `MSG_WriteDeltaPlayerstate`'s (0x813f586) choice of rows for a
    /// live snapshot (cod4-netfields.md 7; gates: `cod4::wire_ab`'s join
    /// and walk). A row goes out when it differs, except the `-87` view
    /// angles, sent only by [`view_goes_out`], and the predicted rows
    /// (hint 3), sent exactly when `to.origin_sent`. A hint 2 row below
    /// the last one that goes out is written whatever it holds.
    pub fn natural(from: &PlayerState, to: &PlayerState) -> PsWire {
        let view = view_goes_out(from, to);
        let table = &fields::PLAYER_STATE;
        let pair = |f: &Field| (from.words[f.word()], to.words[f.word()]);
        let wanted = |f: &Field| match (f.bits, f.hint) {
            (-87, _) => view,
            (_, 3) => to.origin_sent,
            _ => {
                let (a, b) = pair(f);
                !values_equal(f.bits, a, b)
            }
        };
        let count = table.iter().rposition(wanted).map_or(0, |last| last + 1);
        let rows = table[..count]
            .iter()
            .map(|f| match pair(f) {
                _ if !wanted(f) && f.hint != 2 => Change::Kept,
                (a, b) if f.bits == -85 && alpha_toggles(a, b) => Change::Toggled,
                _ => Change::Sent,
            })
            .collect();
        PsWire {
            fields: Sent(rows),
            stats: (0..5)
                .filter(|&i| from.stats[i] != to.stats[i])
                .fold(0, |mask, i| mask | 1 << i),
            objectives: (from.objectives != to.objectives).then(|| {
                std::iter::zip(&from.objectives, &to.objectives)
                    .map(|(a, b)| objective_rows(a, b))
                    .collect()
            }),
            hud: (from.hud_archival != to.hud_archival || from.hud_current != to.hud_current).then(
                || {
                    [
                        natural_hudelems(&from.hud_archival, &to.hud_archival),
                        natural_hudelems(&from.hud_current, &to.hud_current),
                    ]
                },
            ),
        }
    }
}

/// Whether a playerstate delta sends the `-87` view angles: with
/// `otherFlags` 2, when `eFlags` 2 flips, on a `viewlocked_entNum`, or in
/// intermission. `MSG_WriteDeltaPlayerstate` (0x813f586) tests the four
/// inline, for the last-sent count (0x813fa4a..0x813fa81) and the row
/// writes (0x813faba..0x8140c05); cod4-netfields.md 7.
fn view_goes_out(from: &PlayerState, to: &PlayerState) -> bool {
    let word = |ps: &PlayerState, name: &str| ps.get(name).unwrap_or(0);
    word(to, "otherFlags") & 2 != 0
        || word(to, "viewlocked_entNum") != ENTITYNUM_NONE
        || word(to, "pm_type") == PM_INTERMISSION
        || (word(from, "eFlags") ^ word(to, "eFlags")) & 2 != 0
}

/// One objective's rows, every one of them, or `None` when none changed.
fn objective_rows(from: &Objective, to: &Objective) -> Option<Sent> {
    let rows: Vec<Change> = fields::OBJECTIVE
        .iter()
        .map(|f| Change::natural(f, from.words[f.word()], to.words[f.word()]))
        .collect();
    rows.iter()
        .any(|&c| c != Change::Kept)
        .then_some(Sent(rows))
}

/// `MSG_ReadDeltaPlayerstate` (`predictedFieldsIgnoreXor` set, as
/// `CL_ParseSnapshot` calls it).
pub fn read_delta_playerstate(
    r: &mut MsgReader,
    ctx: &Ctx,
    from: Option<&PlayerState>,
) -> Result<(PlayerState, PsWire)> {
    let zero = PlayerState::default();
    let from = from.unwrap_or(&zero);
    let mut to = from.clone();
    let mut wire = PsWire::default();
    let table = &fields::PLAYER_STATE;
    to.origin_sent = bit(r);
    let lc = bits(r, min_bits(table.len() as u32)) as usize;
    ensure!(lc <= table.len(), "playerstate: last changed field {lc}");
    for f in &table[..lc] {
        let base = if to.origin_sent && f.hint == 3 {
            0
        } else {
            from.words[f.word()]
        };
        let (v, c) = read_field(r, ctx, base, f);
        wire.fields.0.push(c);
        to.words[f.word()] = v;
    }
    if bit(r) {
        let mask = bits(r, 5);
        wire.stats = mask as u8;
        for i in 0..3 {
            if mask & 1 << i != 0 {
                to.stats[i] = r.read_short() as i32;
            }
        }
        if mask & 8 != 0 {
            to.stats[3] = bits(r, 6) as i32;
        }
        if mask & 16 != 0 {
            to.stats[4] = r.read_byte() as i32;
        }
    }
    if bit(r) {
        for j in 0..4 {
            if bit(r) {
                let mask = r.read_short() as u16;
                for i in 0..16 {
                    if mask & 1 << i != 0 {
                        to.ammo[j * 16 + i] = r.read_short() as i32;
                    }
                }
            }
        }
    }
    for j in 0..8 {
        if bit(r) {
            let mask = r.read_short() as u16;
            for i in 0..16 {
                if mask & 1 << i != 0 {
                    to.ammoclip[j * 16 + i] = r.read_short() as i32;
                }
            }
        }
    }
    if bit(r) {
        let mut objs = Vec::with_capacity(MAX_OBJECTIVES);
        for (o, f) in to.objectives.iter_mut().zip(&from.objectives) {
            o.state = bits(r, 3);
            if bit(r) {
                let mut sent = Vec::with_capacity(fields::OBJECTIVE.len());
                for field in &fields::OBJECTIVE {
                    let (v, c) = read_field(r, ctx, f.words[field.word()], field);
                    sent.push(c);
                    o.words[field.word()] = v;
                }
                objs.push(Some(Sent(sent)));
            } else {
                objs.push(None);
            }
        }
        wire.objectives = Some(objs);
    }
    if bit(r) {
        let a = read_hudelems(r, ctx, &from.hud_archival, &mut to.hud_archival)?;
        let c = read_hudelems(r, ctx, &from.hud_current, &mut to.hud_current)?;
        wire.hud = Some([a, c]);
    }
    if bit(r) {
        for m in to.weapon_models.iter_mut() {
            *m = r.read_byte();
        }
    }
    if r.is_overflowed() {
        bail!("playerstate read past the message");
    }
    Ok((to, wire))
}

/// `MSG_WriteDeltaPlayerstate`, the mirror of [`read_delta_playerstate`]:
/// the netfield rows and the stats mask by `wire`, the ammo, clip and weapon model
/// blocks by what differs.
pub fn write_delta_playerstate(
    w: &mut MsgWriter,
    ctx: &Ctx,
    from: Option<&PlayerState>,
    to: &PlayerState,
    wire: &PsWire,
) {
    let zero = PlayerState::default();
    let from = from.unwrap_or(&zero);
    let table = &fields::PLAYER_STATE;
    put_bit(w, to.origin_sent);
    put_bits(w, wire.fields.lc() as u32, min_bits(table.len() as u32));
    for (f, &changed) in table.iter().zip(&wire.fields.0) {
        let base = if to.origin_sent && f.hint == 3 {
            0
        } else {
            from.words[f.word()]
        };
        write_field(w, ctx, base, to.words[f.word()], f, changed);
    }
    let mask = wire.stats as u32;
    put_bit(w, mask != 0);
    if mask != 0 {
        put_bits(w, mask, 5);
        for i in 0..3 {
            if mask & 1 << i != 0 {
                w.write_short(to.stats[i] as i16);
            }
        }
        if mask & 8 != 0 {
            put_bits(w, to.stats[3] as u32, 6);
        }
        if mask & 16 != 0 {
            w.write_byte(to.stats[4] as u8);
        }
    }
    let masks = |a: &[i32], b: &[i32], n: usize| -> Vec<u16> {
        (0..n)
            .map(|j| {
                (0..16).fold(0u16, |m, i| {
                    m | ((a[j * 16 + i] != b[j * 16 + i]) as u16) << i
                })
            })
            .collect()
    };
    let ammo = masks(&from.ammo, &to.ammo, 4);
    put_bit(w, ammo.iter().any(|&m| m != 0));
    if ammo.iter().any(|&m| m != 0) {
        for (j, &m) in ammo.iter().enumerate() {
            put_bit(w, m != 0);
            if m != 0 {
                w.write_short(m as i16);
                for i in (0..16).filter(|i| m & 1 << i != 0) {
                    w.write_short(to.ammo[j * 16 + i] as i16);
                }
            }
        }
    }
    for (j, m) in masks(&from.ammoclip, &to.ammoclip, 8)
        .into_iter()
        .enumerate()
    {
        put_bit(w, m != 0);
        if m != 0 {
            w.write_short(m as i16);
            for i in (0..16).filter(|i| m & 1 << i != 0) {
                w.write_short(to.ammoclip[j * 16 + i] as i16);
            }
        }
    }
    put_bit(w, wire.objectives.is_some());
    if let Some(objs) = &wire.objectives {
        for ((a, b), s) in from.objectives.iter().zip(&to.objectives).zip(objs) {
            put_bits(w, b.state, 3);
            put_bit(w, s.is_some());
            if let Some(s) = s {
                for (f, &changed) in fields::OBJECTIVE.iter().zip(&s.0) {
                    write_field(w, ctx, a.words[f.word()], b.words[f.word()], f, changed);
                }
            }
        }
    }
    put_bit(w, wire.hud.is_some());
    if let Some([a, c]) = &wire.hud {
        write_hudelems(w, ctx, &from.hud_archival, &to.hud_archival, a);
        write_hudelems(w, ctx, &from.hud_current, &to.hud_current, c);
    }
    let models = from.weapon_models != to.weapon_models;
    put_bit(w, models);
    if models {
        for &m in &to.weapon_models {
            w.write_byte(m);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn min_bits_matches_the_table_widths() {
        assert_eq!(min_bits(61), 6);
        assert_eq!(min_bits(141), 8);
        assert_eq!(min_bits(24), 5);
    }

    fn f(v: f32) -> u32 {
        v.to_bits()
    }

    /// One case per field code form: the plain bits `write_field` puts
    /// out and the word `read_field` takes back from them. Pinned before
    /// the codec's rewrite; the capture re-encodes in `net_decode` check
    /// the same forms against retail.
    #[test]
    fn every_field_form_writes_and_reads_as_pinned() {
        let ctx = Ctx {
            time: 50_000,
            map_center: [429.0, 126.5, 137.0],
        };
        let toggle_from = u32::from_le_bytes([10, 20, 30, 0xff]);
        let toggle_to = u32::from_le_bytes([10, 20, 30, 0]);
        let cases: &[(i32, u32, u32, Change)] = &[
            (0, f(3.0), f(0.0), Change::Sent),
            (0, f(3.0), 0x8000_0000, Change::Sent),
            (0, f(3.0), f(5.0), Change::Sent),
            (0, f(-4096.0), f(4095.0), Change::Sent),
            (0, f(0.0), f(4096.0), Change::Sent),
            (0, f(0.0), f(1.5), Change::Sent),
            (0, f(5000.0), f(7.0), Change::Sent),
            (-89, f(9000.0), f(3.0), Change::Sent),
            (-89, f(1.0), f(0.0), Change::Sent),
            (-89, f(1.0), 0x8000_0000, Change::Sent),
            (-89, f(1.0), f(-5000.0), Change::Sent),
            (-88, f(1.0), f(2.5), Change::Sent),
            (-99, f(7.0), f(0.0), Change::Sent),
            (-99, f(7.0), 0x8000_0000, Change::Sent),
            (-99, f(7.0), f(100.0), Change::Sent),
            (-99, f(7.0), f(2048.0), Change::Sent),
            (-99, f(7.0), f(-2048.0), Change::Sent),
            (-99, f(7.0), f(0.25), Change::Sent),
            (-98, 0x0012_3456, 0x0012_3456 ^ 0x40_0000, Change::Sent),
            (-98, 0x0012_3456, 0x00ab_cdef, Change::Sent),
            (-98, 0x0012_3456, 0x0012_3456, Change::Sent),
            (-97, 0, 50_000, Change::Sent),
            (-97, 0, 49_745, Change::Sent),
            (-97, 0, 49_744, Change::Sent),
            (-97, 0, 50_001, Change::Sent),
            (-96, 3, 1022, Change::Sent),
            (-96, 3, 0, Change::Sent),
            (-96, 3, 5, Change::Sent),
            (-96, 3, 1021, Change::Sent),
            (-95, 0, 1500, Change::Sent),
            (-94, 0, 200, Change::Sent),
            (-93, 0, 7, Change::Sent),
            (-92, f(400.0), f(410.0), Change::Sent),
            (-92, f(400.0), f(1912.5), Change::Sent),
            (-91, f(-521.5), f(-600.0), Change::Sent),
            (-91, f(100.0), f(36.0), Change::Sent),
            (-90, f(100.0), f(163.4), Change::Sent),
            (-90, f(100.0), f(-3000.0), Change::Sent),
            (-100, f(10.0), f(0.0), Change::Sent),
            (-100, f(10.0), 0x8000_0000, Change::Sent),
            (-100, f(10.0), f(90.0), Change::Sent),
            (-100, f(10.0), f(-45.3), Change::Sent),
            (-100, f(10.0), f(0.001), Change::Sent),
            (-87, f(0.0), f(270.0), Change::Sent),
            (-87, f(0.0), f(-179.9), Change::Sent),
            (-86, 0, f(1.4), Change::Sent),
            (-86, 0, f(2.0), Change::Sent),
            (-86, 0, f(4.5), Change::Sent),
            (-85, toggle_from, toggle_to, Change::Toggled),
            (
                -85,
                toggle_from,
                u32::from_le_bytes([10, 20, 30, 0x80]),
                Change::Sent,
            ),
            (
                -85,
                toggle_from,
                u32::from_le_bytes([1, 2, 3, 0xf8]),
                Change::Sent,
            ),
            (8, 0x11, 0x22, Change::Sent),
            (8, 0x11, 0, Change::Sent),
            (10, 0x3ff, 0x155, Change::Sent),
            (-10, 0, 0xffff_fe00, Change::Sent),
            (32, 0xdead_beef, 0x0123_4567, Change::Sent),
            (5, 3, 9, Change::Kept),
        ];
        let mut got = Vec::new();
        for &(bits, from, to, change) in cases {
            let field = Field::new("pin", 0, bits, 0);
            let mut w = MsgWriter::new(&crate::net::parse::UNUSED_HUFF);
            write_field(&mut w, &ctx, from, to, &field, change);
            let plain = w.into_ops();
            let mut r = MsgReader::from_plain(plain.clone());
            let (word, read_change) = read_field(&mut r, &ctx, from, &field);
            assert_eq!(read_change, change, "code {bits} {from:#x} -> {to:#x}");
            let hex: String = plain.iter().map(|b| format!("{b:02x}")).collect();
            got.push(format!("{bits} {from:08x} {to:08x} {hex} {word:08x}"));
        }
        let expected = [
            "0 40400000 00000000 01 00000000",
            "0 40400000 80000000 05 80000000",
            "0 40400000 40a00000 3300 40a00000",
            "0 c5800000 457ff000 fbff 457ff000",
            "0 00000000 45800000 0700008045 45800000",
            "0 00000000 3fc00000 070000c03f 3fc00000",
            "0 459c4000 40e00000 0700407c05 40e00000",
            "-89 460ca000 40400000 2d19 46000c00",
            "-89 3f800000 00000000 0500 00000000",
            "-89 3f800000 80000000 03000080bf 80000000",
            "-89 3f800000 c59c4000 0300401cfa c59c4000",
            "-88 3f800000 40200000 010000a07f 40200000",
            "-99 40e00000 00000000 01 00000000",
            "-99 40e00000 80000000 070000e0c0 80000000",
            "-99 40e00000 42c80000 1b06 42c80000",
            "-99 40e00000 45000000 070000e005 45000000",
            "-99 40e00000 c5000000 3b80 c5000000",
            "-99 40e00000 3e800000 070000607e 3e800000",
            "-98 00123456 00523456 59 00523456",
            "-98 00123456 00abcdef 03efcdab 00abcdef",
            "-98 00123456 00123456 03563412 00123456",
            "-97 00000000 0000c350 0100 0000c350",
            "-97 00000000 0000c251 fd03 0000c251",
            "-97 00000000 0000c250 0350c20000 0000c250",
            "-97 00000000 0000c351 0351c30000 0000c351",
            "-96 00000003 000003fe 03 000003fe",
            "-96 00000003 00000000 05 00000000",
            "-96 00000003 00000005 0901 00000005",
            "-96 00000003 000003fd 09ff 000003fd",
            "-95 00000000 000005dc 1f 000005dc",
            "-94 00000000 000000c8 01c8 000000c8",
            "-93 00000000 00000007 0107 00000007",
            "-92 43c80000 43cd0000 2901 43cd0000",
            "-92 43c80000 44ef1000 bfe803 44ef2000",
            "-91 c4026000 c4160000 470100 c4160000",
            "-91 42c80000 42100000 0100 42100000",
            "-90 42c80000 43236666 fd01 43230000",
            "-90 42c80000 c53b8000 933100 c53b8000",
            "-100 41200000 00000000 01 00000000",
            "-100 41200000 80000000 030000 00000001",
            "-100 41200000 42b40000 030040 42b40000",
            "-100 41200000 c2353333 03cadf c2352fc0",
            "-100 41200000 3a83126f 030000 00000001",
            "-87 00000000 43870000 0100c0 c2b40000",
            "-87 00000000 c333e666 011380 c333e548",
            "-86 00000000 3fb33333 01 3fb33333",
            "-86 00000000 40000000 0d 40000000",
            "-86 00000000 40900000 3f 40900000",
            "-85 ff1e140a 001e140a 03 001e140a",
            "-85 ff1e140a 801e140a 85 801e140a",
            "-85 ff1e140a f8030201 f9010203 f8030201",
            "8 00000011 00000022 0333 00000022",
            "8 00000011 00000000 01 00000000",
            "10 000003ff 00000155 0baa 00000155",
            "-10 00000000 fffffe00 0380 fffffe00",
            "32 deadbeef 01234567 0388fb8edf 01234567",
            "5 00000003 00000009 00 00000003",
        ];
        assert_eq!(got, expected);
    }

    fn letters(s: &Sent) -> String {
        s.0.iter()
            .map(|c| match c {
                Change::Kept => 'k',
                Change::Sent => 's',
                Change::Toggled => 't',
            })
            .collect()
    }

    fn summary(w: &PsWire) -> String {
        let objectives = w.objectives.as_ref().map(|o| {
            o.iter()
                .map(|s| s.as_ref().map_or("-".to_string(), letters))
                .collect::<Vec<_>>()
                .join(",")
        });
        let hud = w.hud.as_ref().map(|h| {
            h.iter()
                .map(|v| v.iter().map(letters).collect::<Vec<_>>().join(","))
                .collect::<Vec<_>>()
                .join("|")
        });
        format!(
            "{} stats={:#x} obj={objectives:?} hud={hud:?}",
            letters(&w.fields),
            w.stats
        )
    }

    fn set(ps: &mut PlayerState, name: &str, v: u32) {
        let f = fields::PLAYER_STATE
            .iter()
            .find(|f| f.name == name)
            .unwrap();
        ps.words[f.word()] = v;
    }

    /// The playerstate rows, stats, objectives and HUD blocks a server
    /// picks between two states. Pinned before the rewrite; `wire_ab`
    /// checks the same choices against retail's snapshots.
    #[test]
    fn a_playerstate_delta_picks_its_rows_as_pinned() {
        let mut base = PlayerState::default();
        set(&mut base, "viewlocked_entNum", ENTITYNUM_NONE);
        base.origin_sent = false;
        let mut cases = Vec::new();
        cases.push(base.clone());
        let mut view = base.clone();
        set(&mut view, "viewangles[1]", 90f32.to_bits());
        cases.push(view.clone());
        let mut flip = view.clone();
        set(&mut flip, "eFlags", 2);
        cases.push(flip);
        let mut locked = view.clone();
        set(&mut locked, "viewlocked_entNum", 7);
        cases.push(locked);
        let mut inter = view.clone();
        set(&mut inter, "pm_type", PM_INTERMISSION);
        cases.push(inter);
        let mut other = view.clone();
        set(&mut other, "otherFlags", 2);
        cases.push(other);
        let mut moved = base.clone();
        moved.origin_sent = true;
        cases.push(moved);
        let mut blocks = base.clone();
        set(&mut blocks, "pm_type", 3);
        blocks.stats[1] = 4;
        blocks.stats[3] = 70;
        blocks.objectives[2].state = 1;
        blocks.objectives[5].words[1] = 9;
        blocks.hud_current[0].0[0] = 1;
        blocks.hud_current[0].0[5] = 3;
        blocks.hud_current[1].0[0] = 1;
        cases.push(blocks);
        let got: Vec<String> = cases
            .iter()
            .map(|to| summary(&PsWire::natural(&base, to)))
            .collect();
        let expected = [
            " stats=0x0 obj=None hud=None",
            " stats=0x0 obj=None hud=None",
            "kssskkkkkkkkkkkkkkkkkkkkkkkkkks stats=0x0 obj=None hud=None",
            "kssskkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkks stats=0x0 obj=None hud=None",
            "kssskkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkskkkksss stats=0x0 obj=None hud=None",
            "kssskkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkks stats=0x0 obj=None hud=None",
            "kkkksssssskkskkkkkks stats=0x0 obj=None hud=None",
            "kkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkskkkksss stats=0xa obj=Some(\"-,-,-,-,-,skkkkk,-,-,-,-,-,-,-,-,-,-\") hud=Some(\"|kkkkskkkkkkkkkkks,kkkks\")",
        ];
        assert_eq!(got, expected);
    }
}
