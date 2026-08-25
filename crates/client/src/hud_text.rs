//! Debug HUD text: an embedded 8x8 ASCII bitmap font (`font8x8` by Daniel
//! Hepper, public domain) and the layout that turns lines into glyph quads.

pub const GLYPH_W: usize = 8;
pub const GLYPH_H: usize = 8;
/// 96 printable ASCII glyphs (0x20..0x7F) in a 16x6 grid.
pub const ATLAS_COLS: usize = 16;
pub const ATLAS_ROWS: usize = 6;
pub const ATLAS_W: usize = ATLAS_COLS * GLYPH_W;
pub const ATLAS_H: usize = ATLAS_ROWS * GLYPH_H;

/// Edge margin, physical pixels.
pub const PAD: f32 = 8.0;
/// Glyph pixels, scaled at layout time.
const LINE_ADVANCE: f32 = (GLYPH_H + 2) as f32;

pub const TEXT_COLOR: [f32; 4] = [1.0, 1.0, 1.0, 1.0];
pub const SHADOW_COLOR: [f32; 4] = [0.0, 0.0, 0.0, 0.8];

/// `pos` is clip space, `uv` indexes the atlas, `color` tints the R8 coverage.
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
pub struct HudVert {
    pub pos: [f32; 2],
    pub uv: [f32; 2],
    pub color: [f32; 4],
}

