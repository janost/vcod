//! Encodes the shadow passes: `vc_shadows`' maps and casters, CoD2's cookies
//! and their receivers, the sun shadow maps.

use super::*;

/// What [`Renderer::encode_cookie_receivers`] reads and writes.
pub(super) struct CookieReceivers<'a> {
    pub(super) device: &'a wgpu::Device,
    pub(super) queue: &'a wgpu::Queue,
    pub(super) world: Option<&'a WorldGpu>,
    pub(super) cookies: &'a mut cod2_cookie::Cookies,
    pub(super) dyn_shadows: &'a mut cod2_cookie::DynamicShadows,
    pub(super) depth_view: &'a wgpu::TextureView,
    /// A CoD2 level's lighting is up.
    pub(super) cod2: bool,
}

/// A receiving soup's range of the gathered indices and its box (`None`:
/// every cookie), `WorldGpu::cookie_runs`.
pub(super) type CookieRun = (std::ops::Range<u32>, Option<(glam::Vec3, glam::Vec3)>);

impl Renderer {
    /// `vc_shadows`' uniforms and caster passes for this frame; false
    /// when it is off or the map has no world or no sun.
    pub(super) fn encode_shadow_maps(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        frame: &Frame,
        camera: &[f32],
        moved: &[glam::Mat4],
    ) -> bool {
        let (Some(sp), Some(world), Some(sun)) = (
            self.shadows.as_mut(),
            self.world.as_mut(),
            self.dynamic.lighting.as_ref().and_then(StaticLighting::sun),
        ) else {
            return false;
        };
        if world.shadow_casters.is_none() {
            world.shadow_casters = Some(shadow_casters(&self.device, world));
        }
        let keep = match std::mem::replace(&mut world.sun_keep, SunKeepState::Done(None)) {
            SunKeepState::Samples(samples, reach) => {
                world.sun_keep = self
                    .dynamic
                    .lighting
                    .as_ref()
                    .and_then(|l| l.sun_keep_job(samples, reach))
                    .map_or(SunKeepState::Done(None), SunKeepState::Running);
                None
            }
            SunKeepState::Running(job) if job.is_finished() => {
                let k = job.join().ok().flatten().map(Arc::new);
                if let Some(k) = &k {
                    log::info!(
                        "vc_shadows: lightmaps measure {:?}, {} cells of their own",
                        k.whole,
                        k.keeps.iter().flatten().count()
                    );
                } else {
                    log::info!("vc_shadows: lightmaps measure nothing");
                }
                world.sun_keep = SunKeepState::Done(k.clone());
                k
            }
            running @ SunKeepState::Running(_) => {
                world.sun_keep = running;
                None
            }
            SunKeepState::Done(keep) => {
                world.sun_keep = SunKeepState::Done(keep.clone());
                keep
            }
        };
        let world = &*world;
        let Some(casters) = &world.shadow_casters else {
            return false;
        };
        sp.prepare(
            &self.queue,
            &sun,
            casters,
            frame.view_proj,
            frame.eye,
            frame.fwd,
            camera,
            (self.config.width as f32, self.config.height as f32),
            moved,
            keep.as_ref(),
        );
        // each entity surface casts through its first depth-writing stage
        let dynamic = &self.dynamic;
        let mats = &dynamic.materials;
        let mut ents = Vec::new();
        for (i, (m, _)) in dynamic.instances.iter().enumerate() {
            let model = &dynamic.models[*m];
            for (surf, skin) in model.vm.surfaces.iter().zip(&model.skins) {
                let Some(st) = skin
                    .stages
                    .iter()
                    .find(|st| mats.pipelines[st.pipeline].0.1)
                else {
                    continue;
                };
                ents.push(shadows::EntCaster {
                    vertex_buf: &model.vm.vertex_buf,
                    index_buf: &model.vm.index_buf,
                    instance: i as u32,
                    first_index: surf.first_index,
                    index_count: surf.index_count,
                    base_vertex: surf.base_vertex,
                    image: match st.image {
                        StageImage::Skin => &model.vm.bind_groups[surf.bind_group],
                        StageImage::Own(k) => &mats.images[k],
                    },
                    params_offset: (u64::from(st.slot) * ENT_STAGE_STRIDE) as u32,
                });
            }
        }
        let moved_models: Vec<usize> = world.moved.iter().map(|&(m, _)| m).collect();
        let time = frame.time;
        let cut = |b: u32| match *world.shadow_cast.get(b as usize)? {
            ShadowCast::Cut => Some(shadows::CutBinding {
                mat: &world.bind_groups[world.batch_draws[b as usize].bind_group],
                stage: None,
            }),
            ShadowCast::Stage(sb) | ShadowCast::Deformed(sb) => {
                let sb = &world.stage_batches[sb as usize];
                Some(shadows::CutBinding {
                    mat: sb.mat.bind_group(time),
                    stage: Some((
                        sb.stage.bind_group(time),
                        u32::try_from(u64::from(sb.slot) * world.stage_params_stride)
                            .expect("stage params slot offset fits a dynamic offset"),
                    )),
                })
            }
            ShadowCast::None | ShadowCast::Solid => None,
        };
        sp.encode_maps(
            encoder,
            &world.vertex_buf,
            casters,
            &moved_models,
            &ents,
            &shadows::EntBindings {
                instance_buf: &dynamic.instance_buf,
                instance_stride: DYNAMIC_INSTANCE_STRIDE,
                bones: &dynamic.bone_bg,
                params: &mats.params_bg,
            },
            &cut,
        );
        true
    }

