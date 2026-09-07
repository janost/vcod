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

use super::{PlayerState, Stance, LEAN_MAX, SPEED_RUN};
use crate::weapon::{AimDef, WeaponDef};

/// `client+0x2278..0x22a8`: what the block keeps between cmds. The gun-kick
/// springs at `+0x22ac` are left out: nothing in the server module ever
/// kicks them, so on a server they hold zero for the whole of a life.
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
}

const PI64: f64 = std::f64::consts::PI;

/// `AngleNormalize360` (`0x3eb30`), 65536ths truncated toward zero (the
/// `fldcw` there sets `0xc00`). A lag the sway eases toward zero reaches it
/// only because of the truncation: rounded to nearest, it stalls a few
/// 65536ths out once a step is under half of one, which a run against
/// ours measured as a constant 0.022 degrees of yaw (combat doc, 15.4).
fn angle_normalize_360(a: f32) -> f32 {
    ((a * (65536.0 / 360.0)) as i32 & 0xffff) as f32 * (360.0 / 65536.0)
}

/// `AngleNormalize180` (`0x3eb70`).
fn angle_normalize_180(a: f32) -> f32 {
    let a = angle_normalize_360(a);
    if a > 180.0 {
        a - 360.0
    } else {
        a
    }
}

/// `AngleSubtract` (`0x3e968`): the difference wrapped into -180..180 by
/// repeated 360s, no rounding.
fn angle_subtract(a: f32, b: f32) -> f32 {
    let mut d = a - b;
    while d > 180.0 {
        d -= 360.0;
    }
    while d < -180.0 {
        d += 360.0;
    }
    d
}