/// One byte per row, LSB = leftmost pixel.
const FONT: [[u8; 8]; 96] = [
    [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], // U+0020 (space)
    [0x18, 0x3C, 0x3C, 0x18, 0x18, 0x00, 0x18, 0x00], // U+0021 (!)
    [0x36, 0x36, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], // U+0022 (")
    [0x36, 0x36, 0x7F, 0x36, 0x7F, 0x36, 0x36, 0x00], // U+0023 (#)
    [0x0C, 0x3E, 0x03, 0x1E, 0x30, 0x1F, 0x0C, 0x00], // U+0024 ($)
    [0x00, 0x63, 0x33, 0x18, 0x0C, 0x66, 0x63, 0x00], // U+0025 (%)
    [0x1C, 0x36, 0x1C, 0x6E, 0x3B, 0x33, 0x6E, 0x00], // U+0026 (&)
    [0x06, 0x06, 0x03, 0x00, 0x00, 0x00, 0x00, 0x00], // U+0027 (')
    [0x18, 0x0C, 0x06, 0x06, 0x06, 0x0C, 0x18, 0x00], // U+0028 (()
    [0x06, 0x0C, 0x18, 0x18, 0x18, 0x0C, 0x06, 0x00], // U+0029 ()
    [0x00, 0x66, 0x3C, 0xFF, 0x3C, 0x66, 0x00, 0x00], // U+002A (*)
    [0x00, 0x0C, 0x0C, 0x3F, 0x0C, 0x0C, 0x00, 0x00], // U+002B (+)
    [0x00, 0x00, 0x00, 0x00, 0x00, 0x0C, 0x0C, 0x06], // U+002C (,)
    [0x00, 0x00, 0x00, 0x3F, 0x00, 0x00, 0x00, 0x00], // U+002D (-)
    [0x00, 0x00, 0x00, 0x00, 0x00, 0x0C, 0x0C, 0x00], // U+002E (.)
    [0x60, 0x30, 0x18, 0x0C, 0x06, 0x03, 0x01, 0x00], // U+002F (/)
    [0x3E, 0x63, 0x73, 0x7B, 0x6F, 0x67, 0x3E, 0x00], // U+0030 (0)
    [0x0C, 0x0E, 0x0C, 0x0C, 0x0C, 0x0C, 0x3F, 0x00], // U+0031 (1)
    [0x1E, 0x33, 0x30, 0x1C, 0x06, 0x33, 0x3F, 0x00], // U+0032 (2)
    [0x1E, 0x33, 0x30, 0x1C, 0x30, 0x33, 0x1E, 0x00], // U+0033 (3)
    [0x38, 0x3C, 0x36, 0x33, 0x7F, 0x30, 0x78, 0x00], // U+0034 (4)
    [0x3F, 0x03, 0x1F, 0x30, 0x30, 0x33, 0x1E, 0x00], // U+0035 (5)
    [0x1C, 0x06, 0x03, 0x1F, 0x33, 0x33, 0x1E, 0x00], // U+0036 (6)
    [0x3F, 0x33, 0x30, 0x18, 0x0C, 0x0C, 0x0C, 0x00], // U+0037 (7)
    [0x1E, 0x33, 0x33, 0x1E, 0x33, 0x33, 0x1E, 0x00], // U+0038 (8)
    [0x1E, 0x33, 0x33, 0x3E, 0x30, 0x18, 0x0E, 0x00], // U+0039 (9)
    [0x00, 0x0C, 0x0C, 0x00, 0x00, 0x0C, 0x0C, 0x00], // U+003A (:)
    [0x00, 0x0C, 0x0C, 0x00, 0x00, 0x0C, 0x0C, 0x06], // U+003B (;)
    [0x18, 0x0C, 0x06, 0x03, 0x06, 0x0C, 0x18, 0x00], // U+003C (<)
    [0x00, 0x00, 0x3F, 0x00, 0x00, 0x3F, 0x00, 0x00], // U+003D (=)
    [0x06, 0x0C, 0x18, 0x30, 0x18, 0x0C, 0x06, 0x00], // U+003E (>)
    [0x1E, 0x33, 0x30, 0x18, 0x0C, 0x00, 0x0C, 0x00], // U+003F (?)
    [0x3E, 0x63, 0x7B, 0x7B, 0x7B, 0x03, 0x1E, 0x00], // U+0040 (@)
    [0x0C, 0x1E, 0x33, 0x33, 0x3F, 0x33, 0x33, 0x00], // U+0041 (A)
    [0x3F, 0x66, 0x66, 0x3E, 0x66, 0x66, 0x3F, 0x00], // U+0042 (B)
    [0x3C, 0x66, 0x03, 0x03, 0x03, 0x66, 0x3C, 0x00], // U+0043 (C)
    [0x1F, 0x36, 0x66, 0x66, 0x66, 0x36, 0x1F, 0x00], // U+0044 (D)
    [0x7F, 0x46, 0x16, 0x1E, 0x16, 0x46, 0x7F, 0x00], // U+0045 (E)
    [0x7F, 0x46, 0x16, 0x1E, 0x16, 0x06, 0x0F, 0x00], // U+0046 (F)
    [0x3C, 0x66, 0x03, 0x03, 0x73, 0x66, 0x7C, 0x00], // U+0047 (G)
    [0x33, 0x33, 0x33, 0x3F, 0x33, 0x33, 0x33, 0x00], // U+0048 (H)
    [0x1E, 0x0C, 0x0C, 0x0C, 0x0C, 0x0C, 0x1E, 0x00], // U+0049 (I)
    [0x78, 0x30, 0x30, 0x30, 0x33, 0x33, 0x1E, 0x00], // U+004A (J)
    [0x67, 0x66, 0x36, 0x1E, 0x36, 0x66, 0x67, 0x00], // U+004B (K)
    [0x0F, 0x06, 0x06, 0x06, 0x46, 0x66, 0x7F, 0x00], // U+004C (L)
    [0x63, 0x77, 0x7F, 0x7F, 0x6B, 0x63, 0x63, 0x00], // U+004D (M)
    [0x63, 0x67, 0x6F, 0x7B, 0x73, 0x63, 0x63, 0x00], // U+004E (N)
    [0x1C, 0x36, 0x63, 0x63, 0x63, 0x36, 0x1C, 0x00], // U+004F (O)
    [0x3F, 0x66, 0x66, 0x3E, 0x06, 0x06, 0x0F, 0x00], // U+0050 (P)
    [0x1E, 0x33, 0x33, 0x33, 0x3B, 0x1E, 0x38, 0x00], // U+0051 (Q)
    [0x3F, 0x66, 0x66, 0x3E, 0x36, 0x66, 0x67, 0x00], // U+0052 (R)
    [0x1E, 0x33, 0x07, 0x0E, 0x38, 0x33, 0x1E, 0x00], // U+0053 (S)
    [0x3F, 0x2D, 0x0C, 0x0C, 0x0C, 0x0C, 0x1E, 0x00], // U+0054 (T)
    [0x33, 0x33, 0x33, 0x33, 0x33, 0x33, 0x3F, 0x00], // U+0055 (U)
    [0x33, 0x33, 0x33, 0x33, 0x33, 0x1E, 0x0C, 0x00], // U+0056 (V)
    [0x63, 0x63, 0x63, 0x6B, 0x7F, 0x77, 0x63, 0x00], // U+0057 (W)
    [0x63, 0x63, 0x36, 0x1C, 0x1C, 0x36, 0x63, 0x00], // U+0058 (X)
    [0x33, 0x33, 0x33, 0x1E, 0x0C, 0x0C, 0x1E, 0x00], // U+0059 (Y)
    [0x7F, 0x63, 0x31, 0x18, 0x4C, 0x66, 0x7F, 0x00], // U+005A (Z)
    [0x1E, 0x06, 0x06, 0x06, 0x06, 0x06, 0x1E, 0x00], // U+005B ([)
    [0x03, 0x06, 0x0C, 0x18, 0x30, 0x60, 0x40, 0x00], // U+005C (\)
    [0x1E, 0x18, 0x18, 0x18, 0x18, 0x18, 0x1E, 0x00], // U+005D (])
    [0x08, 0x1C, 0x36, 0x63, 0x00, 0x00, 0x00, 0x00], // U+005E (^)
    [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xFF], // U+005F (_)
    [0x0C, 0x0C, 0x18, 0x00, 0x00, 0x00, 0x00, 0x00], // U+0060 (`)
    [0x00, 0x00, 0x1E, 0x30, 0x3E, 0x33, 0x6E, 0x00], // U+0061 (a)
    [0x07, 0x06, 0x06, 0x3E, 0x66, 0x66, 0x3B, 0x00], // U+0062 (b)
    [0x00, 0x00, 0x1E, 0x33, 0x03, 0x33, 0x1E, 0x00], // U+0063 (c)
    [0x38, 0x30, 0x30, 0x3e, 0x33, 0x33, 0x6E, 0x00], // U+0064 (d)
    [0x00, 0x00, 0x1E, 0x33, 0x3f, 0x03, 0x1E, 0x00], // U+0065 (e)
    [0x1C, 0x36, 0x06, 0x0f, 0x06, 0x06, 0x0F, 0x00], // U+0066 (f)
    [0x00, 0x00, 0x6E, 0x33, 0x33, 0x3E, 0x30, 0x1F], // U+0067 (g)
    [0x07, 0x06, 0x36, 0x6E, 0x66, 0x66, 0x67, 0x00], // U+0068 (h)
    [0x0C, 0x00, 0x0E, 0x0C, 0x0C, 0x0C, 0x1E, 0x00], // U+0069 (i)
    [0x30, 0x00, 0x30, 0x30, 0x30, 0x33, 0x33, 0x1E], // U+006A (j)
    [0x07, 0x06, 0x66, 0x36, 0x1E, 0x36, 0x67, 0x00], // U+006B (k)
    [0x0E, 0x0C, 0x0C, 0x0C, 0x0C, 0x0C, 0x1E, 0x00], // U+006C (l)
    [0x00, 0x00, 0x33, 0x7F, 0x7F, 0x6B, 0x63, 0x00], // U+006D (m)
    [0x00, 0x00, 0x1F, 0x33, 0x33, 0x33, 0x33, 0x00], // U+006E (n)
    [0x00, 0x00, 0x1E, 0x33, 0x33, 0x33, 0x1E, 0x00], // U+006F (o)
    [0x00, 0x00, 0x3B, 0x66, 0x66, 0x3E, 0x06, 0x0F], // U+0070 (p)
    [0x00, 0x00, 0x6E, 0x33, 0x33, 0x3E, 0x30, 0x78], // U+0071 (q)
    [0x00, 0x00, 0x3B, 0x6E, 0x66, 0x06, 0x0F, 0x00], // U+0072 (r)
    [0x00, 0x00, 0x3E, 0x03, 0x1E, 0x30, 0x1F, 0x00], // U+0073 (s)
    [0x08, 0x0C, 0x3E, 0x0C, 0x0C, 0x2C, 0x18, 0x00], // U+0074 (t)
    [0x00, 0x00, 0x33, 0x33, 0x33, 0x33, 0x6E, 0x00], // U+0075 (u)
    [0x00, 0x00, 0x33, 0x33, 0x33, 0x1E, 0x0C, 0x00], // U+0076 (v)
    [0x00, 0x00, 0x63, 0x6B, 0x7F, 0x7F, 0x36, 0x00], // U+0077 (w)
    [0x00, 0x00, 0x63, 0x36, 0x1C, 0x36, 0x63, 0x00], // U+0078 (x)
    [0x00, 0x00, 0x33, 0x33, 0x33, 0x3E, 0x30, 0x1F], // U+0079 (y)
    [0x00, 0x00, 0x3F, 0x19, 0x0C, 0x26, 0x3F, 0x00], // U+007A (z)
    [0x38, 0x0C, 0x0C, 0x07, 0x0C, 0x0C, 0x38, 0x00], // U+007B ({)
    [0x18, 0x18, 0x18, 0x00, 0x18, 0x18, 0x18, 0x00], // U+007C (|)
    [0x07, 0x0C, 0x0C, 0x38, 0x0C, 0x0C, 0x07, 0x00], // U+007D (})
    [0x6E, 0x3B, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], // U+007E (~)
    [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], // U+007F
];

