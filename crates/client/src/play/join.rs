//! The stock team/weapon menu handshake as a pure state machine
//! (docs/research/cod11-hud-protocol.md, section 0.1). `JoinProbe`
//! (`crates/client/src/probe/join.rs`) is the headless reference this mirrors,
//! minus the socket.

use super::cvars::ClientCvars;
use crate::console::shell::Shell;
use vcod_common::net::protocol::{CS_LAYOUT_V1, CsLayout, CsRange};

/// The 32 script menu configstrings a `t <index>` opens: 1180..=1211 on
/// 1.1, 1333..=1364 on United Offensive.
fn script_menus(cs: &CsLayout) -> std::ops::RangeInclusive<usize> {
    let (first, last) = cs.bounds(CsRange::Menu);
    first..=last
}

/// What a server command asks of the UI's script menus.
#[derive(Debug, PartialEq)]
pub enum MenuCommand {
    /// A reliable command to send: an auto-answer's `mr`, or the `bad` one
    /// for a `t` naming no menu.
    Send(String),
    /// `t`: open the script menu `name`, configstring index `index`, as
    /// `UIMENU_SCRIPT_POPUP`, or `UIMENU_SCRIPT_POPUP_NO_MOUSE`
    /// (cgame_mp_x86.dll 0x3002dae0).
    Popup {
        name: String,
        index: i32,
        no_mouse: bool,
    },
    /// `u`: close the script popup (0x3002de60).
    Close,
}

/// The UI menu `mp_QuickMessage` opens (`UI_SetActiveMenu` case 8,
/// docs/research/cod11-quick-chat.md, "The quick message menu").
pub const QUICK_MESSAGE_MENU: &str = "quickmessage";

/// The cgame's side of the script menus for one connection: which `t`s are
/// answered on the player's behalf, the one the UI refused for now, and the
/// stray key catcher. The menus themselves live in the front end's `Ui`.
pub struct Join {
    team: Option<String>,
    weapon: Option<String>,
    /// Menu indices an auto-answer already went out for, so a menu the
    /// server refuses and reopens is offered to the user instead of
    /// answered again (`JoinProbe::answered`).
    auto_answered: Vec<i32>,
    /// A `t` the UI refused while another menu had the focus
    /// (`ui_waitingScriptMenu`), offered again each frame (0x3002dd10).
    waiting: Option<Waiting>,
    /// The UI key catcher `mp_QuickMessage` set with no menu to show; the
    /// next key goes to the UI and is lost ([`Join::take_stray_key`]).
    stray_catcher: bool,
    pub cvars: ClientCvars,
    /// The connection's configstring layout.
    cs: CsLayout,
}

/// A refused `t`: the menu's name, its configstring index and whether it
/// asked for no mouse.
#[derive(Debug, Clone, PartialEq)]
pub struct Waiting {
    pub name: String,
    pub index: i32,
    pub no_mouse: bool,
}

/// `team_`/`weapon_` menu names auto-answer with the matching field, when
/// set; anything else has no scripted reply. With a team set, 1.5's
/// `serverinfo_<gametype>`, which its stock gametypes open ahead of the
/// team menu, is closed (docs/research/cod15-protocol-diff.md).
fn auto_reply<'a>(name: &str, team: Option<&'a str>, weapon: Option<&'a str>) -> Option<&'a str> {
    if name.starts_with("team_") {
        return team;
    }
    if name.starts_with("serverinfo_") {
        return team.map(|_| "close");
    }
    if name.starts_with("weapon_") {
        return weapon;
    }
    None
}

/// `scriptMenuResponse` (ui_mp_x86.dll 0x40011140): `mr <serverId> <index>
/// <response>`, the index the first script-menu configstring holding the
/// menu's name, -1 when none does.
pub fn script_menu_response(
    menu: &str,
    response: &str,
    configstrings: &[String],
    cs: &CsLayout,
    server_id: i32,
) -> String {
    let menus = script_menus(cs);
    let first = *menus.start();
    let index = menus
        .into_iter()
        .find(|&i| configstrings.get(i).is_some_and(|n| n == menu))
        .map_or(-1, |i| (i - first) as i32);
    format!("mr {server_id} {index} {response}")
}

