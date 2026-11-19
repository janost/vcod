//! Contains routines ported from the Quake III Arena GPL source, Copyright (C) 1999-2005 Id Software, Inc.,
//! and the RTCW-MP GPL source, Copyright (C) 1999-2010 id Software LLC, a ZeniMax Media company. See NOTICE.
//!
//! `PM_CheckDuck` and `PM_ViewHeightAdjust`: the stance and the eye curves.

use super::jump::JUMP_PENALTY_MS;
use super::prone::{PRONE_CHECK_LENGTH, ProneQuery, check_prone, enter_prone};
use super::state::{MoveRules, PlayerState, PmEvent, PmInput, Stance};
use super::{
    HALF_WIDTH, HEIGHT_CROUCH, HEIGHT_STAND, SCALE_CROUCH, SCALE_PRONE, VIEW_CROUCH, VIEW_PRONE,
    VIEW_STAND, weapon,
};
use crate::collision::MASK_PLAYERSOLID;
use crate::movetrace::MoveWorld;
use crate::net::event_ids::{EV_STANCE_FORCE_CROUCH, EV_STANCE_FORCE_PRONE, EV_STANCE_FORCE_STAND};
use glam::Vec3;

/// The eye's lerp times in ms, per leg (`PM_ViewHeightAdjust` 0x309d8,
/// docs/research/cod11-mantle.md, "The eye through a stance change"): the
/// `bg_duck2prone_time`/`bg_prone2duck_time` defaults into and out of prone,
/// and the immediates of the other legs.
const VIEW_LERP_PRONE_MS: i32 = 400;
const VIEW_LERP_DIVE_MS: i32 = 200;
const VIEW_LERP_DUCK_MS: i32 = 150;
const VIEW_LERP_DIVE_DUCK_MS: i32 = 100;
const VIEW_LERP_STAND_MS: i32 = 200;

/// The eye's curve per leg, `(percent, height)` waypoints from `.data`
/// 0x7c730..0x7c8f0; the third word of each record, an origin offset, is 0
/// throughout.
const VIEW_CURVE_STAND_CROUCH: &[(i32, f32)] = &[
    (0, 60.0),
    (1, 59.5),
    (4, 58.5),
    (30, 56.0),
    (80, 44.0),
    (90, 41.5),
    (95, 40.5),
    (100, 40.0),
];
const VIEW_CURVE_CROUCH_STAND: &[(i32, f32)] = &[
    (0, 40.0),
    (5, 40.5),
    (10, 41.5),
    (20, 44.0),
    (70, 56.0),
    (96, 58.5),
    (99, 59.5),
    (100, 60.0),
];
const VIEW_CURVE_CROUCH_PRONE: &[(i32, f32)] = &[
    (0, 40.0),
    (11, 38.0),
    (22, 33.0),
    (34, 25.0),
    (45, 16.0),
    (50, 15.0),
    (55, 16.0),
    (70, 18.0),
    (90, 17.0),
    (100, 11.0),
];
const VIEW_CURVE_DIVE_PRONE: &[(i32, f32)] = &[(0, 40.0), (100, 11.0)];
const VIEW_CURVE_PRONE_CROUCH: &[(i32, f32)] = &[
    (0, 11.0),
    (5, 10.0),
    (30, 21.0),
    (50, 25.0),
    (67, 31.0),
    (83, 34.0),
    (100, 40.0),
];

