//! CoD2 1.3's weapon files (`weapons/mp/<name>`): the `WEAPONFILE` info
//! string read through the loader's field table into a [`WeaponDef`], and
//! the view weapon's anim tree numbering the cgame drives off `weapAnim`.
//! docs/research/cod2-weapons.md.

use anyhow::{Result, anyhow, bail};
use std::collections::HashMap;
use std::sync::OnceLock;
use vcod_common::asset_source::AssetSource;
use vcod_common::text::atoi;

/// A field's `cspFieldType`: 0..7 the shared types
/// (`ParseConfigStringToStruct`, Linux 0x80b8a36), 8..15 the weapon's own
/// (`BG_ParseWeaponDefSpecificFieldType`, 0x80f2de8), each a name looked up
/// case-insensitively in its table. Types 1..3 (fixed-length strings) name
/// no weapon field.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ty {
    /// 0: a pooled string.
    Str,
    /// 4: `atoi`.
    Int,
    /// 5: `atoi(v) != 0`.
    Bool,
    /// 6: `(float)atof`.
    Float,
    /// 7: seconds stored as whole milliseconds, truncated (1.8 is 1799).
    Ms,
    /// 8: [`WEAPON_TYPES`].
    WeapType,
    /// 9: [`WEAPON_CLASSES`].
    WeapClass,
    /// 10: [`OVERLAY_RETICLES`].
    OverlayReticle,
    /// 11: [`WEAPON_SLOTS`].
    Slot,
    /// 12: [`STANCES`].
    Stance,
    /// 13: [`PROJ_EXPLOSIONS`].
    ProjExplosion,
    /// 14: [`OFFHAND_CLASSES`].
    OffhandClass,
    /// 15: a name in `mp/playeranimtypes.txt`.
    PlayerAnimType,
}

impl Ty {
    /// The name table a specific type parses against; `PlayerAnimType`'s is
    /// the pak's, not the binary's.
    pub fn names(self) -> Option<&'static [&'static str]> {
        Some(match self {
            Ty::WeapType => &WEAPON_TYPES,
            Ty::WeapClass => &WEAPON_CLASSES,
            Ty::OverlayReticle => &OVERLAY_RETICLES,
            Ty::Slot => &WEAPON_SLOTS,
            Ty::Stance => &STANCES,
            Ty::ProjExplosion => &PROJ_EXPLOSIONS,
            Ty::OffhandClass => &OFFHAND_CLASSES,
            _ => return None,
        })
    }
}

/// `weaponType` (Linux 0x81869e0).
pub const WEAPON_TYPES: [&str; 4] = ["bullet", "grenade", "projectile", "binoculars"];
/// `weaponClass` (0x8186a20).
pub const WEAPON_CLASSES: [&str; 10] = [
    "rifle",
    "mg",
    "smg",
    "spread",
    "pistol",
    "grenade",
    "rocketlauncher",
    "turret",
    "non-player",
    "item",
];
/// `adsOverlayReticle` (0x8186a08).
pub const OVERLAY_RETICLES: [&str; 5] = ["none", "crosshair", "FG42", "Springfield", "binoculars"];
/// `weaponSlot` (0x81869f0).
pub const WEAPON_SLOTS: [&str; 3] = ["none", "primary", "primaryb"];
/// `stance` (0x8186a48).
pub const STANCES: [&str; 3] = ["stand", "duck", "prone"];
/// `projExplosionType` (0x81869fc).
pub const PROJ_EXPLOSIONS: [&str; 3] = ["grenade", "rocket", "none"];
/// `offhandClass` (0x8186a54).
pub const OFFHAND_CLASSES: [&str; 3] = ["None", "Frag Grenade", "Smoke Grenade"];

/// The `weapType` index of binoculars, which `BG_GetViewmodelWeaponIndex`
/// draws under `pm_flags` 0x20000.
pub const WEAPTYPE_BINOCULARS: i32 = 3;

