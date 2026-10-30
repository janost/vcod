//! Friction, the accelerate and its inertia clamp, the cmd scales, the walk,
//! air, fly, noclip and dead moves and the jump
//! (docs/research/cod2-movement.md 5, 6).

use glam::Vec3;

use super::anim::Event;
use super::fpu::{self, d};
use super::slide::{clip_velocity, jump_clear, step_slide_move};
use super::{ENTITYNUM_NONE, PlayerState, Pml, Pmove, Stance, button, ev, pm_type, pmf};

/// rodata 0x8154ee4: friction under the landing stun.
const LAND_FRICTION_SCALE: f32 = f32::from_bits(0x3e99_999a);
/// rodata 0x8154ee8: the spectator's friction.
const SPECTATOR_FRICTION: f32 = 5.0;
/// rodata 0x8154f68 (a double): an `oldVelocity` shorter than this is no
/// reference for the inertia clamp.
const INERTIA_MIN_OLD_SQ: f64 = 0.0001;
/// rodata 0x8154f74: the sighted walk's and the lean's one move scale.
const SCALE_SIGHT: f32 = f32::from_bits(0x3ecc_cccd);
const SCALE_NOCLIP: f32 = 3.0;
const SCALE_UFO: f32 = 6.0;
/// rodata 0x8154fa0 and 0x8154fa4.
const SCALE_PRONE: f32 = f32::from_bits(0x3e19_999a);
const SCALE_CROUCH: f32 = f32::from_bits(0x3f26_6666);
/// rodata 0x8154fb8: the accel under the landing stun.
const LAND_ACCEL_SCALE: f32 = 0.25;
/// rodata 0x8154fbc: the dead body's drop per frame.
const DEAD_SPEED_DROP: f32 = 20.0;
/// The jump slowdown's timers and scales (`Jump_ApplySlowdown` 0x80dc818).
const JUMP_PENALTY_MS: i32 = 1800;
const JUMP_PENALTY_HIGH_MS: i32 = 1200;
const JUMP_PENALTY_HIGH_LANDING: f32 = 18.0;
const JUMP_PENALTY_SCALE: f32 = f32::from_bits(0x3f26_6666);
const JUMP_PENALTY_HIGH_SCALE: f32 = 0.5;

/// `PM_Friction` (0x80e156e).
pub(crate) fn friction(ps: &mut PlayerState, pml: &Pml, rules: &super::Rules) {
    let mut v = ps.velocity;
    if pml.walking {
        v.z = 0.0;
    }
    let speed = fpu::length(v);
    if speed < 1.0 {
        ps.velocity = Vec3::ZERO;
        return;
    }
    let mut drop = 0f32;
    if pml.walking
        && pml.ground_trace.surface_flags & 2 == 0
        && ps.pm_flags & pmf::TIME_KNOCKBACK == 0
    {
        let mut control = speed.max(rules.stopspeed);
        if ps.pm_flags & pmf::TIME_LAND != 0 {
            control *= LAND_FRICTION_SCALE;
        } else if ps.pm_flags & pmf::JUMPING != 0 {
            control = (d(control) * d(jump_reduce_friction(ps, rules))) as f32;
        }
        drop = (d(control) * d(rules.friction) * d(pml.frametime) + 0.0) as f32;
    }
    if ps.pm_type == pm_type::SPECTATOR {
        drop = (d(drop) + d(SPECTATOR_FRICTION) * d(speed) * d(pml.frametime)) as f32;
    }
    let mut k = speed - drop;
    if k < 0.0 {
        k = 0.0;
    }
    k /= speed;
    ps.velocity *= k;
}

/// The jump slowdown's curve (0x80dc8ca, 0x80dc9ee): `1 + t * 1.5 / 1700`
/// under 1700 ms, 2.5 after; 1 with `jump_slowdownEnable` off.
pub(crate) fn jump_curve(ps: &PlayerState, rules: &super::Rules) -> f32 {
    if !rules.jump_slowdown_enable {
        1.0
    } else if ps.pm_time < 1700 {
        let k = f32::from_bits(0x3a1a_33cd); // 1 / 1700
        (d(k) * 1.5 * f64::from(ps.pm_time as f32) + 1.0) as f32
    } else {
        2.5
    }
}

