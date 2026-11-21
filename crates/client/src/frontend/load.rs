//! Contains routines ported from the RTCW-MP GPL source, Copyright (C) 1999-2010 id Software LLC, a ZeniMax Media company.
//! See NOTICE.
//!
//! The menu lists and files the UI loads (`UI_LoadMenus`), the levelshots,
//! and building, reloading and re-sizing a [`Ui`] off them.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use vcod_common::asset_source::{Asset, AssetSource};
use vcod_common::net::cdkey;
use vcod_common::pk3::Pk3Fs;
use vcod_common::ui_menu;
use vcod_common::ui_menu::{MenuFile, UiMenu};

use super::browser::Browser;
use super::lists::server_columns;
use super::{
    INGAME_LIST, ItemState, MENU_LIST, UIMENU_NONE, Ui, UiEffect, browser, cod2, create, options,
};
use crate::console::shell::Shell;
use crate::hud::font::UiFonts;

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
pub(super) struct Assets {
    /// `itemFocusSound`.
    pub(super) item_focus_sound: Option<String>,
    /// `cursor`, drawn at the mouse while a menu is up ([`Ui::build`]).
    pub(super) cursor: Option<String>,
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

/// The maps `levelshots/` holds an image for.
pub(super) fn levelshots(fs: &Pk3Fs) -> HashSet<String> {
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

    pub(super) fn open(fs: &Pk3Fs, language: &str, load: bool) -> Ui {
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
            tops: super::listbox::Tops::default(),
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
            arenas,
            map_row: 0,
            players: Vec::new(),
            player_clients: Vec::new(),
            muted: 0,
            player_row: 0,
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
            opening: 0,
            text_rects: RefCell::default(),
            list_menus: 0,
            active_type: UIMENU_NONE,
            server_cvars: HashMap::new(),
            cod2: (fs.game() == vcod_common::game::Game::Cod2).then(cod2::Cod2Rects::default),
            key_game: match fs.game() {
                vcod_common::game::Game::Cod2 => (cdkey::KeyGame::Cod2, false),
                _ => {
                    let proto = vcod_common::net::protocol::install_protocol(fs);
                    match proto.is_uo() {
                        true => (cdkey::KeyGame::Uo, false),
                        false => (cdkey::KeyGame::Cod1, proto.is_v1()),
                    }
                }
            },
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

    /// Favourites live in `path`, a server cache of `layout` beside the
    /// game's executable (`servercache.dat`, UO's `uoservercache.dat`).
    pub fn with_server_cache(
        mut self,
        path: PathBuf,
        layout: vcod_common::net::server_cache::Layout,
    ) -> Ui {
        self.set_browser(Browser::with_cache(path, layout));
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
        self.thumb_drag = None;
        self.arrow_hold = None;
        if let Some(name) = focused {
            self.open_menu(&name, shell, out);
        }
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
        self.thumb_drag = self.thumb_drag.filter(|&(m, _)| m < base);
        self.arrow_hold = self.arrow_hold.filter(|&((m, _), ..)| m < base);
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
}

#[cfg(test)]
mod tests {
    use super::*;

    use vcod_common::localize::Localized;
    use winit::keyboard::KeyCode;

    use super::super::tests::{click_item, top_name, ui, ui_with_menu};

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
        let mut ui = ui_with_menu(menu).with_server_cache(
            PathBuf::from("/nonexistent/servercache.dat"),
            vcod_common::net::server_cache::Layout::Cod11,
        );
        assert_eq!(ui.browser.columns, &browser::COLUMNS_V5[..]);
        ui.set_browser(Browser::default());
        assert_eq!(ui.browser.columns, &browser::COLUMNS_V5[..]);
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

    /// A scroll thumb held on a script menu lets go when the script menus
    /// are reloaded: a mouse move after it indexes nothing gone (BUG-081).
    #[test]
    fn a_held_thumb_lets_go_when_the_script_menus_reload() {
        let Some(fs) = vcod_common::testing::stock_fs_11() else {
            return;
        };
        let mut ui = Ui::new(&fs, "0");
        let names = ["team_americangerman".into(), "weapon_american".into()];
        assert!(ui.load_script_menus(&fs, &names, "0").is_empty());
        let m = ui.find("weapon_american").unwrap();
        ui.thumb_drag = Some((m, 0));
        assert!(ui.load_script_menus(&fs, &[], "0").is_empty());
        assert_eq!(ui.thumb_drag, None);
        ui.mouse_move(10.0, 10.0, 640.0, 480.0, &Shell::new());
    }
}
