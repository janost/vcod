//! The renderer's settings and readouts: resize, the cvar setters, the counts
//! and F3 lines, capture requests.

use super::*;

/// CoD4's Options switches the renderer reads (cod4-front-end.md 9):
/// `r_glow_allowed` (the level's glow), `r_specular` (the lit sets'
/// specular), `r_drawWater` (the water's animation), `r_zFeather` (the
/// effects' depth feather), `r_dof_enable` (the depth of field) and
/// `r_distortion` (the distortion effects' bend of the scene copy).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cod4Options {
    pub glow: bool,
    pub specular: bool,
    pub water: bool,
    pub zfeather: bool,
    pub dof: bool,
    pub distortion: bool,
    /// `r_fog`; off, no draw fogs (the camera's fog mode 0).
    pub fog: bool,
    /// `r_vc_normalMaps` (vcod's): off, no CoD4 set reads its normal map
    /// ([`cod2::Cod2World::set_normal_maps`]).
    pub normal_maps: bool,
    /// `r_vc_film` (vcod's): off, the level's film grade is not drawn.
    pub film: bool,
}

impl Default for Cod4Options {
    fn default() -> Self {
        Cod4Options {
            glow: true,
            specular: true,
            water: true,
            zfeather: true,
            dof: true,
            distortion: true,
            fog: true,
            normal_maps: true,
            film: true,
        }
    }
}

impl Renderer {
    pub fn resize(&mut self, w: u32, h: u32) {
        if w == 0 || h == 0 {
            return;
        }
        self.config.width = w;
        self.config.height = h;
        if let Target::Surface(s) = &self.target {
            s.configure(&self.device, &self.config);
        }
        self.msaa_view = create_msaa_view(&self.device, self.world_format, w, h);
        self.depth_view = create_depth_view(&self.device, w, h);
        self.gamma.resize(&self.device, w, h);
        self.ssao.resize(&self.device, &self.depth_view, w, h);
        self.soft.resize(&self.device, &self.depth_view, w, h);
        self.scene_copy.resize(&self.device, self.soft.linz(), w, h);
        self.post.resize(&self.device, w, h);
        self.dof.resize(&self.device, w, h);
        let src = if self.effects_read_post {
            self.post.world_view()
        } else {
            self.gamma.scene_view()
        };
        self.bloom.resize(&self.device, src, w, h);
        self.shafts
            .resize(&self.device, src, &self.depth_view, w, h);
        self.screen_blend
            .resize(&self.device, self.gamma.scene_view(), w, h);
        self.cod2_glow.resize(&self.device, w, h);
        self.dyn_shadows = cod2_cookie::DynamicShadows::new(&self.device, &self.queue, w, h);
        self.rebuild_camera_bg();
    }

    /// The shellshock's screen blend for the next frame
    /// (`play::shock::ShockFrame::blend_ms`): `Some(decay)` blends the
    /// last capture over the 3D view and captures again, `None` drops it.
    pub fn set_screen_blend(&mut self, decay_ms: Option<i32>) {
        self.shock_blend = decay_ms;
    }

    /// This frame's CoD4 depth of field (`cod4_client`); drawn under
    /// `r_dof_enable` when [`crate::cod4_dof::Dof::used`].
    pub fn set_cod4_dof(&mut self, dof: crate::cod4_dof::Dof) {
        self.cod4_dof = Some(dof);
    }

    /// A CoD4 session's shellshock screen for the next frame only
    /// (`play::cod4_shock::Screen`); without one the frame takes
    /// [`Self::set_screen_blend`]'s.
    pub fn set_cod4_shock(&mut self, screen: Option<crate::play::cod4_shock::Screen>) {
        self.cod4_shock = screen;
    }

    /// CoD2's view blur for the next frame, 480-high virtual units (a
    /// `blurWorld` menu's, `frontend::Ui::blur_world`); 0 draws it sharp.
    pub fn set_world_blur(&mut self, radius: f32) {
        self.world_blur = radius;
    }

    /// `vc_grade`, `vc_vignette` and `vc_grain`, applied from the next frame.
    pub fn set_post(&mut self, settings: crate::post::PostSettings) {
        self.post_settings = settings;
    }

    /// A CoD4 level's vision set for the next frame: the film tweak runs
    /// in the post pass and the glow replaces CoD2's default one (`None`
    /// draws none). `None` leaves CoD1's and CoD2's own.
    pub fn set_level_look(&mut self, look: Option<LevelLook>) {
        self.level_look = look;
    }

