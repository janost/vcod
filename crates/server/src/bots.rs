//! Server-side bots: synthetic clients in `Server::clients` whose brains
//! live here. A bot joins through the stock menus like any client (the
//! driver in `server.rs` feeds this module the reliable server commands and
//! sends back the `mr` replies), then sends real usercmds through pmove.
//!
//! The brain is pure: [`Bot::observe`] consumes server commands and returns
//! `mr` replies, [`Bot::think`] returns the tick's usercmd from a read-only
//! [`BotView`] the server fills. Everything wire- and VM-shaped stays in
//! `server.rs`.

use crate::server::FRAME_MS;
use vcod_common::net::msg::{self, NULL_USERCMD, UserCmd};

/// Hip-shot range; beyond it a tap is noise anyway.
pub(crate) const SHOOT_RANGE: f32 = 1500.0;
/// A frag goes at a target this close and no further.
const GRENADE_RANGE: f32 = 600.0;
/// Ticks a dead bot waits before its first use press.
const RESPAWN_DELAY: u32 = 30;
/// Ticks between use-press retries, and the press length: two ticks down,
/// then 1 s of release, repeated until the script's poll catches one.
const RESPAWN_RETRY: u32 = 20;
/// Half the width of what a shot has to land in (chest or head), units; the
/// fire cone is the angle it subtends at the target's range.
const HIT_RADIUS: f32 = 10.0;
/// A bot moving faster than this aims with [`Skill::error_moving`].
const MOVING_SPEED: f32 = 20.0;
/// How far a bot hears gunfire and a blast, units. The stock aliases carry
/// `dist_max` 7800 for every rifle and gun `_fire` and 6000 for
/// `grenade_explode_*` (`soundaliases/iw_sound.csv`), which covers most of a
/// map; these keep the same ratio at a range that leaves bots roaming.
pub(crate) const HEAR_GUNFIRE: f32 = 2000.0;
pub(crate) const HEAR_BLAST: f32 = 1500.0;
/// A remembered or heard spot is reached within this, horizontally.
const ARRIVE_RADIUS: f32 = 64.0;
/// ...and this vertically, so a spot one floor up is not reached from below.
const ARRIVE_HEIGHT: f32 = 96.0;
/// A lost enemy is remembered this far along its last velocity, seconds.
const MEMORY_LEAD_S: f32 = 0.5;

/// How well a bot fights. Ticks are 50 ms; the aim error is a miss distance
/// at the target, in units, so its angle shrinks with range on its own.
#[derive(Clone, Copy, Debug)]
pub struct Skill {
    /// Ticks from first sight of a target to the first turn toward it:
    /// `reaction_min + rand(reaction_spread)`.
    pub reaction_min: u32,
    pub reaction_spread: u32,
    /// Ticks out of sight after which a returning target counts as new.
    pub forget_ticks: u32,
    /// The most the view turns in one tick, degrees.
    pub turn_deg: f32,
    /// Aim error on acquisition; each tick on the target multiplies it by
    /// `error_decay`, down to `error_floor`.
    pub error_start: f32,
    pub error_floor: f32,
    pub error_decay: f32,
    /// The range at which the error doubles.
    pub error_range: f32,
    /// Error multiplier while the bot itself moves.
    pub error_moving: f32,
    /// Past this range a weapon with sights aims down them.
    pub ads_range: f32,
    /// Ticks a lost enemy's last spot stays a goal. Longer than
    /// `forget_ticks`: the spot outlives the target's identity.
    pub memory_ticks: u32,
    /// Ticks a heard noise stays a goal; long enough to walk most of
    /// [`HEAR_GUNFIRE`] at run speed.
    pub noise_ticks: u32,
}

impl Default for Skill {
    fn default() -> Self {
        Skill {
            reaction_min: 5,
            reaction_spread: 6,
            forget_ticks: 20,
            turn_deg: 15.0,
            error_start: 48.0,
            error_floor: 4.0,
            error_decay: 0.88,
            error_range: 1000.0,
            error_moving: 1.75,
            ads_range: 400.0,
            memory_ticks: 100,
            noise_ticks: 200,
        }
    }
}

/// What the brain needs to know about its own body this tick. The server
/// fills it from the sim, the weapon table and the script's team table.
pub struct BotView {
    pub origin: [f32; 3],
    /// `ps.delta_angles`, so the bot's absolute aim lands right after a spawn.
    pub delta_angles: [i32; 3],
    /// `ps.viewangles`, degrees, wire convention (pitch positive down).
    pub view: [f32; 3],
    /// `ps.weapon`, a 1-based index into configstring 7.
    pub weapon: u8,
    pub weapons_held: u64,
    /// The held weapon's loaded magazine; -1 when the weapon file is unknown.
    pub clip: i16,
    /// The held weapon's `fireTime`, in ms; 0 when unknown.
    pub fire_time_ms: i32,
    /// `ps.weaponTime`: the machine is busy and a tap would be latched away.
    pub busy_ms: i32,
    /// The held weapon fires while the trigger is held (`semiAuto 0`).
    pub automatic: bool,
    /// The held weapon has sights (`aimDownSight`).
    pub has_ads: bool,
    /// The held weapon's sights are a scope (`adsOverlayShader`).
    pub sniper: bool,
    /// `ps.fWeaponPosFrac`: 1 once the sights are fully up.
    pub ads_frac: f32,
    /// Horizontal speed, units/s.
    pub speed: f32,
    /// `ps.velocity`, units/s.
    pub velocity: [f32; 3],
    pub dead: bool,
    /// `pm_type` 0, a spawned player rather than a spectator or camera.
    pub playing: bool,
    /// Nearest live enemy with a clear sightline, as the server traced it.
    pub enemy: Option<EnemyView>,
    /// A held grenade's configstring index, when one is in the kit.
    pub grenade: Option<u8>,
    /// The next point on the server's path toward [`Bot::goal`]; `None`
    /// while there is no path, and the bot wanders.
    pub waypoint: Option<[f32; 3]>,
    /// Per compass octant (0 is +x, counter-clockwise by 45 degrees), a
    /// minefield or `trigger_hurt` [`HAZARD_LOOK`] units that way. A wander
    /// heading keeps out of them; the graph already does.
    pub hazard_ahead: [bool; 8],
    /// Per octant as [`Self::hazard_ahead`], the depth of a drop deeper
    /// than a jump within [`HAZARD_LOOK`] that way, before a wall
    /// (`nav::drop_ahead`); all clear off the ground. A wander heading keeps
    /// off them too, and off one that hurts even when cornered.
    pub drop_ahead: [Option<f32>; 8],
    /// The loudest gunfire or blast another player made last tick within
    /// earshot ([`loudest`]), chest high.
    pub noise: Option<[f32; 3]>,
    /// `linkTo` holds the body (a plant or defuse in progress); it cannot
    /// move, so it is not stuck.
    pub linked: bool,
    /// `ps.on_ladder`: the climb looks up, as the graph's walks did.
    pub on_ladder: bool,
    /// `ps.ladder_normal`: a climb faces into it (`nav::climb_yaw`).
    pub ladder_normal: [f32; 3],
    /// On a ladder, the middle of the rungs level with the body
    /// (`nav::NavGraph::ladder_middle`): a climb strafes toward it.
    pub ladder_middle: Option<[f32; 2]>,
    /// `ps.on_ground`.
    pub on_ground: bool,
    /// The way to [`Self::waypoint`] is a jump edge: jump where the body
    /// stands pinned or at a lip, as the graph's walk did (`nav::jump_cue`).
    pub jump: bool,
    /// The jump edge is a leap, taken from rest with a jump at the lip.
    pub leap: bool,
    /// On a jump edge, the body loses the ground next tick (`nav::lip_ahead`).
    pub lip: bool,
    /// [`Self::waypoint`] is a leap's foot: come to rest on it.
    pub stop: bool,
    /// A held pistol's configstring index, when one is in the kit.
    pub pistol: Option<u8>,
    /// Ms from a dry reload's start to the held weapon's next shot; 0 when
    /// unknown.
    pub reload_ms: i32,
    /// Ms from putting the held weapon away to the pistol's first shot
    /// (`dropTime` plus the pistol's `raiseTime`); 0 with no pistol.
    pub draw_ms: i32,
    /// The weapons held with a round left, clip or reserve, as a
    /// `weapons_held` mask.
    pub loaded: u64,
    /// The S&D objectives, on an `sd` level only.
    pub sd: Option<SdView>,
    /// The retrieval objectives, on an `re` level only.
    pub re: Option<ReView>,
    /// The bot's side of Behind Enemy Lines, on a `bel` level only.
    pub bel: Option<BelView>,
}

/// Behind Enemy Lines as the bot plays it (docs/research/bot-objectives.md,
/// "Behind Enemy Lines"): a few allies score by staying alive, the axis
/// hunt them, and an axis player who kills one takes his place.
#[derive(Clone, Debug)]
pub struct BelView {
    /// The bot is allied: the hunted side.
    pub hunted: bool,
    /// The compass markers its team sees. On the axis side, one per allied
    /// player, a lagging mean of where he stood (`bel.gsc`
    /// `make_obj_marker`); the allies see none.
    pub markers: Vec<[f32; 3]>,
    /// The bot's own marker, which the hunters walk to: a hunted bot
    /// knows the rule if not the spot on its compass.
    pub trail: Option<[f32; 3]>,
}

/// The stock `re.gsc` objectives as the server read them this frame
/// (docs/research/bot-objectives.md, "Retrieval").
#[derive(Clone, Debug)]
pub struct ReView {
    pub role: ObjRole,
    /// Each objective still standing.
    pub objectives: Vec<ReObjView>,
    /// As [`SdView::rank`]: defenders spread over the objectives by it.
    pub rank: usize,
}

#[derive(Clone, Copy, Debug)]
pub struct ReObjView {
    /// The middle of its pickup trigger while it lies there to be taken.
    pub pickup: Option<[f32; 3]>,
    /// A feet origin inside its goal trigger the navigation graph reaches.
    pub goal: [f32; 3],
    /// How far from `goal` a body is clear of the goal trigger. Anyone
    /// standing in it takes the trigger's fire (`trigger_multiple`'s wait),
    /// lower slots first, so a guard inside it keeps the carrier from ever
    /// delivering.
    pub goal_clear: f32,
    /// Someone carries it; `mine` when that is this bot.
    pub carried: bool,
    pub mine: bool,
    /// The carrier's feet, while the bot's team's compass follows him: the
    /// attackers always, the defenders only under `scr_re_showcarrier`.
    pub carrier_at: Option<[f32; 3]>,
    /// Where its compass marker last lay; a carry leaves it there, so it
    /// is where the carrier set out from.
    pub laid: Option<[f32; 3]>,
}

/// Which side of the S&D objective the bot's team plays this map.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ObjRole {
    Attack,
    Defend,
}

/// The stock `sd.gsc` objectives as the server read them this frame
/// (docs/research/bot-objectives.md).
#[derive(Clone, Debug)]
pub struct SdView {
    pub role: ObjRole,
    /// The bombzones still standing, A before B; empty once a bomb is down.
    pub sites: Vec<SiteView>,
    /// The planted bomb, until it is defused or explodes.
    pub bomb: Option<BombView>,
    /// The bot's place among its team's bots, by slot: sites are dealt out
    /// by it, so a team spreads over both.
    pub rank: usize,
    /// The bot is its team's bot nearest the bomb: the one defender that
    /// goes for the defuse while the rest keep off its line of sight.
    pub lead: bool,
}

#[derive(Clone, Copy, Debug)]
pub struct SiteView {
    /// The zone's absolute bounds.
    pub mins: [f32; 3],
    pub maxs: [f32; 3],
    /// A feet origin inside the bounds the navigation graph reaches.
    pub stand: [f32; 3],
}

#[derive(Clone, Copy, Debug)]
pub struct BombView {
    /// The defuse trigger's origin, which the script's 64-unit range reads.
    pub origin: [f32; 3],
    /// The point the view has to rest on for `isLookingAt`: the middle of
    /// the trigger's bounds.
    pub aim: [f32; 3],
}

/// Where a bot wants to go. The brain names it from its view; the server
/// plans the way there over the map's navigation graph (`crate::nav`) and
/// hands back the next [`BotView::waypoint`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Goal {
    /// Nowhere: dead, spectating or mid-throw.
    Hold,
    /// Anywhere far: the server picks a spot and keeps it until reached.
    Roam,
    /// A point, feet or chest high.
    To([f32; 3]),
    /// Anywhere far from a threat: the server picks a spot away from it and
    /// keeps it until reached or the threat moves.
    Away([f32; 3]),
}

#[derive(Clone, Copy)]
pub struct EnemyView {
    /// The enemy's client slot, so a switch of target reads as one.
    pub slot: usize,
    /// The chest, the point the bot aims at.
    pub origin: [f32; 3],
    /// Units/s; where it was heading is where a lost enemy is looked for.
    pub velocity: [f32; 3],
}

/// A shot or blast one tick of the sim made, as the server recorded it.
#[derive(Clone, Copy, Debug)]
pub struct Noise {
    pub at: [f32; 3],
    /// The client whose shot or grenade it was; it does not hear itself.
    pub source: usize,
    /// How far it carries: [`HEAR_GUNFIRE`] or [`HEAR_BLAST`].
    pub radius: f32,
}

/// The noise `listener` hears loudest from `at`: the one nearest relative
/// to its range, first in recorded order on a tie. Teammates' fights count
/// too: a friend shooting means an enemy close to him.
pub(crate) fn loudest(noises: &[Noise], listener: usize, at: [f32; 3]) -> Option<[f32; 3]> {
    noises
        .iter()
        .filter(|n| n.source != listener)
        .map(|n| (dist_sq(at, n.at).sqrt() / n.radius, n.at))
        .filter(|(f, _)| *f < 1.0)
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, p)| p)
}

/// What a bot's body is doing, as the gates read it.
pub struct BotBody {
    pub origin: [f32; 3],
    pub on_ladder: bool,
    pub on_ground: bool,
    pub playing: bool,
    pub dead: bool,
    pub health: i32,
    pub clip: i16,
    /// On the random heading a stuck bot takes, waypoint or not.
    pub unsticking: bool,
}

