//! `PM_GroundTrace` and the landing (docs/research/cod2-movement.md 7, 8).

use glam::Vec3;

use super::anim::Event;
use super::fpu::{self, d};
use super::slide::jump_clear;
use super::{ENTITYNUM_NONE, Pml, Pmove, Tr, ef, ev, pm_type};

/// rodata 0x8155050: the ground trace's reach above and below the origin.
const GROUND_REACH: f32 = 0.25;
/// rodata 0x8155054: an all-solid start retries this far down.
const RETRY_DOWN: f32 = f32::from_bits(0x3a83_126f);
/// rodata 0x8155058: a rising body leaves a plane it moves off faster.
const KICK_OFF: f32 = 10.0;
/// rodata 0x815505c.
pub(crate) const MIN_WALK_NORMAL: f32 = f32::from_bits(0x3f33_3333);
/// rodata 0x8155048 and 0x815504c: the missed-ground probe off a ledge and
/// what of it counts as "almost" on the ground.
const MISSED_REACH: f32 = 64.0;
const ALMOST_FRACTION: f32 = 0.015625;

/// `CorrectAllSolid`'s 26 nudges (rodata 0x81551e0).
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

/// `PM_GroundTrace` (0x80e38a2).
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
        start.z = pm.ps.origin.z - RETRY_DOWN;
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
    if ps.pm_flags & super::pmf::LADDER == 0
        && ps.velocity.z > 0.0
        && fpu::dot(ps.velocity, t.normal) > d(KICK_OFF)
    {
        // Thrown off the plane: the jump anim, unforced, in the air.
        super::anim::event(pm, Event::takeoff(pm.cmd.forward_move), false, false);
        airborne(pm, pml);
        return;
    }
    if t.normal.z < MIN_WALK_NORMAL {
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

/// `PM_CorrectAllSolid` (0x80e34c6): the first of the nudges that starts
/// clear moves the body there and drops it a unit onto whatever is under.
fn correct_all_solid(pm: &mut Pmove, pml: &mut Pml) -> Option<Tr> {
    for n in NUDGES {
        let p = pm.ps.origin + Vec3::from_array(n);
        let t = pm.trace(p, p);
        if t.startsolid {
            continue;
        }
        pm.ps.origin = p;
        let end = Vec3::new(p.x, p.y, p.z - 1.0);
        let t = pm.trace(p, end);
        pml.ground_trace = t;
        pm.ps.origin = fpu::lerp(p, end, t.fraction);
        return Some(t);
    }
    airborne(pm, pml);
    jump_clear(pm.ps);
    None
}

/// `PM_GroundTraceMissed` (0x80e36aa): off the ground, "almost" on it when
/// a short probe (a unit while already in the air, 64 off a ledge) finds
/// something.
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
    // Off a ledge with nothing within reach: the jump anim, forced.
    if reach != 1.0 && t.fraction == 1.0 {
        super::anim::event(pm, Event::takeoff(pm.cmd.forward_move), false, true);
    }
    pm.ps.ground_entity_num = ENTITYNUM_NONE;
    pml.ground_plane = false;
    pml.walking = false;
}

/// `PM_GroundSurfaceType` (0x80e2220): `surfaceFlags` bits 20..24, 0 under
/// the no-sound flag 0x2000.
pub(crate) fn surface_type(t: &Tr) -> i32 {
    if t.surface_flags & 0x2000 != 0 {
        0
    } else {
        ((t.surface_flags & 0x1f0_0000) >> 20) as i32
    }
}

/// The landing's stun scale (0x80e30c4): 0.5 at 500 ms, 0.2 from 1500 on.
fn stun_scale(stun_ms: i32) -> f32 {
    if stun_ms < 501 {
        0.5
    } else if stun_ms < 1500 {
        (0.5 - d(f32::from_bits(0x3e99_999a)) * ((stun_ms as f32 - 500.0) / 1000.0) as f64) as f32
    } else {
        f32::from_bits(0x3e4c_cccd)
    }
}

/// `PM_CrashLand` (0x80e30c4): the fall height solved off the frame's start,
/// the damage linear between `bg_fallDamageMinHeight` and `MaxHeight`, the
/// land bob, and the step, land and pain events.
pub(crate) fn crash_land(pm: &mut Pmove, pml: &mut Pml) {
    let ps = &mut *pm.ps;
    let rules = pm.rules;
    let vz = pml.previous_velocity.z;
    let g = ps.gravity;
    let a = (0.5 * -f64::from(g)) as f32;
    let disc = (d(vz) * d(vz) - 4.0 * d(a) * (d(pml.previous_origin.z) - d(ps.origin.z))) as f32;
    if disc < 0.0 {
        return;
    }
    let root = d(disc).sqrt() as f32;
    let impact = (-(((-d(vz) - d(root)) / (d(a) + d(a))) * -f64::from(g) + d(vz))) as f32;
    let height = (d(impact) * d(impact) / (f64::from(g as f32) + f64::from(g as f32))) as f32;
    let (lo, hi) = (rules.fall_damage_min_height, rules.fall_damage_max_height);
    let surf = surface_type(&pml.ground_trace);
    let damage = if hi <= lo {
        log::warn!("bg_fallDamageMaxHeight must be greater than bg_fallDamageMinHeight");
        0
    } else if height <= lo
        || pml.ground_trace.surface_flags & 1 != 0
        || ps.pm_type > pm_type::INTERMISSION
    {
        0
    } else if hi <= height {
        100
    } else {
        ((100.0 * ((d(height) - d(lo)) / (d(hi) - d(lo)) as f32 as f64)) as i32).clamp(0, 100)
    };
    let bob = if height <= 12.0 {
        0
    } else {
        ((4.0 + 4.0 * ((d(height) - 12.0) / 26.0) as f32 as f64) as i32).min(24)
    };
    // The land anim rides the bob's test (0x80e30c4).
    if height > 12.0 {
        super::anim::event(pm, Event::Land, false, true);
    }
    let ps = &mut *pm.ps;
    let damp = f32::from_bits(0x3f2b_851f);
    if damage == 0 {
        if height > 4.0 {
            if height < 8.0 {
                ps.add_event(
                    if surf == 0 {
                        0
                    } else {
                        ev::FOOTSTEP_WALK + surf
                    },
                    0,
                );
            } else if height < 12.0 {
                ps.add_event(
                    if surf == 0 {
                        0
                    } else {
                        ev::FOOTSTEP_RUN + surf
                    },
                    0,
                );
            } else {
                ps.velocity = fpu::scale(ps.velocity, damp);
                ps.add_event(ev::LANDING + surf, bob);
            }
        }
    } else {
        if damage < 100 && pml.ground_trace.surface_flags & 2 == 0 {
            let stun = (damage * 35 + 500).min(2000);
            ps.pm_time = stun;
            ps.pm_flags |= super::pmf::TIME_LAND;
            ps.velocity = fpu::scale(ps.velocity, stun_scale(stun));
        } else {
            ps.velocity = fpu::scale(ps.velocity, damp);
        }
        ps.add_event(ev::LANDING_PAIN + surf, damage);
    }
}
