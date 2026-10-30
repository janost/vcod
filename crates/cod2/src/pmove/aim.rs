//! The server's aim block: what `ClientThink_real` (lnxded 0x80f5d2e, Mac
//! 0x1a3d00) works out before `Pmove`, the pitch and yaw a cmd's shots
//! leave along (`gclient+0x27d8`). The view's offsets
//! (`BG_CalculateViewAngles`, 0x80f24c8), the turn sway
//! (`BG_CalculateWeaponPosition_Sway`, 0x80f2524) and the weapon's angles
//! (`BG_CalculateWeaponAngles`, 0x80f1c60: the sight's pitch, the movement
//! tilt, the idle drift, the walk bob, the damage kick, the gun spring, the
//! sway), the two multiplied through their axes on a sighted gun without an
//! overlay. The gun spring is kicked by `BG_WeaponFireRecoil` (0x80f095a)
//! off each fire event. docs/research/cod2-combat.md 9.6 has the evidence.
//!
//! `cod2_lnxded` is an unoptimized gcc build: every expression runs on the
//! x87 stack (here `f64`) and every assignment to a float is stored, so
//! rounded to `f32` ([`super::fpu`]).

use glam::Vec3;
use vcod_common::rng::GlibcRand;

use super::fpu::{angle_normalize180, angle_subtract, d, sqrt};
use super::weapon::{WeaponState, Weapons};
use super::{PlayerState, VIEW_CROUCH, VIEW_PRONE, ef, pmf};

/// `bg_bobAmplitudeStanding`, `Ducked`, `Prone` and `bg_bobMax` at their
/// defaults (cod2-movement.md 13).
const BOB_AMPLITUDE: [f32; 3] = [0.007, 0.0075, 0.03];
pub const BOB_MAX: f32 = 8.0;

/// `weaponstate` 5, reloading: no movement tilt.
const WEAPON_RELOADING: i32 = 5;

/// The weapon file's keys the block reads, by `WeaponDef` offset.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AimKeys {
    /// +0x32c `aimDownSight`.
    pub ads: bool,
    /// +0x278 `adsOverlayReticle` not `none`.
    pub overlay: bool,
    /// +0x46c.
    pub ads_aim_pitch: f32,
    /// +0x138, +0x15c, +0x180: `standRot`, `duckedRot`, `proneRot`.
    pub rots: [[f32; 3]; 3],
    /// +0x1a8..+0x1b0: their `*RotMinSpeed`.
    pub rot_min: [f32; 3],
    /// +0x1a0, +0x1a4.
    pub pos_rot_rate: f32,
    pub pos_prone_rot_rate: f32,
    /// +0x2c8, +0x2cc, +0x2d0, +0x2d4, +0x2d8, +0x2dc.
    pub ads_idle_amount: f32,
    pub hip_idle_amount: f32,
    pub ads_idle_speed: f32,
    pub hip_idle_speed: f32,
    pub idle_crouch: f32,
    pub idle_prone: f32,
    /// +0x284, +0x288.
    pub ads_bob_factor: f32,
    pub ads_view_bob_mult: f32,
    /// +0x2e0, +0x2e4: `gunMaxPitch`, `gunMaxYaw`.
    pub gun_max: [f32; 2],
    /// +0x2e8..+0x2fc and +0x304..+0x318: `MaxAngle`, `LerpSpeed`,
    /// `PitchScale`, `YawScale`, `HorizScale`, `VertScale`.
    pub sway: [f32; 6],
    pub ads_sway: [f32; 6],
    /// `GunKick` `Accel`, `SpeedMax`, `SpeedDecay`, `StaticDecay`: ads
    /// +0x490.., hip +0x4d8...
    pub ads_spring: [f32; 4],
    pub hip_spring: [f32; 4],
    /// `GunKickPitchMin/Max`, `YawMin/Max`, `ViewKick` the same: ads
    /// +0x480.. and +0x4a0.., hip +0x4c8.. and +0x4e8...
    pub ads_gun_kick: [f32; 4],
    pub hip_gun_kick: [f32; 4],
    pub ads_view_kick: [f32; 4],
    pub hip_view_kick: [f32; 4],
    /// +0x47c, +0x4c4: `*GunKickReducedKickPercent`.
    pub ads_reduced_percent: f32,
    pub hip_reduced_percent: f32,
}

