//! The weapon half of CoD2's `PmoveSingle`: `PM_Weapon` (Mac 0x7cbb2,
//! Linux's split helpers around 0x80ef1ac) and what it calls for a shot, a
//! reload, a switch, a melee swing and an offhand grenade, plus
//! `PM_UpdateAimDownSightFlag` / `Lerp` and `PM_AdjustAimSpreadScale`.
//! docs/research/cod2-combat.md section 1 has the states and the events.
//!
//! The state lives beside the movement's [`PlayerState`] in a
//! [`WeaponState`], so the server can mirror its ammo arrays and held
//! weapons in and out of the script's tables. The events go on the
//! playerstate's ring, where `ClientEvents` reads the fire events back.
//!
//! The binoculars (states 17..22) are [`binoculars`]' arm.
//!
//! The sniper's breath hold is [`update_hold_breath`].
//!
//! Not here: the weapon slots' alt-weapon bookkeeping (`ps+0x555`), `PM_SetProneMovementOverride`,
//! and the jump and prone stance's effects on the sight. The torso events
//! (`BG_AnimScriptEvent` for the fire, reload, melee, drop and raise) run
//! when the caller hands [`WeaponCmd::anim`] the script.

use crate::net::UserCmd2;
use crate::pmove::anim::{self, AnimInput, AnimTable, Event};
use crate::pmove::{PlayerState, button, pm_type, pmf};
use crate::weapon::WeaponDef;

/// `weaponstate` (ps+0xd8), as `PM_Weapon` numbers them.
pub mod state {
    pub const READY: i32 = 0;
    pub const RAISING: i32 = 1;
    pub const DROPPING: i32 = 2;
    pub const FIRING: i32 = 3;
    pub const RECHAMBERING: i32 = 4;
    pub const RELOADING: i32 = 5;
    pub const RELOADING_INTERRUPTED: i32 = 6;
    pub const RELOAD_START: i32 = 7;
    pub const RELOAD_START_INTERRUPTED: i32 = 8;
    pub const RELOAD_END: i32 = 9;
    pub const MELEE_INIT: i32 = 10;
    pub const MELEE_RELAX: i32 = 11;
    pub const OFFHAND_INIT: i32 = 12;
    pub const OFFHAND_PREPARE: i32 = 13;
    pub const OFFHAND_HOLD: i32 = 14;
    pub const OFFHAND_START: i32 = 15;
    pub const OFFHAND: i32 = 16;
    /// The binoculars' (Mac 0x7cbb2, cod2-combat.md 1.8): leaving the sight
    /// first, the weapon going down, the binoculars coming up, up, going
    /// down, and the weapon coming back.
    pub const BINOCULARS_ADS_EXIT: i32 = 17;
    pub const BINOCULARS_DROP_WEAPON: i32 = 18;
    pub const BINOCULARS_RAISE: i32 = 19;
    pub const BINOCULARS_HOLD: i32 = 20;
    pub const BINOCULARS_DROP: i32 = 21;
    pub const BINOCULARS_RAISE_WEAPON: i32 = 22;
}

/// The weapon events `PM_Weapon` raises (CoD2's table, `net::events`).
pub mod ev {
    pub const NOAMMO: i32 = 146;
    pub const EMPTY_OFFHAND: i32 = 148;
    pub const RESET_ADS: i32 = 149;
    pub const RELOAD: i32 = 150;
    pub const RELOAD_FROM_EMPTY: i32 = 151;
    pub const RELOAD_START: i32 = 152;
    pub const RELOAD_END: i32 = 153;
    pub const RAISE_WEAPON: i32 = 154;
    pub const PUTAWAY_WEAPON: i32 = 155;
    pub const WEAPON_ALT: i32 = 156;
    pub const PULLBACK_WEAPON: i32 = 157;
    pub const FIRE_WEAPON: i32 = 158;
    pub const FIRE_WEAPON_LASTSHOT: i32 = 160;
    pub const RECHAMBER_WEAPON: i32 = 161;
    pub const EJECT_BRASS: i32 = 162;
    pub const MELEE_SWIPE: i32 = 163;
    pub const FIRE_MELEE: i32 = 164;
    pub const PREP_OFFHAND: i32 = 165;
    pub const USE_OFFHAND: i32 = 166;
    pub const SWITCH_OFFHAND: i32 = 167;
    pub const BINOCULAR_ENTER: i32 = 168;
    pub const BINOCULAR_EXIT: i32 = 169;
    pub const BINOCULAR_FIRE: i32 = 170;
    pub const BINOCULAR_RELEASE: i32 = 171;
    pub const BINOCULAR_DROP: i32 = 172;
    pub const GRENADE_SUICIDE: i32 = 197;
}

/// The cmd buttons `PM_Weapon` reads beside `button::ATTACK` and
/// `button::ADS`: the melee key (0x4), the reload key (0x10) and the two
/// offhand keys, frag 0x10000 and smoke 0x20000 (the `local_24` class
/// pick). INFERRED names, VERIFIED masks.
pub mod key {
    pub const MELEE: i32 = 0x4;
    pub const RELOAD: i32 = 0x10;
    pub const FRAG: i32 = 0x1_0000;
    pub const SMOKE: i32 = 0x2_0000;
    /// `+binoculars`.
    pub const BINOCULARS: i32 = 0x4000;
    /// `+activate` and `+usereload`, which look through the binoculars.
    pub const USE: i32 = 0x28;
    /// `+holdbreath` (the cmd's 0x8000, read at 0x80ed084).
    pub const HOLD_BREATH: i32 = 0x8000;
}

/// `pm_flags` the weapon owns: 0x10 an offhand in hand (the view draws
/// `offHandIndex`), 0x8 a script's reload request, 0x2000 the melee key's
/// latch, 0x4000000 weapons disabled.
pub mod wpf {
    pub const OFFHAND: u32 = 0x10;
    pub const RELOAD_REQUEST: u32 = 0x8;
    pub const MELEE_HELD: u32 = 0x2000;
    pub const DISABLED: u32 = 0x400_0000;
    /// The binoculars are up (`BG_GetViewmodelWeaponIndex` draws them).
    pub const BINOCULARS: u32 = 0x2_0000;
    /// The breath is held (0x80ecf6c sets it, 0x80ecf84 clears it).
    pub const HOLD_BREATH: u32 = 0x4000;
}

/// The `player_breath_*` dvars the breath hold reads (registered 0x1180,
/// mirrored to the client), at their defaults: seconds and rates per second.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Breath {
    pub hold_time: f32,
    pub gasp_time: f32,
    pub fire_delay: f32,
    pub gasp_scale: f32,
    pub hold_lerp: f32,
    pub gasp_lerp: f32,
}

impl Breath {
    pub const DEFAULT: Breath = Breath {
        hold_time: 4.5,
        gasp_time: 1.0,
        fire_delay: 0.0,
        gasp_scale: 4.5,
        hold_lerp: 4.0,
        gasp_lerp: 6.0,
    };
}

/// `(int)(1000.0 * seconds)` under the truncating control word, the product
/// unrounded on the x87 stack.
fn breath_ms(seconds: f32) -> i32 {
    (1000.0 * f64::from(seconds)) as i32
}

/// The weapon def fields `PM_Weapon` and the server's fire path read, by
/// their `WeaponDef` offsets (cod2-weapons.md 3), with the indices the
/// level's weapon table gave the ammo and the clip.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct WeaponParams {
    pub name: String,
    /// +0x74: the `playerAnimType` index the anim script reads.
    pub player_anim_type: i32,
    /// +0x78: 0 bullet, 1 grenade, 2 projectile, 3 binoculars.
    pub weap_type: i32,
    /// +0x7c.
    pub weap_class: i32,
    /// +0x84: 0 none, 1 frag, 2 smoke.
    pub offhand_class: i32,
    pub ammo_index: usize,
    pub clip_index: usize,
    pub clip_size: i32,
    pub shot_count: i32,
    pub damage: i32,
    pub melee_damage: i32,
    pub min_damage: i32,
    pub fire_delay: i32,
    pub melee_delay: i32,
    pub fire_time: i32,
    pub rechamber_time: i32,
    pub rechamber_bolt_time: i32,
    pub hold_fire_time: i32,
    pub melee_time: i32,
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
    pub fuse_time: i32,
    pub hip_spread_stand_min: f32,
    pub hip_spread_ducked_min: f32,
    pub hip_spread_prone_min: f32,
    pub hip_spread_max: f32,
    pub hip_spread_ducked_max: f32,
    pub hip_spread_prone_max: f32,
    pub hip_spread_decay_rate: f32,
    pub hip_spread_fire_add: f32,
    pub hip_spread_turn_add: f32,
    pub hip_spread_move_add: f32,
    pub hip_spread_ducked_decay: f32,
    pub hip_spread_prone_decay: f32,
    pub ads_spread: f32,
    /// +0x588 and +0x58c: `1 / adsTransInTime` and `1 / adsTransOutTime`,
    /// 1/300 and 1/500 for a time of 0 (`BG_LoadWeaponDefInternal`).
    pub ads_in_rate: f32,
    pub ads_out_rate: f32,
    /// +0x2c0 and +0x2c4: `adsTransInTime` and `adsTransOutTime` in ms.
    pub ads_trans_in_time: i32,
    pub ads_trans_out_time: i32,
    pub ads_overlay_reticle: i32,
    pub rifle_bullet: bool,
    pub armor_piercing: bool,
    pub semi_auto: bool,
    pub bolt_action: bool,
    pub aim_down_sight: bool,
    pub rechamber_while_ads: bool,
    pub cook_off_hold: bool,
    pub clip_only: bool,
    pub ads_fire: bool,
    pub no_partial_reload: bool,
    pub segmented_reload: bool,
    pub reload_ammo_add: i32,
    pub reload_start_add: i32,
    /// The `altWeapon`'s index, 0 for none.
    pub alt_index: u8,
    pub ads_kick_reduced_bullets: i32,
    pub hip_kick_reduced_bullets: i32,
    pub explosion_radius: i32,
    pub explosion_inner_damage: i32,
    pub explosion_outer_damage: i32,
    pub projectile_speed: i32,
    pub projectile_speed_up: i32,
    /// +0x5a0..+0x5ac: a rocket's wobble (cod2-combat.md 2.6).
    pub destabilization_base_time: f32,
    pub destabilization_time_reduction_ratio: f32,
    pub destabilization_angle_max: f32,
    pub destabilize_distance: i32,
    /// +0x390: 0 grenade, 1 rocket, 2 none.
    pub proj_explosion_type: i32,
    pub proj_explosion_effect: String,
    pub proj_impact_explode: bool,
    pub max_damage_range: f32,
    /// +0x260: how far an enemy under the crosshair lights it
    /// (`Player_UpdateLookAtEntity`).
    pub enemy_crosshair_range: f32,
    /// +0x264: `moveSpeedScale`, the walk's cmd scale (0x80e1c58 applies
    /// it only above 0).
    pub move_speed_scale: f32,
    pub min_damage_range: f32,
    /// +0x5b0..: `locNone` .. `locGun`, by hit location.
    pub loc_multipliers: [f32; 19],
    /// +0x3a0 and +0x3fc: the bounce kept along and across a surface, by
    /// surface type (23).
    pub parallel_bounce: [f32; 23],
    pub perpendicular_bounce: [f32; 23],
    /// The keys the server's aim block reads (`super::aim`).
    pub aim: super::aim::AimKeys,
}

/// The loc keys in hit-location order (+0x5b0 + 4 * location).
const LOC_KEYS: [&str; 19] = [
    "locNone",
    "locHelmet",
    "locHead",
    "locNeck",
    "locTorsoUpper",
    "locTorsoLower",
    "locRightArmUpper",
    "locLeftArmUpper",
    "locRightArmLower",
    "locLeftArmLower",
    "locRightHand",
    "locLeftHand",
    "locRightLegUpper",
    "locLeftLegUpper",
    "locRightLegLower",
    "locLeftLegLower",
    "locRightFoot",
    "locLeftFoot",
    "locGun",
];

