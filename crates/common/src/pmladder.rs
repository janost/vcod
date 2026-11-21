//! The ladder CoD2 1.3 and CoD4 1.7 share: `PM_CheckLadderMove`,
//! `PM_LadderMove`, the jump's push-off, the ladder arm of
//! `PM_UpdateViewAngles` and the rung step of `PM_FootstepEvent`
//! (docs/research/cod2-movement.md 11, cod4-pmove.md 7, 8 and 9). Where
//! the two binaries part, [`LadderPm`] has a hook naming both addresses.
//! Each constant cites CoD2's rodata, then 1.7's. What this rounds where
//! 1.7 keeps the value on the stack: cod4-pmove.md 7.

use glam::Vec3;

use crate::clipmap::{
    CONTENTS_BODY, CONTENTS_PLAYERCLIP, LADDER_SURFACE, SURF_LADDER, surface_material,
};
use crate::net::protocol::ENTITYNUM_NONE;
use crate::pmove::Stance;
use crate::pmshared::{PmState, Pml, airborne, leave_ladder};
use crate::x87::{dot, dot2, ma_stored as ma};

/// The probe box's shrink a side: 0x815514c (0x8155084 in the footstep),
/// 0x820aacc.
const PROBE_SHRINK: f32 = 6.0;
/// The probe box's floor: an immediate, 0x820aac4.
const PROBE_FLOOR: f32 = 8.0;
/// The reach walking and airborne: immediates 0x41000000 and 0x41f00000,
/// 0x820aac4 and 0x820dee8.
const REACH_WALK: f32 = 8.0;
const REACH_AIR: f32 = 30.0;
/// The climb's bias and gain on `forward.z`: 0x8155150 and 0x8155154,
/// 0x820966c and 0x820a31c.
const UP_BIAS: f32 = 0.25;
const UP_GAIN: f32 = 2.5;
/// The forward and strafe keys' shares: 0x815515c and 0x8155160, 0x8209628
/// and 0x8209690.
const CLIMB_SCALE: f32 = 0.5;
const STRAFE_SCALE: f32 = f32::from_bits(0x3e4c_cccd);
/// The accelerate's rate: immediate 0x41100000, at 0x805d8a8 on 1.7.
const ACCEL: f32 = 9.0;
/// The sideways bleed per second: 0x8155164, 0x820967c.
const SIDE_FRICTION: f32 = 16.0;
/// The airborne press into the ladder: immediate 0xc2480000, 0x820aad8.
const PRESS: f32 = -50.0;
/// The ladder's facing is its normal turned round: 0x8155168 (0x8155128 in
/// the yaw cap), 0x8209668 in 1.7's yaw cap.
const TURN: f32 = 180.0;
/// `movementDir`'s cap on the ladder: immediate 0x4b, 0x805da44.
const MOVEMENT_DIR_CAP: i32 = 75;
/// The push-off's vz scale and the reflection: 0x8153344 and 0x8153348,
/// 0x820a328 and 0x820a32c.
const PUSH_OFF_VZ: f32 = 0.75;
const REFLECT: f32 = -2.0;
/// The rung footstep's reach into the ladder: immediate 0xc1f80000,
/// 0x820deec.
const RUNG_REACH: f32 = -31.0;
/// The rung footstep's material with none under the probe: immediate 0x15,
/// 0x805a4d5..0x805a4f8.
const RUNG_DEFAULT_SURFACE: u32 = LADDER_SURFACE;

#[inline]
fn d(x: f32) -> f64 {
    f64::from(x)
}

/// A trace as the ladder reads it.
#[derive(Clone, Copy, Debug)]
pub struct LadderHit {
    pub fraction: f32,
    pub normal: Vec3,
    pub surface_flags: u32,
}

/// A game's `pmove_t` as the ladder reads it, the moves it calls, and the
/// hooks where CoD2 and 1.7 part (each names CoD2's address, then 1.7's).
pub trait LadderPm {
    type Ps: PmState;
    /// The game's trace, `Pml`'s ground trace.
    type Tr;

    /// The airborne press of 50 waits for a moving body: not on CoD2
    /// (0x80e847e), on 1.7 (0x805d973..0x805d9a2).
    const PRESS_NEEDS_MOTION: bool;

