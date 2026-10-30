//! The real `Renderer` built headless and one frame drawn, for a CoD1 map
//! (`COD_DIR`), CoD2's mp_toujane (`COD2_DIR`) and CoD4's mp_crash
//! (`COD4_DIR`): every pipeline, layout
//! and bind group the app makes, and every draw's bindings, go through
//! wgpu's validation with its errors collected instead of panicking. A
//! layout that hides a binding from a stage its shader reads it in fails
//! here, not on the player's first frame. Skips without a non-GL adapter
//! with BC textures (lavapipe will do: `VK_DRIVER_FILES`,
//! `WGPU_BACKEND=vulkan`, `GALLIVM_PERF=nopt`).

use std::sync::Mutex;
use vcod_common::asset_source::AssetSource;

use glam::Vec3;

use super::*;
use crate::fx::sim::FxQuad;

/// The validation errors a device raised, in order.
pub(crate) type Errors = Arc<Mutex<Vec<String>>>;

const SIZE: (u32, u32) = (320, 240);

/// One map-loading live session at a time in the test binary. Each holds
/// a server's and a client's map, a lavapipe device and its frames; three
/// or four side by side ran past the build scope's 10 GB and stalled in
/// reclaim. Take it before the server loads; a session that panicked
/// leaves it free.
pub(crate) fn live_session_slot() -> std::sync::MutexGuard<'static, ()> {
    static SLOT: Mutex<()> = Mutex::new(());
    SLOT.lock().unwrap_or_else(|e| e.into_inner())
}

impl Renderer {
    /// [`Renderer::new`] without a window: the same device features, limits
    /// and build, drawing into an offscreen target of the swapchain's
    /// format. `None` without an adapter that has BC textures.
    fn headless(fs: &dyn AssetSource, hdr: bool, samples: u32) -> Option<(Renderer, Errors)> {
        Self::headless_sized(fs, hdr, samples, SIZE)
    }

    /// [`Renderer::headless`] at `width` by `height` (each row a multiple
    /// of 64 pixels, for [`Renderer::read_offscreen`]).
    pub(crate) fn headless_sized(
        fs: &dyn AssetSource,
        hdr: bool,
        samples: u32,
        (width, height): (u32, u32),
    ) -> Option<(Renderer, Errors)> {
        let instance =
            wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
        let adapter = pollster::block_on(instance.request_adapter(&Default::default())).ok()?;
        let info = adapter.get_info();
        eprintln!("adapter: {} {:?}", info.name, info.backend);
        // the app runs on Vulkan, DX12 or Metal; GL lacks depth loads (SSAO)
        if info.backend == wgpu::Backend::Gl {
            return None;
        }
        if !adapter
            .features()
            .contains(wgpu::Features::TEXTURE_COMPRESSION_BC)
        {
            return None;
        }
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

    /// The feather's `floatZ` as the last frame left it: the view depth
    /// per pixel, row by row.
    pub(crate) fn read_floatz(&self) -> Vec<f32> {
        let t = self.soft.linz().texture();
        let (w, h) = (t.width(), t.height());
        let row = (w * 4).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
        let buf = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("floatZ readback"),
            size: u64::from(row * h),
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
                    bytes_per_row: Some(row),
                    rows_per_image: Some(h),
                },
            },
            t.size(),
        );
        self.queue.submit([enc.finish()]);
        buf.slice(..).map_async(wgpu::MapMode::Read, |_| {});
        self.device
            .poll(wgpu::PollType::wait_indefinitely())
            .unwrap();
        let bytes = buf.slice(..).get_mapped_range().unwrap().to_vec();
        bytes
            .chunks(row as usize)
            .flat_map(|r| r[..(w * 4) as usize].chunks(4))
            .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
            .collect()
    }

    /// Turns on every pass a cvar builds or enables, so their pipelines
    /// and bind groups are made and drawn.
    fn force_every_pass(&mut self) {
        self.set_shadows(true);
        self.set_ssao(1.0);
        self.set_bloom(0.5, 0.5);
        self.set_sun_shafts(1.0);
        self.set_soft_particles(1.0);
        self.set_bicubic(true);
        self.set_post(crate::post::PostSettings {
            grade: 1.0,
            vignette: 1.0,
            grain: 1.0,
        });
        self.set_screen_blend(Some(500));
        self.set_dynamic_lights(1, true);
    }
}

/// `(origin, yaw degrees)` of the map's first spawn, the eye raised to a
/// standing view.
fn spawn_eye(bsp: &MapWorld) -> (Vec3, f32) {
    let (o, yaw) = vcod_common::bsp::find_spawn(&bsp.entities).expect("the map has a spawn");
    (Vec3::from(o) + Vec3::Z * 60.0, yaw)
}

