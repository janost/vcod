//! The kicks CoD4 1.7 gives the own view and gun (docs/research/
//! cod4-client-view.md 14): a shot's recoil, which rides the cmd's angles
//! (`CG_KickAngles`, `CL_SetUserCmdAimValues`), a hit's damage kick, which
//! only turns the drawn view and gun (`CG_DamageFeedback`,
//! `BG_CalculateView_DamageKick`, `BG_CalculateWeaponPosition_DamageKick`),
//! and the landing's dip of the eye and the gun. The recoil's state and
//! steps are CoD1's (`play::recoil::Recoil`): 1.7's `CG_KickAngles`
//! (`iw3mp.exe` 0x44fe20) and the gun spring keep its constants.

use crate::play::recoil::Recoil;
use vcod_common::weapon::WeaponDef as KickKeys;

/// The keys `BG_WeaponFireRecoil` (0x41a7b0) and the gun spring read,
/// CoD1's names at their 1.7 `WeaponDef` offsets
/// (`vcod_cod4::weapon_file::FIELDS`).
const KICK_KEYS: &[&str] = &[
    "aimDownSight",
    "gunMaxPitch",
    "gunMaxYaw",
    "hipGunKickPitchMin",
    "hipGunKickPitchMax",
    "hipGunKickYawMin",
    "hipGunKickYawMax",
    "hipGunKickAccel",
    "hipGunKickSpeedMax",
    "hipGunKickSpeedDecay",
    "hipGunKickStaticDecay",
    "hipViewKickPitchMin",
    "hipViewKickPitchMax",
    "hipViewKickYawMin",
    "hipViewKickYawMax",
    "hipViewKickCenterSpeed",
    "adsGunKickPitchMin",
    "adsGunKickPitchMax",
    "adsGunKickYawMin",
    "adsGunKickYawMax",
    "adsGunKickAccel",
    "adsGunKickSpeedMax",
    "adsGunKickSpeedDecay",
    "adsGunKickStaticDecay",
    "adsViewKickPitchMin",
    "adsViewKickPitchMax",
    "adsViewKickYawMin",
    "adsViewKickYawMax",
    "adsViewKickCenterSpeed",
];

/// One weapon's kick keys, read off its 0x878-byte `WeaponDef`.
#[derive(Clone)]
pub struct KickDef {
    keys: KickKeys,
    /// `hipGunKickReducedKickPercent` (+1828), `adsGunKickReducedKickPercent`
    /// (+1756).
    reduced_percent: [f32; 2],
}

impl KickDef {
    pub fn from_raw(raw: &[u8]) -> KickDef {
        let word = |o: u16| {
            raw.get(o as usize..o as usize + 4)
                .map_or([0; 4], |b| b.try_into().unwrap())
        };
        let field = |name: &str| {
            vcod_cod4::weapon_file::FIELDS
                .iter()
                .find(|f| f.0 == name)
                .map(|&(_, o, kind)| match kind {
                    6 => f32::from_le_bytes(word(o)).to_string(),
                    _ => i32::from_le_bytes(word(o)).to_string(),
                })
                .unwrap_or_default()
        };
        let map = KICK_KEYS
            .iter()
            .map(|&k| (k.to_string(), field(k)))
            .collect();
        KickDef {
            keys: KickKeys::from_map(&map),
            reduced_percent: [1828, 1756].map(|o| f32::from_le_bytes(word(o))),
        }
    }
}

/// Whether a fired event's name runs `CG_FireWeapon`'s recoil for the
/// own body (`G_PlayerEvent`'s list).
pub fn is_fire(name: &str) -> bool {
    matches!(
        name,
        "EV_FIRE_WEAPON" | "EV_FIRE_WEAPON_LASTSHOT" | "EV_FIRE_WEAPON_MG42"
    )
}

/// `otherFlags` 4: the playerstate is our own view, whose kick steps
/// (0x4529fb); 6, ours or followed, whose shots kick (0x457cc7).
const OF_OWN_VIEW: u32 = 4;
const OF_VIEW_BODY: u32 = 6;

/// `cg.kickAngles` and the gun spring, and `cg.landChange` /
/// `cg.landTime`.
#[derive(Default)]
pub struct Cod4Kick {
    recoil: Recoil,
    last_ms: Option<i32>,
    /// `landChange` and `landTime` (cg time).
    land: (f32, i32),
    /// `clientNum` and `stats[4]`: a change is `CG_Respawn`.
    spawn: Option<(u32, i32)>,
}

impl Cod4Kick {
    /// `CG_Respawn` when the view's client or spawn count changes: both
    /// kicks zeroed.
    pub fn check_respawn(&mut self, client: u32, spawn_count: i32) {
        if self.spawn.is_some_and(|s| s != (client, spawn_count)) {
            self.recoil.reset();
        }
        self.spawn = Some((client, spawn_count));
    }

