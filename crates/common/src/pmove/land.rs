//! Contains routines ported from the Quake III Arena GPL source, Copyright (C) 1999-2005 Id Software, Inc.,
//! and the RTCW-MP GPL source, Copyright (C) 1999-2010 id Software LLC, a ZeniMax Media company. See NOTICE.
//!
//! `PM_CrashLand`: the landing's damage, stun and events.

use super::PMF_TIME_KNOCKBACK;
use super::footsteps::SURF_NO_SOUND;
use super::ground::on_slick;
use super::state::{FallHeights, PlayerState, PmEvent};
use crate::net::event_ids::{
    EV_FOOTSTEP_RUN_BASE, EV_FOOTSTEP_WALK_BASE, EV_LANDING_BASE, EV_LANDING_PAIN_BASE,
};

/// `PM_CrashLand`'s gate on the land anim, against the vertical velocity the
/// move started with (`game.mp.i386.so` rodata 0x70a08).
pub const LAND_ANIM_SPEED: f32 = -220.0;

/// `PM_CrashLand` (0x2fd68): the fall height off the move's start, the fall
/// damage, and either the landing stun or the damage-free ladder
/// (docs/research/cod11-player-clip.md 8, cod11-sound-system.md "Landing").
/// `dead` is `pm_type > 5`, which lands with no damage.
pub(super) fn crash_land(
    ps: &mut PlayerState,
    start_vz: f32,
    dead: bool,
    events: &mut Vec<PmEvent>,
) -> i32 {
    if ps.water_level >= 3 {
        return 0;
    }
    // Retail solves the impact speed and squares it back over 2g; this is the
    // same height in closed form, and no landing at all when it has no root.
    let den = start_vz * start_vz + 2.0 * ps.gravity * (ps.move_start.z - ps.origin.z);
    if den < 0.0 {
        return 0;
    }
    let height = den / (2.0 * ps.gravity);
    let sf = ps.ground_surface_flags;
    let damage = if dead {
        0
    } else {
        fall_damage(height, sf, ps.water_level, ps.fall_heights)
    };
    if damage == 0 {
        if height >= LANDING_DAMP_HEIGHT {
            ps.velocity *= LANDING_DAMP;
        }
        events.extend(landing_event(height, ps));
        return 0;
    }
    // slick ground skips the stun (0x30013)
    if damage < 100 && !on_slick(ps) {
        let stun = (35 * damage + 500).min(2000);
        ps.knockback_ms = stun as f32;
        ps.knockback_flags |= PMF_TIME_KNOCKBACK;
        ps.velocity *= landing_stun_scale(stun);
    } else {
        ps.velocity *= LANDING_DAMP;
    }
    events.push(PmEvent {
        event: EV_LANDING_PAIN_BASE + landing_material(sf),
        parm: damage,
    });
    damage
}

/// The ground surface flag that takes no fall damage (0x2feaa).
pub(super) const SURF_NODAMAGE: u32 = 0x1;

pub const EV_LANDING_PAIN_LAST: i32 =
    EV_LANDING_PAIN_BASE + crate::net::event_ids::SURFACE_GROUP_SIZE - 1;

/// The fall damage percent: linear from 0 at the min height to 100 at the
/// max, truncated, halved (truncated again) at water level 2. Bounds out of
/// order or a negative min take none (0x2fe6c).
pub(super) fn fall_damage(height: f32, sf: u32, water_level: u32, h: FallHeights) -> i32 {
    let bad_bounds = h.max <= h.min || h.min < 0.0;
    let damage = if bad_bounds || height <= h.min || sf & SURF_NODAMAGE != 0 {
        0
    } else if height >= h.max {
        100
    } else {
        let frac = (height - h.min) / (h.max - h.min);
        ((frac * 100.0) as i32).clamp(0, 100)
    };
    if water_level == 2 {
        (damage as f32 * 0.5) as i32
    } else {
        damage
    }
}

/// The stun's velocity scale: 0.5 at 500 ms down to 0.2 at 1500 and past.
fn landing_stun_scale(stun_ms: i32) -> f32 {
    0.5 - (stun_ms - 500).clamp(0, 1000) as f32 / 1000.0 * 0.3
}

/// The fall height from which a landing plays the land event and damps the
/// velocity (rodata 0x70a18).
const LANDING_DAMP_HEIGHT: f32 = 12.0;
const LANDING_DAMP: f32 = 0.67;

/// The landing's surface index: 0, the default, on a no-sound surface.
fn landing_material(sf: u32) -> i32 {
    if sf & SURF_NO_SOUND != 0 {
        0
    } else {
        crate::collision::sound_material(sf)
    }
}

/// The damage-free ladder (0x30130): nothing at or under 4 units, a walk-step
/// to 8 and a run-step to 12, both silent on material 0, and from 12 the land
/// event, which plays the default on material 0 and carries the view bob.
pub(super) fn landing_event(height: f32, ps: &PlayerState) -> Option<PmEvent> {
    let mat = landing_material(ps.ground_surface_flags);
    if height >= LANDING_DAMP_HEIGHT {
        // (h - 12) / 26 * 4 + 4, truncated, at most 24; 0 at exactly 12
        let bob = if height > LANDING_DAMP_HEIGHT {
            (((height - LANDING_DAMP_HEIGHT) / 26.0 * 4.0 + 4.0) as i32).min(24)
        } else {
            0
        };
        return Some(PmEvent {
            event: EV_LANDING_BASE + mat,
            parm: bob,
        });
    }
    let base = if height >= 8.0 {
        EV_FOOTSTEP_RUN_BASE
    } else if height > 4.0 {
        EV_FOOTSTEP_WALK_BASE
    } else {
        return None;
    };
    (mat != 0).then_some(PmEvent {
        event: base + mat,
        parm: 0,
    })
}
