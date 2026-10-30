//! CoD2's depth feather through the fx pass's own pipelines and the
//! scene depth copy, against `lib/z_feather.hlsl` evaluated on the CPU
//! (cod2-materials.md 9.5). Vulkan or Metal only; passes vacuously on a
//! machine with no adapter.

use super::*;
use wgpu::util::DeviceExt;

/// retail's clip z of a point `depth` units ahead: the infinite
/// perspective's `0.9995 * (depth - r_znear)` (cod2-materials.md 9.5)
fn retail_clip_z(depth: f32) -> f32 {
    f32::from_bits(0x3f7f_df40) * (depth - 4.0)
}

/// `z_feather.hlsl`'s alpha for a white, opaque sprite `sprite` units ahead
/// with the scene's float z `scene` units ahead: the vertex shader's
/// near-camera term times the pixel shader's depth-gap term.
fn hlsl_alpha(sprite: f32, scene: f32, feather: f32) -> f32 {
    let tcz = retail_clip_z(sprite) * feather;
    let near = tcz.clamp(0.0, 1.0);
    let diff = retail_clip_z(scene) * feather - tcz;
    near * diff.abs().clamp(0.0, 1.0)
}

fn headless() -> Option<(wgpu::Device, wgpu::Queue)> {
    let instance =
        wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
    let adapter = pollster::block_on(instance.request_adapter(&Default::default())).ok()?;
    let info = adapter.get_info();
    eprintln!("adapter: {} {:?}", info.name, info.backend);
    // sample_image's per-mode samplers need more than GL gives a texture
    if info.backend == wgpu::Backend::Gl {
        return None;
    }
    pollster::block_on(adapter.request_device(&Default::default())).ok()
}

/// A quad with corners `c` (in order round the edge), white, opaque.
fn quad(c: [[f32; 3]; 4], feather: f32) -> [FxVert; 4] {
    c.map(|pos| FxVert {
        pos,
        uv: [0.5, 0.5],
        rgba: [1.0; 4],
        size: 100.0,
        flags: FX_VERT_NOFOG,
        feather,
    })
}

