//! The bot glue CoD2's and CoD4's servers share over their [`Conn`] slots:
//! adding a bot, the reliable commands a bot reads, and the shared bot
//! admin's hooks (`server/bot_admin.rs`'s [`AdminHost`]). A [`BotGame`]
//! hook is what the two games do differently. The bots' state is
//! [`GameBots`]; CoD1's server keeps its own admin.

use std::collections::BTreeMap;

use super::Engine;
use super::conn::ConnGame;
use super::slot::{Conn, State};
use crate::botmgmt::{self, Member, Settings, Side, cvar};
use crate::bots::Bot;
use crate::bots::driver::BotWorld;
use crate::bots::game_bots::{GameBots, OwnBots};
use crate::rcon::Redirect;
use crate::server::bot_admin::{AdminHost, Ledger, roster_side};

/// What CoD2's and CoD4's bot glue does differently.
pub trait BotGame: ConnGame {
    /// What the game's bots keep beside [`GameBots`]' shared part.
    type Own: OwnBots;
    /// The game's log prefix.
    const TAG: &'static str;
    fn bots(&self) -> Option<&GameBots<Self::Own>>;
    fn bots_slot(&mut self) -> &mut Option<GameBots<Self::Own>>;
    fn level_running(&self) -> bool;
    fn gametype(&self) -> &str;
    /// Connects a bot client in `slot` with `userinfo` (the game's
    /// `ClientConnect`), its slot marked a bot to the engine and the level.
    fn admit_bot(e: &mut Engine<Self>, slot: usize, userinfo: &str);
    /// The team menu's index as an `mr` names it.
    fn team_menu(e: &Engine<Self>) -> Option<usize>;
}

impl<G: BotGame> Engine<G> {
    /// `count` bots, each in a free slot; `shoot` lets them fight. A team
    /// game alternates them between `allies` and `axis`, `dm` autoassigns.
    /// Returns the slots taken; they count toward the bots the management
    /// pass keeps.
    pub fn add_bots(&mut self, count: usize, shoot: bool) -> Vec<usize> {
        let mut taken = Vec::new();
        for _ in 0..count {
            let side = match self.game.gametype() {
                "dm" => Side::None,
                _ if self.brain_count().is_multiple_of(2) => Side::Allies,
                _ => Side::Axis,
            };
            let Some(slot) = self.add_bot(side, shoot, None) else {
                break;
            };
            taken.push(slot);
        }
        if let Some(b) = self.game.bots_slot() {
            b.admin.manual = b.admin.manual.max(b.bots().len());
        }
        taken
    }

    /// One bot in the first free public slot asking for `side` (in a team
    /// game `autoassign` takes allies, as CoD1's), at skill `level` or
    /// `vc_bot_skill`'s.
    pub(crate) fn add_bot(&mut self, side: Side, shoot: bool, level: Option<u8>) -> Option<usize> {
        let Some(slot) = self
            .public_slots()
            .find(|&i| self.core.clients[i].is_none())
        else {
            log::warn!("{}: no free slot for a bot", G::TAG);
            return None;
        };
        let name = if G::cvar(self, cvar::NAMES) == "0" {
            format!("bot{}", self.brain_count())
        } else {
            // CoD1's pool and draw (`botmgmt::draw_name`), off the level's rand.
            let taken: Vec<String> = self
                .game
                .bots()
                .map(|b| b.bots().values().map(|b| b.name.clone()).collect())
                .unwrap_or_default();
            botmgmt::draw_name(&taken, &mut |k| self.rand() as usize % k)
        };
        let team = match (side, self.game.gametype()) {
            (Side::None, "dm") => "autoassign",
            (Side::Axis, _) => "axis",
            _ => "allies",
        };
        G::admit_bot(
            self,
            slot,
            &format!("\\name\\{name}\\rate\\25000\\snaps\\20"),
        );
        let seed = self.rand() as u64;
        let skill = self.skill_cvar();
        let bots = self
            .game
            .bots_slot()
            .get_or_insert_with(|| GameBots::new(shoot));
        bots.add(slot, &name, team, seed);
        if let Some(b) = bots.bots_mut().get_mut(&slot) {
            b.skill = crate::bots::Skill::level(level.unwrap_or(skill));
        }
        if let Some(l) = level {
            bots.admin.skills.insert(slot, l);
        }
        log::info!("{}: bot {name} in slot {slot}", G::TAG);
        Some(slot)
    }

    /// How many brains the bots have now.
    fn brain_count(&self) -> usize {
        self.game.bots().map_or(0, |b| b.bots().len())
    }

    fn skill_cvar(&self) -> u8 {
        crate::game::builtins::cvar::atoi(G::cvar(self, cvar::SKILL))
            .clamp(0, i32::from(crate::bots::MAX_LEVEL)) as u8
    }

    /// The slots past `sv_privateClients`.
    fn public_slots(&self) -> std::ops::Range<usize> {
        let n = self.core.clients.len();
        (self.int("sv_privateClients").max(0) as usize).min(n)..n
    }
}

