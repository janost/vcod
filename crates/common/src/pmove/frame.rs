//! Contains routines ported from the Quake III Arena GPL source, Copyright (C) 1999-2005 Id Software, Inc.,
//! and the RTCW-MP GPL source, Copyright (C) 1999-2010 id Software LLC, a ZeniMax Media company. See NOTICE.
//!
//! `PmoveSingle` and its arms: the frame's setup, the linked and seated arms, the
//! default arm's tail and the dead arms.

use super::footsteps::footsteps;
use super::ground::{ground_trace, leave_ground, set_water_level};
use super::jump::{check_jump, jump_penalty_landing};
use super::ladder::{cap_ladder_yaw, check_ladder_move, ladder_check_skipped, ladder_move};
use super::land::{LAND_ANIM_SPEED, crash_land};
use super::lean::update_lean;
use super::prone::{update_prone_pitch, update_prone_view};
use super::stance::{step_view_target, update_stance, view_height_adjust_v15};
use super::state::{MoveRules, PlayerState, PmEvent, PmInput, Stance, VehicleSeat};
use super::uo::{ads_flag, uo_prologue, update_fatigue, update_sprint_flag};
use super::walk::{air_move, friction, walk_move};
use super::{DEAD_VIEW_LERP_SPEED, MAX_FRAME_MS, VIEW_DEAD, weapon};
use crate::collision::MASK_PLAYERSOLID;
use crate::movetrace::{MASK_DEADSOLID, MoveWorld};
use crate::weapon::WeaponDef;
use glam::Vec3;

/// `pml.msec` back out of a step's frametime ([`cmd::frametime`]), which is
/// not a whole number of milliseconds times 0.001 for most lengths.
pub(super) fn msec(dt: f32) -> f32 {
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

/// The per-move outputs `PmoveSingle` clears at its top whatever the
/// pm_type (`pm_flags` 0x8000 at 0x33e5a): every move arm calls it first.
pub(super) fn begin_move(ps: &mut PlayerState) {
    ps.view_yaw_correction = 0.0;
    ps.view_pitch_correction = 0.0;
    ps.view_yaw_shorts = None;
    ps.view_capped = [None; 2];
    ps.prone_blocked = false;
    ps.jumped = false;
    ps.land_anim = false;
    ps.takeoff = None;
    ps.breakdown_continue = false;
    ps.prone_flip = false;
    ps.prone_flip_clears_torso = false;
}

/// `dt` in seconds, clamped to `MAX_FRAME_MS`. Returns the frame's events
/// (movement, stance and weapon) in wire `EV_*` numbering.
pub fn pmove(
    ps: &mut PlayerState,
    input: &PmInput,
    world: &MoveWorld,
    dt: f32,
    weapons: &[Option<WeaponDef>],
) -> Vec<PmEvent> {
    let dt = dt.min(MAX_FRAME_MS / 1000.0);
    begin_move(ps);
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
    weapon::adjust_aim_spread_scale(ps, input, weapon::weapon_def(weapons, ps.weapon), dt);
    let weapon_def = weapon::weapon_def(weapons, ps.weapon);
    let mut events = Vec::new();
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
    cap_ladder_yaw(ps, world);
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
    cap_ladder_yaw(ps, world);
    update_prone_view(ps, input, world, dt);
    update_lean(ps, input, true, world, dt);
    // The arm's first store: `groundEntityNum` 1023, `pml.walking` and
    // `pml.groundPlane` 0.
    leave_ground(ps);
    ads_flag(
        ps,
        input,
        weapon::weapon_def(weapons, ps.weapon),
        world,
        events,
    );
    ps.walking = walking_flag(ps, input);
    update_sprint_flag(ps, input, weapon::weapon_def(weapons, ps.weapon));
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
    leave_ground(ps);
    // `PM_UpdateAimDownSightFlag`'s first clause (0x32c0b).
    ps.ads_active = input.ads;
    ps.walking = walking_flag(ps, input);
    let def = weapon::weapon_def(weapons, ps.weapon);
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

/// `PmoveSingle`'s early zeroing of the cmd's forward and right moves under
/// the sight flag: on a prone body (1.1 0x33f48 tests `pm_flags` 0x21, 1.5
/// 0x281ae the effective stance) and, on UO, on a deployed bipod
/// (coduo-movement.md, 1 and 13).
pub(super) fn hold_still_sighted(ps: &PlayerState, input: &PmInput) -> PmInput {
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
pub(super) fn clamp_velocity_to_move(ps: &mut PlayerState, world: &MoveWorld, dt: f32) {
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
pub(super) fn snap_velocity(ps: &mut PlayerState) {
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
    begin_move(ps);
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
        leave_ground(ps);
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
    begin_move(ps);
    ps.move_start = ps.origin;
    ps.ads_active = false;
    update_lean(ps, &PmInput::default(), false, world, dt);
    leave_ground(ps);
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
