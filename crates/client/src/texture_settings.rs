//! The Graphics page's texture settings as retail's renderer applies them:
//! `r_picmip`, `r_picmip2`, `r_texturebits`, `r_simpleMipMaps` and
//! `r_weightMipMaps` at image load (latched, so a change waits for
//! `vid_restart`) and `r_textureMode` on the samplers
//! (docs/research/cod11-front-end.md, section 14, "Texture settings").

use vcod_common::asset_source::AssetSource;
use vcod_common::assets::{self, Image, ImageData};

/// What one renderer build loads and samples images with.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TextureSettings {
    /// `r_picmip`: mip levels dropped from world images, 0..=3.
    pub picmip: u32,
    /// `r_picmip2`: the same for model images (Character Textures).
    pub picmip2: u32,
    /// `r_texturebits`: 16 stores uncompressed images at 16 bits a texel.
    pub bits: u32,
    /// `r_simpleMipMaps` and `r_weightMipMaps`.
    pub mip_filter: MipFilter,
    pub filter: TextureFilter,
    /// CoD4's `r_picmip_bump` and `r_picmip_spec`: levels dropped from a
    /// normal and a specular map ([`iwi_picmip`]); 0 off CoD4.
    pub picmip_bump: u32,
    pub picmip_spec: u32,
}

impl Default for TextureSettings {
    /// vcod's cvar defaults: every level kept, `r_texturebits` 0,
    /// `GL_LINEAR_MIPMAP_NEAREST`.
    fn default() -> Self {
        TextureSettings {
            picmip: 0,
            picmip2: 0,
            bits: 0,
            mip_filter: MipFilter::Box,
            filter: TextureFilter::by_name("GL_LINEAR_MIPMAP_NEAREST").unwrap(),
            picmip_bump: 0,
            picmip_spec: 0,
        }
    }
}

impl TextureSettings {
    /// The settings `r_picmip`, `r_picmip2`, `r_texturebits`,
    /// `r_simpleMipMaps`, `r_weightMipMaps` and `r_textureMode` read as; a
    /// filter name retail would refuse keeps `prev`'s filter, as
    /// `GL_TextureMode` leaves the images alone.
    pub fn from_cvars(
        [picmip, picmip2, bits, simple, weight]: [f32; 5],
        texture_mode: &str,
        prev: TextureFilter,
    ) -> TextureSettings {
        TextureSettings {
            picmip: picmip_level(picmip),
            picmip2: picmip_level(picmip2),
            bits: bits as i32 as u32,
            mip_filter: MipFilter::from_cvars(simple, weight),
            filter: TextureFilter::by_name(texture_mode).unwrap_or(prev),
            picmip_bump: 0,
            picmip_spec: 0,
        }
    }

    /// CoD4's three picmip dvars (`R_SetPicmip`, iw3mp.exe 0x617520):
    /// `r_picmip` for the world's colour maps, `r_picmip_bump` and
    /// `r_picmip_spec`, each clamped to 0..=3 (0x616d40). vcod's models
    /// keep every level (cod4-front-end.md 9.3).
    pub fn set_cod4_picmip(&mut self, [color, bump, spec]: [f32; 3]) {
        self.picmip = picmip_level(color);
        self.picmip_bump = picmip_level(bump);
        self.picmip_spec = picmip_level(spec);
    }

    /// How images store their levels under these settings.
    pub fn store(&self) -> Store {
        Store {
            bits: self.bits,
            mip_filter: self.mip_filter,
        }
    }

    /// Levels to drop from an image of a world shader (`nopicmip` keeps
    /// every level).
    pub fn world_picmip(&self, nopicmip: bool) -> u32 {
        if nopicmip { 0 } else { self.picmip }
    }

    /// Levels to drop from a model's image: a shader registered without a
    /// lightmap starts with the `picmip2` flag (0x4fc6f3).
    pub fn model_picmip(&self, nopicmip: bool) -> u32 {
        if nopicmip { 0 } else { self.picmip2 }
    }
}

/// The 2x2 step 0x4eb1d0 builds a smaller level with (the picmip halving
/// and the mip chain both), picked by the latched `r_simpleMipMaps` and
/// `r_weightMipMaps` (docs/research/cod11-front-end.md, section 14).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum MipFilter {
    /// The defaults: the truncated mean of each 2x2 block.
    #[default]
    Box,
    /// `r_weightMipMaps` in (0, 1]: `w` of the block's top-left texel plus
    /// `(1 - w) / 4` of the block's sum.
    Weighted(f32),
    /// `r_simpleMipMaps 0`: Q3's `R_MipMap2` 4x4 kernel, wrapping (0x4eac90).
    Kernel,
}

impl MipFilter {
    /// `r_simpleMipMaps` is read as an integer, `r_weightMipMaps` as a
    /// float (0x4eb32f, 0x4eb3a0).
    pub fn from_cvars(simple: f32, weight: f32) -> MipFilter {
        if simple as i32 == 0 {
            MipFilter::Kernel
        } else if weight > 0.0 && weight <= 1.0 {
            MipFilter::Weighted(weight)
        } else {
            MipFilter::Box
        }
    }
}

/// How [`prepare`] stores an image: `r_texturebits` and the mip filter.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Store {
    pub bits: u32,
    pub mip_filter: MipFilter,
}

/// `0x4ea380`: the cvar's integer clamped to 0..=3.
fn picmip_level(v: f32) -> u32 {
    (v as i32).clamp(0, 3) as u32
}

