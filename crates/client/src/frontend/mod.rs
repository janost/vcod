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
//! `vcod-server` and joins it ([`create`]). Menus vcod cannot run yet
//! (single player, the language and record restarts) are refused with a
//! console line instead of drawing screens whose controls do nothing.

pub mod browser;
pub mod cod2;
pub mod create;
pub mod item_rules;
mod listbox;
mod options;
mod players;
pub mod profiles;
pub mod scan;
mod status;

mod input;
mod lists;
mod load;
mod paint;
mod script;
mod stack;

use input::contains;
use load::Assets;
pub(crate) use paint::Painter;
use paint::is_edit_field;
pub use paint::{GPU_INFO, GpuInfo};

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::net::SocketAddrV4;
use std::path::PathBuf;
use std::time::{Duration, Instant};
use vcod_common::net::cdkey;
use vcod_common::ui_menu;
#[cfg(test)]
use vcod_common::ui_menu::ITEM_TYPE_BUTTON;
use vcod_common::ui_menu::UiMenu;

use crate::hud::HudQuad;
use crate::hud::font::UiFonts;
use browser::Browser;
use status::StatusQuery;

/// `ui_menuFiles`' default (`ui_mp_x86.dll` 0x40036c9c): the menu list the
/// UI loads, which a mod replaces by shipping its own.
const MENU_LIST: &str = "ui_mp/menus.txt";
/// The second list `_UI_Init` loads, after [`MENU_LIST`] and without
/// clearing it (0x4000d64c). Of what stock names only
/// `ui_mp/wm_quickmessage.menu` ships.
const INGAME_LIST: &str = "ui_mp/ingame.txt";

/// The stock menus vcod cannot run yet; `open` refuses them with a console
/// line instead of drawing controls that do nothing. Any other menu the
/// list loads, a mod's included, opens.
const UNSUPPORTED: [&str; 7] = [
    "single_popmenu",
    "language_restart_popmenu",
    "rec_restart_popmenu",
    "multi_menu",
    "connect",
    "single_player_menu",
    "auconfirm",
];

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

/// `ownerdraw` ids from `ui_mp/menudef.h`.
const UI_NETSOURCE: i32 = 220;
const UI_NETGAMETYPE: i32 = 245;
const UI_NETMAPPREVIEW: i32 = 221;
const UI_NETMAPCINEMATIC: i32 = 246;
const UI_STARTMAPCINEMATIC: i32 = 255;
const UI_SERVERREFRESHDATE: i32 = 247;
const UI_GLINFO: i32 = 249;
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
    /// An edit field's `cursorPos` (item `+0x24c`) and `paintOffset`
    /// (edit data `+0x1c`), in characters; they outlive the edit and a
    /// close, until a load (`Item_TextField_HandleKey`, 0x40012690).
    edit: options::EditPos,
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
    /// Each scrolled list's first row drawn, by feeder.
    tops: listbox::Tops,
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
    /// The `.arena` maps on the search path.
    arenas: Vec<create::Arena>,
    /// The selected row of `FEEDER_ALLMAPS`.
    map_row: usize,
    /// `FEEDER_PLAYER_LIST`'s and CoD2's `FEEDER_MUTELIST`'s cleaned names
    /// (one list, `UI_FeederCount` Mac 0x156946), their client numbers
    /// (CoD2's only), the selected row (0 until a pick, as retail's zeroed
    /// `uiInfo`) and next rebuild.
    players: Vec<String>,
    player_clients: Vec<u32>,
    /// `CL_IsPlayerMuted` for the mute list, bit `n` for client `n`, set
    /// each frame ([`Self::set_muted`]).
    muted: u64,
    player_row: usize,
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
    /// Nested [`Ui::open_menu`] calls: an `open` reached from a menu's
    /// own `onOpen` or `mouseEnter`.
    opening: u32,
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
    /// `uiDC.realTime`, ms since the first [`Self::frame`]: the clock the
    /// focus pulse reads ([`paint::pulse`]); 0 until a frame runs.
    real_time: i32,
    clock: Option<Instant>,
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
    /// Whose CD key the CD key menu shows and sets, and whether setting it
    /// writes `main/codkey` (1.1 only).
    key_game: (cdkey::KeyGame, bool),
}

/// `menudef.h`'s upper case edit field, which `Item_SetTextExtents`
/// measures a cvar for beside 4 and 9 (0x40013ca4).
const ITEM_TYPE_UPREDITFIELD: i32 = 15;

/// `menudef.h`'s `ITEM_ALIGN_CENTER2`: measured as right aligned, never
/// kept (0x40013c28, 0x40013cf2).
const ITEM_ALIGN_CENTER2: i32 = 3;

