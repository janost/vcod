//! Entity model materials. A skin named `<type>@<image>` takes the stages of
//! `shadertypes/model/<type>.stype` with `$texturename` bound to the skin; a
//! skin without one is the implicit model skin, one opaque `lightingDiffuse`
//! stage (0x4fc440). Only `lightingDiffuse` stages take the light grid; the
//! rest draw their constant or wave colour
//! (docs/research/cod11-light-grid-and-leaf-lights.md, section 13).

use std::collections::HashMap;
use std::rc::Rc;

use crate::renderer::{
    DEPTH_FORMAT, DYNAMIC_INSTANCE_STRIDE, MSAA_SAMPLES, STAGE_PARAMS_SIZE, stage_animated,
    stage_params,
};
use vcod_common::pk3::Pk3Fs;
use vcod_common::shader::{
    AlphaGen, BlendFactor, Bundle, ImageRef, RgbGen, Shader, Stage, WarnSet, parse_shader,
    split_blocks,
};
use vcod_common::xmodel::VmVert;

/// The skin's own image in a template stage.
pub const TEXTURE_NAME: &str = "$texturename";

/// The implicit model skin: one opaque stage lit from the grid.
pub fn implicit(skin: &str) -> Shader {
    Shader {
        name: skin.to_string(),
        stages: vec![Stage {
            bundles: vec![Bundle {
                image: ImageRef::Path(TEXTURE_NAME.to_string()),
                anim: None,
                clamp: false,
                tcmods: Vec::new(),
                vector: None,
            }],
            blend: None,
            depth_write: None,
            alpha_func: None,
            rgb_gen: RgbGen::LightingDiffuse,
            alpha_gen: AlphaGen::Identity,
        }],
        ..Default::default()
    }
}

/// `<type>` of a `<type>@<image>` skin, lowercased; `None` without an `@`.
pub fn skin_type(skin: &str) -> Option<String> {
    let at = skin.rfind('@')?;
    let start = skin[..at].rfind(['/', '\\']).map_or(0, |i| i + 1);
    Some(skin[start..at].to_ascii_lowercase())
}

/// Parses a `.stype` body. vcod drops `tcGen environment` stages
/// (`*_env`); the blended stage they leave behind draws opaque instead of
/// over nothing.
pub fn from_stype(skin: &str, text: &str, warns: &mut WarnSet) -> Option<Shader> {
    let (_, body) = split_blocks(text).into_iter().next()?;
    let mut sh = parse_shader(skin, &body, warns);
    if sh.stages.is_empty() {
        return None;
    }
    if sh.dropped_stages > 0 {
        sh.stages[0].blend = None;
        sh.stages[0].depth_write = None;
    }
    Some(sh)
}

/// The skin's material: its template, or the implicit skin when it has no
/// `@` or the template is missing or empty.
pub fn load(fs: &Pk3Fs, skin: &str, warns: &mut WarnSet) -> Shader {
    skin_type(skin)
        .and_then(|kind| fs.read(&format!("shadertypes/model/{kind}.stype")))
        .and_then(|text| from_stype(skin, &String::from_utf8_lossy(&text), warns))
        .unwrap_or_else(|| implicit(skin))
}

/// Whether some stage takes the light grid: retail picks an entity's lights
/// only then (0x50e3e0, the `+0x54 & 0x18` test).
pub fn is_lit(sh: &Shader) -> bool {
    sh.stages
        .iter()
        .any(|st| st.rgb_gen == RgbGen::LightingDiffuse)
}

// ---- GPU side: the dynamic pass's per-stage pipelines and params ----

/// One stage of an entity material.
#[derive(Clone, Copy)]
pub(crate) struct EntStageDraw {
    /// Into `EntityMaterials::pipelines`.
    pub(crate) pipeline: usize,
    /// The white image instead of the surface's skin.
    pub(crate) white: bool,
    /// `StageParams` slot in `EntityMaterials::params_buf`.
    pub(crate) slot: u32,
}

/// One `StageParams` slot per entity material stage, at the widest uniform
/// offset alignment so no device limit is needed.
pub(crate) const ENT_STAGE_STRIDE: u64 = 256;
const MAX_ENT_STAGES: u32 = 1024;
const _: () = assert!(STAGE_PARAMS_SIZE <= ENT_STAGE_STRIDE);

/// A stage's blend pair and depth write.
type EntPipelineKey = (Option<(BlendFactor, BlendFactor)>, bool);

