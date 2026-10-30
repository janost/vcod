//! Image pixels: a zone image's own (`GfxImageLoadDef`) or an `.iwi` v6
//! out of the iwds, decoded into the [`Image`] the renderer uploads.
//! Formats, layouts and sources: `docs/research/cod4-world.md` section 5.

use crate::assets::material::{Image as ZoneImage, ImageLoadDef};
use anyhow::{Result, bail, ensure};
use vcod_common::assets::{Image, ImageData, MAX_IMAGE_DIM, TextureFormat};
use vcod_common::iwi::Iwi;

/// `D3DFORMAT` values a load def's `format` holds.
pub mod d3dfmt {
    pub const A8R8G8B8: u32 = 21;
    pub const X8R8G8B8: u32 = 22;
    pub const L8: u32 = 50;
    pub const DXT1: u32 = u32::from_le_bytes(*b"DXT1");
    pub const DXT3: u32 = u32::from_le_bytes(*b"DXT3");
    pub const DXT5: u32 = u32::from_le_bytes(*b"DXT5");
}

/// `GfxImageLoadDef.flags`: a single level.
pub const IMG_FLAG_NOMIPMAPS: u8 = 0x2;

/// `GfxImage.mapType`'s cube value (`MAPTYPE_CUBE`).
pub const MAPTYPE_CUBE: u8 = 5;

/// Whether the zone carries the image's pixels (a nonzero `resourceSize`),
/// so no `.iwi` is read for it.
pub fn has_pixels(img: &ZoneImage) -> bool {
    img.load_def.as_ref().is_some_and(|d| d.resource_size > 0)
}

/// Bytes one `w` x `h` level of `format` takes, or `None` for a format
/// vcod does not decode.
fn level_bytes(format: u32, w: u32, h: u32) -> Option<usize> {
    let blocks = || w.div_ceil(4) as usize * h.div_ceil(4) as usize;
    Some(match format {
        d3dfmt::A8R8G8B8 | d3dfmt::X8R8G8B8 => 4 * (w * h) as usize,
        d3dfmt::L8 => (w * h) as usize,
        d3dfmt::DXT1 => 8 * blocks(),
        d3dfmt::DXT3 | d3dfmt::DXT5 => 16 * blocks(),
        _ => return None,
    })
}

/// A zone image's pixels, face `face` (0 on a 2D image; a cube's faces
/// in `D3DCUBEMAP_FACES` order). The data holds each face's levels
/// largest first, face after face, and must fill `resourceSize` exactly.
pub fn decode_zone(img: &ZoneImage, face: usize) -> Result<Image> {
    let Some(def) = img.load_def.as_ref().filter(|d| d.resource_size > 0) else {
        bail!("{}: no pixels in the zone", img.name);
    };
    let Some(data) = def.data.as_deref() else {
        bail!("{}: pixels not kept (walk without keep_bulk)", img.name);
    };
    let faces = if img.map_type == MAPTYPE_CUBE { 6 } else { 1 };
    decode_load_def(def, data, faces, face).map_err(|e| e.context(img.name.clone()))
}

fn decode_load_def(def: &ImageLoadDef, data: &[u8], faces: usize, face: usize) -> Result<Image> {
    ensure!(face < faces, "face {face} of {faces}");
    let [w, h, depth] = def.dimensions.map(u32::from);
    ensure!(depth <= 1, "volume image ({depth} deep)");
    ensure!(
        w > 0 && h > 0 && w <= MAX_IMAGE_DIM && h <= MAX_IMAGE_DIM,
        "bad size {w}x{h}"
    );
    // Image_CountMipmaps: one level under IMG_FLAG_NOMIPMAPS, else down to
    // 1x1; the def's own levelCount says less on a mipped cube probe.
    let levels = if def.flags & IMG_FLAG_NOMIPMAPS != 0 {
        1
    } else {
        w.max(h).ilog2() + 1
    };
    let mut sizes = Vec::with_capacity(levels as usize);
    for l in 0..levels {
        let (lw, lh) = ((w >> l).max(1), (h >> l).max(1));
        let Some(n) = level_bytes(def.format, lw, lh) else {
            bail!("load def format {:#x} not decoded", def.format);
        };
        sizes.push(n);
    }
    let face_bytes: usize = sizes.iter().sum();
    ensure!(
        face_bytes * faces == data.len(),
        "{faces} faces of {levels} levels take {} bytes, the load def holds {}",
        face_bytes * faces,
        data.len()
    );
    let mut off = face * face_bytes;
    let mut mips = Vec::with_capacity(sizes.len());
    for n in sizes {
        mips.push(&data[off..off + n]);
        off += n;
    }
    let bc = |format| ImageData::Bc {
        format,
        mips: mips.iter().map(|m| m.to_vec()).collect(),
    };
    let top = mips[0];
    let data = match def.format {
        d3dfmt::DXT1 => bc(TextureFormat::Bc1RgbaUnorm),
        d3dfmt::DXT3 => bc(TextureFormat::Bc2RgbaUnorm),
        d3dfmt::DXT5 => bc(TextureFormat::Bc3RgbaUnorm),
        d3dfmt::A8R8G8B8 => ImageData::Rgba8(
            top.chunks(4)
                .flat_map(|p| [p[2], p[1], p[0], p[3]])
                .collect(),
        ),
        d3dfmt::X8R8G8B8 => ImageData::Rgba8(
            top.chunks(4)
                .flat_map(|p| [p[2], p[1], p[0], 255])
                .collect(),
        ),
        d3dfmt::L8 => ImageData::Rgba8(top.iter().flat_map(|&l| [l, l, l, 255]).collect()),
        _ => unreachable!("sized above"),
    };
    Ok(Image {
        width: w,
        height: h,
        data,
    })
}