/// The 23 surface types the bounce keys name, in `surfaceFlags >> 20`
/// order (the `parallel<Surface>Bounce` run, +0x3a0..+0x3f8).
pub const SURFACE_NAMES: [&str; 23] = [
    "Default", "Bark", "Brick", "Carpet", "Cloth", "Concrete", "Dirt", "Flesh", "Foliage", "Glass",
    "Grass", "Gravel", "Ice", "Metal", "Mud", "Paper", "Plaster", "Rock", "Sand", "Snow", "Water",
    "Wood", "Asphalt",
];

/// The `pmove_t` move scale for a body holding `weapon` (ps+0xd4): its
/// `moveSpeedScale`, 0 (none) for no weapon or one the table lacks. The
/// walk's cmd scale (0x80e1c58) reads `BG_GetWeaponDef(ps->weapon)` +0x264.
pub fn move_scale(defs: &dyn Weapons, weapon: u8) -> f32 {
    if weapon == 0 {
        return 0.0;
    }
    defs.get(weapon).map_or(0.0, |w| w.move_speed_scale)
}

impl WeaponParams {
    /// The fields of `def`, with the level table's ammo and clip indices,
    /// the clip size of the clip, and the alt weapon's index.
    pub fn from_def(
        def: &WeaponDef,
        ammo_index: usize,
        clip_index: usize,
        alt_index: u8,
    ) -> WeaponParams {
        let rate = |key: &str, default: f32| match def.int(key) {
            0 => default,
            ms => 1.0 / ms as f32,
        };
        // `maxDamageRange` and `minDamageRange` at or below 0 read 999999
        // and 999999.125 (cod2-weapons.md 1).
        let range = |key: &str, word: u32| match def.float(key) {
            r if r <= 0.0 => f32::from_bits(word),
            r => r,
        };
        WeaponParams {
            name: def.name.clone(),
            player_anim_type: def.int("playerAnimType"),
            move_speed_scale: def.float("moveSpeedScale"),
            weap_type: def.weap_type(),
            weap_class: def.weap_class(),
            offhand_class: def.int("offhandClass"),
            ammo_index,
            clip_index,
            clip_size: def.int("clipSize"),
            shot_count: def.int("shotCount"),
            damage: def.int("damage"),
            melee_damage: def.int("meleeDamage"),
            min_damage: def.int("minDamage"),
            fire_delay: def.int("fireDelay"),
            melee_delay: def.int("meleeDelay"),
            fire_time: def.int("fireTime"),
            rechamber_time: def.int("rechamberTime"),
            rechamber_bolt_time: def.int("rechamberBoltTime"),
            hold_fire_time: def.int("holdFireTime"),
            melee_time: def.int("meleeTime"),
            reload_time: def.int("reloadTime"),
            reload_empty_time: def.int("reloadEmptyTime"),
            reload_add_time: def.int("reloadAddTime"),
            reload_start_time: def.int("reloadStartTime"),
            reload_start_add_time: def.int("reloadStartAddTime"),
            reload_end_time: def.int("reloadEndTime"),
            drop_time: def.int("dropTime"),
            raise_time: def.int("raiseTime"),
            alt_drop_time: def.int("altDropTime"),
            alt_raise_time: def.int("altRaiseTime"),
            quick_drop_time: def.int("quickDropTime"),
            quick_raise_time: def.int("quickRaiseTime"),
            fuse_time: def.int("fuseTime"),
            hip_spread_stand_min: def.float("hipSpreadStandMin"),
            hip_spread_ducked_min: def.float("hipSpreadDuckedMin"),
            hip_spread_prone_min: def.float("hipSpreadProneMin"),
            hip_spread_max: def.float("hipSpreadMax"),
            hip_spread_ducked_max: def.float("hipSpreadDuckedMax"),
            hip_spread_prone_max: def.float("hipSpreadProneMax"),
            hip_spread_decay_rate: def.float("hipSpreadDecayRate"),
            hip_spread_fire_add: def.float("hipSpreadFireAdd"),
            hip_spread_turn_add: def.float("hipSpreadTurnAdd"),
            hip_spread_move_add: def.float("hipSpreadMoveAdd"),
            hip_spread_ducked_decay: def.float("hipSpreadDuckedDecay"),
            hip_spread_prone_decay: def.float("hipSpreadProneDecay"),
            ads_spread: def.float("adsSpread"),
            ads_in_rate: rate("adsTransInTime", f32::from_bits(0x3b5a_740e)),
            ads_out_rate: rate("adsTransOutTime", 0.002),
            ads_trans_in_time: def.int("adsTransInTime"),
            ads_trans_out_time: def.int("adsTransOutTime"),
            ads_overlay_reticle: def.int("adsOverlayReticle"),
            rifle_bullet: def.flag("rifleBullet"),
            armor_piercing: def.flag("armorPiercing"),
            semi_auto: def.flag("semiAuto"),
            bolt_action: def.flag("boltAction"),
            aim_down_sight: def.flag("aimDownSight"),
            rechamber_while_ads: def.flag("rechamberWhileAds"),
            cook_off_hold: def.flag("cookOffHold"),
            clip_only: def.flag("clipOnly"),
            ads_fire: def.flag("adsFire"),
            no_partial_reload: def.flag("noPartialReload"),
            segmented_reload: def.flag("segmentedReload"),
            reload_ammo_add: def.int("reloadAmmoAdd"),
            reload_start_add: def.int("reloadStartAdd"),
            alt_index,
            ads_kick_reduced_bullets: def.int("adsGunKickReducedKickBullets"),
            hip_kick_reduced_bullets: def.int("hipGunKickReducedKickBullets"),
            explosion_radius: def.int("explosionRadius"),
            explosion_inner_damage: def.int("explosionInnerDamage"),
            explosion_outer_damage: def.int("explosionOuterDamage"),
            projectile_speed: def.int("projectileSpeed"),
            projectile_speed_up: def.int("projectileSpeedUp"),
            destabilization_base_time: def.float("destabilizationBaseTime"),
            destabilization_time_reduction_ratio: def.float("destabilizationTimeReductionRatio"),
            destabilization_angle_max: def.float("destabilizationAngleMax"),
            destabilize_distance: def.int("destabilizeDistance"),
            proj_explosion_type: def.int("projExplosionType"),
            proj_explosion_effect: def.str("projExplosionEffect").to_string(),
            proj_impact_explode: def.flag("projImpactExplode"),
            max_damage_range: range("maxDamageRange", 0x4974_23f0),
            enemy_crosshair_range: def.float("enemyCrosshairRange"),
            min_damage_range: range("minDamageRange", 0x4974_23f2),
            loc_multipliers: LOC_KEYS.map(|k| def.float(k)),
            parallel_bounce: SURFACE_NAMES.map(|s| def.float(&format!("parallel{s}Bounce"))),
            perpendicular_bounce: SURFACE_NAMES
                .map(|s| def.float(&format!("perpendicular{s}Bounce"))),
            aim: super::aim::AimKeys::of(def),
        }
    }

    pub fn is_bullet(&self) -> bool {
        self.weap_type == 0
    }

    pub fn is_grenade(&self) -> bool {
        self.weap_type == 1
    }

    /// `Bullet_CalcDamageRange` (0x811fb7c): full damage inside
    /// `maxDamageRange`, `minDamage` past `minDamageRange`, between them
    /// the stored fraction `t` into `(1 - t) * damage + t * minDamage`
    /// (0x8120e7e) on the stack, truncated (`fistp` under `| 0xc00`).
    pub fn damage_at(&self, dist: f32) -> i32 {
        if dist < self.max_damage_range {
            return self.damage;
        }
        if dist >= self.min_damage_range {
            return self.min_damage;
        }
        let span = self.min_damage_range - self.max_damage_range;
        if span == 0.0 {
            return self.damage;
        }
        let t = ((f64::from(dist) - f64::from(self.max_damage_range)) / f64::from(span)) as f32;
        let t = f64::from(t);
        let (full, min) = (
            f64::from(self.damage as f32),
            f64::from(self.min_damage as f32),
        );
        ((1.0 - t) * full + min * t) as i32
    }
}

/// The level's weapon table as `PM_Weapon` reads it: `bg_weaponDefs`,
/// index 0 the empty weapon.
pub trait Weapons {
    fn get(&self, index: u8) -> Option<&WeaponParams>;
    /// `bg_iNumWeapons`.
    fn count(&self) -> usize;
}

impl Weapons for Vec<Option<WeaponParams>> {
    fn get(&self, index: u8) -> Option<&WeaponParams> {
        self.as_slice()
            .get(usize::from(index))
            .and_then(Option::as_ref)
    }

    fn count(&self) -> usize {
        self.len().saturating_sub(1)
    }
}

/// The weapon fields of `playerState_s` beside the movement's.
#[derive(Clone, Debug, PartialEq)]
pub struct WeaponState {
    /// ps+0xd4.
    pub weapon: u8,
    /// ps+0xd0.
    pub offhand: u8,
    /// ps+0x34.
    pub weapon_time: i32,
    /// ps+0x38.
    pub weapon_delay: i32,
    /// ps+0x3c: the fuse left on a held grenade.
    pub grenade_time_left: i32,
    /// ps+0x40.
    pub restrict_kick_time: i32,
    /// ps+0xd8.
    pub state: i32,
    /// ps+0x5d0: the view anim, bit 0x200 its restart toggle.
    pub weap_anim: i32,
    /// ps+0xdc, `fWeaponPosFrac`: 0 at the hip, 1 down the sight.
    pub pos_frac: f32,
    /// ps+0xe0, `adsDelayTime`.
    pub ads_delay_time: i32,
    /// ps+0x5b4, `holdBreathScale`: the sway's scale, 1 at rest, 0 held,
    /// up to `player_breath_gasp_scale` after a gasp.
    pub hold_breath_scale: f32,
    /// ps+0x5b8, `holdBreathTimer`: ms the breath has been held, counting
    /// back down once let go.
    pub hold_breath_timer: i32,
    /// ps+0x544: the held weapons, bit `n` weapon `n`.
    pub held: u128,
    /// ps+0x55c: the weapons that owe a rechamber.
    pub rechamber: u128,
    /// ps+0x144 and ps+0x344, by ammo and clip index.
    pub ammo: [i32; 128],
    pub clip: [i32; 128],
}

impl Default for WeaponState {
    fn default() -> Self {
        WeaponState {
            weapon: 0,
            offhand: 0,
            weapon_time: 0,
            weapon_delay: 0,
            grenade_time_left: 0,
            restrict_kick_time: 0,
            state: state::READY,
            weap_anim: 0,
            pos_frac: 0.0,
            ads_delay_time: 0,
            hold_breath_scale: 0.0,
            hold_breath_timer: 0,
            held: 0,
            rechamber: 0,
            ammo: [0; 128],
            clip: [0; 128],
        }
    }
}

impl WeaponState {
    pub fn holds(&self, weapon: u8) -> bool {
        weapon < 128 && self.held >> weapon & 1 != 0
    }

    fn clip_of(&self, w: &WeaponParams) -> i32 {
        self.clip[w.clip_index]
    }

    fn ammo_of(&self, w: &WeaponParams) -> i32 {
        self.ammo[w.ammo_index]
    }

    fn owes_rechamber(&self, weapon: u8) -> bool {
        weapon < 128 && self.rechamber >> weapon & 1 != 0
    }

    fn set_rechamber(&mut self, weapon: u8, on: bool) {
        if weapon < 128 {
            if on {
                self.rechamber |= 1 << weapon;
            } else {
                self.rechamber &= !(1 << weapon);
            }
        }
    }
}

/// What the step needs from `pmove_t` and `pml_t`.
pub struct WeaponCmd<'a> {
    pub cmd: &'a UserCmd2,
    pub oldcmd: &'a UserCmd2,
    /// `pml.msec`.
    pub msec: i32,
    /// `player_adsExitDelay` (0 by default).
    pub ads_exit_delay: i32,
    /// `pmove_t` +0xe0: the move's `PM_CheckDuck` went into or out of
    /// prone, which restarts a reload's torso anim.
    pub prone_change: bool,
    /// The anim script the torso events run on; `None` raises none.
    pub anim: Option<WeaponAnims<'a>>,
}

/// What `PM_Weapon`'s `BG_AnimScriptEvent` calls read: the script, and the
/// draw among a clause's commands (retail's `rand() % n`, only asked for
/// where a clause lists more than one).
#[derive(Clone, Copy)]
pub struct WeaponAnims<'a> {
    pub table: &'a AnimTable,
    pub pick: &'a dyn Fn(usize) -> usize,
}

