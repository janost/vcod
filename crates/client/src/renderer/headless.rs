//! A `Renderer` with no window, for the tests and the headless probes that
//! pose bodies on the live draw path.

use std::sync::Mutex;
use vcod_common::asset_source::AssetSource;

use super::*;

/// The validation errors a device raised, in order.
pub(crate) type Errors = Arc<Mutex<Vec<String>>>;

/// Set, a headless test that finds no adapter it can draw on fails instead
/// of skipping: a run on lavapipe sets it (docs/dev/testing.md).
pub(crate) const REQUIRE_GPU: &str = "VCOD_REQUIRE_GPU";

/// What a headless test asks of its adapter.
#[derive(Clone, Copy, PartialEq)]
pub(crate) enum Need {
    /// Any adapter, GL included.
    Any,
    /// Not GL: the app runs on Vulkan, DX12 or Metal, and GL lacks depth
    /// loads and binds a texture to one sampler.
    #[cfg(test)]
    NotGl,
    /// Not GL, with BC textures (every stock colour map).
    Bc,
}

/// The adapter a headless test or probe draws on, `None` when the machine
/// has none that meets `need`, which [`REQUIRE_GPU`] turns into a panic.
pub(crate) fn test_adapter(need: Need) -> Option<wgpu::Adapter> {
    let instance =
        wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
    let why = match pollster::block_on(instance.request_adapter(&Default::default())) {
        Err(_) => "no adapter".to_string(),
        Ok(a) => {
            let info = a.get_info();
            if need != Need::Any && info.backend == wgpu::Backend::Gl {
                format!("{} is GL", info.name)
            } else if need == Need::Bc
                && !a
                    .features()
                    .contains(wgpu::Features::TEXTURE_COMPRESSION_BC)
            {
                format!("{} has no BC textures", info.name)
            } else {
                eprintln!("adapter: {} {:?}", info.name, info.backend);
                return Some(a);
            }
        }
    };
    assert!(
        std::env::var_os(REQUIRE_GPU).is_none(),
        "{why}, and {REQUIRE_GPU} is set"
    );
    eprintln!("{why}: skipped");
    None
}

/// A device off [`test_adapter`] with BC textures when `need` asks for them.
#[cfg(any(test, feature = "testing"))]
pub(crate) fn test_device(need: Need) -> Option<(wgpu::Device, wgpu::Queue)> {
    let adapter = test_adapter(need)?;
    let required_features = match need {
        Need::Bc => wgpu::Features::TEXTURE_COMPRESSION_BC,
        _ => wgpu::Features::empty(),
    };
    let desc = wgpu::DeviceDescriptor {
        required_features,
        ..Default::default()
    };
    pollster::block_on(adapter.request_device(&desc)).ok()
}

impl Renderer {
    /// [`Renderer::new`] without a window: the same device features, limits
    /// and build, drawing into an offscreen target of the swapchain's format,
    /// `width` by `height` (each row a multiple of 64 pixels for a test's
    /// readback). `None` without an adapter that has BC textures. The
    /// headless probes pose their bodies through it, as the tests do.
    pub(crate) fn headless_sized(
        fs: &dyn AssetSource,
        hdr: bool,
        samples: u32,
        (width, height): (u32, u32),
    ) -> Option<(Renderer, Errors)> {
        let adapter = test_adapter(Need::Bc)?;
        let timestamps = adapter.features().contains(wgpu::Features::TIMESTAMP_QUERY);
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("pipeline check device"),
            required_features: wgpu::Features::TEXTURE_COMPRESSION_BC
                | (adapter.features() & wgpu::Features::TIMESTAMP_QUERY)
                | (adapter.features() & crate::frame_prof::frag_features()),
            required_limits: super::device_limits(&adapter),
            ..Default::default()
        }))
        .ok()?;
        let errors = Errors::default();
        let sink = errors.clone();
        device.on_uncaptured_error(Arc::new(move |e: wgpu::Error| {
            sink.lock().unwrap().push(e.to_string());
        }));
        // the scene's format, as a window takes where its surface allows,
        // so an identity gamma ramp skips the gamma pass here too
        let format = super::SCENE_FORMAT;
        let target = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("pipeline check target"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            color_space: wgpu::SurfaceColorSpace::Auto,
            width,
            height,
            present_mode: wgpu::PresentMode::AutoVsync,
            alpha_mode: wgpu::CompositeAlphaMode::Auto,
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };
        let r = Renderer::build(
            device,
            queue,
            Target::Offscreen(target),
            config,
            timestamps,
            fs,
            hdr,
            TextureSettings::default(),
            samples,
        );
        Some((r, errors))
    }

    /// The offscreen target's last frame: `(width, height, BGRA bytes)`.
    #[cfg(any(test, feature = "testing"))]
    pub(crate) fn read_offscreen(&self) -> Option<(u32, u32, Vec<u8>)> {
        let Target::Offscreen(t) = &self.target else {
            return None;
        };
        let (w, h) = (t.width(), t.height());
        let buf = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("offscreen readback"),
            size: u64::from(w * h * 4),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut enc = self.device.create_command_encoder(&Default::default());
        enc.copy_texture_to_buffer(
            t.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &buf,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(w * 4),
                    rows_per_image: Some(h),
                },
            },
            t.size(),
        );
        self.queue.submit([enc.finish()]);
        buf.slice(..).map_async(wgpu::MapMode::Read, |_| {});
        self.device.poll(wgpu::PollType::wait_indefinitely()).ok()?;
        let mut bytes = buf.slice(..).get_mapped_range().ok()?.to_vec();
        // RGBA to BGRA; alpha as an opaque swapchain shows it, whatever the
        // 2D's blends wrote there when no gamma pass set it to 1
        for px in bytes.as_chunks_mut::<4>().0 {
            *px = [px[2], px[1], px[0], 255];
        }
        Some((w, h, bytes))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A new material lib drops the mark shaders the old one resolved, so
    /// a decal an earlier level missed resolves on the next (BUG-124).
    /// Skips without an adapter.
    #[test]
    fn a_new_material_lib_drops_the_mark_shaders() {
        let fs = vcod_common::pk3::Pk3Fs::empty();
        let Some((mut r, _)) = Renderer::headless_sized(&fs, false, 1, (64, 64)) else {
            return;
        };
        r.prewarm_marks(&fs, ["gfx/impact/bullethit_dirt"]);
        assert_eq!(r.mark_shaders(), 1);
        r.set_material_lib(vcod_common::material::MaterialLib::default());
        assert_eq!(r.mark_shaders(), 0);
    }
}
