//! The server's open script menu drawn as a keyboard list: a stock
//! `.menu` file (`vcod_common::menu`) turned into localized, cvar-filtered
//! rows and glyph quads.

use std::collections::HashMap;

use vcod_common::localize::Localized;
use vcod_common::menu::Menu;
use vcod_common::pk3::Pk3Fs;

use super::font::{self, Font};
use super::HudQuad;

/// One selectable row: the localized label, the response it sends, and the
/// `execKey` (if any) that answers it directly without moving the selection.
pub struct MenuRow {
    pub label: String,
    pub response: String,
    pub key: Option<String>,
}

/// A menu's live state: the rows a client can pick from and which is
/// highlighted.
pub struct MenuView {
    pub title: String,
    pub background: Option<String>,
    pub rows: Vec<MenuRow>,
    pub selected: usize,
}

/// Builds a [`MenuView`]: rows from `menu.choices(cvar)`, labels through
/// `loc.translate`, each row's `key` matched against `menu.exec_keys` by
/// its response text.
pub fn view(menu: &Menu, loc: &Localized, cvar: impl Fn(&str) -> Option<String>) -> MenuView {
    let rows = menu
        .choices(cvar)
        .into_iter()
        .map(|item| {
            let response = item.response.clone().unwrap_or_default();
            let key = menu
                .exec_keys
                .iter()
                .find(|(_, r)| *r == response)
                .map(|(k, _)| k.clone());
            MenuRow {
                label: loc.translate(&item.text).into_owned(),
                response,
                key,
            }
        })
        .collect();
    MenuView {
        title: menu.name.clone(),
        background: menu.background.clone(),
        rows,
        selected: 0,
    }
}

impl MenuView {
    /// Wraps to the last row from the first.
    pub fn up(&mut self) {
        if self.rows.is_empty() {
            return;
        }
        self.selected = (self.selected + self.rows.len() - 1) % self.rows.len();
    }

    /// Wraps to the first row from the last.
    pub fn down(&mut self) {
        if self.rows.is_empty() {
            return;
        }
        self.selected = (self.selected + 1) % self.rows.len();
    }

    pub fn selected_response(&self) -> Option<&str> {
        self.rows.get(self.selected).map(|r| r.response.as_str())
    }

    pub fn response_for_key(&self, key: &str) -> Option<&str> {
        self.rows
            .iter()
            .find(|r| r.key.as_deref() == Some(key))
            .map(|r| r.response.as_str())
    }
}

const SELECTED_COLOR: [f32; 4] = [1.0, 0.8, 0.2, 1.0];

/// Panel size at the 640x480 reference resolution the stock window was
/// designed at.
const PANEL_W: f32 = 448.0;
const PANEL_H: f32 = 288.0;
const REF_H: f32 = 480.0;

const TITLE_Y: f32 = 24.0;
const ROWS_TOP: f32 = 72.0;
const ROW_GAP: f32 = 28.0;
const ROW_X: f32 = 24.0;

/// A panel centred on the screen: the background image stretched over it
/// when the menu has one, the title, then one text row per choice, the
/// selected row in yellow.
pub fn build(
    view: &MenuView,
    font: &Font,
    scale: f32,
    screen_w: f32,
    screen_h: f32,
    out: &mut Vec<HudQuad>,
) {
    let panel_scale = screen_h / REF_H;
    let w = PANEL_W * panel_scale;
    let h = PANEL_H * panel_scale;
    let x = (screen_w - w) / 2.0;
    let y = (screen_h - h) / 2.0;

    if let Some(bg) = &view.background {
        out.push(HudQuad {
            verts: [[x, y], [x + w, y], [x + w, y + h], [x, y + h]],
            uvs: [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
            rgba: [1.0, 1.0, 1.0, 1.0],
            texture: bg.clone(),
        });
    }

    let text_scale = scale * panel_scale;
    font::layout(
        font,
        &view.title,
        x + ROW_X * panel_scale,
        y + TITLE_Y * panel_scale,
        text_scale,
        font::COLORS[7],
        out,
    );

    for (i, row) in view.rows.iter().enumerate() {
        let color = if i == view.selected {
            SELECTED_COLOR
        } else {
            font::COLORS[7]
        };
        let ry = y + (ROWS_TOP + ROW_GAP * i as f32) * panel_scale;
        font::layout(
            font,
            &row.label,
            x + ROW_X * panel_scale,
            ry,
            text_scale,
            color,
            out,
        );
    }
}

/// One parsed menu per stock name, lazily loaded from
/// `ui_mp/scriptmenus/<name>.menu`; a missing file caches as `None` so a
/// server naming a bad menu is only tried, and warned about, once.
#[derive(Default)]
pub struct MenuCache {
    cache: HashMap<String, Option<Menu>>,
}

impl MenuCache {
    pub fn get(&mut self, fs: &Pk3Fs, name: &str) -> Option<&Menu> {
        if !self.cache.contains_key(name) {
            let path = format!("ui_mp/scriptmenus/{name}.menu");
            let parsed = fs
                .read(&path)
                .map(|bytes| vcod_common::menu::parse(&String::from_utf8_lossy(&bytes)));
            if parsed.is_none() {
                log::warn!("menu: no script menu at {path}");
            }
            self.cache.insert(name.to_string(), parsed);
        }
        self.cache.get(name).unwrap().as_ref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn menu() -> Menu {
        vcod_common::menu::parse(
            r#"{ menuDef { name "weapon_american"
      itemDef { name "a" visible 1 text "@MPMENU_1_M1A1_CARBINE" cvartest "scr_allow_m1carbine" hideCvar { "0" } action { scriptMenuResponse "m1carbine_mp"; } }
      execKey "1" { scriptMenuResponse "m1carbine_mp"; }
      itemDef { name "b" visible 1 text "Garand" action { scriptMenuResponse "m1garand_mp"; } }
    } }"#,
        )
    }

    #[test]
    fn rows_follow_choices_and_keys() {
        let v = view(&menu(), &Localized::default(), |_| None);
        assert_eq!(v.rows.len(), 2);
        assert_eq!(v.rows[0].label, "MPMENU_1_M1A1_CARBINE");
        assert_eq!(v.rows[0].key.as_deref(), Some("1"));
        assert_eq!(v.response_for_key("1"), Some("m1carbine_mp"));
        assert_eq!(v.response_for_key("2"), None);
    }

    #[test]
    fn a_disallowed_weapon_has_no_row() {
        let v = view(&menu(), &Localized::default(), |c| {
            (c == "scr_allow_m1carbine").then(|| "0".into())
        });
        assert_eq!(v.rows.len(), 1);
        assert_eq!(v.rows[0].response, "m1garand_mp");
    }

    #[test]
    fn selection_wraps() {
        let mut v = view(&menu(), &Localized::default(), |_| None);
        v.up();
        assert_eq!(v.selected_response(), Some("m1garand_mp"));
        v.down();
        assert_eq!(v.selected_response(), Some("m1carbine_mp"));
    }
}
