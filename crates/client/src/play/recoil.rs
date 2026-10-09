//! The kick a shot gives the client's own view and gun
//! (docs/research/cod11-combat.md, 15.7). The view kick rides the usercmd
//! angles, so the server aims later shots where the kicked view points; the
//! gun spring only moves the drawn gun and the scope.

use vcod_common::net::event_ids::{
    EV_FIRE_QUADBARREL_1, EV_FIRE_QUADBARREL_2, EV_FIRE_WEAPON, EV_FIRE_WEAPON_LASTSHOT,
    EV_FIRE_WEAPON_MG42, EV_FIRE_WEAPONB,
};
use vcod_common::pmove::aim::GunKick;
use vcod_common::weapon::WeaponDef;

/// `pm_flags` 0x40000: the playerstate is the client's own view.
pub const PMF_OWN_VIEW: i32 = 0x40000;
/// `pm_flags` 0x40000 or 0x10000 (a follow): the view rides that body, and
/// its shots kick (`0x30038bbf`).
pub const PMF_VIEW_BODY: i32 = 0x50000;

/// `CG_KickAngles`' clamp on each axis, degrees (`0x30069350`).
const KICK_MAX: f32 = 10.0;
/// The centring speed with no weapon held (`0x300693ec`).
const NO_WEAPON_CENTER_SPEED: f32 = 2400.0;
/// A step back toward centre moves at this share of the speed (`0x300693e8`).
const RETURN_SCALE: f32 = 0.06;
/// `CG_KickAngles` steps the frame in slices of at most this many ms.
const STEP_MS: i32 = 5;

/// How many times `event` runs `CG_FireWeapon`, and so the recoil: the
/// quad-barrel pair fires both barrels (docs/research/cod11-sound-system.md,
/// the event table).
pub fn fire_calls(event: i32) -> usize {
    match event {
        EV_FIRE_WEAPON | EV_FIRE_WEAPONB | EV_FIRE_WEAPON_LASTSHOT | EV_FIRE_WEAPON_MG42 => 1,
        EV_FIRE_QUADBARREL_1 | EV_FIRE_QUADBARREL_2 => 2,
        _ => 0,
    }
}

/// MSVC's `rand` (`0x3004b189`), which the cgame's `random()` divides by
/// 32768.
struct CrtRand(u32);

impl CrtRand {
    fn random(&mut self) -> f32 {
        self.0 = self.0.wrapping_mul(214_013).wrapping_add(2_531_011);
        ((self.0 >> 16) & 0x7fff) as f32 / 32768.0
    }
}

/// `cg.kickAngles` and `cg.kickAVel` (`0x3020cb38`, `0x3020cb2c`) and the
/// gun spring (`0x3020cb94`).
pub struct Recoil {
    angles: [f32; 3],
    speed: [f32; 3],
    gun: GunKick,
    rng: CrtRand,
}

impl Default for Recoil {
    fn default() -> Self {
        Recoil::seeded(1)
    }
}

impl Recoil {
    pub fn seeded(seed: u32) -> Recoil {
        Recoil {
            angles: [0.0; 3],
            speed: [0.0; 3],
            gun: GunKick::default(),
            rng: CrtRand(seed),
        }
    }

    /// `CG_WeaponFireRecoil` (`0x30038850`), off `CG_FireWeapon` for the
    /// client's own shot. The view kick's speed is set, not added: pitch up
    /// by the drawn pitch, yaw by the drawn yaw and roll by half of it the
    /// other way, from the sight's ranges only at a full `frac`. The gun's
    /// speed is added to, from the sight's ranges at any `frac` above 0.
    pub fn fire(&mut self, def: &WeaponDef, frac: f32) {
        let a = &def.aim;
        let ads = usize::from(frac == 1.0);
        let pitch = self.draw(a.view_kick_pitch[ads]);
        let yaw = self.draw(a.view_kick_yaw[ads]);
        self.speed = [-pitch, yaw, yaw * -0.5];
        let ads = usize::from(frac > 0.0);
        let gun_pitch = self.draw(a.gun_kick_pitch[ads]);
        let gun_yaw = self.draw(a.gun_kick_yaw[ads]);
        self.gun.kick(gun_pitch, gun_yaw);
    }

    fn draw(&mut self, [min, max]: [f32; 2]) -> f32 {
        self.rng.random() * (max - min) + min
    }

    /// One frame of `frametime_ms`: `CG_KickAngles` (`0x300328b0`) on the
    /// view while `own_view`, else the view kick is zeroed as
    /// `CG_DrawActiveFrame` does (`0x30033d5f`); then the gun spring.
    /// `def` is the held weapon's file, `None` for weapon 0.
    pub fn step(&mut self, frametime_ms: i32, def: Option<&WeaponDef>, frac: f32, own_view: bool) {
        if own_view {
            self.step_view(frametime_ms, def, frac);
        } else {
            self.angles = [0.0; 3];
            self.speed = [0.0; 3];
        }
        if let Some(def) = def {
            self.gun.step(def, frac, frametime_ms as f32 * 0.001);
        }
    }

