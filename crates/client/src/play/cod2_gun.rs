//! Where a CoD2 view weapon sits off the view: `CG_AddViewWeapon` (Mac
//! 0x1da2f8) with `BG_CalculateWeaponPosition_Sway` (0x7b5fe, the turn lag)
//! and `BG_CalculateWeaponAngles` (0x7f226: lean roll, `adsAimPitch`, the
//! movement tilt, the idle drift, the walk bob, the damage kick, the turn
//! lag). docs/research/cod2-hud.md 18 lists the constants. The gun's recoil
//! spring is another module's; its angles come in through
//! [`GunInput::recoil`].

use vcod_cod2::weapon::WeaponDef;

use crate::play::cod2_view::{DamageKick, IdleState};

/// `weaponstate` 5, reloading: no movement offset or tilt.
const WEAPON_RELOADING: i32 = 5;
/// `eFlags` 4 crouched, 8 prone: the tilt's stance.
const EF_CROUCH: i32 = 0x4;
const EF_PRONE: i32 = 0x8;
/// `viewHeightTarget` of a crouched and a prone body: the offset's stance.
const VIEW_CROUCH: i32 = 40;
const VIEW_PRONE: i32 = 11;
/// `bg_bobAmplitudeStanding`, `Ducked`, `Prone` (cod2-movement.md 13).
const BOB_AMPLITUDE: [f32; 3] = [0.007, 0.0075, 0.03];

/// The weapon file's keys the gun's pose reads.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct GunKeys {
    pub ads: bool,
    /// `adsOverlayReticle` not `none`.
    pub reticle: bool,
    pub ads_aim_pitch: f32,
    pub ads_bob_factor: f32,
    /// `stand`, `ducked`, `prone` `Move{F,R,U}`, `Rot{P,Y,R}`, `Ofs{F,R,U}`
    /// (stand has none) and their `*MoveMinSpeed`, `*RotMinSpeed`.
    pub moves: [[f32; 3]; 3],
    pub rots: [[f32; 3]; 3],
    pub ofs: [[f32; 3]; 2],
    pub move_min: [f32; 3],
    pub rot_min: [f32; 3],
    pub pos_move_rate: f32,
    pub pos_prone_move_rate: f32,
    pub pos_rot_rate: f32,
    pub pos_prone_rot_rate: f32,
    pub hip_idle_amount: f32,
    pub ads_idle_amount: f32,
    pub hip_idle_speed: f32,
    pub ads_idle_speed: f32,
    pub idle_crouch: f32,
    pub idle_prone: f32,
    /// `sway{MaxAngle,LerpSpeed,PitchScale,YawScale,HorizScale,VertScale}`
    /// and the `adsSway*` six.
    pub sway: [f32; 6],
    pub ads_sway: [f32; 6],
    pub sway_shellshock_scale: f32,
}

impl GunKeys {
    pub fn of(d: &WeaponDef) -> GunKeys {
        let v3 =
            |p: &str, a: &str, b: &str, c: &str| [a, b, c].map(|s| d.float(&format!("{p}{s}")));
        let six = |p: &str| {
            [
                "MaxAngle",
                "LerpSpeed",
                "PitchScale",
                "YawScale",
                "HorizScale",
                "VertScale",
            ]
            .map(|s| d.float(&format!("{p}{s}")))
        };
        let stances = ["stand", "ducked", "prone"];
        GunKeys {
            ads: d.flag("aimDownSight"),
            reticle: d.int("adsOverlayReticle") != 0,
            ads_aim_pitch: d.float("adsAimPitch"),
            ads_bob_factor: d.float("adsBobFactor"),
            moves: stances.map(|s| v3(s, "MoveF", "MoveR", "MoveU")),
            rots: stances.map(|s| v3(s, "RotP", "RotY", "RotR")),
            ofs: ["ducked", "prone"].map(|s| v3(s, "OfsF", "OfsR", "OfsU")),
            move_min: stances.map(|s| d.float(&format!("{s}MoveMinSpeed"))),
            rot_min: stances.map(|s| d.float(&format!("{s}RotMinSpeed"))),
            pos_move_rate: d.float("posMoveRate"),
            pos_prone_move_rate: d.float("posProneMoveRate"),
            pos_rot_rate: d.float("posRotRate"),
            pos_prone_rot_rate: d.float("posProneRotRate"),
            hip_idle_amount: d.float("hipIdleAmount"),
            ads_idle_amount: d.float("adsIdleAmount"),
            hip_idle_speed: d.float("hipIdleSpeed"),
            ads_idle_speed: d.float("adsIdleSpeed"),
            idle_crouch: d.float("idleCrouchFactor"),
            idle_prone: d.float("idleProneFactor"),
            sway: six("sway"),
            ads_sway: six("adsSway"),
            sway_shellshock_scale: d.float("swayShellShockScale"),
        }
    }
}

