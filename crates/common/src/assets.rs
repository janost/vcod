use crate::pk3::Pk3Fs;
use anyhow::{bail, ensure, Result};
use std::collections::HashMap;

/// Block-compressed layouts from `parse_dds`. Variant names mirror
/// `wgpu::TextureFormat` so the renderer's mapping is a lookup; no wgpu here.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(clippy::enum_variant_names)]
pub enum TextureFormat {
    Bc1RgbaUnormSrgb,
    Bc2RgbaUnormSrgb,
    Bc3RgbaUnormSrgb,
}

impl TextureFormat {
    /// Bytes per 4x4 block.
    pub fn block_size(self) -> u32 {
        match self {
            TextureFormat::Bc1RgbaUnormSrgb => 8,
            TextureFormat::Bc2RgbaUnormSrgb | TextureFormat::Bc3RgbaUnormSrgb => 16,
        }
    }
}

pub enum ImageData {
    Bc {
        format: TextureFormat,
        mips: Vec<Vec<u8>>,
    },
    Rgba8(Vec<u8>),
}

pub struct Image {
    pub width: u32,
    pub height: u32,
    pub data: ImageData,
}

/// Largest texture side any decoder here accepts; stock textures top out
/// at 2048. Bounds every allocation the header sizes.
pub const MAX_IMAGE_DIM: u32 = 8192;

pub fn parse_dds(data: &[u8]) -> Result<Image> {
    ensure!(
        data.len() >= 128 && &data[0..4] == b"DDS ",
        "not a DDS file"
    );
    let u = |o: usize| u32::from_le_bytes(data[o..o + 4].try_into().unwrap());
    let (height, width) = (u(12), u(16));
    ensure!(width > 0 && height > 0, "zero-sized DDS");
    ensure!(
        width <= MAX_IMAGE_DIM && height <= MAX_IMAGE_DIM,
        "DDS {width}x{height} exceeds {MAX_IMAGE_DIM}"
    );
    let mip_count = u(28).max(1);
    let max_mips = width.max(height).ilog2() + 1;
    ensure!(
        mip_count <= max_mips,
        "DDS mip count {mip_count} exceeds the {max_mips}-level chain of {width}x{height}"
    );
    let pf_flags = u(80);

    if pf_flags & 0x4 != 0 {
        let fourcc = &data[84..88];
        let (format, block_size) = match fourcc {
            b"DXT1" => (TextureFormat::Bc1RgbaUnormSrgb, 8),
            b"DXT3" => (TextureFormat::Bc2RgbaUnormSrgb, 16),
            b"DXT5" => (TextureFormat::Bc3RgbaUnormSrgb, 16),
            _ => bail!(
                "unsupported DDS fourcc {:?}",
                String::from_utf8_lossy(fourcc)
            ),
        };
        let mut mips = Vec::new();
        let mut off = 128usize;
        let (mut w, mut h) = (width, height);
        for _ in 0..mip_count {
            let size = w.max(1).div_ceil(4) as usize * h.max(1).div_ceil(4) as usize * block_size;
            ensure!(off + size <= data.len(), "DDS truncated");
            mips.push(data[off..off + size].to_vec());
            off += size;
            w = (w / 2).max(1);
            h = (h / 2).max(1);
        }
        Ok(Image {
            width,
            height,
            data: ImageData::Bc { format, mips },
        })
    } else if pf_flags & 0x40 != 0 {
        // uncompressed BGRA / BGR
        let bitcount = u(88);
        ensure!(
            matches!(bitcount, 24 | 32),
            "unsupported uncompressed DDS bitcount {bitcount}"
        );
        // u64: the dimension cap keeps this under 2^28, but u32 would wrap
        // on the header values before the cap is known to hold
        let pixels = width as u64 * height as u64;
        let expected = (pixels * bitcount as u64 / 8) as usize;
        ensure!(
            data.len() - 128 >= expected,
            "DDS truncated: {} bytes of pixels, {width}x{height}x{bitcount} needs {expected}",
            data.len() - 128
        );
        let src = &data[128..128 + expected];
        let mut px = Vec::with_capacity(pixels as usize * 4);
        if bitcount == 32 {
            for c in src.as_chunks::<4>().0 {
                px.extend_from_slice(&[c[2], c[1], c[0], c[3]]);
            }
        } else {
            for c in src.as_chunks::<3>().0 {
                px.extend_from_slice(&[c[2], c[1], c[0], 255]);
            }
        }
        Ok(Image {
            width,
            height,
            data: ImageData::Rgba8(px),
        })
    } else {
        bail!("unsupported DDS pixel format flags {pf_flags:#x}")
    }
}

/// Retail's `*default`, bound for every material whose image cannot be found:
/// 16x16 of RGBA (32, 32, 32, 32) inside a one-pixel opaque black border.
/// From `R_CreateDefaultImage`, CoDMP.exe 1.1 @ 0x4f0380.
pub fn default_image() -> Image {
    const DIM: u32 = 16;
    let mut px = vec![32u8; (DIM * DIM * 4) as usize];
    let mut edge = |x: u32, y: u32| {
        let o = ((y * DIM + x) * 4) as usize;
        px[o..o + 4].copy_from_slice(&[0, 0, 0, 255]);
    };
    for i in 0..DIM {
        edge(i, 0);
        edge(i, DIM - 1);
        edge(0, i);
        edge(DIM - 1, i);
    }
    Image {
        width: DIM,
        height: DIM,
        data: ImageData::Rgba8(px),
    }
}

