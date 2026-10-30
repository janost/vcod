//! The server's script menus on a live CoD4 session, renderer-free: the
//! server's `t <n>` opens the menu at `CS_SCRIPT_MENUS + n`, `u`
//! (`closeMenu`) and `L` (`closeInGameMenu`) close it, and a pick goes back
//! as `mr <sv_serverid> <n> <response>` (cod4-netfields.md 8), `<n>` the
//! slot of the menu the pick was made in.
//!
//! A menu the zones give runs as 1.7's UI runs it (cod4-hud.md 12): its
//! `onOpen`, then each frame its items' expressions over the UI's local
//! variables, the dvars its scripts and the server (`v`) set, the profile's
//! stats, the player's team and the zones' string tables; the cursor and
//! the keys move the focus, whose `onFocus` and `leaveFocus` run, and a
//! click or Enter runs the focused item's `action`. The team pick's
//! `changeclass_marines` opens `changeclass` on an online game
//! (`onlinegame` 1, what retail's client answered in the capture). A menu
//! with an expression vcod cannot evaluate, or one known only by its
//! responses ([`fallback`], before the zones load), shows its responses
//! as a numbered list. `--team` and `--class` answer their menus on the
//! first open, as CoD1's `--team` / `--weapon` do.
//!
//! The in-game menu's other pages run on vcod's console ([`Host`], the
//! `controls` module): the bind rows show and set its binds, the option
//! controls the cvars vcod has an equivalent for (the rest draw greyed),
//! the call vote's pages send `callvote`, and Mute Players mutes a
//! player's chat (cod4-hud.md 12.6).

pub mod browser;
pub mod controls;
pub mod front;

use std::collections::{HashMap, HashSet};

pub use controls::Lists;

use vcod_cod4::hud::Table;
use vcod_cod4::menuexp::{self, Env, Unsupported, Value};
use vcod_cod4::net::parse::Gamestate;
use vcod_cod4::net::stats::Stats;
use vcod_cod4::scriptmenu::{
    Cmd, Layout, LayoutItem, ScriptMenu, WINDOW_DECORATION, WINDOW_VISIBLE,
};

use crate::hud::cod2elem::ScreenPlacement;

/// `CS_SCRIPT_MENUS` (cod4-netfields.md 8).
pub const CS_SCRIPT_MENUS: u16 = 1970;
const MAX_SCRIPT_MENUS: u16 = 32;
/// The class `--team allies|axis` picks when no `--class` names one.
pub const DEFAULT_CLASS: &str = "assault_mp,0";
/// How deep scripts may open menus that run scripts.
const MAX_DEPTH: u32 = 8;

/// What to answer on its own.
#[derive(Clone, Debug, Default)]
pub struct Join {
    pub team: Option<String>,
    pub class: Option<String>,
}

/// The menu up: the one shown, its slot and what it answers.
#[derive(Clone, Debug, PartialEq)]
pub struct OpenMenu {
    pub name: String,
    pub slot: u16,
    pub responses: Vec<String>,
    pub esc: Option<String>,
    /// The drawn menu; `None` draws the numbered list.
    pub layout: Option<Layout>,
    /// The focused item (`WINDOW_HASFOCUS`).
    pub focus: Option<usize>,
    /// The items the cursor is over (`dynamicFlags` 1).
    pub over: Vec<bool>,
    /// Each list box's first drawn row and selected row
    /// (`listBoxDef_s.startPos`, `itemDef_s.cursorPos`), by item.
    pub lists: HashMap<usize, (usize, usize)>,
}

/// A menu key, as `Menu_HandleKey` reads it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuKey {
    /// Tab or down arrow: the next item.
    Next,
    /// Up arrow: the one before.
    Prev,
    /// The left and right arrows: the focused control's step, else the
    /// item before or after (`Menu_HandleKey` 156, 157).
    Left,
    Right,
    /// Enter: the focused item's `action`.
    Enter,
    Escape,
    /// A character key, for the menu's `execKey`s and the list's 1-9.
    Char(u8),
}

/// The configs the menus' `exec`s run themselves: `options_graphics`'
/// `onOpen` copies the video dvars into their `ui_r_*` twins
/// (`setfromdvar ui_r_mode r_mode`, iw_00.iwd), the video restart popup's
/// Yes copies them back before its `vid_restart` (cod4-front-end.md 9).
pub const MENU_CFGS: [&str; 2] = ["options_graphics.cfg", "options_graphics_set.cfg"];

/// What the menus read and write past the UI, refreshed from the client
/// each frame a menu is up: vcod's console cvars by lowercased name, its
/// binds (key, command), and the players the newest snapshot lists
/// (client number and name, by number; `UI_BuildPlayerList`).
#[derive(Clone, Debug, Default)]
pub struct Host {
    pub cvars: HashMap<String, String>,
    /// The enum cvars' entries (`r_mode`, `r_displayRefresh`), by
    /// lowercased name.
    pub enums: HashMap<String, Vec<String>>,
    pub binds: Vec<(String, String)>,
    pub players: Vec<(u32, String)>,
}

/// The UI's own state: `UILocalVarContext` and the dvars menu scripts
/// and the server set.
#[derive(Clone, Debug, Default)]
pub struct UiVars {
    pub locals: HashMap<String, Value>,
    /// By lowercased name.
    pub dvars: HashMap<String, String>,
}