/// The playerstate fields the pose reads.
#[derive(Clone, Copy, Debug, Default)]
pub struct GunPs {
    pub view: [f32; 3],
    pub pos_frac: f32,
    pub leanf: f32,
    pub eflags: i32,
    pub weaponstate: i32,
    pub view_height_target: i32,
    pub view_height_current: f32,
    /// `ps.speed`, the full run speed.
    pub speed: f32,
    /// `BG_GetSpeed`: the flat speed.
    pub xyspeed: f32,
    pub bob_cycle: u8,
}

/// What the frame hands the pose beyond the playerstate.
#[derive(Clone, Copy, Debug)]
pub struct GunInput<'a> {
    pub kick: &'a DamageKick,
    /// cg.time and the frame's ms.
    pub now: i32,
    pub msec: i32,
    /// The shellshock's ms left (`CG_AddViewWeapon` scales the sway by
    /// `swayShellShockScale` over the kick's last 3 s), 0 with none.
    pub shock_left_ms: i32,
    /// The recoil spring's pitch, yaw and roll, added after the kick
    /// (`BG_CalculateWeaponAngles`' `+0x2c` angles on an ADS weapon).
    pub recoil: [f32; 3],
}

/// The gun's place in the view: `offset` along (forward, left, up), the
/// model's `angles` (pitch, yaw, roll) in the view's frame.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct GunPose {
    pub offset: [f32; 3],
    pub angles: [f32; 3],
}

/// `DiffTrack` (Mac 0x2c220): `cur` toward `target` by `rate * dt` of the
/// gap, the target outright once within 0.001 or past it.
pub(crate) fn diff_track(target: f32, cur: f32, rate: f32, dt: f32) -> f32 {
    let gap = target - cur;
    let step = gap * rate * dt;
    if gap.abs() > 0.001 && step.abs() <= gap.abs() {
        cur + step
    } else {
        target
    }
}

/// `DiffTrackAngle` (Mac 0x2ce40): [`diff_track`] the short way round,
/// then truncated to 65536ths into -180..180.
fn diff_track_angle(mut target: f32, cur: f32, rate: f32, dt: f32) -> f32 {
    while target - cur > 180.0 {
        target -= 360.0;
    }
    while target - cur < -180.0 {
        target += 360.0;
    }
    let gap = target - cur;
    let step = gap * rate * dt;
    let v = if gap.abs() > 0.001 && step.abs() <= gap.abs() {
        cur + step
    } else {
        target
    };
    let a = ((v * 182.044_45) as i32 & 0xffff) as f32 * 0.005_493_164;
    if a > 180.0 { a - 360.0 } else { a }
}

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

fn lean_fraction(f: f32) -> f32 {
    (2.0 - f.abs()) * f
}

/// The movement offset's and tilt's ease: `rate * dt` of the gap, at least
/// 0.1 a second, never past the target.
fn ease(cur: &mut f32, target: f32, rate: f32, dt: f32) {
    if *cur == target {
        return;
    }
    let step = (target - *cur) * dt * rate;
    if target <= *cur {
        *cur = (*cur + step.min(dt * -0.1)).max(target);
    } else {
        *cur = (*cur + step.max(dt * 0.1)).min(target);
    }
}