/// `Jump_ReduceFriction` (0x80dc91c): the curve, or the jump cleared once
/// its timer is past the slowdown.
fn jump_reduce_friction(ps: &mut PlayerState, rules: &super::Rules) -> f32 {
    if ps.pm_time < JUMP_PENALTY_MS + 1 {
        jump_curve(ps, rules)
    } else {
        jump_clear(ps);
        1.0
    }
}

/// The inertia clamp (0x80e17fe, test 0x80e170c): unless noclipping, a step
/// over `inertiaMax` is cut to it when the flat velocity it would make turns
/// past `inertiaAngle` (dot of the normalized flat directions) from
/// `oldVelocity`.
pub(crate) fn inertia_clamp(
    ps: &PlayerState,
    accel: f32,
    wishdir: Vec3,
    rules: &super::Rules,
) -> f32 {
    if ps.pm_type == pm_type::NOCLIP || accel <= rules.inertia_max {
        return accel;
    }
    let [ox, oy] = ps.old_velocity;
    if d(ox) * d(ox) + d(oy) * d(oy) < INERTIA_MIN_OLD_SQ {
        return accel;
    }
    let nx = (d(ps.velocity.x) + d(accel) * d(wishdir.x)) as f32;
    let ny = (d(ps.velocity.y) + d(accel) * d(wishdir.y)) as f32;
    let (ox, oy) = fpu::normalize2(ox, oy);
    let (nx, ny) = fpu::normalize2(nx, ny);
    let turn = (d(ox) * d(nx) + d(oy) * d(ny)) as f32;
    if turn < rules.inertia_angle {
        rules.inertia_max
    } else {
        accel
    }
}

/// `PM_Accelerate` (0x80e188e) off a ladder: Q3's, the rate floored at
/// `stopspeed`, the step through the inertia clamp. On a ladder (`pm_flags`
/// 0x20) Q2's: the velocity moved toward `wishdir * wishspeed` by at most
/// `accel * frametime * wishspeed`.
pub(crate) fn accelerate(pm: &mut Pmove, pml: &Pml, wishdir: Vec3, wishspeed: f32, accel: f32) {
    let ps = &mut *pm.ps;
    if ps.pm_flags & pmf::LADDER != 0 {
        let push = fpu::scale(wishdir, wishspeed) - ps.velocity;
        let (pushdir, pushlen) = fpu::normalize(push);
        let canpush = (d(accel) * d(pml.frametime) * d(wishspeed)) as f32;
        ps.velocity = fpu::ma(ps.velocity, canpush.min(pushlen), pushdir);
        return;
    }
    let current = fpu::dot(ps.velocity, wishdir) as f32;
    let add = wishspeed - current;
    if add <= 0.0 {
        return;
    }
    let control = wishspeed.max(pm.rules.stopspeed);
    let step = (d(accel) * d(pml.frametime) * d(control)) as f32;
    let step = step.min(add);
    let step = inertia_clamp(ps, step, wishdir, &pm.rules);
    ps.velocity = fpu::ma(ps.velocity, step, wishdir);
}

/// The 2-axis cmd scale (0x80e1b1e): air and spectator moves.
pub(crate) fn cmd_scale_flat(ps: &PlayerState, fm: i8, rm: i8, rules: &super::Rules) -> f32 {
    let (f, r) = (i32::from(fm), i32::from(rm));
    let total = fpu::sqrt(f64::from((r * r + f * f) as f32));
    let max = f.abs().max(r.abs());
    if max == 0 {
        return 0.0;
    }
    let s = (f64::from(max as f32) * f64::from(ps.speed as f32) / (127.0 * d(total))) as f32;
    type_scale(ps, s, rules)
}

/// The 3-axis cmd scale (0x80e1a02): noclip and the spectator's rise.
fn cmd_scale_3(ps: &PlayerState, a: f32, b: f32, c: f32, rules: &super::Rules) -> f32 {
    let max = a.abs().max(b.abs()).max(c.abs());
    if max == 0.0 {
        return 0.0;
    }
    let total = fpu::sqrt(d(c) * d(c) + d(b) * d(b) + d(a) * d(a));
    let s = (f64::from(ps.speed as f32) * d(max) / (127.0 * d(total))) as f32;
    type_scale(ps, s, rules)
}

