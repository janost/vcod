//! Friction, the accelerate and its inertia clamp, the move scales, the
//! walk, air, fly, noclip and dead moves and the jump
//! (docs/research/cod4-pmove.md 3, 6).

use glam::Vec3;

use super::fpu::{self, d};
use super::slide::{clip_velocity, jump_clear, project_velocity, step_slide_move};
use super::{
    ENTITYNUM_NONE, PlayerState, Pml, Pmove, Rules, Stance, VIEW_CROUCH, VIEW_PRONE, button, ev,
    pm_type, pmf,
};

/// Friction under the landing stun.
const LAND_FRICTION_SCALE: f32 = f32::from_bits(0x3e99_999a);
/// The spectator's friction.
const SPECTATOR_FRICTION: f32 = 5.0;
/// An `oldVelocity` shorter than this (squared) is no reference for the
/// inertia clamp (0x820dff8).
const INERTIA_MIN_OLD_SQ: f64 = 0.0001;
/// The sighted walk's, the lean's and a slowing shellshock's scale
/// (0x820969c).
const SCALE_SIGHT: f32 = f32::from_bits(0x3ecc_cccd);
/// Noclip's and UFO's (0x8209658, 0x820aacc).
const SCALE_NOCLIP: f32 = 3.0;
const SCALE_UFO: f32 = 6.0;
/// The prone and crouched moves' scales (0x820deac, 0x820a310).
const SCALE_PRONE: f32 = f32::from_bits(0x3e19_999a);
const SCALE_CROUCH: f32 = f32::from_bits(0x3f26_6666);
/// The accel under the landing stun (0x820966c).
const LAND_ACCEL_SCALE: f32 = 0.25;
/// The dead body's drop per frame.
const DEAD_SPEED_DROP: f32 = 20.0;
/// A landing out of a jump holds the jump flag for a penalty: its length
/// in `pm_time` and the cut to the speed it lands with (cod4-pmove.md 10).
struct Penalty {
    ms: i32,
    cut: f32,
}

/// Landing lower than 18 over the takeoff (0x820a314): 0.65 (0x820a310).
const LOW_LANDING: Penalty = Penalty {
    ms: 1800,
    cut: f32::from_bits(0x3f26_6666),
};
/// Landing at least that high: 0.5 (0x8209628).
const HIGH_LANDING: Penalty = Penalty { ms: 1200, cut: 0.5 };
const HIGH_LANDING_RISE: f32 = 18.0;

/// `PM_Friction`: the speed, the drop and the scale kept on the stack,
/// each velocity component stored.
pub(crate) fn friction(ps: &mut PlayerState, pml: &Pml, rules: &super::Rules) {
    let mut v = ps.velocity;
    if pml.walking {
        v.z = 0.0;
    }
    // The inlined `sqrtf` roots the kept sum and keeps the root (0x8056fa0).
    let speed = fpu::dot(v, v).sqrt();
    if speed < 1.0 {
        ps.velocity = Vec3::ZERO;
        return;
    }
    let mut drop = 0f64;
    if ps.pm_flags & pmf::MELEE_CHARGE != 0 {
        // The lunge's own friction, on the ground or off it (0x8056fb8).
        drop = d(rules.melee_charge_friction) * d(pml.frametime);
    } else if pml.walking
        && pml.ground_trace.surface_flags & 2 == 0
        && ps.pm_flags & pmf::TIME_KNOCKBACK == 0
    {
        let mut control = speed.max(d(rules.stopspeed));
        if ps.pm_flags & pmf::TIME_LAND != 0 {
            control *= d(LAND_FRICTION_SCALE);
        } else if ps.pm_flags & pmf::JUMPING != 0 {
            control *= penalty_friction(ps, rules);
        }
        drop += control * d(rules.friction) * d(pml.frametime);
    }
    if ps.pm_type == pm_type::SPECTATOR {
        drop += speed * d(SPECTATOR_FRICTION) * d(pml.frametime);
    }
    let k = (speed - drop).max(0.0) / speed;
    ps.velocity = fpu::scale_kept(ps.velocity, k);
}