    fn ps(&mut self) -> &mut Self::Ps;
    /// The cmd's box (`pmove_t` mins and maxs).
    fn bounds(&self) -> (Vec3, Vec3);
    fn tracemask(&self) -> u32;
    /// The cmd's `serverTime`, `forwardmove` and `rightmove`.
    fn cmd(&self) -> (i32, i8, i8);
    /// `bg_ladder_yawcap`.
    fn ladder_yawcap(&self) -> f32;
    /// `jump_ladderPushVel`.
    fn ladder_push_vel(&self) -> f32;
    /// `PM_playerTrace`: the box trace, past a body it starts in.
    fn ladder_trace(
        &mut self,
        start: Vec3,
        end: Vec3,
        mins: Vec3,
        maxs: Vec3,
        mask: u32,
    ) -> LadderHit;

    fn jump_check(&mut self, pml: &mut Pml<Self::Tr>) -> bool;
    fn air_move(&mut self, pml: &mut Pml<Self::Tr>);
    fn accelerate(&mut self, pml: &Pml<Self::Tr>, wishdir: Vec3, wishspeed: f32, accel: f32);
    /// `PM_StepSlideMove` without gravity.
    fn step_slide_move(&mut self, pml: &mut Pml<Self::Tr>);
    /// `BG_AnimScriptEvent(ps, ANIM_ET_JUMP, 0, 1)`: the jump anim, forced.
    fn jump_anim(&mut self);

    /// `PM_CmdScale` on the two move keys: CoD2's stored (0x80e1b1e), 1.7's
    /// kept (0x805d73f..0x805d812).
    fn cmd_scale(&mut self, fm: i8, rm: i8) -> f64;
    /// The yaw cap's excess into `delta_angles[1]`: CoD2's short add
    /// (0x80e6eb8), 1.7's float add (0x8057f19..0x8057f1e).
    fn push_delta_yaw(&mut self, excess: f32);
    /// The rung footstep on material `surf`: CoD2's event `surf + 1` with
    /// no parm (0x80e5402), 1.7's `EV_FOOTSTEP_RUN` with `surf`
    /// (0x805a4ca..0x805a4f8).
    fn rung_event(&mut self, surf: u32);

    /// `Vec3Normalize`: CoD2's stored (0x80a4424), 1.7's kept (0x81915d8).
    fn normalize(v: Vec3) -> (Vec3, f32);
    /// `Vec3NormalizeTo`: CoD2's stored (0x80a45fc), 1.7's kept (0x8192a64).
    fn normalize_to(v: Vec3) -> (Vec3, f32);
    /// `vectoyaw`: CoD2's stored (0x80a4d5a), 1.7's kept (0x8191c22).
    fn vectoyaw(v: Vec3) -> f32;
    /// `AngleDelta`: CoD2's on the short grid (0x80a85bc), 1.7's off
    /// `floor` (0x8190590).
    fn angle_delta(a: f32, b: f32) -> f32;
    /// The yaw cap's fold: CoD2's `AngleNormalize360Accurate` (0x80a84b8),
    /// 1.7's `AngleNormalize360` (0x8190540).
    fn angle_normalize360(a: f32) -> f32;
}

/// The probe box: the cmd's box shrunk 6 a side, its floor at 8 and its top
/// at least 8.
fn probe_box((mut mins, mut maxs): (Vec3, Vec3)) -> (Vec3, Vec3) {
    mins.x += PROBE_SHRINK;
    mins.y += PROBE_SHRINK;
    mins.z = PROBE_FLOOR;
    maxs.x -= PROBE_SHRINK;
    maxs.y -= PROBE_SHRINK;
    if maxs.z < mins.z {
        maxs.z = mins.z;
    }
    (mins, maxs)
}

fn ladder(t: &LadderHit) -> bool {
    t.fraction < 1.0 && t.surface_flags & SURF_LADDER != 0
}

