//! World-space effect quads: their vertex layout and blend kinds, the stage
//! draws an effect material expands to, the fx pass and its textures, and the
//! fx lights uniform.

use super::*;

/// `fx::sim::MAX_PARTICLES` and room for the head icons.
pub(super) const MAX_FX_QUADS: usize = 2048 + 256;
/// Quads the fx buffers hold once each is drawn once per stage: the stock
/// effect materials have at most two base stages.
pub(super) const MAX_FX_DRAWN: usize = MAX_FX_QUADS * 2;

/// Per-vertex rgba lets particles, decals and tracers share one pipeline.
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
pub(super) struct FxVert {
    pub(super) pos: [f32; 3],
    pub(super) uv: [f32; 2],
    pub(super) rgba: [f32; 4],
    /// The quad's smaller half-extent; scales the `vc_softParticles` fade.
    pub(super) size: f32,
    /// `FX_VERT_*` bits, the stage's (`fx_stage_draw`).
    pub(super) flags: u32,
    /// A `zfeather*` material's `featherParms.x` on a stage whose blend
    /// reads source alpha; 0 draws no feather (`fx_feather`).
    pub(super) feather: f32,
}

const _: () = assert!(std::mem::size_of::<FxVert>() == 48);

/// `fx.wgsl`: the material is `nofog`.
pub(super) const FX_VERT_NOFOG: u32 = 1;
/// `fx.wgsl`: an additive draw scales by the texture's alpha
/// (`GL_SRC_ALPHA GL_ONE`).
pub(super) const FX_VERT_TEX_ALPHA: u32 = 2;
/// `fx.wgsl`: the stage's `alphaFunc`, tested on texture alpha times the
/// gen's: `GT0`, `LT128`, `GE128`.
pub(super) const FX_VERT_ALPHA_GT0: u32 = 4;
pub(super) const FX_VERT_ALPHA_LT128: u32 = 8;
pub(super) const FX_VERT_ALPHA_GE128: u32 = 16;
/// `fx.wgsl`: the feather reads the view depth itself, CoD4's
/// `zfeather.hlsl`, not CoD2's clip z (`Material::feather_view_depth`).
pub(super) const FX_VERT_FEATHER_W: u32 = 32;
/// `fx.wgsl`: a CoD4 distortion quad, whose `feather` and `size` carry
/// the material's `distortionScale` (`fs_distort`).
pub(super) const FX_VERT_DISTORT: u32 = 64;
/// `tex_samplers.wgsl`'s `F_CLAMP_S`, `F_CLAMP_T`: bundle 0 clamps that axis.
pub(super) const FX_VERT_CLAMP_S: u32 = STAGE_FLAG_CLAMP_S;
pub(super) const FX_VERT_CLAMP_T: u32 = STAGE_FLAG_CLAMP_T;

/// The fx pass's blend states, retail's `glBlendFunc` for each stage
/// (GL_State 0x4d57b0 maps the script's pair straight through;
/// cod11-shader-scripts.md section 3).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum FxBlend {
    /// `GL_SRC_ALPHA GL_ONE_MINUS_SRC_ALPHA`, and any pair not below.
    Alpha,
    /// Onto `GL_ONE`, drawn premultiplied as `GL_ONE GL_ONE`.
    Add,
    /// `GL_ONE GL_ONE_MINUS_SRC_COLOR` (UO's `flamethrowerFire*`).
    Screen,
    /// No blend (or `GL_ONE GL_ZERO`): writes depth, as the gib and debris
    /// materials' `alphaFunc GE128` stages do.
    Opaque,
}

/// One draw of an fx material: a stage's image (the anim frame at the
/// frame's shader time), blend, `FX_VERT_*` bits and texture matrix. The
/// fx pass draws a run's quads once per stage, stage by stage, as retail's
/// stage loop does (0x500b30).
pub(super) struct FxStageDraw<'a> {
    pub(super) image: &'a str,
    pub(super) blend: FxBlend,
    pub(super) flags: u32,
    /// bundle 0's tcMods folded (`bundle_affine`); identity without a script.
    pub(super) uv: [f32; 6],
    /// `None` for a material without a script (`rgbGen vertex`).
    pub(super) stage: Option<&'a Stage>,
    /// `depthFunc always`: no depth test (`headiconYouInKillCam`).
    pub(super) depth_always: bool,
}

