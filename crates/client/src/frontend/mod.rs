//! Contains routines ported from the RTCW-MP GPL source, Copyright (C) 1999-2010 id Software LLC, a ZeniMax Media company.
//! See NOTICE.
//!
//! The front end: the stock main menu, server browser, options screens,
//! quit and error popups drawn from their `.menu` files and driven by mouse
//! and keys, as the UI module does while no game is up
//! (docs/research/cod11-front-end.md). Menus vcod cannot run yet (create
//! server, mods) are refused with a console line instead of drawing screens
//! whose controls do nothing.

pub mod browser;
mod options;

use std::net::SocketAddrV4;
use std::time::{Duration, Instant};

use vcod_common::localize::Localized;
use vcod_common::pk3::Pk3Fs;
use vcod_common::ui_menu::{
    self, FEEDER_SERVERS, ITEM_ALIGN_CENTER, ITEM_ALIGN_RIGHT, ITEM_TYPE_LISTBOX, ITEM_TYPE_MULTI,
    ITEM_TYPE_OWNERDRAW, ITEM_TYPE_SLIDER, UiItem, UiMenu, WINDOW_STYLE_FILLED,
    WINDOW_STYLE_SHADER,
};
use winit::keyboard::KeyCode;

use crate::console::shell::Shell;
use crate::hud::HudQuad;
use crate::hud::font::{self, Slot, UiFonts};
use browser::{Browser, Status};

/// The files `ui_mp/menus.txt` loads that hold the menus vcod drives.
const MENU_FILES: [&str; 4] = [
    "ui_mp/main.menu",
    "ui_mp/joinserver.menu",
    "ui/quit.menu",
    "ui/error.menu",
];

/// Menus `open` may show, with [`options::MENUS`]. Anything else is refused
/// with a console line.
const SUPPORTED: [&str; 4] = ["main", "joinserver", "quit_popmenu", "error_popmenu"];

fn supported(name: &str) -> bool {
    SUPPORTED
        .iter()
        .chain(&options::MENUS)
        .any(|s| s.eq_ignore_ascii_case(name))
}

/// The browser's "New Favorite" popup; vcod hands that job to the console.
const FAVORITE_MENU: &str = "createfavorite_popmenu";

/// `ownerdraw` ids from `ui_mp/menudef.h`.
const UI_NETSOURCE: i32 = 220;
const UI_SERVERREFRESHDATE: i32 = 247;
const UI_JOINGAMETYPE: i32 = 253;
/// `ui_mp/menudef.h`'s `UI_SHOW_FAVORITESERVERS`: shown only while the
/// source is Favorites, which vcod's browser never is.
const UI_SHOW_FAVORITESERVERS: i32 = 0x4;

/// Q3's `SCROLLBAR_SIZE`, which a list row's highlight stops short of.
const SCROLLBAR_SIZE: f32 = 16.0;
const DOUBLE_CLICK: Duration = Duration::from_millis(400);

/// What a menu script asks of the app.
#[derive(Debug, Clone, PartialEq)]
pub enum UiEffect {
    /// A console command line (`connect`, `disconnect`, `quit`, `set`).
    Command(String),
    /// A sound alias played on the viewer.
    Sound(String),
    /// Open the console with this text in its field.
    ConsoleInput(String),
    /// `execOnCvarIntValue` / `execOnCvarFloatValue`: run `command` when
    /// `cvar` holds `value` (compared as integers when `int`). Read when the
    /// effect runs, after the commands queued before it.
    ExecOnCvar {
        cvar: String,
        value: f32,
        int: bool,
        command: String,
    },
}

impl UiEffect {
    /// Whether an `ExecOnCvar`'s test passes on `current`.
    pub fn cvar_matches(current: &str, value: f32, int: bool) -> bool {
        let cur = current.trim().parse::<f32>().unwrap_or(0.0);
        if int {
            cur as i32 == value as i32
        } else {
            cur == value
        }
    }
}

/// An item's runtime state: `show`/`hide` and `setitemcolor` change it.
#[derive(Clone)]
struct ItemState {
    visible: bool,
    back: [f32; 4],
    border_color: [f32; 4],
}

