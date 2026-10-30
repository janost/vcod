//! The weapon half of 1.7's `PmoveSingle`: `PM_AdjustAimSpreadScale`,
//! `PM_UpdateAimDownSightFlag` and `PM_Weapon` with what it calls for a
//! shot, a burst, a reload, a rechamber, a switch and the sprint's states
//! (docs/research/cod4-pmove.md 16). The kill capture's shooter replays
//! through it (`cod4_moves_ab`'s `shooter` rows).
//!
//! The offhand (frag, smoke, flash: states 15..20, the cook and the
//! suicide) and the melee (12..14) run here too; the walk capture's
//! frag throw replays through them (`cod4_moves_ab`'s `walk` rows, armed).
//!
//! The melee charge's swing (`meleeChargeTime`, anim 9) starts here, its
//! lunge is `super::charge`. Not here: detonation (21), night vision (25, 26), `ViewModelOverride` and the anim
//! script's events. A state this module does not run holds until the
//! server's cmds move it on.

use super::{Pml, Pmove, button, ef, pm_type, pmf};

/// `ps.weaponstate` (ps+0xec), vcod's names. `PM_Weapon`'s timer switch
/// at 0x80636c4 jumps through 27 slots (rodata 0x820e524). What each value
/// means is read off where 1.7 stores it, VERIFIED (the disassembly): 9
/// with `EV_RELOAD_START` (0x806132d), 8 and 10 when the trigger cuts a
/// segmented reload (0x8061597, 0x8061694), 11 with `EV_RELOAD_END`
/// (0x8063e76), 6 with `EV_RECHAMBER_WEAPON` (0x806362c), 12 with
/// `EV_MELEE_SWIPE` (0x806350c), 13 with `EV_FIRE_MELEE` (0x8063f16), 15
/// after `EV_SWITCH_OFFHAND` (0x806491e), 16 at `iHoldFireTime`
/// (0x8063bd5), 18 when the pin's timer runs out (0x8063c2a, the fuse
/// set), held there while both cmds hold the offhand keys, then 17 at
/// `iFireTime` (0x8064b81), 19 after `EV_USE_OFFHAND` (0x8063ce3), 20
/// next (0x8063a9e), 22 and 23 the sprint's (0x806313f, 0x8063d52), 24
/// its way out (0x8064d1b). The kill capture's shooter reads 5 firing and
/// 7 reloading (VERIFIED live). Which of 25 and 26 puts the goggles on is
/// INFERRED; vcod runs neither.
pub mod state {
    pub const READY: i32 = 0;
    pub const RAISE: i32 = 1;
    pub const RAISE_ALT: i32 = 2;
    pub const LOWER: i32 = 3;
    pub const LOWER_QUICK: i32 = 4;
    pub const FIRE: i32 = 5;
    pub const RECHAMBER: i32 = 6;
    pub const RELOAD: i32 = 7;
    pub const RELOAD_CUT: i32 = 8;
    pub const RELOAD_OPEN: i32 = 9;
    pub const RELOAD_OPEN_CUT: i32 = 10;
    pub const RELOAD_CLOSE: i32 = 11;
    pub const MELEE_SWING: i32 = 12;
    pub const MELEE_HIT: i32 = 13;
    pub const MELEE_RECOVER: i32 = 14;
    pub const OFFHAND_LOWER: i32 = 15;
    pub const GRENADE_PIN: i32 = 16;
    pub const GRENADE_THROW: i32 = 17;
    pub const GRENADE_HELD: i32 = 18;
    pub const GRENADE_GONE: i32 = 19;
    pub const OFFHAND_RAISE: i32 = 20;
    pub const SPRINT_IN: i32 = 22;
    pub const SPRINT_HOLD: i32 = 23;
    pub const SPRINT_OUT: i32 = 24;
    pub const NVG_ON: i32 = 25;
    pub const NVG_OFF: i32 = 26;
}

/// The weapon events, 1.7's numbers (`net::events`, VERIFIED off the name
/// table).
pub mod ev {
    pub const STOP_WEAPON_SOUND: u32 = 2;
    pub const NOAMMO: u32 = 11;
    pub const EMPTY_OFFHAND: u32 = 13;
    pub const RESET_ADS: u32 = 14;
    pub const RELOAD: u32 = 15;
    pub const RELOAD_FROM_EMPTY: u32 = 16;
    pub const RELOAD_START: u32 = 17;
    pub const RELOAD_END: u32 = 18;
    pub const RELOAD_START_NOTIFY: u32 = 19;
    pub const RELOAD_ADDAMMO: u32 = 20;
    pub const RAISE_WEAPON: u32 = 21;
    pub const FIRST_RAISE_WEAPON: u32 = 22;
    pub const PUTAWAY_WEAPON: u32 = 23;
    pub const WEAPON_ALT: u32 = 24;
    pub const PULLBACK_WEAPON: u32 = 25;
    pub const FIRE_WEAPON: u32 = 26;
    pub const FIRE_WEAPON_LASTSHOT: u32 = 27;
    pub const RECHAMBER_WEAPON: u32 = 28;
    pub const EJECT_BRASS: u32 = 29;
    pub const MELEE_SWIPE: u32 = 30;
    pub const FIRE_MELEE: u32 = 31;
    pub const PREP_OFFHAND: u32 = 32;
    pub const USE_OFFHAND: u32 = 33;
    pub const SWITCH_OFFHAND: u32 = 34;
    pub const GRENADE_SUICIDE: u32 = 62;
    pub const NO_FRAG_GRENADE_HINT: u32 = 67;
    pub const NO_SPECIAL_GRENADE_HINT: u32 = 68;
}

/// `OffhandClass`: the frag key takes class 1, the smoke key 2,
/// or 3 under `offhandSecondary` 1.
pub const OFFHAND_CLASS_FRAG: i32 = 1;
pub const OFFHAND_CLASS_SMOKE: i32 = 2;
pub const OFFHAND_CLASS_FLASH: i32 = 3;

/// `weapFlags` bits.
pub mod wf {
    /// A reload asked for by the script (the script reload's flag).
    pub const RELOAD: u32 = 1;
    /// The view model shows the offhand.
    pub const USING_OFFHAND: u32 = 2;
    pub const HOLDING_BREATH: u32 = 4;
    pub const NO_ADS: u32 = 0x20;
    /// `disableWeapons`.
    pub const DISABLED: u32 = 0x80;
    /// A fresh trigger pull on a burst weapon (the capture's 0x100 after a
    /// burst whose key went up, VERIFIED live).
    pub const TRIGGER_PULLED: u32 = 0x100;
}

/// `weapFireType_t`: 0 full auto, 1 single, 2..4 the bursts.
pub const FIRETYPE_FULLAUTO: i32 = 0;
pub const FIRETYPE_SINGLESHOT: i32 = 1;
pub const FIRETYPE_BURSTFIRE2: i32 = 2;
pub const FIRETYPE_BURSTFIRE4: i32 = 4;

/// `weapType_t` 1, the grenade.
pub const WEAPTYPE_GRENADE: i32 = 1;
/// `weapClass_t` 4 and 9.
pub const WEAPCLASS_PISTOL: i32 = 4;
pub const WEAPCLASS_ITEM: i32 = 9;

/// The `WeaponDef` fields the weapon half reads, at their 1.7 offsets in
/// the zone's 0x878-byte struct (the field order INFERRED, anchored as
/// cod4-gsc.md 7 says; the table in cod4-pmove.md 16), with the ammo and
/// clip slots the level's registration gave it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct WeaponDef {
    pub name: String,
    /// `playerAnimType` (+296): the anim script's `PLAYERANIMTYPE`.
    pub player_anim_type: i32,
    pub weap_type: i32,
    pub weap_class: i32,
    /// `penetrateType` (+308): 0 none, 1 small, 2 medium, 3 large, the
    /// row of `info/bullet_penetration_mp` a round reads.
    pub penetrate_type: i32,
    pub fire_type: i32,
    pub offhand_class: i32,
    pub ammo_index: usize,
    pub clip_index: usize,
    pub clip_size: i32,
    /// `maxAmmo` (+820): the reserve's cap.
    pub max_ammo: i32,
    pub shot_count: i32,
    pub damage: i32,
    pub player_damage: i32,
    pub fire_delay: i32,
    pub fire_time: i32,
    pub rechamber_time: i32,
    pub rechamber_bolt_time: i32,
    pub hold_fire_time: i32,
    pub reload_time: i32,
    pub reload_empty_time: i32,
    pub reload_add_time: i32,
    pub reload_start_time: i32,
    pub reload_start_add_time: i32,
    pub reload_end_time: i32,
    pub drop_time: i32,
    pub raise_time: i32,
    pub alt_drop_time: i32,
    pub alt_raise_time: i32,
    pub quick_drop_time: i32,
    pub quick_raise_time: i32,
    pub first_raise_time: i32,
    pub empty_raise_time: i32,
    pub empty_drop_time: i32,
    pub sprint_in_time: i32,
    pub sprint_out_time: i32,
    pub fuse_time: i32,
    pub no_ads_when_mag_empty: bool,
    pub overlay_reticle: i32,
    pub hip_spread_stand_min: f32,
    pub hip_spread_ducked_min: f32,
    pub hip_spread_prone_min: f32,
    pub hip_spread_stand_max: f32,
    pub hip_spread_ducked_max: f32,
    pub hip_spread_prone_max: f32,
    pub hip_spread_decay_rate: f32,
    pub hip_spread_fire_add: f32,
    pub hip_spread_turn_add: f32,
    pub hip_spread_move_add: f32,
    pub hip_spread_ducked_decay: f32,
    pub hip_spread_prone_decay: f32,
    pub ads_spread: f32,
    /// A turret's `playerSpread` (+0x7b0): its rounds' spread in degrees,
    /// both ends of it (cod4-effects.md 12.2).
    pub player_spread: f32,
    pub rifle_bullet: bool,
    pub armor_piercing: bool,
    pub bolt_action: bool,
    pub aim_down_sight: bool,
    pub rechamber_while_ads: bool,
    pub clip_only: bool,
    pub ads_fire_only: bool,
    /// `cancelAutoHolsterWhenEmpty` (+1288): an empty magazine neither
    /// clicks nor switches away (`CG_EntityEvent`'s EV_NOAMMO, `iw3mp.exe`
    /// 0x43633b).
    pub cancel_auto_holster_when_empty: bool,
    /// `inventoryType` (+316): 0 primary, 3 an alt mode (a grenade
    /// launcher).
    pub inventory_type: i32,
    pub no_partial_reload: bool,
    pub segmented_reload: bool,
    pub reload_ammo_add: i32,
    pub reload_start_add: i32,
    /// `altWeaponIndex`, set at the level's registration.
    pub alt_weapon: u32,
    pub has_detonator: bool,
    pub hold_button_to_throw: bool,
    pub freeze_movement_when_firing: bool,
    pub ads_gun_kick_reduced_kick_bullets: i32,
    pub hip_gun_kick_reduced_kick_bullets: i32,
    pub position_reload_trans_time: i32,
    /// The inverse sight times: 1 / `adsTransInTime` and 1 / `adsTransOutTime`.
    pub oo_pos_anim_length: [f32; 2],
    pub max_range: f32,
    pub min_damage: i32,
    pub min_player_damage: i32,
    pub max_damage_range: f32,
    pub min_damage_range: f32,
    pub location_damage: [f32; 19],
    /// The melee: `meleeDamage`, `meleeDelay`, `meleeTime` and whether
    /// a knife model is set (the knife's own raise after the swing).
    pub melee_damage: i32,
    pub melee_delay: i32,
    pub melee_time: i32,
    pub knife_model: bool,
    /// The charge's swing: `meleeChargeDelay`, `meleeChargeTime` and
    /// whether `meleeChargeAnim` (view anim 8) names an anim, which only
    /// [`WeaponDef::of`] reads (the raw struct holds a pointer).
    pub melee_charge_delay: i32,
    pub melee_charge_time: i32,
    pub melee_charge_anim: bool,
    /// `cookOffHold`: the fuse runs while the grenade is held.
    pub cook_off_hold: bool,
    /// The projectile: `explosionRadius`, `..RadiusMin`, `..InnerDamage`,
    /// `..OuterDamage`, `damageConeAngle`, `projectileSpeed`, `..SpeedUp`,
    /// `..SpeedForward`, `projectileActivateDist`, `projLifetime`,
    /// `projExplosion`, `projExplosionEffectForceNormalUp`,
    /// `projImpactExplode`, `stickiness`, `timedDetonation`, `rotate`,
    /// and the bounce factors by surface type.
    pub explosion_radius: i32,
    pub explosion_radius_min: i32,
    pub explosion_inner_damage: i32,
    pub explosion_outer_damage: i32,
    pub damage_cone_angle: f32,
    pub projectile_speed: i32,
    pub projectile_speed_up: i32,
    pub projectile_speed_forward: i32,
    pub projectile_activate_dist: i32,
    pub proj_lifetime: f32,
    pub proj_explosion: i32,
    pub proj_explosion_normal_up: bool,
    pub proj_impact_explode: bool,
    pub stickiness: i32,
    pub timed_detonation: bool,
    pub rotate: bool,
    pub parallel_bounce: [f32; 29],
    pub perpendicular_bounce: [f32; 29],
    /// `guidedMissileType`.
    pub guided_missile_type: i32,
    /// A rocket's flight (cod4-gsc.md 3.2, "Missiles under 1.7"):
    /// `timeToAccelerate`, `projectileCurvature`, `maxSteeringAccel`,
    /// `destabilizationRateTime`, `destabilizationCurvatureMax` and
    /// `destabilizeDistance`.
    pub time_to_accelerate: f32,
    pub projectile_curvature: f32,
    pub max_steering_accel: f32,
    pub destabilization_rate_time: f32,
    pub destabilization_curvature_max: f32,
    pub destabilize_distance: i32,
    /// What the shot's aim block reads (`super::aim`).
    pub aim: super::aim::AimKeys,
}