/// The slowdown's factor `pm_time` into a penalty: rising from 1 by 1.5
/// over 1700 ms (1/1700 as the float 0x820a320), then held at 2.5
/// (0x820a31c); 1 with `jump_slowdownEnable` off. Returned on the x87
/// stack (cod4-pmove.md 1).
fn slowdown_factor(pm_time: i32, rules: &super::Rules) -> f64 {
    const RAMP_MS: i32 = 1700;
    let per_ms = d(f32::from_bits(0x3a1a_33cd));
    match pm_time {
        _ if !rules.jump_slowdown_enable => 1.0,
        t if t >= RAMP_MS => 2.5,
        t => f64::from(t) * 1.5 * per_ms + 1.0,
    }
}

/// The friction's multiplier on a jumping body (0x80509e6): the slowdown
/// factor while the penalty runs; once `pm_time` is past the longest one
/// the jump is over and friction is plain.
fn penalty_friction(ps: &mut PlayerState, rules: &super::Rules) -> f64 {
    match ps.pm_time {
        t if t > LOW_LANDING.ms => {
            jump_clear(ps);
            1.0
        }
        t => slowdown_factor(t, rules),
    }
}

/// The inertia clamp inside `PM_Accelerate` (`cod4_lnxded` 0x80592e8,
/// `inertiaMax` and `inertiaAngle` read at 0x8059372 and 0x80594cb): a
/// step longer than `inertiaMax` comes back as `inertiaMax` when the flat
/// velocity it would leave turns from `oldVelocity` by more than
/// `inertiaAngle` allows, the turn read as the dot against the lengths'
/// product. A noclip body, or an `oldVelocity` under 0.01 long
/// (0x820dff8), keeps its step. Inlined: the step, the velocity it would
/// leave, both squares, their product's root and the dot all stay on the
/// stack (0x805937e..0x80594db).
pub(crate) fn inertia_clamp(ps: &PlayerState, step: f64, wishdir: Vec3, rules: &Rules) -> f64 {
    if ps.pm_type == pm_type::NOCLIP || step <= d(rules.inertia_max) {
        return step;
    }
    let before = ps.old_velocity.map(d);
    let before_sq = before[0] * before[0] + before[1] * before[1];
    if before_sq < INERTIA_MIN_OLD_SQ {
        return step;
    }
    let after = [0, 1].map(|i| step * d(wishdir[i]) + d(ps.velocity[i]));
    let lengths = ((after[0] * after[0] + after[1] * after[1]) * before_sq).sqrt();
    let allowed = d(rules.inertia_angle) * lengths;
    if before[0] * after[0] + before[1] * after[1] >= allowed {
        step
    } else {
        d(rules.inertia_max)
    }
}

/// The share of `ps.speed` a set of keys asks for: the longest key over
/// the keys' length, out of 127 (0x820ded8). `None` with no key held.
fn key_share(speed: i32, longest: f64, keys: &[f32]) -> Option<f64> {
    if longest == 0.0 {
        return None;
    }
    let length = keys.iter().map(|&k| d(k) * d(k)).sum::<f64>().sqrt();
    Some(f64::from(speed) * longest / (length * 127.0))
}

/// Multiplies `base` by each factor present, in order, on the x87 stack.
fn apply(base: f64, factors: impl IntoIterator<Item = Option<f64>>) -> f64 {
    factors.into_iter().flatten().fold(base, |s, k| s * k)
}

/// The sighted walk's and the lean's 0.4 (0x820969c).
fn sighted(ps: &PlayerState, prone_sighted: bool) -> Option<f64> {
    (ps.pm_flags & pmf::SIGHT_WALK != 0 || ps.leanf != 0.0 || prone_sighted).then(|| d(SCALE_SIGHT))
}

