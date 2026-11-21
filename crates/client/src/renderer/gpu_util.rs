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