#[cfg(test)]
mod tests {
    use super::*;

    use vcod_common::localize::Localized;
    use vcod_common::pk3::Pk3Fs;
    use vcod_common::ui_menu::{FEEDER_ALLMAPS, ITEM_TYPE_TEXT, UiItem};
    use winit::keyboard::KeyCode;

    use crate::console::shell::Shell;

    /// A `Ui` whose Local refresh broadcasts nowhere.
    pub(super) fn ui() -> Option<Ui> {
        let fs = vcod_common::testing::stock_fs_11()?;
        let mut ui = Ui::new(&fs, "0");
        ui.browser.lan_targets.clear();
        Some(ui)
    }

    pub(super) fn click_item(ui: &mut Ui, menu: &str, text: &str, shell: &Shell) -> Vec<UiEffect> {
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

    /// The map list over `createserver`, its item and the down arrow's spot.
    pub(super) fn map_list(ui: &mut Ui) -> (UiItem, [f32; 2]) {
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

    /// The main menu open with the cursor on Start New Server.
    pub(super) fn main_at_start(ui: &mut Ui, shell: &Shell) -> (usize, usize) {
        ui.open_main(&Shell::new(), &mut Vec::new());
        let m = ui.find("main").unwrap();
        let start = (ui.menus[m].items.iter())
            .position(|i| i.text == "@MENU_START_NEW_SERVER")
            .unwrap();
        let r = ui.menus[m].items[start].rect;
        ui.mouse_move(r[0] + 2.0, r[1] + 2.0, 640.0, 480.0, shell);
        (m, start)
    }

    /// The main menu with Start New Server turned into a text item whose
    /// `action` plays "start", the cursor off every item and the menu
    /// painted once.
    pub(super) fn start_as_text(ui: &mut Ui, shell: &Shell) -> (usize, usize) {
        let (m, start) = main_at_start(ui, shell);
        ui.mouse_move(0.0, 479.0, 640.0, 480.0, shell);
        let item = &mut ui.menus[m].items[start];
        item.kind = ITEM_TYPE_TEXT;
        item.action = vec!["play".into(), "start".into()];
        assert_eq!(ui.text_rect(m, start), None, "never painted");
        ui.build(640.0, 480.0, &Localized::default(), shell);
        (m, start)
    }

    /// A UI over one menu file, `ui_mp/m.menu`, holding `body`.
    pub(super) fn ui_with_menu(body: &str) -> Ui {
        let mut fs = Pk3Fs::empty();
        fs.overlay(
            "ui_mp/menus.txt",
            br#"{ loadMenu { "ui_mp/m.menu" } }"#.to_vec(),
        );
        fs.overlay("ui_mp/m.menu", body.as_bytes().to_vec());
        Ui::new(&fs, "0")
    }

    pub(super) fn response(menu: &str, response: &str) -> UiEffect {
        UiEffect::ScriptMenuResponse {
            menu: menu.into(),
            response: response.into(),
        }
    }

    /// A UI over a pak holding one script menu shaped like the stock team
    /// menus, loaded as the cgame loads the precached ones.
    pub(super) fn ui_with_team_menu() -> Ui {
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

    /// Applies the `set` commands a menu sent, as the app does.
    pub(super) fn apply(shell: &mut Shell, out: &[UiEffect]) {
        for e in out {
            if let UiEffect::Command(line) = e {
                shell.execute(line);
            }
        }
    }

    pub(super) fn top_name(ui: &Ui) -> &str {
        &ui.menus[*ui.open.last().unwrap()].name
    }

    pub(super) fn click_at(ui: &mut Ui, m: usize, i: usize, shell: &mut Shell) {
        let r = ui.menus[m].items[i].rect;
        ui.mouse_move(r[0] + 2.0, r[1] + 2.0, 640.0, 480.0, shell);
        assert_eq!(ui.hover, Some((m, i)));
        let out = ui.click(Instant::now(), shell);
        apply(shell, &out);
        ui.frame(Instant::now(), shell);
    }

    pub(super) fn click_named(ui: &mut Ui, menu: &str, name: &str, shell: &mut Shell) {
        let m = ui.find(menu).unwrap();
        let i = (0..ui.menus[m].items.len())
            .find(|&i| ui.menus[m].items[i].name == name && ui.shown(m, i, shell))
            .unwrap_or_else(|| panic!("{name}"));
        click_at(ui, m, i, shell);
    }

    pub(super) fn type_text(ui: &mut Ui, text: &str, shell: &mut Shell) {
        let out = ui.edit_key(KeyCode::KeyA, Some(text), shell);
        apply(shell, &out);
        ui.edit_key(KeyCode::Enter, None, shell);
    }
}