    /// CoD2's shadow cookies for this frame (`cod2_cookie`): picks them
    /// off the scene's entities, draws their casters and blurs them; the
    /// receivers follow ([`Self::encode_cookie_receivers`]).
    pub(super) fn encode_cookies(&mut self, encoder: &mut wgpu::CommandEncoder, frame: &Frame) {
        let d = &self.dynamic;
        let Some(c2) = d.cod2_light.as_ref().filter(|_| self.world.is_some()) else {
            return;
        };
        // each entity's posed box over its placed parts
        let entities = if self.cookies.settings.enable {
            self.cookies.entities.len()
        } else {
            0
        };
        let mut boxes: Vec<Option<(glam::Vec3, glam::Vec3)>> = vec![None; entities];
        let mut owner = HashMap::new();
        for (e, ent) in self.cookies.entities[..entities].iter().enumerate() {
            for part in ent.parts.clone() {
                owner.insert(part, e);
            }
        }
        for p in &d.placed {
            let (Some(&e), Some((lo, hi))) = (owner.get(&p.src), posed_box(d, p)) else {
                continue;
            };
            let b = &mut boxes[e];
            *b = Some(b.map_or((lo, hi), |(a, z)| (a.min(lo), z.max(hi))));
        }
        let cands: Vec<cod2_cookie::Candidate> = self.cookies.entities[..entities]
            .iter()
            .zip(&boxes)
            .map(|(ent, b)| {
                let (mins, maxs) = b.unwrap_or((ent.origin, ent.origin));
                cod2_cookie::Candidate {
                    origin: ent.origin,
                    mins,
                    maxs,
                }
            })
            .collect();
        let sun = glam::Vec3::from(c2.sun().dir);
        let now_ms = (frame.time * 1000.0) as i32;
        let cookies = self
            .cookies
            .prepare(&self.queue, sun, frame.eye, &cands, now_ms);
        let mut draws = Vec::new();
        for (ci, c) in cookies.iter().enumerate() {
            let parts = &self.cookies.entities[c.candidate].parts;
            for (slot, ((m, _), src)) in d.instances.iter().zip(&d.instance_src).enumerate() {
                if !parts.contains(src) {
                    continue;
                }
                let model = &d.models[*m];
                for (surf, skin) in model.vm.surfaces.iter().zip(&model.skins) {
                    if !skin.cookie_caster {
                        continue;
                    }
                    let indices = surf.first_index..surf.first_index + surf.index_count;
                    // the instance's last caster draw takes a surface whose
                    // indices follow on: the same triangles, in order
                    if let Some(last) =
                        draws.last_mut().filter(|l: &&mut cod2_cookie::CasterDraw| {
                            l.cookie == ci
                                && l.instance == slot as u32
                                && l.base_vertex == surf.base_vertex
                                && l.indices.end == indices.start
                        })
                    {
                        last.indices.end = indices.end;
                        continue;
                    }
                    draws.push(cod2_cookie::CasterDraw {
                        cookie: ci,
                        vertex_buf: &model.vm.vertex_buf,
                        index_buf: &model.vm.index_buf,
                        indices,
                        base_vertex: surf.base_vertex,
                        instance: slot as u32,
                    });
                }
            }
        }
        crate::frame_prof::count(crate::frame_prof::Count::CookieDraws, draws.len());
        self.cookies.encode(
            encoder,
            &draws,
            &d.instance_buf,
            DYNAMIC_INSTANCE_STRIDE,
            &d.bone_bg,
        );
    }

