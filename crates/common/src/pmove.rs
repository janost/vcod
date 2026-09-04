//! Contains routines ported from the Quake III Arena GPL source, Copyright (C) 1999-2005 Id Software, Inc.,
//! and the RTCW-MP GPL source, Copyright (C) 1999-2010 id Software LLC, a ZeniMax Media company. See NOTICE.
//!
//! Q3/RTCW-derived player movement on top of `collision::box_trace`. Every
//! constant's provenance: docs/research/bsp-ibsp59-format.md, "Movement
//! constants and their provenance".

use crate::collision::CollisionWorld;
use crate::weapon::WeaponDef;
use glam::Vec3;

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
/// Ground-jump heights: vz = sqrt(2 * height * GRAVITY). Retail rodata
/// 0x70BE8/0x70BEC, applied in fn 0x316F4 @0x31CC0 (game.mp.i386.so).
pub const JUMP_HEIGHT_STAND: f32 = 34.0;
pub const JUMP_HEIGHT_LOW: f32 = 24.0;
// Accelerate/friction/stopspeed: retail CoD 1.1 rodata (game.mp.i386.so),
// loaded by PM_Friction @0x2e460 and the movers.
pub const PM_ACCELERATE: f32 = 9.0;
/// Stance accelerates: values selected at 0x2f4b0-0x2f4ca in the steep-slope
/// mover; their walk-path application is INFERRED from that selection.
pub const PM_DUCKED_ACCELERATE: f32 = 12.0;
pub const PM_PRONE_ACCELERATE: f32 = 19.0;
pub const PM_AIRACCELERATE: f32 = 1.0;
pub const PM_FRICTION: f32 = 5.5;
/// Friction control floor: drop uses max(speed, stopspeed) (@0x2e500).
pub const PM_STOPSPEED: f32 = 100.0;
pub const STEPSIZE: f32 = 18.0;
/// Step height while prone (PM_StepSlideMove @0x35045 tests pm_flags bit 0x1).
pub const STEPSIZE_PRONE: f32 = 10.0;
pub const OVERCLIP: f32 = 1.001;
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
pub const MAX_FRAME_MS: f32 = 66.0;
pub const LEAN_MAX: f32 = 28.0; // eye offset in units; roll is lean/2 degrees
pub const LEAN_TIME_TO_MS: f32 = 340.0;
pub const LEAN_TIME_FROM_MS: f32 = 350.0;

/// Viewheight lerp times: PM_GetViewHeightLerpTime @0x345B8 (200 ms for the
/// stand/crouch family) and the bg_duck2prone_time/bg_prone2duck_time
/// defaults (400, docs/research/cod11-server-handshake.md).
pub const VIEW_LERP_MS: f32 = 200.0;
pub const VIEW_LERP_PRONE_MS: f32 = 400.0;

/// Prone tunables, from the retail server: `bg_prone_yawcap` 85 and
/// `bg_prone_softyawedge` 1 are cvars, the 55 deg/s swing rate and the
/// 54-unit body clearance are rodata. The clearance is traced with a
/// +/-6 box straight behind the facing, which is the space a body needs to
/// lie down in (docs/research/cod11-mantle.md, "Prone").
pub const PRONE_YAWCAP: f32 = 85.0;
/// The body only starts turning once the view is this far off it.
pub const PRONE_SOFT_EDGE: f32 = PRONE_YAWCAP - 5.0;
pub const PRONE_SWING_DEG_PER_SEC: f32 = 55.0;
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
/// Re-grab lock after a ladder push-off: pm_ladderJumpTime = 300 int
/// (rodata 0x70830, compared as delta <= 299 @0x33822).
pub const LADDER_REGRAB_LOCK_MS: f32 = 300.0;
/// Re-jump gate on the ladder push-off: cmd.serverTime - ps.jumpTime > 499
/// (@0x2ebb3).
pub const LADDER_REJUMP_COOLDOWN_MS: f32 = 500.0;
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
/// Ladder climb steps stay quiet this long after a push-off
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
    pub ground_normal: Vec3,
    pub lean: f32, // -LEAN_MAX..LEAN_MAX
    /// World yaw of the prone body in degrees, retail's `ps.proneDirection`.
    /// Meaningless unless the stance is prone.
    pub prone_direction: f32,
    /// Degrees the view must move this frame to stay inside the prone cone.
    /// Retail applies it to `delta_angles`; the caller owns that, so pmove
    /// reports it rather than writing it.
    pub view_yaw_correction: f32,
    /// 0 dry, 1 feet, 2 waist, 3 eyes under (RTCW waterlevel).
    pub water_level: u32,
    /// Remaining control lock while flying out of water; 0 when free.
    pub waterjump_ms: f32,
    /// Touching a climbable surface (trace hit with SURF_LADDER) this frame.
    pub on_ladder: bool,
    /// Plane normal of that surface; persists while off the wall so the
    /// airborne probe can stick with the ladder we left.
    pub ladder_normal: Vec3,
    /// ms elapsed since the last ladder push-off; INFINITY when never. Retail
    /// stamps cmd.serverTime into ps.jumpTime on the ladder/steep jumps only
    /// and compares deltas (@0x2ebb3, @0x33822) - there is no ground-jump
    /// timer to port.
    pub since_jump_ms: f32,
    /// A held jump key blocks another ladder push-off until released
    /// (pm_flags bit 0x8: set @0x2ec34, cleared @0x34135 when upmove <= 9).
    pub jump_latched: bool,
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
    /// legs' heading is measured off the displacement it gives.
    move_start: Vec3,
    /// Fastest downward speed sampled while airborne; feeds the landing
    /// sound thresholds (PM_CrashLand's kinematic impact, approximated).
    air_speed_peak: f32,
    /// Eased eye height; trails `stance.view_height()` after a stance change
    /// (retail lerps the view while the bbox snaps).
    view_height_cur: f32,
    /// Lerp pace in units/s, fixed per transition when the stance flips.
    view_height_speed: f32,
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
    /// The previous cmd's view angles in ANGLE2SHORT units, pitch and yaw.
    /// Retail's `pm->oldcmd.angles`, which the spread's turn term subtracts
    /// this cmd's from.
    pub last_cmd_angles: [i32; 2],
    /// The previous cmd's sight bit, retail's `pm->oldcmd.buttons & 0x10`.
    /// Only the prone arm of the ADS flag reads it.
    pub last_cmd_ads: bool,
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
            ground_normal: Vec3::Z,
            lean: 0.0,
            prone_direction: 0.0,
            view_yaw_correction: 0.0,
            water_level: 0,
            waterjump_ms: 0.0,
            on_ladder: false,
            ladder_normal: Vec3::ZERO,
            since_jump_ms: f32::INFINITY,
            jump_latched: false,
            bob_cycle: 0,
            movement_dir: 0,
            move_start: origin,
            ground_surface_flags: 0,
            air_speed_peak: 0.0,
            view_height_cur: Stance::Stand.view_height(),
            view_height_speed: 0.0,
            ducked: false,
            // Retail leaves the target at 0 until the first stance change.
            view_lerp_target: 0.0,
            view_lerp_down: false,
            backwards_run: false,
            jumped: false,
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
            last_cmd_angles: [0; 2],
            last_cmd_ads: false,
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