#[derive(Default)]
pub struct ScriptMenus {
    defs: HashMap<String, ScriptMenu>,
    tables: HashMap<String, Table>,
    auto: Join,
    team_done: bool,
    class_done: bool,
    open: Option<OpenMenu>,
    /// `g_scriptMainMenu`, the menu Esc opens with none up, as the server
    /// last set it (`v`).
    main_menu: Option<String>,
    /// Every menu the server opened, by name, for the probe and tests.
    pub opened: Vec<String>,
    pub ui: UiVars,
    /// The profile's stats `stat()` reads: the block the join sent.
    stats: Option<Stats>,
    /// The configstrings' dvar mirror (cod4-hud.md 2), by lowercased name.
    pub mirror: HashMap<String, String>,
    /// The client's startup configs' `set`s (`h::startup_dvars`), by
    /// lowercased name: under the mirror and the server's `v`, as later
    /// writes to 1.7's one dvar table (`scr_game_spectatetype` 1, which
    /// the team menu's Spectator item reads).
    pub startup: HashMap<String, String>,
    /// The configs a menu's `exec` runs line by line ([`MENU_CFGS`]), by
    /// name: the Options pages' copies of the video dvars.
    pub cfgs: HashMap<String, String>,
    /// The local player's `team_t` and the level's `mapname`.
    pub team: i32,
    pub mapname: String,
    /// The level's `g_gametype`, its display name and the localized
    /// `cg_objectiveText`, which `gameType()`, `gameTypeName()` and
    /// `gameTypeDescription()` read (cod4-hud.md 12.2).
    pub gametype: String,
    pub gametype_name: String,
    pub objective: String,
    /// Console lines the menus' `exec`s ran that are not theirs to run
    /// (`disconnect`), for the client's console.
    console: Vec<String>,
    /// The cursor in window pixels, once a menu has had the mouse.
    cursor: Option<[f32; 2]>,
    /// Menus drawn as the list, each logged once.
    listed: HashSet<String>,
    /// vcod's console and the players, as the client last gave them.
    pub host: Host,
    /// A server Start New Server's Dedicated No started, which this
    /// client is joined to, runs: retail's listen server, so `sv_running`
    /// reads 1 (`App::refuse_vid_restart` stands in for it too).
    pub listen_server: bool,
    /// The open menu's bind item waiting for its key (`g_waitingForKey`,
    /// `g_bindItem`).
    waiting: Option<usize>,
    /// The muted client numbers (`s_playerMute`).
    muted: HashSet<u32>,
    /// The call vote's maps and game types.
    lists: Lists,
    /// The player list's pick (`uiInfo.playerIndex`).
    player_index: Option<usize>,
    /// Option dvars logged as having no vcod equivalent.
    greyed: HashSet<String>,
    /// What the expressions' cgame calls read, as the client last saw it.
    pub game: menuexp::GameView,
    /// The zones' strings, for `locString()` and `keyBinding()`.
    pub strings: Option<std::sync::Arc<vcod_common::localize::Localized>>,
    /// The item MOUSE1 holds (`itemCapture`): a slider or a list box's
    /// thumb, which the cursor drags until MOUSE1 goes up.
    capture: Option<usize>,
    /// The last list row clicked (`lastListBoxClickTime`, `mousePos`):
    /// when, the item and the row.
    last_list_click: Option<(std::time::Instant, usize, usize)>,
    /// The menu stack (`UiContext::menuStack`): the menus open under
    /// [`Self::open`], bottom first. A script's `open` stacks the new one
    /// over them (`Menus_OpenByName`), `close` uncovers the next; a
    /// server's `t` starts a fresh stack.
    below: Vec<OpenMenu>,
    /// The front end's ([`Self::front_end`]): no server, its `ui_*` dvars
    /// in the UI's table, its `uiScript`s and menu sounds run.
    is_front: bool,
    /// The front end's scripts' requests, lists and sounds.
    pub front: front::Front,
    /// The server browser behind `pc_join_unranked` (`browser`), its
    /// rows as shown, and whether `joinpb_popmenu`'s Yes was taken.
    pub browser: browser::Browser,
    display: browser::Display,
    joinpb_answered: bool,
    /// The open menu's edit field taking typed text (`g_editingField`).
    editing: Option<usize>,
}

impl ScriptMenus {
    /// `CL_IsPlayerMuted`: client `n` toggled muted on the Mute Players
    /// page.
    pub fn is_muted(&self, n: u32) -> bool {
        self.muted.contains(&n)
    }

    pub fn new(auto: Join) -> Self {
        ScriptMenus {
            defs: fallback(),
            auto,
            stats: Some(Stats::fresh()),
            ..ScriptMenus::default()
        }
    }

    /// The front end's menus (`cod4_front`): no server, a menu stack, and
    /// the main menu's `uiScript`s; the profile's stats are a fresh one's.
    pub fn front_end() -> Self {
        ScriptMenus {
            is_front: true,
            stats: Some(Stats::fresh()),
            ..ScriptMenus::default()
        }
    }

    /// `Menus_OpenByName` from the client (`UI_SetActiveMenu`'s `main`):
    /// the console lines and responses its scripts ran.
    pub fn open_by_name(&mut self, name: &str, gs: &Gamestate) -> Vec<String> {
        let mut out = Vec::new();
        self.open_menu(name, None, gs, &mut out, 0);
        out
    }

    /// `Menus_CloseAll`: each open menu's `onClose` runs, bottom up (retail
    /// walks them in load order), and no menu is left up, the ones those
    /// scripts opened included (their console lines and responses are
    /// returned).
    pub fn close_all(&mut self, gs: &Gamestate) -> Vec<String> {
        let mut out = Vec::new();
        let open: Vec<OpenMenu> = self.below.drain(..).chain(self.open.take()).collect();
        self.close();
        for m in open {
            if let Some(l) = m.layout {
                self.run(&m.name, None, &l.on_close, gs, &mut out, 1);
            }
        }
        self.below.clear();
        self.close();
        out
    }

    /// The menus open, bottom first, from the topmost `fullScreen` one up
    /// (`Menu_PaintAll`).
    pub fn drawn(&self) -> Vec<&OpenMenu> {
        let all: Vec<&OpenMenu> = self.below.iter().chain(self.open.as_ref()).collect();
        let from = all
            .iter()
            .rposition(|o| o.layout.as_ref().is_some_and(|l| l.fullscreen))
            .unwrap_or(0);
        all[from..].to_vec()
    }

    /// The aliases the menus' `play` played since the last call.
    pub fn take_sounds(&mut self) -> Vec<String> {
        std::mem::take(&mut self.front.sounds)
    }

    /// The `soundName` of the topmost drawn menu that has one, played as
    /// the menu opens and while it draws: the main menu's
    /// `music_mainmenu_mp` (cod4-front-end.md 1 and 8; when retail plays
    /// it is INFERRED, not yet confirmed on 1.7).
    pub fn music(&self) -> Option<&str> {
        self.drawn()
            .into_iter()
            .rev()
            .find_map(|o| o.layout.as_ref()?.sound.as_deref())
    }

    /// `Menu_IsMenuOpenAndVisible`: `name` anywhere on the stack.
    pub fn is_open(&self, name: &str) -> bool {
        self.below
            .iter()
            .chain(self.open.as_ref())
            .any(|o| o.name.eq_ignore_ascii_case(name))
    }

