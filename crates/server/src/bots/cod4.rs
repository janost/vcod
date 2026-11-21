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
use vcod_common::net::msg::UserCmd;
use vcod_gsc::EntId;

use super::bridge::eye;
use super::game_bots::{GameBots, OwnBots};
use super::{
    BombView, BotView, DomFlagView, DomView, EnemyView, KeyBomb, ObjRole, SdView, SiteView, UoView,
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

/// How long a bot takes to enter the world on a fresh level. A client
/// loads the map first; a bot at once would spawn before `_spawnlogic`'s
/// `spawnPerFrameUpdate` has reached every spawn point (one a frame), and
/// koth's `getSpawnpoint_NearTeam` then compares an unset
/// `numPlayersAtLastUpdate`, a runtime error that ends retail's level
/// (cod4-gsc.md 8). 3 s covers 60 spawn points.
pub(crate) const BOT_LOAD_MS: i32 = 3000;

/// The CoD4 server's bots: the shared state ([`GameBots`]) and what
/// CoD4's hooks keep beside it ([`Cod4Own`]).
pub type Cod4Bots = GameBots<Cod4Own>;

/// What CoD4's hooks keep beside the shared bot state.
pub struct Cod4Own {
    /// The level's world the graph, the sight and the throws trace, and
    /// the clip map the mantle probe steps 1.7's pmove over.
    world: Option<(Rc<World>, Arc<ClipMap>)>,
    /// The graph cache's mark for the brush entities `world` clips
    /// ([`brush_key`]).
    brush_key: String,
    /// The world and graph came over from the last level on this map: the
    /// first frame's brush entities say whether they still stand.
    recheck_brushes: bool,
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
    hits: HashMap<usize, (u32, Option<super::HitFelt>)>,
    /// The level's scored hits read so far, and each victim's last
    /// attacker off them with the level time.
    hits_read: usize,
    attackers: HashMap<usize, (usize, i32)>,
    /// Every client's `eventSequence` last read.
    heard: HashMap<usize, i32>,
    /// The missiles in flight at the last read, and the bursts and spent
    /// missiles already heard ([`blasts`]).
    flying: Vec<Flying>,
    heard_blasts: Vec<EntId>,
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

impl OwnBots for Cod4Own {
    fn new() -> Self {
        Cod4Own {
            world: None,
            brush_key: String::new(),
            recheck_brushes: false,
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
            flying: Vec::new(),
            heard_blasts: Vec::new(),
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

    /// The hits, events heard and weapon numbers of the old level go; the
    /// classes picked, the gun counts and `bot_match`'s `OBJ`, `GUNS` and
    /// `THROWBACK` stay, and on the same map the world, rechecked against
    /// the first frame's brush entities.
    fn new_level(&mut self, same_map: bool) {
        let mut fresh = Cod4Own::new();
        fresh.on_level = self.on_level.take();
        fresh.classes = std::mem::take(&mut self.classes);
        fresh.objectives = self.objectives;
        fresh.man_guns = self.man_guns;
        fresh.throw_back = self.throw_back;
        fresh.gun.carry_counts(&self.gun);
        if same_map {
            fresh.world = self.world.take();
            fresh.brush_key = std::mem::take(&mut self.brush_key);
            fresh.recheck_brushes = fresh.world.is_some();
        }
        *self = fresh;
    }

    /// Every field named, so a new one is placed: per-slot or per-game.
    /// `attackers` is keyed by every victim, bot or not, and stays.
    fn forget(&mut self, slot: usize) {
        let Cod4Own {
            world: _,
            brush_key: _,
            recheck_brushes: _,
            on_level: _,
            level_start: _,
            throwing,
            mantling,
            climbed,
            crouched,
            stance_seq,
            arms,
            hits,
            hits_read: _,
            attackers: _,
            heard,
            flying: _,
            heard_blasts: _,
            classes,
            switches,
            objectives: _,
            man_guns: _,
            throw_back: _,
            gun,
            pass: _,
            next_think: _,
            think_at: _,
            held,
        } = self;
        held.remove(&slot);
        throwing.remove(&slot);
        mantling.remove(&slot);
        climbed.remove(&slot);
        crouched.remove(&slot);
        stance_seq.remove(&slot);
        arms.remove(&slot);
        hits.remove(&slot);
        heard.remove(&slot);
        classes.remove(&slot);
        switches.remove(&slot);
        gun.forget(slot);
    }
}

impl Cod4Bots {
    /// The level times the bots think at in the `SV_Frame` whose frames
    /// run on from `level` by `frame` each: one every [`BOT_TICK_MS`] of
    /// level time, before the next frame's. At `sv_fps` 20 that is one, at
    /// `level`, every frame (a catch-up's too); at 40 one every other
    /// frame; at 10 two a frame, so a bot moves 100 ms of cmds in its
    /// 100 ms. A bot clock behind the level (a catch-up) or far ahead (a
    /// clock that went back) starts over at `level`.
    pub fn think_times(&mut self, level: i32, frame: i32) -> Vec<i32> {
        let mut next = match self.own.next_think {
            Some(n) if n >= level && n - level <= BOT_TICK_MS => n,
            _ => level,
        };
        let mut out = Vec::new();
        while next < level + frame.max(1) {
            out.push(next);
            next += BOT_TICK_MS;
        }
        self.own.next_think = Some(next);
        out
    }

    /// The level the bots now stand on, `server_id` and `map`: on another
    /// one every brain forgets the menus it answered and what the old level
    /// left (paths, enemies, hits, events heard, weapon numbers); a new map
    /// drops the graph and the world too, for the next tick to build, and
    /// the same map keeps them while its brush entities stand the same.
    pub fn on_level(&mut self, server_id: i32, map: &str) {
        let same_map = self.own.on_level.as_ref().is_some_and(|(_, m)| m == map);
        if self
            .own
            .on_level
            .as_ref()
            .is_some_and(|(id, m)| *id == server_id && m == map)
        {
            return;
        }
        let first = self.own.on_level.is_none();
        self.own.on_level = Some((server_id, map.to_string()));
        if !first {
            self.new_level(same_map);
        }
    }

    /// The bots' world, for a test to tell a rebuilt one from a kept one
    /// (`Rc::ptr_eq`; holding it keeps a rebuild off the old address).
    #[cfg(test)]
    pub(crate) fn world_rc(&self) -> Option<Rc<World>> {
        self.own.world.as_ref().map(|(w, _)| w.clone())
    }

    /// The world and everything built over it, for the next tick to build
    /// again.
    fn drop_world(&mut self) {
        self.own.world = None;
        self.own.brush_key.clear();
        self.driver.new_world();
    }

    pub(crate) fn world(&self) -> Option<&World> {
        self.own.world.as_ref().map(|(w, _)| &**w)
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
        // Another gametype on the same map can leave other ones standing:
        // the carried world and graph go.
        if self.own.recheck_brushes && level.level_time() > 0 {
            self.own.recheck_brushes = false;
            if brush_key(level.brushes()) != self.own.brush_key {
                self.drop_world();
            }
        }
        if self.own.world.is_none()
            && let Some(clip) = clip
            && level.level_time() > 0
        {
            let brushes = level.brushes().to_vec();
            self.own.brush_key = brush_key(&brushes);
            self.own.world = Some((bot_world(clip.clone(), entities, brushes), clip.clone()));
        }
        let Some((world, clip)) = self.own.world.as_ref() else {
            return;
        };
        if self.driver.nav.is_none() {
            let rules = level.rules;
            let job = self.driver.nav_job.get_or_insert_with(|| {
                NavJob::start_opts(
                    &format!("cod4 {map}{}", self.own.brush_key),
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
        self.own.pass.states = (0..level.players.len())
            .filter_map(|s| Some((s, level.pm_state(s)?)))
            .collect();
        self.own.pass.now = self.own.think_at.unwrap_or_else(|| level.level_time());
        self.own.pass.dm = level.rt.cvars().get("g_gametype") == "dm";
        for &slot in self.driver.bots.keys() {
            // A follower's playerstate is the followed client's.
            let Some(ps) = self
                .own
                .pass
                .states
                .get(&slot)
                .filter(|ps| ps.other_flags & vcod_cod4::pmove::POF_PLAYER != 0)
            else {
                continue;
            };
            let arms = self.own.arms.entry(slot).or_default();
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
        let clip = self.own.world.as_ref().map(|w| w.1.clone());
        for &slot in self.driver.bots.keys() {
            let Some(ps) = self.own.pass.states.get(&slot) else {
                continue;
            };
            let seq = ps.event_sequence;
            let from = self.own.stance_seq.insert(slot, seq).unwrap_or(seq);
            if ps.pm_type != pm_type::NORMAL || ps.other_flags & vcod_cod4::pmove::POF_PLAYER == 0 {
                self.own.crouched.remove(&slot);
                continue;
            }
            if (from.max(seq - 4)..seq)
                .any(|i| ps.events[(i & 3) as usize] == ev::STANCE_FORCE_CROUCH)
            {
                self.own.crouched.insert(slot);
            } else if self.own.crouched.contains(&slot)
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
                self.own.crouched.remove(&slot);
            }
        }
    }

    fn ps(&self, slot: usize) -> Option<&PlayerState> {
        self.own.pass.states.get(&slot)
    }

    /// Each bot's reliable server commands since the last tick.
    pub(crate) fn inbox(&mut self, inbox: BTreeMap<usize, Vec<String>>) {
        self.own.pass.inbox = inbox;
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
        let Some(texts) = self.own.pass.inbox.get(&slot) else {
            return;
        };
        let changeclass = menus.iter().position(|m| m == "changeclass");
        let mut named: Vec<(i32, String)> = Vec::new();
        let mut class_asked = false;
        for (seq, text) in texts.iter().enumerate() {
            // The brain numbers weapons its own way ([`Arms`]): the switch
            // is asked for in CoD4's numbers by [`Self::cmd`].
            if let Some(w) = text.strip_prefix("a ").and_then(|w| w.trim().parse().ok()) {
                self.own.switches.insert(slot, (w, SWITCH_TICKS));
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
        if let Some(&c) = self.own.classes.get(&slot) {
            return c;
        }
        let count = |c: usize| self.own.classes.values().filter(|&&x| x == c).count();
        let pick = (0..CLASSES.len()).min_by_key(|&c| count(c)).unwrap_or(0);
        self.own.classes.insert(slot, pick);
        pick
    }

    /// The hit the bot in `slot` felt this tick ([`Self::hurt`]).
    pub(crate) fn feel(&mut self, level: &Cod4Level, slot: usize) {
        let hurt = self.hurt(level, slot);
        self.own.pass.felt = Some((slot, hurt));
    }

    fn felt(&self, slot: usize) -> Option<super::Hurt> {
        self.own.pass.felt.filter(|f| f.0 == slot).and_then(|f| f.1)
    }

    /// The level's scored hits since the last read: each victim's last
    /// attacker, at the level time.
    pub(crate) fn read_hits(&mut self, level: &Cod4Level) {
        let now = level.level_time();
        for &(attacker, victim, _) in level.hits_since(self.own.hits_read) {
            if attacker != victim {
                self.own.attackers.insert(victim, (attacker, now));
            }
        }
        self.own.hits_read = level.hit_count();
    }

    /// [`BotView::hurt`] for the bot in `slot`: a `damageEvent` new since
    /// the last tick is a hit, by the client the level last scored a hit on
    /// it for, along the direction it came (`Cod4Level::hit_dir`; zero for
    /// none); reported for [`super::HURT_MS`] while the attacker still
    /// plays.
    fn hurt(&mut self, level: &Cod4Level, slot: usize) -> Option<super::Hurt> {
        let now = self.own.pass.now;
        let w = level.wire_state(slot)?;
        let word = |name: &str| {
            vcod_cod4::net::fields::PLAYER_STATE
                .iter()
                .find(|f| f.name == name)
                .map_or(0, |f| w.words[f.word()])
        };
        let event = word("damageEvent");
        let attacker = self.own.attackers.get(&slot).copied();
        let latch = self.own.hits.entry(slot).or_insert((event, None));
        let hit = super::feel_hit(latch, event, attacker, || level.hit_dir(slot), now);
        super::hurt_of(hit, now, |a| self.playing(level, a))
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
        if !self.shoots() || !self.playing(level, slot) {
            return None;
        }
        let me = self.ps(slot)?;
        let (world, _) = self.own.world.as_ref()?;
        let mine = teams.get(slot).copied().unwrap_or(0);
        let (tracked, fov) = super::tracked_and_fov(self.driver.bots.get(&slot));
        let my_eye = eye(me);
        let looker = super::Looker {
            origin: me.origin,
            eye: my_eye,
            view: me.view_angles,
            fov,
            tracked,
            attacker,
        };
        let dm = self.own.pass.dm;
        let foes = (self.own.pass.states.iter())
            .filter(|&(&i, _)| i != slot && self.playing(level, i))
            .filter(|&(&i, _)| super::foe(dm, mine, teams.get(i).copied().unwrap_or(0)))
            .map(|(&i, p)| super::Seen {
                slot: i,
                origin: p.origin,
                eye: eye(p),
                view_height: p.view_height_current,
                velocity: p.velocity.into(),
            });
        let w = &world.collision;
        super::sight_enemy(&looker, foes, |to| self.shot_clear(w, my_eye, to.into()))
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
            && !self.own.pass.blockers.iter().any(|b| {
                b.trace(from, to, 1.0).is_some()
                    && b.trace_surfs(from, to, 1.0, crate::game::cod4::combat::MASK_SHOT)
                        .is_some()
            })
    }

    /// The frame's noises since the last tick: each playing client's new
    /// fire events chest high as gunfire, and each burst since as a blast
    /// ([`blasts`]), as CoD2's and the CoD1 driver gather them.
    pub(crate) fn listen(&mut self, level: &Cod4Level) -> Vec<super::Noise> {
        let mut noises = Vec::new();
        let mut slots: Vec<usize> = self.own.pass.states.keys().copied().collect();
        slots.sort_unstable();
        let fire = [weap::ev::FIRE_WEAPON, weap::ev::FIRE_WEAPON_LASTSHOT];
        for i in slots {
            if !self.playing(level, i) {
                self.own.heard.remove(&i);
                continue;
            }
            let ps = &self.own.pass.states[&i];
            if super::fired(&mut self.own.heard, i, ps.event_sequence, &ps.events, &fire) {
                noises.push(super::gunfire(i, ps.origin.into(), ps.view_height_current));
            }
        }
        let c4 = level.rt.host.cod4();
        let defs = level.weapon_defs();
        let loud = |w: u32| {
            defs.get(w as usize).is_some_and(|d| {
                d.explosion_radius > 0 && d.offhand_class != weap::OFFHAND_CLASS_SMOKE
            })
        };
        let flying = |m: &crate::game::cod4::missile::Missile4| Flying {
            id: m.id,
            owner: m.owner,
            weapon: m.weapon,
            at: m.origin,
        };
        let missiles: Vec<(Flying, bool)> = (c4.missiles.iter())
            .map(|m| (flying(m), m.free_at.is_some() && m.e_flags & SPENT != 0))
            .collect();
        let bursts: Vec<(EntId, u32, Vec3)> = c4
            .bursts
            .iter()
            .map(|b| (b.id, b.weapon, b.origin))
            .collect();
        let heard = &mut self.own.heard_blasts;
        noises.extend(blasts(&self.own.flying, &missiles, &bursts, heard, loud));
        self.own.flying = (c4.missiles.iter())
            .filter(|m| m.free_at.is_none())
            .map(flying)
            .collect();
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

    /// The body in `slot` as the follower reads it
    /// ([`super::bridge::follower_body`]).
    pub(crate) fn move_ps(&self, slot: usize) -> Option<vcod_common::pmove::PlayerState> {
        let ps = self.ps(slot)?;
        Some(super::bridge::follower_body(
            ps,
            self.own.pass.now,
            GRAPH_RULES,
        ))
    }

    /// The bots following a mantle edge (`BotWorld::routed`).
    pub(crate) fn routed(&mut self, slot: usize, follower: &crate::nav::Follower, g: &NavGraph) {
        match follower.mantle_edge(g) {
            Some(e) => self.own.mantling.insert(slot, e),
            None => self.own.mantling.remove(&slot),
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
        let arms = self.own.arms.get(&slot).unwrap_or(&no_arms);
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
            Some(g) if in_throw && self.own.throwing.contains_key(&slot) => g,
            _ => arms.brain(ps.weapon),
        };
        let vitals = level.rt.client_vitals(slot);
        let dead = vitals.dead || ps.pm_type >= pm_type::DEAD;
        let on_ladder = ps.pm_flags & pmf::LADDER != 0;
        let on_ground = ps.on_ground();
        let origin: [f32; 3] = ps.origin.into();
        let (w, _) = self.own.world.as_ref()?;
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
            delta_angles: ps.delta_angles.map(|d| cmd_angle(f64::from(d))),
            delta_deg: Some(ps.delta_angles),
            view: ps.view_angles,
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
            ads_frac: ps.weapon_pos_frac,
            spread_scale: ps.aim_spread_scale,
            speed: ps.velocity.truncate().length(),
            velocity: ps.velocity.into(),
            dead,
            playing: playing && !dead,
            enemy,
            hurt: self.felt(slot),
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
            stand_room: true,
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
            on_ground,
            // `ClientThink` steps pmove at the held weapon's scale; the
            // keys are whole (the wire carries only their sign).
            move_scale: 1.0,
            pistol,
            primaries: [0; 2],
            gun,
            reload_ms,
            draw_ms,
            loaded,
            one_life: level.rt.cvars().get("g_gametype") == "sd",
            survive: super::retreat::Vitals {
                health: vitals.health,
                max_health: vitals.max_health.max(1),
                clip_size: held.map_or(0, |d| d.clip_size.min(i32::from(i16::MAX)) as i16),
                enemy_eye,
                heal: None,
                cover: None,
            },
            ..Default::default()
        })
    }

    /// The nearest live frag within [`super::FLEE_REACH`] across and
    /// [`super::FLEE_HEIGHT`] up or down, the bot's own or one it notices.
    fn frag_near(&self, level: &Cod4Level, slot: usize) -> Option<[f32; 3]> {
        let ps = self.ps(slot)?;
        let (world, _) = self.own.world.as_ref()?;
        let eye = eye(ps);
        let defs = level.weapon_defs();
        let frags = (level.rt.host.cod4().missiles.iter())
            .filter(|m| {
                defs.get(m.weapon as usize).is_some_and(|d| {
                    d.offhand_class == weap::OFFHAND_CLASS_FRAG && d.explosion_radius > 0
                })
            })
            .map(|m| (m.origin, m.owner == Some(slot)));
        super::frag_near(ps.origin, frags, |o| {
            super::notices_frag(eye, ps.view_angles[1], o, |a, b| {
                world.collision.shot_trace(a, b).fraction >= 1.0
            })
        })
    }

    /// [`BotView::lob`] for the bot in `slot` throwing frag `weapon` (the
    /// brain's number) at `target`, flown by [`super::lob`]. With friendly
    /// fire on, none onto a playing teammate's `explosionRadius`.
    pub(crate) fn lob_probe(
        &self,
        level: &Cod4Level,
        slot: usize,
        weapon: u8,
        target: [f32; 3],
        teams: &[i32],
    ) -> Option<[f32; 2]> {
        let ps = self.ps(slot)?;
        let w = self.own.arms.get(&slot)?.weapon(weapon);
        let def: &WeaponDef = level.weapon_defs().get(w as usize).filter(|_| w > 0)?;
        let radius = def.explosion_radius as f32;
        let target = Vec3::from(target);
        let mates = teams
            .iter()
            .enumerate()
            .filter(|&(i, _)| i != slot && self.playing(level, i))
            .filter_map(|(i, &t)| Some((t, self.ps(i)?.origin)));
        if friendly_fire(level)
            && super::mate_in_blast(teams.get(slot).copied(), mates, target, radius)
        {
            return None;
        }
        let (world, _) = self.own.world.as_ref()?;
        let throw = super::Lob {
            from: eye(ps),
            thrower: ps.origin,
            speed: def.projectile_speed as f32,
            speed_up: def.projectile_speed_up as f32,
            fuse_ms: def.fuse_time,
            radius,
        };
        super::lob(&world.collision, &throw, target)
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
        let arms = self.own.arms.get(&slot).cloned().unwrap_or_default();
        let throwing = self.own.throwing.remove(&slot);
        // A body out of play throws nothing.
        let grenade = view
            .grenade
            .filter(|_| view.playing)
            .map(|g| (g, arms.weapon(g)));
        let (mut c4, armed) = offhand_cmd(&cmd, grenade, ps.weapon, throwing);
        if let Some(armed) = armed {
            self.own.throwing.insert(slot, armed);
        }
        if armed.is_none() {
            c4.weapon = arms.weapon(cmd.weapon) as u8;
            if c4.weapon == 0 {
                c4.weapon = ps.weapon as u8;
            }
        }
        // A script's switch until it lands, or a weapon never given.
        if let Some((w, left)) = self.own.switches.get_mut(&slot) {
            let held = ps.has_weapon(*w);
            *left = left.saturating_sub(u8::from(!held));
            if ps.weapon == *w || *left == 0 {
                self.own.switches.remove(&slot);
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
        match self.own.mantling.get(&slot).copied() {
            Some(edge) => {
                c4.buttons &= !button::JUMP;
                if ps.pm_flags & pmf::MANTLE != 0 {
                    self.own.climbed.insert(slot, edge);
                } else if self.own.climbed.get(&slot) != Some(&edge)
                    && ps.mantle.flags & vcod_cod4::pmove::mantle::flag::HINT != 0
                {
                    c4.buttons |= button::JUMP;
                }
            }
            None => {
                self.own.climbed.remove(&slot);
            }
        }
        if self.own.crouched.contains(&slot) {
            c4.buttons = (c4.buttons & !button::PRONE) | button::CROUCH;
        }
        if c4.offhand == 0 {
            c4.offhand = ps.off_hand_index as u8;
        }
        self.apply_steps(slot, &mut c4, &ps);
        let climbing = view.on_ladder && !view.on_ground;
        Some(
            crate::nav::tick_times(self.own.pass.now, climbing)
                .map(|t| UserCmd4 {
                    server_time: t,
                    ..c4
                })
                .collect(),
        )
    }

    /// The tick's end.
    pub(crate) fn tick_done(&mut self) {
        self.own.pass = Pass::default();
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
        let frame = Cod4Frame(level.objectives().filter(|_| self.own.objectives));
        self.read_guns(level, frame.0.is_none());
        self.own.pass.blockers = level.shot_blockers();
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
                hot: None,
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
    collision.set_source(clip.checksum);
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
    ) -> vcod_common::clipmap::ClipTrace {
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
        world.trace(start, end, mins, maxs, none, mask).into()
    }

    fn point_contents(&self, p: Vec3) -> u32 {
        self.near(p, p).fold(self.clip.point_contents(p), |c, b| {
            let q = vcod_common::turretpose::angles_quat(b.angles.to_array());
            c | self
                .clip
                .point_contents_model(b.model, q.inverse() * (p - b.origin))
        })
    }

    fn shot_mask(&self) -> u32 {
        vcod_cod4::collision::MASK_SHOT
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

    /// What a walk's body moves over: the clip map and the map's brushes.
    fn world(&self) -> crate::game::cod4::clip::Cod4Clip<'_> {
        crate::game::cod4::clip::Cod4Clip {
            clip: &self.clip,
            bodies: &[],
            brushes: &self.brushes,
        }
    }
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
        use vcod_cod4::pmove::{Cmd, ENTITYNUM_NONE, MASK_PLAYERSOLID, PmWorld, WeaponMove};
        // Weapon 0: no weapon scale, the graph's 190 units/s.
        let mut c = Cmd::from(&to_cmd4(cmd));
        c.weapon = 0;
        super::bridge::walk_cmd(
            ps,
            carry,
            from_ms,
            |b: &mut PlayerState, old: &mut Cmd| {
                // 1.7 keeps `delta_angles` as floats: the walk's shorts
                // go in and come back rounded.
                b.delta_angles = delta.map(|d| (f64::from(d) * SHORT_DEG) as f32);
                let world = self.world();
                vcod_cod4::pmove::player_step(b, &c, old, &world, self.rules, WeaponMove::NONE);
                *delta = b
                    .delta_angles
                    .map(|d| (f64::from(d) / SHORT_DEG).round() as i32);
                c.buttons & button::JUMP != 0
            },
            // The clip map and the brushes, as the pmove moves over them.
            |from, to, mins, maxs| {
                let none = ENTITYNUM_NONE as u32;
                self.world()
                    .trace(from, to, mins, maxs, none, MASK_PLAYERSOLID)
                    .into()
            },
        );
    }

    fn key(&self) -> String {
        self.key.clone()
    }
}

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
/// stands once it mantles over `clip` ([`super::bridge::mantle_walk`],
/// cod4-pmove.md 17).
pub fn mantle_walk(
    clip: &ClipMap,
    rules: vcod_cod4::pmove::Rules,
    from: Vec3,
    yaw: f32,
) -> Option<Vec3> {
    use vcod_cod4::pmove::{Cmd, WeaponMove};
    if !rules.mantle_enable || rules.mantle_anims.is_none() {
        return None;
    }
    let angle = cmd_short(yaw);
    let mut old = Cmd::default();
    super::bridge::mantle_walk(
        from,
        yaw,
        rules.mantle_check_radius,
        |a, b, mins, maxs, mask| ClipMap::trace(clip, a, b, mins, maxs, mask).into(),
        |ps: &mut PlayerState, server_time, jump, forward| {
            let cmd = Cmd {
                server_time,
                buttons: if jump { button::JUMP } else { 0 },
                angles: [0, angle, 0],
                forward_move: if forward { 127 } else { 0 },
                ..Default::default()
            };
            vcod_cod4::pmove::player_step(ps, &cmd, &mut old, clip, rules, WeaponMove::NONE);
        },
    )
}

/// A cmd angle as 1.7 makes one: `(int)` of the stored float `deg *
/// 182.04445f + 0.5f`, masked (`iw3mp.exe` 0x463aa9..0x463acf, the
/// server's cmd base `cod4_lnxded` 0x8130adc..0x8130b29). CoD2 1.3
/// truncates without the half ([`vcod_common::x87::angle_shorts`]).
pub(crate) fn cmd_short(deg: f32) -> u16 {
    (cmd_angle(f64::from(deg)) & 0xffff) as u16
}

/// [`cmd_short`] before the mask. The server's base takes `viewangles -
/// delta_angles` on the x87 stack, so a difference comes in as an `f64`.
pub(crate) fn cmd_angle(deg: f64) -> i32 {
    (deg * f64::from(vcod_common::x87::SHORT_PER_DEG) + 0.5) as f32 as i32
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
    let (key, armed) = super::bridge::frag_key(trigger, throwing);
    if key {
        c.buttons |= button::FRAG;
    }
    c.offhand = g as u8;
    (c, Some(armed))
}

/// 1.7's bits for the CoD1 keys (`vcod_cod4::pmove::button`): the attack,
/// melee, use and sight to 0x1, 0x4, 0x8 and 0x800; the reload, the leans
/// and the two stances to 0x10, 0x40/0x80 and 0x100/0x200; a jump is
/// 0x400. The use key is `ClientThink`'s 0x28 read, the reload
/// `PM_Weapon_CheckForReload`'s.
const BUTTONS: super::bridge::Buttons = super::bridge::Buttons {
    attack: button::ATTACK,
    melee: button::MELEE,
    use_key: 0x8,
    ads: button::ADS_MODE,
    reload: button::RELOAD,
    lean_left: button::LEAN_LEFT,
    lean_right: button::LEAN_RIGHT,
    prone: button::PRONE,
    crouch: button::CROUCH,
    jump: button::JUMP,
};

/// A CoD1 usercmd in 1.7's bits ([`BUTTONS`]). The moves go whole, as the
/// wire carries only their sign.
pub fn to_cmd4(cmd: &UserCmd) -> UserCmd4 {
    let whole = |m: i8| (i32::from(m).signum() * 127) as i8;
    UserCmd4 {
        server_time: cmd.server_time,
        buttons: super::bridge::cmd_buttons(cmd, &BUTTONS),
        angles: cmd.angles.map(|a| a as u16),
        weapon: cmd.weapon,
        offhand: 0,
        forwardmove: whole(cmd.forward),
        rightmove: whole(cmd.right),
        ..Default::default()
    }
}

/// Whether the level's teammates can hurt each other: 1.7's
/// `scr_team_fftype` (the stock scripts' `level.friendlyfire`) or the
/// engine's `scr_friendlyfire`.
fn friendly_fire(level: &Cod4Level) -> bool {
    let cvars = level.rt.cvars();
    ["scr_team_fftype", "scr_friendlyfire"]
        .iter()
        .any(|n| crate::game::builtins::cvar::atoi(cvars.get(n)) != 0)
}

/// A missile at a read: its entity, thrower, weapon and origin.
#[derive(Clone, Copy, Debug)]
struct Flying {
    id: EntId,
    owner: Option<usize>,
    weapon: u32,
    at: Vec3,
}

/// `eFlags` 0x20, which a missile spent on impact carries (cod4-combat.md
/// 10.3).
const SPENT: u32 = 0x20;

/// The blasts not heard yet, as CoD2's and CoD1's bots hear them, each as
/// its thrower's: a burst (`bursts`, entity, weapon, point) laid to the
/// missile of its weapon nearest it that was in flight at the last read
/// (`was`) and is gone now, a fuse's burst leaving no thrower of its own;
/// and a missile spent on impact (`missiles`, each with whether it is). Only
/// weapons `loud` names are heard; `heard` keeps what was heard of what is
/// still up.
fn blasts(
    was: &[Flying],
    missiles: &[(Flying, bool)],
    bursts: &[(EntId, u32, Vec3)],
    heard: &mut Vec<EntId>,
    loud: impl Fn(u32) -> bool,
) -> Vec<super::Noise> {
    heard.retain(|id| bursts.iter().any(|b| b.0 == *id) || missiles.iter().any(|m| m.0.id == *id));
    let mut gone: Vec<Flying> = (was.iter())
        .filter(|w| !missiles.iter().any(|(m, _)| m.id == w.id))
        .copied()
        .collect();
    let mut noises = Vec::new();
    for &(id, weapon, at) in bursts {
        if heard.contains(&id) || !loud(weapon) {
            continue;
        }
        heard.push(id);
        let thrower = (gone.iter().enumerate())
            .filter(|(_, g)| g.weapon == weapon)
            .min_by(|a, b| a.1.at.distance(at).total_cmp(&b.1.at.distance(at)))
            .map(|(k, _)| k);
        if let Some(owner) = thrower.and_then(|k| gone.remove(k).owner) {
            noises.push(super::blast(owner, at.into()));
        }
    }
    for &(m, spent) in missiles {
        if !spent || heard.contains(&m.id) || !loud(m.weapon) {
            continue;
        }
        heard.push(m.id);
        if let Some(owner) = m.owner {
            noises.push(super::blast(owner, m.at.into()));
        }
    }
    noises
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The brain's aim goes out as 1.7 rounds it off the float delta;
    /// truncated shorts less the truncated delta send 2366 for each.
    #[test]
    fn a_brain_cmd_angle_rounds_as_1_7_does() {
        let d = [0.0, 90.0, 0.0];
        let v = crate::bots::BotView {
            delta_angles: d.map(|d| cmd_angle(f64::from(d))),
            delta_deg: Some(d),
            ..Default::default()
        };
        assert_eq!(crate::bots::cmd_angles(&v, [13.0, 103.0]), [2367, 2367, 0]);
    }

    /// 1.7 adds the half and truncates toward zero, so -45 (-8192.0002
    /// shorts) goes out as -8191, where a round and CoD2 1.3's truncation
    /// both send -8192.
    #[test]
    fn a_mantle_walk_yaw_goes_out_as_1_7_sends_it() {
        assert_eq!(cmd_short(-45.0), 0xe001);
        assert_eq!(vcod_common::x87::angle_shorts(-45.0), 0xe000);
        assert_eq!(cmd_short(45.0), 0x2000);
        assert_eq!(cmd_short(315.0), 0xe000);
        assert_eq!(cmd_short(13.0), 2367);
        assert_eq!(vcod_common::x87::angle_shorts(13.0), 2366);
    }
    use vcod_common::net::msg;

    /// A fuse's burst is its vanished frag's thrower's, heard once; a
    /// spent impact is heard where it struck; smoke and an unseen
    /// missile's burst are not.
    #[test]
    fn a_blast_is_heard_once_as_its_thrower_s() {
        let fly = |id, owner, weapon, x| Flying {
            id: EntId(id, 0),
            owner: Some(owner),
            weapon,
            at: Vec3::new(x, 0.0, 0.0),
        };
        let loud = |w: u32| w != 9;
        let was = [
            fly(100, 2, 5, 0.0),
            fly(101, 3, 5, 400.0),
            fly(102, 4, 9, 0.0),
        ];
        let mut heard = Vec::new();
        let at = Vec3::new(390.0, 0.0, 0.0);
        let bursts = [(EntId(200, 0), 5, at), (EntId(201, 0), 9, Vec3::ZERO)];
        let missiles = [(fly(100, 2, 5, 10.0), false), (fly(103, 6, 5, 0.0), true)];
        let n = blasts(&was, &missiles, &bursts, &mut heard, loud);
        let got: Vec<(usize, f32)> = n.iter().map(|n| (n.source, n.at[0])).collect();
        assert_eq!(got, [(3, 390.0), (6, 0.0)]);
        assert!(blasts(&was, &missiles, &bursts, &mut heard, loud).is_empty());
        let stray = [(EntId(202, 0), 5, at)];
        assert!(blasts(&[], &[], &stray, &mut Vec::new(), loud).is_empty());
    }

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

    /// One think a [`BOT_TICK_MS`] of level time: every frame at sv_fps
    /// 20, every other at 40, two a frame at 10; a clock that went back or
    /// fell behind starts over at the level's time.
    #[test]
    fn bots_think_every_50_ms_of_level_time() {
        let mut b = Cod4Bots::new(false);
        let mut at = |level, frame| b.think_times(level, frame);
        assert_eq!(at(1000, 50), [1000]);
        assert_eq!(at(1050, 50), [1050]);
        assert_eq!(at(1100, 25), [1100]);
        assert_eq!(at(1125, 25), [] as [i32; 0]);
        assert_eq!(at(1150, 25), [1150]);
        assert_eq!(at(1200, 100), [1200, 1250]);
        assert_eq!(at(1300, 100), [1300, 1350]);
        assert_eq!(at(400, 100), [400, 450], "the clock went back");
        assert_eq!(at(9000, 50), [9000], "a catch-up past the bots");
    }

    /// A new level rearms every brain and drops what the old one left; the
    /// same map keeps the world it built, another drops it; the admin's
    /// settings and the class picks carry over either way.
    #[test]
    fn a_new_level_rearms_the_brains_and_keeps_the_map_only_when_it_stays() {
        let mut b = Cod4Bots::new(true);
        b.add(0, "bot0", "axis", 1);
        b.on_level(1, "mp_crash");
        b.own.brush_key = "crash brushes".into();
        b.own.throwing.insert(0, true);
        b.own.classes.insert(0, 2);
        b.own.objectives = false;
        b.driver.bots.get_mut(&0).unwrap().answered.push(3);
        b.on_level(1, "mp_crash");
        assert!(b.own.throwing.contains_key(&0), "the same level keeps all");

        b.on_level(2, "mp_crash");
        let bot = &b.driver.bots[&0];
        assert!(bot.answered.is_empty() && bot.round_start);
        assert!(b.own.throwing.is_empty());
        assert_eq!(b.own.brush_key, "crash brushes");
        assert_eq!((b.own.classes.get(&0), b.own.objectives), (Some(&2), false));

        b.on_level(3, "mp_strike");
        assert!(b.own.brush_key.is_empty(), "another map builds its own");
        assert_eq!((b.own.classes.get(&0), b.own.objectives), (Some(&2), false));
        assert_eq!(b.own.on_level, Some((3, "mp_strike".into())));
        assert!(b.driver.bots.contains_key(&0));
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

    /// A walk's body in the air on mp_cargoship's `pipe_shootable` (`*76`),
    /// on its facet at (1340, 61, 114) too steep to stand on: the step's
    /// pmove clips the pipe, so the slope read under the body must trace
    /// it too, or the body reads as falling where it slides.
    #[test]
    fn a_cod4_walk_slides_on_the_brush_entity_its_pmove_clips() {
        let Some(install) = vcod_cod4::testing::cod4_install() else {
            return;
        };
        let zones = crate::game::cod4::Cod4Zones::load(&install, "mp_cargoship").unwrap();
        let clip = zones.clip.clone().unwrap();
        let pipe = map_brushes(&zones.map.entities)
            .into_iter()
            .filter(|b| b.model == 76)
            .collect::<Vec<_>>();
        let walk = Cod4Walk::new(clip.clone(), vcod_cod4::pmove::Rules::default(), pipe);
        let world = bot_world(clip, &zones.map.entities, Vec::new());
        let mut ps = vcod_common::pmove::PlayerState::spawn(Vec3::new(1340.0, 61.0, 114.0), 0.0);
        ps.on_ground = false;
        let cmd = UserCmd {
            server_time: 50,
            ..msg::NULL_USERCMD
        };
        use crate::nav::WalkStep;
        walk.step(
            &mut ps,
            &mut [0; 3],
            &mut None,
            0,
            &cmd,
            &world.collision,
            &[],
        );
        // The deck under the pipe is at 52: the pipe holds the body.
        assert!(ps.origin.z > 100.0, "{}", ps.origin);
        assert!(!ps.on_ground && ps.sliding(), "{}", ps.origin);
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