/// The stages the fx pass draws for a material, in order. Retail's base loop
/// (0x500b30) draws every stage not flagged `perLight` (vcod parks those in
/// `Material::dlight`), each through 0x5008a0 with its own colours
/// (0x4ffa60), texture coordinates (0x500400), image (0x4fe0e0) and state
/// (0x4d57b0). Bundle 1 is not drawn: a `$lightmap` there is the white
/// image under a registered material's lightmap index -4 (0x4f3699). A stage
/// whose bundle 0 is not an image file draws nothing here. A stage wraps
/// its image unless the bundle says `clamp` (image flags 0x10, 0x20;
/// 0x4ec6e9). A material without a script draws its name as the image,
/// straight alpha and clamped, as the pass always drew it.
pub(super) fn fx_stage_draws<'a>(
    shader: Option<&'a Material>,
    name: &'a str,
    t: impl Into<ShaderClock>,
) -> Vec<FxStageDraw<'a>> {
    let t = t.into();
    let Some(sh) = shader else {
        return vec![FxStageDraw {
            image: name,
            blend: FxBlend::Alpha,
            flags: FX_VERT_CLAMP_S | FX_VERT_CLAMP_T,
            uv: [1.0, 0.0, 0.0, 1.0, 0.0, 0.0],
            stage: None,
            depth_always: false,
        }];
    };
    sh.stages
        .iter()
        .filter_map(|st| {
            let b0 = st.bundles.first()?;
            let image = match (&b0.anim, &b0.image) {
                (Some(anim), _) if !anim.paths.is_empty() => {
                    anim.paths[anim_frame_index(anim.fps, anim.paths.len(), t)].as_str()
                }
                (_, ImageRef::Path(p)) => p.as_str(),
                _ => return None,
            };
            let (blend, mut flags) = fx_stage_draw(sh, st);
            if b0.clamp.s {
                flags |= FX_VERT_CLAMP_S;
            }
            if b0.clamp.t {
                flags |= FX_VERT_CLAMP_T;
            }
            Some(FxStageDraw {
                image,
                blend,
                flags,
                uv: bundle_affine(&b0.tcmods, t),
                stage: Some(st),
                depth_always: st.depth_func == DepthFunc::Always,
            })
        })
        .collect()
}

/// An fx quad's material: the map's script, or retail's `<default>` for
/// an element with no `shaders` (`fx::sim`'s quads name it).
pub(super) fn fx_material<'a>(lib: &'a MaterialLib, name: &str) -> Option<&'a Material> {
    static DEFAULT: std::sync::LazyLock<Material> =
        std::sync::LazyLock::new(vcod_common::shader::default_material);
    match name {
        vcod_common::shader::DEFAULT_SHADER => Some(&DEFAULT),
        _ => lib.get(name),
    }
}

/// Every image [`fx_stage_draws`] can name for a material at any time
/// (each anim frame), and whether a script stage names it.
pub(super) fn fx_stage_images<'a>(
    shader: Option<&'a Material>,
    name: &'a str,
) -> Vec<(&'a str, bool)> {
    let Some(sh) = shader else {
        return vec![(name, false)];
    };
    let mut out = Vec::new();
    for st in &sh.stages {
        let Some(b0) = st.bundles.first() else {
            continue;
        };
        match (&b0.anim, &b0.image) {
            (Some(anim), _) if !anim.paths.is_empty() => {
                out.extend(anim.paths.iter().map(|p| (p.as_str(), true)));
            }
            (_, ImageRef::Path(p)) => out.push((p.as_str(), true)),
            _ => {}
        }
    }
    out
}