/// A zone cube's every level of every face as RGBA, `[face][level]`
/// largest first: a reflection probe's (A8R8G8B8 or X8R8G8B8).
pub fn decode_zone_cube(img: &ZoneImage) -> Result<Vec<Vec<Image>>> {
    ensure!(img.map_type == MAPTYPE_CUBE, "{}: not a cube", img.name);
    let Some(def) = img.load_def.as_ref().filter(|d| d.resource_size > 0) else {
        bail!("{}: no pixels in the zone", img.name);
    };
    let Some(data) = def.data.as_deref() else {
        bail!("{}: pixels not kept (walk without keep_bulk)", img.name);
    };
    ensure!(
        matches!(def.format, d3dfmt::A8R8G8B8 | d3dfmt::X8R8G8B8),
        "{}: cube format {:#x}",
        img.name,
        def.format
    );
    let [w, h, _] = def.dimensions.map(u32::from);
    ensure!(
        w > 0 && h > 0 && w <= MAX_IMAGE_DIM && h <= MAX_IMAGE_DIM,
        "bad size {w}x{h}"
    );
    let levels = if def.flags & IMG_FLAG_NOMIPMAPS != 0 {
        1
    } else {
        w.max(h).ilog2() + 1
    };
    let dims: Vec<(u32, u32)> = (0..levels)
        .map(|l| ((w >> l).max(1), (h >> l).max(1)))
        .collect();
    let face_bytes: usize = dims.iter().map(|&(lw, lh)| 4 * (lw * lh) as usize).sum();
    ensure!(
        6 * face_bytes == data.len(),
        "{}: 6 faces of {levels} levels take {} bytes, the load def holds {}",
        img.name,
        6 * face_bytes,
        data.len()
    );
    let opaque = def.format == d3dfmt::X8R8G8B8;
    let mut off = 0;
    let mut faces = Vec::with_capacity(6);
    for _ in 0..6 {
        let mut out = Vec::with_capacity(dims.len());
        for &(lw, lh) in &dims {
            let n = 4 * (lw * lh) as usize;
            let px = data[off..off + n]
                .chunks(4)
                .flat_map(|p| [p[2], p[1], p[0], if opaque { 255 } else { p[3] }])
                .collect();
            off += n;
            out.push(Image {
                width: lw,
                height: lh,
                data: ImageData::Rgba8(px),
            });
        }
        faces.push(out);
    }
    Ok(faces)
}

/// An `.iwi` v6's first face (every level of a DXT one, the largest of a
/// bitmap one as RGBA).
pub fn decode_iwi(data: &[u8]) -> Result<Image> {
    Ok(Iwi::parse_v6(data)?.to_image())
}

/// Where an image's pixels come from: the iwd path `images/<name>.iwi`.
pub fn iwi_path(name: &str) -> String {
    format!("images/{name}.iwi")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn def(format: u32, w: u16, h: u16, flags: u8, data: Vec<u8>) -> (ImageLoadDef, Vec<u8>) {
        let d = ImageLoadDef {
            level_count: 1,
            flags,
            dimensions: [w, h, 1],
            format,
            resource_size: data.len() as u32,
            data: None,
        };
        (d, data)
    }

    #[test]
    fn splits_levels_and_faces() {
        // A 4x4 DXT1 mips to 2x2 and 1x1, a block of 8 bytes each.
        let data: Vec<u8> = (0..6 * 24).map(|i| (i / 8) as u8).collect();
        let (d, data) = def(d3dfmt::DXT1, 4, 4, 0, data);
        let img = decode_load_def(&d, &data, 6, 2).unwrap();
        let ImageData::Bc { mips, .. } = img.data else {
            panic!("not bc")
        };
        assert_eq!(mips, [[6; 8], [7; 8], [8; 8]]);
        assert!(decode_load_def(&d, &data, 1, 0).is_err());
    }

    #[test]
    fn swizzles_bgra_and_widens_luminance() {
        let (d, data) = def(d3dfmt::A8R8G8B8, 1, 1, IMG_FLAG_NOMIPMAPS, vec![1, 2, 3, 4]);
        let ImageData::Rgba8(px) = decode_load_def(&d, &data, 1, 0).unwrap().data else {
            panic!()
        };
        assert_eq!(px, [3, 2, 1, 4]);
        let (d, data) = def(d3dfmt::L8, 2, 1, IMG_FLAG_NOMIPMAPS, vec![9, 7]);
        let ImageData::Rgba8(px) = decode_load_def(&d, &data, 1, 0).unwrap().data else {
            panic!()
        };
        assert_eq!(px, [9, 9, 9, 255, 7, 7, 7, 255]);
    }
}