pub struct Ui {
    menus: Vec<UiMenu>,
    state: Vec<Vec<ItemState>>,
    /// Open menus, drawn in order; the last takes input.
    open: Vec<usize>,
    fonts: Option<UiFonts>,
    /// Mouse position on the 640x480 grid.
    cursor: [f32; 2],
    /// Item under the mouse, as (menu, item): the top menu's, or one of an
    /// open menu below it that no popup covers.
    hover: Option<(usize, usize)>,
    pub browser: Browser,
    selected: Option<SocketAddrV4>,
    /// First list row drawn.
    list_top: usize,
    last_click: Option<(Instant, SocketAddrV4)>,
    /// `com_errorMessage`, which `error_popmenu` shows.
    error: String,
    options: options::State,
}

impl Ui {
    pub fn new(fs: &Pk3Fs) -> Ui {
        let read = |p: &str| fs.read(p).map(|b| String::from_utf8_lossy(&b).into_owned());
        let mut menus = Vec::new();
        for path in MENU_FILES.iter().chain(&options::MENU_FILES) {
            match read(path) {
                Some(text) => menus.extend(ui_menu::parse_file(&text, &read)),
                None => log::warn!("ui: no {path}"),
            }
        }
        let state = menus
            .iter()
            .map(|m| {
                m.items
                    .iter()
                    .map(|i| ItemState {
                        visible: i.visible,
                        back: i.back,
                        border_color: i.border_color,
                    })
                    .collect()
            })
            .collect();
        let fonts = UiFonts::load(fs)
            .map_err(|e| log::warn!("ui: {e}, drawing no text"))
            .ok();
        Ui {
            menus,
            state,
            open: Vec::new(),
            fonts,
            cursor: [0.0; 2],
            hover: None,
            browser: Browser::default(),
            selected: None,
            list_top: 0,
            last_click: None,
            error: String::new(),
            options: options::State::default(),
        }
    }

    /// Whether a menu is up and taking input.
    pub fn active(&self) -> bool {
        !self.open.is_empty()
    }

    /// The main menu, as retail shows it with no game up.
    pub fn open_main(&mut self, out: &mut Vec<UiEffect>) {
        self.open_menu("main", out);
    }

    /// Everything closes when a game starts.
    pub fn close_all(&mut self) {
        self.open.clear();
        self.hover = None;
        self.options.clear();
        self.browser.stop();
    }

    /// A drop or a failed connect: the main menu with the error popup over
    /// it, as `Com_Error` leaves retail.
    pub fn show_error(&mut self, message: &str, out: &mut Vec<UiEffect>) {
        self.error = message.to_string();
        self.open_menu("main", out);
        self.open_menu("error_popmenu", out);
    }

    pub fn frame(&mut self, now: Instant) {
        self.browser.poll(now);
    }

    fn find(&self, name: &str) -> Option<usize> {
        self.menus
            .iter()
            .position(|m| m.name.eq_ignore_ascii_case(name))
    }

    fn open_menu(&mut self, name: &str, out: &mut Vec<UiEffect>) {
        if name.eq_ignore_ascii_case(FAVORITE_MENU) {
            out.push(UiEffect::ConsoleInput("connect ".into()));
            return;
        }
        let Some(m) = self.find(name).filter(|_| supported(name)) else {
            crate::console::log::print(&format!("The {name} menu is not in vcod yet."));
            return;
        };
        self.open.retain(|&o| o != m);
        self.open.push(m);
        self.hover = None;
        let script = self.menus[m].on_open.clone();
        self.run(m, &script, out);
    }

    fn close_menu(&mut self, name: &str, out: &mut Vec<UiEffect>) {
        let Some(m) = self.find(name) else { return };
        if !self.open.contains(&m) {
            return;
        }
        self.open.retain(|&o| o != m);
        self.hover = None;
        self.options.forget(m);
        let script = self.menus[m].on_close.clone();
        self.run(m, &script, out);
    }

