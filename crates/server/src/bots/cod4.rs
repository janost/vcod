//! CoD4 bots: vcod's CoD1 brain ([`Bot`]) and graph ([`crate::nav`])
//! driving clients of a [`Cod4Level`] (docs/bots.md, "CoD4"), the way
//! [`super::cod2`] drives CoD2's.
//!
//! The graph floods the map's clip map (the shared
//! [`vcod_common::collision::WorldClip`] hook, [`bot_world`]) with walks
//! under 1.5's movement rules, as CoD2's does, and adds the mantles 1.7's
//! own pmove climbs ([`mantle_probe`], `NavGraph::mantles`). The brain
//! reads a [`BotView`] filled from the 1.7 playerstate and the level's
//! weapon defs, its weapons renumbered into the brain's 63 ([`Arms`]); its
//! CoD1 usercmd is rewritten into 1.7's button bits ([`to_cmd4`], the frag
//! onto the offhand key by [`offhand_cmd`]) and run through the level's
//! `ClientThink`, so the body moves by CoD4's pmove. The stock menus are
//! answered as a client's buttons answer them (`_menus.gsc`): the team
//! menu with the bot's team, the class menu (`changeclass_marines`,
//! `changeclass_opfor`) through `changeclass` with a stock class
//! ([`CLASSES`]). The stock objective scripts' state
//! ([`Cod4Level::objectives`]) is dealt out as CoD1's S&D view (`sd`, `sab`,
//! the bomb a [`KeyBomb`]) and United Offensive's domination view (`dom`,
//! `koth`), so the shared brain plants, defuses and holds zones through the
//! use key and its feet (bot-objectives.md 12). A script's `switchToWeapon`
//! (`a <weapon>`, the briefcase of a plant or defuse) is asked for in the
//! cmd's weapon byte until it lands, as a client's cgame selects it.
//!
//! The tick is the shared driver's (`bots::driver::step`, through
//! `Cod4Server`'s `BotWorld` in `cod4/bots.rs`); its route goes without
//! the driver's yields and CoD1 pmove probes, as CoD2's.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;

use glam::Vec3;
use vcod_cod4::collision::ClipMap;
use vcod_cod4::net::clmsg::UserCmd as UserCmd4;
use vcod_cod4::pmove::weapon::{self as weap, WeaponDef};
use vcod_cod4::pmove::{PlayerState, button, pm_type, pmf};
use vcod_common::net::msg::{self, UserCmd};

use super::{
    BombView, Bot, BotView, DomFlagView, DomView, EnemyView, KeyBomb, ObjRole, SdView, SiteView,
    UoView,
};
use crate::game::cod4::bot_feed::{BombGame, Cod4Objectives, Zone};
use crate::game::cod4::level::Cod4Level;
use crate::game::script::{Bounds, TEAM_ALLIES, TEAM_AXIS};
use crate::nav::{NavGraph, NavJob};
use crate::world::World;

mod gun;

/// The movement rules the graph's walks run under: CoD4's movement is
/// CoD2's, which starts from 1.5's (cod4-pmove.md, opening), and CoD2's
/// graph walks under 1.5's ([`super::cod2::GRAPH_RULES`]).
pub const GRAPH_RULES: vcod_common::pmove::MoveRules = vcod_common::pmove::MoveRules::V1_5;

/// Whether the graph is built for bots that read a leap's lip: CoD4's
/// route has no pmove probes (`BotWorld::PMOVE_PROBES`), so its leaps are
/// kept only as plain jumps (`NavGraph::build_opts`, bot-objectives.md
/// 12.14).
pub const LIP: bool = false;

/// The stock classes the class menu offers (`changeclass_marines.menu`'s
/// buttons send `<class>_mp,0`, the `damage_soak` probes' answers).
pub const CLASSES: [&str; 5] = [
    "assault_mp,0",
    "specops_mp,0",
    "heavygunner_mp,0",
    "demolitions_mp,0",
    "sniper_mp,0",
];

/// The most weapons a brain numbers: its masks are 64 bits, bit 0 empty
/// hands.
const BRAIN_WEAPONS: usize = 63;

/// One bot's weapons as its brain numbers them: brain weapon `i + 1` is
/// CoD4 weapon `self.0[i]`. 1.7 numbers its weapons past 64 (every
/// attachment is a weapon of its own); a weapon keeps its number for the
/// bot's life, so the brain's picks hold across ticks.
#[derive(Clone, Debug, Default)]
pub struct Arms(Vec<u32>);

impl Arms {
    /// Gives CoD4 weapon `w` a brain number if it has none and room is
    /// left.
    fn number(&mut self, w: u32) {
        if w != 0 && !self.0.contains(&w) && self.0.len() < BRAIN_WEAPONS {
            self.0.push(w);
        }
    }

    /// The brain's number for CoD4 weapon `w`; 0 for none.
    fn brain(&self, w: u32) -> u8 {
        self.0
            .iter()
            .position(|&x| x == w && w != 0)
            .map_or(0, |i| i as u8 + 1)
    }

    /// The CoD4 weapon behind brain number `b`.
    fn weapon(&self, b: u8) -> u32 {
        match b {
            0 => 0,
            b => self.0.get(usize::from(b) - 1).copied().unwrap_or(0),
        }
    }
}

/// The CoD4 server's bots: the shared driver's state and what CoD4's hooks
/// keep beside it.
/// How long a bot takes to enter the world on a fresh level. A client
/// loads the map first; a bot at once would spawn before `_spawnlogic`'s
/// `spawnPerFrameUpdate` has reached every spawn point (one a frame), and
/// koth's `getSpawnpoint_NearTeam` then compares an unset
/// `numPlayersAtLastUpdate`, a runtime error that ends retail's level
/// (cod4-gsc.md 8). 3 s covers 60 spawn points.
pub(crate) const BOT_LOAD_MS: i32 = 3000;

pub struct Cod4Bots {
    pub(crate) driver: super::driver::BotDriver,
    shoot: bool,
    /// The management's state (`crate::server::bot_admin`), shared with
    /// CoD1's and CoD2's.
    pub admin: crate::server::bot_admin::Ledger,
    /// The `vc_bot_shoot` and `vc_bot_skill` values last applied.
    pub applied: Option<(bool, u8)>,
    /// The level's world the graph, the sight and the throws trace, and
    /// the clip map the mantle probe steps 1.7's pmove over.
    world: Option<(Rc<World>, Arc<ClipMap>)>,
    /// The graph cache's mark for the brush entities `world` clips
    /// ([`brush_key`]).
    brush_key: String,
    /// The level the bots run on: `sv_serverid` and the map. Another one
    /// is a new level ([`Self::new_level`]).
    on_level: Option<(i32, String)>,
    /// The level time the bots first saw on this level: a bot enters the
    /// world [`BOT_LOAD_MS`] after it, as a client loading the level would.
    pub(crate) level_start: Option<i32>,
    /// The bots whose last cmd asked for the frag ([`offhand_cmd`]), and
    /// whether the brain has held the trigger on it since.
    throwing: HashMap<usize, bool>,
    /// The mantle edge each bot follows this tick, and the one it has
    /// already climbed: one climb an edge (bot-objectives.md 12.5).
    mantling: HashMap<usize, (u32, u32)>,
    climbed: HashMap<usize, (u32, u32)>,
    /// The bots a mantle onto a ledge too low to stand on forced to
    /// crouch (`EV_STANCE_FORCE_CROUCH`), held until standing fits.
    crouched: HashSet<usize>,
    /// Each bot's `eventSequence` last read for the stance events.
    stance_seq: HashMap<usize, i32>,
    arms: HashMap<usize, Arms>,
    /// Each bot's `damageEvent` last read and the hit it reported.
    hits: HashMap<usize, (u32, Option<HitFelt>)>,
    /// The level's scored hits read so far, and each victim's last
    /// attacker off them with the level time.
    hits_read: usize,
    attackers: HashMap<usize, (usize, i32)>,
    /// Every client's `eventSequence` last read.
    heard: HashMap<usize, i32>,
    /// Classes picked, by bot, for the spread.
    classes: BTreeMap<usize, usize>,
    /// Each bot's script switch under way (`a <weapon>`): the weapon and
    /// the ticks left to ask for it.
    switches: HashMap<usize, (u32, u8)>,
    /// Whether the bots play the objectives; `bot_match`'s `OBJ=0` turns it
    /// off for a before and after.
    pub objectives: bool,
    /// Whether bots man the mounted guns and throw live frags back
    /// ([`gun`]); `bot_match`'s `GUNS=0` and `THROWBACK=0` turn them off.
    pub man_guns: bool,
    pub throw_back: bool,
    /// The guns' and throw backs' state ([`gun`]).
    pub(crate) gun: gun::GunState,
    pass: Pass,
    /// The level time the next think is due ([`Self::think_times`]).
    next_think: Option<i32>,
    /// The think under way's level time, which the pass reads as its now.
    pub(crate) think_at: Option<i32>,
    /// Bots a test holds still ([`crate::cod4::Cod4Server::add_held_bot_for_test`]),
    /// with the view their first held cmd took.
    pub(crate) held: HashMap<usize, Option<[u16; 3]>>,
}

/// The bots' tick: the 50 ms the brain, its timers and the nav graph's
/// walks were built on, whatever `sv_fps` steps the server.
pub const BOT_TICK_MS: i32 = 50;

/// A hit a bot felt: the attacker, the level time, the direction.
type HitFelt = (usize, i32, [f32; 3]);

/// What one tick carries from one of the driver's hooks to a later one.
#[derive(Default)]
struct Pass {
    /// Each bot's reliable server commands since the last tick.
    inbox: BTreeMap<usize, Vec<String>>,
    now: i32,
    dm: bool,
    /// Every client's playerstate as pmove's, read once at the tick's top.
    states: HashMap<usize, PlayerState>,
    felt: Option<(usize, Option<super::Hurt>)>,
    /// The guns the bots may man this tick, and the bot in pass 3 with its
    /// gun step and throw back ([`gun`]).
    guns: Vec<crate::game::cod2_turret::GunInfo>,
    steps: Option<(usize, Option<gun::GunCmd>, Option<gun::ThrowBack>)>,
    /// What stops a round besides the world and the bodies: the
    /// destructible cars (`Cod4Level::shot_blockers`).
    blockers: Vec<crate::game::cod4::entity_damage::DamageBox>,
}

impl Cod4Bots {
    pub fn new(shoot: bool) -> Cod4Bots {
        Cod4Bots {
            driver: super::driver::BotDriver {
                nav_background: true,
                ..Default::default()
            },
            shoot,
            admin: Default::default(),
            applied: None,
            world: None,
            brush_key: String::new(),
            on_level: None,
            level_start: None,
            next_think: None,
            think_at: None,
            throwing: HashMap::new(),
            mantling: HashMap::new(),
            climbed: HashMap::new(),
            crouched: HashSet::new(),
            stance_seq: HashMap::new(),
            arms: HashMap::new(),
            hits: HashMap::new(),
            hits_read: 0,
            attackers: HashMap::new(),
            heard: HashMap::new(),
            classes: BTreeMap::new(),
            switches: HashMap::new(),
            held: HashMap::new(),
            objectives: true,
            man_guns: true,
            throw_back: true,
            gun: Default::default(),
            pass: Pass::default(),
        }
    }

    /// A brain for the client in `slot`, answering the team menu with
    /// `team` (`autoassign`, `allies` or `axis`).
    pub fn add(&mut self, slot: usize, name: &str, team: &str, seed: u64) {
        let mut bot = Bot::new(team, self.shoot, seed);
        bot.name = name.to_string();
        self.driver.bots.insert(slot, bot);
    }

