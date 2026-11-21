//! What the server shows a bot each tick: [`BotView`], the per-gametype views, and [`BotBody`] for
//! the gates.

use super::*;

/// What the brain needs to know about its own body this tick. The server
/// fills it from the sim, the weapon table and the script's team table.
/// Compared whole by the pass's plans on worker threads (`driver::par`).
/// `Default` is the empty view each game's builder fills over.
#[derive(Clone, Default, PartialEq)]
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
    /// 1.7's float `ps.delta_angles`, degrees, which [`cmd_angles`] rounds
    /// the cmd off as 1.7 does; `None` on CoD1 and CoD2.
    pub delta_deg: Option<[f32; 3]>,
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
    /// The pitch a deployed LMG's view is clamped round, wire degrees
    /// (positive down): `proneTorsoPitch` once deployed or prone, which the
    /// clamp measures from (coduo-movement.md 7), else level, the deploy's
    /// prone torso not yet laid on the ground.
    pub lmg_pitch_from: f32,
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
    /// The burning UO hulls still to go off a second time, for a bot on
    /// foot: inside one's [`WRECK_REACH`] it walks the graph out, fight or
    /// no fight, and a point goal inside one waits (bot-objectives.md
    /// 10.10).
    pub wrecks: Vec<[f32; 3]>,
    /// Where a client of the bot's side died within the last
    /// [`posts::FALLEN_MS`]: a `bas` charge room one died in is held, and
    /// takes a frag before the bot goes in ([`Bot::room_frag`]).
    pub fallen: Vec<[f32; 3]>,
    /// A post the game sends the bot to, walked to over the graph ahead
    /// of a chase: CoD4's mounted gun a bot is making for
    /// (`bots::cod4::gun`). Never set on CoD1 or CoD2.
    pub post: Option<[f32; 3]>,
    /// A spot a step away from a teammate held up behind this bot, who is
    /// planning round it (`bots::driver`'s `give_way`): the bot makes
    /// room, whatever post it holds. Never set on CoD1 or CoD2.
    pub give_way: Option<[f32; 3]>,
    /// The post round its objective the driver dealt the bot
    /// ([`posts::assign`]), with team play and [`Bot::tactics`] on: it
    /// walks there and holds it instead of the first spot in reach.
    pub guard_post: Option<GuardPost>,
    /// A standing box at the feet starts clear (`driver::stand_room`): a
    /// body that crouches or lies down where it does not cannot rise again
    /// (`pmove::update_stance`), so a standing bot takes neither there.
    pub stand_room: bool,
    /// A way in to the objective the bot makes for, dealt by the driver
    /// apart from its teammates' ([`posts::lanes`]): it walks there first.
    pub lane: Option<[f32; 3]>,
    /// The share of the objective's ways in that see the body
    /// ([`posts::Survey::exposure`]), sampled every few ticks near one:
    /// `bot_match`'s count only.
    pub exposure: Option<f32>,
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
    /// [`Self::waypoint`] lies more than a jump down a drop a run off
    /// misses (`nav::Follower::backs_off`): the graph backed down it.
    pub back_drop: bool,
    /// Hold still on the rungs: another body over the bot on them is not
    /// rising (`nav::Follower::under_hold`).
    pub rungs_hold: bool,
    /// The edge under way is a CoD2 mantle (`nav::Follower::mantling`):
    /// pmove offers the climb only to a view on the wall
    /// (bot-combat.md 17.1), so no look turns it away.
    pub mantle: bool,
    /// pmove's climb is under way (CoD4's `PMF_MANTLE`): the body moves on
    /// the climb's own track, so it is no stall (bot-navigation.md 3.15).
    pub climbing: bool,
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
    /// The follower planned round a body in the way this tick
    /// (`nav::Follower::bodies_in_way`): held up, not stuck.
    pub detour: bool,
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

/// A guard's post ([`BotView::guard_post`]): the feet, whether a crouch
/// there hides it from most of the ways in, and the objective it watches.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GuardPost {
    pub at: [f32; 3],
    pub crouch: bool,
    pub look: [f32; 3],
    /// Walking in to it with care: a guard pulled back to the objective
    /// (bot-objectives.md 14) walks the last [`CAREFUL_NEAR`] looking at
    /// `toward`, the objective, and crouches the last [`CAREFUL_CROUCH`].
    pub careful: Option<[f32; 3]>,
}

/// Behind Enemy Lines as the bot plays it (docs/research/bot-objectives.md,
/// "Behind Enemy Lines"): a few allies score by staying alive, the axis
/// hunt them, and an axis player who kills one takes his place.
#[derive(Clone, Debug, PartialEq)]
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
#[derive(Clone, Debug, PartialEq)]
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

#[derive(Clone, Copy, Debug, PartialEq)]
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
    /// Dropped and lying: where its pickup trigger's middle goes back to
    /// and the seconds left before `objective_timeout` sends it there
    /// (`re.gsc` 2422-2432, 60 s from the drop).
    pub returns: Option<([f32; 3], f32)>,
}

/// Which side of the S&D objective the bot's team plays this map.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ObjRole {
    Attack,
    Defend,
}