impl WeaponDef {
    /// The def off a zone weapon's raw struct; `None` for one shorter than
    /// 0x878 bytes.
    pub fn from_raw(name: &str, raw: &[u8]) -> Option<WeaponDef> {
        if raw.len() < 0x878 {
            return None;
        }
        let i = |o: usize| i32::from_le_bytes(raw[o..o + 4].try_into().unwrap());
        let f = |o: usize| f32::from_le_bytes(raw[o..o + 4].try_into().unwrap());
        let b = |o: usize| i(o) != 0;
        Some(WeaponDef {
            name: name.to_ascii_lowercase(),
            player_anim_type: i(296),
            weap_type: i(300),
            weap_class: i(304),
            penetrate_type: i(308),
            fire_type: i(320),
            offhand_class: i(324),
            ammo_index: 0,
            clip_index: 0,
            clip_size: i(824),
            max_ammo: i(820),
            shot_count: i(828),
            damage: i(844),
            player_damage: i(848),
            fire_delay: i(860),
            fire_time: i(876),
            rechamber_time: i(880),
            rechamber_bolt_time: i(884),
            hold_fire_time: i(888),
            reload_time: i(904),
            reload_empty_time: i(912),
            reload_add_time: i(916),
            reload_start_time: i(920),
            reload_start_add_time: i(924),
            reload_end_time: i(928),
            drop_time: i(932),
            raise_time: i(936),
            alt_drop_time: i(940),
            alt_raise_time: i(944),
            quick_drop_time: i(948),
            quick_raise_time: i(952),
            first_raise_time: i(956),
            empty_raise_time: i(960),
            empty_drop_time: i(964),
            sprint_in_time: i(968),
            sprint_out_time: i(976),
            fuse_time: i(1004),
            no_ads_when_mag_empty: b(1016),
            overlay_reticle: i(1080),
            hip_spread_stand_min: f(1104),
            hip_spread_ducked_min: f(1108),
            hip_spread_prone_min: f(1112),
            hip_spread_stand_max: f(1116),
            hip_spread_ducked_max: f(1120),
            hip_spread_prone_max: f(1124),
            hip_spread_decay_rate: f(1128),
            hip_spread_fire_add: f(1132),
            hip_spread_turn_add: f(1136),
            hip_spread_move_add: f(1140),
            hip_spread_ducked_decay: f(1144),
            hip_spread_prone_decay: f(1148),
            ads_spread: f(1820),
            player_spread: f(1968),
            rifle_bullet: b(1248),
            armor_piercing: b(1252),
            bolt_action: b(1256),
            aim_down_sight: b(1260),
            rechamber_while_ads: b(1264),
            clip_only: b(1280),
            ads_fire_only: b(1284),
            cancel_auto_holster_when_empty: b(1288),
            inventory_type: i(316),
            no_partial_reload: b(1324),
            segmented_reload: b(1328),
            reload_ammo_add: i(1332),
            reload_start_add: i(1336),
            alt_weapon: 0,
            has_detonator: b(1448),
            hold_button_to_throw: b(1460),
            freeze_movement_when_firing: b(1464),
            ads_gun_kick_reduced_kick_bullets: i(1752),
            hip_gun_kick_reduced_kick_bullets: i(1824),
            position_reload_trans_time: i(1940),
            oo_pos_anim_length: [f(2040), f(2044)],
            max_range: f(2000),
            min_damage: i(2048),
            min_player_damage: i(2052),
            max_damage_range: f(2056),
            min_damage_range: f(2060),
            location_damage: std::array::from_fn(|k| f(2076 + 4 * k)),
            melee_damage: i(852),
            melee_delay: i(864),
            melee_time: i(896),
            knife_model: i(772) != 0,
            melee_charge_delay: i(868),
            melee_charge_time: i(900),
            melee_charge_anim: false,
            cook_off_hold: b(1276),
            explosion_radius: i(1364),
            explosion_radius_min: i(1368),
            explosion_inner_damage: i(1372),
            explosion_outer_damage: i(1376),
            damage_cone_angle: f(1380),
            projectile_speed: i(1384),
            projectile_speed_up: i(1388),
            projectile_speed_forward: i(1392),
            projectile_activate_dist: i(1396),
            proj_lifetime: f(1400),
            proj_explosion: i(1416),
            proj_explosion_normal_up: b(1424),
            proj_impact_explode: b(1440),
            stickiness: i(1444),
            timed_detonation: b(1452),
            rotate: b(1456),
            parallel_bounce: std::array::from_fn(|k| f(1472 + 4 * k)),
            perpendicular_bounce: std::array::from_fn(|k| f(1588 + 4 * k)),
            guided_missile_type: i(1720),
            time_to_accelerate: f(1404),
            projectile_curvature: f(1408),
            max_steering_accel: f(1724),
            destabilization_rate_time: f(2064),
            destabilization_curvature_max: f(2068),
            destabilize_distance: i(2072),
            aim: super::aim::AimKeys::from_raw(raw),
        })
    }
}

impl WeaponDef {
    /// [`WeaponDef::from_raw`] of a loaded weapon, a zone's or a weapon
    /// file's, with what its strings say.
    pub fn of(w: &crate::assets::weapon::Weapon) -> Option<WeaponDef> {
        let mut d = WeaponDef::from_raw(&w.name, &w.raw)?;
        d.melee_charge_anim = w
            .xanims
            .get(8)
            .is_some_and(|a| a.as_ref().is_some_and(|a| !a.is_empty()));
        Some(d)
    }

    /// `PM_WeaponHasChargeMelee` (0x8063481): a charge anim and a
    /// `meleeChargeTime`.
    pub fn has_charge_melee(&self) -> bool {
        self.melee_charge_anim && self.melee_charge_time > 0
    }
}

/// The level's weapons by number, 0 the empty hand.
pub trait WeaponDefs {
    fn def(&self, weapon: u32) -> Option<&WeaponDef>;
    /// `BG_GetNumWeapons`: one past the last weapon number.
    fn count(&self) -> u32;
}

impl WeaponDefs for [WeaponDef] {
    fn def(&self, weapon: u32) -> Option<&WeaponDef> {
        self.get(weapon as usize).filter(|_| weapon > 0)
    }
    fn count(&self) -> u32 {
        self.len() as u32
    }
}

impl WeaponDefs for Vec<WeaponDef> {
    fn def(&self, weapon: u32) -> Option<&WeaponDef> {
        self.as_slice().def(weapon)
    }
    fn count(&self) -> u32 {
        self.len() as u32
    }
}

/// `player_burstFireCooldown` (0.2), `player_adsExitDelay` (0, see
/// [`ads_lerp`]),
/// `player_breath_fire_delay` (0), `bg_aimSpreadMoveSpeedThreshold` (11),
/// `player_scopeExitOnDamage` (0) and `player_sustainAmmo` (0): 1.7's
/// registered defaults (`tools/re/cod4_dvars.py` on `cod4_lnxded`,
/// VERIFIED).
const BURST_FIRE_COOLDOWN: f32 = 0.2;
const AIM_SPREAD_MOVE_SPEED_THRESHOLD: f32 = 11.0;

fn bit(bits: &[u32; 4], i: u32) -> bool {
    i < 128 && bits[(i / 32) as usize] >> (i % 32) & 1 != 0
}

fn set_bit(bits: &mut [u32; 4], i: u32, on: bool) {
    if i < 128 {
        let w = &mut bits[(i / 32) as usize];
        if on {
            *w |= 1 << (i % 32);
        } else {
            *w &= !(1 << (i % 32));
        }
    }
}

impl super::PlayerState {
    /// `PM_StartWeaponAnim`: the anim with the restart toggle flipped.
    pub fn start_weapon_anim(&mut self, anim: u32) {
        if self.pm_type < pm_type::DEAD {
            self.weap_anim = anim | ((self.weap_anim & 0x200) ^ 0x200);
        }
    }

    fn continue_weapon_anim(&mut self, anim: u32) {
        if self.weap_anim & !0x200 != anim {
            self.start_weapon_anim(anim);
        }
    }

    /// `ps.weapons` holds `w`.
    pub fn has_weapon(&self, w: u32) -> bool {
        bit(&self.weapons, w)
    }
}

fn clip(ps: &super::PlayerState, d: &WeaponDef) -> i32 {
    ps.ammoclip.get(d.clip_index).copied().unwrap_or(0)
}

fn reserve(ps: &super::PlayerState, d: &WeaponDef) -> i32 {
    ps.ammo.get(d.ammo_index).copied().unwrap_or(0)
}

/// The held weapon's def, or the empty hand's.
fn held<'d>(pm: &Pmove<'d>) -> Option<&'d WeaponDef> {
    pm.defs?.def(pm.ps.weapon)
}

/// `PM_WeaponAmmoAvailable`: the held weapon's clip.
pub fn ammo_available(ps: &super::PlayerState, defs: &dyn WeaponDefs) -> i32 {
    defs.def(ps.weapon).map_or(0, |d| clip(ps, d))
}

fn is_reloading(s: i32) -> bool {
    matches!(s, state::RELOAD..=state::RELOAD_CLOSE)
}

fn is_offhand(s: i32) -> bool {
    (state::OFFHAND_LOWER..=state::OFFHAND_RAISE).contains(&s)
}

fn is_melee(s: i32) -> bool {
    matches!(s, state::MELEE_SWING..=state::MELEE_RECOVER)
}