    /// The level times the bots think at in the `SV_Frame` whose frames
    /// run on from `level` by `frame` each: one every [`BOT_TICK_MS`] of
    /// level time, before the next frame's. At `sv_fps` 20 that is one, at
    /// `level`, every frame (a catch-up's too); at 40 one every other
    /// frame; at 10 two a frame, so a bot moves 100 ms of cmds in its
    /// 100 ms. A bot clock behind the level (a catch-up) or far ahead (a
    /// clock that went back) starts over at `level`.
    pub fn think_times(&mut self, level: i32, frame: i32) -> Vec<i32> {
        let mut next = match self.next_think {
            Some(n) if n >= level && n - level <= BOT_TICK_MS => n,
            _ => level,
        };
        let mut out = Vec::new();
        while next < level + frame.max(1) {
            out.push(next);
            next += BOT_TICK_MS;
        }
        self.next_think = Some(next);
        out
    }

    /// The level the bots now stand on, `server_id` and `map`: on another
    /// one every brain forgets the menus it answered and what the old level
    /// left (paths, enemies, hits, events heard, weapon numbers); a new map
    /// drops the graph and the world too, for the next tick to build.
    pub fn on_level(&mut self, server_id: i32, map: &str) {
        let same_map = self.on_level.as_ref().is_some_and(|(_, m)| m == map);
        if self
            .on_level
            .as_ref()
            .is_some_and(|(id, m)| *id == server_id && m == map)
        {
            return;
        }
        let first = self.on_level.is_none();
        self.on_level = Some((server_id, map.to_string()));
        if first {
            return;
        }
        let mut fresh = Cod4Bots::new(self.shoot);
        fresh.admin = std::mem::take(&mut self.admin);
        fresh.applied = self.applied;
        fresh.driver.nav_background = self.driver.nav_background;
        fresh.on_level = self.on_level.take();
        fresh.classes = std::mem::take(&mut self.classes);
        fresh.objectives = self.objectives;
        fresh.man_guns = self.man_guns;
        fresh.throw_back = self.throw_back;
        fresh.gun.carry_counts(&self.gun);
        if same_map {
            fresh.driver.nav_job = self.driver.nav_job.take();
            fresh.driver.nav = self.driver.nav.take();
            fresh.driver.creases = std::mem::take(&mut self.driver.creases);
            fresh.driver.probes = std::mem::take(&mut self.driver.probes);
            fresh.driver.sites = std::mem::take(&mut self.driver.sites);
            fresh.world = self.world.take();
            fresh.brush_key = std::mem::take(&mut self.brush_key);
        }
        fresh.driver.bots = std::mem::take(&mut self.driver.bots);
        for bot in fresh.driver.bots.values_mut() {
            bot.rearm();
        }
        *self = fresh;
    }

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

    pub fn remove(&mut self, slot: usize) {
        self.held.remove(&slot);
        self.admin.pinned.remove(&slot);
        self.admin.skills.remove(&slot);
        self.driver.bots.remove(&slot);
        self.driver.paths.remove(&slot);
        self.driver.enemies.remove(&slot);
        self.driver.hunts.remove(&slot);
        self.driver.covers.remove(&slot);
        self.throwing.remove(&slot);
        self.mantling.remove(&slot);
        self.climbed.remove(&slot);
        self.crouched.remove(&slot);
        self.stance_seq.remove(&slot);
        self.arms.remove(&slot);
        self.hits.remove(&slot);
        self.heard.remove(&slot);
        self.classes.remove(&slot);
        self.switches.remove(&slot);
        self.gun.forget(slot);
    }

    /// The brains, by slot.
    pub fn bots(&self) -> &BTreeMap<usize, Bot> {
        &self.driver.bots
    }

