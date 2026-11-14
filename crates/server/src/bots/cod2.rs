//! CoD2 bots: vcod's CoD1 brain ([`Bot`]) and graph ([`crate::nav`])
//! driving clients of a [`Cod2Level`] (docs/bots.md, "CoD2").
//!
//! The graph floods the CoD2 collision world (`ClipMap` through the
//! shared [`vcod_common::collision::WorldClip`] hook, its ladder brushes
//! too) with walks under 1.5's movement rules, which CoD2's pmove starts
//! from (cod2-movement.md), and adds the mantles CoD2's own pmove climbs
//! ([`mantle_probe`], `NavGraph::mantles`). The brain reads a [`BotView`]
//! filled from the CoD2 playerstate, weapon state and weapon table, and
//! its CoD1 usercmd is rewritten into CoD2's button bits ([`to_cmd2`], the
//! frag throw onto the offhand key by [`offhand_cmd`]) and run through
//! the level's own `ClientThink`, so the body moves by CoD2's pmove. The
//! stock menus are answered as a client would: `serverinfo_<gametype>`
//! with `close`, the team menu with the bot's team, the weapon menu off
//! [`WEAPON_MENUS_COD2`]. The `sd`, `ctf` and `hq` objectives are read off
//! the stock scripts (`game/cod2_objectives.rs`) into CoD1's views
//! (bot-objectives.md 11).
//!
//! The tick is the shared driver's (`bots::driver::step`, through
//! `Cod2Server`'s `BotWorld`): this file holds what CoD2 reads and sends
//! there, in CoD1's order; its route goes without what the trait's consts
//! name (no `YIELDS`, no `PMOVE_PROBES`;
//! bot-combat.md 15 measured both on), though it parts teammates
//! (`PARTS_MATES`, bot-objectives.md 11.5).
//!
//! The CoD1 brain's fights are fed as the CoD1 driver feeds them: the hit
//! felt ([`BotView::hurt`], its direction off the playerstate's
//! `damageEvent`, `damageYaw` and `damagePitch`), the gunfire and blasts
//! heard, the cover spot and its leans, and the retreat's cover. Smoke is
//! the driver's own ([`SmokeThrow`]): a bot with a smoke grenade throws it
//! on the line to the enemy when it heads for a plant or defuse, covers a
//! planter or starts a retreat, and a cloud blocks every bot's sight for
//! its life (docs/bots.md, "CoD2").
//!
//! In `dm` and `tdm` a bot mans a mounted gun with an enemy in its arc
//! ([`gun`], cod2-turrets.md 17).

mod fetch;
mod gun;

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

use glam::Vec3;
use vcod_cod2::net::UserCmd2;
use vcod_cod2::pmove::{button, pm_type, pmf};
use vcod_common::net::msg::{self, UserCmd};

use super::{Bot, BotView, EnemyView, WeaponMenu};
use crate::game::cod2::Cod2Level;
use crate::game::script::{TEAM_ALLIES, TEAM_AXIS};
use crate::nav::{NavGraph, NavJob};

/// The movement rules the graph's walks run under: CoD2's movement is
/// 1.5's plus the mantle, the inertia clamp and the jump cap
/// (cod2-movement.md, opening). Walked under CoD2's own pmove instead
/// ([`Cod2Walk`]), the graph changed by under 1% and its bots stalled more
/// (bot-combat.md 16), so the walks stay on these.
pub const GRAPH_RULES: vcod_common::pmove::MoveRules = vcod_common::pmove::MoveRules::V1_5;

/// Whether the graph is built for bots that read a leap's lip: CoD2's
/// route has no pmove probes (`BotWorld::PMOVE_PROBES`), so a bot on a
/// leap never jumped and paced mp_decoy's pit at (8650, -12570, -530);
/// its leaps are kept only as plain jumps (`NavGraph::build_opts`,
/// bot-objectives.md 11.8), as CoD4's are.
pub const LIP: bool = false;

/// The nav walks' bodies under CoD2's pmove ([`crate::nav::WalkStep`]): each
/// walk's CoD1 body is the read of a CoD2 body stepped as `ClientThink`
/// steps a bot's, its cmd through [`to_cmd2`], over the level's `rules`.
/// Measured and left off (bot-combat.md 16): [`Cod2Bots::nav_tick`] hands
/// the build none.
pub struct Cod2Walk {
    rules: vcod_cod2::pmove::Rules,
    key: String,
}

impl Cod2Walk {
    pub fn new(rules: vcod_cod2::pmove::Rules) -> Cod2Walk {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        format!("{rules:?}").hash(&mut h);
        Cod2Walk {
            rules,
            key: format!("cod2 {:016x}", h.finish()),
        }
    }
}

/// The CoD2 body a walk carries between cmds ([`Cod2Walk`]), the cmd before
/// it, and where it left the walk's CoD1 body.
#[derive(Clone)]
struct WalkBody {
    ps: vcod_cod2::pmove::PlayerState,
    old: UserCmd2,
    origin: Vec3,
    velocity: Vec3,
}

impl crate::nav::WalkStep for Cod2Walk {
    fn step(
        &self,
        ps: &mut vcod_common::pmove::PlayerState,
        delta: &mut [i32; 3],
        carry: &mut crate::nav::WalkCarry,
        from_ms: i32,
        cmd: &UserCmd,
        world: &vcod_common::collision::CollisionWorld,
        weapons: &[Option<vcod_common::weapon::WeaponDef>],
    ) {
        use vcod_common::movetrace::MoveWorld;
        // The walk's own CoD1 body is the truth when it moved it since.
        let mut body = carry
            .take()
            .and_then(|c| c.downcast::<WalkBody>().ok())
            .map(Arc::unwrap_or_clone)
            .filter(|b| b.origin == ps.origin && b.velocity == ps.velocity)
            .unwrap_or_else(|| WalkBody {
                ps: cod2_walk_body(ps, from_ms),
                old: UserCmd2::default(),
                origin: ps.origin,
                velocity: ps.velocity,
            });
        body.ps.delta_angles = *delta;
        let cmd2 = to_cmd2(cmd);
        let scale = weapons
            .get(ps.weapon as usize)
            .and_then(Option::as_ref)
            .map_or(1.0, |d| d.move_speed_scale);
        let mw = MoveWorld::bare(world);
        vcod_cod2::pmove::player_step(&mut body.ps, &cmd2, &mut body.old, mw, self.rules, scale);
        *delta = body.ps.delta_angles;
        read_walk_body(&body.ps, ps, cmd2.buttons & button::JUMP != 0, world);
        body.origin = ps.origin;
        body.velocity = ps.velocity;
        *carry = Some(Arc::new(body));
    }

    fn key(&self) -> String {
        self.key.clone()
    }
}

/// A walk's first CoD2 body off its CoD1 one, its clock at `time`: moving
/// as the CoD1 body moves (`old_velocity` its pace, as a body that has run
/// so for a while), its last jump `since_jump_ms` back.
fn cod2_walk_body(
    ps: &vcod_common::pmove::PlayerState,
    time: i32,
) -> vcod_cod2::pmove::PlayerState {
    use vcod_cod2::pmove::{ENTITYNUM_NONE, VIEW_CROUCH, VIEW_PRONE};
    use vcod_common::pmove::Stance;
    let mut b = vcod_cod2::pmove::PlayerState::spawn(ps.origin, ps.yaw.to_degrees(), time);
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
    // `Jump_Check` refuses a jump inside 500 ms of `jumpTime`.
    b.jump_time = time - ps.since_jump_ms.min(10_000.0) as i32;
    b.jump_origin_z = ps.jump_origin_z;
    if ps.jump_penalty {
        b.pm_flags |= pmf::JUMPING;
        b.pm_time = ps.knockback_ms as i32;
    }
    b
}

/// What the walk reads off the CoD2 body `b` into its CoD1 body `ps`; a
/// body off the ground over ground too steep to stand on reads sliding.
fn read_walk_body(
    b: &vcod_cod2::pmove::PlayerState,
    ps: &mut vcod_common::pmove::PlayerState,
    jump_held: bool,
    world: &vcod_common::collision::CollisionWorld,
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
        vcod_cod2::pmove::Stance::Stand => Stance::Stand,
        vcod_cod2::pmove::Stance::Crouch => Stance::Crouch,
        vcod_cod2::pmove::Stance::Prone => Stance::Prone,
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
        let (mins, maxs) = b.stance_box();
        let t = vcod_common::movetrace::MoveWorld::bare(world).box_trace(
            b.origin,
            b.origin - Vec3::Z * 0.25,
            mins,
            maxs,
            vcod_cod2::pmove::MASK_PLAYERSOLID,
        );
        (!t.startsolid && t.fraction < 1.0).then_some(t.normal)
    };
    ps.set_ground_plane(plane);
}

/// CoD2's stock weapon menus (`weapon_<nationality>`), each weapon with
/// the `scr_allow_*` cvar `_weapons::restrictWeaponByServerCvars` checks
/// (`maps/mp/gametypes/_weapons.gsc`, iw_15.iwd).
pub(crate) const WEAPON_MENUS_COD2: [WeaponMenu; 4] = [
    (
        "american",
        &[
            ("m1carbine_mp", "scr_allow_m1carbine"),
            ("m1garand_mp", "scr_allow_m1garand"),
            ("thompson_mp", "scr_allow_thompson"),
            ("bar_mp", "scr_allow_bar"),
            ("springfield_mp", "scr_allow_springfield"),
            ("greasegun_mp", "scr_allow_greasegun"),
            ("shotgun_mp", "scr_allow_shotgun"),
        ],
    ),
    (
        "british",
        &[
            ("enfield_mp", "scr_allow_enfield"),
            ("sten_mp", "scr_allow_sten"),
            ("bren_mp", "scr_allow_bren"),
            ("enfield_scope_mp", "scr_allow_enfieldsniper"),
            ("m1garand_mp", "scr_allow_m1garand"),
            ("thompson_mp", "scr_allow_thompson"),
            ("shotgun_mp", "scr_allow_shotgun"),
        ],
    ),
    (
        "russian",
        &[
            ("mosin_nagant_mp", "scr_allow_nagant"),
            ("SVT40_mp", "scr_allow_svt40"),
            ("PPS42_mp", "scr_allow_pps42"),
            ("ppsh_mp", "scr_allow_ppsh"),
            ("mosin_nagant_sniper_mp", "scr_allow_nagantsniper"),
            ("shotgun_mp", "scr_allow_shotgun"),
        ],
    ),
    (
        "german",
        &[
            ("kar98k_mp", "scr_allow_kar98k"),
            ("g43_mp", "scr_allow_g43"),
            ("mp40_mp", "scr_allow_mp40"),
            ("mp44_mp", "scr_allow_mp44"),
            ("kar98k_sniper_mp", "scr_allow_kar98ksniper"),
            ("shotgun_mp", "scr_allow_shotgun"),
        ],
    ),
];

