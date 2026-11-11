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
pub(crate) mod hunt;

use crate::server::FRAME_MS;
pub mod team;
use vcod_common::net::msg::{self, NULL_USERCMD, UserCmd};
pub use vehicle::{ArmorView, Crowd, SeatView, VehicleSpot};
pub(crate) use vehicle::{
    GUN_RANGE as VEHICLE_GUN_RANGE, ROCKET_CLEAR_MARGIN, ROCKET_MIN as VEHICLE_ROCKET_MIN,
    ROCKET_RANGE as VEHICLE_ROCKET_RANGE, SEEK_RANGE as VEHICLE_SEEK_RANGE,
};

mod vehicle;

pub(crate) mod retreat;
mod skill;
pub(crate) mod yell;
pub use skill::{DEFAULT_LEVEL, MAX_LEVEL};

/// Hip-shot range; beyond it a tap is noise anyway.
pub(crate) const SHOOT_RANGE: f32 = 1500.0;
/// A frag goes at a target this close and no further, and no nearer than
/// `GRENADE_MIN`, inside which the 350-unit blast takes a third or more of
/// the thrower's health too; before a lob is probed the view turns
/// `LOB_DEG` over the line to it at the longest range.
const GRENADE_RANGE: f32 = 800.0;
const GRENADE_MIN: f32 = 250.0;
const LOB_DEG: f32 = 25.0;
/// A probed lob's frag goes off this near its target or is not thrown;
/// a throw is flown in full only when its arc, unhindered, comes down
/// this near the range (`Server::lob_probe`).
pub(crate) const LOB_MISS: f32 = 150.0;
pub(crate) const LOB_REACH_SLACK: f32 = 250.0;
/// A teammate is kept out of the blast for this many seconds of his pace,
/// and a body out of the throw's early arc by this much all round, as it
/// may step in before the release (bot-combat.md 12).
pub(crate) const MATE_LEAD_S: u32 = 2;
pub(crate) const LOB_BODY_GROW: f32 = 24.0;
/// A live grenade this near is run from: a frag's blast is 350 across,
/// and a bot that followed its enemy after a throw, or stood where the
/// frag rolled back to, took its own blast (bot-objectives.md 6).
pub(crate) const FLEE_REACH: f32 = 400.0;
pub(crate) const FLEE_HEIGHT: f32 = 160.0;
/// Another's frag is run from only once noticed ([`notices_frag`]): in a
/// clear line and within this of the view's yaw either way, or this near.
const FRAG_SEEN_DEG: f32 = 60.0;
const FRAG_HEARD: f32 = 150.0;

/// Whether a bot whose eye is at `eye`, looking along `yaw` (degrees),
/// notices someone else's live frag at `frag`: in a clear line (`clear`)
/// and in front, or close enough to hear it land. Run from wherever it lay,
/// no frag ever went off near a bot (0 kills of 718 throws over the
/// `tdm` table's 16 matches, bot-combat.md 6.5); CoD 1 draws no grenade
/// indicator.
pub(crate) fn notices_frag(
    eye: glam::Vec3,
    yaw: f32,
    frag: glam::Vec3,
    clear: impl Fn(glam::Vec3, glam::Vec3) -> bool,
) -> bool {
    let to = frag - eye;
    let ahead = yaw_diff(to.y.atan2(to.x).to_degrees(), yaw).abs() < FRAG_SEEN_DEG;
    (ahead || to.length() < FRAG_HEARD) && clear(eye, frag)
}
/// A throw turned away from a close enemy with no clear lob found goes
/// this high (wire pitch, up). A release waits for a clear lob and the view
/// within `LOB_SLACK` of it, else within `AWAY_DEG` of the throw's yaw,
/// `AWAY_TICKS` at most.
const AWAY_PITCH: f32 = -30.0;
const AWAY_DEG: f32 = 30.0;
const AWAY_TICKS: u32 = 16;
/// Ticks past the hold a frag waits for its pin ([`BotView::grenade_ready`])
/// before it is put back: the raise and the pullback take 17 at most.
const PIN_WAIT_TICKS: u32 = 30;
/// A UO cook's hold: `COOK_TICKS` plus a draw under `COOK_SPREAD`, the
/// sight button down from `COOK_SPOON_AFTER` ticks in (`Bot::think_grenade`).
const COOK_TICKS: u32 = 24;
const COOK_SPREAD: u32 = 10;
const COOK_SPOON_AFTER: u32 = 4;
/// The most of a UO fuse a bot's cook burns before the release: the
/// longest spooned hold and the longest wait for the view after it.
pub(crate) const COOK_MOST_MS: i32 =
    ((COOK_TICKS + COOK_SPREAD - 1 - COOK_SPOON_AFTER + AWAY_TICKS) * FRAME_MS as u32) as i32;
/// What the spooned hold burns on the mean, the view already on the lob:
/// where the probe looks for the burst.
pub(crate) const COOK_MEAN_MS: i32 =
    ((COOK_TICKS + COOK_SPREAD / 2 - COOK_SPOON_AFTER) * FRAME_MS as u32) as i32;
/// A turned-away throw is probed at a spot this far off along its yaw.
const AWAY_REACH: f32 = 400.0;
/// The view's pitch either way of level: past pmove's 16000-short clamp
/// (87.9) no further, and high enough for a lob that comes down 250 off
/// (`Server::lob_probe` tries 85).
const PITCH_MOST: f32 = 85.0;
/// A release on a probed lob waits until the view is this near it in
/// pitch and yaw, under a lob step of the probe.
pub(crate) const LOB_SLACK: f32 = 2.0;
/// Ticks a dead bot waits before its first use press.
const RESPAWN_DELAY: u32 = 30;
/// Ticks between use-press retries, and the press length: two ticks down,
/// then 1 s of release, repeated until the script's poll catches one.
const RESPAWN_RETRY: u32 = 20;
/// Half the width of what a shot has to land in (chest or head), units; the
/// fire cone is the angle it subtends at the target's range.
const HIT_RADIUS: f32 = 10.0;
/// The run speed at which a moving bot's error takes all of
/// [`Skill::error_moving`]; slower, it takes that share of it.
const RUN_SPEED: f32 = 190.0;
/// The move keys' share a bot spacing out behind a teammate keeps
/// (`Bot::spaced`): pmove scales the pace by the key, so about half.
const EASE_KEY: f32 = 0.5;
/// A semi-automatic with a `fireTime` at least this long works a bolt.
const BOLT_MS: i32 = 500;
/// How far off its line the trigger may go down, in fire cones: past one
/// the chance falls to none at this ([`Bot::trigger`]). An automatic
/// sprays early, a bolt rifle waits for its one round.
const REACH_AUTO: f32 = 4.5;
const REACH_SEMI: f32 = 3.0;
const REACH_BOLT: f32 = 2.0;
/// Inside this an automatic fires from the hip while its sights come up.
const AUTO_HIP_RANGE: f32 = 700.0;
/// Inside this a bolt rifle's footwork backs away from the target.
const BACK_OFF_RANGE: f32 = 300.0;
/// Past this a footwork spell may lie down; inside three quarters of it
/// the body gets up again.
const PRONE_RANGE: f32 = 900.0;
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
#[derive(Clone, Copy, Debug, PartialEq)]
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
    /// Error multiplier while the bot itself moves at run speed; slower,
    /// a share of it ([`RUN_SPEED`]).
    pub error_moving: f32,
    /// Seconds the aim trails a target moving across the line: its speed
    /// across times this adds to the miss, in units.
    pub track_lag: f32,
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

/// What the brain needs to know about its own body this tick. The server
/// fills it from the sim, the weapon table and the script's team table.
pub struct BotView {
    pub origin: [f32; 3],
    /// `ps.viewHeightCurrent`: the eye, and the muzzle a round leaves from,
    /// this far over `origin`. Crouched it is 40, not the standing 60.
    pub eye_height: f32,
    /// The eye has reached its stance's height. While it eases between
    /// stances the round leaves from a height the aim was not taken at.
    pub eye_settled: bool,
    /// Where a lean puts the eye, and the muzzle with it, off the upright
    /// eye (`aim::lean_offset`): the aim is taken from there.
    pub lean_offset: [f32; 3],
    /// `ps.delta_angles`, so the bot's absolute aim lands right after a spawn.
    pub delta_angles: [i32; 3],
    /// `ps.viewangles`, degrees, wire convention (pitch positive down).
    pub view: [f32; 3],
    /// Where the last cmd's shot went off the view it was aimed from,
    /// (pitch, yaw) degrees (`ClientSim::gun_offset`). Down a sight the
    /// round leaves along the swayed gun, up to 0.45 degrees off on a scope
    /// at rest, and from the view the cmd before left (combat doc 15.1):
    /// the reticle shows both, and the bot aims by it.
    pub gun_offset: [f32; 2],
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
    /// The held weapon is a United Offensive light machine gun, which
    /// fires only off its deployed bipod (coduo-movement.md 6, 7).
    pub lmg: bool,
    /// The bipod is down (`PlayerState::deployed`), and the yaw the
    /// deploy faced (`proneDirection`), which the view keeps within
    /// `bg_lmg_yawcap` of.
    pub deployed: bool,
    pub deploy_yaw: f32,
    /// `ps.fWeaponPosFrac`: 1 once the sights are fully up.
    pub ads_frac: f32,
    /// `ps.aimSpreadScale`, 0 to 255: what the cone has opened past the
    /// weapon's floor (`spread_deg`).
    pub spread_scale: f32,
    /// Horizontal speed, units/s.
    pub speed: f32,
    /// `ps.velocity`, units/s.
    pub velocity: [f32; 3],
    pub dead: bool,
    /// `pm_type` 0, a spawned player rather than a spectator or camera.
    pub playing: bool,
    /// The enemy in sight the server picked (`Server::bot_enemy`'s
    /// order: the one who just hit the bot, the tracked target, the
    /// nearest in the sight cone, [`Skill::fov_deg`]).
    pub enemy: Option<EnemyView>,
    /// The last hit another client dealt this body, within [`HURT_MS`].
    pub hurt: Option<Hurt>,
    /// A live teammate stands across the line to [`Self::enemy`]
    /// ([`friend_in_line`]): the trigger stays up.
    pub friend_in_line: bool,
    /// With [`Self::friend_in_line`], a spot a short way across the line
    /// on the graph from whose eye the round would pass the teammate
    /// ([`crate::nav::NavGraph::sidestep`]); the bot steps to it.
    pub sidestep: Option<[f32; 3]>,
    /// With [`Self::enemy`] in sight, a graph node near that hides the
    /// body from its eye and peeks at it ([`cover::find`]).
    pub cover: Option<cover::Spot>,
    /// A node near the spot a seen enemy was lost at whose eye sees it
    /// ([`hunt::vantage`]), for the spot [`Bot::hunt_query`] names.
    pub hunt: Option<[f32; 3]>,
    /// The cursor hint the end frame left is a weapon lying in reach: a
    /// use press from this view takes it (`Cmd_Activate_f` picks what
    /// `G_GetActivateEnt` scores best) and swaps the held weapon away.
    pub use_takes_weapon: bool,
    /// A held frag's configstring index, when one is in the kit (not a
    /// UO smoke grenade or satchel charge).
    pub grenade: Option<u8>,
    /// The frag's fuse runs from a spoon in the hand (UO's `cookOffHold`).
    pub grenade_cooks: bool,
    /// A frag is in the hand with its pin out and the pullback's
    /// `holdFireTime` run (`grenadeTimeLeft` set, `weaponDelay` at 1 or
    /// less): the trigger coming up now throws it.
    pub grenade_ready: bool,
    /// The nearest thrown frag or satchel still to go off within
    /// [`FLEE_REACH`] across and [`FLEE_HEIGHT`] up or down, anyone's own
    /// included: the bot runs from it.
    pub frag_near: Option<[f32; 3]>,
    /// A post the game sends the bot to, walked to over the graph ahead
    /// of a chase: CoD4's mounted gun a bot is making for
    /// (`bots::cod4::gun`). Never set on CoD1 or CoD2.
    pub post: Option<[f32; 3]>,
    /// A spot a step away from a teammate held up behind this bot, who is
    /// planning round it (`bots::driver`'s `give_way`): the bot makes
    /// room, whatever post it holds. Never set on CoD1 or CoD2.
    pub give_way: Option<[f32; 3]>,
    /// A lob at [`Self::enemy`] whose flight meets no wall or body short of
    /// most of the range, as (pitch, yaw) wire degrees (`Server::lob_probe`):
    /// a UO frag bounces off both at full speed, back at the thrower. Filled
    /// on the ticks [`Bot::wants_lob`] names; no throw starts without one.
    pub lob: Option<[f32; 2]>,
    /// The next point on the server's path toward [`Bot::goal`]; `None`
    /// while there is no path, and the bot wanders.
    pub waypoint: Option<[f32; 3]>,
    /// Per compass octant (0 is +x, counter-clockwise by 45 degrees), a
    /// minefield or `trigger_hurt` [`HAZARD_LOOK`] units that way. A wander
    /// heading keeps out of them; the graph already does.
    pub hazard_ahead: [bool; 8],
    /// Per octant as [`Self::hazard_ahead`], the depth of a drop deeper
    /// than a jump within [`HAZARD_LOOK`] that way, before a wall
    /// (`nav::drop_ahead`); all clear off the ground, and on a tick the
    /// brain cannot read it ([`Bot::reads_drops`]). A wander heading keeps
    /// off them too, and off one that hurts even when cornered.
    pub drop_ahead: [Option<f32>; 8],
    /// Per octant, a wall the standing body pins on at once that way
    /// (`nav::wall_ahead`), which the drop probe reads as no drop; all
    /// clear off the ground, and on a tick the brain cannot pick a
    /// heading ([`Bot::reads_walls`]). A wander heading takes one last.
    pub wall_ahead: [bool; 8],
    /// The loudest gunfire or blast another player made last tick within
    /// earshot ([`loudest`]), chest high.
    pub noise: Option<[f32; 3]>,
    /// `linkTo` holds the body (a plant or defuse in progress); it cannot
    /// move, so it is not stuck.
    pub linked: bool,
    /// The graph's nearest node to the body has no edge out
    /// (`nav::NavGraph::dead_end`): the only way off is a fall. Or, once the
    /// graph stands over the script's brushes, it reaches no node of the
    /// largest component and no spawn cut off the same way leads to it
    /// (`nav::NavGraph::pit`): no way out the graph knows. Or the follower
    /// found it hemmed in by parked vehicles (`nav::Follower::penned`).
    pub stranded: bool,
    /// `ps.on_ladder`: the climb looks up, as the graph's walks did.
    pub on_ladder: bool,
    /// `ps.ladder_normal`: a climb faces into it (`nav::climb_yaw`).
    pub ladder_normal: [f32; 3],
    /// The game's ladder takes no flat move into the face (CoD2's
    /// `PM_LadderMove`): the rungs let go only past their top, so a head
    /// across the face is climbed on to whatever its distance across.
    pub tops_out: bool,
    /// On a ladder, the middle of the rungs level with the body
    /// (`nav::NavGraph::ladder_middle`): a climb strafes toward it.
    pub ladder_middle: Option<[f32; 2]>,
    /// Backing down a ladder whose fall kills, the yaw square into its face
    /// and the middle of its rungs (`nav::Follower::back_face`): the creep
    /// off the head holds the one and strafes to the other.
    pub back_face: Option<(f32, [f32; 2])>,
    /// `ps.on_ground`.
    pub on_ground: bool,
    /// The way to [`Self::waypoint`] is a jump edge: jump where the
    /// graph's walk did (`nav::PinJump`), or at a leap's lip.
    pub jump: bool,
    /// `ps.sliding()`: off the ground against a face too steep to stand on.
    pub sliding: bool,
    /// The jump edge is a leap, taken from rest with a jump at the lip.
    pub leap: bool,
    /// On a jump edge, the body loses the ground next tick (`nav::lip_ahead`).
    pub lip: bool,
    /// The jump edge is a crouched one (`nav::Follower::crouch_jumping`):
    /// hold crouch from the cmd after the jump until the body lands.
    pub crouch_jump: bool,
    /// [`Self::waypoint`] is a leap's foot: come to rest on it.
    pub stop: bool,
    /// A teammate walks the same way just ahead ([`team::mate_ahead`]),
    /// on a level with a team objective: out of a fight the bot eases to a
    /// walk until he is a blast's width off ([`Bot::spaced`]).
    pub mate_ahead: bool,
    /// The bot's part in its team's S&D attack this round ([`team::squad`],
    /// set by the driver after the game's deal); `Main` elsewhere.
    pub squad: team::Squad,
    /// [`Self::waypoint`] lies more than [`BACK_DOWN_HEIGHT`] down a drop
    /// a run off arrives at (`nav::Follower::running_off`): run, don't back.
    pub run_off: bool,
    /// The edge under way is a CoD2 mantle (`nav::Follower::mantling`):
    /// pmove offers the climb only to a view on the wall
    /// (bot-combat.md 17.1), so no look turns it away.
    pub mantle: bool,
    /// The held weapon's `moveSpeedScale`: a bot following its path on the
    /// ground runs at the graph's pace, not the weapon's (`Bot::think`).
    pub move_scale: f32,
    /// `ps.fatigueScale` on a United Offensive server, `None` where there
    /// is no sprint.
    pub fatigue: Option<f32>,
    /// `pm_flags` 0x10000: the last cmd sprinted.
    pub sprinting: bool,
    /// How far the path runs on straight and level from the body
    /// (`nav::Follower::straight_run`): room for a sprint.
    pub sprint_room: f32,
    /// On the rungs with [`Self::waypoint`] a floor behind the climber's
    /// back: the yaw of the push-off that lands there
    /// (`nav::push_off`).
    pub push_off: Option<f32>,
    /// Standing pinned short of [`Self::waypoint`] on its floor: a heading
    /// that frees the body (`nav::unpin_yaw`), held for
    /// `nav::UNPIN_TICKS`.
    pub unpin: Option<f32>,
    /// A held pistol's configstring index, when one is in the kit.
    pub pistol: Option<u8>,
    /// A held rocket launcher (`weaponClass rocketlauncher`) with a round
    /// left, clip or reserve: what breaches a `bas` base.
    pub rocket: Option<u8>,
    /// `ps.weaponslots` 1 and 2, the two primaries (0 for an empty slot).
    /// With both full a primary pickup drops the weapon in hand, or slot
    /// 1's with the pistol out, never the pistol (`cod11-items.md` 5), so
    /// a `bas` bot draws the one it would rather lose before it taps.
    pub primaries: [u8; 2],
    /// The rocket launchers held (`weaponClass rocketlauncher`), as a
    /// `weapons_held` mask.
    pub launchers: u64,
    /// A held bullet weapon other than the pistol with a round left, clip
    /// or reserve: what the bot fights with once its primary is a spent
    /// launcher.
    pub gun: Option<u8>,
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
    /// United Offensive's `ctf`, `dom`, `hq` and `bas` objectives, on such
    /// a level only.
    pub uo: Option<UoView>,
    /// A death sits out the rest of the round (`sd`, `re`: the stock
    /// `Callback_PlayerKilled` spawns a spectator), so a wedged bot holds
    /// on rather than `kill` itself.
    pub one_life: bool,
    /// Health, clip size and cover, for the retreat (`bots/retreat.rs`).
    pub survive: retreat::Vitals,
    /// United Offensive vehicles the bot could board, on foot only
    /// (`vehicle`).
    pub vehicles: Vec<VehicleSpot>,
    /// The vehicle seat the bot sits in.
    pub seat: Option<SeatView>,
    /// The nearest enemy-crewed vehicle in a clear line, for a bot on foot
    /// with a loaded launcher or one driving a tank (`vehicle`).
    pub armor: Option<ArmorView>,
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
    /// One of the attackers nearest the teammate carrying an objective
    /// ([`team::escorts`]): it keeps by him, and the rest hold his goal.
    pub escort: bool,
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
    /// The site the bot goes to, `rank % sites` ([`team::site_rank`]):
    /// one per round for every attacker, the defenders spread over both.
    pub rank: usize,
    /// The bot is its team's bot nearest the bomb: the one defender that
    /// goes for the defuse while the rest keep off its line of sight.
    pub lead: bool,
    /// A teammate planting nearby ([`team::cover`]): the bot guards him
    /// rather than make for its own site.
    pub cover: Option<[f32; 3]>,
    /// A bomb one bot carries and only its carrier plants with (CoD4's
    /// key object); `None` where every attacker plants (CoD1, CoD2).
    pub key: Option<KeyBomb>,
}

