//! The shader-script stage machinery: the stage flags and deform codes the WGSL
//! reads, `StageParams` and its evaluation, the stage batches and variants, the
//! stage pipelines and bind groups.

use super::*;
use crate::renderer::gpu_util;

pub const STAGE_FLAG_VERTEX_RGB: u32 = 1;
pub const STAGE_FLAG_VERTEX_ALPHA: u32 = 2;
pub const STAGE_FLAG_BUNDLE1_LIGHTMAP: u32 = 4;
pub const STAGE_FLAG_BUNDLE0_LIGHTMAP: u32 = 8;
// alphaFunc encoding in bits 16..32: 0 = none, GT0 = 16, LT128 = 32, GE128 = both
pub const STAGE_FLAG_ALPHAFUNC_GT0: u32 = 16;
pub const STAGE_FLAG_ALPHAFUNC_LT128: u32 = 32;
pub const STAGE_FLAG_ALPHAFUNC_GE128: u32 = 48;
pub const STAGE_FLAG_BUNDLE0_VECTOR: u32 = 64;
pub const STAGE_FLAG_BUNDLE1_VECTOR: u32 = 128;
/// Sky dome draws only: add the view origin (StageParams.eye_off) to vertices.
pub const STAGE_FLAG_EYE_OFFSET: u32 = 256;
/// Sky dome draws only: fog applies in exp mode, never in linear mode
/// (docs/protocol-1.1.md, configstring 12, VERIFIED live on mp_ship).
pub const STAGE_FLAG_SKY: u32 = 512;
/// `rgbGen vertex` scales the vertex rgb by identityLight; `exactVertex`
/// stays raw (0x4ffa60's rgbGen switch, cod11-shader-scripts.md 3).
pub const STAGE_FLAG_VERTEX_RGB_HALF: u32 = 1024;
/// The stage blends onto `GL_ONE`: GL_State loads a black fog colour for
/// it (CoDMP.exe 0x4d2ad0), so fog fades it instead of greying it.
pub const STAGE_FLAG_FOG_BLACK: u32 = 2048;
/// The tint rgb scales by identityLight, which the camera uniform carries:
/// `identityLighting`, `constLighting` and `wave` (0x4ffa60).
pub const STAGE_FLAG_TINT_LIGHT: u32 = 4096;
/// `lightingDiffuse`: the entity pass lights the stage from its light set
/// (`dynamic_model.wgsl`); the world draws it as `identityLighting`.
pub const STAGE_FLAG_LIT: u32 = 8192;
/// `tcGen environment` on bundle 0 / 1: UVs from the view vector reflected
/// about the normal in the model's frame (RB_CalcEnvironmentTexCoords,
/// CoDMP.exe 0x514d40), before the bundle's tcMods.
pub const STAGE_FLAG_BUNDLE0_ENV: u32 = 16384;
pub const STAGE_FLAG_BUNDLE1_ENV: u32 = 32768;
/// Bundle 1's texture environment, a `nextbundle`'s `blendFunc`: `add`
/// adds it, `blend` decals it; neither multiplies (shader doc 3).
pub const STAGE_FLAG_ENV1_ADD: u32 = 65536;
pub const STAGE_FLAG_ENV1_DECAL: u32 = 131072;
/// The sprite deforms over vertices `vcod_common::deform` encoded at load.
pub const STAGE_FLAG_AUTOSPRITE: u32 = 1 << 20;
pub const STAGE_FLAG_AUTOSPRITE2: u32 = 1 << 21;
/// `nofog`: the stage draws unfogged.
pub const STAGE_FLAG_NOFOG: u32 = 1 << 22;

/// Sorted past `vc_shadows`' mask, the stage lays its own colour over what
/// is behind it and darkens that colour itself (`shadow_receive.wgsl`).
pub const STAGE_FLAG_SUN_SHADOW: u32 = 1 << 23;
/// Bundle 0's image clamps on `s` / `t` (`tex_samplers.wgsl`); bundle 1's
/// two bits sit two higher (`STAGE_FLAG_CLAMP1_S`, `_T`).
pub const STAGE_FLAG_CLAMP_S: u32 = 1 << 24;
pub const STAGE_FLAG_CLAMP_T: u32 = 1 << 25;
pub const STAGE_FLAG_CLAMP1_S: u32 = 1 << 26;
pub const STAGE_FLAG_CLAMP1_T: u32 = 1 << 27;
/// A CoD2 model's `lprobe*` stage (`Material::probe`), lit per pixel by
/// cod2_model.wgsl, and its `_a`, `_s`, `_dtl` and `_scroll` switches.
pub const STAGE_FLAG_COD2_PROBE: u32 = 1 << 28;
pub const STAGE_FLAG_COD2_ALPHA: u32 = 1 << 18;
pub const STAGE_FLAG_COD2_SPEC: u32 = 1 << 29;
pub const STAGE_FLAG_COD2_DETAIL: u32 = 1 << 30;
pub const STAGE_FLAG_COD2_SCROLL: u32 = 1 << 19;
/// A CoD2 `objective` set's stage (`ModelLight::Objective`).
pub const STAGE_FLAG_OBJECTIVE: u32 = 1 << 31;

