//! Contains routines ported from the Quake III Arena GPL source, Copyright (C) 1999-2005 Id Software, Inc.,
//! and the RTCW-MP GPL source, Copyright (C) 1999-2010 id Software LLC, a ZeniMax Media company. See NOTICE.
//!
//! `PM_Friction`, `PM_CmdScale`, the accelerates, `PM_ClipVelocity`, `PM_WalkMove`,
//! `PM_AirMove` and the legs' movement direction.

use super::angles::view_deg;
use super::ground::{WATER_FRICTION, on_slick};
use super::jump::{JUMP_PENALTY_MS, jump_penalty_scale};
use super::slide::step_slide_move;
use super::stance::{stance_speed_scale, stance_speed_scale_kept};
use super::state::{MoveRules, PlayerState, PmEvent, PmInput, Stance};
use super::uo::SCALE_SPRINT;
use super::{
    KNOCKBACK_ACCEL_SCALE, KNOCKBACK_FRICTION_SCALE, OVERCLIP, PM_ACCELERATE, PM_AIRACCELERATE,
    PM_DUCKED_ACCELERATE, PM_FRICTION, PM_PRONE_ACCELERATE, PM_SPECTATOR_FRICTION, PM_STOPSPEED,
    PMF_TIME_DAMAGE, PMF_TIME_KNOCKBACK, SCALE_BACK, SCALE_LEAN, SCALE_SHELLSHOCK, SCALE_STRAFE,
    SCALE_WALK, WADE_SCALE, WALK_ACCEL_FLOOR, aim, weapon,
};
use crate::movetrace::MoveWorld;
use crate::weapon::WeaponDef;
use crate::x87;
use glam::Vec3;

/// How far the legs may turn off the view. The ground path caps at 90
/// (@0x2e9d2), `PM_LadderMove` at 75 (@0x33dd1).
const MOVEMENT_DIR_CAP: i32 = 90;
pub(super) const LADDER_MOVEMENT_DIR_CAP: i32 = 75;

/// `PM_Friction`: ground friction only while walking in water level <= 1,
/// plus a water term that already applies while wading. Unlike RTCW's it has
/// no ladder term (game.mp.i386.so 0x2e460, cgame 0x30007430).
pub(super) fn friction(ps: &mut PlayerState, world: &MoveWorld, dt: f32) {
    if world.world.arith() == crate::patch::Arith::Msvc {
        return friction_msvc(ps, dt);
    }
    // `PM_Friction` (0x2e460) on the x87: the speed is stored as a float,
    // the drop and the scale stay on the stack, and each component is
    // rounded once.
    let v = ps.velocity.as_dvec3();
    let vz = if ps.on_ground { 0.0 } else { v.z };
    let speed = ((v.x * v.x + v.y * v.y) + vz * vz).sqrt() as f32;
    if speed < 1.0 {
        ps.velocity.x = 0.0;
        ps.velocity.y = 0.0;
        // 1.5 stops the fall too (0x1eb4f), at a jump's apex as well.
        if ps.rules.v15() {
            ps.velocity.z = 0.0;
        }
        return;
    }
    if ps.rules.v15() {
        return friction_stored(ps, speed, dt, false);
    }
    let (s, dt) = (f64::from(speed), f64::from(dt));
    let mut drop = 0.0;
    // Slick ground and a hit's knockback slide free of the ground term
    // (0x2e4ed, 0x2e4fb).
    if ps.on_ground
        && ps.water_level <= 1
        && !on_slick(ps)
        && ps.knockback_flags & PMF_TIME_DAMAGE == 0
    {
        let mut control = s.max(f64::from(PM_STOPSPEED));
        if ps.knockback_flags & PMF_TIME_KNOCKBACK != 0 {
            control *= f64::from(KNOCKBACK_FRICTION_SCALE);
        } else if ps.jump_penalty && ps.knockback_ms <= JUMP_PENALTY_MS {
            control *= f64::from(jump_penalty_scale(ps.knockback_ms));
        }
        drop += control * f64::from(PM_FRICTION) * dt;
    }
    if ps.water_level > 0 {
        drop += f64::from(ps.water_level) * s * f64::from(WATER_FRICTION) * dt;
    }
    let scale = (s - drop).max(0.0) / s;
    ps.velocity = x87::scale(ps.velocity, scale);
}