    pub fn bots_mut(&mut self) -> &mut BTreeMap<usize, Bot> {
        &mut self.driver.bots
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

    pub(crate) fn world(&self) -> Option<&World> {
        self.world.as_ref().map(|(w, _)| &**w)
    }

    /// The level's world, built once a level from its clip map and the
    /// map's spawn points, and the graph's build under CoD4's mantle,
    /// polled until it lands (`BotWorld::nav_tick`).
    pub(crate) fn nav_tick(
        &mut self,
        level: &Cod4Level,
        map: &str,
        clip: Option<&Arc<ClipMap>>,
        entities: &[Vec<(String, String)>],
    ) {
        // Once the level has run a frame: its gametype has deleted the
        // other gametypes' objects by then and `brushes` holds what stands.
        if self.world.is_none()
            && let Some(clip) = clip
            && level.level_time() > 0
        {
            let brushes = level.brushes().to_vec();
            self.brush_key = brush_key(&brushes);
            self.world = Some((bot_world(clip.clone(), entities, brushes), clip.clone()));
        }
        let Some((world, clip)) = self.world.as_ref() else {
            return;
        };
        if self.driver.nav.is_none() {
            let rules = level.rules;
            let job = self.driver.nav_job.get_or_insert_with(|| {
                NavJob::start_opts(
                    &format!("cod4 {map}{}", self.brush_key),
                    world,
                    GRAPH_RULES,
                    Some(mantle_probe(clip.clone(), rules)),
                    walk_step(clip.clone(), rules, entities),
                    LIP,
                )
            });
            self.driver.nav = job.poll(!self.driver.nav_background);
        }
    }

    /// The tick's top: every client's playerstate, read once, and each
    /// bot's weapons numbered for its brain ([`Arms`]).
    pub(crate) fn refresh(&mut self, level: &Cod4Level) {
        self.pass.states = (0..level.players.len())
            .filter_map(|s| Some((s, level.pm_state(s)?)))
            .collect();
        self.pass.now = self.think_at.unwrap_or_else(|| level.level_time());
        self.pass.dm = level.rt.cvars().get("g_gametype") == "dm";
        for &slot in self.driver.bots.keys() {
            // A follower's playerstate is the followed client's.
            let Some(ps) = self
                .pass
                .states
                .get(&slot)
                .filter(|ps| ps.other_flags & vcod_cod4::pmove::POF_PLAYER != 0)
            else {
                continue;
            };
            let arms = self.arms.entry(slot).or_default();
            for w in 1..128u32 {
                if ps.weapons[(w / 32) as usize] >> (w % 32) & 1 == 1 {
                    arms.number(w);
                }
            }
        }
        self.read_stance_events();
    }

    /// `EV_STANCE_FORCE_CROUCH` in a bot's own ring since the last tick
    /// crouches it, as the client game's `CL_SetStance` does for a player; it
    /// stands again once a standing body fits where it is, on
    /// the ground. A bot that stood on mp_crash's sill after the crouched
    /// climb onto it wedged under the lintel for the rest of the match
    /// (bot-objectives.md 12.5).
    fn read_stance_events(&mut self) {
        use vcod_cod4::pmove::{MASK_PLAYERSOLID, PLAYER_MAXS, PLAYER_MINS, PmWorld, ev};
        let clip = self.world.as_ref().map(|w| w.1.clone());
        for &slot in self.driver.bots.keys() {
            let Some(ps) = self.pass.states.get(&slot) else {
                continue;
            };
            let seq = ps.event_sequence;
            let from = self.stance_seq.insert(slot, seq).unwrap_or(seq);
            if ps.pm_type != pm_type::NORMAL || ps.other_flags & vcod_cod4::pmove::POF_PLAYER == 0 {
                self.crouched.remove(&slot);
                continue;
            }
            if (from.max(seq - 4)..seq)
                .any(|i| ps.events[(i & 3) as usize] == ev::STANCE_FORCE_CROUCH)
            {
                self.crouched.insert(slot);
            } else if self.crouched.contains(&slot)
                && ps.on_ground()
                && clip.as_ref().is_some_and(|c| {
                    !PmWorld::trace(
                        c.as_ref(),
                        ps.origin,
                        ps.origin,
                        PLAYER_MINS,
                        PLAYER_MAXS,
                        ps.client_num,
                        MASK_PLAYERSOLID,
                    )
                    .startsolid
                })
            {
                self.crouched.remove(&slot);
            }
        }
    }

    fn ps(&self, slot: usize) -> Option<&PlayerState> {
        self.pass.states.get(&slot)
    }

    /// Each bot's reliable server commands since the last tick.
    pub(crate) fn inbox(&mut self, inbox: BTreeMap<usize, Vec<String>>) {
        self.pass.inbox = inbox;
    }

    /// The `t` menus the bot in `slot` was sent, named off their
    /// configstrings (`menus`, from the menu block's first slot), and the
    /// `mr` answers into `replies`: a team menu goes to the brain, which
    /// answers its team, a class menu gets the bot's class through
    /// `changeclass`, as the menu's buttons send it.
    pub(crate) fn read_commands(
        &mut self,
        slot: usize,
        menus: &[String],
        server_id: i32,
        replies: &mut Vec<(usize, String)>,
    ) {
        let Some(texts) = self.pass.inbox.get(&slot) else {
            return;
        };
        let changeclass = menus.iter().position(|m| m == "changeclass");
        let mut named: Vec<(i32, String)> = Vec::new();
        let mut class_asked = false;
        for (seq, text) in texts.iter().enumerate() {
            // The brain numbers weapons its own way ([`Arms`]): the switch
            // is asked for in CoD4's numbers by [`Self::cmd`].
            if let Some(w) = text.strip_prefix("a ").and_then(|w| w.trim().parse().ok()) {
                self.switches.insert(slot, (w, SWITCH_TICKS));
                continue;
            }
            let menu = text
                .strip_prefix("t ")
                .and_then(|i| i.split_whitespace().next()?.parse::<usize>().ok())
                .map(|i| menus.get(i).map_or("", String::as_str));
            match menu {
                Some(m) if m.starts_with("changeclass_") || m.starts_with("class_") => {
                    class_asked = true;
                    continue;
                }
                Some(m) if !m.is_empty() => {
                    named.push((seq as i32, format!("v g_scriptMainMenu {m}")));
                }
                _ => {}
            }
            named.push((seq as i32, text.clone()));
        }
        let Some(bot) = self.driver.bots.get_mut(&slot) else {
            return;
        };
        let named: Vec<(i32, &str)> = named.iter().map(|(q, t)| (*q, t.as_str())).collect();
        for r in bot.observe(server_id, &named, &|_| true) {
            replies.push((slot, r));
        }
        if class_asked && let Some(i) = changeclass {
            let class = self.class_of(slot);
            replies.push((slot, format!("mr {server_id} {i} {}", CLASSES[class])));
        }
    }

    /// The class the bot in `slot` answers with: kept once picked, else the
    /// one the fewest other bots carry, the lowest first.
    fn class_of(&mut self, slot: usize) -> usize {
        if let Some(&c) = self.classes.get(&slot) {
            return c;
        }
        let count = |c: usize| self.classes.values().filter(|&&x| x == c).count();
        let pick = (0..CLASSES.len()).min_by_key(|&c| count(c)).unwrap_or(0);
        self.classes.insert(slot, pick);
        pick
    }

    /// The hit the bot in `slot` felt this tick ([`Self::hurt`]).
    pub(crate) fn feel(&mut self, level: &Cod4Level, slot: usize) {
        let hurt = self.hurt(level, slot);
        self.pass.felt = Some((slot, hurt));
    }

    fn felt(&self, slot: usize) -> Option<super::Hurt> {
        self.pass.felt.filter(|f| f.0 == slot).and_then(|f| f.1)
    }

    /// The level's scored hits since the last read: each victim's last
    /// attacker, at the level time.
    pub(crate) fn read_hits(&mut self, level: &Cod4Level) {
        let now = level.level_time();
        let hits = level.hits_since(self.hits_read);
        self.hits_read += hits.len();
        for &(attacker, victim, _) in hits {
            if attacker != victim {
                self.attackers.insert(victim, (attacker, now));
            }
        }
    }

    /// [`BotView::hurt`] for the bot in `slot`: a `damageEvent` new since
    /// the last tick is a hit, by the client the level last scored a hit on
    /// it for, along the direction it came (`Cod4Level::hit_dir`; zero for
    /// none); reported for [`super::HURT_MS`] while the attacker still
    /// plays.
    fn hurt(&mut self, level: &Cod4Level, slot: usize) -> Option<super::Hurt> {
        let now = self.pass.now;
        let w = level.wire_state(slot)?;
        let word = |name: &str| {
            vcod_cod4::net::fields::PLAYER_STATE
                .iter()
                .find(|f| f.name == name)
                .map_or(0, |f| w.words[f.word()])
        };
        let event = word("damageEvent");
        let attacker = self.attackers.get(&slot).copied();
        let seen = self.hits.entry(slot).or_insert((event, None));
        if seen.0 != event {
            seen.0 = event;
            seen.1 = attacker
                .filter(|&(_, t)| (0..1000).contains(&now.wrapping_sub(t)))
                .map(|(a, _)| (a, now, level.hit_dir(slot).unwrap_or(Vec3::ZERO).to_array()));
        }
        let (a, at, dir) = seen.1?;
        let ago_ms = now.wrapping_sub(at);
        if !(0..super::HURT_MS).contains(&ago_ms) || !self.playing(level, a) {
            return None;
        }
        Some(super::Hurt {
            slot: a,
            ago_ms,
            dir,
        })
    }

    /// The enemy the bot in `slot` fights, the one that just hit it seen
    /// wherever it stands.
    pub(crate) fn sight(&self, level: &Cod4Level, slot: usize, teams: &[i32]) -> Option<EnemyView> {
        let aware = self.driver.bots.get(&slot).is_none_or(|b| b.aware);
        let attacker = self.felt(slot).map(|h| h.slot).filter(|_| aware);
        self.enemy(level, slot, teams, attacker)
    }

    /// The enemy in sight the CoD1 driver would pick: the tracked target,
    /// else the nearest in the sight cone within range whose chest or eye
    /// the eye line reaches; `attacker` is seen wherever he stands.
    fn enemy(
        &self,
        level: &Cod4Level,
        slot: usize,
        teams: &[i32],
        attacker: Option<usize>,
    ) -> Option<EnemyView> {
        if !self.shoot || !self.playing(level, slot) {
            return None;
        }
        let me = self.ps(slot)?;
        let (world, _) = self.world.as_ref()?;
        let bot = self.driver.bots.get(&slot);
        let mine = teams.get(slot).copied().unwrap_or(0);
        let tracked = bot
            .and_then(|b| b.target_slot())
            .filter(|_| bot.is_none_or(|b| b.aware));
        let fov = bot.map_or(super::Skill::default().fov_deg, |b| b.skill.fov_deg);
        let my_eye = eye(me);
        let mut candidates: Vec<(f32, Vec3, EnemyView)> = Vec::new();
        for (&i, p) in &self.pass.states {
            if i == slot || !self.playing(level, i) {
                continue;
            }
            let theirs = teams.get(i).copied().unwrap_or(0);
            let foe = self.pass.dm
                || !(mine == TEAM_AXIS || mine == TEAM_ALLIES)
                || (theirs != mine && (theirs == TEAM_AXIS || theirs == TEAM_ALLIES));
            if !foe {
                continue;
            }
            let d = super::dist_sq(me.origin.into(), p.origin.into());
            if d > super::SHOOT_RANGE * super::SHOOT_RANGE {
                continue;
            }
            let chest = super::chest(p.origin.into(), p.view_height_current);
            let known = Some(i) == tracked || Some(i) == attacker;
            if !known && !super::in_sight_cone(my_eye, me.view_angles, chest.into(), fov) {
                continue;
            }
            candidates.push((
                d,
                eye(p),
                EnemyView {
                    slot: i,
                    origin: chest,
                    velocity: p.velocity.into(),
                },
            ));
        }
        // The states map has no order: nearest first, then by slot.
        candidates.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.2.slot.cmp(&b.2.slot)));
        let w = &world.collision;
        let sighted = |i: usize| {
            let (_, their_eye, e) = &candidates[i];
            let clear = |to: [f32; 3]| self.shot_clear(w, my_eye, to.into());
            let origin = super::aim_point(e.origin, (*their_eye).into(), clear)?;
            Some(EnemyView { origin, ..*e })
        };
        let order: Vec<(f32, usize)> = candidates.iter().map(|c| (c.0, c.2.slot)).collect();
        super::pick_enemy(&order, attacker, tracked, sighted)
    }

    /// Whether a round from `from` reaches `to`: the world's shot trace
    /// and no destructible car's collision triangle between
    /// (`Pass::blockers`), the parts `hidePart` took away (a broken
    /// window) skipped as a round skips them (cod4-combat.md 13.1).
    pub(super) fn shot_clear(
        &self,
        w: &vcod_common::collision::CollisionWorld,
        from: Vec3,
        to: Vec3,
    ) -> bool {
        w.shot_trace(from, to).fraction >= 1.0
            && !self.pass.blockers.iter().any(|b| {
                b.trace(from, to, 1.0).is_some()
                    && b.trace_surfs(from, to, 1.0, crate::game::cod4::combat::MASK_SHOT)
                        .is_some()
            })
    }

    /// The frame's gunfire since the last tick: each playing client's new
    /// fire events chest high, as the CoD1 driver gathers them. Blasts are
    /// not heard yet.
    pub(crate) fn listen(&mut self, level: &Cod4Level) -> Vec<super::Noise> {
        let mut noises = Vec::new();
        let mut slots: Vec<usize> = self.pass.states.keys().copied().collect();
        slots.sort_unstable();
        for i in slots {
            if !self.playing(level, i) {
                self.heard.remove(&i);
                continue;
            }
            let ps = &self.pass.states[&i];
            let seq = ps.event_sequence;
            let prev = self.heard.insert(i, seq).unwrap_or(seq);
            let fired = (prev.max(seq - 4)..seq).any(|k| {
                matches!(
                    ps.events[(k & 3) as usize],
                    weap::ev::FIRE_WEAPON | weap::ev::FIRE_WEAPON_LASTSHOT
                )
            });
            if fired {
                noises.push(super::Noise {
                    at: super::chest(ps.origin.into(), ps.view_height_current),
                    source: i,
                    radius: super::HEAR_GUNFIRE,
                });
            }
        }
        noises
    }

    /// A client playing in the world: `pm_type` normal or linked, alive,
    /// and its own body (`POF_PLAYER`): a spectator following another
    /// client is sent that client's playerstate whole, `pm_type` and
    /// origin with it (bot-objectives.md 12.4).
    pub(crate) fn playing(&self, level: &Cod4Level, slot: usize) -> bool {
        self.ps(slot).is_some_and(|ps| {
            matches!(ps.pm_type, pm_type::NORMAL | pm_type::NORMAL_LINKED)
                && ps.other_flags & vcod_cod4::pmove::POF_PLAYER != 0
        }) && !level.rt.client_vitals(slot).dead
    }

    /// The body in `slot` as the driver's team passes read it.
    pub(crate) fn body(&self, level: &Cod4Level, slot: usize) -> Option<super::driver::Body> {
        let ps = self.ps(slot)?;
        Some(super::driver::Body {
            origin: ps.origin,
            velocity: ps.velocity,
            playing: self.playing(level, slot),
            on_ground: ps.on_ground(),
            on_ladder: ps.pm_flags & pmf::LADDER != 0,
        })
    }

    /// What the cover search reads of the bot in `slot` and its `threat`:
    /// 1.7's standing lean is CoD2's half (`PM_UpdateLean`).
    pub(crate) fn cover_body(
        &self,
        slot: usize,
        threat: usize,
    ) -> Option<super::driver::CoverBody> {
        let me = self.ps(slot)?;
        let threat = self.ps(threat)?;
        Some(super::driver::CoverBody {
            origin: me.origin,
            threat_eye: eye(threat),
            lean: 0.5,
            mask: vcod_cod4::pmove::MASK_PLAYERSOLID,
        })
    }

    /// The CoD1 body the follower's foot and jump tests read: where the
    /// CoD4 body stands and how it moves, its last jump's age, and the jump
    /// penalty `GRAPH_RULES` jumps under (1.7's `JUMPING` with its
    /// `pm_time`, as CoD2's).
    pub(crate) fn move_ps(&self, slot: usize) -> Option<vcod_common::pmove::PlayerState> {
        let ps = self.ps(slot)?;
        let now = self.pass.now;
        let mut b = vcod_common::pmove::PlayerState::spawn(ps.origin, ps.view_angles[1]);
        b.rules = GRAPH_RULES;
        b.velocity = ps.velocity;
        b.on_ground = ps.on_ground();
        b.on_ladder = ps.pm_flags & pmf::LADDER != 0;
        b.ladder_normal = ps.ladder_vec;
        b.since_jump_ms = if ps.jump_time > 0 {
            now.saturating_sub(ps.jump_time).max(0) as f32
        } else {
            f32::MAX
        };
        b.jump_penalty = ps.pm_flags & pmf::JUMPING != 0;
        b.knockback_ms = ps.pm_time as f32;
        b.jump_origin_z = ps.jump_origin_z;
        Some(b)
    }

    /// The bots following a mantle edge (`BotWorld::routed`).
    pub(crate) fn routed(&mut self, slot: usize, follower: &crate::nav::Follower, g: &NavGraph) {
        match follower.mantle_edge(g) {
            Some(e) => self.mantling.insert(slot, e),
            None => self.mantling.remove(&slot),
        };
    }

    /// The body in `slot` as the brain reads it, its weapons in the
    /// brain's numbers.
    pub(crate) fn view(
        &self,
        level: &Cod4Level,
        slot: usize,
        enemy: Option<EnemyView>,
    ) -> Option<BotView> {
        let ps = self.ps(slot)?.clone();
        let defs = level.weapon_defs();
        let def = |w: u32| defs.get(w as usize).filter(|_| w > 0);
        let holds = |w: u32| w < 128 && ps.weapons[(w / 32) as usize] >> (w % 32) & 1 == 1;
        let rounds = |w: u32| {
            def(w).map_or(0, |d| {
                let reserve = if d.clip_only {
                    0
                } else {
                    ps.ammo.get(d.ammo_index).copied().unwrap_or(0)
                };
                ps.ammoclip.get(d.clip_index).copied().unwrap_or(0) + reserve
            })
        };
        let no_arms = Arms::default();
        let arms = self.arms.get(&slot).unwrap_or(&no_arms);
        let mut weapons_held = 0u64;
        let mut loaded = 0u64;
        let (mut pistol, mut gun, mut grenade) = (None, None, None);
        for w in (1..defs.len() as u32).filter(|&w| holds(w)) {
            let b = arms.brain(w);
            if b == 0 {
                continue;
            }
            let Some(d) = def(w) else { continue };
            weapons_held |= 1 << b;
            let has = rounds(w) > 0;
            if has {
                loaded |= 1 << b;
            }
            if d.offhand_class == weap::OFFHAND_CLASS_FRAG {
                if has && grenade.is_none() {
                    grenade = Some(b);
                }
            } else if d.offhand_class != 0 || d.weap_class == weap::WEAPCLASS_ITEM {
            } else if d.weap_class == weap::WEAPCLASS_PISTOL {
                pistol.get_or_insert(b);
            } else if has && d.weap_type == 0 {
                gun.get_or_insert(b);
            }
        }
        let held = def(ps.weapon);
        let reload_ms = held.map_or(0, |d| {
            if d.segmented_reload {
                d.reload_start_time + d.reload_time
            } else if d.reload_empty_time > 0 {
                d.reload_empty_time
            } else {
                d.reload_time
            }
        });
        let draw_ms = pistol
            .map(|b| arms.weapon(b))
            .and_then(def)
            .map_or(0, |p| held.map_or(0, |d| d.drop_time) + p.raise_time);
        // The brain throws by switching to its frag and holding the
        // trigger; on CoD4 the frag key does both ([`offhand_cmd`]), so the
        // frag reads as in hand from the cmd that asked for it, while the
        // brain is in the throw. Read in hand after a death ended the throw,
        // it became the brain's armed weapon: the next life held the frag
        // key with the pin out and the trigger never pressed until the fuse
        // ran out in the hand (bot-objectives.md 12.5).
        let in_throw = self.driver.bots.get(&slot).is_some_and(|b| b.in_throw());
        let weapon = match grenade {
            Some(g) if in_throw && self.throwing.contains_key(&slot) => g,
            _ => arms.brain(ps.weapon),
        };
        let vitals = level.rt.client_vitals(slot);
        let dead = vitals.dead || ps.pm_type >= pm_type::DEAD;
        let on_ladder = ps.pm_flags & pmf::LADDER != 0;
        let on_ground = ps.on_ground();
        let origin: [f32; 3] = ps.origin.into();
        let (w, _) = self.world.as_ref()?;
        let nav = self.driver.nav.as_ref();
        let enemy_eye = enemy.and_then(|e| Some(eye(self.ps(e.slot)?).into()));
        let frag_near = self.frag_near(level, slot);
        let playing = self.playing(level, slot);
        Some(BotView {
            origin,
            eye_height: ps.view_height_current,
            eye_settled: ps.view_height_current == ps.view_height_target as f32,
            lean_offset: vcod_common::pmove::aim::lean_offset(
                ps.view_angles[1].to_radians(),
                ps.leanf,
            )
            .into(),
            delta_angles: ps.delta_angles.map(deg_short),
            view: ps.view_angles,
            gun_offset: [0.0; 2],
            weapon,
            weapons_held,
            clip: held.map_or(-1, |d| {
                ps.ammoclip.get(d.clip_index).copied().unwrap_or(0) as i16
            }),
            fire_time_ms: held.map_or(0, fire_cycle),
            busy_ms: ps.weapon_time,
            automatic: held.is_some_and(|d| d.fire_type == weap::FIRETYPE_FULLAUTO),
            has_ads: held.is_some_and(|d| d.aim_down_sight),
            sniper: held.is_some_and(|d| d.overlay_reticle != 0),
            lmg: false,
            deployed: false,
            deploy_yaw: 0.0,
            ads_frac: ps.weapon_pos_frac,
            spread_scale: ps.aim_spread_scale,
            speed: ps.velocity.truncate().length(),
            velocity: ps.velocity.into(),
            dead,
            playing: playing && !dead,
            enemy,
            hurt: self.felt(slot),
            friend_in_line: false,
            sidestep: None,
            cover: None,
            hunt: None,
            use_takes_weapon: false,
            grenade,
            grenade_cooks: grenade
                .map(|b| arms.weapon(b))
                .and_then(def)
                .is_some_and(|d| d.cook_off_hold),
            // 1.7's pin-out wait is state 18 (`GRENADE_HELD`): the pin's
            // timer moves to it (0x8063c2a), and the throw goes through 17
            // (`GRENADE_THROW`, 0x8064b81).
            grenade_ready: ps.weapon_state == weap::state::GRENADE_HELD,
            frag_near,
            post: None,
            give_way: None,
            lob: None,
            waypoint: None,
            hazard_ahead: [false; 8],
            drop_ahead: [None; 8],
            // Felt by the driver, on the ticks the brain reads it.
            wall_ahead: [false; 8],
            noise: None,
            linked: ps.pm_type == pm_type::NORMAL_LINKED,
            stranded: nav.is_some_and(|g| g.dead_end(origin) || g.pit(origin, &w.spawn_points)),
            on_ladder,
            ladder_normal: ps.ladder_vec.into(),
            // 1.7's `PM_LadderMove` is CoD2's: no flat move into the face,
            // so the rungs let go only past their top (bot-objectives.md
            // 12.15).
            tops_out: true,
            ladder_middle: nav
                .filter(|_| on_ladder)
                .and_then(|g| g.ladder_middle(ps.origin))
                .map(Into::into),
            back_face: None,
            on_ground,
            jump: false,
            sliding: false,
            leap: false,
            lip: false,
            crouch_jump: false,
            stop: false,
            mate_ahead: false,
            squad: Default::default(),
            run_off: false,
            mantle: false,
            // `ClientThink` steps pmove at the held weapon's scale; the
            // keys are whole (the wire carries only their sign).
            move_scale: 1.0,
            fatigue: None,
            sprinting: false,
            sprint_room: 0.0,
            push_off: None,
            unpin: None,
            detour: false,
            pistol,
            rocket: None,
            primaries: [0; 2],
            launchers: 0,
            gun,
            reload_ms,
            draw_ms,
            loaded,
            sd: None,
            re: None,
            bel: None,
            uo: None,
            one_life: level.rt.cvars().get("g_gametype") == "sd",
            survive: super::retreat::Vitals {
                health: vitals.health,
                max_health: vitals.max_health.max(1),
                clip_size: held.map_or(0, |d| d.clip_size.min(i32::from(i16::MAX)) as i16),
                enemy_eye,
                heal: None,
                cover: None,
            },
            vehicles: Vec::new(),
            seat: None,
            armor: None,
        })
    }

    /// The nearest live frag within [`super::FLEE_REACH`] across and
    /// [`super::FLEE_HEIGHT`] up or down, the bot's own or one it notices.
    fn frag_near(&self, level: &Cod4Level, slot: usize) -> Option<[f32; 3]> {
        let ps = self.ps(slot)?;
        let (world, _) = self.world.as_ref()?;
        let at = ps.origin;
        let eye = eye(ps);
        let defs = level.weapon_defs();
        level
            .rt
            .host
            .cod4()
            .missiles
            .iter()
            .filter(|m| {
                defs.get(m.weapon as usize).is_some_and(|d| {
                    d.offhand_class == weap::OFFHAND_CLASS_FRAG && d.explosion_radius > 0
                })
            })
            .filter(|m| {
                m.owner == Some(slot)
                    || super::notices_frag(eye, ps.view_angles[1], m.origin, |a, b| {
                        world.collision.shot_trace(a, b).fraction >= 1.0
                    })
            })
            .map(|m| m.origin)
            .filter(|o| {
                (o.z - at.z).abs() < super::FLEE_HEIGHT
                    && (*o - at).truncate().length() < super::FLEE_REACH
            })
            .min_by(|a, b| a.distance(at).total_cmp(&b.distance(at)))
            .map(Into::into)
    }

    /// [`BotView::lob`] for the bot in `slot` throwing frag `weapon` (the
    /// brain's number) at `target`, as CoD2's [`super::cod2::lob_probe`]
    /// flies it: of the throws from 10 degrees down to 85 up, every 5, the
    /// one whose frag comes down nearest `target`, within
    /// [`super::LOB_MISS`], and not within its `explosionRadius` of the
    /// thrower, flown from the eye at `projectileSpeed` plus
    /// `projectileSpeedUp` under gravity 800 until it meets the world.
    pub(crate) fn lob_probe(
        &self,
        level: &Cod4Level,
        slot: usize,
        weapon: u8,
        target: [f32; 3],
    ) -> Option<[f32; 2]> {
        const STEP_S: f32 = 0.05;
        let ps = self.ps(slot)?;
        let w = self.arms.get(&slot)?.weapon(weapon);
        let def: &WeaponDef = level.weapon_defs().get(w as usize).filter(|_| w > 0)?;
        let (world, _) = self.world.as_ref()?;
        let radius = def.explosion_radius as f32;
        let target = Vec3::from(target);
        let from = eye(ps);
        let to = target - from;
        let yaw = to.y.atan2(to.x).to_degrees();
        let fuse_s = def.fuse_time.max(1000) as f32 / 1000.0;
        let mut best: Option<([f32; 2], f32)> = None;
        for up in (-2..=17).map(|i| i as f32 * 5.0) {
            let (sp, cp) = up.to_radians().sin_cos();
            let (sy, cy) = yaw.to_radians().sin_cos();
            let mut v = Vec3::new(cp * cy, cp * sy, sp) * def.projectile_speed as f32;
            v.z += def.projectile_speed_up as f32;
            let mut at = from;
            let mut t = 0.0;
            let mut landed = None;
            while t < fuse_s {
                let next = at + v * STEP_S - Vec3::Z * 400.0 * STEP_S * STEP_S;
                v.z -= 800.0 * STEP_S;
                let tr = world.collision.shot_trace(at, next);
                if tr.fraction < 1.0 {
                    landed = Some(at + (next - at) * tr.fraction);
                    break;
                }
                at = next;
                t += STEP_S;
            }
            let burst = landed.unwrap_or(at);
            if burst.distance(ps.origin) <= radius {
                continue;
            }
            let miss = burst.distance(target);
            if miss < super::LOB_MISS && best.is_none_or(|(_, m)| miss < m) {
                best = Some(([-up, yaw], miss));
            }
        }
        best.map(|(aim, _)| aim)
    }

    /// The brain's `cmd` in 1.7's bits, its weapon back in CoD4's numbers,
    /// with the frag and the mantle over it, stamped for the tick; `None`
    /// with no body.
    pub(crate) fn cmd(
        &mut self,
        slot: usize,
        cmd: UserCmd,
        view: &BotView,
    ) -> Option<Vec<UserCmd4>> {
        let ps = self.ps(slot)?.clone();
        let arms = self.arms.get(&slot).cloned().unwrap_or_default();
        let throwing = self.throwing.remove(&slot);
        // A body out of play throws nothing.
        let grenade = view
            .grenade
            .filter(|_| view.playing)
            .map(|g| (g, arms.weapon(g)));
        let (mut c4, armed) = offhand_cmd(&cmd, grenade, ps.weapon, throwing);
        if let Some(armed) = armed {
            self.throwing.insert(slot, armed);
        }
        if armed.is_none() {
            c4.weapon = arms.weapon(cmd.weapon) as u8;
            if c4.weapon == 0 {
                c4.weapon = ps.weapon as u8;
            }
        }
        // A script's switch until it lands, or a weapon never given.
        if let Some((w, left)) = self.switches.get_mut(&slot) {
            let held = ps.has_weapon(*w);
            *left = left.saturating_sub(u8::from(!held));
            if ps.weapon == *w || *left == 0 {
                self.switches.remove(&slot);
            } else if held && armed.is_none() {
                c4.weapon = *w as u8;
            }
        }
        // A mantle edge: on to the ledge with the jump key once pmove
        // offers the climb (`Mantle_Check`'s hint).
        // Once: a body that climbed the edge and comes down past its
        // landing in the air turns back for the node, and the wall's back
        // offered a second climb. mp_crash's sill at (-524, 1952, 290) is a
        // `MANTLE_OVER` east to west and a crouched climb onto its top back
        // east, under a lintel a standing body does not fit beneath
        // (bot-objectives.md 12.5).
        match self.mantling.get(&slot).copied() {
            Some(edge) => {
                c4.buttons &= !button::JUMP;
                if ps.pm_flags & pmf::MANTLE != 0 {
                    self.climbed.insert(slot, edge);
                } else if self.climbed.get(&slot) != Some(&edge)
                    && ps.mantle.flags & vcod_cod4::pmove::mantle::flag::HINT != 0
                {
                    c4.buttons |= button::JUMP;
                }
            }
            None => {
                self.climbed.remove(&slot);
            }
        }
        if self.crouched.contains(&slot) {
            c4.buttons = (c4.buttons & !button::PRONE) | button::CROUCH;
        }
        if c4.offhand == 0 {
            c4.offhand = ps.off_hand_index as u8;
        }
        self.apply_steps(slot, &mut c4, &ps);
        let climbing = view.on_ladder && !view.on_ground;
        Some(
            crate::nav::tick_times(self.pass.now, climbing)
                .map(|t| UserCmd4 {
                    server_time: t,
                    ..c4
                })
                .collect(),
        )
    }

    /// The tick's end.
    pub(crate) fn tick_done(&mut self) {
        self.pass = Pass::default();
    }
}