/// `StageParams::deform_kinds` codes: retail's deform numbers
/// (CoDMP.exe 0x5149b0's switch), 0 for an empty slot.
pub const DEFORM_WAVE: u32 = 1;
pub const DEFORM_FLAP_S: u32 = 2;
pub const DEFORM_FLAP_T: u32 = 3;
pub const DEFORM_NORMAL: u32 = 4;
pub const DEFORM_BULGE: u32 = 6;
pub const DEFORM_MOVE: u32 = 7;
/// A wave's or flap's form in its kind's bits from here up ([`deform_form`]).
pub const DEFORM_FORM_SHIFT: u32 = 8;

/// deform.wgsl's table code for a wave form: sin 0, then square, triangle,
/// sawtooth, inverse sawtooth and noise (retail's genfuncs 1..6 less one).
pub(super) fn deform_form(form: &WaveForm) -> u32 {
    let code = match form {
        WaveForm::Sin => 0,
        WaveForm::Square => 1,
        WaveForm::Triangle => 2,
        WaveForm::Sawtooth => 3,
        WaveForm::InverseSawtooth => 4,
        WaveForm::Noise => 5,
    };
    code << DEFORM_FORM_SHIFT
}
/// `StageParams::deform_kinds[3]`, past the three deform slots: a CoD2
/// `lprobe_flag*` stage, whose vertices `dynamic_model.wgsl` waves
/// (cod2-materials.md 9.7).
pub const DEFORM_COD2_FLAG: u32 = 1;
/// `StageParams::deform_kinds[3]`: a CoD4 reflex sight's dot
/// (`Material::reflex`), which `viewmodel.wgsl` projects off the view's
/// angle to the lens, its `detailScale` in `eye_off` (cod4-client-view.md
/// 4.5).
pub const DEFORM_COD4_REFLEX: u32 = 2;
/// `StageParams::deform_kinds[3]`: a CoD4 `lp_flag_*` stage, whose
/// vertices `dynamic_model.wgsl` waves (`cod4_flag.wgsl`) and whose sun
/// takes `|N.L|` (cod4-world.md 3.4).
pub const DEFORM_COD4_FLAG: u32 = 3;

/// Whether a stage lays its own colour over the frame: no blend, an alpha
/// or a premultiplied blend. An additive stage adds light and a filter
/// (source from the frame's colour, or zero) tints what is behind, which the
/// shadow mask has already darkened.
pub(super) fn lays_own_colour(blend: &Option<(BlendFactor, BlendFactor)>) -> bool {
    match blend {
        None => true,
        Some((src, dst)) => {
            matches!(src, BlendFactor::One | BlendFactor::SrcAlpha) && *dst != BlendFactor::One
        }
    }
}

/// Per-stage draw parameters, one dynamic-offset slot per stage batch. WGSL
/// mirror is `StageParams` in shader.wgsl; byte offsets:
/// uv0 @0 and uv1 @24 as mat3x2 (column-major, matching [`bundle_affine`]),
/// turb01 @48, tint @64, flags @80, pad @84..96, then per bundle the two
/// tcGen vector bases, or `tcGen environment`'s s and t axes, padded to
/// vec4: vec0_s @96, vec0_t @112, vec1_s @128, vec1_t @144,
/// the sky dome's view-origin offset @160, then the deforms: wave @176,
/// bulge @192, move @208, normal @224, then the falloff and the CoD4
/// world fx (`fx`) @272. The entity, viewmodel and shadow
/// shaders declare only the prefix they read.
#[repr(C)]
#[derive(Copy, Clone, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct StageParams {
    pub(crate) uv0: [f32; 6],
    pub(crate) uv1: [f32; 6],
    pub(crate) turb01: [f32; 4],
    pub(crate) tint: [f32; 4],
    pub(crate) flags: u32,
    /// Bundle 0's lone `tcMod scroll` (s and t per second) or `rotate`
    /// (degrees per second), which an instance with its own material clock
    /// (`InstanceRaw::time_offset`) runs further; zero otherwise.
    pub(crate) uv0_rate: [f32; 3],
    pub(crate) vec0_s: [f32; 4],
    pub(crate) vec0_t: [f32; 4],
    pub(crate) vec1_s: [f32; 4],
    pub(crate) vec1_t: [f32; 4],
    pub(crate) eye_off: [f32; 4],
    /// The shader's deform records in script order, which `deform.wgsl`
    /// runs one after another: wave and flap `[base, amp, phase + t *
    /// rate, spread]`, bulge `[width, height, t * speed, 0]`, move this
    /// frame's offset, normal `[amp, t * freq, 0, 0]`.
    pub(crate) deform: [[f32; 4]; 3],
    /// Each record's `DEFORM_*` code; the fourth is padding.
    pub(crate) deform_kinds: [u32; 4],
    /// A `Material::falloff` stage's falloff: `[begin rgb, scale]`,
    /// `[end rgb, bias]`; the vertex shader applies it while the scale is
    /// nonzero (no flag bit is left), its kind in `eye_off.w`.
    pub(crate) falloff: [[f32; 4]; 2],
    /// A CoD4 world or static model surface's `cod4_worldfx` draw: a
    /// `zfeather` set's `featherParms.x`, then a distortion set's
    /// `distortionScale`; zero otherwise.
    pub(crate) fx: [f32; 4],
}