/// One row of `GL_TextureMode`'s table (CoDMP.exe 0x5712f8): the min and
/// mag filters of a mipmapped image. `mip` `None` is a min filter without
/// mipmaps, which samples level 0 only.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TextureFilter {
    pub min: wgpu::FilterMode,
    pub mag: wgpu::FilterMode,
    pub mip: Option<wgpu::MipmapFilterMode>,
    /// The anisotropy a trilinear sampler takes: vcod's 16 on CoD1 and
    /// CoD2, CoD4's `r_texFilterAnisoMin` ([`TextureFilter::cod4_aniso`]).
    pub aniso: u16,
}

impl TextureFilter {
    /// The six names `GL_TextureMode` (0x4ea530) accepts, compared without
    /// case; anything else prints `bad filter name`.
    pub fn by_name(name: &str) -> Option<TextureFilter> {
        use wgpu::FilterMode::{Linear, Nearest};
        use wgpu::MipmapFilterMode as M;
        let (min, mag, mip) = match name.to_ascii_uppercase().as_str() {
            "GL_NEAREST" => (Nearest, Nearest, None),
            "GL_LINEAR" => (Linear, Linear, None),
            "GL_NEAREST_MIPMAP_NEAREST" => (Nearest, Nearest, Some(M::Nearest)),
            "GL_LINEAR_MIPMAP_NEAREST" => (Linear, Linear, Some(M::Nearest)),
            "GL_NEAREST_MIPMAP_LINEAR" => (Nearest, Nearest, Some(M::Linear)),
            "GL_LINEAR_MIPMAP_LINEAR" => (Linear, Linear, Some(M::Linear)),
            _ => return None,
        };
        Some(TextureFilter {
            min,
            mag,
            mip,
            aniso: 16,
        })
    }

    /// `R_SetTexFilter` (iw3mp.exe 0x631740): `r_texFilterAnisoMax` capped
    /// at the card's limit (wgpu's 16) and at least 1, then
    /// `r_texFilterAnisoMin` capped at that is a linear mipped sampler's
    /// anisotropy.
    pub fn cod4_aniso(self, min: i32, max: i32) -> TextureFilter {
        let max = max.clamp(1, 16);
        TextureFilter {
            aniso: min.clamp(1, max) as u16,
            ..self
        }
    }

    /// A sampler descriptor for mipmapped images under this filter. The
    /// anisotropy ([`TextureFilter::aniso`]) rides only the trilinear mode,
    /// the one wgpu allows it with; retail 1.1 forces
    /// `r_ext_texture_filter_anisotropic` to 0.
    pub fn sampler<'a>(
        &self,
        label: &'a str,
        address: wgpu::AddressMode,
    ) -> wgpu::SamplerDescriptor<'a> {
        let trilinear = self.min == wgpu::FilterMode::Linear
            && self.mag == wgpu::FilterMode::Linear
            && self.mip == Some(wgpu::MipmapFilterMode::Linear);
        wgpu::SamplerDescriptor {
            label: Some(label),
            address_mode_u: address,
            address_mode_v: address,
            address_mode_w: address,
            mag_filter: self.mag,
            min_filter: self.min,
            mipmap_filter: self.mip.unwrap_or(wgpu::MipmapFilterMode::Nearest),
            lod_max_clamp: if self.mip.is_some() { 32.0 } else { 0.0 },
            anisotropy_clamp: if trilinear { self.aniso.max(1) } else { 1 },
            ..Default::default()
        }
    }
}

/// Loads a level by name; `None` when no such image exists.
pub type LoadLevel<'a> = &'a dyn Fn(&str) -> Option<Image>;

/// How an image's smaller levels are made: retail's image flag 1
/// (mipmapped, cleared by `nomipmaps`) and its `#N` custom levels.
#[derive(Clone, Copy, Default)]
pub struct Mips<'a> {
    /// Flag 1: build the chain down to 1x1 (0x4ebf20's mip loop).
    pub chain: bool,
    /// The image's name and a loader for a level by name. A name with
    /// `#<digits>` takes its next level from `#<digits + 1>` when that
    /// image exists at half the size (0x4eb1d0); picmip halving reads it
    /// too.
    pub custom: Option<(&'a str, LoadLevel<'a>)>,
}

impl<'a> Mips<'a> {
    /// A mipmapped image, levels box filtered or taken from `#N` files.
    pub fn chain(name: &'a str, load: LoadLevel<'a>) -> Self {
        Mips {
            chain: true,
            custom: Some((name, load)),
        }
    }

    /// `nomipmaps`: level 0 only; picmip still reads `#N` files.
    pub fn single(name: &'a str, load: LoadLevel<'a>) -> Self {
        Mips {
            chain: false,
            custom: Some((name, load)),
        }
    }
}

/// A `#N` level by pk3 path, probed over the image extensions as
/// `R_LoadImage` probes them (0x4edf20).
pub fn path_levels(fs: &dyn AssetSource) -> impl Fn(&str) -> Option<Image> + '_ {
    |name| assets::resolve_bundle_image(fs, name).map(|p| assets::load_path_image(fs, &p))
}

/// A `#N` level of a model skin, through `load_skin_image` so foliage masks
/// invert like their level 0.
pub fn skin_levels(fs: &dyn AssetSource) -> impl Fn(&str) -> Option<Image> + '_ {
    |name| {
        assets::resolve_bundle_image(fs, &format!("skins/{name}"))
            .map(|p| assets::load_skin_image(fs, &p["skins/".len()..]))
    }
}

