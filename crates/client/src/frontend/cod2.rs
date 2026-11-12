//! CoD2's menus on the real screen. A CoD2 `rect` carries a horizontal and
//! a vertical alignment, and every draw goes through `CalcScreenPlacement`
//! (Mac 0x18ed96) with the item's pair, or the menu's when the item gives
//! neither (`Item_SetScreenCoords`, 0x192612). The front end lays out, paints
//! and hit-tests on the 640x480 grid of the window's centred 4:3 column,
//! CoD2's `SUBLEFT`/`SUBTOP` frame, so each rect is placed once per window
//! size and mapped back onto that grid ([`Cod2Rects::place`]); the rest of
//! the front end runs unchanged. docs/research/cod2-fonts-menus.md, 2 and 3.
//!
//! A CoD2 session drives a server's script menus through the same calls as
//! CoD1's (`App::cod2_script_menus`): its precached names into
//! [`super::Ui::load_script_menus`] at each gamestate, a `t <index>` into
//! [`super::Ui::open_script_popup`] with that name (a refusal waits and is
//! offered again next frame), a `u` into [`super::Ui::close_script_popup`],
//! a `K` into [`super::Ui::close_in_game_menu`] ([`menu_command`]), and each [`super::UiEffect::ScriptMenuResponse`] the UI hands back sent
//! as the session's `mr`. Esc over the game opens the menu
//! `g_scriptMainMenu` names ([`escape`]), `ingame` on the stock gametypes.

use vcod_cod2::net::protocol::CS_LAYOUT_118;
use vcod_common::ui_menu::{Rect, UiMenu};

use super::{Ui, UiEffect};
use crate::console::shell::Shell;
use crate::hud::cod2elem::ScreenPlacement;
use crate::hud::hudelem::Virtual;
use crate::play::join::{Join, MenuCommand, Waiting};

/// Esc over a CoD2 game with no menu up (`UI_SetActiveMenu` 2, Mac
/// 0x15118e): `main`, the menu `g_scriptMainMenu` names once the server
/// set it and it is loaded, else the main menu over the game (Back to
/// Game, Disconnect, Options).
pub fn escape(ui: &mut Ui, main: Option<&str>, shell: &Shell, out: &mut Vec<UiEffect>) {
    match main.filter(|n| ui.has_menu(n)) {
        Some(name) => ui.open_script_main(name, shell, out),
        None => ui.open_ingame(shell, out),
    }
}

/// The script menus a CoD2 server precached, configstrings 1246..1277
/// (`GScr_PrecacheMenu`, docs/research/cod2-protocol.md 6).
pub fn script_menu_names(configstrings: &[String]) -> Vec<String> {
    crate::play::join::script_menu_names(configstrings, &CS_LAYOUT_118)
}

/// A `t`, `u` or `K` the CoD2 session left to the player's UI. A `t` the UI
/// refuses waits in `join` (`CG_OpenScriptMenu`, Mac 0x1e216c) for
/// [`offer_waiting`]; a refusal that displaces another waiting menu
/// returns the `mr <index> noop` to send, as does a `t` [`Join`] answers.
/// `K` closes the menu Esc opened ([`Ui::close_in_game_menu`]); the stock
/// `ingame` buttons send their response without closing it, and the
/// server's `closeInGameMenu` ahead of the next `t` lets that one in.
pub fn menu_command(
    ui: &mut Ui,
    join: &mut Join,
    tokens: &[String],
    configstrings: &[String],
    server_id: i32,
    shell: &Shell,
    out: &mut Vec<UiEffect>,
) -> Option<String> {
    if tokens.first().map(String::as_str) == Some("K") {
        out.extend(ui.close_in_game_menu(shell));
        return None;
    }
    match join.on_server_command(tokens, configstrings, server_id)? {
        MenuCommand::Send(cmd) => Some(cmd),
        MenuCommand::Popup {
            name,
            index,
            no_mouse,
        } => {
            ui.set_server_cvars(join.cvars.all(configstrings, shell));
            if ui.open_script_popup(&name, no_mouse, shell, out) {
                return None;
            }
            log::info!("cod2: script menu {name} waits while another is up");
            join.wait(Waiting {
                name,
                index,
                no_mouse,
            })
        }
        MenuCommand::Close => {
            out.extend(ui.close_script_popup(shell));
            None
        }
    }
}

/// `CG_CheckOpenWaitingScriptMenu` (Mac 0x1e2314), once a frame: the
/// waiting `t` is offered to the UI again.
pub fn offer_waiting(
    ui: &mut Ui,
    join: &mut Join,
    configstrings: &[String],
    shell: &Shell,
    out: &mut Vec<UiEffect>,
) {
    let Some(w) = join.waiting() else {
        return;
    };
    ui.set_server_cvars(join.cvars.all(configstrings, shell));
    if ui.open_script_popup(&w.name, w.no_mouse, shell, out) {
        join.opened_waiting();
    }
}

/// `mr <serverId> <index> <response>` for a script menu's
/// `scriptMenuResponse`: the index of `menu`'s name from configstring 1246,
/// -1 when none holds it. The session's own is
/// [`crate::play::cod2::Cod2Session::menu_response`]; this one is the tests'.
#[cfg(test)]
pub fn script_menu_response(
    menu: &str,
    response: &str,
    configstrings: &[String],
    server_id: i32,
) -> String {
    crate::play::join::script_menu_response(
        menu,
        response,
        configstrings,
        &CS_LAYOUT_118,
        server_id,
    )
}

