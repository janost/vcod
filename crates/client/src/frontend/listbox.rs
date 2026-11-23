//! A vertical list box's scroll bar, as `ui_mp_x86.dll` 1.1 draws and works
//! it (docs/research/cod11-front-end.md, section 19): the
//! arrows, the bar and the thumb on the right edge, a click on each part,
//! the thumb's drag, a held arrow's repeat and the keys over the list.

use std::time::{Duration, Instant};

use vcod_common::ui_menu::{
    FEEDER_ALLMAPS, FEEDER_MODS, FEEDER_MUTELIST, FEEDER_PLAYER_LIST, FEEDER_PLAYER_PROFILES,
    FEEDER_SERVERS, FEEDER_SERVERSTATUS, UiItem,
};
use winit::keyboard::KeyCode;

use super::SCROLLBAR_SIZE;
use super::paint::Painter;
use crate::hud::HudQuad;

const ARROW_UP: &str = "ui/assets/scrollbar_arrow_up_a.tga";
const ARROW_DOWN: &str = "ui/assets/scrollbar_arrow_dwn_a.tga";
const BAR: &str = "ui/assets/scrollbar.tga";
const THUMB: &str = "ui/assets/scrollbar_thumb.tga";

/// The first row each scrolled feeder list draws. The mute list shares
/// the player list's: they are one list (`UI_FeederCount`).
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct Tops([usize; 6]);

impl Tops {
    fn slot(feeder: i32) -> Option<usize> {
        Some(match feeder {
            FEEDER_ALLMAPS => 0,
            FEEDER_SERVERS => 1,
            FEEDER_MODS => 2,
            FEEDER_PLAYER_LIST | FEEDER_MUTELIST => 3,
            FEEDER_PLAYER_PROFILES => 4,
            FEEDER_SERVERSTATUS => 5,
            _ => return None,
        })
    }

    /// `None` for a feeder that does not scroll.
    pub(super) fn get(&self, feeder: i32) -> Option<usize> {
        Self::slot(feeder).map(|i| self.0[i])
    }

    pub(super) fn set(&mut self, feeder: i32, top: usize) {
        if let Some(i) = Self::slot(feeder) {
            self.0[i] = top;
        }
    }
}

impl std::ops::Index<i32> for Tops {
    type Output = usize;
    fn index(&self, feeder: i32) -> &usize {
        &self.0[Self::slot(feeder).expect("a scrolled feeder")]
    }
}

impl std::ops::IndexMut<i32> for Tops {
    fn index_mut(&mut self, feeder: i32) -> &mut usize {
        &mut self.0[Self::slot(feeder).expect("a scrolled feeder")]
    }
}

/// The part of the bar under a point (`Item_ListBox_OverLB`, 0x40011840:
/// the window flags 0x800, 0x1000, 0x2000, 0x4000, 0x8000).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Part {
    Up,
    Down,
    Thumb,
    PageUp,
    PageDown,
}

/// Rows a page holds: `rect.h / elementHeight` truncated (0x40011e1b).
pub(super) fn view_max(item: &UiItem) -> usize {
    (item.rect[3] / item.element_height.max(1.0)).max(0.0) as usize
}

/// `Item_ListBox_MaxScroll` (0x400115a0): the last first row, one past a
/// full last page.
pub(super) fn max_scroll(item: &UiItem, len: usize) -> usize {
    (len + 1).saturating_sub(view_max(item))
}

/// `Item_ListBox_ThumbPosition` (0x400115f0): the thumb's top.
fn thumb_y(item: &UiItem, len: usize, top: usize) -> f32 {
    let max = max_scroll(item, len);
    let step = if max > 0 {
        (item.rect[3] - 2.0 * SCROLLBAR_SIZE - 2.0 - SCROLLBAR_SIZE) / max as f32
    } else {
        0.0
    };
    (item.rect[1] + 1.0 + SCROLLBAR_SIZE + step * top as f32).trunc()
}

fn inside(r: [f32; 4], p: [f32; 2]) -> bool {
    super::contains(r, p)
}

