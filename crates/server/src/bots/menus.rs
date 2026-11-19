//! The join: server commands in, `mr` replies to the team and weapon menus out, and the stock
//! weapon menus.

use super::*;

impl Bot {
    /// The reliable server commands the bot received this tick, as
    /// `(seq, text)`; returns the `mr` replies to send, in order. `server_id`
    /// is what the reply must name; `allowed` answers whether a `scr_allow_*`
    /// cvar lets its weapon through.
    pub(crate) fn observe(
        &mut self,
        server_id: i32,
        cmds: &[(i32, &str)],
        allowed: &dyn Fn(&str) -> bool,
    ) -> Vec<String> {
        let mut replies = Vec::new();
        for (_, text) in cmds {
            let tokens: Vec<&str> = text.split_whitespace().collect();
            match tokens.as_slice() {
                ["v", "g_scriptMainMenu", menu] => {
                    // `set_client_cvar` quotes the value (hud protocol doc 0.1).
                    self.main_menu = menu.trim_matches('"').to_string();
                }
                // A restart reruns `ClientConnect` without a gamestate and
                // reopens the menus under the indices the last level used
                // (the `n` is what the probes forget them on too).
                ["n"] => self.rearm(),
                // `switchToWeapon` and a use pickup's `a <n>`: a cgame
                // selects `n` and its cmds ask for it (combat doc 9.8), so the
                // bot's byte does too; the pickup half reads only the byte.
                ["a", w] => {
                    // The held mask is 64 bits: a weapon past it is never held.
                    if let Ok(w) = w.parse::<u8>()
                        && w < 64
                    {
                        self.switch_to = Some(w);
                    }
                }
                ["t", idx] => {
                    let Ok(idx) = idx.parse::<i32>() else {
                        continue;
                    };
                    if self.answered.contains(&idx) {
                        continue;
                    }
                    let Some(reply) = self.menu_reply(allowed) else {
                        continue;
                    };
                    self.answered.push(idx);
                    replies.push(format!("mr {server_id} {idx} {reply}"));
                }
                _ => {}
            }
        }
        replies
    }

    /// The team menu takes the bot's team; the weapon menu takes a weapon
    /// its nationality offers and the cvars allow, picked at random.
    fn menu_reply(&mut self, allowed: &dyn Fn(&str) -> bool) -> Option<String> {
        // `bel`'s team menu takes axis only (`bel.gsc` 150, 287); the script
        // deals out the allied places.
        if self.main_menu == "team_germanonly" {
            return Some("axis".to_string());
        }
        if self.main_menu.starts_with("team_") {
            return Some(self.team.clone());
        }
        let menu = self.main_menu.strip_prefix("weapon_")?.to_string();
        let open: Vec<&'static str> = menu_weapons(self.menus, &menu)?
            .filter(|(_, cvar)| allowed(cvar))
            .map(|(w, _)| *w)
            .collect();
        if let Some((m, w)) = &self.loadout
            && *m == menu
            && open.contains(w)
        {
            return Some(w.to_string());
        }
        if open.is_empty() {
            return None;
        }
        let pick = open[self.rand() as usize % open.len()];
        self.loadout = Some((menu, pick));
        Some(pick.to_string())
    }

    /// The weapon menu and the weapon the bot answers it with, once picked.
    pub(crate) fn loadout(&self) -> Option<(&str, &'static str)> {
        self.loadout.as_ref().map(|(m, w)| (m.as_str(), *w))
    }

    /// The weapon the menu `menu` is answered with from now on, if it
    /// offers it and the cvars allow it.
    pub(crate) fn set_loadout(&mut self, menu: &str, weapon: &'static str) {
        self.loadout = Some((menu.to_string(), weapon));
    }

    /// A new gamestate reruns `ClientConnect` and reopens the menus under the
    /// indices the last one used; without the clear the bot never spawns
    /// again (the same rejoin the probes learned).
    pub fn rearm(&mut self) {
        self.stage = Stage::Wander;
        self.end_throw();
        self.main_menu.clear();
        self.answered.clear();
        self.round_start = true;
    }
}

/// A weapon menu's nationality and its weapons, each with the cvar
/// `_teams::restrict` checks it against.
pub(crate) type WeaponMenu = (&'static str, &'static [(&'static str, &'static str)]);

/// The weapons `menu` offers: a nationality's own, or for United
/// Offensive's two-nation menus (`weapon_americangerman`, `bel.gsc` 222)
/// both nations'.
pub(crate) fn menu_weapons<'a>(
    menus: &'a [WeaponMenu],
    menu: &str,
) -> Option<impl Iterator<Item = &'a (&'static str, &'static str)>> {
    let nation = |n: &str| menus.iter().find(|(m, _)| *m == n).map(|(_, w)| *w);
    if let Some(w) = nation(menu) {
        return Some(w.iter().chain([].iter()));
    }
    let (a, b) = menus
        .iter()
        .find_map(|(m, _)| Some((nation(m)?, nation(menu.strip_prefix(m)?)?)))?;
    Some(a.iter().chain(b.iter()))
}

