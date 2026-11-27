//! Texture uploads: images and their mip levels, bundle images resolved per
//! load, and the test-only upload digest.

use super::*;

pub(super) fn wgpu_format(f: assets::TextureFormat) -> wgpu::TextureFormat {
    match f {
        assets::TextureFormat::Bc1RgbaUnorm => wgpu::TextureFormat::Bc1RgbaUnorm,
        assets::TextureFormat::Bc2RgbaUnorm => wgpu::TextureFormat::Bc2RgbaUnorm,
        assets::TextureFormat::Bc3RgbaUnorm => wgpu::TextureFormat::Bc3RgbaUnorm,
    }
}

/// An image after `texture_settings::prepare`, with its smaller levels.
pub(super) type PreparedImage = (Image, Vec<Vec<u8>>);

/// A model skin's image decoded and mipped under the renderer's texture
/// settings, as a model upload binds it.
pub(super) fn prepare_skin(
    fs: &Pk3Fs,
    lib: &MaterialLib,
    images: &assets::DecodedImages,
    textures: &TextureSettings,
    gamma: Option<&[u8; 256]>,
    skin: &str,
) -> PreparedImage {
    let mut img = model_skin_image(fs, lib, images, skin);
    let levels = texture_settings::prepare(
        &mut img,
        textures.picmip2,
        gamma,
        textures.store(),
        texture_settings::Mips::chain(skin, &texture_settings::skin_levels(fs)),
    );
    (img, levels)
}

/// BC mip chain as-is, or a single RGBA level.
pub(crate) fn upload_image(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    label: &str,
    img: &Image,
) -> wgpu::TextureView {
    upload_levels(device, queue, label, img, &[])
}

/// `upload_image` with an RGBA image's smaller levels
/// (`texture_settings::prepare`'s chain) under level 0.
pub(crate) fn upload_levels(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    label: &str,
    img: &Image,
    levels: &[Vec<u8>],
) -> wgpu::TextureView {
    #[cfg(any(test, feature = "testing"))]
    upload_digest::feed_image(label, img, levels);
    match &img.data {
        ImageData::Rgba8(px) if !levels.is_empty() => {
            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d {
                    width: img.width,
                    height: img.height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1 + levels.len() as u32,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            for (level, data) in std::iter::once(px).chain(levels).enumerate() {
                let w = (img.width >> level).max(1);
                let h = (img.height >> level).max(1);
                queue.write_texture(
                    wgpu::TexelCopyTextureInfo {
                        texture: &texture,
                        mip_level: level as u32,
                        origin: wgpu::Origin3d::ZERO,
                        aspect: wgpu::TextureAspect::All,
                    },
                    data,
                    wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(w * 4),
                        rows_per_image: Some(h),
                    },
                    wgpu::Extent3d {
                        width: w,
                        height: h,
                        depth_or_array_layers: 1,
                    },
                );
            }
            texture.create_view(&Default::default())
        }
        ImageData::Rgba8(px) => upload_rgba(device, queue, label, img.width, img.height, px),
        ImageData::Bc { format, mips } => {
            let block_size = format.block_size();
            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d {
                    width: img.width,
                    height: img.height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: mips.len() as u32,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu_format(*format),
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            for (level, data) in mips.iter().enumerate() {
                let w = (img.width >> level).max(1);
                let h = (img.height >> level).max(1);
                // Copy extents are whole blocks; the 2x2 and 1x1 tail mips
                // still occupy one 4x4 block each.
                let (bw, bh) = (w.div_ceil(4), h.div_ceil(4));
                queue.write_texture(
                    wgpu::TexelCopyTextureInfo {
                        texture: &texture,
                        mip_level: level as u32,
                        origin: wgpu::Origin3d::ZERO,
                        aspect: wgpu::TextureAspect::All,
                    },
                    data,
                    wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(bw * block_size),
                        rows_per_image: Some(bh),
                    },
                    wgpu::Extent3d {
                        width: bw * 4,
                        height: bh * 4,
                        depth_or_array_layers: 1,
                    },
                );
            }
            texture.create_view(&Default::default())
        }
    }
}

pub(super) fn upload_rgba(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    label: &str,
    width: u32,
    height: u32,
    px: &[u8],
) -> wgpu::TextureView {
    upload_texel4(
        device,
        queue,
        label,
        width,
        height,
        wgpu::TextureFormat::Rgba8Unorm,
        px,
    )
}

/// One mip of four-byte texels in `format`.
pub(crate) fn upload_texel4(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    label: &str,
    width: u32,
    height: u32,
    format: wgpu::TextureFormat,
    px: &[u8],
) -> wgpu::TextureView {
    #[cfg(any(test, feature = "testing"))]
    upload_digest::feed(label, &[px]);
    let size = wgpu::Extent3d {
        width,
        height,
        depth_or_array_layers: 1,
    };
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        px,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(width * 4),
            rows_per_image: Some(height),
        },
        size,
    );
    texture.create_view(&Default::default())
}

