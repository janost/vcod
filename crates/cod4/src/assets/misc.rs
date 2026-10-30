//! Small assets: physpreset, localize, rawfile, stringtable, lightdef.

use super::material::{Image, image};
use super::{Id, asset, push};
use crate::walk::{Handle, W};
use crate::zone::{Block, Pointer};
use anyhow::Result;
use std::io::BufRead;

/// `PhysPreset`, 0x2C.
#[derive(Clone, Debug)]
pub struct PhysPreset {
    pub name: String,
    pub snd_alias_prefix: Option<String>,
    /// The whole struct as loaded (mass at 8, bounce at 12, ...).
    pub raw: Vec<u8>,
}

pub fn phys_preset<R: BufRead>(
    w: &mut W<R>,
    p: Pointer,
    slot: u32,
) -> Result<Option<Id<PhysPreset>>> {
    asset(w, p, slot, Handle::ASSET, |w| {
        let s = w.load(0x2C)?;
        w.push(Block::Virtual);
        let name = w.xstring(s.ptr(0))?.unwrap_or_default();
        let snd_alias_prefix = w.xstring(s.ptr(28))?;
        w.pop()?;
        Ok(push(
            &mut w.out.phys_presets,
            PhysPreset {
                name,
                snd_alias_prefix,
                raw: s.b,
            },
        ))
    })
}

/// `LocalizeEntry`, 8: the value slot comes first.
#[derive(Clone, Debug)]
pub struct Localize {
    pub name: String,
    pub value: Option<String>,
}

pub fn localize<R: BufRead>(w: &mut W<R>, p: Pointer, slot: u32) -> Result<Option<Id<Localize>>> {
    asset(w, p, slot, Handle::ASSET, |w| {
        let s = w.load(8)?;
        w.push(Block::Virtual);
        let value = w.xstring(s.ptr(0))?;
        let name = w.xstring(s.ptr(4))?.unwrap_or_default();
        w.pop()?;
        Ok(push(&mut w.out.localize, Localize { name, value }))
    })
}

/// `RawFile`, 0xC: name, `len`, buffer of `len + 1` bytes.
#[derive(Clone, Debug)]
pub struct RawFile {
    pub name: String,
    /// The file's `len` bytes, without the NUL the zone stores after them.
    pub data: Vec<u8>,
}

pub fn raw_file<R: BufRead>(w: &mut W<R>, p: Pointer, slot: u32) -> Result<Option<Id<RawFile>>> {
    asset(w, p, slot, Handle::ASSET, |w| {
        let s = w.load(0xC)?;
        w.push(Block::Virtual);
        let name = w.xstring(s.ptr(0))?.unwrap_or_default();
        let len = s.u32(4) as usize;
        let mut data = Vec::new();
        if w.inline_only(s.ptr(8), 1)? {
            data = w.bytes(1, len + 1)?;
            data.truncate(len);
        }
        w.pop()?;
        Ok(push(&mut w.out.raw_files, RawFile { name, data }))
    })
}

/// `StringTable`, 0x10: `columnCount * rowCount` string slots, row-major.
#[derive(Clone, Debug)]
pub struct StringTable {
    pub name: String,
    pub columns: u32,
    pub rows: u32,
    pub values: Vec<Option<String>>,
}

/// The one type whose handle is a plain pointer: no temp block, no alias.
pub fn string_table<R: BufRead>(
    w: &mut W<R>,
    p: Pointer,
    slot: u32,
) -> Result<Option<Id<StringTable>>> {
    asset(w, p, slot, Handle::PLAIN, |w| {
        let s = w.load(0x10)?;
        w.push(Block::Virtual);
        let name = w.xstring(s.ptr(0))?.unwrap_or_default();
        let (columns, rows) = (s.u32(4), s.u32(8));
        let mut values = Vec::new();
        if w.inline_only(s.ptr(12), 4)? {
            values = w.xstring_array(4, (columns * rows) as usize)?;
        }
        w.pop()?;
        Ok(push(
            &mut w.out.string_tables,
            StringTable {
                name,
                columns,
                rows,
                values,
            },
        ))
    })
}

/// `GfxLightDef`, 0x10: name, `GfxLightImage { image, samplerState }`,
/// `lmapLookupStart`.
#[derive(Clone, Debug)]
pub struct LightDef {
    pub name: String,
    pub image: Option<Id<Image>>,
    pub sampler_state: u8,
    pub lmap_lookup_start: i32,
}

pub fn light_def<R: BufRead>(w: &mut W<R>, p: Pointer, slot: u32) -> Result<Option<Id<LightDef>>> {
    asset(w, p, slot, Handle::ASSET, |w| {
        let s = w.load(0x10)?;
        w.push(Block::Virtual);
        let name = w.xstring(s.ptr(0))?.unwrap_or_default();
        let img = image(w, s.ptr(4), s.slot(4))?;
        w.pop()?;
        Ok(push(
            &mut w.out.light_defs,
            LightDef {
                name,
                image: img,
                sampler_state: s.u8(8),
                lmap_lookup_start: s.i32(12),
            },
        ))
    })
}
