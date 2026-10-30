//! The ladder: `PM_CheckLadderMove` (0x80e805a), `PM_LadderMove`
//! (0x80e847e), the jump's push-off (0x80dcb48), the yaw cap of
//! `PM_UpdateViewAngles` and the rung footstep of `PM_FootstepEvent`
//! (docs/research/cod2-movement.md 11).

use glam::Vec3;

use super::fpu::{self, d};
use super::moves::{accelerate, air_move, cmd_scale_flat, jump_check};
use super::slide::step_slide_move;
use super::{ENTITYNUM_NONE, Pml, Pmove, Stance, leave_ladder, pm_type, pmf};
use crate::collision::{CONTENTS_BODY, CONTENTS_PLAYERCLIP, SURF_LADDER};

/// rodata 0x815514c (and 0x8155084 in the footstep): the probe box's
/// shrink a side.
const PROBE_SHRINK: f32 = 6.0;
/// The probe box's floor (an immediate).
const PROBE_FLOOR: f32 = 8.0;
/// The reach walking and airborne (immediates 0x41000000, 0x41f00000).
const REACH_WALK: f32 = 8.0;
const REACH_AIR: f32 = 30.0;
/// rodata 0x8155150, 0x8155154: the climb's bias and gain on `forward.z`.
const UP_BIAS: f32 = 0.25;
const UP_GAIN: f32 = 2.5;
/// rodata 0x815515c, 0x8155160: the forward and strafe keys' shares.
const CLIMB_SCALE: f32 = 0.5;
const STRAFE_SCALE: f32 = f32::from_bits(0x3e4c_cccd);
/// The accelerate's rate (immediate 0x41100000).
const ACCEL: f32 = 9.0;
/// rodata 0x8155164: the sideways bleed per second.
const SIDE_FRICTION: f32 = 16.0;
/// The airborne press into the ladder (immediate 0xc2480000).
const PRESS: f32 = -50.0;
/// rodata 0x8155168 (and 0x8155128 in the yaw cap): the ladder's facing is
/// its normal turned round.
const TURN: f32 = 180.0;
/// `movementDir`'s cap on the ladder (immediate 0x4b).
const MOVEMENT_DIR_CAP: i32 = 75;
/// rodata 0x8153344, 0x8153348: the push-off's vz scale and the reflection.
const PUSH_OFF_VZ: f32 = 0.75;
const REFLECT: f32 = -2.0;
/// The rung footstep's reach into the ladder (immediate 0xc1f80000).
const RUNG_REACH: f32 = -31.0;
/// The rung footstep with no material under the probe (immediate 0x15).
const RUNG_DEFAULT_SURFACE: i32 = 0x15;

/// The probe box: the cmd's box shrunk 6 a side, its floor at 8 and its top
/// at least 8.
fn probe_box(pm: &Pmove) -> (Vec3, Vec3) {
    let mut mins = pm.mins;
    mins.x += PROBE_SHRINK;
    mins.y += PROBE_SHRINK;
    mins.z = PROBE_FLOOR;
    let mut maxs = pm.maxs;
    maxs.x -= PROBE_SHRINK;
    maxs.y -= PROBE_SHRINK;
    if maxs.z < mins.z {
        maxs.z = mins.z;
    }
    (mins, maxs)
}

