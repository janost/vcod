//! The 2D layer: the HUD and debug text passes, HUD quad upload and HUD texture
//! resolution as retail registers 2D images.

use super::*;

/// Debug-HUD glyph budget per frame; lines past it are cut.
pub(super) const MAX_HUD_GLYPHS: usize = 1024;

pub(super) const MAX_HUD_QUADS: usize = 16384;

/// Debug-HUD text over an embedded font atlas; drawn last, ignoring depth.
pub(super) struct HudTextPass {
    pub(super) pipeline: wgpu::RenderPipeline,
    pub(super) pipeline_1x: wgpu::RenderPipeline,
    pub(super) vertex_buf: wgpu::Buffer,
    pub(super) index_buf: wgpu::Buffer,
    pub(super) bind_group: wgpu::BindGroup,
}

/// On-screen HUD (chat, killfeed, scoreboard, status). Same lazy texture
/// cache and run pattern as [`FxPass`]; straight alpha, plus additive for
/// [`hud::ADDITIVE`] quads; no depth test.
pub(super) struct HudPass {
    pub(super) pipeline: wgpu::RenderPipeline,
    pub(super) pipeline_1x: wgpu::RenderPipeline,
    /// The same blending `SrcAlpha One`, for [`hud::ADDITIVE`] quads.
    pub(super) additive: wgpu::RenderPipeline,
    pub(super) additive_1x: wgpu::RenderPipeline,
    pub(super) vertex_buf: wgpu::Buffer,
    pub(super) index_buf: wgpu::Buffer,
    pub(super) texture_layout: wgpu::BindGroupLayout,
    /// image name -> texture; `None` = failed to resolve, warned already.
    pub(super) textures: HashMap<String, Option<HudTexture>>,
    /// Implicit images the cgame registers mipmapped ([`hud_mip_names`]);
    /// empty without a gamestate, where no cgame runs.
    pub(super) mip_names: HashSet<String>,
    /// (texture, quad count, additive) runs in draw order.
    pub(super) runs: Vec<(String, usize, bool)>,
    /// For the F3 overlay.
    pub(super) quad_count: usize,
}

impl HudPass {
    /// The runs into `pass`, the single-sample pipelines when `single`.
    /// Returns the draws.
    pub(super) fn draw(&self, pass: &mut wgpu::RenderPass<'_>, single: bool) -> usize {
        let mut draws = 0;
        pass.set_vertex_buffer(0, self.vertex_buf.slice(..));
        pass.set_index_buffer(self.index_buf.slice(..), wgpu::IndexFormat::Uint16);
        let mut first = 0usize;
        let mut bound = None;
        for (texture, count, add) in &self.runs {
            if let Some(Some(tex)) = self.textures.get(texture) {
                if bound != Some(*add) {
                    pass.set_pipeline(match (*add, single) {
                        (false, false) => &self.pipeline,
                        (false, true) => &self.pipeline_1x,
                        (true, false) => &self.additive,
                        (true, true) => &self.additive_1x,
                    });
                    bound = Some(*add);
                }
                pass.set_bind_group(0, &tex.bg, &[]);
                pass.draw_indexed((first * 6) as u32..((first + count) * 6) as u32, 0, 0..1);
                draws += 1;
            }
            first += count;
        }
        draws
    }
}

/// A HUD image and its bind group, which holds `r_textureMode`'s sampler
/// for the image's address mode and is rebuilt when the mode changes.
pub(super) struct HudTexture {
    pub(super) view: wgpu::TextureView,
    pub(super) clamp: bool,
    pub(super) bg: wgpu::BindGroup,
    /// Retail's default shader, which draws whatever the quad's colour.
    pub(super) default: bool,
}

