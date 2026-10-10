//! Contains routines ported from the RTCW-MP GPL source, Copyright (C) 1999-2010 id Software LLC, a ZeniMax Media company.
//! See NOTICE.
//!
//! The front end: the stock main menu, server browser and its popups
//! (password, server info, filter, favourites), options screens, quit and
//! error popups drawn from their `.menu` files and driven by mouse and keys,
//! as the UI module does while no game is up, and the main menu again over a
//! game with `cl_ingame` 1 (docs/research/cod11-front-end.md).
//! The Mods menu lists the mod directories and switches `fs_game`, which
//! rebuilds the UI off the new search path. Menus vcod cannot run yet
//! (create server) are refused with a console line instead of drawing
//! screens whose controls do nothing.

pub mod browser;
mod options;
mod status;

use std::collections::HashMap;
use std::net::SocketAddrV4;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use vcod_common::localize::Localized;
use vcod_common::pk3::Pk3Fs;
use vcod_common::ui_menu::{
    self, FEEDER_MODS, FEEDER_SERVERS, FEEDER_SERVERSTATUS, ITEM_ALIGN_CENTER, ITEM_ALIGN_RIGHT,
    ITEM_TYPE_LISTBOX, ITEM_TYPE_MULTI, ITEM_TYPE_OWNERDRAW, ITEM_TYPE_SLIDER, UiItem, UiMenu,
    WINDOW_STYLE_FILLED, WINDOW_STYLE_SHADER,
};
use winit::keyboard::KeyCode;

use crate::console::shell::Shell;
use crate::hud::HudQuad;
use crate::hud::font::{self, Slot, UiFonts};
use browser::{AddFavorite, Browser, Filter, Source, Status};
use status::StatusQuery;

/// `ui_menuFiles`' default (`ui_mp_x86.dll` 0x40036c9c): the menu list the
/// UI loads, which a mod replaces by shipping its own.
const MENU_LIST: &str = "ui_mp/menus.txt";
/// The second list `_UI_Init` loads, after [`MENU_LIST`] and without
/// clearing it (0x4000d64c). Of what stock names only
/// `ui_mp/wm_quickmessage.menu` ships.
const INGAME_LIST: &str = "ui_mp/ingame.txt";

/// `UI_LoadMenus` (`ui_mp_x86.dll` 0x400085b0): appends the menus the list
/// at `list` names, in order. A missing list falls back to [`MENU_LIST`]
/// with retail's warning. Each file is tried under `cl_language`'s
/// directory first ([`ui_menu::localized_menu_path`]).
fn load_menus(fs: &Pk3Fs, list: &str, language: &str, menus: &mut Vec<UiMenu>) {
    let read = |p: &str| fs.read(p).map(|b| String::from_utf8_lossy(&b).into_owned());
    let text = read(list).or_else(|| {
        crate::console::log::print(&format!("^3menu file not found: {list}, using default"));
        read(MENU_LIST)
    });
    let files = text.map_or_else(Vec::new, |t| ui_menu::menu_list(&t));
    if files.is_empty() {
        log::warn!("ui: no menus in {list}");
    }
    for path in &files {
        let text = ui_menu::localized_menu_path(path, language)
            .and_then(|p| read(&p))
            .or_else(|| read(path));
        match text {
            Some(text) => menus.extend(ui_menu::parse_file(&text, &read)),
            // Stock `menus.txt` names three files no pak ships, `ingame.txt` six.
            None => log::debug!("ui: menu file not found: {path}"),
        }
    }
}

/// Each item's starting state.
fn item_states(menus: &[UiMenu]) -> Vec<Vec<ItemState>> {
    menus
        .iter()
        .map(|m: &UiMenu| {
            m.items
                .iter()
                .map(|i| ItemState {
                    visible: i.visible,
                    back: i.back,
                    border_color: i.border_color,
                })
                .collect()
        })
        .collect()
}

/// The stock menus vcod cannot run yet; `open` refuses them with a console
/// line instead of drawing controls that do nothing. Any other menu the
/// list loads, a mod's included, opens.
const UNSUPPORTED: [&str; 18] = [
    "single_popmenu",
    "options_driverinfo",
    "options_credits",
    "language_restart_popmenu",
    "rec_restart_popmenu",
    "multi_menu",
    "createserver",
    "createserver_maps",
    "createserver_op",
    "cdkey_menu",
    "connect",
    "single_player_menu",
    "auconfirm",
    "settings_dm",
    "settings_tdm",
    "settings_sd",
    "settings_re",
    "settings_bel",
];

fn unsupported(name: &str) -> bool {
    UNSUPPORTED.iter().any(|s| s.eq_ignore_ascii_case(name))
}