/// R8 atlas, 0 or 255 per pixel, glyphs row-major from ASCII 0x20.
pub fn build_atlas() -> Vec<u8> {
    let mut atlas = vec![0u8; ATLAS_W * ATLAS_H];
    for (idx, glyph) in FONT.iter().enumerate() {
        let (cx, cy) = (idx % ATLAS_COLS * GLYPH_W, idx / ATLAS_COLS * GLYPH_H);
        for (y, row) in glyph.iter().enumerate() {
            for x in 0..GLYPH_W {
                if row >> x & 1 != 0 {
                    atlas[(cy + y) * ATLAS_W + cx + x] = 255;
                }
            }
        }
    }
    atlas
}

/// Right-aligned at the top-right corner, `PAD` from both edges, glyphs at
/// `scale`x. Four vertices per glyph in the renderer's static quad order,
/// shadow layer first. Spaces emit nothing; non-printable bytes draw as '?'.
pub fn layout_lines(lines: &[String], screen_w: f32, screen_h: f32, scale: f32) -> Vec<HudVert> {
    let (gw, gh) = (GLYPH_W as f32 * scale, GLYPH_H as f32 * scale);
    // (pixel x, pixel y, glyph index) of every drawn glyph
    let mut glyphs = Vec::new();
    for (li, line) in lines.iter().enumerate() {
        let y = PAD + li as f32 * LINE_ADVANCE * scale;
        let right = screen_w - PAD;
        let n = line.len() as f32;
        for (ci, byte) in line.bytes().enumerate() {
            if byte == b' ' {
                continue;
            }
            let idx = if (0x20..0x7f).contains(&byte) {
                (byte - 0x20) as usize
            } else {
                (b'?' - 0x20) as usize
            };
            glyphs.push((right - (n - ci as f32) * gw, y, idx));
        }
    }

    let mut verts = Vec::with_capacity(glyphs.len() * 8);
    let mut quads = |offset: f32, color: [f32; 4]| {
        for &(x, y, idx) in &glyphs {
            let (px, py) = (x + offset, y + offset);
            let u0 = (idx % ATLAS_COLS * GLYPH_W) as f32 / ATLAS_W as f32;
            let v0 = (idx / ATLAS_COLS * GLYPH_H) as f32 / ATLAS_H as f32;
            let (u1, v1) = (
                u0 + GLYPH_W as f32 / ATLAS_W as f32,
                v0 + GLYPH_H as f32 / ATLAS_H as f32,
            );
            let clip = |px: f32, py: f32| [px / screen_w * 2.0 - 1.0, 1.0 - py / screen_h * 2.0];
            // corner order matches the static [b, b+1, b+2, b, b+2, b+3] quads
            verts.extend([
                HudVert {
                    pos: clip(px, py),
                    uv: [u0, v0],
                    color,
                },
                HudVert {
                    pos: clip(px + gw, py),
                    uv: [u1, v0],
                    color,
                },
                HudVert {
                    pos: clip(px + gw, py + gh),
                    uv: [u1, v1],
                    color,
                },
                HudVert {
                    pos: clip(px, py + gh),
                    uv: [u0, v1],
                    color,
                },
            ]);
        }
    };
    quads(scale, SHADOW_COLOR);
    quads(0.0, TEXT_COLOR);
    verts
}