impl Renderer {
    /// Quads in draw order, pixel coords with origin top-left. Textures
    /// resolve lazily by name; an unresolvable one warns once and is cached
    /// as `None`.
    pub fn set_hud_quads(&mut self, fs: &dyn AssetSource, quads: Vec<HudQuad>) {
        let up = HudUpload {
            device: &self.device,
            queue: &self.queue,
            samplers: &self.tex_samplers,
            image_map: self.shaders.image_map(),
            lib: &self.shader_lib,
            textures: &self.textures,
            image_gamma: self.image_gamma.as_ref(),
            identity_light: self.identity_light,
            size: (self.config.width as f32, self.config.height as f32),
        };
        up.upload(
            &mut self.hud_pass,
            fs,
            &quads,
            &mut self.hud_quad_cap_warned,
        );
    }

    /// Quads drawn last frame, for the F3 overlay.
    pub fn hud_quad_count(&self) -> usize {
        self.hud_pass.quad_count
    }

    /// The weapons a gamestate's cgame registers, configstring 7's table
    /// (`None` without a gamestate): their icons go up mipmapped. A changed
    /// set reloads the HUD images.
    pub fn set_hud_weapons(&mut self, weapons: Option<&[Option<vcod_common::weapon::WeaponDef>]>) {
        let names = weapons.map(hud_mip_names).unwrap_or_default();
        if names != self.hud_pass.mip_names {
            self.hud_pass.textures.clear();
            self.hud_pass.mip_names = names;
        }
    }
}

/// A HUD colour is a display value; `scale` (`colour_scale`) takes it into
/// the framebuffer, and the ramp's overbright doubling brings a lit gen back
/// on display. Alpha is a coverage weight.
pub(super) fn hud_colour([r, g, b, a]: [f32; 4], scale: f32) -> [f32; 4] {
    [r * scale, g * scale, b * scale, a]
}

pub(super) fn create_hud_text_pass(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    format: wgpu::TextureFormat,
) -> HudTextPass {
    // Nearest sampling: glyphs draw at integer scales, filtering would blur them.
    let atlas = hud_text::build_atlas();
    let size = wgpu::Extent3d {
        width: hud_text::ATLAS_W as u32,
        height: hud_text::ATLAS_H as u32,
        depth_or_array_layers: 1,
    };
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("hud font atlas"),
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::R8Unorm,
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
        &atlas,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(hud_text::ATLAS_W as u32),
            rows_per_image: None,
        },
        size,
    );
    let view = texture.create_view(&Default::default());
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("hud font sampler"),
        address_mode_u: wgpu::AddressMode::ClampToEdge,
        address_mode_v: wgpu::AddressMode::ClampToEdge,
        mag_filter: wgpu::FilterMode::Nearest,
        min_filter: wgpu::FilterMode::Nearest,
        ..Default::default()
    });

    let bg_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("hud text layout"),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
        ],
    });
    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("hud text"),
        layout: &bg_layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(&sampler),
            },
        ],
    });

    // Two quads per glyph (shadow + text); quad n spans vertices 4n..4n+4.
    const MAX_QUADS: usize = MAX_HUD_GLYPHS * 2;
    let vertex_buf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("hud text vertices"),
        size: (MAX_QUADS * 4 * std::mem::size_of::<HudVert>()) as u64,
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let indices: Vec<u16> = (0..MAX_QUADS)
        .flat_map(|q| {
            let b = (q * 4) as u16;
            [b, b + 1, b + 2, b, b + 2, b + 3]
        })
        .collect();
    let index_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("hud text indices"),
        contents: bytemuck::cast_slice(&indices),
        usage: wgpu::BufferUsages::INDEX,
    });

    let shader = device.create_shader_module(wgpu::include_wgsl!("../hud_text.wgsl"));
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("hud text pipeline layout"),
        bind_group_layouts: &[Some(&bg_layout)],
        immediate_size: 0,
    });
    // The map pass (MSAA or not) carries the depth target; the pass after
    // the bloom draws single-sample with no depth into the resolved scene.
    let make = |samples: u32, depth: bool| {
        device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("hud text pipeline"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<HudVert>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![
                        0 => Float32x2, // pos (clip space)
                        1 => Float32x2, // uv
                        2 => Float32x4, // color
                    ],
                })],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: depth.then(|| wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                // depth ignored, draws over everything
                depth_write_enabled: Some(false),
                depth_compare: Some(wgpu::CompareFunction::Always),
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
    let (pipeline, pipeline_1x) = (make(msaa_samples(), true), make(1, false));

    HudTextPass {
        pipeline,
        pipeline_1x,
        vertex_buf,
        index_buf,
        bind_group,
    }
}