/// `PM_CheckLadderMove`: on the ground the flat view must meet a ladder
/// surface within 8 with a forward key held; in the air within 30, along
/// `-vLadderVec` once on one. A fresh grab takes the struck normal as
/// `vLadderVec` and must meet the ladder again along it. Skipped while a
/// land or knockback timer runs off the ladder; refused prone, dead, within
/// 300 ms of a jump and until a walking frame after leaving one.
pub(crate) fn check_ladder_move(pm: &mut Pmove, pml: &mut Pml) {
    if pml.walking {
        pm.ps.pm_flags &= !pmf::LADDER_LEFT;
    }
    let flags = pm.ps.pm_flags;
    if pm.ps.pm_time != 0
        && flags & pmf::LADDER == 0
        && flags & (pmf::TIME_LAND | pmf::TIME_KNOCKBACK) != 0
    {
        return;
    }
    let reach = if pml.walking { REACH_WALK } else { REACH_AIR };
    let stick = flags & pmf::LADDER != 0 && pm.ps.ground_entity_num == ENTITYNUM_NONE;
    let dir = if stick {
        -pm.ps.ladder_vec
    } else {
        fpu::normalize(Vec3::new(pml.forward.x, pml.forward.y, 0.0)).0
    };
    if pm.ps.pm_type >= pm_type::DEAD {
        pm.ps.ground_entity_num = ENTITYNUM_NONE;
        pml.ground_plane = false;
        pml.almost_ground_plane = false;
        pml.walking = false;
        leave_ladder(pm.ps);
        return;
    }
    if flags & pmf::LADDER_LEFT != 0
        || pm.ps.effective_stance() == Stance::Prone
        || pm.cmd.server_time - pm.ps.jump_time < 300
    {
        leave_ladder(pm.ps);
        return;
    }
    let (mins, maxs) = probe_box(pm);
    let origin = pm.ps.origin;
    let mask = pm.tracemask;
    let t = pm.trace_box(origin, fpu::ma(origin, reach, dir), mins, maxs, mask);
    let ladder = |t: &super::Tr| t.fraction < 1.0 && t.surface_flags & SURF_LADDER != 0;
    if ladder(&t) && (!pml.walking || pm.cmd.forward_move > 0) {
        if flags & pmf::LADDER != 0 {
            pm.ps.pm_flags |= pmf::LADDER;
            return;
        }
        pm.ps.ladder_vec = t.normal;
        let back = -pm.ps.ladder_vec;
        let t = pm.trace_box(origin, fpu::ma(origin, reach, back), mins, maxs, mask);
        if ladder(&t) {
            pm.ps.pm_flags |= pmf::LADDER;
            return;
        }
    }
    leave_ladder(pm.ps);
    // Let go of the rungs in the air: the jump anim, forced (0x80e805a).
    if stick {
        super::anim::event(pm, super::anim::Event::Jump, false, true);
    }
}

/// `PM_LadderMove`: a jump pushes off through the air move; else the climb
/// `forwardmove * 0.5 * clamp((forward.z + 0.25) * 2.5, -1, 1)` up and the
/// strafe `rightmove * 0.2` along the flat right laid on the ladder's
/// plane, both times the 2-axis cmd scale, through the ladder's
/// accelerate; vz bled by gravity with no forward key, the sideways
/// velocity bled 16/s with no strafe key, and in the air the velocity into
/// the ladder taken away and a 50 press into it while the flat speed is no
/// more than the vertical. `movementDir` faces the ladder, capped at 75.
pub(crate) fn ladder_move(pm: &mut Pmove, pml: &mut Pml) {
    if jump_check(pm, pml) {
        air_move(pm, pml);
        return;
    }
    let up = ((d(UP_GAIN) * (d(UP_BIAS) + d(pml.forward.z))) as f32).clamp(-1.0, 1.0);
    pml.forward.z = 0.0;
    pml.forward = fpu::normalize(pml.forward).0;
    pml.right.z = 0.0;
    let right = fpu::normalize_to(pml.right).0;
    // `ProjectPointOnPlane` (0x80aa0e4): the dot cast to a float.
    let n = pm.ps.ladder_vec;
    pml.right = fpu::ma(right, -(fpu::dot(n, right) as f32), n);

    let (fm, rm) = (pm.cmd.forward_move, pm.cmd.right_move);
    let scale = cmd_scale_flat(pm.ps, fm, rm, &pm.rules);
    let mut wishvel = Vec3::ZERO;
    if fm != 0 {
        wishvel.z = (f64::from(fm) * d(CLIMB_SCALE) * d(up) * d(scale)) as f32;
    }
    if rm != 0 {
        let s = (f64::from(rm) * d(STRAFE_SCALE) * d(scale)) as f32;
        wishvel = fpu::ma(wishvel, s, pml.right);
    }
    let (wishdir, wishspeed) = fpu::normalize_to(wishvel);
    accelerate(pm, pml, wishdir, wishspeed, ACCEL);

    let ps = &mut *pm.ps;
    if fm == 0 {
        let g = f64::from(ps.gravity as f32) * d(pml.frametime);
        if ps.velocity.z > 0.0 {
            ps.velocity.z = (d(ps.velocity.z) - g) as f32;
            if ps.velocity.z < 0.0 {
                ps.velocity.z = 0.0;
            }
        } else {
            ps.velocity.z = (d(ps.velocity.z) + g) as f32;
            if ps.velocity.z > 0.0 {
                ps.velocity.z = 0.0;
            }
        }
    }
    if rm == 0 {
        let (sx, sy) = fpu::normalize2(pml.right.x, pml.right.y);
        let side = Vec3::new(sx, sy, 0.0);
        let along = fpu::dot2(side, ps.velocity) as f32;
        if along != 0.0 {
            ps.velocity = ma2(ps.velocity, -along, side);
            let mut drop = (d(SIDE_FRICTION) * d(along) * d(pml.frametime)) as f32;
            if drop.abs() < along.abs() {
                if drop.abs() < 1.0 {
                    drop = if drop.is_sign_negative() { -1.0 } else { 1.0 };
                }
                ps.velocity = ma2(ps.velocity, along - drop, side);
            }
        }
    }
    if !pml.walking {
        let into = fpu::dot2(n, ps.velocity) as f32;
        ps.velocity = ma2(ps.velocity, -into, n);
        let flat = fpu::dot2(ps.velocity, ps.velocity) as f32;
        if f64::from(flat) <= d(ps.velocity.z) * d(ps.velocity.z) {
            ps.velocity = ma2(ps.velocity, PRESS, n);
        }
    }
    step_slide_move(pm, pml, false);
    let ps = &mut *pm.ps;
    let facing = (d(TURN) + d(fpu::vectoyaw(ps.ladder_vec))) as f32;
    let a = fpu::angle_delta(facing, ps.view_angles[1]) as i32;
    let a = if a.abs() > MOVEMENT_DIR_CAP {
        if a < 1 {
            -MOVEMENT_DIR_CAP
        } else {
            MOVEMENT_DIR_CAP
        }
    } else {
        a
    };
    ps.movement_dir = a as i8 as i32;
}

