//! Contains routines ported from the Quake III Arena GPL source, Copyright (C) 1999-2005 Id Software, Inc.,
//! and the RTCW-MP GPL source, Copyright (C) 1999-2010 id Software LLC, a ZeniMax Media company. See NOTICE.
//!
//! Q3/RTCW-derived player movement on top of `movetrace::MoveWorld`. Every
//! constant's provenance: docs/research/bsp-ibsp59-format.md, "Movement
//! constants and their provenance".

use crate::collision::MASK_PLAYERSOLID;
use crate::movetrace::{MASK_DEADSOLID, MoveWorld};
use crate::net::protocol::{ENTITYNUM_NONE, ENTITYNUM_WORLD};
use crate::weapon::WeaponDef;
use glam::Vec3;

pub mod aim;
pub mod cmd;
pub mod predict;
pub mod weapon;

pub const GRAVITY: f32 = 800.0;
pub const SPEED_RUN: f32 = 190.0;
/// Wire `ps.speed` of a captured retail spectator.
pub const SPEED_SPECTATOR: f32 = 400.0;
/// Retail rodata 0x70860; a spectator's fly friction (Q3's PM_Friction with
/// pm_spectatorfriction instead of the walk constant).
pub const PM_SPECTATOR_FRICTION: f32 = 5.0;
pub const SCALE_WALK: f32 = 0.4;
pub const SCALE_CROUCH: f32 = 0.65;
pub const SCALE_PRONE: f32 = 0.15;
/// Retail's `ps.backSpeedScale`, `strafeSpeedScale` and `leanSpeedScale`
/// as the wire carries them (`crates/server/tests/fixtures/playerstate/
/// mp_carentan-dm.txt`); the walk mover's cmd scale reads all three
/// (game.mp.i386.so 0x2e690, cod11-mantle.md "The wish speed").
pub const SCALE_BACK: f32 = 0.7;
pub const SCALE_STRAFE: f32 = 0.8;
pub const SCALE_LEAN: f32 = 0.4;
/// The wade slowdown `1 - waterlevel / 3 * WADE_SCALE` in the same scale
/// (rodata 0x70890).
const WADE_SCALE: f32 = 0.5;
/// Prone-dive heights: vz = sqrt(2 * height * GRAVITY). Retail rodata
/// 0x70BE8/0x70BEC, applied in fn 0x316F4 @0x31CC0 (game.mp.i386.so).
pub const DIVE_HEIGHT_STAND: f32 = 34.0;
pub const DIVE_HEIGHT_LOW: f32 = 24.0;
/// `PM_CheckJump` (0x2eb98): vz = sqrt(g * 78) and `fJumpOriginZ` 39 above
/// the takeoff (rodata 0x708c8/0x708cc), so the jump's apex and its origin
/// are one height (docs/research/cod11-mantle.md, "Jumps").
pub const JUMP_HEIGHT: f32 = 39.0;
/// The jump's `aimSpreadScale` kick and its cap (rodata 0x708dc/0x708e0).
const JUMP_SPREAD_ADD: f32 = 64.0;
const JUMP_SPREAD_MAX: f32 = 255.0;
// Accelerate/friction/stopspeed: retail CoD 1.1 rodata (game.mp.i386.so),
// loaded by PM_Friction @0x2e460 and the movers.
pub const PM_ACCELERATE: f32 = 9.0;
/// Stance accelerates, selected at 0x2f4b0-0x2f4ca in `PM_WalkMove`.
pub const PM_DUCKED_ACCELERATE: f32 = 12.0;
pub const PM_PRONE_ACCELERATE: f32 = 19.0;
pub const PM_AIRACCELERATE: f32 = 1.0;
pub const PM_FRICTION: f32 = 5.5;
/// Friction control floor: drop uses max(speed, stopspeed) (@0x2e500).
pub const PM_STOPSPEED: f32 = 100.0;
/// The walk's own accel floor: `PM_WalkMove` scales the accel by
/// max(wishspeed, 100) (rodata 0x70908, @0x2f50f), not by the wish speed.
pub const WALK_ACCEL_FLOOR: f32 = 100.0;
pub const STEPSIZE: f32 = 18.0;
/// The revert test's margin, rodata 0x70ef4 (`PM_StepSlideMove` 0x35441).
const STEP_REVERT_EPS: f32 = 0.001;
/// Step height while prone (PM_StepSlideMove @0x35045 tests pm_flags bit 0x1).
pub const STEPSIZE_PRONE: f32 = 10.0;
pub const OVERCLIP: f32 = 1.001;
/// `pm_flags` bit 0x100, the timer `StuckInClient`'s push and the landing
/// stun start (`docs/research/cod11-player-clip.md` 7 and 8,
/// `PM_DropTimers` 0x32a44).
pub const PMF_TIME_KNOCKBACK: i32 = 0x100;
/// `pm_flags` bit 0x200, the timer a damage knockback starts
/// (`docs/research/cod11-combat.md` 4.5): no ground friction, a walk accel of
/// 1 and gravity on the ground while it runs.
pub const PMF_TIME_DAMAGE: i32 = 0x200;
/// `PM_Friction`'s ground control multiplier while the knockback timer runs
/// (0x2e51c).
const KNOCKBACK_FRICTION_SCALE: f32 = 0.3;
/// `PM_WalkMove`'s accel multiplier while the knockback timer runs (0x2f4d8).
const KNOCKBACK_ACCEL_SCALE: f32 = 0.25;
/// Entity numbers below this are clients.
const MAX_CLIENTS: u32 = 64;
pub const MIN_WALK_NORMAL: f32 = 0.7;
pub const MAX_CLIP_PLANES: usize = 5;
pub const HALF_WIDTH: f32 = 15.0; // bbox is (-15,-15,0)..(15,15,height)
pub const HEIGHT_STAND: f32 = 70.0;
pub const HEIGHT_CROUCH: f32 = 50.0;
pub const HEIGHT_PRONE: f32 = 30.0;
pub const VIEW_STAND: f32 = 60.0;
pub const VIEW_CROUCH: f32 = 40.0;
pub const VIEW_PRONE: f32 = 11.0;
/// `deadViewHeight`, which retail leaves at 8 through a death and a respawn
/// (docs/research/cod11-combat.md, section 8.1).
pub const VIEW_DEAD: f32 = 8.0;
/// The dead eye drops 9 units per 50 ms snapshot, 60 to 8 over six frames in
/// the same capture; a rate, not one of the stance lerp times.
pub const DEAD_VIEW_LERP_SPEED: f32 = 180.0;
/// The `Pmove` chop, not a discard: retail runs a move longer than this as
/// several `PmoveSingle` steps of at most this length (`game.mp.i386.so`
/// 0x344d3). `docs/protocol-1.1.md`, "How long a cmd is simulated for".
pub const MAX_FRAME_MS: f32 = 66.0;
pub const LEAN_MAX: f32 = 28.0; // eye offset in units; roll is lean/2 degrees
pub const LEAN_TIME_TO_MS: f32 = 340.0;
pub const LEAN_TIME_FROM_MS: f32 = 350.0;

/// The eye's lerp times in ms, per leg (`PM_ViewHeightAdjust` 0x309d8,
/// docs/research/cod11-mantle.md, "The eye through a stance change"): the
/// `bg_duck2prone_time`/`bg_prone2duck_time` defaults into and out of prone,
/// and the immediates of the other legs.
const VIEW_LERP_PRONE_MS: i32 = 400;
const VIEW_LERP_DIVE_MS: i32 = 200;
const VIEW_LERP_DUCK_MS: i32 = 150;
const VIEW_LERP_DIVE_DUCK_MS: i32 = 100;
const VIEW_LERP_STAND_MS: i32 = 200;

/// The eye's curve per leg, `(percent, height)` waypoints from `.data`
/// 0x7c730..0x7c8f0; the third word of each record, an origin offset, is 0
/// throughout.
const VIEW_CURVE_STAND_CROUCH: &[(i32, f32)] = &[
    (0, 60.0),
    (1, 59.5),
    (4, 58.5),
    (30, 56.0),
    (80, 44.0),
    (90, 41.5),
    (95, 40.5),
    (100, 40.0),
];
const VIEW_CURVE_CROUCH_STAND: &[(i32, f32)] = &[
    (0, 40.0),
    (5, 40.5),
    (10, 41.5),
    (20, 44.0),
    (70, 56.0),
    (96, 58.5),
    (99, 59.5),
    (100, 60.0),
];
const VIEW_CURVE_CROUCH_PRONE: &[(i32, f32)] = &[
    (0, 40.0),
    (11, 38.0),
    (22, 33.0),
    (34, 25.0),
    (45, 16.0),
    (50, 15.0),
    (55, 16.0),
    (70, 18.0),
    (90, 17.0),
    (100, 11.0),
];
const VIEW_CURVE_DIVE_PRONE: &[(i32, f32)] = &[(0, 40.0), (100, 11.0)];
const VIEW_CURVE_PRONE_CROUCH: &[(i32, f32)] = &[
    (0, 11.0),
    (5, 10.0),
    (30, 21.0),
    (50, 25.0),
    (67, 31.0),
    (83, 34.0),
    (100, 40.0),
];

/// Prone tunables, from the retail server: `bg_prone_yawcap` 85 and
/// `bg_prone_softyawedge` 1 are cvars, the 55 deg/s swing rate and the
/// 54-unit body clearance are rodata. The clearance is traced with a
/// +/-6 box straight behind the facing, which is the space a body needs to
/// lie down in (docs/research/cod11-mantle.md, "Prone").
pub const PRONE_YAWCAP: f32 = 85.0;
/// The body only starts turning once the view is this far off it.
pub const PRONE_SOFT_EDGE: f32 = PRONE_YAWCAP - 5.0;
pub const PRONE_SWING_DEG_PER_SEC: f32 = 55.0;
/// How far the prone view may pitch off `proneTorsoPitch`, rodata 0x70c94
/// and 0x70c98 (`PM_UpdateViewAngles`).
pub const PRONE_PITCHCAP: f32 = 45.0;
/// The rate `proneDirectionPitch` and `proneTorsoPitch` ease toward the
/// ground's pitch, rodata 0x70ca4 (`PM_UpdatePronePitch`).
pub const PRONE_PITCH_DEG_PER_SEC: f32 = 70.0;
pub const PRONE_BODY_LENGTH: f32 = 54.0;
const PRONE_BODY_HALF_BOX: f32 = 6.0;

// Water: RTCW-MP bg_pmove.c multipliers against CoD's absolute speeds.
// Swim cap is SCALE_SWIM * SPEED_RUN; no lava/slime exists in CoD maps.
pub const SCALE_SWIM: f32 = 0.5;
pub const WATER_ACCELERATE: f32 = 4.0;
pub const WATER_FRICTION: f32 = 1.0;
/// Idle wish toward the bottom while swimming.
pub const WATER_SINK_SPEED: f32 = 60.0;
pub const WATERJUMP_FORWARD: f32 = 200.0;
pub const WATERJUMP_UP: f32 = 350.0;
pub const WATERJUMP_TIME_MS: f32 = 2000.0;

// Ladders: structure from RTCW-MP bg_pmove.c PM_CheckLadderMove/PM_LadderMove;
// numbers from retail CoD 1.1 (game.mp.i386.so: PM_CheckLadderMove @0x336e8,
// PM_LadderMove @0x33944, .rodata floats, pm_ladderfriction data symbol),
// which retunes RTCW's 1/48 reach, 0.5 upscale bias, 0.9 climb / 0.5 strafe
// coeffs, 100-cap-less wishspeed and friction 14.
/// Forward-trace reach while walking / airborne.
pub const LADDER_TRACE_DIST_WALK: f32 = 8.0;
pub const LADDER_TRACE_DIST_AIR: f32 = 30.0;
/// Horizontal shrink per side of the ladder probe box (rodata 0x70CB0).
pub const LADDER_PROBE_SHRINK: f32 = 6.0;
pub const LADDER_UPSCALE_BIAS: f32 = 0.25;
pub const LADDER_UPSCALE_GAIN: f32 = 2.5;
pub const LADDER_CLIMB_SCALE: f32 = 0.5;
pub const LADDER_STRAFE_SCALE: f32 = 0.2;
pub const LADDER_WISHSPEED_CAP: f32 = 100.0;
pub const LADDER_ACCELERATE: f32 = 9.0;
pub const PM_LADDER_FRICTION: f32 = 16.0;
/// RTCW's grab-from-above push into the wall (`ladderforward`).
pub const LADDER_PUSH_SPEED: f32 = 200.0;
/// Re-grab lock after any jump: pm_ladderJumpTime = 300 int
/// (rodata 0x70830, compared as delta <= 299 @0x33822).
pub const LADDER_REGRAB_LOCK_MS: f32 = 300.0;
/// Re-jump gate on every jump: cmd.serverTime - ps.jumpTime > 499 (@0x2ebb3).
pub const JUMP_COOLDOWN_MS: f32 = 500.0;
/// Horizontal reset along the reflected forward when leaving a ladder
/// (pm_ladderPushOff, rodata 0x708d8).
pub const LADDER_PUSHOFF_SPEED: f32 = 128.0;

// Footstep cadence, decoded from PM_ShouldMakeFootsteps @0x322c8 and
// PM_FootstepEvent @0x31fe4 (game.mp.i386.so); facts live in
// docs/research/cod11-sound-system.md, "Footstep and landing cadence".
/// Wire `EV_*` group bases; the sound surface index is added to the base.
const EV_FOOTSTEP_RUN_BASE: i32 = 1;
const EV_FOOTSTEP_WALK_BASE: i32 = 24;
const EV_FOOTSTEP_PRONE_BASE: i32 = 47;
const EV_JUMP_BASE: i32 = 70;
const EV_LANDING_BASE: i32 = 93;
/// `PM_CrashLand`'s gate on the land anim, against the vertical velocity the
/// move started with (`game.mp.i386.so` rodata 0x70a08).
pub const LAND_ANIM_SPEED: f32 = -220.0;
/// `EV_STEP_VIEW`: the vertical jump the step machinery added this frame,
/// which the client smooths the eye over
/// (docs/research/cod11-mantle.md, "The step event and the velocity scale").
const EV_STEP_VIEW: i32 = 143;
/// Step below which retail raises nothing (double @0x70f08).
const STEP_VIEW_EPS: f32 = 0.5;
/// Clamp and bias the rounded step takes before it becomes the parm
/// (@0x35727-0x35742); the parm is 8 bits on the wire.
const STEP_VIEW_MIN: i32 = -16;
const STEP_VIEW_MAX: i32 = 24;
const STEP_VIEW_BIAS: i32 = 128;
/// Post-step velocity scale `0.2 + 0.8 * (1 - |dz| / stepSize)`
/// (rodata 0x70f14/0x70f10, applied @0x35770).
const STEP_SCALE_BASE: f32 = 0.2;
const STEP_SCALE_GAIN: f32 = 0.8;
/// Ladder climb steps stay quiet this long after a jump
/// (`cmd.serverTime - ps->jumpTime <= 0x12b`, @0x323b9-0x323c8).
const LADDER_STEP_QUIET_MS: f32 = 299.0;
/// Water-material footstep ids (base + 20), fixed regardless of ground surface.
const EV_FOOTSTEP_PRONE_WATER: i32 = 67;
const EV_FOOTSTEP_WALK_WATER: i32 = 44;
const EV_FOOTSTEP_RUN_WATER: i32 = 21;
/// Material surfaceparm that silences footsteps and landings
/// (`sf & 0x2000`, PM_FootstepEvent @0x32167, PM_CrashLand @0x30108).
const SURF_NO_SOUND: u32 = 0x2000;
/// Ladder-step interval divisor and rate keys (rodata 0x70c10-18).
const LADDER_STEP_DIVISOR: f32 = 95.25;
const LADDER_STEP_K_WALK: f32 = 0.35;
const LADDER_STEP_K_RUN: f32 = 0.45;
/// Ladder probe: box shrink per side, z floor, reach along -ladder_normal
/// (rodata 0x70c04/08/0c). A missed trace or material 0 defaults to metal (13).
const LADDER_STEP_PROBE: f32 = 31.0;
const DEFAULT_MATERIAL: i32 = 13;

/// How far the legs may turn off the view. The ground path caps at 90
/// (@0x2e9d2), `PM_LadderMove` at 75 (@0x33dd1).
const MOVEMENT_DIR_CAP: i32 = 90;
const LADDER_MOVEMENT_DIR_CAP: i32 = 75;

/// One movement event a frame produced, in wire `EV_*` numbering so the
/// client's cue resolver handles it unchanged.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PmEvent {
    pub event: i32,
    /// The wire's `eventParm`. Every movement event carries 0; the weapon
    /// step is what will fill it.
    pub parm: i32,
}

/// Ticks per millisecond for each gait; with MP-default speed scales the
/// retail weight `W` collapses to the current speed, leaving bare K
/// (PM_ShouldMakeFootsteps 0x326c4-0x327cc, rodata 0x70c28-0x70c44).
fn step_rate(stance: Stance, walking: bool, backpedal: bool) -> f32 {
    match stance {
        Stance::Stand => match (walking, backpedal) {
            (true, true) => 0.325,
            (true, false) => 0.305,
            (false, true) => 0.36,
            (false, false) => 0.335,
        },
        Stance::Crouch if walking => 0.315,
        Stance::Crouch => 0.34,
        Stance::Prone if walking => 0.24,
        Stance::Prone => 0.25,
    }
}

/// Advance the bob cycle; one step event fires per crossing of a multiple of
/// 128, which is `(old + 64) ^ (new + 64)` going negative on the byte ring.
fn tick_bob_cycle(old: u8, advance: f32) -> u8 {
    ((old as f32 + advance).round() as i32 & 0xff) as u8
}

fn crossed_step_boundary(old: u8, new: u8) -> bool {
    (old.wrapping_add(64) ^ new.wrapping_add(64)) & 0x80 != 0
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

#[derive(Clone, Copy, Debug)]
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
    pub lean: f32, // -LEAN_MAX..LEAN_MAX
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
    /// `ps.proneDirectionPitch`: the ground's pitch along the body, eased
    /// toward it while prone. Degrees, wire convention.
    pub prone_direction_pitch: f32,
    /// `ps.proneTorsoPitch`: the ground's pitch along the view, eased the
    /// same way; the prone pitch clamp is centred on it.
    pub prone_torso_pitch: f32,
    /// Retail's `pm_flags` 0x4: the prone press landed on a player moving
    /// forward or back, which throws it into the air (the dive). Held while
    /// the prone key is, and it shortens the eye's drop to 200 ms.
    pub prone_dive: bool,
    /// The plane the last ground trace hit, walkable or not, and not while
    /// the velocity carries the player off it: retail's `pml.groundPlane`
    /// and its normal. Only the prone pitch reads it.
    ground_plane: Option<Vec3>,
    /// 0 dry, 1 feet, 2 waist, 3 eyes under (RTCW waterlevel).
    pub water_level: u32,
    /// Remaining control lock while flying out of water; 0 when free.
    pub waterjump_ms: f32,
    /// `pm_time`: what is left of the knockback timer, 0 when free.
    pub knockback_ms: f32,
    /// The `pm_flags` bits riding `knockback_ms`, cleared with it.
    /// [`PMF_TIME_KNOCKBACK`], `StuckInClient`'s push
    /// (`crates/server/src/game/stuck.rs`) and a damaging landing's stun
    /// (`crash_land`), quarters `walk_move`'s accel and
    /// softens ground friction to 0.3 of its control term
    /// (`docs/research/cod11-player-clip.md`); [`PMF_TIME_DAMAGE`] is a hit's.
    pub knockback_flags: i32,
    /// Touching a climbable surface (trace hit with SURF_LADDER) this frame.
    pub on_ladder: bool,
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
    /// plus [`JUMP_HEIGHT`] from a jump until the ground trace next hits, 0
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
    move_start: Vec3,
    /// Eased eye height; trails `stance.view_height()` after a stance change
    /// (retail lerps the view while the bbox snaps).
    view_height_cur: f32,
    /// Milliseconds into the eye's current leg, retail's `cmd.serverTime -
    /// viewHeightLerpTime`; `None` while no leg runs (`viewHeightLerpTime` 0).
    view_lerp_ms: Option<i32>,
    /// Retail's `pm_flags` 0x2: set on entering a crouch, cleared on standing,
    /// and left alone by prone, so a prone entered from a crouch carries it
    /// and one entered from standing does not (both measured,
    /// `crates/server/tests/playerstate_motion_ab.rs`).
    pub ducked: bool,
    /// Eye height the last stance change aimed at, and whether it went down.
    /// Both are wire state: retail carries them in `viewHeightLerpTarget` and
    /// `viewHeightLerpDown`, and leaves the target at 0 until the first
    /// stance change (measured, docs/research/cod11-player-movement.md).
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
    /// raises it only below [`LAND_ANIM_SPEED`] (docs/research/cod11-sound-system.md,
    /// "Landing"). Cleared at the top of every move, like `jumped`.
    pub land_anim: bool,
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
    /// The weapon a putaway in flight will raise; 0 means none is.
    pub pending_weapon: u8,
    /// The weapon the ladder holstered, to be given back at the top. Retail
    /// re-reads `cmd.weapon` there instead (`pmove::weapon::leave_ladder`).
    pub stowed_weapon: u8,
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
    /// `pm_flags` 0x400: a weapon change threw away a cooking grenade. Not a
    /// netfield, and nothing reads it yet; retail ORs the bit only for a
    /// prone player (combat doc, section 1.8).
    pub grenade_cancelled: bool,
    /// The previous cmd's view angles in ANGLE2SHORT units, pitch and yaw.
    /// Retail's `pm->oldcmd.angles`, which the spread's turn term subtracts
    /// this cmd's from.
    pub last_cmd_angles: [i32; 2],
    /// The previous cmd's sight bit, retail's `pm->oldcmd.buttons & 0x10`.
    /// Only the prone arm of the ADS flag reads it.
    pub last_cmd_ads: bool,
    /// Retail's `pm_flags` 0x80, the ADS walk: the sight held on a player
    /// who has the ADS flag, is not prone and is not reloading
    /// (`PM_UpdatePlayerWalkingFlag`, 0x33694). It is what puts
    /// `walkSpeedScale` on the wish speed.
    pub walking: bool,
    /// `ps.pm_type` 1, the link `linkTo` makes: [`pmove`] runs retail's
    /// linked arm, which moves nothing. The caller owns the link and sets it.
    pub linked: bool,
    /// `eFlags & 0xC000`, the mounted-gun bits: `Some(stance)` while riding a
    /// turret, the gun's own stance. `None` off a gun. The caller owns the
    /// mount and sets it (docs/research/cod11-turrets.md, section 5).
    pub mounted: Option<Stance>,
    /// The two fall damage cvars. Not playerstate on retail but systeminfo
    /// cvars both ends read; the caller copies them in.
    pub fall_heights: FallHeights,
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
            prone_direction_pitch: 0.0,
            prone_torso_pitch: 0.0,
            prone_dive: false,
            ground_plane: None,
            water_level: 0,
            waterjump_ms: 0.0,
            knockback_ms: 0.0,
            knockback_flags: 0,
            on_ladder: false,
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
            ducked: false,
            // Retail leaves the target at 0 until the first stance change.
            view_lerp_target: 0.0,
            view_lerp_down: false,
            backwards_run: false,
            jumped: false,
            land_anim: false,
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
            pending_weapon: 0,
            stowed_weapon: 0,
            weapon_pos_frac: 0.0,
            ads_active: false,
            aim_spread_scale: 0.0,
            grenade_time_left_ms: 0,
            melee_latched: false,
            grenade_cancelled: false,
            last_cmd_angles: [0; 2],
            last_cmd_ads: false,
            walking: false,
            linked: false,
            mounted: None,
            fall_heights: FallHeights::default(),
        }
    }

    /// `ps.groundEntityNum`: `ENTITYNUM_NONE` off the ground.
    pub fn ground_entity_num(&self) -> u32 {
        if self.on_ground {
            self.ground_entity
        } else {
            ENTITYNUM_NONE
        }
    }

    pub fn mins(&self) -> Vec3 {
        Vec3::new(-HALF_WIDTH, -HALF_WIDTH, 0.0)
    }

    pub fn maxs(&self) -> Vec3 {
        Vec3::new(HALF_WIDTH, HALF_WIDTH, self.stance.height())
    }

    pub fn view_height(&self) -> f32 {
        self.view_height_cur
    }

    /// Whether the eye has caught up with the stance it is easing towards.
    pub fn view_height_settled(&self) -> bool {
        (self.view_height_cur - self.stance.view_height()).abs() < 0.01
    }

    /// `viewHeightLerpTime` for a playerstate whose `commandTime` is
    /// `server_time`: the time the running leg began, or 0.
    pub fn view_lerp_stamp(&self, server_time: i32) -> i32 {
        self.view_lerp_ms.map_or(0, |ms| server_time - ms)
    }

    /// Eye and angles with the lean offset; roll = lean/2 degrees (RTCW).
    pub fn view(&self) -> ViewParams {
        let right = Vec3::new(self.yaw.sin(), -self.yaw.cos(), 0.0);
        ViewParams {
            eye: self.origin + Vec3::Z * self.view_height() + right * self.lean,
            yaw: self.yaw,
            pitch: self.pitch,
            roll: (self.lean * 0.5).to_radians(),
        }
    }
}

pub struct ViewParams {
    pub eye: Vec3,
    pub yaw: f32,
    pub pitch: f32,
    pub roll: f32,
}

#[derive(Default, Clone, Copy, Debug, PartialEq)]
pub struct PmInput {
    pub forward: f32, // -1..1
    pub right: f32,   // -1..1
    pub jump: bool,
    pub crouch: bool, // held
    pub prone: bool,  // toggled state, main.rs owns the toggle
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
}

/// `PM_DropTimers` (0x32a44) runs from `PmoveSingle` for every `pm_type`, so
/// every arm below calls this once a move to keep a pushed player's penalty
/// ticking down even while linked, mounted or dead.
fn drop_knockback(ps: &mut PlayerState, dt: f32) {
    if ps.knockback_ms > 0.0 {
        let ms = dt * 1000.0;
        if ms >= ps.knockback_ms {
            ps.knockback_ms = 0.0;
            ps.knockback_flags = 0;
        } else {
            ps.knockback_ms -= ms;
        }
    }
}