    /// The zones' menus, by lowercased name, and their string tables; the
    /// fallback's stay under any the zones lack.
    pub fn set_defs(&mut self, defs: HashMap<String, ScriptMenu>, tables: HashMap<String, Table>) {
        self.defs.extend(defs);
        self.tables = tables;
    }

    pub fn open(&self) -> Option<&OpenMenu> {
        self.open.as_ref()
    }

    /// A server command; returns the client commands it answers with.
    pub fn command(&mut self, text: &[u8], gs: &Gamestate) -> Vec<String> {
        let text = String::from_utf8_lossy(text);
        let mut words = text.split_whitespace();
        match words.next() {
            Some("t") => {
                let Some(n) = words.next().and_then(|w| w.parse::<u16>().ok()) else {
                    return Vec::new();
                };
                let mut out = Vec::new();
                self.show(n, gs, &mut out);
                out.extend(self.auto_answer(gs));
                out
            }
            Some("u" | "L") => {
                self.below.clear();
                self.close();
                Vec::new()
            }
            Some("v") => {
                if let Some(vcod_cod4::hud::Command::Dvars(d)) =
                    vcod_cod4::hud::Command::parse(text.as_bytes())
                {
                    for (k, v) in d {
                        if k.eq_ignore_ascii_case("g_scriptMainMenu") {
                            self.main_menu = Some(v.clone());
                        }
                        self.ui.dvars.insert(k.to_ascii_lowercase(), v);
                    }
                }
                Vec::new()
            }
            // `CG_ServerCommand`'s `N <index> <value>`: `LiveStorage_SetStat`,
            // INFERRED; the server's `setStat` (cod4-gsc.md).
            Some("N") => {
                let mut n = || words.next().and_then(|w| w.parse::<i64>().ok());
                if let (Some(i), Some(v), Some(stats)) = (n(), n(), self.stats.as_mut())
                    && let Ok(i) = u16::try_from(i)
                {
                    stats.set(i, v as u32);
                }
                Vec::new()
            }
            _ => Vec::new(),
        }
    }

    /// The profile's stats as the server's `N` commands left them.
    pub fn stats(&self) -> Option<&Stats> {
        self.stats.as_ref()
    }

    /// The profile's own stats in place of a fresh profile's.
    pub fn set_stats(&mut self, stats: Stats) {
        self.stats = Some(stats);
    }

    /// Esc with no menu up: `g_scriptMainMenu` opens, when the server
    /// named one that is precached.
    pub fn open_main(&mut self, gs: &Gamestate) -> Vec<String> {
        let mut out = Vec::new();
        if let Some(n) = self.main_menu.as_deref().and_then(|m| menu_slot(gs, m)) {
            self.show(n, gs, &mut out);
        }
        out
    }

    /// The menu at slot `n` opens.
    fn show(&mut self, n: u16, gs: &Gamestate, out: &mut Vec<String>) {
        let name = menu_name(gs, n);
        self.opened.push(name.clone());
        self.below.clear();
        self.open = None;
        self.open_menu(&name, Some(n), gs, out, 0);
    }

    /// `Menus_OpenByName`: `name` goes up (at `slot`, else its own
    /// configstring slot) and its `onOpen` runs. A menu without a layout
    /// opens the one its `opens` name, the fallback's redirect.
    fn open_menu(
        &mut self,
        name: &str,
        slot: Option<u16>,
        gs: &Gamestate,
        out: &mut Vec<String>,
        depth: u32,
    ) {
        if depth > MAX_DEPTH {
            return;
        }
        self.waiting = None;
        self.editing = None;
        (self.capture, self.last_list_click) = (None, None);
        let key = name.to_ascii_lowercase();
        let Some(def) = self.defs.get(&key).cloned() else {
            log::debug!("cod4 menu {name}: not in the zones");
            if let Some(n) = slot {
                self.open = Some(OpenMenu {
                    name: name.to_string(),
                    slot: n,
                    responses: Vec::new(),
                    esc: None,
                    layout: None,
                    focus: None,
                    over: Vec::new(),
                    lists: HashMap::new(),
                });
            }
            return;
        };
        let slot = menu_slot(gs, name).or(slot).unwrap_or(0);
        let layout = def.layout.clone().filter(|l| self.evaluates(&def.name, l));
        self.push_open(&def.name, gs, out);
        let on_open = layout.as_ref().map(|l| l.on_open.clone());
        self.open = Some(OpenMenu {
            name: def.name.clone(),
            slot,
            responses: def.responses.clone(),
            esc: def.esc.clone(),
            over: vec![false; layout.as_ref().map_or(0, |l| l.items.len())],
            layout,
            focus: None,
            lists: HashMap::new(),
        });
        self.opened_controls(gs);
        match on_open {
            Some(cmds) => self.run(&def.name, None, &cmds, gs, out, depth),
            None => {
                let shown = def.shown(|d| self.dvar(d)).to_string();
                if !shown.eq_ignore_ascii_case(&def.name) {
                    // The fallback's redirect stands in for its menu.
                    self.open = None;
                    self.open_menu(&shown, None, gs, out, depth + 1);
                }
            }
        }
    }

    /// `Menus_Open` on a stack: the open menu loses the focus (its focused
    /// item's `leaveFocus`) and goes under the one opening, which leaves
    /// the stack first if it was on it (`Menus_AddToStack`).
    fn push_open(&mut self, name: &str, gs: &Gamestate, out: &mut Vec<String>) {
        self.clear_focus(gs, out);
        self.below.retain(|o| !o.name.eq_ignore_ascii_case(name));
        if let Some(cur) = self.open.take()
            && !cur.name.eq_ignore_ascii_case(name)
        {
            self.below.push(cur);
        }
    }

    /// `Menus_CloseByName` on a stack: `name` leaves it, wherever it is,
    /// and its `onClose` runs; closing the top gives the next one down
    /// the focus.
    fn close_named(&mut self, name: &str, gs: &Gamestate, out: &mut Vec<String>, depth: u32) {
        if depth > MAX_DEPTH {
            return;
        }
        let closed = if self
            .open
            .as_ref()
            .is_some_and(|o| o.name.eq_ignore_ascii_case(name))
        {
            let top = self.open.take();
            self.close();
            top
        } else {
            let at = self
                .below
                .iter()
                .position(|o| o.name.eq_ignore_ascii_case(name));
            at.map(|i| self.below.remove(i))
        };
        let Some(closed) = closed else {
            return;
        };
        let on_close = closed.layout.map(|l| l.on_close).unwrap_or_default();
        self.run(&closed.name, None, &on_close, gs, out, depth);
    }

