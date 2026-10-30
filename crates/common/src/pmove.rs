//! Contains routines ported from the Quake III Arena GPL source, Copyright (C) 1999-2005 Id Software, Inc.,
//! and the RTCW-MP GPL source, Copyright (C) 1999-2010 id Software LLC, a ZeniMax Media company. See NOTICE.
//!
//! Q3/RTCW-derived player movement on top of `movetrace::MoveWorld`. Every
//! constant's provenance: docs/research/bsp-ibsp59-format.md, "Movement
//! constants and their provenance".

use crate::collision::MASK_PLAYERSOLID;
use crate::movetrace::{MASK_DEADSOLID, MASK_SPECTATOR, MoveWorld};
use crate::net::protocol::{ENTITYNUM_NONE, ENTITYNUM_WORLD};
use crate::weapon::WeaponDef;
use crate::x87;
use glam::Vec3;

pub mod aim;
pub mod cmd;
pub mod movers;
pub mod predict;
pub mod weapon;

pub const GRAVITY: f32 = 800.0;
pub const SPEED_RUN: f32 = 190.0;
/// Wire `ps.speed` of a captured retail spectator.
pub const SPEED_SPECTATOR: f32 = 400.0;
/// Retail rodata 0x70860; a spectator's fly friction (Q3's PM_Friction with
/// pm_spectatorfriction instead of the walk constant).
pub const PM_SPECTATOR_FRICTION: f32 = 5.0;
/// `PM_CheckDuck`'s `pm_type` 4 arm (0x316f4) gives a spectator this hull,
/// rodata 0x70bd0..0x70bd8, in place of the player's.
pub const SPECTATOR_MINS: Vec3 = Vec3::new(-8.0, -8.0, -8.0);
pub const SPECTATOR_MAXS: Vec3 = Vec3::new(8.0, 8.0, 16.0);
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
/// United Offensive's `ps.sprintSpeedScale`, in place of the run scale while
/// sprinting (docs/research/coduo-movement.md, 3).
pub const SCALE_SPRINT: f32 = 1.6;
/// `pm_sprintFatigue`, `pm_fatigueRegen` and `pm_fatigueRegenDelay` (UO
/// 0x9b4e8..0x9b4f0): a full bar lasts 3 s of sprint and refills in 7 s,
/// 1 s after the last sprinting cmd (coduo-movement.md, 4).
const SPRINT_FATIGUE: f32 = 1.0 / 3.0;
const FATIGUE_REGEN: f32 = 1.0 / 7.0;
const FATIGUE_REGEN_DELAY_MS: i32 = 1000;
/// A sprint may start above this fatigue and carries on down to 0 (UO
/// 0x9b96c, 0x9b968).
const SPRINT_START_FATIGUE: f32 = 0.25;
/// Under this the body is winded (UO 0x9b900).
const WINDED_FATIGUE: f32 = 0.5;
/// `PM_CmdScale`'s factor for a held `wbuttons` 0x4, a shellshocked
/// client's (rodata 0x70894, read at 0x2e95f).
pub const SCALE_SHELLSHOCK: f32 = 0.4;
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
const MAX_CLIENTS: u32 = crate::net::protocol::MAX_CLIENTS as u32;
pub const MIN_WALK_NORMAL: f32 = 0.7;
/// `PM_SlideMove` clears the velocity past 7 planes (game.mp 0x34a1c),
/// which four bumps on two starting planes never reach; Q3 has 5.
pub const MAX_CLIP_PLANES: usize = 8;
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
/// `PM_UpdateLean`'s ramp times for a full lean, out and back (rodata
/// 0x70c60, 0x70c5c), and the lean the crouched and prone stances top out at,
/// which also scales both ramps (0x70c58, 0x70c54).
pub const LEAN_TIME_TO_MS: f32 = 350.0;
pub const LEAN_TIME_FROM_MS: f32 = 280.0;
const LEAN_SCALE_CROUCH: f32 = 0.5;
const LEAN_SCALE_PRONE: f32 = 0.25;

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
pub const PRONE_SOFT_EDGE: f32 = PRONE_YAWCAP - PRONE_SOFT_MARGIN;
const PRONE_SOFT_MARGIN: f32 = 5.0;
/// United Offensive's `bg_lmg_yawcap` default, the cap round an LMG's
/// `proneDirection` (coduo-movement.md 7). Mirrored to clients; vcod
/// keeps the default.
pub const LMG_YAWCAP: f32 = 55.0;
/// `bg_ladder_yawcap`'s default: how far a climber off the ground may look
/// away from the rungs (cod11-mantle.md, "The ladder yaw cap"). Mirrored
/// to clients; vcod keeps the default.
pub const LADDER_YAWCAP: f32 = 100.0;
pub const PRONE_SWING_DEG_PER_SEC: f32 = 55.0;
/// How far the prone view may pitch off `proneTorsoPitch`, rodata 0x70c94
/// and 0x70c98 (`PM_UpdateViewAngles`).
pub const PRONE_PITCHCAP: f32 = 45.0;
/// The rate `proneDirectionPitch` and `proneTorsoPitch` ease toward the
/// ground's pitch, rodata 0x70ca4 (`PM_UpdatePronePitch`).
pub const PRONE_PITCH_DEG_PER_SEC: f32 = 70.0;
pub const PRONE_BODY_LENGTH: f32 = 54.0;
const PRONE_BODY_HALF_BOX: f32 = 6.0;

// Water. Retail 1.1 MP has no swim and no water jump: `PmoveSingle`
// dispatches to the ladder, walk and air moves only (0x34305-0x34322), and
// the water level reaches the move through the wade scale, the friction and
// the fall damage (docs/research/cod11-mantle.md, "Water").
pub const WATER_FRICTION: f32 = 1.0;

// Ladders: retail CoD 1.1 `PM_CheckLadderMove` (game.mp.i386.so 0x336e8) and
// `PM_LadderMove` (0x33944), rodata 0x70ca8-0x70cdc
// (docs/research/cod11-mantle.md, "Ladders").
/// Forward-trace reach while walking / airborne (0x70ca8, 0x70cac).
pub const LADDER_TRACE_DIST_WALK: f32 = 8.0;
pub const LADDER_TRACE_DIST_AIR: f32 = 30.0;
/// Horizontal shrink per side of the ladder probe box (rodata 0x70cb0).
pub const LADDER_PROBE_SHRINK: f32 = 6.0;
/// The probe box's floor above the feet: the 8.0 at 0x70ca8 again
/// (0x3383d-0x3384d), with the top at the body's own, at least that.
pub const LADDER_PROBE_FLOOR: f32 = 8.0;
pub const LADDER_UPSCALE_BIAS: f32 = 0.25;
pub const LADDER_UPSCALE_GAIN: f32 = 2.5;
pub const LADDER_CLIMB_SCALE: f32 = 0.5;
pub const LADDER_STRAFE_SCALE: f32 = 0.2;
/// The inline accelerate's rate is `9 * frametime * max(wishspeed, 100)`
/// (0x70cc8, 0x70ccc), the walk's floor (cod11-mantle.md, "The walk's accel
/// floor"), not a cap on the wish.
pub const LADDER_ACCEL_FLOOR: f32 = 100.0;
pub const LADDER_ACCELERATE: f32 = 9.0;
/// The only ladder friction: on the sideways velocity, with no strafe key
/// (0x70cd0). `PM_Friction` has no ladder term.
pub const PM_LADDER_FRICTION: f32 = 16.0;
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
use crate::net::event_ids::{
    EV_FOOTSTEP_PRONE_BASE, EV_FOOTSTEP_RUN_BASE, EV_FOOTSTEP_WALK_BASE, EV_JUMP_BASE,
    EV_LANDING_BASE,
};
/// `PM_CrashLand`'s gate on the land anim, against the vertical velocity the
/// move started with (`game.mp.i386.so` rodata 0x70a08).
pub const LAND_ANIM_SPEED: f32 = -220.0;
use crate::net::event_ids::{EV_STANCE_FORCE_CROUCH, EV_STANCE_FORCE_PRONE, EV_STANCE_FORCE_STAND};
/// `EV_STEP_VIEW`: the vertical jump the step machinery added this frame,
/// which the client smooths the eye over
/// (docs/research/cod11-mantle.md, "The step event and the velocity scale").
use crate::net::event_ids::{EV_STEP_VIEW, EV_UO_FATIGUE_LAST_SOUND};
/// Step below which retail raises nothing (double @0x70f08).
const STEP_VIEW_EPS: f32 = 0.5;
/// Clamp and bias the rounded step takes before it becomes the parm
/// (@0x35727-0x35742); the parm is 8 bits on the wire.
const STEP_VIEW_MIN: i32 = -16;
const STEP_VIEW_MAX: i32 = 24;
const STEP_VIEW_BIAS: i32 = 128;
/// Post-step velocity scale `0.2 + 0.8 * (1 - |dz| / stepSize)`
/// (rodata 0x70f14/0x70f10, applied @0x35770). The 0.2 is 0x3e4ccccc, a
/// bit under `0.2f32`; 1.5b and UO reach the same bits as `1.0 - 0.8f`
/// (1.5b 0x2a4fb, 0x2a5d9).
const STEP_SCALE_BASE: f32 = f32::from_bits(0x3e4c_cccc);
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

/// The bob cycle's ticks per millisecond at full speed for each gait
/// (`PM_Footsteps` 0x322c8, rodata 0x70c28-0x70c44; 1.5 0x75a2c-0x75a48).
/// `backpedal` is `pm_flags` 0x40; only the standing gaits read it.
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

/// `PM_Footsteps`' full speed for the cmd (0x323f0-0x3268b): `ps.speed`
/// through the keys' back and strafe blend (rodata 0x70c20 0.75, 0x70c24
/// 0.5), the walk or run scale and [`stance_speed_scale`]. No weapon or wade
/// factor, unlike the wish speed, so the cycle's rate is the gait's rate times
/// how much of this speed the body has.
fn footstep_speed(ps: &PlayerState, input: &PmInput, walking: bool) -> f64 {
    let (f, r) = (
        (input.forward * 127.0).round(),
        (input.right * 127.0).round(),
    );
    let strafe = 1.0 + 0.75 * (f64::from(SCALE_STRAFE) - 1.0);
    let mut w = f64::from(ps.speed);
    if f == 0.0 {
        if r != 0.0 {
            w *= strafe;
        }
    } else if r == 0.0 {
        if f < 0.0 {
            w *= f64::from(SCALE_BACK);
        }
    } else {
        w *= (1.0 + strafe) * 0.5;
        if f < 0.0 {
            w *= (1.0 + f64::from(SCALE_BACK)) * 0.5;
        }
    }
    w *= f64::from(if walking {
        SCALE_WALK
    } else if ps.sprinting {
        // UO's divisor takes the sprint scale for the run's (0x297e4).
        SCALE_SPRINT
    } else {
        1.0
    });
    w * f64::from(stance_speed_scale(ps))
}