/// `weaponClass` `rifle` and `pistol` (`vcod_cod2::weapon::WEAPON_CLASSES`).
const CLASS_RIFLE: i32 = 0;
const CLASS_PISTOL: i32 = 4;
/// `weaponType` `bullet`.
const TYPE_BULLET: i32 = 0;
/// `offhandClass` `Frag Grenade`, `Smoke Grenade`.
const OFFHAND_FRAG: i32 = 1;
const OFFHAND_SMOKE: i32 = 2;
/// A smoke cloud stops a bot's eye line from this long after the burst
/// until this long after it, inside this radius of the burst raised by
/// [`SMOKE_LIFT`]: `fx/explosions/smoke_grenade.efx`'s volumes start
/// 3000 to 8000 ms in, live 30 to 40 s, and spread 150 to 200 units.
/// vcod's own (retail's bots are none; its `SV_FX_GetVisibility` is not
/// ported).
const SMOKE_FORM_MS: i32 = 2000;
const SMOKE_GONE_MS: i32 = 35_000;
const SMOKE_RADIUS: f32 = 150.0;
const SMOKE_LIFT: f32 = 48.0;
/// A smoke throw is for an enemy this far off: nearer, the fight is on.
const SMOKE_MIN_RANGE: f32 = 400.0;
const SMOKE_MAX_RANGE: f32 = 1500.0;
/// How near an objective (the bot's site, the planted bomb) a bot heading
/// to plant or defuse throws.
const SMOKE_OBJECTIVE_REACH: f32 = 800.0;
/// The cloud lands on the line to the enemy this far out, or a third of
/// the way, whichever is further, and at least [`SMOKE_SHORT`] short of
/// him.
const SMOKE_OUT: f32 = 250.0;
const SMOKE_SHORT: f32 = 150.0;
/// A throw that has not let go in this many ticks is dropped.
const SMOKE_TICKS: u32 = 60;
/// The view is on the throw's aim within this, degrees, before the key
/// comes up.
const SMOKE_AIMED_DEG: f32 = 2.0;

/// The CoD2 server's bots: the shared driver's state ([`Self::driver`])
/// and what CoD2's hooks keep beside it (the throws, guns, hits heard and
/// smoke).
pub struct Cod2Bots {
    /// The brains, the graph and what the shared driver keeps per bot.
    pub(crate) driver: super::driver::BotDriver,
    shoot: bool,
    /// The management's state (`crate::server::bot_admin`), shared with
    /// CoD1's.
    pub admin: crate::server::bot_admin::Ledger,
    /// The `vc_bot_shoot` and `vc_bot_skill` values last applied.
    pub applied: Option<(bool, u8)>,
    /// Whether bots man the mounted guns ([`gun`]); `bot_match`'s
    /// `GUNS=0` turns it off for a before and after.
    pub man_guns: bool,
    /// The bots whose last cmd asked for the frag ([`offhand_cmd`]), and
    /// whether the brain has held the trigger on it since.
    throwing: HashMap<usize, bool>,
    /// The bots following a mantle edge this tick.
    mantling: HashMap<usize, bool>,
    /// The bots a mantle onto a ledge too low to stand on forced to crouch
    /// ([`Self::forced_crouch`]), and each bot's `eventSequence` last read
    /// for it.
    crouched: HashSet<usize>,
    stance_seq: HashMap<usize, i32>,
    /// Each bot's walk to a gun or its turn on one ([`gun`]).
    guns: HashMap<usize, gun::GunTask>,
    /// Each bot's walk to a dropped gun ([`fetch`]).
    fetches: HashMap<usize, fetch::Fetch>,
    /// Each bot's view at the last tick: the view its last cmd's shots
    /// left from, which [`BotView::gun_offset`] is read off.
    last_views: HashMap<usize, [f32; 2]>,
    /// Smoke grenades thrown this game.
    smokes: u32,
    /// Each bot's `damageEvent` last read and the hit it reported: the
    /// attacker, the level time read, the direction it came along.
    hits: HashMap<usize, (i32, Option<HitFelt>)>,
    /// Every client's `eventSequence` last read, and the gunfire and blasts
    /// of the frame since the last tick.
    heard: HashMap<usize, i32>,
    last_tick: i32,
    /// The smoke clouds burst, each burst point and its level time.
    clouds: Vec<(Vec3, i32)>,
    /// The smoke throws under way, and each bot's retreat last tick.
    smoking: HashMap<usize, SmokeThrow>,
    retreats: HashMap<usize, u8>,
    pass: Pass,
}

/// A hit a bot felt: the attacker, the level time, the direction.
type HitFelt = (usize, i32, [f32; 3]);

/// A smoke grenade on its way out: the throw's (pitch, yaw), the smoke
/// weapon, whether the pullback has reached the hold, and the ticks
/// since the ask.
#[derive(Clone, Copy, Debug)]
pub struct SmokeThrow {
    aim: [f32; 2],
    weapon: u8,
    ticks: u32,
}

/// What one tick carries from one of the driver's hooks to a later one.
#[derive(Default)]
struct Pass {
    /// Each bot's reliable server commands since the last tick.
    inbox: BTreeMap<usize, Vec<String>>,
    /// Every bot's weapon pick, as the weapon spread counts them.
    loadouts: Vec<(usize, &'static str)>,
    now: i32,
    dm: bool,
    guns: Vec<crate::game::cod2_turret::GunInfo>,
    /// The bot whose hit was just read, and the hit.
    felt: Option<(usize, Option<super::Hurt>)>,
    /// The bot in pass 3 and its smoke throw, gun step, fetch step and
    /// whether it is on its way to a gun.
    smoke: Option<(usize, SmokeThrow)>,
    steps: Option<(usize, Option<gun::GunCmd>, Option<fetch::FetchCmd>, bool)>,
}

/// The tick's objective state ([`super::driver::BotWorld::Frame`]).
pub(crate) struct Cod2Frame(Option<Objectives>);

/// What the stock objective scripts hold this frame, read once a tick.
enum Objectives {
    Sd(crate::game::script::SdObjectives),
    Uo(crate::game::script::UoObjectives),
}

impl Cod2Bots {
    pub fn new(shoot: bool) -> Cod2Bots {
        Cod2Bots {
            driver: super::driver::BotDriver {
                nav_background: true,
                ..Default::default()
            },
            shoot,
            admin: Default::default(),
            applied: None,
            man_guns: true,
            throwing: HashMap::new(),
            mantling: HashMap::new(),
            crouched: HashSet::new(),
            stance_seq: HashMap::new(),
            guns: HashMap::new(),
            fetches: HashMap::new(),
            last_views: HashMap::new(),
            smokes: 0,
            hits: HashMap::new(),
            heard: HashMap::new(),
            last_tick: 0,
            clouds: Vec::new(),
            smoking: HashMap::new(),
            retreats: HashMap::new(),
            pass: Pass::default(),
        }
    }

    /// A brain for the client in `slot`, answering the team menu with
    /// `team` (`autoassign`, `allies` or `axis`).
    pub fn add(&mut self, slot: usize, name: &str, team: &str, seed: u64) {
        let mut bot = Bot::new(team, self.shoot, seed);
        bot.name = name.to_string();
        bot.menus = &WEAPON_MENUS_COD2;
        self.driver.bots.insert(slot, bot);
    }