/// `dt` in seconds, clamped to `MAX_FRAME_MS`. Returns the frame's movement
/// sound events in wire `EV_*` numbering.
pub fn pmove(
    ps: &mut PlayerState,
    input: &PmInput,
    world: &CollisionWorld,
    dt: f32,
    weapons: &[Option<WeaponDef>],
) -> Vec<PmEvent> {
    let dt = dt.min(MAX_FRAME_MS / 1000.0);
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
    let mut events = Vec::new();
    ps.jumped = false;
    let was_on_ground = ps.on_ground;
    // retail's `pml.previous_origin`, taken at the top of PmoveSingle
    ps.move_start = ps.origin;
    if ps.on_ground {
        ps.air_speed_peak = 0.0;
    }
    update_stance(ps, input, world, dt);
    update_prone_yaw(ps, world, dt);
    update_lean(ps, input, world, dt);
    set_water_level(ps, world);
    ground_trace(ps, world);
    // Retail updates the ADS flag once per `pm_type` arm, after the ground
    // trace and before the arm's move (`PM_UpdateAimDownSightFlag`, combat
    // doc 1.13), so it reads the ground state the move starts from.
    weapon::update_ads_flag(
        ps,
        input,
        weapons.get(ps.weapon as usize).and_then(Option::as_ref),
    );
    if ps.waterjump_ms > 0.0 {
        ps.waterjump_ms -= dt * 1000.0;
        if ps.waterjump_ms < 0.0 {
            ps.waterjump_ms = 0.0;
        }
    }
    // retail checks ladders right after the first ground trace and dispatches
    // them before waterjump/water
    let ladder = check_ladder_move(ps, input, world);
    if let Some((normal, ladderforward)) = ladder {
        ladder_move(ps, input, normal, ladderforward, world, dt, &mut events);
    } else if ps.waterjump_ms > 0.0 {
        water_jump_move(ps, world, dt);
    } else if ps.water_level > 1 {
        water_move(ps, input, world, dt);
    } else {
        // retail ground jump (fn 0x316F4 @0x31CC0): stance-dependent height,
        // horizontal velocity kept. Its bit-0x20 check (@0x31ccb) reads the
        // ADS-active flag PM_UpdateAimDownSightFlag maintains, which
        // `ps.ads_active` carries (combat doc, 1.13). What the check does
        // with it was not read, so the gate is still not ported. No cooldown
        // timer exists on this path
        // either (ps.jumpTime is never written here). The forwardmove gate
        // this used to carry was a misread: retail jumps standing still
        // (docs/research/cod11-mantle.md, "Jumps").
        if input.jump && ps.on_ground {
            let height = match ps.stance {
                Stance::Stand => JUMP_HEIGHT_STAND,
                _ => JUMP_HEIGHT_LOW,
            };
            ps.velocity.z = (2.0 * height * GRAVITY).sqrt();
            ps.on_ground = false;
            ps.jumped = true;
        }
        if ps.on_ground {
            friction(ps, ps.on_ladder, dt);
            walk_move(ps, input, world, dt);
        } else {
            air_move(ps, input, world, dt);
        }
    }
    ground_trace(ps, world);
    set_water_level(ps, world);

    // PM_Footsteps @0x322c8 runs once per move, after the final ground trace.
    footsteps(ps, input, world, dt, &mut events);
    if !was_on_ground && ps.on_ground {
        crash_land(ps, &mut events);
    }
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
    events
}

