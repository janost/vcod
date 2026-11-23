//! Contains routines ported from the RTCW-MP GPL source, Copyright (C) 1999-2010 id Software LLC, a ZeniMax Media company.
//! See NOTICE.
//!
//! Menu scripts: the item and menu script commands, `uiScript`, the CD key
//! fields, favourites and the cvars the scripts read.

use std::time::Instant;

use vcod_common::net::cdkey;
use vcod_common::ui_menu::{self, FEEDER_MODS, FEEDER_SERVERS, FEEDER_SERVERSTATUS};

use super::browser::{AddFavorite, Source};
use super::paint::fill;
use super::status::StatusQuery;
use super::{UNSUPPORTED, Ui, UiEffect, profiles};
use crate::console::shell::Shell;

pub(super) fn unsupported(name: &str) -> bool {
    UNSUPPORTED.iter().any(|s| s.eq_ignore_ascii_case(name))
}

impl Ui {
    /// `ui_joinGametype` as an index into "All" and the game types; out of
    /// range reads as All, as `UI_DrawJoinGameType` resets it (0x400088b0).
    pub(super) fn join_game_type(&self) -> usize {
        let v = self.ui_cvar("ui_joinGametype");
        v.trim()
            .parse::<f32>()
            .ok()
            .map(|f| f as i32)
            .and_then(|i| usize::try_from(i).ok())
            .filter(|&i| i <= self.game_types.len())
            .unwrap_or(0)
    }

    /// A [`UI_CVARS`] value: this input's write, else last frame's.
    pub(super) fn ui_cvar(&self, name: &str) -> String {
        if let Some(v) = self.options.pending(name) {
            return v.to_string();
        }
        self.cvars
            .get(&name.to_ascii_lowercase())
            .cloned()
            .unwrap_or_default()
    }

    /// `Item_RunScript` over `script`, with `menu` as the item's parent.
    pub(super) fn run(
        &mut self,
        menu: usize,
        script: &[String],
        shell: &Shell,
        out: &mut Vec<UiEffect>,
    ) {
        for cmd in ui_menu::script_commands(script) {
            let arg = |i: usize| cmd.get(i).map(String::as_str).unwrap_or("");
            match cmd[0].to_ascii_lowercase().as_str() {
                "play" => out.push(UiEffect::Sound(arg(1).to_string())),
                "open" => self.open_menu(arg(1), shell, out),
                "close" => self.close_menu(arg(1), shell, out),
                "show" | "hide" => {
                    let visible = cmd[0].eq_ignore_ascii_case("show");
                    for (item, st) in self.menus[menu].items.iter().zip(&mut self.state[menu]) {
                        if item.name.eq_ignore_ascii_case(arg(1))
                            || item.group.eq_ignore_ascii_case(arg(1))
                        {
                            st.visible = visible;
                        }
                    }
                }
                "setitemcolor" => {
                    let c: [f32; 4] = std::array::from_fn(|k| arg(3 + k).parse().unwrap_or(0.0));
                    for (item, st) in self.menus[menu].items.iter().zip(&mut self.state[menu]) {
                        if item.name.eq_ignore_ascii_case(arg(1))
                            || item.group.eq_ignore_ascii_case(arg(1))
                        {
                            match arg(2).to_ascii_lowercase().as_str() {
                                "backcolor" => st.back = c,
                                "bordercolor" => st.border_color = c,
                                _ => {}
                            }
                        }
                    }
                }
                "exec" => out.push(UiEffect::Command(arg(1).to_string())),
                "setcvar" => self.set_cvar(arg(1), arg(2), out),
                "uiscript" if profiles::is_profile_script(arg(1)) => {
                    self.profile_script(arg(1), shell, out);
                }
                "uiscript" if menu_on_dvar(arg(1)).is_some() => {
                    self.menu_on_dvar(&cmd[1..], shell, out);
                }
                "uiscript" if arg(1).eq_ignore_ascii_case("getCDKey") => {
                    let (key, check) = cdkey::menu_key(self.key_game.0);
                    self.cdkey_fields(&key, &check, out);
                }
                "uiscript" if arg(1).eq_ignore_ascii_case("verifyCDKey") => {
                    self.verify_cdkey(shell, out);
                }
                "uiscript" => self.ui_script(&cmd[1..], out),
                "scriptmenuresponse" => out.push(UiEffect::ScriptMenuResponse {
                    menu: self.menus[menu].name.clone(),
                    response: arg(1).to_string(),
                }),
                // `openForGameType <fmt>` / `closeForGameType <fmt>`
                // (0x40010770, 0x400107f0): the menu `fmt` names once its
                // `%s` holds the item's cvar.
                "openforgametype" | "closeforgametype" => {
                    let value = self
                        .script_cvar
                        .clone()
                        .map(|c| self.ui_cvar(&c))
                        .unwrap_or_default();
                    let name = fill(arg(1), &[&value]);
                    if cmd[0].eq_ignore_ascii_case("openforgametype") {
                        self.open_menu(&name, shell, out);
                    } else {
                        self.close_menu(&name, shell, out);
                    }
                }
                // `execOnCvarIntValue <cvar> <n> <command>` and its float
                // twin: the command runs when the cvar holds that value.
                "execoncvarintvalue" | "execoncvarfloatvalue" => out.push(UiEffect::ExecOnCvar {
                    cvar: arg(1).to_string(),
                    value: arg(2).trim().parse().unwrap_or(0.0),
                    int: cmd[0].eq_ignore_ascii_case("execoncvarintvalue"),
                    command: arg(3).to_string(),
                }),
                // Fades are cosmetic; vcod shows the item as it is.
                "fadein" | "fadeout" => {}
                // `main`'s Esc and Back to Game; inert with no game up.
                "ingameclose" if self.in_game => self.close_menu(arg(1), shell, out),
                _ => {}
            }
        }
    }