pub struct Bot {
    pub name: String,
    shoot: bool,
    pub skill: Skill,
    /// The team the team menu is answered with.
    team: String,
    /// The menu retail last named in `v g_scriptMainMenu`; the `t` that
    /// follows opens it.
    main_menu: String,
    /// Menu indices already answered, so a reopened menu is not answered
    /// twice (the same bookkeeping `JoinProbe` keeps).
    answered: Vec<i32>,
    /// The tick's usercmd is stamped with the server's own clock; the driver
    /// stamps it, so the brain carries only the input state below.
    rng: u64,
    /// Wander heading, degrees, wire convention.
    heading: f32,
    /// Ticks left on the current heading.
    heading_ticks: u32,
    /// Where the bot was when it picked its heading; a heading that barely
    /// moves the origin is swapped for a new one.
    stall_origin: [f32; 3],
    stall_ticks: u32,
    /// Ticks left on a wander heading taken to get unstuck, waypoint or not.
    unstick_ticks: u32,
    /// Where the last unstick spell started, and how many in a row started
    /// within [`CORNER`] of the one before.
    unstick_at: [f32; 3],
    unstick_tries: u32,
    /// Ticks until the trigger may go down again: a semi-auto's tap spacing,
    /// an automatic's pause between bursts.
    fire_cooldown: u32,
    /// Ticks an automatic's trigger stays held.
    burst_ticks: u32,
    target: Option<Target>,
    /// Where to go looking once nothing is in sight.
    recall: Option<Recall>,
    /// Engagement footwork: the strafe direction (`right`), the ticks left
    /// on it, and the ticks left on a crouch.
    strafe: i8,
    strafe_ticks: u32,
    crouch_ticks: u32,
    grenade_cooldown: u32,
    respawn_ticks: u32,
    stage: Stage,
    /// The weapon a grenade throw hands back to.
    rifle: u8,
    /// A weapon switch under way: the byte every cmd carries until
    /// `ps.weapon` reads it (AGENTS.md, "Every cmd carries the weapon").
    switch_to: Option<u8>,
    /// The weapon a pistol draw put away, and the ticks since the bot last
    /// saw an enemy with the pistol out.
    primary: u8,
    calm_ticks: u32,
    /// The weapon menu answered last and the weapon it was answered with;
    /// a reopened menu gets the same one while the cvars allow it.
    loadout: Option<(String, &'static str)>,
    /// The heading a back down onto a ladder holds (`crate::nav::Creep`).
    creep: crate::nav::Creep,
    /// The last cmd held the jump key; a second jump needs it released.
    jump_held: bool,
    obj: Objective,
    /// The `bel` marker this bot last stood at; it holds still for
    /// `scr_bel_positiontime` seconds, and the hunt looks round from it
    /// rather than standing on it.
    marker_reached: Option<[f32; 3]>,
    /// The last reliable server command the bot consumed.
    pub(crate) last_seen_seq: i32,
    /// The client-command sequence the bot's own replies use.
    pub(crate) next_command_seq: i32,
}

/// The enemy the bot is engaging and how settled its aim on it is.
struct Target {
    slot: usize,
    /// Ticks before the bot turns toward it at all.
    react: u32,
    /// Aim error, units at the target.
    error: f32,
    /// The error's direction, (pitch, yaw), unit length; resampled every
    /// `wobble` ticks so the aim drifts rather than sitting off by a constant.
    dir: [f32; 2],
    wobble: u32,
    /// Ticks out of sight.
    lost: u32,
}

/// The S&D objective bookkeeping; one life is one round, so a death
/// starts it over (docs/research/bot-objectives.md).
#[derive(Default)]
struct Objective {
    /// Sites given up on this life; the site is `(rank + pick) % sites`.
    pick: u32,
    /// Ticks the use key has been held at the objective; nonzero means the
    /// plant or defuse is in progress and nothing else may break it off.
    held: u32,
    /// Ticks the view has rested on the bomb.
    settled: u32,
    /// Ticks before another try after one that ran out.
    rest: u32,
    /// Set while the bot plays, so the end of a life redraws `pick` once.
    live: bool,
    /// A defender has walked to where the carried objective was taken
    /// from; cleared once nothing is carried.
    swept: bool,
}

/// What the objective wants of the bot right now.
#[derive(Clone, Copy, Debug)]
enum ObjTarget {
    Plant(SiteView),
    /// Stand within [`GUARD_RADIUS`] of a point and fight from there, but
    /// no nearer than `inner`.
    Guard {
        at: [f32; 3],
        inner: f32,
    },
    Defuse(BombView),
    /// Aim at a retrieval objective's pickup trigger, `aim`, and tap use,
    /// standing within reach of `from`.
    Pickup {
        aim: [f32; 3],
        from: [f32; 3],
    },
    /// Carry a retrieval objective into its goal; touching it delivers.
    Deliver([f32; 3]),
    /// Run at a moving point: a carrier the compass shows.
    Chase([f32; 3]),
    /// Walk the carrier's likely way backwards, from the goal side to where
    /// the objective was taken, once per carry.
    Sweep([f32; 3]),
}

/// `level.planttime` is 5 s (`sd.gsc` `bombzones`); a hold this long that
/// planted nothing has failed.
const PLANT_TICKS: u32 = 120;
/// `level.defusetime` is 10 s.
const DEFUSE_TICKS: u32 = 210;
/// The defuse wants `distance(origin, trigger.origin) < 64`.
const DEFUSE_REACH: f32 = 48.0;
const GUARD_RADIUS: f32 = 250.0;
/// A guard ring is at least this wide.
const GUARD_BAND: f32 = 150.0;
/// An attacker steps off the bomb it planted: a body on the line from a
/// defender's eye to the trigger blocks the defuse's `isLookingAt`, which
/// is the defender's problem, not the bot's to make.
const BOMB_GUARD_INNER: f32 = 96.0;
/// A pickup trigger is aimed at and used from within this, horizontally:
/// `G_GetActivateEnt` reaches 128 units from the eye.
const PICKUP_REACH: f32 = 48.0;
/// How far round the pickup a bot steps for another try.
const PICKUP_RING: f32 = 40.0;
/// Use held this long with no link means the plant or defuse never
/// started (not touching, not on the ground, the aim off the trigger).
const START_TICKS: u32 = 20;
/// Release after a failed try, so the next press is a fresh one.
const RETRY_TICKS: u32 = 10;
/// The aim trace is read a frame late (object-model doc 23.1): the view
/// rests on the bomb this many ticks before the press.
const SETTLE_TICKS: u32 = 2;
const SETTLE_DEG: f32 = 1.0;
/// The planter's view pitch, wire convention (up). An item on the floor
/// inside 128 units sits more than 30 degrees below level, and
/// `G_GetActivateEnt`'s cone is 40 either side of the view.
const PLANT_PITCH: f32 = -60.0;
/// Inset from a bombzone's edges before the bot counts as inside it.
const SITE_MARGIN: f32 = 8.0;
/// The standing player's box height, for the zone's z overlap.
const PLAYER_HEIGHT: f32 = 70.0;

/// A spot worth a look: where a seen enemy was lost, or a noise.
#[derive(Clone, Copy, Debug)]
struct Recall {
    at: [f32; 3],
    /// Ticks before it is dropped.
    ticks: u32,
    /// A seen enemy's spot; a noise does not replace it.
    seen: bool,
}

enum Stage {
    Wander,
    ToGrenade,
    Cook {
        left: u32,
    },
    /// The throw is out; hold the rifle byte until `ps.weapon` follows.
    BackToRifle {
        weapon: u8,
    },
}

impl Bot {
    pub fn new(team: &str, shoot: bool, seed: u64) -> Self {
        Bot {
            name: String::new(),
            shoot,
            skill: Skill::default(),
            main_menu: String::new(),
            answered: Vec::new(),
            heading: 0.0,
            heading_ticks: 0,
            stall_origin: [0.0; 3],
            stall_ticks: 0,
            unstick_ticks: 0,
            unstick_at: [f32::INFINITY; 3],
            unstick_tries: 0,
            fire_cooldown: 0,
            burst_ticks: 0,
            target: None,
            recall: None,
            strafe: 0,
            strafe_ticks: 0,
            crouch_ticks: 0,
            grenade_cooldown: 200,
            respawn_ticks: 0,
            stage: Stage::Wander,
            rifle: 0,
            switch_to: None,
            primary: 0,
            calm_ticks: 0,
            loadout: None,
            creep: crate::nav::Creep::default(),
            jump_held: false,
            obj: Objective::default(),
            marker_reached: None,
            last_seen_seq: 0,
            next_command_seq: 1,
            team: team.to_string(),
            rng: seed | 1,
        }
    }

    /// xorshift64* masked to 31 bits, the server's own generator.
    fn rand(&mut self) -> i32 {
        crate::game::host::rand_int(&mut self.rng)
    }

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
        let open: Vec<&'static str> = WEAPON_MENUS
            .iter()
            .find(|(n, _)| *n == menu)?
            .1
            .iter()
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

    /// A new gamestate reruns `ClientConnect` and reopens the menus under the
    /// indices the last one used; without the clear the bot never spawns
    /// again (the same rejoin the probes learned).
    pub fn rearm(&mut self) {
        self.main_menu.clear();
        self.answered.clear();
    }

    /// Where the bot wants to be this tick: the enemy it sees, else the
    /// spot it remembers or heard, else anywhere far. Asked before
    /// [`Bot::think`], which then follows the waypoint the server planned
    /// toward it.
    pub fn goal(&self, view: &BotView) -> Goal {
        if view.dead || !view.playing || !matches!(self.stage, Stage::Wander) {
            return Goal::Hold;
        }
        // A plant or defuse aborts on release, so it outranks a fight.
        if self.objective_busy(view) {
            return Goal::Hold;
        }
        if let Some(e) = view.enemy {
            return Goal::To(e.origin);
        }
        if let Some(bel) = view.bel.as_ref() {
            return self.bel_goal(view, bel);
        }
        if let Some(g) = self.objective_goal(view) {
            return g;
        }
        if let Some(p) = self.recall_goal(view) {
            return Goal::To(p);
        }
        Goal::Roam
    }

    /// The hunted run from the last enemy they saw or heard, else from
    /// their own marker, where the hunters are headed; the hunters go after
    /// what they saw or heard, else the nearest compass marker they have
    /// not yet stood at.
    fn bel_goal(&self, view: &BotView, bel: &BelView) -> Goal {
        if bel.hunted {
            return match self.recall.map(|r| r.at).or(view.noise).or(bel.trail) {
                Some(threat) => Goal::Away(threat),
                None => Goal::Roam,
            };
        }
        if let Some(p) = self.recall_goal(view) {
            return Goal::To(p);
        }
        match self.next_marker(view, bel) {
            Some(m) => Goal::To(m),
            None => Goal::Roam,
        }
    }

    /// The nearest marker other than the one last stood at.
    fn next_marker(&self, view: &BotView, bel: &BelView) -> Option<[f32; 3]> {
        bel.markers
            .iter()
            .filter(|m| self.marker_reached.is_none_or(|r| dist_sq(r, **m) > 1.0))
            .min_by(|a, b| dist_sq(view.origin, **a).total_cmp(&dist_sq(view.origin, **b)))
            .copied()
    }

    /// A plant or defuse is under way: use is held, or the script holds
    /// the body.
    fn objective_busy(&self, view: &BotView) -> bool {
        self.obj.held > 0 || view.linked
    }

    /// The objective this tick, from the bot's role and the round's state.
    fn objective_target(&self, view: &BotView) -> Option<ObjTarget> {
        if let Some(re) = view.re.as_ref() {
            return self.retrieval_target(view, re);
        }
        let sd = view.sd.as_ref()?;
        let site = || {
            let n = sd.sites.len();
            (n > 0).then(|| sd.sites[(sd.rank + self.obj.pick as usize) % n])
        };
        match (sd.role, sd.bomb) {
            (ObjRole::Attack, Some(b)) => Some(ObjTarget::Guard {
                at: b.origin,
                inner: BOMB_GUARD_INNER,
            }),
            (ObjRole::Attack, None) => site().map(ObjTarget::Plant),
            (ObjRole::Defend, Some(b)) if sd.lead => Some(ObjTarget::Defuse(b)),
            // Everyone else keeps off the defuser's line to the bomb.
            (ObjRole::Defend, Some(b)) => Some(ObjTarget::Guard {
                at: b.origin,
                inner: BOMB_GUARD_INNER,
            }),
            (ObjRole::Defend, None) => site().map(|s| ObjTarget::Guard {
                at: s.stand,
                inner: 0.0,
            }),
        }
    }

    /// Attackers: deliver what they carry, else go for the nearest objective
    /// lying there, else escort a teammate carrying one, else stand by the
    /// goal it goes to. Defenders: each guards an objective lying there by
    /// rank. Once one is carried the first by rank holds its goal and the
    /// rest go after the carrier: at him while the compass shows him, else
    /// back along his likely way once, then to the goal.
    fn retrieval_target(&self, view: &BotView, re: &ReView) -> Option<ObjTarget> {
        let o = &re.objectives;
        let nearest = |at: fn(&ReObjView) -> Option<[f32; 3]>| {
            o.iter()
                .filter_map(at)
                .min_by(|a, b| dist_sq(view.origin, *a).total_cmp(&dist_sq(view.origin, *b)))
        };
        match re.role {
            ObjRole::Attack => {
                if let Some(m) = o.iter().find(|m| m.mine) {
                    return Some(ObjTarget::Deliver(m.goal));
                }
                if let Some(aim) = nearest(|m| m.pickup) {
                    // Each tap that took nothing tries the next side: the
                    // use key's pick traces from the eye to the trigger's
                    // middle, and a table or crate can stand in the way.
                    let from = match self.obj.pick % 9 {
                        0 => aim,
                        k => {
                            let a = (k as f32 - 1.0) * 45f32.to_radians();
                            [
                                aim[0] + PICKUP_RING * a.cos(),
                                aim[1] + PICKUP_RING * a.sin(),
                                aim[2],
                            ]
                        }
                    };
                    return Some(ObjTarget::Pickup { aim, from });
                }
                if let Some(at) = Self::carrier(view, o) {
                    return Some(ObjTarget::Guard { at, inner: 0.0 });
                }
                Self::goal_guard(view, o)
            }
            ObjRole::Defend => {
                if let Some(g) = Self::goal_guard(view, o) {
                    if re.rank == 0 {
                        return Some(g);
                    }
                    if let Some(at) = Self::carrier(view, o) {
                        return Some(ObjTarget::Chase(at));
                    }
                    let laid = o.iter().filter(|m| m.carried).find_map(|m| m.laid);
                    return Some(match laid {
                        Some(at) if !self.obj.swept => ObjTarget::Sweep(at),
                        _ => g,
                    });
                }
                let lying: Vec<[f32; 3]> = o.iter().filter_map(|m| m.pickup).collect();
                (!lying.is_empty()).then(|| ObjTarget::Guard {
                    at: lying[re.rank % lying.len()],
                    inner: 0.0,
                })
            }
        }
    }