/// `PM_CheckDuck`'s stance arms (0x31840-0x31b80): a prone press the body
/// does not fit in is refused, and a rise without headroom stays down. Each
/// refusal tells the client which stance to fall back to through
/// `EV_STANCE_FORCE_*`, unless the cmd's stance came from a held key
/// (docs/research/cod11-mantle.md, "Prone Blocked").
pub(super) fn update_stance(
    ps: &mut PlayerState,
    input: &PmInput,
    world: &MoveWorld,
    dt: f32,
    events: &mut Vec<PmEvent>,
) {
    let before = ps.stance;
    let (mut crouch, mut prone) = (input.crouch, input.prone);
    // UO's deployed bipod takes no stance key at all (0x27e16).
    let keys = !ps.deployed();
    // 0x318a0: a climber's stance keys are taken off the cmd, announced
    // whether held or not.
    if keys && ps.on_ladder && (crouch || prone) {
        (crouch, prone) = (false, false);
        push_event(events, EV_STANCE_FORCE_STAND);
    }
    let force = |events: &mut Vec<PmEvent>, event: i32| {
        if !input.stance_held {
            push_event(events, event);
        }
    };
    let v15 = ps.rules.v15();
    let (origin, mins) = (ps.origin, ps.mins());
    // 1.5's stance traces leave bodies out (0x23a18 and on).
    let mask = if v15 {
        MASK_PLAYERSOLID & !crate::movetrace::CONTENTS_BODY
    } else {
        MASK_PLAYERSOLID
    };
    let fits = |height: f32| {
        let maxs = Vec3::new(HALF_WIDTH, HALF_WIDTH, height);
        !world.box_trace(origin, origin, mins, maxs, mask).startsolid
    };
    if !keys {
        // The deploy holds the stance it set.
    } else if prone {
        if v15 && before == Stance::Prone {
            // A held prone key keeps 0x2 off (0x2390c).
            ps.ducked = false;
        } else if v15 && !ps.on_ground {
            // 1.5 drops an airborne press without a word (0x237d6).
        } else if before != Stance::Prone {
            let q = ProneQuery {
                origin: ps.origin,
                yaw_deg: ps.yaw.to_degrees(),
                entry: true,
                ground: ps.on_ground,
                length: PRONE_CHECK_LENGTH,
                rules: ps.rules,
            };
            match check_prone(world, q) {
                Some(body) => {
                    ps.prone_body = body;
                    ps.stance = Stance::Prone;
                    if v15 {
                        // 1.5 clears 0x2 on the way down (0x2390c).
                        ps.ducked = false;
                    }
                    if v15 {
                        enter_prone(ps, input, world);
                    }
                }
                // 0x3196b: every cmd that keeps asking is refused again.
                None => {
                    ps.prone_blocked = true;
                    ps.prone_dive = false;
                    let event = if ps.ducked {
                        EV_STANCE_FORCE_CROUCH
                    } else {
                        EV_STANCE_FORCE_STAND
                    };
                    force(events, event);
                }
            }
        }
    } else {
        // The dive flag lives as long as the prone key is held (0x319a6,
        // 0x31a13).
        ps.prone_dive = false;
        match (before, crouch) {
            (Stance::Prone, true) => {
                if fits(HEIGHT_CROUCH) {
                    ps.stance = Stance::Crouch;
                } else {
                    force(events, EV_STANCE_FORCE_PRONE);
                }
            }
            (_, true) => ps.stance = Stance::Crouch,
            // A prone player who lets go stands if it can, else crouches.
            (Stance::Prone, false) => {
                if fits(HEIGHT_STAND) {
                    ps.stance = Stance::Stand;
                } else if fits(HEIGHT_CROUCH) {
                    ps.stance = Stance::Crouch;
                } else {
                    force(events, EV_STANCE_FORCE_PRONE);
                }
            }
            (Stance::Crouch, false) => {
                if fits(HEIGHT_STAND) {
                    ps.stance = Stance::Stand;
                } else {
                    force(events, EV_STANCE_FORCE_CROUCH);
                }
            }
            (Stance::Stand, false) => {}
        }
    }
    if ps.stance != before {
        match ps.stance {
            Stance::Crouch => ps.ducked = true,
            Stance::Stand => ps.ducked = false,
            Stance::Prone => {}
        }
    }
    let msec = (dt * 1000.0).round() as i32;
    if v15 {
        step_view_target(ps);
        view_height_adjust_v15(ps, msec);
        return;
    }
    // 0x31c1d-0x31c9b: the prone box stays the crouch's until the eye is
    // down or past half its leg, `(cmd.serverTime - viewHeightLerpTime) /
    // duration` read before this cmd's adjust (0x308cc, 0 with no leg).
    ps.prone_box_crouch = ps.stance == Stance::Prone && ps.view_height_cur != VIEW_PRONE && {
        let frac = ps.view_lerp_ms.map_or(0.0, |ms| {
            (f64::from(ms.wrapping_add(msec)) / f64::from(view_lerp_duration(ps))).clamp(0.0, 1.0)
        });
        frac <= 0.5
    };
    // 1.1 lies down in the box just sized (0x31ca1).
    if ps.stance == Stance::Prone && before != Stance::Prone {
        enter_prone(ps, input, world);
    }
    // The bbox snaps; the eye eases.
    view_height_adjust(ps, msec);
}

