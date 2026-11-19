//! Server-side bots: synthetic clients in `Server::clients` whose brains
//! live here. A bot joins through the stock menus like any client (the
//! driver in `server.rs` feeds this module the reliable server commands and
//! sends back the `mr` replies), then sends real usercmds through pmove.
//!
//! The brain is pure: [`Bot::observe`] consumes server commands and returns
//! `mr` replies, [`Bot::think`] returns the tick's usercmd from a read-only
//! [`BotView`] the server fills. Everything wire- and VM-shaped stays in
//! `server.rs`.

pub mod cod2;
pub mod cod4;
pub(crate) mod cover;
pub(crate) mod driver;
pub use driver::with_threads;
pub(crate) mod hunt;
pub(crate) mod posts;

use crate::server::FRAME_MS;
pub mod team;
use vcod_common::net::msg::{self, NULL_USERCMD, UserCmd};
pub use vehicle::{ArmorView, Crowd, GunSight, SeatView, VehicleSpot};
pub(crate) use vehicle::{
    GUN_RANGE as VEHICLE_GUN_RANGE, ROCKET_CLEAR_MARGIN, ROCKET_MIN as VEHICLE_ROCKET_MIN,
    ROCKET_RANGE as VEHICLE_ROCKET_RANGE, SEEK_RANGE as VEHICLE_SEEK_RANGE,
};

mod vehicle;

mod aim;
pub(crate) mod retreat;
mod skill;
pub(crate) mod yell;
pub use skill::{DEFAULT_LEVEL, MAX_LEVEL};

mod fight;
mod grenade;
mod menus;
mod objective;
mod sight;
mod view;
mod walk;
pub(crate) use fight::HURT_MS;
use fight::{PITCH_MOST, Target};
pub(crate) use grenade::{
    COOK_MEAN_MS, COOK_MOST_MS, LOB_BODY_GROW, LOB_MISS, LOB_REACH_SLACK, LOB_SLACK, MATE_LEAD_S,
};
use grenade::{GRENADE_MIN, GRENADE_RANGE, Stage, ThrowFor};
use menus::WEAPON_MENUS;
pub(crate) use menus::{WEAPON_MENUS_UO, WeaponMenu, menu_weapons};
pub use objective::{BREACH_CLEAR, BREACH_MIN, BREACH_NEAR, BREACH_RANGE};
use objective::{
    CAREFUL_CROUCH, DRY_TICKS, ObjTarget, Objective, PLAYER_HEIGHT, SHORT_GAIN, SHORT_TICKS,
    SHUN_TICKS, SITE_MARGIN,
};
pub(crate) use objective::{GuardHold, RADIO_HEIGHT, RADIO_HEIGHT_COD2, RADIO_MARGIN, inside_site};
use sight::Recall;
pub(crate) use sight::{
    HEAR_BLAST, HEAR_GUNFIRE, WRECK_REACH, aim_point, chest, friend_in_line, held_sidestep,
    in_sight_cone, loudest, notices_frag, pick_enemy, wreck_near,
};
pub(crate) use view::Breach;
pub use view::{
    BasBaseView, BasView, BelView, BombView, BotBody, BotView, CtfFlagView, CtfView, DomFlagView,
    DomView, EnemyView, GuardPost, HqRadioView, HqView, Hurt, KeyBomb, Noise, ObjRole, ReObjView,
    ReView, SdView, SiteView, UoView,
};
#[cfg(test)]
pub(crate) use walk::along_face;
pub use walk::{BACK_DOWN_HEIGHT, HAZARD_LOOK, octant};
pub(crate) use walk::{FLEE_HEIGHT, FLEE_REACH, rekey};

/// Hip-shot range; beyond it a tap is noise anyway.
pub(crate) const SHOOT_RANGE: f32 = 1500.0;
/// Ticks a dead bot waits before its first use press.
const RESPAWN_DELAY: u32 = 30;
/// Ticks between use-press retries, and the press length: two ticks down,
/// then 1 s of release, repeated until the script's poll catches one.
const RESPAWN_RETRY: u32 = 20;
/// The run speed at which a moving bot's error takes all of
/// [`Skill::error_moving`]; slower, it takes that share of it.
const RUN_SPEED: f32 = 190.0;
/// A remembered or heard spot is reached within this, horizontally.
const ARRIVE_RADIUS: f32 = 64.0;
/// ...and this vertically, so a spot one floor up is not reached from below.
const ARRIVE_HEIGHT: f32 = 96.0;

/// How well a bot fights. Ticks are 50 ms; the aim error is a miss distance
/// at the target, in units, so its angle shrinks with range on its own.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Skill {
    /// Ticks from first sight of a target to the first turn toward it:
    /// `reaction_min + rand(reaction_spread)`.
    pub reaction_min: u32,
    pub reaction_spread: u32,
    /// Ticks out of sight after which a returning target counts as new.
    pub forget_ticks: u32,
    /// The most a look turns the view in one tick (`Bot::turn_toward`),
    /// degrees; on a target, a flick's mean a tick, 1.875 times it at its
    /// peak ([`aim::Hand::steer`]).
    pub turn_deg: f32,
    /// Aim error on acquisition; each tick on the target multiplies it by
    /// `error_decay`, down to `error_floor`.
    pub error_start: f32,
    pub error_floor: f32,
    pub error_decay: f32,
    /// The range at which the error doubles.
    pub error_range: f32,
    /// Error multiplier while the bot itself moves at run speed; slower,
    /// a share of it ([`RUN_SPEED`]).
    pub error_moving: f32,
    /// Seconds the aim trails a target moving across the line: its speed
    /// across times this adds to the miss, in units.
    pub track_lag: f32,
    /// A flick's landing spread, a share of its angle (bot-combat.md 18).
    pub flick_spread: f32,
    /// The share of the gun's sway and recoil the view takes back, and
    /// how many ticks late.
    pub recoil_share: f32,
    pub recoil_ticks: u32,
    /// The chance a footwork spell on the sights moves (a slow strafe, a
    /// bolt rifle's back-off); a quarter of it lies down at range.
    pub footwork: f32,
    /// Past this range a weapon with sights aims down them.
    pub ads_range: f32,
    /// Ticks a lost enemy's last spot stays a goal. Longer than
    /// `forget_ticks`: the spot outlives the target's identity.
    pub memory_ticks: u32,
    /// Ticks a heard noise stays a goal; long enough to walk most of
    /// [`HEAR_GUNFIRE`] at run speed.
    pub noise_ticks: u32,
    /// Scales how far a noise carries to this bot ([`loudest`]).
    pub hearing: f32,
    /// The sight cone a new target is acquired in, degrees across, about
    /// the view. A tracked target, the attacker of a fresh hit
    /// ([`BotView::hurt`]) and what the bot hears are known outside it.
    pub fov_deg: f32,
    /// The most the walking view turns in one tick, degrees: the keys
    /// keep the path while it comes round ([`Bot::walk_turn`]).
    pub walk_turn_deg: f32,
}

impl Default for Skill {
    fn default() -> Self {
        Skill {
            reaction_min: 4,
            reaction_spread: 4,
            forget_ticks: 20,
            turn_deg: 15.0,
            error_start: 36.0,
            error_floor: 4.0,
            error_decay: 0.85,
            error_range: 1000.0,
            error_moving: 1.75,
            track_lag: 0.06,
            flick_spread: 0.1,
            recoil_share: 0.7,
            recoil_ticks: 3,
            footwork: 0.45,
            ads_range: 400.0,
            memory_ticks: 100,
            noise_ticks: 200,
            hearing: 1.0,
            fov_deg: 120.0,
            walk_turn_deg: 24.0,
        }
    }
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

pub struct Bot {
    pub name: String,
    shoot: bool,
    pub skill: Skill,
    /// Fighting from cover ([`cover`]).
    pub cover: cover::Cover,
    /// Shares out an objective round with its team ([`team`]); off, it
    /// plays as before team play did (a `bot_match` measuring arm).
    pub team_play: bool,
    /// With [`Self::team_play`], keeps apart from its team on an objective
    /// level: its own route noise, a walk behind a teammate just ahead,
    /// a staggered round start and the S&D attack's flank and rear
    /// ([`team::Squad`], bot-objectives.md 9.1); off, it moves as before
    /// (a `bot_match` measuring arm).
    pub squad: bool,
    /// With [`Self::team_play`], plays its objective by posts: guards of
    /// one objective spread over posts with cover round it and move between
    /// them ([`posts`], bot-objectives.md 13); off, it guards from the
    /// first spot in reach as before (a `bot_match` measuring arm).
    pub tactics: bool,
    /// With [`Self::team_play`], an S&D defender goes to the site an enemy
    /// was seen near ([`SdView::hot`]); off, it keeps its own site (a
    /// `bot_match` measuring arm).
    pub converge: bool,
    /// With [`Self::team_play`], walks to its objective by the way in the
    /// driver dealt it ([`BotView::lane`]); off, straight there as before
    /// (a `bot_match` measuring arm).
    pub lanes: bool,
    /// With [`Self::tactics`], guards an objective from the ways in, with
    /// at most one teammate near it, and leaves a post it was seen or shot
    /// on ([`GuardHold::Far`], bot-objectives.md 14); off, round the
    /// objective as section 13 (a `bot_match` measuring arm).
    pub far: bool,
    /// With [`Self::far`], a share of the guards walks back to the
    /// objective with care on a cue (an enemy seen or heard near it, a
    /// teammate killed there, an enemy on it), S&D defenders make for a
    /// planted bomb with care, and on `ctf` the nearest teammates go for
    /// the enemy carrying their flag when one of them sees him
    /// (bot-objectives.md 14); off, they hold (a `bot_match` arm).
    pub cues: bool,
    /// With [`Self::squad`], walks farther behind a teammate ahead and
    /// takes a way in from nearer its objective (bot-objectives.md 14);
    /// off, as section 9.1 (a `bot_match` arm).
    pub travel: bool,
    /// A `ctf` carrier keeps for its goal through a fight, and while its
    /// own flag is away returns it from near or waits in cover near the
    /// goal (bot-objectives.md 14.11); off, it closes on what it sees and
    /// stands in the goal (a `bot_match` arm).
    pub ctf_carry: bool,
    /// bot-objectives.md 14.12's experiments on a `ctf` carrier's run
    /// home, off by default (none raised captures past the seed spread),
    /// each switched on by its `bot_match` arm. `ctf_escort`: the
    /// teammates nearest the carrier, and one of his flag's holders, walk
    /// one ahead along his path and the rest behind his shoulders.
    pub ctf_escort: bool,
    /// The carrier plans round where his side saw enemies of late and
    /// round the outposts by the flag he took.
    pub ctf_route: bool,
    /// The carrier holds in cover from an enemy in sight ahead of him.
    pub ctf_hold: bool,
    /// On United Offensive the carrier sprints on from an enemy in sight
    /// behind or beside him instead of turning the gun on him.
    pub ctf_sprint: bool,
    /// bot-objectives.md 14.13's experiments on a `ctf` flag's guards
    /// once the flag is taken, off by default (none raised captures past
    /// the seed spread). `ctf_watch`: a guard near the flag's home holds
    /// the outpost nearest it that sees the carrier's way out, looking at
    /// that way, instead of making for his goal.
    pub ctf_watch: bool,
    /// The guards near the flag's home close on the carrier along his way
    /// out instead of making for his goal.
    pub ctf_close: bool,
    /// The guards of a `ctf` flag at home hold outposts from a band
    /// nearer it ([`posts::CTF_NEAR_BAND`]).
    pub ctf_near: bool,
    /// The point its objective centres on, read at the top of the last
    /// think ([`Bot::objective_point`]).
    anchor: Option<[f32; 3]>,
    /// The point it guarded at the last think, a guard on its post or in
    /// its ring only ([`Bot::guard_point`]).
    guarded: Option<[f32; 3]>,
    /// [`BotView::exposure`] at the last think.
    exposure: Option<f32>,
    /// Ticks left on the hold a one-life round starts with
    /// ([`team::START_HOLD`]).
    start_hold: u32,
    /// Moves while it fights: strafes on the sights, backs off a close
    /// enemy with a bolt rifle, lies down at range ([`Bot::footwork`]);
    /// off, it stands still on the sights as before (a `bot_match` arm).
    pub footwork: bool,
    /// Knows who shot it and keeps its target ([`pick_enemy`],
    /// [`Bot::track`], [`Bot::face_hurt`]) and sees in a cone
    /// ([`Skill::fov_deg`]); off, it fights the nearest enemy in sight all
    /// round, as before (a `bot_match` measuring arm).
    pub aware: bool,
    /// Varies its peeks from cover and comes at a lost enemy's spot from a
    /// vantage over it ([`cover`], [`hunt`]); off, it peeks the one way
    /// and walks onto the spot, as before (a `bot_match` measuring arm).
    pub wary: bool,
    /// Aims as a hand does ([`aim`], bot-combat.md 18); off, the old aim:
    /// straight turns, a jittered error, every sway and kick taken back at
    /// once (a `bot_match` measuring arm).
    pub human_aim: bool,
    /// Thinks run, so the aim knows a want from last tick.
    clock: u32,
    /// The gun's offsets ([`BotView::gun_offset`]), newest first.
    gun_kept: [[f32; 2]; aim::RECOIL_KEEP],
    /// Hunts that reached the spot or its vantage (the `bot_match` column).
    hunts: u32,
    /// The team the team menu is answered with.
    team: String,
    /// The menu retail last named in `v g_scriptMainMenu`; the `t` that
    /// follows opens it.
    main_menu: String,
    /// Menu indices already answered, so a reopened menu is not answered
    /// twice (the same bookkeeping `JoinProbe` keeps).
    answered: Vec<i32>,
    // The tick's usercmd is stamped with the server's own clock; the driver
    // stamps it, so the brain carries only the input state below.
    /// The bot's own generator: its brain's draws and its path's
    /// (`nav::Follower::waypoint`), never the server's shared `rand()`
    /// the spread, the recoil and the melee roll take, so one bot's
    /// choices never move another's (bot-objectives.md, "One generator
    /// per bot").
    pub(crate) rng: u64,
    /// Wander heading, degrees, wire convention.
    heading: f32,
    /// Ticks left on the current heading.
    heading_ticks: u32,
    /// Where the bot was when it picked its heading; a heading that barely
    /// moves the origin is swapped for a new one.
    stall_origin: [f32; 3],
    /// The farthest the body has been from `stall_origin` this window,
    /// squared.
    stall_reach: f32,
    stall_ticks: u32,
    /// Ticks left on a wander heading taken to get unstuck, waypoint or not.
    unstick_ticks: u32,
    /// The last think was on a leg a bot sprints when it has the breath.
    sprint_leg: bool,
    /// The last think steered the body along its waypoint, no unstick
    /// spell and no fight ([`BotBody::walking`]).
    walking: bool,
    /// The last think's objective had the bot stand: a plant or defuse
    /// under way, or at its objective's spot ([`BotBody::on_objective`]).
    on_objective: bool,
    /// The point goal whose plan the follower walked out this tick, set
    /// by the driver where the game gives up goals no body reaches
    /// (`BotWorld::GIVES_UP_GOALS`); `None` elsewhere.
    pub(crate) arrived_goal: Option<[f32; 3]>,
    /// The health item [`BotView`]'s vitals sent the bot to last tick
    /// (`retreat::nearest_pack`'s `held`).
    pub(crate) heal_at: Option<[f32; 3]>,
    /// The driver's follower walked out the plan to the health item this
    /// tick: the body heads straight at it (`Bot::track_heal`).
    pub(crate) heal_walked: bool,
    /// The point goal whose plan the driver's follower walked out this
    /// tick, in every game (`bel`'s markers, [`Bot::marker_short`]).
    pub(crate) walked_to: Option<[f32; 3]>,
    /// The health item headed straight at, the nearest the body came to
    /// it and the ticks since it last came [`SHORT_GAIN`] nearer.
    heal_short: Option<([f32; 3], f32, u32)>,
    /// The last think's view was set by a fight, a hurt look, a cover
    /// peek, a recalled spot or a retreat, not the walk: the walking
    /// view takes the path's heading at once ([`Bot::walk_turn`]).
    view_owned: bool,
    /// Ticks since the body was last on the rungs, and whether the spell
    /// under way began within [`RUNG_GRACE`] of them.
    rungs_ago: u32,
    rung_spell: bool,
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
    /// on it, and the ticks left on a crouch; whether the spell moves on
    /// the sights, and the ticks left lying down.
    strafe: i8,
    strafe_ticks: u32,
    crouch_ticks: u32,
    shuffle: bool,
    prone_ticks: u32,
    grenade_cooldown: u32,
    respawn_ticks: u32,
    stage: Stage,
    /// Ticks a throw's stage has asked for a weapon that is not in hand
    /// with the weapon machine idle: a switch that never lands
    /// ([`SWITCH_IDLE_TICKS`]).
    switch_idle: u32,
    /// The weapon a grenade throw hands back to.
    rifle: u8,
    /// The last weapon `ps.weapon` held, which the cmd keeps asking for
    /// while the hands are empty: a real client's byte is its selection,
    /// and after `enableWeapon` the pickup reads only the byte
    /// (coduo-diff.md 22).
    armed: u8,
    /// A weapon switch under way: the byte every cmd carries until
    /// `ps.weapon` reads it (docs/dev/traps.md, "Wire and netcode", "The cmd's
    /// weapon byte").
    switch_to: Option<u8>,
    /// `bas` launchers tapped for with nothing taken and health items no
    /// body reaches (`Bot::track_heal`), and the ticks each stays passed
    /// over ([`SHUN_TICKS`]); the last launcher tapped and its taps.
    shunned: Vec<([f32; 3], u32)>,
    tapped: Option<([f32; 3], u32)>,
    /// The `bas` launcher walked to, the nearest the body has come to it
    /// and the ticks since it last came nearer.
    fetch: Option<([f32; 3], f32, u32)>,
    /// The held charge room this life has thrown its frag into, or gave
    /// up on, and the ticks it has stood for the throw.
    room_thrown: Option<[f32; 3]>,
    room_wait: u32,
    /// Lives lost: a `bas` attacker goes for the next of the other side's
    /// bases after each, off the defender that killed it.
    deaths: u32,
    /// The weapon a pistol draw put away, and the ticks since the bot last
    /// saw an enemy with the pistol out.
    primary: u8,
    calm_ticks: u32,
    /// The weapon menu answered last and the weapon it was answered with;
    /// a reopened menu gets the same one while the cvars allow it.
    loadout: Option<(String, &'static str)>,
    /// The stock weapon menus this server's scripts open: 1.1's
    /// [`WEAPON_MENUS`] or United Offensive's [`WEAPON_MENUS_UO`].
    pub(crate) menus: &'static [WeaponMenu],
    /// The heading a back down onto a ladder holds (`crate::nav::Creep`).
    creep: crate::nav::Creep,
    /// The last cmd held the jump key; a second jump needs it released.
    jump_held: bool,
    /// Jumped on a crouched jump edge and not yet landed: crouch is held.
    crouch_air: bool,
    /// The waypoint the last cmd held a move key toward: a body is pinned
    /// only against a key held at its waypoint, as the walk always held it.
    pressed: Option<[f32; 3]>,
    /// The graph walk's jump rule on a jump edge, the origin the last cmd
    /// moved from, and whether the body is in the air off its own jump.
    pin: crate::nav::PinJump,
    pin_from: Option<[f32; 3]>,
    pin_air: bool,
    /// The yaw of a push-off still in the air: the forward key, facing the
    /// rungs left behind, brakes the flight until the body lands
    /// (`nav::push_off`).
    pushed_off: Option<f32>,
    /// An unpinning heading (`BotView::unpin`) and the ticks it is still
    /// held for.
    unpin: Option<(f32, usize)>,
    /// Where the body last moved while off the ground and off the rungs,
    /// and the ticks since (`Bot::wedged`).
    wedge_at: [f32; 3],
    wedge_ticks: u32,
    /// Ticks in a row [`BotView::stranded`].
    strand_ticks: u32,
    /// Ticks with no round left in any gun, launcher or pistol.
    dry_ticks: u32,
    /// A `kill` for the driver to send as a client command.
    kill: bool,
    /// A yell for the driver to send as a quick message (`yell`), and
    /// whether the body played last think and a round restart came since
    /// ([`Bot::rearm`]): the first think alive after either starts a
    /// round of a one-life gametype.
    yell: Option<yell::Yell>,
    alive: bool,
    round_start: bool,
    /// An unstick spell began this tick: the driver's follower rounds the
    /// edge it was on (`nav::Follower::stuck`).
    stuck: bool,
    /// A goal a test pins in place of the brain's own (`Bot::force_goal`).
    forced: Option<[f32; 3]>,
    obj: Objective,
    /// An LMG's bipod: ticks the sight button has been held with no deploy.
    lmg_tries: u32,
    /// Ticks left before another bipod try once one was refused or the
    /// target left the yaw cap.
    lmg_rest: u32,
    /// Ticks in sight of an enemy with no round fired at him.
    futile: u32,
    /// The enemy given up on for a while past [`FUTILE_TICKS`]
    /// ([`Bot::ignores`]).
    ignored: Option<(usize, u32)>,
    /// Ticks the bipod stays down with no enemy.
    lmg_linger: u32,
    /// A throw turned away from an enemy that came inside the blast: the
    /// yaw it goes to. And the ticks a throw has been held past its release
    /// for the view to come round onto its yaw.
    throw_away: Option<f32>,
    /// Ticks the hold has waited for the pin ([`PIN_WAIT_TICKS`]).
    pin_wait: u32,
    held_over: u32,
    /// Where the enemy a throw in hand goes at was last seen: one that left
    /// sight mid-hold is still thrown at, by a probed lob.
    throw_at: Option<[f32; 3]>,
    /// Whom a throw under way follows ([`ThrowFor`]).
    throw_for: ThrowFor,
    /// Grenades let go this game, for `bot_match`.
    throws: u32,
    /// Ticks since an enemy was last in sight: a tactical reload waits on
    /// [`QUIET_RELOAD_TICKS`].
    quiet_ticks: u32,
    /// How fast the last enemy in sight moved, flat, units/s.
    seen_speed: f32,
    /// The `bel` marker this bot last stood at; it holds still for
    /// `scr_bel_positiontime` seconds, and the hunt looks round from it
    /// rather than standing on it.
    marker_reached: Option<[f32; 3]>,
    /// A marker walked out to: it, the nearest the body came, and the
    /// ticks since it came [`SHORT_GAIN`] nearer ([`Bot::marker_short`]).
    marker_near: Option<([f32; 3], f32, u32)>,
    /// The last reliable server command the bot consumed.
    pub(crate) last_seen_seq: i32,
    /// The client-command sequence the bot's own replies use.
    pub(crate) next_command_seq: i32,
    /// The retreat (`bots/retreat.rs`): whether this bot retreats at all,
    /// the one under way, the health the last hurt one started at, and
    /// whether this dry clip already had its retreat.
    retreat_on: bool,
    /// `vc_bot_retreat`, applied by the bot pass ([`Bot::allow_retreat`]).
    retreat_cvar: bool,
    retreat: Option<retreat::Retreat>,
    hurt_at: i32,
    dry_had: bool,
    /// Ticks before a `ctf` carrier may hold from an enemy ahead again
    /// (`retreat::AHEAD_REST`).
    ahead_rest: u32,
    /// Boarding, driving and leaving a vehicle (`vehicle`).
    rider: vehicle::Rider,
    /// The last cmd held the trigger.
    fired: bool,
    /// The target last dropped or replaced: its slot, its aim error then,
    /// and the ticks since ([`Bot::track`]).
    prior: Option<(usize, f32, u32)>,
}

impl Bot {
    pub fn new(team: &str, shoot: bool, seed: u64) -> Self {
        Bot {
            name: String::new(),
            shoot,
            skill: Skill::default(),
            cover: cover::Cover::default(),
            team_play: true,
            squad: true,
            tactics: true,
            converge: true,
            lanes: true,
            far: true,
            cues: true,
            travel: true,
            ctf_carry: true,
            ctf_escort: false,
            ctf_route: false,
            ctf_hold: false,
            ctf_sprint: false,
            ctf_watch: false,
            ctf_close: false,
            ctf_near: false,
            anchor: None,
            guarded: None,
            exposure: None,
            start_hold: 0,
            footwork: true,
            aware: true,
            wary: true,
            human_aim: true,
            clock: 0,
            gun_kept: [[0.0; 2]; aim::RECOIL_KEEP],
            hunts: 0,
            main_menu: String::new(),
            answered: Vec::new(),
            heading: 0.0,
            heading_ticks: 0,
            stall_origin: [0.0; 3],
            stall_reach: 0.0,
            stall_ticks: 0,
            unstick_ticks: 0,
            sprint_leg: false,
            walking: false,
            on_objective: false,
            arrived_goal: None,
            heal_at: None,
            heal_walked: false,
            walked_to: None,
            heal_short: None,
            view_owned: false,
            rungs_ago: u32::MAX,
            rung_spell: false,
            unstick_at: [f32::INFINITY; 3],
            unstick_tries: 0,
            fire_cooldown: 0,
            burst_ticks: 0,
            target: None,
            recall: None,
            strafe: 0,
            strafe_ticks: 0,
            crouch_ticks: 0,
            shuffle: false,
            prone_ticks: 0,
            grenade_cooldown: 200,
            respawn_ticks: 0,
            deaths: 0,
            shunned: Vec::new(),
            tapped: None,
            fetch: None,
            room_thrown: None,
            room_wait: 0,
            stage: Stage::Wander,
            switch_idle: 0,
            rifle: 0,
            armed: 0,
            switch_to: None,
            primary: 0,
            calm_ticks: 0,
            loadout: None,
            menus: &WEAPON_MENUS,
            creep: crate::nav::Creep::default(),
            jump_held: false,
            crouch_air: false,
            pressed: None,
            pin: crate::nav::PinJump::default(),
            pin_from: None,
            pin_air: false,
            pushed_off: None,
            unpin: None,
            wedge_at: [f32::INFINITY; 3],
            wedge_ticks: 0,
            strand_ticks: 0,
            dry_ticks: 0,
            kill: false,
            yell: None,
            alive: false,
            round_start: true,
            stuck: false,
            forced: None,
            obj: Objective::default(),
            lmg_tries: 0,
            lmg_rest: 0,
            futile: 0,
            ignored: None,
            lmg_linger: 0,
            throw_away: None,
            pin_wait: 0,
            throw_at: None,
            throw_for: ThrowFor::Any,
            throws: 0,
            quiet_ticks: 0,
            seen_speed: 0.0,
            held_over: 0,
            marker_reached: None,
            marker_near: None,
            last_seen_seq: 0,
            next_command_seq: 1,
            rider: Default::default(),
            fired: false,
            prior: None,
            team: team.to_string(),
            rng: seed | 1,
            retreat_on: true,
            retreat_cvar: true,
            retreat: None,
            hurt_at: i32::MAX,
            dry_had: false,
            ahead_rest: 0,
        }
    }