/// The two-axis `VectorMA` (0x80e970a): x and y only, each stored.
fn ma2(a: Vec3, s: f32, b: Vec3) -> Vec3 {
    Vec3::new(
        (d(a.x) + d(s) * d(b.x)) as f32,
        (d(a.y) + d(s) * d(b.y)) as f32,
        a.z,
    )
}

/// The push-off (0x80dcb48), after the jump's takeoff on a ladder: vz
/// x0.75, and the flat view, reflected off `vLadderVec` when it faces into
/// it, times `jump_ladderPushVel`; the ladder flag cleared without the
/// left-ladder mark.
pub(crate) fn push_off(pm: &mut Pmove, pml: &Pml) {
    let ps = &mut *pm.ps;
    ps.velocity.z = (d(PUSH_OFF_VZ) * d(ps.velocity.z)) as f32;
    let flat = fpu::normalize(Vec3::new(pml.forward.x, pml.forward.y, 0.0)).0;
    let n = ps.ladder_vec;
    let dir = if (fpu::dot(n, pml.forward) as f32) < 0.0 {
        let k = (d(REFLECT) * d(fpu::dot(flat, n) as f32)) as f32;
        fpu::normalize(fpu::ma(flat, k, n)).0
    } else {
        flat
    };
    let push = pm.rules.jump_ladder_push_vel;
    ps.velocity.x = (d(push) * d(dir.x)) as f32;
    ps.velocity.y = (d(push) * d(dir.y)) as f32;
    ps.pm_flags &= !pmf::LADDER;
}

/// The ladder arm of `PM_UpdateViewAngles` (0x80e6eb8): on a ladder off
/// the ground, a view more than `bg_ladder_yawcap` off the ladder's facing
/// is pushed back to the cap through `delta_angles`.
pub(crate) fn cap_yaw(pm: &mut Pmove) {
    let cap = pm.rules.ladder_yawcap;
    let ps = &mut *pm.ps;
    if ps.pm_flags & pmf::LADDER == 0 || ps.ground_entity_num != ENTITYNUM_NONE || cap == 0.0 {
        return;
    }
    let facing = (d(TURN) + d(fpu::vectoyaw(ps.ladder_vec))) as f32;
    let mut delta = fpu::angle_delta(facing, ps.view_angles[1]);
    if delta <= cap && delta >= -cap {
        return;
    }
    delta = if delta > cap {
        delta - cap
    } else {
        delta + cap
    };
    ps.delta_angles[1] = ps.delta_angles[1].wrapping_add(fpu::shorts(delta));
    ps.view_angles[1] = fpu::normalize360_accurate(if delta > 0.0 {
        facing - cap
    } else {
        facing + cap
    });
}

/// `PM_FootstepEvent`'s airborne arm: on a ladder, the footstep of the
/// material 31 into it (the probe box, no bodies, no player clip), 21 for
/// none.
pub(crate) fn rung_footstep(pm: &mut Pmove) {
    let (mins, maxs) = probe_box(pm);
    let mask = pm.tracemask & !(CONTENTS_BODY | CONTENTS_PLAYERCLIP);
    let origin = pm.ps.origin;
    let end = fpu::ma(origin, RUNG_REACH, pm.ps.ladder_vec);
    let t = pm.trace_box(origin, end, mins, maxs, mask);
    let mut surf = ((t.surface_flags & 0x1f0_0000) >> 20) as i32;
    if t.fraction == 1.0 || surf == 0 {
        surf = RUNG_DEFAULT_SURFACE;
    }
    pm.ps.add_event(surf + 1, 0);
}
