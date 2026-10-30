//! `PM_Footsteps` (`cod4_lnxded` 0x805a516): `xyspeed`, the bob cycle and
//! the footstep events (docs/research/cod4-pmove.md 9), and its anim
//! script calls ([`super::anim`]).

use super::anim::Gait;
use super::fpu::{self, d};
use super::{ENTITYNUM_NONE, Pml, Pmove, Stance, ef, ev, pm_type, pmf};

/// The bob rates (rodata 0x820e220, read at 0x805ac99): the cycle's rate per
/// unit of the speed asked for, at `2 * stance code + walking` (the codes
/// of [`stance_code`], backwards three on).
const BOB_RATES: [f32; 12] = [
    f32::from_bits(0x3eab_851f), // standing: run, walk
    f32::from_bits(0x3e9c_28f6),
    0.25, // prone
    f32::from_bits(0x3e75_c28f),
    f32::from_bits(0x3eae_147b), // crouched
    f32::from_bits(0x3ea1_47ae),
    f32::from_bits(0x3eb8_51ec), // backwards: standing, prone, crouched
    f32::from_bits(0x3ea6_6666),
    0.25,
    f32::from_bits(0x3e75_c28f),
    f32::from_bits(0x3eae_147b),
    f32::from_bits(0x3ea1_47ae),
];
/// The sighted walk's share of the speed (0x820969c).
const WALK_SCALE: f32 = f32::from_bits(0x3ecc_cccd);
/// The climb's cycle: vz over 95.25 (0x820def8) times 0.45 (0x820defc); in
/// the sighted walk over 38.1 (0x820def0, a float near 95.25 * 0.4) times
/// 0.35 (0x820def4).
const CLIMB_SPEED: f32 = 95.25;
const CLIMB_WALK_SPEED: f32 = f32::from_bits(0x4218_6667);
const CLIMB_RATE: f32 = f32::from_bits(0x3ee6_6666);
const CLIMB_WALK_RATE: f32 = f32::from_bits(0x3eb3_3333);
/// No bob within this long of a jump (0x805a9d9).
const CLIMB_AFTER_JUMP_MS: i32 = 300;

fn stance_code(s: Stance) -> usize {
    match s {
        Stance::Stand => 0,
        Stance::Prone => 1,
        Stance::Crouch => 2,
    }
}

/// `PM_ShouldMakeFootsteps`: standing, not in the sighted walk, at
/// `player_footstepsThreshhold` or over.
pub(crate) fn should_make_footsteps(pm: &Pmove) -> bool {
    pm.ps.effective_stance() == Stance::Stand
        && pm.ps.pm_flags & pmf::SIGHT_WALK == 0
        && pm.rules.footsteps_threshold <= pm.xyspeed
}

/// `PM_FootstepEvent` (0x805a2a8): when the cycle's step from `old` to
/// `new` crosses a quarter (bit 7 of `cycle + 64` flips) and `make` asks
/// for it, a footstep with the surface in its parm. On the ground the
/// event is 0 on a silent surface, else prone, walk (sighted or leaning),
/// sprint or run, in that order of precedence; off it only a climber
/// steps, on the rung.
pub(crate) fn footstep(pm: &mut Pmove, pml: &Pml, old: i32, new: i32, make: bool) {
    let quarter = |cycle: i32| (cycle + 64) & 0x80;
    if !make || quarter(old) == quarter(new) {
        return;
    }
    let ps = &*pm.ps;
    if ps.ground_entity_num == ENTITYNUM_NONE {
        if ps.pm_flags & pmf::LADDER != 0 {
            super::ladder::rung_footstep(pm);
        }
        return;
    }
    let surface = super::ground::surface_type(&pml.ground_trace);
    let event = match () {
        _ if surface == 0 => 0,
        _ if ps.pm_flags & pmf::PRONE != 0 => ev::FOOTSTEP_PRONE,
        _ if ps.pm_flags & pmf::SIGHT_WALK != 0 || ps.leanf != 0.0 => ev::FOOTSTEP_WALK,
        _ if ps.is_sprinting() => ev::FOOTSTEP_SPRINT,
        _ => ev::FOOTSTEP_RUN,
    };
    pm.ps.add_event(event, surface);
}