/// CoD4's carried bomb (`_gameobjects.gsc` `setKeyObject`,
/// docs/research/bot-objectives.md 12).
#[derive(Clone, Copy, Debug)]
pub struct KeyBomb {
    /// Where the bomb lies, for the one bot that walks into it to take it.
    pub fetch: Option<[f32; 3]>,
    /// The bot carries it: the rest of its team guard the site.
    pub mine: bool,
    /// How long a plant and a defuse hold use.
    pub plant_ticks: u32,
    pub defuse_ticks: u32,
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

/// A United Offensive objective gametype as the bot plays it
/// (docs/research/bot-objectives.md section 7). The server fills it on a
/// `ctf`, `dom`, `hq` or `bas` level from `ScriptRuntime::uo_objectives`.
#[derive(Clone, Debug)]
pub enum UoView {
    Ctf(Box<CtfView>),
    Dom(DomView),
    Hq(HqView),
    Bas(BasView),
}

impl UoView {
    /// The bot's place among its team's bots by slot: roles and zones are
    /// dealt out by it, as [`SdView::rank`].
    fn rank(&self) -> usize {
        match self {
            UoView::Ctf(v) => v.rank,
            UoView::Dom(v) => v.rank,
            UoView::Hq(v) => v.rank,
            UoView::Bas(v) => v.rank,
        }
    }
}

/// Capture the flag: the bot's team's flag and the other team's.
#[derive(Clone, Debug)]
pub struct CtfView {
    pub ours: CtfFlagView,
    pub theirs: CtfFlagView,
    /// The bot carries the other team's flag.
    pub carrying: bool,
    /// A teammate carrying the other team's flag, where he stands (the
    /// compass shows teammates).
    pub escort: Option<[f32; 3]>,
    pub rank: usize,
}

#[derive(Clone, Copy, Debug)]
pub struct CtfFlagView {
    /// Its trigger at home, while it stands there.
    pub home: Option<SiteView>,
    /// Where it lies since its carrier fell; a touch by the other side
    /// takes it, one by its own returns it home.
    pub dropped: Option<SiteView>,
    /// The trigger a carrier of this flag captures it at, which wants the
    /// carrier's own flag at home.
    pub goal: SiteView,
    pub carried: bool,
}

/// Domination: every flag zone and who holds it.
#[derive(Clone, Debug)]
pub struct DomView {
    pub flags: Vec<DomFlagView>,
    pub rank: usize,
}

#[derive(Clone, Copy, Debug)]
pub struct DomFlagView {
    pub site: SiteView,
    /// The bot's team holds it; neutral and the other team's are taken by
    /// standing in the zone with no enemy in it.
    pub ours: bool,
    /// Both teams stand in it, so its capture is held: what the stalled
    /// bar tells a player in it (CoD4's `numTouching`; never set on UO).
    pub contested: bool,
}

/// Headquarters: the radio on show.
#[derive(Clone, Debug)]
pub struct HqView {
    pub radio: Option<HqRadioView>,
    /// No radio up: the compass marker of the next one.
    pub next: Option<[f32; 3]>,
    pub rank: usize,
}

#[derive(Clone, Copy, Debug)]
pub struct HqRadioView {
    pub at: [f32; 3],
    /// How near across a body has to stand.
    pub radius: f32,
    /// How far up or down (`level.zradioradius`: [`RADIO_HEIGHT`] on
    /// United Offensive, [`RADIO_HEIGHT_COD2`] on CoD2).
    pub height: f32,
    /// Where the bot walks to take it: the radio itself on United
    /// Offensive, a graph node inside the capture reach on CoD2
    /// (`NavGraph::radio_stand`).
    pub stand: [f32; 3],
    /// The bot's team holds it; neutral or not, anyone else's is stood at
    /// (the capture, or the neutralize).
    pub ours: bool,
}

/// Base assault: the bases still alive on either side.
#[derive(Clone, Debug)]
pub struct BasView {
    pub bases: Vec<BasBaseView>,
    /// Rocket launchers lying about to be picked up with the use key: the
    /// maps' `mpweapon_bazooka`, `_panzerschreck` and `_panzerfaust`, which
    /// respawn 15 s after a pickup, and drops with a round left.
    pub launchers: Vec<[f32; 3]>,
    /// Bullet weapons lying about with a round in the clip or the reserve:
    /// drops, mostly.
    pub guns: Vec<[f32; 3]>,
    pub rank: usize,
}

#[derive(Clone, Debug)]
pub struct BasBaseView {
    pub at: [f32; 3],
    pub ours: bool,
    /// Its health is gone: the other side can plant a charge on it.
    pub breached: bool,
    /// A charge ticks on it.
    pub planted: bool,
    /// While planted, whole seconds before the charge blows.
    pub blows_in: Option<f32>,
    /// Where the plant and the defuse are done: the objective model (the
    /// 150-unit reach) and the middle of the trigger the view has to rest
    /// on.
    pub charge: Option<BombView>,
    /// With [`BotView::rocket`] held, a point a rocket fired at from where
    /// the bot stands bursts within [`BREACH_NEAR`] of, the base's own
    /// origin among them (`Server::breach_shot`); only for the other
    /// side's bases still standing.
    pub shot: Option<[f32; 3]>,
    /// With [`BotView::rocket`] held, the graph's spots round the base a
    /// standing bot has such a shot from (`Server::breach_stands`).
    pub stands: Vec<[f32; 3]>,
}

/// What the server deals a `bas` bot's view from, once a tick
/// (`Server::bot_uo_view`).
pub(crate) struct Breach<'a> {
    /// The bot holds a loaded rocket launcher ([`BotView::rocket`]).
    pub rocket: bool,
    pub launchers: &'a [[f32; 3]],
    pub guns: &'a [[f32; 3]],
    /// The script models round the bases, which a rocket strikes.
    pub models: &'a [crate::game::combat::PlacedModel],
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

/// A tracked target in sight is kept over a new one unless the new one
/// stands nearer than this share of its range: picked nearest-first every
/// refresh, a bot between two enemies at like range swapped targets, and
/// paid a fresh reaction and full aim error, at each swap.
pub(crate) const RETARGET_RATIO: f32 = 0.6;

/// Whether `at` lies inside the sight cone `fov` degrees across about the
/// view `view` (wire degrees, pitch down) from `eye`.
pub(crate) fn in_sight_cone(eye: glam::Vec3, view: [f32; 3], at: glam::Vec3, fov: f32) -> bool {
    let [pitch, yaw, _] = view.map(f32::to_radians);
    let forward = glam::Vec3::new(
        pitch.cos() * yaw.cos(),
        pitch.cos() * yaw.sin(),
        -pitch.sin(),
    );
    fov >= 360.0 || (at - eye).normalize_or_zero().dot(forward) >= (fov * 0.5).to_radians().cos()
}

/// The enemy a bot fights from `order`, the candidates' (squared range,
/// slot) nearest first; `sighted(i)` traces the `i`th and gives its view
/// when the eye line reaches it. The `attacker` of a fresh hit in sight
/// comes first, then a new one nearer than [`RETARGET_RATIO`] of the
/// `tracked` target's range, then the tracked one, then the nearest.
pub(crate) fn pick_enemy(
    order: &[(f32, usize)],
    attacker: Option<usize>,
    tracked: Option<usize>,
    sighted: impl Fn(usize) -> Option<EnemyView>,
) -> Option<EnemyView> {
    let find = |slot: Option<usize>| {
        let i = order.iter().position(|c| Some(c.1) == slot)?;
        Some((order[i].0, sighted(i)?))
    };
    if let Some((_, e)) = find(attacker) {
        return Some(e);
    }
    let held = find(tracked);
    let reach = held.map_or(f32::INFINITY, |(d, _)| d * RETARGET_RATIO * RETARGET_RATIO);
    order
        .iter()
        .enumerate()
        .filter(|(_, c)| c.0 < reach && Some(c.1) != tracked && Some(c.1) != attacker)
        .find_map(|(i, _)| sighted(i))
        .or(held.map(|(_, e)| e))
}

/// A hit [`BotView::hurt`] reports for this long, ms: the attacker is
/// preferred as a target and, out of sight, looked for along the hit.
pub(crate) const HURT_MS: i32 = 2000;
/// How long a hit from an unseen enemy turns the view toward it, ms.
const HURT_LOOK_MS: i32 = 1500;
/// A target taken again after [`Bot::prior`] keeps its error times this;
/// one who just hit the bot starts at this share of `error_start`. Both
/// take a third and a half of the reaction delay.
const REACQUIRE_ERROR: f32 = 1.25;
const HIT_BY_ERROR: f32 = 0.75;

/// The last hit another client dealt a bot.
#[derive(Clone, Copy, Debug)]
pub struct Hurt {
    /// The attacker's client slot.
    pub slot: usize,
    /// Ms since the hit.
    pub ago_ms: i32,
    /// The damage direction, from the attacker toward the body; zero
    /// when the hit carried none.
    pub dir: [f32; 3],
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

/// The point a bot aims at on a body standing at `origin` with its eye
/// `eye_height` over it: two thirds of the way up to the eye, 40 on a
/// standing body, 26.7 crouched, 7.3 prone, so a crouched or prone enemy
/// is not shot over.
pub(crate) fn chest(origin: [f32; 3], eye_height: f32) -> [f32; 3] {
    [origin[0], origin[1], origin[2] + eye_height * (2.0 / 3.0)]
}

/// Where a bot aims at an enemy whose chest is `chest` and eye `eye`, given
/// whether the line from its own eye reaches a point: the chest, or over a
/// wall that covers the chest but not the eye, the head; `None` when the
/// line reaches neither. Aimed at a chest behind cover, every round goes
/// into the cover.
pub(crate) fn aim_point(
    chest: [f32; 3],
    eye: [f32; 3],
    clear: impl Fn([f32; 3]) -> bool,
) -> Option<[f32; 3]> {
    [chest, eye].into_iter().find(|p| clear(*p))
}

/// How long a bot keeps walking to one sidestep before it picks again.
const SIDESTEP_HOLD_MS: i32 = 3000;
/// A sidestep this near across is reached: the bot picks again if the line
/// is still blocked from there.
const SIDESTEP_REACHED: f32 = 16.0;

/// The sidestep a bot at `at` keeps walking to: `held`, the spot and the
/// sv_time it was picked, while it is not yet reached, not
/// [`SIDESTEP_HOLD_MS`] old and `clear` still passes it. Picked afresh
/// every tick, the nearest spot moves with the body and a moving
/// teammate's line, and the bot jitters between spots.
pub(crate) fn held_sidestep(
    held: Option<([f32; 3], i32)>,
    now_ms: i32,
    at: glam::Vec3,
    clear: impl Fn(glam::Vec3) -> bool,
) -> Option<[f32; 3]> {
    let (p, since) = held?;
    let v = glam::Vec3::from(p);
    (now_ms.wrapping_sub(since) < SIDESTEP_HOLD_MS
        && (v - at).truncate().length() > SIDESTEP_REACHED
        && clear(v))
    .then_some(p)
}

/// How far round a teammate's box a shot is held, units: aim error and
/// spread put a round a few units off the line.
const FRIEND_MARGIN: f32 = 8.0;

/// Whether the line from `eye` to `target` crosses any of `friends`' boxes
/// (`(mins, maxs)`, world), each grown by [`FRIEND_MARGIN`], short of the
/// target: a round fired at the target would strike a teammate first.
pub(crate) fn friend_in_line(
    eye: glam::Vec3,
    target: glam::Vec3,
    friends: &[(glam::Vec3, glam::Vec3)],
) -> bool {
    let m = glam::Vec3::splat(FRIEND_MARGIN);
    friends
        .iter()
        .any(|(lo, hi)| crate::game::combat::ray_box(eye, target, *lo - m, *hi + m).is_some())
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
/// to its range scaled by `hearing` ([`Skill::hearing`]), first in recorded
/// order on a tie. Teammates' fights count too: a friend shooting means an
/// enemy close to him. The server leaves out those its teammates nearer the
/// noise answer instead (`team::answers`).
pub(crate) fn loudest(
    noises: &[Noise],
    listener: usize,
    at: [f32; 3],
    hearing: f32,
) -> Option<[f32; 3]> {
    noises
        .iter()
        .filter(|n| n.source != listener)
        .map(|n| (dist_sq(at, n.at).sqrt() / (n.radius * hearing), n.at))
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
    /// `ps.weapon`, and whether it fires bullets (`weaponType bullet`):
    /// `bot_match` counts a bullet gun's clip drops as rounds fired.
    pub weapon: u8,
    pub gun: bool,
    /// On the random heading a stuck bot takes, waypoint or not.
    pub unsticking: bool,
    /// UO's `pm_flags` 0x10000: the last cmd sprinted.
    pub sprinting: bool,
    /// The last think's leg was one a bot sprints, breath aside
    /// ([`Bot::sprint_leg`]): what a sprint gate measures against.
    pub sprint_leg: bool,
    /// A UO light machine gun's bipod is down: the body holds still on
    /// purpose.
    pub deployed: bool,
    /// Tracking an enemy it saw within the last second: a scoped or
    /// sighted shot at range stands still on purpose.
    pub engaged: bool,
    /// Breaking contact (`bots/retreat.rs`): 1 hurt, 2 dry, 3 at the
    /// cover, 0 not.
    pub retreating: u8,
    /// The UO vehicle (entity number) and seat it sits in.
    pub seat: Option<(u32, i32)>,
    /// Driving, and holding the hull for a teammate walking up to board.
    pub crew_wait: bool,
    /// Times it got into a vehicle.
    pub boardings: u32,
    /// Rockets it fired on foot at enemy vehicles.
    pub rockets_at_armor: u32,
    /// The client slot the bot tracks ([`Bot::target_slot`]), and whether
    /// its last cmd held the trigger.
    pub target: Option<usize>,
    pub firing: bool,
    /// The held weapon's `clipSize` when it is a bullet weapon, else 0.
    pub clip_size: i16,
    /// Grenades it let go this game ([`Bot::throws`]).
    pub throws: u32,
    /// The view's yaw in degrees, and whether the last think steered the
    /// body along its waypoint with no target tracked: `bot_match`'s
    /// walking turn columns.
    pub yaw: f32,
    pub walking: bool,
    /// Holding a cover spot ([`cover`]).
    pub holds_cover: bool,
    /// Planting, defusing, or standing at its objective's spot (a guard
    /// ring, a site, the HQ radio) last think.
    pub on_objective: bool,
}

impl BotBody {
    /// Holding still on purpose: tracking an enemy, on a cover spot,
    /// retreating or at the cover, bipod down, in a seat or holding the
    /// hull for crew, or at its objective. `bot_match` counts a 12 s
    /// stall tick that holds as `held`, the rest as `stalled`
    /// (bot-combat.md section 1).
    pub fn holds(&self) -> bool {
        self.engaged
            || self.holds_cover
            || self.retreating != 0
            || self.deployed
            || self.seat.is_some()
            || self.crew_wait
            || self.on_objective
    }
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
    /// The tick's usercmd is stamped with the server's own clock; the driver
    /// stamps it, so the brain carries only the input state below.
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
    /// `ps.weapon` reads it (AGENTS.md, "Every cmd carries the weapon").
    switch_to: Option<u8>,
    /// `bas` launchers tapped for with nothing taken, and the ticks each
    /// stays passed over ([`SHUN_TICKS`]); the last tapped and its taps.
    shunned: Vec<([f32; 3], u32)>,
    tapped: Option<([f32; 3], u32)>,
    /// The `bas` launcher walked to, the nearest the body has come to it
    /// and the ticks since it last came nearer.
    fetch: Option<([f32; 3], f32, u32)>,
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
    /// An LMG's bipod: ticks the sight button has been held with no deploy,
    /// ticks left before another try once one was refused or the target
    /// left the yaw cap, and ticks the bipod stays down with no enemy.
    lmg_tries: u32,
    lmg_rest: u32,
    /// Ticks in sight of an enemy with no round fired at him, and the
    /// enemy given up on for a while past [`FUTILE_TICKS`]
    /// ([`Bot::ignores`]).
    futile: u32,
    ignored: Option<(usize, u32)>,
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
    /// The last reliable server command the bot consumed.
    pub(crate) last_seen_seq: i32,
    /// The client-command sequence the bot's own replies use.
    pub(crate) next_command_seq: i32,
    /// The retreat (`bots/retreat.rs`): whether this bot retreats at all,
    /// the one under way, the health the last hurt one started at, and
    /// whether this dry clip already had its retreat.
    retreat_on: bool,
    retreat: Option<retreat::Retreat>,
    hurt_at: i32,
    dry_had: bool,
    /// Boarding, driving and leaving a vehicle (`vehicle`).
    rider: vehicle::Rider,
    /// The last cmd held the trigger.
    fired: bool,
    /// The target last dropped or replaced: its slot, its aim error then,
    /// and the ticks since ([`Bot::track`]).
    prior: Option<(usize, f32, u32)>,
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
    /// The domination flag this life is set on, by its index, until its
    /// team holds it.
    zone: Option<usize>,
    /// Ticks stood at a breach stand with no shot from it; past
    /// [`BREACH_IDLE_TICKS`] the next stand is tried (`pick`).
    idle: u32,
    /// Ticks since an S&D defuser's, or a `bas` planter's or defuser's,
    /// settled view picked a weapon lying by the charge instead of its
    /// trigger; while set it closes in to [`DEFUSE_CLOSE`].
    close_in: u32,
    /// The far side of a contested zone the bot stands in, walked to
    /// before it settles again ([`Bot::sweep_contested`]).
    sweep: Option<[f32; 3]>,
    /// The objective's point the follower walked out its plan to with the
    /// objective not reached, the nearest the body has come to it since
    /// and the ticks since it last came [`SHORT_GAIN`] nearer
    /// ([`Bot::track_reach`]).
    short: Option<([f32; 3], f32, u32)>,
    /// An objective's point given up on and the ticks left: the objective
    /// there reads as a guard round it ([`Bot::objective_target`]).
    shy: Option<([f32; 3], u32)>,
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
    /// Walk within `reach` of the bomb, rest the view on it and hold use
    /// `ticks`: S&D's defuse, and base assault's plant and defuse.
    Defuse {
        bomb: BombView,
        reach: f32,
        ticks: u32,
    },
    /// Stand inside a zone until the script hands it over: a domination
    /// flag, or a capture goal waiting on the carrier's own flag.
    Occupy(SiteView),
    /// Stand within `radius` across and `height` up or down of a
    /// headquarters radio, walking to `stand`.
    Radio {
        at: [f32; 3],
        radius: f32,
        height: f32,
        stand: [f32; 3],
    },
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
    /// Fire rockets at a `bas` base: walk toward it, and from where a shot
    /// bursts at it (`aim`) stand and fire (bot-objectives.md 7.2).
    Breach {
        base: [f32; 3],
        stand: Option<[f32; 3]>,
        aim: Option<[f32; 3]>,
    },
}

/// `level.planttime` is 5 s (`sd.gsc` `bombzones`); a hold this long that
/// planted nothing has failed.
const PLANT_TICKS: u32 = 120;
/// `level.defusetime` is 10 s.
const DEFUSE_TICKS: u32 = 210;
/// The defuse wants `distance(origin, trigger.origin) < 64`.
const DEFUSE_REACH: f32 = 48.0;
/// A defuser whose press would take a weapon lying by the bomb stands this
/// near the bomb instead: `G_GetActivateEnt` scores an entity dead on the
/// view at its distance from the eye, so with the eye over the bomb the
/// trigger's middle is nearer than anything lying on the floor.
const DEFUSE_CLOSE: f32 = 16.0;
/// Past this long closing in (the bomb's own body in the way), the press
/// goes ahead whatever it takes.
const DEFUSE_CLOSE_TICKS: u32 = 100;
const GUARD_RADIUS: f32 = 250.0;
/// Ticks a bot that walked out its plan to an objective's point may go
/// without coming [`SHORT_GAIN`] nearer it before it gives the point up
/// ([`Bot::track_reach`]).
const SHORT_TICKS: u32 = 60;
const SHORT_GAIN: f32 = 16.0;
/// Ticks a point given up on reads as a guard round it.
const SHY_TICKS: u32 = 300;
/// How near a target's point must stand to the one given up on to be it.
const SHY_SAME: f32 = 32.0;
/// A guard ring is at least this wide.
const GUARD_BAND: f32 = 150.0;
/// An attacker steps off the bomb it planted: a body on the line from a
/// defender's eye to the trigger blocks the defuse's `isLookingAt`, which
/// is the defender's problem, not the bot's to make.
const BOMB_GUARD_INNER: f32 = 96.0;
/// An attacker this near its own dropped flag returns it on the way.
const CTF_RETURN_NEAR: f32 = 1000.0;
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
/// `level.zradioradius` (`hq.gsc` 186): how far up or down of a radio a
/// body still counts; the bot stands this far inside both reaches.
pub(crate) const RADIO_HEIGHT: f32 = 50.0;
/// CoD2's `level.zradioradius` (`hq.gsc` 214, iw_15.iwd): 72, and
/// `level.radioradius` 120 is a distance in three dimensions (1255).
pub(crate) const RADIO_HEIGHT_COD2: f32 = 72.0;
pub(crate) const RADIO_MARGIN: f32 = 16.0;
/// `scr_bas_planttime` 15 s and `scr_bas_defusetime` 10 s, a half second
/// over; the hold aborts past 155 units from the objective model.
const BAS_PLANT_TICKS: u32 = 310;
const BAS_DEFUSE_TICKS: u32 = 210;
const BAS_REACH: f32 = 112.0;
/// A base's charge goes off with a 1000-damage `radiusDamage` on every
/// player within 500 units of it (`BaseAssault_bomb_countdown`).
const BAS_BLAST: f32 = 520.0;
/// An attacker holds the room of its own charge until this many seconds
/// are left on it, then gets out of the blast: with 8 s a planter climbing
/// out of the room was still inside it when it went.
const BAS_CLEAR_S: f32 = 20.0;
/// An attacker this near a charge on its own base turns back to defuse it.
const BAS_DEFUSE_NEAR: f32 = 2000.0;
/// The graph's run, units a second: a bot this far from a charge in
/// straight line needs at least that long to reach it.
const BAS_RUN_SPEED: f32 = 190.0;
/// A rocket's burst counts at a base this near its origin: the base links
/// as a point with the turret's box round it (`GameHost::blast_entities`),
/// and the rocket's 256-unit splash falls off from 100 to 10.
pub const BREACH_NEAR: f32 = 64.0;
/// A rocket is fired at a base from this far at least, clear of its own
/// 256-unit splash, and at most this far.
pub const BREACH_MIN: f32 = 400.0;
/// A rocket's burst on the way to a base is this far from the muzzle at
/// least: the launchers' 256-unit splash, and a margin.
pub const BREACH_CLEAR: f32 = 320.0;
pub const BREACH_RANGE: f32 = 1500.0;
/// A base's own launchers (`mpweapon_*` with `script_gameobjectname bas`)
/// lie in its bunker, this near its origin, behind the walls a breach
/// deletes (about 145 units on mp_foy).
const BAS_BUNKER: f32 = 300.0;
/// Dry this long, a bot uses `kill` (`Bot::think`).
const DRY_TICKS: u32 = 200;
/// A dry `bas` bot fetches a weapon lying this near instead.
const DRY_FETCH: f32 = 1500.0;
/// A `bas` bot left with only a rocket ([`Bot::unarmed`]) fetches a gun
/// this near: from 1000 and 1500 units off, the walk cost mp_foy's seeds 7
/// and 11 a minute and more.
const ARMS_FETCH: f32 = 800.0;
/// Ticks a fetch may go without coming [`FETCH_CLOSING`] nearer its
/// launcher (the reach included: one stood at counts down too).
const FETCH_STALL_TICKS: u32 = 400;
const FETCH_CLOSING: f32 = 32.0;
/// Taps at one launcher before it is passed over, and for how long.
const LAUNCHER_TAPS: u32 = 3;
const SHUN_TICKS: u32 = 1200;
/// Stood this long at a breach stand with no shot, the bot tries another.
const BREACH_IDLE_TICKS: u32 = 40;
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

/// Whom a throw under way goes at ([`Bot::throw_enemy`]).
#[derive(Clone, Copy)]
enum ThrowFor {
    /// Any enemy in sight (a stage set by hand in a test).
    Any,
    /// The enemy in this slot while in sight, else where he was last seen.
    Enemy(usize),
    /// A lost enemy's spot, whoever comes into sight.
    Spot,
}

enum Stage {
    Wander,
    ToGrenade,
    /// The pullback held `left` more ticks; from `spoon` ticks left the
    /// sight button is held too, which starts a UO fuse.
    Cook {
        left: u32,
        spoon: u32,
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
            cover: cover::Cover::default(),
            team_play: true,
            squad: true,
            start_hold: 0,
            footwork: true,
            aware: true,
            wary: true,
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
            last_seen_seq: 0,
            next_command_seq: 1,
            rider: Default::default(),
            fired: false,
            prior: None,
            team: team.to_string(),
            rng: seed | 1,
            retreat_on: true,
            retreat: None,
            hurt_at: i32::MAX,
            dry_had: false,
        }
    }

    /// xorshift64* masked to 31 bits off the bot's own state.
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
                // `switchToWeapon` and a use pickup's `a <n>`: a cgame
                // selects `n` and its cmds ask for it (combat doc 9.8), so the
                // bot's byte does too; the pickup half reads only the byte.
                ["a", w] => {
                    if let Ok(w) = w.parse::<u8>() {
                        self.switch_to = Some(w);
                    }
                }
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
        let open: Vec<&'static str> = menu_weapons(self.menus, &menu)?
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

    /// The weapon menu and the weapon the bot answers it with, once picked.
    pub(crate) fn loadout(&self) -> Option<(&str, &'static str)> {
        self.loadout.as_ref().map(|(m, w)| (m.as_str(), *w))
    }

    /// The weapon the menu `menu` is answered with from now on, if it
    /// offers it and the cvars allow it.
    pub(crate) fn set_loadout(&mut self, menu: &str, weapon: &'static str) {
        self.loadout = Some((menu.to_string(), weapon));
    }

    /// A new gamestate reruns `ClientConnect` and reopens the menus under the
    /// indices the last one used; without the clear the bot never spawns
    /// again (the same rejoin the probes learned).
    pub fn rearm(&mut self) {
        self.main_menu.clear();
        self.answered.clear();
        self.round_start = true;
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
        if let Some(c) = Self::blast_near(view) {
            return Goal::Away(c);
        }
        if self.start_held(view) {
            return Goal::Hold;
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

    /// A `bas` charge about to go off with the bot inside its blast, fight
    /// or no fight: one of its own side's in the last [`BAS_CLEAR_S`], one
    /// on its own base once a defuse could no longer finish.
    fn blast_near(view: &BotView) -> Option<[f32; 3]> {
        let Some(UoView::Bas(b)) = view.uo.as_ref() else {
            return None;
        };
        b.bases.iter().filter(|x| x.planted).find_map(|x| {
            let (left, c) = (x.blows_in?, x.charge?);
            let late = if x.ours {
                left < BAS_DEFUSE_TICKS as f32 / 20.0
            } else {
                left <= BAS_CLEAR_S
            };
            (late && dist_sq(view.origin, c.origin) < BAS_BLAST * BAS_BLAST).then_some(c.origin)
        })
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

    /// The objective this tick, from the bot's role and the round's state:
    /// [`Self::objective_wanted`], or a guard round its point while that
    /// point is given up on ([`Bot::track_reach`]).
    fn objective_target(&self, view: &BotView) -> Option<ObjTarget> {
        let t = self.objective_wanted(view)?;
        match (self.obj.shy, Self::target_point(&t)) {
            (Some((at, _)), Some(p)) if dist_sq(at, p) < SHY_SAME * SHY_SAME => {
                Some(ObjTarget::Guard { at: p, inner: 0.0 })
            }
            _ => Some(t),
        }
    }

    /// The point a target's walk heads for when the bot may give it up
    /// ([`Bot::track_reach`]): not a guard ring, which holds anywhere in
    /// it, nor a breach stand, which waits there for its shot.
    fn target_point(t: &ObjTarget) -> Option<[f32; 3]> {
        match *t {
            ObjTarget::Plant(s) | ObjTarget::Occupy(s) => Some(s.stand),
            ObjTarget::Defuse { bomb, .. } => Some(bomb.origin),
            ObjTarget::Radio { stand, .. } => Some(stand),
            ObjTarget::Pickup { from, .. } => Some(from),
            ObjTarget::Deliver(at) | ObjTarget::Chase(at) | ObjTarget::Sweep(at) => Some(at),
            ObjTarget::Guard { .. } | ObjTarget::Breach { .. } => None,
        }
    }

    /// Gives up an objective's point the bot cannot close on: its follower
    /// walked out the plan to it ([`Self::arrived_goal`]), the objective is
    /// not reached, and the body came no [`SHORT_GAIN`] nearer the point
    /// in [`SHORT_TICKS`]. The point then reads as a guard round it for
    /// [`SHY_TICKS`]. A plan's end short of a point no node reaches is the
    /// node nearest it, and from there the bot walked at the point into a
    /// wall or a teammate standing on it for the rest of the round
    /// (bot-objectives.md 12.19).
    fn track_reach(&mut self, view: &BotView) {
        if let Some((at, left)) = self.obj.shy {
            self.obj.shy = (left > 1).then_some((at, left - 1));
        }
        let Some(p) = self.arrived_goal.take() else {
            self.obj.short = None;
            return;
        };
        let short = self.objective_wanted(view).filter(|t| {
            self.obj.shy.is_none()
                && Self::target_point(t) == Some(p)
                && !Self::at_objective(view, t)
                && !self.objective_busy(view)
        });
        if short.is_none() {
            self.obj.short = None;
            return;
        }
        let d = dist_sq(view.origin, p).sqrt();
        let (best, ticks) = match self.obj.short {
            Some((at, best, ticks)) if at == p && d > best - SHORT_GAIN => (best, ticks + 1),
            _ => (d, 0),
        };
        if ticks >= SHORT_TICKS {
            self.obj.shy = Some((p, SHY_TICKS));
            self.obj.short = None;
        } else {
            self.obj.short = Some((p, best, ticks));
        }
    }

    /// [`Self::objective_target`] before a given-up point turns it to a
    /// guard.
    fn objective_wanted(&self, view: &BotView) -> Option<ObjTarget> {
        if let Some(uo) = view.uo.as_ref() {
            return self.uo_target(view, uo);
        }
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
            (ObjRole::Attack, None) => match (sd.key, sd.cover) {
                (
                    Some(KeyBomb {
                        fetch: Some(at), ..
                    }),
                    _,
                ) => Some(ObjTarget::Deliver(at)),
                (_, Some(at)) => Some(ObjTarget::Guard {
                    at,
                    inner: team::COVER_INNER,
                }),
                (Some(k), None) if !k.mine => site().map(|s| ObjTarget::Guard {
                    at: s.stand,
                    inner: 0.0,
                }),
                _ => site().map(ObjTarget::Plant),
            },
            (ObjRole::Defend, Some(b)) if sd.lead => Some(ObjTarget::Defuse {
                bomb: b,
                reach: self.press_reach(DEFUSE_REACH),
                ticks: sd.key.map_or(DEFUSE_TICKS, |k| k.defuse_ticks),
            }),
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
                if let Some(at) = Self::carrier(view, o).filter(|_| re.escort) {
                    return Some(ObjTarget::Guard { at, inner: 0.0 });
                }
                // The rest go ahead and hold the goal he makes for.
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

    /// United Offensive's objective gametypes. Odd ranks keep their own
    /// side's objective and even ranks go for the other side's, so a lone
    /// bot attacks.
    fn uo_target(&self, view: &BotView, uo: &UoView) -> Option<ObjTarget> {
        let rank = uo.rank();
        let defends = rank % 2 == 1;
        let guard = |at: [f32; 3]| ObjTarget::Guard { at, inner: 0.0 };
        match uo {
            UoView::Ctf(c) => {
                if c.carrying {
                    // The capture wants our flag home: wait in the goal.
                    return Some(if c.ours.home.is_some() {
                        ObjTarget::Deliver(c.theirs.goal.stand)
                    } else {
                        ObjTarget::Occupy(c.theirs.goal)
                    });
                }
                // Our flag lying out: a touch returns it, and their carrier
                // cannot score until it is home.
                if let Some(d) = c.ours.dropped
                    && (defends
                        || c.theirs.carried
                        || dist_sq(view.origin, d.stand) < CTF_RETURN_NEAR * CTF_RETURN_NEAR)
                {
                    return Some(ObjTarget::Deliver(d.stand));
                }
                if defends {
                    return Some(match c.ours.home {
                        Some(h) => guard(h.stand),
                        // Carried off: hold the goal their carrier needs.
                        None => ObjTarget::Guard {
                            at: c.ours.goal.stand,
                            inner: 0.0,
                        },
                    });
                }
                if let Some(at) = c.theirs.home.or(c.theirs.dropped) {
                    return Some(ObjTarget::Deliver(at.stand));
                }
                c.escort.map(guard)
            }
            UoView::Dom(d) => {
                if let Some(at) = self.obj.sweep {
                    return Some(ObjTarget::Sweep(at));
                }
                let open: Vec<usize> = (0..d.flags.len()).filter(|i| !d.flags[*i].ours).collect();
                if open.is_empty() {
                    let n = d.flags.len();
                    return (n > 0).then(|| guard(d.flags[rank % n].site.stand));
                }
                let pick = self
                    .obj
                    .zone
                    .filter(|z| open.contains(z))
                    .unwrap_or_else(|| self.dom_pick(view, d, &open, rank));
                Some(ObjTarget::Occupy(d.flags[pick].site))
            }
            UoView::Hq(h) => match h.radio {
                Some(r) if r.ours => Some(guard(r.at)),
                Some(r) => Some(ObjTarget::Radio {
                    at: r.at,
                    radius: r.radius,
                    height: r.height,
                    stand: r.stand,
                }),
                None => h.next.map(guard),
            },
            UoView::Bas(b) => {
                let side = |ours: bool| -> Vec<&BasBaseView> {
                    b.bases.iter().filter(|x| x.ours == ours).collect()
                };
                let (mine, theirs) = (side(true), side(false));
                // A charge on one of ours: every defender goes for it, and
                // an attacker near it. One that does not shoot only from
                // near: the planter cannot stop it, and a side's defuser
                // back from across the map in time for every plant left
                // two lone bases standing for good (seeds 17 and 19).
                // Not one that goes off before a defuse could finish, nor
                // one whose hold under way cannot: the defuser held use
                // inside the blast to the end (seed 5). A second to settle
                // the view on the trigger.
                let fits = |x: &BasBaseView, c: BombView| {
                    let need = if self.obj.held > 0 {
                        BAS_DEFUSE_TICKS.saturating_sub(self.obj.held) as f32 / 20.0
                    } else {
                        let trip = dist_sq(view.origin, c.origin).sqrt() / BAS_RUN_SPEED;
                        BAS_DEFUSE_TICKS as f32 / 20.0 + trip + 1.0
                    };
                    x.blows_in.is_none_or(|s| s > need)
                };
                let ticking = mine
                    .iter()
                    .filter(|x| x.planted)
                    .find_map(|x| x.charge.filter(|c| fits(x, *c)));
                if let Some(c) = ticking
                    && ((defends && self.shoot)
                        || theirs.is_empty()
                        || dist_sq(view.origin, c.origin) < BAS_DEFUSE_NEAR * BAS_DEFUSE_NEAR)
                {
                    return Some(ObjTarget::Defuse {
                        bomb: c,
                        reach: self.press_reach(BAS_REACH),
                        ticks: BAS_DEFUSE_TICKS,
                    });
                }
                // Nothing left to fire: the nearest weapon lying within
                // `DRY_FETCH`, before the `kill` (`DRY_TICKS`). A planter
                // that ran dry in its charge's room killed itself, and the
                // charge was defused (seeds 1 and 7).
                // With a rocket left, a gun only, and from nearer.
                let dry = Self::dry(view);
                let reach = if dry { DRY_FETCH } else { ARMS_FETCH };
                if (dry || self.unarmed(view))
                    && let Some(&w) =
                        b.guns
                            .iter()
                            .chain(b.launchers.iter().filter(|l| {
                                dry && b.bases.iter().all(|x| {
                                    x.breached || dist_sq(**l, x.at) > BAS_BUNKER * BAS_BUNKER
                                })
                            }))
                            .filter(|l| dist_sq(view.origin, **l) < reach * reach)
                            .filter(|l| !self.shunned.iter().any(|s| arrived(s.0, **l)))
                            .min_by(|x, y| {
                                dist_sq(view.origin, **x).total_cmp(&dist_sq(view.origin, **y))
                            })
                {
                    return Some(ObjTarget::Pickup { aim: w, from: w });
                }
                // A defender keeps the charge room of a breached base of
                // ours, the one way a plant comes; with none breached there
                // is nothing to defend and it attacks with the rest. One
                // that does not shoot stops no plant there, and defused
                // each charge the moment it was down for good: it attacks
                // too, and defuses what it passes near.
                let open: Vec<&&BasBaseView> = mine.iter().filter(|x| x.breached).collect();
                // A charge of ours ticking: a defender that shoots joins
                // its planter in holding its room. One on one, the
                // defender in the room won most fights and defused. So does
                // one whose side has breached the other side's last base,
                // charged or not: it came from its own base after the
                // planter had died and the charge was defused (seed 23).
                let backing = theirs
                    .iter()
                    .find(|x| x.planted && x.charge.is_some())
                    .or_else(|| theirs.first().filter(|x| theirs.len() == 1 && x.breached))
                    .filter(|_| self.shoot);
                if theirs.is_empty()
                    || (defends && self.shoot && !open.is_empty() && backing.is_none())
                {
                    let n = open.len();
                    return if n > 0 {
                        let x = open[(rank / 2) % n];
                        Some(guard(x.charge.map_or(x.at, |c| c.origin)))
                    } else {
                        (!mine.is_empty()).then(|| guard(mine[(rank / 2) % mine.len()].at))
                    };
                }
                // Without shooting, a side's two bots split over the other
                // side's bases: on one, its lone defender ran back and
                // defused every plant (seed 7's 67 defuses).
                let pick = if self.shoot { rank / 2 } else { rank };
                let t = match backing {
                    Some(x) if defends => x,
                    _ => theirs[(pick + self.deaths as usize) % theirs.len()],
                };
                Some(match t.charge {
                    // Our charge: hold its room against the defuse, then
                    // clear the blast before it goes.
                    Some(c) if t.planted => ObjTarget::Guard {
                        at: c.origin,
                        inner: if t.blows_in.is_some_and(|s| s > BAS_CLEAR_S) {
                            0.0
                        } else {
                            BAS_BLAST
                        },
                    },
                    Some(c) if t.breached => ObjTarget::Defuse {
                        bomb: c,
                        reach: self.press_reach(BAS_REACH),
                        ticks: BAS_PLANT_TICKS,
                    },
                    _ if t.breached => guard(t.at),
                    // Standing: rockets breach it. Without one, the nearest
                    // launcher lying about first.
                    _ if view.rocket.is_some() => ObjTarget::Breach {
                        base: t.at,
                        stand: self.breach_stand(&mine, t),
                        aim: t.shot,
                    },
                    // Those in a standing base, ours or theirs, lie behind
                    // its walls.
                    _ => match b
                        .launchers
                        .iter()
                        .filter(|l| {
                            b.bases
                                .iter()
                                .all(|x| x.breached || dist_sq(**l, x.at) > BAS_BUNKER * BAS_BUNKER)
                        })
                        .filter(|l| !self.shunned.iter().any(|s| arrived(s.0, **l)))
                        .min_by(|x, y| {
                            dist_sq(view.origin, **x).total_cmp(&dist_sq(view.origin, **y))
                        }) {
                        Some(&l) => ObjTarget::Pickup { aim: l, from: l },
                        None => guard(t.at),
                    },
                })
            }
        }
    }

    /// The breach stand a bot with a rocket walks to: the stands nearest
    /// its own side's nearest base first, one given up on (`pick`) for the
    /// next.
    fn breach_stand(&self, mine: &[&BasBaseView], t: &BasBaseView) -> Option<[f32; 3]> {
        let home = mine
            .iter()
            .map(|m| m.at)
            .min_by(|a, b| dist_sq(*a, t.at).total_cmp(&dist_sq(*b, t.at)))
            .unwrap_or(t.at);
        let mut stands = t.stands.clone();
        stands.sort_by(|a, b| dist_sq(*a, home).total_cmp(&dist_sq(*b, home)));
        (!stands.is_empty()).then(|| stands[self.obj.pick as usize % stands.len()])
    }

    /// The domination flag a bot sets out for: among those its team does
    /// not hold, nearest first, dealt out by rank.
    fn dom_pick(&self, view: &BotView, d: &DomView, open: &[usize], rank: usize) -> usize {
        let mut by_dist = open.to_vec();
        by_dist.sort_by(|a, b| {
            let da = dist_sq(view.origin, d.flags[*a].site.stand);
            let db = dist_sq(view.origin, d.flags[*b].site.stand);
            da.total_cmp(&db).then(a.cmp(b))
        });
        by_dist[rank % by_dist.len()]
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
            ObjTarget::Plant(s) => inside_site(o, s),
            ObjTarget::Guard { at, inner } => {
                let d = dist_sq(o, *at);
                let outer = GUARD_RADIUS.max(inner + GUARD_BAND);
                d < outer * outer && d >= inner * inner
            }
            ObjTarget::Defuse { bomb, reach, .. } => dist_sq(o, bomb.origin) < reach * reach,
            ObjTarget::Occupy(s) => inside_site(o, s),
            ObjTarget::Radio {
                at, radius, height, ..
            } => {
                (at[0] - o[0]).hypot(at[1] - o[1]) < radius - RADIO_MARGIN
                    && (at[2] - o[2]).abs() < height - RADIO_MARGIN
            }
            ObjTarget::Pickup { aim, from } => {
                let reach = if from == aim { PICKUP_REACH } else { 16.0 };
                (from[0] - o[0]).hypot(from[1] - o[1]) < reach && (aim[2] - o[2]).abs() < 64.0
            }
            // The goal trigger's touch delivers; there is nothing to stand
            // at.
            ObjTarget::Deliver(_) | ObjTarget::Chase(_) => false,
            ObjTarget::Sweep(at) => arrived(o, *at),
            ObjTarget::Breach { aim, .. } => aim.is_some(),
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
            // A charge about to blow: away from it, the way out of its room.
            ObjTarget::Guard { at, inner }
                if inner >= BAS_BLAST && dist_sq(view.origin, at) < inner * inner =>
            {
                Goal::Away(at)
            }
            ObjTarget::Guard { at, inner } if dist_sq(view.origin, at) < inner * inner => {
                Goal::Roam
            }
            ObjTarget::Guard { at, .. } => Goal::To(at),
            ObjTarget::Defuse { bomb, .. } => Goal::To(bomb.origin),
            ObjTarget::Occupy(s) => Goal::To(s.stand),
            ObjTarget::Radio { stand, .. } => Goal::To(stand),
            ObjTarget::Breach {
                stand: Some(at), ..
            } => Goal::To(at),
            // Inside its own splash of the base: off, then back.
            ObjTarget::Breach { base, .. }
                if dist_sq(view.origin, base) < BREACH_MIN * BREACH_MIN =>
            {
                Goal::Away(base)
            }
            ObjTarget::Breach { base: at, .. }
            | ObjTarget::Pickup { from: at, .. }
            | ObjTarget::Deliver(at)
            | ObjTarget::Chase(at)
            | ObjTarget::Sweep(at) => Goal::To(at),
        })
    }

    /// The bot stands at its objective without acting on it: it holds its
    /// ground instead of following a waypoint, and is not stuck.
    fn objective_standing(&self, view: &BotView) -> bool {
        view.linked
            || (view.give_way.is_none()
                && self
                    .objective_target(view)
                    .is_some_and(|t| Self::at_objective(view, &t)))
    }

    /// In a contested zone (an enemy stands in it too, out of sight, and
    /// the capture is held) the bot walks to the zone's far side, and back
    /// once there, until the contest ends or it leaves the zone. On
    /// mp_backlot's L-shaped `koth` room an axis and an allies bot each
    /// held a leg of it for the rest of the match, the axis one in the
    /// door its teammates queued behind (bot-objectives.md 12.6).
    fn sweep_contested(&mut self, view: &BotView) {
        let Some(UoView::Dom(d)) = view.uo.as_ref() else {
            self.obj.sweep = None;
            return;
        };
        let Some(f) = d
            .flags
            .iter()
            .find(|f| f.contested && inside_site(view.origin, &f.site))
        else {
            self.obj.sweep = None;
            return;
        };
        if self.obj.sweep.is_some_and(|at| !arrived(view.origin, at)) {
            return;
        }
        let (lo, hi) = (f.site.mins, f.site.maxs);
        let o = view.origin;
        self.obj.sweep = Some([lo[0] + hi[0] - o[0], lo[1] + hi[1] - o[1], f.site.stand[2]]);
    }

    /// The plant or defuse itself: the cmd for this tick while one is under
    /// way or starting, `None` to play on as usual.
    fn think_objective(&mut self, view: &BotView, cmd: UserCmd) -> Option<UserCmd> {
        self.obj.rest = self.obj.rest.saturating_sub(1);
        let t = self.objective_target(view);
        if Self::press_swaps_guarded(view) && matches!(t, Some(ObjTarget::Defuse { .. })) {
            if self.obj.close_in > 0 {
                self.obj.close_in += 1;
            }
        } else {
            self.obj.close_in = 0;
        }
        let at = t.is_some_and(|t| Self::at_objective(view, &t));
        if let Some(re) = view.re.as_ref() {
            if matches!(t, Some(ObjTarget::Sweep(_))) && at {
                self.obj.swept = true;
            }
            if !re.objectives.iter().any(|m| m.carried) {
                self.obj.swept = false;
            }
        }
        // The domination flag picked stays the bot's until its team holds it.
        if let (Some(UoView::Dom(d)), Some(ObjTarget::Occupy(site))) = (view.uo.as_ref(), t) {
            self.obj.zone = d.flags.iter().position(|f| f.site.stand == site.stand);
        }
        self.sweep_contested(view);
        let ready = self.obj.rest == 0 && (at || (view.linked && self.obj.held > 0));
        self.fetching(view, t);
        let cur = [yaw_diff(view.view[0], 0.0), view.view[1]];
        match t {
            Some(ObjTarget::Plant(_)) if ready => {
                let plant_ticks =
                    (view.sd.as_ref().and_then(|sd| sd.key)).map_or(PLANT_TICKS, |k| k.plant_ticks);
                if self.obj.held >= plant_ticks || (self.obj.held >= START_TICKS && !view.linked) {
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
            Some(ObjTarget::Defuse { bomb: b, ticks, .. }) if ready => {
                if self.obj.held >= ticks || (self.obj.held >= START_TICKS && !view.linked) {
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
                let swaps = Self::press_swaps_guarded(view)
                    && view.use_takes_weapon
                    && self.obj.close_in < DEFUSE_CLOSE_TICKS;
                if self.obj.settled >= SETTLE_TICKS && swaps {
                    // A weapon lying by the bomb outscores the trigger
                    // from here: close in over the bomb.
                    self.obj.close_in = self.obj.close_in.max(1);
                    self.obj.settled = 0;
                    cmd.buttons = 0;
                } else if self.obj.settled >= SETTLE_TICKS {
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
                // The view's pitch stops at 80: one right underfoot settles
                // there.
                let tp = tp.clamp(-80.0, 80.0);
                let aim = self.turn_toward(view, [tp, ty]);
                let off = yaw_diff(tp, cur[0]).hypot(yaw_diff(ty, cur[1]));
                self.obj.settled = if off < SETTLE_DEG {
                    self.obj.settled + 1
                } else {
                    0
                };
                let mut cmd = self.hold_use(view, cmd, aim);
                // The primary it would rather lose in hand first: the swap
                // drops what is held, and a bot that tapped with its gun
                // out traded it.
                let swap = match view.uo.as_ref() {
                    Some(UoView::Bas(b)) => Self::swap_out(view, b.launchers.contains(&spot)),
                    _ => None,
                }
                .filter(|&w| w != view.weapon);
                if let Some(r) = swap {
                    cmd.weapon = r;
                    cmd.buttons = 0;
                } else if self.obj.settled < SETTLE_TICKS {
                    cmd.buttons = 0;
                } else {
                    // A tap: two ticks down, then up for a while. Held, the
                    // use key drops what the carrier holds (`re.gsc`
                    // `holduse`).
                    self.obj.held += 1;
                    if self.obj.held >= 2 {
                        if matches!(view.uo, Some(UoView::Bas(_))) {
                            self.tapped_launcher(spot);
                        }
                        self.obj.held = 0;
                        self.obj.settled = 0;
                        self.obj.rest = RETRY_TICKS;
                        self.obj.pick = self.obj.pick.wrapping_add(1);
                    }
                }
                Some(cmd)
            }
            Some(ObjTarget::Breach {
                stand: Some(st),
                aim: None,
                ..
            }) if arrived(view.origin, st) => {
                self.obj.idle += 1;
                if self.obj.idle >= BREACH_IDLE_TICKS {
                    self.obj.idle = 0;
                    self.obj.pick = self.obj.pick.wrapping_add(1);
                }
                None
            }
            Some(ObjTarget::Breach {
                aim: Some(spot), ..
            }) if at => {
                let rocket = view.rocket?;
                // A visible enemy comes first.
                if self.shoot && view.enemy.is_some() {
                    self.obj.settled = 0;
                    return None;
                }
                Some(self.breach(view, cmd, rocket, spot))
            }
            _ => {
                self.obj.held = 0;
                self.obj.settled = 0;
                self.obj.idle = 0;
                None
            }
        }
    }

    /// A `bas` launcher the body has come no nearer to in
    /// [`FETCH_STALL_TICKS`] is passed over: one lay where the graph's
    /// nearest node was a floor over it, and the bot walked into the wall
    /// for good; at another the use tap took nothing, and the bot stood
    /// there for twenty minutes.
    fn fetching(&mut self, view: &BotView, t: Option<ObjTarget>) {
        let Some(ObjTarget::Pickup { aim, .. }) =
            t.filter(|_| matches!(view.uo, Some(UoView::Bas(_))))
        else {
            self.fetch = None;
            return;
        };
        let d = dist_sq(view.origin, aim).sqrt();
        let (best, ticks) = match self.fetch {
            Some((p, best, ticks)) if p == aim && d > best - FETCH_CLOSING => (best, ticks + 1),
            _ => (d, 0),
        };
        if ticks >= FETCH_STALL_TICKS {
            self.shunned.push((aim, SHUN_TICKS));
            self.fetch = None;
        } else {
            self.fetch = Some((aim, best, ticks));
        }
    }

    /// The S&D defuse and the `bas` plant and defuse hold use off the
    /// press a weapon lying in reach would answer ([`DEFUSE_CLOSE`]).
    fn press_swaps_guarded(view: &BotView) -> bool {
        view.sd.is_some() || matches!(view.uo, Some(UoView::Bas(_)))
    }

    /// `far`, or [`DEFUSE_CLOSE`] while closing in over a charge whose
    /// press would take a weapon lying by it.
    fn press_reach(&self, far: f32) -> f32 {
        if (1..DEFUSE_CLOSE_TICKS).contains(&self.obj.close_in) {
            DEFUSE_CLOSE
        } else {
            far
        }
    }

    /// The primary a `bas` bot holds when it taps for a weapon lying about,
    /// a launcher with `launcher`: with a slot free nothing drops (`None`);
    /// else an empty one before a loaded one, then one of the kind being
    /// fetched, then not the gun it fights with. A launcher fetch with a
    /// spent launcher and a gun drops the launcher; a gun fetch with a
    /// loaded launcher and a dry gun drops the gun.
    fn swap_out(view: &BotView, launcher: bool) -> Option<u8> {
        let [a, b] = view.primaries;
        if a == 0 || b == 0 {
            return None;
        }
        let cost = |w: u8| {
            let bit = |m: u64| w < 64 && m >> w & 1 == 1;
            (
                bit(view.loaded),
                bit(view.launchers) != launcher,
                view.gun == Some(w),
                w != view.weapon,
            )
        };
        Some(if cost(b) < cost(a) { b } else { a })
    }

    /// A tap at a `bas` launcher: [`LAUNCHER_TAPS`] at one with no rocket
    /// in hand after passes it over for a while. The use pickup refused
    /// some for good (a bot stood tapping at one for 20 minutes).
    fn tapped_launcher(&mut self, at: [f32; 3]) {
        let taps = match self.tapped {
            Some((p, n)) if arrived(p, at) => n + 1,
            _ => 1,
        };
        self.tapped = Some((at, taps));
        if taps >= LAUNCHER_TAPS {
            self.tapped = None;
            self.shunned.push((at, SHUN_TICKS));
        }
    }

    /// One tick of a stand at a base: the launcher out, the view on `spot`,
    /// a round fired once the view has rested on it, a reload after.
    fn breach(&mut self, view: &BotView, mut cmd: UserCmd, rocket: u8, spot: [f32; 3]) -> UserCmd {
        self.disengage();
        self.restart_stall(view.origin);
        let (tp, ty) = aim_angles(view, spot);
        let aim = self.turn_toward(view, [tp, ty]);
        let cur = [yaw_diff(view.view[0], 0.0), view.view[1]];
        let off = yaw_diff(tp, cur[0]).hypot(yaw_diff(ty, cur[1]));
        self.obj.settled = if off < SETTLE_DEG {
            self.obj.settled + 1
        } else {
            0
        };
        cmd.angles = cmd_angles(view, aim);
        if view.weapon != rocket {
            self.switch_to = Some(rocket);
            cmd.weapon = rocket;
            return cmd;
        }
        self.switch_to = None;
        cmd.weapon = rocket;
        if view.busy_ms != 0 {
            return cmd;
        }
        // A rocket leaves along the spread aim (`Weapon_RocketLauncher_Fire`):
        // up to 12 degrees off from the hip, 4 sighted. From the hip a
        // breach rocket struck the stand's own walls, and so did one fired
        // sighted with the cone a turn opened on the way up still wide:
        // the stand's checks clear the sighted floor only.
        let sighted = (!view.has_ads || view.ads_frac >= 1.0) && view.spread_scale == 0.0;
        if view.clip == 0 {
            cmd.wbuttons |= msg::WBUTTON_RELOAD;
        } else if view.has_ads {
            cmd.buttons |= msg::BUTTON_ADS;
        }
        if view.clip != 0 && sighted && self.obj.settled >= SETTLE_TICKS && self.fire_cooldown == 0
        {
            cmd.buttons |= msg::BUTTON_ATTACK;
            // The bit comes up between rounds.
            self.fire_cooldown = 2;
        }
        cmd
    }

    /// Use down, no move keys, the view at `aim`. Nothing else runs: the
    /// stall watch restarts from here and the fight is dropped.
    fn hold_use(&mut self, view: &BotView, mut cmd: UserCmd, aim: [f32; 2]) -> UserCmd {
        self.disengage();
        self.restart_stall(view.origin);
        cmd.buttons = msg::BUTTON_USE;
        cmd.angles = cmd_angles(view, aim);
        cmd
    }

    /// No round left in any gun, launcher or pistol.
    fn dry(view: &BotView) -> bool {
        let spent = |w: u8| w == 0 || w >= 64 || view.loaded >> w & 1 == 0;
        view.gun.is_none()
            && view.rocket.is_none()
            && view.pistol.is_none_or(spent)
            && spent(view.weapon)
    }

    /// Shooting, only a rocket left (none goes at a player) and an enemy
    /// given up on ([`Self::count_futile`]): the bot fetches a gun lying
    /// near. Two such bots on mp_foy, seed 23, stood side by side in a
    /// charge room while one planted and the other defused, eight times
    /// over 5 min.
    fn unarmed(&self, view: &BotView) -> bool {
        let spent = |w: u8| w == 0 || w >= 64 || view.loaded >> w & 1 == 0;
        self.shoot
            && self.ignored.is_some()
            && view.rocket.is_some()
            && view.gun.is_none()
            && view.pistol.is_none_or(spent)
    }

    /// Held in the air, off the rungs and unlinked, without moving for
    /// [`WEDGE_TICKS`]: a crease that holds a body at rest under any key
    /// (bot-navigation.md section 3, "mp_hurtgen's crease"). A player
    /// there has `kill` left, and so does the bot, once per wait.
    /// Or [`BotView::stranded`] for [`STRAND_TICKS`]: the wander keeps off
    /// the fall, and the graph has no way on.
    fn wedged(&mut self, view: &BotView) -> bool {
        self.strand_ticks = if view.stranded {
            self.strand_ticks + 1
        } else {
            0
        };
        if self.strand_ticks >= STRAND_TICKS {
            self.strand_ticks = 0;
            return true;
        }
        if view.on_ground
            || view.on_ladder
            || view.linked
            || dist_sq(view.origin, self.wedge_at) > WEDGE_SLACK * WEDGE_SLACK
        {
            self.wedge_at = view.origin;
            self.wedge_ticks = 0;
            return false;
        }
        self.wedge_ticks += 1;
        if self.wedge_ticks < WEDGE_TICKS {
            return false;
        }
        self.wedge_ticks = 0;
        true
    }

    /// Whether the server should probe [`BotView::lob`] this tick: a throw
    /// may start, or one is in hand.
    pub(crate) fn wants_lob(&self, view: &BotView) -> bool {
        self.shoot
            && view.grenade.is_some()
            && self.throw_target(view).is_some()
            && (self.grenade_cooldown <= 1 || !matches!(self.stage, Stage::Wander))
    }

    /// What a throw goes at: a turned-away throw's spot `AWAY_REACH` off
    /// along its yaw; with a throw under way the enemy in sight, else
    /// where it was last seen; before one, [`Bot::throw_spot`]. A frag let
    /// go along the view once the enemy had stepped behind cover struck
    /// the cover and rolled back.
    pub(crate) fn throw_target(&self, view: &BotView) -> Option<[f32; 3]> {
        if let Some(yaw) = self.throw_away {
            let (s, c) = yaw.to_radians().sin_cos();
            let [x, y, z] = view.origin;
            return Some([x + c * AWAY_REACH, y + s * AWAY_REACH, z]);
        }
        if matches!(self.stage, Stage::Wander) {
            return self.throw_spot(view);
        }
        self.throw_enemy(view).map(|e| e.origin).or(self.throw_at)
    }

    /// The enemy in sight a throw under way follows: the one it started
    /// at, none for a lost enemy's spot. A frag held for one enemy that
    /// went at another who came into sight mid-hold left on the old view,
    /// off the new lob, and killed its thrower.
    fn throw_enemy(&self, view: &BotView) -> Option<EnemyView> {
        view.enemy.filter(|e| match self.throw_for {
            ThrowFor::Any => true,
            ThrowFor::Enemy(slot) => e.slot == slot,
            ThrowFor::Spot => false,
        })
    }

    /// Where a throw would start for, between [`GRENADE_MIN`] and
    /// [`GRENADE_RANGE`] off: an enemy in sight no round has gone at for
    /// [`COVER_THROW_TICKS`] (behind cover, or never lined up), else the
    /// spot a seen enemy was lost at within [`LOST_THROW_TICKS`], standing
    /// or slow ([`LOST_STILL`]). An enemy the gun can hit takes the gun: a
    /// frag at one in the open made 5 of 2908 kills (bot-combat.md 6.5).
    fn throw_spot(&self, view: &BotView) -> Option<[f32; 3]> {
        let at = match view.enemy {
            Some(e) => (self.futile >= COVER_THROW_TICKS).then_some(e.origin)?,
            None if self.seen_speed < LOST_STILL => {
                let r = self.recall.filter(|r| r.seen)?;
                let lost = self.skill.memory_ticks.saturating_sub(r.ticks);
                (LOST_THROW_MIN..=LOST_THROW_TICKS)
                    .contains(&lost)
                    .then_some(r.at)?
            }
            None => return None,
        };
        let d = dist_sq(view.origin, at);
        (d > GRENADE_MIN * GRENADE_MIN && d < GRENADE_RANGE * GRENADE_RANGE).then_some(at)
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
        self.cover.end_life();
    }

    /// The remembered spot not yet reached, else this tick's noise. The same
    /// rule [`Bot::remember`] applies a tick later, so the two agree.
    fn recall_goal(&self, view: &BotView) -> Option<[f32; 3]> {
        self.hunt_goal(view)
            .or_else(|| view.noise.filter(|n| !arrived(view.origin, *n)))
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
        let hunted = self.hunt_arrived(view);
        self.hunts += u32::from(hunted);
        if hunted
            || self
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
        self.track_reach(view);
        // Read before the think moves the objective's state, as the goal was.
        self.on_objective = self.objective_busy(view)
            || self
                .objective_target(view)
                .is_some_and(|t| Self::at_objective(view, &t));
        let cmd = self.think_body(view);
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

    /// The bot eases to a walk behind a teammate just ahead
    /// ([`BotView::mate_ahead`]): out of a fight and a retreat, not
    /// carrying, not with a bomb down (a defuse or its guard is a race).
    fn spaced(&self, view: &BotView) -> bool {
        view.mate_ahead
            && self.team_play
            && self.squad
            && view.enemy.is_none()
            && self.retreat.is_none()
            && !self.carrying(view)
            && view.sd.as_ref().is_none_or(|sd| sd.bomb.is_none())
    }

    /// An enemy in sight whom no round has gone at for [`FUTILE_TICKS`] is
    /// given up on for [`IGNORE_TICKS`]: a UO bot with only an LMG it
    /// could not deploy on a stair, and one with a dry pistol and rockets
    /// (none go at players), stood facing each other for twenty minutes.
    fn count_futile(&mut self, view: &BotView, cmd: &UserCmd) {
        if let Some((slot, left)) = self.ignored {
            self.ignored = (left > 1).then_some((slot, left - 1));
        }
        match view.enemy {
            Some(e) if cmd.buttons & msg::BUTTON_ATTACK == 0 => {
                self.futile += 1;
                if self.futile >= FUTILE_TICKS {
                    self.futile = 0;
                    self.ignored = Some((e.slot, IGNORE_TICKS));
                }
            }
            _ => self.futile = 0,
        }
    }

    /// Whether the server leaves the enemy in `slot` out of this bot's
    /// view ([`Bot::count_futile`]).
    pub fn ignores(&self, slot: usize) -> bool {
        self.ignored.is_some_and(|(s, _)| s == slot)
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
            // Spectator or intermission camera: nothing to press.
            self.respawn_ticks = 0;
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
        self.remember(view);
        self.quiet_ticks = match view.enemy {
            Some(e) => {
                self.seen_speed = e.velocity[0].hypot(e.velocity[1]);
                0
            }
            None => self.quiet_ticks.saturating_add(1),
        };
        if let Some(bel) = view.bel.as_ref()
            && let Some(m) = self.next_marker(view, bel)
            && arrived(view.origin, m)
        {
            self.marker_reached = Some(m);
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

    /// A live grenade near: the keys run from it whatever the fight wants,
    /// the view stays where `yaw` (the cmd's own) has it.
    fn flee(&self, view: &BotView, cmd: &mut UserCmd, yaw: f32) {
        if let Some(f) = view.frag_near
            && !view.deployed
            && !self.objective_busy(view)
        {
            let away = (view.origin[1] - f[1])
                .atan2(view.origin[0] - f[0])
                .to_degrees();
            let d = yaw_diff(away, yaw).to_radians();
            cmd.forward = (d.cos() * 127.0).round() as i8;
            cmd.right = (-d.sin() * 127.0).round() as i8;
            cmd.up = 0;
            cmd.wbuttons &= !(msg::WBUTTON_CROUCH
                | msg::WBUTTON_PRONE
                | msg::WBUTTON_LEAN_LEFT
                | msg::WBUTTON_LEAN_RIGHT);
        }
    }

    fn think_wander(&mut self, view: &BotView, mut cmd: UserCmd) -> UserCmd {
        if let Some(cmd) = self.think_objective(view, cmd) {
            return cmd;
        }
        // A frag goes at an enemy the gun cannot reach ([`Bot::throw_spot`])
        // once the cooldown is spent and the server found a clear lob with
        // no teammate by the landing, from a body at rest: one that started
        // on the run slid on through the switch to where a wall over the
        // eye stopped every lob, and the hold let go into it. The switch
        // itself is the stage machine below.
        self.grenade_cooldown = self.grenade_cooldown.saturating_sub(1);
        if self.shoot
            && self.grenade_cooldown == 0
            && self.switch_to.is_none()
            && !view.deployed
            && view.speed < THROW_SPEED
            && !view.on_ladder
            && view.eye_settled
            && view.lob.is_some()
            && let (Some(g), Some(at)) = (view.grenade, self.throw_spot(view))
        {
            self.grenade_cooldown = GRENADE_COOLDOWN_TICKS;
            self.rifle = view.weapon;
            self.stage = Stage::ToGrenade;
            // The start tick aims as the stages do: a cmd left at null
            // angles turned the view onto `delta_angles`, off the enemy,
            // and a target never recorded left the release no lob
            // (bot-objectives.md 6).
            self.throw_at = Some(at);
            self.throw_for = view
                .enemy
                .map_or(ThrowFor::Spot, |e| ThrowFor::Enemy(e.slot));
            cmd.angles = self.lob_angles(view);
            cmd.weapon = g;
            return cmd;
        }

        // A body that has not left a 15-unit circle in ten ticks (2 units
        // creeping) is stuck, path or not, and takes a random heading for a spell. One standing
        // at its objective, or held by a link, means to stay put.
        self.lmg_rest = self.lmg_rest.saturating_sub(1);
        self.lmg_linger = self.lmg_linger.saturating_sub(1);
        // A bipod down holds the body; it means to stay put.
        let standing = self.objective_standing(view)
            || view.deployed
            || self.holding_cover(view)
            || self.start_held(view);
        self.stall_ticks += 1;
        self.stall_reach = self
            .stall_reach
            .max(dist_sq(view.origin, self.stall_origin));
        self.rungs_ago = if view.on_ladder {
            0
        } else {
            self.rungs_ago.saturating_add(1)
        };
        // Coming to rest at a leap's foot is not being stuck either.
        if standing || view.stop {
            self.restart_stall(view.origin);
            self.unstick_ticks = 0;
        } else if view.unpin.is_some() && self.unstick_ticks == 0 {
            // A pin with a heading out of it gets its run aside first.
            self.restart_stall(view.origin);
        } else if self.stall_ticks >= STALL_TICKS {
            // A creep covers about 8 units in ten ticks and is stuck only
            // when it stops: a back down over a thin ladder's top creeps 20
            // and more (mp_depot's at (-2750, -1217), section 3).
            let radius = if self.creep.creeping() {
                2.0
            } else {
                STALL_RADIUS
            };
            // Left the circle and came back is no stall: a body that flew
            // past a node down mp_ship's crow's nest spar walked back up to
            // it and on, and the spell's heading ran it off the spar's side.
            if self.stall_reach < radius * radius
                && !self.closes_in(view, self.stall_origin)
                && !self.topping(view)
            {
                // Pinned mid-spell: another heading, but the spell still
                // ends on time. Chained spells never handed the body back
                // to its waypoint (mp_ship's hold ladder head).
                if self.unstick_ticks == 0 {
                    // Stuck again where the last spell started: a pocket a
                    // drop is the only way out of (mp_ship's rim at (4223,
                    // 345, 276)).
                    let again = dist_sq(view.origin, self.unstick_at) < CORNER * CORNER;
                    self.unstick_tries = if again { self.unstick_tries + 1 } else { 0 };
                    self.unstick_at = view.origin;
                    self.unstick_ticks = UNSTICK_TICKS;
                    self.rung_spell = self.rungs_ago <= RUNG_GRACE;
                    self.stuck = true;
                    // A creep's held heading belongs to where it began.
                    self.creep = Default::default();
                }
                self.pick_heading(view);
            } else {
                self.restart_stall(view.origin);
            }
        }
        let spell = self.unstick_ticks > 0;
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
            // view of a wander climbs at under two thirds of the rate, which
            // held bots under a spar on mp_ship's ladders for good.
            _ if view.on_ladder => {
                self.unstick_ticks = self.unstick_ticks.saturating_sub(1);
                (crate::nav::LADDER_PITCH, self.heading, 127)
            }
            // Just off the rungs, a spell or a wait for a plan holds still:
            // a climb that bobs under a ladder's top lets go and grabs again,
            // and a heading taken there, on the head's plate or in the air
            // over it, ran bots off mp_ship's crow's nest plate down its
            // hatch (bot-navigation.md section 3).
            _ if (self.rung_spell && self.unstick_ticks > 0)
                || (self.unstick_ticks == 0 && !view.on_ground && self.rungs_ago <= RUNG_GRACE) =>
            {
                self.unstick_ticks = self.unstick_ticks.saturating_sub(1);
                (0.0, self.heading, 0)
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
        // A spell's end starts the stall window afresh: one straddling the
        // turn back to the waypoint reads the way out and the way back as
        // standing still (mp_ship's ladder at x 3792, backed down a spell
        // at a time).
        if spell && self.unstick_ticks == 0 {
            self.restart_stall(view.origin);
        }
        // On level ground with a remembered spot in range, the view turns
        // toward it while the keys keep the path, so a returning enemy is
        // already near the crosshair. The reaction delay is `track`'s
        // business either way.
        // A UO bot with the breath to sprint a straight that ends short of
        // the spot runs it looking along the path: turned to the spot, the
        // keys leave the line and the sprint never starts, and in a game of
        // running fights that left 64 sprinted leg-ticks in 90 s (seed 19).
        let sprints_on = |r: &Recall| {
            view.fatigue
                .is_some_and(|f| view.sprinting || f >= SPRINT_FRESH)
                && view.sprint_room >= SPRINT_ROOM
                && dist_sq(view.origin, r.at) > view.sprint_room * view.sprint_room
        };
        // Not on a mantle edge (bot-combat.md 17.1).
        let mut recall_look = false;
        if self.shoot
            && view.enemy.is_none()
            && !view.mantle
            && pitch == 0.0
            && forward == 127
            && let Some(r) = self.recall
            && dist_sq(view.origin, r.at) < SHOOT_RANGE * SHOOT_RANGE
            && !sprints_on(&r)
        {
            let (tp, ty) = aim_angles(view, r.at);
            let look = self.turn_toward(view, [tp, ty]);
            // The path heading relative to the view, counter-clockwise.
            let d = yaw_diff(yaw, look[1]).to_radians();
            forward = (d.cos() * 127.0).round() as i8;
            cmd.right = (-d.sin() * 127.0).round() as i8;
            [pitch, yaw] = look;
            recall_look = true;
        }
        // Slowing onto a leap's foot, then standing on it. Under a recall
        // look the keys are turned off the view, so both scale: a forward
        // key set along the view walked mp_carentan's bots away from their
        // stop into a corner (bot-combat.md 17.1).
        if let Some(w) = foot {
            let flat = (w[0] - view.origin[0]).hypot(w[1] - view.origin[1]);
            let slow = if flat < crate::nav::LEAP_FOOT * 0.5 {
                0.0
            } else {
                (flat / LEAP_SLOW).min(1.0)
            };
            if recall_look {
                forward = (f32::from(forward) * slow) as i8;
                cmd.right = (f32::from(cmd.right) * slow) as i8;
            } else {
                forward = if slow == 0.0 {
                    0
                } else {
                    (flat / LEAP_SLOW * 127.0).min(127.0) as i8
                };
            }
        }
        // Engaging overrides all of this: `footwork` owns the move keys and
        // the aim owns the view.
        cmd.forward = forward;
        if steering
            && view.jump
            && let Some(w) = view.waypoint
        {
            let flat = (w[0] - view.origin[0]).hypot(w[1] - view.origin[1]);
            let rise = w[2] - view.origin[2];
            let cue = if view.leap {
                crate::nav::leap_cue(view.lip, flat, rise)
            } else {
                // The walk that proved the edge held its key at the top
                // every tick: a tick counts as pinned only with the key held
                // at this waypoint, closing on it under the pinned pace (the
                // walk, straight at the top, closes as far as it moves), and
                // not in the air off a jump. A body set off from rest on a
                // foot has not yet held its key (`nav::Follower::jump_wait`).
                // No room test: the edge was proved again by this rule
                // (`nav::NavGraph::reprove_jumps`). Read off the distance
                // moved instead, a bot sliding along the rail over mp_ship's
                // stair drop at (5402, 321, 344) jumped late, past it, into
                // the crease at (5445, 355, 292) (bot-navigation.md, "The
                // jump cue and the re-proof"). On the ground the body has to
                // be slow too, as the walk measures it: one that struck the
                // slanted face of mp_ship's crate at (6207, -74, 80) and slid
                // along it at 2.6 a tick jumped where the walk slid on to the
                // corner, and came down on the crate beside it (seed 661);
                // read so off the ground as well, bots sliding on the hull's
                // curve at (2200, -300, -45) never jumped and stuck in its
                // crease (bot-navigation.md section 3, "The recheck's
                // leftovers").
                let held = self.pressed == Some(w);
                if !held {
                    self.pin = crate::nav::PinJump::default();
                }
                self.pin_air &= !(view.on_ground || view.on_ladder);
                let armed = !crate::nav::on_jump_top(flat, rise);
                let climbed = self.pin_from.map_or(0.0, |p| view.origin[2] - p[2]);
                let under = crate::nav::jump_under(flat, rise, climbed);
                let to = glam::Vec2::new(w[0] - view.origin[0], w[1] - view.origin[1]);
                let v = glam::Vec2::new(view.velocity[0], view.velocity[1]);
                let closing = v.dot(to.normalize_or_zero()) * (FRAME_MS as f32 / 1000.0);
                let moved = self.pin_from.map_or(f32::INFINITY, |p| {
                    glam::Vec3::from(p).distance(glam::Vec3::from(view.origin))
                });
                let slow = closing < crate::nav::PINNED
                    && !(view.on_ground && moved >= crate::nav::PINNED);
                let stalled = held && slow && !self.pin_air;
                self.pin.before(armed, view.on_ground, under, || true)
                    || self
                        .pin
                        .after(armed, stalled, view.on_ground, view.sliding, true, || true)
                        == crate::nav::PinTick::Jump
            };
            cmd.up = crate::nav::jump_key(view.on_ground, cue, self.jump_held);
            self.pin_air |= cmd.up > 0;
        } else {
            self.pin = crate::nav::PinJump::default();
            self.pin_air = false;
        }
        self.pin_from = Some(view.origin);
        // A crouched jump edge: `PM_CheckJump` refuses a crouched body, so
        // the crouch starts on the cmd after the jump's and is held, as a
        // retail client holds it, until the body is on the ground again.
        self.crouch_air &= !view.on_ground;
        if self.crouch_air {
            cmd.wbuttons |= msg::WBUTTON_CROUCH;
            cmd.up = -127;
        }
        self.crouch_air |= view.crouch_jump && cmd.up > 0;
        // Off the rungs backwards onto a floor behind: jump facing the
        // rungs, which reflects the jump out, then brake facing the same
        // way until the body lands, as `nav::push_off` proved it.
        if view.on_ground || (view.on_ladder && view.push_off.is_none()) {
            self.pushed_off = None;
        }
        if let Some(y) = view.push_off.filter(|_| steering) {
            (pitch, yaw, forward) = (0.0, y, 0);
            cmd.right = 0;
            cmd.up = if self.jump_held { 0 } else { 127 };
            self.pushed_off = Some(y);
        } else if let Some(y) = self.pushed_off {
            (pitch, yaw, forward) = (0.0, y, 127);
            cmd.right = 0;
        }
        // Pinned in a crease: run aside for a few ticks, then at the
        // waypoint again with the pace that slides through.
        if let Some(y) = view.unpin.filter(|_| steering) {
            self.unpin = Some((y, crate::nav::UNPIN_TICKS));
        }
        match self.unpin.as_mut() {
            Some((y, n)) if *n > 0 && steering && view.on_ground => {
                (pitch, yaw, forward) = (0.0, *y, 127);
                cmd.right = 0;
                *n -= 1;
            }
            _ => self.unpin = None,
        }
        // A plain run on the ground: the view comes round to the path's
        // heading at `walk_turn_deg` a tick and the keys keep the body on
        // the heading meanwhile. A climb, a back down, a jump, a run off a
        // drop and a leap's or a creep's foot keep the view the walk that
        // proved them had (bot-combat.md section 11). Handed back from a
        // view something else owned, the view takes the heading at once:
        // brought round from the threat, a bot leaving its cover saw the
        // same enemy again and dug back in (11.4).
        if steering
            && view.on_ground
            && !view.on_ladder
            && pitch == 0.0
            && forward == 127
            && cmd.right == 0
            && cmd.up == 0
            && foot.is_none()
            && !view.stop
            && !view.jump
            && !view.run_off
            && self.pushed_off.is_none()
            && self.unpin.is_none()
            && !self.creep.creeping()
            && !self.view_owned
        {
            (forward, cmd.right, yaw) = self.walk_turn(view, yaw);
        }
        let walked = [pitch, yaw];
        // On the ground the keys are cut to the graph's pace: its walks ran
        // at 190 units/s, and a light weapon's 215 to 228 flies a run off
        // mp_ship's mast platform at (3564, -127, 896) over the narrow ledge
        // at 760 it lands on (bot-navigation.md section 3). The air and
        // ladder moves take no weapon scale, and a jump's tick moves in
        // the air.
        // On UO a long straight leg is sprinted while there is breath for
        // it; the sprint leaves the weapon's scale out (coduo-movement.md
        // 2-4, bot-navigation.md "Other rules").
        // A plain run behind a teammate just ahead eases to a walk
        // (`Bot::spaced`): a team in single file opens out to a blast's
        // width apart instead of falling to one burst or frag.
        let ease = self.spaced(view)
            && steering
            && view.on_ground
            && !view.on_ladder
            && cmd.up == 0
            && foot.is_none()
            && !view.stop
            && !view.jump
            && !view.run_off
            && !view.mantle
            && self.pushed_off.is_none()
            && self.unpin.is_none()
            && !self.creep.creeping();
        self.sprint_leg = !ease
            && view.fatigue.is_some()
            && steering
            && forward == 127
            && cmd.right == 0
            && cmd.up == 0
            && foot.is_none()
            && !view.stop
            && !view.jump
            && !view.run_off
            && self.pushed_off.is_none()
            && self.unpin.is_none()
            && view.enemy.is_none()
            && view.on_ground
            && !view.on_ladder
            && view.sprint_room
                >= if view.sprinting {
                    SPRINT_TAIL
                } else {
                    SPRINT_ROOM
                };
        let sprint = self.sprint_leg
            && view
                .fatigue
                .is_some_and(|f| view.sprinting || f >= SPRINT_FRESH);
        if sprint {
            cmd.buttons |= msg::BUTTON_SPRINT_V22;
        } else if view.on_ground && !view.on_ladder && cmd.up == 0 {
            forward = crate::nav::paced_key(forward, view.move_scale);
            cmd.right = crate::nav::paced_key(cmd.right, view.move_scale);
        }
        if ease {
            forward = (f32::from(forward) * EASE_KEY) as i8;
            cmd.right = (f32::from(cmd.right) * EASE_KEY) as i8;
        }
        cmd.forward = forward;
        self.jump_held = cmd.up > 0;
        self.pressed = view.waypoint.filter(|_| forward != 0);
        // A `bas` charge about to go: the keys keep running the graph's way
        // out whatever the fight wants, and no bipod goes down. A planter
        // and a defender fighting in the charge room both stood in the
        // blast to the end (seed 5). A retrieval carrier runs on the same
        // way (`team`'s `carrying`).
        let clearing = Self::blast_near(view).is_some() || self.carrying(view);
        let path = ((cmd.forward, cmd.right, cmd.up, cmd.wbuttons), yaw);
        if self.shoot {
            self.track(view);
            let keys = (cmd.forward, cmd.right, cmd.up, cmd.wbuttons);
            if !clearing && let Some(aim) = self.think_armor(view, &mut cmd, yaw) {
                [pitch, yaw] = aim;
            } else if let Some(e) = view.enemy
                && !(clearing && view.lmg)
                && let Some(aim) = self.engage(view, e, &mut cmd)
            {
                if clearing {
                    (cmd.forward, cmd.right) = rekey(keys.0, keys.1, yaw, aim[1]);
                    (cmd.up, cmd.wbuttons) = (keys.2, keys.3);
                    cmd.buttons &= !msg::BUTTON_ADS;
                }
                [pitch, yaw] = aim;
            } else if view.lmg && view.deployed && self.lmg_linger > 0 && !clearing {
                // The bipod stays down a while for the target to show again.
                cmd.buttons |= msg::BUTTON_ADS;
                cmd.forward = 0;
                cmd.right = 0;
                [pitch, yaw] = [yaw_diff(view.view[0], 0.0), view.view[1]];
            } else if view.enemy.is_none()
                && !clearing
                && let Some(look) = self.face_hurt(view, &mut cmd, yaw)
            {
                [pitch, yaw] = look;
            } else if view.enemy.is_none()
                && !clearing
                && let Some(look) = self.cover_unseen(view, &mut cmd)
            {
                [pitch, yaw] = look;
            }
        }
        [pitch, yaw] = self.retreat_keys(view, &mut cmd, path.0, path.1, [pitch, yaw]);
        self.flee(view, &mut cmd, yaw);
        // A switch ends when it lands, or when the weapon is gone (dropped,
        // or swapped at a pickup) and the byte would read as a holster for
        // good.
        if let Some(w) = self.switch_to
            && (view.weapon == w || view.weapons_held >> w & 1 == 0)
        {
            self.switch_to = None;
        }
        if self.shoot {
            self.off_launcher(view);
            self.off_dry(view);
            self.sidearm(view);
        }
        if let Some(w) = self.switch_to {
            cmd.weapon = w;
        }
        // A dry clip reloads, and so does one under half once no enemy has
        // been in sight for [`QUIET_RELOAD_TICKS`] (not the pistol, which
        // goes back by then), unless a switch is putting it away or there
        // is nothing to load; the tap is suppressed while the machine is
        // busy so a held bit cannot restart the reload it started.
        let tactical = self.quiet_ticks >= QUIET_RELOAD_TICKS
            && view.clip > 0
            && 2 * view.clip < view.survive.clip_size
            && view.pistol != Some(view.weapon)
            && view.grenade != Some(view.weapon);
        if (view.clip == 0 || tactical)
            && view.busy_ms == 0
            && self.switch_to.is_none()
            && view.loaded >> view.weapon.min(63) & 1 == 1
        {
            cmd.wbuttons |= msg::WBUTTON_RELOAD;
        }
        self.retreat_update(view);
        self.walking = steering && view.on_ground && !view.on_ladder && !self.engaged();
        self.view_owned = recall_look || [pitch, yaw] != walked;
        cmd.angles = cmd_angles(view, [pitch, yaw]);
        cmd
    }

    /// A held rocket launcher fires at no player (`trigger`): rockets are
    /// for the bases and for vehicles (`Bot::think_armor`). With a 320-unit floor on the range, a rocket at a
    /// moving body still burst on cover by the shooter often enough that a
    /// bot killed itself in most four-bot rounds (bot-objectives.md 7.2).
    fn launcher_in_hand(view: &BotView) -> bool {
        view.rocket.is_some_and(|r| r == view.weapon)
    }

    /// The gun, else the pistol, comes out over a held rocket launcher
    /// with an enemy in sight: rockets are for the bases and vehicles,
    /// and one at an enemy is a splash on the shooter as often as not. The
    /// breach and a vehicle draw the launcher again.
    fn off_launcher(&mut self, view: &BotView) {
        if view.enemy.is_none()
            || self.rider.at_armor
            || view.rocket.is_none_or(|r| r != view.weapon)
        {
            return;
        }
        let loaded = |w: u8| w < 64 && view.loaded >> w & 1 == 1;
        let to = view.gun.or(view.pistol.filter(|p| loaded(*p)));
        if let Some(to) = to
            && self.switch_to.is_none()
        {
            self.primary = view.weapon;
            self.calm_ticks = 0;
            self.switch_to = Some(to);
        }
    }

    /// The held weapon and the pistol both spent (a launcher fired dry):
    /// the gun comes out, or the bot stands with nothing to fire.
    fn off_dry(&mut self, view: &BotView) {
        let loaded = |w: u8| w != 0 && w < 64 && view.loaded >> w & 1 == 1;
        if self.switch_to.is_some()
            || view.weapon == 0
            || loaded(view.weapon)
            || view.pistol.is_some_and(loaded)
            || view.grenade == Some(view.weapon)
        {
            return;
        }
        if let Some(g) = view.gun {
            self.switch_to = Some(g);
        }
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
            // An LMG can be put away only with the bipod up, and its deploy
            // takes as long as a fight at pistol range.
            let bipod_off = view.lmg && !view.deployed && (close || self.lmg_rest > 0);
            if (view.clip == 0 && close && quicker)
                || !loaded(view.weapon)
                || (bipod_off && view.enemy.is_some())
            {
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
        let back = Some(self.primary).filter(|w| loaded(*w)).or(view.gun);
        if let Some(back) = back
            && (self.calm_ticks >= HOLSTER_TICKS || !loaded(pistol))
        {
            self.switch_to = Some(back);
        }
    }

    /// The body has closed on its waypoint by [`CLOSING`] since `from`,
    /// across plus up or down: the measure the follower's own give-up runs
    /// on (`nav::Follower::waypoint`). A slow way on is no stall. On
    /// mp_ship's crow's nest a bot backing for the deck slid along the
    /// hatch's rim at under a unit a tick, one climbing bobbed under the
    /// head's lip, one turned onto the head off its exit; each read as
    /// stuck, and the spell ran it down the hatch past the rungs
    /// (bot-navigation.md section 3).
    fn closes_in(&self, view: &BotView, from: [f32; 3]) -> bool {
        let Some(w) = view.waypoint else {
            return false;
        };
        let left = |p: [f32; 3]| (w[0] - p[0]).hypot(w[1] - p[1]) + (w[2] - p[2]).abs();
        left(from) - left(view.origin) >= CLOSING
    }

    /// On or just off the rungs right under a waypoint over them: a climb
    /// that bobs under the ladder's top, let go 8 under it, falling back
    /// and grabbing again until a step-up clears the lip. It steps on in
    /// about ten ticks; the stall circle read it as stuck and the spell
    /// climbed down, again and again under mp_ship's crow's nest plate
    /// (bot-navigation.md section 3).
    fn topping(&self, view: &BotView) -> bool {
        let Some(w) = view.waypoint else {
            return false;
        };
        let rise = w[2] - view.origin[2];
        self.rungs_ago <= RUNG_GRACE
            && rise > 0.0
            && rise < vcod_common::pmove::JUMP_HEIGHT
            && (w[0] - view.origin[0]).hypot(w[1] - view.origin[1]) < TOPPING_REACH
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
        // A head a step below on a lip just across the face, within a body's
        // half-width of the rungs, is climbed over: backing off the rungs
        // there pushed the body off the face and short of mp_ship's hold
        // ladder lip at (4332, -87, 615). Farther across, the back off
        // stays: two bots side by side over the top of the ladder at
        // x 3792 pinned each other there. Rungs that let go only past their
        // top (`BotView::tops_out`) are climbed on for any head across:
        // backed down once level with the floor 77 across mp_matmata's
        // ladder at (5025, 6749), a CoD2 bot went up and down to the end
        // (bot-objectives.md 11.4).
        let n = view.ladder_normal;
        let across = (to[0] - from[0]) * n[0] + (to[1] - from[1]) * n[1];
        let over_top = dz > -vcod_common::pmove::STEPSIZE
            && (across < -1.0 && view.tops_out
                || (-vcod_common::pmove::HALF_WIDTH..-1.0).contains(&across));
        // A jump edge was proved by a forward run whatever its drop: backed
        // off mp_ship's stair at (5402, 321, 344) for the floor 140 under
        // it, a bot jumped backwards onto the stair and slid down it
        // (seed 1275, bot-navigation.md section 3).
        let backs = dz < -BACK_DOWN_HEIGHT && !view.run_off && !view.jump;
        if backs || (view.on_ladder && dz < 0.0 && !over_top) {
            let flat = (to[0] - from[0]).hypot(to[1] - from[1]);
            let forward = crate::nav::back_move(flat, view.on_ladder);
            // Off the head of a ladder whose fall kills, the creep faces
            // the rungs and strafes to their middle: backed toward the
            // foot from a few units off the line, bots slid past mp_ship's
            // crow's nest rungs down the hatch (bot-navigation.md section 3).
            if !view.on_ladder
                && forward == -crate::nav::CREEP
                && let Some((face, middle)) = view.back_face
            {
                let yaw = self.creep.hold(forward, face);
                let right = along_face(yaw, from, middle).clamp(forward, -forward);
                return (-crate::nav::LADDER_PITCH, yaw, forward, right);
            }
            let yaw = self.creep.hold(forward, yaw_diff(yaw + 180.0, 0.0));
            // On the rungs only the strafe moves a body along the face, and
            // only facing it: square into it, sliding toward the waypoint,
            // or a floor beside the ladder is passed under its lip
            // (mp_ship's hold ladder at z 304).
            if view.on_ladder
                && let Some(yaw) = crate::nav::climb_yaw(view.ladder_normal.into())
            {
                let right = along_face(yaw, from, [to[0], to[1]]);
                return (-crate::nav::LADDER_PITCH, yaw, forward, right);
            }
            return (-crate::nav::LADDER_PITCH, yaw, forward, 0);
        }
        self.creep.hold(127, yaw);
        if view.on_ladder && dz > vcod_common::pmove::STEPSIZE {
            // Square into the face, strafing toward the climb's line, until
            // level with the waypoint: a ledge beside the rungs is stepped
            // onto facing it.
            let yaw = crate::nav::climb_yaw(view.ladder_normal.into()).unwrap_or(yaw);
            let right = view
                .ladder_middle
                .filter(|_| dz > STRAFE_STOP)
                .map_or(0, |m| along_face(yaw, from, m));
            // Released while the body still slides the other way: under a
            // full-rate climb the strafe's accelerate adds nothing to a pace
            // the grab carried in, and only a released key bleeds it
            // (`pmove::ladder_move`). Held, a climber that grabbed mp_depot's
            // ladder at (-417, 1112) running west slid off its edge.
            let (s, c) = yaw.to_radians().sin_cos();
            let side = view.velocity[0] * s - view.velocity[1] * c;
            let right = if side * f32::from(right) < 0.0 && side.abs() > 1.0 {
                0
            } else {
                right
            };
            (-crate::nav::LADDER_PITCH, yaw, 127, right)
        } else if dz > CLIMB_HEIGHT || view.on_ladder {
            (-crate::nav::LADDER_PITCH, yaw, 127, 0)
        } else {
            (0.0, yaw, 127, 0)
        }
    }

    /// Keeps [`Bot::target`] in step with the server's enemy: a new slot is
    /// a new target, one out of sight for `forget_ticks` is dropped. A new
    /// target starts with the full reaction delay and aim error, less for
    /// one the bot let go of within `forget_ticks` (it keeps most of the
    /// error it had) or one who just hit it.
    fn track(&mut self, view: &BotView) {
        if let Some(p) = self.prior.as_mut() {
            p.2 += 1;
            if p.2 > self.skill.forget_ticks {
                self.prior = None;
            }
        }
        let Some(e) = view.enemy else {
            // Out of sight behind its own cover the bot still knows where
            // the target is.
            let covering = self.covering();
            if let Some(t) = self.target.as_mut().filter(|_| !covering) {
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
        let full = self.skill.reaction_min + self.rand_below(self.skill.reaction_spread);
        let again = self.prior.filter(|p| p.0 == e.slot && self.aware);
        let (react, error) = if let Some((_, error, _)) = again {
            (
                full / 3,
                (error * REACQUIRE_ERROR).clamp(self.skill.error_floor, self.skill.error_start),
            )
        } else if view.hurt.is_some_and(|h| h.slot == e.slot) && self.aware {
            (full / 2, self.skill.error_start * HIT_BY_ERROR)
        } else {
            (full, self.skill.error_start)
        };
        if again.is_none() {
            self.want_yell(yell::Yell::Spotted);
        }
        if let Some(t) = self.target.take() {
            self.prior = Some((t.slot, t.error, 0));
        }
        self.burst_ticks = 0;
        self.target = Some(Target {
            slot: e.slot,
            react,
            error,
            dir: [0.0, 1.0],
            wobble: 0,
            lost: 0,
        });
    }

    /// Drops the target, remembered for a while as [`Bot::prior`].
    fn disengage(&mut self) {
        if let Some(t) = self.target.take() {
            self.prior = Some((t.slot, t.error, 0));
        }
        self.burst_ticks = 0;
        self.crouch_ticks = 0;
        self.prone_ticks = 0;
    }

    /// Hit by an enemy not in sight: the view turns toward where the hit
    /// came from, at the bot's turn rate, for [`HURT_LOOK_MS`], so the
    /// sight cone sweeps over him. The path's keys follow the turn. Not on
    /// a mantle edge, whose climb wants the view on the wall.
    fn face_hurt(&self, view: &BotView, cmd: &mut UserCmd, yaw: f32) -> Option<[f32; 2]> {
        let h = view
            .hurt
            .filter(|h| h.ago_ms < HURT_LOOK_MS && self.aware && !view.mantle)?;
        let [x, y, z] = h.dir;
        if x == 0.0 && y == 0.0 {
            return None;
        }
        // Back along the direction the damage travelled.
        let want = [
            (z.atan2(x.hypot(y)).to_degrees()).clamp(-45.0, 45.0),
            (-y).atan2(-x).to_degrees(),
        ];
        let look = self.turn_toward(view, want);
        (cmd.forward, cmd.right) = rekey(cmd.forward, cmd.right, yaw, look[1]);
        Some(look)
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
        let moving =
            1.0 + (self.skill.error_moving - 1.0) * (view.speed / RUN_SPEED).min(1.0).powi(2);
        // The target's speed across the line: the aim trails it.
        let to = [0, 1, 2].map(|i| (e.origin[i] - view.origin[i]) / dist);
        let along: f32 = (0..3).map(|i| e.velocity[i] * to[i]).sum();
        let across = (0..3)
            .map(|i| (e.velocity[i] - along * to[i]).powi(2))
            .sum::<f32>()
            .sqrt();
        let miss = (t.error + across * self.skill.track_lag)
            * (1.0 + dist / self.skill.error_range)
            * moving;
        let err_deg = (miss / dist).atan().to_degrees();
        let dir = t.dir;
        self.target = Some(t);

        let (tp, ty) = aim_angles(view, e.origin);
        // The view goes where it puts the gun on the line.
        let g = view.gun_offset;
        let aim = self.turn_toward(
            view,
            [tp - g[0] + dir[0] * err_deg, ty - g[1] + dir[1] * err_deg],
        );
        // This cmd's round leaves along the view the last one left, not
        // this cmd's turn: the gun's miss of the true chest point from it.
        let off = yaw_diff(view.view[0] + g[0], tp).hypot(yaw_diff(view.view[1] + g[1], ty));
        let cone = (HIT_RADIUS / dist).atan().to_degrees();

        if std::env::var_os("PONYRI_DBG").is_some() {
            eprintln!(
                "BOT org={:?} e={} dist={:.0} off={:.3} cone={:.3} err={:.1} cd={} busy={} ads_frac={} fil={} clip={} view={:?} aim={:?}",
                view.origin,
                e.slot,
                dist,
                off,
                cone,
                self.target.as_ref().map_or(0.0, |t| t.error),
                self.fire_cooldown,
                view.busy_ms,
                view.ads_frac,
                view.friend_in_line,
                view.clip,
                view.view,
                aim
            );
        }
        if view.lmg {
            let ready = self.bipod(view, [tp - g[0], ty], cmd);
            self.trigger(view, dist, off, cone, ready, cmd);
            return Some(aim);
        }
        let ads = view.has_ads
            && if view.sniper {
                dist > SCOPE_MIN_RANGE
            } else {
                dist > self.skill.ads_range
            };
        if ads {
            cmd.buttons |= msg::BUTTON_ADS;
        }
        // A scope or sights still coming up would waste the shot on hip
        // spread; an automatic inside `AUTO_HIP_RANGE` sprays it anyway.
        let rising = view.automatic && dist < AUTO_HIP_RANGE;
        let ready = (!ads || view.ads_frac >= 1.0 || rising) && view.eye_settled;
        self.trigger(view, dist, off, cone, ready, cmd);
        self.footwork(view, dist, ads, cmd);
        // A teammate across the line: step aside, standing and off the
        // sights, until the line clears.
        if view.friend_in_line
            && let Some(p) = view.sidestep
        {
            let toward = (p[1] - view.origin[1])
                .atan2(p[0] - view.origin[0])
                .to_degrees();
            let d = yaw_diff(toward, aim[1]).to_radians();
            cmd.forward = (d.cos() * 127.0).round() as i8;
            cmd.right = (-d.sin() * 127.0).round() as i8;
            cmd.buttons &= !msg::BUTTON_ADS;
            cmd.wbuttons &= !(msg::WBUTTON_CROUCH | msg::WBUTTON_PRONE);
            cmd.up = 0;
        } else {
            self.cover_engage(view, e, dist, aim[1], cmd);
        }
        Some(aim)
    }

    /// A United Offensive LMG fires only with its bipod down and the sight
    /// fully up (coduo-movement.md 6, 7): the sight button deploys it where
    /// the bot stands, on a mount or prone, and the body holds still. A
    /// deploy the engine refuses, or a target past the yaw cap round the
    /// deploy's facing, lets the button go for a while: the breakdown, then
    /// a fresh deploy facing the target. A target past the deploy's pitch
    /// clamp rests the button too, so the pistol comes out: the pinned view
    /// fires under the line. Whether the trigger may go down.
    fn bipod(&mut self, view: &BotView, want: [f32; 2], cmd: &mut UserCmd) -> bool {
        self.lmg_linger = LMG_LINGER_TICKS;
        cmd.forward = 0;
        cmd.right = 0;
        // A pistol draw under way: the sight would hold the LMG out.
        if self.lmg_rest > 0 || self.switch_to.is_some() {
            return false;
        }
        if want[0].abs() > LMG_PITCH_DEG
            || (view.deployed && yaw_diff(want[1], view.deploy_yaw).abs() > LMG_ARC_DEG)
        {
            self.lmg_rest = LMG_REST_TICKS;
            return false;
        }
        self.lmg_tries = if view.deployed { 0 } else { self.lmg_tries + 1 };
        if self.lmg_tries > LMG_TRY_TICKS {
            self.lmg_tries = 0;
            self.lmg_rest = LMG_REST_TICKS;
            return false;
        }
        cmd.buttons |= msg::BUTTON_ADS;
        view.deployed && view.ads_frac >= LMG_READY_FRAC
    }

    /// The walking view one tick on toward the path's heading `want`, at
    /// most [`Skill::walk_turn_deg`], and the (forward, right) keys that run
    /// the body along `want` from it ([`rekey`]'s turn), scaled so the
    /// larger is 127: `PM_CmdScale` then gives the run's full speed.
    fn walk_turn(&self, view: &BotView, want: f32) -> (i8, i8, f32) {
        let cur = view.view[1];
        let d = yaw_diff(want, cur);
        let most = self.skill.walk_turn_deg;
        if d.abs() <= most {
            return (127, 0, want);
        }
        let look = cur + d.clamp(-most, most);
        let off = yaw_diff(want, look).to_radians();
        let (f, r) = (off.cos(), -off.sin());
        let m = f.abs().max(r.abs());
        let key = |k: f32| (k / m * 127.0).round().clamp(-127.0, 127.0) as i8;
        (key(f), key(r), look)
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
        [
            (cur[0] + d[0] * k).clamp(-PITCH_MOST, PITCH_MOST),
            cur[1] + d[1] * k,
        ]
    }

    /// Semi-autos tap, at least `fireTime` apart (AGENTS.md, "Fire is
    /// tapped"); automatics hold the bit for a burst, shorter at range.
    /// Inside the cone the trigger goes down; past it, with a chance that
    /// falls to none at the weapon's reach ([`REACH_AUTO`]), so a settling
    /// aim fires before it has settled.
    fn trigger(
        &mut self,
        view: &BotView,
        dist: f32,
        off: f32,
        cone: f32,
        ready: bool,
        cmd: &mut UserCmd,
    ) {
        if view.clip == 0 || view.friend_in_line || Self::launcher_in_hand(view) {
            self.burst_ticks = 0;
            return;
        }
        let reach = if view.automatic {
            REACH_AUTO
        } else if view.fire_time_ms >= BOLT_MS {
            REACH_BOLT
        } else {
            REACH_SEMI
        };
        if view.automatic && self.burst_ticks > 0 {
            // A burst rides out recoil and wobble, but not a lost line.
            if off <= cone * reach {
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
        if self.fire_cooldown > 0 || view.busy_ms != 0 || !ready || off >= cone * reach {
            return;
        }
        if off > cone {
            let chance = (reach - off / cone) / (reach - 1.0);
            if (self.rand_below(1000) as f32) >= chance * 1000.0 {
                return;
            }
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
    /// only past the weapon's working range. On the sights a spell moves
    /// only with [`Skill::footwork`]'s chance: a slow strafe, or with a bolt
    /// rifle inside [`BACK_OFF_RANGE`] a step back; at range it may lie
    /// down instead. A scope holds still. Off ([`Bot::footwork`]), sighted
    /// shots always stand still, since moving widens the error.
    fn footwork(&mut self, view: &BotView, dist: f32, ads: bool, cmd: &mut UserCmd) {
        if self.strafe_ticks == 0 {
            self.strafe = if self.rand() % 2 == 0 { 127 } else { -127 };
            self.strafe_ticks = 8 + self.rand_below(16);
            let roll = self.rand_below(1000) as f32 / 1000.0;
            self.shuffle = self.footwork && roll < self.skill.footwork;
            if self.shuffle
                && self.prone_ticks == 0
                && dist > PRONE_RANGE
                && roll < self.skill.footwork / 4.0
            {
                self.prone_ticks = 40 + self.rand_below(40);
            } else if self.crouch_ticks == 0
                && self.prone_ticks == 0
                && self.rand_below(if view.sniper { 2 } else { 4 }) == 0
            {
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
        let bolt = !view.automatic && view.fire_time_ms >= BOLT_MS;
        cmd.forward = if dist > range {
            127
        } else if self.shuffle && bolt && dist < BACK_OFF_RANGE {
            -127
        } else {
            0
        };
        cmd.right = if !ads {
            self.strafe
        } else if self.shuffle && !view.sniper {
            self.strafe / 2
        } else {
            0
        };
        if self.prone_ticks > 0 && dist < PRONE_RANGE * 0.75 {
            self.prone_ticks = 0;
        }
        if self.prone_ticks > 0 {
            self.prone_ticks -= 1;
            // Held like the crouch, with `upmove` -127 (protocol doc,
            // "Usercmd input bits"); a prone body fires from where it lies.
            cmd.wbuttons |= msg::WBUTTON_PRONE;
            cmd.up = -127;
            cmd.forward = 0;
            cmd.right = 0;
        } else if self.crouch_ticks > 0 {
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

    /// A fresh wander heading, and a new stall baseline to measure it by.
    /// A new stall window from `at`.
    fn restart_stall(&mut self, at: [f32; 3]) {
        self.stall_origin = at;
        self.stall_ticks = 0;
        self.stall_reach = 0.0;
    }

    /// Whether this tick's [`Self::think`] can read [`BotView::drop_ahead`]:
    /// only a heading picked or kept off the waypoint does ([`Self::shuns`]),
    /// with no waypoint, on a spell, on guard, or on a stall window's last
    /// tick with the body still in its circle. The probe is the view's
    /// dearest part, so the server runs it only then (bot-navigation.md
    /// section 3).
    pub(crate) fn reads_drops(&self, view: &BotView) -> bool {
        let reach = self
            .stall_reach
            .max(dist_sq(view.origin, self.stall_origin));
        view.waypoint.is_none()
            || self.unstick_ticks > 0
            || (self.stall_ticks + 1 >= STALL_TICKS && reach < STALL_RADIUS * STALL_RADIUS)
            || self.objective_standing(view)
    }

    /// Whether this tick's [`Self::think`] can pick a wander heading, the
    /// one reader of [`BotView::wall_ahead`]: a tick [`Self::reads_drops`]
    /// names, or one on guard behind a bipod or at a cover spot (taken
    /// without asking whether a retreat holds it, which the think may
    /// start). Eight box traces a bot, so the server feels for walls only
    /// then.
    pub(crate) fn reads_walls(&self, view: &BotView) -> bool {
        self.reads_drops(view)
            || view.deployed
            || view
                .survive
                .cover
                .is_some_and(|c| retreat::at_cover(view.origin, c))
    }

    fn pick_heading(&mut self, view: &BotView) {
        self.heading = (self.rand() % 360) as f32;
        // Turned a step at a time off a hazard or a drop, beside the
        // heading too where it can (a heading into a wall slides along it:
        // on mp_ship's deck one 30 degrees off the drop slid a bot off it),
        // then off a hazard alone; boxed in by hazards, it goes anyway. A
        // heading into a wall goes nowhere, and reads as no drop, so it
        // comes after every open one (mp_ship's hold ladder head).
        let start = self.heading;
        let turn = |ok: &dyn Fn(f32) -> bool| {
            (0..8)
                .map(|i| (start + 45.0 * i as f32) % 360.0)
                .find(|h| ok(*h))
        };
        let wide = |h: f32| [-45.0, 0.0, 45.0].iter().all(|d| !self.shuns(view, h + d));
        let open = |h: f32| !view.wall_ahead[octant(h)];
        self.heading = turn(&|h| open(h) && wide(h))
            .or_else(|| turn(&|h| open(h) && !self.shuns(view, h)))
            .or_else(|| turn(&wide))
            .or_else(|| turn(&|h| !self.shuns(view, h)))
            .or_else(|| turn(&|h| !view.hazard_ahead[octant(h)]))
            .unwrap_or(start);
        self.heading_ticks = 40 + (self.rand() % 40) as u32;
        self.restart_stall(view.origin);
    }

    /// A hazard, or a drop, lies along `yaw`'s octant. A drop is fine to
    /// a cornered bot, and to one whose path drops: its waypoint lies more
    /// than a jump below. Neither takes one that hurts, unless the path
    /// drops as far: a back down onto a ladder is no licence to leave the
    /// roof (mp_depot's at (-2750, -1217), section 3).
    fn shuns(&self, view: &BotView, yaw: f32) -> bool {
        let o = octant(yaw);
        let jump = vcod_common::pmove::JUMP_HEIGHT;
        let path_drop = view.waypoint.map_or(0.0, |w| view.origin[2] - w[2]);
        let path_drops = path_drop > jump;
        let shun = |depth: f32| {
            if path_drops {
                depth > FALL_HURTS && depth > path_drop + jump
            } else {
                self.unstick_tries < CORNERED || depth > FALL_HURTS
            }
        };
        view.hazard_ahead[o] || view.drop_ahead[o].is_some_and(shun)
    }

    /// The body moves toward a drop it shuns, whatever its heading: one
    /// pushed into a wall slides along it.
    fn sliding_off(&self, view: &BotView) -> bool {
        let [x, y, _] = view.velocity;
        x.hypot(y) > 1.0 && self.shuns(view, y.atan2(x).to_degrees())
    }

    /// The view through a throw: up over the enemy in sight, or where it
    /// was last seen, by a lob that grows with the range, else held where
    /// it is. The cmd used to carry
    /// zero angles here, which threw the frag wherever `delta_angles`
    /// pointed, often into a wall at the thrower's feet.
    fn lob_angles(&self, view: &BotView) -> [i32; 3] {
        let aim = match self.throw_target(view) {
            // The throw leaves along the aim, the view and the gun's
            // offset off it, as a round does ([`BotView::gun_offset`]).
            Some(_) if let Some(aim) = view.lob => {
                let g = view.gun_offset;
                self.turn_toward(view, [aim[0] - g[0], aim[1] - g[1]])
            }
            // Turning away to throw it clear, no lob found yet: up and behind.
            _ if let Some(yaw) = self.throw_away => self.turn_toward(view, [AWAY_PITCH, yaw]),
            Some(at) => {
                let (pitch, yaw) = aim_angles(view, at);
                let range = dist_sq(view.origin, at).sqrt();
                let lob = (range / GRENADE_RANGE * LOB_DEG).clamp(5.0, LOB_DEG);
                self.turn_toward(view, [pitch - lob, yaw])
            }
            None => [yaw_diff(view.view[0], 0.0), view.view[1]],
        };
        cmd_angles(view, aim)
    }

    fn think_grenade(&mut self, view: &BotView, mut cmd: UserCmd) -> UserCmd {
        if let Some(g) = view.grenade {
            cmd.weapon = g;
            if view.weapon == g {
                self.stage = if view.grenade_cooks {
                    // UO's fuse starts at the sight button's edge once the
                    // pullback runs (coduo-movement.md 12): pressed 200 ms
                    // in and held 1 to 1.5 s, which leaves 2.5 s of a 4 s
                    // fuse for the flight.
                    let left = COOK_TICKS + (self.rand() % COOK_SPREAD as i32) as u32;
                    Stage::Cook {
                        left,
                        spoon: left - COOK_SPOON_AFTER,
                    }
                } else {
                    // No cook exists in 1.1 (combat doc 1.11): the release
                    // throws, so the hold is only long enough to look like
                    // a throw, not a cook.
                    let left = 6 + (self.rand() % 8) as u32;
                    Stage::Cook { left, spoon: 0 }
                };
            }
            return cmd;
        }
        self.stage = Stage::Wander;
        cmd
    }

    fn think_cook(&mut self, view: &BotView, mut cmd: UserCmd, left: u32, spoon: u32) -> UserCmd {
        if let Some(g) = view.grenade {
            cmd.weapon = g;
        }
        if view.weapon != cmd.weapon || !view.playing {
            return cmd;
        }
        // An enemy closed inside the blast during the hold: the frag goes
        // the other way, high, once the view has come round. A UO frag
        // cooks on in the hand meanwhile, with 2.5 s and more left.
        let close = view
            .enemy
            .is_some_and(|e| dist_sq(view.origin, e.origin) < GRENADE_MIN * GRENADE_MIN);
        if left <= 1
            && self.throw_away.is_none()
            && let Some(e) = view.enemy.filter(|_| close)
        {
            self.throw_away = Some(aim_angles(view, e.origin).1 + 180.0);
        }
        // The release waits for the view to come round onto the throw: a
        // new target late in the hold turns it at the usual rate, and a
        // frag let go on the old heading hit the wall beside the thrower.
        // A probed lob is waited for in pitch too: its flight is the only
        // one known clear, and one let go 16 degrees over it struck the
        // wall above the enemy and rolled back past the thrower.
        let target = self.throw_target(view);
        let lob = view.lob.filter(|_| target.is_some());
        let yaw = lob
            .map(|l| l[1])
            .or(self.throw_away)
            .or_else(|| target.map(|at| aim_angles(view, at).1));
        // No clear lob at the target yet: hold on for one.
        let blocked = target.is_some() && view.lob.is_none();
        let off = match lob {
            Some(l) => {
                let g = view.gun_offset;
                yaw_diff(l[0], yaw_diff(view.view[0] + g[0], 0.0)).abs() > LOB_SLACK
                    || yaw_diff(l[1], view.view[1] + g[1]).abs() > LOB_SLACK
            }
            None => yaw.is_some_and(|y| yaw_diff(y, view.view[1]).abs() > AWAY_DEG),
        };
        // The pin is not out yet, or the pullback's `holdFireTime` not run:
        // the trigger stays down. A release before it waits for the pin
        // in pmove, and the next tick's switch to the rifle cancelled the
        // throw: 1.1's hold of 6 to 13 ticks from the switch is shorter
        // than the 0.25 s raise and 0.6 s hold, and 30 of 31 frags never
        // left the hand (bot-combat.md 12).
        let pinned = left <= 1 && !view.grenade_ready;
        if pinned {
            self.pin_wait += 1;
        }
        let turning = left <= 1 && !pinned && self.held_over < AWAY_TICKS && (blocked || off);
        if turning {
            self.held_over += 1;
        }
        // Still no clear lob, or the view off it, once the wait is out: a
        // frag with no fuse running is put back, the switch cancelling the
        // pullback with the clip unspent (cod11-combat.md 1.14). One let go
        // anyway met the wall the probe saw and came back at the thrower.
        let put_back = if pinned {
            self.pin_wait > PIN_WAIT_TICKS
        } else {
            left <= 1 && !turning && (blocked || off) && !view.grenade_cooks
        };
        if put_back {
            self.end_throw();
            cmd.weapon = self.rifle;
            self.stage = Stage::BackToRifle { weapon: self.rifle };
            return cmd;
        }
        if left > 1 || turning || pinned {
            cmd.buttons = msg::BUTTON_ATTACK;
            if left <= spoon {
                cmd.buttons |= msg::BUTTON_ADS;
            }
            self.stage = Stage::Cook {
                left: left.saturating_sub(1).max(1),
                spoon,
            };
        } else {
            self.end_throw();
            // The release; the next stage walks the byte back to the rifle.
            self.throws += 1;
            self.want_yell(yell::Yell::Grenade);
            log::debug!(
                "{} throws from {:?}, enemy {:?} at {:.0}, view {:?}",
                self.name,
                view.origin.map(|x| x.round()),
                view.enemy.map(|e| e.origin.map(|x| x.round())),
                view.enemy
                    .map_or(-1.0, |e| dist_sq(view.origin, e.origin).sqrt()),
                view.view.map(|x| x.round()),
            );
            self.stage = Stage::BackToRifle { weapon: self.rifle };
        }
        cmd
    }

    /// A throw is out, put back or cut short by a death: nothing of it
    /// carries over to the next.
    fn end_throw(&mut self) {
        self.throw_away = None;
        self.held_over = 0;
        self.pin_wait = 0;
        self.throw_at = None;
        self.throw_for = ThrowFor::Any;
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
/// A climb this near under its waypoint stops strafing: a body still
/// moving along the face when the step-up over the top comes has the step
/// reverted (`PM_StepSlideMove` keeps the plain slide when it got as far
/// along the velocity), and hangs under the lip once the ladder lets go 8
/// under its top. Two steps leave the bleed a fifth of a second.
const STRAFE_STOP: f32 = 2.0 * vcod_common::pmove::STEPSIZE;
/// Ticks a stuck bot spends on a random heading before its waypoint again.
const UNSTICK_TICKS: u32 = 15;
/// The ticks of a stall window, and the circle a body that stays in it is
/// stuck in ([`Bot::think`]).
const STALL_TICKS: u32 = 10;
const STALL_RADIUS: f32 = 15.0;
/// A spell that starts this many ticks or fewer after the body was on the
/// rungs climbs down on them and holds still off them.
const RUNG_GRACE: u32 = 10;
/// How near a ladder's head, flat, a climb bobbing under it is not stuck
/// (`Bot::topping`).
const TOPPING_REACH: f32 = 24.0;
/// Five seconds held off the ground within a unit of one spot.
const WEDGE_TICKS: u32 = 100;
/// Ten seconds nearest a node with no way on.
const STRAND_TICKS: u32 = 200;
const WEDGE_SLACK: f32 = 1.0;
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
/// (the graph proved the edge the same way, `crate::nav`), unless a run off
/// arrives (`BotView::run_off`).
pub const BACK_DOWN_HEIGHT: f32 = 64.0;
/// The least a body closes on its waypoint in the stall window to count
/// as moving ([`Bot::closes_in`]).
const CLOSING: f32 = 2.0;

/// The pistol comes out against an enemy this close with the primary dry.
const PISTOL_RANGE: f32 = 500.0;
/// Ticks with no enemy in sight before the pistol goes back.
const HOLSTER_TICKS: u32 = 40;
/// A scope inside this is a liability: the zoomed view cannot track a
/// target this close, so the shot goes from the hip.
const SCOPE_MIN_RANGE: f32 = 200.0;

/// An LMG's deploy that has not taken this many ticks after the press was
/// refused (a wall ahead, no room to go prone); the button rests this long
/// before the next try. The bipod stays down this long once the target is
/// gone, the shot waits on the sight this far up (0.99, coduo-movement.md
/// 6), and a target more than this far round the deploy's facing is past
/// `bg_lmg_yawcap`'s 55 degrees, near enough.
const LMG_TRY_TICKS: u32 = 10;
/// In sight this long with no round fired, an enemy is left out of the
/// view this long ([`Bot::count_futile`]).
const FUTILE_TICKS: u32 = 200;
const IGNORE_TICKS: u32 = 600;
const LMG_REST_TICKS: u32 = 40;
const LMG_LINGER_TICKS: u32 = 60;
const LMG_READY_FRAC: f32 = 0.99;
const LMG_ARC_DEG: f32 = 50.0;
/// The pitch clamp of a deployed LMG's view, either way of level
/// (`PM_UpdateViewAngles`, coduo-movement.md 7). A prone deploy measures it
/// from the torso's pitch, which the brain does not read.
const LMG_PITCH_DEG: f32 = 45.0;
/// Ticks between one throw's start and the next's.
const GRENADE_COOLDOWN_TICKS: u32 = 200;
/// Ticks a throw's switch may sit with the weapon machine idle and the
/// weapon it asked for not in hand before the throw is given up: a switch
/// lands a server frame or two after the putaway ends.
const SWITCH_IDLE_TICKS: u32 = 20;
/// An enemy in sight this many ticks with no round at him is thrown at;
/// one lost from sight between these ticks ago is thrown at where he went
/// ([`Bot::throw_spot`]).
const COVER_THROW_TICKS: u32 = 20;
const LOST_THROW_MIN: u32 = 4;
const LOST_THROW_TICKS: u32 = 60;
/// ...if he was slower than this when lost, units/s: a frag at the spot a
/// runner left lands where he no longer is.
const LOST_STILL: f32 = 100.0;
/// A throw starts from a body slower than this, units/s.
const THROW_SPEED: f32 = 60.0;
/// Ticks with no enemy in sight before a clip under half reloads.
const QUIET_RELOAD_TICKS: u32 = 20;
/// A sprint starts with this much straight path ahead and this much of the
/// fatigue bar left, and stops this far from the straight's end, a tick or
/// two of friction back to the graph's pace.
const SPRINT_ROOM: f32 = 400.0;
const SPRINT_FRESH: f32 = 0.5;
const SPRINT_TAIL: f32 = 160.0;
use vcod_common::pmove::cmd::ANGLE2SHORT;

/// Whether a body at `origin` (feet) has reached the chest-high `spot`.
/// The feet stand inside a zone, [`SITE_MARGIN`] in from its sides, or on
/// its stand when the zone is narrower than the margin.
pub(crate) fn inside_site(o: [f32; 3], s: &SiteView) -> bool {
    let inside = (0..2).all(|i| o[i] >= s.mins[i] + SITE_MARGIN && o[i] <= s.maxs[i] - SITE_MARGIN)
        && o[2] <= s.maxs[2]
        && o[2] + PLAYER_HEIGHT >= s.mins[2];
    let flat = (o[0] - s.stand[0]).hypot(o[1] - s.stand[1]);
    inside || (flat < 16.0 && (o[2] - s.stand[2]).abs() < 32.0)
}

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
/// The strafe key that slides a body on a ladder, facing it at `yaw`,
/// toward `m` along the face; none once within [`LADDER_CENTRED`].
pub(crate) fn along_face(yaw: f32, from: [f32; 3], m: [f32; 2]) -> i8 {
    let (s, c) = yaw.to_radians().sin_cos();
    let off = (m[0] - from[0]) * s - (m[1] - from[1]) * c;
    if off.abs() < LADDER_CENTRED {
        0
    } else {
        (off * 16.0).clamp(-64.0, 64.0) as i8
    }
}

fn yaw_diff(to: f32, from: f32) -> f32 {
    (to - from + 180.0).rem_euclid(360.0) - 180.0
}

/// The forward and right keys that move the body the way `forward` and
/// `right` did under a view at `from` (degrees), with the view at `to`.
pub(crate) fn rekey(forward: i8, right: i8, from: f32, to: f32) -> (i8, i8) {
    let d = yaw_diff(from, to).to_radians();
    let (f, r) = (forward as f32, right as f32);
    let key = |k: f32| k.round().clamp(-127.0, 127.0) as i8;
    (
        key(f * d.cos() + r * d.sin()),
        key(r * d.cos() - f * d.sin()),
    )
}

fn deg_short(deg: f32) -> i32 {
    (deg * ANGLE2SHORT) as i32
}

/// A weapon menu's nationality and its weapons, each with the cvar
/// `_teams::restrict` checks it against.
pub(crate) type WeaponMenu = (&'static str, &'static [(&'static str, &'static str)]);

/// The weapons `menu` offers: a nationality's own, or for United
/// Offensive's two-nation menus (`weapon_americangerman`, `bel.gsc` 222)
/// both nations'.
pub(crate) fn menu_weapons<'a>(
    menus: &'a [WeaponMenu],
    menu: &str,
) -> Option<impl Iterator<Item = &'a (&'static str, &'static str)>> {
    let nation = |n: &str| menus.iter().find(|(m, _)| *m == n).map(|(_, w)| *w);
    if let Some(w) = nation(menu) {
        return Some(w.iter().chain([].iter()));
    }
    let (a, b) = menus
        .iter()
        .find_map(|(m, _)| Some((nation(m)?, nation(menu.strip_prefix(m)?)?)))?;
    Some(a.iter().chain(b.iter()))
}

/// The stock weapon menus (`ui_mp/scriptmenus/weapon_<nationality>.menu`),
/// each weapon with the cvar `_teams::restrict` checks it against. Any other
/// response is "restricted" and reopens the menu.
const WEAPON_MENUS: [WeaponMenu; 4] = [
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

/// United Offensive's menus (`pakuo00.pk3`), the same files with
/// `mg30cal_mp` added to the American and British, `svt40_mp` and
/// `dp28_mp` to the Russian, `gewehr43_mp` and `mg34_mp` to the German.
/// The three light machine guns fire only off their bipod
/// (coduo-movement.md 6, 7), which a bot deploys ([`Bot::bipod`]).
pub(crate) const WEAPON_MENUS_UO: [WeaponMenu; 4] = [
    (
        "american",
        &[
            ("m1carbine_mp", "scr_allow_m1carbine"),
            ("m1garand_mp", "scr_allow_m1garand"),
            ("thompson_mp", "scr_allow_thompson"),
            ("bar_mp", "scr_allow_bar"),
            ("springfield_mp", "scr_allow_springfield"),
            ("mg30cal_mp", "scr_allow_mg30cal"),
        ],
    ),
    (
        "british",
        &[
            ("enfield_mp", "scr_allow_enfield"),
            ("sten_mp", "scr_allow_sten"),
            ("bren_mp", "scr_allow_bren"),
            ("springfield_mp", "scr_allow_springfield"),
            ("mg30cal_mp", "scr_allow_mg30cal"),
        ],
    ),
    (
        "russian",
        &[
            ("mosin_nagant_mp", "scr_allow_nagant"),
            ("ppsh_mp", "scr_allow_ppsh"),
            ("svt40_mp", "scr_allow_svt40"),
            ("mosin_nagant_sniper_mp", "scr_allow_nagantsniper"),
            ("dp28_mp", "scr_allow_dp28"),
        ],
    ),
    (
        "german",
        &[
            ("kar98k_mp", "scr_allow_kar98k"),
            ("gewehr43_mp", "scr_allow_gewehr43"),
            ("mp40_mp", "scr_allow_mp40"),
            ("mp44_mp", "scr_allow_mp44"),
            ("kar98k_sniper_mp", "scr_allow_kar98ksniper"),
            ("mg34_mp", "scr_allow_mg34"),
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

    pub(super) fn view() -> BotView {
        BotView {
            origin: [0.0, 0.0, 64.0],
            eye_height: vcod_common::pmove::VIEW_STAND,
            eye_settled: true,
            lean_offset: [0.0; 3],
            delta_angles: [0; 3],
            view: [0.0, 90.0, 0.0],
            gun_offset: [0.0; 2],
            weapon: 10,
            weapons_held: (1 << 10) | (1 << 6),
            clip: 8,
            fire_time_ms: 300,
            busy_ms: 0,
            automatic: false,
            has_ads: true,
            sniper: false,
            lmg: false,
            deployed: false,
            deploy_yaw: 0.0,
            ads_frac: 0.0,
            spread_scale: 0.0,
            speed: 0.0,
            velocity: [0.0; 3],
            dead: false,
            playing: true,
            enemy: None,
            hurt: None,
            friend_in_line: false,
            sidestep: None,
            cover: None,
            hunt: None,
            use_takes_weapon: false,
            grenade: Some(6),
            grenade_cooks: false,
            grenade_ready: true,
            frag_near: None,
            post: None,
            give_way: None,
            lob: None,
            waypoint: None,
            hazard_ahead: [false; 8],
            drop_ahead: [None; 8],
            wall_ahead: [false; 8],
            noise: None,
            linked: false,
            stranded: false,
            on_ladder: false,
            ladder_normal: [0.0; 3],
            tops_out: false,
            ladder_middle: None,
            back_face: None,
            on_ground: true,
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
            move_scale: 1.0,
            fatigue: None,
            sprinting: false,
            sprint_room: 0.0,
            push_off: None,
            unpin: None,
            pistol: None,
            rocket: None,
            primaries: [10, 0],
            launchers: 0,
            gun: None,
            reload_ms: 2500,
            draw_ms: 750,
            loaded: (1 << 10) | (1 << 6),
            sd: None,
            re: None,
            bel: None,
            uo: None,
            one_life: false,
            survive: Default::default(),
            vehicles: Vec::new(),
            seat: None,
            armor: None,
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

    /// Slowing onto a stop under a recall look scales the turned keys, so
    /// the body still heads for the stop (bot-combat.md 17.1).
    #[test]
    fn a_recall_look_slows_onto_a_stop_along_the_path() {
        let mut v = view();
        v.waypoint = Some([0.0, 24.0, 64.0]);
        v.stop = true;
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
    /// (combat doc 15.1).
    #[test]
    fn a_scoped_bot_aims_the_swayed_gun() {
        let (mut bot, mut v) = engaged(3);
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
        for tick in 0..10 {
            let cmd = bot.think(&v);
            if cmd.buttons & BUTTON_ATTACK != 0 {
                fired = Some((tick, v.view));
                break;
            }
            let a = cmd.angles.map(|a| a as f32 / ANGLE2SHORT);
            v.view = [a[0], a[1], 0.0];
        }
        let (tick, at) = fired.expect("never fired");
        assert!(tick > 0, "fired with the gun off the chest");
        let miss = yaw_diff(at[0] + 1.0, tp).hypot(yaw_diff(at[1] - 1.2, ty));
        assert!(miss < 0.01, "the gun missed the chest by {miss} degrees");
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

    /// A target running across the line widens the miss by its speed times
    /// `track_lag`; one running along the line adds nothing.
    #[test]
    fn a_target_crossing_the_line_is_missed_by_more() {
        let miss = |velocity: [f32; 3]| {
            let (mut bot, mut v) = engaged(5);
            bot.skill.error_start = bot.skill.error_floor;
            let at = [900.0, 0.0, 64.0];
            v.enemy = Some(EnemyView {
                slot: 3,
                origin: at,
                velocity,
            });
            let (tp, ty) = aim_angles(&v, at);
            (0..40)
                .map(|_| {
                    let a = bot.think(&v).angles.map(|a| a as f32 / ANGLE2SHORT);
                    v.view = [a[0], a[1], 0.0];
                    yaw_diff(a[0], tp).hypot(yaw_diff(a[1], ty))
                })
                .sum::<f32>()
        };
        let still = miss([0.0; 3]);
        let along = miss([190.0, 0.0, 0.0]);
        let across = miss([0.0, 190.0, 0.0]);
        assert!(along < still * 1.3, "along {along} still {still}");
        assert!(across > still * 1.8, "across {across} still {still}");
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
        });
        v
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
