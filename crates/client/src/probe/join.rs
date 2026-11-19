//! The stock team and weapon menu handshake every capture that spawns goes through.

use super::*;

/// The team a capture joins when `--probe-team` names none; the stage gates
/// were all taken with a client joining allies through the real menu.
const DEFAULT_JOIN_TEAM: &str = "allies";

/// How long after the weapon answer the capture is taken, so the spawn has
/// landed and the drop to the floor has finished.
pub(super) const SPAWN_SETTLE: Duration = Duration::from_secs(3);

/// Drives the stock team/weapon menu handshake under `--save-playerstate` and
/// keeps what the fixture header needs.
pub(super) struct JoinProbe {
    /// What the team menu is answered with: `allies`, `axis`, `autoassign`
    /// or `spectator`, the four the stock gametypes accept.
    pub(super) team: String,
    /// The menu retail last named in `v g_scriptMainMenu`; the `t` that follows
    /// opens it.
    main_menu: String,
    /// Menu indices already answered, so a reopened menu is not answered twice.
    pub(super) answered: Vec<i32>,
    /// What the weapon menu is answered with when `--probe-weapon` names one,
    /// instead of the nationality's default rifle. The stock menu accepts any
    /// weapon `_teams::restrict` allows for it, which under the default
    /// `scr_allow_*` cvars includes the two scoped rifles.
    weapon_pick: Option<String>,
    pub(super) weapon: String,
    /// When the weapon answer went out.
    answered_weapon: Option<Instant>,
    /// Every serverCommand since the gamestate, verbatim and in order.
    pub(super) commands: Vec<String>,
}

impl JoinProbe {
    pub(super) fn new(team: Option<&str>, weapon: Option<&str>) -> Self {
        Self {
            team: team.unwrap_or(DEFAULT_JOIN_TEAM).to_string(),
            main_menu: String::new(),
            answered: Vec::new(),
            weapon_pick: weapon.map(str::to_string),
            weapon: String::new(),
            answered_weapon: None,
            commands: Vec::new(),
        }
    }

    /// `v g_scriptMainMenu <menu>` names the menu, `t <index>` opens it, and
    /// `mr <serverId> <index> <response>` answers the index `t` named
    /// (docs/research/cod11-hud-protocol.md, section 0.1).
    pub(super) fn on_server_command(
        &mut self,
        tokens: &[String],
        client: &mut NetClient<UdpTransport>,
        now: Instant,
    ) {
        match tokens.first().map(String::as_str) {
            Some("v") if tokens.get(1).map(String::as_str) == Some("g_scriptMainMenu") => {
                self.main_menu = tokens.get(2).cloned().unwrap_or_default();
            }
            Some("t") => {
                let Some(idx) = tokens.get(1).and_then(|t| t.parse::<i32>().ok()) else {
                    return;
                };
                if self.answered.contains(&idx) {
                    return;
                }
                // `t` names its menu by script-menu index; 1.5 opens
                // `serverinfo_<gametype>` there before `g_scriptMainMenu`.
                let first = client.protocol().cs.bounds(net::protocol::CsRange::Menu).0;
                let menu = usize::try_from(idx)
                    .ok()
                    .map(|i| client.configstring(first + i))
                    .filter(|m| !m.is_empty())
                    .unwrap_or(&self.main_menu)
                    .to_string();
                let Some(reply) = menu_reply(&menu, &self.team, self.weapon_pick.as_deref()) else {
                    println!("JOIN: menu {idx} ({menu:?}) has no scripted reply");
                    return;
                };
                println!("JOIN: answering menu {idx} ({menu}) with {reply}");
                client.send_reliable(&format!("mr {} {idx} {reply}", client.server_id()));
                self.answered.push(idx);
                if menu.starts_with("weapon_") {
                    self.weapon = reply.to_string();
                    self.answered_weapon = Some(now);
                }
            }
            _ => {}
        }
    }

    /// A reopened menu carries the indices the last one used, so a probe that
    /// keeps them never answers it and never spawns again. Clearing them is
    /// all a map restart needs: the retail dm capture reoffers the team menu
    /// under index 0 after one, while sd, whose `pers[]` survives, reoffers
    /// no menu at all and this is inert.
    pub(super) fn reopen_menus(&mut self) {
        self.main_menu.clear();
        self.answered.clear();
    }

    /// A new gamestate reruns retail's `ClientConnect` as well, and also puts
    /// the client back on the menus, so the weapon answer is no longer a
    /// spawn this run has settled on.
    pub(super) fn rearm(&mut self) {
        self.reopen_menus();
        self.answered_weapon = None;
        self.commands.clear();
    }

    pub(super) fn settled(&self, now: Instant) -> bool {
        self.answered_weapon
            .is_some_and(|t| now.duration_since(t) >= SPAWN_SETTLE)
    }
}

/// The team menu takes a team; the weapon menu takes a weapon `_teams::restrict`
/// allows for that menu's nationality, which is why one weapon literal cannot
/// serve both gate maps. All four are on under the stock `scr_allow_*` defaults.
/// `weapon` overrides the default rifle with whatever `--probe-weapon` named;
/// the server refuses one the menu's nationality does not allow and reopens
/// the menu, which the probe never answers twice, so a wrong pick never spawns.
fn menu_reply<'a>(menu: &str, team: &'a str, weapon: Option<&'a str>) -> Option<&'a str> {
    if menu.starts_with("team_") {
        return Some(team);
    }
    // 1.5's stock gametypes open `serverinfo_<gametype>` first and the team
    // menu once it answers "close" (docs/research/cod15-protocol-diff.md).
    if menu.starts_with("serverinfo_") {
        return Some("close");
    }
    if let Some(w) = weapon {
        return menu.starts_with("weapon_").then_some(w);
    }
    match menu.strip_prefix("weapon_")? {
        "american" => Some("m1carbine_mp"),
        "british" => Some("enfield_mp"),
        "russian" => Some("mosin_nagant_mp"),
        "german" => Some("kar98k_mp"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A map change and a dm map restart both rerun retail's `ClientConnect`
    /// and offer the team menu again, under the indices the last one used.
    /// Without the re-arm the probe skips them and never spawns again; a join
    /// left "settled" across a gamestate would have the capture reading the
    /// new map's menu as a spawn.
    #[test]
    fn a_restart_and_a_gamestate_reopen_the_menus() {
        let t0 = Instant::now();
        let mut join = JoinProbe::new(Some("allies"), None);
        join.main_menu = "weapon_american".to_string();
        join.answered = vec![0, 1];
        join.weapon = "m1carbine_mp".to_string();
        join.answered_weapon = Some(t0);
        assert!(join.settled(t0 + SPAWN_SETTLE));

        // A map restart reopens the menu without unspawning the run: the sd
        // pair crosses four of them and its script must not pause.
        join.reopen_menus();
        assert!(join.answered.is_empty());
        assert!(join.main_menu.is_empty());
        assert!(join.settled(t0 + SPAWN_SETTLE));

        join.answered = vec![0, 1];
        join.answered_weapon = Some(t0);
        join.rearm();
        assert!(join.answered.is_empty());
        assert!(!join.settled(t0 + SPAWN_SETTLE));
        // The team is the run's, not the map's: it survives.
        assert_eq!(join.team, "allies");
    }
}