/// Entity materials on the GPU: their pipelines and stage params.
pub(crate) struct EntityMaterials {
    shader: wgpu::ShaderModule,
    layout: wgpu::PipelineLayout,
    format: wgpu::TextureFormat,
    /// One pipeline per (blend pair, depth write); entry 0 is opaque.
    pub(crate) pipelines: Vec<(EntPipelineKey, wgpu::RenderPipeline)>,
    pub(crate) params_buf: wgpu::Buffer,
    pub(crate) params_bg: wgpu::BindGroup,
    pub(crate) white_bg: wgpu::BindGroup,
    /// By skin name; shared by every model that names the skin.
    by_skin: HashMap<String, Rc<[EntStageDraw]>>,
    lit: HashMap<String, bool>,
    /// Stages whose params move with time; rewritten each frame.
    pub(crate) animated: Vec<(Shader, usize, u32)>,
    next_slot: u32,
    warns: WarnSet,
}

impl EntityMaterials {
    /// Group 3's params buffer and the pipeline layout over the dynamic
    /// pass's groups 0 to 2.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        camera_layout: &wgpu::BindGroupLayout,
        skin_layout: &wgpu::BindGroupLayout,
        bone_layout: &wgpu::BindGroupLayout,
        sampler: &wgpu::Sampler,
        white_view: &wgpu::TextureView,
    ) -> Self {
        let params_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("entity stage params layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: true,
                    min_binding_size: wgpu::BufferSize::new(STAGE_PARAMS_SIZE),
                },
                count: None,
            }],
        });
        let params_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("entity stage params"),
            size: u64::from(MAX_ENT_STAGES) * ENT_STAGE_STRIDE,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let params_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("entity stage params bind group"),
            layout: &params_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: &params_buf,
                    offset: 0,
                    size: wgpu::BufferSize::new(STAGE_PARAMS_SIZE),
                }),
            }],
        });
        let white_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("entity white image"),
            layout: skin_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(white_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(sampler),
                },
            ],
        });
        let shader = device.create_shader_module(wgpu::include_wgsl!("dynamic_model.wgsl"));
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("dynamic pipeline layout"),
            bind_group_layouts: &[
                Some(camera_layout),
                Some(skin_layout),
                Some(bone_layout),
                Some(&params_layout),
            ],
            immediate_size: 0,
        });
        let mut materials = Self {
            shader,
            layout,
            format,
            pipelines: Vec::new(),
            params_buf,
            params_bg,
            white_bg,
            by_skin: HashMap::new(),
            lit: HashMap::new(),
            animated: Vec::new(),
            next_slot: 0,
            warns: WarnSet::new(),
        };
        // pipeline 0 is opaque, the implicit skin's
        materials.pipeline(device, None, true);
        materials
    }

    /// Forgets every material, for a map change (retail reloads them too).
    pub(crate) fn clear(&mut self) {
        self.by_skin.clear();
        self.lit.clear();
        self.animated.clear();
        self.next_slot = 0;
    }

    /// The pipeline for a stage's blend pair and depth write, built on first use.
    fn pipeline(
        &mut self,
        device: &wgpu::Device,
        blend: Option<(BlendFactor, BlendFactor)>,
        depth_write: bool,
    ) -> usize {
        let key = (blend, depth_write);
        if let Some(i) = self.pipelines.iter().position(|(k, _)| *k == key) {
            return i;
        }
        let blend_state = key.0.as_ref().map(|(src, dst)| {
            let c = wgpu::BlendComponent {
                src_factor: wgpu_blend_factor(src),
                dst_factor: wgpu_blend_factor(dst),
                operation: wgpu::BlendOperation::Add,
            };
            wgpu::BlendState { color: c, alpha: c }
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("dynamic model pipeline"),
            layout: Some(&self.layout),
            vertex: wgpu::VertexState {
                module: &self.shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[
                    Some(wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<VmVert>() as u64,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &wgpu::vertex_attr_array![
                            0 => Float32x3, // pos
                            1 => Float32x3, // normal
                            2 => Float32x2, // uv
                            3 => Uint8x4,   // bone_indices
                            4 => Float32x4, // bone_weights
                        ],
                    }),
                    // `InstanceRaw`: the transform's four columns, then
                    // bone_base and light_set; the padding needs no attribute.
                    Some(wgpu::VertexBufferLayout {
                        array_stride: DYNAMIC_INSTANCE_STRIDE,
                        step_mode: wgpu::VertexStepMode::Instance,
                        attributes: &wgpu::vertex_attr_array![
                            5 => Float32x4,
                            6 => Float32x4,
                            7 => Float32x4,
                            8 => Float32x4,
                            9 => Uint32,
                            10 => Uint32,
                        ],
                    }),
                ],
            },
            fragment: Some(wgpu::FragmentState {
                module: &self.shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: self.format,
                    blend: blend_state,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                // xmodel winding, same as the viewmodel; cull nothing.
                front_face: wgpu::FrontFace::Cw,
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: Some(depth_write),
                // a later stage redraws the same depths, so LessEqual passes it
                depth_compare: Some(wgpu::CompareFunction::LessEqual),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: wgpu::MultisampleState {
                count: MSAA_SAMPLES,
                mask: !0,
                alpha_to_coverage_enabled: false,
            },
            multiview_mask: None,
            cache: None,
        });
        self.pipelines.push((key, pipeline));
        self.pipelines.len() - 1
    }

    /// A skin's stages and whether one is lit, built on first use. Past
    /// `MAX_ENT_STAGES` a new skin draws as the implicit one at slot 0.
    pub(crate) fn stages(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        fs: &Pk3Fs,
        skin: &str,
    ) -> (Rc<[EntStageDraw]>, bool) {
        if let Some(d) = self.by_skin.get(skin) {
            return (d.clone(), self.lit[skin]);
        }
        if self.next_slot == 0 {
            // slot 0: the implicit skin, the fallback
            self.write_params(queue, &implicit(""), 0, 0);
            self.next_slot = 1;
        }
        let sh = load(fs, skin, &mut self.warns);
        let mut lit = is_lit(&sh);
        let mut out = Vec::with_capacity(sh.stages.len());
        for (i, st) in sh.stages.iter().enumerate() {
            if self.next_slot == MAX_ENT_STAGES {
                static FULL: std::sync::Once = std::sync::Once::new();
                FULL.call_once(|| {
                    log::warn!(
                        "more than {MAX_ENT_STAGES} entity material stages; the rest draw plain"
                    )
                });
                out.clear();
                break;
            }
            let white = match &st.bundles[0].image {
                ImageRef::White => true,
                ImageRef::Path(p) if p.eq_ignore_ascii_case(TEXTURE_NAME) => false,
                other => {
                    self.warns.warn_once(
                        skin,
                        &format!("entity stage image {other:?} draws the skin"),
                    );
                    false
                }
            };
            let slot = self.next_slot;
            self.next_slot += 1;
            self.write_params(queue, &sh, i, slot);
            let depth_write = sh.classify_stage(i).is_some_and(|c| c.depth_write);
            let pipeline = self.pipeline(device, st.blend.clone(), depth_write);
            out.push(EntStageDraw {
                pipeline,
                white,
                slot,
            });
        }
        if out.is_empty() {
            out.push(EntStageDraw {
                pipeline: 0,
                white: false,
                slot: 0,
            });
            lit = true;
        }
        let out: Rc<[EntStageDraw]> = out.into();
        self.by_skin.insert(skin.to_string(), out.clone());
        self.lit.insert(skin.to_string(), lit);
        (out, lit)
    }

    fn write_params(&mut self, queue: &wgpu::Queue, sh: &Shader, idx: usize, slot: u32) {
        if let Some(p) = stage_params(sh, idx, 0.0) {
            queue.write_buffer(
                &self.params_buf,
                u64::from(slot) * ENT_STAGE_STRIDE,
                bytemuck::bytes_of(&p),
            );
        }
        if stage_animated(sh, idx) {
            self.animated.push((sh.clone(), idx, slot));
        }
    }
}

