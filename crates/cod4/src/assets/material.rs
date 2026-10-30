//! Materials, technique sets and images.

use super::{Id, asset, push};
use crate::walk::{Follow, Handle, S, W};
use crate::zone::{Block, Pointer};
use anyhow::Result;
use std::io::BufRead;

/// `Material`, 0x50.
#[derive(Clone, Debug)]
pub struct Material {
    pub name: String,
    pub game_flags: u8,
    pub sort_key: u8,
    pub surface_type_bits: u32,
    /// `stateBitsEntry[34]`: per technique, the state bits entry it uses.
    pub state_bits_entry: Vec<u8>,
    pub state_flags: u8,
    pub camera_region: u8,
    pub techset: Option<Id<TechSet>>,
    pub textures: Vec<TextureDef>,
    pub constants: Vec<Constant>,
    /// `GfxStateBits.loadBits[2]` per entry.
    pub state_bits: Vec<[u32; 2]>,
}

/// `MaterialTextureDef`, 0xC.
#[derive(Clone, Debug)]
pub struct TextureDef {
    pub name_hash: u32,
    pub name_start: u8,
    pub name_end: u8,
    pub sampler_state: u8,
    pub semantic: u8,
    /// The image, or for the water semantic (11) the water's own image.
    pub image: Option<Id<Image>>,
    pub water: Option<Water>,
}

/// `water_t`, 0x44: the FFT water a water-map texture carries.
#[derive(Clone, Debug)]
pub struct Water {
    pub m: u32,
    pub n: u32,
    pub raw: Vec<u8>,
    /// `H0`: `M * N` complex amplitudes (re, im f32), when the walk kept
    /// bulk data.
    pub h0: Option<Vec<u8>>,
    /// `wTerm`: `M * N` angular frequencies (f32).
    pub w_term: Option<Vec<u8>>,
}

/// `MaterialConstantDef`, 0x20: hash, 12-byte name, literal.
#[derive(Clone, Debug)]
pub struct Constant {
    pub name_hash: u32,
    pub name: [u8; 12],
    pub literal: [f32; 4],
}

/// `TS_WATER_MAP`.
const TS_WATER_MAP: u8 = 11;

pub fn material<R: BufRead>(w: &mut W<R>, p: Pointer, slot: u32) -> Result<Option<Id<Material>>> {
    asset(w, p, slot, Handle::ASSET, |w| {
        let s = w.load(0x50)?;
        w.push(Block::Virtual);
        let name = w.xstring(s.ptr(0))?.unwrap_or_default();
        let techset = techset(w, s.ptr(64), s.slot(64))?;
        let textures = match w.follow(s.ptr(68), 4)? {
            Follow::Null => Vec::new(),
            Follow::Inline => {
                let at = w.here()?;
                let t = texture_table(w, s.u8(58) as usize)?;
                w.remember("texture table", at, t.clone());
                t
            }
            Follow::At(a) => w.recall("texture table", a).unwrap_or_default(),
        };
        let constants = match w.follow(s.ptr(72), 16)? {
            Follow::Inline => {
                let c = w.load(0x20 * s.u8(59) as usize)?;
                (0..s.u8(59) as usize)
                    .map(|i| {
                        let e = c.elem(i, 0x20);
                        Constant {
                            name_hash: e.u32(0),
                            name: e.b[4..16].try_into().unwrap(),
                            literal: [e.f32(16), e.f32(20), e.f32(24), e.f32(28)],
                        }
                    })
                    .collect()
            }
            _ => Vec::new(),
        };
        let state_bits = match w.follow(s.ptr(76), 4)? {
            Follow::Inline => {
                let b = w.load(8 * s.u8(60) as usize)?;
                (0..s.u8(60) as usize)
                    .map(|i| [b.u32(8 * i), b.u32(8 * i + 4)])
                    .collect()
            }
            _ => Vec::new(),
        };
        w.pop()?;
        Ok(push(
            &mut w.out.materials,
            Material {
                name,
                game_flags: s.u8(4),
                sort_key: s.u8(5),
                surface_type_bits: s.u32(16),
                state_bits_entry: s.b[24..58].to_vec(),
                state_flags: s.u8(61),
                camera_region: s.u8(62),
                techset,
                textures,
                constants,
                state_bits,
            },
        ))
    })
}

fn texture_table<R: BufRead>(w: &mut W<R>, n: usize) -> Result<Vec<TextureDef>> {
    let t = w.load(0xC * n)?;
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let e = t.elem(i, 0xC);
        let semantic = e.u8(7);
        let (mut image_id, mut water) = (None, None);
        if semantic == TS_WATER_MAP {
            match w.follow(e.ptr(8), 4)? {
                Follow::Inline => {
                    let at = w.here()?;
                    let v = water_t(w, &mut image_id)?;
                    w.remember("water", at, (v.clone(), image_id));
                    water = Some(v);
                }
                Follow::At(a) => {
                    if let Some((v, img)) = w.recall::<(Water, Option<Id<Image>>)>("water", a) {
                        water = Some(v);
                        image_id = img;
                    }
                }
                Follow::Null => {}
            }
        } else {
            image_id = image(w, e.ptr(8), e.slot(8))?;
        }
        out.push(TextureDef {
            name_hash: e.u32(0),
            name_start: e.u8(4),
            name_end: e.u8(5),
            sampler_state: e.u8(6),
            semantic,
            image: image_id,
            water,
        });
    }
    Ok(out)
}

