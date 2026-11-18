//! The script's hudelems (playerstate block 5) laid out on retail's 640x480
//! virtual screen. How the cgame draws one: docs/research/cod11-hud-protocol.md,
//! section 8.

use std::collections::BTreeSet;
use std::sync::Mutex;

use super::HudQuad;
use super::font::{self, Font, Slot, UiFonts};
use vcod_common::localize::Localized;
use vcod_common::net::msg::{HudElem, hud_field as f};

use vcod_common::net::protocol::CsLayout;
#[cfg(test)]
pub use vcod_common::net::protocol::{CS_LOCALIZED_V1, CS_SHADERS_V1};

/// Retail's 640x480 virtual screen, scaled to fit the window and centred,
/// so a wider window pillarboxes it to 4:3 and a narrower one letterboxes
/// it. `spread` is how far an edge-anchored element may move past the box
/// (`Virtual::at`): the pillarbox's width, or 0 with `vc_widescreen 0`
/// (docs/research/cod11-hud-protocol.md, "vcod: wide windows").
#[derive(Clone, Copy, Debug)]
pub struct Virtual {
    pub scale: f32,
    pub x0: f32,
    pub y0: f32,
    /// The window's width in px, what a full-width element stretches over.
    pub w: f32,
    spread: f32,
}

/// Which window edge a HUD element keeps to on a window wider than 4:3.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Anchor {
    Left,
    Centre,
    Right,
}

impl Anchor {
    /// vcod's rule for an element with no fixed side (a hud elem, a mod's
    /// `hud.menu` item): by where its span `x0..x1` on the 640 grid has its
    /// middle, the left third, the middle third or the right third.
    pub fn of_span(x0: f32, x1: f32) -> Anchor {
        let mid = (x0 + x1) * 0.5;
        if mid < 640.0 / 3.0 {
            Anchor::Left
        } else if mid > 1280.0 / 3.0 {
            Anchor::Right
        } else {
            Anchor::Centre
        }
    }
}

/// Whether a span on the 640 grid covers the whole width: a fade, a
/// backdrop, a killcam bar. Such an element stretches over the window.
pub fn full_width(x: f32, w: f32) -> bool {
    x <= 0.0 && x + w >= 640.0
}

const FULL_UV: [[f32; 2]; 4] = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];

/// One axis of [`Virtual::quad_fill`]: the span `at..at + len` of a grid
/// `grid` long, placed at `offset` px with `scale`, or, when it covers the
/// whole grid, stretched over the window's `window` px, overhang kept.
fn stretch(at: f32, len: f32, grid: f32, offset: f32, window: f32, scale: f32) -> (f32, f32) {
    if at <= 0.0 && at + len >= grid {
        (at * scale, window + (at + len - grid) * scale)
    } else {
        (offset + at * scale, offset + (at + len) * scale)
    }
}

impl Virtual {
    /// The grid on a `w` x `h` window, edge anchoring on.
    pub fn new((w, h): (f32, f32)) -> Virtual {
        let scale = (h / 480.0).min(w / 640.0);
        let x0 = (w - 640.0 * scale) / 2.0;
        Virtual {
            scale,
            x0,
            y0: (h - 480.0 * scale) / 2.0,
            w,
            spread: x0.max(0.0),
        }
    }

    /// The same grid with every element kept inside the 4:3 box.
    pub fn boxed(self) -> Virtual {
        Virtual {
            spread: 0.0,
            ..self
        }
    }

    /// [`Virtual::new`], boxed unless `edges`.
    pub fn with_edges(screen: (f32, f32), edges: bool) -> Virtual {
        let v = Virtual::new(screen);
        if edges { v } else { v.boxed() }
    }

    /// The grid moved to keep to `anchor`'s window edge: a left element
    /// draws as far left of the box as the window allows, a right one as
    /// far right.
    pub fn at(self, anchor: Anchor) -> Virtual {
        let x0 = match anchor {
            Anchor::Left => self.x0 - self.spread,
            Anchor::Centre => self.x0,
            Anchor::Right => self.x0 + self.spread,
        };
        Virtual {
            x0,
            spread: 0.0,
            ..self
        }
    }

    pub fn left(self) -> Virtual {
        self.at(Anchor::Left)
    }

    pub fn right(self) -> Virtual {
        self.at(Anchor::Right)
    }

    /// The window's size in px.
    pub fn screen(&self) -> (f32, f32) {
        (self.w, 2.0 * self.y0 + 480.0 * self.scale)
    }

    pub fn point(&self, x: f32, y: f32) -> [f32; 2] {
        [self.x0 + x * self.scale, self.y0 + y * self.scale]
    }

    /// Window px back to the grid, clamped to it: the menu cursor's map.
    pub fn grid_point(&self, px: f32, py: f32) -> [f32; 2] {
        [
            ((px - self.x0) / self.scale).clamp(0.0, 640.0),
            ((py - self.y0) / self.scale).clamp(0.0, 480.0),
        ]
    }

