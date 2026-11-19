//! The engine half of CoD2 bots (`crate::bots::cod2` holds the brains):
//! a bot is a client with no address that enters the world on the frame
//! after its `ClientConnect`, reads its reliable server commands straight
//! off the ring, answers them through [`Cod2Server::client_command`] and
//! moves by its cmds through the level's `ClientThink`, once per frame
//! ahead of `G_RunFrame`, as a client's packet between two frames would.
//! `addbot`, `removebot`, `listbots` and the management pass are the
//! shared `server/bot_admin.rs` over `engine/bots.rs`, which CoD4 shares;
//! this file holds what CoD2 does its own way.

use std::collections::BTreeMap;
use std::net::SocketAddr;

use vcod_cod2::net::UserCmd2;
use vcod_common::net::msg::UserCmd;

use super::{Cod2Server, State};
use crate::bots::cod2::{self, Cod2Bots, Cod2Frame, Cod2Own};
use crate::bots::driver::{Body, BotDriver, BotWorld, CoverBody, Joining};
use crate::bots::{BotView, EnemyView, Noise};
use crate::engine::bots::{BotGame, read_reliable};
use crate::nav::{Follower, NavGraph};
use crate::server::bot_admin;
use crate::world::World;

impl Cod2Server {
    /// The bots' pass (lnxded 0x8096952) for `addTestClient`'s clients:
    /// each in slot order draws its random cmd and, active, thinks on it
    /// (`SV_ClientThink` 0x8090dac).
    fn step_test_clients(&mut self) {
        let Some(level) = self.game.level.as_mut() else {
            return;
        };
        for (slot, c) in self.core.clients.iter_mut().enumerate() {
            let Some(c) = c.as_mut().filter(|c| c.extra.test) else {
                continue;
            };
            let Some(cmd) = level.test_client_cmd(slot) else {
                continue;
            };
            if c.state == State::Active {
                level.client_think(slot, &cmd);
            }
        }
    }

    /// The bots, for the in-process measuring runs (`examples/bot_match.rs`'s
    /// CoD2 mode); [`Self::level`] and [`Self::level_mut`] lend the level.
    pub fn bots_mut(&mut self) -> Option<&mut Cod2Bots> {
        self.game.bots.as_mut()
    }

    pub fn level(&self) -> Option<&crate::game::cod2::Cod2Level> {
        self.game.level.as_ref()
    }

    pub fn level_mut(&mut self) -> Option<&mut crate::game::cod2::Cod2Level> {
        self.game.level.as_mut()
    }

    /// One frame of the bots, before `G_RunFrame`: the admin, the test
    /// clients, then the shared driver (`bots::driver::step`): a bot
    /// connected last frame enters the world, then every bot reads its new
    /// server commands, answers them and sends its cmds.
    pub(super) fn step_bots(&mut self) {
        bot_admin::manage(self);
        self.step_test_clients();
        if self.game.bots.is_none() {
            return;
        }
        if self.game.level.is_none() {
            // The bots enter and read, and nothing moves.
            self.enter_bots();
            self.flush_game_commands();
            return;
        }
        // The walks the pass runs (the rest-jump test, the re-proofs) move
        // as the graph's did, as CoD1's pass runs under its server's rules
        // (bot-navigation.md, "Other rules").
        match self.game.bots.as_ref().and_then(|b| b.graph().cloned()) {
            Some(g) => g.moving(|| crate::bots::driver::step(self)),
            None => crate::nav::with_rules(cod2::GRAPH_RULES, || crate::bots::driver::step(self)),
        }
    }

    /// A bot connected last frame enters the world at once (`begin`); every
    /// bot reads all its reliable commands sent and acknowledges them.
    fn enter_bots(&mut self) {
        let Some(bots) = self.game.bots.as_ref() else {
            return;
        };
        let slots: Vec<usize> = bots.bots().keys().copied().collect();
        let mut inbox: BTreeMap<usize, Vec<String>> = BTreeMap::new();
        for &slot in &slots {
            let Some(c) = self.core.clients[slot].as_mut() else {
                continue;
            };
            if c.state == State::Connected {
                c.state = State::Active;
                if let Some(level) = self.game.level.as_mut() {
                    level.begin(slot);
                }
            }
            // The bot reads everything sent; its acknowledge says so.
            inbox.insert(slot, read_reliable(c));
        }
        if let Some(b) = self.game.bots.as_mut() {
            b.inbox(inbox);
        }
    }