/// The anim conditions `BG_AnimUpdatePlayerStateConditions` (Mac 0x182666)
/// sets before each step: `playerAnimType` and `weaponClass` of the
/// viewmodel weapon (`BG_GetViewmodelWeaponIndex`, Mac 0x7b226: the
/// binoculars while up, the offhand while one is in hand), weapon 0's
/// `none` with nothing up.
pub fn anim_input<'t>(
    table: &'t AnimTable,
    ps: &PlayerState,
    ws: &WeaponState,
    defs: &dyn Weapons,
) -> AnimInput<'t> {
    let w = defs.get(view_weapon(ps, ws, defs));
    AnimInput {
        table,
        player_anim_type: w.map_or(0, |w| w.player_anim_type),
        weapon_class: w.map_or(0, |w| w.weap_class),
        pick: None,
    }
}

/// `BG_AnimScriptEvent(ps, event, 0, force)` from `PM_Weapon` (Linux
/// 0x80d96ee), on the conditions of the weapon in view: the raise updates
/// them to the new weapon before its event (0x80ede6c), and nothing else
/// moves the viewmodel weapon between the step's start and an event.
fn torso_event(
    ps: &mut PlayerState,
    ws: &WeaponState,
    c: &WeaponCmd,
    defs: &dyn Weapons,
    event: Event,
    force: bool,
) {
    if let Some(a) = c.anim {
        let input = anim_input(a.table, ps, ws, defs);
        anim::script_event(
            ps,
            c.cmd.buttons,
            &input,
            event.name(),
            a.pick,
            (false, force),
        );
    }
}

/// `PM_StartWeaponAnim`: `n` with the toggle flipped, only while alive.
fn start_anim(ps: &PlayerState, ws: &mut WeaponState, n: i32) {
    if ps.pm_type < pm_type::DEAD {
        ws.weap_anim = ((ws.weap_anim >> 8 & 2) ^ 2) << 8 | n;
    }
}

/// `PM_ContinueWeaponAnim(ps, 0)`: back to idle unless already there.
fn idle_anim(ps: &PlayerState, ws: &mut WeaponState) {
    if ws.weap_anim & !0x200 != 0 {
        start_anim(ps, ws, 0);
    }
}

/// What `PmoveSingle`'s mounted arm runs where the others run `PM_Weapon`
/// (lnxded 0x80e8934 calls 0x80f0946, which is 0x80ef946): `pm_flags` 0x10
/// and 0x800 off, the binoculars left at once (`PM_ExitBinocularsQuick`
/// 0x80effda) from states 17..22, the weapon ready with no time or delay
/// and its anim restarted at 0. Every step on a gun, so `weapAnim`'s toggle
/// flips each cmd. VERIFIED, the calls and the stores, and the gunner's
/// capture (cod2-turrets.md 11).
fn mounted_reset(ps: &mut PlayerState, ws: &mut WeaponState) {
    ps.pm_flags &= !(wpf::OFFHAND | pmf::PRONE_SIGHT);
    if ws.state > 0x10 && ws.state < 0x17 {
        ws.pos_frac = 0.0;
        exit_sight(ps);
        ps.pm_flags &= !wpf::BINOCULARS;
        ps.add_event(ev::BINOCULAR_EXIT, 0);
    }
    ws.weapon_time = 0;
    ws.weapon_delay = 0;
    ws.state = state::READY;
    start_anim(ps, ws, 0);
}

/// `PM_ExitAimDownSight` (0x80ecc72).
fn exit_sight(ps: &mut PlayerState) {
    ps.add_event(ev::RESET_ADS, 0);
    ps.pm_flags &= !pmf::SIGHT;
}

/// The weapon the sight and the lerp read (`BG_GetViewmodelWeaponIndex`,
/// Mac 0x7b226): the binoculars while they are up, the offhand while one
/// is in hand, else the weapon.
pub(crate) fn view_weapon(ps: &PlayerState, ws: &WeaponState, defs: &dyn Weapons) -> u8 {
    if ps.pm_flags & wpf::BINOCULARS != 0 {
        binocular_weapon(defs)
    } else if ps.pm_flags & wpf::OFFHAND != 0 {
        ws.offhand
    } else {
        ws.weapon
    }
}

/// `PmoveSingle`'s `EF_FIRING` (lnxded 0x80e8934, past 0x80e8b56): cleared,
/// then set on a cmd holding the attack, out of the intermission and the
/// respawn hold, with the weapon ready or firing and rounds in its clip
/// (`PM_WeaponAmmoAvailable`). A gunner's carried rifle counts: the flag
/// is the mounted body's firing anim (cod2-turrets.md 6). VERIFIED.
pub fn update_firing_flag(
    ps: &mut PlayerState,
    ws: &WeaponState,
    cmd: &UserCmd2,
    defs: &dyn Weapons,
) {
    ps.e_flags &= !super::ef::FIRING;
    let ready = matches!(ws.state, state::READY | state::FIRING);
    let rounds = defs.get(ws.weapon).is_some_and(|w| ws.clip_of(w) != 0);
    if ps.pm_type != pm_type::INTERMISSION
        && ps.pm_flags & pmf::RESPAWNED == 0
        && ready
        && rounds
        && cmd.buttons & button::ATTACK != 0
    {
        ps.e_flags |= super::ef::FIRING;
    }
}

/// `PM_UpdateAimDownSightFlag` (Mac 0x7bc60): `pm_flags` 0x40 follows the
/// sight key on a weapon with `aimDownSight`, alive, not raising, dropping,
/// meleeing or on an offhand. Prone it takes 0x840, and only on the key's
/// press or a still body.
pub fn update_sight_flag(
    ps: &mut PlayerState,
    ws: &WeaponState,
    cmd: &UserCmd2,
    oldcmd: &UserCmd2,
    defs: &dyn Weapons,
) {
    let before = ps.pm_flags;
    ps.pm_flags &= !pmf::SIGHT;
    if !matches!(ps.pm_type, pm_type::NORMAL | pm_type::NORMAL_LINKED) {
        return;
    }
    let Some(w) = defs.get(view_weapon(ps, ws, defs)) else {
        return;
    };
    let st = ws.state;
    if !w.aim_down_sight
        || (state::OFFHAND_INIT..=state::OFFHAND).contains(&st)
        || matches!(st, state::MELEE_INIT | state::MELEE_RELAX)
        || matches!(st, state::RAISING | state::DROPPING)
        || cmd.buttons & button::ADS == 0
    {
        return;
    }
    if before & pmf::PRONE == 0 {
        ps.pm_flags |= pmf::SIGHT;
    } else if oldcmd.buttons & button::ADS == 0 || (cmd.forward_move == 0 && cmd.right_move == 0) {
        ps.pm_flags |= pmf::SIGHT | pmf::PRONE_SIGHT;
    }
}

/// `PM_UpdateAimDownSightLerp` (Mac 0x7c762): `fWeaponPosFrac` toward 1 at
/// `1 / adsTransInTime` a ms while sighted, back at `1 / adsTransOutTime`,
/// held at the hip through a reload past `adsReloadTransTime` (not done:
/// the time is not carried) and a rechamber unless `rechamberWhileAds`.
fn update_sight_lerp(ps: &PlayerState, ws: &mut WeaponState, c: &WeaponCmd, defs: &dyn Weapons) {
    let Some(w) = defs.get(view_weapon(ps, ws, defs)) else {
        ws.pos_frac = 0.0;
        ws.ads_delay_time = 0;
        return;
    };
    if !w.aim_down_sight {
        ws.pos_frac = 0.0;
        ws.ads_delay_time = 0;
        return;
    }
    let st = ws.state;
    let reloading = if w.segmented_reload {
        (state::RELOADING..=state::RELOAD_END).contains(&st)
    } else {
        st == state::RELOADING
    };
    let mut want = !reloading
        && !(!w.rechamber_while_ads && st == state::RECHAMBERING)
        && ps.pm_flags & pmf::SIGHT != 0;
    if w.ads_fire && ws.weapon_delay != 0 && st == state::FIRING {
        want = true;
    }
    // `msec * rate + frac` stays on the x87 stack and rounds once, at the
    // store (cod2-combat.md 9.1).
    let msec = f64::from(c.msec);
    if want {
        ws.ads_delay_time = 0;
        if ws.pos_frac != 1.0 {
            let f = (msec * f64::from(w.ads_in_rate) + f64::from(ws.pos_frac)) as f32;
            ws.pos_frac = f.clamp(0.0, 1.0);
        }
        return;
    }
    if ws.pos_frac == 1.0 && c.ads_exit_delay > 0 {
        if ws.ads_delay_time == 0 {
            ws.ads_delay_time = c.ads_exit_delay + c.cmd.server_time;
        }
        if c.cmd.server_time < ws.ads_delay_time {
            return;
        }
    }
    ws.ads_delay_time = 0;
    if ws.pos_frac != 0.0 {
        let f = (f64::from(ws.pos_frac) - msec * f64::from(w.ads_out_rate)) as f32;
        ws.pos_frac = f.clamp(0.0, 1.0);
    }
}

/// Whether the breath hold applies: fully down the sight of a weapon with
/// an `adsOverlayReticle` that is not an `item` (0x80ed084's and
/// 0x80ecf9c's shared test).
fn breath_weapon(ps: &PlayerState, ws: &WeaponState, defs: &dyn Weapons) -> bool {
    ws.pos_frac == 1.0
        && defs
            .get(view_weapon(ps, ws, defs))
            .is_some_and(|w| w.ads_overlay_reticle != 0 && w.weap_class != 9)
}

/// `DiffTrack` (0x80a3cea): `cur` toward `goal` by `rate * (goal - cur) *
/// dt`, landing on `goal` once within 0.001 or the step would pass it.
fn diff_track(goal: f32, cur: f32, rate: f32, dt: f32) -> f32 {
    let diff = goal - cur;
    let step = (f64::from(rate) * f64::from(diff) * f64::from(dt)) as f32;
    if diff.abs() > f32::from_bits(0x3a83_126f) && diff.abs() >= step.abs() {
        cur + step
    } else {
        goal
    }
}

/// The breath hold (0x80ed084, after the sight lerp in `PM_Weapon`): the
/// hold key down the sight of a scoped weapon holds the breath while the
/// timer is 0, the timer counts the hold up and back down, a hold past
/// `player_breath_hold_time` ends in a gasp (the timer set to hold plus
/// gasp time), and `holdBreathScale` tracks 0 while held, else
/// `1 + (gasp_scale - 1) * timer / (hold + gasp)`, both scaled by
/// `fWeaponPosFrac`. VERIFIED (cod2-movement.md 18).
pub fn update_hold_breath(
    ps: &mut PlayerState,
    ws: &mut WeaponState,
    c: &WeaponCmd,
    defs: &dyn Weapons,
    b: &Breath,
) {
    let hold = breath_ms(b.hold_time);
    let gasp = breath_ms(b.gasp_time);
    if hold < 1 {
        ps.pm_flags &= !wpf::HOLD_BREATH;
        ws.hold_breath_scale = 1.0;
        ws.hold_breath_timer = 0;
        return;
    }
    if !breath_weapon(ps, ws, defs) || c.cmd.buttons & key::HOLD_BREATH == 0 {
        ps.pm_flags &= !wpf::HOLD_BREATH;
    } else if ws.hold_breath_timer == 0 {
        ps.pm_flags |= wpf::HOLD_BREATH;
    }
    let held = ps.pm_flags & wpf::HOLD_BREATH != 0;
    ws.hold_breath_timer = (ws.hold_breath_timer + if held { c.msec } else { -c.msec }).max(0);
    if held && ws.hold_breath_timer > hold {
        ws.hold_breath_timer = gasp + hold;
        ps.pm_flags &= !wpf::HOLD_BREATH;
    }
    let (target, rate) = if ps.pm_flags & wpf::HOLD_BREATH == 0 {
        // The ratio of the two ints is stored as a float (0x80ed26a); the
        // rest stays on the x87 stack to the store.
        let ratio = (f64::from(ws.hold_breath_timer) / f64::from(gasp + hold)) as f32;
        let t = (f64::from(b.gasp_scale) - 1.0) * f64::from(ratio) + 1.0;
        (t as f32, b.gasp_lerp)
    } else {
        (0.0, b.hold_lerp)
    };
    let goal = ((f64::from(target) - 1.0) * f64::from(ws.pos_frac) + 1.0) as f32;
    let dt = (f64::from(f32::from_bits(0x3a83_126f)) * f64::from(c.msec)) as f32;
    ws.hold_breath_scale = diff_track(goal, ws.hold_breath_scale, rate, dt);
}