    /// The nearest carrier the compass shows.
    fn carrier(view: &BotView, o: &[ReObjView]) -> Option<[f32; 3]> {
        o.iter()
            .filter(|m| !m.mine)
            .filter_map(|m| m.carrier_at)
            .min_by(|a, b| dist_sq(view.origin, *a).total_cmp(&dist_sq(view.origin, *b)))
    }

    /// A ring round the nearest goal an objective is being carried to, just
    /// outside its trigger.
    fn goal_guard(view: &BotView, o: &[ReObjView]) -> Option<ObjTarget> {
        o.iter()
            .filter(|m| m.carried)
            .min_by(|a, b| dist_sq(view.origin, a.goal).total_cmp(&dist_sq(view.origin, b.goal)))
            .map(|m| ObjTarget::Guard {
                at: m.goal,
                inner: m.goal_clear,
            })
    }

    fn at_objective(view: &BotView, t: &ObjTarget) -> bool {
        let o = view.origin;
        match t {
            ObjTarget::Plant(s) => {
                let inside = (0..2)
                    .all(|i| o[i] >= s.mins[i] + SITE_MARGIN && o[i] <= s.maxs[i] - SITE_MARGIN)
                    && o[2] <= s.maxs[2]
                    && o[2] + PLAYER_HEIGHT >= s.mins[2];
                // A zone narrower than the margin still has its stand.
                let flat = (o[0] - s.stand[0]).hypot(o[1] - s.stand[1]);
                inside || (flat < 16.0 && (o[2] - s.stand[2]).abs() < 32.0)
            }
            ObjTarget::Guard { at, inner } => {
                let d = dist_sq(o, *at);
                let outer = GUARD_RADIUS.max(inner + GUARD_BAND);
                d < outer * outer && d >= inner * inner
            }
            ObjTarget::Defuse(b) => dist_sq(o, b.origin) < DEFUSE_REACH * DEFUSE_REACH,
            ObjTarget::Pickup { aim, from } => {
                let reach = if from == aim { PICKUP_REACH } else { 16.0 };
                (from[0] - o[0]).hypot(from[1] - o[1]) < reach && (aim[2] - o[2]).abs() < 64.0
            }
            // The goal trigger's touch delivers; there is nothing to stand
            // at.
            ObjTarget::Deliver(_) | ObjTarget::Chase(_) => false,
            ObjTarget::Sweep(at) => arrived(o, *at),
        }
    }

    /// Where the objective sends a bot that is not there yet; `Hold` once
    /// it is, and `None` when the round has no objective for it.
    fn objective_goal(&self, view: &BotView) -> Option<Goal> {
        let t = self.objective_target(view)?;
        if Self::at_objective(view, &t) {
            return Some(Goal::Hold);
        }
        Some(match t {
            ObjTarget::Plant(s) => Goal::To(s.stand),
            // Too close: any way out will do, and the roam leaves the ring
            // well before it gets anywhere.
            ObjTarget::Guard { at, inner } if dist_sq(view.origin, at) < inner * inner => {
                Goal::Roam
            }
            ObjTarget::Guard { at, .. } => Goal::To(at),
            ObjTarget::Defuse(b) => Goal::To(b.origin),
            ObjTarget::Pickup { from: at, .. }
            | ObjTarget::Deliver(at)
            | ObjTarget::Chase(at)
            | ObjTarget::Sweep(at) => Goal::To(at),
        })
    }

    /// The bot stands at its objective without acting on it: it holds its
    /// ground instead of following a waypoint, and is not stuck.
    fn objective_standing(&self, view: &BotView) -> bool {
        view.linked
            || self
                .objective_target(view)
                .is_some_and(|t| Self::at_objective(view, &t))
    }

    /// The plant or defuse itself: the cmd for this tick while one is under
    /// way or starting, `None` to play on as usual.
    fn think_objective(&mut self, view: &BotView, cmd: UserCmd) -> Option<UserCmd> {
        self.obj.rest = self.obj.rest.saturating_sub(1);
        let t = self.objective_target(view);
        let at = t.is_some_and(|t| Self::at_objective(view, &t));
        if let Some(re) = view.re.as_ref() {
            if matches!(t, Some(ObjTarget::Sweep(_))) && at {
                self.obj.swept = true;
            }
            if !re.objectives.iter().any(|m| m.carried) {
                self.obj.swept = false;
            }
        }
        let ready = self.obj.rest == 0 && (at || (view.linked && self.obj.held > 0));
        let cur = [yaw_diff(view.view[0], 0.0), view.view[1]];
        match t {
            Some(ObjTarget::Plant(_)) if ready => {
                if self.obj.held >= PLANT_TICKS || (self.obj.held >= START_TICKS && !view.linked) {
                    // Nothing planted: another zone's plant blocks this one,
                    // or the body never touched. Try the other site.
                    self.obj.held = 0;
                    self.obj.rest = RETRY_TICKS;
                    self.obj.pick = self.obj.pick.wrapping_add(1);
                    return None;
                }
                self.obj.held += 1;
                // Looking up: the press's rising edge also takes the item
                // or turret the view picks (`Cmd_Activate_f`), and a dropped
                // weapon in the zone sits on the floor.
                Some(self.hold_use(view, cmd, [PLANT_PITCH, cur[1]]))
            }
            Some(ObjTarget::Defuse(b)) if ready => {
                if self.obj.held >= DEFUSE_TICKS || (self.obj.held >= START_TICKS && !view.linked) {
                    // The aim trace never reached the trigger (a body in
                    // the way); release and settle again.
                    self.obj.held = 0;
                    self.obj.settled = 0;
                    self.obj.rest = RETRY_TICKS;
                    return None;
                }
                let (tp, ty) = aim_angles(view, b.aim);
                let aim = self.turn_toward(view, [tp, ty]);
                if self.obj.held > 0 {
                    self.obj.held += 1;
                    return Some(self.hold_use(view, cmd, aim));
                }
                // Not started yet: a visible enemy comes first.
                if self.shoot && view.enemy.is_some() {
                    self.obj.settled = 0;
                    return None;
                }
                let off = yaw_diff(tp, cur[0]).hypot(yaw_diff(ty, cur[1]));
                self.obj.settled = if off < SETTLE_DEG {
                    self.obj.settled + 1
                } else {
                    0
                };
                let mut cmd = self.hold_use(view, cmd, aim);
                if self.obj.settled >= SETTLE_TICKS {
                    self.obj.held = 1;
                } else {
                    cmd.buttons = 0;
                }
                Some(cmd)
            }
            Some(ObjTarget::Pickup { aim: spot, .. }) if ready => {
                // Not with an enemy in sight: the pickup is a tap, and can
                // wait.
                if self.shoot && view.enemy.is_some() {
                    self.obj.settled = 0;
                    return None;
                }
                let (tp, ty) = aim_angles(view, spot);
                let aim = self.turn_toward(view, [tp, ty]);
                let off = yaw_diff(tp, cur[0]).hypot(yaw_diff(ty, cur[1]));
                self.obj.settled = if off < SETTLE_DEG {
                    self.obj.settled + 1
                } else {
                    0
                };
                let mut cmd = self.hold_use(view, cmd, aim);
                if self.obj.settled < SETTLE_TICKS {
                    cmd.buttons = 0;
                } else {
                    // A tap: two ticks down, then up for a while. Held, the
                    // use key drops what the carrier holds (`re.gsc`
                    // `holduse`).
                    self.obj.held += 1;
                    if self.obj.held >= 2 {
                        self.obj.held = 0;
                        self.obj.settled = 0;
                        self.obj.rest = RETRY_TICKS;
                        self.obj.pick = self.obj.pick.wrapping_add(1);
                    }
                }
                Some(cmd)
            }
            _ => {
                self.obj.held = 0;
                self.obj.settled = 0;
                None
            }
        }
    }

    /// Use down, no move keys, the view at `aim`. Nothing else runs: the
    /// stall watch restarts from here and the fight is dropped.
    fn hold_use(&mut self, view: &BotView, mut cmd: UserCmd, aim: [f32; 2]) -> UserCmd {
        self.disengage();
        self.stall_origin = view.origin;
        self.stall_ticks = 0;
        cmd.buttons = msg::BUTTON_USE;
        cmd.angles = cmd_angles(view, aim);
        cmd
    }

    /// One life over: the next is a new round, from the first site again,
    /// and no switch carries over.
    fn end_life(&mut self) {
        if self.obj.live {
            self.obj = Objective::default();
        }
        self.switch_to = None;
        self.calm_ticks = 0;
    }

    /// The remembered spot not yet reached, else this tick's noise. The same
    /// rule [`Bot::remember`] applies a tick later, so the two agree.
    fn recall_goal(&self, view: &BotView) -> Option<[f32; 3]> {
        match self.recall {
            Some(r) if !arrived(view.origin, r.at) => Some(r.at),
            _ => view.noise.filter(|n| !arrived(view.origin, *n)),
        }
    }

    /// Keeps [`Bot::recall`] current: a visible enemy is remembered where it
    /// is headed; out of sight the memory runs down and is dropped on
    /// arrival; a noise replaces anything but a seen enemy's spot.
    fn remember(&mut self, view: &BotView) {
        if let Some(e) = view.enemy {
            let lead = |i: usize| e.origin[i] + e.velocity[i] * MEMORY_LEAD_S;
            self.recall = Some(Recall {
                at: [lead(0), lead(1), e.origin[2]],
                ticks: self.skill.memory_ticks,
                seen: true,
            });
            return;
        }
        if let Some(r) = self.recall.as_mut() {
            r.ticks = r.ticks.saturating_sub(1);
        }
        if self
            .recall
            .is_some_and(|r| r.ticks == 0 || arrived(view.origin, r.at))
        {
            self.recall = None;
        }
        if let Some(at) = view.noise
            && !self.recall.is_some_and(|r| r.seen)
        {
            self.recall = Some(Recall {
                at,
                ticks: self.skill.noise_ticks,
                seen: false,
            });
        }
    }

    /// The tick's usercmd. The driver stamps `server_time` and pushes the
    /// cmd into the client's queue.
    pub(crate) fn think(&mut self, view: &BotView) -> UserCmd {
        self.fire_cooldown = self.fire_cooldown.saturating_sub(1);
        let mut cmd = NULL_USERCMD;
        cmd.weapon = view.weapon;
        // Death and spectatorship outrank the stage machine: a corpse still
        // in a grenade phase must reach the respawn press below, not sit in
        // `think_cook` forever.
        if view.dead {
            self.respawn_ticks += 1;
            // Whatever the death interrupted, the new life starts clean: no
            // throw resumes on respawn.
            self.stage = Stage::Wander;
            self.disengage();
            self.recall = None;
            self.end_life();
            // The stock death flow polls the use key only after its own
            // `wait 2` (dm.gsc, `waitRespawnButton`), so a one-shot press
            // lands before any poll reads it; retry every second, the way
            // the probe's target does.
            let since_death = self.respawn_ticks.saturating_sub(RESPAWN_DELAY);
            if self.respawn_ticks >= RESPAWN_DELAY && since_death % RESPAWN_RETRY < 2 {
                cmd.buttons = msg::BUTTON_USE;
            }
            return cmd;
        }
        if !view.playing {
            // Spectator or intermission camera: nothing to press.
            self.respawn_ticks = 0;
            self.disengage();
            self.recall = None;
            self.end_life();
            return cmd;
        }
        self.respawn_ticks = 0;
        self.obj.live = true;
        self.remember(view);
        if let Some(bel) = view.bel.as_ref()
            && let Some(m) = self.next_marker(view, bel)
            && arrived(view.origin, m)
        {
            self.marker_reached = Some(m);
        }
        match self.stage {
            Stage::ToGrenade => return self.think_grenade(view, cmd),
            Stage::Cook { left } => return self.think_cook(view, cmd, left),
            Stage::BackToRifle { weapon } => return self.think_back(view, cmd, weapon),
            Stage::Wander => {}
        }
        if let Some(cmd) = self.think_objective(view, cmd) {
            return cmd;
        }
        // A frag goes at a close enemy, occasionally, once the cooldown is
        // spent; the switch itself is the stage machine below.
        self.grenade_cooldown = self.grenade_cooldown.saturating_sub(1);
        if self.shoot
            && self.grenade_cooldown == 0
            && self.switch_to.is_none()
            && let (Some(g), Some(e)) = (view.grenade, view.enemy)
        {
            let close = dist_sq(view.origin, e.origin) < GRENADE_RANGE * GRENADE_RANGE;
            if close && self.rand() % 4 == 0 {
                self.grenade_cooldown = GRENADE_COOLDOWN_TICKS;
                self.rifle = view.weapon;
                self.stage = Stage::ToGrenade;
                cmd.weapon = g;
                return cmd;
            }
        }

        // A body that has not left a 15-unit circle in ten ticks is stuck,
        // path or not, and takes a random heading for a spell. One standing
        // at its objective, or held by a link, means to stay put.
        let standing = self.objective_standing(view);
        self.stall_ticks += 1;
        // Coming to rest at a leap's foot is not being stuck either.
        if standing || view.stop {
            self.stall_origin = view.origin;
            self.stall_ticks = 0;
            self.unstick_ticks = 0;
        } else if self.stall_ticks >= 10 {
            if dist_sq(view.origin, self.stall_origin) < 15.0 * 15.0 {
                // Stuck again where the last spell started: a pocket a drop
                // is the only way out of (mp_ship's rim at (4223, 345, 276)).
                let again = dist_sq(view.origin, self.unstick_at) < CORNER * CORNER;
                self.unstick_tries = if again { self.unstick_tries + 1 } else { 0 };
                self.unstick_at = view.origin;
                self.pick_heading(view);
                self.unstick_ticks = UNSTICK_TICKS;
            } else {
                self.stall_origin = view.origin;
                self.stall_ticks = 0;
            }
        }
        let steering = !standing && view.waypoint.is_some() && self.unstick_ticks == 0;
        // A leap's foot is slowed onto only on its floor: one at a ladder's
        // head is climbed to at the full rate first.
        let foot = view.waypoint.filter(|w| {
            steering && view.stop && !view.on_ladder && (w[2] - view.origin[2]).abs() < CLIMB_HEIGHT
        });
        let (mut pitch, mut yaw, mut forward) = match view.waypoint {
            // On guard: look about, feet still.
            _ if standing => {
                if self.heading_ticks == 0 {
                    self.pick_heading(view);
                } else {
                    self.heading_ticks -= 1;
                }
                (0.0, self.heading, 0)
            }
            Some(w) if self.unstick_ticks == 0 => {
                let (pitch, yaw, forward, right) = self.steer(view, w);
                cmd.right = right;
                (pitch, yaw, forward)
            }
            // On a ladder with no way to steer, or pinned on it: down.
            // The heading is nothing to a body on a ladder, and the level
            // view of a wander climbs at a third of the rate, which held
            // bots under a spar on mp_ship's ladders for good.
            _ if view.on_ladder => {
                self.unstick_ticks = self.unstick_ticks.saturating_sub(1);
                (crate::nav::LADDER_PITCH, self.heading, 127)
            }
            _ => {
                self.unstick_ticks = self.unstick_ticks.saturating_sub(1);
                if self.heading_ticks == 0
                    || self.shuns(view, self.heading)
                    || self.sliding_off(view)
                {
                    self.pick_heading(view);
                } else {
                    self.heading_ticks -= 1;
                }
                (0.0, self.heading, 127)
            }
        };
        // On level ground with a remembered spot in range, the view turns
        // toward it while the keys keep the path, so a returning enemy is
        // already near the crosshair. The reaction delay is `track`'s
        // business either way.
        if self.shoot
            && view.enemy.is_none()
            && pitch == 0.0
            && forward == 127
            && let Some(r) = self.recall
            && dist_sq(view.origin, r.at) < SHOOT_RANGE * SHOOT_RANGE
        {
            let (tp, ty) = aim_angles(view, r.at);
            let look = self.turn_toward(view, [tp, ty]);
            // The path heading relative to the view, counter-clockwise.
            let d = yaw_diff(yaw, look[1]).to_radians();
            forward = (d.cos() * 127.0).round() as i8;
            cmd.right = (-d.sin() * 127.0).round() as i8;
            [pitch, yaw] = look;
        }
        // Slowing onto a leap's foot, then standing on it.
        if let Some(w) = foot {
            let flat = (w[0] - view.origin[0]).hypot(w[1] - view.origin[1]);
            forward = if flat < crate::nav::LEAP_FOOT * 0.5 {
                0
            } else {
                (flat / LEAP_SLOW * 127.0).min(127.0) as i8
            };
        }
        // Engaging overrides all of this: `footwork` owns the move keys and
        // the aim owns the view.
        cmd.forward = forward;
        if steering
            && view.jump
            && let Some(w) = view.waypoint
        {
            // Closing on the waypoint under the walk's pinned pace: stopped
            // at the ledge's face, or sliding along it.
            let to = glam::Vec2::new(w[0] - view.origin[0], w[1] - view.origin[1]);
            let v = glam::Vec2::new(view.velocity[0], view.velocity[1]);
            let closing = v.dot(to.normalize_or_zero()) * (FRAME_MS as f32 / 1000.0);
            let rise = w[2] - view.origin[2];
            let cue = crate::nav::jump_cue(view.leap, view.lip, closing, to.length(), rise);
            cmd.up = crate::nav::jump_key(view.on_ground, cue, self.jump_held);
        }
        self.jump_held = cmd.up > 0;
        if self.shoot {
            self.track(view.enemy);
            if let Some(e) = view.enemy
                && let Some(aim) = self.engage(view, e, &mut cmd)
            {
                [pitch, yaw] = aim;
            }
        }
        // A switch ends when it lands, or when the weapon is gone (dropped,
        // or swapped at a pickup) and the byte would read as a holster for
        // good.
        if let Some(w) = self.switch_to
            && (view.weapon == w || view.weapons_held >> w & 1 == 0)
        {
            self.switch_to = None;
        }
        if self.shoot {
            self.sidearm(view);
        }
        if let Some(w) = self.switch_to {
            cmd.weapon = w;
        }
        // A dry clip reloads, unless a switch is putting it away or there is
        // nothing to load; the tap is suppressed while the machine is busy
        // so a held bit cannot restart the reload it started.
        if view.clip == 0
            && view.busy_ms == 0
            && self.switch_to.is_none()
            && view.loaded >> view.weapon.min(63) & 1 == 1
        {
            cmd.wbuttons |= msg::WBUTTON_RELOAD;
        }
        cmd.angles = cmd_angles(view, [pitch, yaw]);
        cmd
    }