/// A black wall `x = 100 + 0.2 y` over the bottom half of a 90 degree
/// view down +x, a feathered sprite at depth 100 over the same half (in
/// front of the wall on the left, behind it on the right) and another at
/// depth 20 over the top half with only the cleared far plane behind it.
#[test]
fn feathered_sprite_fades_into_the_wall_as_the_hlsl_does() {
    let Some((device, queue)) = headless() else {
        eprintln!("no adapter; skipped");
        return;
    };
    const N: u32 = 128;
    const FEATHER: f32 = 0.025;
    let uniform = |binding| wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    };
    let mut entries = vec![uniform(0)];
    for binding in 9..=16 {
        entries.push(wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
            count: None,
        });
    }
    let camera_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: None,
        entries: &entries,
    });
    let texture_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: None,
        entries: &[wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        }],
    });
    let depth = create_depth_view(&device, N, N);
    let msaa = create_msaa_view(&device, SCENE_FORMAT, N, N);
    let soft = crate::soft_particles::SoftParticles::new(&device, &depth, N, N);
    let copy = crate::scene_copy::SceneCopy::new(&device, SCENE_FORMAT, soft.linz(), N, N);
    let fx = create_fx_pass(
        &device,
        SCENE_FORMAT,
        &camera_layout,
        &texture_layout,
        &soft.fx_layout,
        &copy.layout,
    );

    // the Camera block: view_proj, then five vec4s; fog mode 0
    let vp = crate::camera::view_proj_from(glam::Vec3::ZERO, 0.0, 0.0, 0.0, 90.0, 1.0);
    let mut cam = [0f32; 36];
    cam[..16].copy_from_slice(&vp.to_cols_array());
    let cam_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: None,
        contents: bytemuck::cast_slice(&cam),
        usage: wgpu::BufferUsages::UNIFORM,
    });
    let sampler = device.create_sampler(&Default::default());
    let mut cam_entries = vec![wgpu::BindGroupEntry {
        binding: 0,
        resource: cam_buf.as_entire_binding(),
    }];
    for binding in 9..=16 {
        cam_entries.push(wgpu::BindGroupEntry {
            binding,
            resource: wgpu::BindingResource::Sampler(&sampler),
        });
    }
    let cam_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &camera_layout,
        entries: &cam_entries,
    });
    let white = device.create_texture_with_data(
        &queue,
        &wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        },
        Default::default(),
        &[255; 4],
    );
    let tex_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &texture_layout,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: wgpu::BindingResource::TextureView(&white.create_view(&Default::default())),
        }],
    });

    let mut wall = quad(
        [
            [60.0, -200.0, 0.0],
            [140.0, 200.0, 0.0],
            [140.0, 200.0, -200.0],
            [60.0, -200.0, -200.0],
        ],
        0.0,
    );
    for v in &mut wall {
        v.rgba = [0.0, 0.0, 0.0, 1.0];
    }
    let near = quad(
        [
            [100.0, -200.0, 0.0],
            [100.0, 200.0, 0.0],
            [100.0, 200.0, -200.0],
            [100.0, -200.0, -200.0],
        ],
        FEATHER,
    );
    let sky = quad(
        [
            [20.0, -40.0, 40.0],
            [20.0, 40.0, 40.0],
            [20.0, 40.0, 0.0],
            [20.0, -40.0, 0.0],
        ],
        FEATHER,
    );
    let verts: Vec<FxVert> = [wall, near, sky].concat();
    queue.write_buffer(&fx.vertex_buf, 0, bytemuck::cast_slice(&verts));

    let resolve = device.create_texture(&wgpu::TextureDescriptor {
        label: None,
        size: wgpu::Extent3d {
            width: N,
            height: N,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: SCENE_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let resolve_view = resolve.create_view(&Default::default());
    let mut enc = device.create_command_encoder(&Default::default());
    let begin = |enc: &mut wgpu::CommandEncoder, first: bool| {
        let mut pass = enc
            .begin_render_pass(&wgpu::RenderPassDescriptor {
                label: None,
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &msaa,
                    depth_slice: None,
                    resolve_target: (!first).then_some(&resolve_view),
                    ops: wgpu::Operations {
                        load: if first {
                            wgpu::LoadOp::Clear(wgpu::Color::BLACK)
                        } else {
                            wgpu::LoadOp::Load
                        },
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &depth,
                    depth_ops: Some(wgpu::Operations {
                        load: if first {
                            wgpu::LoadOp::Clear(1.0)
                        } else {
                            wgpu::LoadOp::Load
                        },
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            })
            .forget_lifetime();
        pass.set_bind_group(0, &cam_bg, &[]);
        pass.set_bind_group(1, &tex_bg, &[]);
        pass.set_vertex_buffer(0, fx.vertex_buf.slice(..));
        pass.set_index_buffer(fx.index_buf.slice(..), wgpu::IndexFormat::Uint32);
        pass
    };
    // the opaque scene, then the split's copy, then the feathered run
    {
        let mut pass = begin(&mut enc, true);
        pass.set_pipeline(&fx.pipelines[FxBlend::Opaque as usize]);
        pass.draw_indexed(0..6, 0, 0..1);
    }
    soft.encode(&mut enc);
    {
        let mut pass = begin(&mut enc, false);
        pass.set_bind_group(2, soft.fx_bind_group(), &[]);
        pass.set_pipeline(&fx.soft_pipelines[FxBlend::Alpha as usize]);
        pass.draw_indexed(6..18, 0, 0..1);
    }
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: u64::from(N * N * 4),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    enc.copy_texture_to_buffer(
        resolve.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &readback,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(N * 4),
                rows_per_image: None,
            },
        },
        wgpu::Extent3d {
            width: N,
            height: N,
            depth_or_array_layers: 1,
        },
    );
    queue.submit([enc.finish()]);
    readback.slice(..).map_async(wgpu::MapMode::Read, |_| {});
    device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    let px = readback.slice(..).get_mapped_range().unwrap().to_vec();

    // the copy reads MSAA sample 0, Vulkan's standard 4x position
    // (0.375, 0.125) in the pixel; the sprite's own depth is flat
    let ndc = |p: u32, off: f32| (p as f32 + off) / (N as f32 / 2.0) - 1.0;
    let (mut checked, mut max_err, mut faded) = (0, 0.0f32, 0);
    for y in 0..N {
        for x in 0..N {
            let up = -ndc(y, 0.5);
            let right = ndc(x, 0.5);
            // keep off the halves' seam and the wall's crossing
            if up.abs() < 0.05 || right.abs() < 0.05 {
                continue;
            }
            let want = if up > 0.0 {
                hlsl_alpha(20.0, crate::camera::Z_FAR, FEATHER)
            } else {
                // the view ray (1, -right, up) meets x = 100 + 0.2 y at t
                let t = 100.0 / (1.0 + 0.2 * ndc(x, 0.375));
                if t < 100.0 {
                    0.0
                } else {
                    hlsl_alpha(100.0, t, FEATHER)
                }
            };
            let got = f32::from(px[((y * N + x) * 4) as usize]) / 255.0;
            let err = (got - want).abs();
            assert!(
                err <= 2.5 / 255.0,
                "pixel ({x}, {y}): {got} vs the hlsl's {want}"
            );
            max_err = max_err.max(err);
            checked += 1;
            if want > 0.0 && want < 0.99 {
                faded += 1;
            }
        }
    }
    eprintln!("{checked} pixels, {faded} part faded, max error {max_err}");
    // the near term alone: 0.9995 * 16 * 0.025
    assert!((hlsl_alpha(20.0, 1e6, FEATHER) - 0.39980).abs() < 1e-4);
    assert!(faded > 1000);
}
