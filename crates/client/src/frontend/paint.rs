//! Contains routines ported from the RTCW-MP GPL source, Copyright (C) 1999-2010 id Software LLC, a ZeniMax Media company.
//! See NOTICE.
//!
//! Painting: the menus' windows, text, owner draws and the driver info
//! page as [`HudQuad`]s, through [`Painter`].

use vcod_common::localize::Localized;
use vcod_common::ui_menu::{
    FEEDER_ALLMAPS, FEEDER_MODS, FEEDER_MUTELIST, FEEDER_PLAYER_LIST, FEEDER_PLAYER_PROFILES,
    FEEDER_SERVERS, FEEDER_SERVERSTATUS, ITEM_ALIGN_CENTER, ITEM_ALIGN_RIGHT, ITEM_TYPE_BIND,
    ITEM_TYPE_BUTTON, ITEM_TYPE_DECIMALFIELD, ITEM_TYPE_DVARENUM, ITEM_TYPE_EDITFIELD,
    ITEM_TYPE_LISTBOX, ITEM_TYPE_MULTI, ITEM_TYPE_NUMERICFIELD, ITEM_TYPE_OWNERDRAW,
    ITEM_TYPE_SLIDER, ITEM_TYPE_TEXT, ITEM_TYPE_YESNO, UiItem, UiMenu, WINDOW_STYLE_CVAR_SHADER,
    WINDOW_STYLE_FILLED, WINDOW_STYLE_SHADER,
};

use super::browser::{Source, Status};
use super::input::contains;
use super::{
    ITEM_ALIGN_CENTER2, ITEM_TYPE_UPREDITFIELD, ItemState, UI_GLINFO, UI_JOINGAMETYPE,
    UI_NETGAMETYPE, UI_NETMAPCINEMATIC, UI_NETMAPPREVIEW, UI_NETSOURCE, UI_SERVERREFRESHDATE,
    UI_STARTMAPCINEMATIC, Ui, UiEffect, cod2, listbox,
};
use crate::console::shell::Shell;
use crate::hud::font::{Slot, TextStyle, UiFonts};
use crate::hud::hudelem::{Virtual, full_width};
use crate::hud::{HudQuad, font};

impl Ui {
    /// `Item_SetTextExtents` (0x40013bb0) and `ToWindowCoords`
    /// (0x40013b70): the text's left edge at `textalignx` moved left by the
    /// label's width (right aligned) or half of it (centred), an owner
    /// draw's value width included, then shifted by the border size when
    /// the item has a border. The baseline sits `textaligny` down. Returns
    /// the left edge and the baseline on the 640 grid.
    fn text_origin(item: &UiItem, label_w: f32, value_w: f32, empty: bool) -> (f32, f32) {
        let b = if item.border != 0 {
            item.border_size
        } else {
            0.0
        };
        let x = item.rect[0] + item.text_align_x + b;
        let y = item.rect[1] + item.text_align_y + b;
        // An owner draw with no label draws at `textalignx` whatever its
        // alignment (`UI_OwnerDraw`'s `rect.x + text_x`).
        let left = match item.text_align {
            _ if empty => x,
            ITEM_ALIGN_RIGHT => x - label_w - value_w,
            ITEM_ALIGN_CENTER => x - (label_w + value_w) / 2.0,
            _ => x,
        };
        (left, y)
    }

    /// What a text item prints: its `text`, else its cvar's value (a
    /// multi's cvar picks a choice instead). The script menus' motd is such
    /// a cvar item: `scr_motd` on 1.1, `ui_motd` on 1.5 and UO, both off
    /// the 140/204 mirror (cod11-front-end.md, "The message of the day").
    pub(super) fn label(&self, item: &UiItem, loc: &Localized, shell: &Shell) -> String {
        if !item.text.is_empty() {
            loc.translate(&item.text).into_owned()
        } else if let Some(cvar) = &item.cvar
            && !matches!(
                item.kind,
                ITEM_TYPE_MULTI | ITEM_TYPE_DVARENUM | ITEM_TYPE_DECIMALFIELD
            )
        {
            // `ui_favorite_message` holds a localized key.
            let v = self.cvar(cvar, shell).unwrap_or_default();
            loc.translate(&v).into_owned()
        } else {
            String::new()
        }
    }

    /// The text item's `textRect` from its last paint, as
    /// `Item_CorrectedTextRect` (0x40013630) hands it out: the text's width
    /// and the font's height, standing on the baseline. `None` before the
    /// item was painted, when retail's rect is still zero.
    pub(super) fn text_rect(&self, menu: usize, item: usize) -> Option<[f32; 4]> {
        let [x, y, w, h] = *self.text_rects.borrow().get(&(menu, item))?;
        Some([x, y - h, w, h])
    }

    /// Whether `at` is on the text of `item` of `menu`.
    pub(super) fn on_text(&self, menu: usize, item: usize, at: [f32; 2]) -> bool {
        self.text_rect(menu, item).is_some_and(|r| contains(r, at))
    }