/// `dt` in seconds, clamped to `MAX_FRAME_MS`. Returns the frame's movement
/// sound events in wire `EV_*` numbering.
pub fn pmove(
    ps: &mut PlayerState,
    input: &PmInput,
    world: &MoveWorld,
    dt: f32,
    weapons: &[Option<WeaponDef>],
) -> Vec<PmEvent> {
    let dt = dt.min(MAX_FRAME_MS / 1000.0);
    ps.view_yaw_correction = 0.0;
    ps.view_pitch_correction = 0.0;
    ps.since_jump_ms += dt * 1000.0;
    // retail clears the held-jump latch post-move when upmove drops (@0x34135)
    if !input.jump {
        ps.jump_latched = false;
    }
    // The backpedal latch, decided before the move from the cmd alone, RTCW's
    // `PmoveSingle` block verbatim (`bg_pmove.c:3915`).
    if input.forward < 0.0 {
        ps.backwards_run = true;
    } else if input.forward > 0.0 || input.right != 0.0 {
        ps.backwards_run = false;
    }
    // `PM_AdjustAimSpreadScale` runs first of all, ahead of the view-angle
    // update and of every flag this frame writes, so it reads the previous
    // frame's ground state and ADS fraction (combat doc, 2.1).
    weapon::adjust_aim_spread_scale(
        ps,
        input,
        weapons.get(ps.weapon as usize).and_then(Option::as_ref),
        dt,
    );
    let weapon_def = weapons.get(ps.weapon as usize).and_then(Option::as_ref);
    let mut events = Vec::new();
    ps.jumped = false;
    ps.land_anim = false;
    let was_on_ground = ps.on_ground;
    // retail's `pml.previous_origin` and `previous_velocity`, taken at the
    // top of PmoveSingle
    ps.move_start = ps.origin;
    let start_vz = ps.velocity.z;
    // 0x34274: a mounted player's pmove updates the sight flag, the walking
    // flag and the stance to the gun's, and returns before the move/ground/
    // weapon dispatch below ever runs; the turret moves the body
    // (docs/research/cod11-turrets.md, section 5).
    if let Some(gun) = ps.mounted {
        ps.on_ground = false;
        ps.ground_normal = Vec3::Z;
        ps.ground_surface_flags = 0;
        weapon::update_ads_flag(ps, input, weapon_def);
        ps.walking = walking_flag(ps, input);
        ps.stance = gun;
        ps.ducked = gun == Stance::Crouch;
        ps.lean = 0.0;
        ps.on_ladder = false;
        drop_knockback(ps, dt);
        return events;
    }
    if ps.linked {
        linked_move(ps, input, world, dt, weapons, &mut events);
        return events;
    }
    // `PM_UpdateViewAngles` runs ahead of the stance (`PmoveSingle` 0x340fc),
    // so the prone clamps read last frame's stance and pitches.
    update_prone_view(ps, input, world, dt);
    update_stance(ps, input, world, dt);
    update_lean(ps, input, world, dt);
    set_water_level(ps, world);
    ground_trace(ps, world, MASK_PLAYERSOLID);
    // Retail updates the ADS flag once per `pm_type` arm, after the ground
    // trace and before the arm's move (`PM_UpdateAimDownSightFlag`, combat
    // doc 1.13), so it reads the ground state the move starts from.
    weapon::update_ads_flag(ps, input, weapon_def);
    // `PM_UpdatePlayerWalkingFlag` follows it in the same arm (0x342d8), so
    // the walk reads the ADS flag this frame just set.
    ps.walking = walking_flag(ps, input);
    // Then `PM_UpdatePronePitch` (0x342dd), off this frame's ground plane.
    update_prone_pitch(ps, dt);
    if ps.waterjump_ms > 0.0 {
        ps.waterjump_ms -= dt * 1000.0;
        if ps.waterjump_ms < 0.0 {
            ps.waterjump_ms = 0.0;
        }
    }
    // retail checks ladders right after the first ground trace and dispatches
    // them before waterjump/water; the check reads `pm_time` before
    // `PM_DropTimers` (0x342f4, 0x342f9)
    let ladder = check_ladder_move(ps, input, world);
    drop_knockback(ps, dt);
    if let Some((normal, ladderforward)) = ladder {
        ladder_move(ps, input, normal, ladderforward, world, dt, &mut events);
    } else if ps.waterjump_ms > 0.0 {
        water_jump_move(ps, world, dt, MASK_PLAYERSOLID, Some(&mut events));
    } else if ps.water_level > 1 {
        water_move(ps, input, world, dt, MASK_PLAYERSOLID, Some(&mut events));
    } else {
        // `PM_WalkMove` opens with `PM_CheckJump` (0x2f261); a jump runs the
        // air mover instead and stamps `jumpTime` after it (0x2f279).
        let mut cmd = *input;
        let jumped = ps.on_ground && check_jump(ps, &mut cmd, None, &mut events);
        if ps.on_ground {
            friction(ps, ps.on_ladder, dt);
            walk_move(
                ps,
                &cmd,
                weapon_def,
                world,
                dt,
                MASK_PLAYERSOLID,
                Some(&mut events),
            );
        } else {
            air_move(ps, &cmd, world, dt, MASK_PLAYERSOLID, Some(&mut events));
        }
        if jumped {
            ps.since_jump_ms = 0.0;
        }
    }
    ground_trace(ps, world, MASK_PLAYERSOLID);
    // `PM_CrashLand` runs inside the ground trace (0x30721), so it reads the
    // water level the frame began with and the footsteps read its damping.
    if !was_on_ground && ps.on_ground {
        crash_land(ps, start_vz, false, &mut events);
        ps.land_anim = start_vz < LAND_ANIM_SPEED;
    }
    set_water_level(ps, world);

    // PM_Footsteps @0x322c8 runs once per move, after the final ground trace.
    footsteps(ps, input, world, dt, &mut events);
    weapon::pm_weapon(
        ps,
        input,
        weapons,
        (dt * 1000.0).round() as i32,
        &mut events,
    );
    // Retail's `pm->oldcmd`: what the next step subtracts this one from.
    ps.last_cmd_angles = input.angles;
    ps.last_cmd_ads = input.ads;
    clamp_velocity_to_move(ps, dt);
    snap_velocity(ps);
    events
}

/// `PmoveSingle`'s arm for `pm_type` 1 (0x34220): no ground trace, no move,
/// no footsteps and no velocity snap, so a linked player's origin and velocity
/// stay where the link froze them. What runs is the view (prone cap, lean),
/// the sight flags, the stance and its eye lerp, the timers and the weapon
/// (docs/research/cod11-gsc-object-model.md, 23.2).
fn linked_move(
    ps: &mut PlayerState,
    input: &PmInput,
    world: &MoveWorld,
    dt: f32,
    weapons: &[Option<WeaponDef>],
    events: &mut Vec<PmEvent>,
) {
    // `PM_UpdateViewAngles`, ahead of the dispatch in every arm; under a link
    // the lean skips its wall clamp (0x32c63).
    update_prone_view(ps, input, world, dt);
    update_lean_unclamped(ps, input, dt);
    // The arm's first store: `groundEntityNum` 1023, `pml.walking` and
    // `pml.groundPlane` 0.
    ps.on_ground = false;
    ps.ground_plane = None;
    ps.ground_normal = Vec3::Z;
    ps.ground_surface_flags = 0;
    weapon::update_ads_flag(
        ps,
        input,
        weapons.get(ps.weapon as usize).and_then(Option::as_ref),
    );
    ps.walking = walking_flag(ps, input);
    update_stance(ps, input, world, dt);
    if ps.waterjump_ms > 0.0 {
        ps.waterjump_ms = (ps.waterjump_ms - dt * 1000.0).max(0.0);
    }
    drop_knockback(ps, dt);
    weapon::pm_weapon(ps, input, weapons, (dt * 1000.0).round() as i32, events);
    ps.last_cmd_angles = input.angles;
    ps.last_cmd_ads = input.ads;
}

/// `PM_UpdatePlayerWalkingFlag` (0x33694): the sight held on a player who
/// has the ADS flag, is not prone and is not reloading.
fn walking_flag(ps: &PlayerState, input: &PmInput) -> bool {
    input.ads
        && ps.ads_active
        && ps.stance != Stance::Prone
        && !matches!(
            ps.weaponstate,
            weapon::WEAPON_RELOADING..=weapon::WEAPON_RELOAD_END
        )
}

/// The default arm's tail ahead of the snap (0x34398-0x3443d): a frame that
/// moved the player less than half what its velocity says takes the move
/// itself, over the frame time, as the velocity.
fn clamp_velocity_to_move(ps: &mut PlayerState, dt: f32) {
    let moved = ps.origin - ps.move_start;
    if ps.velocity.length_squared() * 0.25 > moved.length_squared() / (dt * dt) {
        ps.velocity = moved / dt;
    }
}

/// `trap_SnapVector(ps.velocity)`, the last call of `PmoveSingle`'s default
/// arm: each component rounded to the nearest integer, ties to even, and
/// stored back as a float (docs/research/cod11-mantle.md, "Frame flow").
fn snap_velocity(ps: &mut PlayerState) {
    // Through `i32` so a component rounding to zero stores +0.0, as the
    // engine's `fistp`/`fild` pair does; `round_ties_even` alone keeps -0.0.
    let snap = |v: f32| v.round_ties_even() as i32 as f32;
    ps.velocity = Vec3::new(
        snap(ps.velocity.x),
        snap(ps.velocity.y),
        snap(ps.velocity.z),
    );
}

/// A dead player's frame: gravity and ground friction with no input, no
/// stance, lean or weapon step, and the eye easing to `VIEW_DEAD`. Q3's
/// `PM_DEAD` arm of `PmoveSingle` with the movement input zeroed; the eye
/// rate is the retail capture's (`DEAD_VIEW_LERP_SPEED`). A corpse landing
/// runs `PM_CrashLand` too, which takes no damage at `pm_type > 5`; its
/// events are the return.
pub fn dead_move(ps: &mut PlayerState, world: &MoveWorld, dt: f32) -> Vec<PmEvent> {
    let dt = dt.min(MAX_FRAME_MS / 1000.0);
    let idle = PmInput::default();
    let mut events = Vec::new();
    ps.jumped = false;
    let was_on_ground = ps.on_ground;
    ps.move_start = ps.origin;
    let start_vz = ps.velocity.z;
    drop_knockback(ps, dt);
    // `PM_ClearAimDownSightFlag` (`game.mp.i386.so` 0x3abd4), which
    // `PmoveSingle` calls in the dead arm. The fraction is left where the
    // death froze it: the weapon step that would ramp it down does not run
    // for a dead player (combat doc, 1.12 and 1.13).
    ps.ads_active = false;
    ground_trace(ps, world, MASK_DEADSOLID);
    // No events and no post-step velocity scale for a corpse: retail's step
    // block sits behind `ps->pm_type > 5` (@0x35660).
    ps.walking = false;
    if ps.on_ground {
        friction(ps, false, dt);
        walk_move(ps, &idle, None, world, dt, MASK_DEADSOLID, None);
    } else {
        air_move(ps, &idle, world, dt, MASK_DEADSOLID, None);
    }
    ground_trace(ps, world, MASK_DEADSOLID);
    // `PmoveSingle`'s dead arm takes the default path's two ground traces
    // (0x342ce, 0x34327), and the landing one calls `PM_CrashLand`.
    if !was_on_ground && ps.on_ground {
        crash_land(ps, start_vz, true, &mut events);
    }
    // A target no stance has drops any leg and moves at a flat rate (0x30a84).
    ps.view_lerp_ms = None;
    let step = DEAD_VIEW_LERP_SPEED * dt;
    let gap = VIEW_DEAD - ps.view_height_cur;
    ps.view_height_cur += gap.clamp(-step, step);
    // `pm_type` 6 takes the default arm, tail and snap included.
    clamp_velocity_to_move(ps, dt);
    snap_velocity(ps);
    events
}

/// Retail footstep cadence (`PM_Footsteps` @0x322c8). The bob cycle ticks by
/// `msec * rate`; a step event fires on each 128-tick crossing while move
/// keys are held and the gait is audible. The ladder branch runs ahead of the
/// speed gates (@0x323a2 precedes the fcom @0x3249f).
fn footsteps(
    ps: &mut PlayerState,
    input: &PmInput,
    world: &MoveWorld,
    dt: f32,
    events: &mut Vec<PmEvent>,
) {
    let msec = dt * 1000.0;
    if ps.on_ground {
        let speed = (ps.velocity.x * ps.velocity.x + ps.velocity.y * ps.velocity.y).sqrt();
        if speed >= 10.0 {
            let walking = input.walk_slow;
            let backpedal = input.forward < 0.0;
            let rate = step_rate(ps.stance, walking, backpedal);
            let old = ps.bob_cycle;
            ps.bob_cycle = tick_bob_cycle(old, msec * rate);
            if input.forward != 0.0 || input.right != 0.0 {
                // Audible only upright with the walk key released
                // (PM_ShouldMakeFootsteps @0x3221c).
                let enable = ps.stance == Stance::Stand && !walking;
                ground_step_event(ps, old, ps.bob_cycle, walking, enable, events);
            }
        } else if speed > 1.0 {
            // creep: freeze at phase zero so the next gait starts together (@0x324ba)
            ps.bob_cycle = 0;
        }
    } else if ps.on_ladder {
        // quiet for 299 ms after a jump (@0x323b9)
        if ps.since_jump_ms > LADDER_STEP_QUIET_MS {
            let k = if input.walk_slow {
                LADDER_STEP_K_WALK
            } else {
                LADDER_STEP_K_RUN
            };
            let rate = ps.velocity.z / LADDER_STEP_DIVISOR * k;
            let old = ps.bob_cycle;
            ps.bob_cycle = tick_bob_cycle(old, msec * rate);
            ladder_step_event(ps, world, old, ps.bob_cycle, events);
        }
    }
}

/// `PM_FootstepEvent` @0x31fe4 for a grounded step: water ids first, then the
/// ground trace's material.
fn ground_step_event(
    ps: &PlayerState,
    old: u8,
    new: u8,
    walking: bool,
    enable: bool,
    events: &mut Vec<PmEvent>,
) {
    if !crossed_step_boundary(old, new) {
        return;
    }
    match ps.water_level {
        1 | 2 => {
            // Fixed water-material ids; they bypass the enable gate.
            let id = match ps.stance {
                Stance::Prone => EV_FOOTSTEP_PRONE_WATER,
                _ if walking => EV_FOOTSTEP_WALK_WATER,
                _ => EV_FOOTSTEP_RUN_WATER,
            };
            events.push(PmEvent { event: id, parm: 0 });
            return;
        }
        3 => return,
        _ => {}
    }
    if !enable {
        return;
    }
    push_surface_step(
        ps,
        EV_FOOTSTEP_RUN_BASE,
        EV_FOOTSTEP_WALK_BASE,
        EV_FOOTSTEP_PRONE_BASE,
        walking,
        events,
    );
}

fn push_surface_step(
    ps: &PlayerState,
    run_base: i32,
    walk_base: i32,
    prone_base: i32,
    walking: bool,
    events: &mut Vec<PmEvent>,
) {
    let sf = ps.ground_surface_flags;
    if sf & SURF_NO_SOUND != 0 {
        return;
    }
    let mat = crate::collision::sound_material(sf);
    if mat == 0 {
        return;
    }
    let base = match ps.stance {
        Stance::Prone => prone_base,
        _ if walking => walk_base,
        _ => run_base,
    };
    events.push(PmEvent {
        event: base + mat,
        parm: 0,
    });
}

/// `PM_FootstepEvent`'s airborne branch: probe into the ladder face and use
/// its material, run-numbered; miss or material 0 defaults to metal (@0x32039).
fn ladder_step_event(
    ps: &PlayerState,
    world: &MoveWorld,
    old: u8,
    new: u8,
    events: &mut Vec<PmEvent>,
) {
    if !crossed_step_boundary(old, new) || !ps.on_ladder {
        return;
    }
    let mins = Vec3::new(-HALF_WIDTH + 6.0, -HALF_WIDTH + 6.0, 8.0);
    let maxs = Vec3::new(
        HALF_WIDTH - 6.0,
        HALF_WIDTH - 6.0,
        ps.stance.height().max(8.0),
    );
    let t = world.box_trace(
        ps.origin,
        ps.origin - ps.ladder_normal * LADDER_STEP_PROBE,
        mins,
        maxs,
        MASK_PLAYERSOLID,
    );
    let mut mat = crate::collision::sound_material(t.surface_flags);
    if t.fraction >= 1.0 || mat == 0 {
        mat = DEFAULT_MATERIAL;
    }
    events.push(PmEvent {
        event: EV_FOOTSTEP_RUN_BASE + mat,
        parm: 0,
    });
}

/// `PM_CrashLand` (0x2fd68): the fall height off the move's start, the fall
/// damage, and either the landing stun or the damage-free ladder
/// (docs/research/cod11-player-clip.md 8, cod11-sound-system.md "Landing").
/// `dead` is `pm_type > 5`, which lands with no damage.
fn crash_land(ps: &mut PlayerState, start_vz: f32, dead: bool, events: &mut Vec<PmEvent>) {
    if ps.water_level >= 3 {
        return;
    }
    // Retail solves the impact speed and squares it back over 2g; this is the
    // same height in closed form, and no landing at all when it has no root.
    let den = start_vz * start_vz + 2.0 * GRAVITY * (ps.move_start.z - ps.origin.z);
    if den < 0.0 {
        return;
    }
    let height = den / (2.0 * GRAVITY);
    let sf = ps.ground_surface_flags;
    let damage = if dead {
        0
    } else {
        fall_damage(height, sf, ps.water_level, ps.fall_heights)
    };
    if damage == 0 {
        if height >= LANDING_DAMP_HEIGHT {
            ps.velocity *= LANDING_DAMP;
        }
        events.extend(landing_event(height, ps));
        return;
    }
    // slick ground skips the stun (0x30013)
    if damage < 100 && !on_slick(ps) {
        let stun = (35 * damage + 500).min(2000);
        ps.knockback_ms = stun as f32;
        ps.knockback_flags |= PMF_TIME_KNOCKBACK;
        ps.velocity *= landing_stun_scale(stun);
    } else {
        ps.velocity *= LANDING_DAMP;
    }
    events.push(PmEvent {
        event: EV_LANDING_PAIN_BASE + landing_material(sf),
        parm: damage,
    });
}

/// The ground surface flag that takes no fall damage (0x2feaa).
const SURF_NODAMAGE: u32 = 0x1;
/// `EV_LANDING_PAIN_*`, one per surface material up to 138; the server's
/// `ClientEvents` turns each into fall damage.
pub const EV_LANDING_PAIN_BASE: i32 = 116;
pub const EV_LANDING_PAIN_LAST: i32 = 138;

/// The fall damage percent: linear from 0 at the min height to 100 at the
/// max, truncated, halved (truncated again) at water level 2. Bounds out of
/// order or a negative min take none (0x2fe6c).
fn fall_damage(height: f32, sf: u32, water_level: u32, h: FallHeights) -> i32 {
    let bad_bounds = h.max <= h.min || h.min < 0.0;
    let damage = if bad_bounds || height <= h.min || sf & SURF_NODAMAGE != 0 {
        0
    } else if height >= h.max {
        100
    } else {
        let frac = (height - h.min) / (h.max - h.min);
        ((frac * 100.0) as i32).clamp(0, 100)
    };
    if water_level == 2 {
        (damage as f32 * 0.5) as i32
    } else {
        damage
    }
}

/// The stun's velocity scale: 0.5 at 500 ms down to 0.2 at 1500 and past.
fn landing_stun_scale(stun_ms: i32) -> f32 {
    0.5 - (stun_ms - 500).clamp(0, 1000) as f32 / 1000.0 * 0.3
}

/// The fall height from which a landing plays the land event and damps the
/// velocity (rodata 0x70a18).
const LANDING_DAMP_HEIGHT: f32 = 12.0;
const LANDING_DAMP: f32 = 0.67;

/// The landing's surface index: 0, the default, on a no-sound surface.
fn landing_material(sf: u32) -> i32 {
    if sf & SURF_NO_SOUND != 0 {
        0
    } else {
        crate::collision::sound_material(sf)
    }
}

/// The damage-free ladder (0x30130): nothing at or under 4 units, a walk-step
/// to 8 and a run-step to 12, both silent on material 0, and from 12 the land
/// event, which plays the default on material 0 and carries the view bob.
fn landing_event(height: f32, ps: &PlayerState) -> Option<PmEvent> {
    let mat = landing_material(ps.ground_surface_flags);
    if height >= LANDING_DAMP_HEIGHT {
        // (h - 12) / 26 * 4 + 4, truncated, at most 24; 0 at exactly 12
        let bob = if height > LANDING_DAMP_HEIGHT {
            (((height - LANDING_DAMP_HEIGHT) / 26.0 * 4.0 + 4.0) as i32).min(24)
        } else {
            0
        };
        return Some(PmEvent {
            event: EV_LANDING_BASE + mat,
            parm: bob,
        });
    }
    let base = if height >= 8.0 {
        EV_FOOTSTEP_RUN_BASE
    } else if height > 4.0 {
        EV_FOOTSTEP_WALK_BASE
    } else {
        return None;
    };
    (mat != 0).then_some(PmEvent {
        event: base + mat,
        parm: 0,
    })
}

/// A CoD 1.1 spectator: fly accelerate, friction always, and no collision at
/// all. Axes are usercmd values scaled to -1..1.
///
/// The position integrates straight off the velocity, as Q3's `PM_NoclipMove`
/// does, rather than sliding against the world. This is a divergence from the
/// RTCW lineage, which sends `PM_SPECTATOR` to the colliding `PM_FlyMove`;
/// CoD 1.1 does not (docs/protocol-1.1.md, "Spectator").
pub fn spectator_move(ps: &mut PlayerState, forward: f32, right: f32, up: f32, dt: f32) {
    let dt = dt.min(MAX_FRAME_MS / 1000.0);
    ps.on_ground = false;
    let fwd = Vec3::new(ps.yaw.cos(), ps.yaw.sin(), 0.0);
    let rt = Vec3::new(ps.yaw.sin(), -ps.yaw.cos(), 0.0);
    let wishdir = (fwd * forward + rt * right + Vec3::Z * up).normalize_or_zero();
    // Q3's PM_SpectatorMove applies PM_Friction unconditionally with
    // `pm_spectatorfriction`; master's friction() gates ground friction on
    // `on_ground`, which a flier never sets, so apply it here directly.
    let speed = ps.velocity.length();
    if speed < 1.0 {
        ps.velocity.x = 0.0;
        ps.velocity.y = 0.0;
    } else {
        let control = speed.max(PM_STOPSPEED);
        let drop = control * PM_SPECTATOR_FRICTION * dt;
        ps.velocity *= ((speed - drop) / speed).max(0.0);
    }
    accelerate(ps, wishdir, SPEED_SPECTATOR, PM_ACCELERATE, dt);
    ps.origin += ps.velocity * dt;
}

/// Standing back up needs headroom for the taller bbox.
fn update_stance(ps: &mut PlayerState, input: &PmInput, world: &MoveWorld, dt: f32) {
    let before = ps.stance;
    let mut desired = if input.prone {
        Stance::Prone
    } else if input.crouch {
        Stance::Crouch
    } else {
        Stance::Stand
    };
    // Retail refuses a prone the body does not fit in, which is why a player
    // facing a wall stays standing.
    let entering_prone = desired == Stance::Prone && before != Stance::Prone;
    if entering_prone && !prone_fits(world, ps.origin, ps.yaw.to_degrees()) {
        desired = before;
    }
    // The dive flag lives as long as the prone key is held (`PM_CheckDuck`
    // 0x316f4 clears it on every other arm).
    if !input.prone || desired != Stance::Prone {
        ps.prone_dive = false;
    }
    if desired.height() <= ps.stance.height() {
        ps.stance = desired;
        if entering_prone && desired == Stance::Prone {
            enter_prone(ps, input, world);
        }
    } else {
        let maxs = Vec3::new(HALF_WIDTH, HALF_WIDTH, desired.height());
        let t = world.box_trace(ps.origin, ps.origin, ps.mins(), maxs, MASK_PLAYERSOLID);
        if !t.startsolid {
            ps.stance = desired;
        }
    }

    if ps.stance != before {
        match ps.stance {
            Stance::Crouch => ps.ducked = true,
            Stance::Stand => ps.ducked = false,
            Stance::Prone => {}
        }
    }
    // The bbox snaps; the eye eases.
    view_height_adjust(ps, (dt * 1000.0).round() as i32);
}

/// Retail's `PM_ViewHeightAdjust` (0x309d8), called at the end of
/// `PM_CheckDuck`: the eye walks a curve per leg, and a leg only ever spans
/// neighbouring stances, so standing to prone is two legs through the crouch
/// height (docs/research/cod11-mantle.md, "The eye through a stance change").
fn view_height_adjust(ps: &mut PlayerState, msec: i32) {
    let target = ps.stance.view_height();
    let mut pct = 0;
    if let Some(ms) = ps.view_lerp_ms.as_mut() {
        *ms += msec;
        pct = (*ms * 100 / view_lerp_duration(ps)).clamp(0, 100);
        if pct == 100 {
            ps.view_height_cur = ps.view_lerp_target;
            ps.view_lerp_ms = None;
        } else {
            ps.view_height_cur = view_curve_height(view_curve(ps), pct);
        }
    }
    if ps.view_lerp_ms.is_some() {
        if target == ps.view_lerp_target {
            return;
        }
        let reverses = if ps.view_lerp_down {
            target > ps.view_lerp_target
        } else {
            target < ps.view_lerp_target
        };
        if !reverses {
            return;
        }
        // Turn back mid-leg: the same stretch of the other leg's clock.
        pct = 100 - pct;
        ps.view_lerp_down = !ps.view_lerp_down;
        let t = ps.view_lerp_target;
        ps.view_lerp_target = match (ps.view_lerp_down, t) {
            (true, VIEW_STAND) => VIEW_CROUCH,
            (true, VIEW_CROUCH) => VIEW_PRONE,
            (false, VIEW_PRONE) => VIEW_CROUCH,
            (false, VIEW_CROUCH) => VIEW_STAND,
            _ => t,
        };
        if pct == 100 {
            ps.view_height_cur = ps.view_lerp_target;
            ps.view_lerp_ms = None;
        } else {
            // x87 product of the int percent, the float 0.01 at rodata
            // 0x70bcc and the int duration, truncated (0x312a5-0x312c6).
            let into = f64::from(pct) * f64::from(0.01f32) * f64::from(view_lerp_duration(ps));
            ps.view_lerp_ms = Some(into as i32);
        }
        return;
    }
    if target == ps.view_height_cur {
        return;
    }
    ps.view_lerp_ms = Some(0);
    let cur = ps.view_height_cur;
    (ps.view_lerp_down, ps.view_lerp_target) = match ps.stance {
        Stance::Prone => (
            true,
            if cur > VIEW_CROUCH {
                VIEW_CROUCH
            } else {
                VIEW_PRONE
            },
        ),
        Stance::Crouch => (cur > VIEW_CROUCH, VIEW_CROUCH),
        Stance::Stand => (
            false,
            if cur < VIEW_CROUCH {
                VIEW_CROUCH
            } else {
                VIEW_STAND
            },
        ),
    };
}

/// The running leg's length (the duration pick at 0x30b46-0x30b98, the same
/// as `PM_GetViewHeightLerpTime` 0x345b8).
fn view_lerp_duration(ps: &PlayerState) -> i32 {
    view_lerp_length(ps.view_lerp_target, ps.view_lerp_down, ps.prone_dive)
}

/// `PM_GetViewHeightLerpTime(ps, target, down)`, `dive` being `pm_flags` 0x4.
fn view_lerp_length(target: f32, down: bool, dive: bool) -> i32 {
    match (target, down, dive) {
        (VIEW_PRONE, _, true) => VIEW_LERP_DIVE_MS,
        (VIEW_PRONE, _, false) => VIEW_LERP_PRONE_MS,
        (VIEW_CROUCH, true, true) => VIEW_LERP_DIVE_DUCK_MS,
        (VIEW_CROUCH, true, false) => VIEW_LERP_DUCK_MS,
        (VIEW_CROUCH, false, _) => VIEW_LERP_PRONE_MS,
        _ => VIEW_LERP_STAND_MS,
    }
}