    pub(super) fn cookie_receivers(&mut self) -> CookieReceivers<'_> {
        CookieReceivers {
            device: &self.device,
            queue: &self.queue,
            world: self.world.as_ref(),
            cookies: &mut self.cookies,
            dyn_shadows: &mut self.dyn_shadows,
            depth_view: &self.depth_view,
            cod2: self.dynamic.cod2_light.is_some(),
        }
    }

    /// Whether this frame's cookie receivers draw over the world's depth
    /// prepass, between it and the entities' (retail's order, and each
    /// pixel shades once): with the prepass on, one sample.
    pub(super) fn cookie_receivers_on_prepass(&self) -> bool {
        self.depth_prepass
            && self.world.is_some()
            && self.dynamic.cod2_light.is_some()
            && msaa_samples() == 1
            && self.cookies_on_prepass
    }

    /// The cookies' receivers into the dynamic shadow the main pass's lit
    /// world and models read; with no cookie it takes no sun. With
    /// `prepass` they test against the scene depth the world's prepass
    /// left (`cod2-client-perf.md` 11).
    /// An associated function over the fields it reads, so a caller
    /// holding the scene's targets can run it.
    pub(super) fn encode_cookie_receivers(
        r: CookieReceivers<'_>,
        encoder: &mut wgpu::CommandEncoder,
        frame: &Frame,
        prepass: bool,
    ) {
        let CookieReceivers {
            device,
            queue,
            world,
            cookies,
            dyn_shadows,
            depth_view,
            cod2,
        } = r;
        if !cod2 {
            return;
        }
        let Some(world) = world else {
            return;
        };
        let views: Vec<(glam::Mat4, glam::Mat4)> = std::iter::once(glam::Mat4::IDENTITY)
            .chain(world.moved.iter().map(|&(_, pose)| pose))
            .map(|pose| (frame.view_proj, pose))
            .collect();
        // each receiving soup's cookie mask, off its box
        let mask_of = cookies.receiver_masks();
        let every = mask_of(None);
        let depth = prepass.then_some(depth_view);
        cookies.encode_receivers(
            device,
            queue,
            encoder,
            dyn_shadows,
            depth,
            &views,
            |pass, set| {
                pass.set_vertex_buffer(0, world.vertex_buf.slice(..));
                pass.set_index_buffer(world.index_buf.slice(..), wgpu::IndexFormat::Uint32);
                // abutting draws of one mask go as one
                let mut run: Option<(std::ops::Range<u32>, u32)> = None;
                let mut add = |pass: &mut wgpu::RenderPass<'_>,
                               next: std::ops::Range<u32>,
                               mask| {
                    match &mut run {
                        Some((at, m)) if at.end == next.start && *m == mask => at.end = next.end,
                        _ => {
                            if let Some((at, m)) = run.replace((next, mask)) {
                                pass.draw_indexed(at, 0, m..m + 1);
                                crate::frame_prof::count(crate::frame_prof::Count::CookieDraws, 1);
                            }
                        }
                    }
                };
                if set == 0 {
                    // retail's per-cookie receiver lists keep a surface whose
                    // box a cookie's volume reaches (0x10027980): each soup
                    // tests the cookies its own box reaches, and over the
                    // prepass, which a receiver writes no depth into, one
                    // that reaches none is left out
                    for (at, b) in &world.cookie_runs {
                        let mask = match b {
                            Some(b) => mask_of(Some(*b)),
                            None => every,
                        };
                        if mask == 0 && prepass {
                            continue;
                        }
                        add(pass, at.clone(), mask);
                    }
                } else {
                    // a moved brush model's draws test every cookie
                    for r in &world.moved_draws[set - 1].0 {
                        if world.cookie_receive.get(r.batch as usize) == Some(&true) {
                            add(pass, r.first..r.first + r.count, every);
                        }
                    }
                }
                if let Some((at, m)) = run {
                    pass.draw_indexed(at, 0, m..m + 1);
                    crate::frame_prof::count(crate::frame_prof::Count::CookieDraws, 1);
                }
            },
        );
    }

    /// CoD4's sun shadow map for this frame's view (cod4_sunshadow), or
    /// its receivers turned off when `sm_enable` is 0 or the map casts none.
    pub(super) fn encode_sun_shadow(&mut self, encoder: &mut wgpu::CommandEncoder, frame: &Frame) {
        #[cfg(test)]
        let before = self.sun_shadow.redraws();
        self.encode_sun_shadow_maps(encoder, frame);
        #[cfg(test)]
        {
            // a depth texel is 4 bytes: the partitions are 1024 square,
            // the spot tiles 512
            let (drawn, sun, spot) = self.sun_shadow.redraws();
            let part: u64 = 1024 * 1024 * 4;
            if sun > before.1 {
                frame_write!(
                    self,
                    "sun shadow static redraw",
                    u64::from(sun - before.1) * part
                );
            }
            if drawn {
                frame_write!(self, "sun shadow atlas (static blit + entities)", 2 * part);
            }
            if spot > before.2 {
                frame_write!(
                    self,
                    "spot shadow static redraw",
                    u64::from(spot - before.2) * part / 4
                );
            }
            let tiles = self.sun_shadow.spots().len();
            if tiles > 0 {
                frame_write!(
                    self,
                    "spot shadow tiles (static blit + entities)",
                    tiles as u64 * part / 4
                );
            }
        }
    }

    pub(super) fn encode_sun_shadow_maps(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        frame: &Frame,
    ) {
        let (on, settings) = self.sun_shadow_set;
        let world = self.world.as_ref();
        let sun = self.dynamic.cod4_light.as_ref().map(|l| l.sun_dir);
        let casters = world
            .and_then(|w| w.sun_casters.as_ref())
            .filter(|_| on && sun.is_some());
        let casters = casters.zip(world.map(|w| &w.vertex_buf));
        // every live entity's casting surfaces at its picked LOD
        let d = &self.dynamic;
        let mats = &d.materials;
        let mut draws = Vec::new();
        if casters.is_some() {
            for (slot, (m, _)) in d.instances.iter().enumerate() {
                let model = &d.models[*m];
                for (surf, skin) in model.vm.surfaces.iter().zip(&model.skins) {
                    if skin.sun_caster {
                        draws.push(crate::cod4_sunshadow::EntityCaster {
                            cut: skin.sun_cut.map(|image| match image {
                                StageImage::Skin => &model.vm.bind_groups[surf.bind_group],
                                StageImage::Own(k) => &mats.images[k],
                            }),
                            vertex_buf: &model.vm.vertex_buf,
                            index_buf: &model.vm.index_buf,
                            indices: surf.first_index..surf.first_index + surf.index_count,
                            base_vertex: surf.base_vertex,
                            instance: slot as u32,
                            bounds: d.light_boxes.get(slot).copied(),
                        });
                    }
                }
            }
        }
        let entities = crate::cod4_sunshadow::Entities {
            draws,
            instance_buf: &d.instance_buf,
            bones: &d.bone_bg,
            bone_layout: &d.bone_layout,
            image_layout: &mats.skin_layout,
        };
        let lod = self.cod4_lod;
        self.sun_shadow
            .pick_smodel_lods(casters.map(|c| c.0), frame.eye, lod);
        self.sun_shadow.encode(
            &self.device,
            &self.queue,
            encoder,
            casters,
            Some(&entities),
            (frame.view_proj, frame.eye, frame.fwd, crate::camera::Z_NEAR),
            sun.unwrap_or(glam::Vec3::Z),
            &settings,
        );
        // the spot light's map under `sm_enable` too (`R_GetAllowShadowMaps`),
        // over the same casters
        let lights = d.cod4_light.as_ref().map_or(&[][..], |l| &l.lights[..]);
        // the lights a visible surface stores (`scene.shadowableLightIsUsed`):
        // this frame's world and static model draws
        let mut used = vec![false; lights.len()];
        if let Some(w) = world {
            for r in &w.draws {
                if let Some(u) = w
                    .batch_lights
                    .get(r.batch as usize)
                    .and_then(|&l| used.get_mut(usize::from(l)))
                {
                    *u = true;
                }
            }
        }
        self.sun_shadow.encode_spot(
            &self.device,
            &self.queue,
            encoder,
            casters,
            Some(&entities),
            (lights, &used),
            (frame.eye, frame.fwd, frame.time),
            &self.spot_shadow_set,
        );
    }
}

