//! The kick a CoD2 shot gives the client's own view and gun
//! (docs/research/cod2-weapons.md, "The recoil"). CoD2 1.3's cgame runs
//! CoD1's code with one addition, the reduced kick, so the state and the
//! steps are [`Recoil`]'s:
//!
//! - `CG_FireWeapon` (Mac 0x1d823c) runs `BG_WeaponFireRecoil` (Mac
//!   0x7b298) for a shot of the playerstate's own body: it sets the view
//!   kick's speed (`cg + 0x2c030`) and adds to the gun spring's
//!   (`cg + 0x2c0b0`).
//! - `CG_DrawActiveFrame` (Mac 0x1d4a50) steps the view kick
//!   (`cg + 0x2c03c`) back to centre in 5 ms slices, or zeroes it off the
//!   body's own view, and `CG_DrawActive` (Mac 0x1cefca) hands it to
//!   `CL_SetUserCmdAimValues`: `CL_CreateCmd` (Mac 0x189f0e) adds it to the
//!   cmd's angles. The kick moves the aim; the server and the predictor
//!   read it off the cmd like any turn.
//! - `CG_AddViewWeapon` (Mac 0x1da2f8) steps the gun spring in
//!   `BG_CalculateWeaponAngles` (Mac 0x7f226) and turns the view weapon by
//!   it (`play::cod2_gun::GunInput::recoil`).

use std::collections::HashMap;

use vcod_cod2::net::PlayerView;
use vcod_cod2::net::events as ev;
use vcod_common::weapon::WeaponDef as KickKeys;

use crate::play::cod2_events::Event2;
use crate::play::recoil::Recoil;

/// `pm_flags` under which `CG_FireWeapon` kicks the playerstate's own
/// shot (`snap->ps.pm_flags & 0xc00000`).
const PMF_FIRST_PERSON: i32 = 0xc0_0000;
/// `pm_flags` under which the view kick steps; off it the kick is zeroed
/// every frame (`snap->ps.pm_flags` byte 2 & 0x80).
const PMF_OWN_VIEW: i32 = 0x80_0000;
/// `pm_flags`: the off hand is out; `BG_GetViewmodelWeaponIndex` takes
/// `offHandIndex`.
const PMF_OFFHAND: i32 = 0x10;

/// The keys `BG_WeaponFireRecoil` and `BG_CalculateWeaponAngles` read.
/// They carry CoD1's names, so they parse into CoD1's def, which
/// [`Recoil`] steps.
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

/// One weapon's kick keys.
pub struct KickDef {
    keys: KickKeys,
    /// `hipGunKickReducedKickPercent`, `adsGunKickReducedKickPercent`.
    reduced_percent: [f32; 2],
}

impl KickDef {
    pub fn new(def: &vcod_cod2::weapon::WeaponDef) -> KickDef {
        let map: HashMap<String, String> = KICK_KEYS
            .iter()
            .map(|&k| {
                let v = if k == "aimDownSight" {
                    u8::from(def.flag(k)).to_string()
                } else {
                    def.float(k).to_string()
                };
                (k.to_string(), v)
            })
            .collect();
        KickDef {
            keys: KickKeys::from_map(&map),
            reduced_percent: [
                def.float("hipGunKickReducedKickPercent"),
                def.float("adsGunKickReducedKickPercent"),
            ],
        }
    }
}

/// How many times `event` runs `CG_FireWeapon` (`CG_EntityEvent`, Mac
/// 0x1e4650): once for 158..160 and 175, twice for the quad barrel's 176
/// and 177.
pub fn fire_calls(event: i32) -> usize {
    match event {
        ev::EV_FIRE_WEAPON
        | ev::EV_FIRE_WEAPONB
        | ev::EV_FIRE_WEAPON_LASTSHOT
        | ev::EV_FIRE_WEAPON_MG42 => 1,
        ev::EV_FIRE_QUADBARREL_1 | ev::EV_FIRE_QUADBARREL_2 => 2,
        _ => 0,
    }
}

/// `BG_WeaponFireRecoil`'s scale on every draw: 1, or while
/// `weaponRestrictKickTime` runs the reduced percent (the sight's only at
/// a full `frac`) over 100.
pub fn kick_scale(restrict_kick_time: i32, frac: f32, reduced_percent: [f32; 2]) -> f32 {
    if restrict_kick_time > 0 {
        0.01 * reduced_percent[usize::from(frac == 1.0)]
    } else {
        1.0
    }
}

/// `BG_GetViewmodelWeaponIndex` less the binoculars, which fire nothing
/// and whose index the kick does not need.
pub fn kick_weapon(ps: &PlayerView) -> i32 {
    if ps.pm_flags & PMF_OFFHAND != 0 {
        ps.raw.i32("offHandIndex")
    } else {
        ps.weapon
    }
}

/// The recoil of a CoD2 connection's own body.
#[derive(Default)]
pub struct Cod2Recoil {
    recoil: Recoil,
    /// `cg.time` at the last step.
    last_ms: Option<i32>,
}