/// Every reliable command sent to a bot's slot since it last read, read
/// now: a bot's are never written to a message, so read is sent too, and
/// `SV_AddServerCommand` squashes only commands not yet sent.
pub(crate) fn read_reliable<F, X>(c: &mut Conn<F, X>) -> Vec<String> {
    let seq = c.link.netchan.reliable_sequence as i32;
    let mask = c.link.netchan.reliable.len() - 1;
    let texts = (c.link.reliable_ack + 1..=seq)
        .map(|s| c.link.netchan.reliable[s as usize & mask].clone())
        .collect();
    c.link.reliable_ack = seq;
    c.link.reliable_sent = seq;
    texts
}

/// The CoD2 and CoD4 servers' hooks for the shared bot admin.
impl<G: BotGame> AdminHost for Engine<G>
where
    Engine<G>: BotWorld,
{
    type Out = Redirect;

    fn say(&mut self, out: &mut Redirect, msg: &str) {
        out.print(msg);
    }

    fn level_running(&self) -> bool {
        self.game.level_running()
    }

    /// No bot yet: only a floor brings the bots' state in.
    fn admin_ready(&mut self) -> bool {
        if !self.game.level_running() {
            return false;
        }
        if self.game.bots().is_none() {
            let s = self.bot_settings();
            if s.min_players == 0 {
                return false;
            }
            *self.game.bots_slot() = Some(GameBots::new(s.shoot));
        }
        true
    }

    fn admin_now(&self) -> i32 {
        self.core.sv_time_ms
    }

    fn ledger(&mut self) -> Option<&mut Ledger> {
        self.game.bots_slot().as_mut().map(|b| &mut b.admin)
    }

    fn ledger_or_new(&mut self) -> &mut Ledger {
        let shoot = self
            .game
            .bots()
            .map_or_else(|| self.bot_settings().shoot, |b| b.shoots());
        &mut self
            .game
            .bots_slot()
            .get_or_insert_with(|| GameBots::new(shoot))
            .admin
    }

    fn bot_settings(&self) -> Settings {
        let on = |name: &str| crate::game::builtins::cvar::atoi(G::cvar(self, name)) != 0;
        Settings {
            min_players: self.int(cvar::MIN_PLAYERS).max(0) as usize,
            autobalance: on(cvar::AUTOBALANCE),
            shoot: on(cvar::SHOOT),
            names: on(cvar::NAMES),
            skill: self.skill_cvar(),
            retreat: on(cvar::RETREAT),
        }
    }

    fn apply_settings(&mut self, s: Settings) {
        if let Some(b) = self.game.bots_slot() {
            b.apply_settings(s);
        }
    }

    fn bot_teamed(&self) -> bool {
        self.game.gametype() != "dm"
    }

    fn brains(&self) -> Option<&BTreeMap<usize, Bot>> {
        self.game.bots().map(|b| b.bots())
    }

    /// A zombie is a client state here, so `clients` alone says it.
    fn free_slots(&self) -> usize {
        self.public_slots()
            .filter(|&i| self.core.clients[i].is_none())
            .count()
    }

    /// Zombies are left out.
    fn bot_roster(&mut self) -> Vec<Member> {
        let mut out = Vec::new();
        for slot in 0..self.core.clients.len() {
            if self.core.clients[slot]
                .as_ref()
                .is_none_or(|c| c.state == State::Zombie)
            {
                continue;
            }
            let (team, state, score) = match G::script(self) {
                Some(rt) => (
                    rt.client_pers(slot, "team").unwrap_or_default(),
                    rt.client_field(slot, "sessionstate").unwrap_or_default(),
                    rt.client_field(slot, "score").unwrap_or_default(),
                ),
                None => Default::default(),
            };
            let bots = self.game.bots();
            let bot = bots.and_then(|b| b.bots().get(&slot));
            let joining = !matches!(state.as_str(), "playing" | "dead");
            out.push(Member {
                slot,
                bot: bot.is_some(),
                side: roster_side(&team, bot.map(|b| b.team()), joining),
                score: score.parse().unwrap_or(0),
                playing: !matches!(state.as_str(), "dead" | "spectator"),
                pinned: bots.is_some_and(|b| b.admin.pinned.contains(&slot)),
            });
        }
        out
    }

    fn admit_bot(&mut self, side: Side, _: Settings, level: Option<u8>) -> Option<usize> {
        let shoot = self.game.bots().is_none_or(|b| b.shoots());
        self.add_bot(side, shoot, level)
    }

    /// The drop forgets a bot at once on CoD2, at its zombie's turn on
    /// CoD4; either way it goes here.
    fn remove_bot(&mut self, slot: usize) {
        G::drop_client(self, slot, "EXE_PLAYERKICKED");
        if let Some(b) = self.game.bots_slot() {
            b.remove(slot);
        }
    }

    /// The team menu's answer from the bot, as its button sends it; the
    /// stock handlers switch a dead or spectating player and reopen the
    /// weapon or class menu, which the bot answers again. The reply runs
    /// at once.
    fn move_bot(&mut self, slot: usize, side: Side) {
        let Some(bot) = self
            .game
            .bots_slot()
            .as_mut()
            .and_then(|b| b.bots_mut().get_mut(&slot))
        else {
            return;
        };
        bot.set_team(side.response());
        log::info!(
            "{}: bot {slot} {:?} moves to {}",
            G::TAG,
            bot.name,
            side.response()
        );
        if let Some(i) = G::team_menu(self) {
            let reply = format!("mr {} {i} {}", self.server_id(), side.response());
            self.bot_command(slot, reply);
        }
    }
}