fn wgpu_blend_factor(f: &BlendFactor) -> wgpu::BlendFactor {
    match f {
        BlendFactor::Zero => wgpu::BlendFactor::Zero,
        BlendFactor::One => wgpu::BlendFactor::One,
        BlendFactor::DstColor => wgpu::BlendFactor::Dst,
        BlendFactor::OneMinusDstColor => wgpu::BlendFactor::OneMinusDst,
        BlendFactor::OneMinusSrcColor => wgpu::BlendFactor::OneMinusSrc,
        BlendFactor::SrcAlpha => wgpu::BlendFactor::SrcAlpha,
        BlendFactor::OneMinusSrcAlpha => wgpu::BlendFactor::OneMinusSrcAlpha,
        BlendFactor::DstAlpha => wgpu::BlendFactor::DstAlpha,
        BlendFactor::OneMinusDstAlpha => wgpu::BlendFactor::OneMinusDstAlpha,
        BlendFactor::SrcAlphaSaturate => wgpu::BlendFactor::SrcAlphaSaturated,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vcod_common::shader::{AlphaFunc, BlendFactor, WaveForm};

    const OBJECTIVE: &str = "{ { map $texturename rgbGen lightingDiffuse } \
        { map $whiteimage rgbgen constLighting ( 0.20 0.158 0.062 ) blendFunc add } \
        { map $whiteimage rgbgen constLighting ( 0.8 0.632 0.252  ) \
          alphaGen wave sin -0.1 1.1 0 .5 blendFunc GL_SRC_ALPHA GL_ONE } }";

    #[test]
    fn objective_is_a_lit_base_and_two_additive_stages() {
        let sh = from_stype("objective@x.dds", OBJECTIVE, &mut WarnSet::new()).unwrap();
        assert_eq!(sh.stages.len(), 3);
        assert_eq!(sh.stages[0].rgb_gen, RgbGen::LightingDiffuse);
        assert_eq!(sh.stages[0].blend, None);
        assert_eq!(sh.stages[1].bundles[0].image, ImageRef::White);
        assert_eq!(
            sh.stages[1].rgb_gen,
            RgbGen::ConstLighting([0.20, 0.158, 0.062])
        );
        assert_eq!(
            sh.stages[1].blend,
            Some((BlendFactor::One, BlendFactor::One))
        );
        assert_eq!(
            sh.stages[2].blend,
            Some((BlendFactor::SrcAlpha, BlendFactor::One))
        );
        let AlphaGen::Wave(w) = &sh.stages[2].alpha_gen else {
            panic!("alphaGen wave");
        };
        assert_eq!((w.form.clone(), w.base, w.amp), (WaveForm::Sin, -0.1, 1.1));
        assert!(is_lit(&sh));
    }

    #[test]
    fn unlit_types_carry_no_lit_stage() {
        let incomplete =
            "{ { map $textureName rgbgen constLighting ( 0.40 0.316 0.124 ) blendFunc add } }";
        let sh = from_stype("objective_incomplete@x", incomplete, &mut WarnSet::new()).unwrap();
        assert!(!is_lit(&sh));
        let light = "{ surfaceparm glass surfaceparm noshadow { map $texturename rgbGen identityLighting } }";
        let sh = from_stype("glass_light@x", light, &mut WarnSet::new()).unwrap();
        assert_eq!(sh.stages[0].rgb_gen, RgbGen::IdentityLighting);
        assert!(!is_lit(&sh));
        assert!(is_lit(&implicit("plain.dds")));
    }

    #[test]
    fn an_env_material_keeps_its_texture_stage_opaque() {
        let env = "{ surfaceparm metal nomipmaps \
            { map textures/sfx/environmap_1day.jpg tcgen environment rgbgen lightingdiffuse } \
            { map $texturename blendFunc GL_ONE_MINUS_SRC_ALPHA GL_SRC_ALPHA rgbgen lightingdiffuse } }";
        let sh = from_stype("metal_env@x", env, &mut WarnSet::new()).unwrap();
        assert_eq!(sh.stages.len(), 1);
        assert_eq!(sh.stages[0].blend, None);
        assert_eq!(sh.stages[0].rgb_gen, RgbGen::LightingDiffuse);
    }

    #[test]
    fn masked_types_alpha_test() {
        let masked = "{ surfaceparm cloth { map $texturename rgbGen lightingDiffuse alphaFunc GE128 depthWrite } }";
        let sh = from_stype("cloth_masked@x", masked, &mut WarnSet::new()).unwrap();
        assert_eq!(sh.stages[0].alpha_func, Some(AlphaFunc::Ge128));
        assert_eq!(implicit("x").stages[0].alpha_func, None);
    }

    /// The stock templates the snapshot entities name: pickup glow,
    /// objective pulse, the unlit objective and the lamp glass.
    #[test]
    fn stock_entity_templates_load() {
        let Some(fs) = vcod_common::testing::game_fs() else {
            return;
        };
        let mut warns = WarnSet::new();
        let pickup = load(&fs, "pickup@medpack.dds", &mut warns);
        assert_eq!(pickup.stages.len(), 2);
        assert!(matches!(pickup.stages[1].rgb_gen, RgbGen::Wave(_)));
        assert_eq!(
            pickup.stages[1].blend,
            Some((BlendFactor::One, BlendFactor::One))
        );
        assert_eq!(
            load(&fs, "objective@tnt_block.dds", &mut warns)
                .stages
                .len(),
            3
        );
        let incomplete = load(&fs, "objective_incomplete@tnt_block.dds", &mut warns);
        assert!(!is_lit(&incomplete));
        assert!(!is_lit(&load(&fs, "glass_light@bulb.dds", &mut warns)));
        let plain = load(&fs, "metal@whatever.dds", &mut warns);
        assert_eq!(plain.stages.len(), 1);
        assert!(is_lit(&plain));
    }

    #[test]
    fn skin_type_is_the_name_before_the_at() {
        assert_eq!(skin_type("Pickup@medpack.dds").as_deref(), Some("pickup"));
        assert_eq!(
            skin_type("skins/objective@a.tga").as_deref(),
            Some("objective")
        );
        assert_eq!(skin_type("plain.dds"), None);
    }
}
