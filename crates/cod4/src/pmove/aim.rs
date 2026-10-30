//! The server's aim block and the shot's eye (docs/research/cod4-combat.md
//! 3.2). `ClientThink_real` (`cod4_lnxded` 0x80a5c1a) works out, before
//! `Pmove`, the pitch and yaw a cmd's rounds leave along (the gun
//! pitch and yaw, gclient +0x30a4, +0x30a8): the view as the last cmd left it
//! plus `BG_CalculateViewAngles` (0x806523e), and on a sighted
//! `aimDownSight` weapon without an overlay that turned by
//! `BG_CalculateWeaponAngles` (0x80659d0: the lean's roll, the sight's
//! pitch, the movement tilt, the idle drift, the walk bob, the gun spring,
//! less the turn sway of `BG_CalculateWeaponPosition_Sway`, 0x8060f20).
//! `BG_WeaponFireRecoil` (0x80609fc) kicks the spring off each fire event.
//! `BG_GetPlayerViewOrigin` (0x805399c) is the eye a round starts at.
//! Both blocks ease in the damage kick `P_DamageFeedback` (0x80a5156)
//! leaves ([`DamageKick`]), so a hit shooter's next rounds turn with it.

use glam::Vec3;

use super::breath::diff_track;
use super::fpu::{angle_delta, d};
use super::{PlayerState, ef, pmf};

/// `bg_bobAmplitudeStanding`, `Ducked`, `Prone`, `Sprinting` (horizontal,
/// vertical) at their defaults, and `bg_bobMax` (cod4-client-view.md 4).
const BOB_STANDING: [f32; 2] = [0.007, 0.007];
const BOB_DUCKED: [f32; 2] = [0.0075, 0.0075];
const BOB_PRONE: [f32; 2] = [0.02, 0.005];
const BOB_SPRINTING: [f32; 2] = [0.02, 0.014];
pub const BOB_MAX: f32 = 8.0;

/// `PWF_USING_OFFHAND`: the view weapon is the offhand.
const USING_OFFHAND: u32 = 2;

/// The `WeaponDef` fields the block reads, at their 1.7 offsets (the reads
/// in the functions above).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AimKeys {
    /// +0x4ec `aimDownSight`, +0x438 `overlayReticle` not none.
    pub ads: bool,
    pub overlay: bool,
    /// +0x6cc.
    pub ads_aim_pitch: f32,
    /// +0x240, +0x264, +0x288: `standRot`, `duckedRot`, `proneRot`.
    pub rots: [[f32; 3]; 3],
    /// +0x2b0..+0x2b8: their `*RotMinSpeed`.
    pub rot_min: [f32; 3],
    /// +0x2a8, +0x2ac.
    pub pos_rot_rate: f32,
    pub pos_prone_rot_rate: f32,
    /// +0x48c, +0x490, +0x494, +0x498, +0x49c, +0x4a0.
    pub ads_idle_amount: f32,
    pub hip_idle_amount: f32,
    pub ads_idle_speed: f32,
    pub hip_idle_speed: f32,
    pub idle_crouch: f32,
    pub idle_prone: f32,
    /// +0x448, +0x44c.
    pub ads_bob_factor: f32,
    pub ads_view_bob_mult: f32,
    /// +0x4a4, +0x4a8: `gunMaxPitch`, `gunMaxYaw`.
    pub gun_max: [f32; 2],
    /// +0x4ac.. and +0x4c8..: `MaxAngle`, `LerpSpeed`, `PitchScale`,
    /// `YawScale`, `HorizScale`, `VertScale`.
    pub sway: [f32; 6],
    pub ads_sway: [f32; 6],
    /// `GunKick` `Accel`, `SpeedMax`, `SpeedDecay`, `StaticDecay`: ads
    /// +0x6f0.., hip +0x738...
    pub ads_spring: [f32; 4],
    pub hip_spring: [f32; 4],
    /// `GunKickPitchMin/Max`, `YawMin/Max`: ads +0x6e0.., hip +0x728...
    pub ads_gun_kick: [f32; 4],
    pub hip_gun_kick: [f32; 4],
    /// +0x6dc, +0x724: `*GunKickReducedKickPercent`.
    pub ads_reduced_percent: f32,
    pub hip_reduced_percent: f32,
}