/// A shot's breath (0x80ecf9c, after its fire event): down a scoped sight
/// it lets the breath go, adding `player_breath_fire_delay` to the timer up
/// to the hold time. VERIFIED.
fn breath_on_fire(ps: &mut PlayerState, ws: &mut WeaponState, defs: &dyn Weapons, b: &Breath) {
    if !breath_weapon(ps, ws, defs) {
        return;
    }
    let hold = breath_ms(b.hold_time);
    if ws.hold_breath_timer < hold {
        ws.hold_breath_timer = (ws.hold_breath_timer + breath_ms(b.fire_delay)).min(hold);
    }
    ps.pm_flags &= !wpf::HOLD_BREATH;
}

/// `bg_aimSpreadMoveSpeedThreshold`'s default (`Dvar_RegisterFloat`,
/// 0x41300000).
const MOVE_SPREAD_THRESHOLD: f32 = 11.0;

/// `PM_AdjustAimSpreadScale` (0x80ee9ee): `aimSpreadScale` (0..255) decays
/// by `hipSpreadDecayRate` (halved in the air, scaled crouched or prone)
/// and grows by the turn, the move and the air; down the sight it only
/// decays. VERIFIED, constants rodata 0x8155764..0x8155774.
pub fn adjust_aim_spread(
    ps: &mut PlayerState,
    ws: &WeaponState,
    c: &WeaponCmd,
    defs: &dyn Weapons,
) {
    let frametime = c.msec as f32 * 0.001;
    let (inc, dec) = match defs.get(ws.weapon) {
        Some(w) if w.hip_spread_decay_rate != 0.0 => {
            let airborne = !ps.on_ground() && ps.pm_type != pm_type::NORMAL_LINKED;
            let mut rate = w.hip_spread_decay_rate;
            if airborne {
                rate *= 0.5;
            } else if ps.e_flags & 0x8 != 0 {
                rate *= w.hip_spread_prone_decay;
            } else if ps.e_flags & 0x4 != 0 {
                rate *= w.hip_spread_ducked_decay;
            }
            let dec = rate * frametime;
            let inc = if ws.pos_frac == 1.0 {
                0.0
            } else {
                let mut add = 0.0f32;
                if w.hip_spread_turn_add != 0.0 && frametime > 0.0 {
                    for k in 0..2 {
                        let short = 360.0 / 65536.0;
                        let d = angle_subtract(
                            c.cmd.angles[k] as f32 * short,
                            c.oldcmd.angles[k] as f32 * short,
                        );
                        add += 0.01 * d.abs() * w.hip_spread_turn_add / frametime;
                    }
                }
                let moving = c.cmd.forward_move != 0 || c.cmd.right_move != 0;
                if w.hip_spread_move_add != 0.0
                    && moving
                    && ps.velocity.truncate().length() > MOVE_SPREAD_THRESHOLD
                {
                    add += w.hip_spread_move_add;
                }
                if airborne {
                    add += 1.28 * 2.0;
                }
                add * frametime
            };
            (inc, dec)
        }
        _ => (0.0, 1.0),
    };
    ps.aim_spread_scale = (ps.aim_spread_scale + 255.0 * (inc - dec)).clamp(0.0, 255.0);
}

/// `AngleSubtract`: `a - b` folded into (-180, 180].
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

/// `BG_GetSpreadForWeapon` (0x80ec8cc): the hip spread's min and max at the
/// eye's height, stand to crouch above 40, crouch to prone below. VERIFIED,
/// rodata 0x8155714..0x8155720.
pub fn spread_for_weapon(view_height: f32, w: &WeaponParams) -> (f32, f32) {
    if view_height > 40.0 {
        let f = (view_height - 40.0) / 20.0;
        (
            (w.hip_spread_stand_min - w.hip_spread_ducked_min) * f + w.hip_spread_ducked_min,
            (w.hip_spread_max - w.hip_spread_ducked_max) * f + w.hip_spread_ducked_max,
        )
    } else {
        let f = (view_height - 11.0) / 29.0;
        (
            (w.hip_spread_ducked_min - w.hip_spread_prone_min) * f + w.hip_spread_prone_min,
            (w.hip_spread_ducked_max - w.hip_spread_prone_max) * f + w.hip_spread_prone_max,
        )
    }
}

/// `FireWeaponAntiLag`'s spread (0x8120870): down the sight from
/// `adsSpread`, else from the hip's min, toward the hip's max by
/// `aimSpreadScale / 255` (`ClientEndFrame`'s `gclient+0x2810`).
pub fn shot_spread(
    view_height: f32,
    pos_frac: f32,
    aim_spread_scale: f32,
    w: &WeaponParams,
) -> f32 {
    let (min, max) = spread_for_weapon(view_height, w);
    let scale = aim_spread_scale / 255.0;
    if pos_frac == 1.0 {
        w.ads_spread + (max - w.ads_spread) * scale
    } else {
        min + (max - min) * scale
    }
}

/// `PM_Weapon_AllowReload` (Mac 0x7ac60).
fn allow_reload(ws: &WeaponState, w: &WeaponParams) -> bool {
    let (reserve, clip) = (ws.ammo_of(w), ws.clip_of(w));
    if reserve == 0 || clip >= w.clip_size {
        return false;
    }
    if w.no_partial_reload {
        if w.reload_ammo_add == 0 || w.reload_ammo_add >= w.clip_size {
            return clip == 0;
        }
        return w.clip_size - clip >= w.reload_ammo_add;
    }
    true
}

/// `PM_ReloadClip` (Mac 0x7aba4): rounds from the reserve into the clip,
/// `reloadStartAdd` at a segmented start, `reloadAmmoAdd` a segment.
fn reload_clip(ws: &mut WeaponState, w: &WeaponParams) {
    let start = matches!(
        ws.state,
        state::RELOAD_START | state::RELOAD_START_INTERRUPTED
    );
    if start && w.reload_start_add == 0 {
        return;
    }
    let (reserve, clip) = (ws.ammo_of(w), ws.clip_of(w));
    let mut n = (w.clip_size - clip).min(reserve);
    if start {
        if w.reload_start_add < w.clip_size && w.reload_start_add < n {
            n = w.reload_start_add;
        }
    } else if w.reload_ammo_add != 0 && w.reload_ammo_add < w.clip_size && w.reload_ammo_add < n {
        n = w.reload_ammo_add;
    }
    if n == 0 {
        return;
    }
    ws.ammo[w.ammo_index] = reserve - n;
    ws.clip[w.clip_index] = clip + n;
}

/// The add time of the reload the state names: the start's
/// `reloadStartAddTime` (capped by `reloadStartTime`), else the reload's
/// `reloadAddTime` capped by its time.
fn reload_add_time(ws: &WeaponState, w: &WeaponParams) -> i32 {
    if matches!(
        ws.state,
        state::RELOAD_START | state::RELOAD_START_INTERRUPTED
    ) {
        let t = w.reload_start_add_time;
        if t != 0 && w.reload_start_time <= t {
            w.reload_start_time
        } else {
            t
        }
    } else {
        let mut t = if ws.clip_of(w) == 0 && w.is_bullet() {
            w.reload_empty_time
        } else {
            w.reload_time
        };
        if w.reload_add_time != 0 && w.reload_add_time < t {
            t = w.reload_add_time;
        }
        t
    }
}

/// The delay a reload's ammo lands on: the add time, or for a bolt-action
/// rifle that owes a rechamber its bolt time first (the shared tail of
/// `PM_SetReloadingState` and `PM_BeginWeaponReload`).
fn set_reload_delay(ws: &mut WeaponState, w: &WeaponParams) {
    let t = reload_add_time(ws, w);
    if w.bolt_action && ws.owes_rechamber(ws.weapon) {
        let t = if t == 0 { ws.weapon_time } else { t };
        ws.weapon_delay = t.min(w.rechamber_bolt_time).max(1);
    } else if t != 0 {
        ws.weapon_delay = t;
    }
}

/// `PM_SetReloadingState` (Mac 0x7c3d6).
fn set_reloading_state(ps: &mut PlayerState, ws: &mut WeaponState, w: &WeaponParams) {
    if ws.clip_of(w) == 0 && w.is_bullet() {
        start_anim(ps, ws, 12);
        ws.weapon_time = w.reload_empty_time;
        ps.add_event(ev::RELOAD_FROM_EMPTY, 0);
    } else {
        start_anim(ps, ws, 11);
        ws.weapon_time = w.reload_time;
        ps.add_event(ev::RELOAD, 0);
    }
    ws.state = if ws.state == state::RELOAD_START_INTERRUPTED {
        state::RELOADING_INTERRUPTED
    } else {
        state::RELOADING
    };
    set_reload_delay(ws, w);
}

/// `PM_BeginWeaponReload` (Mac 0x7c5a2, Linux 0x80edb04): the forced
/// `reload` torso event unless the weapon is clip-only.
fn begin_reload(
    ps: &mut PlayerState,
    ws: &mut WeaponState,
    w: &WeaponParams,
    c: &WeaponCmd,
    defs: &dyn Weapons,
) {
    if !matches!(ws.state, state::READY | state::FIRING | state::RECHAMBERING) || ws.weapon == 0 {
        return;
    }
    if !w.clip_only {
        torso_event(ps, ws, c, defs, Event::Reload, true);
    }
    ps.add_event(ev::RESET_ADS, 0);
    if !w.segmented_reload || w.reload_start_time == 0 {
        set_reloading_state(ps, ws, w);
        return;
    }
    start_anim(ps, ws, 13);
    ws.weapon_time = w.reload_start_time;
    ws.state = state::RELOAD_START;
    ps.add_event(ev::RELOAD_START, 0);
    set_reload_delay(ws, w);
}

/// `PM_Weapon_ReloadDelayedAction` (Mac 0x7b114): a bolt-action rifle that
/// owes a rechamber ejects its brass first, then the rounds land.
fn reload_delayed_action(ps: &mut PlayerState, ws: &mut WeaponState, w: &WeaponParams) {
    if w.bolt_action && ws.owes_rechamber(ws.weapon) {
        ws.set_rechamber(ws.weapon, false);
        ps.add_event(ev::EJECT_BRASS, 0);
        let start = matches!(
            ws.state,
            state::RELOAD_START | state::RELOAD_START_INTERRUPTED
        );
        if start && w.reload_start_add_time == 0 {
            return;
        }
        if ws.weapon_time != 0 {
            let t = reload_add_time(ws, w);
            let bolt = if t <= w.rechamber_bolt_time {
                1
            } else {
                w.rechamber_bolt_time
            };
            if t - bolt > 0 {
                ws.weapon_delay = t - bolt;
                return;
            }
        }
    }
    reload_clip(ws, w);
}

/// `PM_BeginWeaponChange` (Mac 0x7c110, Linux 0x80edc30), without the
/// primary slots' bookkeeping: a weapon going down that is not swapping to
/// its alt raises the unforced `dropweapon` torso event, unless mantling.
fn begin_change(
    ps: &mut PlayerState,
    ws: &mut WeaponState,
    new: u8,
    c: &WeaponCmd,
    defs: &dyn Weapons,
) {
    if !(new == 0 || ws.holds(new)) || ws.state == state::DROPPING {
        return;
    }
    ws.weapon_delay = 0;
    let cur = ws.weapon;
    if cur != 0
        && ws.holds(cur)
        && ws.grenade_time_left < 1
        && let Some(w) = defs.get(cur)
    {
        let alt = new != 0 && new == w.alt_index;
        let has_rounds = !w.clip_only || ws.clip_of(w) != 0;
        ws.grenade_time_left = 0;
        if alt {
            ps.add_event(ev::WEAPON_ALT, 0);
            start_anim(ps, ws, 15);
        } else if has_rounds {
            ps.add_event(ev::PUTAWAY_WEAPON, 0);
            start_anim(ps, ws, 9);
        }
        if !alt && ps.pm_flags & pmf::MANTLE == 0 {
            torso_event(ps, ws, c, defs, Event::DropWeapon, false);
        }
        ws.state = state::DROPPING;
        ws.weapon_time = if alt { w.alt_drop_time } else { w.drop_time };
        return;
    }
    ws.weapon_time = 0;
    ws.state = state::DROPPING;
    ws.grenade_time_left = 0;
}