    /// Whether every expression of `layout` evaluates; a menu that does
    /// not is drawn as the list, logged once.
    fn evaluates(&mut self, name: &str, layout: &Layout) -> bool {
        let env = Ctx(self);
        let mut statements =
            std::iter::once(&layout.visible).chain(layout.items.iter().flat_map(|it| {
                let e = &it.exps;
                [
                    &e.visible,
                    &e.text,
                    &e.material,
                    &e.rect_x,
                    &e.rect_y,
                    &e.rect_w,
                    &e.rect_h,
                    &e.forecolor_a,
                ]
            }));
        let err = statements.find_map(|s| menuexp::eval(s, &env).err());
        match err {
            None => true,
            Some(Unsupported(what)) => {
                if self.listed.insert(name.to_ascii_lowercase()) {
                    log::warn!("cod4 menu {name}: {what} not evaluated; drawn as a list");
                }
                false
            }
        }
    }

    /// `--team` on the first menu that offers it, `--class` likewise.
    fn auto_answer(&mut self, gs: &Gamestate) -> Option<String> {
        let open = self.open.as_ref()?;
        let offers = |r: Option<&String>| r.filter(|r| open.responses.contains(r)).cloned();
        if !self.team_done
            && let Some(team) = offers(self.auto.team.as_ref())
        {
            self.team_done = true;
            return self.answer(&team, gs);
        }
        if !self.class_done
            && let Some(class) = offers(self.auto.class.as_ref())
        {
            self.class_done = true;
            return self.answer(&class, gs);
        }
        None
    }

    /// Pick `i` of the open menu's list (`scriptMenuResponse`, then
    /// `close self`).
    pub fn choose(&mut self, i: usize, gs: &Gamestate) -> Option<String> {
        let r = self.open.as_ref()?.responses.get(i)?.clone();
        self.answer(&r, gs)
    }

    /// Esc: a drawn menu runs its `onEsc`; the list answers its `onEsc`
    /// response and closes.
    pub fn escape(&mut self, gs: &Gamestate) -> Vec<String> {
        let Some(open) = self.open.as_ref() else {
            return Vec::new();
        };
        if let Some(l) = &open.layout {
            let (name, cmds) = (open.name.clone(), l.on_esc.clone());
            let mut out = Vec::new();
            self.run(&name, None, &cmds, gs, &mut out, 0);
            return out;
        }
        match open.esc.clone() {
            Some(r) => self.answer(&r, gs).into_iter().collect(),
            None => {
                self.close();
                Vec::new()
            }
        }
    }

    /// A key while a menu is up (`Menu_HandleKey`); false when the menu
    /// does nothing with it.
    pub fn key(&mut self, key: MenuKey, gs: &Gamestate, out: &mut Vec<String>) -> bool {
        let Some(open) = self.open.as_ref() else {
            return false;
        };
        if open.layout.is_none() {
            return match key {
                MenuKey::Escape => {
                    out.extend(self.escape(gs));
                    true
                }
                MenuKey::Char(c @ b'1'..=b'9') => {
                    out.extend(self.choose(usize::from(c - b'1'), gs));
                    true
                }
                _ => false,
            };
        };
        let name = open.name.clone();
        if self.edit_key(key, gs, out) {
            return true;
        }
        let Some(layout) = self.open.as_ref().and_then(|o| o.layout.as_ref()) else {
            return false;
        };
        if let MenuKey::Char(c) = key
            && let Some((_, cmds)) = layout.on_key.iter().find(|(k, _)| *k == i32::from(c))
        {
            let cmds = cmds.clone();
            self.run(&name, None, &cmds, gs, out, 0);
            return true;
        }
        let focus = self.open.as_ref().and_then(|o| o.focus);
        match key {
            MenuKey::Escape => out.extend(self.escape(gs)),
            MenuKey::Next | MenuKey::Prev => self.step_focus(key == MenuKey::Next, gs, out),
            // `Item_HandleKey` first, then the item's `action` either way.
            MenuKey::Enter => {
                if let Some(i) = focus {
                    let a = self.action_of(i);
                    self.item_key(i, controls::ItemKey::Enter, gs);
                    self.action(a, gs, out);
                }
            }
            MenuKey::Left | MenuKey::Right => {
                let step = if key == MenuKey::Left {
                    controls::ItemKey::Left
                } else {
                    controls::ItemKey::Right
                };
                let a = focus.and_then(|i| self.action_of(i));
                match focus {
                    Some(i) if self.item_key(i, step, gs) => self.action(a, gs, out),
                    _ => self.step_focus(key == MenuKey::Right, gs, out),
                }
            }
            MenuKey::Char(_) => return false,
        }
        true
    }

    /// `Menu_SetNextCursorItem` / `Prev`: the next item that takes the
    /// focus, wrapping once.
    fn step_focus(&mut self, next: bool, gs: &Gamestate, out: &mut Vec<String>) {
        let Some(open) = &self.open else {
            return;
        };
        let Some(layout) = &open.layout else {
            return;
        };
        let n = layout.items.len() as i32;
        let mut i = open.focus.map_or(if next { -1 } else { n }, |f| f as i32);
        for _ in 0..n {
            i = (i + if next { 1 } else { -1 }).rem_euclid(n);
            if self.can_focus(&layout.items[i as usize]) {
                self.set_focus(i as usize, gs, out);
                return;
            }
        }
    }

    /// Whether item `it` is drawn: `WINDOW_VISIBLE`, its `dvarTest`'s
    /// show or hide, and its `visible when` (`Item_IsVisible`).
    pub fn item_visible(&self, it: &LayoutItem) -> bool {
        it.dynamic_flags & WINDOW_VISIBLE != 0
            && (it.dvar_flags & 0xc == 0 || self.dvar_enabled(it, 4))
            && (it.exps.visible.is_empty()
                || menuexp::truth(&it.exps.visible, &Ctx(self)).unwrap_or(false))
    }