    /// The pistol comes out when the held weapon runs dry with an enemy
    /// close and the draw beats the reload to the next shot, or when the
    /// held weapon has nothing left to reload. It goes back once no enemy
    /// has been in sight for [`HOLSTER_TICKS`], or at once when the pistol
    /// is spent, as long as the primary has a round left.
    fn sidearm(&mut self, view: &BotView) {
        let Some(pistol) = view.pistol else {
            return;
        };
        if self.switch_to.is_some() {
            return;
        }
        let loaded = |w: u8| w != 0 && w < 64 && view.loaded >> w & 1 == 1;
        if view.weapon != pistol {
            let held = view.weapon != 0 && view.weapons_held >> view.weapon & 1 == 1;
            if !held || view.grenade == Some(view.weapon) || !loaded(pistol) {
                return;
            }
            let close = view
                .enemy
                .is_some_and(|e| dist_sq(view.origin, e.origin) < PISTOL_RANGE * PISTOL_RANGE);
            let quicker = view.draw_ms < view.reload_ms;
            if (view.clip == 0 && close && quicker) || !loaded(view.weapon) {
                self.primary = view.weapon;
                self.calm_ticks = 0;
                self.switch_to = Some(pistol);
            }
            return;
        }
        self.calm_ticks = if view.enemy.is_some() {
            0
        } else {
            self.calm_ticks + 1
        };
        if loaded(self.primary) && (self.calm_ticks >= HOLSTER_TICKS || !loaded(pistol)) {
            self.switch_to = Some(self.primary);
        }
    }

    /// (pitch, yaw, forward) that take the body toward waypoint `to`. A
    /// waypoint well above, or the body on a ladder with the waypoint above,
    /// is climbed looking up; one well below, or below while on a ladder, is
    /// backed toward facing away, creeping at the lip. That is how the
    /// graph's walks proved the edge (`crate::nav`).
    fn steer(&mut self, view: &BotView, to: [f32; 3]) -> (f32, f32, i8, i8) {
        let from = view.origin;
        let yaw = (to[1] - from[1]).atan2(to[0] - from[0]).to_degrees();
        let dz = to[2] - from[2];
        if dz < -BACK_DOWN_HEIGHT || (view.on_ladder && dz < 0.0) {
            let flat = (to[0] - from[0]).hypot(to[1] - from[1]);
            let forward = crate::nav::back_move(flat, view.on_ladder);
            let yaw = self.creep.hold(forward, yaw_diff(yaw + 180.0, 0.0));
            return (-crate::nav::LADDER_PITCH, yaw, forward, 0);
        }
        self.creep.hold(127, yaw);
        if view.on_ladder && dz > vcod_common::pmove::STEPSIZE {
            // Square into the face, strafing toward the climb's line, until
            // level with the waypoint: a ledge beside the rungs is stepped
            // onto facing it.
            let yaw = crate::nav::climb_yaw(view.ladder_normal.into()).unwrap_or(yaw);
            let right = view.ladder_middle.map_or(0, |m| {
                let (s, c) = yaw.to_radians().sin_cos();
                let off = (m[0] - from[0]) * s - (m[1] - from[1]) * c;
                if off.abs() < LADDER_CENTRED {
                    0
                } else {
                    (off * 16.0).clamp(-64.0, 64.0) as i8
                }
            });
            (-crate::nav::LADDER_PITCH, yaw, 127, right)
        } else if dz > CLIMB_HEIGHT || view.on_ladder {
            (-crate::nav::LADDER_PITCH, yaw, 127, 0)
        } else {
            (0.0, yaw, 127, 0)
        }
    }

    /// Keeps [`Bot::target`] in step with the server's enemy: a new slot is
    /// a new target with a fresh reaction and full error; one out of sight
    /// for `forget_ticks` is dropped.
    fn track(&mut self, enemy: Option<EnemyView>) {
        let Some(e) = enemy else {
            if let Some(t) = self.target.as_mut() {
                t.lost += 1;
                if t.lost > self.skill.forget_ticks {
                    self.disengage();
                }
            }
            return;
        };
        if let Some(t) = self.target.as_mut().filter(|t| t.slot == e.slot) {
            t.lost = 0;
            return;
        }
        let react = self.skill.reaction_min + self.rand_below(self.skill.reaction_spread);
        self.burst_ticks = 0;
        self.target = Some(Target {
            slot: e.slot,
            react,
            error: self.skill.error_start,
            dir: [0.0, 1.0],
            wobble: 0,
            lost: 0,
        });
    }

    fn disengage(&mut self) {
        self.target = None;
        self.burst_ticks = 0;
        self.crouch_ticks = 0;
    }

    /// One tick against the tracked target: turn toward it at a bounded
    /// rate, sights, trigger and footwork into `cmd`. Returns the commanded
    /// (pitch, yaw), or `None` while the reaction delay still runs.
    fn engage(&mut self, view: &BotView, e: EnemyView, cmd: &mut UserCmd) -> Option<[f32; 2]> {
        let mut t = self.target.take()?;
        if t.react > 0 {
            t.react -= 1;
            self.target = Some(t);
            return None;
        }
        t.error = (t.error * self.skill.error_decay).max(self.skill.error_floor);
        if t.wobble == 0 {
            let a = (self.rand() % 360) as f32;
            t.dir = [a.to_radians().sin(), a.to_radians().cos()];
            t.wobble = 4 + self.rand_below(5);
        }
        t.wobble -= 1;
        let dist = dist_sq(view.origin, e.origin).sqrt().max(1.0);
        let moving = if view.speed > MOVING_SPEED {
            self.skill.error_moving
        } else {
            1.0
        };
        let miss = t.error * (1.0 + dist / self.skill.error_range) * moving;
        let err_deg = (miss / dist).atan().to_degrees();
        let dir = t.dir;
        self.target = Some(t);

        let (tp, ty) = aim_angles(view, e.origin);
        let aim = self.turn_toward(view, [tp + dir[0] * err_deg, ty + dir[1] * err_deg]);
        // The view's miss of the true chest point after this cmd's turn.
        let off = yaw_diff(aim[0], tp).hypot(yaw_diff(aim[1], ty));
        let cone = (HIT_RADIUS / dist).atan().to_degrees();

        let ads = view.has_ads
            && if view.sniper {
                dist > SCOPE_MIN_RANGE
            } else {
                dist > self.skill.ads_range
            };
        if ads {
            cmd.buttons |= msg::BUTTON_ADS;
        }
        // A scope or sights still coming up would waste the shot on hip spread.
        let ready = !ads || view.ads_frac >= 1.0;
        self.trigger(view, dist, off, cone, ready, cmd);
        self.footwork(view, dist, ads, cmd);
        Some(aim)
    }

    /// The (pitch, yaw) one tick's turn takes the view to on its way to
    /// `want`, at most `turn_deg` along the straight line between them.
    fn turn_toward(&self, view: &BotView, want: [f32; 2]) -> [f32; 2] {
        let cur = [yaw_diff(view.view[0], 0.0), view.view[1]];
        let d = [yaw_diff(want[0], cur[0]), yaw_diff(want[1], cur[1])];
        let len = d[0].hypot(d[1]);
        let k = if len > self.skill.turn_deg {
            self.skill.turn_deg / len
        } else {
            1.0
        };
        [(cur[0] + d[0] * k).clamp(-80.0, 80.0), cur[1] + d[1] * k]
    }

    /// Semi-autos tap, at least `fireTime` apart (AGENTS.md, "Fire is
    /// tapped"); automatics hold the bit for a burst, shorter at range.
    fn trigger(
        &mut self,
        view: &BotView,
        dist: f32,
        off: f32,
        cone: f32,
        ready: bool,
        cmd: &mut UserCmd,
    ) {
        if view.clip == 0 {
            self.burst_ticks = 0;
            return;
        }
        if view.automatic && self.burst_ticks > 0 {
            // A burst rides out recoil and wobble, but not a lost line.
            if off <= cone * 2.0 {
                cmd.buttons |= msg::BUTTON_ATTACK;
                self.burst_ticks -= 1;
            } else {
                self.burst_ticks = 0;
            }
            if self.burst_ticks == 0 {
                self.fire_cooldown = 3 + self.rand_below(4);
            }
            return;
        }
        if self.fire_cooldown > 0 || view.busy_ms != 0 || !ready || off > cone {
            return;
        }
        cmd.buttons |= msg::BUTTON_ATTACK;
        if view.automatic {
            let long = dist > 800.0;
            self.burst_ticks = if long { 1 } else { 2 } + self.rand_below(if long { 3 } else { 6 });
        } else {
            let fire = if view.fire_time_ms > 0 {
                view.fire_time_ms
            } else {
                200
            };
            // Rounded up, and never under two ticks: the bit has to come up
            // between taps for the next to read as a press.
            let ticks = ((fire + FRAME_MS - 1) / FRAME_MS).max(2) as u32;
            self.fire_cooldown = ticks + self.rand_below(3);
        }
    }

    /// Strafe in spells and crouch now and then while engaging; close in
    /// only past the weapon's working range. Scoped and sighted shots at
    /// range stand still, since moving widens the error.
    fn footwork(&mut self, view: &BotView, dist: f32, ads: bool, cmd: &mut UserCmd) {
        if self.strafe_ticks == 0 {
            self.strafe = if self.rand() % 2 == 0 { 127 } else { -127 };
            self.strafe_ticks = 8 + self.rand_below(16);
            if self.crouch_ticks == 0 && self.rand_below(if view.sniper { 2 } else { 4 }) == 0 {
                self.crouch_ticks = 20 + self.rand_below(30);
            }
        } else {
            self.strafe_ticks -= 1;
        }
        let range = if view.sniper {
            f32::INFINITY
        } else if view.automatic {
            350.0
        } else {
            700.0
        };
        cmd.forward = if dist > range { 127 } else { 0 };
        cmd.right = if ads { 0 } else { self.strafe };
        if self.crouch_ticks > 0 {
            self.crouch_ticks -= 1;
            // The stance is a held level; retail sends `upmove` -127 with it
            // (protocol doc, "Usercmd input bits").
            cmd.wbuttons |= msg::WBUTTON_CROUCH;
            cmd.up = -127;
        }
    }