/// The Windows cgames' `PM_Friction` (cgame_mp_x86.dll 1.1 0x30007430,
/// 1.5 0x30007b40, UO 0x30008470; cod15-movement.md 8): the speed sums
/// `(vz^2 + vy^2) + vx^2` and is kept on 1.1's 53-bit stack, stored on
/// 1.5's and UO's; the control, the drop, the new speed and its ratio stay
/// on the stack, and each component is rounded once. The ground term is
/// `(frametime * 5.5) * control`, UO's `(control * frametime) * 5.5`, and
/// 1.5's jump penalty scale `pm_time * (1.5 / 1700) + 1` with the constant
/// folded into one float.
fn friction_msvc(ps: &mut PlayerState, dt: f32) {
    let v = ps.velocity.as_dvec3();
    let vz = if ps.on_ground { 0.0 } else { v.z };
    let mut s = ((vz * vz + v.y * v.y) + v.x * v.x).sqrt();
    if ps.rules.v15() {
        s = f64::from(s as f32);
    }
    if s < 1.0 {
        ps.velocity.x = 0.0;
        ps.velocity.y = 0.0;
        if ps.rules.v15() {
            ps.velocity.z = 0.0;
        }
        return;
    }
    let ft = f64::from(dt);
    let mut drop = 0.0;
    if ps.on_ground
        && ps.water_level <= 1
        && !on_slick(ps)
        && ps.knockback_flags & PMF_TIME_DAMAGE == 0
    {
        let mut control = s.max(f64::from(PM_STOPSPEED));
        if ps.knockback_flags & PMF_TIME_KNOCKBACK != 0 {
            control *= f64::from(KNOCKBACK_FRICTION_SCALE);
        } else if ps.jump_penalty && ps.knockback_ms <= JUMP_PENALTY_MS {
            let scale = if ps.knockback_ms >= 1700.0 {
                2.5
            } else {
                f64::from(ps.knockback_ms) * f64::from(0.000_882_352_94_f32) + 1.0
            };
            control *= scale;
        }
        drop = if ps.rules == MoveRules::Uo {
            control * ft * f64::from(PM_FRICTION)
        } else {
            ft * f64::from(PM_FRICTION) * control
        };
    }
    if ps.water_level > 0 {
        drop += f64::from(ps.water_level) * f64::from(WATER_FRICTION) * ft * s;
    }
    let scale = (s - drop).max(0.0) / s;
    ps.velocity = x87::scale(ps.velocity, scale);
}

/// 1.5b's `PM_Friction` past the stop (0x1ebba-0x1ed59), which UO shares:
/// the control, the drop, the new speed and its ratio to the speed are each
/// stored, and each component is the float times the stored ratio. A
/// `spectator`'s `pm_type` 4 term comes last.
pub(super) fn friction_stored(ps: &mut PlayerState, speed: f32, dt: f32, spectator: bool) {
    let r = |x: f64| f64::from(x as f32);
    let s = f64::from(speed);
    let dt = f64::from(dt);
    let mut drop = 0.0;
    if ps.on_ground
        && ps.water_level <= 1
        && !on_slick(ps)
        && ps.knockback_flags & PMF_TIME_DAMAGE == 0
    {
        let mut control = r(s.max(f64::from(PM_STOPSPEED)));
        if ps.knockback_flags & PMF_TIME_KNOCKBACK != 0 {
            control = r(control * f64::from(KNOCKBACK_FRICTION_SCALE));
        } else if ps.jump_penalty && ps.knockback_ms <= JUMP_PENALTY_MS {
            control = r(f64::from(jump_penalty_scale(ps.knockback_ms)) * control);
        }
        drop = r(control * f64::from(PM_FRICTION) * dt + drop);
    }
    if ps.water_level > 0 {
        // Retail's instruction order, not Ghidra's print (cod15-movement.md 8).
        drop = r(s * f64::from(WATER_FRICTION) * f64::from(ps.water_level) * dt + drop);
    }
    if spectator {
        drop = r(s * f64::from(PM_SPECTATOR_FRICTION) * dt + drop);
    }
    let mut new = r(s - drop);
    if new < 0.0 {
        new = 0.0;
    }
    let scale = r(new / s);
    ps.velocity = x87::scale(ps.velocity, scale);
}

/// How a build's walk cmd scale rounds its steps (cod15-movement.md 8.2,
/// "The walk's cmd scale").
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum WishArith {
    /// 1.1d: the whole product on the x87 stack.
    Kept,
    /// 1.5b's and UO's Linux builds (0x1f08b): the max, the length, the
    /// base scale and each factor's product stored.
    Stored,
    /// The Windows cgames (1.1 0x300076e0, 1.5 0x30007e40, UO 0x30008770):
    /// the strafe product and the walk, run or sprint product stored, the
    /// rest kept at 53 bits.
    Msvc,
}