/// The scale off the keys for every move but the walk: `PM_CmdScale` (the
/// air, the ladder and the fly) on the move keys, `PM_MoveScale` (noclip,
/// the fly's rise) on three. The sighted walk and the lean take 0.4, noclip
/// 3 (0x8209658), UFO 6 (0x820aacc) and the spectator
/// `player_spectateSpeedScale`. Returned unrounded (cod4-pmove.md 1).
/// Inlined in `PM_AirMove` (0x805958c, its 127 read at 0x80597e1) and in
/// `PmoveSingle`'s fly and noclip (0x805bd14).
pub(crate) fn free_scale(ps: &PlayerState, rules: &Rules, keys: &[f32]) -> f64 {
    let longest = keys.iter().fold(0f32, |m, k| m.max(k.abs()));
    let Some(base) = key_share(ps.speed, d(longest), keys) else {
        return 0.0;
    };
    let kind = match ps.pm_type {
        pm_type::NOCLIP => Some(d(SCALE_NOCLIP)),
        pm_type::UFO => Some(d(SCALE_UFO)),
        pm_type::SPECTATOR => Some(d(rules.spectate_speed_scale)),
        _ => None,
    };
    apply(base, [sighted(ps, false), kind])
}

/// `PM_CmdScaleForStance` (0x8056c72): prone 0.15 (0x820deac), crouch 0.65
/// (0x820a310), standing 1. While the eye lerps down to prone, or up from
/// prone to the crouch, the two crouch-prone scales mix over the lerp's
/// 400 ms (0x820dea8); a lerp not yet started reads the stance.
pub(crate) fn stance_speed(ps: &PlayerState, time: i32) -> f32 {
    let mix = match (ps.view_height_lerp_target, ps.view_height_lerp_down) {
        _ if ps.view_height_lerp_time == 0 => None,
        (VIEW_PRONE, _) => Some((SCALE_CROUCH, SCALE_PRONE)),
        (VIEW_CROUCH, false) => Some((SCALE_PRONE, SCALE_CROUCH)),
        _ => None,
    };
    if let Some((from, to)) = mix {
        // The fraction stays on the stack (0x8056cd6..0x8056d56).
        let done = (f64::from(time - ps.view_height_lerp_time) / 400.0).clamp(0.0, 1.0);
        if done != 0.0 {
            return (done * d(to) + (1.0 - done) * d(from)) as f32;
        }
    }
    match ps.effective_stance() {
        Stance::Stand => 1.0,
        Stance::Crouch => SCALE_CROUCH,
        Stance::Prone => SCALE_PRONE,
    }
}

/// `PM_DamageScale_Walk` (0x8056046, inlined at 0x8059a8f): a hurt body
/// walks slower by `player_dmgtimer_minScale` for every
/// `player_dmgtimer_maxTime` ms of `damageTimer` left. 1 at the stock
/// minimum 0, with no timer or with no maximum. Only the result is
/// stored (0x8059d81).
fn hurt_scale(timer: i32, rules: &Rules) -> f32 {
    if timer == 0 || rules.dmgtimer_max_time == 0.0 {
        return 1.0;
    }
    let per_ms = -d(rules.dmgtimer_min_scale) / d(rules.dmgtimer_max_time);
    (1.0 + f64::from(timer) * per_ms) as f32
}