impl AimKeys {
    /// Off a zone weapon's 0x878-byte struct.
    pub fn from_raw(raw: &[u8]) -> AimKeys {
        let i = |o: usize| i32::from_le_bytes(raw[o..o + 4].try_into().unwrap());
        let f = |o: usize| f32::from_le_bytes(raw[o..o + 4].try_into().unwrap());
        let v = |o: usize| [f(o), f(o + 4), f(o + 8)];
        let n6 = |o: usize| std::array::from_fn(|k| f(o + 4 * k));
        let n4 = |o: usize| std::array::from_fn(|k| f(o + 4 * k));
        AimKeys {
            ads: i(0x4ec) != 0,
            overlay: i(0x438) != 0,
            ads_aim_pitch: f(0x6cc),
            rots: [v(0x240), v(0x264), v(0x288)],
            rot_min: [f(0x2b0), f(0x2b4), f(0x2b8)],
            pos_rot_rate: f(0x2a8),
            pos_prone_rot_rate: f(0x2ac),
            ads_idle_amount: f(0x48c),
            hip_idle_amount: f(0x490),
            ads_idle_speed: f(0x494),
            hip_idle_speed: f(0x498),
            idle_crouch: f(0x49c),
            idle_prone: f(0x4a0),
            ads_bob_factor: f(0x448),
            ads_view_bob_mult: f(0x44c),
            gun_max: [f(0x4a4), f(0x4a8)],
            sway: n6(0x4ac),
            ads_sway: n6(0x4c8),
            ads_spring: n4(0x6f0),
            hip_spring: n4(0x738),
            ads_gun_kick: n4(0x6e0),
            hip_gun_kick: n4(0x728),
            ads_reduced_percent: f(0x6dc),
            hip_reduced_percent: f(0x724),
        }
    }
}

/// `bg_viewKickScale`, `bg_viewKickMin`, `bg_viewKickMax` at their 1.7
/// defaults (cod4-combat.md 3.2).
const VIEW_KICK_SCALE: f32 = 0.2;
const VIEW_KICK_MIN: f32 = 5.0;
const VIEW_KICK_MAX: f32 = 90.0;

/// The damage kick `P_DamageFeedback` (0x80a5156) leaves in the gclient:
/// `damageTime` (+0x310c), `v_dmg_roll` (+0x3110), `v_dmg_pitch`
/// (+0x3114).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct DamageKick {
    /// `level.time - 20` at the hit; 0 for none.
    pub time: i32,
    pub pitch: f32,
    pub roll: f32,
}

impl DamageKick {
    /// A hit of `count` (the damage's share of the max health, at most
    /// 127) on a view at `view`: `count * bg_viewKickScale` within its min
    /// and max, along the view's forward as pitch and against its left as
    /// roll; with no direction (`damage_fromWorld`) the pitch is `-kick`.
    pub fn feedback(count: i32, from: Option<Vec3>, view: [f32; 3], level_time: i32) -> Self {
        let kick = (count as f32 * VIEW_KICK_SCALE).clamp(VIEW_KICK_MIN, VIEW_KICK_MAX);
        let (pitch, roll) = match from {
            None => (-kick, 0.0),
            Some(from) => {
                let axis = vcod_common::x87::angles_to_axis(Vec3::from(view));
                let dot = |v: Vec3| vcod_common::x87::dot(from, v);
                (
                    (dot(axis[0]) * d(kick)) as f32,
                    (dot(axis[1]) * -d(kick)) as f32,
                )
            }
        };
        DamageKick {
            time: level_time - 20,
            pitch,
            roll,
        }
    }

    /// The kick's share at `t` ms past the hit: `GetLeanFraction` up over
    /// `deflect`, down over `ret`; `None` once it is over.
    fn ramp(t: f32, deflect: f32, ret: f32) -> Option<f64> {
        if t < deflect {
            return Some(lean_fraction(t / deflect));
        }
        let r = 1.0 - (t - deflect) / ret;
        (r > 0.0).then(|| 1.0 - lean_fraction(1.0 - r))
    }