/// Clicks the topmost shown item whose text or name is `what` on a `w` x
/// `h` window, then runs the `set` lines it queued and a UI frame, as the
/// app does. Tests and the offscreen menu shot walk the menus with it.
#[cfg(test)]
pub(crate) fn click_shown(
    ui: &mut Ui,
    shell: &mut Shell,
    what: &str,
    (w, h): (f32, f32),
) -> Vec<UiEffect> {
    let found = ui.open.iter().rev().find_map(|&m| {
        (0..ui.menus[m].items.len())
            .find(|&i| {
                let item = &ui.menus[m].items[i];
                (item.text == what || item.name == what) && ui.shown(m, i, shell)
            })
            .map(|i| (m, i))
    });
    let open: Vec<&str> = ui.open.iter().map(|&m| ui.menus[m].name.as_str()).collect();
    let (m, i) = found.unwrap_or_else(|| panic!("no {what} in {open:?}"));
    ui.set_screen(w, h);
    let r = ui.menus[m].items[i].rect;
    let [px, py] = grid(w, h).point(r[0] + 2.0, r[1] + 2.0);
    ui.mouse_move(px, py, w, h, shell);
    let out = ui.click(std::time::Instant::now(), shell);
    for e in &out {
        if let UiEffect::Command(c) = e {
            shell.execute(c);
        }
    }
    ui.frame(std::time::Instant::now(), shell);
    out
}

/// The rects as parsed, per menu (the menu's, then each item's), and the
/// window size the menus' rects were last placed for.
#[derive(Default)]
pub(super) struct Cod2Rects {
    parsed: Vec<(Rect, Vec<Rect>)>,
    placed_for: Option<(f32, f32)>,
}

impl Cod2Rects {
    /// Keeps the parsed rects of the first `keep` menus and takes the rest
    /// from `menus`, which must not have been placed yet. Places again on
    /// the next [`Self::place`].
    pub(super) fn sync(&mut self, menus: &[UiMenu], keep: usize) {
        self.parsed.truncate(keep);
        self.parsed.extend(
            menus[self.parsed.len()..]
                .iter()
                .map(|m| (m.rect, m.items.iter().map(|i| i.rect).collect())),
        );
        self.placed_for = None;
    }

    /// Rewrites every menu's and item's rect as placed on a `w` x `h`
    /// window, in grid units of the window's 4:3 column. False when the
    /// rects already were.
    pub(super) fn place(&mut self, menus: &mut [UiMenu], w: f32, h: f32) -> bool {
        if self.placed_for == Some((w, h)) {
            return false;
        }
        self.placed_for = Some((w, h));
        let sp = ScreenPlacement::new(w, h);
        let v = grid(w, h);
        for (menu, (rect, items)) in menus.iter_mut().zip(&self.parsed) {
            menu.rect = on_grid(&sp, &v, *rect, menu.align);
            for (item, rect) in menu.items.iter_mut().zip(items) {
                let align = if item.align == [0, 0] {
                    menu.align
                } else {
                    item.align
                };
                item.rect = on_grid(&sp, &v, *rect, align);
            }
        }
        true
    }
}

/// The 640x480 grid CoD2's menus paint on: the window's centred 4:3
/// column, nothing stretched to the window's edges.
pub(super) fn grid(w: f32, h: f32) -> Virtual {
    Virtual::new((w, h)).boxed()
}