    /// `0..n`, 0 when `n` is 0.
    fn rand_below(&mut self, n: u32) -> u32 {
        if n == 0 { 0 } else { self.rand() as u32 % n }
    }

    /// On the random heading a stuck bot takes ([`BotBody::unsticking`]).
    pub fn unsticking(&self) -> bool {
        self.unstick_ticks > 0
    }

    /// A fresh wander heading, and a new stall baseline to measure it by.
    fn pick_heading(&mut self, view: &BotView) {
        self.heading = (self.rand() % 360) as f32;
        // Turned a step at a time off a hazard or a drop, beside the
        // heading too where it can (a heading into a wall slides along it:
        // on mp_ship's deck one 30 degrees off the drop slid a bot off it),
        // then off a hazard alone; boxed in by hazards, it goes anyway.
        let start = self.heading;
        let turn = |ok: &dyn Fn(f32) -> bool| {
            (0..8)
                .map(|i| (start + 45.0 * i as f32) % 360.0)
                .find(|h| ok(*h))
        };
        let wide = |h: f32| [-45.0, 0.0, 45.0].iter().all(|d| !self.shuns(view, h + d));
        self.heading = turn(&wide)
            .or_else(|| turn(&|h| !self.shuns(view, h)))
            .or_else(|| turn(&|h| !view.hazard_ahead[octant(h)]))
            .unwrap_or(start);
        self.heading_ticks = 40 + (self.rand() % 40) as u32;
        self.stall_origin = view.origin;
        self.stall_ticks = 0;
    }

    /// A hazard, or a drop, lies along `yaw`'s octant. A drop is fine to
    /// a cornered bot, and to one whose path drops: its waypoint lies more
    /// than a jump below.
    fn shuns(&self, view: &BotView, yaw: f32) -> bool {
        let o = octant(yaw);
        let below = |w: [f32; 3]| w[2] < view.origin[2] - vcod_common::pmove::JUMP_HEIGHT;
        let path_drops = view.waypoint.is_some_and(below);
        let shun =
            |depth: f32| !path_drops && (self.unstick_tries < CORNERED || depth > FALL_HURTS);
        view.hazard_ahead[o] || view.drop_ahead[o].is_some_and(shun)
    }

    /// The body moves toward a drop it shuns, whatever its heading: one
    /// pushed into a wall slides along it.
    fn sliding_off(&self, view: &BotView) -> bool {
        let [x, y, _] = view.velocity;
        x.hypot(y) > 1.0 && self.shuns(view, y.atan2(x).to_degrees())
    }

    fn think_grenade(&mut self, view: &BotView, mut cmd: UserCmd) -> UserCmd {
        if let Some(g) = view.grenade {
            cmd.weapon = g;
            if view.weapon == g {
                // No cook exists in 1.1 (combat doc 1.11): the release throws,
                // so the hold is only long enough to look like a throw, not a
                // cook.
                let left = 6 + (self.rand() % 8) as u32;
                self.stage = Stage::Cook { left };
            }
            return cmd;
        }
        self.stage = Stage::Wander;
        cmd
    }

    fn think_cook(&mut self, view: &BotView, mut cmd: UserCmd, left: u32) -> UserCmd {
        if let Some(g) = view.grenade {
            cmd.weapon = g;
        }
        if view.weapon != cmd.weapon || !view.playing {
            return cmd;
        }
        if left > 1 {
            cmd.buttons = msg::BUTTON_ATTACK;
            self.stage = Stage::Cook { left: left - 1 };
        } else {
            // The release; the next stage walks the byte back to the rifle.
            self.stage = Stage::BackToRifle { weapon: self.rifle };
        }
        cmd
    }