    /// A rect on the grid; one that spans the whole width ([`full_width`])
    /// spans the whole window's width instead, and one that spans the whole
    /// height its height.
    pub fn quad_fill(
        &self,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        rgba: [f32; 4],
        texture: &str,
    ) -> HudQuad {
        let (left, right) = stretch(x, w, 640.0, self.x0, self.w, self.scale);
        let (top, bottom) = stretch(
            y,
            h,
            480.0,
            self.y0,
            2.0 * self.y0 + 480.0 * self.scale,
            self.scale,
        );
        HudQuad {
            verts: [[left, top], [right, top], [right, bottom], [left, bottom]],
            uvs: FULL_UV,
            rgba,
            texture: texture.to_string(),
        }
    }

    pub fn quad(&self, x: f32, y: f32, w: f32, h: f32, rgba: [f32; 4], texture: &str) -> HudQuad {
        self.quad_uv(x, y, w, h, FULL_UV, rgba, texture)
    }

    /// `uvs` in the quad's ring order: TL, TR, BR, BL.
    pub fn quad_uv(
        &self,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        uvs: [[f32; 2]; 4],
        rgba: [f32; 4],
        texture: &str,
    ) -> HudQuad {
        HudQuad {
            verts: [
                self.point(x, y),
                self.point(x + w, y),
                self.point(x + w, y + h),
                self.point(x, y + h),
            ],
            uvs,
            rgba,
            texture: texture.to_string(),
        }
    }

    /// `corners` (TL, TR, BR, BL) relative to the virtual point `(cx, cy)`,
    /// turned `deg` degrees clockwise on screen about it.
    pub fn rotated(
        &self,
        (cx, cy): (f32, f32),
        corners: [[f32; 2]; 4],
        deg: f32,
        rgba: [f32; 4],
        texture: &str,
    ) -> HudQuad {
        let (sin, cos) = deg.to_radians().sin_cos();
        let verts =
            corners.map(|[x, y]| self.point(cx + x * cos - y * sin, cy + x * sin + y * cos));
        HudQuad {
            verts,
            uvs: FULL_UV,
            rgba,
            texture: texture.to_string(),
        }
    }
}

/// Retail's text scale for a font slot (`0.25 * fontScale` for `default`,
/// `fontScale / 3` for the two fixed ones), the height alignment uses, and the
/// fixed slots' cell width in virtual units.
struct TextFont<'a> {
    font: &'a Font,
    retail_scale: f32,
    height: f32,
    cell: Option<f32>,
}