/// `r_textureMode`'s samplers, repeating, clamped on both axes and clamped
/// on `s` or `t` alone, at bindings 9, 10, 13 and 14 of group 0 in every
/// pass that draws mipmapped images, and the fixed `GL_LINEAR` set at 11,
/// 12, 15 and 16 that images without mipmaps sample through
/// (`tex_samplers.wgsl`). Images bind without a sampler, so a filter change
/// rebuilds the samplers and the few group-0 bind groups only, as retail's
/// `GL_TextureMode` resets every image's filter in place.
pub struct TexSamplers {
    pub repeat: wgpu::Sampler,
    pub clamp: wgpu::Sampler,
    pub linear: wgpu::Sampler,
    pub linear_clamp: wgpu::Sampler,
    pub clamp_s: wgpu::Sampler,
    pub clamp_t: wgpu::Sampler,
    pub linear_clamp_s: wgpu::Sampler,
    pub linear_clamp_t: wgpu::Sampler,
}

impl TexSamplers {
    pub const REPEAT_BINDING: u32 = 9;
    pub const CLAMP_BINDING: u32 = 10;
    pub const LINEAR_BINDING: u32 = 11;
    pub const LINEAR_CLAMP_BINDING: u32 = 12;
    pub const CLAMP_S_BINDING: u32 = 13;
    pub const CLAMP_T_BINDING: u32 = 14;
    pub const LINEAR_CLAMP_S_BINDING: u32 = 15;
    pub const LINEAR_CLAMP_T_BINDING: u32 = 16;
    const BINDINGS: [u32; 8] = [
        Self::REPEAT_BINDING,
        Self::CLAMP_BINDING,
        Self::LINEAR_BINDING,
        Self::LINEAR_CLAMP_BINDING,
        Self::CLAMP_S_BINDING,
        Self::CLAMP_T_BINDING,
        Self::LINEAR_CLAMP_S_BINDING,
        Self::LINEAR_CLAMP_T_BINDING,
    ];

    pub fn new(device: &wgpu::Device, filter: TextureFilter) -> Self {
        use wgpu::AddressMode::{ClampToEdge, Repeat};
        let linear = TextureFilter::by_name("GL_LINEAR").unwrap();
        // `s` is u, `t` is v
        let one_axis = |f: TextureFilter, label, s_clamped: bool| {
            let mut d = f.sampler(label, Repeat);
            if s_clamped {
                d.address_mode_u = ClampToEdge;
            } else {
                d.address_mode_v = ClampToEdge;
            }
            device.create_sampler(&d)
        };
        TexSamplers {
            repeat: device.create_sampler(&filter.sampler("image sampler", Repeat)),
            clamp: device.create_sampler(&filter.sampler("image clamp sampler", ClampToEdge)),
            linear: device.create_sampler(&linear.sampler("unmipped image sampler", Repeat)),
            linear_clamp: device
                .create_sampler(&linear.sampler("unmipped image clamp sampler", ClampToEdge)),
            clamp_s: one_axis(filter, "image clamp s sampler", true),
            clamp_t: one_axis(filter, "image clamp t sampler", false),
            linear_clamp_s: one_axis(linear, "unmipped image clamp s sampler", true),
            linear_clamp_t: one_axis(linear, "unmipped image clamp t sampler", false),
        }
    }

    pub fn layout_entries() -> [wgpu::BindGroupLayoutEntry; 8] {
        Self::BINDINGS.map(|binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
            count: None,
        })
    }

    pub fn entries(&self) -> [wgpu::BindGroupEntry<'_>; 8] {
        let samplers = [
            &self.repeat,
            &self.clamp,
            &self.linear,
            &self.linear_clamp,
            &self.clamp_s,
            &self.clamp_t,
            &self.linear_clamp_s,
            &self.linear_clamp_t,
        ];
        std::array::from_fn(|i| wgpu::BindGroupEntry {
            binding: Self::BINDINGS[i],
            resource: wgpu::BindingResource::Sampler(samplers[i]),
        })
    }
}

/// The levels a CoD4 `.iwi` (version 6) drops at `picmip`
/// (`Image_LoadFromFileWithReader`, iw3mp.exe 0x64244b): none when its
/// header sets `NOPICMIP` or `NOMIPMAPS` (flags 0x3) or its shorter side
/// is under 32 texels. Anything else (a CoD2 `.iwi`, another format)
/// keeps `picmip`.
pub fn iwi_picmip(data: &[u8], picmip: u32) -> u32 {
    use vcod_common::iwi::{IwiHeader, flags};
    match IwiHeader::parse_version(data, 6) {
        _ if picmip == 0 => 0,
        Ok(h)
            if h.flags & (flags::NOPICMIP | flags::NOMIPMAPS) != 0
                || h.width.min(h.height) < 32 =>
        {
            0
        }
        _ => picmip,
    }
}