impl AimKeys {
    pub fn of(d: &crate::weapon::WeaponDef) -> AimKeys {
        let f = |k: &str| d.float(k);
        let v3 = |p: &str| ["P", "Y", "R"].map(|s| f(&format!("{p}Rot{s}")));
        let six = |p: &str| {
            [
                "MaxAngle",
                "LerpSpeed",
                "PitchScale",
                "YawScale",
                "HorizScale",
                "VertScale",
            ]
            .map(|s| f(&format!("{p}{s}")))
        };
        let spring = |p: &str| {
            ["Accel", "SpeedMax", "SpeedDecay", "StaticDecay"].map(|s| f(&format!("{p}GunKick{s}")))
        };
        let kick =
            |p: &str| ["PitchMin", "PitchMax", "YawMin", "YawMax"].map(|s| f(&format!("{p}{s}")));
        AimKeys {
            ads: d.flag("aimDownSight"),
            overlay: d.int("adsOverlayReticle") != 0,
            ads_aim_pitch: f("adsAimPitch"),
            rots: [v3("stand"), v3("ducked"), v3("prone")],
            rot_min: ["stand", "ducked", "prone"].map(|s| f(&format!("{s}RotMinSpeed"))),
            pos_rot_rate: f("posRotRate"),
            pos_prone_rot_rate: f("posProneRotRate"),
            ads_idle_amount: f("adsIdleAmount"),
            hip_idle_amount: f("hipIdleAmount"),
            ads_idle_speed: f("adsIdleSpeed"),
            hip_idle_speed: f("hipIdleSpeed"),
            idle_crouch: f("idleCrouchFactor"),
            idle_prone: f("idleProneFactor"),
            ads_bob_factor: f("adsBobFactor"),
            ads_view_bob_mult: f("adsViewBobMult"),
            gun_max: [f("gunMaxPitch"), f("gunMaxYaw")],
            sway: six("sway"),
            ads_sway: six("adsSway"),
            ads_spring: spring("ads"),
            hip_spring: spring("hip"),
            ads_gun_kick: kick("adsGunKick"),
            hip_gun_kick: kick("hipGunKick"),
            ads_view_kick: kick("adsViewKick"),
            hip_view_kick: kick("hipViewKick"),
            ads_reduced_percent: f("adsGunKickReducedKickPercent"),
            hip_reduced_percent: f("hipGunKickReducedKickPercent"),
        }
    }
}

/// The gclient's aim state, +0x2840..+0x289c, which `ClientSpawn`'s memset
/// zeroes and nothing else resets.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct AimState {
    /// +0x2840, +0x2844, +0x2848: the damage kick `P_DamageFeedback`
    /// starts (its time 20 ms before the frame), its roll and pitch.
    pub kick_time: i32,
    pub kick_roll: f32,
    pub kick_pitch: f32,
    /// +0x284c: the view the sway last read.
    pub sway_view: [f32; 3],
    /// +0x2858, +0x2864: the sway's offset and angles.
    pub sway_offset: [f32; 3],
    pub sway_angles: [f32; 3],
    /// +0x2870: the movement tilt.
    pub move_rot: [f32; 3],
    /// +0x287c: the idle drift's stance factor.
    pub idle_factor: f32,
    /// +0x2880, +0x288c: the gun spring's angles and speeds (pitch, yaw).
    pub spring: [f32; 3],
    pub spring_speed: [f32; 3],
    /// +0x2898: the idle drift's clock, ms at the idle speed.
    pub idle_time: i32,
}

/// What one cmd's block reads off the client.
pub struct AimInput<'a> {
    pub ps: &'a PlayerState,
    pub ws: &'a WeaponState,
    pub defs: &'a dyn Weapons,
    pub level_time: i32,
    /// The cmd's ms, 1..200.
    pub msec: i32,
}

impl AimInput<'_> {
    fn keys(&self, weapon: u8) -> AimKeys {
        self.defs
            .get(weapon)
            .map(|w| w.aim.clone())
            .unwrap_or_default()
    }

    fn view_keys(&self) -> AimKeys {
        self.keys(super::weapon::view_weapon(self.ps, self.ws, self.defs))
    }

    fn pos(&self) -> f32 {
        self.ws.pos_frac
    }
}

/// `BG_GetSpeed` (0x80e9528): the flat speed, or on a ladder the climb
/// once 500 ms past the last jump.
pub fn get_speed(ps: &PlayerState, level_time: i32) -> f32 {
    if ps.pm_flags & pmf::LADDER == 0 {
        sqrt(d(ps.velocity.y) * d(ps.velocity.y) + d(ps.velocity.x) * d(ps.velocity.x))
    } else if level_time - ps.jump_time < 500 {
        0.0
    } else {
        ps.velocity.z
    }
}