/// A dead player's frame: gravity and ground friction with no input, no
/// stance, lean or weapon step, and the eye easing to `VIEW_DEAD`. Q3's
/// `PM_DEAD` arm of `PmoveSingle` with the movement input zeroed; the eye
/// rate is the retail capture's (`DEAD_VIEW_LERP_SPEED`).
pub fn dead_move(ps: &mut PlayerState, world: &CollisionWorld, dt: f32) {
    let dt = dt.min(MAX_FRAME_MS / 1000.0);
    let idle = PmInput::default();
    ps.jumped = false;
    ps.move_start = ps.origin;
    // `PM_ClearAimDownSightFlag` (`game.mp.i386.so` 0x3abd4), which
    // `PmoveSingle` calls in the dead arm. The fraction is left where the
    // death froze it: the weapon step that would ramp it down does not run
    // for a dead player (combat doc, 1.12 and 1.13).
    ps.ads_active = false;
    ground_trace(ps, world);
    if ps.on_ground {
        friction(ps, false, dt);
        walk_move(ps, &idle, world, dt);
    } else {
        air_move(ps, &idle, world, dt);
    }
    ground_trace(ps, world);
    ps.view_height_speed = DEAD_VIEW_LERP_SPEED;
    let step = ps.view_height_speed * dt;
    let gap = VIEW_DEAD - ps.view_height_cur;
    ps.view_height_cur += gap.clamp(-step, step);
}

/// Retail footstep cadence (`PM_Footsteps` @0x322c8). The bob cycle ticks by
/// `msec * rate`; a step event fires on each 128-tick crossing while move
/// keys are held and the gait is audible. The ladder branch runs ahead of the
/// speed gates (@0x323a2 precedes the fcom @0x3249f).
fn footsteps(
    ps: &mut PlayerState,
    input: &PmInput,
    world: &CollisionWorld,
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
        // quiet for 299 ms after a push-off (@0x323b9)
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
    world: &CollisionWorld,
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

/// Landing sound from `PM_CrashLand`'s damage-free ladder (@0x30141): nothing
/// at or under 4, a walk-step to 8, a run-step to 12, a land event past that.
fn crash_land(ps: &mut PlayerState, events: &mut Vec<PmEvent>) {
    let impact = std::mem::take(&mut ps.air_speed_peak);
    if let Some(ev) = landing_event(impact, ps) {
        events.push(ev);
    }
}

fn landing_event(impact: f32, ps: &PlayerState) -> Option<PmEvent> {
    if ps.water_level >= 3 {
        return None;
    }
    let sf = ps.ground_surface_flags;
    if sf & SURF_NO_SOUND != 0 {
        return None;
    }
    let mat = crate::collision::sound_material(sf);
    if mat == 0 {
        return None;
    }
    let id = if impact >= 12.0 {
        EV_LANDING_BASE + mat
    } else if impact >= 8.0 {
        EV_FOOTSTEP_RUN_BASE + mat
    } else if impact > 4.0 {
        EV_FOOTSTEP_WALK_BASE + mat
    } else {
        return None;
    };
    Some(PmEvent { event: id, parm: 0 })
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
fn update_stance(ps: &mut PlayerState, input: &PmInput, world: &CollisionWorld, dt: f32) {
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
    if desired == Stance::Prone && before != Stance::Prone {
        if prone_fits(world, ps.origin, ps.yaw.to_degrees()) {
            ps.prone_direction = normalize180(ps.yaw.to_degrees());
        } else {
            desired = before;
        }
    }
    if desired.height() <= ps.stance.height() {
        ps.stance = desired;
    } else {
        let maxs = Vec3::new(HALF_WIDTH, HALF_WIDTH, desired.height());
        let t = world.box_trace(ps.origin, ps.origin, ps.mins(), maxs);
        if !t.startsolid {
            ps.stance = desired;
        }
    }

    // The bbox snaps; the eye eases. PM_GetViewHeightLerpTime (@0x345B8):
    // 200 ms stand/crouch family, the 400 ms bg_duck2prone_time/
    // bg_prone2duck_time defaults into and out of prone.
    if ps.stance != before {
        match ps.stance {
            Stance::Crouch => ps.ducked = true,
            Stance::Stand => ps.ducked = false,
            Stance::Prone => {}
        }
        ps.view_lerp_target = ps.stance.view_height();
        ps.view_lerp_down = ps.stance.view_height() < before.view_height();
        let ms = if ps.stance == Stance::Prone || before == Stance::Prone {
            VIEW_LERP_PRONE_MS
        } else {
            VIEW_LERP_MS
        };
        ps.view_height_speed = (ps.stance.view_height() - ps.view_height_cur).abs() / ms * 1000.0;
    }
    let step = ps.view_height_speed * dt;
    let gap = ps.stance.view_height() - ps.view_height_cur;
    ps.view_height_cur += gap.clamp(-step, step);
}

/// RTCW `bg_pmove.c` `PM_UpdateLean`. Differences: leans while moving (no
/// `!cmd->forwardmove` gate), and prone blocks leaning.
/// Whether a body may lie down at `origin` facing `yaw_deg`: retail sweeps a
/// 12-unit cube 54 units straight *backwards* from the facing, which is where
/// the body goes (`BG_CheckProneValid` 0x2d428, first trace at 0x2d57a).
/// Sloped-ground pitch, the rest of that function, is an animation output and
/// is not modelled.
pub fn prone_fits(world: &CollisionWorld, origin: Vec3, yaw_deg: f32) -> bool {
    let back = (yaw_deg + 180.0).to_radians();
    let dir = Vec3::new(back.cos(), back.sin(), 0.0);
    // Retail traces from the player's origin, which sits at the feet.
    let start = origin + Vec3::Z * VIEW_PRONE;
    let end = start + dir * PRONE_BODY_LENGTH;
    let half = Vec3::splat(PRONE_BODY_HALF_BOX);
    let t = world.box_trace(start, end, -half, half);
    !t.startsolid && t.fraction >= 1.0
}

/// Degrees folded to -180..180, the `AngleNormalize180` the prone code runs
/// its yaw difference through.
fn normalize180(deg: f32) -> f32 {
    (deg + 180.0).rem_euclid(360.0) - 180.0
}

/// The prone body swinging to follow the view, and the view capped to the cone
/// around the body. Retail runs both in `PM_UpdateViewAngles` (0x32d7c); the
/// cap is enforced by pushing `delta_angles`, so the correction is reported
/// here and the caller applies it to whatever owns the view
/// (docs/research/cod11-mantle.md, "Prone").
fn update_prone_yaw(ps: &mut PlayerState, world: &CollisionWorld, dt: f32) {
    ps.view_yaw_correction = 0.0;
    if ps.stance != Stance::Prone {
        return;
    }
    let view = ps.yaw.to_degrees();
    let delta = normalize180(ps.prone_direction - view);
    // The body only starts to turn past the soft edge, and only into a
    // direction it still fits in.
    if delta.abs() > PRONE_SOFT_EDGE {
        let step = PRONE_SWING_DEG_PER_SEC * dt;
        let candidate = if step >= delta.abs() {
            view
        } else if delta > 0.0 {
            ps.prone_direction - step
        } else {
            ps.prone_direction + step
        };
        if prone_fits(world, ps.origin, candidate) {
            ps.prone_direction = normalize180(candidate);
        }
    }
    let delta = normalize180(ps.prone_direction - view);
    if delta.abs() > PRONE_YAWCAP {
        let excess = delta - PRONE_YAWCAP.copysign(delta);
        ps.view_yaw_correction = excess;
        ps.yaw = (view + excess).to_radians();
    }
}

fn update_lean(ps: &mut PlayerState, input: &PmInput, world: &CollisionWorld, dt: f32) {
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
        return;
    }
    ps.lean = (ps.lean + dir * (msec / LEAN_TIME_TO_MS) * LEAN_MAX).clamp(-LEAN_MAX, LEAN_MAX);

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
    );
    ps.lean *= t.fraction;
}