    fn step_view(&mut self, frametime_ms: i32, def: Option<&WeaponDef>, frac: f32) {
        let center = def.map_or(NO_WEAPON_CENTER_SPEED, |d| {
            d.aim.view_kick_center_speed[usize::from(frac > 0.5)]
        });
        let mut t = frametime_ms;
        while t > 0 {
            let ft = t.min(STEP_MS) as f32 * 0.001;
            for i in 0..3 {
                let (angle, speed) = (&mut self.angles[i], &mut self.speed[i]);
                if *angle == 0.0 && *speed == 0.0 {
                    continue;
                }
                if *angle != 0.0 {
                    let toward = if *angle > 0.0 { -1.0 } else { 1.0 };
                    *speed += toward * center * ft;
                }
                let mut change = ft * *speed;
                if change * *angle < 0.0 {
                    change *= RETURN_SCALE;
                }
                let next = *angle + change;
                if next * *angle < 0.0 {
                    // About to cross the centre: stop there.
                    *angle = 0.0;
                    *speed = 0.0;
                    continue;
                }
                *angle = next;
                if next == 0.0 {
                    *speed = 0.0;
                } else if next.abs() > KICK_MAX {
                    *angle = KICK_MAX.copysign(next);
                    *speed = 0.0;
                }
            }
            t -= STEP_MS;
        }
    }

    /// Zeroes both kicks, as `0x30028a70` does on a respawn.
    pub fn reset(&mut self) {
        self.angles = [0.0; 3];
        self.speed = [0.0; 3];
        self.gun = GunKick::default();
    }

    /// The view kick, degrees per axis, wire convention (pitch positive
    /// down): what syscall 0x56 hands `CL_FinishMove`.
    pub fn view_kick(&self) -> [f32; 3] {
        self.angles
    }

