//! `PM_UpdateHoldBreath`: the breath's sway scale tracks its target every
//! step (cod4-pmove.md 15). With the level's weapon defs a sighted scope
//! (`overlayReticle`, cod4-pmove.md 16) holds the breath on the 0x2000 key
//! for `player_breath_hold_time`, `perk_extraBreath` more under
//! `specialty_holdbreath` (perk 0x10); no capture holds it yet.

use super::{Pml, Pmove, ef, pm_type, pmf};

/// `weapFlags` 4: the breath held.
const WEAPFLAG_HOLDING_BREATH: u32 = 4;
/// `usercmd.buttons` 0x2000, the breath key.
const HOLD_BREATH_KEY: u32 = 0x2000;
/// `perk_extraBreath`'s 1.7 default, 5 s (`tools/re/cod4_dvars.py`,
/// VERIFIED).
const PERK_EXTRA_BREATH: f32 = 5.0;

/// Seconds of a breath dvar as whole ms: the product stored as a float,
/// then truncated (`cod4_lnxded` 0x8062e66, 0x8062e91, 0x8062ef7).
fn ms(seconds: f32) -> i32 {
    (f64::from(seconds) * 1000.0) as f32 as i32
}

/// The breath's step, run by `PM_Weapon` and on the arms that skip it
/// (cod4-combat.md 2). Only a live player off a turret (`eFlags` 0x300)
/// past his respawn cmd breathes. With no hold time, the perk's included,
/// the breath never holds and the sway sits at 1. Otherwise a scope (not an
/// item) all the way into its sight holds on the breath key once the
/// timer has run back to 0; the hold spends the timer up to the hold time
/// (the perk's 5 s more), past which the body gasps: the timer jumps to
/// hold plus `player_breath_gasp_time` and runs back down. The sway eases
/// toward 0 held, else toward `player_breath_gasp_scale` by what the
/// timer has left, both scaled by `fWeaponPosFrac`. INFERRED; no capture
/// holds the breath (`pmove::tests`' `a_sighted_scope_holds_its_breath_until_it_gasps`
/// pins it).
pub(super) fn update(pm: &mut Pmove, pml: &Pml) {
    let r = pm.rules;
    let msec = pml.msec;
    // The def is the view weapon's: the offhand under `weapFlags` 2
    // (`cod4_lnxded` 0x8062e42, else `ps.weapon` at 0x8062f24).
    let scope = pm
        .defs
        .and_then(|d| d.def(super::aim::view_weapon(pm.ps)))
        .is_some_and(|d| d.overlay_reticle != 0 && d.weap_class != super::weapon::WEAPCLASS_ITEM);
    let key = pm.cmd.buttons & HOLD_BREATH_KEY != 0;
    let ps = &mut *pm.ps;
    let breathing = ps.pm_type < pm_type::DEAD
        && ps.pm_flags & pmf::RESPAWNED == 0
        && ps.e_flags & ef::TURRET == 0;
    if !breathing {
        return;
    }
    // 1.7 adds the perk's time before it tests the hold (0x8062e9d, the
    // `jle` at 0x8062ea1): with the perk a hold time of 0 still holds.
    let extra = if ps.perks & 0x10 != 0 {
        ms(PERK_EXTRA_BREATH)
    } else {
        0
    };
    let hold = ms(r.breath_hold_time) + extra;
    if hold <= 0 {
        ps.weap_flags &= !WEAPFLAG_HOLDING_BREATH;
        (ps.hold_breath_scale, ps.hold_breath_timer) = (1.0, 0);
        return;
    }
    let gasp = ms(r.breath_gasp_time);
    let was_holding = ps.weap_flags & WEAPFLAG_HOLDING_BREATH != 0;
    let wants = key && scope && ps.weapon_pos_frac == 1.0;
    let mut holding = wants && (was_holding || ps.hold_breath_timer == 0);
    let mut timer = (ps.hold_breath_timer + if holding { msec } else { -msec }).max(0);
    if holding && timer > hold {
        holding = false;
        timer = hold + gasp;
    }
    ps.hold_breath_timer = timer;
    if holding {
        ps.weap_flags |= WEAPFLAG_HOLDING_BREATH;
    } else {
        ps.weap_flags &= !WEAPFLAG_HOLDING_BREATH;
    }
    let (sway, rate) = if holding {
        (0.0, r.breath_hold_lerp)
    } else {
        let left = f64::from(timer) / f64::from(gasp + hold);
        let gasping = (f64::from(r.breath_gasp_scale) - 1.0) * left + 1.0;
        (gasping, r.breath_gasp_lerp)
    };
    let goal = (sway - 1.0) * f64::from(ps.weapon_pos_frac) + 1.0;
    ps.hold_breath_scale = diff_track(goal as f32, ps.hold_breath_scale, rate, pml.frametime);
}

/// `DiffTrack`: `cur` a step of `rate * gap * dt` toward `tgt`, or `tgt`
/// itself once within `EQUAL_EPSILON` or where the step would pass it.
/// The gap and the step stay on the x87 stack; the sum is stored. The
/// walk capture's spawn reads 0.519216, two steps (66 and 34 ms) at rate
/// 6 from 0 (VERIFIED live).
pub(super) fn diff_track(tgt: f32, cur: f32, rate: f32, dt: f32) -> f32 {
    let gap = f64::from(tgt) - f64::from(cur);
    let step = f64::from(rate) * gap * f64::from(dt);
    if gap.abs() > 0.001 && step.abs() <= gap.abs() {
        (f64::from(cur) + step) as f32
    } else {
        tgt
    }
}

#[cfg(test)]
mod tests {
    use super::diff_track;

    #[test]
    fn the_spawn_think_reads_the_captured_scale() {
        let dt = |ms: i32| (f64::from(super::super::MSEC_TO_SEC) * f64::from(ms)) as f32;
        let a = diff_track(1.0, 0.0, 6.0, dt(66));
        assert_eq!(diff_track(1.0, a, 6.0, dt(34)), 0.519216);
    }
}