/// Q3 `bg_pmove.c` `PM_GroundTrace`.
fn ground_trace(ps: &mut PlayerState, world: &CollisionWorld) {
    let t = world.box_trace(ps.origin, ps.origin - Vec3::Z * 0.25, ps.mins(), ps.maxs());
    // A plain `velocity.z <= 0.0` test fails: OVERCLIP leaves a hair of
    // positive z after a floor clip and the player would stay airborne.
    let thrown_off = ps.velocity.z > 0.0 && ps.velocity.dot(t.normal) > 10.0;
    if t.fraction < 1.0 && t.normal.z >= MIN_WALK_NORMAL && !thrown_off {
        ps.on_ground = true;
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
fn set_water_level(ps: &mut PlayerState, world: &CollisionWorld) {
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
    if ps.on_ground && ps.water_level <= 1 {
        // Retail floors drop at a flat 100 (@0x2e500), which stalls prone
        // under the Q3 accelerate shape (4.4 loss vs 4.33 gain per frame);
        // stance-scaling the floor is the labeled deviation that keeps every
        // stance moving.
        let control = speed.max(PM_STOPSPEED * ps.stance.speed_scale());
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
fn wish(ps: &PlayerState, input: &PmInput) -> (Vec3, f32) {
    let fwd = Vec3::new(ps.yaw.cos(), ps.yaw.sin(), 0.0);
    let right = Vec3::new(ps.yaw.sin(), -ps.yaw.cos(), 0.0);
    let dir = (fwd * input.forward + right * input.right).normalize_or_zero();
    let scale = match ps.stance {
        Stance::Stand if input.walk_slow => SCALE_WALK,
        stance => stance.speed_scale(),
    };
    (dir, SPEED_RUN * scale)
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
    // this branch while the view is locked to another entity (eFlags
    // 0xc000, the mounted-gun case); nothing here mounts anything.
    if ps.stance == Stance::Prone {
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
fn walk_move(ps: &mut PlayerState, input: &PmInput, world: &CollisionWorld, dt: f32) {
    // eye-deep and looking up an upward slope: swim instead of trudging
    if ps.water_level > 2 && forward3(ps).dot(ps.ground_normal) > 0.0 {
        water_move(ps, input, world, dt);
        return;
    }
    let (dir, wishspeed) = wish(ps, input);
    // accelerate per stance: values selected at 0x2f4b0-0x2f4ca in the
    // steep-slope mover; walk-path application INFERRED
    let accel = match ps.stance {
        Stance::Stand => PM_ACCELERATE,
        Stance::Crouch => PM_DUCKED_ACCELERATE,
        Stance::Prone => PM_PRONE_ACCELERATE,
    };
    // along the slope, so it costs no speed
    let dir = clip_velocity(dir, ps.ground_normal).normalize_or_zero();
    accelerate(ps, dir, wishspeed, accel, dt);
    ps.velocity = clip_velocity(ps.velocity, ps.ground_normal);
    // Standing still skips the move but not the legs: retail jumps straight
    // to PM_SetMovementDir (@0x2f6db), which is what keeps a prone player's
    // legs following its body while it turns on the spot.
    if ps.velocity.x != 0.0 || ps.velocity.y != 0.0 {
        step_slide_move(ps, world, dt, false);
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
fn water_move(ps: &mut PlayerState, input: &PmInput, world: &CollisionWorld, dt: f32) {
    if try_start_water_jump(ps, world) {
        water_jump_move(ps, world, dt);
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
    slide_move(ps, world, dt, false);
}

/// RTCW `bg_pmove.c` `PM_CheckWaterJump`: chest-deep against a low lip, the
/// probe 4 units up must hit solid and 20 units up must be clear.
fn try_start_water_jump(ps: &mut PlayerState, world: &CollisionWorld) -> bool {
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
fn water_jump_move(ps: &mut PlayerState, world: &CollisionWorld, dt: f32) {
    step_slide_move(ps, world, dt, true);
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
    world: &CollisionWorld,
) -> Option<(Vec3, bool)> {
    // retail's pm_time gate covers exactly this lock in vcod
    if ps.waterjump_ms > 0.0 {
        ps.on_ladder = false;
        return None;
    }
    // skip detection within pm_ladderJumpTime of a push-off (@0x33822): this
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

/// Retail `PM_Jump` body (@0x2eb98), reached from a ladder only. vz =
/// sqrt(GRAVITY * 78) scaled x0.75 leaving the wall (@0x708c8/0x708d0);
/// horizontal reset to 128 along the horizontal forward reflected off the
/// ladder plane while facing it (@0x2eca2-0x2ed36, coefficient -2.0
/// @0x708d4), else along the plain forward. The weapon-state gates at
/// @0x2ebcc-0x2ec03 have no vcod counterpart; events/anim are out of scope.
fn ladder_push_off(ps: &mut PlayerState, input: &PmInput, normal: Vec3) -> bool {
    if !input.jump
        || ps.stance != Stance::Stand
        || ps.jump_latched
        || ps.since_jump_ms <= LADDER_REJUMP_COOLDOWN_MS - 1.0
    {
        return false;
    }
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
    ps.velocity.z = (GRAVITY * 78.0).sqrt() * 0.75;
    ps.on_ladder = false;
    ps.jump_latched = true;
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
    world: &CollisionWorld,
    dt: f32,
    events: &mut Vec<PmEvent>,
) {
    // retail tries the push-off first; a jump runs the normal mover this
    // frame and stamps jumpTime (@0x3394c-0x33964)
    if ladder_push_off(ps, input, normal) {
        // PM_Jump @0x2eda8-0x2edb9: 70 + material off the last ground trace;
        // material 0 or the silent surfaceparm emits nothing
        let sf = ps.ground_surface_flags;
        let mat = crate::collision::sound_material(sf);
        if mat != 0 && sf & SURF_NO_SOUND == 0 {
            events.push(PmEvent {
                event: EV_JUMP_BASE + mat,
                parm: 0,
            });
        }
        air_move(ps, input, world, dt);
        ps.since_jump_ms = 0.0;
        return;
    }

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
    step_slide_move(ps, world, dt, false);
    // PM_LadderMove @0x33d71: the legs face into the wall, and the cap here
    // is 75 degrees, not the 90 the ground path uses.
    ps.movement_dir = clamp_movement_dir(
        angle_delta(yaw_of(normal) + 180.0, ps.yaw.to_degrees()) as i32,
        LADDER_MOVEMENT_DIR_CAP,
    );
}

/// Q3 `bg_pmove.c` `PM_AirMove`.
fn air_move(ps: &mut PlayerState, input: &PmInput, world: &CollisionWorld, dt: f32) {
    ps.air_speed_peak = ps.air_speed_peak.max(-ps.velocity.z);
    let (dir, wishspeed) = wish(ps, input);
    accelerate(ps, dir, wishspeed, PM_AIRACCELERATE, dt);
    step_slide_move(ps, world, dt, true);
    set_movement_dir(ps, input, dt);
}

/// Q3 `bg_slidemove.c` `PM_SlideMove`. Returns true if anything was hit.
fn slide_move(ps: &mut PlayerState, world: &CollisionWorld, dt: f32, gravity: bool) -> bool {
    const NUM_BUMPS: usize = 4;
    let (mins, maxs) = (ps.mins(), ps.maxs());

    // average of start and end velocity, matching the analytic parabola
    let mut end_velocity = ps.velocity;
    if gravity {
        end_velocity.z -= GRAVITY * dt;
        ps.velocity.z = (ps.velocity.z + end_velocity.z) * 0.5;
        if ps.on_ground {
            ps.velocity = clip_velocity(ps.velocity, ps.ground_normal);
        }
    }

    let mut planes: Vec<Vec3> = Vec::with_capacity(MAX_CLIP_PLANES);
    // never turn against the ground plane or back against the original move
    if ps.on_ground {
        planes.push(ps.ground_normal);
    }
    planes.push(ps.velocity.normalize_or_zero());

    let mut time_left = dt;
    let mut bumps = 0;
    for _ in 0..NUM_BUMPS {
        let end = ps.origin + ps.velocity * time_left;
        let t = world.box_trace(ps.origin, end, mins, maxs);
        if t.allsolid {
            // trapped in solid: keep the horizontal control, kill the fall
            ps.velocity.z = 0.0;
            return true;
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
            return true;
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
                    return true;
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
    bumps != 0
}

/// Q3 `bg_slidemove.c` `PM_StepSlideMove`. Step height 18, or 10 while prone
/// (retail picks the height off pm_flags bit 0x1 @0x35045, not ladder state).
fn step_slide_move(ps: &mut PlayerState, world: &CollisionWorld, dt: f32, gravity: bool) {
    let step_size = if ps.stance == Stance::Prone {
        STEPSIZE_PRONE
    } else {
        STEPSIZE
    };
    let start_o = ps.origin;
    let start_v = ps.velocity;
    if !slide_move(ps, world, dt, gravity) {
        return;
    }
    let (mins, maxs) = (ps.mins(), ps.maxs());
    let (down_o, down_v) = (ps.origin, ps.velocity);

    // `allsolid`, not `startsolid`: a bbox merely touching the floor is
    // startsolid, and stepping up is how that resolves.
    let up = world.box_trace(start_o, start_o + Vec3::Z * step_size, mins, maxs);
    let step = up.endpos.z - start_o.z;
    if up.allsolid || step <= 0.0 {
        return;
    }

    ps.origin = up.endpos;
    ps.velocity = start_v;
    slide_move(ps, world, dt, gravity);

    let down = world.box_trace(ps.origin, ps.origin - Vec3::Z * step, mins, maxs);
    if !down.allsolid {
        ps.origin = down.endpos;
    }
    let stepped = (ps.origin.truncate() - start_o.truncate()).length_squared();
    let flat = (down_o.truncate() - start_o.truncate()).length_squared();
    if down.normal.z < MIN_WALK_NORMAL || flat > stepped {
        // into air, onto a too-steep slope, or the flat slide got further
        ps.origin = down_o;
        ps.velocity = down_v;
        return;
    }
    if down.fraction < 1.0 {
        ps.velocity = clip_velocity(ps.velocity, down.normal);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collision::test_world;

    fn flat() -> CollisionWorld {
        test_world(&[])
    }

    /// A death takes the sight down: `PmoveSingle`'s dead arm calls
    /// `PM_ClearAimDownSightFlag` and nothing else about the weapon runs, so
    /// the fraction stays where the death froze it
    /// (docs/research/cod11-combat.md, 1.13).
    #[test]
    fn a_death_clears_the_ads_flag() {
        let w = flat();
        let mut ps = PlayerState::spawn(Vec3::ZERO, 0.0);
        ps.ads_active = true;
        ps.weapon_pos_frac = 1.0;
        dead_move(&mut ps, &w, 0.05);
        assert!(!ps.ads_active);
        assert_eq!(ps.weapon_pos_frac, 1.0);
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

    fn tick(ps: &mut PlayerState, input: &PmInput, w: &CollisionWorld, n: usize) {
        for _ in 0..n {
            pmove(ps, input, w, 1.0 / 125.0, &[]);
        }
    }

    /// Stance changes ease the eye instead of snapping: 200 ms for the
    /// stand/crouch family, 400 ms into/out of prone
    /// (PM_GetViewHeightLerpTime @0x345B8; bg_duck2prone_time/
    /// bg_prone2duck_time default 400, docs/research/cod11-server-handshake.md).
    #[test]
    fn viewheight_lerps_on_stance_change() {
        let w = flat();
        let mut ps = PlayerState::spawn(Vec3::ZERO, 0.0);
        tick(&mut ps, &PmInput::default(), &w, 50); // settle on the ground
        assert_eq!(ps.view_height(), VIEW_STAND);

        let crouch = PmInput {
            crouch: true,
            ..PmInput::default()
        };
        tick(&mut ps, &crouch, &w, 12); // ~100 ms: partway down
        let mid = ps.view_height();
        assert!(mid < VIEW_STAND - 2.0 && mid > VIEW_CROUCH + 2.0, "{mid}");
        tick(&mut ps, &crouch, &w, 14); // ~200 ms total: settled
        assert_eq!(ps.view_height(), VIEW_CROUCH);

        let prone = PmInput {
            prone: true,
            ..PmInput::default()
        };
        tick(&mut ps, &prone, &w, 25); // ~200 ms: prone runs at the 400 ms pace
        let mid = ps.view_height();
        assert!(mid > VIEW_PRONE + 2.0 && mid < VIEW_CROUCH, "{mid}");
        tick(&mut ps, &prone, &w, 26); // ~400 ms total
        assert_eq!(ps.view_height(), VIEW_PRONE);
    }

    /// `movementDir` is the legs' heading off the view, which the player
    /// entity carries as `angles2[1]`. Straight ahead is 0, a pure strafe is
    /// the 90-degree cap, and a backpedal folds by 180.
    #[test]
    fn movement_dir_is_the_legs_yaw_off_the_view() {
        let w = flat();
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

    #[test]
    fn landing_sound_bands_follow_impact() {
        let mut ps = PlayerState::spawn(Vec3::ZERO, 0.0);
        ps.ground_surface_flags = 6 << 20;
        let band = |impact: f32| landing_event(impact, &ps).map(|e| e.event);
        assert_eq!(band(4.0), None);
        assert_eq!(band(4.5), Some(EV_FOOTSTEP_WALK_BASE + 6));
        assert_eq!(band(11.5), Some(EV_FOOTSTEP_RUN_BASE + 6));
        assert_eq!(band(12.0), Some(EV_LANDING_BASE + 6));
    }

    /// Water level 1-2 replaces the ground material with the fixed water ids
    /// and ignores the enable gate (PM_FootstepEvent @0x321b0).
    #[test]
    fn water_steps_use_the_fixed_water_ids() {
        let w = shallow_water();
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
            footsteps(&mut ps, &input, &dirt_flat(), dt, &mut events);
        }
        assert!(!events.is_empty(), "climbing at vz=80 must step");
        assert!(events
            .iter()
            .all(|e| e.event == EV_FOOTSTEP_RUN_BASE + DEFAULT_MATERIAL));

        // same climb against a wooden wall: the probe reports wood
        let wall = crate::collision::synthetic_world(
            &[(
                "textures/test/wood",
                crate::collision::CONTENTS_SOLID,
                21 << 20,
            )],
            &[(0, [16.0, -1024.0, -16.0], [32.0, 1024.0, 512.0])],
        );
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

    /// PM_Jump @0x2eda8: the push-off event carries the last ground trace's
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
            footsteps(&mut ps, &input, &dirt_flat(), 0.05, &mut events);
        }
        assert!(events.is_empty(), "quiet for 299 ms, got {events:?}");
        for _ in 0..20 {
            ps.since_jump_ms += 50.0;
            footsteps(&mut ps, &input, &dirt_flat(), 0.05, &mut events);
        }
        assert!(!events.is_empty(), "climbing must step once quiet");
    }

    fn ladder_push_off_ran(ps: &PlayerState) -> bool {
        ps.jump_latched && ps.since_jump_ms == 0.0
    }

    #[test]
    fn accelerates_to_run_speed_cap() {
        let w = flat();
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
    fn ground_jump_apex_matches_stance_height() {
        let w = flat();
        let run = PmInput {
            forward: 1.0,
            ..Default::default()
        };
        // retail: vz = sqrt(2 * height * g), height 34 standing (0x70be8)
        let mut ps = PlayerState::spawn(Vec3::ZERO, 0.0);
        tick(&mut ps, &PmInput::default(), &w, 50); // settle
        let mut apex = 0.0f32;
        let launch = PmInput {
            forward: 1.0,
            jump: true,
            ..Default::default()
        };
        pmove(&mut ps, &launch, &w, 1.0 / 125.0, &[]);
        for _ in 0..200 {
            pmove(&mut ps, &run, &w, 1.0 / 125.0, &[]);
            apex = apex.max(ps.origin.z);
        }
        assert!(
            (apex - JUMP_HEIGHT_STAND).abs() < 2.0,
            "standing apex {apex}, expected ~{}",
            JUMP_HEIGHT_STAND
        );
        assert!(ps.on_ground);

        // crouched jumps too, at the lower 24-unit height (0x70bec)
        let launch = PmInput {
            forward: 1.0,
            jump: true,
            crouch: true,
            ..Default::default()
        };
        let mut ps = PlayerState::spawn(Vec3::ZERO, 0.0);
        ps.stance = Stance::Crouch;
        tick(&mut ps, &launch, &w, 50); // settle crouched
        let mut apex = 0.0f32;
        pmove(&mut ps, &launch, &w, 1.0 / 125.0, &[]);
        for _ in 0..200 {
            pmove(&mut ps, &launch, &w, 1.0 / 125.0, &[]);
            apex = apex.max(ps.origin.z);
        }
        assert!(
            (apex - JUMP_HEIGHT_LOW).abs() < 2.0,
            "crouch apex {apex}, expected ~{}",
            JUMP_HEIGHT_LOW
        );
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
        // 150 degrees off a body at -5.5 is past the 85 cap, so the view is
        // pushed back to exactly the cap.
        let delta = ps.prone_direction - 150.0;
        assert!(
            (ps.view_yaw_correction - (delta + PRONE_YAWCAP)).abs() < 0.01,
            "correction {}",
            ps.view_yaw_correction
        );
        assert!(
            ((ps.yaw.to_degrees() - ps.prone_direction).abs() - PRONE_YAWCAP).abs() < 0.01,
            "the view ends exactly on the cap"
        );
    }

    #[test]
    fn a_standing_jump_needs_no_forward_input() {
        // The forwardmove gate once read off fn 0x316F4 @0x31CC0 is not there:
        // a retail server jumps a probe holding upmove alone
        // (docs/research/cod11-mantle.md, "Jumps").
        let w = flat();
        let mut ps = PlayerState::spawn(Vec3::ZERO, 0.0);
        tick(&mut ps, &PmInput::default(), &w, 50); // settle
        let mut apex = ps.origin.z;
        for _ in 0..200 {
            pmove(
                &mut ps,
                &PmInput {
                    jump: true,
                    ..Default::default()
                },
                &w,
                1.0 / 125.0,
                &[],
            );
            apex = apex.max(ps.origin.z);
        }
        assert!(
            (apex - JUMP_HEIGHT_STAND).abs() < 2.0,
            "standing apex {apex}, expected ~{JUMP_HEIGHT_STAND}"
        );
    }

    #[test]
    fn ground_jumps_chain_while_holding_forward_and_jump() {
        // no cooldown to port: retail's ps.jumpTime is written only by the
        // ladder push-off (@0x33964) and steep-slope jump (@0x2f279), never
        // by the ground jump; its bit-0x20 gate is ADS idle state, not a timer
        let w = flat();
        let hop = PmInput {
            forward: 1.0,
            jump: true,
            ..Default::default()
        };
        let mut ps = PlayerState::spawn(Vec3::ZERO, 0.0);
        tick(&mut ps, &PmInput::default(), &w, 50); // settle
        let mut launches = 0;
        let mut prev_vz = ps.velocity.z;
        for _ in 0..300 {
            pmove(&mut ps, &hop, &w, 1.0 / 125.0, &[]);
            if prev_vz < 100.0 && ps.velocity.z > 200.0 {
                launches += 1;
            }
            prev_vz = ps.velocity.z;
        }
        assert!(launches >= 3, "expected chained jumps, got {launches}");
    }

    #[test]
    fn prone_jumps_at_the_low_height() {
        // bit 0x20 (the ground jump's extra gate @0x31ccb) is set only while
        // the ADS button is held (@0x37247); without ADS modeled, prone jumps
        // like crouch at the 24-unit height (@0x70bec)
        let w = flat();
        let hop = PmInput {
            forward: 1.0,
            jump: true,
            prone: true,
            ..Default::default()
        };
        let mut ps = PlayerState::spawn(Vec3::ZERO, 0.0);
        tick(&mut ps, &hop, &w, 50); // settle prone
        let mut apex = 0.0f32;
        for _ in 0..60 {
            pmove(&mut ps, &hop, &w, 1.0 / 125.0, &[]);
            apex = apex.max(ps.origin.z);
        }
        assert!(
            (apex - JUMP_HEIGHT_LOW).abs() < 2.5,
            "prone apex {apex}, expected ~{JUMP_HEIGHT_LOW}"
        );
    }

    #[test]
    fn prone_steps_lower_than_standing() {
        // retail picks the 10-unit step height off pm_flags bit 0x1
        // (PM_StepSlideMove @0x35045), 18 otherwise (@0x35034)
        let w = test_world(&[(Vec3::new(50.0, -200.0, 0.0), Vec3::new(1024.0, 200.0, 14.0))]);
        let mut stand = PlayerState::spawn(Vec3::new(30.0, 0.0, 0.0), 0.0);
        stand.velocity = Vec3::new(200.0, 0.0, 0.0);
        for _ in 0..20 {
            stand.velocity.x = 200.0;
            step_slide_move(&mut stand, &w, 1.0 / 125.0, false);
        }
        assert!(
            (stand.origin.z - 14.0).abs() < 0.5,
            "standing should step 14, at {}",
            stand.origin
        );

        let mut prone = PlayerState::spawn(Vec3::new(30.0, 0.0, 0.0), 0.0);
        prone.stance = Stance::Prone;
        for _ in 0..20 {
            prone.velocity.x = 200.0;
            step_slide_move(&mut prone, &w, 1.0 / 125.0, false);
        }
        assert!(
            prone.origin.z < 1.0 && prone.origin.x < 36.0,
            "prone must not step 14, at {}",
            prone.origin
        );
    }

    #[test]
    fn ankle_deep_water_barely_slows_running() {
        // retail has no wading wish clamp: nothing references
        // pm_waterSwimScale/pm_waterWadeScale and the walk mover (0x2F03C)
        // never reads water level - only the water-friction term slows you
        let w = pool_world();
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
        assert!(
            wet_speed > 160.0 && wet_speed > dry_speed - 12.0,
            "ankle-deep run {wet_speed} vs dry {dry_speed}"
        );
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
        let mut ps = PlayerState::spawn(Vec3::ZERO, 0.0);
        ps.stance = Stance::Crouch;
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
        assert!(
            diag < solo && (0.7..0.95).contains(&ratio),
            "W+D must climb slower than W alone, {solo} -> {diag} (x{ratio})"
        );
    }

    #[test]
    fn releasing_input_mid_climb_hangs_without_sliding() {
        let w = ladder_world();
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
    fn ladder_push_off_gate_matrix() {
        // PM_Jump gates @0x2ebb3-0x2ec13: 500 ms since the last push-off, not
        // crouched/prone, jump key released since the previous one
        let n = Vec3::new(-1.0, 0.0, 0.0);
        let mk = |stance: Stance| {
            let mut ps = PlayerState::spawn(Vec3::ZERO, 0.0);
            ps.stance = stance;
            ps.on_ladder = true;
            ps.since_jump_ms = 10000.0;
            ps
        };
        let jump = PmInput {
            jump: true,
            ..Default::default()
        };

        let mut ps = mk(Stance::Stand);
        assert!(ladder_push_off(&mut ps, &jump, n));
        // facing straight into the wall (yaw 0, normal -1): reflection sends
        // the push straight back; vz = sqrt(78 * g) * 0.75 = 187.35
        assert!((ps.velocity.z - 187.35).abs() < 0.1, "vz {}", ps.velocity.z);
        assert!((ps.velocity.x + 128.0).abs() < 0.1, "vx {}", ps.velocity.x);
        assert!(ps.velocity.y.abs() < 0.1);
        assert!(ps.jump_latched);

        let mut ps = mk(Stance::Stand);
        ps.since_jump_ms = LADDER_REJUMP_COOLDOWN_MS - 1.0;
        assert!(!ladder_push_off(&mut ps, &jump, n), "inside the cooldown");

        // boundary: retail allows at delta > 499, so exactly 500 passes
        let mut ps = mk(Stance::Stand);
        ps.since_jump_ms = LADDER_REJUMP_COOLDOWN_MS;
        assert!(ladder_push_off(&mut ps, &jump, n), "500 ms allows");

        let mut ps = mk(Stance::Stand);
        ps.jump_latched = true;
        assert!(
            !ladder_push_off(&mut ps, &jump, n),
            "held key must release first"
        );

        let mut ps = mk(Stance::Crouch);
        assert!(!ladder_push_off(&mut ps, &jump, n), "crouch refuses");
        let mut ps = mk(Stance::Prone);
        assert!(!ladder_push_off(&mut ps, &jump, n), "prone refuses");

        let mut ps = mk(Stance::Stand);
        assert!(
            !ladder_push_off(&mut ps, &PmInput::default(), n),
            "no jump key"
        );
    }

    #[test]
    fn ladder_push_off_leaves_the_wall_with_reflected_forward() {
        let w = ladder_world();
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
        assert!(ladder_push_off(
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
        assert!(ladder_push_off(
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
                        if sign > 0.0 {
                            180.0f32
                        } else {
                            0.0
                        }
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
}