/// `ownerdraw` ids from `ui_mp/menudef.h`.
const UI_NETSOURCE: i32 = 220;
const UI_SERVERREFRESHDATE: i32 = 247;
const UI_JOINGAMETYPE: i32 = 253;
/// `ui_mp/menudef.h`'s `UI_SHOW_FAVORITESERVERS` and
/// `UI_SHOW_NOTFAVORITESERVERS`: shown only while the source is, or is not,
/// Favorites (ui_mp_x86.dll 0x40009780).
const UI_SHOW_FAVORITESERVERS: i32 = 0x4;
const UI_SHOW_NOTFAVORITESERVERS: i32 = 0x1000;

/// The shell cvars the browser's scripts read, copied each frame.
const UI_CVARS: [&str; 7] = [
    "ui_netSource",
    "ui_browserShowFull",
    "ui_browserShowEmpty",
    "ui_browserShowPassword",
    "ui_browserShowNoPassword",
    "ui_favoriteName",
    "ui_favoriteAddress",
];

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
    /// `RunMod`'s `fs_game` and `vid_restart`: switch to this mod
    /// directory, or back to the base game with `None` (`Quake3`).
    RunMod(Option<String>),
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
    /// `cl_ingame`: the menus are up over a game, not the disconnected
    /// front end.
    in_game: bool,
    /// `ui_favorite_message`, which `fav_message_popmenu` shows.
    fav_message: String,
    /// The server info popup's `getstatus`.
    status: Option<StatusQuery>,
    /// [`UI_CVARS`] as the shell had them last frame (lower-case names).
    cvars: HashMap<String, String>,
    /// The install root and the base game directory the Mods menu lists
    /// beside; `None` lists nothing.
    mod_root: Option<(PathBuf, String)>,
    /// `LoadMods`' list and the selected row.
    mods: Vec<vcod_common::pk3::ModEntry>,
    mod_selected: Option<usize>,
    last_mod_click: Option<(Instant, usize)>,
}