/// What [`Renderer::set_hud_quads`] resolves and uploads HUD quads with,
/// apart so an offscreen test can draw a frame through the same HUD pass.
pub(super) struct HudUpload<'a> {
    pub(super) device: &'a wgpu::Device,
    pub(super) queue: &'a wgpu::Queue,
    pub(super) samplers: &'a TexSamplers,
    pub(super) image_map: &'a HashMap<String, String>,
    pub(super) lib: &'a MaterialLib,
    pub(super) textures: &'a texture_settings::TextureSettings,
    pub(super) image_gamma: Option<&'a [u8; 256]>,
    pub(super) identity_light: f32,
    /// The target's size in px.
    pub(super) size: (f32, f32),
}

impl HudUpload<'_> {
    /// Resolves each quad's texture into `pass`'s cache, writes the vertex
    /// buffer and the draw runs. Past [`MAX_HUD_QUADS`] the rest are dropped
    /// with one warning, `cap_warned` keeping it.
    pub(super) fn upload(
        &self,
        pass: &mut HudPass,
        fs: &dyn AssetSource,
        quads: &[HudQuad],
        cap_warned: &mut bool,
    ) {
        let quads = if quads.len() > MAX_HUD_QUADS {
            if !*cap_warned {
                *cap_warned = true;
                log::warn!(
                    "hud: {} quads exceeds the {MAX_HUD_QUADS} cap, truncating",
                    quads.len()
                );
            }
            &quads[..MAX_HUD_QUADS]
        } else {
            quads
        };

        let (w, h) = self.size;
        let mut verts: Vec<HudVert> = Vec::with_capacity(quads.len() * 4);
        let mut runs: Vec<(String, usize, bool)> = Vec::new();
        // The texture the last quad named and what it resolved to: a run of
        // glyphs on one font page asks the same of every quad.
        let mut last: Option<(&str, bool, f32)> = None;
        for quad in quads {
            let (key, add) = match quad.texture.strip_prefix(hud::ADDITIVE) {
                Some(k) => (k, true),
                None => (quad.texture.as_str(), false),
            };
            let (default, scale) = match last {
                Some((t, default, scale)) if t == quad.texture => (default, scale),
                _ => {
                    // A default-if-missing quad that resolves draws as its own name.
                    let name = default_shader_fallback(key).unwrap_or(key);
                    if !pass.textures.contains_key(key) {
                        let how = hud_image(name, self.lib.get(name), &pass.mip_names);
                        let tex = resolve_hud_texture(
                            self.device,
                            self.queue,
                            &pass.texture_layout,
                            self.samplers,
                            fs,
                            self.image_map,
                            cod2_hud_image(fs, self.lib, name),
                            key,
                            how,
                            self.textures.world_picmip(!how.picmip),
                            self.image_gamma,
                            self.textures.store(),
                        );
                        pass.textures.insert(key.to_string(), tex);
                    }
                    let default = matches!(
                        pass.textures.get(key),
                        Some(Some(t)) if t.default
                    );
                    let scale = colour_scale(self.lib.get(name), self.identity_light);
                    last = Some((quad.texture.as_str(), default, scale));
                    (default, scale)
                }
            };

            // `black` shares `white`'s 1x1 texture; force the rgb here so it
            // draws black regardless of the element colour, alpha untouched.
            let rgba = if key == "black" {
                [0.0, 0.0, 0.0, quad.rgba[3]]
            } else if default {
                [1.0; 4]
            } else {
                hud_colour(quad.rgba, scale)
            };

            for i in 0..4 {
                let [px, py] = quad.verts[i];
                verts.push(HudVert {
                    // pixel coords, origin top-left -> clip space
                    pos: [px / w * 2.0 - 1.0, 1.0 - py / h * 2.0],
                    uv: quad.uvs[i],
                    color: rgba,
                });
            }
            match runs.last_mut() {
                Some((name, count, a)) if name == key && *a == add => *count += 1,
                _ => runs.push((key.to_string(), 1, add)),
            }
        }

        self.queue
            .write_buffer(&pass.vertex_buf, 0, bytemuck::cast_slice(&verts));
        pass.quad_count = quads.len();
        pass.runs = runs;
    }
}