/// `PM_Weapon` for one step of `c.msec` (see the module header for what is
/// left out).
pub fn pm_weapon(ps: &mut PlayerState, ws: &mut WeaponState, c: &WeaponCmd, defs: &dyn Weapons) {
    // `PmoveSingle`'s default arm (`pm_type` 0 and 6) takes the mounted
    // reset in place of `PM_Weapon`, so a gunner killed on the gun keeps
    // his weapon until his release (cod2-turrets.md 14.1).
    if ps.e_flags & 0x300 != 0 && matches!(ps.pm_type, pm_type::NORMAL | pm_type::DEAD) {
        mounted_reset(ps, ws);
        return;
    }
    if ps.pm_flags & pmf::RESPAWNED != 0 {
        return;
    }
    if ps.pm_type > pm_type::INTERMISSION {
        ws.weapon = 0;
        return;
    }
    if ps.e_flags & 0x300 != 0 {
        mounted_reset(ps, ws);
        return;
    }
    update_sight_lerp(ps, ws, c, defs);
    update_hold_breath(ps, ws, c, defs, &Breath::DEFAULT);
    let (buttons, oldbuttons) = (c.cmd.buttons, c.oldcmd.buttons);
    let attack = buttons & button::ATTACK != 0;
    let msec = c.msec;
    // A cooked grenade that runs out in hand.
    if ps.pm_flags & wpf::OFFHAND != 0
        && let Some(o) = defs.get(ws.offhand)
        && o.is_grenade()
        && ws.grenade_time_left > 0
        && o.cook_off_hold
    {
        ws.grenade_time_left -= msec;
        if ws.grenade_time_left < 1 {
            ws.clip[o.clip_index] -= 1;
            ps.add_event(ev::GRENADE_SUICIDE, 0);
            return;
        }
    }
    if ws.restrict_kick_time > 0 {
        ws.restrict_kick_time = (ws.restrict_kick_time - msec).max(0);
    }
    let cur = defs.get(ws.weapon).cloned();
    if ws.weapon_time != 0 {
        ws.weapon_time -= msec;
        if ws.weapon_time < 1 {
            let st = ws.state;
            let hold = !(state::OFFHAND_INIT..=state::OFFHAND).contains(&st)
                && cur
                    .as_ref()
                    .is_some_and(|w| w.semi_auto && ws.clip_of(w) != 0)
                && attack
                && ws.weapon == c.cmd.weapon;
            if !hold {
                ws.weapon_time = 0;
            } else {
                // A semi-automatic holds at 1 while the trigger stays down.
                ws.weapon_time = 1;
                if (state::RELOADING..=state::RELOAD_END).contains(&st) {
                    ws.weapon_time = 0;
                }
                if matches!(
                    st,
                    state::FIRING | state::RECHAMBERING | state::MELEE_INIT | state::MELEE_RELAX
                ) {
                    idle_anim(ps, ws);
                    ws.state = state::READY;
                }
            }
        }
    }
    let delay_fired = if ws.weapon_delay != 0 {
        ws.weapon_delay -= msec;
        if ws.weapon_delay > 0 {
            false
        } else {
            ws.weapon_delay = 0;
            true
        }
    } else {
        false
    };
    binoculars(ps, ws, c, defs);
    // The offhand keys: select the offhand the cmd names, then start a
    // throw with the first held grenade of the key's class.
    if !(state::OFFHAND_INIT..=state::OFFHAND_START).contains(&ws.state) && !in_binoculars(ws.state)
    {
        if ws.holds(c.cmd.off_hand_index) {
            ws.offhand = c.cmd.off_hand_index;
        }
        let class = if buttons & key::FRAG != 0 {
            1
        } else if buttons & key::SMOKE != 0 {
            2
        } else {
            0
        };
        if class != 0 {
            let pick = (1..=defs.count().min(127) as u8).find(|&i| {
                defs.get(i).is_some_and(|o| {
                    o.offhand_class == class && ws.holds(i) && ws.ammo_of(o) + ws.clip_of(o) != 0
                })
            });
            match pick {
                Some(i) => {
                    ps.add_event(ev::SWITCH_OFFHAND, i32::from(i));
                    ws.offhand = i;
                    if ws.weapon == 0 || ws.state == state::OFFHAND {
                        prepare_offhand(ps, ws, defs);
                    } else {
                        ws.state = state::OFFHAND_INIT;
                        ws.weapon_time = cur.as_ref().map_or(0, |w| w.quick_drop_time);
                        ws.weapon_delay = 0;
                        ps.pm_flags &= !wpf::OFFHAND;
                        exit_sight(ps);
                        start_anim(ps, ws, 0x11);
                    }
                }
                None => ps.add_event(ev::EMPTY_OFFHAND, 0),
            }
        }
    }
    // The switch the cmd's weapon byte asks for.
    let st = ws.state;
    if !(state::OFFHAND_INIT..=state::OFFHAND).contains(&st)
        && !in_binoculars(st)
        && (ws.weapon_time == 0
            || (state::RELOADING..=state::RELOAD_END).contains(&st)
            || st == state::RECHAMBERING
            || (!matches!(
                st,
                state::FIRING | state::RECHAMBERING | state::MELEE_INIT | state::MELEE_RELAX
            ) && ws.weapon_delay == 0))
    {
        let want = c.cmd.weapon;
        // Mantling, on the rungs or with weapons disabled the gun goes down
        // (Linux 0x80eef06, cod2-combat.md 1.7).
        if holstered(ps) {
            if ws.weapon != 0 {
                begin_change(ps, ws, 0, c, defs);
            }
        } else if ws.weapon != want && (ps.pm_flags & 0x8000 == 0 || ws.weapon == 0) {
            let valid = want == 0 || (ws.holds(want) && usize::from(want) <= defs.count());
            if valid {
                begin_change(ps, ws, want, c, defs);
            } else if ws.weapon != 0 && !ws.holds(ws.weapon) {
                begin_change(ps, ws, 0, c, defs);
            }
        } else if ws.weapon != 0 && !ws.holds(ws.weapon) {
            begin_change(ps, ws, 0, c, defs);
        }
    }
    let cur = defs.get(ws.weapon).cloned();
    // The reload key, an empty clip, and a segmented reload's interrupt.
    if !(state::OFFHAND_INIT..=state::OFFHAND).contains(&ws.state)
        && !in_binoculars(ws.state)
        && let Some(w) = cur.as_ref()
    {
        let mut reload = buttons & key::RELOAD != 0;
        if ps.pm_flags & wpf::RELOAD_REQUEST != 0 {
            ps.pm_flags &= !wpf::RELOAD_REQUEST;
            reload = true;
        }
        let st = ws.state;
        if w.segmented_reload
            && matches!(st, state::RELOAD_START | state::RELOADING)
            && attack
            && oldbuttons & button::ATTACK == 0
        {
            ws.state = if st == state::RELOAD_START {
                state::RELOAD_START_INTERRUPTED
            } else {
                state::RELOADING_INTERRUPTED
            };
        } else if matches!(st, state::READY | state::FIRING | state::RECHAMBERING) {
            let moving_prone =
                ps.pm_flags & pmf::PRONE != 0 && (c.cmd.forward_move != 0 || c.cmd.right_move != 0);
            let empty = ws.clip_of(w) == 0 && ws.ammo_of(w) != 0 && st != state::FIRING;
            if (empty && !moving_prone) || (reload && allow_reload(ws, w)) {
                begin_reload(ps, ws, w, c, defs);
            }
        }
        // Linux 0x80ee724: a reload under way restarts its torso anim on a
        // prone change.
        if (state::RELOADING..=state::RELOAD_END).contains(&ws.state)
            && c.prone_change
            && !w.clip_only
        {
            torso_event(ps, ws, c, defs, Event::Reload, true);
        }
    }
    // The melee key.
    if let Some(w) = cur.as_ref()
        && w.melee_damage != 0
        && !delay_fired
        && !(state::OFFHAND_INIT..=state::OFFHAND).contains(&ws.state)
        && !in_binoculars(ws.state)
        && (ws.weapon_delay == 0 || (state::RELOADING..=state::RELOAD_END).contains(&ws.state))
    {
        if buttons & key::MELEE == 0 {
            ps.pm_flags &= !wpf::MELEE_HELD;
        } else if ps.pm_flags & wpf::MELEE_HELD == 0 {
            ps.pm_flags |= wpf::MELEE_HELD;
            let scoped = ws.pos_frac > 0.0 && w.ads_overlay_reticle != 0;
            if !scoped
                && !matches!(
                    ws.state,
                    state::RAISING | state::DROPPING | state::MELEE_INIT | state::MELEE_RELAX
                )
            {
                // Linux 0x80ef74e: the forced `meleeattack` torso event.
                torso_event(ps, ws, c, defs, Event::MeleeAttack, true);
                start_anim(ps, ws, 8);
                ps.add_event(ev::MELEE_SWIPE, 0);
                if w.melee_delay == 0 {
                    ws.weapon_time = ws.weapon_time.max(w.melee_time - w.melee_delay);
                    ps.add_event(ev::FIRE_MELEE, 0);
                    ws.state = state::MELEE_RELAX;
                } else {
                    ws.weapon_time = w.melee_time;
                    ws.weapon_delay = w.melee_delay;
                    ws.state = state::MELEE_INIT;
                }
            }
        }
    }
    // A bolt-action rifle's rechamber after its shot.
    if let Some(w) = defs.get(ws.weapon).cloned()
        && !(state::OFFHAND_INIT..=state::OFFHAND).contains(&ws.state)
        && !in_binoculars(ws.state)
        && w.bolt_action
        && ws.owes_rechamber(ws.weapon)
    {
        let mut skip = false;
        if ws.state == state::RECHAMBERING && delay_fired {
            ws.set_rechamber(ws.weapon, false);
            ps.add_event(ev::EJECT_BRASS, 0);
            if ws.weapon_time != 0 {
                return;
            }
        } else if ws.weapon_time != 0
            && (matches!(
                ws.state,
                state::FIRING | state::RECHAMBERING | state::MELEE_INIT | state::MELEE_RELAX
            ) || ws.weapon_delay != 0)
        {
            skip = true;
        }
        if !skip {
            if ws.state == state::RECHAMBERING {
                idle_anim(ps, ws);
                ws.state = state::READY;
            } else if ws.state == state::READY {
                start_anim(ps, ws, if ws.pos_frac <= 0.75 { 4 } else { 7 });
                ws.state = state::RECHAMBERING;
                ws.weapon_time = w.rechamber_time;
                ws.weapon_delay =
                    if w.rechamber_bolt_time == 0 || w.rechamber_time <= w.rechamber_bolt_time {
                        1
                    } else {
                        w.rechamber_bolt_time
                    };
                ps.add_event(ev::RECHAMBER_WEAPON, 0);
            }
        }
    }
    let moving = c.cmd.forward_move != 0 || c.cmd.right_move != 0;
    if (ps.pm_flags & pmf::PRONE != 0 && moving)
        || matches!(ws.state, state::MELEE_INIT | state::MELEE_RELAX)
    {
        ps.aim_spread_scale = 255.0;
    }
    if !delay_fired && (ws.weapon_time != 0 || ws.weapon_delay != 0) {
        return;
    }
    run_state(ps, ws, c, defs, delay_fired);
}

/// The quick drop's end, or an empty hand's offhand key: the grenade comes
/// up and the pin comes out (state 13).
/// `Mantle_IsWeaponInactive`, the ladder or `pm_flags` 0x4000000: the
/// weapon stays down.
fn holstered(ps: &PlayerState) -> bool {
    super::mantle::weapon_inactive(ps) || ps.pm_flags & (pmf::LADDER | wpf::DISABLED) != 0
}