const _: () = assert!(std::mem::size_of::<StageParams>() == 288);
pub(crate) const STAGE_PARAMS_SIZE: u64 = std::mem::size_of::<StageParams>() as u64;

/// `tcGen environment`'s s and t axes, which the reflection is dotted
/// with: the drawing frame's y and z, or a static prop's model axes.
pub(super) fn env_axes(shader: &Material) -> [[f32; 4]; 2] {
    let [s, t] = shader
        .model_axes
        .unwrap_or([[0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]);
    [[s[0], s[1], s[2], 0.0], [t[0], t[1], t[2], 0.0]]
}

pub(super) const UV_AFFINE_IDENTITY: [f32; 6] = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];

/// Evaluates one stage's draw parameters at time `t`: bundle affines/turb,
/// tint from rgbGen/alphaGen, and the flag bits the WGSL reads. `None` when
/// `idx` is out of range or the stage has no bundles.
pub(crate) fn stage_params(shader: &Material, idx: usize, t: f32) -> Option<StageParams> {
    let st = shader.stages.get(idx)?;
    let b0 = st.bundles.first()?;
    let b1 = st.bundles.get(1);

    let mut flags = match &st.alpha_func {
        None => 0,
        Some(AlphaFunc::Gt0) => STAGE_FLAG_ALPHAFUNC_GT0,
        Some(AlphaFunc::Lt128) => STAGE_FLAG_ALPHAFUNC_LT128,
        Some(AlphaFunc::Ge128) => STAGE_FLAG_ALPHAFUNC_GE128,
    };
    // the lightmap and the dlight image share the lightmap's coordinates
    if b0.takes_lightmap_coords() {
        flags |= STAGE_FLAG_BUNDLE0_LIGHTMAP;
    }
    if b1.is_some_and(Bundle::takes_lightmap_coords) {
        flags |= STAGE_FLAG_BUNDLE1_LIGHTMAP;
    }
    // each bundle wraps its own axes (image flags 0x10, 0x20; 0x4ec6e9)
    let clamp_bits = |c: Clamp, s, t| (if c.s { s } else { 0 }) | (if c.t { t } else { 0 });
    flags |= clamp_bits(b0.clamp, STAGE_FLAG_CLAMP_S, STAGE_FLAG_CLAMP_T);
    if let Some(b1) = b1 {
        flags |= clamp_bits(b1.clamp, STAGE_FLAG_CLAMP1_S, STAGE_FLAG_CLAMP1_T);
    }
    match b1.map(|b| b.env) {
        Some(TexEnv::Add) => flags |= STAGE_FLAG_ENV1_ADD,
        Some(TexEnv::Decal) => flags |= STAGE_FLAG_ENV1_DECAL,
        _ => {}
    }

    let mut rgb = [1.0f32; 3];
    // a table wave clamps after its identityLight scale (0x514c00), which
    // the shader applies, so it reaches the shader unclamped above
    let mut rgb_max = 1.0f32;
    // identityLight (1 / (1 << overbrightBits)) scales the lit gens in the
    // shader; identity, const and exactVertex pass through unscaled
    // (docs/research/cod11-gamma.md, section 5).
    match &st.rgb_gen {
        RgbGen::Vertex => {
            flags |= STAGE_FLAG_VERTEX_RGB | STAGE_FLAG_VERTEX_RGB_HALF;
        }
        RgbGen::ExactVertex => flags |= STAGE_FLAG_VERTEX_RGB,
        RgbGen::Identity => {}
        RgbGen::IdentityLighting => flags |= STAGE_FLAG_TINT_LIGHT,
        RgbGen::LightingDiffuse => flags |= STAGE_FLAG_TINT_LIGHT | STAGE_FLAG_LIT,
        // the entity's shaderRGBA, which no stock entity material reads
        RgbGen::LightingPrecalc => {}
        RgbGen::Const(c) => rgb = *c,
        RgbGen::ConstLighting(c) => {
            flags |= STAGE_FLAG_TINT_LIGHT;
            rgb = *c;
        }
        RgbGen::Wave(w) => {
            // the noise glow skips identityLight (0x514c00)
            if w.form != WaveForm::Noise {
                flags |= STAGE_FLAG_TINT_LIGHT;
                rgb_max = f32::MAX;
            }
            rgb = [rgb_wave_value(w, t); 3];
        }
    }
    let alpha = match &st.alpha_gen {
        // `rgbGen vertex` and `exactVertex` imply it unless an alphaGen was
        // set first (the parser, shader doc 3)
        AlphaGen::Vertex => {
            flags |= STAGE_FLAG_VERTEX_ALPHA;
            1.0
        }
        AlphaGen::Const(v) => *v,
        AlphaGen::Wave(w) => wave_value(w, t),
        // the world entity's shaderRGBA is opaque white; no stock entity
        // material reads an entity alpha
        AlphaGen::Identity | AlphaGen::Entity => 1.0,
    };

    let mut p = StageParams {
        uv0: bundle_affine(&b0.tcmods, t),
        uv1: b1.map_or(UV_AFFINE_IDENTITY, |b| bundle_affine(&b.tcmods, t)),
        // [amp0, now0, amp1, now1]; the VS adds amp*sin(pos_axis/1024 + now),
        // s on x+z, t on y (RB_CalcTurbulentTexCoords)
        turb01: {
            let tb0 = bundle_turb(&b0.tcmods, t);
            let tb1 = b1.map_or([0.0; 4], |b| bundle_turb(&b.tcmods, t));
            [tb0[0], tb0[1], tb1[0], tb1[1]]
        },
        // Retail writes a table wave as colour bytes clamped after the
        // identityLight scale (cod11-shader-scripts.md 4, 0x514c00); the
        // other clamps follow Q3 and are unverified against retail.
        tint: [
            rgb[0].clamp(0.0, rgb_max),
            rgb[1].clamp(0.0, rgb_max),
            rgb[2].clamp(0.0, rgb_max),
            alpha.clamp(0.0, 1.0),
        ],
        flags,
        uv0_rate: match b0.tcmods.as_slice() {
            [TcMod::Scroll(s, t)] => [*s, *t, 0.0],
            [TcMod::Rotate(dps)] => [0.0, 0.0, *dps],
            _ => [0.0; 3],
        },
        vec0_s: [0.0; 4],
        vec0_t: [0.0; 4],
        vec1_s: [0.0; 4],
        vec1_t: [0.0; 4],
        eye_off: [0.0; 4],
        deform: [[0.0; 4]; 3],
        deform_kinds: [0; 4],
        falloff: shader.falloff.map_or([[0.0; 4]; 2], |f| {
            let [b, e] = [f.begin, f.end];
            [[b[0], b[1], b[2], f.scale], [e[0], e[1], e[2], f.bias]]
        }),
        fx: {
            let feather = shader.feather.filter(|_| shader.feather_view_depth);
            let d = shader.distortion.unwrap_or([0.0; 2]);
            [feather.unwrap_or(0.0), d[0], d[1], 0.0]
        },
    };
    match &b0.tc_gen {
        TcGen::Base => {}
        TcGen::Vector(v) => {
            flags |= STAGE_FLAG_BUNDLE0_VECTOR;
            p.vec0_s = [v[0], v[1], v[2], 0.0];
            p.vec0_t = [v[3], v[4], v[5], 0.0];
        }
        TcGen::Environment => {
            flags |= STAGE_FLAG_BUNDLE0_ENV;
            [p.vec0_s, p.vec0_t] = env_axes(shader);
        }
    }
    match b1.map(|b| &b.tc_gen) {
        None | Some(TcGen::Base) => {}
        Some(TcGen::Vector(v)) => {
            flags |= STAGE_FLAG_BUNDLE1_VECTOR;
            p.vec1_s = [v[0], v[1], v[2], 0.0];
            p.vec1_t = [v[3], v[4], v[5], 0.0];
        }
        Some(TcGen::Environment) => {
            flags |= STAGE_FLAG_BUNDLE1_ENV;
            [p.vec1_s, p.vec1_t] = env_axes(shader);
        }
    }
    // a wave of rate 0 is one phase for every vertex (0x513570, 0x5136e0);
    // the phase goes up as its fraction, which indexes the 1024-entry
    // table as the whole would, keeping the f32 add on the GPU small
    let wave = |w: &Wave, spread: f32| {
        let spread = if w.freq == 0.0 { 0.0 } else { spread };
        let phase = (f64::from(w.phase) + f64::from(t) * f64::from(w.freq)).rem_euclid(1.0);
        [w.base, w.amp, phase as f32, spread]
    };
    let records = shader.deforms.iter().filter_map(|d| match d {
        Deform::Wave { wave: w, spread } => {
            Some((DEFORM_WAVE | deform_form(&w.form), wave(w, *spread)))
        }
        Deform::Flap(f) => Some((
            if f.along_t {
                DEFORM_FLAP_T
            } else {
                DEFORM_FLAP_S
            } | deform_form(&f.wave.form),
            wave(&f.wave, f.spread),
        )),
        Deform::Bulge(b) => Some((DEFORM_BULGE, [b.width, b.height, t * b.speed, 0.0])),
        Deform::Move { by, wave: w } => {
            let k = wave_value(w, t);
            Some((DEFORM_MOVE, [by[0] * k, by[1] * k, by[2] * k, 0.0]))
        }
        Deform::Normal(n) => Some((DEFORM_NORMAL, [n.amp, t * n.freq, 0.0, 0.0])),
        Deform::Other => None,
    });
    for (i, (kind, rec)) in records.take(3).enumerate() {
        p.deform_kinds[i] = kind;
        p.deform[i] = rec;
    }
    match shader.sprite {
        Some(Sprite::Auto) => flags |= STAGE_FLAG_AUTOSPRITE,
        Some(Sprite::Auto2) => flags |= STAGE_FLAG_AUTOSPRITE2,
        None => {}
    }
    if shader.nofog {
        flags |= STAGE_FLAG_NOFOG;
    }
    if matches!(st.blend, Some((_, BlendFactor::One))) {
        flags |= STAGE_FLAG_FOG_BLACK;
    }
    if shader.sort > SORT_DECAL && lays_own_colour(&st.blend) {
        flags |= STAGE_FLAG_SUN_SHADOW;
    }
    // an entity's CoD2 probe stage; the world and props draw it as before
    if let Some(pr) = shader
        .probe
        .filter(|_| st.rgb_gen == RgbGen::LightingDiffuse)
    {
        flags |= STAGE_FLAG_COD2_PROBE;
        let bit = |on: bool, f: u32| if on { f } else { 0 };
        flags |= bit(pr.alpha, STAGE_FLAG_COD2_ALPHA)
            | bit(pr.specular, STAGE_FLAG_COD2_SPEC)
            | bit(pr.detail, STAGE_FLAG_COD2_DETAIL)
            | bit(pr.scroll, STAGE_FLAG_COD2_SCROLL);
        if pr.flag {
            p.deform_kinds[3] = DEFORM_COD2_FLAG;
        }
        if shader.phong.and_then(|p| p.cod4).is_some_and(|c| c.flag) {
            p.deform_kinds[3] = DEFORM_COD4_FLAG;
        }
        // the sky's eye offset is free on a model stage
        p.eye_off = [pr.detail_scale[0], pr.detail_scale[1], 0.0, 0.0];
        // a CoD4 lit model set's envMapParms, normal map, ambient and sun
        // shadow map switches (cod2_model.wgsl's `cod4_model`) in the tcGen vector
        // slots, which a model stage never reads
        if let Some(c) = shader.phong.and_then(|p| p.cod4) {
            p.vec0_s = c.env;
            let ambient = c.light == vcod_common::material::Cod4Light::GridAmbient;
            p.vec0_t = [
                f32::from(u8::from(c.normal_map)),
                f32::from(u8::from(ambient)),
                f32::from(u8::from(c.sun_shadow)),
                0.0,
            ];
        }
    }
    if shader.model_light == ModelLight::Objective {
        flags |= STAGE_FLAG_OBJECTIVE;
    }
    // CoD4's falloff kind and eye offset (cod4-world.md 9.5): a CoD4
    // stage is neither the sky nor a CoD2 probe, the two other eye_off users
    if let Some(f) = shader.falloff {
        p.eye_off[3] = match f.kind {
            FalloffKind::Distance => 0.0,
            FalloffKind::Angle => 1.0,
        };
    }
    if let Some(e) = shader.eye_offset {
        p.eye_off[2] = e;
    }
    // a reflex sight's detailScale; it carries no falloff or eye offset
    if let Some([sx, sy]) = shader.reflex {
        p.deform_kinds[3] = DEFORM_COD4_REFLEX;
        p.eye_off[0] = sx;
        p.eye_off[1] = sy;
    }
    p.flags = flags;
    Some(p)
}