/// An image as retail uploads it (0x4ebf20): `picmip` levels dropped, the
/// texture gamma baked in (`gamma::bake_image`) unless the image goes up
/// unmipped at its own size, the mip chain built from that, then
/// `r_texturebits` on every level. Block-compressed images lose whole mip
/// levels while both sides stay above 4 and keep their own chain only with
/// `chain`; uncompressed ones are halved to `size >> picmip` and stored at
/// 16 bits a texel with `r_texturebits 16` (0x4ebe30). Returns an
/// uncompressed image's levels below level 0 (empty without `chain`).
pub fn prepare(
    img: &mut Image,
    picmip: u32,
    gamma: Option<&[u8; 256]>,
    store: Store,
    mips: Mips<'_>,
) -> Vec<Vec<u8>> {
    let bits = store.bits;
    let mut down = Down {
        name: mips.custom.map(|(n, _)| n.to_string()),
        load: mips.custom.map(|(_, l)| l),
        filter: store.mip_filter,
    };
    let size = (img.width, img.height);
    drop_levels(img, picmip, &mut down);
    // 0x4ec004..0x4ec03c: an unmipped image at its own size skips 0x4eaa90
    if mips.chain || size != (img.width, img.height) {
        crate::gamma::bake_image(img, gamma);
    }
    let px = match &mut img.data {
        ImageData::Rgba8(px) => px,
        ImageData::Bc { mips: levels, .. } => {
            // without flag 1 only level 0 goes up (0x4ebf20's mip loop)
            if !mips.chain {
                levels.truncate(1);
            }
            return Vec::new();
        }
    };
    let mut levels = Vec::new();
    if mips.chain {
        let (mut w, mut h) = (img.width, img.height);
        while w > 1 || h > 1 {
            let prev = levels.last().unwrap_or(&*px);
            let (nw, nh, next) = down.step(w, h, prev);
            levels.push(next);
            (w, h) = (nw, nh);
        }
    }
    if bits == 16 {
        // the driver converts each level as it is uploaded
        let opaque = px.iter().skip(3).step_by(4).all(|&a| a == 255);
        quantize_16(px, opaque);
        for l in &mut levels {
            quantize_16(l, opaque);
        }
    }
    levels
}

/// Retail's level-down step (0x4eb1d0): the next `#N` image when one
/// exists at the right size, else `filter`.
struct Down<'a> {
    name: Option<String>,
    load: Option<LoadLevel<'a>>,
    filter: MipFilter,
}

impl Down<'_> {
    fn step(&mut self, w: u32, h: u32, px: &[u8]) -> (u32, u32, Vec<u8>) {
        // The name steps on before the load, found or not: the next level
        // reads `#N+2` (0x4eb1d0 rewrites it in place).
        let next = self.name.as_deref().and_then(next_custom_level);
        if next.is_some() {
            self.name.clone_from(&next);
        }
        if let (Some(next), Some(load)) = (next, self.load)
            && let Some(img) = load(&next)
        {
            // retail's (w + 1) >> 1; the GPU's chain halves down, so an odd
            // side (no retail image has one) keeps the box
            let want = (w.div_ceil(2), h.div_ceil(2));
            let fits = want == ((w / 2).max(1), (h / 2).max(1));
            match img.data {
                ImageData::Rgba8(level) if fits && (img.width, img.height) == want => {
                    return (want.0, want.1, level);
                }
                // UO warns and filters; 1.1 and 1.5 refuse the whole image
                // (cod11-front-end.md 14, "Custom mip levels"). Stock UO
                // skins land here, so debug.
                _ => log::debug!(
                    "custom mipmap image '{next}' should be {} x {}, is {} x {}; filtering instead",
                    want.0,
                    want.1,
                    img.width,
                    img.height
                ),
            }
        }
        match self.filter {
            MipFilter::Box => halve(w, h, px),
            MipFilter::Weighted(wt) => halve_weighted(w, h, px, wt),
            MipFilter::Kernel => halve_kernel(w, h, px),
        }
    }
}

/// `a#3.tga` -> `a#4.tga`: the digits after the first `#` plus one, the
/// rest kept (0x4eb1d0, `"%i%s"` at 0x54bf78). `None` without a `#`.
fn next_custom_level(name: &str) -> Option<String> {
    let at = name.find('#')? + 1;
    let digits = name[at..].bytes().take_while(u8::is_ascii_digit).count();
    let n: i64 = name[at..at + digits].parse().unwrap_or(0);
    Some(format!("{}{}{}", &name[..at], n + 1, &name[at + digits..]))
}

fn drop_levels(img: &mut Image, picmip: u32, down: &mut Down) {
    match &mut img.data {
        ImageData::Bc { mips, .. } => {
            let mut drop = 0;
            let (mut w, mut h) = (img.width, img.height);
            while drop < picmip && w >= 5 && h >= 5 && mips.len() > drop as usize + 1 {
                w >>= 1;
                h >>= 1;
                drop += 1;
            }
            mips.drain(..drop as usize);
            img.width = w;
            img.height = h;
        }
        ImageData::Rgba8(px) => {
            for _ in 0..picmip {
                if img.width == 1 && img.height == 1 {
                    break;
                }
                let (w, h, half) = down.step(img.width, img.height, px);
                *px = half;
                img.width = w;
                img.height = h;
            }
        }
    }
}

/// One level down as 0x4eb1d0 builds it with `r_simpleMipMaps 1` and
/// `r_weightMipMaps 0` (their defaults): each texel the truncated mean of
/// a 2x2 block. An image one texel wide or high averages neighbouring
/// pairs instead.
fn halve(w: u32, h: u32, px: &[u8]) -> (u32, u32, Vec<u8>) {
    let (nw, nh) = ((w / 2).max(1), (h / 2).max(1));
    let mut out = vec![0u8; (nw * nh * 4) as usize];
    if w == 1 || h == 1 {
        let pairs = px.as_chunks::<8>().0;
        for (o, pair) in out.as_chunks_mut::<4>().0.iter_mut().zip(pairs) {
            for c in 0..4 {
                o[c] = ((u32::from(pair[c]) + u32::from(pair[4 + c])) >> 1) as u8;
            }
        }
        return (nw, nh, out);
    }
    let row = (w * 4) as usize;
    for (y, o_row) in out.chunks_exact_mut((nw * 4) as usize).enumerate() {
        let r0 = &px[2 * y * row..][..row];
        let r1 = &px[(2 * y + 1) * row..][..row];
        let pairs = r0.as_chunks::<8>().0.iter().zip(r1.as_chunks::<8>().0);
        for (o, (a, b)) in o_row.as_chunks_mut::<4>().0.iter_mut().zip(pairs) {
            for c in 0..4 {
                let sum =
                    u16::from(a[c]) + u16::from(a[4 + c]) + u16::from(b[c]) + u16::from(b[4 + c]);
                o[c] = (sum >> 2) as u8;
            }
        }
    }
    (nw, nh, out)
}