    /// CoD2's `openMenuOnDvar`, `openMenuOnDvarNot`, `closeMenuOnDvar` and
    /// `closeMenuOnDvarNot` `<dvar> <value> <menu>` (`UI_RunMenuScript`,
    /// Mac 0x1548e8): the menu opens or closes when the dvar's value equals
    /// `value` case folded, or for the `Not` pair when it does not. A dvar
    /// that does not exist does nothing (`cannot find dvar`), as does a
    /// missing argument.
    fn menu_on_dvar(&mut self, args: &[String], shell: &Shell, out: &mut Vec<UiEffect>) {
        let Some((open, equal)) = args.first().and_then(|a| menu_on_dvar(a)) else {
            return;
        };
        let [_, dvar, value, menu, ..] = args else {
            log::debug!("ui: {}: missing arguments", args[0]);
            return;
        };
        let Some(current) = self.cvar(dvar, shell) else {
            log::debug!("ui: {}: cannot find dvar {dvar}", args[0]);
            return;
        };
        if current.eq_ignore_ascii_case(value) == equal {
            if open {
                self.open_menu(menu, shell, out);
            } else {
                self.close_menu(menu, shell, out);
            }
        }
    }

    /// `getCDKey` (ui_mp_x86.dll 1.1 0x4000a72c): the five fields cleared,
    /// then the key's four quarters when it has 16 characters and the check
    /// when it has 4.
    fn cdkey_fields(&mut self, key: &str, check: &str, out: &mut Vec<UiEffect>) {
        for i in 1..=5 {
            self.set_cvar(&format!("cdkey{i}"), "", out);
        }
        if key.len() == 16 {
            for i in 0..4 {
                self.set_cvar(&format!("cdkey{}", i + 1), &key[i * 4..][..4], out);
            }
        }
        if check.len() == 4 {
            self.set_cvar("cdkey5", check, out);
        }
    }