/// The walk mover's wish direction and speed, retail's `PM_CmdScale`
/// (game.mp.i386.so 0x2e690, called from `PM_WalkMove` at 0x2f2bf): Q3's
/// `speed * max / (127 * total)` over the cmd bytes, with a backpedal read
/// through `backSpeedScale` and a strafe through `strafeSpeedScale` before
/// the max is taken, then `walkSpeedScale` on the ADS walk and otherwise
/// `runSpeedScale` with `leanSpeedScale` on a lean, the stance scale
/// ([`stance_speed_scale`]), the wade scale and the weapon's `moveSpeedScale`. The cmd magnitude then
/// multiplies back in, so a lone full key wishes `ps.speed` and a diagonal
/// wishes no more (docs/research/cod11-mantle.md, "The wish speed").
///
/// `walk_slow` is the client's own fly-mode key and takes the walk scale.
/// Last comes the shellshock's [`SCALE_SHELLSHOCK`] on `wbuttons` 0x4
/// (0x2e959), inside the same unrounded product.
///
/// The direction is `PM_WalkMove`'s (0x2f2c7-0x2f433): `pml.forward` and
/// `pml.right` flattened, clipped onto the ground plane and normalized, then
/// combined by the cmd bytes and normalized again; the speed is that
/// length times the stored scale, left unrounded.
pub(super) fn wish(
    ps: &PlayerState,
    input: &PmInput,
    weapon: Option<&WeaponDef>,
    arith: WishArith,
) -> (Vec3, f64) {
    let (f, r) = (
        (input.forward * 127.0).round(),
        (input.right * 127.0).round(),
    );
    let (fd, rd) = (f64::from(f), f64::from(r));
    let stored = arith == WishArith::Stored;
    let msvc = arith == WishArith::Msvc;
    let st = |x: f64| if stored { f64::from(x as f32) } else { x };
    // The two steps the Windows cgames store as well.
    let st_win = |x: f64| {
        if stored || msvc {
            f64::from(x as f32)
        } else {
            x
        }
    };
    let back = st(if f < 0.0 {
        (fd * f64::from(SCALE_BACK)).abs()
    } else {
        fd.abs()
    });
    let max = back.max(st_win((rd * f64::from(SCALE_STRAFE)).abs()));
    if max == 0.0 {
        return (Vec3::ZERO, 0.0);
    }
    let total = st((fd * fd + rd * rd).sqrt());
    let mut scale = st((f64::from(ps.speed) * max) / (total * 127.0));
    // UO walks an LMG's breakdown (0x23af5).
    let breakdown = ps.lmg && ps.weaponstate == weapon::WEAPON_BREAKDOWN;
    if ps.walking || input.walk_slow || breakdown && ps.rules == MoveRules::Uo {
        scale = st_win(scale * f64::from(SCALE_WALK));
    } else if ps.lean != 0.0 {
        // 1.1's Windows cgame stores the run scale's product before the
        // lean's (0x30007791); 1.5's and UO's take a lean as the walk.
        if msvc && ps.rules == MoveRules::V1_1 {
            scale = st_win(scale);
        }
        scale = st_win(scale * f64::from(SCALE_LEAN));
    } else if ps.sprinting {
        scale = st_win(scale * f64::from(SCALE_SPRINT));
    } else {
        // The run scale, 1.0.
        scale = st_win(scale);
    }
    // 1.5b and the Windows cgames blend a stance leg's two scales on the
    // stack (0x1f2b5, 0x1f335; 1.5 cgame 0x30007f79).
    let stance = if stored || msvc {
        stance_speed_scale_kept(ps)
    } else {
        f64::from(stance_speed_scale(ps))
    };
    if stance != 1.0 {
        scale = st(scale * stance);
    }
    if ps.water_level > 0 {
        // The Windows cgames multiply by the float 1/3 (1.5 cgame 0x30008008).
        let wade = if msvc {
            f64::from(ps.water_level) * f64::from(1.0f32 / 3.0)
        } else {
            st(f64::from(ps.water_level) / 3.0)
        };
        scale = st(scale * st(1.0 - wade * f64::from(WADE_SCALE)));
    }
    // A sprint leaves the weapon's scale out (UO 0x23af5).
    if let Some(w) = weapon.filter(|w| w.move_speed_scale > 0.0 && !ps.sprinting) {
        scale = st(scale * f64::from(w.move_speed_scale));
    }
    if input.shellshock {
        scale = st(scale * f64::from(SCALE_SHELLSHOCK));
    }
    let scale = scale as f32;

    let turn = if msvc {
        crate::patch::Arith::Msvc
    } else {
        crate::patch::Arith::Kept
    };
    let (forward, right) = view_vectors(ps, turn);
    let v15 = ps.rules.v15();
    // 1.5b's walk (0x2008c-0x203f5) clips and normalizes through its
    // stored `PM_ClipVelocity` and `VectorNormalize`; the Windows cgames'
    // `VectorNormalize` stores alike (1.1 0x30039cc0), and their inline
    // clip takes the reciprocal overclip (cod15-movement.md 8).
    let norm = |v: Vec3| {
        if v15 || msvc {
            x87::normalize_stored(v)
        } else {
            x87::normalize(v)
        }
    };
    let clip = |v: Vec3| {
        if msvc {
            clip_velocity_msvc(v, ps.ground_normal)
        } else {
            clip_velocity(v, ps.ground_normal, v15)
        }
    };
    let flat = |v: Vec3| norm(clip(v.with_z(0.0)));
    let (forward, right) = (flat(forward), flat(right));
    let wishvel = (forward.as_dvec3() * fd + right.as_dvec3() * rd).as_vec3();
    let sq = x87::dot(wishvel, wishvel);
    let len = if v15 || msvc {
        f64::from(sq as f32).sqrt() as f32
    } else {
        sq.sqrt() as f32
    };
    (norm(wishvel), f64::from(len) * f64::from(scale))
}

