//! `PM_CheckJump` and 1.5's jump penalty.

use super::footsteps::SURF_NO_SOUND;
use super::ground::Takeoff;
use super::ladder::{LADDER_PUSHOFF_SPEED, forward3};
use super::stance::move_stance;
use super::state::{MoveRules, PlayerState, PmEvent, PmInput, Stance};
use crate::net::event_ids::EV_JUMP_BASE;
use glam::Vec3;

/// `PM_CheckJump` (0x2eb98): vz = sqrt(g * 78) and `fJumpOriginZ` 39 above
/// the takeoff (rodata 0x708c8/0x708cc), so the jump's apex and its origin
/// are one height (docs/research/cod11-mantle.md, "Jumps").
pub const JUMP_HEIGHT: f32 = 39.0;
/// The jump's `aimSpreadScale` kick and its cap (rodata 0x708dc/0x708e0).
pub(super) const JUMP_SPREAD_ADD: f32 = 64.0;
const JUMP_SPREAD_MAX: f32 = 255.0;

/// Re-jump gate on every jump: cmd.serverTime - ps.jumpTime > 499 (@0x2ebb3).
pub const JUMP_COOLDOWN_MS: f32 = 500.0;

/// Retail `PM_CheckJump` (0x2eb98), called from `PM_WalkMove` and
/// `PM_LadderMove`; `ladder` is the ladder plane when the latter. Refused
/// inside 500 ms of the last jump, on a held key and to anything but a
/// standing player. vz = sqrt(g * 78) with horizontal velocity kept, and
/// `fJumpOriginZ` 39 up. Off a ladder, vz is scaled 0.75 and the horizontal
/// reset to 128 along the forward, reflected off the plane while facing it.
/// A held key refused here is taken off the cmd for the rest of the move, as
/// retail zeroes `cmd.upmove` (0x2ec13), which the ladder's wish reads. Not
/// modelled: the `PMF_RESPAWNED` gate (0x800), which the spawn's own think
/// clears before any cmd of a live player (docs/research/cod11-mantle.md,
/// "Jumps").
pub(super) fn check_jump(
    ps: &mut PlayerState,
    input: &mut PmInput,
    ladder: Option<Vec3>,
    events: &mut Vec<PmEvent>,
) -> bool {
    // UO refuses a deployed bipod ahead of the latch (0x24483).
    if ps.since_jump_ms <= JUMP_COOLDOWN_MS - 1.0
        || move_stance(ps) != Stance::Stand
        || !input.jump
        || ps.deployed()
    {
        return false;
    }
    if ps.jump_latched {
        input.jump = false;
        return false;
    }
    ps.on_ground = false;
    ps.ground_plane = None;
    ps.jump_latched = true;
    match ps.rules {
        MoveRules::V1_1 => {
            ps.velocity.z = (2.0 * JUMP_HEIGHT * ps.gravity).sqrt();
            ps.jump_origin_z = ps.origin.z + JUMP_HEIGHT;
        }
        MoveRules::V1_5 | MoveRules::Uo => {
            // 1.5's takeoff (0x1f882): the penalty divides the squared
            // speed, then restarts with the timer stopped until the landing,
            // and the origin is the takeoff height itself.
            let mut v2 = 2.0 * JUMP_HEIGHT * ps.gravity;
            if ps.jump_penalty && ps.knockback_ms <= JUMP_PENALTY_MS {
                v2 = (f64::from(v2) / f64::from(jump_penalty_scale(ps.knockback_ms))) as f32;
            }
            ps.velocity.z = f64::from(v2).sqrt() as f32;
            ps.jump_penalty = true;
            ps.knockback_ms = 0.0;
            ps.jump_origin_z = ps.origin.z;
        }
    }
    if let Some(normal) = ladder {
        ps.velocity.z *= 0.75;
        let f = Vec3::new(ps.yaw.cos(), ps.yaw.sin(), 0.0).normalize_or_zero();
        // reflection only when looking into the wall (n.forward < 0, @0x2ecd5)
        let dir = if normal.dot(forward3(ps)) < 0.0 {
            let d2 = f.x * normal.x + f.y * normal.y;
            (f - normal * (2.0 * d2)).normalize_or_zero()
        } else {
            f
        };
        ps.velocity.x = dir.x * LADDER_PUSHOFF_SPEED;
        ps.velocity.y = dir.y * LADDER_PUSHOFF_SPEED;
        ps.on_ladder = false;
    }
    // 70 + material off the last ground trace; material 0 or the silent
    // surfaceparm emits nothing (@0x2ed90-0x2edb9).
    let sf = ps.ground_surface_flags;
    let mat = crate::collision::sound_material(sf);
    if mat != 0 && sf & SURF_NO_SOUND == 0 {
        events.push(PmEvent {
            event: EV_JUMP_BASE + mat,
            parm: 0,
        });
    }
    ps.aim_spread_scale = (ps.aim_spread_scale + JUMP_SPREAD_ADD).min(JUMP_SPREAD_MAX);
    ps.jumped = true;
    // The ground jump's anim, forced; a ladder push-off raises none
    // (1.1 0x2eb98: the event sits in the `pm_flags & 0x10 == 0` arm).
    if ladder.is_none() {
        ps.takeoff = Some(Takeoff::raised(true, input));
    }
    true
}

/// The longest 1.5 jump penalty, `pm_time` at a low landing (1.5 0x20169).
pub(super) const JUMP_PENALTY_MS: f32 = 1800.0;
/// A landing this far above the takeoff starts a shorter, harsher penalty
/// (1.5 rodata 0x756ec, 0x201e2).
const JUMP_PENALTY_HIGH_LANDING: f32 = 18.0;
const JUMP_PENALTY_HIGH_MS: f32 = 1200.0;
/// The landing's velocity scales, low and high (rodata 0x756e8, 0x756f0).
const JUMP_PENALTY_LANDING_SCALE: f32 = 0.65;
const JUMP_PENALTY_HIGH_SCALE: f32 = 0.5;

/// 1.5's penalty curve, the friction multiplier and the jump's divisor
/// alike (0x1e9f1, 0x1ea52): 1 at `pm_time` 0 rising linearly to 2.5 at
/// 1700, and 2.5 from there (rodata 0x7566c..0x75678).
pub(super) fn jump_penalty_scale(pm_time: f32) -> f32 {
    if pm_time > 1699.0 {
        2.5
    } else {
        (f64::from(pm_time) * 1.5 * f64::from(0.000_588_235_27_f32) + 1.0) as f32
    }
}

/// The top of 1.5's `PM_WalkMove` (0x2003d..0x201e2): the first walking
/// frame after a jump starts the penalty's timer and takes speed off, a
/// timer over the penalty's (a landing stun) ends it instead.
pub(super) fn jump_penalty_landing(ps: &mut PlayerState) {
    if !ps.jump_penalty {
        return;
    }
    if ps.knockback_ms > JUMP_PENALTY_MS {
        ps.jump_penalty = false;
        ps.jump_origin_z = 0.0;
        ps.velocity *= JUMP_PENALTY_LANDING_SCALE;
    } else if ps.knockback_ms == 0.0 {
        if ps.origin.z < ps.jump_origin_z + JUMP_PENALTY_HIGH_LANDING {
            ps.knockback_ms = JUMP_PENALTY_MS;
            ps.velocity *= JUMP_PENALTY_LANDING_SCALE;
        } else {
            ps.knockback_ms = JUMP_PENALTY_HIGH_MS;
            ps.velocity *= JUMP_PENALTY_HIGH_SCALE;
        }
    }
}
