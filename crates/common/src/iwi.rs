//! CoD2's `.iwi` images (version 5): `Image_LoadFromFile` and the loaders it
//! dispatches to. Layout, format table and addresses in
//! docs/research/cod2-iwi-iwd.md. CoD4's version 6 keeps the header, the
//! formats and the level order ([`Iwi::parse_v6`], cod4-world.md).

use crate::assets::{Image, ImageData, MAX_IMAGE_DIM, TextureFormat};
use anyhow::{Context, Result, bail, ensure};

mod wavelet;
pub use wavelet::{WaveletTables, load_tables as load_wavelet_tables};

/// `GfxImageFileHeader`: 28 bytes ahead of the pixels.
pub const HEADER_LEN: usize = 0x1c;

/// The header's `format` byte, `Image_LoadFromData`'s switch.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum IwiFormat {
    /// 1: `D3DFMT_A8R8G8B8`, bytes B G R A.
    Argb32,
    /// 2: 24-bit B G R, widened to `D3DFMT_X8R8G8B8`.
    Rgb24,
    /// 3: `D3DFMT_A8L8`, bytes L A.
    La16,
    /// 4: `D3DFMT_L8`.
    L8,
    /// 5: `D3DFMT_A8`.
    A8,
    /// 6..=10: the bitmap formats 1..=5 wavelet-compressed
    /// (`Image_LoadWavelet`). Holds the bitmap format it decodes to.
    Wavelet(u8),
    /// 11: `DXT1`.
    Dxt1,
    /// 12: `DXT3`.
    Dxt3,
    /// 13: `DXT5`.
    Dxt5,
}

impl IwiFormat {
    pub fn from_byte(b: u8) -> Option<Self> {
        Some(match b {
            1 => Self::Argb32,
            2 => Self::Rgb24,
            3 => Self::La16,
            4 => Self::L8,
            5 => Self::A8,
            6..=10 => Self::Wavelet(b - 5),
            11 => Self::Dxt1,
            12 => Self::Dxt3,
            13 => Self::Dxt5,
            _ => return None,
        })
    }

    /// Bytes per pixel of a bitmap format, per 4x4 block of a DXT one.
    fn unit_bytes(self) -> Option<usize> {
        Some(match self {
            Self::Argb32 => 4,
            Self::Rgb24 => 3,
            Self::La16 => 2,
            Self::L8 | Self::A8 => 1,
            Self::Dxt1 => 8,
            Self::Dxt3 | Self::Dxt5 => 16,
            Self::Wavelet(_) => return None,
        })
    }

    /// The bitmap format a level holds once loaded: a wavelet one's, with
    /// format 7's three bytes widened to four (`Image_LoadWavelet`).
    fn decoded(self) -> Self {
        match self {
            Self::Wavelet(1 | 2) => Self::Argb32,
            Self::Wavelet(b) => Self::from_byte(b).expect("bitmap format"),
            f => f,
        }
    }

    fn is_dxt(self) -> bool {
        matches!(self, Self::Dxt1 | Self::Dxt3 | Self::Dxt5)
    }

    /// Bytes one face of a `w` x `h` level takes in the file.
    fn level_bytes(self, w: u32, h: u32) -> Option<usize> {
        let unit = self.unit_bytes()?;
        Some(if self.is_dxt() {
            w.div_ceil(4) as usize * h.div_ceil(4) as usize * unit
        } else {
            w as usize * h as usize * unit
        })
    }
}

/// Header flag bits (`Image_Setup`, `Image_LoadFromFile`).
pub mod flags {
    /// Skips picmip.
    pub const NOPICMIP: u8 = 0x1;
    /// One level only; also skips picmip.
    pub const NOMIPMAPS: u8 = 0x2;
    /// Six faces per level (`Image_CreateCubeTexture`).
    pub const CUBE: u8 = 0x4;
    /// A volume (`Image_Create3DTexture`).
    pub const VOLUME: u8 = 0x8;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IwiHeader {
    pub format: IwiFormat,
    pub flags: u8,
    pub width: u16,
    pub height: u16,
    pub depth: u16,
    /// File offset where each of levels 0..4 ends (level 0, the largest,
    /// is stored last). The loader reads the levels in sequence and never
    /// consults these.
    pub level_ends: [u32; 4],
}

impl IwiHeader {
    pub fn parse(data: &[u8]) -> Result<Self> {
        Self::parse_version(data, 5)
    }