/// `PmoveSingle`'s `AngleVectors` of `ps.viewangles` (game.mp 0x34120,
/// 1.5 0x284ec, UO 0x2dbdd; the Windows cgames' at 1.1 0x3000c718), forward
/// and right. No roll. The sim's radians are the Linux builds' own; a
/// Windows cgame turns the degrees by its float constant instead
/// ([`x87::radians_msvc`]).
pub(super) fn view_vectors(ps: &PlayerState, arith: crate::patch::Arith) -> (Vec3, Vec3) {
    // The sim's pitch is positive up; the view's is positive down.
    let [f, r, _] = if arith == crate::patch::Arith::Msvc {
        x87::angle_vectors_msvc_of_radians(Vec3::new(
            x87::radians_msvc(view_deg(-ps.pitch)),
            x87::radians_msvc(view_deg(ps.yaw)),
            0.0,
        ))
    } else {
        x87::angle_vectors_of_radians(Vec3::new(-ps.pitch, ps.yaw, 0.0))
    };
    (f, r)
}

/// `AngleVectors` of `(0, viewangles[YAW], 0)`'s forward, as `arith`'s
/// build turns it (see [`view_vectors`]).
pub(super) fn yaw_forward(ps: &PlayerState, arith: crate::patch::Arith) -> Vec3 {
    if arith == crate::patch::Arith::Msvc {
        let rad = x87::radians_msvc(view_deg(ps.yaw));
        x87::angle_vectors_msvc_of_radians(Vec3::new(0.0, rad, 0.0))[0]
    } else {
        x87::angle_vectors_of_radians(Vec3::new(0.0, ps.yaw, 0.0))[0]
    }
}

/// The air mover's wish, retail's Q3-shaped scale (0x2e5bc, called from
/// `PM_AirMove` at 0x2f083): the magnitude rule over forward, right and up,
/// and the walk/run pick, with no stance, lean, weapon or wade factor. The
/// cmd's `upmove` dilutes the horizontal wish the way Q3's does.
fn wish_air(ps: &PlayerState, input: &PmInput, stored: bool) -> (Vec3, f32, f32) {
    let (f, r, u) = (input.forward * 127.0, input.right * 127.0, input.up * 127.0);
    if f.abs().max(r.abs()).max(u.abs()) <= 0.0 {
        return (Vec3::ZERO, 0.0, 0.0);
    }
    let scale = cmd_scale(ps, input, stored);
    let fwd = Vec3::new(ps.yaw.cos(), ps.yaw.sin(), 0.0);
    let right = Vec3::new(ps.yaw.sin(), -ps.yaw.cos(), 0.0);
    let wishvel = fwd * f + right * r;
    (wishvel.normalize_or_zero(), wishvel.length(), scale)
}

