//! What the HUD menus' expressions read ([`HudEnv`]): dvars, configstrings,
//! the game type's strings, key bindings, and the action slots.

use std::collections::HashMap;
use vcod_common::net::Msec;

use vcod_cod4::hud as h;
use vcod_cod4::net::parse::Snapshot;
use vcod_common::localize::Localized;

use super::assets::Loaded;
use super::{CS_GAME_END_TIME, Cod4Hud, PM_DEAD, in_killcam, menus};
use crate::console::shell::Shell;
use crate::play::cod4::Cod4Session;

impl Cod4Hud {
    /// What the menus' cgame calls read off the newest snapshot and the
    /// last `score` reply (cod4-hud.md 12.2): the board, the killcam,
    /// death, spectating, intermission, the clip, night vision, the team
    /// scores and the player's own row. The HUD's fades read 1: vcod
    /// fades none of the three (`hud_fade_*` 0, never).
    pub(crate) fn menu_game(&self, s: &Cod4Session, now: i32) -> vcod_cod4::menuexp::GameView {
        use vcod_cod4::menuexp::{GameView, ScoreRow};
        let mut g = GameView {
            fade_dpad: 1.0,
            fade_weapon: 1.0,
            fade_compass: 1.0,
            ..GameView::default()
        };
        let own = s.gamestate().map_or(0, |gs| gs.client_num);
        g.team_scores = self.team_scores;
        if let Some(sc) = &self.scores {
            g.ranked = sc.rows.iter().map(|r| r.score).collect();
            g.own = sc.rows.iter().find(|r| r.client == own).map(|r| ScoreRow {
                score: r.score,
                ping: r.ping,
                deaths: r.deaths,
                kills: r.kills,
            });
        }
        let Some(snap) = s.client().snapshots.last() else {
            return g;
        };
        let u = |name: &str| snap.ps.get(name).unwrap_or(0);
        // The time left: configstring 11, `setGameEndTime`'s
        // level time (`CG_ParseGameEndTime`), against the snapshot's.
        let end = h::atoi(&self.configstring(s, CS_GAME_END_TIME));
        g.time_left = Some((Msec::from_raw(end) - snap.server_time) / 1000);
        g.scoreboard = self.scoreboard_fade(snap, now).is_some();
        g.killcam = in_killcam(snap);
        g.dead = u("pm_type") >= PM_DEAD;
        g.intermission = snap.ps.word(vcod_cod4::ps_word!("pm_type")) == h::PM_INTERMISSION;
        g.following = u("otherFlags") & 2 != 0;
        g.selecting_location = u("locationSelectionInfo") != 0;
        g.night_vision = crate::cod4_look::night_vision(u("weapFlags"));
        // `CG_GetPlayerClipAmmoCount`: the held weapon's clip slot.
        if let (Some(l), Some(gs)) = (self.loaded.as_ref().and_then(|l| l.as_ref()), s.gamestate())
        {
            let list = crate::play::cod4::weapon_list(gs);
            let slots = l.ammo_slots(&list);
            let w = u("weapon") as usize;
            if let Some(&(_, clip)) = w.checked_sub(1).and_then(|i| slots.get(i)) {
                g.clip_ammo = snap.ps.ammoclip.get(clip).copied().unwrap_or(0);
            }
        }
        g.action_slots = std::array::from_fn(|k| action_slot(s, snap, k).usable);
        g.fade_dpad = if u("weapFlags") & 0x80 != 0 { 0.0 } else { 1.0 };
        g
    }