/// Advance the bob cycle; one step event fires per crossing of a multiple of
/// 128, which is `(old + 64) ^ (new + 64)` going negative on the byte ring.
/// Retail truncates the sum toward zero (`fistp` under control word 0xc00,
/// 0x32806-0x32819, 1.5 0x25678-0x2568a).
fn tick_bob_cycle(old: u8, msec: f32, rate: f32) -> u8 {
    ((f64::from(msec) * f64::from(rate) + f64::from(old)) as i32 & 0xff) as u8
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
    /// back toward. [`cmd::apply_view`] keeps it.
    pub prev_yaw: f32,
    /// `ps.proneDirectionPitch`: the ground's pitch along the body, eased
    /// toward it while prone. Degrees, wire convention.
    pub prone_direction_pitch: f32,
    /// `ps.proneTorsoPitch`: the ground's pitch along the view, eased the
    /// same way; the prone pitch clamp is centred on it.
    pub prone_torso_pitch: f32,
    /// `fTorsoHeight`, `fTorsoPitch` and `fWaistPitch` (`ps+0x3c4..0x3cc`):
    /// how the prone body bends over the ground, which `BG_CheckProneValid`
    /// writes off its ground samples ([`check_prone`]).
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
    ground_plane: Option<Vec3>,
    /// 1.5's and UO's `pml.almostGroundPlane` (`pml+0x34`): the last ground
    /// trace's plane, or, off the ground, a floor within a unit below. The
    /// sight flag reads it (docs/research/cod15-movement.md 5, "The sight
    /// off the ground"); 1.1 has no such field and reads `ground_plane`.
    near_ground: bool,
    /// 0 dry, 1 feet, 2 waist, 3 eyes under (RTCW waterlevel).
    pub water_level: u32,
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
    /// 1.1's prone box at the crouch's 50 (`PM_CheckDuck` 0x31c1d-0x31c9b):
    /// set by each cmd's stance pass while the eye is off the prone height
    /// and its leg at most half run (docs/research/cod11-mantle.md, "The
    /// prone box while the eye goes down").
    prone_box_crouch: bool,
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
    /// The cmd running is a spectator's: [`spectator_move`] sets it for its
    /// slide and clears it after, so the hull and the step gate below read
    /// retail's `pm_type` 4 arms.
    pub spectator: bool,
    /// Which patch's movement this player runs. The server and bots run
    /// 1.1's; the predictor takes the connection's
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
    /// breath [`fatigue_sound`] raised, 0 once rested.
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
    pub fn for_protocol(p: &crate::net::protocol::Protocol) -> Self {
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
            grenade_cancelled: false,
            last_cmd_angles: [0; 2],
            last_cmd_ads: false,
            walking: false,
            linked: false,
            mounted: None,
            seat: None,
            fall_heights: FallHeights::default(),
            bob: BobCvars::default(),
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
    /// (docs/research/cod11-gsc-object-model.md 22.3).
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
        self.view_lerp_ms.map_or(0, |ms| server_time - ms)
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
    /// speed takes [`SCALE_SHELLSHOCK`].
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

/// `pml.msec` back out of a step's frametime ([`cmd::frametime`]), which is
/// not a whole number of milliseconds times 0.001 for most lengths.
fn msec(dt: f32) -> f32 {
    (dt * 1000.0).round()
}

/// `PM_DropTimers` (0x32a44) runs from `PmoveSingle` for every `pm_type`, so
/// every arm below calls this once a move to keep a pushed player's penalty
/// ticking down even while linked, mounted or dead.
fn drop_knockback(ps: &mut PlayerState, dt: f32) {
    if ps.knockback_ms > 0.0 {
        let ms = msec(dt);
        if ms >= ps.knockback_ms {
            ps.knockback_ms = 0.0;
            ps.knockback_flags = 0;
            // The same `pm_flags &= ~0x2300` takes 1.5's penalty off.
            ps.jump_penalty = false;
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
    ps.view_yaw_shorts = None;
    ps.view_capped = [None; 2];
    ps.prone_blocked = false;
    ps.since_jump_ms += msec(dt);
    if ps.rules == MoveRules::Uo {
        uo_prologue(ps, weapons);
    }
    let input = &hold_still_sighted(ps, input);
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
    ps.takeoff = None;
    ps.breakdown_continue = false;
    let was_on_ground = ps.on_ground;
    // retail's `pml.previous_origin` and `previous_velocity`, taken at the
    // top of PmoveSingle
    ps.move_start = ps.origin;
    let start_vz = ps.velocity.z;
    // 0x34274: a mounted player's pmove updates the sight flag, the walking
    // flag and the stance to the gun's, and returns before the move/ground/
    // weapon dispatch below ever runs; the turret moves the body
    // (docs/research/cod11-turrets.md, section 5).
    if let Some(seat) = ps.seat {
        seat_move(ps, input, seat, dt, weapons, &mut events);
        return events;
    }
    if let Some(gun) = ps.mounted {
        // `PM_UpdateViewAngles` (0x340fc) runs ahead of the return, so a lean
        // carried onto the gun eases back.
        update_lean(ps, input, true, world, dt);
        ps.on_ground = false;
        ps.ground_normal = Vec3::Z;
        ps.ground_surface_flags = 0;
        ads_flag(ps, input, weapon_def, world, &mut events);
        ps.walking = walking_flag(ps, input);
        update_sprint_flag(ps, input, weapon_def);
        ps.stance = gun;
        ps.ducked = gun == Stance::Crouch;
        if ps.rules.v15() {
            // 1.5's mounted arm runs the whole `PM_CheckDuck` (0x332c4): the
            // eye's target steps to the gun's stance and the eye eases
            // there, so `eFlags` reads the gun's stance from the mount on
            // (docs/research/cod11-turrets.md 14.8).
            step_view_target(ps);
            view_height_adjust_v15(ps, (dt * 1000.0).round() as i32);
        }
        ps.on_ladder = false;
        drop_knockback(ps, dt);
        update_fatigue(ps, input, dt, &mut events);
        return events;
    }
    if ps.linked {
        linked_move(ps, input, world, dt, weapons, &mut events);
        return events;
    }
    // `PM_UpdateViewAngles` runs ahead of the stance (`PmoveSingle` 0x340fc),
    // so the prone clamps read last frame's stance and pitches, and the
    // ladder cap last frame's ladder flag and ground.
    cap_ladder_yaw(ps);
    update_prone_view(ps, input, world, dt);
    update_lean(ps, input, true, world, dt);
    update_stance(ps, input, world, dt, &mut events);
    set_water_level(ps, world);
    ground_trace(ps, world, MASK_PLAYERSOLID, input);
    // A takeoff the first ground trace raised is run down by this move's
    // `PM_DropTimers` (1.1 0x342ce, then 0x342f9).
    if let Some(t) = &mut ps.takeoff {
        t.before_timers = true;
    }
    // Retail updates the ADS flag once per `pm_type` arm, after the ground
    // trace and before the arm's move (`PM_UpdateAimDownSightFlag`, combat
    // doc 1.13), so it reads the ground state the move starts from.
    ads_flag(ps, input, weapon_def, world, &mut events);
    // `PM_UpdatePlayerWalkingFlag` follows it in the same arm (0x342d8), so
    // the walk reads the ADS flag this frame just set.
    ps.walking = walking_flag(ps, input);
    // UO's sprint flag next (0x2dfe3), so this cmd's speed reads it.
    update_sprint_flag(ps, input, weapon_def);
    // Then `PM_UpdatePronePitch` (0x342dd), off this frame's ground plane.
    update_prone_pitch(ps, world, dt, &mut events);
    // retail checks ladders right after the first ground trace; the check
    // reads `pm_time` before `PM_DropTimers` (0x342f4, 0x342f9)
    let ladder = check_ladder_move(ps, input, world);
    drop_knockback(ps, dt);
    update_fatigue(ps, input, dt, &mut events);
    if let Some(normal) = ladder {
        ladder_move(ps, input, normal, world, dt, &mut events);
    } else {
        // `PM_WalkMove` opens with `PM_CheckJump` (0x2f261); a jump runs the
        // air mover instead and stamps `jumpTime` after it (0x2f279).
        let mut cmd = *input;
        if ps.rules.v15() && ps.on_ground {
            jump_penalty_landing(ps);
        }
        let jumped = ps.on_ground && check_jump(ps, &mut cmd, None, &mut events);
        if ps.on_ground {
            friction(ps, world, dt);
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
    ground_trace(ps, world, MASK_PLAYERSOLID, input);
    // `PM_CrashLand` runs inside the ground trace (0x30721), so it reads the
    // water level the frame began with and the footsteps read its damping.
    if !was_on_ground && ps.on_ground {
        let damage = crash_land(ps, start_vz, false, &mut events);
        ps.land_anim = match ps.rules {
            MoveRules::V1_1 => start_vz < LAND_ANIM_SPEED,
            // 1.5 plays the land anim from the damage branch only (0x2144f).
            MoveRules::V1_5 | MoveRules::Uo => damage > 0,
        };
    }
    set_water_level(ps, world);

    // PM_Footsteps @0x322c8 runs once per move, after the final ground trace:
    // after `PM_Weapon` on 1.1 (0x34331, 0x34336), before it on 1.5
    // (0x28889, 0x2888e) and UO (0x2e04e, 0x2e053).
    let dt_ms = (dt * 1000.0).round() as i32;
    if ps.rules == MoveRules::V1_1 {
        weapon::pm_weapon(ps, input, weapons, dt_ms, &mut events);
        footsteps(ps, input, world, dt, &mut events);
    } else {
        footsteps(ps, input, world, dt, &mut events);
        weapon::pm_weapon(ps, input, weapons, dt_ms, &mut events);
    }
    // Retail's `pm->oldcmd`: what the next step subtracts this one from.
    ps.last_cmd_angles = input.angles;
    ps.last_cmd_ads = input.ads;
    clamp_velocity_to_move(ps, world, dt);
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
    cap_ladder_yaw(ps);
    update_prone_view(ps, input, world, dt);
    update_lean(ps, input, true, world, dt);
    // The arm's first store: `groundEntityNum` 1023, `pml.walking` and
    // `pml.groundPlane` 0.
    ps.on_ground = false;
    ps.ground_plane = None;
    ps.ground_normal = Vec3::Z;
    ps.ground_surface_flags = 0;
    ads_flag(
        ps,
        input,
        weapons.get(ps.weapon as usize).and_then(Option::as_ref),
        world,
        events,
    );
    ps.walking = walking_flag(ps, input);
    update_sprint_flag(
        ps,
        input,
        weapons.get(ps.weapon as usize).and_then(Option::as_ref),
    );
    update_stance(ps, input, world, dt, events);
    drop_knockback(ps, dt);
    update_fatigue(ps, input, dt, events);
    weapon::pm_weapon(ps, input, weapons, (dt * 1000.0).round() as i32, events);
    ps.last_cmd_angles = input.angles;
    ps.last_cmd_ads = input.ads;
}

/// UO's arm for a vehicle seat, `PmoveSingle`'s `eFlags & 0x106000` branch
/// (game 0x2de0a; coduo-vehicles.md 3): nothing moves, the vehicle carries
/// the body. The sight is the button alone, and only a four-wheel's seat 3
/// runs `PM_Weapon`; every other seat steps the sight's fraction.
fn seat_move(
    ps: &mut PlayerState,
    input: &PmInput,
    seat: VehicleSeat,
    dt: f32,
    weapons: &[Option<WeaponDef>],
    events: &mut Vec<PmEvent>,
) {
    ps.on_ladder = false;
    ps.on_ground = false;
    ps.ground_plane = None;
    ps.ground_normal = Vec3::Z;
    ps.ground_surface_flags = 0;
    // `PM_UpdateAimDownSightFlag`'s first clause (0x32c0b).
    ps.ads_active = input.ads;
    ps.walking = walking_flag(ps, input);
    let def = weapons.get(ps.weapon as usize).and_then(Option::as_ref);
    update_sprint_flag(ps, input, def);
    update_fatigue(ps, input, dt, events);
    let dt_ms = (dt * 1000.0).round() as i32;
    if seat.own_weapon() {
        weapon::pm_weapon(ps, input, weapons, dt_ms, events);
    } else {
        weapon::seat_ads_lerp(ps, seat, def, dt_ms);
    }
    ps.last_cmd_angles = input.angles;
    ps.last_cmd_ads = input.ads;
}

/// United Offensive's `PmoveSingle` ahead of everything else (0x2d530):
/// the held weapon looked up and a sprint on the previous cmd cutting a
/// deploy short (docs/research/coduo-movement.md, 1).
fn uo_prologue(ps: &mut PlayerState, weapons: &[Option<WeaponDef>]) {
    ps.lmg = weapons
        .get(ps.weapon as usize)
        .and_then(Option::as_ref)
        .is_some_and(WeaponDef::is_lmg);
    if ps.sprinting {
        weapon::interrupt_with_sprint(ps);
    }
}

/// `PmoveSingle`'s early zeroing of the cmd's forward and right moves under
/// the sight flag: on a prone body (1.1 0x33f48 tests `pm_flags` 0x21, 1.5
/// 0x281ae the effective stance) and, on UO, on a deployed bipod
/// (coduo-movement.md, 1 and 13).
fn hold_still_sighted(ps: &PlayerState, input: &PmInput) -> PmInput {
    let prone = match ps.rules {
        MoveRules::V1_1 => ps.stance == Stance::Prone,
        MoveRules::V1_5 | MoveRules::Uo => ps.effective_stance() == Stance::Prone,
    };
    let mut input = *input;
    if ps.ads_active && (prone || ps.deployed()) {
        input.forward = 0.0;
        input.right = 0.0;
    }
    input
}

/// The sight flag: [`weapon::update_ads_flag`], or under UO's rules
/// `PM_UpdateAimDownSightFlag` (0x32c0b) with its LMG deploy. An LMG sights
/// only by deploying: on a mount surface 15 ahead (the `hmgstanding` and
/// `hmgcrouching` brushes, contents 0x404000), which sets the stance the
/// surface names, or else by going prone where a body fits; one refused
/// tells the client to stand up again. The server's mount hint
/// ([`PlayerState::mount_hint`]) deploys where the probe misses. A deploy
/// holds while the button is down (docs/research/coduo-movement.md, 7, 11).
fn ads_flag(
    ps: &mut PlayerState,
    input: &PmInput,
    def: Option<&WeaponDef>,
    world: &MoveWorld,
    events: &mut Vec<PmEvent>,
) {
    let lmg = def.is_some_and(WeaponDef::is_lmg);
    if ps.rules != MoveRules::Uo || !lmg {
        weapon::update_ads_flag(ps, input, def);
        return;
    }
    let keep = input.ads
        && def.is_some_and(|d| d.aim_down_sight)
        && !matches!(
            ps.weaponstate,
            weapon::WEAPON_RAISING
                | weapon::WEAPON_DROPPING
                | weapon::WEAPON_MELEE_WINDUP
                | weapon::WEAPON_MELEE_RELAX
        )
        && (ps.weaponstate != weapon::WEAPON_BREAKDOWN || input.weapon == ps.weapon)
        && (ps.sight_ground() || ps.linked);
    if !keep {
        ps.ads_active = false;
        return;
    }
    if ps.ads_active {
        return;
    }
    let view_yaw = ps.yaw.to_degrees();
    let (sin, cos) = ps.yaw.sin_cos();
    // 15 along `AngleVectors` of (0, yaw, 0) (0x32e3c), then 1 up.
    let fwd = yaw_forward(ps, world.world.arith());
    let probe = (ps.origin.as_dvec3() + fwd.as_dvec3() * 15.0).as_vec3() + Vec3::Z;
    let contents = world.point_contents(probe) & CONTENTS_HMG;
    let mount = if contents == 0 {
        ps.mount_hint
    } else if contents & CONTENTS_HMG_CROUCH != 0 {
        Some(Stance::Crouch)
    } else {
        Some(Stance::Stand)
    };
    if let Some(stance) = mount {
        ps.prone_direction = view_yaw;
        ps.ads_active = true;
        ps.stance = stance;
        ps.ducked = stance == Stance::Crouch;
        return;
    }
    if ps.stance != Stance::Prone {
        // The deploy's `BG_CheckProneValid` also wants the gun's 38 units
        // clear ahead, a +/-6 box 8 up (`BG_CheckProneValid` 0x2162c, its 16th argument).
        let gun = |o: Vec3| {
            let from = o + Vec3::Z * 8.0;
            world
                .box_trace(
                    from,
                    from + Vec3::new(cos, sin, 0.0) * 38.0,
                    Vec3::splat(-6.0),
                    Vec3::splat(6.0),
                    MASK_DEADSOLID | 0x20,
                )
                .fraction
                >= 1.0
        };
        let fit = (ps.on_ground && gun(ps.origin))
            .then(|| {
                check_prone(
                    world,
                    ProneQuery {
                        origin: ps.origin,
                        yaw_deg: view_yaw,
                        entry: true,
                        ground: true,
                        length: PRONE_CHECK_LENGTH,
                        rules: ps.rules,
                    },
                )
            })
            .flatten();
        let Some(body) = fit else {
            ps.prone_blocked = true;
            if !input.stance_held {
                push_event(
                    events,
                    if ps.ducked {
                        EV_STANCE_FORCE_CROUCH
                    } else {
                        EV_STANCE_FORCE_STAND
                    },
                );
            }
            return;
        };
        ps.prone_body = body;
        ps.prone_direction = view_yaw;
        ps.stance = Stance::Prone;
    }
    ps.ads_active = true;
}

/// UO's mount surfaces: `textures/common/hmgstanding` (0x4000) and
/// `hmgcrouching` (0x400000), the mask the deploy probes with.
pub const CONTENTS_HMG_CROUCH: u32 = 0x40_0000;
pub const CONTENTS_HMG: u32 = 0x4000 | CONTENTS_HMG_CROUCH;

/// UO's `PM_UpdatePlayerSprintingFlag` (0x2c74a), per cmd: the button held
/// with a move key, standing, off the sight, the ladder and any gun, with
/// fatigue left. Starting needs ground and more than a quarter of the bar;
/// a sprint already running keeps going in the air and down to empty.
/// A grenade in the hand must not be firing or cooking (coduo-movement.md,
/// 2).
fn update_sprint_flag(ps: &mut PlayerState, input: &PmInput, def: Option<&WeaponDef>) {
    let was = ps.sprinting;
    ps.sprinting = false;
    if ps.rules != MoveRules::Uo || !input.sprint {
        return;
    }
    let grenade_busy = def.is_some_and(|d| {
        d.weapon_type == "grenade"
            && (input.attack || ps.weaponstate == weapon::WEAPON_FIRING || weapon::cooking(ps, d))
    });
    ps.sprinting = ps.fatigue > 0.0
        && (was || ps.fatigue > SPRINT_START_FATIGUE)
        && ps.stance != Stance::Prone
        && !ps.ducked
        && !ps.ads_active
        && (was || ps.on_ground)
        && (input.forward != 0.0 || input.right != 0.0)
        && ps.mounted.is_none()
        && ps.weaponstate != weapon::WEAPON_BREAKDOWN
        && !grenade_busy
        && !ps.on_ladder;
}

/// UO's `PM_UpdateFatigue` (0x2aa22): a sprinting cmd stamps
/// `lastSprintTime` and drains the bar, winding the body and raising
/// `EV_FATIGUE_LAST_SOUND` on every cmd under half; one that neither
/// sprints nor holds the button refills it from a second after the last
/// sprint, and a full bar clears the wind. Either way a cmd that does not
/// sprint goes on to [`fatigue_sound`], unless it is still inside the
/// refill's delay without the button. The products stay on the x87 stack
/// until the store.
fn update_fatigue(ps: &mut PlayerState, input: &PmInput, dt: f32, events: &mut Vec<PmEvent>) {
    if ps.rules != MoveRules::Uo {
        return;
    }
    let step = |rate: f32| 0.001f32 as f64 * f64::from(msec(dt)) * f64::from(rate);
    if ps.sprinting {
        ps.last_sprint_time = input.server_time;
        ps.fatigue = (f64::from(ps.fatigue) - step(SPRINT_FATIGUE)) as f32;
        if ps.fatigue < WINDED_FATIGUE {
            push_event(events, EV_UO_FATIGUE_LAST_SOUND);
            ps.winded = true;
        }
        if ps.fatigue < 0.0 {
            ps.fatigue = 0.0;
        }
    } else if input.sprint {
        fatigue_sound(ps, input.server_time, events);
    } else if ps.last_sprint_time.wrapping_add(FATIGUE_REGEN_DELAY_MS) <= input.server_time {
        ps.fatigue = (f64::from(ps.fatigue) + step(FATIGUE_REGEN)) as f32;
        if ps.fatigue >= 1.0 {
            ps.fatigue = 1.0;
            ps.winded = false;
        }
        fatigue_sound(ps, input.server_time, events);
    }
}

/// UO's `PM_PlayFatigueSound` (0x2a968): a winded body breathes
/// (`EV_FATIGUE_LAST_SOUND` again) once `iFatigueSoundTime` is more than
/// 1700 ms old, stamping it; a rested one clears a stamp that old.
fn fatigue_sound(ps: &mut PlayerState, server_time: i32, events: &mut Vec<PmEvent>) {
    let stale = ps.fatigue_sound_time + FATIGUE_SOUND_MS < server_time;
    if !ps.winded {
        if ps.fatigue_sound_time > 0 && stale {
            ps.fatigue_sound_time = 0;
        }
    } else if stale {
        push_event(events, EV_UO_FATIGUE_LAST_SOUND);
        ps.fatigue_sound_time = server_time;
    }
}

/// `PM_PlayFatigueSound`'s 0x6a4.
const FATIGUE_SOUND_MS: i32 = 1700;

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
/// itself, over the frame time, as the velocity. Both sides stay on the x87
/// stack: a fall from rest moves exactly half its end velocity, and which
/// way that tie goes rides the last bits of the origin (cod11-mantle.md,
/// "The tail of the default arm").
fn clamp_velocity_to_move(ps: &mut PlayerState, world: &MoveWorld, dt: f32) {
    if world.world.arith() == crate::patch::Arith::Msvc {
        // Every Windows cgame (cgame_mp_x86.dll 1.1 0x3000c90d, 1.5
        // 0x3000d6c4, UO 0x3000e64f) keeps x, the rate and the squared
        // speed on its 53-bit stack and stores y and z; UO sums the speed
        // from x, 1.1 and 1.5 from z (cod15-movement.md 8).
        let ft = f64::from(dt);
        let gap = |a: f32, b: f32| f64::from(a) - f64::from(b);
        let dx = gap(ps.origin.x, ps.move_start.x);
        let dy = f64::from(gap(ps.origin.y, ps.move_start.y) as f32);
        let dz = f64::from(gap(ps.origin.z, ps.move_start.z) as f32);
        let ratio = ((dz * dz + dy * dy) + dx * dx) / (ft * ft);
        let v = ps.velocity.as_dvec3();
        let speed2 = if ps.rules == MoveRules::Uo {
            (v.x * v.x + v.y * v.y) + v.z * v.z
        } else {
            (v.z * v.z + v.y * v.y) + v.x * v.x
        };
        if speed2 * 0.25 > ratio {
            let inv = 1.0 / ft;
            ps.velocity = Vec3::new((inv * dx) as f32, (inv * dy) as f32, (inv * dz) as f32);
        }
        return;
    }
    if ps.rules.v15() {
        // 1.5b's tail (0x288ad-0x289e3) stores the displacement, the rate
        // and the squared speed as floats; only the sums, the divide and
        // `1 / frametime` stay on the stack (cod15-movement.md 7).
        let moved = (ps.origin - ps.move_start).as_dvec3();
        let ft = f64::from(dt);
        let ratio =
            (((moved.x * moved.x + moved.y * moved.y) + moved.z * moved.z) / (ft * ft)) as f32;
        let v = ps.velocity.as_dvec3();
        let speed2 = ((v.x * v.x + v.y * v.y) + v.z * v.z) as f32;
        if f64::from(speed2) * 0.25 > f64::from(ratio) {
            let inv = 1.0 / ft;
            ps.velocity = (moved * inv).as_vec3();
        }
        return;
    }
    let moved = ps.origin.as_dvec3() - ps.move_start.as_dvec3();
    let ft = f64::from(dt);
    let ratio = ((moved.x * moved.x + moved.y * moved.y) + moved.z * moved.z) / (ft * ft);
    let v = ps.velocity.as_dvec3();
    if ((v.y * v.y + v.x * v.x) + v.z * v.z) * 0.25 > ratio {
        // x off the stack, y and z off their stored floats.
        let inv = 1.0 / ft;
        ps.velocity = Vec3::new(
            (moved.x * inv) as f32,
            (inv * f64::from(moved.y as f32)) as f32,
            (inv * f64::from(moved.z as f32)) as f32,
        );
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

/// A dead player's frame: no input, no stance or weapon step, a lean that
/// only eases back (`PM_UpdateViewAngles`' dead arm, 0x32deb), and the
/// eye easing to `VIEW_DEAD` at the retail capture's rate
/// (`DEAD_VIEW_LERP_SPEED`). `PmoveSingle` zeroes a dead cmd's moves
/// (0x3416a) and runs the default arm, whose ladder check drops a corpse off
/// the ground before every move, so a corpse falls on every frame and the
/// closing ground trace lands it again (docs/research/cod11-player-clip.md
/// 8.11). The landing runs `PM_CrashLand`, which takes no damage at
/// `pm_type > 5`; its events are the return.
pub fn dead_move(ps: &mut PlayerState, world: &MoveWorld, dt: f32) -> Vec<PmEvent> {
    let dt = dt.min(MAX_FRAME_MS / 1000.0);
    let idle = PmInput::default();
    let mut events = Vec::new();
    ps.jumped = false;
    ps.move_start = ps.origin;
    let start_vz = ps.velocity.z;
    // `PM_ClearAimDownSightFlag` (`game.mp.i386.so` 0x3abd4), which
    // `PmoveSingle` calls in the dead arm. The fraction is left where the
    // death froze it: the weapon step that would ramp it down does not run
    // for a dead player (combat doc, 1.12 and 1.13).
    ps.ads_active = false;
    update_lean(ps, &idle, false, world, dt);
    ground_trace(ps, world, MASK_DEADSOLID, &idle);
    dead_friction(ps);
    // `PM_CheckLadderMove` past its `pm_time` return: a dead player loses the
    // ladder flag, the ground entity, `pml.groundPlane` and `pml.walking`
    // (0x33782-0x337a5). A 1.5 or UO corpse killed inside a jump penalty
    // still drops.
    if !ladder_check_skipped(ps) {
        ps.on_ladder = false;
        ps.on_ground = false;
        ps.ground_plane = None;
        ps.ground_normal = Vec3::Z;
        ps.ground_surface_flags = 0;
    }
    drop_knockback(ps, dt);
    // No events and no post-step velocity scale for a corpse: retail's step
    // block sits behind `ps->pm_type > 5` (@0x35660).
    ps.walking = false;
    let grounded = ps.on_ground;
    if grounded {
        friction(ps, world, dt);
        walk_move(ps, &idle, None, world, dt, MASK_DEADSOLID, None);
    } else {
        air_move(ps, &idle, world, dt, MASK_DEADSOLID, None);
    }
    // The closing ground trace (0x34327) calls `PM_CrashLand` when it finds
    // ground under a `groundEntityNum` of `ENTITYNUM_NONE`.
    ground_trace(ps, world, MASK_DEADSOLID, &idle);
    if !grounded && ps.on_ground {
        crash_land(ps, start_vz, true, &mut events);
    }
    dead_eye(ps, dt);
    // `pm_type` 6 takes the default arm, tail and snap included.
    clamp_velocity_to_move(ps, world, dt);
    snap_velocity(ps);
    events
}

/// `PM_CheckDuck`'s dead arm (0x316f4, `pm_type > 5`): a target no stance
/// has drops any leg and moves at a flat rate (0x30a84).
fn dead_eye(ps: &mut PlayerState, dt: f32) {
    ps.view_lerp_ms = None;
    let step = DEAD_VIEW_LERP_SPEED * dt;
    let gap = VIEW_DEAD - ps.view_height_cur;
    ps.view_height_cur += gap.clamp(-step, step);
}

/// A linked corpse, `pm_type` 7: `PmoveSingle`'s table sends it to the arm
/// a linked live player takes (1.1 0x34220, 1.5 0x286f1, UO 0x2de0a), not
/// to the dead move. No ground trace and no move: `groundEntityNum` 1023,
/// the velocity left as it was and not snapped. What runs is the dead
/// lean, `PM_CheckDuck`'s dead eye and the timers; the weapon drop is
/// `cmd::dead_step`'s (cod11-player-clip.md 1.1).
pub fn dead_linked_move(ps: &mut PlayerState, world: &MoveWorld, dt: f32) {
    let dt = dt.min(MAX_FRAME_MS / 1000.0);
    ps.jumped = false;
    ps.move_start = ps.origin;
    ps.ads_active = false;
    update_lean(ps, &PmInput::default(), false, world, dt);
    ps.on_ground = false;
    ps.ground_plane = None;
    ps.ground_normal = Vec3::Z;
    ps.ground_surface_flags = 0;
    ps.walking = false;
    dead_eye(ps, dt);
    drop_knockback(ps, dt);
}

/// What a dead player on the ground loses every pmove frame on top of the
/// friction, `.rodata 0x7090c`.
const DEAD_SPEED_DROP: f32 = 20.0;

/// `PM_DeadMove` (0x2f700), which `PmoveSingle` calls for `pm_type` 6 only
/// (0x342e9): on walkable ground the speed, z included, drops by 20 every
/// frame and the velocity stops once that reaches 0. The drop is per frame,
/// not scaled by its length, so a corpse's slide depends on its client's cmd
/// rate (docs/research/cod11-combat.md 5.7).
fn dead_friction(ps: &mut PlayerState) {
    if !ps.on_ground {
        return;
    }
    let speed = ps.velocity.length();
    let left = speed - DEAD_SPEED_DROP;
    ps.velocity = if left <= 0.0 {
        Vec3::ZERO
    } else {
        ps.velocity / speed * left
    };
}

/// Retail footstep cadence (`PM_Footsteps` 0x322c8, 1.5 0x24ae8). The bob
/// cycle ticks by `msec * rate * speed / full speed`
/// ([`footstep_speed`]); a step event fires on each 128-tick crossing while
/// move keys are held and the gait is audible. The ladder branch runs ahead
/// of the speed gates (0x323a2 precedes the fcom at 0x3249f). The walk is
/// `pm_flags` 0x80, the ADS walk, and under 1.5 a lean too (0x24bf7); UO's
/// ground cadence leaves the lean out (`game.mp.uo` 0x297e4).
fn footsteps(
    ps: &mut PlayerState,
    input: &PmInput,
    world: &MoveWorld,
    dt: f32,
    events: &mut Vec<PmEvent>,
) {
    let msec = msec(dt);
    let v15 = ps.rules.v15();
    let walking = ps.walking || input.walk_slow || v15 && ps.lean != 0.0;
    let stance = ps.effective_stance();
    let speed = (ps.velocity.x * ps.velocity.x + ps.velocity.y * ps.velocity.y).sqrt();
    ps.footsteps_xyspeed = speed;
    if !ps.on_ground {
        // quiet for 299 ms after a jump (0x323b9; 1.5 `pm_ladderJumpTime`
        // 300 at 0x75630)
        if ps.on_ladder && ps.since_jump_ms > LADDER_STEP_QUIET_MS {
            let (k, scale) = if walking {
                (LADDER_STEP_K_WALK, SCALE_WALK)
            } else {
                (LADDER_STEP_K_RUN, 1.0)
            };
            let rate = ps.velocity.z / (scale * LADDER_STEP_DIVISOR) * k;
            let old = ps.bob_cycle;
            ps.bob_cycle = tick_bob_cycle(old, msec, rate);
            ladder_step_event(ps, world, old, ps.bob_cycle, events);
        }
        // 1.5 hands an airborne body whose eye target is not its flags'
        // stance to the ground code (0x24e51), ladder or not.
        if !v15 || stance == ps.stance {
            return;
        }
    }
    if speed < 10.0 {
        // a stop puts the phase at zero; a creep keeps it (0x324af-0x324ba)
        if speed < 1.0 {
            ps.bob_cycle = 0;
        }
        return;
    }
    // UO's ground cadence walks on the ADS walk alone: a lean ticks at the
    // run's rate there (`game.mp.uo` 0x297e4), though its ladder cadence and
    // `PM_FootstepEvent` (0x2937c) still walk on one (coduo-movement.md 16).
    let gait = if ps.rules == MoveRules::Uo {
        ps.walking || input.walk_slow
    } else {
        walking
    };
    let full = footstep_speed(ps, input, gait);
    let rate =
        (f64::from(speed) / full * f64::from(step_rate(stance, gait, ps.backwards_run))) as f32;
    let old = ps.bob_cycle;
    ps.bob_cycle = tick_bob_cycle(old, msec, rate);
    if input.forward != 0.0 || input.right != 0.0 {
        // Audible only upright without the ADS walk; a lean does not quiet
        // it (`PM_ShouldMakeFootsteps` 0x3221c, 1.5 0x24a5a).
        let enable = stance == Stance::Stand && !(ps.walking || input.walk_slow);
        ground_step_event(ps, old, ps.bob_cycle, walking, enable, events);
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
    // UO's sprint step is its own group, the jump's ids (0x35b9b: prone,
    // then the sprint flag, then the walk).
    let base = match ps.stance {
        Stance::Prone => prone_base,
        _ if ps.sprinting => EV_JUMP_BASE,
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
fn crash_land(ps: &mut PlayerState, start_vz: f32, dead: bool, events: &mut Vec<PmEvent>) -> i32 {
    if ps.water_level >= 3 {
        return 0;
    }
    // Retail solves the impact speed and squares it back over 2g; this is the
    // same height in closed form, and no landing at all when it has no root.
    let den = start_vz * start_vz + 2.0 * ps.gravity * (ps.move_start.z - ps.origin.z);
    if den < 0.0 {
        return 0;
    }
    let height = den / (2.0 * ps.gravity);
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
        return 0;
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
    damage
}

/// The ground surface flag that takes no fall damage (0x2feaa).
const SURF_NODAMAGE: u32 = 0x1;
/// `EV_LANDING_PAIN_*`, one per surface material up to 138; the server's
/// `ClientEvents` turns each into fall damage.
pub use crate::net::event_ids::EV_LANDING_PAIN_BASE;
pub const EV_LANDING_PAIN_LAST: i32 =
    EV_LANDING_PAIN_BASE + crate::net::event_ids::SURFACE_GROUP_SIZE - 1;

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

/// A CoD 1.1 spectator, `PM_FlyMove` (0x2ee6c): friction always, then the
/// fly accelerate, then `PM_StepSlideMove` without gravity in the
/// spectator's own hull under [`MASK_SPECTATOR`] (docs/protocol-1.1.md,
/// "Spectator"). The axes are the usercmd's, -127..127, and `wbuttons` its
/// button byte, whose lean bits fly a spectator down and up. With no world
/// it integrates straight off the velocity: a host with no map loaded.
pub fn spectator_move(
    ps: &mut PlayerState,
    world: Option<&MoveWorld>,
    forward: f32,
    right: f32,
    up: f32,
    wbuttons: u8,
    dt: f32,
) -> Vec<PmEvent> {
    let dt = dt.min(MAX_FRAME_MS / 1000.0);
    ps.on_ground = false;
    let arith = world.map_or(crate::patch::Arith::Kept, |w| w.world.arith());
    if ps.rules.v15() {
        spectator_friction_stored(ps, dt);
        fly_accelerate_stored(ps, forward, right, up, wbuttons, dt, arith);
        return spectator_slide(ps, world, dt);
    }
    // `PM_Friction` (0x2e460): a spectator is never walking, so only its
    // `pm_type` 4 arm (0x2e56d) drops speed, by the speed itself times 5
    // (0x70874), with no stop-speed floor.
    let speed = x87::dot(ps.velocity, ps.velocity).sqrt() as f32;
    if speed < 1.0 {
        ps.velocity.x = 0.0;
        ps.velocity.y = 0.0;
    } else {
        let speed = f64::from(speed);
        let drop = speed * f64::from(PM_SPECTATOR_FRICTION) * f64::from(dt);
        let scale = (speed - drop).max(0.0) / speed;
        ps.velocity = (ps.velocity.as_dvec3() * scale).as_vec3();
    }
    fly_accelerate(ps, forward, right, up, wbuttons, dt, arith);
    spectator_slide(ps, world, dt)
}

/// The fly move's `PM_StepSlideMove` in the spectator's hull, gravity off.
fn spectator_slide(ps: &mut PlayerState, world: Option<&MoveWorld>, dt: f32) -> Vec<PmEvent> {
    let mut events = Vec::new();
    match world {
        Some(world) => {
            ps.spectator = true;
            step_slide_move(ps, world, dt, false, MASK_SPECTATOR, Some(&mut events));
            ps.spectator = false;
        }
        None => ps.origin += ps.velocity * dt,
    }
    events
}

/// `PM_FlyMove`'s wish and accelerate (0x2ee8b..0x2f026). The wish runs
/// along the full view, pitch included (`PmoveSingle`'s `AngleVectors` of
/// `ps.viewangles` at 0x34120), plus `upmove` straight up, scaled by
/// `PM_CmdScale` (0x2e5bc), so a partial key flies slower. Lean right then
/// takes 512 off the wish's z and lean left adds 256, outside the scale
/// (0x2ef19..0x2ef4a). No wish leaves the velocity alone, signed zeros
/// included. The accelerate is 8 (0x708e4) times a wish speed of at least
/// 100 (0x708e8).
fn fly_accelerate(
    ps: &mut PlayerState,
    f: f32,
    r: f32,
    u: f32,
    wbuttons: u8,
    dt: f32,
    arith: crate::patch::Arith,
) {
    let max = f.abs().max(r.abs()).max(u.abs());
    let lean_down = if wbuttons & crate::net::msg::WBUTTON_LEAN_RIGHT != 0 {
        512.0
    } else {
        0.0
    };
    let lean_up = if wbuttons & crate::net::msg::WBUTTON_LEAN_LEFT != 0 {
        256.0
    } else {
        0.0
    };
    if max == 0.0 && lean_down == 0.0 && lean_up == 0.0 {
        return;
    }
    let mut wishvel = Vec3::ZERO;
    if max != 0.0 {
        let (f, r, u) = (f64::from(f), f64::from(r), f64::from(u));
        let total = (f * f + r * r + u * u).sqrt();
        let scale = (f64::from(ps.speed) * f64::from(max) / (127.0 * total)) as f32;
        let scale = f64::from(scale);
        let (fwd, right) = view_vectors(ps, arith);
        let (fwd, right) = (fwd.as_dvec3(), right.as_dvec3());
        wishvel = (fwd * scale * f + right * scale * r).as_vec3();
        wishvel.z = (f64::from(wishvel.z) + scale * u) as f32;
    }
    // Both lean terms ride one x87 chain and round once, at the store.
    wishvel.z = ((f64::from(wishvel.z) - lean_down) + lean_up) as f32;
    let wishdir = x87::normalize(wishvel);
    let wishspeed = x87::dot(wishvel, wishvel).sqrt() as f32;
    let add = f64::from(wishspeed) - x87::dot(ps.velocity, wishdir);
    if add <= 0.0 {
        return;
    }
    let accel = (8.0 * f64::from(wishspeed.max(100.0)) * f64::from(dt)).min(add);
    ps.velocity = (ps.velocity.as_dvec3() + wishdir.as_dvec3() * accel).as_vec3();
}

/// 1.5b's and UO's `PM_Friction` for a spectator (1.5 0x1eab3, UO
/// 0x234ef): the speed sums `(vz^2 + vy^2) + vx^2` and is stored, under one
/// unit x, y and z stop, and otherwise the `pm_type` 4 term `speed *
/// pm_spectatorfriction (5) * frametime` joins the stored drop of
/// [`friction_stored`].
fn spectator_friction_stored(ps: &mut PlayerState, dt: f32) {
    let v = ps.velocity.as_dvec3();
    let speed = ((v.z * v.z + v.y * v.y) + v.x * v.x).sqrt() as f32;
    if speed < 1.0 {
        ps.velocity = Vec3::ZERO;
        return;
    }
    friction_stored(ps, speed, dt, true);
}

/// 1.5b's and UO's `PM_FlyMove` past the friction (1.5 0x1fcf9, UO
/// 0x24838): `PM_CmdScale` (UO 0x23905) stored, the wish
/// `right * scale * pml.right + forward * scale * pml.forward` plus `up *
/// scale` on z, each component stored; the lean's 512 down and 256 up only
/// while `ps.speed` is not 0 (UO 0x2492e, `ps+0x48`); then the stored
/// `VectorNormalize` and `PM_Accelerate` (UO 0x2379e) at
/// `pm_flyaccelerate` 8. The run scale `PM_CmdScale` multiplies in is 1 on
/// a spectator.
fn fly_accelerate_stored(
    ps: &mut PlayerState,
    f: f32,
    r: f32,
    u: f32,
    wbuttons: u8,
    dt: f32,
    arith: crate::patch::Arith,
) {
    let max = f.abs().max(r.abs()).max(u.abs());
    let mut wishvel = Vec3::ZERO;
    if max != 0.0 && ps.speed != 0.0 {
        let (fd, rd, ud) = (f64::from(f), f64::from(r), f64::from(u));
        let total = (ud * ud + fd * fd + rd * rd).sqrt();
        let scale = f64::from(((f64::from(max) * f64::from(ps.speed)) / (127.0 * total)) as f32);
        let (fwd, right) = view_vectors(ps, arith);
        let (fwd, right) = (fwd.as_dvec3(), right.as_dvec3());
        wishvel = (right * (rd * scale) + fwd * (fd * scale)).as_vec3();
        wishvel.z = (f64::from(wishvel.z) + ud * scale) as f32;
    }
    if ps.speed != 0.0 {
        let down = if wbuttons & crate::net::msg::WBUTTON_LEAN_RIGHT != 0 {
            512.0
        } else {
            0.0
        };
        let up = if wbuttons & crate::net::msg::WBUTTON_LEAN_LEFT != 0 {
            256.0
        } else {
            0.0
        };
        wishvel.z = ((f64::from(wishvel.z) - down) + up) as f32;
    }
    let wishspeed = f64::from(x87::dot(wishvel, wishvel) as f32).sqrt() as f32;
    let wishdir = x87::normalize_stored(wishvel);
    accelerate_stored(ps, wishdir, wishspeed, 8.0, dt);
}

/// `PM_CheckDuck`'s stance arms (0x31840-0x31b80): a prone press the body
/// does not fit in is refused, and a rise without headroom stays down. Each
/// refusal tells the client which stance to fall back to through
/// `EV_STANCE_FORCE_*`, unless the cmd's stance came from a held key
/// (docs/research/cod11-mantle.md, "Prone Blocked").
fn update_stance(
    ps: &mut PlayerState,
    input: &PmInput,
    world: &MoveWorld,
    dt: f32,
    events: &mut Vec<PmEvent>,
) {
    let before = ps.stance;
    let (mut crouch, mut prone) = (input.crouch, input.prone);
    // UO's deployed bipod takes no stance key at all (0x27e16).
    let keys = !ps.deployed();
    // 0x318a0: a climber's stance keys are taken off the cmd, announced
    // whether held or not.
    if keys && ps.on_ladder && (crouch || prone) {
        (crouch, prone) = (false, false);
        push_event(events, EV_STANCE_FORCE_STAND);
    }
    let force = |events: &mut Vec<PmEvent>, event: i32| {
        if !input.stance_held {
            push_event(events, event);
        }
    };
    let v15 = ps.rules.v15();
    let (origin, mins) = (ps.origin, ps.mins());
    // 1.5's stance traces leave bodies out (0x23a18 and on).
    let mask = if v15 {
        MASK_PLAYERSOLID & !crate::movetrace::CONTENTS_BODY
    } else {
        MASK_PLAYERSOLID
    };
    let fits = |height: f32| {
        let maxs = Vec3::new(HALF_WIDTH, HALF_WIDTH, height);
        !world.box_trace(origin, origin, mins, maxs, mask).startsolid
    };
    if !keys {
        // The deploy holds the stance it set.
    } else if prone {
        if v15 && before == Stance::Prone {
            // A held prone key keeps 0x2 off (0x2390c).
            ps.ducked = false;
        } else if v15 && !ps.on_ground {
            // 1.5 drops an airborne press without a word (0x237d6).
        } else if before != Stance::Prone {
            let q = ProneQuery {
                origin: ps.origin,
                yaw_deg: ps.yaw.to_degrees(),
                entry: true,
                ground: ps.on_ground,
                length: PRONE_CHECK_LENGTH,
                rules: ps.rules,
            };
            match check_prone(world, q) {
                Some(body) => {
                    ps.prone_body = body;
                    ps.stance = Stance::Prone;
                    if v15 {
                        // 1.5 clears 0x2 on the way down (0x2390c).
                        ps.ducked = false;
                    }
                    if v15 {
                        enter_prone(ps, input, world);
                    }
                }
                // 0x3196b: every cmd that keeps asking is refused again.
                None => {
                    ps.prone_blocked = true;
                    ps.prone_dive = false;
                    let event = if ps.ducked {
                        EV_STANCE_FORCE_CROUCH
                    } else {
                        EV_STANCE_FORCE_STAND
                    };
                    force(events, event);
                }
            }
        }
    } else {
        // The dive flag lives as long as the prone key is held (0x319a6,
        // 0x31a13).
        ps.prone_dive = false;
        match (before, crouch) {
            (Stance::Prone, true) => {
                if fits(HEIGHT_CROUCH) {
                    ps.stance = Stance::Crouch;
                } else {
                    force(events, EV_STANCE_FORCE_PRONE);
                }
            }
            (_, true) => ps.stance = Stance::Crouch,
            // A prone player who lets go stands if it can, else crouches.
            (Stance::Prone, false) => {
                if fits(HEIGHT_STAND) {
                    ps.stance = Stance::Stand;
                } else if fits(HEIGHT_CROUCH) {
                    ps.stance = Stance::Crouch;
                } else {
                    force(events, EV_STANCE_FORCE_PRONE);
                }
            }
            (Stance::Crouch, false) => {
                if fits(HEIGHT_STAND) {
                    ps.stance = Stance::Stand;
                } else {
                    force(events, EV_STANCE_FORCE_CROUCH);
                }
            }
            (Stance::Stand, false) => {}
        }
    }
    if ps.stance != before {
        match ps.stance {
            Stance::Crouch => ps.ducked = true,
            Stance::Stand => ps.ducked = false,
            Stance::Prone => {}
        }
    }
    let msec = (dt * 1000.0).round() as i32;
    if v15 {
        step_view_target(ps);
        view_height_adjust_v15(ps, msec);
        return;
    }
    // 0x31c1d-0x31c9b: the prone box stays the crouch's until the eye is
    // down or past half its leg, `(cmd.serverTime - viewHeightLerpTime) /
    // duration` read before this cmd's adjust (0x308cc, 0 with no leg).
    ps.prone_box_crouch = ps.stance == Stance::Prone && ps.view_height_cur != VIEW_PRONE && {
        let frac = ps.view_lerp_ms.map_or(0.0, |ms| {
            (f64::from(ms + msec) / f64::from(view_lerp_duration(ps))).clamp(0.0, 1.0)
        });
        frac <= 0.5
    };
    // 1.1 lies down in the box just sized (0x31ca1).
    if ps.stance == Stance::Prone && before != Stance::Prone {
        enter_prone(ps, input, world);
    }
    // The bbox snaps; the eye eases.
    view_height_adjust(ps, msec);
}

/// 1.5's `PM_CheckDuck` tail (0x23e3c..0x24049): while no leg of the eye
/// runs, the target moves one stance toward the flags' stance. Reaching
/// prone starts the jump penalty's timer, so a body that just lay down
/// jumps low once it stands (docs/research/cod15-movement.md, "Stance").
fn step_view_target(ps: &mut PlayerState) {
    if ps.view_lerp_ms.is_some() {
        return;
    }
    let t = ps.view_target;
    ps.view_target = if ps.stance == Stance::Prone {
        if t == VIEW_STAND {
            VIEW_CROUCH
        } else if t != VIEW_PRONE {
            ps.jump_penalty = true;
            ps.knockback_ms = JUMP_PENALTY_MS;
            VIEW_PRONE
        } else {
            t
        }
    } else if t == VIEW_PRONE || ps.ducked {
        VIEW_CROUCH
    } else {
        VIEW_STAND
    };
}

/// 1.5's `PM_ViewHeightAdjust` (0x226a6) for a live player: the running leg
/// advances, and a new one starts toward the target only once none runs.
/// The target never moves mid-leg, so 1.1's turn back never happens.
fn view_height_adjust_v15(ps: &mut PlayerState, msec: i32) {
    if let Some(ms) = ps.view_lerp_ms.as_mut() {
        *ms += msec;
        let pct = (*ms * 100 / view_lerp_duration(ps)).clamp(0, 100);
        if pct == 100 {
            ps.view_height_cur = ps.view_lerp_target;
            ps.view_lerp_ms = None;
        } else {
            ps.view_height_cur = view_curve_height(view_curve(ps), pct);
        }
        return;
    }
    if ps.view_target == ps.view_height_cur {
        return;
    }
    ps.view_lerp_ms = Some(0);
    ps.view_lerp_down = ps.view_target < ps.view_height_cur;
    ps.view_lerp_target = ps.view_target;
}

fn push_event(events: &mut Vec<PmEvent>, event: i32) {
    events.push(PmEvent { event, parm: 0 });
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
    // 1.5's `PM_GetViewHeightLerpTime` (0x223a6) takes 200 ms down to the
    // crouch and has no dive.
    if ps.rules.v15() && ps.view_lerp_target == VIEW_CROUCH && ps.view_lerp_down {
        return VIEW_LERP_STAND_MS;
    }
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
    if ps.rules.v15() {
        return ps.effective_stance();
    }
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

/// [`stance_speed_scale`] as 1.5b's walk scale reads it: a leg's blend
/// left on the x87 stack (0x1f2b5-0x1f36d), the lerp fraction a float.
fn stance_speed_scale_kept(ps: &PlayerState) -> f64 {
    let blend = |f: f32, to: f32, from: f32| {
        let f = f64::from(f);
        f64::from(to) * f + (1.0 - f) * f64::from(from)
    };
    let f = view_lerp_frac(ps, VIEW_CROUCH, VIEW_PRONE);
    if f != 0.0 {
        return blend(f, SCALE_PRONE, SCALE_CROUCH);
    }
    let f = view_lerp_frac(ps, VIEW_PRONE, VIEW_CROUCH);
    if f != 0.0 {
        return blend(f, SCALE_CROUCH, SCALE_PRONE);
    }
    f64::from(move_stance(ps).speed_scale())
}

/// `fTorsoHeight`, `fTorsoPitch`, `fWaistPitch`: the chest's height off the
/// origin and the pitches of the torso and the legs along the ground, in
/// degrees, wire convention.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ProneBody {
    pub torso_height: f32,
    pub torso_pitch: f32,
    pub waist_pitch: f32,
}

/// What [`check_prone`] is asked: `entry` is `PM_CheckDuck`'s first-frame
/// call, which also needs room for the prone box where the player stands;
/// `ground` asks for the ground samples, which every caller passes as
/// "on the ground" but `PM_VerifyPronePosition`, which always does.
/// `length` is 1.5's body length argument, the sweep's reach plus the half
/// box: 60 everywhere but the prone view's swing (1.1's fixed sweep of
/// [`PRONE_BODY_LENGTH`] is the same 60).
#[derive(Clone, Copy)]
pub struct ProneQuery {
    pub origin: Vec3,
    pub yaw_deg: f32,
    pub entry: bool,
    pub ground: bool,
    pub length: f32,
    pub rules: MoveRules,
}

/// [`ProneQuery::length`] for every caller but the 1.5 swing.
pub const PRONE_CHECK_LENGTH: f32 = PRONE_BODY_LENGTH + PRONE_BODY_HALF_BOX;

/// `BG_CheckProneValid` (`game.mp.i386.so` 0x2d428, 1.5 0x1cf58;
/// docs/research/cod11-mantle.md, "The ground samples", and
/// cod15-movement.md 4): whether a body may lie at `origin` facing
/// `yaw_deg`, and how it bends over the ground there. `None` is a refusal.
/// Without `ground` a fit writes a flat body: 1.1 stops after the sweep, 1.5
/// runs the ground samples anyway and falls back to the flat body when one
/// misses. 1.5 also refuses a sample landing on a plane steeper than
/// [`MIN_WALK_NORMAL`], ground or not, and retries a blocked leg sweep 18
/// higher.
pub fn check_prone(world: &MoveWorld, q: ProneQuery) -> Option<ProneBody> {
    const MASK: u32 = MASK_DEADSOLID;
    let v15 = q.rules.v15();
    let o = q.origin;
    let hw = HALF_WIDTH;
    if q.entry {
        let t = world.box_trace(
            o,
            o + Vec3::Z * 10.0,
            Vec3::new(-hw, -hw, 0.0),
            Vec3::new(hw, hw, HEIGHT_PRONE),
            MASK,
        );
        if t.allsolid {
            return None;
        }
    }
    let half = Vec3::splat(PRONE_BODY_HALF_BOX);
    let trace = |a: Vec3, b: Vec3| world.box_trace(a, b, -half, half, MASK);
    // `AngleVectors` of (0, yaw - 180, 0) (0x2d5e0, 1.5 0x1d1d4, UO
    // 0x21955; the Windows cgames' own, cod11-movers.md 22).
    let back = Vec3::new(0.0, q.yaw_deg - 180.0, 0.0);
    let fwd = x87::angle_vectors_in(world.world.arith(), back)[0];
    let h = HEIGHT_PRONE - PRONE_BODY_HALF_BOX;
    let sweep = q.length - PRONE_BODY_HALF_BOX;
    // The sweep behind the facing: clear, or clear far enough (`reach`).
    let start = o + Vec3::Z * h;
    let mut end = start + fwd * sweep;
    let mut t = trace(start, end);
    let mut reach = None;
    if t.fraction < 1.0 {
        if !q.ground {
            return None;
        }
        let mut got = t.fraction * sweep + PRONE_BODY_HALF_BOX;
        if hw + 2.0 > got {
            return None;
        }
        let limit = h * 0.7 + 24.0;
        if got < limit {
            // Again 22 units higher: over a kerb or a sill.
            end.z += 22.0;
            let len = (end - start).length();
            t = trace(start, end);
            if t.fraction < 1.0 {
                got = t.fraction * len + PRONE_BODY_HALF_BOX;
                if got < limit {
                    return None;
                }
                reach = Some(got);
            }
        } else {
            reach = Some(got);
        }
    }
    let mut feet = t.endpos;
    if !q.ground && !v15 {
        return Some(ProneBody::default());
    }
    // A sample that misses refuses on the ground and lays the body flat off
    // it (1.5 0x2e655); a steep one refuses outright (0x1d842, 0x1dd86,
    // 0x1def4).
    let missed = || (!q.ground).then(ProneBody::default);
    let steep = |t: &crate::collision::Trace| v15 && t.normal.z < MIN_WALK_NORMAL;
    // The hips, 24 behind the origin, dropped onto the ground.
    let drop = h + hw * 2.5 - PRONE_BODY_HALF_BOX;
    let hips_from = o + fwd * 24.0 + Vec3::Z * h;
    let t = trace(hips_from, hips_from - Vec3::Z * drop);
    if t.fraction == 1.0 {
        return missed();
    }
    if steep(&t) {
        return None;
    }
    let hips = t.endpos;
    let hips_drop = drop * t.fraction + PRONE_BODY_HALF_BOX;
    if let Some(got) = reach {
        // A short body bends its legs up the obstacle; the sweep is again
        // from the hips, toward a point between the full length and that.
        if hips_drop * -0.75 > got - hips_drop {
            return missed();
        }
        let v = ((feet - hips) + fwd * PRONE_BODY_HALF_BOX + Vec3::Z * PRONE_BODY_HALF_BOX)
            .normalize_or_zero();
        let reach_to = hips_from + v * (sweep - 24.0);
        let far = o + fwd * sweep;
        let to = Vec3::new(
            0.5 * (far.x + reach_to.x),
            0.5 * (far.y + reach_to.y),
            reach_to.z,
        );
        let mut t = trace(hips_from, to);
        if t.fraction < 1.0 && v15 {
            // 1.5 tries once more from the hit, 18 up (rodata 0x74fb0).
            let up = Vec3::Z * 18.0;
            t = trace(t.endpos + up, to + up);
        }
        if t.fraction < 1.0 {
            return missed();
        }
        feet = t.endpos;
    }
    // The feet and the chest dropped onto the ground.
    let t = trace(feet, feet - Vec3::Z * ((feet.z - hips.z) * 2.0 + hw));
    if t.fraction == 1.0 {
        return missed();
    }
    if steep(&t) {
        return None;
    }
    let feet = t.endpos;
    let t = trace(o + Vec3::Z * h, Vec3::new(o.x, o.y, o.z - hw * 1.5));
    if t.fraction == 1.0 {
        return missed();
    }
    if steep(&t) {
        return None;
    }
    let chest = t.endpos;
    let bend = angle_subtract(vectopitch(feet - hips), vectopitch(hips - chest));
    // Legs bent up past the hips: 1.1 and 1.5 allow 50 degrees (rodata 0x701a8,
    // 1.5 0x74fb8), UO 45 (0x9a9e8; coduo-movement.md 17).
    let bend_up = if q.rules == MoveRules::Uo {
        -45.0
    } else {
        -50.0
    };
    if !(bend_up..=70.0).contains(&bend) {
        return missed();
    }
    // Nothing between the three samples, 5 units up.
    let up = Vec3::Z * 5.0;
    let line = |a: Vec3, b: Vec3| world.box_trace(a + up, b + up, Vec3::ZERO, Vec3::ZERO, MASK);
    if line(chest, hips).fraction < 1.0 || line(hips, feet).fraction < 1.0 {
        return missed();
    }
    // One x87 expression, stored once (1.1d 0x2e14c, 1.5b 0x1e589, UO
    // 0x22ed5): the difference is never rounded to a float on its own.
    let torso_height = (f64::from(chest.z) - f64::from(o.z)) - f64::from(PRONE_BODY_HALF_BOX);
    Some(ProneBody {
        torso_height: torso_height as f32,
        torso_pitch: angle_normalize180(vectopitch(chest - hips)),
        waist_pitch: angle_normalize180(vectopitch(hips - feet)),
    })
}

/// Whether a body may lie down at `origin` facing `yaw_deg`, the sweep
/// behind the facing alone: [`check_prone`] off the ground.
pub fn prone_fits(world: &MoveWorld, origin: Vec3, yaw_deg: f32) -> bool {
    check_prone(
        world,
        ProneQuery {
            origin,
            yaw_deg,
            entry: false,
            ground: false,
            length: PRONE_CHECK_LENGTH,
            rules: MoveRules::V1_1,
        },
    )
    .is_some()
}

/// `vectopitch` (0x3dd7c): wire convention (positive down), in 0..360.
fn vectopitch(v: Vec3) -> f32 {
    if v.x == 0.0 && v.y == 0.0 {
        return if v.z > 0.0 { 270.0 } else { 90.0 };
    }
    let len = (v.x * v.x + v.y * v.y).sqrt();
    let p = (-f64::from(v.z).atan2(f64::from(len)) * 180.0 / std::f64::consts::PI) as f32;
    if p < 0.0 { p + 360.0 } else { p }
}

/// `AngleSubtract`: `a - b` folded into -180..180.
fn angle_subtract(a: f32, b: f32) -> f32 {
    let mut d = a - b;
    while d > 180.0 {
        d -= 360.0;
    }
    while d < -180.0 {
        d += 360.0;
    }
    d
}

/// `AngleNormalize180` (0x3eb70): through a truncated 16-bit angle first.
pub fn angle_normalize180(deg: f32) -> f32 {
    let short = ((deg * (65536.0 / 360.0)) as i32) & 0xffff;
    let a = short as f32 * (360.0 / 65536.0);
    if a > 180.0 { a - 360.0 } else { a }
}

/// [`angle_normalize180`] on the x87's unrounded product: the float
/// argument times rodata's 182.04445 truncated to a short, as 1.1d's
/// `AngleNormalize180` (0x3eb70) and `AngleDelta` (0x3ec9c, on `a - b`) do.
fn short_normalize180(deg: f64) -> f32 {
    let short = ((deg * f64::from(cmd::ANGLE2SHORT)) as i32) & 0xffff;
    let a = short as f32 * (360.0 / 65536.0);
    if a > 180.0 { a - 360.0 } else { a }
}

/// `AngleNormalize360` (1.1 0x3eb30, 1.5 0x36a3c, UO 0x3dc8c) on the x87's
/// unrounded product: 0..360 on the shorts grid.
fn short_normalize360(deg: f64) -> f32 {
    let short = ((deg * f64::from(cmd::ANGLE2SHORT)) as i32) & 0xffff;
    short as f32 * (360.0 / 65536.0)
}

/// `AngleNormalize180Accurate` (1.5 0x36b75): whole turns added while the
/// angle is at or under -180 and taken away while it is over 180, each sum
/// stored, so an angle already in range comes back as it went in.
fn normalize180_accurate(mut deg: f32) -> f32 {
    while deg <= -180.0 {
        deg += 360.0;
    }
    while deg > 180.0 {
        deg -= 360.0;
    }
    deg
}

/// A view angle in degrees off the sim's radians. Retail holds degrees,
/// and every view the prone, ladder and entry code reads is on the shorts
/// grid (`SHORT2ANGLE` of the cmd, or a 1.1 cap's
/// `AngleNormalize180/360`), which the radians miss by up to 0.005 of a
/// short near 360 degrees: one under the grid truncates a whole short away
/// in `AngleDelta`. An angle further off the grid is kept.
pub(crate) fn view_deg(rad: f32) -> f32 {
    let deg = rad.to_degrees();
    let shorts = f64::from(deg) * (65536.0 / 360.0);
    let grid = shorts.round();
    if (shorts - grid).abs() < 0.05 {
        (grid * (360.0 / 65536.0)) as f32
    } else {
        deg
    }
}

/// Degrees to the sim's radians as `AngleVectors` takes them, by the double
/// pi / 180 rounded to a float (game.mp 0x3b234), as [`cmd::apply_view`]
/// does.
fn view_rad(deg: f32) -> f32 {
    (f64::from(deg) * (std::f64::consts::PI / 180.0)) as f32
}

/// The frame a player goes prone, the tail of `PM_CheckDuck`'s prone arm
/// (0x31ca9-0x31f50): a press on a player moving forward or back throws it
/// into the air, the body takes the view's yaw, and both prone pitches start
/// from the ground under it (docs/research/cod11-mantle.md, "Prone").
fn enter_prone(ps: &mut PlayerState, input: &PmInput, world: &MoveWorld) {
    // Retail tests the ADS flag here too, but a move clears it first, so a
    // forward or back cmd always dives. 1.5 has no dive.
    if input.forward != 0.0 && ps.rules == MoveRules::V1_1 {
        ps.prone_dive = true;
        if ps.on_ground {
            let height = if ps.ducked {
                DIVE_HEIGHT_LOW
            } else {
                DIVE_HEIGHT_STAND
            };
            ps.velocity.z = (2.0 * height * ps.gravity).sqrt();
            ps.on_ground = false;
            ps.ground_plane = None;
        }
        ps.aim_spread_scale = 255.0;
    }
    if ps.rules == MoveRules::V1_1 {
        // 0x31d35-0x31df1: the box goes 10 up and back down, and the body
        // takes where it lands.
        let (mins, maxs) = (ps.mins(), ps.maxs());
        let up = world.box_trace(
            ps.origin,
            ps.origin + Vec3::Z * 10.0,
            mins,
            maxs,
            MASK_PLAYERSOLID,
        );
        ps.origin = world
            .box_trace(up.endpos, ps.origin, mins, maxs, MASK_PLAYERSOLID)
            .endpos;
    }
    // The body takes the view's yaw as it is (1.1 0x31dfc; 1.5 in 0x232c4).
    ps.prone_direction = view_deg(ps.yaw);
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
    let view_pitch = view_deg(-ps.pitch);
    let d = angle_delta(ps.prone_direction_pitch, view_pitch, ps.rules);
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
pub fn pitch_for_yaw_on_normal(yaw_deg: f32, normal: Vec3) -> f32 {
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

/// The ladder arm of `PM_UpdateViewAngles` (1.1 0x32e7d-0x32f75, 1.5
/// 0x26270-0x263ca, UO 0x2b535-0x2b6a5): on last frame's ladder flag and
/// off the ground, a view more than [`LADDER_YAWCAP`] off the rungs'
/// facing, `AngleDelta(vectoyaw(vLadderVec) + 180, yaw)`, is pushed back
/// onto the cap through `delta_angles` and placed at `facing -/+ cap`,
/// through `AngleNormalize360` on 1.1 and `AngleNormalize360Accurate` on
/// 1.5 and UO (cod11-mantle.md, "The ladder yaw cap").
fn cap_ladder_yaw(ps: &mut PlayerState) {
    if !ps.on_ladder || ps.on_ground {
        return;
    }
    let d = angle_delta(
        ladder_facing(ps.ladder_normal, ps.rules),
        view_deg(ps.yaw),
        ps.rules,
    );
    if d.abs() <= LADDER_YAWCAP {
        return;
    }
    let excess = d - LADDER_YAWCAP.copysign(d);
    // 1.1 keeps the facing on the x87 stack; 1.5 and UO stored it.
    let facing = if ps.rules.v15() {
        f64::from(ladder_facing(ps.ladder_normal, ps.rules))
    } else {
        vectoyaw(ps.ladder_normal) + 180.0
    };
    let at = (facing - f64::from(LADDER_YAWCAP.copysign(excess))) as f32;
    let yaw = if ps.rules.v15() {
        normalize360_accurate(at)
    } else {
        short_normalize360(f64::from(at))
    };
    ps.view_yaw_correction = excess;
    ps.view_capped[1] = Some(yaw);
    ps.yaw = view_rad(yaw);
}

/// The prone half of `PM_UpdateViewAngles` (0x32d7c): the body swinging to
/// follow the view, the view capped to the yaw cone around the body and
/// pitched no further than the cap off `proneTorsoPitch`. Retail enforces
/// both caps by pushing `delta_angles`, so the corrections are reported here
/// and the caller applies them to whatever owns the view
/// (docs/research/cod11-mantle.md, "Prone").
fn update_prone_view(ps: &mut PlayerState, input: &PmInput, world: &MoveWorld, dt: f32) {
    let prone = ps.stance == Stance::Prone;
    if !prone && !ps.deployed() {
        return;
    }
    if ps.rules.v15() {
        // UO caps any LMG at `bg_lmg_yawcap`, swings only an undeployed
        // body and pulls the view to none holding an LMG (0x2b15c).
        let cap = if ps.lmg && ps.rules == MoveRules::Uo {
            LMG_YAWCAP
        } else {
            PRONE_YAWCAP
        };
        let lmg = ps.lmg && ps.rules == MoveRules::Uo;
        update_prone_view_v15(
            ps,
            input,
            world,
            dt,
            cap,
            prone && !ps.deployed(),
            prone && !lmg,
        );
        cap_prone_pitch(ps);
        return;
    }
    let view = view_deg(ps.yaw);
    // `AngleNormalize180(AngleDelta(..))` (0x3306d, 0x3307b): folded through the
    // shorts twice, so the cap's push below truncates what retail's does.
    let delta = angle_normalize180(angle_delta(ps.prone_direction, view, ps.rules));
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
        let q = ProneQuery {
            origin: ps.origin,
            yaw_deg: candidate,
            entry: false,
            ground: ps.on_ground,
            length: PRONE_CHECK_LENGTH,
            rules: ps.rules,
        };
        if let Some(body) = check_prone(world, q) {
            ps.prone_direction = candidate;
            ps.prone_body = body;
        } else if delta.abs() > PRONE_YAWCAP + 0.1 {
            // 0x3319d-0x331c4: a refused swing announces itself only once
            // the view is past the cap (rodata 0x70c90).
            ps.prone_blocked = true;
        }
    }
    // The cap measures the excess before the swing and places the view
    // after it (0x331c8-0x33235).
    // The view lands on the cap through `AngleNormalize360` (0x33266): on
    // the shorts grid, in 0..360.
    if delta.abs() > PRONE_YAWCAP {
        ps.view_yaw_correction = delta - PRONE_YAWCAP.copysign(delta);
        let yaw = short_normalize360(f64::from(ps.prone_direction - PRONE_YAWCAP.copysign(delta)));
        ps.view_capped[1] = Some(yaw);
        ps.yaw = view_rad(yaw);
    }
    cap_prone_pitch(ps);
}

/// The prone pitch cap (1.1 0x33295.., 1.5 0x26ba0..): the view pitch no
/// further than [`PRONE_PITCHCAP`] off `proneTorsoPitch`, measured through
/// `AngleDelta` on both. 1.1 places the view through the short-truncating
/// `AngleNormalize180` (0x33346), 1.5 and UO through
/// `AngleNormalize180Accurate` (cod11-mantle.md, "AngleDelta").
fn cap_prone_pitch(ps: &mut PlayerState) {
    let view_pitch = view_deg(-ps.pitch);
    let d = angle_delta(ps.prone_torso_pitch, view_pitch, ps.rules);
    if d.abs() > PRONE_PITCHCAP {
        ps.view_pitch_correction = d - PRONE_PITCHCAP.copysign(d);
        let at = ps.prone_torso_pitch - PRONE_PITCHCAP.copysign(d);
        let pitch = if ps.rules.v15() {
            normalize180_accurate(at)
        } else {
            short_normalize180(f64::from(at))
        };
        ps.view_capped[0] = Some(pitch);
        ps.pitch = -view_rad(pitch);
    }
}

/// The prone arm of `SetClientViewAngle` (1.1 0x41e30, 1.5 0x3bf08, UO
/// 0x44301): a prone player not on a turret or in a seat has the angles
/// he is given held 45 degrees either side of the body's yaw and from 45
/// down to 15 up off `proneTorsoPitch`, each measured through
/// `AngleDelta` and placed through the short-truncating
/// `AngleNormalize360` / `AngleNormalize180` on all three. Wire degrees,
/// pitch positive down (cod11-mantle.md, "AngleDelta").
pub fn prone_view_angle(ps: &PlayerState, angles: [f32; 3]) -> [f32; 3] {
    ProneCone::of(ps).map_or(angles, |cone| cone.hold(angles))
}

/// What [`prone_view_angle`] reads off a playerstate: the body's yaw and
/// torso pitch of a prone player not on a turret or in a seat. The server
/// mirrors it to the script host, whose `setPlayerAngles` writes the held
/// angles to the entity's `angles` (1.1 0x4209f).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ProneCone {
    pub direction: f32,
    pub torso_pitch: f32,
    pub rules: MoveRules,
}

impl ProneCone {
    pub fn of(ps: &PlayerState) -> Option<Self> {
        (ps.stance == Stance::Prone && ps.mounted.is_none() && ps.seat.is_none()).then_some(Self {
            direction: ps.prone_direction,
            torso_pitch: ps.prone_torso_pitch,
            rules: ps.rules,
        })
    }

    /// `angles` held inside the cone, wire degrees.
    pub fn hold(self, mut angles: [f32; 3]) -> [f32; 3] {
        let d = angle_normalize180(angle_delta(self.direction, angles[1], self.rules));
        if d > 45.0 {
            angles[1] = short_normalize360(f64::from(self.direction - 45.0));
        } else if d < -45.0 {
            angles[1] = short_normalize360(f64::from(self.direction + 45.0));
        }
        let d = angle_normalize180(angle_delta(self.torso_pitch, angles[0], self.rules));
        if d > 45.0 {
            angles[0] = short_normalize180(f64::from(self.torso_pitch - 45.0));
        } else if d < -15.0 {
            angles[0] = short_normalize180(f64::from(self.torso_pitch + 15.0));
        }
        angles
    }
}

/// `AngleNormalize360Accurate` (1.5 0x36ae0): whole turns added or taken
/// away until the angle is in 0..360.
pub(crate) fn normalize360_accurate(mut deg: f32) -> f32 {
    while deg < 0.0 {
        deg += 360.0;
    }
    while deg >= 360.0 {
        deg -= 360.0;
    }
    deg
}

/// One `delta_angles` push: `(int)(deg * 182.04445)`, truncated (1.5
/// 0x26368 and three more, control word 0xc00).
pub fn angle_shorts(deg: f32) -> i32 {
    (f64::from(deg) * f64::from(cmd::ANGLE2SHORT)) as i32 & 0xffff
}

/// 1.5's prone yaw (`PM_UpdateViewAngles` 0x26118, cod15-movement.md 4).
/// The body swings toward the view as in 1.1, but a swing that does not fit
/// walks back toward the old heading a degree at a time until one does, and
/// each try is the fit at a direction and length between the body's and the
/// view's (`swing_fits`, 0x25ff5) plus a 45-unit fit of both the view and
/// the new heading. Then a body still off the view must fit a 45-unit body
/// along its own heading; while it does not, the view turns a degree at a
/// time toward the body. The cap is measured after that and places the view
/// off the new heading. Any refusal sets 0x8000, and a refused turn of at
/// most a degree eases the view 98% of the way back to where it was.
fn update_prone_view_v15(
    ps: &mut PlayerState,
    input: &PmInput,
    world: &MoveWorld,
    dt: f32,
    cap: f32,
    swing: bool,
    pull: bool,
) {
    // `AngleDelta` folds through the shorts (`AngleNormalize180` 0x36a8f),
    // the float difference times the float 182.04445 left unrounded on the
    // x87 stack before the truncation (`AngleNormalize360` 0x36a4e-0x36a6b).
    let delta = |a: f32, b: f32| {
        let short = (f64::from(a - b) * f64::from(65536.0f32 / 360.0)) as i32 & 0xffff;
        let d = short as f32 * (360.0 / 65536.0);
        if d > 180.0 { d - 360.0 } else { d }
    };
    let origin = ps.origin;
    let query = |yaw_deg: f32, length: f32, ground: bool| ProneQuery {
        origin,
        yaw_deg,
        entry: false,
        ground,
        length,
        rules: MoveRules::V1_5,
    };
    let ground = ps.on_ground;
    let fits45 = |yaw: f32| check_prone(world, query(yaw, 45.0, ground)).is_some();
    // 0x25ff5: off the view by `d`, the body is tried a third of the way
    // from the view per 80 degrees, and shorter, down to 45 at 240.
    let swing_fits = |body: &mut ProneBody, yaw: f32, view: f32| {
        let d = delta(yaw, view);
        let f = d.abs() / 240.0;
        let q = query(
            normalize360_accurate(yaw - (1.0 - f) * d),
            60.0 * (1.0 - f) + 45.0 * f,
            ground,
        );
        let fit = check_prone(world, q);
        if let Some(b) = fit {
            *body = b;
        }
        fit.is_some()
    };
    let old_view = ps.prev_yaw.to_degrees();
    // The view starts on the shorts grid (`SHORT2ANGLE` of the cmd plus
    // `delta_angles`); taking it back off the radians would miss a fold by
    // an ulp.
    let new_view = {
        let step = 360.0 / 65536.0;
        f32::from((ps.yaw.to_degrees() / step).round() as i32 as i16) * step
    };
    let mut view = new_view;
    let mut shorts = 0i32;
    let mut pushed = false;
    let mut blocked = false;
    let mut body = ps.prone_body;

    let mut d = delta(ps.prone_direction, view);
    let moving = input.forward != 0.0 || input.right != 0.0;
    'swing: {
        if !swing || d.abs() <= cap - PRONE_SOFT_MARGIN && !(moving && d != 0.0) {
            break 'swing;
        }
        let step = PRONE_SWING_DEG_PER_SEC * dt;
        let mut cand = if d.abs() < step {
            view
        } else if d > 0.0 {
            ps.prone_direction - step
        } else {
            ps.prone_direction + step
        };
        let mut searching = true;
        while !swing_fits(&mut body, cand, view) {
            if !searching {
                break 'swing;
            }
            let mut back = delta(ps.prone_direction, cand);
            searching = back.abs() > 1.0;
            if searching {
                back = if back > 0.0 { 1.0 } else { -1.0 };
            } else {
                blocked = true;
            }
            cand = normalize360_accurate(cand + back);
        }
        if fits45(view) && fits45(cand) {
            ps.prone_direction = cand;
        } else {
            blocked = true;
        }
    }
    d = delta(ps.prone_direction, view);
    'pull: {
        if !pull || d == 0.0 {
            break 'pull;
        }
        let mut cand = ps.prone_direction;
        let mut searching = true;
        loop {
            let fit = fits45(cand);
            if fit && swing_fits(&mut body, cand, view) {
                break;
            }
            if !searching {
                break 'pull;
            }
            searching = d.abs() > 1.0;
            if searching {
                d = if d > 0.0 { 1.0 } else { -1.0 };
            }
            blocked = true;
            shorts += angle_shorts(d);
            pushed = true;
            view = normalize360_accurate(view + d);
            d = delta(ps.prone_direction, view);
            if !fit {
                cand = normalize360_accurate(cand + d);
            }
        }
        ps.prone_direction = cand;
    }
    if d.abs() > cap {
        let excess = d - cap.copysign(d);
        shorts += angle_shorts(excess);
        pushed = true;
        view = normalize360_accurate(ps.prone_direction - cap.copysign(excess));
    }
    if blocked {
        ps.prone_blocked = true;
        let mut back = delta(old_view, view);
        if back.abs() <= 1.0 && back * delta(new_view, view) > 0.0 {
            back *= 0.98;
            view = normalize360_accurate(view + back);
            shorts += angle_shorts(back);
            pushed = true;
        }
    }
    ps.prone_body = body;
    if pushed {
        ps.view_yaw_shorts = Some(shorts & 0xffff);
        ps.view_capped[1] = Some(view);
        ps.yaw = view.to_radians();
    }
}

/// `PM_UpdatePronePitch` (0x3338c): both prone pitches ease toward the
/// ground's pitch under the body and under the view, or toward level with no
/// ground plane under the player. Airborne it checks the fit and flattens the
/// body, or refuses it (0x33434-0x33461): `pm_flags` 0x8000 and a forced
/// crouch for the client, while the body stays prone.
fn update_prone_pitch(ps: &mut PlayerState, world: &MoveWorld, dt: f32, events: &mut Vec<PmEvent>) {
    if ps.stance != Stance::Prone {
        return;
    }
    if !ps.on_ground {
        let q = ProneQuery {
            origin: ps.origin,
            yaw_deg: ps.prone_direction,
            entry: false,
            ground: false,
            length: PRONE_CHECK_LENGTH,
            rules: ps.rules,
        };
        match check_prone(world, q) {
            Some(body) => ps.prone_body = body,
            None => {
                push_event(events, EV_STANCE_FORCE_CROUCH);
                ps.prone_blocked = true;
            }
        }
    }
    let rules = ps.rules;
    // Both eases measure through the short-truncating `AngleDelta` (1.1
    // 0x334e3, 1.5 0x26fc8), so a pitch stops up to a short short of its
    // target. 1.1 stores through `AngleNormalize180` (0x33577), whole
    // shorts; 1.5 and UO through `AngleNormalize180Accurate` (0x2709a)
    // (cod11-mantle.md, "The prone pitches").
    let ease = |cur: f32, target: f32| {
        let d = angle_delta(target, cur, rules);
        if d == 0.0 {
            return cur;
        }
        let rate = f64::from(dt) * f64::from(PRONE_PITCH_DEG_PER_SEC);
        let next = if f64::from(d.abs()) > rate {
            (f64::from(cur) + rate.copysign(f64::from(d))) as f32
        } else {
            cur + d
        };
        if rules.v15() {
            normalize180_accurate(next)
        } else {
            short_normalize180(f64::from(next))
        }
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

/// `PM_UpdateLean` (0x32ac8), the last step of `PM_UpdateViewAngles` (0x33377), so it
/// reads the stance, ground and eye the previous move left
/// (docs/research/bsp-ibsp59-format.md, "Lean"). `alive` is `pm_type < 6`:
/// a dead body's lean only eases back. A linked player leans off the ground
/// and skips the wall clamp (0x32c63).
fn update_lean(ps: &mut PlayerState, input: &PmInput, alive: bool, world: &MoveWorld, dt: f32) {
    let mut dir = 0;
    // UO's deployed bipod eases any lean back (`PM_UpdateLean` 0x2ac45).
    if alive && (ps.on_ground || ps.linked) && ps.mounted.is_none() && !ps.deployed() {
        if input.lean_left {
            dir -= 1;
        }
        if input.lean_right {
            dir += 1;
        }
    }
    let scale = lean_scale(ps);
    let msec = msec(dt);
    let cur = ps.lean;
    ps.lean = match dir {
        0 => {
            let step = msec / LEAN_TIME_FROM_MS * scale;
            if cur > 0.0 {
                (cur - step).max(0.0)
            } else {
                (cur + step).min(0.0)
            }
        }
        // Past the stance's limit (a lean carried into prone) it snaps back.
        d if d < 0 => {
            let l = if cur > -scale {
                cur - msec / LEAN_TIME_TO_MS * scale
            } else {
                cur
            };
            l.max(-scale)
        }
        _ => {
            let l = if cur < scale {
                cur + msec / LEAN_TIME_TO_MS * scale
            } else {
                cur
            };
            l.min(scale)
        }
    };
    // 1.5 traces a linked player's lean too.
    // 1.1's skip is `pm_type == 1` alone (0x32c63): a linked corpse clamps.
    if ps.lean == 0.0 || alive && ps.linked && ps.rules == MoveRules::V1_1 {
        return;
    }
    // A sphere from the eye to where a full lean would put it; the lean
    // keeps no more than the fraction the sphere got (0x32c97-0x32d50).
    let start = ps.origin + Vec3::Z * ps.view_height();
    let side = ps.lean.signum();
    let end = aim::add_lean(start, ps.yaw, side, world.world.arith());
    let t = world.box_trace(
        start,
        end,
        Vec3::splat(-8.0),
        Vec3::splat(8.0),
        MASK_PLAYERSOLID,
    );
    let max = aim::unget_lean_fraction(t.fraction);
    if max < ps.lean.abs() {
        ps.lean = side * max;
    }
}

/// The lean's limit and ramp scale off the stance `PM_UpdateLean` reads from
/// `pm_flags` and the eye's lerp: prone, or easing up out of it, 0.25;
/// crouched, or easing down into it, 0.5; standing 1.
fn lean_scale(ps: &PlayerState) -> f32 {
    if ps.rules.v15() {
        // 1.5 0x25ce0..0x25cf5: no full lean standing.
        return if ps.effective_stance() == Stance::Prone {
            LEAN_SCALE_PRONE
        } else {
            LEAN_SCALE_CROUCH
        };
    }
    let lerping = ps.view_lerp_ms.is_some();
    let target = ps.view_lerp_target;
    if ps.stance == Stance::Prone
        || target == VIEW_PRONE
        || (lerping && target == VIEW_CROUCH && !ps.view_lerp_down)
    {
        LEAN_SCALE_PRONE
    } else if ps.ducked || (lerping && target == VIEW_CROUCH) {
        LEAN_SCALE_CROUCH
    } else {
        1.0
    }
}

/// A `jump` (`jumpbk` on a backward cmd) anim event raised inside a move:
/// by `PM_CheckJump`, forced, by `PM_GroundTrace`'s kickoff, unforced (it
/// takes the legs only once their timer reads under 50), or by
/// `PM_GroundTraceMissed` over a drop of more than 64, forced. The event's
/// timer is the clause's duration + 50; one raised by the move's first
/// ground trace is run down by the same move's `PM_DropTimers`
/// (player-model-anim-system.md, "The takeoff and the landing").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Takeoff {
    pub forced: bool,
    /// `cmd.forwardmove < 0`, which picks `jumpbk`.
    pub back: bool,
    /// Raised ahead of the move's `PM_DropTimers`.
    pub before_timers: bool,
}

impl Takeoff {
    fn raised(forced: bool, input: &PmInput) -> Self {
        Takeoff {
            forced,
            back: input.forward < 0.0,
            before_timers: false,
        }
    }
}

/// Q3 `bg_pmove.c` `PM_GroundTrace`'s kickoff test: the player's own velocity
/// is carrying it off the plane it is standing on. A plain `velocity.z <= 0.0`
/// test fails in its place: OVERCLIP leaves a hair of positive z after a floor
/// clip and the player would stay airborne.
fn thrown_off_ground(ps: &PlayerState, normal: Vec3) -> bool {
    ps.velocity.z > 0.0 && ps.velocity.dot(normal) > 10.0
}

/// `PM_CorrectAllSolid`'s nudges (`.rodata 0x70a40`), tried in this order.
const ALL_SOLID_NUDGES: [[f32; 3]; 26] = [
    [0.0, 0.0, 1.0],
    [-1.0, 0.0, 1.0],
    [0.0, -1.0, 1.0],
    [1.0, 0.0, 1.0],
    [0.0, 1.0, 1.0],
    [-1.0, 0.0, 0.0],
    [0.0, -1.0, 0.0],
    [1.0, 0.0, 0.0],
    [0.0, 1.0, 0.0],
    [0.0, 0.0, -1.0],
    [-1.0, 0.0, -1.0],
    [0.0, -1.0, -1.0],
    [1.0, 0.0, -1.0],
    [0.0, 1.0, -1.0],
    [-1.0, -1.0, 1.0],
    [1.0, -1.0, 1.0],
    [1.0, 1.0, 1.0],
    [-1.0, 1.0, 1.0],
    [-1.0, -1.0, 0.0],
    [1.0, -1.0, 0.0],
    [1.0, 1.0, 0.0],
    [-1.0, 1.0, 0.0],
    [-1.0, -1.0, -1.0],
    [1.0, -1.0, -1.0],
    [1.0, 1.0, -1.0],
    [-1.0, 1.0, -1.0],
];

/// The mover's own trace: 1.5's and UO's `PM_trace`
/// ([`MoveWorld::pm_trace`]), 1.1's plain `pm->trace`.
fn pm_trace(
    ps: &PlayerState,
    world: &MoveWorld,
    start: Vec3,
    end: Vec3,
    mins: Vec3,
    maxs: Vec3,
    mask: u32,
) -> crate::collision::Trace {
    if ps.rules.v15() {
        world.pm_trace(start, end, mins, maxs, mask)
    } else {
        world.box_trace(start, end, mins, maxs, mask)
    }
}

/// `PM_CorrectAllSolid` (`game.mp.i386.so` 0x30214): the first one-unit
/// nudge whose box is clear, the origin moved there and dropped up to a
/// unit, and that drop's trace as the ground trace. `None` when every nudge
/// is in solid, which leaves the player in the air with no jump origin, so
/// the slide's allsolid return and the step's gate hold it where it is
/// (docs/research/cod11-mantle.md, "A start inside a solid").
fn correct_all_solid(
    ps: &mut PlayerState,
    world: &MoveWorld,
    mask: u32,
) -> Option<crate::collision::Trace> {
    let (mins, maxs) = (ps.mins(), ps.maxs());
    let free = ALL_SOLID_NUDGES
        .iter()
        .map(|n| ps.origin + Vec3::from_array(*n))
        .find(|p| !pm_trace(ps, world, *p, *p, mins, maxs, mask).startsolid)?;
    let t = pm_trace(ps, world, free, free - Vec3::Z, mins, maxs, mask);
    ps.origin = t.endpos;
    Some(t)
}

/// `PM_GroundTraceMissed`'s `pml.almostGroundPlane` on 1.5 (0x21a17) and
/// UO (0x2656d): a body already off the ground is near it when a unit's
/// drop meets a floor, one that has just left it when a 64-unit drop
/// meets one within that unit. 1.1's (0x3037c) clears the field it lacks.
fn near_ground_missed(ps: &PlayerState, world: &MoveWorld, mask: u32) -> bool {
    if !ps.rules.v15() {
        return false;
    }
    let reach = if ps.on_ground { 64.0 } else { 1.0 };
    let t = world.box_trace(
        ps.origin,
        ps.origin - Vec3::Z * reach,
        ps.mins(),
        ps.maxs(),
        world.pm_mask(mask),
    );
    if ps.on_ground {
        t.fraction < 1.0 / 64.0
    } else {
        t.fraction < 1.0
    }
}

/// Q3 `bg_pmove.c` `PM_GroundTrace`.
fn ground_trace(ps: &mut PlayerState, world: &MoveWorld, mask: u32, input: &PmInput) {
    // From a quarter unit above the origin to a quarter below (0x304c0..
    // 0x304d3, `.rodata 0x70ba8`): a box resting on a face starts clear. On
    // a mounted gun from the origin to a unit below (0x304a5..0x304b6).
    let reach = if ps.mounted.is_some() {
        (0.0, 1.0)
    } else {
        (0.25, 0.25)
    };
    let (x, y) = (ps.origin.x, ps.origin.y);
    let end = ps.origin - Vec3::Z * reach.1;
    let mut t = pm_trace(
        ps,
        world,
        ps.origin + Vec3::Z * reach.0,
        end,
        ps.mins(),
        ps.maxs(),
        mask,
    );
    if t.allsolid {
        match correct_all_solid(ps, world, mask) {
            Some(drop) => t = drop,
            None => {
                ps.jump_origin_z = 0.0;
                ps.jump_penalty = false;
                lose_ground(ps);
                return;
            }
        }
    }
    // A start in solid that is not all solid (a terrain clip's, which never
    // reads all solid) is traced again from a thousandth under the origin
    // (0x30540..0x3058c, `.rodata 0x70bac`); a start in solid there too
    // leaves the ground, the jump's origin kept (0x3065e). 1.5 0x21e26 and
    // UO alike (cod11-mantle.md, "A start in solid on the ground trace").
    // The x, y and end are the first trace's, the z the corrected origin's.
    if t.startsolid {
        let from = Vec3::new(x, y, ps.origin.z - 0.001);
        t = pm_trace(ps, world, from, end, ps.mins(), ps.maxs(), mask);
        if t.startsolid {
            lose_ground(ps);
            return;
        }
    }
    // Any hit ends the jump's step allowance, thrown off or not (0x305c8).
    // 1.5 keeps the takeoff height through a landing (docs/research/
    // cod15-movement.md, "The jump penalty").
    if t.fraction < 1.0 && ps.rules == MoveRules::V1_1 {
        ps.jump_origin_z = 0.0;
    }
    let thrown_off = thrown_off_ground(ps, t.normal);
    // 1.5's steep branch ends the penalty (0x220f5, 0x22107).
    if ps.rules.v15() && t.fraction < 1.0 && !thrown_off && t.normal.z < MIN_WALK_NORMAL {
        ps.jump_penalty = false;
        ps.jump_origin_z = 0.0;
    }
    // The kickoff raises the takeoff anim unforced, a step off a drop of
    // more than 64 forced (`PM_GroundTrace` 1.1 0x30474, 1.5 0x31ccd's
    // caller, UO alike; `PM_GroundTraceMissed` 1.1 0x3037c, 1.5 0x31a17).
    // Not on the rungs: retail's kickoff test opens with `pm_flags` 0x10.
    if t.fraction < 1.0 && thrown_off && !ps.on_ladder {
        ps.takeoff.get_or_insert(Takeoff::raised(false, input));
    } else if t.fraction >= 1.0 && ps.on_ground && !ps.on_ladder {
        // `PM_GroundTraceMissed` traces without `PM_trace`, under the
        // move's mask as it stands.
        let drop = world.box_trace(
            ps.origin,
            ps.origin - Vec3::Z * 64.0,
            ps.mins(),
            ps.maxs(),
            world.pm_mask(mask),
        );
        if drop.fraction >= 1.0 {
            ps.takeoff.get_or_insert(Takeoff::raised(true, input));
        }
    }
    ps.ground_plane = (t.fraction < 1.0 && !thrown_off).then_some(t.normal);
    ps.near_ground = if t.fraction < 1.0 {
        !thrown_off
    } else {
        near_ground_missed(ps, world, mask)
    };
    if t.fraction < 1.0 && t.normal.z >= MIN_WALK_NORMAL && !thrown_off {
        ps.on_ground = true;
        // `groundEntityNum` is the trace's own entity (0x30732).
        ps.ground_entity = world.entity_num(&t);
        ps.ground_normal = t.normal;
        ps.ground_surface_flags = t.surface_flags;
    } else {
        ps.on_ground = false;
        ps.ground_normal = Vec3::Z;
        ps.ground_surface_flags = 0;
    }
}

/// What a ground trace that cannot stand the body clears: `groundEntityNum`
/// 1023 and `pml.groundPlane`, `pml.walking` and 1.5's near flag.
fn lose_ground(ps: &mut PlayerState) {
    ps.ground_plane = None;
    ps.near_ground = false;
    ps.on_ground = false;
    ps.ground_normal = Vec3::Z;
    ps.ground_surface_flags = 0;
}

/// Feet, waist and eye point-contents samples.
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

/// `PM_Friction`: ground friction only while walking in water level <= 1,
/// plus a water term that already applies while wading. Unlike RTCW's it has
/// no ladder term (game.mp.i386.so 0x2e460, cgame 0x30007430).
fn friction(ps: &mut PlayerState, world: &MoveWorld, dt: f32) {
    if world.world.arith() == crate::patch::Arith::Msvc {
        return friction_msvc(ps, dt);
    }
    // `PM_Friction` (0x2e460) on the x87: the speed is stored as a float,
    // the drop and the scale stay on the stack, and each component is
    // rounded once.
    let v = ps.velocity.as_dvec3();
    let vz = if ps.on_ground { 0.0 } else { v.z };
    let speed = ((v.x * v.x + v.y * v.y) + vz * vz).sqrt() as f32;
    if speed < 1.0 {
        ps.velocity.x = 0.0;
        ps.velocity.y = 0.0;
        // 1.5 stops the fall too (0x1eb4f), at a jump's apex as well.
        if ps.rules.v15() {
            ps.velocity.z = 0.0;
        }
        return;
    }
    if ps.rules.v15() {
        return friction_stored(ps, speed, dt, false);
    }
    let (s, dt) = (f64::from(speed), f64::from(dt));
    let mut drop = 0.0;
    // Slick ground and a hit's knockback slide free of the ground term
    // (0x2e4ed, 0x2e4fb).
    if ps.on_ground
        && ps.water_level <= 1
        && !on_slick(ps)
        && ps.knockback_flags & PMF_TIME_DAMAGE == 0
    {
        let mut control = s.max(f64::from(PM_STOPSPEED));
        if ps.knockback_flags & PMF_TIME_KNOCKBACK != 0 {
            control *= f64::from(KNOCKBACK_FRICTION_SCALE);
        } else if ps.jump_penalty && ps.knockback_ms <= JUMP_PENALTY_MS {
            control *= f64::from(jump_penalty_scale(ps.knockback_ms));
        }
        drop += control * f64::from(PM_FRICTION) * dt;
    }
    if ps.water_level > 0 {
        drop += f64::from(ps.water_level) * s * f64::from(WATER_FRICTION) * dt;
    }
    let scale = (s - drop).max(0.0) / s;
    ps.velocity = x87::scale(ps.velocity, scale);
}

/// The Windows cgames' `PM_Friction` (cgame_mp_x86.dll 1.1 0x30007430,
/// 1.5 0x30007b40, UO 0x30008470; cod15-movement.md 8): the speed sums
/// `(vz^2 + vy^2) + vx^2` and is kept on 1.1's 53-bit stack, stored on
/// 1.5's and UO's; the control, the drop, the new speed and its ratio stay
/// on the stack, and each component is rounded once. The ground term is
/// `(frametime * 5.5) * control`, UO's `(control * frametime) * 5.5`, and
/// 1.5's jump penalty scale `pm_time * (1.5 / 1700) + 1` with the constant
/// folded into one float.
fn friction_msvc(ps: &mut PlayerState, dt: f32) {
    let v = ps.velocity.as_dvec3();
    let vz = if ps.on_ground { 0.0 } else { v.z };
    let mut s = ((vz * vz + v.y * v.y) + v.x * v.x).sqrt();
    if ps.rules.v15() {
        s = f64::from(s as f32);
    }
    if s < 1.0 {
        ps.velocity.x = 0.0;
        ps.velocity.y = 0.0;
        if ps.rules.v15() {
            ps.velocity.z = 0.0;
        }
        return;
    }
    let ft = f64::from(dt);
    let mut drop = 0.0;
    if ps.on_ground
        && ps.water_level <= 1
        && !on_slick(ps)
        && ps.knockback_flags & PMF_TIME_DAMAGE == 0
    {
        let mut control = s.max(f64::from(PM_STOPSPEED));
        if ps.knockback_flags & PMF_TIME_KNOCKBACK != 0 {
            control *= f64::from(KNOCKBACK_FRICTION_SCALE);
        } else if ps.jump_penalty && ps.knockback_ms <= JUMP_PENALTY_MS {
            let scale = if ps.knockback_ms >= 1700.0 {
                2.5
            } else {
                f64::from(ps.knockback_ms) * f64::from(0.000_882_352_94_f32) + 1.0
            };
            control *= scale;
        }
        drop = if ps.rules == MoveRules::Uo {
            control * ft * f64::from(PM_FRICTION)
        } else {
            ft * f64::from(PM_FRICTION) * control
        };
    }
    if ps.water_level > 0 {
        drop += f64::from(ps.water_level) * f64::from(WATER_FRICTION) * ft * s;
    }
    let scale = (s - drop).max(0.0) / s;
    ps.velocity = x87::scale(ps.velocity, scale);
}

/// 1.5b's `PM_Friction` past the stop (0x1ebba-0x1ed59), which UO shares:
/// the control, the drop, the new speed and its ratio to the speed are each
/// stored, and each component is the float times the stored ratio. A
/// `spectator`'s `pm_type` 4 term comes last.
fn friction_stored(ps: &mut PlayerState, speed: f32, dt: f32, spectator: bool) {
    let r = |x: f64| f64::from(x as f32);
    let s = f64::from(speed);
    let dt = f64::from(dt);
    let mut drop = 0.0;
    if ps.on_ground
        && ps.water_level <= 1
        && !on_slick(ps)
        && ps.knockback_flags & PMF_TIME_DAMAGE == 0
    {
        let mut control = r(s.max(f64::from(PM_STOPSPEED)));
        if ps.knockback_flags & PMF_TIME_KNOCKBACK != 0 {
            control = r(control * f64::from(KNOCKBACK_FRICTION_SCALE));
        } else if ps.jump_penalty && ps.knockback_ms <= JUMP_PENALTY_MS {
            control = r(f64::from(jump_penalty_scale(ps.knockback_ms)) * control);
        }
        drop = r(control * f64::from(PM_FRICTION) * dt + drop);
    }
    if ps.water_level > 0 {
        // Retail's instruction order, not Ghidra's print (cod15-movement.md 8).
        drop = r(s * f64::from(WATER_FRICTION) * f64::from(ps.water_level) * dt + drop);
    }
    if spectator {
        drop = r(s * f64::from(PM_SPECTATOR_FRICTION) * dt + drop);
    }
    let mut new = r(s - drop);
    if new < 0.0 {
        new = 0.0;
    }
    let scale = r(new / s);
    ps.velocity = x87::scale(ps.velocity, scale);
}

/// How a build's walk cmd scale rounds its steps (cod15-movement.md 8,
/// "The walk's cmd scale").
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum WishArith {
    /// 1.1d: the whole product on the x87 stack.
    Kept,
    /// 1.5b's and UO's Linux builds (0x1f08b): the max, the length, the
    /// base scale and each factor's product stored.
    Stored,
    /// The Windows cgames (1.1 0x300076e0, 1.5 0x30007e40, UO 0x30008770):
    /// the strafe product and the walk, run or sprint product stored, the
    /// rest kept at 53 bits.
    Msvc,
}

/// Desired direction (world space, unit or zero) and speed.
/// The walk mover's wish direction and speed, retail's `PM_CmdScale`
/// (game.mp.i386.so 0x2e690, called from `PM_WalkMove` at 0x2f2bf): Q3's
/// `speed * max / (127 * total)` over the cmd bytes, with a backpedal read
/// through `backSpeedScale` and a strafe through `strafeSpeedScale` before
/// the max is taken, then `walkSpeedScale` on the ADS walk and otherwise
/// `runSpeedScale` with `leanSpeedScale` on a lean, the stance scale
/// ([`stance_speed_scale`]), the wade scale and the weapon's `moveSpeedScale`. The cmd magnitude then
/// multiplies back in, so a lone full key wishes `ps.speed` and a diagonal
/// wishes no more (docs/research/cod11-mantle.md, "The wish speed").
///
/// `walk_slow` is the client's own fly-mode key and takes the walk scale.
/// Last comes the shellshock's [`SCALE_SHELLSHOCK`] on `wbuttons` 0x4
/// (0x2e959), inside the same unrounded product.
///
/// The direction is `PM_WalkMove`'s (0x2f2c7-0x2f433): `pml.forward` and
/// `pml.right` flattened, clipped onto the ground plane and normalized, then
/// combined by the cmd bytes and normalized again; the speed is that
/// length times the stored scale, left unrounded.
fn wish(
    ps: &PlayerState,
    input: &PmInput,
    weapon: Option<&WeaponDef>,
    arith: WishArith,
) -> (Vec3, f64) {
    let (f, r) = (
        (input.forward * 127.0).round(),
        (input.right * 127.0).round(),
    );
    let (fd, rd) = (f64::from(f), f64::from(r));
    let stored = arith == WishArith::Stored;
    let msvc = arith == WishArith::Msvc;
    let st = |x: f64| if stored { f64::from(x as f32) } else { x };
    // The two steps the Windows cgames store as well.
    let st_win = |x: f64| {
        if stored || msvc {
            f64::from(x as f32)
        } else {
            x
        }
    };
    let back = st(if f < 0.0 {
        (fd * f64::from(SCALE_BACK)).abs()
    } else {
        fd.abs()
    });
    let max = back.max(st_win((rd * f64::from(SCALE_STRAFE)).abs()));
    if max == 0.0 {
        return (Vec3::ZERO, 0.0);
    }
    let total = st((fd * fd + rd * rd).sqrt());
    let mut scale = st((f64::from(ps.speed) * max) / (total * 127.0));
    // UO walks an LMG's breakdown (0x23af5).
    let breakdown = ps.lmg && ps.weaponstate == weapon::WEAPON_BREAKDOWN;
    if ps.walking || input.walk_slow || breakdown && ps.rules == MoveRules::Uo {
        scale = st_win(scale * f64::from(SCALE_WALK));
    } else if ps.lean != 0.0 {
        // 1.1's Windows cgame stores the run scale's product before the
        // lean's (0x30007791); 1.5's and UO's take a lean as the walk.
        if msvc && ps.rules == MoveRules::V1_1 {
            scale = st_win(scale);
        }
        scale = st_win(scale * f64::from(SCALE_LEAN));
    } else if ps.sprinting {
        scale = st_win(scale * f64::from(SCALE_SPRINT));
    } else {
        // The run scale, 1.0.
        scale = st_win(scale);
    }
    // 1.5b and the Windows cgames blend a stance leg's two scales on the
    // stack (0x1f2b5, 0x1f335; 1.5 cgame 0x30007f79).
    let stance = if stored || msvc {
        stance_speed_scale_kept(ps)
    } else {
        f64::from(stance_speed_scale(ps))
    };
    if stance != 1.0 {
        scale = st(scale * stance);
    }
    if ps.water_level > 0 {
        // The Windows cgames multiply by the float 1/3 (1.5 cgame 0x30008008).
        let wade = if msvc {
            f64::from(ps.water_level) * f64::from(1.0f32 / 3.0)
        } else {
            st(f64::from(ps.water_level) / 3.0)
        };
        scale = st(scale * st(1.0 - wade * f64::from(WADE_SCALE)));
    }
    // A sprint leaves the weapon's scale out (UO 0x23af5).
    if let Some(w) = weapon.filter(|w| w.move_speed_scale > 0.0 && !ps.sprinting) {
        scale = st(scale * f64::from(w.move_speed_scale));
    }
    if input.shellshock {
        scale = st(scale * f64::from(SCALE_SHELLSHOCK));
    }
    let scale = scale as f32;

    let turn = if msvc {
        crate::patch::Arith::Msvc
    } else {
        crate::patch::Arith::Kept
    };
    let (forward, right) = view_vectors(ps, turn);
    let v15 = ps.rules.v15();
    // 1.5b's walk (0x2008c-0x203f5) clips and normalizes through its
    // stored `PM_ClipVelocity` and `VectorNormalize`; the Windows cgames'
    // `VectorNormalize` stores alike (1.1 0x30039cc0), and their inline
    // clip takes the reciprocal overclip (cod15-movement.md 8).
    let norm = |v: Vec3| {
        if v15 || msvc {
            x87::normalize_stored(v)
        } else {
            x87::normalize(v)
        }
    };
    let clip = |v: Vec3| {
        if msvc {
            clip_velocity_msvc(v, ps.ground_normal)
        } else {
            clip_velocity(v, ps.ground_normal, v15)
        }
    };
    let flat = |v: Vec3| norm(clip(v.with_z(0.0)));
    let (forward, right) = (flat(forward), flat(right));
    let wishvel = (forward.as_dvec3() * fd + right.as_dvec3() * rd).as_vec3();
    let sq = x87::dot(wishvel, wishvel);
    let len = if v15 || msvc {
        f64::from(sq as f32).sqrt() as f32
    } else {
        sq.sqrt() as f32
    };
    (norm(wishvel), f64::from(len) * f64::from(scale))
}

/// `PmoveSingle`'s `AngleVectors` of `ps.viewangles` (game.mp 0x34120,
/// 1.5 0x284ec, UO 0x2dbdd; the Windows cgames' at 1.1 0x3000c718), forward
/// and right. No roll. The sim's radians are the Linux builds' own; a
/// Windows cgame turns the degrees by its float constant instead
/// ([`x87::radians_msvc`]).
fn view_vectors(ps: &PlayerState, arith: crate::patch::Arith) -> (Vec3, Vec3) {
    // The sim's pitch is positive up; the view's is positive down.
    let [f, r, _] = if arith == crate::patch::Arith::Msvc {
        x87::angle_vectors_msvc_of_radians(Vec3::new(
            x87::radians_msvc(view_deg(-ps.pitch)),
            x87::radians_msvc(view_deg(ps.yaw)),
            0.0,
        ))
    } else {
        x87::angle_vectors_of_radians(Vec3::new(-ps.pitch, ps.yaw, 0.0))
    };
    (f, r)
}

/// `AngleVectors` of `(0, viewangles[YAW], 0)`'s forward, as `arith`'s
/// build turns it (see [`view_vectors`]).
fn yaw_forward(ps: &PlayerState, arith: crate::patch::Arith) -> Vec3 {
    if arith == crate::patch::Arith::Msvc {
        let rad = x87::radians_msvc(view_deg(ps.yaw));
        x87::angle_vectors_msvc_of_radians(Vec3::new(0.0, rad, 0.0))[0]
    } else {
        x87::angle_vectors_of_radians(Vec3::new(0.0, ps.yaw, 0.0))[0]
    }
}

/// The air mover's wish, retail's Q3-shaped scale (0x2e5bc, called from
/// `PM_AirMove` at 0x2f083): the magnitude rule over forward, right and up,
/// and the walk/run pick, with no stance, lean, weapon or wade factor. The
/// cmd's `upmove` dilutes the horizontal wish the way Q3's does.
fn wish_air(ps: &PlayerState, input: &PmInput) -> (Vec3, f32, f32) {
    let (f, r, u) = (input.forward * 127.0, input.right * 127.0, input.up * 127.0);
    if f.abs().max(r.abs()).max(u.abs()) <= 0.0 {
        return (Vec3::ZERO, 0.0, 0.0);
    }
    let scale = cmd_scale(ps, input);
    let fwd = Vec3::new(ps.yaw.cos(), ps.yaw.sin(), 0.0);
    let right = Vec3::new(ps.yaw.sin(), -ps.yaw.cos(), 0.0);
    let wishvel = fwd * f + right * r;
    (wishvel.normalize_or_zero(), wishvel.length(), scale)
}

/// Retail's Q3-shaped `PM_CmdScale` (0x2e5bc), shared by the air and ladder
/// moves: `speed * max / (127 * total)` over forward, right and up, then
/// the walk or run scale. It reads the cmd's own `upmove` (0x2e5e9), not
/// the stance bits: a prone body whose client sends `up` 0 falls off a
/// ledge on the undiluted wish (cod11-mantle.md, "The wish speed").
fn cmd_scale(ps: &PlayerState, input: &PmInput) -> f32 {
    let (f, r, u) = (input.forward * 127.0, input.right * 127.0, input.up * 127.0);
    let max = f.abs().max(r.abs()).max(u.abs());
    if max <= 0.0 {
        return 0.0;
    }
    let total = (f * f + r * r + u * u).sqrt();
    let mut scale = ps.speed * max / (127.0 * total);
    // 1.5 takes a lean as the walk here too (0x1eec9).
    let lean = ps.rules.v15() && ps.lean != 0.0;
    if ps.walking || input.walk_slow || lean {
        scale *= SCALE_WALK;
    } else if ps.sprinting {
        scale *= SCALE_SPRINT;
    }
    scale
}

/// 1.1d's `PM_CmdScale` (0x2e5bc) as `PM_LadderMove` receives it: the
/// length stored as a float, then `speed * max / (127 * length)` and the
/// walk scale left on the x87 stack, unrounded, as the function returns
/// it (cod11-mantle.md, "Ladders").
fn cmd_scale_x87(ps: &PlayerState, input: &PmInput) -> f64 {
    let (f, r, u) = (input.forward * 127.0, input.right * 127.0, input.up * 127.0);
    let max = f.abs().max(r.abs()).max(u.abs());
    if max <= 0.0 {
        return 0.0;
    }
    let total = (f * f + r * r + u * u).sqrt();
    let scale = f64::from(ps.speed) * f64::from(max) / (f64::from(total) * 127.0);
    if ps.walking || input.walk_slow {
        scale * f64::from(SCALE_WALK)
    } else {
        scale
    }
}

/// 1.5b's `PM_Accelerate` (0x1ed62), which its walk, air and ladder moves
/// all call: the speed along the wish and what it still lacks stored as
/// floats, the rate floored at `pm_stopspeed` (100) and stored once, capped
/// at the lack, and each component rounded once. The ground term's
/// `1 / ps.friction` is 1: only `ClientSpawn` writes the field, 1.0
/// (cod15-movement.md 8, "The walk's cmd scale").
fn accelerate_stored(ps: &mut PlayerState, wishdir: Vec3, wishspeed: f32, accel: f32, dt: f32) {
    let current = x87::dot(ps.velocity, wishdir) as f32;
    let add = wishspeed - current;
    if add <= 0.0 {
        return;
    }
    let control = wishspeed.max(PM_STOPSPEED);
    let speed = ((f64::from(accel) * f64::from(dt) * f64::from(control)) as f32).min(add);
    ps.velocity = (wishdir.as_dvec3() * f64::from(speed) + ps.velocity.as_dvec3()).as_vec3();
}

/// 1.1d's air accelerate, inline in `PM_AirMove` (0x2f14b-0x2f1cd): Q3's
/// with the walk's floor, the rate `max(wishspeed, 100)` (rodata 0x708ec)
/// times the frame time (`pm_airaccelerate` 1 folded away), capped at the
/// lack; the wish speed, the dot and the rate stay on the x87 stack and
/// each component is rounded once (cod11-mantle.md, "The walk's accel
/// floor").
fn air_accelerate(ps: &mut PlayerState, wishdir: Vec3, wishspeed: f64, dt: f32) {
    let add = wishspeed - x87::dot(ps.velocity, wishdir);
    if add <= 0.0 {
        return;
    }
    let speed = (wishspeed.max(f64::from(PM_STOPSPEED)) * f64::from(dt)).min(add);
    ps.velocity = (wishdir.as_dvec3() * speed + ps.velocity.as_dvec3()).as_vec3();
}

/// The ladder's facing, `vectoyaw(vLadderVec) + 180`, as the float
/// `PM_LadderMove` hands `AngleDelta`. `vectoyaw` (1.1 0x3dccc, 1.5
/// 0x338ab) rounds the double atan2's degrees to a float and adds 360 to a
/// negative one; 1.1 adds the 180 on the x87 stack, 1.5 and UO to a stored
/// float.
fn ladder_facing(v: Vec3, rules: MoveRules) -> f32 {
    let yaw = vectoyaw(v);
    if rules.v15() {
        yaw as f32 + 180.0
    } else {
        (yaw + 180.0) as f32
    }
}

/// `vectoyaw` (1.1 0x3dccc, 1.5 0x338ab): the double atan2's degrees
/// rounded to a float, 360 added to a negative one, 0 straight up or down.
/// 1.1 hands the sum back on the x87 stack, so it is left unrounded here.
fn vectoyaw(v: Vec3) -> f64 {
    if v.x == 0.0 && v.y == 0.0 {
        return 0.0;
    }
    let yaw = f64::from(f64::from(v.y).atan2(f64::from(v.x)).to_degrees() as f32);
    if yaw < 0.0 { yaw + 360.0 } else { yaw }
}

/// `AngleDelta`: `a - b` through a truncated 16-bit angle, in -180..180.
/// 1.1d's (0x3ec9c) subtracts on the x87 stack; 1.5b's (0x36c0a) and UO's
/// (0x3de5a) store the float difference and call `AngleNormalize180`, which
/// truncates the same way (cod11-mantle.md, "AngleDelta").
pub(crate) fn angle_delta(a: f32, b: f32, rules: MoveRules) -> f32 {
    if rules.v15() {
        short_normalize180(f64::from(a - b))
    } else {
        short_normalize180(f64::from(a) - f64::from(b))
    }
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
        // `AngleDelta` folds through the shorts, so the cap's 85 reads 84.
        ps.movement_dir = clamp_movement_dir(
            aim::angle_normalize_180(ps.prone_direction - ps.yaw.to_degrees()) as i32,
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
    // `vectoangles` (0x3de94) puts the yaw in 0..360, and `AngleDelta`
    // (0x3ec9c) and `AngleNormalize180` (0x3eb70) fold through whole 16-bit
    // shorts, so a back key on a body still sliding forward reads +180, not
    // -180 (cod11-movers.md 15, "A moving body's clip").
    let yaw = {
        let y = (moved.y as f64).atan2(moved.x as f64).to_degrees() as f32;
        if y < 0.0 { y + 360.0 } else { y }
    };
    let mut deg = aim::angle_normalize_180(yaw - ps.yaw.to_degrees()) as i32;
    if input.forward < 0.0 {
        deg = aim::angle_normalize_180(deg as f32 + 180.0) as i32;
    }
    ps.movement_dir = clamp_movement_dir(deg, MOVEMENT_DIR_CAP);
}

/// Q3 `bg_pmove.c` `PM_ClipVelocity` (game.mp 0x3460c): the dot, the
/// overclip's product or quotient and each component's difference stay on
/// the x87 stack, and only the stored component is rounded. 1.5b's
/// (0x1e8e5), which UO shares, stores the dot, the backoff and each
/// component's change (`stored`).
fn clip_velocity(vel: Vec3, normal: Vec3, stored: bool) -> Vec3 {
    let (v, n) = (vel.as_dvec3(), normal.as_dvec3());
    if stored {
        let r = |x: f64| f64::from(x as f32);
        let dot = r((v.x * n.x + v.y * n.y) + v.z * n.z);
        let backoff = r(if dot < 0.0 {
            dot * f64::from(OVERCLIP)
        } else {
            dot / f64::from(OVERCLIP)
        });
        let c = |i: usize| (v[i] - r(n[i] * backoff)) as f32;
        return Vec3::new(c(0), c(1), c(2));
    }
    let dot = v.dot(n);
    let backoff = if dot < 0.0 {
        dot * f64::from(OVERCLIP)
    } else {
        dot / f64::from(OVERCLIP)
    };
    (v - n * backoff).as_vec3()
}

/// The Windows cgames' `PM_ClipVelocity` as `PM_WalkMove` and `PM_AirMove`
/// inline it (1.1 `cgame_mp_x86.dll` 0x30008397, 0x30008016; 1.5
/// 0x30008bb3; UO 0x30009595): the dot summed `(z + y) + x` and kept, a
/// positive one times the float `1 / 1.001` (1.1 rodata 0x30069750,
/// 0x3f7fbe87) instead of divided by 1.001, each component rounded once
/// (cod15-movement.md 8).
fn clip_velocity_msvc(vel: Vec3, normal: Vec3) -> Vec3 {
    let (v, n) = (vel.as_dvec3(), normal.as_dvec3());
    let dot = (v.z * n.z + v.y * n.y) + v.x * n.x;
    let backoff = if dot < 0.0 {
        dot * f64::from(OVERCLIP)
    } else {
        dot * f64::from(1.0 / OVERCLIP)
    };
    (v - n * backoff).as_vec3()
}

/// The Windows cgames' `PM_Accelerate` (1.1 `cgame_mp_x86.dll` 0x30007560,
/// 1.5 0x30007cb0, UO 0x300085d0), which every move calls with a stored
/// wish speed and accel: the speed along the wish summed `(z + y) + x` and
/// kept, the lack `wishspeed - dot` stored, the rate `(max(wishspeed, 100)
/// * frametime) * accel` kept and capped at the stored lack, each component
/// rounded once. Its `1 / ps.friction` with a ground entity is 1, as in
/// [`accelerate_stored`] (cod15-movement.md 8).
fn accelerate_msvc(ps: &mut PlayerState, wishdir: Vec3, wishspeed: f32, accel: f32, dt: f32) {
    let (v, d) = (ps.velocity.as_dvec3(), wishdir.as_dvec3());
    let dot = (v.z * d.z + v.y * d.y) + d.x * v.x;
    let add = (f64::from(wishspeed) - dot) as f32;
    if add <= 0.0 {
        return;
    }
    let control = f64::from(wishspeed.max(PM_STOPSPEED));
    let speed = (control * f64::from(dt) * f64::from(accel)).min(f64::from(add));
    ps.velocity = (d * speed + v).as_vec3();
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
    // along the slope, so it costs no speed
    let msvc = world.world.arith() == crate::patch::Arith::Msvc;
    let arith = if msvc {
        WishArith::Msvc
    } else if ps.rules.v15() {
        WishArith::Stored
    } else {
        WishArith::Kept
    };
    let (dir, wishspeed) = wish(ps, input, weapon, arith);
    // Slick ground or a hit's knockback (0x2f492, 0x2f58c).
    let sliding = on_slick(ps) || ps.knockback_flags & PMF_TIME_DAMAGE != 0;
    let mut accel = f64::from(match move_stance(ps) {
        _ if sliding => 1.0,
        Stance::Stand => PM_ACCELERATE,
        Stance::Crouch => PM_DUCKED_ACCELERATE,
        Stance::Prone => PM_PRONE_ACCELERATE,
    });
    if ps.knockback_flags & PMF_TIME_KNOCKBACK != 0 {
        accel *= f64::from(KNOCKBACK_ACCEL_SCALE);
    }
    // Q3's `PM_Accelerate` inline, with the rate floored: a prone or
    // sighted wish of under 100 still gains 100's worth per frame, capped
    // at the wish (docs/research/cod11-mantle.md, "The walk's accel floor").
    // All of it on the x87 stack, the velocity rounded once per component
    // (0x2f4e4-0x2f57c). 1.5b stores the wish speed (0x203f5) and calls its
    // `PM_Accelerate`, and so do the Windows cgames (1.1 0x30008253,
    // 0x3000830f) with their own.
    if msvc {
        accelerate_msvc(ps, dir, wishspeed as f32, accel as f32, dt);
    } else if ps.rules.v15() {
        accelerate_stored(ps, dir, wishspeed as f32, accel as f32, dt);
    } else {
        let add = wishspeed - x87::dot(ps.velocity, dir);
        if add > 0.0 {
            let rate = wishspeed.max(f64::from(WALK_ACCEL_FLOOR));
            let speed = (accel * f64::from(dt) * rate).min(add);
            ps.velocity = (dir.as_dvec3() * speed + ps.velocity.as_dvec3()).as_vec3();
        }
    }
    // Q3's slick-or-knockback gravity, which the clip below turns into
    // ground speed.
    if sliding {
        ps.velocity.z = (f64::from(ps.velocity.z) - f64::from(ps.gravity) * f64::from(dt)) as f32;
    }
    // The clip onto the ground keeps the speed whenever it leaves the
    // velocity pointing the same way (0x2f5b8-0x2f6b3), so a landing that
    // still carries its fall turns it into ground speed.
    let before = ps.velocity;
    // The 1.5 cgame sums the speed `(z + y) + x` (0x30008b5c-0x30008b70).
    let sq = if msvc && ps.rules == MoveRules::V1_5 {
        let v = before.as_dvec3();
        (v.z * v.z + v.y * v.y) + v.x * v.x
    } else {
        x87::dot(before, before)
    };
    let speed = sq.sqrt() as f32;
    ps.velocity = if msvc {
        clip_velocity_msvc(ps.velocity, ps.ground_normal)
    } else {
        clip_velocity(ps.velocity, ps.ground_normal, ps.rules.v15())
    };
    if x87::dot(ps.velocity, before) > 0.0 {
        let dir = if ps.rules.v15() || msvc {
            x87::normalize_stored(ps.velocity)
        } else {
            x87::normalize(ps.velocity)
        };
        ps.velocity = x87::scale(dir, f64::from(speed));
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

/// Whether `PM_CheckLadderMove` returns at its `pm_time` test, keeping last
/// frame's ladder flag: on 1.1 any running timer (0x336f6), on 1.5 and UO
/// only a knockback's or a stun's, not the jump penalty's (1.5
/// 0x2734f-0x27369, UO `FUN_0003c9a6`'s opening test).
fn ladder_check_skipped(ps: &PlayerState) -> bool {
    match ps.rules {
        MoveRules::V1_1 => ps.knockback_ms > 0.0,
        MoveRules::V1_5 | MoveRules::Uo => {
            ps.knockback_ms > 0.0
                && ps.knockback_flags & (PMF_TIME_KNOCKBACK | PMF_TIME_DAMAGE) != 0
        }
    }
}

/// Retail `PM_CheckLadderMove` (game.mp.i386.so 0x336e8): a box trace
/// 30 units along the flat view (8 while walking, which also needs the
/// forward key), or along `-vLadderVec` while already on a ladder in the air;
/// the box is the body shrunk 6 per horizontal side from 8 above the feet to
/// its top. A prone body never takes a ladder. Sets `ps.on_ladder` and
/// `ps.ladder_normal` and returns the normal.
fn check_ladder_move(ps: &mut PlayerState, input: &PmInput, world: &MoveWorld) -> Option<Vec3> {
    if ladder_check_skipped(ps) {
        return ps.on_ladder.then_some(ps.ladder_normal);
    }
    let was_on_ladder = ps.on_ladder;
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
    ps.on_ladder = false;
    // `PM_GetEffectiveStance` 1 (0x33802), then the re-grab lock after a
    // jump, `pm_ladderJumpTime` 300 (0x33822): what lets a push-off leave.
    if move_stance(ps) == Stance::Prone || ps.since_jump_ms < LADDER_REGRAB_LOCK_MS {
        return None;
    }
    let mut probe_mins = ps.mins();
    probe_mins.x += LADDER_PROBE_SHRINK;
    probe_mins.y += LADDER_PROBE_SHRINK;
    probe_mins.z = LADDER_PROBE_FLOOR;
    let mut probe_maxs = ps.maxs();
    probe_maxs.x -= LADDER_PROBE_SHRINK;
    probe_maxs.y -= LADDER_PROBE_SHRINK;
    probe_maxs.z = probe_maxs.z.max(LADDER_PROBE_FLOOR);
    let t = pm_trace(
        ps,
        world,
        ps.origin,
        ps.origin + dir * tracedist,
        probe_mins,
        probe_maxs,
        MASK_PLAYERSOLID,
    );
    let is_ladder = |t: &crate::collision::Trace| {
        t.fraction < 1.0 && t.surface_flags & crate::collision::SURF_LADDER != 0
    };
    let mut ladder = is_ladder(&t) && (!walking || input.forward > 0.0);
    // 1.5 confirms a new grab along the face's normal (0x27652, 0x277a9).
    if ladder && ps.rules.v15() && !was_on_ladder {
        let back = pm_trace(
            ps,
            world,
            ps.origin,
            ps.origin - t.normal * tracedist,
            probe_mins,
            probe_maxs,
            MASK_PLAYERSOLID,
        );
        ladder = is_ladder(&back);
    }
    ps.on_ladder = ladder;
    if ladder {
        ps.ladder_normal = t.normal;
    }
    ladder.then_some(t.normal)
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
    // UO refuses a deployed bipod ahead of the latch (0x24483).
    if ps.since_jump_ms <= JUMP_COOLDOWN_MS - 1.0
        || move_stance(ps) != Stance::Stand
        || !input.jump
        || ps.deployed()
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
    match ps.rules {
        MoveRules::V1_1 => {
            ps.velocity.z = (2.0 * JUMP_HEIGHT * ps.gravity).sqrt();
            ps.jump_origin_z = ps.origin.z + JUMP_HEIGHT;
        }
        MoveRules::V1_5 | MoveRules::Uo => {
            // 1.5's takeoff (0x1f882): the penalty divides the squared
            // speed, then restarts with the timer stopped until the landing,
            // and the origin is the takeoff height itself.
            let mut v2 = 2.0 * JUMP_HEIGHT * ps.gravity;
            if ps.jump_penalty && ps.knockback_ms <= JUMP_PENALTY_MS {
                v2 = (f64::from(v2) / f64::from(jump_penalty_scale(ps.knockback_ms))) as f32;
            }
            ps.velocity.z = f64::from(v2).sqrt() as f32;
            ps.jump_penalty = true;
            ps.knockback_ms = 0.0;
            ps.jump_origin_z = ps.origin.z;
        }
    }
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
    // The ground jump's anim, forced; a ladder push-off raises none
    // (1.1 0x2eb98: the event sits in the `pm_flags & 0x10 == 0` arm).
    if ladder.is_none() {
        ps.takeoff = Some(Takeoff::raised(true, input));
    }
    true
}

/// The longest 1.5 jump penalty, `pm_time` at a low landing (1.5 0x20169).
const JUMP_PENALTY_MS: f32 = 1800.0;
/// A landing this far above the takeoff starts a shorter, harsher penalty
/// (1.5 rodata 0x756ec, 0x201e2).
const JUMP_PENALTY_HIGH_LANDING: f32 = 18.0;
const JUMP_PENALTY_HIGH_MS: f32 = 1200.0;
/// The landing's velocity scales, low and high (rodata 0x756e8, 0x756f0).
const JUMP_PENALTY_LANDING_SCALE: f32 = 0.65;
const JUMP_PENALTY_HIGH_SCALE: f32 = 0.5;

/// 1.5's penalty curve, the friction multiplier and the jump's divisor
/// alike (0x1e9f1, 0x1ea52): 1 at `pm_time` 0 rising linearly to 2.5 at
/// 1700, and 2.5 from there (rodata 0x7566c..0x75678).
fn jump_penalty_scale(pm_time: f32) -> f32 {
    if pm_time > 1699.0 {
        2.5
    } else {
        (f64::from(pm_time) * 1.5 * f64::from(0.000_588_235_27_f32) + 1.0) as f32
    }
}

/// The top of 1.5's `PM_WalkMove` (0x2003d..0x201e2): the first walking
/// frame after a jump starts the penalty's timer and takes speed off, a
/// timer over the penalty's (a landing stun) ends it instead.
fn jump_penalty_landing(ps: &mut PlayerState) {
    if !ps.jump_penalty {
        return;
    }
    if ps.knockback_ms > JUMP_PENALTY_MS {
        ps.jump_penalty = false;
        ps.jump_origin_z = 0.0;
        ps.velocity *= JUMP_PENALTY_LANDING_SCALE;
    } else if ps.knockback_ms == 0.0 {
        if ps.origin.z < ps.jump_origin_z + JUMP_PENALTY_HIGH_LANDING {
            ps.knockback_ms = JUMP_PENALTY_MS;
            ps.velocity *= JUMP_PENALTY_LANDING_SCALE;
        } else {
            ps.knockback_ms = JUMP_PENALTY_HIGH_MS;
            ps.velocity *= JUMP_PENALTY_HIGH_SCALE;
        }
    }
}

/// Retail `PM_LadderMove` (game.mp.i386.so 0x33944): the push-off, then a
/// wish of `forwardmove * 0.5 * upscale` up, where the view's pitch sets
/// `upscale = clamp((forward.z + 0.25) * 2.5, -1, 1)`, plus `rightmove * 0.2`
/// along the flat right laid on the ladder's plane, both times the cmd
/// scale; an inline accelerate with no friction; a vertical stop without the
/// forward key; a 16/s bleed of the sideways velocity without the strafe
/// key; and in the air, the press into the wall
/// (docs/research/cod11-mantle.md, "Ladders").
fn ladder_move(
    ps: &mut PlayerState,
    input: &PmInput,
    normal: Vec3,
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
    let v15 = ps.rules.v15();
    // Each value below rounds where the binaries store it: 1.1d keeps every
    // expression on the x87 stack, the scale included (0x33970-0x33d64);
    // 1.5b (0x27846) and UO (0x2cebc) store every variable
    // (cod11-mantle.md, "Ladders", "The ladder move's arithmetic").
    let r32 = |x: f64| x as f32;
    let (forward, right) = view_vectors(ps, world.world.arith());
    let upscale =
        r32((f64::from(forward.z) + f64::from(LADDER_UPSCALE_BIAS))
            * f64::from(LADDER_UPSCALE_GAIN))
        .clamp(-1.0, 1.0);
    // `ProjectPointOnPlane(pml.right, flat right, vLadderVec)` (0x339fa):
    // strafing slides along the face instead of into or off it.
    let flat_right = x87::normalize(right.with_z(0.0));
    let right = flat_right - normal * flat_right.dot(normal);
    let (f, r) = (input.forward * 127.0, input.right * 127.0);
    let scale = if v15 {
        f64::from(cmd_scale(ps, input))
    } else {
        cmd_scale_x87(ps, input)
    };
    let mut wishvel = Vec3::ZERO;
    if f != 0.0 {
        let climb = f64::from(upscale) * f64::from(LADDER_CLIMB_SCALE) * scale;
        // 1.1d stores the climb's scale before the key multiplies it.
        let climb = if v15 { climb } else { f64::from(r32(climb)) };
        wishvel.z = r32(f64::from(f) * climb);
    }
    if r != 0.0 {
        let side = scale * f64::from(LADDER_STRAFE_SCALE) * f64::from(r);
        for i in 0..3 {
            wishvel[i] = r32(side * f64::from(right[i]) + f64::from(wishvel[i]));
        }
    }
    let msvc = world.world.arith() == crate::patch::Arith::Msvc;
    if v15 {
        let sq = r32(x87::dot(wishvel, wishvel));
        let wishspeed = r32(f64::from(sq).sqrt());
        let wishdir = x87::normalize_stored(wishvel);
        // `PM_Accelerate` (1.5 0x1ed62 from 0x27a7e, UO 0x2379e from 0x2d0f4).
        if msvc {
            accelerate_msvc(ps, wishdir, wishspeed, LADDER_ACCELERATE, dt);
        } else {
            accelerate_stored(ps, wishdir, wishspeed, LADDER_ACCELERATE, dt);
        }
    } else if msvc {
        // The 1.1 cgame's ladder calls its `PM_Accelerate` (0x3000c1cc).
        let wishspeed = r32(x87::dot(wishvel, wishvel).sqrt());
        accelerate_msvc(
            ps,
            x87::normalize(wishvel),
            wishspeed,
            LADDER_ACCELERATE,
            dt,
        );
    } else {
        let wishspeed = f64::from(r32(x87::dot(wishvel, wishvel).sqrt()));
        let wishdir = x87::normalize(wishvel);
        // The accelerate is inline (0x33acb-0x33b8b): the walk's floored
        // rate, capped at what the wish still lacks. Its division by
        // `ps.friction` with a ground entity (0x33b4c) divides by 1.0:
        // `ClientSpawn` is the field's only writer on all three patches
        // (cod11-mantle.md, "Ladders").
        let add = wishspeed - x87::dot(ps.velocity, wishdir);
        if add > 0.0 {
            let rate = f64::from(LADDER_ACCELERATE)
                * f64::from(dt)
                * wishspeed.max(f64::from(LADDER_ACCEL_FLOOR));
            let speed = rate.min(add);
            ps.velocity = (wishdir.as_dvec3() * speed + ps.velocity.as_dvec3()).as_vec3();
        }
    }
    if f == 0.0 {
        // no climb key: vertical velocity decays toward zero by gravity
        // instead of falling (0x33b95-0x33bf9)
        let g = f64::from(ps.gravity) * f64::from(dt);
        let vz = f64::from(ps.velocity.z);
        if ps.velocity.z > 0.0 {
            ps.velocity.z = r32(vz - g).max(0.0);
        } else {
            ps.velocity.z = r32(vz + g).min(0.0);
        }
    }
    if r == 0.0 {
        // No strafe key: the velocity along the flat right loses 16/s of
        // itself, at least a unit a frame, and all of it once that is more
        // than it has (0x33c0a-0x33ce4).
        let flat = Vec3::new(right.x, right.y, 0.0);
        let side = if v15 {
            x87::normalize_stored(flat)
        } else {
            x87::normalize(flat)
        };
        let d = f64::from(ps.velocity.x) * f64::from(side.x)
            + f64::from(ps.velocity.y) * f64::from(side.y);
        let d = if v15 { f64::from(r32(d)) } else { d };
        if d != 0.0 {
            ps.velocity.x = r32(f64::from(ps.velocity.x) - d * f64::from(side.x));
            ps.velocity.y = r32(f64::from(ps.velocity.y) - d * f64::from(side.y));
            let drop = d * f64::from(dt) * f64::from(PM_LADDER_FRICTION);
            let mut drop = if v15 { f64::from(r32(drop)) } else { drop };
            if drop.abs() < d.abs() {
                if drop.abs() < 1.0 {
                    drop = if drop < 0.0 { -1.0 } else { 1.0 };
                }
                let left = d - drop;
                let left = if v15 { f64::from(r32(left)) } else { left };
                ps.velocity.x = r32(f64::from(side.x) * left + f64::from(ps.velocity.x));
                ps.velocity.y = r32(f64::from(side.y) * left + f64::from(ps.velocity.y));
            }
        }
    }
    // airborne wall glue (@0x33cf1, gated on pml.walking == 0): strip
    // velocity along the ladder normal and press back into the wall - 500
    // while the wish climbs, else 250 (@0x70cd4/0x70cd8). This is retail's
    // sticky grip, not a dismount hop; leaving happens via the push-off above.
    if !ps.on_ground {
        let (nx, ny) = (f64::from(normal.x), f64::from(normal.y));
        let d = nx * f64::from(ps.velocity.x) + ny * f64::from(ps.velocity.y);
        let d = if v15 { f64::from(r32(d)) } else { d };
        ps.velocity.x = r32(f64::from(ps.velocity.x) - d * nx);
        ps.velocity.y = r32(f64::from(ps.velocity.y) - d * ny);
        let k = if wishvel.z > 0.0 { -500.0 } else { -250.0 };
        ps.velocity.x = r32(nx * k + f64::from(ps.velocity.x));
        ps.velocity.y = r32(ny * k + f64::from(ps.velocity.y));
    }
    // no gravity while going up a ladder
    step_slide_move(ps, world, dt, false, MASK_PLAYERSOLID, Some(events));
    // PM_LadderMove @0x33d71: the legs face into the wall, and the cap here
    // is 75 degrees, not the 90 the ground path uses. `AngleDelta` (0x33daa)
    // truncates to a short before the `(int)` takes the degree.
    ps.movement_dir = clamp_movement_dir(
        angle_delta(ladder_facing(normal, ps.rules), view_deg(ps.yaw), ps.rules) as i32,
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
    // `PM_AirMove` opens with `PM_Friction` (0x2f045): off the ground only
    // its sub-unit stop and the water term can bite.
    friction(ps, world, dt);
    let (dir, len, scale) = wish_air(ps, input);
    let msvc = world.world.arith() == crate::patch::Arith::Msvc;
    if msvc {
        // The Windows cgames store the product too (1.1 0x30007fce) and
        // call their `PM_Accelerate` (0x30007fd7).
        accelerate_msvc(ps, dir, len * scale, PM_AIRACCELERATE, dt);
    } else if ps.rules.v15() {
        // 1.5b stores the product (0x1ffbb) and accelerates through the
        // walk's `PM_Accelerate`, floor and all (0x1ffd7).
        accelerate_stored(ps, dir, len * scale, PM_AIRACCELERATE, dt);
    } else {
        air_accelerate(ps, dir, f64::from(len) * f64::from(scale), dt);
    }
    // A plane too steep to stand on still steers the fall (0x2f1d3).
    if let Some(n) = ps.ground_plane {
        ps.velocity = if msvc {
            clip_velocity_msvc(ps.velocity, n)
        } else {
            clip_velocity(ps.velocity, n, ps.rules.v15())
        };
    }
    step_slide_move(ps, world, dt, true, mask, events);
    set_movement_dir(ps, input, dt);
}

/// What [`slide_move`] ran into.
#[derive(Clone, Copy)]
struct Slide {
    blocked: bool,
}

/// Q3 `bg_slidemove.c` `PM_SlideMove`.
fn slide_move(ps: &mut PlayerState, world: &MoveWorld, dt: f32, gravity: bool, mask: u32) -> Slide {
    const NUM_BUMPS: usize = 4;
    let (mins, maxs) = (ps.mins(), ps.maxs());

    // `pml.groundPlane`, which a steep plane sets too (0x3483b reads it, not
    // `pml.walking`); `on_ground` covers a state no ground trace has run on.
    let ground = ps.ground_plane.or(ps.on_ground.then_some(ps.ground_normal));
    // average of start and end velocity, matching the analytic parabola
    // Q3's `primal_velocity`: what a running timer hands back at the end.
    let mut primal = ps.velocity;
    let mut end_velocity = ps.velocity;
    if gravity {
        // The x87 keeps `v - g * frametime` unrounded into the average and
        // rounds each store once (0x34811-0x34838).
        let mut end_z = f64::from(ps.velocity.z) - f64::from(ps.gravity) * f64::from(dt);
        end_velocity.z = end_z as f32;
        if ps.rules.v15() || world.world.arith() == crate::patch::Arith::Msvc {
            // 1.5b stores it first (0x28e34) and averages the float
            // (cod15-movement.md 7), and so does every Windows cgame
            // (8).
            end_z = f64::from(end_velocity.z);
        }
        primal.z = end_velocity.z;
        ps.velocity.z = ((end_z + f64::from(ps.velocity.z)) * 0.5) as f32;
        if let Some(n) = ground {
            ps.velocity = clip_velocity(ps.velocity, n, ps.rules.v15());
        }
    }

    let mut planes: Vec<Vec3> = Vec::with_capacity(MAX_CLIP_PLANES);
    // never turn against the ground plane or back against the original move
    if let Some(n) = ground {
        planes.push(n);
    }
    // 1.5b's `VectorNormalize2` (0x43216) stores its steps too.
    planes.push(if ps.rules.v15() {
        x87::normalize_stored(ps.velocity)
    } else {
        x87::normalize(ps.velocity)
    });

    let v15 = ps.rules.v15();
    let mut time_left = dt;
    let mut bumps = 0;
    for _ in 0..NUM_BUMPS {
        // `VectorMA` on the x87 rounds once, into the float store.
        let end = (ps.origin.as_dvec3() + ps.velocity.as_dvec3() * f64::from(time_left)).as_vec3();
        let t = pm_trace(ps, world, ps.origin, end, mins, maxs, mask);
        if t.allsolid {
            // trapped in solid: keep the horizontal control, kill the fall
            ps.velocity.z = 0.0;
            return Slide { blocked: true };
        }
        if t.fraction > 0.0 {
            ps.origin = t.endpos;
        }
        if t.fraction == 1.0 {
            break;
        }
        world.touch(&t);
        bumps += 1;
        time_left = (f64::from(time_left) - f64::from(time_left) * f64::from(t.fraction)) as f32;

        if planes.len() >= MAX_CLIP_PLANES {
            ps.velocity = Vec3::ZERO;
            return Slide { blocked: true };
        }
        // same plane again: nudge out along it (`> 0.999`, 1.1d 0x34ad9,
        // 1.5b 0x291e4, UO 0x2ead8)
        if planes
            .iter()
            .any(|p| x87::dot(t.normal, *p) > f64::from(0.999f32))
        {
            ps.velocity += t.normal;
            continue;
        }
        planes.push(t.normal);

        for (i, &plane_i) in planes.iter().enumerate() {
            // 1.5b stores this dot before the compare (0x293b5).
            let into = x87::dot(ps.velocity, plane_i);
            let into = if v15 { f64::from(into as f32) } else { into };
            if into >= 0.1 {
                continue;
            }
            let mut clipped = clip_velocity(ps.velocity, plane_i, v15);
            let mut end_clipped = clip_velocity(end_velocity, plane_i, v15);

            for (j, &plane_j) in planes.iter().enumerate() {
                if j == i || x87::dot(clipped, plane_j) >= 0.1 {
                    continue;
                }
                clipped = clip_velocity(clipped, plane_j, v15);
                end_clipped = clip_velocity(end_clipped, plane_j, v15);
                if x87::dot(clipped, plane_i) >= 0.0 {
                    continue;
                }
                // two planes: slide along their crease
                // Two opposed planes cross to a zero vector, which
                // `VectorNormalize` leaves signed: head-on into a wall the
                // velocity comes out (0, 0, -0) (protocol doc, "Spectator").
                // 1.5b stores the normalize's steps and each dot (0x29667-0x29714).
                let cross = x87::cross(plane_i, plane_j);
                let crease = if v15 {
                    x87::normalize_stored(cross)
                } else {
                    x87::normalize_in_place(cross)
                };
                let along = |v: Vec3| {
                    let d = x87::dot(crease, v);
                    if v15 { f64::from(d as f32) } else { d }
                };
                clipped = x87::scale(crease, along(ps.velocity));
                end_clipped = x87::scale(crease, along(end_velocity));

                // three planes: nowhere left to go
                if planes
                    .iter()
                    .enumerate()
                    .any(|(k, &p)| k != i && k != j && x87::dot(clipped, p) < 0.1)
                {
                    ps.velocity = Vec3::ZERO;
                    return Slide { blocked: true };
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
    // Under a running `pm_time` the clips do not reach the velocity
    // (0x34f83-0x34f9c; docs/research/cod11-player-clip.md 8.5).
    if ps.knockback_ms != 0.0 {
        ps.velocity = primal;
    }
    Slide {
        blocked: bumps != 0,
    }
}

/// Q3 `bg_slidemove.c` `PM_StepSlideMove`. Step height 18, or 10 while prone
/// (retail picks the height off pm_flags bit 0x1 @0x35045, not ladder state).
/// A prone move ends in `PM_VerifyPronePosition` (0x346e0): a body that no
/// longer fits where the move left it goes back to where it started, with
/// no step event (docs/research/cod11-mantle.md, "The ground samples").
fn step_slide_move(
    ps: &mut PlayerState,
    world: &MoveWorld,
    dt: f32,
    gravity: bool,
    mask: u32,
    mut events: Option<&mut Vec<PmEvent>>,
) {
    let (start_o, start_v) = (ps.origin, ps.velocity);
    let raised = events.as_ref().map_or(0, |e| e.len());
    // The entry gate's and the down pass's early outs skip the tail, and
    // the check with it (0x350ce..0x3510c, 0x35377).
    if !step_slide(ps, world, dt, gravity, mask, events.as_deref_mut())
        || ps.stance != Stance::Prone
    {
        return;
    }
    let q = ProneQuery {
        origin: ps.origin,
        yaw_deg: ps.prone_direction,
        entry: false,
        ground: true,
        length: PRONE_CHECK_LENGTH,
        rules: ps.rules,
    };
    match check_prone(world, q) {
        Some(body) => ps.prone_body = body,
        None => {
            ps.origin = start_o;
            ps.velocity = start_v;
            if let Some(e) = events {
                e.truncate(raised);
            }
        }
    }
}

fn step_slide(
    ps: &mut PlayerState,
    world: &MoveWorld,
    dt: f32,
    gravity: bool,
    mask: u32,
    events: Option<&mut Vec<PmEvent>>,
) -> bool {
    let mut step_size = if ps.stance == Stance::Prone {
        STEPSIZE_PRONE
    } else {
        STEPSIZE
    };
    let start_o = ps.origin;
    let start_v = ps.velocity;
    let v15 = ps.rules.v15();
    // 1.5's top (0x2997f-0x299b2): a ladder ends the jump penalty, and so
    // does a penalty timer running in the air.
    let penalty_timed = ps.jump_penalty && ps.knockback_ms != 0.0;
    if v15 && (ps.on_ladder || ps.ground_plane.is_none() && penalty_timed) {
        ps.jump_penalty = false;
        ps.jump_origin_z = 0.0;
    }
    let slide = slide_move(ps, world, dt, gravity, mask);
    let blocked = slide.blocked;
    // And again for any airborne body, steep plane or not (0x299dd).
    if v15 && !ps.on_ground && ps.jump_penalty && ps.knockback_ms != 0.0 {
        ps.jump_penalty = false;
        ps.jump_origin_z = 0.0;
    }
    // The height the jump's step may reach: 1.1 stores it, 1.5 stores the
    // takeoff and adds the jump height while the penalty flag stands.
    let peak = match ps.rules {
        MoveRules::V1_1 => (ps.jump_origin_z.abs() > 0.001).then_some(ps.jump_origin_z),
        MoveRules::V1_5 | MoveRules::Uo => {
            ps.jump_penalty.then_some(ps.jump_origin_z + JUMP_HEIGHT)
        }
    };
    // The entry gate (0x35057-0x35112). Retail takes the down pass on every
    // grounded frame, not only a blocked one, where Q3 returns as soon as the
    // slide succeeded (docs/research/cod11-mantle.md, "The ground snap"). In
    // the air it steps only a blocked move: one still under its jump's origin,
    // with the step cut to reach no higher than that origin, or a climb up a
    // ladder ("The jump's step").
    let mut jump_step = false;
    if let Some(peak) = peak.filter(|&p| blocked && !ps.on_ground && start_o.z < p) {
        let reach = peak - start_o.z;
        if reach < 1.0 {
            return false;
        }
        step_size = reach.min(STEPSIZE);
        jump_step = true;
    }
    // A spectator's `groundEntityNum` is never ground traced and reads 0,
    // not `ENTITYNUM_NONE`, so the gate (0x3507f) always lets it through.
    // 1.5 takes a rising climber through blocked or not (0x29a70).
    let climbing = ps.on_ladder && ps.velocity.z > 0.0 && (blocked || v15);
    let through = ps.on_ground || ps.spectator || jump_step || climbing;
    if !through {
        return false;
    }
    let (mins, maxs) = (ps.mins(), ps.maxs());
    let (down_o, down_v) = (ps.origin, ps.velocity);
    // Retail's `ebx` (0x34fdf-0x34ff1): on a ground plane and not on a
    // ladder. 1.5 reads `pml.groundPlane`, a steep plane's too.
    let ground_plane = !ps.on_ladder
        && match ps.rules {
            MoveRules::V1_1 => ps.on_ground,
            MoveRules::V1_5 | MoveRules::Uo => ps.ground_plane.is_some(),
        };
    // The ground snap's reach: half a step in 1.1, 9 whatever the step in
    // 1.5 (rodata 0x75cb4).
    let snap = if v15 { 9.0 } else { step_size * 0.5 };

    // The step-up runs on a blocked slide only (0x35166): the up trace over
    // `stepSize + 1`, `stepUp` a unit under what it reached and nothing under
    // 1 (0x351ca-0x351e8), the slide again from up there
    // (docs/research/cod11-mantle.md, "The step-up stops a unit short").
    let mut step = 0.0;
    if blocked {
        let up = pm_trace(
            ps,
            world,
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
        let reach = step + if ground_plane { snap } else { 0.0 };
        let down = pm_trace(
            ps,
            world,
            ps.origin,
            ps.origin - Vec3::Z * reach,
            mins,
            maxs,
            mask,
        );
        // A down pass that meets a player drops the step and the snap and
        // keeps the plain slide (0x3533a; docs/research/cod11-player-clip.md 2.3).
        if world.entity_num(&down) < MAX_CLIENTS {
            ps.origin = down_o;
            ps.velocity = down_v;
            return false;
        }
        if down.fraction < 1.0 {
            ps.origin = down.endpos;
            ps.velocity = clip_velocity(ps.velocity, down.normal, ps.rules.v15());
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
    // A jump's step that ends at or above the jump's origin is reverted too
    // (0x35450-0x35468).
    let v = ps.velocity.truncate();
    let flat = v.dot((down_o - start_o).truncate());
    let stepped = v.dot((ps.origin - start_o).truncate());
    let peak = peak.unwrap_or(0.0);
    if flat + STEP_REVERT_EPS > stepped || jump_step && ps.origin.z >= peak {
        ps.origin = down_o;
        ps.velocity = down_v;
        // The ground snap proper (0x354cc-0x3557b): a reverted move on a
        // ground plane is pulled down by half a step size from the plain
        // slide's end, which is what walks a player down a kerb without a
        // fall and raises the negative step events the captures carry.
        if ground_plane {
            let down = pm_trace(
                ps,
                world,
                ps.origin,
                ps.origin - Vec3::Z * snap,
                mins,
                maxs,
                mask,
            );
            if down.fraction < 1.0 {
                ps.origin = down.endpos;
                ps.velocity = clip_velocity(ps.velocity, down.normal, ps.rules.v15());
            }
        }
    }
    // A jump's step that rose is held to the speed that just reaches the
    // jump's origin, and to none within 0.1 of it (0x355a4-0x35655).
    if jump_step && ps.origin.z > down_o.z {
        let left = peak - ps.origin.z;
        ps.velocity.z = if left < 0.1 {
            0.0
        } else {
            ps.velocity.z.min((2.0 * left * ps.gravity).sqrt())
        };
    }
    // The step-up and the snap both reach this, and so does a reverted step:
    // retail's tail is past every arm of the move (@0x35659), so everything
    // the entry gate let through announces its step.
    // 1.5 gates the tail, and the prone check with it, on the ground plane
    // the move started on (0x2a46f).
    let tail = if v15 { ground_plane } else { through };
    if !tail {
        return false;
    }
    if let Some(events) = events {
        step_view(ps, events, start_o.z, down_o.z, step_size);
    }
    true
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
    let units = units.clamp(STEP_VIEW_MIN, STEP_VIEW_MAX);
    events.push(PmEvent {
        event: EV_STEP_VIEW,
        parm: units + STEP_VIEW_BIAS,
    });
    ps.velocity *=
        STEP_SCALE_BASE + STEP_SCALE_GAIN * (1.0 - (ps.origin.z - start_z).abs() / step_size);
    // 0x3579c-0x3585c: a step of more than 3 on the ground pushes the bob
    // cycle on by 7 plus 1.25 per two units (at most 4), truncated, and
    // may land a footstep.
    if units.abs() > 3 && (ps.on_ground || ps.spectator) && footsteps_audible(ps) {
        let old = ps.bob_cycle;
        let push = (units.abs() / 2).min(4) as f32 * 1.25 + 7.0;
        ps.bob_cycle = ((f32::from(old) + push) as i32 & 0xff) as u8;
        ground_step_event(ps, old, ps.bob_cycle, false, true, events);
    }
}

/// `PM_ShouldMakeFootsteps` (0x3221c): upright by the eye's stance and not
/// walking the sight (`pm_flags` 0x80).
fn footsteps_audible(ps: &PlayerState) -> bool {
    ps.effective_stance() == Stance::Stand && !ps.walking
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collision::{CollisionWorld, test_world};
    use crate::movetrace::{Body, CONTENTS_BODY};

    fn flat() -> CollisionWorld {
        test_world(&[])
    }

    /// `probe_blastloop`'s slot 3 on retail (mantle doc, "A start inside a
    /// solid"): set down nine units inside a clip brush it stays exactly
    /// where it was put, a second and more of cmds with no input later. A
    /// box one unit deep is nudged out by `PM_CorrectAllSolid` instead.
    #[test]
    fn a_player_set_down_inside_a_brush_stays_there() {
        let w = test_world(&[(Vec3::new(-64.0, -64.0, -16.0), Vec3::new(64.0, 64.0, 9.125))]);
        let mw = MoveWorld::bare(&w);
        let mut deep = PlayerState::spawn(Vec3::new(0.0, 0.0, 0.0), 0.0);
        tick(&mut deep, &PmInput::default(), &mw, 150);
        assert_eq!(deep.origin, Vec3::ZERO);
        assert!(!deep.on_ground);

        let w = test_world(&[(Vec3::new(-64.0, -64.0, -16.0), Vec3::new(64.0, 64.0, 0.5))]);
        let mw = MoveWorld::bare(&w);
        let mut shallow = PlayerState::spawn(Vec3::new(0.0, 0.0, 0.0), 0.0);
        tick(&mut shallow, &PmInput::default(), &mw, 10);
        assert!((shallow.origin.z - 0.5).abs() < 0.2, "{:?}", shallow.origin);
        assert!(shallow.on_ground);
    }

    /// A shellshocked client's `wbuttons` 0x4 takes the walk's wish speed to
    /// 0.4: retail's shocked carbine runs at 89.6 and backs at 62.72
    /// (`server/tests/shellshock_ab.rs`).
    #[test]
    fn shellshock_bit_slows_the_walk_wish() {
        let ps = PlayerState::spawn(Vec3::new(0.0, 0.0, 0.1), 0.0);
        let run = PmInput {
            forward: 1.0,
            ..Default::default()
        };
        let shocked = PmInput {
            shellshock: true,
            ..run
        };
        let (_, free) = wish(&ps, &run, None, WishArith::Kept);
        let (_, slow) = wish(&ps, &shocked, None, WishArith::Kept);
        assert!((slow - free * 0.4).abs() < 1e-4, "{slow} {free}");
    }

    /// 1.5b's walk scale (0x1f08b) stores the length, the base scale and
    /// each factor, where 1.1d rounds the whole product once: a diagonal
    /// run at 190 wishes a float step more (cod15-movement.md 8, "The
    /// walk's cmd scale").
    #[test]
    fn the_1_5_walk_scale_rounds_each_step() {
        let mut ps = PlayerState::spawn(Vec3::new(0.0, 0.0, 0.1), 0.0);
        ps.rules = MoveRules::V1_5;
        assert_eq!(ps.speed, 190.0);
        let run = PmInput {
            forward: 1.0,
            right: 1.0,
            ..Default::default()
        };
        let len = f64::from(32258f32.sqrt());
        let (_, kept) = wish(&ps, &run, None, WishArith::Kept);
        let (_, stored) = wish(&ps, &run, None, WishArith::Stored);
        assert_eq!(kept / len, f64::from(f32::from_bits(0x3f87687d)));
        assert_eq!(stored / len, f64::from(f32::from_bits(0x3f87687e)));
    }

    /// The Windows cgames' walk scale (1.5 0x30007e40) stores the run
    /// scale's product before the stance's, where 1.1d rounds once: a
    /// crouched diagonal run wishes a float step less on every protocol's
    /// client (cod15-movement.md 8, "The walk's cmd scale").
    /// The Windows cgames' `PM_Accelerate` (1.1 0x30007560) caps the rate
    /// at the lack stored as a float, where 1.1d's inline one keeps it.
    #[test]
    fn the_windows_accelerate_stores_the_lack() {
        let dir = x87::normalize_stored(Vec3::new(1.0, 1.0, 0.0));
        let mut ps = PlayerState::spawn(Vec3::ZERO, 0.0);
        ps.velocity = Vec3::new(150.0, 0.0, 0.0);
        let mut kept = ps;
        accelerate_msvc(&mut ps, dir, 110.0, PM_ACCELERATE, 0.05);
        // 1.1d's walk (0x2f4e4-0x2f57c): the lack is under the rate of 49.5.
        let add = 110.0 - x87::dot(kept.velocity, dir);
        kept.velocity = (dir.as_dvec3() * add + kept.velocity.as_dvec3()).as_vec3();
        assert_eq!(ps.velocity, Vec3::new(152.78175, 2.7817473, 0.0));
        assert_eq!(kept.velocity, Vec3::new(152.78175, 2.781747, 0.0));
    }

    /// The Windows walk's inline clip multiplies a positive dot by the float
    /// `1 / 1.001` (1.1 rodata 0x30069750), a last bit off the division.
    #[test]
    fn the_windows_walk_clip_takes_the_reciprocal_overclip() {
        let n = x87::normalize_stored(Vec3::new(0.3, 0.1, 1.0));
        let v = Vec3::new(101.76, 30.0, 0.0);
        assert_eq!(
            clip_velocity_msvc(v, n),
            Vec3::new(92.62514, 26.955044, -30.44955)
        );
        assert_eq!(
            clip_velocity(v, n, false),
            Vec3::new(92.62514, 26.955046, -30.44955)
        );
    }

    #[test]
    fn the_windows_walk_scale_stores_the_run_product() {
        let run = PmInput {
            forward: 1.0,
            right: 1.0,
            ..Default::default()
        };
        let len = f64::from(32258f32.sqrt());
        let base = 190.0 * 127.0 / (32258f64.sqrt() * 127.0);
        let crouch = f64::from(SCALE_CROUCH);
        for rules in [MoveRules::V1_1, MoveRules::V1_5, MoveRules::Uo] {
            let mut ps = PlayerState::spawn(Vec3::new(0.0, 0.0, 0.1), 0.0);
            ps.rules = rules;
            ps.settle_stance(Stance::Crouch);
            let (_, kept) = wish(&ps, &run, None, WishArith::Kept);
            let (_, msvc) = wish(&ps, &run, None, WishArith::Msvc);
            assert_eq!(kept / len, f64::from((base * crouch) as f32), "{rules:?}");
            let windows = (f64::from(base as f32) * crouch) as f32;
            assert_eq!(msvc / len, f64::from(windows), "{rules:?}");
            assert_ne!(kept, msvc, "{rules:?}");
        }
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

    /// `PM_DeadMove` takes 20 off a grounded corpse's speed every frame on
    /// top of the friction, so the same knockback slides it a fraction of
    /// what it slides a live player: the retail probe measured 25 units
    /// against 60 for a flat 200 (docs/research/cod11-combat.md 5.7). The
    /// drop is per frame, so halving the frame length shortens the slide.
    #[test]
    fn a_corpse_sheds_twenty_units_a_frame_on_the_ground() {
        let w = flat();
        let w = MoveWorld::bare(&w);
        let slide = |dt: f32| {
            let mut ps = PlayerState::spawn(Vec3::new(0.0, 0.0, 0.125), 0.0);
            ps.velocity = Vec3::new(240.0, 0.0, 0.0);
            ps.knockback_ms = 120.0;
            ps.knockback_flags = PMF_TIME_KNOCKBACK;
            for _ in 0..(2.0 / dt) as usize {
                dead_move(&mut ps, &w, dt);
            }
            assert_eq!(ps.velocity, Vec3::ZERO);
            ps.origin.x
        };
        let mut ps = PlayerState::spawn(Vec3::new(0.0, 0.0, 0.125), 0.0);
        ps.velocity = Vec3::new(10.0, 0.0, 0.0);
        dead_move(&mut ps, &w, 0.016);
        assert_eq!(ps.velocity, Vec3::ZERO, "10 is under one frame's drop");
        let (coarse, fine) = (slide(0.033), slide(0.016));
        assert!((20.0..35.0).contains(&coarse), "{coarse}");
        assert!(fine < coarse - 5.0, "{fine} against {coarse}");
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

    /// The air accelerate floors its rate at 100 as the walk's does: 1.1d's
    /// inline one at 0x2f159 (rodata 0x708ec), 1.5b's and UO's
    /// `PM_Accelerate` (0x1ed62, `pm_stopspeed`). 1.5's air wish takes a
    /// lean as the walk (0x1eec9): a jump while leaning wishes 76 until
    /// the lean has eased back, and gains 100's 5 units in a 50 ms frame,
    /// not Q3's 3.8 (the snap rounds that to 4). 1.1's walk key does the
    /// same.
    #[test]
    fn a_wish_under_100_in_the_air_accelerates_at_100s_rate() {
        let world = test_world(&[]);
        let w = MoveWorld::bare(&world);
        for rules in [MoveRules::V1_1, MoveRules::V1_5, MoveRules::Uo] {
            let input = PmInput {
                forward: 1.0,
                walk_slow: rules == MoveRules::V1_1,
                lean_left: rules.v15(),
                ..Default::default()
            };
            let mut ps = PlayerState::spawn(Vec3::new(0.0, 0.0, 4000.0), 0.0);
            ps.rules = rules;
            // A lean carried off the ground, easing back in the air.
            ps.lean = 0.25;
            pmove(&mut ps, &input, &w, 0.05, &[]);
            assert!(!ps.on_ground, "{rules:?}");
            assert_eq!(
                ps.velocity.truncate(),
                glam::Vec2::new(5.0, 0.0),
                "{rules:?} lean {}",
                ps.lean
            );
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
        // settles at -59 for a 60-degree turn the other way: `AngleDelta`
        // folds 60 through the shorts to 59.996 and the cast truncates.
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
        assert_eq!(ps.movement_dir, 59, "prone, view turned 60 degrees");
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

    /// Full run on dirt: rate 0.335 ticks/ms, truncated per frame
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
        // Retail truncates the cycle per frame (fistp under control word
        // 0xc00 @0x32819), so at fixed 8 ms frames the 2.68-tick advance
        // drops to 2 and the period stretches to 512 ms: 5 or 6 in 3 s.
        assert!(
            steps.len() >= 5 && steps.len() <= 6,
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

    /// 1.1 runs `PM_Weapon` ahead of `PM_Footsteps` (0x34331, 0x34336), 1.5
    /// and UO behind it: a step that both fires and steps rings the shot
    /// first on 1.1 only.
    #[test]
    fn the_shot_and_the_footstep_ring_in_each_versions_order() {
        let def = WeaponDef {
            clip_size: 200,
            fire_time: 0.001,
            clip_index: 1,
            ammo_index: 1,
            ..WeaponDef::default()
        };
        let weapons = [None, Some(def)];
        let w = dirt_flat();
        let w = MoveWorld::bare(&w);
        let run = PmInput {
            forward: 1.0,
            attack: true,
            weapon: 1,
            ..PmInput::default()
        };
        for (rules, shot_first) in [(MoveRules::V1_1, true), (MoveRules::V1_5, false)] {
            let mut ps = PlayerState::spawn(Vec3::new(0.0, 0.0, 0.125), 0.0);
            ps.rules = rules;
            weapon::give(&mut ps, 1, 1);
            ps.weapon = 1;
            ps.ammoclip[1] = 200;
            let both = (0..125)
                .map(|_| pmove(&mut ps, &run, &w, 0.008, &weapons))
                .find(|ev| {
                    ev.iter().any(|e| e.event == weapon::EV_FIRE_WEAPON)
                        && ev.iter().any(|e| e.event == EV_FOOTSTEP_RUN_BASE + 6)
                })
                .expect("a step that fires and steps");
            let first = both
                .iter()
                .find(|e| e.event == weapon::EV_FIRE_WEAPON || e.event == EV_FOOTSTEP_RUN_BASE + 6)
                .unwrap();
            assert_eq!(
                first.event == weapon::EV_FIRE_WEAPON,
                shot_first,
                "{rules:?} {both:?}"
            );
        }
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
        ladder_move(&mut ps, &jump, n, &wall, 1.0 / 125.0, &mut events);
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
        ladder_move(&mut ps, &jump, n, &wall, 1.0 / 125.0, &mut events);
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

    /// The walk wishes `ps.speed`, not a fixed 190: a `g_speed` 250
    /// server runs its bodies at 250, and a speed of 0 stands still.
    #[test]
    fn the_run_takes_the_playerstate_s_speed() {
        let w = flat();
        let w = MoveWorld::bare(&w);
        let run = |speed: f32| {
            let mut ps = PlayerState::spawn(Vec3::ZERO, 0.0);
            ps.speed = speed;
            let input = PmInput {
                forward: 1.0,
                ..Default::default()
            };
            tick(&mut ps, &input, &w, 250);
            ps.velocity.truncate().length()
        };
        let h = run(250.0);
        assert!((h - 250.0).abs() < 5.0, "run speed {h}");
        assert_eq!(run(0.0), 0.0);
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
        // Every refused cmd raises `pm_flags` 0x8000; the next cmd starts
        // clear.
        assert!(ps.prone_blocked, "the refusal is announced");
        tick(&mut ps, &PmInput::default(), &w, 1);
        assert!(!ps.prone_blocked, "cleared once the press is gone");

        ps.yaw = 180f32.to_radians();
        tick(&mut ps, &prone, &w, 20);
        assert_eq!(
            ps.stance,
            Stance::Prone,
            "prone with room behind must be taken"
        );
        assert!(!ps.prone_blocked);
        assert!(
            normalize180_accurate(ps.prone_direction - 180.0).abs() < 0.01,
            "the body faces the view, at {}",
            ps.prone_direction
        );
    }

    fn stance_events(ps: &mut PlayerState, input: &PmInput, w: &MoveWorld, n: usize) -> Vec<i32> {
        let mut out = Vec::new();
        for _ in 0..n {
            for e in pmove(ps, input, w, 1.0 / 125.0, &[]) {
                if (EV_STANCE_FORCE_STAND..=EV_STANCE_FORCE_PRONE).contains(&e.event) {
                    out.push(e.event);
                }
            }
        }
        out
    }

    /// `PM_CheckDuck` 0x3196b-0x31996: every refused prone cmd tells the
    /// client to stand, or to crouch when it came from a crouch, unless the
    /// stance rides a held key (`wbuttons` 0x2).
    #[test]
    fn a_refused_prone_forces_the_stance_it_came_from() {
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
        let got = stance_events(&mut ps, &prone, &w, 3);
        assert_eq!(got, [EV_STANCE_FORCE_STAND; 3], "one per refused cmd");
        assert_eq!(ps.stance, Stance::Stand);

        let held = PmInput {
            stance_held: true,
            ..prone
        };
        assert!(stance_events(&mut ps, &held, &w, 3).is_empty());
        assert!(ps.prone_blocked, "a held key is refused all the same");

        let crouch = PmInput {
            crouch: true,
            ..Default::default()
        };
        tick(&mut ps, &crouch, &w, 5);
        let got = stance_events(&mut ps, &prone, &w, 1);
        assert_eq!(got, [EV_STANCE_FORCE_CROUCH]);
        assert_eq!(ps.stance, Stance::Crouch);
    }

    /// `PM_CheckDuck`'s rising arms (0x319a4-0x31b78): a prone player let go
    /// under a crouch-high ceiling crouches; with no room for that either it
    /// stays prone and is told so (142); a crouch under a ceiling it cannot
    /// stand in stays and is told so (141).
    #[test]
    fn a_rise_without_headroom_stays_down_and_says_so() {
        let w = crate::collision::test_world(&[
            // Room to crouch, not to stand.
            (
                Vec3::new(200.0, -100.0, 55.0),
                Vec3::new(400.0, 100.0, 120.0),
            ),
            // Room to lie only.
            (
                Vec3::new(600.0, -100.0, 40.0),
                Vec3::new(800.0, 100.0, 120.0),
            ),
        ]);
        let w = MoveWorld::bare(&w);
        let prone = PmInput {
            prone: true,
            ..Default::default()
        };
        let crouch = PmInput {
            crouch: true,
            ..Default::default()
        };
        let lie_down_at = |x: f32| {
            let mut ps = PlayerState::spawn(Vec3::ZERO, 0.0);
            tick(&mut ps, &PmInput::default(), &w, 20);
            tick(&mut ps, &prone, &w, 20);
            assert_eq!(ps.stance, Stance::Prone);
            // Off the floor's face, which a zero-length trace reads as solid.
            ps.origin = Vec3::new(x, 0.0, 0.125);
            ps
        };

        let mut ps = lie_down_at(700.0);
        assert_eq!(
            stance_events(&mut ps, &crouch, &w, 1),
            [EV_STANCE_FORCE_PRONE]
        );
        assert_eq!(
            stance_events(&mut ps, &PmInput::default(), &w, 1),
            [EV_STANCE_FORCE_PRONE]
        );
        assert_eq!(ps.stance, Stance::Prone);

        let mut ps = lie_down_at(300.0);
        assert!(stance_events(&mut ps, &PmInput::default(), &w, 1).is_empty());
        assert_eq!(ps.stance, Stance::Crouch, "no room to stand: crouch");
        assert_eq!(
            stance_events(&mut ps, &PmInput::default(), &w, 1),
            [EV_STANCE_FORCE_CROUCH]
        );
        assert_eq!(ps.stance, Stance::Crouch);
    }

    /// `PM_UpdatePronePitch`'s ease (0x334e2-0x33585): 1.1 truncates both
    /// pitches to whole shorts after each step, 1.5 keeps the float.
    #[test]
    fn the_prone_pitches_ease_in_whole_shorts_on_1_1() {
        let w = crate::collision::test_world(&[]);
        let w = MoveWorld::bare(&w);
        let eased = |rules| {
            let mut ps = PlayerState::spawn(Vec3::ZERO, 0.0);
            ps.rules = rules;
            ps.stance = Stance::Prone;
            ps.on_ground = true;
            ps.ground_plane = None;
            // -8673 shorts, the wire's value at the speedgrav capture's ct 48146.
            ps.prone_torso_pitch = -47.642_21;
            update_prone_pitch(&mut ps, &w, 0.008, &mut Vec::new());
            ps.prone_torso_pitch
        };
        assert_eq!(eased(MoveRules::V1_1), -8571.0 * (360.0 / 65536.0));
        assert!((eased(MoveRules::V1_5) - (-47.642_21 + 0.56)).abs() < 1e-5);
    }

    /// Every build's ease measures through the short-truncating
    /// `AngleDelta` (1.1 0x334e3, 1.5 0x26fc8): a pitch under a short off
    /// its target reads a difference of 0 and stays where it is.
    #[test]
    fn a_prone_pitch_a_fraction_of_a_short_off_level_stays() {
        let w = crate::collision::test_world(&[]);
        let w = MoveWorld::bare(&w);
        for rules in [MoveRules::V1_1, MoveRules::V1_5, MoveRules::Uo] {
            let mut ps = PlayerState::spawn(Vec3::ZERO, 0.0);
            ps.rules = rules;
            ps.stance = Stance::Prone;
            ps.on_ground = true;
            ps.ground_plane = None;
            ps.prone_torso_pitch = -0.003;
            update_prone_pitch(&mut ps, &w, 0.008, &mut Vec::new());
            assert_eq!(ps.prone_torso_pitch, -0.003, "{rules:?}");
        }
    }

    /// The pitch cap measures through `AngleDelta` on every build (1.1
    /// 0x33295, 1.5 0x26ba0..): a torso pitch 45.003 degrees off the view
    /// reads 45 and holds nothing.
    #[test]
    fn the_prone_pitch_cap_measures_in_whole_shorts() {
        for rules in [MoveRules::V1_1, MoveRules::V1_5, MoveRules::Uo] {
            let mut ps = PlayerState::spawn(Vec3::ZERO, 0.0);
            ps.rules = rules;
            ps.stance = Stance::Prone;
            ps.prone_torso_pitch = -0.003;
            // 45 down on the wire, 8192 shorts.
            ps.pitch = -view_rad(45.0);
            cap_prone_pitch(&mut ps);
            assert_eq!(ps.view_pitch_correction, 0.0, "{rules:?}");
            assert_eq!(ps.view_capped[0], None, "{rules:?}");
        }
    }

    /// 1.1's caps place the view through the short-truncating
    /// `AngleNormalize360` (0x33266) and `AngleNormalize180` (0x33346):
    /// the yaw on the grid in 0..360, the pitch on the grid. 1.5 places the
    /// pitch through `AngleNormalize180Accurate`, off the grid.
    #[test]
    fn a_capped_prone_view_lands_on_the_shorts_grid_on_1_1() {
        let w = flat();
        let w = MoveWorld::bare(&w);
        let on_grid = |deg: f32| {
            let s = f64::from(deg) * 65536.0 / 360.0;
            (s - s.round()).abs() < 1e-6
        };
        let mut ps = PlayerState::spawn(Vec3::ZERO, 0.0);
        ps.stance = Stance::Prone;
        ps.on_ground = true;
        ps.prone_direction = 10.3;
        ps.prone_torso_pitch = -4.1;
        // A view 147 degrees off the body and 60 down, both on the grid.
        ps.yaw = view_rad(-25000.0 * (360.0 / 65536.0));
        ps.pitch = -view_rad(10923.0 * (360.0 / 65536.0));
        update_prone_view(&mut ps, &PmInput::default(), &w, 0.008);
        let yaw = view_deg(ps.yaw);
        assert!((0.0..360.0).contains(&yaw) && on_grid(yaw), "yaw {yaw}");
        assert_eq!(
            yaw,
            short_normalize360(f64::from(ps.prone_direction - PRONE_YAWCAP))
        );
        assert_eq!(ps.view_capped[1], Some(yaw));
        let pitch = ps.view_capped[0].expect("the pitch is capped");
        assert!(on_grid(pitch), "pitch {pitch}");
        assert_eq!(pitch, short_normalize180(f64::from(-4.1f32 + 45.0)));

        ps.rules = MoveRules::V1_5;
        ps.pitch = -view_rad(10923.0 * (360.0 / 65536.0));
        cap_prone_pitch(&mut ps);
        assert_eq!(ps.view_capped[0], Some(-4.1f32 + 45.0));
    }

    /// `PM_LadderMove` (0x33daa) takes the legs' angle off `AngleDelta`
    /// before the `(int)`: a ladder 30.001 degrees round from the view
    /// reads 5461 shorts, 29.998 degrees, and the legs 29.
    #[test]
    fn the_ladder_legs_angle_truncates_through_the_shorts() {
        let w = crate::collision::test_world(&[]);
        let w = MoveWorld::bare(&w);
        let mut ps = PlayerState::spawn(Vec3::new(0.0, 0.0, 200.0), 0.0);
        ps.on_ground = false;
        let n = {
            let a = 210.001f64.to_radians();
            Vec3::new(a.cos() as f32, a.sin() as f32, 0.0)
        };
        ladder_move(&mut ps, &PmInput::default(), n, &w, 0.008, &mut Vec::new());
        assert_eq!(ps.movement_dir, 29);
    }

    /// `bg_ladder_yawcap` (1.1 0x32e7d, 1.5 0x26270): off the ground on
    /// the rungs facing 90, a view at 200 is pushed back 10 degrees and
    /// placed at 190, on the shorts grid on 1.1 and exactly on 1.5; on the
    /// ground, or inside the cap, it stays (mp_ship-dm-ladder-turn*.txt).
    #[test]
    fn a_climber_looking_away_is_held_100_off_the_rungs() {
        let at = |yaw: f32, ground: bool, rules: MoveRules| {
            let mut ps = PlayerState::spawn(Vec3::ZERO, yaw);
            ps.rules = rules;
            ps.on_ladder = true;
            ps.on_ground = ground;
            ps.ladder_normal = -Vec3::Y;
            cap_ladder_yaw(&mut ps);
            (ps.view_yaw_correction, ps.view_capped[1])
        };
        let (c, yaw) = at(200.0, false, MoveRules::V1_1);
        assert!((c + 10.0).abs() < 0.01, "{c}");
        assert_eq!(yaw, Some(short_normalize360(190.0)));
        assert_eq!(at(200.0, false, MoveRules::V1_5).1, Some(190.0));
        assert_eq!(at(-30.0, false, MoveRules::Uo).1, Some(350.0));
        assert_eq!(at(200.0, true, MoveRules::V1_1), (0.0, None));
        assert_eq!(at(189.0, false, MoveRules::V1_1), (0.0, None));
    }

    /// `SetClientViewAngle`'s prone arm (1.1 0x41e30, 1.5 0x3bf08): the
    /// yaw held 45 either side of the body, the pitch from 45 down to 15 up
    /// off the torso, both placed on the shorts grid.
    #[test]
    fn a_prone_player_given_a_view_is_held_in_the_cone() {
        let mut ps = PlayerState::spawn(Vec3::ZERO, 0.0);
        let free = prone_view_angle(&ps, [60.0, 100.0, 0.0]);
        assert_eq!(free, [60.0, 100.0, 0.0]);
        ps.stance = Stance::Prone;
        ps.prone_direction = 10.3;
        ps.prone_torso_pitch = -4.0;
        let held = prone_view_angle(&ps, [60.0, 100.0, 0.0]);
        assert_eq!(held[1], short_normalize360(f64::from(10.3f32 + 45.0)));
        assert_eq!(held[0], short_normalize180(f64::from(-4.0f32 + 15.0)));
        let held = prone_view_angle(&ps, [-60.0, -50.0, 0.0]);
        assert_eq!(held[1], short_normalize360(f64::from(10.3f32 - 45.0)));
        assert!(held[1] > 300.0);
        assert_eq!(held[0], short_normalize180(f64::from(-4.0f32 - 45.0)));
    }

    /// `PM_UpdatePronePitch`'s airborne arm (0x33434-0x33461): a prone body
    /// in the air that no longer fits raises `pm_flags` 0x8000 and event
    /// 141, and stays prone.
    #[test]
    fn an_airborne_prone_body_that_does_not_fit_forces_a_crouch() {
        let w = crate::collision::test_world(&[(
            Vec3::new(-40.0, 470.0, 50.0),
            Vec3::new(-20.0, 530.0, 300.0),
        )]);
        let w = MoveWorld::bare(&w);
        let mut ps = PlayerState::spawn(Vec3::ZERO, 0.0);
        let prone = PmInput {
            prone: true,
            ..Default::default()
        };
        tick(&mut ps, &PmInput::default(), &w, 20);
        tick(&mut ps, &prone, &w, 20);
        assert_eq!(ps.stance, Stance::Prone);
        assert!(stance_events(&mut ps, &prone, &w, 1).is_empty());

        ps.origin = Vec3::new(0.0, 500.0, 100.0);
        ps.on_ground = false;
        assert_eq!(
            stance_events(&mut ps, &prone, &w, 1),
            [EV_STANCE_FORCE_CROUCH]
        );
        assert!(ps.prone_blocked);
        assert_eq!(ps.stance, Stance::Prone);
    }

    /// `PM_CheckDuck` 0x3189c-0x318be: on a ladder the stance keys are taken
    /// off the cmd and the client is told to stand.
    #[test]
    fn a_climber_s_stance_keys_force_a_stand() {
        let w = flat();
        let w = MoveWorld::bare(&w);
        let mut ps = PlayerState::spawn(Vec3::ZERO, 0.0);
        tick(&mut ps, &PmInput::default(), &w, 20);
        ps.on_ladder = true;
        let crouch = PmInput {
            crouch: true,
            stance_held: true,
            ..Default::default()
        };
        assert_eq!(
            stance_events(&mut ps, &crouch, &w, 1),
            [EV_STANCE_FORCE_STAND]
        );
        assert_eq!(ps.stance, Stance::Stand);
    }

    /// `PM_StepSlideMove`'s last block (0x3579c-0x3585c): a grounded step of
    /// more than 3 units pushes the bob cycle on by 7 + 1.25 * min(step / 2,
    /// 4), and a step across a quarter of the cycle lands a footstep. A
    /// crouched body is not audible and keeps its cycle.
    #[test]
    fn a_step_up_pushes_the_bob_cycle() {
        let mat = 5;
        let stepped = |ducked: bool, bob: u8| {
            let mut ps = PlayerState::spawn(Vec3::new(0.0, 0.0, 8.0), 0.0);
            ps.on_ground = true;
            ps.ducked = ducked;
            ps.bob_cycle = bob;
            ps.ground_surface_flags = mat << 20;
            let mut events = Vec::new();
            step_view(&mut ps, &mut events, 0.0, 0.0, STEPSIZE);
            (ps.bob_cycle, events)
        };
        let (bob, events) = stepped(false, 60);
        assert_eq!(bob, 60 + 12);
        assert_eq!(events.len(), 2);
        assert_eq!(events[1].event, EV_FOOTSTEP_RUN_BASE + mat as i32);
        let (bob, events) = stepped(false, 0);
        assert_eq!((bob, events.len()), (12, 1), "no quarter crossed");
        let (bob, _) = stepped(true, 60);
        assert_eq!(bob, 60);
    }

    /// `BG_CheckProneValid`'s ground samples: a chest on a 4-unit slab and
    /// hips and feet on the floor behind it pitch the torso up by
    /// atan(4 / 24) and leave the legs flat.
    #[test]
    fn a_prone_body_bends_over_a_step_under_the_chest() {
        let w = crate::collision::test_world(&[
            (
                Vec3::new(-200.0, -200.0, -16.0),
                Vec3::new(200.0, 200.0, 0.0),
            ),
            (Vec3::new(-10.0, -200.0, 0.0), Vec3::new(200.0, 200.0, 4.0)),
        ]);
        let w = MoveWorld::bare(&w);
        let q = ProneQuery {
            origin: Vec3::new(0.0, 0.0, 4.125),
            yaw_deg: 0.0,
            entry: false,
            ground: true,
            length: PRONE_CHECK_LENGTH,
            rules: MoveRules::V1_1,
        };
        let body = check_prone(&w, q).expect("room to lie down");
        assert!(body.torso_height.abs() < 0.2, "{body:?}");
        let up = -(4.0f32 / 24.0).atan().to_degrees();
        assert!((body.torso_pitch - up).abs() < 0.2, "{body:?}");
        assert!(body.waist_pitch.abs() < 0.01, "{body:?}");
        // Without the ground samples the fit writes a flat body.
        let flat = check_prone(&w, ProneQuery { ground: false, ..q });
        assert_eq!(flat, Some(ProneBody::default()));
    }

    /// The entity's copy is whole once the eye has settled, follows the eye's
    /// leg into prone, and is flat for a standing body.
    #[test]
    fn the_entity_scales_the_prone_body_by_the_eye_leg() {
        let mut ps = PlayerState::spawn(Vec3::ZERO, 0.0);
        ps.prone_body = ProneBody {
            torso_height: 2.0,
            torso_pitch: 45.0,
            waist_pitch: -315.0,
        };
        assert_eq!(ps.entity_prone_body(0, 1000), ProneBody::default());
        ps.stance = Stance::Prone;
        let whole = ProneBody {
            torso_height: 2.0,
            torso_pitch: 45.0,
            waist_pitch: 45.0,
        };
        assert_eq!(ps.entity_prone_body(0, 1000), whole);
        ps.view_lerp_target = VIEW_PRONE;
        ps.view_lerp_down = true;
        let len = view_lerp_length(VIEW_PRONE, true, false);
        let half = ps.entity_prone_body(1000, 1000 + len / 2);
        assert!((half.torso_pitch - 22.5).abs() < 0.1, "{half:?}");
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

    /// A swing the body does not fit in is refused, and announced through
    /// `pm_flags` 0x8000 while the view sits past the cap (0x331c4).
    #[test]
    fn a_refused_swing_past_the_cap_raises_prone_blocked() {
        // Lying facing +x, the body runs along -x; a wall 20 units off on -y
        // stops it from turning toward +y.
        let w = crate::collision::test_world(&[(
            Vec3::new(-80.0, -60.0, -8.0),
            Vec3::new(80.0, -20.0, 72.0),
        )]);
        let w = MoveWorld::bare(&w);
        let mut ps = PlayerState::spawn(Vec3::new(0.0, 0.0, 0.125), 0.0);
        let prone = PmInput {
            prone: true,
            ..Default::default()
        };
        tick(&mut ps, &prone, &w, 20);
        assert_eq!(ps.stance, Stance::Prone);
        assert!(!ps.prone_blocked);
        let mut blocked = false;
        for _ in 0..200 {
            // The cmd keeps asking for a view 100 degrees off the body.
            ps.yaw = (ps.prone_direction + 100.0).to_radians();
            pmove(&mut ps, &prone, &w, 0.05, &[]);
            blocked |= ps.prone_blocked;
        }
        assert!(blocked, "the body stopped at {}", ps.prone_direction);
        assert!(ps.prone_direction < 90.0, "the wall held the body");
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

    /// The takeoff anim each arm raises (`Takeoff`): the jump forced and
    /// after the move's timers drop, the dive's kickoff unforced and ahead
    /// of them, a step off a drop past 64 forced, a shorter drop none.
    #[test]
    fn each_takeoff_raises_its_own_anim_event() {
        let w = flat();
        let w = MoveWorld::bare(&w);
        let run = PmInput {
            forward: 1.0,
            ..Default::default()
        };
        let mut ps = PlayerState::spawn(Vec3::ZERO, 0.0);
        tick(&mut ps, &run, &w, 30);
        pmove(&mut ps, &PmInput { jump: true, ..run }, &w, 0.008, &[]);
        assert_eq!(
            ps.takeoff,
            Some(Takeoff {
                forced: true,
                back: false,
                before_timers: false
            })
        );
        pmove(&mut ps, &run, &w, 0.008, &[]);
        assert_eq!(ps.takeoff, None, "cleared by the next move");

        let mut ps = PlayerState::spawn(Vec3::ZERO, 0.0);
        let back = PmInput {
            forward: -1.0,
            ..Default::default()
        };
        tick(&mut ps, &back, &w, 30);
        pmove(
            &mut ps,
            &PmInput {
                prone: true,
                ..back
            },
            &w,
            0.008,
            &[],
        );
        assert_eq!(
            ps.takeoff,
            Some(Takeoff {
                forced: false,
                back: true,
                before_timers: true
            })
        );

        for (height, lift) in [(100.0, true), (40.0, false)] {
            let shelf = test_world(&[(
                Vec3::new(-1024.0, -1024.0, 0.0),
                Vec3::new(0.0, 1024.0, height),
            )]);
            let shelf = MoveWorld::bare(&shelf);
            let mut ps = PlayerState::spawn(Vec3::new(-40.0, 0.0, height + 0.125), 0.0);
            let mut raised = None;
            for _ in 0..60 {
                pmove(&mut ps, &run, &shelf, 0.008, &[]);
                raised = raised.or(ps.takeoff);
            }
            assert!(ps.origin.x > 30.0 && !ps.on_ground, "{:?}", ps.origin);
            let want = lift.then_some(Takeoff {
                forced: true,
                back: false,
                before_timers: false,
            });
            assert_eq!(raised, want, "a {height} unit drop");
        }
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
        let world = crate::collision::synthetic_world(
            &[("textures/test/solid", crate::collision::CONTENTS_SOLID, 0)],
            &[(0, [-64.0, -64.0, -1100.0], [64.0, 64.0, -1000.0])],
        );
        let w = MoveWorld::bare(&world);
        let mut ps = PlayerState::spawn(Vec3::new(1.0, 0.0, 0.0), 0.0);
        ps.move_start = Vec3::ZERO;
        // 1 unit in 8 ms is 125 u/s: 200 is under twice that and stays.
        ps.velocity = Vec3::new(200.0, 0.0, 0.0);
        clamp_velocity_to_move(&mut ps, &w, 0.008);
        assert_eq!(ps.velocity.x, 200.0);
        ps.velocity = Vec3::new(300.0, 0.0, 40.0);
        clamp_velocity_to_move(&mut ps, &w, 0.008);
        assert!(ps.velocity.abs_diff_eq(Vec3::new(125.0, 0.0, 0.0), 1e-3));
    }

    /// A fall from rest moves exactly half its end velocity, and 1.1d's
    /// stack keeps the tie or takes the move by the origin's last bits:
    /// `probe_spawnview`'s 66 + 34 ms spawn thinks on mp_carentan
    /// (cod11-gametypes-re-bel.md 8.5) fell 3.0888 from these heights and
    /// 4.0068 from the rest, and 1.5b's stored floats took it only at -2
    /// and ended the fall from 4 at -0.00680161 (cod15-movement.md 7); the
    /// Windows clients fall as 1.1d on every patch (8).
    #[test]
    fn a_fall_from_rest_ties_the_tail_by_the_start_height() {
        let world = crate::collision::synthetic_world(
            &[("textures/test/solid", crate::collision::CONTENTS_SOLID, 0)],
            &[(0, [-64.0, -64.0, -1100.0], [64.0, 64.0, -1000.0])],
        );
        let w = MoveWorld::bare(&world);
        let end_as = |z: f32, rules: MoveRules, arith: crate::patch::Arith| {
            world.set_arith(arith);
            let mut ps = PlayerState::spawn(Vec3::new(0.0, 0.0, z), 0.0);
            ps.rules = rules;
            pmove(&mut ps, &PmInput::default(), &w, 0.066, &[]);
            pmove(&mut ps, &PmInput::default(), &w, 0.034, &[]);
            ps.origin.z
        };
        let end = |z: f32, rules: MoveRules| {
            let linux = if rules.v15() {
                crate::patch::Arith::Stored
            } else {
                crate::patch::Arith::Kept
            };
            end_as(z, rules, linux)
        };
        let fall = |z: f32, rules: MoveRules| ((z - end(z, rules)) * 10000.0).round() / 10000.0;
        // The Windows client stores only y and z of the tail and keeps the
        // trace's end: on every patch it falls as 1.1d does
        // (cod15-movement.md 8).
        for rules in [MoveRules::V1_1, MoveRules::V1_5, MoveRules::Uo] {
            let msvc = |z: f32| {
                let e = end_as(z, rules, crate::patch::Arith::Msvc);
                ((z - e) * 10000.0).round() / 10000.0
            };
            for z in [-2.0, -1.0, 0.0, 1.0, 2.0, 4.0] {
                assert_eq!(msvc(z), 3.0888, "{rules:?} on Windows from {z}");
            }
            for z in [-4.0, 8.0, 16.0, 100.0] {
                assert_eq!(msvc(z), 4.0068, "{rules:?} on Windows from {z}");
            }
        }
        for z in [-2.0, -1.0, 0.0, 1.0, 2.0, 4.0] {
            assert_eq!(fall(z, MoveRules::V1_1), 3.0888, "1.1 from {z}");
        }
        for z in [-4.0, 8.0, 16.0, 100.0] {
            assert_eq!(fall(z, MoveRules::V1_1), 4.0068, "1.1 from {z}");
        }
        for z in [-4.0, -1.0, 0.0, 1.0, 2.0, 4.0, 8.0, 16.0, 100.0] {
            assert_eq!(fall(z, MoveRules::V1_5), 4.0068, "1.5 from {z}");
        }
        assert_eq!(fall(-2.0, MoveRules::V1_5), 3.0888);
        assert_eq!((end(4.0, MoveRules::V1_5) * 1e8).round(), -680161.0);
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

    /// `PM_SlideMove`'s same-plane test is `> 0.999` (1.1d rodata 0x70d7c,
    /// 1.5b 0x75b80, UO 0x9ba40): a 5 degree ramp met from the flat floor
    /// (0.9962 against the ground plane) is a new plane and clips the
    /// velocity up the ramp, where a 0.99 test nudged the body off it.
    #[test]
    fn a_shallow_ramp_met_from_the_floor_is_a_new_plane() {
        let w = crate::collision::ramp_test_world(5.0, 0.0, 300.0);
        let w = MoveWorld::bare(&w);
        let mut ps = PlayerState::spawn(Vec3::new(-60.0, 0.0, 1.0), 0.0);
        tick(&mut ps, &PmInput::default(), &w, 40);
        assert!(ps.on_ground && ps.ground_plane.is_some(), "{}", ps.origin);
        ps.velocity = Vec3::new(320.0, 0.0, 0.0);
        let slide = slide_move(&mut ps, &w, 0.25, false, MASK_PLAYERSOLID);
        assert!(slide.blocked);
        assert!(ps.velocity.z > 20.0, "velocity {}", ps.velocity);
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
    fn a_running_timer_keeps_the_velocity_the_slide_clipped() {
        let w = test_world(&[(Vec3::new(50.0, -400.0, 0.0), Vec3::new(100.0, 400.0, 100.0))]);
        let w = MoveWorld::bare(&w);
        let slide = |timer: f32| {
            let mut ps = PlayerState::spawn(Vec3::new(30.0, 0.0, 1.0), 0.0);
            ps.velocity = Vec3::new(300.0, 300.0, 0.0);
            ps.knockback_ms = timer;
            slide_move(&mut ps, &w, 0.05, false, MASK_PLAYERSOLID);
            ps
        };
        let free = slide(0.0);
        assert!(
            free.velocity.x <= 0.0,
            "the wall took the x: {}",
            free.velocity
        );
        let held = slide(500.0);
        assert_eq!(held.origin, free.origin, "the move itself is the same");
        assert_eq!(held.velocity, Vec3::new(300.0, 300.0, 0.0));
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

    /// `PM_UpdateLean`'s ramps: a full lean in 350 ms, back in 280.
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
        tick(&mut ps, &lean_r, &w, 43);
        assert!(
            (ps.lean - 43.0 * 8.0 / 350.0).abs() < 1e-4,
            "lean {}",
            ps.lean
        );
        tick(&mut ps, &lean_r, &w, 20);
        assert_eq!(ps.lean, 1.0);
        tick(&mut ps, &PmInput::default(), &w, 20);
        assert!(
            (ps.lean - (1.0 - 160.0 / 280.0)).abs() < 1e-4,
            "lean {}",
            ps.lean
        );
        tick(&mut ps, &PmInput::default(), &w, 20);
        assert_eq!(ps.lean, 0.0);
    }

    /// Off the ground the lean keys do nothing and a lean eases back.
    #[test]
    fn an_airborne_player_does_not_lean() {
        let w = flat();
        let w = MoveWorld::bare(&w);
        let mut ps = PlayerState::spawn(Vec3::new(0.0, 0.0, 200.0), 0.0);
        ps.lean = 0.5;
        let lean_r = PmInput {
            lean_right: true,
            ..Default::default()
        };
        tick(&mut ps, &lean_r, &w, 5);
        assert!(!ps.on_ground);
        assert!(
            (ps.lean - (0.5 - 40.0 / 280.0)).abs() < 1e-4,
            "lean {}",
            ps.lean
        );
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
        assert!(ps.lean > -0.9, "wall should limit lean, got {}", ps.lean);
    }

    /// Prone tops the lean out at 0.25 (`PM_UpdateLean`'s stance scale).
    #[test]
    fn prone_caps_lean_at_a_quarter() {
        let w = flat();
        let w = MoveWorld::bare(&w);
        let mut ps = PlayerState::spawn(Vec3::ZERO, 0.0);
        let input = PmInput {
            prone: true,
            lean_right: true,
            ..Default::default()
        };
        tick(&mut ps, &input, &w, 125);
        assert_eq!(ps.stance, Stance::Prone);
        assert_eq!(ps.lean, 0.25);
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

    /// A full right lean at yaw 0 moves the eye `20 * right(0, 0, 16)`:
    /// 19.2 to -Y and 5.5 down (`AddLeanToPosition`).
    #[test]
    fn view_reflects_stance_and_lean() {
        let w = flat();
        let w = MoveWorld::bare(&w);
        let mut ps = PlayerState::spawn(Vec3::ZERO, 0.0);
        tick(&mut ps, &PmInput::default(), &w, 50);
        let v = ps.view();
        assert!((v.eye.z - VIEW_STAND).abs() < 1.0);
        let lean_r = PmInput {
            lean_right: true,
            ..Default::default()
        };
        tick(&mut ps, &lean_r, &w, 125);
        assert_eq!(ps.lean, 1.0);
        let d = ps.view().eye - (ps.origin + Vec3::Z * ps.view_height());
        let (s, c) = 16f32.to_radians().sin_cos();
        assert!(
            (d - Vec3::new(0.0, -20.0 * c, -20.0 * s)).length() < 1e-4,
            "{d}"
        );
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
    fn deep_water_has_no_swim() {
        // Retail 1.1 MP walks and falls through water: no swim, no sink
        // wish, no water jump (cod11-mantle.md, "Water").
        let w = pool_world();
        let w = MoveWorld::bare(&w);
        let mut ps = PlayerState::spawn(Vec3::new(0.0, -200.0, 20.0), 0.0);
        let mut fastest = 0.0f32;
        for _ in 0..60 {
            pmove(&mut ps, &PmInput::default(), &w, 0.016, &[]);
            fastest = fastest.max(-ps.velocity.z);
        }
        assert!(fastest > 200.0, "a free fall through the water: {fastest}");
        assert!(ps.on_ground, "on the pool floor at {}", ps.origin);
        assert_eq!(ps.water_level, 3);
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
        // two units off the face; settled climb speeds, no friction on the
        // rungs: 0.5 * upscale * 190, 0.625 level, clamped to 1 at 30 up
        // (snapped to whole units at the frame's end)
        let mut level = PlayerState::spawn(Vec3::new(33.0, 0.0, 5.0), 0.0);
        tick(&mut level, &input, &w, 100);
        let mut up = PlayerState::spawn(Vec3::new(33.0, 0.0, 5.0), 0.0);
        up.pitch = 30.0f32.to_radians();
        tick(&mut up, &input, &w, 100);
        assert!(level.on_ladder && up.on_ladder);
        assert_eq!(level.velocity.z, 59.0, "level climb");
        assert_eq!(up.velocity.z, 95.0, "climb looking up");
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
            let axial_bounds = |b: &crate::mapworld::Brush| -> ([f32; 3], [f32; 3]) {
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

    const FULL: f32 = 127.0;

    #[test]
    fn spectator_flies_forward_at_spectator_speed() {
        let mut ps = PlayerState::spawn(Vec3::ZERO, 90.0); // yaw 90 deg faces +Y
        ps.speed = SPEED_SPECTATOR;
        for _ in 0..250 {
            spectator_move(&mut ps, None, FULL, 0.0, 0.0, 0, 1.0 / 125.0);
        }
        let h = ps.velocity.truncate().length();
        assert!((h - SPEED_SPECTATOR).abs() < 6.0, "speed {h}");
        assert!(ps.velocity.y > 350.0 && ps.velocity.x.abs() < 1.0);
    }

    /// `PM_FlyMove` wishes along the full view and scales by
    /// `PM_CmdScale`: looking down flies down, half a key flies at half
    /// speed, and no key leaves the velocity alone, a -0 included
    /// (docs/protocol-1.1.md, "Spectator").
    #[test]
    fn spectator_flies_along_the_view_at_the_key_s_speed() {
        let mut ps = PlayerState::spawn(Vec3::ZERO, 0.0);
        ps.speed = SPEED_SPECTATOR;
        ps.pitch = -45f32.to_radians();
        for _ in 0..250 {
            spectator_move(&mut ps, None, FULL, 0.0, 0.0, 0, 1.0 / 125.0);
        }
        assert!(
            ps.velocity.z < -250.0 && ps.velocity.x > 250.0,
            "{}",
            ps.velocity
        );

        let mut ps = PlayerState::spawn(Vec3::ZERO, 0.0);
        ps.speed = SPEED_SPECTATOR;
        for _ in 0..250 {
            spectator_move(&mut ps, None, 63.5, 0.0, 0.0, 0, 1.0 / 125.0);
        }
        assert!(
            (ps.velocity.x - SPEED_SPECTATOR / 2.0).abs() < 1.0,
            "{}",
            ps.velocity
        );

        let mut ps = PlayerState::spawn(Vec3::ZERO, 0.0);
        ps.speed = SPEED_SPECTATOR;
        ps.velocity = Vec3::new(0.0, 0.0, -0.0);
        spectator_move(&mut ps, None, 0.0, 0.0, 0.0, 0, 1.0 / 125.0);
        assert_eq!(ps.velocity.z.to_bits(), 0x8000_0000);
    }

    /// The lean bits fly a spectator with no key down 512 (right) and up
    /// 256 (left), and down 256 with both. Retail's ramp under both, from
    /// `--probe-edge lean-fly` on mp_carentan (docs/protocol-1.1.md,
    /// "Spectator"): the cmd times it sent and the `velocity[2]` bits of the
    /// three snapshots that followed, from 0.9565565 at rest.
    #[test]
    fn spectator_lean_bits_fly_down_512_and_up_256() {
        use crate::net::msg::{WBUTTON_LEAN_LEFT, WBUTTON_LEAN_RIGHT};
        let both = WBUTTON_LEAN_LEFT | WBUTTON_LEAN_RIGHT;
        let mut ps = PlayerState::spawn(Vec3::ZERO, 0.0);
        ps.velocity = Vec3::new(0.0, 0.0, f32::from_bits(0x3f74_e0e3));
        let mut ct = 22441;
        for (cmds, bits) in [
            ([22461, 22478, 22495], 0xc2c8_543e_u32),
            ([22512, 22529, 22546], 0xc32c_8db8),
            ([22561, 22577, 22594], 0xc361_0530),
        ] {
            for t in cmds {
                let dt = cmd::frametime(t - ct);
                spectator_move(&mut ps, None, 0.0, 0.0, 0.0, both, dt);
                ct = t;
            }
            assert_eq!(
                ps.velocity.z.to_bits(),
                bits,
                "{} {:08x}",
                ps.velocity.z,
                ps.velocity.z.to_bits()
            );
        }

        for (wbuttons, vz) in [(WBUTTON_LEAN_RIGHT, -512.0), (WBUTTON_LEAN_LEFT, 256.0)] {
            let mut ps = PlayerState::spawn(Vec3::ZERO, 0.0);
            for _ in 0..125 {
                spectator_move(&mut ps, None, 0.0, 0.0, 0.0, wbuttons, 1.0 / 125.0);
            }
            assert_eq!(ps.velocity, Vec3::new(0.0, 0.0, vz));
        }
    }

    #[test]
    fn spectator_rises_with_upmove_and_stops_on_friction() {
        let mut ps = PlayerState::spawn(Vec3::ZERO, 0.0);
        for _ in 0..63 {
            spectator_move(&mut ps, None, 0.0, 0.0, FULL, 0, 1.0 / 125.0);
        }
        assert!(ps.velocity.z > 100.0, "climbing, vz {}", ps.velocity.z);
        for _ in 0..250 {
            spectator_move(&mut ps, None, 0.0, 0.0, 0.0, 0, 1.0 / 125.0);
        }
        assert!(ps.velocity.length() < 1.0, "friction must stop the fly");
    }

    /// A spectator flies in its own 16-unit hull and stops on a wall, with
    /// its centre 8.125 short of the face; a ledge under its hull it steps
    /// onto, in the air, and announces the step (docs/protocol-1.1.md,
    /// "Spectator").
    #[test]
    fn a_spectator_stops_on_a_wall_and_steps_onto_a_ledge() {
        let wall = (
            Vec3::new(64.0, -512.0, -16.0),
            Vec3::new(128.0, 512.0, 512.0),
        );
        let ledge = (
            Vec3::new(32.0, -512.0, -16.0),
            Vec3::new(128.0, 512.0, 100.0),
        );
        let cw = test_world(&[wall, ledge]);
        let w = MoveWorld::bare(&cw);
        let mut ps = PlayerState::spawn(Vec3::new(-200.0, 0.0, 96.0), 0.0);
        let mut events = Vec::new();
        for _ in 0..250 {
            events.extend(spectator_move(
                &mut ps,
                Some(&w),
                FULL,
                0.0,
                0.0,
                0,
                1.0 / 125.0,
            ));
        }
        assert_eq!(ps.origin.x, 64.0 - 8.125, "origin {}", ps.origin);
        assert_eq!(ps.origin.z, 108.125, "origin {}", ps.origin);
        assert!(!ps.spectator, "the flag outlived the move");
        let step = events
            .iter()
            .find(|e| e.event == EV_STEP_VIEW)
            .expect("a step event");
        assert_eq!(step.parm, 12 + STEP_VIEW_BIAS);
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
        // pass, which misses the bare radius; nothing gets past that. The
        // down pass's float fraction leaves the floor a few millionths under
        // 0.125, as retail's does.
        assert!(
            (ps.origin.z - 0.125).abs() < 1e-5 && (30.0..30.2).contains(&(60.0 - ps.origin.x)),
            "{:?}",
            ps.origin
        );
    }

    #[test]
    fn dead_mask_ignores_bodies() {
        let w = flat();
        let bodies = [body(20.0, 0.0, 0.0)];
        let mw = MoveWorld::new(&w, &bodies, 0);
        let mut ps = PlayerState::spawn(Vec3::new(0.0, 0.0, 0.125), 0.0);
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
        assert!(ps.lean < 0.9, "a body should limit lean, got {}", ps.lean);
    }

    fn v15(origin: Vec3) -> PlayerState {
        PlayerState {
            rules: MoveRules::V1_5,
            ..PlayerState::spawn(origin, 0.0)
        }
    }

    /// The apex of one standing jump at 125 fps from where `ps` stands.
    fn jump_apex(ps: &mut PlayerState, w: &MoveWorld) -> f32 {
        let start = ps.origin.z;
        let jump = PmInput {
            jump: true,
            ..Default::default()
        };
        pmove(ps, &jump, w, 0.008, &[]);
        let mut apex = ps.origin.z;
        while !ps.on_ground {
            pmove(ps, &PmInput::default(), w, 0.008, &[]);
            apex = apex.max(ps.origin.z);
        }
        apex - start
    }

    /// 1.5's jump penalty (docs/research/cod15-movement.md, "The jump
    /// penalty"): a landing starts an 1800 ms timer, a jump inside it takes
    /// off at the squared speed over the curve, and 1.1 has none of it.
    #[test]
    fn a_1_5_jump_inside_the_penalty_is_lower() {
        let w = flat();
        let w = MoveWorld::bare(&w);
        let mut ps = v15(Vec3::ZERO);
        tick(&mut ps, &PmInput::default(), &w, 30);
        // 125 fps jumps a little over the 39 (`a_125_fps_jump_goes_higher`).
        let first = jump_apex(&mut ps, &w);
        assert!((first - JUMP_HEIGHT).abs() < 3.0, "{first}");
        assert!(ps.jump_penalty);
        assert_eq!(ps.jump_origin_z, 0.0, "the takeoff height, kept");
        // The landing frame armed the timer; a jump 500 ms on pays the curve
        // at ~1300 ms left: 39 / (1 + 1300 * 1.5 / 1700) = 18.2.
        tick(&mut ps, &PmInput::default(), &w, 62);
        assert!(ps.knockback_ms > 1200.0 && ps.knockback_ms < 1350.0);
        let second = jump_apex(&mut ps, &w);
        assert!(second > 16.0 && second < 20.0, "{second}");
        // Past the timer the flag drops and the full jump is back.
        tick(&mut ps, &PmInput::default(), &w, 250);
        assert!(!ps.jump_penalty && ps.knockback_ms == 0.0);
        let third = jump_apex(&mut ps, &w);
        assert!((third - first).abs() < 0.5, "{third}");

        let mut ps = PlayerState::spawn(Vec3::ZERO, 0.0);
        tick(&mut ps, &PmInput::default(), &w, 30);
        jump_apex(&mut ps, &w);
        tick(&mut ps, &PmInput::default(), &w, 62);
        let again = jump_apex(&mut ps, &w);
        assert!((again - first).abs() < 0.5, "1.1: {again}");
    }

    /// UO's sprint (docs/research/coduo-movement.md, 2-4): 1.6 times the
    /// run, a full bar spent in 3 s with the wind under half, nothing back
    /// for a second after, then the whole bar in 7 s and the wind gone.
    #[test]
    fn a_uo_sprint_runs_fast_and_tires() {
        let w = flat();
        let w = MoveWorld::bare(&w);
        let mut ps = PlayerState {
            rules: MoveRules::Uo,
            ..PlayerState::spawn(Vec3::ZERO, 0.0)
        };
        let mut t = 0;
        let mut run = |ps: &mut PlayerState, input: PmInput, ms: i32| {
            for _ in 0..ms / 8 {
                t += 8;
                pmove(
                    ps,
                    &PmInput {
                        server_time: t,
                        ..input
                    },
                    &w,
                    0.008,
                    &[],
                );
            }
        };
        let sprint = PmInput {
            forward: 1.0,
            sprint: true,
            ..Default::default()
        };
        run(&mut ps, PmInput::default(), 240);
        run(&mut ps, sprint, 1000);
        assert!(ps.sprinting);
        let speed = ps.velocity.truncate().length();
        assert!((speed - SPEED_RUN * SCALE_SPRINT).abs() < 2.0, "{speed}");
        run(&mut ps, sprint, 600);
        assert!(ps.winded && ps.fatigue < 0.5);
        run(&mut ps, sprint, 1500);
        assert!(!ps.sprinting && ps.fatigue == 0.0);
        let mut rested = ps;
        run(&mut rested, PmInput::default(), 900);
        assert_eq!(rested.fatigue, 0.0, "the regeneration delay");
        // Held on an empty bar: no sprint and no refill, though the delay
        // runs from the last sprinting cmd.
        run(&mut ps, sprint, 2000);
        assert_eq!(ps.fatigue, 0.0);
        run(&mut ps, PmInput::default(), 3500);
        assert!(ps.winded && (ps.fatigue - 0.5).abs() < 0.01);
        run(&mut ps, PmInput::default(), 3600);
        assert!(!ps.winded && ps.fatigue == 1.0);
    }

    /// Every patch drops the moves of a sighted prone body before the
    /// move (1.1 0x33f48 on `pm_flags` 0x21, 1.5 0x281ae on the effective
    /// stance); a crouched or unsighted one keeps them.
    #[test]
    fn a_sighted_prone_body_holds_still_on_every_patch() {
        let moving = PmInput {
            forward: 1.0,
            right: -1.0,
            ..Default::default()
        };
        for rules in [MoveRules::V1_1, MoveRules::V1_5, MoveRules::Uo] {
            let mut ps = PlayerState::spawn(Vec3::ZERO, 0.0);
            ps.rules = rules;
            ps.stance = Stance::Prone;
            ps.view_target = VIEW_PRONE;
            ps.ads_active = true;
            let held = hold_still_sighted(&ps, &moving);
            assert_eq!((held.forward, held.right), (0.0, 0.0), "{rules:?}");
            ps.ads_active = false;
            assert_eq!(hold_still_sighted(&ps, &moving).forward, 1.0, "{rules:?}");
            ps.ads_active = true;
            ps.stance = Stance::Crouch;
            ps.view_target = VIEW_CROUCH;
            assert_eq!(hold_still_sighted(&ps, &moving).forward, 1.0, "{rules:?}");
        }
    }

    /// The sight off the ground (cod15-movement.md 5): 1.5 and UO raise it
    /// on a body falling within a unit of the floor, `PM_GroundTraceMissed`'s
    /// `almostGroundPlane`; 1.1 and a body higher up wait for the landing.
    #[test]
    fn a_15_sight_starts_a_unit_over_the_floor() {
        let w = flat();
        let w = MoveWorld::bare(&w);
        let rifle = WeaponDef {
            aim_down_sight: true,
            ads_trans_in: 0.3,
            ads_trans_out: 0.3,
            clip_size: 5,
            ammo_index: 1,
            clip_index: 1,
            ..WeaponDef::default()
        };
        let weapons = [None, Some(rifle)];
        let sight = PmInput {
            ads: true,
            weapon: 1,
            ..Default::default()
        };
        let first_cmd = |rules: MoveRules, z: f32| {
            let mut ps = PlayerState {
                rules,
                ..PlayerState::spawn(Vec3::new(0.0, 0.0, z), 0.0)
            };
            weapon::give(&mut ps, 1, 1);
            ps.weapon = 1;
            ps.ammoclip[1] = 5;
            ps.on_ground = false;
            pmove(&mut ps, &sight, &w, 0.016, &weapons);
            ps.ads_active
        };
        for rules in [MoveRules::V1_5, MoveRules::Uo] {
            assert!(first_cmd(rules, 0.63), "{rules:?}");
            assert!(!first_cmd(rules, 1.5), "{rules:?}");
        }
        assert!(!first_cmd(MoveRules::V1_1, 0.63));
    }

    /// UO's bipod from prone (coduo-movement.md 7): the sight goes down in
    /// `adsTransInTime`, the body takes no move and its view no more than
    /// 55 degrees off `proneDirection`, and letting go breaks it down in
    /// `adsTransOutTime` at the walk.
    #[test]
    fn a_uo_bipod_deploys_prone_and_holds_the_view() {
        let w = flat();
        let w = MoveWorld::bare(&w);
        let lmg = WeaponDef {
            weapon_class: "lmg".into(),
            aim_down_sight: true,
            ads_trans_in: 1.8,
            ads_trans_out: 1.9,
            move_speed_scale: 1.1,
            clip_size: 100,
            ammo_index: 1,
            clip_index: 1,
            ..WeaponDef::default()
        };
        let weapons = [None, Some(lmg)];
        let mut ps = PlayerState {
            rules: MoveRules::Uo,
            ..PlayerState::spawn(Vec3::ZERO, 0.0)
        };
        weapon::give(&mut ps, 1, 1);
        ps.weapon = 1;
        ps.ammoclip[1] = 100;
        let tick = |ps: &mut PlayerState, input: &PmInput, n: usize| {
            for _ in 0..n {
                pmove(ps, input, &w, 0.008, &weapons);
            }
        };
        let prone = PmInput {
            prone: true,
            weapon: 1,
            ..Default::default()
        };
        tick(&mut ps, &prone, 125);
        assert_eq!(ps.stance, Stance::Prone);
        let sight = PmInput { ads: true, ..prone };
        tick(&mut ps, &sight, 1);
        assert!(ps.deployed());
        assert_eq!(ps.weaponstate, weapon::WEAPON_DEPLOY);
        assert_eq!(ps.weapon_time_ms, 1799);
        tick(&mut ps, &sight, 230);
        assert_eq!(ps.weaponstate, weapon::WEAPON_READY);
        assert_eq!(ps.weapon_pos_frac, 1.0);
        let at = ps.origin;
        tick(
            &mut ps,
            &PmInput {
                forward: 1.0,
                ..sight
            },
            60,
        );
        assert_eq!(ps.origin.truncate(), at.truncate());
        ps.yaw = 80f32.to_radians();
        tick(&mut ps, &sight, 1);
        assert!(
            (ps.yaw.to_degrees() - LMG_YAWCAP).abs() < 0.1,
            "{}",
            ps.yaw.to_degrees()
        );
        assert_eq!(ps.prone_direction, 0.0, "a deployed body does not swing");
        tick(&mut ps, &prone, 1);
        assert_eq!(ps.weaponstate, weapon::WEAPON_BREAKDOWN);
        assert_eq!(ps.weapon_time_ms, 1899);
    }

    /// 1.5 has no prone dive, and its eye target steps one stance a leg:
    /// stand to prone is a 200 ms leg to the crouch height, the target's
    /// step to prone on the next cmd, which arms the jump penalty, and a
    /// 400 ms leg on. The box and the effective stance follow the target.
    #[test]
    fn a_1_5_prone_press_steps_the_eye_and_never_dives() {
        let w = flat();
        let w = MoveWorld::bare(&w);
        let mut ps = v15(Vec3::ZERO);
        let run = PmInput {
            forward: 1.0,
            ..Default::default()
        };
        tick(&mut ps, &run, &w, 30);
        let prone = PmInput { prone: true, ..run };
        pmove(&mut ps, &prone, &w, 0.008, &[]);
        assert_eq!(ps.stance, Stance::Prone);
        assert!(!ps.prone_dive && ps.on_ground);
        assert_eq!(ps.view_target, VIEW_CROUCH);
        assert_eq!(ps.maxs().z, HEIGHT_CROUCH);
        let mut first_leg = 8;
        while ps.view_target == VIEW_CROUCH {
            pmove(&mut ps, &prone, &w, 0.008, &[]);
            first_leg += 8;
        }
        // The press starts the 200 ms leg at 0, the 26th cmd ends it and the
        // 27th steps the target.
        assert_eq!(first_leg, 27 * 8);
        assert_eq!(ps.view_target, VIEW_PRONE);
        assert_eq!(ps.maxs().z, HEIGHT_PRONE);
        // Less the cmd's own 8 ms: `PM_DropTimers` runs after the stance.
        assert!(ps.jump_penalty && ps.knockback_ms == JUMP_PENALTY_MS - 8.0);
        assert_eq!(ps.effective_stance(), Stance::Prone);
    }

    /// UO's `PM_CheckDuck` (0x28d66..0x28ef0) sizes the box off the flags
    /// where they stand higher than the eye's target: a crouch let go
    /// inside the 200 ms leg down stands the box at once, where 1.5's waits
    /// for the target. So a UO body that crouch-walks under a 60-unit
    /// ceiling after the release is stopped at its edge, and a 1.5 one
    /// walks under it and stands up into it, held there all solid
    /// (coduo-movement.md 18; mp_cassino held bots so until they typed
    /// `kill`).
    #[test]
    fn a_uo_rise_sizes_the_box_off_the_flags() {
        let w = test_world(&[(Vec3::new(0.0, -64.0, 60.0), Vec3::new(256.0, 64.0, 80.0))]);
        let w = MoveWorld::bare(&w);
        let crouch = PmInput {
            crouch: true,
            ..Default::default()
        };
        let walk = PmInput {
            forward: 1.0,
            ..Default::default()
        };
        let wedged = |rules: MoveRules| {
            let mut ps = PlayerState {
                rules,
                ..PlayerState::spawn(Vec3::new(-16.0, 0.0, 0.125), 0.0)
            };
            tick(&mut ps, &PmInput::default(), &w, 10);
            tick(&mut ps, &crouch, &w, 5);
            assert_eq!(ps.view_target, VIEW_CROUCH);
            assert_eq!(ps.maxs().z, HEIGHT_CROUCH);
            tick(&mut ps, &walk, &w, 1);
            assert_eq!(ps.stance, Stance::Stand);
            assert!(ps.view_lerp_ms.is_some(), "still inside the leg down");
            let box_at_release = ps.maxs().z;
            tick(&mut ps, &walk, &w, 125);
            let x = ps.origin.x;
            tick(&mut ps, &walk, &w, 25);
            (box_at_release, x, ps.origin.x, ps.maxs().z)
        };
        let (uo_box, uo_x, uo_end, uo_h) = wedged(MoveRules::Uo);
        assert_eq!(uo_box, HEIGHT_STAND);
        assert!(uo_x <= -15.0 && uo_end <= -15.0, "{uo_x} {uo_end}");
        assert_eq!(uo_h, HEIGHT_STAND);
        let (box15, x15, end15, h15) = wedged(MoveRules::V1_5);
        assert_eq!(box15, HEIGHT_CROUCH);
        assert!(
            x15 > -15.0 && x15 == end15,
            "held under the ceiling: {x15} {end15}"
        );
        assert_eq!(h15, HEIGHT_STAND);
    }

    /// `PM_Footsteps`: a lean is the walk under 1.5, in the rate and the
    /// ids, and audible; under 1.1 it is a slow run (0x24bf7, 0x3105c).
    #[test]
    fn a_1_5_lean_run_steps_like_a_walk() {
        let w = dirt_flat();
        let w = MoveWorld::bare(&w);
        let lean_run = PmInput {
            forward: 1.0,
            lean_right: true,
            ..Default::default()
        };
        let ids: Vec<Vec<i32>> = [MoveRules::V1_1, MoveRules::V1_5]
            .into_iter()
            .map(|rules| {
                let mut ps = PlayerState {
                    rules,
                    ..PlayerState::spawn(Vec3::ZERO, 0.0)
                };
                (0..500)
                    .flat_map(|_| pmove(&mut ps, &lean_run, &w, 0.008, &[]))
                    .map(|e| e.event)
                    .collect()
            })
            .collect();
        assert!(
            ids[0].iter().all(|&e| e == EV_FOOTSTEP_RUN_BASE + 6),
            "{ids:?}"
        );
        assert!(
            ids[1].iter().all(|&e| e == EV_FOOTSTEP_WALK_BASE + 6),
            "{ids:?}"
        );
        assert!(ids[1].len() > ids[0].len(), "{ids:?}");
    }

    /// The cycle ticks by the share of full speed the body has, truncated:
    /// a start from rest moves it nothing on the first cmd. Under 10 u/s a
    /// creep keeps the phase and a stop (under 1) zeroes it (0x324af).
    #[test]
    fn the_bob_cycle_runs_on_the_share_of_full_speed() {
        let w = dirt_flat();
        let w = MoveWorld::bare(&w);
        let mut ps = PlayerState::spawn(Vec3::ZERO, 0.0);
        let run = PmInput {
            forward: 1.0,
            ..Default::default()
        };
        pmove(&mut ps, &run, &w, 0.008, &[]);
        assert!(ps.velocity.length() > 10.0);
        assert_eq!(ps.bob_cycle, 0);
        tick(&mut ps, &run, &w, 40);
        let phase = ps.bob_cycle;
        assert_ne!(phase, 0);
        let still = PmInput::default();
        ps.velocity = Vec3::new(5.0, 0.0, 0.0);
        footsteps(&mut ps, &still, &w, 0.008, &mut Vec::new());
        assert_eq!(ps.bob_cycle, phase);
        ps.velocity = Vec3::new(0.5, 0.0, 0.0);
        footsteps(&mut ps, &still, &w, 0.008, &mut Vec::new());
        assert_eq!(ps.bob_cycle, 0);
    }

    /// A leaning body's bob ticks at the walk's rate on 1.5 and the run's on
    /// UO (`game.mp.uo` 0x297e4): the same strafe at the same speed advances
    /// UO's cycle by less (moveshot_ab.rs, the lean-strafe rounds).
    #[test]
    fn a_uo_lean_bobs_at_the_run_s_rate() {
        let w = dirt_flat();
        let w = MoveWorld::bare(&w);
        let strafe = PmInput {
            right: 1.0,
            ..Default::default()
        };
        let tick_of = |rules: MoveRules| {
            let mut ps = PlayerState::spawn(Vec3::ZERO, 0.0);
            ps.rules = rules;
            ps.on_ground = true;
            ps.lean = -0.5;
            ps.velocity = Vec3::new(0.0, -71.0, 0.0);
            footsteps(&mut ps, &strafe, &w, 0.050, &mut Vec::new());
            ps.bob_cycle
        };
        let (v15, uo) = (tick_of(MoveRules::V1_5), tick_of(MoveRules::Uo));
        assert!(uo > 0 && uo < v15, "1.5 {v15} UO {uo}");
    }

    fn prone_on(w: &MoveWorld, origin: Vec3, ground: bool, rules: MoveRules) -> Option<ProneBody> {
        check_prone(
            w,
            ProneQuery {
                origin,
                yaw_deg: 0.0,
                entry: false,
                ground,
                length: PRONE_CHECK_LENGTH,
                rules,
            },
        )
    }

    /// UO's `BG_CheckProneValid` lets the legs bend up 45 degrees past the
    /// hips (0x9a9e8), where 1.1 and 1.5 allow 50 (coduo-movement.md 17):
    /// the feet on a 30-unit step behind bend them about 48.
    #[test]
    fn a_uo_body_refuses_legs_bent_up_past_45_degrees() {
        let step = |height: f32| {
            test_world(&[
                (
                    Vec3::new(-200.0, -200.0, -16.0),
                    Vec3::new(200.0, 200.0, 0.0),
                ),
                (
                    Vec3::new(-200.0, -200.0, 0.0),
                    Vec3::new(-54.0, 200.0, height),
                ),
            ])
        };
        let o = Vec3::new(0.0, 0.0, 0.125);
        let high = step(30.0);
        let w = MoveWorld::bare(&high);
        for rules in [MoveRules::V1_1, MoveRules::V1_5] {
            let body = prone_on(&w, o, true, rules).expect("a fit");
            assert!((45.0..50.0).contains(&body.waist_pitch), "{body:?}");
        }
        assert!(prone_on(&w, o, true, MoveRules::Uo).is_none());
        let low = step(20.0);
        assert!(prone_on(&MoveWorld::bare(&low), o, true, MoveRules::Uo).is_some());
    }

    /// 1.5's `BG_CheckProneValid` refuses a body whose samples land on a
    /// plane steeper than 0.7 (0x1d842, 0x1dd86, 0x1def4); 1.1 lays it down.
    #[test]
    fn a_1_5_body_does_not_lie_on_a_steep_plane() {
        let w = crate::collision::ramp_test_world(50.0, -200.0, 200.0);
        let w = MoveWorld::bare(&w);
        let o = Vec3::new(0.0, 0.0, 200.0 * 50f32.to_radians().tan() + 1.0);
        assert!(prone_on(&w, o, true, MoveRules::V1_1).is_some());
        assert!(prone_on(&w, o, true, MoveRules::V1_5).is_none());
    }

    /// Off the ground 1.1 stops at the sweep and lays the body flat; 1.5
    /// samples the ground under it anyway (cod15-movement.md 4).
    #[test]
    fn a_1_5_airborne_body_still_samples_the_ground() {
        let w = crate::collision::ramp_test_world(20.0, -200.0, 200.0);
        let w = MoveWorld::bare(&w);
        let o = Vec3::new(0.0, 0.0, 200.0 * 20f32.to_radians().tan() + 1.0);
        assert_eq!(
            prone_on(&w, o, false, MoveRules::V1_1),
            Some(ProneBody::default())
        );
        let body = prone_on(&w, o, false, MoveRules::V1_5).expect("a fit");
        assert!(body.torso_pitch.abs() > 10.0, "{body:?}");
    }

    /// 1.5's prone view (0x26118): turned toward a wall the legs cannot
    /// sweep, the body refuses the swing, the refusal sets 0x8000, and the
    /// view is pulled back toward the body further than 1.1's cap leaves it.
    #[test]
    fn a_1_5_prone_turn_into_a_wall_pulls_the_view_back() {
        let w = test_world(&[(
            Vec3::new(-200.0, -60.0, 0.0),
            Vec3::new(200.0, -40.0, 100.0),
        )]);
        let w = MoveWorld::bare(&w);
        let mut off = Vec::new();
        for rules in [MoveRules::V1_1, MoveRules::V1_5] {
            let mut ps = PlayerState {
                rules,
                ..PlayerState::spawn(Vec3::new(0.0, 0.0, 0.125), 0.0)
            };
            let prone = PmInput {
                prone: true,
                ..Default::default()
            };
            tick(&mut ps, &prone, &w, 100);
            assert_eq!(ps.stance, Stance::Prone);
            let mut blocked = false;
            for i in 0..250 {
                ps.prev_yaw = ps.yaw;
                ps.yaw = (120.0 * i as f32 / 250.0).to_radians();
                pmove(&mut ps, &prone, &w, 0.008, &[]);
                blocked |= ps.prone_blocked;
            }
            off.push((
                ps.yaw.to_degrees() - ps.prone_direction,
                blocked,
                ps.view_yaw_shorts,
            ));
        }
        assert!(off[1].1, "{off:?}");
        assert!(off[1].0 < off[0].0 - 1.0, "{off:?}");
    }
}