/// The sight and lean scale and the fly types' (shared by both flat scales).
fn type_scale(ps: &PlayerState, mut s: f32, rules: &super::Rules) -> f32 {
    if ps.pm_flags & pmf::SIGHT_WALK != 0 || ps.leanf != 0.0 {
        s *= SCALE_SIGHT;
    }
    match ps.pm_type {
        pm_type::NOCLIP => s *= SCALE_NOCLIP,
        pm_type::UFO => s *= SCALE_UFO,
        pm_type::SPECTATOR => s *= rules.spectate_speed_scale,
        _ => {}
    }
    s
}

/// The eye's lerp fraction between two stance heights (0x80e3dae), 0 when
/// no lerp runs that way.
pub(crate) fn lerp_frac(ps: &PlayerState, from: i32, to: i32, time: i32) -> f32 {
    if ps.view_height_lerp_time == 0 {
        return 0.0;
    }
    let target = ps.view_height_lerp_target;
    let down = ps.view_height_lerp_down;
    if to == target && (to != 40 || (from == 11 && !down) || (from == 60 && down)) {
        let dur = super::stance::lerp_duration(target, down);
        ((time - ps.view_height_lerp_time) as f32 / dur as f32).clamp(0.0, 1.0)
    } else {
        0.0
    }
}

/// The walk's cmd scale (0x80e1c58): the back and strafe dvars, the stance
/// (blended over the eye's lerp between crouch and prone), the weapon and
/// the shellshock.
fn cmd_scale_walk(pm: &Pmove, fm: i8, rm: i8) -> f32 {
    let ps = &*pm.ps;
    let (f, r) = (i32::from(fm), i32::from(rm));
    let total = fpu::sqrt(f64::from((r * r + f * f) as f32));
    let fwd = if fm < 0 {
        (f64::from(fm) * d(pm.rules.back_speed_scale)) as f32
    } else {
        f32::from(fm)
    }
    .abs();
    let side = ((f64::from(rm) * d(pm.rules.strafe_speed_scale)) as f32).abs();
    let max = if fwd - side >= 0.0 { fwd } else { side };
    if max == 0.0 {
        return 0.0;
    }
    let mut s = (f64::from(ps.speed as f32) * d(max) / (127.0 * d(total))) as f32;
    if ps.pm_flags & pmf::SIGHT_WALK != 0 || ps.leanf != 0.0 {
        s *= SCALE_SIGHT;
    }
    match ps.pm_type {
        pm_type::NOCLIP => s *= SCALE_NOCLIP,
        pm_type::UFO => s *= SCALE_UFO,
        _ => {
            let t = pm.cmd.server_time;
            let down = lerp_frac(ps, 40, 11, t);
            let up = lerp_frac(ps, 11, 40, t);
            let blend = |a: f32, b: f32, f: f32| (d(a) * (1.0 - d(f)) + d(b) * d(f)) as f32;
            if down != 0.0 {
                s = (d(s) * d(blend(SCALE_CROUCH, SCALE_PRONE, down))) as f32;
            } else if up != 0.0 {
                s = (d(s) * d(blend(SCALE_PRONE, SCALE_CROUCH, up))) as f32;
            } else {
                match ps.effective_stance() {
                    Stance::Prone => s *= SCALE_PRONE,
                    Stance::Crouch => s *= SCALE_CROUCH,
                    Stance::Stand => {}
                }
            }
        }
    }
    if pm.move_scale > 0.0 {
        s *= pm.move_scale;
    }
    if pm.cmd.buttons & button::SHELLSHOCK != 0 {
        s *= SCALE_SIGHT;
    }
    s
}

/// The wish built off the view's flat forward and right.
fn flat_wish(pml: &Pml, fm: i8, rm: i8, axes: usize) -> Vec3 {
    let (f, r) = (f64::from(fm), f64::from(rm));
    let mut w = Vec3::ZERO;
    for i in 0..axes {
        w[i] = (d(pml.right[i]) * r + d(pml.forward[i]) * f) as f32;
    }
    w
}