    /// `Item_SetFocus`'s test, for the item kinds that keep a rect: a
    /// visible, non-decoration item that is not plain text.
    fn can_focus(&self, it: &LayoutItem) -> bool {
        it.kind != 0
            && it.static_flags & WINDOW_DECORATION == 0
            && self.item_visible(it)
            && !self.greyed(it)
    }

    /// `Item_SetFocus`: the old focus's `leaveFocus` (`Menu_ClearFocus`),
    /// then item `i`'s `onFocus`.
    fn set_focus(&mut self, i: usize, gs: &Gamestate, out: &mut Vec<String>) {
        let Some(open) = &self.open else {
            return;
        };
        if open.focus == Some(i) {
            return;
        }
        self.clear_focus(gs, out);
        let Some(open) = &mut self.open else {
            return;
        };
        open.focus = Some(i);
        let name = open.name.clone();
        let cmds = open
            .layout
            .as_ref()
            .and_then(|l| l.items.get(i))
            .map(|it| it.on_focus.clone())
            .unwrap_or_default();
        self.run(&name, Some(i), &cmds, gs, out, 0);
    }

    /// `Menu_ClearFocus`: the focused item's `leaveFocus`.
    fn clear_focus(&mut self, gs: &Gamestate, out: &mut Vec<String>) {
        let Some(open) = &mut self.open else {
            return;
        };
        let Some(old) = open.focus.take() else {
            return;
        };
        let name = open.name.clone();
        let cmds = open
            .layout
            .as_ref()
            .and_then(|l| l.items.get(old))
            .map(|it| it.leave_focus.clone())
            .unwrap_or_default();
        self.run(&name, Some(old), &cmds, gs, out, 0);
    }

    /// Item `i` of the open menu, with that menu's name and the item's
    /// `action`, taken before the key reaches the item: retail's
    /// `Menu_HandleKey` holds the item pointer across `Item_HandleKey`, so
    /// `Item_Action` runs this item's script even when the key's own
    /// script (a list's `doubleClick` opening `joinpb_popmenu`) replaced
    /// the menu.
    fn action_of(&self, i: usize) -> Option<Action> {
        let open = self.open.as_ref()?;
        let it = open.layout.as_ref()?.items.get(i)?;
        Some(Action {
            menu: open.name.clone(),
            item: i,
            cmds: it.action.clone(),
        })
    }

    /// `Item_Action`: a held item's `action`.
    fn action(&mut self, a: Option<Action>, gs: &Gamestate, out: &mut Vec<String>) {
        if let Some(a) = a {
            self.run(&a.menu, Some(a.item), &a.cmds, gs, out, 0);
        }
    }

    /// The cursor in window pixels: where it last was, else the middle.
    pub fn cursor(&self, size: (f32, f32)) -> [f32; 2] {
        self.cursor.unwrap_or([0.5 * size.0, 0.5 * size.1])
    }

    /// The mouse moved by raw counts `d` (a captured pointer) or to `at`.
    pub fn mouse_move(
        &mut self,
        d: Option<[f32; 2]>,
        at: Option<[f32; 2]>,
        size: (f32, f32),
        gs: &Gamestate,
    ) -> Vec<String> {
        let c = self.cursor(size);
        let c = at.unwrap_or_else(|| {
            let d = d.unwrap_or_default();
            [c[0] + d[0], c[1] + d[1]]
        });
        self.cursor = Some([c[0].clamp(0.0, size.0), c[1].clamp(0.0, size.1)]);
        let mut out = Vec::new();
        if let Some(i) = self.capture {
            self.drag(i, size);
            return out;
        }
        self.hover(size, gs, &mut out);
        out
    }

    /// MOUSE1 up: a drag ends (`Item_HandleKey` clears `itemCapture`).
    pub fn release(&mut self) {
        self.capture = None;
    }

    /// `Menu_HandleMouseMove`: the items under the cursor run their
    /// `mouseEnter`, the topmost that takes the focus gets it, the ones
    /// it left run their `mouseExit`, and a focus the cursor left with no
    /// other taking it clears.
    fn hover(&mut self, size: (f32, f32), gs: &Gamestate, out: &mut Vec<String>) {
        let Some(open) = &self.open else {
            return;
        };
        let Some(layout) = open.layout.clone() else {
            return;
        };
        let name = open.name.clone();
        let sp = ScreenPlacement::cod4(size.0, size.1);
        let c = self.cursor(size);
        let mut focus_set = false;
        let same = |m: &ScriptMenus| m.open.as_ref().is_some_and(|o| o.name == name);
        for (i, it) in layout.items.iter().enumerate().rev() {
            // A script that closed or replaced the menu ends the walk.
            if !same(self) {
                return;
            }
            if !self.item_visible(it) {
                continue;
            }
            let inside = contains(placed(&sp, self.rect(it, &layout)), c);
            let over = self
                .open
                .as_ref()
                .is_some_and(|o| o.over.get(i).copied().unwrap_or(false));
            if inside {
                if !over {
                    self.set_over(i, true);
                    self.run(&name, Some(i), &it.mouse_enter, gs, out, 0);
                }
                if !focus_set && self.can_focus(it) {
                    self.set_focus(i, gs, out);
                    focus_set = true;
                }
            } else if over {
                self.set_over(i, false);
                self.run(&name, Some(i), &it.mouse_exit, gs, out, 0);
            }
        }
        if !focus_set
            && same(self)
            && let Some(f) = self.open.as_ref().and_then(|o| o.focus)
            && let Some(it) = layout.items.get(f)
            && !contains(placed(&sp, self.rect(it, &layout)), c)
        {
            self.clear_focus(gs, out);
        }
    }

    fn set_over(&mut self, i: usize, over: bool) {
        if let Some(o) = self.open.as_mut().and_then(|o| o.over.get_mut(i)) {
            *o = over;
        }
    }