    /// `BG_CalculateView_DamageKick` (inlined in 0x806523e): factor `1 -
    /// frac / 2`, times `1 + frac / 2` up an overlay's sight; in over
    /// 100 ms, out over 400; pitch and roll.
    fn view(&self, a: &mut [f32; 3], frac: f32, overlay: bool, time: i32) {
        if self.time == 0 {
            return;
        }
        let mut f = -0.5 * frac + 1.0;
        if frac != 0.0 && overlay {
            f *= frac * 0.5 + 1.0;
        }
        let Some(l) = Self::ramp((time - self.time) as f32, 100.0, 400.0) else {
            return;
        };
        a[0] = (l * d(f) * d(self.pitch) + d(a[0])) as f32;
        a[2] = (l * d(f) * d(self.roll) + d(a[2])) as f32;
    }

    /// `BG_CalculateWeaponPosition_DamageKick` (inlined in 0x80659d0): `k =
    /// frac / 2 + 0.5`, in over `100 k` ms, out over `400 k`, scaled by
    /// `k` (times `1 - frac * 0.75` up an overlay's sight); half the
    /// pitch, the roll against the yaw, half the roll.
    fn gun(&self, a: &mut [f32; 3], frac: f32, overlay: bool, time: i32) {
        if self.time == 0 {
            return;
        }
        let k = 0.5 * frac + 0.5;
        let scale = if frac != 0.0 && overlay {
            (frac * -0.75 + 1.0) * k
        } else {
            k
        };
        let Some(l) = Self::ramp((time - self.time) as f32, 100.0 * k, 400.0 * k) else {
            return;
        };
        let l = l * d(scale);
        a[0] = (d(self.pitch) * l * 0.5 + d(a[0])) as f32;
        a[1] = (d(a[1]) - d(self.roll) * l) as f32;
        a[2] = (l * d(self.roll) * 0.5 + d(a[2])) as f32;
    }
}

/// The gclient's aim state, which `ClientSpawn`'s memset zeroes: the damage
/// kick (+0x310c..+0x3114), the sway's last view, offset and angles
/// (+0x3118, +0x3124, +0x3130), the movement tilt (+0x313c), the idle
/// factor (+0x3148), the gun spring's angles and speeds (+0x314c, +0x3158)
/// and the idle clock (+0x3164).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct AimState {
    pub kick: DamageKick,
    pub sway_view: [f32; 3],
    pub sway_offset: [f32; 3],
    pub sway_angles: [f32; 3],
    pub move_ang: [f32; 3],
    pub idle_factor: f32,
    pub gun_offset: [f32; 3],
    pub gun_speed: [f32; 3],
    pub idle_time: i32,
}

/// `BG_GetViewmodelWeaponIndex`: the offhand while it is in hand.
pub fn view_weapon(ps: &PlayerState) -> u32 {
    if ps.weap_flags & USING_OFFHAND != 0 {
        ps.off_hand_index
    } else {
        ps.weapon
    }
}

/// `BG_GetSpeed` (0x8056c38): the flat speed, or on a ladder the climb once
/// 500 ms past the last jump.
pub fn get_speed(ps: &PlayerState, time: i32) -> f32 {
    if ps.pm_flags & pmf::LADDER != 0 {
        if time - ps.jump_time > 499 {
            return ps.velocity.z;
        }
        return 0.0;
    }
    (d(ps.velocity.y) * d(ps.velocity.y) + d(ps.velocity.x) * d(ps.velocity.x)).sqrt() as f32
}

/// `BG_GetBobCycle` (0x805f8ca): the byte as a turn, plus one.
pub fn bob_cycle(ps: &PlayerState) -> f32 {
    let x = std::f32::consts::PI * ((ps.bob_cycle & 0xff) as f32 / 255.0);
    x + x + std::f32::consts::TAU
}

/// The stance's `bg_bobAmplitude*` off `viewHeightTarget` and the sprint
/// flag.
fn amplitude(ps: &PlayerState) -> [f32; 2] {
    match ps.view_height_target {
        11 => BOB_PRONE,
        40 => BOB_DUCKED,
        _ if ps.pm_flags & pmf::SPRINTING != 0 => BOB_SPRINTING,
        _ => BOB_STANDING,
    }
}

/// `BG_GetVerticalBobFactor` (0x805fac4): `(sinf(2c) + 0.2 sin(4c + pi/2))
/// * 0.75` of the capped amplitude, on the stack.
pub fn vertical_bob(ps: &PlayerState, cycle: f32, speed: f32, max: f32) -> f64 {
    let amp = (speed * amplitude(ps)[1]).min(max);
    let a = (cycle + cycle).sin();
    let b = f64::from(std::f32::consts::FRAC_PI_2 + cycle * 4.0).sin() as f32;
    d(amp) * (d(a) + d(b) * d(0.2f32)) * 0.75
}