/// `GetLeanFraction` (`0x7ba64`): `(2 - |f|) * f`, the ease every kick
/// curve and the lean roll go through.
fn lean_fraction(f: f32) -> f32 {
    (2.0 - f.abs()) * f
}

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
/// the target is the stance's own height.
fn bob_stance_mult(ps: &PlayerState) -> f32 {
    stance_pick(ps, 0.007, 0.0075, 0.03)
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

    let dp = angle_subtract(view[0], st.last_view[0]).clamp(-max_angle, max_angle);
    let dy = angle_subtract(view[1], st.last_view[1]).clamp(-max_angle, max_angle);
    let k = lerp_speed * msec as f32 * 0.001;

    st.sway_offset[1] = approach(st.sway_offset[1], horiz_scale * dy, k);
    st.sway_offset[2] = approach(st.sway_offset[2], dp * vert_scale, k);

    let mut ta0 = dp * pitch_scale;
    let mut ta1 = dy * yaw_scale;
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
/// half of the sight's rise.
fn movement_rot(
    ps: &PlayerState,
    aim: &AimDef,
    st: &mut AimState,
    spd: f32,
    dt: f32,
    out: &mut [f32; 3],
) {
    let min_speed = stance_pick(
        ps,
        aim.stand_rot_min_speed,
        aim.ducked_rot_min_speed,
        aim.prone_rot_min_speed,
    );
    let frac = ps.weapon_pos_frac;
    let mut target = [0.0f32; 3];
    if spd > min_speed && ps.weaponstate != super::weapon::WEAPON_RELOADING {
        let f = ((spd - min_speed) / (SPEED_RUN - min_speed)).clamp(0.0, 1.0);
        let rot = match ps.stance {
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
    for (cur, &target) in st.rot.iter_mut().zip(&target) {
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
    let leanf = ps.lean / LEAN_MAX;
    if leanf != 0.0 {
        out[2] -= 2.0 * lean_fraction(leanf);
    }
    if def.aim_down_sight {
        out[0] += ps.weapon_pos_frac * aim.ads_aim_pitch;
    }
    let dt = input.msec as f32 * 0.001;
    movement_rot(ps, aim, st, spd, dt, &mut out);
    idle(ps, aim, def.aim_down_sight, st, input.now_ms, dt, &mut out);
    walk_bob(ps, def, spd, &mut out);
    gun_damage_kick(ps, aim, &input.kick, input.now_ms, &mut out);
    out[0] = angle_subtract(out[0], st.sway_angles[0]);
    out[1] = angle_subtract(out[1], st.sway_angles[1]);
    out
}

/// `BG_CalculateViewAngles` (`0x3a930`): the damage kick on the view
/// (`0x3a2d4`), 100 ms up and 400 down, halved down the sight except on a
/// scope, and the sight's own walk bob (`0x3a3c4`) by `adsViewBobMult`.
fn view_angles(ps: &PlayerState, def: &WeaponDef, input: &AimInput, spd: f32) -> [f32; 3] {
    let mut out = [0.0f32; 3];
    let frac = ps.weapon_pos_frac;
    let kick = &input.kick;
    if kick.time_ms != 0 {
        let mut f = 1.0 - frac * 0.5;
        if frac > 0.0 && def.aim.ads_overlay_reticle {
            f *= frac * 0.5 + 1.0;
        }
        if let Some(g) = kick_envelope(input.now_ms - kick.time_ms, 100.0, 400.0) {
            out[0] += g * f * kick.pitch;
            out[2] += g * f * kick.side;
        }
    }
    if frac != 0.0 && def.ads_view_bob_mult != 0.0 {
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

/// `AngleVectors` (`0x3b228`): forward, right and up of wire angles.
fn angle_vectors(angles: [f32; 3]) -> [[f32; 3]; 3] {
    let (sy, cy) = angles[1].to_radians().sin_cos();
    let (sp, cp) = angles[0].to_radians().sin_cos();
    let (sr, cr) = angles[2].to_radians().sin_cos();
    let forward = [cp * cy, cp * sy, -sp];
    let right = [-sr * sp * cy + cr * sy, -sr * sp * sy - cr * cy, -sr * cp];
    let up = [cr * sp * cy + sr * sy, cr * sp * sy - sr * cy, cr * cp];
    [forward, right, up]
}

/// `AnglesToAxis` (`0x3ef3c`): `[forward, left, up]` of wire angles.
pub fn angles_to_axis(angles: [f32; 3]) -> [[f32; 3]; 3] {
    let [f, r, u] = angle_vectors(angles);
    [f, [-r[0], -r[1], -r[2]], u]
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

/// The block (`ClientThink_real` `0x40169`-`0x40456`). Returns the pitch
/// and yaw `FireWeapon` and `FireWeaponMelee` read off `client+0x220c` and
/// `+0x2210`: the view plus its kick, and down the sight of an
/// `aimDownSight` weapon that rotated by the gun's own angles, degrees in
/// the wire convention. A weapon index with no file behind it aims the raw
/// view and leaves the state alone.
pub fn aim_angles(ps: &PlayerState, st: &mut AimState, input: &AimInput) -> [f32; 2] {
    let Some(def) = input.def else {
        return [input.view[0], input.view[1]];
    };
    let spd = speed(ps);
    let vk = view_angles(ps, def, input, spd);
    let mut aim = input.view;
    for (a, v) in aim.iter_mut().zip(&vk) {
        *a += v;
    }
    sway(ps, &def.aim, def.aim_down_sight, st, input.view, input.msec);
    let wa = weapon_angles(ps, def, st, input, spd);
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
    use glam::Vec3;
    use std::collections::HashMap;

    /// The kar98k sniper's aim keys as `pak0.pk3` spells them.
    fn sniper() -> WeaponDef {
        let mut m = HashMap::new();
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
            m.insert(k.to_string(), v.to_string());
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
        sway(&ps, &def.aim, true, &mut st, [0.0, 10.0, 0.0], 50);
        let want = 54.0 * (360.0 / 65536.0);
        assert!(
            (st.sway_angles[1] - want).abs() < 1e-5,
            "{}",
            st.sway_angles[1]
        );
        assert_eq!(st.last_view, [0.0, 10.0, 0.0]);
        let scoped = still(0.5);
        let before = st;
        sway(&scoped, &def.aim, true, &mut st, [0.0, 20.0, 0.0], 50);
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

    /// A weapon the table has no file for aims the raw view.
    #[test]
    fn no_file_means_the_raw_view() {
        let ps = still(1.0);
        let mut st = AimState::default();
        let input = AimInput {
            def: None,
            view: [5.0, 6.0, 0.0],
            msec: 50,
            now_ms: 1000,
            kick: DamageKick::default(),
        };
        assert_eq!(aim_angles(&ps, &mut st, &input), [5.0, 6.0]);
        assert_eq!(st, AimState::default());
    }
}
