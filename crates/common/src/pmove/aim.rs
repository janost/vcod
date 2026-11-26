//! Where a shot goes: `ClientThink_real`'s aim block, run once per usercmd
//! ahead of `Pmove`. The sight's idle sway, the turn lag, the gun's tilt
//! under movement, the walk bob and the damage kick are computed here from
//! the playerstate and the cmd clock, and a shot down a sight leaves along
//! the swayed direction rather than the raw view
//! (docs/research/cod11-combat.md, section 15). The client draws its view
//! off the same functions, so the reticle and the bullet agree.
//!
//! Every constant is retail's `.rodata`, cited by file offset in the doc.
//! Sines of `level.time` are taken in f64 the way the x87 takes them in
//! extended precision: at a few million milliseconds an f32 argument has
//! lost the fractional part that decides the phase.

use super::{PlayerState, Stance};
use crate::fguard::{clamp, near};
use crate::patch::Arith;
use crate::weapon::{AimDef, WeaponDef};
use crate::x87;
use glam::Vec3;

/// `client+0x2278..0x22a8`: what the block keeps between cmds. The gun-kick
/// spring at `+0x22ac` is [`GunKick`], kept by the caller and handed in
/// through [`AimInput::gun_kick`]: the client steps the cgame's copy, and a
/// 1.5 or UO server its own (1.1's never kicks it; cod15-movement.md 6).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct AimState {
    /// `ps.viewangles` the sway last saw (`+0x2278`).
    last_view: [f32; 3],
    /// The turn sway's position offset (`+0x2284`), lerped but never read
    /// for the aim; kept because it is the same state.
    sway_offset: [f32; 3],
    /// The turn sway's angle lag (`+0x2290`), subtracted off the gun angles.
    sway_angles: [f32; 3],
    /// The speed-driven tilt (`+0x229c`).
    rot: [f32; 3],
    /// The idle amount's stance factor, eased (`+0x22a8`).
    idle_factor: f32,
}

impl AimState {
    /// The turn sway's position lag, the cgame's `0x302095f0`: `[1]` is
    /// the yaw turn by `swayHorizScale`, `[2]` the pitch turn by
    /// `swayVertScale`.
    pub fn sway_offset(&self) -> [f32; 3] {
        self.sway_offset
    }
}

/// The last damage `P_DamageFeedback` recorded (combat doc, section 6,
/// steps 7, 8 and 11): `client+0x226c` and the two kick angles.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct DamageKick {
    /// 0 until the first hit, then `level.time - 20` of the last one.
    pub time_ms: i32,
    /// `client+0x2274`, the kick along the view.
    pub pitch: f32,
    /// `client+0x2270`, the kick across it.
    pub side: f32,
}

/// Everything the block reads that is not on the playerstate.
#[derive(Clone, Copy)]
pub struct AimInput<'a> {
    pub def: Option<&'a WeaponDef>,
    /// `ps.viewangles` in degrees, wire convention (pitch positive down).
    pub view: [f32; 3],
    /// The cmd's length, `ucmd.serverTime - ps.commandTime`, capped at 200.
    pub msec: i32,
    /// `level.time`.
    pub now_ms: i32,
    pub kick: DamageKick,
    /// The gun-kick spring's pitch and yaw ([`GunKick::angles`]), added to
    /// the gun of an `aimDownSight` weapon. Zero on a 1.1 server.
    pub gun_kick: [f32; 2],
    /// Multiplies the turn sway's four targets: 1 on a server, the client's
    /// `swayShellShockScale` eased in through a shellshock (cgame
    /// `0x300361f0`, combat doc 15.3).
    pub sway_scale: f32,
}

/// The gun-kick spring (game `0x39e14`, cgame `0x30012a60`; combat doc
/// 15.7): a shot adds to the speeds, and each axis springs back to centre.
/// Pitch and yaw in degrees, speeds in degrees a second.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct GunKick {
    pub angles: [f32; 2],
    pub speed: [f32; 2],
}

impl GunKick {
    /// `CG_WeaponFireRecoil`'s gun half (`0x30038850`): adds to the speeds.
    pub fn kick(&mut self, pitch: f32, yaw: f32) {
        self.speed[0] += pitch;
        self.speed[1] += yaw;
    }

    /// Steps the spring `secs` in 5 ms substeps, the four keys blended hip
    /// to sight by `frac` and `gunMaxPitch`/`gunMaxYaw` as the clamps. A
    /// weapon without `aimDownSight` is never stepped.
    pub fn step(&mut self, def: &WeaponDef, frac: f32, secs: f32) {
        if !def.aim_down_sight {
            return;
        }
        let a = &def.aim;
        let blend = |k: [f32; 2]| k[0] + (k[1] - k[0]) * frac;
        let accel = blend(a.gun_kick_accel);
        let speed_max = blend(a.gun_kick_speed_max);
        let speed_decay = blend(a.gun_kick_speed_decay);
        let static_decay = blend(a.gun_kick_static_decay);
        let max = [a.gun_max_pitch, a.gun_max_yaw];
        let mut left = secs;
        while left > 0.0 {
            let dt = if left > 0.005 { 0.005 } else { left };
            left = if left > 0.005 { left - 0.005 } else { 0.0 };
            let mut settled = true;
            for ((angle, speed), max) in self.angles.iter_mut().zip(&mut self.speed).zip(max) {
                settled &= spring_axis(
                    angle,
                    speed,
                    dt,
                    max,
                    accel,
                    speed_max,
                    speed_decay,
                    static_decay,
                );
            }
            if settled {
                break;
            }
        }
    }
}

/// One axis of the spring (cgame `0x30012910`). True, with both zeroed,
/// once the angle is under 0.25 and the speed under 1.
fn spring_axis(
    angle: &mut f32,
    speed: &mut f32,
    dt: f32,
    max: f32,
    accel: f32,
    speed_max: f32,
    speed_decay: f32,
    static_decay: f32,
) -> bool {
    if angle.abs() < 0.25 && speed.abs() < 1.0 {
        *angle = 0.0;
        *speed = 0.0;
        return true;
    }
    *angle += dt * *speed;
    if *angle > max {
        *angle = max;
        if *speed > 0.0 {
            *speed = 0.0;
        }
    } else if *angle < -max {
        *angle = -max;
        if *speed < 0.0 {
            *speed = 0.0;
        }
    }
    if *angle > 0.0 {
        *speed -= dt * accel;
    } else if *angle < 0.0 {
        *speed += dt * accel;
    }
    *speed -= dt * *speed * speed_decay;
    if *speed > 0.0 {
        *speed = (*speed - dt * static_decay).max(0.0);
    } else {
        *speed = (*speed + dt * static_decay).min(0.0);
    }
    *speed = clamp(*speed, -speed_max, speed_max);
    false
}

