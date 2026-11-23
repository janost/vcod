//! Contains routines ported from the RTCW-MP GPL source, Copyright (C) 1999-2010 id Software LLC, a ZeniMax Media company.
//! See NOTICE.
//!
//! The menu stack: which menus are open, opening and closing them by name
//! and the main, in-game, error and script menus.

use std::collections::HashMap;

use winit::keyboard::KeyCode;

use super::input::exec_key;
use super::script::unsupported;
use super::{
    UIMENU_INGAME, UIMENU_MAIN, UIMENU_NONE, UIMENU_SCRIPT_POPUP, UIMENU_WM_QUICKMESSAGE, Ui,
    UiEffect,
};
use crate::console::shell::Shell;

impl Ui {
    /// Whether a menu is up and taking input.
    pub fn active(&self) -> bool {
        !self.open.is_empty() && !self.down
    }

    /// `UI_IS_FULLSCREEN` (ui_mp_x86.dll 0x40016ab0): a menu on the stack
    /// flagged visible and fullscreen, a UI taken down by a key included.
    pub fn fullscreen(&self) -> bool {
        self.open.iter().any(|&m| self.menus[m].fullscreen)
    }

    /// The open menus `Menu_PaintAll` paints, bottom first: the stack from
    /// its topmost fullscreen menu up, a hidden one counting (1.1
    /// 0x40018a00..0x40018a46, cod11-front-end.md 18.4.1).
    pub(super) fn painted(&self) -> &[usize] {
        let open = self.open.iter().rposition(|&m| self.menus[m].fullscreen);
        let hidden = self
            .hidden
            .iter()
            .filter(|&&(m, _)| self.menus[m].fullscreen);
        let from = open
            .into_iter()
            .chain(hidden.map(|&(_, below)| below))
            .max();
        &self.open[from.unwrap_or(0)..]
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
    pub(super) fn unfocused_key(&mut self) -> bool {
        if self.active() && self.focused_menu().is_none() {
            self.down = true;
            return true;
        }
        false
    }

    /// The main menu, as retail shows it with no game up. While connecting
    /// or loading this is also what Esc opens (`UIMENU_MAIN`).
    pub fn open_main(&mut self, shell: &Shell, out: &mut Vec<UiEffect>) {
        self.server_cvars.clear();
        self.in_game = false;
        self.active_type = UIMENU_MAIN;
        self.open_menu("main", shell, out);
    }

    /// `UI_SetActiveMenu` 1 (ui_mp_x86.dll 0x4000d8a9): the UI's key
    /// catcher up and `main` opened, then `error_popmenu` while
    /// `com_errorMessage` holds anything but `;` (0x4000d8ef). Nothing
    /// closes first.
    pub fn set_active_main(&mut self, shell: &Shell, out: &mut Vec<UiEffect>) {
        self.open_main(shell, out);
        if !self.error.is_empty() && !self.error.eq_ignore_ascii_case(";") {
            self.open_menu("error_popmenu", shell, out);
        }
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
        self.server_cvars.clear();
        self.in_game = false;
        self.active_type = UIMENU_MAIN;
        self.open_menu("main", shell, out);
        self.open_menu("error_popmenu", shell, out);
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

    /// `CG_MapRestart`'s menu closes (events doc 9.8): the script popup
    /// twice as `u` closes it, its `scriptMenuResponse`s dropped as
    /// `ui_scriptMenuAllowResponse` 0 drops them (ui_mp_x86.dll
    /// 0x40011110); then each Escape syscall's three down Escs straight to
    /// the UI, skipped while the active menu type is `UIMENU_MAIN` (1.1,
    /// UI `vmMain` 8) or a fullscreen menu is up (1.5 and UO, `vmMain` 6).
    pub fn map_restart(
        &mut self,
        closes: crate::play::join::RestartCloses,
        shell: &Shell,
    ) -> Vec<UiEffect> {
        let mut out = Vec::new();
        if closes.popups {
            for _ in 0..2 {
                out.extend(self.close_script_popup(shell));
            }
            out.retain(|e| !matches!(e, UiEffect::ScriptMenuResponse { .. }));
        }
        for _ in 0..closes.escapes {
            let skip = if closes.skip_on_main {
                self.active_type == UIMENU_MAIN
            } else {
                self.fullscreen()
            };
            if !skip {
                for _ in 0..3 {
                    out.extend(self.key(KeyCode::Escape, shell).1);
                }
            }
        }
        out
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
    pub(super) fn close_every_menu(&mut self, shell: &Shell, out: &mut Vec<UiEffect>) {
        for m in 0..self.menus.len() {
            if self.on_stack(m) {
                self.close_at(m, shell, out);
            }
        }
    }

    /// Whether the loaded menus hold `name`, as `Menus_ActivateByName`
    /// looks it up (0x400134b0).
    pub fn has_menu(&self, name: &str) -> bool {
        self.find(name).is_some()
    }

    pub(super) fn find(&self, name: &str) -> Option<usize> {
        self.menus
            .iter()
            .position(|m| m.name.eq_ignore_ascii_case(name))
    }

    /// `Menus_Activate` (0x40013400): the menu goes on top of the stack and
    /// takes the focus, its `onOpen` runs, then `Display_MouseMove` at the
    /// cursor.
    pub(super) fn open_menu(&mut self, name: &str, shell: &Shell, out: &mut Vec<UiEffect>) {
        // Retail recurses forever on a menu that opens itself; vcod stops.
        const MAX_OPENING: u32 = 16;
        let Some(m) = self.find(name).filter(|_| !unsupported(name)) else {
            crate::console::log::print(&format!("The {name} menu is not in vcod yet."));
            return;
        };
        if self.opening >= MAX_OPENING {
            log::warn!("menu {name}: opened {MAX_OPENING} deep, not opened");
            return;
        }
        self.opening += 1;
        self.open_menu_at(m, shell, out);
        self.opening -= 1;
    }

    fn open_menu_at(&mut self, m: usize, shell: &Shell, out: &mut Vec<UiEffect>) {
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
    pub(super) fn close_menu(&mut self, name: &str, shell: &Shell, out: &mut Vec<UiEffect>) {
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
    pub(super) fn leave_stack(&mut self, m: usize) {
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
    pub(super) fn focused_menu(&self) -> Option<usize> {
        self.focus_menu.filter(|m| self.open.contains(m))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::time::Instant;

    use vcod_common::localize::Localized;
    use vcod_common::pk3::Pk3Fs;

    use super::super::input::contains;
    use super::super::tests::{
        click_item, response, top_name, ui, ui_with_menu, ui_with_team_menu,
    };

    /// `UI_SetActiveMenu` 1, which the disconnected frame and Esc run:
    /// main, the error popup over it while an error other than `;` waits.
    #[test]
    fn set_active_main_reopens_main_and_a_waiting_error() {
        let Some(mut ui) = ui() else { return };
        let shell = Shell::new();
        let names = |ui: &Ui| -> Vec<String> {
            ui.open.iter().map(|&m| ui.menus[m].name.clone()).collect()
        };
        ui.set_active_main(&shell, &mut Vec::new());
        assert_eq!(names(&ui), ["main"]);
        ui.close_all();
        ui.error = "Server timed out".into();
        ui.set_active_main(&shell, &mut Vec::new());
        assert_eq!(names(&ui), ["main", "error_popmenu"]);
        ui.close_all();
        ui.error = ";".into();
        ui.set_active_main(&shell, &mut Vec::new());
        assert_eq!(names(&ui), ["main"]);
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

    /// The driver info and credits pages open (a mod or the console can
    /// reach credits; the stock button is commented out).
    #[test]
    fn the_driver_info_and_credits_pages_open() {
        let Some(mut ui) = ui() else { return };
        let shell = Shell::new();
        let mut out = Vec::new();
        ui.open_main(&shell, &mut out);
        for name in ["options_credits", "options_driverinfo"] {
            ui.open_menu(name, &shell, &mut out);
            assert!(ui.open.iter().any(|&m| ui.menus[m].name == name), "{name}");
        }
        assert!(
            !ui.build(640.0, 480.0, &Localized::default(), &shell)
                .is_empty()
        );
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

    /// Menus that open themselves or each other from `onOpen` or
    /// `mouseEnter` end.
    #[test]
    fn a_menu_opening_itself_ends() {
        let mut ui = ui_with_menu(
            r#"{ menuDef { name "main" rect 0 0 640 480 onOpen { open main } }
            menuDef { name "a" rect 0 0 640 480 onOpen { open b } }
            menuDef { name "b" rect 0 0 640 480 onOpen { open a } }
            menuDef { name "c" rect 0 0 640 480
              itemDef { rect 0 0 640 480 visible 1 type 1 mouseEnter { open c } } } }"#,
        );
        let shell = Shell::new();
        ui.open_main(&shell, &mut Vec::new());
        ui.open_menu("a", &shell, &mut Vec::new());
        ui.mouse_move(320.0, 240.0, 640.0, 480.0, &shell);
        ui.open_menu("c", &shell, &mut Vec::new());
        assert_eq!(ui.opening, 0);
        assert_eq!(ui.focused_menu(), ui.find("c"));
    }

    /// Off the server, a menu reads the console again, not the last
    /// server's cvars: a drop and the main menu both forget them.
    #[test]
    fn the_server_cvars_go_with_the_game() {
        let mut ui = ui_with_menu(r#"{ menuDef { name "main" rect 0 0 640 480 } }"#);
        let mut shell = Shell::new();
        shell.execute("set scr_allow_panzerfaust 1");
        let server = || HashMap::from([("scr_allow_panzerfaust".into(), "0".into())]);
        ui.set_server_cvars(server());
        assert_eq!(
            ui.cvar("scr_allow_panzerfaust", &shell).as_deref(),
            Some("0")
        );
        ui.show_error("dropped", &shell, &mut Vec::new());
        assert_eq!(
            ui.cvar("scr_allow_panzerfaust", &shell).as_deref(),
            Some("1")
        );
        ui.set_server_cvars(server());
        ui.open_main(&shell, &mut Vec::new());
        assert_eq!(
            ui.cvar("scr_allow_panzerfaust", &shell).as_deref(),
            Some("1")
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
    fn a_map_restart_closes_the_menus_as_each_build_does() {
        use crate::play::join::RestartCloses;
        let shell = Shell::new();
        let restart = |cmd: &str, v1, uo| RestartCloses::for_command(&[cmd.to_string()], v1, uo);
        // the popup's `close` stays home under `ui_scriptMenuAllowResponse` 0
        let mut ui = ui_with_team_menu();
        ui.open_script_popup("team_x", false, &shell, &mut Vec::new());
        assert!(ui.map_restart(restart("n", true, false), &shell).is_empty());
        assert!(!ui.active());
        // a menu Esc opened: 1.5's `n` leaves it, 1.1's Escs close it, and
        // its `close` goes out, the cvar back at 1
        ui.open_script_main("team_x", &shell, &mut Vec::new());
        assert!(
            ui.map_restart(restart("n", false, false), &shell)
                .is_empty()
        );
        assert!(ui.active());
        assert_eq!(
            ui.map_restart(restart("n", true, false), &shell),
            [response("team_x", "close")]
        );
        assert!(!ui.active());
        // 1.1 sends no Esc while the main menu's type is active
        ui.open_script_main("team_x", &shell, &mut Vec::new());
        ui.active_type = UIMENU_MAIN;
        assert!(ui.map_restart(restart("n", true, false), &shell).is_empty());
        assert!(ui.active());
        // UO's `n` sends its one set whatever the type
        assert_eq!(
            ui.map_restart(restart("n", false, true), &shell),
            [response("team_x", "close")]
        );
        assert!(!ui.active());
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
        proto: &vcod_common::net::protocol::Cod1Wire,
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
}
