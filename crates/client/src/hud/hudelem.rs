//! The script's hudelems (playerstate block 5) laid out on retail's 640x480
//! virtual screen. How the cgame draws one: docs/research/cod11-hud-protocol.md,
//! section 8.

use std::collections::BTreeSet;
use std::sync::Mutex;

use super::font::{self, Font};
use super::HudQuad;
use vcod_common::localize::Localized;
use vcod_common::net::msg::{hud_field as f, HudElem};

/// `text`, `label` and the hint strings index configstrings from here.
pub const CS_LOCALIZED: usize = 1244;
/// `shaderIndex` and the objective icons index configstrings from here.
pub const CS_SHADERS: usize = 1500;

/// Retail's 640x480 virtual screen, scaled by the window height and centred
/// horizontally, so a wider window letterboxes to 4:3.
#[derive(Clone, Copy, Debug)]
pub struct Virtual {
    pub scale: f32,
    pub x0: f32,
}

const FULL_UV: [[f32; 2]; 4] = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];

impl Virtual {
    pub fn new((w, h): (f32, f32)) -> Virtual {
        let scale = h / 480.0;
        Virtual {
            scale,
            x0: (w - 640.0 * scale) / 2.0,
        }
    }

    pub fn point(&self, x: f32, y: f32) -> [f32; 2] {
        [self.x0 + x * self.scale, y * self.scale]
    }

    pub fn quad(&self, x: f32, y: f32, w: f32, h: f32, rgba: [f32; 4], texture: &str) -> HudQuad {
        self.quad_uv(x, y, w, h, FULL_UV, rgba, texture)
    }

    /// `uvs` in the quad's ring order: TL, TR, BR, BL.
    #[allow(clippy::too_many_arguments)]
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

/// Retail's text scale for a font slot: `0.25 * fontScale` for `default`,
/// `fontScale / 3` for the two fixed fonts, and the height alignment uses.
struct TextFont<'a> {
    font: &'a Font,
    retail_scale: f32,
    height: f32,
}

impl TextFont<'_> {
    /// The `scale` [`font::layout`] and [`font::measure`] take for window px.
    fn px_scale(&self, v: &Virtual) -> f32 {
        self.retail_scale * v.scale / self.font.unit_scale()
    }

    fn width(&self, text: &str) -> f32 {
        font::measure(self.font, text, self.retail_scale / self.font.unit_scale())
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
    for (i, q) in out[start..].iter_mut().enumerate() {
        q.rgba[3] = if i % 2 == 0 { 0.8 * color[3] } else { color[3] };
    }
}

/// Draws `elems` in retail's order: sorted by `sort`, stably, so pass the
/// archived array first and the current one after it. `fonts` are the
/// `default`, `bigfixed` and `smallfixed` slots; no fixed-font atlas ships,
/// so pass the default font for those.
pub fn build(
    elems: &[HudElem],
    configstrings: &[String],
    loc: &Localized,
    fonts: (&Font, &Font, &Font),
    server_time: i32,
    screen: (f32, f32),
    out: &mut Vec<HudQuad>,
) {
    let v = Virtual::new(screen);
    let mut order: Vec<&HudElem> = elems.iter().collect();
    order.sort_by(|a, b| {
        a.get_f32(f::SORT)
            .partial_cmp(&b.get_f32(f::SORT))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    for e in order {
        draw(e, configstrings, loc, fonts, server_time, &v, out);
    }
}

fn draw(
    e: &HudElem,
    cs: &[String],
    loc: &Localized,
    fonts: (&Font, &Font, &Font),
    now: i32,
    v: &Virtual,
    out: &mut Vec<HudQuad>,
) {
    let ty = e.get(f::TYPE);
    if !(1..=9).contains(&ty) {
        return;
    }
    let color = fade_color(e, now);
    let font_scale = e.get_f32(f::FONT_SCALE);
    let tf = match e.get(f::FONT) {
        slot @ (1 | 2) => TextFont {
            font: if slot == 1 { fonts.1 } else { fonts.2 },
            retail_scale: font_scale / 3.0,
            height: 16.0,
        },
        _ => TextFont {
            font: fonts.0,
            retail_scale: font_scale * 0.25,
            height: fonts.0.max_height as f32 * fonts.0.glyph_scale * font_scale * 0.25,
        },
    };

    let body = match ty {
        1 => Some(match e.get(f::TEXT) {
            0 => String::new(),
            i => localized(loc, cs, CS_LOCALIZED + i as usize),
        }),
        2 => Some(format_g(e.get_f32(f::VALUE))),
        4..=7 => Some(timer_text(ty, e.get(f::TIME), now)),
        _ => None,
    };

    if let Some(body) = body {
        let label = match e.get(f::LABEL) {
            0 => String::new(),
            i => localized(loc, cs, CS_LOCALIZED + i as usize),
        };
        let s = merge_label(&label, &body);
        if s.is_empty() {
            return;
        }
        let pos = place(e, now, tf.width(&s), tf.height);
        text(tf.font, &s, pos, tf.px_scale(v), color, v, out);
        return;
    }

    let Some(material) = shader(cs, e.get(f::SHADER)) else {
        return;
    };
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
    let (x, y) = place(e, now, w, h);
    out.push(v.quad(x, y, w, h, color, material));
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
/// key when the table lacks it; an empty configstring reads empty.
fn localized(loc: &Localized, cs: &[String], index: usize) -> String {
    let key = cs.get(index).map(String::as_str).unwrap_or("");
    if key.is_empty() {
        return String::new();
    }
    let bare = key.strip_prefix('@').unwrap_or(key);
    loc.get(bare).unwrap_or(bare).to_string()
}

/// `shaderIndex`'s material, or `None` (warned once per index) when its
/// configstring is empty.
fn shader(cs: &[String], index: i32) -> Option<&str> {
    let name = cs
        .get(CS_SHADERS + index.max(0) as usize)
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
            height_f: 11.0,
            bearing: 0.0,
            advance: 8.0,
            image_width: 8,
            image_height: 10,
            s: 0.0,
            t: 0.0,
            s2: 1.0,
            t2: 1.0,
        };
        Font {
            size: 16,
            glyphs: vec![g; 256],
            glyph_scale: 3.0,
            line_advance: 30.0,
            max_height: 10,
            page: "fonts/test".into(),
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
        let font = test_font();
        let mut out = Vec::new();
        build(
            elems,
            cs,
            &Localized::default(),
            (&font, &font, &font),
            now,
            screen,
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
    fn text_centres_on_its_point_both_ways() {
        let cs = cs_with(&[(CS_LOCALIZED + 1, "HELLO")]);
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
    fn shader_element_fills_its_rect() {
        let cs = cs_with(&[(CS_SHADERS + 5, "white")]);
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
        let cs = cs_with(&[(CS_SHADERS + 5, "white")]);
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
        let cs = cs_with(&[(CS_SHADERS + 2, "white")]);
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
        let cs = cs_with(&[(CS_SHADERS + 2, "white")]);
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
            (CS_SHADERS + 1, "a"),
            (CS_SHADERS + 2, "b"),
            (CS_SHADERS + 3, "c"),
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
}
