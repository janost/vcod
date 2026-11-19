//! The bot state CoD2's and CoD4's servers keep alike: the driver, the
//! admin's ledger and the cvars last applied, beside each game's own part
//! ([`GameBots::own`]). The server half over it is `engine/bots.rs`.

use std::collections::BTreeMap;
use std::sync::Arc;

use super::Bot;
use super::driver::BotDriver;
use crate::botmgmt::Settings;
use crate::nav::NavGraph;
use crate::server::bot_admin::Ledger;

/// A game's own bot state beside the shared part of [`GameBots`].
pub trait OwnBots {
    /// The state of a match with no bot yet.
    fn new() -> Self;
    /// A new brain as this game dresses it.
    fn dress(_bot: &mut Bot) {}
    /// A new level: what the game carries over and what starts again;
    /// `same_map` keeps what rests on the world.
    fn new_level(&mut self, same_map: bool);
    /// Drops what this game keeps for `slot`'s bot.
    fn forget(&mut self, slot: usize);
}

/// The bots of a CoD2 or CoD4 server.
pub struct GameBots<G> {
    /// The brains, the graph and what the shared driver keeps per bot.
    pub(crate) driver: BotDriver,
    shoot: bool,
    /// The management's state (`crate::server::bot_admin`), shared with
    /// CoD1's.
    pub admin: Ledger,
    /// The `vc_bot_shoot` and `vc_bot_skill` values last applied.
    pub applied: Option<(bool, u8)>,
    /// What the game keeps beside.
    pub own: G,
}

impl<G: OwnBots> GameBots<G> {
    pub fn new(shoot: bool) -> Self {
        GameBots {
            driver: BotDriver {
                nav_background: true,
                ..Default::default()
            },
            shoot,
            admin: Default::default(),
            applied: None,
            own: G::new(),
        }
    }

    /// A brain for the client in `slot`, answering the team menu with
    /// `team` (`autoassign`, `allies` or `axis`).
    pub fn add(&mut self, slot: usize, name: &str, team: &str, seed: u64) {
        let mut bot = Bot::new(team, self.shoot, seed);
        bot.name = name.to_string();
        G::dress(&mut bot);
        self.driver.bots.insert(slot, bot);
    }

    /// A new level under the bots (`map`, `map_restart`, `fast_restart`,
    /// `map_rotate`): every brain forgets the menus it answered, as the
    /// level reopens them under the same indices (CoD1's `Bot::rearm` on a
    /// gamestate), and what the old level left goes
    /// ([`BotDriver::new_level`]); a new map drops the graph too, for the
    /// next tick to build the new map's.
    pub fn new_level(&mut self, same_map: bool) {
        self.driver.new_level(same_map);
        for bot in self.driver.bots.values_mut() {
            bot.rearm();
        }
        self.own.new_level(same_map);
    }

    /// Drops the bot in `slot`: its brain, its pins and what the driver
    /// and the game keep for it.
    pub fn remove(&mut self, slot: usize) {
        self.admin.pinned.remove(&slot);
        self.admin.skills.remove(&slot);
        self.driver.forget(slot);
        self.own.forget(slot);
    }
}

impl<G> GameBots<G> {
    /// Whether new bots fight, and `vc_bot_shoot`'s change for every bot.
    pub fn set_shoot(&mut self, shoot: bool) {
        self.shoot = shoot;
        for b in self.driver.bots.values_mut() {
            b.set_shoot(shoot);
        }
    }

    pub fn shoots(&self) -> bool {
        self.shoot
    }

    /// The brains, by slot.
    pub fn bots(&self) -> &BTreeMap<usize, Bot> {
        &self.driver.bots
    }

    pub fn bots_mut(&mut self) -> &mut BTreeMap<usize, Bot> {
        &mut self.driver.bots
    }

    /// `bot_match`'s cue counts (`Server::bot_cue_tally`).
    pub fn cue_tally(&self) -> [(u32, u32, u64); 2] {
        self.driver.posts.cue_tally()
    }

    /// Block the tick on the graph's build rather than let the bots wander
    /// until it lands: the in-process measuring runs set it.
    pub fn wait_for_graph(&mut self) {
        self.driver.nav_background = false;
    }

    /// The graph once its build has landed.
    pub fn graph(&self) -> Option<&Arc<NavGraph>> {
        self.driver.nav.as_ref()
    }

    /// `vc_bot_retreat` every pass; `vc_bot_shoot` and `vc_bot_skill` once
    /// they move from the values first seen, so `--bots-peaceful` and the
    /// cvar's default stand until then. A pinned level stays.
    pub(crate) fn apply_settings(&mut self, s: Settings) {
        for bot in self.driver.bots.values_mut() {
            bot.allow_retreat(s.retreat);
        }
        match self.applied {
            None => self.applied = Some((s.shoot, s.skill)),
            Some((shoot, skill)) => {
                if shoot != s.shoot {
                    self.set_shoot(s.shoot);
                }
                if skill != s.skill {
                    for (slot, bot) in self.driver.bots.iter_mut() {
                        if !self.admin.skills.contains_key(slot) {
                            bot.skill = crate::bots::Skill::level(s.skill);
                        }
                    }
                }
                self.applied = Some((s.shoot, s.skill));
            }
        }
    }
}