/// `BG_GetBobCycle` (0x80f0bde): `2 pi` past twice the cycle byte's turn,
/// stored.
pub fn bob_cycle(ps: &PlayerState) -> f32 {
    let t = turn(ps);
    (t + t + std::f64::consts::TAU) as f32
}

/// The cycle byte's half turn, `byte / 255 * pi` on the stack.
fn turn(ps: &PlayerState) -> f64 {
    f64::from(ps.bob_cycle & 0xff) / 255.0 * std::f64::consts::PI
}

fn bob_amplitude(ps: &PlayerState, speed: f32, max: f32) -> f32 {
    let amp = match ps.view_height_target {
        VIEW_PRONE => BOB_AMPLITUDE[2],
        VIEW_CROUCH => BOB_AMPLITUDE[1],
        _ => BOB_AMPLITUDE[0],
    };
    let a = (d(speed) * d(amp)) as f32;
    if max < a { max } else { a }
}

/// `BG_GetVerticalBobFactor` (0x80f0c3a): each sine and each product stored.
pub fn vertical_bob(ps: &PlayerState, cycle: f32, speed: f32, max: f32) -> f32 {
    let amp = bob_amplitude(ps, speed, max);
    let s1 = (d(cycle) + d(cycle)).sin() as f32;
    let s2 = (d(cycle) * 4.0 + std::f64::consts::FRAC_PI_2).sin() as f32;
    let a = (d(s2) * d(0.2f32) + d(s1)) as f32;
    let a = (d(0.75f32) * d(a)) as f32;
    (d(a) * d(amp)) as f32
}

/// `BG_GetHorizontalBobFactor` (0x80f0cfa): the sine stored, the product
/// handed back on the stack (exact in `f64`).
pub fn horizontal_bob(ps: &PlayerState, cycle: f32, speed: f32, max: f32) -> f64 {
    let amp = bob_amplitude(ps, speed, max);
    d(d(cycle).sin() as f32) * d(amp)
}

/// `GetLeanFraction` (0x80b8c60): `(2 - |f|) * f` on the stack.
fn lean_fraction(f: f32) -> f64 {
    (2.0 - d(f.abs())) * d(f)
}

/// `DiffTrack` (0x80a3cea): `cur` toward `target` by `rate * dt` of the
/// gap, the target outright inside 0.001 or past it.
fn diff_track(target: f32, cur: f32, rate: f32, dt: f32) -> f32 {
    let gap = target - cur;
    let step = (d(rate) * d(gap) * d(dt)) as f32;
    if d(gap.abs()) > d(0.001f32) {
        if gap.abs() < step.abs() {
            target
        } else {
            cur + step
        }
    } else {
        target
    }
}

/// `DiffTrackAngle` (0x80a3d66): [`diff_track`] the short way round, then
/// `AngleNormalize180`.
fn diff_track_angle(mut target: f32, cur: f32, rate: f32, dt: f32) -> f32 {
    while target - cur > 180.0 {
        target -= 360.0;
    }
    while target - cur < -180.0 {
        target += 360.0;
    }
    angle_normalize180(diff_track(target, cur, rate, dt))
}

fn stance(ps: &PlayerState) -> usize {
    if ps.e_flags & ef::PRONE != 0 {
        2
    } else if ps.e_flags & ef::CROUCH != 0 {
        1
    } else {
        0
    }
}