/// The hip spread's step (`PM_AdjustAimSpreadScale`, cod4-combat.md 2):
/// the scale (0..255) eases back by the weapon's decay rate, scaled by the
/// stance on the ground and halved in the air, and opens up with the
/// view's turn, a run past `bg_aimSpreadMoveSpeedThreshold` and the air;
/// through the sight it only eases back. A script's spread override
/// (state 1) eases back by the rate over the override's place in the
/// stance's range and ends once the scale times that place drops under
/// 255. The products stay on the x87 stack; the stance's rate, its range
/// place and the scale are stored floats.
pub(super) fn adjust_aim_spread(pm: &mut Pmove, pml: &Pml) {
    let Some(d) = held(pm) else {
        // The empty hand's def has no decay rate: the scale drops a whole
        // 255 a step (the mantle capture's 0 at 48850, the cmd after its
        // weapon went to 0, VERIFIED live).
        if pm.defs.is_some() {
            pm.ps.aim_spread_scale = (pm.ps.aim_spread_scale - 255.0).clamp(0.0, 255.0);
        }
        return;
    };
    let ft = f64::from(pml.frametime);
    let grounded =
        pm.ps.ground_entity_num != super::ENTITYNUM_NONE || pm.ps.pm_type == pm_type::NORMAL_LINKED;
    let overriding = pm.ps.spread_override_state == 1;
    let (rate, place) = spread_decay(pm.ps, d, grounded);
    // The change this step, in 255ths of the scale.
    let change = if rate == 0.0 {
        -1.0
    } else if overriding {
        -(rate * ft / f64::from(place))
    } else {
        let opened = if pm.ps.weapon_pos_frac == 1.0 {
            0.0
        } else {
            spread_growth(pm, d, ft, grounded) * ft
        };
        if opened > 0.0 { opened } else { -(rate * ft) }
    };
    let ps = &mut *pm.ps;
    let mut scale = (f64::from(ps.aim_spread_scale) + change * 255.0) as f32;
    if overriding && scale * place < 255.0 {
        ps.spread_override_state = 0;
        scale *= place;
    }
    ps.aim_spread_scale = scale.clamp(0.0, 255.0);
}

/// The hip spread's decay rate a second for the body's stance, stored to
/// a float, and where the spread override sits in that stance's
/// min..max (1 for a weapon that never decays). On the ground the prone
/// and crouched stances take their own decay and range; in the air the
/// standing range at half the rate.
fn spread_decay(ps: &super::PlayerState, d: &WeaponDef, grounded: bool) -> (f64, f32) {
    let base = f64::from(d.hip_spread_decay_rate);
    if base == 0.0 {
        return (0.0, 1.0);
    }
    let (scale, min, max) = match grounded {
        false => (0.5, d.hip_spread_stand_min, d.hip_spread_stand_max),
        true if ps.e_flags & ef::PRONE != 0 => (
            f64::from(d.hip_spread_prone_decay),
            d.hip_spread_prone_min,
            d.hip_spread_prone_max,
        ),
        true if ps.e_flags & ef::CROUCH != 0 => (
            f64::from(d.hip_spread_ducked_decay),
            d.hip_spread_ducked_min,
            d.hip_spread_ducked_max,
        ),
        true => (1.0, d.hip_spread_stand_min, d.hip_spread_stand_max),
    };
    let (min, max) = (f64::from(min), f64::from(max));
    let place = (f64::from(ps.spread_override) - min) / (max - min);
    (f64::from((base * scale) as f32), place as f32)
}

/// What opens the hip spread a second: the view's turn in pitch and yaw
/// since the last cmd (`hipSpreadTurnAdd` a hundredth of a degree), the
/// run with a move key down past the threshold (`hipSpreadMoveAdd` of the
/// flat speed over `ps.speed`), and 2.56 in the air.
fn spread_growth(pm: &Pmove, d: &WeaponDef, ft: f64, grounded: bool) -> f64 {
    // A cmd angle's short in degrees.
    let deg = |s: u16| (f64::from(s as i16) * 0.0054931640625) as f32;
    let hundredth = f64::from(0.01f32);
    let mut grow = 0.0f64;
    if d.hip_spread_turn_add != 0.0 {
        for axis in 0..2 {
            let turn = angle_delta(deg(pm.cmd.angles[axis]), deg(pm.oldcmd.angles[axis])).abs();
            grow += f64::from(turn) * hundredth * f64::from(d.hip_spread_turn_add) / ft;
        }
    }
    let moving = pm.cmd.forward_move != 0 || pm.cmd.right_move != 0;
    if d.hip_spread_move_add != 0.0 && moving {
        let v = pm.ps.velocity;
        let flat_sq = (f64::from(v.y) * f64::from(v.y) + f64::from(v.x) * f64::from(v.x)) as f32;
        let floor = f64::from(AIM_SPREAD_MOVE_SPEED_THRESHOLD);
        if f64::from(flat_sq) > floor * floor {
            grow += f64::from(d.hip_spread_move_add) * f64::from(flat_sq.sqrt())
                / f64::from(pm.ps.speed);
        }
    }
    if !grounded {
        grow += 2.0 * (hundredth * 128.0);
    }
    grow
}

/// `AngleDelta`: `a1 - a2` into -180..180.
fn angle_delta(a1: f32, a2: f32) -> f32 {
    let d = f64::from(a1) - f64::from(a2);
    let d = d - 360.0 * (d * (1.0 / 360.0) + 0.5).floor();
    d as f32
}

/// The weapon states the sight never shows in: the offhand, the melee,
/// the raise and drop, and night vision going on or off.
fn sight_barred_state(st: i32) -> bool {
    is_offhand(st)
        || is_melee(st)
        || (state::RAISE..=state::LOWER_QUICK).contains(&st)
        || matches!(st, state::NVG_ON | state::NVG_OFF)
}

/// Whether the sight may come up (`PM_IsAdsAllowed`, cod4-combat.md 2): a
/// player in the world (`POF_PLAYER`) walking or linked to something with
/// ground under him, holding a weapon that sights, off a turret, not
/// barred by the script (`weapFlags` 0x20), in a state that shows it, and
/// with a round where the weapon wants one (`noADSWhenMagEmpty`).
fn ads_allowed(pm: &Pmove, pml: &Pml) -> bool {
    let ps = &*pm.ps;
    let body_ok = match ps.pm_type {
        pm_type::NORMAL_LINKED => !pml.almost_ground_plane,
        pm_type::NOCLIP
        | pm_type::UFO
        | pm_type::SPECTATOR
        | pm_type::INTERMISSION
        | pm_type::DEAD
        | pm_type::DEAD_LINKED => false,
        _ => true,
    };
    let Some(d) = held(pm) else { return false };
    body_ok
        && ps.other_flags & super::POF_PLAYER != 0
        && d.aim_down_sight
        && !sight_barred_state(ps.weapon_state)
        && ps.e_flags & ef::TURRET == 0
        && ps.weap_flags & wf::NO_ADS == 0
        && !(d.no_ads_when_mag_empty && clip(ps, d) == 0)
}

/// `PM_ExitAimDownSight`.
fn exit_ads(ps: &mut super::PlayerState) {
    ps.add_event(ev::RESET_ADS, 0);
    ps.pm_flags &= !pmf::SIGHT;
}

/// The sight flag for this cmd (`PM_UpdateAimDownSightFlag`, run on every
/// `PmoveSingle` arm before the sprint): set while the held sight key
/// (0x800) asks and [`ads_allowed`] lets it. The sprint key drops the
/// sight (`EV_RESET_ADS` each cmd) unless a scope's breath key (0x2000) is
/// down with it. Prone, a sight that is not a scope part way up comes up
/// on the key's press or a still body, and holds the prone body still.
pub(super) fn update_ads_flag(pm: &mut Pmove, pml: &Pml) {
    let Some(defs) = pm.defs else { return };
    pm.ps.pm_flags &= !pmf::SIGHT;
    let scope = defs
        .def(pm.ps.weapon)
        .is_some_and(|d| d.overlay_reticle != 0);
    let keys = pm.cmd.buttons;
    let sprint_out = keys & button::SPRINT != 0 && !(scope && keys & 0x2000 != 0);
    if sprint_out {
        exit_ads(pm.ps);
    }
    if sprint_out || keys & button::ADS_MODE == 0 || !ads_allowed(pm, pml) {
        return;
    }
    if pm.ps.pm_flags & pmf::PRONE == 0 || using_sniper_scope(pm) {
        pm.ps.pm_flags |= pmf::SIGHT;
        return;
    }
    let pressed = pm.oldcmd.buttons & button::ADS_MODE == 0;
    let still = pm.cmd.forward_move == 0 && pm.cmd.right_move == 0;
    if pressed || still {
        pm.ps.pm_flags |= pmf::SIGHT | pmf::PRONE_STILL;
    }
}

/// `BG_UsingSniperScope`: a scope weapon on its way into the sight.
pub(super) fn using_sniper_scope(pm: &Pmove) -> bool {
    held(pm).is_some_and(|d| d.overlay_reticle != 0) && pm.ps.weapon_pos_frac > 0.0
}

/// Whether a reload or a rechamber holds the gun down out of the sight: a
/// plain reload until its last `adsReloadTransTime` ms, a segmented
/// one through its start and rounds and until that point of its end, and
/// a rechamber on a weapon without `rechamberWhileAds`.
fn sight_lowered(ps: &super::PlayerState, d: &WeaponDef) -> bool {
    let st = ps.weapon_state;
    let before_trans = ps.weapon_time - d.position_reload_trans_time > 0;
    let reload_down = if d.segmented_reload {
        (state::RELOAD..=state::RELOAD_OPEN_CUT).contains(&st)
            || (st == state::RELOAD_CLOSE && before_trans)
    } else {
        st == state::RELOAD && before_trans
    };
    reload_down || (st == state::RECHAMBER && !d.rechamber_while_ads)
}

/// `fWeaponPosFrac`'s step (`PM_UpdateAimDownSightLerp`): toward 1 at the
/// def's in rate while the sight flag is up and nothing lowers the gun
/// (an `adsFireOnly` weapon's delayed shot raises it whatever the flag),
/// toward 0 at the out rate otherwise, clamped to 0..1. Without a sighting
/// weapon, or on a turret, it reads 0. `player_adsExitDelay`'s default 0
/// leaves `adsDelayTime` at 0.
fn ads_lerp(pm: &mut Pmove, pml: &Pml) {
    let d = held(pm).filter(|d| d.aim_down_sight && pm.ps.e_flags & ef::TURRET == 0);
    let ps = &mut *pm.ps;
    ps.ads_delay_time = 0;
    let Some(d) = d else {
        ps.weapon_pos_frac = 0.0;
        return;
    };
    let fire_only = d.ads_fire_only && ps.weapon_delay != 0 && ps.weapon_state == state::FIRE;
    let up = fire_only || (ps.pm_flags & pmf::SIGHT != 0 && !sight_lowered(ps, d));
    let goal = if up { 1.0 } else { 0.0 };
    if ps.weapon_pos_frac == goal {
        return;
    }
    let step = f64::from(pml.msec) * f64::from(d.oo_pos_anim_length[usize::from(!up)]);
    let frac = f64::from(ps.weapon_pos_frac);
    let next = if up { frac + step } else { frac - step };
    ps.weapon_pos_frac = (next as f32).clamp(0.0, 1.0);
}

/// How the trigger meters rounds, off `weapFireType_t` (0 automatic, 1
/// single, 2..4 a burst of that many; cod4-combat.md 2).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Trigger {
    Auto,
    Single,
    Burst(i32),
}

impl WeaponDef {
    fn trigger(&self) -> Trigger {
        match self.fire_type {
            FIRETYPE_SINGLESHOT => Trigger::Single,
            n @ FIRETYPE_BURSTFIRE2..=FIRETYPE_BURSTFIRE4 => Trigger::Burst(n),
            _ => Trigger::Auto,
        }
    }

    fn fires_bursts(&self) -> bool {
        matches!(self.trigger(), Trigger::Burst(_))
    }
}

