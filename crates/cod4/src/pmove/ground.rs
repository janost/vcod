//! `PM_GroundTrace` and the landing (docs/research/cod4-pmove.md 4). 1.7
//! stands on a contact the trace calls walkable, not on a normal's z.

use glam::Vec3;

use super::fpu::{self, d};
use super::slide::jump_clear;
use super::{ENTITYNUM_NONE, Pml, Pmove, Tr, ef, ev, pm_type, pmf};

/// The ground trace's reach above and below the origin.
const GROUND_REACH: f32 = 0.25;
/// An all-solid start retries this far down (`EQUAL_EPSILON`).
const RETRY_DOWN: f32 = f32::from_bits(0x3a83_126f);
/// A rising body leaves a plane it moves off faster.
const KICK_OFF: f32 = 10.0;
/// The missed-ground probe off a ledge and what of it counts as "almost"
/// on the ground.
const MISSED_REACH: f32 = 64.0;
const ALMOST_FRACTION: f32 = 0.015625;
/// The landing's speed damp.
const LAND_DAMP: f32 = f32::from_bits(0x3f2b_851f);

/// `CorrectSolidDeltas`: `PM_CorrectAllSolid`'s 26 nudges.
const NUDGES: [[f32; 3]; 26] = [
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

fn airborne(pm: &mut Pmove, pml: &mut Pml) {
    pm.ps.ground_entity_num = ENTITYNUM_NONE;
    pml.ground_plane = false;
    pml.almost_ground_plane = false;
    pml.walking = false;
}

/// `PM_GroundTrace`.
pub(crate) fn ground_trace(pm: &mut Pmove, pml: &mut Pml) {
    let o = pm.ps.origin;
    let (mut start, end) = if pm.ps.e_flags & ef::TURRET == 0 {
        (
            Vec3::new(o.x, o.y, o.z + GROUND_REACH),
            Vec3::new(o.x, o.y, o.z - GROUND_REACH),
        )
    } else {
        (o, Vec3::new(o.x, o.y, o.z - 1.0))
    };
    let mut t = pm.trace(start, end);
    pml.ground_trace = t;
    if t.allsolid {
        match correct_all_solid(pm, pml) {
            Some(fixed) => t = fixed,
            None => return,
        }
    }
    if t.startsolid {
        start.z = (d(pm.ps.origin.z) - d(RETRY_DOWN)) as f32;
        t = pm.trace(start, end);
        if t.startsolid {
            airborne(pm, pml);
            return;
        }
        pml.ground_trace = t;
    }
    if t.fraction == 1.0 {
        ground_trace_missed(pm, pml);
        return;
    }
    let ps = &mut *pm.ps;
    if ps.pm_flags & pmf::LADDER == 0
        && ps.velocity.z > 0.0
        && fpu::dot(ps.velocity, t.normal) > d(KICK_OFF)
    {
        // Thrown off the plane.
        airborne(pm, pml);
        return;
    }
    if !t.walkable {
        // Too steep to stand on: a plane to slide along.
        ps.ground_entity_num = ENTITYNUM_NONE;
        pml.ground_plane = true;
        pml.almost_ground_plane = true;
        pml.walking = false;
        jump_clear(ps);
        return;
    }
    pml.ground_plane = true;
    pml.almost_ground_plane = true;
    pml.walking = true;
    if pm.ps.ground_entity_num == ENTITYNUM_NONE {
        crash_land(pm, pml);
    }
    pm.ps.ground_entity_num = (t.entity_num & 0xffff) as i32;
    pm.add_touch(t.entity_num & 0xffff);
}

/// `PM_CorrectAllSolid`: the first of the nudges that starts clear moves
/// the body there and drops it 1.25 onto whatever is under.
fn correct_all_solid(pm: &mut Pmove, pml: &mut Pml) -> Option<Tr> {
    for n in NUDGES {
        let p = pm.ps.origin + Vec3::from_array(n);
        let t = pm.trace(p, p);
        if t.startsolid {
            continue;
        }
        pm.ps.origin = p;
        let end = Vec3::new(p.x, p.y, (d(p.z) - 1.0 - 0.25) as f32);
        let t = pm.trace(p, end);
        pml.ground_trace = t;
        pm.ps.origin = fpu::lerp(p, end, t.fraction);
        return Some(t);
    }
    airborne(pm, pml);
    jump_clear(pm.ps);
    None
}

/// `PM_GroundTraceMissed`: off the ground, "almost" on it when a short
/// probe (a unit while already in the air, 64 off a ledge) finds something.
fn ground_trace_missed(pm: &mut Pmove, pml: &mut Pml) {
    let o = pm.ps.origin;
    let reach = if pm.ps.ground_entity_num == ENTITYNUM_NONE {
        1.0
    } else {
        MISSED_REACH
    };
    let end = Vec3::new(o.x, o.y, o.z - reach);
    let t = pm.raw_trace(o, end, pm.mins, pm.maxs, pm.tracemask);
    pml.almost_ground_plane = if reach == 1.0 {
        t.fraction != 1.0
    } else {
        t.fraction != 1.0 && t.fraction < ALMOST_FRACTION
    };
    // Off the ground with nothing 64 under: the takeoff's anim (the
    // server's step alone traces for it).
    if pm.anim.is_some() && pm.ps.ground_entity_num != ENTITYNUM_NONE {
        let low = Vec3::new(o.x, o.y, o.z - 64.0);
        if pm
            .raw_trace(o, low, pm.mins, pm.maxs, pm.tracemask)
            .fraction
            == 1.0
        {
            let e = if pm.cmd.forward_move < 0 {
                super::anim::Event::JumpBk
            } else {
                super::anim::Event::Jump
            };
            super::anim::event(pm, e, false, true);
        }
    }
    pm.ps.ground_entity_num = ENTITYNUM_NONE;
    pml.ground_plane = false;
    pml.walking = false;
}

/// `PM_GroundSurfaceType`: `surfaceFlags` bits 20..24, 0 under the
/// no-sound flag 0x2000.
pub(crate) fn surface_type(t: &Tr) -> u32 {
    if t.surface_flags & 0x2000 != 0 {
        0
    } else {
        (t.surface_flags & 0x1f0_0000) >> 20
    }
}

/// The soft landings, lowest first: under each height the event that
/// plays, with the surface in its parm (4, 8 and 12 at 0x8209660,
/// 0x820aac4, 0x820dee0).
const SOFT_LANDINGS: [(f64, u32); 2] = [(8.0, ev::FOOTSTEP_WALK), (12.0, ev::FOOTSTEP_RUN)];
const SILENT_FALL: f64 = 4.0;
const HARD_FALL: f64 = 12.0;
/// The view's dip on a hard landing: 4 at 12 high, 4 more per 26
/// (0x820df14), at most 24.
const DIP_PER: f64 = 26.0;
const DIP_MAX: i32 = 24;

/// The speed a hurt landing keeps through its stun: half up to 500 ms, a
/// fifth (0x8209690) from 1500, losing 0.3 per second between, all on the
/// x87 stack (0x805bca4).
fn stun_keep(stun_ms: i32) -> f64 {
    match stun_ms {
        ..=500 => 0.5,
        1500.. => d(f32::from_bits(0x3e4c_cccd)),
        t => f64::from(t - 500) / 1000.0 * d(-f32::from_bits(0x3e99_999a)) + 0.5,
    }
}

/// `PM_CrashLand`, inlined in `PM_GroundTrace` at 0x805b7bd: the height
/// fallen, read back off this frame's start z and vz under the body's
/// gravity (only the impact time is stored; the rest stays on the x87
/// stack), then what the landing does with it.
///
/// - Hurt: the share of the way from `bg_fallDamageMinHeight` to
///   `MaxHeight`, as a percentage stored as a float and truncated; none
///   below the minimum, on a no-damage surface (flag 1), for the dead, or
///   when the minimum is not under the maximum. A fatal fall (100) or a
///   slick floor damps the speed to 0.67 (0x820df1c); anything less stuns
///   for 35 ms a point over 500, capped at 2000, and slows by
///   [`stun_keep`]. `EV_LANDING_PAIN_*` carries the damage.
/// - Unhurt: silent to 4 high, a walk or run footstep below 8 and 12, a
///   damped `EV_LANDING_*` with the view's dip above.
/// - Over 12 the land anim plays, hurt or not.
pub(crate) fn crash_land(pm: &mut Pmove, pml: &mut Pml) {
    let Some(height) = fall_height(pm.ps, pml) else {
        return;
    };
    let ps = &*pm.ps;
    let rules = &pm.rules;
    let (lo, hi) = (
        d(rules.fall_damage_min_height),
        d(rules.fall_damage_max_height),
    );
    let spared = lo >= hi
        || height <= lo
        || pml.ground_trace.surface_flags & 1 != 0
        || ps.pm_type >= pm_type::DEAD;
    let hurt = match () {
        _ if spared => 0,
        _ if height >= hi => 100,
        _ => (((height - lo) / (hi - lo) * 100.0) as f32 as i32).clamp(0, 100),
    };
    let hard = height > HARD_FALL;
    let dip = if hard {
        (((height - HARD_FALL) / DIP_PER * 4.0 + 4.0) as f32 as i32).min(DIP_MAX)
    } else {
        0
    };
    if hard {
        super::anim::event(pm, super::anim::Event::Land, false, true);
    }

    let ps = &mut *pm.ps;
    let surf = surface_type(&pml.ground_trace);
    let on_surface = |event: u32| if surf == 0 { 0 } else { event };
    let slick = pml.ground_trace.surface_flags & 2 != 0;
    if hurt != 0 {
        if hurt == 100 || slick {
            ps.velocity = fpu::scale(ps.velocity, LAND_DAMP);
        } else {
            let stun = (hurt * 35 + 500).min(2000);
            ps.pm_flags |= pmf::TIME_LAND;
            ps.pm_time = stun;
            ps.velocity = fpu::scale_kept(ps.velocity, stun_keep(stun));
        }
        ps.add_event(on_surface(ev::LANDING_PAIN_FIRST + surf), hurt as u32);
        return;
    }
    if height <= SILENT_FALL {
        return;
    }
    match SOFT_LANDINGS.iter().find(|&&(under, _)| height < under) {
        Some(&(_, footstep)) => ps.add_event(on_surface(footstep), surf),
        None => {
            ps.velocity = fpu::scale(ps.velocity, LAND_DAMP);
            ps.add_event(on_surface(ev::LANDING_FIRST + surf), dip as u32);
        }
    }
}

/// How high the body fell this frame: solve `z0 + v t + g t^2 / 2` for
/// the moment it reached the ground, take the speed there and turn it
/// back into a height (`v^2 / 2g`). None when the frame's start cannot
/// reach the ground (no real root).
fn fall_height(ps: &super::PlayerState, pml: &Pml) -> Option<f64> {
    let g = f64::from(ps.gravity);
    let dropped = d(pml.previous_origin.z) - d(ps.origin.z);
    let v0 = d(pml.previous_velocity.z);
    let half_accel = -g * 0.5;
    let disc = v0 * v0 + -4.0 * half_accel * dropped;
    if disc < 0.0 {
        return None;
    }
    let t = ((-v0 - disc.sqrt()) / (half_accel + half_accel)) as f32;
    let impact = v0 - g * d(t);
    Some(impact * impact / (g + g))
}