/// `r_weightMipMaps` in (0, 1] (0x4eb3d7..): each channel
/// `trunc(sum * k + top_left * w)` with `k = (1 - w) * 0.25` stored as a
/// float, the rest in x87 precision (exact in f64 for these magnitudes).
/// A side of one averages pairs as the box does.
fn halve_weighted(w: u32, h: u32, px: &[u8], wt: f32) -> (u32, u32, Vec<u8>) {
    if w == 1 || h == 1 {
        return halve(w, h, px);
    }
    let k = f64::from((1.0 - wt) * 0.25);
    let wt = f64::from(wt);
    let (nw, nh) = (w / 2, h / 2);
    let row = (w * 4) as usize;
    let mut out = vec![0u8; (nw * nh * 4) as usize];
    for (y, o_row) in out.chunks_exact_mut((nw * 4) as usize).enumerate() {
        let r0 = &px[2 * y * row..][..row];
        let r1 = &px[(2 * y + 1) * row..][..row];
        let pairs = r0.as_chunks::<8>().0.iter().zip(r1.as_chunks::<8>().0);
        for (o, (a, b)) in o_row.as_chunks_mut::<4>().0.iter_mut().zip(pairs) {
            for c in 0..4 {
                let sum = u32::from(a[c]) + u32::from(a[4 + c]) + u32::from(b[c]);
                let sum = sum + u32::from(b[4 + c]);
                o[c] = (f64::from(sum) * k + f64::from(a[c]) * wt) as u8;
            }
        }
    }
    (nw, nh, out)
}

/// `r_simpleMipMaps 0` (0x4eac90, Q3's `R_MipMap2`): each texel the 1 2 2 1
/// by 1 2 2 1 kernel over the 4x4 block around its 2x2, coordinates wrapped
/// by the side's mask, divided by 36 truncating. With a side of one it
/// writes nothing, so the level is the first texels of the one above.
fn halve_kernel(w: u32, h: u32, px: &[u8]) -> (u32, u32, Vec<u8>) {
    let (nw, nh) = ((w / 2).max(1), (h / 2).max(1));
    if w == 1 || h == 1 {
        return (nw, nh, px[..(nw * nh * 4) as usize].to_vec());
    }
    const WEIGHT: [u32; 4] = [1, 2, 2, 1];
    let (wm, hm) = (w as i64 - 1, h as i64 - 1);
    let at = |x: i64, y: i64| (((y & hm) * w as i64 + (x & wm)) * 4) as usize;
    let mut out = vec![0u8; (nw * nh * 4) as usize];
    for y in 0..nh as i64 {
        for x in 0..nw as i64 {
            let o = ((y * nw as i64 + x) * 4) as usize;
            for c in 0..4 {
                let mut total = 0u32;
                for (dy, wy) in WEIGHT.iter().enumerate() {
                    for (dx, wx) in WEIGHT.iter().enumerate() {
                        let t = at(2 * x - 1 + dx as i64, 2 * y - 1 + dy as i64);
                        total += wy * wx * u32::from(px[t + c]);
                    }
                }
                out[o + c] = (total / 36) as u8;
            }
        }
    }
    (nw, nh, out)
}

