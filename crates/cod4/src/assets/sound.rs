//! Sound aliases, falloff curves and loaded (in-zone) sounds.

use super::{Id, asset, push};
use crate::walk::{Follow, Handle, S, W};
use crate::zone::{Block, Pointer};
use anyhow::Result;
use std::io::BufRead;

/// `snd_alias_list_t`, 0xC: name, `head`, `count`.
#[derive(Clone, Debug)]
pub struct SoundAliasList {
    pub name: String,
    pub aliases: Vec<SoundAlias>,
}

/// `snd_alias_t`, 0x5C.
#[derive(Clone, Debug)]
pub struct SoundAlias {
    pub name: Option<String>,
    pub subtitle: Option<String>,
    pub secondary: Option<String>,
    pub chain: Option<String>,
    pub file: Option<SoundFile>,
    pub falloff: Option<Id<SndCurve>>,
    pub speaker_map: Option<SpeakerMap>,
    /// The whole struct (volumes at 24, distances at 40, flags at 48...).
    pub raw: Vec<u8>,
}

/// `SoundFile`, 0xC: `type` (1 loaded, 2 streamed), `exists`, the ref.
#[derive(Clone, Debug)]
pub enum SoundFile {
    Loaded(Option<Id<LoadedSound>>),
    Streamed {
        dir: Option<String>,
        name: Option<String>,
    },
}

/// `SAT_LOADED`.
const SAT_LOADED: u8 = 1;

pub fn sound<R: BufRead>(
    w: &mut W<R>,
    p: Pointer,
    slot: u32,
) -> Result<Option<Id<SoundAliasList>>> {
    asset(w, p, slot, Handle::ASSET, |w| {
        let s = w.load(0xC)?;
        w.push(Block::Virtual);
        let name = w.xstring(s.ptr(0))?.unwrap_or_default();
        let aliases = match w.follow(s.ptr(4), 4)? {
            Follow::Inline => {
                let at = w.here()?;
                let n = s.u32(8) as usize;
                let a = w.load(0x5C * n)?;
                let v = (0..n)
                    .map(|i| alias(w, &a.elem(i, 0x5C)))
                    .collect::<Result<Vec<_>>>()?;
                w.remember("sound aliases", at, v.clone());
                v
            }
            Follow::At(a) => w.recall("sound aliases", a).unwrap_or_default(),
            Follow::Null => Vec::new(),
        };
        w.pop()?;
        Ok(push(&mut w.out.sounds, SoundAliasList { name, aliases }))
    })
}

fn alias<R: BufRead>(w: &mut W<R>, s: &S) -> Result<SoundAlias> {
    let name = w.xstring(s.ptr(0))?;
    let subtitle = w.xstring(s.ptr(4))?;
    let secondary = w.xstring(s.ptr(8))?;
    let chain = w.xstring(s.ptr(12))?;
    let file = match w.follow(s.ptr(16), 4)? {
        Follow::Inline => {
            let at = w.here()?;
            let f = w.load(0xC)?;
            let file = if f.u8(0) == SAT_LOADED {
                SoundFile::Loaded(loaded_sound(w, f.ptr(4), f.slot(4))?)
            } else {
                SoundFile::Streamed {
                    dir: w.xstring(f.ptr(4))?,
                    name: w.xstring(f.ptr(8))?,
                }
            };
            w.remember("sound file", at, file.clone());
            Some(file)
        }
        Follow::At(a) => w.recall("sound file", a),
        Follow::Null => None,
    };
    let falloff = snd_curve(w, s.ptr(72), s.slot(72))?;
    let speaker_map = match w.follow(s.ptr(88), 4)? {
        Follow::Inline => {
            let at = w.here()?;
            let m = w.load(0x198)?;
            let name = w.xstring(m.ptr(4))?.unwrap_or_default();
            let map = Some(SpeakerMap::read(name, &m));
            w.remember("speaker map", at, map.clone());
            map
        }
        Follow::At(a) => w.recall("speaker map", a).flatten(),
        Follow::Null => None,
    };
    Ok(SoundAlias {
        name,
        subtitle,
        secondary,
        chain,
        file,
        falloff,
        speaker_map,
        raw: s.b.clone(),
    })
}