/// `PM_CmdScale_Walk`, inlined in `PM_WalkMove` (0x805987e..0x8059a89).
/// The keys' share takes `player_backSpeedScale` on a back key and
/// `player_strafeSpeedScale` on a side key; then, in turn, the sighted
/// 0.4 (also prone with the weapon raised), the sprint's
/// `player_sprintSpeedScale`, noclip's 3, UFO's 6 or the stance's scale,
/// the weapon's `moveSpeedScale` (its `adsMoveSpeedScale` while sighted or
/// when it has none), a slowing shellshock's 0.4 and
/// `moveSpeedScaleMultiplier`. The product goes through memory around
/// each call and at the end, so it comes back a float (cod4-pmove.md 6).
fn walk_scale(pm: &Pmove, fm: i8, rm: i8) -> f32 {
    let ps = &*pm.ps;
    let rules = &pm.rules;
    // Each scaled key is stored (0x805999b, 0x8059b08, 0x80599b6).
    let ahead = if fm < 0 {
        (f64::from(fm) * d(rules.back_speed_scale)).abs() as f32
    } else {
        f32::from(fm).abs()
    };
    let aside = (d(rules.strafe_speed_scale) * f64::from(rm)).abs() as f32;
    let keys = [f32::from(fm), f32::from(rm)];
    let Some(base) = key_share(ps.speed, d(ahead.max(aside)), &keys) else {
        return 0.0;
    };
    let prone_sighted = ps.pm_flags & pmf::PRONE != 0 && ps.weapon_pos_frac > 0.0;
    let sprint = (ps.pm_flags & pmf::SPRINTING != 0).then(|| d(rules.sprint_speed_scale));
    let s = apply(base, [sighted(ps, prone_sighted), sprint]);
    let s = match ps.pm_type {
        pm_type::NOCLIP => s * d(SCALE_NOCLIP),
        pm_type::UFO => s * d(SCALE_UFO),
        // Stored across `PM_CmdScaleForStance` (0x8059dad).
        _ => d(s as f32) * d(stance_speed(ps, pm.cmd.server_time)),
    };
    // Stored across the weapon def's lookup (0x8059a2c).
    let mut s = d(s as f32);
    let w = pm.weapon;
    let raised = ps.pm_flags & pmf::SIGHT_WALK != 0 || prone_sighted;
    let weapon = match ps.weapon {
        0 => None,
        _ if w.move_speed_scale > 0.0 && !raised => Some(d(w.move_speed_scale)),
        _ => (w.ads_move_speed_scale > 0.0).then(|| d(w.ads_move_speed_scale)),
    };
    s = apply(s, [weapon]);
    // `bg_shock_movement`: the record's slowdown (cod4-pmove.md 6), the
    // product stored across the record's lookup (0x8059dce).
    if ps.pm_flags & pmf::SHELLSHOCKED != 0 {
        s = d(s as f32);
        let slot = ps.shellshock_index;
        if (1..16).contains(&slot) && rules.shock_slows >> slot & 1 != 0 {
            s *= d(SCALE_SIGHT);
        }
    }
    // Stored once more (0x8059a89).
    (s * d(ps.move_speed_scale_multiplier)) as f32
}

/// The walk's ground acceleration: 1 on a slick surface or under
/// knockback, else 19, 12 or 9 by stance (0x820dedc, 0x820dee0, 0x820dee4),
/// a quarter of it while a hard landing lasts.
fn walk_accel(ps: &PlayerState, slick: bool) -> f32 {
    let accel = if slick {
        1.0
    } else {
        match ps.effective_stance() {
            Stance::Stand => 9.0,
            Stance::Crouch => 12.0,
            Stance::Prone => 19.0,
        }
    };
    if ps.pm_flags & pmf::TIME_LAND != 0 {
        accel * LAND_ACCEL_SCALE
    } else {
        accel
    }
}

/// The move keys along the view's forward and right laid flat: the wish
/// direction and its length.
fn flat_wish(pml: &mut Pml, fm: i8, rm: i8) -> (Vec3, f32) {
    for axis in [&mut pml.forward, &mut pml.right] {
        axis.z = 0.0;
        *axis = fpu::normalize2v(*axis);
    }
    let (f, r) = (d(f32::from(fm)), d(f32::from(rm)));
    let along = |i: usize| (f * d(pml.forward[i]) + r * d(pml.right[i])) as f32;
    fpu::normalize_to(Vec3::new(along(0), along(1), 0.0))
}

/// `PM_Accelerate` off a ladder: Q3's, the rate floored at `stopspeed`, the
/// step through the inertia clamp. On a ladder Q2's: the velocity moved
/// toward `wishdir * wishspeed` by at most `accel * frametime * wishspeed`.
pub(crate) fn accelerate(pm: &mut Pmove, pml: &Pml, wishdir: Vec3, wishspeed: f32, accel: f32) {
    let ps = &mut *pm.ps;
    if ps.pm_flags & pmf::LADDER != 0 {
        let push = fpu::scale(wishdir, wishspeed) - ps.velocity;
        let (pushdir, pushlen) = fpu::normalize(push);
        let canpush = (d(accel) * d(pml.frametime) * d(wishspeed)) as f32;
        ps.velocity = fpu::ma(ps.velocity, canpush.min(pushlen), pushdir);
        return;
    }
    let add = d(wishspeed) - fpu::dot(ps.velocity, wishdir);
    if add <= 0.0 {
        return;
    }
    let control = d(wishspeed.max(pm.rules.stopspeed));
    // The step stays on the stack into the add (0x8059366..0x80593c7).
    let step = (d(accel) * d(pml.frametime) * control).min(add);
    let step = inertia_clamp(ps, step, wishdir, &pm.rules);
    ps.velocity = fpu::ma_kept(ps.velocity, step, wishdir);
}