    /// `Item_RunScript` over `script`, with `menu` as the item's parent.
    fn run(&mut self, menu: usize, script: &[String], out: &mut Vec<UiEffect>) {
        for cmd in ui_menu::script_commands(script) {
            let arg = |i: usize| cmd.get(i).map(String::as_str).unwrap_or("");
            match cmd[0].to_ascii_lowercase().as_str() {
                "play" => out.push(UiEffect::Sound(arg(1).to_string())),
                "open" => self.open_menu(arg(1), out),
                "close" => self.close_menu(arg(1), out),
                "show" | "hide" => {
                    let visible = cmd[0].eq_ignore_ascii_case("show");
                    for (item, st) in self.menus[menu].items.iter().zip(&mut self.state[menu]) {
                        if item.name.eq_ignore_ascii_case(arg(1))
                            || item.group.eq_ignore_ascii_case(arg(1))
                        {
                            st.visible = visible;
                        }
                    }
                }
                "setitemcolor" => {
                    let c: [f32; 4] = std::array::from_fn(|k| arg(3 + k).parse().unwrap_or(0.0));
                    for (item, st) in self.menus[menu].items.iter().zip(&mut self.state[menu]) {
                        if item.name.eq_ignore_ascii_case(arg(1))
                            || item.group.eq_ignore_ascii_case(arg(1))
                        {
                            match arg(2).to_ascii_lowercase().as_str() {
                                "backcolor" => st.back = c,
                                "bordercolor" => st.border_color = c,
                                _ => {}
                            }
                        }
                    }
                }
                "exec" => out.push(UiEffect::Command(arg(1).to_string())),
                "setcvar" => self.set_cvar(arg(1), arg(2), out),
                "uiscript" => self.ui_script(&cmd[1..], out),
                // `execOnCvarIntValue <cvar> <n> <command>` and its float
                // twin: the command runs when the cvar holds that value.
                "execoncvarintvalue" | "execoncvarfloatvalue" => out.push(UiEffect::ExecOnCvar {
                    cvar: arg(1).to_string(),
                    value: arg(2).trim().parse().unwrap_or(0.0),
                    int: cmd[0].eq_ignore_ascii_case("execoncvarintvalue"),
                    command: arg(3).to_string(),
                }),
                // Fades are cosmetic; vcod shows the item as it is.
                "fadein" | "fadeout" => {}
                // `ingameclose` acts only in a game, where vcod shows no menu.
                _ => {}
            }
        }
    }

    /// `UI_RunMenuScript`, for the scripts the stock browser and main menu
    /// call.
    fn ui_script(&mut self, args: &[String], out: &mut Vec<UiEffect>) {
        let Some(name) = args.first() else { return };
        match name.to_ascii_lowercase().as_str() {
            "refreshservers" => {
                self.browser.refresh();
                self.list_top = 0;
            }
            "refreshfilter" => self.browser.requery(),
            "stoprefresh" | "closejoin" => self.browser.stop(),
            "serversort" => {
                self.browser.sort = args.get(1).and_then(|c| c.parse().ok());
            }
            "joinserver" => {
                if let Some(addr) = self.selected {
                    out.push(UiEffect::Command(format!("connect {addr}")));
                }
            }
            "quit" => out.push(UiEffect::Command("quit".into())),
            "clearerror" => self.error.clear(),
            // vcod reads binds straight from the console, so there is
            // nothing to load; one language is all vcod reads.
            "loadcontrols" | "getlanguage" | "verifylanguage" => {}
            // `update ui_mousePitch` (0x4000a377): m_pitch 0.022, negated
            // when the toggle is on.
            "update"
                if args
                    .get(1)
                    .is_some_and(|a| a.eq_ignore_ascii_case("ui_mousePitch")) =>
            {
                if let Some(v) = self.options.pending("ui_mousePitch") {
                    let on = v.trim().parse::<f32>().unwrap_or(0.0) != 0.0;
                    let pitch = if on { "-0.022" } else { "0.022" };
                    self.set_cvar("m_pitch", pitch, out);
                }
            }
            // Filters and the rate picker's refresh have nothing to update.
            "updatefilter" | "update" => {}
            other => log::debug!("ui: no UI script {other}"),
        }
    }

    /// The first menu `script` opens that vcod cannot show. A button whose
    /// script would close its own menu first and then open such a menu is
    /// refused whole, so the screen does not go blank.
    fn refused(&self, script: &[String]) -> Option<String> {
        ui_menu::script_commands(script)
            .into_iter()
            .filter(|c| c.len() > 1 && c[0].eq_ignore_ascii_case("open"))
            .map(|c| c[1].clone())
            .find(|name| !name.eq_ignore_ascii_case(FAVORITE_MENU) && !supported(name))
    }

