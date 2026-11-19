//! Contains routines ported from the Quake III Arena GPL source, Copyright (C) 1999-2005 Id Software, Inc.,
//! and the RTCW-MP GPL source, Copyright (C) 1999-2010 id Software LLC, a ZeniMax Media company. See NOTICE.
//!
//! The sight flag with United Offensive's deploy, the sprint and its fatigue.

use super::frame::msec;
use super::prone::{PRONE_CHECK_LENGTH, ProneQuery, check_prone};
use super::stance::push_event;
use super::state::{MoveRules, PlayerState, PmEvent, PmInput, Stance};
use super::walk::yaw_forward;
use super::weapon;
use crate::movetrace::{MASK_DEADSOLID, MoveWorld};
use crate::net::event_ids::{
    EV_STANCE_FORCE_CROUCH, EV_STANCE_FORCE_STAND, EV_UO_FATIGUE_LAST_SOUND,
};
use crate::weapon::WeaponDef;
use glam::Vec3;

/// United Offensive's `ps.sprintSpeedScale`, in place of the run scale while
/// sprinting (docs/research/coduo-movement.md, 3).
pub const SCALE_SPRINT: f32 = 1.6;
/// `pm_sprintFatigue`, `pm_fatigueRegen` and `pm_fatigueRegenDelay` (UO
/// 0x9b4e8..0x9b4f0): a full bar lasts 3 s of sprint and refills in 7 s,
/// 1 s after the last sprinting cmd (coduo-movement.md, 4).
const SPRINT_FATIGUE: f32 = 1.0 / 3.0;
const FATIGUE_REGEN: f32 = 1.0 / 7.0;
const FATIGUE_REGEN_DELAY_MS: i32 = 1000;
/// A sprint may start above this fatigue and carries on down to 0 (UO
/// 0x9b96c, 0x9b968).
const SPRINT_START_FATIGUE: f32 = 0.25;
/// Under this the body is winded (UO 0x9b900).
const WINDED_FATIGUE: f32 = 0.5;

/// United Offensive's `PmoveSingle` ahead of everything else (0x2d530):
/// the held weapon looked up and a sprint on the previous cmd cutting a
/// deploy short (docs/research/coduo-movement.md, 1).
pub(super) fn uo_prologue(ps: &mut PlayerState, weapons: &[Option<WeaponDef>]) {
    ps.lmg = weapons
        .get(ps.weapon as usize)
        .and_then(Option::as_ref)
        .is_some_and(WeaponDef::is_lmg);
    if ps.sprinting {
        weapon::interrupt_with_sprint(ps);
    }
}

/// The sight flag: [`weapon::update_ads_flag`], or under UO's rules
/// `PM_UpdateAimDownSightFlag` (0x32c0b) with its LMG deploy. An LMG sights
/// only by deploying: on a mount surface 15 ahead (the `hmgstanding` and
/// `hmgcrouching` brushes, contents 0x404000), which sets the stance the
/// surface names, or else by going prone where a body fits; one refused
/// tells the client to stand up again. The server's mount hint
/// ([`PlayerState::mount_hint`]) deploys where the probe misses. A deploy
/// holds while the button is down (docs/research/coduo-movement.md, 7, 11).
pub(super) fn ads_flag(
    ps: &mut PlayerState,
    input: &PmInput,
    def: Option<&WeaponDef>,
    world: &MoveWorld,
    events: &mut Vec<PmEvent>,
) {
    let lmg = def.is_some_and(WeaponDef::is_lmg);
    if ps.rules != MoveRules::Uo || !lmg {
        weapon::update_ads_flag(ps, input, def);
        return;
    }
    let keep = input.ads
        && def.is_some_and(|d| d.aim_down_sight)
        && !matches!(
            ps.weaponstate,
            weapon::WEAPON_RAISING
                | weapon::WEAPON_DROPPING
                | weapon::WEAPON_MELEE_WINDUP
                | weapon::WEAPON_MELEE_RELAX
        )
        && (ps.weaponstate != weapon::WEAPON_BREAKDOWN || input.weapon == ps.weapon)
        && (ps.sight_ground() || ps.linked);
    if !keep {
        ps.ads_active = false;
        return;
    }
    if ps.ads_active {
        return;
    }
    let view_yaw = ps.yaw.to_degrees();
    let (sin, cos) = ps.yaw.sin_cos();
    // 15 along `AngleVectors` of (0, yaw, 0) (0x32e3c), then 1 up.
    let fwd = yaw_forward(ps, world.world.arith());
    let probe = (ps.origin.as_dvec3() + fwd.as_dvec3() * 15.0).as_vec3() + Vec3::Z;
    let contents = world.point_contents(probe) & CONTENTS_HMG;
    let mount = if contents == 0 {
        ps.mount_hint
    } else if contents & CONTENTS_HMG_CROUCH != 0 {
        Some(Stance::Crouch)
    } else {
        Some(Stance::Stand)
    };
    if let Some(stance) = mount {
        ps.prone_direction = view_yaw;
        ps.ads_active = true;
        ps.stance = stance;
        ps.ducked = stance == Stance::Crouch;
        return;
    }
    if ps.stance != Stance::Prone {
        // The deploy's `BG_CheckProneValid` also wants the gun's 38 units
        // clear ahead, a +/-6 box 8 up (`BG_CheckProneValid` 0x2162c, its 16th argument).
        let gun = |o: Vec3| {
            let from = o + Vec3::Z * 8.0;
            world
                .box_trace(
                    from,
                    from + Vec3::new(cos, sin, 0.0) * 38.0,
                    Vec3::splat(-6.0),
                    Vec3::splat(6.0),
                    MASK_DEADSOLID | 0x20,
                )
                .fraction
                >= 1.0
        };
        let fit = (ps.on_ground && gun(ps.origin))
            .then(|| {
                check_prone(
                    world,
                    ProneQuery {
                        origin: ps.origin,
                        yaw_deg: view_yaw,
                        entry: true,
                        ground: true,
                        length: PRONE_CHECK_LENGTH,
                        rules: ps.rules,
                    },
                )
            })
            .flatten();
        let Some(body) = fit else {
            ps.prone_blocked = true;
            if !input.stance_held {
                push_event(
                    events,
                    if ps.ducked {
                        EV_STANCE_FORCE_CROUCH
                    } else {
                        EV_STANCE_FORCE_STAND
                    },
                );
            }
            return;
        };
        ps.prone_body = body;
        ps.prone_direction = view_yaw;
        ps.stance = Stance::Prone;
    }
    ps.ads_active = true;
}