/// A camera quad `ahead` units down the view from `eye`, with `shader`.
fn fx_quad(eye: Vec3, yaw: f32, ahead: f32, shader: &str) -> FxQuad {
    let (f, r, u) = crate::camera::basis(yaw.to_radians(), 0.0);
    let c = eye + f * ahead;
    let s = 16.0;
    FxQuad {
        verts: [
            c - r * s + u * s,
            c + r * s + u * s,
            c + r * s - u * s,
            c - r * s - u * s,
        ]
        .map(|v| v.to_array()),
        uvs: [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
        rgba: [1.0; 4],
        shader: shader.into(),
    }
}

/// Effects, a mark, HUD quads and text lines in front of `eye`, then
/// two frames (the shadow layer's first redraw and a steady one), the
/// view weapon `vm` over them.
fn draw_frames(
    r: &mut Renderer,
    fs: &Pk3Fs,
    (eye, yaw): (Vec3, f32),
    fx: &[&str],
    vm: impl Fn() -> Option<VmDraw>,
) {
    let quads = fx
        .iter()
        .enumerate()
        .map(|(i, s)| fx_quad(eye, yaw, 64.0 + 32.0 * i as f32, s))
        .collect();
    r.set_fx_quads(fs, quads, 1.0);
    // a CoD2 snow cloud through the cloud pass (no map: drawn untested)
    let (f, right, up) = crate::camera::basis(yaw.to_radians(), 0.0);
    let cloud = crate::fx::cod2_cloud::FxCloud::new(
        eye + f * 200.0,
        Vec3::new(0.0, 0.0, -50.0),
        (100.0, 1.5, 1.5),
        30.0,
        Vec3::Z,
        [1.0; 4],
        "cloud_snowflake01",
        (right, up),
    );
    let clouds = crate::fx::cod2_cloud::Clouds {
        list: vec![cloud],
        right,
        up,
    };
    r.set_fx_clouds(fs, None, &clouds, 1.0);
    r.set_fx_lights(&[crate::fx::sim::FxLight {
        pos: eye.to_array(),
        radius: 200.0,
        rgb: [1.0, 0.5, 0.2],
    }]);
    let mark = crate::marks::MarkRequest {
        shader: "gfx/impact/bullethole1".into(),
        origin: eye - Vec3::Z * 60.0,
        dir: Vec3::Z,
        orientation: 0.0,
        rgba: [1.0; 4],
        alpha_fade: true,
        radius: 8.0,
        life_ms: 8000,
    };
    r.set_marks(fs, &[mark], &[], 1000, 1.0, true);
    r.set_hud_quads(
        fs,
        // A plain quad, then one through the additive pipelines (the font
        // glow's).
        ["white".to_string(), format!("{}white", hud::ADDITIVE)]
            .into_iter()
            .map(|texture| HudQuad {
                verts: [[10.0, 10.0], [60.0, 10.0], [60.0, 60.0], [10.0, 60.0]],
                uvs: [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
                rgba: [1.0; 4],
                texture,
            })
            .collect(),
    );
    let fov = 80.0;
    for time in [1.0, 1.05] {
        let (fwd, _, up) = crate::camera::basis(yaw.to_radians(), 0.0);
        let frame = Frame {
            view_proj: crate::camera::view_proj_from(
                eye,
                yaw.to_radians(),
                0.0,
                0.0,
                fov,
                r.aspect(),
            ),
            eye,
            fwd,
            up,
            time,
            fov_x: fov,
            cull: CullMode::On,
            hud_lines: vec!["pipeline check".into()],
        };
        r.render(frame, vm().map(|v| VmDraw { fov_x: fov, ..v }));
    }
    r.device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("the frames finish");
}

fn assert_clean(errors: &Errors, what: &str) {
    let errors = errors.lock().unwrap();
    assert!(
        errors.is_empty(),
        "{what}: {} wgpu validation error(s):\n{}",
        errors.len(),
        errors.join("\n---\n")
    );
}

/// mp_carentan through the app's path: the world, its submodels, a player
/// body bind-posed and skinned, the kar98k viewmodel, a smoke and an
/// impact sprite, a bullet hole and the HUD, every cvar pass on.
/// The headless device asks for the window's limits: with the default
/// 256 MB buffer size mp_creek's and mp_killhouse's every-LOD vertices
/// were an invalid buffer and the map sweep's client panicked at load.
#[test]
fn headless_device_has_the_window_limits() {
    let Some((r, errors)) = Renderer::headless(&vcod_common::pk3::Pk3Fs::empty(), false, 1) else {
        eprintln!("no adapter with BC textures; skipped");
        return;
    };
    assert_clean(&errors, "renderer build");
    let instance =
        wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
    let adapter = pollster::block_on(instance.request_adapter(&Default::default())).unwrap();
    let have = r.device.limits();
    let want = device_limits(&adapter);
    assert_eq!(have.max_buffer_size, want.max_buffer_size);
    assert!(have.max_buffer_size > 256 << 20, "{}", have.max_buffer_size);
    assert_eq!(have.max_texture_array_layers, want.max_texture_array_layers);
    assert_eq!(
        have.max_sampled_textures_per_shader_stage,
        want.max_sampled_textures_per_shader_stage
    );
}

#[test]
fn cod1_renderer_builds_and_draws_without_validation_errors() {
    let Some(fs) = vcod_common::testing::game_fs() else {
        return;
    };
    let Some((mut r, errors)) = Renderer::headless(&fs, false, 4) else {
        eprintln!("no adapter with BC textures; skipped");
        return;
    };
    assert_clean(&errors, "CoD1 renderer build");
    let data = fs.read("maps/mp/mp_carentan.bsp").expect("mp_carentan");
    let bsp = vcod_common::mapworld::load(&data).unwrap();
    r.load_world(&bsp, &fs).unwrap();
    r.set_submodels(
        &(1..bsp.models.len())
            .map(|m| (m, glam::Mat4::IDENTITY))
            .collect::<Vec<_>>(),
    );
    r.force_every_pass();
    let (eye, yaw) = spawn_eye(&bsp);

    let body = model::load(&fs, "playerbody_american_airborne").unwrap();
    let handle = r
        .upload_dynamic_model(&fs, &body)
        .expect("the body uploads");
    let (fwd, _, _) = crate::camera::basis(yaw.to_radians(), 0.0);
    let at = |d: f32| glam::Mat4::from_translation(eye + fwd * d - Vec3::Z * 60.0);
    r.set_dynamic_models(&[
        DynamicModelInstance {
            model: handle,
            transform: at(80.0),
            bones: None,
            light_origin: eye,
            shader_time: None,
        },
        DynamicModelInstance {
            model: handle,
            transform: at(120.0),
            bones: Some(vec![glam::Mat4::IDENTITY; 40]),
            light_origin: eye,
            shader_time: Some(2.0),
        },
    ]);
    let (models, _) = crate::viewmodel::load_view_weapon(&fs, "kar98k_mp", None).unwrap();
    r.set_viewmodel(&fs, &models);
    let bone_sets = models
        .iter()
        .map(|m| vec![glam::Mat4::IDENTITY; m.bones.len().max(1)])
        .collect::<Vec<_>>();
    draw_frames(
        &mut r,
        &fs,
        (eye, yaw),
        &["gfx/effects/smoke/roller", "gfx/impact/bullethit_plaster"],
        || {
            Some(VmDraw {
                transform: glam::Mat4::IDENTITY,
                light_origin: eye,
                fov_x: 0.0,
                bone_sets: bone_sets.clone(),
            })
        },
    );
    assert_clean(&errors, "CoD1 mp_carentan frame");
}

/// A windowed CoD1 frame at `r_gamma` 1 skips the gamma pass and draws the
/// scene and the 2D into the target (`scene_direct`), and CoD1's default
/// draws no depth prepass (cod11-client-perf.md 4): the bytes are the
/// gamma pass's and the prepass's, at one sample and at four.
#[test]
fn cod1_identity_ramp_frame_matches_the_gamma_pass_and_the_prepass() {
    let Some(fs) = vcod_common::testing::game_fs() else {
        return;
    };
    let data = fs.read("maps/mp/mp_carentan.bsp").expect("mp_carentan");
    let bsp = vcod_common::mapworld::load(&data).unwrap();
    let body = model::load(&fs, "playerbody_american_airborne").unwrap();
    for samples in [1, 4] {
        let Some((mut r, errors)) = Renderer::headless(&fs, false, samples) else {
            eprintln!("no adapter with BC textures; skipped");
            return;
        };
        r.set_gamma(1.0, crate::gamma::overbright_bits(fs.game(), false, true));
        r.load_world(&bsp, &fs).unwrap();
        let at = spawn_eye(&bsp);
        let handle = r
            .upload_dynamic_model(&fs, &body)
            .expect("the body uploads");
        let (fwd, _, _) = crate::camera::basis(at.1.to_radians(), 0.0);
        r.set_dynamic_models(&[DynamicModelInstance {
            model: handle,
            transform: glam::Mat4::from_translation(at.0 + fwd * 80.0 - Vec3::Z * 60.0),
            bones: None,
            light_origin: at.0,
            shader_time: None,
        }]);
        let mut shot = |direct: bool, prepass: bool| {
            r.swap_direct = direct;
            r.set_depth_prepass(prepass);
            r.prepass_drawn.set(0);
            draw_frames(&mut r, &fs, at, &["gfx/impact/bullethit_plaster"], || None);
            assert_eq!(r.gamma_skipped, direct, "{samples}x: the gamma pass ran");
            assert_eq!(r.prepass_drawn.get() > 0, prepass, "{samples}x: prepass");
            r.read_offscreen().expect("offscreen target").2
        };
        let before = shot(false, true);
        for (direct, prepass) in [(true, true), (false, false), (true, false)] {
            let after = shot(direct, prepass);
            let differ = before.iter().zip(&after).filter(|(a, b)| a != b).count();
            assert_eq!(
                differ, 0,
                "{samples}x, direct {direct}, prepass {prepass}: {differ} bytes differ"
            );
        }
        assert_clean(&errors, "CoD1 identity ramp frames");
    }
}

/// CoD4's mp_crash through fly mode's path (`cod4_fly`): the zone's world,
/// lightmaps, static models and sky under its own materials and an AK-47
/// world model, viewed from the first `mp_tdm_spawn` and from above the
/// town, with the sun shadow map on and off. With
/// `VCOD_C4_SHOTS=<dir>` each view is written there as a TGA.
#[test]
fn cod4_renderer_builds_and_draws_without_validation_errors() {
    let Some(install) = vcod_cod4::testing::cod4_install() else {
        return;
    };
    let dir = install.root().to_path_buf();
    let fs = crate::cod4_fly::open_fs(&dir, None, None).unwrap();
    let Some((mut r, errors)) = Renderer::headless_sized(&fs, false, 1, (1280, 704)) else {
        eprintln!("no adapter with BC textures; skipped");
        return;
    };
    assert_clean(&errors, "CoD4 renderer build");
    // the window's ramp, as main.rs sets it: gamma 1, windowed overbright
    r.set_gamma(1.0, crate::gamma::overbright_bits(fs.game(), false, true));
    // an AK-47's world model to cast into the sun shadow map
    let ents = crate::cod4_fly::EntityAssets {
        models: Vec::new(),
        weapons: vec!["ak47_mp".into()],
    };
    let map = crate::cod4_fly::load(&dir, None, "mp_crash", &ents).unwrap();
    r.set_material_lib(map.materials.clone());
    r.load_world(&map.bsp, &fs).unwrap();
    r.set_submodels(
        &(1..map.bsp.models.len())
            .map(|m| (m, glam::Mat4::IDENTITY))
            .collect::<Vec<_>>(),
    );
    let (o, yaw) = map.spawn;
    let spawn = (Vec3::from(o) + Vec3::Z * 60.0, yaw);
    let gun = map.weapons["ak47_mp"]
        .world
        .as_ref()
        .and_then(|w| map.models.get(w))
        .expect("the AK-47's world model");
    let handle = r.upload_dynamic_model(&fs, gun).expect("the gun uploads");
    let (fwd, _, _) = crate::camera::basis(yaw.to_radians(), 0.0);
    r.set_dynamic_models(&[DynamicModelInstance {
        model: handle,
        transform: glam::Mat4::from_translation(Vec3::from(o) + fwd * 60.0 + Vec3::Z * 40.0),
        bones: None,
        light_origin: Vec3::from(o) + fwd * 60.0 + Vec3::Z * 40.0,
        shader_time: None,
    }]);
    let above = (Vec3::new(600.0, 400.0, 900.0), 225.0);
    let shots = std::env::var_os("VCOD_C4_SHOTS").map(std::path::PathBuf::from);
    for (name, view) in [("spawn", spawn), ("above", above)] {
        draw_frames(&mut r, &fs, view, &[], || None);
        assert_clean(&errors, &format!("CoD4 mp_crash frame ({name})"));
        let (w, h, bgra) = r.read_offscreen().expect("offscreen target");
        let lit = bgra
            .chunks(4)
            .filter(|p| p[0] > 8 || p[1] > 8 || p[2] > 8)
            .count();
        eprintln!("{name}: {lit} of {} pixels lit", w * h);
        assert!(
            lit > (w * h / 4) as usize,
            "{name}: the view is mostly black"
        );
        if let Some(dir) = &shots {
            let rgba: Vec<u8> = bgra
                .chunks(4)
                .flat_map(|p| [p[2], p[1], p[0], 255])
                .collect();
            image::save_buffer(
                dir.join(format!("mp_crash_{name}.tga")),
                &rgba,
                w,
                h,
                image::ExtendedColorType::Rgba8,
            )
            .unwrap();
        }
    }
    // `r_vc_depthprepass` single-sampled, over the moved brush models'
    // sets too (cod4_live checks its pixels at 4x MSAA)
    r.set_depth_prepass(true);
    draw_frames(&mut r, &fs, spawn, &[], || None);
    assert_clean(&errors, "CoD4 mp_crash frame with the depth prepass");
    assert!(r.prepass_drawn.get() > 0, "the prepass drew nothing");
    r.set_depth_prepass(false);
    // mp_bog's glow over the town (cod4-world.md 11.4): the setup, two 1D
    // passes at this size and the post pass's add, which only brightens
    let sum = |r: &Renderer| -> u64 {
        let (_, _, bgra) = r.read_offscreen().expect("offscreen target");
        bgra.chunks(4)
            .map(|p| u64::from(p[0]) + u64::from(p[1]) + u64::from(p[2]))
            .sum()
    };
    // the sun shadow map draws by default (cod4-world.md 3.6), and off the
    // baked visibility alone moves pixels
    let line = r.sun_shadow_line();
    eprintln!("{line}");
    assert!(line.starts_with("sunshadow sm_enable"), "{line}");
    assert!(!line.contains(" 0 ents"), "the gun casts: {line}");
    let shadowed = sum(&r);
    r.set_sun_shadow(false, Default::default());
    draw_frames(&mut r, &fs, above, &[], || None);
    assert_clean(&errors, "CoD4 mp_crash frame without the sun shadow map");
    let baked = sum(&r);
    eprintln!("sun shadow map: pixel sum {shadowed}, baked only {baked}");
    assert_ne!(shadowed, baked);
    assert_eq!(r.sun_shadow_line(), "sunshadow off");
    r.set_sun_shadow(true, Default::default());
    draw_frames(&mut r, &fs, above, &[], || None);
    let before = sum(&r);
    r.set_level_look(Some(LevelLook {
        film: None,
        glow: Some(crate::cod2_glow::Cod4Glow {
            cutoff: 0.5,
            desaturation: 0.75,
            intensity: 1.42171,
            radius: 25.2691,
        }),
    }));
    draw_frames(&mut r, &fs, above, &[], || None);
    assert_clean(&errors, "CoD4 mp_crash frame with a glow");
    let after = sum(&r);
    eprintln!("glow: pixel sum {before} -> {after}");
    assert!(after > before + before / 100, "{before} -> {after}");
}

/// mp_toujane through the CoD2 session's draw: the retail capture's frame
/// (its turrets, flak wrecks, brush models and the other client's body,
/// probe lit) handed over by `Cod2Draw`, the kar98k view weapon, a
/// feathered and a plain effect, a bullet hole and the HUD, every cvar
/// pass on, viewed from behind the body.
#[test]
fn cod2_renderer_builds_and_draws_without_validation_errors() {
    let Some(fs) = vcod_common::testing::cod2_fs() else {
        return;
    };
    let Some((mut r, errors)) = Renderer::headless(&fs, false, 1) else {
        eprintln!("no adapter with BC textures; skipped");
        return;
    };
    assert_clean(&errors, "CoD2 renderer build");
    vcod_cod2::model::register();
    let data = fs.read("maps/mp/mp_toujane.d3dbsp").expect("mp_toujane");
    let bsp = vcod_cod2::world::from_bytes(&data).unwrap();
    r.load_world(&bsp, &fs).unwrap();
    r.force_every_pass();

    let view = crate::cod2_scene::tests::replay();
    let f = crate::cod2_scene::tests::mid(&view);
    let mut draw = crate::cod2_scene::Cod2Draw::new();
    let counts = draw.draw(&f, &fs, &mut r);
    assert!(counts.player_parts >= 3, "{counts:?}");
    let body = crate::cod2_scene::players(&f)[0].frame.origin;
    let yaw = 0.0;
    let eye = body - Vec3::X * 100.0 + Vec3::Z * 60.0;

    let rig = crate::cod2_viewmodel::load_rig(&fs, "kar98k_mp", None).unwrap();
    r.set_viewmodel(&fs, &rig.models);
    let feathered = r
        .shader_lib
        .names()
        .filter(|n| r.shader_lib.get(n).is_some_and(|m| m.feather.is_some()))
        .min()
        .expect("CoD2 has zfeather materials")
        .to_string();
    let plain = "gfx/impact/bullethit_plaster";
    // a `blurWorld 5.0` menu's blur over the glow (cod2-materials.md 9.13)
    r.set_world_blur(5.0);
    draw_frames(&mut r, &fs, (eye, yaw), &[&feathered, plain], || {
        Some(crate::cod2_viewmodel::idle_draw(&rig, eye))
    });
    assert_clean(&errors, "CoD2 mp_toujane frame");
    // the body casts a shadow cookie onto the street (cod2-materials.md 9.8)
    let shaded = r.dyn_shadows.shaded(&r.device, &r.queue);
    eprintln!(
        "{} shadow cookies, {shaded} pixels shaded",
        r.cookies.live()
    );
    assert!(r.cookies.live() > 0 && shaded > 0);
}

/// mp_toujane's retail capture frame (`Cod2Draw`: turrets, flak wrecks,
/// brush models, the other client's body) at the default cvars, from the
/// spawn and from behind the body, each frame written as a PPM under
/// `VCOD_C4_SHOTS` for a byte compare of a renderer change.
#[test]
fn cod2_capture_frame_draws_lit() {
    let Some(fs) = vcod_common::testing::cod2_fs() else {
        return;
    };
    let Some((mut r, errors)) = Renderer::headless(&fs, false, 1) else {
        eprintln!("no adapter with BC textures; skipped");
        return;
    };
    // the window's ramp, as `main.rs`'s frame sets it
    r.set_gamma(1.0, crate::gamma::overbright_bits(fs.game(), false, true));
    vcod_cod2::model::register();
    let data = fs.read("maps/mp/mp_toujane.d3dbsp").expect("mp_toujane");
    let bsp = vcod_cod2::world::from_bytes(&data).unwrap();
    r.load_world(&bsp, &fs).unwrap();
    let view = crate::cod2_scene::tests::replay();
    let f = crate::cod2_scene::tests::mid(&view);
    let mut draw = crate::cod2_scene::Cod2Draw::new();
    draw.draw(&f, &fs, &mut r);
    let body = crate::cod2_scene::players(&f)[0].frame.origin;
    let shots = std::env::var_os("VCOD_C4_SHOTS").map(std::path::PathBuf::from);
    let behind = (body - Vec3::X * 100.0 + Vec3::Z * 40.0, 0.0);
    for (name, at) in [("spawn", spawn_eye(&bsp)), ("behind", behind)] {
        draw_frames(&mut r, &fs, at, &[], || None);
        assert_clean(&errors, &format!("CoD2 mp_toujane frame ({name})"));
        let (w, h, bgra) = r.read_offscreen().expect("offscreen target");
        let lit = bgra
            .chunks(4)
            .filter(|p| p[0] > 8 || p[1] > 8 || p[2] > 8)
            .count();
        eprintln!("{name}: {lit} of {} pixels lit", w * h);
        assert!(lit > (w * h / 4) as usize, "{name}: mostly black");
        if let Some(dir) = &shots {
            let mut ppm = format!("P6\n{w} {h}\n255\n").into_bytes();
            ppm.extend(bgra.chunks(4).flat_map(|p| [p[2], p[1], p[0]]));
            std::fs::write(dir.join(format!("mp_toujane_{name}.ppm")), ppm).unwrap();
        }
    }
}

/// The cookie receivers over the world's depth prepass against on their
/// own depth (cod2-client-perf.md 11), mp_toujane's capture frame with
/// the prepass on, from the spawn and from behind the body: the frames'
/// pixels are the same, and under `VCOD_C4_SHOTS`, written as
/// `mp_toujane_<view>_<own|prepass>.ppm`.
#[test]
fn cod2_cookie_receivers_over_the_prepass() {
    let Some(fs) = vcod_common::testing::cod2_fs() else {
        return;
    };
    let Some((mut r, errors)) = Renderer::headless(&fs, false, 1) else {
        eprintln!("no adapter with BC textures; skipped");
        return;
    };
    r.set_gamma(1.0, crate::gamma::overbright_bits(fs.game(), false, true));
    r.set_depth_prepass(true);
    vcod_cod2::model::register();
    let data = fs.read("maps/mp/mp_toujane.d3dbsp").expect("mp_toujane");
    let bsp = vcod_cod2::world::from_bytes(&data).unwrap();
    r.load_world(&bsp, &fs).unwrap();
    let view = crate::cod2_scene::tests::replay();
    let f = crate::cod2_scene::tests::mid(&view);
    let mut draw = crate::cod2_scene::Cod2Draw::new();
    draw.draw(&f, &fs, &mut r);
    let body = crate::cod2_scene::players(&f)[0].frame.origin;
    let shots = std::env::var_os("VCOD_C4_SHOTS").map(std::path::PathBuf::from);
    let behind = (body - Vec3::X * 100.0 + Vec3::Z * 40.0, 0.0);
    for (name, at) in [("spawn", spawn_eye(&bsp)), ("behind", behind)] {
        let mut frames = Vec::new();
        for (tag, on) in [("own", false), ("prepass", true)] {
            r.set_cookies_on_prepass(on);
            draw_frames(&mut r, &fs, at, &[], || None);
            assert_clean(&errors, &format!("CoD2 mp_toujane frame ({name}, {tag})"));
            let (w, h, bgra) = r.read_offscreen().expect("offscreen target");
            if let Some(dir) = &shots {
                let mut ppm = format!("P6\n{w} {h}\n255\n").into_bytes();
                ppm.extend(bgra.chunks(4).flat_map(|p| [p[2], p[1], p[0]]));
                std::fs::write(dir.join(format!("mp_toujane_{name}_{tag}.ppm")), ppm).unwrap();
            }
            let shaded = r.dyn_shadows.shaded(&r.device, &r.queue);
            if name == "behind" {
                assert!(shaded > 0, "{name}, {tag}: no pixel under a cookie");
            }
            assert_eq!(r.cookie_receivers_on_prepass(), on);
            eprintln!("{name}, {tag}: {shaded} pixels under the cookies");
            frames.push(bgra);
        }
        let moved = frames[0]
            .chunks(4)
            .zip(frames[1].chunks(4))
            .filter(|(a, b)| a != b)
            .count();
        eprintln!("{name}: {moved} pixels differ of {}", frames[0].len() / 4);
        assert_eq!(moved, 0, "{name}: the folded receivers moved pixels");
    }
}

/// Not a check: times `Renderer::cull` (the cell walk and the static
/// models' cell test, the profile's `cull`) from each map's
/// `mp_global_intermission` view, where `cod4_perf.sh`'s CoD2 spectator
/// stands (cod2-client-perf.md 8). Maps from `VCOD_PROFILE_MAPS`.
#[test]
#[ignore]
fn cod2_cull_profile() {
    let Some(fs) = vcod_common::testing::cod2_fs() else {
        return;
    };
    let Some((mut r, _errors)) = Renderer::headless(&fs, false, 1) else {
        eprintln!("no adapter with BC textures; skipped");
        return;
    };
    let maps =
        std::env::var("VCOD_PROFILE_MAPS").unwrap_or_else(|_| "mp_toujane mp_carentan".into());
    for map in maps.split_whitespace() {
        let data = fs.read(&format!("maps/mp/{map}.d3dbsp")).expect(map);
        let bsp = vcod_cod2::world::from_bytes(&data).unwrap();
        r.load_world(&bsp, &fs).unwrap();
        let spot = vcod_common::bsp::entity_blocks(&bsp.entities)
            .into_iter()
            .find(|b| b.get("classname").map(String::as_str) == Some("mp_global_intermission"))
            .expect("an intermission spot");
        let eye = Vec3::from(vcod_common::bsp::parse_vec3(&spot["origin"]).unwrap());
        let ang = spot
            .get("angles")
            .and_then(|a| vcod_common::bsp::parse_vec3(a))
            .unwrap_or([0.0; 3]);
        let view_proj = crate::camera::view_proj_from(
            eye,
            ang[1].to_radians(),
            ang[0].to_radians(),
            0.0,
            80f32.to_radians(),
            640.0 / 372.0,
        );
        let frustum = Frustum::from_view_proj(view_proj);
        let world = r.world.as_ref().unwrap();
        const ROUNDS: u32 = 200;
        let (mut walk, mut props) = (0.0, 0.0);
        let mut seen = (0, 0);
        for _ in 0..ROUNDS {
            let t = std::time::Instant::now();
            let v = world.vis.visible(eye, &frustum, Some(view_proj));
            walk += t.elapsed().as_secs_f64();
            let t = std::time::Instant::now();
            let ok = world
                .prop_bounds
                .iter()
                .filter(|&&(lo, hi)| world.vis.prop_visible(&v, &frustum, lo, hi))
                .count();
            props += t.elapsed().as_secs_f64();
            seen = (v.soups.iter().filter(|&&s| s).count(), ok);
        }
        let ms = |s: f64| s * 1000.0 / f64::from(ROUNDS);
        eprintln!(
            "CULL {map}: walk {:.3} ms, props {:.3} ms ({} placements), {} soups, {} props seen, {} cells visited",
            ms(walk),
            ms(props),
            world.prop_bounds.len(),
            seen.0,
            seen.1,
            world
                .vis
                .visible(eye, &frustum, Some(view_proj))
                .stats
                .cells_visited,
        );
    }
}

/// Not a check: draws each map's spawn views (eye 60 up, four yaws) under
/// the CoD1 recursive cell walk and under retail's queued one
/// (`MapWorld::portals_only`, cod2-client-perf.md 9) and counts the pixels
/// that differ and the soups each marked: a soup the queued walk drops
/// that a view shows is a hole. Maps from `VCOD_PROFILE_MAPS`; at most
/// `VCOD_WALK_SPAWNS` (default 8) spawns a map.
#[test]
#[ignore]
fn cod2_cell_walks_draw_alike() {
    let Some(fs) = vcod_common::testing::cod2_fs() else {
        return;
    };
    let Some((mut r, errors)) = Renderer::headless(&fs, false, 1) else {
        eprintln!("no adapter with BC textures; skipped");
        return;
    };
    r.set_gamma(1.0, crate::gamma::overbright_bits(fs.game(), false, true));
    vcod_cod2::model::register();
    let maps =
        std::env::var("VCOD_PROFILE_MAPS").unwrap_or_else(|_| "mp_toujane mp_carentan".into());
    let spawns: usize = std::env::var("VCOD_WALK_SPAWNS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(8);
    for map in maps.split_whitespace() {
        let data = fs.read(&format!("maps/mp/{map}.d3dbsp")).expect(map);
        let mut bsp = vcod_cod2::world::from_bytes(&data).unwrap();
        let views: Vec<(Vec3, f32)> = vcod_common::bsp::entity_blocks(&bsp.entities)
            .into_iter()
            .filter(|b| {
                b.get("classname")
                    .is_some_and(|c| c.starts_with("mp_") && c.ends_with("_spawn"))
            })
            .filter_map(|b| vcod_common::bsp::parse_vec3(b.get("origin")?))
            .take(spawns)
            .flat_map(|o| (0..4).map(move |k| (Vec3::from(o) + Vec3::Z * 60.0, 90.0 * k as f32)))
            .collect();
        let mut drawn: Vec<Vec<Vec<u8>>> = Vec::new();
        for queued in [false, true] {
            bsp.portals_only = queued;
            r.load_world(&bsp, &fs).unwrap();
            let mut frames = Vec::new();
            for &at in &views {
                draw_frames(&mut r, &fs, at, &[], || None);
                assert_clean(&errors, &format!("CoD2 {map} walk frame"));
                frames.push(r.read_offscreen().expect("offscreen target").2);
            }
            drawn.push(frames);
        }
        let mut worst = (0usize, (Vec3::ZERO, 0.0));
        let mut total = 0usize;
        let mut views_differ = 0usize;
        for (i, (a, b)) in drawn[0].iter().zip(&drawn[1]).enumerate() {
            let n = a
                .chunks(4)
                .zip(b.chunks(4))
                .filter(|(p, q)| p[..3] != q[..3])
                .count();
            total += n;
            views_differ += usize::from(n > 0);
            if let (true, Some(dir)) = (n > 0, std::env::var_os("VCOD_C4_SHOTS")) {
                let (w, h, _) = r.read_offscreen().expect("offscreen target");
                for (walk, px) in [("recursive", a), ("queued", b)] {
                    let mut ppm = format!("P6\n{w} {h}\n255\n").into_bytes();
                    ppm.extend(px.chunks(4).flat_map(|p| [p[2], p[1], p[0]]));
                    let name = format!("{map}_walk{i}_{walk}.ppm");
                    std::fs::write(std::path::Path::new(&dir).join(name), ppm).unwrap();
                }
            }
            if n > worst.0 {
                worst = (n, views[i]);
            }
        }
        eprintln!(
            "WALKDRAW {map}: {} views, {views_differ} differ, {total} pixels differ, most {} at {:?} yaw {}",
            views.len(),
            worst.0,
            worst.1.0,
            worst.1.1
        );
    }
}

/// Not a check: times CoD2 map loads through the renderer, the session's
/// collision and the first draw's models, each map twice (the second
/// load reuses what the first cached), with a digest of every texture
/// uploaded so a faster load can show it uploads the same. Maps from
/// `VCOD_PROFILE_MAPS` (space-separated). Stage lines log at info.
#[test]
#[ignore]
fn cod2_load_profile() {
    let _ = env_logger::builder()
        .is_test(true)
        .filter_level(log::LevelFilter::Info)
        .try_init();
    let Some(fs) = vcod_common::testing::cod2_fs() else {
        return;
    };
    let t = std::time::Instant::now();
    let Some((mut r, _errors)) = Renderer::headless(&fs, false, 1) else {
        eprintln!("no adapter with BC textures; skipped");
        return;
    };
    eprintln!(
        "PROFILE renderer build {:.0} ms",
        t.elapsed().as_secs_f64() * 1000.0
    );
    vcod_cod2::model::register();
    let maps = std::env::var("VCOD_PROFILE_MAPS")
        .unwrap_or_else(|_| "mp_toujane mp_carentan mp_burgundy".into());
    let view = crate::cod2_scene::tests::replay();
    let f = crate::cod2_scene::tests::mid(&view);
    let ms = |t: std::time::Instant| t.elapsed().as_secs_f64() * 1000.0;
    let mut draw = crate::cod2_scene::Cod2Draw::new();
    let mut audio = crate::audio::AudioSystem::new(
        &fs,
        crate::audio::AudioOpts {
            enabled: false,
            volume: 1.0,
            rate: 0,
        },
    );
    for round in 0..2 {
        for map in maps.split_whitespace() {
            let total = std::time::Instant::now();
            let t = std::time::Instant::now();
            let data = fs.read(&format!("maps/mp/{map}.d3dbsp")).expect(map);
            let file = vcod_cod2::map::MapFile::parse(&data).unwrap();
            let bsp = file.world();
            let parse = ms(t);
            // as `load_cod2_map` loads: the prediction's collision beside
            let t = std::time::Instant::now();
            super::upload_digest::start();
            fs.clear_references();
            let collision = std::thread::scope(|sc| {
                let cw = sc.spawn(|| {
                    let t = std::time::Instant::now();
                    let clip = std::sync::Arc::new(file.clip().unwrap());
                    crate::play::cod2::prediction_world(&bsp, clip);
                    ms(t)
                });
                r.load_world(&bsp, &fs).unwrap();
                vcod_common::laps::joined(cw)
            });
            let (digest, textures) = super::upload_digest::take().unwrap();
            let paks: Vec<String> = fs
                .referenced_paks()
                .iter()
                .map(|p| p.name.clone())
                .collect();
            let world = ms(t);
            let t = std::time::Instant::now();
            // one draw across maps, as `load_cod2_map` carries it
            draw.next_map();
            draw.draw(&f, &fs, &mut r);
            let scene = ms(t);
            let t = std::time::Instant::now();
            audio.on_gamestate(map);
            let sounds = ms(t);
            let t = std::time::Instant::now();
            let rig = crate::cod2_viewmodel::load_rig(&fs, "kar98k_mp", None).unwrap();
            r.set_viewmodel(&fs, &rig.models);
            let gun = ms(t);
            eprintln!(
                "PROFILE round {round} {map}: total {:.0} ms: read+parse {parse:.0}, \
                 renderer with collision {world:.0} (collision {collision:.0}), first scene {scene:.0}, \
                 view weapon {gun:.0}, sound aliases {sounds:.0}; {textures} textures digest {digest:016x}, paks {}",
                ms(total),
                paks.join(" ")
            );
        }
    }
}

/// mp_harbor's water (cod2-materials.md 9.12): the water pipeline, its
/// groups and the frame's height map upload, viewed from above a water
/// surface.
#[test]
fn cod2_water_builds_and_draws_without_validation_errors() {
    let Some(fs) = vcod_common::testing::cod2_fs() else {
        return;
    };
    let Some((mut r, errors)) = Renderer::headless(&fs, false, 1) else {
        eprintln!("no adapter with BC textures; skipped");
        return;
    };
    vcod_cod2::model::register();
    let data = fs.read("maps/mp/mp_harbor.d3dbsp").expect("mp_harbor");
    let bsp = vcod_cod2::world::from_bytes(&data).unwrap();
    let soup = bsp
        .soups
        .iter()
        .find(|s| bsp.materials[usize::from(s.material)].name == "water_elalamein")
        .expect("mp_harbor draws water_elalamein");
    let on = Vec3::from_array(bsp.verts[soup.first_vertex as usize].pos);
    r.load_world(&bsp, &fs).unwrap();
    r.force_every_pass();
    let waters = r
        .world
        .as_ref()
        .and_then(|w| w.cod2.as_ref()?.water.as_ref())
        .map_or(0, |w| w.sims.len());
    assert_eq!(waters, 1);
    draw_frames(&mut r, &fs, (on + Vec3::Z * 60.0, 0.0), &[], || None);
    assert_clean(&errors, "CoD2 mp_harbor water frame");
}

/// The front end with no map loaded, single-sampled as CoD2 runs by
/// default (`r_aaSamples` 1): the HUD draws in the pass that carries the
/// depth target, so its pipeline must name the depth format too.
#[test]
fn cod2_menu_draws_single_sampled_without_validation_errors() {
    let Some(fs) = vcod_common::testing::cod2_fs() else {
        return;
    };
    let Some((mut r, errors)) = Renderer::headless(&fs, false, 1) else {
        eprintln!("no adapter with BC textures; skipped");
        return;
    };
    r.set_hud_quads(
        &fs,
        vec![HudQuad {
            verts: [[10.0, 10.0], [60.0, 10.0], [60.0, 60.0], [10.0, 60.0]],
            uvs: [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
            rgba: [1.0; 4],
            texture: "white".into(),
        }],
    );
    let eye = Vec3::ZERO;
    let (fwd, _, up) = crate::camera::basis(0.0, 0.0);
    let frame = Frame {
        view_proj: crate::camera::view_proj_from(eye, 0.0, 0.0, 0.0, 80.0, r.aspect()),
        eye,
        fwd,
        up,
        time: 1.0,
        fov_x: 80.0,
        cull: CullMode::On,
        hud_lines: vec!["menu".into()],
    };
    r.render(frame, None);
    r.device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("the frame finishes");
    assert_clean(&errors, "CoD2 menu frame");
}