/// `BG_GetHorizontalBobFactor` (0x805fa4c): `sinf(c)` of the capped
/// amplitude.
pub fn horizontal_bob(ps: &PlayerState, cycle: f32, speed: f32, max: f32) -> f64 {
    let amp = (speed * amplitude(ps)[0]).min(max);
    d(amp) * d(cycle.sin())
}

/// `GetLeanFraction` (0x81a91e8): `(2 - |f|) * f`, on the stack.
fn lean_fraction(f: f32) -> f64 {
    (2.0 - d(f.abs())) * d(f)
}

/// `BG_GetPlayerViewOrigin` (0x805399c) off a turret: the eye
/// `viewHeightCurrent` over the origin, the vertical bob on z, the
/// horizontal along the view's right, `AddLeanToPosition` (0x81a9f8e: the
/// lean fraction times 20 along the right of (0, yaw, 16 times it)), never
/// under 8 above the feet.
pub fn view_origin(ps: &PlayerState, time: i32) -> Vec3 {
    let mut o = ps.origin;
    o.z += ps.view_height_current;
    let cycle = bob_cycle(ps);
    let speed = get_speed(ps, time);
    o.z = (vertical_bob(ps, cycle, speed, BOB_MAX) + d(o.z)) as f32;
    let h = horizontal_bob(ps, cycle, speed, BOB_MAX) as f32;
    let right = vcod_common::x87::angle_vectors(Vec3::from(ps.view_angles))[1];
    o = Vec3::new(
        (d(right.x) * d(h) + d(o.x)) as f32,
        (d(right.y) * d(h) + d(o.y)) as f32,
        (d(right.z) * d(h) + d(o.z)) as f32,
    );
    if ps.leanf != 0.0 {
        let f = lean_fraction(ps.leanf) as f32;
        let ang = Vec3::new(0.0, ps.view_angles[1], 16.0 * f);
        let right = vcod_common::x87::angle_vectors(ang)[1];
        let s = f * 20.0;
        o = Vec3::new(
            (d(right.x) * d(s) + d(o.x)) as f32,
            (d(right.y) * d(s) + d(o.y)) as f32,
            (d(right.z) * d(s) + d(o.z)) as f32,
        );
    }
    let floor = d(ps.origin.z) + 8.0;
    if floor > d(o.z) {
        o.z = floor as f32;
    }
    o
}

/// The stance index off `eFlags`: 0 standing, 1 crouched, 2 prone.
fn stance(ps: &PlayerState) -> usize {
    if ps.e_flags & ef::PRONE != 0 {
        2
    } else if ps.e_flags & ef::CROUCH != 0 {
        1
    } else {
        0
    }
}

/// The idle drift's amount and speed: blended by the sight on an
/// `aimDownSight` weapon, 80 and 1 for a weapon with no hip amount.
fn idle_amount(k: &AimKeys, frac: f32) -> (f32, f32) {
    if k.ads {
        (
            (k.ads_idle_amount - k.hip_idle_amount) * frac + k.hip_idle_amount,
            (k.ads_idle_speed - k.hip_idle_speed) * frac + k.hip_idle_speed,
        )
    } else if k.hip_idle_amount == 0.0 {
        (80.0, 1.0)
    } else {
        (k.hip_idle_amount, k.hip_idle_speed)
    }
}

/// The idle factor's stance target.
fn idle_target(k: &AimKeys, ps: &PlayerState) -> f32 {
    match stance(ps) {
        2 => k.idle_prone,
        1 => k.idle_crouch,
        _ => 1.0,
    }
}

/// The idle factor eased toward its target at 0.5 a second.
fn ease_idle_factor(cur: &mut f32, target: f32, dt: f32) {
    if *cur == target {
        return;
    }
    if *cur < target {
        *cur = (d(0.5f32) * d(dt) + d(*cur)) as f32;
        if target < *cur {
            *cur = target;
        }
    } else {
        *cur = (d(-0.5f32) * d(dt) + d(*cur)) as f32;
        if *cur < target {
            *cur = target;
        }
    }
}