/// The names in the script-menu configstrings, which the cgame has the UI
/// load (cgame_mp_x86.dll 0x3002c060).
pub fn script_menu_names(configstrings: &[String], cs: &CsLayout) -> Vec<String> {
    script_menus(cs)
        .filter_map(|i| configstrings.get(i))
        .filter(|n| !n.is_empty())
        .cloned()
        .collect()
}

/// Whether a server command runs `CG_MapRestart`: 1.1's `n`, and on 1.5
/// and United Offensive `n` (keeping `pers[]`) or `B` (dropping it).
/// 1.1 has no `B` handler, so it never comes from a 1.1 server
/// (cod15-protocol-diff.md 8, events doc 9.8).
pub fn is_map_restart(tokens: &[String]) -> bool {
    tokens.first().is_some_and(|t| t == "n" || t == "B")
}

/// The menus `CG_MapRestart` closes for one restart command (events doc
/// 9.8): the script popup twice, its responses held back, then sets of
/// three Escs to the UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RestartCloses {
    /// The two `u` closes (1.1 cgame 0x3002de60) under
    /// `ui_scriptMenuAllowResponse` 0.
    pub popups: bool,
    /// Escape syscalls, each three down Escs (1.1 0x7c, CoDMP.exe
    /// 0x40362c; 1.5 0x7d; UO 0x7e).
    pub escapes: u32,
    /// 1.1's syscall sends none while the UI's active menu type is
    /// `UIMENU_MAIN`; 1.5's and UO's none while a fullscreen menu is up.
    pub skip_on_main: bool,
}

impl RestartCloses {
    /// 1.1's `n`: everything once. 1.5: `B` (`CG_MapRestart(0)`) everything
    /// once, `n` nothing. UO: `B` the popups and two sets of Escs, `n` one
    /// set.
    pub fn for_command(tokens: &[String], v1: bool, uo: bool) -> Self {
        let full = tokens.first().is_some_and(|t| t == "B");
        let (popups, escapes) = match (v1, uo, full) {
            (true, _, _) => (true, 1),
            (_, true, true) => (true, 2),
            (_, true, false) => (false, 1),
            (_, _, true) => (true, 1),
            _ => (false, 0),
        };
        RestartCloses {
            popups,
            escapes,
            skip_on_main: v1,
        }
    }
}

impl Join {
    pub fn new(team: Option<String>, weapon: Option<String>) -> Self {
        Self {
            team,
            weapon,
            auto_answered: Vec::new(),
            waiting: None,
            stray_catcher: false,
            cvars: ClientCvars::default(),
            cs: CS_LAYOUT_V1,
        }
    }

    /// The connection's configstring layout, which a gamestate settles.
    pub fn set_layout(&mut self, cs: CsLayout) {
        self.cs = cs;
        self.cvars.set_layout(cs);
    }

    /// `t <idx>` opens the script menu `configstrings[1180 + idx]` names,
    /// answered at once if it is a `team_`/`weapon_` menu covered by
    /// `Join::new`'s arguments and not already auto-answered since the last
    /// rearm; `u` closes it and drops a waiting one. Everything else is
    /// ignored; `v` goes through `self.cvars` before this.
    pub fn on_server_command(
        &mut self,
        tokens: &[String],
        configstrings: &[String],
        server_id: i32,
    ) -> Option<MenuCommand> {
        match tokens.first().map(String::as_str) {
            Some("t") => {
                let idx = tokens.get(1).and_then(|t| t.parse::<i32>().ok())?;
                let menus = script_menus(&self.cs);
                let slot = usize::try_from(idx)
                    .ok()
                    .map(|i| menus.start() + i)
                    .filter(|cs| menus.contains(cs));
                let Some(name) = slot
                    .and_then(|cs| configstrings.get(cs))
                    .filter(|n| !n.is_empty())
                else {
                    // Either way the cgame answers `bad` (0x3002dae0).
                    let why = if slot.is_some() { "non-loaded" } else { "bad" };
                    crate::console::log::print(&format!(
                        "Server tried to open a {why} script menu index: {idx}"
                    ));
                    return Some(MenuCommand::Send(format!("mr {idx} bad")));
                };
                if !self.auto_answered.contains(&idx)
                    && let Some(reply) =
                        auto_reply(name, self.team.as_deref(), self.weapon.as_deref())
                {
                    self.auto_answered.push(idx);
                    return Some(MenuCommand::Send(format!("mr {server_id} {idx} {reply}")));
                }
                self.stray_catcher = false;
                // cgame's `t` handler (0x3002dae0) asks for no mouse when a
                // third argument is there and not empty.
                let no_mouse = tokens.get(2).is_some_and(|t| !t.is_empty());
                Some(MenuCommand::Popup {
                    name: name.clone(),
                    index: idx,
                    no_mouse,
                })
            }
            Some("u") => {
                self.waiting = None;
                Some(MenuCommand::Close)
            }
            _ => None,
        }
    }