    fn cod2_bots(&self) -> &Cod2Bots {
        self.game
            .bots
            .as_ref()
            .expect("the bots' driver runs with bots")
    }

    fn cod2_level(&self) -> Option<&crate::game::cod2::Cod2Level> {
        self.game.level.as_ref()
    }
}

/// CoD2's game under the bots' driver (`bots::driver`), in CoD1's order;
/// its route leaves out what the driver's consts name, and its brains
/// read the CoD2 playerstate and weapons (`bots::cod2`).
impl BotWorld for Cod2Server {
    type Frame = Cod2Frame;
    type Cmd = Vec<UserCmd2>;

    /// Both on, CoD2's tdm bots stalled more for no kill (bot-combat.md 15).
    const YIELDS: bool = false;
    const SPREADS: bool = true;
    /// Off: not measured here; `PARTS_MATES` plans round a still
    /// teammate (bot-navigation.md section 3.8, "Bodies in the way").
    const SHUNS_BODIES: bool = false;
    /// On: two teammates held each other at mp_decoy's (8650, -12570,
    /// -531) for whole matches (bot-objectives.md 11.5).
    const PARTS_MATES: bool = true;
    /// On: on mp_leningrad's ctf a bot hung on the step lip at (-598,
    /// -617, 14) against a carrier holding the capture zone above it,
    /// never on the ground, so never held up (bot-objectives.md 11.6).
    const PARTS_MATES_ALOFT: bool = true;
    const PMOVE_PROBES: bool = false;
    /// On: mp_brecourt's mantle from (-528, -816, 1) lands 192 across, and
    /// the follower dropped the path at its foot every tick
    /// (bot-objectives.md 11.4).
    const LONG_MANTLES: bool = true;
    /// On: mp_brecourt's jump edge from (-1440, -570, 28) runs 190
    /// across, and a bot passing its foot 3 short read the top as off
    /// its path and stood there the rest of an `hq` match
    /// (bot-objectives.md 11.7).
    const FOOT_SLACK: bool = true;

    fn driver(&self) -> &BotDriver {
        &self.cod2_bots().driver
    }

    fn driver_mut(&mut self) -> &mut BotDriver {
        &mut self
            .game
            .bots
            .as_mut()
            .expect("the bots' driver runs with bots")
            .driver
    }

    fn now(&self) -> i32 {
        self.cod2_level().map_or(0, |l| l.level_time())
    }

    fn server_id(&self) -> i32 {
        self.game.server_id
    }

    fn world(&self) -> Option<&World> {
        self.cod2_level().map(|l| &*l.world)
    }

    fn connected(&self, slot: usize) -> bool {
        self.bot_connected(slot)
    }

    fn take_noises(&mut self) -> Vec<Noise> {
        match (self.game.bots.as_mut(), self.game.level.as_ref()) {
            (Some(b), Some(level)) => b.listen(level),
            _ => Vec::new(),
        }
    }

    /// The admin pass ran ahead of the driver (`step_bots`): here the bots
    /// enter and read their commands.
    fn manage(&mut self) {
        self.enter_bots();
    }

    /// No bot: the pass still stamps its packet and sends what the frame
    /// queued.
    fn idle(&mut self) {
        if let Some(level) = self.game.level.as_mut() {
            level.packet();
        }
        self.flush_game_commands();
    }

    fn nav_tick(&mut self) {
        if let (Some(b), Some(level)) = (self.game.bots.as_mut(), self.game.level.as_ref()) {
            b.nav_tick(level, &self.game.cfg.map);
        }
    }

    fn teams(&mut self) -> Vec<i32> {
        let Some(level) = self.game.level.as_mut() else {
            return Vec::new();
        };
        (0..level.players.len())
            .map(|s| level.rt.client_team(s))
            .collect()
    }