/// The feather scale a stage's quads carry: the material's
/// `featherParms.x` when the stage's blend reads the source alpha the
/// feather fades (z_feather.hlsl scales only `color.a`;
/// cod2-materials.md 9.5), else 0. `r_zFeather` 0 (`zfeather` false)
/// draws a CoD4 set as its plain remap, with no feather at all
/// (cod4-effects.md 14).
pub(super) fn fx_feather(sh: Option<&Material>, draw: &FxStageDraw, zfeather: bool) -> f32 {
    let Some(fp) = sh
        .filter(|sh| zfeather || !sh.feather_view_depth)
        .and_then(|sh| sh.feather)
    else {
        return 0.0;
    };
    let reads_alpha = match draw.blend {
        FxBlend::Alpha => true,
        FxBlend::Add => draw.flags & FX_VERT_TEX_ALPHA != 0,
        FxBlend::Screen | FxBlend::Opaque => false,
    };
    if reads_alpha { fp.max(0.0) } else { 0.0 }
}

/// How the fx pass draws one stage: its blend and its `FX_VERT_*` bits.
/// Retail fogs a stage unless the shader is `nofog` (0x4fbc71), towards
/// black when it blends onto `GL_ONE` (0x4d2ad0); the colour that reaches
/// the blend is texture times the gen's colour under `GL_MODULATE`, so a
/// `GL_SRC_ALPHA` source reads the texture's alpha.
pub(super) fn fx_stage_draw(sh: &Material, st: &Stage) -> (FxBlend, u32) {
    use BlendFactor as F;
    let mut flags = if sh.nofog { FX_VERT_NOFOG } else { 0 };
    flags |= match st.alpha_func {
        None => 0,
        Some(AlphaFunc::Gt0) => FX_VERT_ALPHA_GT0,
        Some(AlphaFunc::Lt128) => FX_VERT_ALPHA_LT128,
        Some(AlphaFunc::Ge128) => FX_VERT_ALPHA_GE128,
    };
    let blend = match st.blend.as_ref() {
        None | Some((F::One, F::Zero)) => FxBlend::Opaque,
        Some((F::SrcAlpha, F::One)) => {
            flags |= FX_VERT_TEX_ALPHA;
            FxBlend::Add
        }
        Some((_, F::One)) => FxBlend::Add,
        Some((F::One, F::OneMinusSrcColor)) => FxBlend::Screen,
        _ => FxBlend::Alpha,
    };
    (blend, flags)
}

/// A stage's vertex colour for a sprite whose own colour is `v` (the efx
/// `rgb` and `alpha` curves, CoDMP.exe 0x48e640, 0x48e7d0), as 0x4ffa60
/// computes it: `identity` is white and `identityLighting` identityLight
/// flat, both opaque, whatever the sprite's colour; `vertex` takes the
/// sprite's colour times identityLight, `exactVertex` as it is; `const` and
/// `wave` ignore it. The parser gives `rgbGen vertex` and `exactVertex`
/// `alphaGen vertex` (0x4f8d4f), so their stages fade with the sprite, and
/// alphaGen identity is opaque elsewhere (0x4ffa60's alpha case 0, which
/// leaves `rgbGen vertex`'s copied alpha at identityLight 1). A
/// stage blending `GL_ONE GL_ONE` reads no source alpha; `fs_additive`
/// scales by it, so it gets 1.
pub(super) fn fx_stage_rgba(
    stage: Option<&Stage>,
    v: [f32; 4],
    identity_light: f32,
    t: impl Into<ShaderClock>,
) -> [f32; 4] {
    let t = t.into();
    let [r, g, b, a] = v;
    let il = identity_light;
    let Some(st) = stage else {
        return [r * il, g * il, b * il, a];
    };
    let rgb = match &st.rgb_gen {
        RgbGen::Identity => [1.0; 3],
        RgbGen::IdentityLighting => [il; 3],
        RgbGen::Vertex | RgbGen::LightingDiffuse => [r * il, g * il, b * il],
        RgbGen::ExactVertex | RgbGen::LightingPrecalc => [r, g, b],
        RgbGen::Const(c) => *c,
        RgbGen::ConstLighting(c) => c.map(|x| x * il),
        // a table wave clamps after its identityLight scale; the noise
        // glow skips the scale (0x514c00)
        RgbGen::Wave(w) if w.form == WaveForm::Noise => [rgb_wave_value(w, t).clamp(0.0, 1.0); 3],
        RgbGen::Wave(w) => [(rgb_wave_value(w, t) * il).clamp(0.0, 1.0); 3],
    };
    let alpha = match &st.alpha_gen {
        AlphaGen::Vertex | AlphaGen::Entity => a,
        // case 0 keeps what `rgbGen vertex` copied only at identityLight 1
        AlphaGen::Identity if st.rgb_gen == RgbGen::Vertex && il == 1.0 => a,
        AlphaGen::Identity => 1.0,
        AlphaGen::Const(c) => c.clamp(0.0, 1.0),
        AlphaGen::Wave(w) => wave_value(w, t).clamp(0.0, 1.0),
    };
    let alpha = if st.blend == Some((BlendFactor::One, BlendFactor::One)) {
        1.0
    } else {
        alpha
    };
    [rgb[0], rgb[1], rgb[2], alpha]
}