/// `weaponDefFields` (Linux 0x8186a60, Mac 0x348c60): `{name, offset into
/// the 0x604-byte WeaponDef, type}` in table order, all 0x16e records.
/// `tools/re/dump_cod2_weapon_fields.py ... rust` prints these rows.
pub const FIELDS: [(&str, u16, Ty); 366] = [
    ("displayName", 0x4, Ty::Str),
    ("AIOverlayDescription", 0x8, Ty::Str),
    ("modeName", 0x70, Ty::Str),
    ("playerAnimType", 0x74, Ty::PlayerAnimType),
    ("gunModel", 0xc, Ty::Str),
    ("handModel", 0x10, Ty::Str),
    ("idleAnim", 0x18, Ty::Str),
    ("emptyIdleAnim", 0x1c, Ty::Str),
    ("fireAnim", 0x20, Ty::Str),
    ("holdFireAnim", 0x24, Ty::Str),
    ("lastShotAnim", 0x28, Ty::Str),
    ("rechamberAnim", 0x2c, Ty::Str),
    ("meleeAnim", 0x30, Ty::Str),
    ("reloadAnim", 0x34, Ty::Str),
    ("reloadEmptyAnim", 0x38, Ty::Str),
    ("reloadStartAnim", 0x3c, Ty::Str),
    ("reloadEndAnim", 0x40, Ty::Str),
    ("raiseAnim", 0x44, Ty::Str),
    ("dropAnim", 0x48, Ty::Str),
    ("altRaiseAnim", 0x4c, Ty::Str),
    ("altDropAnim", 0x50, Ty::Str),
    ("quickRaiseAnim", 0x54, Ty::Str),
    ("quickDropAnim", 0x58, Ty::Str),
    ("adsFireAnim", 0x5c, Ty::Str),
    ("adsLastShotAnim", 0x60, Ty::Str),
    ("adsRechamberAnim", 0x64, Ty::Str),
    ("adsUpAnim", 0x68, Ty::Str),
    ("adsDownAnim", 0x6c, Ty::Str),
    ("script", 0x584, Ty::Str),
    ("weaponType", 0x78, Ty::WeapType),
    ("weaponClass", 0x7c, Ty::WeapClass),
    ("weaponSlot", 0x80, Ty::Slot),
    ("slotStackable", 0x88, Ty::Bool),
    ("offhandClass", 0x84, Ty::OffhandClass),
    ("viewFlashEffect", 0x90, Ty::Str),
    ("worldFlashEffect", 0x94, Ty::Str),
    ("pickupSound", 0x98, Ty::Str),
    ("ammoPickupSound", 0x9c, Ty::Str),
    ("projectileSound", 0xa0, Ty::Str),
    ("pullbackSound", 0xa4, Ty::Str),
    ("fireSound", 0xa8, Ty::Str),
    ("fireSoundPlayer", 0xac, Ty::Str),
    ("loopFireSound", 0xb0, Ty::Str),
    ("loopFireSoundPlayer", 0xb4, Ty::Str),
    ("stopFireSound", 0xb8, Ty::Str),
    ("stopFireSoundPlayer", 0xbc, Ty::Str),
    ("lastShotSound", 0xc0, Ty::Str),
    ("lastShotSoundPlayer", 0xc4, Ty::Str),
    ("meleeSwipeSound", 0xc8, Ty::Str),
    ("rechamberSound", 0xcc, Ty::Str),
    ("rechamberSoundPlayer", 0xd0, Ty::Str),
    ("reloadSound", 0xd4, Ty::Str),
    ("reloadSoundPlayer", 0xd8, Ty::Str),
    ("reloadEmptySound", 0xdc, Ty::Str),
    ("reloadEmptySoundPlayer", 0xe0, Ty::Str),
    ("reloadStartSound", 0xe4, Ty::Str),
    ("reloadStartSoundPlayer", 0xe8, Ty::Str),
    ("reloadEndSound", 0xec, Ty::Str),
    ("reloadEndSoundPlayer", 0xf0, Ty::Str),
    ("raiseSound", 0xf4, Ty::Str),
    ("altSwitchSound", 0xf8, Ty::Str),
    ("putawaySound", 0xfc, Ty::Str),
    ("noteTrackSoundA", 0x100, Ty::Str),
    ("noteTrackSoundB", 0x104, Ty::Str),
    ("noteTrackSoundC", 0x108, Ty::Str),
    ("noteTrackSoundD", 0x10c, Ty::Str),
    ("shellEjectEffect", 0x110, Ty::Str),
    ("lastShotEjectEffect", 0x114, Ty::Str),
    ("reticleCenter", 0x118, Ty::Str),
    ("reticleSide", 0x11c, Ty::Str),
    ("reticleCenterSize", 0x120, Ty::Int),
    ("reticleSideSize", 0x124, Ty::Int),
    ("reticleMinOfs", 0x128, Ty::Int),
    ("standMoveF", 0x12c, Ty::Float),
    ("standMoveR", 0x130, Ty::Float),
    ("standMoveU", 0x134, Ty::Float),
    ("standRotP", 0x138, Ty::Float),
    ("standRotY", 0x13c, Ty::Float),
    ("standRotR", 0x140, Ty::Float),
    ("duckedOfsF", 0x144, Ty::Float),
    ("duckedOfsR", 0x148, Ty::Float),
    ("duckedOfsU", 0x14c, Ty::Float),
    ("duckedMoveF", 0x150, Ty::Float),
    ("duckedMoveR", 0x154, Ty::Float),
    ("duckedMoveU", 0x158, Ty::Float),
    ("duckedRotP", 0x15c, Ty::Float),
    ("duckedRotY", 0x160, Ty::Float),
    ("duckedRotR", 0x164, Ty::Float),
    ("proneOfsF", 0x168, Ty::Float),
    ("proneOfsR", 0x16c, Ty::Float),
    ("proneOfsU", 0x170, Ty::Float),
    ("proneMoveF", 0x174, Ty::Float),
    ("proneMoveR", 0x178, Ty::Float),
    ("proneMoveU", 0x17c, Ty::Float),
    ("proneRotP", 0x180, Ty::Float),
    ("proneRotY", 0x184, Ty::Float),
    ("proneRotR", 0x188, Ty::Float),
    ("posMoveRate", 0x18c, Ty::Float),
    ("posProneMoveRate", 0x190, Ty::Float),
    ("standMoveMinSpeed", 0x194, Ty::Float),
    ("duckedMoveMinSpeed", 0x198, Ty::Float),
    ("proneMoveMinSpeed", 0x19c, Ty::Float),
    ("posRotRate", 0x1a0, Ty::Float),
    ("posProneRotRate", 0x1a4, Ty::Float),
    ("standRotMinSpeed", 0x1a8, Ty::Float),
    ("duckedRotMinSpeed", 0x1ac, Ty::Float),
    ("proneRotMinSpeed", 0x1b0, Ty::Float),
    ("worldModel", 0x1b4, Ty::Str),
    ("hudIcon", 0x1b8, Ty::Str),
    ("modeIcon", 0x1bc, Ty::Str),
    ("startAmmo", 0x1c0, Ty::Int),
    ("ammoName", 0x1c4, Ty::Str),
    ("clipName", 0x1cc, Ty::Str),
    ("maxAmmo", 0x1d4, Ty::Int),
    ("clipSize", 0x1d8, Ty::Int),
    ("shotCount", 0x1dc, Ty::Int),
    ("sharedAmmoCapName", 0x1e0, Ty::Str),
    ("sharedAmmoCap", 0x1e8, Ty::Int),
    ("damage", 0x1ec, Ty::Int),
    ("playerDamage", 0x1f0, Ty::Int),
    ("meleeDamage", 0x1f4, Ty::Int),
    ("minDamage", 0x590, Ty::Int),
    ("minPlayerDamage", 0x594, Ty::Int),
    ("maxDamageRange", 0x598, Ty::Float),
    ("minDamageRange", 0x59c, Ty::Float),
    ("destabilizationBaseTime", 0x5a0, Ty::Float),
    ("destabilizationTimeReductionRatio", 0x5a4, Ty::Float),
    ("destabilizationAngleMax", 0x5a8, Ty::Float),
    ("destabilizeDistance", 0x5ac, Ty::Int),
    ("fireDelay", 0x1fc, Ty::Ms),
    ("meleeDelay", 0x200, Ty::Ms),
    ("fireTime", 0x204, Ty::Ms),
    ("rechamberTime", 0x208, Ty::Ms),
    ("rechamberBoltTime", 0x20c, Ty::Ms),
    ("holdFireTime", 0x210, Ty::Ms),
    ("meleeTime", 0x214, Ty::Ms),
    ("reloadTime", 0x218, Ty::Ms),
    ("reloadEmptyTime", 0x21c, Ty::Ms),
    ("reloadAddTime", 0x220, Ty::Ms),
    ("reloadStartTime", 0x224, Ty::Ms),
    ("reloadStartAddTime", 0x228, Ty::Ms),
    ("reloadEndTime", 0x22c, Ty::Ms),
    ("dropTime", 0x230, Ty::Ms),
    ("raiseTime", 0x234, Ty::Ms),
    ("altDropTime", 0x238, Ty::Ms),
    ("altRaiseTime", 0x23c, Ty::Ms),
    ("quickDropTime", 0x240, Ty::Ms),
    ("quickRaiseTime", 0x244, Ty::Ms),
    ("fuseTime", 0x248, Ty::Ms),
    ("autoAimRange", 0x24c, Ty::Float),
    ("slowdownAimRange", 0x250, Ty::Float),
    ("slowdownAimRangeAds", 0x254, Ty::Float),
    ("lockonAimRange", 0x258, Ty::Float),
    ("lockonAimRangeAds", 0x25c, Ty::Float),
    ("enemyCrosshairRange", 0x260, Ty::Float),
    ("moveSpeedScale", 0x264, Ty::Float),
    ("idleCrouchFactor", 0x2d8, Ty::Float),
    ("idleProneFactor", 0x2dc, Ty::Float),
    ("gunMaxPitch", 0x2e0, Ty::Float),
    ("gunMaxYaw", 0x2e4, Ty::Float),
    ("swayMaxAngle", 0x2e8, Ty::Float),
    ("swayLerpSpeed", 0x2ec, Ty::Float),
    ("swayPitchScale", 0x2f0, Ty::Float),
    ("swayYawScale", 0x2f4, Ty::Float),
    ("swayHorizScale", 0x2f8, Ty::Float),
    ("swayVertScale", 0x2fc, Ty::Float),
    ("swayShellShockScale", 0x300, Ty::Float),
    ("adsSwayMaxAngle", 0x304, Ty::Float),
    ("adsSwayLerpSpeed", 0x308, Ty::Float),
    ("adsSwayPitchScale", 0x30c, Ty::Float),
    ("adsSwayYawScale", 0x310, Ty::Float),
    ("adsSwayHorizScale", 0x314, Ty::Float),
    ("adsSwayVertScale", 0x318, Ty::Float),
    ("rifleBullet", 0x31c, Ty::Bool),
    ("armorPiercing", 0x320, Ty::Bool),
    ("semiAuto", 0x324, Ty::Bool),
    ("boltAction", 0x328, Ty::Bool),
    ("aimDownSight", 0x32c, Ty::Bool),
    ("rechamberWhileAds", 0x330, Ty::Bool),
    ("adsViewErrorMin", 0x334, Ty::Float),
    ("adsViewErrorMax", 0x338, Ty::Float),
    ("clipOnly", 0x340, Ty::Bool),
    ("cookOffHold", 0x33c, Ty::Bool),
    ("wideListIcon", 0x344, Ty::Bool),
    ("adsFire", 0x348, Ty::Bool),
    ("killIcon", 0x34c, Ty::Str),
    ("wideKillIcon", 0x350, Ty::Bool),
    ("flipKillIcon", 0x354, Ty::Bool),
    ("noPartialReload", 0x358, Ty::Bool),
    ("segmentedReload", 0x35c, Ty::Bool),
    ("reloadAmmoAdd", 0x360, Ty::Int),
    ("reloadStartAdd", 0x364, Ty::Int),
    ("altWeapon", 0x368, Ty::Str),
    ("dropAmmoMin", 0x370, Ty::Int),
    ("dropAmmoMax", 0x374, Ty::Int),
    ("explosionRadius", 0x378, Ty::Int),
    ("explosionInnerDamage", 0x37c, Ty::Int),
    ("explosionOuterDamage", 0x380, Ty::Int),
    ("projectileSpeed", 0x384, Ty::Int),
    ("projectileSpeedUp", 0x388, Ty::Int),
    ("projectileModel", 0x38c, Ty::Str),
    ("projExplosionType", 0x390, Ty::ProjExplosion),
    ("projExplosionEffect", 0x394, Ty::Str),
    ("projExplosionSound", 0x398, Ty::Str),
    ("projImpactExplode", 0x39c, Ty::Bool),
    ("parallelDefaultBounce", 0x3a0, Ty::Float),
    ("parallelBarkBounce", 0x3a4, Ty::Float),
    ("parallelBrickBounce", 0x3a8, Ty::Float),
    ("parallelCarpetBounce", 0x3ac, Ty::Float),
    ("parallelClothBounce", 0x3b0, Ty::Float),
    ("parallelConcreteBounce", 0x3b4, Ty::Float),
    ("parallelDirtBounce", 0x3b8, Ty::Float),
    ("parallelFleshBounce", 0x3bc, Ty::Float),
    ("parallelFoliageBounce", 0x3c0, Ty::Float),
    ("parallelGlassBounce", 0x3c4, Ty::Float),
    ("parallelGrassBounce", 0x3c8, Ty::Float),
    ("parallelGravelBounce", 0x3cc, Ty::Float),
    ("parallelIceBounce", 0x3d0, Ty::Float),
    ("parallelMetalBounce", 0x3d4, Ty::Float),
    ("parallelMudBounce", 0x3d8, Ty::Float),
    ("parallelPaperBounce", 0x3dc, Ty::Float),
    ("parallelPlasterBounce", 0x3e0, Ty::Float),
    ("parallelRockBounce", 0x3e4, Ty::Float),
    ("parallelSandBounce", 0x3e8, Ty::Float),
    ("parallelSnowBounce", 0x3ec, Ty::Float),
    ("parallelWaterBounce", 0x3f0, Ty::Float),
    ("parallelWoodBounce", 0x3f4, Ty::Float),
    ("parallelAsphaltBounce", 0x3f8, Ty::Float),
    ("perpendicularDefaultBounce", 0x3fc, Ty::Float),
    ("perpendicularBarkBounce", 0x400, Ty::Float),
    ("perpendicularBrickBounce", 0x404, Ty::Float),
    ("perpendicularCarpetBounce", 0x408, Ty::Float),
    ("perpendicularClothBounce", 0x40c, Ty::Float),
    ("perpendicularConcreteBounce", 0x410, Ty::Float),
    ("perpendicularDirtBounce", 0x414, Ty::Float),
    ("perpendicularFleshBounce", 0x418, Ty::Float),
    ("perpendicularFoliageBounce", 0x41c, Ty::Float),
    ("perpendicularGlassBounce", 0x420, Ty::Float),
    ("perpendicularGrassBounce", 0x424, Ty::Float),
    ("perpendicularGravelBounce", 0x428, Ty::Float),
    ("perpendicularIceBounce", 0x42c, Ty::Float),
    ("perpendicularMetalBounce", 0x430, Ty::Float),
    ("perpendicularMudBounce", 0x434, Ty::Float),
    ("perpendicularPaperBounce", 0x438, Ty::Float),
    ("perpendicularPlasterBounce", 0x43c, Ty::Float),
    ("perpendicularRockBounce", 0x440, Ty::Float),
    ("perpendicularSandBounce", 0x444, Ty::Float),
    ("perpendicularSnowBounce", 0x448, Ty::Float),
    ("perpendicularWaterBounce", 0x44c, Ty::Float),
    ("perpendicularWoodBounce", 0x450, Ty::Float),
    ("perpendicularAsphaltBounce", 0x454, Ty::Float),
    ("projTrailEffect", 0x458, Ty::Str),
    ("projectileDLight", 0x45c, Ty::Int),
    ("projectileRed", 0x460, Ty::Float),
    ("projectileGreen", 0x464, Ty::Float),
    ("projectileBlue", 0x468, Ty::Float),
    ("adsTransInTime", 0x2c0, Ty::Ms),
    ("adsTransOutTime", 0x2c4, Ty::Ms),
    ("adsIdleAmount", 0x2c8, Ty::Float),
    ("adsIdleSpeed", 0x2d0, Ty::Float),
    ("adsZoomFov", 0x268, Ty::Float),
    ("adsZoomInFrac", 0x26c, Ty::Float),
    ("adsZoomOutFrac", 0x270, Ty::Float),
    ("adsOverlayShader", 0x274, Ty::Str),
    ("adsOverlayReticle", 0x278, Ty::OverlayReticle),
    ("adsOverlayWidth", 0x27c, Ty::Float),
    ("adsOverlayHeight", 0x280, Ty::Float),
    ("adsBobFactor", 0x284, Ty::Float),
    ("adsViewBobMult", 0x288, Ty::Float),
    ("adsAimPitch", 0x46c, Ty::Float),
    ("adsCrosshairInFrac", 0x470, Ty::Float),
    ("adsCrosshairOutFrac", 0x474, Ty::Float),
    ("adsReloadTransTime", 0x524, Ty::Ms),
    ("adsGunKickReducedKickBullets", 0x478, Ty::Int),
    ("adsGunKickReducedKickPercent", 0x47c, Ty::Float),
    ("adsGunKickPitchMin", 0x480, Ty::Float),
    ("adsGunKickPitchMax", 0x484, Ty::Float),
    ("adsGunKickYawMin", 0x488, Ty::Float),
    ("adsGunKickYawMax", 0x48c, Ty::Float),
    ("adsGunKickAccel", 0x490, Ty::Float),
    ("adsGunKickSpeedMax", 0x494, Ty::Float),
    ("adsGunKickSpeedDecay", 0x498, Ty::Float),
    ("adsGunKickStaticDecay", 0x49c, Ty::Float),
    ("adsViewKickPitchMin", 0x4a0, Ty::Float),
    ("adsViewKickPitchMax", 0x4a4, Ty::Float),
    ("adsViewKickYawMin", 0x4a8, Ty::Float),
    ("adsViewKickYawMax", 0x4ac, Ty::Float),
    ("adsViewKickCenterSpeed", 0x4b0, Ty::Float),
    ("adsSpread", 0x4bc, Ty::Float),
    ("hipSpreadStandMin", 0x28c, Ty::Float),
    ("hipSpreadDuckedMin", 0x290, Ty::Float),
    ("hipSpreadProneMin", 0x294, Ty::Float),
    ("hipSpreadMax", 0x298, Ty::Float),
    ("hipSpreadDuckedMax", 0x29c, Ty::Float),
    ("hipSpreadProneMax", 0x2a0, Ty::Float),
    ("hipSpreadDecayRate", 0x2a4, Ty::Float),
    ("hipSpreadFireAdd", 0x2a8, Ty::Float),
    ("hipSpreadTurnAdd", 0x2ac, Ty::Float),
    ("hipSpreadMoveAdd", 0x2b0, Ty::Float),
    ("hipSpreadDuckedDecay", 0x2b4, Ty::Float),
    ("hipSpreadProneDecay", 0x2b8, Ty::Float),
    ("hipReticleSidePos", 0x2bc, Ty::Float),
    ("hipIdleAmount", 0x2cc, Ty::Float),
    ("hipIdleSpeed", 0x2d4, Ty::Float),
    ("hipGunKickReducedKickBullets", 0x4c0, Ty::Int),
    ("hipGunKickReducedKickPercent", 0x4c4, Ty::Float),
    ("hipGunKickPitchMin", 0x4c8, Ty::Float),
    ("hipGunKickPitchMax", 0x4cc, Ty::Float),
    ("hipGunKickYawMin", 0x4d0, Ty::Float),
    ("hipGunKickYawMax", 0x4d4, Ty::Float),
    ("hipGunKickAccel", 0x4d8, Ty::Float),
    ("hipGunKickSpeedMax", 0x4dc, Ty::Float),
    ("hipGunKickSpeedDecay", 0x4e0, Ty::Float),
    ("hipGunKickStaticDecay", 0x4e4, Ty::Float),
    ("hipViewKickPitchMin", 0x4e8, Ty::Float),
    ("hipViewKickPitchMax", 0x4ec, Ty::Float),
    ("hipViewKickYawMin", 0x4f0, Ty::Float),
    ("hipViewKickYawMax", 0x4f4, Ty::Float),
    ("hipViewKickCenterSpeed", 0x4f8, Ty::Float),
    ("leftArc", 0x528, Ty::Float),
    ("rightArc", 0x52c, Ty::Float),
    ("topArc", 0x530, Ty::Float),
    ("bottomArc", 0x534, Ty::Float),
    ("accuracy", 0x538, Ty::Float),
    ("aiSpread", 0x53c, Ty::Float),
    ("playerSpread", 0x540, Ty::Float),
    ("maxVertTurnSpeed", 0x54c, Ty::Float),
    ("maxHorTurnSpeed", 0x550, Ty::Float),
    ("minVertTurnSpeed", 0x544, Ty::Float),
    ("minHorTurnSpeed", 0x548, Ty::Float),
    ("pitchConvergenceTime", 0x554, Ty::Float),
    ("yawConvergenceTime", 0x558, Ty::Float),
    ("suppressionTime", 0x55c, Ty::Float),
    ("maxRange", 0x560, Ty::Float),
    ("animHorRotateInc", 0x564, Ty::Float),
    ("playerPositionDist", 0x568, Ty::Float),
    ("stance", 0x8c, Ty::Stance),
    ("useHintString", 0x56c, Ty::Str),
    ("dropHintString", 0x570, Ty::Str),
    ("horizViewJitter", 0x57c, Ty::Float),
    ("vertViewJitter", 0x580, Ty::Float),
    ("fightDist", 0x504, Ty::Float),
    ("maxDist", 0x508, Ty::Float),
    ("aiVsAiAccuracyGraph", 0x50c, Ty::Str),
    ("aiVsPlayerAccuracyGraph", 0x510, Ty::Str),
    ("locNone", 0x5b0, Ty::Float),
    ("locHelmet", 0x5b4, Ty::Float),
    ("locHead", 0x5b8, Ty::Float),
    ("locNeck", 0x5bc, Ty::Float),
    ("locTorsoUpper", 0x5c0, Ty::Float),
    ("locTorsoLower", 0x5c4, Ty::Float),
    ("locRightArmUpper", 0x5c8, Ty::Float),
    ("locRightArmLower", 0x5d0, Ty::Float),
    ("locRightHand", 0x5d8, Ty::Float),
    ("locLeftArmUpper", 0x5cc, Ty::Float),
    ("locLeftArmLower", 0x5d4, Ty::Float),
    ("locLeftHand", 0x5dc, Ty::Float),
    ("locRightLegUpper", 0x5e0, Ty::Float),
    ("locRightLegLower", 0x5e8, Ty::Float),
    ("locRightFoot", 0x5f0, Ty::Float),
    ("locLeftLegUpper", 0x5e4, Ty::Float),
    ("locLeftLegLower", 0x5ec, Ty::Float),
    ("locLeftFoot", 0x5f4, Ty::Float),
    ("locGun", 0x5f8, Ty::Float),
    ("fireRumble", 0x5fc, Ty::Str),
    ("meleeImpactRumble", 0x600, Ty::Str),
];