fn view_curve(ps: &PlayerState) -> &'static [(i32, f32)] {
    match (ps.view_lerp_target, ps.view_lerp_down, ps.prone_dive) {
        (VIEW_PRONE, _, true) => VIEW_CURVE_DIVE_PRONE,
        (VIEW_PRONE, _, false) => VIEW_CURVE_CROUCH_PRONE,
        (VIEW_CROUCH, true, _) => VIEW_CURVE_STAND_CROUCH,
        (VIEW_CROUCH, false, _) => VIEW_CURVE_PRONE_CROUCH,
        _ => VIEW_CURVE_CROUCH_STAND,
    }
}

/// The eye at `pct` of a leg: linear between the waypoints either side.
fn view_curve_height(curve: &[(i32, f32)], pct: i32) -> f32 {
    let Some(i) = curve.iter().position(|&(p, _)| p >= pct) else {
        return curve[0].1;
    };
    let (p1, h1) = curve[i];
    if p1 == pct || i == 0 {
        return h1;
    }
    let (p0, h0) = curve[i - 1];
    let t = f64::from(pct - p0) / f64::from(p1 - p0);
    (f64::from(h0) + t * f64::from(h1 - h0)) as f32
}

/// Retail's stance for the walk's scale and accel and the jump's gate, one
/// test inlined three times (0x2e7a1, 0x2f436, 0x2ebc8): the eye's leg counts
/// as much as the flags, so a player standing up out of prone keeps prone's
/// accel and cannot jump until the eye reaches the crouch height.
fn move_stance(ps: &PlayerState) -> Stance {
    let lerping = ps.view_lerp_ms.is_some();
    if ps.stance == Stance::Prone
        || ps.view_lerp_target == VIEW_PRONE
        || (lerping && ps.view_lerp_target == VIEW_CROUCH && !ps.view_lerp_down)
    {
        Stance::Prone
    } else if ps.ducked || (lerping && ps.view_lerp_target == VIEW_CROUCH) {
        Stance::Crouch
    } else {
        Stance::Stand
    }
}

/// How far the running leg has come from `from` toward `to`, 0..1, or 0 when
/// that is not the leg running (0x308cc).
fn view_lerp_frac(ps: &PlayerState, from: f32, to: f32) -> f32 {
    let Some(ms) = ps.view_lerp_ms else {
        return 0.0;
    };
    if to != ps.view_lerp_target {
        return 0.0;
    }
    let from_ok =
        (from == VIEW_PRONE && !ps.view_lerp_down) || (from == VIEW_STAND && ps.view_lerp_down);
    if to == VIEW_CROUCH && !from_ok {
        return 0.0;
    }
    (ms as f32 / view_lerp_duration(ps) as f32).clamp(0.0, 1.0)
}

/// The walk scale's stance factor (0x2e7a1-0x2e8d2): across a leg between
/// crouch and prone the two scales blend by the leg's progress, otherwise
/// [`move_stance`]'s own.
fn stance_speed_scale(ps: &PlayerState) -> f32 {
    let f = view_lerp_frac(ps, VIEW_CROUCH, VIEW_PRONE);
    if f != 0.0 {
        return f * SCALE_PRONE + (1.0 - f) * SCALE_CROUCH;
    }
    let f = view_lerp_frac(ps, VIEW_PRONE, VIEW_CROUCH);
    if f != 0.0 {
        return f * SCALE_CROUCH + (1.0 - f) * SCALE_PRONE;
    }
    move_stance(ps).speed_scale()
}

/// RTCW `bg_pmove.c` `PM_UpdateLean`. Differences: leans while moving (no
/// `!cmd->forwardmove` gate), and prone blocks leaning.
/// Whether a body may lie down at `origin` facing `yaw_deg`: retail sweeps a
/// 12-unit cube 54 units straight *backwards* from the facing, which is where
/// the body goes (`BG_CheckProneValid` 0x2d428, first trace at 0x2d57a). Not
/// modelled: the ground samples along the body that follow it on the ground,
/// which can refuse a bent body and write `fTorsoHeight`, `fTorsoPitch` and
/// `fWaistPitch`, and the partial clearance they accept.
pub fn prone_fits(world: &MoveWorld, origin: Vec3, yaw_deg: f32) -> bool {
    let back = (yaw_deg + 180.0).to_radians();
    let dir = Vec3::new(back.cos(), back.sin(), 0.0);
    // The cube's top at the prone height every caller passes (30).
    let start = origin + Vec3::Z * (HEIGHT_PRONE - PRONE_BODY_HALF_BOX);
    let end = start + dir * PRONE_BODY_LENGTH;
    let half = Vec3::splat(PRONE_BODY_HALF_BOX);
    let t = world.box_trace(start, end, -half, half, MASK_DEADSOLID);
    !t.startsolid && t.fraction >= 1.0
}

/// Degrees folded to -180..180, the `AngleNormalize180` the prone code runs
/// its yaw difference through.
fn normalize180(deg: f32) -> f32 {
    (deg + 180.0).rem_euclid(360.0) - 180.0
}

/// The frame a player goes prone, the tail of `PM_CheckDuck`'s prone arm
/// (0x31ca9-0x31f50): a press on a player moving forward or back throws it
/// into the air, the body takes the view's yaw, and both prone pitches start
/// from the ground under it (docs/research/cod11-mantle.md, "Prone").
fn enter_prone(ps: &mut PlayerState, input: &PmInput, world: &MoveWorld) {
    // Retail tests the ADS flag here too, but a move clears it first, so a
    // forward or back cmd always dives.
    if input.forward != 0.0 {
        ps.prone_dive = true;
        if ps.on_ground {
            let height = if ps.ducked {
                DIVE_HEIGHT_LOW
            } else {
                DIVE_HEIGHT_STAND
            };
            ps.velocity.z = (2.0 * height * GRAVITY).sqrt();
            ps.on_ground = false;
            ps.ground_plane = None;
        }
        ps.aim_spread_scale = 255.0;
    }
    let view_yaw = ps.yaw.to_degrees();
    ps.prone_direction = normalize180(view_yaw);
    let t = world.box_trace(
        ps.origin,
        ps.origin - Vec3::Z * 0.25,
        ps.mins(),
        ps.maxs(),
        MASK_PLAYERSOLID,
    );
    ps.prone_direction_pitch = if t.fraction < 1.0 && !t.startsolid {
        pitch_for_yaw_on_normal(ps.prone_direction, t.normal)
    } else {
        0.0
    };
    // The torso starts on the ground's pitch, but no further than the cap
    // from the view, so lying down never yanks the view.
    let view_pitch = -ps.pitch.to_degrees();
    let d = angle_delta(ps.prone_direction_pitch, view_pitch);
    ps.prone_torso_pitch = if d < -PRONE_PITCHCAP {
        view_pitch - PRONE_PITCHCAP
    } else if d > PRONE_PITCHCAP {
        view_pitch + PRONE_PITCHCAP
    } else {
        ps.prone_direction_pitch
    };
}

/// The pitch, wire convention (positive down), of the yaw's direction laid
/// on a plane: `PitchForYawOnNormal` (0x3d274), in 0..360.
fn pitch_for_yaw_on_normal(yaw_deg: f32, normal: Vec3) -> f32 {
    let (sin, cos) = yaw_deg.to_radians().sin_cos();
    let fwd = Vec3::new(cos, sin, 0.0);
    let inv = 1.0 / normal.length_squared();
    let p = fwd - normal * (fwd.dot(normal) * inv * inv);
    if p.x == 0.0 && p.y == 0.0 {
        return if p.z > 0.0 { 270.0 } else { 90.0 };
    }
    let pitch = -p.z.atan2(p.truncate().length()).to_degrees();
    if pitch < 0.0 { pitch + 360.0 } else { pitch }
}

/// The prone half of `PM_UpdateViewAngles` (0x32d7c): the body swinging to
/// follow the view, the view capped to the yaw cone around the body and
/// pitched no further than the cap off `proneTorsoPitch`. Retail enforces
/// both caps by pushing `delta_angles`, so the corrections are reported here
/// and the caller applies them to whatever owns the view
/// (docs/research/cod11-mantle.md, "Prone").
fn update_prone_view(ps: &mut PlayerState, input: &PmInput, world: &MoveWorld, dt: f32) {
    if ps.stance != Stance::Prone {
        return;
    }
    let view = ps.yaw.to_degrees();
    let delta = normalize180(ps.prone_direction - view);
    // The body turns past the soft edge, and inside it whenever the player
    // moves (0x330bd), and only into a direction it still fits in.
    let moving = input.forward != 0.0 || input.right != 0.0;
    if delta.abs() > PRONE_SOFT_EDGE || (moving && delta != 0.0) {
        let step = PRONE_SWING_DEG_PER_SEC * dt;
        let candidate = if step > delta.abs() {
            view
        } else if delta > 0.0 {
            ps.prone_direction - step
        } else {
            ps.prone_direction + step
        };
        if prone_fits(world, ps.origin, candidate) {
            ps.prone_direction = candidate;
        }
    }
    // The cap measures the excess before the swing and places the view
    // after it (0x331c8-0x33235).
    if delta.abs() > PRONE_YAWCAP {
        ps.view_yaw_correction = delta - PRONE_YAWCAP.copysign(delta);
        ps.yaw = (ps.prone_direction - PRONE_YAWCAP.copysign(delta)).to_radians();
    }
    let view_pitch = -ps.pitch.to_degrees();
    let d = angle_delta(ps.prone_torso_pitch, view_pitch);
    if d.abs() > PRONE_PITCHCAP {
        ps.view_pitch_correction = d - PRONE_PITCHCAP.copysign(d);
        ps.pitch = -normalize180(ps.prone_torso_pitch - PRONE_PITCHCAP.copysign(d)).to_radians();
    }
}

/// `PM_UpdatePronePitch` (0x3338c): both prone pitches ease toward the
/// ground's pitch under the body and under the view, or toward level with no
/// ground plane under the player. Not modelled: its refusal event (141) and
/// `pm_flags` 0x8000, which no capture has raised.
fn update_prone_pitch(ps: &mut PlayerState, dt: f32) {
    if ps.stance != Stance::Prone {
        return;
    }
    let rate = PRONE_PITCH_DEG_PER_SEC * dt;
    let ease = |cur: f32, target: f32| {
        let d = angle_delta(target, cur);
        if d == 0.0 {
            return cur;
        }
        normalize180(cur + d.clamp(-rate, rate))
    };
    let (body, view) = match ps.ground_plane {
        Some(n) => (
            pitch_for_yaw_on_normal(ps.prone_direction, n),
            pitch_for_yaw_on_normal(ps.yaw.to_degrees(), n),
        ),
        None => (0.0, 0.0),
    };
    ps.prone_direction_pitch = ease(ps.prone_direction_pitch, body);
    ps.prone_torso_pitch = ease(ps.prone_torso_pitch, view);
}

fn update_lean(ps: &mut PlayerState, input: &PmInput, world: &MoveWorld, dt: f32) {
    if !update_lean_unclamped(ps, input, dt) {
        return;
    }

    // wall clamp with RTCW's lean box
    let start = ps.origin + Vec3::Z * ps.view_height();
    let mut right = Vec3::new(ps.yaw.sin(), -ps.yaw.cos(), 0.0);
    right.z = if ps.lean < 0.0 { 0.25 } else { -0.25 };
    let end = start + right * ps.lean;
    let t = world.box_trace(
        start,
        end,
        Vec3::new(-12.0, -12.0, -6.0),
        Vec3::new(12.0, 12.0, 10.0),
        MASK_PLAYERSOLID,
    );
    ps.lean *= t.fraction;
}

/// The lean's ramp without the wall clamp; true when a lean key drove it.
fn update_lean_unclamped(ps: &mut PlayerState, input: &PmInput, dt: f32) -> bool {
    let msec = dt * 1000.0;
    let mut dir = 0.0f32;
    if input.lean_left {
        dir -= 1.0;
    }
    if input.lean_right {
        dir += 1.0;
    }
    if ps.stance == Stance::Prone {
        dir = 0.0;
    }

    if dir == 0.0 {
        // return to center
        let step = msec / LEAN_TIME_FROM_MS * LEAN_MAX;
        ps.lean = if ps.lean > 0.0 {
            (ps.lean - step).max(0.0)
        } else {
            (ps.lean + step).min(0.0)
        };
        return false;
    }
    ps.lean = (ps.lean + dir * (msec / LEAN_TIME_TO_MS) * LEAN_MAX).clamp(-LEAN_MAX, LEAN_MAX);
    true
}

/// Q3 `bg_pmove.c` `PM_GroundTrace`'s kickoff test: the player's own velocity
/// is carrying it off the plane it is standing on. A plain `velocity.z <= 0.0`
/// test fails in its place: OVERCLIP leaves a hair of positive z after a floor
/// clip and the player would stay airborne.
fn thrown_off_ground(ps: &PlayerState, normal: Vec3) -> bool {
    ps.velocity.z > 0.0 && ps.velocity.dot(normal) > 10.0
}

/// Q3 `bg_pmove.c` `PM_GroundTrace`.
fn ground_trace(ps: &mut PlayerState, world: &MoveWorld, mask: u32) {
    let t = world.box_trace(
        ps.origin,
        ps.origin - Vec3::Z * 0.25,
        ps.mins(),
        ps.maxs(),
        mask,
    );
    // Any hit ends the jump's step allowance, thrown off or not (0x305c8).
    if t.fraction < 1.0 {
        ps.jump_origin_z = 0.0;
    }
    let thrown_off = thrown_off_ground(ps, t.normal);
    ps.ground_plane = (t.fraction < 1.0 && !thrown_off).then_some(t.normal);
    if t.fraction < 1.0 && t.normal.z >= MIN_WALK_NORMAL && !thrown_off {
        ps.on_ground = true;
        // `groundEntityNum` is the trace's own entity (0x30732).
        ps.ground_entity = world.entity_num(&t);
        ps.ground_normal = t.normal;
        ps.ground_surface_flags = t.surface_flags;
        // RTCW clears the waterjump lock on touching walkable ground
        ps.waterjump_ms = 0.0;
    } else {
        ps.on_ground = false;
        ps.ground_normal = Vec3::Z;
        ps.ground_surface_flags = 0;
    }
}

/// Feet, waist and eye point-contents samples; swimming starts at waist-deep.
fn set_water_level(ps: &mut PlayerState, world: &MoveWorld) {
    use crate::collision::CONTENTS_WATER;
    ps.water_level = 0;
    let o = ps.origin;
    if world.point_contents(Vec3::new(o.x, o.y, o.z + 1.0)) & CONTENTS_WATER == 0 {
        return;
    }
    ps.water_level = 1;
    let vh = ps.view_height();
    let at = |z: f32| world.point_contents(Vec3::new(o.x, o.y, z)) & CONTENTS_WATER != 0;
    if at(o.z + vh * 0.5) {
        ps.water_level = 2;
        if at(o.z + vh) {
            ps.water_level = 3;
        }
    }
}

/// The ground trace hit a `surfaceparm slick` material (pml+0x50 bit 0x2).
fn on_slick(ps: &PlayerState) -> bool {
    ps.ground_surface_flags & crate::collision::SURF_SLICK != 0
}

/// RTCW `bg_pmove.c` `PM_Friction`: ground friction only while walking in
/// water level <= 1, plus a water term that already applies while wading, and
/// the ladder term whenever on a ladder.
fn friction(ps: &mut PlayerState, on_ladder: bool, dt: f32) {
    // when walking, slope movement along z does not count toward the speed
    let mut planar = ps.velocity;
    if ps.on_ground {
        planar.z = 0.0;
    }
    let speed = planar.length();
    if speed < 1.0 {
        ps.velocity.x = 0.0;
        ps.velocity.y = 0.0;
        return;
    }
    let mut drop = 0.0;
    // Slick ground and a hit's knockback slide free of the ground term
    // (0x2e4ed, 0x2e4fb).
    if ps.on_ground
        && ps.water_level <= 1
        && !on_slick(ps)
        && ps.knockback_flags & PMF_TIME_DAMAGE == 0
    {
        let mut control = speed.max(PM_STOPSPEED);
        if ps.knockback_flags & PMF_TIME_KNOCKBACK != 0 {
            control *= KNOCKBACK_FRICTION_SCALE;
        }
        drop += control * PM_FRICTION * dt;
    }
    if ps.water_level > 0 {
        drop += speed * WATER_FRICTION * ps.water_level as f32 * dt;
    }
    if on_ladder {
        drop += speed * PM_LADDER_FRICTION * dt;
    }
    let scale = ((speed - drop) / speed).max(0.0);
    ps.velocity *= scale;
}

/// Desired direction (world space, unit or zero) and speed.
/// The walk mover's wish direction and speed, retail's `PM_CmdScale`
/// (game.mp.i386.so 0x2e690, called from `PM_WalkMove` at 0x2f2bf): Q3's
/// `speed * max / (127 * total)` over the cmd bytes, with a backpedal read
/// through `backSpeedScale` and a strafe through `strafeSpeedScale` before
/// the max is taken, then `walkSpeedScale` on the ADS walk and otherwise
/// `runSpeedScale` with `leanSpeedScale` on a lean, the stance scale
/// ([`stance_speed_scale`]), the wade scale and the weapon's `moveSpeedScale`. The cmd magnitude then
/// multiplies back in, so a lone full key wishes `SPEED_RUN` and a diagonal
/// wishes no more (docs/research/cod11-mantle.md, "The wish speed").
///
/// `walk_slow` is the client's own fly-mode key and takes the walk scale.
/// Not ported: the `wbuttons` 0x4 factor (0.4, rodata 0x70894), which no
/// measured key sets.
fn wish(ps: &PlayerState, input: &PmInput, weapon: Option<&WeaponDef>) -> (Vec3, f32) {
    let (f, r) = (input.forward * 127.0, input.right * 127.0);
    let max = if f < 0.0 { -f * SCALE_BACK } else { f }.max(r.abs() * SCALE_STRAFE);
    if max <= 0.0 {
        return (Vec3::ZERO, 0.0);
    }
    let total = (f * f + r * r).sqrt();
    let mut scale = SPEED_RUN * max / (127.0 * total);
    if ps.walking || input.walk_slow {
        scale *= SCALE_WALK;
    } else if ps.lean != 0.0 {
        scale *= SCALE_LEAN;
    }
    scale *= stance_speed_scale(ps);
    scale *= 1.0 - ps.water_level as f32 / 3.0 * WADE_SCALE;
    if let Some(w) = weapon.filter(|w| w.move_speed_scale > 0.0) {
        scale *= w.move_speed_scale;
    }
    let fwd = Vec3::new(ps.yaw.cos(), ps.yaw.sin(), 0.0);
    let right = Vec3::new(ps.yaw.sin(), -ps.yaw.cos(), 0.0);
    let wishvel = fwd * f + right * r;
    (wishvel.normalize_or_zero(), wishvel.length() * scale)
}

/// The air mover's wish, retail's Q3-shaped scale (0x2e5bc, called from
/// `PM_AirMove` at 0x2f083): the magnitude rule over forward, right and up,
/// and the walk/run pick, with no stance, lean, weapon or wade factor. A
/// held jump or a held stance key puts 127 in `upmove`, and that dilutes the
/// horizontal wish the way Q3's does.
fn wish_air(ps: &PlayerState, input: &PmInput) -> (Vec3, f32) {
    let (f, r) = (input.forward * 127.0, input.right * 127.0);
    let u = if input.jump || input.crouch || input.prone {
        127.0
    } else {
        0.0
    };
    let max = f.abs().max(r.abs()).max(u);
    if max <= 0.0 {
        return (Vec3::ZERO, 0.0);
    }
    let total = (f * f + r * r + u * u).sqrt();
    let mut scale = SPEED_RUN * max / (127.0 * total);
    if ps.walking || input.walk_slow {
        scale *= SCALE_WALK;
    }
    let fwd = Vec3::new(ps.yaw.cos(), ps.yaw.sin(), 0.0);
    let right = Vec3::new(ps.yaw.sin(), -ps.yaw.cos(), 0.0);
    let wishvel = fwd * f + right * r;
    (wishvel.normalize_or_zero(), wishvel.length() * scale)
}

/// Q3 `bg_pmove.c` `PM_Accelerate`.
fn accelerate(ps: &mut PlayerState, wishdir: Vec3, wishspeed: f32, accel: f32, dt: f32) {
    let current = ps.velocity.dot(wishdir);
    let add = wishspeed - current;
    if add <= 0.0 {
        return;
    }
    let accel_speed = (accel * dt * wishspeed).min(add);
    ps.velocity += wishdir * accel_speed;
}

/// `vectoyaw`: the yaw of a direction, in degrees.
fn yaw_of(v: Vec3) -> f32 {
    v.y.atan2(v.x).to_degrees()
}

/// `AngleDelta`: `a - b` folded to -180..180.
fn angle_delta(a: f32, b: f32) -> f32 {
    normalize180(a - b)
}

fn clamp_movement_dir(deg: i32, cap: i32) -> i32 {
    if deg > cap {
        cap
    } else if deg < -cap {
        -cap
    } else {
        deg
    }
}

/// `PM_SetMovementDir` @0x2e970, which retail runs at the tail of both
/// `PM_WalkMove` and `PM_AirMove`. The angle comes off the frame's
/// displacement rather than off the velocity, so a player scraping along a
/// wall turns its legs with the slide. Every intermediate is truncated to a
/// whole degree, as retail's `(int)` casts are.
fn set_movement_dir(ps: &mut PlayerState, input: &PmInput, dt: f32) {
    // Prone lays the legs along the body instead (@0x2e98d). Retail skips
    // this branch while the view is locked to another entity, eFlags
    // 0xc000; `ps.mounted` carries that state here.
    if ps.stance == Stance::Prone && ps.mounted.is_none() {
        ps.movement_dir = clamp_movement_dir(
            angle_delta(ps.prone_direction, ps.yaw.to_degrees()) as i32,
            MOVEMENT_DIR_CAP,
        );
        return;
    }
    // Retail has a ladder branch here too (@0x2e9f6), for the frames its
    // pm_flags ladder bit outlives its ladder mover; ours cannot, since
    // `on_ladder` is set by the same test that picks `ladder_move`.
    let moved = ps.origin - ps.move_start;
    // Airborne, no move key, or barely moved: the legs face the view. The
    // threshold is 5 units per second of frametime (@0x2ea4b-@0x2eaa7).
    if (input.forward == 0.0 && input.right == 0.0) || !ps.on_ground || moved.length() <= dt * 5.0 {
        ps.movement_dir = 0;
        return;
    }
    let mut deg = angle_delta(yaw_of(moved), ps.yaw.to_degrees()) as i32;
    if input.forward < 0.0 {
        deg = normalize180(deg as f32 + 180.0) as i32;
    }
    ps.movement_dir = clamp_movement_dir(deg, MOVEMENT_DIR_CAP);
}

/// Q3 `bg_pmove.c` `PM_ClipVelocity`.
fn clip_velocity(vel: Vec3, normal: Vec3) -> Vec3 {
    let backoff = vel.dot(normal)
        * if vel.dot(normal) < 0.0 {
            OVERCLIP
        } else {
            1.0 / OVERCLIP
        };
    vel - normal * backoff
}

/// Q3 `bg_pmove.c` `PM_WalkMove`.
fn walk_move(
    ps: &mut PlayerState,
    input: &PmInput,
    weapon: Option<&WeaponDef>,
    world: &MoveWorld,
    dt: f32,
    mask: u32,
    events: Option<&mut Vec<PmEvent>>,
) {
    // eye-deep and looking up an upward slope: swim instead of trudging
    if ps.water_level > 2 && forward3(ps).dot(ps.ground_normal) > 0.0 {
        water_move(ps, input, world, dt, mask, events);
        return;
    }
    let (dir, wishspeed) = wish(ps, input, weapon);
    // Slick ground or a hit's knockback (0x2f492, 0x2f58c).
    let sliding = on_slick(ps) || ps.knockback_flags & PMF_TIME_DAMAGE != 0;
    let mut accel = match move_stance(ps) {
        _ if sliding => 1.0,
        Stance::Stand => PM_ACCELERATE,
        Stance::Crouch => PM_DUCKED_ACCELERATE,
        Stance::Prone => PM_PRONE_ACCELERATE,
    };
    if ps.knockback_flags & PMF_TIME_KNOCKBACK != 0 {
        accel *= KNOCKBACK_ACCEL_SCALE;
    }
    // along the slope, so it costs no speed
    let dir = clip_velocity(dir, ps.ground_normal).normalize_or_zero();
    // Q3's `PM_Accelerate` inline, with the rate floored: a prone or
    // sighted wish of under 100 still gains 100's worth per frame, capped
    // at the wish (docs/research/cod11-mantle.md, "The walk's accel floor").
    let current = ps.velocity.dot(dir);
    let add = wishspeed - current;
    if add > 0.0 {
        ps.velocity += dir * (accel * dt * wishspeed.max(WALK_ACCEL_FLOOR)).min(add);
    }
    // Q3's slick-or-knockback gravity, which the clip below turns into
    // ground speed.
    if sliding {
        ps.velocity.z -= GRAVITY * dt;
    }
    // The clip onto the ground keeps the speed whenever it leaves the
    // velocity pointing the same way (0x2f5b8-0x2f6b3), so a landing that
    // still carries its fall turns it into ground speed.
    let (before, speed) = (ps.velocity, ps.velocity.length());
    ps.velocity = clip_velocity(ps.velocity, ps.ground_normal);
    if ps.velocity.dot(before) > 0.0 {
        ps.velocity = ps.velocity.normalize_or_zero() * speed;
    }
    // Standing still skips the move but not the legs: retail jumps straight
    // to PM_SetMovementDir (@0x2f6db), which is what keeps a prone player's
    // legs following its body while it turns on the spot.
    if ps.velocity.x != 0.0 || ps.velocity.y != 0.0 {
        step_slide_move(ps, world, dt, false, mask, events);
    }
    set_movement_dir(ps, input, dt);
}

/// Look direction including pitch; pitch is up-positive.
fn forward3(ps: &PlayerState) -> Vec3 {
    Vec3::new(
        ps.pitch.cos() * ps.yaw.cos(),
        ps.pitch.cos() * ps.yaw.sin(),
        ps.pitch.sin(),
    )
}

/// Q3 `bg_pmove.c` `PM_CmdScale`: input magnitude in command units. Q3 only
/// reads forward/right here, so a lone up key would scale to zero and the
/// sink wish would win; CoD-style play expects jump alone to swim up, so up
/// joins the magnitude.
fn cmd_scale(input: &PmInput) -> f32 {
    let up = if input.jump { 1.0 } else { 0.0 };
    let max = input.forward.abs().max(input.right.abs()).max(up);
    if max <= 0.0 {
        return 0.0;
    }
    let total = (input.forward * input.forward + input.right * input.right + up * up).sqrt();
    total / max * (127.0 / 128.0)
}