/// A plant or defuse holds use this much past the script's use time: the
/// switch to the briefcase first (`useHoldThinkLoop` counts only once it is
/// in hand, and waits at most 1.5 s for it), and a margin.
const KEY_SLACK_TICKS: u32 = 40;
/// A script's `switchToWeapon` is asked for this many ticks at most before
/// the weapon is in the loadout.
const SWITCH_TICKS: u8 = 20;

/// The standing body's height (`playerMaxs`): the brain's sites count a
/// body in a zone whose box its own meets.
const STAND_HEIGHT: f32 = 70.0;

/// The other side of `team`.
fn other(team: i32) -> i32 {
    match team {
        TEAM_AXIS => TEAM_ALLIES,
        TEAM_ALLIES => TEAM_AXIS,
        _ => 0,
    }
}

/// The tick's objective state ([`super::driver::BotWorld::Frame`]).
#[derive(Default)]
pub(crate) struct Cod4Frame(Option<Cod4Objectives>);

impl Cod4Bots {
    /// The stock objective scripts' state, read once a tick
    /// (`BotWorld::frame`); nothing with [`Self::objectives`] off.
    pub(crate) fn frame(&mut self, level: &mut Cod4Level) -> Cod4Frame {
        let frame = Cod4Frame(level.objectives().filter(|_| self.objectives));
        self.read_guns(level, frame.0.is_none());
        self.pass.blockers = level.shot_blockers();
        frame
    }