    /// Whether the world draws into the float target (`vc_hdr` when the
    /// renderer was built).
    pub fn hdr(&self) -> bool {
        self.world_format == HDR_FORMAT
    }

    /// `vc_bloom` (0 off) and `vc_bloomThreshold`, applied from the next frame.
    pub fn set_bloom(&mut self, intensity: f32, threshold: f32) {
        self.bloom_intensity = intensity.max(0.0);
        self.bloom_threshold = threshold;
    }

    /// `r_textureMode`, applied from the next frame: new samplers and the
    /// group-0 bind groups that hold them, as `GL_TextureMode` (0x4ea530)
    /// resets every mipmapped image's filter in place.
    pub fn set_texture_filter(&mut self, filter: texture_settings::TextureFilter) {
        if filter == self.textures.filter {
            return;
        }
        self.textures.filter = filter;
        self.tex_samplers = TexSamplers::new(&self.device, filter);
        self.rebuild_camera_bg();
        self.vm_pass.uniform_bg = vm_uniform_group(
            &self.device,
            &self.vm_pass.uniform_layout,
            &self.vm_pass.uniform_buf,
            &self.tex_samplers,
        );
        if let Some(flare) = self.sky.as_mut().and_then(|s| s.flare.as_mut()) {
            flare.set_samplers(&self.device, &self.tex_samplers);
        }
        for tex in self.hud_pass.textures.values_mut().flatten() {
            tex.bg = hud_bind_group(
                &self.device,
                &self.hud_pass.texture_layout,
                &self.tex_samplers,
                &tex.view,
                tex.clamp,
            );
        }
    }

    /// The `sc_*` dvars CoD2's shadow cookies read.
    pub fn set_cookie_settings(&mut self, s: CookieSettings) {
        self.cookies.settings = s;
    }

    /// The scene entities that may cast a shadow cookie this frame, each a
    /// range of the instances last handed to [`Self::set_dynamic_models`].
    pub fn set_cookie_entities(&mut self, entities: Vec<CookieEntity>) {
        self.cookies.entities = entities;
    }

    /// `vc_shadows`: builds the shadow pass on the first frame it is on,
    /// drops it when it goes off.
    pub fn set_shadows(&mut self, on: bool) {
        if on == self.shadows.is_some() {
            return;
        }
        self.shadows = on.then(|| {
            let [vertex, instance] = crate::entity_material::vertex_layouts();
            let mats = &self.dynamic.materials;
            shadows::ShadowPass::new(
                &self.device,
                &self.queue,
                self.world_format,
                msaa_samples(),
                shadows::EntLayouts {
                    skin: &mats.skin_layout,
                    bones: &self.dynamic.bone_layout,
                    params: &mats.params_layout,
                    vertex,
                    instance,
                },
                shadows::WorldLayouts {
                    material: &self.material_layout,
                    stage: &self.stage_layout,
                },
            )
        });
        // the camera groups bind the receiver inputs of the new state
        self.rebuild_camera_bg();
    }

    /// The camera group over `camera` (the shared Camera block or a moved
    /// brush model's) and the renderer's receiver inputs and samplers.
    pub(super) fn camera_group(&self, camera: &wgpu::Buffer) -> wgpu::BindGroup {
        camera_bind_group(
            &self.device,
            &self.camera_layout,
            camera,
            &self.fx_lights_buf,
            &self.shadow_receive(),
            &self.tex_samplers,
            self.dyn_shadows.view(),
            &self.sun_shadow,
        )
    }

    /// Rebuilds the camera group after an input it binds changed; the moved
    /// brush models' groups are rebuilt as the next frame needs them.
    pub(super) fn rebuild_camera_bg(&mut self) {
        self.camera_bg = self.camera_group(&self.camera_buf);
        self.moved_cams.clear();
    }