/// `vc_shadows`' casters from the world: the soups and props that draw
/// opaque, per [`ShadowCast`]; the deformed ones apart.
pub(super) fn shadow_casters(device: &wgpu::Device, world: &WorldGpu) -> shadows::Casters {
    let casts = |r: &IndexRange| world.shadow_cast[r.batch as usize] != ShadowCast::None;
    let range = |r: &IndexRange, bounds| shadows::CasterRange {
        first: r.first,
        count: r.count,
        bounds,
        cut: match world.shadow_cast[r.batch as usize] {
            ShadowCast::Cut | ShadowCast::Stage(_) | ShadowCast::Deformed(_) => Some(r.batch),
            ShadowCast::None | ShadowCast::Solid => None,
        },
    };
    let mut models: Vec<Vec<shadows::CasterRange>> =
        world.model_soups.iter().map(|_| Vec::new()).collect();
    let deforms =
        |r: &IndexRange| matches!(world.shadow_cast[r.batch as usize], ShadowCast::Deformed(_));
    let (mut flat, mut deformed) = (Vec::new(), Vec::new());
    for (si, r) in world.soup_ranges.iter().enumerate() {
        let Some(r) = r.filter(casts).filter(|_| !world.coarse_lod[si]) else {
            continue;
        };
        let c = range(&r, world.soup_bounds[si]);
        match world.soup_model[si] as usize {
            _ if deforms(&r) => deformed.push(c),
            0 => flat.push(c),
            m => models[m].push(c),
        }
    }
    // LOD 0 alone: the static layer is cached across views, so it can't
    // follow the camera's pick.
    for (p, _, r) in world.prop_ranges.iter().filter(|(_, l, _)| *l == 0) {
        let bounds = world.prop_bounds[*p as usize];
        if casts(r) && bounds.0 != bounds.1 {
            if deforms(r) {
                deformed.push(range(r, bounds));
            } else {
                flat.push(range(r, bounds));
            }
        }
    }
    let count = |c: ShadowCast| world.shadow_cast.iter().filter(|&&s| s == c).count();
    let staged = world
        .shadow_cast
        .iter()
        .filter(|s| matches!(s, ShadowCast::Stage(_)))
        .count();
    log::info!(
        "shadow casters: {} solid, {} implicit cutout, {} staged cutout batches, {} deformed casters",
        count(ShadowCast::Solid),
        count(ShadowCast::Cut),
        staged,
        deformed.len()
    );
    shadows::Casters::build(device, &world.cpu_indices, &flat, &models, &deformed)
}