/// `PM_WalkMove` (0x80e25a6).
pub(crate) fn walk_move(pm: &mut Pmove, pml: &mut Pml) {
    if pm.ps.pm_flags & pmf::JUMPING != 0 {
        jump_apply_slowdown(pm.ps, &pm.rules);
    }
    if jump_check(pm, pml) {
        air_move(pm, pml);
        return;
    }
    friction(pm.ps, pml, &pm.rules);
    let (fm, rm) = (pm.cmd.forward_move, pm.cmd.right_move);
    let scale = cmd_scale_walk(pm, fm, rm);
    // `PM_DamageScale_Walk` is 1 at the stock `player_dmgtimer_minScale` 0.
    let ft_ms = (1000.0 * d(pml.frametime)) as i32;
    pm.ps.damage_timer = (pm.ps.damage_timer - ft_ms).max(0);
    let n = pml.ground_trace.normal;
    pml.forward.z = 0.0;
    pml.right.z = 0.0;
    pml.forward = fpu::normalize(clip_velocity(pml.forward, n)).0;
    pml.right = fpu::normalize(clip_velocity(pml.right, n)).0;
    let (wishdir, len) = fpu::normalize(flat_wish(pml, fm, rm, 3));
    let wishspeed = (d(len) * d(scale)) as f32;
    let slick =
        pml.ground_trace.surface_flags & 2 != 0 || pm.ps.pm_flags & pmf::TIME_KNOCKBACK != 0;
    let mut accel = if slick {
        1.0
    } else {
        match pm.ps.effective_stance() {
            Stance::Prone => 19.0,
            Stance::Crouch => 12.0,
            Stance::Stand => 9.0,
        }
    };
    if pm.ps.pm_flags & pmf::TIME_LAND != 0 {
        accel *= LAND_ACCEL_SCALE;
    }
    accelerate(pm, pml, wishdir, wishspeed, accel);
    let ps = &mut *pm.ps;
    if slick {
        ps.velocity.z = (d(ps.velocity.z) - f64::from(ps.gravity) * d(pml.frametime)) as f32;
    }
    // The speed is kept across the ground clip.
    let speed = fpu::length(ps.velocity);
    let before = ps.velocity;
    ps.velocity = clip_velocity(ps.velocity, n);
    if fpu::dot(ps.velocity, before) > 0.0 {
        ps.velocity = fpu::scale(fpu::normalize(ps.velocity).0, speed);
    }
    if ps.velocity.x != 0.0 || ps.velocity.y != 0.0 {
        step_slide_move(pm, pml, false);
    }
    set_movement_dir(pm, pml);
}

/// `PM_AirMove` (0x80e23f8).
pub(crate) fn air_move(pm: &mut Pmove, pml: &mut Pml) {
    friction(pm.ps, pml, &pm.rules);
    let (fm, rm) = (pm.cmd.forward_move, pm.cmd.right_move);
    let scale = cmd_scale_flat(pm.ps, fm, rm, &pm.rules);
    pml.forward.z = 0.0;
    pml.right.z = 0.0;
    pml.forward = fpu::normalize(pml.forward).0;
    pml.right = fpu::normalize(pml.right).0;
    let (wishdir, len) = fpu::normalize(flat_wish(pml, fm, rm, 2));
    let wishspeed = (d(len) * d(scale)) as f32;
    accelerate(pm, pml, wishdir, wishspeed, 1.0);
    if pml.ground_plane {
        pm.ps.velocity = clip_velocity(pm.ps.velocity, pml.ground_trace.normal);
    }
    step_slide_move(pm, pml, true);
    set_movement_dir(pm, pml);
}