    /// [`Self::parse`] for a file that must be `version` (CoD4's 6).
    pub fn parse_version(data: &[u8], version: u8) -> Result<Self> {
        ensure!(data.len() >= HEADER_LEN, "iwi shorter than its header");
        ensure!(&data[0..3] == b"IWi", "not an IW image");
        ensure!(
            data[3] == version,
            "iwi is version {} but should be version {version}",
            data[3]
        );
        let Some(format) = IwiFormat::from_byte(data[4]) else {
            bail!("unknown iwi format {}", data[4]);
        };
        let u16_at = |o: usize| u16::from_le_bytes([data[o], data[o + 1]]);
        let u32_at = |o: usize| u32::from_le_bytes(data[o..o + 4].try_into().unwrap());
        Ok(Self {
            format,
            flags: data[5],
            width: u16_at(6),
            height: u16_at(8),
            depth: u16_at(10),
            level_ends: [u32_at(12), u32_at(16), u32_at(20), u32_at(24)],
        })
    }

    pub fn is_cube(&self) -> bool {
        self.flags & flags::CUBE != 0
    }

    pub fn faces(&self) -> usize {
        if self.is_cube() { 6 } else { 1 }
    }

    /// Levels stored: one under `NOMIPMAPS`, else down to 1x1 of the
    /// largest of width, height and depth.
    pub fn level_count(&self) -> u32 {
        if self.flags & flags::NOMIPMAPS != 0 {
            1
        } else {
            let side = self.width.max(self.height).max(self.depth).max(1);
            u32::from(side).ilog2() + 1
        }
    }
}

/// One level, every face of it, raw as stored.
pub struct IwiLevel {
    pub width: u32,
    pub height: u32,
    /// One entry, or six for a cube in `D3DCUBEMAP_FACES` order
    /// (+X, -X, +Y, -Y, +Z, -Z).
    pub faces: Vec<Vec<u8>>,
}

/// A parsed `.iwi`: the header and its levels, largest first.
pub struct Iwi {
    pub header: IwiHeader,
    pub levels: Vec<IwiLevel>,
    /// File offset where reading each level stopped, largest first: what
    /// the header's `level_ends` should hold.
    pub read_ends: Vec<usize>,
}

impl Iwi {
    /// Splits the pixel data into levels and faces. The file holds the
    /// smallest level first, each level's faces together. A wavelet file
    /// decodes through the tables [`load_wavelet_tables`] read, and fails
    /// without them.
    pub fn parse(data: &[u8]) -> Result<Self> {
        let tables = wavelet::tables();
        Self::parse_with(data, tables)
    }

    /// [`Iwi::parse`] with the wavelet tables given.
    pub fn parse_with(data: &[u8], tables: Option<&WaveletTables>) -> Result<Self> {
        Self::parse_version_with(data, 5, tables)
    }

    /// A CoD4 `.iwi` (version 6), which [`Iwi::parse`] refuses as CoD2
    /// does. The wavelet tables are the ones [`load_wavelet_tables`] found
    /// (CoD4's `iw3mp.exe` carries CoD2's three).
    pub fn parse_v6(data: &[u8]) -> Result<Self> {
        Self::parse_version_with(data, 6, wavelet::tables())
    }

    /// A CoD2 (version 5) or CoD4 (version 6) file, by its version byte:
    /// the two share the header and the layouts.
    pub fn parse_any(data: &[u8]) -> Result<Self> {
        match data.get(3) {
            Some(6) => Self::parse_v6(data),
            _ => Self::parse(data),
        }
    }

    fn parse_version_with(
        data: &[u8],
        version: u8,
        tables: Option<&WaveletTables>,
    ) -> Result<Self> {
        let header = IwiHeader::parse_version(data, version)?;
        let format = header.format;
        ensure!(
            header.flags & flags::VOLUME == 0,
            "volume iwi is not supported"
        );
        let (w, h) = (u32::from(header.width), u32::from(header.height));
        ensure!(w > 0 && h > 0, "zero-sized iwi");
        ensure!(
            w <= MAX_IMAGE_DIM && h <= MAX_IMAGE_DIM,
            "iwi {w}x{h} exceeds {MAX_IMAGE_DIM}"
        );
        if let IwiFormat::Wavelet(bitmap) = format {
            let tables = tables.with_context(|| {
                format!(
                    "wavelet-compressed iwi (format {}) without decode tables",
                    data[4]
                )
            })?;
            let src = IwiFormat::from_byte(bitmap)
                .and_then(IwiFormat::unit_bytes)
                .expect("bitmap format");
            let (levels, read_ends) = wavelet::decode(
                tables,
                &data[HEADER_LEN..],
                (w, h),
                header.level_count(),
                header.faces(),
                src,
            )?;
            return Ok(Self {
                header,
                levels,
                read_ends,
            });
        }
        let mut off = HEADER_LEN;
        let mut levels = Vec::new();
        let mut read_ends = Vec::new();
        for level in (0..header.level_count()).rev() {
            let (lw, lh) = ((w >> level).max(1), (h >> level).max(1));
            let size = format.level_bytes(lw, lh).expect("not wavelet");
            let mut faces = Vec::with_capacity(header.faces());
            for _ in 0..header.faces() {
                ensure!(
                    off + size <= data.len(),
                    "iwi truncated at level {level}: {} bytes, needs {}",
                    data.len(),
                    off + size
                );
                faces.push(data[off..off + size].to_vec());
                off += size;
            }
            levels.push(IwiLevel {
                width: lw,
                height: lh,
                faces,
            });
            read_ends.push(off);
        }
        levels.reverse();
        read_ends.reverse();
        Ok(Self {
            header,
            levels,
            read_ends,
        })
    }