    /// Every open menu as HUD quads for a `w` x `h` window.
    pub fn build(&self, w: f32, h: f32, loc: &Localized, shell: &Shell) -> Vec<HudQuad> {
        let mut out = Vec::new();
        let p = Painter {
            v: match self.cod2 {
                Some(_) => cod2::grid(w, h),
                None => Virtual::new((w, h)),
            },
            fonts: self.fonts.as_ref(),
            exact: self.cod2.is_some(),
        };
        for &m in self.painted() {
            let menu = &self.menus[m];
            menu_window(&p, menu, &mut out);
            for (i, item) in menu.items.iter().enumerate() {
                if !self.shown(m, i, shell) {
                    continue;
                }
                let st = &self.state[m][i];
                if item.style == WINDOW_STYLE_CVAR_SHADER {
                    // `Item_Paint` (0x40016470) paints the window with the
                    // cvar's value for its shader and nothing of its type.
                    let shader = item.cvar.as_ref().and_then(|c| self.cvar(c, shell));
                    p.window(item, st, shader.as_deref(), &mut out);
                    continue;
                }
                p.window(item, st, None, &mut out);
                let focused =
                    self.focused(m, i) && !item.decoration && item.kind != ITEM_TYPE_LISTBOX;
                let rt = self.real_time;
                let color = match focused {
                    true => pulse(menu.focus_color, dim(menu.focus_color), rt),
                    false => item.fore,
                };
                // The last bind to wait for a key pulses toward dark red
                // (0x4001527f..0x4001529f, CoD2MP_s.exe 0x540a12).
                let value_color = match self.options.bind_item == Some((m, i)) && focused {
                    true => pulse(menu.focus_color, [0.8, 0.0, 0.0, 0.8], rt),
                    false => color,
                };
                if let Some((len, first)) = self.list_scroll(item) {
                    let thumb = self
                        .thumb_drag
                        .filter(|&d| d == (m, i))
                        .and_then(|_| listbox::drag_thumb(item, self.cursor[1]));
                    listbox::paint(&p, item, len, first, thumb, &mut out);
                }
                if item.kind == ITEM_TYPE_LISTBOX && item.feeder == Some(FEEDER_SERVERS) {
                    self.server_list(&p, item, &mut out);
                    continue;
                }
                if item.kind == ITEM_TYPE_LISTBOX && item.feeder == Some(FEEDER_SERVERSTATUS) {
                    self.status_list(&p, item, loc, &mut out);
                    continue;
                }
                if item.kind == ITEM_TYPE_LISTBOX && item.feeder == Some(FEEDER_MODS) {
                    self.mod_rows(&p, item, &mut out);
                    continue;
                }
                if item.kind == ITEM_TYPE_LISTBOX && item.feeder == Some(FEEDER_ALLMAPS) {
                    self.map_rows(&p, item, &mut out);
                    continue;
                }
                if item.kind == ITEM_TYPE_LISTBOX && item.feeder == Some(FEEDER_PLAYER_PROFILES) {
                    self.profile_rows(&p, item, &mut out);
                    continue;
                }
                if item.kind == ITEM_TYPE_LISTBOX && item.feeder == Some(FEEDER_PLAYER_LIST) {
                    self.player_rows(&p, item, &mut out);
                    continue;
                }
                if item.kind == ITEM_TYPE_LISTBOX && item.feeder == Some(FEEDER_MUTELIST) {
                    self.mute_rows(&p, item, loc, &mut out);
                    continue;
                }
                if matches!(item.ownerdraw, UI_NETMAPPREVIEW | UI_NETMAPCINEMATIC) {
                    // `UI_OwnerDraw` moves the rect by the text offsets.
                    let [x, y, w, h] = item.rect;
                    let r = [x + item.text_align_x, y + item.text_align_y, w, h];
                    p.quad(r, [1.0; 4], &self.map_preview(), &mut out);
                    continue;
                }
                if item.ownerdraw == UI_GLINFO {
                    let [x, y, w, h] = item.rect;
                    let r = [x + item.text_align_x, y + item.text_align_y, w, h];
                    gl_info(&p, r, item, color, &mut out);
                    continue;
                }
                if item.ownerdraw == UI_STARTMAPCINEMATIC {
                    let [x, y, w, h] = item.rect;
                    let r = [x + item.text_align_x, y + item.text_align_y, w, h];
                    p.quad(r, [1.0; 4], &self.start_map_preview(), &mut out);
                    continue;
                }
                let label = self.label(item, loc, shell);
                let value = match item.ownerdraw {
                    UI_NETSOURCE => Some(fill(
                        &loc.translate("@EXE_NETSOURCE"),
                        &[&loc.translate(Source::NAMES[self.browser.source as usize])],
                    )),
                    UI_SERVERREFRESHDATE => Some(self.refresh_text(loc, shell)),
                    UI_JOINGAMETYPE => Some(match self.join_game_type() {
                        0 => loc.translate("@EXE_ALL").into_owned(),
                        i => {
                            let name = &self.game_types[i - 1].name;
                            loc.get(name).unwrap_or(name).to_string()
                        }
                    }),
                    UI_NETGAMETYPE => Some(self.gametype_text(loc)),
                    _ => self.option_value(m, i, loc, shell),
                };
                // An owner draw or a multi's value follows 8 units after
                // the label.
                let label_w = p.width(&label, item);
                let value_w = value
                    .as_ref()
                    .filter(|_| item.kind == ITEM_TYPE_OWNERDRAW)
                    .map_or(0.0, |v| p.width(v, item));
                let (mut left, mut y) = Self::text_origin(item, label_w, value_w, label.is_empty());
                // Where the label's width ends, which a value follows.
                let mut label_end = label_w;
                if paints_text(item) {
                    // `Item_SetTextExtents` keeps a rect with a width, and
                    // the text draws where the kept rect says.
                    let mut rects = self.text_rects.borrow_mut();
                    let cvar_w = match item.cvar.as_ref() {
                        Some(c) if takes_cvar_width(item) => {
                            p.width(&self.cvar(c, shell).unwrap_or_default(), item)
                        }
                        _ => 0.0,
                    };
                    let widths = TextWidths {
                        label: label_w,
                        value: value_w,
                        cvar: cvar_w,
                        height: p.height(item),
                    };
                    let rect = text_extents(item, rects.get(&(m, i)).copied(), widths);
                    [left, y, label_end] = [rect[0], rect[1], rect[2]];
                    let rect = match item.autowrap {
                        // Each line moves the rect to its own spot.
                        true => match p.wrap_lines(&label, item).last() {
                            Some(&(_, x, ly)) => [x, ly, rect[2], rect[3]],
                            None => rect,
                        },
                        false => rect,
                    };
                    rects.insert((m, i), rect);
                }
                if item.autowrap {
                    for (line, x, ly) in p.wrap_lines(&label, item) {
                        let style = item.text_style;
                        p.text(&line, x, ly, item, color, style, &mut out);
                    }
                    continue;
                }
                if !label.is_empty() {
                    let style = item.text_style;
                    p.text(&label, left, y, item, color, style, &mut out);
                }
                if item.kind == ITEM_TYPE_SLIDER {
                    let end = (!label.is_empty()).then_some(left + label_end);
                    self.slider_quads(&p, (m, i), item, end, value_color, shell, &mut out);
                }
                if let Some(v) = value {
                    let gap = if label.is_empty() { 0.0 } else { 8.0 };
                    let x = left + label_end + gap;
                    let style = item.text_style;
                    p.text(&v, x, y, item, value_color, style, &mut out);
                    if let Some((at, glyph)) = self.edit_cursor(m, i)
                        && at <= v.chars().count()
                        && cursor_blink_on(blink_clock(), self.cod2.is_some())
                    {
                        let head: String = v.chars().take(at).collect();
                        let cx = x + p.width(&head, item);
                        let glyph = glyph.to_string();
                        // CoD2 draws the cursor glyph once, unshadowed
                        // (`gfx_d3d_mp_x86_s.dll` 0x10032877..0x100328e5).
                        let style = if self.cod2.is_some() { 0 } else { style };
                        p.text(&glyph, cx, y, item, value_color, style, &mut out);
                    }
                }
            }
        }
        // `UI_Refresh` (0x40007b20): the cursor over every menu, 32 units
        // square and centred on the mouse, in white.
        if let Some(cursor) = self.assets.cursor.as_ref().filter(|_| self.active()) {
            let [x, y] = self.cursor;
            p.quad([x - 16.0, y - 16.0, 32.0, 32.0], [1.0; 4], cursor, &mut out);
        }
        out
    }

    /// The selected server's `levelshots/<mapname>`, or
    /// `menu/art/unknownmap` when the map has none, as the server feeder's
    /// selection registers it (0x4000cd2a) and `UI_DrawNetMapPreview`
    /// (0x40008b60) falls back. No 1.1 pak holds `menu/art/unknownmap`.
    fn map_preview(&self) -> String {
        let map = self
            .selected
            .and_then(|a| self.browser.row(a))
            .and_then(|s| s.info.as_ref())
            .map(|i| i.mapname.clone())
            .unwrap_or_default();
        self.levelshot(&map)
    }

    /// `levelshots/<map>`, or `menu/art/unknownmap` when there is none.
    pub(super) fn levelshot(&self, map: &str) -> String {
        let map = map.to_ascii_lowercase();
        if self.levelshots.contains(&map) {
            format!("levelshots/{map}")
        } else {
            "menu/art/unknownmap".into()
        }
    }

    /// `UI_SERVERREFRESHDATE`'s line (ui_mp_x86.dll 0x40008fc5): the
    /// refresh's progress with the source's list length, else
    /// `EXE_REFRESHTIME` with `ui_lastServerRefresh_<source>`
    /// (`0x400090fa`..`0x4000913b`), empty for a source not refreshed.
    fn refresh_text(&self, loc: &Localized, shell: &Shell) -> String {
        let (total, _) = self.browser.counts();
        match self.browser.status {
            Status::Idle | Status::Done | Status::Failed => fill(
                &loc.translate("@EXE_REFRESHTIME"),
                &[&self
                    .cvar(&refresh_cvar(self.browser.source), shell)
                    .unwrap_or_default()],
            ),
            Status::Master => loc
                .translate("@EXE_WAITINGFORMASTERSERVERRESPONSE")
                .into_owned(),
            Status::Pinging => fill(
                &loc.translate("@EXE_GETTINGINFOFORSERVERS"),
                &[&total.to_string()],
            ),
        }
    }
}

/// The cvar `UI_StartServerRefresh` dates `source` in (ui_mp_x86.dll
/// `0x4002f000`): not in the vmCvar table, so nothing archives it.
pub(super) fn refresh_cvar(source: Source) -> String {
    format!("ui_lastServerRefresh_{}", source as usize)
}

impl Ui {
    /// The `set` of `ui_lastServerRefresh_<source>` for each refresh started
    /// since the last call, its month localized as the UI localizes it
    /// (ui_mp_x86.dll 0x4000ea90); once a frame, before the menus draw.
    pub fn date_refreshes(&mut self, loc: &Localized) -> Vec<UiEffect> {
        let mut out = Vec::new();
        for (source, t) in std::mem::take(&mut self.browser.stamps) {
            self.set_cvar(&refresh_cvar(source), &refresh_date(t, loc), &mut out);
        }
        out
    }
}

/// `UI_StartServerRefresh`'s date (ui_mp_x86.dll 0x4000ea90): `%s %i, %i
/// %i:%02i` over the month's `EXE_MONTH_ABV_*` text, day, year, hour and
/// minute. Retail's June key is `EXE_MONTH_ABV_JUN`, which `exe.str` lacks
/// (it has `MONTH_ABV_JUNE`), so June prints the key.
fn refresh_date(t: jiff::civil::DateTime, loc: &Localized) -> String {
    const MONTHS: [&str; 12] = [
        "@EXE_MONTH_ABV_JANUARY",
        "@EXE_MONTH_ABV_FEBRUARY",
        "@EXE_MONTH_ABV_MARCH",
        "@EXE_MONTH_ABV_APRIL",
        "@EXE_MONTH_ABV_MAY",
        "@EXE_MONTH_ABV_JUN",
        "@EXE_MONTH_ABV_JULY",
        "@EXE_MONTH_ABV_AUGUST",
        "@EXE_MONTH_ABV_SEPTEMBER",
        "@EXE_MONTH_ABV_OCTOBER",
        "@EXE_MONTH_ABV_NOVEMBER",
        "@EXE_MONTH_ABV_DECEMBER",
    ];
    let month = loc.translate(MONTHS[t.month() as usize - 1]);
    format!(
        "{month} {}, {}   {}:{:02}",
        t.day(),
        t.year(),
        t.hour(),
        t.minute()
    )
}