/// Like `create_hud_text_pass` but with `hud.wgsl`, which samples an image
/// with mipmaps through `r_textureMode`'s sampler and one without through
/// `GL_LINEAR` ([`HudTexture`]).
pub(super) fn create_hud_pass(device: &wgpu::Device, format: wgpu::TextureFormat) -> HudPass {
    let texture_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("hud texture layout"),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 2,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
        ],
    });

    // Quad n spans vertices 4n..4n+4.
    let vertex_buf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("hud vertices"),
        size: (MAX_HUD_QUADS * 4 * std::mem::size_of::<HudVert>()) as u64,
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let indices: Vec<u16> = (0..MAX_HUD_QUADS)
        .flat_map(|q| {
            let b = (q * 4) as u16;
            [b, b + 1, b + 2, b, b + 2, b + 3]
        })
        .collect();
    let index_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("hud indices"),
        contents: bytemuck::cast_slice(&indices),
        usage: wgpu::BufferUsages::INDEX,
    });

    let shader = device.create_shader_module(wgpu::include_wgsl!("../hud.wgsl"));
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("hud pipeline layout"),
        bind_group_layouts: &[Some(&texture_layout)],
        immediate_size: 0,
    });
    // The map pass (MSAA or not) carries the depth target; the pass after
    // the bloom draws single-sample with no depth into the resolved scene.
    let make = |samples: u32, depth: bool, blend: wgpu::BlendState| {
        device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("hud pipeline"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<HudVert>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![
                        0 => Float32x2, // pos (clip space)
                        1 => Float32x2, // uv
                        2 => Float32x4, // color
                    ],
                })],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(blend),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: depth.then(|| wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                // depth ignored, draws over everything
                depth_write_enabled: Some(false),
                depth_compare: Some(wgpu::CompareFunction::Always),
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
    let alpha = wgpu::BlendState::ALPHA_BLENDING;
    let add = wgpu::BlendComponent {
        src_factor: wgpu::BlendFactor::SrcAlpha,
        dst_factor: wgpu::BlendFactor::One,
        operation: wgpu::BlendOperation::Add,
    };
    let add = wgpu::BlendState {
        color: add,
        alpha: add,
    };
    let (pipeline, pipeline_1x) = (make(msaa_samples(), true, alpha), make(1, false, alpha));
    let (additive, additive_1x) = (make(msaa_samples(), true, add), make(1, false, add));

    HudPass {
        pipeline,
        pipeline_1x,
        additive,
        additive_1x,
        vertex_buf,
        index_buf,
        texture_layout,
        textures: HashMap::new(),
        mip_names: HashSet::new(),
        runs: Vec::new(),
        quad_count: 0,
    }
}

/// Where a HUD texture name resolves. `white` and `$whiteimage` are the
/// engine built-in; `black` (pak4 `scripts/hud.shader`: `map $whiteimage`,
/// `rgbGen const 0`) shares the same texture, and `set_hud_quads` forces the
/// quad colour to black for it.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum HudTex {
    White,
    Black,
    /// [`crate::hud::DEFAULT_SHADER`], or a [`crate::hud::default_if_missing`]
    /// name with no image.
    Default,
    Path(String),
    Missing,
}