/// Fades a movement term out over the first half of the sight's rise.
fn sight_fade(pos: f32) -> f32 {
    if pos == 0.0 {
        1.0
    } else if pos >= 0.5 {
        0.0
    } else {
        pos * -2.0 + 1.0
    }
}

/// The view gun's state across frames: `cg + 0x285d4` (the last view),
/// `0x285ec` (the sway's offset), `0x285e0` (its angles), `0x284a8` (the
/// movement offset) and `0x284b4` (the movement tilt).
#[derive(Clone, Copy, Debug, Default)]
pub struct GunState {
    last_view: [f32; 3],
    sway_offset: [f32; 3],
    sway_angles: [f32; 3],
    move_pos: [f32; 3],
    rot: [f32; 3],
}

impl GunState {
    /// `BG_CalculateWeaponPosition_Sway`: the view's turn since the last
    /// frame, per 60th of a second and clamped to the max angle, is the
    /// target the offset and the angle lag ease toward. A scope's sight
    /// freezes it, the last view included.
    fn sway(&mut self, ps: &GunPs, k: &GunKeys, scale: f32, msec: i32) {
        if msec == 0 {
            return;
        }
        let pos = ps.pos_frac;
        let s = if !k.ads {
            k.sway
        } else {
            if pos > 0.0 && k.reticle {
                return;
            }
            std::array::from_fn(|i| (k.ads_sway[i] - k.sway[i]) * pos + k.sway[i])
        };
        let [max, rate, pitch_s, yaw_s, horiz_s, vert_s] = s;
        let dt = msec as f32 * 0.001;
        let per = 1.0 / (dt * 60.0);
        let dp = (angle_subtract(ps.view[0], self.last_view[0]) * per).clamp(-max, max);
        let dy = (angle_subtract(ps.view[1], self.last_view[1]) * per).clamp(-max, max);
        self.sway_offset[1] = diff_track(dy * horiz_s * scale, self.sway_offset[1], rate, dt);
        self.sway_offset[2] = diff_track(vert_s * scale * dp, self.sway_offset[2], rate, dt);
        self.sway_angles[0] = diff_track_angle(pitch_s * scale * dp, self.sway_angles[0], rate, dt);
        self.sway_angles[1] = diff_track_angle(yaw_s * scale * dy, self.sway_angles[1], rate, dt);
        self.last_view = ps.view;
    }

    /// `CG_AddViewWeapon`'s movement offset: the stance's `*Move` scaled
    /// past its `*MoveMinSpeed` (none while reloading), plus `duckedOfs` or
    /// `proneOfs` at those heights, eased at `posMoveRate`
    /// (`posProneMoveRate` at the prone height), faded by the sight.
    fn move_offset(&mut self, ps: &GunPs, k: &GunKeys, dt: f32) -> [f32; 3] {
        let stance = if ps.eflags & EF_PRONE != 0 {
            2
        } else if ps.eflags & EF_CROUCH != 0 {
            1
        } else {
            0
        };
        let min = k.move_min[stance];
        let mut target = [0.0f32; 3];
        if ps.xyspeed > min && ps.weaponstate != WEAPON_RELOADING {
            let f = ((ps.xyspeed - min) / (ps.speed - min)).clamp(0.0, 1.0);
            target = k.moves[stance].map(|m| f * m);
        }
        let ofs = match ps.view_height_target {
            VIEW_CROUCH => Some(k.ofs[0]),
            VIEW_PRONE => Some(k.ofs[1]),
            _ => None,
        };
        if let Some(o) = ofs {
            for (t, o) in target.iter_mut().zip(o) {
                *t += o;
            }
        }
        let rate = if ps.view_height_current == VIEW_PRONE as f32 {
            k.pos_prone_move_rate
        } else {
            k.pos_move_rate
        };
        for (cur, t) in self.move_pos.iter_mut().zip(target) {
            ease(cur, t, rate, dt);
        }
        let f = sight_fade(ps.pos_frac);
        self.move_pos.map(|p| p * f)
    }

