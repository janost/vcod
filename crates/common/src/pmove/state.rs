//! The playerstate, the cmd input, the move rules and the per-server cvars `pmove` reads.

use super::angles::angle_normalize180;
use super::ground::Takeoff;
use super::ladder::LADDER_YAWCAP;
use super::prone::{LMG_YAWCAP, ProneBody};
use super::stance::view_lerp_length;
use super::{
    GRAVITY, HALF_WIDTH, HEIGHT_CROUCH, HEIGHT_PRONE, HEIGHT_STAND, SCALE_CROUCH, SCALE_PRONE,
    SPECTATOR_MAXS, SPECTATOR_MINS, SPEED_RUN, VIEW_CROUCH, VIEW_PRONE, VIEW_STAND, aim, weapon,
};
use crate::net::protocol::{ENTITYNUM_NONE, ENTITYNUM_WORLD};
use glam::Vec3;

/// One movement event a frame produced, in wire `EV_*` numbering so the
/// client's cue resolver handles it unchanged.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PmEvent {
    pub event: i32,
    /// The wire's `eventParm`: the step's view offset, the landing's bob,
    /// the landing pain's damage; 0 on the rest.
    pub parm: i32,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Stance {
    Stand,
    Crouch,
    Prone,
}

impl Stance {
    /// Bbox height above the feet.
    pub fn height(self) -> f32 {
        match self {
            Stance::Stand => HEIGHT_STAND,
            Stance::Crouch => HEIGHT_CROUCH,
            Stance::Prone => HEIGHT_PRONE,
        }
    }

    /// Fraction of `SPEED_RUN`. Slow-walk is an input modifier, not a stance.
    pub fn speed_scale(self) -> f32 {
        match self {
            Stance::Stand => 1.0,
            Stance::Crouch => SCALE_CROUCH,
            Stance::Prone => SCALE_PRONE,
        }
    }

    /// Eye height above the feet.
    pub fn view_height(self) -> f32 {
        match self {
            Stance::Stand => VIEW_STAND,
            Stance::Crouch => VIEW_CROUCH,
            Stance::Prone => VIEW_PRONE,
        }
    }
}

/// A UO vehicle seat as pmove reads it (docs/research/coduo-vehicles.md, 3).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VehicleSeat {
    /// `ps.vehType`: 1 a four-wheel, 2 a tank, 5 artillery.
    pub kind: i32,
    /// `ps.vehPos`: 1 the driver, 2 the gunner, 3.. passengers.
    pub pos: i32,
    /// The vehicle's angles, wire convention: `pm` +0x3c, the hull's on the
    /// server, the vehicle entity's on the client.
    pub angles: [f32; 3],
}

impl VehicleSeat {
    /// `BG_AllowPlayerWeaponAtVehiclePos` (game 0x23148, cgame 0x30008210):
    /// only a four-wheel's seat 3 fires the rider's own weapon.
    pub fn own_weapon(&self) -> bool {
        self.kind == 1 && self.pos == 3
    }