/// RTCW `bg_pmove.c` `PM_WaterMove`. No gravity: buoyancy is implicit, the
/// idle sink is a wish toward the bottom, jump is the up command.
fn water_move(
    ps: &mut PlayerState,
    input: &PmInput,
    world: &MoveWorld,
    dt: f32,
    mask: u32,
    events: Option<&mut Vec<PmEvent>>,
) {
    if try_start_water_jump(ps, world) {
        water_jump_move(ps, world, dt, mask, events);
        return;
    }
    friction(ps, ps.on_ladder, dt);
    let m = cmd_scale(input) * 127.0;
    let right = Vec3::new(ps.yaw.sin(), -ps.yaw.cos(), 0.0);
    let wishvel = if m == 0.0 {
        Vec3::new(0.0, 0.0, -WATER_SINK_SPEED)
    } else {
        let mut w = forward3(ps) * (input.forward * m) + right * (input.right * m);
        w.z += if input.jump { m } else { 0.0 };
        w
    };
    let wishspeed = wishvel.length();
    let wishdir = if wishspeed > 0.0 {
        wishvel / wishspeed
    } else {
        Vec3::ZERO
    };
    let cap = SPEED_RUN * SCALE_SWIM;
    let wishspeed = if wishspeed > cap { cap } else { wishspeed };
    accelerate(ps, wishdir, wishspeed, WATER_ACCELERATE, dt);

    // crawl up underwater slopes without losing speed. RTCW also re-scales
    // the clipped velocity to its old length here, which mirrors a full sink
    // into the floor into a full-power launch the frame grounding starts
    // (their "FIXME: still have z friction underwater?" marks this spot);
    // dropping the re-scale keeps bottom contact settled.
    if ps.on_ground && ps.velocity.dot(ps.ground_normal) < 0.0 {
        ps.velocity = clip_velocity(ps.velocity, ps.ground_normal);
    }
    slide_move(ps, world, dt, false, mask);
}

/// RTCW `bg_pmove.c` `PM_CheckWaterJump`: chest-deep against a low lip, the
/// probe 4 units up must hit solid and 20 units up must be clear.
fn try_start_water_jump(ps: &mut PlayerState, world: &MoveWorld) -> bool {
    use crate::collision::CONTENTS_SOLID;
    if ps.waterjump_ms > 0.0 || ps.water_level != 2 {
        return false;
    }
    let flat = Vec3::new(ps.yaw.cos(), ps.yaw.sin(), 0.0);
    let spot = ps.origin + flat * 30.0 + Vec3::Z * 4.0;
    if world.point_contents(spot) & CONTENTS_SOLID == 0 {
        return false;
    }
    if world.point_contents(spot + Vec3::Z * 16.0) != 0 {
        return false;
    }
    ps.velocity = forward3(ps) * WATERJUMP_FORWARD;
    ps.velocity.z = WATERJUMP_UP;
    ps.waterjump_ms = WATERJUMP_TIME_MS;
    true
}

/// RTCW `bg_pmove.c` `PM_WaterJumpMove`: no control, extra gravity, cancels
/// once falling again (landing clears via ground_trace).
fn water_jump_move(
    ps: &mut PlayerState,
    world: &MoveWorld,
    dt: f32,
    mask: u32,
    events: Option<&mut Vec<PmEvent>>,
) {
    step_slide_move(ps, world, dt, true, mask, events);
    ps.velocity.z -= GRAVITY * dt;
    if ps.velocity.z < 0.0 {
        ps.waterjump_ms = 0.0;
    }
}

/// Retail `PM_CheckLadderMove` (game.mp.i386.so @0x336e8): reach 30/8,
/// forwardmove gate while walking, probe bbox shrunk 6 per horizontal side
/// with the top lowered by the probe distance (@0x70cb0), direction
/// `-vLadderVec` when already on a ladder and airborne. Sets `ps.on_ladder`
/// and `ps.ladder_normal`; returns the ladder plane normal plus whether this
/// frame is a grab-from-above push (RTCW's guard, kept from the port).
fn check_ladder_move(
    ps: &mut PlayerState,
    input: &PmInput,
    world: &MoveWorld,
) -> Option<(Vec3, bool)> {
    if ps.waterjump_ms > 0.0 {
        ps.on_ladder = false;
        return None;
    }
    // A running knockback or landing-stun timer skips the check and keeps
    // last frame's ladder flag (`pm_time` test at 0x336f6).
    if ps.knockback_ms > 0.0 {
        return ps.on_ladder.then_some((ps.ladder_normal, false));
    }
    // skip detection within pm_ladderJumpTime of a jump (@0x33822): this
    // is what makes a push-off actually leave the wall
    if ps.since_jump_ms < LADDER_REGRAB_LOCK_MS {
        ps.on_ladder = false;
        return None;
    }
    let walking = ps.on_ground;
    let tracedist = if walking {
        LADDER_TRACE_DIST_WALK
    } else {
        LADDER_TRACE_DIST_AIR
    };
    // stick with the wall we left instead of requiring facing it
    let dir = if ps.on_ladder && !walking && ps.ladder_normal != Vec3::ZERO {
        -ps.ladder_normal
    } else {
        Vec3::new(ps.yaw.cos(), ps.yaw.sin(), 0.0).normalize_or_zero()
    };
    let mut probe_mins = ps.mins();
    probe_mins.x += LADDER_PROBE_SHRINK;
    probe_mins.y += LADDER_PROBE_SHRINK;
    let mut probe_maxs = ps.maxs();
    probe_maxs.x -= LADDER_PROBE_SHRINK;
    probe_maxs.y -= LADDER_PROBE_SHRINK;
    probe_maxs.z -= tracedist;
    let t = world.box_trace(
        ps.origin,
        ps.origin + dir * tracedist,
        probe_mins,
        probe_maxs,
        MASK_PLAYERSOLID,
    );
    let mut ladder = t.fraction < 1.0 && t.surface_flags & crate::collision::SURF_LADDER != 0;
    let normal = t.normal;
    let mut ladderforward = false;
    if ladder && !walking && t.fraction * tracedist > 1.0 {
        // grab-from-above guard: only trust a far hit when a backwards trace
        // confirms the wall behind us, else it would fling us off the top
        ladder = false;
        let back = world.box_trace(
            ps.origin,
            ps.origin - normal * tracedist,
            probe_mins,
            probe_maxs,
            MASK_PLAYERSOLID,
        );
        if back.fraction < 1.0 && back.surface_flags & crate::collision::SURF_LADDER != 0 {
            ladder = true;
            ladderforward = true;
        }
    }
    // standing at the base only climbs while pushing forward
    if ladder && walking && input.forward <= 0.0 {
        ladder = false;
    }
    ps.on_ladder = ladder;
    if ladder {
        ps.ladder_normal = normal;
    }
    ladder.then_some((normal, ladderforward))
}

/// Retail `PM_CheckJump` (0x2eb98), called from `PM_WalkMove` and
/// `PM_LadderMove`; `ladder` is the ladder plane when the latter. Refused
/// inside 500 ms of the last jump, on a held key and to anything but a
/// standing player. vz = sqrt(g * 78) with horizontal velocity kept, and
/// `fJumpOriginZ` 39 up. Off a ladder, vz is scaled 0.75 and the horizontal
/// reset to 128 along the forward, reflected off the plane while facing it.
/// A held key refused here is taken off the cmd for the rest of the move, as
/// retail zeroes `cmd.upmove` (0x2ec13), which the ladder's wish reads. Not
/// modelled: the `PMF_RESPAWNED` gate (0x800), which the spawn's own think
/// clears before any cmd of a live player (docs/research/cod11-mantle.md,
/// "Jumps").
fn check_jump(
    ps: &mut PlayerState,
    input: &mut PmInput,
    ladder: Option<Vec3>,
    events: &mut Vec<PmEvent>,
) -> bool {
    if ps.since_jump_ms <= JUMP_COOLDOWN_MS - 1.0 || move_stance(ps) != Stance::Stand || !input.jump
    {
        return false;
    }
    if ps.jump_latched {
        input.jump = false;
        return false;
    }
    ps.on_ground = false;
    ps.ground_plane = None;
    ps.jump_latched = true;
    ps.velocity.z = (2.0 * JUMP_HEIGHT * GRAVITY).sqrt();
    ps.jump_origin_z = ps.origin.z + JUMP_HEIGHT;
    if let Some(normal) = ladder {
        ps.velocity.z *= 0.75;
        let f = Vec3::new(ps.yaw.cos(), ps.yaw.sin(), 0.0).normalize_or_zero();
        // reflection only when looking into the wall (n.forward < 0, @0x2ecd5)
        let dir = if normal.dot(forward3(ps)) < 0.0 {
            let d2 = f.x * normal.x + f.y * normal.y;
            (f - normal * (2.0 * d2)).normalize_or_zero()
        } else {
            f
        };
        ps.velocity.x = dir.x * LADDER_PUSHOFF_SPEED;
        ps.velocity.y = dir.y * LADDER_PUSHOFF_SPEED;
        ps.on_ladder = false;
    }
    // 70 + material off the last ground trace; material 0 or the silent
    // surfaceparm emits nothing (@0x2ed90-0x2edb9).
    let sf = ps.ground_surface_flags;
    let mat = crate::collision::sound_material(sf);
    if mat != 0 && sf & SURF_NO_SOUND == 0 {
        events.push(PmEvent {
            event: EV_JUMP_BASE + mat,
            parm: 0,
        });
    }
    ps.aim_spread_scale = (ps.aim_spread_scale + JUMP_SPREAD_ADD).min(JUMP_SPREAD_MAX);
    ps.jumped = true;
    true
}

/// RTCW `bg_pmove.c` `PM_LadderMove` with retail CoD 1.1 coefficients (const
/// provenance above). Pitch drives the climb rate; strafe slides along the
/// ladder face.
fn ladder_move(
    ps: &mut PlayerState,
    input: &PmInput,
    normal: Vec3,
    ladderforward: bool,
    world: &MoveWorld,
    dt: f32,
    events: &mut Vec<PmEvent>,
) {
    // retail tries the push-off first; a jump runs the normal mover this
    // frame and stamps jumpTime (@0x3394c-0x33964)
    let mut cmd = *input;
    if check_jump(ps, &mut cmd, Some(normal), events) {
        air_move(ps, &cmd, world, dt, MASK_PLAYERSOLID, Some(events));
        ps.since_jump_ms = 0.0;
        return;
    }
    let input = &cmd;

    if ladderforward {
        let push = -LADDER_PUSH_SPEED;
        ps.velocity.x = normal.x * push;
        ps.velocity.y = normal.y * push;
    }

    let fwd = forward3(ps);
    let upscale = ((fwd.z + LADDER_UPSCALE_BIAS) * LADDER_UPSCALE_GAIN).clamp(-1.0, 1.0);
    let flat_right = Vec3::new(ps.yaw.sin(), -ps.yaw.cos(), 0.0);
    // VERIFIED (game.mp.i386.so PM_LadderMove @0x339fa):
    // ProjectPointOnPlane(pml.right, flat_right, ps->vLadderVec @ps+0x58), so
    // strafing slides along the face instead of into or off it
    let tangent_right = flat_right - normal * flat_right.dot(normal);

    // VERIFIED (call @0x33a0b -> PM_CmdShape fn @0x2e5bc): retail multiplies
    // both terms by speed * max / (127 * total) * stance scale, so combined
    // inputs shrink each axis; here normalized to SPEED_RUN for a lone full key
    let up = if input.jump { 1.0 } else { 0.0 };
    let max = input.forward.abs().max(input.right.abs()).max(up);
    let total = (input.forward * input.forward + input.right * input.right + up * up).sqrt();
    let mag = if max <= 0.0 {
        0.0
    } else {
        SPEED_RUN * max / total
    };

    let mut wishvel = tangent_right * (LADDER_STRAFE_SCALE * mag * input.right);
    wishvel.z = LADDER_CLIMB_SCALE * upscale * mag * input.forward;

    friction(ps, true, dt);
    let wishspeed = wishvel.length().min(LADDER_WISHSPEED_CAP);
    let wishdir = if wishspeed > 0.0 {
        wishvel / wishspeed
    } else {
        Vec3::ZERO
    };
    accelerate(ps, wishdir, wishspeed, LADDER_ACCELERATE, dt);
    if wishvel.z == 0.0 {
        // no climb input: vertical velocity decays toward zero instead of
        // falling (RTCW)
        if ps.velocity.z > 0.0 {
            ps.velocity.z = (ps.velocity.z - GRAVITY * dt).max(0.0);
        } else {
            ps.velocity.z = (ps.velocity.z + GRAVITY * dt).min(0.0);
        }
    }
    // airborne wall glue (@0x33cf1, gated on pml.walking == 0): strip
    // velocity along the ladder normal and press back into the wall - 500
    // holding forward, else 250 (@0x70cd4/0x70cd8). This is retail's sticky
    // grip, not a dismount hop; leaving happens via the push-off above.
    if !ps.on_ground {
        let n = Vec3::new(normal.x, normal.y, 0.0);
        let d = ps.velocity.x * n.x + ps.velocity.y * n.y;
        ps.velocity.x -= d * n.x;
        ps.velocity.y -= d * n.y;
        // the 500 selector reads the climb-rate slot's sign (@0x33a50/
        // @0x33ab4 -> @0x33d2e): forward * 0.5 * upscale * cmdScale plus a
        // strafe term through pml.right.z, zeroed upstream (@0x339c6) - so
        // the sign is forward * upscale, which crosses zero at pitch
        // fz = -0.25 and goes negative beyond (0.25@0x70cb4, 2.5@0x70cb8)
        let u = ((forward3(ps).z + LADDER_UPSCALE_BIAS) * LADDER_UPSCALE_GAIN).clamp(-1.0, 1.0);
        let k = if input.forward * u > 0.0 {
            -500.0
        } else {
            -250.0
        };
        ps.velocity.x += k * n.x;
        ps.velocity.y += k * n.y;
    }
    // no gravity while going up a ladder
    step_slide_move(ps, world, dt, false, MASK_PLAYERSOLID, Some(events));
    // PM_LadderMove @0x33d71: the legs face into the wall, and the cap here
    // is 75 degrees, not the 90 the ground path uses.
    ps.movement_dir = clamp_movement_dir(
        angle_delta(yaw_of(normal) + 180.0, ps.yaw.to_degrees()) as i32,
        LADDER_MOVEMENT_DIR_CAP,
    );
}

/// Q3 `bg_pmove.c` `PM_AirMove`.
fn air_move(
    ps: &mut PlayerState,
    input: &PmInput,
    world: &MoveWorld,
    dt: f32,
    mask: u32,
    events: Option<&mut Vec<PmEvent>>,
) {
    let (dir, wishspeed) = wish_air(ps, input);
    accelerate(ps, dir, wishspeed, PM_AIRACCELERATE, dt);
    // A plane too steep to stand on still steers the fall (0x2f1d3).
    if let Some(n) = ps.ground_plane {
        ps.velocity = clip_velocity(ps.velocity, n);
    }
    step_slide_move(ps, world, dt, true, mask, events);
    set_movement_dir(ps, input, dt);
}

/// What [`slide_move`] ran into.
#[derive(Clone, Copy)]
struct Slide {
    blocked: bool,
    /// The start was inside something: the trace could not move at all.
    stuck: bool,
}

/// Q3 `bg_slidemove.c` `PM_SlideMove`.
fn slide_move(ps: &mut PlayerState, world: &MoveWorld, dt: f32, gravity: bool, mask: u32) -> Slide {
    const NUM_BUMPS: usize = 4;
    let (mins, maxs) = (ps.mins(), ps.maxs());

    // `pml.groundPlane`, which a steep plane sets too (0x3483b reads it, not
    // `pml.walking`); `on_ground` covers a state no ground trace has run on.
    let ground = ps.ground_plane.or(ps.on_ground.then_some(ps.ground_normal));
    // average of start and end velocity, matching the analytic parabola
    let mut end_velocity = ps.velocity;
    if gravity {
        end_velocity.z -= GRAVITY * dt;
        ps.velocity.z = (ps.velocity.z + end_velocity.z) * 0.5;
        if let Some(n) = ground {
            ps.velocity = clip_velocity(ps.velocity, n);
        }
    }

    let mut planes: Vec<Vec3> = Vec::with_capacity(MAX_CLIP_PLANES);
    // never turn against the ground plane or back against the original move
    if let Some(n) = ground {
        planes.push(n);
    }
    planes.push(ps.velocity.normalize_or_zero());

    let mut time_left = dt;
    let mut bumps = 0;
    for _ in 0..NUM_BUMPS {
        let end = ps.origin + ps.velocity * time_left;
        let t = world.box_trace(ps.origin, end, mins, maxs, mask);
        if t.allsolid {
            // trapped in solid: keep the horizontal control, kill the fall
            ps.velocity.z = 0.0;
            return Slide {
                blocked: true,
                stuck: true,
            };
        }
        if t.fraction > 0.0 {
            ps.origin = t.endpos;
        }
        if t.fraction == 1.0 {
            break;
        }
        bumps += 1;
        time_left -= time_left * t.fraction;

        if planes.len() >= MAX_CLIP_PLANES {
            ps.velocity = Vec3::ZERO;
            return Slide {
                blocked: true,
                stuck: false,
            };
        }
        // same plane again: nudge out along it (epsilon on non-axial planes)
        if planes.iter().any(|p| t.normal.dot(*p) > 0.99) {
            ps.velocity += t.normal;
            continue;
        }
        planes.push(t.normal);

        for (i, &plane_i) in planes.iter().enumerate() {
            if ps.velocity.dot(plane_i) >= 0.1 {
                continue;
            }
            let mut clipped = clip_velocity(ps.velocity, plane_i);
            let mut end_clipped = clip_velocity(end_velocity, plane_i);

            for (j, &plane_j) in planes.iter().enumerate() {
                if j == i || clipped.dot(plane_j) >= 0.1 {
                    continue;
                }
                clipped = clip_velocity(clipped, plane_j);
                end_clipped = clip_velocity(end_clipped, plane_j);
                if clipped.dot(plane_i) >= 0.0 {
                    continue;
                }
                // two planes: slide along their crease
                let crease = plane_i.cross(plane_j).normalize_or_zero();
                clipped = crease * crease.dot(ps.velocity);
                end_clipped = crease * crease.dot(end_velocity);

                // three planes: nowhere left to go
                if planes
                    .iter()
                    .enumerate()
                    .any(|(k, &p)| k != i && k != j && clipped.dot(p) < 0.1)
                {
                    ps.velocity = Vec3::ZERO;
                    return Slide {
                        blocked: true,
                        stuck: false,
                    };
                }
            }

            ps.velocity = clipped;
            end_velocity = end_clipped;
            break;
        }
    }

    if gravity {
        ps.velocity = end_velocity;
    }
    Slide {
        blocked: bumps != 0,
        stuck: false,
    }
}

/// Q3 `bg_slidemove.c` `PM_StepSlideMove`. Step height 18, or 10 while prone
/// (retail picks the height off pm_flags bit 0x1 @0x35045, not ladder state).
fn step_slide_move(
    ps: &mut PlayerState,
    world: &MoveWorld,
    dt: f32,
    gravity: bool,
    mask: u32,
    events: Option<&mut Vec<PmEvent>>,
) {
    let mut step_size = if ps.stance == Stance::Prone {
        STEPSIZE_PRONE
    } else {
        STEPSIZE
    };
    let start_o = ps.origin;
    let start_v = ps.velocity;
    let slide = slide_move(ps, world, dt, gravity, mask);
    let blocked = slide.blocked;
    // The entry gate (0x35057-0x35112). Retail takes the down pass on every
    // grounded frame, not only a blocked one, where Q3 returns as soon as the
    // slide succeeded (docs/research/cod11-mantle.md, "The ground snap"). In
    // the air it steps only a blocked move: one still under its jump's origin,
    // with the step cut to reach no higher than that origin, or a climb up a
    // ladder ("The jump's step").
    let mut jump_step = false;
    if blocked && !ps.on_ground && ps.jump_origin_z.abs() > 0.001 && start_o.z < ps.jump_origin_z {
        let reach = ps.jump_origin_z - start_o.z;
        if reach < 1.0 {
            return;
        }
        step_size = reach.min(STEPSIZE);
        jump_step = true;
    }
    let through = ps.on_ground || jump_step || blocked && ps.on_ladder && ps.velocity.z > 0.0;
    // A start inside a solid steps out of it as before: retail never has a
    // player there (the revert below), and vcod's tests do.
    if !through && !slide.stuck {
        return;
    }
    let (mins, maxs) = (ps.mins(), ps.maxs());
    let (down_o, down_v) = (ps.origin, ps.velocity);
    // Retail's `ebx` (0x34fdf-0x34ff1): on a ground plane and not on a
    // ladder. A waterjump sets its launch inside the move and the down
    // pass's clip would take it away, so it is kept out of this.
    let ground_plane = ps.on_ground && !ps.on_ladder && ps.waterjump_ms <= 0.0;

    // The step-up runs on a blocked slide only (0x35166): the up trace over
    // `stepSize + 1`, `stepUp` a unit under what it reached and nothing under
    // 1 (0x351ca-0x351e8), the slide again from up there
    // (docs/research/cod11-mantle.md, "The step-up stops a unit short").
    let mut step = 0.0;
    if blocked {
        let up = world.box_trace(
            start_o,
            start_o + Vec3::Z * (step_size + 1.0),
            mins,
            maxs,
            mask,
        );
        let room = (step_size + 1.0) * up.fraction - 1.0;
        if !up.allsolid && room >= 1.0 {
            step = room;
            ps.origin = start_o + Vec3::Z * room;
            ps.velocity = start_v;
            slide_move(ps, world, dt, gravity, mask);
        }
    }

    // The down pass: past the step by half a step size more on a ground
    // plane (0x352e8), and taken on an unblocked grounded frame too, where
    // it reaches 9 units under the plain slide (docs/research/cod11-mantle.md,
    // "The ground snap").
    if ground_plane || step != 0.0 {
        let reach = step + if ground_plane { step_size * 0.5 } else { 0.0 };
        let down = world.box_trace(ps.origin, ps.origin - Vec3::Z * reach, mins, maxs, mask);
        // A down pass that meets a player drops the step and the snap and
        // keeps the plain slide (0x3533a; docs/research/cod11-player-clip.md 2.3).
        if world.entity_num(&down) < MAX_CLIENTS {
            ps.origin = down_o;
            ps.velocity = down_v;
            return;
        }
        if down.fraction < 1.0 {
            ps.origin = down.endpos;
            ps.velocity = clip_velocity(ps.velocity, down.normal);
        } else {
            // Nothing within reach: the step is undone and the snap is not
            // a fall (0x353d0).
            ps.origin.z -= step;
        }
    }

    // Retail's revert (0x353f7-0x3546e): the plain slide's displacement and
    // the stepped one's, both projected on the velocity the down pass left,
    // and the plain slide's state comes back whenever it got as far, so a
    // step that cleared nothing is undone together with its down pass
    // (docs/research/cod11-mantle.md, "The ground snap").
    // A start inside a solid keeps its step out of it: retail's test would
    // put it back, but retail never has a player there, since its spawns sit
    // 0.125 up, and vcod's tests and its fallback spawn do.
    // A jump's step that ends at or above the jump's origin is reverted too
    // (0x35450-0x35468).
    let v = ps.velocity.truncate();
    let flat = v.dot((down_o - start_o).truncate());
    let stepped = v.dot((ps.origin - start_o).truncate());
    if !slide.stuck && flat + STEP_REVERT_EPS > stepped
        || jump_step && ps.origin.z >= ps.jump_origin_z
    {
        ps.origin = down_o;
        ps.velocity = down_v;
        // The ground snap proper (0x354cc-0x3557b): a reverted move on a
        // ground plane is pulled down by half a step size from the plain
        // slide's end, which is what walks a player down a kerb without a
        // fall and raises the negative step events the captures carry.
        if ground_plane {
            let down = world.box_trace(
                ps.origin,
                ps.origin - Vec3::Z * (step_size * 0.5),
                mins,
                maxs,
                mask,
            );
            if down.fraction < 1.0 {
                ps.origin = down.endpos;
                ps.velocity = clip_velocity(ps.velocity, down.normal);
            }
        }
    }
    // A jump's step that rose is held to the speed that just reaches the
    // jump's origin, and to none within 0.1 of it (0x355a4-0x35655).
    if jump_step && ps.origin.z > down_o.z {
        let left = ps.jump_origin_z - ps.origin.z;
        ps.velocity.z = if left < 0.1 {
            0.0
        } else {
            ps.velocity.z.min((2.0 * left * GRAVITY).sqrt())
        };
    }
    // The step-up and the snap both reach this, and so does a reverted step:
    // retail's tail is past every arm of the move (@0x35659), so everything
    // the entry gate let through announces its step.
    if let Some(events) = events.filter(|_| through) {
        step_view(ps, events, start_o.z, down_o.z, step_size);
    }
}