impl AimState {
    /// `BG_CalculateViewAngles` (0x80f24c8): the damage kick, the idle drift
    /// and walk bob of an overlay's sight, the sighted view bob.
    fn view_offsets(&self, inp: &AimInput, xyspeed: f32) -> [f32; 3] {
        let mut out = [0.0f32; 3];
        let ps = inp.ps;
        let k = inp.view_keys();
        let pos = inp.pos();
        // 0x80f1d52: the damage kick.
        if self.kick_time != 0 {
            let mut f = (1.0 - d(0.5f32) * d(pos)) as f32;
            if pos != 0.0 && k.overlay {
                f = (d(f) * (d(0.5f32) * d(pos) + 1.0)) as f32;
            }
            let t = (inp.level_time - self.kick_time) as f32;
            if t < 100.0 {
                let g = lean_fraction((d(t) / 100.0) as f32);
                out[0] = (d(out[0]) + g * d(f) * d(self.kick_pitch)) as f32;
                out[2] = (d(out[2]) + g * d(f) * d(self.kick_roll)) as f32;
            } else {
                let r = (1.0 - (d(t) - 100.0) / 400.0) as f32;
                if r > 0.0 {
                    let g = lean_fraction(1.0 - r);
                    let f = ((1.0 - g) * d(f)) as f32;
                    out[0] = (d(out[0]) + d(f) * d(self.kick_pitch)) as f32;
                    out[2] = (d(out[2]) + d(f) * d(self.kick_roll)) as f32;
                }
            }
        }
        // 0x80f1ede and 0x80f21ba, the overlay's idle drift and walk bob,
        // are left out: no overlay sight's shot is multiplied through the
        // weapon's angles, and the view's own stays the client's.
        // 0x80f237e: the sighted view bob.
        if ps.e_flags & ef::TURRET == 0 && pos != 0.0 && k.ads_view_bob_mult != 0.0 {
            let cycle = bob_cycle(ps);
            let v = vertical_bob(ps, cycle, xyspeed, 45.0);
            let v = (d(pos) * d(k.ads_view_bob_mult) * d(v)) as f32;
            out[0] -= v;
            let h = horizontal_bob(ps, cycle, xyspeed, 45.0) as f32;
            let h = (d(pos) * d(k.ads_view_bob_mult) * d(h)) as f32;
            out[1] -= h;
        }
        out
    }

    /// `BG_CalculateWeaponPosition_Sway` (0x80f2524) at scale 1: the view's
    /// turn since the last cmd, per 60th of a second and clamped, is what
    /// the offset and the angles ease toward. A sighted overlay freezes it.
    fn sway(&mut self, inp: &AimInput) {
        if inp.msec == 0 {
            return;
        }
        let pos = inp.pos();
        let k = inp.view_keys();
        let dt = (d(inp.msec as f32) * d(0.001f32)) as f32;
        let s = if !k.ads {
            k.sway
        } else {
            if pos > 0.0 && k.overlay {
                return;
            }
            std::array::from_fn(|i| {
                ((d(k.ads_sway[i]) - d(k.sway[i])) * d(pos) + d(k.sway[i])) as f32
            })
        };
        let [max, rate, pitch_s, yaw_s, horiz_s, vert_s] = s;
        let view = inp.ps.view_angles;
        let mut delta = [0.0f32; 3];
        for i in 0..3 {
            delta[i] = angle_subtract(view[i], self.sway_view[i]);
        }
        let per = (1.0 / (d(dt) * 60.0)) as f32;
        let delta = delta.map(|v| (d(per) * d(v)) as f32);
        let clamp = |v: f32| {
            let m = if v - max >= 0.0 { max } else { v };
            if -max - m >= 0.0 { -max } else { m }
        };
        let dp = clamp(delta[0]);
        let dy = clamp(delta[1]);
        let horiz = (d(dy) * d(horiz_s)) as f32;
        let vert = (d(dp) * d(vert_s)) as f32;
        self.sway_offset[1] = diff_track(horiz, self.sway_offset[1], rate, dt);
        self.sway_offset[2] = diff_track(vert, self.sway_offset[2], rate, dt);
        let p = (d(dp) * d(pitch_s)) as f32;
        let y = (d(dy) * d(yaw_s)) as f32;
        self.sway_angles[0] = diff_track_angle(p, self.sway_angles[0], rate, dt);
        self.sway_angles[1] = diff_track_angle(y, self.sway_angles[1], rate, dt);
        self.sway_view = view;
    }

    /// `BG_CalculateWeaponAngles` (0x80f1c60).
    fn weapon_angles(&mut self, inp: &AimInput, xyspeed: f32) -> [f32; 3] {
        let ps = inp.ps;
        let k = inp.view_keys();
        let pos = inp.pos();
        let dt = (d(inp.msec as f32) * d(0.001f32)) as f32;
        let mut out = [0.0f32; 3];
        if ps.leanf != 0.0 {
            let l = lean_fraction(ps.leanf) as f32;
            out[2] -= l + l;
        }
        // 0x80f1140: the sight's pitch, then 0x80f0d74, the movement tilt.
        if k.ads {
            out[0] = (d(out[0]) + d(pos) * d(k.ads_aim_pitch)) as f32;
        }
        self.tilt(inp, &k, xyspeed, dt, &mut out);
        self.idle(inp, &k, dt, &mut out);
        walk_bob(ps, &k, pos, xyspeed, &mut out);
        self.damage_kick(inp, &k, &mut out);
        if k.ads {
            self.spring_step(&k, pos, dt);
            out[0] = (d(out[0]) + d(self.spring[0])) as f32;
            out[1] = (d(out[1]) + d(self.spring[1])) as f32;
            out[2] = (d(out[2]) + d(self.spring[2])) as f32;
        }
        out[0] = angle_subtract(out[0], self.sway_angles[0]);
        out[1] = angle_subtract(out[1], self.sway_angles[1]);
        out
    }