/// Keys the stock 1.3 weapon files carry that no record of [`FIELDS`]
/// names, so the loader never reads them (`ParseConfigStringToStruct`
/// looks each record's key up; the file's other keys are never visited).
/// None of the names is a string anywhere in either 1.3 binary.
pub const STALE_KEYS: [&str; 11] = [
    "adsTransBlendTime",
    "adsZoomGunFov",
    "grabarc",
    "horTurnSpeed",
    "isHandModelOverridable",
    "projectileTrail",
    "projectileTrailRadius",
    "projectileTrailTime",
    "shellEject",
    "twoHanded",
    "vertTurnSpeed",
];

/// `enemyCrosshairRange` past this is a `Com_Error` (Linux 0x8157670).
pub const MAX_ENEMY_CROSSHAIR_RANGE: f32 = 15000.0;
/// `maxDamageRange` and `minDamageRange` at or below 0 read as these
/// (Linux 0x80f3186): 999999.0 and 999999.125.
pub const DEFAULT_MAX_DAMAGE_RANGE: f32 = f32::from_bits(0x4974_23f0);
pub const DEFAULT_MIN_DAMAGE_RANGE: f32 = f32::from_bits(0x4974_23f2);

/// One field's parsed value; a field the file leaves out or empty holds
/// the loader's zero (`""` for a string).
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Str(String),
    /// `Int` and `Ms` (whole milliseconds).
    Int(i32),
    Bool(bool),
    Float(f32),
    /// An index into the type's name table (`Ty::names`, or the pak's
    /// player anim types).
    Enum(i32),
}

