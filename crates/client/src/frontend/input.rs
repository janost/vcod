//! Contains routines ported from the RTCW-MP GPL source, Copyright (C) 1999-2010 id Software LLC, a ZeniMax Media company.
//! See NOTICE.
//!
//! Mouse and key input: the cursor, focus, clicks, key presses and the
//! focus steps between items.

use std::time::Instant;

use vcod_common::ui_menu::{
    FEEDER_ALLMAPS, FEEDER_MODS, FEEDER_MUTELIST, FEEDER_PLAYER_LIST, FEEDER_PLAYER_PROFILES,
    FEEDER_SERVERS, ITEM_TYPE_BIND, ITEM_TYPE_DVARENUM, ITEM_TYPE_LISTBOX, ITEM_TYPE_MULTI,
    ITEM_TYPE_OWNERDRAW, ITEM_TYPE_SLIDER, ITEM_TYPE_TEXT, ITEM_TYPE_YESNO, UiItem, UiMenu,
};
use winit::keyboard::KeyCode;

use super::browser::Source;
use super::paint::is_edit_field;
use super::{
    DOUBLE_CLICK, K_ENTER, K_KP_ENTER, K_MOUSE1, K_MOUSE2, K_MOUSE3, K_MOUSE4, K_MOUSE5,
    UI_JOINGAMETYPE, UI_NETGAMETYPE, UI_NETSOURCE, UI_SHOW_FAVORITESERVERS,
    UI_SHOW_NOTFAVORITESERVERS, Ui, UiEffect, listbox,
};
use crate::console::shell::Shell;
use crate::hud::hudelem::Virtual;

impl Ui {
    /// Whether `item` of `menu` is drawn and can take the mouse.
    pub(super) fn shown(&self, menu: usize, i: usize, shell: &Shell) -> bool {
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
        if let Some(item) = self
            .thumb_drag
            .and_then(|(m, i)| self.menus.get(m)?.items.get(i))
            && let Some((len, _)) = self.list_scroll(item)
        {
            let top = listbox::drag(item, len, self.cursor[1]);
            let feeder = item.feeder;
            self.set_list_top(feeder, top);
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
    pub(super) fn track_cursor(&mut self, shell: &Shell, out: &mut Vec<UiEffect>) {
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
            if is_edit_field(item.kind) {
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

    /// `Menu_SetNextCursorItem` (0x40013240, `down`) or `_Prev`
    /// (0x40013160): the focus moves to the next item that takes it and
    /// `Menu_HandleMouseMove` runs at that item's corner, which leaves the
    /// hovered items and enters the ones there. The next key event's mouse
    /// pass at the cursor ([`Self::settle_focus`]) undoes it.
    pub(super) fn step_focus(
        &mut self,
        top: usize,
        down: bool,
        shell: &Shell,
        out: &mut Vec<UiEffect>,
    ) {
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
    pub(crate) fn edit_step(&mut self, down: bool, shell: &Shell, out: &mut Vec<UiEffect>) {
        let Some((m, _)) = self.options.editing else {
            return;
        };
        let Some(target) = self.next_focus(m, down, shell) else {
            return;
        };
        let cursor = self.cursor;
        self.set_focus(m, target, cursor, shell, out);
        if is_edit_field(self.menus[m].items[target].kind) {
            self.options.editing = Some((m, target));
        }
    }

    /// The next key event: `Menu_HandleKey` runs `Menu_HandleMouseMove`
    /// for the focused menu at the real cursor, which leaves what a focus
    /// step entered and enters the hovered items again; the hovered item
    /// takes the focus and the cursor item back.
    pub(super) fn settle_focus(&mut self, shell: &Shell, out: &mut Vec<UiEffect>) {
        if let Some(m) = self.focused_menu() {
            self.menu_mouse_move(m, shell, out);
        }
    }

    /// Whether `item` of `menu` is drawn in the focus colour: it has the
    /// focus bit (`Item_TextColor`, 0x40013df0), whether the mouse, a focus
    /// step or an edit's Tab put it there.
    pub(super) fn focused(&self, menu: usize, item: usize) -> bool {
        self.focus.get(&menu) == Some(&item)
    }
}

/// `menu`'s `execKey` script for retail's key number `n`
/// ([`crate::console::keys::number`]).
pub(super) fn exec_key(menu: &UiMenu, n: u32) -> Option<Vec<String>> {
    // `Menu_HandleKey` tries the slot for keys 1..255.
    let n = u8::try_from(n).ok().filter(|&n| n != 0)?;
    let (_, script) = menu.exec_keys.iter().find(|(k, _)| *k == n)?;
    Some(script.clone())
}

pub(super) fn contains(r: [f32; 4], p: [f32; 2]) -> bool {
    p[0] >= r[0] && p[0] <= r[0] + r[2] && p[1] >= r[1] && p[1] <= r[1] + r[3]
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::collections::HashMap;
    use std::time::Duration;

    use vcod_common::localize::Localized;
    use vcod_common::ui_menu;
    use vcod_common::ui_menu::ITEM_TYPE_EDITFIELD;

    use super::super::tests::{
        apply, click_at, click_item, click_named, main_at_start, map_list, response, start_as_text,
        top_name, ui, ui_with_team_menu,
    };

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
}