/// One speaker's row of an `MSSChannelMap`: the output speaker and a
/// level per source channel (one for a mono source, two for stereo).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SpeakerLevels {
    pub speaker: u32,
    pub levels: [f32; 2],
}

/// `SpeakerMap`, 0x198: `isDefault` (byte 0), the name at 4, then
/// `channelMaps[2][2]` at 8, each an `MSSChannelMap` (0x64: `speakerCount`,
/// six 0x10-byte `{speaker, numLevels, levels[2]}`), indexed by
/// [source is stereo][output is multichannel] (cod4-sound.md 2).
#[derive(Clone, Debug, PartialEq)]
pub struct SpeakerMap {
    pub name: String,
    pub is_default: bool,
    pub maps: [[Vec<SpeakerLevels>; 2]; 2],
}

impl SpeakerMap {
    fn read(name: String, m: &S) -> SpeakerMap {
        let map = |k: usize| -> Vec<SpeakerLevels> {
            let at = 8 + k * 0x64;
            let n = (m.u32(at) as usize).min(6);
            (0..n)
                .map(|i| {
                    let o = at + 4 + i * 0x10;
                    SpeakerLevels {
                        speaker: m.u32(o),
                        levels: [m.f32(o + 8), m.f32(o + 12)],
                    }
                })
                .collect()
        };
        SpeakerMap {
            name,
            is_default: m.u8(0) != 0,
            maps: [[map(0), map(1)], [map(2), map(3)]],
        }
    }
}

/// `SndCurve`, 0x48: name, `knotCount`, eight (x, y) knots.
#[derive(Clone, Debug)]
pub struct SndCurve {
    pub name: String,
    pub knots: Vec<[f32; 2]>,
}

pub fn snd_curve<R: BufRead>(w: &mut W<R>, p: Pointer, slot: u32) -> Result<Option<Id<SndCurve>>> {
    asset(w, p, slot, Handle::ASSET, |w| {
        let s = w.load(0x48)?;
        w.push(Block::Virtual);
        let name = w.xstring(s.ptr(0))?.unwrap_or_default();
        w.pop()?;
        let n = (s.u32(4) as usize).min(8);
        let knots = (0..n)
            .map(|i| [s.f32(8 + 8 * i), s.f32(12 + 8 * i)])
            .collect();
        Ok(push(&mut w.out.snd_curves, SndCurve { name, knots }))
    })
}

/// `LoadedSound`, 0x2C: name, then `MssSound` (an `AILSOUNDINFO` and the
/// data slot at 0x28).
#[derive(Clone, Debug)]
pub struct LoadedSound {
    pub name: String,
    pub format: u32,
    pub rate: u32,
    pub bits: u32,
    pub channels: u32,
    pub samples: u32,
    pub block_size: u32,
    pub data_len: u32,
    /// The sample data, under [`crate::walk::WalkOptions::keep_bulk`].
    pub data: Option<Vec<u8>>,
}

pub fn loaded_sound<R: BufRead>(
    w: &mut W<R>,
    p: Pointer,
    slot: u32,
) -> Result<Option<Id<LoadedSound>>> {
    asset(w, p, slot, Handle::ASSET, |w| {
        let s = w.load(0x2C)?;
        w.push(Block::Virtual);
        let name = w.xstring(s.ptr(0))?.unwrap_or_default();
        let data_len = s.u32(12);
        // The samples ride the temp block, aliased like an asset handle.
        w.push(Block::Temp);
        let data = match s.ptr(40) {
            Pointer::Null | Pointer::Offset { .. } => None,
            p => {
                if p == Pointer::InlineShared {
                    w.alias_slot();
                }
                w.bulk(1, data_len as usize)?
            }
        };
        w.pop()?;
        w.pop()?;
        Ok(push(
            &mut w.out.loaded_sounds,
            LoadedSound {
                name,
                format: s.u32(4),
                rate: s.u32(16),
                bits: s.u32(20),
                channels: s.u32(24),
                samples: s.u32(28),
                block_size: s.u32(32),
                data_len,
                data,
            },
        ))
    })
}