    /// One frame of the gun's pose; `idle` is the clock and stance factor
    /// the view's sway shares.
    pub fn step(
        &mut self,
        ps: &GunPs,
        k: &GunKeys,
        idle: &mut IdleState,
        input: &GunInput,
    ) -> GunPose {
        let pos = ps.pos_frac;
        let dt = input.msec as f32 * 0.001;
        let scale = crate::play::shock::sway_scale(k.sway_shellshock_scale, input.shock_left_ms);
        self.sway(ps, k, scale, input.msec);

        // The offset: movement, the lean along the rolled right, the sway.
        let mut off = self.move_offset(ps, k, dt);
        if ps.leanf != 0.0 && pos < 1.0 {
            let f = lean_fraction(ps.leanf);
            let d = (1.0 - pos) * f * 1.6;
            // `AngleVectors((0, 0, -2f))`'s right, (0, -cos, -sin), added
            // straight onto the (forward, left, up) offset.
            let (sr, cr) = (f * -2.0).to_radians().sin_cos();
            off[1] -= cr * d;
            off[2] -= sr * d;
        }
        off[1] -= self.sway_offset[1];
        off[2] += self.sway_offset[2];

        let mut a = [0.0f32; 3];
        if ps.leanf != 0.0 {
            a[2] += lean_fraction(ps.leanf) * -2.0;
        }
        if k.ads {
            a[0] += pos * k.ads_aim_pitch;
        }
        self.tilt(ps, k, dt, &mut a);
        idle_drift(ps, k, idle, dt, &mut a);
        walk_bob(ps, k, &mut a);
        damage_kick(ps, k, input, &mut a);
        if k.ads {
            for (o, r) in a.iter_mut().zip(input.recoil) {
                *o += r;
            }
        }
        a[0] = angle_subtract(a[0], self.sway_angles[0]);
        a[1] = angle_subtract(a[1], self.sway_angles[1]);
        GunPose {
            offset: off,
            angles: a,
        }
    }

    /// The movement tilt: the stance's `*Rot` scaled past its
    /// `*RotMinSpeed`, eased at `posRotRate` (`posProneRotRate` at the prone
    /// height), faded by the sight.
    fn tilt(&mut self, ps: &GunPs, k: &GunKeys, dt: f32, out: &mut [f32; 3]) {
        let stance = if ps.eflags & EF_PRONE != 0 {
            2
        } else if ps.eflags & EF_CROUCH != 0 {
            1
        } else {
            0
        };
        let min = k.rot_min[stance];
        let mut target = [0.0f32; 3];
        if ps.xyspeed > min && ps.weaponstate != WEAPON_RELOADING {
            let f = ((ps.xyspeed - min) / (ps.speed - min)).clamp(0.0, 1.0);
            target = k.rots[stance].map(|r| f * r);
        }
        if ps.pos_frac != 0.0 {
            target = target.map(|t| (1.0 - ps.pos_frac) * t);
        }
        let rate = if ps.view_height_current == VIEW_PRONE as f32 {
            k.pos_prone_rot_rate
        } else {
            k.pos_rot_rate
        };
        for (cur, t) in self.rot.iter_mut().zip(target) {
            ease(cur, t, rate, dt);
        }
        let f = sight_fade(ps.pos_frac);
        for (o, r) in out.iter_mut().zip(self.rot) {
            *o += f * r;
        }
    }
}

fn bob_amplitude(ps: &GunPs) -> f32 {
    match ps.view_height_target {
        VIEW_PRONE => BOB_AMPLITUDE[2],
        VIEW_CROUCH => BOB_AMPLITUDE[1],
        _ => BOB_AMPLITUDE[0],
    }
}