    /// CoD2 begins a bot as it connects ([`Self::manage`]) and its commands
    /// were acknowledged as read.
    fn read_commands(
        &mut self,
        slot: usize,
        sid: i32,
        replies: &mut Vec<(usize, String)>,
    ) -> Joining {
        if let (Some(b), Some(level)) = (self.game.bots.as_mut(), self.game.level.as_ref()) {
            b.read_commands(level, slot, &self.core.configstrings, sid, replies);
        }
        Joining::No
    }

    fn frame(&mut self) -> Cod2Frame {
        let (b, level) = (
            self.game
                .bots
                .as_mut()
                .expect("the bots' driver runs with bots"),
            self.game
                .level
                .as_mut()
                .expect("the bots' driver runs on a level"),
        );
        b.frame(level)
    }

    fn feel(&mut self, slot: usize) {
        if let (Some(b), Some(level)) = (self.game.bots.as_mut(), self.game.level.as_ref()) {
            b.feel(level, slot);
        }
    }

    fn body(&self, slot: usize) -> Option<Body> {
        cod2::body(self.cod2_level()?, slot)
    }

    fn enemy(&self, slot: usize, teams: &[i32]) -> Option<EnemyView> {
        self.cod2_bots().sight(self.cod2_level()?, slot, teams)
    }

    fn view(&self, slot: usize, enemy: Option<EnemyView>) -> Option<BotView> {
        self.cod2_bots().felt_view(self.cod2_level()?, slot, enemy)
    }

    fn cover_body(&self, slot: usize, threat: usize) -> Option<CoverBody> {
        cod2::cover_body(self.cod2_level()?, slot, threat)
    }

    fn deal(
        &mut self,
        frame: Cod2Frame,
        teams: &[i32],
        _solo: &[usize],
        round: u32,
        views: &mut Vec<(usize, BotView)>,
    ) {
        if let (Some(b), Some(level)) = (self.game.bots.as_mut(), self.game.level.as_ref()) {
            b.deal(level, frame, teams, round, views);
        }
    }

    fn pre_route(&mut self, slot: usize, view: &mut BotView, teams: &[i32]) {
        if let (Some(b), Some(level)) = (self.game.bots.as_mut(), self.game.level.as_ref()) {
            b.pre_route(level, slot, view, teams);
        }
    }

    fn routed(&mut self, slot: usize, follower: &Follower, g: &NavGraph) {
        if let Some(b) = self.game.bots.as_mut() {
            b.routed(slot, follower, g);
        }
    }

    fn pre_think(&mut self, slot: usize, view: &mut BotView, teams: &[i32]) {
        if let (Some(b), Some(level)) = (self.game.bots.as_mut(), self.game.level.as_ref()) {
            b.pre_think(level, slot, view, teams);
        }
    }

    fn keeps_alive(&mut self, slot: usize) -> bool {
        self.cod2_bots().keeps_alive(slot)
    }

    fn cmd(&mut self, slot: usize, cmd: UserCmd, view: &BotView) -> Option<Vec<UserCmd2>> {
        let (b, level) = (self.game.bots.as_mut()?, self.game.level.as_ref()?);
        b.cmd(level, slot, cmd, view)
    }

    fn move_ps(&self, slot: usize) -> Option<vcod_common::pmove::PlayerState> {
        cod2::move_ps(self.cod2_level()?, slot)
    }

    fn lob_probe(
        &self,
        slot: usize,
        weapon: u8,
        target: [f32; 3],
        teams: &[i32],
    ) -> Option<[f32; 2]> {
        cod2::lob_probe(self.cod2_level()?, slot, weapon, target, teams)
    }

    fn bot_command(&mut self, slot: usize, text: String) {
        let Some(seq) = self.core.clients[slot]
            .as_ref()
            .map(|c| c.link.last_client_command + 1)
        else {
            return;
        };
        self.client_command(slot, seq, text);
    }

    /// Off the table `openMenu` precached into, as it looks menus up.
    fn menu_index(&self, name: &str) -> Option<usize> {
        let host = &self.cod2_level()?.rt.host;
        crate::configstrings::script_menu_index(&host.configstrings, host.allocators.layout(), name)
    }

    fn killer(&self, slot: usize) -> Option<usize> {
        let level = self.cod2_level()?;
        let (a, at) = level.players.get(slot)?.as_ref()?.last_attacker?;
        (0..1000)
            .contains(&level.level_time().wrapping_sub(at))
            .then_some(a)
    }

