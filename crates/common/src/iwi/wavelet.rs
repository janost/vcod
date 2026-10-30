//! CoD2's wavelet-compressed `.iwi` levels (formats 6..=10):
//! `Image_LoadWavelet` and `Wavelet_DecompressLevel`. The bit stream, the
//! level reconstruction and why the prefix-code tables are read from the
//! install are in docs/research/cod2-iwi-iwd.md section 4.

use super::IwiLevel;
use anyhow::{Result, ensure};
use std::path::Path;
use std::sync::OnceLock;

/// Entries in a decode table: one per value of the window's low 12 bits.
const TABLE_ENTRIES: usize = 4096;
const TABLE_BYTES: usize = TABLE_ENTRIES * 4;
/// A table value that escapes to a literal.
const ESCAPE: i16 = -0x8000;

/// FNV-1a 64 of each 16 KiB table as the 1.3 render DLLs and the Mac 1.3
/// binary store it: `waveletDecodeBlue`, `waveletDecodeRedGreen`,
/// `waveletDecodeAlpha`.
const TABLE_HASHES: [u64; 3] = [
    0x2c2d_b1f5_3dfe_354d,
    0x0616_70ce_d362_09a3,
    0x5a5b_0527_3f37_3fdd,
];

/// The install files searched for the tables, under the install root:
/// CoD2's render DLLs, and CoD4's executable, which holds the same three
/// (cod4-world.md).
const TABLE_SOURCES: [&str; 3] = ["gfx_d3d_mp_x86_s.dll", "gfx_d3d_x86_s.dll", "iw3mp.exe"];

/// One prefix-code table: per 12-bit window, the value and its code length.
struct Table(Box<[(i16, u8)]>);

/// The three decode tables `Wavelet_DecompressLevel` reads.
pub struct WaveletTables {
    blue: Table,
    red_green: Table,
    alpha: Table,
}

static TABLES: OnceLock<WaveletTables> = OnceLock::new();

fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |h, &b| {
        (h ^ u64::from(b)).wrapping_mul(0x100_0000_01b3)
    })
}

/// `bytes` read as a table when every entry has a length 1..=12 and every
/// window sharing an entry's low `length` bits holds the same entry: the
/// shape a prefix-code lookup table has. Cheap to refuse, so a scan can try
/// every offset.
fn table_at(bytes: &[u8]) -> Option<Table> {
    let entry = |i: usize| {
        let o = i * 4;
        let v = i16::from_le_bytes([bytes[o], bytes[o + 1]]);
        let l = u16::from_le_bytes([bytes[o + 2], bytes[o + 3]]);
        (v, l)
    };
    let shaped = (0..TABLE_ENTRIES).all(|i| {
        let (v, l) = entry(i);
        (1..=12).contains(&l) && entry(i & ((1 << l) - 1)) == (v, l)
    });
    shaped.then(|| {
        Table(
            (0..TABLE_ENTRIES)
                .map(|i| (entry(i).0, entry(i).1 as u8))
                .collect(),
        )
    })
}

impl WaveletTables {
    /// Finds the three tables anywhere in `bin` (a CoD2 executable or render
    /// DLL), each named by its hash.
    pub fn find(bin: &[u8]) -> Option<Self> {
        let mut found: [Option<Table>; 3] = [None, None, None];
        let mut off = 0;
        while off + TABLE_BYTES <= bin.len() {
            let window = &bin[off..off + TABLE_BYTES];
            let hit = table_at(window).and_then(|table| {
                let slot = TABLE_HASHES.iter().position(|&h| h == fnv1a(window))?;
                Some((slot, table))
            });
            match hit {
                Some((slot, table)) => {
                    found[slot] = Some(table);
                    if found.iter().all(Option::is_some) {
                        break;
                    }
                    off += TABLE_BYTES;
                }
                None => off += 4,
            }
        }
        let [Some(blue), Some(red_green), Some(alpha)] = found else {
            return None;
        };
        Some(Self {
            blue,
            red_green,
            alpha,
        })
    }

    /// The tables out of the install whose root (the directory holding
    /// `main/`) is `root`.
    pub fn from_install(root: &Path) -> Option<Self> {
        TABLE_SOURCES.iter().find_map(|name| {
            let bin = std::fs::read(root.join(name)).ok()?;
            Self::find(&bin)
        })
    }
}

/// Reads the tables from the install at `root` for every later wavelet
/// decode, once a process; true when they are loaded.
pub fn load_tables(root: &Path) -> bool {
    if TABLES.get().is_some() {
        return true;
    }
    match WaveletTables::from_install(root) {
        Some(t) => {
            let _ = TABLES.set(t);
            true
        }
        None => {
            log::warn!(
                "no wavelet decode tables in {}: wavelet .iwi images draw grey",
                root.display()
            );
            false
        }
    }
}

/// The loaded tables, if [`load_tables`] found them.
pub fn tables() -> Option<&'static WaveletTables> {
    TABLES.get()
}

/// The stream after the raw levels: bits taken least significant first,
/// the next 16 always in view (`WaveletDecode`'s window). Reads past the
/// end as zeros.
struct Bits<'a> {
    data: &'a [u8],
    pos: usize,
}