    fn think_back(&mut self, view: &BotView, mut cmd: UserCmd, weapon: u8) -> UserCmd {
        cmd.weapon = weapon;
        if view.weapon == weapon {
            self.stage = Stage::Wander;
        }
        cmd
    }
}

/// How far ahead of a wandering bot [`BotView::hazard_ahead`] looks.
pub const HAZARD_LOOK: f32 = 64.0;

/// The compass octant of a yaw in degrees, as [`BotView::hazard_ahead`]
/// indexes them.
pub fn octant(yaw: f32) -> usize {
    ((yaw.rem_euclid(360.0) + 22.5) / 45.0) as usize % 8
}

/// A cornered bot still keeps off a drop this deep: past
/// `bg_fallDamageMinHeight` the fall hurts. mp_ship's 280 off the 216
/// floor at x 2010 took a bot's health with it (section 3).
const FALL_HURTS: f32 = 256.0;
/// A climb this near the middle of the rungs along the face stops strafing.
const LADDER_CENTRED: f32 = 2.0;
/// Ticks a stuck bot spends on a random heading before its waypoint again.
const UNSTICK_TICKS: u32 = 15;
/// Unstick spells that start within [`CORNER`] units of the one before;
/// from this many on, the bot is cornered and a drop is a way out.
const CORNERED: u32 = 3;
const CORNER: f32 = 96.0;
/// Inside this of a leap's foot, flat, the run slows in proportion.
const LEAP_SLOW: f32 = 48.0;
/// A waypoint this far above the feet is up a ladder: look up, where
/// `ladder_move` climbs at full rate.
const CLIMB_HEIGHT: f32 = 48.0;
/// A waypoint this far below is down a ladder or off a ledge: back onto it
/// facing the way it came, which is the side a ladder's grab traces toward
/// (the graph proved the edge the same way, `crate::nav`).
const BACK_DOWN_HEIGHT: f32 = 64.0;

/// The pistol comes out against an enemy this close with the primary dry.
const PISTOL_RANGE: f32 = 500.0;
/// Ticks with no enemy in sight before the pistol goes back.
const HOLSTER_TICKS: u32 = 40;
/// A scope inside this is a liability: the zoomed view cannot track a
/// target this close, so the shot goes from the hip.
const SCOPE_MIN_RANGE: f32 = 200.0;

/// Grenade reuses are minutes apart, not seconds.
const GRENADE_COOLDOWN_TICKS: u32 = 400;
const BOT_EYE_HEIGHT: f32 = 60.0;
use vcod_common::pmove::cmd::ANGLE2SHORT;

/// Whether a body at `origin` (feet) has reached the chest-high `spot`.
fn arrived(origin: [f32; 3], spot: [f32; 3]) -> bool {
    let h = (spot[0] - origin[0]).hypot(spot[1] - origin[1]);
    h < ARRIVE_RADIUS && (spot[2] - origin[2]).abs() < ARRIVE_HEIGHT
}

pub(crate) fn dist_sq(a: [f32; 3], b: [f32; 3]) -> f32 {
    let d = [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
    d[0] * d[0] + d[1] * d[1] + d[2] * d[2]
}

/// Wire pitch is positive down (protocol doc, "View angles"), so a target
/// below the eye aims positive.
fn aim_angles(view: &BotView, at: [f32; 3]) -> (f32, f32) {
    let dx = at[0] - view.origin[0];
    let dy = at[1] - view.origin[1];
    let dz = at[2] - (view.origin[2] + BOT_EYE_HEIGHT);
    let horiz = (dx * dx + dy * dy).sqrt().max(1.0);
    let pitch = (-dz.atan2(horiz).to_degrees()).clamp(-80.0, 80.0);
    let yaw = dy.atan2(dx).to_degrees();
    (pitch, yaw)
}

/// An absolute (pitch, yaw) as the cmd carries it, net of `delta_angles`.
fn cmd_angles(view: &BotView, aim: [f32; 2]) -> [i32; 3] {
    [
        deg_short(aim[0]) - view.delta_angles[0],
        deg_short(aim[1]) - view.delta_angles[1],
        -view.delta_angles[2],
    ]
}

/// The shorter signed way round, degrees.
fn yaw_diff(to: f32, from: f32) -> f32 {
    (to - from + 180.0).rem_euclid(360.0) - 180.0
}

fn deg_short(deg: f32) -> i32 {
    (deg * ANGLE2SHORT) as i32
}

/// The stock weapon menus (`ui_mp/scriptmenus/weapon_<nationality>.menu`),
/// each weapon with the cvar `_teams::restrict` checks it against. Any other
/// response is "restricted" and reopens the menu.
const WEAPON_MENUS: [(&str, &[(&str, &str)]); 4] = [
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

#[cfg(test)]
mod tests {
    use super::*;
    use vcod_common::net::msg::{BUTTON_ADS, BUTTON_ATTACK, BUTTON_USE, WBUTTON_RELOAD};

    const SID: i32 = 0x10;

    fn all(_: &str) -> bool {
        true
    }

    /// A shooting bot with no reaction delay, already looking at an enemy
    /// 100 units down +x.
    fn engaged(seed: u64) -> (Bot, BotView) {
        let mut bot = Bot::new("allies", true, seed);
        bot.skill.reaction_min = 0;
        bot.skill.reaction_spread = 0;
        let mut v = view();
        v.view = [0.0, 0.0, 0.0];
        v.enemy = Some(EnemyView {
            slot: 3,
            origin: [100.0, 0.0, 124.0],
            velocity: [0.0; 3],
        });
        (bot, v)
    }

    fn view() -> BotView {
        BotView {
            origin: [0.0, 0.0, 64.0],
            delta_angles: [0; 3],
            view: [0.0, 90.0, 0.0],
            weapon: 10,
            weapons_held: (1 << 10) | (1 << 6),
            clip: 8,
            fire_time_ms: 300,
            busy_ms: 0,
            automatic: false,
            has_ads: true,
            sniper: false,
            ads_frac: 0.0,
            speed: 0.0,
            velocity: [0.0; 3],
            dead: false,
            playing: true,
            enemy: None,
            grenade: Some(6),
            waypoint: None,
            hazard_ahead: [false; 8],
            drop_ahead: [None; 8],
            noise: None,
            linked: false,
            on_ladder: false,
            ladder_normal: [0.0; 3],
            ladder_middle: None,
            on_ground: true,
            jump: false,
            leap: false,
            lip: false,
            stop: false,
            pistol: None,
            reload_ms: 2500,
            draw_ms: 750,
            loaded: (1 << 10) | (1 << 6),
            sd: None,
            re: None,
            bel: None,
        }
    }

    /// A view standing in the middle of one bombzone, with a second far off.
    fn sd_view(role: ObjRole) -> BotView {
        let mut v = view();
        v.sd = Some(SdView {
            role,
            sites: vec![
                SiteView {
                    mins: [-100.0, -100.0, 0.0],
                    maxs: [100.0, 100.0, 128.0],
                    stand: [0.0, 0.0, 64.0],
                },
                SiteView {
                    mins: [1900.0, -100.0, 0.0],
                    maxs: [2100.0, 100.0, 128.0],
                    stand: [2000.0, 0.0, 64.0],
                },
            ],
            bomb: None,
            rank: 0,
            lead: true,
        });
        v
    }

    /// An attacker that always goes to the first site.
    fn attacker() -> Bot {
        let mut bot = Bot::new("allies", true, 1);
        bot.obj.pick = 0;
        bot
    }

    fn plant_bomb(v: &mut BotView, at: [f32; 3]) {
        let sd = v.sd.as_mut().unwrap();
        sd.sites.clear();
        sd.bomb = Some(BombView {
            origin: at,
            aim: [at[0], at[1], at[2] + 8.0],
        });
    }

    #[test]
    fn an_attacker_at_a_site_holds_use_still_until_the_zones_go() {
        let mut bot = attacker();
        let mut v = sd_view(ObjRole::Attack);
        for tick in 0..100 {
            let cmd = bot.think(&v);
            assert_eq!(cmd.buttons, BUTTON_USE, "tick {tick}");
            assert_eq!((cmd.forward, cmd.right, cmd.up), (0, 0, 0), "tick {tick}");
            assert_eq!(cmd.weapon, 10, "the cmd carries the held weapon byte");
            assert_eq!(bot.goal(&v), Goal::Hold);
            // The script links the planter once its loop reads the press.
            v.linked = tick >= 2;
        }
        // Planted: the zones are gone and the bomb sits at the feet.
        plant_bomb(&mut v, [0.0, 0.0, 64.0]);
        v.linked = false;
        let cmd = bot.think(&v);
        assert_eq!(cmd.buttons & BUTTON_USE, 0, "use held past the plant");
        assert_eq!(bot.goal(&v), Goal::Roam, "the planter stays on its bomb");
    }

    #[test]
    fn a_planting_bot_holds_use_through_an_enemy_and_a_stall() {
        let mut bot = attacker();
        let mut v = sd_view(ObjRole::Attack);
        bot.think(&v);
        v.linked = true;
        v.enemy = Some(EnemyView {
            slot: 3,
            origin: [300.0, 0.0, 104.0],
            velocity: [0.0; 3],
        });
        for tick in 0..90 {
            let cmd = bot.think(&v);
            assert_eq!(cmd.buttons, BUTTON_USE, "tick {tick}: let go to fight");
            assert_eq!((cmd.forward, cmd.right), (0, 0), "tick {tick}");
            assert_eq!(bot.goal(&v), Goal::Hold);
        }
    }

    #[test]
    fn a_plant_that_never_links_is_let_go_for_the_other_site() {
        let mut bot = attacker();
        let v = sd_view(ObjRole::Attack);
        let held = (0..60)
            .take_while(|_| bot.think(&v).buttons == BUTTON_USE)
            .count();
        assert_eq!(held as u32, START_TICKS);
        assert_eq!(bot.goal(&v), Goal::To([2000.0, 0.0, 64.0]));
    }

    #[test]
    fn a_defender_turns_onto_the_bomb_then_holds_use() {
        let mut bot = Bot::new("axis", true, 1);
        let mut v = sd_view(ObjRole::Defend);
        let bomb = [30.0, 0.0, 64.0];
        plant_bomb(&mut v, bomb);
        // Facing away from it, down +y.
        v.view = [0.0, 90.0, 0.0];
        assert_eq!(bot.goal(&v), Goal::Hold, "within reach already");
        let (tp, ty) = aim_angles(&v, [30.0, 0.0, 72.0]);
        let mut pressed = None;
        for tick in 0..40 {
            let cmd = bot.think(&v);
            let aim = [
                cmd.angles[0] as f32 / ANGLE2SHORT,
                cmd.angles[1] as f32 / ANGLE2SHORT,
            ];
            assert_eq!((cmd.forward, cmd.right), (0, 0));
            if cmd.buttons & BUTTON_USE != 0 {
                // Pressed only with the view, not just the cmd, on the bomb.
                assert!(yaw_diff(v.view[0], tp).hypot(yaw_diff(v.view[1], ty)) < SETTLE_DEG);
                pressed = Some(tick);
                break;
            }
            v.view = [aim[0], aim[1], 0.0];
        }
        let pressed = pressed.expect("never pressed use on the bomb");
        assert!(pressed >= 6, "90 degrees at 15 a tick takes six ticks");
        v.linked = true;
        for _ in 0..180 {
            assert_eq!(bot.think(&v).buttons, BUTTON_USE);
        }
        // Defused: the trigger is gone, so is the press.
        v.sd.as_mut().unwrap().bomb = None;
        v.linked = false;
        assert_eq!(bot.think(&v).buttons & BUTTON_USE, 0);
    }

    #[test]
    fn a_planter_looks_up_off_the_floor() {
        let mut bot = attacker();
        let v = sd_view(ObjRole::Attack);
        let cmd = bot.think(&v);
        assert_eq!(cmd.buttons, BUTTON_USE);
        assert_eq!(
            cmd.angles[0],
            deg_short(PLANT_PITCH),
            "pressed looking down"
        );
    }

    #[test]
    fn a_team_deals_its_bots_out_over_both_sites() {
        for role in [ObjRole::Attack, ObjRole::Defend] {
            let mut v = sd_view(role);
            v.origin = [1000.0, 0.0, 64.0];
            let goals: Vec<Goal> = (0..4)
                .map(|rank| {
                    v.sd.as_mut().unwrap().rank = rank;
                    Bot::new("allies", true, rank as u64).goal(&v)
                })
                .collect();
            let (a, b) = (Goal::To([0.0, 0.0, 64.0]), Goal::To([2000.0, 0.0, 64.0]));
            assert_eq!(goals, [a, b, a, b], "{role:?}");
        }
    }

    #[test]
    fn only_the_lead_defender_goes_for_the_bomb() {
        let bomb = [30.0, 0.0, 64.0];
        let mut v = sd_view(ObjRole::Defend);
        plant_bomb(&mut v, bomb);
        v.sd.as_mut().unwrap().lead = false;
        v.view = [0.0, 0.0, 0.0];
        let mut bot = Bot::new("axis", true, 1);
        // On the bomb: it steps off, the defuser's line clear.
        assert_eq!(bot.goal(&v), Goal::Roam);
        for _ in 0..60 {
            assert_eq!(bot.think(&v).buttons & BUTTON_USE, 0);
        }
        // Off to the side, inside the guard ring, it holds.
        v.origin = [200.0, 0.0, 64.0];
        assert_eq!(bot.goal(&v), Goal::Hold);
        // Far off, it closes on the ring, not the bomb's reach.
        v.origin = [1000.0, 0.0, 64.0];
        assert_eq!(bot.goal(&v), Goal::To(bomb));
        bot.think(&v);
        v.origin = [150.0, 0.0, 64.0];
        assert_eq!(bot.goal(&v), Goal::Hold);
    }

    /// A view on an `re` level: one objective lying 30 units ahead (its
    /// pickup trigger's middle on the floor), its goal far off.
    fn re_view(role: ObjRole) -> BotView {
        let mut v = view();
        v.view = [0.0, 0.0, 0.0];
        v.re = Some(ReView {
            role,
            rank: 0,
            objectives: vec![ReObjView {
                pickup: Some([30.0, 0.0, 72.0]),
                goal: [3000.0, 0.0, 64.0],
                goal_clear: 300.0,
                carried: false,
                mine: false,
                carrier_at: None,
                laid: Some([30.0, 0.0, 64.0]),
            }],
        });
        v
    }

    #[test]
    fn an_attacker_taps_use_on_the_objective_then_carries_it_home() {
        let mut bot = Bot::new("allies", true, 1);
        let mut v = re_view(ObjRole::Attack);
        assert_eq!(bot.goal(&v), Goal::Hold, "within reach already");
        let mut presses = Vec::new();
        while presses.len() < 40 && !presses.ends_with(&[true, true, false]) {
            let cmd = bot.think(&v);
            if !presses.ends_with(&[true, true]) {
                assert_eq!((cmd.forward, cmd.right), (0, 0), "moved before the tap");
            }
            // The sim takes the view where the cmd points it.
            let a = cmd.angles.map(|a| a as f32 / ANGLE2SHORT);
            v.view = [a[0], a[1], 0.0];
            presses.push(cmd.buttons & BUTTON_USE != 0);
        }
        // Two ticks down, then up: held, the use key drops what a carrier
        // holds.
        assert!(
            presses.ends_with(&[false, true, true, false]),
            "{presses:?}"
        );
        // A tap that took nothing tries from beside the objective next.
        assert_ne!(bot.goal(&v), Goal::Hold);
        // Taken: the trigger is gone and the bot carries it to the goal,
        // use up all the way.
        let o = &mut v.re.as_mut().unwrap().objectives[0];
        (o.pickup, o.carried, o.mine) = (None, true, true);
        assert_eq!(bot.goal(&v), Goal::To([3000.0, 0.0, 64.0]));
        for _ in 0..100 {
            assert_eq!(bot.think(&v).buttons & BUTTON_USE, 0);
        }
    }

    #[test]
    fn a_defender_guards_the_objective_then_the_goal_it_is_carried_to() {
        let bot = Bot::new("axis", true, 1);
        let mut v = re_view(ObjRole::Defend);
        v.origin = [1000.0, 0.0, 64.0];
        assert_eq!(bot.goal(&v), Goal::To([30.0, 0.0, 72.0]));
        let o = &mut v.re.as_mut().unwrap().objectives[0];
        (o.pickup, o.carried) = (None, true);
        assert_eq!(bot.goal(&v), Goal::To([3000.0, 0.0, 64.0]));
        // It holds outside the goal trigger, and walks out of it.
        v.origin = [2650.0, 0.0, 64.0];
        assert_eq!(bot.goal(&v), Goal::Hold);
        v.origin = [2900.0, 0.0, 64.0];
        assert_eq!(bot.goal(&v), Goal::Roam);
        // A teammate carries it: an attacker escorts it the same way.
        let mut v = re_view(ObjRole::Attack);
        let o = &mut v.re.as_mut().unwrap().objectives[0];
        (o.pickup, o.carried) = (None, true);
        assert_eq!(bot.goal(&v), Goal::To([3000.0, 0.0, 64.0]));
        v.origin = [2900.0, 0.0, 64.0];
        assert_eq!(bot.goal(&v), Goal::Roam);
    }

    /// Past the first by rank, a defender goes after a carried objective:
    /// back to where it was taken while the compass hides the carrier,
    /// then to the goal; straight at him once the compass shows him.
    #[test]
    fn the_other_defenders_go_after_the_carrier() {
        let mut bot = Bot::new("axis", true, 1);
        let mut v = re_view(ObjRole::Defend);
        v.origin = [2500.0, 0.0, 64.0];
        let re = v.re.as_mut().unwrap();
        re.rank = 1;
        let o = &mut re.objectives[0];
        (o.pickup, o.carried) = (None, true);
        assert_eq!(bot.goal(&v), Goal::To([30.0, 0.0, 64.0]));
        v.origin = [40.0, 0.0, 64.0];
        bot.think(&v);
        assert_eq!(bot.goal(&v), Goal::To([3000.0, 0.0, 64.0]), "swept once");
        v.re.as_mut().unwrap().objectives[0].carrier_at = Some([1200.0, 50.0, 64.0]);
        assert_eq!(bot.goal(&v), Goal::To([1200.0, 50.0, 64.0]));
        // Dropped and lying again: the next carry is swept afresh.
        let o = &mut v.re.as_mut().unwrap().objectives[0];
        (o.pickup, o.carried, o.carrier_at) = (Some([30.0, 0.0, 72.0]), false, None);
        bot.think(&v);
        let o = &mut v.re.as_mut().unwrap().objectives[0];
        (o.pickup, o.carried) = (None, true);
        v.origin = [2500.0, 0.0, 64.0];
        assert_eq!(bot.goal(&v), Goal::To([30.0, 0.0, 64.0]));
    }

    /// An attacker escorts a teammate's carry: it closes on him and holds
    /// once within the guard ring.
    #[test]
    fn an_attacker_escorts_the_carrier() {
        let bot = Bot::new("allies", true, 1);
        let mut v = re_view(ObjRole::Attack);
        let o = &mut v.re.as_mut().unwrap().objectives[0];
        (o.pickup, o.carried, o.carrier_at) = (None, true, Some([1000.0, 0.0, 64.0]));
        assert_eq!(bot.goal(&v), Goal::To([1000.0, 0.0, 64.0]));
        v.origin = [900.0, 0.0, 64.0];
        assert_eq!(bot.goal(&v), Goal::Hold);
    }

    #[test]
    fn use_is_pressed_only_at_the_objective() {
        for role in [ObjRole::Attack, ObjRole::Defend] {
            let mut bot = attacker();
            let mut v = sd_view(role);
            v.origin = [800.0, 0.0, 64.0];
            v.waypoint = Some([700.0, 0.0, 64.0]);
            assert_eq!(bot.goal(&v), Goal::To([0.0, 0.0, 64.0]), "{role:?}");
            for _ in 0..100 {
                let cmd = bot.think(&v);
                assert_eq!(cmd.buttons & BUTTON_USE, 0, "{role:?}");
                assert_eq!(cmd.forward, 127, "{role:?}");
                v.origin[0] -= 1.0;
            }
            // A guarding defender stands in its zone and presses nothing.
            if role == ObjRole::Defend {
                v.origin = [0.0, 0.0, 64.0];
                assert_eq!(bot.goal(&v), Goal::Hold);
                for _ in 0..100 {
                    let cmd = bot.think(&v);
                    assert_eq!((cmd.buttons, cmd.forward), (0, 0));
                }
            }
        }
        // A defender short of the bomb walks to it without a press.
        let mut bot = Bot::new("axis", true, 1);
        let mut v = sd_view(ObjRole::Defend);
        plant_bomb(&mut v, [500.0, 0.0, 64.0]);
        assert_eq!(bot.goal(&v), Goal::To([500.0, 0.0, 64.0]));
        assert_eq!(bot.think(&v).buttons & BUTTON_USE, 0);
    }

    #[test]
    fn an_allied_bot_answers_bels_team_menu_with_axis() {
        let mut bot = Bot::new("allies", false, 1);
        bot.observe(SID, &[(1, "v g_scriptMainMenu \"team_germanonly\"")], &all);
        let reply = bot.observe(SID, &[(2, "t 3")], &all);
        assert_eq!(reply, [format!("mr {SID} 3 axis")]);
    }

    fn bel_view(hunted: bool, markers: Vec<[f32; 3]>) -> BotView {
        let mut v = view();
        v.bel = Some(BelView {
            hunted,
            markers,
            trail: None,
        });
        v
    }

    /// A hunter heads for the nearest allied marker, looks round from it
    /// once there instead of standing on it, and a seen or heard enemy
    /// comes first.
    #[test]
    fn a_hunter_goes_for_the_nearest_marker_once() {
        let mut bot = Bot::new("axis", true, 1);
        let mut v = bel_view(false, vec![[2000.0, 0.0, 64.0], [800.0, 0.0, 64.0]]);
        assert_eq!(bot.goal(&v), Goal::To([800.0, 0.0, 64.0]));
        v.origin = [790.0, 10.0, 64.0];
        bot.think(&v);
        assert_eq!(
            bot.goal(&v),
            Goal::To([2000.0, 0.0, 64.0]),
            "the next marker"
        );
        v.bel.as_mut().unwrap().markers.pop();
        bot.think(&v);
        v.origin = [1990.0, 0.0, 64.0];
        bot.think(&v);
        assert_eq!(bot.goal(&v), Goal::Roam, "every marker stood at");
        // The marker moves on: a new one to go for.
        v.bel.as_mut().unwrap().markers = vec![[1500.0, 300.0, 64.0]];
        assert_eq!(bot.goal(&v), Goal::To([1500.0, 300.0, 64.0]));
        v.noise = Some([0.0, 900.0, 104.0]);
        assert_eq!(bot.goal(&v), Goal::To([0.0, 900.0, 104.0]), "a noise first");
    }

    /// The hunted run from what they last saw or heard, else from their own
    /// marker, and fight what they see.
    #[test]
    fn the_hunted_run_from_a_threat() {
        let mut bot = Bot::new("allies", true, 1);
        let mut v = bel_view(true, Vec::new());
        assert_eq!(bot.goal(&v), Goal::Roam);
        v.noise = Some([0.0, 900.0, 104.0]);
        assert_eq!(bot.goal(&v), Goal::Away([0.0, 900.0, 104.0]));
        bot.think(&v);
        v.noise = None;
        assert_eq!(bot.goal(&v), Goal::Away([0.0, 900.0, 104.0]), "remembered");
        // Nothing seen or heard: away from its own marker.
        let mut calm = bel_view(true, Vec::new());
        calm.bel.as_mut().unwrap().trail = Some([-300.0, 0.0, 64.0]);
        assert_eq!(
            Bot::new("allies", true, 1).goal(&calm),
            Goal::Away([-300.0, 0.0, 64.0])
        );
        v.enemy = Some(EnemyView {
            slot: 3,
            origin: [500.0, 0.0, 40.0],
            velocity: [0.0; 3],
        });
        assert_eq!(bot.goal(&v), Goal::To([500.0, 0.0, 40.0]));
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

    #[test]
    fn a_wandering_bot_runs_forward_carrying_the_held_weapon() {
        let mut bot = Bot::new("allies", false, 1);
        let cmd = bot.think(&view());
        assert_eq!(cmd.forward, 127, "the wander holds forward");
        assert_eq!(cmd.weapon, 10, "the cmd carries the held weapon byte");
        assert_eq!(cmd.buttons, 0, "nothing is pressed without an enemy");
        assert_eq!(cmd.right, 0);
    }

    #[test]
    fn a_stalled_heading_is_swapped_for_a_new_one() {
        let mut bot = Bot::new("allies", false, 1);
        let v = view();
        let first = bot.think(&v).angles[1];
        // Grinding into a wall: ten ticks without moving off the stall spot.
        for _ in 0..20 {
            bot.think(&v);
        }
        let after = bot.think(&v).angles[1];
        assert_ne!(first, after, "a bot that cannot move keeps one heading");
        // And the new heading eventually turns again too.
        for _ in 0..40 {
            bot.think(&v);
        }
        assert_ne!(after, bot.think(&v).angles[1], "the stall swap is not once");
    }

    #[test]
    fn the_goal_is_the_seen_enemy_else_anywhere() {
        let bot = Bot::new("allies", true, 1);
        let mut v = view();
        assert_eq!(bot.goal(&v), Goal::Roam);
        v.enemy = Some(EnemyView {
            slot: 3,
            origin: [500.0, 0.0, 40.0],
            velocity: [0.0; 3],
        });
        assert_eq!(bot.goal(&v), Goal::To([500.0, 0.0, 40.0]));
        v.dead = true;
        assert_eq!(bot.goal(&v), Goal::Hold);
    }

    /// A shooting bot that has just seen an enemy at `at`, moving at
    /// `velocity`, and then lost it.
    fn lost(at: [f32; 3], velocity: [f32; 3]) -> (Bot, BotView) {
        let mut bot = Bot::new("allies", true, 1);
        let mut v = view();
        v.enemy = Some(EnemyView {
            slot: 3,
            origin: at,
            velocity,
        });
        bot.think(&v);
        v.enemy = None;
        (bot, v)
    }

    #[test]
    fn a_lost_enemy_is_looked_for_where_it_was_headed_until_memory_runs_out() {
        let (mut bot, v) = lost([500.0, 0.0, 104.0], [200.0, 0.0, 0.0]);
        let spot = Goal::To([500.0 + 200.0 * MEMORY_LEAD_S, 0.0, 104.0]);
        assert_eq!(bot.goal(&v), spot);
        for _ in 1..bot.skill.memory_ticks {
            bot.think(&v);
        }
        assert_eq!(bot.goal(&v), spot, "forgotten early");
        bot.think(&v);
        assert_eq!(bot.goal(&v), Goal::Roam, "never forgotten");
    }

    #[test]
    fn reaching_the_remembered_spot_with_nothing_there_forgets_it() {
        let (mut bot, mut v) = lost([500.0, 0.0, 104.0], [0.0; 3]);
        v.origin = [470.0, 10.0, 64.0];
        assert_eq!(bot.goal(&v), Goal::Roam);
        bot.think(&v);
        v.origin = [0.0, 0.0, 64.0];
        assert_eq!(bot.goal(&v), Goal::Roam, "the reached spot came back");
    }

    #[test]
    fn a_heard_noise_is_a_goal_that_outlasts_the_tick() {
        let mut bot = Bot::new("allies", false, 1);
        let mut v = view();
        v.noise = Some([0.0, 900.0, 104.0]);
        assert_eq!(bot.goal(&v), Goal::To([0.0, 900.0, 104.0]));
        bot.think(&v);
        v.noise = None;
        assert_eq!(bot.goal(&v), Goal::To([0.0, 900.0, 104.0]));
    }

    #[test]
    fn a_seen_enemys_spot_beats_a_noise() {
        let (mut bot, mut v) = lost([500.0, 0.0, 104.0], [0.0; 3]);
        v.noise = Some([0.0, 900.0, 104.0]);
        assert_eq!(bot.goal(&v), Goal::To([500.0, 0.0, 104.0]));
        bot.think(&v);
        v.noise = None;
        assert_eq!(bot.goal(&v), Goal::To([500.0, 0.0, 104.0]));
        // Once the spot is reached, the noise is what is left to check.
        v.origin = [500.0, 0.0, 64.0];
        v.noise = Some([0.0, 900.0, 104.0]);
        bot.think(&v);
        v.noise = None;
        assert_eq!(bot.goal(&v), Goal::To([0.0, 900.0, 104.0]));
    }

    #[test]
    fn the_view_turns_toward_a_remembered_spot_while_the_keys_keep_the_path() {
        let (mut bot, mut v) = lost([0.0, 500.0, 124.0], [0.0; 3]);
        v.view = [0.0, 0.0, 0.0];
        v.waypoint = Some([500.0, 0.0, 64.0]);
        let cmd = bot.think(&v);
        let yaw = cmd.angles[1] as f32 / ANGLE2SHORT;
        assert!((yaw - bot.skill.turn_deg).abs() < 0.1, "yaw {yaw}");
        // The path runs along +x, right of a view turned 15 degrees left.
        assert!(cmd.forward > 120 && cmd.right > 30, "{cmd:?}");
    }

    #[test]
    fn the_loudest_noise_is_the_nearest_for_its_range_and_never_ones_own() {
        let at = [0.0, 0.0, 0.0];
        let noise = |x: f32, source, radius| Noise {
            at: [x, 0.0, 0.0],
            source,
            radius,
        };
        let heard = |n: &[Noise]| loudest(n, 1, at);
        assert_eq!(heard(&[noise(10.0, 1, HEAR_GUNFIRE)]), None, "own shot");
        assert_eq!(heard(&[noise(HEAR_BLAST + 1.0, 2, HEAR_BLAST)]), None);
        // 1800 of 2000 (0.90) is louder than 1400 of 1500 (0.93).
        let far_shot = noise(1800.0, 2, HEAR_GUNFIRE);
        let near_blast = noise(1400.0, 3, HEAR_BLAST);
        assert_eq!(heard(&[near_blast, far_shot]), Some(far_shot.at));
    }

    /// With no path a bot wanders on a random heading; it never takes one
    /// into a minefield, and turns off one it walks up to.
    #[test]
    fn a_wandering_bot_keeps_out_of_hazards() {
        for seed in 1..20 {
            let mut bot = Bot::new("allies", false, seed);
            let mut v = view();
            // Everywhere but -y (octant 6) is a minefield.
            v.hazard_ahead = [true; 8];
            v.hazard_ahead[6] = false;
            let cmd = bot.think(&v);
            assert_eq!(
                octant(bot.heading),
                6,
                "seed {seed}: heading {}",
                bot.heading
            );
            assert_eq!(cmd.forward, 127);
            // A hazard turns up in front: the next tick picks again.
            v.hazard_ahead = [false; 8];
            v.hazard_ahead[6] = true;
            bot.think(&v);
            assert_ne!(octant(bot.heading), 6, "seed {seed}: kept walking in");
        }
    }

    #[test]
    fn octants_run_counter_clockwise_from_plus_x() {
        assert_eq!(octant(0.0), 0);
        assert_eq!(octant(22.0), 0);
        assert_eq!(octant(23.0), 1);
        assert_eq!(octant(90.0), 2);
        assert_eq!(octant(-90.0), 6);
        assert_eq!(octant(350.0), 0);
    }

    #[test]
    fn a_bot_runs_at_its_waypoint() {
        let mut bot = Bot::new("allies", false, 1);
        let mut v = view();
        v.waypoint = Some([0.0, 100.0, 64.0]);
        let cmd = bot.think(&v);
        assert_eq!(cmd.forward, 127);
        assert_eq!(cmd.angles, [0, deg_short(90.0), 0], "level, facing +y");
    }

    /// Pinned at a ledge's face on a jump edge: jump, release the key the
    /// next tick (a held key never jumps again), then press again.
    #[test]
    fn a_bot_jumps_a_jump_edge_and_releases_the_key() {
        let mut bot = Bot::new("allies", false, 1);
        let mut v = view();
        v.waypoint = Some([30.0, 0.0, 96.0]);
        assert_eq!(bot.think(&v).up, 0, "a plain edge");
        v.jump = true;
        let ups: Vec<i8> = (0..3).map(|_| bot.think(&v).up).collect();
        assert_eq!(ups, [127, 0, 127]);
        // Still running at the ledge: no jump yet.
        v.velocity = [190.0, 0.0, 0.0];
        v.speed = 190.0;
        assert_eq!(bot.think(&v).up, 0);
    }

    /// At a leap's foot the run slows and stops on it; the leap itself
    /// jumps only at the lip.
    #[test]
    fn a_bot_stops_at_a_leaps_foot_and_leaps_at_the_lip() {
        let mut bot = Bot::new("allies", false, 1);
        let mut v = view();
        v.stop = true;
        v.waypoint = Some([100.0, 0.0, 64.0]);
        assert_eq!(bot.think(&v).forward, 127);
        v.waypoint = Some([24.0, 0.0, 64.0]);
        assert_eq!(bot.think(&v).forward, 63);
        v.waypoint = Some([4.0, 0.0, 64.0]);
        assert_eq!(bot.think(&v).forward, 0);
        v.stop = false;
        v.jump = true;
        v.leap = true;
        v.waypoint = Some([60.0, 0.0, 96.0]);
        assert_eq!(bot.think(&v).up, 0, "at rest, no lip");
        v.lip = true;
        assert_eq!(bot.think(&v).up, 127);
    }

    #[test]
    fn a_waypoint_up_a_ladder_looks_up_and_one_below_is_backed_onto() {
        let mut bot = Bot::new("allies", false, 1);
        let mut v = view();
        v.waypoint = Some([32.0, 0.0, 200.0]);
        let up = bot.think(&v);
        assert_eq!(
            (up.forward, up.angles[0]),
            (127, deg_short(-crate::nav::LADDER_PITCH))
        );
        v.waypoint = Some([32.0, 0.0, -100.0]);
        let down = bot.think(&v);
        assert_eq!(down.forward, -127, "backs off the ledge");
        assert_eq!(
            down.angles[1],
            deg_short(-180.0),
            "facing away from the waypoint"
        );
    }

    #[test]
    fn a_climb_looks_up_to_the_top_and_a_descent_backs_all_the_way_down() {
        let mut bot = Bot::new("allies", false, 1);
        let mut v = view();
        v.on_ladder = true;
        // Near the top: the head is only 20 above, still looking up.
        v.waypoint = Some([16.0, 0.0, 84.0]);
        let up = bot.think(&v);
        assert_eq!(
            (up.forward, up.angles[0]),
            (127, deg_short(-crate::nav::LADDER_PITCH))
        );
        // Near the bottom of a descent: the foot is 20 below, still backing.
        v.waypoint = Some([16.0, 0.0, 44.0]);
        assert_eq!(bot.think(&v).forward, -127);
    }

    #[test]
    fn a_leaps_foot_at_a_ladders_head_is_climbed_to_at_full_rate() {
        let mut bot = Bot::new("allies", false, 1);
        let mut v = view();
        v.stop = true;
        v.on_ladder = true;
        v.waypoint = Some([4.0, 0.0, 600.0]);
        let climb = bot.think(&v);
        assert_eq!(
            (climb.forward, climb.angles[0]),
            (127, deg_short(-crate::nav::LADDER_PITCH))
        );
    }

    /// Up a ladder the climb faces square into it, whatever the waypoint's
    /// bearing, and strafes toward the middle of the rungs.
    #[test]
    fn a_climb_faces_the_ladder_and_strafes_to_its_middle() {
        let mut bot = Bot::new("allies", false, 1);
        let mut v = view();
        v.on_ladder = true;
        v.ladder_normal = [-1.0, 0.0, 0.0];
        v.waypoint = Some([40.0, -60.0, 300.0]);
        v.ladder_middle = Some([v.origin[0], v.origin[1] - 8.0]);
        let cmd = bot.think(&v);
        assert_eq!(cmd.angles[1], deg_short(0.0), "square into the face");
        assert_eq!(cmd.forward, 127);
        assert!(
            cmd.right > 0,
            "facing +x, the middle at -y is to the right: {}",
            cmd.right
        );
        v.ladder_middle = Some([v.origin[0], v.origin[1]]);
        assert_eq!(bot.think(&v).right, 0, "centred");
    }

    #[test]
    fn a_bot_on_a_ladder_with_no_waypoint_or_pinned_climbs_down() {
        let mut bot = Bot::new("allies", false, 1);
        let mut v = view();
        v.on_ladder = true;
        let down = (127, deg_short(crate::nav::LADDER_PITCH));
        let cmd = bot.think(&v);
        assert_eq!((cmd.forward, cmd.angles[0]), down, "no waypoint");
        // Pinned under something on the way up: the unstick goes down too.
        v.waypoint = Some([16.0, 0.0, 200.0]);
        let unstuck = (0..25)
            .map(|_| bot.think(&v))
            .any(|c| (c.forward, c.angles[0]) == down);
        assert!(unstuck, "a pinned climber kept pushing up");
    }

    #[test]
    fn a_bot_creeps_backward_over_a_lip_and_keeps_facing_the_ladder() {
        let mut bot = Bot::new("allies", false, 1);
        let mut v = view();
        // The foot is 300 below, 20 ahead in +x: back toward it facing -x.
        v.waypoint = Some([20.0, 0.0, -236.0]);
        let first = bot.think(&v);
        assert_eq!(first.forward, -crate::nav::CREEP_KEY);
        assert_eq!(first.angles[1], deg_short(-180.0));
        // Over the lip and past the foot's column: same heading.
        v.origin[0] = 21.0;
        let past = bot.think(&v);
        assert_eq!(
            (past.forward, past.angles[1]),
            (first.forward, first.angles[1])
        );
    }

    #[test]
    fn a_stuck_bot_leaves_its_waypoint_for_a_spell() {
        let mut bot = Bot::new("allies", false, 1);
        let mut v = view();
        v.waypoint = Some([0.0, 100.0, 64.0]);
        let toward = bot.think(&v).angles[1];
        // Pinned: the origin never moves, and the stall reads within two
        // ten-tick windows.
        let turned = (0..25).any(|_| bot.think(&v).angles[1] != toward);
        assert!(turned, "a pinned bot kept pushing at its waypoint");
        // The random heading gets it moving again.
        for _ in 0..UNSTICK_TICKS {
            v.origin[0] += 5.0;
            v.waypoint = Some([v.origin[0], 100.0, 64.0]);
            bot.think(&v);
        }
        assert_eq!(bot.think(&v).angles[1], toward, "and goes back to it after");
    }

    /// A bot pinned by a deck's edge takes its unstick heading along the
    /// deck, never off it, and turns off a drop it walks up to. Seed 5 on
    /// mp_ship walked one off the deck at (3288, -460, 56) to the hull.
    #[test]
    fn a_stuck_bot_unsticks_away_from_a_drop() {
        for seed in 1..20 {
            let mut bot = Bot::new("allies", false, seed);
            let mut v = view();
            v.waypoint = Some([0.0, 100.0, 64.0]);
            // A drop everywhere but +y (octant 2), a minefield at -y.
            v.drop_ahead = [Some(100.0); 8];
            v.drop_ahead[2] = None;
            v.hazard_ahead[6] = true;
            let unstuck = (0..25).find(|_| {
                bot.think(&v);
                bot.unsticking()
            });
            assert!(unstuck.is_some(), "seed {seed}: never unstuck");
            assert_eq!(octant(bot.heading), 2, "seed {seed}: {}", bot.heading);
            // Boxed in by drops: off the hazard still.
            v.drop_ahead = [Some(100.0); 8];
            bot.pick_heading(&v);
            assert_ne!(octant(bot.heading), 6, "seed {seed}: into the minefield");
            // A drop turns up ahead mid-spell: the next tick picks again.
            v.drop_ahead = [None; 8];
            v.hazard_ahead = [false; 8];
            bot.heading = 90.0;
            v.drop_ahead[2] = Some(100.0);
            bot.think(&v);
            assert_ne!(octant(bot.heading), 2, "seed {seed}: kept walking off");
        }
    }

    /// Stuck spell after spell in one spot with only drops open (mp_ship's
    /// rim at (4223, 345, 276), under a ramp), a bot is cornered and takes
    /// a drop out.
    #[test]
    fn a_cornered_bot_takes_a_drop_out() {
        let mut dropped = 0;
        for seed in 1..20 {
            let mut bot = Bot::new("allies", false, seed);
            let mut v = view();
            v.waypoint = Some([0.0, 100.0, 64.0]);
            // +y (octant 2) is the wall it is pinned against.
            v.drop_ahead = [Some(100.0); 8];
            v.drop_ahead[2] = None;
            let mut spells = Vec::new();
            for _ in 0..200 {
                bot.think(&v);
                // A spell starts at UNSTICK_TICKS and its first tick spends one.
                if bot.unstick_ticks == UNSTICK_TICKS - 1 {
                    spells.push(octant(bot.heading));
                }
            }
            let first = CORNERED as usize;
            assert!(spells.len() > first, "seed {seed}: {spells:?}");
            assert!(
                spells[..first].iter().all(|o| *o == 2),
                "seed {seed}: {spells:?}"
            );
            dropped += usize::from(spells[first..].iter().any(|o| *o != 2));
        }
        assert!(dropped > 10, "only {dropped} of 19 seeds took a drop out");
        // A drop that hurts stays off even cornered.
        for seed in 1..20 {
            let mut bot = Bot::new("allies", false, seed);
            let mut v = view();
            v.waypoint = Some([0.0, 100.0, 64.0]);
            v.drop_ahead = [Some(f32::INFINITY); 8];
            v.drop_ahead[2] = None;
            for _ in 0..200 {
                bot.think(&v);
                if bot.unstick_ticks == UNSTICK_TICKS - 1 {
                    assert_eq!(octant(bot.heading), 2, "seed {seed}: off a deep drop");
                }
            }
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

    #[test]
    fn a_new_target_holds_fire_through_the_reaction_delay() {
        let (_, v) = engaged(7);
        let mut bot = Bot::new("allies", true, 7);
        // Settled aim from the start: without the delay the first tick fires.
        bot.skill.error_start = bot.skill.error_floor;
        let react = bot.skill.reaction_min as usize;
        for tick in 0..react {
            let cmd = bot.think(&v);
            assert_eq!(cmd.buttons & BUTTON_ATTACK, 0, "fired on tick {tick}");
        }
        let fired = (0..40).any(|_| bot.think(&v).buttons & BUTTON_ATTACK != 0);
        assert!(fired, "never fired once the delay ran out");
        // A switch to another enemy restarts the delay.
        let e = v.enemy.unwrap();
        let mut other = v;
        other.enemy = Some(EnemyView { slot: 4, ..e });
        bot.fire_cooldown = 0;
        bot.burst_ticks = 0;
        for tick in 0..react {
            let cmd = bot.think(&other);
            assert_eq!(cmd.buttons & BUTTON_ATTACK, 0, "fired on tick {tick}");
        }
    }

    #[test]
    fn the_view_turns_a_bounded_step_per_tick() {
        let (mut bot, mut v) = engaged(3);
        // The enemy is behind: yaw 180 from a view at 0.
        v.enemy = Some(EnemyView {
            slot: 3,
            origin: [-500.0, 0.0, 124.0],
            velocity: [0.0; 3],
        });
        let step = bot.skill.turn_deg;
        let mut yaw = 0.0f32;
        for _ in 0..30 {
            v.view = [0.0, yaw, 0.0];
            let cmd = bot.think(&v);
            let next = cmd.angles[1] as f32 / ANGLE2SHORT;
            assert!(
                yaw_diff(next, yaw).abs() <= step + 0.01,
                "turned {} in one tick",
                yaw_diff(next, yaw)
            );
            yaw = next;
        }
        assert!(yaw_diff(180.0, yaw).abs() < 5.0, "never came round: {yaw}");
    }

    #[test]
    fn the_aim_error_shrinks_on_a_held_target() {
        let (mut bot, v) = engaged(5);
        bot.think(&v);
        let first = bot.target.as_ref().unwrap().error;
        for _ in 0..40 {
            bot.think(&v);
        }
        let later = bot.target.as_ref().unwrap().error;
        assert!(later < first / 4.0, "{first} -> {later}");
        assert_eq!(later, bot.skill.error_floor);
    }

    #[test]
    fn an_automatic_holds_the_trigger_and_a_semi_auto_taps() {
        let (mut bot, mut v) = engaged(9);
        v.automatic = true;
        let held: Vec<bool> = (0..40)
            .map(|_| bot.think(&v).buttons & BUTTON_ATTACK != 0)
            .collect();
        assert!(
            held.windows(2).any(|w| w[0] && w[1]),
            "an automatic never held across two ticks"
        );

        let (mut bot, v) = engaged(9);
        let mut last = None;
        let mut count = 0;
        let gap = (v.fire_time_ms / FRAME_MS).max(2);
        for tick in 0..60 {
            if bot.think(&v).buttons & BUTTON_ATTACK != 0 {
                if let Some(l) = last {
                    assert!(tick - l >= gap, "taps {l} and {tick} too close");
                }
                last = Some(tick);
                count += 1;
            }
        }
        assert!(count >= 3, "a semi-auto tapped {count} times in 3 s");
    }

    #[test]
    fn sights_go_up_at_range_and_hold_fire_until_they_are() {
        let (mut bot, mut v) = engaged(11);
        v.enemy = Some(EnemyView {
            slot: 3,
            origin: [900.0, 0.0, 124.0],
            velocity: [0.0; 3],
        });
        for _ in 0..40 {
            let cmd = bot.think(&v);
            assert!(cmd.buttons & BUTTON_ADS != 0, "no sights at 900 units");
            assert_eq!(cmd.buttons & BUTTON_ATTACK, 0, "fired from the hip");
            assert_eq!(cmd.right, 0, "strafed while sighted");
        }
        v.ads_frac = 1.0;
        assert!((0..40).any(|_| bot.think(&v).buttons & BUTTON_ATTACK != 0));
    }

    #[test]
    fn an_engaging_bot_strafes() {
        let (mut bot, v) = engaged(13);
        let rights: Vec<i8> = (0..80).map(|_| bot.think(&v).right).collect();
        assert!(
            rights.contains(&127) && rights.contains(&-127),
            "{rights:?}"
        );
    }

    #[test]
    fn a_dry_primary_draws_the_pistol_against_a_close_enemy() {
        let (mut bot, mut v) = engaged(21);
        v.pistol = Some(2);
        v.weapons_held |= 1 << 2;
        v.loaded |= 1 << 2;
        v.clip = 0;
        let cmd = bot.think(&v);
        assert_eq!(cmd.weapon, 2, "kept the dry rifle up");
        assert_eq!(cmd.wbuttons & WBUTTON_RELOAD, 0, "reloaded instead");
        // The byte rides every cmd until `ps.weapon` reads it.
        assert_eq!(bot.think(&v).weapon, 2);
        v.weapon = 2;
        v.clip = 7;
        assert_eq!(bot.think(&v).weapon, 2);
        // Nobody in sight for a while: the rifle goes back up.
        v.enemy = None;
        let back = (0..HOLSTER_TICKS + 1).any(|_| bot.think(&v).weapon == 10);
        assert!(back, "the pistol stayed out");
    }

    /// A reload quicker than the draw (a Garand's 1.6 s against a slow
    /// putaway) keeps the primary up even point blank.
    #[test]
    fn a_quick_reload_beats_a_slow_draw() {
        let (mut bot, mut v) = engaged(21);
        v.pistol = Some(2);
        v.weapons_held |= 1 << 2;
        v.loaded |= 1 << 2;
        v.clip = 0;
        (v.reload_ms, v.draw_ms) = (700, 750);
        let cmd = bot.think(&v);
        assert_eq!(cmd.weapon, 10);
        assert!(cmd.wbuttons & WBUTTON_RELOAD != 0);
    }

    /// A primary with nothing left goes away for the pistol with nobody in
    /// sight, presses no reload, and is not drawn again while it stays
    /// empty; a spent pistol hands back to a primary that has rounds.
    #[test]
    fn an_empty_primary_is_swapped_and_not_redrawn() {
        let mut bot = Bot::new("allies", true, 1);
        let mut v = view();
        v.pistol = Some(2);
        v.weapons_held |= 1 << 2;
        (v.clip, v.loaded) = (0, 1 << 2);
        let cmd = bot.think(&v);
        assert_eq!(cmd.weapon, 2);
        assert_eq!(cmd.wbuttons & WBUTTON_RELOAD, 0, "reloaded an empty gun");
        (v.weapon, v.clip) = (2, 7);
        for _ in 0..HOLSTER_TICKS * 2 {
            assert_eq!(bot.think(&v).weapon, 2, "the empty primary came back");
        }
        // A pickup refills the primary: the calm spell hands it back.
        v.loaded |= 1 << 10;
        assert_eq!(bot.think(&v).weapon, 10);
        // The pistol runs out mid-fight with the primary loaded: back at
        // once.
        let (mut bot, mut v) = engaged(21);
        v.pistol = Some(2);
        v.weapons_held |= 1 << 2;
        v.loaded |= 1 << 2;
        v.clip = 0;
        assert_eq!(bot.think(&v).weapon, 2);
        (v.weapon, v.clip, v.loaded) = (2, 0, 1 << 10);
        assert_eq!(bot.think(&v).weapon, 10, "kept the spent pistol up");
    }

    /// A switch whose weapon leaves the kit before it lands stops riding
    /// the cmds; the byte would otherwise read as a holster for good.
    #[test]
    fn a_switch_to_a_weapon_no_longer_held_is_dropped() {
        let (mut bot, mut v) = engaged(21);
        v.pistol = Some(2);
        v.weapons_held |= 1 << 2;
        v.loaded |= 1 << 2;
        v.clip = 0;
        assert_eq!(bot.think(&v).weapon, 2);
        v.pistol = None;
        v.weapons_held &= !(1 << 2);
        v.loaded &= !(1 << 2);
        assert_eq!(bot.think(&v).weapon, 10);
    }

    #[test]
    fn a_dry_primary_at_range_reloads_instead() {
        let (mut bot, mut v) = engaged(21);
        v.pistol = Some(2);
        v.clip = 0;
        v.enemy = Some(EnemyView {
            slot: 3,
            origin: [900.0, 0.0, 124.0],
            velocity: [0.0; 3],
        });
        let cmd = bot.think(&v);
        assert_eq!(cmd.weapon, 10);
        assert!(cmd.wbuttons & WBUTTON_RELOAD != 0);
    }

    #[test]
    fn a_scope_stays_down_at_point_blank() {
        let (mut bot, mut v) = engaged(23);
        v.sniper = true;
        for _ in 0..20 {
            assert_eq!(bot.think(&v).buttons & BUTTON_ADS, 0, "scoped at 100");
        }
        v.enemy = Some(EnemyView {
            slot: 3,
            origin: [900.0, 0.0, 124.0],
            velocity: [0.0; 3],
        });
        assert!(bot.think(&v).buttons & BUTTON_ADS != 0, "no scope at 900");
    }

    #[test]
    fn an_empty_clip_is_reloaded() {
        let mut bot = Bot::new("allies", true, 1);
        let mut v = view();
        v.clip = 0;
        let cmd = bot.think(&v);
        assert!(cmd.wbuttons & WBUTTON_RELOAD != 0, "a dry clip reloads");
    }

    #[test]
    fn a_dead_bot_presses_use_after_a_beat() {
        let mut bot = Bot::new("allies", false, 1);
        let mut v = view();
        v.dead = true;
        v.playing = false;
        for _ in 0..RESPAWN_DELAY + 2 {
            let cmd = bot.think(&v);
            if cmd.buttons & BUTTON_USE != 0 {
                return;
            }
        }
        panic!("a dead bot never pressed use");
    }

    #[test]
    fn a_bot_throw_switches_to_the_frag_cooks_and_throws() {
        let mut bot = Bot::new("allies", true, 0x1234_5678);
        let mut v = view();
        v.enemy = Some(EnemyView {
            slot: 3,
            origin: [300.0, 0.0, 40.0],
            velocity: [0.0; 3],
        });
        // Grind until the grenade machine starts; the cooldown is short with
        // a live enemy this close.
        let mut switched = None;
        let mut held = false;
        let mut released = false;
        for _ in 0..600 {
            let cmd = bot.think(&v);
            // The real sim follows the byte; the fake view does the same.
            v.weapon = cmd.weapon;
            if cmd.weapon == 6 {
                switched = Some(cmd.weapon);
                held |= cmd.buttons & BUTTON_ATTACK != 0;
            }
            if held && cmd.weapon == 6 && cmd.buttons & BUTTON_ATTACK == 0 {
                released = true;
            }
        }
        assert!(switched.is_some(), "the bot never switched to the frag");
        assert!(held, "the pullback never held the trigger");
        assert!(released, "the cooked throw never released");
    }

    /// A death mid-grenade-phase must not strand the corpse: the stage
    /// machine must not swallow the respawn press.
    #[test]
    fn a_bot_killed_mid_grenade_phase_still_presses_use() {
        for start_stage in 0..3 {
            let mut bot = Bot::new("allies", true, 1);
            // Walk the machine into each non-wander stage by force.
            match start_stage {
                0 => {
                    bot.rifle = 10;
                    bot.stage = Stage::ToGrenade;
                }
                1 => {
                    bot.rifle = 10;
                    bot.stage = Stage::Cook { left: 5 };
                }
                _ => {
                    bot.stage = Stage::BackToRifle { weapon: 10 };
                }
            }
            let mut v = view();
            v.dead = true;
            v.playing = false;
            let mut pressed = false;
            for _ in 0..RESPAWN_DELAY + RESPAWN_RETRY * 3 {
                if bot.think(&v).buttons & BUTTON_USE != 0 {
                    pressed = true;
                    break;
                }
            }
            assert!(
                pressed,
                "stage {start_stage}: a bot killed mid-grenade-phase never pressed use"
            );
        }
    }
}