impl Ui {
    /// `_UI_Init` sets `ui_menuFiles` back to its default before reading
    /// it (0x4000d324), so the UI always starts off [`MENU_LIST`], then
    /// adds [`INGAME_LIST`]'s. `language` is `cl_language`'s value.
    pub fn new(fs: &Pk3Fs, language: &str) -> Ui {
        let mut menus = Vec::new();
        load_menus(fs, MENU_LIST, language, &mut menus);
        load_menus(fs, INGAME_LIST, language, &mut menus);
        let state = item_states(&menus);
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
            in_game: false,
            fav_message: String::new(),
            status: None,
            cvars: HashMap::new(),
            mod_root: None,
            mods: Vec::new(),
            mod_selected: None,
            last_mod_click: None,
        }
    }

    /// The Mods menu lists the directories of `game_dir` beside `base`.
    pub fn with_mod_root(mut self, game_dir: PathBuf, base: String) -> Ui {
        self.mod_root = Some((game_dir, base));
        self
    }

    /// Favourites live in `path` (`servercache.dat`, beside `CoDMP.exe`).
    pub fn with_server_cache(mut self, path: PathBuf) -> Ui {
        self.browser = Browser::with_cache(path);
        self
    }

    /// Whether a menu is up and taking input.
    pub fn active(&self) -> bool {
        !self.open.is_empty()
    }

    /// The main menu, as retail shows it with no game up. While connecting
    /// or loading this is also what Esc opens (`UIMENU_MAIN`).
    pub fn open_main(&mut self, out: &mut Vec<UiEffect>) {
        self.in_game = false;
        self.open_menu("main", out);
    }

    /// The main menu over a game: `cl_ingame` 1 swaps Join a Game and Start
    /// New Server for Back to Game and Disconnect, and its Esc closes it.
    pub fn open_ingame(&mut self, out: &mut Vec<UiEffect>) {
        self.open.clear();
        self.in_game = true;
        self.open_menu("main", out);
    }

    /// `ui_load` (`UI_Load`, 0x400086c0): reload the menus off the list
    /// `ui_menuFiles` names, close them all and reopen the one that had
    /// focus. [`INGAME_LIST`]'s menus are gone until the next restart, as
    /// `UI_Load` resets the menus and loads the one list.
    pub fn reload(&mut self, fs: &Pk3Fs, list: &str, language: &str, out: &mut Vec<UiEffect>) {
        let focused = self.open.last().map(|&m| self.menus[m].name.clone());
        let list = if list.is_empty() { MENU_LIST } else { list };
        self.menus.clear();
        load_menus(fs, list, language, &mut self.menus);
        self.state = item_states(&self.menus);
        self.open.clear();
        self.hover = None;
        self.options.clear();
        if let Some(name) = focused {
            self.open_menu(&name, out);
        }
    }

    /// Everything closes when a game starts.
    pub fn close_all(&mut self) {
        self.open.clear();
        self.hover = None;
        self.options.clear();
        self.status = None;
        self.browser.stop();
        self.in_game = false;
    }

    /// A drop or a failed connect: the main menu with the error popup over
    /// it, as `Com_Error` leaves retail.
    pub fn show_error(&mut self, message: &str, out: &mut Vec<UiEffect>) {
        self.error = message.to_string();
        self.in_game = false;
        self.open_menu("main", out);
        self.open_menu("error_popmenu", out);
    }

    /// Once a frame while a menu is up: reads the browser's cvars and runs
    /// the network queries.
    pub fn frame(&mut self, now: Instant, shell: &Shell) {
        self.cvars = UI_CVARS
            .iter()
            .map(|n| {
                (
                    n.to_ascii_lowercase(),
                    shell.cvar(n).unwrap_or("").to_string(),
                )
            })
            .collect();
        let on = |n: &str| self.ui_cvar(n).trim().parse::<f32>().unwrap_or(0.0) != 0.0;
        self.browser.filter = Filter {
            show_full: on("ui_browserShowFull"),
            show_empty: on("ui_browserShowEmpty"),
            show_password: on("ui_browserShowPassword"),
            show_no_password: on("ui_browserShowNoPassword"),
        };
        // A console `set` or the config picked another source.
        let source = Source::from_cvar(&self.ui_cvar("ui_netSource"));
        if source != self.browser.source {
            self.browser.stop();
            self.browser.source = source;
            self.browser.status = Status::Idle;
            self.selected = None;
            self.list_top = 0;
        }
        self.browser.poll(now);
        if let Some(q) = &mut self.status {
            q.poll(now);
        }
    }

    /// A [`UI_CVARS`] value: this input's write, else last frame's.
    fn ui_cvar(&self, name: &str) -> String {
        if let Some(v) = self.options.pending(name) {
            return v.to_string();
        }
        self.cvars
            .get(&name.to_ascii_lowercase())
            .cloned()
            .unwrap_or_default()
    }

    fn find(&self, name: &str) -> Option<usize> {
        self.menus
            .iter()
            .position(|m| m.name.eq_ignore_ascii_case(name))
    }

    fn open_menu(&mut self, name: &str, out: &mut Vec<UiEffect>) {
        let Some(m) = self.find(name).filter(|_| !unsupported(name)) else {
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
                // `main`'s Esc and Back to Game; inert with no game up.
                "ingameclose" if self.in_game => self.close_menu(arg(1), out),
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
            // `UpdateFilter` refreshes a Local list on its own (ui_mp_x86.dll
            // 0x4000ac92); the filter itself applies at every draw.
            "updatefilter" => {
                if self.browser.source == Source::Local {
                    self.browser.refresh();
                }
            }
            "serverstatus" => {
                self.status = self
                    .selected
                    .map(|addr| StatusQuery::start(addr, Instant::now()));
            }
            "addfavorite" if self.browser.source != Source::Favorites => {
                let rows = self.browser.rows();
                let picked = self
                    .selected
                    .and_then(|a| rows.iter().find(|s| s.addr == a));
                let (name, addr) = picked.map_or((String::new(), String::new()), |s| {
                    let name = s
                        .info
                        .as_ref()
                        .map_or(String::new(), |i| i.hostname.clone());
                    (name, s.addr.to_string())
                });
                self.add_favorite(&name, &addr);
            }
            "createfavorite" if self.browser.source == Source::Favorites => {
                let name = self.ui_cvar("ui_favoriteName");
                let addr = self.ui_cvar("ui_favoriteAddress");
                self.add_favorite(&name, &addr);
            }
            "deletefavorite" if self.browser.source == Source::Favorites => {
                if let Some(addr) = self.selected.take() {
                    self.browser.remove_favorite(addr);
                }
            }
            "serversort" => {
                self.browser.sort = args.get(1).and_then(|c| c.parse().ok());
            }
            "joinserver" => {
                if let Some(addr) = self.selected {
                    out.push(UiEffect::Command(format!("connect {addr}")));
                }
            }
            "quit" => out.push(UiEffect::Command("quit".into())),
            // `UI_LoadMods` (ui_mp_x86.dll 0x40009e10), at most 64.
            "loadmods" => {
                self.mods = self
                    .mod_root
                    .as_ref()
                    .map(|(dir, base)| vcod_common::pk3::mod_list(dir, base))
                    .unwrap_or_default();
                self.mods.truncate(64);
                self.mod_selected = None;
            }
            "runmod" => {
                if let Some(m) = self.mod_selected.and_then(|i| self.mods.get(i)) {
                    out.push(UiEffect::RunMod(Some(m.dir.clone())));
                }
            }
            // The base game again; no stock menu calls it.
            "quake3" => out.push(UiEffect::RunMod(None)),
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
            // The rate picker's refresh has nothing to update.
            "update" => {}
            other => log::debug!("ui: no UI script {other}"),
        }
    }

    /// The favourite checks `addFavorite` and `createFavorite` share
    /// (ui_mp_x86.dll 0x4000a4a0), leaving the outcome in
    /// `ui_favorite_message`.
    fn add_favorite(&mut self, name: &str, addr: &str) {
        let key = if name.is_empty() {
            "@EXE_FAVORITENAMEEMPTY"
        } else if addr.is_empty() {
            "@EXE_FAVORITEADDRESSEMPTY"
        } else {
            match self.browser.add_favorite(name, addr) {
                AddFavorite::InList => "@EXE_FAVORITEINLIST",
                AddFavorite::Full => "@EXE_FAVORITELISTFULL",
                AddFavorite::BadAddress => "@EXE_BADSERVERADDRESS",
                AddFavorite::Added => "@EXE_FAVORITEADDED",
            }
        };
        self.fav_message = key.to_string();
    }

    /// The first menu `script` opens that vcod cannot show. A button whose
    /// script would close its own menu first and then open such a menu is
    /// refused whole, so the screen does not go blank.
    fn refused(&self, script: &[String]) -> Option<String> {
        ui_menu::script_commands(script)
            .into_iter()
            .filter(|c| c.len() > 1 && c[0].eq_ignore_ascii_case("open"))
            .map(|c| c[1].clone())
            .find(|name| unsupported(name) || self.find(name).is_none())
    }

    /// Whether `item` of `menu` is drawn and can take the mouse.
    fn shown(&self, menu: usize, i: usize, shell: &Shell) -> bool {
        let item = &self.menus[menu].items[i];
        let favorites = self.browser.source == Source::Favorites;
        self.state[menu][i].visible
            && (item.ownerdraw_flag & UI_SHOW_FAVORITESERVERS == 0 || favorites)
            && (item.ownerdraw_flag & UI_SHOW_NOTFAVORITESERVERS == 0 || !favorites)
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
        if item.kind == ITEM_TYPE_LISTBOX && item.feeder == Some(FEEDER_MODS) {
            let row = ((self.cursor[1] - item.rect[1] - 1.0) / item.element_height.max(1.0)).floor()
                as usize;
            if row < self.mods.len() {
                let double = self
                    .last_mod_click
                    .is_some_and(|(t, r)| r == row && now - t < DOUBLE_CLICK);
                self.mod_selected = Some(row);
                self.last_mod_click = Some((now, row));
                self.run(top, &item.action, &mut out);
                if double {
                    self.last_mod_click = None;
                    self.run(top, &item.double_click, &mut out);
                }
            }
            return out;
        }
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
        if item.ownerdraw == UI_NETSOURCE {
            // `UI_NetSource_HandleKey`, then the item's action.
            let next = self.browser.source.next();
            self.browser.set_source(next);
            self.selected = None;
            self.list_top = 0;
            self.set_cvar("ui_netSource", &(next as i32).to_string(), &mut out);
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
                if item.kind == ITEM_TYPE_LISTBOX && item.feeder == Some(FEEDER_SERVERSTATUS) {
                    self.status_list(&p, item, loc, &mut out);
                    continue;
                }
                if item.kind == ITEM_TYPE_LISTBOX && item.feeder == Some(FEEDER_MODS) {
                    self.mod_rows(&p, item, &mut out);
                    continue;
                }
                let label = if !item.text.is_empty() {
                    loc.translate(&item.text).into_owned()
                } else if let Some(cvar) = &item.cvar
                    && item.kind != ITEM_TYPE_MULTI
                {
                    // `ui_favorite_message` holds a localized key.
                    let v = self.cvar(cvar, shell).unwrap_or_default();
                    loc.translate(&v).into_owned()
                } else {
                    String::new()
                };
                let value = match item.ownerdraw {
                    UI_NETSOURCE => Some(fill(
                        &loc.translate("@EXE_NETSOURCE"),
                        &[&loc.translate(Source::NAMES[self.browser.source as usize])],
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

    /// `UI_SERVERREFRESHDATE`'s line: the refresh's progress with the
    /// source's list length (ui_mp_x86.dll 0x4000904d), then the count when
    /// it is done.
    fn refresh_text(&self, loc: &Localized) -> String {
        let (total, answered) = self.browser.counts();
        match self.browser.status {
            Status::Idle => String::new(),
            Status::Master => loc
                .translate("@EXE_WAITINGFORMASTERSERVERRESPONSE")
                .into_owned(),
            Status::Pinging => fill(
                &loc.translate("@EXE_GETTINGINFOFORSERVERS"),
                &[&total.to_string()],
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

    /// `Item_ListBox_Paint` for `FEEDER_SERVERSTATUS`: the server info
    /// popup's rows, not selectable; a cell holding a localized key prints
    /// its text.
    fn status_list(&self, p: &Painter, item: &UiItem, loc: &Localized, out: &mut Vec<HudQuad>) {
        let Some(q) = &self.status else { return };
        let [x, y0, _, _] = item.rect;
        let (x, mut y) = (x + 1.0, y0 + 1.0);
        let eh = item.element_height.max(1.0);
        for row in q.rows.iter().take(visible_rows(item)) {
            for (col, cell) in item.columns.iter().zip(row) {
                let text = truncate_visible(&loc.translate(cell), col.max_chars);
                p.text(
                    &text,
                    x + 4.0 + col.pos + item.text_align_x,
                    y + eh + item.text_align_y,
                    item.text_scale,
                    item.fore,
                    out,
                );
            }
            y += eh;
        }
    }

    /// `Item_ListBox_Paint` for `FEEDER_MODS`: each mod's description, or
    /// its directory when it has none (ui_mp_x86.dll 0x4000caa9).
    fn mod_rows(&self, p: &Painter, item: &UiItem, out: &mut Vec<HudQuad>) {
        let [x, y0, w, _] = item.rect;
        let (x, mut y) = (x + 1.0, y0 + 1.0);
        let eh = item.element_height.max(1.0);
        for (i, m) in self.mods.iter().enumerate().take(visible_rows(item)) {
            let text = if m.description.is_empty() {
                &m.dir
            } else {
                &m.description
            };
            p.text(
                text,
                x + 4.0 + item.text_align_x,
                y + eh + item.text_align_y,
                item.text_scale,
                item.fore,
                out,
            );
            if self.mod_selected == Some(i) {
                p.fill(
                    [x, y, w - SCROLLBAR_SIZE - 4.0, eh - 1.0],
                    item.outline_color,
                    out,
                );
            }
            y += eh;
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
    /// The cvars a menu reads: `cl_ingame` is whether the menus are up over
    /// a game, `shortversion` is vcod's, `com_errorMessage` the last error,
    /// `ui_multiplayer` 1 (this is the MP UI), `cl_languagesavailable` 1
    /// (vcod reads English only), `ui_mousePitch` on when `m_pitch` is
    /// negative (as the UI sets it at load), anything this input set, then
    /// the console's.
    fn cvar(&self, name: &str, shell: &Shell) -> Option<String> {
        if let Some(v) = self.options.pending(name) {
            return Some(v.to_string());
        }
        match name.to_ascii_lowercase().as_str() {
            "cl_ingame" => Some(if self.in_game { "1" } else { "0" }.into()),
            "com_errormessage" => Some(self.error.clone()),
            "ui_favorite_message" => Some(self.fav_message.clone()),
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
pub(crate) struct Painter<'a> {
    pub(crate) sx: f32,
    pub(crate) sy: f32,
    pub(crate) fonts: Option<&'a UiFonts>,
}

impl Painter<'_> {
    pub(crate) fn quad(&self, r: [f32; 4], rgba: [f32; 4], texture: &str, out: &mut Vec<HudQuad>) {
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

    pub(crate) fn fill(&self, r: [f32; 4], rgba: [f32; 4], out: &mut Vec<HudQuad>) {
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
    pub(crate) fn width(&self, text: &str, scale: f32) -> f32 {
        self.font(scale)
            .map_or(0.0, |f| font::measure(f, text, scale / f.unit_scale()))
    }

    /// `text` with its baseline at grid `(x, y)`.
    pub(crate) fn text(
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

    /// A `Ui` whose Local refresh broadcasts nowhere.
    fn ui() -> Option<Ui> {
        let fs = vcod_common::testing::game_fs()?;
        let mut ui = Ui::new(&fs, "0");
        ui.browser.lan_targets.clear();
        Some(ui)
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
    fn the_in_game_main_menu_swaps_its_buttons_and_closes_on_esc() {
        let Some(mut ui) = ui() else { return };
        let shell = Shell::new();
        ui.open_ingame(&mut Vec::new());
        assert_eq!(ui.cvar("cl_ingame", &shell).as_deref(), Some("1"));
        let main = ui.find("main").unwrap();
        let shown: Vec<&str> = (0..ui.menus[main].items.len())
            .filter(|&i| ui.shown(main, i, &shell))
            .map(|i| ui.menus[main].items[i].text.as_str())
            .collect();
        assert!(shown.contains(&"@MENU_BACKTOGAME"));
        assert!(shown.contains(&"@MENU_DISCONNECT"));
        assert!(!shown.contains(&"@MENU_JOIN_GAME"));
        assert!(!shown.contains(&"@MENU_START_NEW_SERVER"));

        // Quit's No reopens main, still over the game.
        click_item(&mut ui, "main", "@MENU_QUIT", &shell);
        click_item(&mut ui, "quit_popmenu", "@MENU_NO", &shell);
        assert_eq!(ui.menus[*ui.open.last().unwrap()].name, "main");
        assert_eq!(ui.cvar("cl_ingame", &shell).as_deref(), Some("1"));

        let out = click_item(&mut ui, "main", "@MENU_DISCONNECT", &shell);
        assert!(out.contains(&UiEffect::Command("disconnect".into())));

        // `onEsc` runs `ingameclose main`.
        let (used, _) = ui.key(KeyCode::Escape);
        assert!(used);
        assert!(!ui.active());

        ui.open_ingame(&mut Vec::new());
        click_item(&mut ui, "main", "@MENU_BACKTOGAME", &shell);
        assert!(!ui.active());

        // With no game up the same Esc leaves main open.
        ui.open_main(&mut Vec::new());
        assert_eq!(ui.cvar("cl_ingame", &shell).as_deref(), Some("0"));
        ui.key(KeyCode::Escape);
        assert!(ui.active());
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
    fn a_mods_menu_list_loads_and_opens_its_own_menus() {
        if vcod_common::testing::game_fs().is_none() {
            return;
        }
        let root = tempfile::tempdir().unwrap();
        let mut w = zip::ZipWriter::new(std::fs::File::create(root.path().join("z.pk3")).unwrap());
        let opts = zip::write::SimpleFileOptions::default();
        w.start_file("ui_mp/menus.txt", opts).unwrap();
        std::io::Write::write_all(
            &mut w,
            br#"{ loadMenu { "ui_mp/main.menu" } loadMenu { "ui_mp/zmenu.menu" } }"#,
        )
        .unwrap();
        w.start_file("ui_mp/zmenu.menu", opts).unwrap();
        std::io::Write::write_all(
            &mut w,
            br#"{ menuDef { name "zmenu" rect 0 0 640 480 onOpen { exec "zmod_opened" } } }"#,
        )
        .unwrap();
        w.finish().unwrap();
        let base = vcod_common::testing::game_dir().join("main");
        let fs = Pk3Fs::open_layered(&base, Some(root.path())).unwrap();
        let mut ui = Ui::new(&fs, "0");
        // Only what the mod's list names: the stock browser is gone.
        assert!(ui.find("joinserver").is_none());
        assert!(ui.find("main").is_some());
        let mut out = Vec::new();
        ui.open_menu("zmenu", &mut out);
        assert_eq!(top_name(&ui), "zmenu");
        assert_eq!(out, [UiEffect::Command("zmod_opened".into())]);
    }

    /// `_UI_Init` adds `ingame.txt`'s menus after `menus.txt`'s, and each
    /// file is tried under `cl_language`'s directory first; `ui_load`
    /// resets the menus and loads only the one list.
    #[test]
    fn the_ui_loads_ingame_txt_and_the_language_directory_first() {
        let mut fs = Pk3Fs::empty();
        let menu = |name: &str, opened: &str| {
            format!(
                r#"{{ menuDef {{ name "{name}" rect 0 0 640 480 onOpen {{ exec "{opened}" }} }} }}"#
            )
            .into_bytes()
        };
        fs.overlay(
            "ui_mp/menus.txt",
            br#"{ loadMenu { "ui_mp/a.menu" } }"#.to_vec(),
        );
        fs.overlay(
            "ui_mp/ingame.txt",
            br#"{ loadMenu { "ui_mp/ingame.menu" "ui_mp/q.menu" } }"#.to_vec(),
        );
        fs.overlay("ui_mp/a.menu", menu("a", "plain_a"));
        fs.overlay("ui_mp/german/a.menu", menu("a", "german_a"));
        fs.overlay("ui_mp/q.menu", menu("quickmessage", "q"));
        let opened = |ui: &mut Ui| {
            let mut out = Vec::new();
            ui.open_menu("a", &mut out);
            out
        };
        let mut ui = Ui::new(&fs, "0");
        assert!(ui.find("quickmessage").is_some());
        assert_eq!(opened(&mut ui), [UiEffect::Command("plain_a".into())]);
        let mut ui = Ui::new(&fs, "2");
        assert_eq!(opened(&mut ui), [UiEffect::Command("german_a".into())]);
        ui.reload(&fs, "", "2", &mut Vec::new());
        assert!(ui.find("quickmessage").is_none());
    }

    /// `ui_load` reads the list `ui_menuFiles` names, falls back to the stock
    /// list when it is missing, and reopens the menu that had focus.
    #[test]
    fn ui_load_reloads_off_the_named_list_and_reopens_the_focused_menu() {
        let mut fs = Pk3Fs::empty();
        let menu = |name: &str, opened: &str| {
            format!(
                r#"{{ menuDef {{ name "{name}" rect 0 0 640 480 onOpen {{ exec "{opened}" }} }} }}"#
            )
            .into_bytes()
        };
        fs.overlay(
            "ui_mp/menus.txt",
            br#"{ loadMenu { "ui_mp/a.menu" } }"#.to_vec(),
        );
        fs.overlay("ui_mp/a.menu", menu("a", "stock_a"));
        fs.overlay(
            "ui_mp/alt.txt",
            br#"{ loadMenu { "ui_mp/a2.menu" "ui_mp/b.menu" } }"#.to_vec(),
        );
        fs.overlay("ui_mp/a2.menu", menu("a", "alt_a"));
        fs.overlay("ui_mp/b.menu", menu("b", "alt_b"));
        let mut ui = Ui::new(&fs, "0");
        assert!(ui.find("b").is_none());
        ui.open_menu("a", &mut Vec::new());
        let mut out = Vec::new();
        ui.reload(&fs, "ui_mp/alt.txt", "0", &mut out);
        assert!(ui.find("b").is_some());
        assert_eq!(top_name(&ui), "a");
        assert_eq!(out, [UiEffect::Command("alt_a".into())]);
        out.clear();
        ui.reload(&fs, "ui_mp/missing.txt", "0", &mut out);
        assert!(ui.find("b").is_none(), "back on the stock list");
        assert_eq!(out, [UiEffect::Command("stock_a".into())]);
    }

    #[test]
    fn the_mods_menu_lists_mod_dirs_and_runs_the_picked_one() {
        let Some(ui) = ui() else { return };
        let root = tempfile::tempdir().unwrap();
        let zmod = root.path().join("zmod");
        std::fs::create_dir_all(&zmod).unwrap();
        let mut w = zip::ZipWriter::new(std::fs::File::create(zmod.join("z.pk3")).unwrap());
        w.start_file("a.txt", zip::write::SimpleFileOptions::default())
            .unwrap();
        w.finish().unwrap();
        std::fs::write(zmod.join("description.txt"), "Zombies").unwrap();
        let mut ui = ui.with_mod_root(root.path().to_path_buf(), "main".into());
        let mut shell = Shell::new();
        ui.open_main(&mut Vec::new());
        click_item(&mut ui, "main", "@MENU_MODS", &shell);
        assert_eq!(top_name(&ui), "mods_menu");
        assert_eq!(ui.mods.len(), 1, "loadMods ran on open");
        let quads = ui.build(640.0, 480.0, &Localized::default(), &shell);
        assert!(!quads.is_empty());
        // A click picks the row and shows Launch; Launch runs the mod.
        click_named(&mut ui, "mods_menu", "modlist", &mut shell);
        assert_eq!(ui.mod_selected, Some(0));
        let m = ui.find("mods_menu").unwrap();
        let accept = (0..ui.menus[m].items.len())
            .find(|&i| ui.menus[m].items[i].name == "accept")
            .unwrap();
        assert!(ui.shown(m, accept, &shell));
        let r = ui.menus[m].items[accept].rect;
        ui.mouse_move(r[0] + 2.0, r[1] + 2.0, 640.0, 480.0, &shell);
        let out = ui.click(Instant::now(), &shell);
        assert!(
            out.contains(&UiEffect::RunMod(Some("zmod".into()))),
            "{out:?}"
        );
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

    /// Applies the `set` commands a menu sent, as the app does.
    fn apply(shell: &mut Shell, out: &[UiEffect]) {
        for e in out {
            if let UiEffect::Command(line) = e {
                shell.execute(line);
            }
        }
    }

    fn top_name(ui: &Ui) -> &str {
        &ui.menus[*ui.open.last().unwrap()].name
    }

    fn click_at(ui: &mut Ui, m: usize, i: usize, shell: &mut Shell) {
        let r = ui.menus[m].items[i].rect;
        ui.mouse_move(r[0] + 2.0, r[1] + 2.0, 640.0, 480.0, shell);
        assert_eq!(ui.hover, Some((m, i)));
        let out = ui.click(Instant::now(), shell);
        apply(shell, &out);
        ui.frame(Instant::now(), shell);
    }

    fn click_named(ui: &mut Ui, menu: &str, name: &str, shell: &mut Shell) {
        let m = ui.find(menu).unwrap();
        let i = (0..ui.menus[m].items.len())
            .find(|&i| ui.menus[m].items[i].name == name && ui.shown(m, i, shell))
            .unwrap_or_else(|| panic!("{name}"));
        click_at(ui, m, i, shell);
    }

    fn type_text(ui: &mut Ui, text: &str, shell: &mut Shell) {
        let out = ui.edit_key(KeyCode::KeyA, Some(text), shell);
        apply(shell, &out);
        ui.edit_key(KeyCode::Enter, None, shell);
    }

    #[test]
    fn a_new_favourite_is_typed_in_and_listed() {
        let Some(ui) = ui() else { return };
        let dir = tempfile::tempdir().unwrap();
        let mut ui = ui.with_server_cache(dir.path().join("servercache.dat"));
        ui.browser.lan_targets.clear();
        let mut shell = Shell::new();
        ui.frame(Instant::now(), &shell);
        ui.open_menu("joinserver", &mut Vec::new());
        // Local, then Internet, then Favorites; New Favorite shows on the last.
        click_named(&mut ui, "joinserver", "sourcefield", &mut shell);
        assert_eq!(ui.browser.source, Source::Internet);
        click_named(&mut ui, "joinserver", "sourcefield", &mut shell);
        assert_eq!(shell.cvar("ui_netSource"), Some("2"));
        click_named(&mut ui, "joinserver", "createFavorite", &mut shell);
        assert_eq!(top_name(&ui), "createfavorite_popmenu");
        let m = ui.find("createfavorite_popmenu").unwrap();
        let fields: Vec<usize> = (0..ui.menus[m].items.len())
            .filter(|&i| ui.menus[m].items[i].kind == ui_menu::ITEM_TYPE_EDITFIELD)
            .collect();
        for (&i, text) in fields.iter().zip(["Home", "10.0.0.7:29661"]) {
            click_at(&mut ui, m, i, &mut shell);
            type_text(&mut ui, text, &mut shell);
        }
        assert_eq!(shell.cvar("ui_favoriteAddress"), Some("10.0.0.7:29661"));
        ui.frame(Instant::now(), &shell);
        click_named(&mut ui, "createfavorite_popmenu", "yes", &mut shell);
        assert_eq!(top_name(&ui), "fav_message_popmenu");
        assert_eq!(ui.fav_message, "@EXE_FAVORITEADDED");
        let rows: Vec<String> = ui
            .browser
            .rows()
            .iter()
            .map(|s| s.addr.to_string())
            .collect();
        assert_eq!(rows, ["10.0.0.7:29661"]);
    }

    #[test]
    fn the_password_and_filter_popups_set_their_cvars() {
        let Some(mut ui) = ui() else { return };
        let mut shell = Shell::new();
        ui.frame(Instant::now(), &shell);
        ui.open_menu("joinserver", &mut Vec::new());
        click_named(&mut ui, "joinserver", "passwordenter", &mut shell);
        assert_eq!(top_name(&ui), "password_popmenu");
        click_named(&mut ui, "password_popmenu", "passwordEntry", &mut shell);
        type_text(&mut ui, "letmein_and_more", &mut shell);
        // `maxchars 12`.
        assert_eq!(shell.cvar("password"), Some("letmein_and_"));
        assert_eq!(shell.userinfo().password, "letmein_and_");
        click_named(&mut ui, "password_popmenu", "yes", &mut shell);
        assert_eq!(top_name(&ui), "joinserver");

        click_named(&mut ui, "joinserver", "filterServers", &mut shell);
        assert_eq!(top_name(&ui), "filter_popmenu");
        let m = ui.find("filter_popmenu").unwrap();
        let i = ui.menus[m]
            .items
            .iter()
            .position(|i| i.cvar.as_deref() == Some("ui_browserShowEmpty"))
            .unwrap();
        click_at(&mut ui, m, i, &mut shell);
        assert_eq!(shell.cvar("ui_browserShowEmpty"), Some("0"));
        assert!(!ui.browser.filter.show_empty);
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