/// The wgpu blend of each [`FxBlend`] pipeline. `fs_additive` and
/// `fs_screen` write alpha 0, so their alpha component leaves the target's
/// alpha alone.
pub(super) fn fx_blend_state(blend: FxBlend) -> Option<wgpu::BlendState> {
    let component = |src_factor, dst_factor| wgpu::BlendComponent {
        src_factor,
        dst_factor,
        operation: wgpu::BlendOperation::Add,
    };
    let add = component(wgpu::BlendFactor::One, wgpu::BlendFactor::One);
    match blend {
        FxBlend::Alpha => Some(wgpu::BlendState::ALPHA_BLENDING),
        FxBlend::Add => Some(wgpu::BlendState {
            color: add,
            alpha: add,
        }),
        FxBlend::Screen => Some(wgpu::BlendState {
            color: component(wgpu::BlendFactor::One, wgpu::BlendFactor::OneMinusSrc),
            alpha: add,
        }),
        FxBlend::Opaque => None,
    }
}

/// Matches `FxLights` in shader.wgsl (std140, vec4 aligned); `vs_prop` reads it.
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
pub(super) struct FxLightsUniform {
    /// xyz position, w radius; zero radius means unused.
    pub(super) pos_radius: [[f32; 4]; MAX_LIGHTS],
    /// rgb, w unused.
    pub(super) color: [[f32; 4]; MAX_LIGHTS],
}

impl FxLightsUniform {
    pub(super) fn from_lights(lights: &[FxLight]) -> Self {
        let mut pos_radius = [[0.0f32; 4]; MAX_LIGHTS];
        let mut color = [[0.0f32; 4]; MAX_LIGHTS];
        for (i, l) in lights.iter().take(MAX_LIGHTS).enumerate() {
            pos_radius[i] = [l.pos[0], l.pos[1], l.pos[2], l.radius];
            color[i] = [l.rgb[0], l.rgb[1], l.rgb[2], 0.0];
        }
        Self { pos_radius, color }
    }
}

/// World-space fx quads. One pipeline per [`FxBlend`], and the same four
/// with the `vc_softParticles` fade. One draw call per stage of each
/// contiguous same-shader run.
pub(super) struct FxPass {
    /// Indexed by `FxBlend as usize`.
    pub(super) pipelines: [wgpu::RenderPipeline; 4],
    /// The `vc_softParticles` four; group 2 is
    /// `SoftParticles::fx_bind_group`.
    pub(super) soft_pipelines: [wgpu::RenderPipeline; 4],
    /// The first four without the depth test, for a `depthFunc always`
    /// stage.
    pub(super) always_pipelines: [wgpu::RenderPipeline; 4],
    /// `fs_distort`, straight alpha; group 2 is `SceneCopy::bind_group`.
    pub(super) distort_pipeline: wgpu::RenderPipeline,
    pub(super) vertex_buf: wgpu::Buffer,
    pub(super) index_buf: wgpu::Buffer, // static quad pattern, MAX_FX_DRAWN * 6, u32
    /// image name -> bind group; None = failed to resolve, warned already.
    pub(super) textures: HashMap<String, Option<wgpu::BindGroup>>,
    /// Draws in order, by sort first.
    pub(super) runs: Vec<FxRun>,
    /// CoD2's particle clouds, whose runs ride `runs`.
    pub(super) clouds: fx_cloud::CloudPass,
}

