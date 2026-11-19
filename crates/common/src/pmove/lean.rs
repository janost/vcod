//! Contains routines ported from the Quake III Arena GPL source, Copyright (C) 1999-2005 Id Software, Inc.,
//! and the RTCW-MP GPL source, Copyright (C) 1999-2010 id Software LLC, a ZeniMax Media company. See NOTICE.
//!
//! `PM_UpdateLean`.

use super::frame::msec;
use super::state::{MoveRules, PlayerState, PmInput, Stance};
use super::{VIEW_CROUCH, VIEW_PRONE, aim};
use crate::collision::MASK_PLAYERSOLID;
use crate::movetrace::MoveWorld;
use glam::Vec3;

/// `PM_UpdateLean`'s ramp times for a full lean, out and back (rodata
/// 0x70c60, 0x70c5c), and the lean the crouched and prone stances top out at,
/// which also scales both ramps (0x70c58, 0x70c54).
pub const LEAN_TIME_TO_MS: f32 = 350.0;
pub const LEAN_TIME_FROM_MS: f32 = 280.0;
const LEAN_SCALE_CROUCH: f32 = 0.5;
const LEAN_SCALE_PRONE: f32 = 0.25;

/// `PM_UpdateLean` (0x32ac8), the last step of `PM_UpdateViewAngles` (0x33377), so it
/// reads the stance, ground and eye the previous move left
/// (docs/research/bsp-ibsp59-format.md, "Lean"). `alive` is `pm_type < 6`:
/// a dead body's lean only eases back. A linked player leans off the ground
/// and skips the wall clamp (0x32c63).
pub(super) fn update_lean(
    ps: &mut PlayerState,
    input: &PmInput,
    alive: bool,
    world: &MoveWorld,
    dt: f32,
) {
    let mut dir = 0;
    // UO's deployed bipod eases any lean back (`PM_UpdateLean` 0x2ac45).
    if alive && (ps.on_ground || ps.linked) && ps.mounted.is_none() && !ps.deployed() {
        if input.lean_left {
            dir -= 1;
        }
        if input.lean_right {
            dir += 1;
        }
    }
    let scale = lean_scale(ps);
    let msec = msec(dt);
    let cur = ps.lean;
    ps.lean = match dir {
        0 => {
            let step = msec / LEAN_TIME_FROM_MS * scale;
            if cur > 0.0 {
                (cur - step).max(0.0)
            } else {
                (cur + step).min(0.0)
            }
        }
        // Past the stance's limit (a lean carried into prone) it snaps back.
        d if d < 0 => {
            let l = if cur > -scale {
                cur - msec / LEAN_TIME_TO_MS * scale
            } else {
                cur
            };
            l.max(-scale)
        }
        _ => {
            let l = if cur < scale {
                cur + msec / LEAN_TIME_TO_MS * scale
            } else {
                cur
            };
            l.min(scale)
        }
    };
    // 1.5 traces a linked player's lean too.
    // 1.1's skip is `pm_type == 1` alone (0x32c63): a linked corpse clamps.
    if ps.lean == 0.0 || alive && ps.linked && ps.rules == MoveRules::V1_1 {
        return;
    }
    // A sphere from the eye to where a full lean would put it; the lean
    // keeps no more than the fraction the sphere got (0x32c97-0x32d50).
    let start = ps.origin + Vec3::Z * ps.view_height();
    let side = ps.lean.signum();
    let end = aim::add_lean(start, ps.yaw, side, world.world.arith());
    let t = world.box_trace(
        start,
        end,
        Vec3::splat(-8.0),
        Vec3::splat(8.0),
        MASK_PLAYERSOLID,
    );
    let max = aim::unget_lean_fraction(t.fraction);
    if max < ps.lean.abs() {
        ps.lean = side * max;
    }
}

/// The lean's limit and ramp scale off the stance `PM_UpdateLean` reads from
/// `pm_flags` and the eye's lerp: prone, or easing up out of it, 0.25;
/// crouched, or easing down into it, 0.5; standing 1.
fn lean_scale(ps: &PlayerState) -> f32 {
    if ps.rules.v15() {
        // 1.5 0x25ce0..0x25cf5: no full lean standing.
        return if ps.effective_stance() == Stance::Prone {
            LEAN_SCALE_PRONE
        } else {
            LEAN_SCALE_CROUCH
        };
    }
    let lerping = ps.view_lerp_ms.is_some();
    let target = ps.view_lerp_target;
    if ps.stance == Stance::Prone
        || target == VIEW_PRONE
        || (lerping && target == VIEW_CROUCH && !ps.view_lerp_down)
    {
        LEAN_SCALE_PRONE
    } else if ps.ducked || (lerping && target == VIEW_CROUCH) {
        LEAN_SCALE_CROUCH
    } else {
        1.0
    }
}