/// The idle drift: `hipIdleAmount` (80 at speed 1 when the file has none)
/// or, on an ADS weapon, it blended to `adsIdleAmount` by the sight, times
/// the stance factor eased at 0.5 a second, gone down a scope; three sines
/// of the idle clock, which runs at the idle speed.
fn idle_drift(ps: &GunPs, k: &GunKeys, idle: &mut IdleState, dt: f32, out: &mut [f32; 3]) {
    let pos = ps.pos_frac;
    let (mut amount, speed) = if k.ads {
        (
            (k.ads_idle_amount - k.hip_idle_amount) * pos + k.hip_idle_amount,
            (k.ads_idle_speed - k.hip_idle_speed) * pos + k.hip_idle_speed,
        )
    } else if k.hip_idle_amount != 0.0 {
        (k.hip_idle_amount, k.hip_idle_speed)
    } else {
        (80.0, 1.0)
    };
    let target = if ps.eflags & EF_PRONE != 0 {
        k.idle_prone
    } else if ps.eflags & EF_CROUCH != 0 {
        k.idle_crouch
    } else {
        1.0
    };
    if (!k.reticle || pos == 0.0) && target != idle.factor {
        idle.factor = if idle.factor < target {
            (idle.factor + dt * 0.5).min(target)
        } else {
            (idle.factor - dt * 0.5).max(target)
        };
    }
    amount *= idle.factor;
    if k.reticle {
        amount *= 1.0 - pos;
    }
    idle.time = idle.time.wrapping_add((speed * dt * 1000.0) as i32);
    let t = idle.time as f32;
    out[2] += (t * 0.0005).sin() * amount * 0.01;
    out[1] += (t * 0.0007).sin() * amount * 0.01;
    out[0] += 0.01 * amount * (t * 0.001).sin();
}

/// The walk bob on the gun: the view's scope bob unconditionally, toward
/// `adsBobFactor` by the sight, and gone down a scope.
fn walk_bob(ps: &GunPs, k: &GunKeys, out: &mut [f32; 3]) {
    let turn = f64::from(f32::from(ps.bob_cycle) / 255.0) * std::f64::consts::PI;
    let c = f64::from((turn + turn + std::f64::consts::TAU) as f32);
    let c = (c + std::f64::consts::FRAC_PI_4 + std::f64::consts::TAU) as f32;
    let base = 0.16 * ps.xyspeed;
    let amp = bob_amplitude(ps);
    let a = (base * amp).min(10.0);
    let mut p = -(((c + c).sin()
        + ((f64::from(c * 4.0) + std::f64::consts::FRAC_PI_2).sin() as f32) * 0.2)
        * 0.75
        * a);
    let mut y = -(a * c.sin());
    let a2 = (base * 1.5 * amp).min(10.0);
    let r = ((f64::from(c) - 0.471_238_916_763_820_4) as f32).sin() * a2;
    let mut r = if r <= 0.0 { r } else { 0.0 };
    let pos = ps.pos_frac;
    if pos != 0.0 {
        let m = 1.0 - (1.0 - k.ads_bob_factor) * pos;
        p *= m;
        y *= m;
        r *= m;
    }
    if k.reticle {
        let m = 1.0 - pos;
        p *= m;
        y *= m;
        r *= m;
    }
    out[0] += p;
    out[1] += y;
    out[2] += r;
}