    /// 0x80f0d74: the stance's `*Rot` past its `*RotMinSpeed`, eased at
    /// `posRotRate` (`posProneRotRate` at the prone height), at least 0.1
    /// a second, faded over the first half of the sight.
    fn tilt(&mut self, inp: &AimInput, k: &AimKeys, xyspeed: f32, dt: f32, out: &mut [f32; 3]) {
        let ps = inp.ps;
        let pos = inp.pos();
        let st = stance(ps);
        let min = k.rot_min[st];
        let mut target = [0.0f32; 3];
        if xyspeed > min && inp.ws.state != WEAPON_RELOADING {
            let f = ((d(xyspeed) - d(min)) / (d(ps.speed as f32) - d(min))) as f32;
            let f = if 0.0 - f >= 0.0 { 0.0 } else { f };
            let f = if f - 1.0 >= 0.0 { 1.0 } else { f };
            target = k.rots[st].map(|r| (d(f) * d(r)) as f32);
        }
        if pos != 0.0 {
            let m = 1.0 - pos;
            target = target.map(|t| (d(m) * d(t)) as f32);
        }
        let rate = if ps.view_height_current == 11.0 {
            k.pos_prone_rot_rate
        } else {
            k.pos_rot_rate
        };
        for (rot, &t) in self.move_rot.iter_mut().zip(&target) {
            let cur = *rot;
            if cur == t {
                continue;
            }
            let mut step = ((d(t) - d(cur)) * d(dt) * d(rate)) as f32;
            if cur < t {
                let floor = d(0.1f32) * d(dt);
                if d(step) < floor {
                    step = floor as f32;
                }
                let v = cur + step;
                *rot = if t < v { t } else { v };
            } else {
                let ceil = d(-0.1f32) * d(dt);
                if ceil < d(step) {
                    step = ceil as f32;
                }
                let v = cur + step;
                *rot = if v < t { t } else { v };
            }
        }
        if pos != 0.0 {
            if pos < 0.5 {
                let m = (1.0 - (d(pos) + d(pos))) as f32;
                for (o, r) in out.iter_mut().zip(self.move_rot) {
                    *o = (d(*o) + d(m) * d(r)) as f32;
                }
            }
        } else {
            for (o, r) in out.iter_mut().zip(self.move_rot) {
                *o += r;
            }
        }
    }

    /// 0x80f11b0: the idle drift. The stance factor eases at 0.5 a second
    /// (not down an overlay's sight), the clock steps by the idle speed's
    /// ms, truncated, and three sines of it at 0.5, 0.7 and 1 mrad a ms
    /// move roll, yaw and pitch by a hundredth of the amount.
    fn idle(&mut self, inp: &AimInput, k: &AimKeys, dt: f32, out: &mut [f32; 3]) {
        let pos = inp.pos();
        let (mut amount, speed) = if !k.ads {
            if k.hip_idle_amount != 0.0 {
                (k.hip_idle_amount, k.hip_idle_speed)
            } else {
                (80.0, 1.0)
            }
        } else {
            (
                ((d(k.ads_idle_amount) - d(k.hip_idle_amount)) * d(pos) + d(k.hip_idle_amount))
                    as f32,
                ((d(k.ads_idle_speed) - d(k.hip_idle_speed)) * d(pos) + d(k.hip_idle_speed)) as f32,
            )
        };
        let target = match stance(inp.ps) {
            2 => k.idle_prone,
            1 => k.idle_crouch,
            _ => 1.0,
        };
        if (!k.overlay || pos == 0.0) && target != self.idle_factor {
            if self.idle_factor < target {
                let v = (d(self.idle_factor) + d(0.5f32) * d(dt)) as f32;
                self.idle_factor = if target < v { target } else { v };
            } else {
                let v = (d(self.idle_factor) - d(0.5f32) * d(dt)) as f32;
                self.idle_factor = if v < target { target } else { v };
            }
        }
        amount = (d(amount) * d(self.idle_factor)) as f32;
        if k.overlay {
            amount = (d(amount) * (1.0 - d(pos))) as f32;
        }
        self.idle_time = self
            .idle_time
            .wrapping_add((d(dt) * 1000.0 * d(speed)) as i32);
        let t = f64::from(self.idle_time);
        let wave = |rate: f32| (t * d(rate)).sin() as f32;
        let s = wave(0.0005);
        out[2] = (d(out[2]) + d(amount) * d(s) * d(0.01f32)) as f32;
        let s = wave(0.0007);
        out[1] = (d(out[1]) + d(amount) * d(s) * d(0.01f32)) as f32;
        let s = wave(0.001);
        out[0] = (d(out[0]) + d(amount) * d(s) * d(0.01f32)) as f32;
    }

