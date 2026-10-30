//! Effects and impact tables.

use super::material::{Material, material};
use super::model::{XModel, xmodel};
use super::{Id, asset, push};
use crate::walk::{Handle, S, W};
use crate::zone::{Block, Pointer};
use anyhow::Result;
use std::io::BufRead;

/// `FxEffectDef`, 0x20: name, flags, sizes, three element counts and the
/// elements (emission, looping and one-shot counts summed).
#[derive(Clone, Debug)]
pub struct Fx {
    pub name: String,
    pub flags: i32,
    pub msec_looping_life: i32,
    pub looping: i32,
    pub one_shot: i32,
    pub emission: i32,
    pub elems: Vec<FxElem>,
}

/// `FxElemDef`, 0xFC.
#[derive(Clone, Debug)]
pub struct FxElem {
    pub elem_type: u8,
    pub visuals: Vec<FxVisual>,
    pub effect_on_impact: Option<String>,
    pub effect_on_death: Option<String>,
    pub effect_emitted: Option<String>,
    /// `velSamples`, `velIntervalCount + 1` of 0x60, kept under
    /// [`crate::walk::WalkOptions::keep_bulk`].
    pub vel_samples: Option<Vec<u8>>,
    /// `visSamples`, `visStateIntervalCount + 1` of 0x30, likewise.
    pub vis_samples: Option<Vec<u8>>,
    /// The whole struct as loaded (ranges, atlas, sort order...).
    pub raw: Vec<u8>,
    /// A trail's `FxTrailDef` (+244), always kept: it is small.
    pub trail: Option<FxTrail>,
}

/// `FxTrailDef`, 0x1C: scroll time, repeat and split distances, the
/// cross-section's vertices (0x14 each) and its index pairs
/// (cod4-effects.md 16).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FxTrail {
    /// `scrollTimeMsec`: the texture's scroll period along the trail; 0
    /// still, negative backwards.
    pub scroll_ms: i32,
    /// Units the texture's `u` takes to repeat once.
    pub repeat_dist: i32,
    /// Units the effect moves between two of the trail's cross-sections.
    pub split_dist: i32,
    pub verts: Vec<FxTrailVertex>,
    /// Index pairs into `verts`: each pair a quad between two
    /// cross-sections.
    pub inds: Vec<u16>,
}

/// `FxTrailVertex`: the cross-section's point (times the element's two
/// sizes), its normal and its texture `v`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FxTrailVertex {
    pub pos: [f32; 2],
    pub normal: [f32; 2],
    pub tex: f32,
}

/// One of an element's visuals, by its element type.
#[derive(Clone, Debug)]
pub enum FxVisual {
    /// Sprites, tails, trails, clouds (types 0-4).
    Material(Option<Id<Material>>),
    Model(Option<Id<XModel>>),
    Sound(Option<String>),
    /// A runner's effect, by name.
    Effect(Option<String>),
    /// A decal's two materials.
    Mark([Option<Id<Material>>; 2]),
    /// Lights carry no visual.
    None,
}

const FX_ELEM_TYPE_MODEL: u8 = 5;
const FX_ELEM_TYPE_SOUND: u8 = 8;
const FX_ELEM_TYPE_DECAL: u8 = 9;
const FX_ELEM_TYPE_RUNNER: u8 = 10;

pub fn fx<R: BufRead>(w: &mut W<R>, p: Pointer, slot: u32) -> Result<Option<Id<Fx>>> {
    asset(w, p, slot, Handle::ASSET, |w| {
        let s = w.load(0x20)?;
        w.push(Block::Virtual);
        let name = w.xstring(s.ptr(0))?.unwrap_or_default();
        let n = (s.i32(16) + s.i32(20) + s.i32(24)).max(0) as usize;
        let mut elems = Vec::new();
        if w.inline_only(s.ptr(28), 4)? {
            let a = w.load(0xFC * n)?;
            for i in 0..n {
                elems.push(elem(w, a.elem(i, 0xFC))?);
            }
        }
        w.pop()?;
        Ok(push(
            &mut w.out.fx,
            Fx {
                name,
                flags: s.i32(4),
                msec_looping_life: s.i32(12),
                looping: s.i32(16),
                one_shot: s.i32(20),
                emission: s.i32(24),
                elems,
            },
        ))
    })
}