/// 1.1d's `PM_AirMove` wish (0x2f036..0x2f11a): `PM_CmdScale` stored as a
/// float (0x2f088), the view's forward and right with `z` zeroed and
/// `VectorNormalize`d (0x2f0a7, 0x2f0b9), each wish component
/// `forward * fmove + right * smove` rounded once (0x2f0f2), then
/// normalized with its float length.
fn wish_air_x87(ps: &PlayerState, input: &PmInput) -> (Vec3, f32, f32) {
    let (fm, sm) = (input.forward * 127.0, input.right * 127.0);
    let scale = cmd_scale_x87(ps, input) as f32;
    let (forward, right) = view_vectors(ps, crate::patch::Arith::Kept);
    let f = x87::normalize_in_place(forward.with_z(0.0));
    let r = x87::normalize_in_place(right.with_z(0.0));
    let c = |f: f32, r: f32| (f64::from(f) * f64::from(fm) + f64::from(r) * f64::from(sm)) as f32;
    let wishvel = Vec3::new(c(f.x, r.x), c(f.y, r.y), 0.0);
    let len = x87::dot(wishvel, wishvel).sqrt() as f32;
    (x87::normalize_in_place(wishvel), len, scale)
}

/// 1.1d's `ProjectPointOnPlane` (game.mp 0x3f01c): `1 / |n|^2`, the dot and
/// the scaled normal all on the stack, each component of `p - d * n'`
/// rounded once.
pub(super) fn project_point_on_plane_x87(p: Vec3, n: Vec3) -> Vec3 {
    let (p, n) = (p.as_dvec3(), n.as_dvec3());
    let inv = 1.0 / ((n.x * n.x + n.y * n.y) + n.z * n.z);
    let d = ((n.x * p.x + n.y * p.y) + n.z * p.z) * inv;
    (p - (n * inv) * d).as_vec3()
}

/// Retail's Q3-shaped `PM_CmdScale` (0x2e5bc), shared by the air and ladder
/// moves: `speed * max / (127 * total)` over forward, right and up, then
/// the walk or run scale. It reads the cmd's own `upmove` (0x2e5e9), not
/// the stance bits: a prone body whose client sends `up` 0 falls off a
/// ledge on the undiluted wish (cod11-mantle.md, "The wish speed").
///
/// `stored` is 1.5b's and UO's (1.5b 0x1eec9, UO 0x23905): the length stored, then
/// `(speed * max) / (length * 127)` on the stack and stored once, and each
/// factor after it stored (cod15-movement.md 7).
pub(super) fn cmd_scale(ps: &PlayerState, input: &PmInput, stored: bool) -> f32 {
    let (f, r, u) = (input.forward * 127.0, input.right * 127.0, input.up * 127.0);
    let max = f.abs().max(r.abs()).max(u.abs());
    if max <= 0.0 {
        return 0.0;
    }
    let total = (f * f + r * r + u * u).sqrt();
    let mut scale = if stored {
        ((f64::from(ps.speed) * f64::from(max)) / (f64::from(total) * 127.0)) as f32
    } else {
        ps.speed * max / (127.0 * total)
    };
    // 1.5 takes a lean as the walk here too (0x1eec9).
    let lean = ps.rules.v15() && ps.lean != 0.0;
    if ps.walking || input.walk_slow || lean {
        scale *= SCALE_WALK;
    } else if ps.sprinting {
        scale *= SCALE_SPRINT;
    }
    scale
}

/// 1.1d's `PM_CmdScale` (0x2e5bc) as `PM_LadderMove` receives it: the
/// length stored as a float, then `speed * max / (127 * length)` and the
/// walk scale left on the x87 stack, unrounded, as the function returns
/// it (cod11-mantle.md, "Ladders").
pub(super) fn cmd_scale_x87(ps: &PlayerState, input: &PmInput) -> f64 {
    let (f, r, u) = (input.forward * 127.0, input.right * 127.0, input.up * 127.0);
    let max = f.abs().max(r.abs()).max(u.abs());
    if max <= 0.0 {
        return 0.0;
    }
    let total = (f * f + r * r + u * u).sqrt();
    let scale = f64::from(ps.speed) * f64::from(max) / (f64::from(total) * 127.0);
    if ps.walking || input.walk_slow {
        scale * f64::from(SCALE_WALK)
    } else {
        scale
    }
}

