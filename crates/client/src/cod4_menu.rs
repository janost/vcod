//! The open CoD4 script menu (`play::cod4_menus`) drawn as 1.7's UI
//! paints it (`Menu_Paint`, `Item_Paint`;
//! docs/research/cod4-hud.md 12): each visible item's window
//! (a filled rect in `backColor`, or its material in `foreColor` or
//! white), then its text placed by `textAlignMode` in its font, the
//! focused item's in the menu's pulsing `focusColor`; the cursor over
//! all. The in-game pages' controls draw their values (a bind row's keys,
//! yes or no, a list's entry), a text field its value past its own text,
//! a front-end owner draw its value at `textalignx` / `y` off its rect, a
//! slider its bar and thumb, a list box its rows (`Item_ListBox_Paint`)
//! (docs/research/cod4-hud.md 12.4); an option vcod has no cvar for draws
//! in the menu's `disableColor`. A menu without a layout, or with an expression
//! vcod does not evaluate, is a numbered list of its responses: 1-9 pick,
//! Esc backs out.

use vcod_cod4::assets::ui::TypeData;
use vcod_cod4::menuexp;
use vcod_cod4::scriptmenu::{
    ITEM_TYPE_LISTBOX, ITEM_TYPE_OWNERDRAW, ITEM_TYPE_SLIDER, Layout, LayoutItem,
    WINDOW_FORECOLORSET, is_text_field,
};

use crate::cod4_hud::{MenuAssets, quad, text};
use crate::hud::HudQuad;
use crate::hud::cod2elem::ScreenPlacement;
use crate::hud::font::{self, Font};
use crate::play::cod4_menus::controls::{
    SCROLL, SLIDER_H, SLIDER_W, THUMB_H, THUMB_W, UI_MAPPREVIEW, list_rows,
};
use crate::play::cod4_menus::{OpenMenu, ScriptMenus, label, placed};

/// The list's left edge, first line and line step on the 640x480 grid,
/// and its text scale.
const LEFT: f32 = 200.0;
const TOP: f32 = 140.0;
const STEP: f32 = 24.0;
const SCALE: f32 = 0.4;
/// The cursor's size on the 640x480 grid, drawn full screen
/// (`UI_Refresh`, `ui_cursor`).
const CURSOR: f32 = 32.0;

/// The open menu's quads on a `size` window at UI time `now` (ms);
/// `cursor` draws the mouse cursor.
pub(crate) fn quads(
    menus: &ScriptMenus,
    a: &MenuAssets,
    size: (f32, f32),
    now: i32,
    cursor: bool,
) -> Vec<HudQuad> {
    paint(menus, a, size, now, cursor).0
}

/// [`quads`], with each text drawn and its baseline's window position.
pub(crate) fn paint(
    menus: &ScriptMenus,
    a: &MenuAssets,
    size: (f32, f32),
    now: i32,
    cursor: bool,
) -> (Vec<HudQuad>, Vec<(String, [f32; 2])>) {
    let mut texts = Vec::new();
    let Some(open) = menus.open() else {
        return (Vec::new(), texts);
    };
    if open.layout.is_none() {
        let font = a
            .fonts
            .pick(1, SCALE, &ScreenPlacement::cod4(size.0, size.1));
        return (list(open, font, size), texts);
    }
    let sp = ScreenPlacement::cod4(size.0, size.1);
    let mut out = Vec::new();
    let env = menus.env();
    // The front end's stack paints bottom up; the server's menus are one.
    for m in menus.drawn() {
        let Some(layout) = &m.layout else {
            continue;
        };
        let top = std::ptr::eq(m, open);
        if layout.visible.is_empty() || menuexp::truth(&layout.visible, &env).unwrap_or(false) {
            for (i, it) in layout.items.iter().enumerate() {
                if menus.item_visible(it) {
                    let at = Drawn { open: m, i, top };
                    let shown = item(menus, a, layout, at, &sp, now, &mut out, &mut texts);
                    texts.extend(shown);
                }
            }
        }
    }
    if cursor && let Some(tex) = a.image("ui_cursor") {
        let [x, y] = menus.cursor(size);
        let (w, h) = (CURSOR * size.0 / 640.0, CURSOR * size.1 / 480.0);
        out.push(quad(x - 0.5 * w, y - 0.5 * h, w, h, [1.0; 4], tex, false));
    }
    (out, texts)
}

/// The item drawn: its menu, its index, and whether that menu is the
/// open one (the stack's top), whose list and bind state the menus hold.
#[derive(Clone, Copy)]
struct Drawn<'a> {
    open: &'a OpenMenu,
    i: usize,
    top: bool,
}

/// `Item_Paint` of a visible item: its window, then its text, which it
/// returns with its baseline's window position.
#[allow(clippy::too_many_arguments)]
fn item(
    menus: &ScriptMenus,
    a: &MenuAssets,
    layout: &Layout,
    at: Drawn,
    sp: &ScreenPlacement,
    now: i32,
    out: &mut Vec<HudQuad>,
    texts: &mut Vec<(String, [f32; 2])>,
) -> Option<(String, [f32; 2])> {
    let Drawn { i, top, .. } = at;
    let focused = at.open.focus == Some(i);
    let it = &layout.items[i];
    let env = menus.env();
    let mut fore = it.fore_color;
    if !it.exps.forecolor_a.is_empty() {
        fore[3] = menuexp::float(&it.exps.forecolor_a, &env).unwrap_or(0.0);
    }
    let (rect, align) = menus.rect(it, layout);
    let material = if it.style == 5 {
        it.dvar.as_deref().and_then(|d| env_dvar(menus, d))
    } else if !it.exps.material.is_empty() {
        menuexp::string(&it.exps.material, &env).ok()
    } else {
        it.background.clone()
    }
    .map(|m| m.to_ascii_lowercase());
    window(a, it, rect, align, fore, material.as_deref(), sp, out);
    let colour = if menus.greyed(it) {
        layout.disable_color
    } else {
        text_colour(layout, it, fore, focused, now)
    };
    match it.kind {
        ITEM_TYPE_SLIDER => {
            slider(menus, a, layout, it, colour, sp, out);
            return None;
        }
        ITEM_TYPE_LISTBOX => {
            texts.extend(list_box(menus, a, at, it, rect, align, sp, out));
            return None;
        }
        _ => {}
    }
    if it.kind == ITEM_TYPE_OWNERDRAW && it.owner_draw == UI_MAPPREVIEW {
        if let Some(tex) = menus.map_preview().and_then(|m| a.image(&m)) {
            let [x, y, w, h] = placed(sp, (rect, align));
            out.push(quad(x, y, w, h, colour, tex, false));
        }
        return None;
    }
    // A bind row waits only in the open menu.
    let value = menus.control_text(if top { i } else { usize::MAX }, it);
    let paint = Paint {
        a,
        it,
        rect,
        align,
        colour,
        sp,
    };
    if is_text_field(it.kind) {
        return text_field(&paint, value.unwrap_or_default(), &env, menus, out, texts);
    }
    if it.kind == ITEM_TYPE_OWNERDRAW {
        return owner_draw(&paint, value?, &env, menus, out, texts);
    }
    let raw = match value {
        Some(t) => t,
        None if !matches!(it.kind, 0 | 1) => return None,
        None => item_text(it, &env, menus)?,
    };
    if it.static_flags & ITEM_AUTOWRAPPED != 0 {
        texts.extend(paint.wrapped(&localize(a, &raw), out));
        return None;
    }
    paint
        .text_item(&localize(a, &raw), 0.0, out)
        .map(|(s, p, _)| (s, p))
}

/// `autowrapped`'s static flag (`ItemParse_autowrapped`, `iw3mp.exe`
/// 0x556cc0), which `Item_Text_Paint` tests (0x551fb9).
const ITEM_AUTOWRAPPED: u32 = 0x80_0000;

/// Where the line at the head of `s` stops for `width` grid units
/// (`R_TextLineWrapPosition`): just past a newline; otherwise, once the
/// text through some character is wider than `width`, before the last
/// whitespace seen by then (never the first character), so a word wider
/// than the line runs on to the next break; else the whole of `s`.
fn line_break(s: &str, width: f32, measure: &impl Fn(&str) -> f32) -> usize {
    let mut last_gap = None;
    for (at, ch) in s.char_indices() {
        let through = at + ch.len_utf8();
        if ch == '\n' {
            return through;
        }
        if ch.is_ascii_whitespace() && at > 0 {
            last_gap = Some(at);
        }
        match last_gap {
            Some(gap) if measure(&s[..through]) > width => return gap,
            _ => {}
        }
    }
    s.len()
}

/// `s` cut into lines no wider than `width` where a break allows
/// ([`line_break`]); a line loses its newline and the next starts past
/// any whitespace.
fn break_lines<'s>(s: &'s str, width: f32, measure: &impl Fn(&str) -> f32) -> Vec<&'s str> {
    let mut lines = Vec::new();
    let mut rest = s;
    while !rest.is_empty() {
        let (line, tail) = rest.split_at(line_break(rest, width, measure));
        lines.push(line.trim_end_matches('\n'));
        rest = tail.trim_start_matches(|c: char| c.is_ascii_whitespace());
    }
    lines
}

/// The lines the wrapped text draw puts `s` in for a rect `width` wide. A
/// centred text that overflows is balanced first
/// (`UI_PickWordWrapLineWidth`): the fewest lines the rect's width allows
/// are kept at the narrowest width, tried from an even split upward in
/// steps of 8 below the rect's, that still breaks into that many.
fn wrap_lines(s: &str, width: f32, centred: bool, measure: impl Fn(&str) -> f32) -> Vec<&str> {
    let full = measure(s);
    let rect = width as i32;
    let balanced = (centred && width != 0.0 && full > width)
        .then(|| {
            let lines = (full / width).ceil() as i32;
            let even = (full / lines as f32).ceil() as i32;
            (even..rect)
                .step_by(8)
                .find(|&w| break_lines(s, w as f32, &measure).len() as i32 == lines)
        })
        .flatten();
    break_lines(s, balanced.unwrap_or(rect) as f32, &measure)
}

/// What an item's texts are drawn with: its rect and alignment, the
/// text's colour.
struct Paint<'a> {
    a: &'a MenuAssets<'a>,
    it: &'a LayoutItem,
    rect: [f32; 4],
    align: [i32; 2],
    colour: [f32; 4],
    sp: &'a ScreenPlacement,
}