fn water_t<R: BufRead>(w: &mut W<R>, image_id: &mut Option<Id<Image>>) -> Result<Water> {
    let s = w.load(0x44)?;
    let (m, n) = (s.u32(12), s.u32(16));
    let cells = (m * n) as usize;
    let mut h0 = None;
    if w.inline_only(s.ptr(4), 4)? {
        h0 = w.bulk(4, 8 * cells)?; // H0: complex_s
    }
    let mut w_term = None;
    if w.inline_only(s.ptr(8), 4)? {
        w_term = w.bulk(4, 4 * cells)?; // wTerm: float
    }
    *image_id = image(w, s.ptr(64), s.slot(64))?;
    Ok(Water {
        m,
        n,
        raw: s.b,
        h0,
        w_term,
    })
}

/// `MaterialTechniqueSet`, 0x94: name, `worldVertFormat`, 34 technique
/// slots at 0xC.
#[derive(Clone, Debug)]
pub struct TechSet {
    pub name: String,
    pub world_vert_format: u8,
    pub techniques: Vec<Option<Id<Technique>>>,
}

/// `MaterialTechnique`: an 8-byte head and `passCount` 0x14-byte passes
/// inline after it.
#[derive(Clone, Debug)]
pub struct Technique {
    pub name: String,
    pub flags: u16,
    pub passes: Vec<Pass>,
}

/// `MaterialPass`, 0x14.
#[derive(Clone, Debug)]
pub struct Pass {
    /// `MaterialVertexDeclaration`, 0x64, as loaded.
    pub vertex_decl: Option<Vec<u8>>,
    pub vertex_shader: Option<Shader>,
    pub pixel_shader: Option<Shader>,
    pub per_prim_args: u8,
    pub per_obj_args: u8,
    pub stable_args: u8,
    pub custom_sampler_flags: u8,
    pub args: Vec<ShaderArg>,
}

/// `MaterialVertexShader` / `MaterialPixelShader`, 0x10: name and the
/// D3D9 bytecode (`programSize` dwords).
#[derive(Clone, Debug)]
pub struct Shader {
    pub name: String,
    pub program: Option<Vec<u8>>,
}

/// `MaterialShaderArgument`, 8: `type`, `dest`, then a code constant, a
/// sampler or (types 1 and 7) a literal.
#[derive(Clone, Debug)]
pub struct ShaderArg {
    pub kind: u16,
    pub dest: u16,
    pub value: u32,
    pub literal: Option<[f32; 4]>,
}

pub fn techset<R: BufRead>(w: &mut W<R>, p: Pointer, slot: u32) -> Result<Option<Id<TechSet>>> {
    asset(w, p, slot, Handle::ASSET, |w| {
        let s = w.load(0x94)?;
        w.push(Block::Virtual);
        let name = w.xstring(s.ptr(0))?.unwrap_or_default();
        let mut techniques = Vec::with_capacity(34);
        for i in 0..34 {
            techniques.push(match w.follow(s.ptr(12 + 4 * i), 4)? {
                Follow::Null => None,
                Follow::Inline => {
                    let at = w.here()?;
                    let t = technique(w)?;
                    let id = push(&mut w.out.techniques, t);
                    w.remember("technique", at, id);
                    Some(id)
                }
                Follow::At(a) => w.recall("technique", a),
            });
        }
        w.pop()?;
        Ok(push(
            &mut w.out.techsets,
            TechSet {
                name,
                world_vert_format: s.u8(4),
                techniques,
            },
        ))
    })
}

fn technique<R: BufRead>(w: &mut W<R>) -> Result<Technique> {
    let head = w.load(8)?;
    let n = head.u16(6) as usize;
    let passes_s = w.load(0x14 * n)?;
    let mut passes = Vec::with_capacity(n);
    for i in 0..n {
        passes.push(pass(w, &passes_s.elem(i, 0x14))?);
    }
    let name = w.xstring(head.ptr(0))?.unwrap_or_default();
    Ok(Technique {
        name,
        flags: head.u16(4),
        passes,
    })
}