    /// The first face as an [`Image`] the renderer uploads: DXT keeps every
    /// level, a bitmap its largest as RGBA.
    pub fn to_image(&self) -> Image {
        self.face_image(0)
    }

    /// Face `face` (`D3DCUBEMAP_FACES` order on a cube) as [`to_image`]
    /// builds the first. Panics past the file's faces.
    ///
    /// [`to_image`]: Iwi::to_image
    pub fn face_image(&self, face: usize) -> Image {
        let (width, height) = (self.levels[0].width, self.levels[0].height);
        let bc = |format| ImageData::Bc {
            format,
            mips: self.levels.iter().map(|l| l.faces[face].clone()).collect(),
        };
        let data = match self.header.format {
            IwiFormat::Dxt1 => bc(TextureFormat::Bc1RgbaUnorm),
            IwiFormat::Dxt3 => bc(TextureFormat::Bc2RgbaUnorm),
            IwiFormat::Dxt5 => bc(TextureFormat::Bc3RgbaUnorm),
            f => ImageData::Rgba8(bitmap_to_rgba(f.decoded(), &self.levels[0].faces[face])),
        };
        Image {
            width,
            height,
            data,
        }
    }
}

/// Parses an `.iwi` of either version ([`Iwi::parse_any`]) and returns its
/// first face (see [`Iwi::to_image`]).
pub fn decode(data: &[u8]) -> Result<Image> {
    Ok(Iwi::parse_any(data)?.to_image())
}

/// [`decode`], except that a wavelet-compressed file with no decode tables
/// loaded warns and gives a 1x1 mid grey so the surface still draws.
pub fn decode_or_placeholder(data: &[u8]) -> Result<Image> {
    let version = if data.get(3) == Some(&6) { 6 } else { 5 };
    if let IwiFormat::Wavelet(_) = IwiHeader::parse_version(data, version)?.format
        && wavelet::tables().is_none()
    {
        log::warn!("wavelet-compressed iwi, drawing a grey placeholder");
        return Ok(Image {
            width: 1,
            height: 1,
            data: ImageData::Rgba8(vec![128, 128, 128, 255]),
        });
    }
    decode(data)
}

/// A bitmap level as RGBA, each format sampled as its D3D9 format reads:
/// L8 is (l, l, l, 1), A8 is (0, 0, 0, a).
fn bitmap_to_rgba(format: IwiFormat, src: &[u8]) -> Vec<u8> {
    let mut px = Vec::with_capacity(src.len() * 4);
    match format {
        IwiFormat::Argb32 => {
            for c in src.as_chunks::<4>().0 {
                px.extend_from_slice(&[c[2], c[1], c[0], c[3]]);
            }
        }
        IwiFormat::Rgb24 => {
            for c in src.as_chunks::<3>().0 {
                px.extend_from_slice(&[c[2], c[1], c[0], 255]);
            }
        }
        IwiFormat::La16 => {
            for c in src.as_chunks::<2>().0 {
                px.extend_from_slice(&[c[0], c[0], c[0], c[1]]);
            }
        }
        IwiFormat::L8 => {
            for &l in src {
                px.extend_from_slice(&[l, l, l, 255]);
            }
        }
        IwiFormat::A8 => {
            for &a in src {
                px.extend_from_slice(&[0, 0, 0, a]);
            }
        }
        IwiFormat::Dxt1 | IwiFormat::Dxt3 | IwiFormat::Dxt5 | IwiFormat::Wavelet(_) => {
            unreachable!("not a bitmap format")
        }
    }
    px
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header(format: u8, flags: u8, w: u16, h: u16, ends: [u32; 4]) -> Vec<u8> {
        let mut b = b"IWi\x05".to_vec();
        b.extend_from_slice(&[format, flags]);
        for v in [w, h, 1] {
            b.extend_from_slice(&v.to_le_bytes());
        }
        for e in ends {
            b.extend_from_slice(&e.to_le_bytes());
        }
        b
    }

    #[test]
    fn dxt1_levels_smallest_first() {
        // 8x4: levels 8x4 (2 blocks), 4x2 (1), 2x1 (1), 1x1 (1), stored 1x1 first
        let mut b = header(11, 0, 8, 4, [0; 4]);
        for (i, blocks) in [1usize, 1, 1, 2].iter().enumerate() {
            b.extend(std::iter::repeat_n(i as u8, blocks * 8));
        }
        let iwi = Iwi::parse(&b).unwrap();
        assert_eq!(iwi.header.level_count(), 4);
        let dims: Vec<_> = iwi.levels.iter().map(|l| (l.width, l.height)).collect();
        assert_eq!(dims, [(8, 4), (4, 2), (2, 1), (1, 1)]);
        assert_eq!(iwi.levels[0].faces[0], [3; 16]);
        assert_eq!(iwi.levels[3].faces[0], [0; 8]);
        let img = iwi.to_image();
        let ImageData::Bc { format, mips } = img.data else {
            panic!("not bc")
        };
        assert_eq!(format, TextureFormat::Bc1RgbaUnorm);
        assert_eq!(mips.len(), 4);
        assert_eq!((img.width, img.height), (8, 4));
    }

    #[test]
    fn bitmaps_swizzle_to_rgba() {
        let mut b = header(1, flags::NOMIPMAPS, 1, 1, [32; 4]);
        b.extend_from_slice(&[1, 2, 3, 4]);
        let ImageData::Rgba8(px) = decode(&b).unwrap().data else {
            panic!()
        };
        assert_eq!(px, [3, 2, 1, 4]);

        let mut b = header(2, flags::NOMIPMAPS, 1, 1, [31; 4]);
        b.extend_from_slice(&[1, 2, 3]);
        let ImageData::Rgba8(px) = decode(&b).unwrap().data else {
            panic!()
        };
        assert_eq!(px, [3, 2, 1, 255]);

        let mut b = header(3, flags::NOMIPMAPS, 2, 1, [32; 4]);
        b.extend_from_slice(&[10, 20, 30, 40]);
        let ImageData::Rgba8(px) = decode(&b).unwrap().data else {
            panic!()
        };
        assert_eq!(px, [10, 10, 10, 20, 30, 30, 30, 40]);
    }

    #[test]
    fn cube_has_six_faces_per_level() {
        // 4x4 DXT5 cube: levels 1x1, 2x2, 4x4, one block a face each
        let mut b = header(13, flags::CUBE, 4, 4, [0; 4]);
        for level in 0..3u8 {
            for face in 0..6u8 {
                b.extend(std::iter::repeat_n(level * 10 + face, 16));
            }
        }
        let iwi = Iwi::parse(&b).unwrap();
        assert_eq!(iwi.levels.len(), 3);
        assert!(iwi.levels.iter().all(|l| l.faces.len() == 6));
        assert_eq!(iwi.levels[0].faces[5], [25; 16]);
        assert_eq!(iwi.levels[2].faces[1], [1; 16]);
        let ImageData::Bc { mips, .. } = iwi.face_image(4).data else {
            panic!("not bc")
        };
        assert_eq!(mips, [[24; 16], [14; 16], [4; 16]]);
    }

    #[test]
    fn refuses_bad_files() {
        let mut b = header(11, 0, 4, 4, [0; 4]);
        b.extend_from_slice(&[0; 16]); // 3 levels need 24 bytes
        assert!(Iwi::parse(&b).is_err());
        let mut v6 = header(11, 0, 4, 4, [0; 4]);
        v6[3] = 6;
        assert!(Iwi::parse(&v6).is_err());
        let wav = header(7, 0, 4, 4, [0; 4]);
        let err = Iwi::parse_with(&wav, None).err().unwrap().to_string();
        assert!(err.contains("wavelet"), "{err}");
        assert!(Iwi::parse(b"IWi").is_err());
        assert!(Iwi::parse(&header(14, 0, 4, 4, [0; 4])).is_err());
    }
}