    /// One own shot (`CG_FireWeapon` with `isPlayer`, 0x457d2e) of a body
    /// whose `otherFlags` are `other`: `BG_WeaponFireRecoil`, its draws
    /// cut to the reduced percent while `weaponRestrictKickTime` runs.
    pub fn fire(&mut self, def: &KickDef, other: u32, frac: f32, restrict_kick_time: i32) {
        if other & OF_VIEW_BODY == 0 {
            return;
        }
        let scale = if restrict_kick_time > 0 {
            0.01 * def.reduced_percent[usize::from(frac == 1.0)]
        } else {
            1.0
        };
        self.recoil.fire_scaled(&def.keys, frac, scale);
    }

    /// One frame at cg time `now`: `CG_KickAngles` while `other` names our
    /// own view, else the view kick zeroed; then the gun spring.
    pub fn step(&mut self, now: i32, def: Option<&KickDef>, frac: f32, other: u32) {
        let ms = self.last_ms.map_or(0, |l| (now - l).clamp(0, 1000));
        self.last_ms = Some(now);
        self.recoil
            .step(ms, def.map(|d| &d.keys), frac, other & OF_OWN_VIEW != 0);
    }

    /// An own landing event at cg time `now` (`CG_EntityEvent`, 0x435e43
    /// and 0x435eb7): a plain one dips by its parm, a hurt one by
    /// `(h - 12) / 26 * 4 + 4` (at most 24) for the fall height `h` its
    /// parm's share of the fall damage range gives, none under 12.
    pub fn land(&mut self, name: &str, parm: u32, fall_heights: (f32, f32), now: i32) {
        if name.starts_with("EV_LANDING_PAIN_") {
            let (lo, hi) = fall_heights;
            let h = (hi - lo) * parm as f32 * 0.01 + lo;
            if h > 12.0 {
                let dip = (((h - 12.0) / 26.0 * 4.0 + 4.0) as i32).min(24);
                if dip > 0 {
                    self.land = (-(dip as f32), now);
                }
            }
        } else if name.starts_with("EV_LANDING_") {
            self.land = (-(parm as f32), now);
        }
    }

    /// The view kick, degrees (pitch positive down, yaw, roll), which the
    /// next cmd's angles carry (0x463aa9).
    pub fn view_kick(&self) -> [f32; 3] {
        self.recoil.view_kick()
    }

    /// The gun spring's pitch and yaw, degrees.
    pub fn gun_kick(&self) -> [f32; 2] {
        self.recoil.gun_kick()
    }

    /// `landChange` and the ms since `landTime` at `now`.
    pub fn landing(&self, now: i32) -> (f32, i32) {
        (self.land.0, now - self.land.1)
    }
}

/// `CG_OffsetFirstPersonView`'s dip of the eye (0x4502b2): down to the
/// change over 150 ms, back over the next 300.
pub fn eye_dip((change, t): (f32, i32)) -> f32 {
    let t = t as f32;
    if t > 0.0 && t < 150.0 {
        change * t / 150.0
    } else if (150.0..450.0).contains(&t) {
        change * (1.0 - (t - 150.0) / 300.0)
    } else {
        0.0
    }
}

/// The weapon position step's dip of the gun (0x455500): a quarter of
/// the eye's, over the same times.
pub fn gun_dip((change, t): (f32, i32)) -> f32 {
    if t < 150 {
        change * 0.25 * t as f32 / 150.0
    } else if t < 450 {
        change * 0.25 * (450 - t) as f32 / 300.0
    } else {
        0.0
    }
}

/// `bg_viewKickScale` 0.2, `bg_viewKickMin` 5 and `bg_viewKickMax` 90
/// (`iw3mp.exe` 0x4095c9..0x4096e5).
const VIEW_KICK_SCALE: f32 = 0.2;
const VIEW_KICK_MIN: f32 = 5.0;
const VIEW_KICK_MAX: f32 = 90.0;

/// `GetLeanFraction`: `(2 - |f|) * f`.
fn lean_fraction(f: f32) -> f32 {
    (2.0 - f.abs()) * f
}

/// `CG_DamageFeedback`'s kick (0x445d00): `cg.v_dmg_pitch`, `v_dmg_roll`
/// and `damageTime`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct DamageKick {
    pub pitch: f32,
    pub roll: f32,
    /// `cg.snap->serverTime` of the hit, 0 for none.
    pub time: i32,
}