    /// The bots' pass resets the skeleton cache once for all of them
    /// (lnxded 0x8096952), as a packet does.
    fn bot_packet(&mut self) {
        if let Some(level) = self.game.level.as_mut() {
            level.packet();
        }
    }

    /// Each cmd past the client's last runs through the level's
    /// `ClientThink`.
    fn send_cmd(&mut self, slot: usize, cmds: Vec<UserCmd2>, _: bool) {
        for cmd in cmds {
            let Some(c) = self.core.clients[slot].as_mut() else {
                break;
            };
            if cmd.server_time <= c.last_cmd_time {
                continue;
            }
            c.last_cmd_time = cmd.server_time;
            if let Some(level) = self.game.level.as_mut() {
                level.client_think(slot, &cmd);
            }
        }
    }

    fn tick_done(&mut self) {
        if let Some(b) = self.game.bots.as_mut() {
            b.tick_done();
        }
        self.flush_game_commands();
    }
}

/// What CoD2's bot glue does its own way (`engine/bots.rs`).
impl BotGame for super::Cod2Game {
    type Own = Cod2Own;
    const TAG: &'static str = "cod2";

    fn bots(&self) -> Option<&Cod2Bots> {
        self.bots.as_ref()
    }

    fn bots_slot(&mut self) -> &mut Option<Cod2Bots> {
        &mut self.bots
    }

    fn level_running(&self) -> bool {
        self.level.is_some()
    }

    fn gametype(&self) -> &str {
        &self.cfg.gametype
    }

    fn admit_bot(e: &mut Cod2Server, slot: usize, userinfo: &str) {
        e.admit(slot, SocketAddr::from(([0, 0, 0, 0], 0)), 0, 0, userinfo);
        if let Some(c) = e.core.clients[slot].as_mut() {
            c.bot = true;
        }
        if let Some(level) = e.game.level.as_mut() {
            level.mark_bot(slot);
        }
    }