impl Cod2Recoil {
    /// One frame at `time_ms` (`cg.time`): each of `events` that is a shot
    /// of `ps`'s own body kicks, then both kicks step by the frame's
    /// length. `def` is the held weapon's, `None` for weapon 0 or a file
    /// that did not load.
    pub fn frame(
        &mut self,
        events: &[Event2],
        ps: &PlayerView,
        def: Option<&KickDef>,
        time_ms: i32,
    ) {
        let frac = ps.raw.f32("fWeaponPosFrac");
        if let Some(def) = def
            && ps.pm_flags & PMF_FIRST_PERSON != 0
        {
            let scale = kick_scale(
                ps.raw.i32("weaponRestrictKickTime"),
                frac,
                def.reduced_percent,
            );
            let shots: usize = events
                .iter()
                .filter(|e| e.entity as i32 == ps.client_num)
                .map(|e| fire_calls(e.event))
                .sum();
            for _ in 0..shots {
                self.recoil.fire_scaled(&def.keys, frac, scale);
            }
        }
        let ms = self.last_ms.map_or(0, |l| (time_ms - l).clamp(0, 1000));
        self.last_ms = Some(time_ms);
        self.recoil.step(
            ms,
            def.map(|d| &d.keys),
            frac,
            ps.pm_flags & PMF_OWN_VIEW != 0,
        );
    }

    /// Zeroes both kicks (`CG_Respawn`, Mac 0x1e3c56).
    pub fn reset(&mut self) {
        self.recoil.reset();
        self.last_ms = None;
    }

    /// The view kick, degrees (pitch positive down, yaw, roll), which the
    /// cmd's angles carry.
    pub fn view_kick(&self) -> [f32; 3] {
        self.recoil.view_kick()
    }

    /// The gun spring's pitch and yaw, degrees.
    pub fn gun_kick(&self) -> [f32; 2] {
        self.recoil.gun_kick()
    }
}

