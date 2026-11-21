//! The engine half of CoD4 bots (`crate::bots::cod4` holds what feeds the
//! brains): a bot is a client with no address that enters the world on
//! the frame after its `ClientConnect`, reads its reliable server commands
//! straight off the ring, answers them through [`Cod4Server::client_command`]
//! and moves by its cmds through the level's `ClientThink`, once per frame
//! in `SV_PreFrame`'s bot slot, as a client's packet between two frames
//! would. The management pass (`vc_bot_*`) is the shared
//! `server/bot_admin.rs` over `engine/bots.rs`, which CoD2 shares; this
//! file holds what CoD4 does its own way, as `cod2/bots.rs` holds CoD2's.

use std::collections::BTreeMap;
use std::net::SocketAddr;

use vcod_cod4::net::clmsg::UserCmd as UserCmd4;
use vcod_common::net::msg::UserCmd;
use vcod_common::net::protocol::CsRange;

use super::{Cod4Server, State};
use crate::botmgmt::Side;
use crate::bots::cod4::{self, Cod4Bots, Cod4Own};
use crate::bots::driver::{Body, BotDriver, BotWorld, CoverBody, Joining};
use crate::bots::{BotView, EnemyView, Noise};
use crate::engine::bots::{BotGame, read_reliable};
use crate::nav::{Follower, NavGraph};
use crate::server::bot_admin;
use crate::world::World;

/// Every one of the seven stats packets in.
const ALL_STATS: u8 = (1 << vcod_cod4::net::stats::STAT_PACKETS) - 1;

impl Cod4Server {
    /// Test-facing: one bot on axis (else allies) that holds still: its
    /// cmds carry no move and no button, and the view its first one took,
    /// so a body put somewhere stands there as put. Its slot.
    #[cfg(any(test, feature = "testing"))]
    pub fn add_held_bot_for_test(&mut self, axis: bool) -> Option<usize> {
        let side = if axis { Side::Axis } else { Side::Allies };
        let slot = self.add_bot(side, false, None)?;
        let bots = self.game.bots.as_mut()?;
        bots.own.held.insert(slot, None);
        bots.admin.manual = bots.admin.manual.max(bots.bots().len());
        Some(slot)
    }

    pub fn bots_mut(&mut self) -> Option<&mut Cod4Bots> {
        self.game.bots.as_mut()
    }

    pub fn level(&self) -> Option<&crate::game::cod4::level::Cod4Level> {
        self.game.level.as_ref()
    }

    pub fn level_mut(&mut self) -> Option<&mut crate::game::cod4::level::Cod4Level> {
        self.game.level.as_mut()
    }

    /// The frame's bots, in `SV_PreFrame`'s bot slot: the admin, then the
    /// shared driver (`bots::driver::step`): a bot connected since enters
    /// the world, then every bot reads its new server commands, answers
    /// them and sends its cmds.
    pub(super) fn step_bots(&mut self) {
        {
            let _p = crate::tick_profile::phase("head/bots/admin");
            bot_admin::manage(self);
        }
        if self.game.bots.is_none() {
            return;
        }
        // A bot is never silent: it holds off `SV_CheckTimeouts`.
        let svs = self.core.sv_time_ms;
        for slot in self.bot_slots() {
            if let Some(c) = self.core.clients[slot]
                .as_mut()
                .filter(|c| c.state != State::Zombie)
            {
                c.link.last_packet_ms = svs;
            }
        }
        if self.game.level.is_none() {
            self.enter_bots();
            return;
        }
        let (id, map) = (self.game.server_id, self.game.cfg.map.clone());
        let level = self.game.level.as_ref().map_or(0, |l| l.level_time());
        let frame = self.frame_ms();
        let times = match self.game.bots.as_mut() {
            Some(b) => {
                b.on_level(id, &map);
                b.think_times(level, frame)
            }
            None => return,
        };
        for at in times {
            if let Some(b) = self.game.bots.as_mut() {
                b.own.think_at = Some(at);
            }
            match self.game.bots.as_ref().and_then(|b| b.graph().cloned()) {
                Some(g) => g.moving(|| crate::bots::driver::step(self)),
                None => cod4::moving(|| crate::bots::driver::step(self)),
            }
        }
        if let Some(b) = self.game.bots.as_mut() {
            b.own.think_at = None;
        }
    }

    fn bot_slots(&self) -> Vec<usize> {
        (0..self.core.clients.len())
            .filter(|&s| self.core.clients[s].as_ref().is_some_and(|c| c.bot))
            .collect()
    }