const PI64: f64 = std::f64::consts::PI;

// `AngleNormalize360` (`0x3eb30`) and `AngleNormalize180` (`0x3eb70`) take
// a float and keep the product into the truncating `fistp` (fldcw 0xc00). A
// lag the sway eases toward zero reaches it only because of the
// truncation: rounded to nearest, it stalls a few 65536ths out once a step
// is under half of one (combat doc 15.4). `AngleSubtract` is 0x3e968.
pub use crate::x87::{
    angle_normalize180_short as angle_normalize_180,
    angle_normalize360_short as angle_normalize_360, angle_subtract,
};

/// `GetLeanFraction` (`0x6ba64`): `(2 - |f|) * f`, the ease every kick
/// curve and the lean roll go through.
pub fn lean_fraction(f: f32) -> f32 {
    (2.0 - f.abs()) * f
}

/// `UnGetLeanFraction` (`0x6ba80`): `1 - sqrt(1 - f)`, the inverse of
/// [`lean_fraction`] over 0..1.
pub fn unget_lean_fraction(f: f32) -> f32 {
    1.0 - (1.0 - f).sqrt()
}

/// `AddLeanToPosition` (`0x6ba9c`): `pos` moved `lean_fraction(leanf) * 20`
/// along the right vector of `(0, yaw, lean_fraction(leanf) * 16)` degrees,
/// `yaw` the sim's radians of `ps.viewangles[YAW]`. The roll only tips the
/// offset down; nothing rolls the view (docs/research/cod11-mantle.md,
/// "Lean"). `arith` names the build: 1.1d keeps the fraction and the
/// distance on the stack, 1.5b and UO store both, and the Windows cgames
/// store both and turn through their own `AngleVectors`
/// (docs/research/cod11-movers.md 22.2.1.1).
pub fn add_lean(pos: Vec3, yaw: f32, leanf: f32, arith: Arith) -> Vec3 {
    if leanf == 0.0 {
        return pos;
    }
    let l = f64::from(leanf);
    let exact = (2.0 - l.abs()) * l;
    let f = match arith {
        Arith::Kept => exact,
        Arith::Stored | Arith::Msvc => f64::from(exact as f32),
    };
    let roll = (f * f64::from(LEAN_ROLL_SCALE)) as f32;
    let right = match arith {
        Arith::Kept | Arith::Stored => {
            x87::angle_vectors_of_radians(Vec3::new(0.0, yaw, x87::radians(roll)))[1]
        }
        Arith::Msvc => x87::angle_vectors_msvc_of_radians(Vec3::new(
            0.0,
            x87::radians_msvc(super::view_deg(yaw)),
            x87::radians_msvc(roll),
        ))[1],
    };
    let d = f * f64::from(LEAN_DIST_SCALE);
    let d = match arith {
        Arith::Kept => d,
        Arith::Stored | Arith::Msvc => f64::from(d as f32),
    };
    (pos.as_dvec3() + right.as_dvec3() * d).as_vec3()
}

/// What [`add_lean`] adds to a point at the origin under 1.1d's
/// arithmetic: the offset the bots aim from.
pub fn lean_offset(yaw: f32, leanf: f32) -> Vec3 {
    add_lean(Vec3::ZERO, yaw, leanf, Arith::Kept)
}

/// `AddLeanToPosition`'s roll and distance scales at every caller
/// (`G_AddLean` rodata 0x73130/0x7312c, `PM_UpdateLean` 0x70c68/0x70c64, the
/// cgame's view at 0x30032dc9, the server's snapshot origin at 0x808f2a9).
const LEAN_ROLL_SCALE: f32 = 16.0;
const LEAN_DIST_SCALE: f32 = 20.0;