/// 1.5b's `PM_Accelerate` (0x1ed62), which its walk, air and ladder moves
/// all call: the speed along the wish and what it still lacks stored as
/// floats, the rate floored at `pm_stopspeed` (100) and stored once, capped
/// at the lack, and each component rounded once. The ground term's
/// `1 / ps.friction` is 1: only `ClientSpawn` writes the field, 1.0
/// (cod15-movement.md 8.2, "The walk's cmd scale").
pub(super) fn accelerate_stored(
    ps: &mut PlayerState,
    wishdir: Vec3,
    wishspeed: f32,
    accel: f32,
    dt: f32,
) {
    let current = x87::dot(ps.velocity, wishdir) as f32;
    let add = wishspeed - current;
    if add <= 0.0 {
        return;
    }
    let control = wishspeed.max(PM_STOPSPEED);
    let speed = ((f64::from(accel) * f64::from(dt) * f64::from(control)) as f32).min(add);
    ps.velocity = (wishdir.as_dvec3() * f64::from(speed) + ps.velocity.as_dvec3()).as_vec3();
}

/// 1.1d's air accelerate, inline in `PM_AirMove` (0x2f14b-0x2f1cd): Q3's
/// with the walk's floor, the rate `max(wishspeed, 100)` (rodata 0x708ec)
/// times the frame time (`pm_airaccelerate` 1 folded away), capped at the
/// lack; the wish speed, the dot and the rate stay on the x87 stack and
/// each component is rounded once (cod11-mantle.md, "The walk's accel
/// floor").
fn air_accelerate(ps: &mut PlayerState, wishdir: Vec3, wishspeed: f64, dt: f32) {
    let add = wishspeed - x87::dot(ps.velocity, wishdir);
    if add <= 0.0 {
        return;
    }
    let speed = (wishspeed.max(f64::from(PM_STOPSPEED)) * f64::from(dt)).min(add);
    ps.velocity = (wishdir.as_dvec3() * speed + ps.velocity.as_dvec3()).as_vec3();
}

pub(super) fn clamp_movement_dir(deg: i32, cap: i32) -> i32 {
    if deg > cap {
        cap
    } else if deg < -cap {
        -cap
    } else {
        deg
    }
}

/// `PM_SetMovementDir` @0x2e970, which retail runs at the tail of both
/// `PM_WalkMove` and `PM_AirMove`. The angle comes off the frame's
/// displacement rather than off the velocity, so a player scraping along a
/// wall turns its legs with the slide. Every intermediate is truncated to a
/// whole degree, as retail's `(int)` casts are.
fn set_movement_dir(ps: &mut PlayerState, input: &PmInput, dt: f32) {
    // Prone lays the legs along the body instead (@0x2e98d). Retail skips
    // this branch while the view is locked to another entity, eFlags
    // 0xc000; `ps.mounted` carries that state here.
    if ps.stance == Stance::Prone && ps.mounted.is_none() {
        // `AngleDelta` folds through the shorts, so the cap's 85 reads 84.
        ps.movement_dir = clamp_movement_dir(
            aim::angle_normalize_180(ps.prone_direction - ps.yaw.to_degrees()) as i32,
            MOVEMENT_DIR_CAP,
        );
        return;
    }
    // Retail has a ladder branch here too (@0x2e9f6), for the frames its
    // pm_flags ladder bit outlives its ladder mover; ours cannot, since
    // `on_ladder` is set by the same test that picks `ladder_move`.
    let moved = ps.origin - ps.move_start;
    // Airborne, no move key, or barely moved: the legs face the view. The
    // threshold is 5 units per second of frametime (@0x2ea4b-@0x2eaa7).
    if (input.forward == 0.0 && input.right == 0.0) || !ps.on_ground || moved.length() <= dt * 5.0 {
        ps.movement_dir = 0;
        return;
    }
    // `vectoangles` (0x3de94) puts the yaw in 0..360, and `AngleDelta`
    // (0x3ec9c) and `AngleNormalize180` (0x3eb70) fold through whole 16-bit
    // shorts, so a back key on a body still sliding forward reads +180, not
    // -180 (cod11-movers.md 15, "A moving body's clip").
    let yaw = {
        let y = (moved.y as f64).atan2(moved.x as f64).to_degrees() as f32;
        if y < 0.0 { y + 360.0 } else { y }
    };
    let mut deg = aim::angle_normalize_180(yaw - ps.yaw.to_degrees()) as i32;
    if input.forward < 0.0 {
        deg = aim::angle_normalize_180(deg as f32 + 180.0) as i32;
    }
    ps.movement_dir = clamp_movement_dir(deg, MOVEMENT_DIR_CAP);
}

