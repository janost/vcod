//! Contains routines ported from the Quake III Arena GPL source, Copyright (C) 1999-2005 Id Software, Inc.,
//! and the RTCW-MP GPL source, Copyright (C) 1999-2010 id Software LLC, a ZeniMax Media company. See NOTICE.
//!
//! `PM_CheckLadderMove`, `PM_LadderMove` and the ladder yaw cap.

use super::angles::{
    angle_delta, angle_normalize360_short, delta_stored, normalize360_accurate, vectoyaw, view_deg,
    view_rad,
};
use super::ground::pm_trace;
use super::jump::check_jump;
use super::slide::step_slide_move;
use super::state::{MoveRules, PlayerState, PmEvent, PmInput, Stance};
use super::walk::{
    LADDER_MOVEMENT_DIR_CAP, accelerate_msvc, accelerate_stored, air_move, clamp_movement_dir,
    cmd_scale, cmd_scale_x87, project_point_on_plane_x87, view_vectors,
};
use super::{PMF_TIME_DAMAGE, PMF_TIME_KNOCKBACK};
use crate::collision::MASK_PLAYERSOLID;
use crate::movetrace::MoveWorld;
use crate::x87;
use glam::Vec3;

/// `bg_ladder_yawcap`'s default: how far a climber off the ground may look
/// away from the rungs (cod11-mantle.md, "The ladder yaw cap").
pub const LADDER_YAWCAP: f32 = 100.0;

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

/// Horizontal reset along the reflected forward when leaving a ladder
/// (pm_ladderPushOff, rodata 0x708d8).
pub const LADDER_PUSHOFF_SPEED: f32 = 128.0;

/// The ladder arm of `PM_UpdateViewAngles` (1.1 0x32e7d-0x32f75, 1.5
/// 0x26270-0x263ca, UO 0x2b535-0x2b6a5): on last frame's ladder flag and
/// off the ground and under a non-zero `bg_ladder_yawcap`, a view more
/// than the cap off the rungs' facing, `AngleDelta(vectoyaw(vLadderVec) + 180, yaw)`, is pushed back
/// onto the cap through `delta_angles` and placed at `facing -/+ cap`,
/// through `AngleNormalize360` on 1.1 and `AngleNormalize360Accurate` on
/// 1.5 and UO (cod11-mantle.md, "The ladder yaw cap").
pub(super) fn cap_ladder_yaw(ps: &mut PlayerState, world: &MoveWorld) {
    if !ps.on_ladder || ps.on_ground || ps.bg.ladder_yawcap == 0 {
        return;
    }
    let cap = ps.bg.ladder_yawcap as f32;
    let d = angle_delta(
        ladder_facing(ps.ladder_normal, ps.rules),
        view_deg(ps.yaw),
        delta_stored(ps.rules, world),
    );
    if d.abs() <= cap {
        return;
    }
    let excess = d - cap.copysign(d);
    // 1.1 keeps the facing on the x87 stack; 1.5 and UO stored it.
    let facing = if ps.rules.v15() {
        f64::from(ladder_facing(ps.ladder_normal, ps.rules))
    } else {
        vectoyaw(ps.ladder_normal) + 180.0
    };
    let at = (facing - f64::from(cap.copysign(excess))) as f32;
    let yaw = if ps.rules.v15() {
        normalize360_accurate(at)
    } else {
        angle_normalize360_short(at)
    };
    ps.view_yaw_correction = excess;
    ps.view_capped[1] = Some(yaw);
    ps.yaw = view_rad(yaw);
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

/// Look direction including pitch; pitch is up-positive.
pub(super) fn forward3(ps: &PlayerState) -> Vec3 {
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
pub(super) fn ladder_check_skipped(ps: &PlayerState) -> bool {
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
pub(super) fn check_ladder_move(
    ps: &mut PlayerState,
    input: &PmInput,
    world: &MoveWorld,
) -> Option<Vec3> {
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
    if ps.effective_stance() == Stance::Prone || ps.since_jump_ms < LADDER_REGRAB_LOCK_MS {
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
    // 1.5 stores the view trace's normal (0x27699), then confirms a new
    // grab along it (0x27652, 0x277a9): a refused grab keeps the normal,
    // which goes on the wire as `vLadderVec` (cod15-movement.md 2).
    if ladder && ps.rules.v15() && !was_on_ladder {
        ps.ladder_normal = t.normal;
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

/// Retail `PM_LadderMove` (game.mp.i386.so 0x33944): the push-off, then a
/// wish of `forwardmove * 0.5 * upscale` up, where the view's pitch sets
/// `upscale = clamp((forward.z + 0.25) * 2.5, -1, 1)`, plus `rightmove * 0.2`
/// along the flat right laid on the ladder's plane, both times the cmd
/// scale; an inline accelerate with no friction; a vertical stop without the
/// forward key; a 16/s bleed of the sideways velocity without the strafe
/// key; and in the air, the press into the wall
/// (docs/research/cod11-mantle.md, "Ladders").
pub(super) fn ladder_move(
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
    let right = if v15 || world.world.arith() == crate::patch::Arith::Msvc {
        flat_right - normal * flat_right.dot(normal)
    } else {
        project_point_on_plane_x87(flat_right, normal)
    };
    let (f, r) = (input.forward * 127.0, input.right * 127.0);
    let scale = if v15 {
        f64::from(cmd_scale(
            ps,
            input,
            world.world.arith() == crate::patch::Arith::Stored,
        ))
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
        angle_delta(
            ladder_facing(normal, ps.rules),
            view_deg(ps.yaw),
            delta_stored(ps.rules, world),
        ) as i32,
        LADDER_MOVEMENT_DIR_CAP,
    );
}