/// The weapon bob's three angles (`..._BobOffset`, `View_BobAngles`): the
/// cycle a quarter turn on, at 0.16 of the speed, capped at 10, scaled by
/// `adsBobFactor` up the sight.
fn bob_angles(ps: &PlayerState, k: &AimKeys, xyspeed: f32) -> [f64; 3] {
    let cycle =
        (f64::from(std::f32::consts::FRAC_PI_4) + d(bob_cycle(ps)) + std::f64::consts::TAU) as f32;
    let cycle = (d(cycle) + std::f64::consts::TAU) as f32;
    let speed = xyspeed * 0.16;
    let mut p = -(vertical_bob(ps, cycle, speed, 10.0) as f32);
    let mut y = -horizontal_bob(ps, cycle, speed, 10.0) as f32 as f64;
    let c2 = (d(cycle) - 0.471_238_94) as f32;
    let h = horizontal_bob(ps, c2, speed * 1.5, 10.0);
    let mut r = if -h < 0.0 { h } else { 0.0 };
    let frac = d(ps.weapon_pos_frac);
    if frac != 0.0 {
        let s = 1.0 - (1.0 - d(k.ads_bob_factor)) * frac;
        p = (d(p) * s) as f32;
        y *= s;
        r *= s;
    }
    [d(p), y, r]
}

impl AimState {
    /// `BG_CalculateViewAngles` (0x806523e): the damage kick, an overlay
    /// weapon's idle drift and walk bob up the sight, and the sighted view
    /// bob (`adsViewBobMult` of the bob at the flat speed, capped at 45)
    /// off a turret.
    fn view_offsets(
        &mut self,
        ps: &PlayerState,
        k: &AimKeys,
        xyspeed: f32,
        dt: f32,
        time: i32,
    ) -> [f32; 3] {
        let mut a = [0f32; 3];
        let frac = ps.weapon_pos_frac;
        self.kick.view(&mut a, frac, k.overlay, time);
        if k.overlay {
            let (amount, speed) = idle_amount(k, frac);
            let target = idle_target(k, ps);
            if frac != 0.0 {
                ease_idle_factor(&mut self.idle_factor, target, dt);
            }
            let scale = amount * self.idle_factor * frac * ps.hold_breath_scale;
            self.idle_time += (ps.hold_breath_scale * dt * 1000.0 * speed) as i32;
            let t = self.idle_time as f32;
            a[1] = (d((0.0007f32 * t).sin()) * d(scale) * 0.01 + d(a[1])) as f32;
            a[0] = (d(scale) * d((0.001f32 * t).sin()) * 0.01 + d(a[0])) as f32;
            let [p, y, r] = bob_angles(ps, k, xyspeed);
            let f = d(frac);
            a[0] = (p * f + d(a[0])) as f32;
            a[1] = (y * f + d(a[1])) as f32;
            a[2] = (r * f + d(a[2])) as f32;
        }
        if ps.e_flags & ef::TURRET == 0 && frac != 0.0 && k.ads_view_bob_mult != 0.0 {
            let cycle = bob_cycle(ps);
            let v = vertical_bob(ps, cycle, xyspeed, 45.0) as f32;
            a[0] -= frac * k.ads_view_bob_mult * v;
            let h = horizontal_bob(ps, cycle, xyspeed, 45.0);
            a[1] = (d(a[1]) - d(frac) * d(k.ads_view_bob_mult) * h) as f32;
        }
        a
    }

    /// `BG_CalculateWeaponPosition_Sway` (0x8060f20) at scale 1 (1.7 passes
    /// no shellshock scale): the view's turn since the last cmd, per 60th
    /// of a second and clamped, tracked into an offset and angles.
    fn sway(&mut self, ps: &PlayerState, k: &AimKeys, msec: i32) {
        if msec == 0 {
            return;
        }
        let f = ps.weapon_pos_frac;
        let s = if k.ads {
            if f > 0.0 && k.overlay {
                return;
            }
            std::array::from_fn::<f32, 6, _>(|i| (k.ads_sway[i] - k.sway[i]) * f + k.sway[i])
        } else {
            k.sway
        };
        let [max, lerp, pitch, yaw, horiz, vert] = s;
        let dt = 0.001f32 * msec as f32;
        let scale = 1.0 / (dt * 60.0);
        let delta: [f32; 2] = std::array::from_fn(|i| {
            let mut x = ps.view_angles[i] - self.sway_view[i];
            while x > 180.0 {
                x -= 360.0;
            }
            while x < -180.0 {
                x += 360.0;
            }
            (x * scale).clamp(-max, max)
        });
        self.sway_offset[1] = diff_track(horiz * delta[1], self.sway_offset[1], lerp, dt);
        self.sway_offset[2] = diff_track(vert * delta[0], self.sway_offset[2], lerp, dt);
        self.sway_angles[0] = diff_track_angle(pitch * delta[0], self.sway_angles[0], lerp, dt);
        self.sway_angles[1] = diff_track_angle(yaw * delta[1], self.sway_angles[1], lerp, dt);
        self.sway_view = ps.view_angles;
    }