    /// `g_scriptMainMenu`, the menu Esc opens in a live game.
    pub fn main_menu(&self, configstrings: &[String], shell: &Shell) -> Option<String> {
        self.cvars
            .get("g_scriptMainMenu", configstrings, shell)
            .filter(|n| !n.is_empty())
    }

    /// The UI refused a popup; it waits (cgame_mp_x86.dll 0x3002dae0). A
    /// menu of another name waiting before it gives way, and the
    /// `mr <index> noop` returned tells the server; one of the same name
    /// (any case) stays as it was.
    pub fn wait(&mut self, menu: Waiting) -> Option<String> {
        match &self.waiting {
            Some(w) if w.name.eq_ignore_ascii_case(&menu.name) => None,
            old => {
                let noop = old.as_ref().map(|w| format!("mr {} noop", w.index));
                self.waiting = Some(menu);
                noop
            }
        }
    }

    /// The waiting popup, to offer the UI again; [`Self::opened_waiting`]
    /// once it takes it.
    pub fn waiting(&self) -> Option<&Waiting> {
        self.waiting.as_ref()
    }

    /// The UI took the waiting popup.
    pub fn opened_waiting(&mut self) {
        self.waiting = None;
    }

    /// `mp_QuickMessage` opened its menu, which takes the stray catcher
    /// over.
    pub fn quick_message_opened(&mut self) {
        self.stray_catcher = false;
    }

    /// `mp_QuickMessage` with no `quickmessage` loaded (after `ui_load`):
    /// `UI_SetActiveMenu` case 8 closes every menu and still sets the UI key
    /// catcher (docs/research/cod11-quick-chat.md).
    pub fn quick_message_missing(&mut self) {
        self.stray_catcher = true;
    }

    /// A key press while the stray catcher is up: `_UI_KeyEvent` finds no
    /// menu, clears the catcher and drops the key (ui_mp_x86.dll
    /// 0x4000d752). True when this key is the one lost.
    pub fn take_stray_key(&mut self) -> bool {
        std::mem::take(&mut self.stray_catcher)
    }

    /// The stray catcher holds the mouse too (`CL_MouseEvent`, CoDMP.exe
    /// 0x40b0a0): the view does not turn until the next key.
    pub fn stray_catcher(&self) -> bool {
        self.stray_catcher
    }

    /// A new gamestate reruns retail's `ClientConnect`, which puts the
    /// client back on the menus (`JoinProbe::rearm`). It also takes the UI
    /// key catcher down, the stray one included: `CL_DownloadsComplete`
    /// shuts the UI down (CoDMP.exe 0x40ffb0, 0x418e30).
    pub fn on_gamestate(&mut self) {
        self.rearm();
        self.waiting = None;
        self.stray_catcher = false;
    }

    /// `n`: under `dm`, a round restart reopens the team menu under the
    /// index it last used; under `sd`, whose `pers[]` survives, nothing
    /// reopens and this is inert (`JoinProbe::reopen_menus`). The caller
    /// runs `CG_MapRestart`'s cvar resets (`ClientCvars::on_map_restart`).
    pub fn on_restart(&mut self) {
        self.rearm();
    }