/// A frame-indexed set of bind groups: one per animMap frame, or a single
/// static entry (fps 0 sticks at index 0).
pub(super) struct AnimFrames {
    pub(super) fps: f32,
    pub(super) groups: Vec<wgpu::BindGroup>,
}

impl AnimFrames {
    /// A single, non-animating entry (fps 0 sticks at index 0).
    pub(super) fn static_(group: wgpu::BindGroup) -> Self {
        Self {
            fps: 0.0,
            groups: vec![group],
        }
    }

    pub(super) fn bind_group(&self, t: f32) -> &wgpu::BindGroup {
        &self.groups[anim_frame_index(self.fps, self.groups.len(), t)]
    }
}

/// One authored stage expanded over its parent batch's index range. Batches
/// of one material share the params slot per stage.
pub(super) struct StageBatch {
    /// Dynamic-offset slot of this (material, stage)'s params.
    pub(super) slot: u32,
    pub(super) variant: StageVariant,
    pub(super) two_sided: bool,
    pub(super) bias: bool,
    /// group(1): bundle-0 image + lightmap page + lightmap sampler.
    pub(super) mat: AnimFrames,
    /// group(2): params UBO slot + bundle-1 image.
    pub(super) stage: AnimFrames,
    /// A CoD2 lit draw: the main pass draws it through `WorldGpu::cod2`
    /// in place of the stage; every other pass keeps the stage.
    pub(super) cod2: Option<cod2::LitBind>,
    /// A CoD4 feathered or distortion stage: the main pass draws it
    /// through `Renderer::world_fx` past the frame's split.
    pub(super) fx: cod4_worldfx::WorldFx,
}