impl Paint<'_> {
    /// The item's font, its pixel scale and its text height on the grid.
    fn font(&self) -> (&Font, f32, f32) {
        let it = self.it;
        let f = self.a.fonts.pick(it.font, it.text_scale, self.sp);
        (
            f,
            it.text_scale / f.unit_scale(),
            (48.0 * it.text_scale) as i32 as f32,
        )
    }

    /// `Item_Text_Paint` of the localized `s`: placed by
    /// `Item_SetTextExtents`, `extra` the width an aligned control keeps
    /// beside it. Returns it with its baseline's window position and its
    /// text rect's right edge on the grid; `None` when empty.
    fn text_item(
        &self,
        s: &str,
        extra: f32,
        out: &mut Vec<HudQuad>,
    ) -> Option<(String, [f32; 2], f32)> {
        if s.is_empty() {
            return None;
        }
        let (f, px, th) = self.font();
        let tw = font::measure(f, s, px);
        let [x, y] = text_rect(self.it, self.rect, tw, th, extra);
        let (s, p) = self.text_at(s, [x, y], out);
        Some((s, p, x + tw))
    }

    /// `UI_DrawText` of `s` with its baseline at grid point `at`; returns
    /// it with the baseline's window position.
    fn text_at(&self, s: &str, [x, y]: [f32; 2], out: &mut Vec<HudQuad>) -> (String, [f32; 2]) {
        let (f, px, th) = self.font();
        let sp = self.sp;
        let tw = font::measure(f, s, px);
        let (rx, _) = sp.axis(0, x, tw, self.align[0]);
        let (ry, _) = sp.axis(1, y - th, th, self.align[1]);
        let from = out.len();
        text(f, s, rx, ry, px * sp.virtual_to_real[1], self.colour, out);
        // Styles 3 and 6 shadow the text; the rest draw it plain. The layout
        // pushes a shadow, then its glyph, per character.
        if !matches!(self.it.text_style, 3 | 6) {
            let glyphs: Vec<HudQuad> = out.drain(from..).skip(1).step_by(2).collect();
            out.extend(glyphs);
        }
        (s.to_string(), [rx, ry + th * sp.virtual_to_real[1]])
    }

    /// `Item_Text_AutoWrapped_Paint` (0x551da0): `s` broken into lines
    /// of the rect's width ([`wrap_lines`]), each aligned on its own, the
    /// first where an unwrapped text's baseline goes and each next one
    /// its height and 5 below. Returns each line with its baseline's
    /// window position.
    fn wrapped(&self, s: &str, out: &mut Vec<HudQuad>) -> Vec<(String, [f32; 2])> {
        let (_, _, th) = self.font();
        let [x, top] = text_rect(self.it, self.rect, 0.0, th, 0.0);
        let align = self.it.text_align & 3;
        // how much of its own width a line sits left of `x`
        let anchor = match align {
            1 => 0.5,
            2 => 1.0,
            _ => 0.0,
        };
        let mut baseline = top;
        let mut drawn = Vec::new();
        for line in wrap_lines(s, self.rect[2], align == 1, |t| self.width(t)) {
            if !line.is_empty() {
                let lx = x - anchor * self.width(line);
                drawn.push(self.text_at(line, [lx, baseline], out));
            }
            baseline += th + 5.0;
        }
        drawn
    }

    fn width(&self, s: &str) -> f32 {
        let (f, px, _) = self.font();
        font::measure(f, s, px)
    }
}

/// `Item_Paint` of a HUD menu's plain item (`cod4_hud::menus`): its
/// window in `fore` over `rect`, then `text` localized and placed by its
/// alignment. Returns the text with its baseline's window position.
#[allow(clippy::too_many_arguments)]
pub(crate) fn hud_item(
    a: &MenuAssets,
    it: &LayoutItem,
    rect: [f32; 4],
    fore: [f32; 4],
    material: Option<&str>,
    text: Option<&str>,
    sp: &ScreenPlacement,
    out: &mut Vec<HudQuad>,
) -> Option<(String, [f32; 2])> {
    window(a, it, rect, it.align, fore, material, sp, out);
    let paint = Paint {
        a,
        it,
        rect,
        align: it.align,
        colour: fore,
        sp,
    };
    paint
        .text_item(&localize(a, text?), 0.0, out)
        .map(|(s, p, _)| (s, p))
}

/// `Item_Text_Paint`'s text: `text`, else `textExp`, else the dvar's
/// value.
fn item_text(it: &LayoutItem, env: &impl menuexp::Env, menus: &ScriptMenus) -> Option<String> {
    match (&it.text, it.exps.text.is_empty(), &it.dvar) {
        (Some(t), _, _) => Some(t.clone()),
        (None, false, _) => Some(menuexp::string(&it.exps.text, env).unwrap_or_default()),
        (None, true, Some(d)) => Some(env_dvar(menus, d).unwrap_or_default()),
        (None, true, None) => None,
    }
}

/// `Item_TextField_Paint`: the item's own text, then the
/// field's `value` (from `paintOffset`, at most `maxPaintChars`) 8 past
/// that text's rect, or right after it when the item has no text. Returns
/// the value; the text goes in `texts`.
fn text_field(
    p: &Paint,
    value: String,
    env: &impl menuexp::Env,
    menus: &ScriptMenus,
    out: &mut Vec<HudQuad>,
    texts: &mut Vec<(String, [f32; 2])>,
) -> Option<(String, [f32; 2])> {
    let it = p.it;
    let value: String = match &it.type_data {
        TypeData::Edit(e) if e.max_paint_chars > 0 => {
            value.chars().take(e.max_paint_chars as usize).collect()
        }
        _ => value,
    };
    let label = item_text(it, env, menus).map(|t| localize(p.a, &t));
    // An aligned field keeps the value's width beside its text.
    let extra = if it.text_align & 3 != 0 {
        p.width(&value)
    } else {
        0.0
    };
    let right = match label.and_then(|l| p.text_item(&l, extra, out)) {
        Some((s, at, right)) => {
            texts.push((s, at));
            right
        }
        // No text drawn: the extents are the value's own.
        None => {
            let (_, _, th) = p.font();
            text_rect(it, p.rect, p.width(&value), th, extra)[0] + p.width(&value)
        }
    };
    if value.is_empty() {
        return None;
    }
    let (_, _, th) = p.font();
    let y = text_rect(it, p.rect, 0.0, th, 0.0)[1];
    let gap = if it.text.as_deref().is_some_and(|t| !t.is_empty()) {
        8.0
    } else {
        0.0
    };
    Some(p.text_at(&value, [right + gap, y], out))
}

/// `Item_OwnerDraw_Paint` of a front-end owner draw (`UI_OwnerDraw`,
/// `iw3mp.exe` 0x544820): with no text its `value` sits at the rect's
/// corner plus `textalignx` / `y`, whatever the `textAlignMode`; an item
/// with text draws it first, the value 8 past its rect at `textaligny`
/// under the rect's top. Returns the value; the text goes in `texts`.
fn owner_draw(
    p: &Paint,
    value: String,
    env: &impl menuexp::Env,
    menus: &ScriptMenus,
    out: &mut Vec<HudQuad>,
    texts: &mut Vec<(String, [f32; 2])>,
) -> Option<(String, [f32; 2])> {
    let it = p.it;
    let value = localize(p.a, &value);
    let [x, y, ..] = p.rect;
    let x = match &it.text {
        None => x + it.text_offset[0],
        Some(t) => {
            let label = item_text(it, env, menus).map(|t| localize(p.a, &t));
            let extra = if it.text_align & 3 != 0 {
                p.width(&value)
            } else {
                0.0
            };
            let right = match label.and_then(|l| p.text_item(&l, extra, out)) {
                Some((s, at, right)) => {
                    texts.push((s, at));
                    right
                }
                None => x + it.text_offset[0],
            };
            right + if t.is_empty() { 0.0 } else { 8.0 }
        }
    };
    if value.is_empty() {
        return None;
    }
    Some(p.text_at(&value, [x, y + it.text_offset[1]], out))
}

