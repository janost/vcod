//! The HUD's zone assets on a 1.7 install (`COD4_DIR`; vacuous without):
//! the fonts and their page, the localized strings, the kill icons and the
//! game message windows (cod4-hud.md 1, 3).

use vcod_cod4::hud::HudAssets;
use vcod_cod4::testing::cod4_install;

#[test]
fn hud_assets_come_out_of_the_ui_zones() {
    let Some(install) = cod4_install() else {
        return;
    };
    let a = HudAssets::load(&install, "mp_crash").unwrap();
    for name in [
        "fonts/smallFont",
        "fonts/normalFont",
        "fonts/bigFont",
        "fonts/boldFont",
        "fonts/extraBigFont",
        "fonts/objectiveFont",
        "fonts/consoleFont",
    ] {
        let (f, page) = a.font(name).unwrap_or_else(|| panic!("no {name}"));
        assert!(f.glyphs.len() > 90, "{name}: {} glyphs", f.glyphs.len());
        assert_eq!(page.as_deref(), Some("gamefonts_pc"), "{name}");
    }
    assert_eq!(a.font("fonts/normalFont").unwrap().0.pixel_height, 16);
    assert_eq!(a.localize["MP_CONNECTED"], "&&1 Connected");
    assert_eq!(a.localize["CGAME_YOUKILLED"], "You killed &&1");
    let m16 = &a.kill_icons["m16_gl_mp"];
    assert_eq!(m16.material, "hud_icon_m16a4_grenade");
    assert_eq!((m16.ratio, m16.flip), (2, true));
    assert!(a.images.contains_key(&m16.material), "{m16:?}");
    // `gamemessages`: bottom left, lines growing up; `boldgamemessages`:
    // centred, objective font.
    let w0 = a.windows[&0];
    assert_eq!(
        (w0.x, w0.y, w0.horz_align, w0.vert_align),
        (6.0, -65.0, 1, 3)
    );
    assert_eq!((w0.text_scale, w0.mode), (0.28125, 1));
    let w1 = a.windows[&1];
    assert_eq!(
        (w1.y, w1.horz_align, w1.font, w1.alignment),
        (-60.0, 7, 6, 1)
    );
    // The compass: its back, the map's window and the player's arrow.
    let bg = &a.compass["minimapBG"];
    assert_eq!(bg.rect, [-8.0, 12.0, 125.0, 125.0]);
    assert_eq!(bg.background.as_deref(), Some("minimap_background"));
    assert_eq!(a.compass["mini_map"].rect, [6.0, 18.0, 102.0, 102.0]);
    assert_eq!(
        a.compass["compassplayer"].background.as_deref(),
        Some("compassping_player")
    );
    // The scoreboard's rank icons and levels (cod4-hud.md 7).
    assert!(a.rank_icons.contains_key(&(0, 0)), "no rank icon table");
    assert_eq!(a.rank_levels.get(&0).map(String::as_str), Some("1"));
    // `weaponinfo`'s owner draws; the stance icon sits off screen
    // (cod4-hud.md 9).
    let stock = &a.weapon_info["ammoStock"];
    assert_eq!(
        (stock.owner_draw, stock.rect.rect[0], stock.rect.horz_align),
        (119, -34.0, 3)
    );
    assert_eq!(a.weapon_info["weapname"].owner_draw, 81);
    let stance = a.stance.as_ref().unwrap();
    assert_eq!((stance.owner_draw, stance.rect.rect[0]), (20, -1000.0));
    let m16 = &a.weapon_hud["m16_gl_mp"];
    assert_eq!(
        (m16.ammo_name.as_str(), m16.offhand_class),
        ("5.56 × 45 mm nato", 0)
    );
    assert_eq!(a.weapon_hud["frag_grenade_mp"].offhand_class, 1);
    // The clip graphic's kind and the clip's size (cod4-hud.md 9).
    let clip = |w: &str| {
        let d = &a.weapon_hud[w];
        (d.ammo_counter_clip, d.clip_size)
    };
    assert_eq!(clip("m16_gl_mp"), (1, 30));
    assert_eq!(clip("beretta_mp"), (1, 15));
    assert_eq!(clip("m40a3_mp"), (2, 5));
    assert_eq!(clip("winchester1200_mp"), (3, 7));
    assert_eq!(clip("rpg_mp"), (4, 1));
    assert_eq!(clip("m60e4_mp"), (5, 100));
    assert_eq!(clip("gl_m16_mp"), (6, 1));
    for m in [
        "ammo_counter_bullet",
        "ammo_counter_riflebullet",
        "ammo_counter_shotgunshell",
        "ammo_counter_rocket",
        "ammo_counter_beltbullet",
    ] {
        assert!(a.images.contains_key(m), "no {m}");
    }
    // The killcam's letterbox, two translucent 640x112 bars over the
    // whole screen, and its title (cod4-hud.md 11).
    let kc = &a.killcam;
    assert_eq!(kc.len(), 3, "{kc:?}");
    for (bar, y) in kc[..2].iter().zip([0.0, 368.0]) {
        assert_eq!(bar.rect.rect, [0.0, y, 640.0, 112.0]);
        assert_eq!((bar.rect.horz_align, bar.rect.vert_align), (4, 4));
        assert_eq!(bar.rect.background.as_deref(), Some("white"));
        assert_eq!(bar.fore_color, [0.49, 0.05, 0.03, 0.5]);
        assert!(a.images.contains_key("white"));
    }
    let title = &kc[2];
    assert_eq!(title.text.as_deref(), Some("KILLCAM"));
    assert_eq!(
        (
            title.rect.rect[1],
            title.rect.horz_align,
            title.rect.vert_align
        ),
        (45.0, 2, 1)
    );
    assert_eq!(
        (
            title.font,
            title.text_scale,
            title.text_style,
            title.text_align
        ),
        (6, 0.9, 6, 1)
    );
    // The map's minimap material.
    assert!(a.images.contains_key("compass_map_mp_crash"), "no minimap");
    // The game types' display names, what `gameTypeName()` translates.
    assert_eq!(a.gametypes["war"], "MPUI_WAR");
    assert_eq!(a.gametypes["sd"], "MPUI_SEARCH_AND_DESTROY_CLASSIC");
    assert_eq!(a.gametypes.len(), 6, "{:?}", a.gametypes);
    assert_eq!(a.localize["MPUI_WAR"], "Team Deathmatch");
}