/// The damage kick on the gun: up over `100 * f` ms and down over `400 *
/// f`, `f` from 0.5 at the hip to 1 sighted, less on a scope.
fn damage_kick(ps: &GunPs, k: &GunKeys, input: &GunInput, out: &mut [f32; 3]) {
    let kick = input.kick;
    if kick.time == 0 {
        return;
    }
    let pos = ps.pos_frac;
    let f = pos * 0.5 + 0.5;
    let (rise, fall) = (f * 100.0, f * 400.0);
    let mut amp = f;
    if pos != 0.0 && k.reticle {
        amp *= pos * -0.75 + 1.0;
    }
    let dt = (input.now - kick.time) as f32;
    let g = if dt < rise {
        lean_fraction(dt / rise)
    } else {
        let r = 1.0 - (dt - rise) / fall;
        if r <= 0.0 {
            return;
        }
        1.0 - lean_fraction(1.0 - r)
    };
    let g = g * amp;
    out[0] += g * kick.pitch * 0.5;
    out[1] -= g * kick.roll;
    out[2] += g * kick.roll * 0.5;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rifle() -> GunKeys {
        GunKeys {
            ads: true,
            sway: [30.0, 6.0, 0.1, 0.1, 0.1, 0.1],
            ads_sway: [15.0, 3.0, 0.1, 0.1, 0.1, 0.1],
            hip_idle_amount: 30.0,
            ads_idle_amount: 20.0,
            hip_idle_speed: 1.0,
            ads_idle_speed: 1.5,
            idle_crouch: 1.0,
            idle_prone: 1.0,
            moves: [[-1.0, -2.0, -0.5]; 3],
            rots: [[2.0, 3.0, 4.0]; 3],
            pos_move_rate: 6.0,
            pos_rot_rate: 6.0,
            move_min: [0.0; 3],
            rot_min: [0.0; 3],
            sway_shellshock_scale: 5.0,
            ..GunKeys::default()
        }
    }

    fn run(ps: &GunPs, k: &GunKeys, frames: i32, st: &mut GunState) -> GunPose {
        let kick = DamageKick::default();
        let mut idle = IdleState::default();
        let mut pose = GunPose::default();
        for i in 0..frames {
            let input = GunInput {
                kick: &kick,
                now: i * 16,
                msec: 16,
                shock_left_ms: 0,
                recoil: [0.0; 3],
            };
            pose = st.step(ps, k, &mut idle, &input);
        }
        pose
    }

    /// `DiffTrack` closes `rate * dt` of the gap, snapping inside 0.001;
    /// `DiffTrackAngle` goes the short way and lands on 65536ths.
    #[test]
    fn diff_track_eases_and_snaps() {
        assert!((diff_track(10.0, 0.0, 6.0, 0.016) - 0.96).abs() < 1e-5);
        assert_eq!(diff_track(1.0005, 1.0, 6.0, 0.016), 1.0005);
        // Past the gap in one step: the target.
        assert_eq!(diff_track(10.0, 0.0, 100.0, 1.0), 10.0);
        let a = diff_track_angle(-170.0, 170.0, 6.0, 0.016);
        // 20 degrees the short way, 0.096 of it.
        assert!(a > 171.9 && a < 172.0, "{a}");
        let units = a / (360.0 / 65536.0);
        assert!((units - units.round()).abs() < 1e-2);
    }

    /// A turn swings the gun's angles and offset against it, eased back
    /// once the view holds still; a scope's sight freezes the sway.
    #[test]
    fn a_turn_lags_the_gun() {
        let k = rifle();
        let mut st = GunState::default();
        let still = GunPs {
            speed: 190.0,
            ..GunPs::default()
        };
        run(&still, &k, 2, &mut st);
        // One 16 ms frame turning 2 degrees of yaw: 2.08 a 60th, under 30.
        let turned = GunPs {
            view: [0.0, 2.0, 0.0],
            ..still
        };
        let kick = DamageKick::default();
        let mut idle = IdleState::default();
        let input = GunInput {
            kick: &kick,
            now: 0,
            msec: 16,
            shock_left_ms: 0,
            recoil: [0.0; 3],
        };
        st.step(&turned, &k, &mut idle, &input);
        // Target 2.08 * 0.1 eased 6 * 0.016 of the way: 0.02, the angle
        // truncated to three 65536ths.
        assert!(
            (st.sway_offset[1] - 0.02).abs() < 1e-3,
            "{:?}",
            st.sway_offset
        );
        assert_eq!(st.sway_angles[1], 3.0 * 360.0 / 65536.0);
        // Held still, it eases back.
        let back = run(&turned, &k, 200, &mut st);
        assert!(st.sway_angles[1].abs() < 0.01, "{back:?}");
        let mut scoped = GunState::default();
        let k = GunKeys { reticle: true, ..k };
        let sighted = GunPs {
            pos_frac: 1.0,
            view: [0.0, 30.0, 0.0],
            ..still
        };
        run(&sighted, &k, 5, &mut scoped);
        assert_eq!(scoped.sway_angles, [0.0; 3]);
    }

    /// Running moves and tilts the gun toward the stance's keys, faded out
    /// by half the sight.
    #[test]
    fn running_moves_the_gun() {
        let k = GunKeys {
            hip_idle_amount: 0.0,
            ..rifle()
        };
        let mut st = GunState::default();
        let ps = GunPs {
            speed: 190.0,
            xyspeed: 190.0,
            ..GunPs::default()
        };
        run(&ps, &k, 300, &mut st);
        assert_eq!(st.move_pos, [-1.0, -2.0, -0.5]);
        assert_eq!(st.rot, [2.0, 3.0, 4.0]);
        let half = GunPs {
            pos_frac: 0.25,
            ..ps
        };
        assert_eq!(st.move_offset(&half, &k, 0.0), [-0.5, -1.0, -0.25]);
        let sighted = GunPs {
            pos_frac: 0.5,
            ..ps
        };
        assert_eq!(st.move_offset(&sighted, &k, 0.0), [0.0; 3]);
    }

    /// The idle drift's clock runs at the idle speed; a scope's sight
    /// takes the drift off the gun.
    #[test]
    fn the_idle_drift_runs_on_its_clock() {
        let k = rifle();
        let ps = GunPs::default();
        let mut idle = IdleState::default();
        let mut out = [0.0; 3];
        idle_drift(&ps, &k, &mut idle, 0.016, &mut out);
        assert_eq!(idle.time, 16);
        // 30 * sin(0.016) * 0.01.
        assert!((out[0] - 0.3 * (0.016f32).sin()).abs() < 1e-6, "{out:?}");
        let scoped = GunKeys { reticle: true, ..k };
        let sighted = GunPs {
            pos_frac: 1.0,
            ..ps
        };
        let mut out = [0.0; 3];
        idle_drift(&sighted, &scoped, &mut idle, 0.016, &mut out);
        assert_eq!(out, [0.0; 3]);
        // At speed 1.5 sighted.
        assert_eq!(idle.time, 16 + 24);
    }

    /// The gun bobs while running and not at rest.
    #[test]
    fn the_gun_bobs_with_speed() {
        let k = rifle();
        let still = GunPs::default();
        let mut out = [0.0; 3];
        walk_bob(&still, &k, &mut out);
        assert_eq!(out, [0.0; 3]);
        let mut peak = 0.0f32;
        for c in 0..=255u8 {
            let run = GunPs {
                xyspeed: 190.0,
                bob_cycle: c,
                ..still
            };
            let mut out = [0.0; 3];
            walk_bob(&run, &k, &mut out);
            peak = peak.max(out[1].abs());
        }
        // 190 * 0.16 * 0.007, under the cap of 10.
        assert!((peak - 0.2128).abs() < 0.01, "{peak}");
    }

    /// The stock rifles' keys read off their files.
    #[test]
    fn stock_weapon_keys_load() {
        let Some(fs) = vcod_common::testing::cod2_fs() else {
            return;
        };
        let k = GunKeys::of(
            &vcod_cod2::weapon::load(&fs, "kar98k_sniper_mp")
                .unwrap()
                .def,
        );
        assert!(k.ads && k.reticle);
        assert_eq!(k.sway, [30.0, 6.0, 0.1, 0.1, 0.1, 0.1]);
        assert_eq!(k.ads_sway[0], 15.0);
        assert_eq!((k.hip_idle_amount, k.ads_idle_speed), (30.0, 1.5));
    }
}