/// The lerp step the sway takes: `k` of the distance, when the distance is
/// over 0.001 and the step does not overshoot; the target outright
/// otherwise.
fn approach(cur: f32, target: f32, k: f32) -> f32 {
    let delta = target - cur;
    let step = k * delta;
    if delta.abs() > 0.001 && step.abs() <= delta.abs() {
        cur + step
    } else {
        target
    }
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

fn stance_pick(ps: &PlayerState, stand: f32, ducked: f32, prone: f32) -> f32 {
    match ps.stance {
        Stance::Prone => prone,
        Stance::Crouch => ducked,
        Stance::Stand => stand,
    }
}

/// `BG_GetSpeed` (`0x3450c`): the horizontal speed, or on a ladder the
/// vertical one once 500 ms past the last jump and 0 before.
fn speed(ps: &PlayerState) -> f32 {
    if ps.on_ladder {
        if ps.since_jump_ms > 499.0 {
            ps.velocity.z
        } else {
            0.0
        }
    } else {
        ps.velocity.truncate().length()
    }
}

/// The retail `viewHeightTarget` compare the bob multipliers switch on:
/// the target is the stance's own height. Under 1.5's rules that is the
/// stepped eye target (1.5 cgame 0x30013520, game 0x40a54), not the stance
/// the flags ask for.
fn bob_stance_mult(ps: &PlayerState) -> f32 {
    let b = &ps.bob;
    let stance = match ps.rules {
        super::MoveRules::V1_1 => ps.stance,
        super::MoveRules::V1_5 | super::MoveRules::Uo => ps.effective_stance(),
    };
    match stance {
        Stance::Prone => b.prone,
        Stance::Crouch => b.ducked,
        Stance::Stand => b.standing,
    }
}

/// The walk bob on the eye, `CG_OffsetFirstPersonView`'s (cgame 1.1
/// 0x30032ae0, 1.5 0x30034180): up by `BG_GetVerticalBobFactor` (cgame 1.1
/// 0x30032a10, 1.5 0x30012f00, game 1.5 0x30a54) and along the view's
/// right by `BG_GetHorizontalBobFactor` (0x30032a90, 0x30012f80, 0x30b37),
/// each amplitude the speed times the stance's `cg_bobAmplitude*` capped at
/// `cg_bobMax`. `view` is the drawn view after the kick, wire degrees.
pub fn eye_bob(ps: &PlayerState, view: [f32; 3]) -> Vec3 {
    let cycle = bob_cycle(ps, false);
    let amp = (speed(ps) * bob_stance_mult(ps)).min(ps.bob.max);
    let up =
        (((2.0 * cycle).sin() as f32 + (PI64 / 2.0 + cycle * 4.0).sin() as f32 * 0.2) * 0.75) * amp;
    let side = cycle.sin() as f32 * amp;
    let [_, right, _] = x87::angle_vectors_msvc(view.into());
    Vec3::new(right.x * side, right.y * side, right.z * side + up)
}

/// The walk bob 1.5's and UO's `CalcMuzzlePoint` (1.5 game 0x7e91f, UO
/// 0x7b8ea) add to a shot's or a throw's start: [`eye_bob`]'s two factors
/// at the game module's precision (`BG_GetBobCycle` 1.5 0x40a05 rounds the
/// cycle to a float), the side one along the right of `ps.viewangles`
/// (combat doc 2.1.1). Returns `at` with the
/// bob added, up first, in retail's order.
pub fn add_muzzle_bob(ps: &PlayerState, at: Vec3) -> Vec3 {
    let f = std::f32::consts::PI * (f32::from(ps.bob_cycle) / 255.0);
    let cycle = ((2.0 * PI64) as f32 as f64 + f as f64 + f as f64) as f32;
    let amp = (speed(ps) * bob_stance_mult(ps)).min(ps.bob.max);
    let c = f64::from(cycle);
    let up = (((c + c).sin() as f32 as f64
        + f64::from(0.2f32) * (f64::from((PI64 / 2.0) as f32) + 4.0 * c).sin() as f32 as f64)
        * 0.75
        * f64::from(amp)) as f32;
    let side = c.sin() as f32 * amp;
    // The sim's radians are the game module's `AngleVectors` radians of
    // `ps.viewangles` (`cmd::apply_view`).
    let [_, right, _] = x87::angle_vectors_of_radians(Vec3::new(-ps.pitch, ps.yaw, 0.0));
    let at = Vec3::new(at.x, at.y, at.z + up);
    at + right * side
}

/// `BG_CalculateWeaponPosition_Sway` (`0x3a548`): the gun lags a turn. The
/// per-axis view delta since the last cmd, clamped to `swayMaxAngle`, is the
/// target the offset and the angle lag ease toward at `swayLerpSpeed` per
/// second. A scoped weapon (`adsOverlayReticle`) leaves the state alone,
/// the last view included, for as long as the sight is up at all.
fn sway(
    ps: &PlayerState,
    aim: &AimDef,
    ads_weapon: bool,
    st: &mut AimState,
    view: [f32; 3],
    msec: i32,
    scale: f32,
) {
    let frac = ps.weapon_pos_frac;
    if ads_weapon && frac > 0.0 && aim.ads_overlay_reticle {
        return;
    }
    let pick = |pair: [f32; 2]| {
        if ads_weapon {
            lerp(pair[0], pair[1], frac)
        } else {
            pair[0]
        }
    };
    let max_angle = pick(aim.sway_max_angle);
    let lerp_speed = pick(aim.sway_lerp_speed);
    let pitch_scale = pick(aim.sway_pitch_scale);
    let yaw_scale = pick(aim.sway_yaw_scale);
    let horiz_scale = pick(aim.sway_horiz_scale);
    let vert_scale = pick(aim.sway_vert_scale);

    let dp = clamp(
        angle_subtract(view[0], st.last_view[0]),
        -max_angle,
        max_angle,
    );
    let dy = clamp(
        angle_subtract(view[1], st.last_view[1]),
        -max_angle,
        max_angle,
    );
    let k = lerp_speed * msec as f32 * 0.001;

    st.sway_offset[1] = approach(st.sway_offset[1], horiz_scale * dy * scale, k);
    st.sway_offset[2] = approach(st.sway_offset[2], dp * vert_scale * scale, k);

    let mut ta0 = near(dp * pitch_scale * scale, st.sway_angles[0]);
    let mut ta1 = near(dy * yaw_scale * scale, st.sway_angles[1]);
    while ta0 - st.sway_angles[0] > 180.0 {
        ta0 -= 360.0;
    }
    while ta1 - st.sway_angles[1] > 180.0 {
        ta1 -= 360.0;
    }
    st.sway_angles[0] = angle_normalize_180(approach(st.sway_angles[0], ta0, k));
    st.sway_angles[1] = angle_normalize_180(approach(st.sway_angles[1], ta1, k));
    st.last_view = view;
}

/// `0x39604`: the tilt the gun takes with speed, `standRotP/Y/R` and its
/// stance twins scaled by how far past the stance's minimum speed the
/// player runs, eased at `posRotRate` (`posProneRotRate` with the eye at the
/// prone height) with a floor of 0.1 a second, and faded out over the first
/// half of the sight's rise. A UO sprint takes `sprintRot*` from no
/// minimum speed in place of the stance's (uo cgame 0x30014ea0).
fn movement_rot(
    ps: &PlayerState,
    aim: &AimDef,
    st: &mut AimState,
    spd: f32,
    dt: f32,
    out: &mut [f32; 3],
) {
    let min_speed = if ps.sprinting {
        0.0
    } else {
        stance_pick(
            ps,
            aim.stand_rot_min_speed,
            aim.ducked_rot_min_speed,
            aim.prone_rot_min_speed,
        )
    };
    let frac = ps.weapon_pos_frac;
    let mut target = [0.0f32; 3];
    if spd > min_speed && ps.weaponstate != super::weapon::WEAPON_RELOADING {
        let f = ((spd - min_speed) / (ps.speed - min_speed)).clamp(0.0, 1.0);
        let rot = match ps.stance {
            _ if ps.sprinting => aim.sprint_rot,
            Stance::Prone => aim.prone_rot,
            Stance::Crouch => aim.ducked_rot,
            Stance::Stand => aim.stand_rot,
        };
        target = rot.map(|r| r * f);
    }
    if frac != 0.0 {
        target = target.map(|t| t * (1.0 - frac));
    }
    let rate = if ps.view_height_cur == super::VIEW_PRONE {
        aim.pos_prone_rot_rate
    } else {
        aim.pos_rot_rate
    };
    ease_toward(&mut st.rot, target, rate, dt);
    let k = if frac == 0.0 {
        1.0
    } else if frac < 0.5 {
        1.0 - 2.0 * frac
    } else {
        return;
    };
    for (o, r) in out.iter_mut().zip(&st.rot) {
        *o += r * k;
    }
}

/// Each axis of `cur` a step of `dt * gap * rate` toward `target`, at least
/// a tenth a second, and never past it.
fn ease_toward(cur: &mut [f32; 3], target: [f32; 3], rate: f32, dt: f32) {
    for (cur, &target) in cur.iter_mut().zip(&target) {
        if target == *cur {
            continue;
        }
        let mut step = dt * (target - *cur) * rate;
        if *cur < target {
            step = step.max(dt * 0.1);
            *cur = (*cur + step).min(target);
        } else {
            step = step.min(dt * -0.1);
            *cur = (*cur + step).max(target);
        }
    }
}

/// The gun's position under movement, the cgame's twin of [`movement_rot`]
/// (`0x300362c0`, combat doc 17.4) with no server copy: `0x3020946c`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct GunMove {
    pos: [f32; 3],
}