/// The team and class menus' responses, which `vcod --game cod4` lists and
/// answers with `mr` (cod4-hud.md 12): `changeclass_marines`, the one the
/// server opens after a team pick, opens `changeclass` on an online game.
#[test]
fn the_team_and_class_menus_list_their_responses() {
    let Some(install) = cod4_install() else {
        return;
    };
    let a = HudAssets::load(&install, "mp_crash").unwrap();
    let team = &a.script_menus["team_marinesopfor"];
    assert_eq!(
        team.responses,
        ["axis", "allies", "autoassign", "spectator"]
    );
    assert_eq!(team.esc.as_deref(), Some("back"));
    let marines = &a.script_menus["changeclass_marines"];
    let online = |d: &str| (d == "onlinegame").then(|| "1".to_string());
    assert_eq!(marines.shown(online), "changeclass");
    let offline = |d: &str| (d == "onlinegame").then(|| "0".to_string());
    assert_eq!(marines.shown(offline), "changeclass_offline");
    let class = &a.script_menus["changeclass"];
    assert_eq!(
        &class.responses[..5],
        [
            "assault_mp,0",
            "specops_mp,0",
            "heavygunner_mp,0",
            "demolitions_mp,0",
            "sniper_mp,0"
        ]
    );
}

/// The in-game pages' controls as `common_mp` gives them (cod4-hud.md
/// 12.6): `options_look`'s sensitivity slider (an `editFieldDef_s` of 1..30,
/// default 5) and its bind rows, `options_graphics`' antialiasing list,
/// the call vote's and mute list's list boxes and their feeders, and the
/// arena file's maps and `_gametypes.txt`'s order the votes pick from.
#[test]
fn the_in_game_pages_carry_their_controls() {
    use vcod_cod4::assets::ui::{EditField, TypeData};
    use vcod_cod4::scriptmenu::{ITEM_TYPE_BIND, ITEM_TYPE_LISTBOX, ITEM_TYPE_SLIDER};
    let Some(install) = cod4_install() else {
        return;
    };
    let a = HudAssets::load(&install, "mp_crash").unwrap();
    let items = |menu: &str| {
        a.script_menus[menu]
            .layout
            .clone()
            .unwrap_or_else(|| panic!("{menu} has a layout"))
            .items
    };
    let look = items("options_look");
    let sens = look
        .iter()
        .find(|it| it.dvar.as_deref() == Some("sensitivity"))
        .unwrap();
    assert_eq!(sens.kind, ITEM_TYPE_SLIDER);
    let TypeData::Edit(EditField {
        min, max, default, ..
    }) = sens.type_data
    else {
        panic!("{:?}", sens.type_data);
    };
    assert_eq!((min, max, default), (1.0, 30.0, 5.0));
    let binds: Vec<&str> = look
        .iter()
        .filter(|it| it.kind == ITEM_TYPE_BIND)
        .filter_map(|it| it.dvar.as_deref())
        .collect();
    assert_eq!(
        binds,
        [
            "+leanleft",
            "+leanright",
            "+lookup",
            "+lookdown",
            "+left",
            "+right",
            "+mlook",
            "centerview"
        ]
    );
    let aa = items("options_graphics")
        .into_iter()
        .find(|it| it.dvar.as_deref() == Some("ui_r_aasamples"))
        .unwrap();
    let TypeData::Multi(m) = &aa.type_data else {
        panic!("{:?}", aa.type_data);
    };
    assert_eq!(m.labels, ["@MENU_OFF", "@MENU_2X", "@MENU_4X"]);
    assert_eq!(
        (m.values.as_slice(), m.str_def),
        (&[1.0, 2.0, 4.0][..], false)
    );
    for (menu, name, feeder) in [
        ("changemap", "maplist", 4.0),
        ("changegametype", "maplist2", 4.0),
        ("kickplayer", "maplist", 7.0),
        ("muteplayer", "playermutelist", 20.0),
    ] {
        let lb = items(menu)
            .into_iter()
            .find(|it| it.name == name)
            .unwrap_or_else(|| panic!("{menu} {name}"));
        assert_eq!((lb.kind, lb.feeder), (ITEM_TYPE_LISTBOX, feeder), "{menu}");
        let TypeData::ListBox(l) = &lb.type_data else {
            panic!("{menu}: {:?}", lb.type_data);
        };
        assert_eq!(l.element_height, 24.0, "{menu}");
    }
    assert_eq!(a.gametype_order, ["dm", "dom", "sd", "sab", "war", "koth"]);
    assert_eq!(a.arenas.len(), 21);
    assert_eq!(a.arenas[0].map, "mp_convoy");
    assert_eq!(a.arenas[0].longname, "MPUI_CONVOY");
}