/// A control's text localized: each `\u{1}`-joined part an `@`-key or
/// plain text, joined by spaces; a `KEY_*` name with no string is the key
/// itself (`GetKeyBindingLocalizedString`).
pub(crate) fn localize(a: &MenuAssets, raw: &str) -> String {
    raw.split('\u{1}')
        .map(|part| match part.strip_prefix('@') {
            Some(key) => {
                let t = a.translate(key);
                match key.strip_prefix("KEY_") {
                    Some(k) if t == key => k.to_string(),
                    _ => t.to_string(),
                }
            }
            None => part.to_string(),
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// `Item_Slider_Paint`: the bar, then the thumb, in the text's colour.
fn slider(
    menus: &ScriptMenus,
    a: &MenuAssets,
    layout: &Layout,
    it: &LayoutItem,
    colour: [f32; 4],
    sp: &ScreenPlacement,
    out: &mut Vec<HudQuad>,
) {
    let Some(s) = menus.slider(it, layout) else {
        return;
    };
    let (_, align) = menus.rect(it, layout);
    for (r, m) in [
        ([s.x, s.y, SLIDER_W, SLIDER_H], "ui_slider2"),
        (
            [s.thumb - 0.5 * THUMB_W, s.y - 2.0, THUMB_W, THUMB_H],
            "ui_sliderbutt_1",
        ),
    ] {
        let [x, y, w, h] = placed(sp, (r, align));
        let tex = a.image(m).unwrap_or_else(|| "white".into());
        out.push(quad(x, y, w, h, colour, tex, false));
    }
}

/// `Item_ListBox_Paint`'s vertical list: from `startPos`, a row each
/// `elementHeight` from 1 in, the picked one filled in `outlineColor`
/// (`Item_ListBox_PaintHighlight`), each column's feeder text at its
/// `pos`, aligned in its width, its baseline the row's bottom plus
/// `textaligny`, until a row no longer fits. Returns each text drawn with
/// its baseline's window position.
#[allow(clippy::too_many_arguments)]
fn list_box(
    menus: &ScriptMenus,
    a: &MenuAssets,
    at: Drawn,
    it: &LayoutItem,
    rect: [f32; 4],
    align: [i32; 2],
    sp: &ScreenPlacement,
    out: &mut Vec<HudQuad>,
) -> Vec<(String, [f32; 2])> {
    let mut texts = Vec::new();
    let TypeData::ListBox(lb) = &it.type_data else {
        return texts;
    };
    // `Item_ListBox_Paint`'s vertical scroll bar: the up arrow, the bar,
    // the down arrow and the thumb, 16 wide, 17 in from the right. A menu
    // under the open one draws its rows only.
    let i = at.i;
    if let Some(bar) = menus.scroll_bar(i).filter(|_| at.top) {
        let x = rect[0] + rect[2] - SCROLL - 1.0;
        let bottom = rect[1] + rect[3] - SCROLL - 1.0;
        for (r, m) in [
            (
                [x, rect[1] + 1.0, SCROLL, SCROLL],
                "ui_scrollbar_arrow_up_a",
            ),
            (
                [x, rect[1] + SCROLL, SCROLL, rect[3] - 2.0 * SCROLL + 1.0],
                "ui_scrollbar",
            ),
            ([x, bottom, SCROLL, SCROLL], "ui_scrollbar_arrow_dwn_a"),
            (
                [x, bar.thumb.min(bottom - SCROLL - 1.0), SCROLL, SCROLL],
                "ui_scrollbar_thumb",
            ),
        ] {
            let [qx, qy, qw, qh] = placed(sp, (r, align));
            let tex = a.image(m).unwrap_or_else(|| "white".into());
            out.push(quad(qx, qy, qw, qh, [1.0; 4], tex, false));
        }
    }
    let count = menus.feeder_count(it.feeder);
    let (start, picked) = at.open.lists.get(&i).copied().unwrap_or((0, 0));
    let eh = lb.element_height;
    let (x, mut y) = (rect[0] + 1.0, rect[1] + 1.0);
    let columns: Vec<[i32; 4]> = if lb.columns.is_empty() {
        vec![[0, lb.element_width as i32, 0, 0]]
    } else {
        lb.columns.clone()
    };
    let f = a.fonts.pick(it.font, it.text_scale, sp);
    let px = it.text_scale / f.unit_scale();
    let th = (48.0 * it.text_scale) as i32 as f32;
    for row in (start..count).take(list_rows(lb, rect[3])) {
        if row == picked {
            let [hx, hy, hw, hh] = placed(sp, ([x + 2.0, y + 2.0, rect[2] - 20.0, eh], align));
            out.push(quad(
                hx,
                hy,
                hw,
                hh,
                it.outline_color,
                "white".into(),
                false,
            ));
        }
        for (c, col) in columns.iter().enumerate() {
            // A column's image fills its cell (`Item_ListBox_PaintTextElem`):
            // the browser's hardware icon.
            if let Some(tex) = menus
                .feeder_image(it.feeder, row, c)
                .and_then(|m| a.image(m))
            {
                let r = [x + col[0] as f32, y, col[1] as f32, eh];
                let [qx, qy, qw, qh] = placed(sp, (r, align));
                out.push(quad(qx, qy, qw, qh, [1.0; 4], tex, false));
                continue;
            }
            let mut t = localize(a, &menus.feeder_text(it.feeder, row, c));
            if col[2] > 0 {
                t = t.chars().take(col[2] as usize).collect();
            }
            if t.is_empty() {
                continue;
            }
            let tw = font::measure(f, &t, px);
            let w = col[1] as f32;
            let adj = match col[3] {
                1 => (0.5 * (w - tw)).max(0.0),
                2 => (w - tw).max(0.0),
                _ => 0.0,
            };
            let tx = x + col[0] as f32 + adj + it.text_offset[0];
            let base = y + eh + it.text_offset[1];
            let (rx, _) = sp.axis(0, tx, tw, align[0]);
            let (ry, _) = sp.axis(1, base - th, th, align[1]);
            let from = out.len();
            text(
                f,
                &t,
                rx,
                ry,
                px * sp.virtual_to_real[1],
                it.fore_color,
                out,
            );
            if !matches!(it.text_style, 3 | 6) {
                let glyphs: Vec<HudQuad> = out.drain(from..).skip(1).step_by(2).collect();
                out.extend(glyphs);
            }
            texts.push((t, [rx, ry + th * sp.virtual_to_real[1]]));
        }
        y += eh;
    }
    texts
}

fn env_dvar(menus: &ScriptMenus, name: &str) -> Option<String> {
    use vcod_cod4::menuexp::Env;
    menus.env().dvar(name)
}

/// `Item_SetTextExtents`: the text's baseline origin on the grid,
/// `textalignx` / `y` off the rect's corner, moved by the horizontal
/// (`textAlignMode & 3`: 1 centre, 2 right) and vertical (`& 0xC`: 4 one
/// line down, 8 centred, 12 at the bottom) alignment.
pub(crate) fn text_origin(it: &LayoutItem, rect: [f32; 4], tw: f32, th: f32) -> [f32; 2] {
    text_rect(it, rect, tw, th, 0.0)
}

/// [`text_origin`] with `extra` more width kept right of the text when it
/// is aligned: a text field's value, an owner draw's.
fn text_rect(it: &LayoutItem, rect: [f32; 4], tw: f32, th: f32, extra: f32) -> [f32; 2] {
    let mut x = it.text_offset[0];
    match it.text_align & 3 {
        1 => x += 0.5 * (rect[2] - tw - extra),
        2 => x += rect[2] - tw - extra,
        _ => {}
    }
    let y0 = it.text_offset[1];
    let y = match it.text_align & 0xc {
        0 => y0,
        4 => y0 + th,
        8 => 0.5 * (rect[3] + th) + y0,
        _ => y0 + rect[3],
    };
    let b = if it.border != 0 { it.border_size } else { 0.0 };
    [x + b + rect[0], y + b + rect[1]]
}

/// `Item_TextColor`: the focused item's text pulses between the menu's
/// `focusColor` and 0.8 of it; a `textStyle` 1 item blinks the same way
/// every other 256 ms.
fn text_colour(
    layout: &Layout,
    it: &LayoutItem,
    fore: [f32; 4],
    focused: bool,
    now: i32,
) -> [f32; 4] {
    let pulse = |c: [f32; 4]| {
        let t = ((now / 75) as f32).sin() * 0.5 + 0.5;
        std::array::from_fn(|k| c[k] + (c[k] * 0.8 - c[k]) * t)
    };
    if focused {
        pulse(layout.focus_color)
    } else if it.text_style != 1 || (now / 256) & 1 != 0 {
        fore
    } else {
        pulse(fore)
    }
}

/// `Window_Paint`: style 1 fills the rect (inside the border) with
/// `backColor`, through the background when there is one; 3, 5 and 6
/// draw the material in `foreColor` when `WINDOW_FORECOLORSET`, else
/// white; then the border.
#[allow(clippy::too_many_arguments)]
fn window(
    a: &MenuAssets,
    it: &LayoutItem,
    rect: [f32; 4],
    align: [i32; 2],
    fore: [f32; 4],
    material: Option<&str>,
    sp: &ScreenPlacement,
    out: &mut Vec<HudQuad>,
) {
    let mut fill = rect;
    if it.border != 0 {
        let b = it.border_size;
        fill = [
            fill[0] + b,
            fill[1] + b,
            fill[2] - (b + 1.0),
            fill[3] - (b + 1.0),
        ];
    }
    // `UI_DrawHandlePic`: a negative width or height flips the picture
    // in place, from the same corner (cod4-hud.md 23).
    let pic = |r: [f32; 4], rgba: [f32; 4], tex: String, out: &mut Vec<HudQuad>| {
        let [x, y, w, h] = placed(sp, ([r[0], r[1], r[2].abs(), r[3].abs()], align));
        let mut q = quad(x, y, w, h, rgba, tex, r[2] < 0.0);
        if r[3] < 0.0 {
            for uv in &mut q.uvs {
                uv[1] = 1.0 - uv[1];
            }
        }
        out.push(q);
    };
    match it.style {
        1 => {
            let tex = material
                .and_then(|m| a.image(m))
                .unwrap_or_else(|| "white".into());
            pic(fill, it.back_color, tex, out);
        }
        3 | 5 | 6 => {
            if let Some(tex) = material.and_then(|m| a.image(m)) {
                let rgba = if it.dynamic_flags & WINDOW_FORECOLORSET != 0 {
                    fore
                } else {
                    [1.0; 4]
                };
                pic(fill, rgba, tex, out);
            }
        }
        _ => {}
    }
    let (b, c) = (it.border_size, it.border_color);
    let [x, y, w, h] = rect;
    let edges: &[[f32; 4]] = match it.border {
        // Full, top and bottom, the sides.
        1 => &[
            [x, y, w, b],
            [x, y + h - b, w, b],
            [x, y, b, h],
            [x + w - b, y, b, h],
        ],
        2 => &[[x, y, w, b], [x, y + h - b, w, b]],
        3 => &[[x, y, b, h], [x + w - b, y, b, h]],
        _ => &[],
    };
    for e in edges {
        pic(*e, c, "white".into(), out);
    }
}

/// The numbered list of a menu vcod does not draw.
fn list(open: &OpenMenu, font: &Font, size: (f32, f32)) -> Vec<HudQuad> {
    let sp = ScreenPlacement::cod4(size.0, size.1);
    let mut out = Vec::new();
    let lines = open.responses.len().min(9);
    let (x, w) = sp.axis(0, LEFT, 640.0 - 2.0 * LEFT, 0);
    let (y, h) = sp.axis(1, TOP - STEP, STEP * (lines as f32 + 2.5), 0);
    out.push(HudQuad {
        verts: [[x, y], [x + w, y], [x + w, y + h], [x, y + h]],
        uvs: [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
        rgba: [0.0, 0.0, 0.0, 0.7],
        texture: "white".into(),
    });
    let px = SCALE / font.unit_scale() * sp.virtual_to_real[1];
    let line = |row: f32, text: &str, rgba: [f32; 4], out: &mut Vec<HudQuad>| {
        let (lx, _) = sp.axis(0, LEFT + 12.0, 0.0, 0);
        let (ly, _) = sp.axis(1, TOP + STEP * row, 0.0, 0);
        font::layout(font, text, lx, ly, px, rgba, out);
    };
    line(
        0.0,
        &format!("{} (list, 1-9 to pick)", open.name),
        [1.0, 0.8, 0.4, 1.0],
        &mut out,
    );
    for (i, r) in open.responses.iter().take(9).enumerate() {
        line(
            i as f32 + 1.0,
            &format!("{}. {}", i + 1, label(r)),
            [1.0; 4],
            &mut out,
        );
    }
    let esc = match open.esc.as_deref() {
        Some(r) => format!("Esc: {}", label(r)),
        None => "Esc: close".into(),
    };
    line(lines as f32 + 1.0, &esc, [0.7, 0.7, 0.7, 1.0], &mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cod4_hud::Cod4Hud;
    use crate::play::cod4_menus::{CS_SCRIPT_MENUS, Join, MenuKey};
    use vcod_cod4::net::parse::Gamestate;

    /// `R_TextLineWrapPosition`'s breaks, a unit a character: at the last
    /// space before the overflow, a longer word whole, a newline always;
    /// a centred text balances its lines (`UI_PickWordWrapLineWidth`).
    /// Fails with the whole new text when `actual` moved off the pin.
    fn pin(tag: &str, actual: &str, expected: &str) {
        assert!(actual == expected, "{tag} moved:\n{actual}");
    }

    #[test]
    fn menu_wrap_pins() {
        // a glyph 5 to 9 units wide by its code
        let m = |t: &str| t.chars().map(|c| (5 + c as u32 % 5) as f32).sum::<f32>();
        let mut texts = crate::cod4_hud::tests::wrap_corpus();
        texts.extend(
            [
                "one\ntwo\n\nfour",
                "\nlead",
                "tail\n",
                "tabs\tand\tspaces between words that wrap",
                "Waiting for teams to be ready, the match begins soon",
            ]
            .map(String::from),
        );
        let mut s = String::new();
        for t in &texts {
            for (w, centred) in [
                (0.0, false),
                (100.0, false),
                (100.0, true),
                (237.5, true),
                (400.0, false),
            ] {
                s += &format!(
                    "{t:?} {w} {centred} => {:?}\n",
                    wrap_lines(t, w, centred, m)
                );
            }
        }
        pin(
            "menu",
            &s,
            r##""" 0 false => []
"" 100 false => []
"" 100 true => []
"" 237.5 true => []
"" 400 false => []
"short" 0 false => ["short"]
"short" 100 false => ["short"]
"short" 100 true => ["short"]
"short" 237.5 true => ["short"]
"short" 400 false => ["short"]
"^" 0 false => ["^"]
"^" 100 false => ["^"]
"^" 100 true => ["^"]
"^" 237.5 true => ["^"]
"^" 400 false => ["^"]
"trailing caret^" 0 false => ["trailing", "caret^"]
"trailing caret^" 100 false => ["trailing", "caret^"]
"trailing caret^" 100 true => ["trailing", "caret^"]
"trailing caret^" 237.5 true => ["trailing caret^"]
"trailing caret^" 400 false => ["trailing caret^"]
"^^1 caret pair ^x not a code" 0 false => ["^^1", "caret", "pair", "^x", "not", "a", "code"]
"^^1 caret pair ^x not a code" 100 false => ["^^1 caret", "pair ^x not a", "code"]
"^^1 caret pair ^x not a code" 100 true => ["^^1 caret", "pair ^x not a", "code"]
"^^1 caret pair ^x not a code" 237.5 true => ["^^1 caret pair ^x not a code"]
"^^1 caret pair ^x not a code" 400 false => ["^^1 caret pair ^x not a code"]
"  leading and trailing spaces  " 0 false => [" ", "leading", "and", "trailing", "spaces"]
"  leading and trailing spaces  " 100 false => ["  leading and", "trailing spaces"]
"  leading and trailing spaces  " 100 true => ["  leading and", "trailing", "spaces  "]
"  leading and trailing spaces  " 237.5 true => ["  leading and trailing spaces  "]
"  leading and trailing spaces  " 400 false => ["  leading and trailing spaces  "]
"^1word word word word word word word word word word word word  tail" 0 false => ["^1word", "word", "word", "word", "word", "word", "word", "word", "word", "word", "word", "word", "tail"]
"^1word word word word word word word word word word word word  tail" 100 false => ["^1word word", "word word", "word word", "word word", "word word", "word word ", "tail"]
"^1word word word word word word word word word word word word  tail" 100 true => ["^1word word", "word word", "word word", "word word", "word word", "word word ", "tail"]
"^1word word word word word word word word word word word word  tail" 237.5 true => ["^1word word word word", "word word word word word", "word word word  tail"]
"^1word word word word word word word word word word word word  tail" 400 false => ["^1word word word word word word word word word word", "word word  tail"]
"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa" 0 false => ["aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"]
"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa" 100 false => ["aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"]
"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa" 100 true => ["aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"]
"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa" 237.5 true => ["aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"]
"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa" 400 false => ["aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"]
"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa" 0 false => ["aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"]
"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa" 100 false => ["aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"]
"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa" 100 true => ["aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"]
"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa" 237.5 true => ["aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"]
"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa" 400 false => ["aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"]
"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa" 0 false => ["aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"]
"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa" 100 false => ["aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"]
"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa" 100 true => ["aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"]
"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa" 237.5 true => ["aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"]
"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa" 400 false => ["aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"]
"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa" 0 false => ["aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"]
"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa" 100 false => ["aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"]
"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa" 100 true => ["aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"]
"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa" 237.5 true => ["aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"]
"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa" 400 false => ["aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"]
"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa" 0 false => ["aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"]
"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa" 100 false => ["aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"]
"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa" 100 true => ["aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"]
"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa" 237.5 true => ["aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"]
"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa" 400 false => ["aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"]
"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa" 0 false => ["aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"]
"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa" 100 false => ["aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"]
"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa" 100 true => ["aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"]
"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa" 237.5 true => ["aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"]
"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa" 400 false => ["aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"]
"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa" 0 false => ["aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"]
"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa" 100 false => ["aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"]
"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa" 100 true => ["aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"]
"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa" 237.5 true => ["aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"]
"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa" 400 false => ["aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"]
"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa" 0 false => ["aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"]
"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa" 100 false => ["aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"]
"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa" 100 true => ["aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"]
"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa" 237.5 true => ["aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"]
"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa" 400 false => ["aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"]
"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa" 0 false => ["aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"]
"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa" 100 false => ["aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"]
"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa" 100 true => ["aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"]
"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa" 237.5 true => ["aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"]
"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa" 400 false => ["aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"]
"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa" 0 false => ["aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"]
"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa" 100 false => ["aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"]
"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa" 100 true => ["aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"]
"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa" 237.5 true => ["aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"]
"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa" 400 false => ["aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"]
"xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx yyyyyyyyyyyyyyyyyyyyyyyyyyyyyy" 0 false => ["xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx", "yyyyyyyyyyyyyyyyyyyyyyyyyyyyyy"]
"xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx yyyyyyyyyyyyyyyyyyyyyyyyyyyyyy" 100 false => ["xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx", "yyyyyyyyyyyyyyyyyyyyyyyyyyyyyy"]
"xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx yyyyyyyyyyyyyyyyyyyyyyyyyyyyyy" 100 true => ["xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx", "yyyyyyyyyyyyyyyyyyyyyyyyyyyyyy"]
"xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx yyyyyyyyyyyyyyyyyyyyyyyyyyyyyy" 237.5 true => ["xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx", "yyyyyyyyyyyyyyyyyyyyyyyyyyyyyy"]
"xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx yyyyyyyyyyyyyyyyyyyyyyyyyyyyyy" 400 false => ["xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx", "yyyyyyyyyyyyyyyyyyyyyyyyyyyyyy"]
"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa ^3bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb" 0 false => ["aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", "^3bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"]
"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa ^3bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb" 100 false => ["aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", "^3bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"]
"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa ^3bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb" 100 true => ["aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", "^3bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"]
"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa ^3bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb" 237.5 true => ["aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", "^3bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"]
"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa ^3bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb" 400 false => ["aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", "^3bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"]
"^1a^2b^3c^4d^5e^6f^7g^8h^9i^0j ^1a^2b^3c^4d^5e^6f^7g^8h^9i^0j ^1a^2b^3c^4d^5e^6f^7g^8h^9i^0j ^1a^2b^3c^4d^5e^6f^7g^8h^9i^0j ^1a^2b^3c^4d^5e^6f^7g^8h^9i^0j ^1a^2b^3c^4d^5e^6f^7g^8h^9i^0j " 0 false => ["^1a^2b^3c^4d^5e^6f^7g^8h^9i^0j", "^1a^2b^3c^4d^5e^6f^7g^8h^9i^0j", "^1a^2b^3c^4d^5e^6f^7g^8h^9i^0j", "^1a^2b^3c^4d^5e^6f^7g^8h^9i^0j", "^1a^2b^3c^4d^5e^6f^7g^8h^9i^0j", "^1a^2b^3c^4d^5e^6f^7g^8h^9i^0j"]
"^1a^2b^3c^4d^5e^6f^7g^8h^9i^0j ^1a^2b^3c^4d^5e^6f^7g^8h^9i^0j ^1a^2b^3c^4d^5e^6f^7g^8h^9i^0j ^1a^2b^3c^4d^5e^6f^7g^8h^9i^0j ^1a^2b^3c^4d^5e^6f^7g^8h^9i^0j ^1a^2b^3c^4d^5e^6f^7g^8h^9i^0j " 100 false => ["^1a^2b^3c^4d^5e^6f^7g^8h^9i^0j", "^1a^2b^3c^4d^5e^6f^7g^8h^9i^0j", "^1a^2b^3c^4d^5e^6f^7g^8h^9i^0j", "^1a^2b^3c^4d^5e^6f^7g^8h^9i^0j", "^1a^2b^3c^4d^5e^6f^7g^8h^9i^0j", "^1a^2b^3c^4d^5e^6f^7g^8h^9i^0j"]
"^1a^2b^3c^4d^5e^6f^7g^8h^9i^0j ^1a^2b^3c^4d^5e^6f^7g^8h^9i^0j ^1a^2b^3c^4d^5e^6f^7g^8h^9i^0j ^1a^2b^3c^4d^5e^6f^7g^8h^9i^0j ^1a^2b^3c^4d^5e^6f^7g^8h^9i^0j ^1a^2b^3c^4d^5e^6f^7g^8h^9i^0j " 100 true => ["^1a^2b^3c^4d^5e^6f^7g^8h^9i^0j", "^1a^2b^3c^4d^5e^6f^7g^8h^9i^0j", "^1a^2b^3c^4d^5e^6f^7g^8h^9i^0j", "^1a^2b^3c^4d^5e^6f^7g^8h^9i^0j", "^1a^2b^3c^4d^5e^6f^7g^8h^9i^0j", "^1a^2b^3c^4d^5e^6f^7g^8h^9i^0j"]
"^1a^2b^3c^4d^5e^6f^7g^8h^9i^0j ^1a^2b^3c^4d^5e^6f^7g^8h^9i^0j ^1a^2b^3c^4d^5e^6f^7g^8h^9i^0j ^1a^2b^3c^4d^5e^6f^7g^8h^9i^0j ^1a^2b^3c^4d^5e^6f^7g^8h^9i^0j ^1a^2b^3c^4d^5e^6f^7g^8h^9i^0j " 237.5 true => ["^1a^2b^3c^4d^5e^6f^7g^8h^9i^0j", "^1a^2b^3c^4d^5e^6f^7g^8h^9i^0j", "^1a^2b^3c^4d^5e^6f^7g^8h^9i^0j", "^1a^2b^3c^4d^5e^6f^7g^8h^9i^0j", "^1a^2b^3c^4d^5e^6f^7g^8h^9i^0j", "^1a^2b^3c^4d^5e^6f^7g^8h^9i^0j "]
"^1a^2b^3c^4d^5e^6f^7g^8h^9i^0j ^1a^2b^3c^4d^5e^6f^7g^8h^9i^0j ^1a^2b^3c^4d^5e^6f^7g^8h^9i^0j ^1a^2b^3c^4d^5e^6f^7g^8h^9i^0j ^1a^2b^3c^4d^5e^6f^7g^8h^9i^0j ^1a^2b^3c^4d^5e^6f^7g^8h^9i^0j " 400 false => ["^1a^2b^3c^4d^5e^6f^7g^8h^9i^0j", "^1a^2b^3c^4d^5e^6f^7g^8h^9i^0j", "^1a^2b^3c^4d^5e^6f^7g^8h^9i^0j", "^1a^2b^3c^4d^5e^6f^7g^8h^9i^0j", "^1a^2b^3c^4d^5e^6f^7g^8h^9i^0j", "^1a^2b^3c^4d^5e^6f^7g^8h^9i^0j "]
"ccccccccccccccccccccccccccccccccccccccccccccc   ddddddddddddddddddddddddddddddddddddddddddddd" 0 false => ["ccccccccccccccccccccccccccccccccccccccccccccc", "ddddddddddddddddddddddddddddddddddddddddddddd"]
"ccccccccccccccccccccccccccccccccccccccccccccc   ddddddddddddddddddddddddddddddddddddddddddddd" 100 false => ["ccccccccccccccccccccccccccccccccccccccccccccc", "ddddddddddddddddddddddddddddddddddddddddddddd"]
"ccccccccccccccccccccccccccccccccccccccccccccc   ddddddddddddddddddddddddddddddddddddddddddddd" 100 true => ["ccccccccccccccccccccccccccccccccccccccccccccc", "ddddddddddddddddddddddddddddddddddddddddddddd"]
"ccccccccccccccccccccccccccccccccccccccccccccc   ddddddddddddddddddddddddddddddddddddddddddddd" 237.5 true => ["ccccccccccccccccccccccccccccccccccccccccccccc", "ddddddddddddddddddddddddddddddddddddddddddddd"]
"ccccccccccccccccccccccccccccccccccccccccccccc   ddddddddddddddddddddddddddddddddddddddddddddd" 400 false => ["ccccccccccccccccccccccccccccccccccccccccccccc", "ddddddddddddddddddddddddddddddddddddddddddddd"]
"cccccccccccccccccccccccccccccc dddddddddddddddddddddddddddddddddddddddd" 0 false => ["cccccccccccccccccccccccccccccc", "dddddddddddddddddddddddddddddddddddddddd"]
"cccccccccccccccccccccccccccccc dddddddddddddddddddddddddddddddddddddddd" 100 false => ["cccccccccccccccccccccccccccccc", "dddddddddddddddddddddddddddddddddddddddd"]
"cccccccccccccccccccccccccccccc dddddddddddddddddddddddddddddddddddddddd" 100 true => ["cccccccccccccccccccccccccccccc", "dddddddddddddddddddddddddddddddddddddddd"]
"cccccccccccccccccccccccccccccc dddddddddddddddddddddddddddddddddddddddd" 237.5 true => ["cccccccccccccccccccccccccccccc", "dddddddddddddddddddddddddddddddddddddddd"]
"cccccccccccccccccccccccccccccc dddddddddddddddddddddddddddddddddddddddd" 400 false => ["cccccccccccccccccccccccccccccc", "dddddddddddddddddddddddddddddddddddddddd"]
"Привет мир, это длинное сообщение Привет мир, это длинное сообщение Привет мир, это длинное сообщение " 0 false => ["Привет", "мир,", "это", "длинное", "сообщение", "Привет", "мир,", "это", "длинное", "сообщение", "Привет", "мир,", "это", "длинное", "сообщение"]
"Привет мир, это длинное сообщение Привет мир, это длинное сообщение Привет мир, это длинное сообщение " 100 false => ["Привет мир,", "это длинное", "сообщение", "Привет мир,", "это длинное", "сообщение", "Привет мир,", "это длинное", "сообщение "]
"Привет мир, это длинное сообщение Привет мир, это длинное сообщение Привет мир, это длинное сообщение " 100 true => ["Привет мир,", "это длинное", "сообщение", "Привет мир,", "это длинное", "сообщение", "Привет мир,", "это длинное", "сообщение "]
"Привет мир, это длинное сообщение Привет мир, это длинное сообщение Привет мир, это длинное сообщение " 237.5 true => ["Привет мир, это длинное сообщение", "Привет мир, это длинное сообщение", "Привет мир, это длинное сообщение "]
"Привет мир, это длинное сообщение Привет мир, это длинное сообщение Привет мир, это длинное сообщение " 400 false => ["Привет мир, это длинное сообщение Привет мир, это длинное", "сообщение Привет мир, это длинное сообщение "]
"日本語のテキストは空白がない日本語のテキストは空白がない日本語のテキストは空白がない日本語のテキストは空白がない" 0 false => ["日本語のテキストは空白がない日本語のテキストは空白がない日本語のテキストは空白がない日本語のテキストは空白がない"]
"日本語のテキストは空白がない日本語のテキストは空白がない日本語のテキストは空白がない日本語のテキストは空白がない" 100 false => ["日本語のテキストは空白がない日本語のテキストは空白がない日本語のテキストは空白がない日本語のテキストは空白がない"]
"日本語のテキストは空白がない日本語のテキストは空白がない日本語のテキストは空白がない日本語のテキストは空白がない" 100 true => ["日本語のテキストは空白がない日本語のテキストは空白がない日本語のテキストは空白がない日本語のテキストは空白がない"]
"日本語のテキストは空白がない日本語のテキストは空白がない日本語のテキストは空白がない日本語のテキストは空白がない" 237.5 true => ["日本語のテキストは空白がない日本語のテキストは空白がない日本語のテキストは空白がない日本語のテキストは空白がない"]
"日本語のテキストは空白がない日本語のテキストは空白がない日本語のテキストは空白がない日本語のテキストは空白がない" 400 false => ["日本語のテキストは空白がない日本語のテキストは空白がない日本語のテキストは空白がない日本語のテキストは空白がない"]
"eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeäffffffffff" 0 false => ["eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeäffffffffff"]
"eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeäffffffffff" 100 false => ["eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeäffffffffff"]
"eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeäffffffffff" 100 true => ["eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeäffffffffff"]
"eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeäffffffffff" 237.5 true => ["eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeäffffffffff"]
"eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeäffffffffff" 400 false => ["eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeäffffffffff"]
"^2Shooter^7: ^7gg, nice shot from across the whole map, really nice one" 0 false => ["^2Shooter^7:", "^7gg,", "nice", "shot", "from", "across", "the", "whole", "map,", "really", "nice", "one"]
"^2Shooter^7: ^7gg, nice shot from across the whole map, really nice one" 100 false => ["^2Shooter^7:", "^7gg, nice", "shot from", "across the", "whole map,", "really nice one"]
"^2Shooter^7: ^7gg, nice shot from across the whole map, really nice one" 100 true => ["^2Shooter^7:", "^7gg, nice", "shot from", "across the", "whole map,", "really nice one"]
"^2Shooter^7: ^7gg, nice shot from across the whole map, really nice one" 237.5 true => ["^2Shooter^7: ^7gg, nice", "shot from across the", "whole map, really nice one"]
"^2Shooter^7: ^7gg, nice shot from across the whole map, really nice one" 400 false => ["^2Shooter^7: ^7gg, nice shot from across the whole map,", "really nice one"]
"one\ntwo\n\nfour" 0 false => ["one", "two", "four"]
"one\ntwo\n\nfour" 100 false => ["one", "two", "four"]
"one\ntwo\n\nfour" 100 true => ["one", "two", "four"]
"one\ntwo\n\nfour" 237.5 true => ["one", "two", "four"]
"one\ntwo\n\nfour" 400 false => ["one", "two", "four"]
"\nlead" 0 false => ["", "lead"]
"\nlead" 100 false => ["", "lead"]
"\nlead" 100 true => ["", "lead"]
"\nlead" 237.5 true => ["", "lead"]
"\nlead" 400 false => ["", "lead"]
"tail\n" 0 false => ["tail"]
"tail\n" 100 false => ["tail"]
"tail\n" 100 true => ["tail"]
"tail\n" 237.5 true => ["tail"]
"tail\n" 400 false => ["tail"]
"tabs\tand\tspaces between words that wrap" 0 false => ["tabs", "and", "spaces", "between", "words", "that", "wrap"]
"tabs\tand\tspaces between words that wrap" 100 false => ["tabs\tand\tspaces", "between words", "that wrap"]
"tabs\tand\tspaces between words that wrap" 100 true => ["tabs\tand\tspaces", "between words", "that wrap"]
"tabs\tand\tspaces between words that wrap" 237.5 true => ["tabs\tand\tspaces between", "words that wrap"]
"tabs\tand\tspaces between words that wrap" 400 false => ["tabs\tand\tspaces between words that wrap"]
"Waiting for teams to be ready, the match begins soon" 0 false => ["Waiting", "for", "teams", "to", "be", "ready,", "the", "match", "begins", "soon"]
"Waiting for teams to be ready, the match begins soon" 100 false => ["Waiting for", "teams to be", "ready, the", "match begins", "soon"]
"Waiting for teams to be ready, the match begins soon" 100 true => ["Waiting for", "teams to be", "ready, the", "match begins", "soon"]
"Waiting for teams to be ready, the match begins soon" 237.5 true => ["Waiting for teams to be", "ready, the match begins soon"]
"Waiting for teams to be ready, the match begins soon" 400 false => ["Waiting for teams to be ready, the match begins soon"]
"##,
        );
    }

    #[test]
    fn wrapped_text_breaks_at_spaces_and_balances_centred_lines() {
        let m = |t: &str| t.len() as f32;
        assert_eq!(wrap_lines("aaa bbb ccc", 7.0, false, m), ["aaa bbb", "ccc"]);
        assert_eq!(
            wrap_lines("aaaaaaaaa bb", 4.0, false, m),
            ["aaaaaaaaa", "bb"]
        );
        assert_eq!(wrap_lines("ab\ncd", 40.0, false, m), ["ab", "cd"]);
        assert_eq!(wrap_lines("short", 40.0, true, m), ["short"]);
        // 23 wide in 20 is two lines; centred, the even split's 12 keeps two.
        let s = "a b c d e f g h i j k l";
        assert_eq!(
            wrap_lines(s, 20.0, false, m),
            ["a b c d e f g h i j", "k l"]
        );
        assert_eq!(wrap_lines(s, 20.0, true, m), ["a b c d e f", "g h i j k l"]);
    }

    fn gamestate() -> Gamestate {
        let mut gs = Gamestate::default();
        gs.configstrings.insert(1, b"\\sv_serverid\\16".to_vec());
        for (n, m) in [
            (8, "team_marinesopfor"),
            (10, "changeclass_marines"),
            (15, "changeclass"),
        ] {
            gs.configstrings
                .insert(CS_SCRIPT_MENUS + n, m.as_bytes().to_vec());
        }
        gs
    }

    /// The team and class menus drawn from `common_mp` on a 640x480
    /// window: the team menu's buttons in their zone rects (centred
    /// x -320..-100, rows 24 apart from y 204, text right-aligned 10 in
    /// and centred down the row), `onOpen`'s `setfocus auto_assign`, and
    /// a click on the Marines row answers `allies`; the class menu
    /// `changeclass_marines` opens shows the five default classes, the
    /// locked ones greyed for a fresh profile, and a click on the first
    /// answers `assault_mp,0` from slot 15 and closes it.
    #[test]
    fn cod4_team_and_class_menus_draw_from_the_zones() {
        let Some(install) = vcod_cod4::testing::cod4_install() else {
            return;
        };
        let mut hud = Cod4Hud::default();
        hud.load(&install.root().join("main"), "mp_crash");
        let (defs, tables) = hud.script_menus().unwrap();
        let a = hud.menu_assets().unwrap();
        let gs = gamestate();
        let mut m = ScriptMenus::new(Join::default());
        m.set_defs(defs.clone(), tables.clone());
        m.mapname = "mp_crash".into();
        for (k, v) in [
            ("g_teamname_allies", "MPUI_MARINES_SHORT"),
            ("g_teamname_axis", "MPUI_OPFOR_SHORT"),
            ("g_teamicon_allies", "faction_128_usmc"),
            ("g_teamicon_axis", "faction_128_arab"),
        ] {
            m.mirror.insert(k.into(), v.into());
        }
        let size = (640.0, 480.0);
        assert!(m.command(b"t 8", &gs).is_empty());
        let open = m.open().unwrap();
        assert_eq!(open.name, "team_marinesopfor");
        let layout = open.layout.as_ref().expect("the team menu evaluates");
        let focus = open.focus.expect("onOpen focuses auto_assign");
        assert_eq!(layout.items[focus].name, "auto_assign");
        let (quads, texts) = paint(&m, &a, size, 0, true);
        assert!(!quads.is_empty());
        let at = |t: &str| {
            texts
                .iter()
                .find(|(s, _)| s == t)
                .map(|(_, p)| *p)
                .unwrap_or_else(|| panic!("no {t:?} in {texts:?}"))
        };
        let tw = |t: &str, scale: f32| {
            let f = a
                .fonts
                .pick(1, scale, &ScreenPlacement::new(size.0, size.1));
            font::measure(f, t, scale / f.unit_scale())
        };
        // Right-aligned 10 in from x 220 (-100 centred), baselines on
        // (22 + 19) / 2 into each row, 19 the 0.4 scale's line height.
        let th = (48.0 * 0.4f32) as i32 as f32;
        for (row, t) in [(0, "OpFor"), (1, "Marines"), (2, "Auto-Assign")] {
            let p = at(t);
            assert!(
                (p[0] - (220.0 - 10.0 - tw(t, 0.4))).abs() < 0.01,
                "{t} x {p:?}"
            );
            let y = 204.0 + 24.0 * row as f32 + 0.5 * (22.0 + th);
            assert!((p[1] - y).abs() < 0.01, "{t} y {p:?}");
        }
        at("CHOOSE TEAM");
        at("Crash");
        assert_eq!(at("Back"), [70.0, 472.0]);
        // A free player sees no Controls row (spectators only).
        assert!(texts.iter().all(|(s, _)| s != "Controls"), "{texts:?}");
        // The mouse onto the Marines row moves the focus there; MOUSE1
        // answers it. The team menu's buttons do not close it; the server
        // does.
        m.mouse_move(None, Some([110.0, 239.0]), size, &gs);
        let f = m.open().unwrap().focus.unwrap();
        assert_eq!(m.open().unwrap().layout.as_ref().unwrap().items[f].kind, 1);
        assert_eq!(m.click(size, &gs), ["mr 16 8 allies"]);
        // Up from there is the OpFor row; Enter answers it.
        let mut out = Vec::new();
        assert!(m.key(MenuKey::Prev, &gs, &mut out));
        assert!(m.key(MenuKey::Enter, &gs, &mut out));
        assert_eq!(out, ["mr 16 8 axis"]);
        assert!(m.command(b"u", &gs).is_empty());
        // The class pick.
        m.team = 2;
        assert!(m.command(b"t 10", &gs).is_empty());
        let open = m.open().unwrap();
        assert_eq!((open.name.as_str(), open.slot), ("changeclass", 15));
        assert!(open.layout.is_some(), "the class menu evaluates");
        let (_, texts) = paint(&m, &a, size, 0, true);
        let names: Vec<&str> = texts.iter().map(|(s, _)| s.as_str()).collect();
        for t in [
            "Assault",
            "Spec Ops",
            "Heavy Gunner",
            "Demolitions",
            "Sniper",
            "M16A4",
        ] {
            assert!(names.contains(&t), "{t} in {names:?}");
        }
        // The highlighted class's perks draw as their icons (the items
        // whose material reads `mp/statstable.csv`'s column 6 off the
        // class's stats 205..207): Assault's Stopping Power and Extreme
        // Conditioning; its first, `specialty_null`, draws nothing.
        let (quads, _) = paint(&m, &a, size, 0, true);
        for perk in ["specialty_bulletdamage", "specialty_longersprint"] {
            let tex = hud.texture_of(perk).unwrap();
            assert!(quads.iter().any(|q| q.texture == tex), "{perk}");
        }
        // Demolitions (stat 257 0) is the greyed text, no button.
        m.mouse_move(None, Some([110.0, 141.0]), size, &gs);
        assert!(m.click(size, &gs).is_empty());
        m.mouse_move(None, Some([110.0, 69.0]), size, &gs);
        assert_eq!(m.click(size, &gs), ["mr 16 15 assault_mp,0"]);
        assert!(m.open().is_none());
        // Offline, `changeclass_marines` opens `changeclass_offline`.
        m.ui.dvars.insert("onlinegame".into(), "0".into());
        assert!(m.command(b"t 10", &gs).is_empty());
        let open = m.open().unwrap();
        assert_eq!(open.name, "changeclass_offline");
        assert!(open.layout.is_some(), "the offline class menu evaluates");
        assert!(!paint(&m, &a, size, 0, true).1.is_empty());
    }

    /// The team menu's Spectator row reads `dvarBool(
    /// scr_game_spectatetype )` out of the client's own dvars, which its
    /// startup `default_mp_gamesettings.cfg` sets 1; retail's server never
    /// mirrors it (`cod4_boot`'s capture). Hidden without the configs,
    /// drawn with them, and a click answers `spectator`.
    #[test]
    fn cod4_team_menu_spectator_reads_the_startup_configs() {
        let Some(install) = vcod_cod4::testing::cod4_install() else {
            return;
        };
        let mut hud = Cod4Hud::default();
        hud.load(&install.root().join("main"), "mp_crash");
        let (defs, tables) = hud.script_menus().unwrap();
        let a = hud.menu_assets().unwrap();
        let gs = gamestate();
        let mut m = ScriptMenus::new(Join::default());
        m.set_defs(defs.clone(), tables.clone());
        m.mapname = "mp_crash".into();
        let size = (640.0, 480.0);
        let shown = |m: &ScriptMenus| {
            let (_, texts) = paint(m, &a, size, 0, true);
            texts.iter().any(|(s, _)| s == "Spectator")
        };
        assert!(m.command(b"t 8", &gs).is_empty());
        assert!(!shown(&m), "no configs, no Spectator");
        m.startup = hud.startup_dvars().unwrap().clone();
        assert_eq!(m.dvar("scr_game_spectatetype").as_deref(), Some("1"));
        assert!(shown(&m));
        m.mouse_move(None, Some([110.0, 297.0]), size, &gs);
        assert_eq!(m.click(size, &gs), ["mr 16 8 spectator"]);
        // A spectator's own menu has no Spectator row.
        m.team = 3;
        assert!(!shown(&m));
    }

    /// Esc in a game: retail's server leaves `g_scriptMainMenu` at
    /// `class_marines` after the team pick (the prematch capture's `v`),
    /// whose `onOpen` opens `class`, drawn from the zone: the game type's
    /// name and objective over the rows, and each row's script: Change
    /// Team answers `changeteam` from `class`'s slot, Leave Game opens
    /// `popup_leavegame`, whose Yes runs `disconnect` on the client's
    /// console (cod4-hud.md 12.5).
    /// A game's menus play their `play` aliases as the front end's do
    /// (cod4-hud.md 23): the `class` menu's row under the cursor plays
    /// `mouse_over` (`onFocus`), a click `mouse_click`, and the menus'
    /// three aliases are in `localized_code_post_gfx_mp`, a zone a game
    /// loads its sounds from (`cod4_fly::SOUND_ZONES`).
    #[test]
    fn cod4_in_game_menus_play_their_sounds() {
        let Some(install) = vcod_cod4::testing::cod4_install() else {
            return;
        };
        let mut hud = Cod4Hud::default();
        hud.load(&install.root().join("main"), "mp_crash");
        let (defs, tables) = hud.script_menus().unwrap();
        let a = hud.menu_assets().unwrap();
        let mut gs = Gamestate::default();
        gs.configstrings.insert(1, b"\\sv_serverid\\16".to_vec());
        for (n, name) in [(9, "class_marines"), (14, "class")] {
            gs.configstrings
                .insert(CS_SCRIPT_MENUS + n, name.as_bytes().to_vec());
        }
        let mut m = ScriptMenus::new(Join::default());
        m.set_defs(defs.clone(), tables.clone());
        m.team = 2;
        m.command(b"v g_scriptMainMenu \"class_marines\"", &gs);
        m.open_main(&gs);
        m.take_sounds();
        let size = (640.0, 480.0);
        let (_, texts) = paint(&m, &a, size, 0, true);
        let p = texts.iter().find(|(s, _)| s == "Options").unwrap().1;
        m.mouse_move(None, Some([p[0] + 2.0, p[1] - 4.0]), size, &gs);
        let over = m.take_sounds();
        m.click(size, &gs);
        let click = m.take_sounds();
        assert_eq!(over, ["mouse_over"]);
        assert!(click.iter().any(|s| s == "mouse_click"), "{click:?}");
        let path = install.zone("english", "localized_code_post_gfx_mp");
        let z = vcod_cod4::zone::Zone::open_path(&path).unwrap();
        let assets = vcod_cod4::walk::walk(z, Default::default()).unwrap().assets;
        let mut set = vcod_cod4::sound::SoundSet::default();
        set.add(&assets);
        for alias in ["mouse_over", "mouse_click", "mouse_submenu_over"] {
            assert!(set.aliases.contains_key(alias), "{alias}");
        }
    }

    #[test]
    fn cod4_esc_in_game_draws_the_class_menu() {
        let Some(install) = vcod_cod4::testing::cod4_install() else {
            return;
        };
        let mut hud = Cod4Hud::default();
        hud.load(&install.root().join("main"), "mp_crash");
        let (defs, tables) = hud.script_menus().unwrap();
        let a = hud.menu_assets().unwrap();
        let mut gs = Gamestate::default();
        gs.configstrings.insert(1, b"\\sv_serverid\\16".to_vec());
        for (n, name) in [(9, "class_marines"), (14, "class"), (15, "changeclass")] {
            gs.configstrings
                .insert(CS_SCRIPT_MENUS + n, name.as_bytes().to_vec());
        }
        let mut m = ScriptMenus::new(Join::default());
        m.set_defs(defs.clone(), tables.clone());
        m.mapname = "mp_crash".into();
        m.team = 2;
        m.gametype = "war".into();
        m.gametype_name = "Team Deathmatch".into();
        m.objective = "Score 750 points for your team.".into();
        m.mirror
            .insert("g_teamname_allies".into(), "MPUI_MARINES_SHORT".into());
        assert!(
            m.command(b"v g_scriptMainMenu \"class_marines\"", &gs)
                .is_empty()
        );
        assert!(m.open_main(&gs).is_empty());
        let open = m.open().unwrap();
        assert_eq!((open.name.as_str(), open.slot), ("class", 14));
        assert!(open.layout.is_some(), "the class menu evaluates");
        let size = (640.0, 480.0);
        let (_, texts) = paint(&m, &a, size, 0, true);
        let names: Vec<&str> = texts.iter().map(|(s, _)| s.as_str()).collect();
        for t in [
            "Team Deathmatch",
            "Score 750 points for your team.",
            "Choose Class",
            "Change Team",
            "Controls",
            "Options",
            "Mute Players",
            "Leave Game",
        ] {
            assert!(names.contains(&t), "{t} in {names:?}");
        }
        let row = |m: &mut ScriptMenus, label: &str| {
            let (_, texts) = paint(m, &a, size, 0, true);
            let p = texts.iter().find(|(s, _)| s == label).unwrap().1;
            m.mouse_move(None, Some([p[0] + 2.0, p[1] - 4.0]), size, &gs);
            m.click(size, &gs)
        };
        assert_eq!(row(&mut m, "Change Team"), ["mr 16 14 changeteam"]);
        assert_eq!(
            row(&mut m, "Choose Class"),
            ["mr 16 14 changeclass_marines"]
        );
        assert!(row(&mut m, "Leave Game").is_empty());
        assert_eq!(m.open().unwrap().name, "popup_leavegame");
        assert!(m.open().unwrap().layout.is_some());
        assert!(row(&mut m, "Yes").is_empty());
        assert!(m.open().is_none());
        assert_eq!(m.take_console(), ["disconnect"]);
        // Controls, Options and Mute Players close `class` and open their
        // own zone menus, drawn too.
        for (label, menu) in [
            ("Controls", "main_controls"),
            ("Options", "main_options"),
            ("Mute Players", "muteplayer"),
        ] {
            m.open_main(&gs);
            assert!(row(&mut m, label).is_empty());
            let open = m.open().unwrap();
            assert_eq!(open.name, menu);
            assert!(open.layout.is_some(), "{menu} evaluates");
            m.command(b"u", &gs);
        }
        // Esc backs out of the menu as its `onEsc` does: `back`.
        m.open_main(&gs);
        assert_eq!(m.escape(&gs), ["mr 16 14 back"]);
    }

    /// The in-game menu as the Esc test opens it, with vcod's stock CoD4
    /// console (cvars and binds) as the host and two players listed.
    fn in_game(hud: &Cod4Hud) -> (ScriptMenus, Gamestate) {
        let (defs, tables) = hud.script_menus().unwrap();
        let mut gs = Gamestate::default();
        gs.configstrings.insert(1, b"\\sv_serverid\\16".to_vec());
        for (n, name) in [(9, "class_marines"), (14, "class"), (15, "changeclass")] {
            gs.configstrings
                .insert(CS_SCRIPT_MENUS + n, name.as_bytes().to_vec());
        }
        let mut m = ScriptMenus::new(Join::default());
        m.set_defs(defs.clone(), tables.clone());
        m.set_lists(hud.ui_lists().unwrap().clone());
        m.mapname = "mp_crash".into();
        m.team = 2;
        m.gametype = "war".into();
        m.mirror.insert("ui_allowvote".into(), "1".into());
        let mut shell = crate::console::shell::Shell::new();
        shell.use_cod2_binds();
        shell.use_cod4_binds();
        m.host = host(&shell);
        m.host.players = vec![(0, "^9walker".into()), (3, "probe".into())];
        m.command(b"v g_scriptMainMenu \"class_marines\"", &gs);
        (m, gs)
    }

    fn host(shell: &crate::console::shell::Shell) -> crate::play::cod4_menus::Host {
        crate::play::cod4_menus::Host {
            cvars: shell
                .cvar_values()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            enums: shell
                .enum_domains()
                .map(|(k, v)| (k.to_string(), v.to_vec()))
                .collect(),
            binds: shell
                .binds()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            players: Vec::new(),
        }
    }

    const SIZE: (f32, f32) = (640.0, 480.0);

    /// The drawn texts of the open menu.
    fn texts(m: &ScriptMenus, a: &MenuAssets) -> Vec<(String, [f32; 2])> {
        paint(m, a, SIZE, 0, true).1
    }

    /// Moves the cursor onto the drawn text `label` and clicks.
    fn click(m: &mut ScriptMenus, a: &MenuAssets, gs: &Gamestate, label: &str) -> Vec<String> {
        let t = texts(m, a);
        let p = t
            .iter()
            .find(|(s, _)| s == label)
            .unwrap_or_else(|| panic!("no {label:?} in {t:?}"))
            .1;
        m.mouse_move(None, Some([p[0] + 2.0, p[1] - 4.0]), SIZE, gs);
        m.click(SIZE, gs)
    }

    /// The text drawn on the row whose label is `label` (the control's
    /// value, right of it).
    fn value_of(m: &ScriptMenus, a: &MenuAssets, label: &str) -> String {
        let t = texts(m, a);
        let y = t.iter().find(|(s, _)| s == label).unwrap().1[1];
        let x = t.iter().find(|(s, _)| s == label).unwrap().1[0];
        t.iter()
            .filter(|(s, p)| (p[1] - y).abs() < 0.5 && p[0] > x && s != label)
            .map(|(s, _)| s.clone())
            .next()
            .unwrap_or_default()
    }

    /// Controls -> Look in the in-game menu: each bind row shows the keys
    /// vcod's console binds to its command (`Key_GetCommandAssignment`'s
    /// first two, `KEY_*` names, "Unbound" for none); a click waits for a
    /// key ("Press a key" in the row, the key bind status line shown) and
    /// the key goes to the console as `bind`; a third key replaces both,
    /// Backspace unbinds, Escape leaves the binds be.
    #[test]
    fn cod4_controls_rebind_through_the_console() {
        let Some(install) = vcod_cod4::testing::cod4_install() else {
            return;
        };
        let mut hud = Cod4Hud::default();
        hud.load(&install.root().join("main"), "mp_crash");
        let a = hud.menu_assets().unwrap();
        let (mut m, gs) = in_game(&hud);
        m.open_main(&gs);
        assert!(click(&mut m, &a, &gs, "Controls").is_empty());
        assert_eq!(m.open().unwrap().name, "main_controls");
        click(&mut m, &a, &gs, "Look...");
        assert_eq!(m.open().unwrap().name, "options_look");
        assert_eq!(value_of(&m, &a, "Lean Left"), "Q");
        assert_eq!(value_of(&m, &a, "Look Up"), "Unbound");
        let names: Vec<String> = texts(&m, &a).into_iter().map(|t| t.0).collect();
        assert!(
            !names.iter().any(|n| n.contains("Press")),
            "the bind status line starts hidden: {names:?}"
        );
        click(&mut m, &a, &gs, "Look Up");
        assert!(m.waiting_for_key());
        assert_eq!(
            value_of(&m, &a, "Look Up"),
            a.translate("MENU_BIND_KEY_PENDING")
        );
        let names: Vec<String> = texts(&m, &a).into_iter().map(|t| t.0).collect();
        assert!(
            names.iter().any(|n| n == a.translate("EXE_KEYWAIT")),
            "{names:?}"
        );
        m.bind_key("PGUP");
        assert!(!m.waiting_for_key());
        assert_eq!(m.take_console(), ["bind PGUP \"+lookup\""]);
        assert_eq!(value_of(&m, &a, "Look Up"), "Page Up");
        click(&mut m, &a, &gs, "Look Up");
        m.bind_key("HOME");
        m.take_console();
        assert_eq!(value_of(&m, &a, "Look Up"), "Page Up or Home");
        // A third key unbinds the two first.
        click(&mut m, &a, &gs, "Look Up");
        m.bind_key("MOUSE3");
        assert_eq!(
            m.take_console(),
            ["unbind PGUP", "unbind HOME", "bind MOUSE3 \"+lookup\""]
        );
        click(&mut m, &a, &gs, "Look Up");
        m.bind_key("ESCAPE");
        assert!(m.take_console().is_empty());
        click(&mut m, &a, &gs, "Look Up");
        m.bind_key("BACKSPACE");
        assert_eq!(m.take_console(), ["unbind MOUSE3"]);
        assert_eq!(value_of(&m, &a, "Look Up"), "Unbound");
        // The console runs the lines as `main.rs` does; the config keeps them.
        let mut shell = crate::console::shell::Shell::new();
        shell.execute("bind PGUP \"+lookup\"");
        assert_eq!(shell.keys_bound_to("+lookup"), ["PGUP"]);
    }

    /// Options' rows on vcod's cvars: the sensitivity slider (1..30 from
    /// the zone) set by a click on its bar and stepped 5% by the arrows,
    /// Invert Mouse flipping `m_pitch` through `uiScript update
    /// ui_mousePitch`, brightness on `r_gamma`, the master volume on
    /// `mss_volume`; an option vcod has no cvar for drawn in the menu's
    /// `disableColor` and passed over by the focus.
    #[test]
    fn cod4_options_set_vcods_cvars() {
        let Some(install) = vcod_cod4::testing::cod4_install() else {
            return;
        };
        let mut hud = Cod4Hud::default();
        hud.load(&install.root().join("main"), "mp_crash");
        let a = hud.menu_assets().unwrap();
        let (mut m, gs) = in_game(&hud);
        m.open_main(&gs);
        click(&mut m, &a, &gs, "Controls");
        click(&mut m, &a, &gs, "Look...");
        assert_eq!(value_of(&m, &a, "Invert Mouse"), "No");
        click(&mut m, &a, &gs, "Invert Mouse");
        assert_eq!(m.take_console(), ["set m_pitch \"-0.022\""]);
        assert_eq!(value_of(&m, &a, "Invert Mouse"), "Yes");
        // The slider's bar: 96 wide at x 200 past the rect's -74, centred.
        let layout = m.open().unwrap().layout.clone().unwrap();
        let (i, it) = layout
            .items
            .iter()
            .enumerate()
            .find(|(_, it)| it.dvar.as_deref() == Some("sensitivity"))
            .unwrap();
        let s = m.slider(it, &layout).unwrap();
        assert_eq!([s.x, s.y], [126.0, 298.0 + 3.0]);
        // 5 of 1..30 sits 4/29 along the 84 units from 6 in.
        assert!((s.thumb - (126.0 + 6.0 + 84.0 * 4.0 / 29.0)).abs() < 1e-3);
        let (_, align) = m.rect(it, &layout);
        let sp = ScreenPlacement::new(SIZE.0, SIZE.1);
        let [x, y, _, _] = placed(&sp, ([126.0 + 6.0 + 42.0, s.y + 4.0, 0.0, 0.0], align));
        m.mouse_move(None, Some([x, y]), SIZE, &gs);
        assert_eq!(m.open().unwrap().focus, Some(i));
        m.click(SIZE, &gs);
        assert_eq!(m.take_console(), ["set sensitivity \"15.5\""]);
        let mut out = Vec::new();
        m.key(MenuKey::Right, &gs, &mut out);
        assert_eq!(m.take_console(), ["set sensitivity \"16.95\""]);
        // Graphics: brightness on `r_gamma`, the dual video cards greyed.
        m.open_main(&gs);
        click(&mut m, &a, &gs, "Options");
        click(&mut m, &a, &gs, "Graphics...");
        assert_eq!(m.open().unwrap().name, "options_graphics");
        let layout = m.open().unwrap().layout.clone().unwrap();
        let gamma = layout
            .items
            .iter()
            .find(|it| it.dvar.as_deref() == Some("r_gamma"))
            .unwrap();
        assert!(!m.greyed(gamma));
        let gpu = layout
            .items
            .iter()
            .find(|it| it.dvar.as_deref() == Some("r_multiGpu"))
            .unwrap();
        assert!(m.greyed(gpu));
        let (quads, _) = paint(&m, &a, SIZE, 0, true);
        assert!(quads.iter().any(|q| q.rgba == layout.disable_color));
        // Sound: the master volume is `mss_volume`, 0.8.
        m.open_main(&gs);
        click(&mut m, &a, &gs, "Options");
        click(&mut m, &a, &gs, "Sound...");
        let layout = m.open().unwrap().layout.clone().unwrap();
        let vol = layout
            .items
            .iter()
            .find(|it| it.dvar.as_deref() == Some("snd_volume"))
            .unwrap();
        assert!(!m.greyed(vol));
        let s = m.slider(vol, &layout).unwrap();
        assert!((s.thumb - (s.x + 6.0 + 84.0 * 0.8)).abs() < 1e-3);
    }

    /// Call Vote: Restart Map sends `cmd callvote map_restart`; Change Map
    /// lists the arena file's maps by their long names and votes the
    /// picked one; Change Game Type/Map steps the game type on a click and
    /// votes `typemap`; Kick Player lists the players and votes
    /// `tempBanUser`, as the zone's `voteTempBan` asks.
    #[test]
    fn cod4_call_vote_sends_callvote() {
        let Some(install) = vcod_cod4::testing::cod4_install() else {
            return;
        };
        let mut hud = Cod4Hud::default();
        hud.load(&install.root().join("main"), "mp_crash");
        let a = hud.menu_assets().unwrap();
        let (mut m, gs) = in_game(&hud);
        m.open_main(&gs);
        click(&mut m, &a, &gs, "Call Vote");
        assert_eq!(m.open().unwrap().name, "callvote");
        click(&mut m, &a, &gs, "Restart Map");
        assert_eq!(m.take_console(), ["cmd callvote map_restart"]);
        m.open_main(&gs);
        click(&mut m, &a, &gs, "Call Vote");
        click(&mut m, &a, &gs, "Change Map");
        assert_eq!(m.open().unwrap().name, "changemap");
        let names: Vec<String> = texts(&m, &a).into_iter().map(|t| t.0).collect();
        for map in ["Ambush", "Backlot", "Bloc"] {
            assert!(names.iter().any(|n| n == map), "{map} in {names:?}");
        }
        click(&mut m, &a, &gs, "Backlot");
        click(&mut m, &a, &gs, "Vote");
        assert_eq!(m.take_console(), ["callvote map mp_backlot"]);
        m.open_main(&gs);
        click(&mut m, &a, &gs, "Call Vote");
        click(&mut m, &a, &gs, "Change Game Type/Map");
        assert_eq!(m.open().unwrap().name, "changegametype");
        let names: Vec<String> = texts(&m, &a).into_iter().map(|t| t.0).collect();
        assert!(names.iter().any(|n| n == "Free-for-all"), "{names:?}");
        click(&mut m, &a, &gs, "Free-for-all");
        click(&mut m, &a, &gs, "Bloc");
        click(&mut m, &a, &gs, "Vote");
        assert_eq!(m.take_console(), ["callvote typemap dom mp_bloc"]);
        m.open_main(&gs);
        click(&mut m, &a, &gs, "Call Vote");
        click(&mut m, &a, &gs, "Kick Player");
        click(&mut m, &a, &gs, "probe");
        click(&mut m, &a, &gs, "Vote");
        assert_eq!(m.take_console(), ["callvote tempBanUser \"probe\""]);
    }

    /// Mute Players: the snapshot's players by their cleaned names; Toggle
    /// Mute on the picked one marks the row "Muted", tells the server
    /// (`muteplayer <n>`, as `CL_MutePlayer` does) and holds that player's
    /// chat back; a second toggle undoes it.
    #[test]
    fn cod4_mute_players_toggles_and_filters_chat() {
        let Some(install) = vcod_cod4::testing::cod4_install() else {
            return;
        };
        let mut hud = Cod4Hud::default();
        hud.load(&install.root().join("main"), "mp_crash");
        let a = hud.menu_assets().unwrap();
        let (mut m, gs) = in_game(&hud);
        m.open_main(&gs);
        click(&mut m, &a, &gs, "Mute Players");
        assert_eq!(m.open().unwrap().name, "muteplayer");
        click(&mut m, &a, &gs, "walker");
        let toggle = a.translate("MP_PLAYER_TOGGLE_MUTE").to_string();
        click(&mut m, &a, &gs, &toggle);
        assert_eq!(m.take_console(), ["muteplayer 0"]);
        assert!(m.chat_muted("\u{15}\u{15}^9walker^7: ^7hi"));
        assert!(!m.chat_muted("probe^7: ^7hi"));
        m.open_main(&gs);
        click(&mut m, &a, &gs, "Mute Players");
        let names: Vec<String> = texts(&m, &a).into_iter().map(|t| t.0).collect();
        assert!(
            names.iter().any(|n| n == a.translate("MP_MUTED")),
            "{names:?}"
        );
        click(&mut m, &a, &gs, "walker");
        click(&mut m, &a, &gs, &toggle);
        assert_eq!(m.take_console(), ["unmuteplayer 0"]);
        assert!(!m.chat_muted("walker: hi"));
    }

    /// Mute Players with more players than rows: the list draws a scroll
    /// bar; its down arrow steps a row, a drag of its thumb to the bottom
    /// shows the last rows, and a double click on a row runs the list's
    /// `doubleClick` (`uiScript mutePlayer`), as `Item_ListBox_HandleKey`
    /// does (cod4-hud.md 12.6).
    #[test]
    fn cod4_list_box_scrolls_drags_and_double_clicks() {
        use crate::play::cod4_menus::placed;
        let Some(install) = vcod_cod4::testing::cod4_install() else {
            return;
        };
        let mut hud = Cod4Hud::default();
        hud.load(&install.root().join("main"), "mp_crash");
        let a = hud.menu_assets().unwrap();
        let (mut m, gs) = in_game(&hud);
        m.host.players = (0..40).map(|n| (n, format!("p{n:02}"))).collect();
        m.open_main(&gs);
        click(&mut m, &a, &gs, "Mute Players");
        let layout = m.open().unwrap().layout.clone().unwrap();
        let i = layout
            .items
            .iter()
            .position(|it| it.name == "playermutelist")
            .unwrap();
        let (r, align) = m.rect(&layout.items[i], &layout);
        let sp = ScreenPlacement::new(SIZE.0, SIZE.1);
        let at = |x: f32, y: f32| {
            let [px, py, _, _] = placed(&sp, ([x, y, 0.0, 0.0], align));
            [px, py]
        };
        let bar = m.scroll_bar(i).expect("a scroll bar");
        assert!(bar.max > 0, "{bar:?}");
        // The down arrow, 8 in from the bottom right corner.
        m.mouse_move(
            None,
            Some(at(r[0] + r[2] - 8.0, r[1] + r[3] - 8.0)),
            SIZE,
            &gs,
        );
        m.click(SIZE, &gs);
        assert_eq!(m.list_state(i).0, 1);
        // Hold the thumb and drag it past the bottom.
        let bar = m.scroll_bar(i).unwrap();
        m.mouse_move(None, Some(at(bar.x + 8.0, bar.thumb + 8.0)), SIZE, &gs);
        m.click(SIZE, &gs);
        m.mouse_move(None, Some(at(bar.x + 8.0, r[1] + r[3])), SIZE, &gs);
        assert_eq!(m.list_state(i).0, bar.max);
        m.release();
        m.mouse_move(None, Some(at(bar.x + 8.0, r[1] + 20.0)), SIZE, &gs);
        assert_eq!(m.list_state(i).0, bar.max, "released, no drag");
        assert!(texts(&m, &a).iter().any(|t| t.0 == "p39"));
        // A double click mutes the row.
        let last = "p39";
        click(&mut m, &a, &gs, last);
        assert!(m.take_console().is_empty());
        click(&mut m, &a, &gs, last);
        assert_eq!(m.take_console(), ["muteplayer 39"]);
    }

    /// The in-game Options as 1.7 MP shows them: `ui_multiplayer` reads
    /// 1, so Multiplayer Options stands where single player's Cheats would
    /// and Game Options drops the subtitles and crosshair rows. Shadows
    /// (`sm_enable`) and Allow Downloading (`cl_allowDownload`) set vcod's
    /// cvars, and the player name typed into `ui_name` goes to `name` as
    /// the page closes (cod4-front-end.md 9).
    /// The Options pages' rows that 1.7 swaps by state (cod4-hud.md 12.6):
    /// on a client of a remote server (`sv_running` 0) Graphics opens its
    /// page; a listen server's `sv_running` 1 (vcod's: a joined Dedicated
    /// No server, `ScriptMenus::listen_server`) with
    /// the never-registered `ui_allow_graphic_change` reads the greyed
    /// copy, which opens nothing; the language row shows only under
    /// `developer`.
    #[test]
    fn cod4_options_rows_follow_sv_running_and_developer() {
        let Some(install) = vcod_cod4::testing::cod4_install() else {
            return;
        };
        let mut hud = Cod4Hud::default();
        hud.load(&install.root().join("main"), "mp_crash");
        let a = hud.menu_assets().unwrap();
        let (mut m, gs) = in_game(&hud);
        let names =
            |m: &ScriptMenus| -> Vec<String> { texts(m, &a).into_iter().map(|t| t.0).collect() };
        m.open_main(&gs);
        click(&mut m, &a, &gs, "Options");
        click(&mut m, &a, &gs, "Graphics...");
        assert_eq!(m.open().unwrap().name, "options_graphics");
        m.ui.dvars.insert("sv_running".into(), "1".into());
        m.open_main(&gs);
        click(&mut m, &a, &gs, "Options");
        click(&mut m, &a, &gs, "Graphics...");
        assert_eq!(m.open().unwrap().name, "main_options", "the greyed copy");
        m.ui.dvars.remove("sv_running");
        // vcod's Dedicated No server, joined, is retail's listen server
        m.listen_server = true;
        m.open_main(&gs);
        click(&mut m, &a, &gs, "Options");
        click(&mut m, &a, &gs, "Graphics...");
        assert_eq!(
            m.open().unwrap().name,
            "main_options",
            "greyed when listening"
        );
        m.listen_server = false;
        click(&mut m, &a, &gs, "Game Options...");
        let plain = names(&m);
        m.host.cvars.insert("developer".into(), "1".into());
        let dev = names(&m);
        assert!(dev.len() > plain.len(), "{plain:?} {dev:?}");
        assert!(dev.iter().any(|s| s == "Language"), "{dev:?}");
    }

    #[test]
    fn cod4_options_rows_follow_ui_multiplayer() {
        let Some(install) = vcod_cod4::testing::cod4_install() else {
            return;
        };
        let mut hud = Cod4Hud::default();
        hud.load(&install.root().join("main"), "mp_crash");
        let a = hud.menu_assets().unwrap();
        let (mut m, gs) = in_game(&hud);
        let names =
            |m: &ScriptMenus| -> Vec<String> { texts(m, &a).into_iter().map(|t| t.0).collect() };
        m.open_main(&gs);
        click(&mut m, &a, &gs, "Options");
        let n = names(&m);
        assert!(n.iter().any(|s| s == "Multiplayer Options..."), "{n:?}");
        assert!(!n.iter().any(|s| s == "Cheats"), "{n:?}");
        click(&mut m, &a, &gs, "Game Options...");
        let n = names(&m);
        assert!(n.iter().any(|s| s == "Show Blood"), "{n:?}");
        for sp in ["Subtitles", "Enable Crosshair"] {
            assert!(!n.iter().any(|s| s == sp), "{sp} in {n:?}");
        }
        m.open_main(&gs);
        click(&mut m, &a, &gs, "Options");
        click(&mut m, &a, &gs, "Graphics...");
        assert_eq!(value_of(&m, &a, "Shadows"), "Yes");
        click(&mut m, &a, &gs, "Shadows");
        assert_eq!(m.take_console(), ["set sm_enable \"0\""]);
        m.open_main(&gs);
        click(&mut m, &a, &gs, "Options");
        click(&mut m, &a, &gs, "Multiplayer Options...");
        assert_eq!(m.open().unwrap().name, "options_multi");
        click(&mut m, &a, &gs, "Allow Downloading");
        assert_eq!(m.take_console(), ["set cl_allowdownload \"0\""]);
        click(&mut m, &a, &gs, "Unknown Soldier");
        assert!(m.editing(), "the name field edits");
        // The field holds 15 characters, full with the default name.
        let mut out = Vec::new();
        m.key(MenuKey::Char(127), &gs, &mut out);
        m.type_text("X");
        m.key(MenuKey::Enter, &gs, &mut out);
        m.key(MenuKey::Escape, &gs, &mut out);
        let c = m.take_console();
        assert!(
            c.contains(&"set name \"Unknown SoldieX\"".to_string()),
            "the name goes to `name` as the page closes"
        );
    }
}