/// The stock `sd.gsc` objectives as the server read them this frame
/// (docs/research/bot-objectives.md).
#[derive(Clone, Debug, PartialEq)]
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
    /// The site an enemy was seen near by a teammate within the last
    /// [`driver::HOT_MS`], before a plant: every defender converges on it
    /// (set by the driver after the game's deal, with
    /// [`Bot::converge`] on).
    pub hot: Option<usize>,
}

/// CoD4's carried bomb (`_gameobjects.gsc` `setKeyObject`,
/// docs/research/bot-objectives.md 12).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct KeyBomb {
    /// Where the bomb lies, for the one bot that walks into it to take it.
    pub fetch: Option<[f32; 3]>,
    /// The bot carries it: the rest of its team guard the site.
    pub mine: bool,
    /// How long a plant and a defuse hold use.
    pub plant_ticks: u32,
    pub defuse_ticks: u32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SiteView {
    /// The zone's absolute bounds.
    pub mins: [f32; 3],
    pub maxs: [f32; 3],
    /// A feet origin inside the bounds the navigation graph reaches.
    pub stand: [f32; 3],
}

#[derive(Clone, Copy, Debug, PartialEq)]
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
#[derive(Clone, Debug, PartialEq)]
pub enum UoView {
    Ctf(Box<CtfView>),
    Dom(DomView),
    Hq(HqView),
    Bas(BasView),
}

impl UoView {
    /// The bot's place among its team's bots by slot: roles and zones are
    /// dealt out by it, as [`SdView::rank`].
    pub(super) fn rank(&self) -> usize {
        match self {
            UoView::Ctf(v) => v.rank,
            UoView::Dom(v) => v.rank,
            UoView::Hq(v) => v.rank,
            UoView::Bas(v) => v.rank,
        }
    }
}

/// Capture the flag: the bot's team's flag and the other team's.
#[derive(Clone, Debug, PartialEq)]
pub struct CtfView {
    pub ours: CtfFlagView,
    pub theirs: CtfFlagView,
    /// The bot carries the other team's flag.
    pub carrying: bool,
    /// A teammate carrying the other team's flag, where he stands (the
    /// compass shows teammates).
    pub escort: Option<[f32; 3]>,
    pub rank: usize,
    /// Where a teammate last saw the enemy carrying the bot's flag, for a
    /// bot the driver sends after him (bot-objectives.md 14, with
    /// [`Bot::cues`] on).
    pub hunt: Option<[f32; 3]>,
    /// The point the driver sends one of a carrier's escorts to, ahead of
    /// him along his path or at his side ([`Bot::ctf_escort`]).
    pub lead: Option<[f32; 3]>,
    /// The outpost the driver keeps the bot on once its flag is taken,
    /// looking at the carrier's way out ([`Bot::ctf_watch`],
    /// bot-objectives.md 14.13).
    pub watch: Option<GuardPost>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
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
    /// Who carries it: the driver knows him when a teammate sees him.
    pub carrier: Option<usize>,
}

/// Domination: every flag zone and who holds it.
#[derive(Clone, Debug, PartialEq)]
pub struct DomView {
    pub flags: Vec<DomFlagView>,
    pub rank: usize,
}

#[derive(Clone, Copy, Debug, PartialEq)]
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
#[derive(Clone, Debug, PartialEq)]
pub struct HqView {
    pub radio: Option<HqRadioView>,
    /// No radio up: the compass marker of the next one.
    pub next: Option<[f32; 3]>,
    pub rank: usize,
}

#[derive(Clone, Copy, Debug, PartialEq)]
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
#[derive(Clone, Debug, PartialEq)]
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

#[derive(Clone, Debug, PartialEq)]
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
    /// In a tank's driver's seat: a shell fired now bursts on the base
    /// (`Server::shell_view`).
    pub laid: bool,
}

/// What the server deals a `bas` bot's view from, once a tick
/// (`Server::bot_uo_view`).
pub(crate) struct Breach<'a> {
    /// The bot holds a loaded rocket launcher ([`BotView::rocket`]).
    pub rocket: bool,
    /// The bot drives a tank: its gun ([`SeatView::sight`]) and the hull,
    /// which its own shell passes.
    pub tank: Option<(GunSight, vcod_gsc::EntId)>,
    pub launchers: &'a [[f32; 3]],
    pub guns: &'a [[f32; 3]],
    /// The script models round the bases, which a rocket strikes.
    pub models: &'a [crate::game::combat::PlacedModel],
}

/// The last hit another client dealt a bot.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hurt {
    /// The attacker's client slot.
    pub slot: usize,
    /// Ms since the hit.
    pub ago_ms: i32,
    /// The damage direction, from the attacker toward the body; zero
    /// when the hit carried none.
    pub dir: [f32; 3],
}

#[derive(Clone, Copy, PartialEq)]
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
    /// How the last retreat ended (`Bot::retreat_end`): 1 held, 2 loaded,
    /// 3 out of rounds, 4 no cover, 5 a duty, 6 timed out; 0 none.
    pub retreat_end: u8,
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
    /// The point its objective centred on last think
    /// ([`Bot::objective_point`]), and the share of that objective's ways
    /// in that saw it ([`Bot::exposure`]).
    pub objective: Option<[f32; 3]>,
    pub exposure: Option<f32>,
    /// The point it guarded last think ([`Bot::guard_point`]).
    pub guard: Option<[f32; 3]>,
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