    /// MOUSE1 (`Menu_HandleKey` 200): a bind waiting for its key takes
    /// it; else the focused item under the cursor takes the key
    /// (`Item_HandleKey`), then runs its `action`.
    pub fn click(&mut self, size: (f32, f32), gs: &Gamestate) -> Vec<String> {
        let mut out = Vec::new();
        if self.waiting.is_some() {
            self.bind_key("MOUSE1");
            return out;
        }
        // A press while an item holds the mouse lets it go first
        // (`Item_HandleKey`); the focus catches up with the cursor.
        if self.capture.take().is_some() {
            self.hover(size, gs, &mut out);
        }
        let Some(open) = &self.open else {
            return out;
        };
        let (Some(layout), Some(f)) = (&open.layout, open.focus) else {
            return out;
        };
        let Some(it) = layout.items.get(f) else {
            return out;
        };
        let sp = ScreenPlacement::cod4(size.0, size.1);
        if contains(placed(&sp, self.rect(it, layout)), self.cursor(size)) {
            let a = self.action_of(f);
            self.click_item(f, size, gs, &mut out);
            self.action(a, gs, &mut out);
        }
        out
    }

    /// Item `it`'s rect this frame: its `rect` with its rect expressions
    /// (`Item_Paint`; x and y off the menu's).
    pub fn rect(&self, it: &LayoutItem, layout: &Layout) -> ([f32; 4], [i32; 2]) {
        let env = Ctx(self);
        let e = &it.exps;
        let f = |s: &vcod_cod4::assets::ui::Statement| menuexp::float(s, &env).unwrap_or(0.0);
        let mut r = it.rect;
        if !e.rect_x.is_empty() {
            r[0] = f(&e.rect_x) + layout.rect[0];
        }
        if !e.rect_y.is_empty() {
            r[1] = f(&e.rect_y) + layout.rect[1];
        }
        if !e.rect_w.is_empty() {
            r[2] = f(&e.rect_w);
        }
        if !e.rect_h.is_empty() {
            r[3] = f(&e.rect_h);
        }
        (r, it.align)
    }

