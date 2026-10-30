//! The binary `materials/<name>` file: the in-memory `MaterialObj` with file
//! offsets where pointers go (docs/research/cod2-materials.md 2).

use anyhow::{Result, bail, ensure};

/// `MaterialObj`'s size; the texture table always starts here.
pub const HEADER_LEN: usize = 0x44;
const TEXTURE_LEN: usize = 12;
const CONSTANT_LEN: usize = 20;
const WATER_LEN: usize = 32;

/// `MaterialTextureDef::semantic` values seen in stock (2.1).
pub mod semantic {
    /// The 2D materials' `colorMap`; 0 and 1 take no picmip.
    pub const TWO_D: u8 = 0;
    pub const COLOR: u8 = 2;
    pub const NORMAL: u8 = 3;
    pub const SPECULAR: u8 = 4;
    /// `u` points at a [`super::WaterDef`], not an image name.
    pub const WATER: u8 = 5;
}

/// The two stateBits words (cod2-materials.md 3).
pub type StateBits = [u32; 2];

#[derive(Clone, Debug, PartialEq)]
pub struct MaterialFile {
    pub name: String,
    /// Radiant's preview image.
    pub ref_image_name: String,
    pub hash_index: u16,
    pub sorted_index: u16,
    /// 0x8 sky, 0x4 tools and water (skipped by the cell walk).
    pub game_flags: u8,
    pub sort_key: u8,
    pub atlas_rows: u8,
    pub atlas_columns: u8,
    pub max_deform_move: f32,
    pub deform_flags: u8,
    /// Editor category.
    pub usage: u8,
    pub tool_flags: u16,
    pub locale: u32,
    pub auto_tex_scale: [u16; 2],
    pub tess_size: f32,
    pub surface_flags: u32,
    pub contents: u32,
    pub state_bits: StateBits,
    pub technique_set: String,
    pub textures: Vec<TextureDef>,
    pub constants: Vec<ConstantDef>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TextureDef {
    /// The shader's name for it: `colorMap`, `normalMap`, ...
    pub name: String,
    pub sampler: SamplerState,
    pub semantic: u8,
    /// `unused_0`, `unused_1`: 0 in every stock record.
    pub unused: [u8; 2],
    pub image: TextureImage,
}

#[derive(Clone, Debug, PartialEq)]
pub enum TextureImage {
    /// An `images/<name>.iwi` base name, a `$` code image or a `~` packed one
    /// ([`super::image_name`]).
    Image(String),
    Water(WaterDef),
}

impl TextureDef {
    pub fn image_name(&self) -> Option<&str> {
        match &self.image {
            TextureImage::Image(n) => Some(n),
            TextureImage::Water(_) => None,
        }
    }
}

/// `MaterialWaterDef`, behind a semantic 5 texture (2.1).
#[derive(Clone, Debug, PartialEq)]
pub struct WaterDef {
    pub texture_width: i32,
    pub horizontal_world_length: f32,
    pub vertical_world_length: f32,
    pub amplitude: f32,
    pub wind_speed: f32,
    pub wind_direction: [f32; 2],
    /// 0 in every stock file; the loader stores the built image there.
    pub map: Option<String>,
}

/// `MaterialConstantDefObj`: read by a technique as `material.<name>`.
#[derive(Clone, Debug, PartialEq)]
pub struct ConstantDef {
    pub name: String,
    pub literal: [f32; 4],
}

/// `MaterialTextureDef::samplerState`, as `RB_SetSampler` decodes it (2.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SamplerState(pub u8);

/// `_defaultSamplerStateTable[r_textureMode]`: nearest, linear, bilinear,
/// trilinear, anisotropic.
pub const DEFAULT_SAMPLER_STATES: [u8; 5] = [1, 2, 6, 10, 11];

/// One entry of `_s_filterTable`: 0 none, 1 point, 2 linear, 3 anisotropic.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Filter {
    None,
    Point,
    Linear,
    Anisotropic,
}

impl Filter {
    fn of(bits: u8) -> Self {
        match bits & 3 {
            0 => Self::None,
            1 => Self::Point,
            2 => Self::Linear,
            _ => Self::Anisotropic,
        }
    }
}