/// `PM_WalkMove` (0x805987e).
pub(crate) fn walk_move(pm: &mut Pmove, pml: &mut Pml) {
    if pm.ps.pm_flags & pmf::JUMPING != 0 {
        landing_penalty(pm.ps, &pm.rules);
    }
    if pm.ps.pm_flags & pmf::SPRINTING != 0 {
        // `player_sprintStrafeSpeedScale` cuts the side key itself: the
        // product stored as a float, then truncated (0x80598c0).
        let rm = f32::from(pm.cmd.right_move) * pm.rules.sprint_strafe_speed_scale;
        pm.cmd.right_move = rm as i32 as i8;
    }
    if jump_check(pm, pml) {
        air_move(pm, pml);
        return;
    }
    friction(pm.ps, pml, &pm.rules);

    let (fm, rm) = (pm.cmd.forward_move, pm.cmd.right_move);
    let scale = d(walk_scale(pm, fm, rm)) * d(hurt_scale(pm.ps.damage_timer, &pm.rules));
    // The frame's ms, stored as a float before the truncation (0x8059b3e).
    let spent = (1000.0 * pml.frametime) as i32;
    pm.ps.damage_timer = (pm.ps.damage_timer - spent).max(0);

    let (dir, keys) = flat_wish(pml, fm, rm);
    let plane = pml.ground_trace.normal;
    let wishdir = project_velocity(dir, plane);
    let slick =
        pml.ground_trace.surface_flags & 2 != 0 || pm.ps.pm_flags & pmf::TIME_KNOCKBACK != 0;
    let accel = walk_accel(pm.ps, slick);
    accelerate(pm, pml, wishdir, (d(keys) * scale) as f32, accel);

    let ps = &mut *pm.ps;
    if slick {
        ps.velocity.z = (d(ps.velocity.z) - d(pml.frametime) * f64::from(ps.gravity)) as f32;
    }
    ps.velocity = project_velocity(ps.velocity, plane);
    if ps.velocity.truncate() != glam::Vec2::ZERO {
        step_slide_move(pm, pml, false);
    }
    update_movement_dir(pm, pml);
}

/// `PM_AirMove`.
pub(crate) fn air_move(pm: &mut Pmove, pml: &mut Pml) {
    friction(pm.ps, pml, &pm.rules);
    let (fm, rm) = (pm.cmd.forward_move, pm.cmd.right_move);
    let scale = free_scale(pm.ps, &pm.rules, &[f32::from(fm), f32::from(rm)]);
    pml.forward.z = 0.0;
    pml.right.z = 0.0;
    pml.forward = fpu::normalize(pml.forward).0;
    pml.right = fpu::normalize(pml.right).0;
    let (f, r) = (f64::from(fm), f64::from(rm));
    let wish = Vec3::new(
        (d(pml.forward.x) * f + d(pml.right.x) * r) as f32,
        (d(pml.forward.y) * f + d(pml.right.y) * r) as f32,
        0.0,
    );
    let (wishdir, len) = fpu::normalize(wish);
    let wishspeed = (d(len) * scale) as f32;
    accelerate(pm, pml, wishdir, wishspeed, 1.0);
    if pml.ground_plane {
        pm.ps.velocity = clip_velocity(pm.ps.velocity, pml.ground_trace.normal);
    }
    step_slide_move(pm, pml, true);
    update_movement_dir(pm, pml);
}