    /// A bot connected since its last tick enters the world
    /// (`ClientBegin`) once the level has run `BOT_LOAD_MS`; every bot in
    /// it reads all its reliable commands and acknowledges them. A dropped
    /// bot's brain goes.
    fn enter_bots(&mut self) {
        let mut inbox: BTreeMap<usize, Vec<String>> = BTreeMap::new();
        let loaded = match (self.game.level.as_ref(), self.game.bots.as_mut()) {
            (Some(level), Some(b)) => {
                let now = level.level_time();
                now - *b.own.level_start.get_or_insert(now) >= crate::bots::cod4::BOT_LOAD_MS
            }
            _ => true,
        };
        for slot in self.bot_slots() {
            let Some(c) = self.core.clients[slot].as_mut() else {
                continue;
            };
            if c.state == State::Zombie {
                if let Some(b) = self.game.bots.as_mut() {
                    b.remove(slot);
                }
                continue;
            }
            if c.state < State::Active {
                if !loaded {
                    continue;
                }
                c.state = State::Active;
                c.extra.stat_packets = ALL_STATS;
                if let Some(level) = self.game.level.as_mut() {
                    level.begin(slot);
                }
            }
            inbox.insert(slot, read_reliable(c));
        }
        if let Some(b) = self.game.bots.as_mut() {
            b.inbox(inbox);
        }
    }

    /// The configstrings' menu block, from its first slot.
    fn menu_names(&self) -> Vec<String> {
        let (first, last) = vcod_cod4::net::wire::PROTOCOL_17.cs.bounds(CsRange::Menu);
        (first..=last)
            .map(|i| self.core.configstrings.get(i).cloned().unwrap_or_default())
            .collect()
    }

    fn cod4_bots(&self) -> &Cod4Bots {
        self.game
            .bots
            .as_ref()
            .expect("the bots' driver runs with bots")
    }
}

/// CoD4's game under the bots' driver (`bots::driver`), in CoD1's order;
/// its route leaves out what the driver's consts name, as CoD2's, and its
/// brains read the 1.7 playerstate and weapons (`bots::cod4`).
impl BotWorld for Cod4Server {
    type Frame = cod4::Cod4Frame;
    type Cmd = Vec<UserCmd4>;

    /// Off as on CoD2 (bot-combat.md 15).
    const YIELDS: bool = false;
    const SPREADS: bool = true;
    /// Off: not measured here; `PARTS_MATES` plans round a still
    /// teammate (bot-navigation.md section 3.8, "Bodies in the way").
    const SHUNS_BODIES: bool = false;
    /// On: a guard standing in a door held his teammates in a queue
    /// behind him (bot-objectives.md 12.7). CoD2's is on too (11.5).
    const PARTS_MATES: bool = true;
    /// On: on mp_cargoship's `koth` a guard held the radio zone from the
    /// top of the stair at (2844, -65, 336), 40 over the treads where his
    /// teammates queued, out of reach of [`Self::PARTS_MATES`] in three dimensions
    /// (bot-objectives.md 12.16).
    const PARTS_MATES_FLAT: bool = true;
    /// On: on mp_bog's `sd` two guards at (4312, 338, -6) and (4239, 345,
    /// -6) held a teammate pressed in the gap between their boxes, the
    /// way round each one's nearest node running through it
    /// (bot-objectives.md 12.17).
    const PARTS_MATES_BOX: bool = true;
    const PMOVE_PROBES: bool = false;
    /// On: 56 of mp_crash's mantle edges land past the off-path reach
    /// (bot-objectives.md 12.4). CoD2's is on too (11.4).
    const LONG_MANTLES: bool = true;
    /// On: mp_crash's ledge at (745, 964, 174) is climbed facing the
    /// landing 16 units off its node, never facing the node
    /// (bot-objectives.md 12.8).
    const MANTLE_AIMS: bool = true;
    /// On: CoD1's creep backs down none of mp_showdown's mantles over the
    /// roof's parapet at (708, 608, 192), so the follower avoided each in
    /// turn and the roof held its bots (bot-objectives.md 12.16).
    const MANTLE_RESTS: bool = true;
    /// On: on mp_crash a bot climbed onto a teammate standing on the ledge
    /// at (1403, 196, 188), was pushed off and climbed again for 140
    /// ticks (bot-combat.md 20).
    const MANTLE_YIELDS: bool = true;
    /// On: a plan's end short of a point no node reaches left bots walking
    /// at mp_bog's `sab` zone over its wall and round mp_bloc's `koth`
    /// radio stand outside its room (bot-objectives.md 12.19).
    const GIVES_UP_GOALS: bool = true;
    /// The bots' world is the clip map and the brush entities standing on
    /// the level's first frame (`Cod4Bots::nav_tick`).
    const STILL_WORLD: bool = true;

    fn driver(&self) -> &BotDriver {
        &self.cod4_bots().driver
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
        self.game.level.as_ref().map_or(0, |l| l.level_time())
    }

    fn server_id(&self) -> i32 {
        self.game.server_id
    }