impl Bits<'_> {
    fn peek(&self) -> u32 {
        let byte = self.pos >> 3;
        let b = |i| u32::from(self.data.get(byte + i).copied().unwrap_or(0));
        ((b(0) | b(1) << 8 | b(2) << 16) >> (self.pos & 7)) & 0xffff
    }

    fn skip(&mut self, n: u32) {
        self.pos += n as usize;
    }

    fn bit(&mut self) -> u8 {
        let b = self.peek() as u8 & 1;
        self.skip(1);
        b
    }

    /// A prefix code through `t`; the escape takes the next `esc.0` bits as
    /// a literal less `esc.1`.
    fn code(&mut self, t: &Table, (esc_bits, bias): (u32, i32)) -> i32 {
        let (v, l) = t.0[(self.peek() & 0xfff) as usize];
        self.skip(u32::from(l));
        if v != ESCAPE {
            return i32::from(v);
        }
        let lit = (self.peek() & ((1 << esc_bits) - 1)) as i32 - bias;
        self.skip(esc_bits);
        lit
    }

    /// A rounding bit and three detail coefficients through `t`.
    fn details(&mut self, t: &Table, esc: (u32, i32)) -> (u8, [i32; 3]) {
        let bit = self.bit();
        let d = [self.code(t, esc), self.code(t, esc), self.code(t, esc)];
        (bit, d)
    }
}

/// Alpha and blue literals are 9 bits less 255, red and green 10 less 510.
const ESC_9: (u32, i32) = (9, 0xff);
const ESC_10: (u32, i32) = (10, 0x1fe);

/// One channel's 2x2 block from its base value `b`: the inverse of a Haar
/// step, every sum halved with an arithmetic shift and kept mod 256.
fn haar(b: u8, bit: u8, [d0, d1, d2]: [i32; 3]) -> [u8; 4] {
    let s = d0 + 2 * i32::from(b);
    let t = 2 * i32::from(b) - d0;
    [
        (((s + d1 + d2) >> 1) as u8).wrapping_add(bit),
        ((s - (d1 + d2)) >> 1) as u8,
        ((t + (d1 - d2)) >> 1) as u8,
        ((t - (d1 - d2)) >> 1) as u8,
    ]
}

/// Decodes every level of every face, largest level first, each face's
/// pixels in its bitmap format's byte order (three-byte format 7 widened
/// to four with 0xff, as the loader's output size has it). `src` is the
/// bytes per pixel the stream codes: 4, 3, 2, 1, 1 for formats 6..=10.
/// Also gives the file offset each level's bits end at, largest first.
pub(super) fn decode(
    t: &WaveletTables,
    pixels: &[u8],
    (w, h): (u32, u32),
    levels: u32,
    faces: usize,
    src: usize,
) -> Result<(Vec<IwiLevel>, Vec<usize>)> {
    let out = if src == 3 { 4 } else { src };
    let mut byte = 0usize;
    let mut bits: Option<Bits> = None;
    let mut decoded: Vec<IwiLevel> = Vec::new();
    let mut ends = Vec::new();
    // each face's previous level, refined in place by the next
    let mut base: Vec<Vec<u8>> = vec![Vec::new(); faces];
    for level in (0..levels).rev() {
        let (lw, lh) = ((w >> level).max(1) as usize, (h >> level).max(1) as usize);
        let mut level_faces = Vec::with_capacity(faces);
        for prev in base.iter_mut() {
            let mut px = vec![0u8; lw * lh * out];
            if lw < 2 || lh < 2 {
                // stored raw, before the bit stream starts
                ensure!(bits.is_none(), "raw {lw}x{lh} level after a coded one");
                let n = lw * lh * src;
                ensure!(byte + n <= pixels.len(), "wavelet iwi truncated");
                for (o, i) in px
                    .chunks_exact_mut(out)
                    .zip(pixels[byte..byte + n].chunks(src))
                {
                    o[..src].copy_from_slice(i);
                    if out != src {
                        o[src] = 0xff;
                    }
                }
                byte += n;
            } else {
                ensure!(lw % 2 == 0 && lh % 2 == 0, "odd wavelet level {lw}x{lh}");
                ensure!(prev.len() * 4 == px.len(), "wavelet base size mismatch");
                let s = bits.get_or_insert_with(|| Bits {
                    data: pixels,
                    pos: byte * 8,
                });
                ensure!(s.pos / 8 < pixels.len(), "wavelet iwi truncated");
                refine_base(s, t, prev, src, out);
                code_level(s, t, prev, &mut px, lw, src, out);
            }
            *prev = px.clone();
            level_faces.push(px);
        }
        // the window keeps the next 16 bits in view; the level ends at the
        // byte holding its last bit
        ends.push(super::HEADER_LEN + bits.as_ref().map_or(byte, |s| s.pos.div_ceil(8)));
        decoded.push(IwiLevel {
            width: lw as u32,
            height: lh as u32,
            faces: level_faces,
        });
    }
    if let Some(s) = &bits {
        ensure!(
            s.pos.div_ceil(8) <= pixels.len(),
            "wavelet iwi truncated: {} of {} bytes",
            s.pos.div_ceil(8),
            pixels.len()
        );
    }
    decoded.reverse();
    ends.reverse();
    Ok((decoded, ends))
}