fn pass<R: BufRead>(w: &mut W<R>, s: &S) -> Result<Pass> {
    let vertex_decl = match w.follow(s.ptr(0), 4)? {
        Follow::Inline => {
            let at = w.here()?;
            let d = w.load(0x64)?.b;
            w.remember("vertex decl", at, d.clone());
            Some(d)
        }
        Follow::At(a) => w.recall("vertex decl", a),
        Follow::Null => None,
    };
    let vertex_shader = shader(w, s.ptr(4), "vertex shader")?;
    let pixel_shader = shader(w, s.ptr(8), "pixel shader")?;
    let (prim, obj, stable) = (s.u8(12), s.u8(13), s.u8(14));
    let mut args = Vec::new();
    if w.inline_only(s.ptr(16), 4)? {
        let n = (prim as usize) + (obj as usize) + (stable as usize);
        let a = w.load(8 * n)?;
        for i in 0..n {
            let e = a.elem(i, 8);
            let kind = e.u16(0);
            // MTL_ARG_LITERAL_VERTEX_CONST, MTL_ARG_LITERAL_PIXEL_CONST.
            let literal = if kind == 1 || kind == 7 {
                match w.follow(e.ptr(4), 4)? {
                    Follow::Inline => {
                        let at = w.here()?;
                        let l = w.load(16)?;
                        let v = [l.f32(0), l.f32(4), l.f32(8), l.f32(12)];
                        w.remember("literal", at, v);
                        Some(v)
                    }
                    Follow::At(a) => w.recall("literal", a),
                    Follow::Null => None,
                }
            } else {
                None
            };
            args.push(ShaderArg {
                kind,
                dest: e.u16(2),
                value: e.u32(4),
                literal,
            });
        }
    }
    Ok(Pass {
        vertex_decl,
        vertex_shader,
        pixel_shader,
        per_prim_args: prim,
        per_obj_args: obj,
        stable_args: stable,
        custom_sampler_flags: s.u8(15),
        args,
    })
}

/// A vertex or pixel shader: name, then `GfxVertexShaderLoadDef { program,
/// programSize: u16 }` at 8.
fn shader<R: BufRead>(w: &mut W<R>, p: Pointer, kind: &'static str) -> Result<Option<Shader>> {
    Ok(match w.follow(p, 4)? {
        Follow::Null => None,
        Follow::At(a) => w.recall(kind, a),
        Follow::Inline => {
            let at = w.here()?;
            let s = w.load(0x10)?;
            let name = w.xstring(s.ptr(0))?.unwrap_or_default();
            let mut program = None;
            if w.inline_only(s.ptr(8), 4)? {
                program = Some(w.bytes(4, 4 * s.u16(12) as usize)?);
            }
            let sh = Shader { name, program };
            w.remember(kind, at, sh.clone());
            Some(sh)
        }
    })
}

/// `GfxImage`, 0x24.
#[derive(Clone, Debug)]
pub struct Image {
    pub name: String,
    pub map_type: u8,
    pub semantic: u8,
    pub width: u16,
    pub height: u16,
    pub depth: u16,
    pub category: u8,
    pub delay_load_pixels: bool,
    /// Pixels carried in the zone (`GfxImageLoadDef`); `None` where they
    /// come from an `.iwi`.
    pub load_def: Option<ImageLoadDef>,
}

/// `GfxImageLoadDef`: a 0x10-byte head, then `resourceSize` bytes.
#[derive(Clone, Debug)]
pub struct ImageLoadDef {
    pub level_count: u8,
    pub flags: u8,
    pub dimensions: [u16; 3],
    pub format: u32,
    pub resource_size: u32,
    /// The pixels, under [`crate::walk::WalkOptions::keep_bulk`].
    pub data: Option<Vec<u8>>,
}

pub fn image<R: BufRead>(w: &mut W<R>, p: Pointer, slot: u32) -> Result<Option<Id<Image>>> {
    asset(w, p, slot, Handle::ASSET, |w| {
        let s = w.load(0x24)?;
        w.push(Block::Virtual);
        let name = w.xstring(s.ptr(32))?.unwrap_or_default();
        // GfxTexture: the load def rides the temp block, aliased as an
        // asset handle is.
        w.push(Block::Temp);
        let load_def = match s.ptr(4) {
            Pointer::Null => None,
            Pointer::Offset { block, offset } => {
                w.recall("image load def", (block as u32) << 28 | offset)
            }
            p => {
                w.align(4)?;
                let alias = (p == Pointer::InlineShared).then(|| w.alias_slot());
                let at = w.here()?;
                let h = w.load(0x10)?;
                let size = h.u32(12);
                let data = w.bulk(1, size as usize)?;
                let d = ImageLoadDef {
                    level_count: h.u8(0),
                    flags: h.u8(1),
                    dimensions: [h.u16(2), h.u16(4), h.u16(6)],
                    format: h.u32(8),
                    resource_size: size,
                    data,
                };
                w.remember("image load def", alias.unwrap_or(at), d.clone());
                Some(d)
            }
        };
        w.pop()?;
        w.pop()?;
        Ok(push(
            &mut w.out.images,
            Image {
                name,
                map_type: s.u8(0),
                semantic: s.u8(11),
                width: s.u16(24),
                height: s.u16(26),
                depth: s.u16(28),
                category: s.u8(30),
                delay_load_pixels: s.u8(31) != 0,
                load_def,
            },
        ))
    })
}