    /// `verifyCDKey` (0x4000a87e): the first four fields joined are the key,
    /// the fifth its check; `ui_cdkeyvalid` says whether they pass, and a
    /// valid key is set (`net::cdkey::store`).
    fn verify_cdkey(&mut self, shell: &Shell, out: &mut Vec<UiEffect>) {
        let field = |i: u32| self.cvar(&format!("cdkey{i}"), shell).unwrap_or_default();
        let key: String = (1..=4).map(field).collect();
        let check = field(5);
        let ok = cdkey::valid(&key, &check);
        let text = if ok {
            "@EXE_CDKEYVALID"
        } else {
            "@EXE_CDKEYINVALID"
        };
        self.set_cvar("ui_cdkeyvalid", text, out);
        if ok {
            cdkey::store(self.key_game.0, &key, &check, self.key_game.1);
        }
    }

    /// `UI_RunMenuScript`, for the scripts the stock browser and main menu
    /// call.
    pub(super) fn ui_script(&mut self, args: &[String], out: &mut Vec<UiEffect>) {
        let Some(name) = args.first() else { return };
        match name.to_ascii_lowercase().as_str() {
            "refreshservers" => {
                self.browser.refresh();
                self.tops[FEEDER_SERVERS] = 0;
            }
            "refreshfilter" => self.browser.requery(),
            // CoD2's `mutePlayer` (`UI_RunMenuScript`, Mac 0x1548e8): the
            // selected row's client to `CL_MutePlayer`.
            "muteplayer" => {
                if let Some(&c) = self.player_clients.get(self.player_row) {
                    out.push(UiEffect::MutePlayer(c));
                }
            }
            "stoprefresh" | "closejoin" => self.browser.stop(),
            // `UpdateFilter` refreshes a Local list on its own (ui_mp_x86.dll
            // 0x4000ac92); the filter itself applies at every draw.
            "updatefilter" => {
                if self.browser.source == Source::Local {
                    self.browser.refresh();
                }
            }
            "serverstatus" => {
                self.status = self
                    .selected
                    .map(|addr| StatusQuery::start(addr, Instant::now()));
                self.tops[FEEDER_SERVERSTATUS] = 0;
            }
            "addfavorite" if self.browser.source != Source::Favorites => {
                let rows = self.browser.rows();
                let picked = self
                    .selected
                    .and_then(|a| rows.iter().find(|s| s.addr == a));
                let (name, addr) = picked.map_or((String::new(), String::new()), |s| {
                    let name = s
                        .info
                        .as_ref()
                        .map_or(String::new(), |i| i.hostname.clone());
                    (name, s.addr.to_string())
                });
                self.add_favorite(&name, &addr);
            }
            "createfavorite" if self.browser.source == Source::Favorites => {
                let name = self.ui_cvar("ui_favoriteName");
                let addr = self.ui_cvar("ui_favoriteAddress");
                self.add_favorite(&name, &addr);
            }
            "deletefavorite" if self.browser.source == Source::Favorites => {
                if let Some(addr) = self.selected.take() {
                    self.browser.remove_favorite(addr);
                }
            }
            "serversort" => {
                if let Some(c) = args.get(1).and_then(|c| c.parse().ok()) {
                    self.browser.server_sort(c);
                }
            }
            // 1.5's opens `joinpb_popmenu` on a `pb` server while
            // `cl_punkbuster` is 0 (ui_mp_x86.dll 1.5 0x4000b124), whose Yes
            // turns PunkBuster on. vcod runs no PunkBuster, so it connects
            // and shows the server's refusal (cod15-protocol-diff.md,
            // "PunkBuster").
            "joinserver" => {
                if let Some(addr) = self.selected {
                    out.push(UiEffect::Command(format!("connect {addr}")));
                }
            }
            "quit" => out.push(UiEffect::Command("quit".into())),
            // `UI_LoadMods` (ui_mp_x86.dll 0x40009e10), at most 64.
            "loadmods" => {
                self.mods = self
                    .mod_root
                    .as_ref()
                    .map(|(dir, base)| vcod_common::pk3::mod_list(dir, base))
                    .unwrap_or_default();
                self.mods.truncate(64);
                self.mod_selected = None;
                self.tops[FEEDER_MODS] = 0;
            }
            "runmod" => {
                if let Some(m) = self.mod_selected.and_then(|i| self.mods.get(i)) {
                    out.push(UiEffect::RunMod(Some(m.dir.clone())));
                }
            }
            // The base game again; no stock menu calls it.
            "quake3" => out.push(UiEffect::RunMod(None)),
            "clearerror" => self.error.clear(),
            "loadarenas" => self.load_arenas(out),
            "startserver" => self.start_server(out),
            w @ ("votemap" | "votetypemap" | "votegame") => self.vote_script(w, out),
            "votekick" => self.vote_player("kick", out),
            "votetempban" => self.vote_player("tempBanUser", out),
            // vcod reads binds straight from the console, so there is
            // nothing to load; one language is all vcod reads.
            "loadcontrols" | "getlanguage" | "verifylanguage" => {}
            // `update ui_mousePitch` (0x4000a377): m_pitch 0.022, negated
            // when the toggle is on.
            "update"
                if args
                    .get(1)
                    .is_some_and(|a| a.eq_ignore_ascii_case("ui_mousePitch")) =>
            {
                if let Some(v) = self.options.pending("ui_mousePitch") {
                    let on = v.trim().parse::<f32>().unwrap_or(0.0) != 0.0;
                    let pitch = if on { "-0.022" } else { "0.022" };
                    self.set_cvar("m_pitch", pitch, out);
                }
            }
            // The rate picker's refresh has nothing to update.
            "update" => {}
            other => log::debug!("ui: no UI script {other}"),
        }
    }

