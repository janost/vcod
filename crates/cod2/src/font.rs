//! CoD2's `fonts/<name>` files: the renderer's `Font_s` written out with its
//! pointers as file offsets (docs/research/cod2-fonts-menus.md, section 1).

use anyhow::{Result, bail, ensure};

/// The file's 16-byte header; the glyphs follow it.
const HEADER: usize = 16;
/// One glyph record.
const GLYPH: usize = 24;
/// Glyphs 0x20..0x7f sit first, one per letter in order; the rest are
/// sorted by letter (`R_GetCharacterGlyph`).
const ASCII_RUN: usize = 0x60;
/// The glyph a missing letter draws: index 14 of the ASCII run, `.`.
const MISSING: usize = 14;

/// One glyph, pixels on the font's own image.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Glyph {
    pub letter: u16,
    /// Left bearing.
    pub x0: i8,
    /// Baseline to the glyph's top; negative above the baseline.
    pub y0: i8,
    /// Advance.
    pub dx: u8,
    pub pixel_width: u8,
    pub pixel_height: u8,
    pub s0: f32,
    pub t0: f32,
    pub s1: f32,
    pub t1: f32,
}

#[derive(Debug, Clone)]
pub struct Font {
    /// `fonts/normalFont`, as the file names itself.
    pub name: String,
    /// The em in pixels; text drawn at `scale` is `48 * scale / pixel_height`
    /// times the glyph pixels (`R_NormalizedTextScale`).
    pub pixel_height: i32,
    /// The material the glyphs sit on (`fonts/gamefonts`).
    pub material: String,
    pub glyphs: Vec<Glyph>,
}

impl Font {
    /// The glyph `R_GetCharacterGlyph` draws for `letter`.
    pub fn glyph(&self, letter: u32) -> &Glyph {
        let i = if (0x20..0x80).contains(&letter) {
            Some(letter as usize - 0x20)
        } else {
            self.glyphs[ASCII_RUN.min(self.glyphs.len())..]
                .binary_search_by_key(&letter, |g| u32::from(g.letter))
                .ok()
                .map(|i| i + ASCII_RUN)
        };
        &self.glyphs[i.unwrap_or(MISSING)]
    }
}

/// The font a size of `assetGlobalDef` names: `ui_mp/main.menu` registers
/// these six files under the sizes CoD1's `fontImage_<size>.dat` used.
pub fn name_for_size(size: u32) -> Option<&'static str> {
    Some(match size {
        12 => "fonts/smallFont",
        16 => "fonts/normalFont",
        18 => "fonts/consoleFont",
        24 => "fonts/bigFont",
        30 => "fonts/boldFont",
        32 => "fonts/extraBigFont",
        _ => return None,
    })
}

/// Parses a `fonts/*` file (`R_LoadFont`).
pub fn parse(bytes: &[u8]) -> Result<Font> {
    ensure!(bytes.len() >= HEADER, "font file is {} bytes", bytes.len());
    let u32_at = |o: usize| u32::from_le_bytes(bytes[o..o + 4].try_into().unwrap());
    let f32_at = |o: usize| f32::from_le_bytes(bytes[o..o + 4].try_into().unwrap());
    let cstr = |o: usize| -> Result<String> {
        let tail = bytes.get(o..).unwrap_or_default();
        let Some(end) = tail.iter().position(|&b| b == 0) else {
            bail!("unterminated string at {o:#x}");
        };
        Ok(String::from_utf8_lossy(&tail[..end]).into_owned())
    };
    let name = cstr(u32_at(0) as usize)?;
    let pixel_height = u32_at(4) as i32;
    let count = u32_at(8) as usize;
    let material = cstr(u32_at(12) as usize)?;
    ensure!(count > MISSING, "{name}: {count} glyphs");
    ensure!(
        bytes.len() >= HEADER + count * GLYPH,
        "{name}: {count} glyphs past the end of {} bytes",
        bytes.len()
    );
    let glyphs: Vec<Glyph> = (0..count)
        .map(|i| {
            let o = HEADER + i * GLYPH;
            Glyph {
                letter: u16::from_le_bytes([bytes[o], bytes[o + 1]]),
                x0: bytes[o + 2] as i8,
                y0: bytes[o + 3] as i8,
                dx: bytes[o + 4],
                pixel_width: bytes[o + 5],
                pixel_height: bytes[o + 6],
                s0: f32_at(o + 8),
                t0: f32_at(o + 12),
                s1: f32_at(o + 16),
                t1: f32_at(o + 20),
            }
        })
        .collect();
    for (i, g) in glyphs.iter().take(ASCII_RUN).enumerate() {
        ensure!(
            usize::from(g.letter) == 0x20 + i,
            "{name}: glyph {i} is {:#x}, not {:#x}",
            g.letter,
            0x20 + i
        );
    }
    Ok(Font {
        name,
        pixel_height,
        material,
        glyphs,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A font of the 96 ASCII glyphs plus é, laid out as the files are.
    fn sample() -> Vec<u8> {
        let count = ASCII_RUN + 1;
        let strings = HEADER + count * GLYPH;
        let mut b = Vec::new();
        b.extend((strings as u32).to_le_bytes());
        b.extend(16u32.to_le_bytes());
        b.extend((count as u32).to_le_bytes());
        b.extend((strings as u32 + 11).to_le_bytes());
        for i in 0..count {
            let letter = if i < ASCII_RUN { 0x20 + i as u16 } else { 0xe9 };
            b.extend(letter.to_le_bytes());
            b.extend([1u8, (-11i8) as u8, 7, 5, 11, 0]);
            for f in [0.25f32, 0.5, 0.75, 1.0] {
                b.extend(f.to_le_bytes());
            }
        }
        b.extend(b"fonts/test\0fonts/gamefonts\0");
        b
    }

    #[test]
    fn parses_the_header_and_glyphs() {
        let f = parse(&sample()).unwrap();
        assert_eq!(f.name, "fonts/test");
        assert_eq!(f.material, "fonts/gamefonts");
        assert_eq!(f.pixel_height, 16);
        let a = f.glyph('A' as u32);
        assert_eq!((a.letter, a.x0, a.y0, a.dx), (b'A' as u16, 1, -11, 7));
        assert_eq!((a.pixel_width, a.pixel_height), (5, 11));
        assert_eq!([a.s0, a.t0, a.s1, a.t1], [0.25, 0.5, 0.75, 1.0]);
        assert_eq!(f.glyph(0xe9).letter, 0xe9);
        // A letter the font lacks draws its `.`.
        assert_eq!(f.glyph(0x416).letter, b'.' as u16);
    }

    #[test]
    fn refuses_a_short_file() {
        assert!(parse(&[0; 8]).is_err());
        let mut b = sample();
        b.truncate(HEADER + 10 * GLYPH);
        assert!(parse(&b).is_err());
    }
}