/// `rect` placed by `CalcScreenPlacement` under `align` (horizontal,
/// vertical), then read back on `v`'s grid.
fn on_grid(sp: &ScreenPlacement, v: &Virtual, rect: Rect, align: [i32; 2]) -> Rect {
    let [x, y, w, h] = rect;
    let (px, pw) = sp.axis(0, x, w, align[0]);
    let (py, ph) = sp.axis(1, y, h, align[1]);
    [
        (px - v.x0) / v.scale,
        (py - v.y0) / v.scale,
        pw / v.scale,
        ph / v.scale,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    const LEFT: i32 = 1;
    const CENTER: i32 = 2;
    const RIGHT: i32 = 3;
    const FULLSCREEN: i32 = 4;

    fn menu(align: [i32; 2], items: &[(Rect, [i32; 2])]) -> UiMenu {
        UiMenu {
            rect: [0.0, 0.0, 640.0, 480.0],
            align,
            items: items
                .iter()
                .map(|&(rect, align)| vcod_common::ui_menu::UiItem {
                    rect,
                    align,
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        }
    }

    /// On a 4:3 window `SUBLEFT` and `LEFT` agree; `CENTER` and `RIGHT`
    /// move a rect by half and all of the screen; an item with no pair
    /// takes its menu's.
    #[test]
    fn a_rect_is_placed_by_its_alignment_or_its_menus() {
        let mut menus = vec![menu(
            [0, 0],
            &[
                ([10.0, 20.0, 30.0, 40.0], [0, 0]),
                ([10.0, 20.0, 30.0, 40.0], [LEFT, 1]),
                ([-10.0, -20.0, 30.0, 40.0], [CENTER, CENTER]),
                ([-10.0, -10.0, 0.0, 0.0], [RIGHT, 3]),
            ],
        )];
        menus.push(menu([RIGHT, 3], &[([-50.0, -50.0, 10.0, 10.0], [0, 0])]));
        let mut r = Cod2Rects::default();
        r.sync(&menus, 0);
        assert!(r.place(&mut menus, 1280.0, 960.0));
        assert!(!r.place(&mut menus, 1280.0, 960.0), "placed once a size");
        let rects: Vec<Rect> = menus[0].items.iter().map(|i| i.rect).collect();
        assert_eq!(rects[0], [10.0, 20.0, 30.0, 40.0]);
        assert_eq!(rects[1], [10.0, 20.0, 30.0, 40.0]);
        assert_eq!(rects[2], [310.0, 220.0, 30.0, 40.0]);
        assert_eq!(rects[3], [630.0, 470.0, 0.0, 0.0]);
        assert_eq!(menus[1].items[0].rect, [590.0, 430.0, 10.0, 10.0]);
    }

    /// On a 16:9 window the 4:3 column sits in the middle: `SUBLEFT` stays
    /// in it, `LEFT` and `RIGHT` reach the window's edges and `FULLSCREEN`
    /// stretches over the whole window.
    #[test]
    fn a_wide_window_moves_the_edge_alignments_out_of_the_column() {
        let mut menus = vec![menu(
            [0, 0],
            &[
                ([0.0, 0.0, 640.0, 64.0], [0, 0]),
                ([0.0, 0.0, 10.0, 10.0], [LEFT, 1]),
                ([-10.0, 0.0, 10.0, 10.0], [RIGHT, 1]),
                ([0.0, 0.0, 640.0, 64.0], [FULLSCREEN, FULLSCREEN]),
            ],
        )];
        let mut r = Cod2Rects::default();
        r.sync(&menus, 0);
        // 1920x1080: the column is 1440 wide from x 240, a grid unit 2.25 px.
        r.place(&mut menus, 1920.0, 1080.0);
        let rects: Vec<Rect> = menus[0].items.iter().map(|i| i.rect).collect();
        let side = 240.0 / 2.25;
        assert_eq!(rects[0], [0.0, 0.0, 640.0, 64.0]);
        assert_eq!(rects[1][0], -side);
        assert_eq!(rects[2][0], 630.0 + side);
        assert_eq!(rects[3], [-side, 0.0, 640.0 + 2.0 * side, 64.0]);
        // A new size places again off the parsed rects.
        r.place(&mut menus, 640.0, 480.0);
        assert_eq!(menus[0].items[3].rect, [0.0, 0.0, 640.0, 64.0]);
    }

    /// A reload keeps the first menus' parsed rects and takes the new ones'.
    #[test]
    fn sync_keeps_the_placed_menus_parse() {
        let mut menus = vec![menu([0, 0], &[([-10.0, 0.0, 5.0, 5.0], [RIGHT, 1])])];
        let mut r = Cod2Rects::default();
        r.sync(&menus, 0);
        r.place(&mut menus, 640.0, 480.0);
        menus.push(menu([0, 0], &[([-10.0, 0.0, 5.0, 5.0], [CENTER, 1])]));
        r.sync(&menus, 1);
        r.place(&mut menus, 640.0, 480.0);
        assert_eq!(menus[0].items[0].rect[0], 630.0);
        assert_eq!(menus[1].items[0].rect[0], 310.0);
    }

    use super::super::{Painter, Ui, UiEffect, paints_text};
    use crate::console::shell::Shell;
    use vcod_common::localize::Localized;
    use vcod_common::pk3::Pk3Fs;
    use vcod_common::ui_menu::WINDOW_STYLE_SHADER;

    /// The front end off a CoD2 install with every stock script menu
    /// loaded after the lists' menus, as a server's precache would.
    fn stock_ui(fs: &Pk3Fs) -> Ui {
        let mut ui = Ui::new(fs, "");
        let names: Vec<String> = fs
            .list_prefix("ui_mp/scriptmenus/")
            .iter()
            .filter_map(|p| p.strip_prefix("ui_mp/scriptmenus/")?.strip_suffix(".menu"))
            .map(str::to_string)
            .collect();
        assert!(names.len() > 10, "{names:?}");
        assert_eq!(ui.load_script_menus(fs, &names, ""), Vec::<String>::new());
        ui
    }

    /// Runs the `set` lines a menu script queued, as the app does.
    fn apply(shell: &mut Shell, out: Vec<UiEffect>) {
        for e in out {
            if let UiEffect::Command(c) = e {
                shell.execute(&c);
            }
        }
    }

    /// Every stock MP menu, the script menus included, on a 4:3 and a 16:9
    /// window: each rect placed and finite, each text item's font found
    /// and measuring its text, each shader item's material in the library,
    /// and each menu, opened alone, painting finite quads.
    #[test]
    fn every_stock_mp_menu_lays_out() {
        let Some(fs) = vcod_common::testing::cod2_fs() else {
            return;
        };
        let mut ui = stock_ui(&fs);
        let lib = vcod_cod2::materials::load_lib(&fs).unwrap();
        let loc = Localized::load(&fs);
        let shell = Shell::new();
        assert!(ui.fonts.is_some(), "CoD2's fonts load");
        assert!(ui.menus.len() > 60, "{} menus", ui.menus.len());
        let mut missing = std::collections::BTreeSet::new();
        for (w, h) in [(640.0, 480.0), (1920.0, 1080.0)] {
            ui.set_screen(w, h);
            let p = Painter {
                v: grid(w, h),
                fonts: ui.fonts.as_ref(),
                exact: true,
            };
            for m in 0..ui.menus.len() {
                let menu = &ui.menus[m];
                assert!(menu.rect.iter().all(|v| v.is_finite()), "{}", menu.name);
                for item in &menu.items {
                    let at = format!("{} / {}", menu.name, item.name);
                    assert!(item.rect.iter().all(|v| v.is_finite()), "{at}");
                    if paints_text(item) && !item.text.is_empty() {
                        let text = loc.translate(&item.text);
                        assert!(p.font(item.text_scale).is_some(), "{at}");
                        assert!(p.width(&text, item.text_scale) > 0.0, "{at}: {text:?}");
                    }
                    if item.style == WINDOW_STYLE_SHADER
                        && let Some(bg) = &item.background
                        && lib.get(bg).is_none()
                    {
                        missing.insert(bg.clone());
                    }
                }
                ui.open = vec![m];
                let quads = ui.build(w, h, &loc, &shell);
                let bad = quads
                    .iter()
                    .find(|q| q.verts.iter().flatten().any(|v| !v.is_finite()));
                assert!(
                    bad.is_none(),
                    "{}: {:?}",
                    ui.menus[m].name,
                    bad.map(|q| &q.verts)
                );
            }
        }
        assert!(missing.is_empty(), "no material for {missing:?}");
    }

    /// The stock main menu as bare `vcod --game cod2` opens it: `main`
    /// opens `bg` and `main_text`, whose `onOpen` names the backdrop, and
    /// the paint draws the backdrop, the logo, the bars and the buttons'
    /// text, each through a material the library holds.
    #[test]
    fn the_main_menu_opens_its_backdrop_and_buttons() {
        let Some(fs) = vcod_common::testing::cod2_fs() else {
            return;
        };
        let mut ui = Ui::new(&fs, "");
        let mut shell = Shell::new();
        let mut out = Vec::new();
        ui.open_main(&shell, &mut out);
        apply(&mut shell, out);
        let open: Vec<&str> = ui.open.iter().map(|&m| ui.menus[m].name.as_str()).collect();
        assert_eq!(open, ["main", "bg", "main_text"]);
        // The browser asks cod.pm's CoD2 list for protocol 118 alone.
        let q = ui.browser.master_query.unwrap();
        assert_eq!(
            (q.master, q.protocol),
            (vcod_common::net::master::COD2_MASTER, 118)
        );
        ui.set_screen(1280.0, 960.0);
        let loc = Localized::load(&fs);
        let quads = ui.build(1280.0, 960.0, &loc, &shell);
        let lib = vcod_cod2::materials::load_lib(&fs).unwrap();
        let mut textures: Vec<&str> = quads.iter().map(|q| q.texture.as_str()).collect();
        textures.dedup();
        for t in &textures {
            let name = t.strip_prefix("$default:").unwrap_or(t);
            assert!(name == "white" || lib.get(name).is_some(), "{t}");
        }
        assert!(textures.contains(&"background_american_w"), "{textures:?}");
        assert!(textures.contains(&"fonts/gamefonts"), "{textures:?}");
        // The top bar spans the window; the backdrop overhangs the 4:3
        // column by 128 grid units a side, here 256 px.
        let bar = quads
            .iter()
            .find(|q| q.texture == "white" && q.verts[0][1] == 0.0)
            .unwrap();
        assert_eq!(bar.verts[0], [0.0, 0.0]);
        assert_eq!(bar.verts[2][0], 1280.0);
        let back = quads
            .iter()
            .find(|q| q.texture == "background_american_w")
            .unwrap();
        assert_eq!((back.verts[0][0], back.verts[1][0]), (-256.0, 1536.0));
    }

    /// A server's team menu: a click on its auto-assign button answers
    /// `scriptMenuResponse` with the menu's own name, what a CoD2 session
    /// sends as `mr <serverId> <index> autoassign` ([`mr_command`]).
    #[test]
    fn a_script_menu_click_answers_its_response() {
        let Some(fs) = vcod_common::testing::cod2_fs() else {
            return;
        };
        let mut ui = stock_ui(&fs);
        let shell = Shell::new();
        // The button shows only while the server allows auto-assign.
        ui.set_server_cvars([("ui_allow_joinauto".to_string(), "1".to_string())].into());
        let mut out = Vec::new();
        assert!(ui.open_script_popup("team_britishgerman", false, &shell, &mut out));
        let m = ui.find("team_britishgerman").unwrap();
        let (w, h) = (1280.0, 960.0);
        ui.set_screen(w, h);
        let i = ui.menus[m]
            .items
            .iter()
            .position(|i| {
                ui_menu_script_has(&i.action, "autoassign")
                    && i.kind == super::super::ITEM_TYPE_BUTTON
            })
            .expect("an auto-assign button");
        let [x, y, _, _] = ui.menus[m].items[i].rect;
        let v = grid(w, h);
        let [px, py] = v.point(x + 2.0, y + 2.0);
        ui.mouse_move(px, py, w, h, &shell);
        let out = ui.click(std::time::Instant::now(), &shell);
        let resp = out.iter().find_map(|e| match e {
            UiEffect::ScriptMenuResponse { menu, response } => {
                Some((menu.as_str(), response.as_str()))
            }
            _ => None,
        });
        assert_eq!(resp, Some(("team_britishgerman", "autoassign")), "{out:?}");
    }

    /// The join as retail's capture has it (cod2-protocol.md 7): the
    /// server precaches its menus, `t 1` opens `team_britishgerman` and the
    /// auto-assign button answers `mr 16 1 autoassign`.
    #[test]
    fn a_server_named_team_menu_answers_as_retail() {
        let Some(fs) = vcod_common::testing::cod2_fs() else {
            return;
        };
        let mut cs = vec![String::new(); 1278];
        for (i, name) in ["serverinfo_tdm", "team_britishgerman", "weapon_british"]
            .iter()
            .enumerate()
        {
            cs[1246 + i] = name.to_string();
        }
        let names = script_menu_names(&cs);
        assert_eq!(
            names,
            ["serverinfo_tdm", "team_britishgerman", "weapon_british"]
        );
        let mut ui = Ui::new(&fs, "");
        assert_eq!(ui.load_script_menus(&fs, &names, ""), Vec::<String>::new());
        ui.set_server_cvars([("ui_allow_joinauto".to_string(), "1".to_string())].into());
        let shell = Shell::new();
        let mut out = Vec::new();
        assert!(ui.open_script_popup(&names[1], false, &shell, &mut out));
        // Its `onOpen` answers `open`.
        let sent: Vec<String> = responses(&out, &cs);
        assert_eq!(sent, ["mr 16 1 open"]);
        let (w, h) = (1920.0, 1080.0);
        ui.set_screen(w, h);
        let m = ui.find(&names[1]).unwrap();
        let item = ui.menus[m]
            .items
            .iter()
            .find(|i| i.name == "button_autoassign")
            .unwrap();
        let [px, py] = grid(w, h).point(item.rect[0] + 2.0, item.rect[1] + 2.0);
        ui.mouse_move(px, py, w, h, &shell);
        let out = ui.click(std::time::Instant::now(), &shell);
        assert_eq!(responses(&out, &cs), ["mr 16 1 autoassign"]);
    }

    /// `openscriptmenu ingame changeweapon` (X), then the weapon menu the
    /// server opens: its execKey 6 picks the Thompson, the 1 of the
    /// in-game menu asks for the weapon menu, as a player keys them.
    #[test]
    fn the_ingame_and_weapon_menus_answer_their_keys() {
        let Some(fs) = vcod_common::testing::cod2_fs() else {
            return;
        };
        let mut cs = vec![String::new(); 1278];
        for (i, name) in ["ingame", "team_britishgerman", "weapon_british"]
            .iter()
            .enumerate()
        {
            cs[1246 + i] = name.to_string();
        }
        let names = script_menu_names(&cs);
        let mut ui = Ui::new(&fs, "");
        assert_eq!(ui.load_script_menus(&fs, &names, ""), Vec::<String>::new());
        let allow = [
            "sten",
            "enfield",
            "m1garand",
            "enfieldsniper",
            "shotgun",
            "thompson",
            "bren",
        ];
        ui.set_server_cvars(
            allow
                .iter()
                .map(|w| (format!("ui_allow_{w}"), "1".to_string()))
                .collect(),
        );
        let shell = Shell::new();
        let mut out = Vec::new();
        ui.open_script_main("ingame", &shell, &mut out);
        let (_, out) = ui.key(winit::keyboard::KeyCode::Digit1, &shell);
        assert_eq!(responses(&out, &cs), ["mr 16 0 changeweapon"]);
        ui.close_all();
        let mut out = Vec::new();
        assert!(ui.open_script_popup("weapon_british", false, &shell, &mut out));
        assert!(ui.wants_key(winit::keyboard::KeyCode::Digit6));
        let (_, out) = ui.key(winit::keyboard::KeyCode::Digit6, &shell);
        assert_eq!(responses(&out, &cs), ["mr 16 2 thompson_mp"]);
    }

    /// Esc's `ingame` menu off the stock iwds, each button clicked: the
    /// click sends `mr 16 0 <response>` and leaves `ingame` up; the server
    /// answers `u`, `K`, `t <index>`, the same on retail and vcod-server
    /// (`VCOD_PROBE_INGAME` captures, cod2-protocol.md "Script menus on a
    /// CoD2 client"), and the `t`'s menu opens alone. A weapon pick sends
    /// its `mr` and the `u`, `K` after it close the menu; Esc closes the
    /// mute menu. Without the `K` the `t` waits and opens once `ingame`
    /// goes.
    #[test]
    fn every_ingame_button_opens_the_menu_the_server_answers() {
        use crate::play::join::Join;
        let Some(fs) = vcod_common::testing::cod2_fs() else {
            return;
        };
        let mut cs = vec![String::new(); 1278];
        // vcod-server's and retail's precache order on mp_toujane dm.
        let precached = [
            "ingame",
            "team_britishgerman",
            "weapon_british",
            "weapon_german",
            "serverinfo_dm",
            "callvote",
            "muteplayer",
        ];
        for (i, name) in precached.iter().enumerate() {
            cs[1246 + i] = name.to_string();
        }
        let names = script_menu_names(&cs);
        let mut ui = Ui::new(&fs, "");
        assert_eq!(ui.load_script_menus(&fs, &names, ""), Vec::<String>::new());
        let mut shell = Shell::new();
        // The `v`s both servers send once the player is on a team.
        for w in [
            "weaponchange",
            "sten",
            "enfield",
            "m1garand",
            "thompson",
            "bren",
        ] {
            shell.execute(&format!("set ui_allow_{w} 1"));
        }
        // The Call Vote button shows only under it.
        shell.execute("set ui_allowvote 1");
        let mut join = Join::new(None, None);
        join.set_layout(CS_LAYOUT_118);
        let open = |ui: &Ui| -> Vec<String> {
            ui.open.iter().map(|&m| ui.menus[m].name.clone()).collect()
        };
        let feed = |ui: &mut Ui, join: &mut Join, shell: &Shell, line: &str| {
            let tokens: Vec<String> = line.split(' ').map(str::to_string).collect();
            let mut out = Vec::new();
            let sent = menu_command(ui, join, &tokens, &cs, 16, shell, &mut out);
            assert_eq!(sent, None, "{line}");
            offer_waiting(ui, join, &cs, shell, &mut out);
            out
        };
        for (button, response, index, menu) in [
            ("button_changeweapon", "changeweapon", 2, "weapon_british"),
            ("button_changeteam", "changeteam", 1, "team_britishgerman"),
            ("button_muteplayer", "muteplayer", 6, "muteplayer"),
            ("button_callvote", "callvote", 5, "callvote"),
        ] {
            let mut out = Vec::new();
            escape(&mut ui, Some("ingame"), &shell, &mut out);
            apply(&mut shell, out);
            assert_eq!(open(&ui), ["ingame"]);
            let out = click_shown(&mut ui, &mut shell, button, (640.0, 480.0));
            assert_eq!(responses(&out, &cs), [format!("mr 16 0 {response}")]);
            assert_eq!(open(&ui), ["ingame"], "{button}: the click leaves it up");
            for line in ["u", "K", &format!("t {index}")] {
                let out = feed(&mut ui, &mut join, &shell, line);
                apply(&mut shell, out);
            }
            assert_eq!(open(&ui), [menu], "{button}");
            assert!(ui.active(), "{button}: the menu holds the mouse");
            if menu == "weapon_british" {
                let out = click_shown(&mut ui, &mut shell, "button_thompson", (640.0, 480.0));
                assert_eq!(responses(&out, &cs), ["mr 16 2 thompson_mp"]);
                for line in ["u", "K"] {
                    let out = feed(&mut ui, &mut join, &shell, line);
                    apply(&mut shell, out);
                }
            } else {
                let (_, out) = ui.key(winit::keyboard::KeyCode::Escape, &shell);
                apply(&mut shell, out);
            }
            assert_eq!(open(&ui), Vec::<String>::new(), "{button}: closed");
            assert!(!ui.active(), "{button}: the game has the mouse back");
        }
        // A `t` with no `K` ahead of it is refused while `ingame` holds the
        // focus, waits, and opens once `ingame` closes.
        let mut out = Vec::new();
        escape(&mut ui, Some("ingame"), &shell, &mut out);
        let out = feed(&mut ui, &mut join, &shell, "t 6");
        apply(&mut shell, out);
        assert_eq!(open(&ui), ["ingame"]);
        let (_, out) = ui.key(winit::keyboard::KeyCode::Escape, &shell);
        apply(&mut shell, out);
        let mut out = Vec::new();
        offer_waiting(&mut ui, &mut join, &cs, &shell, &mut out);
        assert_eq!(open(&ui), ["muteplayer"]);
        assert!(join.waiting().is_none());
    }

    /// The `muteplayer` script menu's list (`FEEDER_MUTELIST`): the rows
    /// the session offers, a key moves the selection, and the toggle
    /// button's `uiScript mutePlayer` names the selected row's client
    /// (cod2-voice.md 5).
    #[test]
    fn the_mute_list_toggles_the_selected_client() {
        use crate::play::join::Join;
        let Some(fs) = vcod_common::testing::cod2_fs() else {
            return;
        };
        let mut cs = vec![String::new(); 1278];
        cs[1246] = "ingame".into();
        cs[1252] = "muteplayer".into();
        let names = script_menu_names(&cs);
        let mut ui = Ui::new(&fs, "");
        assert_eq!(ui.load_script_menus(&fs, &names, ""), Vec::<String>::new());
        let mut shell = Shell::new();
        let mut join = Join::new(None, None);
        join.set_layout(CS_LAYOUT_118);
        let mut out = Vec::new();
        escape(&mut ui, Some("ingame"), &shell, &mut out);
        apply(&mut shell, out);
        for line in ["u", "K", "t 6"] {
            let tokens: Vec<String> = line.split(' ').map(str::to_string).collect();
            let mut out = Vec::new();
            assert_eq!(
                menu_command(&mut ui, &mut join, &tokens, &cs, 16, &shell, &mut out),
                None
            );
            offer_waiting(&mut ui, &mut join, &cs, &shell, &mut out);
            apply(&mut shell, out);
        }
        let open: Vec<String> = ui.open.iter().map(|&m| ui.menus[m].name.clone()).collect();
        assert_eq!(open, ["muteplayer"]);
        ui.offer_clients(std::time::Instant::now(), || {
            vec![(2, "^1Able".into()), (5, "Baker".into())]
        });
        ui.set_muted(1 << 5);
        let out = click_shown(&mut ui, &mut shell, "playermutelist", (640.0, 480.0));
        apply(&mut shell, out);
        assert_eq!(ui.selected_client(), Some(2));
        let (_, out) = ui.key(winit::keyboard::KeyCode::ArrowDown, &shell);
        apply(&mut shell, out);
        assert_eq!(ui.selected_client(), Some(5));
        let out = click_shown(&mut ui, &mut shell, "playermutetoggle", (640.0, 480.0));
        assert!(
            out.iter().any(|e| matches!(e, UiEffect::MutePlayer(5))),
            "{out:?}"
        );
        let quads = ui.build(
            640.0,
            480.0,
            &vcod_common::localize::Localized::load(&fs),
            &shell,
        );
        assert!(!quads.is_empty());
    }

    fn responses(out: &[UiEffect], cs: &[String]) -> Vec<String> {
        out.iter()
            .filter_map(|e| match e {
                UiEffect::ScriptMenuResponse { menu, response } => {
                    Some(script_menu_response(menu, response, cs, 16))
                }
                _ => None,
            })
            .collect()
    }

    fn ui_menu_script_has(script: &[String], word: &str) -> bool {
        script.iter().any(|t| t.eq_ignore_ascii_case(word))
    }
}

/// The CoD2 front end walked as a player clicks it, on a 640x480 window
/// where the grid is the window.
#[cfg(test)]
mod flow {
    use super::super::*;
    use crate::console::shell::Shell;
    use vcod_common::localize::Localized;

    fn names(ui: &Ui) -> Vec<String> {
        ui.open.iter().map(|&m| ui.menus[m].name.clone()).collect()
    }

    fn ui() -> Option<(Ui, Shell)> {
        let fs = vcod_common::testing::cod2_fs()?;
        let mut ui = Ui::new(&fs, "");
        ui.browser.lan_targets.clear();
        ui.set_screen(640.0, 480.0);
        Some((ui, Shell::new()))
    }

    fn click(ui: &mut Ui, shell: &mut Shell, what: &str) -> Vec<UiEffect> {
        super::click_shown(ui, shell, what, (640.0, 480.0))
    }

    /// Join Game opens the browser with CoD2's eleven columns; a server's
    /// row shows its `hw` icon out of CoD2's materials and `X` for voice,
    /// pure, mod and PunkBuster, as `UI_FeederItemText` does.
    #[test]
    fn the_browser_lists_cod2s_eleven_columns() {
        let Some((mut ui, mut shell)) = ui() else {
            return;
        };
        ui.open_main(&shell, &mut Vec::new());
        click(&mut ui, &mut shell, "@MENU_JOIN_GAME");
        assert_eq!(names(&ui), ["main", "bg", "joinserver"]);
        assert_eq!(ui.browser.columns, &browser::COLUMNS_V118[..]);
        let mut s = browser::Server::new("127.0.0.1:28960".parse().unwrap());
        s.info = Some(vcod_common::net::master::ServerInfo::parse(
            "\\challenge\\xyz\\protocol\\118\\hostname\\CoD2Host\\mapname\\mp_toujane\\sv_maxclients\\8\\gametype\\dm\\pure\\1\\kc\\1\\hw\\1\\mod\\0\\voice\\1\\pb\\0",
        ));
        s.ping = Some(12);
        let texts: Vec<_> = browser::COLUMNS_V118
            .iter()
            .map(|&c| browser::column_cell(&s, c))
            .collect();
        use browser::Cell::{Image, Text};
        let t = |s: &str| Text(s.to_string());
        assert_eq!(
            texts,
            [
                t(""),
                Image("server_hardware_linux_dedicated"),
                t("CoD2Host"),
                t("mp_toujane"),
                t("0 (8)"),
                t("dm"),
                t("X"),
                t("X"),
                t(""),
                t(""),
                t("12"),
            ]
        );
        ui.browser.lists[0].push(s);
        // The map column draws the arena's long name.
        assert_eq!(
            ui.list_cell(&ui.browser.lists[0][0], browser::Column::Map),
            t("Toujane, Tunisia")
        );
        let fs = vcod_common::testing::cod2_fs().unwrap();
        let lib = vcod_cod2::materials::load_lib(&fs).unwrap();
        let quads = ui.build(640.0, 480.0, &Localized::load(&fs), &shell);
        let icon = quads
            .iter()
            .find(|q| q.texture == "server_hardware_linux_dedicated")
            .expect("the hw icon is drawn");
        assert!(lib.get(&icon.texture).is_some());
    }

    /// Start New Server: the map list and Start hand the app a CoD2
    /// server, whose command line runs `vcod-server --game cod2` with the
    /// server settings the menu edits.
    #[test]
    fn start_new_server_starts_a_cod2_vcod_server() {
        let Some((mut ui, mut shell)) = ui() else {
            return;
        };
        ui.open_main(&shell, &mut Vec::new());
        click(&mut ui, &mut shell, "@MENU_START_NEW_SERVER");
        assert!(names(&ui).contains(&"createserver_serversettings".to_string()));
        shell.execute("set sv_hostname \"my host\"; set g_password secret");
        let out = click(&mut ui, &mut shell, "@MENU_START");
        let req = out
            .iter()
            .find_map(|e| match e {
                UiEffect::StartServer(r) => Some(r.clone()),
                _ => None,
            })
            .unwrap_or_else(|| panic!("{out:?}"));
        assert!(req.cod2);
        assert!(ui.arenas.iter().any(|a| a.map == req.map), "{}", req.map);
        let value = |n: &str| shell.cvar(n).map(str::to_string);
        let dirs = (std::path::Path::new("d"), "main", None, false);
        let args = create::server_args(&req, 28960, dirs, &value);
        let has = |a: &str, b: &str| args.windows(2).any(|w| w[0] == a && w[1] == b);
        assert!(has("--game", "cod2"), "{args:?}");
        assert!(has("--gametype", &req.gametype), "{args:?}");
        assert!(has("--hostname", "my host"), "{args:?}");
        assert!(has("--set", "g_password=secret"), "{args:?}");
    }

    /// Esc over the game opens what `g_scriptMainMenu` names, `ingame` on
    /// the stock gametypes: its buttons answer the server, and Leave Game's
    /// Yes disconnects.
    #[test]
    fn esc_in_a_game_opens_the_servers_main_menu() {
        let Some((mut ui, mut shell)) = ui() else {
            return;
        };
        let fs = vcod_common::testing::cod2_fs().unwrap();
        let names_cs: Vec<String> = ["ingame", "team_britishgerman", "weapon_british"]
            .map(String::from)
            .to_vec();
        assert!(ui.load_script_menus(&fs, &names_cs, "").is_empty());
        ui.set_server_cvars([("ui_allow_weaponchange".to_string(), "1".to_string())].into());
        let mut out = Vec::new();
        super::escape(&mut ui, Some("ingame"), &shell, &mut out);
        assert_eq!(names(&ui), ["ingame"]);
        let out = click(&mut ui, &mut shell, "@MPUI_1_CHANGE_WEAPON");
        assert!(out.contains(&UiEffect::ScriptMenuResponse {
            menu: "ingame".into(),
            response: "changeweapon".into(),
        }));
        super::escape(&mut ui, Some("ingame"), &shell, &mut Vec::new());
        click(&mut ui, &mut shell, "@MPUI_5_LEAVE_GAME");
        assert_eq!(names(&ui), ["leavegame"]);
        let out = click(&mut ui, &mut shell, "@MPUI_1_YES");
        assert!(
            out.contains(&UiEffect::Command("disconnect".into())),
            "{out:?}"
        );
    }

    /// Before the server names a main menu, Esc opens the main menu over
    /// the game: Back to Game and Disconnect, no Join Game.
    #[test]
    fn esc_before_a_script_main_menu_opens_main_over_the_game() {
        let Some((mut ui, mut shell)) = ui() else {
            return;
        };
        super::escape(&mut ui, None, &shell, &mut Vec::new());
        assert_eq!(names(&ui), ["main", "bg", "main_text"]);
        let out = click(&mut ui, &mut shell, "@MENU_DISCONNECT");
        assert!(
            out.contains(&UiEffect::Command("disconnect".into())),
            "{out:?}"
        );
        super::escape(&mut ui, None, &shell, &mut Vec::new());
        click(&mut ui, &mut shell, "@MENU_BACKTOGAME");
        assert!(!ui.active());
    }

    /// The options screens a player needs: look (sensitivity slider,
    /// binds), move and shoot binds, graphics, sound (the
    /// `snd_volume` slider), each opening beside the options menu.
    #[test]
    fn the_options_screens_open_with_their_controls() {
        let Some((mut ui, mut shell)) = ui() else {
            return;
        };
        ui.open_main(&shell, &mut Vec::new());
        click(&mut ui, &mut shell, "@MENU_OPTIONS");
        let shown = |ui: &Ui, shell: &Shell, kind: i32, cvar: &str| {
            let top = *ui.open.last().unwrap();
            (0..ui.menus[top].items.len()).any(|i| {
                let it = &ui.menus[top].items[i];
                it.kind == kind
                    && it
                        .cvar
                        .as_deref()
                        .is_some_and(|c| c.eq_ignore_ascii_case(cvar))
                    && ui.shown(top, i, shell)
            })
        };
        for (tab, menu, kind, cvar) in [
            (
                "@MENU_LOOK",
                "options_look",
                ITEM_TYPE_SLIDER,
                "sensitivity",
            ),
            ("@MENU_MOVE", "options_move", ITEM_TYPE_BIND, "+forward"),
            ("@MENU_SHOOT", "options_shoot", ITEM_TYPE_BIND, "+attack"),
            (
                "@MENU_GRAPHICS",
                "options_graphics",
                ITEM_TYPE_SLIDER,
                "r_gamma",
            ),
            (
                "@MENU_SOUND",
                "options_sound",
                ITEM_TYPE_SLIDER,
                "snd_volume",
            ),
        ] {
            click(&mut ui, &mut shell, tab);
            assert_eq!(names(&ui).last().unwrap(), menu);
            assert!(shown(&ui, &shell, kind, cvar), "{menu}: {cvar}");
        }
    }

    /// `uiScript openMenuOnDvar` and its kin (`UI_RunMenuScript`, Mac
    /// 0x1548e8): the graphics Apply opens the restart popup while
    /// `g_reloading` is 0, a missing dvar does nothing.
    #[test]
    fn menus_open_and_close_on_a_dvar() {
        let Some((mut ui, mut shell)) = ui() else {
            return;
        };
        let script = |s: &str| s.split(' ').map(String::from).collect::<Vec<_>>();
        let m = ui.find("main").unwrap();
        let mut out = Vec::new();
        shell.execute("set g_reloading 0");
        ui.run(
            m,
            &script("uiScript openMenuOnDvarNot g_reloading 0 vid_restart_popmenu_dead"),
            &shell,
            &mut out,
        );
        assert!(names(&ui).is_empty());
        ui.run(
            m,
            &script("uiScript openMenuOnDvar g_reloading 0 vid_restart_popmenu_listen"),
            &shell,
            &mut out,
        );
        assert_eq!(names(&ui), ["vid_restart_popmenu_listen"]);
        ui.run(
            m,
            &script("uiScript closeMenuOnDvarNot g_reloading 1 vid_restart_popmenu_listen"),
            &shell,
            &mut out,
        );
        assert!(names(&ui).is_empty());
        ui.run(
            m,
            &script("uiScript openMenuOnDvar com_playerProfile x player_profile"),
            &shell,
            &mut out,
        );
        assert!(names(&ui).is_empty());
    }

    /// The graphics menu's video mode is an `ITEM_TYPE_DVARENUM` over
    /// `r_mode`'s modes: it shows the entry `ui_r_mode` names and a click
    /// steps it by index, back on a right click. The look menu's decimal
    /// field shows `sensitivity` to two places.
    #[test]
    fn the_video_mode_steps_through_r_modes_list() {
        let Some((mut ui, mut shell)) = ui() else {
            return;
        };
        shell.execute("set r_mode 800x600");
        shell.set_enum_domain(
            "r_mode",
            ["640x480", "800x600", "1024x768"]
                .map(String::from)
                .to_vec(),
        );
        shell.execute("setfromdvar ui_r_mode r_mode");
        let loc = Localized::load(&vcod_common::testing::cod2_fs().unwrap());
        let m = ui.find("options_graphics").unwrap();
        let i = (0..ui.menus[m].items.len())
            .find(|&i| ui.menus[m].items[i].enum_list.as_deref() == Some("r_mode"))
            .unwrap();
        let item = ui.menus[m].items[i].clone();
        let apply = |shell: &mut Shell, out: Vec<UiEffect>| {
            for e in out {
                if let UiEffect::Command(c) = e {
                    shell.execute(&c);
                }
            }
        };
        assert_eq!(ui.option_value(m, i, &loc, &shell).unwrap(), "800x600");
        let mut out = Vec::new();
        ui.enum_step(&item, false, &shell, &mut out);
        apply(&mut shell, out);
        assert_eq!(shell.cvar("ui_r_mode"), Some("2"));
        assert_eq!(ui.option_value(m, i, &loc, &shell).unwrap(), "1024x768");
        let mut out = Vec::new();
        ui.enum_step(&item, true, &shell, &mut out);
        ui.enum_step(&item, true, &shell, &mut out);
        apply(&mut shell, out);
        assert_eq!(shell.cvar("ui_r_mode"), Some("0"));
        // The Apply's `setfromdvar r_mode ui_r_mode` stores the entry.
        shell.execute("setfromdvar r_mode ui_r_mode");
        assert_eq!(shell.cvar("r_mode"), Some("640x480"));

        let look = ui.find("options_look").unwrap();
        let d = (0..ui.menus[look].items.len())
            .find(|&i| ui.menus[look].items[i].kind == ITEM_TYPE_DECIMALFIELD)
            .unwrap();
        shell.execute("set sensitivity 5");
        assert_eq!(ui.option_value(look, d, &loc, &shell).unwrap(), "5.00");
    }

    /// Player profiles: with none active the main menu opens the profile
    /// menu and, with no profile on disk, the create popup; creating one
    /// makes its directory and selects it, loading it hands the app its
    /// name, and deleting it removes the directory.
    #[test]
    fn a_profile_is_created_loaded_and_deleted() {
        let Some((mut ui, mut shell)) = ui() else {
            return;
        };
        let dir = std::env::temp_dir().join(format!("vcod-ui-profiles-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        ui.set_profiles_dir(dir.clone());
        shell.use_cod2_cvars();
        let apply = |shell: &mut Shell, out: &[UiEffect]| {
            for e in out {
                if let UiEffect::Command(c) = e {
                    shell.execute(c);
                }
            }
        };
        let mut out = Vec::new();
        ui.open_main(&shell, &mut out);
        apply(&mut shell, &out);
        let open = names(&ui);
        assert!(open.iter().any(|n| n == "player_profile"), "{open:?}");
        assert!(
            open.iter().any(|n| n == "profile_create_popmenu"),
            "{open:?}"
        );
        shell.execute("set ui_playerProfileNameNew Tester");
        let script = |s: &str| s.split(' ').map(String::from).collect::<Vec<_>>();
        let m = ui.find("player_profile").unwrap();
        let mut out = Vec::new();
        ui.run(m, &script("uiScript createPlayerProfile"), &shell, &mut out);
        apply(&mut shell, &out);
        assert!(dir.join("Tester").is_dir());
        assert_eq!(shell.cvar("ui_playerProfileSelected"), Some("Tester"));
        assert_eq!(shell.cvar("ui_playerProfileCount"), Some("1"));
        assert_eq!(shell.cvar("ui_playerProfileNameNew"), Some(""));
        let mut out = Vec::new();
        ui.run(m, &script("uiScript loadPlayerProfile"), &shell, &mut out);
        assert!(out.contains(&UiEffect::LoadProfile("Tester".into())));
        let mut out = Vec::new();
        ui.run(m, &script("uiScript deletePlayerProfile"), &shell, &mut out);
        apply(&mut shell, &out);
        assert!(!dir.join("Tester").exists());
        assert_eq!(shell.cvar("ui_playerProfileCount"), Some("0"));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
