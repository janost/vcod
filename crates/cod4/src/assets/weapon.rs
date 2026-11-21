//! Weapons: `WeaponDef`, 0x878, whose slots load in the order of
//! [`OPS`].

use super::fx::{Fx, fx};
use super::material::{Material, material};
use super::model::{XModel, xmodel};
use super::{Id, asset, push};
use crate::walk::{Follow, Handle, S, W};
use crate::zone::{Block, Pointer};
use anyhow::Result;
use std::collections::HashMap;
use std::io::BufRead;

/// `WeaponDef`'s loaded slots, by field name.
#[derive(Clone, Debug)]
pub struct Weapon {
    pub name: String,
    pub strings: HashMap<&'static str, String>,
    pub models: HashMap<&'static str, Id<XModel>>,
    /// `gunModel[16]` and `worldModel[16]`.
    pub model_lists: HashMap<&'static str, Vec<Option<Id<XModel>>>>,
    pub materials: HashMap<&'static str, Id<Material>>,
    pub effects: HashMap<&'static str, Id<Fx>>,
    /// Sound alias names (`SndAliasCustom` carries only the name).
    pub sounds: HashMap<&'static str, String>,
    /// The 33 view anims, by slot (`idleAnim` is 1; the weapon file names
    /// each, `tools/re/cod4_weapon_fields.py`).
    pub xanims: Vec<Option<String>>,
    /// `bounceSound`: one alias name per surface type (29).
    pub bounce_sounds: Vec<Option<String>>,
    /// The whole struct as loaded.
    pub raw: Vec<u8>,
}