    /// Pass 2's objective roles (`BotWorld::deal`).
    pub(crate) fn deal(
        &mut self,
        frame: Cod4Frame,
        teams: &[i32],
        round: u32,
        views: &mut [(usize, BotView)],
    ) {
        match &frame.0 {
            Some(Cod4Objectives::Bomb(g)) => self.deal_bomb(g, teams, round, views),
            Some(Cod4Objectives::Zones(z)) => self.deal_zones(z, teams, views),
            None => {}
        }
    }

    fn site(&mut self, (mins, maxs): Bounds) -> SiteView {
        SiteView {
            mins,
            maxs,
            stand: self.driver.zone_stand(mins, maxs),
        }
    }

    /// `sd` and `sab` as CoD1's S&D view (`Server`'s `BotWorld::deal`,
    /// bot-objectives.md 2 and 12): a team attacks the zones it plants at
    /// while it carries the bomb or may take it lying, and defends its own
    /// otherwise; once the bomb is down its planters guard it and the other
    /// side's bot nearest it defuses. The bomb is a key object
    /// ([`KeyBomb`]): only its carrier plants, the team's playing bot
    /// nearest it lying walks into it, the rest guard the zone they would
    /// plant at.
    fn deal_bomb(
        &mut self,
        g: &BombGame,
        teams: &[i32],
        round: u32,
        views: &mut [(usize, BotView)],
    ) {
        let team = |slot: usize| teams.get(slot).copied().unwrap_or(0);
        let ticks = |ms: i32| (ms.max(0) as u32).div_ceil(50) + KEY_SLACK_TICKS;
        let (plant_ticks, defuse_ticks) = (ticks(g.plant_ms), ticks(g.defuse_ms));
        let zones: Vec<(i32, SiteView)> = g.zones.iter().map(|&(t, b)| (t, self.site(b))).collect();
        let planted = g.planted.map(|p| {
            let site = self.site(p.zone);
            let aim = std::array::from_fn(|i| (p.zone.0[i] + p.zone.1[i]) * 0.5);
            (p.by, BombView { origin: p.at, aim }, site)
        });
        let carrier_team = g.carrier.map(team);
        let near = |slots: &mut dyn Iterator<Item = (usize, f32)>| {
            slots
                .min_by(|a, b| a.1.total_cmp(&b.1).then(a.0.cmp(&b.0)))
                .map(|a| a.0)
        };
        // Each team's playing bot nearest the bomb lying, if it may take it.
        let fetcher = |t: i32| -> Option<usize> {
            let (at, who) = g.lying?;
            if who != 0 && who != t {
                return None;
            }
            near(
                &mut views
                    .iter()
                    .filter(|(s, v)| team(*s) == t && v.playing)
                    .map(|(s, v)| (*s, super::dist_sq(v.origin, at))),
            )
        };
        let fetchers = [fetcher(TEAM_AXIS), fetcher(TEAM_ALLIES)];
        let bomb_at = planted.map(|p| p.1.origin);
        let lead = |t: i32| -> Option<usize> {
            let at = bomb_at?;
            near(
                &mut views
                    .iter()
                    .filter(|(s, v)| team(*s) == t && v.playing)
                    .map(|(s, v)| (*s, super::dist_sq(v.origin, at))),
            )
        };
        let leads = [lead(TEAM_AXIS), lead(TEAM_ALLIES)];
        let planting: Vec<(usize, [f32; 3], bool)> = views
            .iter()
            .map(|(s, v)| {
                let held = v.playing
                    && v.linked
                    && zones
                        .iter()
                        .any(|(t, z)| *t == team(*s) && super::inside_site(v.origin, z));
                (*s, v.origin, held)
            })
            .collect();
        let roster: Vec<(usize, i32)> = views.iter().map(|(s, _)| (*s, team(*s))).collect();
        for (slot, view) in views.iter_mut() {
            let mine = team(*slot);
            let side = match mine {
                TEAM_AXIS => 0,
                TEAM_ALLIES => 1,
                _ => continue,
            };
            let (role, sites, bomb) = match planted {
                Some((by, b, site)) => {
                    let role = if by == mine {
                        ObjRole::Attack
                    } else {
                        ObjRole::Defend
                    };
                    (role, vec![site], Some(b))
                }
                None => {
                    let of = |t: i32| -> Vec<SiteView> {
                        zones.iter().filter(|z| z.0 == t).map(|z| z.1).collect()
                    };
                    let (ours, theirs) = (of(mine), of(other(mine)));
                    let may_take = g.lying.is_some_and(|(_, who)| who == 0 || who == mine);
                    if !ours.is_empty() && (carrier_team == Some(mine) || may_take) {
                        (ObjRole::Attack, ours, None)
                    } else {
                        (ObjRole::Defend, theirs, None)
                    }
                }
            };
            if sites.is_empty() && bomb.is_none() {
                continue;
            }
            let attack = role == ObjRole::Attack;
            let mates: Vec<usize> = roster.iter().filter(|r| r.1 == mine).map(|r| r.0).collect();
            let alone = self.driver.bots.get(slot).is_some_and(|b| !b.team_play);
            let rank = if alone {
                mates.iter().filter(|s| *s < slot).count()
            } else {
                super::team::site_rank(attack, *slot, &mates, mine, round)
            };
            let cover = (attack && !alone)
                .then(|| {
                    let mates: Vec<_> = planting
                        .iter()
                        .filter(|p| team(p.0) == mine)
                        .copied()
                        .collect();
                    super::team::cover(*slot, view.origin, &mates)
                })
                .flatten();
            view.sd = Some(SdView {
                role,
                sites,
                bomb,
                rank,
                lead: leads[side] == Some(*slot),
                cover,
                key: Some(KeyBomb {
                    fetch: g
                        .lying
                        .filter(|_| fetchers[side] == Some(*slot) && planted.is_none())
                        .map(|l| l.0),
                    mine: g.carrier == Some(*slot),
                    plant_ticks,
                    defuse_ticks,
                }),
            });
        }
    }

    /// `dom`'s flags and `koth`'s radio as United Offensive's domination
    /// view (`Server::bot_uo_view`): a zone not up yet reads as held, so
    /// the bots wait round it. The rank is the bot's place among its team's
    /// bots by slot.
    fn deal_zones(&mut self, zones: &[Zone], teams: &[i32], views: &mut [(usize, BotView)]) {
        let team = |slot: usize| teams.get(slot).copied().unwrap_or(0);
        // A zone counts a body only while its box meets the trigger
        // (`_gameobjects.gsc` `triggerTouchThink`'s `isTouching`): the
        // feet stand on a floor inside it, not a storey under it with the
        // head poking in. mp_crash's radio trigger at (524, 1288, 332)
        // spans z 271..393 over a stair whose nodes at 201..209 the brain
        // read as in it, and nobody ever touched it (bot-objectives.md
        // 12.4).
        let floor = |(lo, hi): Bounds| -> Bounds { ([lo[0], lo[1], lo[2] + STAND_HEIGHT], hi) };
        let sites: Vec<(SiteView, Zone)> = zones
            .iter()
            .map(|z| (self.site(floor(z.bounds)), *z))
            .collect();
        let slots: Vec<usize> = views.iter().map(|(s, _)| *s).collect();
        for (slot, view) in views.iter_mut() {
            let mine = team(*slot);
            if mine != TEAM_AXIS && mine != TEAM_ALLIES {
                continue;
            }
            let rank = slots
                .iter()
                .filter(|s| team(**s) == mine && *s < slot)
                .count();
            let flags = sites
                .iter()
                .map(|(site, z)| DomFlagView {
                    site: *site,
                    ours: z.team == mine || !z.live,
                    contested: z.live && z.contested,
                })
                .collect();
            view.uo = Some(UoView::Dom(DomView { flags, rank }));
        }
    }
}

/// The bots' world over the level's clip map and the brush entities
/// `brushes` (the level's solid ones once its gametype has set them up,
/// [`Cod4Level::brushes`]): every trace through it
/// (`CollisionWorld::set_world_clip`, as `game::cod4::clip::level_world`),
/// the graph flooded from the map's spawn points.
pub fn bot_world(
    clip: Arc<ClipMap>,
    entities: &[Vec<(String, String)>],
    brushes: Vec<crate::game::cod4::clip::Brush4>,
) -> Rc<World> {
    let map = vcod_common::mapworld::MapWorld::default();
    let mut collision = vcod_common::collision::CollisionWorld::build(&map, &[]);
    if brushes.is_empty() {
        collision.set_world_clip(clip);
    } else {
        collision.set_world_clip(Arc::new(BrushedClip::new(clip, brushes)));
    }
    Rc::new(World {
        collision,
        vis: map.visibility(),
        spawn: ([0.0; 3], 0.0),
        spawn_points: spawn_points(entities),
        hazards: Vec::new(),
        rubble: Vec::new(),
        nav_cover: Vec::new(),
    })
}

/// Every spawn point's origin among the map's entities, as
/// [`crate::nav::spawn_points`] reads a BSP's (`mp_*_spawn*` classes).
pub fn spawn_points(entities: &[Vec<(String, String)>]) -> Vec<[f32; 3]> {
    entities
        .iter()
        .filter(|e| {
            e.iter()
                .any(|(k, v)| k == "classname" && v.starts_with("mp_") && v.contains("_spawn"))
        })
        .filter_map(|e| {
            let (_, o) = e.iter().find(|(k, _)| k == "origin")?;
            vcod_common::bsp::parse_vec3(o)
        })
        .collect()
}

/// The step the graph's walks move by: CoD1's pmove under
/// [`GRAPH_RULES`], or a [`Cod4Walk`] with `VC_NAV_COD4=1`. Measured and
/// left off: over six seeds of the 210-pair census it stalls 3% more
/// (bot-objectives.md 12.10 to 12.12).
pub fn walk_step(
    clip: Arc<ClipMap>,
    rules: vcod_cod4::pmove::Rules,
    entities: &[Vec<(String, String)>],
) -> Option<Arc<dyn crate::nav::WalkStep>> {
    if std::env::var("VC_NAV_COD4").as_deref() != Ok("1") {
        return None;
    }
    Some(Arc::new(Cod4Walk::new(clip, rules, map_brushes(entities))))
}

/// The map's `script_brushmodel`s where they stand at load, as the level
/// links them: a body's pmove clips them. mp_cargoship's `pipe_shootable`
/// (`*76` at (1300, 61, 104)) caps the jump up the deck's step at (1313,
/// 36, 80) that the clip map alone lets through (bot-objectives.md 12.11).
pub fn map_brushes(entities: &[Vec<(String, String)>]) -> Vec<crate::game::cod4::clip::Brush4> {
    fn get<'a>(e: &'a [(String, String)], k: &str) -> Option<&'a str> {
        e.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str())
    }
    entities
        .iter()
        .enumerate()
        .filter(|(_, e)| get(e, "classname") == Some("script_brushmodel"))
        .filter_map(|(i, e)| {
            let model = get(e, "model")?.strip_prefix('*')?.parse().ok()?;
            let origin = get(e, "origin")
                .and_then(vcod_common::bsp::parse_vec3)
                .unwrap_or([0.0; 3]);
            let angles = get(e, "angles")
                .and_then(vcod_common::bsp::parse_vec3)
                .unwrap_or([0.0; 3]);
            Some(crate::game::cod4::clip::Brush4 {
                entity: 1 + i as u32,
                model,
                origin: origin.into(),
                angles: angles.into(),
            })
        })
        .collect()
}