    /// The favourite checks `addFavorite` and `createFavorite` share
    /// (ui_mp_x86.dll 0x4000a4a0), leaving the outcome in
    /// `ui_favorite_message`.
    pub(super) fn add_favorite(&mut self, name: &str, addr: &str) {
        let key = if name.is_empty() {
            "@EXE_FAVORITENAMEEMPTY"
        } else if addr.is_empty() {
            "@EXE_FAVORITEADDRESSEMPTY"
        } else {
            match self.browser.add_favorite(name, addr) {
                AddFavorite::InList => "@EXE_FAVORITEINLIST",
                AddFavorite::Full => "@EXE_FAVORITELISTFULL",
                AddFavorite::BadAddress => "@EXE_BADSERVERADDRESS",
                AddFavorite::Added => "@EXE_FAVORITEADDED",
            }
        };
        self.fav_message = key.to_string();
    }

    /// The first menu `script` opens that vcod cannot show. A button whose
    /// script would close its own menu first and then open such a menu is
    /// refused whole, so the screen does not go blank.
    pub(super) fn refused(&self, script: &[String]) -> Option<String> {
        ui_menu::script_commands(script)
            .into_iter()
            .filter(|c| c.len() > 1 && c[0].eq_ignore_ascii_case("open"))
            .map(|c| c[1].clone())
            .find(|name| unsupported(name) || self.find(name).is_none())
    }
}

