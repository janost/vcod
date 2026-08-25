//! Q3-style bitmap font loader and CPU text layout for the HUD.
//! File layout: docs/research/cod11-hud-protocol.md, section 6. Q3's `top`
//! slot (+4) holds the glyph width here, so there is no vertical bearing;
//! baseline alignment uses `height` against `Font::max_height`.
//!
//! Gotcha: lowercase p/v/w/x/y draw as their uppercase shapes. The five
//! records point at distinct UV rects, but the shipped atlas art in those
//! cells is the uppercase letterform (1.1 and 1.5 pak5, sizes 12/16/24).
//! The parser is faithful; there is no correct art to substitute.

use super::HudQuad;
use vcod_common::pk3::Pk3Fs;

/// One 80-byte glyph record (doc section 6).
#[derive(Debug, Clone, Copy)]
pub struct Glyph {
    /// +0, px.
    pub height: i32,
    /// +4, px. Q3's `top` slot; not a vertical bearing.
    #[allow(dead_code)]
    pub width: i32,
    /// +8, always `height + 1`.
    #[allow(dead_code)]
    pub height_f: f32,
    /// +12, horizontal bearing, design units.
    pub bearing: f32,
    /// +16, horizontal advance, design units.
    pub advance: f32,
    pub image_width: i32,
    pub image_height: i32,
    pub s: f32,
    pub t: f32,
    pub s2: f32,
    pub t2: f32,
}

pub struct Font {
    /// Requested point size; the cap height in window px at `scale == 1.0`.
    pub size: u32,
    pub glyphs: Vec<Glyph>, // 256 entries, indexed by byte
    pub glyph_scale: f32,
    /// Newline advance, design units (the second header float).
    pub line_advance: f32,
    /// Max `height` over 0x20..=0x7e. A quad top at `(max_height - height) * s`
    /// puts every glyph bottom on one baseline, since `image_height == height`.
    pub max_height: i32,
    pub page: String, // "fonts/fontImage_0_<size>"
}

impl Font {
    /// Design units to window px, so the tallest printable glyph is `size` px
    /// at `scale == 1.0`. Raw `glyph_scale` alone renders about 3x too big.
    pub fn unit_scale(&self) -> f32 {
        self.size as f32 / (self.max_height as f32 * self.glyph_scale)
    }

    /// Line spacing in window px at `scale`.
    pub fn line_height(&self, scale: f32) -> f32 {
        self.line_advance * self.unit_scale() * scale
    }
}

const RECORD_SIZE: usize = 80;
const GLYPH_COUNT: usize = 256;
const GLYPH_BLOCK: usize = RECORD_SIZE * GLYPH_COUNT; // 20480
const FILE_SIZE: usize = GLYPH_BLOCK + 4 + 4 + 64; // 20552

pub fn parse_font_dat(bytes: &[u8], size: u32) -> Result<Font, String> {
    if bytes.len() != FILE_SIZE {
        return Err(format!(
            "bad fontImage_{size}.dat: {} bytes (want {FILE_SIZE})",
            bytes.len()
        ));
    }

    let i32_at = |o: usize| i32::from_le_bytes(bytes[o..o + 4].try_into().unwrap());
    let f32_at = |o: usize| f32::from_le_bytes(bytes[o..o + 4].try_into().unwrap());

    let mut glyphs = Vec::with_capacity(GLYPH_COUNT);
    for i in 0..GLYPH_COUNT {
        let o = i * RECORD_SIZE;
        glyphs.push(Glyph {
            height: i32_at(o),
            width: i32_at(o + 4),
            height_f: f32_at(o + 8),
            bearing: f32_at(o + 12),
            advance: f32_at(o + 16),
            image_width: i32_at(o + 20),
            image_height: i32_at(o + 24),
            s: f32_at(o + 28),
            t: f32_at(o + 32),
            s2: f32_at(o + 36),
            t2: f32_at(o + 40),
            // +44 glyph handle, +48..+80 shader name: skipped.
        });
    }

    let glyph_scale = f32_at(GLYPH_BLOCK);
    let line_advance = f32_at(GLYPH_BLOCK + 4);
    let max_height = (0x20u8..=0x7e)
        .map(|b| glyphs[b as usize].height)
        .max()
        .unwrap_or(0);

    Ok(Font {
        size,
        glyphs,
        glyph_scale,
        line_advance,
        max_height,
        page: format!("fonts/fontImage_0_{size}"),
    })
}