    /// The gun spring's pitch and yaw, degrees, wire convention.
    pub fn gun_kick(&self) -> [f32; 2] {
        self.gun.angles
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// `m1carbine_mp`'s kick keys as `pak0.pk3` spells them.
    fn carbine() -> WeaponDef {
        let mut m = HashMap::new();
        for (k, v) in [
            ("aimDownSight", "1"),
            ("gunMaxPitch", "5"),
            ("gunMaxYaw", "5"),
            ("adsGunKickPitchMin", "-60"),
            ("adsGunKickPitchMax", "-60"),
            ("adsGunKickYawMin", "-40"),
            ("adsGunKickYawMax", "40"),
            ("adsGunKickAccel", "800"),
            ("adsGunKickSpeedMax", "300"),
            ("adsGunKickSpeedDecay", "50"),
            ("adsGunKickStaticDecay", "10"),
            ("adsViewKickPitchMin", "30"),
            ("adsViewKickPitchMax", "45"),
            ("adsViewKickYawMin", "-10"),
            ("adsViewKickYawMax", "30"),
            ("adsViewKickCenterSpeed", "800"),
            ("hipGunKickPitchMin", "-30"),
            ("hipGunKickPitchMax", "-35"),
            ("hipGunKickYawMin", "-2"),
            ("hipGunKickYawMax", "18"),
            ("hipGunKickAccel", "1000"),
            ("hipGunKickSpeedMax", "1200"),
            ("hipGunKickSpeedDecay", "100"),
            ("hipGunKickStaticDecay", "2"),
            ("hipViewKickPitchMin", "40"),
            ("hipViewKickPitchMax", "40"),
            ("hipViewKickYawMin", "-15"),
            ("hipViewKickYawMax", "15"),
            ("hipViewKickCenterSpeed", "800"),
        ] {
            m.insert(k.to_string(), v.to_string());
        }
        WeaponDef::from_map(&m)
    }

    /// The first CRT draws from seed 1 are 41 and 18467.
    #[test]
    fn rand_is_msvc_rand() {
        let mut r = CrtRand(1);
        assert_eq!(r.random(), 41.0 / 32768.0);
        assert_eq!(r.random(), 18467.0 / 32768.0);
    }

    /// A hip carbine shot sets the view kick's pitch speed to -40 (both
    /// ends of its range) and the roll to half the yaw the other way.
    #[test]
    fn a_hip_shot_sets_the_view_speed_from_the_hip_ranges() {
        let def = carbine();
        let mut r = Recoil::default();
        r.fire(&def, 0.0);
        assert_eq!(r.speed[0], -40.0);
        assert!((-15.0..=15.0).contains(&r.speed[1]));
        assert_eq!(r.speed[2], r.speed[1] * -0.5);
        // The gun's hip pitch range is -30..-35.
        assert!((-35.0..=-30.0).contains(&r.gun.speed[0]), "{:?}", r.gun);
        // A second shot sets the view speed again but adds to the gun's.
        let gun = r.gun.speed[0];
        r.fire(&def, 0.0);
        assert_eq!(r.speed[0], -40.0);
        assert!(r.gun.speed[0] < gun - 29.0);
    }

    /// The sight's view ranges need a full `frac`; its gun ranges any.
    #[test]
    fn the_sight_ranges_follow_the_fraction() {
        let def = carbine();
        let mut r = Recoil::default();
        r.fire(&def, 0.5);
        assert_eq!(r.speed[0], -40.0, "half up is still the hip view kick");
        assert_eq!(r.gun.speed[0], -60.0, "but the sight's gun kick");
        r.fire(&def, 1.0);
        assert!((-45.0..=-30.0).contains(&r.speed[0]), "{:?}", r.speed);
    }

    /// The hip shot's curve: the pitch rises to its peak where 800 a second
    /// squared has eaten the 40 a second (50 ms, about one degree up), then
    /// comes back at 6% of the speed and stops at the centre without
    /// crossing it.
    #[test]
    fn the_view_kick_peaks_and_settles_at_the_centre() {
        let def = carbine();
        let mut r = Recoil::default();
        r.fire(&def, 0.0);
        r.speed[1] = 0.0;
        r.speed[2] = 0.0;
        let mut peak = 0.0f32;
        let mut peak_ms = 0;
        let mut settled_ms = None;
        for ms in (5..=3000).step_by(5) {
            r.step(5, Some(&def), 0.0, true);
            if r.angles[0] < peak {
                peak = r.angles[0];
                peak_ms = ms;
            }
            assert!(r.angles[0] <= 0.0, "crossed the centre at {ms} ms");
            if settled_ms.is_none() && r.angles[0] == 0.0 && r.speed[0] == 0.0 {
                settled_ms = Some(ms);
            }
        }
        assert!((45..=55).contains(&peak_ms), "peak at {peak_ms} ms");
        assert!((-1.15..-0.95).contains(&peak), "peak {peak}");
        let settled_ms = settled_ms.expect("back at the centre");
        assert!(settled_ms < 1000, "settled at {settled_ms} ms");
    }

    /// A long frame is the same as its 5 ms slices.
    #[test]
    fn a_frame_steps_in_five_ms_slices() {
        let def = carbine();
        let mut a = Recoil::default();
        a.fire(&def, 0.0);
        let mut b = Recoil::default();
        b.fire(&def, 0.0);
        a.step(33, Some(&def), 0.0, true);
        for ms in [5, 5, 5, 5, 5, 5, 3] {
            b.step(ms, Some(&def), 0.0, true);
        }
        assert_eq!(a.angles, b.angles);
        assert_eq!(a.speed, b.speed);
    }

    /// A rapid string of shots piles the kick up to the 10 degree clamp.
    #[test]
    fn the_view_kick_clamps_at_ten_degrees() {
        let def = carbine();
        let mut r = Recoil::default();
        for _ in 0..400 {
            r.speed = [-2000.0, 0.0, 0.0];
            r.step(5, Some(&def), 0.0, true);
        }
        assert_eq!(r.angles[0], -10.0);
    }

    /// Off the client's own view the view kick is zeroed every frame.
    #[test]
    fn a_followed_view_has_no_view_kick() {
        let def = carbine();
        let mut r = Recoil::default();
        r.fire(&def, 0.0);
        r.step(20, Some(&def), 0.0, false);
        assert_eq!(r.view_kick(), [0.0; 3]);
    }

    /// The gun spring under the sight: the carbine's -60 kicks the gun up,
    /// `gunMaxPitch` 5 caps it, and it settles back to zero.
    #[test]
    fn the_gun_spring_kicks_and_settles() {
        let def = carbine();
        let mut r = Recoil::default();
        r.fire(&def, 1.0);
        let mut top = 0.0f32;
        for _ in 0..200 {
            r.step(10, Some(&def), 1.0, true);
            top = top.min(r.gun_kick()[0]);
            assert!(r.gun_kick()[0] >= -5.0);
        }
        assert!(top < -0.25, "the gun rose, {top}");
        assert_eq!(r.gun_kick(), [0.0; 2]);
    }

    /// A weapon without `aimDownSight` never steps its spring.
    #[test]
    fn no_sight_no_gun_spring() {
        let mut def = carbine();
        def.aim_down_sight = false;
        let mut r = Recoil::default();
        r.fire(&def, 0.0);
        r.step(50, Some(&def), 0.0, true);
        assert_eq!(r.gun_kick(), [0.0; 2]);
    }
}
