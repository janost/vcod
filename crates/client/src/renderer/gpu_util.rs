//! wgpu descriptor builders the passes share. Each returns the descriptor
//! its callers used to write out by hand, field for field, so a pass that
//! moves onto one builds the same layout, sampler or pipeline.

/// A texture binding seen by the fragment stage.
pub(crate) fn texture(
    binding: u32,
    sample_type: wgpu::TextureSampleType,
    view_dimension: wgpu::TextureViewDimension,
    multisampled: bool,
) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type,
            view_dimension,
            multisampled,
        },
        count: None,
    }
}

/// A filterable float texture seen by the fragment stage.
pub(crate) fn texture_float(
    binding: u32,
    view_dimension: wgpu::TextureViewDimension,
) -> wgpu::BindGroupLayoutEntry {
    texture(
        binding,
        wgpu::TextureSampleType::Float { filterable: true },
        view_dimension,
        false,
    )
}

/// A filterable float 2D texture seen by the fragment stage: most passes'
/// image binding.
pub(crate) fn texture_2d(binding: u32) -> wgpu::BindGroupLayoutEntry {
    texture_float(binding, wgpu::TextureViewDimension::D2)
}

/// A float 2D texture read with `textureLoad`, not filtered.
pub(crate) fn unfilterable_2d(binding: u32) -> wgpu::BindGroupLayoutEntry {
    texture(
        binding,
        wgpu::TextureSampleType::Float { filterable: false },
        wgpu::TextureViewDimension::D2,
        false,
    )
}

/// A depth 2D texture seen by the fragment stage.
pub(crate) fn depth_2d(binding: u32, multisampled: bool) -> wgpu::BindGroupLayoutEntry {
    texture(
        binding,
        wgpu::TextureSampleType::Depth,
        wgpu::TextureViewDimension::D2,
        multisampled,
    )
}

/// A sampler binding seen by the fragment stage.
pub(crate) fn sampler(binding: u32, ty: wgpu::SamplerBindingType) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Sampler(ty),
        count: None,
    }
}

/// A filtering sampler binding seen by the fragment stage.
pub(crate) fn filtering_sampler(binding: u32) -> wgpu::BindGroupLayoutEntry {
    sampler(binding, wgpu::SamplerBindingType::Filtering)
}

/// A buffer binding.
pub(crate) fn buffer(
    binding: u32,
    visibility: wgpu::ShaderStages,
    ty: wgpu::BufferBindingType,
    has_dynamic_offset: bool,
    min_binding_size: Option<wgpu::BufferSize>,
) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility,
        ty: wgpu::BindingType::Buffer {
            ty,
            has_dynamic_offset,
            min_binding_size,
        },
        count: None,
    }
}

/// A uniform buffer binding with no dynamic offset and no minimum size.
pub(crate) fn uniform(binding: u32, visibility: wgpu::ShaderStages) -> wgpu::BindGroupLayoutEntry {
    buffer(
        binding,
        visibility,
        wgpu::BufferBindingType::Uniform,
        false,
        None,
    )
}

/// A uniform buffer binding of at least `size` bytes, read at a dynamic
/// offset when `dynamic`.
pub(crate) fn uniform_sized(
    binding: u32,
    visibility: wgpu::ShaderStages,
    dynamic: bool,
    size: u64,
) -> wgpu::BindGroupLayoutEntry {
    buffer(
        binding,
        visibility,
        wgpu::BufferBindingType::Uniform,
        dynamic,
        wgpu::BufferSize::new(size),
    )
}

/// A read-only storage buffer binding with no minimum size.
pub(crate) fn storage_ro(
    binding: u32,
    visibility: wgpu::ShaderStages,
) -> wgpu::BindGroupLayoutEntry {
    buffer(
        binding,
        visibility,
        wgpu::BufferBindingType::Storage { read_only: true },
        false,
        None,
    )
}

/// A linear, clamp-to-edge sampler with one mip level: what the passes
/// sample their own targets with.
pub(crate) fn linear_clamp_sampler(device: &wgpu::Device, label: Option<&str>) -> wgpu::Sampler {
    device.create_sampler(&wgpu::SamplerDescriptor {
        label,
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        ..Default::default()
    })
}

/// A pipeline with no vertex buffers whose vertex stage makes its own
/// triangle (a full-screen pass, a blit, a depth clear). `vs` and `fs`
/// come from one module. Build it with [`Fullscreen::new`], override a
/// field with struct update, then [`Fullscreen::build`].
pub(crate) struct Fullscreen<'a> {
    pub label: Option<&'a str>,
    pub layout: Option<&'a wgpu::PipelineLayout>,
    pub module: &'a wgpu::ShaderModule,
    pub vs: &'a str,
    /// None: no fragment stage.
    pub fs: Option<&'a str>,
    /// The one colour target; None: no colour attachment.
    pub target: Option<wgpu::ColorTargetState>,
    /// The fragment stage's override constants.
    pub constants: &'a [(&'a str, f64)],
    pub depth: Option<wgpu::DepthStencilState>,
    pub samples: u32,
}

