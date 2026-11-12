//! Contains routines ported from the RTCW-MP GPL source, Copyright (C) 1999-2010 id Software LLC, a ZeniMax Media company.
//! See NOTICE.
//!
//! The front end: the stock main menu, server browser and its popups
//! (password, server info, filter, favourites), options screens, quit and
//! error popups drawn from their `.menu` files and driven by mouse and keys,
//! as the UI module does while no game is up, and the main menu again over a
//! game with `cl_ingame` 1 (docs/research/cod11-front-end.md).
//! The Mods menu lists the mod directories and switches `fs_game`, which
//! rebuilds the UI off the new search path. Start New Server launches a
//! `vcod-server` and joins it ([`create`]). Menus vcod cannot run yet (CD
//! key, single player) are refused with a console line instead of drawing
//! screens whose controls do nothing.

pub mod browser;
pub mod cod2;
pub mod create;
mod listbox;
mod options;
mod players;
pub mod profiles;
mod status;

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::net::SocketAddrV4;
use std::path::PathBuf;
use std::time::{Duration, Instant};
use vcod_common::asset_source::{Asset, AssetSource};
use vcod_common::localize::Localized;
use vcod_common::pk3::Pk3Fs;
use vcod_common::ui_menu::{
    self, FEEDER_ALLMAPS, FEEDER_MODS, FEEDER_MUTELIST, FEEDER_PLAYER_LIST, FEEDER_PLAYER_PROFILES,
    FEEDER_SERVERS, FEEDER_SERVERSTATUS, ITEM_ALIGN_CENTER, ITEM_ALIGN_RIGHT, ITEM_TYPE_BIND,
    ITEM_TYPE_BUTTON, ITEM_TYPE_DECIMALFIELD, ITEM_TYPE_DVARENUM, ITEM_TYPE_EDITFIELD,
    ITEM_TYPE_LISTBOX, ITEM_TYPE_MULTI, ITEM_TYPE_NUMERICFIELD, ITEM_TYPE_OWNERDRAW,
    ITEM_TYPE_SLIDER, ITEM_TYPE_TEXT, ITEM_TYPE_YESNO, MenuFile, UiItem, UiMenu,
    WINDOW_STYLE_CVAR_SHADER, WINDOW_STYLE_FILLED, WINDOW_STYLE_SHADER,
};
use winit::keyboard::KeyCode;

use crate::console::shell::Shell;
use crate::hud::HudQuad;
use crate::hud::font::{self, Slot, UiFonts};
use crate::hud::hudelem::Virtual;
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
/// Returns what the files' `assetGlobalDef`s set, the last one winning.
fn load_menus(fs: &dyn AssetSource, list: &str, language: &str, menus: &mut Vec<UiMenu>) -> Assets {
    let read = |p: &str| fs.read(p).map(|b| String::from_utf8_lossy(&b).into_owned());
    let text = read(list).or_else(|| {
        crate::console::log::print(&format!("^3menu file not found: {list}, using default"));
        read(MENU_LIST)
    });
    let files = text.map_or_else(Vec::new, |t| ui_menu::menu_list(&t));
    if files.is_empty() {
        log::warn!("ui: no menus in {list}");
    }
    let mut assets = Assets::default();
    for path in &files {
        let file = ui_menu::localized_menu_path(path, language)
            .and_then(|p| MenuFile::find(fs, &p).ok())
            .or_else(|| MenuFile::find(fs, path).ok());
        match file {
            Some(file) => {
                menus.extend(file.menus);
                assets.item_focus_sound = file.item_focus_sound.or(assets.item_focus_sound);
                assets.cursor = file.cursor.or(assets.cursor);
            }
            // Stock lists name files no pak ships (CoD2's eleven,
            // cod2-fonts-menus.md); `UI_ParseMenuInternal` (Mac 0x1acac4)
            // skips one without a warning.
            None => log::trace!("ui: no {path}, skipped"),
        }
    }
    assets
}

/// The `assetGlobalDef` keys the UI uses.
#[derive(Default)]
struct Assets {
    /// `itemFocusSound`.
    item_focus_sound: Option<String>,
    /// `cursor`, drawn at the mouse while a menu is up ([`Ui::build`]).
    cursor: Option<String>,
}