/// Q3 `bg_pmove.c` `PM_ClipVelocity` (game.mp 0x3460c): the dot, the
/// overclip's product or quotient and each component's difference stay on
/// the x87 stack, and only the stored component is rounded. 1.5b's
/// (0x1e8e5), which UO shares, stores the dot, the backoff and each
/// component's change (`stored`).
pub(super) fn clip_velocity(vel: Vec3, normal: Vec3, stored: bool) -> Vec3 {
    let (v, n) = (vel.as_dvec3(), normal.as_dvec3());
    if stored {
        let r = |x: f64| f64::from(x as f32);
        let dot = r((v.x * n.x + v.y * n.y) + v.z * n.z);
        let backoff = r(if dot < 0.0 {
            dot * f64::from(OVERCLIP)
        } else {
            dot / f64::from(OVERCLIP)
        });
        let c = |i: usize| (v[i] - r(n[i] * backoff)) as f32;
        return Vec3::new(c(0), c(1), c(2));
    }
    let dot = v.dot(n);
    let backoff = if dot < 0.0 {
        dot * f64::from(OVERCLIP)
    } else {
        dot / f64::from(OVERCLIP)
    };
    (v - n * backoff).as_vec3()
}

/// The Windows cgames' `PM_ClipVelocity` as `PM_WalkMove` and `PM_AirMove`
/// inline it (1.1 `cgame_mp_x86.dll` 0x30008397, 0x30008016; 1.5
/// 0x30008bb3; UO 0x30009595): the dot summed `(z + y) + x` and kept, a
/// positive one times the float `1 / 1.001` (1.1 rodata 0x30069750,
/// 0x3f7fbe87) instead of divided by 1.001, each component rounded once
/// (cod15-movement.md 8).
pub(super) fn clip_velocity_msvc(vel: Vec3, normal: Vec3) -> Vec3 {
    let (v, n) = (vel.as_dvec3(), normal.as_dvec3());
    let dot = (v.z * n.z + v.y * n.y) + v.x * n.x;
    let backoff = if dot < 0.0 {
        dot * f64::from(OVERCLIP)
    } else {
        dot * f64::from(1.0 / OVERCLIP)
    };
    (v - n * backoff).as_vec3()
}

/// The Windows cgames' `PM_Accelerate` (1.1 `cgame_mp_x86.dll` 0x30007560,
/// 1.5 0x30007cb0, UO 0x300085d0), which every move calls with a stored
/// wish speed and accel: the speed along the wish summed `(z + y) + x` and
/// kept, the lack `wishspeed - dot` stored, the rate `(max(wishspeed, 100)
/// * frametime) * accel` kept and capped at the stored lack, each component
/// rounded once. Its `1 / ps.friction` with a ground entity is 1, as in
/// [`accelerate_stored`] (cod15-movement.md 8).
pub(super) fn accelerate_msvc(
    ps: &mut PlayerState,
    wishdir: Vec3,
    wishspeed: f32,
    accel: f32,
    dt: f32,
) {
    let (v, d) = (ps.velocity.as_dvec3(), wishdir.as_dvec3());
    let dot = (v.z * d.z + v.y * d.y) + d.x * v.x;
    let add = (f64::from(wishspeed) - dot) as f32;
    if add <= 0.0 {
        return;
    }
    let control = f64::from(wishspeed.max(PM_STOPSPEED));
    let speed = (control * f64::from(dt) * f64::from(accel)).min(f64::from(add));
    ps.velocity = (d * speed + v).as_vec3();
}