impl Value {
    fn zero(ty: Ty) -> Value {
        match ty {
            Ty::Str => Value::Str(String::new()),
            Ty::Int | Ty::Ms => Value::Int(0),
            Ty::Bool => Value::Bool(false),
            Ty::Float => Value::Float(0.0),
            _ => Value::Enum(0),
        }
    }
}

/// A field's index in [`FIELDS`] by name, case-insensitive as the info
/// string lookup is.
pub fn field_index(key: &str) -> Option<usize> {
    static INDEX: OnceLock<HashMap<String, usize>> = OnceLock::new();
    INDEX
        .get_or_init(|| {
            FIELDS
                .iter()
                .enumerate()
                .map(|(i, (name, _, _))| (name.to_ascii_lowercase(), i))
                .collect()
        })
        .get(&key.to_ascii_lowercase())
        .copied()
}

/// A loaded weapon file: every [`FIELDS`] record's value.
#[derive(Clone, Debug, PartialEq)]
pub struct WeaponDef {
    /// The file name (`kar98k_mp`), the def's +0.
    pub name: String,
    values: Vec<Value>,
}

impl WeaponDef {
    /// The value of the field `key` names; `None` for a key not in
    /// [`FIELDS`].
    pub fn get(&self, key: &str) -> Option<&Value> {
        self.values.get(field_index(key)?)
    }