    /// `pm` +0x48: the view's arcs around the vehicle, pitch and yaw
    /// degrees, set for the four-wheel's seat 3 only (`ClientThink_real`,
    /// cgame `CG_PredictPlayerState` 0x30035800).
    pub fn arcs(&self) -> [f32; 3] {
        if self.own_weapon() {
            [60.0, 70.0, 0.0]
        } else {
            [0.0; 3]
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlayerState {
    pub origin: Vec3, // feet
    pub velocity: Vec3,
    pub yaw: f32, // radians, same convention as FlyCamera
    pub pitch: f32,
    pub stance: Stance,
    pub on_ground: bool,
    /// The entity the last ground trace stood on, retail's `groundEntityNum`
    /// while `on_ground`: the world, or a submodel entity's number. Read it
    /// through [`PlayerState::ground_entity_num`].
    pub ground_entity: u32,
    pub ground_normal: Vec3,
    /// Retail's `leanf` (`ps+0x40`), -1 full left to 1 full right.
    pub lean: f32,
    /// World yaw of the prone body in degrees, retail's `ps.proneDirection`.
    /// Meaningless unless the stance is prone.
    pub prone_direction: f32,
    /// Degrees the view must move this frame to stay inside the prone cone.
    /// Retail applies it to `delta_angles`; the caller owns that, so pmove
    /// reports it rather than writing it.
    pub view_yaw_correction: f32,
    /// The same for the prone pitch clamp, on `delta_angles[0]`, in the
    /// wire's pitch convention (positive down).
    pub view_pitch_correction: f32,
    /// 1.5's prone view pushes on `delta_angles[1]` this frame, already in
    /// ANGLE2SHORT units (each push truncated on its own, as retail adds
    /// them), or `None` when the 1.5 view moved nothing. Under 1.5 it stands
    /// in for `view_yaw_correction`.
    pub view_yaw_shorts: Option<i32>,
    /// The view a prone cap placed this frame, wire degrees (pitch, yaw), as
    /// retail stores it in `viewangles`: what the cmd's view reads instead
    /// of the radians, which do not round-trip.
    pub view_capped: [Option<f32>; 2],
    /// The view yaw before this cmd's angles, retail's `viewangles[1]` at
    /// the top of `PM_UpdateViewAngles`: what 1.5's refused prone turn eases
    /// back toward. [`cmd::apply_view`](super::cmd::apply_view) keeps it.
    pub prev_yaw: f32,
    /// `ps.proneDirectionPitch`: the ground's pitch along the body, eased
    /// toward it while prone. Degrees, wire convention.
    pub prone_direction_pitch: f32,
    /// `ps.proneTorsoPitch`: the ground's pitch along the view, eased the
    /// same way; the prone pitch clamp is centred on it.
    pub prone_torso_pitch: f32,
    /// `fTorsoHeight`, `fTorsoPitch` and `fWaistPitch` (`ps+0x3c4..0x3cc`):
    /// how the prone body bends over the ground, which `BG_CheckProneValid`
    /// writes off its ground samples ([`check_prone`](super::check_prone)).
    pub prone_body: ProneBody,
    /// Retail's `pm_flags` 0x4: the prone press landed on a player moving
    /// forward or back, which throws it into the air (the dive). Held while
    /// the prone key is, and it shortens the eye's drop to 200 ms.
    pub prone_dive: bool,
    /// Retail's `pm_flags` 0x8000: this cmd's prone press, or the body's
    /// swing past the yaw cap, did not fit. Cleared at the top of every
    /// move (`PmoveSingle` 0x33e5a); the client prints "Prone Blocked".
    pub prone_blocked: bool,
    /// The plane the last ground trace hit, walkable or not, and not while
    /// the velocity carries the player off it: retail's `pml.groundPlane`
    /// and its normal. The prone pitch and [`PlayerState::sliding`] read it.
    pub(super) ground_plane: Option<Vec3>,
    /// 1.5's and UO's `pml.almostGroundPlane` (`pml+0x34`): the last ground
    /// trace's plane, or, off the ground, a floor within a unit below. The
    /// sight flag reads it (docs/research/cod15-movement.md 5, "The sight
    /// off the ground"); 1.1 has no such field and reads `ground_plane`.
    pub(super) near_ground: bool,
    /// 0 dry, 1 feet, 2 waist, 3 eyes under (RTCW waterlevel).
    pub water_level: u32,
    /// `pm_time`: what is left of the knockback timer, 0 when free.
    pub knockback_ms: f32,
    /// The `pm_flags` bits riding `knockback_ms`, cleared with it.
    /// [`PMF_TIME_KNOCKBACK`](super::PMF_TIME_KNOCKBACK), `StuckInClient`'s push
    /// (`crates/server/src/game/stuck.rs`) and a damaging landing's stun
    /// (`crash_land`), quarters `walk_move`'s accel and
    /// softens ground friction to 0.3 of its control term
    /// (`docs/research/cod11-player-clip.md`); [`PMF_TIME_DAMAGE`](super::PMF_TIME_DAMAGE) is a hit's.
    pub knockback_flags: i32,
    /// Touching a climbable surface (trace hit with SURF_LADDER) this frame.
    pub on_ladder: bool,
    /// 1.5's `disableWeapon`, `pm_flags` 0x100000 there: the weapon goes
    /// down and stays down as on a ladder (docs/research/cod15-protocol-diff.md,
    /// "Server side").
    pub weapon_disabled: bool,
    /// `ps.speed`, the wish speed every mover and `PM_Footsteps` scale from
    /// and the gun's tilt and position scale against: a playing body's is
    /// `g_speed` (1.1) or the client's `maxspeed` (1.5, UO), stored by
    /// `ClientEndFrame`; a spectator's is 400, or 0 under 1.5's refused free
    /// flight (`SpectatorThink`, 1.5 game.mp 0x389eb). The predictor reads
    /// it off the wire (cod15-movement.md, "ps.speed").
    pub speed: f32,
    /// `ps.gravity`, which the fall, the jump, the dive, the ladder's decay
    /// and the slick slide read: `(int)g_gravity` stored by `ClientEndFrame`
    /// (cod15-movement.md, "ps.gravity").
    pub gravity: f32,
    /// Plane normal of that surface; persists while off the wall so the
    /// airborne probe can stick with the ladder we left.
    pub ladder_normal: Vec3,
    /// ms elapsed since the last jump, ground or ladder; INFINITY when never.
    /// Retail stamps cmd.serverTime into ps.jumpTime after either (@0x2f279,
    /// @0x33964) and compares deltas (@0x2ebb3, @0x33822).
    pub since_jump_ms: f32,
    /// A held jump key blocks another jump until released (pm_flags bit 0x8:
    /// set @0x2ec36, cleared @0x34135 when upmove <= 9).
    pub jump_latched: bool,
    /// Retail's `fJumpOriginZ` (ps+0x68, wire `fJumpPeak`): the takeoff height
    /// plus [`JUMP_HEIGHT`](super::JUMP_HEIGHT) from a jump until the ground trace next hits, 0
    /// otherwise. Only an airborne step reads it.
    pub jump_origin_z: f32,
    /// Footstep phase counter, retail `ps->bobCycle` (ps+0x8). Steps fire on
    /// 128-tick crossings.
    pub bob_cycle: u8,
    /// Retail `ps.movementDir` (ps+0x7c in `PLAYER_FIELDS`): the yaw the legs
    /// are moving along, relative to the view, in whole degrees. The player
    /// entity carries it as `angles2[1]`, which is how another client turns
    /// a strafing player's legs while the torso keeps facing the view.
    pub movement_dir: i32,
    /// Lump-0 `surface_flags` of the ground we stand on; 0 while airborne.
    pub ground_surface_flags: u32,
    /// Origin at the top of this move, retail's `pml.previous_origin`. The
    /// legs' heading and the landing's fall height are measured off it.
    pub(super) move_start: Vec3,
    /// Eased eye height; trails `stance.view_height()` after a stance change
    /// (retail lerps the view while the bbox snaps).
    pub(super) view_height_cur: f32,
    /// Milliseconds into the eye's current leg, retail's `cmd.serverTime -
    /// viewHeightLerpTime`; `None` while no leg runs (`viewHeightLerpTime` 0).
    pub(super) view_lerp_ms: Option<i32>,
    /// 1.1's prone box at the crouch's 50 (`PM_CheckDuck` 0x31c1d-0x31c9b):
    /// set by each cmd's stance pass while the eye is off the prone height
    /// and its leg at most half run (docs/research/cod11-mantle.md, "The
    /// prone box while the eye goes down").
    pub(super) prone_box_crouch: bool,
    /// Retail's `pm_flags` 0x2: set on entering a crouch, cleared on standing,
    /// and left alone by prone, so a prone entered from a crouch carries it
    /// and one entered from standing does not (both measured,
    /// `crates/server/tests/playerstate_motion_ab.rs`).
    pub ducked: bool,
    /// Eye height the last stance change aimed at, and whether it went down.
    /// Both are wire state: retail carries them in `viewHeightLerpTarget` and
    /// `viewHeightLerpDown`, and leaves the target at 0 until the first
    /// stance change (measured, cod11-mantle.md "The eye through a stance
    /// change", `playerstate_motion_ab`).
    pub view_lerp_target: f32,
    pub view_lerp_down: bool,
    /// Retail's `pm_flags` 0x40, the backpedal latch: a backwards cmd sets it,
    /// a forwards one or a pure strafe clears it, and no input at all leaves it
    /// alone. The animation selection reads this bit and never the usercmd
    /// (`game.mp.i386.so` 0x326f1/0x32739/0x32768), so it is the only thing
    /// that puts a player in the `runbk` family.
    pub backwards_run: bool,
    /// Whether this move took a jump impulse, ground or ladder push-off.
    /// Cleared at the top of every move. Leaving the ground is not the same
    /// thing: a player who runs off a ledge or mounts a ladder is airborne
    /// without having jumped, and the animation machine has to tell those
    /// apart (docs/research/player-model-anim-system.md).
    pub jumped: bool,
    /// Whether this move landed fast enough for the land anim: `PM_CrashLand`
    /// raises it only below [`LAND_ANIM_SPEED`](super::LAND_ANIM_SPEED) (docs/research/cod11-sound-system.md,
    /// "Landing"). Cleared at the top of every move, like `jumped`.
    pub land_anim: bool,
    /// The horizontal speed the last `PM_Footsteps` read, ahead of the
    /// move clamp and the velocity snap (`PmoveSingle` calls it at 1.1d
    /// 0x34336 and 1.5b 0x28889, `trap_SnapVector` after): the movetype selects by it, not
    /// by the snapped velocity (player-model-anim-system.md, "How retail
    /// picks the movetype").
    pub footsteps_xyspeed: f32,
    /// The `jump`/`jumpbk` anim event this move raised, if any. Cleared at
    /// the top of every move, like `jumped`.
    pub takeoff: Option<Takeoff>,
    /// UO: this move's `PM_Weapon` raised `LMG_BREAKDOWN` again as a
    /// continue (`weapon::deploy_driver`). Cleared at the top of every move.
    pub breakdown_continue: bool,
    /// 1.5 and UO: this move's `PM_CheckDuck` moved the eye's target into
    /// prone or out of it (`pmove_t` +0xe8 on 1.5b, +0x100 on UO;
    /// `step_view_target`). Cleared at the top of every move.
    pub prone_flip: bool,
    /// The same flip cleared the torso: `BG_PlayAnim(ps, 0, torso, 0, 0,
    /// 1, 1)` (1.5b 0x2401b, 0x240e6; UO 0x28bfa, 0x28cf7), which UO skips
    /// in a deploy or a breakdown.
    pub prone_flip_clears_torso: bool,
    /// `ps.weapon`, a 1-based index into configstring 7; 0 is no weapon.
    pub weapon: u8,
    /// `ps.weapons`, bit N for weapon N.
    pub weapons_held: u64,
    /// `ps.weaponslots`, the weapon index in each slot (0 empty).
    pub weapon_slots: [u8; weapon::NUM_SLOTS],
    /// `ps.weaponstate`, one of `weapon::WEAPON_*`.
    pub weaponstate: u8,
    /// `ps.weaponTime`: while non-zero the machine is busy. 1 is the
    /// semi-automatic latch (combat doc, section 1.4).
    pub weapon_time_ms: i32,
    /// `ps.weaponDelay`: the sub-step inside a state (the shot inside
    /// `fireTime`, the rounds inside a reload).
    pub weapon_delay_ms: i32,
    /// `ps.weapAnim`, a `weapon::WEAP_*` index plus the 512 restart toggle.
    pub weap_anim: i32,
    /// `ps.ammo`, the reserve, by `WeaponDef::ammo_index`.
    pub ammo: [i16; weapon::NUM_AMMO],
    /// `ps.ammoclip`, the loaded magazine, by `WeaponDef::clip_index`.
    pub ammoclip: [i16; weapon::NUM_AMMO],
    /// `ps.weaponrechamber`, bit N set while weapon N holds a spent case
    /// (combat doc, section 1.9).
    pub weapon_rechamber: u64,
    /// `ps.fWeaponPosFrac`, 0 at the hip and 1 at the sight. A netfield the
    /// client predicts from, so a constant here restarts its ADS lerp every
    /// snapshot (`pmove::weapon::advance_ads`).
    pub weapon_pos_frac: f32,
    /// Retail's `pm_flags` 0x20, the ADS flag: the gated form of the usercmd
    /// sight bit, and the only thing the fraction's ramp reads
    /// (`pmove::weapon::update_ads_flag`).
    pub ads_active: bool,
    /// `ps.aimSpreadScale`, 0..255: how far the hip cone has opened
    /// (`pmove::weapon::adjust_aim_spread_scale`).
    pub aim_spread_scale: f32,
    /// `ps.grenadeTimeLeft`: the fuse a pulled grenade carries, in ms, and 0
    /// whenever none is armed. Retail 1.1 MP never counts it down; it holds
    /// `fuseTime` from the pullback to the throw (combat doc, section 1.11).
    pub grenade_time_left_ms: i32,
    /// `pm_flags` 0x1000: the melee bit was down last frame. The only edge
    /// latch in the weapon machine (combat doc, section 1.10).
    pub melee_latched: bool,
    /// The previous cmd's view angles in ANGLE2SHORT units, pitch and yaw.
    /// Retail's `pm->oldcmd.angles`, which the spread's turn term subtracts
    /// this cmd's from.
    pub last_cmd_angles: [i32; 2],
    /// The previous cmd's sight bit, retail's `pm->oldcmd.buttons & 0x10`.
    /// Only the prone arm of the ADS flag reads it.
    pub last_cmd_ads: bool,
    /// The previous cmd's `forwardmove` and `rightmove`, retail's
    /// `pm->oldcmd`, which the prone move key test reads
    /// ([`weapon::prone_move_key`]).
    pub last_cmd_moves: [i8; 2],
    /// Retail's `pm_flags` 0x80, the ADS walk: the sight held on a player
    /// who has the ADS flag, is not prone and is not reloading
    /// (`PM_UpdatePlayerWalkingFlag`, 0x33694). It is what puts
    /// `walkSpeedScale` on the wish speed.
    pub walking: bool,
    /// `ps.pm_type` 1, the link `linkTo` makes: [`pmove`](super::pmove) runs retail's
    /// linked arm, which moves nothing. The caller owns the link and sets it.
    pub linked: bool,
    /// `eFlags & 0xC000`, the mounted-gun bits: `Some(stance)` while riding a
    /// turret, the gun's own stance. `None` off a gun. The caller owns the
    /// mount and sets it (docs/research/cod11-turrets.md, section 5).
    pub mounted: Option<Stance>,
    /// United Offensive's vehicle seat (`eFlags` 0x100000), `None` on foot.
    /// The caller owns it and the vehicle's angles in it
    /// (docs/research/coduo-vehicles.md, 3).
    pub seat: Option<VehicleSeat>,
    /// The two fall damage cvars. Not playerstate on retail but systeminfo
    /// cvars both ends read; the caller copies them in.
    pub fall_heights: FallHeights,
    /// The cgame's bob cvars, which the gun, the sight and the eye bobs
    /// read; the caller copies them in.
    pub bob: BobCvars,
    /// The movement cvars; the caller copies them in.
    pub bg: BgCvars,
    /// The cmd running is a spectator's: [`spectator_move`](super::spectator_move) sets it for its
    /// slide and clears it after, so the hull and the step gate below read
    /// retail's `pm_type` 4 arms.
    pub spectator: bool,
    /// Which patch's movement this player runs: the server's protocol for
    /// its players, the connection's for the predictor
    /// ([`MoveRules::for_protocol`]).
    pub rules: MoveRules,
    /// 1.5's `pm_flags` 0x2000, the jump penalty, timed by `pm_time`
    /// (docs/research/cod15-movement.md, "The jump penalty"). Never set
    /// under 1.1's rules.
    pub jump_penalty: bool,
    /// `viewHeightTarget` under 1.5's rules: the eye height the body has
    /// stepped to, one stance at a time and only while no leg of the eye
    /// runs. 1.5's effective stance, box and `eFlags` follow it
    /// (docs/research/cod15-movement.md, "Stance"). Unread under 1.1's.
    pub view_target: f32,
    /// United Offensive's `pm_flags` 0x10000: this cmd sprints
    /// (`PM_UpdatePlayerSprintingFlag`, coduo-movement.md 2). Never set
    /// under the other rules.
    pub sprinting: bool,
    /// UO's `pm_flags` 0x20000, winded: set under half fatigue, cleared
    /// only once the bar is full again.
    pub winded: bool,
    /// UO's `ps.fatigueScale`, 1 rested down to 0.
    pub fatigue: f32,
    /// UO's `ps.lastSprintTime`: the serverTime of the last sprinting cmd.
    pub last_sprint_time: i32,
    /// UO's `ps.iFatigueSoundTime`: the serverTime of the last winded
    /// breath `fatigue_sound` raised, 0 once rested.
    pub fatigue_sound_time: i32,
    /// UO's `serverCursorHint` 7, a mount ahead the server found (an
    /// `hmgstanding`/`hmgcrouching` brush or a `trigger_mount`), with the
    /// stance `serverCursorHintVal` names: the bipod deploys there even
    /// where the client's own probe sees nothing (coduo-movement.md, 11).
    pub mount_hint: Option<Stance>,
    /// The held weapon is `weaponClass lmg`, read at the top of each step
    /// under UO's rules: with the sight flag it is the bipod deploy
    /// ([`PlayerState::deployed`]).
    pub lmg: bool,
}

/// The movement a patch runs. 1.1 is the reference; 1.5 differs where
/// docs/research/cod15-movement.md says, United Offensive is 1.5's plus
/// what docs/research/coduo-movement.md says (sprint, fatigue, the bipod).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MoveRules {
    #[default]
    V1_1,
    V1_5,
    Uo,
}

impl MoveRules {
    /// The rules a server speaking this protocol runs.
    pub fn for_protocol(p: &crate::net::protocol::Cod1Wire) -> Self {
        match p.transport.version {
            6 => MoveRules::V1_5,
            22 => MoveRules::Uo,
            _ => MoveRules::V1_1,
        }
    }

    /// 1.5's movement, which United Offensive keeps.
    pub fn v15(self) -> bool {
        self != MoveRules::V1_1
    }
}

/// `cg_bobAmplitudeStanding`, `cg_bobAmplitudeDucked`, `cg_bobAmplitudeProne`
/// and `cg_bobMax`: the cgame's own cvars on 1.1 (cgame table 0x30074d70,
/// flag 0x200), which vcod does not register, and on 1.5 also the game
/// module's (game table `.data` 0x83990, flag 0x800), so a 1.5 server's
/// values reach its clients through the 140/204 mirror (cgame 1.5 table
/// 0x30077220, flag 0xa00; cod15-movement.md 6).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BobCvars {
    pub standing: f32,
    pub ducked: f32,
    pub prone: f32,
    pub max: f32,
}

impl Default for BobCvars {
    /// The defaults both patches register.
    fn default() -> Self {
        BobCvars {
            standing: 0.007,
            ducked: 0.0075,
            prone: 0.03,
            max: 8.0,
        }
    }
}

impl BobCvars {
    /// The four out of a server's client cvars; a missing one keeps its
    /// default, an unparsable one reads 0 as `atof` would.
    pub fn from_cvars(get: impl Fn(&str) -> Option<String>) -> Self {
        let d = BobCvars::default();
        let read = |name: &str, default: f32| {
            get(name).map_or(default, |v| v.trim().parse().unwrap_or(0.0))
        };
        BobCvars {
            standing: read("cg_bobAmplitudeStanding", d.standing),
            ducked: read("cg_bobAmplitudeDucked", d.ducked),
            prone: read("cg_bobAmplitudeProne", d.prone),
            max: read("cg_bobMax", d.max),
        }
    }
}

/// The game module's movement cvars pmove reads, each as its
/// `vmCvar.integer`: `bg_ladder_yawcap` (all three patches), and on United
/// Offensive `bg_lmg_yawcap` and `bg_nofatigue`. The server reads its own;
/// the cgame's come over the 140/204 mirror (cod11-mantle.md, "The ladder
/// yaw cap"; coduo-movement.md 1, 7).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BgCvars {
    /// `bg_ladder_yawcap`; 0 lifts the cap.
    pub ladder_yawcap: i32,
    /// `bg_lmg_yawcap`.
    pub lmg_yawcap: i32,
    /// `bg_nofatigue`: a sprint drains nothing. UO's cgame never registers
    /// it, so the predictor keeps it off.
    pub nofatigue: bool,
}