/// The pull's rounds are all out (`weaponShotCount` counts them): a
/// single shot after one, a burst after its count, never an automatic.
fn pull_spent(ps: &super::PlayerState, d: &WeaponDef) -> bool {
    let n = ps.weapon_shot_count;
    match d.trigger() {
        Trigger::Auto => false,
        Trigger::Single => n > 0,
        Trigger::Burst(rounds) => n >= rounds,
    }
}

/// A burst has fired some of its rounds and owes the rest: the cmd's
/// requests (sprint, change, reload, melee, offhand) wait for it and the
/// trigger needs no key (the shooter's `weaponShotCount` 1..3, cod4-combat.md 2).
fn mid_burst(ps: &super::PlayerState, d: Option<&WeaponDef>) -> bool {
    let n = ps.weapon_shot_count;
    d.is_some_and(|d| matches!(d.trigger(), Trigger::Burst(rounds) if n > 0 && n < rounds))
}

/// The gun at rest: no offhand on the view, the prone move given back, both
/// clocks cleared, ready with its idle anim restarted. Where a melee, an
/// offhand throw or a sprint's lowered gun ends.
fn settle_ready(ps: &mut super::PlayerState) {
    ps.weap_flags &= !wf::USING_OFFHAND;
    ps.pm_flags &= !pmf::PRONE_STILL;
    (ps.weapon_time, ps.weapon_delay) = (0, 0);
    ps.weapon_state = state::READY;
    ps.start_weapon_anim(0);
}

/// Ready on the idle anim the gun already plays (a shot's or a rechamber's
/// end), against [`settle_ready`]'s restart.
fn back_to_ready(ps: &mut super::PlayerState) {
    ps.continue_weapon_anim(0);
    ps.weapon_state = state::READY;
}

/// A weapon action begun while prone holds the prone body still
/// (`pm_flags` 0x200) until the gun settles.
fn pin_prone(ps: &mut super::PlayerState) {
    if ps.pm_flags & pmf::PRONE != 0 {
        ps.pm_flags |= pmf::PRONE_STILL;
    }
}

/// `perk_weapReloadMultiplier` (0.5) and `perk_weapRateMultiplier` (0.75),
/// 1.7's defaults (`tools/re/cod4_dvars.py`, VERIFIED).
const PERK_RELOAD_MULTIPLIER: f32 = 0.5;
const PERK_RATE_MULTIPLIER: f32 = 0.75;
/// Sleight of Hand and Double Tap, the `perks` bits 0x4 and 0x8 the clock
/// reads (cod4-combat.md 2.1).
const PERK_FAST_RELOAD: u32 = 0x4;
const PERK_FAST_FIRE: u32 = 0x8;

/// A perk's step: `msec / multiplier` rounded half up, as 1.7 computes it
/// (`floor(x + 0.5)`, `cod4_lnxded` 0x80645c8; cod4-combat.md 2.1).
fn perk_msec(msec: i32, multiplier: f32) -> i32 {
    (f64::from(msec) / f64::from(multiplier) + 0.5).floor() as i32
}

/// The milliseconds this cmd takes off both weapon clocks: a reload's run
/// faster under Sleight of Hand, a shot's and a rechamber's under Double
/// Tap (cod4-combat.md 2.1; the `doubletap` rows).
fn clock_step(ps: &super::PlayerState, msec: i32) -> i32 {
    let (perk, multiplier) = match ps.weapon_state {
        s if is_reloading(s) => (PERK_FAST_RELOAD, PERK_RELOAD_MULTIPLIER),
        state::FIRE | state::RECHAMBER => (PERK_FAST_FIRE, PERK_RATE_MULTIPLIER),
        _ => return msec,
    };
    if ps.perks & perk != 0 {
        perk_msec(msec, multiplier)
    } else {
        msec
    }
}

/// Runs a clock down by `step`; true on the cmd it reaches 0. What it
/// overshoots is dropped, so the next round waits whole cmds
/// (cod4-combat.md 2.1, 0x8064592 and 0x80645b5).
fn run_down(clock: &mut i32, step: i32) -> bool {
    if *clock == 0 {
        return false;
    }
    *clock = (*clock - step).max(0);
    *clock == 0
}

/// The cmd's clocks (`PM_Weapon_WeaponTimeAdjust`, inlined in `PM_Weapon`
/// at `cod4_lnxded` 0x8062df0): the kick window on the plain msec, then
/// `weaponTime` and `weaponDelay` on [`clock_step`]. True when the delay
/// ran out this cmd: a delayed shot, a bolt's brass or a reload's rounds
/// land now.
fn run_clocks(pm: &mut Pmove, pml: &Pml) -> bool {
    let ps = &mut *pm.ps;
    if ps.weapon_restrict_kick_time > 0 {
        ps.weapon_restrict_kick_time = (ps.weapon_restrict_kick_time - pml.msec).max(0);
    }
    let step = clock_step(ps, pml.msec);
    if run_down(&mut pm.ps.weapon_time, step) && !weapon_time_out(pm) {
        return false;
    }
    run_down(&mut pm.ps.weapon_delay, step)
}

/// `player_burstFireCooldown` in ms: the gun stays ready but shut after a
/// burst (cod4-combat.md 2, the `shooter` rows' 0xc8).
fn burst_cooldown_ms() -> i32 {
    (f64::from(BURST_FIRE_COOLDOWN) * 1000.0).round_ties_even() as i32
}

/// `weaponTime` just ran out. A finished burst starts its cooldown and
/// returns false: the delay is not stepped on that cmd. A trigger still
/// held on a spent pull (or on a hold-to-throw grenade) keeps the gun shut
/// a millisecond at a time and takes it out of its shot, rechamber or
/// melee; otherwise a released (or freshly pulled) trigger ends the pull.
fn weapon_time_out(pm: &mut Pmove) -> bool {
    let d = held(pm);
    let cmd = pm.cmd;
    let ps = &mut *pm.ps;
    let st = ps.weapon_state;
    if let Some(d) = d
        && st == state::FIRE
        && d.fires_bursts()
        && !mid_burst(ps, Some(d))
    {
        ps.weapon_time = burst_cooldown_ms();
        back_to_ready(ps);
        return false;
    }
    let attack = cmd.buttons & button::ATTACK != 0;
    let fresh_pull = ps.weap_flags & wf::TRIGGER_PULLED != 0;
    let held_shut = d.is_some_and(|d| {
        let waits_for_release = (pull_spent(ps, d) && !fresh_pull)
            || (d.weap_type == WEAPTYPE_GRENADE && d.hold_button_to_throw);
        waits_for_release
            && attack
            && !is_offhand(st)
            && ps.weapon == u32::from(cmd.weapon)
            && clip(ps, d) != 0
    });
    if !held_shut {
        if (!attack || fresh_pull) && !mid_burst(ps, d) {
            ps.weapon_shot_count = 0;
        }
        return true;
    }
    if is_reloading(st) {
        ps.weapon_shot_count = 0;
        return true;
    }
    ps.weapon_time = 1;
    if matches!(st, state::FIRE | state::RECHAMBER) || is_melee(st) {
        back_to_ready(ps);
    }
    true
}

/// A burst weapon's attack key going down marks a fresh pull
/// (`weapFlags` 0x100, cod4-combat.md 2), which may cut a cooldown's wait.
fn note_trigger_pull(pm: &mut Pmove) {
    let pressed = pm.cmd.buttons & !pm.oldcmd.buttons & button::ATTACK != 0;
    if pressed && held(pm).is_some_and(WeaponDef::fires_bursts) {
        pm.ps.weap_flags |= wf::TRIGGER_PULLED;
    }
}

/// The gun and the sprint (`PM_Weapon_CheckForSprint`): the sprint's start
/// lowers it over the def's `sprintInTime` (anim 23), its end brings it
/// back over `sprintOutTime` (anim 25). A gun moving, firing, rechambering,
/// in a melee, the offhand or night vision is left alone; a reload is not,
/// and the sprint cuts it short (`weapon_pins`' `SPRINT`).
fn sprint_request(pm: &mut Pmove) {
    let Some(d) = held(pm) else { return };
    let ps = &mut *pm.ps;
    let st = ps.weapon_state;
    let busy = matches!(st, state::RAISE..=state::RECHAMBER)
        || is_melee(st)
        || is_offhand(st)
        || matches!(st, state::NVG_ON | state::NVG_OFF);
    if pm.cmd.weapon == 0 || busy {
        return;
    }
    let sprinting = ps.pm_flags & pmf::SPRINTING != 0;
    let (next, time, anim) = match st {
        state::SPRINT_IN..=state::SPRINT_OUT if sprinting => return,
        _ if sprinting => (state::SPRINT_IN, d.sprint_in_time, 23),
        state::SPRINT_IN | state::SPRINT_HOLD => (state::SPRINT_OUT, d.sprint_out_time, 25),
        _ => return,
    };
    ps.weapon_state = next;
    (ps.weapon_time, ps.weapon_delay) = (time, 0);
    ps.start_weapon_anim(anim);
}

/// What the cmd's weapon byte asks of the hand this cmd.
enum Change {
    /// Nothing moves.
    Stay,
    /// The weapon going away is the one asked for again: it comes back up.
    Recall,
    /// Put the held weapon away for `.0`, quick when `.1`.
    To(u32, bool),
}

/// `PM_Weapon_CheckForChangeWeapon`: the gun may change outside a melee,
/// the offhand and night vision, once its clocks allow (a reload and a
/// rechamber always may, a shot never before its time). A mantle on a high
/// row, the ladder and `disableWeapons` take it away (the first two
/// quick, cod4-pmove.md 16); otherwise the cmd's byte names the weapon, and
/// the first change after a mantle is quick (`mantleState.flags` 0x10,
/// cod4-combat.md 2).
fn change_request(pm: &mut Pmove) {
    let Some(defs) = pm.defs else { return };
    let wanted = u32::from(pm.cmd.weapon);
    let high_mantle = super::mantle::weapon_inactive(pm.ps, &pm.rules);
    let ps = &mut *pm.ps;
    let st = ps.weapon_state;
    if is_melee(st) || is_offhand(st) || matches!(st, state::NVG_ON | state::NVG_OFF) {
        return;
    }
    let clocks_allow = match st {
        _ if ps.weapon_time == 0 => true,
        state::RECHAMBER => true,
        s if is_reloading(s) => true,
        state::FIRE => false,
        _ => ps.weapon_delay == 0,
    };
    if !clocks_allow {
        return;
    }
    let ladder = ps.pm_flags & pmf::LADDER != 0;
    let change = if high_mantle || ladder || ps.weap_flags & wf::DISABLED != 0 {
        if ps.weapon == 0 {
            Change::Stay
        } else {
            Change::To(0, high_mantle || ladder)
        }
    } else if ps.weapon == wanted {
        if matches!(st, state::LOWER | state::LOWER_QUICK) {
            Change::Recall
        } else {
            Change::Stay
        }
    } else {
        let pinned = ps.pm_flags & (pmf::RESPAWNED | pmf::FROZEN) != 0 && ps.weapon != 0;
        let unusable = wanted != 0 && (wanted >= defs.count() || !ps.has_weapon(wanted));
        if pinned || unusable {
            Change::Stay
        } else {
            Change::To(wanted, ps.mantle.flags & super::mantle::flag::ENDED != 0)
        }
    };
    match change {
        Change::Recall => {
            settle_ready(ps);
            ps.start_weapon_anim(1);
        }
        Change::To(next, quick) => put_away(ps, defs, next, quick),
        // A weapon the server took from the hand goes, plainly.
        Change::Stay if ps.weapon != 0 && !ps.has_weapon(ps.weapon) => {
            put_away(ps, defs, 0, false);
        }
        Change::Stay => {}
    }
}