pub fn load_font(fs: &Pk3Fs, size: u32) -> Result<Font, String> {
    let name = format!("fonts/fontImage_{size}.dat");
    let bytes = fs.read(&name).ok_or_else(|| format!("missing {name}"))?;
    parse_font_dat(&bytes, size)
}

/// The engine's `colorTable` for `^0`..`^7` (doc section 7, CoDMP.exe
/// `0x004d7f13`). Only these eight codes exist. `^7` restores the caller's
/// colour rather than reading entry 7; [`split_color_codes`] does the same.
pub const COLORS: [[f32; 4]; 8] = [
    [0.0, 0.0, 0.0, 1.0], // ^0 black
    [1.0, 0.0, 0.0, 1.0], // ^1 red
    [0.0, 1.0, 0.0, 1.0], // ^2 green
    [1.0, 1.0, 0.0, 1.0], // ^3 yellow
    [0.0, 0.0, 1.0, 1.0], // ^4 blue
    [0.0, 1.0, 1.0, 1.0], // ^5 cyan
    [1.0, 0.0, 1.0, 1.0], // ^6 magenta
    [1.0, 1.0, 1.0, 1.0], // ^7 white; the renderer restores the caller's colour instead
];

/// Splits `"^1Bob^7: hi"` into `[("Bob", red), (": hi", default)]`. `^7`
/// restores `default`, not `COLORS[7]`. `^8`, `^9`, `^^` and a lone `^` are
/// literal (doc section 7).
pub fn split_color_codes(text: &str, default: [f32; 4]) -> Vec<(String, [f32; 4])> {
    let mut out = Vec::new();
    let mut buf = String::new();
    let mut color = default;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '^' {
            match chars.peek() {
                Some(d) if ('0'..='7').contains(d) => {
                    let idx = d.to_digit(10).unwrap() as usize;
                    chars.next();
                    if !buf.is_empty() {
                        out.push((std::mem::take(&mut buf), color));
                    }
                    color = if idx == 7 { default } else { COLORS[idx] };
                    continue;
                }
                Some('^') => {
                    buf.push('^');
                    chars.next();
                    continue;
                }
                _ => {
                    buf.push('^');
                    continue;
                }
            }
        }
        buf.push(c);
    }
    if !buf.is_empty() {
        out.push((buf, color));
    }
    out
}

/// `text` with its `^N` codes removed.
#[allow(dead_code)] // only caller outside tests is `measure`, below
fn strip_color_codes(text: &str) -> String {
    split_color_codes(text, [0.0; 4])
        .into_iter()
        .map(|(s, _)| s)
        .collect()
}

/// Width of `text` in window px at `scale`, colour codes excluded.
pub fn measure(font: &Font, text: &str, scale: f32) -> f32 {
    let s = font.glyph_scale * font.unit_scale() * scale;
    strip_color_codes(text)
        .chars()
        .map(|c| font.glyphs[glyph_index(c)].advance * s)
        .sum()
}

/// Names decode as latin-1 (`ClientState::name`), so every byte has its own
/// record; anything above 0xFF becomes `?`.
fn glyph_index(c: char) -> usize {
    let cp = c as u32;
    if cp <= 0xFF {
        cp as usize
    } else {
        b'?' as usize
    }
}