/// One stage of a same-shader run of fx quads.
pub(super) struct FxRun {
    /// Key into `FxPass::textures`.
    pub(super) image: String,
    pub(super) blend: FxBlend,
    pub(super) quads: usize,
    /// The material's sort.
    pub(super) sort: f32,
    /// Its quads carry a feather, so it reads the scene depth copy.
    pub(super) feather: bool,
    /// CoD4's feather (`feather_view_depth`): 1.7's `floatZ` holds the
    /// models and the view weapon, so its copy waits for the split
    /// (cod4-client-perf.md 20); CoD2's goes before the first model.
    pub(super) view_depth: bool,
    /// A CoD4 distortion run (`FX_VERT_DISTORT`): drawn through
    /// `FxPass::distort_pipeline` over the scene copy, after the split.
    pub(super) distort: bool,
    /// A run of CoD2 clouds (`fx_cloud`): its instances, and no quads.
    pub(super) cloud: Option<std::ops::Range<u32>>,
    /// Drawn through `FxPass::always_pipelines`.
    pub(super) depth_always: bool,
}

impl Renderer {
    /// An fx material's image as the fx pass binds it: a scripted stage
    /// names its image file, a material without a script resolves through
    /// the image map.
    pub(super) fn fx_texture(
        &self,
        fs: &dyn AssetSource,
        sh: Option<&Material>,
        image: &str,
        scripted: bool,
    ) -> Option<wgpu::BindGroup> {
        let no_map = HashMap::new();
        let images = if scripted {
            &no_map
        } else {
            self.shaders.image_map()
        };
        resolve_fx_texture(
            &self.device,
            &self.queue,
            &self.vm_pass.skin_layout,
            fs,
            images,
            image,
            FxImage {
                gamma: self.image_gamma.as_ref(),
                picmip: self.textures.world_picmip(sh.is_some_and(|sh| sh.nopicmip)),
                store: self.textures.store(),
                mipmap: !sh.is_some_and(|sh| sh.nomipmaps),
            },
        )
    }

