//! The array blocks that close a playerstate delta, one wire on CoD1 and
//! CoD2 (docs/protocol-1.1.md, "The five array blocks"; cod2-protocol.md
//! 5.3): the six stats by width, the ammo and clip arrays in gated
//! sub-blocks of 16, an objective's state and fields, a HUD element's
//! last-changed index and fields. Each game keeps its block order, gates
//! and field tables; these are the blocks' insides.

use super::codec::{FieldCodec, MsgReader, MsgWriter, Word, write_delta_field};

/// Block 1's scalars, `ps.stats[6]`.
pub const STATS: usize = 6;

/// Block 1's widths in mask-bit order: shorts, but `stats[3]` 6 unsigned
/// bits and `stats[5]` a byte.
enum StatWidth {
    Short,
    Bits(i32),
    Byte,
}

const STAT_WIDTHS: [StatWidth; STATS] = [
    StatWidth::Short,
    StatWidth::Short,
    StatWidth::Short,
    StatWidth::Bits(6),
    StatWidth::Short,
    StatWidth::Byte,
];

/// Block 1: a gate, a 6-bit mask of the stats that differ from the base,
/// then each of those by its width.
pub fn write_stats(w: &mut MsgWriter, from: &[i32; STATS], to: &[i32; STATS]) {
    let mask = (0..STATS).fold(0, |m, i| m | i32::from(from[i] != to[i]) << i);
    if mask == 0 {
        w.write_bits(0, 1);
        return;
    }
    w.write_bits(1, 1);
    w.write_bits(mask, STATS as i32);
    for (i, width) in STAT_WIDTHS.iter().enumerate() {
        if mask >> i & 1 != 0 {
            match width {
                StatWidth::Short => w.write_short(to[i] as i16),
                StatWidth::Bits(bits) => w.write_bits(to[i], *bits),
                StatWidth::Byte => w.write_byte(to[i] as u8),
            }
        }
    }
}

/// The read of [`write_stats`] over `out`, which holds the base.
pub fn read_stats(r: &mut MsgReader, out: &mut [i32; STATS]) {
    if r.read_bits(1) == 0 {
        return;
    }
    let mask = r.read_bits(STATS as i32);
    for (i, width) in STAT_WIDTHS.iter().enumerate() {
        if mask >> i & 1 != 0 {
            out[i] = match width {
                StatWidth::Short => i32::from(r.read_short()),
                StatWidth::Bits(bits) => r.read_bits(*bits),
                StatWidth::Byte => i32::from(r.read_byte()),
            };
        }
    }
}

/// A slot of blocks 2 and 3: CoD1 keeps `i16`, CoD2 `i32`.
pub trait Short: Copy + PartialEq {
    fn short(self) -> i16;
    fn of_short(v: i16) -> Self;
}

impl Short for i16 {
    fn short(self) -> i16 {
        self
    }

    fn of_short(v: i16) -> i16 {
        v
    }
}

impl Short for i32 {
    fn short(self) -> i16 {
        self as i16
    }

    fn of_short(v: i16) -> i32 {
        i32::from(v)
    }
}

/// Elements per gated sub-block of blocks 2 and 3; the mask is one short.
const SUB_BLOCK: usize = 16;

/// Blocks 2 and 3, `ps.ammo[64]` and `ps.ammoclip[64]` (cod_lnxded
/// 0x807ea4b, 0x807eb49): four sub-blocks of 16, each a gate, a 16-bit
/// changed mask and the changed shorts low to high.
pub fn write_short_groups<T: Short>(w: &mut MsgWriter, from: &[T; 64], to: &[T; 64]) {
    for block in to.chunks(SUB_BLOCK).zip(from.chunks(SUB_BLOCK)) {
        let (to, from) = block;
        let mask = (0..SUB_BLOCK).fold(0u16, |m, i| m | u16::from(from[i] != to[i]) << i);
        if mask == 0 {
            w.write_bits(0, 1);
            continue;
        }
        w.write_bits(1, 1);
        w.write_short(mask as i16);
        for (i, v) in to.iter().enumerate() {
            if mask >> i & 1 != 0 {
                w.write_short(v.short());
            }
        }
    }
}

/// The read of [`write_short_groups`] over `out`, which holds the base.
pub fn read_short_groups<T: Short>(r: &mut MsgReader, out: &mut [T; 64]) {
    for block in out.chunks_mut(SUB_BLOCK) {
        if r.read_bits(1) == 0 {
            continue;
        }
        let mask = r.read_short() as u16;
        for (i, v) in block.iter_mut().enumerate() {
            if mask >> i & 1 != 0 {
                *v = T::of_short(r.read_short());
            }
        }
    }
}

/// Block 4's per-objective state, ahead of the fields' gate.
pub const OBJECTIVE_STATE_BITS: i32 = 3;

/// One objective of block 4 (docs/protocol-1.1.md, "Block 4"): its
/// state, then a gate, clear when the six fields equal the base's, and
/// the fields as deltas of `bits`.
pub fn write_objective<T: Word>(
    w: &mut MsgWriter,
    state: i32,
    from: &[T; 6],
    to: &[T; 6],
    bits: &[i32; 6],
) {
    w.write_bits(state, OBJECTIVE_STATE_BITS);
    if from == to {
        w.write_bits(0, 1);
        return;
    }
    w.write_bits(1, 1);
    for i in 0..6 {
        write_delta_field(w, from[i].word(), to[i].word(), bits[i]);
    }
}

/// The read of [`write_objective`] on `codec`: its state back; `to` takes
/// the fields, the base's under a clear gate.
pub fn read_objective<T: Word>(
    r: &mut MsgReader,
    codec: FieldCodec,
    from: &[T; 6],
    to: &mut [T; 6],
    bits: &[i32; 6],
) -> i32 {
    let state = r.read_bits(OBJECTIVE_STATE_BITS);
    if r.read_bits(1) == 0 {
        *to = *from;
        return state;
    }
    for i in 0..6 {
        to[i] = T::of_word(codec.read_delta_field(r, from[i].word(), bits[i]));
    }
    state
}

/// Block 5's per-element last-changed field index.
pub const HUD_INDEX_BITS: i32 = 5;

/// One HUD element of block 5 (cod_lnxded 0x807ce85): the index of its
/// last field that differs from the base, 0 when none does, so an
/// unchanged element still carries one clear bit; then fields `0..=last`
/// as deltas of `bits(k)`.
pub fn write_hud_elem<T: Word>(
    w: &mut MsgWriter,
    from: &[T],
    to: &[T],
    bits: impl Fn(usize) -> i32,
) {
    let last = from.iter().zip(to).rposition(|(a, b)| a != b).unwrap_or(0);
    w.write_bits(last as i32, HUD_INDEX_BITS);
    for k in 0..=last {
        write_delta_field(w, from[k].word(), to[k].word(), bits(k));
    }
}