/// What the driver info page prints: vcod's GPU in the GL strings'
/// places, set once the renderer has its adapter.
#[derive(Clone, Debug, Default)]
pub struct GpuInfo {
    /// The driver's name, in `GL_VENDOR`'s place.
    pub vendor: String,
    /// The driver's version, in `GL_VERSION`'s.
    pub version: String,
    /// The adapter's name, in `GL_RENDERER`'s.
    pub renderer: String,
    pub color_bits: u32,
    pub depth_bits: u32,
    pub stencil_bits: u32,
    /// The adapter's features, space separated, in `GL_EXTENSIONS`' place.
    pub extensions: String,
}

pub static GPU_INFO: std::sync::OnceLock<GpuInfo> = std::sync::OnceLock::new();

/// `UI_DrawGLInfo` (ui_mp_x86.dll 0x400091c0) over `r`, the owner draw's
/// rect moved by its text offsets: three lines 15 apart cut at 80
/// characters, then the extensions three to a row 10 apart at thirds of
/// the width, cut at 36, while a row's baseline stays within 11 of the
/// bottom; at most 256 (front-end doc 14, "Driver Info"). Each line with
/// its baseline.
fn gl_info_lines(info: &GpuInfo, r: [f32; 4]) -> Vec<(String, f32, f32)> {
    let cut = |s: &str, n: usize| s.chars().take(n).collect::<String>();
    let [x, y, w, h] = r;
    let head = [
        format!("VENDOR: {}", info.vendor),
        format!("VERSION: {}: {}", info.version, info.renderer),
        format!(
            "PIXELFORMAT: color({}-bits) Z({}-bits) stencil({}-bits)",
            info.color_bits, info.depth_bits, info.stencil_bits
        ),
    ];
    let mut out: Vec<_> = (head.iter().enumerate())
        .map(|(i, l)| (cut(l, 80), x + 2.0, y + 15.0 * i as f32))
        .collect();
    let words: Vec<&str> = (info.extensions.split(' '))
        .filter(|e| !e.is_empty())
        .take(256)
        .collect();
    let mut row_y = y + 45.0;
    for row in words.chunks(3) {
        if row_y > y + h - 11.0 {
            break;
        }
        for (col, e) in row.iter().enumerate() {
            out.push((cut(e, 36), x + 2.0 + w * col as f32 / 3.0, row_y));
        }
        row_y += 10.0;
    }
    out
}

fn gl_info(p: &Painter, r: [f32; 4], item: &UiItem, color: [f32; 4], out: &mut Vec<HudQuad>) {
    let info = GPU_INFO.get().cloned().unwrap_or_default();
    for (line, x, y) in gl_info_lines(&info, r) {
        p.text(&line, x, y, item, color, item.text_style, out);
    }
}

/// `%s` and `%d` in a localized format filled in order.
pub(super) fn fill(fmt: &str, args: &[&str]) -> String {
    // CoD2's strings number their slots `&&1`..`&&9` (cod2-protocol.md
    // 3.4.1); no CoD1 string holds `&&`.
    if fmt.contains("&&") {
        let mut out = fmt.to_string();
        for (n, a) in args.iter().enumerate() {
            out = out.replace(&format!("&&{}", n + 1), a);
        }
        return out;
    }
    let mut out = String::new();
    let mut args = args.iter();
    let mut rest = fmt;
    while let Some(p) = rest.find('%') {
        out.push_str(&rest[..p]);
        let spec = rest.get(p + 1..p + 2);
        if matches!(spec, Some("s") | Some("d")) {
            out.push_str(args.next().copied().unwrap_or(""));
            rest = &rest[p + 2..];
        } else {
            out.push('%');
            rest = &rest[p + 1..];
        }
    }
    out.push_str(rest);
    out
}

/// The first `n` printable characters of `text`, colour codes kept;
/// `n == 0` keeps everything.
pub(super) fn truncate_visible(text: &str, n: usize) -> String {
    if n == 0 {
        return text.to_string();
    }
    let mut out = String::new();
    let mut count = 0;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '^'
            && let Some(&d) = chars.peek()
            && d.is_ascii_digit()
        {
            out.push(c);
            out.push(d);
            chars.next();
            continue;
        }
        if count == n {
            break;
        }
        out.push(c);
        count += 1;
    }
    out
}

/// The pulse of a focused item's colours (ui_mp_x86.dll 1.1
/// `Item_TextColor` 0x40013e3b..0x40013ec4, the value paints alike;
/// cod11-front-end.md 20.12): from `c` toward `low` by `(sin(realTime / 75)
/// + 1) / 2`, the division an int's, each channel clamped to 0..1
/// (`LerpColor` 0x4000f160).
pub(super) fn pulse(c: [f32; 4], low: [f32; 4], real_time: i32) -> [f32; 4] {
    let t = ((f64::from(real_time / 75).sin() + 1.0) * 0.5) as f32;
    std::array::from_fn(|i| (c[i] + t * (low[i] - c[i])).clamp(0.0, 1.0))
}

/// The low end of a pulse: 0.8 of the colour (`0x40030010`).
fn dim(c: [f32; 4]) -> [f32; 4] {
    c.map(|v| v * 0.8)
}

/// Milliseconds since the first edit cursor drawn, the base the renderer
/// takes on its first cursor (CoDMP.exe 0x4d8118..0x4d813c).
fn blink_clock() -> u128 {
    static BASE: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    BASE.get_or_init(std::time::Instant::now)
        .elapsed()
        .as_millis()
}

/// The edit cursor shows in the odd 256 ms steps of [`blink_clock`]
/// (`sar 8; test 1`, CoDMP.exe 0x4d8142..0x4d8150); CoD2's in the even
/// ones, off the engine clock with no base taken (`gfx_d3d_mp_x86_s.dll`
/// 0x10032861..0x10032875), whose phase vcod's first-cursor clock stands in
/// for.
fn cursor_blink_on(ms: u128, cod2: bool) -> bool {
    ((ms >> 8) & 1 == 1) != cod2
}

/// The types `Item_HandleKey` edits (`ui_mp_x86.dll` 1.1 0x400139da,
/// 0x40013a42: 4, 9 and 15); 15 types upper case (0x400127a5). CoD2's
/// test (CoD2MP_s.exe 0x53d4d0) adds its decimal field; its 16 and 18
/// read as vcod's 4 and 15.
pub(crate) fn is_edit_field(kind: i32) -> bool {
    matches!(
        kind,
        ITEM_TYPE_EDITFIELD
            | ITEM_TYPE_NUMERICFIELD
            | ITEM_TYPE_UPREDITFIELD
            | ITEM_TYPE_DECIMALFIELD
    )
}

/// Whether the item's paint runs `Item_Text_Paint` (0x400144d0), which
/// sets its text rect: text items and buttons from `Item_Paint`'s switch
/// (0x40016470), the edit fields' paint (0x40014600) always, the slider's,
/// yes/no's, multi's, bind's and owner draw's only when the item has text.
pub(super) fn paints_text(item: &UiItem) -> bool {
    match item.kind {
        ITEM_TYPE_TEXT
        | ITEM_TYPE_BUTTON
        | ITEM_TYPE_EDITFIELD
        | ITEM_TYPE_NUMERICFIELD
        | ITEM_TYPE_UPREDITFIELD
        | ITEM_TYPE_DECIMALFIELD => true,
        ITEM_TYPE_OWNERDRAW | ITEM_TYPE_SLIDER | ITEM_TYPE_YESNO | ITEM_TYPE_MULTI
        | ITEM_TYPE_DVARENUM | ITEM_TYPE_BIND => !item.text.is_empty(),
        _ => false,
    }
}

/// A centred edit field with a cvar adds the cvar's text width to the one
/// its alignment halves (0x40013c9a..0x40013cf0).
fn takes_cvar_width(item: &UiItem) -> bool {
    matches!(
        item.kind,
        ITEM_TYPE_EDITFIELD
            | ITEM_TYPE_NUMERICFIELD
            | ITEM_TYPE_UPREDITFIELD
            | ITEM_TYPE_DECIMALFIELD
    ) && item.text_align == ITEM_ALIGN_CENTER
}

/// What [`text_extents`] measures, on the 640 grid: the label, an owner
/// draw's value, a centred edit field's cvar text and the font's height.
struct TextWidths {
    label: f32,
    value: f32,
    cvar: f32,
    height: f32,
}

