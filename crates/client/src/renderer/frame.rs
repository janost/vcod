//! One frame: the cull, the scene targets, `Renderer::render` and the
//! single-sample passes it opens.

use super::*;

/// What `Renderer::cull` reads off the frame, bit for bit.
pub(super) type CullKey = ([u32; 3], [u32; 16], [u32; 2]);

pub(super) fn cull_key(eye: glam::Vec3, view_proj: &glam::Mat4, lod: (f32, f32)) -> CullKey {
    (
        eye.to_array().map(f32::to_bits),
        view_proj.to_cols_array().map(f32::to_bits),
        [lod.0.to_bits(), lod.1.to_bits()],
    )
}

/// The frame's MSAA colour and depth. With SSAO the frame is two passes
/// split after the last opaque or decal draw (`ssao_split`): the first
/// keeps its MSAA colour for the SSAO multiply, the second loads it and
/// resolves.
pub(super) struct SceneTargets<'a> {
    pub(super) msaa: &'a wgpu::TextureView,
    pub(super) depth: &'a wgpu::TextureView,
    pub(super) resolve: &'a wgpu::TextureView,
    pub(super) timer: Option<&'a crate::ssao::GpuTimer>,
    pub(super) clear: wgpu::Color,
    /// The sun flare's probe query, which runs in the last pass.
    pub(super) sun_queries: Option<&'a wgpu::QuerySet>,
    /// The depth prepass laid the depth down: the first pass loads it.
    pub(super) depth_cleared: bool,
}

impl SceneTargets<'_> {
    pub(super) fn begin<'e>(
        &self,
        encoder: &'e mut wgpu::CommandEncoder,
        first: bool,
        last: bool,
    ) -> wgpu::RenderPass<'e> {
        // single-sampled, the scene draws straight into its resolve target
        let resolving = msaa_samples() > 1;
        encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("map pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: if resolving { self.msaa } else { self.resolve },
                depth_slice: None,
                resolve_target: (last && resolving).then_some(self.resolve),
                ops: wgpu::Operations {
                    load: if first {
                        wgpu::LoadOp::Clear(self.clear)
                    } else {
                        wgpu::LoadOp::Load
                    },
                    // only the resolved single-sample image is needed
                    store: if last && resolving {
                        wgpu::StoreOp::Discard
                    } else {
                        wgpu::StoreOp::Store
                    },
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: self.depth,
                depth_ops: Some(wgpu::Operations {
                    load: if first && !self.depth_cleared {
                        wgpu::LoadOp::Clear(1.0)
                    } else {
                        wgpu::LoadOp::Load
                    },
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            // a middle pass (the feather's depth copy splits one off) times nothing
            timestamp_writes: self.timer.filter(|_| first || last).map(|t| {
                t.writes(
                    first.then_some(crate::ssao::GpuTimer::SCENE_BEGIN),
                    last.then_some(crate::ssao::GpuTimer::SCENE_END),
                )
            }),
            occlusion_query_set: if last { self.sun_queries } else { None },
            multiview_mask: None,
        })
    }
}

/// The frame's shape, decided once before its passes
/// ([`Renderer::plan_frame`]): which passes run, the targets they draw
/// into and what the scene pass's draws read.
pub(super) struct FramePlan {
    /// The scene pass's clear colour.
    pub(super) clear: wgpu::Color,
    /// `vc_bloom` runs.
    pub(super) bloom: bool,
    /// `vc_hdr`.
    pub(super) hdr: bool,
    /// CoD4's film, glow (with its intensity) and depth of field.
    pub(super) film: Option<crate::post::Film>,
    pub(super) glow4: Option<(crate::cod2_glow::Plan, f32)>,
    pub(super) dof4: Option<crate::cod4_dof::Dof>,
    /// The world resolves into the post pass's target.
    pub(super) post: bool,
    /// The post pass and the 2D draw straight into the swapchain.
    pub(super) gamma_direct: bool,
    /// `vc_sunShafts` add into the resolved world.
    pub(super) shafts: bool,
    /// The scene resolves and the 2D draws straight into the swapchain.
    pub(super) scene_direct: bool,
    /// `vc_softParticles`: every fx run reads the depth copy.
    pub(super) soft_all: bool,
    /// A distortion draws: the split takes the colour copy.
    pub(super) distort_on: bool,
    /// A draw reads the depth copy.
    pub(super) soft_on: bool,
    /// A CoD2 feather reads a depth without the entities.
    pub(super) feathered: bool,
    /// `r_vc_depthprepass` runs, and takes the entities.
    pub(super) prepass_on: bool,
    pub(super) ent_prepass: bool,
    /// The prepass writes `floatZ`, in place of the depth copy.
    pub(super) floatz: bool,
    /// The world runs, and the entity runs and the prepass, replay from
    /// bundles.
    pub(super) bundling: bool,
    pub(super) ent_bundling: bool,
    /// The scene pass splits after its last opaque or decal draw.
    pub(super) split: bool,
    /// The shellshock's decay and CoD4's shellshock screen.
    pub(super) shock: Option<i32>,
    pub(super) cod4_shock: Option<crate::play::cod4_shock::Screen>,
    /// CoD2's glow and a `blurWorld` menu's blur.
    pub(super) glow: Option<crate::cod2_glow::Plan>,
    pub(super) world_blur: Option<crate::cod2_glow::BlurPlan>,
    /// A pass follows the scene pass: the 2D draws single-sampled.
    pub(super) after: bool,
}

impl Renderer {
    /// The frame's [`FramePlan`]. It takes this frame's depth of field and
    /// CoD4 shellshock screen, points the bloom and the shafts at the post
    /// pass's target when that changes, and writes the shafts' parameters.
    pub(super) fn plan_frame(
        &mut self,
        frame: &Frame,
        ssao_on: bool,
        shadow_on: bool,
    ) -> FramePlan {
        // Linear fog clears to the fog colour (docs/protocol-1.1.md,
        // configstring 12);
        // otherwise the fixed steel blue stands in until the sky draws.
        let clear = if self.fog.set && self.fog.live.linear {
            wgpu::Color {
                r: self.fog.live.color[0] as f64,
                g: self.fog.live.color[1] as f64,
                b: self.fog.live.color[2] as f64,
                a: 1.0,
            }
        } else {
            wgpu::Color {
                r: 0.35,
                g: 0.42,
                b: 0.50,
                a: 1.0,
            }
        };
        let bloom = self.bloom_intensity > 0.0;
        let hdr = self.hdr();
        // With vc_hdr or a grade the world resolves into the post
        // pass's target; otherwise straight into the gamma scene.
        let film = self
            .level_look
            .and_then(|l| l.film)
            .filter(|_| self.cod4_options.film);
        // a CoD4 glow: the setup reads the film off or on, and the post
        // pass adds the result after the film
        let glow4 = self
            .level_look
            .and_then(|l| l.glow)
            .filter(|_| self.cod4_options.glow)
            .and_then(|g| {
                let consts = film.unwrap_or(crate::post::Film::OFF);
                crate::cod2_glow::plan_cod4(
                    &g,
                    consts.setup(self.identity_light),
                    self.config.width,
                    self.config.height,
                )
                .map(|p| (p, g.intensity))
            });
        // CoD4's depth of field blurs the resolved world before the
        // film and the glow (`RB_ProcessPostEffects`)
        let dof4 = self
            .cod4_dof
            .take()
            .filter(|d| self.cod4_options.dof && self.world.is_some() && d.used());
        let post =
            self.post_settings.active(hdr) || film.is_some() || glow4.is_some() || dof4.is_some();
        if post != self.effects_read_post {
            self.effects_read_post = post;
            let src = if post {
                self.post.world_view()
            } else {
                self.gamma.scene_view()
            };
            let (w, h) = (self.config.width, self.config.height);
            self.bloom.resize(&self.device, src, w, h);
            self.shafts
                .resize(&self.device, src, &self.depth_view, w, h);
        }
        // An identity ramp leaves the gamma pass a copy, which 1.7 never
        // makes (its ramp is the device's): the post pass and the 2D then
        // draw straight into the swapchain (cod4-client-perf.md 30). It
        // needs the swapchain in the scene's format and opaque (the 2D
        // blends write alpha the gamma pass sets to 1), and nothing that
        // samples or copies the gamma scene: without the post pass the
        // glow, blur, bloom and shafts work in it (`scene_direct` below),
        // and a shellshock captures it.
        let swap_direct = self.swap_direct
            && self.gamma.identity()
            && self.config.format == SCENE_FORMAT
            && (matches!(self.target, Target::Offscreen(_))
                || self.config.alpha_mode == wgpu::CompositeAlphaMode::Opaque)
            && self.shock_blend.is_none()
            && !matches!(
                self.cod4_shock,
                Some(
                    crate::play::cod4_shock::Screen::Save
                        | crate::play::cod4_shock::Screen::Blur { .. }
                )
            );
        let gamma_direct = post && swap_direct;
        // Shafts need a sun flare's direction (shader doc 8.7).
        let shafts = self
            .sky
            .as_ref()
            .and_then(|s| s.flare.as_ref())
            .is_some_and(|f| {
                self.shafts.prepare(
                    &self.queue,
                    frame.view_proj,
                    f.dir(),
                    (self.config.width as f32, self.config.height as f32),
                    self.identity_light,
                )
            });
        // A CoD1 or UO frame at an identity ramp (windowed, `r_gamma` 1)
        // with no post pass resolves the scene and draws the 2D straight
        // into the swapchain, as retail's ramp is the device's
        // (cod11-client-perf.md 4): only while nothing reads the gamma
        // scene (bloom, shafts, CoD2's glow and blur) or draws into the
        // split's scene (vc_ssao, vc_shadows).
        let scene_direct = swap_direct
            && !post
            && !bloom
            && !shafts
            && !ssao_on
            && !shadow_on
            && self.world_fx.is_none()
            && self.world.as_ref().is_none_or(|w| w.cod2.is_none());
        // only worth the split when an fx run that reads the depth
        // copy draws after it: every run under vc_softParticles, a
        // feathered one always, a CoD4 distortion one under
        // r_distortion, which reads the colour copy too
        // (`resolvedPostSun`)
        let soft_all = self.soft.on();
        // and a world feathered or distortion stage (`cod4_worldfx`)
        let (world_feather, world_distort) = self
            .world
            .as_ref()
            .map_or((false, false), |w| self.world_fx_drawn(w));
        let distort_on = self.world.is_some()
            && self.cod4_options.distortion
            && (world_distort
                || self
                    .fx
                    .runs
                    .iter()
                    .any(|r| r.sort > SORT_DECAL && r.distort));
        let soft_on = distort_on
            || world_feather && self.cod4_options.zfeather
            || self.world.is_some()
                && self
                    .fx
                    .runs
                    .iter()
                    .any(|r| r.sort > SORT_DECAL && (soft_all || r.feather));
        // CoD2's feather reads a depth without the entities (floatZ,
        // cod2-materials.md 9.5): its copy goes before the first one.
        // CoD4's `floatZ` holds them (cod4-client-perf.md 20), so a
        // CoD4 feather copies at the split and keeps them in the
        // prepass.
        let feathered = self.fx.runs.iter().any(|r| r.feather && !r.view_depth);
        // `r_vc_depthprepass` (cod4-client-perf.md 12): the opaque lit
        // world draws and the entities' opaque first stages lay their
        // depth down in a pass of their own, then draw their colour
        // under EQUAL in place. Every other draw keeps its state, so
        // the frame comes out the same. The entities stay out while a
        // feather's depth copy must hold the world alone.
        let prepass_on = self.depth_prepass && self.world.is_some();
        let ent_prepass = prepass_on && !feathered;
        // A CoD4 frame a feather reads writes `floatZ` in the prepass,
        // as 1.7's does (cod4-client-perf.md 35), and keeps no split or
        // copy for it. One sample only: the copy reads sample 0 of an
        // MSAA depth, which a fragment's write does not give. With
        // `r_zFeather` 0 and the distortion alone the frame is as
        // before.
        let feather_on = self.cod4_options.zfeather
            && (world_feather
                || self
                    .fx
                    .runs
                    .iter()
                    .any(|r| r.sort > SORT_DECAL && r.feather));
        let floatz = self.floatz_prepass
            && ent_prepass
            && feather_on
            && !soft_all
            && msaa_samples() == 1
            && self.dynamic.cod4_light.is_some();
        // CoD4's entity runs and depth prepass replay from bundles as its
        // world runs do (cod4-client-perf.md 40)
        let ent_bundling = self.ent_bundles_on && self.dynamic.cod4_light.is_some();
        // the scene pass splits for the shadow mask, the SSAO multiply
        // and the copies (`ssao_split`)
        let split = ssao_on || shadow_on || distort_on || soft_on && !floatz;
        // CoD4's world runs replay from bundles (cod4-client-perf.md 31): a
        // run is one `Frag::World` kind for the fragment counts, and one GPU
        // group, its first item's, for the profile's stamps
        let bundling = self.world_bundles_on && self.dynamic.cod4_light.is_some();
        let shock = self.shock_blend;
        // A CoD4 session sets it every frame it draws.
        let cod4_shock = self.cod4_shock.take();
        // CoD2's r_glow, on at its default on every CoD2 map; a CoD4
        // level's own
        let sky_cube = self
            .sky
            .as_ref()
            .and_then(|s| s.farbox.as_ref())
            .and_then(|f| f.cube.as_ref());
        let glow = self
            .world
            .as_ref()
            .is_some_and(|w| w.cod2.is_some())
            .then(|| match &glow4 {
                Some((p, _)) => Some(p.clone()),
                None if self.level_look.is_some() => None,
                None => crate::cod2_glow::plan(
                    &Default::default(),
                    self.config.width,
                    self.config.height,
                    sky_cube.is_some(),
                ),
            })
            .flatten();
        // a `blurWorld` menu's blur, after the glow (cod2-materials.md 9.13)
        let world_blur = self
            .world
            .as_ref()
            .is_some_and(|w| w.cod2.is_some())
            .then(|| {
                crate::cod2_glow::blur_plan(self.world_blur, self.config.width, self.config.height)
            })
            .flatten();
        // A CoD4 shellshock's capture and blends run single-sampled
        // with no depth, as the CoD1 and CoD2 one does.
        let cod4_screen = matches!(
            cod4_shock,
            Some(
                crate::play::cod4_shock::Screen::Save
                    | crate::play::cod4_shock::Screen::Blur { .. }
                    | crate::play::cod4_shock::Screen::Flash { .. }
            )
        );
        let after = bloom
            || shafts
            || post
            || shock.is_some()
            || cod4_screen
            || glow.is_some()
            || world_blur.is_some();
        FramePlan {
            clear,
            bloom,
            hdr,
            film,
            glow4,
            dof4,
            post,
            gamma_direct,
            shafts,
            scene_direct,
            soft_all,
            distort_on,
            soft_on,
            feathered,
            prepass_on,
            ent_prepass,
            floatz,
            bundling,
            ent_bundling,
            split,
            shock,
            cod4_shock,
            glow,
            world_blur,
            after,
        }
    }