/// `PM_CheckLadderMove`: on the ground the flat view must meet a ladder
/// surface within 8 with a forward key held; in the air within 30, along
/// `-vLadderVec` once on one. A fresh grab takes the struck normal as
/// `vLadderVec` and must meet the ladder again along it. Skipped while a
/// land or knockback timer runs off the ladder; refused prone, dead, within
/// 300 ms of a jump and until a walking frame after leaving one. Letting go
/// in the air raises the forced jump anim.
pub fn check_ladder_move<P: LadderPm>(pm: &mut P, pml: &mut Pml<P::Tr>) {
    let (server_time, forward_move, _) = pm.cmd();
    let (mins, maxs) = probe_box(pm.bounds());
    let mask = pm.tracemask();
    let ps = pm.ps();
    if pml.walking {
        *ps.pm_flags() &= !P::Ps::PMF_LADDER_LEFT;
    }
    let flags = *ps.pm_flags();
    if *ps.pm_time() != 0
        && flags & P::Ps::PMF_LADDER == 0
        && flags & (P::Ps::PMF_TIME_LAND | P::Ps::PMF_TIME_KNOCKBACK) != 0
    {
        return;
    }
    let reach = if pml.walking { REACH_WALK } else { REACH_AIR };
    let stick = flags & P::Ps::PMF_LADDER != 0 && *ps.ground_entity_num() == ENTITYNUM_NONE as i32;
    let dir = if stick {
        -*ps.ladder_vec()
    } else {
        P::normalize(Vec3::new(pml.forward.x, pml.forward.y, 0.0)).0
    };
    if *ps.pm_type() >= P::Ps::PM_DEAD {
        airborne(ps, pml);
        leave_ladder(ps);
        return;
    }
    if flags & P::Ps::PMF_LADDER_LEFT != 0
        || ps.effective_stance() == Stance::Prone
        || server_time - *ps.jump_time() < 300
    {
        leave_ladder(ps);
        return;
    }
    let origin = *ps.origin();
    let t = pm.ladder_trace(origin, ma(origin, reach, dir), mins, maxs, mask);
    if ladder(&t) && (!pml.walking || forward_move > 0) {
        if flags & P::Ps::PMF_LADDER != 0 {
            *pm.ps().pm_flags() |= P::Ps::PMF_LADDER;
            return;
        }
        *pm.ps().ladder_vec() = t.normal;
        let back = -t.normal;
        let t = pm.ladder_trace(origin, ma(origin, reach, back), mins, maxs, mask);
        if ladder(&t) {
            *pm.ps().pm_flags() |= P::Ps::PMF_LADDER;
            return;
        }
    }
    leave_ladder(pm.ps());
    // Let go of the rungs in the air: CoD2 0x80e805a, 1.7 0x805a0a7..0x805a0cc.
    if stick {
        pm.jump_anim();
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
pub fn ladder_move<P: LadderPm>(pm: &mut P, pml: &mut Pml<P::Tr>) {
    if pm.jump_check(pml) {
        pm.air_move(pml);
        return;
    }
    let up = ((d(UP_GAIN) * (d(UP_BIAS) + d(pml.forward.z))) as f32).clamp(-1.0, 1.0);
    pml.forward.z = 0.0;
    pml.forward = P::normalize(pml.forward).0;
    pml.right.z = 0.0;
    let right = P::normalize_to(pml.right).0;
    // `ProjectPointOnPlane` (CoD2 0x80aa0e4, 1.7 0x8190e76): the dot cast to a float.
    let n = *pm.ps().ladder_vec();
    pml.right = ma(right, -(dot(n, right) as f32), n);

    let (_, fm, rm) = pm.cmd();
    let scale = pm.cmd_scale(fm, rm);
    let mut wishvel = Vec3::ZERO;
    if fm != 0 {
        wishvel.z = (f64::from(fm) * d(CLIMB_SCALE) * d(up) * scale) as f32;
    }
    if rm != 0 {
        let s = (f64::from(rm) * d(STRAFE_SCALE) * scale) as f32;
        wishvel = ma(wishvel, s, pml.right);
    }
    let (wishdir, wishspeed) = P::normalize_to(wishvel);
    pm.accelerate(pml, wishdir, wishspeed, ACCEL);

    let ps = pm.ps();
    let mut v = *ps.velocity();
    if fm == 0 {
        let g = f64::from(*ps.gravity() as f32) * d(pml.frametime);
        if v.z > 0.0 {
            v.z = (d(v.z) - g) as f32;
            if v.z < 0.0 {
                v.z = 0.0;
            }
        } else {
            v.z = (d(v.z) + g) as f32;
            if v.z > 0.0 {
                v.z = 0.0;
            }
        }
    }
    if rm == 0 {
        let (sx, sy) = crate::x87::normalize2_kept(pml.right.x, pml.right.y);
        let side = Vec3::new(sx, sy, 0.0);
        let along = dot2(side, v) as f32;
        if along != 0.0 {
            v = ma2(v, -along, side);
            let mut drop = (d(SIDE_FRICTION) * d(along) * d(pml.frametime)) as f32;
            if drop.abs() < along.abs() {
                if drop.abs() < 1.0 {
                    drop = if drop.is_sign_negative() { -1.0 } else { 1.0 };
                }
                v = ma2(v, along - drop, side);
            }
        }
    }
    if !pml.walking {
        let into = dot2(n, v) as f32;
        v = ma2(v, -into, n);
        let flat = dot2(v, v) as f32;
        if (!P::PRESS_NEEDS_MOTION || v != Vec3::ZERO) && f64::from(flat) <= d(v.z) * d(v.z) {
            v = ma2(v, PRESS, n);
        }
    }
    *ps.velocity() = v;
    pm.step_slide_move(pml);
    let ps = pm.ps();
    let facing = (d(TURN) + d(P::vectoyaw(*ps.ladder_vec()))) as f32;
    let a = P::angle_delta(facing, ps.view_angles()[1]) as i32;
    let a = if a.abs() > MOVEMENT_DIR_CAP {
        if a < 1 {
            -MOVEMENT_DIR_CAP
        } else {
            MOVEMENT_DIR_CAP
        }
    } else {
        a
    };
    *ps.movement_dir() = a as i8 as i32;
}

/// The two-axis `VectorMA` (CoD2 0x80e970a): x and y only, each stored.
fn ma2(a: Vec3, s: f32, b: Vec3) -> Vec3 {
    Vec3::new(
        (d(a.x) + d(s) * d(b.x)) as f32,
        (d(a.y) + d(s) * d(b.y)) as f32,
        a.z,
    )
}

/// The push-off (CoD2 0x80dcb48, 1.7 0x8050c0f..0x8050d65), after the
/// jump's takeoff on a ladder: vz x0.75, and the flat view, reflected off
/// `vLadderVec` when it faces into it, times `jump_ladderPushVel`; the
/// ladder flag cleared without the left-ladder mark.
pub fn push_off<P: LadderPm>(pm: &mut P, pml: &Pml<P::Tr>) {
    let push = pm.ladder_push_vel();
    let ps = pm.ps();
    let mut v = *ps.velocity();
    v.z = (d(PUSH_OFF_VZ) * d(v.z)) as f32;
    let flat = P::normalize(Vec3::new(pml.forward.x, pml.forward.y, 0.0)).0;
    let n = *ps.ladder_vec();
    let dir = if (dot(n, pml.forward) as f32) < 0.0 {
        let k = (d(REFLECT) * d(dot(flat, n) as f32)) as f32;
        P::normalize(ma(flat, k, n)).0
    } else {
        flat
    };
    v.x = (d(push) * d(dir.x)) as f32;
    v.y = (d(push) * d(dir.y)) as f32;
    *ps.velocity() = v;
    *ps.pm_flags() &= !P::Ps::PMF_LADDER;
}

/// The ladder arm of `PM_UpdateViewAngles` (CoD2 0x80e6eb8, 1.7
/// 0x8057ea2..0x8058179): on a ladder off the ground, a view more than
/// `bg_ladder_yawcap` off the ladder's facing is pushed back to the cap
/// through `delta_angles`.
pub fn cap_yaw<P: LadderPm>(pm: &mut P) {
    let cap = pm.ladder_yawcap();
    let ps = pm.ps();
    if *ps.pm_flags() & P::Ps::PMF_LADDER == 0
        || *ps.ground_entity_num() != ENTITYNUM_NONE as i32
        || cap == 0.0
    {
        return;
    }
    let facing = (d(TURN) + d(P::vectoyaw(*ps.ladder_vec()))) as f32;
    let mut delta = P::angle_delta(facing, ps.view_angles()[1]);
    if delta <= cap && delta >= -cap {
        return;
    }
    delta = if delta > cap {
        delta - cap
    } else {
        delta + cap
    };
    pm.push_delta_yaw(delta);
    pm.ps().view_angles()[1] = P::angle_normalize360(if delta > 0.0 {
        facing - cap
    } else {
        facing + cap
    });
}

/// `PM_FootstepEvent`'s airborne arm (CoD2 0x80e5402, 1.7 0x805a356): on
/// a ladder, the footstep of the material 31 into it (the probe box, no
/// bodies, no player clip), 21 for none.
pub fn rung_footstep<P: LadderPm>(pm: &mut P) {
    let (mins, maxs) = probe_box(pm.bounds());
    let mask = pm.tracemask() & !(CONTENTS_BODY | CONTENTS_PLAYERCLIP);
    let ps = pm.ps();
    let origin = *ps.origin();
    let end = ma(origin, RUNG_REACH, *ps.ladder_vec());
    let t = pm.ladder_trace(origin, end, mins, maxs, mask);
    let mut surf = surface_material(t.surface_flags);
    if t.fraction == 1.0 || surf == 0 {
        surf = RUNG_DEFAULT_SURFACE;
    }
    pm.rung_event(surf);
}