    pub(super) fn shadow_receive(&self) -> shadows::Receive<'_> {
        self.shadows
            .as_ref()
            .map_or_else(|| self.shadow_off.receive(), shadows::ShadowPass::receive)
    }

    /// The F3 `shadow:` line while `vc_shadows` is on.
    pub fn shadow_debug(&self) -> Option<String> {
        self.shadows.as_ref().map(shadows::ShadowPass::debug_line)
    }

    /// `r_gamma`, already clamped to retail's range, and retail's overbright
    /// bits (`gamma::overbright_bits`); applied from the next frame.
    pub fn set_gamma(&mut self, gamma: f32, overbright: u32) {
        self.gamma.set_gamma(&self.queue, gamma, overbright);
        self.identity_light = crate::gamma::identity_light(overbright);
    }

    /// `vc_bicubic`, applied from the next frame.
    pub fn set_bicubic(&mut self, on: bool) {
        self.bicubic = on;
    }

    /// `vc_sunShafts` (0 off), applied from the next frame.
    pub fn set_sun_shafts(&mut self, strength: f32) {
        self.shafts.set_strength(strength);
    }

    /// `r_flares`: off, the sun's flare and washes neither step nor draw.
    pub fn set_flares(&mut self, on: bool) {
        self.flares = on;
    }

    /// `r_drawSun`: off, the sky stage neither draws the disc nor adds the
    /// sun's flare (0x4e84d0); the washes still fade.
    pub fn set_draw_sun(&mut self, on: bool) {
        self.draw_sun = on;
    }

    /// `vc_softParticles`, applied from the next frame; 0 is off.
    pub fn set_soft_particles(&mut self, scale: f32) {
        self.soft.set_scale(&self.queue, scale);
    }

    /// `vc_ssao`, applied from the next frame; 0 is off.
    pub fn set_ssao(&mut self, strength: f32) {
        self.ssao.set_strength(strength);
    }

    /// The F3 `ssao` line: strength and smoothed GPU ms for the scene
    /// passes and the SSAO passes alone.
    pub fn ssao_line(&self) -> String {
        let s = self.ssao.strength();
        let head = if s > 0.0 {
            format!("ssao {s}")
        } else {
            "ssao off".to_string()
        };
        match self.ssao.timer().map(|t| t.ms()) {
            Some((scene, Some(ao))) => format!("{head}  gpu scene {scene:.2}ms ao {ao:.2}ms"),
            Some((scene, None)) => format!("{head}  gpu scene {scene:.2}ms"),
            None => format!("{head}  (no gpu timestamps)"),
        }
    }

    /// `r_swapInterval`, read every frame (`GLimp_EndFrame` 0x50a610 calls
    /// `wglSwapIntervalEXT` when it changes): 0 presents without waiting
    /// for the vertical blank.
    pub fn set_swap_interval(&mut self, on: bool) {
        let mode = if on {
            wgpu::PresentMode::AutoVsync
        } else {
            wgpu::PresentMode::AutoNoVsync
        };
        if self.config.present_mode != mode {
            self.config.present_mode = mode;
            if let Target::Surface(s) = &self.target {
                s.configure(&self.device, &self.config);
            }
        }
    }

    /// CoD4's Options switches (cod4-front-end.md 9).
    pub fn set_cod4_options(&mut self, options: Cod4Options) {
        self.cod4_options = options;
    }

    /// The last frame's draws and state sets.
    #[cfg(test)]
    pub(crate) fn census(&self) -> PassCensus {
        self.census.get()
    }

    /// The last frame's GPU writes in order ([`FrameWrite`]).
    #[cfg(test)]
    pub(crate) fn frame_writes(&self) -> Vec<FrameWrite> {
        self.frame_writes.borrow().clone()
    }

    /// Whether a CoD4 feather's `floatZ` comes from the depth prepass
    /// ([`Self::floatz_prepass`]) or the copy at the split.
    #[cfg(test)]
    pub(crate) fn set_floatz_prepass(&mut self, on: bool) {
        self.floatz_prepass = on;
    }

    /// Off: the cookie receivers keep their own depth (the A/B in
    /// `pipeline_check`).
    #[cfg(test)]
    pub fn set_cookies_on_prepass(&mut self, on: bool) {
        self.cookies_on_prepass = on;
    }

    /// `r_vc_depthprepass` (cod4-client-perf.md 12).
    pub fn set_depth_prepass(&mut self, on: bool) {
        self.depth_prepass = on;
    }

    /// `r_lodscale` and `r_lodbias`, read every frame like retail's
    /// `RE_RenderScene`.
    pub fn set_lod(&mut self, scale: f32, bias: f32) {
        self.lod_cvars = (scale, bias);
    }

    /// `r_dynamiclight` and `r_dlightQuality`, both read every frame.
    pub fn set_dynamic_lights(&mut self, mode: i32, quality: bool) {
        self.dynamic_light = mode;
        self.dlight_quality = quality;
    }

    /// The `r_gamma` textures bake in from the next `load_world`, or `None`
    /// with device gamma. HUD images reload under a new one, as retail
    /// reloads every image at a map load.
    pub fn set_image_gamma(&mut self, gamma: Option<f32>) {
        let table = gamma.map(|g| crate::gamma::ramp(g, 0));
        if table != self.image_gamma {
            self.hud_pass.textures.clear();
        }
        self.image_gamma = table;
    }

    /// Reads the next frame `render` draws back for [`Self::take_capture`]
    /// (`screenshot`, `cl_avidemo`): as the screen shows it, or with
    /// `pre_gamma` the framebuffer's bytes before the gamma ramp.
    pub fn request_capture(&mut self, pre_gamma: bool) {
        self.capture_wanted = Some(pre_gamma);
    }

    /// The frame [`Self::request_capture`] asked for, once drawn: `None`
    /// before that, `Some(None)` when it could not be read back.
    pub fn take_capture(&mut self) -> Option<Option<Capture>> {
        self.captured.take()
    }

    /// For a lost or outdated swapchain.
    pub fn reconfigure(&mut self) {
        let (w, h) = (self.config.width, self.config.height);
        self.resize(w, h);
    }

    /// The dynamic models' LOD pick, for the debug overlay.
    pub fn lod_counts(&self) -> LodCounts {
        self.dynamic.lod_counts
    }

    /// (world draw calls, dynamic instances, bone matrices) for the debug overlay.
    pub fn debug_counts(&self) -> (usize, usize, usize) {
        (
            self.world.as_ref().map_or(0, |w| w.draws.len()),
            self.dynamic.instances.len(),
            self.dynamic.bone_mats.len(),
        )
    }

    pub fn vis_counts(&self) -> &VisCounts {
        &self.vis_counts
    }

    /// (stage draws emitted last frame, dlight draws last frame, stages
    /// dropped at load, unique shader-lib warnings) for the F3 `shader:` line.
    pub fn shader_stats(&self) -> (usize, usize, usize, usize) {
        (
            self.world.as_ref().map_or(0, |w| w.stage_draws_last),
            self.world.as_ref().map_or(0, |w| {
                w.dlight_runs
                    .iter()
                    .map(|r| w.dlights_of_batch[r.draw.batch as usize].len())
                    .sum()
            }),
            self.shader_dropped,
            self.lib_warn_count,
        )
    }

    /// The spot light shadow dvars (`r_spotLightShadows`, `sm_spotEnable`
    /// and kin, crate::cod4_spotshadow).
    pub fn set_spot_shadow(&mut self, settings: crate::cod4_spotshadow::Settings) {
        self.spot_shadow_set = settings;
    }

    /// The F3 `spotshadow` line.
    pub fn spot_shadow_line(&self) -> String {
        self.sun_shadow.spot_line()
    }

    /// The shadowed spot lights and their fades, a tile each.
    #[cfg(test)]
    pub fn shadowed_spots(&self) -> Vec<(usize, f32)> {
        self.sun_shadow.spots()
    }

    /// `sm_enable` and the dvars the partitions read (CoD4's sun shadow
    /// map), clamped to their registered ranges.
    pub fn set_sun_shadow(&mut self, on: bool, mut settings: vcod_cod4::sunshadow::Settings) {
        settings.sample_size_near = settings.sample_size_near.clamp(0.0625, 32.0);
        settings.scale = settings
            .scale
            .clamp(vcod_cod4::sunshadow::SHADOW_SCALE_MIN, 1.0);
        self.sun_shadow_set = (on, settings);
    }

    /// The F3 `sunshadow` line.
    pub fn sun_shadow_line(&self) -> String {
        self.sun_shadow.debug_line()
    }

    /// The aspect every 3D view projects at: the window's, or a CoD4
    /// session's [`Renderer::set_view_aspect`].
    pub fn aspect(&self) -> f32 {
        self.view_aspect
            .unwrap_or(self.config.width as f32 / self.config.height.max(1) as f32)
    }

    /// CoD4's `r_aspectRatio` (`R_StoreWindowSettings`, iw3mp.exe
    /// 0x5f3500, `camera::cod4_aspect`): the view projects at that aspect
    /// across the whole window, stretched where the window's own differs.
    /// `None` is the window's.
    pub fn set_view_aspect(&mut self, aspect: Option<f32>) {
        self.view_aspect = aspect;
    }

    /// Pixels.
    pub fn screen_size(&self) -> (f32, f32) {
        (self.config.width as f32, self.config.height as f32)
    }
}