    /// The visible set and the prop flags that go with it. A placement whose
    /// model failed to load has zero bounds and is never drawn or counted.
    /// `lod` is CoD4's rigid LOD ramp, `(scale, bias)`
    /// ([`vcod_common::mapworld::cod4_lod_ramp`]): each static model keeps
    /// the soups of the LOD it picks, none past its `cullDist`.
    pub(super) fn cull(
        world: &WorldGpu,
        eye: glam::Vec3,
        view_proj: &glam::Mat4,
        frustum: &Frustum,
        lod: (f32, f32),
    ) -> (Visible, Vec<bool>) {
        let mut v = world.vis.visible(eye, frustum, Some(*view_proj));
        for sl in &world.static_lods {
            let (Some(&(_, first, _)), Some(&(_, last, n))) = (sl.lods.first(), sl.lods.last())
            else {
                continue;
            };
            let all = first as usize..(last + n) as usize;
            if !v.soups[all.clone()].iter().any(|&s| s) {
                continue;
            }
            let keep = sl.pick(eye.into(), lod.0, lod.1).map(|l| {
                let (_, f, n) = sl.lods[l];
                f as usize..(f + n) as usize
            });
            for si in all {
                v.soups[si] = keep.as_ref().is_some_and(|k| k.contains(&si));
            }
        }
        let prop_ok = world
            .prop_bounds
            .iter()
            .map(|&(lo, hi)| {
                (lo, hi) != (glam::Vec3::ZERO, glam::Vec3::ZERO)
                    && world.vis.prop_visible(&v, frustum, lo, hi)
            })
            .collect();
        (v, prop_ok)
    }