impl<'a> TextFont<'a> {
    fn of(fonts: &'a UiFonts, slot: i32, font_scale: f32, v: &Virtual) -> TextFont<'a> {
        let screen_h = 480.0 * v.scale;
        let fixed = |slot, cell| {
            let retail_scale = font_scale / 3.0;
            TextFont {
                font: fonts.pick(slot, retail_scale, screen_h),
                retail_scale,
                height: 16.0,
                cell: Some(cell),
            }
        };
        match slot {
            1 => fixed(Slot::BigFixed, 16.0),
            2 => fixed(Slot::SmallFixed, 8.0),
            _ => {
                let retail_scale = font_scale * 0.25;
                let font = fonts.pick(Slot::Default, retail_scale, screen_h);
                TextFont {
                    font,
                    retail_scale,
                    height: font.max_height as f32 * font.glyph_scale * retail_scale,
                    cell: None,
                }
            }
        }
    }

    /// The `scale` [`font::layout`] and [`font::measure`] take for window px.
    fn px_scale(&self, v: &Virtual) -> f32 {
        self.retail_scale * v.scale / self.font.unit_scale()
    }

    fn width(&self, text: &str) -> f32 {
        match self.cell {
            Some(cell) => font::char_count(text) as f32 * cell,
            None => font::measure(self.font, text, self.retail_scale / self.font.unit_scale()),
        }
    }

    fn draw(&self, s: &str, at: (f32, f32), color: [f32; 4], v: &Virtual, out: &mut Vec<HudQuad>) {
        let start = out.len();
        match self.cell {
            Some(cell) => {
                let [px, py] = v.point(at.0, at.1);
                font::layout_fixed(
                    self.font,
                    s,
                    px,
                    py,
                    self.px_scale(v),
                    cell * v.scale,
                    color,
                    out,
                );
                shadow_alpha(&mut out[start..], color[3]);
            }
            None => text(self.font, s, at, self.px_scale(v), color, v, out),
        }
    }
}

/// [`font::layout`] at a virtual top-left, with the shadow's alpha scaled by
/// `color`'s and every glyph taking `color`'s alpha, as retail's `^N` does.
pub fn text(
    fnt: &Font,
    s: &str,
    (x, y): (f32, f32),
    px_scale: f32,
    color: [f32; 4],
    v: &Virtual,
    out: &mut Vec<HudQuad>,
) {
    let start = out.len();
    let [px, py] = v.point(x, y);
    font::layout(fnt, s, px, py, px_scale, color, out);
    shadow_alpha(&mut out[start..], color[3]);
}

/// Alternating shadow and glyph quads: the shadow at 0.8 of `alpha`.
fn shadow_alpha(quads: &mut [HudQuad], alpha: f32) {
    for (i, q) in quads.iter_mut().enumerate() {
        q.rgba[3] = if i % 2 == 0 { 0.8 * alpha } else { alpha };
    }
}

/// Draws `elems` in retail's order: sorted by `sort`, stably, so pass the
/// archived array first and the current one after it.
#[expect(clippy::too_many_arguments)]
pub fn build(
    elems: &[HudElem],
    configstrings: &[String],
    layout: &CsLayout,
    loc: &Localized,
    bound: &dyn Fn(&str) -> Option<String>,
    fonts: &UiFonts,
    server_time: i32,
    v: &Virtual,
    out: &mut Vec<HudQuad>,
) {
    for e in draw_order(elems) {
        draw(
            e,
            configstrings,
            layout,
            loc,
            bound,
            fonts,
            server_time,
            v,
            out,
        );
    }
}

/// `elems` stably sorted by `sort`. A NaN key, which no stock server
/// sends, sorts as 0 so the comparator stays a total order.
fn draw_order(elems: &[HudElem]) -> Vec<&HudElem> {
    let key = |e: &HudElem| {
        let k = e.get_f32(f::SORT);
        if k.is_nan() { 0.0 } else { k }
    };
    let mut order: Vec<&HudElem> = elems.iter().collect();
    order.sort_by(|a, b| {
        key(a)
            .partial_cmp(&key(b))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    order
}

#[expect(clippy::too_many_arguments)]
fn draw(
    e: &HudElem,
    cs: &[String],
    layout: &CsLayout,
    loc: &Localized,
    bound: &dyn Fn(&str) -> Option<String>,
    fonts: &UiFonts,
    now: i32,
    v: &Virtual,
    out: &mut Vec<HudQuad>,
) {
    let ty = e.get(f::TYPE);
    if !(1..=9).contains(&ty) {
        return;
    }
    let color = fade_color(e, now);
    let tf = TextFont::of(fonts, e.get(f::FONT), e.get_f32(f::FONT_SCALE), v);

    let body = match ty {
        1 => Some(match e.get(f::TEXT) {
            0 => String::new(),
            i => localized(loc, bound, cs, layout.localized() + i as usize),
        }),
        2 => Some(format_g(e.get_f32(f::VALUE))),
        4..=7 => Some(timer_text(ty, e.get(f::TIME), now)),
        _ => None,
    };

    let label = match e.get(f::LABEL) {
        0 => String::new(),
        i => localized(loc, bound, cs, layout.localized() + i as usize),
    };
    if let Some(body) = body {
        let s = merge_label(&label, &body);
        if s.is_empty() {
            return;
        }
        let w = tf.width(&s);
        let pos = place(e, now, w, tf.height);
        let v = v.at(Anchor::of_span(pos.0, pos.0 + w));
        tf.draw(&s, pos, color, &v, out);
        return;
    }

    let tween = |to: usize, from: usize| {
        let or_font = |n: i32| if n == 0 { tf.height } else { n as f32 };
        lerp_over(
            or_font(e.get(from)),
            or_font(e.get(to)),
            now,
            e.get(f::SCALE_START_TIME),
            e.get(f::SCALE_TIME),
        )
    };
    let w = tween(f::WIDTH, f::FROM_WIDTH);
    let h = tween(f::HEIGHT, f::FROM_HEIGHT);
    // The label leads the shader in one box; each aligns by its own height.
    let label_w = if label.is_empty() {
        0.0
    } else {
        tf.width(&label)
    };
    let (x, y) = place(e, now, label_w + w, h);
    // A full-width shader (a fade, a killcam bar) covers the window; the
    // rest keep to the edge their span is nearest.
    let stretched = full_width(x + label_w, w);
    let edge = v.at(Anchor::of_span(x, x + label_w + w));
    let v = if stretched { v } else { &edge };
    if label_w > 0.0 {
        let (_, label_y) = place(e, now, label_w + w, tf.height);
        tf.draw(&label, (x, label_y), color, v, out);
    }
    let Some(material) = shader(cs, layout, e.get(f::SHADER)) else {
        return;
    };
    let x = x + label_w;
    out.push(v.quad_fill(x, y, w, h, color, material));
    if ty >= 8 {
        let ms = timer_ms(ty, e.get(f::TIME), now) as f32;
        let turn = match e.get(f::DURATION) {
            0 => ms / 60_000.0,
            d => ms / d as f32,
        };
        let deg = turn.fract() * 360.0;
        let (hw, hh) = (w / 2.0, h / 2.0);
        let corners = [[-hw, -hh], [hw, -hh], [hw, hh], [-hw, hh]];
        let needle = format!("{material}Needle");
        out.push(v.rotated((x + hw, y + hh), corners, deg, color, &needle));
    }
}

/// The element's top-left: `x`/`y` after the move tween, less the alignment
/// share of its own `w` x `h`.
fn place(e: &HudElem, now: i32, w: f32, h: f32) -> (f32, f32) {
    let (start, time) = (e.get(f::MOVE_START_TIME), e.get(f::MOVE_TIME));
    let x = lerp_over(
        e.get(f::FROM_X) as f32,
        e.get(f::X) as f32,
        now,
        start,
        time,
    );
    let y = lerp_over(
        e.get(f::FROM_Y) as f32,
        e.get(f::Y) as f32,
        now,
        start,
        time,
    );
    let share = |align: i32| match align {
        1 => 0.5,
        2 => 1.0,
        _ => 0.0,
    };
    (
        x - w * share(e.get(f::ALIGN_X)),
        y - h * share(e.get(f::ALIGN_Y)),
    )
}

/// `to` once `time` ms from `start` have passed or when `time` is 0, `from`
/// before `start`.
fn lerp_over(from: f32, to: f32, now: i32, start: i32, time: i32) -> f32 {
    if time <= 0 || now - start >= time {
        return to;
    }
    let t = ((now - start) as f32 / time as f32).clamp(0.0, 1.0);
    from + (to - from) * t
}

fn fade_color(e: &HudElem, now: i32) -> [f32; 4] {
    let to = unpack_color(e.get(f::COLOR));
    let from = unpack_color(e.get(f::FROM_COLOR));
    let (start, time) = (e.get(f::FADE_START_TIME), e.get(f::FADE_TIME));
    std::array::from_fn(|i| lerp_over(from[i], to[i], now, start, time))
}

/// A localized-string configstring as text: its table entry, or the bare
/// key when the table lacks it, through the `[{command}]` pass
/// (`hud::bind_keys`, the cgame's "hudelem string"); an empty configstring
/// reads empty.
fn localized(
    loc: &Localized,
    bound: &dyn Fn(&str) -> Option<String>,
    cs: &[String],
    index: usize,
) -> String {
    let key = cs.get(index).map(String::as_str).unwrap_or("");
    if key.is_empty() {
        return String::new();
    }
    let bare = key.strip_prefix('@').unwrap_or(key);
    super::bind_keys(loc.get(bare).unwrap_or(bare), bound)
}

/// `shaderIndex`'s material, or `None` (warned once per index) when its
/// configstring is empty.
fn shader<'a>(cs: &'a [String], layout: &CsLayout, index: i32) -> Option<&'a str> {
    let name = cs
        .get(layout.shaders() + index.max(0) as usize)
        .map(String::as_str)
        .unwrap_or("");
    if name.is_empty() {
        warn_once("shader", index);
        return None;
    }
    Some(name)
}

static WARNED: Mutex<BTreeSet<(&str, i32)>> = Mutex::new(BTreeSet::new());

fn warn_once(what: &'static str, index: i32) {
    let mut warned = WARNED.lock().unwrap_or_else(|p| p.into_inner());
    if warned.insert((what, index)) {
        log::warn!("hudelem: {what} index {index} has no configstring, element skipped");
    }
}

/// `color.rgba` as the wire carries it: red in the low byte, alpha in the high.
pub fn unpack_color(rgba: i32) -> [f32; 4] {
    rgba.to_le_bytes().map(|b| b as f32 / 255.0)
}

/// The ms a timer or clock element (types 4 to 9) shows, never below 0.
fn timer_ms(ty: i32, time: i32, now: i32) -> i32 {
    let ms = match ty {
        4 => time - now + 999,
        6 => time - now + 99,
        8 => time - now,
        5 | 7 | 9 => now - time,
        _ => 0,
    };
    ms.max(0)
}

/// What a timer element (types 4 to 7) prints `now` against its `time`.
pub fn timer_text(ty: i32, time: i32, now: i32) -> String {
    let ms = timer_ms(ty, time, now);
    if ty >= 6 {
        let tenths = ms / 100;
        let (h, rest) = (tenths / 36_000, tenths % 36_000);
        let (m, s, t) = (rest / 600, rest % 600 / 10, rest % 10);
        return match h {
            0 => format!("{m}:{s:02}.{t}"),
            _ => format!("{h}:{m:02}:{s:02}.{t}"),
        };
    }
    let secs = ms / 1000;
    let (h, rest) = (secs / 3600, secs % 3600);
    match h {
        0 => format!("{}:{:02}", rest / 60, rest % 60),
        _ => format!("{h}:{:02}:{:02}", rest / 60, rest % 60),
    }
}

/// C's `%g` at its default precision of six significant digits.
pub fn format_g(v: f32) -> String {
    let v = v as f64;
    if v == 0.0 || !v.is_finite() {
        return format!("{v}");
    }
    let sci = format!("{v:.5e}");
    let (mantissa, exp) = sci.split_once('e').expect("{:e} always has an exponent");
    let exp: i32 = exp.parse().expect("{:e} exponent is an integer");
    if !(-4..6).contains(&exp) {
        let sign = if exp < 0 { '-' } else { '+' };
        return format!("{}e{sign}{:02}", trim_fraction(mantissa), exp.abs());
    }
    trim_fraction(&format!("{v:.*}", (5 - exp) as usize)).to_string()
}

fn trim_fraction(s: &str) -> &str {
    if s.contains('.') {
        s.trim_end_matches('0').trim_end_matches('.')
    } else {
        s
    }
}

/// A `label` holding `%s` takes the text in its place; any other label
/// prefixes it. Either alone prints as it is.
pub fn merge_label(label: &str, text: &str) -> String {
    if label.is_empty() || text.is_empty() {
        return format!("{label}{text}");
    }
    match label.find("%s") {
        Some(i) => format!("{}{text}{}", &label[..i], &label[i + 2..]),
        None => format!("{label}{text}"),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::hud::font::Glyph;

    /// Every glyph 8 wide and 10 tall at `glyph_scale` 3, so at the default
    /// slot's 0.25 a glyph advances 6 virtual units and stands 7.5 tall.
    pub(crate) fn test_font() -> Font {
        let g = Glyph {
            height: 10,
            width: 8,
            top: 10.0,
            bearing: 0.0,
            advance: 8.0,
            image_width: 8,
            image_height: 10,
            s: 0.0,
            t: 0.0,
            s2: 1.0,
            t2: 1.0,
            page: 0,
        };
        Font {
            size: 16,
            glyphs: vec![g; 256],
            glyph_scale: 3.0,
            line_advance: 30.0,
            max_height: 10,
            page: "fonts/test".into(),
            pages: vec!["fonts/test".into()],
        }
    }

    /// [`test_font`] in all six places.
    pub(crate) fn test_fonts() -> UiFonts {
        UiFonts {
            small: test_font(),
            normal: test_font(),
            console: test_font(),
            big: test_font(),
            bold: test_font(),
            extra_big: test_font(),
        }
    }

    fn elem(fields: &[(usize, i32)]) -> HudElem {
        let mut e = HudElem::default();
        e.set(f::COLOR, -1);
        e.set_f32(f::FONT_SCALE, 1.0);
        for &(k, v) in fields {
            e.set(k, v);
        }
        e
    }

    fn cs_with(entries: &[(usize, &str)]) -> Vec<String> {
        let mut cs = vec![String::new(); 2048];
        for &(i, s) in entries {
            cs[i] = s.to_string();
        }
        cs
    }

    fn run(elems: &[HudElem], cs: &[String], now: i32, screen: (f32, f32)) -> Vec<HudQuad> {
        let fonts = test_fonts();
        let mut out = Vec::new();
        build(
            elems,
            cs,
            &vcod_common::net::protocol::CS_LAYOUT_V1,
            &Localized::default(),
            &|_| None,
            &fonts,
            now,
            &Virtual::new(screen),
            &mut out,
        );
        out
    }

    /// (min x, min y, max x, max y) over the glyph quads, shadows skipped.
    fn glyph_bounds(quads: &[HudQuad]) -> [f32; 4] {
        let mut b = [f32::MAX, f32::MAX, f32::MIN, f32::MIN];
        for q in quads.iter().skip(1).step_by(2) {
            for [x, y] in q.verts {
                b = [b[0].min(x), b[1].min(y), b[2].max(x), b[3].max(y)];
            }
        }
        b
    }

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-3
    }

    #[test]
    fn a_nan_sort_key_sorts_as_zero() {
        let keyed = |k: f32| {
            let mut e = elem(&[(f::TYPE, 1)]);
            e.set_f32(f::SORT, k);
            e
        };
        let mut keys = vec![6.0];
        keys.extend([5.0; 10]);
        keys.push(f32::NAN);
        keys.extend([1.0; 12]);
        keys.extend([f32::NAN, -0.0, 2.0, f32::NAN, 0.0].repeat(4));
        let elems: Vec<HudElem> = keys.iter().map(|&k| keyed(k)).collect();
        let sorted: Vec<f32> = draw_order(&elems)
            .iter()
            .map(|e| e.get_f32(f::SORT))
            .map(|k| if k.is_nan() { 0.0 } else { k })
            .collect();
        assert!(sorted.windows(2).all(|w| w[0] <= w[1]), "{sorted:?}");
        run(&elems, &cs_with(&[]), 0, (640.0, 480.0));
    }

    #[test]
    fn text_centres_on_its_point_both_ways() {
        let cs = cs_with(&[(CS_LOCALIZED_V1 + 1, "HELLO")]);
        let e = elem(&[
            (f::TYPE, 1),
            (f::TEXT, 1),
            (f::X, 320),
            (f::Y, 240),
            (f::ALIGN_X, 1),
            (f::ALIGN_Y, 1),
        ]);
        // Five glyphs of 6 make 30 wide, 7.5 tall, centred on (320, 240).
        let b = glyph_bounds(&run(&[e], &cs, 0, (640.0, 480.0)));
        assert!(close(b[0], 305.0) && close(b[2], 335.0), "{b:?}");
        assert!(close(b[1], 236.25) && close(b[3], 243.75), "{b:?}");

        // A 16:9 window twice as tall: scaled by 2 and centred.
        let b = glyph_bounds(&run(&[e], &cs, 0, (1706.0, 960.0)));
        let x0 = (1706.0 - 1280.0) / 2.0;
        assert!(close(b[0], x0 + 610.0) && close(b[2], x0 + 670.0), "{b:?}");
    }

    #[test]
    fn a_fixed_slot_centres_each_glyph_in_its_cell() {
        let cs = cs_with(&[(CS_LOCALIZED_V1 + 1, "^1AB")]);
        let fixed = |font: i32| {
            elem(&[
                (f::TYPE, 1),
                (f::TEXT, 1),
                (f::FONT, font),
                (f::X, 320),
                (f::ALIGN_X, 1),
            ])
        };
        // bigfixed: two 16-wide cells from 304. A glyph advances 8
        // design units at 1/3 x 3, so 8 virtual, centred in its cell.
        let out = run(&[fixed(1)], &cs, 0, (640.0, 480.0));
        let glyphs: Vec<f32> = out
            .iter()
            .skip(1)
            .step_by(2)
            .map(|q| q.verts[0][0])
            .collect();
        assert_eq!(glyphs, [308.0, 324.0]);
        // smallfixed: 8-wide cells, the glyph filling each.
        let out = run(&[fixed(2)], &cs, 0, (640.0, 480.0));
        let glyphs: Vec<f32> = out
            .iter()
            .skip(1)
            .step_by(2)
            .map(|q| q.verts[0][0])
            .collect();
        assert_eq!(glyphs, [312.0, 320.0]);
    }

    #[test]
    fn the_ui_picks_a_font_by_slot_and_drawn_size() {
        let mut fonts = test_fonts();
        for (font, size) in [
            (&mut fonts.small, 12),
            (&mut fonts.normal, 16),
            (&mut fonts.console, 18),
            (&mut fonts.big, 24),
            (&mut fonts.bold, 30),
            (&mut fonts.extra_big, 32),
        ] {
            font.size = size;
        }
        let size = |slot, scale, h| fonts.pick(slot, scale, h).size;
        // The default slot at fontScale 1 is 0.25: small at 480 lines.
        assert_eq!(size(Slot::Default, 0.25, 480.0), 12);
        assert_eq!(size(Slot::Default, 0.25, 720.0), 16);
        assert_eq!(size(Slot::Default, 0.25, 768.0), 24);
        assert_eq!(size(Slot::Default, 0.25, 1080.0), 32);
        // bigfixed tops out at the bold font; smallfixed is always the console's.
        assert_eq!(size(Slot::BigFixed, 1.0 / 3.0, 480.0), 16);
        assert_eq!(size(Slot::BigFixed, 1.0 / 3.0, 1080.0), 30);
        assert_eq!(size(Slot::BigFixed, 0.2, 480.0), 12);
        assert_eq!(size(Slot::SmallFixed, 1.0 / 3.0, 1080.0), 18);
    }

    #[test]
    fn shader_element_fills_its_rect() {
        let cs = cs_with(&[(CS_SHADERS_V1 + 5, "white")]);
        let e = elem(&[
            (f::TYPE, 3),
            (f::SHADER, 5),
            (f::X, 100),
            (f::Y, 50),
            (f::WIDTH, 64),
            (f::HEIGHT, 32),
        ]);
        let out = run(&[e], &cs, 0, (1280.0, 960.0));
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].texture, "white");
        assert_eq!(
            out[0].verts,
            [
                [200.0, 100.0],
                [328.0, 100.0],
                [328.0, 164.0],
                [200.0, 164.0]
            ]
        );
    }

    #[test]
    fn a_shader_shorter_than_its_font_aligns_by_its_own_height() {
        let cs = cs_with(&[(CS_SHADERS_V1 + 5, "white")]);
        let e = elem(&[
            (f::TYPE, 3),
            (f::SHADER, 5),
            (f::FONT, 1),
            (f::Y, 100),
            (f::ALIGN_Y, 2),
            (f::WIDTH, 64),
            (f::HEIGHT, 4),
        ]);
        let q = &run(&[e], &cs, 0, (640.0, 480.0))[0];
        assert_eq!((q.verts[0][1], q.verts[2][1]), (96.0, 100.0));
    }

    #[test]
    fn a_shader_label_leads_it_inside_the_aligned_box() {
        let cs = cs_with(&[(CS_SHADERS_V1 + 5, "white"), (CS_LOCALIZED_V1 + 2, "AB")]);
        let e = elem(&[
            (f::TYPE, 3),
            (f::SHADER, 5),
            (f::LABEL, 2),
            (f::X, 100),
            (f::Y, 50),
            (f::ALIGN_X, 2),
            (f::WIDTH, 20),
            (f::HEIGHT, 20),
        ]);
        let out = run(&[e], &cs, 0, (640.0, 480.0));
        // Two glyphs of 6: the label spans 68..80 and the shader 80..100.
        let label = glyph_bounds(&out[..4]);
        assert!(close(label[0], 68.0) && close(label[2], 80.0), "{label:?}");
        let shader = out.last().expect("shader");
        assert_eq!((shader.verts[0][0], shader.verts[1][0]), (80.0, 100.0));
    }

    #[test]
    fn empty_shader_configstring_skips_the_element() {
        let cs = cs_with(&[]);
        let e = elem(&[(f::TYPE, 3), (f::SHADER, 9), (f::WIDTH, 8), (f::HEIGHT, 8)]);
        assert!(run(&[e, e], &cs, 0, (640.0, 480.0)).is_empty());
    }

    #[test]
    fn countdown_reads_whole_seconds_and_stops_at_zero() {
        let now = 100_000;
        assert_eq!(timer_text(4, now + 65_000, now), "1:05");
        assert_eq!(timer_text(4, now - 3_000, now), "0:00");
        assert_eq!(timer_text(5, now - 3_723_000, now), "1:02:03");
        assert_eq!(timer_text(6, now + 1_250, now), "0:01.3");
        assert_eq!(timer_text(7, now + 10, now), "0:00.0");
    }

    #[test]
    fn progress_bar_scale_tween_is_halfway_at_half_time() {
        let cs = cs_with(&[(CS_SHADERS_V1 + 2, "white")]);
        let bar = |from_w: i32| {
            elem(&[
                (f::TYPE, 3),
                (f::SHADER, 2),
                (f::WIDTH, 192),
                (f::HEIGHT, 8),
                (f::FROM_WIDTH, from_w),
                (f::FROM_HEIGHT, 8),
                (f::SCALE_START_TIME, 9_000),
                (f::SCALE_TIME, 2_000),
            ])
        };
        let width = |q: &HudQuad| q.verts[1][0] - q.verts[0][0];
        assert!(close(
            width(&run(&[bar(32)], &cs, 10_000, (640.0, 480.0))[0]),
            112.0
        ));
        // A zero width reads as the font's height, 7.5 here, so the stock
        // `setShader("white", 0, 8)` bar starts there.
        let w = width(&run(&[bar(0)], &cs, 10_000, (640.0, 480.0))[0]);
        assert!(close(w, (7.5 + 192.0) / 2.0), "{w}");
        // Past the tween: the target width.
        assert!(close(
            width(&run(&[bar(0)], &cs, 12_000, (640.0, 480.0))[0]),
            192.0
        ));
    }

    #[test]
    fn fade_tween_is_halfway_at_half_time() {
        let cs = cs_with(&[(CS_SHADERS_V1 + 2, "white")]);
        let e = elem(&[
            (f::TYPE, 3),
            (f::SHADER, 2),
            (f::WIDTH, 8),
            (f::HEIGHT, 8),
            (f::FROM_COLOR, 0),
            (f::FADE_START_TIME, 1_000),
            (f::FADE_TIME, 1_000),
        ]);
        let rgba = run(&[e], &cs, 1_500, (640.0, 480.0))[0].rgba;
        assert!(rgba.iter().all(|c| close(*c, 127.5 / 255.0)), "{rgba:?}");
        let rgba = run(&[e], &cs, 2_000, (640.0, 480.0))[0].rgba;
        assert_eq!(rgba, [1.0; 4]);
    }

    #[test]
    fn alpha_is_the_high_byte() {
        assert_eq!(
            unpack_color(0x80FF_0000_u32 as i32),
            [0.0, 0.0, 1.0, 128.0 / 255.0]
        );
        assert_eq!(unpack_color(0x0000_00FF), [1.0, 0.0, 0.0, 0.0]);
    }

    #[test]
    fn sorted_by_sort_stably_across_both_arrays() {
        let cs = cs_with(&[
            (CS_SHADERS_V1 + 1, "a"),
            (CS_SHADERS_V1 + 2, "b"),
            (CS_SHADERS_V1 + 3, "c"),
        ]);
        let sh = |shader: i32, sort: f32| {
            let mut e = elem(&[
                (f::TYPE, 3),
                (f::SHADER, shader),
                (f::WIDTH, 8),
                (f::HEIGHT, 8),
            ]);
            e.set_f32(f::SORT, sort);
            e
        };
        // Archived `a`, then current `b` and `c`.
        let out = run(
            &[sh(1, 1.0), sh(2, 0.0), sh(3, 1.0)],
            &cs,
            0,
            (640.0, 480.0),
        );
        let order: Vec<&str> = out.iter().map(|q| q.texture.as_str()).collect();
        assert_eq!(order, ["b", "a", "c"]);
    }

    #[test]
    fn value_prints_like_percent_g() {
        assert_eq!(format_g(5.0), "5");
        assert_eq!(format_g(0.5), "0.5");
        assert_eq!(format_g(-12.25), "-12.25");
        assert_eq!(format_g(100.0), "100");
        assert_eq!(format_g(1_234_567.0), "1.23457e+06");
        assert_eq!(format_g(0.0001), "0.0001");
        assert_eq!(format_g(0.00001), "1e-05");
        assert_eq!(format_g(0.0), "0");
    }

    #[test]
    fn label_substitutes_or_prefixes() {
        assert_eq!(merge_label("Time: %s left", "1:00"), "Time: 1:00 left");
        assert_eq!(merge_label("Score: ", "5"), "Score: 5");
        assert_eq!(merge_label("", "5"), "5");
        assert_eq!(merge_label("Score: %s", ""), "Score: %s");
    }

    /// The grid on a 21:9 window: scaled by the height, centred, and an
    /// edge element moved by the pillarbox's width; boxed, none moves.
    #[test]
    fn a_wide_window_centres_the_grid_and_anchors_the_edges() {
        let v = Virtual::new((2520.0, 1080.0));
        assert!(close(v.scale, 2.25) && close(v.x0, 540.0) && v.y0 == 0.0);
        assert_eq!(v.point(0.0, 0.0), [540.0, 0.0]);
        assert_eq!(v.left().point(0.0, 480.0), [0.0, 1080.0]);
        assert_eq!(v.right().point(640.0, 0.0), [2520.0, 0.0]);
        assert_eq!(v.at(Anchor::Centre).point(320.0, 0.0), [1260.0, 0.0]);
        let boxed = v.boxed();
        assert_eq!(boxed.left().point(0.0, 0.0), [540.0, 0.0]);
        assert_eq!(boxed.right().point(640.0, 0.0), [1980.0, 0.0]);
        // A 5:4 window letterboxes: scaled by the width, centred down.
        let v = Virtual::new((1280.0, 1024.0));
        assert!(close(v.scale, 2.0) && v.x0 == 0.0 && close(v.y0, 32.0));
        assert_eq!(v.left().point(0.0, 0.0), [0.0, 32.0]);
        assert_eq!(v.screen(), (1280.0, 1024.0));
    }

    #[test]
    fn the_cursor_maps_through_the_box_and_clamps_to_it() {
        let v = Virtual::new((1920.0, 1080.0));
        assert_eq!(v.grid_point(960.0, 540.0), [320.0, 240.0]);
        assert_eq!(v.grid_point(240.0, 0.0), [0.0, 0.0]);
        assert_eq!(v.grid_point(10.0, 1080.0), [0.0, 480.0], "left of the box");
        assert_eq!(v.grid_point(1910.0, 2.25), [640.0, 1.0]);
        for (px, py) in [(500.0, 100.0), (1500.0, 900.0)] {
            let [x, y] = v.grid_point(px, py);
            let [bx, by] = v.point(x, y);
            assert!(close(bx, px) && close(by, py));
        }
    }

    /// A span keeps to the third its middle is in.
    #[test]
    fn a_span_anchors_by_its_middle_third() {
        assert_eq!(Anchor::of_span(6.0, 120.0), Anchor::Left);
        assert_eq!(
            Anchor::of_span(192.0, 448.0),
            Anchor::Centre,
            "a progress bar"
        );
        assert_eq!(Anchor::of_span(505.0, 635.0), Anchor::Right);
        assert_eq!(Anchor::of_span(0.0, 640.0), Anchor::Centre);
    }

    /// A full-screen shader (a killcam bar, a fade) covers the window; a
    /// script's clock at 520 keeps to the right edge, its progress bar to
    /// the middle.
    #[test]
    fn hud_elems_stretch_full_width_and_keep_to_their_edge() {
        let cs = cs_with(&[(CS_SHADERS_V1 + 1, "black"), (CS_LOCALIZED_V1 + 1, "9")]);
        let bar = elem(&[
            (f::TYPE, 3),
            (f::SHADER, 1),
            (f::WIDTH, 640),
            (f::HEIGHT, 50),
        ]);
        let out = run(&[bar], &cs, 0, (2520.0, 1080.0));
        assert_eq!(out[0].verts[0], [0.0, 0.0]);
        assert_eq!(out[0].verts[2], [2520.0, 112.5]);
        let clock = elem(&[(f::TYPE, 1), (f::TEXT, 1), (f::X, 520), (f::Y, 100)]);
        let b = glyph_bounds(&run(&[clock], &cs, 0, (2520.0, 1080.0)));
        assert!(close(b[0], 540.0 * 2.0 + 520.0 * 2.25), "{b:?}");
        let progress = elem(&[
            (f::TYPE, 3),
            (f::SHADER, 1),
            (f::X, 192),
            (f::WIDTH, 256),
            (f::HEIGHT, 8),
        ]);
        let out = run(&[progress], &cs, 0, (2520.0, 1080.0));
        assert_eq!(out[0].verts[0][0], 540.0 + 192.0 * 2.25);
    }
}