    /// Loads every image the fx materials in `shaders` draw now, so an
    /// effect's first play binds them without reading and uploading them
    /// in its frame (cod4-map-sweep.md 5).
    pub fn prewarm_fx<'a>(
        &mut self,
        fs: &dyn AssetSource,
        shaders: impl IntoIterator<Item = &'a str>,
    ) {
        for name in shaders {
            let sh = fx_material(&self.shader_lib, name);
            for (image, scripted) in fx_stage_images(sh, name) {
                if !self.fx.textures.contains_key(image) {
                    let bg = self.fx_texture(fs, sh, image, scripted);
                    self.fx.textures.insert(image.to_string(), bg);
                }
            }
        }
    }

    /// Images the fx pass holds, loaded or not found.
    #[cfg(test)]
    pub(crate) fn fx_images(&self) -> usize {
        self.fx.textures.len()
    }

    /// Quads in draw order (already back-to-front); `time` is the client's
    /// clock in seconds ([`ShaderClock`]) (tcMod, animMap, wave gens). Unseen images resolve
    /// lazily; an unresolvable one warns once and is cached as `None`. Each
    /// run draws once per stage (`fx_stage_draws`).
    pub fn set_fx_quads(&mut self, fs: &dyn AssetSource, quads: Vec<FxQuad>, time: f64) {
        let time = ShaderClock(time);
        // build_quads is capped by construction; the vertex buffer is sized
        // exactly, so truncate anyway.
        let quads = if quads.len() > MAX_FX_QUADS {
            if !std::mem::replace(&mut self.fx_cap_warned, true) {
                log::warn!(
                    "fx: {} quads exceeds the {MAX_FX_QUADS} cap, truncating",
                    quads.len()
                );
            }
            &quads[..MAX_FX_QUADS]
        } else {
            &quads[..]
        };

        // Retail's draw list keys an effect's polygons by material sort
        // first (shader doc 5); within a sort they keep the sim's
        // back-to-front order. An fx material without a script is
        // implicit, straight alpha, and sorts as a blend.
        // One sort lookup per run of a shader, keyed before the (stable)
        // sort, not two hash lookups per comparison.
        let mut sorts: HashMap<&str, f32> = HashMap::new();
        let mut last: Option<(&str, f32)> = None;
        let mut order: Vec<(f32, &FxQuad)> = Vec::with_capacity(quads.len());
        for quad in quads {
            let sort = match last {
                Some((name, sort)) if name == &*quad.shader => sort,
                _ => {
                    let sort = *sorts.entry(&*quad.shader).or_insert_with(|| {
                        fx_material(&self.shader_lib, &quad.shader).map_or(SORT_BLEND, |sh| sh.sort)
                    });
                    last = Some((&*quad.shader, sort));
                    sort
                }
            };
            order.push((sort, quad));
        }
        order.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut verts: Vec<FxVert> = Vec::with_capacity(quads.len() * 4);
        let mut runs: Vec<FxRun> = Vec::new();
        let mut rest = &order[..];
        'runs: while let Some(&(sort, first)) = rest.first() {
            let name = &*first.shader;
            let len = rest.iter().take_while(|(_, q)| &*q.shader == name).count();
            let (group, tail) = rest.split_at(len);
            rest = tail;
            let sh = fx_material(&self.shader_lib, name);
            for draw in fx_stage_draws(sh, name, time) {
                if verts.len() / 4 + group.len() > MAX_FX_DRAWN {
                    if !std::mem::replace(&mut self.fx_cap_warned, true) {
                        log::warn!(
                            "fx: stage draws exceed the {MAX_FX_DRAWN} quad cap, truncating"
                        );
                    }
                    break 'runs;
                }
                if !self.fx.textures.contains_key(draw.image) {
                    let bg = self.fx_texture(fs, sh, draw.image, draw.stage.is_some());
                    self.fx.textures.insert(draw.image.to_string(), bg);
                }
                let [a, b, c, d, tx, ty] = draw.uv;
                let distortion = sh.and_then(|sh| sh.distortion);
                let feather = match distortion {
                    Some(_) => 0.0,
                    None => fx_feather(sh, &draw, self.cod4_options.zfeather),
                };
                let mut flags = if sh.is_some_and(|sh| sh.feather_view_depth) {
                    draw.flags | FX_VERT_FEATHER_W
                } else {
                    draw.flags
                };
                if distortion.is_some() {
                    flags |= FX_VERT_DISTORT;
                }
                for (_, quad) in group {
                    let rgba = fx_stage_rgba(draw.stage, quad.rgba, self.identity_light, time);
                    let size = quad_half_extent(&quad.verts);
                    // a distortion quad's two scales ride feather and size
                    let (size, feather) = distortion.map_or((size, feather), |d| (d[1], d[0]));
                    for i in 0..4 {
                        let [s, t] = quad.uvs[i];
                        verts.push(FxVert {
                            pos: quad.verts[i],
                            uv: [a * s + c * t + tx, b * s + d * t + ty],
                            rgba,
                            size,
                            flags,
                            feather,
                        });
                    }
                }
                runs.push(FxRun {
                    image: draw.image.to_string(),
                    blend: draw.blend,
                    quads: group.len(),
                    sort,
                    feather: feather > 0.0,
                    view_depth: sh.is_some_and(|sh| sh.feather_view_depth),
                    distort: distortion.is_some(),
                    cloud: None,
                    depth_always: draw.depth_always,
                });
            }
        }

        self.queue
            .write_buffer(&self.fx.vertex_buf, 0, bytemuck::cast_slice(&verts));
        self.fx.runs = runs;
    }

    /// The frame's scene lights (at most `MAX_SCENE_LIGHTS`), nearest the
    /// camera first: entity picks and the dlight pass see all of them, the
    /// prop vertex shader the first `MAX_LIGHTS`. An empty slice zeroes
    /// every slot.
    pub fn set_fx_lights(&mut self, lights: &[FxLight]) {
        self.dynamic.scene_lights = entity_light::scene_lights(lights);
        let uniform = FxLightsUniform::from_lights(lights);
        self.queue
            .write_buffer(&self.fx_lights_buf, 0, bytemuck::bytes_of(&uniform));
    }
}

/// The smaller half-extent of a ring-ordered quad.
pub(super) fn quad_half_extent(v: &[[f32; 3]; 4]) -> f32 {
    let [a, b, _, d] = v.map(glam::Vec3::from);
    0.5 * (b - a).length().min((d - a).length())
}