    /// Whether `item` of `menu` is drawn and can take the mouse.
    fn shown(&self, menu: usize, i: usize, shell: &Shell) -> bool {
        let item = &self.menus[menu].items[i];
        self.state[menu][i].visible
            && item.ownerdraw_flag & UI_SHOW_FAVORITESERVERS == 0
            && item.gate.as_ref().is_none_or(|g| {
                let v = self.cvar(&g.cvar, shell);
                g.passes(v.as_deref())
            })
    }

    /// A mouse move in window px on a `w` x `h` window. Runs the
    /// `mouseExit` and `mouseEnter` scripts as the hovered item changes.
    pub fn mouse_move(&mut self, x: f32, y: f32, w: f32, h: f32, shell: &Shell) -> Vec<UiEffect> {
        let mut out = Vec::new();
        self.options.new_input();
        self.cursor = [x * 640.0 / w, y * 480.0 / h];
        self.drag_to(shell, &mut out);
        let hit = self.hit(shell);
        if hit != self.hover {
            if let Some((m, old)) = self.hover {
                let s = self.menus[m].items[old].mouse_exit.clone();
                self.run(m, &s, &mut out);
            }
            self.hover = hit;
            if let Some((m, new)) = hit {
                let s = self.menus[m].items[new].mouse_enter.clone();
                self.run(m, &s, &mut out);
            }
        }
        out
    }

    /// The item under the cursor: the top menu's first, then those of the
    /// menus under it down to the first popup, as `Menus_HandleOOBClick`
    /// passes a click outside the focused menu to the one it lands on.
    fn hit(&self, shell: &Shell) -> Option<(usize, usize)> {
        for &m in self.open.iter().rev() {
            let found = (0..self.menus[m].items.len()).find(|&i| {
                let item = &self.menus[m].items[i];
                !item.decoration && self.shown(m, i, shell) && contains(item.rect, self.cursor)
            });
            if let Some(i) = found {
                return Some((m, i));
            }
            if self.menus[m].popup {
                break;
            }
        }
        None
    }

    /// A left click at the last mouse position.
    pub fn click(&mut self, now: Instant, shell: &Shell) -> Vec<UiEffect> {
        let mut out = Vec::new();
        self.options.new_input();
        // A click ends an edit, then acts as a click.
        self.options.editing = None;
        let Some((top, i)) = self.hover else {
            return out;
        };
        let item = self.menus[top].items[i].clone();
        if item.kind == ITEM_TYPE_LISTBOX && item.feeder == Some(FEEDER_SERVERS) {
            let row = ((self.cursor[1] - item.rect[1] - 1.0) / item.element_height.max(1.0)).floor()
                as usize
                + self.list_top;
            let rows = self.browser.rows();
            let Some(addr) = rows.get(row).map(|s| s.addr) else {
                return out;
            };
            let double = self
                .last_click
                .is_some_and(|(t, a)| a == addr && now - t < DOUBLE_CLICK);
            self.selected = Some(addr);
            self.last_click = Some((now, addr));
            if double {
                self.last_click = None;
                self.run(top, &item.double_click, &mut out);
            }
            return out;
        }
        if let Some(name) = self.refused(&item.action) {
            crate::console::log::print(&format!("The {name} menu is not in vcod yet."));
            return out;
        }
        if self.option_click(top, i, shell, &mut out) {
            self.run(top, &item.action, &mut out);
        }
        out
    }

    /// Keys while a menu is up: Esc runs the top menu's `onEsc`; the arrows,
    /// Page Up/Down and Enter work the server list. False when the key is
    /// not the menu's.
    pub fn key(&mut self, code: KeyCode) -> (bool, Vec<UiEffect>) {
        let mut out = Vec::new();
        self.options.new_input();
        let Some(&top) = self.open.last() else {
            return (false, out);
        };
        let has_list = self.menus[top]
            .items
            .iter()
            .any(|i| i.feeder == Some(FEEDER_SERVERS));
        match code {
            KeyCode::Escape => {
                let s = self.menus[top].on_esc.clone();
                self.run(top, &s, &mut out);
            }
            KeyCode::ArrowUp | KeyCode::ArrowDown if has_list => {
                let rows: Vec<SocketAddrV4> = self.browser.rows().iter().map(|s| s.addr).collect();
                let at = self
                    .selected
                    .and_then(|a| rows.iter().position(|&r| r == a));
                let next = match (code, at) {
                    (_, None) => 0,
                    (KeyCode::ArrowUp, Some(p)) => p.saturating_sub(1),
                    (_, Some(p)) => (p + 1).min(rows.len().saturating_sub(1)),
                };
                self.selected = rows.get(next).copied();
                self.keep_visible(next, top);
            }
            KeyCode::Enter | KeyCode::NumpadEnter if self.enter_on_bind() => {}
            KeyCode::PageUp if has_list => self.scroll(true),
            KeyCode::PageDown if has_list => self.scroll(false),
            KeyCode::Enter | KeyCode::NumpadEnter if has_list && self.selected.is_some() => {
                self.ui_script(&["JoinServer".to_string()], &mut out);
            }
            _ => return (false, out),
        }
        (true, out)
    }