/// One dlight-pass stage over a parent batch's range, drawn once per light
/// whose gathered range holds that batch.
pub(super) struct DlightBatch {
    pub(super) blend: DlightBlend,
    pub(super) two_sided: bool,
    pub(super) bias: bool,
    pub(super) slot: u32,
    pub(super) mat: AnimFrames,
    pub(super) stage: AnimFrames,
}

/// One light's draws this frame: its slot in the light uniform, the moved
/// brush model whose camera it draws under (`None` for the world), and the
/// gathered range in `WorldGpu::dlight_index_buf`.
pub(super) struct DlightRun {
    pub(super) light: u32,
    pub(super) moved: Option<usize>,
    pub(super) draw: DrawRange,
}

/// floor(t * fps) % len, clamped to a valid frame; static or empty anims stick at 0.
pub(super) fn anim_frame_index(fps: f32, len: usize, t: f32) -> usize {
    if fps <= 0.0 || len == 0 {
        return 0;
    }
    (t.max(0.0) * fps) as usize % len
}

/// Which prebuilt stage pipeline a StageBatch draws with; selected from
/// `ClassedStage` plus the stage's blend pair.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub(super) enum StageVariant {
    /// Unblended: depth write on, alphafunc discard in fs_stage.
    Opaque,
    /// SrcAlpha/OneMinusSrcAlpha, no depth write.
    Blend,
    /// Blend that kept depth write (explicit depthWrite keyword).
    BlendDepthWrite,
    /// dst factor exactly GL_ONE with src GL_ONE, drawn One/One.
    AdditiveFixed,
    /// Any other additive pair, drawn SrcAlpha/One so source alpha carries.
    AdditiveFaded,
    /// Additive stages that kept depth write (explicit depthWrite keyword).
    AdditiveFixedDepthWrite,
    AdditiveFadedDepthWrite,
    /// A CoD2 multiply decal (`Material::vertex_multiply`): fs_multiply's
    /// factor times the framebuffer, no depth write.
    Multiply,
    /// A CoD4 `distfalloff` stage (`Material::falloff`): (DstColor,
    /// SrcColor), twice the colour times the framebuffer, no depth write.
    Multiply2x,
    /// A CoD4 premultiplied (OneMinusDstColor, One) stage
    /// (`Material::premultiplied`): the colour times its alpha, times the
    /// framebuffer's complement, onto the framebuffer; no depth write.
    AdditiveInvDst,
}
pub(super) const STAGE_VARIANTS: usize = 10;
/// `StageVariant`s in discriminant order, the stage pipeline table's index.
pub(super) const STAGE_VARIANT_ORDER: [StageVariant; STAGE_VARIANTS] = [
    StageVariant::Opaque,
    StageVariant::Blend,
    StageVariant::BlendDepthWrite,
    StageVariant::AdditiveFixed,
    StageVariant::AdditiveFaded,
    StageVariant::AdditiveFixedDepthWrite,
    StageVariant::AdditiveFadedDepthWrite,
    StageVariant::Multiply,
    StageVariant::Multiply2x,
    StageVariant::AdditiveInvDst,
];