/// `PM_BeginWeaponChange` (`cod4_lnxded` 0x805fd22): the held weapon goes
/// down on its way to `next` (unowned: no change; already going down:
/// no restart). A pistol always comes quick. A reload's sound stops
/// (`EV_STOP_WEAPON_SOUND`, its state the parm); the drop is the alt
/// switch's when `next` is the held weapon's alt (`EV_WEAPON_ALT`, anim
/// 17), else `EV_PUTAWAY_WEAPON` with the empty, quick or plain drop (21,
/// 19, 10; no anim while sprinting), each its own def time. With nothing
/// owned in hand, or a cooking grenade, the drop takes no time.
fn put_away(ps: &mut super::PlayerState, defs: &dyn WeaponDefs, next: u32, quick: bool) {
    let going_down = matches!(ps.weapon_state, state::LOWER | state::LOWER_QUICK);
    if (next != 0 && !ps.has_weapon(next)) || going_down {
        return;
    }
    let quick = quick
        || defs
            .def(next)
            .is_some_and(|d| d.weap_class == WEAPCLASS_PISTOL);
    if is_reloading(ps.weapon_state) {
        ps.add_event(ev::STOP_WEAPON_SOUND, ps.weapon_state as u32);
    }
    let sprinting = ps.pm_flags & pmf::SPRINTING != 0;
    let drop_time = match defs.def(ps.weapon) {
        Some(od) if ps.has_weapon(ps.weapon) && ps.grenade_time_left <= 0 => {
            let (event, anim, time) = if !sprinting && next != 0 && od.alt_weapon == next {
                (ev::WEAPON_ALT, Some(17), od.alt_drop_time)
            } else {
                let (anim, time) = if clip(ps, od) == 0 {
                    (21, od.empty_drop_time)
                } else if quick {
                    (19, od.quick_drop_time)
                } else {
                    (10, od.drop_time)
                };
                (ev::PUTAWAY_WEAPON, (!sprinting).then_some(anim), time)
            };
            ps.add_event(event, 0);
            if let Some(anim) = anim {
                ps.start_weapon_anim(anim);
            }
            time
        }
        _ => 0,
    };
    ps.grenade_time_left = 0;
    (ps.weapon_time, ps.weapon_delay) = (drop_time, 0);
    ps.weapon_state = if quick {
        state::LOWER_QUICK
    } else {
        state::LOWER
    };
    pin_prone(ps);
}

/// The old weapon is down (`PM_Weapon_FinishWeaponChange`): the cmd's byte
/// is taken up, or the empty hand where the change would have refused it.
/// The weapon already in hand just goes ready. A new one rises by the alt
/// switch (anim 18, its sway kept at least 128) or by the empty, first
/// (`weaponold` not yet set for it), quick or plain raise (22, 12, 20, 11,
/// sway reset to 255) with `EV_FIRST_RAISE_WEAPON` or `EV_RAISE_WEAPON`
/// when something was in hand. The old weapon, a clip-only one with
/// nothing left and no detonator, is then taken (`BG_TakeClipOnlyWeaponIfEmpty`).
fn bring_up(pm: &mut Pmove, quick: bool) {
    let Some(defs) = pm.defs else { return };
    let wanted = u32::from(pm.cmd.weapon);
    let high_mantle = super::mantle::weapon_inactive(pm.ps, &pm.rules);
    let ps = &mut *pm.ps;
    let barred = high_mantle
        || ps.pm_flags & pmf::LADDER != 0
        || ps.weap_flags & wf::DISABLED != 0
        || wanted >= defs.count()
        || !ps.has_weapon(wanted);
    let new = if barred { 0 } else { wanted };
    let old = std::mem::replace(&mut ps.weapon, new);
    if old == new {
        restart_ready(ps);
        return;
    }
    let first = !bit(&ps.weaponold, new);
    set_bit(&mut ps.weaponold, new, true);
    let empty_hand;
    let d = match defs.def(new) {
        Some(d) => d,
        None => {
            empty_hand = WeaponDef::default();
            &empty_hand
        }
    };
    let alt = ps.pm_flags & pmf::SPRINTING == 0
        && old != 0
        && new != 0
        && defs.def(old).is_some_and(|od| od.alt_weapon == new);
    if alt {
        ps.weapon_state = state::RAISE_ALT;
        ps.weapon_time = d.alt_raise_time;
        ps.aim_spread_scale = ps.aim_spread_scale.max(128.0);
        ps.start_weapon_anim(18);
    } else {
        let (anim, time) = [
            (clip(ps, d) == 0, 22, d.empty_raise_time),
            (first, 12, d.first_raise_time),
            (quick, 20, d.quick_raise_time),
            (true, 11, d.raise_time),
        ]
        .into_iter()
        .find_map(|(when, anim, time)| when.then_some((anim, time)))
        .unwrap_or_default();
        if old != 0 {
            let event = if first {
                ev::FIRST_RAISE_WEAPON
            } else {
                ev::RAISE_WEAPON
            };
            ps.add_event(event, 0);
        }
        ps.weapon_state = state::RAISE;
        ps.weapon_time = time;
        ps.aim_spread_scale = 255.0;
        ps.start_weapon_anim(anim);
    }
    pin_prone(ps);
    let spent = defs.def(old).is_some_and(|od| {
        od.clip_only && !od.has_detonator && clip(ps, od) == 0 && reserve(ps, od) == 0
    });
    if spent && ps.has_weapon(old) {
        set_bit(&mut ps.weapons, old, false);
    }
}

/// `usercmd.buttons` 0x10, the reload key.
pub const RELOAD_BUTTON: u32 = 0x10;

/// How far into a segmented reload's start the trigger may cut it: past
/// this fraction of `reloadStartTime` the start ends early (state 10).
/// The float at `cod4_lnxded` 0x82095bc, compared at 0x8061682 against
/// `(start - weaponTime) / start`, strictly greater; VERIFIED (the
/// disassembly). `weapon_pins`' `INTERRUPT` puts a start exactly on it.
const START_CUT_FRACTION: f32 = 0.4;

fn in_reload_start(s: i32) -> bool {
    matches!(s, state::RELOAD_OPEN | state::RELOAD_OPEN_CUT)
}

/// The trigger's mark on a reload state: the start's and the loop's
/// interrupted twins.
fn interrupted(s: i32) -> i32 {
    if in_reload_start(s) {
        state::RELOAD_OPEN_CUT
    } else {
        state::RELOAD_CUT
    }
}

/// `PM_Weapon_AllowReload`: a reload has rounds to take and room to put
/// them. A no-partial weapon also needs room for its whole `reloadAmmoAdd`
/// (when that is less than a clip) or else an empty clip.
fn may_reload(ps: &super::PlayerState, d: &WeaponDef) -> bool {
    let room = d.clip_size - clip(ps, d);
    if reserve(ps, d) == 0 || room <= 0 {
        return false;
    }
    match d.reload_ammo_add {
        _ if !d.no_partial_reload => true,
        add if add != 0 && add < d.clip_size => room >= add,
        _ => room == d.clip_size,
    }
}

/// `PM_Weapon_CheckForReload` (`cod4_lnxded` 0x806148e). Outside the
/// offhand and a melee: the trigger's edge cuts a segmented reload (its
/// start only past [`START_CUT_FRACTION`]); the reload key (0x10) or the
/// script's flag (consumed here) starts a reload the clip has room for,
/// and an empty clip with a reserve starts one unasked except mid-shot
/// and in the sprint (cod4-combat.md 2: the shooter's 58900). A gun
/// moving or already reloading starts nothing.
fn reload_request(pm: &mut Pmove) {
    let Some(d) = held(pm) else { return };
    let pulled = pm.cmd.buttons & !pm.oldcmd.buttons & button::ATTACK != 0;
    let key = pm.cmd.buttons & RELOAD_BUTTON != 0;
    let ps = &mut *pm.ps;
    let st = ps.weapon_state;
    if is_offhand(st) || is_melee(st) {
        return;
    }
    let scripted = ps.weap_flags & wf::RELOAD != 0;
    ps.weap_flags &= !wf::RELOAD;
    if d.segmented_reload && pulled {
        let cut = match st {
            state::RELOAD => true,
            state::RELOAD_OPEN if d.reload_start_time != 0 => {
                let start = f64::from(d.reload_start_time);
                (start - f64::from(ps.weapon_time)) / start > f64::from(START_CUT_FRACTION)
            }
            _ => false,
        };
        if cut {
            ps.weapon_state = interrupted(st);
        }
    }
    let st = ps.weapon_state;
    if matches!(st, state::RAISE..=state::LOWER_QUICK) || is_reloading(st) {
        return;
    }
    let unasked = clip(ps, d) == 0
        && reserve(ps, d) != 0
        && st != state::FIRE
        && !(state::SPRINT_IN..=state::SPRINT_OUT).contains(&st);
    if unasked || ((key || scripted) && may_reload(ps, d)) {
        start_reload(ps, d);
    }
}

/// `PM_BeginWeaponReload`: from ready, a shot, a rechamber or the sprint,
/// holding a weapon. The burst count clears and the sight resets
/// (`EV_RESET_ADS`, then `EV_RELOAD_START_NOTIFY`); a segmented weapon
/// with a start plays it (anim 15, `EV_RELOAD_START`), any other goes
/// straight to its first segment.
fn start_reload(ps: &mut super::PlayerState, d: &WeaponDef) {
    let from = ps.weapon_state;
    let idle = matches!(from, state::READY | state::FIRE | state::RECHAMBER)
        || (state::SPRINT_IN..=state::SPRINT_OUT).contains(&from);
    if !idle || ps.weapon == 0 {
        return;
    }
    ps.weapon_shot_count = 0;
    ps.add_event(ev::RESET_ADS, 0);
    ps.add_event(ev::RELOAD_START_NOTIFY, 0);
    if !d.segmented_reload || d.reload_start_time == 0 {
        load_segment(ps, d);
        return;
    }
    ps.weapon_state = state::RELOAD_OPEN;
    ps.weapon_time = d.reload_start_time;
    ps.start_weapon_anim(15);
    ps.add_event(ev::RELOAD_START, 0);
    arm_add_delay(ps, d);
}

/// A clip with rounds in it reloads (anim 13, `reloadTime`,
/// `EV_RELOAD`); an empty one, a bullet weapon's only, from empty (14,
/// `reloadEmptyTime`, `EV_RELOAD_FROM_EMPTY`; cod4-combat.md 2's 58900).
fn reloads_partial(ps: &super::PlayerState, d: &WeaponDef) -> bool {
    clip(ps, d) != 0 || d.weap_type != 0
}

/// `PM_SetReloadingState` (`cod4_lnxded` 0x805fb7a): one reload segment,
/// interrupted if its start was.
fn load_segment(ps: &mut super::PlayerState, d: &WeaponDef) {
    let (anim, time, event) = if reloads_partial(ps, d) {
        (13, d.reload_time, ev::RELOAD)
    } else {
        (14, d.reload_empty_time, ev::RELOAD_FROM_EMPTY)
    };
    ps.start_weapon_anim(anim);
    ps.weapon_time = time;
    ps.add_event(event, 0);
    ps.weapon_state = if ps.weapon_state == state::RELOAD_OPEN_CUT {
        state::RELOAD_CUT
    } else {
        state::RELOAD
    };
    arm_add_delay(ps, d);
}

/// When into the segment its rounds go in. A start's is its
/// `reloadStartAddTime` within its `reloadStartTime`, `None` when it has
/// none; a segment's its `reloadAddTime` when shorter than its time (the
/// M16's 1500 of 2365, cod4-combat.md 2), else the whole time.
fn add_moment(ps: &super::PlayerState, d: &WeaponDef) -> Option<i32> {
    if in_reload_start(ps.weapon_state) {
        return (d.reload_start_add_time != 0)
            .then(|| d.reload_start_add_time.min(d.reload_start_time));
    }
    let time = if reloads_partial(ps, d) {
        d.reload_time
    } else {
        d.reload_empty_time
    };
    Some(match d.reload_add_time {
        add if add != 0 && add < time => add,
        _ => time,
    })
}