    /// 0x80f166c: the damage kick on the gun, up over `100 f` ms and down
    /// over `400 f`, `f` from 0.5 at the hip to 1 sighted.
    fn damage_kick(&self, inp: &AimInput, k: &AimKeys, out: &mut [f32; 3]) {
        if self.kick_time == 0 {
            return;
        }
        let pos = inp.pos();
        let mut f = (d(0.5f32) + d(0.5f32) * d(pos)) as f32;
        let rise = (d(100.0f32) * d(f)) as f32;
        let fall = (d(400.0f32) * d(f)) as f32;
        if pos != 0.0 && k.overlay {
            f = (d(f) * (1.0 - d(0.75f32) * d(pos))) as f32;
        }
        let t = (inp.level_time - self.kick_time) as f32;
        let g = if t < rise {
            (lean_fraction((d(t) / d(rise)) as f32) * d(f)) as f32
        } else {
            let r = (1.0 - (d(t) - d(rise)) / d(fall)) as f32;
            if r <= 0.0 {
                return;
            }
            ((1.0 - lean_fraction(1.0 - r)) * d(f)) as f32
        };
        out[0] = (d(out[0]) + d(0.5f32) * d(g) * d(self.kick_pitch)) as f32;
        out[1] = (d(out[1]) - d(g) * d(self.kick_roll)) as f32;
        out[2] = (d(out[2]) + d(0.5f32) * d(g) * d(self.kick_roll)) as f32;
    }

    /// 0x80f1a6a: the gun spring, pitch then yaw, in 5 ms slices of the
    /// cmd until both are zeroed.
    fn spring_step(&mut self, k: &AimKeys, pos: f32, dt: f32) {
        let blend = |i: usize| {
            ((d(k.ads_spring[i]) - d(k.hip_spring[i])) * d(pos) + d(k.hip_spring[i])) as f32
        };
        let (accel, speed_max, speed_decay, static_decay) =
            (blend(0), blend(1), blend(2), blend(3));
        let mut left = dt;
        loop {
            if left <= 0.0 {
                break;
            }
            let slice;
            if d(left) > 0.005 {
                slice = 0.005f32;
                left = (d(left) - 0.005) as f32;
            } else {
                slice = left;
                left = 0.0;
            }
            let p = spring_axis(
                &mut self.spring[0],
                &mut self.spring_speed[0],
                slice,
                k.gun_max[0],
                [accel, speed_max, speed_decay, static_decay],
            );
            let y = spring_axis(
                &mut self.spring[1],
                &mut self.spring_speed[1],
                slice,
                k.gun_max[1],
                [accel, speed_max, speed_decay, static_decay],
            );
            if p && y {
                break;
            }
        }
    }

    /// `BG_WeaponFireRecoil` (0x80f095a) for a fire event: view pitch, view
    /// yaw, gun pitch, gun yaw, each `min + G_random() * (max - min)`; the
    /// view kick is dropped, the gun's adds to the spring's speed. The view
    /// kick takes the sight's ranges only at a frac of 1, the gun's at any
    /// frac above 0.
    pub fn fire_recoil(
        &mut self,
        ws: &WeaponState,
        ps: &PlayerState,
        defs: &dyn Weapons,
        rand: &mut dyn FnMut() -> f64,
    ) {
        let k = defs
            .get(super::weapon::view_weapon(ps, ws, defs))
            .map(|w| w.aim.clone())
            .unwrap_or_default();
        let pos = ws.pos_frac;
        let mut scale = 1.0f32;
        if ws.restrict_kick_time > 0 {
            let pct = if pos == 1.0 {
                k.ads_reduced_percent
            } else {
                k.hip_reduced_percent
            };
            scale = (d(0.01f32) * d(pct)) as f32;
        }
        let mut range = |r: [f32; 4]| {
            let a = (d(r[0]) + (d(r[1]) - d(r[0])) * rand()) as f32;
            let b = (d(r[2]) + (d(r[3]) - d(r[2])) * rand()) as f32;
            (a, b)
        };
        let _view = range(if pos == 1.0 {
            k.ads_view_kick
        } else {
            k.hip_view_kick
        });
        let (gp, gy) = range(if pos > 0.0 {
            k.ads_gun_kick
        } else {
            k.hip_gun_kick
        });
        self.spring_speed[0] = (d(self.spring_speed[0]) + d(gp) * d(scale)) as f32;
        self.spring_speed[1] = (d(self.spring_speed[1]) + d(gy) * d(scale)) as f32;
    }