impl SamplerState {
    /// The byte the backend uses: a low nibble of 0 takes the
    /// `r_textureMode` default ([`DEFAULT_SAMPLER_STATES`]).
    pub fn resolved(self, default: u8) -> Self {
        if self.0 & 0xf == 0 {
            Self(self.0 | default)
        } else {
            self
        }
    }

    pub fn min_mag(self) -> Filter {
        Filter::of(self.0)
    }

    pub fn mip(self) -> Filter {
        Filter::of(self.0 >> 2)
    }

    pub fn clamp_u(self) -> bool {
        self.0 & 0x10 != 0
    }

    pub fn clamp_v(self) -> bool {
        self.0 & 0x20 != 0
    }

    pub fn clamp_w(self) -> bool {
        self.0 & 0x40 != 0
    }
}

/// Byte ownership over the file: every byte must be owned exactly once.
struct Cover<'a> {
    data: &'a [u8],
    owned: Vec<bool>,
    strings: Vec<u32>,
}

impl<'a> Cover<'a> {
    fn own(&mut self, at: usize, len: usize, what: &str) -> Result<&'a [u8]> {
        let Some(bytes) = at.checked_add(len).and_then(|end| self.data.get(at..end)) else {
            bail!(
                "{what} at {at:#x}+{len:#x} runs past the end ({:#x})",
                self.data.len()
            );
        };
        for (i, o) in self.owned[at..at + len].iter_mut().enumerate() {
            ensure!(!*o, "{what} overlaps another field at {:#x}", at + i);
            *o = true;
        }
        Ok(bytes)
    }

    /// A NUL-terminated string at a file offset, owned once however many
    /// fields share it.
    fn string(&mut self, at: u32, what: &str) -> Result<String> {
        let start = at as usize;
        let Some(rest) = self.data.get(start..) else {
            bail!("{what} offset {at:#x} is past the end");
        };
        let Some(len) = rest.iter().position(|&b| b == 0) else {
            bail!("{what} at {at:#x} has no terminator");
        };
        if !self.strings.contains(&at) {
            self.own(start, len + 1, what)?;
            self.strings.push(at);
        }
        Ok(vcod_common::text::latin1(&rest[..len]))
    }
}

fn u16_at(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}