/// A recoil 20 ms after a BAR shot at the hip, its view kick off zero.
#[cfg(test)]
pub(crate) fn kicked() -> Cod2Recoil {
    let def = tests::bar();
    let ps = tests::own_ps(PMF_OWN_VIEW, 0.0);
    let mut r = Cod2Recoil::default();
    r.frame(&[], &ps, Some(&def), 1000);
    r.frame(&[tests::shot(0)], &ps, Some(&def), 1000);
    r.frame(&[], &ps, Some(&def), 1020);
    r
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::Vec3;
    use vcod_cod2::net::PlayerState2;
    use vcod_cod2::net::delta::field_index;
    use vcod_cod2::net::fields::PLAYER_STATE_FIELDS;

    /// `bar_mp`'s kick keys as the 1.3 `iw_13.iwd` spells them.
    const BAR: &str = "WEAPONFILE\\aimDownSight\\1\\\
        gunMaxPitch\\3\\gunMaxYaw\\3\\fireTime\\0.11\\\
        hipGunKickReducedKickBullets\\0\\hipGunKickReducedKickPercent\\0\\\
        hipGunKickPitchMin\\55\\hipGunKickPitchMax\\60\\hipGunKickYawMin\\-30\\hipGunKickYawMax\\30\\\
        hipGunKickAccel\\1000\\hipGunKickSpeedMax\\500\\hipGunKickSpeedDecay\\16\\\
        hipGunKickStaticDecay\\20\\\
        adsGunKickReducedKickBullets\\0\\adsGunKickReducedKickPercent\\0\\\
        adsGunKickPitchMin\\55\\adsGunKickPitchMax\\60\\adsGunKickYawMin\\-30\\adsGunKickYawMax\\30\\\
        adsGunKickAccel\\1000\\adsGunKickSpeedMax\\1000\\adsGunKickSpeedDecay\\100\\\
        adsGunKickStaticDecay\\2\\\
        hipViewKickPitchMin\\60\\hipViewKickPitchMax\\65\\hipViewKickYawMin\\-70\\\
        hipViewKickYawMax\\60\\hipViewKickCenterSpeed\\1700\\\
        adsViewKickPitchMin\\60\\adsViewKickPitchMax\\65\\adsViewKickYawMin\\-70\\\
        adsViewKickYawMax\\60\\adsViewKickCenterSpeed\\1700";

    pub(super) fn bar() -> KickDef {
        let p = vcod_cod2::weapon::parse("bar_mp", BAR.as_bytes(), &[]).expect("parses");
        KickDef::new(&p.def)
    }

    pub(super) fn own_ps(pm_flags: i32, frac: f32) -> PlayerView {
        let mut raw = PlayerState2::default();
        let mut set = |n: &str, v: u32| raw.f[field_index(&PLAYER_STATE_FIELDS, n)] = v;
        set("pm_flags", pm_flags as u32);
        set("weapon", 3);
        set("fWeaponPosFrac", frac.to_bits());
        PlayerView::new(&raw, 0)
    }

    pub(super) fn shot(entity: u32) -> Event2 {
        Event2 {
            entity,
            event: ev::EV_FIRE_WEAPON,
            parm: 0,
            etype: 1,
            eflags: 0,
            weapon: 3,
            surf: 0,
            other_entity: 0,
            time: 0,
            scale: 0,
            pos: Vec3::ZERO,
            angles: Vec3::ZERO,
            angles2: [0.0; 2],
            from_ps: true,
        }
    }

    #[test]
    fn the_kick_keys_carry_over() {
        let k = bar().keys;
        assert!(k.aim_down_sight);
        assert_eq!(k.aim.view_kick_pitch, [[60.0, 65.0]; 2]);
        assert_eq!(k.aim.view_kick_yaw, [[-70.0, 60.0]; 2]);
        assert_eq!(k.aim.view_kick_center_speed, [1700.0; 2]);
        assert_eq!(k.aim.gun_kick_pitch, [[55.0, 60.0]; 2]);
        assert_eq!(k.aim.gun_kick_accel, [1000.0; 2]);
        assert_eq!(k.aim.gun_kick_speed_max, [500.0, 1000.0]);
        assert_eq!([k.aim.gun_max_pitch, k.aim.gun_max_yaw], [3.0, 3.0]);
    }

    /// The reduced percent only while `weaponRestrictKickTime` runs, the
    /// sight's only at a full `frac`.
    #[test]
    fn the_reduced_kick_scale() {
        assert_eq!(kick_scale(0, 1.0, [25.0, 50.0]), 1.0);
        assert_eq!(kick_scale(1, 0.0, [25.0, 50.0]), 0.25);
        assert_eq!(kick_scale(1, 0.99, [25.0, 50.0]), 0.25);
        assert_eq!(kick_scale(1, 1.0, [25.0, 50.0]), 0.5);
    }

    #[test]
    fn fire_calls_in_cod2_numbering() {
        assert_eq!(fire_calls(158), 1);
        assert_eq!(fire_calls(160), 1);
        assert_eq!(fire_calls(175), 1);
        assert_eq!(fire_calls(176), 2);
        assert_eq!(fire_calls(161), 0);
    }

    /// A BAR shot pitches the view up 60 to 65 degrees a second, the
    /// centring speed of 1700 stops it within 40 ms, and it is back at the
    /// centre well inside a second.
    #[test]
    fn a_bar_shot_kicks_the_view_up_and_back() {
        let def = bar();
        let ps = own_ps(PMF_OWN_VIEW, 0.0);
        let mut r = Cod2Recoil::default();
        r.frame(&[], &ps, Some(&def), 1000);
        r.frame(&[shot(0)], &ps, Some(&def), 1010);
        let mut peak = 0.0f32;
        for t in (1020..2000).step_by(10) {
            r.frame(&[], &ps, Some(&def), t);
            peak = peak.min(r.view_kick()[0]);
            assert!(r.view_kick()[0] <= 0.0);
        }
        assert!((-1.3..-0.9).contains(&peak), "peak {peak}");
        assert_eq!(r.view_kick(), [0.0; 3]);
    }

    /// Another body's shot, a followed view and a spectator's playerstate
    /// kick nothing.
    #[test]
    fn only_the_own_body_kicks() {
        let def = bar();
        let mut r = Cod2Recoil::default();
        let ps = own_ps(PMF_OWN_VIEW, 0.0);
        r.frame(&[shot(5)], &ps, Some(&def), 1000);
        r.frame(&[], &ps, Some(&def), 1020);
        assert_eq!(r.view_kick(), [0.0; 3]);
        // Followed (0x400000): the kick fires but the view zeroes it.
        let ps = own_ps(0x40_0000, 0.0);
        r.frame(&[shot(0)], &ps, Some(&def), 1040);
        assert_eq!(r.view_kick(), [0.0; 3]);
        let mut r = Cod2Recoil::default();
        let ps = own_ps(0, 0.0);
        r.frame(&[shot(0)], &ps, Some(&def), 1060);
        assert_eq!(r.gun_kick(), [0.0; 2]);
    }

    /// A BAR burst at the hip springs the gun pitch positive (nose down
    /// against the view) up to `gunMaxPitch`, and comes back to within a
    /// few hundredths of a degree: the spring's 1000 a second squared turns
    /// the speed by 5 a 5 ms slice, so it never falls under the 1 a second
    /// that zeroes it, and the gun hovers there.
    #[test]
    fn the_gun_spring_kicks_and_settles() {
        let def = bar();
        let ps = own_ps(PMF_OWN_VIEW, 0.0);
        let mut r = Cod2Recoil::default();
        r.frame(&[], &ps, Some(&def), 0);
        let mut top = 0.0f32;
        for t in (10..3000).step_by(10) {
            let evs = if t % 110 == 0 && t < 1000 {
                vec![shot(0)]
            } else {
                vec![]
            };
            r.frame(&evs, &ps, Some(&def), t);
            top = top.max(r.gun_kick()[0]);
            assert!(r.gun_kick()[0] <= 3.0);
        }
        assert!(top > 1.0, "the gun moved, {top}");
        assert!(r.gun_kick()[0].abs() < 0.15, "{:?}", r.gun_kick());
    }
}