    /// The pitch and yaw the cmd's shots leave along (`gclient+0x27d8`):
    /// the view plus its offsets, multiplied through the weapon's angles on
    /// a sighted aim-down-sight gun without an overlay. Steps the state.
    pub fn fire_angles(&mut self, inp: &AimInput) -> [f32; 2] {
        let xyspeed = get_speed(inp.ps, inp.level_time);
        let off = self.view_offsets(inp, xyspeed);
        let view = inp.ps.view_angles;
        let sum = [view[0] + off[0], view[1] + off[1], view[2] + off[2]];
        self.sway(inp);
        let w = self.weapon_angles(inp, xyspeed);
        let held = inp.keys(inp.ws.weapon);
        if held.ads && inp.pos() != 0.0 && !held.overlay {
            let a = angles_to_axis(w);
            let b = angles_to_axis(sum);
            let m = matrix_multiply(a, b);
            return axis_to_angles(m);
        }
        [sum[0], sum[1]]
    }
}

/// `BG_CalculateWeaponPosition_GunRecoil_SingleAngle` (0x80f1862): one
/// slice of one axis. True when the axis was at rest and is zeroed.
fn spring_axis(angle: &mut f32, speed: &mut f32, dt: f32, max: f32, k: [f32; 4]) -> bool {
    let [accel, speed_max, speed_decay, static_decay] = k;
    if !(d(angle.abs()) >= 0.25 || d(speed.abs()) >= 1.0) {
        *angle = 0.0;
        *speed = 0.0;
        return true;
    }
    *angle = (d(*angle) + d(*speed) * d(dt)) as f32;
    if max < *angle {
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
        *speed = (d(*speed) - d(accel) * d(dt)) as f32;
    } else if *angle < 0.0 {
        *speed = (d(*speed) + d(accel) * d(dt)) as f32;
    }
    *speed = (d(*speed) - d(*speed) * d(speed_decay) * d(dt)) as f32;
    if *speed > 0.0 {
        *speed = (d(*speed) - d(static_decay) * d(dt)) as f32;
        if *speed < 0.0 {
            *speed = 0.0;
        }
    } else {
        *speed = (d(*speed) + d(static_decay) * d(dt)) as f32;
        if *speed > 0.0 {
            *speed = 0.0;
        }
    }
    if speed_max < *speed {
        *speed = speed_max;
    } else if *speed < -speed_max {
        *speed = -speed_max;
    }
    false
}

/// 0x80f14aa: the walk bob on the gun, a quarter turn and `2 pi` past the
/// cycle, at 0.16 of the speed; its roll a 0.4712 turn back at 1.5 times
/// that and never up; toward `adsBobFactor` by the sight, gone down an
/// overlay's.
fn walk_bob(ps: &PlayerState, k: &AimKeys, pos: f32, xyspeed: f32, out: &mut [f32; 3]) {
    let t = turn(ps);
    let c = (t + t + std::f64::consts::TAU) as f32;
    let mut c = (d(c) + std::f64::consts::FRAC_PI_4 + std::f64::consts::TAU) as f32;
    let mut speed = (d(xyspeed) * d(0.16f32)) as f32;
    let mut p = -vertical_bob(ps, c, speed, 10.0);
    let mut y = -(horizontal_bob(ps, c, speed, 10.0) as f32);
    c = (d(c) - 0.471_238_916_763_820_4) as f32;
    speed = (d(speed) * d(1.5f32)) as f32;
    let r = horizontal_bob(ps, c, speed, 10.0) as f32;
    let mut r = if 0.0 - r >= 0.0 { r } else { 0.0 };
    if pos != 0.0 {
        let m = (1.0 - (1.0 - d(k.ads_bob_factor)) * d(pos)) as f32;
        p = (d(p) * d(m)) as f32;
        y = (d(y) * d(m)) as f32;
        r = (d(r) * d(m)) as f32;
    }
    if k.overlay {
        let m = (1.0 - d(pos)) as f32;
        p = (d(m) * d(p)) as f32;
        y = (d(m) * d(y)) as f32;
        r = (d(m) * d(r)) as f32;
    }
    out[0] += p;
    out[1] += y;
    out[2] += r;
}

