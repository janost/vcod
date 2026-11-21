//! CoD4's world and static model surfaces that read the frame's copies
//! (`crate::scene_copy`, `crate::soft_particles`): the `zfeather` effect
//! sets, whose depth feather and near fade read the scene's view depth
//! (cod4-effects.md 14), and the world's distortion set
//! (`wc/office_brickglass`), which bends the colour copy (17). Both draw
//! past the frame's split, the only place the copies are this frame's:
//! a feathered stage before it draws as its plain pipeline, a distortion
//! one not at all, as retail skips a technique that needs the copy
//! without one (`R_SetupMaterial`). The pipelines are the stage
//! pipelines' layout plus group 3, `SceneCopy::world_bind_group`; they are
//! built at a world's load for the keys its batches use, once a renderer
//! ([`WorldFxKeep`]).

use super::{
    DEPTH_FORMAT, Material, StageVariant, msaa_samples, scene_pipeline, stage_variant_state,
};
use crate::renderer::gpu_util;
use std::collections::HashMap;

/// What a stage batch draws through this pass.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) enum WorldFx {
    None,
    /// A `zfeather` set (`Material::feather` on the view depth).
    Feather,
    /// A distortion set (`Material::distortion`).
    Distort,
}

/// `sh`'s draw here, for a stage of `variant`.
pub(super) fn world_fx(sh: &Material, variant: StageVariant) -> WorldFx {
    if sh.distortion.is_some() {
        return WorldFx::Distort;
    }
    let feathered = sh.feather_view_depth && sh.feather.is_some_and(|k| k > 0.0);
    match variant {
        // an opaque stage has nothing to fade; a multiply decal's shader is
        // its own
        StageVariant::Opaque | StageVariant::Multiply => WorldFx::None,
        _ if feathered => WorldFx::Feather,
        _ => WorldFx::None,
    }
}

/// A feathered variant's fragment entry: the fade on the alpha where the
/// blend reads it, on the colour too where it does not.
fn feather_entry(v: StageVariant) -> &'static str {
    match v {
        StageVariant::AdditiveInvDst => "fs_stage_premul_feather",
        StageVariant::AdditiveFixed
        | StageVariant::AdditiveFixedDepthWrite
        | StageVariant::Multiply2x => "fs_stage_feather_rgb",
        _ => "fs_stage_feather",
    }
}

type Key = (WorldFx, StageVariant, bool, bool);

pub(super) struct WorldFxPass {
    pipelines: HashMap<Key, wgpu::RenderPipeline>,
}

/// The shader, layout and every pipeline the loads' [`WorldFxPass`]es
/// made, kept by the renderer, so a map's load compiles only the keys no
/// earlier map used.
#[derive(Default)]
pub(super) struct WorldFxKeep {
    base: Option<(wgpu::ShaderModule, wgpu::PipelineLayout)>,
    pipelines: HashMap<Key, wgpu::RenderPipeline>,
}

impl WorldFxKeep {
    /// The pipelines for `keys` (fx, variant, two-sided, polygon offset),
    /// those not made yet built on every core.
    pub(super) fn pass(
        &mut self,
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        layouts: [&wgpu::BindGroupLayout; 4],
        bias: wgpu::DepthBiasState,
        keys: impl IntoIterator<Item = Key>,
    ) -> WorldFxPass {
        let (shader, layout) = self.base.get_or_insert_with(|| {
            let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("shader.wgsl (world fx)"),
                source: wgpu::ShaderSource::Wgsl(super::WORLD_WGSL.into()),
            });
            let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("world fx pipeline layout"),
                bind_group_layouts: &layouts.map(Some),
                immediate_size: 0,
            });
            (shader, layout)
        });
        let mut seen = std::collections::HashSet::new();
        let keys: Vec<Key> = keys
            .into_iter()
            .filter(|k| k.0 != WorldFx::None && seen.insert(*k))
            .collect();
        let fresh: Vec<Key> = keys
            .iter()
            .filter(|k| !self.pipelines.contains_key(k))
            .copied()
            .collect();
        let (shader, layout) = (&*shader, &*layout);
        let built = super::par_build(fresh.len(), |i| {
            make(device, shader, layout, format, bias, fresh[i])
        });
        self.pipelines.extend(fresh.into_iter().zip(built));
        WorldFxPass {
            pipelines: keys
                .into_iter()
                .map(|k| (k, self.pipelines[&k].clone()))
                .collect(),
        }
    }
}

/// The pipeline of one key.
fn make(
    device: &wgpu::Device,
    shader: &wgpu::ShaderModule,
    layout: &wgpu::PipelineLayout,
    format: wgpu::TextureFormat,
    bias: wgpu::DepthBiasState,
    (fx, variant, two_sided, want_bias): Key,
) -> wgpu::RenderPipeline {
    let (_, blend, depth_write) = stage_variant_state(variant);
    let bias = if want_bias { bias } else { Default::default() };
    let cull = (!two_sided).then_some(wgpu::Face::Back);
    let label = format!("world fx {fx:?} {variant:?} twosided={two_sided} bias={want_bias}");
    match fx {
        WorldFx::Feather => scene_pipeline(
            device,
            shader,
            format,
            &label,
            layout,
            ("vs_stage", feather_entry(variant)),
            bias,
            false,
            blend,
            depth_write,
            cull,
        ),
        _ => distort_pipeline(
            device,
            shader,
            format,
            &label,
            layout,
            (blend, depth_write),
            bias,
            cull,
        ),
    }
}

impl WorldFxPass {
    pub(super) fn pipeline(
        &self,
        fx: WorldFx,
        variant: StageVariant,
        two_sided: bool,
        bias: bool,
    ) -> Option<&wgpu::RenderPipeline> {
        self.pipelines.get(&(fx, variant, two_sided, bias))
    }
}

/// The distortion pipeline: the world vertex in buffer 0, the tangent
/// stream (`cod2::tangent_stream`) in buffer 1, as the lit pipeline reads
/// them.
fn distort_pipeline(
    device: &wgpu::Device,
    shader: &wgpu::ShaderModule,
    format: wgpu::TextureFormat,
    label: &str,
    layout: &wgpu::PipelineLayout,
    (blend, depth_write): (Option<wgpu::BlendState>, bool),
    bias: wgpu::DepthBiasState,
    cull: Option<wgpu::Face>,
) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(label),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some("vs_world_distort"),
            compilation_options: Default::default(),
            buffers: &[
                Some(gpu_util::world_vertex_layout(&gpu_util::WORLD_VERTEX_ATTRS)),
                Some(gpu_util::tangent_layout()),
            ],
        },
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some("fs_world_distort"),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend,
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
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
            count: msaa_samples(),
            mask: !0,
            alpha_to_coverage_enabled: false,
        },
        multiview_mask: None,
        cache: None,
    })
}