/// A material name (`hudStanceStand`, `hintHealth`) resolves through the
/// shader-script image map, same as fx; anything else is a bare pk3 path.
/// `ShaderLib::load` folds every material name to lowercase, so the lookup
/// does too before falling back to the name as given.
pub(super) fn hud_texture_path(
    name: &str,
    image_map: &HashMap<String, String>,
    fs: &dyn AssetSource,
) -> HudTex {
    match name {
        "white" | "$whiteimage" => return HudTex::White,
        "black" => return HudTex::Black,
        hud::DEFAULT_SHADER => return HudTex::Default,
        _ => {}
    }
    if let Some(rest) = default_shader_fallback(name) {
        return match hud_texture_path(rest, image_map, fs) {
            HudTex::Missing => HudTex::Default,
            found => found,
        };
    }
    let path = image_map
        .get(name)
        .or_else(|| image_map.get(&name.to_ascii_lowercase()))
        .map(String::as_str)
        .unwrap_or(name);
    match resolve_fx_path(&HashMap::new(), fs, path) {
        Some(p) => HudTex::Path(p),
        None => HudTex::Missing,
    }
}

/// A CoD2 HUD name that is a material (`fonts/gamefonts`): its colour map,
/// ahead of any loose image of the same name.
pub(super) fn cod2_hud_image<'a>(
    fs: &dyn AssetSource,
    lib: &'a MaterialLib,
    name: &str,
) -> Option<&'a str> {
    if fs.game() != vcod_common::game::Game::Cod2 {
        return None;
    }
    match lib.get(name)?.texture(TexSemantic::Color)? {
        ImageRef::Path(p) => Some(p),
        _ => None,
    }
}

/// `img` with every texel's alpha at 255.
pub(super) fn opaque(mut img: Image) -> Image {
    if let ImageData::Rgba8(px) = &mut img.data {
        px.as_chunks_mut::<4>()
            .0
            .iter_mut()
            .for_each(|t| t[3] = 255);
    }
    img
}

/// The name a [`crate::hud::default_if_missing`] quad asks for.
pub(super) fn default_shader_fallback(name: &str) -> Option<&str> {
    name.strip_prefix(hud::DEFAULT_SHADER)?.strip_prefix(':')
}

/// How retail uploads a HUD or menu image: flag 1 (mipmapped), flag 2
/// (`r_picmip`) and clamped (flags 0x10 and 0x20,
/// docs/research/cod11-front-end.md, section 14).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct HudImage {
    pub(super) mip: bool,
    pub(super) picmip: bool,
    pub(super) clamp: bool,
}

/// A scripted shader's stage `map` sets the flags from its keywords whatever
/// registered it; an implicit one takes flags 3 (mipmapped, picmipped,
/// repeating) from `RE_RegisterShader` and 0x30 (clamped, neither) from
/// `RE_RegisterShaderNoMip`, and of the cgame's 2D images only those in
/// `mip_names` go through the former.
pub(super) fn hud_image(
    name: &str,
    sh: Option<&Material>,
    mip_names: &HashSet<String>,
) -> HudImage {
    if let Some(sh) = sh.filter(|sh| !sh.stages.is_empty()) {
        let clamp = sh
            .stages
            .iter()
            .flat_map(|st| st.bundles.first())
            .next()
            .is_some_and(|b| b.clamp.any());
        return HudImage {
            mip: !sh.nomipmaps,
            picmip: !sh.nopicmip,
            clamp,
        };
    }
    let mip = mip_names.contains(&hud_image_key(name));
    HudImage {
        mip,
        picmip: mip,
        clamp: !mip,
    }
}

/// A 2D image name as `hud_mip_names` stores it: lowercase, no extension.
pub(super) fn hud_image_key(name: &str) -> String {
    let lower = name.to_ascii_lowercase().replace('\\', "/");
    match lower.rsplit_once('.') {
        Some((stem, "tga" | "jpg" | "dds")) => stem.to_string(),
        _ => lower,
    }
}