/// A stage variant's fragment entry in shader.wgsl, blend and depth write.
pub(super) fn stage_variant_state(
    v: StageVariant,
) -> (&'static str, Option<wgpu::BlendState>, bool) {
    let both = |src_factor, dst_factor| wgpu::BlendState {
        color: wgpu::BlendComponent {
            src_factor,
            dst_factor,
            operation: wgpu::BlendOperation::Add,
        },
        alpha: wgpu::BlendComponent {
            src_factor,
            dst_factor,
            operation: wgpu::BlendOperation::Add,
        },
    };
    use wgpu::BlendFactor as F;
    let one_one = both(F::One, F::One);
    let srcalpha_one = both(F::SrcAlpha, F::One);
    match v {
        StageVariant::Opaque => ("fs_stage", None, true),
        StageVariant::Blend => ("fs_stage", Some(wgpu::BlendState::ALPHA_BLENDING), false),
        StageVariant::BlendDepthWrite => ("fs_stage", Some(wgpu::BlendState::ALPHA_BLENDING), true),
        StageVariant::AdditiveFixed => ("fs_stage", Some(one_one), false),
        StageVariant::AdditiveFaded => ("fs_stage", Some(srcalpha_one), false),
        // explicit depthWrite on additive stages
        StageVariant::AdditiveFixedDepthWrite => ("fs_stage", Some(one_one), true),
        StageVariant::AdditiveFadedDepthWrite => ("fs_stage", Some(srcalpha_one), true),
        // dst * src colour; the destination alpha stays
        StageVariant::Multiply => (
            "fs_multiply",
            Some(wgpu::BlendState {
                color: wgpu::BlendComponent {
                    src_factor: F::Zero,
                    dst_factor: F::Src,
                    operation: wgpu::BlendOperation::Add,
                },
                alpha: wgpu::BlendComponent {
                    src_factor: F::Zero,
                    dst_factor: F::One,
                    operation: wgpu::BlendOperation::Add,
                },
            }),
            false,
        ),
        // src * a * (1 - dst) + dst; the destination alpha stays
        StageVariant::AdditiveInvDst => (
            "fs_stage_premul",
            Some(wgpu::BlendState {
                color: wgpu::BlendComponent {
                    src_factor: F::OneMinusDst,
                    dst_factor: F::One,
                    operation: wgpu::BlendOperation::Add,
                },
                alpha: wgpu::BlendComponent {
                    src_factor: F::Zero,
                    dst_factor: F::One,
                    operation: wgpu::BlendOperation::Add,
                },
            }),
            false,
        ),
        // src * dst + dst * src; the destination alpha stays
        StageVariant::Multiply2x => (
            "fs_stage",
            Some(wgpu::BlendState {
                color: wgpu::BlendComponent {
                    src_factor: F::Dst,
                    dst_factor: F::Src,
                    operation: wgpu::BlendOperation::Add,
                },
                alpha: wgpu::BlendComponent {
                    src_factor: F::Zero,
                    dst_factor: F::One,
                    operation: wgpu::BlendOperation::Add,
                },
            }),
            false,
        ),
    }
}