/// `Item_ListBox_OverLB` for a vertical list.
pub(super) fn part_at(item: &UiItem, len: usize, top: usize, p: [f32; 2]) -> Option<Part> {
    let [x, y, w, h] = item.rect;
    let bx = x + w - SCROLLBAR_SIZE;
    let s = SCROLLBAR_SIZE;
    let thumb = thumb_y(item, len, top);
    [
        (Part::Up, [bx, y, s, s]),
        (Part::Down, [bx, y + h - s, s, s]),
        (Part::Thumb, [bx, thumb, s, s]),
        (Part::PageUp, [bx, y + s, s, thumb - (y + s)]),
        (Part::PageDown, [bx, thumb + s, s, y + h - s - (thumb + s)]),
    ]
    .into_iter()
    .find(|&(_, r)| inside(r, p))
    .map(|(part, _)| part)
}

/// A click on `part` (`Item_ListBox_HandleKey`, 0x40011f67): the first row
/// after it. The thumb only starts a drag.
pub(super) fn click(item: &UiItem, len: usize, top: usize, part: Part) -> usize {
    let (max, page) = (max_scroll(item, len), view_max(item));
    match part {
        Part::Up => top.saturating_sub(1),
        Part::Down => (top + 1).min(max),
        Part::PageUp => top.saturating_sub(page),
        Part::PageDown => (top + page).min(max),
        Part::Thumb => top,
    }
}

/// The first row for the thumb dragged to `cursor_y`
/// (`Scroll_ListBox_ThumbFunc`, 0x40012b90, run only when the cursor moved).
pub(super) fn drag(item: &UiItem, len: usize, cursor_y: f32) -> usize {
    let start = item.rect[1] + SCROLLBAR_SIZE + 1.0;
    let span = item.rect[3] - 2.0 * SCROLLBAR_SIZE - 2.0 - SCROLLBAR_SIZE;
    let max = max_scroll(item, len);
    if span <= 0.0 {
        return 0;
    }
    let pos = (cursor_y - start - SCROLLBAR_SIZE / 2.0) * max as f32 / span;
    (pos.max(0.0) as usize).min(max)
}

/// A held scroll bar arrow (`Scroll_ListBox_AutoFunc`, 0x40012b20, armed
/// by `Item_StartCapture`, 0x40012dd0): the press steps once, the next step
/// comes 500 ms on, and the gap shrinks by 40 ms every 150 ms down to 20.
#[derive(Clone, Copy, Debug)]
pub(super) struct Repeat {
    next_scroll: Instant,
    next_adjust: Instant,
    adjust_ms: u64,
}

impl Repeat {
    pub(super) fn new(now: Instant) -> Repeat {
        Repeat {
            next_scroll: now + Duration::from_millis(500),
            next_adjust: now + Duration::from_millis(150),
            adjust_ms: 500,
        }
    }

    /// Once a frame while the button is down: true when the arrow steps.
    pub(super) fn due(&mut self, now: Instant) -> bool {
        let step = now > self.next_scroll;
        if step {
            self.next_scroll = now + Duration::from_millis(self.adjust_ms);
        }
        if now > self.next_adjust {
            self.next_adjust = now + Duration::from_millis(150);
            if self.adjust_ms > 20 {
                self.adjust_ms -= 40;
            }
        }
        step
    }
}

/// A key over a list with `len` rows (`Item_ListBox_HandleKey`'s vertical
/// half, 0x40011da0): the arrows step the selection, Home and End jump to
/// the ends, Page Up and Down move it a page (`h / elementHeight`); the
/// first row follows so the selection stays in view. The keypad's keys do
/// the same. An empty list still takes them. `None` when the key is not the
/// list's.
pub(super) fn key(
    item: &UiItem,
    len: usize,
    top: usize,
    sel: usize,
    code: KeyCode,
) -> Option<(usize, usize)> {
    use KeyCode::*;
    let page = view_max(item);
    let last = len.saturating_sub(1);
    let sel = match code {
        ArrowUp | Numpad8 => sel.saturating_sub(1),
        ArrowDown | Numpad2 => (sel + 1).min(last),
        Home | Numpad7 => 0,
        End | Numpad1 => last,
        PageUp | Numpad9 => sel.saturating_sub(page),
        PageDown | Numpad3 => (sel + page).min(last),
        _ => return None,
    };
    if len == 0 {
        return Some((top, 0));
    }
    let mut top = if matches!(code, Home | Numpad7) {
        0
    } else {
        top.min(sel)
    };
    if sel >= top + page {
        top = (sel + 1).saturating_sub(page);
    }
    Some((top, sel))
}