/// The stock weapon menus (`ui_mp/scriptmenus/weapon_<nationality>.menu`),
/// each weapon with the cvar `_teams::restrict` checks it against. Any other
/// response is "restricted" and reopens the menu.
pub(super) const WEAPON_MENUS: [WeaponMenu; 4] = [
    (
        "american",
        &[
            ("m1carbine_mp", "scr_allow_m1carbine"),
            ("m1garand_mp", "scr_allow_m1garand"),
            ("thompson_mp", "scr_allow_thompson"),
            ("bar_mp", "scr_allow_bar"),
            ("springfield_mp", "scr_allow_springfield"),
        ],
    ),
    (
        "british",
        &[
            ("enfield_mp", "scr_allow_enfield"),
            ("sten_mp", "scr_allow_sten"),
            ("bren_mp", "scr_allow_bren"),
            ("springfield_mp", "scr_allow_springfield"),
        ],
    ),
    (
        "russian",
        &[
            ("mosin_nagant_mp", "scr_allow_nagant"),
            ("ppsh_mp", "scr_allow_ppsh"),
            ("mosin_nagant_sniper_mp", "scr_allow_nagantsniper"),
        ],
    ),
    (
        "german",
        &[
            ("kar98k_mp", "scr_allow_kar98k"),
            ("mp40_mp", "scr_allow_mp40"),
            ("mp44_mp", "scr_allow_mp44"),
            ("kar98k_sniper_mp", "scr_allow_kar98ksniper"),
        ],
    ),
];