    /// The expression context, for the drawing.
    pub fn env(&self) -> impl Env + '_ {
        Ctx(self)
    }

    /// Runs a script of menu `menu` (its item `item`, or the menu's own).
    fn run(
        &mut self,
        menu: &str,
        item: Option<usize>,
        cmds: &[Cmd],
        gs: &Gamestate,
        out: &mut Vec<String>,
        depth: u32,
    ) {
        let _ = item;
        for c in cmds {
            match c {
                Cmd::Response(r) => {
                    if let (Some(sid), Some(open)) = (gs.systeminfo("sv_serverid"), &self.open) {
                        out.push(format!("mr {sid} {} {r}", open.slot));
                    }
                }
                Cmd::Close(m) => {
                    let m = if m.eq_ignore_ascii_case("self") {
                        menu
                    } else {
                        m
                    };
                    self.close_named(m, gs, out, depth + 1);
                }
                Cmd::Open(m) => self.push_menu(m, gs, out, depth + 1),
                Cmd::OpenMenuOnDvar {
                    dvar,
                    value,
                    menu: m,
                    not,
                } => {
                    if (self.dvar(dvar).unwrap_or_default() == *value) != *not {
                        self.push_menu(m, gs, out, depth + 1);
                    }
                }
                Cmd::SetFocus(name) => {
                    let at = self.open.as_ref().and_then(|o| {
                        o.layout
                            .as_ref()?
                            .items
                            .iter()
                            .position(|it| it.name.eq_ignore_ascii_case(name))
                    });
                    match at {
                        Some(i)
                            if self
                                .open
                                .as_ref()
                                .is_some_and(|o| o.name.eq_ignore_ascii_case(menu)) =>
                        {
                            self.set_focus(i, gs, out);
                            self.begin_edit(i);
                        }
                        _ => log::debug!("cod4 menu {menu}: setFocus {name}: no such item"),
                    }
                }
                Cmd::SetDvar(d, v) => self.set_dvar(d, v),
                Cmd::Show(name) | Cmd::Hide(name) => {
                    let show = matches!(c, Cmd::Show(_));
                    self.show_item(menu, name, show);
                }
                Cmd::UiScript(name, args) => {
                    if !(self.is_front && self.front_script(name, args, gs, out, depth))
                        && !self.browser_script(name, args, gs, out)
                    {
                        self.ui_script(name, args, gs);
                    }
                }
                Cmd::SetLocal(n, v) => {
                    self.ui.locals.insert(n.to_ascii_lowercase(), v.clone());
                }
                Cmd::Exec(line) => self.exec(line),
                Cmd::ExecOnDvar {
                    dvar,
                    value,
                    line,
                    kind,
                } => {
                    if kind.passes(&self.dvar(dvar).unwrap_or_default(), value) {
                        self.exec(line);
                    }
                }
                // `Script_playLocalSound`: the front end's and a game's
                // menus alike (cod4-hud.md 23).
                Cmd::Play(alias) => self.front.sounds.push(alias.clone()),
                Cmd::Other(_) => {}
            }
        }
    }

    /// A menu's console line: the `set`s and `statGetInDvar`s the stock
    /// menus run; anything else is not run.
    fn exec(&mut self, line: &str) {
        for part in line.split(';') {
            let w: Vec<&str> = part.split_whitespace().collect();
            match w.as_slice() {
                [cmd, name, rest @ ..]
                    if ["set", "seta", "sets", "setu"]
                        .iter()
                        .any(|s| cmd.eq_ignore_ascii_case(s)) =>
                {
                    self.set_dvar(name, &rest.join(" "));
                }
                [cmd, stat, name] if cmd.eq_ignore_ascii_case("statGetInDvar") => {
                    let v = self.stat(stat.parse().unwrap_or(-1));
                    self.ui
                        .dvars
                        .insert(name.to_ascii_lowercase(), v.to_string());
                }
                // `popup_leavegame`'s yes: the client's own console runs
                // it, and the game's menus go with the game.
                [cmd]
                    if ["disconnect", "quit"]
                        .iter()
                        .any(|s| cmd.eq_ignore_ascii_case(s)) =>
                {
                    self.console.push(cmd.to_ascii_lowercase());
                    self.below.clear();
                    self.close();
                }
                // `selectStringTableEntryInDvar`: a row of the table, its
                // column, into the dvar (`main_text`'s "did you know").
                [cmd, table, column, name]
                    if cmd.eq_ignore_ascii_case("selectStringTableEntryInDvar") =>
                {
                    let v = self.table_entry(table, column.parse().unwrap_or(0));
                    self.ui.dvars.insert(name.to_ascii_lowercase(), v);
                }
                [cmd, to, from] if cmd.eq_ignore_ascii_case("setfromdvar") => {
                    let v = self.dvar(from).unwrap_or_default();
                    self.set_dvar(to, &v);
                }
                [cmd] if cmd.eq_ignore_ascii_case("wait") => {}
                // The Options pages' configs, line by line.
                [cmd, file]
                    if cmd.eq_ignore_ascii_case("exec")
                        && MENU_CFGS.iter().any(|c| c.eq_ignore_ascii_case(file)) =>
                {
                    let text = self.cfgs.get(&file.to_ascii_lowercase()).cloned();
                    match text {
                        Some(t) => t.lines().for_each(|l| self.exec(l)),
                        None => log::debug!("cod4 menu exec {file}: not in the iwds"),
                    }
                }
                // The video restart popup's Yes and `apply_picmip_popmenu`'s
                // reload: the client's own.
                [cmd]
                    if ["vid_restart", "snd_restart", "r_applyPicmip"]
                        .iter()
                        .any(|s| cmd.eq_ignore_ascii_case(s)) =>
                {
                    self.console.push(cmd.to_ascii_lowercase());
                }
                // The call vote's `cmd callvote map_restart`: to the server.
                [cmd, _, ..] if cmd.eq_ignore_ascii_case("cmd") => {
                    self.console.push(part.trim().to_string())
                }
                [] => {}
                _ => log::debug!("cod4 menu exec {part:?}: not run"),
            }
        }
    }

    /// A table's entry in `column`, of a row picked off the clock
    /// (`selectStringTableEntryInDvar` picks one at random).
    fn table_entry(&self, table: &str, column: usize) -> String {
        let Some(t) = self.tables.get(&table.to_ascii_lowercase()) else {
            return String::new();
        };
        let rows = t.cells.len() / t.columns.max(1);
        if rows == 0 {
            return String::new();
        }
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.subsec_nanos() as usize);
        let row = nanos % rows;
        t.cells
            .get(row * t.columns + column)
            .cloned()
            .unwrap_or_default()
    }

    /// The console lines the menus ran since the last call.
    pub fn take_console(&mut self) -> Vec<String> {
        std::mem::take(&mut self.console)
    }

    /// `stat(index)`, 0 past the block.
    pub(crate) fn stat(&self, index: i32) -> i32 {
        let i = u16::try_from(index).ok();
        i.and_then(|i| self.stats.as_ref()?.get(i)).unwrap_or(0) as i32
    }

    /// A dvar as the menus read it.
    pub(crate) fn dvar(&self, name: &str) -> Option<String> {
        let key = name.to_ascii_lowercase();
        if let Some(v) = self.host_cvar(&key).and_then(|c| self.host.cvars.get(c)) {
            return Some(v.clone());
        }
        if let Some(v) = self
            .ui
            .dvars
            .get(&key)
            .or_else(|| self.mirror.get(&key))
            .or_else(|| self.startup.get(&key))
        {
            return Some(v.clone());
        }
        match key.as_str() {
            // `UI_RegisterDvars`: `m_pitch < 0`.
            "ui_mousepitch" => {
                let pitch = self.host.cvars.get("m_pitch")?;
                Some(i32::from(menuexp::atof(pitch) < 0.0).to_string())
            }
            // What retail's client answered in the capture (cod4-hud.md 12).
            "onlinegame" => Some("1".into()),
            // Registered 1, read-only (iw3mp.exe 0x5492fb, cod4-front-end.md 9).
            "ui_multiplayer" => Some("1".into()),
            "mapname" => Some(self.mapname.clone()),
            // `R_StoreWindowSettings` sets it off 4:3 (cod4-front-end.md 10).
            "widescreen" => Some(i32::from(crate::hud::cod2elem::cod4_wide_screen()).to_string()),
            // 1.7's Graphics, Texture Settings, Sound and Reset rows
            // grey under it (cod4-hud.md 12.6), as its `vid_restart`
            // refuses (iw3mp.exe 0x46a1ad).
            "sv_running" => Some(i32::from(self.listen_server).to_string()),
            // The console's `developer`, which `options_game`'s language
            // row is `visible when` (cod4-hud.md 12.6).
            "developer" => self.host.cvars.get("developer").cloned(),
            _ => browser::ui_dvar(&key).map(str::to_string),
        }
    }

    /// The open menu closes, uncovering the one under it; no bind waits
    /// for a key.
    fn close(&mut self) {
        self.open = self.below.pop();
        self.waiting = None;
        self.editing = None;
    }

    /// A script's `open`: `name` goes up over the open menu, which stays
    /// under it; a menu already in the stack comes up from there.
    fn push_menu(&mut self, name: &str, gs: &Gamestate, out: &mut Vec<String>, depth: u32) {
        if !self.defs.contains_key(&name.to_ascii_lowercase()) {
            log::debug!("cod4 menu {name}: not in the zones");
            return;
        }
        self.open_menu(name, None, gs, out, depth);
    }

    fn answer(&mut self, response: &str, gs: &Gamestate) -> Option<String> {
        self.waiting = None;
        let open = self.open.take()?;
        self.open = self.below.pop();
        let sid = gs.systeminfo("sv_serverid")?;
        Some(format!("mr {sid} {} {response}", open.slot))
    }
}

/// An item's rect placed in window pixels: x, y, w, h.
pub fn placed(sp: &ScreenPlacement, (r, align): ([f32; 4], [i32; 2])) -> [f32; 4] {
    let (x, w) = sp.axis(0, r[0], r[2], align[0]);
    let (y, h) = sp.axis(1, r[1], r[3], align[1]);
    [x, y, w, h]
}

/// `Rect_ContainsPoint`.
pub(crate) fn contains(r: [f32; 4], p: [f32; 2]) -> bool {
    p[0] > r[0] && p[0] < r[0] + r[2] && p[1] > r[1] && p[1] < r[1] + r[3]
}

/// What the expressions read.
/// An item's `action` held across its key (`ScriptMenus::action_of`).
struct Action {
    menu: String,
    item: usize,
    cmds: Vec<Cmd>,
}

struct Ctx<'a>(&'a ScriptMenus);