/// Frame-time smoothing and per-second rates for the debug overlay, published
/// once per [`Self::WINDOW`] so the readout holds still.
pub struct HudStats {
    /// Seconds, 0 until the first frame.
    pub dt_smooth: f32,
    win_time: f32,
    win_worst: f32,
    last_restarts: u64,
    last_misses: u64,
    /// Published once per window:
    pub worst_ms: f32,
    pub restarts_per_s: f32,
    pub misses_per_s: f32,
}

impl HudStats {
    /// Publishing period, seconds.
    const WINDOW: f32 = 0.5;

    pub fn new() -> HudStats {
        HudStats {
            dt_smooth: 0.0,
            win_time: 0.0,
            win_worst: 0.0,
            last_restarts: 0,
            last_misses: 0,
            worst_ms: 0.0,
            restarts_per_s: 0.0,
            misses_per_s: 0.0,
        }
    }

    /// `restarts` and `misses` are cumulative; the rates are their growth over
    /// the window.
    pub fn frame(&mut self, dt: f32, restarts: u64, misses: u64) {
        self.dt_smooth = if self.dt_smooth == 0.0 {
            dt
        } else {
            self.dt_smooth * 0.95 + dt * 0.05
        };
        self.win_time += dt;
        self.win_worst = self.win_worst.max(dt);
        if self.win_time >= Self::WINDOW {
            self.worst_ms = self.win_worst * 1000.0;
            self.restarts_per_s =
                restarts.saturating_sub(self.last_restarts) as f32 / self.win_time;
            self.misses_per_s = misses.saturating_sub(self.last_misses) as f32 / self.win_time;
            self.last_restarts = restarts;
            self.last_misses = misses;
            self.win_time = 0.0;
            self.win_worst = 0.0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stats_publish_worst_frame_and_rates_per_window() {
        let mut s = HudStats::new();
        // 0.4 s of 10 ms frames leaves the window open
        let mut restarts = 0u64;
        for _ in 0..40 {
            s.frame(0.010, restarts, 0);
        }
        assert_eq!(s.worst_ms, 0.0);
        // one 50 ms spike and 5 restarts close the window past 0.5 s
        restarts += 5;
        for _ in 0..9 {
            s.frame(0.010, restarts, 0);
        }
        s.frame(0.050, restarts, 0);
        assert!((s.worst_ms - 50.0).abs() < 0.01, "worst {}", s.worst_ms);
        assert!((s.restarts_per_s - 5.0 / 0.54).abs() < 0.5);
        assert!((s.dt_smooth - 0.010).abs() < 0.005);
        for _ in 0..51 {
            s.frame(0.010, restarts, 0);
        }
        assert!((s.worst_ms - 10.0).abs() < 0.01);
        assert_eq!(s.restarts_per_s, 0.0);
    }

    /// Pixel-space (min_x, min_y, max_x, max_y) of the quads with `color`.
    fn pixel_bounds(verts: &[HudVert], color: [f32; 4], w: f32, h: f32) -> (f32, f32, f32, f32) {
        let mut b = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        for v in verts.iter().filter(|v| v.color == color) {
            let px = (v.pos[0] + 1.0) * 0.5 * w;
            let py = (1.0 - v.pos[1]) * 0.5 * h;
            b = (b.0.min(px), b.1.min(py), b.2.max(px), b.3.max(py));
        }
        b
    }

    #[test]
    fn atlas_has_ink_for_glyphs_and_none_for_space() {
        let atlas = build_atlas();
        assert_eq!(atlas.len(), ATLAS_W * ATLAS_H);
        // pixels of one glyph cell, `idx` counted from ASCII 0x20
        let cell = |idx: usize| {
            let (cx, cy) = (idx % ATLAS_COLS * GLYPH_W, idx / ATLAS_COLS * GLYPH_H);
            (0..GLYPH_H).flat_map(move |y| (0..GLYPH_W).map(move |x| (cx + x, (cy + y) * ATLAS_W)))
        };
        let ink = |idx: usize| cell(idx).filter(|&(x, row)| atlas[row + x] != 0).count();
        assert_eq!(ink(0), 0, "space must be blank");
        assert!(ink(b'A' as usize - 0x20) > 0, "'A' must have pixels");
        assert!(ink(b'0' as usize - 0x20) > 0, "'0' must have pixels");
        assert!(atlas.iter().all(|&p| p == 0 || p == 255));
    }

    #[test]
    fn layout_right_aligns_each_line_and_stacks_downward() {
        let lines = vec!["FPS 60".to_string(), "abc".to_string()];
        let (w, h, scale) = (1600.0, 900.0, 2.0);
        let verts = layout_lines(&lines, w, h, scale);
        let (min_x, min_y, max_x, max_y) = pixel_bounds(&verts, TEXT_COLOR, w, h);
        assert!((max_x - (w - PAD)).abs() < 0.01, "right edge at {max_x}");
        // the 6-glyph line sets the left extent
        let expect_left = w - PAD - 6.0 * GLYPH_W as f32 * scale;
        assert!((min_x - expect_left).abs() < 0.01, "left edge at {min_x}");
        assert!((min_y - PAD).abs() < 0.01, "top edge at {min_y}");
        let expect_bottom = PAD + LINE_ADVANCE * scale + GLYPH_H as f32 * scale;
        assert!(
            (max_y - expect_bottom).abs() < 0.01,
            "bottom edge at {max_y}"
        );
    }

    #[test]
    fn layout_emits_shadow_offset_from_text_and_skips_spaces() {
        let lines = vec!["a b".to_string()];
        let (w, h, scale) = (800.0, 600.0, 1.0);
        let verts = layout_lines(&lines, w, h, scale);
        // 2 drawn glyphs (space skipped) x 2 layers x 4 corners
        assert_eq!(verts.len(), 16);
        let text = pixel_bounds(&verts, TEXT_COLOR, w, h);
        let shadow = pixel_bounds(&verts, SHADOW_COLOR, w, h);
        assert!((shadow.0 - (text.0 + scale)).abs() < 0.01);
        assert!((shadow.1 - (text.1 + scale)).abs() < 0.01);
        assert_eq!(verts[0].color, SHADOW_COLOR);
    }

    #[test]
    fn layout_uvs_select_the_glyph_cell() {
        // '!' is glyph index 1: u in [8/128, 16/128), v in [0, 8/48)
        let verts = layout_lines(&["!".to_string()], 800.0, 600.0, 1.0);
        let us: Vec<f32> = verts
            .iter()
            .filter(|v| v.color == TEXT_COLOR)
            .map(|v| v.uv[0])
            .collect();
        let vs: Vec<f32> = verts
            .iter()
            .filter(|v| v.color == TEXT_COLOR)
            .map(|v| v.uv[1])
            .collect();
        let (u0, u1) = (
            GLYPH_W as f32 / ATLAS_W as f32,
            2.0 * GLYPH_W as f32 / ATLAS_W as f32,
        );
        let v1 = GLYPH_H as f32 / ATLAS_H as f32;
        assert!(us
            .iter()
            .all(|&u| (u - u0).abs() < 1e-6 || (u - u1).abs() < 1e-6));
        assert!(vs.iter().all(|&v| v.abs() < 1e-6 || (v - v1).abs() < 1e-6));
    }
}