impl Default for BgCvars {
    fn default() -> Self {
        BgCvars {
            ladder_yawcap: LADDER_YAWCAP as i32,
            lmg_yawcap: LMG_YAWCAP as i32,
            nofatigue: false,
        }
    }
}

impl BgCvars {
    /// The three out of a side's cvars through `atoi`; a missing one keeps
    /// its default.
    pub fn from_cvars(get: impl Fn(&str) -> Option<String>) -> Self {
        let d = BgCvars::default();
        let read = |name: &str, default: i32| get(name).map_or(default, |v| crate::text::atoi(&v));
        BgCvars {
            ladder_yawcap: read("bg_ladder_yawcap", d.ladder_yawcap),
            lmg_yawcap: read("bg_lmg_yawcap", d.lmg_yawcap),
            nofatigue: read("bg_nofatigue", 0) != 0,
        }
    }
}

/// `bg_fallDamageMinHeight` and `bg_fallDamageMaxHeight`, the systeminfo
/// cvars `PM_CrashLand` reads (docs/research/cod11-player-clip.md 8.3).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FallHeights {
    pub min: f32,
    pub max: f32,
}

impl Default for FallHeights {
    /// The `gameCvarTable` defaults (rows 0x7e368 and 0x7e380).
    fn default() -> Self {
        FallHeights {
            min: 256.0,
            max: 480.0,
        }
    }
}