/// `Item_SetTextExtents` (0x40013bb0) and `ToWindowCoords` (0x40013b70):
/// the text rect (left, baseline, width, height). A `kept` rect with a
/// width stands unless the item is aligned 3 or centred on an owner draw
/// or a cvar. Widths and the height are ints, as the renderer's text width
/// truncates (`R_TextWidth` 0x4df310 ends in `_ftol`); the label's width
/// plus an owner draw's value (centred or right aligned), a centred field's
/// cvar text, or for alignment 3 the label again, shifts the left edge by
/// all of it (right) or half, rounded toward zero (centred and 3).
fn text_extents(item: &UiItem, kept: Option<[f32; 4]>, widths: TextWidths) -> [f32; 4] {
    let align = item.text_align;
    let remeasure = align == ITEM_ALIGN_CENTER2
        || (align == ITEM_ALIGN_CENTER
            && (item.kind == ITEM_TYPE_OWNERDRAW || item.cvar.is_some()));
    if let Some(r) = kept.filter(|r| r[2] as i32 != 0 && !remeasure) {
        return r;
    }
    let w = widths.label as i32;
    let extra = if item.kind == ITEM_TYPE_OWNERDRAW
        && matches!(align, ITEM_ALIGN_CENTER | ITEM_ALIGN_RIGHT)
    {
        widths.value as i32
    } else if takes_cvar_width(item) && item.cvar.is_some() {
        widths.cvar as i32
    } else if align == ITEM_ALIGN_CENTER2 {
        w
    } else {
        0
    };
    let total = w + extra;
    let shift = match align {
        ITEM_ALIGN_RIGHT => total,
        ITEM_ALIGN_CENTER | ITEM_ALIGN_CENTER2 => total / 2,
        _ => 0,
    };
    let b = if item.border != 0 {
        item.border_size
    } else {
        0.0
    };
    [
        item.rect[0] + item.text_align_x - shift as f32 + b,
        item.rect[1] + item.text_align_y + b,
        w as f32,
        widths.height as i32 as f32,
    ]
}

/// `Menu_Paint` (ui_mp_x86.dll 1.1 0x40016cf0) paints the menu's own
/// window over its rect before the items (0x40016d8d); `Window_Paint`
/// draws nothing for style 0 with no border (0x4000f961..0x4000f96d).
fn menu_window(p: &Painter, menu: &UiMenu, out: &mut Vec<HudQuad>) {
    let w = &menu.window;
    if w.style == 0 && w.border == 0 {
        return;
    }
    let mut window = w.clone();
    window.rect = menu.rect;
    let st = ItemState {
        visible: true,
        back: w.back,
        border_color: w.border_color,
        edit: Default::default(),
    };
    p.window(&window, &st, None, out);
}

/// `Window_Paint`'s border shader for style 4 (`DC->Assets.gradientBar`,
/// registered from `0x4002f48c` unless `assetGlobalDef` names another).
const GRADIENT_BAR: &str = "ui/assets/gradientbar2.tga";

/// `Window_Paint`'s border (ui_mp_x86.dll 1.1 0x4000fb64..0x4000fcf2,
/// cod11-front-end.md 20.10) over the rect's edges `[left, top, right,
/// bottom]`, `bs` its size and `one` one unit, as edges and a shader: 1
/// (full) the top and bottom across the width and then the sides down the
/// whole height, so the corners draw twice; 2 (horizontal) the top and
/// bottom; 3 (vertical) the sides; 4 the gradient bar `bs` tall across the
/// top and again from one unit above the bottom, when `gradient` (CoD1).
fn border_pieces(
    border: i32,
    [l, t, r, b]: [f32; 4],
    bs: f32,
    one: f32,
    gradient: bool,
) -> Vec<([f32; 4], &'static str)> {
    let top = ([l, t, r, t + bs], "white");
    let bottom = ([l, b - bs, r, b], "white");
    let left = ([l, t, l + bs, b], "white");
    let right = ([r - bs, t, r, b], "white");
    match border {
        1 => vec![top, bottom, left, right],
        2 => vec![top, bottom],
        3 => vec![left, right],
        4 if gradient => vec![
            ([l, t, r, t + bs], GRADIENT_BAR),
            ([l, b - one, r, b - one + bs], GRADIENT_BAR),
        ],
        _ => Vec::new(),
    }
}

/// An upright quad over window px `[left, top, right, bottom]`.
fn quad_px([l, t, r, b]: [f32; 4], rgba: [f32; 4], texture: &str, out: &mut Vec<HudQuad>) {
    out.push(HudQuad {
        verts: [[l, t], [r, t], [r, b], [l, b]],
        uvs: HudQuad::UNIT_UVS,
        rgba,
        texture: texture.to_string(),
    });
}

/// A text draw's font handle and scale: an item's `textfont` and
/// `textscale`, or a bare scale in the default handle.
#[derive(Clone, Copy)]
pub(crate) struct Size {
    font: i32,
    scale: f32,
}

impl From<f32> for Size {
    fn from(scale: f32) -> Size {
        Size { font: 0, scale }
    }
}

impl From<&UiItem> for Size {
    fn from(item: &UiItem) -> Size {
        Size {
            font: item.text_font,
            scale: item.text_scale,
        }
    }
}

/// The font a `textfont` handle draws in at `scale` on a screen `screen_h`
/// tall (ui_mp_x86.dll 1.1 0x400079e0, CoD2MP_s.exe 0x532410;
/// cod11-front-end.md 20.9): 2 `bigFont`, 3 `smallFont` and 5 `consoleFont`
/// at any size, 4 small, normal or bold by drawn size (the hudelems'
/// `bigfixed` pick), any other the default pick.
fn ui_font(fonts: &UiFonts, font: i32, scale: f32, screen_h: f32) -> &font::Font {
    match font {
        2 => &fonts.big,
        3 => &fonts.small,
        4 => fonts.pick(Slot::BigFixed, scale, screen_h),
        5 => &fonts.console,
        _ => fonts.pick(Slot::Default, scale, screen_h),
    }
}

/// Paints menu items on the 640 grid, drawn in the window's centred 4:3
/// box ([`Virtual`]); a rect that spans the whole grid width or height
/// (a backdrop, a fade) spans the window's.
pub(crate) struct Painter<'a> {
    pub(crate) v: Virtual,
    pub(crate) fonts: Option<&'a UiFonts>,
    /// CoD2: every rect maps straight through `v`, none stretched to the
    /// window, as its rects were already placed ([`cod2`]); text measures
    /// unrounded. False on CoD1, whose text traps return ints.
    pub(crate) exact: bool,
}