    /// The menu block's slots short of its last, as CoD2's bots have
    /// always read it (CoD4's read the last too).
    fn team_menu(e: &Cod2Server) -> Option<usize> {
        use vcod_common::net::protocol::CsRange;
        let (first, last) = vcod_cod2::net::protocol::CS_LAYOUT_118.bounds(CsRange::Menu);
        (first..last)
            .find(|&i| {
                e.core
                    .configstrings
                    .get(i)
                    .is_some_and(|m| m.starts_with("team_"))
            })
            .map(|i| i - first)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rcon::Redirect;
    use vcod_cod2::net::ops::MAX_RELIABLE_COMMANDS;

    fn bots(sv: &Cod2Server) -> usize {
        sv.game.bots.as_ref().map_or(0, |b| b.bots().len())
    }

    /// `addbot`, `removebot`, `listbots` and `vc_bot_minplayers` on the
    /// CoD2 server, as on CoD1's (`server/bot_admin.rs`). Needs `COD2_DIR`.
    #[test]
    fn rcon_adds_lists_and_removes_cod2_bots() {
        let Some(fs) = vcod_common::testing::cod2_fs() else {
            return;
        };
        let now = std::time::Instant::now();
        let mut sv = Cod2Server::new(
            super::super::Cod2Config {
                map: "mp_toujane".into(),
                gametype: "tdm".into(),
                hostname: "bots".into(),
                max_clients: 8,
                gametype_script: None,
            },
            3,
            now,
        );
        sv.load(Some(std::rc::Rc::new(fs))).unwrap();
        let run = |sv: &mut Cod2Server, line: &str| {
            let mut out = Redirect::default();
            sv.exec(line, &mut out);
            out.finish().concat()
        };
        let said = run(&mut sv, "addbot axis 2 skill 3");
        assert!(said.contains("added 2 bot(s) to axis"), "{said}");
        assert_eq!(bots(&sv), 2);
        let b = sv.game.bots.as_ref().unwrap();
        assert_eq!(b.admin.pinned.len(), 2);
        assert!(
            b.bots()
                .values()
                .all(|b| b.team() == "axis" && b.skill.level_of() == Some(3))
        );
        let list = run(&mut sv, "listbots");
        assert!(list.contains("2 bot(s)"), "{list}");
        // A floor of 4 tops the bots up on the next pass.
        run(&mut sv, "vc_bot_minplayers 4");
        sv.tick_frames(now + std::time::Duration::from_millis(50), 1);
        assert_eq!(bots(&sv), 4);
        run(&mut sv, "vc_bot_minplayers 0");
        let said = run(&mut sv, "kickbot all");
        assert_eq!(said.matches("removed").count(), 4, "{said}");
        sv.tick_frames(now + std::time::Duration::from_millis(100), 1);
        assert_eq!(bots(&sv), 0);
    }

    /// A dropped bot's zombie slot holds no bot: the driver forgets it as
    /// CoD4's does. Needs `COD2_DIR`.
    #[test]
    fn a_zombie_slot_is_no_bot_s() {
        let Some(fs) = vcod_common::testing::cod2_fs() else {
            return;
        };
        let now = std::time::Instant::now();
        let mut sv = Cod2Server::new(
            super::super::Cod2Config {
                map: "mp_toujane".into(),
                gametype: "dm".into(),
                hostname: "zombie".into(),
                max_clients: 4,
                gametype_script: None,
            },
            3,
            now,
        );
        sv.load(Some(std::rc::Rc::new(fs))).unwrap();
        let slot = sv.add_bots(1, false)[0];
        assert!(BotWorld::connected(&sv, slot));
        sv.core.clients[slot].as_mut().unwrap().state = State::Zombie;
        assert!(!BotWorld::connected(&sv, slot));
    }

    /// vcod's bots never vote, so they stay out of `numVotingClients`: one
    /// playing client among seven bots passes `callvote typemap` on the
    /// caller's own yes. Needs `COD2_DIR`.
    #[test]
    fn bots_do_not_hold_a_cod2_vote() {
        let Some(fs) = vcod_common::testing::cod2_fs() else {
            return;
        };
        let now = std::time::Instant::now();
        let mut sv = Cod2Server::new(
            super::super::Cod2Config {
                map: "mp_toujane".into(),
                gametype: "dm".into(),
                hostname: "votes".into(),
                max_clients: 8,
                gametype_script: None,
            },
            3,
            now,
        );
        sv.load(Some(std::rc::Rc::new(fs))).unwrap();
        let slots = sv.add_bots(8, false);
        assert_eq!(slots.len(), 8);
        let mut t = 0u64;
        let mut frames = |sv: &mut Cod2Server, n: u64| {
            for _ in 0..n {
                t += 50;
                sv.drain_console(now);
                sv.tick_frames(now + std::time::Duration::from_millis(t), 1);
            }
        };
        frames(&mut sv, 60);
        // The first bot loses its brain and its mark: a client that votes.
        let human = slots[0];
        sv.game.bots.as_mut().unwrap().bots_mut().remove(&human);
        let level = sv.game.level.as_mut().unwrap();
        level.rt.host.client_roster[human].as_mut().unwrap().bot = false;
        let from = sv.core.clients[human]
            .as_ref()
            .unwrap()
            .link
            .netchan
            .reliable_sequence;
        let seq = sv.core.clients[human]
            .as_ref()
            .unwrap()
            .link
            .last_client_command
            + 1;
        assert!(sv.client_command(human, seq, "callvote typemap tdm mp_toujane".into()));
        frames(&mut sv, 2);
        let c = sv.core.clients[human].as_ref().unwrap();
        let sent: Vec<&str> = (from + 1..=c.link.netchan.reliable_sequence)
            .map(|s| c.link.netchan.reliable[s as usize & (MAX_RELIABLE_COMMANDS - 1)].as_str())
            .collect();
        assert!(
            sent.iter().any(|l| l.contains("GAME_CALLEDAVOTE")),
            "{sent:?}"
        );
        assert!(
            sent.iter().any(|l| l.contains("GAME_VOTEPASSED")),
            "{sent:?}"
        );
        // The pass loads tdm 3 s later; the bots keep their mark across it.
        frames(&mut sv, 80);
        assert_eq!(sv.game.cfg.gametype, "tdm");
        let level = sv.game.level.as_ref().unwrap();
        assert!(level.rt.host.client_roster[slots[1]].is_some_and(|r| r.bot));
    }
}