/// The vertical jump the step machinery added, told to the client and paid
/// for in speed. Retail measures the event against the position the plain
/// slide left (@0x3568d) and the velocity scale against the frame's start
/// (@0x35761), and skips both when the step moved the eye by half a unit or
/// less (docs/research/cod11-mantle.md, "The step event and the velocity
/// scale").
fn step_view(
    ps: &mut PlayerState,
    events: &mut Vec<PmEvent>,
    start_z: f32,
    down_z: f32,
    step_size: f32,
) {
    let stepped = ps.origin.z - down_z;
    if stepped.abs() <= STEP_VIEW_EPS {
        return;
    }
    // rounded by a +0.5 bias and an x87 truncate toward zero (@0x356af)
    let units = (stepped + 0.5) as i32;
    if units == 0 {
        return;
    }
    events.push(PmEvent {
        event: EV_STEP_VIEW,
        parm: units.clamp(STEP_VIEW_MIN, STEP_VIEW_MAX) + STEP_VIEW_BIAS,
    });
    ps.velocity *=
        STEP_SCALE_BASE + STEP_SCALE_GAIN * (1.0 - (ps.origin.z - start_z).abs() / step_size);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collision::{CollisionWorld, test_world};
    use crate::movetrace::{Body, CONTENTS_BODY};

    fn flat() -> CollisionWorld {
        test_world(&[])
    }

    /// `PM_WalkMove`'s accel and `PM_Friction`'s ground control both scale
    /// down while the knockback timer runs.
    #[test]
    fn knockback_quarters_accel_and_softens_friction() {
        let w = flat();
        let mw = MoveWorld::bare(&w);
        let run = PmInput {
            forward: 1.0,
            ..Default::default()
        };
        let mut free = PlayerState::spawn(Vec3::new(0.0, 0.0, 0.1), 0.0);
        let mut kb = free;
        kb.knockback_ms = 300.0;
        kb.knockback_flags = PMF_TIME_KNOCKBACK;
        // One frame can't separate a 0.25 accel scale from snap_velocity's
        // rounding, which alone moves either side by up to 0.5; ten frames
        // of walk accel put both well clear of that noise floor.
        for _ in 0..10 {
            pmove(&mut free, &run, &mw, 0.016, &[]);
            pmove(&mut kb, &run, &mw, 0.016, &[]);
        }
        assert!(
            (kb.velocity.x - free.velocity.x * 0.25).abs() < 3.0,
            "{} vs {}",
            kb.velocity.x,
            free.velocity.x
        );

        let mut slide = PlayerState::spawn(Vec3::new(0.0, 0.0, 0.1), 0.0);
        slide.velocity = Vec3::new(190.0, 0.0, 0.0);
        let mut slide_kb = slide;
        slide_kb.knockback_ms = 300.0;
        slide_kb.knockback_flags = PMF_TIME_KNOCKBACK;
        pmove(&mut slide, &PmInput::default(), &mw, 0.008, &[]);
        pmove(&mut slide_kb, &PmInput::default(), &mw, 0.008, &[]);
        assert!(slide_kb.velocity.x > slide.velocity.x);
    }

    /// A hit's knockback timer (`pm_flags` 0x200) takes the ground friction
    /// away, and the gravity `PM_WalkMove` adds under it is folded into
    /// ground speed by the clip; the drop ends it ahead of the walk.
    #[test]
    fn a_damage_knockback_slides_free_of_friction() {
        let w = flat();
        let mw = MoveWorld::bare(&w);
        let mut ps = PlayerState::spawn(Vec3::new(0.0, 0.0, 0.1), 0.0);
        pmove(&mut ps, &PmInput::default(), &mw, 0.008, &[]);
        ps.velocity = Vec3::new(80.0, 0.0, 0.0);
        ps.knockback_ms = 50.0;
        ps.knockback_flags = PMF_TIME_DAMAGE;
        let mut free = ps;
        free.knockback_ms = 0.0;
        free.knockback_flags = 0;
        for _ in 0..6 {
            pmove(&mut ps, &PmInput::default(), &mw, 0.008, &[]);
            pmove(&mut free, &PmInput::default(), &mw, 0.008, &[]);
        }
        assert!(ps.velocity.x >= 80.0, "{}", ps.velocity.x);
        assert!(free.velocity.x < 70.0, "{}", free.velocity.x);
        assert_eq!(ps.knockback_ms, 2.0);
        pmove(&mut ps, &PmInput::default(), &mw, 0.008, &[]);
        assert_eq!((ps.knockback_ms, ps.knockback_flags), (0.0, 0));
        assert!(ps.velocity.x < 80.0, "friction is back, {}", ps.velocity.x);
    }

    /// Slick ground takes the ground friction away, walks at an accel of 1
    /// where standing on dirt takes 9, and adds the gravity the clip folds
    /// back into speed, so a slide keeps going where dirt stops it.
    #[test]
    fn slick_ground_has_no_friction_and_an_accel_of_one() {
        let floor = |flags| {
            crate::collision::synthetic_world(
                &[(
                    "textures/test/floor",
                    crate::collision::CONTENTS_SOLID,
                    flags,
                )],
                &[(0, [-2048.0, -2048.0, -16.0], [2048.0, 2048.0, 0.0])],
            )
        };
        let (ice, dirt) = (floor(crate::collision::SURF_SLICK), floor(0));
        let (ice, dirt) = (MoveWorld::bare(&ice), MoveWorld::bare(&dirt));
        let mut on_ice = PlayerState::spawn(Vec3::new(0.0, 0.0, 0.1), 0.0);
        pmove(&mut on_ice, &PmInput::default(), &ice, 0.008, &[]);
        assert!(on_slick(&on_ice));
        let mut on_dirt = PlayerState::spawn(Vec3::new(0.0, 0.0, 0.1), 0.0);
        pmove(&mut on_dirt, &PmInput::default(), &dirt, 0.008, &[]);
        assert!(!on_slick(&on_dirt));

        let (mut slide_ice, mut slide_dirt) = (on_ice, on_dirt);
        slide_ice.velocity = Vec3::new(80.0, 0.0, 0.0);
        slide_dirt.velocity = slide_ice.velocity;
        for _ in 0..6 {
            pmove(&mut slide_ice, &PmInput::default(), &ice, 0.008, &[]);
            pmove(&mut slide_dirt, &PmInput::default(), &dirt, 0.008, &[]);
        }
        assert!(slide_ice.velocity.x >= 80.0, "{}", slide_ice.velocity.x);
        assert!(slide_dirt.velocity.x < 70.0, "{}", slide_dirt.velocity.x);
        assert!(slide_ice.on_ground);

        let run = PmInput {
            forward: 1.0,
            ..Default::default()
        };
        let (mut run_ice, mut run_dirt) = (on_ice, on_dirt);
        pmove(&mut run_ice, &run, &ice, 0.016, &[]);
        pmove(&mut run_dirt, &run, &dirt, 0.016, &[]);
        // Ice: the accel's 1 * 0.016 * 190 = 3.04 plus the gravity's 12.8
        // down, which the clip's rescale turns into |(3.04, 12.8)| = 13.16
        // of ground speed. An accel of 9 would read 30, no gravity 3.
        assert_eq!(run_ice.velocity, Vec3::new(13.0, 0.0, 0.0));
        // Dirt: 9 * 0.016 * 190 = 27.36.
        assert_eq!(run_dirt.velocity, Vec3::new(27.0, 0.0, 0.0));
    }

    /// `PM_DropTimers` zeroes the timer once the frame's ms reach it, and
    /// otherwise subtracts.
    #[test]
    fn knockback_runs_out() {
        let w = flat();
        let mut ps = PlayerState::spawn(Vec3::ZERO, 0.0);
        ps.knockback_ms = 20.0;
        pmove(
            &mut ps,
            &PmInput::default(),
            &MoveWorld::bare(&w),
            0.016,
            &[],
        );
        assert_eq!(ps.knockback_ms, 4.0);
        pmove(
            &mut ps,
            &PmInput::default(),
            &MoveWorld::bare(&w),
            0.016,
            &[],
        );
        assert_eq!(ps.knockback_ms, 0.0);
    }

    /// `PM_DropTimers` runs from `PmoveSingle` for every `pm_type`, so a
    /// linked player (an S&D planter) still drops the timer even though
    /// `linked_move` skips the move, ground and weapon dispatch.
    #[test]
    fn a_linked_player_still_drops_knockback() {
        let w = flat();
        let mut ps = PlayerState::spawn(Vec3::ZERO, 0.0);
        ps.linked = true;
        ps.knockback_ms = 20.0;
        pmove(
            &mut ps,
            &PmInput::default(),
            &MoveWorld::bare(&w),
            0.016,
            &[],
        );
        assert_eq!(ps.knockback_ms, 4.0);
        pmove(
            &mut ps,
            &PmInput::default(),
            &MoveWorld::bare(&w),
            0.016,
            &[],
        );
        assert_eq!(ps.knockback_ms, 0.0);
    }

    /// A death takes the sight down: `PmoveSingle`'s dead arm calls
    /// `PM_ClearAimDownSightFlag` and nothing else about the weapon runs, so
    /// the fraction stays where the death froze it
    /// (docs/research/cod11-combat.md, 1.13).
    #[test]
    fn a_death_clears_the_ads_flag() {
        let w = flat();
        let w = MoveWorld::bare(&w);
        let mut ps = PlayerState::spawn(Vec3::ZERO, 0.0);
        ps.ads_active = true;
        ps.weapon_pos_frac = 1.0;
        dead_move(&mut ps, &w, 0.05);
        assert!(!ps.ads_active);
        assert_eq!(ps.weapon_pos_frac, 1.0);
    }

    /// The mounted arm (0x34274) never reaches the move dispatch or
    /// `PM_Weapon`, so a mounted player's origin holds and no fire event
    /// comes out of pmove (docs/research/cod11-turrets.md, section 5).
    #[test]
    fn a_mounted_player_does_not_move_or_fire() {
        let world = flat();
        let world = MoveWorld::bare(&world);
        let mut ps = PlayerState::spawn(Vec3::new(0.0, 0.0, 0.0), 0.0);
        ps.mounted = Some(Stance::Stand);
        let input = PmInput {
            forward: 127.0,
            attack: true,
            ..Default::default()
        };
        let before = ps.origin;
        let events = pmove(&mut ps, &input, &world, 0.05, &[]);
        assert_eq!(ps.origin, before);
        assert!(!ps.on_ground, "groundEntityNum NONE while mounted");
        assert!(events.iter().all(
            |e| e.event != weapon::EV_FIRE_WEAPON && e.event != weapon::EV_FIRE_WEAPON_LASTSHOT
        ));
    }

    /// The stance step (0x316f4) puts the player at the gun's stance
    /// whatever the cmd asks, and `PM_UpdateLean` forces the lean input to 0
    /// (docs/research/cod11-turrets.md, section 5).
    #[test]
    fn a_mounted_player_takes_the_gun_stance_whatever_the_cmd_asks() {
        let world = flat();
        let world = MoveWorld::bare(&world);
        let mut ps = PlayerState::spawn(Vec3::ZERO, 0.0);
        ps.mounted = Some(Stance::Prone);
        let input = PmInput {
            crouch: true,
            ..Default::default()
        };
        pmove(&mut ps, &input, &world, 0.05, &[]);
        assert_eq!(ps.stance, Stance::Prone);
        assert_eq!(ps.lean, 0.0);
    }

    /// 0x34274 calls no footstep routine, so a gunner mounted off a ladder
    /// neither steps nor stays on it.
    #[test]
    fn a_mounted_player_takes_no_step_and_leaves_the_ladder() {
        let world = flat();
        let world = MoveWorld::bare(&world);
        let mut ps = PlayerState::spawn(Vec3::ZERO, 0.0);
        ps.mounted = Some(Stance::Stand);
        ps.on_ladder = true;
        ps.since_jump_ms = 10_000.0;
        ps.velocity = Vec3::new(0.0, 0.0, 200.0);
        let before = ps.bob_cycle;
        let events = pmove(&mut ps, &PmInput::default(), &world, 0.05, &[]);
        assert!(!ps.on_ladder);
        assert_eq!(ps.bob_cycle, before);
        assert!(events.is_empty(), "{events:?}");
    }

    /// Flat ground whose material carries the dirt sound surface (6).
    fn dirt_flat() -> CollisionWorld {
        crate::collision::synthetic_world(
            &[(
                "textures/test/dirt",
                crate::collision::CONTENTS_SOLID,
                6 << 20,
            )],
            &[(0, [-2048.0, -2048.0, -16.0], [2048.0, 2048.0, 0.0])],
        )
    }

    /// Ankle-deep water over a floor at -6; both materials carry no sound
    /// surface, so any footstep id can only come from the fixed water ids.
    fn shallow_water() -> CollisionWorld {
        crate::collision::synthetic_world(
            &[
                ("textures/test/solid", crate::collision::CONTENTS_SOLID, 0),
                ("textures/common/water", crate::collision::CONTENTS_WATER, 0),
            ],
            &[
                (0, [-1024.0, -1024.0, -22.0], [1024.0, 1024.0, -6.0]),
                (1, [-1024.0, -1024.0, -74.0], [1024.0, 1024.0, 10.0]),
            ],
        )
    }

    /// A pool: dry ground west, deep water (surface z=36) over a floor at
    /// -74, a chest-deep shelf (top 0) and an ankle-deep shelf (top 30), an
    /// exit wall at x=200 rising to 15, and landing ground east of it.
    fn pool_world() -> CollisionWorld {
        crate::collision::synthetic_world(
            &[
                ("textures/test/solid", crate::collision::CONTENTS_SOLID, 0),
                ("textures/common/water", crate::collision::CONTENTS_WATER, 0),
            ],
            &[
                (0, [-600.0, -300.0, -16.0], [-200.0, 300.0, 0.0]),
                (0, [-200.0, -300.0, -90.0], [200.0, 300.0, -74.0]),
                (0, [-160.0, -100.0, -84.0], [185.0, 100.0, 0.0]),
                (0, [-200.0, 150.0, -84.0], [200.0, 260.0, 30.0]),
                (0, [200.0, -300.0, -90.0], [216.0, 300.0, 15.0]),
                (0, [240.0, -300.0, -16.0], [600.0, 300.0, 0.0]),
                (1, [-200.0, -300.0, -74.0], [200.0, 300.0, 36.0]),
            ],
        )
    }

    fn tick(ps: &mut PlayerState, input: &PmInput, w: &MoveWorld, n: usize) {
        for _ in 0..n {
            pmove(ps, input, w, 1.0 / 125.0, &[]);
        }
    }

    /// Standing to prone is two legs of the eye, 150 ms down to the crouch
    /// height and 400 ms on along a curve that dips to 15 at half way, and the
    /// walk scale is prone's through the first and blends from crouch's
    /// through the second (docs/research/cod11-mantle.md, "The eye through a
    /// stance change"; the street capture's sideways press is the evidence).
    #[test]
    fn a_prone_press_walks_the_eye_through_the_crouch_height() {
        let w = flat();
        let w = MoveWorld::bare(&w);
        let mut ps = PlayerState::spawn(Vec3::ZERO, 0.0);
        tick(&mut ps, &PmInput::default(), &w, 50); // settle on the ground
        assert_eq!(ps.view_height(), VIEW_STAND);

        let prone = PmInput {
            prone: true,
            ..PmInput::default()
        };
        tick(&mut ps, &prone, &w, 1);
        assert_eq!(
            (ps.view_lerp_target, ps.view_lerp_down),
            (VIEW_CROUCH, true)
        );
        assert_eq!(stance_speed_scale(&ps), SCALE_PRONE);
        tick(&mut ps, &prone, &w, 19); // 152 ms: the crouch leg is done
        assert_eq!(
            (ps.view_lerp_target, ps.view_lerp_ms),
            (VIEW_PRONE, Some(0))
        );
        assert_eq!(ps.view_height(), VIEW_CROUCH);
        tick(&mut ps, &prone, &w, 25); // 200 ms into the prone leg
        assert_eq!(ps.view_height(), 15.0);
        let half = SCALE_PRONE * 0.5 + SCALE_CROUCH * 0.5;
        assert!((stance_speed_scale(&ps) - half).abs() < 1e-6);
        tick(&mut ps, &prone, &w, 25);
        assert_eq!(ps.view_height(), VIEW_PRONE);
        assert_eq!(ps.view_lerp_ms, None);
        assert_eq!(stance_speed_scale(&ps), SCALE_PRONE);
    }

    /// The hip cone's minimum follows the same two legs, linearly in time
    /// from one end's minimum to the other's (combat doc 2.1).
    #[test]
    fn a_prone_press_blends_the_hip_spread_minimum_across_both_legs() {
        use weapon::{SpreadStance, hip_spread_min};
        let def = WeaponDef {
            hip_spread_stand_min: 3.0,
            hip_spread_ducked_min: 2.0,
            hip_spread_prone_min: 1.0,
            ..WeaponDef::default()
        };
        let w = flat();
        let w = MoveWorld::bare(&w);
        let mut ps = PlayerState::spawn(Vec3::ZERO, 0.0);
        tick(&mut ps, &PmInput::default(), &w, 50);
        let min =
            |ps: &PlayerState, time: i32| hip_spread_min(&def, &SpreadStance::of(ps, 0, time));
        assert_eq!(min(&ps, 0), 3.0);

        let prone = PmInput {
            prone: true,
            ..PmInput::default()
        };
        tick(&mut ps, &prone, &w, 1);
        assert_eq!(ps.view_lerp_ms, Some(0));
        assert_eq!(min(&ps, 0), 3.0);
        assert_eq!(min(&ps, 75), 2.5, "half the 150 ms leg down to crouch");
        assert_eq!(min(&ps, -10), 3.0, "a clock behind the leg's stamp");
        tick(&mut ps, &prone, &w, 19);
        assert_eq!(min(&ps, 0), 2.0, "the prone leg's start");
        tick(&mut ps, &prone, &w, 25);
        assert_eq!(min(&ps, 0), 1.5, "half the 400 ms leg to prone");
        assert_eq!(min(&ps, 1000), 1.0, "a clock past the leg's end");
        tick(&mut ps, &prone, &w, 25);
        assert_eq!(min(&ps, 0), 1.0);
    }

    /// `movementDir` is the legs' heading off the view, which the player
    /// entity carries as `angles2[1]`. Straight ahead is 0, a pure strafe is
    /// the 90-degree cap, and a backpedal folds by 180.
    #[test]
    fn movement_dir_is_the_legs_yaw_off_the_view() {
        let w = flat();
        let w = MoveWorld::bare(&w);
        // From a standstill each time: velocity left over from the previous
        // input keeps the displacement off the key being held.
        let held = |input: &PmInput| {
            let mut ps = PlayerState::spawn(Vec3::ZERO, 0.0);
            tick(&mut ps, &PmInput::default(), &w, 50);
            tick(&mut ps, input, &w, 50);
            ps.movement_dir
        };
        assert_eq!(held(&PmInput::default()), 0, "standing still");
        assert_eq!(
            held(&PmInput {
                forward: 1.0,
                ..PmInput::default()
            }),
            0,
            "running along the view"
        );
        // Right is -y at yaw 0, so a right strafe reads negative.
        assert_eq!(
            held(&PmInput {
                right: 1.0,
                ..PmInput::default()
            }),
            -90,
            "strafing right"
        );
        let diagonal = held(&PmInput {
            forward: 1.0,
            right: 1.0,
            ..PmInput::default()
        });
        assert!(
            (diagonal + 45).abs() <= 1,
            "running forward-right: {diagonal}"
        );
        // Backpedalling points the legs the way the body faces, not the way
        // it travels: retail folds the angle by 180 when forwardmove < 0.
        assert_eq!(
            held(&PmInput {
                forward: -1.0,
                ..PmInput::default()
            }),
            0,
            "backpedalling"
        );

        // Prone reads the body's own yaw instead of the move, so turning the
        // view inside the prone cone turns the legs away from it. Retail
        // settles at -59 for a 60-degree turn, the truncation of the same
        // angle.
        let mut ps = PlayerState::spawn(Vec3::ZERO, 0.0);
        tick(&mut ps, &PmInput::default(), &w, 50);
        let prone = PmInput {
            prone: true,
            ..PmInput::default()
        };
        tick(&mut ps, &prone, &w, 60);
        assert_eq!(ps.stance, Stance::Prone);
        ps.yaw = (-60f32).to_radians();
        tick(&mut ps, &prone, &w, 20);
        assert_eq!(ps.movement_dir, 60, "prone, view turned 60 degrees");
    }

    #[test]
    fn falls_and_lands_on_floor() {
        let w = flat();
        let w = MoveWorld::bare(&w);
        let mut ps = PlayerState::spawn(Vec3::new(0.0, 0.0, 50.0), 0.0);
        tick(&mut ps, &PmInput::default(), &w, 250);
        assert!(ps.on_ground);
        assert!(ps.origin.z.abs() < 0.5, "settled at {}", ps.origin.z);
        assert!(ps.velocity.length() < 1.0);
    }

    /// Full run on dirt: rate 0.335 ticks/ms puts a step every ~382 ms
    /// (rodata 0x70c44, PM_Footsteps @0x327f8).
    #[test]
    fn running_steps_fire_on_the_retail_cadence() {
        let w = dirt_flat();
        let w = MoveWorld::bare(&w);
        let mut ps = PlayerState::spawn(Vec3::ZERO, 0.0);
        let run = PmInput {
            forward: 1.0,
            ..Default::default()
        };
        for _ in 0..125 {
            pmove(&mut ps, &run, &w, 1.0 / 125.0, &[]);
        }
        assert!(ps.velocity.truncate().length() > 180.0);
        let mut steps = Vec::new();
        for _ in 0..375 {
            steps.extend(
                pmove(&mut ps, &run, &w, 8.0 / 1000.0, &[])
                    .into_iter()
                    .map(|e| e.event),
            );
        }
        assert!(
            steps.iter().all(|&e| e == EV_FOOTSTEP_RUN_BASE + 6),
            "{steps:?}"
        );
        // Retail rounds the cycle per frame (fistp @0x32819), so at fixed 8 ms
        // frames the 2.68-tick advance quantizes to 3 and the period shrinks
        // to ~340 ms; 7..9 covers both that and the settle-phase offset.
        assert!(
            steps.len() >= 7 && steps.len() <= 9,
            "{} events",
            steps.len()
        );
    }

    /// The enable gate: walk key and the crouched/prone gaits tick the cycle
    /// but never emit (PM_ShouldMakeFootsteps @0x3221c).
    #[test]
    fn walk_key_and_stances_are_silent() {
        let w = dirt_flat();
        let w = MoveWorld::bare(&w);
        for (name, input) in [
            (
                "walk key",
                PmInput {
                    forward: 1.0,
                    walk_slow: true,
                    ..Default::default()
                },
            ),
            (
                "crouch",
                PmInput {
                    forward: 1.0,
                    crouch: true,
                    ..Default::default()
                },
            ),
            (
                "prone",
                PmInput {
                    forward: 1.0,
                    prone: true,
                    ..Default::default()
                },
            ),
        ] {
            let mut ps = PlayerState::spawn(Vec3::ZERO, 0.0);
            for _ in 0..125 {
                pmove(&mut ps, &input, &w, 1.0 / 125.0, &[]);
            }
            let speed = ps.velocity.truncate().length();
            assert!(speed > 10.0, "{name} must still move, at {speed}");
            let events: Vec<_> = (0..250)
                .flat_map(|_| pmove(&mut ps, &input, &w, 8.0 / 1000.0, &[]))
                .collect();
            assert!(events.is_empty(), "{name} played {events:?}");
        }
    }

    /// Keys released: the timer advances but no event fires (@0x32831 path).
    #[test]
    fn glide_suppresses_step_events() {
        let w = dirt_flat();
        let w = MoveWorld::bare(&w);
        let mut ps = PlayerState::spawn(Vec3::ZERO, 0.0);
        let run = PmInput {
            forward: 1.0,
            ..Default::default()
        };
        for _ in 0..125 {
            pmove(&mut ps, &run, &w, 1.0 / 125.0, &[]);
        }
        let idle = PmInput::default();
        let mut events = Vec::new();
        while ps.velocity.truncate().length() > 10.0 {
            events.extend(pmove(&mut ps, &idle, &w, 8.0 / 1000.0, &[]));
        }
        assert!(events.is_empty(), "{events:?}");
    }

    #[test]
    fn landing_plays_one_event_from_the_impact_bands() {
        let w = dirt_flat();
        let w = MoveWorld::bare(&w);
        let mut ps = PlayerState::spawn(Vec3::new(0.0, 0.0, 200.0), 0.0);
        let idle = PmInput::default();
        let mut events = Vec::new();
        for _ in 0..500 {
            events.extend(pmove(&mut ps, &idle, &w, 8.0 / 1000.0, &[]));
        }
        assert!(ps.on_ground);
        let ids: Vec<i32> = events.into_iter().map(|e| e.event).collect();
        assert_eq!(ids, vec![EV_LANDING_BASE + 6], "a long fall lands once");
    }

    /// The ladder reads the fall height, not the speed: the one-unit drop a
    /// turret release ends in lands in silence, as the retail turret capture
    /// reads it (`crates/server/tests/turret_ab.rs`).
    #[test]
    fn a_one_unit_drop_lands_silently() {
        let w = dirt_flat();
        let w = MoveWorld::bare(&w);
        let mut ps = PlayerState::spawn(Vec3::new(0.0, 0.0, 1.0), 0.0);
        ps.on_ground = false;
        let idle = PmInput::default();
        let mut events = Vec::new();
        for _ in 0..20 {
            events.extend(pmove(&mut ps, &idle, &w, 8.0 / 1000.0, &[]));
        }
        assert!(ps.on_ground);
        assert!(events.is_empty(), "{events:?}");
    }

    /// The land anim's speed half: a 200-unit fall lands faster than
    /// `LAND_ANIM_SPEED` and reports it on the landing move only; a
    /// one-unit drop never does.
    #[test]
    fn only_a_fast_landing_reports_the_land_anim() {
        let w = dirt_flat();
        let w = MoveWorld::bare(&w);
        let idle = PmInput::default();
        let mut ps = PlayerState::spawn(Vec3::new(0.0, 0.0, 200.0), 0.0);
        ps.on_ground = false;
        let mut reported = Vec::new();
        for i in 0..500 {
            let was = ps.on_ground;
            pmove(&mut ps, &idle, &w, 8.0 / 1000.0, &[]);
            if ps.land_anim {
                reported.push((i, was, ps.on_ground));
            }
        }
        assert_eq!(reported.len(), 1, "{reported:?}");
        assert_eq!((reported[0].1, reported[0].2), (false, true));

        let mut ps = PlayerState::spawn(Vec3::new(0.0, 0.0, 1.0), 0.0);
        ps.on_ground = false;
        for _ in 0..20 {
            pmove(&mut ps, &idle, &w, 8.0 / 1000.0, &[]);
            assert!(!ps.land_anim);
        }
        assert!(ps.on_ground);
    }

    #[test]
    fn landing_sound_bands_follow_fall_height() {
        let band = |sf: u32, height: f32| {
            let mut ps = PlayerState::spawn(Vec3::ZERO, 0.0);
            ps.ground_surface_flags = sf;
            landing_event(height, &ps).map(|e| (e.event, e.parm))
        };
        let dirt = 6 << 20;
        assert_eq!(band(dirt, 4.0), None);
        assert_eq!(band(dirt, 4.5), Some((EV_FOOTSTEP_WALK_BASE + 6, 0)));
        assert_eq!(band(dirt, 11.5), Some((EV_FOOTSTEP_RUN_BASE + 6, 0)));
        assert_eq!(band(dirt, 12.0), Some((EV_LANDING_BASE + 6, 0)));
        // the land event's parm is the view bob, 4 at 12 up to 24
        assert_eq!(band(dirt, 38.0), Some((EV_LANDING_BASE + 6, 8)));
        assert_eq!(band(dirt, 200.0), Some((EV_LANDING_BASE + 6, 24)));
        // material 0 silences the steps but lands on the default surface
        let quiet = SURF_NO_SOUND | dirt;
        assert_eq!(band(quiet, 11.5), None);
        assert_eq!(band(quiet, 38.0), Some((EV_LANDING_BASE, 8)));
    }

    /// Drop a player from `height` onto flat dirt at 8 ms frames and return
    /// the landing frame's state and events.
    fn land_from(height: f32, sf: u32) -> (PlayerState, Vec<PmEvent>) {
        let w = crate::collision::synthetic_world(
            &[("textures/test/floor", crate::collision::CONTENTS_SOLID, sf)],
            &[(0, [-2048.0, -2048.0, -16.0], [2048.0, 2048.0, 0.0])],
        );
        let w = MoveWorld::bare(&w);
        let mut ps = PlayerState::spawn(Vec3::new(0.0, 0.0, height), 0.0);
        ps.on_ground = false;
        ps.velocity.x = 100.0;
        for _ in 0..1000 {
            let events = pmove(&mut ps, &PmInput::default(), &w, 8.0 / 1000.0, &[]);
            if ps.on_ground {
                return (ps, events);
            }
        }
        panic!("never landed");
    }

    /// A fall past `bg_fallDamageMinHeight` (256) lands with the damage in
    /// the pain event's parm, `pm_time` 35 * damage + 500 under 0x100 and
    /// the velocity scaled by the stun's multiplier.
    #[test]
    fn a_damaging_landing_starts_the_stun() {
        let dirt = 6 << 20;
        let (ps, events) = land_from(300.0, dirt);
        let damage = events
            .iter()
            .find(|e| e.event == EV_LANDING_PAIN_BASE + 6)
            .map(|e| e.parm)
            .expect("pain event");
        // The velocity snap drops 0.4 a frame off gravity's 6.4, so the 300
        // units read as 281 by the impact speed: 11 percent.
        assert_eq!(damage, 11);
        let stun = 35 * damage + 500;
        assert_eq!(ps.knockback_ms, stun as f32);
        assert_eq!(ps.knockback_flags, PMF_TIME_KNOCKBACK);
        let scale = 0.5 - (stun - 500) as f32 / 1000.0 * 0.3;
        // 38.45, snapped
        assert_eq!(ps.velocity.x, (100.0 * scale).round(), "scale {scale}");

        // past 28 percent the scale pins at 0.2, past 42 the timer at 2000
        let (ps, events) = land_from(420.0, dirt);
        let damage = events
            .iter()
            .find(|e| e.event >= EV_LANDING_PAIN_BASE)
            .unwrap()
            .parm;
        assert!(damage > 42, "damage {damage}");
        assert_eq!(ps.knockback_ms, 2000.0);
        assert!((ps.velocity.x - 20.0).abs() < 0.01, "{}", ps.velocity.x);
    }

    /// Under the min height, on a no-damage or slick surface, and at 100
    /// percent there is no stun; the last three damp by 0.67.
    #[test]
    fn a_soft_landing_starts_no_stun() {
        let dirt = 6 << 20;
        let (ps, events) = land_from(200.0, dirt);
        assert_eq!((ps.knockback_ms, ps.knockback_flags), (0.0, 0));
        assert_eq!(events[0].event, EV_LANDING_BASE + 6);
        assert!((ps.velocity.x - 67.0).abs() < 0.01);

        let (ps, events) = land_from(300.0, dirt | SURF_NODAMAGE);
        assert_eq!((ps.knockback_ms, ps.knockback_flags), (0.0, 0));
        assert_eq!(events[0].event, EV_LANDING_BASE + 6);

        for (sf, height, what) in [
            (dirt | crate::collision::SURF_SLICK, 300.0, "slick"),
            (dirt, 600.0, "fatal"),
        ] {
            let (ps, events) = land_from(height, sf);
            assert_eq!((ps.knockback_ms, ps.knockback_flags), (0.0, 0), "{what}");
            assert_eq!(events[0].event, EV_LANDING_PAIN_BASE + 6, "{what}");
            assert!(
                (ps.velocity.x - 67.0).abs() < 0.01,
                "{what} {}",
                ps.velocity.x
            );
        }
    }

    /// The two bounds are the cvars' values, and bounds out of order or a
    /// negative min take no damage at all (0x2fe6c).
    #[test]
    fn the_fall_damage_bounds_are_the_cvars() {
        let at = |min, max| FallHeights { min, max };
        assert_eq!(fall_damage(300.0, 0, 0, FallHeights::default()), 19);
        assert_eq!(fall_damage(300.0, 0, 0, at(200.0, 1000.0)), 12);
        assert_eq!(fall_damage(300.0, 0, 0, at(100.0, 300.0)), 100);
        assert_eq!(fall_damage(250.0, 0, 2, at(200.0, 300.0)), 25);
        assert_eq!(fall_damage(900.0, 0, 0, at(480.0, 256.0)), 0);
        assert_eq!(fall_damage(900.0, 0, 0, at(-1.0, 256.0)), 0);

        // The retail run's systeminfo under `+set`s (player-clip doc 8.10).
        let info = r"\bg_fallDamageMaxHeight\1000\bg_fallDamageMinHeight\200\sv_cheats\0";
        assert_eq!(FallHeights::from_systeminfo(info), at(200.0, 1000.0));
        assert_eq!(
            FallHeights::from_systeminfo(r"\sv_cheats\0"),
            FallHeights::default()
        );
    }

    /// A corpse dropped onto the floor lands like a live player on a soft
    /// fall, whatever the height: the damp and the land event, no pain and
    /// no stun (`pm_type > 5` at 0x2feba).
    #[test]
    fn a_corpse_lands_with_no_damage() {
        let w = crate::collision::synthetic_world(
            &[(
                "textures/test/floor",
                crate::collision::CONTENTS_SOLID,
                6 << 20,
            )],
            &[(0, [-2048.0, -2048.0, -16.0], [2048.0, 2048.0, 0.0])],
        );
        let w = MoveWorld::bare(&w);
        let mut ps = PlayerState::spawn(Vec3::new(0.0, 0.0, 600.0), 0.0);
        ps.on_ground = false;
        let mut landed = None;
        for _ in 0..1000 {
            let events = dead_move(&mut ps, &w, 0.016);
            if ps.on_ground {
                landed = Some(events);
                break;
            }
        }
        let events = landed.expect("never landed");
        assert_eq!(events.len(), 1, "{events:?}");
        assert_eq!(events[0].event, EV_LANDING_BASE + 6);
        assert_eq!((ps.knockback_ms, ps.knockback_flags), (0.0, 0));
    }

    /// Water level 1-2 replaces the ground material with the fixed water ids
    /// and ignores the enable gate (PM_FootstepEvent @0x321b0).
    #[test]
    fn water_steps_use_the_fixed_water_ids() {
        let w = shallow_water();
        let w = MoveWorld::bare(&w);
        let mut ps = PlayerState::spawn(Vec3::new(0.0, 0.0, -6.0), 0.0);
        let run = PmInput {
            forward: 1.0,
            ..Default::default()
        };
        for _ in 0..125 {
            pmove(&mut ps, &run, &w, 1.0 / 125.0, &[]);
        }
        assert_eq!(ps.water_level, 1);
        let mut ids = Vec::new();
        for _ in 0..250 {
            ids.extend(
                pmove(&mut ps, &run, &w, 8.0 / 1000.0, &[])
                    .into_iter()
                    .map(|e| e.event),
            );
        }
        assert!(!ids.is_empty());
        assert!(ids.iter().all(|&e| e == EV_FOOTSTEP_RUN_WATER), "{ids:?}");
    }

    /// The airborne ladder branch probes into the wall face; a miss or a
    /// material-less hit defaults to metal (@0x32039-0x32135).
    #[test]
    fn ladder_steps_probe_into_the_face_and_default_to_metal() {
        let input = PmInput::default();
        let dt = 0.05;

        let mut ps = PlayerState::spawn(Vec3::ZERO, 0.0);
        ps.on_ground = false;
        ps.on_ladder = true;
        // normal points away from the wall, so the probe reaches back into it
        ps.ladder_normal = -Vec3::X;
        ps.velocity.z = 80.0;
        let mut events = Vec::new();
        for _ in 0..20 {
            footsteps(
                &mut ps,
                &input,
                &MoveWorld::bare(&dirt_flat()),
                dt,
                &mut events,
            );
        }
        assert!(!events.is_empty(), "climbing at vz=80 must step");
        assert!(
            events
                .iter()
                .all(|e| e.event == EV_FOOTSTEP_RUN_BASE + DEFAULT_MATERIAL)
        );

        // same climb against a wooden wall: the probe reports wood
        let wall = crate::collision::synthetic_world(
            &[(
                "textures/test/wood",
                crate::collision::CONTENTS_SOLID,
                21 << 20,
            )],
            &[(0, [16.0, -1024.0, -16.0], [32.0, 1024.0, 512.0])],
        );
        let wall = MoveWorld::bare(&wall);
        let mut ps = PlayerState::spawn(Vec3::new(1.0, 0.0, 40.0), 0.0);
        ps.on_ground = false;
        ps.on_ladder = true;
        ps.ladder_normal = -Vec3::X;
        ps.velocity.z = 80.0;
        let mut events = Vec::new();
        for _ in 0..20 {
            footsteps(&mut ps, &input, &wall, dt, &mut events);
        }
        assert!(!events.is_empty());
        assert!(events.iter().all(|e| e.event == EV_FOOTSTEP_RUN_BASE + 21));
    }

    /// PM_CheckJump @0x2eda8: the push-off event carries the last ground trace's
    /// material; from mid-wall (no ground under the climb) it stays silent.
    #[test]
    fn push_off_jump_event_reads_the_last_ground() {
        let n = Vec3::new(-1.0, 0.0, 0.0);
        let jump = PmInput {
            jump: true,
            ..Default::default()
        };
        let wall = crate::collision::synthetic_world(
            &[
                (
                    "textures/test/wood",
                    crate::collision::CONTENTS_SOLID,
                    21 << 20,
                ),
                (
                    "textures/common/ladder",
                    crate::collision::CONTENTS_SOLID,
                    0x8,
                ),
            ],
            &[
                (0, [-1024.0, -1024.0, -16.0], [1024.0, 1024.0, 0.0]),
                (1, [16.0, -1024.0, 0.0], [32.0, 1024.0, 512.0]),
            ],
        );
        let wall = MoveWorld::bare(&wall);
        let mut ps = PlayerState::spawn(Vec3::new(0.0, 0.0, 0.0), 0.0);
        ps.since_jump_ms = 10_000.0;
        pmove(&mut ps, &PmInput::default(), &wall, 1.0 / 125.0, &[]);
        assert_eq!(ps.ground_surface_flags, 21 << 20);
        let mut events = Vec::new();
        ladder_move(&mut ps, &jump, n, false, &wall, 1.0 / 125.0, &mut events);
        assert_eq!(
            events,
            vec![PmEvent {
                event: EV_JUMP_BASE + 21,
                parm: 0
            }]
        );

        // mid-wall: the last ground trace hit nothing, so no event
        let mut ps = PlayerState::spawn(Vec3::new(1.0, 0.0, 200.0), 0.0);
        ps.on_ground = false;
        ps.since_jump_ms = 10_000.0;
        let mut events = Vec::new();
        ladder_move(&mut ps, &jump, n, false, &wall, 1.0 / 125.0, &mut events);
        assert!(ladder_push_off_ran(&ps));
        assert!(events.is_empty());
    }

    /// The 299 ms post-push-off quiet window (@0x323b9).
    #[test]
    fn ladder_steps_stay_quiet_after_a_push_off() {
        let input = PmInput::default();
        let mut ps = PlayerState::spawn(Vec3::ZERO, 0.0);
        ps.on_ground = false;
        ps.on_ladder = true;
        ps.ladder_normal = -Vec3::X;
        ps.velocity.z = 80.0;
        ps.since_jump_ms = 0.0;
        let mut events = Vec::new();
        for _ in 0..5 {
            ps.since_jump_ms += 50.0;
            footsteps(
                &mut ps,
                &input,
                &MoveWorld::bare(&dirt_flat()),
                0.05,
                &mut events,
            );
        }
        assert!(events.is_empty(), "quiet for 299 ms, got {events:?}");
        for _ in 0..20 {
            ps.since_jump_ms += 50.0;
            footsteps(
                &mut ps,
                &input,
                &MoveWorld::bare(&dirt_flat()),
                0.05,
                &mut events,
            );
        }
        assert!(!events.is_empty(), "climbing must step once quiet");
    }

    /// `check_jump` off a ladder plane, the way `ladder_move` calls it.
    fn push_off(ps: &mut PlayerState, input: &PmInput, normal: Vec3) -> bool {
        let mut cmd = *input;
        check_jump(ps, &mut cmd, Some(normal), &mut Vec::new())
    }

    fn ladder_push_off_ran(ps: &PlayerState) -> bool {
        ps.jump_latched && ps.since_jump_ms == 0.0
    }

    #[test]
    fn accelerates_to_run_speed_cap() {
        let w = flat();
        let w = MoveWorld::bare(&w);
        let mut ps = PlayerState::spawn(Vec3::ZERO, 0.0);
        tick(
            &mut ps,
            &PmInput {
                forward: 1.0,
                ..Default::default()
            },
            &w,
            250,
        );
        let h = ps.velocity.truncate().length();
        assert!((h - SPEED_RUN).abs() < 5.0, "run speed {h}");
        // yaw 0 => moving along +X
        assert!(ps.velocity.x > 100.0 && ps.velocity.y.abs() < 1.0);
    }

    #[test]
    fn friction_stops_the_player() {
        let w = flat();
        let w = MoveWorld::bare(&w);
        let mut ps = PlayerState::spawn(Vec3::ZERO, 0.0);
        tick(
            &mut ps,
            &PmInput {
                forward: 1.0,
                ..Default::default()
            },
            &w,
            250,
        );
        tick(&mut ps, &PmInput::default(), &w, 200);
        assert!(
            ps.velocity.length() < 1.0,
            "still moving at {}",
            ps.velocity.length()
        );
    }

    #[test]
    fn ground_jump_apex_is_the_jump_height() {
        let w = flat();
        let w = MoveWorld::bare(&w);
        let run = PmInput {
            forward: 1.0,
            ..Default::default()
        };
        // retail: vz = sqrt(2 * 39 * g) standing (0x708c8)
        let mut ps = PlayerState::spawn(Vec3::ZERO, 0.0);
        tick(&mut ps, &PmInput::default(), &w, 50); // settle
        let mut apex = 0.0f32;
        let launch = PmInput {
            forward: 1.0,
            jump: true,
            ..Default::default()
        };
        // 20 ms steps take a whole 16 off the velocity per frame, so the
        // snap does not bend the arc (`a_125_fps_jump_goes_higher`).
        pmove(&mut ps, &launch, &w, 0.02, &[]);
        for _ in 0..80 {
            pmove(&mut ps, &run, &w, 0.02, &[]);
            apex = apex.max(ps.origin.z);
        }
        assert!(
            (apex - JUMP_HEIGHT).abs() < 2.0,
            "standing apex {apex}, expected ~{JUMP_HEIGHT}"
        );
        assert!(ps.on_ground);
    }

    /// `PM_CheckJump` refuses `pm_flags` 0x1 and 0x2 (0x2ebc8, 0x2ebf5): a
    /// crouched or prone player holding jump stays on the ground.
    #[test]
    fn neither_a_crouch_nor_a_prone_jumps() {
        let w = flat();
        let w = MoveWorld::bare(&w);
        for (crouch, prone) in [(true, false), (false, true)] {
            let hold = PmInput {
                crouch,
                prone,
                ..Default::default()
            };
            let mut ps = PlayerState::spawn(Vec3::ZERO, 0.0);
            tick(&mut ps, &hold, &w, 80); // settle into the stance
            assert_ne!(ps.stance, Stance::Stand);
            let hop = PmInput { jump: true, ..hold };
            for _ in 0..60 {
                pmove(&mut ps, &hop, &w, 1.0 / 125.0, &[]);
                assert!(ps.on_ground && !ps.jumped, "crouch {crouch} prone {prone}");
            }
        }
    }

    /// Retail sweeps a 12-unit box 54 units behind the facing before it lets a
    /// body lie down (docs/research/cod11-mantle.md, "Prone").
    #[test]
    fn prone_is_refused_when_the_body_has_no_room_behind_it() {
        // A wall 20 units behind the origin, well inside the 54 the body needs.
        let w = crate::collision::test_world(&[(
            Vec3::new(-40.0, -30.0, -8.0),
            Vec3::new(-20.0, 30.0, 72.0),
        )]);
        let w = MoveWorld::bare(&w);
        let mut ps = PlayerState::spawn(Vec3::ZERO, 0.0);
        tick(&mut ps, &PmInput::default(), &w, 20);
        let prone = PmInput {
            prone: true,
            ..Default::default()
        };
        // Facing +x puts the body in the wall behind; facing -x puts it in the
        // open, and the same input is then taken.
        ps.yaw = 0f32.to_radians();
        tick(&mut ps, &prone, &w, 20);
        assert_eq!(
            ps.stance,
            Stance::Stand,
            "prone into a wall must be refused"
        );

        ps.yaw = 180f32.to_radians();
        tick(&mut ps, &prone, &w, 20);
        assert_eq!(
            ps.stance,
            Stance::Prone,
            "prone with room behind must be taken"
        );
        assert!(
            normalize180(ps.prone_direction - 180.0).abs() < 0.01,
            "the body faces the view, at {}",
            ps.prone_direction
        );
    }

    /// Past the soft edge the body turns toward the view at 55 deg/s, and the
    /// view is held inside 85 degrees of the body by a correction the caller
    /// applies to `delta_angles`.
    #[test]
    fn a_prone_view_swings_the_body_and_is_capped() {
        let w = flat();
        let w = MoveWorld::bare(&w);
        let mut ps = PlayerState::spawn(Vec3::ZERO, 0.0);
        tick(&mut ps, &PmInput::default(), &w, 20);
        let prone = PmInput {
            prone: true,
            ..Default::default()
        };
        tick(&mut ps, &prone, &w, 20);
        assert_eq!(ps.stance, Stance::Prone);
        assert!((ps.prone_direction).abs() < 0.01);

        // Inside the soft edge the body does not move and nothing is clamped.
        ps.yaw = 60f32.to_radians();
        pmove(&mut ps, &prone, &w, 0.05, &[]);
        assert!(
            ps.prone_direction.abs() < 0.01,
            "the body holds under 80 degrees"
        );
        assert_eq!(ps.view_yaw_correction, 0.0, "60 degrees is inside the cap");

        // Past it, the body swings at the measured rate.
        ps.yaw = 150f32.to_radians();
        // A frame inside `MAX_FRAME_MS`, which pmove clamps dt to.
        pmove(&mut ps, &prone, &w, 0.05, &[]);
        // The body turns toward the view, which is at +150.
        let swung = PRONE_SWING_DEG_PER_SEC * 0.05;
        assert!(
            (ps.prone_direction - swung).abs() < 0.01,
            "body at {}, expected {swung}",
            ps.prone_direction
        );
        // 150 degrees off the body is past the 85 cap: the push is measured
        // off the body before the swing, the view placed on the cap after it.
        assert!(
            (ps.view_yaw_correction - (-150.0 + PRONE_YAWCAP)).abs() < 0.01,
            "correction {}",
            ps.view_yaw_correction
        );
        assert!(
            ((ps.yaw.to_degrees() - ps.prone_direction).abs() - PRONE_YAWCAP).abs() < 0.01,
            "the view ends exactly on the cap"
        );
    }

    /// A crawling player's body follows the view inside the soft edge, which
    /// a still one's does not (`PM_UpdateViewAngles` 0x330c0).
    #[test]
    fn a_crawl_swings_the_body_inside_the_soft_edge() {
        let w = flat();
        let w = MoveWorld::bare(&w);
        let mut ps = PlayerState::spawn(Vec3::ZERO, 0.0);
        tick(&mut ps, &PmInput::default(), &w, 20);
        let prone = PmInput {
            prone: true,
            ..Default::default()
        };
        tick(&mut ps, &prone, &w, 60);
        ps.yaw = 30f32.to_radians();
        let crawl = PmInput {
            right: 1.0,
            ..prone
        };
        pmove(&mut ps, &crawl, &w, 0.05, &[]);
        let swung = PRONE_SWING_DEG_PER_SEC * 0.05;
        assert!(
            (ps.prone_direction - swung).abs() < 0.01,
            "body at {}, expected {swung}",
            ps.prone_direction
        );
    }

    /// The prone view pitches at most 45 degrees off the ground's pitch along
    /// it, level ground here, and the excess goes to `delta_angles[0]`.
    #[test]
    fn a_prone_view_pitch_is_capped_off_the_ground() {
        let w = flat();
        let w = MoveWorld::bare(&w);
        let mut ps = PlayerState::spawn(Vec3::ZERO, 0.0);
        tick(&mut ps, &PmInput::default(), &w, 20);
        let prone = PmInput {
            prone: true,
            ..Default::default()
        };
        tick(&mut ps, &prone, &w, 60);
        assert_eq!(ps.prone_torso_pitch, 0.0);
        // Camera convention: positive up. 60 down is past the cap by 15.
        ps.pitch = (-60f32).to_radians();
        pmove(&mut ps, &prone, &w, 0.05, &[]);
        assert!((ps.pitch.to_degrees() + PRONE_PITCHCAP).abs() < 0.01);
        assert!(
            (ps.view_pitch_correction + 15.0).abs() < 0.01,
            "correction {}",
            ps.view_pitch_correction
        );
    }

    /// A prone press on a player moving forward throws it into the air at the
    /// standing dive's speed, and holds `pm_flags` 0x4 while the key is down.
    #[test]
    fn a_prone_press_while_running_dives() {
        let w = flat();
        let w = MoveWorld::bare(&w);
        let mut ps = PlayerState::spawn(Vec3::ZERO, 0.0);
        let run = PmInput {
            forward: 1.0,
            ..Default::default()
        };
        tick(&mut ps, &run, &w, 30);
        assert!(ps.on_ground);
        let dive = PmInput { prone: true, ..run };
        pmove(&mut ps, &dive, &w, 0.008, &[]);
        assert_eq!(ps.stance, Stance::Prone);
        assert!(ps.prone_dive);
        assert!(!ps.on_ground);
        let takeoff = (2.0 * DIVE_HEIGHT_STAND * GRAVITY).sqrt();
        assert!(
            (ps.velocity.z - (takeoff - GRAVITY * 0.008)).abs() < 1.0,
            "vz {}",
            ps.velocity.z
        );
        // A sideways press does not dive.
        let mut ps = PlayerState::spawn(Vec3::ZERO, 0.0);
        let strafe = PmInput {
            right: 1.0,
            ..Default::default()
        };
        tick(&mut ps, &strafe, &w, 30);
        pmove(
            &mut ps,
            &PmInput {
                prone: true,
                ..strafe
            },
            &w,
            0.008,
            &[],
        );
        assert!(!ps.prone_dive);
        assert!(ps.on_ground);
    }

    #[test]
    fn a_standing_jump_needs_no_forward_input() {
        // The forwardmove gate once read off fn 0x316F4 @0x31CC0 is not there:
        // a retail server jumps a probe holding upmove alone
        // (docs/research/cod11-mantle.md, "Jumps").
        let w = flat();
        let w = MoveWorld::bare(&w);
        let mut ps = PlayerState::spawn(Vec3::ZERO, 0.0);
        tick(&mut ps, &PmInput::default(), &w, 50); // settle
        let mut apex = ps.origin.z;
        for _ in 0..80 {
            pmove(
                &mut ps,
                &PmInput {
                    jump: true,
                    ..Default::default()
                },
                &w,
                0.02,
                &[],
            );
            apex = apex.max(ps.origin.z);
        }
        assert!(
            (apex - JUMP_HEIGHT).abs() < 2.0,
            "standing apex {apex}, expected ~{JUMP_HEIGHT}"
        );
    }

    /// `groundEntityNum` is the ground trace's entity: a player on a
    /// submodel carries the entity the server named for it, one on model 0
    /// the world's number, and one in the air none.
    #[test]
    fn the_ground_entity_is_the_ground_traces_entity() {
        let w = crate::collision::submodel_test_world(
            "{\n\"classname\" \"script_brushmodel\"\n\"model\" \"*1\"\n\"origin\" \"200 0 0\"\n}",
            &[([-32.0, -32.0, 0.0], [32.0, 32.0, 8.0])],
        );
        w.set_model_entity(1, 177);
        let w = MoveWorld::bare(&w);
        let mut ps = PlayerState::spawn(Vec3::new(200.0, 0.0, 9.0), 0.0);
        tick(&mut ps, &PmInput::default(), &w, 10);
        assert_eq!(ps.ground_entity_num(), 177);
        let mut ps = PlayerState::spawn(Vec3::new(0.0, 0.0, 1.0), 0.0);
        tick(&mut ps, &PmInput::default(), &w, 10);
        assert_eq!(ps.ground_entity_num(), ENTITYNUM_WORLD);
        ps.on_ground = false;
        assert_eq!(ps.ground_entity_num(), ENTITYNUM_NONE);
    }

    #[test]
    fn the_velocity_snap_rounds_to_nearest_even_and_stores_plus_zero() {
        let mut ps = PlayerState::spawn(Vec3::ZERO, 0.0);
        ps.velocity = Vec3::new(183.5, 26.5, -0.4);
        snap_velocity(&mut ps);
        assert_eq!(ps.velocity, Vec3::new(184.0, 26.0, 0.0));
        assert_eq!(ps.velocity.z.to_bits(), 0, "a snapped -0.4 is +0.0");
        ps.velocity = Vec3::new(-2.5, 137.6, -196.2);
        snap_velocity(&mut ps);
        assert_eq!(ps.velocity, Vec3::new(-2.0, 138.0, -196.0));
    }

    #[test]
    fn a_frame_that_moved_under_half_its_velocity_takes_the_move_as_velocity() {
        let mut ps = PlayerState::spawn(Vec3::new(1.0, 0.0, 0.0), 0.0);
        ps.move_start = Vec3::ZERO;
        // 1 unit in 8 ms is 125 u/s: 200 is under twice that and stays.
        ps.velocity = Vec3::new(200.0, 0.0, 0.0);
        clamp_velocity_to_move(&mut ps, 0.008);
        assert_eq!(ps.velocity.x, 200.0);
        ps.velocity = Vec3::new(300.0, 0.0, 40.0);
        clamp_velocity_to_move(&mut ps, 0.008);
        assert!(ps.velocity.abs_diff_eq(Vec3::new(125.0, 0.0, 0.0), 1e-3));
    }

    /// At 8 ms a frame's gravity is 6.4 and the snap keeps 6 of it, so a
    /// 125 fps jump tops out over two units above the 39 a 20 ms one reaches.
    #[test]
    fn a_125_fps_jump_goes_higher() {
        let apex = |dt: f32| {
            let w = flat();
            let w = MoveWorld::bare(&w);
            let mut ps = PlayerState::spawn(Vec3::ZERO, 0.0);
            tick(&mut ps, &PmInput::default(), &w, 50);
            let jump = PmInput {
                jump: true,
                ..Default::default()
            };
            pmove(&mut ps, &jump, &w, dt, &[]);
            let mut apex = ps.origin.z;
            for _ in 0..(1.6 / dt) as usize {
                pmove(&mut ps, &PmInput::default(), &w, dt, &[]);
                apex = apex.max(ps.origin.z);
            }
            apex
        };
        let (fast, slow) = (apex(0.008), apex(0.02));
        assert!((slow - JUMP_HEIGHT).abs() < 1.0, "20 ms apex {slow}");
        assert!(fast - slow > 1.5, "8 ms apex {fast}, 20 ms {slow}");
    }

    /// A held key jumps once: the latch (`pm_flags` 0x8) holds until the key
    /// is released, and a press inside 500 ms of the last jump is refused
    /// (`PM_CheckJump` 0x2ebb3, 0x2ec0d).
    #[test]
    fn a_held_jump_does_not_chain_and_a_repress_waits_out_the_cooldown() {
        let w = flat();
        let w = MoveWorld::bare(&w);
        let dt = 1.0 / 125.0;
        let hop = PmInput {
            forward: 1.0,
            jump: true,
            ..Default::default()
        };
        let run = PmInput { jump: false, ..hop };
        let mut ps = PlayerState::spawn(Vec3::ZERO, 0.0);
        tick(&mut ps, &PmInput::default(), &w, 50); // settle
        let mut launches = 0;
        for _ in 0..300 {
            pmove(&mut ps, &hop, &w, dt, &[]);
            launches += usize::from(ps.jumped);
        }
        assert_eq!(launches, 1, "a held key jumps once");
        assert!(ps.on_ground && ps.jump_latched);

        // Released for a frame and pressed again, well past 500 ms: jumps.
        pmove(&mut ps, &run, &w, dt, &[]);
        assert!(!ps.jump_latched);
        pmove(&mut ps, &hop, &w, dt, &[]);
        assert!(ps.jumped);

        // Land, then re-press inside the cooldown: a full flight outlasts the
        // 500 ms, so the jump is cut short by taking its vertical speed away.
        let mut ps = PlayerState::spawn(Vec3::ZERO, 0.0);
        tick(&mut ps, &PmInput::default(), &w, 50);
        pmove(&mut ps, &hop, &w, dt, &[]);
        assert!(ps.jumped);
        ps.velocity.z = 0.0;
        tick(&mut ps, &run, &w, 20);
        assert!(ps.on_ground, "landed");
        assert!(ps.since_jump_ms < JUMP_COOLDOWN_MS);
        pmove(&mut ps, &hop, &w, dt, &[]);
        assert!(!ps.jumped, "a jump inside 500 ms of the last is refused");
        while ps.since_jump_ms <= JUMP_COOLDOWN_MS - 1.0 {
            pmove(&mut ps, &run, &w, dt, &[]);
        }
        pmove(&mut ps, &hop, &w, dt, &[]);
        assert!(ps.jumped, "and taken once the 500 ms are up");
    }

    /// The step tail: the vertical jump the step added rides `EV_STEP_VIEW`
    /// with a +128 bias, and the frame's velocity is scaled by how much of
    /// the step height it used (docs/research/cod11-mantle.md, "The step
    /// event and the velocity scale").
    #[test]
    fn a_step_announces_itself_and_costs_speed() {
        let w = test_world(&[(Vec3::new(50.0, -200.0, 0.0), Vec3::new(1024.0, 200.0, 6.0))]);
        let w = MoveWorld::bare(&w);
        let mut ps = PlayerState::spawn(Vec3::new(30.0, 0.0, 0.0), 0.0);
        ps.on_ground = true;
        ps.ground_normal = Vec3::Z;
        let mut events = Vec::new();
        for _ in 0..20 {
            ps.velocity = Vec3::new(200.0, 0.0, 0.0);
            step_slide_move(
                &mut ps,
                &w,
                1.0 / 125.0,
                false,
                MASK_PLAYERSOLID,
                Some(&mut events),
            );
            if !events.is_empty() {
                break;
            }
        }
        assert_eq!(
            events,
            vec![PmEvent {
                event: EV_STEP_VIEW,
                parm: 128 + 6,
            }],
            "a 6-unit step, biased by 128"
        );
        // 0.2 + 0.8 * (1 - 6/18)
        assert!(
            (ps.velocity.x - 200.0 * (0.2 + 0.8 * (1.0 - 6.0 / STEPSIZE))).abs() < 0.5,
            "the step cost a third of the frame's speed, got {}",
            ps.velocity.x
        );
    }

    /// The tail is out of reach in the air: retail only gets there on a
    /// grounded frame (@0x350ec).
    #[test]
    fn an_airborne_step_announces_nothing() {
        let w = test_world(&[(Vec3::new(50.0, -200.0, 0.0), Vec3::new(1024.0, 200.0, 6.0))]);
        let w = MoveWorld::bare(&w);
        let mut ps = PlayerState::spawn(Vec3::new(30.0, 0.0, 0.0), 0.0);
        let mut events = Vec::new();
        for _ in 0..20 {
            ps.velocity = Vec3::new(200.0, 0.0, 0.0);
            step_slide_move(
                &mut ps,
                &w,
                1.0 / 125.0,
                false,
                MASK_PLAYERSOLID,
                Some(&mut events),
            );
        }
        assert!(events.is_empty(), "{events:?}");
    }

    /// In the air only a jump steps, and never past its origin
    /// (`PM_StepSlideMove` 0x35057-0x35112, 0x35450; docs/research/cod11-mantle.md,
    /// "The jump's step").
    #[test]
    fn an_airborne_step_needs_a_jump_and_stops_at_its_origin() {
        // A ledge 14 high 20 units ahead of an airborne player at 10.
        let w = test_world(&[(Vec3::new(50.0, -200.0, 0.0), Vec3::new(1024.0, 200.0, 14.0))]);
        let w = MoveWorld::bare(&w);
        let fly = |jump_origin_z: f32| {
            let mut ps = PlayerState::spawn(Vec3::new(30.0, 0.0, 10.0), 0.0);
            ps.jump_origin_z = jump_origin_z;
            let mut events = Vec::new();
            ps.velocity = Vec3::new(200.0, 0.0, 0.0);
            step_slide_move(
                &mut ps,
                &w,
                0.05,
                false,
                MASK_PLAYERSOLID,
                Some(&mut events),
            );
            (ps, events)
        };
        // A fall, no jump: held at the face.
        let (ps, events) = fly(0.0);
        assert!(
            ps.origin.z == 10.0 && ps.origin.x < 36.0,
            "at {}",
            ps.origin
        );
        assert!(events.is_empty());
        // Under a jump's origin: steps onto the ledge and announces it.
        let (ps, events) = fly(39.0);
        assert!(
            (ps.origin.z - 14.0).abs() < 0.2 && ps.origin.x > 36.0,
            "at {}",
            ps.origin
        );
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event, EV_STEP_VIEW);
        // An origin the ledge top is past: the step is reverted.
        let (ps, _) = fly(13.0);
        assert!(
            ps.origin.z == 10.0 && ps.origin.x < 36.0,
            "at {}",
            ps.origin
        );
        // Already above the origin: no step at all.
        let (ps, _) = fly(9.0);
        assert!(
            ps.origin.z == 10.0 && ps.origin.x < 36.0,
            "at {}",
            ps.origin
        );
    }

    #[test]
    fn prone_steps_lower_than_standing() {
        // retail picks the 10-unit step height off pm_flags bit 0x1
        // (PM_StepSlideMove @0x35045), 18 otherwise (@0x35034)
        let w = test_world(&[(Vec3::new(50.0, -200.0, 0.0), Vec3::new(1024.0, 200.0, 14.0))]);
        let w = MoveWorld::bare(&w);
        let mut stand = PlayerState::spawn(Vec3::new(30.0, 0.0, 0.0), 0.0);
        stand.on_ground = true;
        stand.velocity = Vec3::new(200.0, 0.0, 0.0);
        for _ in 0..20 {
            stand.velocity.x = 200.0;
            step_slide_move(&mut stand, &w, 1.0 / 125.0, false, MASK_PLAYERSOLID, None);
        }
        assert!(
            (stand.origin.z - 14.0).abs() < 0.5,
            "standing should step 14, at {}",
            stand.origin
        );

        let mut prone = PlayerState::spawn(Vec3::new(30.0, 0.0, 0.0), 0.0);
        prone.on_ground = true;
        prone.stance = Stance::Prone;
        for _ in 0..20 {
            prone.velocity.x = 200.0;
            step_slide_move(&mut prone, &w, 1.0 / 125.0, false, MASK_PLAYERSOLID, None);
        }
        assert!(
            prone.origin.z < 1.0 && prone.origin.x < 36.0,
            "prone must not step 14, at {}",
            prone.origin
        );
    }

    #[test]
    fn ankle_deep_water_wades_at_retails_scale() {
        // retail has no wading wish clamp: nothing references
        // pm_waterSwimScale/pm_waterWadeScale and the walk mover (0x2F03C)
        // never reads water level - only the water-friction term slows you
        let w = pool_world();
        let w = MoveWorld::bare(&w);
        let run = PmInput {
            forward: 1.0,
            ..Default::default()
        };
        let mut dry = PlayerState::spawn(Vec3::new(-400.0, 0.0, 1.0), 0.0);
        tick(&mut dry, &run, &w, 100);
        let mut wet = PlayerState::spawn(Vec3::new(-150.0, 200.0, 30.2), 0.0);
        tick(&mut wet, &run, &w, 100);
        assert_eq!(wet.water_level, 1);
        let dry_speed = dry.velocity.truncate().length();
        let wet_speed = wet.velocity.truncate().length();
        // Retail's cmd scale wades at `1 - waterlevel / 3 * 0.5`
        // (cod11-mantle.md, "The wish speed").
        assert!(
            (wet_speed - dry_speed * (1.0 - WADE_SCALE / 3.0)).abs() < 2.0,
            "ankle-deep run {wet_speed} vs dry {dry_speed}"
        );
    }

    /// A slope is not a launch ramp. Retail's `PM_StepSlideMove` (0x34fbc)
    /// takes its down pass on every grounded frame and pushes half a step
    /// size past the step it took, so a walker is pulled back onto the ground
    /// at a crest instead of floating off it.
    #[test]
    fn walking_a_slope_never_leaves_the_ground() {
        // 5 degrees over 300 units, then level ground at the top: the crest
        // is what a walker used to launch off.
        let w = crate::collision::ramp_test_world(5.0, 0.0, 300.0);
        let w = MoveWorld::bare(&w);
        let mut ps = PlayerState::spawn(Vec3::new(-200.0, 0.0, 1.0), 0.0);
        tick(&mut ps, &PmInput::default(), &w, 40);
        assert!(ps.on_ground, "the walker never settled on the floor");
        let mut airborne = 0;
        let forward = PmInput {
            forward: 1.0,
            ..Default::default()
        };
        for _ in 0..500 {
            pmove(&mut ps, &forward, &w, 1.0 / 125.0, &[]);
            if !ps.on_ground {
                airborne += 1;
            }
        }
        assert!(
            ps.origin.x > 320.0,
            "the walk never crossed the crest, at {}",
            ps.origin
        );
        assert_eq!(
            airborne, 0,
            "the walk left the ground {airborne} frames, ending at {}",
            ps.origin
        );
    }

    /// Standing on a slope holds its height: the ground trace, the friction
    /// and the gravity must settle rather than trade the player up and down.
    #[test]
    fn standing_on_a_slope_holds_its_height() {
        let w = crate::collision::ramp_test_world(5.0, 0.0, 300.0);
        let w = MoveWorld::bare(&w);
        let mut ps = PlayerState::spawn(Vec3::new(150.0, 0.0, 20.0), 0.0);
        tick(&mut ps, &PmInput::default(), &w, 60);
        let (z, ground) = (ps.origin.z, ps.on_ground);
        assert!(ground, "the stander is not on the slope at {}", ps.origin);
        for _ in 0..100 {
            pmove(&mut ps, &PmInput::default(), &w, 1.0 / 125.0, &[]);
            assert_eq!(ps.on_ground, ground, "the ground flipped under a stander");
            assert!(
                (ps.origin.z - z).abs() < 0.01,
                "a stander drifted from {z} to {}",
                ps.origin.z
            );
        }
    }

    #[test]
    fn steps_up_low_ledge_but_not_high_one() {
        // z=16 ledge ahead (+X), z=40 ledge behind (-X), both reaching the
        // floor edge: 300 ticks covers ~440 units
        let w = test_world(&[
            (Vec3::new(50.0, -200.0, 0.0), Vec3::new(1024.0, 200.0, 16.0)),
            (
                Vec3::new(-1024.0, -200.0, 0.0),
                Vec3::new(-50.0, 200.0, 40.0),
            ),
        ]);
        let w = MoveWorld::bare(&w);
        let mut ps = PlayerState::spawn(Vec3::ZERO, 0.0);
        tick(
            &mut ps,
            &PmInput {
                forward: 1.0,
                ..Default::default()
            },
            &w,
            300,
        );
        assert!(
            ps.origin.x > 60.0 && (ps.origin.z - 16.0).abs() < 0.5,
            "should stand on the ledge, at {}",
            ps.origin
        );

        let mut ps = PlayerState::spawn(Vec3::ZERO, 180.0);
        tick(
            &mut ps,
            &PmInput {
                forward: 1.0,
                ..Default::default()
            },
            &w,
            300,
        );
        assert!(
            ps.origin.x > -50.0 - HALF_WIDTH - 1.0 && ps.origin.z < 1.0,
            "blocked by the wall, at {}",
            ps.origin
        );
    }

    #[test]
    fn slides_along_wall() {
        let w = test_world(&[(Vec3::new(50.0, -400.0, 0.0), Vec3::new(100.0, 400.0, 100.0))]);
        let w = MoveWorld::bare(&w);
        let mut ps = PlayerState::spawn(Vec3::ZERO, 0.0);
        tick(
            &mut ps,
            &PmInput {
                forward: 1.0,
                right: 0.3,
                ..Default::default()
            },
            &w,
            400,
        );
        assert!(ps.origin.x < 50.0 - HALF_WIDTH + 1.0);
        // retail constants (accel 9, stopspeed floor 100) give a lower
        // tangential plateau than the old Q3-derived ones
        assert!(
            ps.origin.y.abs() > 30.0,
            "should have slid along the wall, at {}",
            ps.origin
        );
    }

    #[test]
    fn crouch_lowers_speed_and_ceiling_blocks_standing() {
        // z=60 ceiling: crouch fits, standing does not; wide enough for the
        // ~240-unit run
        let w = test_world(&[(
            Vec3::new(-1024.0, -1024.0, 60.0),
            Vec3::new(1024.0, 1024.0, 100.0),
        )]);
        let w = MoveWorld::bare(&w);
        let mut ps = PlayerState::spawn(Vec3::ZERO, 0.0);
        ps.stance = Stance::Crouch;
        ps.ducked = true;
        let crouched = PmInput {
            forward: 1.0,
            crouch: true,
            ..Default::default()
        };
        tick(&mut ps, &crouched, &w, 250);
        let h = ps.velocity.truncate().length();
        assert!(
            (h - SPEED_RUN * SCALE_CROUCH).abs() < 5.0,
            "crouch speed {h}"
        );
        tick(&mut ps, &PmInput::default(), &w, 10);
        assert_eq!(ps.stance, Stance::Crouch);
    }

    #[test]
    fn prone_is_slowest() {
        let w = flat();
        let w = MoveWorld::bare(&w);
        let mut ps = PlayerState::spawn(Vec3::ZERO, 0.0);
        let prone = PmInput {
            forward: 1.0,
            prone: true,
            ..Default::default()
        };
        tick(&mut ps, &prone, &w, 400);
        let h = ps.velocity.truncate().length();
        assert!((h - SPEED_RUN * SCALE_PRONE).abs() < 3.0, "prone speed {h}");
    }

    #[test]
    fn lean_ramps_up_clamps_and_returns() {
        let w = flat();
        let w = MoveWorld::bare(&w);
        let mut ps = PlayerState::spawn(Vec3::ZERO, 0.0);
        tick(&mut ps, &PmInput::default(), &w, 50); // settle
        let lean_r = PmInput {
            lean_right: true,
            ..Default::default()
        };
        // 280 ms to full lean; run 500 ms
        tick(&mut ps, &lean_r, &w, 63);
        assert!((ps.lean - LEAN_MAX).abs() < 0.5, "lean {}", ps.lean);
        // full return within 350 ms; run 500 ms
        tick(&mut ps, &PmInput::default(), &w, 63);
        assert!(ps.lean.abs() < 0.5, "lean {}", ps.lean);
    }

    #[test]
    fn lean_left_is_negative_and_wall_limits_lean() {
        // Wall on +Y (the left side at yaw 0), past y=12 so the lean box does
        // not start inside it, which would pin fraction at 0.
        let w = test_world(&[(Vec3::new(-200.0, 20.0, 0.0), Vec3::new(200.0, 70.0, 200.0))]);
        let w = MoveWorld::bare(&w);
        let mut ps = PlayerState::spawn(Vec3::ZERO, 0.0);
        tick(&mut ps, &PmInput::default(), &w, 50);
        let lean_l = PmInput {
            lean_left: true,
            ..Default::default()
        };
        tick(&mut ps, &lean_l, &w, 125);
        assert!(ps.lean < 0.0);
        assert!(
            ps.lean > -LEAN_MAX + 1.0,
            "wall should limit lean, got {}",
            ps.lean
        );
    }

    #[test]
    fn prone_blocks_lean() {
        let w = flat();
        let w = MoveWorld::bare(&w);
        let mut ps = PlayerState::spawn(Vec3::ZERO, 0.0);
        let input = PmInput {
            prone: true,
            lean_right: true,
            ..Default::default()
        };
        tick(&mut ps, &input, &w, 125);
        assert_eq!(ps.lean, 0.0);
    }

    #[test]
    fn walks_on_mp_pavlov_terrain() {
        let Some(data) = crate::testing::real_bsp() else {
            return;
        };
        let bsp = crate::bsp::parse(&data).unwrap();
        let world = CollisionWorld::build(&bsp, &[]);
        let world = MoveWorld::bare(&world);
        let (origin, yaw) = crate::bsp::find_spawn(&bsp.entities).unwrap();
        let mut ps = PlayerState::spawn(Vec3::from(origin) + Vec3::Z * 2.0, yaw);
        // must land on terrain triangles near the spawn, not fall to bedrock
        tick(&mut ps, &PmInput::default(), &world, 250);
        assert!(ps.on_ground, "player should land");
        assert!(
            (ps.origin.z - origin[2]).abs() < 40.0,
            "landed at z={}, spawn z={}",
            ps.origin.z,
            origin[2]
        );
        let start = ps.origin;
        tick(
            &mut ps,
            &PmInput {
                forward: 1.0,
                ..Default::default()
            },
            &world,
            500,
        );
        assert!(
            ps.origin.truncate().distance(start.truncate()) > 100.0,
            "should cover ground"
        );
        assert!(
            ps.origin.z > -100.0,
            "must not fall through the map, z={}",
            ps.origin.z
        );
    }

    #[test]
    fn view_reflects_stance_and_lean() {
        let w = flat();
        let w = MoveWorld::bare(&w);
        let mut ps = PlayerState::spawn(Vec3::ZERO, 0.0);
        tick(&mut ps, &PmInput::default(), &w, 50);
        let v = ps.view();
        assert!((v.eye.z - VIEW_STAND).abs() < 1.0);
        assert_eq!(v.roll, 0.0);
        tick(
            &mut ps,
            &PmInput {
                lean_right: true,
                ..Default::default()
            },
            &w,
            125,
        );
        let v = ps.view();
        assert!(v.roll > 0.1, "leaning right should roll the view");
        assert!(v.eye.y < -1.0, "yaw 0 lean right offsets eye toward -Y");
    }

    // --- water ---

    #[test]
    fn water_level_tracks_depth() {
        let w = pool_world();
        let w = MoveWorld::bare(&w);
        // deep floor: only the eye stays above the surface? no - fully under
        let mut ps = PlayerState::spawn(Vec3::new(0.0, -200.0, -73.0), 0.0);
        tick(&mut ps, &PmInput::default(), &w, 5);
        assert_eq!(ps.water_level, 3, "on the pool bottom at {}", ps.origin);

        // chest-deep shelf: feet+1 and waist wet, eyes dry
        let mut ps = PlayerState::spawn(Vec3::new(150.0, 0.0, 0.2), 0.0);
        tick(&mut ps, &PmInput::default(), &w, 5);
        assert_eq!(ps.water_level, 2, "on the shelf at {}", ps.origin);

        // ankle-deep shelf
        let mut ps = PlayerState::spawn(Vec3::new(0.0, 200.0, 30.2), 0.0);
        tick(&mut ps, &PmInput::default(), &w, 5);
        assert_eq!(ps.water_level, 1, "ankle-deep at {}", ps.origin);

        // dry ground west of the pool
        let mut ps = PlayerState::spawn(Vec3::new(-400.0, 0.0, 1.0), 0.0);
        tick(&mut ps, &PmInput::default(), &w, 5);
        assert_eq!(ps.water_level, 0);
    }

    #[test]
    fn swimming_caps_at_the_swim_speed() {
        let w = pool_world();
        let w = MoveWorld::bare(&w);
        let mut ps = PlayerState::spawn(Vec3::new(0.0, -200.0, -20.0), 0.0);
        tick(
            &mut ps,
            &PmInput {
                forward: 1.0,
                ..Default::default()
            },
            &w,
            100,
        );
        let h = ps.velocity.truncate().length();
        assert!(
            (h - SPEED_RUN * SCALE_SWIM).abs() < 6.0,
            "swim speed {h}, expected ~{}",
            SPEED_RUN * SCALE_SWIM
        );
    }

    #[test]
    fn idle_player_sinks_without_freefall() {
        let w = pool_world();
        let w = MoveWorld::bare(&w);
        let mut ps = PlayerState::spawn(Vec3::new(0.0, -200.0, -20.0), 0.0);
        let start = ps.origin.z;
        let input = PmInput::default();
        // the sink wish is an acceleration target, not a velocity cap: speed
        // builds over ~half a second, so watch the whole descent
        let mut worst_fall = 0.0f32;
        for _ in 0..150 {
            pmove(&mut ps, &input, &w, 1.0 / 125.0, &[]);
            worst_fall = worst_fall.max(-ps.velocity.z);
        }
        assert!(
            ps.origin.z < start - 40.0,
            "should reach the bottom, at {}",
            ps.origin
        );
        assert!(ps.on_ground, "settled on the pool floor at {}", ps.origin);
        assert!(worst_fall < 130.0, "sink must not freefall: {worst_fall}");
    }

    #[test]
    fn jump_key_swims_up() {
        let w = pool_world();
        let w = MoveWorld::bare(&w);
        let mut ps = PlayerState::spawn(Vec3::new(0.0, -200.0, -30.0), 0.0);
        tick(
            &mut ps,
            &PmInput {
                jump: true,
                ..Default::default()
            },
            &w,
            150,
        );
        // hovers chest-deep: above that line the waist sample dries, swim
        // gives way to air, and he sinks back into it
        assert!(
            (-6.0..15.0).contains(&ps.origin.z),
            "should bob at the chest line, at {}",
            ps.origin
        );
    }

    #[test]
    fn looking_up_while_submerged_walk_turns_into_swim() {
        let w = pool_world();
        let w = MoveWorld::bare(&w);
        let mut ps = PlayerState::spawn(Vec3::new(0.0, -200.0, -73.0), 0.0);
        ps.pitch = 20.0f32.to_radians();
        tick(
            &mut ps,
            &PmInput {
                forward: 1.0,
                ..Default::default()
            },
            &w,
            30,
        );
        assert!(
            ps.velocity.z > 5.0 || ps.origin.z > -60.0,
            "should swim up off the bottom: vz {} z {}",
            ps.velocity.z,
            ps.origin.z
        );
    }

    // --- ladders ---

    /// Dry floor at z=0 plus a playerclip ladder wall (SURF_LADDER) spanning
    /// x 50..54, y -200..200, z 0..220.
    fn ladder_world() -> CollisionWorld {
        crate::collision::synthetic_world(
            &[
                ("textures/test/solid", crate::collision::CONTENTS_SOLID, 0),
                // CONTENTS_PLAYERCLIP is private to collision.rs
                ("textures/common/ladder", 0x10000, 0x8),
            ],
            &[
                (0, [-600.0, -300.0, -16.0], [600.0, 300.0, 0.0]),
                (1, [50.0, -200.0, 0.0], [54.0, 200.0, 220.0]),
            ],
        )
    }

    #[test]
    fn grabs_a_ladder_and_climbs_it_holding_forward() {
        let w = ladder_world();
        let w = MoveWorld::bare(&w);
        let mut ps = PlayerState::spawn(Vec3::new(20.0, 0.0, 0.2), 0.0);
        tick(
            &mut ps,
            &PmInput {
                forward: 1.0,
                ..Default::default()
            },
            &w,
            150,
        );
        assert!(ps.on_ladder, "should be on the ladder at {}", ps.origin);
        // hugging the face: center stops a half-width short of x=50
        assert!(
            (30.0..40.0).contains(&ps.origin.x),
            "should press against the face, at {}",
            ps.origin
        );
        assert!(ps.origin.z > 30.0, "should have climbed, at {}", ps.origin);
        assert!(ps.velocity.z > 20.0, "climb vz {}", ps.velocity.z);
    }

    #[test]
    fn no_ladder_grab_when_idle_or_backing_off() {
        let w = ladder_world();
        let w = MoveWorld::bare(&w);
        let mut ps = PlayerState::spawn(Vec3::new(33.0, 0.0, 0.2), 0.0);
        tick(&mut ps, &PmInput::default(), &w, 30);
        assert!(!ps.on_ladder, "idle at the base must not grab");
        tick(
            &mut ps,
            &PmInput {
                forward: -1.0,
                ..Default::default()
            },
            &w,
            30,
        );
        assert!(!ps.on_ladder, "backing away must not grab");
        assert!(ps.origin.x < 30.0, "moved away, at {}", ps.origin);
    }

    #[test]
    fn climb_rate_follows_pitch() {
        let w = ladder_world();
        let w = MoveWorld::bare(&w);
        let input = PmInput {
            forward: 1.0,
            ..Default::default()
        };
        // two units off the face; compare settled climb speeds (retail
        // equilibrium: accel 9 fighting friction 16 gives ~0.56 * wish)
        let mut level = PlayerState::spawn(Vec3::new(33.0, 0.0, 5.0), 0.0);
        tick(&mut level, &input, &w, 100);
        let mut up = PlayerState::spawn(Vec3::new(33.0, 0.0, 5.0), 0.0);
        up.pitch = 30.0f32.to_radians();
        tick(&mut up, &input, &w, 100);
        assert!(level.on_ladder && up.on_ladder);
        assert!(
            level.velocity.z > 25.0 && level.velocity.z < 40.0,
            "level climb vz {}",
            level.velocity.z
        );
        assert!(
            up.velocity.z - level.velocity.z > 15.0,
            "looking up must climb faster: up {}, level {}",
            up.velocity.z,
            level.velocity.z
        );
        assert!(up.origin.z > level.origin.z);
    }

    #[test]
    fn diagonal_input_climbs_slower_than_pure_forward() {
        // retail runs the wish through PM_CmdScale, so W+D must not beat the
        // vertical rate of W alone (up 30 deg for a climb-dominated wish)
        let w = ladder_world();
        let w = MoveWorld::bare(&w);
        let climb = |right: f32| {
            let mut ps = PlayerState::spawn(Vec3::new(33.0, 0.0, 5.0), 0.0);
            ps.pitch = 30.0f32.to_radians();
            tick(
                &mut ps,
                &PmInput {
                    forward: 1.0,
                    right,
                    ..Default::default()
                },
                &w,
                100,
            );
            assert!(ps.on_ladder);
            ps.velocity.z
        };
        let solo = climb(0.0);
        let diag = climb(1.0);
        let ratio = diag / solo;
        // Unsnapped the ratio is the cmd scale's 1/sqrt(2); at 8 ms steps the
        // velocity snap takes a rounding share off both terminal speeds,
        // 53.4 -> 50 and 37.8 -> 34.
        assert!(
            diag < solo && (0.6..0.95).contains(&ratio),
            "W+D must climb slower than W alone, {solo} -> {diag} (x{ratio})"
        );
    }

    #[test]
    fn releasing_input_mid_climb_hangs_without_sliding() {
        let w = ladder_world();
        let w = MoveWorld::bare(&w);
        let mut ps = PlayerState::spawn(Vec3::new(20.0, 0.0, 0.2), 0.0);
        tick(
            &mut ps,
            &PmInput {
                forward: 1.0,
                ..Default::default()
            },
            &w,
            100,
        );
        let hang_z = ps.origin.z;
        tick(&mut ps, &PmInput::default(), &w, 50);
        assert!(ps.on_ladder, "must stay on the ladder while hanging");
        assert!(
            ps.velocity.z.abs() < 5.0,
            "vertical speed should damp to zero, vz {}",
            ps.velocity.z
        );
        assert!(
            ps.origin.z > hang_z - 8.0,
            "must not slide down, {} -> {}",
            hang_z,
            ps.origin.z
        );
    }

    #[test]
    fn ladder_probe_box_is_shrunk() {
        // retail shrinks the probe bbox horizontally (@0x70cb0), so a
        // sideways hover this close to the wall must NOT grab
        let w = ladder_world();
        let w = MoveWorld::bare(&w);
        let input = PmInput {
            forward: 1.0,
            ..Default::default()
        };
        let mut ps = PlayerState::spawn(Vec3::new(8.0, 0.0, 40.0), 0.0);
        pmove(&mut ps, &input, &w, 1.0 / 125.0, &[]);
        assert!(!ps.on_ladder, "full-box probe would grab from here");

        // just past the shrunken reach it still grabs
        let mut ps = PlayerState::spawn(Vec3::new(13.0, 0.0, 40.0), 0.0);
        pmove(&mut ps, &input, &w, 1.0 / 125.0, &[]);
        assert!(ps.on_ladder, "should grab within shrunk reach");
    }

    #[test]
    fn facing_away_mid_climb_keeps_the_grab() {
        let w = ladder_world();
        let w = MoveWorld::bare(&w);
        let run = PmInput {
            forward: 1.0,
            ..Default::default()
        };
        let mut ps = PlayerState::spawn(Vec3::new(20.0, 0.0, 0.2), 0.0);
        for _ in 0..200 {
            pmove(&mut ps, &run, &w, 1.0 / 125.0, &[]);
            if ps.on_ladder && ps.origin.z > 25.0 {
                break;
            }
        }
        assert!(ps.on_ladder && ps.origin.z > 25.0, "set up: {}", ps.origin);
        assert!(
            (ps.ladder_normal + Vec3::X).length() < 0.01,
            "stored normal {:?}",
            ps.ladder_normal
        );

        // turn around mid-climb: retail probes along -vLadderVec, so the
        // grab survives facing away while airborne
        ps.yaw += std::f32::consts::PI;
        let hang_z = ps.origin.z;
        tick(&mut ps, &run, &w, 10);
        assert!(ps.on_ladder, "must keep the grab facing away");
        assert!(
            ps.origin.z > hang_z - 5.0,
            "must not fall off, {} -> {}",
            hang_z,
            ps.origin.z
        );
    }

    #[test]
    fn dropping_onto_the_ladder_from_above_catches() {
        let w = ladder_world();
        let w = MoveWorld::bare(&w);
        // past the face plane, above the wall top region, falling; facing the
        // wall so the first trace hits it more than a unit away and the
        // grab-from-above guard pushes back into it
        let mut ps = PlayerState::spawn(Vec3::new(75.0, 0.0, 80.0), 180.0);
        let input = PmInput {
            forward: 1.0,
            ..Default::default()
        };
        let mut grabbed = false;
        for _ in 0..150 {
            pmove(&mut ps, &input, &w, 1.0 / 125.0, &[]);
            grabbed |= ps.on_ladder;
        }
        assert!(grabbed, "should catch the ladder, at {}", ps.origin);
        // pressed against the face (54 + half-width ~ 69) and climbing, not
        // fallen to the floor
        assert!(
            ps.origin.x < 72.0 && ps.origin.z > 60.0,
            "pushed into the wall instead of falling, at {}",
            ps.origin
        );
    }

    #[test]
    fn jump_gate_matrix() {
        // PM_CheckJump gates @0x2ebb3-0x2ec13: 500 ms since the last jump,
        // standing, jump key released since the previous one
        let n = Vec3::new(-1.0, 0.0, 0.0);
        let mk = |stance: Stance| {
            let mut ps = PlayerState::spawn(Vec3::ZERO, 0.0);
            ps.stance = stance;
            ps.ducked = stance == Stance::Crouch;
            ps.since_jump_ms = 10000.0;
            ps
        };
        let jump = PmInput {
            jump: true,
            ..Default::default()
        };
        let try_jump = |ps: &mut PlayerState, input: &PmInput, ladder: Option<Vec3>| {
            let mut cmd = *input;
            check_jump(ps, &mut cmd, ladder, &mut Vec::new())
        };

        // Off the ground: vz = sqrt(78 * g) = 249.8, the horizontal kept and
        // the jump origin 39 up.
        let mut ps = mk(Stance::Stand);
        ps.velocity = Vec3::new(100.0, 20.0, 0.0);
        assert!(try_jump(&mut ps, &jump, None));
        assert!((ps.velocity.z - 249.8).abs() < 0.05, "vz {}", ps.velocity.z);
        assert_eq!((ps.velocity.x, ps.velocity.y), (100.0, 20.0));
        assert_eq!(ps.jump_origin_z, JUMP_HEIGHT);
        assert!(ps.jump_latched && !ps.on_ground);
        assert_eq!(ps.aim_spread_scale, JUMP_SPREAD_ADD);

        // Off a ladder, facing straight into the wall (yaw 0, normal -1):
        // reflection sends the push straight back; vz = 249.8 * 0.75 = 187.35
        let mut ps = mk(Stance::Stand);
        ps.on_ladder = true;
        assert!(try_jump(&mut ps, &jump, Some(n)));
        assert!((ps.velocity.z - 187.35).abs() < 0.1, "vz {}", ps.velocity.z);
        assert!((ps.velocity.x + 128.0).abs() < 0.1, "vx {}", ps.velocity.x);
        assert!(ps.velocity.y.abs() < 0.1);
        assert!(!ps.on_ladder);

        for ladder in [None, Some(n)] {
            let mut ps = mk(Stance::Stand);
            ps.since_jump_ms = JUMP_COOLDOWN_MS - 1.0;
            assert!(!try_jump(&mut ps, &jump, ladder), "inside the cooldown");

            // boundary: retail allows at delta > 499, so exactly 500 passes
            let mut ps = mk(Stance::Stand);
            ps.since_jump_ms = JUMP_COOLDOWN_MS;
            assert!(try_jump(&mut ps, &jump, ladder), "500 ms allows");

            // a held key is refused and taken off the cmd
            let mut ps = mk(Stance::Stand);
            ps.jump_latched = true;
            let mut cmd = jump;
            assert!(
                !check_jump(&mut ps, &mut cmd, ladder, &mut Vec::new()),
                "held key must release first"
            );
            assert!(!cmd.jump);

            let mut ps = mk(Stance::Crouch);
            assert!(!try_jump(&mut ps, &jump, ladder), "crouch refuses");
            let mut ps = mk(Stance::Prone);
            assert!(!try_jump(&mut ps, &jump, ladder), "prone refuses");

            let mut ps = mk(Stance::Stand);
            assert!(
                !try_jump(&mut ps, &PmInput::default(), ladder),
                "no jump key"
            );
        }
    }

    #[test]
    fn ladder_push_off_leaves_the_wall_with_reflected_forward() {
        let w = ladder_world();
        let w = MoveWorld::bare(&w);
        let climb = PmInput {
            forward: 1.0,
            ..Default::default()
        };
        let mut ps = PlayerState::spawn(Vec3::new(20.0, 0.0, 0.2), 0.0);
        for _ in 0..200 {
            pmove(&mut ps, &climb, &w, 1.0 / 125.0, &[]);
            if ps.on_ladder && ps.origin.z > 25.0 {
                break;
            }
        }
        assert!(ps.on_ladder && ps.origin.z > 25.0, "set up: {}", ps.origin);

        // facing straight into the wall (yaw 0, wall face at x=50): the
        // reflection (@0x2eca2-0x2ed36) sends the push straight back out
        pmove(
            &mut ps,
            &PmInput {
                forward: 1.0,
                jump: true,
                ..Default::default()
            },
            &w,
            1.0 / 125.0,
            &[],
        );
        assert!(!ps.on_ladder, "push-off must clear the ladder");
        // impulse values pinned in the gate-matrix unit test; here the normal
        // mover also ran this frame (air accel + gravity), so ranges only
        assert!(
            (170.0..190.0).contains(&ps.velocity.z),
            "push-off vz {}",
            ps.velocity.z
        );
        assert!(
            (-130.0..-124.0).contains(&ps.velocity.x),
            "push-off vx {}",
            ps.velocity.x
        );
        assert!(ps.since_jump_ms < 16.0, "push-off stamps the timer");
    }

    #[test]
    fn ladder_regrab_locks_for_300ms_after_a_push_off() {
        // detection is skipped while within pm_ladderJumpTime of a push-off
        // (@0x33822, pm_ladderJumpTime = 300 @0x70830)
        let w = ladder_world();
        let w = MoveWorld::bare(&w);
        let climb = PmInput {
            forward: 1.0,
            ..Default::default()
        };

        let mut locked = PlayerState::spawn(Vec3::new(33.0, 0.0, 0.2), 0.0);
        locked.since_jump_ms = 100.0;
        tick(&mut locked, &climb, &w, 10);
        assert!(
            !locked.on_ladder,
            "must not regrab inside the lock, at {}",
            locked.origin
        );

        let mut free = PlayerState::spawn(Vec3::new(33.0, 0.0, 0.2), 0.0);
        free.since_jump_ms = LADDER_REGRAB_LOCK_MS - 40.0;
        tick(&mut free, &climb, &w, 10);
        assert!(
            free.on_ladder,
            "must regrab once the lock elapses, at {}",
            free.origin
        );
    }

    #[test]
    fn airborne_ladder_glue_presses_back_into_the_wall() {
        // PM_LadderMove tail @0x33cf1: while airborne on a ladder, strip
        // velocity along the ladder normal and press into the wall at 250,
        // or 500 holding forward (rodata 0x70cd4/0x70cd8)
        let w = flat();
        let w = MoveWorld::bare(&w);
        let n = Vec3::new(-1.0, 0.0, 0.0); // wall to the east, normal west

        let mut ps = PlayerState::spawn(Vec3::new(0.0, 0.0, 40.0), 0.0);
        ps.on_ground = false;
        ladder_move(
            &mut ps,
            &PmInput::default(),
            n,
            false,
            &w,
            1.0 / 125.0,
            &mut Vec::new(),
        );
        assert!(
            (ps.velocity.x - 250.0).abs() < 1.0,
            "neutral vx {}",
            ps.velocity.x
        );

        let mut ps = PlayerState::spawn(Vec3::new(0.0, 0.0, 40.0), 0.0);
        ps.on_ground = false;
        ladder_move(
            &mut ps,
            &PmInput {
                forward: 1.0,
                ..Default::default()
            },
            n,
            false,
            &w,
            1.0 / 125.0,
            &mut Vec::new(),
        );
        assert!(
            (ps.velocity.x - 500.0).abs() < 1.0,
            "forward vx {}",
            ps.velocity.x
        );

        // tangential motion survives the glue (minus ladder friction)
        let mut ps = PlayerState::spawn(Vec3::new(0.0, 0.0, 40.0), 0.0);
        ps.on_ground = false;
        ps.velocity = Vec3::new(0.0, 100.0, 0.0);
        ladder_move(
            &mut ps,
            &PmInput::default(),
            n,
            false,
            &w,
            1.0 / 125.0,
            &mut Vec::new(),
        );
        assert!(
            (72.0..92.0).contains(&ps.velocity.y),
            "tangential vy survives, {}",
            ps.velocity.y
        );
        assert!((ps.velocity.x - 250.0).abs() < 1.0);
    }

    #[test]
    fn glue_strength_follows_the_climb_rate_sign() {
        // selector @0x33d2e reads the climb-rate slot's sign: forward *
        // 0.5 * upscale * cmdScale (@0x33a50), so W past the fz=-0.25 pitch
        // falls back to 250 and S below it strengthens to 500
        let w = flat();
        let w = MoveWorld::bare(&w);
        let n = Vec3::new(-1.0, 0.0, 0.0);
        let vx = |pitch_deg: f32, forward: f32| {
            let mut ps = PlayerState::spawn(Vec3::new(0.0, 0.0, 40.0), 0.0);
            ps.on_ground = false;
            ps.pitch = pitch_deg.to_radians();
            ladder_move(
                &mut ps,
                &PmInput {
                    forward,
                    ..Default::default()
                },
                n,
                false,
                &w,
                1.0 / 125.0,
                &mut Vec::new(),
            );
            ps.velocity.x
        };
        assert!((vx(0.0, 1.0) - 500.0).abs() < 1.0, "W level: 500");
        // -30 deg: fz = -0.5, u = -0.625 -> sign flips
        assert!((vx(-30.0, 1.0) - 250.0).abs() < 1.0, "W down: 250");
        assert!((vx(-30.0, -1.0) - 500.0).abs() < 1.0, "S down: 500");
        assert!((vx(30.0, -1.0) - 250.0).abs() < 1.0, "S up: 250");
    }

    #[test]
    fn push_off_is_pitch_invariant_on_a_vertical_wall() {
        // the composition dots the FLAT normalized forward against the ladder
        // vec (locals built @0x2ec7e-0x2ec90 with a literal-zero z lane; the
        // pitched forward feeds only the facing gate @0x2eca2-0x2ecd3), so a
        // vertical wall pushes 128 horizontally at any look pitch
        let n = Vec3::new(-1.0, 0.0, 0.0);
        let mut ps = PlayerState::spawn(Vec3::ZERO, 0.0);
        ps.pitch = 45f32.to_radians();
        assert!(push_off(
            &mut ps,
            &PmInput {
                jump: true,
                ..Default::default()
            },
            n
        ));
        assert!((ps.velocity.x + LADDER_PUSHOFF_SPEED).abs() < 0.1);
        assert!(ps.velocity.y.abs() < 0.1);
        assert!((ps.velocity.z - 187.35).abs() < 0.1);
    }

    #[test]
    fn push_off_normalizes_the_3d_reflection_before_scaling_xy() {
        // tilted normal: retail normalizes the full 3D reflected vector
        // (@0x2ed36) and only then scales x/y by 128 (@0x2ed5a)
        let n = Vec3::new(-2.0, 0.0, 1.0).normalize();
        let mut ps = PlayerState::spawn(Vec3::ZERO, 0.0);
        assert!(push_off(
            &mut ps,
            &PmInput {
                jump: true,
                ..Default::default()
            },
            n
        ));
        // yaw 0 -> f = (1,0,0); d2 = f.n = n.x; R = f - 2*d2*n
        let d2 = n.x;
        let r = Vec3::new(1.0 - 2.0 * d2 * n.x, 0.0, -2.0 * d2 * n.z);
        let expect_x = r.x / r.length() * LADDER_PUSHOFF_SPEED;
        assert!(
            (ps.velocity.x - expect_x).abs() < 0.1,
            "vx {} vs expected {expect_x}",
            ps.velocity.x
        );
        assert!(ps.velocity.y.abs() < 0.1);
    }

    #[test]
    fn ladder_friction_bleeds_speed_far_faster_than_plain_air() {
        let w = flat();
        let w = MoveWorld::bare(&w);
        let mut ps = PlayerState::spawn(Vec3::ZERO, 0.0);
        ps.velocity = Vec3::new(100.0, 0.0, 50.0);
        friction(&mut ps, true, 1.0 / 125.0);
        let ladder_speed = ps.velocity.length();
        // drop = |v| * 16 * dt ~ 14.4 of the initial 111.8
        assert!(
            (97.0..99.5).contains(&ladder_speed),
            "ladder friction should shed ~14 per frame, got {ladder_speed}"
        );

        let mut ps = PlayerState::spawn(Vec3::ZERO, 0.0);
        ps.velocity = Vec3::new(100.0, 0.0, 50.0);
        friction(&mut ps, false, 1.0 / 125.0);
        assert_eq!(
            ps.velocity.length(),
            111.8034,
            "off-ladder airborne friction must stay a no-op here"
        );
        let _ = w;
    }

    /// Stock maps carry real SURF_LADDER brushes; find one from the BSP like
    /// collision.rs does for harbor water, stand outside its thinnest axis and
    /// climb.
    #[test]
    fn climbs_a_real_map_ladder() {
        let Some(fs) = crate::testing::game_fs() else {
            return;
        };
        for map in ["maps/mp/mp_powcamp.bsp", "maps/mp/mp_harbor.bsp"] {
            let Some(data) = fs.read(map) else {
                continue;
            };
            let Ok(bsp) = crate::bsp::parse(&data) else {
                continue;
            };
            let world = CollisionWorld::build(&bsp, &[]);
            let world = MoveWorld::bare(&world);
            let axial_bounds = |b: &crate::bsp::Brush| -> ([f32; 3], [f32; 3]) {
                let sides = &bsp.brush_sides[b.first_side as usize..][..b.num_sides as usize];
                let mut lo = [0.0f32; 3];
                let mut hi = [0.0f32; 3];
                for axis in 0..3 {
                    lo[axis] = f32::from_bits(sides[axis * 2].plane_or_dist);
                    hi[axis] = f32::from_bits(sides[axis * 2 + 1].plane_or_dist);
                }
                (lo, hi)
            };
            for b in &bsp.brushes {
                if bsp.materials[b.material as usize].surface_flags & 0x8 == 0 {
                    continue;
                }
                let (lo, hi) = axial_bounds(b);
                let ex = hi[0] - lo[0];
                let ey = hi[1] - lo[1];
                if ex <= 0.0 || ey <= 0.0 || hi[2] - lo[2] < 40.0 {
                    continue;
                }
                // thinnest horizontal axis is the climb direction
                let (dir, span) = if ex < ey {
                    (Vec3::X, ex * 0.5)
                } else {
                    (Vec3::Y, ey * 0.5)
                };
                let mid_other = if ex < ey {
                    (lo[1] + hi[1]) * 0.5
                } else {
                    (lo[0] + hi[0]) * 0.5
                };
                for sign in [-1.0f32, 1.0] {
                    let yaw = if dir == Vec3::X {
                        if sign > 0.0 { 180.0f32 } else { 0.0 }
                    } else if sign > 0.0 {
                        90.0
                    } else {
                        -90.0
                    };
                    let base = Vec3::new(
                        if dir == Vec3::X {
                            (lo[0] + hi[0]) * 0.5 + sign * (span + 16.0)
                        } else {
                            mid_other
                        },
                        if dir == Vec3::X {
                            mid_other
                        } else {
                            (lo[1] + hi[1]) * 0.5 + sign * (span + 16.0)
                        },
                        0.0,
                    );
                    for dz in [24.0f32, 48.0] {
                        let start = base + Vec3::Z * (lo[2] + dz);
                        let mut ps = PlayerState::spawn(start, yaw);
                        let before = ps.origin.z;
                        tick(
                            &mut ps,
                            &PmInput {
                                forward: 1.0,
                                ..Default::default()
                            },
                            &world,
                            90,
                        );
                        if ps.on_ladder && ps.origin.z - before > 10.0 {
                            return; // found a working ladder on this map
                        }
                    }
                }
            }
            panic!("no approach to any {map} ladder grabbed and climbed");
        }
        panic!("powcamp/harbor not found under $COD_DIR");
    }

    #[test]
    fn water_jump_leaps_out_of_the_pool() {
        let w = pool_world();
        let w = MoveWorld::bare(&w);
        // far enough from the lip that the bbox clears it only after rising
        let mut ps = PlayerState::spawn(Vec3::new(172.0, 0.0, 0.2), 0.0);
        let input = PmInput {
            forward: 1.0,
            jump: true,
            ..Default::default()
        };
        pmove(&mut ps, &input, &w, 1.0 / 125.0, &[]);
        assert!(
            ps.waterjump_ms > 0.0,
            "waterjump should trigger from the shelf"
        );
        assert!(ps.velocity.z > 300.0, "boost vz {}", ps.velocity.z);
        // hands off for the flight: holding jump would bunny-hop after landing
        tick(&mut ps, &PmInput::default(), &w, 120);
        assert!(
            ps.origin.x > 255.0 && ps.origin.z < 20.0,
            "should land east of the wall, at {}",
            ps.origin
        );
        assert!(ps.on_ground && ps.waterjump_ms == 0.0, "{:?}", ps.origin);
    }

    const FULL: f32 = 127.0;

    #[test]
    fn spectator_flies_forward_at_spectator_speed() {
        let mut ps = PlayerState::spawn(Vec3::ZERO, 90.0); // yaw 90 deg faces +Y
        for _ in 0..250 {
            spectator_move(&mut ps, FULL, 0.0, 0.0, 1.0 / 125.0);
        }
        let h = ps.velocity.truncate().length();
        assert!((h - SPEED_SPECTATOR).abs() < 6.0, "speed {h}");
        assert!(ps.velocity.y > 350.0 && ps.velocity.x.abs() < 1.0);
    }

    #[test]
    fn spectator_rises_with_upmove_and_stops_on_friction() {
        let mut ps = PlayerState::spawn(Vec3::ZERO, 0.0);
        for _ in 0..63 {
            spectator_move(&mut ps, 0.0, 0.0, FULL, 1.0 / 125.0);
        }
        assert!(ps.velocity.z > 100.0, "climbing, vz {}", ps.velocity.z);
        for _ in 0..250 {
            spectator_move(&mut ps, 0.0, 0.0, 0.0, 1.0 / 125.0);
        }
        assert!(ps.velocity.length() < 1.0, "friction must stop the fly");
    }

    /// A spectator passes through solid geometry. VERIFIED live 2026-08-28 by
    /// A/B against the retail server with a retail client: retail clips
    /// through wires, decoration cars, walls and the ground; vcod blocked on
    /// all of them until this. CoD 1.1 diverges from RTCW here, which sends
    /// PM_SPECTATOR to the colliding PM_FlyMove.
    #[test]
    fn spectator_flies_through_solid_geometry() {
        let mut ps = PlayerState::spawn(Vec3::new(-200.0, 0.0, 30.0), 0.0);
        for _ in 0..250 {
            spectator_move(&mut ps, FULL, 0.0, 0.0, 1.0 / 125.0);
        }
        assert!(
            ps.origin.x > 100.0,
            "must pass through a wall spanning x 50..100, stopped at {}",
            ps.origin.x
        );
        assert!(
            ps.origin.y.abs() < 1.0,
            "and fly straight through rather than slide: y {}",
            ps.origin.y
        );
    }

    fn body(x: f32, y: f32, z: f32) -> Body {
        Body {
            entity: 7,
            origin: Vec3::new(x, y, z),
            mins: Vec3::new(-15.0, -15.0, 0.0),
            maxs: Vec3::new(15.0, 15.0, 70.0),
            contents: CONTENTS_BODY,
        }
    }

    #[test]
    fn a_run_into_a_player_stops_at_its_capsule() {
        let w = flat();
        let bodies = [body(60.0, 0.0, 0.0)];
        let mw = MoveWorld::new(&w, &bodies, 0);
        let mut ps = PlayerState::spawn(Vec3::ZERO, 0.0);
        let run = PmInput {
            forward: 1.0,
            ..Default::default()
        };
        for _ in 0..40 {
            pmove(&mut ps, &run, &mw, 0.008, &[]);
        }
        assert!(
            ps.origin.x <= 60.0 - 30.0 && ps.origin.x > 60.0 - 31.0,
            "{}",
            ps.origin.x
        );
    }

    #[test]
    fn a_run_into_a_prone_player_does_not_step_onto_it() {
        let w = flat();
        // Both resting where a floor holds a player, 0.125 up.
        let prone = Body {
            maxs: Vec3::new(15.0, 15.0, 30.0),
            ..body(60.0, 0.0, 0.125)
        };
        let bodies = [prone];
        let mw = MoveWorld::new(&w, &bodies, 0);
        let mut ps = PlayerState::spawn(Vec3::Z * 0.125, 0.0);
        let run = PmInput {
            forward: 1.0,
            ..Default::default()
        };
        for _ in 0..60 {
            pmove(&mut ps, &run, &mw, 0.008, &[]);
        }
        // A push this slow creeps inside the backoff through the step's down
        // pass, which misses the bare radius; nothing gets past that.
        assert!(
            ps.origin.z == 0.125 && (30.0..30.2).contains(&(60.0 - ps.origin.x)),
            "{:?}",
            ps.origin
        );
    }

    #[test]
    fn dead_mask_ignores_bodies() {
        let w = flat();
        let bodies = [body(20.0, 0.0, 0.0)];
        let mw = MoveWorld::new(&w, &bodies, 0);
        let mut ps = PlayerState::spawn(Vec3::ZERO, 0.0);
        ps.velocity = Vec3::new(200.0, 0.0, 0.0);
        for _ in 0..10 {
            dead_move(&mut ps, &mw, 0.05);
        }
        assert!(
            ps.origin.x > 5.0,
            "a corpse slides through players: {}",
            ps.origin.x
        );
    }

    #[test]
    fn prone_fit_ignores_bodies() {
        let w = flat();
        // yaw 0 traces backward (-x) for the prone body's length, so the
        // body has to sit behind the player to be in the trace's way at all.
        let bodies = [body(-30.0, 0.0, 0.0)];
        assert!(prone_fits(&MoveWorld::new(&w, &bodies, 0), Vec3::ZERO, 0.0));
    }

    #[test]
    fn lands_on_a_body() {
        let w = flat();
        let bodies = [body(0.0, 0.0, 0.0)];
        let mw = MoveWorld::new(&w, &bodies, 0);
        let mut ps = PlayerState::spawn(Vec3::new(0.0, 0.0, 120.0), 0.0);
        ps.on_ground = false;
        for _ in 0..60 {
            pmove(&mut ps, &PmInput::default(), &mw, 0.016, &[]);
        }
        assert!(
            ps.on_ground && ps.ground_entity == 7,
            "{:?} {}",
            ps.origin,
            ps.ground_entity
        );
        assert!((ps.origin.z - 70.0).abs() < 1.0, "{}", ps.origin.z);
    }

    #[test]
    fn lean_is_blocked_by_a_body() {
        // yaw 0's right vector is -Y, where lean_right swings the eye; a body
        // there should cut the lean short the way a wall does (INFERRED,
        // PM_UpdateLean's 0x2810011 == MASK_PLAYERSOLID, docs/research/cod11-player-clip.md 1.1).
        let w = flat();
        let bodies = [body(0.0, -35.0, 0.0)];
        let mw = MoveWorld::new(&w, &bodies, 0);
        let mut ps = PlayerState::spawn(Vec3::ZERO, 0.0);
        tick(&mut ps, &PmInput::default(), &mw, 50);
        let lean_r = PmInput {
            lean_right: true,
            ..Default::default()
        };
        tick(&mut ps, &lean_r, &mw, 125);
        assert!(ps.lean > 0.0);
        assert!(
            ps.lean < LEAN_MAX - 1.0,
            "a body should limit lean, got {}",
            ps.lean
        );
    }
}