impl Assets {
    fn or(self, earlier: Assets) -> Assets {
        Assets {
            item_focus_sound: self.item_focus_sound.or(earlier.item_focus_sound),
            cursor: self.cursor.or(earlier.cursor),
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
const UNSUPPORTED: [&str; 10] = [
    "single_popmenu",
    "options_driverinfo",
    "options_credits",
    "language_restart_popmenu",
    "rec_restart_popmenu",
    "multi_menu",
    "cdkey_menu",
    "connect",
    "single_player_menu",
    "auconfirm",
];

fn unsupported(name: &str) -> bool {
    UNSUPPORTED.iter().any(|s| s.eq_ignore_ascii_case(name))
}

/// `ownerdraw` ids from `ui_mp/menudef.h`.
/// Retail key numbers (CoDMP.exe keynames table, 0x57b8d0).
const K_MOUSE1: u32 = 200;
const K_MOUSE2: u32 = 201;
const K_MOUSE3: u32 = 202;
const K_MOUSE4: u32 = 203;
const K_MOUSE5: u32 = 204;
const K_ENTER: u32 = 13;
const K_KP_ENTER: u32 = 191;
const K_MWHEELDOWN: u32 = 205;
const K_MWHEELUP: u32 = 206;

const UI_NETSOURCE: i32 = 220;
const UI_NETGAMETYPE: i32 = 245;
const UI_NETMAPPREVIEW: i32 = 221;
const UI_NETMAPCINEMATIC: i32 = 246;
const UI_STARTMAPCINEMATIC: i32 = 255;
const UI_SERVERREFRESHDATE: i32 = 247;
const UI_JOINGAMETYPE: i32 = 253;
/// `ui_mp/menudef.h`'s `UI_SHOW_FAVORITESERVERS` and
/// `UI_SHOW_NOTFAVORITESERVERS`: shown only while the source is, or is not,
/// Favorites (ui_mp_x86.dll 0x40009780).
const UI_SHOW_FAVORITESERVERS: i32 = 0x4;
const UI_SHOW_NOTFAVORITESERVERS: i32 = 0x1000;

/// The shell cvars the browser's and create server's scripts read, copied
/// each frame.
const UI_CVARS: [&str; 12] = [
    "ui_netSource",
    "ui_joinGametype",
    "ui_browserShowFull",
    "ui_browserShowEmpty",
    "ui_browserShowPassword",
    "ui_browserShowNoPassword",
    "ui_favoriteName",
    "ui_favoriteAddress",
    "ui_netGametype",
    "ui_netGametypeName",
    "ui_dedicated",
    "g_gametype",
];

/// Q3's `SCROLLBAR_SIZE`, which a list row's highlight stops short of.
const SCROLLBAR_SIZE: f32 = 16.0;
const DOUBLE_CLICK: Duration = Duration::from_millis(400);

/// `UI_SetActiveMenu`'s ids (ui_mp_x86.dll 0x4000d810) that vcod opens
/// menus through. The last one used is the UI's active menu type
/// (0x401bf42c), which decides whether a script menu may replace the
/// focused menu; cases 10 and 11 both leave 10.
pub const UIMENU_NONE: i32 = 0;
pub const UIMENU_MAIN: i32 = 1;
pub const UIMENU_INGAME: i32 = 2;
pub const UIMENU_WM_QUICKMESSAGE: i32 = 8;
pub const UIMENU_SCRIPT_POPUP: i32 = 10;

/// What a menu script asks of the app.
#[derive(Debug, Clone, PartialEq)]
pub enum UiEffect {
    /// `scriptMenuResponse <response>` from a script of the menu `menu`:
    /// the `mr` the server reads, its index looked up by the menu's name
    /// (ui_mp_x86.dll 0x40011140).
    ScriptMenuResponse { menu: String, response: String },
    /// A console command line (`connect`, `disconnect`, `quit`, `set`).
    Command(String),
    /// A sound alias played on the viewer.
    Sound(String),
    /// `Item_SetFocus`'s sound: the item's `focusSound` when the menu sound
    /// set has that alias, else `itemFocusSound` when it has that one.
    FocusSound {
        item: Option<String>,
        global: Option<String>,
    },
    /// `RunMod`'s `fs_game` and `vid_restart`: switch to this mod
    /// directory, or back to the base game with `None` (`Quake3`).
    RunMod(Option<String>),
    /// `StartServer`: launch a server and join it.
    StartServer(create::StartServer),
    /// CoD2's `uiScript mutePlayer`: flip the client's mute
    /// (`CL_MutePlayer`, cod2-voice.md 5).
    MutePlayer(u32),
    /// CoD2's `loadPlayerProfile`: make this profile the active one
    /// (`Com_ChangePlayerProfile`, Mac 0x111936).
    LoadProfile(String),
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
    /// Open menus, drawn in order (`menuStack`'s visible ones).
    open: Vec<usize>,
    /// The stack's menus an `outOfBoundsClick` closed, which retail leaves
    /// on the stack without their visible flag (`Menus_HandleOOBClick`,
    /// 0x40013500): each with how many of `open` sit under it, in stack
    /// order. `focus_menu` may name one; no menu has the keys then.
    hidden: Vec<(usize, usize)>,
    /// The open menu with `WINDOW_HASFOCUS`, which takes the keys
    /// (`Menu_GetFocused`, 0x40016970): the last one opened, or the one an
    /// off-menu click went to ([`Ui::click_menu`]); the stack keeps its order.
    focus_menu: Option<usize>,
    fonts: Option<UiFonts>,
    /// Mouse position on the 640x480 grid.
    cursor: [f32; 2],
    /// The item a click at the cursor goes to, as (menu, item): the top
    /// menu's, or one of a menu below it when the cursor is off the top
    /// menu's rect ([`Ui::hit`]).
    hover: Option<(usize, usize)>,
    /// The items with `WINDOW_MOUSEOVER`, as (menu, item): each one whose
    /// `mouseEnter` ran and whose `mouseExit` has not. Overlapping items
    /// all have it.
    over: HashSet<(usize, usize)>,
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
    /// `LoadMods`' list, the selected row and the first row drawn.
    mods: Vec<vcod_common::pk3::ModEntry>,
    mod_selected: Option<usize>,
    mod_top: usize,
    last_mod_click: Option<(Instant, usize)>,
    /// The `.arena` maps on the search path.
    arenas: Vec<create::Arena>,
    /// The selected row of `FEEDER_ALLMAPS` and its first row drawn.
    map_row: usize,
    map_top: usize,
    /// `FEEDER_PLAYER_LIST`'s and CoD2's `FEEDER_MUTELIST`'s cleaned names
    /// (one list, `UI_FeederCount` Mac 0x156946), their client numbers
    /// (CoD2's only), the selected row (0 until a pick, as retail's zeroed
    /// `uiInfo`), first row drawn and next rebuild.
    players: Vec<String>,
    player_clients: Vec<u32>,
    /// `CL_IsPlayerMuted` for the mute list, bit `n` for client `n`, set
    /// each frame ([`Self::set_muted`]).
    muted: u64,
    player_row: usize,
    player_top: usize,
    players_due: Option<Instant>,
    /// CoD2's player profiles (`FEEDER_PLAYER_PROFILES`) and the last
    /// click on their list.
    profiles: profiles::Profiles,
    last_profile_click: Option<Instant>,
    /// The `cvar` of the item whose script runs (`openForGameType` reads it).
    script_cvar: Option<String>,
    /// The list box whose scroll bar thumb the left button holds.
    thumb_drag: Option<(usize, usize)>,
    /// The list box whose scroll bar arrow a button holds, and the arrow.
    arrow_hold: Option<((usize, usize), listbox::Part, listbox::Repeat)>,
    /// Each menu's `cursorItem`: the item focus last landed on, which the
    /// wheel and Tab step from.
    cursor_item: HashMap<usize, usize>,
    /// Each menu's item with `WINDOW_HASFOCUS`, as far as the focus scripts
    /// and sounds go: hover, a focus step and an edit's Tab move it.
    focus: HashMap<usize, usize>,
    /// `UI_LoadGameTypes`' list; the join filter puts "All" ahead of it.
    game_types: Vec<ui_menu::GameType>,
    /// The maps with a `levelshots/<map>` image, lower-case.
    levelshots: HashSet<String>,
    /// `assetGlobalDef`'s `itemFocusSound` and `cursor`.
    assets: Assets,
    /// Menus whose `onClose` is running.
    closing: Vec<usize>,
    /// A key reached the UI with no focused menu: `_UI_KeyEvent`
    /// (0x4000d6f0) took the UI down and left the menus on the stack,
    /// still flagged visible, until a menu opens again.
    down: bool,
    /// The `textRect` (x and baseline, width, height) of each item whose
    /// paint runs `Item_Text_Paint` ([`paints_text`]), as its paints left
    /// it, as (menu, item): `Item_SetTextExtents` (0x40013bb0)
    /// sets it at the first paint and keeps it, an autowrapped item's lines
    /// move it to the last line, and the focus and the click test it
    /// ([`Self::text_rect`]). A load clears it, as `Item_SetScreenCoords`
    /// (0x4000fd00) zeroes the width.
    text_rects: RefCell<HashMap<(usize, usize), [f32; 4]>>,
    /// The menus the lists loaded; the script menus the server precached
    /// follow them ([`Self::load_script_menus`]).
    list_menus: usize,
    /// The `UI_SetActiveMenu` id the last menu opened through
    /// (`UIMENU_*`).
    active_type: i32,
    /// The client cvars the server set (`v`, and the 140/204 mirror),
    /// lower-case names; the script menus' cvar tests read them.
    server_cvars: HashMap<String, String>,
    /// CoD2's menus' parsed rects, placed by their alignment for the
    /// window ([`Self::set_screen`]); `None` for CoD1.
    cod2: Option<cod2::Cod2Rects>,
}

/// The maps `levelshots/` holds an image for.
fn levelshots(fs: &Pk3Fs) -> HashSet<String> {
    fs.list_prefix("levelshots/")
        .iter()
        .filter_map(|p| {
            let name = &p["levelshots/".len()..];
            vcod_common::assets::IMAGE_EXTS
                .iter()
                .find_map(|ext| name.strip_suffix(ext))
        })
        .filter(|n| !n.contains('/'))
        .map(str::to_string)
        .collect()
}

impl Ui {
    /// `_UI_Init` sets `ui_menuFiles` back to its default before reading
    /// it (0x4000d324), so the UI always starts off [`MENU_LIST`], then
    /// adds [`INGAME_LIST`]'s. `language` is `cl_language`'s value.
    pub fn new(fs: &Pk3Fs, language: &str) -> Ui {
        Self::open(fs, language, true)
    }

    /// A front end with no menus, fonts, arenas or game types read: a
    /// CoD4 session draws its own (`play::cod4_menus`), and its search
    /// path holds none of CoD1's or CoD2's.
    pub fn without_menus(fs: &Pk3Fs) -> Ui {
        Self::open(fs, "", false)
    }

    fn open(fs: &Pk3Fs, language: &str, load: bool) -> Ui {
        let mut menus = Vec::new();
        let assets = if load {
            let assets = load_menus(fs, MENU_LIST, language, &mut menus);
            load_menus(fs, INGAME_LIST, language, &mut menus).or(assets)
        } else {
            Assets::default()
        };
        let state = item_states(&menus);
        let arenas = if load {
            create::load_arenas(fs)
        } else {
            Default::default()
        };
        let mut browser = Browser::default();
        browser.columns = server_columns(&menus);
        // The install's own protocol only: 1.1, 1.5, UO 1.51 or CoD2 1.3.
        browser.master_query = Some(browser::MasterQuery::of(match fs.game() {
            vcod_common::game::Game::Cod2 => &vcod_cod2::net::PROTOCOL_118.transport,
            _ => &vcod_common::net::protocol::install_protocol(fs).transport,
        }));
        let fonts = load
            .then(|| {
                UiFonts::load(fs)
                    .map_err(|e| log::warn!("ui: {e}, drawing no text"))
                    .ok()
            })
            .flatten();
        Ui {
            menus,
            state,
            open: Vec::new(),
            hidden: Vec::new(),
            down: false,
            focus_menu: None,
            fonts,
            cursor: [0.0; 2],
            hover: None,
            over: HashSet::new(),
            browser,
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
            mod_top: 0,
            last_mod_click: None,
            arenas,
            map_row: 0,
            map_top: 0,
            players: Vec::new(),
            player_clients: Vec::new(),
            muted: 0,
            player_row: 0,
            player_top: 0,
            players_due: None,
            profiles: Default::default(),
            last_profile_click: None,
            script_cvar: None,
            thumb_drag: None,
            arrow_hold: None,
            cursor_item: HashMap::new(),
            focus: HashMap::new(),
            game_types: if load {
                ui_menu::game_types(fs)
            } else {
                Vec::new()
            },
            levelshots: if load { levelshots(fs) } else { HashSet::new() },
            assets,
            closing: Vec::new(),
            text_rects: RefCell::default(),
            list_menus: 0,
            active_type: UIMENU_NONE,
            server_cvars: HashMap::new(),
            cod2: (fs.game() == vcod_common::game::Game::Cod2).then(cod2::Cod2Rects::default),
        }
        .with_list_menus()
    }

    fn with_list_menus(mut self) -> Ui {
        self.list_menus = self.menus.len();
        self.sync_cod2_rects(0);
        self
    }

    /// CoD2: takes the parsed rects of every menu past the first `keep`.
    fn sync_cod2_rects(&mut self, keep: usize) {
        if let Some(c) = &mut self.cod2 {
            c.sync(&self.menus, keep);
        }
    }

    /// The window is `w` x `h` px. CoD2 places its menus' rects for it
    /// (`CalcScreenPlacement`, [`cod2`]); CoD1's sit on the grid as parsed.
    /// Call before [`Self::build`] and the mouse.
    pub fn set_screen(&mut self, w: f32, h: f32) {
        if let Some(c) = &mut self.cod2
            && c.place(&mut self.menus, w, h)
        {
            // The kept text rects were measured off the old rects.
            self.text_rects.borrow_mut().clear();
        }
    }

    /// The Mods menu lists the directories of `game_dir` beside `base`.
    pub fn with_mod_root(mut self, game_dir: PathBuf, base: String) -> Ui {
        self.mod_root = Some((game_dir, base));
        self
    }

    /// Favourites live in `path` (`servercache.dat`, beside `CoDMP.exe`).
    pub fn with_server_cache(mut self, path: PathBuf) -> Ui {
        self.set_browser(Browser::with_cache(path));
        self
    }

    /// Takes `browser`'s lists, laid out by this UI's `joinserver.menu` and
    /// asking this UI's install's master.
    pub fn set_browser(&mut self, browser: Browser) {
        let master_query = self.browser.master_query;
        self.browser = browser;
        self.browser.columns = server_columns(&self.menus);
        self.browser.master_query = master_query;
    }

    /// Whether a menu is up and taking input.
    pub fn active(&self) -> bool {
        !self.open.is_empty() && !self.down
    }

    /// `UI_IS_FULLSCREEN` (ui_mp_x86.dll 0x40016ab0): a menu on the stack
    /// flagged visible and fullscreen, a UI taken down by a key included.
    pub fn fullscreen(&self) -> bool {
        self.open.iter().any(|&m| self.menus[m].fullscreen)
    }

    /// `CL_GetMenuBlurRadius` (CoD2 Mac 0x00172970): with the UI's key
    /// catcher up, the painted menus' `blurWorld`s added in quadrature
    /// (`Menu_Paint` 0x0017077c), 480-high virtual units; 0 otherwise.
    /// The caller drops it on a demo (cod2-materials.md 9.12).
    pub fn blur_world(&self) -> f32 {
        if !self.active() {
            return 0.0;
        }
        self.open
            .iter()
            .map(|&m| self.menus[m].blur_world.powi(2))
            .sum::<f32>()
            .sqrt()
    }

    /// `_UI_KeyEvent` (0x4000d6f0) for a key, a button or a release: with
    /// no focused menu (`Menu_GetFocused`, 0x40016970, finds none with
    /// both flags) it clears the UI's key catcher bit, runs
    /// `Key_ClearStates` and sets `cl_paused` 0, and the menus stay where
    /// they are. True when the key went that way.
    fn unfocused_key(&mut self) -> bool {
        if self.active() && self.focused_menu().is_none() {
            self.down = true;
            return true;
        }
        false
    }

    /// The main menu, as retail shows it with no game up. While connecting
    /// or loading this is also what Esc opens (`UIMENU_MAIN`).
    pub fn open_main(&mut self, shell: &Shell, out: &mut Vec<UiEffect>) {
        self.in_game = false;
        self.active_type = UIMENU_MAIN;
        self.open_menu("main", shell, out);
    }

    /// The main menu over a game: `cl_ingame` 1 swaps Join a Game and Start
    /// New Server for Back to Game and Disconnect, and its Esc closes it.
    pub fn open_ingame(&mut self, shell: &Shell, out: &mut Vec<UiEffect>) {
        self.open.clear();
        self.hidden.clear();
        self.in_game = true;
        self.active_type = UIMENU_INGAME;
        self.open_menu("main", shell, out);
    }

    /// `ui_load` (`UI_Load`, 0x400086c0): reload the menus off the list
    /// `ui_menuFiles` names, close them all and reopen the one that had
    /// focus. [`INGAME_LIST`]'s menus are gone until the next restart, as
    /// `UI_Load` resets the menus and loads the one list.
    pub fn reload(
        &mut self,
        fs: &Pk3Fs,
        list: &str,
        language: &str,
        shell: &Shell,
        out: &mut Vec<UiEffect>,
    ) {
        let focused = self.focused_menu().map(|m| self.menus[m].name.clone());
        let list = if list.is_empty() { MENU_LIST } else { list };
        self.menus.clear();
        self.text_rects.borrow_mut().clear();
        self.assets = load_menus(fs, list, language, &mut self.menus);
        // The script menus go with the rest until the cgame loads them again.
        self.list_menus = self.menus.len();
        self.sync_cod2_rects(0);
        // `UI_Load` reloads the game types too (0x4000871d).
        self.game_types = ui_menu::game_types(fs);
        self.state = item_states(&self.menus);
        self.arenas = create::load_arenas(fs);
        self.browser.columns = server_columns(&self.menus);
        self.open.clear();
        self.hidden.clear();
        self.focus_menu = None;
        self.hover = None;
        self.over.clear();
        self.cursor_item.clear();
        self.focus.clear();
        self.options.clear();
        if let Some(name) = focused {
            self.open_menu(&name, shell, out);
        }
    }

    /// Everything closes when a game starts.
    pub fn close_all(&mut self) {
        self.open.clear();
        self.hidden.clear();
        self.down = false;
        self.active_type = UIMENU_NONE;
        self.focus_menu = None;
        self.hover = None;
        self.options.clear();
        self.status = None;
        self.browser.stop();
        self.in_game = false;
    }

    /// A drop or a failed connect: the main menu with the error popup over
    /// it, as `Com_Error` leaves retail.
    pub fn show_error(&mut self, message: &str, shell: &Shell, out: &mut Vec<UiEffect>) {
        self.error = message.to_string();
        self.in_game = false;
        self.active_type = UIMENU_MAIN;
        self.open_menu("main", shell, out);
        self.open_menu("error_popmenu", shell, out);
    }

    /// The cgame's script menu precache (cgame_mp_x86.dll 0x3002c060): each
    /// name in configstrings 1180..1211 loads as a UI menu from
    /// `ui_mp/scriptmenus/<name>.menu`, `cl_language`'s directory first (UI
    /// `vmMain` 15, 0x40008330), after the lists' menus. The ones loaded
    /// before go first, closed. Returns the names with no file; retail
    /// drops the game over one (`Could not load script menu file`).
    pub fn load_script_menus(
        &mut self,
        fs: &dyn AssetSource,
        names: &[String],
        language: &str,
    ) -> Vec<String> {
        let base = self.list_menus;
        self.open.retain(|&m| m < base);
        self.hidden.retain(|&(m, _)| m < base);
        self.focus_menu = self.focus_menu.filter(|&m| m < base);
        self.hover = self.hover.filter(|&(m, _)| m < base);
        self.over.retain(|&(m, _)| m < base);
        self.cursor_item.retain(|&m, _| m < base);
        self.focus.retain(|&m, _| m < base);
        self.text_rects.borrow_mut().retain(|&(m, _), _| m < base);
        for m in base..self.menus.len() {
            self.options.forget(m);
        }
        self.menus.truncate(base);
        self.state.truncate(base);
        let mut missing = Vec::new();
        for name in names.iter().filter(|n| !n.is_empty()) {
            let path = format!("ui_mp/scriptmenus/{name}.menu");
            let file = ui_menu::localized_menu_path(&path, language)
                .and_then(|p| MenuFile::find(fs, &p).ok())
                .or_else(|| MenuFile::find(fs, &path).ok());
            match file {
                // `UI_ParseMenu` (0x40008240) runs `Asset_Parse` on a
                // script menu's `assetGlobalDef` as on a list menu's.
                Some(file) => {
                    let assets = Assets {
                        item_focus_sound: file.item_focus_sound,
                        cursor: file.cursor,
                    };
                    self.assets = assets.or(std::mem::take(&mut self.assets));
                    self.state.extend(item_states(&file.menus));
                    self.menus.extend(file.menus);
                }
                None => missing.push(name.clone()),
            }
        }
        self.sync_cod2_rects(base);
        missing
    }

    /// The server's client cvars (`v` and the 140/204 mirror) the script
    /// menus read, keyed by name in any case.
    pub fn set_server_cvars(&mut self, cvars: HashMap<String, String>) {
        self.server_cvars = cvars
            .into_iter()
            .map(|(k, v)| (k.to_ascii_lowercase(), v))
            .collect();
    }

    /// `UI_SetActiveMenu` cases 10 and 11 (0x4000da8b), a `t`: refused
    /// (false) while a menu that did not come from a script popup has the
    /// focus; the focused menu itself again changes nothing. Otherwise the
    /// cursor goes to the centre, or the bottom-right corner for
    /// `no_mouse`, every menu closes and `name` opens. A refused `t` waits
    /// in the cgame and is offered again each frame (0x3002dd10).
    pub fn open_script_popup(
        &mut self,
        name: &str,
        no_mouse: bool,
        shell: &Shell,
        out: &mut Vec<UiEffect>,
    ) -> bool {
        if let Some(f) = self.focused_menu() {
            if self.active_type != UIMENU_SCRIPT_POPUP {
                return false;
            }
            if self.menus[f].name.eq_ignore_ascii_case(name) {
                return true;
            }
        }
        self.active_type = UIMENU_SCRIPT_POPUP;
        self.cursor = if no_mouse {
            [639.0, 479.0]
        } else {
            [320.0, 240.0]
        };
        self.in_game = true;
        self.close_every_menu(shell, out);
        self.open_menu(name, shell, out);
        true
    }

    /// `UI_SetActiveMenu` case 2 (0x4000d9c3), Esc in a live game: every
    /// menu closes and the one `g_scriptMainMenu` names opens. The cursor
    /// stays where it is.
    pub fn open_script_main(&mut self, name: &str, shell: &Shell, out: &mut Vec<UiEffect>) {
        self.active_type = UIMENU_INGAME;
        self.in_game = true;
        self.close_every_menu(shell, out);
        self.open_menu(name, shell, out);
    }

    /// `UI_SetActiveMenu` case 8 (0x4000da11), `mp_QuickMessage`: the
    /// cursor parks in the bottom-right corner, every menu closes and
    /// `quickmessage` opens.
    /// False when no `quickmessage` is loaded (after a `ui_load`): the
    /// menus still close and nothing opens.
    pub fn open_quick_message(&mut self, shell: &Shell, out: &mut Vec<UiEffect>) -> bool {
        self.active_type = UIMENU_WM_QUICKMESSAGE;
        self.cursor = [639.0, 479.0];
        self.in_game = true;
        self.close_every_menu(shell, out);
        let name = crate::play::join::QUICK_MESSAGE_MENU;
        if !self.has_menu(name) {
            return false;
        }
        self.open_menu(name, shell, out);
        true
    }

    /// `u`: the cgame's close (0x3002de60, CoDMP.exe 0x4035d5) sends the UI
    /// an Esc when its active menu type is a script popup.
    pub fn close_script_popup(&mut self, shell: &Shell) -> Vec<UiEffect> {
        if self.active_type != UIMENU_SCRIPT_POPUP {
            return Vec::new();
        }
        self.key(KeyCode::Escape, shell).1
    }

    /// CoD2's `K` (`closeInGameMenu`): `CL_CloseInGameMenu` (Mac
    /// 0x17afbe) runs `UI_CloseFocusedMenu` (0x1518ae) when the menu Esc
    /// opened is up, which closes every menu unless a fullscreen one is
    /// shown. The stock gametypes send it ahead of the `t` that answers an
    /// `ingame` button, which `UI_SetActiveMenu` would refuse while
    /// `ingame` holds the focus.
    pub fn close_in_game_menu(&mut self, shell: &Shell) -> Vec<UiEffect> {
        let mut out = Vec::new();
        if self.active_type == UIMENU_INGAME && self.focused_menu().is_some() && !self.fullscreen()
        {
            self.close_every_menu(shell, &mut out);
        }
        out
    }

    /// Whether a key press goes to the menus while `cl_bypassMouseInput`
    /// lets the rest through (UI `vmMain` 14, 0x4000a170): an edit or a bind
    /// waiting for its key takes every key, else only the focused menu's
    /// `execKey`s.
    pub fn wants_key(&self, code: KeyCode) -> bool {
        if self.editing() || self.waiting_for_key() {
            return true;
        }
        let Some(m) = self.focused_menu() else {
            return false;
        };
        crate::console::keys::key_name(code)
            .map(crate::console::keys::number)
            .and_then(|n| exec_key(&self.menus[m], n))
            .is_some()
    }

    /// `Menus_CloseAll` (0x40010660): every open menu closes in load order,
    /// its `onClose` running.
    fn close_every_menu(&mut self, shell: &Shell, out: &mut Vec<UiEffect>) {
        for m in 0..self.menus.len() {
            if self.on_stack(m) {
                self.close_at(m, shell, out);
            }
        }
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
            game_type: self
                .join_game_type()
                .checked_sub(1)
                .map(|i| self.game_types[i].script.clone()),
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
        self.repeat_arrow(now);
    }

    /// A held scroll bar arrow steps its list on `Scroll_ListBox_AutoFunc`'s
    /// clock. The capture freezes the item's part flags, so it keeps
    /// stepping wherever the cursor goes until the button comes up.
    fn repeat_arrow(&mut self, now: Instant) {
        let Some(((m, i), part, mut repeat)) = self.arrow_hold else {
            return;
        };
        let due = repeat.due(now);
        self.arrow_hold = Some(((m, i), part, repeat));
        let Some(item) = self.menus.get(m).and_then(|menu| menu.items.get(i)) else {
            return;
        };
        if let Some((len, first)) = self.list_scroll(item).filter(|_| due) {
            let feeder = item.feeder;
            let top = listbox::click(item, len, first, part);
            self.set_list_top(feeder, top);
        }
    }

    /// `ui_joinGametype` as an index into "All" and the game types; out of
    /// range reads as All, as `UI_DrawJoinGameType` resets it (0x400088b0).
    fn join_game_type(&self) -> usize {
        let v = self.ui_cvar("ui_joinGametype");
        v.trim()
            .parse::<f32>()
            .ok()
            .map(|f| f as i32)
            .and_then(|i| usize::try_from(i).ok())
            .filter(|&i| i <= self.game_types.len())
            .unwrap_or(0)
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

    /// Whether the loaded menus hold `name`, as `Menus_ActivateByName`
    /// looks it up (0x400134b0).
    pub fn has_menu(&self, name: &str) -> bool {
        self.find(name).is_some()
    }

    fn find(&self, name: &str) -> Option<usize> {
        self.menus
            .iter()
            .position(|m| m.name.eq_ignore_ascii_case(name))
    }

    /// `Menus_Activate` (0x40013400): the menu goes on top of the stack and
    /// takes the focus, its `onOpen` runs, then `Display_MouseMove` at the
    /// cursor.
    fn open_menu(&mut self, name: &str, shell: &Shell, out: &mut Vec<UiEffect>) {
        let Some(m) = self.find(name).filter(|_| !unsupported(name)) else {
            crate::console::log::print(&format!("The {name} menu is not in vcod yet."));
            return;
        };
        self.down = false;
        self.leave_stack(m);
        self.open.push(m);
        self.focus_menu = Some(m);
        self.hover = None;
        let script = self.menus[m].on_open.clone();
        self.run(m, &script, shell, out);
        self.track_cursor(shell, out);
    }

    /// `Menus_Close` (0x40010600): `onClose` runs while the menu is still
    /// open (0x400105c0), then the menu leaves the stack (0x400104a0) and,
    /// when it still has the focus, the new top takes it.
    fn close_menu(&mut self, name: &str, shell: &Shell, out: &mut Vec<UiEffect>) {
        if let Some(m) = self.find(name) {
            self.close_at(m, shell, out);
        }
    }

    fn close_at(&mut self, m: usize, shell: &Shell, out: &mut Vec<UiEffect>) {
        // A `close` of itself inside its own `onClose` would recurse forever
        // in retail; vcod lets the inner one do nothing.
        if !self.on_stack(m) || self.closing.contains(&m) {
            return;
        }
        // `onClose` runs only for a visible menu (0x400105c0).
        if self.open.contains(&m) {
            self.closing.push(m);
            let script = self.menus[m].on_close.clone();
            self.run(m, &script, shell, out);
            self.closing.retain(|&c| c != m);
        }
        self.leave_stack(m);
        if self.focus_menu == Some(m) {
            self.focus_menu = self.stack_top();
        }
        self.hover = None;
        self.options.forget(m);
    }

    /// Whether `m` is on the stack, shown or not.
    fn on_stack(&self, m: usize) -> bool {
        self.open.contains(&m) || self.hidden.iter().any(|&(h, _)| h == m)
    }

    /// `m` off the stack (0x400104a0), shown or hidden.
    fn leave_stack(&mut self, m: usize) {
        self.hidden.retain(|&(h, _)| h != m);
        if let Some(i) = self.open.iter().position(|&o| o == m) {
            self.open.remove(i);
            for (_, below) in &mut self.hidden {
                if *below > i {
                    *below -= 1;
                }
            }
        }
    }

    /// The stack's top menu, which `Menus_Close` hands the focus to: a
    /// hidden one when nothing shown was opened over it.
    fn stack_top(&self) -> Option<usize> {
        match self.hidden.last() {
            Some(&(h, below)) if below == self.open.len() => Some(h),
            _ => self.open.last().copied(),
        }
    }

    /// The open menu that takes the keys (`Menu_GetFocused`, 0x40016970).
    fn focused_menu(&self) -> Option<usize> {
        self.focus_menu.filter(|m| self.open.contains(m))
    }

    /// `Item_RunScript` over `script`, with `menu` as the item's parent.
    fn run(&mut self, menu: usize, script: &[String], shell: &Shell, out: &mut Vec<UiEffect>) {
        for cmd in ui_menu::script_commands(script) {
            let arg = |i: usize| cmd.get(i).map(String::as_str).unwrap_or("");
            match cmd[0].to_ascii_lowercase().as_str() {
                "play" => out.push(UiEffect::Sound(arg(1).to_string())),
                "open" => self.open_menu(arg(1), shell, out),
                "close" => self.close_menu(arg(1), shell, out),
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
                "uiscript" if profiles::is_profile_script(arg(1)) => {
                    self.profile_script(arg(1), shell, out);
                }
                "uiscript" if menu_on_dvar(arg(1)).is_some() => {
                    self.menu_on_dvar(&cmd[1..], shell, out);
                }
                "uiscript" => self.ui_script(&cmd[1..], out),
                "scriptmenuresponse" => out.push(UiEffect::ScriptMenuResponse {
                    menu: self.menus[menu].name.clone(),
                    response: arg(1).to_string(),
                }),
                // `openForGameType <fmt>` / `closeForGameType <fmt>`
                // (0x40010770, 0x400107f0): the menu `fmt` names once its
                // `%s` holds the item's cvar.
                "openforgametype" | "closeforgametype" => {
                    let value = self
                        .script_cvar
                        .clone()
                        .map(|c| self.ui_cvar(&c))
                        .unwrap_or_default();
                    let name = fill(arg(1), &[&value]);
                    if cmd[0].eq_ignore_ascii_case("openforgametype") {
                        self.open_menu(&name, shell, out);
                    } else {
                        self.close_menu(&name, shell, out);
                    }
                }
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
                "ingameclose" if self.in_game => self.close_menu(arg(1), shell, out),
                _ => {}
            }
        }
    }

    /// CoD2's `openMenuOnDvar`, `openMenuOnDvarNot`, `closeMenuOnDvar` and
    /// `closeMenuOnDvarNot` `<dvar> <value> <menu>` (`UI_RunMenuScript`,
    /// Mac 0x1548e8): the menu opens or closes when the dvar's value equals
    /// `value` case folded, or for the `Not` pair when it does not. A dvar
    /// that does not exist does nothing (`cannot find dvar`), as does a
    /// missing argument.
    fn menu_on_dvar(&mut self, args: &[String], shell: &Shell, out: &mut Vec<UiEffect>) {
        let Some((open, equal)) = args.first().and_then(|a| menu_on_dvar(a)) else {
            return;
        };
        let [_, dvar, value, menu, ..] = args else {
            log::debug!("ui: {}: missing arguments", args[0]);
            return;
        };
        let Some(current) = self.cvar(dvar, shell) else {
            log::debug!("ui: {}: cannot find dvar {dvar}", args[0]);
            return;
        };
        if current.eq_ignore_ascii_case(value) == equal {
            if open {
                self.open_menu(menu, shell, out);
            } else {
                self.close_menu(menu, shell, out);
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
            // CoD2's `mutePlayer` (`UI_RunMenuScript`, Mac 0x1548e8): the
            // selected row's client to `CL_MutePlayer`.
            "muteplayer" => {
                if let Some(&c) = self.player_clients.get(self.player_row) {
                    out.push(UiEffect::MutePlayer(c));
                }
            }
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
            // 1.5's opens `joinpb_popmenu` on a `pb` server while
            // `cl_punkbuster` is 0 (ui_mp_x86.dll 1.5 0x4000b124), whose Yes
            // turns PunkBuster on. vcod runs no PunkBuster, so it connects
            // and shows the server's refusal (cod15-protocol-diff.md,
            // "PunkBuster").
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
                self.mod_top = 0;
            }
            "runmod" => {
                if let Some(m) = self.mod_selected.and_then(|i| self.mods.get(i)) {
                    out.push(UiEffect::RunMod(Some(m.dir.clone())));
                }
            }
            // The base game again; no stock menu calls it.
            "quake3" => out.push(UiEffect::RunMod(None)),
            "clearerror" => self.error.clear(),
            "loadarenas" => self.load_arenas(out),
            "startserver" => self.start_server(out),
            w @ ("votemap" | "votetypemap" | "votegame") => self.vote_script(w, out),
            "votekick" => self.vote_kick(out),
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

    /// A mouse move to window px on a `w` x `h` window, for a window that
    /// cannot capture the mouse: mapped through the menus' 4:3 box, clamped
    /// to its edges.
    pub fn mouse_move(&mut self, x: f32, y: f32, w: f32, h: f32, shell: &Shell) -> Vec<UiEffect> {
        self.set_screen(w, h);
        self.cursor = Virtual::new((w, h)).grid_point(x, y);
        self.cursor_moved(shell)
    }

    /// UI `vmMain` 4 (0x4000d790): the mouse's raw motion, unscaled, moves
    /// the cursor, clamped to 0..=640 and 0..=480
    /// (docs/research/cod11-front-end.md section 13).
    pub fn mouse_delta(&mut self, dx: f32, dy: f32, shell: &Shell) -> Vec<UiEffect> {
        self.cursor = [
            (self.cursor[0] + dx).clamp(0.0, 640.0),
            (self.cursor[1] + dy).clamp(0.0, 480.0),
        ];
        self.cursor_moved(shell)
    }

    fn cursor_moved(&mut self, shell: &Shell) -> Vec<UiEffect> {
        let mut out = Vec::new();
        self.options.new_input();
        self.drag_to(shell, &mut out);
        if let Some((m, i)) = self.thumb_drag {
            let item = &self.menus[m].items[i];
            if let Some((len, _)) = self.list_scroll(item) {
                let top = listbox::drag(item, len, self.cursor[1]);
                let feeder = item.feeder;
                self.set_list_top(feeder, top);
            }
        }
        // `Menu_HandleMouseMove` (0x40016b20) does nothing during an edit.
        if !self.editing() {
            self.track_cursor(shell, &mut out);
        }
        out
    }

    /// `Display_MouseMove` (0x40018b40): a popup with the focus takes the
    /// move alone; otherwise each open menu from the top down runs
    /// [`Self::menu_mouse_move`] until an item of one takes the focus.
    fn track_cursor(&mut self, shell: &Shell, out: &mut Vec<UiEffect>) {
        let walk: Vec<usize> = match self.focused_menu() {
            Some(f) if self.menus[f].popup => vec![f],
            _ => self.open.iter().rev().copied().collect(),
        };
        for m in walk {
            // A script can close the menu.
            if self.open.contains(&m) && self.menu_mouse_move(m, shell, out) {
                break;
            }
        }
        self.hover = self.hit(shell);
    }

    /// `Menu_HandleMouseMove` (0x40016b20) for `menu` at the cursor.
    fn menu_mouse_move(&mut self, menu: usize, shell: &Shell, out: &mut Vec<UiEffect>) -> bool {
        let at = self.cursor;
        self.menu_mouse_move_at(menu, at, shell, out)
    }

    /// `Menu_HandleMouseMove` (0x40016b20) for `menu` at `at`: two passes
    /// over the shown items from the last. Every item off the point that has
    /// the mouse runs its `mouseExit`; in the second pass every item on it
    /// without the mouse runs its `mouseEnter`, and each one tries
    /// [`Self::set_focus`] until one takes it, so of two overlapping items
    /// the one without the focus takes it. With no item taking the focus, a
    /// focused item off the point loses it. True when an item took the
    /// focus. Nothing runs during an edit, a capture or a bind waiting for
    /// its key.
    fn menu_mouse_move_at(
        &mut self,
        menu: usize,
        at: [f32; 2],
        shell: &Shell,
        out: &mut Vec<UiEffect>,
    ) -> bool {
        if self.editing() || self.capture_active() || self.waiting_for_key() {
            return false;
        }
        let mut took = false;
        for pass in 0..2 {
            for i in (0..self.menus[menu].items.len()).rev() {
                if !self.shown(menu, i, shell) {
                    continue;
                }
                let item = &self.menus[menu].items[i];
                let (decoration, inside) = (item.decoration, contains(item.rect, at));
                if !inside {
                    if self.over.remove(&(menu, i)) {
                        let s = self.menus[menu].items[i].mouse_exit.clone();
                        self.run(menu, &s, shell, out);
                    }
                } else if pass == 1 {
                    if self.over.insert((menu, i)) {
                        let s = self.menus[menu].items[i].mouse_enter.clone();
                        self.run(menu, &s, shell, out);
                    }
                    if !took && !decoration {
                        took = self.set_focus(menu, i, at, shell, out);
                    }
                }
            }
        }
        if took {
            return true;
        }
        // The walk notes the focused item among the shown ones only, so a
        // focused item a cvar test now hides keeps the focus.
        if let Some(&f) = self.focus.get(&menu)
            && self.shown(menu, f, shell)
            && !contains(self.menus[menu].items[f].rect, at)
        {
            self.clear_focus(menu, shell, out);
        }
        false
    }

    /// `Menu_ClearFocus` (0x4000fe70): every item of `menu` loses the focus
    /// and each one's `leaveFocus` runs, focused or not. Returns the item
    /// that had it.
    fn clear_focus(
        &mut self,
        menu: usize,
        shell: &Shell,
        out: &mut Vec<UiEffect>,
    ) -> Option<usize> {
        let old = self.focus.remove(&menu);
        for i in 0..self.menus[menu].items.len() {
            let s = self.menus[menu].items[i].leave_focus.clone();
            self.run(menu, &s, shell, out);
        }
        old
    }

    /// `Item_SetFocus` (0x40011410) for a shown item that is no decoration,
    /// with the mouse at `at`: false when it has the focus already. A text
    /// item takes it only with the mouse on its text ([`Self::text_rect`])
    /// and otherwise hands it back to the old item, whose `onFocus` runs
    /// again; any other item runs its `onFocus`. Either one that takes it plays its `focusSound`, else
    /// `itemFocusSound`. The item becomes the menu's cursor item.
    fn set_focus(
        &mut self,
        menu: usize,
        i: usize,
        at: [f32; 2],
        shell: &Shell,
        out: &mut Vec<UiEffect>,
    ) -> bool {
        if self.focus.get(&menu) == Some(&i) {
            return false;
        }
        let old = self.clear_focus(menu, shell, out);
        self.cursor_item.insert(menu, i);
        let item = &self.menus[menu].items[i];
        if item.kind == ITEM_TYPE_TEXT && !self.on_text(menu, i, at) {
            if let Some(o) = old {
                self.focus.insert(menu, o);
                let s = self.menus[menu].items[o].on_focus.clone();
                self.run(menu, &s, shell, out);
            }
            return true;
        }
        let (text, sound) = (item.kind == ITEM_TYPE_TEXT, item.focus_sound.clone());
        let script = item.on_focus.clone();
        self.focus.insert(menu, i);
        if !text {
            self.run(menu, &script, shell, out);
        }
        let global = self.assets.item_focus_sound.clone();
        if sound.is_some() || global.is_some() {
            out.push(UiEffect::FocusSound {
                item: sound,
                global,
            });
        }
        true
    }

    /// The item of `menu` under the cursor that takes the mouse: the last
    /// shown one that is no decoration, as `Menu_HandleMouseMove` walks the
    /// items from the last.
    fn item_at(&self, menu: usize, shell: &Shell) -> Option<usize> {
        (0..self.menus[menu].items.len()).rev().find(|&i| {
            let item = &self.menus[menu].items[i];
            !item.decoration && self.shown(menu, i, shell) && contains(item.rect, self.cursor)
        })
    }

    /// The menu a click at the cursor goes to. `Menu_HandleKey`
    /// (0x400136b0) keeps it in the focused menu when that is a popup or
    /// the cursor is on its rect; otherwise `Menus_HandleOOBClick`
    /// (0x40013500) hands it to the first menu from the top for which
    /// `Menu_OverActiveItem` (0x40018d70) holds: its rect and a visible
    /// item that is no decoration hold the cursor. `None` drops the click.
    fn click_menu(&self) -> Option<usize> {
        let f = self.focused_menu()?;
        if self.menus[f].popup || contains(self.menus[f].rect, self.cursor) {
            return Some(f);
        }
        self.oob_menu()
    }

    /// `Menus_HandleOOBClick`'s walk: the first open menu from the top whose
    /// rect and a visible item that is no decoration hold the cursor. The
    /// item's cvar test is not read (`Menu_OverActiveItem`, 0x40018d70).
    fn oob_menu(&self) -> Option<usize> {
        self.open.iter().rev().copied().find(|&m| {
            contains(self.menus[m].rect, self.cursor)
                && (self.menus[m].items.iter().zip(&self.state[m]))
                    .any(|(i, st)| st.visible && !i.decoration && contains(i.rect, self.cursor))
        })
    }

    /// The item a click at the cursor goes to, as (menu, item).
    fn hit(&self, shell: &Shell) -> Option<(usize, usize)> {
        let m = self.click_menu()?;
        self.item_at(m, shell).map(|i| (m, i))
    }

    /// A left click at the last mouse position.
    #[cfg(test)]
    pub fn click(&mut self, now: Instant, shell: &Shell) -> Vec<UiEffect> {
        self.mouse_button(K_MOUSE1, now, shell)
    }

    /// A right click: `Menu_HandleKey` takes `K_MOUSE2` as it takes
    /// `K_MOUSE1` (ui_mp_x86.dll 0x40013a9a), except that the game type and
    /// source owner draws step back (0x40009a60, 0x40009b10, 0x40009b90).
    #[cfg(test)]
    pub fn right_click(&mut self, now: Instant, shell: &Shell) -> Vec<UiEffect> {
        self.mouse_button(K_MOUSE2, now, shell)
    }

    /// A mouse button press by retail key number, `K_MOUSE1` (200) to
    /// `K_MOUSE5` (204), as `Menu_HandleKey` (0x400136b0) takes it. The
    /// first three end an edit and go where [`Self::click_menu`] sends
    /// them, closing first a focused `outOfBoundsClick` menu the cursor is
    /// off; then [`Self::press`].
    pub fn mouse_button(&mut self, key: u32, now: Instant, shell: &Shell) -> Vec<UiEffect> {
        let mut out = Vec::new();
        if self.unfocused_key() {
            return out;
        }
        self.options.new_input();
        let clicks = (K_MOUSE1..=K_MOUSE3).contains(&key);
        // The edit field keeps every other button.
        if self.editing() && !clicks {
            return out;
        }
        // A click ends an edit, runs the mouse move the edit held back
        // (`Display_MouseMove`, 0x400136b0), then acts as a click.
        let edited = self.options.editing.take().is_some();
        // A press during a capture only ends it (`Item_HandleKey`,
        // 0x40013040); the item still takes the click.
        let captured = self.capture_active();
        self.release();
        self.settle_focus(shell, &mut out);
        if edited {
            self.track_cursor(shell, &mut out);
        }
        let top = if clicks {
            if let Some(f) = self.focused_menu()
                && self.menus[f].out_of_bounds_click
                && !self.menus[f].popup
                && !contains(self.menus[f].rect, self.cursor)
            {
                // `Menus_HandleOOBClick` (0x40013500): `onClose` runs and the
                // menu loses its visible and focus flags where it stands on
                // the stack.
                let s = self.menus[f].on_close.clone();
                self.run(f, &s, shell, &mut out);
                if let Some(i) = self.open.iter().position(|&o| o == f) {
                    self.leave_stack(f);
                    let at = self.hidden.partition_point(|&(_, below)| below <= i);
                    self.hidden.insert(at, (f, i));
                }
                self.focus_menu = None;
                self.options.forget(f);
                self.hover = None;
            }
            let m = match self.focused_menu() {
                Some(_) => self.click_menu(),
                None => self.oob_menu(),
            };
            let Some(m) = m else {
                return out;
            };
            if Some(m) != self.focused_menu() {
                // `Menus_HandleOOBClick` (0x40013500): the menu takes the
                // focus where it stands in the stack, `Display_MouseMove`
                // and its own `Menu_HandleMouseMove` run, then the press.
                self.focus_menu = Some(m);
                self.track_cursor(shell, &mut out);
                self.menu_mouse_move(m, shell, &mut out);
            }
            m
        } else {
            let Some(m) = self.focused_menu() else {
                return out;
            };
            m
        };
        self.hover = self.hit(shell);
        self.press(top, key, now, captured, shell, &mut out);
        out
    }

    /// Whether `Item_HandleKey` (0x40013040) takes `key` for an item of
    /// `kind` that has the focus: list boxes the first two buttons, owner
    /// draws those and Enter, sliders, yes/no and multi items the first
    /// three buttons and Enter (each tests the cursor itself), and a bind
    /// item every key (0x40015410 answers 1 throughout).
    fn item_takes(kind: i32, key: u32) -> bool {
        match kind {
            ITEM_TYPE_LISTBOX => matches!(key, K_MOUSE1 | K_MOUSE2),
            ITEM_TYPE_OWNERDRAW => matches!(key, K_MOUSE1 | K_MOUSE2 | K_ENTER | K_KP_ENTER),
            ITEM_TYPE_SLIDER | ITEM_TYPE_YESNO | ITEM_TYPE_MULTI | ITEM_TYPE_DVARENUM => {
                matches!(key, K_MOUSE1 | K_MOUSE2 | K_MOUSE3 | K_ENTER)
            }
            ITEM_TYPE_BIND => true,
            _ => false,
        }
    }

    /// `Menu_HandleKey` (0x400136b0) after its mouse pass, for a press of
    /// `key` in `top`. It acts on the menu's focused item, not on the item
    /// under the cursor: that item's type handler takes the key when
    /// [`Self::item_takes`] says so and the item's `action` follows; else
    /// the menu's `execKey` for the key runs. Else the first two buttons
    /// act on the focused item when the cursor is on it, and Enter, keypad
    /// Enter and the middle button act on it wherever the cursor is: an
    /// edit field starts its edit, anything else runs its `action`. The
    /// fourth and fifth buttons reach only `execKey`.
    fn press(
        &mut self,
        top: usize,
        key: u32,
        now: Instant,
        captured: bool,
        shell: &Shell,
        out: &mut Vec<UiEffect>,
    ) {
        let focused = self.focus.get(&top).copied();
        let handled = focused.is_some_and(|i| Self::item_takes(self.menus[top].items[i].kind, key));
        if !handled && let Some(script) = exec_key(&self.menus[top], key) {
            self.run(top, &script, shell, out);
            return;
        }
        let Some(i) = focused else {
            return;
        };
        let item = self.menus[top].items[i].clone();
        // A text item's click tests its text (`Item_CorrectedTextRect`,
        // 0x40013630), any other item's its rect.
        let inside = if item.kind == ITEM_TYPE_TEXT {
            self.on_text(top, i, self.cursor)
        } else {
            contains(item.rect, self.cursor)
        };
        let click = matches!(key, K_MOUSE1 | K_MOUSE2);
        let back = key == K_MOUSE2;
        if matches!(key, K_MOUSE4 | K_MOUSE5) || (click && !inside) {
            return;
        }
        if !click && !handled {
            // The Enter case (0x400138b6): no cursor test.
            if matches!(item.kind, ITEM_TYPE_EDITFIELD | ITEM_TYPE_NUMERICFIELD) {
                self.options.editing = Some((top, i));
            } else {
                self.item_action(top, &item, shell, out);
            }
            return;
        }
        if click
            && let Some((len, first)) = self.list_scroll(&item)
            && let Some(part) = listbox::part_at(&item, len, first, self.cursor)
        {
            // `Item_StartCapture` (0x40012dd0): an arrow repeats, the
            // thumb drags; the page areas step once.
            match part {
                _ if captured => {}
                listbox::Part::Thumb => self.thumb_drag = Some((top, i)),
                listbox::Part::Up | listbox::Part::Down => {
                    self.arrow_hold = Some(((top, i), part, listbox::Repeat::new(now)));
                }
                listbox::Part::PageUp | listbox::Part::PageDown => {}
            }
            self.set_list_top(item.feeder, listbox::click(&item, len, first, part));
            return;
        }
        if item.kind == ITEM_TYPE_LISTBOX && item.feeder == Some(FEEDER_MODS) {
            let row = ((self.cursor[1] - item.rect[1] - 1.0) / item.element_height.max(1.0)).floor()
                as usize
                + self.mod_top;
            if row < self.mods.len() {
                let double = self
                    .last_mod_click
                    .is_some_and(|(t, r)| r == row && now - t < DOUBLE_CLICK);
                self.mod_selected = Some(row);
                self.last_mod_click = Some((now, row));
                self.run(top, &item.action, shell, out);
                if double {
                    self.last_mod_click = None;
                    self.run(top, &item.double_click, shell, out);
                }
            }
            return;
        }
        if item.kind == ITEM_TYPE_LISTBOX && item.feeder == Some(FEEDER_ALLMAPS) {
            self.map_click(&item);
            return;
        }
        if item.kind == ITEM_TYPE_LISTBOX
            && matches!(item.feeder, Some(FEEDER_PLAYER_LIST | FEEDER_MUTELIST))
        {
            self.player_click(&item);
            return;
        }
        if item.kind == ITEM_TYPE_LISTBOX && item.feeder == Some(FEEDER_PLAYER_PROFILES) {
            let double = self
                .last_profile_click
                .is_some_and(|t| now - t < DOUBLE_CLICK);
            let again = self.profile_click(&item, double);
            self.last_profile_click = Some(now);
            if let Some(name) = self.profile_selected() {
                self.set_cvar("ui_playerProfileSelected", &name, out);
            }
            if again {
                self.last_profile_click = None;
                self.run(top, &item.double_click, shell, out);
            }
            return;
        }
        if item.kind == ITEM_TYPE_LISTBOX && item.feeder == Some(FEEDER_SERVERS) {
            let row = ((self.cursor[1] - item.rect[1] - 1.0) / item.element_height.max(1.0)).floor()
                as usize
                + self.list_top;
            let rows = self.browser.rows();
            let Some(addr) = rows.get(row).map(|s| s.addr) else {
                return;
            };
            let double = self
                .last_click
                .is_some_and(|(t, a)| a == addr && now - t < DOUBLE_CLICK);
            self.selected = Some(addr);
            self.last_click = Some((now, addr));
            if double {
                self.last_click = None;
                self.run(top, &item.double_click, shell, out);
            }
            return;
        }
        if let Some(name) = self.refused(&item.action) {
            crate::console::log::print(&format!("The {name} menu is not in vcod yet."));
            return;
        }
        if item.ownerdraw == UI_NETSOURCE {
            // `UI_NetSource_HandleKey`, then the item's action.
            let next = if back {
                self.browser.source.prev()
            } else {
                self.browser.source.next()
            };
            self.browser.set_source(next);
            self.selected = None;
            self.list_top = 0;
            self.set_cvar("ui_netSource", &(next as i32).to_string(), out);
        }
        if item.ownerdraw == UI_NETGAMETYPE {
            self.step_gametype(back, out);
        }
        if item.ownerdraw == UI_JOINGAMETYPE {
            // `UI_JoinGameType_HandleKey` (0x40009b10): the next game type
            // (the previous on a right click), wrapping; the list is
            // rebuilt, nobody is asked again.
            let n = self.game_types.len() + 1;
            let next = (self.join_game_type() + if back { n - 1 } else { 1 }) % n;
            self.set_cvar("ui_joinGametype", &next.to_string(), out);
        }
        // The option handlers test the cursor; Enter off a yes/no, multi,
        // slider or bind item changes nothing but still runs the action.
        let ran = if inside && item.kind == ITEM_TYPE_DVARENUM {
            self.enum_step(&item, back, shell, out);
            true
        } else if inside {
            self.option_click(top, i, shell, out)
        } else {
            true
        };
        if captured {
            self.release();
        }
        if ran {
            self.item_action(top, &item, shell, out);
        }
    }

    /// `Item_Action` (0x40013140): the item's `action`, with its cvar for
    /// `openForGameType`.
    fn item_action(&mut self, top: usize, item: &UiItem, shell: &Shell, out: &mut Vec<UiEffect>) {
        if let Some(name) = self.refused(&item.action) {
            crate::console::log::print(&format!("The {name} menu is not in vcod yet."));
            return;
        }
        self.script_cvar = item.cvar.clone();
        self.run(top, &item.action, shell, out);
        self.script_cvar = None;
    }

    /// A key press while a menu is up. `Menu_HandleKey` (0x400136b0) first
    /// takes back a focus step; then the list under the cursor takes its
    /// keys, the focused menu's `execKey` for the key runs, Esc runs its
    /// `onEsc`, Enter joins the selected server anywhere in the browser
    /// (vcod's own) unless the focused item's handler takes it, else acts
    /// on the focused item ([`Self::press`]), and Tab and the arrows no item
    /// took step the focus.
    /// False when the key is not the menu's.
    pub fn key(&mut self, code: KeyCode, shell: &Shell) -> (bool, Vec<UiEffect>) {
        let mut out = Vec::new();
        if self.unfocused_key() {
            return (true, out);
        }
        self.options.new_input();
        self.settle_focus(shell, &mut out);
        let Some(top) = self.focused_menu() else {
            return (false, out);
        };
        // `_UI_KeyEvent` (0x4000d72d): an Esc with no fullscreen menu open
        // closes them all, ahead of `onEsc` and the `execKey`s.
        if code == KeyCode::Escape && !self.fullscreen() {
            self.close_every_menu(shell, &mut out);
            return (true, out);
        }
        let has_list = self.menus[top]
            .items
            .iter()
            .any(|i| i.feeder == Some(FEEDER_SERVERS));
        if self.map_list_key(code)
            || self.mod_list_key(code)
            || self.player_list_key(code)
            || self.server_list_key(code)
        {
            return (true, out);
        }
        let number = crate::console::keys::key_name(code).map(crate::console::keys::number);
        if let Some(script) = number.and_then(|n| exec_key(&self.menus[top], n)) {
            self.run(top, &script, shell, &mut out);
            return (true, out);
        }
        match code {
            KeyCode::Escape => {
                let s = self.menus[top].on_esc.clone();
                self.run(top, &s, shell, &mut out);
            }
            KeyCode::Enter | KeyCode::NumpadEnter => {
                let n = if code == KeyCode::Enter {
                    K_ENTER
                } else {
                    K_KP_ENTER
                };
                let takes = self
                    .focus
                    .get(&top)
                    .is_some_and(|&i| Self::item_takes(self.menus[top].items[i].kind, n));
                if has_list && self.selected.is_some() && !takes {
                    self.ui_script(&["JoinServer".to_string()], &mut out);
                } else {
                    self.press(top, n, Instant::now(), false, shell, &mut out);
                }
            }
            // The switch at 0x400138b6: Tab, down, right and keypad down go
            // to `Menu_SetNextCursorItem`, up, left and keypad up to `_Prev`.
            KeyCode::Tab | KeyCode::ArrowDown | KeyCode::ArrowRight | KeyCode::Numpad2 => {
                self.step_focus(top, true, shell, &mut out);
            }
            KeyCode::ArrowUp | KeyCode::ArrowLeft | KeyCode::Numpad8 => {
                self.step_focus(top, false, shell, &mut out);
            }
            _ => return (false, out),
        }
        (true, out)
    }

    /// A key or button coming up while a menu is up: it ends a capture and,
    /// as `Menu_HandleKey` runs `Menu_HandleMouseMove` at the cursor for a
    /// release too, takes back a focus step.
    pub fn key_up(&mut self, shell: &Shell) -> Vec<UiEffect> {
        let mut out = Vec::new();
        self.release();
        if self.unfocused_key() {
            return out;
        }
        self.settle_focus(shell, &mut out);
        out
    }

    /// A key repeat. `CL_KeyEvent` (CoDMP.exe 0x40dc30) keeps repeats from
    /// the UI; only `CL_CharEvent` (0x40e200), which has no repeat test,
    /// reaches an edit field, so typed characters and Backspace repeat
    /// there and nothing else does.
    pub fn key_repeat(
        &mut self,
        code: KeyCode,
        text: Option<&str>,
        shell: &Shell,
    ) -> Vec<UiEffect> {
        let typed = text.is_some_and(|t| t.chars().any(|c| !c.is_control()));
        if self.editing() && (code == KeyCode::Backspace || typed) {
            return self.edit_key(code, text, shell);
        }
        Vec::new()
    }

    /// `Item_ListBox_HandleKey` (0x40011da0) for the server list under the
    /// cursor, which the mouse pass ahead of it has focused: the arrows,
    /// Home, End and page keys move the selection. With nothing selected an
    /// arrow picks the first row.
    fn server_list_key(&mut self, code: KeyCode) -> bool {
        let Some((m, i)) = self.hover else {
            return false;
        };
        let item = &self.menus[m].items[i];
        if item.kind != ITEM_TYPE_LISTBOX || item.feeder != Some(FEEDER_SERVERS) {
            return false;
        }
        let rows: Vec<SocketAddrV4> = self.browser.rows().iter().map(|s| s.addr).collect();
        let at = self
            .selected
            .and_then(|a| rows.iter().position(|&r| r == a));
        let Some((first, row)) =
            listbox::key(item, rows.len(), self.list_top, at.unwrap_or(0), code)
        else {
            return false;
        };
        let row = match (at, code) {
            (None, KeyCode::ArrowUp | KeyCode::ArrowDown | KeyCode::Numpad8 | KeyCode::Numpad2) => {
                0
            }
            _ => row,
        };
        self.selected = rows.get(row).copied();
        self.list_top = first.min(row);
        self.keep_visible(row, m);
        true
    }

    /// A scrolled list's row count and first row drawn: the map list and
    /// the server list.
    fn list_scroll(&self, item: &UiItem) -> Option<(usize, usize)> {
        if item.kind != ITEM_TYPE_LISTBOX {
            return None;
        }
        match item.feeder {
            Some(FEEDER_ALLMAPS) => Some((self.active_maps().len(), self.map_top)),
            Some(FEEDER_SERVERS) => Some((self.browser.rows().len(), self.list_top)),
            Some(FEEDER_MODS) => Some((self.mods.len(), self.mod_top)),
            Some(FEEDER_PLAYER_LIST | FEEDER_MUTELIST) => {
                Some((self.players.len(), self.player_top))
            }
            Some(FEEDER_PLAYER_PROFILES) => Some((self.profiles.len(), self.profiles.top)),
            _ => None,
        }
    }

    fn set_list_top(&mut self, feeder: Option<i32>, top: usize) {
        match feeder {
            Some(FEEDER_ALLMAPS) => self.map_top = top,
            Some(FEEDER_SERVERS) => self.list_top = top,
            Some(FEEDER_MODS) => self.mod_top = top,
            Some(FEEDER_PLAYER_LIST | FEEDER_MUTELIST) => self.player_top = top,
            Some(FEEDER_PLAYER_PROFILES) => self.profiles.top = top,
            _ => {}
        }
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

    /// The wheel in a menu with the server list: half a page.
    fn page(&mut self, up: bool) {
        let Some(top) = self.focused_menu() else {
            return;
        };
        let n = self.list_rows_visible(top);
        let len = self.browser.rows().len();
        self.list_top = if up {
            self.list_top.saturating_sub(n / 2)
        } else {
            (self.list_top + n / 2).min(len.saturating_sub(n))
        };
    }

    /// A wheel notch. Over the map or Mods list it scrolls that list a row,
    /// and in a menu with the server list it pages that list; retail
    /// scrolls no list, and vcod keeps the scroll on purpose. Elsewhere, as
    /// in retail: no item's key handler takes the wheel, so the menu's
    /// `execKey` for `MWHEELDOWN` (205) or `MWHEELUP` (206) runs, else it
    /// steps the focus as Tab does, and its release follows in the same
    /// frame (docs/research/cod11-front-end.md section 19, "The wheel"). An
    /// edit field keeps it.
    pub fn wheel(&mut self, down: bool, shell: &Shell) -> Vec<UiEffect> {
        let mut out = Vec::new();
        self.options.new_input();
        if self.editing() {
            return out;
        }
        self.release();
        self.settle_focus(shell, &mut out);
        let Some(top) = self.focused_menu() else {
            return out;
        };
        if self.wheel_scroll(top, !down) {
            return out;
        }
        let key = if down { K_MWHEELDOWN } else { K_MWHEELUP };
        if let Some(script) = exec_key(&self.menus[top], key) {
            self.run(top, &script, shell, &mut out);
        } else {
            self.step_focus(top, down, shell, &mut out);
        }
        self.settle_focus(shell, &mut out);
        out
    }

    /// `Menu_SetNextCursorItem` (0x40013240, `down`) or `_Prev`
    /// (0x40013160): the focus moves to the next item that takes it and
    /// `Menu_HandleMouseMove` runs at that item's corner, which leaves the
    /// hovered items and enters the ones there. The next key event's mouse
    /// pass at the cursor ([`Self::settle_focus`]) undoes it.
    fn step_focus(&mut self, top: usize, down: bool, shell: &Shell, out: &mut Vec<UiEffect>) {
        let Some(target) = self.next_focus(top, down, shell) else {
            return;
        };
        let cursor = self.cursor;
        self.set_focus(top, target, cursor, shell, out);
        let r = self.menus[top].items[target].rect;
        self.menu_mouse_move_at(top, [r[0] + 1.0, r[1] + 1.0], shell, out);
    }

    /// The item `Menu_SetNextCursorItem` (`down`) or `_Prev` stops on.
    fn next_focus(&self, top: usize, down: bool, shell: &Shell) -> Option<usize> {
        let n = self.menus[top].items.len();
        let from = self.cursor_item.get(&top).copied();
        // Both wrap once; with no cursor item they start one in from the
        // end they walk from.
        let order: Vec<usize> = match (down, from) {
            (true, Some(c)) => (c + 1..n).chain(0..c).collect(),
            (true, None) => (1..n).collect(),
            (false, Some(c)) => (0..c).rev().chain((c + 1..n).rev()).collect(),
            (false, None) => (0..n.saturating_sub(1)).rev().collect(),
        };
        let focused = self.focus.get(&top).copied();
        // `Item_SetFocus` (0x40011410) refuses a decoration, a hidden item
        // and the item that has the focus.
        order.into_iter().find(|&i| {
            Some(i) != focused && !self.menus[top].items[i].decoration && self.shown(top, i, shell)
        })
    }

    /// `Item_TextField_HandleKey` (0x40012690) on Tab, down or up: the
    /// focus steps as [`Self::step_focus`] does but with no mouse pass,
    /// which an edit skips, and the edit moves along when the target is an
    /// edit field.
    pub(super) fn edit_step(&mut self, down: bool, shell: &Shell, out: &mut Vec<UiEffect>) {
        let Some((m, _)) = self.options.editing else {
            return;
        };
        let Some(target) = self.next_focus(m, down, shell) else {
            return;
        };
        let cursor = self.cursor;
        self.set_focus(m, target, cursor, shell, out);
        if matches!(
            self.menus[m].items[target].kind,
            ITEM_TYPE_EDITFIELD | ITEM_TYPE_NUMERICFIELD
        ) {
            self.options.editing = Some((m, target));
        }
    }

    /// The next key event: `Menu_HandleKey` runs `Menu_HandleMouseMove`
    /// for the focused menu at the real cursor, which leaves what a focus
    /// step entered and enters the hovered items again; the hovered item
    /// takes the focus and the cursor item back.
    fn settle_focus(&mut self, shell: &Shell, out: &mut Vec<UiEffect>) {
        if let Some(m) = self.focused_menu() {
            self.menu_mouse_move(m, shell, out);
        }
    }

    /// Whether `item` of `menu` is drawn in the focus colour: it has the
    /// focus bit (`Item_TextColor`, 0x40013df0), whether the mouse, a focus
    /// step or an edit's Tab put it there.
    fn focused(&self, menu: usize, item: usize) -> bool {
        self.focus.get(&menu) == Some(&item)
    }

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
    fn label(&self, item: &UiItem, loc: &Localized, shell: &Shell) -> String {
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
    fn text_rect(&self, menu: usize, item: usize) -> Option<[f32; 4]> {
        let [x, y, w, h] = *self.text_rects.borrow().get(&(menu, item))?;
        Some([x, y - h, w, h])
    }

    /// Whether `at` is on the text of `item` of `menu`.
    fn on_text(&self, menu: usize, item: usize, at: [f32; 2]) -> bool {
        self.text_rect(menu, item).is_some_and(|r| contains(r, at))
    }

    /// The wheel's list scroll; false when no list takes it.
    fn wheel_scroll(&mut self, top: usize, up: bool) -> bool {
        if let Some((m, i)) = self.hover.filter(|&(m, _)| m == top) {
            let item = &self.menus[m].items[i];
            if matches!(
                item.feeder,
                Some(FEEDER_ALLMAPS | FEEDER_MODS | FEEDER_PLAYER_LIST | FEEDER_MUTELIST)
            ) && let Some((len, cur)) = self.list_scroll(item)
            {
                let max = listbox::max_scroll(item, len);
                let next = if up {
                    cur.saturating_sub(1)
                } else {
                    (cur + 1).min(max)
                };
                let feeder = item.feeder;
                self.set_list_top(feeder, next);
                return true;
            }
        }
        if self.menus[top]
            .items
            .iter()
            .any(|i| i.feeder == Some(FEEDER_SERVERS))
        {
            self.page(up);
            return true;
        }
        false
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
        for &m in &self.open {
            let menu = &self.menus[m];
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
                let focused = self.focused(m, i);
                let color = if focused && !item.decoration && item.kind != ITEM_TYPE_LISTBOX {
                    menu.focus_color
                } else {
                    item.fore
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
                    UI_SERVERREFRESHDATE => Some(self.refresh_text(loc)),
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
                let label_w = p.width(&label, item.text_scale);
                let value_w = value
                    .as_ref()
                    .filter(|_| item.kind == ITEM_TYPE_OWNERDRAW)
                    .map_or(0.0, |v| p.width(v, item.text_scale));
                let (mut left, mut y) = Self::text_origin(item, label_w, value_w, label.is_empty());
                // Where the label's width ends, which a value follows.
                let mut label_end = label_w;
                if paints_text(item) {
                    // `Item_SetTextExtents` keeps a rect with a width, and
                    // the text draws where the kept rect says.
                    let mut rects = self.text_rects.borrow_mut();
                    let cvar_w = match item.cvar.as_ref() {
                        Some(c) if takes_cvar_width(item) => {
                            p.width(&self.cvar(c, shell).unwrap_or_default(), item.text_scale)
                        }
                        _ => 0.0,
                    };
                    let widths = TextWidths {
                        label: label_w,
                        value: value_w,
                        cvar: cvar_w,
                        height: p.height(item.text_scale),
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
                        p.text(&line, x, ly, item.text_scale, color, &mut out);
                    }
                    continue;
                }
                if !label.is_empty() {
                    p.text(&label, left, y, item.text_scale, color, &mut out);
                }
                if item.kind == ITEM_TYPE_SLIDER {
                    let end = (!label.is_empty()).then_some(left + label_end);
                    self.slider_quads(&p, (m, i), item, end, color, shell, &mut out);
                }
                if let Some(v) = value {
                    let gap = if label.is_empty() { 0.0 } else { 8.0 };
                    p.text(
                        &v,
                        left + label_end + gap,
                        y,
                        item.text_scale,
                        color,
                        &mut out,
                    );
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
            .and_then(|a| self.browser.rows().into_iter().find(|s| s.addr == a))
            .and_then(|s| s.info.as_ref())
            .map(|i| i.mapname.clone())
            .unwrap_or_default();
        self.levelshot(&map)
    }

    /// `levelshots/<map>`, or `menu/art/unknownmap` when there is none.
    fn levelshot(&self, map: &str) -> String {
        let map = map.to_ascii_lowercase();
        if self.levelshots.contains(&map) {
            format!("levelshots/{map}")
        } else {
            "menu/art/unknownmap".into()
        }
    }

    /// `UI_SERVERREFRESHDATE`'s line (ui_mp_x86.dll 0x40008fc5): the
    /// refresh's progress with the source's list length, else
    /// `EXE_REFRESHTIME` with the date the source's last refresh started.
    fn refresh_text(&self, loc: &Localized) -> String {
        let (total, _) = self.browser.counts();
        match self.browser.status {
            Status::Idle | Status::Done | Status::Failed => fill(
                &loc.translate("@EXE_REFRESHTIME"),
                &[&self.browser.last_refresh[self.browser.source as usize]
                    .map_or_else(String::new, |t| refresh_date(t, loc))],
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

    /// A key with the cursor over the Mods list, as over the map list
    /// (`Item_ListBox_HandleKey`): it moves the selection, which
    /// `UI_FeederSelection` takes without running the item's action.
    fn mod_list_key(&mut self, code: KeyCode) -> bool {
        let Some((m, i)) = self.hover else {
            return false;
        };
        let item = &self.menus[m].items[i];
        if item.kind != ITEM_TYPE_LISTBOX || item.feeder != Some(FEEDER_MODS) {
            return false;
        }
        let sel = self.mod_selected.unwrap_or(0);
        match listbox::key(item, self.mods.len(), self.mod_top, sel, code) {
            Some((top, row)) => {
                (self.mod_top, self.mod_selected) = (top, Some(row));
                true
            }
            None => false,
        }
    }

    /// `Item_ListBox_Paint` for `FEEDER_MODS`: each mod's description, or
    /// its directory when it has none (ui_mp_x86.dll 0x4000caa9).
    fn mod_rows(&self, p: &Painter, item: &UiItem, out: &mut Vec<HudQuad>) {
        let [x, y0, w, _] = item.rect;
        let (x, mut y) = (x + 1.0, y0 + 1.0);
        let eh = item.element_height.max(1.0);
        for (i, m) in self
            .mods
            .iter()
            .enumerate()
            .skip(self.mod_top)
            .take(visible_rows(item))
        {
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

    /// `Item_ListBox_Paint` for `FEEDER_SERVERS`. A cell's icon is a square
    /// of the column's width from half a row down, and takes no text align
    /// (ui_mp_x86.dll 0x400159b0).
    /// A server list cell ([`browser::column_cell`]). CoD2's map column
    /// (`UI_FeederItemText` column 3, Mac 0x152e8a) prints the `longname`
    /// of the `.arena` entry whose `map` is the server's `mapname` in any
    /// case, the `mapname` when none is.
    fn list_cell(&self, s: &browser::Server, kind: browser::Column) -> browser::Cell {
        if kind == browser::Column::Map
            && self.browser.columns.len() == browser::COLUMNS_V118.len()
            && let Some(a) = s.info.as_ref().and_then(|i| {
                self.arenas
                    .iter()
                    .find(|a| a.map.eq_ignore_ascii_case(&i.mapname))
            })
        {
            return browser::Cell::Text(a.long_name.clone());
        }
        browser::column_cell(s, kind)
    }

    fn server_list(&self, p: &Painter, item: &UiItem, out: &mut Vec<HudQuad>) {
        let rows = self.browser.rows();
        let [x, y0, w, _] = item.rect;
        let (x, mut y) = (x + 1.0, y0 + 1.0);
        let eh = item.element_height.max(1.0);
        for s in rows.iter().skip(self.list_top).take(visible_rows(item)) {
            for (&kind, col) in self.browser.columns.iter().zip(&item.columns) {
                match self.list_cell(s, kind) {
                    browser::Cell::Image(shader) => p.quad(
                        [x + 4.0 + col.pos, y - 1.0 + eh * 0.5, col.width, col.width],
                        [1.0; 4],
                        shader,
                        out,
                    ),
                    browser::Cell::Text(text) => p.text(
                        &truncate_visible(&text, col.max_chars),
                        x + 4.0 + col.pos + item.text_align_x,
                        y + eh + item.text_align_y,
                        item.text_scale,
                        item.fore,
                        out,
                    ),
                }
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
            "shortversion" => Some(crate::version_tag()),
            "ui_multiplayer" | "cl_languagesavailable" => Some("1".into()),
            "ui_mousepitch" => Some(
                if shell.cvar_f32("m_pitch") < 0.0 {
                    "1"
                } else {
                    "0"
                }
                .into(),
            ),
            lower => match self.server_cvars.get(lower) {
                Some(v) => Some(v.clone()),
                None => shell.cvar(name).map(str::to_string),
            },
        }
    }
}

/// CoD2's `uiScript` menu-on-dvar names: whether it opens, and whether it
/// acts on an equal value.
fn menu_on_dvar(name: &str) -> Option<(bool, bool)> {
    match name.to_ascii_lowercase().as_str() {
        "openmenuondvar" => Some((true, true)),
        "openmenuondvarnot" => Some((true, false)),
        "closemenuondvar" => Some((false, true)),
        "closemenuondvarnot" => Some((false, false)),
        _ => None,
    }
}

/// The server list's layout off its list box: 1.5's `joinserver.menu`
/// lists nine columns to 1.1's six.
fn server_columns(menus: &[UiMenu]) -> &'static [browser::Column] {
    (menus.iter().flat_map(|m| &m.items))
        .find(|i| i.feeder == Some(FEEDER_SERVERS) && !i.columns.is_empty())
        .map_or(&browser::COLUMNS_V1, |i| {
            browser::columns_for(i.columns.len())
        })
}

/// Rows a list box shows whole: Q3 stops at the last that fits.
fn visible_rows(item: &UiItem) -> usize {
    ((item.rect[3] - 2.0) / item.element_height.max(1.0))
        .floor()
        .max(1.0) as usize
}

/// `menu`'s `execKey` script for retail's key number `n`
/// ([`crate::console::keys::number`]).
fn exec_key(menu: &UiMenu, n: u32) -> Option<Vec<String>> {
    // `Menu_HandleKey` tries the slot for keys 1..255.
    let n = u8::try_from(n).ok().filter(|&n| n != 0)?;
    let (_, script) = menu.exec_keys.iter().find(|(k, _)| *k == n)?;
    Some(script.clone())
}

fn contains(r: [f32; 4], p: [f32; 2]) -> bool {
    p[0] >= r[0] && p[0] <= r[0] + r[2] && p[1] >= r[1] && p[1] <= r[1] + r[3]
}

/// `%s` and `%d` in a localized format filled in order.
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

fn fill(fmt: &str, args: &[&str]) -> String {
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

/// `menudef.h`'s upper case edit field, which `Item_SetTextExtents`
/// measures a cvar for beside 4 and 9 (0x40013ca4).
const ITEM_TYPE_UPREDITFIELD: i32 = 15;
/// `menudef.h`'s `ITEM_ALIGN_CENTER2`: measured as right aligned, never
/// kept (0x40013c28, 0x40013cf2).
const ITEM_ALIGN_CENTER2: i32 = 3;

/// Whether the item's paint runs `Item_Text_Paint` (0x400144d0), which
/// sets its text rect: text items and buttons from `Item_Paint`'s switch
/// (0x40016470), the edit fields' paint (0x40014600) always, the slider's,
/// yes/no's, multi's, bind's and owner draw's only when the item has text.
fn paints_text(item: &UiItem) -> bool {
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

/// Draws on the 640x480 grid stretched over the window, as the UI module
/// does.
/// Paints menu items on the 640 grid, drawn in the window's centred 4:3
/// box ([`Virtual`]); a rect that spans the whole grid width or height
/// (a backdrop, a fade) spans the window's.
pub(crate) struct Painter<'a> {
    pub(crate) v: Virtual,
    pub(crate) fonts: Option<&'a UiFonts>,
    /// CoD2: every rect maps straight through `v`, none stretched to the
    /// window, as its rects were already placed ([`cod2`]).
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

    /// `Window_Paint`: the fill or shader inside the border, then a full
    /// border of `border_size`. `cvar_shader` is a
    /// `WINDOW_STYLE_CVAR_SHADER` item's shader; an empty one draws nothing.
    fn window(
        &self,
        item: &UiItem,
        st: &ItemState,
        cvar_shader: Option<&str>,
        out: &mut Vec<HudQuad>,
    ) {
        let mut r = item.rect;
        if item.border != 0 {
            let b = item.border_size;
            r = [r[0] + b, r[1] + b, r[2] - (b + 1.0), r[3] - (b + 1.0)];
        }
        match (item.style, &item.background) {
            (WINDOW_STYLE_FILLED, Some(bg)) => self.quad(r, st.back, bg, out),
            (WINDOW_STYLE_FILLED, None) => self.fill(r, st.back, out),
            // Style 3 draws whatever handle it holds, and a missing image's
            // or no background's is 0, the default shader (0x4000fa79).
            (WINDOW_STYLE_SHADER, Some(bg)) => {
                self.quad(r, item.fore, &crate::hud::default_if_missing(bg), out)
            }
            (WINDOW_STYLE_SHADER, None) => self.quad(r, item.fore, crate::hud::DEFAULT_SHADER, out),
            (WINDOW_STYLE_CVAR_SHADER, _) => {
                if let Some(bg) = cvar_shader.filter(|b| !b.is_empty()) {
                    self.quad(r, item.fore, bg, out);
                }
            }
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

    /// `Item_Text_AutoWrapped_Paint` (0x40014000): the lines `text` breaks
    /// into, each with its left edge and baseline on the 640 grid. A text
    /// wider than the rect aims at `width / ceil(width / rect.w)` a line, so
    /// the lines come out even: a line breaks at a space past that aim, at
    /// the last space once it is past the rect's width, and at `\n`. A
    /// line aligns on the width measured one character short of its last
    /// word's end, as retail's lagging measure does, and lines sit the
    /// text height plus 5 apart; `\r` reads as a space.
    fn wrap_lines(&self, text: &str, item: &UiItem) -> Vec<(String, f32, f32)> {
        let scale = item.text_scale;
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

    /// The font the UI picks for `scale` on this window.
    fn font(&self, scale: f32) -> Option<&font::Font> {
        self.fonts
            .map(|f| f.pick(Slot::Default, scale, 480.0 * self.v.scale))
    }

    /// Width on the 640 grid.
    pub(crate) fn width(&self, text: &str, scale: f32) -> f32 {
        self.font(scale)
            .map_or(0.0, |f| font::measure(f, text, scale / f.unit_scale()))
    }

    /// The text height on the 640 grid: the font's tallest glyph at
    /// `scale`, as far above the baseline as [`Self::text`] puts the top.
    pub(crate) fn height(&self, scale: f32) -> f32 {
        self.font(scale)
            .map_or(0.0, |f| f.max_height as f32 * f.glyph_scale * scale)
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
        let [px, py] = self.v.point(x, top);
        font::layout(
            f,
            text,
            px,
            py,
            scale * self.v.scale / f.unit_scale(),
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
        let fs = vcod_common::testing::stock_fs_11()?;
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

    fn widths(label: f32, value: f32, cvar: f32) -> TextWidths {
        TextWidths {
            label,
            value,
            cvar,
            height: 12.9,
        }
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

    #[test]
    fn join_a_game_opens_the_browser_and_quit_asks_first() {
        let Some(mut ui) = ui() else { return };
        let shell = Shell::new();
        let mut out = Vec::new();
        ui.open_main(&Shell::new(), &mut out);
        assert!(out.contains(&UiEffect::Sound("music_mainmenu".into())));
        let out = click_item(&mut ui, "main", "@MENU_JOIN_GAME", &shell);
        assert_eq!(out, vec![UiEffect::Sound("mouse_click".into())]);
        let top = *ui.open.last().unwrap();
        assert_eq!(ui.menus[top].name, "joinserver");
        assert_eq!(ui.open.len(), 1, "main closed behind it");

        // Esc goes back to main.
        let (used, _) = ui.key(KeyCode::Escape, &shell);
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
        ui.show_error("Server timed out", &Shell::new(), &mut Vec::new());
        let names: Vec<_> = ui.open.iter().map(|&m| ui.menus[m].name.as_str()).collect();
        assert_eq!(names, ["main", "error_popmenu"]);
        assert_eq!(
            ui.cvar("com_errorMessage", &shell).as_deref(),
            Some("Server timed out")
        );
        let (_, _) = ui.key(KeyCode::Escape, &shell);
        assert_eq!(ui.menus[*ui.open.last().unwrap()].name, "main");
    }

    #[test]
    fn the_in_game_main_menu_swaps_its_buttons_and_closes_on_esc() {
        let Some(mut ui) = ui() else { return };
        let shell = Shell::new();
        ui.open_ingame(&Shell::new(), &mut Vec::new());
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
        let (used, _) = ui.key(KeyCode::Escape, &shell);
        assert!(used);
        assert!(!ui.active());

        ui.open_ingame(&Shell::new(), &mut Vec::new());
        click_item(&mut ui, "main", "@MENU_BACKTOGAME", &shell);
        assert!(!ui.active());

        // With no game up the same Esc leaves main open.
        ui.open_main(&Shell::new(), &mut Vec::new());
        assert_eq!(ui.cvar("cl_ingame", &shell).as_deref(), Some("0"));
        ui.key(KeyCode::Escape, &shell);
        assert!(ui.active());
    }

    #[test]
    fn unsupported_menus_stay_shut() {
        let Some(mut ui) = ui() else { return };
        let mut out = Vec::new();
        ui.open_main(&Shell::new(), &mut out);
        ui.open_menu("cdkey_menu", &Shell::new(), &mut out);
        assert_eq!(ui.open.len(), 1);
        assert_eq!(ui.menus[ui.open[0]].name, "main");
    }

    /// `callvote.menu`'s map and game type picks go to the console as
    /// `callvote` lines off the selected row and game type.
    #[test]
    fn the_callvote_menu_scripts_name_the_selection() {
        let Some(mut ui) = ui() else { return };
        let mut out = Vec::new();
        ui.load_arenas(&mut out);
        let map = ui.active_maps()[0].map.clone();
        let gt = ui.game_types[ui.net_gametype()].script.clone();
        for (script, line) in [
            ("voteMap", format!("callvote map {map}")),
            ("voteTypeMap", format!("callvote typemap {gt} {map}")),
            ("voteGame", format!("callvote g_gametype {gt}")),
        ] {
            let mut out = Vec::new();
            ui.ui_script(&[script.to_string()], &mut out);
            assert_eq!(out, [UiEffect::Command(line)]);
        }
    }

    /// The kick pick names the selected snapshot client, cleaned and
    /// quoted; the list is rebuilt at most every 3 s.
    #[test]
    fn the_kick_pick_names_the_selected_player() {
        let mut ui = Ui::new(&Pk3Fs::empty(), "0");
        let t0 = Instant::now();
        ui.offer_players(t0, || vec!["^1Able".into(), "Baker Two".into()]);
        ui.offer_players(t0 + Duration::from_secs(1), || vec!["Charlie".into()]);
        assert_eq!(ui.players, ["Able", "Baker Two"]);
        let mut out = Vec::new();
        ui.ui_script(&["voteKick".to_string()], &mut out);
        assert_eq!(out, [UiEffect::Command("callvote kick \"Able\"".into())]);
        ui.player_row = 1;
        out.clear();
        ui.ui_script(&["voteKick".to_string()], &mut out);
        assert_eq!(
            out,
            [UiEffect::Command("callvote kick \"Baker Two\"".into())]
        );
        ui.offer_players(t0 + Duration::from_secs(4), || vec!["Charlie".into()]);
        out.clear();
        ui.ui_script(&["voteKick".to_string()], &mut out);
        assert!(out.is_empty(), "row 1 is past the rebuilt list");
    }

    #[test]
    fn start_new_server_picks_a_game_type_and_map_and_starts() {
        let Some(mut ui) = ui() else { return };
        let mut shell = Shell::new();
        let mut out = Vec::new();
        ui.open_main(&Shell::new(), &mut out);
        apply(&mut shell, &out);
        ui.frame(Instant::now(), &shell);
        let out = click_item(&mut ui, "main", "@MENU_START_NEW_SERVER", &shell);
        apply(&mut shell, &out);
        ui.frame(Instant::now(), &shell);
        let open: Vec<&str> = ui.open.iter().map(|&m| ui.menus[m].name.as_str()).collect();
        assert_eq!(
            open,
            ["createserver", "createserver_op", "createserver_maps"]
        );
        // `loadArenas` follows `g_gametype`, "dm" by default.
        assert_eq!(shell.cvar("ui_netGametypeName"), Some("dm"));
        let op = ui.find("createserver_op").unwrap();
        let gt = (0..ui.menus[op].items.len())
            .find(|&i| ui.menus[op].items[i].ownerdraw == UI_NETGAMETYPE)
            .unwrap();
        click_at(&mut ui, op, gt, &mut shell);
        assert_eq!(shell.cvar("ui_netGametypeName"), Some("re"));
        // A right click steps back, wrapping (0x40009a60).
        let out = ui.right_click(Instant::now(), &shell);
        apply(&mut shell, &out);
        ui.frame(Instant::now(), &shell);
        assert_eq!(shell.cvar("ui_netGametypeName"), Some("dm"));
        let out = ui.right_click(Instant::now(), &shell);
        apply(&mut shell, &out);
        ui.frame(Instant::now(), &shell);
        let dm = ui.game_types.iter().position(|g| g.script == "dm").unwrap();
        let n = ui.game_types.len();
        let before = ui.game_types[(dm + n - 1) % n].script.clone();
        assert_eq!(shell.cvar("ui_netGametypeName"), Some(before.as_str()));
        click_at(&mut ui, op, gt, &mut shell);
        click_at(&mut ui, op, gt, &mut shell);
        assert_eq!(shell.cvar("ui_netGametypeName"), Some("re"));
        click_named(
            &mut ui,
            "createserver_op",
            "button_serversettings",
            &mut shell,
        );
        assert_eq!(top_name(&ui), "settings_re");
        // The second map row.
        let maps = ui.find("createserver_maps").unwrap();
        let list = ui.menus[maps]
            .items
            .iter()
            .position(|i| i.feeder == Some(FEEDER_ALLMAPS))
            .unwrap();
        let r = ui.menus[maps].items[list].rect;
        ui.mouse_move(r[0] + 10.0, r[1] + 26.0, 640.0, 480.0, &shell);
        ui.click(Instant::now(), &shell);
        let want = ui.active_maps()[1].map.clone();
        shell.execute("set ui_dedicated 7");
        ui.frame(Instant::now(), &shell);
        let accept = (0..ui.menus[op].items.len())
            .find(|&i| ui.menus[op].items[i].name == "accept")
            .unwrap();
        let r = ui.menus[op].items[accept].rect;
        ui.mouse_move(r[0] + 2.0, r[1] + 2.0, 640.0, 480.0, &shell);
        let out = ui.click(Instant::now(), &shell);
        let Some(UiEffect::StartServer(req)) = out.last() else {
            panic!("{out:?}");
        };
        assert_eq!(
            (req.map.as_str(), req.gametype.as_str()),
            (want.as_str(), "re")
        );
        assert_eq!(req.dedicated, 2);
        for name in [
            "sv_hostname",
            "sv_maxclients",
            "g_password",
            "scr_re_timelimit",
        ] {
            assert!(req.cvars.iter().any(|c| c == name), "{name}");
        }
        assert!(req.cvars.iter().any(|c| c == "scr_allow_springfield"));
        assert!(!req.cvars.iter().any(|c| c.starts_with("ui_")));
        assert!(out.contains(&UiEffect::Command("set g_gametype \"re\"".into())));
    }

    #[test]
    fn the_map_list_scrolls_by_its_bar_and_by_keys_over_it() {
        let Some(mut ui) = ui() else { return };
        let shell = Shell::new();
        let mut out = Vec::new();
        ui.open_main(&Shell::new(), &mut out);
        ui.open_menu("createserver", &Shell::new(), &mut out);
        let maps = ui.find("createserver_maps").unwrap();
        let list = ui.menus[maps]
            .items
            .iter()
            .position(|i| i.feeder == Some(FEEDER_ALLMAPS))
            .unwrap();
        let item = ui.menus[maps].items[list].clone();
        let len = ui.active_maps().len();
        let max = listbox::max_scroll(&item, len);
        assert!(max > 0, "{len} maps fit");
        let [x, y, w, h] = item.rect;
        let bar = x + w - 8.0;
        // The down arrow, then the page below the thumb.
        ui.mouse_move(bar, y + h - 4.0, 640.0, 480.0, &shell);
        ui.click(Instant::now(), &shell);
        assert_eq!((ui.map_top, ui.map_row), (1, 0));
        ui.mouse_move(bar, y + h - 20.0, 640.0, 480.0, &shell);
        ui.click(Instant::now(), &shell);
        assert_eq!(ui.map_top, max);
        // The thumb dragged back to the top.
        ui.mouse_move(bar, y + h - 30.0, 640.0, 480.0, &shell);
        ui.click(Instant::now(), &shell);
        ui.mouse_move(bar, y, 640.0, 480.0, &shell);
        ui.release();
        assert_eq!(ui.map_top, 0);
        // End over the list selects the last map and scrolls to it.
        ui.mouse_move(x + 10.0, y + 30.0, 640.0, 480.0, &shell);
        assert!(ui.key(KeyCode::End, &shell).0);
        assert_eq!(ui.map_row, len - 1);
        assert_eq!(ui.map_top, len - listbox::view_max(&item));
        assert!(ui.key(KeyCode::ArrowUp, &shell).0);
        assert_eq!(ui.map_row, len - 2);
    }

    /// The map list over `createserver`, its item and the down arrow's spot.
    fn map_list(ui: &mut Ui) -> (UiItem, [f32; 2]) {
        let mut out = Vec::new();
        ui.open_main(&Shell::new(), &mut out);
        ui.open_menu("createserver", &Shell::new(), &mut out);
        let maps = ui.find("createserver_maps").unwrap();
        let item = ui.menus[maps]
            .items
            .iter()
            .find(|i| i.feeder == Some(FEEDER_ALLMAPS))
            .unwrap()
            .clone();
        let [x, y, w, h] = item.rect;
        (item, [x + w - 8.0, y + h - 4.0])
    }

    #[test]
    fn the_wheel_ends_an_arrow_hold_and_scrolls_the_list_under_it() {
        let Some(mut ui) = ui() else { return };
        let shell = Shell::new();
        let (_, arrow) = map_list(&mut ui);
        let t0 = Instant::now();
        ui.mouse_move(arrow[0], arrow[1], 640.0, 480.0, &shell);
        ui.click(t0, &shell);
        assert_eq!((ui.map_top, ui.map_row), (1, 0));
        ui.wheel(true, &shell);
        ui.wheel(true, &shell);
        ui.wheel(false, &shell);
        ui.frame(t0 + Duration::from_millis(700), &shell);
        assert_eq!((ui.map_top, ui.map_row), (2, 0), "no repeat after the hold");
    }

    #[test]
    fn a_second_button_in_a_hold_ends_it_and_still_steps() {
        let Some(mut ui) = ui() else { return };
        let shell = Shell::new();
        let (_, arrow) = map_list(&mut ui);
        let t0 = Instant::now();
        ui.mouse_move(arrow[0], arrow[1], 640.0, 480.0, &shell);
        ui.click(t0, &shell);
        ui.right_click(t0 + Duration::from_millis(100), &shell);
        assert_eq!(ui.map_top, 2);
        ui.frame(t0 + Duration::from_millis(700), &shell);
        assert_eq!(ui.map_top, 2, "the right press started no new hold");
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

    /// The main menu open with the cursor on Start New Server.
    fn main_at_start(ui: &mut Ui, shell: &Shell) -> (usize, usize) {
        ui.open_main(&Shell::new(), &mut Vec::new());
        let m = ui.find("main").unwrap();
        let start = (ui.menus[m].items.iter())
            .position(|i| i.text == "@MENU_START_NEW_SERVER")
            .unwrap();
        let r = ui.menus[m].items[start].rect;
        ui.mouse_move(r[0] + 2.0, r[1] + 2.0, 640.0, 480.0, shell);
        (m, start)
    }

    #[test]
    fn tab_moves_the_focus_while_held_and_its_release_takes_it_back() {
        let Some(mut ui) = ui() else { return };
        let shell = Shell::new();
        let (m, start) = main_at_start(&mut ui, &shell);
        let over = UiEffect::Sound("mouse_over".into());
        let (used, out) = ui.key(KeyCode::Tab, &shell);
        assert!(used);
        assert_eq!(out, vec![over.clone()], "the next item's mouseEnter");
        let target = ui.focus[&m];
        assert!(target != start);
        assert!(ui.key_repeat(KeyCode::Tab, None, &shell).is_empty());
        assert!(ui.focused(m, target), "a repeat steps nothing");
        assert_eq!(
            ui.key_up(&shell),
            vec![over],
            "the hovered item's mouseEnter"
        );
        assert!(ui.focused(m, start));
        assert_eq!(ui.cursor_item.get(&m), Some(&start));
        // Up goes the other way.
        ui.key(KeyCode::ArrowUp, &shell);
        let back = ui.focus[&m];
        assert!(back != target && back != start);
    }

    #[test]
    fn with_the_cursor_on_no_item_tab_walks_the_items() {
        let Some(mut ui) = ui() else { return };
        let shell = Shell::new();
        let (m, _) = main_at_start(&mut ui, &shell);
        ui.mouse_move(0.0, 479.0, 640.0, 480.0, &shell);
        assert_eq!(ui.hover, None);
        ui.key(KeyCode::Tab, &shell);
        ui.key_up(&shell);
        let first = ui.cursor_item[&m];
        ui.key(KeyCode::Tab, &shell);
        ui.key_up(&shell);
        let second = ui.cursor_item[&m];
        assert!(second != first);
        ui.key(KeyCode::ArrowLeft, &shell);
        assert_eq!(ui.cursor_item[&m], first);
        // A click takes the step back before it acts.
        ui.click(Instant::now(), &shell);
        assert!(!ui.focus.contains_key(&m));
    }

    #[test]
    fn tab_in_an_edit_field_carries_the_edit_to_the_next_field() {
        let Some(mut ui) = ui() else { return };
        let mut shell = Shell::new();
        ui.open_menu("createfavorite_popmenu", &Shell::new(), &mut Vec::new());
        let m = ui.find("createfavorite_popmenu").unwrap();
        let fields: Vec<usize> = (0..ui.menus[m].items.len())
            .filter(|&i| ui.menus[m].items[i].kind == ui_menu::ITEM_TYPE_EDITFIELD)
            .collect();
        let [name, addr] = fields[..] else {
            panic!("two fields")
        };
        click_at(&mut ui, m, name, &mut shell);
        assert_eq!(ui.options.editing, Some((m, name)));
        let key = |ui: &mut Ui, shell: &mut Shell, code, text| {
            let out = ui.edit_key(code, text, shell);
            apply(shell, &out);
        };
        key(&mut ui, &mut shell, KeyCode::Tab, None);
        assert_eq!(ui.options.editing, Some((m, addr)));
        assert!(ui.focused(m, addr));
        key(&mut ui, &mut shell, KeyCode::KeyX, Some("x"));
        key(&mut ui, &mut shell, KeyCode::ArrowUp, None);
        assert_eq!(ui.options.editing, Some((m, name)));
        key(&mut ui, &mut shell, KeyCode::KeyY, Some("y"));
        assert_eq!(shell.cvar("ui_favoriteAddress"), Some("x"));
        assert_eq!(shell.cvar("ui_favoriteName"), Some("y"));
        // A step onto a button moves the focus and leaves the edit.
        key(&mut ui, &mut shell, KeyCode::ArrowDown, None);
        key(&mut ui, &mut shell, KeyCode::ArrowDown, None);
        let button = ui.focus[&m];
        assert!(button != addr && button != name);
        assert_eq!(ui.options.editing, Some((m, addr)));
        // The mouse pass waits for the edit to end.
        let r = ui.menus[m].items[name].rect;
        ui.mouse_move(r[0] + 2.0, r[1] + 2.0, 640.0, 480.0, &shell);
        assert_eq!(ui.hover, Some((m, name)));
        let r = ui.menus[m].items[button].rect;
        ui.mouse_move(r[0] + 2.0, r[1] + 2.0, 640.0, 480.0, &shell);
        assert_eq!(ui.hover, Some((m, name)), "no mouse pass in an edit");
        key(&mut ui, &mut shell, KeyCode::Enter, None);
        ui.mouse_move(r[0] + 3.0, r[1] + 2.0, 640.0, 480.0, &shell);
        assert_eq!(ui.hover, Some((m, button)));
        assert_eq!(ui.focus.get(&m), Some(&button));
    }

    #[test]
    fn an_exec_key_runs_ahead_of_the_focus_keys() {
        let Some(mut ui) = ui() else { return };
        let shell = Shell::new();
        let (m, start) = main_at_start(&mut ui, &shell);
        let play = |s: &str| vec!["play".to_string(), s.to_string()];
        // Retail's key numbers: up arrow 154, keypad down 189.
        ui.menus[m].exec_keys = vec![
            (b'q', play("q")),
            (9, play("tab")),
            (154, play("up")),
            (189, play("kp")),
        ];
        let (used, out) = ui.key(KeyCode::KeyQ, &shell);
        assert!(used);
        assert_eq!(out, vec![UiEffect::Sound("q".into())]);
        let (_, out) = ui.key(KeyCode::ArrowUp, &shell);
        assert_eq!(out, vec![UiEffect::Sound("up".into())]);
        let (_, out) = ui.key(KeyCode::Numpad2, &shell);
        assert_eq!(out, vec![UiEffect::Sound("kp".into())]);
        let (_, out) = ui.key(KeyCode::Tab, &shell);
        assert_eq!(out, vec![UiEffect::Sound("tab".into())]);
        assert_eq!(ui.focus.get(&m), Some(&start), "Tab's execKey takes it");
        assert!(!ui.key(KeyCode::KeyW, &shell).0);
    }

    #[test]
    fn focus_runs_on_focus_leave_focus_and_the_focus_sound() {
        let Some(mut ui) = ui() else { return };
        let shell = Shell::new();
        ui.open_main(&Shell::new(), &mut Vec::new());
        let m = ui.find("main").unwrap();
        let at = |ui: &Ui, text: &str| (ui.menus[m].items.iter()).position(|i| i.text == text);
        let start = at(&ui, "@MENU_START_NEW_SERVER").unwrap();
        let play = |s: &str| vec!["play".to_string(), s.to_string()];
        for (i, item) in ui.menus[m].items.iter_mut().enumerate() {
            item.mouse_enter.clear();
            item.on_focus = play(&format!("on{i}"));
            item.focus_sound = Some(format!("snd{i}"));
        }
        ui.menus[m].items[0].leave_focus = play("leave");
        let snd = |s: String| UiEffect::Sound(s);
        let r = ui.menus[m].items[start].rect;
        let out = ui.mouse_move(r[0] + 2.0, r[1] + 2.0, 640.0, 480.0, &shell);
        let took = |i: usize| {
            vec![
                snd("leave".into()),
                snd(format!("on{i}")),
                UiEffect::FocusSound {
                    item: Some(format!("snd{i}")),
                    global: None,
                },
            ]
        };
        assert_eq!(out, took(start));
        // Moving inside the item focuses nothing again.
        assert!(
            ui.mouse_move(r[0] + 3.0, r[1] + 2.0, 640.0, 480.0, &shell)
                .is_empty()
        );
        // Tab focuses the next item; its release gives the focus back.
        let (_, out) = ui.key(KeyCode::Tab, &shell);
        let target = ui.focus[&m];
        assert_eq!(out, took(target));
        assert_eq!(ui.key_up(&shell), took(start));
        // Off every item the focus clears.
        let out = ui.mouse_move(0.0, 479.0, 640.0, 480.0, &shell);
        assert_eq!(ui.hover, None);
        assert_eq!(out, vec![snd("leave".into())]);
        assert!(!ui.focus.contains_key(&m));
    }

    #[test]
    fn an_item_with_no_focus_sound_plays_item_focus_sound() {
        let Some(mut ui) = ui() else { return };
        let shell = Shell::new();
        // Stock `main.menu` comments its `itemFocusSound` out.
        assert_eq!(ui.assets.item_focus_sound, None);
        ui.assets.item_focus_sound = Some("global".into());
        let (m, start) = main_at_start(&mut ui, &shell);
        ui.mouse_move(0.0, 479.0, 640.0, 480.0, &shell);
        ui.menus[m].items[start].focus_sound = None;
        let r = ui.menus[m].items[start].rect;
        let out = ui.mouse_move(r[0] + 2.0, r[1] + 2.0, 640.0, 480.0, &shell);
        let sound = UiEffect::FocusSound {
            item: None,
            global: Some("global".into()),
        };
        assert!(out.contains(&sound), "{out:?}");
    }

    #[test]
    fn the_mouse_reaches_a_menu_under_the_top_one_unless_that_is_a_popup() {
        let Some(mut ui) = ui() else { return };
        let shell = Shell::new();
        ui.open_main(&Shell::new(), &mut Vec::new());
        ui.open_menu("mods_menu", &Shell::new(), &mut Vec::new());
        let (main, mods) = (ui.find("main").unwrap(), ui.find("mods_menu").unwrap());
        let join = (ui.menus[main].items.iter())
            .position(|i| i.text == "@MENU_JOIN_GAME")
            .unwrap();
        ui.menus[main].items[join].mouse_enter = vec!["play".into(), "enter".into()];
        let r = ui.menus[main].items[join].rect;
        let at = [r[0] + 2.0, r[1] + 2.0];
        assert!(!contains(ui.menus[mods].rect, at));
        let out = ui.mouse_move(at[0], at[1], 640.0, 480.0, &shell);
        assert!(out.contains(&UiEffect::Sound("enter".into())), "{out:?}");
        assert_eq!(ui.hover, Some((main, join)));
        assert_eq!(ui.focus.get(&main), Some(&join));
        // Off the item main's focus clears; over a popup nothing under it
        // takes the mouse.
        ui.mouse_move(0.0, 479.0, 640.0, 480.0, &shell);
        assert!(!ui.focus.contains_key(&main));
        ui.menus[mods].popup = true;
        let out = ui.mouse_move(at[0], at[1], 640.0, 480.0, &shell);
        assert!(out.is_empty(), "{out:?}");
        assert_eq!(ui.hover, None);
        assert!(!ui.over.contains(&(main, join)));
    }

    #[test]
    fn an_off_menu_click_focuses_the_menu_under_it_in_place() {
        let Some(mut ui) = ui() else { return };
        let shell = Shell::new();
        ui.open_main(&shell, &mut Vec::new());
        ui.open_menu("mods_menu", &shell, &mut Vec::new());
        let (main, mods) = (ui.find("main").unwrap(), ui.find("mods_menu").unwrap());
        assert!(!ui.menus[mods].popup);
        let play = |s: &str| vec!["play".to_string(), s.to_string()];
        let join = (ui.menus[main].items.iter())
            .position(|i| i.text == "@MENU_JOIN_GAME")
            .unwrap();
        ui.menus[main].items[join].action = play("join");
        ui.menus[main].on_esc = play("main esc");
        ui.menus[mods].on_esc = play("mods esc");
        let r = ui.menus[main].items[join].rect;
        ui.mouse_move(r[0] + 2.0, r[1] + 2.0, 640.0, 480.0, &shell);
        assert_eq!(ui.focused_menu(), Some(mods));
        let out = ui.click(Instant::now(), &shell);
        assert!(out.contains(&UiEffect::Sound("join".into())), "{out:?}");
        // `main` takes the keys; the stack keeps its order.
        assert_eq!(ui.focused_menu(), Some(main));
        assert_eq!(ui.open, vec![main, mods]);
        let (_, out) = ui.key(KeyCode::Escape, &shell);
        assert_eq!(out, vec![UiEffect::Sound("main esc".into())]);
        // Opening a menu gives it the focus back.
        ui.open_menu("mods_menu", &shell, &mut Vec::new());
        let (_, out) = ui.key(KeyCode::Escape, &shell);
        assert_eq!(out, vec![UiEffect::Sound("mods esc".into())]);
        // Closing the focused menu hands the focus to the new top.
        ui.close_menu("mods_menu", &shell, &mut Vec::new());
        assert_eq!(ui.focused_menu(), Some(main));
    }

    #[test]
    fn opening_a_menu_runs_a_mouse_pass_at_the_cursor() {
        let Some(mut ui) = ui() else { return };
        let shell = Shell::new();
        ui.open_main(&shell, &mut Vec::new());
        let quit = ui.find("quit_popmenu").unwrap();
        let i = (0..ui.menus[quit].items.len())
            .rev()
            .find(|&i| !ui.menus[quit].items[i].decoration && ui.shown(quit, i, &shell))
            .unwrap();
        ui.menus[quit].items[i].mouse_enter = vec!["play".into(), "enter".into()];
        let r = ui.menus[quit].items[i].rect;
        ui.mouse_move(r[0] + 2.0, r[1] + 2.0, 640.0, 480.0, &shell);
        let mut out = Vec::new();
        ui.open_menu("quit_popmenu", &shell, &mut out);
        assert!(out.contains(&UiEffect::Sound("enter".into())), "{out:?}");
        assert_eq!(ui.hover, Some((quit, i)));
        assert_eq!(ui.focus.get(&quit), Some(&i));
    }

    #[test]
    fn exec_key_takes_the_mouse_buttons_and_the_wheel() {
        let Some(mut ui) = ui() else { return };
        let shell = Shell::new();
        let (m, start) = main_at_start(&mut ui, &shell);
        let play = |s: &str| vec!["play".to_string(), s.to_string()];
        ui.menus[m].items[start].action = play("action");
        ui.menus[m].exec_keys = vec![(200, play("m1")), (204, play("m5")), (206, play("wu"))];
        let sound = |s: &str| UiEffect::Sound(s.into());
        // A button has no key handler, so the menu's MOUSE1 slot runs in
        // place of the click.
        let out = ui.click(Instant::now(), &shell);
        assert_eq!(out, vec![sound("m1")]);
        assert_eq!(
            ui.mouse_button(204, Instant::now(), &shell),
            vec![sound("m5")]
        );
        assert!(ui.mouse_button(203, Instant::now(), &shell).is_empty());
        // With no MOUSE2 slot the right click clicks.
        assert!(
            ui.right_click(Instant::now(), &shell)
                .contains(&sound("action"))
        );
        let out = ui.wheel(false, &shell);
        assert!(out.contains(&sound("wu")), "{out:?}");
        assert_eq!(
            ui.focus.get(&m),
            Some(&start),
            "MWHEELUP's execKey takes it"
        );
    }

    #[test]
    fn overlapping_items_all_take_the_mouse_and_trade_the_focus() {
        let Some(mut ui) = ui() else { return };
        let shell = Shell::new();
        let (m, start) = main_at_start(&mut ui, &shell);
        let join = (ui.menus[m].items.iter())
            .position(|i| i.text == "@MENU_JOIN_GAME")
            .unwrap();
        ui.mouse_move(0.0, 479.0, 640.0, 480.0, &shell);
        ui.menus[m].items[join].rect = ui.menus[m].items[start].rect;
        let play = |s: &str| vec!["play".to_string(), s.to_string()];
        ui.menus[m].items[join].mouse_enter = play("join");
        ui.menus[m].items[start].mouse_enter = play("start");
        let r = ui.menus[m].items[start].rect;
        let out = ui.mouse_move(r[0] + 2.0, r[1] + 2.0, 640.0, 480.0, &shell);
        // Both enter, the later item first; it takes the focus.
        let (hi, lo) = (start.max(join), start.min(join));
        let name = |i: usize| if i == start { "start" } else { "join" };
        let sounds: Vec<UiEffect> = (out.iter())
            .filter(|e| matches!(e, UiEffect::Sound(s) if s == "join" || s == "start"))
            .cloned()
            .collect();
        assert_eq!(
            sounds,
            vec![
                UiEffect::Sound(name(hi).into()),
                UiEffect::Sound(name(lo).into())
            ]
        );
        assert!(ui.over.contains(&(m, start)) && ui.over.contains(&(m, join)));
        assert_eq!(ui.focus.get(&m), Some(&hi));
        // The focused one refuses the focus again, so each move inside
        // hands it to the other.
        ui.mouse_move(r[0] + 3.0, r[1] + 2.0, 640.0, 480.0, &shell);
        assert_eq!(ui.focus.get(&m), Some(&lo));
        ui.mouse_move(r[0] + 4.0, r[1] + 2.0, 640.0, 480.0, &shell);
        assert_eq!(ui.focus.get(&m), Some(&hi));
    }

    #[test]
    fn a_click_acts_on_the_focused_item_of_overlapping_ones() {
        let Some(mut ui) = ui() else { return };
        let shell = Shell::new();
        let (m, start) = main_at_start(&mut ui, &shell);
        let join = (ui.menus[m].items.iter())
            .position(|i| i.text == "@MENU_JOIN_GAME")
            .unwrap();
        ui.mouse_move(0.0, 479.0, 640.0, 480.0, &shell);
        ui.menus[m].items[join].rect = ui.menus[m].items[start].rect;
        let play = |s: &str| vec!["play".to_string(), s.to_string()];
        ui.menus[m].items[join].action = play("join");
        ui.menus[m].items[start].action = play("start");
        let r = ui.menus[m].items[start].rect;
        ui.mouse_move(r[0] + 2.0, r[1] + 2.0, 640.0, 480.0, &shell);
        // The press's own mouse pass hands the focus over first.
        let before = ui.focus[&m];
        let out = ui.click(Instant::now(), &shell);
        let after = ui.focus[&m];
        assert_ne!(before, after);
        let name = if after == start { "start" } else { "join" };
        assert!(out.contains(&UiEffect::Sound(name.into())), "{out:?}");
        let other = if after == start { "join" } else { "start" };
        assert!(!out.contains(&UiEffect::Sound(other.into())), "{out:?}");
    }

    #[test]
    fn a_focused_item_a_cvar_test_hides_still_takes_the_click() {
        let Some(mut ui) = ui() else { return };
        let mut shell = Shell::new();
        shell.execute("set vcod_test_gate 1");
        let m = ui.find("main").unwrap();
        let start = (ui.menus[m].items.iter())
            .position(|i| i.text == "@MENU_START_NEW_SERVER")
            .unwrap();
        ui.menus[m].items[start].gate = Some(vcod_common::menu::CvarGate {
            cvar: "vcod_test_gate".into(),
            show: true,
            values: vec!["1".into()],
        });
        ui.menus[m].items[start].action = vec!["play".into(), "start".into()];
        main_at_start(&mut ui, &shell);
        assert_eq!(ui.focus.get(&m), Some(&start));
        shell.execute("set vcod_test_gate 0");
        assert_eq!(ui.hit(&shell), None);
        // `Menu_HandleMouseMove` notes the focused item among the shown ones
        // only, so it keeps the focus and `Menu_HandleKey` acts on it.
        let out = ui.click(Instant::now(), &shell);
        assert_eq!(ui.focus.get(&m), Some(&start));
        assert!(out.contains(&UiEffect::Sound("start".into())), "{out:?}");
    }

    #[test]
    fn the_middle_button_and_enter_act_on_the_focused_item() {
        let Some(mut ui) = ui() else { return };
        let shell = Shell::new();
        let (m, start) = main_at_start(&mut ui, &shell);
        ui.menus[m].items[start].action = vec!["play".into(), "start".into()];
        let sound = UiEffect::Sound("start".into());
        assert!(
            ui.mouse_button(K_MOUSE3, Instant::now(), &shell)
                .contains(&sound)
        );
        assert!(ui.key(KeyCode::Enter, &shell).1.contains(&sound));
        assert!(ui.key(KeyCode::NumpadEnter, &shell).1.contains(&sound));
        // An execKey for the middle button runs in its place.
        ui.menus[m].exec_keys = vec![(202, vec!["play".into(), "m3".into()])];
        assert_eq!(
            ui.mouse_button(K_MOUSE3, Instant::now(), &shell),
            vec![UiEffect::Sound("m3".into())]
        );
        // Off every item there is no focused item to act on.
        ui.mouse_move(0.0, 479.0, 640.0, 480.0, &shell);
        assert!(!ui.key(KeyCode::Enter, &shell).1.contains(&sound));
    }

    #[test]
    fn enter_on_an_edit_field_starts_the_edit() {
        let Some(mut ui) = ui() else { return };
        let shell = Shell::new();
        ui.open_menu("createfavorite_popmenu", &shell, &mut Vec::new());
        let m = ui.find("createfavorite_popmenu").unwrap();
        let field = (0..ui.menus[m].items.len())
            .find(|&i| ui.menus[m].items[i].kind == ITEM_TYPE_EDITFIELD && ui.shown(m, i, &shell))
            .unwrap();
        let r = ui.menus[m].items[field].rect;
        ui.mouse_move(r[0] + 2.0, r[1] + 2.0, 640.0, 480.0, &shell);
        assert!(!ui.editing());
        ui.key(KeyCode::Enter, &shell);
        assert_eq!(ui.options.editing, Some((m, field)));
        // The middle button too.
        ui.options.editing = None;
        ui.mouse_button(K_MOUSE3, Instant::now(), &shell);
        assert_eq!(ui.options.editing, Some((m, field)));
    }

    #[test]
    fn on_close_runs_before_the_menu_leaves_the_stack() {
        let Some(mut ui) = ui() else { return };
        let shell = Shell::new();
        ui.open_main(&shell, &mut Vec::new());
        ui.open_menu("mods_menu", &shell, &mut Vec::new());
        let (main, mods) = (ui.find("main").unwrap(), ui.find("mods_menu").unwrap());
        // A spot on an item of mods_menu where no item of main takes the mouse.
        let free = |ui: &Ui, p: [f32; 2]| {
            !(0..ui.menus[main].items.len()).any(|i| {
                !ui.menus[main].items[i].decoration
                    && ui.shown(main, i, &shell)
                    && contains(ui.menus[main].items[i].rect, p)
            })
        };
        let (i, at) = (0..ui.menus[mods].items.len())
            .filter(|&i| !ui.menus[mods].items[i].decoration && ui.shown(mods, i, &shell))
            .map(|i| {
                let r = ui.menus[mods].items[i].rect;
                (i, [r[0] + r[2] / 2.0, r[1] + r[3] / 2.0])
            })
            .find(|&(_, p)| free(&ui, p))
            .unwrap();
        ui.mouse_move(at[0], at[1], 640.0, 480.0, &shell);
        ui.over.clear();
        ui.menus[mods].items[i].mouse_enter = vec!["play".into(), "mods enter".into()];
        // `open main` in the onClose runs `Display_MouseMove` with the
        // closing menu still up; a `close` of itself there does nothing.
        ui.menus[mods].on_close = ["open", "main", ";", "close", "mods_menu", ";", "play", "c"]
            .map(String::from)
            .to_vec();
        let mut out = Vec::new();
        ui.close_menu("mods_menu", &shell, &mut out);
        let sounds: Vec<_> = (out.iter())
            .filter(|e| matches!(e, UiEffect::Sound(s) if s == "c" || s == "mods enter"))
            .cloned()
            .collect();
        assert_eq!(
            sounds,
            vec![
                UiEffect::Sound("mods enter".into()),
                UiEffect::Sound("c".into())
            ]
        );
        assert_eq!(ui.open, vec![main]);
        assert_eq!(ui.focused_menu(), Some(main));
    }

    #[test]
    fn an_out_of_bounds_click_closes_the_focused_menu_first() {
        let Some(mut ui) = ui() else { return };
        let shell = Shell::new();
        ui.open_main(&shell, &mut Vec::new());
        ui.open_menu("mods_menu", &shell, &mut Vec::new());
        let (main, mods) = (ui.find("main").unwrap(), ui.find("mods_menu").unwrap());
        ui.menus[mods].out_of_bounds_click = true;
        ui.menus[mods].on_close = vec!["play".into(), "mods close".into()];
        let join = (ui.menus[main].items.iter())
            .position(|i| i.text == "@MENU_JOIN_GAME")
            .unwrap();
        ui.menus[main].items[join].action = vec!["play".into(), "join".into()];
        let r = ui.menus[main].items[join].rect;
        // A click inside the menu leaves it open.
        let mr = ui.menus[mods].rect;
        ui.mouse_move(mr[0] + 1.0, mr[1] + 1.0, 640.0, 480.0, &shell);
        ui.click(Instant::now(), &shell);
        assert_eq!(ui.open, vec![main, mods]);
        ui.mouse_move(r[0] + 2.0, r[1] + 2.0, 640.0, 480.0, &shell);
        let out = ui.click(Instant::now(), &shell);
        let at = |s: &str| out.iter().position(|e| *e == UiEffect::Sound(s.into()));
        assert!(at("mods close").unwrap() < at("join").unwrap(), "{out:?}");
        assert_eq!(ui.open, vec![main]);
        assert_eq!(ui.focused_menu(), Some(main));
    }

    /// `Menus_HandleOOBClick` leaves the closed menu on the stack, hidden:
    /// a menu opened over it and closed hands the focus to it, so no menu
    /// has the keys, a `Menus_CloseAll` runs no `onClose` of it, and an
    /// `open` shows it on top again.
    #[test]
    fn an_out_of_bounds_closed_menu_stays_on_the_stack_hidden() {
        let Some(mut ui) = ui() else { return };
        let shell = Shell::new();
        ui.open_main(&shell, &mut Vec::new());
        ui.open_menu("mods_menu", &shell, &mut Vec::new());
        let (main, mods) = (ui.find("main").unwrap(), ui.find("mods_menu").unwrap());
        ui.menus[mods].out_of_bounds_click = true;
        ui.menus[mods].on_close = vec!["play".into(), "mods close".into()];
        let quit = ui.find("quit_popmenu").unwrap();
        let mr = ui.menus[mods].rect;
        ui.mouse_move(
            mr[0] + mr[2] + 5.0,
            mr[1] + mr[3] + 5.0,
            640.0,
            480.0,
            &shell,
        );
        ui.click(Instant::now(), &shell);
        assert_eq!(ui.open, vec![main]);
        assert_eq!(ui.hidden, vec![(mods, 1)]);

        // Opened over the hidden menu and closed, the popup hands the
        // focus to the stack's top: the hidden menu, so nothing has it.
        ui.open_menu("quit_popmenu", &shell, &mut Vec::new());
        ui.close_menu("quit_popmenu", &shell, &mut Vec::new());
        assert_eq!(ui.open, vec![main]);
        assert_eq!(ui.focused_menu(), None);
        // A shown menu under it closing hands it on the same way.
        ui.open_menu("quit_popmenu", &shell, &mut Vec::new());
        assert_eq!(ui.stack_top(), Some(quit));
        ui.focus_menu = Some(main);
        ui.close_menu("main", &shell, &mut Vec::new());
        assert_eq!(ui.focus_menu, Some(quit));

        let mut out = Vec::new();
        ui.close_every_menu(&shell, &mut out);
        assert!(
            !out.contains(&UiEffect::Sound("mods close".into())),
            "{out:?}"
        );
        assert!(ui.hidden.is_empty() && ui.open.is_empty());

        ui.open_main(&shell, &mut Vec::new());
        ui.open_menu("mods_menu", &shell, &mut Vec::new());
        ui.mouse_move(
            mr[0] + mr[2] + 5.0,
            mr[1] + mr[3] + 5.0,
            640.0,
            480.0,
            &shell,
        );
        ui.click(Instant::now(), &shell);
        ui.open_menu("mods_menu", &shell, &mut Vec::new());
        assert_eq!(ui.open, vec![main, mods]);
        assert!(ui.hidden.is_empty());
        assert_eq!(ui.focused_menu(), Some(mods));
    }

    /// `_UI_KeyEvent` with no focused menu takes the UI down on the next
    /// key or release and leaves the stack as it was; the next open brings
    /// the UI back with the old menus still up.
    #[test]
    fn a_key_with_no_focused_menu_takes_the_ui_down() {
        let Some(mut ui) = ui() else { return };
        let shell = Shell::new();
        ui.open_main(&shell, &mut Vec::new());
        ui.open_menu("mods_menu", &shell, &mut Vec::new());
        let (main, mods) = (ui.find("main").unwrap(), ui.find("mods_menu").unwrap());
        ui.menus[mods].out_of_bounds_click = true;
        let mr = ui.menus[mods].rect;
        ui.mouse_move(
            mr[0] + mr[2] + 5.0,
            mr[1] + mr[3] + 5.0,
            640.0,
            480.0,
            &shell,
        );
        ui.click(Instant::now(), &shell);
        ui.open_menu("quit_popmenu", &shell, &mut Vec::new());
        ui.close_menu("quit_popmenu", &shell, &mut Vec::new());
        assert_eq!(ui.focused_menu(), None);
        assert!(ui.active());

        let (used, out) = ui.key(KeyCode::Tab, &shell);
        assert!(used && out.is_empty(), "the key goes nowhere: {out:?}");
        assert!(!ui.active(), "the UI is down");
        assert_eq!(ui.open, vec![main], "the menus stay flagged visible");
        assert_eq!(ui.hidden, vec![(mods, 1)]);

        ui.open_menu("quit_popmenu", &shell, &mut Vec::new());
        assert!(ui.active());
        assert_eq!(ui.open, vec![main, ui.find("quit_popmenu").unwrap()]);

        // A release reaches `_UI_KeyEvent` too.
        ui.focus_menu = None;
        ui.key_up(&shell);
        assert!(!ui.active());
    }

    #[test]
    fn an_out_of_bounds_click_off_the_last_menu_leaves_the_ui_down() {
        let Some(mut ui) = ui() else { return };
        let shell = Shell::new();
        ui.open_main(&shell, &mut Vec::new());
        let m = ui.find("main").unwrap();
        ui.menus[m].out_of_bounds_click = true;
        ui.menus[m].rect = [100.0, 100.0, 10.0, 10.0];
        ui.mouse_move(300.0, 300.0, 640.0, 480.0, &shell);
        ui.click(Instant::now(), &shell);
        // No menu is visible, so `UI_Pause(0)` drops the UI key catcher;
        // the app takes the mouse back on an inactive UI.
        assert!(!ui.active());
    }

    /// The main menu with Start New Server turned into a text item whose
    /// `action` plays "start", the cursor off every item and the menu
    /// painted once.
    fn start_as_text(ui: &mut Ui, shell: &Shell) -> (usize, usize) {
        let (m, start) = main_at_start(ui, shell);
        ui.mouse_move(0.0, 479.0, 640.0, 480.0, shell);
        let item = &mut ui.menus[m].items[start];
        item.kind = ITEM_TYPE_TEXT;
        item.action = vec!["play".into(), "start".into()];
        assert_eq!(ui.text_rect(m, start), None, "never painted");
        ui.build(640.0, 480.0, &Localized::default(), shell);
        (m, start)
    }

    #[test]
    fn a_text_item_takes_the_focus_and_the_click_on_its_text_only() {
        let Some(mut ui) = ui() else { return };
        let shell = Shell::new();
        let (m, start) = start_as_text(&mut ui, &shell);
        let r = ui.menus[m].items[start].rect;
        let t = ui.text_rect(m, start).unwrap();
        assert!(t[2] > 0.0 && t[3] > 0.0, "{t:?}");
        let corners = [
            [r[0], r[1]],
            [r[0] + r[2], r[1]],
            [r[0], r[1] + r[3]],
            [r[0] + r[2], r[1] + r[3]],
        ];
        let off = corners.into_iter().find(|&p| !contains(t, p)).unwrap();
        let on = [t[0] + t[2] / 2.0, t[1] + t[3] / 2.0];
        let started = |out: &[UiEffect]| out.contains(&UiEffect::Sound("start".into()));
        // Inside the rect but off the text: no focus, no click.
        ui.mouse_move(off[0], off[1], 640.0, 480.0, &shell);
        assert_ne!(ui.focus.get(&m), Some(&start));
        assert!(!started(&ui.click(Instant::now(), &shell)));
        // On the text it takes both.
        ui.mouse_move(on[0], on[1], 640.0, 480.0, &shell);
        assert_eq!(ui.focus.get(&m), Some(&start));
        assert!(started(&ui.click(Instant::now(), &shell)));
        // Off the text again it keeps the focus, as the rect still holds
        // the cursor, but the click tests the text.
        ui.mouse_move(off[0], off[1], 640.0, 480.0, &shell);
        assert_eq!(ui.focus.get(&m), Some(&start));
        assert!(!started(&ui.click(Instant::now(), &shell)));
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
            (ui.build(640.0, 480.0, &loc, &shell).iter())
                .filter(|q| q.rgba == focus_color)
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

    #[test]
    fn max_chars_goto_next_steps_a_full_field_on() {
        let Some(mut ui) = ui() else { return };
        let mut shell = Shell::new();
        ui.open_menu("createfavorite_popmenu", &Shell::new(), &mut Vec::new());
        let m = ui.find("createfavorite_popmenu").unwrap();
        let fields: Vec<usize> = (0..ui.menus[m].items.len())
            .filter(|&i| ui.menus[m].items[i].kind == ui_menu::ITEM_TYPE_EDITFIELD)
            .collect();
        let [name, addr] = fields[..] else {
            panic!("two fields")
        };
        ui.menus[m].items[name].max_chars = 2;
        ui.menus[m].items[name].max_chars_goto_next = true;
        click_at(&mut ui, m, name, &mut shell);
        for (code, c) in [(KeyCode::KeyA, "a"), (KeyCode::KeyB, "b")] {
            assert_eq!(ui.options.editing, Some((m, name)));
            let out = ui.edit_key(code, Some(c), &shell);
            apply(&mut shell, &out);
        }
        assert_eq!(ui.options.editing, Some((m, addr)));
        let out = ui.edit_key(KeyCode::KeyC, Some("c"), &shell);
        apply(&mut shell, &out);
        assert_eq!(shell.cvar("ui_favoriteName"), Some("ab"));
        assert_eq!(shell.cvar("ui_favoriteAddress"), Some("c"));
    }

    #[test]
    fn a_menu_ignores_key_repeats() {
        let Some(mut ui) = ui() else { return };
        let shell = Shell::new();
        let (m, start) = main_at_start(&mut ui, &shell);
        for code in [KeyCode::Escape, KeyCode::Enter, KeyCode::ArrowDown] {
            assert!(ui.key_repeat(code, None, &shell).is_empty());
        }
        assert_eq!(top_name(&ui), "main");
        assert_eq!(ui.focus.get(&m), Some(&start));
    }

    #[test]
    fn the_server_lists_keys_move_its_selection_under_the_cursor_and_left_steps_the_focus() {
        let Some(mut ui) = ui() else { return };
        let shell = Shell::new();
        ui.open_main(&Shell::new(), &mut Vec::new());
        click_item(&mut ui, "main", "@MENU_JOIN_GAME", &shell);
        let top = *ui.open.last().unwrap();
        ui.browser.source = browser::Source::Favorites;
        for i in 0..40 {
            ui.browser.add_favorite("", &format!("10.0.0.{i}:28960"));
        }
        let list = (ui.menus[top].items.iter())
            .position(|i| i.feeder == Some(FEEDER_SERVERS))
            .unwrap();
        let item = ui.menus[top].items[list].clone();
        let page = listbox::view_max(&item);
        let row = |ui: &Ui| {
            let rows = ui.browser.rows();
            rows.iter().position(|s| Some(s.addr) == ui.selected)
        };
        // Off the list the arrows step the focus.
        ui.mouse_move(1.0, 1.0, 640.0, 480.0, &shell);
        assert!(ui.key(KeyCode::ArrowDown, &shell).0);
        assert_eq!(row(&ui), None);
        assert!(ui.focus.contains_key(&top), "a focus step");
        let r = item.rect;
        ui.mouse_move(r[0] + 40.0, r[1] + 40.0, 640.0, 480.0, &shell);
        assert!(ui.key(KeyCode::ArrowDown, &shell).0);
        assert_eq!(row(&ui), Some(0), "the first arrow picks the first row");
        ui.key(KeyCode::ArrowDown, &shell);
        assert_eq!(row(&ui), Some(1));
        ui.key(KeyCode::PageDown, &shell);
        assert_eq!(row(&ui), Some(1 + page), "a page moves the selection");
        assert!(ui.list_top <= 1 + page && 1 + page < ui.list_top + visible_rows(&item));
        ui.key(KeyCode::End, &shell);
        assert_eq!(row(&ui), Some(39));
        ui.key(KeyCode::PageUp, &shell);
        assert_eq!(row(&ui), Some(39 - page));
        assert_eq!(ui.focus.get(&top), Some(&list));
        ui.key(KeyCode::ArrowLeft, &shell);
        assert_ne!(ui.focus.get(&top), Some(&list), "no list takes left");
    }

    #[test]
    fn esc_on_a_game_type_page_leaves_nothing_clickable_under_main() {
        let Some(mut ui) = ui() else { return };
        let mut shell = Shell::new();
        let mut out = Vec::new();
        ui.open_main(&Shell::new(), &mut out);
        ui.frame(Instant::now(), &shell);
        let out = click_item(&mut ui, "main", "@MENU_START_NEW_SERVER", &shell);
        apply(&mut shell, &out);
        ui.frame(Instant::now(), &shell);
        // `createserver`'s `onClose` closes `settings_dm` but no other page.
        let op = ui.find("createserver_op").unwrap();
        let gt = (0..ui.menus[op].items.len())
            .find(|&i| ui.menus[op].items[i].ownerdraw == UI_NETGAMETYPE)
            .unwrap();
        click_at(&mut ui, op, gt, &mut shell);
        click_named(
            &mut ui,
            "createserver_op",
            "button_serversettings",
            &mut shell,
        );
        assert_eq!(top_name(&ui), "settings_re");
        let (_, out) = ui.key(KeyCode::Escape, &shell);
        apply(&mut shell, &out);
        assert_eq!(top_name(&ui), "main");
        // `onEsc` opens main without closing the page; main hides it.
        let page = ui.find("settings_re").unwrap();
        assert!(ui.open.contains(&page));
        let r = ui.menus[page].items[0].rect;
        ui.mouse_move(r[0] + 2.0, r[1] + 2.0, 640.0, 480.0, &shell);
        assert!(ui.hover.is_none_or(|(m, _)| m != page));
        // The mouse still reaches the hidden page where main has no item,
        // as `Display_MouseMove` walks every open menu; a click does not.
        let main = ui.find("main").unwrap();
        let found = (0..ui.menus[page].items.len()).any(|i| {
            let r = ui.menus[page].items[i].rect;
            ui.mouse_move(r[0] + 2.0, r[1] + 2.0, 640.0, 480.0, &shell);
            ui.item_at(main, &shell).is_none() && ui.item_at(page, &shell).is_some()
        });
        assert!(found, "a page item main does not cover");
        let i = ui.item_at(page, &shell).unwrap();
        assert!(ui.over.contains(&(page, i)));
        assert!(ui.hover.is_none_or(|(m, _)| m != page));
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
        ui.open_menu("zmenu", &Shell::new(), &mut out);
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
            ui.open_menu("a", &Shell::new(), &mut out);
            out
        };
        let mut ui = Ui::new(&fs, "0");
        assert!(ui.has_menu("quickmessage"));
        assert_eq!(opened(&mut ui), [UiEffect::Command("plain_a".into())]);
        let mut ui = Ui::new(&fs, "2");
        assert_eq!(opened(&mut ui), [UiEffect::Command("german_a".into())]);
        ui.reload(&fs, "", "2", &Shell::new(), &mut Vec::new());
        assert!(
            !ui.has_menu("quickmessage"),
            "ui_load drops ingame.txt, so mp_QuickMessage opens nothing"
        );
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
        ui.open_menu("a", &Shell::new(), &mut Vec::new());
        let mut out = Vec::new();
        ui.reload(&fs, "ui_mp/alt.txt", "0", &Shell::new(), &mut out);
        assert!(ui.find("b").is_some());
        assert_eq!(top_name(&ui), "a");
        assert_eq!(out, [UiEffect::Command("alt_a".into())]);
        out.clear();
        ui.reload(&fs, "ui_mp/missing.txt", "0", &Shell::new(), &mut out);
        assert!(ui.find("b").is_none(), "back on the stock list");
        assert_eq!(out, [UiEffect::Command("stock_a".into())]);
    }

    #[test]
    fn the_mods_list_scrolls_by_its_bar_and_a_held_arrow_repeats() {
        let Some(ui) = ui() else { return };
        let root = tempfile::tempdir().unwrap();
        for n in 0..16 {
            let dir = root.path().join(format!("mod{n:02}"));
            std::fs::create_dir_all(&dir).unwrap();
            let mut w = zip::ZipWriter::new(std::fs::File::create(dir.join("z.pk3")).unwrap());
            w.start_file("a.txt", zip::write::SimpleFileOptions::default())
                .unwrap();
            w.finish().unwrap();
        }
        let mut ui = ui.with_mod_root(root.path().to_path_buf(), "main".into());
        let shell = Shell::new();
        ui.open_main(&Shell::new(), &mut Vec::new());
        click_item(&mut ui, "main", "@MENU_MODS", &shell);
        assert_eq!(ui.mods.len(), 16);
        let m = ui.find("mods_menu").unwrap();
        let item = ui.menus[m]
            .items
            .iter()
            .find(|i| i.feeder == Some(FEEDER_MODS))
            .unwrap()
            .clone();
        let max = listbox::max_scroll(&item, 16);
        assert!(
            max >= 3,
            "the stock list holds {} rows",
            listbox::view_max(&item)
        );
        let [x, y, w, h] = item.rect;
        // The down arrow steps on the press, again 500 ms on, and stops at
        // the release.
        let t0 = Instant::now();
        ui.mouse_move(x + w - 8.0, y + h - 4.0, 640.0, 480.0, &shell);
        ui.click(t0, &shell);
        assert_eq!(ui.mod_top, 1);
        ui.frame(t0 + Duration::from_millis(490), &shell);
        assert_eq!(ui.mod_top, 1);
        // Off the arrow the capture still steps it.
        ui.mouse_move(x + 10.0, y + 10.0, 640.0, 480.0, &shell);
        ui.frame(t0 + Duration::from_millis(510), &shell);
        assert_eq!(ui.mod_top, 2);
        ui.release();
        ui.frame(t0 + Duration::from_millis(2000), &shell);
        assert_eq!(ui.mod_top, 2);
        assert_eq!(ui.mod_selected, None, "the bar picks no row");
        // A row click counts from the first row drawn.
        ui.click(t0 + Duration::from_secs(3), &shell);
        assert_eq!(ui.mod_selected, Some(2));
        assert!(ui.key(KeyCode::End, &shell).0);
        assert_eq!(ui.mod_selected, Some(15));
        assert_eq!(ui.mod_top, 16 - listbox::view_max(&item));
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
        ui.open_main(&Shell::new(), &mut Vec::new());
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

    /// A UI over one menu file, `ui_mp/m.menu`, holding `body`.
    fn ui_with_menu(body: &str) -> Ui {
        let mut fs = Pk3Fs::empty();
        fs.overlay(
            "ui_mp/menus.txt",
            br#"{ loadMenu { "ui_mp/m.menu" } }"#.to_vec(),
        );
        fs.overlay("ui_mp/m.menu", body.as_bytes().to_vec());
        Ui::new(&fs, "0")
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

    /// The client builds its UI with a server cache and restarts it with
    /// the old lists: both keep the loaded menu's nine columns (issue #3,
    /// a 1.5 list drawn in 1.1's six).
    #[test]
    fn a_new_server_cache_or_carried_lists_keep_the_menus_columns() {
        let menu = r#"{ menuDef { name "joinserver" rect 0 0 640 480
              itemDef { name serverlist rect 9 145 620 302 type 6 elementheight 15
                feeder 2 visible 1
                columns 9 2 20 20  22 14 14  41 40 40  306 40 20  420 5 10
                          484 20 10  528 14 14  548 20 10  568 20 20 } } }"#;
        let mut ui =
            ui_with_menu(menu).with_server_cache(PathBuf::from("/nonexistent/servercache.dat"));
        assert_eq!(ui.browser.columns, &browser::COLUMNS_V5[..]);
        ui.set_browser(Browser::default());
        assert_eq!(ui.browser.columns, &browser::COLUMNS_V5[..]);
    }

    /// 1.5's nine-column list: the hardware and PunkBuster columns draw
    /// icons, a column-wide square from half a row down.
    #[test]
    fn a_15_server_list_draws_the_hardware_and_punkbuster_icons() {
        let mut ui = ui_with_menu(
            r#"{ menuDef { name "joinserver" rect 0 0 640 480
              itemDef { name serverlist rect 9 145 620 302 type 6 elementheight 15
                feeder 2 visible 1
                columns 9 2 20 20  22 14 14  41 40 40  306 40 20  420 5 10
                          484 20 10  528 14 14  548 20 10  568 20 20 } } }"#,
        );
        assert_eq!(ui.browser.columns, &browser::COLUMNS_V5[..]);
        let addr: SocketAddrV4 = "10.0.0.1:28960".parse().unwrap();
        let mut s = browser::Server::new(addr);
        s.info = Some(vcod_common::net::master::ServerInfo {
            hostname: "pb".into(),
            protocol: 6,
            punkbuster: true,
            hardware: 1,
            ..Default::default()
        });
        s.ping = Some(30);
        ui.browser.lists[0].push(s);
        let shell = Shell::new();
        ui.open_menu("joinserver", &shell, &mut Vec::new());
        let quads = ui.build(640.0, 480.0, &Localized::default(), &shell);
        let at = |tex: &str| {
            let q = quads.iter().find(|q| q.texture == tex).unwrap();
            (q.verts[0], q.verts[2])
        };
        // x 9 + 1 + 4 + pos, y 145 + 1 - 1 + 15 / 2.
        assert_eq!(
            at("ui_mp/assets/server_hardware_linux_dedicated"),
            ([36.0, 152.5], [50.0, 166.5])
        );
        assert_eq!(
            at("ui_mp/assets/punkbusterlogo"),
            ([542.0, 152.5], [556.0, 166.5])
        );
    }

    /// `viewmap`'s layout image: the window's shader is the cvar's value,
    /// inside the border, and nothing else of the item paints.
    #[test]
    fn a_cvar_shader_item_paints_its_window_with_the_cvars_shader() {
        let mut ui = ui_with_menu(
            r#"{ menuDef { name "main" rect 0 0 640 480
            itemDef { visible 1 rect 64 32 512 416 style 6
              cvar "scr_layoutimage" border 1 bordersize 2 bordercolor 1 0 0 1 decoration } } }"#,
        );
        let shell = Shell::new();
        let loc = Localized::default();
        ui.open_main(&shell, &mut Vec::new());
        let borders = ui.build(640.0, 480.0, &loc, &shell);
        assert_eq!(borders.len(), 4);
        ui.set_server_cvars(HashMap::from([(
            "scr_layoutimage".into(),
            "levelshots/layouts/x".into(),
        )]));
        let quads = ui.build(640.0, 480.0, &loc, &shell);
        assert_eq!(quads.len(), 5);
        assert_eq!(quads[0].texture, "levelshots/layouts/x");
        assert_eq!(quads[0].verts[0], [66.0, 34.0]);
    }

    fn response(menu: &str, response: &str) -> UiEffect {
        UiEffect::ScriptMenuResponse {
            menu: menu.into(),
            response: response.into(),
        }
    }

    /// A UI over a pak holding one script menu shaped like the stock team
    /// menus, loaded as the cgame loads the precached ones.
    fn ui_with_team_menu() -> Ui {
        let mut fs = Pk3Fs::empty();
        fs.overlay(
            "ui_mp/scriptmenus/team_x.menu",
            br#"{ menuDef { name "team_x" fullscreen 0 rect 0 0 640 480
      onEsc { scriptMenuResponse "close"; close team_x; }
      onOpen { scriptMenuResponse "open"; }
      onClose { scriptMenuResponse "close"; }
      itemDef { name "button_allies" visible 1 rect 16 24 160 23 origin 96 96 type 1 text "Allies"
        action { play "mouse_click"; scriptMenuResponse "allies"; close team_x; } }
      execKey "1" { play "mouse_click"; scriptMenuResponse "allies"; close team_x }
      itemDef { name "button_tab" visible 1 rect 0 0 90 32 origin 96 64 type 1 text "Weapon"
        cvartest "scr_showweapontab" showCvar { "1" }
        action { scriptMenuResponse "weapon"; close team_x; } }
    } }"#
                .to_vec(),
        );
        let mut ui = Ui::new(&fs, "0");
        assert!(
            ui.load_script_menus(&fs, &["team_x".into()], "0")
                .is_empty()
        );
        ui
    }

    #[test]
    fn a_script_menus_asset_global_def_sets_the_cursor() {
        let mut fs = Pk3Fs::empty();
        fs.overlay(
            "ui_mp/scriptmenus/team_x.menu",
            br#"{ assetGlobalDef { cursor "ui/assets/mod_cursor" }
      menuDef { name "team_x" rect 0 0 640 480 } }"#
                .to_vec(),
        );
        let mut ui = Ui::new(&fs, "0");
        assert!(
            ui.load_script_menus(&fs, &["team_x".into()], "0")
                .is_empty()
        );
        let shell = Shell::new();
        ui.open_script_popup("team_x", false, &shell, &mut Vec::new());
        let quads = ui.build(640.0, 480.0, &Localized::default(), &shell);
        assert_eq!(quads.last().unwrap().texture, "ui/assets/mod_cursor");
    }

    /// UI `vmMain` 4: the cursor starts where the `t` put it and moves by
    /// the raw deltas, clamped to the 640x480 screen.
    #[test]
    fn the_cursor_moves_by_clamped_deltas_from_where_the_t_put_it() {
        let mut ui = ui_with_team_menu();
        let shell = Shell::new();
        ui.open_script_popup("team_x", false, &shell, &mut Vec::new());
        ui.mouse_delta(-200.0, -110.0, &shell);
        assert_eq!(ui.cursor, [120.0, 130.0]);
        let out = ui.click(Instant::now(), &shell);
        assert!(out.contains(&response("team_x", "allies")), "{out:?}");
        ui.mouse_delta(5000.0, -5000.0, &shell);
        assert_eq!(ui.cursor, [640.0, 0.0]);
        ui.mouse_delta(-5000.0, 5000.0, &shell);
        assert_eq!(ui.cursor, [0.0, 480.0]);
    }

    #[test]
    fn a_t_opens_the_script_menu_and_a_click_sends_its_response() {
        let mut ui = ui_with_team_menu();
        let shell = Shell::new();
        let mut out = Vec::new();
        assert!(ui.open_script_popup("team_x", false, &shell, &mut out));
        assert_eq!(out, [response("team_x", "open")]);
        assert_eq!(ui.cursor, [320.0, 240.0]);
        assert_eq!(top_name(&ui), "team_x");
        // The button's rect, offset by its origin.
        ui.mouse_move(120.0, 130.0, 640.0, 480.0, &shell);
        let out = ui.click(Instant::now(), &shell);
        assert_eq!(
            out,
            [
                UiEffect::Sound("mouse_click".into()),
                response("team_x", "allies"),
                response("team_x", "close"),
            ]
        );
        assert!(!ui.active());
        // The no-mouse form parks the cursor in the corner.
        ui.open_script_popup("team_x", true, &shell, &mut Vec::new());
        assert_eq!(ui.cursor, [639.0, 479.0]);
        // `u` is an Esc; a script menu is no fullscreen menu, so every menu
        // closes through its `onClose`, not its `onEsc`.
        assert_eq!(ui.close_script_popup(&shell), [response("team_x", "close")]);
        assert!(!ui.active());
    }

    #[test]
    fn a_tab_hidden_by_a_server_cvar_takes_no_click() {
        let mut ui = ui_with_team_menu();
        let shell = Shell::new();
        ui.open_script_popup("team_x", false, &shell, &mut Vec::new());
        let m = ui.find("team_x").unwrap();
        assert!(!ui.shown(m, 1, &shell));
        ui.mouse_move(100.0, 70.0, 640.0, 480.0, &shell);
        assert!(ui.click(Instant::now(), &shell).is_empty());
        ui.set_server_cvars(HashMap::from([("SCR_SHOWWEAPONTAB".into(), "1".into())]));
        assert!(ui.shown(m, 1, &shell));
        ui.mouse_move(101.0, 70.0, 640.0, 480.0, &shell);
        let out = ui.click(Instant::now(), &shell);
        assert_eq!(out[0], response("team_x", "weapon"));
    }

    #[test]
    fn a_t_waits_while_a_menu_esc_opened_has_the_focus() {
        let mut ui = ui_with_team_menu();
        let shell = Shell::new();
        // Esc in a live game opened it as `UIMENU_INGAME`: the `t` is refused.
        ui.open_script_main("team_x", &shell, &mut Vec::new());
        assert!(!ui.open_script_popup("team_x", false, &shell, &mut Vec::new()));
        // An exec key answers it as a click does.
        let (used, out) = ui.key(KeyCode::Digit1, &shell);
        assert!(used);
        assert!(out.contains(&response("team_x", "allies")));
        // With nothing up the `t` opens, and the same `t` again changes
        // nothing.
        assert!(ui.open_script_popup("team_x", false, &shell, &mut Vec::new()));
        let mut out = Vec::new();
        assert!(ui.open_script_popup("team_x", false, &shell, &mut out));
        assert!(out.is_empty());
    }

    #[test]
    fn the_stock_weapon_menu_hides_a_disallowed_weapon() {
        let Some(fs) = vcod_common::testing::stock_fs_11() else {
            return;
        };
        let mut ui = Ui::new(&fs, "0");
        let names = ["team_americangerman".into(), "weapon_american".into()];
        assert!(ui.load_script_menus(&fs, &names, "0").is_empty());
        let shell = Shell::new();
        ui.set_server_cvars(HashMap::from([("scr_allow_m1carbine".into(), "0".into())]));
        ui.open_script_popup("weapon_american", false, &shell, &mut Vec::new());
        let m = ui.find("weapon_american").unwrap();
        let item = |name: &str| ui.menus[m].items.iter().position(|i| i.name == name);
        let carbine = item("button_m1carbine").unwrap();
        assert!(!ui.shown(m, carbine, &shell));
        assert!(ui.shown(m, item("button_m1garand").unwrap(), &shell));
        // Its window and the M1 Carbine's picture (onOpen's `show`) draw.
        let quads = ui.build(640.0, 480.0, &Localized::default(), &shell);
        let drawn = |t: &str| {
            let style3 = crate::hud::default_if_missing(t);
            quads.iter().any(|q| q.texture == t || q.texture == style3)
        };
        assert!(drawn("ui_mp/assets/hud@window_background.tga"));
        assert!(drawn("ui_mp/assets/hud@m1carbine.tga"));
        // The Main Menu tab opens `main` over the game.
        click_item(&mut ui, "weapon_american", "@MPMENU_MAIN_MENU", &shell);
        assert_eq!(top_name(&ui), "main");
        assert_eq!(ui.cvar("cl_ingame", &shell).as_deref(), Some("1"));
    }

    /// A retail `--save-configstrings` capture, one `<index> <value>` line
    /// per slot, as a gamestate's table.
    fn capture_configstrings(name: &str) -> Vec<String> {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../server/tests/fixtures/configstrings")
            .join(name);
        let text = std::fs::read_to_string(&path).unwrap();
        let mut cs = vec![String::new(); 2048];
        for line in text.lines().filter(|l| !l.starts_with('#')) {
            let (i, v) = line.split_once(' ').unwrap_or((line, ""));
            cs[i.parse::<usize>().unwrap()] = v.to_string();
        }
        cs
    }

    /// The motd a player reads is the script menu's `text_motd`: `scr_motd`
    /// in 1.1's team menu, `ui_motd` (the server's `scr_motd`) in 1.5's
    /// and UO's `serverinfo_<gametype>`, both off the 140/204 mirror.
    /// `g_motd`'s configstring (14, UO 15) reaches no menu.
    fn shows_the_mirrors_motd(
        fs: &Pk3Fs,
        proto: &vcod_common::net::protocol::Protocol,
        capture: &str,
        menu: &str,
    ) {
        let cs = capture_configstrings(capture);
        assert_eq!(cs[proto.cs.motd], "Welcome to vcod");
        let mut cvars = crate::play::cvars::ClientCvars::default();
        cvars.set_layout(proto.cs);
        let mut ui = Ui::new(fs, "0");
        assert!(ui.load_script_menus(fs, &[menu.into()], "0").is_empty());
        let shell = Shell::new();
        ui.set_server_cvars(cvars.all(&cs, &shell));
        assert!(ui.open_script_popup(menu, false, &shell, &mut Vec::new()));
        let m = ui.find(menu).unwrap();
        let i = (ui.menus[m].items.iter())
            .position(|i| i.name == "text_motd")
            .unwrap();
        let loc = Localized::default();
        let item = &ui.menus[m].items[i];
        assert!(item.autowrap && item.decoration, "{menu}");
        assert_eq!(ui.label(item, &loc, &shell), "scr motd text", "{menu}");
        ui.build(640.0, 480.0, &loc, &shell);
        assert!(ui.text_rect(m, i).is_some(), "{menu}: not painted");
    }

    #[test]
    fn the_team_menu_shows_scr_motd_on_1_1() {
        let Some(fs) = vcod_common::testing::stock_fs_11() else {
            return;
        };
        use vcod_common::net::protocol::PROTOCOL_V1;
        for menu in [
            "team_americangerman",
            "team_britishgerman",
            "team_russiangerman",
            "team_germanonly",
        ] {
            shows_the_mirrors_motd(&fs, &PROTOCOL_V1, "mp_carentan-dm-motd.txt", menu);
        }
    }

    #[test]
    fn the_serverinfo_menu_shows_ui_motd_on_1_5_and_uo() {
        use vcod_common::net::protocol::{
            PROTOCOL_V1, PROTOCOL_V6, PROTOCOL_V22, install_protocol,
        };
        if let Some(fs) = vcod_common::testing::stock_fs()
            && install_protocol(&fs).transport.version != PROTOCOL_V1.transport.version
        {
            for gt in ["dm", "tdm", "sd", "re", "bel", "hq"] {
                let menu = format!("serverinfo_{gt}");
                shows_the_mirrors_motd(&fs, &PROTOCOL_V6, "mp_carentan-dm-motd-15.txt", &menu);
            }
        }
        if let Some(fs) = vcod_common::testing::uo_fs() {
            for gt in ["dm", "tdm", "sd", "re", "bel", "hq", "ctf", "dom", "bas"] {
                let menu = format!("serverinfo_{gt}");
                shows_the_mirrors_motd(&fs, &PROTOCOL_V22, "mp_cassino-dm-motd-uo.txt", &menu);
            }
        }
    }

    #[test]
    fn the_quick_message_menu_opens_the_quick_commands_and_lets_other_keys_by() {
        let Some(fs) = vcod_common::testing::stock_fs() else {
            return;
        };
        let mut ui = Ui::new(&fs, "0");
        assert!(
            ui.load_script_menus(&fs, &["quickcommands".into()], "0")
                .is_empty()
        );
        let shell = Shell::new();
        let mut out = Vec::new();
        assert!(ui.open_quick_message(&shell, &mut out));
        assert!(out.contains(&UiEffect::Command("set cl_bypassMouseInput \"1\"".into())));
        assert!(ui.wants_key(KeyCode::Digit1));
        assert!(!ui.wants_key(KeyCode::KeyW));
        ui.key(KeyCode::Digit1, &shell);
        assert_eq!(top_name(&ui), "quickcommands");
        let (_, out) = ui.key(KeyCode::Digit1, &shell);
        assert_eq!(out[0], response("quickcommands", "1"));
        assert!(!ui.active());
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
        ui.open_menu("joinserver", &Shell::new(), &mut Vec::new());
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
        ui.open_menu("joinserver", &Shell::new(), &mut Vec::new());
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

    #[test]
    fn the_game_type_owner_draw_cycles_the_filter_through_all_and_the_scripts() {
        let Some(mut ui) = ui() else { return };
        let mut shell = Shell::new();
        let mut out = Vec::new();
        ui.open_menu("joinserver", &Shell::new(), &mut out);
        ui.frame(Instant::now(), &shell);
        assert_eq!(ui.browser.filter.game_type, None);
        let out = click_item(&mut ui, "joinserver", "@MENU_GAME_TYPE", &shell);
        assert!(out.contains(&UiEffect::Command("set ui_joinGametype \"1\"".into())));
        shell.execute("set ui_joinGametype 1");
        ui.mouse_move(0.0, 0.0, 640.0, 480.0, &shell);
        ui.frame(Instant::now(), &shell);
        assert_eq!(ui.browser.filter.game_type.as_deref(), Some("bel"));
        // Past the last script wraps to All.
        shell.execute("set ui_joinGametype 5");
        ui.mouse_move(0.0, 0.0, 640.0, 480.0, &shell);
        ui.frame(Instant::now(), &shell);
        assert_eq!(ui.browser.filter.game_type.as_deref(), Some("tdm"));
        let out = click_item(&mut ui, "joinserver", "@MENU_GAME_TYPE", &shell);
        assert!(out.contains(&UiEffect::Command("set ui_joinGametype \"0\"".into())));
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
}