/// The graph cache's mark for a world with brush entities `brushes`:
/// two gametypes on one map can leave different ones standing.
fn brush_key(brushes: &[crate::game::cod4::clip::Brush4]) -> String {
    use std::hash::{Hash, Hasher};
    if brushes.is_empty() {
        return String::new();
    }
    let mut h = std::collections::hash_map::DefaultHasher::new();
    format!("{brushes:?}").hash(&mut h);
    format!(" (brushes {:016x})", h.finish())
}

/// The clip map and the level's solid brush entities, as a bot's pmove
/// clips them ([`crate::game::cod4::clip::Cod4Clip`]): the bots' world.
/// Over the clip map alone the graph's CoD1 walks proved mp_cargoship's
/// jump up the deck's step at (1313, 36, 80) under `pipe_shootable`,
/// which a live bot hits its head on (bot-objectives.md 12.11, 12.15).
struct BrushedClip {
    clip: Arc<ClipMap>,
    /// Each brush entity with its world box (its submodel's bounding
    /// sphere round the origin), which a sweep must meet to be traced.
    brushes: Vec<(crate::game::cod4::clip::Brush4, Vec3, Vec3)>,
}

impl BrushedClip {
    fn new(clip: Arc<ClipMap>, brushes: Vec<crate::game::cod4::clip::Brush4>) -> BrushedClip {
        let brushes = brushes
            .into_iter()
            .filter(|b| b.model != 0 && b.model < clip.submodels.len())
            .map(|b| {
                let s = &clip.submodels[b.model];
                let r = Vec3::splat(s.mins.abs().max(s.maxs.abs()).length() + 1.0);
                (b, b.origin - r, b.origin + r)
            })
            .collect();
        BrushedClip { clip, brushes }
    }

    /// The brushes whose box the sweep's box meets.
    fn near(&self, lo: Vec3, hi: Vec3) -> impl Iterator<Item = &crate::game::cod4::clip::Brush4> {
        self.brushes
            .iter()
            .filter(move |(_, blo, bhi)| lo.cmple(*bhi).all() && hi.cmpge(*blo).all())
            .map(|(b, _, _)| b)
    }
}

impl vcod_common::collision::WorldClip for BrushedClip {
    fn trace(
        &self,
        start: Vec3,
        end: Vec3,
        mins: Vec3,
        maxs: Vec3,
        mask: u32,
    ) -> vcod_common::collision::WorldHit {
        use vcod_cod4::pmove::PmWorld;
        let near: Vec<_> = self
            .near(start.min(end) + mins, start.max(end) + maxs)
            .copied()
            .collect();
        let world = crate::game::cod4::clip::Cod4Clip {
            clip: &self.clip,
            bodies: &[],
            brushes: &near,
        };
        let none = vcod_cod4::pmove::ENTITYNUM_NONE as u32;
        let t = world.trace(start, end, mins, maxs, none, mask);
        vcod_common::collision::WorldHit {
            fraction: t.fraction,
            normal: t.normal,
            surface_flags: t.surface_flags,
            contents: t.contents,
            startsolid: t.startsolid,
            allsolid: t.allsolid,
        }
    }

    fn point_contents(&self, p: Vec3) -> u32 {
        self.near(p, p).fold(self.clip.point_contents(p), |c, b| {
            let q = vcod_common::turretpose::angles_quat(b.angles.to_array());
            c | self
                .clip
                .point_contents_model(b.model, q.inverse() * (p - b.origin))
        })
    }

    fn ladder_boxes(&self) -> Vec<(Vec3, Vec3)> {
        self.clip.ladder_boxes()
    }
}

/// The nav walks' bodies under 1.7's pmove ([`crate::nav::WalkStep`]):
/// each walk's CoD1 body is the read of a CoD4 body stepped over the
/// level's clip map and `rules`, its cmd through [`to_cmd4`], as
/// `ClientThink` steps a bot's. Under CoD1's pmove the graph held jumps
/// onto mp_killhouse's crates 37 up and mp_cargoship's deck 28 up that a
/// CoD4 body falls back from (bot-objectives.md 12.9). Off by default
/// ([`walk_step`], 12.10).
pub struct Cod4Walk {
    clip: Arc<ClipMap>,
    rules: vcod_cod4::pmove::Rules,
    brushes: Vec<crate::game::cod4::clip::Brush4>,
    key: String,
}

impl Cod4Walk {
    pub fn new(
        clip: Arc<ClipMap>,
        rules: vcod_cod4::pmove::Rules,
        brushes: Vec<crate::game::cod4::clip::Brush4>,
    ) -> Cod4Walk {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        format!("{rules:?} {brushes:?}").hash(&mut h);
        Cod4Walk {
            clip,
            rules,
            brushes,
            key: format!("cod4 {:016x}", h.finish()),
        }
    }
}

/// The CoD4 body a walk carries between cmds ([`Cod4Walk`]), the cmd
/// before it, and where it left the walk's CoD1 body.
#[derive(Clone)]
struct WalkBody {
    ps: PlayerState,
    old: vcod_cod4::pmove::Cmd,
    origin: Vec3,
    velocity: Vec3,
}

/// Shorts to degrees and back, for `delta_angles` (1.7 keeps floats).
const SHORT_DEG: f64 = 360.0 / 65536.0;

impl crate::nav::WalkStep for Cod4Walk {
    fn step(
        &self,
        ps: &mut vcod_common::pmove::PlayerState,
        delta: &mut [i32; 3],
        carry: &mut crate::nav::WalkCarry,
        from_ms: i32,
        cmd: &UserCmd,
        _world: &vcod_common::collision::CollisionWorld,
        _weapons: &[Option<vcod_common::weapon::WeaponDef>],
    ) {
        use vcod_cod4::pmove::{Cmd, WeaponMove};
        // The walk's own CoD1 body is the truth when it moved it since.
        let mut body = carry
            .take()
            .and_then(|c| c.downcast::<WalkBody>().ok())
            .map(Arc::unwrap_or_clone)
            .filter(|b| b.origin == ps.origin && b.velocity == ps.velocity)
            .unwrap_or_else(|| WalkBody {
                ps: cod4_walk_body(ps, from_ms),
                old: Cmd::default(),
                origin: ps.origin,
                velocity: ps.velocity,
            });
        body.ps.delta_angles = delta.map(|d| (f64::from(d) * SHORT_DEG) as f32);
        // Weapon 0: no weapon scale, the graph's 190 units/s.
        let mut c = Cmd::from(&to_cmd4(cmd));
        c.weapon = 0;
        vcod_cod4::pmove::player_step(
            &mut body.ps,
            &c,
            &mut body.old,
            &crate::game::cod4::clip::Cod4Clip {
                clip: &self.clip,
                bodies: &[],
                brushes: &self.brushes,
            },
            self.rules,
            WeaponMove::NONE,
        );
        *delta = body
            .ps
            .delta_angles
            .map(|d| (f64::from(d) / SHORT_DEG).round() as i32);
        read_walk_body(&body.ps, ps, c.buttons & button::JUMP != 0, &self.clip);
        body.origin = ps.origin;
        body.velocity = ps.velocity;
        *carry = Some(Arc::new(body));
    }

    fn key(&self) -> String {
        self.key.clone()
    }
}

/// A walk's first CoD4 body off its CoD1 one, its clock at `time`: moving
/// as the CoD1 body moves, its last jump `since_jump_ms` back.
fn cod4_walk_body(ps: &vcod_common::pmove::PlayerState, time: i32) -> PlayerState {
    use vcod_cod4::pmove::{ENTITYNUM_NONE, VIEW_CROUCH, VIEW_PRONE};
    use vcod_common::pmove::Stance;
    let mut b = PlayerState::spawn(ps.origin, ps.yaw.to_degrees(), time);
    b.view_angles[0] = ps.pitch.to_degrees();
    b.velocity = ps.velocity;
    b.old_velocity = [ps.velocity.x, ps.velocity.y];
    b.speed = ps.speed as i32;
    b.gravity = ps.gravity as i32;
    b.ground_entity_num = if ps.on_ground {
        ps.ground_entity as i32
    } else {
        ENTITYNUM_NONE
    };
    if ps.on_ladder {
        b.pm_flags |= pmf::LADDER;
        b.ladder_vec = ps.ladder_normal;
    }
    let view = match ps.stance {
        Stance::Stand => None,
        Stance::Crouch => Some((pmf::DUCKED, VIEW_CROUCH)),
        Stance::Prone => Some((pmf::PRONE, VIEW_PRONE)),
    };
    if let Some((flag, height)) = view {
        b.pm_flags |= flag;
        b.view_height_target = height;
        b.view_height_current = height as f32;
    }
    // `PM_CheckJump` refuses a jump inside 500 ms of `jumpTime`.
    b.jump_time = time - ps.since_jump_ms.min(10_000.0) as i32;
    b.jump_origin_z = ps.jump_origin_z;
    if ps.jump_penalty {
        b.pm_flags |= pmf::JUMPING;
        b.pm_time = ps.knockback_ms as i32;
    }
    b
}

/// What the walk reads off the CoD4 body `b` into its CoD1 body `ps`; a
/// body off the ground over ground too steep to stand on reads sliding.
fn read_walk_body(
    b: &PlayerState,
    ps: &mut vcod_common::pmove::PlayerState,
    jump_held: bool,
    clip: &ClipMap,
) {
    use vcod_common::pmove::Stance;
    ps.origin = b.origin;
    ps.velocity = b.velocity;
    ps.yaw = b.view_angles[1].to_radians();
    ps.pitch = b.view_angles[0].to_radians();
    ps.on_ground = b.on_ground();
    if ps.on_ground {
        ps.ground_entity = b.ground_entity_num as u32;
    }
    ps.on_ladder = b.pm_flags & pmf::LADDER != 0;
    ps.ladder_normal = b.ladder_vec;
    ps.stance = match b.effective_stance() {
        vcod_cod4::pmove::Stance::Stand => Stance::Stand,
        vcod_cod4::pmove::Stance::Crouch => Stance::Crouch,
        vcod_cod4::pmove::Stance::Prone => Stance::Prone,
    };
    ps.ducked = b.pm_flags & (pmf::DUCKED | pmf::PRONE) != 0;
    ps.since_jump_ms = (b.command_time - b.jump_time).max(0) as f32;
    ps.jump_penalty = b.pm_flags & pmf::JUMPING != 0;
    ps.knockback_ms = b.pm_time as f32;
    ps.jump_origin_z = b.jump_origin_z;
    ps.jump_latched = jump_held;
    let plane = if ps.on_ground {
        Some(Vec3::Z)
    } else {
        use vcod_cod4::pmove::{MASK_PLAYERSOLID, PLAYER_MAXS, PLAYER_MINS};
        let t = clip.trace(
            b.origin,
            b.origin - Vec3::Z * 0.25,
            PLAYER_MINS,
            PLAYER_MAXS,
            MASK_PLAYERSOLID,
        );
        (!t.startsolid && t.fraction < 1.0).then_some(t.normal)
    };
    ps.set_ground_plane(plane);
}

/// How far ahead of a node a wall flagged for the mantle is looked for
/// before the climb is walked, and the walk's and the whole probe's ticks,
/// as CoD2's.
const MANTLE_LOOK: f32 = 64.0;
const MANTLE_WALK_TICKS: usize = 20;
const MANTLE_TICKS: usize = 70;

/// How far a bot's climb may land from the compass walk's and still be
/// the same edge: inside the landing's grid cell, within a step up or down.
const MANTLE_AGREE_XY: f32 = 16.0;
const MANTLE_AGREE_Z: f32 = 18.0;