/// `AnglesToAxis` (0x80a94aa): forward, left, up, the left
/// `vec3_origin - right`.
fn angles_to_axis(a: [f32; 3]) -> [Vec3; 3] {
    vcod_common::x87::angles_to_axis(Vec3::from_array(a))
}

/// `MatrixMultiply` (0x80a58f2): `a * b`, each element's sum on the stack
/// from the last product, stored.
fn matrix_multiply(a: [Vec3; 3], b: [Vec3; 3]) -> [Vec3; 3] {
    let el = |i: usize, j: usize| {
        (d(a[i][2]) * d(b[2][j]) + d(a[i][1]) * d(b[1][j]) + d(a[i][0]) * d(b[0][j])) as f32
    };
    std::array::from_fn(|i| Vec3::new(el(i, 0), el(i, 1), el(i, 2)))
}

/// `AxisToAngles` (0x80a9558)'s pitch and yaw: `vectoangles` (0x80a4fe2) of
/// the forward. The roll it goes on to work out no shot reads.
fn axis_to_angles(m: [Vec3; 3]) -> [f32; 2] {
    let v = m[0];
    if v.y == 0.0 && v.x == 0.0 {
        return [if v.z > 0.0 { 270.0 } else { 90.0 }, 0.0];
    }
    let mut yaw = (180.0 * d(v.y).atan2(d(v.x)) / std::f64::consts::PI) as f32;
    if yaw < 0.0 {
        yaw += 360.0;
    }
    let flat = sqrt(d(v.y) * d(v.y) + d(v.x) * d(v.x));
    let mut pitch = (-180.0 * d(v.z).atan2(d(flat)) / std::f64::consts::PI) as f32;
    if pitch < 0.0 {
        pitch += 360.0;
    }
    [pitch, yaw]
}

/// `G_random` (lnxded 0x80a3774): glibc `rand()` over 2^31, on the stack.
pub fn g_random(r: &mut GlibcRand) -> f64 {
    f64::from(r.next_u31()) / 2_147_483_648.0
}

/// `G_GetPlayerViewOrigin` (0x80f8916) short of the lean: the eye
/// `viewHeightCurrent` over the origin, the vertical bob added to z and the
/// horizontal along the view's right, at most `bg_bobMax` each.
pub fn bobbed_eye(ps: &PlayerState, level_time: i32) -> Vec3 {
    let mut out = ps.origin;
    out.z += ps.view_height_current;
    let cycle = bob_cycle(ps);
    let speed = get_speed(ps, level_time);
    out.z += vertical_bob(ps, cycle, speed, BOB_MAX);
    let h = horizontal_bob(ps, cycle, speed, BOB_MAX) as f32;
    let right = vcod_common::x87::angle_vectors(Vec3::from_array(ps.view_angles))[1];
    super::fpu::ma(out, h, right)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The spring zeroes an axis that has settled and stops the slices.
    #[test]
    fn a_settled_spring_zeroes() {
        let (mut a, mut s) = (0.2f32, 0.5f32);
        assert!(spring_axis(
            &mut a,
            &mut s,
            0.005,
            10.0,
            [1000.0, 100.0, 16.0, 20.0]
        ));
        assert_eq!((a, s), (0.0, 0.0));
        let (mut a, mut s) = (0.0f32, 50.0f32);
        assert!(!spring_axis(
            &mut a,
            &mut s,
            0.005,
            10.0,
            [1000.0, 100.0, 16.0, 20.0]
        ));
        assert_eq!(a, 0.25);
    }

    /// A still body's view takes no bob and no offset: the shot is the
    /// view's at the hip.
    #[test]
    fn a_still_hip_shot_leaves_along_the_view() {
        let ps = PlayerState {
            view_angles: [5.0, 93.0, 0.0],
            ..PlayerState::default()
        };
        let ws = WeaponState::default();
        let defs: Vec<Option<super::super::weapon::WeaponParams>> = vec![None];
        let mut st = AimState::default();
        let a = st.fire_angles(&AimInput {
            ps: &ps,
            ws: &ws,
            defs: &defs,
            level_time: 1000,
            msec: 16,
        });
        assert_eq!(a, [5.0, 93.0]);
    }
}