/// A bolt still to be worked before its rounds (`weaponrechamber`'s bit).
fn unchambered(ps: &super::PlayerState, d: &WeaponDef) -> bool {
    d.bolt_action && bit(&ps.weaponrechamber, ps.weapon)
}

/// `PM_SetWeaponReloadAddAmmoDelay`: `weaponDelay` runs to the add moment.
/// An unchambered bolt's runs to its brass instead, `rechamberBoltTime`
/// at most (the segment's time if it adds nothing), never under 1. No add
/// moment otherwise leaves the delay alone.
fn arm_add_delay(ps: &mut super::PlayerState, d: &WeaponDef) {
    let moment = add_moment(ps, d).unwrap_or(0);
    if unchambered(ps, d) {
        let until = if moment == 0 { ps.weapon_time } else { moment };
        ps.weapon_delay = until.min(d.rechamber_bolt_time).max(1);
    } else if moment != 0 {
        ps.weapon_delay = moment;
    }
}

/// `PM_Weapon_ReloadDelayedAction`: a reload's delay ran out. An
/// unchambered bolt ejects its brass (`EV_EJECT_BRASS`) and, while the
/// segment still runs, waits again for the rest of the add moment past
/// `rechamberBoltTime` (when that is at least a millisecond); otherwise
/// the rounds go in now.
fn reload_delay_out(ps: &mut super::PlayerState, d: &WeaponDef) {
    if unchambered(ps, d) {
        set_bit(&mut ps.weaponrechamber, ps.weapon, false);
        ps.add_event(ev::EJECT_BRASS, 0);
        if let Some(moment) = add_moment(ps, d).filter(|_| ps.weapon_time != 0) {
            let bolt = if d.rechamber_bolt_time >= moment {
                1
            } else {
                d.rechamber_bolt_time
            };
            if moment - bolt >= 1 {
                ps.weapon_delay = moment - bolt;
                return;
            }
        }
    }
    top_up_clip(ps, d);
}

/// `PM_ReloadClip`: rounds from the reserve into the clip, what fits and
/// the reserve holds, a start's `reloadStartAdd` or a segment's
/// `reloadAmmoAdd` at most when less than a clip; `EV_RELOAD_ADDAMMO` if
/// any moved. A start adding no rounds moves none.
fn top_up_clip(ps: &mut super::PlayerState, d: &WeaponDef) {
    let start = in_reload_start(ps.weapon_state);
    if start && d.reload_start_add == 0 {
        return;
    }
    let share = if start {
        d.reload_start_add
    } else {
        d.reload_ammo_add
    };
    let capped = share != 0 && share < d.clip_size;
    let (Some(&have), Some(&loaded)) = (ps.ammo.get(d.ammo_index), ps.ammoclip.get(d.clip_index))
    else {
        return;
    };
    let mut n = have.min(d.clip_size - loaded);
    if capped {
        n = n.min(share);
    }
    if n != 0 {
        ps.ammo[d.ammo_index] -= n;
        ps.ammoclip[d.clip_index] += n;
        ps.add_event(ev::RELOAD_ADDAMMO, 0);
    }
}

/// A reload state's clocks are out (`PM_Weapon_FinishReloadStart` for 9
/// and 10, `PM_Weapon_FinishReload` for 7 and 8). The delay running out
/// first does [`reload_delay_out`]; the segment then waits for its time.
/// A held trigger marks a segmented reload interrupted. Another segment
/// follows while [`may_reload`]: after a start unless it was interrupted
/// with rounds already in, after a segment only on a segmented weapon not
/// interrupted. Otherwise a start or a segmented weapon plays its
/// `reloadEndTime` (anim 16, `EV_RELOAD_END`) when it has one, and the
/// gun goes ready. Each end, and every segment's, clears the bolt's
/// chamber bit; a start's next segment keeps it for its own brass.
fn segment_done(pm: &mut Pmove, delay_done: bool) {
    let Some(d) = held(pm) else { return };
    let attack = pm.cmd.buttons & button::ATTACK != 0;
    let ps = &mut *pm.ps;
    if delay_done {
        reload_delay_out(ps, d);
    }
    if ps.weapon_time != 0 {
        return;
    }
    if d.segmented_reload && attack {
        ps.weapon_state = interrupted(ps.weapon_state);
    }
    let st = ps.weapon_state;
    let start = in_reload_start(st);
    let another = may_reload(ps, d)
        && if start {
            !(st == state::RELOAD_OPEN_CUT && clip(ps, d) != 0)
        } else {
            d.segmented_reload && st != state::RELOAD_CUT
        };
    if !(start && another) {
        set_bit(&mut ps.weaponrechamber, ps.weapon, false);
    }
    if another {
        load_segment(ps, d);
    } else if (start || d.segmented_reload) && d.reload_end_time != 0 {
        ps.weapon_state = state::RELOAD_CLOSE;
        ps.weapon_time = d.reload_end_time;
        ps.start_weapon_anim(16);
        ps.add_event(ev::RELOAD_END, 0);
    } else {
        restart_ready(ps);
    }
}

/// A bolt action's chamber (`weaponrechamber`'s bit for the weapon, set by
/// its shot). From ready the bolt runs for `rechamberTime` with
/// `EV_RECHAMBER_WEAPON` (28, `cod4_lnxded` 0x806365c); the brass goes
/// (`EV_EJECT_BRASS`, 29, 0x80646bb) when the delay, `rechamberBoltTime`,
/// runs out, and the gun is ready once the time does. True ends
/// `PM_Weapon` for the cmd: the brass went while the bolt still runs.
fn check_rechamber(pm: &mut Pmove, delay_done: bool) -> bool {
    let Some(d) = held(pm).filter(|d| d.bolt_action) else {
        return false;
    };
    let ps = &mut *pm.ps;
    let unchambered = bit(&ps.weaponrechamber, ps.weapon);
    match ps.weapon_state {
        _ if !unchambered => {}
        state::RECHAMBER => {
            if delay_done {
                set_bit(&mut ps.weaponrechamber, ps.weapon, false);
                ps.add_event(ev::EJECT_BRASS, 0);
            }
            match ps.weapon_time {
                0 => back_to_ready(ps),
                _ if delay_done => return true,
                _ => {}
            }
        }
        state::READY if ps.weapon_time == 0 || ps.weapon_delay == 0 => {
            // The brass waits for the bolt when the def gives one inside
            // the rechamber, else it goes on the next cmd.
            let bolt = d.rechamber_bolt_time;
            let brass_at = if bolt != 0 && bolt < d.rechamber_time {
                bolt
            } else {
                1
            };
            let anim = if ps.weapon_pos_frac > 0.75 { 7 } else { 4 };
            ps.start_weapon_anim(anim);
            (ps.weapon_state, ps.weapon_time, ps.weapon_delay) =
                (state::RECHAMBER, d.rechamber_time, brass_at);
            ps.add_event(ev::RECHAMBER_WEAPON, 0);
        }
        _ => {}
    }
    false
}

/// The key that fires `d`: a detonator grenade's 0x80000, else the attack
/// key.
fn fire_key(d: &WeaponDef) -> u32 {
    if d.weap_type == WEAPTYPE_GRENADE && d.has_detonator {
        0x8_0000
    } else {
        button::ATTACK
    }
}

/// Whether the ready, firing or rechambering gun shoots this cmd: its
/// trigger is down (a detonator grenade's is the 0x80000 key), unless the
/// def freezes the body and it is in the air; or a shot's delay just ran
/// out; or a burst owes rounds. A gun that does not shoot goes ready, the
/// shot's anim run on.
fn trigger_fires(pm: &mut Pmove, d: &WeaponDef, delay_done: bool) -> bool {
    let ps = &mut *pm.ps;
    let grounded = ps.ground_entity_num != super::ENTITYNUM_NONE;
    let pulled = pm.cmd.buttons & fire_key(d) != 0 && (grounded || !d.freeze_movement_when_firing);
    if pulled || delay_done || mid_burst(ps, Some(d)) {
        return true;
    }
    if ps.weapon_state == state::FIRE {
        back_to_ready(ps);
    } else {
        ps.weapon_state = state::READY;
    }
    false
}

/// An empty clip under the trigger (`PM_Weapon_CheckFiringAmmo`): a reserve
/// reloads; with none a gun clicks (`EV_NOAMMO`) and waits 500 ms, and
/// either drops its chamber and goes to its idle anim (a grenade silently).
fn dry_fire(ps: &mut super::PlayerState, d: &WeaponDef) {
    if reserve(ps, d) >= 1 {
        start_reload(ps, d);
        return;
    }
    let grenade = d.weap_type == WEAPTYPE_GRENADE;
    if !grenade {
        ps.add_event(ev::NOAMMO, 0);
    }
    set_bit(&mut ps.weaponrechamber, ps.weapon, false);
    ps.continue_weapon_anim(0);
    if !grenade {
        ps.weapon_time += 500;
    }
}

/// The clocks a shot sets (`PM_Weapon_StartFiring`, inlined at 0x80640af).
/// A bullet weapon stores `fireTime` and `fireDelay` (0x80640be,
/// 0x80640b5; an ADS-only weapon's delay is the rest of its sight lerp),
/// marks a bolt for its rechamber and, on a pull's first round, opens the
/// kick window over the reduced-kick rounds. A main-hand grenade's first
/// cmd pulls the pin instead (`EV_PULLBACK_WEAPON`, anim 26) and its round
/// waits for `holdFireTime`. True when the anim script hears a shot.
fn load_round(ps: &mut super::PlayerState, d: &WeaponDef, delay_done: bool) -> bool {
    if d.weap_type == WEAPTYPE_GRENADE {
        if delay_done {
            return true;
        }
        ps.grenade_time_left = d.fuse_time;
        ps.start_weapon_anim(26);
        ps.add_event(ev::PULLBACK_WEAPON, ps.weapon);
        (ps.weapon_time, ps.weapon_delay) = (0, d.hold_fire_time);
        return false;
    }
    let frac = f64::from(ps.weapon_pos_frac);
    ps.weapon_time = d.fire_time;
    ps.weapon_delay = if d.ads_fire_only {
        ((1.0 - frac) * (1.0 / f64::from(d.oo_pos_anim_length[0]))) as i32
    } else {
        d.fire_delay
    };
    if d.bolt_action {
        set_bit(&mut ps.weaponrechamber, ps.weapon, true);
    }
    if ps.weapon_state != state::FIRE {
        let reduced = if frac < 1.0 {
            d.hip_gun_kick_reduced_kick_bullets
        } else {
            d.ads_gun_kick_reduced_kick_bullets
        };
        ps.weapon_restrict_kick_time = d.fire_delay + reduced * d.fire_time;
    }
    true
}