impl GunMove {
    /// Steps the eased position by `msec` and returns what it adds to the
    /// gun in the view's (forward, left, up) frame, the file's `R` axis on
    /// the left. The target is the stance's `*Move` scaled by how far past
    /// its `*MoveMinSpeed` the player runs (none while reloading), plus
    /// `duckedOfs`/`proneOfs` at those heights; it eases at `posMoveRate`
    /// (`posProneMoveRate` with the eye at the prone height) with a floor
    /// of 0.1 a second, and fades out over the first half of the sight. A
    /// UO sprint takes `sprintMove*` past `sprintMoveMinSpeed` in place of
    /// the stance's (uo cgame 0x30044ce0).
    pub fn step(&mut self, ps: &PlayerState, aim: &AimDef, msec: i32) -> [f32; 3] {
        let spd = speed(ps);
        let min_speed = if ps.sprinting {
            aim.sprint_move_min_speed
        } else {
            stance_pick(
                ps,
                aim.stand_move_min_speed,
                aim.ducked_move_min_speed,
                aim.prone_move_min_speed,
            )
        };
        let mut target = [0.0f32; 3];
        if spd > min_speed && ps.weaponstate != super::weapon::WEAPON_RELOADING {
            let f = ((spd - min_speed) / (ps.speed - min_speed)).clamp(0.0, 1.0);
            let mv = match ps.stance {
                _ if ps.sprinting => aim.sprint_move,
                Stance::Prone => aim.prone_move,
                Stance::Crouch => aim.ducked_move,
                Stance::Stand => aim.stand_move,
            };
            target = mv.map(|m| m * f);
        }
        let ofs = match ps.stance {
            Stance::Prone => aim.prone_ofs,
            Stance::Crouch => aim.ducked_ofs,
            Stance::Stand => [0.0; 3],
        };
        for (t, o) in target.iter_mut().zip(ofs) {
            *t += o;
        }
        let rate = if ps.view_height_cur == super::VIEW_PRONE {
            aim.pos_prone_move_rate
        } else {
            aim.pos_move_rate
        };
        let dt = msec as f32 * 0.001;
        ease_toward(&mut self.pos, target, rate, dt);
        let frac = ps.weapon_pos_frac;
        let k = if frac == 0.0 {
            1.0
        } else if frac < 0.5 {
            1.0 - 2.0 * frac
        } else {
            0.0
        };
        self.pos.map(|p| p * k)
    }
}

/// `0x3990c`: the breathing. Three sines of `level.time` at 0.5, 0.7 and
/// 1 mrad/ms on roll, yaw and pitch, scaled by `hipIdleAmount` (80 when the
/// file spells none) blended to `adsIdleAmount` by the sight fraction, and
/// by a stance factor eased at 0.5 a second toward `idleCrouchFactor` or
/// `idleProneFactor`.
fn idle(
    ps: &PlayerState,
    aim: &AimDef,
    ads_weapon: bool,
    st: &mut AimState,
    now_ms: i32,
    dt: f32,
    out: &mut [f32; 3],
) {
    let amount = if ads_weapon {
        lerp(aim.hip_idle_amount, aim.ads_idle_amount, ps.weapon_pos_frac)
    } else if aim.hip_idle_amount == 0.0 {
        80.0
    } else {
        aim.hip_idle_amount
    };
    let target = stance_pick(ps, 1.0, aim.idle_crouch_factor, aim.idle_prone_factor);
    if st.idle_factor < target {
        st.idle_factor = (st.idle_factor + dt * 0.5).min(target);
    } else if st.idle_factor > target {
        st.idle_factor = (st.idle_factor - dt * 0.5).max(target);
    }
    let amount = amount * st.idle_factor;
    let t = now_ms as f64;
    let s = |rate: f32| (t * rate as f64).sin() as f32;
    out[2] += amount * s(0.0005) * 0.04;
    out[1] += amount * s(0.0007) * 0.01;
    out[0] += 0.01 * s(0.001) * amount;
}

/// The bob phase both bobs share: `bobCycle` over 255 as a turn, offset by
/// a quarter turn on the gun and by none on the view.
fn bob_cycle(ps: &PlayerState, quarter: bool) -> f64 {
    let turn = PI64 * (ps.bob_cycle as f32 / 255.0) as f64;
    let base = if quarter { PI64 / 4.0 } else { 0.0 };
    base + 2.0 * turn + 2.0 * PI64 * if quarter { 2.0 } else { 1.0 }
}

/// `0x39a5c`: the walk bob on the gun, a pair of sines of the bob cycle
/// with an amplitude of the speed times 0.16 times the stance multiplier,
/// capped at 10, faded toward `adsBobFactor` by the sight fraction.
fn walk_bob(ps: &PlayerState, def: &WeaponDef, spd: f32, out: &mut [f32; 3]) {
    let cycle = bob_cycle(ps, true);
    let sp = spd * 0.16;
    let mult = bob_stance_mult(ps);
    let a = (sp * mult).min(10.0);
    let pitch =
        -(((PI64 / 2.0 + cycle * 4.0).sin() as f32 * 0.2 + (2.0 * cycle).sin() as f32) * 0.75 * a);
    let yaw = -(cycle.sin() as f32 * a);
    let a2 = (sp * 1.5 * mult).min(10.0);
    let roll = (((cycle - 0.4712389167638204) as f32 as f64).sin() as f32 * a2).min(0.0);
    let frac = ps.weapon_pos_frac;
    let k = if frac != 0.0 {
        1.0 - (1.0 - def.ads_bob_factor) * frac
    } else {
        1.0
    };
    out[0] += pitch * k;
    out[1] += yaw * k;
    out[2] += roll * k;
}