impl FallHeights {
    /// The two bounds out of a systeminfo string (configstring 1), the way a
    /// client's cvars take them: a missing key keeps its default.
    pub fn from_systeminfo(info: &str) -> Self {
        let d = FallHeights::default();
        let get = |key, default: f32| {
            crate::net::info_value_for_key(info, key)
                .map_or(default, |v| v.trim().parse().unwrap_or(0.0))
        };
        FallHeights {
            min: get("bg_fallDamageMinHeight", d.min),
            max: get("bg_fallDamageMaxHeight", d.max),
        }
    }
}

impl PlayerState {
    pub fn spawn(origin: Vec3, yaw_deg: f32) -> Self {
        PlayerState {
            origin,
            velocity: Vec3::ZERO,
            yaw: yaw_deg.to_radians(),
            pitch: 0.0,
            stance: Stance::Stand,
            on_ground: false,
            ground_entity: ENTITYNUM_WORLD,
            ground_normal: Vec3::Z,
            lean: 0.0,
            prone_direction: 0.0,
            view_yaw_correction: 0.0,
            view_pitch_correction: 0.0,
            view_yaw_shorts: None,
            view_capped: [None; 2],
            prev_yaw: yaw_deg.to_radians(),
            prone_direction_pitch: 0.0,
            prone_torso_pitch: 0.0,
            prone_body: ProneBody::default(),
            prone_dive: false,
            prone_blocked: false,
            ground_plane: None,
            near_ground: false,
            water_level: 0,
            knockback_ms: 0.0,
            knockback_flags: 0,
            on_ladder: false,
            weapon_disabled: false,
            speed: SPEED_RUN,
            gravity: GRAVITY,
            ladder_normal: Vec3::ZERO,
            since_jump_ms: f32::INFINITY,
            jump_latched: false,
            jump_origin_z: 0.0,
            bob_cycle: 0,
            movement_dir: 0,
            move_start: origin,
            ground_surface_flags: 0,
            view_height_cur: Stance::Stand.view_height(),
            view_lerp_ms: None,
            prone_box_crouch: false,
            ducked: false,
            // Retail leaves the target at 0 until the first stance change.
            view_lerp_target: 0.0,
            view_lerp_down: false,
            backwards_run: false,
            jumped: false,
            land_anim: false,
            footsteps_xyspeed: 0.0,
            takeoff: None,
            breakdown_continue: false,
            prone_flip: false,
            prone_flip_clears_torso: false,
            weapon: 0,
            weapons_held: 0,
            weapon_slots: [0; weapon::NUM_SLOTS],
            weaponstate: weapon::WEAPON_READY,
            weapon_time_ms: 0,
            weapon_delay_ms: 0,
            weap_anim: 0,
            ammo: [0; weapon::NUM_AMMO],
            ammoclip: [0; weapon::NUM_AMMO],
            weapon_rechamber: 0,
            weapon_pos_frac: 0.0,
            ads_active: false,
            aim_spread_scale: 0.0,
            grenade_time_left_ms: 0,
            melee_latched: false,
            last_cmd_angles: [0; 2],
            last_cmd_ads: false,
            last_cmd_moves: [0; 2],
            walking: false,
            linked: false,
            mounted: None,
            seat: None,
            fall_heights: FallHeights::default(),
            bob: BobCvars::default(),
            bg: BgCvars::default(),
            spectator: false,
            rules: MoveRules::V1_1,
            jump_penalty: false,
            view_target: VIEW_STAND,
            sprinting: false,
            winded: false,
            fatigue: 1.0,
            last_sprint_time: 0,
            fatigue_sound_time: 0,
            mount_hint: None,
            lmg: false,
        }
    }