    /// A string field; `""` unset or not a string.
    pub fn str(&self, key: &str) -> &str {
        match self.get(key) {
            Some(Value::Str(s)) => s,
            _ => "",
        }
    }

    /// An int, ms or enum field; 0 otherwise.
    pub fn int(&self, key: &str) -> i32 {
        match self.get(key) {
            Some(Value::Int(v) | Value::Enum(v)) => *v,
            Some(Value::Bool(b)) => i32::from(*b),
            _ => 0,
        }
    }

    pub fn float(&self, key: &str) -> f32 {
        match self.get(key) {
            Some(Value::Float(v)) => *v,
            _ => 0.0,
        }
    }

    pub fn flag(&self, key: &str) -> bool {
        self.int(key) != 0
    }

    /// `weaponType`'s index in [`WEAPON_TYPES`].
    pub fn weap_type(&self) -> i32 {
        self.int("weaponType")
    }

    /// `weaponClass`'s index in [`WEAPON_CLASSES`].
    pub fn weap_class(&self) -> i32 {
        self.int("weaponClass")
    }

    /// The anim view node `node` plays (1..=22, [`view_node`]): its key's
    /// clip, or idle's where the file leaves it empty, as
    /// `CG_RegisterWeapon` (Mac 0x1d85cc) fills the tree.
    pub fn view_anim(&self, node: usize) -> &str {
        let own = VIEW_ANIM_KEYS
            .get(node.wrapping_sub(1))
            .map_or("", |k| self.str(k));
        if own.is_empty() {
            self.str("idleAnim")
        } else {
            own
        }
    }