pub(super) fn create_fx_pass(
    device: &wgpu::Device,
    format: wgpu::TextureFormat,
    camera_layout: &wgpu::BindGroupLayout,
    texture_layout: &wgpu::BindGroupLayout,
    soft_layout: &wgpu::BindGroupLayout,
    copy_layout: &wgpu::BindGroupLayout,
    samples: u32,
) -> FxPass {
    let vertex_buf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("fx vertices"),
        size: (MAX_FX_DRAWN * 4 * std::mem::size_of::<FxVert>()) as u64,
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    // Quad n spans vertices 4n..4n+4. u32 because MAX_FX_DRAWN * 4 can
    // exceed u16::MAX.
    let indices: Vec<u32> = (0..MAX_FX_DRAWN)
        .flat_map(|q| {
            let b = (q * 4) as u32;
            [b, b + 1, b + 2, b, b + 2, b + 3]
        })
        .collect();
    let index_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("fx indices"),
        contents: bytemuck::cast_slice(&indices),
        usage: wgpu::BufferUsages::INDEX,
    });

    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("fx.wgsl"),
        source: wgpu::ShaderSource::Wgsl(FX_WGSL.into()),
    });
    let hard_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("fx pipeline layout"),
        bind_group_layouts: &[Some(camera_layout), Some(texture_layout)],
        immediate_size: 0,
    });
    let soft_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("fx soft pipeline layout"),
        bind_group_layouts: &[Some(camera_layout), Some(texture_layout), Some(soft_layout)],
        immediate_size: 0,
    });
    let distort_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("fx distortion pipeline layout"),
        bind_group_layouts: &[Some(camera_layout), Some(texture_layout), Some(copy_layout)],
        immediate_size: 0,
    });
    // The pipelines differ only in blend (with the depth write it implies),
    // layout and fragment entry, so runs can interleave within one pass.
    let make_in = |label, fs_entry, blend, layout: &wgpu::PipelineLayout, always: bool| {
        device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some(label),
            layout: Some(layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<FxVert>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![
                        0 => Float32x3, // pos
                        1 => Float32x2, // uv
                        2 => Float32x4, // rgba
                        3 => Float32, // size
                        4 => Uint32, // flags
                        5 => Float32, // feather
                    ],
                })],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some(fs_entry),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                // Decal tangent frames flip with the surface normal and
                // billboards face the camera; neither has a meaningful winding.
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                // The world must occlude fx quads, but overlapping blended
                // quads must all draw; an unblended stage writes depth.
                depth_write_enabled: Some(blend.is_none()),
                depth_compare: Some(if always {
                    wgpu::CompareFunction::Always
                } else {
                    wgpu::CompareFunction::LessEqual
                }),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: wgpu::MultisampleState {
                count: samples,
                mask: !0,
                alpha_to_coverage_enabled: false,
            },
            multiview_mask: None,
            cache: None,
        })
    };
    let make = |label, fs_entry, blend, soft: bool| {
        let layout = if soft {
            &soft_pipeline_layout
        } else {
            &hard_layout
        };
        make_in(label, fs_entry, blend, layout, false)
    };

    let [alpha, additive, screen, opaque] = [
        FxBlend::Alpha,
        FxBlend::Add,
        FxBlend::Screen,
        FxBlend::Opaque,
    ]
    .map(fx_blend_state);
    let pipelines = [
        make("fx pipeline", "fs_main", alpha, false),
        make("fx additive pipeline", "fs_additive", additive, false),
        make("fx screen pipeline", "fs_screen", screen, false),
        make("fx opaque pipeline", "fs_main", opaque, false),
    ];
    let soft_pipelines = [
        make("fx soft pipeline", "fs_soft", alpha, true),
        make(
            "fx soft additive pipeline",
            "fs_soft_additive",
            additive,
            true,
        ),
        make("fx soft screen pipeline", "fs_soft_screen", screen, true),
        // nothing to fade into: an opaque stage writes its own depth
        make("fx soft opaque pipeline", "fs_main", opaque, true),
    ];
    let always_pipelines = [
        ("fx always pipeline", "fs_main", alpha),
        ("fx always additive pipeline", "fs_additive", additive),
        ("fx always screen pipeline", "fs_screen", screen),
        ("fx always opaque pipeline", "fs_main", opaque),
    ]
    .map(|(label, fs_entry, blend)| make_in(label, fs_entry, blend, &hard_layout, true));

    // distortion_scale_zfeather's emissive state: (SrcAlpha, InvSrcAlpha)
    let distort_pipeline = make_in(
        "fx distortion pipeline",
        "fs_distort",
        alpha,
        &distort_layout,
        false,
    );
    FxPass {
        pipelines,
        soft_pipelines,
        always_pipelines,
        distort_pipeline,
        vertex_buf,
        index_buf,
        textures: HashMap::new(),
        runs: Vec::new(),
        clouds: fx_cloud::CloudPass::new(device, format, camera_layout, texture_layout, samples),
    }
}