/// The spectator's fly, inlined in `PmoveSingle` (0x805bd14; the wish at
/// 0x805db62): the move keys along a forward that is `(0, 0, 1) x right`,
/// so the pitch never lifts it, and the right; the lean keys fly down
/// (0x40) and up (0x80) at the three-key rate of a full key; accel 8
/// (0x820aac4), clipped by the step slide (cod4-pmove.md 6).
pub(crate) fn fly_move(pm: &mut Pmove, pml: &mut Pml) {
    friction(pm.ps, pml, &pm.rules);
    let (wishdir, wishspeed) = fpu::normalize(fly_wish(pm, pml));
    accelerate(pm, pml, wishdir, wishspeed, 8.0);
    step_slide_move(pm, pml, false);
}

fn fly_wish(pm: &Pmove, pml: &Pml) -> Vec3 {
    let ps = &*pm.ps;
    let (fm, rm) = (f32::from(pm.cmd.forward_move), f32::from(pm.cmd.right_move));
    let scale = free_scale(ps, &pm.rules, &[fm, rm]);
    let mut wish = Vec3::ZERO;
    if scale != 0.0 {
        let level = vcod_common::x87::cross(Vec3::Z, pml.right);
        wish = Vec3::from_array(std::array::from_fn(|i| {
            ((d(fm) * d(level[i]) + d(rm) * d(pml.right[i])) * scale) as f32
        }));
    }
    if ps.speed == 0 {
        return wish;
    }
    let climb = 127.0 * free_scale(ps, &pm.rules, &[0.0, 0.0, 127.0]);
    for (key, sign) in [(button::LEAN_LEFT, -1.0), (button::LEAN_RIGHT, 1.0)] {
        if pm.cmd.buttons & key != 0 {
            wish.z = (d(wish.z) + sign * climb) as f32;
        }
    }
    wish
}

/// The noclip move: its own friction at 1.5x, accel 9, the origin moved by
/// the velocity with nothing in the way. UFO is flown the same here, at its
/// x6 scale.
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
    let scale = free_scale(ps, &pm.rules, &[fm, rm, um]);
    let mut wish = Vec3::ZERO;
    for i in 0..3 {
        wish[i] =
            (d(pml.up[i]) * d(um) + d(pml.right[i]) * d(rm) + d(pml.forward[i]) * d(fm)) as f32;
    }
    let (wishdir, len) = fpu::normalize(wish);
    accelerate(pm, pml, wishdir, (d(len) * scale) as f32, 9.0);
    pm.ps.origin = fpu::ma(pm.ps.origin, pml.frametime, pm.ps.velocity);
}

/// `PM_DeadMove`: a walking body loses 20 a frame.
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

/// `Jump_ApplySlowdown` (0x8050958), run by each walked cmd while the
/// jump flag is up. The first (`pm_time` 0) starts the penalty its
/// landing height picks; a timer past the longest penalty ends the jump
/// with the low landing's cut; a running penalty cuts nothing. The cut
/// scales each velocity component, and only with `jump_slowdownEnable`.
fn landing_penalty(ps: &mut PlayerState, rules: &super::Rules) {
    let cut = match ps.pm_time {
        0 => {
            let high = d(ps.origin.z) >= d(ps.jump_origin_z) + d(HIGH_LANDING_RISE);
            let p = if high { HIGH_LANDING } else { LOW_LANDING };
            ps.pm_time = p.ms;
            p.cut
        }
        t if t > LOW_LANDING.ms => {
            jump_clear(ps);
            LOW_LANDING.cut
        }
        _ => return,
    };
    if rules.jump_slowdown_enable {
        ps.velocity = fpu::scale(ps.velocity, cut);
    }
}

/// `Jump_Check`: the jump key's edge against `oldcmd`, 500 ms after the
/// last, no `PMF_NO_JUMP`, no respawn latch, no mantle, alive and standing.
/// A held key is taken off the cmd.
pub(crate) fn jump_check(pm: &mut Pmove, pml: &mut Pml) -> bool {
    let ps = &*pm.ps;
    if ps.pm_flags & pmf::NO_JUMP != 0
        || pm.cmd.server_time - ps.jump_time < 500
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
    take_off(pm, pml);
    let takeoff = if pm.cmd.forward_move < 0 {
        super::anim::Event::JumpBk
    } else {
        super::anim::Event::Jump
    };
    super::anim::event(pm, takeoff, false, true);
    // `Jump_AddSurfaceEvent`: the surface rides the parm, a ladder's 0x15.
    if pm.ps.pm_flags & pmf::LADDER != 0 {
        pm.ps.add_event(ev::JUMP, 0x15);
        super::ladder::push_off(pm, pml);
    } else {
        let surf = super::ground::surface_type(&pml.ground_trace);
        if surf != 0 {
            pm.ps.add_event(ev::JUMP, surf);
        }
    }
    true
}