/// 1.5's `PM_CheckDuck` tail (0x23e3c..0x24049): while no leg of the eye
/// runs, the target moves one stance toward the flags' stance. Reaching
/// prone starts the jump penalty's timer, so a body that just lay down
/// jumps low once it stands (docs/research/cod15-movement.md, "Stance").
pub(super) fn step_view_target(ps: &mut PlayerState) {
    if ps.view_lerp_ms.is_some() {
        return;
    }
    let t = ps.view_target;
    ps.view_target = if ps.stance == Stance::Prone {
        if t == VIEW_STAND {
            VIEW_CROUCH
        } else if t != VIEW_PRONE {
            ps.jump_penalty = true;
            ps.knockback_ms = JUMP_PENALTY_MS;
            prone_flip(ps);
            VIEW_PRONE
        } else {
            t
        }
    } else if t == VIEW_PRONE {
        prone_flip(ps);
        VIEW_CROUCH
    } else if ps.ducked {
        VIEW_CROUCH
    } else {
        VIEW_STAND
    };
}

/// The two arms of [`step_view_target`] that take the eye's target into or
/// out of prone mark the move and clear the torso
/// ([`PlayerState::prone_flip_clears_torso`]).
pub(super) fn prone_flip(ps: &mut PlayerState) {
    ps.prone_flip = true;
    ps.prone_flip_clears_torso = !(ps.rules == MoveRules::Uo
        && matches!(
            ps.weaponstate,
            weapon::WEAPON_DEPLOY | weapon::WEAPON_BREAKDOWN
        ));
}

/// 1.5's `PM_ViewHeightAdjust` (0x226a6) for a live player: the running leg
/// advances, and a new one starts toward the target only once none runs.
/// The target never moves mid-leg, so 1.1's turn back never happens.
pub(super) fn view_height_adjust_v15(ps: &mut PlayerState, msec: i32) {
    if let Some(ms) = ps.view_lerp_ms.as_mut() {
        *ms = ms.wrapping_add(msec);
        let pct = (ms.wrapping_mul(100) / view_lerp_duration(ps)).clamp(0, 100);
        if pct == 100 {
            ps.view_height_cur = ps.view_lerp_target;
            ps.view_lerp_ms = None;
        } else {
            ps.view_height_cur = view_curve_height(view_curve(ps), pct);
        }
        return;
    }
    if ps.view_target == ps.view_height_cur {
        return;
    }
    ps.view_lerp_ms = Some(0);
    ps.view_lerp_down = ps.view_target < ps.view_height_cur;
    ps.view_lerp_target = ps.view_target;
}

pub(super) fn push_event(events: &mut Vec<PmEvent>, event: i32) {
    events.push(PmEvent { event, parm: 0 });
}