/// The damage kick's envelope: an eased rise over `rise` ms from the hit
/// and an eased fall over `fall` more, `None` once it is over.
fn kick_envelope(dt_ms: i32, rise: f32, fall: f32) -> Option<f32> {
    let dt = dt_ms as f32;
    if dt < rise {
        Some(lean_fraction(dt / rise))
    } else {
        let r = 1.0 - (dt - rise) / fall;
        (r > 0.0).then(|| 1.0 - lean_fraction(1.0 - r))
    }
}

/// `0x39ce8`: the damage kick on the gun, over 100 ms up and 400 down at
/// the hip and half again as long down the sight, and damped by the sight
/// on a scope.
fn gun_damage_kick(
    ps: &PlayerState,
    aim: &AimDef,
    kick: &DamageKick,
    now_ms: i32,
    out: &mut [f32; 3],
) {
    if kick.time_ms == 0 {
        return;
    }
    let frac = ps.weapon_pos_frac;
    let mut f = frac * 0.5 + 0.5;
    let rise = f * 100.0;
    let fall = f * 400.0;
    if frac != 0.0 && aim.ads_overlay_reticle {
        f *= 1.0 - frac * 0.75;
    }
    let Some(g) = kick_envelope(now_ms - kick.time_ms, rise, fall) else {
        return;
    };
    let g = g * f;
    out[0] += g * kick.pitch * 0.5;
    out[1] -= g * kick.side;
    out[2] += g * kick.side * 0.5;
}

/// `BG_CalculateWeaponAngles` (`0x3a1e4`): the lean roll, `adsAimPitch` by
/// the sight fraction, then the four terms above, less the turn sway's lag.
fn weapon_angles(
    ps: &PlayerState,
    def: &WeaponDef,
    st: &mut AimState,
    input: &AimInput,
    spd: f32,
) -> [f32; 3] {
    let aim = &def.aim;
    let mut out = [0.0f32; 3];
    if ps.lean != 0.0 {
        out[2] -= 2.0 * lean_fraction(ps.lean);
    }
    if def.aim_down_sight {
        out[0] += ps.weapon_pos_frac * aim.ads_aim_pitch;
    }
    let dt = input.msec as f32 * 0.001;
    movement_rot(ps, aim, st, spd, dt, &mut out);
    idle(ps, aim, def.aim_down_sight, st, input.now_ms, dt, &mut out);
    walk_bob(ps, def, spd, &mut out);
    gun_damage_kick(ps, aim, &input.kick, input.now_ms, &mut out);
    if def.aim_down_sight {
        out[0] += input.gun_kick[0];
        out[1] += input.gun_kick[1];
    }
    out[0] = angle_subtract(out[0], st.sway_angles[0]);
    out[1] = angle_subtract(out[1], st.sway_angles[1]);
    out
}

/// `BG_CalculateViewAngles` (`0x3a930`): the damage kick on the view
/// (`0x3a2d4`), 100 ms up and 400 down, halved down the sight except on a
/// scope, and the sight's own walk bob (`0x3a3c4`) by `adsViewBobMult`.
/// The cgame adds the same two to the drawn view (`0x30012cb0` and
/// `0x30012dc0`, combat doc 15.5). Degrees, wire convention; `def` is
/// `ps.weapon`'s file, `None` reading as one with neither key set.
pub fn view_angles(
    ps: &PlayerState,
    def: Option<&WeaponDef>,
    kick: &DamageKick,
    now_ms: i32,
) -> [f32; 3] {
    let mut out = [0.0f32; 3];
    let frac = ps.weapon_pos_frac;
    if kick.time_ms != 0 {
        let mut f = 1.0 - frac * 0.5;
        if frac > 0.0 && def.is_some_and(|d| d.aim.ads_overlay_reticle) {
            f *= frac * 0.5 + 1.0;
        }
        if let Some(g) = kick_envelope(now_ms - kick.time_ms, 100.0, 400.0) {
            out[0] += g * f * kick.pitch;
            out[2] += g * f * kick.side;
        }
    }
    let Some(def) = def else {
        return out;
    };
    if frac != 0.0 && def.ads_view_bob_mult != 0.0 {
        let spd = speed(ps);
        let cycle = bob_cycle(ps, false);
        let a = (spd * bob_stance_mult(ps)).min(45.0);
        let k = frac * def.ads_view_bob_mult;
        out[0] -= k
            * (((PI64 / 2.0 + cycle * 4.0).sin() as f32 * 0.2 + (2.0 * cycle).sin() as f32)
                * 0.75
                * a);
        out[1] -= k * cycle.sin() as f32 * a;
    }
    out
}

/// `AnglesToAxis` (`0x3ef3c`): `[forward, left, up]` of wire angles, as
/// the Linux servers round it ([`x87::angles_to_axis`]).
pub fn angles_to_axis(angles: [f32; 3]) -> [[f32; 3]; 3] {
    x87::angles_to_axis(angles.into()).map(|v| v.to_array())
}

/// [`angles_to_axis`] as the Windows cgames round it
/// ([`x87::angles_to_axis_msvc`]): what the client draws with.
pub fn angles_to_axis_msvc(angles: [f32; 3]) -> [[f32; 3]; 3] {
    x87::angles_to_axis_msvc(angles.into()).map(|v| v.to_array())
}

/// `AxisToAngles` (`0x3c808`), the pitch and yaw half: `vectoangles` of
/// the forward with both in 0..360.
fn forward_to_angles(f: [f32; 3]) -> [f32; 2] {
    if f[0] == 0.0 && f[1] == 0.0 {
        return [if f[2] > 0.0 { 270.0 } else { 90.0 }, 0.0];
    }
    let mut yaw = f[1].atan2(f[0]).to_degrees();
    if yaw < 0.0 {
        yaw += 360.0;
    }
    let mut pitch =
        f[2].atan2((f[0] * f[0] + f[1] * f[1]).sqrt()) * (-180.0 / std::f32::consts::PI);
    if pitch < 0.0 {
        pitch += 360.0;
    }
    [pitch, yaw]
}