/// The implicit 2D images the cgame registers through trap 0x30
/// (`RE_RegisterShader`, mipmapped) rather than 0x58 (`NoMip`): the four
/// icons of each weapon in `weapons`, configstring 7's table
/// (`CG_RegisterWeapon`, cgame 0x30034cf0), and the fixed names of
/// `CG_RegisterGraphics` (0x30020da0).
pub(super) fn hud_mip_names(weapons: &[Option<vcod_common::weapon::WeaponDef>]) -> HashSet<String> {
    const GRAPHICS: [&str; 5] = [
        "gfx/2d/net",
        "gfx/2d/backtile",
        "gfx/2d/select",
        "gfx/hud/hud@objective_friendly",
        "gfx/hud/hud@objective_friendly_chat",
    ];
    let mut names: HashSet<String> = GRAPHICS.iter().map(|n| n.to_string()).collect();
    for def in weapons.iter().flatten() {
        for icon in [
            &def.hud_icon,
            &def.kill_icon,
            &def.mode_icon,
            &def.ammo_icon,
        ]
        .into_iter()
        .flatten()
        {
            names.insert(hud_image_key(icon));
        }
    }
    names
}

/// The bind group for a HUD texture: the image, `r_textureMode`'s sampler
/// and the `GL_LINEAR` one, both of the image's address mode.
pub(super) fn hud_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    tex: &TexSamplers,
    view: &wgpu::TextureView,
    clamp: bool,
) -> wgpu::BindGroup {
    let (mode, linear) = if clamp {
        (&tex.clamp, &tex.linear_clamp)
    } else {
        (&tex.repeat, &tex.linear)
    };
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("hud texture"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(mode),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::Sampler(linear),
            },
        ],
    })
}

/// A HUD texture by name ([`hud_texture_path`]), uploaded as `how` says
/// with `picmip` levels dropped, `gamma` baked as retail's upload bakes it
/// and stored as `store` says. Unresolvable names warn once; the caller
/// caches the `None`.
#[expect(clippy::too_many_arguments)]
pub(super) fn resolve_hud_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    texture_layout: &wgpu::BindGroupLayout,
    tex: &TexSamplers,
    fs: &dyn AssetSource,
    image_map: &HashMap<String, String>,
    material_image: Option<&str>,
    name: &str,
    how: HudImage,
    picmip: u32,
    gamma: Option<&[u8; 256]>,
    store: texture_settings::Store,
) -> Option<HudTexture> {
    let found = match (hud_texture_path(name, image_map, fs), material_image) {
        // A CoD2 material's colour map wins, a default-if-missing name's too.
        (HudTex::Path(_) | HudTex::Missing | HudTex::Default, Some(p)) => {
            HudTex::Path(p.to_string())
        }
        (found, _) => found,
    };
    let default = found == HudTex::Default;
    let (mut img, path) = match found {
        HudTex::White | HudTex::Black => (assets::white_1x1(), None),
        // The default shader's one stage blends nothing (0x4fd780), so its
        // image's alpha of 32 never shows.
        HudTex::Default => (opaque(assets::default_image()), None),
        HudTex::Path(path) => (assets::load_path_image(fs, &path), Some(path)),
        HudTex::Missing => {
            log::warn!("hud: no texture found for {name:?}, dropping its quads");
            return None;
        }
    };
    let load = texture_settings::path_levels(fs);
    let level_name = path.as_deref().unwrap_or(name);
    let mips = if how.mip {
        texture_settings::Mips::chain(level_name, &load)
    } else {
        texture_settings::Mips::single(level_name, &load)
    };
    let levels = texture_settings::prepare(&mut img, picmip, gamma, store, mips);
    let view = upload_levels(device, queue, name, &img, &levels);
    let bg = hud_bind_group(device, texture_layout, tex, &view, how.clamp);
    Some(HudTexture {
        view,
        clamp: how.clamp,
        bg,
        default,
    })
}