/// The spectator's fly (0x80e225e): the lean keys fly down (0x40) and up
/// (0x80), accel 8, clipped by the step slide.
pub(crate) fn fly_move(pm: &mut Pmove, pml: &mut Pml) {
    friction(pm.ps, pml, &pm.rules);
    let (fm, rm) = (pm.cmd.forward_move, pm.cmd.right_move);
    let scale = cmd_scale_flat(pm.ps, fm, rm, &pm.rules);
    let mut wish = Vec3::ZERO;
    if scale != 0.0 {
        for i in 0..3 {
            wish[i] = (f64::from(rm) * d(scale) * d(pml.right[i])
                + f64::from(fm) * d(scale) * d(pml.forward[i])) as f32;
        }
    }
    if pm.ps.speed != 0 {
        let up = cmd_scale_3(pm.ps, 0.0, 0.0, 127.0, &pm.rules);
        if pm.cmd.buttons & button::LEAN_LEFT != 0 {
            wish.z = (d(wish.z) - 127.0 * d(up)) as f32;
        }
        if pm.cmd.buttons & button::LEAN_RIGHT != 0 {
            wish.z = (d(wish.z) + 127.0 * d(up)) as f32;
        }
    }
    let (wishdir, wishspeed) = fpu::normalize(wish);
    accelerate(pm, pml, wishdir, wishspeed, 8.0);
    step_slide_move(pm, pml, false);
}

/// The noclip move (0x80e2aa2): its own friction at 1.5x, accel 9, the
/// origin moved by the velocity with nothing in the way. UFO (`pm_type` 3,
/// 0x80e2cec) is flown the same here, at its x6 scale.
pub(crate) fn noclip_move(pm: &mut Pmove, pml: &mut Pml) {
    let ps = &mut *pm.ps;
    ps.view_height_target = super::VIEW_STAND;
    let speed = fpu::length(ps.velocity);
    if speed < 1.0 {
        ps.velocity = Vec3::ZERO;
    } else {
        let fric = (1.5 * d(pm.rules.friction)) as f32;
        let control = speed.max(pm.rules.stopspeed);
        let drop = (d(control) * d(fric) * d(pml.frametime) + 0.0) as f32;
        let k = ((speed - drop).max(0.0)) / speed;
        ps.velocity = fpu::scale(ps.velocity, k);
    }
    let (fm, rm) = (f32::from(pm.cmd.forward_move), f32::from(pm.cmd.right_move));
    let mut um = 0f32;
    if pm.cmd.buttons & button::LEAN_RIGHT != 0 {
        um = 127.0;
    }
    if pm.cmd.buttons & button::LEAN_LEFT != 0 {
        um -= 127.0;
    }
    let scale = cmd_scale_3(ps, fm, rm, um, &pm.rules);
    let mut wish = Vec3::ZERO;
    for i in 0..3 {
        wish[i] =
            (d(pml.up[i]) * d(um) + d(pml.right[i]) * d(rm) + d(pml.forward[i]) * d(fm)) as f32;
    }
    let (wishdir, len) = fpu::normalize(wish);
    accelerate(pm, pml, wishdir, (d(len) * d(scale)) as f32, 9.0);
    pm.ps.origin = fpu::ma(pm.ps.origin, pml.frametime, pm.ps.velocity);
}

/// The dead move (0x80e2a22): a walking body loses 20 a frame.
pub(crate) fn dead_move(ps: &mut PlayerState, pml: &Pml) {
    if !pml.walking {
        return;
    }
    let speed = fpu::length(ps.velocity) - DEAD_SPEED_DROP;
    if speed <= 0.0 {
        ps.velocity = Vec3::ZERO;
    } else {
        ps.velocity = fpu::scale(fpu::normalize(ps.velocity).0, speed);
    }
}

/// `Jump_ApplySlowdown` (0x80dc818): past 1800 ms the jump clears at 0.65;
/// at `pm_time` 0, a landing under the jump's start plus 18 starts 1800 ms
/// at 0.65 and a higher one 1200 ms at 0.5.
fn jump_apply_slowdown(ps: &mut PlayerState, rules: &super::Rules) {
    let mut k = 1.0f32;
    if ps.pm_time <= JUMP_PENALTY_MS {
        if ps.pm_time == 0 {
            if d(ps.origin.z) < d(JUMP_PENALTY_HIGH_LANDING) + d(ps.jump_origin_z) {
                ps.pm_time = JUMP_PENALTY_MS;
                k = JUMP_PENALTY_SCALE;
            } else {
                ps.pm_time = JUMP_PENALTY_HIGH_MS;
                k = JUMP_PENALTY_HIGH_SCALE;
            }
        }
    } else {
        jump_clear(ps);
        k = JUMP_PENALTY_SCALE;
    }
    if !rules.jump_slowdown_enable {
        k = 1.0;
    }
    ps.velocity = fpu::scale(ps.velocity, k);
}