/// The round leaves once no delay holds it: one off the clip (not on a
/// turret), the fire anim by the sight and the last round (2 and 3 from
/// the hip, 5 and 6 sighted), `EV_FIRE_WEAPON` or `EV_FIRE_WEAPON_LASTSHOT`
/// (0x8064273), a held breath let go on a sighted scope, the hip spread's
/// kick, and `EV_NOAMMO` once clip and reserve are both dry.
fn release_round(ps: &mut super::PlayerState, d: &WeaponDef) {
    if ps.e_flags & ef::TURRET == 0 {
        use_ammo(ps, d, 1);
    }
    if d.weap_type == WEAPTYPE_GRENADE {
        ps.weapon_time = d.fire_time;
    }
    let last = clip(ps, d) == 0;
    let sighted = ps.weapon_pos_frac > 0.75;
    ps.start_weapon_anim(if sighted { 5 } else { 2 } + u32::from(last));
    ps.add_event(
        if last {
            ev::FIRE_WEAPON_LASTSHOT
        } else {
            ev::FIRE_WEAPON
        },
        0,
    );
    let full_sight = ps.weapon_pos_frac == 1.0;
    // `player_breath_fire_delay` is 0: a scope's shot only lets the breath go.
    if full_sight && d.overlay_reticle != 0 && d.weap_class != WEAPCLASS_ITEM {
        ps.weap_flags &= !wf::HOLDING_BREATH;
    }
    if !full_sight {
        let kicked = f64::from(ps.aim_spread_scale) + f64::from(d.hip_spread_fire_add) * 255.0;
        ps.aim_spread_scale = (kicked as f32).min(255.0);
    }
    if last && reserve(ps, d) == 0 && !d.has_detonator {
        ps.add_event(ev::NOAMMO, 0);
    }
}

/// A shot (`PM_Weapon_FireWeapon`): an empty clip goes to [`dry_fire`];
/// otherwise the round is loaded, the gun goes to `FIRE`, a pull's
/// rounds are counted (the count capped at 4, a pull's first clearing the
/// fresh-pull flag) and the round leaves now unless a delay holds it.
fn fire(pm: &mut Pmove, d: &WeaponDef, delay_done: bool) {
    if clip(pm.ps, d) < 1 {
        let seq = pm.ps.event_sequence;
        dry_fire(pm.ps, d);
        reload_anim(pm, seq);
        return;
    }
    if load_round(pm.ps, d, delay_done) {
        super::anim::event(pm, super::anim::Event::FireWeapon, false, true);
    }
    let ps = &mut *pm.ps;
    ps.weapon_state = state::FIRE;
    pin_prone(ps);
    if d.trigger() != Trigger::Auto {
        if ps.weapon_shot_count == 0 {
            ps.weap_flags &= !wf::TRIGGER_PULLED;
        }
        ps.weapon_shot_count = (ps.weapon_shot_count + 1).min(4);
    }
    if ps.weapon_delay == 0 {
        release_round(ps, d);
    }
}

/// What a state does once its clocks are out (or its delay ran out this
/// cmd). The second argument is that delay. Each state names its handler
/// in [`on_expiry`]; a state with none holds until the server moves it.
type Expiry = fn(&mut Pmove, bool);

fn on_expiry(st: i32) -> Expiry {
    match st {
        state::READY | state::FIRE | state::RECHAMBER => on_trigger,
        state::RAISE | state::RAISE_ALT | state::RELOAD_CLOSE => |pm, _| restart_ready(pm.ps),
        state::LOWER => |pm, _| on_lowered(pm, false),
        state::LOWER_QUICK => |pm, _| on_lowered(pm, true),
        state::RELOAD..=state::RELOAD_OPEN_CUT => segment_done,
        state::MELEE_SWING => |pm, _| melee_hit(pm.ps),
        state::MELEE_HIT => |pm, _| after_swing(pm),
        state::MELEE_RECOVER | state::OFFHAND_RAISE | state::SPRINT_OUT => {
            |pm, _| settle_ready(pm.ps)
        }
        state::OFFHAND_LOWER => |pm, _| throw_stage(pm, state::GRENADE_PIN),
        state::GRENADE_PIN => |pm, _| throw_stage(pm, state::GRENADE_HELD),
        state::GRENADE_THROW => |pm, _| throw_stage(pm, state::GRENADE_GONE),
        state::GRENADE_HELD => |pm, _| cooked_throw(pm),
        state::GRENADE_GONE => |pm, _| throw_stage(pm, state::OFFHAND_RAISE),
        state::SPRINT_IN => |pm, _| sprint_lowered(pm.ps),
        _ => |_, _| {},
    }
}

/// Ready with the idle anim restarted: a raise or a reload's end done.
fn restart_ready(ps: &mut super::PlayerState) {
    ps.weapon_state = state::READY;
    ps.start_weapon_anim(0);
}

/// The sprint's lowering done: the lowered loop (anim 24) until the sprint
/// ends.
fn sprint_lowered(ps: &mut super::PlayerState) {
    ps.weapon_state = state::SPRINT_HOLD;
    (ps.weapon_time, ps.weapon_delay) = (0, 0);
    ps.start_weapon_anim(24);
}

/// The old weapon is down: the change goes through, and a raise it starts
/// plays the anim script's `RAISEWEAPON`.
fn on_lowered(pm: &mut Pmove, quick: bool) {
    let seq = pm.ps.event_sequence;
    bring_up(pm, quick);
    if raised(pm.ps, seq, ev::RAISE_WEAPON) || raised(pm.ps, seq, ev::FIRST_RAISE_WEAPON) {
        super::anim::event(pm, super::anim::Event::RaiseWeapon, false, false);
    }
}

/// The ready, firing and rechambering states: the trigger. Nothing fires
/// on empty hands, with a main-hand grenade held back for its throw, or
/// while frozen.
fn on_trigger(pm: &mut Pmove, delay_done: bool) {
    let Some(d) = held(pm).filter(|_| pm.ps.weapon != 0) else {
        return;
    };
    if !trigger_fires(pm, d, delay_done) {
        return;
    }
    // A grenade thrown on the key's release keeps its due release at a
    // 1 ms delay while the key stays down, frozen or not.
    let on_release = d.weap_type == WEAPTYPE_GRENADE && !d.hold_button_to_throw;
    if delay_done && on_release && pm.cmd.buttons & fire_key(d) != 0 {
        pm.ps.weapon_delay = 1;
    } else if pm.ps.pm_flags & pmf::FROZEN == 0 {
        fire(pm, d, delay_done);
    }
}

/// The requests a cmd may make of an idle gun, in 1.7's order: sprint,
/// offhand, change (the anim script's `DROPWEAPON` for a put-away outside a
/// mantle), reload (its `RELOAD`), melee, a grenade's cancel. A burst owing
/// rounds takes none.
fn take_requests(pm: &mut Pmove, delay_done: bool) {
    sprint_request(pm);
    offhand_request(pm);
    let seq = pm.ps.event_sequence;
    change_request(pm);
    if raised(pm.ps, seq, ev::PUTAWAY_WEAPON) && pm.ps.pm_flags & pmf::MANTLE == 0 {
        super::anim::event(pm, super::anim::Event::DropWeapon, false, true);
    }
    let seq = pm.ps.event_sequence;
    reload_request(pm);
    reload_anim(pm, seq);
    melee_request(pm, delay_done);
    throw_cancel(pm);
}

/// `PM_Weapon` (`cod4_lnxded` 0x8062df0): a live player off a turret and
/// past his respawn cmd runs the sight's lerp and the breath, a cooking
/// grenade (which may end the cmd here), the trigger's edge and the clocks;
/// then the cmd's requests unless mid-burst, the rechamber, the hip spread
/// pinned open crawling or in a melee, and, once the clocks are out, the
/// state's [`on_expiry`] handler. The dead hold no weapon.
pub(super) fn pm_weapon(pm: &mut Pmove, pml: &Pml) {
    if pm.ps.pm_type >= pm_type::DEAD {
        pm.ps.weapon = 0;
        return;
    }
    if pm.ps.pm_flags & pmf::RESPAWNED != 0 || pm.ps.e_flags & ef::TURRET != 0 {
        return;
    }
    ads_lerp(pm, pml);
    super::breath::update(pm, pml);
    if fuse_in_hand(pm, pml) {
        return;
    }
    note_trigger_pull(pm);
    let delay_done = run_clocks(pm, pml);
    if !mid_burst(pm.ps, held(pm)) {
        take_requests(pm, delay_done);
    }
    if check_rechamber(pm, delay_done) {
        return;
    }
    let ps = &mut *pm.ps;
    let crawling = ps.pm_flags & pmf::PRONE != 0
        && (pm.cmd.forward_move != 0 || pm.cmd.right_move != 0)
        && ps.weapon_pos_frac != 1.0;
    if crawling || is_melee(ps.weapon_state) {
        ps.aim_spread_scale = 255.0;
    }
    let waiting = ps.weapon_time != 0 || ps.weapon_delay != 0;
    if waiting && !delay_done {
        return;
    }
    on_expiry(ps.weapon_state)(pm, delay_done);
}

fn def_of<'d>(pm: &Pmove<'d>, w: u32) -> Option<&'d WeaponDef> {
    pm.defs?.def(w)
}

/// `BG_ThrowingBackGrenade`.
fn throwing_back(ps: &super::PlayerState) -> bool {
    ps.throw_back_grenade_owner != super::ENTITYNUM_NONE
}

/// `BG_WeaponAmmo`: the clip and the reserve.
fn weapon_ammo(ps: &super::PlayerState, d: &WeaponDef) -> i32 {
    clip(ps, d) + reserve(ps, d)
}

/// `PM_WeaponUseAmmo`: `amount` off the clip, never below 0.
fn use_ammo(ps: &mut super::PlayerState, d: &WeaponDef, amount: i32) {
    if let Some(c) = ps.ammoclip.get_mut(d.clip_index) {
        *c -= (*c).min(amount);
    }
}

/// A grenade's fuse running in the hand (cod4-combat.md 10.1): the
/// offhand's while it shows (`weapFlags` 2), else the main hand's. Under
/// `cookOffHold` it counts the cmd down; once out it reads -1,
/// `EV_GRENADE_SUICIDE` (the offhand in the parm) rides the ring and the
/// round is spent, unless it was someone else's grenade thrown back. True
/// ends `PM_Weapon` for the cmd.
fn fuse_in_hand(pm: &mut Pmove, pml: &Pml) -> bool {
    let w = if pm.ps.weap_flags & wf::USING_OFFHAND != 0 {
        pm.ps.off_hand_index
    } else {
        pm.ps.weapon
    };
    let Some(d) = def_of(pm, w).filter(|d| d.weap_type == WEAPTYPE_GRENADE) else {
        return false;
    };
    let ps = &mut *pm.ps;
    if ps.grenade_time_left <= 0 {
        return false;
    }
    let tick = if d.cook_off_hold { pml.msec } else { 0 };
    ps.grenade_time_left -= tick;
    if ps.grenade_time_left > 0 {
        return false;
    }
    ps.grenade_time_left = -1;
    ps.add_event(ev::GRENADE_SUICIDE, ps.off_hand_index);
    if !throwing_back(ps) {
        use_ammo(ps, d, 1);
    }
    true
}

/// The first weapon of offhand `class` the player holds, by number; with
/// `loaded` only one with a round (or any while a grenade is being thrown
/// back).
fn offhand_of(pm: &Pmove, class: i32, loaded: bool) -> Option<u32> {
    let defs = pm.defs?;
    let ps = &*pm.ps;
    (1..defs.count()).find(|&w| {
        let Some(d) = defs.def(w) else { return false };
        let ready = !loaded || ps.throw_back_grenade_time_left > 0 || weapon_ammo(ps, d) > 0;
        d.offhand_class == class && ps.has_weapon(w) && ready
    })
}

/// The offhand class a cmd's keys ask for: the frag key first, then the
/// smoke key, which takes the flash class under `offhandSecondary`.
fn offhand_key_class(buttons: u32, secondary: u32) -> Option<i32> {
    if buttons & button::FRAG != 0 {
        Some(OFFHAND_CLASS_FRAG)
    } else if buttons & button::SMOKE == 0 {
        None
    } else if secondary != 0 {
        Some(OFFHAND_CLASS_FLASH)
    } else {
        Some(OFFHAND_CLASS_SMOKE)
    }
}