/// `r_texturebits 16`: an image with any alpha below 255 goes up as
/// `GL_RGBA4`, an opaque one as `GL_RGB5` (0x4ebe30), every level by level
/// 0's alpha. The driver's rounding is not in the binary; this rounds to
/// nearest and widens back.
fn quantize_16(px: &mut [u8], opaque: bool) {
    let q = |v: u8, bits: u32| -> u8 {
        let max = (1u32 << bits) - 1;
        let n = (u32::from(v) * max + 127) / 255;
        ((n * 255 + max / 2) / max) as u8
    };
    for t in px.as_chunks_mut::<4>().0 {
        if opaque {
            for c in &mut t[..3] {
                *c = q(*c, 5);
            }
        } else {
            for c in t.iter_mut() {
                *c = q(*c, 4);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vcod_common::assets::TextureFormat;

    fn bits(bits: u32) -> Store {
        Store {
            bits,
            ..Store::default()
        }
    }

    fn rgba(w: u32, h: u32, px: Vec<u8>) -> Image {
        Image {
            width: w,
            height: h,
            data: ImageData::Rgba8(px),
        }
    }

    #[test]
    fn picmip_cvars_clamp_to_three_levels() {
        let f = TextureFilter::by_name("GL_LINEAR").unwrap();
        let s = TextureSettings::from_cvars([-1.0, 7.0, 16.0, 1.0, 0.0], "gl_linear", f);
        assert_eq!((s.picmip, s.picmip2, s.bits), (0, 3, 16));
        assert_eq!(s.world_picmip(false), 0);
        assert_eq!(s.model_picmip(false), 3);
        assert_eq!(s.model_picmip(true), 0);
    }

    #[test]
    fn texture_modes_map_to_retail_filters() {
        use wgpu::FilterMode::{Linear, Nearest};
        use wgpu::MipmapFilterMode as M;
        let bilinear = TextureFilter::by_name("GL_LINEAR_MIPMAP_NEAREST").unwrap();
        assert_eq!(
            (bilinear.min, bilinear.mag, bilinear.mip),
            (Linear, Linear, Some(M::Nearest))
        );
        let tri = TextureFilter::by_name("gl_linear_mipmap_linear").unwrap();
        assert_eq!(tri.mip, Some(M::Linear));
        let near = TextureFilter::by_name("GL_NEAREST").unwrap();
        assert_eq!((near.min, near.mip), (Nearest, None));
        assert_eq!(
            near.sampler("t", wgpu::AddressMode::Repeat).lod_max_clamp,
            0.0
        );
        assert_eq!(
            tri.sampler("t", wgpu::AddressMode::Repeat).anisotropy_clamp,
            16
        );
        assert_eq!(
            bilinear
                .sampler("t", wgpu::AddressMode::Repeat)
                .anisotropy_clamp,
            1
        );
        // a refused name keeps the filter already set
        let s = TextureSettings::from_cvars([1.0, 2.0, 0.0, 1.0, 0.0], "GL_CUBIC", tri);
        assert_eq!(s.filter, tri);
    }

    #[test]
    fn picmip_halves_uncompressed_images() {
        let mut img = rgba(4, 2, (0..32).map(|i| (i * 8) as u8).collect());
        prepare(&mut img, 1, None, Store::default(), Mips::default());
        assert_eq!((img.width, img.height), (2, 1));
        let ImageData::Rgba8(px) = &img.data else {
            unreachable!()
        };
        // red of texels 0, 1, 4 and 5 (0, 32, 128, 160) averaged
        assert_eq!(px[0], 80);
        prepare(&mut img, 3, None, Store::default(), Mips::default());
        assert_eq!((img.width, img.height), (1, 1));
    }

    #[test]
    fn picmip_drops_compressed_levels_while_sides_exceed_four() {
        let mips = (0..4).map(|l| vec![l as u8; 8]).collect();
        let mut img = Image {
            width: 16,
            height: 8,
            data: ImageData::Bc {
                format: TextureFormat::Bc1RgbaUnorm,
                mips,
            },
        };
        let chain = Mips {
            chain: true,
            custom: None,
        };
        prepare(&mut img, 3, None, bits(16), chain);
        // 16x8 -> 8x4, then 4 is not above 4
        assert_eq!((img.width, img.height), (8, 4));
        let ImageData::Bc { mips, .. } = &img.data else {
            unreachable!()
        };
        assert_eq!(mips.len(), 3);
        assert_eq!(mips[0][0], 1);
    }

    #[test]
    fn unmipped_compressed_images_keep_level_zero_only() {
        let bc = || Image {
            width: 8,
            height: 8,
            data: ImageData::Bc {
                format: TextureFormat::Bc1RgbaUnorm,
                mips: (0..4).map(|l| vec![l as u8; 8]).collect(),
            },
        };
        let levels = |img: &Image| match &img.data {
            ImageData::Bc { mips, .. } => mips.len(),
            ImageData::Rgba8(_) => unreachable!(),
        };
        let mut img = bc();
        prepare(&mut img, 0, None, Store::default(), Mips::default());
        assert_eq!(levels(&img), 1);
        let mut img = bc();
        let chain = Mips {
            chain: true,
            custom: None,
        };
        prepare(&mut img, 0, None, Store::default(), chain);
        assert_eq!(levels(&img), 4);
    }

    #[test]
    fn texture_gamma_skips_unmipped_images_at_their_own_size() {
        let mut table = [0u8; 256];
        table[100] = 150;
        let gamma = Some(&table);
        let red = |img: &Image| match &img.data {
            ImageData::Rgba8(px) => px[0],
            ImageData::Bc { .. } => unreachable!(),
        };
        let chain = Mips {
            chain: true,
            custom: None,
        };
        let mut img = rgba(1, 1, vec![100, 100, 100, 255]);
        prepare(&mut img, 0, gamma, Store::default(), Mips::default());
        assert_eq!(red(&img), 100, "native size, no mipmaps: straight upload");
        let mut img = rgba(1, 1, vec![100, 100, 100, 255]);
        prepare(&mut img, 0, gamma, Store::default(), chain);
        assert_eq!(red(&img), 150, "mipmapped");
        let mut img = rgba(2, 1, [100, 100, 100, 255].repeat(2));
        prepare(&mut img, 1, gamma, Store::default(), Mips::default());
        assert_eq!(red(&img), 150, "picmip resampled");
    }

    #[test]
    fn sixteen_bits_quantize_by_alpha() {
        let mut opaque = rgba(1, 1, vec![100, 200, 7, 255]);
        prepare(&mut opaque, 0, None, bits(16), Mips::default());
        let ImageData::Rgba8(px) = &opaque.data else {
            unreachable!()
        };
        // 5 bits: 100 -> 12 -> 99, 200 -> 24 -> 197, 7 -> 1 -> 8
        assert_eq!(px.as_slice(), &[99, 197, 8, 255]);
        let mut alpha = rgba(1, 1, vec![100, 200, 7, 128]);
        prepare(&mut alpha, 0, None, bits(16), Mips::default());
        let ImageData::Rgba8(px) = &alpha.data else {
            unreachable!()
        };
        // 4 bits: 100 -> 6 -> 102, 200 -> 12 -> 204, 7 -> 0, 128 -> 8 -> 136
        assert_eq!(px.as_slice(), &[102, 204, 0, 136]);
        let mut kept = rgba(1, 1, vec![100, 200, 7, 128]);
        prepare(&mut kept, 0, None, bits(32), Mips::default());
        let ImageData::Rgba8(px) = &kept.data else {
            unreachable!()
        };
        assert_eq!(px.as_slice(), &[100, 200, 7, 128]);
    }

    fn px(img: &Image) -> &[u8] {
        let ImageData::Rgba8(px) = &img.data else {
            unreachable!()
        };
        px
    }

    fn no_files(_: &str) -> Option<Image> {
        None
    }

    #[test]
    fn mip_chain_box_filters_down_to_one_texel() {
        // 4x2, red = texel index * 10, alpha 255
        let data = (0..8u8).flat_map(|i| [i * 10, 0, 0, 255]).collect();
        let mut img = rgba(4, 2, data);
        let levels = prepare(
            &mut img,
            0,
            None,
            Store::default(),
            Mips::chain("a", &no_files),
        );
        assert_eq!(levels.len(), 2);
        // 2x1: (0 + 10 + 40 + 50) / 4 = 25, (20 + 30 + 60 + 70) / 4 = 45
        assert_eq!(levels[0], vec![25, 0, 0, 255, 45, 0, 0, 255]);
        // 1x1 from a one-high level: the pair's mean, truncated
        assert_eq!(levels[1], vec![35, 0, 0, 255]);
        assert_eq!(px(&img).len(), 32);
    }

    #[test]
    fn box_filter_truncates() {
        let mut img = rgba(2, 2, vec![1, 2, 3, 4, 0, 0, 0, 0, 0, 0, 0, 0, 2, 2, 2, 2]);
        let levels = prepare(
            &mut img,
            0,
            None,
            Store::default(),
            Mips::chain("a", &no_files),
        );
        // 3/4, 4/4, 5/4, 6/4
        assert_eq!(levels, vec![vec![0, 1, 1, 1]]);
    }

    #[test]
    fn nomipmaps_uploads_level_zero_only() {
        let mut img = rgba(2, 2, vec![9; 16]);
        assert!(
            prepare(
                &mut img,
                0,
                None,
                Store::default(),
                Mips::single("a", &no_files)
            )
            .is_empty()
        );
        assert!(prepare(&mut img, 0, None, Store::default(), Mips::default()).is_empty());
    }

    #[test]
    fn custom_levels_come_from_the_next_numbered_image() {
        assert_eq!(
            next_custom_level("t/a@b#0.tga").as_deref(),
            Some("t/a@b#1.tga")
        );
        assert_eq!(next_custom_level("a#9").as_deref(), Some("a#10"));
        assert_eq!(next_custom_level("plain.tga"), None);
        // a#1 exists at half size, a#2 is missing
        let load = |name: &str| match name {
            "a#1.tga" => Some(rgba(2, 2, vec![200; 16])),
            _ => None,
        };
        let mut img = rgba(4, 4, vec![0; 64]);
        let levels = prepare(
            &mut img,
            0,
            None,
            Store::default(),
            Mips::chain("a#0.tga", &load),
        );
        assert_eq!(levels.len(), 2);
        assert_eq!(levels[0], vec![200; 16]);
        // the 1x1 level is filtered from the custom one
        assert_eq!(levels[1], vec![200; 4]);
        // a level of the wrong size is box filtered instead
        let wrong = |_: &str| Some(rgba(4, 4, vec![7; 64]));
        let mut img = rgba(4, 4, vec![0; 64]);
        let levels = prepare(
            &mut img,
            0,
            None,
            Store::default(),
            Mips::chain("a#0.tga", &wrong),
        );
        assert_eq!(levels[0], vec![0; 16]);
    }

    #[test]
    fn a_refused_or_missing_custom_level_still_steps_the_name() {
        // UO's `deadtreehoriz#01` (512) over 1.1's `#2` (128): level 1
        // refuses `#2`, level 2 reads `#3`, never the fitting `#2`
        let load = |name: &str| match name {
            "a#2" => Some(rgba(1, 1, vec![9; 4])),
            _ => None,
        };
        let mut img = rgba(4, 4, vec![0; 64]);
        let levels = prepare(
            &mut img,
            0,
            None,
            Store::default(),
            Mips::chain("a#01", &load),
        );
        assert_eq!(levels, vec![vec![0; 16], vec![0; 4]]);
        // a missing `#1` still makes the next level `#2`
        let load = |name: &str| match name {
            "a#2" => Some(rgba(1, 1, vec![9; 4])),
            _ => None,
        };
        let mut img = rgba(4, 4, vec![0; 64]);
        let levels = prepare(
            &mut img,
            0,
            None,
            Store::default(),
            Mips::chain("a#0", &load),
        );
        assert_eq!(levels, vec![vec![0; 16], vec![9; 4]]);
    }

    #[test]
    fn uo_foliage_over_the_1_1_levels_filters_its_whole_chain() {
        let Some(fs) = vcod_common::testing::uo_fs() else {
            return;
        };
        let skin = "foliage_masked@deadtreehoriz#01.tga";
        let mut img = vcod_common::assets::load_skin_image(&fs, skin);
        assert_eq!((img.width, img.height), (512, 512));
        let levels = prepare(
            &mut img,
            0,
            None,
            Store::default(),
            Mips::chain(skin, &skin_levels(&fs)),
        );
        // no level is 1.1's `#2`, which would sit at index 1 (128 x 128)
        let ImageData::Rgba8(base) = &img.data else {
            unreachable!()
        };
        let (_, _, l1) = halve(512, 512, base);
        let (_, _, l2) = halve(256, 256, &l1);
        assert_eq!(levels[0], l1);
        assert_eq!(levels[1], l2);
    }

    #[test]
    fn picmip_reads_custom_levels_and_the_chain_continues_after_them() {
        let load = |name: &str| match name {
            "a#1" => Some(rgba(2, 2, vec![100; 16])),
            "a#2" => Some(rgba(1, 1, vec![50; 4])),
            _ => None,
        };
        let mut img = rgba(4, 4, vec![0; 64]);
        let levels = prepare(
            &mut img,
            1,
            None,
            Store::default(),
            Mips::chain("a#0", &load),
        );
        assert_eq!((img.width, img.height), (2, 2));
        assert_eq!(px(&img), &[100; 16]);
        assert_eq!(levels, vec![vec![50; 4]]);
        // without a chain, picmip still takes the custom level
        let mut img = rgba(4, 4, vec![0; 64]);
        assert!(
            prepare(
                &mut img,
                1,
                None,
                Store::default(),
                Mips::single("a#0", &load)
            )
            .is_empty()
        );
        assert_eq!(px(&img), &[100; 16]);
    }

    #[test]
    fn sixteen_bits_quantize_every_level_by_level_zero_alpha() {
        let mut img = rgba(2, 1, vec![100, 100, 100, 255, 100, 100, 100, 255]);
        let levels = prepare(&mut img, 0, None, bits(16), Mips::chain("a", &no_files));
        // opaque: 5 bits, 100 -> 99 on both levels
        assert_eq!(px(&img)[0], 99);
        assert_eq!(levels[0], vec![99, 99, 99, 255]);
    }

    #[test]
    fn mip_filter_cvars_pick_retail_branches() {
        assert_eq!(MipFilter::from_cvars(1.0, 0.0), MipFilter::Box);
        assert_eq!(MipFilter::from_cvars(0.0, 0.5), MipFilter::Kernel);
        assert_eq!(MipFilter::from_cvars(1.0, 0.5), MipFilter::Weighted(0.5));
        assert_eq!(MipFilter::from_cvars(1.0, 1.0), MipFilter::Weighted(1.0));
        assert_eq!(MipFilter::from_cvars(1.0, 1.5), MipFilter::Box);
        assert_eq!(MipFilter::from_cvars(1.0, -1.0), MipFilter::Box);
    }

    #[test]
    fn weighted_filter_adds_the_top_left_texel() {
        let px: Vec<u8> = [10, 20, 30, 40]
            .iter()
            .flat_map(|&v| [v, 0, 0, 255])
            .collect();
        let level = |w: f32| {
            let mut img = rgba(2, 2, px.clone());
            let store = Store {
                mip_filter: MipFilter::Weighted(w),
                ..Store::default()
            };
            prepare(&mut img, 0, None, store, Mips::chain("a", &|_| None)).remove(0)
        };
        // 100 * 0.125 + 10 * 0.5, truncated
        assert_eq!(level(0.5), [17, 0, 0, 255]);
        assert_eq!(level(1.0), [10, 0, 0, 255]);
    }

    #[test]
    fn kernel_filter_wraps_and_divides_by_36() {
        let mut px = vec![0u8; 4 * 4 * 4];
        px[0] = 180;
        let (w, h, out) = halve_kernel(4, 4, &px);
        assert_eq!((w, h), (2, 2));
        // the texel's weight at each output texel: 4, then 2 and 2 wrapped
        // in from the far side, then 1
        let red: Vec<u8> = out.iter().step_by(4).copied().collect();
        assert_eq!(red, [20, 10, 10, 5]);
        // a side of one writes nothing: the leading texels of the level above
        let line: Vec<u8> = (0..16).collect();
        assert_eq!(halve_kernel(4, 1, &line), (2, 1, line[..8].to_vec()));
    }

    /// 1.7's picmip refusal (iw3mp.exe 0x64244b): flags 0x3 or a side
    /// under 32; a CoD2 header keeps the level.
    #[test]
    fn cod4_iwi_header_refuses_picmip() {
        let head = |version: u8, flags: u8, w: u16, h: u16| {
            let mut b = b"IWi".to_vec();
            b.extend_from_slice(&[version, 11, flags]);
            b.extend_from_slice(&w.to_le_bytes());
            b.extend_from_slice(&h.to_le_bytes());
            b.resize(vcod_common::iwi::HEADER_LEN, 0);
            b
        };
        assert_eq!(iwi_picmip(&head(6, 0, 512, 256), 2), 2);
        assert_eq!(iwi_picmip(&head(6, 0, 512, 256), 0), 0);
        assert_eq!(iwi_picmip(&head(6, 1, 512, 256), 2), 0);
        assert_eq!(iwi_picmip(&head(6, 2, 512, 256), 2), 0);
        assert_eq!(iwi_picmip(&head(6, 0, 512, 16), 2), 0);
        assert_eq!(iwi_picmip(&head(5, 1, 512, 256), 2), 2);
    }

    /// `R_SetTexFilter`'s linear mipped anisotropy (iw3mp.exe 0x631740).
    #[test]
    fn cod4_anisotropy_is_the_minimum_under_the_maximum() {
        let tri = TextureFilter::by_name("GL_LINEAR_MIPMAP_LINEAR").unwrap();
        assert_eq!(tri.aniso, 16);
        assert_eq!(tri.cod4_aniso(1, 16).aniso, 1);
        assert_eq!(tri.cod4_aniso(8, 16).aniso, 8);
        assert_eq!(tri.cod4_aniso(8, 4).aniso, 4);
        assert_eq!(tri.cod4_aniso(8, 0).aniso, 1);
        assert_eq!(tri.cod4_aniso(32, 64).aniso, 16);
        let bilinear = TextureFilter::by_name("GL_LINEAR_MIPMAP_NEAREST").unwrap();
        let d = bilinear
            .cod4_aniso(8, 16)
            .sampler("t", wgpu::AddressMode::Repeat);
        assert_eq!(d.anisotropy_clamp, 1, "wgpu takes it trilinear only");
    }
}