/// 1x1 opaque white; bound for bundle images that retail binds its own
/// default for (e.g. `textures/battleship/deckflag_np.tga` ships in no pak).
pub fn white_1x1() -> Image {
    Image {
        width: 1,
        height: 1,
        data: ImageData::Rgba8(vec![255, 255, 255, 255]),
    }
}

/// The engine-generated `$dlight` light blob, never a file on disk (CoDMP
/// binds it for `perlight` bundles, e.g. the pak9 window.shader one). A
/// bright white core falling off smoothly to transparent; the neuville
/// window glass samples it so the panes show a soft glow instead of the
/// default-image placeholder.
pub fn dlight_blob() -> Image {
    const DIM: u32 = 64;
    let r = (DIM as f32) * 0.5;
    let mut px = Vec::with_capacity((DIM * DIM * 4) as usize);
    for y in 0..DIM {
        for x in 0..DIM {
            // Q3's dlight image is a smooth white radial falloff; square the
            // distance so the core stays bright and the edge fades out.
            let dx = x as f32 + 0.5 - r;
            let dy = y as f32 + 0.5 - r;
            let d = (dx * dx + dy * dy).sqrt();
            let a = (1.0 - (d / r)).max(0.0);
            let a = a * a;
            let v = (255.0 * a) as u8;
            px.extend_from_slice(&[255, 255, 255, v]);
        }
    }
    Image {
        width: DIM,
        height: DIM,
        data: ImageData::Rgba8(px),
    }
}

/// Material facts from `scripts/*.shader` (world) and `fxshaders/*.shader`
/// (effects, pak5); the scan is by suffix, not directory. Thin queries over
/// [`ShaderLib`] for the consumers that predate the full parser.
pub struct Shaders {
    lib: crate::shader::ShaderLib,
}

impl Shaders {
    /// First texture path of the material (lowercased, image extension
    /// stripped), for implicit-material loading.
    pub fn image(&self, name: &str) -> Option<&str> {
        self.lib.image(name)
    }

    /// The whole material->image table, for helpers that take the map form.
    pub fn image_map(&self) -> &HashMap<String, String> {
        self.lib.image_map()
    }

    /// The stage carrying the material's first image blends onto GL_ONE;
    /// the fx pass draws such quads additive.
    pub fn is_additive(&self, name: &str) -> bool {
        self.lib.is_additive(name)
    }
    pub fn uses_polygon_offset(&self, name: &str) -> bool {
        self.lib.uses_polygon_offset(name)
    }
}

pub fn load_shaders(fs: &Pk3Fs) -> Shaders {
    Shaders {
        lib: crate::shader::ShaderLib::load(fs),
    }
}

/// Probed in this order.
pub const IMAGE_EXTS: [&str; 3] = [".dds", ".tga", ".jpg"];

/// First `base + ext` present in `fs`, without reading it.
pub fn probe_image_path(fs: &Pk3Fs, base: &str) -> Option<String> {
    IMAGE_EXTS
        .iter()
        .map(|ext| format!("{base}{ext}"))
        .find(|path| fs.contains(path))
}

/// The bundle-image probe the renderer and the corpus test share: path as-is,
/// else with a known extension stripped, probed over [`IMAGE_EXTS`] (scripts
/// often name the `.tga` while the art ships as `.dds`, e.g. killIcon).
pub fn resolve_bundle_image(fs: &Pk3Fs, path: &str) -> Option<String> {
    if fs.contains(path) {
        return Some(path.to_string());
    }
    let base = match path.rfind('.') {
        Some(i) if IMAGE_EXTS.iter().any(|e| path[i..].eq_ignore_ascii_case(e)) => &path[..i],
        _ => path,
    };
    probe_image_path(fs, base)
}

fn rgba_image(img: image::DynamicImage) -> Image {
    let rgba = img.to_rgba8();
    Image {
        width: rgba.width(),
        height: rgba.height(),
        data: ImageData::Rgba8(rgba.into_raw()),
    }
}

/// `image` decode with `MAX_IMAGE_DIM` enforced; `format` None sniffs.
fn decode_with_image(raw: &[u8], format: Option<image::ImageFormat>) -> Result<Image> {
    let mut reader = image::ImageReader::new(std::io::Cursor::new(raw));
    match format {
        Some(f) => reader.set_format(f),
        None => reader = reader.with_guessed_format()?,
    }
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_IMAGE_DIM);
    limits.max_image_height = Some(MAX_IMAGE_DIM);
    reader.limits(limits);
    Ok(rgba_image(reader.decode()?))
}

/// `ext` is the lowercase dotted extension; anything but dds and tga is sniffed.
fn decode_image(ext: &str, raw: &[u8]) -> Result<Image> {
    match ext {
        ".dds" => parse_dds(raw),
        ".tga" => decode_with_image(raw, Some(image::ImageFormat::Tga)),
        _ => decode_with_image(raw, None),
    }
}

fn try_load_image(fs: &Pk3Fs, name: &str) -> Option<Image> {
    for ext in IMAGE_EXTS {
        let Some(raw) = fs.read(&format!("{name}{ext}")) else {
            continue;
        };
        match decode_image(ext, &raw) {
            Ok(img) => return Some(img),
            Err(e) => log::warn!("bad {ext} for {name}: {e}"),
        }
    }
    None
}

/// File probe first, then the shader script image. Never fails: missing or
/// broken assets warn and return retail's default image.
pub fn load_material_image(fs: &Pk3Fs, shaders: &Shaders, name: &str) -> Image {
    if let Some(img) = try_load_image(fs, name) {
        return img;
    }
    if let Some(mapped) = shaders.image(name) {
        if let Some(img) = try_load_image(fs, mapped) {
            return img;
        }
        log::warn!("shader {name} maps to missing image {mapped}");
    }
    log::warn!("no texture found for material {name}, using the default image");
    default_image()
}

