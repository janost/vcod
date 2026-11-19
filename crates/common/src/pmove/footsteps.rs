//! `PM_Footsteps`: the bob cycle and the step events.

use super::frame::msec;
use super::stance::stance_speed_scale;
use super::state::{MoveRules, PlayerState, PmEvent, PmInput, Stance};
use super::uo::SCALE_SPRINT;
use super::{HALF_WIDTH, SCALE_BACK, SCALE_STRAFE, SCALE_WALK};
use crate::collision::MASK_PLAYERSOLID;
use crate::movetrace::MoveWorld;
/// Wire `EV_*` group bases; the sound surface index is added to the base.
use crate::net::event_ids::{
    EV_FOOTSTEP_PRONE_BASE, EV_FOOTSTEP_RUN_BASE, EV_FOOTSTEP_WALK_BASE, EV_JUMP_BASE,
};
use glam::Vec3;

// Footstep cadence, decoded from PM_ShouldMakeFootsteps @0x322c8 and
// PM_FootstepEvent @0x31fe4 (game.mp.i386.so); facts live in
// docs/research/cod11-sound-system.md, "Footstep and landing cadence".
/// Ladder climb steps stay quiet this long after a jump
/// (`cmd.serverTime - ps->jumpTime <= 0x12b`, @0x323b9-0x323c8).
const LADDER_STEP_QUIET_MS: f32 = 299.0;
/// Water-material footstep ids (base + 20), fixed regardless of ground surface.
const EV_FOOTSTEP_PRONE_WATER: i32 = 67;
const EV_FOOTSTEP_WALK_WATER: i32 = 44;
pub(super) const EV_FOOTSTEP_RUN_WATER: i32 = 21;
/// Material surfaceparm that silences footsteps and landings
/// (`sf & 0x2000`, PM_FootstepEvent @0x32167, PM_CrashLand @0x30108).
pub(super) const SURF_NO_SOUND: u32 = 0x2000;
/// Ladder-step interval divisor and rate keys (rodata 0x70c10-18).
const LADDER_STEP_DIVISOR: f32 = 95.25;
const LADDER_STEP_K_WALK: f32 = 0.35;
const LADDER_STEP_K_RUN: f32 = 0.45;
/// Ladder probe: box shrink per side, z floor, reach along -ladder_normal
/// (rodata 0x70c04/08/0c). A missed trace or material 0 defaults to metal (13).
const LADDER_STEP_PROBE: f32 = 31.0;
pub(super) const DEFAULT_MATERIAL: i32 = 13;

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

/// Retail footstep cadence (`PM_Footsteps` 0x322c8, 1.5 0x24ae8). The bob
/// cycle ticks by `msec * rate * speed / full speed`
/// ([`footstep_speed`]); a step event fires on each 128-tick crossing while
/// move keys are held and the gait is audible. The ladder branch runs ahead
/// of the speed gates (0x323a2 precedes the fcom at 0x3249f). The walk is
/// `pm_flags` 0x80, the ADS walk, and under 1.5 a lean too (0x24bf7); UO's
/// ground cadence leaves the lean out (`game.mp.uo` 0x297e4).
pub(super) fn footsteps(
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
pub(super) fn ground_step_event(
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

/// `PM_ShouldMakeFootsteps` (0x3221c): upright by the eye's stance and not
/// walking the sight (`pm_flags` 0x80).
pub(super) fn footsteps_audible(ps: &PlayerState) -> bool {
    ps.effective_stance() == Stance::Stand && !ps.walking
}