    /// `vm` is `None` to draw the world only. Skips the frame if no
    /// swapchain texture can be acquired.
    pub fn render(&mut self, frame: Frame, vm: Option<VmDraw>) {
        #[cfg(test)]
        self.frame_writes.borrow_mut().clear();
        let lod_view = model::LodView::new(
            frame.fov_x,
            crate::camera::fov_y(frame.fov_x, self.aspect()),
            self.lod_cvars.0,
            self.lod_cvars.1,
        );
        self.pick_dynamic_lods(frame.eye, &lod_view, frame.time);
        let (camera, moved, ssao_on) = self.upload_frame_uniforms(&frame);
        let t0 = std::time::Instant::now();
        let mut prof = crate::frame_prof::span(crate::frame_prof::Stage::Cull);
        self.cull_and_gather(&frame, &lod_view, t0, &mut prof);
        let hud_quads = self.upload_hud_text(&frame);
        self.upload_entities(&frame);
        let draw_vm = self.upload_view_model(&frame, vm.as_ref());
        let ent_draws = self.frame_ent_draws(&frame, draw_vm);
        prof.next(crate::frame_prof::Stage::Acquire);
        let Some((surface_tex, view)) = self.acquire() else {
            return;
        };
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("frame"),
            });
        prof.next(crate::frame_prof::Stage::Encode);
        let prof_slot = self.prof_gpu.as_mut().and_then(|g| g.begin());
        if let Some(f) = &mut self.frag_stats {
            f.begin_frame(crate::frame_prof::frame());
        }
        // out of `self` for the frame, so a stamp never holds `self`
        let mut gpu_groups = self.gpu_groups.take();
        if let Some(g) = &mut gpu_groups {
            g.begin_frame();
        }
        // The profile's opaque and split stamps go at the split, the blend's
        // at the post pass; a frame without one writes them later, together.
        let (mut split_stamped, mut post_stamped) = (false, false);
        if let Some(g) = &self.prof_gpu {
            g.stamp(prof_slot, &mut encoder, 0);
        }
        let (shadow_on, receivers_on_prepass) =
            self.encode_shadow_passes(&mut encoder, &frame, (&camera, &moved), prof_slot);
        {
            let plan = self.plan_frame(&frame, ssao_on, shadow_on);
            if let Some(flare) = self
                .sky
                .as_mut()
                .and_then(|s| s.flare.as_mut())
                .filter(|_| self.flares)
            {
                flare.prepare(
                    &self.device,
                    &self.queue,
                    &frame,
                    (self.config.width as f32, self.config.height as f32),
                    self.identity_light,
                    self.sky_drawn && self.draw_sun,
                );
            }
            self.gamma_skipped = plan.gamma_direct || plan.scene_direct;
            // The world's draws across the world and every moved brush
            // model, by their place in the sorted shader list; on one
            // shader a moved model, an entity in retail's key, draws before
            // the world (entity 0x3fe, shader doc 5).
            let mut world_list: Vec<WorldItem> = Vec::new();
            if let Some(world) = &self.world {
                for (set, (_, frame_draws, _)) in self.world_sets(world).enumerate() {
                    for (i, fd) in frame_draws.iter().enumerate() {
                        world_list.push(WorldItem {
                            sort: fd.sort,
                            rank: fd.rank,
                            set,
                            draw: i as u32,
                        });
                    }
                }
            }
            // stable: a batch keeps its stage order
            world_list.sort_by_key(|w| (w.rank, w.set.wrapping_sub(1)));
            // the world draws the prepass laid down untested, which draw
            // their colour under EQUAL: a `floatZ` prepass takes only 1.7's
            // set
            let world_equal = |world: &WorldGpu, kind: DrawRef| {
                plan.prepass_on
                    && takes_prepass(world, kind)
                    && (!plan.floatz || takes_floatz(world, kind) == Some(false))
            };
            // the entity skins whose first stage the prepass laid down
            let ent_equal = |skin: &EntSkin| {
                plan.ent_prepass
                    && ent_takes_prepass(skin)
                    && (!plan.floatz || skin.stages.first().is_some_and(|st| st.floatz))
            };
            self.census.set(PassCensus::default());
            self.encode_prepass(
                &mut encoder,
                &frame,
                &plan,
                (&world_list, &ent_draws),
                receivers_on_prepass,
                &mut gpu_groups,
            );
            // With SSAO or vc_shadows the frame is two passes split after
            // the last opaque or decal draw (`ssao_split`): the first keeps
            // its MSAA colour for the shadow mask and the SSAO multiply, the
            // second loads it and resolves.
            let targets = SceneTargets {
                msaa: &self.msaa_view,
                depth: &self.depth_view,
                resolve: if plan.post {
                    self.post.world_view()
                } else if plan.scene_direct {
                    &view
                } else {
                    self.gamma.scene_view()
                },
                timer: self.ssao.timer(),
                clear: plan.clear,
                sun_queries: self
                    .sky
                    .as_ref()
                    .and_then(|s| s.flare.as_ref())
                    .and_then(SunFlareGpu::query_set),
                depth_cleared: plan.prepass_on,
            };
            if let Some(g) = &mut gpu_groups {
                g.enc(&mut encoder, Some(crate::frame_prof::Group::Sky));
            }
            let mut pass = targets.begin(&mut encoder, true, !plan.split);
            frame_write!(
                self,
                "scene pass (colour + depth)",
                self.scene_pass_bytes(!plan.split)
            );
            pass.set_bind_group(0, &self.camera_bg, &[]);
            self.draw_sky(&mut pass, &frame);
            // One list, as retail's draw list keys every surface (shader doc
            // 5): the world's draws, the entity and viewmodel surfaces and
            // the fx runs merged by material sort, the world first on a
            // tie, then the models, then the fx. The gun keeps its
            // depth-hack range (renderfx 0x8c: the front fifth), so what
            // draws after it behind it fails the depth test. The flat list
            // mixes pipeline layouts; rebind only on change. An fx run
            // whose texture failed to resolve is skipped.
            let (vw, vh) = (self.config.width as f32, self.config.height as f32);
            #[cfg(test)]
            let ent_draws = {
                let mut d = ent_draws;
                if let Some((i, s)) = self.ent_only.get() {
                    d.retain(|e| e.gun || e.inst == i && s.is_none_or(|s| e.surf == s));
                }
                *self.ent_census.borrow_mut() = d
                    .iter()
                    .filter(|e| !e.gun)
                    .map(|e| {
                        let model = &self.dynamic.models[self.dynamic.instances[e.inst as usize].0];
                        let skin = &model.skins[e.surf as usize];
                        let surf = &model.vm.surfaces[e.surf as usize];
                        let equal = ent_equal(skin);
                        let takes = ent_takes_prepass(skin);
                        (
                            e.inst,
                            e.surf,
                            surf.index_count / 3,
                            skin.stages.len(),
                            equal,
                            takes,
                        )
                    })
                    .collect();
                d
            };
            let world_sorts: Vec<f32> = world_list.iter().map(|w| w.sort).collect();
            let model_sorts: Vec<f32> = ent_draws.iter().map(|d| d.sort).collect();
            let fx_sorts: Vec<f32> = self.fx.runs.iter().map(|r| r.sort).collect();
            let fx_firsts: Vec<usize> = self
                .fx
                .runs
                .iter()
                .scan(0, |first, run| {
                    let at = *first;
                    *first += run.quads;
                    Some(at)
                })
                .collect();
            let mark_sorts: Vec<f32> = self.marks.runs.iter().map(|r| r.sort).collect();
            let items = merge_by_sort(&world_sorts, &mark_sorts, &model_sorts, &fx_sorts);
            crate::frame_prof::count(crate::frame_prof::Count::Draws, items.len());
            let mut dlight_at = dlight_slot(&items, &world_sorts);
            let split_at = plan
                .split
                .then(|| ssao_split(&items, &world_sorts, &mark_sorts, &model_sorts, &fx_sorts));
            let copy_at = split_at
                .filter(|_| !plan.floatz)
                .map(|at| match plan.feathered {
                    true => feather_copy_at(&items, at),
                    false => at,
                });
            // where the effects may read the feather's depth from
            let soft_from = if plan.floatz { Some(0) } else { copy_at };
            // the dlights add light, so a shadow must not darken them
            if shadow_on && let Some(at) = split_at {
                dlight_at = dlight_at.max(at);
            }
            let mut emit = Emit::None;
            // Past the items an entity surface run or a world bundle drew.
            let mut skip_to = 0;
            let mut bundles = std::mem::take(&mut self.world_bundles);
            bundles.begin(Some(self.world_format));
            // The items where the loop below ends a pass or draws the
            // dlights: a world run stops short of them.
            let dlights_drawn = self
                .world
                .as_ref()
                .is_some_and(|w| !w.dlight_runs.is_empty());
            let stops = [
                copy_at.filter(|_| plan.soft_on && copy_at != split_at),
                split_at,
                dlights_drawn.then_some(dlight_at),
            ];
            // a superset of `stops`, so no entity run crosses a run's end
            let ent_stops = [copy_at, split_at, Some(dlight_at)];
            // the run from `k`: the items up to the next stop or one `joins`
            // turns down
            let run_end = |k: usize, joins: &dyn Fn(SortedItem) -> bool| {
                let mut end = k + 1;
                while items.get(end).is_some_and(|&item| joins(item)) && !stops.contains(&Some(end))
                {
                    end += 1;
                }
                end
            };
            // whether this frame's depth and colour copies are taken by item
            // `k` ([`Self::emit_world_draw`])
            let fx_ready = |k: usize| {
                (
                    plan.soft_on
                        && self.cod4_options.zfeather
                        && soft_from.is_some_and(|at| k >= at),
                    plan.distort_on && split_at.is_some_and(|at| k >= at),
                )
            };
            // whether fx run `r` at item `k` draws on the soft pipelines,
            // reading the depth copy: only after the split is it this frame's
            let fx_soft = |r: &FxRun, k: usize| {
                plan.soft_on
                    && (plan.soft_all || r.feather)
                    && soft_from.is_some_and(|at| k >= at)
                    && !r.distort
                    && r.cloud.is_none()
            };
            for k in 0..=items.len() {
                if k < skip_to {
                    continue;
                }
                if plan.soft_on && copy_at == Some(k) && copy_at != split_at {
                    if let Some(f) = &mut self.frag_stats {
                        f.switch(&mut pass, None);
                    }
                    drop(pass);
                    if let Some(g) = &mut gpu_groups {
                        g.enc(&mut encoder, Some(crate::frame_prof::Group::SoftCopy));
                    }
                    self.soft.encode(&mut encoder);
                    frame_write!(self, "feather depth copy (R32F)", self.target_bytes(4));
                    pass = targets.begin(&mut encoder, false, false);
                    frame_write!(
                        self,
                        "scene pass after the depth copy",
                        self.scene_pass_bytes(false)
                    );
                    pass.set_bind_group(0, &self.camera_bg, &[]);
                    emit = Emit::None;
                }
                // Before the dlights, so their light is not occluded; the
                // shadow mask before the SSAO multiply.
                if split_at == Some(k) {
                    if let Some(f) = &mut self.frag_stats {
                        f.switch(&mut pass, None);
                    }
                    drop(pass);
                    if let Some(g) = &self.prof_gpu {
                        g.stamp(prof_slot, &mut encoder, 4);
                    }
                    let scene = if msaa_samples() > 1 {
                        &self.msaa_view
                    } else {
                        targets.resolve
                    };
                    use crate::frame_prof::Group;
                    let mut group = |encoder: &mut wgpu::CommandEncoder, g| {
                        if let Some(gg) = &mut gpu_groups {
                            gg.enc(encoder, Some(g));
                        }
                    };
                    if shadow_on && let Some(sp) = &self.shadows {
                        group(&mut encoder, Group::ShadowMask);
                        sp.encode_mask(&self.device, &mut encoder, scene, &self.depth_view);
                        frame_write!(self, "vc_shadows mask (not sized)", 0);
                    }
                    if ssao_on {
                        group(&mut encoder, Group::Ssao);
                        self.ssao.encode(&mut encoder, scene, &self.depth_view);
                        frame_write!(self, "vc_ssao (not sized)", 0);
                    }
                    if plan.soft_on && copy_at == split_at {
                        group(&mut encoder, Group::SoftCopy);
                        self.soft.encode(&mut encoder);
                        frame_write!(self, "feather depth copy (R32F)", self.target_bytes(4));
                    }
                    if plan.distort_on {
                        group(&mut encoder, Group::SceneCopy);
                        self.scene_copy.encode(&self.device, &mut encoder, scene);
                        frame_write!(
                            self,
                            "distortion colour copy",
                            self.target_bytes(self.world_bpp())
                        );
                    }
                    if let Some(g) = &self.prof_gpu {
                        g.stamp(prof_slot, &mut encoder, 5);
                    }
                    split_stamped = true;
                    pass = targets.begin(&mut encoder, false, true);
                    frame_write!(
                        self,
                        "scene pass after the split",
                        self.scene_pass_bytes(true)
                    );
                    pass.set_bind_group(0, &self.camera_bg, &[]);
                    emit = Emit::None;
                }
                if let Some(world) = &self.world
                    && dlight_at == k
                    && !world.dlight_runs.is_empty()
                {
                    if matches!(emit, Emit::Gun { .. }) {
                        pass.set_viewport(0.0, 0.0, vw, vh, 0.0, 1.0);
                    }
                    if let Some(f) = &mut self.frag_stats {
                        f.switch(&mut pass, Some(crate::frame_prof::Frag::Dlight));
                    }
                    if let Some(g) = &mut gpu_groups {
                        g.pass(&mut pass, Some(crate::frame_prof::Group::Dlight));
                    }
                    self.emit_dlights(&mut pass, world, frame.time);
                    emit = Emit::None;
                }
                let Some(&item) = items.get(k) else {
                    break;
                };
                let prop = self.frag_stats.is_some() && self.is_prop_draw(item, &world_list);
                if let Some(f) = &mut self.frag_stats {
                    use crate::frame_prof::Frag;
                    let kind = match item {
                        SortedItem::World(_) if prop => Frag::Props,
                        SortedItem::World(_) => Frag::World,
                        SortedItem::Model(mi) if ent_draws[mi].gun => Frag::Gun,
                        SortedItem::Model(_) => Frag::Ents,
                        SortedItem::Mark(_) => Frag::Marks,
                        SortedItem::Fx(fi) if fx_soft(&self.fx.runs[fi], k) => Frag::FxSoft,
                        SortedItem::Fx(_) => Frag::Fx,
                    };
                    f.switch(&mut pass, Some(kind));
                }
                if gpu_groups.is_some() {
                    let g = self.draw_group(item, &world_list, &ent_draws, |fi| {
                        fx_soft(&self.fx.runs[fi], k)
                    });
                    if let Some(gg) = &mut gpu_groups {
                        gg.pass(&mut pass, Some(g));
                    }
                }
                match item {
                    SortedItem::World(_) => {
                        let world = self.world.as_ref().expect("world draws come from a world");
                        if plan.bundling {
                            // the run, recorded as one item at a time would
                            // encode it from `Emit::None`
                            let end = run_end(k, &|item| matches!(item, SortedItem::World(_)));
                            if matches!(emit, Emit::Gun { .. }) {
                                pass.set_viewport(0.0, 0.0, vw, vh, 0.0, 1.0);
                            }
                            let mut rec = Recording::default();
                            self.encode_world_items(
                                &mut rec,
                                world,
                                &world_list,
                                &items[k..end],
                                None,
                                frame.time,
                                fx_ready(k),
                                world_equal,
                            );
                            bundles.draw(
                                &mut pass,
                                &self.device,
                                rec,
                                crate::frame_prof::Count::Bundled,
                            );
                            // a bundle leaves the pass's pipeline, groups and
                            // buffers unset
                            skip_to = end;
                            emit = Emit::None;
                        } else {
                            let at = match emit {
                                Emit::World(at) => Some(at),
                                other => {
                                    if matches!(other, Emit::Gun { .. }) {
                                        pass.set_viewport(0.0, 0.0, vw, vh, 0.0, 1.0);
                                    }
                                    None
                                }
                            };
                            emit = Emit::World(self.encode_world_items(
                                &mut pass,
                                world,
                                &world_list,
                                &items[k..=k],
                                at,
                                frame.time,
                                fx_ready(k),
                                world_equal,
                            ));
                        }
                    }
                    SortedItem::Model(mi) if !ent_draws[mi].gun => {
                        if plan.ent_bundling {
                            // the entity runs, recorded as the items one at a
                            // time would encode them from `Emit::None`
                            let end = run_end(
                                k,
                                &|item| matches!(item, SortedItem::Model(m) if !ent_draws[m].gun),
                            );
                            if matches!(emit, Emit::Gun { .. }) {
                                pass.set_viewport(0.0, 0.0, vw, vh, 0.0, 1.0);
                            }
                            let mut rec = Recording::default();
                            self.encode_ent_items(
                                &mut rec,
                                &items,
                                (k, end),
                                &ent_draws,
                                &ent_stops,
                                None,
                                ent_equal,
                            );
                            bundles.draw(
                                &mut pass,
                                &self.device,
                                rec,
                                crate::frame_prof::Count::EntBundled,
                            );
                            skip_to = end;
                            emit = Emit::None;
                        } else {
                            let at = match emit {
                                Emit::Ent(at) => Some(at),
                                other => {
                                    if matches!(other, Emit::Gun { .. }) {
                                        pass.set_viewport(0.0, 0.0, vw, vh, 0.0, 1.0);
                                    }
                                    None
                                }
                            };
                            let (at, next) = self.encode_ent_items(
                                &mut pass,
                                &items,
                                (k, k + 1),
                                &ent_draws,
                                &ent_stops,
                                at,
                                ent_equal,
                            );
                            skip_to = next;
                            emit = Emit::Ent(at);
                        }
                    }
                    SortedItem::Model(mi) => {
                        let d = &ent_draws[mi];
                        let vmp = &self.vm_pass;
                        let model = &vmp.models[d.inst as usize];
                        let (same_part, mut bound) = match emit {
                            Emit::Gun { part, bound } => (part == d.inst, bound),
                            _ => {
                                pass.set_viewport(0.0, 0.0, vw, vh, 0.0, VM_DEPTH_RANGE);
                                pass.set_bind_group(0, &vmp.uniform_bg, &[]);
                                tally(&self.census, |c| c.bind_groups += 1);
                                (false, EntBound::default())
                            }
                        };
                        if !same_part {
                            pass.set_vertex_buffer(0, model.vertex_buf.slice(..));
                            pass.set_index_buffer(
                                model.index_buf.slice(..),
                                wgpu::IndexFormat::Uint16,
                            );
                            let bones = model.bones.as_ref().expect("view model bones");
                            pass.set_bind_group(2, &bones.bg, &[]);
                            tally(&self.census, |c| c.bind_groups += 1);
                        }
                        let surf = &model.surfaces[d.surf as usize];
                        draw_ent_stages(
                            &mut pass,
                            &self.dynamic.materials,
                            EntPass::View,
                            &model.bind_groups[surf.bind_group],
                            &vmp.skins[d.inst as usize][d.surf as usize],
                            surf,
                            (surf.first_index..surf.first_index + surf.index_count, 0),
                            &mut bound,
                            Depth::Plain,
                            &self.census,
                        );
                        emit = Emit::Gun {
                            part: d.inst,
                            bound,
                        };
                    }
                    SortedItem::Mark(ri) => {
                        if matches!(emit, Emit::Gun { .. }) {
                            pass.set_viewport(0.0, 0.0, vw, vh, 0.0, 1.0);
                        }
                        pass.set_bind_group(0, &self.camera_bg, &[]);
                        let n = self.marks.draw(&mut pass, ri, frame.time);
                        tally(&self.census, |c| {
                            c.marks += n;
                            c.pipelines += n;
                            c.bind_groups += 1 + 2 * n;
                        });
                        emit = Emit::None;
                    }
                    SortedItem::Fx(fi) => {
                        let fx = &self.fx;
                        let FxRun {
                            image,
                            blend,
                            quads: count,
                            ..
                        } = &fx.runs[fi];
                        let Some(Some(bg)) = fx.textures.get(image) else {
                            continue;
                        };
                        if fx.runs[fi].distort {
                            // after the split only, where the copies are
                            // this frame's (retail skips the draw without
                            // r_distortion: `R_SetupMaterial`)
                            if !plan.distort_on || split_at.is_none_or(|at| k < at) {
                                continue;
                            }
                            if matches!(emit, Emit::Gun { .. }) {
                                pass.set_viewport(0.0, 0.0, vw, vh, 0.0, 1.0);
                            }
                            pass.set_bind_group(0, &self.camera_bg, &[]);
                            pass.set_bind_group(1, bg, &[]);
                            pass.set_bind_group(2, self.scene_copy.bind_group(), &[]);
                            pass.set_vertex_buffer(0, fx.vertex_buf.slice(..));
                            pass.set_index_buffer(
                                fx.index_buf.slice(..),
                                wgpu::IndexFormat::Uint32,
                            );
                            pass.set_pipeline(&fx.distort_pipeline);
                            let start = (fx_firsts[fi] * 6) as u32;
                            let end = ((fx_firsts[fi] + count) * 6) as u32;
                            pass.draw_indexed(start..end, 0, 0..1);
                            tally(&self.census, |c| {
                                c.fx += 1;
                                c.pipelines += 1;
                                c.bind_groups += 3;
                            });
                            emit = Emit::None;
                            continue;
                        }
                        if let Some(instances) = fx.runs[fi].cloud.clone() {
                            if matches!(emit, Emit::Gun { .. }) {
                                pass.set_viewport(0.0, 0.0, vw, vh, 0.0, 1.0);
                            }
                            let vp = frame.view_proj;
                            fx.clouds
                                .draw(&mut pass, &self.camera_bg, bg, *blend, instances, vp);
                            tally(&self.census, |c| {
                                c.fx += 1;
                                c.pipelines += 1;
                                c.bind_groups += 3;
                            });
                            emit = Emit::None;
                            continue;
                        }
                        // a distortion or cloud run went on above
                        let soft = fx_soft(&fx.runs[fi], k);
                        let bound = match emit {
                            Emit::Fx { blend, soft: s } => {
                                if soft && !s {
                                    pass.set_bind_group(2, self.soft.fx_bind_group(), &[]);
                                    tally(&self.census, |c| c.bind_groups += 1);
                                }
                                Some((blend, s))
                            }
                            other => {
                                if matches!(other, Emit::Gun { .. }) {
                                    pass.set_viewport(0.0, 0.0, vw, vh, 0.0, 1.0);
                                }
                                pass.set_bind_group(0, &self.camera_bg, &[]);
                                tally(&self.census, |c| c.bind_groups += 1);
                                if soft {
                                    pass.set_bind_group(2, self.soft.fx_bind_group(), &[]);
                                    tally(&self.census, |c| c.bind_groups += 1);
                                }
                                pass.set_vertex_buffer(0, fx.vertex_buf.slice(..));
                                pass.set_index_buffer(
                                    fx.index_buf.slice(..),
                                    wgpu::IndexFormat::Uint32,
                                );
                                None
                            }
                        };
                        if bound != Some((*blend, soft)) {
                            let set = if soft {
                                &fx.soft_pipelines
                            } else {
                                &fx.pipelines
                            };
                            pass.set_pipeline(&set[*blend as usize]);
                            tally(&self.census, |c| c.pipelines += 1);
                        }
                        pass.set_bind_group(1, bg, &[]);
                        let start = (fx_firsts[fi] * 6) as u32;
                        let end = ((fx_firsts[fi] + count) * 6) as u32;
                        pass.draw_indexed(start..end, 0, 0..1);
                        tally(&self.census, |c| {
                            c.fx += 1;
                            c.bind_groups += 1;
                        });
                        emit = Emit::Fx {
                            blend: *blend,
                            soft,
                        };
                    }
                }
            }
            bundles.end();
            self.world_bundles = bundles;
            if let Some(f) = &mut self.frag_stats {
                f.switch(&mut pass, None);
            }
            let census = self.census.get();
            crate::frame_prof::count(
                crate::frame_prof::Count::Pipelines,
                census.pipelines as usize,
            );
            crate::frame_prof::count(
                crate::frame_prof::Count::BindGroups,
                census.bind_groups as usize,
            );
            crate::frame_prof::count(crate::frame_prof::Count::Calls, census.draws() as usize);
            crate::frame_prof::count(
                crate::frame_prof::Count::EntCalls,
                (census.ents + census.prepass_ents) as usize,
            );
            if matches!(emit, Emit::Gun { .. }) {
                pass.set_viewport(0.0, 0.0, vw, vh, 0.0, 1.0);
            }
            // The sun's flare and washes go over the finished scene, before
            // the 2D (0x4d9770).
            if let Some(flare) = self.sky.as_ref().and_then(|s| s.flare.as_ref())
                && self.flares
            {
                if let Some(g) = &mut gpu_groups {
                    g.pass(&mut pass, Some(crate::frame_prof::Group::Flare));
                }
                flare.draw(&mut pass);
            }
            pass.set_bind_group(0, &self.camera_bg, &[]);

            // The world is resolved: add the shafts, blur it, add the glow
            // (and a CoD2 menu's blur) into the resolved world, run the post pass into the gamma
            // scene when it is on, and draw the HUD there single-sampled, so
            // the HUD never blooms, shafts or takes the grade and the MSAA
            // target is never reloaded.
            if plan.after {
                drop(pass);
                if let Some(g) = &self.prof_gpu {
                    if !split_stamped {
                        g.stamp(prof_slot, &mut encoder, 4);
                        g.stamp(prof_slot, &mut encoder, 5);
                    }
                    g.stamp(prof_slot, &mut encoder, 6);
                    split_stamped = true;
                    post_stamped = true;
                }
                pass = self.encode_post_chain(&mut encoder, &frame, &plan, &view, &mut gpu_groups);
            }
            // The shellshock's render commands 10 and 9 on the finished 3D
            // view, before the 2D, as the cgame queues them.
            use crate::play::cod4_shock::Screen as Cod4Screen;
            if (!matches!(plan.cod4_shock, None | Some(Cod4Screen::Off)) || plan.shock.is_some())
                && let Some(g) = &mut gpu_groups
            {
                g.pass(&mut pass, Some(crate::frame_prof::Group::Shock));
            }
            // a blend's decay and desaturation: the last capture over the 3D
            // view, captured again after
            let blend = match (plan.cod4_shock, plan.shock) {
                // CoD4's flashbang: the first frame saves the still, the
                // later ones add it with the white (cod4-client-view.md 8).
                (Some(Cod4Screen::Save), _) => {
                    drop(pass);
                    self.screen_blend.capture(&mut encoder);
                    frame_write!(self, "screen capture", self.target_bytes(4));
                    pass = single_pass(&mut encoder, self.gamma.scene_view(), "hud pass");
                    None
                }
                (Some(Cod4Screen::Flash { white, grab }), _) => {
                    self.screen_blend.flash(&self.queue, &mut pass, white, grab);
                    None
                }
                (Some(Cod4Screen::Blur { decay_ms }), _) => {
                    Some((decay_ms, crate::screen_blend::COD2_DESATURATE))
                }
                (Some(Cod4Screen::Off), _) => {
                    self.screen_blend.drop_capture();
                    None
                }
                // CoD2 draws the capture through `shellshock`
                (None, Some(decay)) => {
                    let cod2 = self.world.as_ref().is_some_and(|w| w.cod2.is_some());
                    let desaturate = if cod2 {
                        crate::screen_blend::COD2_DESATURATE
                    } else {
                        0.0
                    };
                    Some((decay, desaturate))
                }
                (None, None) => {
                    self.screen_blend.drop_capture();
                    None
                }
            };
            if let Some((decay, desaturate)) = blend {
                let since = self.shock_captured_at.map_or(i32::MAX, |t| {
                    i32::try_from(t.elapsed().as_millis()).unwrap_or(i32::MAX)
                });
                self.screen_blend.blend(
                    &self.queue,
                    &mut pass,
                    crate::screen_blend::alpha(since, decay),
                    desaturate,
                );
                drop(pass);
                self.screen_blend.capture(&mut encoder);
                frame_write!(self, "screen capture", self.target_bytes(4));
                self.shock_captured_at = Some(std::time::Instant::now());
                pass = single_pass(&mut encoder, self.gamma.scene_view(), "hud pass");
            }

            // HUD after the weapon so it sits on top, before the debug
            // overlay so F3 always wins.
            if let Some(g) = &mut gpu_groups {
                g.pass(&mut pass, Some(crate::frame_prof::Group::Hud));
            }
            if !self.hud_pass.runs.is_empty() {
                let n = self.hud_pass.draw(&mut pass, plan.after);
                crate::frame_prof::count(crate::frame_prof::Count::HudDraws, n);
            }

            // Debug HUD text, over everything.
            if hud_quads > 0 {
                let hud = &self.hud;
                pass.set_pipeline(if plan.after {
                    &hud.pipeline_1x
                } else {
                    &hud.pipeline
                });
                pass.set_bind_group(0, &hud.bind_group, &[]);
                pass.set_vertex_buffer(0, hud.vertex_buf.slice(..));
                pass.set_index_buffer(hud.index_buf.slice(..), wgpu::IndexFormat::Uint16);
                pass.draw_indexed(0..(hud_quads * 6) as u32, 0, 0..1);
            }
        }
        if shadow_on && let Some(sp) = &mut self.shadows {
            sp.resolve_timer(&mut encoder);
        }
        if let Some(f) = self.sky.as_mut().and_then(|s| s.flare.as_mut()) {
            f.resolve(&mut encoder);
        }
        if let Some(t) = self.ssao.timer_mut() {
            t.resolve(&mut encoder, ssao_on);
        }
        if let Some(g) = &self.prof_gpu {
            // No post pass: the blend runs on into the HUD, all of it
            // `blend`.
            if !split_stamped {
                g.stamp(prof_slot, &mut encoder, 4);
                g.stamp(prof_slot, &mut encoder, 5);
            }
            if !post_stamped {
                g.stamp(prof_slot, &mut encoder, 6);
            }
            g.stamp(prof_slot, &mut encoder, 7);
        }
        if let Some(g) = &mut gpu_groups {
            g.enc(&mut encoder, None);
            g.resolve(&mut encoder);
        }
        if !self.gamma_skipped {
            self.gamma.draw(&mut encoder, &view);
            frame_write!(self, "gamma pass into the swapchain", self.target_bytes(4));
        }
        if let Some(g) = &mut self.prof_gpu {
            g.stamp(prof_slot, &mut encoder, 8);
            g.resolve(prof_slot, &mut encoder);
        }
        if let Some(f) = &mut self.frag_stats {
            f.resolve(&mut encoder);
        }
        // Before the ramp is the gamma scene, unless the frame drew
        // straight into the swapchain past an identity ramp.
        let readback = self.capture_wanted.take().map(|pre_gamma| {
            let tex = match (&surface_tex, &self.target) {
                _ if pre_gamma && !self.gamma_skipped => self.gamma.scene_view().texture(),
                (Some(t), _) => &t.texture,
                (None, Target::Offscreen(t)) => t,
                (None, Target::Surface(_)) => unreachable!("a surface frame holds its texture"),
            };
            capture::encode(&self.device, &mut encoder, tex)
        });
        prof.next(crate::frame_prof::Stage::Finish);
        let commands = encoder.finish();
        prof.next(crate::frame_prof::Stage::Submit);
        self.queue.submit([commands]);
        if let Some(sp) = &mut self.shadows {
            sp.after_submit();
        }
        self.sun_shadow.after_submit();
        if let Some(f) = self.sky.as_mut().and_then(|s| s.flare.as_mut()) {
            f.after_submit();
        }
        if let Some(rb) = readback {
            self.captured = Some(rb.and_then(|rb| capture::finish(&self.device, rb)));
        }
        if let Some(t) = surface_tex {
            self.queue.present(t);
        }
        if let Some(t) = self.ssao.timer_mut() {
            t.collect(&self.device);
        }
        if let Some(g) = &mut self.prof_gpu {
            g.collect();
            let _ = self.device.poll(wgpu::PollType::Poll);
        }
        if let Some(f) = &mut self.frag_stats {
            f.collect();
            let _ = self.device.poll(wgpu::PollType::Poll);
        }
        self.gpu_groups = gpu_groups;
        if let Some(g) = &mut self.gpu_groups {
            g.collect();
            let _ = self.device.poll(wgpu::PollType::Poll);
        }
    }

    /// The frame's uniforms: the Camera block, the waters, SSAO's and each
    /// moved brush model's camera, the farbox's eye. Returns the camera
    /// block (the last moved model's pose in its model matrix), the moved
    /// models' poses and whether SSAO draws.
    fn upload_frame_uniforms(
        &mut self,
        frame: &Frame,
    ) -> ([f32; CAMERA_FLOATS], Vec<glam::Mat4>, bool) {
        // `proj` (64) + `time_pad` (16) + eye/fog tail (48) + view forward
        // (16), matching Camera in the WGSL modules.
        self.fog.advance(frame.time);
        let mut camera = [0.0f32; CAMERA_FLOATS];
        camera[..16].copy_from_slice(&frame.view_proj.to_cols_array());
        camera[16] = frame.time;
        camera[17] = self.identity_light;
        camera[18] = if self.bicubic { 1.0 } else { 0.0 };
        // `r_specular` 0 drops a CoD4 set's specular (cod2_world.wgsl).
        camera[19] = if self.cod4_options.specular { 0.0 } else { 1.0 };
        if let Some(c2) = self.world.as_mut().and_then(|w| w.cod2.as_mut())
            && c2.normal_maps_on != self.cod4_options.normal_maps
        {
            c2.set_normal_maps(
                &self.device,
                &self.queue,
                self.cod4_options.normal_maps,
                &mut self.cod2_lit,
            );
        }
        if self.fog.set && self.cod4_options.fog {
            let f = &self.fog.live;
            camera[20..24].copy_from_slice(&[frame.eye.x, frame.eye.y, frame.eye.z, f.mode()]);
            // GL_FOG_COLOR carries identityLight (0x4d2ad0); the clear doesn't.
            let il = self.identity_light;
            camera[24..28].copy_from_slice(&[
                f.color[0] * il,
                f.color[1] * il,
                f.color[2] * il,
                f.density,
            ]);
            let radial = if self.fog.radial { 1.0 } else { 0.0 };
            camera[28..32].copy_from_slice(&[f.near, f.far, radial, f.start]);
        } else {
            camera[20..24].copy_from_slice(&[frame.eye.x, frame.eye.y, frame.eye.z, 0.0]);
        }
        camera[32..36].copy_from_slice(&[frame.fwd.x, frame.fwd.y, frame.fwd.z, 0.0]);
        camera[36..52].copy_from_slice(&glam::Mat4::IDENTITY.to_cols_array());
        self.queue
            .write_buffer(&self.camera_buf, 0, bytemuck::cast_slice(&camera));
        frame_write!(self, "upload camera", CAMERA_BYTES);
        // CoD4's water height maps at the view's time (cod4-world.md 9.8),
        // held under `r_drawWater` 0 (cod4-front-end.md 9)
        if self.cod4_options.water {
            self.water_time = frame.time;
        }
        if let Some(c2) = self.world.as_mut().and_then(|w| w.cod2.as_mut()) {
            for w in &mut c2.cod4_waters {
                w.update(&self.queue, self.water_time);
            }
        }
        // CoD2's water height maps at the view's time (cod2-materials.md 9.12)
        if let Some(w) = self
            .world
            .as_mut()
            .and_then(|w| w.cod2.as_mut()?.water.as_mut())
        {
            w.update(&self.queue, frame.time);
        }
        let ssao_on = self.ssao.strength() > 0.0 && self.world.is_some();
        if ssao_on {
            self.ssao.prepare(
                &self.queue,
                &crate::ssao::FrameParams {
                    view_proj: frame.view_proj,
                    near: Z_NEAR,
                    far: Z_FAR,
                    fog_mode: camera[23],
                    fog_density: camera[27],
                    fog_near: camera[28],
                    fog_far: camera[29],
                },
            );
        }
        let moved: Vec<glam::Mat4> = self.world.as_ref().map_or_else(Vec::new, |w| {
            w.moved.iter().map(|&(_, pose)| pose).collect()
        });
        while self.moved_cams.len() < moved.len() {
            let buf = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("moved brush model camera"),
                size: CAMERA_BYTES,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            let bg = self.camera_group(&buf);
            self.moved_cams.push((buf, bg));
        }
        for (pose, (buf, _)) in moved.iter().zip(&self.moved_cams) {
            camera[36..52].copy_from_slice(&pose.to_cols_array());
            self.queue
                .write_buffer(buf, 0, bytemuck::cast_slice(&camera));
            frame_write!(self, "upload moved model camera", CAMERA_BYTES);
        }
        if let Some(sky) = &self.sky
            && let Some(farbox) = &sky.farbox
        {
            self.queue.write_buffer(
                &farbox.eye_buf,
                0,
                bytemuck::bytes_of(&[frame.eye.x, frame.eye.y, frame.eye.z, 0.0f32]),
            );
        }
        (camera, moved, ssao_on)
    }

    /// The visible set, the gathered world indices and their draw order,
    /// the moved brush models', the dlight runs, the coronas and the F3
    /// `vis` counts; `t0` is when the frame's gather began.
    fn cull_and_gather(
        &mut self,
        frame: &Frame,
        lod_view: &model::LodView,
        t0: std::time::Instant,
        prof: &mut crate::frame_prof::Span,
    ) {
        crate::frame_prof::size(self.config.width, self.config.height);
        let frustum = Frustum::from_view_proj(frame.view_proj);
        let cod4_lod = vcod_common::mapworld::cod4_lod_ramp(
            crate::camera::fov_y(frame.fov_x, self.aspect()),
            self.lod_cvars.0,
            self.lod_cvars.1,
        );
        self.cod4_lod = cod4_lod;
        self.sky_drawn = false;
        if let Some(world) = &mut self.world {
            let visible = match frame.cull {
                CullMode::Off => None,
                CullMode::Locked => Some(match world.locked.take() {
                    Some(v) => v,
                    None => Self::cull(world, frame.eye, &frame.view_proj, &frustum, cod4_lod),
                }),
                CullMode::On => {
                    let key = cull_key(frame.eye, &frame.view_proj, cod4_lod);
                    Some(match world.culled.take() {
                        Some((k, v)) if k == key => v,
                        _ => Self::cull(world, frame.eye, &frame.view_proj, &frustum, cod4_lod),
                    })
                }
            };
            // A corona's box goes through the frustum and the visited cells,
            // as a static model's does (0x4e48b8..0x4e4a41).
            let sprites: Vec<crate::corona::Sprite> = self
                .coronas
                .iter()
                .filter(|c| {
                    let (lo, hi) = crate::corona::cull_box(c);
                    match &visible {
                        Some((v, _)) => world.vis.prop_visible(v, &frustum, lo, hi),
                        None => frustum.intersects_aabb(lo, hi),
                    }
                })
                .filter_map(|c| crate::corona::sprite(c, frame.eye, frame.fwd, frame.up))
                .collect();
            self.marks
                .set_sprites(&self.queue, crate::corona::SHADER, &sprites);
            // A static model reads the LOD table like an entity, at its origin
            // and scale (docs/research/xmodel-v14-format.md, "LOD selection").
            let prop_lod: Vec<Option<u8>> = world
                .prop_lods
                .iter()
                .map(|l| {
                    let d = lod_view.distance(frame.eye, l.origin, l.scale);
                    model::select_lod(&l.dists, d).map(|i| i as u8)
                })
                .collect();
            let lod_shown = |p: u32, l: u8| prop_lod[p as usize] == Some(l);
            let mut props_drawn = 0usize;
            prof.next(crate::frame_prof::Stage::Gather);
            // unculled, a static model draws its LOD 0; culled, the pick
            // in `cull` left one LOD's soups
            let culled = visible.is_some();
            let shown = |si: usize| {
                world.static_models[world.soup_model[si] as usize]
                    && (culled || !world.coarse_lod[si])
            };
            let ranges: Vec<IndexRange> = match &visible {
                None => world
                    .soup_ranges
                    .iter()
                    .enumerate()
                    .filter_map(|(si, r)| if shown(si) { *r } else { None })
                    .chain(
                        world
                            .prop_ranges
                            .iter()
                            .filter(|&&(p, l, _)| lod_shown(p, l))
                            .map(|&(_, _, r)| r),
                    )
                    .collect(),
                Some((v, prop_ok)) => {
                    props_drawn = prop_ok
                        .iter()
                        .zip(&prop_lod)
                        .filter(|&(&ok, l)| ok && l.is_some())
                        .count();
                    world
                        .soup_ranges
                        .iter()
                        .zip(&v.soups)
                        .enumerate()
                        .filter_map(|(si, (r, &vis))| if vis && shown(si) { *r } else { None })
                        .chain(
                            world
                                .prop_ranges
                                .iter()
                                .filter(|&&(p, l, _)| prop_ok[p as usize] && lod_shown(p, l))
                                .map(|&(_, _, r)| r),
                        )
                        .collect()
                }
            };
            let same_world = ranges == world.gathered_from;
            if same_world {
                // last frame's moved brush models follow the world's part
                world.gathered.truncate(world.world_gathered);
            } else {
                world.draws = gather(
                    &world.cpu_indices,
                    ranges.iter().copied(),
                    world.batch_draws.len(),
                    &mut world.gathered,
                    &mut world.gather_scratch,
                );
                // the receiving soups where the gather put them, for the
                // cookie masks: a batch's soups follow each other in its draw
                world.cookie_runs.clear();
                for d in &world.draws {
                    if world.cookie_receive.get(d.batch as usize) != Some(&true) {
                        continue;
                    }
                    let mut at = d.first;
                    for r in &world.gather_scratch[d.batch as usize] {
                        let b = world.cookie_bounds.get(&r.first).copied();
                        world.cookie_runs.push((at..at + r.count, b));
                        at += r.count;
                    }
                }
                world.gathered_from = ranges;
                world.world_gathered = world.gathered.len();
            }
            // Animated stages get fresh params; static slots keep the load fill.
            for (sh, si, slot) in &world.animated {
                if let Some(p) = stage_params(sh, *si, frame.time) {
                    self.queue.write_buffer(
                        &world.stage_params_buf,
                        u64::from(*slot) * world.stage_params_stride,
                        bytemuck::bytes_of(&p),
                    );
                }
            }
            // Dome stages always carry the view origin, animated or not, and
            // the sky flag so linear fog skips them.
            for (sh, si, slot) in &world.sky_stages {
                if let Some(mut p) = stage_params(sh, *si, frame.time) {
                    p.flags |= STAGE_FLAG_EYE_OFFSET | STAGE_FLAG_SKY;
                    p.eye_off = [frame.eye.x, frame.eye.y, frame.eye.z, 0.0];
                    self.queue.write_buffer(
                        &world.stage_params_buf,
                        u64::from(*slot) * world.stage_params_stride,
                        bytemuck::bytes_of(&p),
                    );
                }
            }
            if !same_world {
                let mut frame_draws = std::mem::take(&mut world.frame_draws);
                order_draws(
                    &world.batch_keys,
                    &world.batch_draws,
                    &world.stages_of_batch,
                    &world.draws,
                    &mut frame_draws,
                );
                world.frame_draws = frame_draws;
            }
            // A moved brush model's soups follow the world's in the index
            // buffer, each set ordered on its own and drawn under its own
            // camera.
            let mut moved_draws = std::mem::take(&mut world.moved_draws);
            moved_draws.clear();
            for &(m, _) in &world.moved {
                let ranges: Vec<IndexRange> = world.model_soups[m]
                    .clone()
                    .filter_map(|si| world.soup_ranges.get(si).copied().flatten())
                    .collect();
                let mut out = Vec::new();
                let mut draws = gather(
                    &world.cpu_indices,
                    ranges,
                    world.batch_draws.len(),
                    &mut out,
                    &mut world.gather_scratch,
                );
                let base = world.gathered.len() as u32;
                for d in &mut draws {
                    d.first += base;
                }
                world.gathered.extend(out);
                let mut order = Vec::new();
                order_draws(
                    &world.batch_keys,
                    &world.batch_draws,
                    &world.stages_of_batch,
                    &draws,
                    &mut order,
                );
                moved_draws.push((draws, order));
            }
            world.moved_draws = moved_draws;
            // The dlight pass: per light, the visible soups it reaches
            // whose batch takes dlights, then each moved brush model's
            // with the light carried into the model's frame (0x4b59f0).
            world.dlight_runs.clear();
            world.dlight_indices.clear();
            let mut scene = self.dynamic.scene_lights.clone();
            scene.extend_from_slice(&self.dynamic.entity_scene_lights);
            let lights = if lights_world(self.dynamic_light) {
                dlight::from_scene(&scene)
            } else {
                Vec::new()
            };
            if !lights.is_empty() {
                for (i, l) in lights.iter().enumerate() {
                    self.queue.write_buffer(
                        &self.dlight.buf,
                        i as u64 * self.dlight.stride,
                        bytemuck::bytes_of(&dlight::DlightUniform::of(l, self.dlight_quality)),
                    );
                }
                let takes = |si: usize| {
                    world.soup_ranges[si]
                        .is_some_and(|r| !world.dlights_of_batch[r.batch as usize].is_empty())
                };
                let lit: Vec<usize> = (0..world.soup_ranges.len())
                    .filter(|&si| {
                        takes(si) && shown(si) && visible.as_ref().is_none_or(|(v, _)| v.soups[si])
                    })
                    .collect();
                let mut sets: Vec<(Option<usize>, glam::Mat4, Vec<usize>)> =
                    vec![(None, glam::Mat4::IDENTITY, lit)];
                for (mi, &(m, pose)) in world.moved.iter().enumerate() {
                    let soups = world.model_soups[m]
                        .clone()
                        .filter(|&si| takes(si))
                        .collect();
                    sets.push((Some(mi), pose.inverse(), soups));
                }
                let mut out = Vec::new();
                for (moved, to_model, soups) in &sets {
                    for (li, l) in lights.iter().enumerate() {
                        let origin = to_model.transform_point3(l.origin);
                        let ranges = soups.iter().filter_map(|&si| {
                            let (lo, hi) = world.soup_bounds[si];
                            dlight::reaches(l, origin, lo, hi)
                                .then_some(world.soup_ranges[si])
                                .flatten()
                        });
                        let draws = gather(
                            &world.cpu_indices,
                            ranges,
                            world.batch_draws.len(),
                            &mut out,
                            &mut world.gather_scratch,
                        );
                        let base = world.dlight_indices.len() as u32;
                        world.dlight_indices.extend_from_slice(&out);
                        world
                            .dlight_runs
                            .extend(draws.into_iter().map(|d| DlightRun {
                                light: li as u32,
                                moved: *moved,
                                draw: DrawRange {
                                    first: d.first + base,
                                    ..d
                                },
                            }));
                    }
                }
                let bytes = (world.dlight_indices.len() * 4) as u64;
                if bytes > world.dlight_index_buf.size() {
                    world.dlight_index_buf = self.device.create_buffer(&wgpu::BufferDescriptor {
                        label: Some("dlight indices"),
                        size: bytes.next_power_of_two(),
                        usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
                        mapped_at_creation: false,
                    });
                }
                if bytes > 0 {
                    self.queue.write_buffer(
                        &world.dlight_index_buf,
                        0,
                        bytemuck::cast_slice(&world.dlight_indices),
                    );
                    frame_write!(self, "upload dlight indices", bytes);
                }
            }
            world.stage_draws_last = world
                .frame_draws
                .iter()
                .filter(|fd| matches!(fd.kind, DrawRef::Stage(_)))
                .count();
            // a world part gathered from last frame's ranges is in the
            // buffer already; the moved models' part after it is not
            let from = if same_world { world.world_gathered } else { 0 };
            if from < world.gathered.len() {
                self.queue.write_buffer(
                    &world.index_buf,
                    from as u64 * 4,
                    bytemuck::cast_slice(&world.gathered[from..]),
                );
                frame_write!(
                    self,
                    "upload gathered world indices",
                    (world.gathered.len() - from) * 4
                );
            }
            world.prop_lod = prop_lod;
            let cell_total = world.vis.cell_count();
            self.vis_counts = VisCounts {
                mode: Some(frame.cull),
                cells: visible.as_ref().map_or((cell_total, cell_total), |(v, _)| {
                    (v.cells.iter().filter(|&&c| c).count(), cell_total)
                }),
                soups: visible.as_ref().map_or(
                    (world.soup_ranges.len(), world.soup_ranges.len()),
                    |(v, _)| (v.stats.soups, world.soup_ranges.len()),
                ),
                tris: (world.gathered.len() / 3, world.cpu_indices.len() / 3),
                props: (
                    if visible.is_some() {
                        props_drawn
                    } else {
                        world.prop_bounds.len()
                    },
                    world.prop_bounds.len(),
                ),
                occluders: visible.as_ref().map_or(0, |(v, _)| v.stats.occluders),
                portals_occluded: visible
                    .as_ref()
                    .map_or(0, |(v, _)| v.stats.portals_occluded),
                gather_ms: t0.elapsed().as_secs_f32() * 1000.0,
            };
            prof.next(crate::frame_prof::Stage::Build);
            crate::frame_prof::count(crate::frame_prof::Count::Tris, self.vis_counts.tris.0);
            crate::frame_prof::count(crate::frame_prof::Count::Soups, self.vis_counts.soups.0);
            // linear fog is the far plane retail's 0x4e1670 reads (no `r_zfar`)
            let far = (self.fog.set && self.fog.live.linear).then_some(self.fog.live.far);
            let walked = world
                .sky_soups
                .iter()
                .any(|&(si, _)| visible.as_ref().is_none_or(|(v, _)| v.soups[si]) && shown(si));
            let boxes: Vec<_> = world.sky_soups.iter().map(|&(_, b)| b).collect();
            self.sky_drawn = vcod_common::sun::sky_drawn(walked, far, frame.eye, frame.fwd, &boxes);
            world.locked = None;
            world.culled = None;
            match frame.cull {
                CullMode::Locked => world.locked = visible,
                CullMode::On => {
                    let key = cull_key(frame.eye, &frame.view_proj, cod4_lod);
                    world.culled = visible.map(|v| (key, v));
                }
                CullMode::Off => {}
            }
        }
    }

    /// The debug HUD's text quads; returns how many.
    fn upload_hud_text(&mut self, frame: &Frame) -> usize {
        // The glyph cap truncates whole quads, so a cut readout stays legible.

        if frame.hud_lines.is_empty() {
            0
        } else {
            let mut verts = hud_text::layout_lines(
                &frame.hud_lines,
                self.config.width as f32,
                self.config.height as f32,
                2.0,
            );
            verts.truncate(MAX_HUD_GLYPHS * 2 * 4);
            self.queue
                .write_buffer(&self.hud.vertex_buf, 0, bytemuck::cast_slice(&verts));
            verts.len() / 4
        }
    }

    /// The entities' animated stage params, lights, instances and bones.
    fn upload_entities(&mut self, frame: &Frame) {
        // Instance i draws from slot i of instance_buf.
        if !self.dynamic.instances.is_empty() {
            let m = &self.dynamic.materials;
            for (sh, si, slot) in &m.animated {
                if let Some(p) = stage_params(sh, *si, frame.time) {
                    self.queue.write_buffer(
                        &m.params_buf,
                        u64::from(*slot) * ENT_STAGE_STRIDE,
                        bytemuck::bytes_of(&p),
                    );
                }
            }
            let sets = self.pick_entity_lights();
            let raw: Vec<InstanceRaw> = self.dynamic.instances.iter().map(|(_, r)| *r).collect();
            let d = &mut self.dynamic;
            // the spans that changed since the last write
            #[cfg_attr(not(test), expect(unused_variables))]
            let bytes = d.light_written.write(&self.queue, &d.light_buf, &sets)
                + d.instance_written.write(&self.queue, &d.instance_buf, &raw)
                + d.bone_written.write(&self.queue, &d.bone_buf, &d.bone_mats);
            frame_write!(self, "upload entity lights, instances and bones", bytes);
        }
    }

    /// The view model's uniforms, lights and bones; returns whether it
    /// draws.
    fn upload_view_model(&mut self, frame: &Frame, vm: Option<&VmDraw>) -> bool {
        let draw_vm = match &vm {
            Some(draw) if !self.vm_pass.models.is_empty() => {
                let to_view = glam::camera::rh::view::look_to_mat4(frame.eye, frame.fwd, frame.up);
                // CoD2: the grid at the eye, world space (cod2_model.wgsl);
                // CoD4: the view model's own block, world space too
                let cod2 = self.dynamic.cod2_light.is_some() || self.dynamic.cod4_light.is_some();
                let lights = match (&self.dynamic.cod2_light, self.dynamic.lighting.as_mut()) {
                    _ if self.dynamic.cod4_light.is_some() => {
                        let c4 = self.dynamic.cod4_light.as_ref().expect("checked");
                        // the gun's box: its lighting origin alone; its
                        // block the volume's last
                        let o = draw.light_origin;
                        let block = entity_light::COD4_VOLUME_BLOCKS - 1;
                        let (set, texels) = c4.set(o, [o, o], block, self.identity_light);
                        self.dynamic
                            .materials
                            .cod4_volume
                            .write_view_block(&self.queue, &texels);
                        set
                    }
                    (Some(c2), _) => c2.entity_set(draw.light_origin, self.identity_light),
                    (None, Some(l)) => {
                        let scene = model_scene_lights(
                            &self.dynamic.scene_lights,
                            &self.dynamic.entity_scene_lights,
                            self.dynamic_light,
                        );
                        let picked = l.entity_lights(draw.light_origin, &scene);
                        entity_light::pack(&picked, Some(to_view), self.identity_light)
                    }
                    (None, None) => GpuLightSet::default(),
                };
                let uniform = vm_uniform(draw.transform, draw.fov_x, self.aspect());
                self.queue.write_buffer(
                    &self.vm_pass.uniform_buf,
                    0,
                    bytemuck::cast_slice(&uniform),
                );
                self.queue.write_buffer(
                    &self.vm_pass.uniform_buf,
                    VM_LIGHTS_OFFSET,
                    bytemuck::bytes_of(&lights),
                );
                self.queue.write_buffer(
                    &self.vm_pass.uniform_buf,
                    VM_MISC_OFFSET,
                    bytemuck::cast_slice(&[
                        self.identity_light,
                        f32::from(u8::from(cod2)),
                        frame.time,
                        0.0,
                    ]),
                );
                self.queue.write_buffer(
                    &self.vm_pass.uniform_buf,
                    VM_VIEW_TO_WORLD_OFFSET,
                    bytemuck::cast_slice(&to_view.inverse().to_cols_array()),
                );
                for (i, model) in self.vm_pass.models.iter().enumerate() {
                    let Some(bones) = draw.bone_sets.get(i) else {
                        continue;
                    };
                    if bones.is_empty() {
                        continue;
                    }
                    if bones.len() > VM_BONE_COUNT {
                        static VM_BONE_CAP_WARNED: std::sync::Once = std::sync::Once::new();
                        VM_BONE_CAP_WARNED.call_once(|| {
                            log::warn!(
                                "viewmodel bone set of {} exceeds the shader's {VM_BONE_COUNT}; truncating",
                                bones.len()
                            );
                        });
                    }
                    let cols: Vec<[f32; 16]> = bones
                        .iter()
                        .take(VM_BONE_COUNT)
                        .map(glam::Mat4::to_cols_array)
                        .collect();
                    let buf = &model.bones.as_ref().expect("view model bones").buf;
                    self.queue.write_buffer(buf, 0, bytemuck::cast_slice(&cols));
                    frame_write!(self, "upload view model bones", cols.len() * 64);
                }
                true
            }
            _ => false,
        };

        draw_vm && self.vm_pass.skins.len() == self.vm_pass.models.len()
    }

    /// The frame's entity and view model surfaces in draw order; the
    /// shadow casters keep every instance.
    fn frame_ent_draws(&self, frame: &Frame, draw_vm: bool) -> Vec<EntDraw> {
        let vm_skins: &[Vec<Rc<EntSkin>>] = if draw_vm { &self.vm_pass.skins } else { &[] };
        // the instances the view can see; the shadow casters keep them all
        let frustum = vcod_common::vis::Frustum::from_view_proj(frame.view_proj);
        let seen: Vec<bool> = (0..self.dynamic.instances.len())
            .map(|k| !self.draw_joins || instance_seen(&self.dynamic, k, &frustum))
            .collect();

        sorted_ent_draws(
            self.dynamic
                .instances
                .iter()
                .enumerate()
                .filter(|&(i, _)| seen[i])
                .flat_map(|(i, (m, _))| {
                    self.dynamic.models[*m]
                        .skins
                        .iter()
                        .enumerate()
                        .map(move |(j, sk)| (i as u32, j as u32, &**sk))
                }),
            vm_skins.iter().enumerate().flat_map(|(i, skins)| {
                skins
                    .iter()
                    .enumerate()
                    .map(move |(j, sk)| (i as u32, j as u32, &**sk))
            }),
        )
    }

    /// The frame's target and its view. `None` skips the frame: the
    /// surface gave no texture, so the writes queued so far submit now.
    fn acquire(&mut self) -> Option<(Option<wgpu::SurfaceTexture>, wgpu::TextureView)> {
        let surface_tex = match &self.target {
            Target::Offscreen(_) => None,
            Target::Surface(s) => Some(s.get_current_texture()),
        };
        let surface_tex = match surface_tex {
            None => None,
            Some(t) => Some(match t {
                wgpu::CurrentSurfaceTexture::Success(tex)
                | wgpu::CurrentSurfaceTexture::Suboptimal(tex) => tex,
                // A frame with no surface still submits, so the writes it
                // queued above go now and do not pile up until one draws.
                wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded => {
                    self.queue.submit([]);
                    return None;
                }
                wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                    self.queue.submit([]);
                    self.reconfigure();
                    return None;
                }
                wgpu::CurrentSurfaceTexture::Validation => {
                    log::warn!("dropped frame: surface validation error");
                    self.queue.submit([]);
                    return None;
                }
            }),
        };
        let view = match (&surface_tex, &self.target) {
            (Some(t), _) => t.texture.create_view(&Default::default()),
            (None, Target::Offscreen(t)) => t.create_view(&Default::default()),
            (None, Target::Surface(_)) => unreachable!("a surface frame holds its texture"),
        };
        Some((surface_tex, view))
    }

    /// The shadow passes before the scene: `vc_shadows`' maps, CoD2's
    /// cookies (and their receivers unless they draw over the prepass) and
    /// CoD4's sun shadow. Returns whether `vc_shadows` drew and whether the
    /// receivers wait for the prepass.
    fn encode_shadow_passes(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        frame: &Frame,
        (camera, moved): (&[f32; CAMERA_FLOATS], &[glam::Mat4]),
        prof_slot: Option<usize>,
    ) -> (bool, bool) {
        let shadow_on = self.encode_shadow_maps(encoder, frame, camera, moved);
        if shadow_on {
            frame_write!(self, "vc_shadows maps (not sized)", 0);
        }
        if let Some(g) = &self.prof_gpu {
            g.stamp(prof_slot, encoder, 1);
        }
        self.encode_cookies(encoder, frame);
        let receivers_on_prepass = self.cookie_receivers_on_prepass();
        if !receivers_on_prepass {
            Self::encode_cookie_receivers(self.cookie_receivers(), encoder, frame, false);
        }
        if let Some(g) = &self.prof_gpu {
            g.stamp(prof_slot, encoder, 2);
        }
        self.encode_sun_shadow(encoder, frame);
        if let Some(g) = &self.prof_gpu {
            g.stamp(prof_slot, encoder, 3);
        }
        if !shadow_on && let Some(sp) = &self.shadows {
            sp.idle(&self.queue);
        }
        (shadow_on, receivers_on_prepass)
    }

    /// The sky into the scene pass, first: the farbox, the sun disc and
    /// the dome.
    fn draw_sky(&self, pass: &mut wgpu::RenderPass<'_>, frame: &Frame) {
        // The sky draws first so every world draw overwrites it; the box
        // and dome both ignore depth (cleared 1.0) and write none. CoD
        // keeps the sky visible under linear fog (VERIFIED live on
        // mp_ship, unlike RTCW's drawsky=false).
        if let (Some(world), Some(sky)) = (&self.world, &self.sky) {
            if let Some(farbox) = &sky.farbox {
                pass.set_pipeline(&farbox.pipeline);
                pass.set_bind_group(2, &farbox.eye_bg, &[]);
                pass.set_vertex_buffer(0, farbox.vertex_buf.slice(..));
                pass.set_index_buffer(farbox.index_buf.slice(..), wgpu::IndexFormat::Uint32);
                for (axis, &(first, end)) in farbox.sides.iter().enumerate() {
                    pass.set_bind_group(1, &farbox.side_bgs[axis], &[]);
                    pass.draw_indexed(first..end, 0, 0..1);
                }
                tally(&self.census, |c| {
                    let n = farbox.sides.len() as u32;
                    c.sky += n;
                    c.pipelines += 1;
                    c.bind_groups += 1 + n;
                });
            }
            // The sunfile disc: additive billboard along the sun
            // direction at farbox distance, camera-facing via world axes.
            if let Some(sun) = sky.sun.as_ref().filter(|_| self.sky_drawn && self.draw_sun) {
                let center = frame.eye + sun.dir * sky::box_size(Z_FAR, Z_NEAR);
                let up = glam::Vec3::Z;
                let right = up.cross(sun.dir).normalize_or_zero();
                let quad_up = sun.dir.cross(right);
                let r = right * sun.half_extent;
                let u = quad_up * sun.half_extent;
                let corner = |dr: f32, du: f32| sky::SkyBoxVert {
                    pos: (center + r * dr + u * du).to_array(),
                    uv: [(dr + 1.0) * 0.5, 1.0 - (du + 1.0) * 0.5],
                };
                let verts = [
                    corner(-1.0, -1.0),
                    corner(1.0, -1.0),
                    corner(1.0, 1.0),
                    corner(-1.0, -1.0),
                    corner(1.0, 1.0),
                    corner(-1.0, 1.0),
                ];
                self.queue
                    .write_buffer(&sun.vertex_buf, 0, bytemuck::cast_slice(&verts));
                pass.set_pipeline(&sun.pipeline);
                pass.set_bind_group(1, &sun.bind_group, &[]);
                pass.set_vertex_buffer(0, sun.vertex_buf.slice(..));
                pass.draw(0..6, 0..1);
                tally(&self.census, |c| {
                    c.sky += 1;
                    c.pipelines += 1;
                    c.bind_groups += 1;
                });
            }
            if let Some(dome) = &sky.dome {
                pass.set_vertex_buffer(0, dome.vertex_buf.slice(..));
                pass.set_index_buffer(dome.index_buf.slice(..), wgpu::IndexFormat::Uint32);
                let mut bound: Option<(StageVariant, bool)> = None;
                for ds in &dome.stages {
                    let key = (ds.variant, ds.two_sided);
                    if bound != Some(key) {
                        pass.set_pipeline(self.stage_pipeline(ds.variant, ds.two_sided, false));
                        bound = Some(key);
                        tally(&self.census, |c| c.pipelines += 1);
                    }
                    pass.set_bind_group(1, ds.mat.bind_group(frame.time), &[]);
                    pass.set_bind_group(
                        2,
                        ds.stage.bind_group(frame.time),
                        &[world.stage_offset(ds.slot)],
                    );
                    pass.draw_indexed(0..dome.index_count, 0, 0..1);
                    tally(&self.census, |c| {
                        c.sky += 1;
                        c.bind_groups += 2;
                    });
                }
            }
        }
    }

    /// `r_vc_depthprepass`'s passes ([`Self::encode_depth_prepass`]): the
    /// world's, then the entities', with CoD2's cookie receivers between
    /// them when they draw over the world's depth.
    fn encode_prepass(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        frame: &Frame,
        plan: &FramePlan,
        (world_list, ent_draws): (&[WorldItem], &[EntDraw]),
        receivers_on_prepass: bool,
        gpu_groups: &mut Option<crate::frame_prof::GpuGroups>,
    ) {
        let mut pre_bundles = std::mem::take(&mut self.prepass_bundles);
        pre_bundles.begin(plan.floatz.then_some(crate::soft_particles::LINZ_FORMAT));
        if plan.prepass_on {
            if let Some(g) = gpu_groups {
                g.enc(encoder, Some(crate::frame_prof::Group::Prepass));
            }
            // the cookie receivers test the world's depth alone, so the
            // entities' prepass follows them
            let receivers_split = receivers_on_prepass && !plan.floatz;
            let mut n = self.encode_depth_prepass(
                encoder,
                world_list,
                ent_draws,
                PrepassParts {
                    world: true,
                    ents: plan.ent_prepass && !receivers_split,
                    floatz: plan.floatz,
                    load: false,
                },
                frame.time,
                plan.ent_bundling.then_some(&mut pre_bundles),
            );
            if receivers_split {
                if let Some(g) = gpu_groups {
                    g.enc(encoder, Some(crate::frame_prof::Group::Receivers));
                }
                Self::encode_cookie_receivers(
                    CookieReceivers {
                        device: &self.device,
                        queue: &self.queue,
                        world: self.world.as_ref(),
                        cookies: &mut self.cookies,
                        dyn_shadows: &mut self.dyn_shadows,
                        depth_view: &self.depth_view,
                        cod2: self.dynamic.cod2_light.is_some(),
                    },
                    encoder,
                    frame,
                    true,
                );
                if plan.ent_prepass {
                    if let Some(g) = gpu_groups {
                        g.enc(encoder, Some(crate::frame_prof::Group::Prepass));
                    }
                    n += self.encode_depth_prepass(
                        encoder,
                        world_list,
                        ent_draws,
                        PrepassParts {
                            world: false,
                            ents: true,
                            floatz: plan.floatz,
                            load: true,
                        },
                        frame.time,
                        plan.ent_bundling.then_some(&mut pre_bundles),
                    );
                }
            }
            crate::frame_prof::count(crate::frame_prof::Count::Prepass, n);
            frame_write!(
                self,
                "depth prepass (depth)",
                self.target_bytes(4 * msaa_samples())
            );
            if plan.floatz {
                frame_write!(self, "depth prepass floatZ (R32F)", self.target_bytes(4));
            }
            crate::frame_prof::count(
                crate::frame_prof::Count::Feathered,
                usize::from(!plan.ent_prepass),
            );
            self.prepass_drawn.set(n);
            self.floatz_drawn.set(plan.floatz);
        }
        pre_bundles.end();
        self.prepass_bundles = pre_bundles;
    }

    /// The passes after the scene pass, on the resolved world: CoD4's depth
    /// of field, the shafts, the glow and blur, bloom and the post pass.
    /// Returns the 2D's pass: into the swapchain (`view`) past an identity
    /// ramp, else the gamma scene.
    fn encode_post_chain<'e>(
        &mut self,
        encoder: &'e mut wgpu::CommandEncoder,
        frame: &Frame,
        plan: &FramePlan,
        view: &wgpu::TextureView,
        gpu_groups: &mut Option<crate::frame_prof::GpuGroups>,
    ) -> wgpu::RenderPass<'e> {
        let sky_cube = self
            .sky
            .as_ref()
            .and_then(|s| s.farbox.as_ref())
            .and_then(|f| f.cube.as_ref());
        let mut pass;
        let world = if plan.post {
            self.post.world_view()
        } else {
            self.gamma.scene_view()
        };
        use crate::frame_prof::Group;
        let mut group = |encoder: &mut wgpu::CommandEncoder, g| {
            if let Some(gg) = gpu_groups {
                gg.enc(encoder, Some(g));
            }
        };
        if let Some(d) = &plan.dof4 {
            group(encoder, Group::Dof);
            let c = d.consts(self.config.height, Z_NEAR, VM_NEAR);
            self.dof.encode(
                &self.device,
                &self.queue,
                encoder,
                self.post.world(),
                self.post.world_view(),
                &self.depth_view,
                &c,
            );
            // the copy, five quarter steps and the composite
            frame_write!(
                self,
                "r_dof (copy, 5 quarter passes, composite)",
                2 * self.target_bytes(self.world_bpp())
                    + 5 * self.target_bytes(self.world_bpp()) / 16
            );
        }
        if plan.shafts {
            group(encoder, Group::Shafts);
            self.shafts.encode(encoder, world);
            frame_write!(self, "vc_sunShafts (not sized)", 0);
        }
        #[cfg(test)]
        {
            if let Some(g) = &plan.glow {
                let steps: usize = 1 + g.chains.iter().map(Vec::len).sum::<usize>();
                frame_write!(
                    self,
                    format!("glow ({steps} quarter passes, {} applies)", g.applies.len()),
                    steps as u64 * self.target_bytes(4) / 16
                        + g.applies.len() as u64 * self.target_bytes(self.world_bpp())
                );
            }
            if let Some(b) = &plan.world_blur {
                frame_write!(
                    self,
                    "blurWorld",
                    b.chain.len() as u64 * self.target_bytes(4) / 16
                        + self.target_bytes(self.world_bpp())
                );
            }
        }
        group(encoder, Group::Glow);
        self.cod2_glow.encode(
            &self.device,
            &self.queue,
            encoder,
            world,
            sky_cube,
            frame.view_proj.inverse(),
            frame.eye,
            plan.glow.as_ref(),
            plan.world_blur.as_ref(),
        );
        if plan.bloom {
            group(encoder, Group::Bloom);
            self.bloom.set_params(
                &self.queue,
                self.bloom_intensity,
                self.bloom_threshold,
                self.identity_light,
                plan.hdr,
            );
            self.bloom.blur(encoder);
            frame_write!(self, "vc_bloom (not sized)", 0);
        }
        if plan.post {
            if plan.bloom {
                let mut bloom_pass = single_pass(encoder, self.post.world_view(), "bloom pass");
                self.bloom.composite(&mut bloom_pass);
            }
            self.post.bind_glow(
                &self.device,
                plan.glow4.as_ref().map(|_| self.cod2_glow.result()),
            );
            self.post.set_params(
                &self.queue,
                self.post_settings,
                plan.film,
                plan.glow4.as_ref().map_or(0.0, |g| g.1),
                plan.hdr,
                self.identity_light,
                frame.time,
            );
            group(encoder, Group::PostFx);
            pass = if plan.gamma_direct {
                // the post pass writes every texel
                clear_pass(encoder, view, "hud pass")
            } else {
                single_pass(encoder, self.gamma.scene_view(), "hud pass")
            };
            self.post.draw(&mut pass);
            if plan.gamma_direct {
                frame_write!(
                    self,
                    "post pass into the swapchain, then the 2D",
                    self.target_bytes(4)
                );
            } else {
                frame_write!(
                    self,
                    "post pass into the gamma scene, then the 2D",
                    self.target_bytes(4)
                );
            }
        } else {
            pass = single_pass(encoder, self.gamma.scene_view(), "hud pass");
            frame_write!(self, "2D pass (gamma scene)", self.target_bytes(4));
            if plan.bloom {
                self.bloom.composite(&mut pass);
            }
        }
        pass
    }
}