/// The level's first bit: when set, a delta for every coded byte of the
/// base through the alpha table.
fn refine_base(s: &mut Bits, t: &WaveletTables, base: &mut [u8], src: usize, out: usize) {
    if s.bit() == 0 {
        return;
    }
    for p in base.chunks_exact_mut(out) {
        for c in &mut p[..src] {
            *c = c.wrapping_add(s.code(&t.alpha, ESC_9) as u8);
        }
        if out != src {
            p[src] = 0xff;
        }
    }
}

/// Every 2x2 block of a level, row by row, from one base pixel each.
/// Green and red code their details as blue's plus a residual.
fn code_level(
    s: &mut Bits,
    t: &WaveletTables,
    base: &[u8],
    px: &mut [u8],
    w: usize,
    src: usize,
    out: usize,
) {
    let row = w * out;
    for (i, b) in base.chunks_exact(out).enumerate() {
        let (bx, by) = (i % (w / 2), i / (w / 2));
        let p = by * 2 * row + bx * 2 * out;
        let mut put = |c: usize, v: [u8; 4]| {
            px[p + c] = v[0];
            px[p + out + c] = v[1];
            px[p + row + c] = v[2];
            px[p + row + out + c] = v[3];
        };
        if src != 1 {
            let (bit, blue) = s.details(&t.blue, ESC_9);
            put(0, haar(b[0], bit, blue));
            if src > 2 {
                for c in [1, 2] {
                    let (bit, d) = s.details(&t.red_green, ESC_10);
                    put(
                        c,
                        haar(b[c], bit, [d[0] + blue[0], d[1] + blue[1], d[2] + blue[2]]),
                    );
                }
            }
        }
        if src == 3 {
            put(3, [0xff; 4]);
        } else {
            let (bit, d) = s.details(&t.alpha, ESC_9);
            put(out - 1, haar(b[out - 1], bit, d));
        }
    }
}

#[cfg(test)]
fn is_table(bin: &[u8]) -> bool {
    bin.len() >= TABLE_BYTES && table_at(&bin[..TABLE_BYTES]).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A table whose code is the low bit: 0 reads `a`, 1 reads `b`.
    fn one_bit(a: i16, b: i16) -> Table {
        Table(
            (0..TABLE_ENTRIES)
                .map(|i| (if i & 1 == 0 { a } else { b }, 1))
                .collect(),
        )
    }

    #[test]
    fn table_shape_is_checked() {
        let mut bin = Vec::new();
        for i in 0..TABLE_ENTRIES {
            let (v, l): (i16, u16) = if i & 1 == 0 { (0, 1) } else { (5, 2) };
            bin.extend_from_slice(&v.to_le_bytes());
            bin.extend_from_slice(&l.to_le_bytes());
        }
        // (5, 2) at windows 1 and 3 but not 3 alone: a valid prefix shape
        assert!(is_table(&bin));
        bin[3 * 4] = 6; // window 3 now disagrees with window 1
        assert!(!is_table(&bin));
    }

    #[test]
    fn bits_escape_reads_a_literal() {
        let t = one_bit(0, ESCAPE);
        // bit 0: code 1 escapes; bits 1..=9: 0x1ff, so 511 - 255
        let data = [0xff, 0x03, 0];
        let mut s = Bits {
            data: &data,
            pos: 0,
        };
        assert_eq!(s.code(&t, ESC_9), 256);
        assert_eq!(s.pos, 10);
        assert_eq!(s.code(&t, ESC_9), 0);
    }

    #[test]
    fn haar_inverts_a_flat_block() {
        assert_eq!(haar(100, 0, [0, 0, 0]), [100; 4]);
        // TL = (2b + d0 + d1 + d2) / 2 + bit, BR = (2b - d0 - d1 + d2) / 2
        assert_eq!(haar(100, 1, [4, 2, 0]), [104, 101, 99, 97]);
    }

    #[test]
    fn rgb_level_reads_raw_then_codes() {
        let tables = WaveletTables {
            blue: one_bit(0, 2),
            red_green: one_bit(0, 1),
            alpha: one_bit(0, 1),
        };
        // 2x2 RGB: 1x1 raw (3 bytes), then the coded level: no refine bit,
        // blue (bit, 3 codes), green (bit, 3), red (bit, 3), all zeros
        let mut data = vec![10, 20, 30];
        data.extend_from_slice(&[0, 0]);
        let (levels, ends) = decode(&tables, &data, (2, 2), 2, 1, 3).unwrap();
        // 3 raw bytes, then 13 bits
        assert_eq!(
            ends,
            [super::super::HEADER_LEN + 5, super::super::HEADER_LEN + 3]
        );
        assert_eq!(levels.len(), 2);
        assert_eq!(levels[1].faces[0], [10, 20, 30, 0xff]);
        assert_eq!(levels[0].faces[0], [10, 20, 30, 0xff].repeat(4));
    }
}