impl Weapon {
    /// The little-endian int at `at` in [`Self::raw`], 0 past it.
    fn i32_at(&self, at: usize) -> i32 {
        self.raw
            .get(at..at + 4)
            .map_or(0, |b| i32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    /// The little-endian float at `at` in [`Self::raw`], 0 past it.
    fn f32_at(&self, at: usize) -> f32 {
        f32::from_bits(self.i32_at(at) as u32)
    }

    /// Three little-endian floats from `at`.
    fn vec3_at(&self, at: usize) -> [f32; 3] {
        [self.f32_at(at), self.f32_at(at + 4), self.f32_at(at + 8)]
    }

    /// `aimDownSight` (+1260).
    pub fn aim_down_sight(&self) -> bool {
        self.i32_at(1260) != 0
    }

    /// `adsOverlayReticle` (+1080), 0 none.
    pub fn ads_overlay_reticle(&self) -> i32 {
        self.i32_at(1080)
    }

    /// Whether the `adsOverlayShader` slot (+1072) is set in the struct.
    pub fn has_ads_overlay_shader(&self) -> bool {
        self.i32_at(1072) != 0
    }

    /// `adsZoomFov` (+1060).
    pub fn ads_zoom_fov(&self) -> f32 {
        self.f32_at(1060)
    }

    /// `adsZoomInFrac` (+1064) and `adsZoomOutFrac` (+1068).
    pub fn ads_zoom_fracs(&self) -> (f32, f32) {
        (self.f32_at(1064), self.f32_at(1068))
    }

    /// `adsBobFactor` (+1096).
    pub fn ads_bob_factor(&self) -> f32 {
        self.f32_at(1096)
    }

    /// `adsAimPitch` (+1740).
    pub fn ads_aim_pitch(&self) -> f32 {
        self.f32_at(1740)
    }

    /// `adsReloadTransTime` (+1940), ms.
    pub fn ads_reload_trans_time(&self) -> i32 {
        self.i32_at(1940)
    }

    /// Stand, ducked, prone `{stance}Move{F,R,U}` (+564, +600, +636).
    pub fn stance_moves(&self) -> [[f32; 3]; 3] {
        [564, 600, 636].map(|o| self.vec3_at(o))
    }

    /// Stand, ducked, prone `{stance}Rot{P,Y,R}` (+576, +612, +648).
    pub fn stance_rots(&self) -> [[f32; 3]; 3] {
        [576, 612, 648].map(|o| self.vec3_at(o))
    }

    /// Ducked then prone `{stance}Ofs{F,R,U}` (+588, +624); standing has none.
    pub fn stance_ofs(&self) -> [[f32; 3]; 2] {
        [588, 624].map(|o| self.vec3_at(o))
    }

    /// Stand, ducked, prone `{stance}MoveMinSpeed` (+668..+676).
    pub fn move_min_speeds(&self) -> [f32; 3] {
        self.vec3_at(668)
    }

    /// Stand, ducked, prone `{stance}RotMinSpeed` (+688..+696).
    pub fn rot_min_speeds(&self) -> [f32; 3] {
        self.vec3_at(688)
    }

    /// `posMoveRate` (+660) and `posProneMoveRate` (+664).
    pub fn pos_move_rates(&self) -> [f32; 2] {
        [self.f32_at(660), self.f32_at(664)]
    }

    /// `posRotRate` (+680) and `posProneRotRate` (+684).
    pub fn pos_rot_rates(&self) -> [f32; 2] {
        [self.f32_at(680), self.f32_at(684)]
    }

    /// `adsIdleAmount` (+1164) and `hipIdleAmount` (+1168).
    pub fn idle_amounts(&self) -> [f32; 2] {
        [self.f32_at(1164), self.f32_at(1168)]
    }

    /// `adsIdleSpeed` (+1172) and `hipIdleSpeed` (+1176).
    pub fn idle_speeds(&self) -> [f32; 2] {
        [self.f32_at(1172), self.f32_at(1176)]
    }

    /// `idleCrouchFactor` (+1180) and `idleProneFactor` (+1184).
    pub fn idle_factors(&self) -> [f32; 2] {
        [self.f32_at(1180), self.f32_at(1184)]
    }

    /// Hip (+1196) then ads (+1224) sway: `swayMaxAngle`, `swayLerpSpeed`,
    /// `swayPitchScale`, `swayYawScale`, `swayHorizScale`, `swayVertScale`.
    pub fn sways(&self) -> [[f32; 6]; 2] {
        [1196, 1224].map(|o| std::array::from_fn(|k| self.f32_at(o + 4 * k)))
    }

    /// `nightVisionWearTime` (+980), ms.
    pub fn night_vision_wear_time(&self) -> i32 {
        self.i32_at(980)
    }

    /// `reloadTime` (+904) and `reloadShowRocketTime` (+908), ms.
    pub fn reload_times(&self) -> (i32, i32) {
        (self.i32_at(904), self.i32_at(908))
    }

    /// `adsDofStart` (+2160) and `adsDofEnd` (+2164).
    pub fn ads_dof(&self) -> [f32; 2] {
        [self.f32_at(2160), self.f32_at(2164)]
    }

    /// Each view anim node's time in ms off `g_animRateOffsets`
    /// (iw3mp.exe 0x727580, cod4-client-view.md 4.1), `None` for a node
    /// with no time field.
    pub fn anim_rate_times(&self) -> [Option<i32>; 33] {
        ANIM_RATE_OFFSETS.map(|o| (o >= 0).then(|| self.i32_at(o as usize)))
    }

    /// `weapClass` (+304, the weapon file's `weaponClass`); 7 a turret.
    pub fn weap_class(&self) -> i32 {
        self.i32_at(304)
    }

    /// `animHorRotateInc` (+2004): a turret gunner's column step.
    pub fn anim_hor_rotate_inc(&self) -> f32 {
        self.f32_at(2004)
    }

    /// `horizViewJitter` (+2028) and `vertViewJitter` (+2032).
    pub fn view_jitter(&self) -> (f32, f32) {
        (self.f32_at(2028), self.f32_at(2032))
    }

    /// `notetrackSoundMap`: 16 key and 16 value script strings at +232 and
    /// +264, as (key, value) pairs; 0 past [`Self::raw`].
    pub fn notetrack_sound_map(&self) -> [(u16, u16); 16] {
        let at = |o: usize| {
            self.raw
                .get(o..o + 2)
                .map_or(0, |b| u16::from_le_bytes([b[0], b[1]]))
        };
        std::array::from_fn(|k| (at(232 + 2 * k), at(264 + 2 * k)))
    }

    /// `hideTags[8]` at 216: the gun model's bones whose surfaces the view
    /// weapon hides, script strings of the zone `a` the weapon came from
    /// (cod4-client-view.md 4.2).
    pub fn hide_tags(&self, a: &super::Assets) -> Vec<String> {
        (0..8)
            .filter_map(|k| {
                let b = self.raw.get(216 + 2 * k..218 + 2 * k)?;
                let n = u16::from_le_bytes([b[0], b[1]]);
                (n != 0).then_some(n)?;
                a.script_strings.get(usize::from(n)).cloned().flatten()
            })
            .collect()
    }
}

/// `g_animRateOffsets` (iw3mp.exe 0x727580): each view anim node's time
/// field in `WeaponDef`, -1 for none.
const ANIM_RATE_OFFSETS: [i32; 33] = [
    -1, -1, -1, -1, 888, -1, -1, 896, 900, 904, 912, 920, 928, 936, 956, 932, 944, 940, 952, 948,
    960, 964, 968, 972, 976, -1, 980, 992, -1, -1, -1, -1, -1,
];

/// One slot of the walk, at its offset in `WeaponDef`.
#[derive(Clone, Copy, Debug)]
enum Op {
    Str(usize),
    Strs {
        at: usize,
        n: usize,
    },
    Model(usize),
    Models {
        at: usize,
        n: usize,
    },
    Fx(usize),
    Material(usize),
    /// A `SndAliasCustom`: a slot to a struct holding the alias name.
    Snd(usize),
    /// A slot to `n` `SndAliasCustom`.
    SndArray {
        at: usize,
        n: usize,
    },
    /// A slot to `vec2_t` knots, the count an int at `count`.
    Knots {
        at: usize,
        count: usize,
    },
}

/// What a pointer slot of `WeaponDef` at `offset` holds, by
/// [`OPS`]: the struct's field name and the slot's kind, the index into an
/// array slot beside it (`gunModel`'s fourth is `("gunModel", 3)`).
pub(crate) fn slot_at(offset: usize) -> Option<(SlotKind, &'static str, usize)> {
    OPS.iter().find_map(|&(op, name)| {
        let (kind, at, n) = match op {
            Op::Str(o) => (SlotKind::Str, o, 1),
            Op::Strs { at, n } => (SlotKind::Str, at, n),
            Op::Model(o) => (SlotKind::Model, o, 1),
            Op::Models { at, n } => (SlotKind::Model, at, n),
            Op::Fx(o) => (SlotKind::Fx, o, 1),
            Op::Material(o) => (SlotKind::Material, o, 1),
            Op::Snd(o) => (SlotKind::Sound, o, 1),
            Op::SndArray { .. } | Op::Knots { .. } => return None,
        };
        (offset >= at && offset < at + 4 * n && (offset - at).is_multiple_of(4))
            .then(|| (kind, name, (offset - at) / 4))
    })
}

/// A pointer slot's kind ([`slot_at`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SlotKind {
    Str,
    Model,
    Fx,
    Material,
    Sound,
}

/// `Load_WeaponDef`'s slots in load order, each named by the weapon file
/// key 1.7's field table gives its offset (`tools/re/cod4_weapon_fields.py`
/// over `cod4_lnxded`). The name, the view anims and the graph knots have
/// no key of their own.
const OPS: &[(Op, &str)] = &[
    (Op::Str(0), "name"),
    (Op::Str(4), "displayName"),
    (Op::Str(8), "AIOverlayDescription"),
    (Op::Models { at: 12, n: 16 }, "gunModel"),
    (Op::Model(76), "handModel"),
    (Op::Strs { at: 80, n: 33 }, "xanims"),
    (Op::Str(212), "modeName"),
    (Op::Fx(332), "viewFlashEffect"),
    (Op::Fx(336), "worldFlashEffect"),
    (Op::Snd(340), "pickupSound"),
    (Op::Snd(344), "pickupSoundPlayer"),
    (Op::Snd(348), "ammoPickupSound"),
    (Op::Snd(352), "ammoPickupSoundPlayer"),
    (Op::Snd(356), "projectileSound"),
    (Op::Snd(360), "pullbackSound"),
    (Op::Snd(364), "pullbackSoundPlayer"),
    (Op::Snd(368), "fireSound"),
    (Op::Snd(372), "fireSoundPlayer"),
    (Op::Snd(376), "loopFireSound"),
    (Op::Snd(380), "loopFireSoundPlayer"),
    (Op::Snd(384), "stopFireSound"),
    (Op::Snd(388), "stopFireSoundPlayer"),
    (Op::Snd(392), "lastShotSound"),
    (Op::Snd(396), "lastShotSoundPlayer"),
    (Op::Snd(400), "emptyFireSound"),
    (Op::Snd(404), "emptyFireSoundPlayer"),
    (Op::Snd(408), "meleeSwipeSound"),
    (Op::Snd(412), "meleeSwipeSoundPlayer"),
    (Op::Snd(416), "meleeHitSound"),
    (Op::Snd(420), "meleeMissSound"),
    (Op::Snd(424), "rechamberSound"),
    (Op::Snd(428), "rechamberSoundPlayer"),
    (Op::Snd(432), "reloadSound"),
    (Op::Snd(436), "reloadSoundPlayer"),
    (Op::Snd(440), "reloadEmptySound"),
    (Op::Snd(444), "reloadEmptySoundPlayer"),
    (Op::Snd(448), "reloadStartSound"),
    (Op::Snd(452), "reloadStartSoundPlayer"),
    (Op::Snd(456), "reloadEndSound"),
    (Op::Snd(460), "reloadEndSoundPlayer"),
    (Op::Snd(464), "detonateSound"),
    (Op::Snd(468), "detonateSoundPlayer"),
    (Op::Snd(472), "nightVisionWearSound"),
    (Op::Snd(476), "nightVisionWearSoundPlayer"),
    (Op::Snd(480), "nightVisionRemoveSound"),
    (Op::Snd(484), "nightVisionRemoveSoundPlayer"),
    (Op::Snd(488), "altSwitchSound"),
    (Op::Snd(492), "altSwitchSoundPlayer"),
    (Op::Snd(496), "raiseSound"),
    (Op::Snd(500), "raiseSoundPlayer"),
    (Op::Snd(504), "firstRaiseSound"),
    (Op::Snd(508), "firstRaiseSoundPlayer"),
    (Op::Snd(512), "putawaySound"),
    (Op::Snd(516), "putawaySoundPlayer"),
    (Op::SndArray { at: 520, n: 29 }, "bounceSound"),
    (Op::Fx(524), "viewShellEjectEffect"),
    (Op::Fx(528), "worldShellEjectEffect"),
    (Op::Fx(532), "viewLastShotEjectEffect"),
    (Op::Fx(536), "worldLastShotEjectEffect"),
    (Op::Material(540), "reticleCenter"),
    (Op::Material(544), "reticleSide"),
    (Op::Models { at: 700, n: 16 }, "worldModel"),
    (Op::Model(764), "worldClipModel"),
    (Op::Model(768), "rocketModel"),
    (Op::Model(772), "knifeModel"),
    (Op::Model(776), "worldKnifeModel"),
    (Op::Material(780), "hudIcon"),
    (Op::Material(788), "ammoCounterIcon"),
    (Op::Str(804), "ammoName"),
    (Op::Str(812), "clipName"),
    (Op::Str(832), "sharedAmmoCapName"),
    (Op::Material(1072), "adsOverlayShader"),
    (Op::Material(1076), "adsOverlayShaderLowRes"),
    (Op::Material(1304), "killIcon"),
    (Op::Material(1316), "dpadIcon"),
    (Op::Str(1340), "altWeapon"),
    (Op::Model(1412), "projectileModel"),
    (Op::Fx(1420), "projExplosionEffect"),
    (Op::Fx(1428), "projDudEffect"),
    (Op::Snd(1432), "projExplosionSound"),
    (Op::Snd(1436), "projDudSound"),
    (Op::Fx(1704), "projTrailEffect"),
    (Op::Fx(1732), "projIgnitionEffect"),
    (Op::Snd(1736), "projIgnitionSound"),
    (Op::Str(1900), "aiVsAiAccuracyGraph"),
    (
        Op::Knots {
            at: 1908,
            count: 1924,
        },
        "aiVsAiAccuracyGraph knots",
    ),
    (
        Op::Knots {
            at: 1916,
            count: 1924,
        },
        "aiVsAiAccuracyGraph original knots",
    ),
    (Op::Str(1904), "aiVsPlayerAccuracyGraph"),
    (
        Op::Knots {
            at: 1912,
            count: 1928,
        },
        "aiVsPlayerAccuracyGraph knots",
    ),
    (
        Op::Knots {
            at: 1920,
            count: 1928,
        },
        "aiVsPlayerAccuracyGraph original knots",
    ),
    (Op::Str(2012), "useHintString"),
    (Op::Str(2016), "dropHintString"),
    (Op::Str(2036), "script"),
    (Op::Str(2152), "fireRumble"),
    (Op::Str(2156), "meleeImpactRumble"),
];

pub fn weapon<R: BufRead>(w: &mut W<R>, p: Pointer, slot: u32) -> Result<Option<Id<Weapon>>> {
    asset(w, p, slot, Handle::ASSET, |w| {
        let s = w.load(0x878)?;
        w.push(Block::Virtual);
        let mut out = Weapon {
            name: String::new(),
            strings: HashMap::new(),
            models: HashMap::new(),
            model_lists: HashMap::new(),
            materials: HashMap::new(),
            effects: HashMap::new(),
            sounds: HashMap::new(),
            xanims: Vec::new(),
            bounce_sounds: Vec::new(),
            raw: Vec::new(),
        };
        for &(op, field) in OPS {
            match op {
                Op::Str(o) => {
                    if let Some(v) = w.xstring(s.ptr(o))? {
                        out.strings.insert(field, v);
                    }
                }
                Op::Strs { at, n } => out.xanims = w.xstrings_in(&s, at, n)?,
                Op::Model(o) => {
                    if let Some(id) = xmodel(w, s.ptr(o), s.slot(o))? {
                        out.models.insert(field, id);
                    }
                }
                Op::Models { at, n } => {
                    let v = (0..n)
                        .map(|i| xmodel(w, s.ptr(at + 4 * i), s.slot(at + 4 * i)))
                        .collect::<Result<_>>()?;
                    out.model_lists.insert(field, v);
                }
                Op::Fx(o) => {
                    if let Some(id) = fx(w, s.ptr(o), s.slot(o))? {
                        out.effects.insert(field, id);
                    }
                }
                Op::Material(o) => {
                    if let Some(id) = material(w, s.ptr(o), s.slot(o))? {
                        out.materials.insert(field, id);
                    }
                }
                Op::Snd(o) => {
                    if let Some(v) = snd_alias(w, &s, o)? {
                        out.sounds.insert(field, v);
                    }
                }
                Op::SndArray { at, n } => {
                    out.bounce_sounds = match w.follow(s.ptr(at), 4)? {
                        Follow::Inline => {
                            let at = w.here()?;
                            let a = w.load(4 * n)?;
                            let v = (0..n)
                                .map(|i| snd_alias(w, &a, 4 * i))
                                .collect::<Result<Vec<_>>>()?;
                            w.remember("bounce sounds", at, v.clone());
                            v
                        }
                        Follow::At(a) => w.recall("bounce sounds", a).unwrap_or_default(),
                        Follow::Null => Vec::new(),
                    }
                }
                Op::Knots { at, count } => {
                    let n = s.i32(count).max(0) as usize;
                    w.shared(s.ptr(at), 4, 8 * n, "graph knots", false)?;
                }
            }
        }
        w.pop()?;
        out.name = out.strings.get("name").cloned().unwrap_or_default();
        out.raw = s.b;
        Ok(push(&mut w.out.weapons, out))
    })
}

/// A `SndAliasCustom` slot at `o`: a 4-byte struct holding the name.
fn snd_alias<R: BufRead>(w: &mut W<R>, s: &S, o: usize) -> Result<Option<String>> {
    Ok(match w.follow(s.ptr(o), 4)? {
        Follow::Inline => {
            let at = w.here()?;
            let n = w.load(4)?;
            let name = w.xstring(n.ptr(0))?;
            w.remember("sound name", at, name.clone());
            name
        }
        Follow::At(a) => w.recall("sound name", a).flatten(),
        Follow::Null => None,
    })
}