impl Painter<'_> {
    pub(crate) fn quad(&self, r: [f32; 4], rgba: [f32; 4], texture: &str, out: &mut Vec<HudQuad>) {
        if !self.exact {
            out.push(self.v.quad_fill(r[0], r[1], r[2], r[3], rgba, texture));
            return;
        }
        let [x0, y0] = self.v.point(r[0], r[1]);
        let [x1, y1] = self.v.point(r[0] + r[2], r[1] + r[3]);
        out.push(HudQuad {
            verts: [[x0, y0], [x1, y0], [x1, y1], [x0, y1]],
            uvs: [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
            rgba,
            texture: texture.to_string(),
        });
    }

    pub(crate) fn fill(&self, r: [f32; 4], rgba: [f32; 4], out: &mut Vec<HudQuad>) {
        if rgba[3] > 0.0 {
            self.quad(r, rgba, "white", out);
        }
    }

    /// `Window_Paint`: the fill or shader inside the border, then the
    /// border ([`border_pieces`]) in `bordercolor`. `cvar_shader` is a
    /// `WINDOW_STYLE_CVAR_SHADER` item's shader; an empty one draws nothing.
    /// A bordered rect spanning the grid stretches with its border: its
    /// pieces are cut out of the stretched rect in window px.
    fn window(
        &self,
        item: &UiItem,
        st: &ItemState,
        cvar_shader: Option<&str>,
        out: &mut Vec<HudQuad>,
    ) {
        let face = match (item.style, &item.background) {
            (WINDOW_STYLE_FILLED, Some(bg)) => Some((st.back, bg.clone())),
            (WINDOW_STYLE_FILLED, None) => (st.back[3] > 0.0).then(|| (st.back, "white".into())),
            // Style 3 draws whatever handle it holds, and a missing image's
            // or no background's is 0, the default shader (0x4000fa79).
            (WINDOW_STYLE_SHADER, Some(bg)) => {
                Some((item.fore, crate::hud::default_if_missing(bg)))
            }
            (WINDOW_STYLE_SHADER, None) => Some((item.fore, crate::hud::DEFAULT_SHADER.into())),
            (WINDOW_STYLE_CVAR_SHADER, _) => cvar_shader
                .filter(|b| !b.is_empty())
                .map(|bg| (item.fore, bg.to_string())),
            _ => None,
        };
        let [x, y, w, h] = item.rect;
        let b = item.border_size;
        let spans = full_width(x, w) || (y <= 0.0 && y + h >= 480.0);
        let c = st.border_color;
        // CoD2's `Window_Paint` has no gradient border (0x53a2df).
        let gradient = !self.exact;
        if item.border != 0 && spans && !self.exact {
            let q = self.v.quad_fill(x, y, w, h, [0.0; 4], "");
            let [l, t] = q.verts[0];
            let [r, bot] = q.verts[2];
            let (bs, one) = (b * self.v.scale, self.v.scale);
            if let Some((rgba, tex)) = face {
                quad_px([l + bs, t + bs, r - one, bot - one], rgba, &tex, out);
            }
            if c[3] > 0.0 {
                for (e, tex) in border_pieces(item.border, [l, t, r, bot], bs, one, gradient) {
                    quad_px(e, c, tex, out);
                }
            }
            return;
        }
        let mut inner = item.rect;
        if item.border != 0 {
            inner = [x + b, y + b, w - (b + 1.0), h - (b + 1.0)];
        }
        if let Some((rgba, tex)) = face {
            self.quad(inner, rgba, &tex, out);
        }
        if c[3] > 0.0 {
            for ([l, t, r, bot], tex) in
                border_pieces(item.border, [x, y, x + w, y + h], b, 1.0, gradient)
            {
                self.quad([l, t, r - l, bot - t], c, tex, out);
            }
        }
    }

    /// `Item_Text_AutoWrapped_Paint` (0x40014000): the lines `text` breaks
    /// into, each with its left edge and baseline on the 640 grid. A text
    /// wider than the rect aims at `width / ceil(width / rect.w)` a line, so
    /// the lines come out even: a line breaks at a space past that aim, at
    /// the last space once it is past the rect's width, and at `\n`. A
    /// line aligns on the width measured one character short of its last
    /// word's end, as retail's lagging measure does, and lines sit the
    /// text height plus 5 apart; `\r` reads as a space.
    fn wrap_lines(&self, text: &str, item: &UiItem) -> Vec<(String, f32, f32)> {
        let scale = Size::from(item);
        let rect_w = item.rect[2];
        let total = self.width(text, scale);
        let aim = if total > rect_w {
            (total / (total / rect_w).ceil()).trunc()
        } else {
            rect_w.trunc()
        };
        let (x0, mut y) = Ui::text_origin(item, 0.0, 0.0, true);
        let step = self.height(scale) + 5.0;
        let chars: Vec<char> = text.chars().collect();
        let mut lines = Vec::new();
        let mut buf: Vec<char> = Vec::new();
        let (mut p, mut new_line, mut new_ptr) = (0, 0, 0);
        let (mut measured, mut line_w) = (total, 0.0);
        loop {
            let c = chars.get(p).copied().unwrap_or('\0');
            if matches!(c, ' ' | '\t' | '\n' | '\0') {
                new_line = buf.len();
                new_ptr = p + 1;
                line_w = measured;
            }
            measured = self.width(&buf.iter().collect::<String>(), scale);
            let breaks = (new_line != 0 && measured > rect_w)
                || matches!(c, '\n' | '\0')
                || (c == ' ' && measured > aim);
            if !breaks {
                buf.push(if c == '\r' { ' ' } else { c });
                p += 1;
                continue;
            }
            if !buf.is_empty() {
                let x = match item.text_align {
                    ITEM_ALIGN_RIGHT => x0 - line_w,
                    ITEM_ALIGN_CENTER => x0 - line_w / 2.0,
                    _ => x0,
                };
                lines.push((buf[..new_line].iter().collect(), x, y));
            }
            if c == '\0' {
                return lines;
            }
            buf.clear();
            new_line = 0;
            line_w = 0.0;
            y += step;
            p = new_ptr;
        }
    }

    /// The font the UI picks for `size` on this window.
    pub(super) fn font(&self, size: impl Into<Size>) -> Option<&font::Font> {
        let Size { font, scale } = size.into();
        self.fonts
            .map(|f| ui_font(f, font, scale, 480.0 * self.v.scale))
    }

    /// Width on the 640 grid: on CoD1 the DC's `textWidth`, syscall 0x43's
    /// int ([`font::trap_width`]); CoD2's unrounded.
    pub(crate) fn width(&self, text: &str, size: impl Into<Size>) -> f32 {
        let size = size.into();
        let scale = size.scale;
        self.font(size).map_or(0.0, |f| match self.exact {
            false => font::trap_width(f, text, scale),
            true => font::measure(f, text, scale / f.unit_scale()),
        })
    }

    /// The text height on the 640 grid: on CoD1 the DC's `textHeight`,
    /// syscall 0x44's int ([`font::trap_height`]); on CoD2 the font's
    /// tallest glyph at `scale`, as far above the baseline as
    /// [`Self::text`] puts the top.
    pub(crate) fn height(&self, size: impl Into<Size>) -> f32 {
        let size = size.into();
        let scale = size.scale;
        self.font(size).map_or(0.0, |f| match self.exact {
            false => font::trap_height(f, scale),
            true => f.max_height as f32 * f.glyph_scale * scale,
        })
    }

    /// `text` with its baseline at grid `(x, y)`, in an item's `textstyle`
    /// (every 1.1 `Item_*_Paint` passes item `+0xe4`, CoD2's `+0x290`
    /// through `UI_DrawText`).
    pub(crate) fn text(
        &self,
        text: &str,
        x: f32,
        y: f32,
        size: impl Into<Size>,
        color: [f32; 4],
        style: i32,
        out: &mut Vec<HudQuad>,
    ) {
        let size = size.into();
        let scale = size.scale;
        let Some(f) = self.font(size) else { return };
        let top = y - f.max_height as f32 * f.glyph_scale * scale;
        let [px, py] = self.v.point(x, top);
        let px_scale = scale * self.v.scale / f.unit_scale();
        let style = match self.exact {
            true => TextStyle::Cod2 { style, snap: true },
            false => TextStyle::Retail(style),
        };
        font::layout_styled(f, text, px, py, px_scale, color, style, self.v.scale, out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::collections::HashMap;
    use std::net::SocketAddrV4;

    use super::super::tests::{main_at_start, start_as_text, ui, ui_with_menu};
    use super::super::{UiEffect, browser};

    fn widths(label: f32, value: f32, cvar: f32) -> TextWidths {
        TextWidths {
            label,
            value,
            cvar,
            height: 12.9,
        }
    }

    /// Menu text takes the item's `textstyle`: CoD1's shadow offset is in
    /// virtual units, CoD2's in window px (cod2-fonts-menus.md 1.1).
    #[test]
    fn menu_text_draws_in_the_items_style() {
        let fonts = crate::hud::hudelem::tests::test_fonts();
        let draw = |exact, style| {
            let p = Painter {
                v: Virtual::new((1280.0, 960.0)),
                fonts: Some(&fonts),
                exact,
            };
            let mut out = Vec::new();
            p.text("a", 0.0, 20.0, 0.3, [1.0; 4], style, &mut out);
            out
        };
        let offset = |q: &[HudQuad]| q[0].verts[0][0] - q[1].verts[0][0];
        assert_eq!(draw(false, 0).len(), 1);
        let q = draw(false, 6);
        assert_eq!((offset(&q), q[0].rgba[3]), (4.0, 1.0));
        assert_eq!(draw(true, 0).len(), 1);
        let q = draw(true, 6);
        assert_eq!((offset(&q), q[0].rgba[3]), (2.0, 1.0));
    }

    /// `textfont` picks the atlas as `0x400079e0` does: 2, 3 and 5 at any
    /// size, 4 small, normal or bold by drawn size, the rest by drawn size.
    #[test]
    fn textfont_picks_the_retail_font() {
        let font = |size| font::Font {
            size,
            ..crate::hud::hudelem::tests::test_font()
        };
        let fonts = UiFonts {
            small: font(12),
            normal: font(16),
            console: font(18),
            big: font(24),
            bold: font(30),
            extra_big: font(32),
        };
        let size = |handle, scale| ui_font(&fonts, handle, scale, 480.0).size;
        for scale in [0.2, 0.6] {
            assert_eq!([2, 3, 5].map(|h| size(h, scale)), [24, 12, 18]);
        }
        assert_eq!([0.25, 0.3, 0.6].map(|s| size(4, s)), [12, 16, 30]);
        for h in [0, 1, 6] {
            assert_eq!([0.25, 0.3, 0.45, 0.6].map(|s| size(h, s)), [12, 16, 24, 32]);
        }
        // The item's handle reaches the painter.
        let p = Painter {
            v: Virtual::new((640.0, 480.0)),
            fonts: Some(&fonts),
            exact: false,
        };
        let item = |text_font| UiItem {
            text_font,
            text_scale: 0.3,
            ..UiItem::default()
        };
        assert_eq!(p.font(&item(1)).map(|f| f.size), Some(16));
        assert_eq!(p.font(&item(2)).map(|f| f.size), Some(24));
    }

    /// `Window_Paint` style 3 draws its handle whatever it is: no
    /// background is the default shader, a named one falls back to it.
    #[test]
    fn a_style_3_window_draws_the_default_shader_for_no_image() {
        let p = Painter {
            v: Virtual::new((640.0, 480.0)),
            fonts: None,
            exact: false,
        };
        let mut item = UiItem {
            style: WINDOW_STYLE_SHADER,
            rect: [10.0, 10.0, 50.0, 20.0],
            ..UiItem::default()
        };
        let st = ItemState {
            visible: true,
            back: [0.0; 4],
            border_color: [0.0; 4],
            edit: Default::default(),
        };
        let mut out = Vec::new();
        p.window(&item, &st, None, &mut out);
        assert_eq!(out[0].texture, crate::hud::DEFAULT_SHADER);
        item.background = Some("ui_mp/assets/nothing_here".into());
        out.clear();
        p.window(&item, &st, None, &mut out);
        assert_eq!(
            out[0].texture,
            crate::hud::default_if_missing("ui_mp/assets/nothing_here")
        );
    }

    /// A bordered full-width window on a wide window stretches with its
    /// sides at the window's edges and its fill between them.
    #[test]
    fn a_bordered_full_width_window_stretches_whole() {
        let p = Painter {
            v: Virtual::new((1280.0, 480.0)),
            fonts: None,
            exact: false,
        };
        let item = UiItem {
            style: WINDOW_STYLE_FILLED,
            rect: [0.0, 100.0, 640.0, 50.0],
            border: 1,
            border_size: 2.0,
            ..UiItem::default()
        };
        let st = ItemState {
            visible: true,
            back: [0.0, 0.0, 0.0, 0.5],
            border_color: [1.0; 4],
            edit: Default::default(),
        };
        let mut out = Vec::new();
        p.window(&item, &st, None, &mut out);
        let span = |q: &HudQuad| (q.verts[0][0], q.verts[2][0]);
        assert_eq!(span(&out[0]), (2.0, 1279.0), "the fill");
        assert_eq!(span(&out[3]), (0.0, 2.0), "the left side");
        assert_eq!(span(&out[4]), (1278.0, 1280.0), "the right side");
    }

    /// `Window_Paint`'s borders: full sides down the whole height over the
    /// top and bottom, the horizontal and vertical pairs alone, and on CoD1
    /// the gradient bar `bordersize` tall at the top and one unit above the
    /// bottom.
    #[test]
    fn borders_follow_window_paint() {
        let r = [10.0, 20.0, 110.0, 70.0];
        let edges = |border, gradient| {
            border_pieces(border, r, 2.0, 1.0, gradient)
                .into_iter()
                .map(|(e, tex)| (e, tex == GRADIENT_BAR))
                .collect::<Vec<_>>()
        };
        let top = ([10.0, 20.0, 110.0, 22.0], false);
        let bottom = ([10.0, 68.0, 110.0, 70.0], false);
        let left = ([10.0, 20.0, 12.0, 70.0], false);
        let right = ([108.0, 20.0, 110.0, 70.0], false);
        assert_eq!(edges(1, true), [top, bottom, left, right]);
        assert_eq!(edges(2, true), [top, bottom]);
        assert_eq!(edges(3, true), [left, right]);
        assert_eq!(
            edges(4, true),
            [
                ([10.0, 20.0, 110.0, 22.0], true),
                ([10.0, 69.0, 110.0, 71.0], true)
            ]
        );
        assert!(edges(4, false).is_empty() && edges(5, true).is_empty());
    }

    /// A popup's own window (`del_fav_popmenu`'s) paints under its items:
    /// the fill inside the border, then the border.
    #[test]
    fn a_menus_own_window_paints_under_its_items() {
        let mut ui = ui_with_menu(
            r#"{ menuDef { name "main" rect 200 220 250 80 style 1
            backcolor .1 .15 .1 1 border 1 bordercolor .5 .5 .5 .5
            itemDef { visible 1 rect 0 0 10 10 style 1 backcolor 1 1 1 1 decoration } } }"#,
        );
        let shell = Shell::new();
        ui.open_main(&shell, &mut Vec::new());
        let quads = ui.build(640.0, 480.0, &Localized::default(), &shell);
        let rgba: Vec<_> = quads.iter().map(|q| q.rgba).collect();
        let (fill, edge) = ([0.1, 0.15, 0.1, 1.0], [0.5; 4]);
        assert_eq!(rgba, [fill, edge, edge, edge, edge, [1.0; 4]]);
        assert_eq!(quads[0].verts[0], [201.0, 221.0]);
        assert_eq!(quads[0].verts[2], [449.0, 299.0]);
    }

    /// The pulse runs from the colour to 0.8 of it on `sin(realTime / 75)`,
    /// steps each 75 ms and clamps each channel.
    #[test]
    fn the_focus_pulse_follows_item_text_color() {
        let c = [1.0, 0.5, 0.0, 1.0];
        let mid = [0.9, 0.45, 0.0, 0.9];
        assert_eq!(pulse(c, dim(c), 0), mid);
        assert_eq!(pulse(c, dim(c), 74), mid);
        // sin(11) is -0.99999: the full colour; sin(8) is 0.989: near 0.8.
        let near = |a: [f32; 4], b: [f32; 4]| a.iter().zip(b).all(|(x, y)| (x - y).abs() < 0.003);
        assert!(near(pulse(c, dim(c), 11 * 75), c));
        assert!(near(pulse(c, dim(c), 8 * 75), dim(c)));
        assert_eq!(pulse([1.2; 4], [1.2; 4], 0), [1.0; 4]);
    }

    /// A focused slider's bar and thumb pulse with its text.
    #[test]
    fn a_focused_slider_pulses() {
        let mut ui = ui_with_menu(
            r#"{ menuDef { name "main" rect 0 0 640 480 focuscolor 1 .5 0 1
            itemDef { type 10 visible 1 rect 10 10 100 20 cvarFloat "s_volume" 0.5 0 1 } } }"#,
        );
        let shell = Shell::new();
        let loc = Localized::default();
        ui.open_main(&shell, &mut Vec::new());
        let t0 = std::time::Instant::now();
        ui.frame(t0, &shell);
        ui.frame(t0 + std::time::Duration::from_millis(1234), &shell);
        let m = ui.find("main").unwrap();
        let focus = [1.0, 0.5, 0.0, 1.0];
        let lit = pulse(focus, dim(focus), 1234);
        assert_ne!(lit, pulse(focus, dim(focus), 0));
        ui.focus.insert(m, 0);
        let quads = ui.build(640.0, 480.0, &loc, &shell);
        let slider: Vec<_> = (quads.iter())
            .filter(|q| q.texture.contains("slider"))
            .map(|q| q.rgba)
            .collect();
        assert_eq!(slider, [lit, lit]);
    }

    /// The bind whose wait last started shows its keys in a pulse toward
    /// dark red while it has the focus, during the wait and after it.
    #[test]
    fn the_last_waiting_bind_pulses_red() {
        let mut ui = ui_with_menu(
            r#"{ menuDef { name "main" rect 0 0 640 480 focuscolor 1 .5 0 1
            itemDef { type 13 visible 1 rect 10 40 100 20 text "Fire" cvar "+attack" } } }"#,
        );
        ui.fonts = Some(crate::hud::hudelem::tests::test_fonts());
        let mut shell = Shell::new();
        shell.execute("bind f +attack");
        let loc = Localized::default();
        ui.open_main(&shell, &mut Vec::new());
        let m = ui.find("main").unwrap();
        ui.focus.insert(m, 0);
        let focus = [1.0, 0.5, 0.0, 1.0];
        let (lit, red) = (
            pulse(focus, dim(focus), 0),
            pulse(focus, [0.8, 0.0, 0.0, 0.8], 0),
        );
        let count = |ui: &Ui, c| {
            (ui.build(640.0, 480.0, &loc, &shell).iter())
                .filter(|q| q.rgba == c)
                .count()
        };
        let keys = (super::super::options::bind_text("+attack", &loc, &shell).chars()).count();
        assert!(keys > 0);
        assert_eq!((count(&ui, lit), count(&ui, red)), (4 + keys, 0));
        ui.options.waiting = Some((m, 0));
        ui.options.bind_item = Some((m, 0));
        assert_eq!((count(&ui, lit), count(&ui, red)), (4, keys));
        ui.options.waiting = None;
        assert_eq!(count(&ui, red), keys);
    }

    /// `Item_SetTextExtents`: int widths, a centred shift rounded toward
    /// zero, alignment 3 measured as right aligned and never kept, and a
    /// centred owner draw measured again with its value's width.
    #[test]
    fn text_extents_follow_item_set_text_extents() {
        let item = UiItem {
            kind: ITEM_TYPE_BUTTON,
            rect: [100.0, 50.0, 200.0, 20.0],
            text_align: ITEM_ALIGN_CENTER,
            text_align_x: 40.0,
            text_align_y: 15.0,
            ..UiItem::default()
        };
        assert!(paints_text(&item));
        let r = text_extents(&item, None, widths(25.7, 0.0, 0.0));
        assert_eq!(r, [128.0, 65.0, 25.0, 12.0], "x 140 less 25 / 2 = 12");
        // A button keeps its first rect whatever the text becomes.
        assert_eq!(text_extents(&item, Some(r), widths(90.0, 0.0, 0.0)), r);
        // Alignment 3 sits like a right aligned label and is measured each
        // paint.
        let three = UiItem {
            text_align: ITEM_ALIGN_CENTER2,
            ..item.clone()
        };
        assert_eq!(
            text_extents(&three, Some(r), widths(30.2, 0.0, 0.0)),
            [110.0, 65.0, 30.0, 12.0]
        );
        // A centred owner draw adds its value and is measured each paint.
        let owner = UiItem {
            kind: ITEM_TYPE_OWNERDRAW,
            text: "Source:".into(),
            ..item.clone()
        };
        assert_eq!(
            text_extents(&owner, Some(r), widths(30.0, 21.0, 0.0))[0],
            140.0 - 25.0
        );
        // A centred edit field with a cvar halves its cvar text too.
        let field = UiItem {
            kind: ITEM_TYPE_EDITFIELD,
            cvar: Some("name".into()),
            ..item.clone()
        };
        assert_eq!(
            text_extents(&field, None, widths(30.0, 0.0, 41.0))[0],
            140.0 - 35.0
        );
        // An owner draw or a multi with no text runs no `Item_Text_Paint`.
        assert!(!paints_text(&UiItem {
            text: String::new(),
            ..owner
        }));
    }

    /// The UI's `textWidth` and `textHeight` traps return ints: R_TextWidth
    /// truncates the widest line, R_TextHeight `scale` times the font's
    /// second header float (CoDMP.exe 0x4df310, 0x4df540).
    #[test]
    fn the_ui_text_traps_return_ints() {
        let Some(ui) = ui() else { return };
        let p = Painter {
            v: Virtual::new((640.0, 480.0)),
            fonts: ui.fonts.as_ref(),
            exact: false,
        };
        let f = p.font(0.4).unwrap();
        assert_eq!(
            p.height(0.4),
            (0.4f64 * f64::from(f.line_advance)).trunc() as f32
        );
        let w = p.width("Join Game", 0.4);
        assert_eq!(w, w.trunc());
        assert_eq!(p.width("Join Game\nab", 0.4), w);
        assert_eq!(p.width("^1Join ^7Game", 0.4), w);
    }

    /// The driver info page's `UI_GLINFO` over its stock rect: the three
    /// lines, then 35 rows of three extensions down to y 465.
    #[test]
    fn the_driver_info_lays_out_as_ui_draw_gl_info() {
        let info = GpuInfo {
            vendor: "radv".into(),
            renderer: "gpu".into(),
            version: "v".repeat(100),
            extensions: (0..300)
                .map(|i| format!("EXT_{i:040}"))
                .collect::<Vec<_>>()
                .join(" "),
            ..GpuInfo::default()
        };
        let lines = gl_info_lines(&info, [10.0, 80.0, 545.0, 400.0]);
        assert_eq!(lines[0], ("VENDOR: radv".into(), 12.0, 80.0));
        assert_eq!(lines[1].0.chars().count(), 80);
        assert_eq!((lines[1].2, lines[2].2), (95.0, 110.0));
        assert_eq!(lines.len(), 3 + 105);
        assert_eq!((lines[3].1, lines[3].2), (12.0, 125.0));
        assert_eq!(lines[3].0.chars().count(), 36);
        assert_eq!(lines[4].1, 12.0 + 545.0 / 3.0);
        assert_eq!(lines.last().unwrap().2, 465.0);
    }

    #[test]
    fn a_wheel_notch_flicks_the_focus_to_the_next_item_and_back() {
        let Some(mut ui) = ui() else { return };
        let shell = Shell::new();
        ui.open_main(&Shell::new(), &mut Vec::new());
        let m = ui.find("main").unwrap();
        let at = |ui: &Ui, text: &str| {
            ui.menus[m]
                .items
                .iter()
                .position(|i| i.text == text)
                .unwrap()
        };
        let start = at(&ui, "@MENU_START_NEW_SERVER");
        let r = ui.menus[m].items[start].rect;
        ui.mouse_move(r[0] + 2.0, r[1] + 2.0, 640.0, 480.0, &shell);
        // The next shown item's `mouseEnter`, then the hovered one's again.
        let out = ui.wheel(true, &shell);
        let over = UiEffect::Sound("mouse_over".into());
        assert_eq!(out, vec![over.clone(), over]);
        assert_eq!(ui.hover, Some((m, start)));
        assert_eq!(ui.cursor_item.get(&m), Some(&start));
    }

    /// `Item_SetTextExtents` keeps the first width: a left-aligned text
    /// item whose text grows keeps its click rect, and a centred cvar item
    /// measures again.
    #[test]
    fn a_text_rect_keeps_its_first_width_unless_centred_on_a_cvar() {
        let Some(mut ui) = ui() else { return };
        let shell = Shell::new();
        let (m, start) = start_as_text(&mut ui, &shell);
        let first = ui.text_rect(m, start).unwrap();
        ui.menus[m].items[start].text = "a much longer start label".into();
        ui.build(640.0, 480.0, &Localized::default(), &shell);
        assert_eq!(ui.text_rect(m, start), Some(first));

        let item = &mut ui.menus[m].items[start];
        item.text.clear();
        item.cvar = Some("ui_test_label".into());
        item.text_align = ITEM_ALIGN_CENTER;
        ui.set_server_cvars(HashMap::from([("ui_test_label".into(), "short".into())]));
        ui.build(640.0, 480.0, &Localized::default(), &shell);
        let short = ui.text_rect(m, start).unwrap();
        ui.set_server_cvars(HashMap::from([(
            "ui_test_label".into(),
            "a much longer label".into(),
        )]));
        ui.build(640.0, 480.0, &Localized::default(), &shell);
        let long = ui.text_rect(m, start).unwrap();
        assert!(
            long[2] > short[2] && long[0] < short[0],
            "{short:?} {long:?}"
        );
    }

    /// `Item_Text_AutoWrapped_Paint` evens its lines out: a text one and a
    /// half rects wide breaks near its middle, not where the rect fills.
    #[test]
    fn autowrapped_text_breaks_into_even_lines_and_its_rect_sits_on_the_last() {
        let Some(mut ui) = ui() else { return };
        let shell = Shell::new();
        let (m, start) = start_as_text(&mut ui, &shell);
        let text = ["word"; 12].join(" ");
        let p = Painter {
            v: Virtual::new((640.0, 480.0)),
            fonts: ui.fonts.as_ref(),
            exact: false,
        };
        let scale = ui.menus[m].items[start].text_scale;
        let total = p.width(&text, scale);
        let item = &mut ui.menus[m].items[start];
        item.text = text.clone();
        item.autowrap = true;
        item.text_align = 0;
        item.rect[2] = total / 1.5;
        let item = item.clone();
        let lines = p.wrap_lines(&text, &item);
        assert_eq!(lines.len(), 2, "{lines:?}");
        // Packing to the rect's width would put more words on the first.
        let mut greedy = 0;
        while p.width(&vec!["word"; greedy + 1].join(" "), scale) < item.rect[2] {
            greedy += 1;
        }
        let first = lines[0].0.split(' ').count();
        assert!(first < greedy, "{first} words, greedy {greedy}");
        assert_eq!(first + lines[1].0.split(' ').count(), 12);
        assert_eq!(lines[1].2 - lines[0].2, p.height(scale) + 5.0);

        ui.text_rects.borrow_mut().clear();
        ui.build(640.0, 480.0, &Localized::default(), &shell);
        let r = ui.text_rect(m, start).unwrap();
        let h = p.height(scale).trunc();
        assert_eq!([r[0], r[1] + h], [lines[1].1, lines[1].2]);
        assert_eq!(
            r[2],
            total.trunc(),
            "the rect keeps the whole text's int width"
        );
    }

    #[test]
    fn the_focus_colour_follows_the_focus_bit_not_the_hover() {
        let Some(mut ui) = ui() else { return };
        let shell = Shell::new();
        let (m, start) = main_at_start(&mut ui, &shell);
        let join = (ui.menus[m].items.iter())
            .position(|i| i.text == "@MENU_JOIN_GAME")
            .unwrap();
        ui.mouse_move(0.0, 479.0, 640.0, 480.0, &shell);
        ui.menus[m].items[join].rect = ui.menus[m].items[start].rect;
        let focus_color = [0.1, 0.2, 0.3, 1.0];
        ui.menus[m].focus_color = focus_color;
        let loc = Localized::default();
        let lit = |ui: &Ui| {
            let c = pulse(focus_color, dim(focus_color), ui.real_time);
            (ui.build(640.0, 480.0, &loc, &shell).iter())
                .filter(|q| q.rgba == c)
                .count()
        };
        let glyphs = |ui: &Ui, i: usize| {
            let label = loc.translate(&ui.menus[m].items[i].text).into_owned();
            label.chars().filter(|c| !c.is_whitespace()).count()
        };
        let r = ui.menus[m].items[start].rect;
        // Each move inside both hands the focus to the other one; the click
        // target stays the later item.
        for dx in [2.0, 3.0] {
            ui.mouse_move(r[0] + dx, r[1] + 2.0, 640.0, 480.0, &shell);
            let f = ui.focus[&m];
            assert_eq!(ui.hover, Some((m, start.max(join))));
            assert_eq!(lit(&ui), glyphs(&ui, f), "item {f} lit");
        }
    }

    /// On a 16:9 window the menus draw in the centred 4:3 box, the
    /// backdrop (full grid width) over the whole window, and the cursor
    /// maps through the box.
    #[test]
    fn a_wide_window_boxes_the_menus_and_stretches_the_backdrop() {
        let Some(mut ui) = ui() else { return };
        let shell = Shell::new();
        ui.open_main(&Shell::new(), &mut Vec::new());
        let quads = ui.build(1920.0, 1080.0, &Localized::default(), &shell);
        assert_eq!(quads[0].verts[0], [0.0, 0.0]);
        assert_eq!(quads[0].verts[2], [1920.0, 720.0]);
        let boxed = |q: &HudQuad| q.verts.iter().all(|[x, _]| (240.0..=1680.0).contains(x));
        assert!(quads.iter().skip(2).filter(|q| !boxed(q)).count() < quads.len() / 4);
        ui.mouse_move(960.0, 540.0, 1920.0, 1080.0, &shell);
        assert_eq!(ui.cursor, [320.0, 240.0]);
        ui.mouse_move(100.0, 540.0, 1920.0, 1080.0, &shell);
        assert_eq!(ui.cursor, [0.0, 240.0], "clamped to the box");
    }

    #[test]
    fn the_main_menu_draws_its_backdrop_and_buttons() {
        let Some(mut ui) = ui() else { return };
        let shell = Shell::new();
        ui.open_main(&Shell::new(), &mut Vec::new());
        let quads = ui.build(1280.0, 960.0, &Localized::default(), &shell);
        assert_eq!(
            quads[0].texture,
            crate::hud::default_if_missing("ui_mp/assets/main_back_top_mp.tga")
        );
        assert_eq!(quads[0].verts[2], [1280.0, 640.0]);
        assert!(quads.len() > 50, "button text drawn: {}", quads.len());
        assert_eq!(quads.last().unwrap().texture, "ui/assets/3_cursor3");
    }

    #[test]
    fn an_open_menu_draws_the_cursor_shader_centred_on_the_mouse() {
        let mut ui = ui_with_menu(
            r#"{ assetGlobalDef { cursor "ui/assets/3_cursor3" }
            menuDef { name "main" rect 0 0 640 480 } }"#,
        );
        let shell = Shell::new();
        let loc = Localized::default();
        assert!(ui.build(1280.0, 960.0, &loc, &shell).is_empty());
        ui.open_main(&shell, &mut Vec::new());
        ui.mouse_move(200.0, 100.0, 640.0, 480.0, &shell);
        let quads = ui.build(1280.0, 960.0, &loc, &shell);
        let cursor = quads.last().unwrap();
        assert_eq!(cursor.texture, "ui/assets/3_cursor3");
        assert_eq!(cursor.rgba, [1.0; 4]);
        assert_eq!(cursor.verts[0], [368.0, 168.0]);
        assert_eq!(cursor.verts[2], [432.0, 232.0]);
    }

    /// `Menu_PaintAll` (0x40018a00..0x40018a46) paints the stack from its
    /// topmost fullscreen menu up: a page a script left open under `main`
    /// draws nothing, a popup over `main` draws.
    #[test]
    fn menus_under_the_topmost_fullscreen_menu_paint_nothing() {
        let mut ui = ui_with_menu(
            r#"{ menuDef { name "page" rect 0 0 640 480
              itemDef { name "p" rect 10 10 20 20 style 1 backcolor 1 0 0 1 visible 1 decoration } }
            menuDef { name "main" fullscreen 1 rect 0 0 640 480
              itemDef { name "m" rect 50 50 20 20 style 1 backcolor 0 1 0 1 visible 1 decoration } }
            menuDef { name "pop" popup rect 0 0 640 480
              itemDef { name "q" rect 90 90 20 20 style 1 backcolor 0 0 1 1 visible 1 decoration } } }"#,
        );
        let shell = Shell::new();
        let loc = Localized::default();
        let colours = |ui: &Ui| -> Vec<[f32; 4]> {
            ui.build(640.0, 480.0, &loc, &shell)
                .iter()
                .map(|q| q.rgba)
                .collect()
        };
        ui.open_menu("page", &shell, &mut Vec::new());
        assert_eq!(colours(&ui), [[1.0, 0.0, 0.0, 1.0]]);
        ui.open_menu("main", &shell, &mut Vec::new());
        assert_eq!(
            colours(&ui),
            [[0.0, 1.0, 0.0, 1.0]],
            "the page is under main"
        );
        ui.open_menu("pop", &shell, &mut Vec::new());
        assert_eq!(
            colours(&ui),
            [[0.0, 1.0, 0.0, 1.0], [0.0, 0.0, 1.0, 1.0]],
            "main, then the popup over it"
        );
    }

    #[test]
    fn format_and_truncation() {
        assert_eq!(
            fill("Source:     %s", &["Internet"]),
            "Source:     Internet"
        );
        assert_eq!(fill("100%", &[]), "100%");
        // CoD2's numbered slots.
        assert_eq!(fill("Source: &&1", &["Local"]), "Source: Local");
        assert_eq!(truncate_visible("^1abc^7def", 4), "^1abc^7d");
        assert_eq!(truncate_visible("abc", 0), "abc");
    }

    #[test]
    fn the_refresh_date_prints_retails_format_and_its_june_key() {
        let Some(fs) = vcod_common::testing::stock_fs_11() else {
            return;
        };
        let loc = Localized::load(&fs);
        let t = jiff::civil::date(2026, 10, 11).at(9, 5, 0, 0);
        assert_eq!(refresh_date(t, &loc), "Oct 11, 2026   9:05");
        let t = jiff::civil::date(2026, 6, 1).at(23, 59, 0, 0);
        assert_eq!(refresh_date(t, &loc), "EXE_MONTH_ABV_JUN 1, 2026   23:59");
    }

    /// A refresh dates its source in `ui_lastServerRefresh_<source>`, a
    /// console cvar the date line reads back, so a `set` of it shows; a
    /// source never refreshed prints the label alone.
    #[test]
    fn the_refresh_date_lives_in_a_cvar() {
        let Some(mut ui) = ui() else { return };
        let fs = vcod_common::testing::stock_fs_11().unwrap();
        let loc = Localized::load(&fs);
        let mut shell = Shell::new();
        ui.browser.source = Source::Local;
        ui.browser.refresh();
        ui.browser.stop();
        ui.browser.stamps[0].1 = jiff::civil::date(2026, 10, 11).at(9, 5, 0, 0);
        let out = ui.date_refreshes(&loc);
        assert!(
            matches!(&out[..], [UiEffect::Command(c)]
                if c == "set ui_lastServerRefresh_0 \"Oct 11, 2026   9:05\""),
            "{out:?}"
        );
        super::super::tests::apply(&mut shell, &out);
        ui.options.new_input();
        let label = loc.translate("@EXE_REFRESHTIME");
        assert_eq!(
            ui.refresh_text(&loc, &shell),
            fill(&label, &["Oct 11, 2026   9:05"])
        );
        shell.execute("set ui_lastServerRefresh_0 later");
        assert_eq!(ui.refresh_text(&loc, &shell), fill(&label, &["later"]));
        ui.browser.source = Source::Favorites;
        assert_eq!(ui.refresh_text(&loc, &shell), fill(&label, &[""]));
        assert!(ui.date_refreshes(&loc).is_empty());
    }

    #[test]
    fn the_map_preview_is_the_selected_servers_levelshot_or_unknownmap() {
        let Some(mut ui) = ui() else { return };
        let addr: SocketAddrV4 = "10.0.0.1:28960".parse().unwrap();
        let mut s = browser::Server::new(addr);
        s.info = Some(vcod_common::net::master::ServerInfo {
            mapname: "MP_Harbor".into(),
            ..Default::default()
        });
        ui.browser.lists[0].push(s);
        assert_eq!(ui.map_preview(), "menu/art/unknownmap");
        ui.selected = Some(addr);
        assert_eq!(ui.map_preview(), "levelshots/mp_harbor");
        ui.browser.lists[0][0].info.as_mut().unwrap().mapname = "zz_custom".into();
        assert_eq!(ui.map_preview(), "menu/art/unknownmap");
    }

    /// The edit cursor blinks on the renderer's 256 ms steps: off first on
    /// CoD1, on first on CoD2.
    #[test]
    fn the_edit_cursor_shows_every_other_256_ms() {
        let on: Vec<bool> = [0, 255, 256, 511, 512, 768]
            .map(|ms| cursor_blink_on(ms, false))
            .to_vec();
        assert_eq!(on, [false, false, true, true, false, true]);
        assert!(cursor_blink_on(0, true) && !cursor_blink_on(256, true));
    }
}