/// Whether the offhand keys are heard: off a turret, weapons on, not
/// sprinting, no cocked main-hand grenade, not mid-throw (the throw's
/// raise, 20, is free) and not under night vision.
fn offhand_free(pm: &Pmove) -> bool {
    let ps = &*pm.ps;
    let grenade_down = held(pm).is_some_and(|d| {
        d.weap_type == WEAPTYPE_GRENADE
            && !d.hold_button_to_throw
            && pm.cmd.buttons & fire_key(d) != 0
    });
    let busy = (state::OFFHAND_LOWER..state::OFFHAND_RAISE).contains(&ps.weapon_state)
        || matches!(ps.weapon_state, state::NVG_ON | state::NVG_OFF);
    ps.e_flags & ef::TURRET == 0
        && ps.weap_flags & wf::DISABLED == 0
        && ps.pm_flags & pmf::SPRINTING == 0
        && !busy
        && !grenade_down
}

/// The offhand request (`PM_Weapon_CheckForOffHand`, cod4-combat.md 10.1).
/// The cmd's equipped offhand is taken where held. A key whose class has a
/// round switches to it (`EV_SWITCH_OFFHAND`) and starts the throw: at the
/// pin with empty hands or the last throw's raise and no use hint showing,
/// else by putting the gun away. A spent class clicks (`EV_EMPTY_OFFHAND`),
/// with the frag or special grenade hint where one is held.
fn offhand_request(pm: &mut Pmove) {
    if !offhand_free(pm) {
        return;
    }
    let equipped = u32::from(pm.cmd.offhand);
    if pm.ps.has_weapon(equipped) {
        pm.ps.off_hand_index = equipped;
    }
    let Some(class) = offhand_key_class(pm.cmd.buttons, pm.ps.offhand_secondary) else {
        return;
    };
    let Some(w) = offhand_of(pm, class, true) else {
        pm.ps.add_event(ev::EMPTY_OFFHAND, 0);
        if offhand_of(pm, class, false).is_some() {
            let hint = match class {
                OFFHAND_CLASS_FRAG => ev::NO_FRAG_GRENADE_HINT,
                _ => ev::NO_SPECIAL_GRENADE_HINT,
            };
            pm.ps.add_event(hint, 0);
        }
        return;
    };
    pm.ps.add_event(ev::SWITCH_OFFHAND, w);
    pm.ps.off_hand_index = w;
    let hands_free = pm.ps.weapon == 0 || pm.ps.weapon_state == state::OFFHAND_RAISE;
    if hands_free && pm.ps.cursor_hint_ent_index == super::ENTITYNUM_NONE {
        throw_stage(pm, state::GRENADE_PIN);
    } else {
        throw_stage(pm, state::OFFHAND_LOWER);
    }
}

/// Enters stage `to` of an offhand throw (cod4-combat.md 10.1; the walk
/// capture's frag reads 15, 16, 17, 19, 20, VERIFIED live):
///
/// - 15: the gun goes down at its quick drop (100 ms on empty hands), the
///   sight let go, any throw back forgotten.
/// - 16: the pin, for the offhand's `holdFireTime` (`EV_PREP_OFFHAND`
///   with anim 26, or anim 18 for a grenade thrown back).
/// - 18: the pin is out and the fuse starts; held there while the key is.
/// - 17: the throw, anim 2 for `fireTime`, the release at `fireDelay`.
/// - 19: the release (`EV_USE_OFFHAND`) and its round, or a click.
/// - 20: the gun back up at its quick raise.
///
/// The offhand shows on the view model (`weapFlags` 2) from the pin to
/// the release.
fn throw_stage(pm: &mut Pmove, to: i32) {
    let gun = held(pm).filter(|_| pm.ps.weapon != 0).cloned();
    let off = def_of(pm, pm.ps.off_hand_index).cloned();
    let ps = &mut *pm.ps;
    let back = throwing_back(ps);
    ps.weapon_state = to;
    if matches!(to, state::OFFHAND_LOWER | state::OFFHAND_RAISE) {
        ps.weap_flags &= !wf::USING_OFFHAND;
    } else {
        ps.weap_flags |= wf::USING_OFFHAND;
    }
    let field = |f: fn(&WeaponDef) -> i32| off.as_ref().map_or(0, f);
    match to {
        state::OFFHAND_LOWER => {
            exit_ads(ps);
            ps.throw_back_grenade_owner = super::ENTITYNUM_NONE;
            ps.weapon_delay = 0;
            ps.weapon_time = gun.as_ref().map_or(100, |g| g.quick_drop_time);
            if gun.is_some() {
                ps.start_weapon_anim(19);
            }
        }
        state::GRENADE_PIN => {
            (ps.weapon_time, ps.weapon_delay) = (field(|d| d.hold_fire_time), 0);
            if !back {
                ps.add_event(ev::PREP_OFFHAND, ps.off_hand_index);
            }
            ps.start_weapon_anim(if back { 18 } else { 26 });
            pin_prone(ps);
        }
        state::GRENADE_HELD => {
            (ps.weapon_time, ps.weapon_delay) = (0, 0);
            if !back {
                ps.grenade_time_left = field(|d| d.fuse_time);
            }
        }
        state::GRENADE_THROW => {
            ps.weapon_time = field(|d| d.fire_time);
            ps.weapon_delay = field(|d| d.fire_delay);
            ps.start_weapon_anim(2);
            super::anim::event(pm, super::anim::Event::FireWeapon, false, true);
        }
        state::GRENADE_GONE => {
            ps.add_event(ev::USE_OFFHAND, ps.off_hand_index);
            match off.as_ref() {
                _ if back => {}
                Some(d) if weapon_ammo(ps, d) > 0 => use_ammo(ps, d, 1),
                _ => ps.add_event(ev::EMPTY_OFFHAND, 0),
            }
        }
        state::OFFHAND_RAISE => {
            match &gun {
                Some(g) => {
                    (ps.weapon_time, ps.weapon_delay) = (g.quick_raise_time, 0);
                    ps.start_weapon_anim(20);
                }
                None => (ps.weapon_time, ps.weapon_delay) = (0, 1),
            }
            ps.throw_back_grenade_time_left = 0;
            ps.throw_back_grenade_owner = super::ENTITYNUM_NONE;
            ps.pm_flags &= !pmf::PRONE_STILL;
        }
        _ => unreachable!("not an offhand stage: {to}"),
    }
}

/// Stage 18's turn: a grenade that goes on the key's release stays in the
/// hand (its delay at 1) while an offhand key holds from the last cmd,
/// then the throw.
fn cooked_throw(pm: &mut Pmove) {
    let Some(d) = def_of(pm, pm.ps.off_hand_index) else {
        return;
    };
    let keys = button::FRAG | button::SMOKE;
    let held_on = pm.cmd.buttons & keys != 0 && pm.oldcmd.buttons & keys != 0;
    if held_on && !d.hold_button_to_throw {
        pm.ps.weapon_delay = 1;
    } else {
        throw_stage(pm, state::GRENADE_THROW);
    }
}

/// A `holdButtonToThrow` grenade let go early goes back: the offhand
/// still at its pin with no offhand key down skips to the raise, and a
/// main-hand one in its shot with the attack key up returns to ready on
/// anim 1.
fn throw_cancel(pm: &mut Pmove) {
    let keys_up = |mask| pm.cmd.buttons & mask == 0;
    if pm.ps.weapon_state == state::GRENADE_PIN {
        let hold = def_of(pm, pm.ps.off_hand_index).is_some_and(|d| d.hold_button_to_throw);
        if hold && keys_up(button::FRAG | button::SMOKE) {
            throw_stage(pm, state::OFFHAND_RAISE);
        }
        return;
    }
    let main_hold =
        held(pm).is_some_and(|d| d.weap_type == WEAPTYPE_GRENADE && d.hold_button_to_throw);
    if main_hold && pm.ps.weapon_state == state::FIRE && keys_up(button::ATTACK) {
        settle_ready(pm.ps);
        pm.ps.start_weapon_anim(1);
    }
}

/// Whether this cmd's melee key starts a swing (`PM_Weapon_CheckForMelee`,
/// cod4-combat.md 10.4): its press edge, a weapon with `meleeDamage`, no
/// delayed action due this cmd, no delay pending but a reload's, not
/// through a scope's sight, and a state outside the swing, the offhand,
/// night vision and the raise and drop states.
fn swing_allowed(pm: &Pmove, d: &WeaponDef, delay_done: bool) -> bool {
    let ps = &*pm.ps;
    let st = ps.weapon_state;
    let pressed = pm.cmd.buttons & button::MELEE != 0 && pm.oldcmd.buttons & button::MELEE == 0;
    let state_ok = !(is_melee(st)
        || is_offhand(st)
        || (state::RAISE..=state::LOWER_QUICK).contains(&st)
        || matches!(st, state::NVG_ON | state::NVG_OFF));
    let scoped = d.overlay_reticle != 0 && ps.weapon_pos_frac > 0.0;
    pressed
        && state_ok
        && d.melee_damage != 0
        && !delay_done
        && (ps.weapon_delay == 0 || is_reloading(st))
        && !scoped
}

/// The melee request: an allowed press starts or clears the lunge
/// (`charge::start`), then the swing, the charge swing where the lunge is
/// on and the weapon has one (cod4-pmove.md 19), with its anim script
/// event.
fn melee_request(pm: &mut Pmove, delay_done: bool) {
    let Some(d) = held(pm) else { return };
    if !swing_allowed(pm, d, delay_done) {
        return;
    }
    super::charge::start(pm);
    let lunge = pm.ps.pm_flags & pmf::MELEE_CHARGE != 0 && d.has_charge_melee();
    let (time, delay, anim) = if lunge {
        (d.melee_charge_time, d.melee_charge_delay, 9)
    } else {
        (d.melee_time, d.melee_delay, 8)
    };
    let ps = &mut *pm.ps;
    (ps.weapon_state, ps.weapon_time, ps.weapon_delay) = (state::MELEE_SWING, time, delay);
    ps.start_weapon_anim(anim);
    ps.add_event(ev::MELEE_SWIPE, 0);
    pin_prone(ps);
    let script = match (d.knife_model, lunge) {
        (true, true) => super::anim::Event::KnifeMeleeCharge,
        (true, false) => super::anim::Event::KnifeMelee,
        (false, _) => super::anim::Event::MeleeAttack,
    };
    super::anim::event(pm, script, false, true);
}

/// The swing's delay out: the hit (`EV_FIRE_MELEE`, which the server
/// traces), the rest of `meleeTime` in state 13.
fn melee_hit(ps: &mut super::PlayerState) {
    ps.weapon_state = state::MELEE_HIT;
    ps.add_event(ev::FIRE_MELEE, 0);
    pin_prone(ps);
}

/// The swing done: a knife model raises the gun at its quick raise (state
/// 14, anim 20), anything else is ready at once.
fn after_swing(pm: &mut Pmove) {
    let raise = held(pm)
        .filter(|d| d.knife_model)
        .map(|d| d.quick_raise_time);
    let ps = &mut *pm.ps;
    let Some(t) = raise else {
        settle_ready(ps);
        return;
    };
    (ps.weapon_state, ps.weapon_time, ps.weapon_delay) = (state::MELEE_RECOVER, t, 0);
    ps.start_weapon_anim(20);
    pin_prone(ps);
}

/// Whether the events since `seq` hold `event`.
fn raised(ps: &super::PlayerState, seq: i32, event: u32) -> bool {
    (seq.max(ps.event_sequence - 4)..ps.event_sequence)
        .any(|k| ps.events[(k & 3) as usize] == event)
}

/// `PM_BeginWeaponReload`'s `RELOAD` script event for a reload begun since
/// `seq`, not on a clip-only weapon.
fn reload_anim(pm: &mut Pmove, seq: i32) {
    if raised(pm.ps, seq, ev::RELOAD_START_NOTIFY) && !held(pm).is_some_and(|d| d.clip_only) {
        super::anim::event(pm, super::anim::Event::Reload, false, true);
    }
}