fn prepare_offhand(ps: &mut PlayerState, ws: &mut WeaponState, defs: &dyn Weapons) {
    let hold = defs.get(ws.offhand).map_or(0, |o| o.hold_fire_time);
    ws.state = state::OFFHAND_PREPARE;
    ws.weapon_time = hold;
    ws.weapon_delay = 0;
    ps.pm_flags |= wpf::OFFHAND;
    ps.add_event(ev::PREP_OFFHAND, i32::from(ws.offhand));
    start_anim(ps, ws, 0x13);
}

/// `PM_Weapon`'s closing switch on `weaponstate` once both timers ran out.
fn run_state(
    ps: &mut PlayerState,
    ws: &mut WeaponState,
    c: &WeaponCmd,
    defs: &dyn Weapons,
    delay_fired: bool,
) {
    let buttons = c.cmd.buttons;
    match ws.state {
        state::RAISING | state::RELOAD_END | state::MELEE_RELAX => to_ready(ps, ws),
        state::DROPPING => {
            // 0x80ede6c: nothing comes up mantling or on the rungs.
            let want = c.cmd.weapon;
            let new = if !holstered(ps) && ws.holds(want) {
                want
            } else {
                0
            };
            let old = ws.weapon;
            ws.weapon = new;
            if new == old {
                ws.state = state::READY;
                start_anim(ps, ws, 0);
            } else {
                let w = defs.get(new);
                let alt = new != 0 && defs.get(old).is_some_and(|o| o.alt_index == new);
                ws.state = state::RAISING;
                ws.weapon_time = w.map_or(0, |w| if alt { w.alt_raise_time } else { w.raise_time });
                if old != 0 && !alt {
                    ps.add_event(ev::RAISE_WEAPON, 0);
                    // The conditions take the new weapon, then the unforced
                    // `raiseweapon` event (0x80ede6c; empty in the stock
                    // script). Out of empty hands neither runs.
                    torso_event(ps, ws, c, defs, Event::RaiseWeapon, false);
                }
                if !alt {
                    ps.aim_spread_scale = 255.0;
                    start_anim(ps, ws, 10);
                } else {
                    ps.aim_spread_scale = ps.aim_spread_scale.max(128.0);
                    start_anim(ps, ws, 0x10);
                }
            }
        }
        state::RELOADING | state::RELOADING_INTERRUPTED => {
            let Some(w) = defs.get(ws.weapon).cloned() else {
                return to_ready(ps, ws);
            };
            if delay_fired {
                reload_delayed_action(ps, ws, &w);
            }
            if ws.weapon_time != 0 {
                return;
            }
            ws.set_rechamber(ws.weapon, false);
            if !w.segmented_reload {
                return to_ready(ps, ws);
            }
            if ws.state != state::RELOADING_INTERRUPTED && allow_reload(ws, &w) {
                return set_reloading_state(ps, ws, &w);
            }
            end_reload(ps, ws, &w);
        }
        state::RELOAD_START | state::RELOAD_START_INTERRUPTED => {
            let Some(w) = defs.get(ws.weapon).cloned() else {
                return to_ready(ps, ws);
            };
            if delay_fired {
                reload_delayed_action(ps, ws, &w);
            }
            if ws.weapon_time != 0 {
                return;
            }
            if (ws.state != state::RELOAD_START_INTERRUPTED || ws.clip_of(&w) == 0)
                && allow_reload(ws, &w)
            {
                return set_reloading_state(ps, ws, &w);
            }
            ws.set_rechamber(ws.weapon, false);
            end_reload(ps, ws, &w);
        }
        state::MELEE_INIT => {
            if let Some(w) = defs.get(ws.weapon) {
                ws.weapon_time = ws.weapon_time.max(w.melee_time - w.melee_delay);
            }
            ps.add_event(ev::FIRE_MELEE, 0);
            ws.state = state::MELEE_RELAX;
        }
        state::OFFHAND_INIT => prepare_offhand(ps, ws, defs),
        state::OFFHAND_PREPARE => {
            ws.state = state::OFFHAND_HOLD;
            ws.weapon_time = 0;
            ws.weapon_delay = 0;
            ps.pm_flags |= wpf::OFFHAND;
            ws.grenade_time_left = defs.get(ws.offhand).map_or(0, |o| o.fuse_time);
        }
        state::OFFHAND_HOLD => {
            const KEYS: i32 = key::FRAG | key::SMOKE;
            if c.oldcmd.buttons & KEYS == 0 || buttons & KEYS == 0 {
                let o = defs.get(ws.offhand).cloned();
                ws.state = state::OFFHAND_START;
                ws.weapon_time = o.as_ref().map_or(0, |o| o.fire_time);
                ws.weapon_delay = 0;
                ps.pm_flags |= wpf::OFFHAND;
                ps.add_event(ev::USE_OFFHAND, i32::from(ws.offhand));
                start_anim(ps, ws, 2);
                // The throw takes its grenade out of the clip here, in
                // pmove (`PM_Weapon` Mac 0x7cbb2, state 14's arm), before
                // the forced `fireweapon` torso event (Linux 0x80efb12);
                // `G_UseOffHand` takes nothing.
                if let Some(o) = &o {
                    ws.clip[o.clip_index] -= 1;
                }
                torso_event(ps, ws, c, defs, Event::FireWeapon, true);
                if let Some(o) = o
                    && ws.ammo_of(&o) + ws.clip_of(&o) == 0
                {
                    ps.add_event(ev::EMPTY_OFFHAND, 0);
                }
            } else {
                // Held: a delay of 1 asks again next step.
                ws.weapon_delay = 1;
            }
        }
        state::OFFHAND_START => {
            match defs.get(ws.weapon) {
                Some(w) if ws.weapon != 0 => {
                    ws.weapon_time = w.quick_raise_time;
                    ws.weapon_delay = 0;
                    start_anim(ps, ws, 0x12);
                }
                _ => {
                    ws.weapon_time = 0;
                    ws.weapon_delay = 1;
                }
            }
            ws.state = state::OFFHAND;
            ps.pm_flags &= !(wpf::OFFHAND | pmf::PRONE_SIGHT);
        }
        state::OFFHAND => {
            ps.pm_flags &= !(wpf::OFFHAND | pmf::PRONE_SIGHT);
            ws.weapon_time = 0;
            ws.weapon_delay = 0;
            to_ready(ps, ws);
        }
        st if in_binoculars(st) => binocular_state(ps, ws, c, defs),
        _ => fire(ps, ws, c, defs, delay_fired),
    }
}

/// States 17..22.
pub fn in_binoculars(st: i32) -> bool {
    (state::BINOCULARS_ADS_EXIT..=state::BINOCULARS_RAISE_WEAPON).contains(&st)
}

/// `PM_IsBinocularsADS` (Mac 0x7a04a): states 19 and 20, where the client
/// sends the sight bit on its own (cod2-protocol.md, the button bits).
pub fn is_binoculars_ads(ws: &WeaponState) -> bool {
    matches!(ws.state, state::BINOCULARS_RAISE | state::BINOCULARS_HOLD)
}

/// The first weapon whose `weaponType` is binoculars (3), the loop
/// `PM_Weapon` runs over `bg_weaponDefs`; 0 for none.
pub fn binocular_weapon(defs: &dyn Weapons) -> u8 {
    (1..=defs.count().min(127) as u8)
        .find(|&i| defs.get(i).is_some_and(|w| w.weap_type == 3))
        .unwrap_or(0)
}

/// Out of the binoculars at once (Mac 0x7cbb2, the interrupt): the sight
/// reset, `pm_flags` 0x20000 off with the exit event, the weapon ready.
fn leave_binoculars(ps: &mut PlayerState, ws: &mut WeaponState) {
    ps.pm_flags &= !(wpf::OFFHAND | pmf::PRONE_SIGHT);
    ws.pos_frac = 0.0;
    ps.add_event(ev::RESET_ADS, 0);
    ps.pm_flags &= !(wpf::BINOCULARS | pmf::SIGHT);
    ps.add_event(ev::BINOCULAR_EXIT, 0);
    ws.weapon_time = 0;
    ws.weapon_delay = 0;
    ws.state = state::READY;
    start_anim(ps, ws, 0);
}

/// `PM_Weapon`'s binocular block (Mac 0x7cbb2, before the offhand keys):
/// from ready (not down a scoped sight, no fire, melee, reload or offhand
/// key) or inside the binoculars. Inside, a changed weapon or offhand byte
/// or one of those keys drops them at once (a fire key leaves a
/// `weaponTime` of 1). Without the fire key, held up (20) the use key's
/// press and release raise `BINOCULAR_FIRE` and `_RELEASE`; the
/// `+binoculars` press (`player_toggleBinoculars` 1, its default) goes in
/// from ready and out from 18..20. In: with no weapon the binoculars come
/// up (19) at once, `adsTransInTime`, `pm_flags` 0x20000 and
/// `BINOCULAR_ENTER`; down the sight the sight is left first (17) for the
/// weapon's `adsTransOutTime`; else the weapon goes down (18) for its
/// `quickDropTime`. Out: still dropping the weapon (18) it comes straight
/// back (22) with `BINOCULAR_EXIT`; else the binoculars go down (21) for
/// their `adsTransOutTime` with `BINOCULAR_DROP`. VERIFIED, the stores;
/// the hold mode (`player_toggleBinoculars` 0) and the interrupt's
/// `ps+0x128` test are not read.
pub fn binoculars(ps: &mut PlayerState, ws: &mut WeaponState, c: &WeaponCmd, defs: &dyn Weapons) {
    const KEYS: i32 = button::ATTACK | key::MELEE | key::RELOAD | key::FRAG | key::SMOKE;
    let (buttons, old) = (c.cmd.buttons, c.oldcmd.buttons);
    let st = ws.state;
    let inside = in_binoculars(st);
    let scoped = ws.pos_frac > 0.0
        && defs
            .get(ws.weapon)
            .is_some_and(|w| w.ads_overlay_reticle != 0);
    if !inside && (st != state::READY || scoped || buttons & KEYS != 0) {
        return;
    }
    let changed =
        c.cmd.weapon != c.oldcmd.weapon || c.cmd.off_hand_index != c.oldcmd.off_hand_index;
    if inside && (changed || buttons & KEYS != 0) {
        leave_binoculars(ps, ws);
        if buttons & button::ATTACK != 0 {
            ws.weapon_time = 1;
        }
        return;
    }
    if buttons & button::ATTACK != 0 {
        return;
    }
    if st == state::BINOCULARS_HOLD {
        if old & key::USE == 0 && buttons & key::USE != 0 {
            ps.add_event(ev::BINOCULAR_FIRE, 0);
        } else if old & key::USE != 0 && buttons & key::USE == 0 {
            ps.add_event(ev::BINOCULAR_RELEASE, 0);
        }
    }
    let pressed = old & key::BINOCULARS == 0 && buttons & key::BINOCULARS != 0;
    if !pressed {
        return;
    }
    if !inside {
        let binoc = binocular_weapon(defs);
        if binoc == 0 {
            return;
        }
        if c.cmd.weapon == 0 {
            ws.state = state::BINOCULARS_RAISE;
            ws.weapon_time = defs.get(binoc).map_or(0, |b| b.ads_trans_in_time);
            ws.weapon_delay = 0;
            ps.pm_flags |= wpf::BINOCULARS;
            ps.add_event(ev::BINOCULAR_ENTER, 0);
            start_anim(ps, ws, 0);
        } else if buttons & button::ADS == 0 {
            ws.state = state::BINOCULARS_DROP_WEAPON;
            ws.weapon_time = defs.get(ws.weapon).map_or(0, |w| w.quick_drop_time);
            ws.weapon_delay = 0;
            start_anim(ps, ws, 0x11);
        } else {
            let out = defs.get(ws.weapon).map_or(0, |w| w.ads_trans_out_time);
            exit_sight(ps);
            ws.weapon_time = 0;
            ws.weapon_delay = out + c.ads_exit_delay;
            ws.state = state::BINOCULARS_ADS_EXIT;
        }
        return;
    }
    if !(state::BINOCULARS_DROP_WEAPON..=state::BINOCULARS_HOLD).contains(&st) {
        return;
    }
    if st == state::BINOCULARS_DROP_WEAPON {
        ws.state = state::BINOCULARS_RAISE_WEAPON;
        ps.pm_flags &= !wpf::BINOCULARS;
        ps.add_event(ev::BINOCULAR_EXIT, 0);
        raise_weapon_after(ps, ws, c, defs);
    } else {
        let binoc = binocular_weapon(defs);
        ws.state = state::BINOCULARS_DROP;
        ws.weapon_time = defs.get(binoc).map_or(0, |b| b.ads_trans_out_time) + c.ads_exit_delay;
        ws.weapon_delay = 0;
        ps.add_event(ev::BINOCULAR_DROP, 0);
        start_anim(ps, ws, 0);
    }
}