/// `Jump_Check` (0x80dcca8): the jump key's edge against `oldcmd`, 500 ms
/// after the last, no respawn latch, no mantle, alive and standing. A held
/// key is taken off the cmd.
pub(crate) fn jump_check(pm: &mut Pmove, pml: &mut Pml) -> bool {
    let ps = &*pm.ps;
    if pm.cmd.server_time - ps.jump_time < 500
        || ps.pm_flags & (pmf::RESPAWNED | pmf::MANTLE) != 0
        || ps.pm_type >= pm_type::DEAD
        || ps.effective_stance() != Stance::Stand
        || pm.cmd.buttons & button::JUMP == 0
    {
        return false;
    }
    if pm.oldcmd.buttons & button::JUMP != 0 {
        pm.cmd.buttons &= !button::JUMP;
        return false;
    }
    jump_start(pm, pml);
    let surf = super::ground::surface_type(&pml.ground_trace);
    if pm.ps.pm_flags & pmf::LADDER != 0 {
        pm.ps.add_event(ev::JUMP_LADDER, 0);
        super::ladder::push_off(pm, pml);
    } else if surf != 0 {
        pm.ps.add_event(ev::JUMP + surf, 0);
    }
    super::anim::event(pm, Event::takeoff(pm.cmd.forward_move), false, true);
    true
}

/// The takeoff (0x80dca40): `vz^2 = 2 g h`, divided by the slowdown's curve
/// while a slowdown runs; the jump's start height kept for the cap.
fn jump_start(pm: &mut Pmove, pml: &mut Pml) {
    let ps = &mut *pm.ps;
    let h = pm.rules.jump_height;
    let mut v2 = (f64::from(ps.gravity) * (d(h) + d(h))) as f32;
    if ps.pm_flags & pmf::JUMPING != 0 && ps.pm_time <= JUMP_PENALTY_MS {
        v2 /= jump_curve(ps, &pm.rules);
    }
    pml.ground_plane = false;
    pml.almost_ground_plane = false;
    pml.walking = false;
    ps.ground_entity_num = ENTITYNUM_NONE;
    ps.jump_time = pm.cmd.server_time;
    ps.jump_origin_z = ps.origin.z;
    ps.pm_flags |= pmf::JUMPING;
    ps.pm_time = 0;
    ps.velocity.z = fpu::sqrt(d(v2));
    ps.aim_spread_scale += pm.rules.jump_spread_add;
    if ps.aim_spread_scale > 255.0 {
        ps.aim_spread_scale = 255.0;
    }
}

/// `PM_SetMovementDir` (0x80e1f8c): the move's yaw off the view, clamped to
/// 90 either way; a prone body's off its heading.
fn set_movement_dir(pm: &mut Pmove, pml: &Pml) {
    let ps = &mut *pm.ps;
    let clamp = |a: i32| {
        if a.abs() > 90 {
            if a < 1 { -90 } else { 90 }
        } else {
            a
        }
    };
    if ps.pm_flags & pmf::PRONE != 0 && ps.e_flags & super::ef::TURRET == 0 {
        let a = fpu::angle_delta(ps.prone_direction, ps.view_angles[1]) as i32;
        ps.movement_dir = clamp(a) as i8 as i32;
        return;
    }
    if ps.pm_flags & pmf::LADDER != 0 {
        return;
    }
    let delta = ps.origin - pml.previous_origin;
    if (pm.cmd.forward_move != 0 || pm.cmd.right_move != 0)
        && ps.ground_entity_num != ENTITYNUM_NONE
    {
        let len = fpu::length(delta);
        if len != 0.0 && d(5.0) * d(pml.frametime) < d(len) {
            let dir = fpu::normalize_to(delta).0;
            let yaw = fpu::vectoyaw(dir);
            let mut a = fpu::angle_delta(yaw, ps.view_angles[1]) as i32;
            if pm.cmd.forward_move < 0 {
                a = fpu::angle_normalize180(180.0 + a as f32) as i32;
            }
            ps.movement_dir = clamp(a) as i8 as i32;
            return;
        }
    }
    ps.movement_dir = 0;
}