    /// United Offensive's bipod: the sight flag on an LMG, which only the
    /// deploy sets (coduo-movement.md, 7).
    pub fn deployed(&self) -> bool {
        self.rules == MoveRules::Uo && self.lmg && self.ads_active
    }

    /// `ps.groundEntityNum`: `ENTITYNUM_NONE` off the ground.
    pub fn ground_entity_num(&self) -> u32 {
        if self.on_ground {
            self.ground_entity
        } else {
            ENTITYNUM_NONE
        }
    }

    /// Off the ground against a plane too steep to stand on: the last
    /// ground trace hit one, and the velocity does not carry the body off it.
    pub fn sliding(&self) -> bool {
        !self.on_ground && self.ground_plane.is_some()
    }

    /// Sets the plane the last ground trace met, for a body another
    /// game's pmove moved (`vcod-server`'s CoD2 nav walks).
    pub fn set_ground_plane(&mut self, normal: Option<Vec3>) {
        self.ground_plane = normal;
    }

    /// What `PM_UpdateAimDownSightFlag` takes for ground: 1.1's
    /// `pml.groundPlane` (0x37230, a steep face too), 1.5's and UO's
    /// `pml.almostGroundPlane` (1.5 0x2cf6c, UO 0x32c0b). A ground trace
    /// sets both whenever it sets `on_ground`.
    pub fn sight_ground(&self) -> bool {
        self.on_ground
            || if self.rules.v15() {
                self.near_ground
            } else {
                self.ground_plane.is_some()
            }
    }