fn u32_at(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

fn f32_at(b: &[u8], o: usize) -> f32 {
    f32::from_bits(u32_at(b, o))
}

impl MaterialFile {
    /// Parses a whole material file; fails unless the header, tables and
    /// strings own every byte exactly once.
    pub fn parse(data: &[u8]) -> Result<Self> {
        ensure!(
            data.len() >= HEADER_LEN,
            "material file is {} bytes, shorter than its header",
            data.len()
        );
        let mut c = Cover {
            data,
            owned: vec![false; data.len()],
            strings: Vec::new(),
        };
        let h = c.own(0, HEADER_LEN, "header")?;
        let texture_count = u16_at(h, 0x34) as usize;
        let constant_count = u16_at(h, 0x36) as usize;
        let textures_at = u32_at(h, 0x3c) as usize;
        let constants_at = u32_at(h, 0x40) as usize;
        if texture_count > 0 {
            ensure!(
                textures_at == HEADER_LEN,
                "texture table at {textures_at:#x}, not {HEADER_LEN:#x}"
            );
        }
        let mut m = Self {
            name: c.string(u32_at(h, 0x00), "name")?,
            ref_image_name: c.string(u32_at(h, 0x04), "refImageName")?,
            hash_index: u16_at(h, 0x08),
            sorted_index: u16_at(h, 0x0a),
            game_flags: h[0x0c],
            sort_key: h[0x0d],
            atlas_rows: h[0x0e],
            atlas_columns: h[0x0f],
            max_deform_move: f32_at(h, 0x10),
            deform_flags: h[0x14],
            usage: h[0x15],
            tool_flags: u16_at(h, 0x16),
            locale: u32_at(h, 0x18),
            auto_tex_scale: [u16_at(h, 0x1c), u16_at(h, 0x1e)],
            tess_size: f32_at(h, 0x20),
            surface_flags: u32_at(h, 0x24),
            contents: u32_at(h, 0x28),
            state_bits: [u32_at(h, 0x2c), u32_at(h, 0x30)],
            technique_set: c.string(u32_at(h, 0x38), "techniqueSet")?,
            textures: Vec::with_capacity(texture_count),
            constants: Vec::with_capacity(constant_count),
        };
        for i in 0..texture_count {
            let r = c.own(textures_at + TEXTURE_LEN * i, TEXTURE_LEN, "texture")?;
            let semantic = r[5];
            let (sampler, unused, name_at, u) =
                (SamplerState(r[4]), [r[6], r[7]], u32_at(r, 0), u32_at(r, 8));
            let image = if semantic == semantic::WATER {
                let w = c.own(u as usize, WATER_LEN, "water def")?;
                let map_at = u32_at(w, 28);
                TextureImage::Water(WaterDef {
                    texture_width: u32_at(w, 0) as i32,
                    horizontal_world_length: f32_at(w, 4),
                    vertical_world_length: f32_at(w, 8),
                    amplitude: f32_at(w, 12),
                    wind_speed: f32_at(w, 16),
                    wind_direction: [f32_at(w, 20), f32_at(w, 24)],
                    map: match map_at {
                        0 => None,
                        at => Some(c.string(at, "water map")?),
                    },
                })
            } else {
                TextureImage::Image(c.string(u, "image name")?)
            };
            m.textures.push(TextureDef {
                name: c.string(name_at, "texture name")?,
                sampler,
                semantic,
                unused,
                image,
            });
        }
        for i in 0..constant_count {
            let r = c.own(constants_at + CONSTANT_LEN * i, CONSTANT_LEN, "constant")?;
            let literal = [f32_at(r, 4), f32_at(r, 8), f32_at(r, 12), f32_at(r, 16)];
            m.constants.push(ConstantDef {
                name: c.string(u32_at(r, 0), "constant name")?,
                literal,
            });
        }
        if let Some(at) = c.owned.iter().position(|o| !o) {
            bail!("byte {at:#x} of {:#x} belongs to no field", data.len());
        }
        Ok(m)
    }

    pub fn texture(&self, name: &str) -> Option<&TextureDef> {
        self.textures.iter().find(|t| t.name == name)
    }

    pub fn constant(&self, name: &str) -> Option<&ConstantDef> {
        self.constants.iter().find(|t| t.name == name)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A material file builder: header fields, then tables, then strings.
    pub(crate) struct Builder {
        pub buf: Vec<u8>,
        strings: Vec<(String, u32)>,
        fixups: Vec<(usize, String)>,
    }

    impl Builder {
        pub fn new() -> Self {
            Self {
                buf: vec![0; HEADER_LEN],
                strings: Vec::new(),
                fixups: Vec::new(),
            }
        }

        pub fn put_u32(&mut self, at: usize, v: u32) {
            self.buf[at..at + 4].copy_from_slice(&v.to_le_bytes());
        }

        pub fn str_at(&mut self, field: usize, s: &str) {
            self.fixups.push((field, s.to_string()));
        }

        pub fn finish(mut self) -> Vec<u8> {
            for (field, s) in std::mem::take(&mut self.fixups) {
                let at = match self.strings.iter().find(|(n, _)| *n == s) {
                    Some(&(_, at)) => at,
                    None => {
                        let at = self.buf.len() as u32;
                        self.buf.extend_from_slice(s.as_bytes());
                        self.buf.push(0);
                        self.strings.push((s, at));
                        at
                    }
                };
                self.put_u32(field, at);
            }
            self.buf
        }
    }

    /// `name`, technique set `phong_replace`, a colour map and a water
    /// normal map, one constant.
    pub(crate) fn sample() -> Vec<u8> {
        let mut b = Builder::new();
        b.str_at(0x00, "wc/test");
        b.buf[0x0c] = 3;
        b.buf[0x0d] = 3;
        b.buf[0x0e] = 1;
        b.buf[0x0f] = 1;
        b.put_u32(0x24, 0x00d0_0000);
        b.put_u32(0x28, 1);
        b.put_u32(0x2c, 0x2812_8812);
        b.put_u32(0x30, 5);
        b.buf[0x34] = 2;
        b.buf[0x36] = 1;
        b.str_at(0x38, "phong_replace");
        b.put_u32(0x3c, HEADER_LEN as u32);
        // Two texture records, then the water def, then the constant.
        b.buf.resize(HEADER_LEN + 24, 0);
        b.str_at(0x44, "colorMap");
        b.buf[0x48] = 0x30;
        b.buf[0x49] = semantic::COLOR;
        b.str_at(0x4c, "~test-gggr");
        b.str_at(0x50, "normalMap");
        b.buf[0x54] = 0x06;
        b.buf[0x55] = semantic::WATER;
        let water = b.buf.len();
        b.put_u32(0x58, water as u32);
        b.buf.resize(water + 32, 0);
        b.put_u32(water, 64);
        b.put_u32(water + 4, 37f32.to_bits());
        b.put_u32(water + 20, 1f32.to_bits());
        let con = b.buf.len();
        b.put_u32(0x40, con as u32);
        b.buf.resize(con + 20, 0);
        b.str_at(con, "detailScale");
        b.put_u32(con + 4, 16f32.to_bits());
        b.put_u32(con + 8, 8f32.to_bits());
        // `colorMap` again: shared by offset.
        b.str_at(0x04, "colorMap");
        b.finish()
    }

    #[test]
    fn parses_every_field_to_exact_consumption() {
        let m = MaterialFile::parse(&sample()).unwrap();
        assert_eq!(m.name, "wc/test");
        assert_eq!(m.ref_image_name, "colorMap");
        assert_eq!((m.game_flags, m.sort_key), (3, 3));
        assert_eq!((m.atlas_rows, m.atlas_columns), (1, 1));
        assert_eq!(m.surface_flags, 0x00d0_0000);
        assert_eq!(m.state_bits, [0x2812_8812, 5]);
        assert_eq!(m.technique_set, "phong_replace");
        let color = m.texture("colorMap").unwrap();
        assert_eq!(color.image_name(), Some("~test-gggr"));
        assert!(color.sampler.clamp_u() && color.sampler.clamp_v());
        let TextureImage::Water(w) = &m.texture("normalMap").unwrap().image else {
            panic!("no water def");
        };
        assert_eq!((w.texture_width, w.horizontal_world_length), (64, 37.0));
        assert_eq!(w.wind_direction, [1.0, 0.0]);
        assert_eq!(w.map, None);
        assert_eq!(
            m.constant("detailScale").unwrap().literal,
            [16.0, 8.0, 0.0, 0.0]
        );
    }

    #[test]
    fn rejects_stray_overlapping_and_short_files() {
        let mut stray = sample();
        stray.push(0);
        assert!(MaterialFile::parse(&stray).is_err());

        // The constant table pointed back over the texture table.
        let mut overlap = sample();
        overlap[0x40..0x44].copy_from_slice(&0x44u32.to_le_bytes());
        assert!(MaterialFile::parse(&overlap).is_err());

        let short = sample();
        assert!(MaterialFile::parse(&short[..0x30]).is_err());
        assert!(MaterialFile::parse(&short[..short.len() - 1]).is_err());

        let mut far = sample();
        far[0x38..0x3c].copy_from_slice(&0xffff_fff0u32.to_le_bytes());
        assert!(MaterialFile::parse(&far).is_err());
    }

    #[test]
    fn sampler_state_bits() {
        let s = SamplerState(0x37);
        assert_eq!(s.min_mag(), Filter::Anisotropic);
        assert_eq!(s.mip(), Filter::Point);
        assert!(s.clamp_u() && s.clamp_v() && !s.clamp_w());
        let d = SamplerState(0x30).resolved(DEFAULT_SAMPLER_STATES[3]);
        assert_eq!(d, SamplerState(0x3a));
        assert_eq!((d.min_mag(), d.mip()), (Filter::Linear, Filter::Linear));
        assert_eq!(SamplerState(0x02).resolved(10), SamplerState(0x02));
    }
}