    /// xorshift64* masked to 31 bits off the bot's own state.
    fn rand(&mut self) -> i32 {
        crate::game::host::rand_int(&mut self.rng)
    }

    /// The team the team menu is answered with.
    pub fn team(&self) -> &str {
        &self.team
    }

    /// A team move (`server/bot_admin.rs`): the team menu answers `team`
    /// from now on and the weapon menu the switch reopens is answered again.
    pub fn set_team(&mut self, team: &str) {
        self.team = team.to_string();
        self.answered.clear();
    }

    /// `vc_bot_shoot`, applied live.
    pub fn set_shoot(&mut self, shoot: bool) {
        self.shoot = shoot;
    }

    /// Where the bot wants to be this tick: the enemy it sees, else the
    /// spot it remembers or heard, else anywhere far. Asked before
    /// [`Bot::think`], which then follows the waypoint the server planned
    /// toward it.
    pub fn goal(&self, view: &BotView) -> Goal {
        match self.goal_any(view) {
            // A burning hull's second blast went off where the bot's noise
            // walk led (bot-objectives.md 10.9): no walk into one.
            Goal::To(p) if wreck_near(&view.wrecks, p).is_some() => Goal::Hold,
            g => g,
        }
    }

    fn goal_any(&self, view: &BotView) -> Goal {
        if view.dead || !view.playing || !matches!(self.stage, Stage::Wander) {
            return Goal::Hold;
        }
        // A plant or defuse aborts on release, so it outranks a fight.
        if self.objective_busy(view) {
            return Goal::Hold;
        }
        if let Some(p) = view.give_way {
            return Goal::To(p);
        }
        if let Some(p) = self.forced {
            return Goal::To(p);
        }
        if let Some(f) = view.frag_near {
            return Goal::Away(f);
        }
        if let Some(c) = Self::blast_near(view).or_else(|| wreck_near(&view.wrecks, view.origin)) {
            return Goal::Away(c);
        }
        if self.start_held(view) {
            return Goal::Hold;
        }
        // A `ctf` carrier holding in cover from an enemy ahead.
        if self.holding_ahead()
            && let Some(g) = self.retreat_goal(view)
        {
            return g;
        }
        // A carrier keeps for the goal and fights on the way.
        if self.carrying(view)
            && let Some(g) = self.objective_goal(view)
        {
            return g;
        }
        if let Some(g) = self.retreat_goal(view) {
            return g;
        }
        if let Some(e) = view.enemy {
            return Goal::To(e.origin);
        }
        if let Some(g) = self.heal_goal(view) {
            return g;
        }
        if let Some(g) = self.vehicle_goal(view) {
            return g;
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
        if let Some(p) = view.post {
            return Goal::To(p);
        }
        Goal::Roam
    }

    /// The `kill` [`Bot::think`] asked for, once.
    pub(crate) fn take_kill(&mut self) -> bool {
        std::mem::take(&mut self.kill)
    }

    /// The yell [`Bot::think`] asked for, once.
    pub(crate) fn take_yell(&mut self) -> Option<yell::Yell> {
        self.yell.take()
    }

    /// Asks for `y` unless this think already asked for another.
    fn want_yell(&mut self, y: yell::Yell) {
        self.yell.get_or_insert(y);
    }

    /// Whether an unstick spell began since the last call.
    pub(crate) fn take_stuck(&mut self) -> bool {
        std::mem::take(&mut self.stuck)
    }

    /// One life over: the next is a new round, from the first site again,
    /// and no switch carries over.
    fn end_life(&mut self) {
        if self.obj.live {
            self.obj = Objective::default();
        }
        self.switch_to = None;
        self.calm_ticks = 0;
        self.retreat = None;
        self.hurt_at = i32::MAX;
        self.dry_had = false;
        self.prior = None;
        self.room_thrown = None;
        self.room_wait = 0;
        self.cover.end_life();
    }

    /// The tick's usercmd. The driver stamps `server_time` and pushes the
    /// cmd into the client's queue.
    pub(crate) fn think(&mut self, view: &BotView) -> UserCmd {
        self.clock = self.clock.wrapping_add(1);
        self.track_reach(view);
        self.track_heal(view);
        // Read before the think moves the objective's state, as the goal was.
        let target = self.objective_target(view);
        self.on_objective =
            self.objective_busy(view) || target.is_some_and(|t| self.at_post(view, &t));
        self.anchor = target.map(|t| Self::anchor_point(&t));
        self.guarded = match target {
            Some(ObjTarget::Guard { at, .. }) if self.on_objective => Some(at),
            _ => None,
        };
        self.exposure = view.exposure;
        self.heal_at = view.survive.heal;
        let mut cmd = self.think_body(view);
        // Overlapping a terrain patch the move lets it walk into, a body
        // that ducked could never stand again: mp_cassino's ledge at
        // (5779, -1990, 82) held one prone for 100 s (bot-objectives.md
        // 13.9).
        if !view.stand_room && view.on_ground && view.eye_height >= vcod_common::pmove::VIEW_STAND {
            cmd.wbuttons &= !(msg::WBUTTON_CROUCH | msg::WBUTTON_PRONE);
            cmd.up = cmd.up.max(0);
        }
        self.count_futile(view, &cmd);
        self.fired = cmd.buttons & msg::BUTTON_ATTACK != 0;
        let living = view.playing && !view.dead;
        self.start_hold = if living && view.enemy.is_none() {
            self.start_hold.saturating_sub(1)
        } else {
            0
        };
        if living && (self.round_start || !self.alive) {
            self.round_start = false;
            if view.one_life {
                self.want_yell(yell::Yell::Advance);
                self.start_hold = self.start_draw(view);
            }
        }
        self.alive = living;
        cmd
    }

    /// The hold a one-life round starts with on an objective level
    /// ([`team::START_HOLD`]), drawn off the bot's generator without
    /// moving it, so the brain's own draws stay where they were.
    fn start_draw(&self, view: &BotView) -> u32 {
        if !(self.team_play && self.squad && driver::team_objective(view)) {
            return 0;
        }
        let mut z = self.rng ^ 0x5851_f42d_4c95_7f2d;
        let r = (vcod_common::rng::xorshift(&mut z) >> 33) as u32;
        let (lo, hi) = match view.squad {
            team::Squad::Main => (0, team::START_HOLD),
            team::Squad::Flank => (team::FLANK_HOLD, team::START_HOLD),
            team::Squad::Rear => (team::REAR_HOLD, 2 * team::REAR_HOLD),
        };
        lo + r % (hi - lo)
    }

    /// Out of a fight, the bot still holds at the start of its round; a
    /// carrier handed CoD4's bomb after the draw goes at once.
    fn start_held(&self, view: &BotView) -> bool {
        self.start_hold > 0 && view.enemy.is_none() && !self.carrying(view)
    }

    fn think_body(&mut self, view: &BotView) -> UserCmd {
        self.fire_cooldown = self.fire_cooldown.saturating_sub(1);
        self.sprint_leg = false;
        self.walking = false;
        for s in &mut self.shunned {
            s.1 -= 1;
        }
        self.shunned.retain(|s| s.1 > 0);
        if view.rocket.is_some() {
            self.tapped = None;
        }
        self.rider.at_armor = false;
        let mut cmd = NULL_USERCMD;
        if view.weapon != 0 {
            self.armed = view.weapon;
        } else if view.weapons_held >> self.armed.min(63) & 1 == 0
            && let Some(w) = view.gun.or(view.rocket).or(view.pistol)
        {
            // Empty hands after a use pickup swapped the armed weapon away:
            // the byte asks for one still carried, or nothing comes up.
            self.armed = w;
        }
        cmd.weapon = self.armed;
        // Death and spectatorship outrank the stage machine: a corpse still
        // in a grenade phase must reach the respawn press below, not sit in
        // `think_cook` forever.
        if view.dead {
            if self.respawn_ticks == 0 {
                self.deaths = self.deaths.wrapping_add(1);
            }
            self.respawn_ticks += 1;
            // Whatever the death interrupted, the new life starts clean: no
            // throw resumes on respawn.
            self.stage = Stage::Wander;
            self.end_throw();
            self.disengage();
            self.recall = None;
            self.end_life();
            self.vehicle_life_over();
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
            // Spectator or intermission camera: nothing to press, and no
            // throw carries into the next level or round.
            self.respawn_ticks = 0;
            self.stage = Stage::Wander;
            self.end_throw();
            self.disengage();
            self.recall = None;
            self.end_life();
            return cmd;
        }
        self.respawn_ticks = 0;
        // A seat ends any retreat: the vehicle is the bot's cover now.
        if view.seat.is_some() {
            self.retreat = None;
        }
        // The memory runs in a seat and on the walk to one too: without it a
        // driver's goal was the tick's raw noise, gone on the next, and each
        // flip planned the hull a new route (bot-objectives.md 10.8).
        self.remember(view);
        if let Some(cmd) = self.think_vehicle(view, cmd) {
            return cmd;
        }
        if self.wedged(view) && !view.one_life {
            self.kill = true;
        }
        // Nothing left to fire: a respawn brings the kit back. A `bas`
        // defender stood dry at its base for twenty minutes, an attacker
        // in its sight. Not while a `bas` bot fetches a weapon lying near.
        self.dry_ticks = if Self::dry(view) {
            self.dry_ticks + 1
        } else {
            0
        };
        if self.dry_ticks >= DRY_TICKS && !view.one_life && self.fetch.is_none() {
            self.dry_ticks = 0;
            self.kill = true;
        }
        self.obj.live = true;
        self.quiet_ticks = match view.enemy {
            Some(e) => {
                self.seen_speed = e.velocity[0].hypot(e.velocity[1]);
                0
            }
            None => self.quiet_ticks.saturating_add(1),
        };
        let walked = self.walked_to.take();
        if let Some(bel) = view.bel.as_ref()
            && let Some(m) = self.next_marker(view, bel)
            && (arrived(view.origin, m) || self.marker_short(view, m, walked))
        {
            self.marker_reached = Some(m);
            self.marker_near = None;
        }
        // A switch the weapon machine sits idle on never lands: the rungs
        // hold the hands empty (`ps.weapon` 0) for as long as the body is on
        // them, and a throw that waited there with its keys off hung on
        // mp_ship's hold ladder for 3000 ticks, out of reach of the stall
        // circle below (bot-combat.md 17.2). Give the throw up.
        let want = match self.stage {
            Stage::ToGrenade | Stage::Cook { .. } => view.grenade,
            Stage::BackToRifle { weapon } => Some(weapon),
            Stage::Wander => None,
        };
        self.switch_idle = match want {
            Some(w) if view.weapon != w && view.busy_ms == 0 => self.switch_idle + 1,
            _ => 0,
        };
        if self.switch_idle > SWITCH_IDLE_TICKS {
            self.switch_idle = 0;
            self.stage = Stage::Wander;
            self.end_throw();
        }
        if !matches!(self.stage, Stage::Wander) {
            if let Some(e) = self.throw_enemy(view) {
                self.throw_at = Some(e.origin);
            }
            cmd.angles = self.lob_angles(view);
        }
        // A throw under way runs from a live frag too: one that came back
        // off a wall rolled to a bot standing through the switch back to
        // its rifle (bot-objectives.md 6).
        let mut staged = match self.stage {
            Stage::ToGrenade => self.think_grenade(view, cmd),
            Stage::Cook { left, spoon } => self.think_cook(view, cmd, left, spoon),
            Stage::BackToRifle { weapon } => self.think_back(view, cmd, weapon),
            Stage::Wander => {
                return self.think_wander(view, cmd);
            }
        };
        let yaw = (staged.angles[1] + view.delta_angles[1]) as f32 / ANGLE2SHORT;
        self.flee(view, &mut staged, yaw);
        staged
    }

    /// `0..n`, 0 when `n` is 0.
    fn rand_below(&mut self, n: u32) -> u32 {
        if n == 0 { 0 } else { self.rand() as u32 % n }
    }

    /// Test-facing: walk to `p` whatever the brain would want, until
    /// cleared with `None`.
    pub fn force_goal(&mut self, p: Option<[f32; 3]>) {
        self.forced = p;
    }

    /// On the random heading a stuck bot takes ([`BotBody::unsticking`]).
    pub fn unsticking(&self) -> bool {
        self.unstick_ticks > 0
    }

    /// On a leg it sprints when it has the breath ([`BotBody::sprint_leg`]).
    pub fn sprint_leg(&self) -> bool {
        self.sprint_leg
    }

    /// Steered along its waypoint last think ([`BotBody::walking`]).
    pub fn walking(&self) -> bool {
        self.walking
    }

    /// Stood at its objective last think ([`BotBody::on_objective`]).
    pub fn on_objective(&self) -> bool {
        self.on_objective
    }

    /// Switching to its frag or holding it for the throw.
    pub fn in_throw(&self) -> bool {
        matches!(self.stage, Stage::ToGrenade | Stage::Cook { .. })
    }

    /// Grenades let go this game ([`BotBody::throws`]).
    pub fn throws(&self) -> u32 {
        self.throws
    }

    /// Peeks from cover started, those that repeated the spot and pose of
    /// the one before, and hunts that reached their spot or vantage
    /// (bot-combat.md section 1).
    pub fn tactics(&self) -> [u32; 3] {
        [self.cover.peeks, self.cover.repeeks, self.hunts]
    }

    /// Tracking an enemy ([`BotBody::engaged`]), on foot or from a seat.
    pub fn engaged(&self) -> bool {
        self.target.is_some() || self.seated_fight()
    }

    /// The client slot of the enemy the bot tracks, reaction delay and all.
    pub fn target_slot(&self) -> Option<usize> {
        self.target.as_ref().map(|t| t.slot)
    }

    /// Whether the last cmd held the trigger.
    pub fn fired(&self) -> bool {
        self.fired
    }
}
/// Ticks a throw's switch may sit with the weapon machine idle and the
/// weapon it asked for not in hand before the throw is given up: a switch
/// lands a server frame or two after the putaway ends.
const SWITCH_IDLE_TICKS: u32 = 20;
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
    let l = view.lean_offset;
    let dx = at[0] - (view.origin[0] + l[0]);
    let dy = at[1] - (view.origin[1] + l[1]);
    let dz = at[2] - (view.origin[2] + view.eye_height + l[2]);
    let horiz = (dx * dx + dy * dy).sqrt().max(1.0);
    let pitch = (-dz.atan2(horiz).to_degrees()).clamp(-80.0, 80.0);
    let yaw = dy.atan2(dx).to_degrees();
    (pitch, yaw)
}

/// An absolute (pitch, yaw) as the cmd carries it, net of `delta_angles`.
pub(crate) fn cmd_angles(view: &BotView, aim: [f32; 2]) -> [i32; 3] {
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

#[cfg(test)]
mod tests {
    use super::fight::{
        FUTILE_TICKS, HIT_BY_ERROR, HIT_RADIUS, HOLSTER_TICKS, HURT_LOOK_MS, IGNORE_TICKS,
        LMG_TRY_TICKS, REACQUIRE_ERROR,
    };
    use super::grenade::{
        AWAY_DEG, AWAY_TICKS, COVER_THROW_TICKS, LOST_THROW_TICKS, PIN_WAIT_TICKS,
    };
    use super::objective::{
        BAS_CLEAR_S, DEFUSE_CLOSE_TICKS, FETCH_STALL_TICKS, LAUNCHER_TAPS, PLANT_PITCH, SETTLE_DEG,
        SHY_TICKS, START_TICKS,
    };
    use super::sight::{MEMORY_LEAD_S, SIDESTEP_HOLD_MS};
    use super::walk::{
        CORNERED, QUIET_RELOAD_TICKS, RECALL_PACE, STRAND_TICKS, UNSTICK_TICKS, WATCH_DEG,
        WEDGE_TICKS,
    };
    use super::*;
    use vcod_common::net::msg::{BUTTON_ADS, BUTTON_ATTACK, BUTTON_USE, WBUTTON_RELOAD};

    const SID: i32 = 0x10;

    fn all(_: &str) -> bool {
        true
    }

    /// A shooting bot with no reaction delay, already looking at an enemy
    /// 100 units down +x.
    pub(super) fn engaged(seed: u64) -> (Bot, BotView) {
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

    /// A switch to a weapon past the 64-bit held mask (a modded level's
    /// `a <n>`) ends as a weapon not held, with no shift overflow
    /// (BUG-156).
    #[test]
    fn a_switch_past_the_held_mask_ends() {
        let mut bot = Bot::new("allies", true, 1);
        let mut v = view();
        v.playing = true;
        bot.switch_to = Some(70);
        bot.think(&v);
        assert_eq!(bot.switch_to, None);
    }

    /// An intermission or a spectator spell drops a throw under way, as a
    /// death does: the next level does not start on last level's frag
    /// (BUG-135).
    #[test]
    fn a_throw_under_way_ends_when_the_bot_stops_playing() {
        let mut bot = Bot::new("allies", true, 1);
        bot.stage = Stage::Cook { left: 5, spoon: 2 };
        bot.throw_at = Some([100.0, 0.0, 0.0]);
        let mut v = view();
        v.playing = false;
        bot.think(&v);
        assert!(matches!(bot.stage, Stage::Wander));
        assert_eq!(bot.throw_at, None);
    }

    pub(super) fn view() -> BotView {
        BotView {
            origin: [0.0, 0.0, 64.0],
            eye_height: vcod_common::pmove::VIEW_STAND,
            eye_settled: true,
            view: [0.0, 90.0, 0.0],
            weapon: 10,
            weapons_held: (1 << 10) | (1 << 6),
            clip: 8,
            fire_time_ms: 300,
            has_ads: true,
            playing: true,
            grenade: Some(6),
            grenade_ready: true,
            stand_room: true,
            tops_out: false,
            on_ground: true,
            move_scale: 1.0,
            primaries: [10, 0],
            reload_ms: 2500,
            draw_ms: 750,
            loaded: (1 << 10) | (1 << 6),
            ..Default::default()
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
            cover: None,
            key: None,
            hot: None,
        });
        v
    }

    /// A bot in a zone it does not hold, contested or not.
    fn dom_zone_view(at: [f32; 3], contested: bool) -> BotView {
        let mut v = view();
        v.origin = at;
        v.uo = Some(UoView::Dom(DomView {
            flags: vec![DomFlagView {
                site: SiteView {
                    mins: [0.0, 0.0, 0.0],
                    maxs: [400.0, 200.0, 128.0],
                    stand: [200.0, 100.0, 0.0],
                },
                ours: false,
                contested,
            }],
            rank: 0,
        }));
        v
    }

    #[test]
    fn a_bot_in_a_contested_zone_walks_its_far_side_until_the_contest_ends() {
        let mut bot = Bot::new("allies", true, 1);
        let v = dom_zone_view([350.0, 150.0, 0.0], false);
        bot.think(&v);
        assert_eq!(bot.goal(&v), Goal::Hold, "in the zone, nobody else in it");
        let v = dom_zone_view([350.0, 150.0, 0.0], true);
        bot.think(&v);
        assert_eq!(bot.goal(&v), Goal::To([50.0, 50.0, 0.0]), "to the far side");
        let v = dom_zone_view([200.0, 100.0, 0.0], true);
        bot.think(&v);
        assert_eq!(bot.goal(&v), Goal::To([50.0, 50.0, 0.0]), "kept on the way");
        let v = dom_zone_view([50.0, 50.0, 0.0], true);
        bot.think(&v);
        assert_eq!(bot.goal(&v), Goal::To([350.0, 150.0, 0.0]), "and back");
        let v = dom_zone_view([200.0, 100.0, 0.0], false);
        bot.think(&v);
        assert_eq!(bot.goal(&v), Goal::Hold, "the contest over");
    }

    /// A bot whose follower walked out its plan to a zone's stand short of
    /// the zone gives the stand up after `SHORT_TICKS` without coming
    /// nearer, guards round it for `SHY_TICKS`, then walks at it again.
    #[test]
    fn a_bot_that_cannot_close_on_its_zone_guards_round_it_for_a_while() {
        let mut bot = Bot::new("allies", true, 1);
        // Outside the zone, 60 off its stand, walking at it.
        let v = dom_zone_view([200.0, 260.0, 0.0], false);
        let stand = [200.0, 100.0, 0.0];
        assert_eq!(bot.goal(&v), Goal::To(stand));
        for tick in 0..SHORT_TICKS {
            bot.arrived_goal = Some(stand);
            bot.think(&v);
            assert_eq!(bot.goal(&v), Goal::To(stand), "tick {tick}");
        }
        bot.arrived_goal = Some(stand);
        bot.think(&v);
        assert_eq!(bot.goal(&v), Goal::Hold, "a guard round the stand");
        assert!(bot.on_objective(), "held on purpose");
        for _ in 1..SHY_TICKS {
            bot.think(&v);
        }
        assert_eq!(bot.goal(&v), Goal::Hold);
        bot.think(&v);
        assert_eq!(bot.goal(&v), Goal::To(stand), "tried again");
        // A follower still on its plan never gives the stand up.
        for _ in 0..2 * SHORT_TICKS {
            bot.think(&v);
        }
        assert_eq!(bot.goal(&v), Goal::To(stand));
    }

    /// A guard with a post walks to it rather than hold where it first
    /// stands in the ring, then holds it, watching the objective, crouched
    /// where the post hides it; with tactics off it holds where it stands.
    #[test]
    fn a_guard_walks_to_its_post_and_crouches_there() {
        let mut bot = Bot::new("axis", true, 1);
        let mut v = sd_view(ObjRole::Defend);
        v.origin = [100.0, 0.0, 64.0];
        let post = GuardPost {
            at: [-150.0, 60.0, 64.0],
            crouch: true,
            look: [0.0, 0.0, 64.0],
            careful: None,
        };
        v.guard_post = Some(post);
        assert_eq!(bot.goal(&v), Goal::To(post.at), "to the post, not held");
        v.origin = post.at;
        assert_eq!(bot.goal(&v), Goal::Hold);
        let cmd = bot.think(&v);
        assert!(bot.on_objective(), "held on purpose");
        assert_ne!(cmd.wbuttons & msg::WBUTTON_CROUCH, 0, "crouched");
        assert_eq!((cmd.forward, cmd.right), (0, 0));
        // It looks within `WATCH_DEG` of the site's middle.
        let toward = (-60.0f32).atan2(150.0).to_degrees();
        assert!(yaw_diff(bot.heading, toward).abs() <= WATCH_DEG as f32 + 0.5);
        // Where a standing box would not fit, it stays up: it could not
        // rise again.
        v.stand_room = false;
        let cmd = bot.think(&v);
        assert_eq!(cmd.wbuttons & (msg::WBUTTON_CROUCH | msg::WBUTTON_PRONE), 0);
        assert!(cmd.up >= 0);
        v.stand_room = true;
        bot.tactics = false;
        v.origin = [100.0, 0.0, 64.0];
        assert_eq!(bot.goal(&v), Goal::Hold, "anywhere in the ring");
    }

    /// An S&D defender with a site under attack goes there instead of its
    /// own; with converge off it keeps its own.
    #[test]
    fn a_defender_converges_on_the_site_under_attack() {
        let mut bot = Bot::new("axis", true, 1);
        let mut v = sd_view(ObjRole::Defend);
        v.origin = [0.0, 0.0, 64.0];
        assert_eq!(bot.goal(&v), Goal::Hold, "at its own site");
        v.sd.as_mut().unwrap().hot = Some(1);
        assert_eq!(bot.goal(&v), Goal::To([2000.0, 0.0, 64.0]));
        bot.converge = false;
        assert_eq!(bot.goal(&v), Goal::Hold);
    }

    /// An attacker with a lane walks its way in first; with lanes off,
    /// straight to the site.
    #[test]
    fn an_attacker_walks_its_lane_first() {
        let mut bot = attacker();
        let mut v = sd_view(ObjRole::Attack);
        v.origin = [-3000.0, 0.0, 64.0];
        v.lane = Some([-600.0, 900.0, 64.0]);
        assert_eq!(bot.goal(&v), Goal::To([-600.0, 900.0, 64.0]));
        bot.lanes = false;
        assert_eq!(bot.goal(&v), Goal::To([0.0, 0.0, 64.0]));
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

    /// A weapon lying by the bomb that the settled view would take
    /// (a planter's, dropped where he died) keeps the press back: the
    /// defuser closes in over the bomb and presses there, and presses
    /// anyway once [`DEFUSE_CLOSE_TICKS`] run out.
    #[test]
    fn a_defuser_closes_in_rather_than_take_a_weapon_by_the_bomb() {
        let mut bot = Bot::new("axis", true, 1);
        let mut v = sd_view(ObjRole::Defend);
        let bomb = [30.0, 0.0, 64.0];
        plant_bomb(&mut v, bomb);
        let (tp, ty) = aim_angles(&v, [30.0, 0.0, 72.0]);
        v.view = [tp, ty, 0.0];
        v.use_takes_weapon = true;
        for tick in 0..10 {
            assert_eq!(bot.think(&v).buttons & BUTTON_USE, 0, "tick {tick}");
        }
        assert_eq!(bot.goal(&v), Goal::To(bomb), "closes in");
        // Over the bomb the trigger is the pick.
        v.origin = [26.0, 0.0, 64.0];
        v.use_takes_weapon = false;
        assert_eq!(bot.goal(&v), Goal::Hold);
        let (tp, ty) = aim_angles(&v, [30.0, 0.0, 72.0]);
        v.view = [tp, ty, 0.0];
        assert!((0..4).any(|_| bot.think(&v).buttons & BUTTON_USE != 0));

        let mut bot = Bot::new("axis", true, 1);
        v.origin = [0.0, 0.0, 64.0];
        let (tp, ty) = aim_angles(&v, [30.0, 0.0, 72.0]);
        v.view = [tp, ty, 0.0];
        v.use_takes_weapon = true;
        let pressed =
            (0..2 * DEFUSE_CLOSE_TICKS).position(|_| bot.think(&v).buttons & BUTTON_USE != 0);
        assert!(
            pressed.is_some_and(|t| t >= DEFUSE_CLOSE_TICKS as usize - 2),
            "{pressed:?}"
        );
    }

    /// The `bas` charge press keeps back from a weapon lying by the
    /// charge as the S&D defuse does: it closes in, then presses.
    #[test]
    fn a_bas_planter_closes_in_rather_than_take_a_weapon_by_the_charge() {
        let mut bot = Bot::new("allies", true, 1);
        let mut v = bas_view();
        let charge = [2000.0, 0.0, -176.0];
        let aim = [2000.0, 0.0, -160.0];
        let b = their_base(&mut v);
        b.breached = true;
        b.charge = Some(BombView {
            origin: charge,
            aim,
        });
        v.origin = [1920.0, 0.0, -176.0];
        let (tp, ty) = aim_angles(&v, aim);
        v.view = [tp, ty, 0.0];
        v.use_takes_weapon = true;
        for tick in 0..10 {
            assert_eq!(bot.think(&v).buttons & BUTTON_USE, 0, "tick {tick}");
        }
        assert_eq!(bot.goal(&v), Goal::To(charge), "closes in");
        v.origin = [1990.0, 0.0, -176.0];
        v.use_takes_weapon = false;
        assert_eq!(bot.goal(&v), Goal::Hold);
        let (tp, ty) = aim_angles(&v, aim);
        v.view = [tp, ty, 0.0];
        assert!((0..4).any(|_| bot.think(&v).buttons & BUTTON_USE != 0));
    }

    /// A charge room a teammate died in of late is held: the planter
    /// making for it stops in range and lobs one frag at the charge from
    /// outside the room before it goes in, and none into a room nobody
    /// died in.
    #[test]
    fn a_bas_planter_lobs_a_frag_into_a_held_charge_room() {
        let throws = |fallen: Vec<[f32; 3]>, held: bool| {
            let mut bot = Bot::new("allies", true, 1);
            let mut v = bas_view();
            let b = their_base(&mut v);
            b.breached = true;
            b.charge = Some(BombView {
                origin: [2000.0, 0.0, -176.0],
                aim: [2000.0, 0.0, -160.0],
            });
            v.origin = [1500.0, 0.0, -176.0];
            v.lob = Some([-12.0, 0.0]);
            v.fallen = fallen;
            let mut starts = 0;
            for tick in 0..600 {
                // On the run as the first throw's cooldown ends: it stops
                // for the throw.
                v.speed = if tick < 210 { 190.0 } else { 0.0 };
                let cmd = bot.think(&v);
                if (201..210).contains(&tick) && held {
                    assert_eq!((cmd.forward, cmd.right), (0, 0), "tick {tick}");
                }
                if cmd.weapon == 6 && v.weapon != 6 {
                    starts += 1;
                }
                v.weapon = cmd.weapon;
            }
            starts
        };
        assert_eq!(throws(vec![[1950.0, 40.0, -176.0]], true), 1);
        assert_eq!(throws(Vec::new(), false), 0);
        assert_eq!(
            throws(vec![[1500.0, 0.0, -176.0]], false),
            0,
            "died outside the room"
        );
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
    pub(super) fn re_view(role: ObjRole) -> BotView {
        let mut v = view();
        v.view = [0.0, 0.0, 0.0];
        v.re = Some(ReView {
            role,
            rank: 0,
            escort: true,
            objectives: vec![ReObjView {
                pickup: Some([30.0, 0.0, 72.0]),
                goal: [3000.0, 0.0, 64.0],
                goal_clear: 300.0,
                carried: false,
                mine: false,
                carrier_at: None,
                laid: Some([30.0, 0.0, 64.0]),
                returns: None,
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

    /// An escort holds inside its ring round the carrier, but within
    /// `ESCORT_CLEAR` of him it walks off: standing on his next node on a
    /// stair it held mp_carentan's carrier for 75 s.
    #[test]
    fn an_escort_steps_off_a_carrier_it_stands_on() {
        let bot = Bot::new("allies", true, 1);
        let mut v = re_view(ObjRole::Attack);
        let o = &mut v.re.as_mut().unwrap().objectives[0];
        (o.pickup, o.carried, o.carrier_at) = (None, true, Some([500.0, 0.0, 64.0]));
        v.origin = [350.0, 0.0, 64.0];
        assert_eq!(bot.goal(&v), Goal::Hold);
        v.origin = [470.0, 0.0, 64.0];
        assert_eq!(bot.goal(&v), Goal::Away([500.0, 0.0, 64.0]));
    }

    /// A dropped objective 1000 units off: with 10 s left on its clock
    /// an attacker goes for it and a defender guards it; with 3 s left
    /// (570 units of run) both wait where it goes back to.
    #[test]
    fn a_dropped_objective_out_of_reach_of_its_clock_is_awaited_at_its_start() {
        let home = [-2000.0, 0.0, 72.0];
        for role in [ObjRole::Attack, ObjRole::Defend] {
            let bot = Bot::new("allies", true, 1);
            let mut v = re_view(role);
            v.origin = [1030.0, 0.0, 64.0];
            let o = &mut v.re.as_mut().unwrap().objectives[0];
            o.returns = Some((home, 10.0));
            assert_eq!(bot.goal(&v), Goal::To([30.0, 0.0, 72.0]), "{role:?}");
            v.re.as_mut().unwrap().objectives[0].returns = Some((home, 3.0));
            assert_eq!(bot.goal(&v), Goal::To(home), "{role:?}");
        }
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

    /// A carrier with an enemy in sight keeps for the goal: the path
    /// keys stay, and only the view turns to fight.
    #[test]
    fn a_carrier_runs_for_the_goal_through_a_fight() {
        let (mut bot, mut v) = engaged(1);
        let e = v.enemy;
        v.re = re_view(ObjRole::Attack).re;
        let o = &mut v.re.as_mut().unwrap().objectives[0];
        (o.pickup, o.carried, o.mine) = (None, true, true);
        v.enemy = e;
        assert_eq!(bot.goal(&v), Goal::To([3000.0, 0.0, 64.0]));
        v.waypoint = Some([0.0, 500.0, 64.0]);
        let cmd = bot.think(&v);
        assert!(cmd.forward != 0 || cmd.right != 0, "the keys run the path");
        bot.team_play = false;
        assert!(
            !bot.carrying(&v),
            "team play off: the carrier fights as any bot"
        );
    }

    /// Only the escort keeps by the carrier; the rest of the team go
    /// ahead and hold the ring round his goal, clear of its trigger.
    #[test]
    fn the_other_attackers_hold_the_carriers_goal() {
        let bot = Bot::new("allies", true, 1);
        let mut v = re_view(ObjRole::Attack);
        v.re.as_mut().unwrap().escort = false;
        let o = &mut v.re.as_mut().unwrap().objectives[0];
        (o.pickup, o.carried, o.carrier_at) = (None, true, Some([1000.0, 0.0, 64.0]));
        assert_eq!(bot.goal(&v), Goal::To([3000.0, 0.0, 64.0]));
        v.origin = [3000.0 - 350.0, 0.0, 64.0];
        assert_eq!(bot.goal(&v), Goal::Hold);
    }

    /// An attacker with a teammate planting nearby guards him, off the
    /// zone, instead of making for its own site or pressing use in his.
    #[test]
    fn an_attacker_covers_a_planting_teammate() {
        let mut bot = attacker();
        let mut v = sd_view(ObjRole::Attack);
        v.sd.as_mut().unwrap().rank = 1;
        v.sd.as_mut().unwrap().cover = Some([0.0, 0.0, 64.0]);
        v.origin = [1000.0, 0.0, 64.0];
        assert_eq!(bot.goal(&v), Goal::To([0.0, 0.0, 64.0]));
        // On the planter: off him; in the ring: holds, use never pressed.
        v.origin = [20.0, 0.0, 64.0];
        assert_eq!(bot.goal(&v), Goal::Roam);
        v.origin = [200.0, 0.0, 64.0];
        assert_eq!(bot.goal(&v), Goal::Hold);
        for _ in 0..40 {
            assert_eq!(bot.think(&v).buttons & BUTTON_USE, 0);
        }
    }

    #[test]
    fn use_is_pressed_only_at_the_objective() {
        for role in [ObjRole::Attack, ObjRole::Defend] {
            let mut bot = attacker();
            let mut v = sd_view(role);
            v.origin = [800.0, 0.0, 64.0];
            v.waypoint = Some([700.0, 0.0, 64.0]);
            v.view[1] = 180.0;
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

    /// A marker hanging 150 units over the floor (the mean of a spot on a
    /// ledge and one below it): once the plan to it is walked out and the
    /// body comes no nearer for [`SHORT_TICKS`], the hunter moves on.
    #[test]
    fn a_hunter_gives_up_a_marker_off_the_graph() {
        let mut bot = Bot::new("axis", true, 1);
        let high = [800.0, 0.0, 214.0];
        let mut v = bel_view(false, vec![high, [2000.0, 0.0, 64.0]]);
        v.origin = [780.0, 0.0, 64.0];
        for _ in 0..SHORT_TICKS {
            bot.walked_to = Some(high);
            bot.think(&v);
            assert_eq!(bot.goal(&v), Goal::To(high), "still walking at it");
        }
        // A plan not walked out (a way round still ahead) keeps the marker.
        let mut other = Bot::new("axis", true, 1);
        for _ in 0..=SHORT_TICKS {
            other.think(&v);
        }
        assert_eq!(other.goal(&v), Goal::To(high));
        bot.walked_to = Some(high);
        bot.think(&v);
        assert_eq!(bot.goal(&v), Goal::To([2000.0, 0.0, 64.0]), "given up");
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
        bot.wary = false;
        let spot = Goal::To([500.0 + 200.0 * MEMORY_LEAD_S, 0.0, 104.0]);
        assert_eq!(bot.goal(&v), spot);
        for _ in 1..bot.skill.memory_ticks {
            bot.think(&v);
        }
        assert_eq!(bot.goal(&v), spot, "forgotten early");
        bot.think(&v);
        assert_eq!(bot.goal(&v), Goal::Roam, "never forgotten");
    }

    /// A wary bot heads for the server's vantage over the spot, and gives
    /// the spot up before its memory of it runs out.
    #[test]
    fn a_wary_bot_hunts_from_the_vantage_and_gives_up_sooner() {
        let (mut bot, mut v) = lost([500.0, 0.0, 104.0], [0.0; 3]);
        assert_eq!(bot.hunt_query(), Some([500.0, 0.0, 104.0]));
        v.hunt = Some([400.0, 150.0, 64.0]);
        assert_eq!(bot.goal(&v), Goal::To([400.0, 150.0, 64.0]));
        let hunt = 70.min(bot.skill.memory_ticks);
        assert!(hunt < bot.skill.memory_ticks);
        for _ in 1..hunt {
            bot.think(&v);
        }
        assert_eq!(bot.goal(&v), Goal::To([400.0, 150.0, 64.0]));
        bot.think(&v);
        bot.think(&v);
        assert_eq!(bot.goal(&v), Goal::Roam, "hunted past its budget");
        assert_eq!(bot.hunt_query(), None);
        assert!(bot.recall.is_some(), "the spot is still remembered");
    }

    #[test]
    fn reaching_the_vantage_ends_the_hunt() {
        let (mut bot, mut v) = lost([500.0, 0.0, 104.0], [0.0; 3]);
        v.hunt = Some([400.0, 150.0, 64.0]);
        v.origin = [390.0, 140.0, 64.0];
        bot.think(&v);
        assert!(bot.recall.is_none());
        assert_eq!(bot.tactics()[2], 1);
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
        v.speed = vcod_common::pmove::SPEED_RUN;
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
        let heard = |n: &[Noise]| loudest(n, 1, at, 1.0);
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

    /// A waypoint off the walking view's heading: the view comes round at
    /// `walk_turn_deg` a tick while the keys, full, run the body at the
    /// waypoint all along.
    #[test]
    fn the_walking_view_comes_round_while_the_keys_keep_the_path() {
        let mut bot = Bot::new("allies", false, 1);
        let mut v = view();
        v.waypoint = Some([100.0, 0.0, 64.0]);
        let most = bot.skill.walk_turn_deg;
        let mut ticks = 0;
        loop {
            let cmd = bot.think(&v);
            let yaw = cmd.angles[1] as f32 / ANGLE2SHORT;
            let turn = yaw_diff(yaw, v.view[1]);
            assert!(turn.abs() <= most + 0.01, "turned {turn}");
            let way = yaw + (-(cmd.right as f32)).atan2(cmd.forward as f32).to_degrees();
            assert!(yaw_diff(way, 0.0).abs() < 1.0, "keys run at {way}");
            assert_eq!(
                cmd.forward.unsigned_abs().max(cmd.right.unsigned_abs()),
                127
            );
            v.view[1] = yaw;
            ticks += 1;
            if yaw_diff(yaw, 0.0).abs() < 0.01 {
                assert_eq!((cmd.forward, cmd.right), (127, 0));
                break;
            }
            assert!(ticks < 10);
        }
        assert_eq!(ticks, (90.0 / most).ceil() as i32);
    }

    /// A view a fight owned is handed back to the walk at once: the first
    /// walking tick after it faces the path, and the limit runs again
    /// from the tick after.
    #[test]
    fn the_walking_view_takes_the_path_at_once_after_a_fight() {
        let (mut bot, mut v) = engaged(1);
        // Enemy east, waypoint north, the view on the enemy.
        v.waypoint = Some([0.0, 100.0, 64.0]);
        bot.think(&v);
        bot.think(&v);
        // Out of sight, and out of mind, so nothing else owns the view.
        v.enemy = None;
        bot.skill.forget_ticks = 0;
        bot.disengage();
        bot.prior = None;
        bot.recall = None;
        let yaw = |c: &UserCmd| c.angles[1] as f32 / ANGLE2SHORT;
        let cmd = bot.think(&v);
        assert!(yaw_diff(yaw(&cmd), 90.0).abs() < 0.01, "yaw {}", yaw(&cmd));
        v.view[1] = yaw(&cmd);
        // The path turns south-east: the limit is back.
        v.waypoint = Some([100.0, -100.0, 64.0]);
        let cmd = bot.think(&v);
        let turn = yaw_diff(yaw(&cmd), v.view[1]).abs();
        assert!(
            (turn - bot.skill.walk_turn_deg).abs() < 0.01,
            "turned {turn}"
        );
    }

    /// On UO a bot sprints a long straight leg with breath to spare, at
    /// the full key (the sprint takes no weapon scale), and stops short of
    /// the leg's end, with an enemy in sight and once the bar runs low.
    #[test]
    fn a_uo_bot_sprints_a_long_straight_leg() {
        let mut bot = Bot::new("allies", false, 1);
        let mut v = view();
        v.waypoint = Some([0.0, 100.0, 64.0]);
        v.move_scale = 1.2;
        v.sprint_room = 600.0;
        let sprints = |c: &UserCmd| c.buttons & msg::BUTTON_SPRINT_V22 != 0;
        assert!(!sprints(&bot.think(&v)), "no sprint off UO");
        v.fatigue = Some(1.0);
        let cmd = bot.think(&v);
        assert!(sprints(&cmd));
        assert_eq!(cmd.forward, 127, "the full key");
        v.sprinting = true;
        v.sprint_room = 200.0;
        assert!(sprints(&bot.think(&v)), "carries on to the tail");
        v.sprint_room = 100.0;
        assert!(!sprints(&bot.think(&v)), "stops short of the end");
        v.sprinting = false;
        v.sprint_room = 600.0;
        v.fatigue = Some(0.3);
        assert!(!sprints(&bot.think(&v)), "no fresh sprint on a low bar");
        v.fatigue = Some(1.0);
        v.jump = true;
        assert!(!sprints(&bot.think(&v)), "not onto a jump edge");
    }

    /// A remembered spot past the end of a straight the bot has the breath
    /// to sprint is run at along the path; one short of it turns the view
    /// (seed 19 of `uo_bots_fight_and_sprint_in_tdm`).
    #[test]
    fn a_uo_bot_sprints_toward_a_spot_past_the_straight() {
        let sprints = |c: &UserCmd| c.buttons & msg::BUTTON_SPRINT_V22 != 0;
        let mut v = view();
        v.waypoint = Some([0.0, 100.0, 64.0]);
        v.sprint_room = 600.0;
        v.fatigue = Some(1.0);
        v.speed = vcod_common::pmove::SPEED_RUN;
        let think = |at: [f32; 3]| {
            let mut bot = Bot::new("allies", true, 1);
            bot.recall = Some(Recall {
                at,
                ticks: 100,
                seen: true,
            });
            bot.think(&v)
        };
        let cmd = think([400.0, 1200.0, 64.0]);
        assert!(sprints(&cmd), "far spot");
        assert_eq!((cmd.forward, cmd.right), (127, 0));
        assert!(
            !sprints(&think([300.0, 300.0, 64.0])),
            "spot short of the end"
        );
    }

    /// A remembered spot to the side turns the view off the path, but not
    /// on a CoD2 mantle edge: pmove offers the climb only to a view on the
    /// wall (bot-combat.md 17.1).
    #[test]
    fn a_recall_look_keeps_off_a_mantle_edge() {
        let mut v = view();
        v.waypoint = Some([0.0, 100.0, 64.0]);
        v.speed = vcod_common::pmove::SPEED_RUN;
        let think = |v: &BotView| {
            let mut bot = Bot::new("allies", true, 1);
            bot.recall = Some(Recall {
                at: [300.0, -300.0, 64.0],
                ticks: 100,
                seen: true,
            });
            bot.think(v)
        };
        assert_ne!(think(&v).right, 0, "the plain run turns to the spot");
        v.mantle = true;
        assert_eq!(think(&v).right, 0, "a mantle edge keeps the view");
    }

    /// Under [`RECALL_PACE`] the keys keep the path's run: turned off it,
    /// a back key and a strafe at retail's 0.7 and 0.8 scales left a body
    /// against mp_ship's deck corner at (3564, 8, 352) sliding along it at
    /// a few units a tick (bot-navigation.md, "The ship's leftovers").
    #[test]
    fn a_slow_body_keeps_the_path_under_a_recall() {
        let mut v = view();
        v.waypoint = Some([0.0, 100.0, 64.0]);
        let think = |v: &BotView| {
            let mut bot = Bot::new("allies", true, 1);
            bot.recall = Some(Recall {
                at: [300.0, -300.0, 64.0],
                ticks: 100,
                seen: true,
            });
            bot.think(v)
        };
        v.speed = RECALL_PACE - 1.0;
        assert_eq!(think(&v).right, 0, "slow: the path's own keys");
        v.speed = vcod_common::pmove::SPEED_RUN;
        assert_ne!(think(&v).right, 0, "at a run: turned to the spot");
    }

    /// Slowing onto a stop under a recall look scales the turned keys, so
    /// the body still heads for the stop (bot-combat.md 17.1).
    #[test]
    fn a_recall_look_slows_onto_a_stop_along_the_path() {
        let mut v = view();
        v.waypoint = Some([0.0, 24.0, 64.0]);
        v.stop = true;
        v.speed = vcod_common::pmove::SPEED_RUN;
        // Already turned to the spot.
        v.view = [0.0, -45.0, 0.0];
        let mut bot = Bot::new("allies", true, 1);
        bot.recall = Some(Recall {
            at: [300.0, -300.0, 64.0],
            ticks: 100,
            seen: true,
        });
        let cmd = bot.think(&v);
        // The keys in the world: the view's yaw turned by the keys' angle.
        let yaw = f32::from(cmd.angles[1] as i16) * 360.0 / 65536.0;
        let keys = f32::from(-cmd.right)
            .atan2(f32::from(cmd.forward))
            .to_degrees();
        let heading = yaw + keys;
        assert!(
            yaw_diff(heading, 90.0).abs() < 10.0,
            "heads for the stop: view {yaw}, keys ({}, {})",
            cmd.forward,
            cmd.right
        );
    }

    /// UO's frag cooks: the sight button goes down a few ticks into the
    /// pullback, and the hold stays well inside the 4 s fuse.
    #[test]
    fn a_uo_frag_is_spooned_after_the_pullback() {
        let mut bot = Bot::new("allies", true, 7);
        let mut v = view();
        v.grenade_cooks = true;
        v.weapon = 6;
        bot.stage = Stage::ToGrenade;
        let mut ticks = Vec::new();
        for _ in 0..60 {
            let cmd = bot.think(&v);
            ticks.push(cmd.buttons);
            if matches!(bot.stage, Stage::BackToRifle { .. }) {
                break;
            }
        }
        let held = ticks
            .iter()
            .filter(|b| *b & msg::BUTTON_ATTACK != 0)
            .count();
        let spoon = ticks.iter().position(|b| b & msg::BUTTON_ADS != 0);
        assert!(
            spoon.is_some_and(|i| i >= 3),
            "spooned after the pullback: {ticks:?}"
        );
        assert!(held * 50 < 2000, "held {held} ticks");
    }

    /// A light weapon's keys are cut to the graph's 190 units/s on the
    /// ground, and left whole on the rungs, where the climb takes no
    /// weapon scale.
    #[test]
    fn a_light_weapon_follows_at_the_graphs_pace() {
        let mut bot = Bot::new("allies", false, 1);
        let mut v = view();
        v.waypoint = Some([0.0, 100.0, 64.0]);
        v.move_scale = 1.2;
        assert_eq!(bot.think(&v).forward, 106, "127 / 1.2");
        v.move_scale = 0.9;
        assert_eq!(bot.think(&v).forward, 127, "a heavy one keeps the key");
        v.move_scale = 1.2;
        v.on_ground = false;
        v.on_ladder = true;
        v.waypoint = Some([0.0, 0.0, 300.0]);
        assert_eq!(bot.think(&v).forward, 127, "on the rungs");
    }

    /// Pinned at a ledge's face on a jump edge, as the graph's walk was
    /// (two ticks moved under 2 units with the key held at the top): jump,
    /// release the key the next tick (a held key never jumps again), then
    /// jump again after two more pinned ticks.
    #[test]
    fn a_bot_jumps_a_jump_edge_and_releases_the_key() {
        let mut bot = Bot::new("allies", false, 1);
        let mut v = view();
        v.waypoint = Some([30.0, 0.0, 96.0]);
        assert_eq!(bot.think(&v).up, 0, "a plain edge");
        v.jump = true;
        let ups: Vec<i8> = (0..4).map(|_| bot.think(&v).up).collect();
        assert_eq!(ups, [0, 127, 0, 127]);
    }

    /// Not closing on the top for one tick is no jump; the walk's second
    /// pinned tick is. Jumping on the first, a bot in mp_depot's pit at
    /// (-1569, 2087, 212) carried back off each landing jumped at the step
    /// at (-1533, 2112, 239) again and again (seed 1145, bot-navigation.md
    /// "Jumps").
    #[test]
    fn a_bot_jumps_on_its_second_tick_not_closing_on_the_top() {
        let mut bot = Bot::new("allies", false, 1);
        let mut v = view();
        v.jump = true;
        v.waypoint = Some([60.0, 0.0, 96.0]);
        v.velocity = [190.0, 0.0, 0.0];
        assert_eq!(bot.think(&v).up, 0, "running at it");
        // Carried away from the top by a landing, 4 units a tick.
        v.velocity = [-80.0, 0.0, 0.0];
        let ups: Vec<i8> = (0..2).map(|_| bot.think(&v).up).collect();
        assert_eq!(ups, [0, 127]);
    }

    /// Sliding along a ledge's face on the ground at a run, across the
    /// way to the top, is no pin, as the graph's walk measures it: the bot
    /// slides on to where it comes to rest and jumps there, where the walk
    /// jumped.
    #[test]
    fn a_bot_sliding_along_a_ledges_face_jumps_where_it_comes_to_rest() {
        let mut bot = Bot::new("allies", false, 1);
        let mut v = view();
        v.jump = true;
        v.waypoint = Some([20.0, 10.0, 96.0]);
        v.velocity = [0.0, 190.0, 0.0];
        let ups: Vec<i8> = [0.0, 9.5, 19.0, 28.5, 29.0, 29.0]
            .into_iter()
            .map(|y| {
                v.origin = [0.0, y, 64.0];
                bot.think(&v).up
            })
            .collect();
        assert_eq!(ups, [0, 0, 0, 0, 0, 127]);
    }

    /// Sliding along a face too steep to stand on, across the way to the
    /// top, the bot is not closing on it: two such ticks start the walk's
    /// bob, and it jumps on its next tick on the ground.
    #[test]
    fn a_bot_sliding_along_a_steep_face_jumps_out_of_the_bob() {
        let mut bot = Bot::new("allies", false, 1);
        let mut v = view();
        v.jump = true;
        v.waypoint = Some([40.0, 0.0, 96.0]);
        bot.think(&v);
        v.on_ground = false;
        v.sliding = true;
        v.velocity = [0.0, 100.0, 0.0];
        for i in 1..4 {
            v.origin = [0.0, 5.0 * i as f32, 64.0];
            assert_eq!(bot.think(&v).up, 0, "in the air");
        }
        v.on_ground = true;
        v.sliding = false;
        v.origin = [0.0, 20.0, 64.0];
        assert_eq!(bot.think(&v).up, 127, "out of the bob");
    }

    /// Set off from rest on a jump edge's foot (`nav::Follower::jump_wait`),
    /// the body moves little on its first tick: no jump until the key has
    /// been held against the waypoint for two ticks.
    #[test]
    fn a_bot_set_off_from_rest_is_not_pinned() {
        let mut bot = Bot::new("allies", false, 1);
        let mut v = view();
        v.stop = true;
        v.waypoint = Some([2.0, 0.0, 64.0]);
        assert_eq!(bot.think(&v).forward, 0, "at rest on the foot");
        v.stop = false;
        v.jump = true;
        v.waypoint = Some([30.0, 0.0, 84.0]);
        let ups: Vec<i8> = (0..3).map(|_| bot.think(&v).up).collect();
        assert_eq!(ups, [0, 0, 127]);
    }

    /// On a crouched jump edge the cmd after the jump holds crouch, with
    /// `upmove` -127, through the air, and lets it go on the ground.
    #[test]
    fn a_bot_on_a_crouched_jump_edge_crouches_in_the_air() {
        use vcod_common::net::msg::WBUTTON_CROUCH;
        let mut bot = Bot::new("allies", false, 1);
        let mut v = view();
        v.stop = true;
        v.waypoint = Some([2.0, 0.0, 64.0]);
        bot.think(&v);
        v.stop = false;
        v.jump = true;
        v.crouch_jump = true;
        v.waypoint = Some([30.0, 0.0, 84.0]);
        bot.think(&v);
        bot.think(&v);
        let jump = bot.think(&v);
        assert_eq!((jump.up, jump.wbuttons & WBUTTON_CROUCH), (127, 0));
        v.on_ground = false;
        for _ in 0..3 {
            let air = bot.think(&v);
            assert_eq!(
                (air.up, air.wbuttons & WBUTTON_CROUCH),
                (-127, WBUTTON_CROUCH)
            );
        }
        v.on_ground = true;
        v.jump = false;
        v.crouch_jump = false;
        assert_eq!(bot.think(&v).wbuttons & WBUTTON_CROUCH, 0, "landed");
    }

    /// A jump edge whose top is more than a jump below, the body still
    /// slowing from the last waypoint's pace: pinned only after two cmds
    /// with the key held at the new waypoint, as the graph's walk stalled.
    /// Read as pinned at once, a bot jumped off mp_ship's shelf at (5403,
    /// -160, 396) where the walk ran off its lip (bot-navigation.md
    /// section 3).
    #[test]
    fn a_bot_turning_for_a_top_below_is_not_pinned() {
        let mut bot = Bot::new("allies", false, 1);
        let mut v = view();
        v.waypoint = Some([-60.0, 0.0, 64.0]);
        assert_eq!(bot.think(&v).up, 0, "a plain edge west");
        // Slowing west, the jump edge's top east and 140 below.
        v.velocity = [-30.0, 0.0, 0.0];
        v.speed = 30.0;
        v.jump = true;
        v.waypoint = Some([60.0, -40.0, -76.0]);
        let ups: Vec<i8> = (0..3).map(|_| bot.think(&v).up).collect();
        assert_eq!(ups, [0, 0, 127], "the third cmd against the key jumps");
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

    /// A ladder whose rungs let go only past its top (CoD2's): a head a
    /// few units under the body and far across the face is climbed on to,
    /// where CoD1's rungs, which step off onto it, back down.
    #[test]
    fn a_far_head_over_a_ladder_that_tops_out_is_climbed_on_to() {
        let mut v = view();
        v.on_ladder = true;
        v.ladder_normal = [1.0, 0.0, 0.0];
        v.waypoint = Some([v.origin[0] - 77.0, v.origin[1] - 11.0, v.origin[2] - 7.0]);
        let backs = Bot::new("allies", false, 1).think(&v);
        assert!(backs.forward < 0, "CoD1's backs down: {}", backs.forward);
        v.tops_out = true;
        let climbs = Bot::new("allies", false, 1).think(&v);
        assert_eq!(
            (climbs.forward, climbs.angles[0]),
            (127, deg_short(-crate::nav::LADDER_PITCH))
        );
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

    /// A strafe held under a full-rate climb never turns the pace the grab
    /// carried in, and keeps the bleed off it: the key comes up until the
    /// body no longer slides away from the middle.
    #[test]
    fn a_climber_sliding_away_from_the_middle_lets_the_strafe_go() {
        let mut bot = Bot::new("allies", false, 1);
        let mut v = view();
        v.on_ladder = true;
        v.ladder_normal = [-1.0, 0.0, 0.0];
        v.waypoint = Some([40.0, -60.0, 300.0]);
        v.ladder_middle = Some([v.origin[0], v.origin[1] - 8.0]);
        v.velocity = [0.0, 30.0, 95.0];
        assert_eq!(bot.think(&v).right, 0, "sliding +y, the middle at -y");
        v.velocity = [0.0, -30.0, 95.0];
        assert!(bot.think(&v).right > 0, "sliding toward the middle");
        v.velocity = [0.0, 0.5, 95.0];
        assert!(bot.think(&v).right > 0, "all but still");
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
        assert_eq!(first.forward, -crate::nav::CREEP);
        assert_eq!(first.angles[1], deg_short(-180.0));
        // Over the lip and past the foot's column: same heading.
        v.origin[0] = 21.0;
        let past = bot.think(&v);
        assert_eq!(
            (past.forward, past.angles[1]),
            (first.forward, first.angles[1])
        );
    }

    /// A waypoint 50 under the feet is run at, unless the graph's run off
    /// that drop missed: then it is backed toward, as the graph proved it
    /// (`BotView::back_drop`). A body on the rungs told to hold presses no
    /// key (`BotView::rungs_hold`).
    #[test]
    fn a_drop_the_graph_backed_down_is_backed_and_a_held_climber_presses_nothing() {
        let mut bot = Bot::new("allies", false, 1);
        let mut v = view();
        // 50 under the feet at 64.
        v.waypoint = Some([20.0, 0.0, 14.0]);
        assert!(bot.think(&v).forward > 0, "run at");
        v.back_drop = true;
        assert!(bot.think(&v).forward < 0, "backed toward");
        let mut bot = Bot::new("allies", false, 1);
        let mut v = view();
        v.on_ladder = true;
        v.waypoint = Some([0.0, 0.0, 200.0]);
        assert_ne!(bot.think(&v).forward, 0, "climbs");
        v.rungs_hold = true;
        v.stop = true;
        let cmd = bot.think(&v);
        assert_eq!((cmd.forward, cmd.right), (0, 0), "holds");
    }

    /// mp_ship's crow's nest: backed toward the foot from a few units off
    /// the rungs' line, bots slid past the rungs down the hatch (seeds
    /// 1919, 3941, 4161, 4545). Off the head of a ladder whose fall kills
    /// the creep faces the rungs and strafes to their middle.
    #[test]
    fn a_creep_off_a_deadly_head_faces_the_rungs_and_strafes_to_them() {
        let mut bot = Bot::new("allies", false, 1);
        let mut v = view();
        // The rungs face -x at x 4975, y 59 to 69; the body is 8 south.
        v.origin = [4963.9, 56.0, 1219.125];
        v.waypoint = Some([4959.0, 64.0, 692.125]);
        v.back_face = Some((0.0, [4959.0, 64.0]));
        let cmd = bot.think(&v);
        assert_eq!(cmd.forward, -crate::nav::CREEP);
        assert_eq!(cmd.angles[1], deg_short(0.0));
        // Facing +x, the middle at +y is to the left.
        assert!(
            cmd.right < 0 && cmd.right >= -crate::nav::CREEP,
            "{}",
            cmd.right
        );
        v.origin[1] = 64.0;
        assert_eq!(bot.think(&v).right, 0, "on the line");
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

    /// A creep onto a ladder below a lip covers under a unit a tick, short
    /// of the 15-unit stall circle in ten: mp_depot's back down over the
    /// 1-unit ladder at (-2750, -1217, 504) read as stuck, and the spell
    /// walked off the roof (bot-navigation.md section 3). A creep that stops
    /// is stuck all the same.
    #[test]
    fn a_creep_is_not_stuck_until_it_stops() {
        let mut bot = Bot::new("allies", false, 1);
        let mut v = view();
        v.waypoint = Some([20.0, 0.0, -236.0]);
        for _ in 0..30 {
            let cmd = bot.think(&v);
            assert_eq!(cmd.forward, -crate::nav::CREEP);
            assert!(!bot.unsticking(), "creeping at {:?}", v.origin);
            v.origin[0] += 0.7;
        }
        let stuck = (0..12).any(|_| {
            bot.think(&v);
            bot.unsticking()
        });
        assert!(stuck, "a creep held still never unsticks");
    }

    /// mp_ship's crow's nest: a bot backing from the platform for the deck
    /// 527 below pins on the hatch's rim and slides along it at under a
    /// unit a tick before it drops through onto the rungs. That gains on
    /// the waypoint and is no stall; backing in place is.
    #[test]
    fn a_back_down_sliding_along_a_rim_is_not_stuck() {
        let mut bot = Bot::new("allies", false, 1);
        let mut v = view();
        v.origin = [4954.1, 92.0, 1219.125];
        v.waypoint = Some([4929.0, 68.0, 692.125]);
        for _ in 0..30 {
            let cmd = bot.think(&v);
            assert_eq!(cmd.forward, -127);
            assert!(!bot.unsticking(), "sliding at {:?}", v.origin);
            v.origin[1] -= 0.6;
        }
        let stuck = (0..12).any(|i| {
            v.origin[0] += if i % 2 == 0 { 0.5 } else { -0.5 };
            bot.think(&v);
            bot.unsticking()
        });
        assert!(stuck, "a back down held on the rim never unsticks");
    }

    /// On the rungs a head across the face just below is climbed over, a
    /// floor on the climber's side below is backed down to
    /// (`tests/bots.rs`, `a_bot_climbs_over_mp_ships_hold_ladder_top`).
    #[test]
    fn a_head_across_the_face_is_climbed_over() {
        let mut bot = Bot::new("allies", false, 1);
        let mut v = view();
        v.on_ladder = true;
        v.on_ground = false;
        // The face at y 15, its normal toward the climber.
        v.ladder_normal = [0.0, -1.0, 0.0];
        v.waypoint = Some([v.origin[0], v.origin[1] + 6.0, v.origin[2] - 2.0]);
        assert_eq!(bot.think(&v).forward, 127);
        v.waypoint = Some([v.origin[0], v.origin[1] - 1.0, v.origin[2] - 2.0]);
        assert_eq!(bot.think(&v).forward, -127);
    }

    /// A path that drops licenses a drop as deep as its own, not a fall
    /// that hurts below it: mp_depot's bots backing down the ladder from
    /// the roof at 508 to its foot at 326 took unstick headings off the
    /// roof's edge to the yard at -24 and died (section 3).
    #[test]
    fn a_path_down_licenses_no_deeper_fall() {
        let bot = Bot::new("allies", false, 1);
        let mut v = view();
        v.waypoint = Some([20.0, 0.0, v.origin[2] - 180.0]);
        v.drop_ahead[0] = Some(150.0);
        v.drop_ahead[2] = Some(f32::INFINITY);
        v.drop_ahead[4] = Some(500.0);
        assert!(!bot.shuns(&v, 0.0), "a drop the path takes anyway");
        assert!(bot.shuns(&v, 90.0), "a fall that hurts");
        assert!(bot.shuns(&v, 180.0), "a fall that hurts");
        v.waypoint = Some([20.0, 0.0, v.origin[2] - 600.0]);
        assert!(!bot.shuns(&v, 180.0), "a path that drops as far");
    }

    /// The server feels for drops and walls only on the ticks
    /// [`Bot::reads_drops`] and [`Bot::reads_walls`] name: a bot fed them
    /// every tick and one fed them only then press the same keys, stuck
    /// on the way to a waypoint, on its spells and wandering with none.
    #[test]
    fn drops_are_felt_only_when_the_brain_reads_them() {
        let mut drops = [Some(100.0); 8];
        drops[2] = None;
        let walls = [true, false, true, true, false, true, false, true];
        for seed in 1..20 {
            let (mut every, mut lazy) = (
                Bot::new("allies", false, seed),
                Bot::new("allies", false, seed),
            );
            let mut v = view();
            let mut skipped = 0;
            for tick in 0..120 {
                v.waypoint = (tick < 60).then_some([0.0, 100.0, 64.0]);
                v.drop_ahead = drops;
                v.wall_ahead = walls;
                let want = every.think(&v);
                if !lazy.reads_drops(&v) {
                    v.drop_ahead = [None; 8];
                    skipped += 1;
                }
                if !lazy.reads_walls(&v) {
                    v.wall_ahead = [false; 8];
                }
                assert_eq!(lazy.think(&v), want, "seed {seed} tick {tick}");
            }
            assert!(skipped > 10, "seed {seed}: felt every tick");
        }
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

    /// A climb that bobs under a ladder's top lets go of the rungs and
    /// stands on the head's plate in the stall window: the spell holds still
    /// there, where a heading ran bots down mp_ship's crow's nest hatch. So
    /// does a body in the air just off the rungs with no waypoint.
    #[test]
    fn a_spell_just_off_the_rungs_holds_still() {
        let mut bot = Bot::new("allies", false, 1);
        let mut v = view();
        v.waypoint = Some([0.0, 100.0, 64.0]);
        v.on_ground = false;
        v.on_ladder = true;
        bot.think(&v);
        v.on_ground = true;
        v.on_ladder = false;
        let mut keys = Vec::new();
        for _ in 0..12 {
            bot.think(&v);
        }
        v.on_ground = false;
        v.on_ladder = true;
        bot.think(&v);
        v.on_ground = true;
        v.on_ladder = false;
        for _ in 0..12 {
            let cmd = bot.think(&v);
            if bot.unstick_ticks > 0 {
                keys.push(cmd.forward);
            }
        }
        assert!(!keys.is_empty() && keys.iter().all(|&k| k == 0), "{keys:?}");
        // Off the ground with no waypoint, the same.
        let mut bot = Bot::new("allies", false, 1);
        let mut v = view();
        v.on_ground = false;
        v.on_ladder = true;
        bot.think(&v);
        v.on_ladder = false;
        assert_eq!(bot.think(&v).forward, 0);
        // Away from the rungs a spell walks.
        let mut bot = Bot::new("allies", false, 1);
        let mut v = view();
        v.waypoint = Some([0.0, 100.0, 64.0]);
        let walked = (0..30).any(|_| {
            let cmd = bot.think(&v);
            bot.unstick_ticks > 0 && cmd.forward != 0
        });
        assert!(walked);
    }

    /// A body that runs 20 units out and back within the ten ticks left
    /// the stall circle: no spell. One that never left it gets one.
    #[test]
    fn out_and_back_is_no_stall() {
        let mut bot = Bot::new("allies", false, 1);
        let mut v = view();
        v.waypoint = Some([0.0, 300.0, 64.0]);
        for tick in 0..60 {
            let x = [0.0, 10.0, 20.0, 10.0, 0.0][tick % 5];
            v.origin = [x, 0.0, 64.0];
            bot.think(&v);
            assert_eq!(bot.unstick_ticks, 0, "tick {tick}");
        }
        v.origin = [0.0, 0.0, 64.0];
        assert!((0..30).any(|_| {
            bot.think(&v);
            bot.unstick_ticks > 0
        }));
    }

    /// A climb bobbing under a ladder's top, on and off the rungs right
    /// under its waypoint, is not stuck: no spell.
    #[test]
    fn a_climb_bobbing_under_the_top_is_not_stuck() {
        let mut bot = Bot::new("allies", false, 1);
        let mut v = view();
        v.waypoint = Some([4.0, 0.0, 84.0]);
        v.on_ground = false;
        for tick in 0..40 {
            v.on_ladder = tick % 6 < 3;
            bot.think(&v);
            assert_eq!(bot.unstick_ticks, 0, "tick {tick}");
        }
        // Far under it, pinned under a spar, it is.
        v.waypoint = Some([4.0, 0.0, 164.0]);
        assert!((0..40).any(|tick| {
            v.on_ladder = tick % 6 < 3;
            bot.think(&v);
            bot.unstick_ticks > 0
        }));
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

    /// mp_ship's hold ladder head, seed 149: walls north and east, the
    /// shaft south. A heading into a wall reads as no drop, and the bot
    /// stood pinned on one spell after spell; the open one west comes
    /// first.
    #[test]
    fn a_stuck_bot_takes_a_heading_into_a_wall_last() {
        for seed in 1..20 {
            let mut bot = Bot::new("allies", false, seed);
            let mut v = view();
            v.wall_ahead = [true, true, true, true, false, false, false, true];
            v.drop_ahead = [
                None,
                None,
                None,
                None,
                None,
                Some(500.0),
                Some(500.0),
                Some(500.0),
            ];
            bot.pick_heading(&v);
            assert_eq!(octant(bot.heading), 4, "seed {seed}: {}", bot.heading);
            // Walled in but for drops: a wall it is.
            v.wall_ahead[4] = true;
            bot.pick_heading(&v);
            assert!(octant(bot.heading) < 5, "seed {seed}: {}", bot.heading);
        }
    }

    /// Pinned through a spell, the bot picks a new heading but the spell
    /// still ends on time and the waypoint gets its turn. Chained spells
    /// held one on mp_ship's hold ladder head for the rest of the run
    /// while its waypoint lay a walk away.
    /// A push-off jumps facing the rungs, then brakes facing the same way
    /// until the body lands (`nav::push_off`).
    #[test]
    fn a_push_off_jumps_then_brakes_until_it_lands() {
        let mut bot = Bot::new("allies", false, 1);
        let mut v = view();
        v.on_ladder = true;
        v.on_ground = false;
        v.ladder_normal = [0.0, -1.0, 0.0];
        v.origin = [2146.5, -214.1, 103.0];
        v.waypoint = Some([2144.7, -229.9, 104.1]);
        v.push_off = Some(90.0);
        let out = cmd_angles(&v, [0.0, 90.0]);
        let cmd = bot.think(&v);
        assert_eq!((cmd.up, cmd.forward, cmd.angles), (127, 0, out));
        v.on_ladder = false;
        v.push_off = None;
        v.origin = [2146.5, -230.0, 120.0];
        for _ in 0..5 {
            let cmd = bot.think(&v);
            assert_eq!((cmd.up, cmd.forward, cmd.angles), (0, 127, out));
        }
        // Landed facing the waypoint, it runs on.
        v.on_ground = true;
        v.view[1] = (-229.9f32 + 230.0).atan2(2144.7 - 2146.5).to_degrees();
        assert_eq!(bot.think(&v).forward, 127);
    }

    /// An unpinning heading is held for its ticks, then the bot heads at
    /// the waypoint again.
    #[test]
    fn an_unpinning_heading_is_held_for_its_ticks() {
        let mut bot = Bot::new("allies", false, 1);
        let mut v = view();
        v.waypoint = Some([0.0, 100.0, 64.0]);
        let toward = bot.think(&v).angles;
        v.unpin = Some(180.0);
        let aside = cmd_angles(&v, [0.0, 180.0]);
        assert_eq!(bot.think(&v).angles, aside);
        v.unpin = None;
        for _ in 1..crate::nav::UNPIN_TICKS {
            assert_eq!(bot.think(&v).angles, aside);
        }
        assert_eq!(bot.think(&v).angles, toward);
    }

    /// Held still off the ground (mp_hurtgen's crease at (437, 2824, -97))
    /// for [`WEDGE_TICKS`], the bot asks for a `kill`; on the ground, on
    /// the rungs or moving, never.
    #[test]
    fn a_body_held_in_the_air_asks_for_a_kill() {
        let mut bot = Bot::new("allies", false, 1);
        let mut v = view();
        v.waypoint = Some([0.0, 100.0, 64.0]);
        let ticks = |bot: &mut Bot, v: &BotView| {
            (1..=2 * WEDGE_TICKS).find(|_| {
                bot.think(v);
                bot.take_kill()
            })
        };
        assert_eq!(ticks(&mut bot, &v), None, "on the ground");
        v.on_ground = false;
        v.on_ladder = true;
        assert_eq!(ticks(&mut bot, &v), None, "on the rungs");
        v.on_ladder = false;
        assert_eq!(ticks(&mut bot, &v), Some(WEDGE_TICKS));
        assert!(!bot.take_kill(), "asked once");
        for _ in 0..2 * WEDGE_TICKS {
            v.origin[2] -= 2.0;
            bot.think(&v);
            assert!(!bot.take_kill(), "falling");
        }
        // A round's one life is worth more than the crease costs.
        v.one_life = true;
        assert_eq!(ticks(&mut bot, &v), None, "one life a round");
    }

    /// mp_ship, seed 577: a bot slid off the bow roof into the gutter at
    /// (1955, 281, 312), whose nodes lead nowhere: north is a fall of 376,
    /// south the roof's slope it slides back down. The wander keeps off
    /// the fall for good, so the bot asks for a kill, as in a crease.
    #[test]
    fn a_body_stranded_off_the_graph_asks_for_a_kill() {
        let mut bot = Bot::new("allies", false, 1);
        let mut v = view();
        v.stranded = true;
        let ticks = |bot: &mut Bot, v: &BotView| {
            (1..=2 * STRAND_TICKS).find(|_| {
                bot.think(v);
                bot.take_kill()
            })
        };
        assert_eq!(ticks(&mut bot, &v), Some(STRAND_TICKS));
        v.one_life = true;
        assert_eq!(ticks(&mut bot, &v), None, "one life a round");
        v.one_life = false;
        v.stranded = false;
        assert_eq!(ticks(&mut bot, &v), None, "on the graph");
    }

    #[test]
    fn a_pinned_spell_hands_back_to_the_waypoint() {
        let mut bot = Bot::new("allies", false, 1);
        let mut v = view();
        v.waypoint = Some([0.0, 100.0, 64.0]);
        let toward = bot.think(&v).angles[1];
        let start = (0..25).find(|_| {
            bot.think(&v);
            bot.unsticking()
        });
        assert!(start.is_some(), "never unstuck");
        let back = (0..UNSTICK_TICKS + 1).any(|_| bot.think(&v).angles[1] == toward);
        assert!(back, "spell after spell, never the waypoint");
    }

    /// mp_ship's ladder at x 3792: a climb pinned under another bot takes
    /// a spell down the rungs, then climbs back. A stall window that
    /// straddles the turn reads the way down and the way back up as
    /// standing still, and spell followed spell down the ladder.
    #[test]
    fn a_spell_end_starts_the_stall_window_afresh() {
        let mut bot = Bot::new("allies", false, 1);
        let mut v = view();
        v.on_ladder = true;
        v.on_ground = false;
        v.ladder_normal = [-1.0, 0.0, 0.0];
        v.waypoint = Some([3783.0, 64.0, 623.0]);
        v.origin = [3808.0, 67.0, 511.0];
        let start = (0..25).find(|_| {
            bot.think(&v);
            bot.unsticking()
        });
        assert!(start.is_some(), "never unstuck");
        // Down the rungs for the spell at the climb's rate, then back up.
        while bot.unsticking() {
            v.origin[2] -= 2.67;
            bot.think(&v);
        }
        for tick in 0..UNSTICK_TICKS {
            v.origin[2] += 2.67;
            bot.think(&v);
            assert!(!bot.unsticking(), "a new spell {tick} ticks after the last");
        }
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

    /// A sidestep is held until reached, blocked or stale, so a teammate
    /// moving across the line does not swing the bot between spots.
    #[test]
    fn a_sidestep_is_held_until_reached_or_blocked() {
        let spot = [64.0, 0.0, 0.0];
        let at = glam::Vec3::ZERO;
        let all = |_: glam::Vec3| true;
        assert_eq!(held_sidestep(Some((spot, 1000)), 1500, at, all), Some(spot));
        assert_eq!(held_sidestep(None, 1500, at, all), None);
        let near = glam::Vec3::new(56.0, 4.0, 18.0);
        assert_eq!(held_sidestep(Some((spot, 1000)), 1500, near, all), None);
        assert_eq!(held_sidestep(Some((spot, 1000)), 1500, at, |_| false), None);
        let stale = 1000 + SIDESTEP_HOLD_MS;
        assert_eq!(held_sidestep(Some((spot, 1000)), stale, at, all), None);
    }

    /// The sight button stays down through a rechamber: 1.1 keeps the
    /// scope up through it, and 1.5 and UO lower it for the bolt and raise
    /// it again on the held button (cod15-movement.md 6), as a player's.
    /// No round leaves while the sight comes back up.
    #[test]
    fn a_scoped_bot_holds_the_sight_through_a_rechamber() {
        let (mut bot, mut v) = engaged(3);
        v.sniper = true;
        v.enemy.as_mut().unwrap().origin = [1000.0, 0.0, 64.0];
        for (busy, frac) in [(0, 1.0), (900, 1.0), (500, 0.0), (0, 0.4)] {
            v.busy_ms = busy;
            v.ads_frac = frac;
            let cmd = bot.think(&v);
            assert_ne!(cmd.buttons & BUTTON_ADS, 0, "sight let go at busy {busy}");
            if frac < 1.0 {
                assert_eq!(
                    cmd.buttons & BUTTON_ATTACK,
                    0,
                    "fired with the sight at {frac}"
                );
            }
        }
    }

    /// A scoped bot puts the swayed gun on the target, not the view, and
    /// fires on the view its last cmd left, which the round leaves along
    /// (combat doc 15.1). The old aim took the whole offset back at once;
    /// the hand takes `recoil_share` of it, `recoil_ticks` late, and fires
    /// once that brings the gun inside the cone.
    #[test]
    fn a_scoped_bot_aims_the_swayed_gun() {
        for human in [false, true] {
            let (mut bot, mut v) = engaged(3);
            bot.human_aim = human;
            bot.skill.error_start = 0.0;
            bot.skill.error_floor = 0.0;
            v.sniper = true;
            v.fire_time_ms = 1300;
            v.ads_frac = 1.0;
            v.enemy.as_mut().unwrap().origin = [1000.0, 0.0, 64.0];
            let (tp, ty) = aim_angles(&v, [1000.0, 0.0, 64.0]);
            v.gun_offset = [1.0, -1.2];
            // The view on the chest: the gun is off it past a bolt rifle's
            // reach, so no shot.
            v.view = [tp, ty, 0.0];
            let mut fired = None;
            for tick in 0..20 {
                let cmd = bot.think(&v);
                if cmd.buttons & BUTTON_ATTACK != 0 {
                    fired = Some((tick, v.view));
                    break;
                }
                let a = cmd.angles.map(|a| a as f32 / ANGLE2SHORT);
                v.view = [a[0], a[1], 0.0];
            }
            let (tick, at) = fired.expect("never fired");
            let miss = yaw_diff(at[0] + 1.0, tp).hypot(yaw_diff(at[1] - 1.2, ty));
            if human {
                assert!(
                    tick > bot.skill.recoil_ticks as usize,
                    "fired on tick {tick}"
                );
                let cone = (HIT_RADIUS / 1000.0).atan().to_degrees();
                assert!(miss < cone, "the gun missed the chest by {miss} degrees");
                assert!(miss > 0.1, "took the whole offset back: {miss}");
            } else {
                assert!(tick > 0, "fired with the gun off the chest");
                assert!(miss < 0.01, "the gun missed the chest by {miss} degrees");
            }
        }
    }

    /// An LMG bot holds the sight button to deploy, keeps its feet still,
    /// and fires only once the bipod is down and the sight up; a deploy
    /// the engine refuses lets the button go for a rest.
    #[test]
    fn an_lmg_bot_deploys_its_bipod_before_it_fires() {
        let (mut bot, mut v) = engaged(3);
        v.lmg = true;
        v.automatic = true;
        v.enemy.as_mut().unwrap().origin = [800.0, 0.0, 60.0];
        let cmd = bot.think(&v);
        assert_ne!(cmd.buttons & BUTTON_ADS, 0, "the press deploys");
        assert_eq!(cmd.buttons & BUTTON_ATTACK, 0, "no shot off the bipod");
        assert_eq!((cmd.forward, cmd.right), (0, 0));
        v.deployed = true;
        v.ads_frac = 1.0;
        let fired = (0..20).any(|_| {
            let cmd = bot.think(&v);
            // The sim takes the view where the cmd points it.
            let a = cmd.angles.map(|a| a as f32 / ANGLE2SHORT);
            v.view = [a[0], a[1], 0.0];
            cmd.buttons & BUTTON_ATTACK != 0
        });
        assert!(fired, "a deployed LMG fires");
        // Refused: the button rests, then tries again.
        v.deployed = false;
        v.ads_frac = 0.0;
        let held: Vec<bool> = (0..LMG_TRY_TICKS + 3)
            .map(|_| bot.think(&v).buttons & BUTTON_ADS != 0)
            .collect();
        assert!(held[0] && !*held.last().unwrap(), "{held:?}");
    }

    /// A target that left the yaw cap round the deploy's facing breaks the
    /// bipod down, for a fresh deploy facing it.
    #[test]
    fn an_lmg_target_past_the_yaw_cap_breaks_the_bipod_down() {
        let (mut bot, mut v) = engaged(3);
        v.lmg = true;
        v.deployed = true;
        v.ads_frac = 1.0;
        v.deploy_yaw = -90.0;
        let cmd = bot.think(&v);
        assert_eq!(cmd.buttons & (BUTTON_ADS | BUTTON_ATTACK), 0);
    }

    /// A target past the deployed view's pitch clamp breaks the bipod down
    /// rather than firing from the pinned view under the line: on mp_foy
    /// (bas, seed 21) a planter in the allied charge room emptied an MG34
    /// at a defender 53 degrees up through the hole over it, every round
    /// into the lip (bot-objectives.md 7.2).
    #[test]
    fn an_lmg_target_past_the_pitch_clamp_breaks_the_bipod_down() {
        let (mut bot, mut v) = engaged(3);
        v.lmg = true;
        v.automatic = true;
        v.deployed = true;
        v.ads_frac = 1.0;
        v.enemy.as_mut().unwrap().origin = [150.0, 0.0, 324.0];
        for _ in 0..20 {
            let cmd = bot.think(&v);
            assert_eq!(cmd.buttons & (BUTTON_ADS | BUTTON_ATTACK), 0);
            // The clamp pins the view 45 up.
            let a = cmd.angles.map(|a| a as f32 / ANGLE2SHORT);
            v.view = [a[0].max(-45.0), a[1], 0.0];
        }
    }

    /// The clamp measures from `proneTorsoPitch`, not level: prone on a
    /// slope rising toward him, a target 53 degrees up lies 33 off the
    /// torso and is fired at; laid down a slope, one 35 up lies 55 off and
    /// breaks the bipod down (coduo-movement.md 7).
    #[test]
    fn an_lmg_pitch_clamp_measures_from_the_prone_torso() {
        let fire = |torso: f32, z: f32| {
            let (mut bot, mut v) = engaged(3);
            v.lmg = true;
            v.automatic = true;
            v.deployed = true;
            v.ads_frac = 1.0;
            v.lmg_pitch_from = torso;
            v.enemy.as_mut().unwrap().origin = [150.0, 0.0, z];
            (0..20).any(|_| {
                let cmd = bot.think(&v);
                let a = cmd.angles.map(|a| a as f32 / ANGLE2SHORT);
                v.view = [a[0].clamp(torso - 45.0, torso + 45.0), a[1], 0.0];
                cmd.buttons & BUTTON_ATTACK != 0
            })
        };
        assert!(fire(-20.0, 324.0));
        assert!(!fire(0.0, 324.0));
        assert!(!fire(20.0, 229.0));
        assert!(fire(0.0, 229.0));
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

    /// Back to a target it let go of, the bot keeps most of its settled
    /// aim and a third of the delay; onto one who just hit it, half the
    /// delay and three quarters of the first error.
    #[test]
    fn a_known_target_is_taken_again_quicker() {
        let (mut bot, v) = engaged(7);
        bot.skill.reaction_min = 9;
        let e = v.enemy.unwrap();
        bot.track(&v);
        bot.target.as_mut().unwrap().error = bot.skill.error_floor;
        let mut other = engaged(7).1;
        other.enemy = Some(EnemyView { slot: 4, ..e });
        bot.track(&other);
        let t = bot.target.as_ref().unwrap();
        assert_eq!((t.slot, t.react, t.error), (4, 9, bot.skill.error_start));
        bot.track(&v);
        let t = bot.target.as_ref().unwrap();
        assert_eq!((t.slot, t.react), (3, 3));
        assert_eq!(t.error, bot.skill.error_floor * REACQUIRE_ERROR);

        let (mut bot, mut v) = engaged(7);
        bot.skill.reaction_min = 9;
        v.hurt = Some(Hurt {
            slot: 3,
            ago_ms: 50,
            dir: [0.0, 1.0, 0.0],
        });
        bot.track(&v);
        let t = bot.target.as_ref().unwrap();
        assert_eq!(t.react, 4);
        assert_eq!(t.error, bot.skill.error_start * HIT_BY_ERROR);
    }

    #[test]
    fn the_attacker_then_the_held_target_then_the_nearest() {
        let e = |slot| EnemyView {
            slot,
            origin: [0.0; 3],
            velocity: [0.0; 3],
        };
        // Slots 1, 2, 3 at 100, 200, 300 units; slot 3 out of sight.
        let order = [(1e4, 1), (4e4, 2), (9e4, 3)];
        let seen = |i: usize| (order[i].1 != 3).then(|| e(order[i].1));
        let pick = |a, t| pick_enemy(&order, a, t, seen).map(|e| e.slot);
        assert_eq!(pick(None, None), Some(1));
        assert_eq!(pick(Some(2), Some(1)), Some(2));
        // Unseen, the attacker gives way.
        assert_eq!(pick(Some(3), None), Some(1));
        // Slot 1 is under 0.6 of the held slot 2's range; nothing is
        // nearer than 0.6 of slot 1's.
        assert_eq!(pick(None, Some(2)), Some(1));
        let order = [(2e4, 1), (4e4, 2)];
        let seen = |i: usize| Some(e(order[i].1));
        assert_eq!(
            pick_enemy(&order, None, Some(2), seen).map(|e| e.slot),
            Some(2)
        );
    }

    #[test]
    fn the_sight_cone_is_about_the_view() {
        let eye = glam::Vec3::ZERO;
        let ahead = glam::Vec3::new(100.0, 0.0, 0.0);
        let side = glam::Vec3::new(0.0, 100.0, 0.0);
        assert!(in_sight_cone(eye, [0.0; 3], ahead, 120.0));
        assert!(!in_sight_cone(eye, [0.0; 3], side, 120.0));
        assert!(in_sight_cone(eye, [0.0, 90.0, 0.0], side, 120.0));
        assert!(in_sight_cone(eye, [0.0; 3], side, 181.0));
        assert!(in_sight_cone(eye, [0.0; 3], -ahead, 360.0));
        // Pitch is down positive: a view 80 down misses a level target.
        assert!(!in_sight_cone(eye, [80.0, 0.0, 0.0], ahead, 120.0));
    }

    /// Shot from behind by nobody in sight, the bot comes round toward
    /// where the round came from at its turn rate.
    #[test]
    fn a_hit_from_out_of_sight_turns_the_view_toward_it() {
        let (mut bot, mut v) = engaged(3);
        v.enemy = None;
        v.view = [0.0, 0.0, 0.0];
        // From an attacker at -x toward the body.
        v.hurt = Some(Hurt {
            slot: 5,
            ago_ms: 50,
            dir: [1.0, 0.0, 0.0],
        });
        let mut yaw = 0.0f32;
        for _ in 0..20 {
            v.view = [0.0, yaw, 0.0];
            let next = bot.think(&v).angles[1] as f32 / ANGLE2SHORT;
            assert!(yaw_diff(next, yaw).abs() <= bot.skill.turn_deg + 0.01);
            yaw = next;
        }
        assert!(yaw_diff(180.0, yaw).abs() < 1.0, "never came round: {yaw}");
        // Past the look window the hit no longer steers.
        v.hurt.as_mut().unwrap().ago_ms = HURT_LOOK_MS;
        let mut cmd = NULL_USERCMD;
        assert!(bot.face_hurt(&v, &mut cmd, 0.0).is_none());
    }

    /// A flick turns `turn_deg` a tick on the mean and 1.875 times that
    /// at its peak (the drift moves the want a little besides).
    #[test]
    fn the_view_turns_a_bounded_step_per_tick() {
        let (mut bot, mut v) = engaged(3);
        // The enemy is behind: yaw 180 from a view at 0.
        v.enemy = Some(EnemyView {
            slot: 3,
            origin: [-500.0, 0.0, 124.0],
            velocity: [0.0; 3],
        });
        let step = 1.875 * bot.skill.turn_deg + 1.0;
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

    /// A crouched bot's round leaves 40 over its feet, not 60: aimed off
    /// the standing eye, a crouched sniper put every round 20 units low at
    /// 900 and hit legs and low walls (bot-objectives.md 2).
    #[test]
    fn a_crouched_bot_aims_from_its_own_eye() {
        let mut v = view();
        v.eye_height = vcod_common::pmove::VIEW_CROUCH;
        let level = [0.0, 900.0, 64.0 + vcod_common::pmove::VIEW_CROUCH];
        let (pitch, yaw) = aim_angles(&v, level);
        assert!(pitch.abs() < 0.01, "level with the crouched eye: {pitch}");
        assert!((yaw - 90.0).abs() < 0.01);
        // The chest follows the enemy's stance too.
        assert_eq!(chest([0.0; 3], vcod_common::pmove::VIEW_STAND)[2], 40.0);
        assert!(chest([0.0; 3], vcod_common::pmove::VIEW_CROUCH)[2] < 30.0);
    }

    /// A wall 50 high between a level bot and a standing enemy covers the
    /// chest (40) but not the eye (60): the bot aims at the head.
    #[test]
    fn a_chest_behind_cover_is_aimed_over() {
        let (chest, eye) = ([500.0, 0.0, 40.0], [500.0, 0.0, 60.0]);
        let over_wall = |p: [f32; 3]| p[2] > 50.0;
        assert_eq!(aim_point(chest, eye, over_wall), Some(eye));
        assert_eq!(aim_point(chest, eye, |_| true), Some(chest));
        assert_eq!(aim_point(chest, eye, |_| false), None);
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
    fn a_teammate_across_the_line_holds_fire() {
        use glam::Vec3;
        let body = (Vec3::new(40.0, -15.0, 0.0), Vec3::new(70.0, 15.0, 70.0));
        let (eye, chest) = (Vec3::new(0.0, 0.0, 60.0), Vec3::new(100.0, 0.0, 40.0));
        assert!(friend_in_line(eye, chest, &[body]));
        // Beside the line by more than the margin, or behind the target.
        let beside = (body.0 + Vec3::Y * 40.0, body.1 + Vec3::Y * 40.0);
        let behind = (body.0 + Vec3::X * 100.0, body.1 + Vec3::X * 100.0);
        assert!(!friend_in_line(eye, chest, &[beside, behind]));

        for automatic in [false, true] {
            let (mut bot, mut v) = engaged(9);
            v.automatic = automatic;
            v.friend_in_line = true;
            for tick in 0..40 {
                let cmd = bot.think(&v);
                assert_eq!(cmd.buttons & BUTTON_ATTACK, 0, "fired on tick {tick}");
            }
            v.friend_in_line = false;
            assert!((0..40).any(|_| bot.think(&v).buttons & BUTTON_ATTACK != 0));
        }
    }

    /// Blocked by a teammate with a clear spot to the left (+y, the enemy
    /// down +x): the bot holds fire and walks there off its sights, every
    /// tick, instead of strafing at random.
    #[test]
    fn a_blocked_bot_steps_aside_to_clear_the_line() {
        for seed in [3, 9, 21] {
            let (mut bot, mut v) = engaged(seed);
            v.enemy = Some(EnemyView {
                slot: 3,
                origin: [900.0, 0.0, 124.0],
                velocity: [0.0; 3],
            });
            v.friend_in_line = true;
            v.sidestep = Some([0.0, 64.0, 64.0]);
            for tick in 0..30 {
                let cmd = bot.think(&v);
                assert_eq!(cmd.buttons & BUTTON_ATTACK, 0, "fired on tick {tick}");
                assert_eq!(cmd.buttons & BUTTON_ADS, 0, "sighted on tick {tick}");
                assert!(
                    cmd.right < -100,
                    "seed {seed} tick {tick}: right {}",
                    cmd.right
                );
                assert!(cmd.forward.abs() < 20, "forward {}", cmd.forward);
            }
        }
    }

    /// The old aim: a target running across the line widens the miss by
    /// its speed times `track_lag`; one running along the line adds
    /// nothing. The hand trails a crossing target instead: its yaw sits
    /// behind his run by about his speed times `track_lag`.
    #[test]
    fn a_target_crossing_the_line_is_missed_by_more() {
        // Mean (unsigned miss, signed yaw lead) over ticks 10..40, degrees.
        let miss = |velocity: [f32; 3], human: bool| {
            let (mut bot, mut v) = engaged(5);
            bot.human_aim = human;
            bot.skill.error_start = bot.skill.error_floor;
            if human {
                // The drift off, to read the trail alone.
                (bot.skill.error_start, bot.skill.error_floor) = (0.0, 0.0);
            }
            let mut at = [900.0, 0.0, 64.0];
            let (mut sum, mut lead) = (0.0, 0.0);
            for tick in 0..40 {
                // The old aim read only the velocity; the hand sees him move.
                if human {
                    (0..3).for_each(|i| at[i] += velocity[i] * 0.05);
                }
                v.enemy = Some(EnemyView {
                    slot: 3,
                    origin: at,
                    velocity,
                });
                let (tp, ty) = aim_angles(&v, at);
                let a = bot.think(&v).angles.map(|a| a as f32 / ANGLE2SHORT);
                v.view = [a[0], a[1], 0.0];
                if tick >= 10 {
                    sum += yaw_diff(a[0], tp).hypot(yaw_diff(a[1], ty));
                    lead += yaw_diff(a[1], ty);
                }
            }
            (sum / 30.0, lead / 30.0)
        };
        let still = miss([0.0; 3], false).0;
        let along = miss([190.0, 0.0, 0.0], false).0;
        let across = miss([0.0, 190.0, 0.0], false).0;
        assert!(along < still * 1.3, "along {along} still {still}");
        assert!(across > still * 1.8, "across {across} still {still}");

        let lag = Skill::default().track_lag;
        let (_, lead) = miss([0.0, 190.0, 0.0], true);
        // At 900 the target runs to +y (yaw up): the aim trails at less.
        let want = -(190.0 * lag / 900.0f32).atan().to_degrees();
        assert!(
            lead < want * 0.5 && lead > want * 2.0,
            "lead {lead} want {want}"
        );
        let (_, lead) = miss([0.0, -190.0, 0.0], true);
        assert!(
            lead > -want * 0.5 && lead < -want * 2.0,
            "lead {lead} want {}",
            -want
        );
    }

    /// Off the cone the trigger may still go down: an automatic sprays at a
    /// line a bolt rifle waits on.
    #[test]
    fn an_automatic_fires_off_the_cone_where_a_bolt_rifle_waits() {
        let fired = |automatic: bool, fire_time_ms: i32| {
            let (mut bot, mut v) = engaged(7);
            bot.skill.error_start = 0.0;
            bot.skill.error_floor = 0.0;
            v.automatic = automatic;
            v.fire_time_ms = fire_time_ms;
            v.ads_frac = 1.0;
            let at = [700.0, 0.0, 64.0];
            v.enemy = Some(EnemyView {
                slot: 3,
                origin: at,
                velocity: [0.0; 3],
            });
            let (tp, ty) = aim_angles(&v, at);
            // The gun 2.5 cones off a view held on the chest.
            let cone = (HIT_RADIUS / 700.0).atan().to_degrees();
            v.gun_offset = [0.0, 2.5 * cone];
            v.view = [tp, ty, 0.0];
            (0..40)
                .filter(|_| bot.think(&v).buttons & BUTTON_ATTACK != 0)
                .count()
        };
        assert_eq!(fired(false, 1300), 0, "a bolt rifle fired 2.5 cones off");
        assert!(fired(true, 100) > 0, "an automatic never fired");
    }

    /// Footwork on the sights: a bolt rifle backs off a close target and
    /// lies down at range; off, the body never does either.
    #[test]
    fn footwork_backs_a_bolt_rifle_off_and_lies_down_at_range() {
        let keys = |on: bool, dist: f32| {
            let (mut bot, mut v) = engaged(9);
            bot.footwork = on;
            bot.skill.footwork = 1.0;
            v.fire_time_ms = 1300;
            v.enemy = Some(EnemyView {
                slot: 3,
                origin: [dist, 0.0, 64.0],
                velocity: [0.0; 3],
            });
            (0..200).map(|_| bot.think(&v)).collect::<Vec<_>>()
        };
        let prone = |c: &UserCmd| c.wbuttons & msg::WBUTTON_PRONE != 0;
        assert!(
            keys(true, 200.0).iter().any(|c| c.forward < 0),
            "never backed off"
        );
        assert!(keys(false, 200.0).iter().all(|c| c.forward >= 0));
        assert!(keys(true, 1200.0).iter().any(prone), "never lay down");
        assert!(!keys(false, 1200.0).iter().any(prone));
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
            assert!(cmd.right.abs() <= 64, "ran while sighted: {}", cmd.right);
        }
        v.ads_frac = 1.0;
        assert!((0..40).any(|_| bot.think(&v).buttons & BUTTON_ATTACK != 0));
    }

    /// The eye easing up out of a crouch: a round fired now leaves above
    /// the eye the aim was taken from, so the trigger waits for it.
    #[test]
    fn the_trigger_waits_for_the_eye_to_settle() {
        let (mut bot, mut v) = engaged(11);
        v.eye_settled = false;
        for tick in 0..40 {
            assert_eq!(
                bot.think(&v).buttons & BUTTON_ATTACK,
                0,
                "fired on tick {tick}"
            );
        }
        v.eye_settled = true;
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
        // The server's probe found the flight clear.
        v.lob = Some([-12.0, 0.0]);
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

    /// The hold runs out before the raise and the pullback's
    /// `holdFireTime` have: the trigger stays down until the pin is out,
    /// and only then comes up. Let go early, the throw waited for the pin
    /// and the next tick's switch to the rifle cancelled it.
    #[test]
    fn a_frag_is_held_until_its_pin_is_out() {
        let mut bot = Bot::new("allies", true, 1);
        let mut v = view();
        v.weapon = 6;
        v.view = [-12.0, 0.0, 0.0];
        v.enemy = Some(EnemyView {
            slot: 3,
            origin: [300.0, 0.0, 40.0],
            velocity: [0.0; 3],
        });
        v.lob = Some([-12.0, 0.0]);
        v.grenade_ready = false;
        bot.stage = Stage::Cook { left: 1, spoon: 0 };
        for _ in 0..10 {
            let cmd = bot.think(&v);
            assert_ne!(cmd.buttons & BUTTON_ATTACK, 0, "let go on the pin");
            assert_eq!(cmd.weapon, 6);
        }
        v.grenade_ready = true;
        bot.take_yell();
        let cmd = bot.think(&v);
        assert_eq!(cmd.buttons & BUTTON_ATTACK, 0, "held past the pin");
        assert_eq!(bot.throws(), 1);
        assert_eq!(bot.take_yell(), Some(yell::Yell::Grenade));
    }

    /// "Enemy spotted" on a new target, not on one the bot let go of a
    /// moment ago, nor every tick it keeps the same one.
    #[test]
    fn a_new_target_is_yelled_and_a_reacquired_one_is_not() {
        let mut bot = Bot::new("allies", true, 1);
        let mut v = view();
        let at = |slot| EnemyView {
            slot,
            origin: [300.0, 0.0, 40.0],
            velocity: [0.0; 3],
        };
        bot.think(&v);
        bot.take_yell();
        v.enemy = Some(at(3));
        bot.think(&v);
        assert_eq!(bot.take_yell(), Some(yell::Yell::Spotted));
        bot.think(&v);
        assert_eq!(bot.take_yell(), None, "the same target");
        v.enemy = Some(at(4));
        bot.think(&v);
        assert_eq!(bot.take_yell(), Some(yell::Yell::Spotted), "a second enemy");
        // Back to the first, given up on a moment ago.
        v.enemy = Some(at(3));
        bot.think(&v);
        assert_eq!(bot.take_yell(), None, "reacquired");
    }

    /// "Follow me" or "Move in" on the first think alive of a one-life
    /// round: after a death, and after a restart that never killed the
    /// body. Not on a respawn gametype.
    #[test]
    fn a_one_life_round_start_is_yelled() {
        let mut bot = Bot::new("allies", true, 1);
        let mut v = view();
        v.one_life = true;
        bot.think(&v);
        assert_eq!(bot.take_yell(), Some(yell::Yell::Advance));
        bot.think(&v);
        assert_eq!(bot.take_yell(), None, "the same life");
        v.dead = true;
        bot.think(&v);
        v.dead = false;
        bot.think(&v);
        assert_eq!(bot.take_yell(), Some(yell::Yell::Advance), "after a death");
        bot.rearm();
        bot.think(&v);
        assert_eq!(
            bot.take_yell(),
            Some(yell::Yell::Advance),
            "after a restart"
        );
        v.one_life = false;
        v.dead = true;
        bot.think(&v);
        v.dead = false;
        bot.think(&v);
        assert_eq!(bot.take_yell(), None, "a respawn");
    }

    /// An S&D bot away from its site holds still at the start of a round
    /// for a draw of its own (`team::START_HOLD`), the rear guard for
    /// longer; an enemy in sight ends it, and with `Bot::squad` off there
    /// is none.
    #[test]
    fn a_one_life_round_starts_with_a_staggered_hold() {
        let held = |seed: u64, squad: team::Squad, on: bool| {
            let mut bot = Bot::new("allies", true, seed);
            bot.squad = on;
            let mut v = sd_view(ObjRole::Attack);
            v.origin = [1000.0, 800.0, 64.0];
            v.one_life = true;
            v.squad = squad;
            bot.think(&v);
            let n = (0..200)
                .take_while(|_| {
                    let hold = bot.goal(&v) == Goal::Hold;
                    bot.think(&v);
                    hold
                })
                .count() as u32;
            assert_eq!(bot.goal(&v), Goal::To([0.0, 0.0, 64.0]), "then to the site");
            n
        };
        let main: Vec<u32> = (1..=16).map(|s| held(s, team::Squad::Main, true)).collect();
        assert!(main.iter().all(|&n| n < team::START_HOLD), "{main:?}");
        assert!(main.iter().any(|&n| n != main[0]), "each its own: {main:?}");
        for s in 1..=16 {
            let rear = held(s, team::Squad::Rear, true);
            assert!((team::REAR_HOLD..2 * team::REAR_HOLD).contains(&rear));
            let flank = held(s, team::Squad::Flank, true);
            assert!((team::FLANK_HOLD..team::START_HOLD).contains(&flank));
            assert_eq!(held(s, team::Squad::Rear, false), 0);
        }
        let mut bot = Bot::new("allies", true, 1);
        let mut v = sd_view(ObjRole::Attack);
        v.origin = [1000.0, 800.0, 64.0];
        v.one_life = true;
        v.squad = team::Squad::Rear;
        bot.think(&v);
        assert_eq!(bot.goal(&v), Goal::Hold);
        v.enemy = Some(EnemyView {
            slot: 3,
            origin: [1000.0, 1500.0, 64.0],
            velocity: [0.0; 3],
        });
        bot.think(&v);
        v.enemy = None;
        assert_ne!(bot.goal(&v), Goal::Hold, "a fight ends the hold");
    }

    /// Behind a teammate just ahead (`BotView::mate_ahead`) a bot on its
    /// way eases its keys to half; not with an enemy in sight, a bomb down
    /// or `Bot::squad` off.
    #[test]
    fn a_bot_walks_behind_a_teammate_just_ahead() {
        let forward = |ahead: bool, enemy: bool, bomb: bool, on: bool| {
            let mut bot = Bot::new("allies", false, 1);
            bot.squad = on;
            let mut v = sd_view(ObjRole::Attack);
            v.origin = [1000.0, 0.0, 64.0];
            v.view = [0.0, 90.0, 0.0];
            v.waypoint = Some([1000.0, 400.0, 64.0]);
            v.on_ground = true;
            v.move_scale = 1.0;
            v.mate_ahead = ahead;
            if bomb && let Some(sd) = v.sd.as_mut() {
                sd.bomb = Some(BombView {
                    origin: [0.0; 3],
                    aim: [0.0; 3],
                });
            }
            if enemy {
                v.enemy = Some(EnemyView {
                    slot: 3,
                    origin: [3000.0, 3000.0, 64.0],
                    velocity: [0.0; 3],
                });
            }
            bot.think(&v).forward
        };
        assert_eq!(forward(false, false, false, true), 127);
        assert_eq!(forward(true, false, false, true), 63);
        assert_eq!(forward(true, false, false, false), 127);
        assert_eq!(forward(true, false, true, true), 127);
        assert_eq!(
            forward(false, true, false, true),
            forward(true, true, false, true)
        );
    }

    /// A guard pulled back to its objective (`GuardPost::careful`) walks
    /// at half keys looking at the objective from `CAREFUL_NEAR` in and
    /// crouches from `CAREFUL_CROUCH`; out past it, it runs.
    #[test]
    fn a_guard_pulled_back_walks_in_with_care() {
        let think = |at: [f32; 3]| {
            let mut bot = Bot::new("axis", false, 1);
            let mut v = sd_view(ObjRole::Defend);
            v.origin = at;
            v.view = [0.0, 90.0, 0.0];
            v.waypoint = Some([at[0] + 400.0, at[1], 64.0]);
            v.on_ground = true;
            v.move_scale = 1.0;
            v.guard_post = Some(GuardPost {
                at: [100.0, 60.0, 64.0],
                crouch: false,
                look: [0.0, 0.0, 64.0],
                careful: Some([0.0, 0.0, 64.0]),
            });
            bot.think(&v)
        };
        let far = think([-1500.0, 0.0, 64.0]);
        assert_eq!(far.forward.max(far.right.abs()), 127, "runs");
        let near = think([-500.0, 0.0, 64.0]);
        let keys = f32::from(near.forward).hypot(f32::from(near.right));
        assert!(keys < 80.0, "walks: {near:?}");
        assert_eq!(near.wbuttons & msg::WBUTTON_CROUCH, 0);
        let close = think([-200.0, 0.0, 64.0]);
        assert_ne!(close.wbuttons & msg::WBUTTON_CROUCH, 0, "crouched");
    }

    /// A `ctf` bot the driver sends after the enemy carrying its flag
    /// (`CtfView::hunt`) chases him instead of guarding.
    #[test]
    fn a_ctf_bot_hunts_the_carrier_of_its_flag() {
        let bot = Bot::new("axis", true, 1);
        let mut v = view();
        let site = |x: f32| SiteView {
            mins: [x - 50.0, -50.0, 0.0],
            maxs: [x + 50.0, 50.0, 100.0],
            stand: [x, 0.0, 0.0],
        };
        let flag = |home: Option<SiteView>, goal: SiteView, carrier: Option<usize>| CtfFlagView {
            home,
            dropped: None,
            goal,
            carried: carrier.is_some(),
            carrier,
        };
        v.uo = Some(UoView::Ctf(Box::new(CtfView {
            ours: flag(None, site(3000.0), Some(7)),
            theirs: flag(Some(site(3000.0)), site(0.0), None),
            carrying: false,
            escort: None,
            rank: 1,
            hunt: None,
            lead: None,
            watch: None,
        })));
        assert_eq!(bot.goal(&v), Goal::To([3000.0, 0.0, 0.0]), "holds his goal");
        if let Some(UoView::Ctf(c)) = v.uo.as_mut() {
            c.hunt = Some([1500.0, 200.0, 0.0]);
        }
        assert_eq!(bot.goal(&v), Goal::To([1500.0, 200.0, 0.0]));
    }

    /// A `ctf` carrier (`Bot::ctf_carry`) keeps for its goal through a
    /// fight; with its own flag away it returns the flag lying near
    /// itself, else guards round the goal instead of standing in it.
    #[test]
    fn a_ctf_carrier_keeps_home_and_waits_near_the_goal() {
        let mut bot = Bot::new("axis", true, 1);
        let mut v = view();
        let site = |x: f32| SiteView {
            mins: [x - 50.0, -50.0, 0.0],
            maxs: [x + 50.0, 50.0, 100.0],
            stand: [x, 0.0, 0.0],
        };
        let ours = CtfFlagView {
            home: None,
            dropped: Some(site(600.0)),
            goal: site(3000.0),
            carried: false,
            carrier: None,
        };
        v.uo = Some(UoView::Ctf(Box::new(CtfView {
            ours,
            theirs: CtfFlagView {
                home: None,
                dropped: None,
                goal: site(0.0),
                carried: true,
                carrier: Some(0),
            },
            carrying: true,
            escort: None,
            rank: 2,
            hunt: None,
            lead: None,
            watch: None,
        })));
        assert_eq!(bot.goal(&v), Goal::To([600.0, 0.0, 0.0]), "returns it");
        if let Some(UoView::Ctf(c)) = v.uo.as_mut() {
            c.ours.dropped = None;
            c.ours.carried = true;
            c.ours.carrier = Some(7);
        }
        assert!(matches!(
            bot.objective_target(&v),
            Some(ObjTarget::Guard {
                at: [0.0, 0.0, 0.0],
                hold: GuardHold::Near,
                ..
            })
        ));
        assert!(bot.carrying(&v), "keeps for the goal through a fight");
        // Not carrying, dealt a point by the carrier ([`Bot::ctf_escort`]).
        if let Some(UoView::Ctf(c)) = v.uo.as_mut() {
            c.carrying = false;
            c.ours.carried = false;
            c.ours.carrier = None;
            c.ours.home = Some(site(3000.0));
            c.theirs.home = None;
            c.lead = Some([700.0, 0.0, 0.0]);
        }
        assert!(matches!(
            bot.objective_target(&v),
            Some(ObjTarget::Escort([700.0, 0.0, 0.0]))
        ));
        if let Some(UoView::Ctf(c)) = v.uo.as_mut() {
            c.carrying = true;
            c.lead = None;
            c.ours.home = None;
            c.ours.carried = true;
            c.ours.carrier = Some(7);
        }
        bot.ctf_carry = false;
        assert!(matches!(
            bot.objective_target(&v),
            Some(ObjTarget::Occupy(_))
        ));
        assert!(!bot.carrying(&v));
    }

    /// The view on the lob with the gun kicked 17 degrees up off it (a
    /// blast's knock): the release waits until the view plus the offset is
    /// on the lob. UO mp_cassino seed 13 let go there, the frag went up
    /// at 87 degrees and came down on its thrower.
    #[test]
    fn a_throw_waits_for_the_aim_not_the_view() {
        let mut bot = Bot::new("allies", true, 1);
        let mut v = view();
        v.weapon = 6;
        v.view = [-70.0, 173.0, 0.0];
        v.gun_offset = [-17.0, 0.0];
        v.enemy = Some(EnemyView {
            slot: 3,
            origin: [-300.0, 40.0, 40.0],
            velocity: [0.0; 3],
        });
        v.lob = Some([-70.0, 173.0]);
        bot.stage = Stage::Cook { left: 1, spoon: 0 };
        let cmd = bot.think(&v);
        assert_ne!(cmd.buttons & BUTTON_ATTACK, 0, "let go off the aim");
        let pitch = cmd.angles[0] as f32 / ANGLE2SHORT;
        assert!(pitch > -70.0, "the view goes down under the kick: {pitch}");
        v.view = [-53.0, 173.0, 0.0];
        let cmd = bot.think(&v);
        assert_eq!(cmd.buttons & BUTTON_ATTACK, 0, "held on the aim");
    }

    /// A pin that never comes out (the raise refused) puts the frag back.
    #[test]
    fn a_frag_whose_pin_never_comes_out_is_put_back() {
        let mut bot = Bot::new("allies", true, 1);
        bot.rifle = 10;
        let mut v = view();
        v.weapon = 6;
        v.view = [-12.0, 0.0, 0.0];
        v.enemy = Some(EnemyView {
            slot: 3,
            origin: [300.0, 0.0, 40.0],
            velocity: [0.0; 3],
        });
        v.lob = Some([-12.0, 0.0]);
        v.grenade_ready = false;
        bot.stage = Stage::Cook { left: 1, spoon: 0 };
        for _ in 0..=PIN_WAIT_TICKS + 1 {
            bot.think(&v);
        }
        assert!(matches!(bot.stage, Stage::BackToRifle { weapon: 10 }));
        assert_eq!(bot.throws(), 0);
    }

    /// Through the throw the view turns toward the enemy and up over it,
    /// never to the zero angles that threw the frag wherever
    /// `delta_angles` pointed.
    #[test]
    fn a_throw_lobs_over_the_enemy() {
        let mut bot = Bot::new("allies", true, 1);
        let mut v = view();
        v.weapon = 6;
        // The enemy 400 ahead at eye height, the view level on it: the
        // pitch rises over the line, a bounded step a tick.
        v.enemy = Some(EnemyView {
            slot: 3,
            origin: [0.0, 400.0, 64.0 + vcod_common::pmove::VIEW_STAND],
            velocity: [0.0; 3],
        });
        bot.stage = Stage::Cook { left: 20, spoon: 0 };
        let cmd = bot.think(&v);
        let pitch = cmd.angles[0] as f32 / ANGLE2SHORT;
        let yaw = cmd.angles[1] as f32 / ANGLE2SHORT;
        assert!(pitch < -10.0, "pitched up: {pitch}");
        assert!((yaw - 90.0).abs() < 1.0, "on the enemy: {yaw}");
    }

    /// An enemy that closed inside the blast during the hold: the release
    /// waits while the view comes round, then the frag goes the other way,
    /// high (the self-kills of bot-objectives.md 6).
    #[test]
    fn a_throw_turns_away_from_an_enemy_inside_the_blast() {
        let mut bot = Bot::new("allies", true, 1);
        let mut v = view();
        v.weapon = 6;
        v.view = [0.0, 90.0, 0.0];
        v.enemy = Some(EnemyView {
            slot: 3,
            origin: [0.0, 100.0, 64.0 + vcod_common::pmove::VIEW_STAND],
            velocity: [0.0; 3],
        });
        bot.stage = Stage::Cook { left: 1, spoon: 0 };
        let mut released = None;
        for tick in 0..30 {
            let cmd = bot.think(&v);
            v.view = [
                cmd.angles[0] as f32 / ANGLE2SHORT,
                cmd.angles[1] as f32 / ANGLE2SHORT,
                0.0,
            ];
            if cmd.buttons & BUTTON_ATTACK == 0 {
                released = Some((tick, v.view));
                break;
            }
        }
        let (tick, at) = released.expect("never thrown");
        assert!(tick > 5, "released at once, at the enemy: {at:?}");
        assert!(yaw_diff(at[1], -90.0).abs() <= AWAY_DEG + 1.0, "{at:?}");
        assert!(at[0] < -20.0, "high: {at:?}");
    }

    /// The view still high off an earlier lob when the hold runs out: the
    /// release waits for the probed lob's pitch, not only its yaw. mp_cassino
    /// on UO, a bot let go at -22 with the probe's lob at -5.75; the frag
    /// struck the wall above the enemy and its blast killed the thrower.
    #[test]
    fn a_throw_waits_for_the_lobs_pitch() {
        let mut bot = Bot::new("allies", true, 1);
        let mut v = view();
        v.weapon = 6;
        v.view = [-22.0, 6.0, 0.0];
        v.enemy = Some(EnemyView {
            slot: 3,
            origin: [270.0, -6.0, 40.0],
            velocity: [0.0; 3],
        });
        v.lob = Some([-5.75, -1.35]);
        bot.stage = Stage::Cook { left: 1, spoon: 0 };
        let mut released = None;
        for tick in 0..30 {
            let cmd = bot.think(&v);
            if cmd.buttons & BUTTON_ATTACK == 0 {
                released = Some((tick, v.view));
                break;
            }
            v.view = [
                cmd.angles[0] as f32 / ANGLE2SHORT,
                cmd.angles[1] as f32 / ANGLE2SHORT,
                0.0,
            ];
        }
        let (tick, at) = released.expect("never thrown");
        assert!(tick > 0, "let go on the old pitch: {at:?}");
        assert!(yaw_diff(at[0], -5.75).abs() <= LOB_SLACK, "{at:?}");
        assert!(yaw_diff(at[1], -1.35).abs() <= LOB_SLACK, "{at:?}");
    }

    /// The enemy steps out of sight during the hold: the frag still goes
    /// at where it was, and only by a probed lob, held until one is found.
    /// mp_cassino on UO, a bot let go along its view the tick its enemy
    /// left sight; the frag struck the cover and killed the thrower.
    #[test]
    fn a_throw_at_an_enemy_gone_from_sight_waits_for_a_lob() {
        let mut bot = Bot::new("allies", true, 1);
        let mut v = view();
        v.weapon = 6;
        v.enemy = Some(EnemyView {
            slot: 3,
            origin: [300.0, 0.0, 40.0],
            velocity: [0.0; 3],
        });
        v.lob = Some([-10.0, 0.0]);
        bot.stage = Stage::Cook { left: 3, spoon: 0 };
        bot.think(&v);
        v.enemy = None;
        v.lob = None;
        assert_eq!(bot.throw_target(&v), Some([300.0, 0.0, 40.0]));
        assert!(bot.wants_lob(&v), "the server stops probing");
        for tick in 0..4 {
            let cmd = bot.think(&v);
            assert!(cmd.buttons & BUTTON_ATTACK != 0, "let go blind at {tick}");
        }
        v.lob = Some([-10.0, 0.0]);
        v.view = [-10.0, 0.0, 0.0];
        let cmd = bot.think(&v);
        assert_eq!(cmd.buttons & BUTTON_ATTACK, 0, "never thrown on the lob");
        assert_eq!(bot.throw_target(&v), None);
    }

    /// Another's frag is noticed in front in a clear line, or close by;
    /// one behind or out of the line goes unnoticed.
    #[test]
    fn a_frag_is_noticed_in_front_or_close() {
        let eye = glam::Vec3::new(0.0, 0.0, 60.0);
        let open = |_, _| true;
        let notices = |at: [f32; 3], clear: &dyn Fn(glam::Vec3, glam::Vec3) -> bool| {
            notices_frag(eye, 0.0, at.into(), clear)
        };
        assert!(notices([300.0, 50.0, 0.0], &open));
        assert!(!notices([-300.0, 0.0, 0.0], &open), "seen behind");
        assert!(notices([-100.0, 0.0, 0.0], &open), "not heard close");
        assert!(
            !notices([300.0, 0.0, 0.0], &|_, _| false),
            "seen through a wall"
        );
    }

    /// A 1.1 frag with no clear lob once the wait is out is put back: the
    /// cmd that lets the trigger go asks for the rifle, which cancels the
    /// pullback. A UO frag, its fuse running, is let go.
    #[test]
    fn a_throw_with_no_lob_is_put_back() {
        for cooks in [false, true] {
            let mut bot = Bot::new("allies", true, 1);
            bot.rifle = 10;
            let mut v = view();
            v.weapon = 6;
            v.grenade_cooks = cooks;
            v.enemy = Some(EnemyView {
                slot: 3,
                origin: [300.0, 0.0, 40.0],
                velocity: [0.0; 3],
            });
            bot.stage = Stage::Cook { left: 1, spoon: 0 };
            let cmd = (0..=AWAY_TICKS)
                .map(|_| bot.think(&v))
                .find(|c| c.buttons & BUTTON_ATTACK == 0)
                .expect("held for good");
            assert_eq!(cmd.weapon, if cooks { 6 } else { 10 }, "cooks {cooks}");
            assert_eq!(bot.throws(), u32::from(cooks), "cooks {cooks}");
        }
    }

    /// mp_cassino on UO, seed 1: the tick a throw started sent null angles,
    /// which turned the view onto `delta_angles` (45 degrees off the
    /// enemy), and the enemy left sight before any target was recorded.
    /// The cook ran out with no target and the frag went flat along that
    /// heading into the wall beside the thrower.
    #[test]
    fn the_tick_a_throw_starts_aims_and_records_the_enemy() {
        let mut v = view();
        v.view = [0.0, 10.0, 0.0];
        v.delta_angles = [0, 8192, 0];
        v.grenade = Some(6);
        v.lob = Some([-10.0, 0.0]);
        v.enemy = Some(EnemyView {
            slot: 3,
            origin: [400.0, 0.0, 40.0],
            velocity: [0.0; 3],
        });
        let mut bot = Bot::new("allies", true, 1);
        let cmd = (0..400)
            .find_map(|_| {
                let cmd = bot.think(&v);
                matches!(bot.stage, Stage::ToGrenade).then_some(cmd)
            })
            .expect("a throw starts");
        assert_eq!(cmd.weapon, 6);
        let yaw = (cmd.angles[1] + v.delta_angles[1]) as f32 / ANGLE2SHORT;
        assert!(yaw.abs() < 10.0, "the view turns onto the enemy: {yaw}");
        v.enemy = None;
        assert_eq!(bot.throw_target(&v), Some([400.0, 0.0, 40.0]));
    }

    /// On the rungs the hands stay empty: no throw starts there, and one
    /// whose switch never lands (the body grabbed a ladder mid-switch) is
    /// given up, the bot back on its keys. mp_ship's hold ladder held
    /// such a bot 3000 ticks (bot-combat.md 17.2).
    #[test]
    fn a_throw_whose_switch_never_lands_is_given_up() {
        let mut v = view();
        v.grenade = Some(6);
        v.lob = Some([-10.0, 0.0]);
        v.enemy = Some(EnemyView {
            slot: 3,
            origin: [400.0, 0.0, 40.0],
            velocity: [0.0; 3],
        });
        v.on_ladder = true;
        let mut bot = Bot::new("allies", true, 1);
        for _ in 0..400 {
            bot.think(&v);
            assert!(
                matches!(bot.stage, Stage::Wander),
                "a throw starts on the rungs"
            );
        }
        bot.stage = Stage::ToGrenade;
        v.weapon = 0;
        v.busy_ms = 0;
        for _ in 0..SWITCH_IDLE_TICKS {
            bot.think(&v);
            assert!(matches!(bot.stage, Stage::ToGrenade), "given up early");
        }
        bot.think(&v);
        assert!(
            matches!(bot.stage, Stage::Wander),
            "the switch waited for good"
        );
    }

    /// An enemy lost from sight a second ago is thrown at where he went;
    /// one gone longer than [`LOST_THROW_TICKS`] is not.
    #[test]
    fn a_bot_throws_where_an_enemy_was_lost() {
        let mut v = view();
        v.grenade = Some(6);
        v.lob = Some([-10.0, 0.0]);
        let mut bot = Bot::new("allies", true, 1);
        bot.grenade_cooldown = 0;
        let lost = |ticks| Recall {
            at: [400.0, 0.0, 40.0],
            ticks,
            seen: true,
        };
        bot.recall = Some(lost(bot.skill.memory_ticks - LOST_THROW_TICKS - 2));
        bot.think(&v);
        assert!(
            matches!(bot.stage, Stage::Wander),
            "a stale spot drew a throw"
        );
        bot.recall = Some(lost(bot.skill.memory_ticks - 20));
        let cmd = bot.think(&v);
        assert!(matches!(bot.stage, Stage::ToGrenade));
        assert_eq!(cmd.weapon, 6);
        assert_eq!(bot.throw_target(&v), Some([400.0, 0.0, 40.0]));
    }

    /// An enemy in sight takes the gun until no round has gone at him for
    /// [`COVER_THROW_TICKS`].
    #[test]
    fn a_bot_throws_at_an_enemy_only_once_the_gun_has_failed() {
        let mut v = view();
        v.lob = Some([-10.0, 0.0]);
        v.enemy = Some(EnemyView {
            slot: 3,
            origin: [400.0, 0.0, 40.0],
            velocity: [0.0; 3],
        });
        let mut bot = Bot::new("allies", true, 1);
        bot.grenade_cooldown = 0;
        bot.futile = COVER_THROW_TICKS - 2;
        assert_eq!(bot.throw_target(&v), None);
        bot.futile = COVER_THROW_TICKS;
        assert_eq!(bot.throw_target(&v), Some([400.0, 0.0, 40.0]));
        bot.think(&v);
        assert!(matches!(bot.stage, Stage::ToGrenade));
    }

    /// A clip under half reloads once no enemy has been in sight for
    /// [`QUIET_RELOAD_TICKS`]; half or more, or in a fight, it does not.
    #[test]
    fn a_bot_reloads_a_short_clip_between_fights() {
        let mut v = view();
        v.grenade = None;
        v.survive.clip_size = 8;
        v.clip = 3;
        let mut bot = Bot::new("allies", true, 1);
        let reloads = |bot: &mut Bot, v: &BotView| {
            (0..QUIET_RELOAD_TICKS + 1).any(|_| bot.think(v).wbuttons & WBUTTON_RELOAD != 0)
        };
        assert!(reloads(&mut bot, &v), "a short clip reloads");
        v.clip = 4;
        assert!(
            !reloads(&mut Bot::new("allies", true, 1), &v),
            "half a clip reloaded"
        );
        v.clip = 3;
        v.enemy = Some(EnemyView {
            slot: 3,
            origin: [2000.0, 0.0, 40.0],
            velocity: [0.0; 3],
        });
        assert!(
            !reloads(&mut Bot::new("allies", true, 1), &v),
            "reloaded mid-fight"
        );
    }

    /// A live frag near, anyone's: the goal is away from it and the keys
    /// run that way, the view still on the fight.
    #[test]
    fn a_bot_runs_from_a_live_frag() {
        let (mut bot, mut v) = engaged(3);
        v.frag_near = Some([60.0, 0.0, 0.0]);
        assert!(matches!(bot.goal(&v), Goal::Away(_)));
        let cmd = bot.think(&v);
        // The enemy is down +x, the frag too: the run is backwards.
        assert!(cmd.forward < -100, "{}", cmd.forward);
        let yaw = cmd.angles[1] as f32 / ANGLE2SHORT;
        assert!(yaw.abs() < 20.0, "the view stays on the enemy: {yaw}");
    }

    /// mp_cassino, seed 1: a frag thrown away from a close enemy came back
    /// off the wall and rolled to the thrower, who stood through the switch
    /// back to its rifle and took the blast. The keys run from it there too.
    #[test]
    fn a_bot_switching_back_from_a_throw_runs_from_a_live_frag() {
        let (mut bot, mut v) = engaged(3);
        v.frag_near = Some([60.0, 0.0, 0.0]);
        bot.stage = Stage::BackToRifle { weapon: 10 };
        let cmd = bot.think(&v);
        assert!(cmd.forward < -100, "{}", cmd.forward);
    }

    /// A burning hull: inside its reach the goal is away from it, fight or
    /// no fight; outside, a point goal inside its reach waits and one past
    /// it stands.
    #[test]
    fn a_bot_keeps_out_of_a_burning_hull() {
        let (mut bot, mut v) = engaged(3);
        v.wrecks = vec![[200.0, 0.0, 64.0]];
        assert_eq!(bot.goal(&v), Goal::Away([200.0, 0.0, 64.0]));
        v.wrecks = vec![[1000.0, 0.0, 64.0]];
        bot.force_goal(Some([900.0, 0.0, 64.0]));
        assert_eq!(bot.goal(&v), Goal::Hold);
        bot.force_goal(Some([500.0, 0.0, 64.0]));
        assert_eq!(bot.goal(&v), Goal::To([500.0, 0.0, 64.0]));
    }

    /// No clear lob at the enemy (a wall, a ceiling or a body in the way):
    /// no throw starts, whatever the dice.
    #[test]
    fn no_frag_goes_without_a_clear_lob() {
        let mut v = view();
        v.lob = None;
        v.enemy = Some(EnemyView {
            slot: 3,
            origin: [300.0, 0.0, 40.0],
            velocity: [0.0; 3],
        });
        let mut bot = Bot::new("allies", true, 0x1234_5678);
        for _ in 0..600 {
            let cmd = bot.think(&v);
            assert_ne!(cmd.weapon, 6, "switched to the frag");
        }
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
                    bot.stage = Stage::Cook { left: 5, spoon: 0 };
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

    /// Base assault with the bot's one base at home and the other side's
    /// standing 2000 units off down +x, a launcher lying in its bunker and
    /// another near the bot.
    fn bas_view() -> BotView {
        let mut v = view();
        let base = |at: [f32; 3], ours: bool| BasBaseView {
            at,
            ours,
            breached: false,
            planted: false,
            blows_in: None,
            charge: None,
            shot: None,
            stands: Vec::new(),
            laid: false,
        };
        v.uo = Some(UoView::Bas(BasView {
            bases: vec![
                base([-2000.0, 0.0, 0.0], true),
                base([2000.0, 0.0, 0.0], false),
            ],
            launchers: vec![[2100.0, 50.0, 8.0], [300.0, 200.0, 8.0]],
            guns: Vec::new(),
            rank: 0,
        }));
        v
    }

    fn their_base(v: &mut BotView) -> &mut BasBaseView {
        let Some(UoView::Bas(b)) = v.uo.as_mut() else {
            unreachable!()
        };
        &mut b.bases[1]
    }

    /// Without a rocket the attacker fetches a launcher, the near one, not
    /// the one in the standing base's bunker behind its walls.
    #[test]
    fn a_bas_attacker_without_a_rocket_fetches_a_launcher() {
        let bot = Bot::new("allies", false, 1);
        let v = bas_view();
        assert_eq!(bot.goal(&v), Goal::To([300.0, 200.0, 8.0]));
    }

    /// With a rocket and no shot it walks to the breach stand nearest its
    /// own side; at one with a shot it stands, draws the launcher, fires
    /// once the view rests on the aim and reloads the dry clip.
    #[test]
    fn a_bas_attacker_with_a_rocket_walks_to_a_stand_and_fires() {
        let mut bot = Bot::new("allies", false, 1);
        bot.skill.turn_deg = 360.0;
        let mut v = bas_view();
        v.rocket = Some(20);
        v.weapons_held |= 1 << 20;
        their_base(&mut v).stands = vec![[2600.0, 0.0, 0.0], [1400.0, 0.0, 0.0]];
        assert_eq!(bot.goal(&v), Goal::To([1400.0, 0.0, 0.0]));

        v.origin = [1400.0, 0.0, 0.0];
        their_base(&mut v).shot = Some([2000.0, 0.0, 40.0]);
        assert_eq!(bot.goal(&v), Goal::Hold);
        let cmd = bot.think(&v);
        assert_eq!(cmd.weapon, 20, "the launcher comes out");
        assert_eq!(cmd.buttons & BUTTON_ATTACK, 0);

        v.weapon = 20;
        v.clip = 1;
        let (pitch, yaw) = aim_angles(&v, [2000.0, 0.0, 40.0]);
        v.view = [pitch, yaw, 0.0];
        v.ads_frac = 0.5;
        let cmd = bot.think(&v);
        assert_ne!(cmd.buttons & BUTTON_ADS, 0, "sights up for the shot");
        assert_eq!(cmd.buttons & BUTTON_ATTACK, 0, "not from the hip");
        v.ads_frac = 1.0;
        // Sighted, but the cone a turn on the way up opened is still wide:
        // mp_kursk's stand took one such rocket on its own wall.
        v.spread_scale = 40.0;
        let early = (0..4).any(|_| bot.think(&v).buttons & BUTTON_ATTACK != 0);
        assert!(!early, "a round through the opened cone");
        v.spread_scale = 0.0;
        let fired = (0..4).any(|_| bot.think(&v).buttons & BUTTON_ATTACK != 0);
        assert!(fired, "no round at the base");

        v.clip = 0;
        let cmd = bot.think(&v);
        assert_eq!(cmd.buttons & BUTTON_ATTACK, 0);
        assert_ne!(cmd.wbuttons & WBUTTON_RELOAD, 0, "the dry launcher reloads");
    }

    /// In a tank's driver's seat at `bas`: the hull at the origin, its gun's
    /// muzzle 100 ahead and the chase camera 100 behind and 100 up.
    fn shelling(traverse: bool) -> BotView {
        let mut v = bas_view();
        v.seat = Some(SeatView {
            seat: 1,
            tank: true,
            origin: [0.0, 0.0, 0.0],
            yaw: 0.0,
            speed: 0.0,
            gun_yaw: Some(0.0),
            driven: true,
            boarding: false,
            gun_speed: 10000.0,
            crowd: Crowd::default(),
            contact_ms: 0,
            level_ms: 0,
            sight: Some(GunSight {
                muzzle: [100.0, 0.0, 60.0],
                dir: [1.0, 0.0, 0.0],
                speed: 10000.0,
                eye: [-100.0, 0.0, 160.0],
            }),
            traverse,
            capturing: false,
            short: false,
            off: false,
        });
        v.waypoint = Some([1000.0, 0.0, 0.0]);
        v
    }

    /// `vc_bot_vehicles 0` gets a seated bot out: the keys go, and use
    /// lands on the seat's own exit frame (bot-objectives.md 10.4).
    #[test]
    fn vehicles_off_gets_a_seated_bot_out() {
        let mut bot = Bot::new("allies", false, 1);
        let mut v = shelling(true);
        assert_eq!(bot.think(&v).buttons & BUTTON_USE, 0, "rides on");
        if let Some(seat) = v.seat.as_mut() {
            seat.off = true;
        }
        let cmd = bot.think(&v);
        assert_eq!((cmd.forward, cmd.right), (0, 0), "the keys go");
        assert_ne!(cmd.buttons & BUTTON_USE, 0, "presses use on frame 0");
    }

    /// A tank's driver drives at the other side's standing base, and with a
    /// shot at it holds the hull, lays the view from the chase camera on
    /// the aim and fires once the server finds the shell's line on the
    /// base, shooting off or on (bot-objectives.md 7.2, "Tanks").
    #[test]
    fn a_bas_tank_driver_shells_a_standing_base() {
        let mut bot = Bot::new("allies", false, 1);
        let mut v = shelling(true);
        assert_eq!(bot.goal(&v), Goal::To([2000.0, 0.0, 0.0]));
        assert_eq!(bot.think(&v).forward, 127, "drives on");

        their_base(&mut v).shot = Some([2000.0, 0.0, 40.0]);
        assert_eq!(bot.goal(&v), Goal::Hold);
        let cmd = bot.think(&v);
        assert_eq!((cmd.forward, cmd.right, cmd.up), (0, 0, 0), "holds");
        assert_eq!(cmd.buttons & BUTTON_ATTACK, 0, "the line is not on it");
        let want = vehicle::angles_from([-100.0, 0.0, 160.0], [2000.0, 0.0, 40.0]);
        assert_eq!(cmd.angles, cmd_angles(&v, want), "the view from the camera");
        assert!(want[0] > 0.0, "looks down onto the base");

        their_base(&mut v).laid = true;
        assert_ne!(bot.think(&v).buttons & BUTTON_ATTACK, 0, "fires");
        // Breached, the gun has nothing to do there.
        their_base(&mut v).breached = true;
        assert_eq!(bot.think(&v).buttons & BUTTON_ATTACK, 0);
    }

    /// The SU-152's and the Elefant's gun points the hull's way: the
    /// driver holds `upmove`, which turns the hull to the view
    /// (coduo-vehicles.md 2).
    #[test]
    fn a_turretless_tank_turns_its_hull_onto_the_base() {
        let mut bot = Bot::new("allies", false, 1);
        let mut v = shelling(false);
        their_base(&mut v).shot = Some([2000.0, 0.0, 40.0]);
        assert_eq!(bot.think(&v).up, 127);
        let mut v = shelling(true);
        their_base(&mut v).shot = Some([2000.0, 0.0, 40.0]);
        assert_eq!(bot.think(&v).up, 0);
    }

    /// A gun that never lies on the base gets the driver out after
    /// `SHELL_WAIT_TICKS`; the 90 s ride clock does not while it shells.
    #[test]
    fn a_tank_whose_line_never_comes_gets_out() {
        let mut bot = Bot::new("allies", false, 1);
        let mut v = shelling(true);
        their_base(&mut v).shot = Some([2000.0, 0.0, 40.0]);
        let out = (0..400).find(|f| {
            v.seat.as_mut().unwrap().level_ms = f * 50;
            bot.think(&v).buttons & msg::BUTTON_USE != 0
        });
        assert!(out.is_some_and(|f| f >= 200), "{out:?}");
        let mut bot = Bot::new("allies", false, 1);
        their_base(&mut v).laid = true;
        let out = (0..2400).find(|f| {
            v.seat.as_mut().unwrap().level_ms = f * 50;
            bot.think(&v).buttons & msg::BUTTON_USE != 0
        });
        assert_eq!(out, None, "left a tank laid on the base");
    }

    /// The stock script holds the other side's tank 15 s for its capture:
    /// its new driver sends no keys and does not give it up meanwhile.
    #[test]
    fn a_driver_waits_out_a_tank_capture() {
        let mut bot = Bot::new("allies", false, 1);
        let mut v = driving(Crowd::default());
        v.seat.as_mut().unwrap().speed = 0.0;
        v.seat.as_mut().unwrap().capturing = true;
        for f in 0..320 {
            v.seat.as_mut().unwrap().level_ms = f * 50;
            let cmd = bot.think(&v);
            assert_eq!((cmd.forward, cmd.right, cmd.buttons), (0, 0, 0), "tick {f}");
        }
        v.seat.as_mut().unwrap().capturing = false;
        assert_eq!(bot.think(&v).forward, 127, "drives once it is his");
    }

    /// A `bas` bot on its way to fetch a launcher walks to a tank 800 units
    /// off; another vehicle that far it passes by.
    #[test]
    fn a_bas_attacker_takes_a_tank_over_a_launcher() {
        let mut v = bas_view();
        let mut spot = VehicleSpot {
            num: 70,
            origin: [-800.0, 0.0, 0.0],
            yaw: 0.0,
            mins: [-100.0, -50.0, 0.0],
            maxs: [100.0, 50.0, 80.0],
            tank: true,
        };
        v.vehicles = vec![spot];
        let mut b = Bot::new("allies", false, 1);
        b.think(&v);
        assert_eq!(b.goal(&v), Goal::To([-800.0, 0.0, 0.0]));
        spot.tank = false;
        v.vehicles = vec![spot];
        let mut b = Bot::new("allies", false, 1);
        b.think(&v);
        assert_eq!(b.goal(&v), Goal::To([300.0, 200.0, 8.0]), "the launcher");
    }

    /// A breached base is planted as before, rocket or not.
    #[test]
    fn a_breached_base_is_planted_not_shot() {
        let bot = Bot::new("allies", false, 1);
        let mut v = bas_view();
        v.rocket = Some(20);
        let b = their_base(&mut v);
        b.breached = true;
        b.shot = Some([2000.0, 0.0, 40.0]);
        b.charge = Some(BombView {
            origin: [2000.0, 0.0, -176.0],
            aim: [2000.0, 0.0, -160.0],
        });
        assert_eq!(bot.goal(&v), Goal::To([2000.0, 0.0, -176.0]));
    }

    /// A launcher in the bot's own standing base lies behind its walls
    /// too: the bot passed it by and stood at the wall for good.
    #[test]
    fn a_launcher_in_its_own_standing_bunker_is_passed_by() {
        let bot = Bot::new("allies", false, 1);
        let mut v = bas_view();
        v.origin = [-1700.0, 0.0, 64.0];
        let Some(UoView::Bas(b)) = v.uo.as_mut() else {
            unreachable!()
        };
        b.launchers.push([-1900.0, 0.0, 8.0]);
        assert_eq!(bot.goal(&v), Goal::To([300.0, 200.0, 8.0]));
        let Some(UoView::Bas(b)) = v.uo.as_mut() else {
            unreachable!()
        };
        b.bases[0].breached = true;
        assert_eq!(bot.goal(&v), Goal::To([-1900.0, 0.0, 8.0]));
    }

    /// Each life lost sends the attacker at the next of the other side's
    /// bases, off the defender guarding the last.
    #[test]
    fn a_bas_attacker_turns_to_the_next_base_after_a_death() {
        let mut bot = Bot::new("allies", false, 1);
        let mut v = bas_view();
        let Some(UoView::Bas(b)) = v.uo.as_mut() else {
            unreachable!()
        };
        b.bases[1].breached = true;
        let mut other = b.bases[1].clone();
        other.at = [0.0, 2000.0, 0.0];
        b.bases.push(other);
        assert_eq!(bot.goal(&v), Goal::To([2000.0, 0.0, 0.0]));
        v.dead = true;
        bot.think(&v);
        v.dead = false;
        assert_eq!(bot.goal(&v), Goal::To([0.0, 2000.0, 0.0]));
    }

    /// A launcher tapped at three times with nothing taken is passed over.
    #[test]
    fn a_launcher_that_gives_nothing_is_passed_over() {
        let mut bot = Bot::new("allies", false, 1);
        let mut v = bas_view();
        let Some(UoView::Bas(b)) = v.uo.as_mut() else {
            unreachable!()
        };
        b.launchers.push([-900.0, 0.0, 8.0]);
        for _ in 0..LAUNCHER_TAPS {
            assert_eq!(bot.goal(&v), Goal::To([300.0, 200.0, 8.0]));
            bot.tapped_launcher([300.0, 200.0, 8.0]);
        }
        assert_eq!(bot.goal(&v), Goal::To([-900.0, 0.0, 8.0]));
    }

    /// At a launcher with an empty one carried, the bot draws the empty
    /// one before it taps: the swap drops what is held, and with the gun
    /// out it traded the gun.
    #[test]
    fn a_bas_bot_taps_for_a_launcher_with_its_empty_one_in_hand() {
        let mut bot = Bot::new("allies", false, 1);
        let mut v = bas_view();
        v.origin = [260.0, 200.0, 8.0];
        let (pitch, yaw) = aim_angles(&v, [300.0, 200.0, 8.0]);
        v.view = [pitch.clamp(-80.0, 80.0), yaw, 0.0];
        v.primaries = [10, 5];
        v.weapons_held |= 1 << 5;
        v.launchers = 1 << 5;
        let mut used = false;
        for _ in 0..60 {
            let cmd = bot.think(&v);
            assert_eq!(cmd.weapon, 5, "the empty launcher drawn");
            used |= cmd.buttons & BUTTON_USE != 0;
        }
        assert!(!used, "a tap with the gun out");
        v.weapon = 5;
        let used = (0..60).any(|_| bot.think(&v).buttons & BUTTON_USE != 0);
        assert!(used, "no tap once it is in hand");
    }

    /// What a `bas` bot holds for a weapon tap: nothing drawn with a
    /// primary slot free; else an empty primary, one of the fetched kind,
    /// then not the gun it fights with (`cod11-items.md` 5).
    #[test]
    fn a_bas_bot_draws_the_primary_it_would_rather_lose() {
        let mut v = view();
        // Carbine 10 loaded in hand, panzerschreck 5, gun 12.
        v.weapons_held = (1 << 10) | (1 << 5) | (1 << 12);
        v.launchers = 1 << 5;
        v.primaries = [10, 0];
        assert_eq!(Bot::swap_out(&v, true), None, "a slot free");
        // A loaded launcher and a dry gun: a gun fetch loses the gun, not
        // the rocket in hand.
        (v.primaries, v.weapon, v.loaded, v.gun) = ([10, 5], 5, 1 << 5, None);
        assert_eq!(Bot::swap_out(&v, false), Some(10));
        // A launcher fetch the same: the dry gun before the loaded rocket.
        assert_eq!(Bot::swap_out(&v, true), Some(10));
        // Both dry: the kind fetched goes.
        v.loaded = 0;
        assert_eq!(Bot::swap_out(&v, true), Some(5));
        assert_eq!(Bot::swap_out(&v, false), Some(10));
        // Two loaded guns, a launcher fetch: not the one it fights with,
        // whichever is in hand.
        (v.primaries, v.loaded, v.gun) = ([10, 12], (1 << 10) | (1 << 12), Some(10));
        v.weapon = 10;
        assert_eq!(Bot::swap_out(&v, true), Some(12));
        // Nothing else between them: the one in hand, with no draw.
        v.gun = None;
        assert_eq!(Bot::swap_out(&v, true), Some(10));
        v.weapon = 12;
        assert_eq!(Bot::swap_out(&v, true), Some(12));
    }

    /// A bot with no round left anywhere asks for `kill`, once it has
    /// been dry a while; one with a gun loaded never does.
    #[test]
    fn a_bot_with_nothing_to_fire_respawns() {
        let mut bot = Bot::new("allies", true, 1);
        let mut v = view();
        v.gun = Some(10);
        (0..DRY_TICKS + 1).for_each(|_| _ = bot.think(&v));
        assert!(!bot.take_kill());
        v.gun = None;
        (v.clip, v.loaded) = (0, 0);
        (0..DRY_TICKS).for_each(|_| _ = bot.think(&v));
        assert!(bot.take_kill());
    }

    /// A defender attacks while none of its side's bases is breached, and
    /// keeps the charge room of one that is; one that does not shoot
    /// attacks still.
    #[test]
    fn a_bas_defender_attacks_until_a_base_of_its_own_is_breached() {
        let bot = Bot::new("allies", true, 1);
        let mut v = bas_view();
        let Some(UoView::Bas(b)) = v.uo.as_mut() else {
            unreachable!()
        };
        b.rank = 1;
        assert_eq!(
            bot.goal(&v),
            Goal::To([300.0, 200.0, 8.0]),
            "fetches a launcher"
        );
        let Some(UoView::Bas(b)) = v.uo.as_mut() else {
            unreachable!()
        };
        b.bases[0].breached = true;
        b.bases[0].charge = Some(BombView {
            origin: [-2000.0, 0.0, -176.0],
            aim: [-2000.0, 0.0, -160.0],
        });
        assert_eq!(bot.goal(&v), Goal::To([-2000.0, 0.0, -176.0]));
        let calm = Bot::new("allies", false, 1);
        assert_eq!(calm.goal(&v), Goal::To([300.0, 200.0, 8.0]));
        // With two bases of theirs it takes the one the first bot does not.
        let Some(UoView::Bas(b)) = v.uo.as_mut() else {
            unreachable!()
        };
        let mut other = b.bases[1].clone();
        other.at = [2000.0, 3000.0, 0.0];
        other.breached = true;
        other.charge = Some(BombView {
            origin: [2000.0, 3000.0, -176.0],
            aim: [2000.0, 3000.0, -160.0],
        });
        b.bases.push(other);
        assert_eq!(calm.goal(&v), Goal::To([2000.0, 3000.0, -176.0]));
        assert_eq!(bot.goal(&v), Goal::To([-2000.0, 0.0, -176.0]));
    }

    /// A defender with a charge of its side's ticking on the other side's
    /// base leaves its own breached base's room to hold that one's, and so
    /// does one whose side has breached the other side's last base.
    #[test]
    fn a_bas_defender_backs_a_ticking_charge() {
        let bot = Bot::new("allies", true, 1);
        let mut v = bas_view();
        let Some(UoView::Bas(b)) = v.uo.as_mut() else {
            unreachable!()
        };
        b.rank = 1;
        b.bases[0].breached = true;
        b.bases[0].charge = Some(BombView {
            origin: [-2000.0, 0.0, -176.0],
            aim: [-2000.0, 0.0, -160.0],
        });
        let theirs = [2000.0, 0.0, -176.0];
        let t = &mut b.bases[1];
        t.breached = true;
        t.charge = Some(BombView {
            origin: theirs,
            aim: [2000.0, 0.0, -160.0],
        });
        let mut standing = t.clone();
        standing.at = [2000.0, 3000.0, 0.0];
        standing.breached = false;
        standing.charge = None;
        b.bases.push(standing);
        assert_eq!(bot.goal(&v), Goal::To([-2000.0, 0.0, -176.0]), "its room");
        let Some(UoView::Bas(b)) = v.uo.as_mut() else {
            unreachable!()
        };
        b.bases.pop();
        assert_eq!(bot.goal(&v), Goal::To(theirs), "their last base");
        let t = their_base(&mut v);
        t.planted = true;
        t.blows_in = Some(40.0);
        assert_eq!(bot.goal(&v), Goal::To(theirs), "the charge's room");
    }

    /// A charge on a base of its side's: a shooting defender comes back from
    /// anywhere to defuse it while the clock leaves time for the run and
    /// the hold, one that does not shoot only from near.
    #[test]
    fn a_bas_defender_that_does_not_shoot_defuses_only_from_near() {
        let mut v = bas_view();
        let Some(UoView::Bas(b)) = v.uo.as_mut() else {
            unreachable!()
        };
        b.rank = 1;
        let ours = [-2000.0, 0.0, -176.0];
        let mine = &mut b.bases[0];
        mine.breached = true;
        mine.planted = true;
        mine.blows_in = Some(50.0);
        mine.charge = Some(BombView {
            origin: ours,
            aim: [-2000.0, 0.0, -160.0],
        });
        v.origin = [2500.0, 0.0, 0.0];
        assert_eq!(Bot::new("allies", true, 1).goal(&v), Goal::To(ours));
        // 4500 units off, 30 s left: no time for the run and the hold.
        let Some(UoView::Bas(b)) = v.uo.as_mut() else {
            unreachable!()
        };
        b.bases[0].blows_in = Some(30.0);
        let late = Bot::new("allies", true, 1).objective_target(&v);
        assert!(!matches!(late, Some(ObjTarget::Defuse { .. })), "too late");
        let Some(UoView::Bas(b)) = v.uo.as_mut() else {
            unreachable!()
        };
        b.bases[0].blows_in = Some(50.0);
        let calm = Bot::new("allies", false, 1);
        assert_eq!(calm.goal(&v), Goal::To([300.0, 200.0, 8.0]), "attacks on");
        v.origin = [-1000.0, 0.0, 0.0];
        assert_eq!(calm.goal(&v), Goal::To(ours), "near");
    }

    /// A launcher the body comes no nearer to is passed over.
    #[test]
    fn a_launcher_out_of_reach_is_passed_over() {
        let mut bot = Bot::new("allies", false, 1);
        let v = bas_view();
        for _ in 0..FETCH_STALL_TICKS + 1 {
            bot.think(&v);
        }
        assert_ne!(bot.goal(&v), Goal::To([300.0, 200.0, 8.0]));
    }

    /// Empty hands whose last weapon a pickup swapped away ask for one the
    /// bot still carries.
    #[test]
    fn empty_hands_ask_for_a_weapon_still_carried() {
        let mut bot = Bot::new("allies", false, 1);
        let mut v = view();
        bot.think(&v);
        v.weapon = 0;
        v.weapons_held = 1 << 20;
        v.gun = Some(20);
        assert_eq!(bot.think(&v).weapon, 20);
    }

    /// A spent launcher in hand and a spent pistol: the gun comes out.
    #[test]
    fn a_spent_launcher_and_pistol_hand_over_to_the_gun() {
        let (mut bot, mut v) = engaged(21);
        v.weapon = 20;
        v.weapons_held |= (1 << 20) | (1 << 2);
        v.pistol = Some(2);
        v.clip = 0;
        v.loaded = 1 << 10;
        v.gun = Some(10);
        assert_eq!(bot.think(&v).weapon, 10);
    }

    /// With nothing left to fire, a `bas` bot fetches the nearest weapon
    /// lying within `DRY_FETCH`, even from its charge's room, and asks for
    /// no `kill` while it does; with none in reach it uses `kill`.
    #[test]
    fn a_dry_bas_bot_fetches_a_weapon_before_it_kills_itself() {
        let mut bot = Bot::new("allies", true, 1);
        let mut v = bas_view();
        let charge = [2000.0, 0.0, -176.0];
        let b = their_base(&mut v);
        b.breached = true;
        b.planted = true;
        b.blows_in = Some(50.0);
        b.charge = Some(BombView {
            origin: charge,
            aim: [2000.0, 0.0, -160.0],
        });
        v.origin = [1900.0, 0.0, -176.0];
        v.gun = None;
        v.rocket = None;
        v.pistol = None;
        v.weapon = 0;
        v.loaded = 0;
        assert_eq!(bot.goal(&v), Goal::To([2100.0, 50.0, 8.0]), "the launcher");
        let Some(UoView::Bas(b)) = v.uo.as_mut() else {
            unreachable!()
        };
        b.guns.push([1800.0, 40.0, -176.0]);
        assert_eq!(bot.goal(&v), Goal::To([1800.0, 40.0, -176.0]), "the gun");
        for _ in 0..DRY_TICKS + 10 {
            bot.think(&v);
            assert!(!bot.take_kill(), "a kill while fetching");
        }
        let Some(UoView::Bas(b)) = v.uo.as_mut() else {
            unreachable!()
        };
        b.guns.clear();
        b.launchers.clear();
        let killed = (0..DRY_TICKS + 10).any(|_| {
            bot.think(&v);
            bot.take_kill()
        });
        assert!(killed, "nothing in reach");
    }

    /// Shooting, with only a rocket left, a `bas` bot that has given up on
    /// an enemy it cannot fire at fetches a gun lying near, never a
    /// launcher; before it gives up it fights, and one that does not shoot
    /// keeps to its objective.
    #[test]
    fn a_bas_bot_with_only_a_rocket_fetches_a_gun_off_a_futile_enemy() {
        let mut bot = Bot::new("allies", true, 1);
        let mut v = bas_view();
        v.origin = [1900.0, 0.0, -176.0];
        v.gun = None;
        v.pistol = Some(2);
        v.rocket = Some(5);
        v.weapon = 5;
        v.loaded = 1 << 5;
        let gun = [1800.0, 40.0, -176.0];
        let Some(UoView::Bas(b)) = v.uo.as_mut() else {
            unreachable!()
        };
        b.guns.push(gun);
        let fetches = |bot: &Bot, v: &BotView| matches!(bot.objective_target(v), Some(ObjTarget::Pickup { from, .. }) if from == gun);
        assert!(!fetches(&bot, &v), "no enemy given up on");
        v.enemy = Some(EnemyView {
            slot: 3,
            origin: [1950.0, 0.0, -136.0],
            velocity: [0.0; 3],
        });
        assert!(!fetches(&bot, &v), "in the fight");
        for _ in 0..FUTILE_TICKS {
            bot.think(&v);
        }
        assert!(bot.ignores(3));
        v.enemy = None;
        assert!(fetches(&bot, &v), "the gun");
        let Some(UoView::Bas(b)) = v.uo.as_mut() else {
            unreachable!()
        };
        b.guns.clear();
        assert!(
            !matches!(bot.objective_target(&v), Some(ObjTarget::Pickup { .. })),
            "no launcher"
        );
        let mut calm = Bot::new("allies", false, 1);
        calm.ignored = Some((3, IGNORE_TICKS));
        let Some(UoView::Bas(b)) = v.uo.as_mut() else {
            unreachable!()
        };
        b.guns.push(gun);
        assert!(!fetches(&calm, &v), "not shooting");
    }

    /// The planter holds the room of its charge against the defuse while
    /// the clock has time on it, and gets out of the blast before it goes.
    #[test]
    fn a_bas_planter_holds_its_charge_then_clears_the_blast() {
        let bot = Bot::new("allies", false, 1);
        let mut v = bas_view();
        let charge = [2000.0, 0.0, -176.0];
        let b = their_base(&mut v);
        b.breached = true;
        b.planted = true;
        b.blows_in = Some(30.0);
        b.charge = Some(BombView {
            origin: charge,
            aim: [2000.0, 0.0, -160.0],
        });
        assert_eq!(bot.goal(&v), Goal::To(charge), "out of the room");
        v.origin = [1900.0, 0.0, -176.0];
        assert_eq!(bot.goal(&v), Goal::Hold, "in the room, on guard");
        their_base(&mut v).blows_in = Some(BAS_CLEAR_S);
        assert_eq!(bot.goal(&v), Goal::Away(charge), "inside the blast");
        v.enemy = Some(EnemyView {
            slot: 3,
            origin: [1800.0, 0.0, -150.0],
            velocity: [0.0; 3],
        });
        assert_eq!(bot.goal(&v), Goal::Away(charge), "a fight in the blast");
        v.enemy = None;
        v.origin = [1400.0, 0.0, 0.0];
        assert_eq!(bot.goal(&v), Goal::Hold, "clear of it");
    }

    /// A fight inside the blast of a charge about to go: the gun stays on
    /// the enemy, the keys run the graph's way out.
    #[test]
    fn a_fight_in_a_charges_blast_keeps_running_out() {
        let (mut bot, mut v) = engaged(21);
        v.uo = bas_view().uo;
        let b = their_base(&mut v);
        b.breached = true;
        b.planted = true;
        b.blows_in = Some(8.0);
        b.charge = Some(BombView {
            origin: [50.0, 0.0, 0.0],
            aim: [50.0, 0.0, 16.0],
        });
        v.waypoint = Some([-300.0, 0.0, 64.0]);
        for _ in 0..20 {
            let cmd = bot.think(&v);
            let yaw = (cmd.angles[1] + v.delta_angles[1]) as f32 / ANGLE2SHORT;
            let way = yaw + (-(cmd.right as f32)).atan2(cmd.forward as f32).to_degrees();
            assert!(cmd.forward != 0 || cmd.right != 0, "keys down");
            assert!(yaw_diff(way, 180.0).abs() < 45.0, "runs west, not {way}");
            assert_eq!(cmd.buttons & msg::BUTTON_ADS, 0);
            // On the move, or the stall check sends it a random way.
            v.origin[0] -= 10.0;
        }
    }

    /// An enemy no round goes at (an LMG that will not deploy, here) is
    /// given up on after [`FUTILE_TICKS`] for [`IGNORE_TICKS`]; one being
    /// shot at is not.
    #[test]
    fn an_enemy_no_round_goes_at_is_given_up_on() {
        let (mut bot, mut v) = engaged(21);
        let fired = (0..FUTILE_TICKS).any(|_| bot.think(&v).buttons & BUTTON_ATTACK != 0);
        assert!(fired && !bot.ignores(3), "a rifle fires");

        let (mut bot, mut v2) = engaged(21);
        v2.lmg = true;
        v2.automatic = true;
        for _ in 0..FUTILE_TICKS - 1 {
            assert_eq!(bot.think(&v2).buttons & BUTTON_ATTACK, 0);
        }
        assert!(!bot.ignores(3));
        bot.think(&v2);
        assert!(bot.ignores(3), "nothing fired in {FUTILE_TICKS} ticks");
        v.enemy = None;
        for _ in 0..IGNORE_TICKS - 1 {
            bot.think(&v);
        }
        assert!(bot.ignores(3));
        bot.think(&v);
        assert!(!bot.ignores(3), "back in view after {IGNORE_TICKS} ticks");
    }

    /// A rocket launcher in hand fires at no enemy, near or far; the
    /// pistol comes out against one, the gun first when there is one.
    #[test]
    fn no_rocket_goes_at_an_enemy() {
        let (mut bot, mut v) = engaged(21);
        v.rocket = Some(10);
        v.pistol = Some(2);
        v.weapons_held |= 1 << 2;
        v.loaded |= 1 << 2;
        v.clip = 1;
        let cmd = bot.think(&v);
        assert_eq!(cmd.buttons & BUTTON_ATTACK, 0, "a rocket at 100 units");
        assert_eq!(cmd.weapon, 2, "the pistol comes out");

        let (mut bot, mut v) = engaged(21);
        v.rocket = Some(10);
        v.clip = 1;
        v.has_ads = false;
        v.enemy.as_mut().unwrap().origin = [900.0, 0.0, 124.0];
        let fired = (0..20).any(|_| bot.think(&v).buttons & BUTTON_ATTACK != 0);
        assert!(!fired, "a rocket at 900 units");

        v.gun = Some(4);
        v.weapons_held |= 1 << 4;
        v.loaded |= 1 << 4;
        assert_eq!(bot.think(&v).weapon, 4);
    }

    #[test]
    fn a_lead_runs_ahead_by_the_rounds_flight() {
        // A rocket at 2800 on a hull crossing at 200: about a second's
        // flight, a little over for the longer way to the led point.
        let p = vehicle::lead([0.0; 3], [2800.0, 0.0, 0.0], [0.0, 200.0, 50.0], 2800.0);
        assert!((p[1] - 200.5).abs() < 0.5, "{p:?}");
        assert_eq!(p[2], 0.0, "a rise or fall of the target is not led");
        // A bullet goes at the body.
        let at = [500.0, 0.0, 0.0];
        assert_eq!(vehicle::lead([0.0; 3], at, [0.0, 100.0, 0.0], 0.0), at);
    }

    fn driving(crowd: Crowd) -> BotView {
        let mut v = view();
        v.waypoint = Some([1000.0, 0.0, 64.0]);
        v.seat = Some(SeatView {
            seat: 1,
            tank: true,
            origin: [0.0, 0.0, 64.0],
            yaw: 0.0,
            speed: 100.0,
            gun_yaw: Some(0.0),
            driven: true,
            boarding: false,
            gun_speed: 10000.0,
            crowd,
            contact_ms: 0,
            level_ms: 0,
            sight: None,
            traverse: true,
            capturing: false,
            short: false,
            off: false,
        });
        v
    }

    /// A driver remembers a noise as a bot on foot does: its goal holds on
    /// the ticks the noise is not heard, and the hull's route with it
    /// (bot-objectives.md 10.8).
    #[test]
    fn a_driver_keeps_a_noise_it_heard_as_its_goal() {
        let mut bot = Bot::new("allies", false, 1);
        let mut v = driving(Crowd::default());
        v.noise = Some([2000.0, 500.0, 64.0]);
        bot.think(&v);
        v.noise = None;
        assert_eq!(bot.goal(&v), Goal::To([2000.0, 500.0, 64.0]));
    }

    #[test]
    fn a_driver_holds_for_a_teammate_in_its_way_and_then_backs_off() {
        let mut bot = Bot::new("allies", false, 1);
        assert_eq!(bot.think(&driving(Crowd::default())).forward, 127);
        let ahead = Crowd {
            ahead: true,
            ..Crowd::default()
        };
        let cmd = bot.think(&driving(ahead));
        assert_eq!((cmd.forward, cmd.right), (0, 0));
        let backs = (0..60).find(|_| bot.think(&driving(ahead)).forward < 0);
        assert!(backs.is_some(), "the driver never backed off");
        // One behind as well: it waits.
        let mut bot = Bot::new("allies", false, 1);
        let both = Crowd {
            ahead: true,
            behind: true,
            ..Crowd::default()
        };
        assert!((0..100).all(|_| bot.think(&driving(both)).forward == 0));
    }

    #[test]
    fn a_tank_turning_in_place_holds_for_a_teammate_at_its_side() {
        let mut bot = Bot::new("allies", false, 1);
        let mut v = driving(Crowd::default());
        v.waypoint = Some([0.0, 1000.0, 64.0]);
        let cmd = bot.think(&v);
        assert_eq!((cmd.forward, cmd.right), (0, -127), "a turn in place");
        v.seat.as_mut().unwrap().crowd.beside = true;
        assert_eq!(bot.think(&v).right, 0);
    }

    /// Hemmed in at a back corner, a turn in place rolls on ahead when the
    /// way ahead is clear (seed 1 of `uo_bots_fight_and_sprint_in_tdm`).
    #[test]
    fn a_turn_held_from_behind_rolls_on_ahead() {
        let mut bot = Bot::new("allies", false, 1);
        let mut v = driving(Crowd {
            beside: true,
            behind: true,
            ..Crowd::default()
        });
        v.waypoint = Some([0.0, 1000.0, 64.0]);
        let rolls = (0..60).find(|_| bot.think(&v).forward > 0);
        assert!(rolls.is_some(), "the hull never rolled on");
    }

    /// At a route's end short of a goal the hull cannot reach, the driver
    /// gets out at once; at a route's end on its goal it sits out the idle
    /// wait first (bot-objectives.md 14.10).
    #[test]
    fn a_driver_short_of_a_goal_it_cannot_reach_gets_out() {
        let out = |short: bool| {
            let mut bot = Bot::new("allies", false, 1);
            let mut v = driving(Crowd::default());
            v.waypoint = None;
            let seat = v.seat.as_mut().unwrap();
            seat.speed = 0.0;
            seat.short = short;
            (0..200).find(|i| {
                v.seat.as_mut().unwrap().level_ms = *i * 50;
                bot.think(&v).buttons & msg::BUTTON_USE != 0
            })
        };
        assert!(out(true).is_some_and(|i| i < 4), "{:?}", out(true));
        assert!(out(false).is_some_and(|i| i >= 100), "{:?}", out(false));
    }

    /// A hull wedged on something bounces off it, so its speed never
    /// reads stalled: its unbroken contact backs it off, and gets the
    /// driver out before the seven seconds that destroy it (seed 9).
    #[test]
    fn a_driver_backs_off_and_then_leaves_a_hull_wedged_in_contact() {
        let mut bot = Bot::new("allies", false, 1);
        let mut v = driving(Crowd::default());
        v.seat.as_mut().unwrap().contact_ms = 2600;
        assert!((0..2).any(|_| bot.think(&v).forward < 0), "no back-off");
        v.seat.as_mut().unwrap().contact_ms = 5000;
        let out = (0..4).any(|_| bot.think(&v).buttons & msg::BUTTON_USE != 0);
        assert!(out, "the driver stays in");
    }

    /// A gunner with nobody to shoot gets out of a hull that makes no
    /// headway with its driver, not after him (seed 45 of
    /// `uo_bots_fight_and_sprint_in_tdm`).
    #[test]
    fn a_gunner_leaves_a_ride_that_makes_no_headway() {
        let mut bot = Bot::new("allies", false, 1);
        let mut v = driving(Crowd::default());
        let seat = v.seat.as_mut().unwrap();
        seat.seat = 2;
        // Shuffling back and forth at speed, never 300 units from the start.
        let out = (0..200).find(|i| {
            let seat = v.seat.as_mut().unwrap();
            seat.origin[0] = if i % 40 < 20 { 0.0 } else { 150.0 };
            seat.level_ms = *i * 50;
            bot.think(&v).buttons & msg::BUTTON_USE != 0
        });
        assert!(out.is_some_and(|i| i > 150), "{out:?}");
        // A ride on the move keeps him.
        let mut bot = Bot::new("allies", false, 1);
        let out = (0..200).find(|i| {
            let seat = v.seat.as_mut().unwrap();
            seat.origin[0] = *i as f32 * 10.0;
            seat.level_ms = *i * 50;
            bot.think(&v).buttons & msg::BUTTON_USE != 0
        });
        assert_eq!(out, None);
    }

    /// A tank's ground made counts from where a fight's hold left it: the
    /// way back across an older spot is no headway (seed 25).
    #[test]
    fn a_tank_counts_its_headway_from_where_a_fight_held_it() {
        let mut bot = Bot::new("allies", false, 1);
        let mut v = driving(Crowd::default());
        let at = |v: &mut BotView, x: f32, f: i32| {
            let seat = v.seat.as_mut().unwrap();
            seat.origin[0] = x;
            seat.level_ms = f * 50;
        };
        for f in 0..150 {
            at(&mut v, -100.0 + (f % 2) as f32, f);
            bot.think(&v);
        }
        at(&mut v, 250.0, 150);
        v.enemy = Some(EnemyView {
            slot: 3,
            origin: [900.0, 0.0, 64.0],
            velocity: [0.0; 3],
        });
        bot.think(&v);
        v.enemy = None;
        let out = (151..200).find(|&f| {
            at(&mut v, if f % 2 == 0 { 250.0 } else { 0.0 }, f);
            bot.think(&v).buttons & msg::BUTTON_USE != 0
        });
        assert!(out.is_some_and(|f| f < 175), "{out:?}");
    }

    /// Leaving riders press use on their own seat's frames: two never get
    /// out of one vehicle on one frame (seed 15).
    #[test]
    fn riders_leave_a_vehicle_on_frames_of_their_own() {
        let presses = |seat: usize| -> Vec<i32> {
            let mut bot = Bot::new("allies", false, 1);
            let mut v = driving(Crowd::default());
            v.seat.as_mut().unwrap().seat = seat;
            v.seat.as_mut().unwrap().contact_ms = 5000;
            (0..12)
                .filter(|&f| {
                    v.seat.as_mut().unwrap().level_ms = f * 50;
                    bot.think(&v).buttons & msg::BUTTON_USE != 0
                })
                .collect()
        };
        let (d, g) = (presses(1), presses(2));
        assert!(!d.is_empty() && !g.is_empty());
        assert!(d.iter().all(|f| !g.contains(f)), "{d:?} {g:?}");
    }

    /// A tank turning in place between walls shrugs a few degrees each
    /// way: a window that turned it under 15 degrees backs it off.
    #[test]
    fn a_tank_that_cannot_turn_in_place_backs_off() {
        let mut bot = Bot::new("allies", false, 1);
        let mut v = driving(Crowd::default());
        v.waypoint = Some([0.0, 1000.0, 64.0]);
        v.seat.as_mut().unwrap().speed = 0.0;
        let backs = (0..40).find(|_| bot.think(&v).forward < 0);
        assert!(backs.is_some(), "the turn in place never backed off");
        // Turning, it does not.
        let mut bot = Bot::new("allies", false, 1);
        let backs = (0..40).find(|i| {
            v.seat.as_mut().unwrap().yaw = -2.0 * *i as f32;
            bot.think(&v).forward < 0
        });
        assert_eq!(backs, None);
    }

    fn armored(at: [f32; 3]) -> BotView {
        let mut v = view();
        v.rocket = Some(20);
        v.weapons_held |= 1 << 20;
        v.loaded |= 1 << 20;
        v.armor = Some(ArmorView {
            num: 90,
            centre: at,
            velocity: [0.0; 3],
            crew: 1 << 5,
            shot_speed: 2800.0,
            friend_in_line: false,
            muzzle_clear: true,
        });
        let (pitch, yaw) = aim_angles(&v, at);
        v.view = [pitch, yaw, 0.0];
        v
    }

    #[test]
    fn a_bot_with_a_launcher_draws_it_and_fires_at_an_enemy_vehicle() {
        let mut bot = Bot::new("allies", true, 1);
        let v = armored([800.0, 300.0, 100.0]);
        let cmd = bot.think(&v);
        assert_eq!(cmd.weapon, 20, "the launcher is not drawn");
        assert_eq!(cmd.buttons & BUTTON_ATTACK, 0);
        let mut v = armored([800.0, 300.0, 100.0]);
        v.weapon = 20;
        v.clip = 1;
        let cmd = bot.think(&v);
        assert_ne!(cmd.buttons & BUTTON_ATTACK, 0, "no rocket at the hull");
        assert_eq!((cmd.forward, cmd.right), (0, 0));
        assert_eq!(bot.rockets_at_armor(), 1);
        // A teammate across the line holds it.
        let mut bot = Bot::new("allies", true, 1);
        v.armor.as_mut().unwrap().friend_in_line = true;
        assert_eq!(bot.think(&v).buttons & BUTTON_ATTACK, 0);
    }

    /// A wall in the cone by the muzzle holds the rocket, and the feet keep
    /// walking: the view lies on the hull and the keys still run the path.
    #[test]
    fn a_launcher_with_a_wall_by_its_muzzle_holds_its_rocket() {
        let mut bot = Bot::new("allies", true, 1);
        let mut v = armored([800.0, 300.0, 100.0]);
        v.weapon = 20;
        v.clip = 1;
        v.armor.as_mut().unwrap().muzzle_clear = false;
        v.waypoint = Some([0.0, 400.0, 0.0]);
        let cmd = bot.think(&v);
        assert_eq!(cmd.buttons & BUTTON_ATTACK, 0, "fired into the wall");
        assert_ne!((cmd.forward, cmd.right), (0, 0), "stood still behind it");
        assert_eq!(bot.rockets_at_armor(), 0);
    }

    #[test]
    fn a_nearer_enemy_on_foot_comes_before_a_vehicle() {
        let mut bot = Bot::new("allies", true, 1);
        bot.skill.reaction_min = 0;
        bot.skill.reaction_spread = 0;
        let mut v = armored([1200.0, 0.0, 100.0]);
        v.enemy = Some(EnemyView {
            slot: 3,
            origin: [300.0, 0.0, 124.0],
            velocity: [0.0; 3],
        });
        assert_eq!(bot.think(&v).weapon, 10, "the launcher came out");
        // The same enemy riding in the vehicle is the vehicle.
        let mut bot = Bot::new("allies", true, 1);
        v.armor.as_mut().unwrap().crew |= 1 << 3;
        assert_eq!(bot.think(&v).weapon, 20);
    }

    #[test]
    fn a_tank_driver_lays_its_gun_on_an_enemy_vehicle() {
        let mut bot = Bot::new("allies", true, 1);
        let mut v = driving(Crowd::default());
        let at = [1500.0, 0.0, 100.0];
        v.armor = Some(ArmorView {
            num: 90,
            centre: at,
            velocity: [0.0; 3],
            crew: 1 << 5,
            shot_speed: 10000.0,
            friend_in_line: false,
            muzzle_clear: true,
        });
        let cmd = bot.think(&v);
        assert_eq!(cmd.forward, 0, "the tank does not hold for the shot");
        assert_ne!(cmd.buttons & BUTTON_ATTACK, 0, "the laid gun did not fire");
    }
}
