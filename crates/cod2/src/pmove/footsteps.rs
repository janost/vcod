//! `PM_Footsteps` (0x80e572c): `xyspeed`, the bob cycle and the footstep
//! events (docs/research/cod2-movement.md 12); the anim script calls it
//! makes are [`super::anim::footsteps`].

use super::fpu::{self, d};
use super::{ENTITYNUM_NONE, Pml, Pmove, Stance, ef, ev, pm_type, pmf};

/// rodata 0x81550c8: the sighted or leaning scale on the bob's speed.
const SIGHTED: f32 = f32::from_bits(0x3ecc_cccd);
/// rodata 0x81550cc, 0x81550d0: the ladder bob's rate, sighted first, per
/// 95.25 units/s of vz (an immediate).
const LADDER_RATE: [f32; 2] = [f32::from_bits(0x3eb3_3333), f32::from_bits(0x3ee6_6666)];
const LADDER_SPEED: f64 = 95.25;
/// rodata 0x81550d4 and 0x81550d8: the strafe dvar's weight, the diagonal's
/// half.
const STRAFE_WEIGHT: f32 = 0.75;
const HALF: f32 = 0.5;
/// rodata 0x81550dc and 0x81550e0: prone's and crouch's share of the speed.
const PRONE_SPEED: f32 = f32::from_bits(0x3e19_999a);
const CROUCH_SPEED: f32 = f32::from_bits(0x3f26_6666);
/// rodata 0x81550e4..0x8155100: the bob's rate per stance, sighted first.
const RATE_PRONE: [f32; 2] = [f32::from_bits(0x3e75_c28f), 0.25];
const RATE_CROUCH: [f32; 2] = [f32::from_bits(0x3ea1_47ae), f32::from_bits(0x3eae_147b)];
const RATE_BACK: [f32; 2] = [f32::from_bits(0x3ea6_6666), f32::from_bits(0x3eb8_51ec)];
const RATE_STAND: [f32; 2] = [f32::from_bits(0x3e9c_28f6), f32::from_bits(0x3eab_851f)];

/// `PM_GetEffectiveStance`'s number (0x80e1532): prone 1, crouch 2, the
/// `pm_flags` stance bits' values.
fn stance_code(s: Stance) -> u32 {
    match s {
        Stance::Stand => 0,
        Stance::Prone => 1,
        Stance::Crouch => 2,
    }
}

/// `PM_ShouldMakeFootsteps` (0x80e55ca): standing, not in the sighted walk,
/// at `player_footstepsThreshhold` or over.
pub(crate) fn should_make_footsteps(pm: &Pmove) -> bool {
    pm.ps.effective_stance() == Stance::Stand
        && pm.ps.pm_flags & pmf::SIGHT_WALK == 0
        && pm.rules.footsteps_threshold <= pm.xyspeed
}

/// The footstep event for the ground (0x80e2fb0): none on a silent surface,
/// else the surface past the prone, the walk (sighted or leaning) or the
/// run block.
fn footstep_event(pm: &Pmove, pml: &Pml) -> i32 {
    let surf = super::ground::surface_type(&pml.ground_trace);
    if surf == 0 {
        0
    } else if pm.ps.pm_flags & pmf::PRONE != 0 {
        ev::FOOTSTEP_PRONE + surf
    } else if pm.ps.pm_flags & pmf::SIGHT_WALK != 0 || pm.ps.leanf != 0.0 {
        ev::FOOTSTEP_WALK + surf
    } else {
        ev::FOOTSTEP_RUN + surf
    }
}

/// `PM_FootstepEvent` (0x80e5402): a footstep when the cycle crossed a
/// quarter (bit 7 of `cycle + 64` changed) and when asked: off the ground
/// only on a ladder, the rung's.
pub(crate) fn footstep(pm: &mut Pmove, pml: &Pml, old: i32, new: i32, make: bool) {
    if ((old + 64) ^ (new + 64)) & 0x80 == 0 || !make {
        return;
    }
    if pm.ps.ground_entity_num == ENTITYNUM_NONE {
        if pm.ps.pm_flags & pmf::LADDER != 0 {
            super::ladder::rung_footstep(pm);
        }
        return;
    }
    let e = footstep_event(pm, pml);
    pm.ps.add_event(e, 0);
}