/// Foliage skins (`foliage_masked@`, `foliage_detail@`, tga and dds) store
/// the cutout mask inverted: leaves at alpha 0, fill at 255. Measured by RGB
/// variance per alpha half over every masked skin in the retail pk3s. The
/// `treeshdw_*` tree-shadow decals are the one normal set; the `shdw_*`
/// decals measure inverted despite the name. Other `*_masked@` prefixes
/// (metal, cloth, flesh) are normal and left alone.
fn is_inverted_foliage_mask(filename: &str) -> bool {
    let lower = filename.to_lowercase();
    ["foliage_masked@", "foliage_detail@"]
        .iter()
        .filter_map(|p| lower.strip_prefix(p))
        .any(|rest| !rest.starts_with("treeshdw"))
}

/// Flips alpha in place. BC2 alpha is explicit nibbles in the first 8 bytes
/// of each block; BC3 goes through `invert_bc3_alpha`.
fn invert_mask_alpha(data: &mut ImageData) {
    match data {
        ImageData::Rgba8(px) => {
            for a in px.iter_mut().skip(3).step_by(4) {
                *a = 255 - *a;
            }
        }
        ImageData::Bc { format, mips } => match format {
            TextureFormat::Bc2RgbaUnormSrgb => {
                for block in mips.iter_mut().flat_map(|m| m.as_chunks_mut::<16>().0) {
                    for b in &mut block[..8] {
                        *b ^= 0xFF;
                    }
                }
            }
            TextureFormat::Bc3RgbaUnormSrgb => {
                for block in mips.iter_mut().flat_map(|m| m.as_chunks_mut::<16>().0) {
                    invert_bc3_alpha(&mut block[..8]);
                }
            }
            // BC1's 1-bit alpha is tied to the colour endpoints; no foliage
            // skin ships as DXT1
            TextureFormat::Bc1RgbaUnormSrgb => {
                log::warn!("cannot invert the mask alpha of {format:?}, leaving it as-is")
            }
        },
    }
}

/// Inverts one BC3 alpha block to exactly 255 minus its old values. The
/// endpoints invert and swap, which keeps the block's mode; codes 0/1 swap,
/// the interpolated codes reverse (k -> 9-k in the a0 > a1 mode, k -> 7-k in
/// the other, whose literal 0/255 codes 6/7 swap).
fn invert_bc3_alpha(b: &mut [u8]) {
    let map: [u64; 8] = if b[0] > b[1] {
        [1, 0, 7, 6, 5, 4, 3, 2]
    } else {
        [1, 0, 5, 4, 3, 2, 7, 6]
    };
    let (a0, a1) = (b[0], b[1]);
    b[0] = 255 - a1;
    b[1] = 255 - a0;
    let bits = u64::from_le_bytes([b[2], b[3], b[4], b[5], b[6], b[7], 0, 0]);
    let mut out = 0u64;
    for t in 0..16 {
        out |= map[((bits >> (3 * t)) & 7) as usize] << (3 * t);
    }
    b[2..8].copy_from_slice(&out.to_le_bytes()[..6]);
}

/// Loads `skins/<filename>`; missing or undecodable warns and returns
/// retail's default image.
pub fn load_skin_image(fs: &Pk3Fs, filename: &str) -> Image {
    let path = format!("skins/{filename}");
    // some prop skins spell the extension in caps ("wood@pine1.TGA")
    let ext = filename
        .rfind('.')
        .map(|i| filename[i..].to_lowercase())
        .unwrap_or_default();
    if let Some(raw) = fs.read(&path) {
        match decode_image(&ext, &raw) {
            Ok(mut img) => {
                if is_inverted_foliage_mask(filename) {
                    invert_mask_alpha(&mut img.data);
                }
                return img;
            }
            Err(e) => log::warn!("bad {ext} for {path}: {e}"),
        }
    }
    log::warn!("no texture found for {path}, using the default image");
    default_image()
}