/// Resolves a bundle image path through the asset probe chain; a missing
/// file warns and lands in the default image. Views are deduped per load:
/// `rule` applies to an image's first upload only.
pub(super) fn upload_bundle_view(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    fs: &Pk3Fs,
    cache: &mut BundleViews,
    path: &str,
    rule: ImageRule,
) -> wgpu::TextureView {
    if let Some(v) = cache.views.get(path) {
        return v.clone();
    }
    let mut picmip = rule.picmip;
    let mut img = if path == "$dlight" {
        // engine-generated light blob, never a file on disk (research doc §8)
        assets::dlight_blob()
    } else if path == vcod_common::shader::DEFAULT_IMAGE {
        assets::default_image()
    } else if path == "textures/battleship/deckflag_np.tga" {
        // ships in no stock pak; retail binds its default image there too
        assets::white_1x1()
    } else {
        match assets::resolve_bundle_image(fs, path) {
            Some(full) => {
                // a CoD4 image's header can refuse the picmip
                if picmip > 0 && full.to_ascii_lowercase().ends_with(".iwi") {
                    let head = fs.read_unflagged(&full).unwrap_or_default();
                    picmip = texture_settings::iwi_picmip(&head, picmip);
                }
                match cache.prefetched {
                    Some(p) => p.load_path_image(fs, &full),
                    None => assets::load_path_image(fs, &full),
                }
            }
            None => {
                log::warn!("shader image {path} not found, using the default image");
                assets::default_image()
            }
        }
    };
    // Retail generates the blob unmipped at its native size, which skips
    // the texture gamma (docs/research/cod11-gamma.md, section 3).
    let load = texture_settings::path_levels(fs);
    let levels = if path == "$dlight" {
        Vec::new()
    } else {
        let mips = if rule.mipmap {
            texture_settings::Mips::chain(path, &load)
        } else {
            texture_settings::Mips::single(path, &load)
        };
        texture_settings::prepare(&mut img, picmip, cache.table, cache.store, mips)
    };
    let v = upload_levels(device, queue, path, &img, &levels);
    cache.views.insert(path.to_string(), v.clone());
    v
}

/// How an image uploads the first time it loads: the levels it drops and
/// whether it gets a mip chain. A cached image keeps its first rule, as
/// retail's image cache does (0x4ee7ff warns on the mix).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct ImageRule {
    pub(super) picmip: u32,
    pub(super) mipmap: bool,
}

impl ImageRule {
    /// Every level, mipmapped: what a block without `nopicmip` or
    /// `nomipmaps` loads at `r_picmip 0`; the lit tests' rule.
    #[cfg(test)]
    pub(super) const FULL: ImageRule = ImageRule {
        picmip: 0,
        mipmap: true,
    };

    /// A world shader's images: `r_picmip` unless `nopicmip`, a mip chain
    /// unless `nomipmaps`.
    pub(super) fn world(t: &TextureSettings, sh: &Material) -> ImageRule {
        ImageRule {
            picmip: t.world_picmip(sh.nopicmip),
            mipmap: !sh.nomipmaps,
        }
    }

    /// A model shader's images: `r_picmip2` in `r_picmip`'s place.
    pub(super) fn model(t: &TextureSettings, sh: &Material) -> ImageRule {
        ImageRule {
            picmip: t.model_picmip(sh.nopicmip),
            mipmap: !sh.nomipmaps,
        }
    }

    /// [`Self::world`] for `sh`, or a world image no block flags
    /// (`r_picmip`, mipmapped) when there is none.
    pub(super) fn world_or_bare(t: &TextureSettings, sh: Option<&Material>) -> ImageRule {
        ImageRule {
            picmip: t.world_picmip(sh.is_some_and(|sh| sh.nopicmip)),
            mipmap: !sh.is_some_and(|sh| sh.nomipmaps),
        }
    }
}

/// Shader-stage images uploaded during one world load, and the texture
/// gamma they load through (`r_ignorehwgamma 1`, `gamma::bake_image`).
pub(super) struct BundleViews<'a> {
    pub(super) views: HashMap<String, wgpu::TextureView>,
    pub(super) table: Option<&'a [u8; 256]>,
    /// `r_texturebits` and the mip filter.
    pub(super) store: texture_settings::Store,
    /// What the world load decoded ahead ([`assets::DecodedImages`]).
    pub(super) prefetched: Option<&'a assets::DecodedImages>,
}

pub(super) fn fallback_pixels() -> Vec<u8> {
    match assets::default_image().data {
        ImageData::Rgba8(px) => px,
        _ => Vec::new(),
    }
}

/// True for `load_material_image`'s default-image placeholder.
pub(super) fn is_fallback(img: &Image, fallback_px: &[u8]) -> bool {
    img.width == 16
        && img.height == 16
        && matches!(&img.data, ImageData::Rgba8(px) if px == fallback_px)
}

/// A running hash of every texture the load uploads, label and bytes in
/// upload order, for the load profile to show a faster load uploads the
/// same textures. Off unless a test or profile turns it on.
#[cfg(any(test, feature = "testing"))]
pub(crate) mod upload_digest {
    use std::hash::Hasher;
    use std::sync::Mutex;

    static DIGEST: Mutex<Option<(std::collections::hash_map::DefaultHasher, usize)>> =
        Mutex::new(None);

    /// Starts a digest; [`take`] ends it.
    #[cfg(not(test))]
    pub fn start() {
        *DIGEST.lock().unwrap() = Some(Default::default());
    }

    /// `(hash, textures)` since [`start`].
    #[cfg(not(test))]
    pub fn take() -> Option<(u64, usize)> {
        DIGEST.lock().unwrap().take().map(|(h, n)| (h.finish(), n))
    }

    pub fn feed(label: &str, data: &[&[u8]]) {
        if let Some((h, n)) = DIGEST.lock().unwrap().as_mut() {
            h.write(label.as_bytes());
            for d in data {
                h.write(d);
            }
            *n += 1;
        }
    }

    pub fn feed_image(label: &str, img: &vcod_common::assets::Image, levels: &[Vec<u8>]) {
        use vcod_common::assets::ImageData;
        let mut data: Vec<&[u8]> = match &img.data {
            ImageData::Rgba8(px) => vec![px],
            ImageData::Bc { mips, .. } => mips.iter().map(Vec::as_slice).collect(),
        };
        data.extend(levels.iter().map(Vec::as_slice));
        feed(label, &data);
    }
}