/// The turn sway alone, stepped `msec` with its four targets times `scale`,
/// returning [`AimState::sway_offset`]. United Offensive's cgame runs it on
/// a UO gunner's eye off a `thompson_MP` copy of the playerstate at scale 2
/// (0x30015ca0 from 0x30040810, docs/research/coduo-vehicles.md 5).
pub fn step_turn_sway(
    ps: &PlayerState,
    def: &WeaponDef,
    st: &mut AimState,
    view: [f32; 3],
    msec: i32,
    scale: f32,
) -> [f32; 3] {
    sway(ps, &def.aim, def.aim_down_sight, st, view, msec, scale);
    st.sway_offset
}

/// The turn sway stepped, then `BG_CalculateWeaponAngles`: the gun's
/// angles off the view, wire convention. The client centres its scope
/// overlay on them. `None`, with the state untouched, for a weapon index
/// with no file behind it.
pub fn gun_angles(ps: &PlayerState, st: &mut AimState, input: &AimInput) -> Option<[f32; 3]> {
    let def = input.def?;
    sway(
        ps,
        &def.aim,
        def.aim_down_sight,
        st,
        input.view,
        input.msec,
        input.sway_scale,
    );
    Some(weapon_angles(ps, def, st, input, speed(ps)))
}

/// The block (`ClientThink_real` `0x40169`-`0x40456`). Returns the pitch
/// and yaw `FireWeapon` and `FireWeaponMelee` read off `client+0x220c` and
/// `+0x2210`: the view plus its kick, and down the sight of an
/// `aimDownSight` weapon that rotated by the gun's own angles, degrees in
/// the wire convention. A weapon index with no file behind it runs on
/// weapon 0's def, which is all zeros but its name: the block still runs
/// and the sway still records the view it saw (combat doc 15.3).
pub fn aim_angles(ps: &PlayerState, st: &mut AimState, input: &AimInput) -> [f32; 2] {
    static NONE: std::sync::LazyLock<WeaponDef> = std::sync::LazyLock::new(WeaponDef::default);
    let def = input.def.unwrap_or(&NONE);
    let input = &AimInput {
        def: Some(def),
        ..*input
    };
    let vk = view_angles(ps, Some(def), &input.kick, input.now_ms);
    let mut aim = input.view;
    for (a, v) in aim.iter_mut().zip(&vk) {
        *a += v;
    }
    let wa = gun_angles(ps, st, input).unwrap_or_default();
    if def.aim_down_sight && ps.weapon_pos_frac != 0.0 {
        // `MatrixMultiply(weaponAxis, viewAxis)`: the gun's forward
        // expressed in the view's basis.
        let a = angles_to_axis(wa);
        let b = angles_to_axis(aim);
        let mut f = [0.0f32; 3];
        for (j, fj) in f.iter_mut().enumerate() {
            *fj = a[0][0] * b[0][j] + a[0][1] * b[1][j] + a[0][2] * b[2][j];
        }
        return forward_to_angles(f);
    }
    [aim[0], aim[1]]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pmove::SPEED_RUN;
    use crate::xmodel::WeaponKeys;
    use glam::Vec3;

    /// 988.76947 degrees is 179999.993 65536ths: retail's x87 product
    /// truncates to 179999, where an `f32` product rounds onto 180000.
    #[test]
    fn angle_normalize_360_truncates_the_unrounded_product() {
        assert_eq!(angle_normalize_360(988.7695), 268.764_04);
        assert_eq!(angle_normalize_360(-90.0), 270.0);
    }

    /// The kar98k sniper's aim keys as `pak0.pk3` spells them.
    fn sniper() -> WeaponDef {
        let mut m = WeaponKeys::default();
        for (k, v) in [
            ("aimDownSight", "1"),
            ("adsOverlayReticle", "FG42"),
            ("adsIdleAmount", "45"),
            ("hipIdleAmount", "30"),
            ("idleCrouchFactor", "0.2"),
            ("idleProneFactor", "0.085"),
            ("adsAimPitch", "0.6"),
            ("adsBobFactor", "0"),
            ("adsViewBobMult", "0"),
            ("swayMaxAngle", "30"),
            ("swayLerpSpeed", "6"),
            ("swayPitchScale", "0.1"),
            ("swayYawScale", "0.1"),
            ("swayHorizScale", "0.1"),
            ("swayVertScale", "0.1"),
            ("adsSwayMaxAngle", "30"),
            ("adsSwayLerpSpeed", "6"),
            ("adsSwayPitchScale", "0.1"),
            ("adsSwayYawScale", "0.1"),
            ("adsSwayHorizScale", "0.1"),
            ("adsSwayVertScale", "0.1"),
            ("standRotP", "0"),
            ("standRotY", "0"),
            ("standRotR", "-0.2"),
            ("posRotRate", "8"),
            ("posProneRotRate", "30"),
            ("standRotMinSpeed", "80"),
            ("duckedRotMinSpeed", "20"),
            ("proneRotMinSpeed", "0"),
        ] {
            m.insert(k, v.to_string());
        }
        WeaponDef::from_map(&m)
    }

    fn still(frac: f32) -> PlayerState {
        let mut ps = PlayerState::spawn(Vec3::ZERO, 0.0);
        ps.on_ground = true;
        ps.weapon_pos_frac = frac;
        ps
    }

    fn input<'a>(def: &'a WeaponDef, view: [f32; 3], now_ms: i32) -> AimInput<'a> {
        AimInput {
            def: Some(def),
            view,
            msec: 50,
            now_ms,
            kick: DamageKick::default(),
            gun_kick: [0.0; 2],
            sway_scale: 1.0,
        }
    }

    /// The idle sway alone, standing still down a settled scope, against
    /// the three sines read out of `0x3990c`: at `level.time` 100000 the
    /// arguments are 50, 70 and 100 rad, `adsIdleAmount` 45 with the
    /// stance factor at 1, so the gun reads
    /// `pitch 0.01 * sin(100) * 45`, `yaw 0.01 * sin(70) * 45`, plus
    /// `adsAimPitch` 0.6 on the pitch. Composed onto a level view at yaw 0
    /// the small-angle result is that pitch and that yaw, with pitch
    /// wrapped into 0..360 by `AxisToAngles`.
    #[test]
    fn a_settled_scope_aims_along_the_idle_sine() {
        let def = sniper();
        let ps = still(1.0);
        let mut st = AimState {
            idle_factor: 1.0,
            ..Default::default()
        };
        let aim = aim_angles(&ps, &mut st, &input(&def, [0.0, 0.0, 0.0], 100_000));
        let pitch = 0.6 + 0.01 * (100.0f64.sin() as f32) * 45.0;
        let yaw = 0.01 * (70.0f64.sin() as f32) * 45.0;
        let want_pitch = if pitch < 0.0 { pitch + 360.0 } else { pitch };
        let want_yaw = if yaw < 0.0 { yaw + 360.0 } else { yaw };
        assert!(
            (aim[0] - want_pitch).abs() < 0.01,
            "pitch {} vs {}",
            aim[0],
            want_pitch
        );
        assert!(
            (aim[1] - want_yaw).abs() < 0.01,
            "yaw {} vs {}",
            aim[1],
            want_yaw
        );
    }

    /// The kar98k's position under movement: at a run the gun eases
    /// toward `standMove` (0, 0.6, -1) at `posMoveRate` 4 a second, sits
    /// at `duckedOfs` crouched still, and the raised sight takes it away.
    #[test]
    fn the_gun_moves_with_speed_and_stance() {
        let mut m = WeaponKeys::default();
        for (k, v) in [
            ("standMoveR", "0.6"),
            ("standMoveU", "-1"),
            ("duckedOfsF", "-2"),
            ("duckedOfsR", "1"),
            ("duckedOfsU", "-0.2"),
            ("posMoveRate", "4"),
            ("posProneMoveRate", "20"),
            ("standMoveMinSpeed", "80"),
            ("duckedMoveMinSpeed", "20"),
        ] {
            m.insert(k, v.to_string());
        }
        let aim = WeaponDef::from_map(&m).aim;
        let mut ps = still(0.0);
        let mut g = GunMove::default();
        assert_eq!(g.step(&ps, &aim, 50), [0.0; 3], "still and standing");
        ps.velocity = Vec3::new(SPEED_RUN, 0.0, 0.0);
        // 50 ms at rate 4: a fifth of the way.
        let [f, l, u] = g.step(&ps, &aim, 50);
        assert_eq!(f, 0.0);
        assert!((l - 0.6 * 0.2).abs() < 1e-6 && (u + 0.2).abs() < 1e-6);
        for _ in 0..200 {
            g.step(&ps, &aim, 50);
        }
        assert_eq!(g.step(&ps, &aim, 50), [0.0, 0.6, -1.0], "settled at a run");
        // A half-raised sight takes all of it, a quarter-raised half.
        ps.weapon_pos_frac = 0.5;
        assert_eq!(g.step(&ps, &aim, 0), [0.0; 3]);
        ps.weapon_pos_frac = 0.25;
        assert_eq!(g.step(&ps, &aim, 0), [0.0, 0.3, -0.5]);
        ps.weapon_pos_frac = 0.0;
        ps.velocity = Vec3::ZERO;
        ps.stance = Stance::Crouch;
        for _ in 0..200 {
            g.step(&ps, &aim, 50);
        }
        assert_eq!(g.step(&ps, &aim, 50), [-2.0, 1.0, -0.2], "crouched still");
    }

    /// The run's fraction is over the wire's `ps.speed`, not 190: under a
    /// `g_speed` of 250 a 190 run settles at (190 - 80) / (250 - 80) of
    /// the stance's pose.
    #[test]
    fn the_gun_move_scales_against_the_server_s_speed() {
        let mut m = WeaponKeys::default();
        for (k, v) in [
            ("standMoveR", "0.6"),
            ("standMoveU", "-1"),
            ("posMoveRate", "4"),
            ("standMoveMinSpeed", "80"),
        ] {
            m.insert(k, v.to_string());
        }
        let aim = WeaponDef::from_map(&m).aim;
        let mut ps = still(0.0);
        ps.speed = 250.0;
        ps.velocity = Vec3::new(SPEED_RUN, 0.0, 0.0);
        let mut g = GunMove::default();
        for _ in 0..400 {
            g.step(&ps, &aim, 50);
        }
        let f = 110.0 / 170.0;
        let [_, l, u] = g.step(&ps, &aim, 50);
        assert!(
            (l - 0.6 * f).abs() < 1e-5 && (u + f).abs() < 1e-5,
            "{l} {u}"
        );
    }

    /// UO's sprint pose: the stock files' `sprintMove*` past
    /// `sprintMoveMinSpeed`, eased in at `posMoveRate`, and the stance's
    /// pose again once the sprint ends.
    #[test]
    fn a_uo_sprint_swings_the_gun_to_its_sprint_pose() {
        let mut m = WeaponKeys::default();
        for (k, v) in [
            ("standMoveR", "0.5"),
            ("standMoveU", "-1.2"),
            ("sprintMoveF", "-14"),
            ("sprintMoveR", "-10"),
            ("sprintMoveU", "-14"),
            ("posMoveRate", "4"),
            ("standMoveMinSpeed", "80"),
            ("sprintMoveMinSpeed", "110"),
        ] {
            m.insert(k, v.to_string());
        }
        let aim = WeaponDef::from_map(&m).aim;
        let mut ps = still(0.0);
        ps.sprinting = true;
        ps.velocity = Vec3::new(304.0, 0.0, 0.0);
        let mut g = GunMove::default();
        let [f, l, u] = g.step(&ps, &aim, 50);
        assert!((f + 14.0 * 0.2).abs() < 1e-5 && (l + 2.0).abs() < 1e-5 && (u + 2.8).abs() < 1e-5);
        for _ in 0..200 {
            g.step(&ps, &aim, 50);
        }
        assert_eq!(
            g.step(&ps, &aim, 50),
            [-14.0, -10.0, -14.0],
            "settled at a sprint"
        );
        ps.sprinting = false;
        ps.velocity = Vec3::new(SPEED_RUN, 0.0, 0.0);
        for _ in 0..200 {
            g.step(&ps, &aim, 50);
        }
        assert_eq!(g.step(&ps, &aim, 50), [0.0, 0.5, -1.2], "back at a run");
    }

    /// Off the sight the gun's angles never enter the aim: a hip shot goes
    /// where the view points however the gun sways.
    #[test]
    fn a_hip_shot_aims_the_raw_view() {
        let def = sniper();
        let ps = still(0.0);
        let mut st = AimState::default();
        let aim = aim_angles(&ps, &mut st, &input(&def, [-10.0, 45.0, 0.0], 123_456));
        assert_eq!(aim, [-10.0, 45.0]);
    }

    /// The turn sway: a 10 degree yaw step with `swayLerpSpeed` 6 over a
    /// 50 ms cmd moves the lag 0.3 of the way to `10 * swayYawScale`, which
    /// `AngleNormalize180`'s 65536ths then truncate to 54/182.04; and a
    /// scope with the sight up leaves the state untouched, the last view
    /// included.
    #[test]
    fn the_turn_sway_lags_the_view_and_a_scope_freezes_it() {
        let def = sniper();
        let ps = still(0.0);
        let mut st = AimState::default();
        sway(&ps, &def.aim, true, &mut st, [0.0, 10.0, 0.0], 50, 1.0);
        let want = 54.0 * (360.0 / 65536.0);
        assert!(
            (st.sway_angles[1] - want).abs() < 1e-5,
            "{}",
            st.sway_angles[1]
        );
        assert_eq!(st.last_view, [0.0, 10.0, 0.0]);
        let scoped = still(0.5);
        let before = st;
        sway(&scoped, &def.aim, true, &mut st, [0.0, 20.0, 0.0], 50, 1.0);
        assert_eq!(st, before);
    }

    /// The composition is a rotation in the view's own frame: a gun pitched
    /// 1 degree down on a view yawed 90 degrees aims 1 degree down at yaw 90,
    /// not at a yaw the world's pitch axis would give.
    #[test]
    fn the_gun_angles_compose_in_the_views_frame() {
        let a = angles_to_axis([1.0, 0.0, 0.0]);
        let b = angles_to_axis([0.0, 90.0, 0.0]);
        let mut f = [0.0f32; 3];
        for (j, fj) in f.iter_mut().enumerate() {
            *fj = a[0][0] * b[0][j] + a[0][1] * b[1][j] + a[0][2] * b[2][j];
        }
        let [pitch, yaw] = forward_to_angles(f);
        assert!((pitch - 1.0).abs() < 1e-3, "{pitch}");
        assert!((yaw - 90.0).abs() < 1e-3, "{yaw}");
    }

    /// A weapon the table has no file for runs on weapon 0's zeroed def:
    /// it aims the raw view and the sway records that view, so the next
    /// weapon's first cmd turns off it. Off a spawn's 180 onto 0 the turn
    /// reads -180, not +180 (combat doc 15.3.1).
    #[test]
    fn no_file_aims_the_raw_view_and_records_it() {
        let ps = still(0.0);
        let mut st = AimState::default();
        let none = AimInput {
            def: None,
            view: [0.0, 180.0, 0.0],
            msec: 100,
            now_ms: 1000,
            kick: DamageKick::default(),
            gun_kick: [0.0; 2],
            sway_scale: 1.0,
        };
        assert_eq!(aim_angles(&ps, &mut st, &none), [0.0, 180.0]);
        assert_eq!(st.last_view, [0.0, 180.0, 0.0]);
        assert_eq!(st.sway_angles, [0.0; 3]);
        let def = sniper();
        aim_angles(&ps, &mut st, &input(&def, [0.0, 0.0, 0.0], 1016));
        assert!(st.sway_angles[1] < 0.0, "{:?}", st.sway_angles);
    }

    /// The view kick eases up over 100 ms and down over 400, in full at the
    /// hip, halved down an iron sight and three quarters down a scope.
    #[test]
    fn the_view_kick_envelope_and_sight_factors() {
        let kick = DamageKick {
            time_ms: 1000,
            pitch: -10.0,
            side: 4.0,
        };
        let hip = still(0.0);
        let at = |ps: &PlayerState, def: Option<&WeaponDef>, t: i32| view_angles(ps, def, &kick, t);
        assert_eq!(at(&hip, None, 1000), [0.0; 3]);
        // GetLeanFraction(0.5) = 0.75 halfway up.
        assert_eq!(at(&hip, None, 1050), [-7.5, 0.0, 3.0]);
        assert_eq!(at(&hip, None, 1100), [-10.0, 0.0, 4.0]);
        // Halfway down: 1 - GetLeanFraction(0.5).
        assert_eq!(at(&hip, None, 1300), [-2.5, 0.0, 1.0]);
        assert_eq!(at(&hip, None, 1500), [0.0; 3]);
        assert_eq!(at(&hip, None, 9000), [0.0; 3]);
        let sighted = still(1.0);
        assert_eq!(at(&sighted, None, 1100), [-5.0, 0.0, 2.0]);
        let scope = sniper();
        assert_eq!(at(&sighted, Some(&scope), 1100), [-7.5, 0.0, 3.0]);
        let never = DamageKick::default();
        assert_eq!(view_angles(&hip, None, &never, 50), [0.0; 3]);
    }

    /// A weapon file's crossed sway and kick maxima, a NaN or out-of-range
    /// sight fraction and a view of 1e30 or infinity step the sway and the
    /// spring without a panic or a hang (SEC-048, SEC-062).
    #[test]
    fn hostile_keys_and_fractions_step_the_gun() {
        let mut m = WeaponKeys::default();
        for (k, v) in [
            ("aimDownSight", "1"),
            ("swayMaxAngle", "-1"),
            ("adsSwayMaxAngle", "nan"),
            ("swayPitchScale", "1e30"),
            ("swayYawScale", "inf"),
            ("swayLerpSpeed", "6"),
            ("hipGunKickSpeedMax", "-5"),
            ("adsGunKickSpeedMax", "nan"),
            ("hipGunKickAccel", "100"),
            ("gunMaxPitch", "10"),
            ("gunMaxYaw", "10"),
        ] {
            m.insert(k, v.to_string());
        }
        let def = WeaponDef::from_map(&m);
        for frac in [0.0, 0.5, 2.0, -1.0, f32::NAN, f32::INFINITY] {
            let ps = still(frac);
            let mut st = AimState::default();
            for view in [
                [0.0, 0.0, 0.0],
                [1e30, -1e30, 0.0],
                [f32::INFINITY, f32::NAN, 0.0],
            ] {
                gun_angles(&ps, &mut st, &input(&def, view, 1000));
                aim_angles(&ps, &mut st, &input(&def, view, 1050));
            }
            let mut kick = GunKick::default();
            kick.kick(30.0, -30.0);
            kick.step(&def, frac, 0.1);
        }
    }
}