    pub fn mins(&self) -> Vec3 {
        if self.spectator {
            return SPECTATOR_MINS;
        }
        Vec3::new(-HALF_WIDTH, -HALF_WIDTH, 0.0)
    }

    /// The height of the box `ClientThink_real` leaves in `r.maxs` after
    /// this state's move: the stance box's, or for a spectator the standing
    /// one `ClientSpawn` wrote, which `SpectatorThink` never copies over
    /// (docs/research/cod11-gsc-object-model.md 22.3). `spectator` reads
    /// true only inside [`spectator_move`](super::spectator_move)'s slide, so the callers, all
    /// outside it, never take that arm.
    pub fn box_height(&self) -> f32 {
        if self.spectator {
            return HEIGHT_STAND;
        }
        self.maxs().z - self.mins().z
    }

    pub fn maxs(&self) -> Vec3 {
        if self.spectator {
            return SPECTATOR_MAXS;
        }
        let stance = match self.rules {
            MoveRules::V1_1 if self.stance == Stance::Prone && self.prone_box_crouch => {
                Stance::Crouch
            }
            MoveRules::V1_1 => self.stance,
            // `PM_CheckDuck` sizes the box off the stepped target (1.5
            // 0x24155..0x241ce).
            MoveRules::V1_5 => self.effective_stance(),
            // UO's (0x28d66..0x28ef0) takes the flags' box where they stand
            // higher than the target, so a rise the stance traces let
            // through grows the box at once (coduo-movement.md 18).
            MoveRules::Uo => {
                let risen = self.stance != Stance::Prone && !self.ducked;
                match self.effective_stance() {
                    Stance::Prone if risen => Stance::Stand,
                    Stance::Prone if self.ducked => Stance::Crouch,
                    Stance::Crouch if risen => Stance::Stand,
                    s => s,
                }
            }
        };
        Vec3::new(HALF_WIDTH, HALF_WIDTH, stance.height())
    }