    /// The HUD menus' expression context: the session's menus' with the
    /// HUD's dvars (the configstring mirror, then `v`), our team off the
    /// snapshot and the cgame state of [`Self::menu_game`].
    pub(super) fn env<'a>(&'a self, s: &'a Cod4Session, now: i32, shell: &'a Shell) -> HudEnv<'a> {
        let intermission = s
            .client()
            .snapshots
            .last()
            .is_some_and(|x| x.ps.word(vcod_cod4::ps_word!("pm_type")) == h::PM_INTERMISSION);
        HudEnv {
            hud: self,
            s,
            game: self.menu_game(s, now),
            team: s.own_team(),
            shell,
            board_menu: self.show_scores || intermission,
        }
    }

    /// Menu `name`'s `visible when`, as the HUD's context reads it; `None`
    /// before the zones load or when it does not evaluate.
    pub(super) fn menu_visible(&self, l: &Loaded, name: &str, env: &HudEnv) -> Option<bool> {
        let layout = l.script_menus.get(name)?.layout.as_ref()?;
        if layout.visible.is_empty() {
            return Some(true);
        }
        vcod_cod4::menuexp::truth(&layout.visible, env).ok()
    }

    /// A server message's text (`SEH_LocalizeTextMessage`); its key as
    /// itself before the strings are loaded.
    pub(super) fn message(&self, text: &str) -> String {
        match self.localized() {
            Some(l) => l.message(text),
            None => text.replace(['\u{14}', '\u{15}'], ""),
        }
    }

    /// `GetKeyBindingLocalizedString` (`iw3mp.exe` 0x552460): the one or
    /// two keys bound to `cmd` by their `KEY_*` names, two joined by
    /// `KEY_OR`, else `KEY_UNBOUND` (0x4380a8).
    pub(super) fn key_binding(&self, shell: &Shell, cmd: &str) -> String {
        let empty = Localized::default();
        let loc = self.localized().unwrap_or(&empty);
        shell
            .key_text(cmd, loc)
            .unwrap_or_else(|| loc.get("KEY_UNBOUND").unwrap_or("KEY_UNBOUND").to_string())
    }

    /// Configstring `i`: a `d` change, else the gamestate's.
    pub(crate) fn configstring(&self, s: &Cod4Session, i: u16) -> String {
        String::from_utf8_lossy(self.configstring_bytes(s, i)).into_owned()
    }

    /// [`Self::configstring`] as stored, without a copy.
    fn configstring_bytes<'a>(&'a self, s: &'a Cod4Session, i: u16) -> &'a [u8] {
        match self.cs.get(&i) {
            Some(v) => v.as_bytes(),
            None => s.gamestate().map_or(&[], |g| g.configstring(i)),
        }
    }

    /// A server dvar: the configstrings' mirror (names at
    /// `CS_CODINFO`..=147, each value 128 slots on), else a `v` command's.
    /// The HUD's menus ask some fifty a frame, so the names are compared
    /// where they lie (cod4-client-perf.md 33).
    pub(super) fn server_dvar(&self, s: &Cod4Session, name: &str) -> Option<String> {
        (h::CS_CODINFO..h::CS_CODINFO_VALUE)
            .find(|&i| {
                self.configstring_bytes(s, i)
                    .eq_ignore_ascii_case(name.as_bytes())
            })
            .map(|i| self.configstring(s, i + h::CS_CODINFO_VALUE - h::CS_CODINFO))
            .or_else(|| self.dvars.get(name).cloned())
    }

    /// What the menus' `gameType()`, `gameTypeName()` and
    /// `gameTypeDescription()` read: serverinfo's `g_gametype`, its
    /// display name translated (the name itself for a game type the zones
    /// do not list, `UI_GetGameTypeDisplayName`) and `cg_objectiveText`
    /// localized (`CG_GetGametypeDescription`, cod4-hud.md 12.2).
    pub(crate) fn gametype_strings(&self, s: &Cod4Session) -> (String, String, String) {
        let info = self.configstring(s, 0);
        let parts: Vec<&str> = info.split('\\').collect();
        let gametype = parts
            .iter()
            .skip(1)
            .step_by(2)
            .zip(parts.iter().skip(2).step_by(2))
            .find(|(k, _)| k.eq_ignore_ascii_case("g_gametype"))
            .map_or_else(String::new, |(_, v)| v.to_string());
        let name = self
            .loaded
            .as_ref()
            .and_then(|l| l.as_ref())
            .and_then(|l| {
                let key = l.gametypes.get(&gametype.to_ascii_lowercase())?;
                Some(l.localized.get(key).unwrap_or(key).to_string())
            })
            .unwrap_or_else(|| gametype.clone());
        let objective = self
            .server_dvar(s, "cg_objectiveText")
            .map(|t| self.message(&t))
            .unwrap_or_default();
        (gametype, name, objective)
    }

    /// `ui_hud_hardcore`, which the prematch sets (cod4-hud.md 13): the
    /// `compass_old` and `weaponinfo` menus' `visible when` read it 0.
    pub(super) fn hardcore(&self, s: &Cod4Session) -> bool {
        self.server_dvar(s, "ui_hud_hardcore")
            .is_some_and(|v| vcod_cod4::menuexp::atoi(&v) != 0)
    }

    /// The configstrings' dvar mirror by lowercased name, for the script
    /// menus.
    pub(crate) fn mirrored_dvars(&self, s: &Cod4Session) -> HashMap<String, String> {
        (h::CS_CODINFO..h::CS_CODINFO_VALUE)
            .filter_map(|i| {
                let name = self.configstring(s, i);
                (!name.is_empty()).then(|| {
                    let v = self.configstring(s, i + h::CS_CODINFO_VALUE - h::CS_CODINFO);
                    (name.to_ascii_lowercase(), v)
                })
            })
            .collect()
    }
}