impl DamageKick {
    /// A hit of `count` from `yaw` and `pitch` (bytes, 255 and 255 none)
    /// on a view at `view` (pitch, yaw, degrees): `count * 0.2` within
    /// 5..90, along the view's forward as pitch and its left as roll.
    pub fn feedback(&mut self, count: u32, yaw: u32, pitch: u32, view: [f32; 2], time: i32) {
        let kick = (count as f32 * VIEW_KICK_SCALE).clamp(VIEW_KICK_MIN, VIEW_KICK_MAX);
        if (yaw, pitch) == (255, 255) {
            (self.pitch, self.roll) = (-kick, 0.0);
        } else {
            let p = (pitch as f32 / 255.0 * 360.0).to_radians();
            let y = (yaw as f32 / 255.0 * 360.0).to_radians();
            let dir = glam::Vec3::new(p.cos() * y.cos(), p.cos() * y.sin(), -p.sin());
            let (vp, vy) = (view[0].to_radians(), view[1].to_radians());
            let forward = glam::Vec3::new(vp.cos() * vy.cos(), vp.cos() * vy.sin(), -vp.sin());
            let left = glam::Vec3::new(-vy.sin(), vy.cos(), 0.0);
            (self.pitch, self.roll) = (kick * dir.dot(forward), -kick * dir.dot(left));
        }
        self.time = time;
    }

    /// `BG_CalculateView_DamageKick` (0x41b8b0) at `t` ms since the hit:
    /// in over 100 ms, out over 400, scaled by `1 - frac / 2` (times
    /// `1 + frac / 2` under a scope's overlay); (pitch, yaw, roll).
    pub fn view(&self, t: i32, frac: f32, overlay: bool) -> [f32; 3] {
        let t = t.max(0);
        if self.time == 0 {
            return [0.0; 3];
        }
        let mut factor = 1.0 - frac * 0.5;
        if frac != 0.0 && overlay {
            factor *= frac * 0.5 + 1.0;
        }
        let f = ramp(t as f32, 100.0, 400.0) * factor;
        [f * self.pitch, 0.0, f * self.roll]
    }

    /// `BG_CalculateWeaponPosition_DamageKick` (0x41b2b0): the gun's, in
    /// over `100 * k` ms and out over `400 * k` for `k = frac / 2 + 0.5`,
    /// scaled by `k` (times `1 - frac * 0.75` under a scope's overlay);
    /// half the pitch, the roll as yaw against it, half the roll.
    pub fn gun(&self, t: i32, frac: f32, overlay: bool) -> [f32; 3] {
        let t = t.max(0);
        if self.time == 0 {
            return [0.0; 3];
        }
        let k = frac * 0.5 + 0.5;
        let factor = if frac != 0.0 && overlay {
            (1.0 - frac * 0.75) * k
        } else {
            k
        };
        let f = ramp(t as f32, k * 100.0, k * 400.0) * factor;
        [f * self.pitch * 0.5, -f * self.roll, f * self.roll * 0.5]
    }
}

/// The damage kick's share at `t` ms: `GetLeanFraction` up over `deflect`,
/// then down over `ret`, 0 after.
fn ramp(t: f32, deflect: f32, ret: f32) -> f32 {
    if t < deflect {
        lean_fraction(t / deflect)
    } else {
        let r = 1.0 - (t - deflect) / ret;
        if r > 0.0 {
            1.0 - lean_fraction(1.0 - r)
        } else {
            0.0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A hit from the left (the push toward -y, yaw byte 191) on a view
    /// at yaw 0 rolls it right and does not pitch it; 100 ms in it is
    /// whole, 500 ms in gone.
    #[test]
    fn a_side_hit_rolls_the_view_in_and_out() {
        let mut d = DamageKick::default();
        d.feedback(50, 191, 0, [0.0, 0.0], 1000);
        assert!((d.roll - 10.0).abs() < 0.05 && d.pitch.abs() < 0.1, "{d:?}");
        assert_eq!(d.view(100, 0.0, false), [d.pitch, 0.0, d.roll]);
        assert_eq!(d.view(500, 0.0, false), [0.0; 3]);
        let g = d.gun(50, 0.0, false);
        assert!(g[1] < 0.0 && g[2] > 0.0, "{g:?}");
        // The kick is at least 5, at most 90.
        d.feedback(1, 255, 255, [0.0, 0.0], 1000);
        assert_eq!((d.pitch, d.roll), (-5.0, 0.0));
    }

    /// A plain landing of 8 dips the eye 8 at 150 ms and the gun 2, back
    /// by 450; a hurt one from 200 units dips 28 capped to 24.
    #[test]
    fn the_landing_dips() {
        let mut k = Cod4Kick::default();
        k.land("EV_LANDING_DIRT", 8, (128.0, 300.0), 1000);
        assert_eq!(eye_dip(k.landing(1150)), -8.0);
        assert_eq!(gun_dip(k.landing(1150)), -2.0);
        assert_eq!(eye_dip(k.landing(1450)), 0.0);
        k.land("EV_LANDING_PAIN_DIRT", 100, (128.0, 300.0), 2000);
        assert_eq!(k.landing(2000).0, -24.0);
    }
}
