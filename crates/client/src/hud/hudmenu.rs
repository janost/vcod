//! The `hud.menu` items vcod does not draw natively: every visible item of a
//! visible menu that is not an owner draw. Stock `hud.menu` has none; mods
//! add text that reads a cvar the server sets with `v` (a corner banner, a
//! killstreak line) and shader panels
//! (docs/research/cod11-hud-protocol.md, section 9, "Mod items").

use super::HudQuad;
use super::font::UiFonts;
use crate::frontend::Painter;
use vcod_common::localize::Localized;
use vcod_common::pk3::Pk3Fs;
use vcod_common::ui_menu::{
    ITEM_ALIGN_CENTER, ITEM_ALIGN_RIGHT, ITEM_TYPE_OWNERDRAW, UiItem, WINDOW_STYLE_FILLED,
    WINDOW_STYLE_SHADER, parse_file,
};

/// `cg_hudFiles`' default: a `loadMenu { "..." }` list.
const HUD_FILES: &str = "ui_mp/hud.txt";

#[derive(Default)]
pub struct HudMenu {
    items: Vec<UiItem>,
}

impl HudMenu {
    pub fn load(fs: &Pk3Fs) -> HudMenu {
        let read = |path: &str| {
            fs.read(path)
                .map(|b| String::from_utf8_lossy(&b).into_owned())
        };
        let files = read(HUD_FILES).map_or_else(Vec::new, |t| menu_files(&t));
        let files = if files.is_empty() {
            vec!["ui_mp/hud.menu".to_string()]
        } else {
            files
        };
        let items = files
            .iter()
            .filter_map(|f| read(f))
            .flat_map(|text| parse_file(&text, &read))
            .filter(|m| m.visible)
            .flat_map(|m| m.items)
            .filter(|i| i.visible && i.kind != ITEM_TYPE_OWNERDRAW && i.ownerdraw == 0)
            .collect();
        HudMenu { items }
    }

    /// Paints the items on the 640x480 grid; `cvar` reads a client cvar.
    pub fn build(
        &self,
        fonts: &UiFonts,
        screen: (f32, f32),
        loc: &Localized,
        cvar: &dyn Fn(&str) -> Option<String>,
        out: &mut Vec<HudQuad>,
    ) {
        let p = Painter {
            sx: screen.0 / 640.0,
            sy: screen.1 / 480.0,
            fonts: Some(fonts),
        };
        for item in &self.items {
            if !item
                .gate
                .as_ref()
                .is_none_or(|g| g.passes(cvar(&g.cvar).as_deref()))
            {
                continue;
            }
            match (item.style, &item.background) {
                (WINDOW_STYLE_FILLED, Some(bg)) => p.quad(item.rect, item.back, bg, out),
                (WINDOW_STYLE_FILLED, None) => p.fill(item.rect, item.back, out),
                (WINDOW_STYLE_SHADER, Some(bg)) => p.quad(item.rect, item.fore, bg, out),
                _ => {}
            }
            // `Item_Text_Paint`: the text, or the cvar's value when there is none.
            let text = if !item.text.is_empty() {
                loc.translate(&item.text).into_owned()
            } else if let Some(c) = &item.cvar {
                loc.translate(&cvar(c).unwrap_or_default()).into_owned()
            } else {
                continue;
            };
            if text.is_empty() {
                continue;
            }
            let w = p.width(&text, item.text_scale);
            let x = item.rect[0]
                + item.text_align_x
                + match item.text_align {
                    ITEM_ALIGN_RIGHT => -w,
                    ITEM_ALIGN_CENTER => -w / 2.0,
                    _ => 0.0,
                };
            let y = item.rect[1] + item.text_align_y;
            p.text(&text, x, y, item.text_scale, item.fore, out);
        }
    }
}

/// The quoted `.menu` paths of a `loadMenu` list.
fn menu_files(text: &str) -> Vec<String> {
    text.split('"')
        .skip(1)
        .step_by(2)
        .filter(|s| s.to_ascii_lowercase().ends_with(".menu"))
        .map(str::to_string)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Shaped like a mod's addition to `hud.menu`: two stock owner draws, a
    /// banner fed by a server-set cvar, a static label and a hidden menu.
    const HUD: &str = r#"
#define MENU_TRUE 1
{
  menuDef { name "Health" visible MENU_TRUE rect 0 0 640 480
    itemDef { name "healthbar" visible 1 rect 10 10 64 8 background "gfx/hud/hud@health_bar.tga" ownerdraw 21 }
  }
  menuDef { name "mod_corner" visible MENU_TRUE rect 0 0 640 480
    itemDef { name "banner" type 1 visible MENU_TRUE rect 5 3 480 14 cvar "scr_mod_banner" decoration textscale 0.22 textaligny 10 }
    itemDef { name "label" visible 1 rect 5 20 100 14 text "Hello" textscale 0.22 }
    itemDef { name "gated" visible 1 rect 5 40 100 14 text "Gated" cvartest "scr_mod_on" showCvar { "1" } }
    itemDef { name "off" visible 0 rect 5 60 100 14 text "Off" }
  }
  menuDef { name "hidden" visible 0 rect 0 0 640 480
    itemDef { name "x" visible 1 rect 0 0 10 10 text "Hidden" }
  }
}
"#;

    #[test]
    fn keeps_the_visible_items_that_are_not_owner_draws() {
        let mut fs = Pk3Fs::empty();
        fs.overlay("ui_mp/hud.menu", HUD.as_bytes().to_vec());
        let h = HudMenu::load(&fs);
        let names: Vec<_> = h.items.iter().map(|i| i.name.as_str()).collect();
        assert_eq!(names, ["banner", "label", "gated"]);
    }

    #[test]
    fn hud_txt_names_the_files() {
        assert_eq!(
            menu_files("{\n\tloadMenu { \"ui_mp/hud.menu\" \"ui_mp/mod.MENU\" }\n}"),
            ["ui_mp/hud.menu", "ui_mp/mod.MENU"]
        );
    }

    #[test]
    fn stock_hud_menu_adds_nothing() {
        let Some(fs) = vcod_common::testing::game_fs() else {
            return;
        };
        assert!(HudMenu::load(&fs).items.is_empty());
    }

    #[test]
    fn draws_a_cvar_banner_once_the_server_sets_it() {
        let Some(fs) = vcod_common::testing::game_fs() else {
            return;
        };
        let fonts = UiFonts::load(&fs).unwrap();
        let mut over = Pk3Fs::empty();
        over.overlay("ui_mp/hud.menu", HUD.as_bytes().to_vec());
        let h = HudMenu::load(&over);
        let loc = Localized::default();
        let quads = |cvar: &dyn Fn(&str) -> Option<String>| {
            let mut out = Vec::new();
            h.build(&fonts, (640.0, 480.0), &loc, cvar, &mut out);
            out.len()
        };
        let label = quads(&|_| None);
        assert!(label > 0, "the static label draws");
        let banner = quads(&|c| (c == "scr_mod_banner").then(|| "King".to_string()));
        assert_eq!(banner, label + 2 * 4, "four glyphs, each with its shadow");
        let gated = quads(&|c| (c == "scr_mod_on").then(|| "1".to_string()));
        assert_eq!(gated, label + 2 * 5);
    }
}