/// One action slot as `CG_ActionSlotIsUsable` and the slot's active
/// test read it.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) struct ActionSlot {
    usable: bool,
    pub(super) active: bool,
    /// The weapon it shows, 0 for none.
    pub(super) weapon: u8,
    pub(super) night_vision: bool,
}

/// Action slot `k` (0..4) of the drawn playerstate: a weapon slot is
/// usable while its weapon is held and active while selected; an alt
/// slot while the selection or a held primary has an alt mode; a night
/// vision slot always, active while the goggles are on.
pub(super) fn action_slot(s: &Cod4Session, snap: &Snapshot, k: usize) -> ActionSlot {
    use crate::play::cod4_input::{holds, slot_kind};
    let ps = &snap.ps;
    let u = |n: &str| ps.get(n).unwrap_or(0);
    let held: [u32; 4] = std::array::from_fn(|i| u(&format!("weapons[{i}]")));
    let selected = s.input.weapons().0;
    match u(&format!("actionSlotType[{k}]")) {
        slot_kind::WEAPON => {
            let w = u(&format!("actionSlotParam[{k}]")) as u8;
            let usable = w != 0 && holds(held, w);
            ActionSlot {
                usable,
                active: w != 0 && w == selected,
                weapon: if usable { w } else { 0 },
                night_vision: false,
            }
        }
        slot_kind::ALT_WEAPON => {
            let (w, active) = s.alt_toggle_weapon();
            ActionSlot {
                usable: w != 0,
                active,
                weapon: w,
                night_vision: false,
            }
        }
        slot_kind::NIGHT_VISION => ActionSlot {
            usable: true,
            active: crate::cod4_look::night_vision(u("weapFlags")),
            weapon: 0,
            night_vision: true,
        },
        _ => ActionSlot::default(),
    }
}

/// What the HUD menus' expressions read (`Cod4Hud::env`).
pub(super) struct HudEnv<'a> {
    pub(super) hud: &'a Cod4Hud,
    pub(super) s: &'a Cod4Session,
    pub(super) game: vcod_cod4::menuexp::GameView,
    pub(super) team: i32,
    pub(super) shell: &'a Shell,
    /// The UI's `scoreboard` menu is up: `+scores` held or the
    /// intermission (cod4-hud.md 23).
    board_menu: bool,
}