/// `Item_ListBox_Paint`'s bar (0x400159b0): the up arrow, the bar, the down
/// arrow, then the thumb, kept above the down arrow.
pub(super) fn paint(
    p: &Painter,
    item: &UiItem,
    len: usize,
    top: usize,
    thumb_at: Option<f32>,
    out: &mut Vec<HudQuad>,
) {
    let [x, y, w, h] = item.rect;
    let s = SCROLLBAR_SIZE;
    let x = x + w - s - 1.0;
    let mut y = y + 1.0;
    p.quad([x, y, s, s], [1.0; 4], ARROW_UP, out);
    y += s - 1.0;
    let size = h - 2.0 * s;
    p.quad([x, y, s, size + 1.0], [1.0; 4], BAR, out);
    y += size - 1.0;
    p.quad([x, y, s, s], [1.0; 4], ARROW_DOWN, out);
    let thumb = thumb_at.map_or_else(|| thumb_y(item, len, top), |t| t.trunc());
    p.quad([x, thumb.min(y - s - 1.0), s, s], [1.0; 4], THUMB, out);
}

/// Where `Item_ListBox_Paint` (0x400159b0) puts a row: `x` one in from the
/// box's left, `y` the row's top, `eh` the row height.
#[derive(Clone, Copy)]
pub(super) struct Row {
    pub(super) x: f32,
    pub(super) y: f32,
    pub(super) eh: f32,
}

impl Row {
    /// A text cell of the column at `col_pos`.
    pub(super) fn text(
        &self,
        p: &Painter,
        item: &UiItem,
        text: &str,
        col_pos: f32,
        out: &mut Vec<HudQuad>,
    ) {
        p.text(
            text,
            self.x + 4.0 + col_pos + item.text_align_x,
            self.y + self.eh + item.text_align_y,
            item.text_scale,
            item.fore,
            item.text_style,
            out,
        );
    }
}

/// `Item_ListBox_Paint`'s rows for a feeder of `len`: from `top`, as many
/// as fit whole, each drawn by `cells`, then outlined when `selected`.
pub(super) fn paint_rows(
    p: &Painter,
    item: &UiItem,
    len: usize,
    top: usize,
    selected: impl Fn(usize) -> bool,
    mut cells: impl FnMut(usize, Row, &mut Vec<HudQuad>),
    out: &mut Vec<HudQuad>,
) {
    let [x, y, w, _] = item.rect;
    let eh = item.element_height.max(1.0);
    let mut row = Row {
        x: x + 1.0,
        y: y + 1.0,
        eh,
    };
    for i in (top..len).take(super::lists::visible_rows(item)) {
        cells(i, row, out);
        if selected(i) {
            p.fill(
                [row.x, row.y, w - SCROLLBAR_SIZE - 4.0, eh - 1.0],
                item.outline_color,
                out,
            );
        }
        row.y += eh;
    }
}

/// The row under `cursor_y` in a list whose first row drawn is `top`.
pub(super) fn row_at(item: &UiItem, cursor_y: f32, top: usize) -> usize {
    let row = ((cursor_y - item.rect[1] - 1.0) / item.element_height.max(1.0)).floor();
    row.max(0.0) as usize + top
}