/// The graph's mantle probe ([`crate::nav::MantleProbe`]): [`mantle_walk`]
/// over the clip map, under the level's `rules`, kept only when a bot
/// facing its landing climbs it too ([`climbs_facing`]). On mp_crash a
/// climb found facing 315 off (672, 964, 174) slid along a stair into the
/// few units of wall that offer it; a bot facing the landing (800, 964,
/// 139) was never offered it and ran at the wall for the rest of the
/// round (bot-objectives.md 12.6). The probe's
/// own world argument is the same clip map behind the shared hook.
pub fn mantle_probe(clip: Arc<ClipMap>, rules: vcod_cod4::pmove::Rules) -> crate::nav::MantleProbe {
    Arc::new(
        move |_: &vcod_common::collision::CollisionWorld, from: Vec3, yaw: f32| {
            let q = mantle_walk(&clip, rules, from, yaw)?;
            climbs_facing(&clip, rules, from, q).then_some(q)
        },
    )
}

/// Whether a body run at `land` from `from`, as a bot following the edge
/// faces, climbs and lands there too.
fn climbs_facing(clip: &ClipMap, rules: vcod_cod4::pmove::Rules, from: Vec3, land: Vec3) -> bool {
    let d = land - from;
    let facing = d.y.atan2(d.x).to_degrees();
    mantle_walk(clip, rules, from, facing).is_some_and(|r| {
        (r - land).truncate().length() <= MANTLE_AGREE_XY && (r.z - land.z).abs() <= MANTLE_AGREE_Z
    })
}

/// Where a CoD4 body standing at `from` and run at the wall along `yaw`
/// stands once it mantles (cod4-pmove.md 17): only off a wall flagged
/// `MANTLE_ON` or `MANTLE_OVER` within [`MANTLE_LOOK`]; the body walks at
/// it, presses the jump key on the first step pmove offers the climb, as a
/// bot does, and the climb's end on the ground is the answer.
pub fn mantle_walk(
    clip: &ClipMap,
    rules: vcod_cod4::pmove::Rules,
    from: Vec3,
    yaw: f32,
) -> Option<Vec3> {
    use vcod_cod4::pmove::mantle::{CONTENTS_MANTLE, SURF_MANTLE_ON, SURF_MANTLE_OVER, flag};
    use vcod_cod4::pmove::{Cmd, WeaponMove};
    if !rules.mantle_enable || rules.mantle_anims.is_none() {
        return None;
    }
    let (s, c) = yaw.to_radians().sin_cos();
    let dir = Vec3::new(c, s, 0.0);
    let r = rules.mantle_check_radius.max(1.0);
    let t = ClipMap::trace(
        clip,
        from,
        from + dir * MANTLE_LOOK,
        Vec3::new(-r, -r, 0.0),
        Vec3::new(r, r, 70.0),
        CONTENTS_MANTLE,
    );
    if t.startsolid
        || t.fraction >= 1.0
        || t.surface_flags & (SURF_MANTLE_ON | SURF_MANTLE_OVER) == 0
    {
        return None;
    }
    let mut ps = PlayerState::spawn(from + Vec3::Z * 0.125, yaw, 0);
    let angle = ((f64::from(yaw) * 65536.0 / 360.0).round() as i64 & 0xffff) as u16;
    let mut old = Cmd::default();
    let mut mantled = false;
    for tick in 0..MANTLE_TICKS {
        if !mantled && tick >= MANTLE_WALK_TICKS {
            return None;
        }
        let jump = !mantled && ps.mantle.flags & flag::HINT != 0;
        let cmd = Cmd {
            server_time: ps.command_time + 50,
            buttons: if jump { button::JUMP } else { 0 },
            angles: [0, angle, 0],
            forward_move: if mantled { 0 } else { 127 },
            ..Default::default()
        };
        vcod_cod4::pmove::player_step(&mut ps, &cmd, &mut old, clip, rules, WeaponMove::NONE);
        if ps.pm_flags & pmf::MANTLE != 0 {
            mantled = true;
        } else if mantled && ps.on_ground() {
            return Some(ps.origin);
        } else if !mantled && jump {
            return None;
        }
    }
    None
}

/// A weapon's time between rounds as the brain paces its trigger: a bolt
/// gun rechambers after its `fireTime`.
fn fire_cycle(d: &WeaponDef) -> i32 {
    if d.bolt_action {
        d.fire_time + d.rechamber_time
    } else {
        d.fire_time
    }
}

fn eye(ps: &PlayerState) -> Vec3 {
    ps.origin + Vec3::Z * ps.view_height_current
}

/// `ANGLE2SHORT` of 1.7's float `delta_angles`, as the brain's cmd angles
/// subtract them.
fn deg_short(a: f32) -> i32 {
    (a * (65536.0 / 360.0)) as i32
}

/// The cmd the brain asks for, in 1.7's bits, and the frag throw's state
/// to keep, as CoD2's [`super::cod2::offhand_cmd`]: while the brain asks
/// for the frag (`grenade`, its brain number and CoD4 weapon) the weapon
/// byte stays on the gun in hand (`held`), the frag key is held from the
/// ask until the brain has held the trigger and let it go, and the trigger
/// itself goes nowhere. `None` once the brain asks for the frag no more.
pub fn offhand_cmd(
    cmd: &UserCmd,
    grenade: Option<(u8, u32)>,
    held: u32,
    throwing: Option<bool>,
) -> (UserCmd4, Option<bool>) {
    let mut c = to_cmd4(cmd);
    let Some((_, g)) = grenade.filter(|&(b, _)| cmd.weapon == b) else {
        return (c, None);
    };
    c.weapon = held as u8;
    let trigger = c.buttons & button::ATTACK != 0;
    c.buttons &= !(button::ATTACK | button::ADS_MODE);
    let armed = throwing.unwrap_or(false) || trigger;
    if trigger || !armed {
        c.buttons |= button::FRAG;
    }
    c.offhand = g as u8;
    (c, Some(armed))
}

/// The reload key (`PM_Weapon_CheckForReload`'s 0x10) and the use key
/// (0x8, `ClientThink`'s 0x28 reads it).
const BUTTON_RELOAD: u32 = 0x10;
const BUTTON_USE: u32 = 0x8;