/// The engine's polygon offsets, `glPolygonOffset(r_offsetfactor,
/// r_offsetunits)` at their registered -1 and -2, doubled for
/// `polygonOffset2` and with no slope for `polygonOffsetConst` (CoDMP.exe
/// 0x4d5991..0x4d5a05, 0x4b3f0a; events doc 9.5).
pub(crate) fn depth_bias(form: PolygonOffset) -> wgpu::DepthBiasState {
    let (slope_scale, constant) = match form {
        PolygonOffset::Single => (-1.0, -2),
        PolygonOffset::Double => (-2.0, -4),
        PolygonOffset::Const => (0.0, -2),
    };
    wgpu::DepthBiasState {
        constant,
        slope_scale,
        clamp: 0.0,
    }
}

/// Group 1 of the world and stage pipelines: the colour image, the
/// lightmap page, a sampler and the page's block runs.
pub(super) fn material_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("material layout"),
        entries: &[
            gpu_util::texture_2d(0),
            gpu_util::texture_2d(1),
            // images sample through the camera group's samplers
            gpu_util::filtering_sampler(3),
            // the lightmap page's block runs (lightmap_blocks), read by
            // vc_bicubic's taps
            gpu_util::texture(
                4,
                wgpu::TextureSampleType::Uint,
                wgpu::TextureViewDimension::D2,
                false,
            ),
        ],
    })
}

pub(super) fn stage_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    // Group 2 of the stage pipelines: the per-stage params slot plus
    // bundle 1's image (white is bound when bundle 1 has none).
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("stage params layout"),
        entries: &[
            gpu_util::uniform_sized(
                0,
                wgpu::ShaderStages::VERTEX_FRAGMENT,
                true,
                STAGE_PARAMS_SIZE,
            ),
            gpu_util::texture_2d(1),
        ],
    })
}

/// `f(0..n)` on scoped threads, in index order. Pipeline creation is
/// thread safe, and a driver that compiles on the calling thread (lavapipe)
/// takes seconds a pipeline for `shader.wgsl`'s stage entries.
pub(crate) fn par_build<T: Send>(n: usize, f: impl Fn(usize) -> T + Sync) -> Vec<T> {
    par_build_on(n, n, f)
}

/// [`par_build`] on at most `workers` threads, each taking the next index
/// as it finishes one: no thread waits on a slow neighbour's batch.
pub(crate) fn par_build_on<T: Send>(
    workers: usize,
    n: usize,
    f: impl Fn(usize) -> T + Sync,
) -> Vec<T> {
    let next = std::sync::atomic::AtomicUsize::new(0);
    let mut out: Vec<Option<T>> = (0..n).map(|_| None).collect();
    std::thread::scope(|s| {
        let (f, next) = (&f, &next);
        let handles: Vec<_> = (0..workers.min(n))
            .map(|_| {
                s.spawn(move || {
                    let mut done = Vec::new();
                    loop {
                        let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        if i >= n {
                            return done;
                        }
                        done.push((i, f(i)));
                    }
                })
            })
            .collect();
        for h in handles {
            for (i, t) in h.join().unwrap() {
                out[i] = Some(t);
            }
        }
    });
    out.into_iter()
        .map(|t| t.expect("every index built"))
        .collect()
}

/// A pipeline over shader.wgsl's world vertex layout into the scene target
/// (its format and sample count): the world, layer, overlay and stage
/// pipelines, and the marks'.
#[expect(clippy::too_many_arguments)]
pub(crate) fn scene_pipeline(
    device: &wgpu::Device,
    shader: &wgpu::ShaderModule,
    (format, samples): (wgpu::TextureFormat, u32),
    label: &str,
    layout: &wgpu::PipelineLayout,
    (vs_entry, fs_entry): (&str, &str),
    bias: wgpu::DepthBiasState,
    alpha_to_coverage: bool,
    blend: Option<wgpu::BlendState>,
    depth_write: bool,
    cull: Option<wgpu::Face>,
) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(label),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some(vs_entry),
            compilation_options: Default::default(),
            buffers: &[Some(gpu_util::world_vertex_layout(
                &gpu_util::WORLD_VERTEX_ATTRS,
            ))],
        },
        fragment: Some(wgpu::FragmentState {
            module: shader,
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
            // Map triangles are wound clockwise seen from the front.
            // Cull back faces like the engine; coplanar interior and
            // exterior faces z-fight otherwise.
            front_face: wgpu::FrontFace::Cw,
            cull_mode: cull,
            ..Default::default()
        },
        depth_stencil: Some(wgpu::DepthStencilState {
            format: DEPTH_FORMAT,
            depth_write_enabled: Some(depth_write),
            depth_compare: Some(wgpu::CompareFunction::LessEqual),
            stencil: Default::default(),
            bias,
        }),
        multisample: wgpu::MultisampleState {
            count: samples,
            mask: !0,
            alpha_to_coverage_enabled: alpha_to_coverage,
        },
        multiview_mask: None,
        cache: None,
    })
}