/// `Item_ListBox_ThumbDrawPosition` (0x400116a0): while dragged, the thumb
/// sits half its size above the cursor inside the bar's travel; else where
/// the first row puts it.
pub(super) fn drag_thumb(item: &UiItem, cursor_y: f32) -> Option<f32> {
    let min = item.rect[1] + SCROLLBAR_SIZE + 1.0;
    let max = item.rect[1] + item.rect[3] - 2.0 * SCROLLBAR_SIZE - 1.0;
    let half = SCROLLBAR_SIZE / 2.0;
    (cursor_y >= min + half && cursor_y <= max + half).then_some(cursor_y - half)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_mute_list_scrolls_with_the_player_list_and_others_apart() {
        let mut t = Tops::default();
        t.set(FEEDER_MUTELIST, 3);
        t.set(FEEDER_MODS, 5);
        t.set(1, 9);
        assert_eq!(t.get(FEEDER_PLAYER_LIST), Some(3));
        assert_eq!((t[FEEDER_MODS], t[FEEDER_SERVERS]), (5, 0));
        assert_eq!(t.get(1), None, "a feeder that does not scroll keeps no top");
    }

    /// The stock map list: `rect 404 271 192 130` less `origin -405 -135`
    /// inside `createserver_maps` at 404 137, 20 per row.
    fn map_list() -> UiItem {
        UiItem {
            rect: [403.0, 273.0, 192.0, 130.0],
            element_height: 20.0,
            ..UiItem::default()
        }
    }

    #[test]
    fn eleven_maps_scroll_six_rows_one_past_the_last_page() {
        let item = map_list();
        assert_eq!(view_max(&item), 6);
        assert_eq!(max_scroll(&item, 11), 6);
        assert_eq!(max_scroll(&item, 4), 0);
        // Thumb at 273 + 17, travel (130 - 50) / 6 per row.
        assert_eq!(thumb_y(&item, 11, 0), 290.0);
        assert_eq!(thumb_y(&item, 11, 3), 330.0);
    }

    #[test]
    fn the_bar_parts_step_and_page_the_first_row() {
        let item = map_list();
        let bx = 403.0 + 192.0 - 8.0;
        assert_eq!(part_at(&item, 11, 0, [bx, 280.0]), Some(Part::Up));
        assert_eq!(part_at(&item, 11, 0, [bx, 400.0]), Some(Part::Down));
        assert_eq!(part_at(&item, 11, 0, [bx, 295.0]), Some(Part::Thumb));
        assert_eq!(part_at(&item, 11, 3, [bx, 300.0]), Some(Part::PageUp));
        assert_eq!(part_at(&item, 11, 0, [bx, 350.0]), Some(Part::PageDown));
        assert_eq!(part_at(&item, 11, 0, [500.0, 300.0]), None);
        assert_eq!(click(&item, 11, 0, Part::Down), 1);
        assert_eq!(click(&item, 11, 6, Part::Down), 6);
        assert_eq!(click(&item, 11, 0, Part::Up), 0);
        assert_eq!(click(&item, 11, 1, Part::PageDown), 6);
        assert_eq!(click(&item, 11, 5, Part::PageUp), 0);
        assert_eq!(drag(&item, 11, 290.0 + 8.0), 0);
        assert_eq!(drag(&item, 11, 400.0), 6);
    }

    #[test]
    fn a_held_arrow_repeats_after_half_a_second_then_faster() {
        let t0 = Instant::now();
        let mut r = Repeat::new(t0);
        let steps: Vec<u64> = (1..=300)
            .map(|f| f * 10)
            .filter(|&ms| r.due(t0 + Duration::from_millis(ms)))
            .collect();
        // At 510 the gap is 380 (three 40 ms cuts by then), at 900 it is 300.
        assert_eq!(&steps[..3], &[510, 900, 1210]);
        // From 1.8 s on the gap is 20 ms: every third 10 ms frame.
        let tail: Vec<u64> = steps
            .windows(2)
            .rev()
            .take(5)
            .map(|w| w[1] - w[0])
            .collect();
        assert_eq!(tail, vec![30; 5]);
    }

    #[test]
    fn keys_move_the_selection_and_keep_it_in_view() {
        let item = map_list();
        assert_eq!(key(&item, 11, 0, 5, KeyCode::ArrowDown), Some((1, 6)));
        assert_eq!(key(&item, 11, 1, 1, KeyCode::ArrowUp), Some((0, 0)));
        assert_eq!(key(&item, 11, 0, 0, KeyCode::End), Some((5, 10)));
        assert_eq!(key(&item, 11, 5, 10, KeyCode::Home), Some((0, 0)));
        assert_eq!(key(&item, 11, 0, 2, KeyCode::PageDown), Some((3, 8)));
        assert_eq!(key(&item, 11, 0, 0, KeyCode::KeyA), None);
    }
}