    fn list_rows_visible(&self, top: usize) -> usize {
        self.menus[top]
            .items
            .iter()
            .find(|i| i.feeder == Some(FEEDER_SERVERS))
            .map_or(1, visible_rows)
    }

    fn keep_visible(&mut self, row: usize, top: usize) {
        let n = self.list_rows_visible(top);
        if row < self.list_top {
            self.list_top = row;
        } else if row >= self.list_top + n {
            self.list_top = row + 1 - n;
        }
    }

    /// The mouse wheel over the list.
    pub fn scroll(&mut self, up: bool) {
        let Some(&top) = self.open.last() else { return };
        let n = self.list_rows_visible(top);
        let len = self.browser.rows().len();
        self.list_top = if up {
            self.list_top.saturating_sub(n / 2)
        } else {
            (self.list_top + n / 2).min(len.saturating_sub(n))
        };
    }

    /// Every open menu as HUD quads for a `w` x `h` window.
    pub fn build(&self, w: f32, h: f32, loc: &Localized, shell: &Shell) -> Vec<HudQuad> {
        let mut out = Vec::new();
        let p = Painter {
            sx: w / 640.0,
            sy: h / 480.0,
            fonts: self.fonts.as_ref(),
        };
        for &m in &self.open {
            let menu = &self.menus[m];
            for (i, item) in menu.items.iter().enumerate() {
                if !self.shown(m, i, shell) {
                    continue;
                }
                let st = &self.state[m][i];
                p.window(item, st, &mut out);
                let focused = self.hover == Some((m, i));
                let color = if focused && !item.decoration && item.kind != ITEM_TYPE_LISTBOX {
                    menu.focus_color
                } else {
                    item.fore
                };
                if item.kind == ITEM_TYPE_LISTBOX && item.feeder == Some(FEEDER_SERVERS) {
                    self.server_list(&p, item, &mut out);
                    continue;
                }
                let label = if !item.text.is_empty() {
                    loc.translate(&item.text).into_owned()
                } else if let Some(cvar) = &item.cvar
                    && item.kind != ITEM_TYPE_MULTI
                {
                    self.cvar(cvar, shell).unwrap_or_default()
                } else {
                    String::new()
                };
                let value = match item.ownerdraw {
                    UI_NETSOURCE => Some(fill(
                        &loc.translate("@EXE_NETSOURCE"),
                        &[&loc.translate("@EXE_INTERNET")],
                    )),
                    UI_SERVERREFRESHDATE => Some(self.refresh_text(loc)),
                    UI_JOINGAMETYPE => Some(loc.translate("@EXE_ALL").into_owned()),
                    _ => self.option_value(m, i, loc, shell),
                };
                // `Item_SetTextExtents`: the label at `textalignx`, moved
                // left by its width (right) or half of it (centre); an owner
                // draw or a multi's value follows 8 units after it.
                let x = item.rect[0] + item.text_align_x;
                let y = item.rect[1] + item.text_align_y;
                let label_w = p.width(&label, item.text_scale);
                let value_w = value
                    .as_ref()
                    .filter(|_| item.kind == ITEM_TYPE_OWNERDRAW)
                    .map_or(0.0, |v| p.width(v, item.text_scale));
                // An owner draw with no label draws at `textalignx` whatever
                // its alignment (`UI_OwnerDraw`'s `rect.x + text_x`).
                let left = match item.text_align {
                    _ if label.is_empty() => x,
                    ITEM_ALIGN_RIGHT => x - label_w - value_w,
                    ITEM_ALIGN_CENTER => x - (label_w + value_w) / 2.0,
                    _ => x,
                };
                if item.autowrap {
                    p.wrapped(&label, item, color, &mut out);
                    continue;
                }
                if !label.is_empty() {
                    p.text(&label, left, y, item.text_scale, color, &mut out);
                }
                if item.kind == ITEM_TYPE_SLIDER {
                    self.slider_quads(&p, (m, i), item, &label, color, shell, &mut out);
                }
                if let Some(v) = value {
                    let gap = if label.is_empty() { 0.0 } else { 8.0 };
                    p.text(
                        &v,
                        left + label_w + gap,
                        y,
                        item.text_scale,
                        color,
                        &mut out,
                    );
                }
            }
        }
        out
    }