    fn world(&self) -> Option<&World> {
        self.cod4_bots().world()
    }

    fn connected(&self, slot: usize) -> bool {
        self.bot_connected(slot)
    }

    /// The tick's top: the playerstates read, the hits scored since, then
    /// the gunfire heard.
    fn take_noises(&mut self) -> Vec<Noise> {
        match (self.game.bots.as_mut(), self.game.level.as_ref()) {
            (Some(b), Some(level)) => {
                b.refresh(level);
                b.read_hits(level);
                b.listen(level)
            }
            _ => Vec::new(),
        }
    }

    /// The admin pass ran ahead of the driver (`step_bots`): here the bots
    /// enter and read their commands.
    fn manage(&mut self) {
        self.enter_bots();
    }

    fn idle(&mut self) {
        self.flush_game_commands();
    }

    fn nav_tick(&mut self) {
        let zones = self.game.zones4.clone();
        let (clip, entities) = match zones.as_deref() {
            Some(z) => (z.clip.as_ref(), z.map.entities.as_slice()),
            None => (None, &[][..]),
        };
        if let (Some(b), Some(level)) = (self.game.bots.as_mut(), self.game.level.as_ref()) {
            b.nav_tick(level, &self.game.cfg.map, clip, entities);
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

    /// CoD4 begins a bot as it connects ([`Self::manage`]) and its commands
    /// were acknowledged as read.
    fn read_commands(
        &mut self,
        slot: usize,
        sid: i32,
        replies: &mut Vec<(usize, String)>,
    ) -> Joining {
        let menus = self.menu_names();
        if let Some(b) = self.game.bots.as_mut() {
            b.read_commands(slot, &menus, sid, replies);
        }
        Joining::No
    }

    /// The stock objective scripts' state (`sd`, `sab`, `dom`, `koth`).
    fn frame(&mut self) -> cod4::Cod4Frame {
        let (Some(b), Some(level)) = (self.game.bots.as_mut(), self.game.level.as_mut()) else {
            return cod4::Cod4Frame::default();
        };
        b.frame(level)
    }

    fn feel(&mut self, slot: usize) {
        if let (Some(b), Some(level)) = (self.game.bots.as_mut(), self.game.level.as_ref()) {
            b.feel(level, slot);
        }
    }

    fn body(&self, slot: usize) -> Option<Body> {
        self.cod4_bots().body(self.game.level.as_ref()?, slot)
    }

    fn enemy(&self, slot: usize, teams: &[i32]) -> Option<EnemyView> {
        self.cod4_bots()
            .sight(self.game.level.as_ref()?, slot, teams)
    }

    fn view(&self, slot: usize, enemy: Option<EnemyView>) -> Option<BotView> {
        self.cod4_bots()
            .view(self.game.level.as_ref()?, slot, enemy)
    }

    fn cover_body(&self, slot: usize, threat: usize) -> Option<CoverBody> {
        self.cod4_bots().cover_body(slot, threat)
    }

    fn deal(
        &mut self,
        frame: cod4::Cod4Frame,
        teams: &[i32],
        _: &[usize],
        round: u32,
        views: &mut Vec<(usize, BotView)>,
    ) {
        if let Some(b) = self.game.bots.as_mut() {
            b.deal(frame, teams, round, views);
        }
    }

    fn routed(&mut self, slot: usize, follower: &Follower, g: &NavGraph) {
        if let Some(b) = self.game.bots.as_mut() {
            b.routed(slot, follower, g);
        }
    }

    /// A bot walking to a gun over the graph makes for the spot behind it
    /// (`bots::cod4`'s `gun`).
    fn pre_route(&mut self, slot: usize, view: &mut BotView, _: &[i32]) {
        view.post = self.game.bots.as_ref().and_then(|b| b.gun_post(slot));
    }

    /// The walk to a gun and the throw back (`bots::cod4`'s `gun`).
    fn pre_think(&mut self, slot: usize, view: &mut BotView, teams: &[i32]) {
        if let (Some(b), Some(level)) = (self.game.bots.as_mut(), self.game.level.as_ref()) {
            b.pre_think(level, slot, view, teams);
        }
    }

    fn cmd(&mut self, slot: usize, cmd: UserCmd, view: &BotView) -> Option<Vec<UserCmd4>> {
        let bots = self.game.bots.as_mut()?;
        let mut out = bots.cmd(slot, cmd, view)?;
        if let Some(view) = bots.own.held.get_mut(&slot) {
            for c in &mut out {
                let angles = *view.get_or_insert(c.angles);
                *c = UserCmd4 {
                    server_time: c.server_time,
                    weapon: c.weapon,
                    angles,
                    ..UserCmd4::default()
                };
            }
        }
        Some(out)
    }

    fn move_ps(&self, slot: usize) -> Option<vcod_common::pmove::PlayerState> {
        self.cod4_bots().move_ps(slot)
    }

    /// Under the level's `bg_fallDamageMinHeight` and `MaxHeight`.
    fn fall_kills(&self, slot: usize) -> Option<f32> {
        let level = self.game.level.as_ref()?;
        let v = level.rt.client_vitals(slot);
        let h = vcod_common::pmove::FallHeights {
            min: level.rules.fall_damage_min_height,
            max: level.rules.fall_damage_max_height,
        };
        Some(crate::nav::fall_that_kills(v.health, v.max_health, h))
    }

    fn lob_probe(
        &self,
        slot: usize,
        weapon: u8,
        target: [f32; 3],
        teams: &[i32],
    ) -> Option<[f32; 2]> {
        self.cod4_bots()
            .lob_probe(self.game.level.as_ref()?, slot, weapon, target, teams)
    }

    fn bot_command(&mut self, slot: usize, text: String) {
        let Some(seq) = self.core.clients[slot]
            .as_ref()
            .map(|c| c.link.last_client_command + 1)
        else {
            return;
        };
        self.client_command(slot, seq, text.as_bytes(), false);
    }

    fn bot_packet(&mut self) {}

    /// Each cmd past the client's last runs through the level's
    /// `ClientThink`, as `SV_UserMove` runs a packet's.
    fn send_cmd(&mut self, slot: usize, cmds: Vec<UserCmd4>, _: bool) {
        for cmd in cmds {
            let Some(c) = self.core.clients[slot].as_mut() else {
                break;
            };
            if c.state != State::Active || cmd.server_time <= c.last_cmd_time {
                continue;
            }
            c.last_cmd_time = cmd.server_time;
            c.extra.command_time = cmd.server_time;
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

/// What CoD4's bot glue does its own way (`engine/bots.rs`).
impl BotGame for super::Cod4Game {
    type Own = Cod4Own;
    const TAG: &'static str = "cod4";

    fn bots(&self) -> Option<&Cod4Bots> {
        self.bots.as_ref()
    }

    fn bots_slot(&mut self) -> &mut Option<Cod4Bots> {
        &mut self.bots
    }

    fn level_running(&self) -> bool {
        self.level.is_some()
    }

    fn gametype(&self) -> &str {
        &self.cfg.gametype
    }

    /// With a fresh profile's stats, as a new player's client sends them:
    /// `Callback_PlayerConnect` kicks a client whose class stats read 0
    /// (`_globallogic.gsc`'s stat integrity check), and a `fast_restart`
    /// hands the level the slot's own block.
    fn admit_bot(e: &mut Cod4Server, slot: usize, userinfo: &str) {
        e.admit(
            slot,
            SocketAddr::from(([0, 0, 0, 0], 0)),
            0,
            0,
            String::new(),
            userinfo,
        );
        if let Some(c) = e.core.clients[slot].as_mut() {
            c.bot = true;
            c.extra.stats = vcod_cod4::net::stats::Stats::fresh();
            c.extra.stat_packets = ALL_STATS;
        }
        if let Some(l) = e.game.level.as_mut() {
            l.mark_bot(slot);
        }
    }

    fn team_menu(e: &Cod4Server) -> Option<usize> {
        e.menu_names().iter().position(|m| m.starts_with("team_"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::GameModule;
    use crate::game::cod4::bot_feed::Cod4Objectives;
    use crate::game::script::{TEAM_ALLIES, TEAM_AXIS};
    use crate::rcon::Redirect;
    use std::rc::Rc;
    use std::time::{Duration, Instant};

    /// A round restart under the same gametype keeps the bots' world (and
    /// the graph over it); another gametype on the same map, whose brush
    /// entities differ, builds it again (BUG-057). Needs `COD4_DIR`.
    #[test]
    fn cod4_bots_rebuild_their_world_for_another_gametype_on_the_map() {
        let Some(install) = vcod_cod4::testing::cod4_install() else {
            return;
        };
        let mut sv = Cod4Server::new(
            super::super::Cod4Config {
                map: "mp_crash".into(),
                gametype: "war".into(),
                hostname: "bots".into(),
                max_clients: 8,
            },
            5,
        );
        sv.set_cvar("sv_pure", "0");
        sv.load(Some(&install)).unwrap();
        sv.add_bots(1, false);
        let mut now = Instant::now();
        let mut run = |sv: &mut Cod4Server, frames: usize| {
            for _ in 0..frames {
                now += Duration::from_millis(50);
                super::super::Cod4Game::drain_console(sv, now);
                sv.tick_frames(now, 1);
                sv.take_outgoing();
            }
        };
        let world = |sv: &Cod4Server| sv.game.bots.as_ref().unwrap().world_rc();
        run(&mut sv, 20);
        let war = world(&sv).expect("no world built");
        sv.push_console("fast_restart");
        run(&mut sv, 20);
        let kept = world(&sv).expect("no world after the restart");
        assert!(Rc::ptr_eq(&kept, &war), "a round restart rebuilt the world");
        sv.push_console("g_gametype sd");
        sv.push_console("map_restart");
        run(&mut sv, 20);
        let sd = world(&sv).expect("no world after the change");
        assert!(!Rc::ptr_eq(&sd, &war), "the war world stayed under sd");
    }

    /// Two bots on a fresh mp_crash `koth` level enter it as a loading
    /// client would ([`crate::bots::cod4::BOT_LOAD_MS`]): by their first
    /// spawn `spawnPerFrameUpdate` has reached every spawn point, so
    /// `getSpawnpoint_NearTeam` runs clean, and both spawn. Needs
    /// `COD4_DIR`.
    #[test]
    fn cod4_bots_spawn_clean_on_a_fresh_koth_level() {
        let Some(install) = vcod_cod4::testing::cod4_install() else {
            return;
        };
        let mut sv = Cod4Server::new(
            super::super::Cod4Config {
                map: "mp_crash".into(),
                gametype: "koth".into(),
                hostname: "bots".into(),
                max_clients: 8,
            },
            5,
        );
        sv.set_cvar("sv_pure", "0");
        sv.load(Some(&install)).unwrap();
        let slots = sv.add_bots(2, false);
        let mut now = Instant::now();
        let mut spawned = std::collections::BTreeSet::new();
        for _ in 0..200 {
            now += Duration::from_millis(50);
            super::super::Cod4Game::drain_console(&mut sv, now);
            sv.tick_frames(now, 1);
            sv.take_outgoing();
            let level = sv.game.level.as_mut().unwrap();
            for &s in &slots {
                if level.rt.client_field(s, "sessionstate").as_deref() == Some("playing") {
                    spawned.insert(s);
                }
            }
        }
        assert_eq!(spawned.len(), 2, "spawned {spawned:?}");
        let aborts = sv.game.level.as_mut().unwrap().rt.aborts();
        assert!(aborts.is_empty(), "script errors:\n{}", aborts.join("\n"));
    }

    /// The roster's bot flag, which keeps a bot out of `numVotingClients`,
    /// survives `fast_restart` and a map change (BUG-042: each level's
    /// `ClientConnect` cleared it).
    #[test]
    fn cod4_bots_stay_marked_across_restarts_and_map_changes() {
        let Some(install) = vcod_cod4::testing::cod4_install() else {
            return;
        };
        let mut sv = Cod4Server::new(
            super::super::Cod4Config {
                map: "mp_crash".into(),
                gametype: "war".into(),
                hostname: "bots".into(),
                max_clients: 8,
            },
            5,
        );
        sv.load(Some(&install)).unwrap();
        let slots = sv.add_bots(2, false);
        let marked = |sv: &Cod4Server| {
            let clients = &sv.game.level.as_ref().unwrap().rt.host.clients;
            slots
                .iter()
                .all(|&s| clients[s].roster.is_some_and(|r| r.bot))
        };
        assert!(marked(&sv));
        sv.map_restart(true).unwrap();
        assert!(marked(&sv), "after fast_restart");
        sv.spawn_server("mp_crash").unwrap();
        assert!(marked(&sv), "after the map change");
    }

    /// Four shooting bots on mp_crash `war` for 100 s: every one comes
    /// through the team and class menus to a spawn, the graph carries
    /// mantles, a bot mantles, and somebody kills somebody else. Then
    /// `map mp_crash` loads the level again and every bot plays again
    /// through the menus it reopens. Needs `COD4_DIR`; ~15 s in the test
    /// profile, the graph's build included.
    #[test]
    #[ignore = "a 100 s match and a map change, ~15 s"]
    fn cod4_bots_join_roam_mantle_fight_and_rejoin_on_mp_crash() {
        let Some(install) = vcod_cod4::testing::cod4_install() else {
            return;
        };
        let mut sv = Cod4Server::new(
            super::super::Cod4Config {
                map: "mp_crash".into(),
                gametype: "war".into(),
                hostname: "bots".into(),
                max_clients: 8,
            },
            5,
        );
        sv.load(Some(&install)).unwrap();
        let slots = sv.add_bots(4, true);
        assert_eq!(slots.len(), 4);
        sv.game.bots.as_mut().unwrap().wait_for_graph();
        let mut now = Instant::now();
        let mut spawned = std::collections::BTreeSet::new();
        let mut mantled = false;
        let mut run =
            |sv: &mut Cod4Server, ticks: usize, spawned: &mut std::collections::BTreeSet<usize>| {
                for _ in 0..ticks {
                    now += Duration::from_millis(50);
                    super::super::Cod4Game::drain_console(sv, now);
                    sv.tick_frames(now, 1);
                    sv.take_outgoing();
                    let level = sv.game.level.as_mut().unwrap();
                    for &s in &slots {
                        if level.rt.client_field(s, "sessionstate").as_deref() == Some("playing") {
                            spawned.insert(s);
                        }
                        mantled |= level
                            .pm_state(s)
                            .is_some_and(|ps| ps.pm_flags & vcod_cod4::pmove::pmf::MANTLE != 0);
                    }
                }
            };
        run(&mut sv, 2000, &mut spawned);
        assert_eq!(spawned.len(), 4, "spawned {spawned:?}");
        let g = sv.game.bots.as_ref().unwrap().graph().unwrap();
        assert!(
            g.len() > 1000 && !g.edges_where(|k| k.mantle).next().is_none(),
            "{} nodes",
            g.len()
        );
        let level = sv.game.level.as_ref().unwrap();
        let kills = level
            .rt
            .script_log()
            .iter()
            .filter(|l| {
                let f: Vec<&str> = l.trim_end().split(';').collect();
                f[0] == "K" && f.len() >= 13 && f[6] != "-1" && f[6] != f[2]
            })
            .count();
        assert!(kills > 0, "no kill");
        // The new level reopens the menus to every client it connects.
        sv.exec("map mp_crash", &mut Redirect::default());
        let mut again = std::collections::BTreeSet::new();
        run(&mut sv, 400, &mut again);
        assert_eq!(again.len(), 4, "spawned again {again:?}");
        assert!(mantled, "no bot mantled");
    }

    /// `bots` non-shooting bots on `map` under `gametype` for up to `ticks`
    /// ticks, `done` asked of the objectives each tick: the tick it held,
    /// or `None`. Needs `COD4_DIR`.
    fn objective_run(
        map: &str,
        gametype: &str,
        bots: usize,
        ticks: usize,
        mut done: impl FnMut(&crate::game::cod4::bot_feed::Cod4Objectives) -> bool,
    ) -> Option<Option<usize>> {
        let install = vcod_cod4::testing::cod4_install()?;
        let mut sv = Cod4Server::new(
            super::super::Cod4Config {
                map: map.into(),
                gametype: gametype.into(),
                hostname: "bots".into(),
                max_clients: 8,
            },
            3,
        );
        sv.set_cvar("sv_pure", "0");
        sv.load(Some(&install)).unwrap();
        sv.add_bots(bots, false);
        sv.game.bots.as_mut().unwrap().wait_for_graph();
        let mut now = Instant::now();
        for tick in 0..ticks {
            now += Duration::from_millis(50);
            super::super::Cod4Game::drain_console(&mut sv, now);
            sv.tick_frames(now, 1);
            sv.take_outgoing();
            let level = sv.game.level.as_mut().unwrap();
            if std::env::var("OBJ_TRACE").is_ok() && tick % 20 == 0 {
                let o = level.objectives();
                eprintln!("tick {tick}: {o:?}");
                for s in 0..bots {
                    if let Some(ps) = level.pm_state(s) {
                        eprintln!(
                            "  {s} at {:?} pm {} w {} ws {}",
                            ps.origin, ps.pm_type, ps.weapon, ps.weapon_state
                        );
                    }
                }
            }
            if level.objectives().is_some_and(|o| done(&o)) {
                return Some(Some(tick));
            }
        }
        Some(None)
    }

    /// A bot on mp_backlot's ground floor under the upstairs crouch SAW
    /// (393), its team having seen an enemy down the gun's lane: no
    /// straight walk reaches the spot behind the gun, so it walks there
    /// over the graph, up the stairs, and mans it. Needs `COD4_DIR`.
    #[test]
    fn cod4_bot_walks_upstairs_to_backlots_saw() {
        let Some(install) = vcod_cod4::testing::cod4_install() else {
            return;
        };
        let mut sv = Cod4Server::new(
            super::super::Cod4Config {
                map: "mp_backlot".into(),
                gametype: "war".into(),
                hostname: "bots".into(),
                max_clients: 8,
            },
            3,
        );
        sv.set_cvar("sv_pure", "0");
        sv.load(Some(&install)).unwrap();
        let slot = sv.add_bots(1, false)[0];
        sv.game.bots.as_mut().unwrap().wait_for_graph();
        let gun = (sv.game.level.as_mut().unwrap().gun_list())
            .into_iter()
            .find(|g| (g.origin.x + 720.0).abs() < 8.0 && (g.origin.y - 68.0).abs() < 8.0)
            .expect("mp_backlot's crouch SAW")
            .id;
        let mut now = Instant::now();
        let mut placed = None;
        let mut manned = None;
        for tick in 0..1600 {
            now += Duration::from_millis(50);
            super::super::Cod4Game::drain_console(&mut sv, now);
            sv.tick_frames(now, 1);
            sv.take_outgoing();
            let level = sv.game.level.as_mut().unwrap();
            // In play past the prematch, which freezes the body.
            let free = level.rt.client_field(slot, "sessionstate").as_deref() == Some("playing")
                && level
                    .pm_state(slot)
                    .is_some_and(|ps| ps.pm_flags & vcod_cod4::pmove::pmf::FROZEN == 0);
            if free && placed.is_none() {
                // The ground floor's eye under the gun, 228 across from
                // the spot and 189 under it.
                level.set_view_pos(slot, [-728.0, 292.0, 129.0], [0.0; 3]);
                placed = Some(tick);
            }
            let level_ms = level.rt.host.level_time_ms;
            if placed.is_some()
                && let Some(b) = sv.game.bots.as_mut()
            {
                b.mark_lane_for_test(gun, TEAM_ALLIES, level_ms);
                b.mark_lane_for_test(gun, TEAM_AXIS, level_ms);
                if b.gun_counts().0 > 0 {
                    manned = Some(tick);
                    break;
                }
            }
        }
        let placed = placed.expect("the bot spawned");
        let manned = manned.expect("the bot manned the gun");
        // About 2 s up the stairs and round: well inside the walk's limit.
        assert!(manned - placed < 300, "manned {} ticks on", manned - placed);
    }

    /// Two bots on mp_crash `sd`, neither shooting: an attacker walks into
    /// the bomb, carries it to a zone and plants it through the use key and
    /// the briefcase, and the defender defuses it.
    #[test]
    fn cod4_bots_take_plant_and_defuse_the_sd_bomb() {
        let mut planted = false;
        let Some(t) = objective_run("mp_crash", "sd", 2, 4000, |o| {
            let Cod4Objectives::Bomb(g) = o else {
                return false;
            };
            planted |= g.planted.is_some();
            planted && g.planted.is_none() && !g.exploded
        }) else {
            return;
        };
        assert!(t.is_some(), "planted {planted}");
    }

    /// Two bots on mp_crash `sab`: one walks into the neutral bomb in the
    /// middle, carries it to the other side's zone and plants it, and the
    /// other side's bot defuses it.
    #[test]
    fn cod4_bots_take_plant_and_defuse_the_sab_bomb() {
        let mut planted = false;
        let Some(t) = objective_run("mp_crash", "sab", 2, 4000, |o| {
            let Cod4Objectives::Bomb(g) = o else {
                return false;
            };
            planted |= g.planted.is_some();
            planted && g.planted.is_none() && !g.exploded
        }) else {
            return;
        };
        assert!(t.is_some(), "planted {planted}");
    }

    /// Two bots on mp_crash `dom`: each team takes a flag by standing in
    /// its zone.
    #[test]
    fn cod4_bots_capture_dom_flags() {
        let Some(t) = objective_run("mp_crash", "dom", 2, 2000, |o| {
            let Cod4Objectives::Zones(z) = o else {
                return false;
            };
            [TEAM_AXIS, TEAM_ALLIES]
                .iter()
                .all(|t| z.iter().any(|f| f.team == *t))
        }) else {
            return;
        };
        assert!(t.is_some());
    }

    /// A bot on mp_crash `koth` takes the radio: its stand is on the floor
    /// inside the trigger, not the stair under it whose head only reaches
    /// in (bot-objectives.md 12.4).
    #[test]
    fn cod4_bots_capture_the_koth_radio_on_mp_crash() {
        let Some(t) = objective_run("mp_crash", "koth", 1, 4000, |o| {
            let Cod4Objectives::Zones(z) = o else {
                return false;
            };
            z.iter().any(|r| r.live && r.team != 0)
        }) else {
            return;
        };
        assert!(t.is_some());
    }

    /// The use key on a dropped weapon (`Player_GetUseList`'s item arm):
    /// on mp_crash `war` a bot drops its gun through the stock
    /// `dropWeaponForDeath`, another player stands over it looking down
    /// and holds use; he takes it, hands over the weapon in hand as a new
    /// item, and the dropped one is gone. Each gun keeps its camo: the item
    /// sends `weapon + 128 * model` and the taker holds it on the dropper's
    /// slot (cod4-client-view.md 4.7).
    #[test]
    fn cod4_use_key_swaps_a_held_gun_for_a_dropped_one() {
        let Some(install) = vcod_cod4::testing::cod4_install() else {
            return;
        };
        let mut sv = Cod4Server::new(
            super::super::Cod4Config {
                map: "mp_crash".into(),
                gametype: "war".into(),
                hostname: "bots".into(),
                max_clients: 8,
            },
            3,
        );
        sv.set_cvar("sv_pure", "0");
        sv.load(Some(&install)).unwrap();
        let slots = sv.add_bots(2, false);
        sv.game.bots.as_mut().unwrap().wait_for_graph();
        let (taker, dropper) = (slots[0], slots[1]);
        let mut now = Instant::now();
        let mut tick = |sv: &mut Cod4Server| {
            now += Duration::from_millis(50);
            super::super::Cod4Game::drain_console(sv, now);
            sv.tick_frames(now, 1);
            sv.take_outgoing();
        };
        let playing = |sv: &mut Cod4Server, s: usize| {
            let level = sv.game.level.as_mut().unwrap();
            level.rt.client_field(s, "sessionstate").as_deref() == Some("playing")
        };
        for _ in 0..1500 {
            tick(&mut sv);
            if playing(&mut sv, taker) && playing(&mut sv, dropper) {
                break;
            }
        }
        // The taker is driven by hand from here on.
        sv.game.bots.as_mut().unwrap().remove(taker);
        for _ in 0..40 {
            tick(&mut sv);
        }
        let level = sv.game.level.as_mut().unwrap();
        let clients = &mut level.rt.host.cod4_mut().clients;
        for (s, camo) in [(dropper, 3), (taker, 2)] {
            let l = &mut clients[s].loadout;
            l.weapon_models[l.weapon] = camo;
        }
        level
            .start_player_script(
                dropper,
                "maps/mp/gametypes/_weapons",
                "dropweaponfordeath",
                &["x"],
            )
            .unwrap();
        for _ in 0..60 {
            tick(&mut sv);
        }
        let level = sv.game.level.as_mut().unwrap();
        let item = level
            .rt
            .host
            .cod4()
            .items
            .iter()
            .find(|i| i.resting())
            .cloned();
        let item = item.expect("a dropped weapon at rest");
        assert_eq!(item.model, 3);
        let index = level
            .item_entities()
            .iter()
            .find(|e| e.0[0] == item.id.0)
            .map(|e| {
                let f = vcod_cod4::net::delta::EntityState::table(3)
                    .iter()
                    .find(|f| f.name == "index")
                    .unwrap();
                e.0[f.word()]
            });
        assert_eq!(index, Some(item.weapon + 3 * 128));
        let ps = level.pm_state(taker).unwrap();
        assert!(!ps.has_weapon(item.weapon));
        let held = ps.weapon;
        // Over the item, looking down at it from 40 units off.
        let at = [item.base.x - 40.0, item.base.y, item.base.z + 60.0];
        level.set_view_pos(taker, at, [56.0, 0.0, 0.0]);
        // The press takes the hint the last end frame set (cod4-gsc.md 9.2).
        for _ in 0..2 {
            tick(&mut sv);
        }
        for _ in 0..10 {
            let level = sv.game.level.as_mut().unwrap();
            let ps = level.pm_state(taker).unwrap();
            let short =
                |a: f32, d: f32| (cod4::cmd_angle(f64::from(a) - f64::from(d)) & 0xffff) as u16;
            let cmd = vcod_cod4::net::clmsg::UserCmd {
                server_time: level.level_time() + 50,
                buttons: 0x8,
                angles: [
                    short(56.0, ps.delta_angles[0]),
                    short(0.0, ps.delta_angles[1]),
                    0,
                ],
                weapon: ps.weapon as u8,
                offhand: ps.off_hand_index as u8,
                ..Default::default()
            };
            level.client_think(taker, &cmd);
            tick(&mut sv);
        }
        let level = sv.game.level.as_mut().unwrap();
        let ps = level.pm_state(taker).unwrap();
        assert!(ps.has_weapon(item.weapon), "took {}", item.weapon);
        let c4 = level.rt.host.cod4();
        assert_eq!(
            c4.clients[taker].loadout.weapon_models[item.weapon as usize],
            3
        );
        let items = &c4.items;
        assert!(items.iter().all(|i| i.id != item.id), "the item is gone");
        assert!(
            items
                .iter()
                .any(|i| i.weapon == held && i.model == 2 && i.owner == taker as u32),
            "handed {held} over on its camo: {items:?}"
        );
        assert!(!ps.has_weapon(held));
    }

    /// A bot on mp_backlot `koth`: the radio comes up and it takes it by
    /// standing in its trigger (two non-shooting bots of the two teams in it
    /// hold the capture still, `updateUseRate`).
    #[test]
    fn cod4_bots_capture_the_koth_radio() {
        let Some(t) = objective_run("mp_backlot", "koth", 1, 3000, |o| {
            let Cod4Objectives::Zones(z) = o else {
                return false;
            };
            z.iter().any(|r| r.live && r.team != 0)
        }) else {
            return;
        };
        assert!(t.is_some());
    }
}