/// United Offensive's menus (`pakuo00.pk3`), the same files with
/// `mg30cal_mp` added to the American and British, `svt40_mp` and
/// `dp28_mp` to the Russian, `gewehr43_mp` and `mg34_mp` to the German.
/// The three light machine guns fire only off their bipod
/// (coduo-movement.md 6, 7), which a bot deploys ([`Bot::bipod`]).
pub(crate) const WEAPON_MENUS_UO: [WeaponMenu; 4] = [
    (
        "american",
        &[
            ("m1carbine_mp", "scr_allow_m1carbine"),
            ("m1garand_mp", "scr_allow_m1garand"),
            ("thompson_mp", "scr_allow_thompson"),
            ("bar_mp", "scr_allow_bar"),
            ("springfield_mp", "scr_allow_springfield"),
            ("mg30cal_mp", "scr_allow_mg30cal"),
        ],
    ),
    (
        "british",
        &[
            ("enfield_mp", "scr_allow_enfield"),
            ("sten_mp", "scr_allow_sten"),
            ("bren_mp", "scr_allow_bren"),
            ("springfield_mp", "scr_allow_springfield"),
            ("mg30cal_mp", "scr_allow_mg30cal"),
        ],
    ),
    (
        "russian",
        &[
            ("mosin_nagant_mp", "scr_allow_nagant"),
            ("ppsh_mp", "scr_allow_ppsh"),
            ("svt40_mp", "scr_allow_svt40"),
            ("mosin_nagant_sniper_mp", "scr_allow_nagantsniper"),
            ("dp28_mp", "scr_allow_dp28"),
        ],
    ),
    (
        "german",
        &[
            ("kar98k_mp", "scr_allow_kar98k"),
            ("gewehr43_mp", "scr_allow_gewehr43"),
            ("mp40_mp", "scr_allow_mp40"),
            ("mp44_mp", "scr_allow_mp44"),
            ("kar98k_sniper_mp", "scr_allow_kar98ksniper"),
            ("mg34_mp", "scr_allow_mg34"),
        ],
    ),
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bots::tests::*;

    const SID: i32 = 0x10;

    #[test]
    fn an_allied_bot_answers_bels_team_menu_with_axis() {
        let mut bot = Bot::new("allies", false, 1);
        bot.observe(SID, &[(1, "v g_scriptMainMenu \"team_germanonly\"")], &all);
        let reply = bot.observe(SID, &[(2, "t 3")], &all);
        assert_eq!(reply, [format!("mr {SID} 3 axis")]);
    }

    #[test]
    fn a_reopened_weapon_menu_gets_the_same_weapon() {
        for seed in 1..16 {
            let mut bot = Bot::new("axis", false, seed);
            bot.observe(SID, &[(1, "v g_scriptMainMenu weapon_german")], &all);
            let first = bot.observe(SID, &[(2, "t 1")], &all);
            for idx in 2..6 {
                let t = format!("t {idx}");
                let again = bot.observe(SID, &[(idx, t.as_str())], &all);
                assert_eq!(
                    again[0].rsplit(' ').next(),
                    first[0].rsplit(' ').next(),
                    "seed {seed}"
                );
            }
        }
        // Restricted since: a new pick among what is left.
        let mut bot = Bot::new("axis", false, 3);
        bot.observe(SID, &[(1, "v g_scriptMainMenu weapon_german")], &all);
        let first = bot.observe(SID, &[(2, "t 1")], &all).remove(0);
        let w = first.rsplit(' ').next().unwrap().to_string();
        let cvar = WEAPON_MENUS[3].1.iter().find(|(n, _)| *n == w).unwrap().1;
        let not_it = |c: &str| c != cvar;
        let again = bot.observe(SID, &[(3, "t 2")], &not_it).remove(0);
        assert_ne!(again.rsplit(' ').next().unwrap(), w);
    }

    #[test]
    fn an_open_team_menu_is_answered_with_the_bots_team_once() {
        let mut bot = Bot::new("allies", false, 1);
        assert_eq!(
            bot.observe(SID, &[(1, "v g_scriptMainMenu \"team_allies\"")], &all),
            Vec::<String>::new()
        );
        assert_eq!(
            bot.observe(SID, &[(2, "t 0")], &all),
            vec![format!("mr {SID} 0 allies")]
        );
        assert_eq!(bot.observe(SID, &[(3, "t 0")], &all), Vec::<String>::new());
    }

    /// A UO bot answers from UO's menus, the light machine guns among them,
    /// and a two-nation menu offers both nations' weapons.
    #[test]
    fn a_uo_bot_picks_from_uos_menus() {
        let mut seen = std::collections::HashSet::new();
        for seed in 1..200 {
            let mut bot = Bot::new("allies", true, seed);
            bot.menus = &WEAPON_MENUS_UO;
            for menu in ["weapon_russian", "weapon_german", "weapon_americangerman"] {
                bot.main_menu = menu.into();
                bot.loadout = None;
                seen.insert(bot.menu_reply(&all).expect("a reply"));
            }
        }
        for w in [
            "svt40_mp",
            "gewehr43_mp",
            "m1garand_mp",
            "kar98k_mp",
            "mg30cal_mp",
            "mg34_mp",
            "dp28_mp",
        ] {
            assert!(seen.contains(w), "{w} never picked: {seen:?}");
        }
    }

    #[test]
    fn a_weapon_menu_reply_is_one_the_nationality_offers_and_the_cvars_allow() {
        for (nation, weapons) in WEAPON_MENUS {
            let mut seen = Vec::new();
            for seed in 1..64 {
                let mut bot = Bot::new("axis", false, seed);
                let menu = format!("v g_scriptMainMenu weapon_{nation}");
                bot.observe(SID, &[(1, menu.as_str())], &all);
                let reply = bot.observe(SID, &[(2, "t 1")], &all);
                let w = reply[0]
                    .strip_prefix(&format!("mr {SID} 1 "))
                    .unwrap()
                    .to_string();
                assert!(weapons.iter().any(|(n, _)| *n == w), "{nation}: {w}");
                seen.push(w);
            }
            for (w, _) in weapons {
                assert!(seen.iter().any(|s| s == w), "{nation}: {w} never picked");
            }
        }
        // A restricted weapon is never picked; nothing left means no reply.
        let only_kar = |c: &str| c == "scr_allow_kar98k";
        for seed in 1..32 {
            let mut bot = Bot::new("axis", false, seed);
            bot.observe(SID, &[(1, "v g_scriptMainMenu weapon_german")], &all);
            assert_eq!(
                bot.observe(SID, &[(2, "t 1")], &only_kar),
                vec![format!("mr {SID} 1 kar98k_mp")]
            );
        }
        let mut bot = Bot::new("allies", false, 1);
        bot.observe(SID, &[(1, "v g_scriptMainMenu weapon_russian")], &all);
        assert!(bot.observe(SID, &[(2, "t 1")], &|_: &str| false).is_empty());
    }

    /// `switchToWeapon`'s `a <n>` moves the bot's byte, as a cgame's
    /// selection, until `ps.weapon` reads it: the pickup reads only the cmd.
    #[test]
    fn an_a_command_selects_the_weapon_the_cmds_ask_for() {
        let (mut bot, mut v) = engaged(21);
        v.pistol = Some(2);
        v.weapons_held |= 1 << 2;
        v.loaded |= 1 << 2;
        assert_eq!(bot.think(&v).weapon, 10);
        assert!(bot.observe(1, &[(5, "a 2")], &|_| true).is_empty());
        assert_eq!(bot.think(&v).weapon, 2, "mid-drop");
        assert_eq!(bot.think(&v).weapon, 2, "still mid-drop");
        v.weapon = 2;
        assert_eq!(bot.think(&v).weapon, 2, "landed");
    }
}