#[allow(clippy::too_many_arguments)]
fn push_quad(
    out: &mut Vec<HudQuad>,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    g: &Glyph,
    rgba: [f32; 4],
    texture: &str,
) {
    out.push(HudQuad {
        verts: [[x, y], [x + w, y], [x + w, y + h], [x, y + h]],
        uvs: [[g.s, g.t], [g.s2, g.t], [g.s2, g.t2], [g.s, g.t2]],
        rgba,
        texture: texture.to_string(),
    });
}

/// Lays out `text` with its top-left at (x, y): a 1px black shadow quad, then
/// the glyph quad, per char. Returns the advance in window px.
pub fn layout(
    font: &Font,
    text: &str,
    x: f32,
    y: f32,
    scale: f32,
    color: [f32; 4],
    out: &mut Vec<HudQuad>,
) -> f32 {
    let s = font.glyph_scale * font.unit_scale() * scale;
    let mut cursor = x;
    for (seg, seg_color) in split_color_codes(text, color) {
        for c in seg.chars() {
            let g = &font.glyphs[glyph_index(c)];
            let w = g.image_width as f32 * s;
            let h = g.image_height as f32 * s;
            let gx = cursor + g.bearing * s;
            let gy = y + (font.max_height - g.height) as f32 * s;

            push_quad(
                out,
                gx + 1.0,
                gy + 1.0,
                w,
                h,
                g,
                [0.0, 0.0, 0.0, 0.8],
                &font.page,
            );
            push_quad(out, gx, gy, w, h, g, seg_color, &font.page);

            cursor += g.advance * s;
        }
    }
    cursor - x
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_color_codes() {
        // Non-white default, so a ^7 that looked up COLORS[7] would show.
        let default = COLORS[2]; // green
        let out = split_color_codes("^1Bob^7: hi", default);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].0, "Bob");
        assert_eq!(out[0].1, COLORS[1]);
        assert_eq!(out[1], (": hi".to_string(), default));
        assert_eq!(split_color_codes("a^", default)[0].0, "a^");
        assert_eq!(split_color_codes("^8x", default)[0].0, "^8x");
    }

    #[test]
    fn latin1_char_measures_as_one_glyph_not_two_question_marks() {
        // 0xE9 ('é') arrives as one latin-1 char and must hit record 0xE9.
        let Some(fs) = vcod_common::testing::game_fs() else {
            return;
        };
        let f = load_font(&fs, 16).unwrap();
        let e_acute = '\u{00E9}'; // é
        assert_eq!(glyph_index(e_acute), 0xE9);
        let one_glyph_width = f.glyphs[0xE9].advance;
        let two_q_width = f.glyphs[b'?' as usize].advance * 2.0;
        let s = font_scale_unit(&f);
        assert!((measure(&f, "é", 1.0) - one_glyph_width * s).abs() < 1e-3);
        assert!(
            (measure(&f, "é", 1.0) - two_q_width * s).abs() > 1e-3,
            "should not measure as two '?' glyphs"
        );
        assert_eq!(glyph_index('\u{1F600}'), b'?' as usize);
    }

    fn font_scale_unit(f: &Font) -> f32 {
        f.glyph_scale * f.unit_scale()
    }

    #[test]
    fn real_font_parses_and_measures() {
        let Some(fs) = vcod_common::testing::game_fs() else {
            return;
        };
        let f = load_font(&fs, 16).unwrap();
        assert_eq!(f.glyphs.len(), 256);
        let a = &f.glyphs[b'A' as usize];
        // Doc section 6's table for 'A'.
        assert_eq!(a.height, 11);
        assert_eq!(a.width, 12);
        assert!((a.advance - 10.667).abs() < 1e-2, "{a:?}");
        assert!((a.bearing - (-0.333)).abs() < 1e-2, "{a:?}");
        assert_eq!(a.image_width, 12);
        assert!(a.s < a.s2 && a.t < a.t2, "{a:?}");
        assert!(measure(&f, "AAA", 1.0) > measure(&f, "A", 1.0));
        assert_eq!(measure(&f, "^1A", 1.0), measure(&f, "A", 1.0));
        assert_eq!(f.page, "fonts/fontImage_0_16");
        // 262.578 design units * unit_scale (16/45).
        assert!(
            (measure(&f, "Hello World", 1.0) - 93.361).abs() < 1e-2,
            "{}",
            measure(&f, "Hello World", 1.0)
        );
    }

    #[test]
    fn lowercase_uv_rects_differ_from_uppercase() {
        // The p/v/y defect is in the atlas art (module doc), not the parser;
        // two records collapsing onto one rect would be a parser regression.
        let Some(fs) = vcod_common::testing::game_fs() else {
            return;
        };
        let f = load_font(&fs, 16).unwrap();
        for (lo, up) in [('p', 'P'), ('v', 'V'), ('y', 'Y')] {
            let l = &f.glyphs[lo as usize];
            let u = &f.glyphs[up as usize];
            assert_ne!(
                (l.s, l.t, l.s2, l.t2),
                (u.s, u.t, u.s2, u.t2),
                "{lo:?}/{up:?} UV rects unexpectedly identical"
            );
        }
    }

    #[test]
    fn unit_scale_normalizes_cap_height_to_font_size() {
        let Some(fs) = vcod_common::testing::game_fs() else {
            return;
        };
        let f = load_font(&fs, 16).unwrap();
        // 16 / (15 * 3), this file's values.
        assert!(
            (f.unit_scale() - (16.0 / 45.0)).abs() < 1e-6,
            "{}",
            f.unit_scale()
        );

        // '(' (height 15) is this font's tallest printable glyph, not 'W' (11).
        assert_eq!(
            f.glyphs[b'(' as usize].height, f.max_height,
            "test assumes '(' is this baked font's tallest printable glyph"
        );
        let mut out = Vec::new();
        layout(&f, "(", 0.0, 0.0, 1.0, COLORS[7], &mut out);
        let cap_height = out[1].verts[2][1] - out[1].verts[0][1]; // glyph quad (index 1) BR.y - TL.y
        assert!((cap_height - 16.0).abs() < 0.01, "{cap_height}");

        // line_advance(45) * unit_scale(16/45) == 16.0.
        assert!(
            (f.line_height(1.0) - 16.0).abs() < 0.01,
            "{}",
            f.line_height(1.0)
        );
        assert!(
            (f.line_height(2.0) - 32.0).abs() < 0.02,
            "{}",
            f.line_height(2.0)
        );
    }

    #[test]
    fn layout_emits_shadow_then_glyph_per_char() {
        let Some(fs) = vcod_common::testing::game_fs() else {
            return;
        };
        let f = load_font(&fs, 16).unwrap();
        let mut out = Vec::new();
        let adv = layout(&f, "Hi", 10.0, 20.0, 1.0, COLORS[7], &mut out);
        assert_eq!(out.len(), 4);
        assert_eq!(out[0].rgba, [0.0, 0.0, 0.0, 0.8]);
        assert_eq!(out[1].rgba, COLORS[7]);
        assert!(out[1].verts[0][0] >= 10.0 && out[1].verts[0][1] >= 20.0);
        assert!((adv - measure(&f, "Hi", 1.0)).abs() < 1e-3);
        assert!(out.iter().all(|q| q.texture == f.page));

        // '.' and 'W' at the same y must share a bottom edge, not a top edge.
        let mut dot = Vec::new();
        layout(&f, ".", 0.0, 0.0, 1.0, COLORS[7], &mut dot);
        let mut w = Vec::new();
        layout(&f, "W", 0.0, 0.0, 1.0, COLORS[7], &mut w);
        let dot_bottom = dot[1].verts[2][1]; // glyph quad (index 1), BR corner
        let w_bottom = w[1].verts[2][1];
        assert!(
            (dot_bottom - w_bottom).abs() < 1e-3,
            "{dot_bottom} vs {w_bottom}"
        );
    }
}
