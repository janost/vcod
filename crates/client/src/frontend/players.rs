//! `FEEDER_PLAYER_LIST`, the call vote menu's kick list, and `uiScript
//! voteKick` (cod11-votes.md section 5).

use std::time::{Duration, Instant};

use vcod_common::text::clean_str;
use vcod_common::ui_menu::{FEEDER_PLAYER_LIST, UiItem};
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
        self.player_top = self.player_top.min(self.players.len().saturating_sub(1));
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
        if item.feeder != Some(FEEDER_PLAYER_LIST) {
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