/// Retail's `PM_ViewHeightAdjust` (0x309d8), called at the end of
/// `PM_CheckDuck`: the eye walks a curve per leg, and a leg only ever spans
/// neighbouring stances, so standing to prone is two legs through the crouch
/// height (docs/research/cod11-mantle.md, "The eye through a stance change").
fn view_height_adjust(ps: &mut PlayerState, msec: i32) {
    let target = ps.stance.view_height();
    let mut pct = 0;
    if let Some(ms) = ps.view_lerp_ms.as_mut() {
        *ms = ms.wrapping_add(msec);
        pct = (ms.wrapping_mul(100) / view_lerp_duration(ps)).clamp(0, 100);
        if pct == 100 {
            ps.view_height_cur = ps.view_lerp_target;
            ps.view_lerp_ms = None;
        } else {
            ps.view_height_cur = view_curve_height(view_curve(ps), pct);
        }
    }
    if ps.view_lerp_ms.is_some() {
        if target == ps.view_lerp_target {
            return;
        }
        let reverses = if ps.view_lerp_down {
            target > ps.view_lerp_target
        } else {
            target < ps.view_lerp_target
        };
        if !reverses {
            return;
        }
        // Turn back mid-leg: the same stretch of the other leg's clock.
        pct = 100 - pct;
        ps.view_lerp_down = !ps.view_lerp_down;
        let t = ps.view_lerp_target;
        ps.view_lerp_target = match (ps.view_lerp_down, t) {
            (true, VIEW_STAND) => VIEW_CROUCH,
            (true, VIEW_CROUCH) => VIEW_PRONE,
            (false, VIEW_PRONE) => VIEW_CROUCH,
            (false, VIEW_CROUCH) => VIEW_STAND,
            _ => t,
        };
        if pct == 100 {
            ps.view_height_cur = ps.view_lerp_target;
            ps.view_lerp_ms = None;
        } else {
            // x87 product of the int percent, the float 0.01 at rodata
            // 0x70bcc and the int duration, truncated (0x312a5-0x312c6).
            let into = f64::from(pct) * f64::from(0.01f32) * f64::from(view_lerp_duration(ps));
            ps.view_lerp_ms = Some(into as i32);
        }
        return;
    }
    if target == ps.view_height_cur {
        return;
    }
    ps.view_lerp_ms = Some(0);
    let cur = ps.view_height_cur;
    (ps.view_lerp_down, ps.view_lerp_target) = match ps.stance {
        Stance::Prone => (
            true,
            if cur > VIEW_CROUCH {
                VIEW_CROUCH
            } else {
                VIEW_PRONE
            },
        ),
        Stance::Crouch => (cur > VIEW_CROUCH, VIEW_CROUCH),
        Stance::Stand => (
            false,
            if cur < VIEW_CROUCH {
                VIEW_CROUCH
            } else {
                VIEW_STAND
            },
        ),
    };
}

/// The running leg's length (the duration pick at 0x30b46-0x30b98, the same
/// as `PM_GetViewHeightLerpTime` 0x345b8).
fn view_lerp_duration(ps: &PlayerState) -> i32 {
    // 1.5's `PM_GetViewHeightLerpTime` (0x223a6) takes 200 ms down to the
    // crouch and has no dive.
    if ps.rules.v15() && ps.view_lerp_target == VIEW_CROUCH && ps.view_lerp_down {
        return VIEW_LERP_STAND_MS;
    }
    view_lerp_length(ps.view_lerp_target, ps.view_lerp_down, ps.prone_dive)
}

/// `PM_GetViewHeightLerpTime(ps, target, down)`, `dive` being `pm_flags` 0x4.
pub(super) fn view_lerp_length(target: f32, down: bool, dive: bool) -> i32 {
    match (target, down, dive) {
        (VIEW_PRONE, _, true) => VIEW_LERP_DIVE_MS,
        (VIEW_PRONE, _, false) => VIEW_LERP_PRONE_MS,
        (VIEW_CROUCH, true, true) => VIEW_LERP_DIVE_DUCK_MS,
        (VIEW_CROUCH, true, false) => VIEW_LERP_DUCK_MS,
        (VIEW_CROUCH, false, _) => VIEW_LERP_PRONE_MS,
        _ => VIEW_LERP_STAND_MS,
    }
}

fn view_curve(ps: &PlayerState) -> &'static [(i32, f32)] {
    match (ps.view_lerp_target, ps.view_lerp_down, ps.prone_dive) {
        (VIEW_PRONE, _, true) => VIEW_CURVE_DIVE_PRONE,
        (VIEW_PRONE, _, false) => VIEW_CURVE_CROUCH_PRONE,
        (VIEW_CROUCH, true, _) => VIEW_CURVE_STAND_CROUCH,
        (VIEW_CROUCH, false, _) => VIEW_CURVE_PRONE_CROUCH,
        _ => VIEW_CURVE_CROUCH_STAND,
    }
}