    /// A new level under the bots (`map`, `map_restart`, `fast_restart`,
    /// `map_rotate`): every brain forgets the menus it answered, as the
    /// level reopens them under the same indices (CoD1's `Bot::rearm` on a
    /// gamestate), and what the old level left (paths, enemies, hits,
    /// heard events, guns, smoke) is dropped. `new_map` drops the graph
    /// too, for the next tick to build the new map's.
    pub fn new_level(&mut self, new_map: bool) {
        let mut fresh = Cod2Bots::new(self.shoot);
        fresh.admin = std::mem::take(&mut self.admin);
        fresh.applied = self.applied;
        fresh.driver.nav_background = self.driver.nav_background;
        fresh.man_guns = self.man_guns;
        fresh.smokes = self.smokes;
        if !new_map {
            fresh.driver.nav_job = self.driver.nav_job.take();
            fresh.driver.nav = self.driver.nav.take();
            fresh.driver.creases = std::mem::take(&mut self.driver.creases);
            fresh.driver.sites = std::mem::take(&mut self.driver.sites);
            fresh.driver.radios = std::mem::take(&mut self.driver.radios);
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

    /// Smoke grenades the bots threw this game (`bot_match`).
    pub fn smokes(&self) -> u32 {
        self.smokes
    }

    pub fn remove(&mut self, slot: usize) {
        self.admin.pinned.remove(&slot);
        self.admin.skills.remove(&slot);
        self.driver.bots.remove(&slot);
        self.driver.paths.remove(&slot);
        self.driver.enemies.remove(&slot);
        self.driver.hunts.remove(&slot);
        self.throwing.remove(&slot);
        self.mantling.remove(&slot);
        self.crouched.remove(&slot);
        self.stance_seq.remove(&slot);
        self.guns.remove(&slot);
        self.fetches.remove(&slot);
        self.last_views.remove(&slot);
        self.hits.remove(&slot);
        self.driver.covers.remove(&slot);
        self.heard.remove(&slot);
        self.smoking.remove(&slot);
        self.retreats.remove(&slot);
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

    /// The graph's build under CoD2's movement and mantle probe, polled
    /// until it lands (`BotWorld::nav_tick`).
    pub(crate) fn nav_tick(&mut self, level: &Cod2Level, map: &str) {
        if self.driver.nav.is_none() {
            let job = self.driver.nav_job.get_or_insert_with(|| {
                NavJob::start_opts(
                    map,
                    &level.world,
                    GRAPH_RULES,
                    Some(mantle_probe(level.rules)),
                    None,
                    LIP,
                )
            });
            self.driver.nav = job.poll(!self.driver.nav_background);
        }
    }

    /// Each bot's reliable server commands since the last tick, for
    /// [`Self::read_commands`]; the weapon spread reads every bot's pick as
    /// it stood here.
    pub(crate) fn inbox(&mut self, inbox: BTreeMap<usize, Vec<String>>) {
        self.pass.loadouts = self
            .driver
            .bots
            .iter()
            .filter_map(|(s, b)| Some((*s, b.loadout()?.1)))
            .collect();
        self.pass.inbox = inbox;
    }

    /// The `t` menus the bot in `slot` was sent, named off their
    /// configstrings, and the `mr` answers into `replies`
    /// (`serverinfo_*` gets `close`, as a client's `close` button sends).
    pub(crate) fn read_commands(
        &mut self,
        level: &Cod2Level,
        slot: usize,
        configstrings: &[String],
        server_id: i32,
        replies: &mut Vec<(usize, String)>,
    ) {
        use vcod_common::net::protocol::CsRange;
        let base = vcod_cod2::net::protocol::CS_LAYOUT_118
            .bounds(CsRange::Menu)
            .0;
        let cvars = level.rt.cvars();
        let allowed = |name: &str| {
            crate::game::builtins::cvar::atof(cvars.get(name)) != 0.0 || name.is_empty()
        };
        let Some(texts) = self.pass.inbox.get(&slot) else {
            return;
        };
        let Some(bot) = self.driver.bots.get_mut(&slot) else {
            return;
        };
        let loadouts = &mut self.pass.loadouts;
        let mut named: Vec<(i32, String)> = Vec::new();
        for (seq, text) in texts.iter().enumerate() {
            let menu = text
                .strip_prefix("t ")
                .and_then(|i| i.split_whitespace().next()?.parse::<usize>().ok())
                .map(|i| (i, configstrings.get(base + i).map_or("", String::as_str)));
            match menu {
                Some((i, m)) if m.starts_with("serverinfo_") => {
                    replies.push((slot, format!("mr {server_id} {i} close")));
                    continue;
                }
                Some((_, m)) if !m.is_empty() => {
                    named.push((seq as i32, format!("v g_scriptMainMenu {m}")));
                }
                _ => {}
            }
            named.push((seq as i32, text.clone()));
        }
        if let Some(menu) = named
            .iter()
            .find_map(|(_, t)| t.strip_prefix("v g_scriptMainMenu weapon_"))
        {
            // CoD1's pick is a draw; a CoD2 bot takes the weapon its
            // menu offers that the fewest other bots carry.
            if bot.loadout().is_none_or(|(m, _)| m != menu) {
                let carried: Vec<&'static str> = loadouts
                    .iter()
                    .filter(|(s, _)| *s != slot)
                    .map(|(_, w)| *w)
                    .collect();
                let roll = bot.rng;
                if let Some(w) = spread_pick(menu, &carried, &allowed, roll) {
                    bot.set_loadout(menu, w);
                    loadouts.retain(|(s, _)| *s != slot);
                    loadouts.push((slot, w));
                }
            }
        }
        let named: Vec<(i32, &str)> = named.iter().map(|(q, t)| (*q, t.as_str())).collect();
        for r in bot.observe(server_id, &named, &allowed) {
            replies.push((slot, r));
        }
    }

    /// The stock objective scripts' state and the guns, read once a tick
    /// (`BotWorld::frame`).
    pub(crate) fn frame(&mut self, level: &mut Cod2Level) -> Cod2Frame {
        let gametype = level.rt.cvars().get("g_gametype").to_string();
        let objectives = match gametype.as_str() {
            "sd" => Some(Objectives::Sd(level.rt.cod2_sd_objectives())),
            "ctf" => level
                .rt
                .cod2_ctf_objectives(level.players.len())
                .map(Objectives::Uo),
            "hq" => level
                .rt
                .uo_objectives("hq", level.players.len())
                .map(Objectives::Uo),
            _ => None,
        };
        // The guns only where no objective deals out roles.
        self.pass.guns = if objectives.is_none() && self.man_guns {
            level.gun_list()
        } else {
            Vec::new()
        };
        self.pass.now = level.level_time();
        self.pass.dm = gametype == "dm";
        Cod2Frame(objectives)
    }

    /// The hit the bot in `slot` felt this tick ([`Self::hurt`]).
    pub(crate) fn feel(&mut self, level: &Cod2Level, slot: usize) {
        let hurt = self.hurt(level, slot, self.pass.now);
        self.pass.felt = Some((slot, hurt));
    }

    fn felt(&self, slot: usize) -> Option<super::Hurt> {
        self.pass.felt.filter(|f| f.0 == slot).and_then(|f| f.1)
    }

    /// The enemy the bot in `slot` fights, the one that just hit it seen
    /// wherever it stands.
    pub(crate) fn sight(&self, level: &Cod2Level, slot: usize, teams: &[i32]) -> Option<EnemyView> {
        let aware = self.driver.bots.get(&slot).is_none_or(|b| b.aware);
        let attacker = self.felt(slot).map(|h| h.slot).filter(|_| aware);
        self.enemy(level, slot, teams, self.pass.dm, attacker, self.pass.now)
    }

    /// [`Self::view`] with the hit felt.
    pub(crate) fn felt_view(
        &self,
        level: &Cod2Level,
        slot: usize,
        enemy: Option<EnemyView>,
    ) -> Option<BotView> {
        let mut view = self.view(level, slot, enemy)?;
        view.hurt = self.felt(slot);
        Some(view)
    }

    /// Pass 2's objective roles (`BotWorld::deal`).
    pub(crate) fn deal(
        &mut self,
        level: &Cod2Level,
        frame: Cod2Frame,
        teams: &[i32],
        round: u32,
        views: &mut [(usize, BotView)],
    ) {
        match &frame.0 {
            Some(Objectives::Sd(o)) => self.deal_sd(o, teams, views, round),
            Some(Objectives::Uo(o)) => self.deal_uo(level, o, teams, views),
            None => {}
        }
    }

    /// The smoke throw ahead of the route ([`Self::smoke_start`]): the frag
    /// waits on its key.
    pub(crate) fn pre_route(
        &mut self,
        level: &Cod2Level,
        slot: usize,
        view: &mut BotView,
        teams: &[i32],
    ) {
        let smoke = self.smoke_start(level, slot, view, teams);
        if smoke.is_some() {
            view.grenade = None;
        }
        self.pass.smoke = smoke.map(|t| (slot, t));
    }

    /// The bots following a mantle edge (`BotWorld::routed`).
    pub(crate) fn routed(&mut self, slot: usize, follower: &crate::nav::Follower, g: &NavGraph) {
        self.mantling.insert(slot, follower.mantling(g));
    }

    /// The walk to a gun, else to a dropped one, over the route's waypoint.
    pub(crate) fn pre_think(
        &mut self,
        level: &Cod2Level,
        slot: usize,
        view: &mut BotView,
        teams: &[i32],
    ) {
        let guns = std::mem::take(&mut self.pass.guns);
        let gun_step = self.gun_step(level, slot, view, &guns, teams, self.pass.dm);
        self.pass.guns = guns;
        let fetch_step = if gun_step.is_none() {
            self.fetch_step(level, slot, view)
        } else {
            None
        };
        let fetching = self.fetching(slot);
        self.pass.steps = Some((slot, gun_step, fetch_step, fetching));
    }

    /// A dry bot on its way to a gun lying near holds off the brain's
    /// respawn.
    pub(crate) fn keeps_alive(&self, slot: usize) -> bool {
        self.pass.steps.as_ref().is_some_and(|s| s.0 == slot && s.3)
    }

    /// The brain's `cmd` in CoD2's bits, with the frag, the smoke, the
    /// mantle, the gun, the fetch and the breath over it, stamped for the
    /// tick; `None` with no body.
    pub(crate) fn cmd(
        &mut self,
        level: &Cod2Level,
        slot: usize,
        cmd: UserCmd,
        view: &BotView,
    ) -> Option<Vec<UserCmd2>> {
        let turn_deg = self.driver.bots.get(&slot)?.skill.turn_deg;
        let p = level.players[slot].as_ref()?;
        let smoke = self.pass.smoke.take().filter(|s| s.0 == slot).map(|s| s.1);
        let (gun_step, fetch_step) = match self.pass.steps.take() {
            Some((s, g, f, _)) if s == slot => (g, f),
            _ => (None, None),
        };
        let throwing = self.throwing.remove(&slot);
        let (mut c2, armed) = offhand_cmd(&cmd, view.grenade, p.ws.weapon, throwing);
        if let Some(armed) = armed {
            self.throwing.insert(slot, armed);
        }
        if let Some(t) = smoke {
            self.smoke_cmd(slot, t, view, p, &mut c2);
        }
        // A mantle edge: on to the ledge with the jump key once pmove
        // offers the climb (`Mantle_CheckLedge`'s hint); a jump short
        // of it would leave the ground and lose the hint.
        if self.mantling.get(&slot).copied().unwrap_or(false) {
            c2.buttons &= !button::JUMP;
            if p.ps.mantle.flags & vcod_cod2::pmove::mantle::flag::HINT != 0 {
                c2.buttons |= button::JUMP;
            }
        }
        if self.forced_crouch(level, slot) {
            c2.buttons = (c2.buttons & !button::PRONE) | button::CROUCH;
        }
        if let Some(step) = &gun_step {
            gun::apply(&mut c2, step, p.ps.delta_angles);
        }
        if let Some(f) = &fetch_step {
            fetch::apply(&mut c2, f, view, turn_deg);
        }
        let scoped = level
            .combat
            .weapons
            .get(usize::from(p.ws.weapon))
            .and_then(Option::as_ref)
            .is_some_and(|d| d.ads_overlay_reticle != 0 && d.weap_class != 9);
        let breath = Breath {
            held: p.ps.pm_flags & vcod_cod2::pmove::weapon::wpf::HOLD_BREATH != 0,
            timer: p.ws.hold_breath_timer,
        };
        let sighted = scoped && p.ws.pos_frac >= 1.0 && c2.buttons & button::ADS != 0;
        if hold_breath(breath, sighted && view.enemy.is_some()) {
            c2.buttons |= vcod_cod2::pmove::weapon::key::HOLD_BREATH;
        }
        self.last_views
            .insert(slot, [p.ps.view_angles[0], p.ps.view_angles[1]]);
        let climbing = view.on_ladder && !view.on_ground;
        Some(
            crate::nav::tick_times(self.pass.now, climbing)
                .map(|t| UserCmd2 {
                    server_time: t,
                    ..c2
                })
                .collect(),
        )
    }

    /// Whether the bot in `slot` holds crouch for a mantle: an
    /// `EV_STANCE_FORCE_CROUCH` in its own ring since the last tick sets
    /// the client's stance to crouch and `EV_STANCE_FORCE_STAND` back to
    /// stand, as `CG_EntityEvent` does for its own client (Mac 0x1e4650,
    /// cod2-movement.md 9); a player then stands up himself, so the bot
    /// does once a standing body fits where it stands, on the ground.
    /// Standing in the mantle's end under mp_breakout's ledge at (5809,
    /// 4944, 96), a bot hung in the solid for the rest of the round
    /// (bot-objectives.md 11.4).
    fn forced_crouch(&mut self, level: &Cod2Level, slot: usize) -> bool {
        use vcod_cod2::pmove::{MASK_PLAYERSOLID, PLAYER_MAXS, PLAYER_MINS};
        let Some(ps) = level
            .players
            .get(slot)
            .and_then(Option::as_ref)
            .map(|p| &p.ps)
        else {
            return false;
        };
        let seq = ps.event_sequence;
        let from = self.stance_seq.insert(slot, seq).unwrap_or(seq);
        if ps.pm_type != pm_type::NORMAL {
            self.crouched.remove(&slot);
            return false;
        }
        match forced_stance(ps, from) {
            Some(true) => {
                self.crouched.insert(slot);
            }
            Some(false) => {
                self.crouched.remove(&slot);
            }
            None => {}
        }
        if self.crouched.contains(&slot)
            && ps.on_ground()
            && !vcod_common::movetrace::MoveWorld::bare(&level.world.collision)
                .box_trace(
                    ps.origin,
                    ps.origin,
                    PLAYER_MINS,
                    PLAYER_MAXS,
                    MASK_PLAYERSOLID,
                )
                .startsolid
        {
            self.crouched.remove(&slot);
        }
        self.crouched.contains(&slot)
    }

    /// The tick's end: the missiles burst since now are the next tick's.
    pub(crate) fn tick_done(&mut self) {
        self.last_tick = self.pass.now;
        self.pass = Pass::default();
    }

    /// The S&D view off `sd.gsc`'s state, as the CoD1 driver deals it
    /// (`Server`'s `BotWorld::deal`, bot-objectives.md 2 and 9): the role off
    /// `game["attackers"]`, the site rank, the defuse lead (the team's
    /// playing bot nearest the bomb) and a planting teammate to cover.
    fn deal_sd(
        &mut self,
        o: &crate::game::script::SdObjectives,
        teams: &[i32],
        views: &mut [(usize, BotView)],
        round: u32,
    ) {
        use super::{BombView, ObjRole, SdView, SiteView};
        let side = |t: &str| match t {
            "axis" => TEAM_AXIS,
            "allies" => TEAM_ALLIES,
            _ => -1,
        };
        let (attackers, defenders) = (side(&o.attackers), side(&o.defenders));
        let sites: Vec<SiteView> = o
            .sites
            .iter()
            .map(|&(mins, maxs)| SiteView {
                mins,
                maxs,
                stand: self.zone_stand(mins, maxs),
            })
            .collect();
        let bomb = o.bomb.map(|(origin, (lo, hi))| BombView {
            origin,
            aim: std::array::from_fn(|i| (lo[i] + hi[i]) * 0.5),
        });
        let team = |slot: usize| teams.get(slot).copied().unwrap_or(0);
        let bomb_dist = |v: &BotView| bomb.map_or(0.0, |b| super::dist_sq(v.origin, b.origin));
        let roster: Vec<(usize, i32, f32, bool)> = views
            .iter()
            .map(|(s, v)| (*s, team(*s), bomb_dist(v), v.playing))
            .collect();
        let planting: Vec<(usize, [f32; 3], bool)> = views
            .iter()
            .map(|(s, v)| {
                let held =
                    v.playing && v.linked && sites.iter().any(|z| super::inside_site(v.origin, z));
                (*s, v.origin, held)
            })
            .collect();
        for (slot, view) in views.iter_mut() {
            let mine = team(*slot);
            let role = if mine == attackers {
                ObjRole::Attack
            } else if mine == defenders {
                ObjRole::Defend
            } else {
                continue;
            };
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
            let lead = roster
                .iter()
                .filter(|r| r.1 == mine && r.3)
                .min_by(|a, b| a.2.total_cmp(&b.2).then(a.0.cmp(&b.0)))
                .is_some_and(|r| r.0 == *slot);
            view.sd = Some(SdView {
                role,
                sites: sites.clone(),
                bomb,
                rank,
                lead,
                cover,
                key: None,
                hot: None,
            });
        }
    }

    /// The `ctf` and `hq` views, as `Server::bot_uo_view` deals United
    /// Offensive's: the rank is the bot's place among its team's bots by
    /// slot.
    fn deal_uo(
        &mut self,
        level: &Cod2Level,
        o: &crate::game::script::UoObjectives,
        teams: &[i32],
        views: &mut [(usize, BotView)],
    ) {
        use super::{
            CtfFlagView, CtfView, HqRadioView, HqView, RADIO_HEIGHT_COD2, RADIO_MARGIN, SiteView,
            UoView,
        };
        use crate::game::script::UoObjectives;
        let team = |slot: usize| teams.get(slot).copied().unwrap_or(0);
        let slots: Vec<usize> = views.iter().map(|(s, _)| *s).collect();
        let origin_of = |c: usize| -> Option<[f32; 3]> {
            Some(level.players.get(c)?.as_ref()?.ps.origin.into())
        };
        for (slot, view) in views.iter_mut() {
            let mine = team(*slot);
            if mine != TEAM_AXIS && mine != TEAM_ALLIES {
                continue;
            }
            let rank = slots
                .iter()
                .filter(|s| team(**s) == mine && *s < slot)
                .count();
            view.uo = match o {
                UoObjectives::Ctf(flags) => {
                    let ours = flags.iter().find(|f| f.team == mine);
                    let theirs = flags.iter().find(|f| f.team != mine);
                    let (Some(ours), Some(theirs)) = (ours, theirs) else {
                        continue;
                    };
                    let mut site = |(mins, maxs): crate::game::script::Bounds| SiteView {
                        mins,
                        maxs,
                        stand: self.zone_stand(mins, maxs),
                    };
                    let mut flag = |f: &crate::game::script::CtfFlag| CtfFlagView {
                        home: f.home.map(&mut site),
                        dropped: f.dropped.map(&mut site),
                        goal: site(f.goal),
                        carried: f.carrier.is_some(),
                    };
                    let (ours_v, theirs_v) = (flag(ours), flag(theirs));
                    Some(UoView::Ctf(Box::new(CtfView {
                        ours: ours_v,
                        theirs: theirs_v,
                        carrying: theirs.carrier == Some(*slot),
                        escort: theirs.carrier.filter(|c| c != slot).and_then(origin_of),
                        rank,
                    })))
                }
                UoObjectives::Hq { radios, next } => Some(UoView::Hq(HqView {
                    radio: radios.first().map(|r| HqRadioView {
                        at: r.at,
                        radius: r.radius,
                        height: RADIO_HEIGHT_COD2,
                        // The radio's own point can lie past the reach
                        // of the floor a capture stands on: mp_trainstation's
                        // first sits 50 over the floor beside it, and its
                        // nearest node is a platform 115 across
                        // (bot-objectives.md 11.6).
                        stand: self.driver.radio_stand(
                            r.at,
                            r.radius - 2.0 * RADIO_MARGIN,
                            RADIO_HEIGHT_COD2 - RADIO_MARGIN,
                        ),
                        ours: r.team == mine,
                    }),
                    next: *next,
                    rank,
                })),
                _ => None,
            };
        }
    }

    /// [`super::driver::BotDriver::zone_stand`].
    fn zone_stand(&mut self, mins: [f32; 3], maxs: [f32; 3]) -> [f32; 3] {
        self.driver.zone_stand(mins, maxs)
    }

    /// The enemy in sight the CoD1 driver would pick (`Server::bot_enemy`):
    /// the tracked target, else the nearest in the sight cone within
    /// range whose chest or eye the eye line reaches.
    /// `attacker` is the client who just hit the bot ([`BotView::hurt`]),
    /// whom it sees wherever he stands, as the tracked target; a smoke
    /// cloud between ([`Self::smoked`]) hides anyone.
    fn enemy(
        &self,
        level: &Cod2Level,
        slot: usize,
        teams: &[i32],
        dm: bool,
        attacker: Option<usize>,
        now: i32,
    ) -> Option<EnemyView> {
        if !self.shoot || !playing(level, slot) {
            return None;
        }
        let me = level.players[slot].as_ref()?;
        let bot = self.driver.bots.get(&slot);
        let mine = teams.get(slot).copied().unwrap_or(0);
        let tracked = bot
            .and_then(|b| b.target_slot())
            .filter(|_| bot.is_none_or(|b| b.aware));
        let fov = bot.map_or(super::Skill::default().fov_deg, |b| b.skill.fov_deg);
        let my_eye = eye(&me.ps);
        let mut candidates: Vec<(f32, Vec3, EnemyView)> = Vec::new();
        for (i, p) in level.players.iter().enumerate() {
            let Some(p) = p.as_ref().filter(|_| i != slot && playing(level, i)) else {
                continue;
            };
            let theirs = teams.get(i).copied().unwrap_or(0);
            let foe = dm
                || !(mine == TEAM_AXIS || mine == TEAM_ALLIES)
                || (theirs != mine && (theirs == TEAM_AXIS || theirs == TEAM_ALLIES));
            if !foe {
                continue;
            }
            let d = super::dist_sq(me.ps.origin.into(), p.ps.origin.into());
            if d > super::SHOOT_RANGE * super::SHOOT_RANGE {
                continue;
            }
            let chest = super::chest(p.ps.origin.into(), p.ps.view_height_current);
            let known = Some(i) == tracked || Some(i) == attacker;
            if !known && !super::in_sight_cone(my_eye, me.ps.view_angles, chest.into(), fov) {
                continue;
            }
            candidates.push((
                d,
                eye(&p.ps),
                EnemyView {
                    slot: i,
                    origin: chest,
                    velocity: p.ps.velocity.into(),
                },
            ));
        }
        candidates.sort_by(|a, b| a.0.total_cmp(&b.0));
        let w = &level.world.collision;
        let sighted = |i: usize| {
            let (_, their_eye, e) = &candidates[i];
            let clear = |to: [f32; 3]| {
                w.shot_trace(my_eye, to.into()).fraction >= 1.0
                    && !self.smoked(my_eye, to.into(), now)
            };
            let origin = super::aim_point(e.origin, (*their_eye).into(), clear)?;
            Some(EnemyView { origin, ..*e })
        };
        let order: Vec<(f32, usize)> = candidates.iter().map(|c| (c.0, c.2.slot)).collect();
        super::pick_enemy(&order, attacker, tracked, sighted)
    }

    /// Whether the segment `a` to `b` passes through a smoke cloud thick
    /// at `now` ([`SMOKE_FORM_MS`]).
    fn smoked(&self, a: Vec3, b: Vec3, now: i32) -> bool {
        self.clouds.iter().any(|&(at, t)| {
            let age = now.wrapping_sub(t);
            (SMOKE_FORM_MS..SMOKE_GONE_MS).contains(&age)
                && segment_dist(a, b, at + Vec3::Z * SMOKE_LIFT) < SMOKE_RADIUS
        })
    }

    /// The frame's noises and clouds since the last tick: each playing
    /// client's new fire events (`EV_FIRE_WEAPON`, `_LASTSHOT`) chest high
    /// as gunfire, and each missile burst since as a blast, a smoke
    /// grenade's as a cloud, as the CoD1 driver gathers them
    /// (`BotDriver::noises`).
    pub(crate) fn listen(&mut self, level: &Cod2Level) -> Vec<super::Noise> {
        use vcod_cod2::pmove::weapon::ev;
        // The tick's top, ahead of `frame`: the pass holds no time yet.
        let now = level.level_time();
        let mut noises = Vec::new();
        for (i, p) in level.players.iter().enumerate() {
            let Some(p) = p.as_ref().filter(|_| playing(level, i)) else {
                self.heard.remove(&i);
                continue;
            };
            let seq = p.ps.event_sequence;
            let prev = self.heard.insert(i, seq).unwrap_or(seq);
            let fired = (prev.max(seq - 4)..seq).any(|k| {
                matches!(
                    p.ps.events[(k & 3) as usize],
                    ev::FIRE_WEAPON | ev::FIRE_WEAPON_LASTSHOT
                )
            });
            if fired {
                noises.push(super::Noise {
                    at: super::chest(p.ps.origin.into(), p.ps.view_height_current),
                    source: i,
                    radius: super::HEAR_GUNFIRE,
                });
            }
        }
        let since = self.last_tick;
        for m in &level.combat.missiles {
            let Some(t) = m.exploded_at.filter(|&t| t.wrapping_sub(since) > 0) else {
                continue;
            };
            let def = level
                .combat
                .weapons
                .get(usize::from(m.weapon))
                .and_then(Option::as_ref);
            if def.is_some_and(|d| d.offhand_class == OFFHAND_SMOKE) {
                self.clouds.push((m.origin, t));
            } else if let Some(owner) = m.owner
                && def.is_some_and(|d| d.explosion_radius > 0)
            {
                noises.push(super::Noise {
                    at: m.origin.into(),
                    source: owner,
                    radius: super::HEAR_BLAST,
                });
            }
        }
        self.clouds
            .retain(|&(_, t)| (0..SMOKE_GONE_MS).contains(&now.wrapping_sub(t)));
        noises
    }

    /// [`BotView::hurt`] for the bot in `slot`: a `damageEvent` new since
    /// the last tick is a hit, by the client `finishPlayerDamage` last
    /// named (`Cod2Player::last_attacker`), along the direction it came
    /// (`Cod2Player::damage_dir`; zero for none); reported for
    /// [`super::HURT_MS`] while the attacker still plays, as the CoD1
    /// driver's `bot_hurt`.
    fn hurt(&mut self, level: &Cod2Level, slot: usize, now: i32) -> Option<super::Hurt> {
        let p = level.players.get(slot)?.as_ref()?;
        let seen = self.hits.entry(slot).or_insert((p.damage_event, None));
        if seen.0 != p.damage_event {
            seen.0 = p.damage_event;
            seen.1 = p
                .last_attacker
                .filter(|&(_, t)| p.damage_event != 0 && (0..1000).contains(&now.wrapping_sub(t)))
                .map(|(a, _)| (a, now, p.damage_dir().unwrap_or(Vec3::ZERO).to_array()));
        }
        let (a, at, dir) = seen.1?;
        let ago_ms = now.wrapping_sub(at);
        if !(0..super::HURT_MS).contains(&ago_ms) || !playing(level, a) {
            return None;
        }
        Some(super::Hurt {
            slot: a,
            ago_ms,
            dir,
        })
    }

    /// The smoke throw the bot in `slot` is under way with, or the one it
    /// starts this tick: with a smoke grenade, on the ground, no frag on
    /// its way and an enemy in sight [`SMOKE_MIN_RANGE`] to
    /// [`SMOKE_MAX_RANGE`] off, when it heads for its plant or defuse
    /// within [`SMOKE_OBJECTIVE_REACH`], covers a planting teammate, or
    /// has just started a retreat. The cloud lands on the line to the
    /// enemy ([`smoke_point`]) on a lob [`lob_probe`] finds.
    fn smoke_start(
        &mut self,
        level: &Cod2Level,
        slot: usize,
        view: &BotView,
        teams: &[i32],
    ) -> Option<SmokeThrow> {
        let retreat = self.driver.bots.get(&slot).map_or(0, |b| b.retreating());
        let was = self.retreats.insert(slot, retreat).unwrap_or(0);
        if !view.playing {
            self.smoking.remove(&slot);
            return None;
        }
        if let Some(t) = self.smoking.get(&slot) {
            return Some(*t);
        }
        let p = level.players.get(slot)?.as_ref()?;
        let e = view.enemy?;
        if self.throwing.contains_key(&slot) || !view.on_ground || view.on_ladder || view.linked {
            return None;
        }
        let range = super::dist_sq(view.origin, e.origin).sqrt();
        if !(SMOKE_MIN_RANGE..SMOKE_MAX_RANGE).contains(&range) {
            return None;
        }
        let near = |at: [f32; 3]| super::dist_sq(view.origin, at) < SMOKE_OBJECTIVE_REACH.powi(2);
        let objective = view.sd.as_ref().is_some_and(|sd| match sd.role {
            super::ObjRole::Attack => {
                sd.cover.is_some()
                    || (sd.bomb.is_none()
                        && !sd.sites.is_empty()
                        && near(sd.sites[sd.rank % sd.sites.len()].stand))
            }
            super::ObjRole::Defend => sd.lead && sd.bomb.is_some_and(|b| near(b.origin)),
        });
        let retreating = was == 0 && matches!(retreat, 1 | 2);
        if !objective && !retreating {
            return None;
        }
        let defs = &level.combat.weapons;
        let weapon = (1u8..64).find(|&w| {
            p.ws.held >> w & 1 == 1
                && defs
                    .get(usize::from(w))
                    .and_then(Option::as_ref)
                    .is_some_and(|d| {
                        d.offhand_class == OFFHAND_SMOKE
                            && p.ws.clip.get(d.clip_index).copied().unwrap_or(0)
                                + p.ws.ammo.get(d.ammo_index).copied().unwrap_or(0)
                                > 0
                    })
        })?;
        let at = smoke_point(view.origin, e.origin);
        let aim = lob_probe(level, slot, weapon, at, teams)?;
        let t = SmokeThrow {
            aim,
            weapon,
            ticks: 0,
        };
        self.smoking.insert(slot, t);
        Some(t)
    }

    /// The throw's cmd over the brain's `c2`: the view turned toward the
    /// lob at the bot's turn rate, the gun's trigger and sight up, the
    /// smoke key held through the pullback until `OFFHAND_HOLD` and the
    /// view on the lob, then let go, which throws (`PM_Weapon`'s state 14).
    /// The path's keys follow the turn.
    fn smoke_cmd(
        &mut self,
        slot: usize,
        mut t: SmokeThrow,
        view: &BotView,
        p: &crate::game::cod2::Cod2Player,
        c2: &mut UserCmd2,
    ) {
        use vcod_cod2::pmove::weapon::{key, state};
        let step = self
            .driver
            .bots
            .get(&slot)
            .map_or(15.0, |b| b.skill.turn_deg);
        let turn = |from: f32, to: f32| {
            let d = (to - from + 180.0).rem_euclid(360.0) - 180.0;
            from + d.clamp(-step, step)
        };
        let look = [turn(view.view[0], t.aim[0]), turn(view.view[1], t.aim[1])];
        let off = |a: f32, b: f32| ((a - b + 180.0).rem_euclid(360.0) - 180.0).abs();
        let aimed =
            off(look[0], t.aim[0]) < SMOKE_AIMED_DEG && off(look[1], t.aim[1]) < SMOKE_AIMED_DEG;
        let from = (c2.angles[1] + view.delta_angles[1]) as f32 * 360.0 / 65536.0;
        (c2.forward_move, c2.right_move) =
            super::rekey(c2.forward_move, c2.right_move, from, look[1]);
        c2.angles = super::cmd_angles(view, look);
        c2.buttons &= !(button::ATTACK | button::ADS | key::FRAG);
        c2.off_hand_index = t.weapon;
        let hold = p.ws.state == state::OFFHAND_HOLD;
        t.ticks += 1;
        if hold && aimed {
            // The key comes up: the throw.
            self.smoking.remove(&slot);
            self.smokes += 1;
            return;
        }
        if t.ticks > SMOKE_TICKS {
            // Let go wherever the view is rather than hold the grenade.
            self.smoking.remove(&slot);
            return;
        }
        c2.buttons |= key::SMOKE;
        self.smoking.insert(slot, t);
    }

    /// The body in `slot` as the brain reads it.
    fn view(&self, level: &Cod2Level, slot: usize, enemy: Option<EnemyView>) -> Option<BotView> {
        let p = level.players.get(slot)?.as_ref()?;
        let (ps, ws) = (&p.ps, &p.ws);
        let defs = &level.combat.weapons;
        let def = |w: u8| defs.get(usize::from(w)).and_then(Option::as_ref);
        let held = def(ws.weapon);
        let holds = |w: u8| w < 64 && ws.held >> w & 1 == 1;
        let rounds = |w: u8| {
            def(w).map_or(0, |d| {
                let reserve = if d.clip_only {
                    0
                } else {
                    ws.ammo.get(d.ammo_index).copied().unwrap_or(0)
                };
                ws.clip.get(d.clip_index).copied().unwrap_or(0) + reserve
            })
        };
        let loaded = (1u8..64)
            .filter(|&w| holds(w) && rounds(w) > 0)
            .fold(0u64, |m, w| m | 1 << w);
        let pistol =
            (1u8..64).find(|&w| holds(w) && def(w).is_some_and(|d| d.weap_class == CLASS_PISTOL));
        let gun = (1u8..64).find(|&w| {
            loaded >> w & 1 == 1
                && def(w)
                    .is_some_and(|d| d.weap_type == TYPE_BULLET && d.weap_class != CLASS_PISTOL)
        });
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
            .and_then(def)
            .map_or(0, |p| held.map_or(0, |d| d.drop_time) + p.raise_time);
        // The offhand frag (`offhandClass` 1) with a grenade left.
        let grenade = (1u8..64).find(|&w| {
            holds(w) && rounds(w) > 0 && def(w).is_some_and(|d| d.offhand_class == OFFHAND_FRAG)
        });
        // The brain throws by switching to its frag and holding the
        // trigger; on CoD2 the frag key does both ([`offhand_cmd`]), so
        // the frag reads as in hand from the cmd that asked for it.
        let weapon = match grenade {
            Some(g) if self.throwing.contains_key(&slot) => g,
            _ => ws.weapon,
        };
        let vitals = level
            .rt
            .host
            .client_vitals
            .get(slot)
            .copied()
            .unwrap_or_default();
        let dead = vitals.dead || ps.pm_type >= pm_type::DEAD;
        let on_ladder = ps.pm_flags & pmf::LADDER != 0;
        let on_ground = ps.on_ground();
        let origin: [f32; 3] = ps.origin.into();
        let w = &level.world;
        let nav = self.driver.nav.as_ref();
        let enemy_eye =
            enemy.and_then(|e| Some(eye(&level.players.get(e.slot)?.as_ref()?.ps).into()));
        Some(BotView {
            origin,
            eye_height: ps.view_height_current,
            eye_settled: ps.view_height_current == ps.view_height_target as f32,
            lean_offset: vcod_common::pmove::aim::lean_offset(
                ps.view_angles[1].to_radians(),
                ps.leanf,
            )
            .into(),
            delta_angles: ps.delta_angles,
            view: ps.view_angles,
            gun_offset: self.gun_offset(slot, p),
            weapon,
            weapons_held: ws.held as u64,
            clip: held.map_or(-1, |d| {
                ws.clip.get(d.clip_index).copied().unwrap_or(0) as i16
            }),
            fire_time_ms: held.map_or(0, fire_cycle),
            busy_ms: ws.weapon_time,
            automatic: held.is_some_and(|d| !d.semi_auto && !d.bolt_action),
            has_ads: held.is_some_and(|d| d.aim_down_sight),
            sniper: held.is_some_and(|d| d.name.contains("sniper") || d.name.contains("scope")),
            lmg: false,
            deployed: false,
            deploy_yaw: 0.0,
            ads_frac: ws.pos_frac,
            spread_scale: ps.aim_spread_scale,
            speed: ps.velocity.truncate().length(),
            velocity: ps.velocity.into(),
            dead,
            playing: playing(level, slot) && !dead,
            enemy,
            hurt: None,
            friend_in_line: false,
            sidestep: None,
            cover: None,
            hunt: None,
            use_takes_weapon: false,
            grenade,
            grenade_cooks: grenade.and_then(def).is_some_and(|d| d.cook_off_hold),
            grenade_ready: ws.state == vcod_cod2::pmove::weapon::state::OFFHAND_HOLD,
            frag_near: frag_near(level, slot),
            post: None,
            give_way: None,
            guard_post: None,
            lane: None,
            stand_room: true,
            exposure: None,
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
            // `client_think` steps pmove at the held weapon's scale.
            move_scale: held
                .map(|d| d.move_speed_scale)
                .filter(|&m| m > 0.0)
                .unwrap_or(1.0),
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

    /// [`BotView::gun_offset`]: where the last cmd's shots left
    /// (`Cod2Player::fire_angles`, the aim block's sway, bob and gun
    /// spring over the view) off the view that cmd started from, the one
    /// the last tick read. The brain aims by it as by CoD1's reticle.
    fn gun_offset(&self, slot: usize, p: &crate::game::cod2::Cod2Player) -> [f32; 2] {
        let Some(v) = self.last_views.get(&slot) else {
            return [0.0; 2];
        };
        let d = |a: f32, b: f32| (a - b + 180.0).rem_euclid(360.0) - 180.0;
        let g = [d(p.fire_angles[0], v[0]), d(p.fire_angles[1], v[1])];
        // A view the cmd turned past what a gun swings (a respawn, a
        // setPlayerAngles) reads no offset.
        if g[0].abs() > 15.0 || g[1].abs() > 15.0 {
            return [0.0; 2];
        }
        g
    }
}

/// How far ahead of a node a wall flagged for the mantle is looked for
/// before the climb is walked: past half the graph's widest pitch.
const MANTLE_LOOK: f32 = 64.0;
/// The walk to the wall's ticks, and the whole probe's: the slowest
/// mantle (`mp_mantle_up_57` and `_over_high`) runs under 1.5 s.
const MANTLE_WALK_TICKS: usize = 20;
const MANTLE_TICKS: usize = 70;

/// The graph's mantle probe ([`crate::nav::MantleProbe`]) under the
/// level's CoD2 movement `rules`: [`mantle_walk`].
pub fn mantle_probe(rules: vcod_cod2::pmove::Rules) -> crate::nav::MantleProbe {
    Arc::new(
        move |world: &vcod_common::collision::CollisionWorld, from: Vec3, yaw: f32| {
            mantle_walk(world, rules, from, yaw)
        },
    )
}

/// Where a CoD2 body standing at `from` and run at the wall along `yaw`
/// stands once it mantles (cod2-movement.md 9): only off a wall flagged
/// `MANTLE_ON` or `MANTLE_OVER` within [`MANTLE_LOOK`]; the body walks at
/// it, presses the jump key on the first step pmove offers the climb
/// (`Mantle_CheckLedge`'s hint), as a bot does, and the climb's end on the
/// ground is the answer.
pub fn mantle_walk(
    world: &vcod_common::collision::CollisionWorld,
    rules: vcod_cod2::pmove::Rules,
    from: Vec3,
    yaw: f32,
) -> Option<Vec3> {
    use vcod_cod2::collision::{CONTENTS_MANTLE, SURF_MANTLE_ON, SURF_MANTLE_OVER};
    use vcod_cod2::pmove::mantle::flag;
    use vcod_common::movetrace::MoveWorld;
    if !rules.mantle_enable || rules.mantle_anims.is_none() {
        return None;
    }
    let (s, c) = yaw.to_radians().sin_cos();
    let dir = Vec3::new(c, s, 0.0);
    let r = rules.mantle_check_radius.max(1.0);
    let t = MoveWorld::bare(world).box_trace(
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
    let mut ps = vcod_cod2::pmove::PlayerState::spawn(from + Vec3::Z * 0.125, yaw, 0);
    let angle = (yaw * 65536.0 / 360.0) as i32 & 0xffff;
    let mut old = UserCmd2::default();
    let mut mantled = false;
    for tick in 0..MANTLE_TICKS {
        if !mantled && tick >= MANTLE_WALK_TICKS {
            return None;
        }
        let jump = !mantled && ps.mantle.flags & flag::HINT != 0;
        let cmd = UserCmd2 {
            server_time: ps.command_time + 50,
            buttons: if jump { button::JUMP } else { 0 },
            angles: [0, angle, 0],
            forward_move: if mantled { 0 } else { 127 },
            ..Default::default()
        };
        vcod_cod2::pmove::player_step(&mut ps, &cmd, &mut old, MoveWorld::bare(world), rules, 1.0);
        if ps.pm_flags & pmf::MANTLE != 0 {
            mantled = true;
        } else if mantled && ps.on_ground() {
            return Some(ps.origin);
        } else if !mantled && jump {
            // The jump key jumped instead.
            return None;
        }
    }
    None
}

/// The nearest `p` comes to the segment `a` to `b`.
fn segment_dist(a: Vec3, b: Vec3, p: Vec3) -> f32 {
    let ab = b - a;
    let t = ((p - a).dot(ab) / ab.length_squared().max(1e-6)).clamp(0.0, 1.0);
    (a + ab * t).distance(p)
}

/// Where a smoke grenade thrown from `from` at an enemy at `enemy` is
/// meant to land: on the line between, [`SMOKE_OUT`] or a third of the
/// way out, whichever is further, and [`SMOKE_SHORT`] short of him at
/// least.
fn smoke_point(from: [f32; 3], enemy: [f32; 3]) -> [f32; 3] {
    let (from, enemy) = (Vec3::from(from), Vec3::from(enemy));
    let d = from.distance(enemy);
    let out = (d / 3.0).max(SMOKE_OUT).min(d - SMOKE_SHORT).max(0.0);
    (from + (enemy - from).normalize_or_zero() * out).into()
}

/// The ms between a weapon's shots as the brain reads them
/// ([`BotView::fire_time_ms`]): CoD2 rechambers a bolt rifle after its
/// `fireTime` for `rechamberTime` (`PM_Weapon`'s state 3), so a kar98k's
/// cycle is 1330 ms, not its 330 ms `fireTime`. The shotgun pumps the same
/// way but stays a close gun: read as a bolt rifle the brain would back
/// away inside 300 units.
fn fire_cycle(d: &vcod_cod2::pmove::weapon::WeaponParams) -> i32 {
    if d.bolt_action && d.weap_class == CLASS_RIFLE {
        d.fire_time + d.rechamber_time
    } else {
        d.fire_time
    }
}

/// How long a bot holds its breath down a scope before it lets go: short
/// of `player_breath_hold_time`'s 4.5 s, whose gasp would swing the scope
/// at 4.5 times its sway.
const BREATH_HOLD_MS: i32 = 3000;

/// The breath as the playerstate holds it: `pm_flags` 0x4000 and
/// `holdBreathTimer`.
#[derive(Clone, Copy)]
struct Breath {
    held: bool,
    timer: i32,
}

/// Whether the bot's cmd holds its breath (`+holdbreath`, cod2-movement.md
/// 18) when `aiming`, fully down a scope (`adsOverlayReticle`,
/// `update_hold_breath`'s test) with the sight button down and an enemy in
/// sight: from a breath recovered (timer 0) until [`BREATH_HOLD_MS`].
/// vcod's own rule.
fn hold_breath(b: Breath, aiming: bool) -> bool {
    aiming
        && if b.held {
            b.timer < BREATH_HOLD_MS
        } else {
            b.timer == 0
        }
}

/// The weapon a bot answers `weapon_<menu>` with: of those the menu offers
/// and the cvars allow, one the fewest of `carried` (the other bots'
/// picks) are, drawn among the ties off `roll`.
fn spread_pick(
    menu: &str,
    carried: &[&str],
    allowed: &dyn Fn(&str) -> bool,
    roll: u64,
) -> Option<&'static str> {
    let open: Vec<&'static str> = super::menu_weapons(&WEAPON_MENUS_COD2, menu)?
        .filter(|(_, cvar)| allowed(cvar))
        .map(|(w, _)| *w)
        .collect();
    let count = |w: &str| carried.iter().filter(|c| **c == w).count();
    let least = open.iter().map(|w| count(w)).min()?;
    let ties: Vec<&'static str> = open.into_iter().filter(|w| count(w) == least).collect();
    // The bot's state may hold only its 31-bit seed: mixed, so the high
    // bits draw too.
    let r = roll.wrapping_mul(0x9e37_79b9_7f4a_7c15) >> 33;
    Some(ties[r as usize % ties.len()])
}

/// A client playing in the world: `pm_type` normal or linked.
fn playing(level: &Cod2Level, slot: usize) -> bool {
    level
        .players
        .get(slot)
        .and_then(Option::as_ref)
        .is_some_and(|p| matches!(p.ps.pm_type, pm_type::NORMAL | pm_type::NORMAL_LINKED))
}

/// The stance the last `EV_STANCE_FORCE_*` in `ps`'s ring past
/// `eventSequence` `from` sets: `Some(true)` crouch, `Some(false)` stand.
/// `EV_STANCE_FORCE_PRONE` is left to the brain.
fn forced_stance(ps: &vcod_cod2::pmove::PlayerState, from: i32) -> Option<bool> {
    use vcod_cod2::pmove::ev;
    let seq = ps.event_sequence;
    (from.max(seq - 4)..seq)
        .rev()
        .find_map(|i| match ps.events[(i & 3) as usize] {
            ev::STANCE_FORCE_CROUCH => Some(true),
            ev::STANCE_FORCE_STAND => Some(false),
            _ => None,
        })
}

fn eye(ps: &vcod_cod2::pmove::PlayerState) -> Vec3 {
    ps.origin + Vec3::Z * ps.view_height_current
}

/// The body in `slot` as the driver's team passes read it. A client
/// killed this frame keeps `pm_type` normal until its end frame; the
/// vitals already say dead, and the yields walk round no corpse.
pub(crate) fn body(level: &Cod2Level, slot: usize) -> Option<super::driver::Body> {
    let p = level.players.get(slot)?.as_ref()?;
    let dead = level
        .rt
        .host
        .client_vitals
        .get(slot)
        .is_some_and(|v| v.dead);
    Some(super::driver::Body {
        origin: p.ps.origin,
        velocity: p.ps.velocity,
        playing: playing(level, slot) && !dead,
        on_ground: p.ps.on_ground(),
        on_ladder: p.ps.pm_flags & pmf::LADDER != 0,
    })
}

/// What the cover search reads of the bot in `slot` and its `threat`:
/// CoD2's standing lean is 0.5, as 1.5's (`PM_UpdateLean`,
/// `vcod_cod2::pmove`).
pub(crate) fn cover_body(
    level: &Cod2Level,
    slot: usize,
    threat: usize,
) -> Option<super::driver::CoverBody> {
    let me = level.players.get(slot)?.as_ref()?;
    let threat = level.players.get(threat)?.as_ref()?;
    Some(super::driver::CoverBody {
        origin: me.ps.origin,
        threat_eye: eye(&threat.ps),
        lean: 0.5,
        mask: vcod_cod2::pmove::MASK_PLAYERSOLID,
    })
}

/// The body in `slot` as the follower reads it ([`cod1_body`]).
pub(crate) fn move_ps(level: &Cod2Level, slot: usize) -> Option<vcod_common::pmove::PlayerState> {
    let p = level.players.get(slot)?.as_ref()?;
    Some(cod1_body(&p.ps, level.level_time()))
}

/// The CoD1 body the follower's foot and jump tests read: where the CoD2
/// body stands and how it moves, its last jump's age off `jump_time`, and
/// the jump penalty `GRAPH_RULES` jumps under: CoD2's `JUMPING` with its
/// `pm_time` is 1.5's 0x2000 (cod2-movement.md), as [`Cod2Walk`] maps it.
fn cod1_body(ps: &vcod_cod2::pmove::PlayerState, now: i32) -> vcod_common::pmove::PlayerState {
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
    b
}

/// The nearest live frag within [`super::FLEE_REACH`] across and
/// [`super::FLEE_HEIGHT`] up or down, the bot's own or one it notices
/// ([`super::notices_frag`]), as the CoD1 driver fills
/// [`BotView::frag_near`].
fn frag_near(level: &Cod2Level, slot: usize) -> Option<[f32; 3]> {
    let p = level.players.get(slot)?.as_ref()?;
    let at = p.ps.origin;
    let eye = eye(&p.ps);
    let w = &level.world.collision;
    level
        .combat
        .missiles
        .iter()
        .filter(|m| !m.rocket && m.exploded_at.is_none())
        .filter(|m| {
            level
                .combat
                .weapons
                .get(usize::from(m.weapon))
                .and_then(Option::as_ref)
                .is_some_and(|d| d.explosion_radius > 0 && d.explosion_inner_damage > 0)
        })
        .filter(|m| {
            m.owner == Some(slot)
                || super::notices_frag(eye, p.ps.view_angles[1], m.origin, |a, b| {
                    w.shot_trace(a, b).fraction >= 1.0
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

/// [`BotView::lob`] for the bot in `slot` throwing frag `weapon` at
/// `target`: of the throws from 10 degrees down to 85 up, every 5, the
/// one whose frag comes down nearest `target`, within
/// [`super::LOB_MISS`], and not within its `explosionRadius` of the
/// thrower. The flight is `weapon_grenadelauncher_fire`'s
/// (`projectileSpeed` along the view plus `projectileSpeedUp` up, from the
/// eye) under `TR_GRAVITY`'s 800, up to where it first meets the world: a
/// bounce is not flown. With friendly fire on, none while a live teammate
/// stands within the radius of `target`.
pub(crate) fn lob_probe(
    level: &Cod2Level,
    slot: usize,
    weapon: u8,
    target: [f32; 3],
    teams: &[i32],
) -> Option<[f32; 2]> {
    const STEP_S: f32 = 0.05;
    let p = level.players.get(slot)?.as_ref()?;
    let def = level.combat.weapons.get(usize::from(weapon))?.as_ref()?;
    let radius = def.explosion_radius as f32;
    let target = Vec3::from(target);
    let mine = teams.get(slot).copied().unwrap_or(0);
    let teamplay = (mine == TEAM_AXIS || mine == TEAM_ALLIES)
        && crate::game::builtins::cvar::atoi(level.rt.cvars().get("scr_friendlyfire")) != 0;
    let mate_near = level.players.iter().enumerate().any(|(i, o)| {
        teamplay
            && i != slot
            && teams.get(i).copied() == Some(mine)
            && playing(level, i)
            && o.as_ref()
                .is_some_and(|o| o.ps.origin.distance(target) < radius)
    });
    if mate_near {
        return None;
    }
    let w = &level.world.collision;
    let from = eye(&p.ps);
    let to = target - from;
    let yaw = to.y.atan2(to.x).to_degrees();
    let fuse_s = def.fuse_time as f32 / 1000.0;
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
            let tr = w.shot_trace(at, next);
            if tr.fraction < 1.0 {
                landed = Some(at + (next - at) * tr.fraction);
                break;
            }
            at = next;
            t += STEP_S;
        }
        let burst = landed.unwrap_or(at);
        if burst.distance(p.ps.origin) <= radius {
            continue;
        }
        let miss = burst.distance(target);
        if miss < super::LOB_MISS && best.is_none_or(|(_, m)| miss < m) {
            best = Some(([-up, yaw], miss));
        }
    }
    best.map(|(aim, _)| aim)
}

/// The cmd the brain's asks for, in CoD2's bits, and the frag throw's
/// state to keep: the brain throws as on CoD1, switching to the frag and
/// holding the trigger until the release; CoD2's frag is offhand, its key
/// (0x10000) the drop, the pullback and the hold at once, and its release
/// the throw (`PM_Weapon`'s states 12 to 15). So while the brain asks for
/// the frag the weapon byte stays on the gun in hand, the key is held from
/// the ask until the brain has held the trigger and let it go, and the
/// trigger itself goes nowhere. `throwing` is the last cmd's state; the
/// returned one is `None` once the brain asks for the frag no more.
pub fn offhand_cmd(
    cmd: &UserCmd,
    grenade: Option<u8>,
    held: u8,
    throwing: Option<bool>,
) -> (UserCmd2, Option<bool>) {
    let mut c = to_cmd2(cmd);
    let Some(g) = grenade.filter(|&g| cmd.weapon == g) else {
        return (c, None);
    };
    c.weapon = held;
    let trigger = c.buttons & button::ATTACK != 0;
    c.buttons &= !(button::ATTACK | button::ADS);
    let armed = throwing.unwrap_or(false) || trigger;
    if trigger || !armed {
        c.buttons |= vcod_cod2::pmove::weapon::key::FRAG;
    }
    c.off_hand_index = g;
    (c, Some(armed))
}

/// A CoD1 usercmd in CoD2's bits (cod2-movement.md 3, the client's
/// `CL_CmdButtons` map): the attack, melee, use and sight move to 0x1,
/// 0x4, 0x8 and 0x1000; reload, the leans and the two stances leave
/// `wbuttons` for 0x10, 0x40/0x80 and 0x100/0x200; `upmove` is gone, so a
/// jump is 0x400 and a crouch held through `up` is 0x200.
pub fn to_cmd2(cmd: &UserCmd) -> UserCmd2 {
    let mut b = 0;
    let map = [
        (cmd.buttons & msg::BUTTON_ATTACK, button::ATTACK),
        (cmd.buttons & msg::BUTTON_MELEE, 0x4),
        (cmd.buttons & msg::BUTTON_USE, 0x8),
        (cmd.buttons & msg::BUTTON_ADS, button::ADS),
        (cmd.wbuttons & msg::WBUTTON_RELOAD, 0x10),
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
    UserCmd2 {
        server_time: cmd.server_time,
        buttons: b,
        weapon: cmd.weapon,
        off_hand_index: 0,
        angles: cmd.angles,
        forward_move: cmd.forward,
        right_move: cmd.right,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The last forced stance in the ring past the mark wins; events at or
    /// before it, or rolled out of the four slots, are not read again.
    #[test]
    fn a_forced_stance_reads_the_ring_past_the_mark() {
        use vcod_cod2::pmove::ev;
        let mut ps = vcod_cod2::pmove::PlayerState::spawn(Vec3::ZERO, 0.0, 0);
        let push = |ps: &mut vcod_cod2::pmove::PlayerState, e: i32| {
            ps.events[(ps.event_sequence & 3) as usize] = e;
            ps.event_sequence += 1;
        };
        assert_eq!(forced_stance(&ps, 0), None);
        push(&mut ps, ev::STANCE_FORCE_CROUCH);
        push(&mut ps, 1);
        assert_eq!(forced_stance(&ps, 0), Some(true));
        assert_eq!(forced_stance(&ps, 1), None, "read last tick");
        push(&mut ps, ev::STANCE_FORCE_STAND);
        assert_eq!(forced_stance(&ps, 0), Some(false));
        for _ in 0..4 {
            push(&mut ps, 1);
        }
        assert_eq!(forced_stance(&ps, 0), None, "rolled out of the ring");
    }

    /// Four shooting bots on mp_toujane `dm` for a minute or two: every one
    /// spawns through the stock menus, runs the graph, and somebody kills
    /// somebody else. Needs `COD2_DIR`; without it the test passes.
    #[test]
    fn cod2_bots_join_roam_and_fight_on_mp_toujane() {
        let Some(fs) = vcod_common::testing::cod2_fs() else {
            return;
        };
        let now = std::time::Instant::now();
        let mut sv = crate::cod2::Cod2Server::new(
            crate::cod2::Cod2Config {
                map: "mp_toujane".into(),
                gametype: "dm".into(),
                hostname: "bots".into(),
                max_clients: 8,
                gametype_script: None,
            },
            3,
            now,
        );
        sv.load(Some(std::rc::Rc::new(fs))).unwrap();
        let slots = sv.add_bots(4, true);
        assert_eq!(slots.len(), 4);
        sv.bots_mut().unwrap().wait_for_graph();
        let mut mantles = 0;
        let mut mantling = [false; 4];
        let mut walked = [0.0f32; 4];
        let mut last: [Option<Vec3>; 4] = [None; 4];
        let mut hits_felt = 0;
        // A minute, or two if nobody has mantled by then: which routes cross
        // a mantle edge rides the weapons drawn.
        for tick in 1..=2400u64 {
            if tick > 1200 && mantles > 0 {
                break;
            }
            sv.tick_frames(now + std::time::Duration::from_millis(50 * tick), 1);
            // A hit felt names another bot and a direction.
            let bots = sv.bots_mut().unwrap();
            for (slot, (_, hit)) in &bots.hits {
                if let Some((a, at, dir)) = hit
                    && *at == bots.last_tick
                {
                    assert_ne!(a, slot);
                    assert!(slots.contains(a));
                    assert!((Vec3::from(*dir).length() - 1.0).abs() < 1e-3, "{dir:?}");
                    hits_felt += 1;
                }
            }
            let level = sv.level().unwrap();
            for (i, &slot) in slots.iter().enumerate() {
                let p = level.players[slot].as_ref().unwrap();
                if !level.playing(slot) {
                    last[i] = None;
                    continue;
                }
                if let Some(was) = last[i] {
                    walked[i] += (p.ps.origin - was).truncate().length().min(50.0);
                }
                let m = p.ps.pm_flags & pmf::MANTLE != 0;
                if m && !mantling[i] {
                    mantles += 1;
                }
                mantling[i] = m;
                last[i] = Some(p.ps.origin);
            }
        }
        let level = sv.level().unwrap();
        let aborts: Vec<_> = level
            .rt
            .aborts()
            .into_iter()
            .filter(|e| !e.contains("record cap"))
            .collect();
        assert!(aborts.is_empty(), "{aborts:?}");
        assert!(walked.iter().all(|&d| d > 1000.0), "walked {walked:?}");
        let kills = level
            .rt
            .script_log()
            .iter()
            .filter(|l| {
                let f: Vec<&str> = l.split(';').collect();
                f[0] == "K" && f.len() >= 13 && f[6] != "-1" && f[6] != f[2]
            })
            .count();
        assert!(kills > 0, "no kills in a minute");
        assert!(hits_felt > 0, "no hit felt");
        // The graph's mantle edges, and bots that climb them.
        let g = sv.bots_mut().unwrap().graph().unwrap().clone();
        assert!(g.mantles.len() > 100, "{} mantle edges", g.mantles.len());
        assert!(
            mantles > 0,
            "no bot mantled, walked {walked:?} kills {kills}"
        );
    }

    /// Four shooting bots on mp_toujane `tdm` until one yells: its `mr` on
    /// a quick-message menu runs `_quickmessages.gsc`'s `doQuickMessage`,
    /// the one stock script that sets the `talkingicon` head icon
    /// (`bots::yell`). Needs `COD2_DIR`; without it the test passes.
    #[test]
    fn a_fighting_cod2_bot_yells_a_stock_quick_message() {
        let Some(fs) = vcod_common::testing::cod2_fs() else {
            return;
        };
        let now = std::time::Instant::now();
        let mut sv = crate::cod2::Cod2Server::new(
            crate::cod2::Cod2Config {
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
        let slots = sv.add_bots(4, true);
        sv.bots_mut().unwrap().wait_for_graph();
        // A yell is a roll (`bots::yell`); five minutes give a few dozen.
        for tick in 1..=6000u64 {
            sv.tick_frames(now + std::time::Duration::from_millis(50 * tick), 1);
            let level = sv.level_mut().unwrap();
            if slots
                .iter()
                .any(|&s| level.rt.client_field(s, "headicon").as_deref() == Some("talkingicon"))
            {
                return;
            }
        }
        panic!("five minutes of tdm on mp_toujane and no bot yelled");
    }

    /// mp_decoy's pit at (8650, -12570, -530) lies 35 to 55 under the
    /// ledges round it. Its way out were leaps, which a CoD2 bot never
    /// jumps (no lip probe, `BotWorld::PMOVE_PROBES` off); built without
    /// the lip ([`LIP`]) the graph keeps no leap and the pit a plain jump
    /// up out of it. Needs `COD2_DIR` (bot-objectives.md 11.8).
    #[test]
    fn a_cod2_graph_keeps_no_leap_and_a_jump_out_of_decoys_pit() {
        let Some(fs) = vcod_common::testing::cod2_fs() else {
            return;
        };
        let now = std::time::Instant::now();
        let mut sv = crate::cod2::Cod2Server::new(
            crate::cod2::Cod2Config {
                map: "mp_decoy".into(),
                gametype: "tdm".into(),
                hostname: "bots".into(),
                max_clients: 8,
                gametype_script: None,
            },
            3,
            now,
        );
        sv.load(Some(std::rc::Rc::new(fs))).unwrap();
        sv.add_bots(1, false);
        sv.bots_mut().unwrap().wait_for_graph();
        sv.tick_frames(now + std::time::Duration::from_millis(50), 1);
        let g = sv.bots_mut().unwrap().graph().unwrap().clone();
        assert!(g.leaps.is_empty(), "{} leaps", g.leaps.len());
        let pit = |p: Vec3| (p.x - 8650.0).hypot(p.y + 12570.0) < 100.0 && p.z < -505.0;
        let out = g
            .jumps
            .iter()
            .any(|&(a, b)| pit(g.nodes[a as usize]) && g.nodes[b as usize].z > -500.0);
        assert!(out, "no jump out of the pit");
    }

    /// mp_trainstation's first `hq` radio sits at (7680.83, -3602.28, 18)
    /// (`hq.gsc`), 50 over the floor beside it: the graph's nearest node
    /// to it is a platform out of the capture's reach, and a bot sent
    /// there paced short of the radio for whole rounds. Its stand is a
    /// node the capture counts (within `level.radioradius` 120, in three
    /// dimensions, and `level.zradioradius` 72 up or down). Needs
    /// `COD2_DIR` (bot-objectives.md 11.6).
    #[test]
    fn a_cod2_radio_stand_is_inside_the_capture() {
        use crate::bots::{RADIO_HEIGHT_COD2, RADIO_MARGIN};
        let Some(fs) = vcod_common::testing::cod2_fs() else {
            return;
        };
        let now = std::time::Instant::now();
        let mut sv = crate::cod2::Cod2Server::new(
            crate::cod2::Cod2Config {
                map: "mp_trainstation".into(),
                gametype: "hq".into(),
                hostname: "bots".into(),
                max_clients: 8,
                gametype_script: None,
            },
            3,
            now,
        );
        sv.load(Some(std::rc::Rc::new(fs))).unwrap();
        sv.add_bots(1, false);
        sv.bots_mut().unwrap().wait_for_graph();
        sv.tick_frames(now + std::time::Duration::from_millis(50), 1);
        let bots = sv.bots_mut().unwrap();
        let g = bots.graph().expect("the graph").clone();
        let at = Vec3::new(7680.83, -3602.28, 18.0);
        let counts = |p: Vec3| {
            (p - at).truncate().length() < 120.0 - RADIO_MARGIN
                && (p.z - at.z).abs() < RADIO_HEIGHT_COD2 - RADIO_MARGIN
        };
        let near = g.nodes[g.nearest(at.to_array()).unwrap() as usize];
        assert!(!counts(near), "nearest node {near:?}");
        let stand = Vec3::from(bots.driver.radio_stand(
            at.to_array(),
            120.0 - 2.0 * RADIO_MARGIN,
            RADIO_HEIGHT_COD2 - RADIO_MARGIN,
        ));
        assert!(counts(stand), "stand {stand:?}");
        assert!(stand.distance(at) <= 120.0, "stand {stand:?}");
    }

    /// Two bots that do not shoot on mp_toujane `sd`: the attacker plants
    /// in a bombzone (`trigger_use_touch`, held through the script's
    /// `linkTo`) and the defender defuses. Needs `COD2_DIR`.
    #[test]
    fn cod2_bots_plant_and_defuse_on_mp_toujane() {
        let Some(fs) = vcod_common::testing::cod2_fs() else {
            return;
        };
        let now = std::time::Instant::now();
        let mut sv = crate::cod2::Cod2Server::new(
            crate::cod2::Cod2Config {
                map: "mp_toujane".into(),
                gametype: "sd".into(),
                hostname: "bots".into(),
                max_clients: 8,
                gametype_script: None,
            },
            3,
            now,
        );
        sv.load(Some(std::rc::Rc::new(fs))).unwrap();
        sv.add_bots(2, false);
        sv.bots_mut().unwrap().wait_for_graph();
        let actions = |sv: &crate::cod2::Cod2Server| -> Vec<String> {
            let log = sv.level().unwrap().rt.script_log();
            log.iter()
                .filter_map(|l| {
                    let f: Vec<&str> = l.trim_end().split(';').collect();
                    (f[0] == "A" && f.len() >= 6).then(|| f[5].to_string())
                })
                .collect()
        };
        let mut seen = Vec::new();
        for tick in 1..=2400u64 {
            let at = now + std::time::Duration::from_millis(50 * tick);
            // The round's `map_restart` runs off the console.
            sv.drain_console(at);
            sv.tick_frames(at, 1);
            seen.extend(actions(&sv));
            if seen.iter().any(|a| a == "bomb_defuse") {
                break;
            }
        }
        assert!(seen.iter().any(|a| a == "bomb_plant"), "{seen:?}");
        assert!(seen.iter().any(|a| a == "bomb_defuse"), "{seen:?}");
    }

    /// The brain's frag throw on CoD2's offhand key: the key goes down with
    /// the ask, stays down through the brain's held trigger and comes up
    /// with its release (the throw); the weapon byte stays on the gun.
    #[test]
    fn a_frag_throw_holds_the_offhand_key_until_the_release() {
        let ask = |buttons: u8| UserCmd {
            weapon: 9,
            buttons,
            ..msg::NULL_USERCMD
        };
        let frag = vcod_cod2::pmove::weapon::key::FRAG;
        let (c, t) = offhand_cmd(&ask(0), Some(9), 3, None);
        assert_eq!((c.weapon, c.off_hand_index, c.buttons), (3, 9, frag));
        assert_eq!(t, Some(false));
        let (c, t) = offhand_cmd(&ask(msg::BUTTON_ATTACK), Some(9), 3, t);
        assert_eq!((c.buttons, t), (frag, Some(true)));
        let (c, t) = offhand_cmd(&ask(0), Some(9), 3, t);
        assert_eq!((c.buttons, t), (0, Some(true)));
        let rifle = UserCmd {
            weapon: 3,
            buttons: msg::BUTTON_ATTACK,
            ..msg::NULL_USERCMD
        };
        let (c, t) = offhand_cmd(&rifle, Some(9), 3, t);
        assert_eq!((c.weapon, c.buttons, t), (3, button::ATTACK, None));
    }

    /// The cloud lands on the line to the enemy, and a line through it
    /// is blocked only while it is thick.
    #[test]
    fn a_smoke_cloud_lands_between_and_blocks_for_its_life() {
        let at = smoke_point([0.0; 3], [900.0, 0.0, 0.0]);
        assert_eq!(at, [300.0, 0.0, 0.0]);
        assert_eq!(smoke_point([0.0; 3], [400.0, 0.0, 0.0]), [250.0, 0.0, 0.0]);
        let mut b = Cod2Bots::new(true);
        b.clouds.push((Vec3::from(at), 1000));
        let (eye, far) = (Vec3::new(0.0, 0.0, 60.0), Vec3::new(900.0, 0.0, 60.0));
        assert!(!b.smoked(eye, far, 1000 + SMOKE_FORM_MS - 50));
        assert!(b.smoked(eye, far, 1000 + SMOKE_FORM_MS));
        assert!(!b.smoked(eye, Vec3::new(0.0, 900.0, 60.0), 1000 + SMOKE_FORM_MS));
        assert!(!b.smoked(eye, far, 1000 + SMOKE_GONE_MS));
    }

    /// A weapon menu spreads the bots over its weapons: none twice while
    /// one is left untaken, and never a restricted one.
    #[test]
    fn weapon_picks_spread_over_the_menu() {
        let all = |_: &str| true;
        let mut carried: Vec<&str> = Vec::new();
        for i in 0..6 {
            let w = spread_pick("german", &carried, &all, i).unwrap();
            assert!(!carried.contains(&w), "{w} twice in {carried:?}");
            carried.push(w);
        }
        let w = spread_pick("german", &carried, &all, 7).unwrap();
        assert!(carried.contains(&w));
        let no_mp40 = |c: &str| c != "scr_allow_mp40";
        for r in 0..20 {
            assert_ne!(spread_pick("german", &[], &no_mp40, r), Some("mp40_mp"));
        }
        // Small seeds still draw over the whole menu.
        let firsts: std::collections::BTreeSet<_> = (1..200)
            .filter_map(|r| spread_pick("german", &[], &all, r))
            .collect();
        assert_eq!(firsts.len(), 6, "{firsts:?}");
    }

    /// A bolt rifle's shot cycle counts its rechamber; a semi-automatic's
    /// is its `fireTime`.
    #[test]
    fn a_bolt_rifles_cycle_counts_the_rechamber() {
        use vcod_cod2::pmove::weapon::WeaponParams;
        let kar = WeaponParams {
            fire_time: 330,
            rechamber_time: 1000,
            bolt_action: true,
            ..Default::default()
        };
        let shotgun = WeaponParams {
            weap_class: 3,
            ..kar.clone()
        };
        let garand = WeaponParams {
            fire_time: 135,
            rechamber_time: 100,
            ..Default::default()
        };
        assert_eq!(fire_cycle(&kar), 1330);
        assert_eq!(fire_cycle(&garand), 135);
        assert_eq!(fire_cycle(&shotgun), 330);
    }

    /// The breath is taken only from a recovered breath, held under the
    /// gasp, and let go off the scope.
    #[test]
    fn a_scoped_bot_holds_its_breath_short_of_the_gasp() {
        let b = |held, timer| Breath { held, timer };
        assert!(hold_breath(b(false, 0), true));
        assert!(!hold_breath(b(false, 0), false));
        assert!(!hold_breath(b(false, 200), true));
        assert!(hold_breath(b(true, BREATH_HOLD_MS - 50), true));
        assert!(!hold_breath(b(true, BREATH_HOLD_MS), true));
    }

    #[test]
    fn a_cod1_cmd_lands_on_cod2_bits() {
        let cmd = UserCmd {
            server_time: 50,
            buttons: msg::BUTTON_ATTACK | msg::BUTTON_ADS,
            wbuttons: msg::WBUTTON_RELOAD | msg::WBUTTON_LEAN_LEFT,
            weapon: 7,
            forward: 127,
            right: -127,
            up: 127,
            ..msg::NULL_USERCMD
        };
        let c = to_cmd2(&cmd);
        assert_eq!(
            c.buttons,
            button::ATTACK | button::ADS | 0x10 | button::LEAN_LEFT | button::JUMP
        );
        assert_eq!((c.weapon, c.forward_move, c.right_move), (7, 127, -127));
        let crouch = to_cmd2(&UserCmd {
            up: -127,
            ..msg::NULL_USERCMD
        });
        assert_eq!(crouch.buttons, button::CROUCH);
        let prone = to_cmd2(&UserCmd {
            up: -127,
            wbuttons: msg::WBUTTON_PRONE,
            ..msg::NULL_USERCMD
        });
        assert_eq!(prone.buttons, button::PRONE);
    }
}
