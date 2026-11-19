//! Contains routines ported from the Quake III Arena GPL source, Copyright (C) 1999-2005 Id Software, Inc.,
//! and the RTCW-MP GPL source, Copyright (C) 1999-2010 id Software LLC, a ZeniMax Media company. See NOTICE.
//!
//! Q3/RTCW-derived player movement on top of `movetrace::MoveWorld`. Every
//! constant's provenance: docs/research/bsp-ibsp59-format.md, "Movement
//! constants and their provenance".

use glam::Vec3;

pub mod aim;
pub mod cmd;
pub mod movers;
pub mod predict;
pub mod weapon;

mod angles;
mod footsteps;
mod frame;
mod ground;
mod jump;
mod ladder;
mod land;
mod lean;
mod prone;
mod slide;
mod spectator;
mod stance;
mod state;
mod uo;
mod walk;

pub(crate) use angles::{angle_delta, normalize360_accurate, view_deg};
pub use angles::{angle_normalize180, angle_shorts};
pub use frame::{dead_linked_move, dead_move, pmove};
pub use ground::{Takeoff, WATER_FRICTION};
pub use jump::{JUMP_COOLDOWN_MS, JUMP_HEIGHT};
pub use ladder::{
    LADDER_ACCEL_FLOOR, LADDER_ACCELERATE, LADDER_CLIMB_SCALE, LADDER_PROBE_FLOOR,
    LADDER_PROBE_SHRINK, LADDER_PUSHOFF_SPEED, LADDER_REGRAB_LOCK_MS, LADDER_STRAFE_SCALE,
    LADDER_TRACE_DIST_AIR, LADDER_TRACE_DIST_WALK, LADDER_UPSCALE_BIAS, LADDER_UPSCALE_GAIN,
    LADDER_YAWCAP, PM_LADDER_FRICTION,
};
pub use land::{EV_LANDING_PAIN_LAST, LAND_ANIM_SPEED};
pub use lean::{LEAN_TIME_FROM_MS, LEAN_TIME_TO_MS};
pub use prone::{
    LMG_YAWCAP, PRONE_BODY_LENGTH, PRONE_CHECK_LENGTH, PRONE_PITCH_DEG_PER_SEC, PRONE_PITCHCAP,
    PRONE_SOFT_EDGE, PRONE_SWING_DEG_PER_SEC, PRONE_YAWCAP, ProneBody, ProneCone, ProneQuery,
    check_prone, pitch_for_yaw_on_normal, prone_view_angle,
};
pub use spectator::spectator_move;
use stance::view_lerp_length;
pub use state::{
    BgCvars, BobCvars, FallHeights, MoveRules, PlayerState, PmEvent, PmInput, Stance, VehicleSeat,
    ViewParams,
};
pub use uo::{CONTENTS_HMG, CONTENTS_HMG_CROUCH, SCALE_SPRINT};

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

/// `EV_LANDING_PAIN_*`, one per surface material up to 138; the server's
/// `ClientEvents` turns each into fall damage.
pub use crate::net::event_ids::EV_LANDING_PAIN_BASE;

#[cfg(test)]
mod tests;