/// The weapon back up after the binoculars: its `quickRaiseTime`, or with
/// none in the cmd a delay of 1.
fn raise_weapon_after(
    ps: &mut PlayerState,
    ws: &mut WeaponState,
    c: &WeaponCmd,
    defs: &dyn Weapons,
) {
    if c.cmd.weapon != 0 {
        ws.weapon_time = defs.get(ws.weapon).map_or(0, |w| w.quick_raise_time);
        ws.weapon_delay = 0;
        start_anim(ps, ws, 0x12);
    } else {
        ws.weapon_time = 0;
        ws.weapon_delay = 1;
    }
}

/// The closing switch's binocular arms, once the timers ran out.
fn binocular_state(ps: &mut PlayerState, ws: &mut WeaponState, c: &WeaponCmd, defs: &dyn Weapons) {
    match ws.state {
        state::BINOCULARS_ADS_EXIT => {
            ws.state = state::BINOCULARS_DROP_WEAPON;
            ws.weapon_time = defs.get(ws.weapon).map_or(0, |w| w.quick_drop_time);
            ws.weapon_delay = 0;
            start_anim(ps, ws, 0x11);
        }
        state::BINOCULARS_DROP_WEAPON => {
            let binoc = binocular_weapon(defs);
            ws.state = state::BINOCULARS_RAISE;
            ws.weapon_time = defs.get(binoc).map_or(0, |b| b.ads_trans_in_time);
            ws.weapon_delay = 0;
            ps.pm_flags |= wpf::BINOCULARS;
            ps.add_event(ev::BINOCULAR_ENTER, 0);
            start_anim(ps, ws, 0);
        }
        state::BINOCULARS_RAISE => {
            ws.state = state::BINOCULARS_HOLD;
            ws.weapon_time = 0;
            ws.weapon_delay = 0;
        }
        state::BINOCULARS_DROP => {
            ws.state = state::BINOCULARS_RAISE_WEAPON;
            ps.pm_flags &= !wpf::BINOCULARS;
            ps.add_event(ev::BINOCULAR_EXIT, 0);
            raise_weapon_after(ps, ws, c, defs);
        }
        // The weapon is back (case 0x16): the sight reset and a second
        // exit event, then ready.
        state::BINOCULARS_RAISE_WEAPON => leave_binoculars(ps, ws),
        _ => {}
    }
}

fn to_ready(ps: &PlayerState, ws: &mut WeaponState) {
    ws.state = state::READY;
    start_anim(ps, ws, 0);
}

/// A segmented reload's end: `reloadEndTime` in state 9, or ready.
fn end_reload(ps: &mut PlayerState, ws: &mut WeaponState, w: &WeaponParams) {
    if w.reload_end_time != 0 {
        ws.state = state::RELOAD_END;
        start_anim(ps, ws, 0xe);
        ws.weapon_time = w.reload_end_time;
        ps.add_event(ev::RELOAD_END, 0);
    } else {
        to_ready(ps, ws);
    }
}

/// The ready state's arm: a shot (Linux 0x80ef58a with its helpers
/// 0x80ef1ac, 0x80ef380, 0x80ef496, 0x80ef50a, 0x80ee95e).
fn fire(
    ps: &mut PlayerState,
    ws: &mut WeaponState,
    c: &WeaponCmd,
    defs: &dyn Weapons,
    delay_fired: bool,
) {
    if ws.weapon == 0 {
        return;
    }
    if c.cmd.buttons & button::ATTACK == 0 && !delay_fired {
        if ws.state == state::FIRING {
            idle_anim(ps, ws);
        }
        ws.state = state::READY;
        return;
    }
    let Some(w) = defs.get(ws.weapon).cloned() else {
        return;
    };
    // 0x80ef1ac: the times, the pullback of a grenade.
    if w.is_grenade() {
        if !delay_fired {
            if ws.clip_of(&w) != 0 {
                ws.grenade_time_left = w.fuse_time;
                start_anim(ps, ws, 0x13);
                ps.add_event(ev::PULLBACK_WEAPON, 0);
            }
            ws.weapon_delay = w.hold_fire_time;
            ws.weapon_time = 0;
        }
    } else {
        ws.weapon_delay = w.fire_delay;
        ws.weapon_time = w.fire_time;
        if w.ads_fire {
            ws.weapon_delay =
                ((1.0 / w.ads_in_rate) * (1.0 - ws.pos_frac)).round_ties_even() as i32;
        }
        // The forced `fireweapon` torso event, an empty clip's click too.
        torso_event(ps, ws, c, defs, Event::FireWeapon, true);
        if w.bolt_action {
            ws.set_rechamber(ws.weapon, true);
        }
        if ws.state != state::FIRING {
            let bullets = if ws.pos_frac >= 1.0 {
                w.ads_kick_reduced_bullets
            } else {
                w.hip_kick_reduced_bullets
            };
            ws.restrict_kick_time = w.fire_delay + w.fire_time * bullets;
        }
    }
    ws.state = state::FIRING;
    // 0x80ef380: an empty clip reloads or clicks.
    if ws.clip_of(&w) < 1 {
        let dry = ws.ammo_of(&w) < 1;
        if !w.is_grenade() && dry {
            ps.add_event(ev::NOAMMO, 0);
        }
        if dry {
            ws.set_rechamber(ws.weapon, false);
            idle_anim(ps, ws);
            ws.weapon_time += 500;
        } else {
            begin_reload(ps, ws, &w, c, defs);
        }
        return;
    }
    if ws.weapon_delay != 0 {
        return;
    }
    if ps.e_flags & 0x300 == 0 {
        ws.clip[w.clip_index] -= 1;
    }
    if w.is_grenade() {
        ws.weapon_time = w.fire_time;
    }
    let last = ws.clip_of(&w) == 0;
    // 0x80ef496: the sighted or hip fire anim, the last shot's own.
    let n = match (ws.pos_frac > 0.75, last) {
        (true, true) => 6,
        (true, false) => 5,
        (false, true) => 3,
        (false, false) => 2,
    };
    start_anim(ps, ws, n);
    ps.add_event(
        if last {
            ev::FIRE_WEAPON_LASTSHOT
        } else {
            ev::FIRE_WEAPON
        },
        0,
    );
    breath_on_fire(ps, ws, defs, &Breath::DEFAULT);
    // 0x80ef50a: the hip's spread grows per shot.
    if ws.pos_frac != 1.0 {
        ps.aim_spread_scale = (ps.aim_spread_scale + 255.0 * w.hip_spread_fire_add).min(255.0);
    }
    // 0x80ee95e: a clip-only weapon with nothing left is taken.
    if w.clip_only && ws.clip_of(&w) == 0 && ws.ammo_of(&w) == 0 {
        ws.held &= !(1u128 << ws.weapon);
        ps.add_event(ev::NOAMMO, 0);
    }
}

/// The level's weapon table off `CS_WEAPONS`' names (weapon `n` is
/// `names[n - 1]`), each weapon's ammo and clip index by
/// [`crate::weapon::clip_indices`] (`BG_SetupAmmoIndexes`,
/// `BG_SetupClipIndexes`) and its `altWeapon` by name. A file that does not
/// load is `None`, and so is index 0.
pub fn table(
    fs: &(impl vcod_common::asset_source::AssetSource + ?Sized),
    names: &[&str],
) -> Vec<Option<WeaponParams>> {
    let defs: Vec<Option<WeaponDef>> = names
        .iter()
        .map(|n| crate::weapon::load(fs, n).ok().map(|p| p.def))
        .collect();
    let index_of = |key: &str| {
        crate::weapon::clip_indices(defs.iter().map(|d| d.as_ref().map_or("", |d| d.str(key))))
    };
    let (ammo, clip) = (index_of("ammoName"), index_of("clipName"));
    let mut out = vec![None];
    for (k, d) in defs.iter().enumerate() {
        out.push(d.as_ref().map(|d| {
            let alt = names
                .iter()
                .position(|n| *n == d.str("altWeapon"))
                .map_or(0, |i| i as u8 + 1);
            WeaponParams::from_def(d, ammo[k], clip[k], alt)
        }));
    }
    out
}