/// UO's mount surfaces: `textures/common/hmgstanding` (0x4000) and
/// `hmgcrouching` (0x400000), the mask the deploy probes with.
pub const CONTENTS_HMG_CROUCH: u32 = 0x40_0000;
pub const CONTENTS_HMG: u32 = 0x4000 | CONTENTS_HMG_CROUCH;

/// UO's `PM_UpdatePlayerSprintingFlag` (0x2c74a), per cmd: the button held
/// with a move key, standing, off the sight, the ladder and any gun, with
/// fatigue left. Starting needs ground and more than a quarter of the bar;
/// a sprint already running keeps going in the air and down to empty.
/// A grenade in the hand must not be firing or cooking (coduo-movement.md,
/// 2).
pub(super) fn update_sprint_flag(ps: &mut PlayerState, input: &PmInput, def: Option<&WeaponDef>) {
    let was = ps.sprinting;
    ps.sprinting = false;
    if ps.rules != MoveRules::Uo || !input.sprint {
        return;
    }
    let grenade_busy = def.is_some_and(|d| {
        d.weapon_type == "grenade"
            && (input.attack || ps.weaponstate == weapon::WEAPON_FIRING || weapon::cooking(ps, d))
    });
    ps.sprinting = ps.fatigue > 0.0
        && (was || ps.fatigue > SPRINT_START_FATIGUE)
        && ps.stance != Stance::Prone
        && !ps.ducked
        && !ps.ads_active
        && (was || ps.on_ground)
        && (input.forward != 0.0 || input.right != 0.0)
        && ps.mounted.is_none()
        && ps.weaponstate != weapon::WEAPON_BREAKDOWN
        && !grenade_busy
        && !ps.on_ladder;
}

/// UO's `PM_UpdateFatigue` (0x2aa22): a sprinting cmd stamps
/// `lastSprintTime` and drains the bar, winding the body and raising
/// `EV_FATIGUE_LAST_SOUND` on every cmd under half; one that neither
/// sprints nor holds the button refills it from a second after the last
/// sprint, and a full bar clears the wind. Either way a cmd that does not
/// sprint goes on to [`fatigue_sound`], unless it is still inside the
/// refill's delay without the button. Under `bg_nofatigue` a sprinting
/// cmd does none of it. The products stay on the x87 stack until the store.
pub(super) fn update_fatigue(
    ps: &mut PlayerState,
    input: &PmInput,
    dt: f32,
    events: &mut Vec<PmEvent>,
) {
    if ps.rules != MoveRules::Uo {
        return;
    }
    let step = |rate: f32| 0.001f32 as f64 * f64::from(msec(dt)) * f64::from(rate);
    if ps.sprinting && ps.bg.nofatigue {
        // `bg_nofatigue` skips the stamp, the drain and the breath (0x2aa63).
    } else if ps.sprinting {
        ps.last_sprint_time = input.server_time;
        ps.fatigue = (f64::from(ps.fatigue) - step(SPRINT_FATIGUE)) as f32;
        if ps.fatigue < WINDED_FATIGUE {
            push_event(events, EV_UO_FATIGUE_LAST_SOUND);
            ps.winded = true;
        }
        if ps.fatigue < 0.0 {
            ps.fatigue = 0.0;
        }
    } else if input.sprint {
        fatigue_sound(ps, input.server_time, events);
    } else if ps.last_sprint_time.wrapping_add(FATIGUE_REGEN_DELAY_MS) <= input.server_time {
        ps.fatigue = (f64::from(ps.fatigue) + step(FATIGUE_REGEN)) as f32;
        if ps.fatigue >= 1.0 {
            ps.fatigue = 1.0;
            ps.winded = false;
        }
        fatigue_sound(ps, input.server_time, events);
    }
}

/// UO's `PM_PlayFatigueSound` (0x2a968): a winded body breathes
/// (`EV_FATIGUE_LAST_SOUND` again) once `iFatigueSoundTime` is more than
/// 1700 ms old, stamping it; a rested one clears a stamp that old.
fn fatigue_sound(ps: &mut PlayerState, server_time: i32, events: &mut Vec<PmEvent>) {
    let stale = ps.fatigue_sound_time.wrapping_add(FATIGUE_SOUND_MS) < server_time;
    if !ps.winded {
        if ps.fatigue_sound_time > 0 && stale {
            ps.fatigue_sound_time = 0;
        }
    } else if stale {
        push_event(events, EV_UO_FATIGUE_LAST_SOUND);
        ps.fatigue_sound_time = server_time;
    }
}

/// `PM_PlayFatigueSound`'s 0x6a4.
const FATIGUE_SOUND_MS: i32 = 1700;