impl vcod_cod4::menuexp::Env for HudEnv<'_> {
    fn dvar(&self, name: &str) -> Option<String> {
        self.hud
            .server_dvar(self.s, name)
            .or_else(|| self.s.menus.env().dvar(name))
    }
    fn local(&self, name: &str) -> Option<vcod_cod4::menuexp::Value> {
        self.s.menus.env().local(name)
    }
    fn stat(&self, index: i32) -> i32 {
        self.s.menus.env().stat(index)
    }
    fn team(&self) -> i32 {
        self.team
    }
    fn table_lookup(&self, table: &str, column: i32, value: &str, ret: i32) -> String {
        self.s.menus.env().table_lookup(table, column, value, ret)
    }
    fn game(&self) -> Option<&vcod_cod4::menuexp::GameView> {
        Some(&self.game)
    }
    /// `UI_GetKeyBindingLocalizedStringSingle`: the first key's `KEY_*`
    /// text, `KEY_UNBOUND`'s with none.
    fn key_binding(&self, cmd: &str) -> String {
        let empty = Localized::default();
        let loc = self.hud.localized().unwrap_or(&empty);
        match self.shell.keys_bound_to(cmd).first() {
            Some(k) => loc.get(&format!("KEY_{k}")).unwrap_or(k).to_string(),
            None => loc.get("KEY_UNBOUND").unwrap_or("KEY_UNBOUND").to_string(),
        }
    }
    fn localize(&self, text: &str) -> String {
        self.hud.message(text)
    }
    fn gametype(&self) -> String {
        self.hud.gametype_strings(self.s).0
    }
    fn gametype_name(&self) -> String {
        self.hud.gametype_strings(self.s).1
    }
    fn gametype_description(&self) -> String {
        self.hud.gametype_strings(self.s).2
    }
    fn ui_active(&self) -> bool {
        self.s.menus.open().is_some() || self.board_menu
    }
    fn menu_open(&self, name: &str) -> bool {
        (self.board_menu && name.eq_ignore_ascii_case(menus::SCOREBOARD_MENU))
            || self.s.menus.env().menu_open(name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use super::super::tests::{replay, replay_in};
    use super::super::weapon_info::{DPAD_MENU, NVG_ICON, OwnerDraw};

    /// The walk capture's spawn gives slot 1 night vision and slot 3 the
    /// M16's grenade launcher (`actionSlotType` 3 and 2): the `dpad` menu
    /// draws the goggles' icon, the launcher's `dpadIcon` with its rounds
    /// and the `[key]` under each, and nothing on slots 2 and 4.
    #[test]
    fn walk_capture_draws_the_action_slots() {
        if vcod_cod4::testing::cod4_install().is_none() {
            return;
        }
        let mut seen: Vec<OwnerDraw> = Vec::new();
        let mut shell = Shell::new();
        shell.use_cod4_binds();
        replay_in("play-1.7-walk.txt", true, |hud, s, now| {
            let (Some(Some(l)), Some(snap)) = (&hud.loaded, s.client().snapshots.last()) else {
                return;
            };
            let env = hud.env(s, now, &shell);
            if hud.menu_visible(l, DPAD_MENU, &env) == Some(true) {
                let d = hud.dpad(l, s, snap, &env);
                if !d.is_empty() {
                    seen = d;
                }
            }
        });
        let pics: Vec<&str> = seen
            .iter()
            .filter_map(|d| match d {
                OwnerDraw::Pic { material, .. } => Some(material.as_str()),
                _ => None,
            })
            .collect();
        let texts: Vec<&str> = seen
            .iter()
            .filter_map(|d| match d {
                OwnerDraw::Text { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        println!("{pics:?} {texts:?}");
        assert_eq!(pics.len(), 2, "{pics:?}");
        assert_eq!(pics[0], NVG_ICON);
        // `BG_WeaponAmmo`: the launcher's clip and its reserve, one each.
        assert!(texts.contains(&"  2"), "{texts:?}");
        assert!(
            texts.contains(&"[N]") && texts.contains(&"[5]"),
            "{texts:?}"
        );
    }

    /// The killcam's respawn line names the `+activate` key: vcod's
    /// default bind, F (cod4-hud.md 8).
    #[test]
    fn kill_capture_names_the_activate_key() {
        let shell = Shell::new();
        let mut texts = std::collections::BTreeSet::new();
        let (hud, _) = replay("play-1.7-kill-target.txt", |hud, s, now| {
            let Some(snap) = s.client().snapshots.last() else {
                return;
            };
            for e in h::hud_elems(&snap.ps) {
                if let Some(t) = hud.elem_text(s, &e, now, &shell) {
                    texts.insert(t);
                }
            }
        });
        if hud.loaded.is_none() {
            return;
        }
        println!("{texts:?}");
        assert!(texts.iter().all(|t| !t.contains("[{")), "{texts:?}");
        assert!(texts.contains("Press^3 F ^7to respawn"), "{texts:?}");
    }
}