    /// `BG_CalculateWeaponAngles` (0x80659d0).
    fn weapon_angles(
        &mut self,
        ps: &PlayerState,
        k: &AimKeys,
        xyspeed: f32,
        dt: f32,
        time: i32,
    ) -> [f32; 3] {
        let mut a = [0f32; 3];
        if ps.leanf != 0.0 {
            a[2] = (lean_fraction(ps.leanf) * -2.0 + d(a[2])) as f32;
        }
        let frac = ps.weapon_pos_frac;
        if k.ads {
            a[0] += frac * k.ads_aim_pitch;
        }
        // The movement tilt: the stance's turn past its min speed, eased.
        let st = stance(ps);
        let min = k.rot_min[st];
        let mut target = if xyspeed <= min || ps.weapon_state == super::weapon::state::RELOAD {
            [0.0; 3]
        } else {
            let t = ((xyspeed - min) / (ps.speed as f32 - min)).clamp(0.0, 1.0);
            k.rots[st].map(|r| r * t)
        };
        if frac != 0.0 {
            target = target.map(|t| t * (1.0 - frac));
        }
        let rate = if ps.view_height_current == 11.0 {
            k.pos_prone_rot_rate
        } else {
            k.pos_rot_rate
        };
        for (last, &tgt) in self.move_ang.iter_mut().zip(&target) {
            if *last == tgt {
                continue;
            }
            let step = (tgt - *last) * dt * rate;
            if *last < tgt {
                *last += step.max(0.1 * dt);
                if tgt < *last {
                    *last = tgt;
                }
            } else {
                *last += step.min(-0.1 * dt);
                if *last < tgt {
                    *last = tgt;
                }
            }
        }
        if frac == 0.0 {
            (0..3).for_each(|i| a[i] += self.move_ang[i]);
        } else if frac < 0.5 {
            let s = frac * -2.0 + 1.0;
            (0..3).for_each(|i| a[i] += self.move_ang[i] * s);
        }
        // The idle drift.
        let (amount, speed) = idle_amount(k, frac);
        if !k.overlay || frac == 0.0 {
            ease_idle_factor(&mut self.idle_factor, idle_target(k, ps), dt);
        }
        let mut scale = amount * self.idle_factor;
        if k.overlay {
            scale *= 1.0 - frac;
        }
        self.idle_time += (dt * 1000.0 * speed) as i32;
        let t = self.idle_time as f32;
        a[2] = (d((0.0005f32 * t).sin()) * d(scale) * 0.01 + d(a[2])) as f32;
        a[1] = (d((0.0007f32 * t).sin()) * d(scale) * 0.01 + d(a[1])) as f32;
        a[0] = (d(scale) * d((0.001f32 * t).sin()) * 0.01 + d(a[0])) as f32;
        // The walk bob, gone up an overlay's sight.
        let [mut p, mut y, mut r] = bob_angles(ps, k, xyspeed);
        if k.overlay {
            let s = 1.0 - d(frac);
            p *= s;
            y *= s;
            r *= s;
        }
        a[0] = (d(a[0]) + p) as f32;
        a[1] = (y + d(a[1])) as f32;
        a[2] = (r + d(a[2])) as f32;
        self.kick.gun(&mut a, frac, k.overlay, time);
        // The gun spring, on a sight weapon.
        if k.ads {
            let lerp = |i: usize| (k.ads_spring[i] - k.hip_spring[i]) * frac + k.hip_spring[i];
            let spring = [lerp(0), lerp(1), lerp(2), lerp(3)];
            let mut total = dt;
            while total > 0.0 {
                let step = if total <= 0.005 {
                    std::mem::replace(&mut total, 0.0)
                } else {
                    total -= 0.005;
                    0.005
                };
                let p = spring_axis(
                    &mut self.gun_offset[0],
                    &mut self.gun_speed[0],
                    step,
                    k.gun_max[0],
                    spring,
                );
                let y = spring_axis(
                    &mut self.gun_offset[1],
                    &mut self.gun_speed[1],
                    step,
                    k.gun_max[1],
                    spring,
                );
                if p && y {
                    break;
                }
            }
            (0..3).for_each(|i| a[i] += self.gun_offset[i]);
        }
        a[0] = angle_delta(a[0], self.sway_angles[0]);
        a[1] = angle_delta(a[1], self.sway_angles[1]);
        a
    }