    /// `UI_SERVERREFRESHDATE`'s line: the refresh's progress, then the
    /// count when it is done.
    fn refresh_text(&self, loc: &Localized) -> String {
        let (total, answered) = self.browser.counts();
        match self.browser.status {
            Status::Idle => String::new(),
            Status::Master => loc
                .translate("@EXE_WAITINGFORMASTERSERVERRESPONSE")
                .into_owned(),
            Status::Pinging => fill(
                &loc.translate("@EXE_GETTINGINFOFORSERVERS"),
                &[&(total - answered).to_string()],
            ),
            Status::Done => {
                let players: u32 = self
                    .browser
                    .rows()
                    .iter()
                    .filter_map(|s| s.info.as_ref())
                    .map(|i| i.clients)
                    .sum();
                format!("{answered} servers listed in browser with {players} players.")
            }
            Status::Failed => "No response from the master server.".into(),
        }
    }

    /// `Item_ListBox_Paint` for `FEEDER_SERVERS`.
    fn server_list(&self, p: &Painter, item: &UiItem, out: &mut Vec<HudQuad>) {
        let rows = self.browser.rows();
        let [x, y0, w, _] = item.rect;
        let (x, mut y) = (x + 1.0, y0 + 1.0);
        let eh = item.element_height.max(1.0);
        for s in rows.iter().skip(self.list_top).take(visible_rows(item)) {
            for (c, col) in item.columns.iter().enumerate() {
                let text = truncate_visible(&browser::column_text(s, c), col.max_chars);
                p.text(
                    &text,
                    x + 4.0 + col.pos + item.text_align_x,
                    y + eh + item.text_align_y,
                    item.text_scale,
                    item.fore,
                    out,
                );
            }
            if self.selected == Some(s.addr) {
                p.fill(
                    [x, y, w - SCROLLBAR_SIZE - 4.0, eh - 1.0],
                    item.outline_color,
                    out,
                );
            }
            y += eh;
        }
    }
}

impl Ui {
    /// The cvars a menu reads: `cl_ingame` is 0 (vcod's front end only runs
    /// with no game up), `shortversion` is vcod's, `com_errorMessage` the
    /// last error, `ui_multiplayer` 1 (this is the MP UI),
    /// `cl_languagesavailable` 1 (vcod reads English only), `ui_mousePitch`
    /// on when `m_pitch` is negative (as the UI sets it at load), anything
    /// this input set, then the console's.
    fn cvar(&self, name: &str, shell: &Shell) -> Option<String> {
        if let Some(v) = self.options.pending(name) {
            return Some(v.to_string());
        }
        match name.to_ascii_lowercase().as_str() {
            "cl_ingame" => Some("0".into()),
            "com_errormessage" => Some(self.error.clone()),
            "shortversion" => Some(concat!("vcod ", env!("CARGO_PKG_VERSION")).into()),
            "ui_multiplayer" | "cl_languagesavailable" => Some("1".into()),
            "ui_mousepitch" => Some(
                if shell.cvar_f32("m_pitch") < 0.0 {
                    "1"
                } else {
                    "0"
                }
                .into(),
            ),
            _ => shell.cvar(name).map(str::to_string),
        }
    }
}

/// Rows a list box shows whole: Q3 stops at the last that fits.
fn visible_rows(item: &UiItem) -> usize {
    ((item.rect[3] - 2.0) / item.element_height.max(1.0))
        .floor()
        .max(1.0) as usize
}

fn contains(r: [f32; 4], p: [f32; 2]) -> bool {
    p[0] >= r[0] && p[0] <= r[0] + r[2] && p[1] >= r[1] && p[1] <= r[1] + r[3]
}