/// One cmd of `PmoveSingle` with its weapon half, as vcod-server's
/// `client_think` runs it: `EF_FIRING`, the sight flag and the aim spread,
/// the move at the held weapon's `moveSpeedScale`, then `PM_Weapon` and
/// `ClientEvents`' zeroing of a thrown grenade's fuse (cod2-combat.md 2.6).
/// Only a living or dead body runs the weapon. The client predicts with
/// it; the movement A/B replays retail's captures through it.
pub fn armed_step(
    ps: &mut PlayerState,
    ws: &mut WeaponState,
    cmd: &UserCmd2,
    oldcmd: &mut UserCmd2,
    world: vcod_common::movetrace::MoveWorld,
    rules: crate::pmove::Rules,
    defs: &dyn Weapons,
) {
    let old = *oldcmd;
    let armed = matches!(
        ps.pm_type,
        pm_type::NORMAL | pm_type::NORMAL_LINKED | pm_type::DEAD | pm_type::DEAD_LINKED
    );
    let msec = if armed {
        (cmd.server_time - ps.command_time).clamp(0, 200)
    } else {
        0
    };
    let wc = WeaponCmd {
        cmd,
        oldcmd: &old,
        msec,
        ads_exit_delay: 0,
        prone_change: false,
        anim: None,
    };
    update_firing_flag(ps, ws, cmd, defs);
    if msec > 0 {
        update_sight_flag(ps, ws, cmd, &old, defs);
        adjust_aim_spread(ps, ws, &wc, defs);
    }
    // `PM_CmdScale_Walk` (0x80e1c58) scales by the held weapon.
    let scale = move_scale(defs, ws.weapon);
    crate::pmove::player_step(ps, cmd, oldcmd, world, rules, scale);
    if msec > 0 {
        let seq = ps.event_sequence;
        pm_weapon(ps, ws, &wc, defs);
        ps.weapon_state = ws.state;
        if (seq..ps.event_sequence).any(|i| ps.events[(i & 3) as usize] == ev::USE_OFFHAND) {
            ws.grenade_time_left = 0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A kar98k-like bolt rifle (1), an smg (2) and a frag (3).
    fn table() -> Vec<Option<WeaponParams>> {
        let rifle = WeaponParams {
            name: "rifle".into(),
            ammo_index: 1,
            clip_index: 1,
            clip_size: 5,
            fire_time: 100,
            rechamber_time: 1000,
            rechamber_bolt_time: 300,
            reload_time: 2000,
            reload_add_time: 1500,
            raise_time: 300,
            drop_time: 300,
            bolt_action: true,
            ..Default::default()
        };
        let smg = WeaponParams {
            name: "smg".into(),
            ammo_index: 2,
            clip_index: 2,
            clip_size: 30,
            fire_time: 100,
            reload_time: 2000,
            reload_empty_time: 2500,
            reload_add_time: 1500,
            quick_drop_time: 150,
            ..Default::default()
        };
        let frag = WeaponParams {
            name: "frag".into(),
            weap_type: 1,
            offhand_class: 1,
            ammo_index: 3,
            clip_index: 3,
            clip_size: 1,
            fire_time: 400,
            hold_fire_time: 200,
            fuse_time: 3500,
            ..Default::default()
        };
        vec![None, Some(rifle), Some(smg), Some(frag)]
    }

    fn holding(weapon: u8) -> WeaponState {
        let mut ws = WeaponState {
            weapon,
            held: 0b1110,
            ..Default::default()
        };
        ws.clip[1] = 5;
        ws.ammo[1] = 10;
        ws.clip[2] = 30;
        ws.ammo[2] = 60;
        ws.clip[3] = 1;
        ws
    }

    /// Steps `ms` of 50 ms cmds holding `buttons` on `weapon`; returns the
    /// events raised.
    fn run(
        ps: &mut PlayerState,
        ws: &mut WeaponState,
        defs: &Vec<Option<WeaponParams>>,
        buttons: i32,
        ms: i32,
        old: &mut UserCmd2,
    ) -> Vec<i32> {
        let mut out = Vec::new();
        for _ in 0..ms / 50 {
            let cmd = UserCmd2 {
                server_time: old.server_time + 50,
                buttons,
                weapon: ws.weapon,
                ..Default::default()
            };
            let seq = ps.event_sequence;
            let c = WeaponCmd {
                cmd: &cmd,
                oldcmd: old,
                msec: 50,
                ads_exit_delay: 0,
                prone_change: false,
                anim: None,
            };
            pm_weapon(ps, ws, &c, defs);
            out.extend((seq..ps.event_sequence).map(|i| ps.events[(i & 3) as usize]));
            *old = cmd;
        }
        out
    }

    #[test]
    fn a_bolt_rifle_fires_once_then_rechambers() {
        let defs = table();
        let mut ps = PlayerState::default();
        let mut ws = holding(1);
        let mut old = UserCmd2::default();
        let evs = run(&mut ps, &mut ws, &defs, button::ATTACK, 50, &mut old);
        assert_eq!(evs, [ev::FIRE_WEAPON]);
        assert_eq!(ws.clip[1], 4);
        // Released: the rechamber runs, the bolt ejects at 300 ms.
        let evs = run(&mut ps, &mut ws, &defs, 0, 1200, &mut old);
        assert_eq!(evs, [ev::RECHAMBER_WEAPON, ev::EJECT_BRASS]);
        assert_eq!(ws.state, state::READY);
        let evs = run(&mut ps, &mut ws, &defs, button::ATTACK, 50, &mut old);
        assert_eq!(evs, [ev::FIRE_WEAPON]);
    }

    #[test]
    fn an_smg_fires_every_fire_time_while_held() {
        let defs = table();
        let mut ps = PlayerState::default();
        let mut ws = holding(2);
        let mut old = UserCmd2::default();
        let evs = run(&mut ps, &mut ws, &defs, button::ATTACK, 1000, &mut old);
        assert_eq!(evs.iter().filter(|&&e| e == ev::FIRE_WEAPON).count(), 10);
        assert_eq!(ws.clip[2], 20);
        assert!(ps.aim_spread_scale >= 0.0);
    }

    #[test]
    fn an_empty_clip_reloads_and_the_rounds_land_on_the_add_time() {
        let defs = table();
        let mut ps = PlayerState::default();
        let mut ws = holding(2);
        ws.clip[2] = 1;
        let mut old = UserCmd2::default();
        let evs = run(&mut ps, &mut ws, &defs, button::ATTACK, 50, &mut old);
        assert_eq!(evs, [ev::FIRE_WEAPON_LASTSHOT]);
        // The fire time runs out, the state goes ready, then the reload.
        let evs = run(&mut ps, &mut ws, &defs, 0, 150, &mut old);
        assert_eq!(evs, [ev::RESET_ADS, ev::RELOAD_FROM_EMPTY]);
        assert_eq!(ws.state, state::RELOADING);
        run(&mut ps, &mut ws, &defs, 0, 1450, &mut old);
        assert_eq!((ws.clip[2], ws.ammo[2]), (0, 60));
        run(&mut ps, &mut ws, &defs, 0, 100, &mut old);
        assert_eq!((ws.clip[2], ws.ammo[2]), (30, 30));
        run(&mut ps, &mut ws, &defs, 0, 1000, &mut old);
        assert_eq!(ws.state, state::READY);
    }

    #[test]
    fn the_frag_key_drops_the_gun_holds_the_grenade_and_throws_on_release() {
        let defs = table();
        let mut ps = PlayerState::default();
        let mut ws = holding(2);
        let mut old = UserCmd2::default();
        let evs = run(&mut ps, &mut ws, &defs, key::FRAG, 50, &mut old);
        assert_eq!(evs, [ev::SWITCH_OFFHAND, ev::RESET_ADS]);
        assert_eq!((ws.state, ws.offhand), (state::OFFHAND_INIT, 3));
        let evs = run(&mut ps, &mut ws, &defs, key::FRAG, 500, &mut old);
        assert_eq!(evs, [ev::PREP_OFFHAND]);
        assert_eq!(ws.state, state::OFFHAND_HOLD);
        assert_eq!(ws.grenade_time_left, 3500);
        // The release takes the last grenade out of the clip and says so.
        let evs = run(&mut ps, &mut ws, &defs, 0, 50, &mut old);
        assert_eq!(evs, [ev::USE_OFFHAND, ev::EMPTY_OFFHAND]);
        assert_eq!(ws.clip[3], 0);
        run(&mut ps, &mut ws, &defs, 0, 1000, &mut old);
        assert_eq!(ws.state, state::READY);
        assert_eq!(ps.pm_flags & wpf::OFFHAND, 0);
    }

    #[test]
    fn the_rungs_put_the_gun_away_and_fire_nothing() {
        let defs = table();
        let mut ps = PlayerState {
            pm_flags: pmf::LADDER,
            ..Default::default()
        };
        let mut ws = holding(1);
        let mut old = UserCmd2::default();
        let evs = run(&mut ps, &mut ws, &defs, 0, 50, &mut old);
        assert_eq!(evs, [ev::PUTAWAY_WEAPON]);
        assert_eq!((ws.state, ws.weap_anim & !0x200), (state::DROPPING, 9));
        // The drop ends on the empty hands; the cmd still asks for the rifle.
        let mut cmd_weapon_run = |ps: &mut PlayerState, ws: &mut WeaponState, buttons, ms| {
            let mut out = Vec::new();
            for _ in 0..ms / 50 {
                let cmd = UserCmd2 {
                    server_time: old.server_time + 50,
                    buttons,
                    weapon: 1,
                    ..Default::default()
                };
                let seq = ps.event_sequence;
                let c = WeaponCmd {
                    cmd: &cmd,
                    oldcmd: &old,
                    msec: 50,
                    ads_exit_delay: 0,
                    prone_change: false,
                    anim: None,
                };
                pm_weapon(ps, ws, &c, &defs);
                out.extend((seq..ps.event_sequence).map(|i| ps.events[(i & 3) as usize]));
                old = cmd;
            }
            out
        };
        let evs = cmd_weapon_run(&mut ps, &mut ws, 0, 300);
        assert_eq!(evs, [ev::RAISE_WEAPON]);
        assert_eq!(ws.weapon, 0);
        let evs = cmd_weapon_run(&mut ps, &mut ws, button::ATTACK, 500);
        assert!(evs.is_empty());
        assert_eq!((ws.weapon, ws.clip[1]), (0, 5));
        // Off the rungs the rifle comes up with no event.
        ps.pm_flags &= !pmf::LADDER;
        let evs = cmd_weapon_run(&mut ps, &mut ws, 0, 400);
        assert!(evs.is_empty());
        assert_eq!((ws.weapon, ws.state), (1, state::READY));
    }

    #[test]
    fn the_damage_falls_off_between_the_two_ranges() {
        let w = WeaponParams {
            damage: 100,
            min_damage: 50,
            max_damage_range: 1000.0,
            min_damage_range: 2000.0,
            ..Default::default()
        };
        assert_eq!(w.damage_at(500.0), 100);
        assert_eq!(w.damage_at(1500.0), 75);
        // 74.95 truncates.
        assert_eq!(w.damage_at(1501.0), 74);
        assert_eq!(w.damage_at(3000.0), 50);
    }

    /// The table with binoculars (4) beside the three.
    fn with_binoculars() -> Vec<Option<WeaponParams>> {
        let mut defs = table();
        if let Some(smg) = defs[2].as_mut() {
            smg.quick_raise_time = 100;
        }
        defs.push(Some(WeaponParams {
            name: "binoculars".into(),
            weap_type: 3,
            ads_trans_in_time: 300,
            ads_trans_out_time: 200,
            aim_down_sight: true,
            ..Default::default()
        }));
        defs
    }

    /// The `+binoculars` press puts the smg down (18), brings the
    /// binoculars up (19, `BINOCULAR_ENTER`, `pm_flags` 0x20000) and holds
    /// them (20), where the use key fires and releases; a second press puts
    /// them down (21, `BINOCULAR_DROP`), the smg comes back (22,
    /// `BINOCULAR_EXIT`) and goes ready with the sight reset and a second
    /// exit.
    #[test]
    fn the_binoculars_come_up_look_and_go_down() {
        let defs = with_binoculars();
        let mut ps = PlayerState::default();
        let mut ws = holding(2);
        let mut old = UserCmd2::default();
        let b = key::BINOCULARS;
        assert!(run(&mut ps, &mut ws, &defs, b, 50, &mut old).is_empty());
        assert_eq!(ws.state, state::BINOCULARS_DROP_WEAPON);
        let evs = run(&mut ps, &mut ws, &defs, b, 150, &mut old);
        assert_eq!(evs, [ev::BINOCULAR_ENTER]);
        assert_eq!(ws.state, state::BINOCULARS_RAISE);
        assert_ne!(ps.pm_flags & wpf::BINOCULARS, 0);
        run(&mut ps, &mut ws, &defs, 0, 300, &mut old);
        assert_eq!(ws.state, state::BINOCULARS_HOLD);
        run(&mut ps, &mut ws, &defs, button::ADS, 100, &mut old);
        let evs = run(
            &mut ps,
            &mut ws,
            &defs,
            button::ADS | key::USE,
            50,
            &mut old,
        );
        assert_eq!(evs, [ev::BINOCULAR_FIRE]);
        let evs = run(&mut ps, &mut ws, &defs, 0, 50, &mut old);
        assert_eq!(evs, [ev::BINOCULAR_RELEASE]);
        let evs = run(&mut ps, &mut ws, &defs, b, 50, &mut old);
        assert_eq!(evs, [ev::BINOCULAR_DROP]);
        assert_eq!(ws.state, state::BINOCULARS_DROP);
        let evs = run(&mut ps, &mut ws, &defs, 0, 200, &mut old);
        assert_eq!(evs, [ev::BINOCULAR_EXIT]);
        assert_eq!(
            (ws.state, ps.pm_flags & wpf::BINOCULARS),
            (state::BINOCULARS_RAISE_WEAPON, 0)
        );
        let evs = run(&mut ps, &mut ws, &defs, 0, 150, &mut old);
        assert_eq!(evs, [ev::RESET_ADS, ev::BINOCULAR_EXIT]);
        assert_eq!(ws.state, state::READY);
        assert_eq!(ws.clip[2], 30, "nothing fired");
    }

    /// The fire key drops the binoculars at once and fires nothing that
    /// step.
    #[test]
    fn the_fire_key_drops_the_binoculars() {
        let defs = with_binoculars();
        let mut ps = PlayerState::default();
        let mut ws = holding(2);
        let mut old = UserCmd2::default();
        run(&mut ps, &mut ws, &defs, key::BINOCULARS, 50, &mut old);
        run(&mut ps, &mut ws, &defs, 0, 500, &mut old);
        assert_eq!(ws.state, state::BINOCULARS_HOLD);
        let evs = run(&mut ps, &mut ws, &defs, button::ATTACK, 50, &mut old);
        assert_eq!(evs, [ev::RESET_ADS, ev::BINOCULAR_EXIT]);
        assert_eq!((ws.state, ws.weapon_time), (state::READY, 1));
        assert_eq!(ps.pm_flags & wpf::BINOCULARS, 0);
        assert_eq!(ws.clip[2], 30);
    }

    /// On a gun every step readies the weapon and restarts anim 0, so the
    /// toggle flips cmd by cmd and no round leaves the hands.
    #[test]
    fn a_gunner_restarts_the_idle_every_step() {
        let defs = with_binoculars();
        let mut ps = PlayerState {
            e_flags: 0x300,
            ..Default::default()
        };
        let mut ws = holding(2);
        ws.weapon_time = 300;
        let mut old = UserCmd2::default();
        let mut toggles = Vec::new();
        for _ in 0..3 {
            run(&mut ps, &mut ws, &defs, button::ATTACK, 50, &mut old);
            toggles.push(ws.weap_anim);
        }
        assert_eq!(toggles, [0x200, 0, 0x200]);
        assert_eq!((ws.state, ws.weapon_time), (state::READY, 0));
        assert_eq!(ws.clip[2], 30);
    }
}