fn elem<R: BufRead>(w: &mut W<R>, s: S) -> Result<FxElem> {
    let elem_type = s.u8(176);
    let count = s.u8(177) as usize;
    let mut vel_samples = None;
    if w.inline_only(s.ptr(180), 4)? {
        vel_samples = w.bulk(4, 0x60 * (s.u8(178) as usize + 1))?;
    }
    let mut vis_samples = None;
    if w.inline_only(s.ptr(184), 4)? {
        vis_samples = w.bulk(4, 0x30 * (s.u8(179) as usize + 1))?;
    }
    let mut visuals = Vec::new();
    if elem_type == FX_ELEM_TYPE_DECAL {
        if w.inline_only(s.ptr(188), 4)? {
            let a = w.load(8 * count)?;
            for i in 0..count {
                let m0 = material(w, a.ptr(8 * i), a.slot(8 * i))?;
                let m1 = material(w, a.ptr(8 * i + 4), a.slot(8 * i + 4))?;
                visuals.push(FxVisual::Mark([m0, m1]));
            }
        }
    } else if count > 1 {
        if w.inline_only(s.ptr(188), 4)? {
            let a = w.load(4 * count)?;
            for i in 0..count {
                visuals.push(visual(w, elem_type, &a, 4 * i)?);
            }
        }
    } else {
        visuals.push(visual(w, elem_type, &s, 188)?);
    }
    let effect_on_impact = w.xstring(s.ptr(216))?;
    let effect_on_death = w.xstring(s.ptr(220))?;
    let effect_emitted = w.xstring(s.ptr(224))?;
    let mut trail = None;
    if w.inline_only(s.ptr(244), 4)? {
        let t = w.load(0x1C)?;
        let mut def = FxTrail {
            scroll_ms: t.i32(0),
            repeat_dist: t.i32(4),
            split_dist: t.i32(8),
            ..FxTrail::default()
        };
        if w.inline_only(t.ptr(16), 4)? {
            let b = w.bulk_kept(4, 0x14 * t.i32(12).max(0) as usize)?;
            let f = |c: &[u8], o: usize| f32::from_le_bytes(c[o..o + 4].try_into().unwrap());
            def.verts = b
                .as_chunks::<0x14>()
                .0
                .iter()
                .map(|c| FxTrailVertex {
                    pos: [f(c, 0), f(c, 4)],
                    normal: [f(c, 8), f(c, 12)],
                    tex: f(c, 16),
                })
                .collect();
        }
        if w.inline_only(t.ptr(24), 2)? {
            let b = w.bulk_kept(2, 2 * t.i32(20).max(0) as usize)?;
            def.inds = b
                .as_chunks::<2>()
                .0
                .iter()
                .map(|c| u16::from_le_bytes(*c))
                .collect();
        }
        trail = Some(def);
    }
    Ok(FxElem {
        elem_type,
        visuals,
        effect_on_impact,
        effect_on_death,
        effect_emitted,
        vel_samples,
        vis_samples,
        raw: s.b,
        trail,
    })
}

/// `FxElemVisuals` at `o` of `s`, read by the element's type.
fn visual<R: BufRead>(w: &mut W<R>, elem_type: u8, s: &S, o: usize) -> Result<FxVisual> {
    Ok(match elem_type {
        0..=4 => FxVisual::Material(material(w, s.ptr(o), s.slot(o))?),
        FX_ELEM_TYPE_MODEL => FxVisual::Model(xmodel(w, s.ptr(o), s.slot(o))?),
        FX_ELEM_TYPE_RUNNER => FxVisual::Effect(w.xstring(s.ptr(o))?),
        FX_ELEM_TYPE_SOUND => FxVisual::Sound(w.xstring(s.ptr(o))?),
        _ => FxVisual::None,
    })
}

/// `FxImpactTable`, 8: name and 12 `FxImpactEntry` (0x84: 29 non-flesh
/// and 4 flesh effect handles).
#[derive(Clone, Debug)]
pub struct ImpactFx {
    pub name: String,
    pub table: Vec<ImpactEntry>,
}

#[derive(Clone, Debug)]
pub struct ImpactEntry {
    pub nonflesh: Vec<Option<Id<Fx>>>,
    pub flesh: Vec<Option<Id<Fx>>>,
}

pub fn impact_fx<R: BufRead>(w: &mut W<R>, p: Pointer, slot: u32) -> Result<Option<Id<ImpactFx>>> {
    asset(w, p, slot, Handle::ASSET, |w| {
        let s = w.load(8)?;
        w.push(Block::Virtual);
        let name = w.xstring(s.ptr(0))?.unwrap_or_default();
        let mut table = Vec::new();
        if w.inline_only(s.ptr(4), 4)? {
            let a = w.load(0x84 * 12)?;
            for i in 0..12 {
                let e = a.elem(i, 0x84);
                let handles = |w: &mut W<R>, o: usize, n: usize| {
                    (0..n)
                        .map(|k| fx(w, e.ptr(o + 4 * k), e.slot(o + 4 * k)))
                        .collect::<Result<Vec<_>>>()
                };
                let nonflesh = handles(w, 0, 29)?;
                let flesh = handles(w, 116, 4)?;
                table.push(ImpactEntry { nonflesh, flesh });
            }
        }
        w.pop()?;
        Ok(push(&mut w.out.impact_fx, ImpactFx { name, table }))
    })
}