    /// The rate the view node plays its clip at, given the clip's length in
    /// ms: `length / time` for the nodes [`NODE_TIMES`] names, 0 when that
    /// time is 0, and 1 for the rest (`CG_RegisterWeapon`).
    pub fn view_rate(&self, node: usize, clip_ms: i32) -> f32 {
        match NODE_TIMES.iter().find(|(n, _)| *n == node) {
            Some((_, key)) => {
                let t = self.int(key);
                if t > 0 {
                    clip_ms as f32 / t as f32
                } else {
                    0.0
                }
            }
            None => 1.0,
        }
    }
}

/// The parse of one file: its def and the keys the loader would pass over.
#[derive(Clone, Debug)]
pub struct Parsed {
    pub def: WeaponDef,
    /// Keys in no [`FIELDS`] record and not in [`STALE_KEYS`].
    pub unknown: Vec<String>,
    /// Keys in [`STALE_KEYS`].
    pub stale: Vec<String>,
}

/// The longest file `BG_LoadWeaponDefInternal` reads past its
/// `WEAPONFILE` tag.
const MAX_BODY: usize = 0x2000;

/// `BG_LoadWeaponDefInternal` (Linux 0x80f3382) on a file's text: the
/// `WEAPONFILE` tag, at most 8191 bytes after it, an info string; each
/// [`FIELDS`] record takes the first value under its key (any case), an
/// empty one leaving the zero. `anim_types` is `mp/playeranimtypes.txt`'s
/// list ([`player_anim_types`]). The errors are the loader's `Com_Error`s
/// and refusals.
pub fn parse(name: &str, text: &[u8], anim_types: &[String]) -> Result<Parsed> {
    let body = text
        .strip_prefix(b"WEAPONFILE")
        .ok_or_else(|| anyhow!("{name} does not appear to be a weapon file"))?;
    if body.len() >= MAX_BODY {
        bail!("{name} is too long of a weapon file to parse");
    }
    let body = vcod_common::text::latin1(body);
    if body.contains(['"', ';']) {
        bail!("{name} is not a valid weapon file");
    }
    let pairs = info_pairs(&body);
    let mut values: Vec<Value> = FIELDS.iter().map(|&(_, _, ty)| Value::zero(ty)).collect();
    for (i, &(key, _, ty)) in FIELDS.iter().enumerate() {
        let Some((_, v)) = pairs.iter().find(|(k, _)| k.eq_ignore_ascii_case(key)) else {
            continue;
        };
        if v.is_empty() {
            continue;
        }
        values[i] = match ty {
            Ty::Str => Value::Str(v.to_string()),
            Ty::Int => Value::Int(atoi(v)),
            Ty::Bool => Value::Bool(atoi(v) != 0),
            Ty::Float => Value::Float(atof(v) as f32),
            Ty::Ms => Value::Int(vcod_common::weapon::stored_ms(atof(v) as f32)),
            Ty::PlayerAnimType => Value::Enum(
                lookup(anim_types.iter().map(String::as_str), v)
                    .ok_or_else(|| anyhow!("unknown playerAnimType \"{v}\" in \"{name}\""))?,
            ),
            _ => {
                let names = ty.names().expect("a specific type");
                Value::Enum(
                    lookup(names.iter().copied(), v)
                        .ok_or_else(|| anyhow!("unknown {key} \"{v}\" in \"{name}\""))?,
                )
            }
        };
    }
    let mut def = WeaponDef {
        name: name.to_string(),
        values,
    };
    for (key, default) in [
        ("maxDamageRange", DEFAULT_MAX_DAMAGE_RANGE),
        ("minDamageRange", DEFAULT_MIN_DAMAGE_RANGE),
    ] {
        if def.float(key) <= 0.0 {
            def.values[field_index(key).expect("a field")] = Value::Float(default);
        }
    }
    if def.float("enemyCrosshairRange") > MAX_ENEMY_CROSSHAIR_RANGE {
        bail!("{name}: enemy crosshair ranges should be less than {MAX_ENEMY_CROSSHAIR_RANGE}");
    }
    if def.weap_type() == 2 {
        let angle = def.float("destabilizationAngleMax");
        if def.int("projectileSpeed") <= 0 {
            bail!("{name}: projectile speed must be greater than 0");
        }
        if !(0.0..45.0).contains(&angle) {
            bail!("{name}: destabilization angle must be between 0 and 45 degrees");
        }
        if def.float("destabilizationBaseTime") <= 0.0
            || def.float("destabilizationTimeReductionRatio") <= 0.0
        {
            bail!("{name}: destabilization base time and reduction ratio must be positive");
        }
    }
    let (mut unknown, mut stale) = (Vec::new(), Vec::new());
    for (k, _) in &pairs {
        if field_index(k).is_some() {
            continue;
        }
        if STALE_KEYS.iter().any(|s| s.eq_ignore_ascii_case(k)) {
            stale.push(k.to_string());
        } else {
            unknown.push(k.to_string());
        }
    }
    Ok(Parsed {
        def,
        unknown,
        stale,
    })
}

