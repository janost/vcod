//! Contains routines ported from the RTCW-MP GPL source, Copyright (C) 1999-2010 id Software LLC, a ZeniMax Media company.
//! See NOTICE.
//!
//! The list boxes the UI feeds: the server and mod lists, the server
//! status list, scrolling, the wheel and the browser's per-frame poll.

use std::net::SocketAddrV4;
use std::time::Instant;

use vcod_common::localize::Localized;
use vcod_common::ui_menu::{
    FEEDER_ALLMAPS, FEEDER_MODS, FEEDER_MUTELIST, FEEDER_PLAYER_LIST, FEEDER_PLAYER_PROFILES,
    FEEDER_SERVERS, FEEDER_SERVERSTATUS, ITEM_TYPE_LISTBOX, UiItem, UiMenu,
};
use winit::keyboard::KeyCode;

use super::browser::{Filter, Source, Status};
use super::input::exec_key;
use super::paint::{Painter, truncate_visible};
use super::{K_MWHEELDOWN, K_MWHEELUP, SCROLLBAR_SIZE, UI_CVARS, Ui, UiEffect, browser, listbox};
use crate::console::shell::Shell;
use crate::hud::HudQuad;

impl Ui {
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
        self.browser.max_ping = shell.cvar_f32("cl_maxPing") as u32;
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