/// Every expression of every menu the HUD zones and mp_crash carry (288
/// menus) evaluates (cod4-hud.md 12.2): none falls back to the list.
#[test]
fn every_menu_expression_evaluates() {
    use vcod_cod4::menuexp::{self, Env, Value};
    let Some(install) = cod4_install() else {
        return;
    };
    struct Empty;
    impl Env for Empty {
        fn dvar(&self, _: &str) -> Option<String> {
            None
        }
        fn local(&self, _: &str) -> Option<Value> {
            None
        }
        fn stat(&self, _: i32) -> i32 {
            0
        }
        fn team(&self) -> i32 {
            0
        }
        fn table_lookup(&self, _: &str, _: i32, _: &str, _: i32) -> String {
            String::new()
        }
    }
    let a = HudAssets::load(&install, "mp_crash").unwrap();
    let mut fails = Vec::new();
    let mut drawn = 0;
    for (name, menu) in &a.script_menus {
        let Some(l) = &menu.layout else {
            continue;
        };
        drawn += 1;
        let statements = std::iter::once(&l.visible).chain(l.items.iter().flat_map(|it| {
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
        for st in statements {
            if let Err(e) = menuexp::eval(st, &Empty) {
                fails.push(format!("{name}: {}", e.0));
            }
        }
    }
    assert!(drawn >= 288, "{drawn} menus");
    assert!(fails.is_empty(), "{fails:#?}");
}

/// The client's startup configs set the team menu's
/// `scr_game_spectatetype` 1 (`default_mp_gamesettings.cfg`).
#[test]
fn startup_configs_set_the_spectate_type() {
    let Some(install) = cod4_install() else {
        return;
    };
    let dirs = install.iwd_dirs();
    let iwds = vcod_common::pk3::Pk3Fs::open_iwd_layers(&dirs[0], &dirs[1..]).unwrap();
    let d = vcod_cod4::hud::startup_dvars(|n| iwds.read(n));
    let get = |name: &str| {
        d.iter()
            .rev()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    };
    assert_eq!(get("scr_game_spectatetype"), Some("1"));
    assert_eq!(get("scr_game_allowkillcam"), Some("1"));
    assert!(d.len() > 100, "{} dvars", d.len());
}

/// Pins `menuexp::eval` over every expression of the stock menus (the HUD
/// zones with mp_crash, and the front end's), under an empty env and a
/// full one whose dvars, locals, stats and game view vary with the name
/// asked. A change to the evaluator that moves any value moves the digest;
/// `MENUEXP_PIN_DUMP=<path>` writes the lines it hashes.
#[test]
fn menu_expression_values_hold() {
    use vcod_cod4::menuexp::{self, Env, GameView, ScoreRow, Value};
    let Some(install) = cod4_install() else {
        return;
    };
    fn mix(s: &str) -> u32 {
        s.bytes().fold(2166136261u32, |h, b| {
            (h ^ u32::from(b)).wrapping_mul(16777619)
        })
    }
    struct Empty;
    impl Env for Empty {
        fn dvar(&self, _: &str) -> Option<String> {
            None
        }
        fn local(&self, _: &str) -> Option<Value> {
            None
        }
        fn stat(&self, _: i32) -> i32 {
            0
        }
        fn team(&self) -> i32 {
            0
        }
        fn table_lookup(&self, _: &str, _: i32, _: &str, _: i32) -> String {
            String::new()
        }
    }
    struct Full(GameView);
    const SPELLED: [&str; 8] = ["", "0", "1", "2", "2.5", "-3", "abc", "1e2"];
    impl Env for Full {
        fn dvar(&self, name: &str) -> Option<String> {
            let h = mix(name);
            (!h.is_multiple_of(9)).then(|| SPELLED[(h / 9) as usize % SPELLED.len()].to_string())
        }
        fn local(&self, name: &str) -> Option<Value> {
            let h = mix(name);
            Some(match h % 4 {
                0 => return None,
                1 => Value::Int((h / 4 % 5) as i32),
                2 => Value::Float((h / 4 % 7) as f32 * 0.5),
                _ => Value::Str(SPELLED[(h / 4) as usize % SPELLED.len()].into()),
            })
        }
        fn stat(&self, i: i32) -> i32 {
            i.wrapping_mul(7).rem_euclid(5)
        }
        fn team(&self) -> i32 {
            2
        }
        fn table_lookup(&self, t: &str, c: i32, v: &str, r: i32) -> String {
            format!("{t}|{c}|{v}|{r}")
        }
        fn millis(&self) -> i32 {
            123456
        }
        fn gametype(&self) -> String {
            "war".into()
        }
        fn gametype_name(&self) -> String {
            "Team Deathmatch".into()
        }
        fn gametype_description(&self) -> String {
            "objective".into()
        }
        fn game(&self) -> Option<&GameView> {
            Some(&self.0)
        }
        fn key_binding(&self, cmd: &str) -> String {
            format!("key[{cmd}]")
        }
        fn localize(&self, text: &str) -> String {
            format!("loc[{text}]")
        }
        fn menu_open(&self, name: &str) -> bool {
            mix(name).is_multiple_of(2)
        }
        fn ui_active(&self) -> bool {
            false
        }
    }
    let full = Full(GameView {
        scoped: true,
        killcam: true,
        clip_ammo: 7,
        own: Some(ScoreRow {
            score: 40,
            ping: 60,
            deaths: 3,
            kills: 5,
        }),
        ranked: vec![90, 40, 10],
        team_scores: [0, 120, 340, 0],
        time_left: Some(185),
        action_slots: [true, false, true, false],
        fade_dpad: 0.25,
        fade_weapon: 0.5,
        fade_compass: 0.75,
        message_windows: [true, false, false, true],
        ..GameView::default()
    });
    let mut lines = Vec::new();
    for (zones, a) in [
        ("hud", HudAssets::load(&install, "mp_crash").unwrap()),
        ("front", HudAssets::load_front(&install).unwrap()),
    ] {
        let mut names: Vec<_> = a.script_menus.keys().collect();
        names.sort();
        for name in names {
            let Some(l) = &a.script_menus[name].layout else {
                continue;
            };
            let statements = std::iter::once(&l.visible).chain(l.items.iter().flat_map(|it| {
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
            for (i, st) in statements.enumerate().filter(|(_, st)| !st.is_empty()) {
                let empty = menuexp::eval(st, &Empty);
                let rich = menuexp::eval(st, &full);
                lines.push(format!("{zones} {name} {i} {empty:?} {rich:?}"));
            }
        }
    }
    let text = lines.join("\n");
    if let Some(path) = std::env::var_os("MENUEXP_PIN_DUMP") {
        std::fs::write(path, &text).unwrap();
    }
    let digest = text.bytes().fold(0xcbf29ce484222325u64, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x100000001b3)
    });
    assert_eq!((lines.len(), digest), (53156, 2862808926149014726), "pin");
}