impl Env for Ctx<'_> {
    fn dvar(&self, name: &str) -> Option<String> {
        self.0.dvar(name)
    }
    fn local(&self, name: &str) -> Option<Value> {
        self.0.ui.locals.get(&name.to_ascii_lowercase()).cloned()
    }
    fn stat(&self, index: i32) -> i32 {
        self.0.stat(index)
    }
    fn team(&self) -> i32 {
        self.0.team
    }
    fn table_lookup(&self, table: &str, column: i32, value: &str, ret: i32) -> String {
        self.0
            .tables
            .get(&table.to_ascii_lowercase())
            .map_or_else(String::new, |t| t.lookup(column, value, ret).to_string())
    }
    fn gametype(&self) -> String {
        self.0.gametype.clone()
    }
    fn gametype_name(&self) -> String {
        self.0.gametype_name.clone()
    }
    fn gametype_description(&self) -> String {
        self.0.objective.clone()
    }
    fn game(&self) -> Option<&menuexp::GameView> {
        Some(&self.0.game)
    }
    /// `UI_GetKeyBindingLocalizedStringSingle`: the first key's `KEY_*`
    /// text, else the key's own name; `KEY_UNBOUND`'s with none.
    fn key_binding(&self, cmd: &str) -> String {
        let get = |k: &str| {
            self.0
                .strings
                .as_ref()
                .and_then(|l| l.get(k).map(str::to_string))
        };
        match self.0.bound_keys(cmd).first() {
            Some(k) => get(&format!("KEY_{k}")).unwrap_or_else(|| k.clone()),
            None => get("KEY_UNBOUND").unwrap_or_else(|| "KEY_UNBOUND".into()),
        }
    }
    fn localize(&self, text: &str) -> String {
        match &self.0.strings {
            Some(l) => l.message(text),
            None => text.replace(['\u{14}', '\u{15}'], ""),
        }
    }
    /// `Menu_IsMenuOpenAndVisible`: the in-game menus keep one up, the
    /// front end a stack.
    fn menu_open(&self, name: &str) -> bool {
        self.0.is_open(name)
    }
}

fn menu_name(gs: &Gamestate, n: u16) -> String {
    String::from_utf8_lossy(gs.configstring(CS_SCRIPT_MENUS + n)).into_owned()
}

pub(crate) fn menu_slot(gs: &Gamestate, name: &str) -> Option<u16> {
    (0..MAX_SCRIPT_MENUS).find(|&i| {
        gs.configstring(CS_SCRIPT_MENUS + i)
            .eq_ignore_ascii_case(name.as_bytes())
    })
}

/// The stock team and class menus' responses as `common_mp`'s menus give
/// them (`vcod_cod4`'s `hud_assets` test pins them), for a session whose
/// zones are not loaded (the headless probe).
pub fn fallback() -> HashMap<String, ScriptMenu> {
    let menu = |name: &str, responses: &[&str], opens: &[&str]| ScriptMenu {
        name: name.to_string(),
        responses: responses.iter().map(|s| s.to_string()).collect(),
        esc: Some("back".into()),
        layout: None,
        opens: opens
            .iter()
            .map(|m| (Some(("onlinegame".into(), "1".into())), m.to_string()))
            .collect(),
    };
    let classes = [
        "assault_mp,0",
        "specops_mp,0",
        "heavygunner_mp,0",
        "demolitions_mp,0",
        "sniper_mp,0",
        "custom1,0",
        "custom2,0",
        "custom3,0",
        "custom4,0",
        "custom5,0",
    ];
    [
        menu(
            "team_marinesopfor",
            &["axis", "allies", "autoassign", "spectator"],
            &[],
        ),
        menu("changeclass_marines", &[], &["changeclass"]),
        menu("changeclass_opfor", &[], &["changeclass"]),
        menu("changeclass", &classes, &[]),
    ]
    .into_iter()
    .map(|m| (m.name.clone(), m))
    .collect()
}

/// A response as the list shows it: the team's name, a class's.
pub fn label(response: &str) -> String {
    match response {
        "allies" => "Marines".into(),
        "axis" => "OpFor".into(),
        "autoassign" => "Auto-assign".into(),
        "spectator" => "Spectator".into(),
        r => {
            let base = r.split(',').next().unwrap_or(r);
            let base = base.strip_suffix("_mp").unwrap_or(base);
            match base.strip_prefix("custom") {
                Some(n) => format!("Custom class {n}"),
                None => {
                    let mut c = base.chars();
                    c.next()
                        .map_or(String::new(), |f| f.to_uppercase().chain(c).collect())
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gamestate() -> Gamestate {
        let mut gs = Gamestate::default();
        gs.configstrings.insert(1, b"\\sv_serverid\\16".to_vec());
        gs.configstrings
            .insert(CS_SCRIPT_MENUS + 8, b"team_marinesopfor".to_vec());
        gs.configstrings
            .insert(CS_SCRIPT_MENUS + 10, b"changeclass_marines".to_vec());
        gs.configstrings
            .insert(CS_SCRIPT_MENUS + 15, b"changeclass".to_vec());
        gs
    }

    #[test]
    fn cod4_team_then_class_answer_on_their_own() {
        let gs = gamestate();
        let mut m = ScriptMenus::new(Join {
            team: Some("allies".into()),
            class: Some("assault_mp,0".into()),
        });
        assert_eq!(m.command(b"t 8", &gs), ["mr 16 8 allies"]);
        assert!(m.open().is_none());
        assert!(m.command(b"u", &gs).is_empty());
        // The class pick goes back from `changeclass`, the menu
        // `changeclass_marines` opens in its place.
        assert_eq!(m.command(b"t 10", &gs), ["mr 16 15 assault_mp,0"]);
        assert_eq!(m.opened, ["team_marinesopfor", "changeclass_marines"]);
    }

    #[test]
    fn cod4_a_menu_without_an_answer_waits_for_a_pick() {
        let gs = gamestate();
        let mut m = ScriptMenus::new(Join::default());
        assert!(m.command(b"t 8", &gs).is_empty());
        let open = m.open().unwrap();
        assert_eq!(open.responses[1], "allies");
        assert_eq!(m.choose(3, &gs).as_deref(), Some("mr 16 8 spectator"));
        assert!(m.command(b"t 8", &gs).is_empty());
        assert_eq!(m.escape(&gs), ["mr 16 8 back"]);
        assert!(m.open().is_none());
        assert_eq!(label("heavygunner_mp,0"), "Heavygunner");
        assert_eq!(label("custom2,0"), "Custom class 2");
    }
}