/// A CoD1 usercmd in 1.7's bits (`vcod_cod4::pmove::button`): the attack,
/// melee, use and sight to 0x1, 0x4, 0x8 and 0x800; the reload, the leans
/// and the two stances out of `wbuttons` to 0x10, 0x40/0x80 and
/// 0x100/0x200; a jump is 0x400 and a crouch held through `up` is 0x200.
/// The moves go whole, as the wire carries only their sign.
pub fn to_cmd4(cmd: &UserCmd) -> UserCmd4 {
    let mut b = 0;
    let map = [
        (cmd.buttons & msg::BUTTON_ATTACK, button::ATTACK),
        (cmd.buttons & msg::BUTTON_MELEE, button::MELEE),
        (cmd.buttons & msg::BUTTON_USE, BUTTON_USE),
        (cmd.buttons & msg::BUTTON_ADS, button::ADS_MODE),
        (cmd.wbuttons & msg::WBUTTON_RELOAD, BUTTON_RELOAD),
        (cmd.wbuttons & msg::WBUTTON_LEAN_LEFT, button::LEAN_LEFT),
        (cmd.wbuttons & msg::WBUTTON_LEAN_RIGHT, button::LEAN_RIGHT),
        (cmd.wbuttons & msg::WBUTTON_PRONE, button::PRONE),
        (cmd.wbuttons & msg::WBUTTON_CROUCH, button::CROUCH),
    ];
    for (on, bit) in map {
        if on != 0 {
            b |= bit;
        }
    }
    if cmd.up > 0 {
        b |= button::JUMP;
    } else if cmd.up < 0 && b & button::PRONE == 0 {
        b |= button::CROUCH;
    }
    let whole = |m: i8| (i32::from(m).signum() * 127) as i8;
    UserCmd4 {
        server_time: cmd.server_time,
        buttons: b,
        angles: cmd.angles.map(|a| a as u16),
        weapon: cmd.weapon,
        offhand: 0,
        forwardmove: whole(cmd.forward),
        rightmove: whole(cmd.right),
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cod1_cmd_maps_onto_cod4_bits_with_whole_moves() {
        let cmd = UserCmd {
            server_time: 1000,
            buttons: msg::BUTTON_ATTACK | msg::BUTTON_ADS,
            wbuttons: msg::WBUTTON_RELOAD | msg::WBUTTON_LEAN_LEFT,
            weapon: 3,
            angles: [-100, 16384, 0],
            forward: 64,
            right: -5,
            up: 127,
            ..Default::default()
        };
        let c = to_cmd4(&cmd);
        assert_eq!(
            c.buttons,
            button::ATTACK | button::ADS_MODE | 0x10 | button::LEAN_LEFT | button::JUMP
        );
        assert_eq!((c.forwardmove, c.rightmove), (127, -127));
        assert_eq!(c.angles, [65436, 16384, 0]);
        let crouched = to_cmd4(&UserCmd {
            up: -127,
            ..Default::default()
        });
        assert_eq!(crouched.buttons, button::CROUCH);
    }

    #[test]
    fn a_weapon_keeps_its_brain_number() {
        let mut arms = Arms::default();
        for w in [130, 7, 95, 7] {
            arms.number(w);
        }
        assert_eq!([arms.brain(130), arms.brain(7), arms.brain(95)], [1, 2, 3]);
        assert_eq!(arms.weapon(3), 95);
        assert_eq!((arms.brain(0), arms.weapon(0), arms.weapon(9)), (0, 0, 0));
    }

    /// The frag key from the brain's ask until it lets the trigger go; the
    /// weapon byte stays on the gun in hand throughout.
    #[test]
    fn a_frag_throw_holds_the_frag_key_until_the_release() {
        let ask = |attack: bool| UserCmd {
            weapon: 4,
            buttons: if attack { msg::BUTTON_ATTACK } else { 0 },
            ..Default::default()
        };
        let frag = Some((4, 120));
        let (c, armed) = offhand_cmd(&ask(false), frag, 77, None);
        assert_eq!(
            (c.buttons, c.weapon, c.offhand, armed),
            (button::FRAG, 77, 120, Some(false))
        );
        let (c, armed) = offhand_cmd(&ask(true), frag, 77, armed);
        assert_eq!((c.buttons, armed), (button::FRAG, Some(true)));
        let (c, armed) = offhand_cmd(&ask(false), frag, 77, armed);
        assert_eq!((c.buttons, armed), (0, Some(true)));
        let (c, armed) = offhand_cmd(&ask(false), None, 77, armed);
        assert_eq!((c.weapon, armed), (4, None));
    }

    /// mp_crash's ledge at (745, 964, 174): the climb a walk facing 315
    /// finds off (676, 964, 174) is not offered to a body run at its
    /// landing, so the probe drops it; the climb east over the radio room's
    /// sill off (-536, 1952, 254) is offered both ways and kept
    /// (bot-objectives.md 12.5, 12.6).
    #[test]
    fn a_mantle_edge_is_kept_only_when_facing_its_landing_climbs_it() {
        let Some(install) = vcod_cod4::testing::cod4_install() else {
            return;
        };
        let zones = crate::game::cod4::Cod4Zones::load(&install, "mp_crash").unwrap();
        let clip = zones.clip.clone().unwrap();
        let rules = vcod_cod4::pmove::Rules::default();
        let world = bot_world(clip.clone(), &zones.map.entities, Vec::new());
        let probe = mantle_probe(clip.clone(), rules);
        let off = Vec3::new(676.0, 964.0, 174.0);
        assert!(mantle_walk(&clip, rules, off, 315.0).is_some());
        assert_eq!(probe(&world.collision, off, 315.0), None);
        let kept = Vec3::new(-536.0, 1952.0, 254.0);
        let land = probe(&world.collision, kept, 0.0).expect("the climb over the sill");
        assert!(
            (land - Vec3::new(-486.0, 1952.0, 252.6)).length() < 1.0,
            "{land}"
        );
    }

    /// mp_shipment's graph is two floods that never meet: the yard the
    /// spawns stand in (1771 nodes) and, larger, the out-of-bounds grounds
    /// round it, flooded from a ladder's ends at (62, 927, 192). The
    /// `koth` radio's stand comes from the spawns' part, inside the
    /// trigger, not 560 units off behind the yard's south wall
    /// (bot-objectives.md 12.8).
    #[test]
    fn a_zone_stand_comes_from_the_floor_the_spawns_flood() {
        let Some(install) = vcod_cod4::testing::cod4_install() else {
            return;
        };
        let zones = crate::game::cod4::Cod4Zones::load(&install, "mp_shipment").unwrap();
        let clip = zones.clip.clone().unwrap();
        let world = bot_world(clip.clone(), &zones.map.entities, Vec::new());
        let probe = mantle_probe(clip, vcod_cod4::pmove::Rules::default());
        let mut job = crate::nav::NavJob::start_with(
            "cod4 mp_shipment",
            &world,
            GRAPH_RULES,
            Some(probe),
            None,
        );
        let g = job.poll(true).expect("the graph");
        let (lo, hi) = ([-113.0, -89.0, 261.0], [121.0, 129.0, 289.0]);
        let stand = g.zone_stand(lo, hi).expect("a stand");
        assert!(
            (0..2).all(|i| stand[i] > lo[i] && stand[i] < hi[i]),
            "{stand:?}"
        );
    }

    /// mp_carentan's curb at (881, 1880): a terrain partition's border 8.3
    /// units high, its top 26 under the player capsule's centre, below
    /// the straight part. 1.7 meets it with the lower sphere (0x811626a);
    /// without that arm a bot walked into the terrain, all solid, and
    /// stood there for the rest of an `sab` match (bot-objectives.md 12.9).
    #[test]
    fn a_sideways_capsule_meets_a_low_terrain_border_with_its_sphere() {
        let Some(install) = vcod_cod4::testing::cod4_install() else {
            return;
        };
        use vcod_cod4::pmove::{MASK_PLAYERSOLID, PLAYER_MAXS, PLAYER_MINS};
        let zones = crate::game::cod4::Cod4Zones::load(&install, "mp_carentan").unwrap();
        let clip = zones.clip.clone().unwrap();
        let (a, b) = (
            Vec3::new(881.0, 1913.0, -47.875),
            Vec3::new(881.0, 1857.0, -47.875),
        );
        let t = clip.trace(a, b, PLAYER_MINS, PLAYER_MAXS, MASK_PLAYERSOLID);
        assert!(!t.startsolid && t.fraction < 0.35, "{t:?}");
        let stop = a.lerp(b, t.fraction);
        let held = clip.trace(stop, stop, PLAYER_MINS, PLAYER_MAXS, MASK_PLAYERSOLID);
        assert!(!held.startsolid, "{stop}");
    }

    /// mp_crash's ledge at (745, 964, 174) again: its probe climbs off
    /// (704, 964, 174) facing 315 and lands at (788, 953, 140), in the
    /// column of the node at (800, 964, 139) the edge ends at. A bot run
    /// at that node is never offered the climb; run at the landing the
    /// graph keeps, it climbs (bot-objectives.md 12.8).
    #[test]
    fn a_mantle_edge_keeps_the_landing_its_probe_climbed_to() {
        let Some(install) = vcod_cod4::testing::cod4_install() else {
            return;
        };
        let zones = crate::game::cod4::Cod4Zones::load(&install, "mp_crash").unwrap();
        let clip = zones.clip.clone().unwrap();
        let rules = vcod_cod4::pmove::Rules::default();
        let world = bot_world(clip.clone(), &zones.map.entities, Vec::new());
        let probe = mantle_probe(clip.clone(), rules);
        let mut job =
            crate::nav::NavJob::start_with("cod4 mp_crash", &world, GRAPH_RULES, Some(probe), None);
        let g = job.poll(true).expect("the graph");
        let node = |p: Vec3| {
            (0..g.nodes.len() as u32)
                .find(|&n| g.nodes[n as usize].distance(p) < 4.0)
                .expect("a node")
        };
        let (a, b) = (
            node(Vec3::new(704.0, 964.0, 174.0)),
            node(Vec3::new(800.0, 964.0, 139.0)),
        );
        assert!(g.mantles.contains(&(a, b)));
        let land = g.mantle_lands[&(a, b)];
        assert!(
            (land - Vec3::new(787.6, 953.1, 140.3)).length() < 1.0,
            "{land}"
        );
        let from = Vec3::new(715.0, 967.0, 174.0);
        let facing = |p: Vec3| (p - from).y.atan2((p - from).x).to_degrees();
        assert!(mantle_walk(&clip, rules, from, facing(land)).is_some());
        assert!(mantle_walk(&clip, rules, from, facing(g.nodes[b as usize])).is_none());
    }

    /// mp_killhouse's crate tops at z 60: the graph's leaps from there up
    /// 37 to (1310, 2557, 97) need a jump at the lip, which a CoD4 bot never
    /// reads (`BotWorld::PMOVE_PROBES` off), so it walked off the crate
    /// to the floor at 28 and back. Its graph has no leap: the edges a
    /// bot's own jump does not land go, and one it lands aimed beside the
    /// top stays a plain jump (bot-objectives.md 12.14).
    #[test]
    fn a_cod4_graph_keeps_no_leap_its_bots_cannot_jump() {
        let Some(install) = vcod_cod4::testing::cod4_install() else {
            return;
        };
        let zones = crate::game::cod4::Cod4Zones::load(&install, "mp_killhouse").unwrap();
        let clip = zones.clip.clone().unwrap();
        let world = bot_world(clip.clone(), &zones.map.entities, Vec::new());
        let probe = mantle_probe(clip, vcod_cod4::pmove::Rules::default());
        let mut job = crate::nav::NavJob::start_opts(
            "cod4 mp_killhouse",
            &world,
            GRAPH_RULES,
            Some(probe),
            None,
            LIP,
        );
        let g = job.poll(true).expect("the graph");
        assert!(g.leaps.is_empty());
        let node = |p: Vec3| {
            (0..g.nodes.len() as u32)
                .find(|&n| g.nodes[n as usize].distance(p) < 4.0)
                .expect("a node")
        };
        let top = node(Vec3::new(1310.0, 2557.0, 97.0));
        for foot in [
            Vec3::new(1280.0, 2496.0, 60.0),
            Vec3::new(1247.0, 2524.0, 60.0),
        ] {
            assert!(!g.edges[node(foot) as usize].contains(&top), "{foot}");
        }
        // This one lands as a plain jump aimed beside the top.
        let kept = node(Vec3::new(1242.0, 2496.0, 60.0));
        assert!(g.jumps.contains(&(kept, top)) && g.aims.contains_key(&(kept, top)));
    }
    /// mp_crash's roof at (64, -643, 313): `link_back` proved jumps 158
    /// across to (160, -518, 304), past the 128 a follower reads as off its
    /// path, so a bot planned one, dropped it at its foot and planned it
    /// again for the rest of a `koth` match. The graph keeps no edge but a
    /// mantle that long, and the roof keeps its drops (bot-objectives.md
    /// 12.15).
    #[test]
    fn a_cod4_graph_keeps_no_edge_its_follower_drops() {
        let Some(install) = vcod_cod4::testing::cod4_install() else {
            return;
        };
        let zones = crate::game::cod4::Cod4Zones::load(&install, "mp_crash").unwrap();
        let clip = zones.clip.clone().unwrap();
        let world = bot_world(clip.clone(), &zones.map.entities, Vec::new());
        let probe = mantle_probe(clip, vcod_cod4::pmove::Rules::default());
        let mut job = crate::nav::NavJob::start_opts(
            "cod4 mp_crash",
            &world,
            GRAPH_RULES,
            Some(probe),
            None,
            LIP,
        );
        let g = job.poll(true).expect("the graph");
        let reach = 4.0 * g.spacing;
        for (a, out) in g.edges.iter().enumerate() {
            for &b in out {
                let flat = (g.nodes[b as usize] - g.nodes[a]).truncate().length();
                assert!(
                    flat <= reach || g.mantles.contains(&(a as u32, b)),
                    "{} -> {}",
                    g.nodes[a],
                    g.nodes[b as usize]
                );
            }
        }
        let node = |p: Vec3| {
            (0..g.nodes.len() as u32)
                .find(|&n| g.nodes[n as usize].distance(p) < 4.0)
                .expect("a node")
        };
        let roof = node(Vec3::new(63.8, -643.5, 313.1));
        assert!(
            g.edges[roof as usize]
                .iter()
                .any(|&b| g.nodes[b as usize].z < 280.0)
        );
    }

    /// mp_cargoship's `pipe_shootable` (`*76` at (1300, 61, 104)) caps the
    /// jump up the deck's step from (1345, 36, 52) to (1313, 36, 80): over
    /// the clip map alone the graph keeps it and live bots rose to 91 and
    /// fell back for a whole match. With the brush entities in the bots'
    /// world it goes (bot-objectives.md 12.11, 12.15).
    #[test]
    fn a_cod4_graph_clips_the_brush_entities_its_bots_clip() {
        let Some(install) = vcod_cod4::testing::cod4_install() else {
            return;
        };
        let zones = crate::game::cod4::Cod4Zones::load(&install, "mp_cargoship").unwrap();
        let clip = zones.clip.clone().unwrap();
        let graph = |brushes: Vec<crate::game::cod4::clip::Brush4>| {
            let key = format!("cod4 mp_cargoship{}", brush_key(&brushes));
            let world = bot_world(clip.clone(), &zones.map.entities, brushes);
            let probe = mantle_probe(clip.clone(), vcod_cod4::pmove::Rules::default());
            let mut job =
                crate::nav::NavJob::start_opts(&key, &world, GRAPH_RULES, Some(probe), None, LIP);
            job.poll(true).expect("the graph")
        };
        let pipe = map_brushes(&zones.map.entities)
            .into_iter()
            .filter(|b| b.model == 76)
            .collect::<Vec<_>>();
        assert_eq!(pipe.len(), 1);
        let step = |g: &NavGraph| {
            let near = |p: Vec3| {
                (0..g.nodes.len() as u32).find(|&n| g.nodes[n as usize].distance(p) < 8.0)
            };
            let (Some(a), Some(b)) = (
                near(Vec3::new(1345.0, 36.0, 52.0)),
                near(Vec3::new(1313.0, 36.0, 80.0)),
            ) else {
                return false;
            };
            g.edges[a as usize].contains(&b)
        };
        assert!(step(&graph(Vec::new())));
        assert!(!step(&graph(pipe)));
    }
    /// mp_bloc's pool at (160, -5540, -24) is reached only down off a
    /// mantle's top and left only by a mantle over its rim: probed once,
    /// before the flood off the tops had found the pool's floor, the
    /// graph held it as a pocket with no way out (bot-objectives.md 12.16).
    #[test]
    fn a_cod4_graph_mantles_out_of_mp_blocs_pool() {
        let Some(install) = vcod_cod4::testing::cod4_install() else {
            return;
        };
        let zones = crate::game::cod4::Cod4Zones::load(&install, "mp_bloc").unwrap();
        let clip = zones.clip.clone().unwrap();
        let world = bot_world(clip.clone(), &zones.map.entities, Vec::new());
        let probe = mantle_probe(clip, vcod_cod4::pmove::Rules::default());
        let mut job = crate::nav::NavJob::start_opts(
            "cod4 mp_bloc",
            &world,
            GRAPH_RULES,
            Some(probe),
            None,
            LIP,
        );
        let g = job.poll(true).expect("the graph");
        let pool = g.nearest([51.0, -5709.0, -20.0]).unwrap();
        let site = g.nearest([-864.0, -6044.0, 0.0]).unwrap();
        assert!(
            g.nodes[pool as usize].z < -16.0,
            "a node on the pool's floor"
        );
        assert!(g.reaches(pool, site) && g.reaches(site, pool));
    }
}