impl<'a> Fullscreen<'a> {
    /// `fs` into `target`, no depth, one sample, no constants.
    pub(crate) fn new(
        label: Option<&'a str>,
        layout: Option<&'a wgpu::PipelineLayout>,
        module: &'a wgpu::ShaderModule,
        vs: &'a str,
        fs: &'a str,
        target: wgpu::ColorTargetState,
    ) -> Self {
        Fullscreen {
            label,
            layout,
            module,
            vs,
            fs: Some(fs),
            target: Some(target),
            constants: &[],
            depth: None,
            samples: 1,
        }
    }

    /// A depth-only pass: `fs` (None: no fragment stage) writes `depth`
    /// and no colour.
    pub(crate) fn depth(
        label: Option<&'a str>,
        layout: Option<&'a wgpu::PipelineLayout>,
        module: &'a wgpu::ShaderModule,
        vs: &'a str,
        fs: Option<&'a str>,
        depth: wgpu::DepthStencilState,
    ) -> Self {
        Fullscreen {
            label,
            layout,
            module,
            vs,
            fs,
            target: None,
            constants: &[],
            depth: Some(depth),
            samples: 1,
        }
    }

    pub(crate) fn build(self, device: &wgpu::Device) -> wgpu::RenderPipeline {
        let one = [self.target];
        let targets: &[Option<wgpu::ColorTargetState>] = if one[0].is_some() { &one } else { &[] };
        device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: self.label,
            layout: self.layout,
            vertex: wgpu::VertexState {
                module: self.module,
                entry_point: Some(self.vs),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: self.fs.map(|fs| wgpu::FragmentState {
                module: self.module,
                entry_point: Some(fs),
                compilation_options: wgpu::PipelineCompilationOptions {
                    constants: self.constants,
                    ..Default::default()
                },
                targets,
            }),
            primitive: Default::default(),
            depth_stencil: self.depth,
            multisample: wgpu::MultisampleState {
                count: self.samples,
                ..Default::default()
            },
            multiview_mask: None,
            cache: None,
        })
    }
}

/// A render pass into `view` alone: no resolve target, no depth, stored.
pub(crate) fn color_pass<'e>(
    encoder: &'e mut wgpu::CommandEncoder,
    label: Option<&str>,
    view: &wgpu::TextureView,
    load: wgpu::LoadOp<wgpu::Color>,
) -> wgpu::RenderPass<'e> {
    color_pass_timed(encoder, label, view, load, None)
}

/// [`color_pass`] writing GPU timestamps.
pub(crate) fn color_pass_timed<'e>(
    encoder: &'e mut wgpu::CommandEncoder,
    label: Option<&str>,
    view: &wgpu::TextureView,
    load: wgpu::LoadOp<wgpu::Color>,
    timestamp_writes: Option<wgpu::RenderPassTimestampWrites<'_>>,
) -> wgpu::RenderPass<'e> {
    encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label,
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view,
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations {
                load,
                store: wgpu::StoreOp::Store,
            },
        })],
        depth_stencil_attachment: None,
        timestamp_writes,
        occlusion_query_set: None,
        multiview_mask: None,
    })
}

// The world vertex layout below is BSP DrawVert's, field for field.
const _: () = {
    use std::mem::offset_of;
    use vcod_common::bsp::DrawVert;
    let a = WORLD_VERTEX_ATTRS;
    assert!(std::mem::size_of::<DrawVert>() == 44);
    assert!(a[0].offset == offset_of!(DrawVert, pos) as u64);
    assert!(a[1].offset == offset_of!(DrawVert, uv) as u64);
    assert!(a[2].offset == offset_of!(DrawVert, lm_uv) as u64);
    assert!(a[3].offset == offset_of!(DrawVert, normal) as u64);
    assert!(a[4].offset == offset_of!(DrawVert, color) as u64);
};

/// DrawVert's fields at shader locations 0..=4: position, uv, lightmap uv,
/// normal, colour.
pub(crate) const WORLD_VERTEX_ATTRS: [wgpu::VertexAttribute; 5] = wgpu::vertex_attr_array![
    0 => Float32x3, // pos
    1 => Float32x2, // uv
    2 => Float32x2, // lm_uv
    3 => Float32x3, // normal
    4 => Unorm8x4,  // color
];

/// The 44-byte world vertex stream, reading `attributes` of it (all of
/// [`WORLD_VERTEX_ATTRS`] or a subset).
pub(crate) const fn world_vertex_layout(
    attributes: &[wgpu::VertexAttribute],
) -> wgpu::VertexBufferLayout<'_> {
    wgpu::VertexBufferLayout {
        array_stride: 44,
        step_mode: wgpu::VertexStepMode::Vertex,
        attributes,
    }
}

/// The world's 24-byte tangent stream at shader locations 5 and 6:
/// tangent, binormal.
pub(crate) const TANGENT_ATTRS: [wgpu::VertexAttribute; 2] = wgpu::vertex_attr_array![
    5 => Float32x3, // tangent
    6 => Float32x3, // binormal
];

pub(crate) const fn tangent_layout() -> wgpu::VertexBufferLayout<'static> {
    wgpu::VertexBufferLayout {
        array_stride: 24,
        step_mode: wgpu::VertexStepMode::Vertex,
        attributes: &TANGENT_ATTRS,
    }
}