/// `PM_GetMaxSpeed`, inlined at 0x805a727: the speed the keys, the gait,
/// the weapon and the stance ask for, which the bob's rate is measured
/// against. A side key takes three quarters (0x820a328) of
/// `player_strafeSpeedScale`'s cut, a diagonal half that, a back key
/// `player_backSpeedScale` (halved on a diagonal); the sighted walk 0.4,
/// else the sprint `player_sprintSpeedScale`; the weapon its
/// `moveSpeedScale`, or its `adsMoveSpeedScale` while sighted or with
/// none; then the stance ([`super::moves::stance_speed`]). The value
/// stays on the x87 stack but where a call spills it: before the weapon
/// def's lookup (0x805acc1) and the stance's (0x805a790).
fn bob_speed(pm: &Pmove, walking: bool, sprinting: bool) -> f64 {
    let ps = &*pm.ps;
    let rules = &pm.rules;
    let (fm, rm) = (pm.cmd.forward_move, pm.cmd.right_move);
    let strafe = (d(rules.strafe_speed_scale) - 1.0) * 0.75 + 1.0;
    let back = d(rules.back_speed_scale);
    let side = (rm != 0).then_some(if fm != 0 {
        (strafe + 1.0) * 0.5
    } else {
        strafe
    });
    let reverse = (fm < 0).then_some(if rm != 0 { (back + 1.0) * 0.5 } else { back });
    let gait = if walking {
        Some(d(WALK_SCALE))
    } else {
        sprinting.then(|| d(rules.sprint_speed_scale))
    };
    let w = pm.weapon;
    let weapon = match ps.weapon {
        0 => None,
        _ if w.move_speed_scale > 0.0 && ps.pm_flags & pmf::SIGHT_WALK == 0 => {
            Some(d(w.move_speed_scale))
        }
        _ => (w.ads_move_speed_scale > 0.0).then(|| d(w.ads_move_speed_scale)),
    };
    let asked = [side, reverse, gait]
        .into_iter()
        .flatten()
        .fold(f64::from(ps.speed), |s, k| k * s);
    let armed = match weapon {
        Some(k) => k * d(asked as f32),
        None => asked,
    };
    d(super::moves::stance_speed(ps, pm.cmd.server_time)) * d(armed as f32)
}

/// Runs the cycle `msec` at `rate` and steps on it: the sum is stored as a
/// float, then truncated to a byte (0x805a7cd, 0x805aa55).
fn advance_bob(pm: &mut Pmove, pml: &Pml, rate: f64, make: bool) {
    let old = pm.ps.bob_cycle;
    let new = (rate * f64::from(pml.msec) + f64::from(old)) as f32 as i32 & 0xff;
    pm.ps.bob_cycle = new;
    footstep(pm, pml, old, new, make);
}

/// `PM_Footsteps`: `xyspeed`, then a gunner's idle, a climber's bob, a
/// body at rest or with no key held, or the walk: the cycle at `xyspeed`
/// over [`bob_speed`] times [`BOB_RATES`] (a standing sprint's
/// `player_sprintCameraBob` instead), a footstep each quarter.
pub(crate) fn footsteps(pm: &mut Pmove, pml: &Pml) {
    if pm.ps.pm_type >= pm_type::DEAD {
        return;
    }
    let [vx, vy, _] = pm.ps.velocity.to_array().map(d);
    pm.xyspeed = fpu::sqrt(vy * vy + vx * vx);
    if pm.ps.e_flags & ef::TURRET != 0 {
        super::anim::legs(pm, Gait::Mounted);
        return;
    }
    let linked = pm.ps.pm_type == pm_type::NORMAL_LINKED;
    if pm.ps.ground_entity_num == ENTITYNUM_NONE && !linked {
        climb_bob(pm, pml);
        return;
    }
    if linked || pm.xyspeed < pm.rules.move_threshold {
        if pm.xyspeed < 1.0 {
            pm.ps.bob_cycle = 0;
        }
        super::anim::legs(pm, Gait::Still);
        return;
    }
    if pm.cmd.forward_move == 0 && pm.cmd.right_move == 0 {
        super::anim::legs(pm, Gait::Coasting);
        return;
    }
    let ps = &*pm.ps;
    let walking = ps.pm_flags & pmf::SIGHT_WALK != 0 || ps.leanf != 0.0;
    let sprinting = ps.pm_flags & pmf::SPRINTING != 0;
    let backwards = ps.pm_flags & pmf::BACKWARDS_RUN != 0;
    let code = stance_code(ps.effective_stance()) + if backwards { 3 } else { 0 };
    super::anim::legs(
        pm,
        Gait::Keyed {
            row: code,
            walking,
            sprinting,
        },
    );
    let rate = if code == 0 && sprinting {
        pm.rules.sprint_camera_bob
    } else {
        BOB_RATES[2 * code + usize::from(walking)]
    };
    let bob = d(pm.xyspeed) / bob_speed(pm, walking, sprinting) * d(rate);
    let make = should_make_footsteps(pm);
    advance_bob(pm, pml, bob, make);
}

/// `PM_Footsteps`' climb (0x805a9b8): on the rungs, from 300 ms after a
/// jump, the cycle runs off vz with a rung footstep each quarter.
fn climb_bob(pm: &mut Pmove, pml: &Pml) {
    let ps = &*pm.ps;
    if ps.pm_flags & pmf::LADDER == 0 || pm.cmd.server_time - ps.jump_time < CLIMB_AFTER_JUMP_MS {
        return;
    }
    super::anim::legs(pm, Gait::Climbing);
    let ps = &*pm.ps;
    let vz = d(ps.velocity.z);
    let bob = if ps.pm_flags & pmf::SIGHT_WALK != 0 || ps.leanf != 0.0 {
        vz / d(CLIMB_WALK_SPEED) * d(CLIMB_WALK_RATE)
    } else {
        vz / d(CLIMB_SPEED) * d(CLIMB_RATE)
    };
    // Spilled across the anim call (0x805aa3a).
    advance_bob(pm, pml, d(bob as f32), true);
}