    pub fn view_height(&self) -> f32 {
        self.view_height_cur
    }

    /// A body at rest in `stance`: the flags, the eye and the eye's last
    /// target all agree, as after a stance change whose legs have run.
    pub fn settle_stance(&mut self, stance: Stance) {
        self.stance = stance;
        self.ducked = stance == Stance::Crouch;
        self.view_height_cur = stance.view_height();
        self.view_lerp_target = stance.view_height();
        self.view_target = stance.view_height();
        self.view_lerp_ms = None;
    }

    /// Whether the eye has caught up with the stance it is easing towards.
    pub fn view_height_settled(&self) -> bool {
        (self.view_height_cur - self.stance.view_height()).abs() < 0.01
    }

    /// `viewHeightLerpTime` for a playerstate whose `commandTime` is
    /// `server_time`: the time the running leg began, or 0.
    pub fn view_lerp_stamp(&self, server_time: i32) -> i32 {
        self.view_lerp_ms
            .map_or(0, |ms| server_time.wrapping_sub(ms))
    }

    /// The stance `ps.eFlags` carries (0x20 crouch, 0x40 prone): the stance
    /// flags on 1.1, the eye's on 1.5 and UO (cod15-movement.md 3).
    pub fn eflags_stance(&self) -> Stance {
        match self.rules {
            MoveRules::V1_1 => self.stance,
            MoveRules::V1_5 | MoveRules::Uo => self.effective_stance(),
        }
    }