    fn rearm(&mut self) {
        self.auto_answered.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cs() -> Vec<String> {
        let mut cs = vec![String::new(); 1300];
        cs[1180] = "team_russiangerman".into();
        cs[1181] = "weapon_russian".into();
        cs
    }
    fn toks(s: &str) -> Vec<String> {
        s.split(' ').map(str::to_string).collect()
    }
    fn popup(name: &str, index: i32, no_mouse: bool) -> Option<MenuCommand> {
        Some(MenuCommand::Popup {
            name: name.into(),
            index,
            no_mouse,
        })
    }
    fn waiting(name: &str, index: i32) -> Waiting {
        Waiting {
            name: name.into(),
            index,
            no_mouse: false,
        }
    }

    #[test]
    fn t_opens_the_menu_its_configstring_names() {
        let mut j = Join::new(None, None);
        let mut shell = Shell::new();
        assert!(
            j.cvars
                .on_server_command(&toks("v g_scriptMainMenu team_russiangerman"), &mut shell)
                .is_some()
        );
        assert_eq!(
            j.on_server_command(&toks("t 0"), &cs(), 7),
            popup("team_russiangerman", 0, false)
        );
        assert_eq!(
            j.main_menu(&cs(), &shell).as_deref(),
            Some("team_russiangerman")
        );
        assert_eq!(
            script_menu_response("team_russiangerman", "allies", &cs(), &CS_LAYOUT_V1, 7),
            "mr 7 0 allies"
        );
        assert_eq!(
            script_menu_response("nope", "x", &cs(), &CS_LAYOUT_V1, 7),
            "mr 7 -1 x"
        );
    }

    #[test]
    fn a_t_with_a_third_argument_asks_for_no_mouse() {
        let mut j = Join::new(None, None);
        assert_eq!(
            j.on_server_command(&toks("t 1 1"), &cs(), 3),
            popup("weapon_russian", 1, true)
        );
    }

    #[test]
    fn a_missing_quick_message_menu_eats_the_next_key_only() {
        let mut j = Join::new(None, None);
        j.quick_message_missing();
        assert!(j.stray_catcher());
        assert!(j.take_stray_key());
        assert!(!j.take_stray_key());
        // A script menu that opens first takes the catcher over.
        j.quick_message_missing();
        j.on_server_command(&toks("t 0"), &cs(), 7);
        assert!(!j.take_stray_key());
        // So does a new gamestate.
        j.quick_message_missing();
        j.on_gamestate();
        assert!(!j.stray_catcher());
    }

    #[test]
    fn u_closes_and_drops_a_waiting_menu() {
        let mut j = Join::new(None, None);
        assert_eq!(j.wait(waiting("team_russiangerman", 0)), None);
        assert_eq!(
            j.on_server_command(&toks("u"), &cs(), 7),
            Some(MenuCommand::Close)
        );
        assert_eq!(j.waiting(), None);
    }

    /// A second refused `t` of another menu takes the waiting one's place
    /// and the server hears `noop` for the first; the same menu again
    /// changes nothing (cgame_mp_x86.dll 0x3002dc3d).
    #[test]
    fn a_refused_t_replaces_the_waiting_menu_with_a_noop() {
        let mut j = Join::new(None, None);
        assert_eq!(j.wait(waiting("team_russiangerman", 0)), None);
        assert_eq!(j.wait(waiting("TEAM_russiangerman", 4)), None);
        assert_eq!(j.waiting(), Some(&waiting("team_russiangerman", 0)));
        assert_eq!(
            j.wait(waiting("weapon_russian", 1)).as_deref(),
            Some("mr 0 noop")
        );
        assert_eq!(j.waiting(), Some(&waiting("weapon_russian", 1)));
        j.opened_waiting();
        assert_eq!(j.waiting(), None);
    }

    #[test]
    fn auto_answers_team_then_weapon() {
        let mut j = Join::new(Some("axis".into()), Some("kar98k_mp".into()));
        assert_eq!(
            j.on_server_command(&toks("t 0"), &cs(), 3),
            Some(MenuCommand::Send("mr 3 0 axis".into()))
        );
        assert_eq!(
            j.on_server_command(&toks("t 1"), &cs(), 3),
            Some(MenuCommand::Send("mr 3 1 kar98k_mp".into()))
        );
    }

    /// 1.5's stock gametypes put `serverinfo_<gametype>` first; with a team
    /// to join it is closed, without one the user sees it.
    #[test]
    fn serverinfo_is_closed_on_the_way_to_the_team_menu() {
        let mut cs = cs();
        cs[1180] = "serverinfo_dm".into();
        cs[1181] = "team_russiangerman".into();
        let mut j = Join::new(Some("axis".into()), None);
        assert_eq!(
            j.on_server_command(&toks("t 0"), &cs, 3),
            Some(MenuCommand::Send("mr 3 0 close".into()))
        );
        assert_eq!(
            j.on_server_command(&toks("t 1"), &cs, 3),
            Some(MenuCommand::Send("mr 3 1 axis".into()))
        );
        let mut j = Join::new(None, None);
        assert_eq!(
            j.on_server_command(&toks("t 0"), &cs, 3),
            popup("serverinfo_dm", 0, false)
        );
    }

    #[test]
    fn auto_weapon_answers_once_per_open() {
        let mut j = Join::new(None, Some("bogus_mp".into()));
        assert!(matches!(
            j.on_server_command(&toks("t 1"), &cs(), 3),
            Some(MenuCommand::Send(_))
        ));
        // The server refused and reopened the same menu: now the user picks.
        assert_eq!(
            j.on_server_command(&toks("t 1"), &cs(), 3),
            popup("weapon_russian", 1, false)
        );
    }

    #[test]
    fn n_and_b_restart_the_map() {
        assert!(is_map_restart(&toks("n")));
        assert!(is_map_restart(&toks("B")));
        assert!(!is_map_restart(&toks("b 0 0")));
        assert!(!is_map_restart(&[]));
    }

    #[test]
    fn restart_closes_follow_each_build() {
        let c = |cmd: &str, v1, uo| {
            let r = RestartCloses::for_command(&toks(cmd), v1, uo);
            (r.popups, r.escapes, r.skip_on_main)
        };
        assert_eq!(c("n", true, false), (true, 1, true));
        assert_eq!(c("B", false, false), (true, 1, false));
        assert_eq!(c("n", false, false), (false, 0, false));
        assert_eq!(c("B", false, true), (true, 2, false));
        assert_eq!(c("n", false, true), (false, 1, false));
    }

    #[test]
    fn rearm_answers_again() {
        let mut j = Join::new(Some("allies".into()), None);
        assert!(j.on_server_command(&toks("t 0"), &cs(), 3).is_some());
        j.on_restart();
        assert_eq!(
            j.on_server_command(&toks("t 0"), &cs(), 4),
            Some(MenuCommand::Send("mr 4 0 allies".into()))
        );
        j.on_gamestate();
        assert_eq!(
            j.on_server_command(&toks("t 0"), &cs(), 5),
            Some(MenuCommand::Send("mr 5 0 allies".into()))
        );
    }

    #[test]
    fn a_t_naming_an_empty_slot_answers_bad() {
        let mut j = Join::new(None, None);
        assert_eq!(
            j.on_server_command(&toks("t 9"), &cs(), 3),
            Some(MenuCommand::Send("mr 9 bad".into()))
        );
    }

    /// Index 32 is past the 32 script-menu slots: cs 1212 is the first hint
    /// string, not a menu.
    #[test]
    fn a_t_past_the_script_menu_range_answers_bad() {
        let mut cs = cs();
        cs[1212] = "team_hintstring".into();
        let mut j = Join::new(Some("allies".into()), None);
        assert_eq!(
            j.on_server_command(&toks("t 32"), &cs, 3),
            Some(MenuCommand::Send("mr 32 bad".into()))
        );
    }

    #[test]
    fn the_precache_list_is_the_named_slots() {
        let mut cs = cs();
        cs[1185] = "quickcommands".into();
        assert_eq!(
            script_menu_names(&cs, &CS_LAYOUT_V1),
            ["team_russiangerman", "weapon_russian", "quickcommands"]
        );
    }
}