    /// `ClientThink_real`'s block before `Pmove`, on the playerstate as the
    /// last cmd left it, `msec` the cmd's (1..200): the pitch and yaw the
    /// cmd's rounds leave along. `view` and `held` are the view weapon's and
    /// `ps.weapon`'s keys.
    pub fn fire_angles(
        &mut self,
        ps: &PlayerState,
        view: &AimKeys,
        held: &AimKeys,
        level_time: i32,
        msec: i32,
    ) -> [f32; 2] {
        let xyspeed = get_speed(ps, level_time);
        let dt = 0.001f32 * msec as f32;
        let off = self.view_offsets(ps, view, xyspeed, dt, level_time);
        let sum: [f32; 3] = std::array::from_fn(|i| ps.view_angles[i] + off[i]);
        self.sway(ps, view, msec);
        let w = self.weapon_angles(ps, view, xyspeed, dt, level_time);
        if held.ads && ps.weapon_pos_frac != 0.0 && !held.overlay {
            let a = vcod_common::x87::angles_to_axis(Vec3::from(w));
            let b = vcod_common::x87::angles_to_axis(Vec3::from(sum));
            return axis_to_angles(matrix_multiply(a, b));
        }
        [sum[0], sum[1]]
    }

    /// `BG_WeaponFireRecoil` (0x80609fc) off a fire event: four `G_random`
    /// draws (the view's pitch and yaw, which the client keeps, then the
    /// gun's), the gun's added to the spring's speeds. The ranges are the
    /// sight's at a full sight, the hip's otherwise (the gun's at any sight
    /// above 0 the sight's); a kick-restricted weapon scales by its
    /// reduced percent.
    pub fn fire_recoil(&mut self, ps: &PlayerState, k: &AimKeys, mut random: impl FnMut() -> f64) {
        let frac = ps.weapon_pos_frac;
        let scale = if ps.weapon_restrict_kick_time < 1 {
            1.0
        } else if frac == 1.0 {
            0.01 * k.ads_reduced_percent
        } else {
            0.01 * k.hip_reduced_percent
        };
        // The view kick's two draws.
        random();
        random();
        let kick = if frac > 0.0 {
            k.ads_gun_kick
        } else {
            k.hip_gun_kick
        };
        let p = (d(kick[0]) + random() * d(kick[1] - kick[0])) as f32;
        let y = random() * d(kick[3] - kick[2]) + d(kick[2]);
        self.gun_speed[0] += p * scale;
        self.gun_speed[1] = (y * d(scale) + d(self.gun_speed[1])) as f32;
    }
}

/// `DiffTrackAngle`: [`diff_track`] the short way round, back into
/// -180..180.
fn diff_track_angle(mut tgt: f32, cur: f32, rate: f32, dt: f32) -> f32 {
    while tgt - cur > 180.0 {
        tgt -= 360.0;
    }
    while tgt - cur < -180.0 {
        tgt += 360.0;
    }
    let a = diff_track(tgt, cur, rate, dt) / 360.0;
    (a - (a + 0.5).floor()) * 360.0
}

/// `BG_CalculateWeaponPosition_GunRecoil_SingleAngle`: one slice of one
/// axis; true when the axis was at rest and is zeroed.
fn spring_axis(angle: &mut f32, speed: &mut f32, dt: f32, max: f32, k: [f32; 4]) -> bool {
    let [accel, speed_max, speed_decay, static_decay] = k;
    if angle.abs() < 0.25 && speed.abs() < 1.0 {
        *angle = 0.0;
        *speed = 0.0;
        return true;
    }
    *angle += *speed * dt;
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
        *speed -= accel * dt;
    } else if *angle < 0.0 {
        *speed += accel * dt;
    }
    *speed -= *speed * speed_decay * dt;
    if *speed > 0.0 {
        *speed = (*speed - dt * static_decay).max(0.0);
    } else {
        *speed = (dt * static_decay + *speed).min(0.0);
    }
    *speed = speed.clamp(-speed_max, speed_max);
    false
}