/// The takeoff `Jump_Check` inlines (0x8050aac..0x8050b5a): the speed
/// that rises `jump_height` under the body's gravity, its square divided
/// by the slowdown factor when a penalty still runs, kept on the stack
/// to `fsqrt` and stored once (cod4-pmove.md 1). The body leaves the
/// ground with the jump flagged from here, the stun and knockback timers
/// dropped, and the aim spread widened by `jump_spreadAdd` up to 255
/// (0x820a324).
fn take_off(pm: &mut Pmove, pml: &mut Pml) {
    let ps = &mut *pm.ps;
    let rise = d(pm.rules.jump_height);
    let mut speed_sq = (rise + rise) * f64::from(ps.gravity);
    let penalised = ps.pm_flags & pmf::JUMPING != 0 && ps.pm_time <= LOW_LANDING.ms;
    if penalised {
        speed_sq /= slowdown_factor(ps.pm_time, &pm.rules);
    }
    ps.velocity.z = speed_sq.sqrt() as f32;
    ps.jump_time = pm.cmd.server_time;
    ps.jump_origin_z = ps.origin.z;
    ps.ground_entity_num = ENTITYNUM_NONE;
    ps.pm_flags = (ps.pm_flags & !(pmf::TIME_LAND | pmf::TIME_KNOCKBACK)) | pmf::JUMPING;
    ps.pm_time = 0;
    ps.sprint.button_up_required = false;
    let spread = (d(ps.aim_spread_scale) + d(pm.rules.jump_spread_add)) as f32;
    ps.aim_spread_scale = if spread > 255.0 { 255.0 } else { spread };
    pml.walking = false;
    pml.ground_plane = false;
    pml.almost_ground_plane = false;
}

/// `PM_SetMovementDir` (0x8056d88): the move's yaw against the view, whole
/// degrees, at most 90 either way (`movementDir`, which the legs read). A
/// prone body off a turret takes its heading, a climber the ladder's
/// facing; otherwise a body on the ground with a key held that moved more
/// than 5 units a second (0x820aae0) takes its own move, turned round when
/// it backs. Anything else reads 0.
fn update_movement_dir(pm: &mut Pmove, pml: &Pml) {
    let dir = movement_yaw(pm, pml).map_or(0, |a| a.clamp(-90, 90));
    pm.ps.movement_dir = dir;
}

fn movement_yaw(pm: &Pmove, pml: &Pml) -> Option<i32> {
    let ps = &*pm.ps;
    let view = ps.view_angles[1];
    if ps.pm_flags & pmf::PRONE != 0 && ps.e_flags & super::ef::TURRET == 0 {
        return Some(fpu::angle_delta(ps.prone_direction, view) as i32);
    }
    if ps.pm_flags & pmf::LADDER != 0 {
        let facing = (d(fpu::vectoyaw(ps.ladder_vec)) + 180.0) as f32;
        return Some(fpu::angle_delta(facing, view) as i32);
    }
    let keyed = pm.cmd.forward_move != 0 || pm.cmd.right_move != 0;
    if !keyed || ps.ground_entity_num == ENTITYNUM_NONE {
        return None;
    }
    let moved = ps.origin - pml.previous_origin;
    let dist = fpu::length(moved);
    if dist == 0.0 || d(dist) <= 5.0 * d(pml.frametime) {
        return None;
    }
    let yaw = fpu::vectoyaw(fpu::normalize_to(moved).0);
    let delta = fpu::angle_delta(yaw, view) as i32;
    Some(if pm.cmd.forward_move < 0 {
        fpu::angle_normalize180(delta as f32 + 180.0) as i32
    } else {
        delta
    })
}

#[cfg(test)]
#[path = "moves_pins.rs"]
mod pins;