/// `weapons/mp/<name>` off `fs`, with the pak's player anim types.
pub fn load(fs: &(impl AssetSource + ?Sized), name: &str) -> Result<Parsed> {
    let path = format!("weapons/mp/{name}");
    let text = fs.read(&path).ok_or_else(|| anyhow!("{path} not found"))?;
    let types = fs
        .read("mp/playeranimtypes.txt")
        .map(|t| player_anim_types(&String::from_utf8_lossy(&t)))
        .unwrap_or_default();
    parse(name, &text, &types)
}

/// `BG_LoadPlayerAnimTypes` (Mac 0xbcbf8 area): one name per token.
pub fn player_anim_types(text: &str) -> Vec<String> {
    text.split_whitespace().map(str::to_string).collect()
}

/// `stricmp` against each name; the first match's index.
fn lookup<'a>(mut names: impl Iterator<Item = &'a str>, v: &str) -> Option<i32> {
    names
        .position(|n| n.eq_ignore_ascii_case(v))
        .map(|i| i as i32)
}

/// `\key\value` pairs in file order; `Info_ValueForKey` reads the first of
/// a repeated key.
fn info_pairs(s: &str) -> Vec<(&str, &str)> {
    let s = s.strip_prefix('\\').unwrap_or(s);
    let mut it = s.split('\\');
    let mut out = Vec::new();
    while let Some(k) = it.next() {
        let v = it.next().unwrap_or("");
        if !k.is_empty() || !v.is_empty() {
            out.push((k, v));
        }
    }
    out
}

/// C's `atof`: the longest float prefix after leading space; 0 when none.
fn atof(s: &str) -> f64 {
    let t = s.trim_start();
    let b = t.as_bytes();
    let mut end = 0;
    if matches!(b.first(), Some(b'+' | b'-')) {
        end = 1;
    }
    let digits = |from: usize| from + b[from..].iter().take_while(|c| c.is_ascii_digit()).count();
    let int_end = digits(end);
    let mut last = int_end;
    if b.get(int_end) == Some(&b'.') {
        last = digits(int_end + 1);
    }
    if last == end || (last == end + 1 && b.get(end) == Some(&b'.')) {
        return 0.0;
    }
    if matches!(b.get(last), Some(b'e' | b'E')) {
        let mut e = last + 1;
        if matches!(b.get(e), Some(b'+' | b'-')) {
            e += 1;
        }
        let e_end = digits(e);
        if e_end > e {
            last = e_end;
        }
    }
    t[..last].parse().unwrap_or(0.0)
}

/// The view weapon's anim keys in tree order: node `n` (1..=22) plays the
/// clip `VIEW_ANIM_KEYS[n - 1]` names, the def's strings at 0x18..0x6c
/// (`CG_RegisterWeapon`, Mac 0x1d85cc, `XAnimCreate(anims, n, ...)`).
pub const VIEW_ANIM_KEYS: [&str; 22] = [
    "idleAnim",
    "emptyIdleAnim",
    "fireAnim",
    "holdFireAnim",
    "lastShotAnim",
    "rechamberAnim",
    "meleeAnim",
    "reloadAnim",
    "reloadEmptyAnim",
    "reloadStartAnim",
    "reloadEndAnim",
    "raiseAnim",
    "dropAnim",
    "altRaiseAnim",
    "altDropAnim",
    "quickRaiseAnim",
    "quickDropAnim",
    "adsFireAnim",
    "adsLastShotAnim",
    "adsRechamberAnim",
    "adsUpAnim",
    "adsDownAnim",
];

/// The view tree's nodes (node 0 is the root blend).
pub mod node {
    pub const IDLE: usize = 1;
    pub const EMPTY_IDLE: usize = 2;
    pub const FIRE: usize = 3;
    pub const HOLD_FIRE: usize = 4;
    pub const LAST_SHOT: usize = 5;
    pub const RECHAMBER: usize = 6;
    pub const MELEE: usize = 7;
    pub const RELOAD: usize = 8;
    pub const RELOAD_EMPTY: usize = 9;
    pub const RELOAD_START: usize = 10;
    pub const RELOAD_END: usize = 11;
    pub const RAISE: usize = 12;
    pub const DROP: usize = 13;
    pub const ALT_RAISE: usize = 14;
    pub const ALT_DROP: usize = 15;
    pub const QUICK_RAISE: usize = 16;
    pub const QUICK_DROP: usize = 17;
    pub const ADS_FIRE: usize = 18;
    pub const ADS_LAST_SHOT: usize = 19;
    pub const ADS_RECHAMBER: usize = 20;
    pub const ADS_UP: usize = 21;
    pub const ADS_DOWN: usize = 22;
    /// Nodes 1..=20 are the main clips; 21 and 22 the sight layer.
    pub const MAIN_END: usize = 21;
}

/// The nodes whose rate `CG_RegisterWeapon` fits to a weapon time, and
/// that time's key. `reloadEndAnim` is fitted to `reloadTime`, not
/// `reloadEndTime`: `CG_RegisterWeapon` divides node 11 by the def's +0x218.
pub const NODE_TIMES: [(usize, &str); 12] = [
    (node::HOLD_FIRE, "holdFireTime"),
    (node::MELEE, "meleeTime"),
    (node::RELOAD, "reloadTime"),
    (node::RELOAD_EMPTY, "reloadEmptyTime"),
    (node::RELOAD_START, "reloadStartTime"),
    (node::RELOAD_END, "reloadTime"),
    (node::RAISE, "raiseTime"),
    (node::DROP, "dropTime"),
    (node::ALT_RAISE, "altRaiseTime"),
    (node::ALT_DROP, "altDropTime"),
    (node::QUICK_RAISE, "quickRaiseTime"),
    (node::QUICK_DROP, "quickDropTime"),
];

/// `ps.weapAnim`'s anim bit that restarts the same anim.
pub const ANIM_TOGGLEBIT: i32 = 0x200;