impl Ui {
    /// The cvars a menu reads: `cl_ingame` is whether the menus are up over
    /// a game, `shortversion` is vcod's, `com_errorMessage` the last error,
    /// `ui_multiplayer` 1 (this is the MP UI), `ui_mousePitch` on when `m_pitch` is
    /// negative (as the UI sets it at load), anything this input set, then
    /// the console's.
    pub(super) fn cvar(&self, name: &str, shell: &Shell) -> Option<String> {
        if let Some(v) = self.options.pending(name) {
            return Some(v.to_string());
        }
        match name.to_ascii_lowercase().as_str() {
            "cl_ingame" => Some(if self.in_game { "1" } else { "0" }.into()),
            "com_errormessage" => Some(self.error.clone()),
            "ui_favorite_message" => Some(self.fav_message.clone()),
            "shortversion" => Some(crate::version_tag()),
            "ui_multiplayer" => Some("1".into()),
            "ui_mousepitch" => Some(
                if shell.cvar_f32("m_pitch") < 0.0 {
                    "1"
                } else {
                    "0"
                }
                .into(),
            ),
            lower => match self.server_cvars.get(lower) {
                Some(v) => Some(v.clone()),
                None => shell.cvar(name).map(str::to_string),
            },
        }
    }
}

/// CoD2's `uiScript` menu-on-dvar names: whether it opens, and whether it
/// acts on an equal value.
fn menu_on_dvar(name: &str) -> Option<(bool, bool)> {
    match name.to_ascii_lowercase().as_str() {
        "openmenuondvar" => Some((true, true)),
        "openmenuondvarnot" => Some((true, false)),
        "closemenuondvar" => Some((false, true)),
        "closemenuondvarnot" => Some((false, false)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::time::Duration;

    use vcod_common::pk3::Pk3Fs;
    use winit::keyboard::KeyCode;

    use super::super::ITEM_TYPE_UPREDITFIELD;
    use super::super::tests::{apply, click_at, click_named, top_name, type_text, ui};

    #[test]
    fn unsupported_menus_stay_shut() {
        let Some(mut ui) = ui() else { return };
        let mut out = Vec::new();
        ui.open_main(&Shell::new(), &mut out);
        ui.open_menu("single_popmenu", &Shell::new(), &mut out);
        assert_eq!(ui.open.len(), 1);
        assert_eq!(ui.menus[ui.open[0]].name, "main");
    }

    /// `callvote.menu`'s map and game type picks go to the console as
    /// `callvote` lines off the selected row and game type.
    #[test]
    fn the_callvote_menu_scripts_name_the_selection() {
        let Some(mut ui) = ui() else { return };
        let mut out = Vec::new();
        ui.load_arenas(&mut out);
        let map = ui.active_maps()[0].map.clone();
        let gt = ui.game_types[ui.net_gametype()].script.clone();
        for (script, line) in [
            ("voteMap", format!("callvote map {map}")),
            ("voteTypeMap", format!("callvote typemap {gt} {map}")),
            ("voteGame", format!("callvote g_gametype {gt}")),
        ] {
            let mut out = Vec::new();
            ui.ui_script(&[script.to_string()], &mut out);
            assert_eq!(out, [UiEffect::Command(line)]);
        }
    }

    /// The kick pick names the selected snapshot client, cleaned and
    /// quoted; the list is rebuilt at most every 3 s.
    #[test]
    fn the_kick_pick_names_the_selected_player() {
        let mut ui = Ui::new(&Pk3Fs::empty(), "0");
        let t0 = Instant::now();
        ui.offer_players(t0, || vec!["^1Able".into(), "Baker Two".into()]);
        ui.offer_players(t0 + Duration::from_secs(1), || vec!["Charlie".into()]);
        assert_eq!(ui.players, ["Able", "Baker Two"]);
        let mut out = Vec::new();
        ui.ui_script(&["voteKick".to_string()], &mut out);
        assert_eq!(out, [UiEffect::Command("callvote kick \"Able\"".into())]);
        ui.player_row = 1;
        out.clear();
        ui.ui_script(&["voteKick".to_string()], &mut out);
        assert_eq!(
            out,
            [UiEffect::Command("callvote kick \"Baker Two\"".into())]
        );
        ui.offer_players(t0 + Duration::from_secs(4), || vec!["Charlie".into()]);
        out.clear();
        ui.ui_script(&["voteKick".to_string()], &mut out);
        assert!(out.is_empty(), "row 1 is past the rebuilt list");
    }

    /// 1.5's TEMPBAN list votes the same selected row as the kick list.
    #[test]
    fn the_temp_ban_pick_names_the_selected_player() {
        let mut ui = Ui::new(&Pk3Fs::empty(), "0");
        ui.offer_players(Instant::now(), || vec!["Able".into(), "^2Baker".into()]);
        ui.player_row = 1;
        let mut out = Vec::new();
        ui.ui_script(&["voteTempBan".to_string()], &mut out);
        assert_eq!(
            out,
            [UiEffect::Command("callvote tempBanUser \"Baker\"".into())]
        );
    }

    /// The list's clock runs only while a menu shows the list, as retail's
    /// build runs off that listbox's `feederCount`.
    #[test]
    fn the_player_list_counts_only_while_a_menu_shows_it() {
        let mut ui = super::super::tests::ui_with_menu(
            r#"{ menuDef { name "callvote" rect 0 0 640 480
              itemDef { name kicklist rect 0 0 160 120 type 6 feeder 7 visible 1
                elementheight 15 columns 1 2 190 25 }
              itemDef { name hidden rect 0 0 160 120 type 6 feeder 7 visible 0 } }
              menuDef { name "other" rect 0 0 640 480
              itemDef { name hidden rect 0 0 160 120 type 6 feeder 7 visible 0 } } }"#,
        );
        let shell = Shell::new();
        assert!(!ui.player_list_shown(&shell), "no menu open");
        ui.open_menu("other", &shell, &mut Vec::new());
        assert!(!ui.player_list_shown(&shell), "its list is hidden");
        ui.open_menu("callvote", &shell, &mut Vec::new());
        assert!(ui.player_list_shown(&shell));
    }

    /// A server-sent name holding a quote and `;` stays one forwarded
    /// `callvote`.
    #[test]
    fn a_kicked_players_name_runs_no_command_of_its_own() {
        let mut ui = Ui::new(&Pk3Fs::empty(), "0");
        ui.offer_players(Instant::now(), || vec!["\";quit;".into()]);
        let mut out = Vec::new();
        ui.ui_script(&["voteKick".to_string()], &mut out);
        let [UiEffect::Command(line)] = out.as_slice() else {
            panic!("{out:?}");
        };
        assert_eq!(
            Shell::new().execute(line),
            [crate::console::shell::Effect::Unknown {
                word: "callvote".into(),
                line: "callvote kick \";quit;\"".into(),
            }]
        );
    }

    #[test]
    fn max_chars_goto_next_steps_a_full_field_on() {
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
        ui.menus[m].items[name].max_chars = 2;
        ui.menus[m].items[name].max_chars_goto_next = true;
        click_at(&mut ui, m, name, &mut shell);
        for (code, c) in [(KeyCode::KeyA, "a"), (KeyCode::KeyB, "b")] {
            assert_eq!(ui.options.editing, Some((m, name)));
            let out = ui.edit_key(code, Some(c), &shell);
            apply(&mut shell, &out);
        }
        assert_eq!(ui.options.editing, Some((m, addr)));
        let out = ui.edit_key(KeyCode::KeyC, Some("c"), &shell);
        apply(&mut shell, &out);
        assert_eq!(shell.cvar("ui_favoriteName"), Some("ab"));
        assert_eq!(shell.cvar("ui_favoriteAddress"), Some("c"));
    }

    /// `cdkey_menu`: the upper-case fields take the key's quarters and its
    /// check, Verify says whether they pass and keeps a valid key.
    #[test]
    fn the_cd_key_menu_verifies_what_is_typed() {
        let Some(mut ui) = ui() else { return };
        // UO's key, which nothing else in these tests reads
        ui.key_game = (cdkey::KeyGame::Uo, false);
        let mut shell = Shell::new();
        ui.frame(Instant::now(), &shell);
        let mut out = Vec::new();
        ui.open_menu("cdkey_menu", &shell, &mut out);
        apply(&mut shell, &out);
        assert_eq!(top_name(&ui), "cdkey_menu");
        assert_eq!(shell.cvar("cdkey1"), Some(""));
        let m = ui.find("cdkey_menu").unwrap();
        let fields: Vec<usize> = (0..ui.menus[m].items.len())
            .filter(|&i| ui.menus[m].items[i].kind == ITEM_TYPE_UPREDITFIELD)
            .collect();
        assert_eq!(fields.len(), 5);
        let verify = |ui: &mut Ui, shell: &mut Shell, parts: [&str; 5]| {
            for (&i, text) in fields.iter().zip(parts) {
                click_at(ui, m, i, shell);
                let out = ui.edit_key(KeyCode::KeyA, Some(text), shell);
                apply(shell, &out);
                ui.options.editing = None;
            }
            click_named(ui, "cdkey_menu", "verify", shell);
            shell.cvar("ui_cdkeyvalid").map(str::to_string)
        };
        let bad = verify(
            &mut ui,
            &mut shell,
            ["abcd", "2345", "efgh", "6789", "0883"],
        );
        assert_eq!(bad.as_deref(), Some("@EXE_CDKEYINVALID"));
        // typed lower case, kept upper case
        assert_eq!(shell.cvar("cdkey1"), Some("ABCD"));
        assert_eq!(cdkey::menu_key(cdkey::KeyGame::Uo).0, "");
        // a full field takes nothing more
        shell.execute("set cdkey5 \"\"");
        let good = verify(
            &mut ui,
            &mut shell,
            ["abcd", "2345", "efgh", "6789", "0882"],
        );
        assert_eq!(good.as_deref(), Some("@EXE_CDKEYVALID"));
        let (key, check) = cdkey::menu_key(cdkey::KeyGame::Uo);
        assert_eq!((key.as_str(), check.as_str()), ("ABCD2345EFGH6789", "0882"));
        // reopened, the fields show the key
        let mut out = Vec::new();
        ui.open_menu("cdkey_menu", &shell, &mut out);
        apply(&mut shell, &out);
        assert_eq!(shell.cvar("cdkey4"), Some("6789"));
        assert_eq!(shell.cvar("cdkey5"), Some("0882"));
        cdkey::forget(cdkey::KeyGame::Uo);
    }

    #[test]
    fn the_password_and_filter_popups_set_their_cvars() {
        let Some(mut ui) = ui() else { return };
        let mut shell = Shell::new();
        ui.frame(Instant::now(), &shell);
        ui.open_menu("joinserver", &Shell::new(), &mut Vec::new());
        click_named(&mut ui, "joinserver", "passwordenter", &mut shell);
        assert_eq!(top_name(&ui), "password_popmenu");
        click_named(&mut ui, "password_popmenu", "passwordEntry", &mut shell);
        type_text(&mut ui, "letmein_and_more", &mut shell);
        // `maxchars 12`.
        assert_eq!(shell.cvar("password"), Some("letmein_and_"));
        assert_eq!(shell.userinfo().password, "letmein_and_");
        click_named(&mut ui, "password_popmenu", "yes", &mut shell);
        assert_eq!(top_name(&ui), "joinserver");

        click_named(&mut ui, "joinserver", "filterServers", &mut shell);
        assert_eq!(top_name(&ui), "filter_popmenu");
        let m = ui.find("filter_popmenu").unwrap();
        let i = ui.menus[m]
            .items
            .iter()
            .position(|i| i.cvar.as_deref() == Some("ui_browserShowEmpty"))
            .unwrap();
        click_at(&mut ui, m, i, &mut shell);
        assert_eq!(shell.cvar("ui_browserShowEmpty"), Some("0"));
        assert!(!ui.browser.filter.show_empty);
    }
}