/// What a quad's vertex colour is scaled by on its way into the
/// framebuffer, from the material's first stage: the lit gens (`vertex`,
/// `identityLighting`, `constLighting`, `wave`) take `identityLight`,
/// `identity`, `exactVertex` and `const` none. A material with no script is
/// implicit 2D or fx, `rgbGen vertex` (0x4fc440 case -4); see
/// docs/research/cod11-gamma.md, section 5.
pub(super) fn colour_scale(shader: Option<&Material>, identity_light: f32) -> f32 {
    match shader
        .and_then(|sh| sh.stages.first())
        .map(|st| &st.rgb_gen)
    {
        Some(RgbGen::Identity | RgbGen::ExactVertex | RgbGen::Const(_)) => 1.0,
        _ => identity_light,
    }
}

/// Resolves an fx or HUD shader name to a loadable texture path through
/// the shader scripts' image map and `assets::resolve_bundle_image`.
/// `load_path_image` never fails (a missing file becomes the default
/// image), so unresolvable names must be caught here or they draw
/// default-image quads.
pub(super) fn resolve_fx_path(
    shader_images: &HashMap<String, String>,
    fs: &dyn AssetSource,
    name: &str,
) -> Option<String> {
    if name.is_empty() {
        return None;
    }
    let path = shader_images.get(name).map(String::as_str).unwrap_or(name);
    assets::resolve_bundle_image(fs, path)
}

/// How an effect's image loads. The efx `shaders` field registers through
/// `RE_RegisterShader` (refexport 0x4fca80, lightmap index -4), so an
/// effect image is mipmapped and takes `r_picmip` unless its shader says
/// `nomipmaps` / `nopicmip` (docs/research/cod11-front-end.md, section 14).
pub(super) struct FxImage<'a> {
    pub(super) gamma: Option<&'a [u8; 256]>,
    pub(super) picmip: u32,
    pub(super) store: texture_settings::Store,
    pub(super) mipmap: bool,
}

/// `upload_levels` handles both RGBA sprites and BC `.dds` mip chains (many
/// effect textures are DXT5). Unresolvable names warn once; the caller
/// caches the `None`.
pub(super) fn resolve_fx_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    texture_layout: &wgpu::BindGroupLayout,
    fs: &dyn AssetSource,
    shader_images: &HashMap<String, String>,
    name: &str,
    how: FxImage,
) -> Option<wgpu::BindGroup> {
    let (path, mut img) = if name == vcod_common::shader::DEFAULT_IMAGE {
        (name.to_string(), assets::default_image())
    } else {
        let Some(path) = resolve_fx_path(shader_images, fs, name) else {
            log::warn!("fx shader {name:?}: no texture found for it, dropping its quads");
            return None;
        };
        let img = assets::load_path_image(fs, &path);
        (path, img)
    };
    let load = texture_settings::path_levels(fs);
    let mips = if how.mipmap {
        texture_settings::Mips::chain(&path, &load)
    } else {
        texture_settings::Mips::single(&path, &load)
    };
    let levels = texture_settings::prepare(&mut img, how.picmip, how.gamma, how.store, mips);
    let view = upload_levels(device, queue, name, &img, &levels);
    Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some(name),
        layout: texture_layout,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: wgpu::BindingResource::TextureView(&view),
        }],
    }))
}
