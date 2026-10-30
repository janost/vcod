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