/// The eye at `pct` of a leg: linear between the waypoints either side.
fn view_curve_height(curve: &[(i32, f32)], pct: i32) -> f32 {
    let Some(i) = curve.iter().position(|&(p, _)| p >= pct) else {
        return curve[0].1;
    };
    let (p1, h1) = curve[i];
    if p1 == pct || i == 0 {
        return h1;
    }
    let (p0, h0) = curve[i - 1];
    let t = f64::from(pct - p0) / f64::from(p1 - p0);
    (f64::from(h0) + t * f64::from(h1 - h0)) as f32
}

/// Retail's stance for the walk's scale and accel and the jump's gate, one
/// test inlined three times (0x2e7a1, 0x2f436, 0x2ebc8): the eye's leg counts
/// as much as the flags, so a player standing up out of prone keeps prone's
/// accel and cannot jump until the eye reaches the crouch height.
pub(super) fn move_stance(ps: &PlayerState) -> Stance {
    if ps.rules.v15() {
        return ps.effective_stance();
    }
    let lerping = ps.view_lerp_ms.is_some();
    if ps.stance == Stance::Prone
        || ps.view_lerp_target == VIEW_PRONE
        || (lerping && ps.view_lerp_target == VIEW_CROUCH && !ps.view_lerp_down)
    {
        Stance::Prone
    } else if ps.ducked || (lerping && ps.view_lerp_target == VIEW_CROUCH) {
        Stance::Crouch
    } else {
        Stance::Stand
    }
}

/// How far the running leg has come from `from` toward `to`, 0..1, or 0 when
/// that is not the leg running (0x308cc).
fn view_lerp_frac(ps: &PlayerState, from: f32, to: f32) -> f32 {
    let Some(ms) = ps.view_lerp_ms else {
        return 0.0;
    };
    if to != ps.view_lerp_target {
        return 0.0;
    }
    let from_ok =
        (from == VIEW_PRONE && !ps.view_lerp_down) || (from == VIEW_STAND && ps.view_lerp_down);
    if to == VIEW_CROUCH && !from_ok {
        return 0.0;
    }
    (ms as f32 / view_lerp_duration(ps) as f32).clamp(0.0, 1.0)
}

/// The walk scale's stance factor (0x2e7a1-0x2e8d2): across a leg between
/// crouch and prone the two scales blend by the leg's progress, otherwise
/// [`move_stance`]'s own.
pub(super) fn stance_speed_scale(ps: &PlayerState) -> f32 {
    let f = view_lerp_frac(ps, VIEW_CROUCH, VIEW_PRONE);
    if f != 0.0 {
        return f * SCALE_PRONE + (1.0 - f) * SCALE_CROUCH;
    }
    let f = view_lerp_frac(ps, VIEW_PRONE, VIEW_CROUCH);
    if f != 0.0 {
        return f * SCALE_CROUCH + (1.0 - f) * SCALE_PRONE;
    }
    move_stance(ps).speed_scale()
}

/// [`stance_speed_scale`] as 1.5b's walk scale reads it: a leg's blend
/// left on the x87 stack (0x1f2b5-0x1f36d), the lerp fraction a float.
pub(super) fn stance_speed_scale_kept(ps: &PlayerState) -> f64 {
    let blend = |f: f32, to: f32, from: f32| {
        let f = f64::from(f);
        f64::from(to) * f + (1.0 - f) * f64::from(from)
    };
    let f = view_lerp_frac(ps, VIEW_CROUCH, VIEW_PRONE);
    if f != 0.0 {
        return blend(f, SCALE_PRONE, SCALE_CROUCH);
    }
    let f = view_lerp_frac(ps, VIEW_PRONE, VIEW_CROUCH);
    if f != 0.0 {
        return blend(f, SCALE_CROUCH, SCALE_PRONE);
    }
    f64::from(move_stance(ps).speed_scale())
}