/// A single-sample pass that clears `view` to black and stores it: the post
/// pass and the 2D straight into the swapchain.
pub(super) fn clear_pass<'e>(
    encoder: &'e mut wgpu::CommandEncoder,
    view: &wgpu::TextureView,
    label: &str,
) -> wgpu::RenderPass<'e> {
    encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some(label),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view,
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations {
                load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                store: wgpu::StoreOp::Store,
            },
        })],
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
        multiview_mask: None,
    })
}

/// A single-sample pass that loads and stores `view`: the bloom composite
/// and the HUD after the map pass.
pub(super) fn single_pass<'e>(
    encoder: &'e mut wgpu::CommandEncoder,
    view: &wgpu::TextureView,
    label: &str,
) -> wgpu::RenderPass<'e> {
    encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some(label),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view,
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations {
                load: wgpu::LoadOp::Load,
                store: wgpu::StoreOp::Store,
            },
        })],
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
        multiview_mask: None,
    })
}

/// The bytes `frame_write!` logs.
#[cfg(test)]
impl Renderer {
    /// A full-screen target's bytes at `bpp` a texel.
    fn target_bytes(&self, bpp: u32) -> u64 {
        u64::from(self.config.width) * u64::from(self.config.height) * u64::from(bpp)
    }

    /// A texel of the world's colour target.
    fn world_bpp(&self) -> u32 {
        self.world_format.block_copy_size(None).unwrap_or(4)
    }

    /// A scene pass's colour and depth (each sample), and its resolve.
    fn scene_pass_bytes(&self, last: bool) -> u64 {
        let s = msaa_samples();
        self.target_bytes((self.world_bpp() + 4) * s)
            + if last && s > 1 {
                self.target_bytes(self.world_bpp())
            } else {
                0
            }
    }
}