fn matrix_multiply(a: [Vec3; 3], b: [Vec3; 3]) -> [Vec3; 3] {
    let el = |i: usize, j: usize| {
        (d(a[i][0]) * d(b[0][j]) + d(a[i][1]) * d(b[1][j]) + d(a[i][2]) * d(b[2][j])) as f32
    };
    std::array::from_fn(|i| Vec3::new(el(i, 0), el(i, 1), el(i, 2)))
}

/// `AxisToAngles`' pitch and yaw: `vectoangles` of the forward.
fn axis_to_angles(m: [Vec3; 3]) -> [f32; 2] {
    let v = m[0];
    if v.y == 0.0 && v.x == 0.0 {
        return [if v.z > 0.0 { 270.0 } else { 90.0 }, 0.0];
    }
    let mut yaw = (d(v.y).atan2(d(v.x)).to_degrees()) as f32;
    if yaw < 0.0 {
        yaw += 360.0;
    }
    let flat = (d(v.y) * d(v.y) + d(v.x) * d(v.x)).sqrt();
    let mut pitch = (-d(v.z).atan2(flat).to_degrees()) as f32;
    if pitch < 0.0 {
        pitch += 360.0;
    }
    [pitch, yaw]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_still_unleaned_eye_sits_at_the_view_height() {
        let ps = PlayerState {
            origin: Vec3::new(10.0, 20.0, 30.0),
            view_height_current: 60.0,
            ..PlayerState::default()
        };
        assert_eq!(view_origin(&ps, 0), Vec3::new(10.0, 20.0, 90.0));
    }

    #[test]
    fn a_full_lean_right_moves_the_eye_twenty_units_less_the_roll() {
        let ps = PlayerState {
            view_height_current: 60.0,
            leanf: 1.0,
            ..PlayerState::default()
        };
        // Facing +x, the right is -y; rolled 16 degrees it dips.
        let o = view_origin(&ps, 0);
        let r = 16f32.to_radians();
        assert!((o.y + 20.0 * r.cos()).abs() < 1e-3, "{o}");
        assert!((o.z - (60.0 - 20.0 * r.sin())).abs() < 1e-3, "{o}");
    }

    #[test]
    fn a_hip_shot_with_no_damage_leaves_along_the_view() {
        let ps = PlayerState {
            view_angles: [3.0, 40.0, 0.0],
            ..PlayerState::default()
        };
        let k = AimKeys::default();
        let mut s = AimState {
            sway_view: ps.view_angles,
            ..AimState::default()
        };
        assert_eq!(s.fire_angles(&ps, &k, &k, 0, 16), [3.0, 40.0]);
    }

    /// A hit of 40 with no direction kicks a hip shot 8 degrees up, whole
    /// 100 ms past the hit's `level.time - 20`, gone 400 ms later; one from
    /// the right rolls the view, which leaves the hip shot alone.
    #[test]
    fn a_hit_kicks_the_next_hip_shots_and_eases_out() {
        let ps = PlayerState {
            view_angles: [3.0, 40.0, 0.0],
            ..PlayerState::default()
        };
        let k = AimKeys::default();
        let mut s = AimState {
            sway_view: ps.view_angles,
            kick: DamageKick::feedback(40, None, ps.view_angles, 1000),
            ..AimState::default()
        };
        assert_eq!(s.kick.pitch, -8.0);
        assert_eq!(s.fire_angles(&ps, &k, &k, 1080, 16), [-5.0, 40.0]);
        let mid = s.fire_angles(&ps, &k, &k, 1280, 16)[0];
        assert!(-5.0 < mid && mid < 3.0, "{mid}");
        assert_eq!(s.fire_angles(&ps, &k, &k, 1480, 16), [3.0, 40.0]);
        let right = vcod_common::x87::angle_vectors(Vec3::from(ps.view_angles))[1];
        let kick = DamageKick::feedback(40, Some(right), ps.view_angles, 1000);
        assert!(
            kick.pitch.abs() < 1e-5 && (kick.roll - 8.0).abs() < 1e-5,
            "{kick:?}"
        );
    }
}