/// The node `CG_WeaponRunXModelAnims` (Mac 0x1dc3cc) starts for
/// `ps.weapAnim` (toggle bit masked). `None` for 0, which waits for the
/// running clip to finish and then plays idle or, the clip empty, empty
/// idle; an unknown number plays idle.
pub fn view_node(weap_anim: i32) -> Option<usize> {
    use node::*;
    Some(match weap_anim & !ANIM_TOGGLEBIT {
        0 => return None,
        2 => FIRE,
        3 => LAST_SHOT,
        4 => RECHAMBER,
        5 => ADS_FIRE,
        6 => ADS_LAST_SHOT,
        7 => ADS_RECHAMBER,
        8 => MELEE,
        9 => DROP,
        10 => RAISE,
        11 => RELOAD,
        12 => RELOAD_EMPTY,
        13 => RELOAD_START,
        14 => RELOAD_END,
        15 => ALT_DROP,
        16 => ALT_RAISE,
        17 => QUICK_DROP,
        18 => QUICK_RAISE,
        19 => HOLD_FIRE,
        _ => IDLE,
    })
}

/// Clip (or ammo) indexes in weapon index order (`BG_SetupClipIndexes`,
/// Mac 0x7a22a; `BG_SetupAmmoIndexes`, 0x7a4e4): each new `clipName`
/// (exact, case-sensitive `strcmp`) takes the next slot. Slot 0 is the
/// built-in default def's (`BG_LoadDefaultWeaponDef`, Mac 0xbcdac), whose
/// names are all `""`, so a file without the key shares it. `names[0]` is
/// weapon 1.
pub fn clip_indices<'a>(clip_names: impl IntoIterator<Item = &'a str>) -> Vec<usize> {
    let mut seen: Vec<&str> = vec![""];
    clip_names
        .into_iter()
        .map(|c| match seen.iter().position(|s| *s == c) {
            Some(i) => i,
            None => {
                seen.push(c);
                seen.len() - 1
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_table_is_whole_and_unique() {
        let mut seen = std::collections::HashSet::new();
        for (name, off, _) in FIELDS {
            assert!(seen.insert(name.to_ascii_lowercase()), "{name} twice");
            assert!(off < 0x604, "{name} at {off:#x}");
        }
        for (i, key) in VIEW_ANIM_KEYS.iter().enumerate() {
            assert_eq!(FIELDS[field_index(key).unwrap()].1, 0x18 + 4 * i as u16);
        }
        assert_eq!(FIELDS[field_index("holdFireTime").unwrap()].1, 0x210);
        assert_eq!(FIELDS[field_index("reloadTime").unwrap()].1, 0x218);
        assert_eq!(FIELDS[field_index("quickRaiseTime").unwrap()].1, 0x244);
        for k in STALE_KEYS {
            assert!(field_index(k).is_none(), "{k}");
        }
    }

    #[test]
    fn parses_like_the_loader() {
        let text = b"WEAPONFILE\\displayName\\X\\weaponType\\Projectile\\fireTime\\1.8\
\\damage\\120abc\\FIRETIME\\9\\maxDamageRange\\-1\\projectileSpeed\\100\
\\destabilizationAngleMax\\5\\destabilizationBaseTime\\1\
\\destabilizationTimeReductionRatio\\2\\playerAnimType\\MP40\\twoHanded\\1\\gunModel\\\\bogus\\2";
        let types = player_anim_types("none\r\nother\r\nmp40\r\n");
        let p = parse("x_mp", text, &types).unwrap();
        let d = &p.def;
        assert_eq!(d.str("displayName"), "X");
        assert_eq!(d.weap_type(), 2);
        assert_eq!(d.int("fireTime"), 1799, "truncated, the first key wins");
        assert_eq!(d.int("damage"), 120);
        assert_eq!(d.float("maxDamageRange"), DEFAULT_MAX_DAMAGE_RANGE);
        assert_eq!(d.float("minDamageRange"), DEFAULT_MIN_DAMAGE_RANGE);
        assert_eq!(d.int("playerAnimType"), 2);
        assert_eq!(d.str("gunModel"), "");
        assert_eq!(p.stale, ["twoHanded"]);
        assert_eq!(p.unknown, ["bogus"]);
        assert!(parse("x", b"WEAPONFILE\\weaponType\\laser", &types).is_err());
        assert!(parse("x", b"WEAPONFILE\\a\\\"", &types).is_err());
        assert!(parse("x", b"NOTWEAPON\\a\\b", &types).is_err());
    }

    #[test]
    fn atof_and_atoi_take_c_prefixes() {
        assert_eq!(atof(" 1.5e2x"), 150.0);
        assert_eq!(atof(".25"), 0.25);
        assert_eq!(atof("-."), 0.0);
        assert_eq!(atof("3e"), 3.0);
        assert_eq!(atoi(" -42.9"), -42);
        assert_eq!(atoi("x"), 0);
    }

    #[test]
    fn wire_anims_name_their_nodes() {
        assert_eq!(view_node(0), None);
        assert_eq!(view_node(ANIM_TOGGLEBIT), None);
        assert_eq!(view_node(2 | ANIM_TOGGLEBIT), Some(node::FIRE));
        assert_eq!(view_node(17), Some(node::QUICK_DROP));
        assert_eq!(view_node(19), Some(node::HOLD_FIRE));
        assert_eq!(view_node(1), Some(node::IDLE));
        assert_eq!(view_node(42), Some(node::IDLE));
    }

    #[test]
    fn clips_index_from_one_by_first_use() {
        assert_eq!(clip_indices(["a", "b", "a", "B"]), [1, 2, 1, 3]);
        // A file without the key shares the default def's slot 0.
        assert_eq!(clip_indices(["a", "", "b", "a"]), [1, 0, 2, 1]);
    }

    #[test]
    fn rates_fit_the_weapon_times() {
        let text = b"WEAPONFILE\\idleAnim\\i\\fireAnim\\f\\reloadTime\\2\\raiseTime\\0.5";
        let d = parse("x", text, &[]).unwrap().def;
        assert_eq!(d.view_rate(node::RELOAD, 1000), 0.5);
        assert_eq!(d.view_rate(node::RELOAD_END, 1000), 0.5);
        assert_eq!(d.view_rate(node::RAISE, 1000), 2.0);
        assert_eq!(d.view_rate(node::DROP, 1000), 0.0);
        assert_eq!(d.view_rate(node::FIRE, 1000), 1.0);
        assert_eq!(d.view_anim(node::FIRE), "f");
        assert_eq!(d.view_anim(node::RELOAD), "i");
    }
}