/// Q3 `bg_pmove.c` `PM_WalkMove`.
pub(super) fn walk_move(
    ps: &mut PlayerState,
    input: &PmInput,
    weapon: Option<&WeaponDef>,
    world: &MoveWorld,
    dt: f32,
    mask: u32,
    events: Option<&mut Vec<PmEvent>>,
) {
    let msvc = world.world.arith() == crate::patch::Arith::Msvc;
    let arith = if msvc {
        WishArith::Msvc
    } else if ps.rules.v15() {
        WishArith::Stored
    } else {
        WishArith::Kept
    };
    let (dir, wishspeed) = wish(ps, input, weapon, arith);
    // Slick ground or a hit's knockback (0x2f492, 0x2f58c).
    let sliding = on_slick(ps) || ps.knockback_flags & PMF_TIME_DAMAGE != 0;
    let mut accel = f64::from(match ps.effective_stance() {
        _ if sliding => 1.0,
        Stance::Stand => PM_ACCELERATE,
        Stance::Crouch => PM_DUCKED_ACCELERATE,
        Stance::Prone => PM_PRONE_ACCELERATE,
    });
    if ps.knockback_flags & PMF_TIME_KNOCKBACK != 0 {
        accel *= f64::from(KNOCKBACK_ACCEL_SCALE);
    }
    // Q3's `PM_Accelerate` inline, with the rate floored: a prone or
    // sighted wish of under 100 still gains 100's worth per frame, capped
    // at the wish (docs/research/cod11-mantle.md, "The walk's accel floor").
    // All of it on the x87 stack, the velocity rounded once per component
    // (0x2f4e4-0x2f57c). 1.5b stores the wish speed (0x203f5) and calls its
    // `PM_Accelerate`, and so do the Windows cgames (1.1 0x30008253,
    // 0x3000830f) with their own.
    if msvc {
        accelerate_msvc(ps, dir, wishspeed as f32, accel as f32, dt);
    } else if ps.rules.v15() {
        accelerate_stored(ps, dir, wishspeed as f32, accel as f32, dt);
    } else {
        let add = wishspeed - x87::dot(ps.velocity, dir);
        if add > 0.0 {
            let rate = wishspeed.max(f64::from(WALK_ACCEL_FLOOR));
            let speed = (accel * f64::from(dt) * rate).min(add);
            ps.velocity = (dir.as_dvec3() * speed + ps.velocity.as_dvec3()).as_vec3();
        }
    }
    // Q3's slick-or-knockback gravity, which the clip below turns into
    // ground speed.
    if sliding {
        ps.velocity.z = (f64::from(ps.velocity.z) - f64::from(ps.gravity) * f64::from(dt)) as f32;
    }
    // The clip onto the ground keeps the speed whenever it leaves the
    // velocity pointing the same way (0x2f5b8-0x2f6b3), so a landing that
    // still carries its fall turns it into ground speed.
    let before = ps.velocity;
    // The 1.5 cgame sums the speed `(z + y) + x` (0x30008b5c-0x30008b70).
    let sq = if msvc && ps.rules == MoveRules::V1_5 {
        let v = before.as_dvec3();
        (v.z * v.z + v.y * v.y) + v.x * v.x
    } else {
        x87::dot(before, before)
    };
    let speed = sq.sqrt() as f32;
    ps.velocity = if msvc {
        clip_velocity_msvc(ps.velocity, ps.ground_normal)
    } else {
        clip_velocity(ps.velocity, ps.ground_normal, ps.rules.v15())
    };
    if x87::dot(ps.velocity, before) > 0.0 {
        let dir = if ps.rules.v15() || msvc {
            x87::normalize_stored(ps.velocity)
        } else {
            x87::normalize(ps.velocity)
        };
        ps.velocity = x87::scale(dir, f64::from(speed));
    }
    // Standing still skips the move but not the legs: retail jumps straight
    // to PM_SetMovementDir (@0x2f6db), which is what keeps a prone player's
    // legs following its body while it turns on the spot.
    if ps.velocity.x != 0.0 || ps.velocity.y != 0.0 {
        step_slide_move(ps, world, dt, false, mask, events);
    }
    set_movement_dir(ps, input, dt);
}

/// Q3 `bg_pmove.c` `PM_AirMove`.
pub(super) fn air_move(
    ps: &mut PlayerState,
    input: &PmInput,
    world: &MoveWorld,
    dt: f32,
    mask: u32,
    events: Option<&mut Vec<PmEvent>>,
) {
    // `PM_AirMove` opens with `PM_Friction` (0x2f045): off the ground only
    // its sub-unit stop and the water term can bite.
    friction(ps, world, dt);
    let msvc = world.world.arith() == crate::patch::Arith::Msvc;
    let (dir, len, scale) = if msvc || ps.rules.v15() {
        wish_air(
            ps,
            input,
            world.world.arith() == crate::patch::Arith::Stored,
        )
    } else {
        wish_air_x87(ps, input)
    };
    if msvc {
        // The Windows cgames store the product too (1.1 0x30007fce) and
        // call their `PM_Accelerate` (0x30007fd7).
        accelerate_msvc(ps, dir, len * scale, PM_AIRACCELERATE, dt);
    } else if ps.rules.v15() {
        // 1.5b stores the product (0x1ffbb) and accelerates through the
        // walk's `PM_Accelerate`, floor and all (0x1ffd7).
        accelerate_stored(ps, dir, len * scale, PM_AIRACCELERATE, dt);
    } else {
        air_accelerate(ps, dir, f64::from(len) * f64::from(scale), dt);
    }
    // A plane too steep to stand on still steers the fall (0x2f1d3).
    if let Some(n) = ps.ground_plane {
        ps.velocity = if msvc {
            clip_velocity_msvc(ps.velocity, n)
        } else {
            clip_velocity(ps.velocity, n, ps.rules.v15())
        };
    }
    step_slide_move(ps, world, dt, true, mask, events);
    set_movement_dir(ps, input, dt);
}
