//! Server-side bots: synthetic clients in `Server::clients` whose brains
//! live here. A bot joins through the stock menus like any client (the
//! driver in `server.rs` feeds this module the reliable server commands and
//! sends back the `mr` replies), then sends real usercmds through pmove.
//!
//! The brain is pure: [`Bot::observe`] consumes server commands and returns
//! `mr` replies, [`Bot::think`] returns the tick's usercmd from a read-only
//! [`BotView`] the server fills. Everything wire- and VM-shaped stays in
//! `server.rs`.

pub(crate) mod bridge;
pub mod cod2;
pub mod cod4;
pub(crate) mod cover;
pub(crate) mod driver;
pub mod game_bots;
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
pub(crate) mod gun;
mod menus;
mod objective;
mod sight;
mod view;
mod walk;
pub(crate) use fight::HURT_MS;
use fight::{PITCH_MOST, Target};
pub(crate) use grenade::{
    COOK_MEAN_MS, COOK_MOST_MS, LOB_BODY_GROW, LOB_MISS, LOB_REACH_SLACK, LOB_SLACK, Lob,
    MATE_LEAD_S, lob, mate_in_blast,
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
    HEAR_BLAST, HEAR_GUNFIRE, HitFelt, Looker, Seen, WRECK_REACH, blast, chest, feel_hit, fired,
    foe, frag_near, friend_in_line, gunfire, held_sidestep, hurt_of, loudest, notices_frag,
    same_team, sight_enemy, tracked_and_fov, wreck_near,
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
    retreat_end: Option<retreat::RetreatEnd>,
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
            retreat_end: None,
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

/// An absolute (pitch, yaw) as the cmd carries it, net of `delta_angles`:
/// CoD1 and CoD2 truncate, 1.7 rounds off its float delta ([`BotView::delta_deg`]).
pub(crate) fn cmd_angles(view: &BotView, aim: [f32; 2]) -> [i32; 3] {
    if let Some(d) = view.delta_deg {
        let a = |i: usize, aim: f32| cod4::cmd_angle(f64::from(aim) - f64::from(d[i]));
        return [a(0, aim[0]), a(1, aim[1]), a(2, 0.0)];
    }
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
    use super::*;
    use vcod_common::net::msg::{BUTTON_ATTACK, BUTTON_USE};

    pub(super) fn all(_: &str) -> bool {
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
    pub(super) fn sd_view(role: ObjRole) -> BotView {
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
    pub(super) fn lost(at: [f32; 3], velocity: [f32; 3]) -> (Bot, BotView) {
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

    /// Base assault with the bot's one base at home and the other side's
    /// standing 2000 units off down +x, a launcher lying in its bunker and
    /// another near the bot.
    pub(super) fn bas_view() -> BotView {
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

    pub(super) fn their_base(v: &mut BotView) -> &mut BasBaseView {
        let Some(UoView::Bas(b)) = v.uo.as_mut() else {
            unreachable!()
        };
        &mut b.bases[1]
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