/// `PM_Footsteps`: `pm->xyspeed`, then the bob cycle at a rate off the
/// stance and the keys, and a footstep each quarter.
pub(crate) fn footsteps(pm: &mut Pmove, pml: &Pml) {
    if pm.ps.pm_type >= pm_type::DEAD {
        return;
    }
    let (vx, vy) = (d(pm.ps.velocity.x), d(pm.ps.velocity.y));
    pm.xyspeed = fpu::sqrt(vy * vy + vx * vx);
    super::anim::footsteps(pm);
    if pm.ps.e_flags & ef::TURRET != 0 {
        return;
    }
    let stance = pm.ps.effective_stance();
    if pm.ps.ground_entity_num == ENTITYNUM_NONE && pm.ps.pm_type != pm_type::NORMAL_LINKED {
        // On a ladder the cycle runs off vz, its footsteps the rungs'.
        if pm.ps.pm_flags & pmf::LADDER != 0 {
            if pm.cmd.server_time - pm.ps.jump_time < 300 {
                return;
            }
            let vz = d(pm.ps.velocity.z);
            let rate = if pm.ps.pm_flags & pmf::SIGHT_WALK != 0 || pm.ps.leanf != 0.0 {
                d(LADDER_RATE[0]) * (vz / (d(SIGHTED) * LADDER_SPEED))
            } else {
                d(LADDER_RATE[1]) * (vz / LADDER_SPEED)
            } as f32;
            let old = pm.ps.bob_cycle;
            let new = (f64::from(pml.msec) * d(rate) + f64::from(old)) as i64 as i32 & 0xff;
            pm.ps.bob_cycle = new;
            footstep(pm, pml, old, new, true);
        }
        if stance_code(stance) == pm.ps.pm_flags & (pmf::PRONE | pmf::DUCKED) {
            return;
        }
    }
    let ps = &*pm.ps;
    let sighted = ps.pm_flags & pmf::SIGHT_WALK != 0 || ps.leanf != 0.0;
    if pm.xyspeed < pm.rules.move_threshold || ps.pm_type == pm_type::NORMAL_LINKED {
        if pm.xyspeed < 1.0 {
            pm.ps.bob_cycle = 0;
        }
        return;
    }
    // The speed the keys ask for, which the bob's rate divides.
    let (fm, rm) = (pm.cmd.forward_move, pm.cmd.right_move);
    let strafe = d(STRAFE_WEIGHT) * (d(pm.rules.strafe_speed_scale) - 1.0) + 1.0;
    let back = d(pm.rules.back_speed_scale);
    let mut base = ps.speed as f32;
    if fm == 0 {
        if rm != 0 {
            base = (d(base) * strafe) as f32;
        }
    } else if rm == 0 {
        if fm < 0 {
            base = (d(base) * back) as f32;
        }
    } else {
        base = (d(base) * d(HALF) * (strafe + 1.0)) as f32;
        if fm < 0 {
            base = (d(base) * d(HALF) * (back + 1.0)) as f32;
        }
    }
    if sighted {
        base = (d(SIGHTED) * d(base)) as f32;
    }
    let t = pm.cmd.server_time;
    let down = super::moves::lerp_frac(ps, 40, 11, t);
    let up = super::moves::lerp_frac(ps, 11, 40, t);
    let blend = |a: f32, b: f32, f: f32| d(a) * (1.0 - d(f)) + d(b) * d(f);
    if down != 0.0 {
        base = (d(base) * blend(CROUCH_SPEED, PRONE_SPEED, down)) as f32;
    } else if up != 0.0 {
        base = (d(base) * blend(PRONE_SPEED, CROUCH_SPEED, up)) as f32;
    } else if stance == Stance::Prone {
        base = (d(PRONE_SPEED) * d(base)) as f32;
    } else if stance == Stance::Crouch {
        base = (d(CROUCH_SPEED) * d(base)) as f32;
    }
    let rates = match stance {
        Stance::Prone => RATE_PRONE,
        Stance::Crouch => RATE_CROUCH,
        Stance::Stand if ps.pm_flags & pmf::BACKWARDS_RUN != 0 => RATE_BACK,
        Stance::Stand => RATE_STAND,
    };
    let rate = (d(rates[usize::from(!sighted)]) * (d(pm.xyspeed) / d(base))) as f32;
    let make = should_make_footsteps(pm);
    let old = pm.ps.bob_cycle;
    // `fistp` under control word 0xc00: truncated.
    let new = (f64::from(pml.msec) * d(rate) + f64::from(old)) as i64 as i32 & 0xff;
    pm.ps.bob_cycle = new;
    // No key held: the cycle runs silent (the anim alone differs past 120).
    if fm == 0 && rm == 0 {
        return;
    }
    footstep(pm, pml, old, new, make);
}