/// Loads by full pk3 path, extension included; no probing, no shader lookup.
/// Missing or undecodable warns and returns retail's default image.
pub fn load_path_image(fs: &Pk3Fs, path: &str) -> Image {
    let ext = path
        .rfind('.')
        .map(|i| path[i..].to_lowercase())
        .unwrap_or_default();
    if let Some(raw) = fs.read(path) {
        match decode_image(&ext, &raw) {
            Ok(img) => return img,
            Err(e) => log::warn!("bad {ext} for {path}: {e}"),
        }
    }
    log::warn!("no texture found for {path}, using the default image");
    default_image()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn err_of<T>(r: Result<T>) -> String {
        match r {
            Ok(_) => panic!("expected an error"),
            Err(e) => e.to_string(),
        }
    }

    // magic + 124-byte header; fourcc at offset 84
    fn dds_header(w: u32, h: u32, fourcc: &[u8; 4], mips: u32) -> Vec<u8> {
        let mut d = vec![0u8; 128];
        d[0..4].copy_from_slice(b"DDS ");
        d[4..8].copy_from_slice(&124u32.to_le_bytes()); // header size
        d[12..16].copy_from_slice(&h.to_le_bytes());
        d[16..20].copy_from_slice(&w.to_le_bytes());
        d[28..32].copy_from_slice(&mips.to_le_bytes());
        d[76..80].copy_from_slice(&32u32.to_le_bytes()); // pixelformat size
        d[80..84].copy_from_slice(&0x4u32.to_le_bytes()); // DDPF_FOURCC
        d[84..88].copy_from_slice(fourcc);
        d
    }

    #[test]
    fn block_sizes_match_dds_block_bytes() {
        assert_eq!(TextureFormat::Bc1RgbaUnormSrgb.block_size(), 8);
        assert_eq!(TextureFormat::Bc2RgbaUnormSrgb.block_size(), 16);
        assert_eq!(TextureFormat::Bc3RgbaUnormSrgb.block_size(), 16);
    }

    #[test]
    fn parses_dxt1_with_mips() {
        let mut d = dds_header(8, 8, b"DXT1", 2);
        d.extend_from_slice(&[0u8; 32]); // mip0: 2x2 blocks * 8 bytes
        d.extend_from_slice(&[0u8; 8]); // mip1: 4x4 -> 1 block
        let img = parse_dds(&d).unwrap();
        assert_eq!((img.width, img.height), (8, 8));
        match img.data {
            ImageData::Bc { format, mips } => {
                assert_eq!(format, TextureFormat::Bc1RgbaUnormSrgb);
                assert_eq!(mips.len(), 2);
                assert_eq!(mips[0].len(), 32);
                assert_eq!(mips[1].len(), 8);
            }
            _ => panic!("expected BC data"),
        }
    }

    #[test]
    fn parses_dxt5() {
        let mut d = dds_header(4, 4, b"DXT5", 1);
        d.extend_from_slice(&[0u8; 16]); // one 16-byte block
        let img = parse_dds(&d).unwrap();
        matches!(
            img.data,
            ImageData::Bc {
                format: TextureFormat::Bc3RgbaUnormSrgb,
                ..
            }
        )
        .then_some(())
        .expect("expected BC3");
    }

    #[test]
    fn rejects_truncated_and_bad_magic() {
        assert!(parse_dds(b"nope").is_err());
        let d = dds_header(8, 8, b"DXT1", 1); // header only, no block data
        assert!(parse_dds(&d).is_err());
    }

    // DDPF_RGB header with `bitcount` bits per pixel
    fn rgb_dds_header(w: u32, h: u32, bitcount: u32) -> Vec<u8> {
        let mut d = dds_header(w, h, b"\0\0\0\0", 1);
        d[80..84].copy_from_slice(&0x40u32.to_le_bytes());
        d[88..92].copy_from_slice(&bitcount.to_le_bytes());
        d
    }

    #[test]
    fn rejects_dds_dimensions_past_the_cap() {
        // 65536 * 65536 * 32 / 8 wraps u32 to 0
        let mut d = rgb_dds_header(65536, 65536, 32);
        d.extend_from_slice(&[0u8; 4]);
        let err = err_of(parse_dds(&d));
        assert!(err.contains("8192"), "got: {err}");
        let err = err_of(parse_dds(&dds_header(16384, 4, b"DXT1", 1)));
        assert!(err.contains("8192"), "got: {err}");
        assert!(parse_dds(&rgb_dds_header(8192, 1, 0)).is_err());
    }

    #[test]
    fn rejects_mip_count_past_the_chain() {
        // 8x8 has a 4-level chain; a fifth "mip" would be the 1x1 block again
        let mut d = dds_header(8, 8, b"DXT1", 5);
        d.extend_from_slice(&[0u8; 32 + 8 + 8 + 8 + 8]);
        let err = err_of(parse_dds(&d));
        assert!(err.contains("mip"), "got: {err}");
        d[28..32].copy_from_slice(&4u32.to_le_bytes());
        assert!(parse_dds(&d).is_ok());
    }

    #[test]
    fn uncompressed_dds_payload_must_cover_the_image() {
        let mut d = rgb_dds_header(2, 2, 24);
        d.extend_from_slice(&[0u8; 11]);
        assert!(parse_dds(&d).is_err());
        d.push(0);
        let img = parse_dds(&d).unwrap();
        match img.data {
            ImageData::Rgba8(px) => assert_eq!(px.len(), 16),
            _ => panic!("expected RGBA"),
        }
    }

    // uncompressed 8-bit grayscale TGA, w x h
    fn gray_tga(w: u16, h: u16) -> Vec<u8> {
        let mut d = vec![0u8; 18];
        d[2] = 3;
        d[12..14].copy_from_slice(&w.to_le_bytes());
        d[14..16].copy_from_slice(&h.to_le_bytes());
        d[16] = 8;
        d.extend(std::iter::repeat_n(0x80u8, w as usize * h as usize));
        d
    }

    #[test]
    fn image_decoders_refuse_dimensions_past_the_cap() {
        let img = decode_image(".tga", &gray_tga(8192, 1)).unwrap();
        assert_eq!((img.width, img.height), (8192, 1));
        assert!(decode_image(".tga", &gray_tga(8193, 1)).is_err());

        let mut jpg = std::io::Cursor::new(Vec::new());
        image::codecs::jpeg::JpegEncoder::new(&mut jpg)
            .encode(&vec![0x80u8; 8193], 8193, 1, image::ExtendedColorType::L8)
            .unwrap();
        assert!(decode_image(".jpg", jpg.get_ref()).is_err());
    }

    fn make_pk3(dir: &std::path::Path, file: &str, entries: &[(&str, &[u8])]) {
        use std::io::Write;
        let f = std::fs::File::create(dir.join(file)).unwrap();
        let mut z = zip::ZipWriter::new(f);
        let opts = zip::write::SimpleFileOptions::default();
        for (name, content) in entries {
            z.start_file(*name, opts).unwrap();
            z.write_all(content).unwrap();
        }
        z.finish().unwrap();
    }

    #[test]
    fn parses_shader_scripts() {
        let script = br#"
// terrain blends
textures/x/fancy
{
	surfaceparm grass
	qer_editorimage textures/x/editor.tga
	{
		map textures/x/real@img
		rgbGen exactVertex
	nextbundle
		map $lightmap
	}
}
textures/x/lightmap_first
{
	{
		map $lightmap
	}
	{
		clampMap textures/x/other.tga
	}
}
textures/x/modifiers
{
	{
		map clamp textures/x/clamped.tga
	}
}
textures/x/clampy_mod
{
	{
		map clampY textures/x/vert.tga
	}
}
textures/x/star_builtin
{
	{
		map *white
	}
	{
		map heightToNormal textures/x/bump
	}
}
textures/x/jpg_ref
{
	{
		map textures/x/window@pane.jpg
	}
}
textures/x/dds_ref
{
	{
		map textures/x/detail.DDS
	}
}
"#;
        let dir = tempfile::tempdir().unwrap();
        make_pk3(dir.path(), "pak0.pk3", &[("scripts/test.shader", script)]);
        let fs = crate::pk3::Pk3Fs::open(dir.path()).unwrap();
        let s = load_shaders(&fs);
        let img = |n: &str| s.image(n).unwrap();
        assert_eq!(img("textures/x/fancy"), "textures/x/real@img");
        // $lightmap first: the lookup falls through to the real image
        assert_eq!(img("textures/x/lightmap_first"), "textures/x/other");
        assert_eq!(img("textures/x/modifiers"), "textures/x/clamped");
        assert_eq!(img("textures/x/clampy_mod"), "textures/x/vert");
        assert_eq!(img("textures/x/star_builtin"), "textures/x/bump");
        assert_eq!(img("textures/x/jpg_ref"), "textures/x/window@pane");
        assert_eq!(img("textures/x/dds_ref"), "textures/x/detail");
    }

    #[test]
    fn polygon_offset_shaders_detected() {
        let script = b"textures/x/blendlayer\n{\n\tsurfaceparm grass\n\tpolygonOffset\n\t{\n\t\tmap textures/x/img.tga\n\t\tblendFunc blend\n\t}\n}\ntextures/x/plain\n{\n\t{\n\t\tmap textures/x/img2.tga\n\t}\n}\n";
        let dir = tempfile::tempdir().unwrap();
        make_pk3(dir.path(), "pak0.pk3", &[("scripts/test.shader", script)]);
        let fs = crate::pk3::Pk3Fs::open(dir.path()).unwrap();
        let shaders = load_shaders(&fs);
        assert!(shaders.uses_polygon_offset("textures/x/blendlayer"));
        assert!(!shaders.uses_polygon_offset("textures/x/plain"));
        assert_eq!(
            shaders.image("textures/x/blendlayer"),
            Some("textures/x/img")
        );
    }

    #[test]
    fn fxshaders_scripts_parse_with_blend_mode_and_slashed_paths() {
        let script = br#"gfx/effects/flash
{
	entityMergable
	sort	additive
	{
		map gfx/effects/flash
		blendFunc GL_ONE GL_ONE
		rgbGen vertex
	}
}
gfx/effects/smoke
{
	entityMergable
	{
		map /gfx/effects/smoke
		blendfunc blend
		rgbGen vertex
	}
}
gfx/effects/shorthand
{
	{
		blendFunc add
		map gfx/effects/spark
	}
}
gfx/effects/premult
{
	{
		map gfx/effects/glow
		blendFunc GL_SRC_ALPHA GL_ONE
	}
}
gfx/effects/second_stage_only
{
	{
		map gfx/effects/base
		blendfunc blend
	}
	{
		map gfx/effects/over
		blendFunc GL_ONE GL_ONE
	}
}
"#;
        let dir = tempfile::tempdir().unwrap();
        make_pk3(dir.path(), "pak0.pk3", &[("fxshaders/pj.shader", script)]);
        let fs = crate::pk3::Pk3Fs::open(dir.path()).unwrap();
        let s = load_shaders(&fs);

        assert_eq!(s.image("gfx/effects/flash"), Some("gfx/effects/flash"));
        assert_eq!(s.image("gfx/effects/smoke"), Some("gfx/effects/smoke"));

        assert!(s.is_additive("gfx/effects/flash"));
        assert!(!s.is_additive("gfx/effects/smoke"));
        // `add` shorthand, blendFunc written before its stage's `map`
        assert!(s.is_additive("gfx/effects/shorthand"));
        assert_eq!(s.image("gfx/effects/shorthand"), Some("gfx/effects/spark"));
        assert!(s.is_additive("gfx/effects/premult"));
        // only the image's stage decides; a later additive stage does not retag
        assert_eq!(
            s.image("gfx/effects/second_stage_only"),
            Some("gfx/effects/base")
        );
        assert!(!s.is_additive("gfx/effects/second_stage_only"));
    }

    #[test]
    fn real_fx_materials_carry_their_blend_modes() {
        let Some(fs) = crate::testing::game_fs() else {
            return;
        };
        let s = load_shaders(&fs);

        assert_eq!(
            s.image("gfx/effects/muzflash2"),
            Some("gfx/effects/muzflash2")
        );
        assert!(s.is_additive("gfx/effects/muzflash2"));
        assert!(s.is_additive("gfx/effects/muzflash2a"));

        // pj_fx.shader writes `map /gfx/effects/whitesmoke`
        assert_eq!(
            s.image("gfx/effects/whitesmoke"),
            Some("gfx/effects/whitesmoke")
        );
        assert!(fs.contains("gfx/effects/whitesmoke.tga"));
        assert!(!s.is_additive("gfx/effects/whitesmoke")); // blendfunc blend

        // image path differs from the material name
        assert_eq!(s.image("gfx/misc/tracer"), Some("textures/sfx/tracer"));
        assert!(s.is_additive("gfx/misc/tracer"));

        assert!(!s.is_additive("gfx/impact/bullethole1"));
        assert!(!s.is_additive("gfx/impact/dustlayer1"));
    }

    #[test]
    fn shader_material_resolves_to_image() {
        // 1x1 uncompressed 24-bit TGA
        let tga: &[u8] = &[
            0, 0, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 1, 0, 24, 0, 10, 20, 30,
        ];
        let script = b"textures/x/fancy\n{\n{\nmap textures/x/real@img\n}\n}\n";
        let dir = tempfile::tempdir().unwrap();
        make_pk3(
            dir.path(),
            "pak0.pk3",
            &[
                ("scripts/test.shader", script),
                ("textures/x/real@img.tga", tga),
            ],
        );
        let fs = crate::pk3::Pk3Fs::open(dir.path()).unwrap();
        let shaders = load_shaders(&fs);
        let img = load_material_image(&fs, &shaders, "textures/x/fancy");
        assert_eq!((img.width, img.height), (1, 1));
        assert!(matches!(img.data, ImageData::Rgba8(_)));
    }

    #[test]
    fn default_image_matches_retail() {
        let img = default_image();
        assert_eq!((img.width, img.height), (16, 16));
        let ImageData::Rgba8(px) = &img.data else {
            panic!("the default image is RGBA");
        };
        assert_eq!(px.len(), 16 * 16 * 4);
        let at = |x: usize, y: usize| &px[(y * 16 + x) * 4..][..4];
        assert_eq!(at(8, 8), [32, 32, 32, 32], "interior");
        for (x, y) in [(0, 0), (15, 0), (0, 15), (15, 15), (7, 0), (0, 7)] {
            assert_eq!(at(x, y), [0, 0, 0, 255], "border at {x},{y}");
        }
    }

    #[test]
    fn dlight_blob_is_bright_centre_and_transparent_edge() {
        let img = dlight_blob();
        assert_eq!((img.width, img.height), (64, 64));
        let ImageData::Rgba8(px) = &img.data else {
            panic!()
        };
        let at = |x: u32, y: u32| {
            let i = ((y * 64 + x) * 4) as usize;
            (px[i], px[i + 1], px[i + 2], px[i + 3])
        };
        // centre is near-opaque white
        assert!(at(32, 32).3 >= 240);
        assert_eq!((at(32, 32).0, at(32, 32).1, at(32, 32).2), (255, 255, 255));
        // corners fade to transparent
        assert_eq!(at(0, 0).3, 0);
        // monotone falloff: a midpoint ring is brighter than the outer corner
        assert!(at(48, 32).3 > at(60, 32).3);
        assert!(at(32, 32).3 >= at(48, 32).3);
    }

    /// Pins the exact set of materials absent from the retail pk3s.
    #[test]
    fn all_maps_materials_resolve() {
        let Some(fs) = crate::testing::game_fs() else {
            return;
        };
        let shaders = load_shaders(&fs);
        let exists = |n: &str| IMAGE_EXTS.iter().any(|e| fs.contains(&format!("{n}{e}")));
        let resolves = |name: &str| {
            let n = name.to_lowercase();
            exists(&n) || shaders.image(&n).is_some_and(exists)
        };

        // stock paks only; downloaded custom paks carry CoD2 material refs
        let is_stock_pak = |p: &std::path::Path| {
            let Some(stem) = p.file_stem().and_then(|s| s.to_str()) else {
                return false;
            };
            let Some(rest) = stem.strip_prefix("pak") else {
                return false;
            };
            !rest.is_empty() && rest.chars().all(|c| c.is_ascii_alphanumeric())
        };
        let maps: Vec<String> = fs
            .find_maps()
            .into_iter()
            .filter(|m| {
                let entry = fs.resolve_map(m).unwrap();
                fs.source_archive(&entry).is_some_and(is_stock_pak)
            })
            .collect();
        assert!(!maps.is_empty());
        let unresolved = std::sync::Mutex::new(std::collections::BTreeSet::new());
        let chunk = maps
            .len()
            .div_ceil(std::thread::available_parallelism().map_or(4, |n| n.get()));
        let (fs, resolves, unresolved) = (&fs, &resolves, &unresolved);
        std::thread::scope(|s| {
            for chunk in maps.chunks(chunk) {
                s.spawn(move || {
                    for map in chunk {
                        let entry = fs.resolve_map(map).unwrap();
                        let bsp = crate::bsp::parse(&fs.read(&entry).unwrap()).unwrap();
                        let used: std::collections::BTreeSet<u16> =
                            bsp.soups.iter().map(|s| s.material).collect();
                        let mut missing = Vec::new();
                        for mi in used {
                            let mat = &bsp.materials[mi as usize];
                            if crate::mesh::implicit_kind(&mat.name)
                                == crate::mesh::MaterialKind::Draw
                                && !resolves(&mat.name)
                            {
                                missing.push(mat.name.clone());
                            }
                        }
                        unresolved.lock().unwrap().extend(missing);
                    }
                });
            }
        });

        // absent from the retail data; the real game cannot resolve these either
        let known_absent: std::collections::BTreeSet<String> = [
            "textures/industrial/glass@factorywindow4a_broken",
            "textures/industrial/glass@factorywindow4o_broken",
            "textures/industrial/glass@factorywindow4z_broken",
            "textures/normandy/windows/glass@window2a",
        ]
        .map(String::from)
        .into();
        assert_eq!(*unresolved.lock().unwrap(), known_absent);
    }

    #[test]
    fn loads_real_pavlov_texture() {
        let Some(fs) = crate::testing::game_fs() else {
            return;
        };
        let all = load_shaders(&fs);
        // mp_neuville terrain blend layer
        assert!(all.uses_polygon_offset("textures/normandy/ground/dirt_oldpacked"));
        let img = load_material_image(&fs, &all, "textures/normandy/walls/brick@damagedwall1_p4");
        assert!(
            matches!(img.data, ImageData::Bc { .. }),
            "expected a DDS BC texture"
        );
        // mp_pavlov spells this material with '_' where the file has '@'
        let alias = load_material_image(&fs, &all, "textures/belgium/ground/snow_1024lightfill");
        assert!(
            matches!(alias.data, ImageData::Bc { .. }),
            "expected '@' alias to resolve"
        );
        // mp_dawnville material defined in scripts/terrain.shader, not a file
        let scripted = load_material_image(&fs, &all, "textures/normandy/ground/dirt_earthbase");
        assert!(
            matches!(scripted.data, ImageData::Bc { .. }),
            "expected shader-script material to resolve"
        );
        let fb = load_material_image(&fs, &all, "textures/does/not/exist");
        assert_eq!((fb.width, fb.height), (16, 16));
    }

    #[test]
    fn loads_impact_textures() {
        let Some(fs) = crate::testing::game_fs() else {
            return;
        };
        let fallback = default_image();
        for path in ["gfx/impact/bullethole1.tga", "gfx/impact/dusty_puff.tga"] {
            let img = load_path_image(&fs, path);
            let ImageData::Rgba8(px) = &img.data else {
                panic!("{path} should decode to RGBA");
            };
            assert!(img.width >= 64 && img.height >= 64, "{path} is tiny");
            assert!(
                !matches!(&fallback.data, ImageData::Rgba8(fb) if fb == px),
                "{path} fell back to the default image"
            );
            // both are cutouts
            assert!(
                px.iter().skip(3).step_by(4).any(|&a| a < 255),
                "{path} has no transparency"
            );
        }
        let fb = load_path_image(&fs, "gfx/impact/no_such_file.tga");
        assert_eq!((fb.width, fb.height), (16, 16));
    }

    #[test]
    fn loads_viewmodel_skins() {
        let Some(fs) = crate::testing::game_fs() else {
            return;
        };
        let dds = load_skin_image(&fs, "viewmodel@woodk98.dds");
        assert!(
            matches!(dds.data, ImageData::Bc { .. }),
            "expected DDS BC texture"
        );
        let jpg = load_skin_image(&fs, "viewhands@default.jpg");
        assert!(
            matches!(jpg.data, ImageData::Rgba8(_)),
            "expected decoded jpg"
        );
        let fb = load_skin_image(&fs, "no@such.dds");
        assert_eq!((fb.width, fb.height), (16, 16));
    }

    /// The format sniffer cannot detect TGA, so the decoder must be picked by
    /// the lowercased extension.
    #[test]
    fn skin_extension_matching_is_case_insensitive() {
        // 1x1 uncompressed 24-bit TGA
        let tga: &[u8] = &[
            0, 0, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 1, 0, 24, 0, 10, 20, 30,
        ];
        let dir = tempfile::tempdir().unwrap();
        make_pk3(dir.path(), "pak0.pk3", &[("skins/wood@pine1.TGA", tga)]);
        let fs = crate::pk3::Pk3Fs::open(dir.path()).unwrap();
        let img = load_skin_image(&fs, "wood@pine1.TGA");
        assert_eq!(
            (img.width, img.height),
            (1, 1),
            "expected the TGA to decode"
        );
    }

    #[test]
    fn only_foliage_masks_are_flipped() {
        assert!(is_inverted_foliage_mask(
            "foliage_masked@spikeybushsparse.tga"
        ));
        assert!(is_inverted_foliage_mask(
            "foliage_masked@bushwallfill#0.tga"
        ));
        assert!(is_inverted_foliage_mask("foliage_detail@grassblades#0.dds"));
        assert!(is_inverted_foliage_mask("foliage_masked@grassblades2a.dds"));
        assert!(is_inverted_foliage_mask("foliage_masked@vase.dds"));
        assert!(!is_inverted_foliage_mask(
            "foliage_masked@treeshdw_oakfrnt.tga"
        ));
        assert!(is_inverted_foliage_mask("foliage_masked@shdw_firfrt.tga"));
        assert!(!is_inverted_foliage_mask("metal_masked@boucher_sign.tga"));
        assert!(!is_inverted_foliage_mask("viewmodel@woodk98.dds"));
    }

    fn make_dds(fourcc: &[u8; 4], w: u32, h: u32, blocks: &[u8]) -> Vec<u8> {
        let mut d = vec![0u8; 128];
        d[0..4].copy_from_slice(b"DDS ");
        d[12..16].copy_from_slice(&h.to_le_bytes());
        d[16..20].copy_from_slice(&w.to_le_bytes());
        d[28..32].copy_from_slice(&1u32.to_le_bytes());
        d[80..84].copy_from_slice(&0x4u32.to_le_bytes());
        d[84..88].copy_from_slice(fourcc);
        d.extend_from_slice(blocks);
        d
    }

    /// Reference BC3 alpha decode; the test endpoints keep every value integral.
    fn bc3_alpha(b: &[u8]) -> [u8; 16] {
        let (a0, a1) = (b[0] as u32, b[1] as u32);
        let mut bits = 0u64;
        for (i, &byte) in b[2..8].iter().enumerate() {
            bits |= (byte as u64) << (8 * i);
        }
        std::array::from_fn(|t| {
            let k = (bits >> (3 * t)) & 7;
            (match (a0 > a1, k) {
                (_, 0) => a0,
                (_, 1) => a1,
                (true, k) => ((8 - k as u32) * a0 + (k as u32 - 1) * a1) / 7,
                (false, 6) => 0,
                (false, 7) => 255,
                (false, k) => ((6 - k as u32) * a0 + (k as u32 - 1) * a1) / 5,
            }) as u8
        })
    }

    #[test]
    fn dxt3_foliage_mask_flip_inverts_alpha_nibbles() {
        let block: &[u8] = &[
            0xF0, 0x0F, 0xFF, 0x00, 0x12, 0x34, 0x56, 0x78, // alpha nibbles
            0xAA, 0xBB, 0xCC, 0xDD, 0x11, 0x22, 0x33, 0x44, // colors
        ];
        let dds = make_dds(b"DXT3", 4, 4, block);
        let dir = tempfile::tempdir().unwrap();
        make_pk3(
            dir.path(),
            "pak0.pk3",
            &[("skins/foliage_masked@x.dds", &dds)],
        );
        let fs = crate::pk3::Pk3Fs::open(dir.path()).unwrap();
        let img = load_skin_image(&fs, "foliage_masked@x.dds");
        let ImageData::Bc { mips, .. } = &img.data else {
            panic!("expected compressed data to stay compressed");
        };
        assert_eq!(
            &mips[0][..8],
            &[0x0F, 0xF0, 0x00, 0xFF, 0xED, 0xCB, 0xA9, 0x87],
            "alpha nibbles must be inverted"
        );
        assert_eq!(&mips[0][8..], &block[8..], "colors must be untouched");
    }

    #[test]
    fn dxt5_foliage_mask_flip_inverts_decoded_alpha() {
        // one block per BC3 mode; endpoints congruent mod 7 and mod 5 keep
        // the interpolation integral; indices walk all 8 codes
        let mut idx = [0u8; 6];
        for t in 0..16u64 {
            let bits = (t % 8) << (3 * t);
            for (i, b) in idx.iter_mut().enumerate() {
                *b |= (bits >> (8 * i)) as u8;
            }
        }
        let mut blocks = Vec::new();
        for (a0, a1) in [(210u8, 70u8), (70, 210)] {
            blocks.extend_from_slice(&[a0, a1]);
            blocks.extend_from_slice(&idx);
            blocks.extend_from_slice(&[0u8; 8]); // color half, don't care
        }
        let dds = make_dds(b"DXT5", 8, 4, &blocks);
        let dir = tempfile::tempdir().unwrap();
        make_pk3(
            dir.path(),
            "pak0.pk3",
            &[("skins/foliage_detail@x.dds", &dds)],
        );
        let fs = crate::pk3::Pk3Fs::open(dir.path()).unwrap();
        let img = load_skin_image(&fs, "foliage_detail@x.dds");
        let ImageData::Bc { mips, .. } = &img.data else {
            panic!("expected compressed data to stay compressed");
        };
        for (before, after) in blocks
            .as_chunks::<16>()
            .0
            .iter()
            .zip(mips[0].as_chunks::<16>().0)
        {
            let (b, a) = (bc3_alpha(&before[..8]), bc3_alpha(&after[..8]));
            for t in 0..16 {
                assert_eq!(a[t], 255 - b[t], "texel {t}: {b:?} vs {a:?}");
            }
        }
    }

    #[test]
    fn real_foliage_dds_masks_come_out_mostly_transparent() {
        let Some(fs) = crate::testing::game_fs() else {
            return;
        };
        let img = load_skin_image(&fs, "foliage_detail@grassblades#0.dds");
        let ImageData::Bc { mips, .. } = &img.data else {
            panic!("expected grassblades to decode as compressed DDS");
        };
        // DXT3 alpha: first 8 bytes of each block, one nibble per texel
        let mut opaque = 0usize;
        let mut total = 0usize;
        for block in mips[0].as_chunks::<16>().0 {
            for byte in &block[..8] {
                opaque += usize::from(byte & 0xf >= 8) + usize::from(byte >> 4 >= 8);
                total += 2;
            }
        }
        let frac = opaque as f32 / total as f32;
        assert!(
            frac < 0.5,
            "expected the grassblades mask to be flipped, {frac} opaque"
        );
    }

    #[test]
    fn foliage_skin_masks_come_out_the_right_way_round() {
        let Some(fs) = crate::testing::game_fs() else {
            return;
        };
        let opaque_fraction = |name: &str| {
            let img = load_skin_image(&fs, name);
            let ImageData::Rgba8(px) = img.data else {
                panic!("{name} did not decode to Rgba8")
            };
            let n = px.len() / 4;
            px.iter().skip(3).step_by(4).filter(|&&a| a >= 128).count() as f32 / n as f32
        };
        // the file stores the sparse bush 85% "opaque"
        assert!(
            opaque_fraction("foliage_masked@spikeybushsparse.tga") < 0.3,
            "expected the sparse bush mask to be flipped"
        );
        assert!(
            opaque_fraction("metal_masked@boucher_sign.tga") > 0.9,
            "expected the sign mask to be left alone"
        );
    }
}