/// Pipeline variant plus the draw facts a StageBatch needs. `None` when
/// `idx` is out of range.
pub(super) fn stage_draw_facts(sh: &Material, idx: usize) -> Option<(StageVariant, bool, bool)> {
    let cs = sh.classify_stage(idx)?;
    let st = &sh.stages[idx];
    let variant = match cs.class {
        DrawClass::Opaque => StageVariant::Opaque,
        _ if sh.vertex_multiply => StageVariant::Multiply,
        _ if sh.falloff.is_some_and(|f| f.kind == FalloffKind::Distance) => {
            StageVariant::Multiply2x
        }
        _ if sh.premultiplied
            && st.blend == Some((BlendFactor::OneMinusDstColor, BlendFactor::One)) =>
        {
            StageVariant::AdditiveInvDst
        }
        DrawClass::Blend => {
            if cs.depth_write {
                StageVariant::BlendDepthWrite
            } else {
                StageVariant::Blend
            }
        }
        DrawClass::Additive => {
            let fixed = st.blend == Some((BlendFactor::One, BlendFactor::One));
            match (fixed, cs.depth_write) {
                (true, false) => StageVariant::AdditiveFixed,
                (true, true) => StageVariant::AdditiveFixedDepthWrite,
                (false, true) => StageVariant::AdditiveFadedDepthWrite,
                (false, false) => StageVariant::AdditiveFaded,
            }
        }
    };
    Some((variant, cs.two_sided, cs.bias))
}

/// Whether the stage's params change over time and need per-frame rewrites.
pub(crate) fn stage_animated(sh: &Material, idx: usize) -> bool {
    let Some(st) = sh.stages.get(idx) else {
        return false;
    };
    sh.deforms.iter().any(|d| *d != Deform::Other)
        || st.bundles.iter().any(|b| has_animated_tcmods(&b.tcmods))
        || matches!(st.rgb_gen, RgbGen::Wave(_))
        || matches!(st.alpha_gen, AlphaGen::Wave(_))
}

/// Group(1) and group(2) bind groups for one authored stage, shared by the
/// world stage-batch loop and the cloud dome. `lm` is the batch's
/// lightmap page (bundle-0 `$lightmap` samples it through the flag); the
/// stage's images load under `rule`.
#[expect(clippy::too_many_arguments)]
pub(super) fn stage_bind_groups(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    fs: &Pk3Fs,
    material_layout: &wgpu::BindGroupLayout,
    stage_layout: &wgpu::BindGroupLayout,
    params_buf: &wgpu::Buffer,
    params_size: u64,
    white_view: &wgpu::TextureView,
    lm: &LightmapPage,
    lightmap_sampler: &wgpu::Sampler,
    bundle_views: &mut BundleViews,
    rule: ImageRule,
    st: &Stage,
) -> (AnimFrames, AnimFrames) {
    let b0 = &st.bundles[0];
    let b1 = st.bundles.get(1);
    let mat_group = |b0_view: &wgpu::TextureView| {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("stage material bind group"),
            layout: material_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(b0_view),
                },
                // bundle-0 lightmaps sample here via the flag
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&lm.view),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::Sampler(lightmap_sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::TextureView(&lm.edges),
                },
            ],
        })
    };
    let stage_group = |b1_view: &wgpu::TextureView| {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("stage draw bind group"),
            layout: stage_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                        buffer: params_buf,
                        offset: 0,
                        size: wgpu::BufferSize::new(params_size),
                    }),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(b1_view),
                },
            ],
        })
    };
    let mut resolve = |p: &str| upload_bundle_view(device, queue, fs, bundle_views, p, rule);
    let mat = match (&b0.anim, &b0.image) {
        (Some(anim), _) => AnimFrames {
            fps: anim.fps,
            groups: anim.paths.iter().map(|p| mat_group(&resolve(p))).collect(),
        },
        (None, ImageRef::Path(p)) => AnimFrames::static_(mat_group(&resolve(p))),
        // $whiteimage draws white; $lightmap samples t_lightmap
        // through the flag, so bundle 0's own slot stays neutral
        (None, _) => AnimFrames::static_(mat_group(white_view)),
    };
    let stage = match b1.map(|b| (&b.anim, &b.image)) {
        Some((Some(anim), _)) => AnimFrames {
            fps: anim.fps,
            groups: anim
                .paths
                .iter()
                .map(|p| stage_group(&resolve(p)))
                .collect(),
        },
        Some((None, ImageRef::Path(p))) => AnimFrames::static_(stage_group(&resolve(p))),
        _ => AnimFrames::static_(stage_group(white_view)),
    };
    (mat, stage)
}

impl Renderer {
    pub(super) fn stage_pipeline(
        &self,
        variant: StageVariant,
        two_sided: bool,
        bias: bool,
    ) -> &wgpu::RenderPipeline {
        &self.stage_pipelines[variant as usize][usize::from(two_sided)][usize::from(bias)]
    }
}