/// `%s` and `%d` in a localized format filled in order.
fn fill(fmt: &str, args: &[&str]) -> String {
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
fn truncate_visible(text: &str, n: usize) -> String {
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

/// Draws on the 640x480 grid stretched over the window, as the UI module
/// does.
struct Painter<'a> {
    sx: f32,
    sy: f32,
    fonts: Option<&'a UiFonts>,
}

impl Painter<'_> {
    fn quad(&self, r: [f32; 4], rgba: [f32; 4], texture: &str, out: &mut Vec<HudQuad>) {
        let (x, y, w, h) = (
            r[0] * self.sx,
            r[1] * self.sy,
            r[2] * self.sx,
            r[3] * self.sy,
        );
        out.push(HudQuad {
            verts: [[x, y], [x + w, y], [x + w, y + h], [x, y + h]],
            uvs: [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
            rgba,
            texture: texture.to_string(),
        });
    }

    fn fill(&self, r: [f32; 4], rgba: [f32; 4], out: &mut Vec<HudQuad>) {
        if rgba[3] > 0.0 {
            self.quad(r, rgba, "white", out);
        }
    }

    /// `Window_Paint`: the fill or shader inside the border, then a full
    /// border of `border_size`.
    fn window(&self, item: &UiItem, st: &ItemState, out: &mut Vec<HudQuad>) {
        let mut r = item.rect;
        if item.border != 0 {
            let b = item.border_size;
            r = [r[0] + b, r[1] + b, r[2] - (b + 1.0), r[3] - (b + 1.0)];
        }
        match (item.style, &item.background) {
            (WINDOW_STYLE_FILLED, Some(bg)) => self.quad(r, st.back, bg, out),
            (WINDOW_STYLE_FILLED, None) => self.fill(r, st.back, out),
            (WINDOW_STYLE_SHADER, Some(bg)) => self.quad(r, item.fore, bg, out),
            _ => {}
        }
        if item.border == 1 {
            let [x, y, w, h] = item.rect;
            let b = item.border_size;
            let c = st.border_color;
            self.fill([x, y, w, b], c, out);
            self.fill([x, y + h - b, w, b], c, out);
            self.fill([x, y + b, b, h - 2.0 * b], c, out);
            self.fill([x + w - b, y + b, b, h - 2.0 * b], c, out);
        }
    }

    /// `Item_Text_AutoWrapped_Paint`: words packed into lines narrower
    /// than the rect, each aligned on `textalignx`, `textaligny` down and
    /// the text height plus 5 apart.
    fn wrapped(&self, text: &str, item: &UiItem, color: [f32; 4], out: &mut Vec<HudQuad>) {
        let Some(f) = self.font(item.text_scale) else {
            return;
        };
        let height = f.max_height as f32 * f.glyph_scale * item.text_scale;
        let mut lines: Vec<String> = Vec::new();
        for word in text.split_whitespace() {
            match lines.last_mut() {
                Some(line)
                    if self.width(&format!("{line} {word}"), item.text_scale) < item.rect[2] =>
                {
                    line.push(' ');
                    line.push_str(word);
                }
                _ => lines.push(word.to_string()),
            }
        }
        for (n, line) in lines.iter().enumerate() {
            let w = self.width(line, item.text_scale);
            let x = item.rect[0]
                + match item.text_align {
                    ITEM_ALIGN_RIGHT => item.text_align_x - w,
                    ITEM_ALIGN_CENTER => item.text_align_x - w / 2.0,
                    _ => item.text_align_x,
                };
            let y = item.rect[1] + item.text_align_y + n as f32 * (height + 5.0);
            self.text(line, x, y, item.text_scale, color, out);
        }
    }

    /// The font the UI picks for `scale` on this window.
    fn font(&self, scale: f32) -> Option<&font::Font> {
        self.fonts
            .map(|f| f.pick(Slot::Default, scale, 480.0 * self.sy))
    }

    /// Width on the 640 grid.
    fn width(&self, text: &str, scale: f32) -> f32 {
        self.font(scale)
            .map_or(0.0, |f| font::measure(f, text, scale / f.unit_scale()))
    }

    /// `text` with its baseline at grid `(x, y)`.
    fn text(
        &self,
        text: &str,
        x: f32,
        y: f32,
        scale: f32,
        color: [f32; 4],
        out: &mut Vec<HudQuad>,
    ) {
        let Some(f) = self.font(scale) else { return };
        let top = y - f.max_height as f32 * f.glyph_scale * scale;
        font::layout(
            f,
            text,
            x * self.sx,
            top * self.sy,
            scale * self.sy / f.unit_scale(),
            color,
            out,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ui() -> Option<Ui> {
        let fs = vcod_common::testing::game_fs()?;
        Some(Ui::new(&fs))
    }

    fn click_item(ui: &mut Ui, menu: &str, text: &str, shell: &Shell) -> Vec<UiEffect> {
        let m = ui.find(menu).unwrap();
        let i = ui.menus[m]
            .items
            .iter()
            .position(|i| i.text == text)
            .unwrap();
        let r = ui.menus[m].items[i].rect;
        ui.mouse_move(r[0] + 2.0, r[1] + 2.0, 640.0, 480.0, shell);
        assert_eq!(ui.hover, Some((m, i)), "{text}");
        ui.click(Instant::now(), shell)
    }

    #[test]
    fn join_a_game_opens_the_browser_and_quit_asks_first() {
        let Some(mut ui) = ui() else { return };
        let shell = Shell::new();
        let mut out = Vec::new();
        ui.open_main(&mut out);
        assert!(out.contains(&UiEffect::Sound("music_mainmenu".into())));
        let out = click_item(&mut ui, "main", "@MENU_JOIN_GAME", &shell);
        assert_eq!(out, vec![UiEffect::Sound("mouse_click".into())]);
        let top = *ui.open.last().unwrap();
        assert_eq!(ui.menus[top].name, "joinserver");
        assert_eq!(ui.open.len(), 1, "main closed behind it");

        // Esc goes back to main.
        let (used, _) = ui.key(KeyCode::Escape);
        assert!(used);
        assert_eq!(ui.menus[*ui.open.last().unwrap()].name, "main");

        click_item(&mut ui, "main", "@MENU_QUIT", &shell);
        assert_eq!(ui.menus[*ui.open.last().unwrap()].name, "quit_popmenu");
        let out = click_item(&mut ui, "quit_popmenu", "@MENU_YES", &shell);
        assert!(out.contains(&UiEffect::Command("quit".into())));
    }

    #[test]
    fn a_drop_shows_its_reason_over_the_main_menu() {
        let Some(mut ui) = ui() else { return };
        let shell = Shell::new();
        ui.show_error("Server timed out", &mut Vec::new());
        let names: Vec<_> = ui.open.iter().map(|&m| ui.menus[m].name.as_str()).collect();
        assert_eq!(names, ["main", "error_popmenu"]);
        assert_eq!(
            ui.cvar("com_errorMessage", &shell).as_deref(),
            Some("Server timed out")
        );
        let (_, _) = ui.key(KeyCode::Escape);
        assert_eq!(ui.menus[*ui.open.last().unwrap()].name, "main");
    }

    #[test]
    fn unsupported_menus_stay_shut() {
        let Some(mut ui) = ui() else { return };
        let shell = Shell::new();
        let mut out = Vec::new();
        ui.open_main(&mut out);
        click_item(&mut ui, "main", "@MENU_START_NEW_SERVER", &shell);
        assert_eq!(ui.open.len(), 1);
        assert_eq!(ui.menus[ui.open[0]].name, "main");
    }

    #[test]
    fn the_main_menu_draws_its_backdrop_and_buttons() {
        let Some(mut ui) = ui() else { return };
        let shell = Shell::new();
        ui.open_main(&mut Vec::new());
        let quads = ui.build(1280.0, 960.0, &Localized::default(), &shell);
        assert_eq!(quads[0].texture, "ui_mp/assets/main_back_top_mp.tga");
        assert_eq!(quads[0].verts[2], [1280.0, 640.0]);
        assert!(quads.len() > 50, "button text drawn: {}", quads.len());
    }

    #[test]
    fn format_and_truncation() {
        assert_eq!(
            fill("Source:     %s", &["Internet"]),
            "Source:     Internet"
        );
        assert_eq!(fill("100%", &[]), "100%");
        assert_eq!(truncate_visible("^1abc^7def", 4), "^1abc^7d");
        assert_eq!(truncate_visible("abc", 0), "abc");
    }
}
