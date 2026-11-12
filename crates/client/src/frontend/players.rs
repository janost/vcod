//! `FEEDER_PLAYER_LIST`, the call vote menu's kick list, and `uiScript
//! voteKick` (cod11-votes.md section 5); CoD2's `FEEDER_MUTELIST`, the
//! `muteplayer` script menu's list over the same rows, and `uiScript
//! mutePlayer` (cod2-voice.md 5).

use std::time::{Duration, Instant};

use vcod_common::localize::Localized;
use vcod_common::text::clean_str;
use vcod_common::ui_menu::{FEEDER_MUTELIST, FEEDER_PLAYER_LIST, UiItem};
use winit::keyboard::KeyCode;

use super::{
    HudQuad, Painter, SCROLLBAR_SIZE, Ui, UiEffect, listbox, truncate_visible, visible_rows,
};

/// `UI_FeederCount` rebuilds the list at most this often (ui_mp_x86.dll
/// 0x4000c66b, `realTime + 3000`).
const REFRESH: Duration = Duration::from_millis(3000);

impl Ui {
    /// `UI_BuildPlayerList` (0x40008ed0) when the last build is 3 s old:
    /// `names` gives the newest snapshot's client names in slot order,
    /// raw; each is kept cleaned. Call once a frame while connected.
    pub fn offer_players(&mut self, now: Instant, names: impl FnOnce() -> Vec<String>) {
        if self.players_due.is_some_and(|due| now <= due) {
            return;
        }
        self.players_due = Some(now + REFRESH);
        self.players = names().iter().map(|n| clean_str(n)).collect();
        self.player_clients.clear();
        self.player_top = self.player_top.min(self.players.len().saturating_sub(1));
    }

    /// CoD2's `UI_BuildPlayerList` (Mac 0x14f5e2) on the same 3 s clock
    /// (`UI_FeederCount`, Mac 0x156946): every client with a name, by
    /// number, the names cleaned; the mute list's rows.
    pub fn offer_clients(&mut self, now: Instant, rows: impl FnOnce() -> Vec<(u32, String)>) {
        if self.players_due.is_some_and(|due| now <= due) {
            return;
        }
        self.players_due = Some(now + REFRESH);
        let rows = rows();
        self.players = rows.iter().map(|(_, n)| clean_str(n)).collect();
        self.player_clients = rows.iter().map(|&(c, _)| c).collect();
        self.player_top = self.player_top.min(self.players.len().saturating_sub(1));
    }

    /// `CL_IsPlayerMuted` for the mute list's column, read live as the list
    /// draws: bit `n` for client `n`.
    pub fn set_muted(&mut self, muted: u64) {
        self.muted = muted;
    }

    /// `uiScript voteKick` (0x4000afc7): `callvote kick "<name>"` off the
    /// selected row; nothing when the row is past the list.
    pub(super) fn vote_kick(&self, out: &mut Vec<UiEffect>) {
        if let Some(name) = self.players.get(self.player_row) {
            out.push(UiEffect::Command(format!("callvote kick \"{name}\"")));
        }
    }

    /// A click on the kick list selects the row under the cursor
    /// (`UI_FeederSelection` 0x4000cdd9 keeps the index).
    pub(super) fn player_click(&mut self, item: &UiItem) {
        let row = ((self.cursor[1] - item.rect[1] - 1.0) / item.element_height.max(1.0)).floor();
        let row = row.max(0.0) as usize + self.player_top;
        if row < self.players.len() {
            self.player_row = row;
        }
    }

    /// A key with the cursor over the kick list, as over the map list.
    pub(super) fn player_list_key(&mut self, code: KeyCode) -> bool {
        let Some((m, i)) = self.hover else {
            return false;
        };
        let item = &self.menus[m].items[i];
        if !matches!(item.feeder, Some(FEEDER_PLAYER_LIST | FEEDER_MUTELIST)) {
            return false;
        }
        match listbox::key(
            item,
            self.players.len(),
            self.player_top,
            self.player_row,
            code,
        ) {
            Some((top, row)) => {
                (self.player_top, self.player_row) = (top, row);
                true
            }
            None => false,
        }
    }

    /// `Item_ListBox_Paint` for `FEEDER_PLAYER_LIST`: the cleaned names
    /// (`UI_FeederItemText` 0x4000ca50).
    pub(super) fn player_rows(&self, p: &Painter, item: &UiItem, out: &mut Vec<HudQuad>) {
        let [x, y0, w, _] = item.rect;
        let (x, mut y) = (x + 1.0, y0 + 1.0);
        let eh = item.element_height.max(1.0);
        let col = item.columns.first().copied();
        for (row, name) in self
            .players
            .iter()
            .enumerate()
            .skip(self.player_top)
            .take(visible_rows(item))
        {
            let text = truncate_visible(name, col.map_or(0, |c| c.max_chars));
            p.text(
                &text,
                x + 4.0 + col.map_or(0.0, |c| c.pos) + item.text_align_x,
                y + eh + item.text_align_y,
                item.text_scale,
                item.fore,
                out,
            );
            if row == self.player_row {
                p.fill(
                    [x, y, w - SCROLLBAR_SIZE - 4.0, eh - 1.0],
                    item.outline_color,
                    out,
                );
            }
            y += eh;
        }
    }

    /// `Item_ListBox_Paint` for CoD2's `FEEDER_MUTELIST`: column 0
    /// `MP_MUTED` for a muted client, column 1 the cleaned name
    /// (`UI_FeederItemText`, Mac 0x152e8a).
    pub(super) fn mute_rows(
        &self,
        p: &Painter,
        item: &UiItem,
        loc: &Localized,
        out: &mut Vec<HudQuad>,
    ) {
        let [x, y0, w, _] = item.rect;
        let (x, mut y) = (x + 1.0, y0 + 1.0);
        let eh = item.element_height.max(1.0);
        let muted = loc.message("MP_MUTED");
        for (row, name) in self
            .players
            .iter()
            .enumerate()
            .skip(self.player_top)
            .take(visible_rows(item))
        {
            let client = self.player_clients.get(row).copied().unwrap_or(u32::MAX);
            let on = client < 64 && self.muted >> client & 1 != 0;
            let cells = [if on { muted.as_str() } else { "" }, name.as_str()];
            for (k, cell) in cells.iter().enumerate() {
                let col = item.columns.get(k).copied();
                let text = truncate_visible(cell, col.map_or(0, |c| c.max_chars));
                if text.is_empty() {
                    continue;
                }
                p.text(
                    &text,
                    x + 4.0 + col.map_or(0.0, |c| c.pos) + item.text_align_x,
                    y + eh + item.text_align_y,
                    item.text_scale,
                    item.fore,
                    out,
                );
            }
            if row == self.player_row {
                p.fill(
                    [x, y, w - SCROLLBAR_SIZE - 4.0, eh - 1.0],
                    item.outline_color,
                    out,
                );
            }
            y += eh;
        }
    }

    /// The mute list's selected client, for a test.
    #[cfg(test)]
    pub(super) fn selected_client(&self) -> Option<u32> {
        self.player_clients.get(self.player_row).copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_name_drops_colours_and_controls() {
        assert_eq!(clean_str("^1Able^7\u{7f}"), "Able");
        assert_eq!(clean_str("a^^b"), "a^");
    }
}