    /// `PM_GetEffectiveStance` (0x34554): the stance the eye is in or
    /// easing towards, so a body still rising out of prone counts as prone.
    pub fn effective_stance(&self) -> Stance {
        if self.rules.v15() {
            // 1.5 0x1e9a5: the target alone.
            return match self.view_target {
                VIEW_CROUCH => Stance::Crouch,
                VIEW_PRONE => Stance::Prone,
                _ => Stance::Stand,
            };
        }
        let lerping = self.view_lerp_ms.is_some();
        if self.stance == Stance::Prone
            || self.view_lerp_target == VIEW_PRONE
            || (lerping && self.view_lerp_target == VIEW_CROUCH && !self.view_lerp_down)
        {
            Stance::Prone
        } else if self.ducked || (lerping && self.view_lerp_target == VIEW_CROUCH) {
            Stance::Crouch
        } else {
            Stance::Stand
        }
    }

    /// `BG_PlayerStateToEntityState`'s prone body (0x2cdd3..0x2cec4): the
    /// playerstate's, folded through `AngleNormalize180` and scaled by how far
    /// the eye's leg into or out of prone has run, for a body whose effective
    /// stance is prone (`PM_GetEffectiveStance` 0x34554), and flat otherwise.
    /// `lerp_time` is the wire's `viewHeightLerpTime`.
    pub fn entity_prone_body(&self, lerp_time: i32, command_time: i32) -> ProneBody {
        let lerping = lerp_time != 0;
        let prone = self.stance == Stance::Prone
            || self.view_lerp_target == VIEW_PRONE
            || (lerping && self.view_lerp_target == VIEW_CROUCH && !self.view_lerp_down);
        if !prone {
            return ProneBody::default();
        }
        let scale = if lerping {
            let len = view_lerp_length(self.view_lerp_target, self.view_lerp_down, self.prone_dive);
            let f = (command_time.wrapping_sub(lerp_time) as f32 / len as f32).clamp(0.0, 1.0);
            if self.view_lerp_down { f } else { 1.0 - f }
        } else {
            1.0
        };
        let b = self.prone_body;
        ProneBody {
            torso_height: b.torso_height * scale,
            torso_pitch: angle_normalize180(b.torso_pitch) * scale,
            waist_pitch: angle_normalize180(b.waist_pitch) * scale,
        }
    }

    /// The eye, `AddLeanToPosition` applied as `G_AddLean` does on the
    /// rules' Linux server, and the view angles. A lean moves the eye and
    /// never rolls the view.
    pub fn view(&self) -> ViewParams {
        let arith = if self.rules.v15() {
            crate::patch::Arith::Stored
        } else {
            crate::patch::Arith::Kept
        };
        let eye = self.origin + Vec3::Z * self.view_height();
        ViewParams {
            eye: aim::add_lean(eye, self.yaw, self.lean, arith),
            yaw: self.yaw,
            pitch: self.pitch,
        }
    }
}

pub struct ViewParams {
    pub eye: Vec3,
    pub yaw: f32,
    pub pitch: f32,
}

#[derive(Default, Clone, Copy, Debug, PartialEq)]
pub struct PmInput {
    pub forward: f32, // -1..1
    pub right: f32,   // -1..1
    /// The cmd's `upmove` over 127. Only `PM_CmdScale` reads it raw; the
    /// jump and the stances read [`Self::jump`] and the stance bits.
    pub up: f32, // -1..1
    pub jump: bool,
    pub crouch: bool, // held
    pub prone: bool,  // toggled state, main.rs owns the toggle
    /// `wbuttons` 0x2: the stance comes from a held `+prone` or `+movedown`
    /// rather than `cl_stance`, so a refusal forces nothing on the client.
    pub stance_held: bool,
    /// `wbuttons` 0x4, which a shellshocked client holds: the walk's wish
    /// speed takes [`SCALE_SHELLSHOCK`](super::SCALE_SHELLSHOCK).
    pub shellshock: bool,
    pub walk_slow: bool,
    pub lean_left: bool,
    pub lean_right: bool,
    pub attack: bool,
    /// The usercmd's melee bit (`BUTTON_MELEE`, 0x20).
    pub melee: bool,
    pub reload: bool,
    pub ads: bool,
    pub use_button: bool,
    /// The usercmd's weapon byte; 0 means the cmd asks for no change.
    pub weapon: u8,
    /// The cmd's view angles in ANGLE2SHORT units, pitch and yaw. Only the
    /// spread's turn term reads them, and it reads the raw cmd rather than
    /// `ps.viewangles` (combat doc, 2.1).
    pub angles: [i32; 2],
    /// United Offensive's sprint button, `buttons` 0x8.
    pub sprint: bool,
    /// The step's `cmd.serverTime`; only UO's fatigue clock reads it.
    pub server_time: i32,
}