    /// `Item_ListBox_HandleKey` (0x40011da0) for the server list under the
    /// cursor, which the mouse pass ahead of it has focused: the arrows,
    /// Home, End and page keys move the selection. With nothing selected an
    /// arrow picks the first row.
    pub(super) fn server_list_key(&mut self, code: KeyCode) -> bool {
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

    /// A scrolled list's row count and first row drawn, for every feeder
    /// list that scrolls (maps, servers, mods, players and mutes, profiles,
    /// server info).
    pub(super) fn list_scroll(&self, item: &UiItem) -> Option<(usize, usize)> {
        if item.kind != ITEM_TYPE_LISTBOX {
            return None;
        }
        match item.feeder {
            Some(FEEDER_ALLMAPS) => Some((self.active_maps().len(), self.map_top)),
            Some(FEEDER_SERVERS) => Some((self.browser.row_count(), self.list_top)),
            Some(FEEDER_MODS) => Some((self.mods.len(), self.mod_top)),
            Some(FEEDER_PLAYER_LIST | FEEDER_MUTELIST) => {
                Some((self.players.len(), self.player_top))
            }
            Some(FEEDER_PLAYER_PROFILES) => Some((self.profiles.len(), self.profiles.top)),
            Some(FEEDER_SERVERSTATUS) => self.status.as_ref().map(|q| (q.rows.len(), q.top)),
            _ => None,
        }
    }

    pub(super) fn set_list_top(&mut self, feeder: Option<i32>, top: usize) {
        match feeder {
            Some(FEEDER_ALLMAPS) => self.map_top = top,
            Some(FEEDER_SERVERS) => self.list_top = top,
            Some(FEEDER_MODS) => self.mod_top = top,
            Some(FEEDER_PLAYER_LIST | FEEDER_MUTELIST) => self.player_top = top,
            Some(FEEDER_PLAYER_PROFILES) => self.profiles.top = top,
            Some(FEEDER_SERVERSTATUS) => {
                if let Some(q) = &mut self.status {
                    q.top = top;
                }
            }
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
        let len = self.browser.row_count();
        self.list_top = if up {
            self.list_top.saturating_sub(n / 2)
        } else {
            (self.list_top + n / 2).min(len.saturating_sub(n))
        };
    }

    /// A wheel notch. Over the map, Mods, player, mute or server info list it
    /// scrolls that list a row, and in a menu with the server list it pages
    /// that list; retail scrolls no list, and vcod keeps the scroll on purpose.
    /// Elsewhere, as in retail: no item's key handler takes the wheel, so the
    /// menu's `execKey` for `MWHEELDOWN` (205) or `MWHEELUP` (206) runs, else
    /// it steps the focus as Tab does, and its release follows in the same
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

    /// The wheel's list scroll; false when no list takes it.
    fn wheel_scroll(&mut self, top: usize, up: bool) -> bool {
        if let Some((m, i)) = self.hover.filter(|&(m, _)| m == top) {
            let item = &self.menus[m].items[i];
            if matches!(
                item.feeder,
                Some(
                    FEEDER_ALLMAPS
                        | FEEDER_MODS
                        | FEEDER_PLAYER_LIST
                        | FEEDER_MUTELIST
                        | FEEDER_SERVERSTATUS
                )
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

    /// `Item_ListBox_Paint` for `FEEDER_SERVERSTATUS`: the server info
    /// popup's rows, not selectable; a cell holding a localized key prints
    /// its text.
    pub(super) fn status_list(
        &self,
        p: &Painter,
        item: &UiItem,
        loc: &Localized,
        out: &mut Vec<HudQuad>,
    ) {
        let Some(q) = &self.status else { return };
        let [x, y0, _, _] = item.rect;
        let (x, mut y) = (x + 1.0, y0 + 1.0);
        let eh = item.element_height.max(1.0);
        let top = q.top.min(listbox::max_scroll(item, q.rows.len()));
        for row in q.rows.iter().skip(top).take(visible_rows(item)) {
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
    pub(super) fn mod_list_key(&mut self, code: KeyCode) -> bool {
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
    pub(super) fn mod_rows(&self, p: &Painter, item: &UiItem, out: &mut Vec<HudQuad>) {
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

    /// A server list cell ([`browser::column_cell`]). CoD2's map column
    /// (`UI_FeederItemText` column 3, Mac 0x152e8a) prints the `longname`
    /// of the `.arena` entry whose `map` is the server's `mapname` in any
    /// case, the `mapname` when none is.
    pub(super) fn list_cell(&self, s: &browser::Server, kind: browser::Column) -> browser::Cell {
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

    /// `Item_ListBox_Paint` for `FEEDER_SERVERS`. A cell's icon is a square
    /// of the column's width from half a row down, and takes no text align
    /// (ui_mp_x86.dll 0x400159b0).
    pub(super) fn server_list(&self, p: &Painter, item: &UiItem, out: &mut Vec<HudQuad>) {
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

/// The server list's layout off its list box: 1.5's `joinserver.menu`
/// lists nine columns to 1.1's six.
pub(super) fn server_columns(menus: &[UiMenu]) -> &'static [browser::Column] {
    (menus.iter().flat_map(|m| &m.items))
        .find(|i| i.feeder == Some(FEEDER_SERVERS) && !i.columns.is_empty())
        .map_or(&browser::COLUMNS_V1, |i| {
            browser::columns_for(i.columns.len())
        })
}

/// Rows a list box shows whole: Q3 stops at the last that fits.
pub(super) fn visible_rows(item: &UiItem) -> usize {
    ((item.rect[3] - 2.0) / item.element_height.max(1.0))
        .floor()
        .max(1.0) as usize
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::time::Duration;

    use vcod_common::ui_menu;

    use super::super::status::StatusQuery;
    use super::super::tests::{
        click_at, click_item, click_named, map_list, top_name, type_text, ui, ui_with_menu,
    };

    /// The server info popup's rows past its first page are reached by the
    /// wheel: a long serverinfo no longer hides the player rows.
    #[test]
    fn the_server_info_list_scrolls() {
        let Some(mut ui) = ui() else { return };
        let shell = Shell::new();
        let (m, i) = ui
            .menus
            .iter()
            .enumerate()
            .find_map(|(m, menu)| {
                let i = menu.items.iter().position(|i| {
                    i.kind == ITEM_TYPE_LISTBOX && i.feeder == Some(FEEDER_SERVERSTATUS)
                })?;
                Some((m, i))
            })
            .expect("a stock menu lists the server's status");
        let rows = (0..40)
            .map(|n| {
                [
                    format!("key{n}"),
                    n.to_string(),
                    String::new(),
                    String::new(),
                ]
            })
            .collect();
        let name = ui.menus[m].name.clone();
        let mut out = Vec::new();
        // Opening runs the menu's `serverstatus` query; the rows come after.
        ui.open_menu(&name, &shell, &mut out);
        ui.status = Some(StatusQuery::with_rows(rows));
        let r = ui.menus[m].items[i].rect;
        ui.mouse_move(r[0] + r[2] / 2.0, r[1] + r[3] / 2.0, 640.0, 480.0, &shell);
        assert_eq!(ui.hover, Some((m, i)));
        ui.wheel(true, &shell);
        ui.wheel(true, &shell);
        assert_eq!(ui.status.as_ref().unwrap().top, 2);
        let item = ui.menus[m].items[i].clone();
        assert_eq!(ui.list_scroll(&item), Some((40, 2)));
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

    #[test]
    fn a_new_favourite_is_typed_in_and_listed() {
        let Some(ui) = ui() else { return };
        let dir = tempfile::tempdir().unwrap();
        let mut ui = ui.with_server_cache(
            dir.path().join("servercache.dat"),
            vcod_common::net::server_cache::Layout::Cod11,
        );
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
}
