//! Encodes the world's draws: the draw groups and lit sets, one world draw into
//! a pass or a bundle, the depth prepass and the dlight pass.

use super::*;

/// Which draws one depth prepass pass takes ([`Renderer::encode_depth_prepass`]):
/// the world's, the entities', `floatZ` too, and whether it keeps the
/// depth an earlier part left.
#[derive(Clone, Copy)]
pub(super) struct PrepassParts {
    pub(super) world: bool,
    pub(super) ents: bool,
    pub(super) floatz: bool,
    pub(super) load: bool,
}

/// Whether world draw `kind` takes the depth prepass
/// ([`cod2::Cod2World::takes_prepass`]): only the lit family's opaque ones.
pub(super) fn takes_prepass(world: &WorldGpu, kind: DrawRef) -> bool {
    let DrawRef::Stage(i) = kind else {
        return false;
    };
    let sb = &world.stage_batches[i as usize];
    match (sb.cod2, &world.cod2) {
        (Some(lit), Some(c2)) => c2.takes_prepass(sb.variant, sb.two_sided, sb.bias, &lit),
        _ => false,
    }
}

/// Whether world draw `kind` writes `floatZ` in the depth prepass of a
/// CoD4 frame a feather reads, and whether it alpha tests there
/// ([`cod2::Cod2World::takes_floatz`]).
pub(super) fn takes_floatz(world: &WorldGpu, kind: DrawRef) -> Option<bool> {
    let DrawRef::Stage(i) = kind else {
        return None;
    };
    let sb = &world.stage_batches[i as usize];
    match (sb.cod2, &world.cod2) {
        (Some(lit), Some(c2)) => c2.takes_floatz(sb.variant, &lit),
        _ => None,
    }
}

/// Whether an entity surface on `skin` takes the depth prepass: its
/// first stage is opaque, depth written and not alpha tested.
pub(super) fn ent_takes_prepass(skin: &EntSkin) -> bool {
    skin.stages.first().is_some_and(|st| st.prepass)
}

impl WorldGpu {
    /// Stage params slot `slot`'s dynamic offset into `stage_params_buf`.
    pub(super) fn stage_offset(&self, slot: u32) -> u32 {
        u32::try_from(u64::from(slot) * self.stage_params_stride)
            .expect("stage params slot offset fits a dynamic offset")
    }
}

impl Renderer {
    /// The profile's GPU group of the main pass's `item` (`frame_prof::Group`):
    /// a world draw on a static model's lit set is `Smodel`; `soft` says
    /// whether an fx run reads the depth copy.
    pub(super) fn draw_group(
        &self,
        item: SortedItem,
        world_list: &[WorldItem],
        ent_draws: &[EntDraw],
        soft: impl Fn(usize) -> bool,
    ) -> crate::frame_prof::Group {
        use crate::frame_prof::Group;
        match item {
            SortedItem::World(wi) => {
                let WorldItem { set, draw: i, .. } = world_list[wi];
                let smodel = self.world.as_ref().is_some_and(|world| {
                    self.world_sets(world).nth(set).is_some_and(|(_, fds, _)| {
                        match fds[i as usize].kind {
                            DrawRef::Stage(si) => {
                                world.stage_batches[si as usize].cod2.is_some_and(|b| {
                                    matches!(
                                        b.entry,
                                        cod2::LitEntry::Cod4Model | cod2::LitEntry::Smodel
                                    )
                                })
                            }
                            DrawRef::Legacy(_) => false,
                        }
                    })
                });
                if smodel { Group::Smodel } else { Group::World }
            }
            SortedItem::Model(mi) if ent_draws[mi].gun => Group::Gun,
            SortedItem::Model(_) => Group::Ents,
            SortedItem::Mark(_) => Group::Marks,
            SortedItem::Fx(fi) if soft(fi) => Group::FxSoft,
            SortedItem::Fx(_) => Group::Fx,
        }
    }

    /// Whether `item` is a CoD1 static model draw (`Pass::Prop`), which the
    /// fragment counts keep apart.
    pub(super) fn is_prop_draw(&self, item: SortedItem, world_list: &[WorldItem]) -> bool {
        let SortedItem::World(wi) = item else {
            return false;
        };
        let WorldItem { set, draw: i, .. } = world_list[wi];
        self.world.as_ref().is_some_and(|world| {
            self.world_sets(world).nth(set).is_some_and(|(_, fds, _)| {
                matches!(fds[i as usize].kind, DrawRef::Legacy(Pass::Prop { .. }))
            })
        })
    }

    /// The world's draw sets: the world under the shared camera, then each
    /// moved brush model under its own.
    pub(super) fn world_sets<'a>(
        &'a self,
        world: &'a WorldGpu,
    ) -> impl Iterator<Item = (&'a Vec<DrawRange>, &'a Vec<FrameDraw>, &'a wgpu::BindGroup)> {
        std::iter::once((&world.draws, &world.frame_draws, &self.camera_bg)).chain(
            world
                .moved_draws
                .iter()
                .zip(&self.moved_cams)
                .map(|((d, o), (_, bg))| (d, o, bg)),
        )
    }

    /// Which `cod4_worldfx` stages the frame's gathered world draws hold:
    /// (feathered, distortion).
    pub(super) fn world_fx_drawn(&self, world: &WorldGpu) -> (bool, bool) {
        use cod4_worldfx::WorldFx;
        let mut out = (false, false);
        if self.world_fx.is_none() {
            return out;
        }
        for (_, frame_draws, _) in self.world_sets(world) {
            for fd in frame_draws {
                if let DrawRef::Stage(sb) = fd.kind {
                    match world.stage_batches[sb as usize].fx {
                        WorldFx::Feather => out.0 = true,
                        WorldFx::Distort => out.1 = true,
                        WorldFx::None => {}
                    }
                }
            }
        }
        out
    }

    /// One world draw on its pipeline; `bound` tracks the bound pipeline.
    /// `fx_ready` says whether this frame's depth and colour copies are
    /// taken for a `cod4_worldfx` stage: without its copy a feathered one
    /// draws as its plain stage, a distortion one not at all.
    #[expect(clippy::too_many_arguments)]
    pub(super) fn emit_world_draw(
        &self,
        pass: &mut impl DrawSink,
        world: &WorldGpu,
        draw: &DrawRange,
        kind: DrawRef,
        bound: &mut Option<Bound>,
        groups: &mut WorldGroups,
        time: f32,
        fx_ready: (bool, bool),
        depth: Depth,
    ) {
        if !self.draw_joins {
            *groups = WorldGroups::default();
        }
        if let DrawRef::Stage(sb_idx) = kind
            && world.stage_batches[sb_idx as usize].fx != cod4_worldfx::WorldFx::None
        {
            use cod4_worldfx::WorldFx;
            let sb = &world.stage_batches[sb_idx as usize];
            let ready = match sb.fx {
                WorldFx::Feather => fx_ready.0,
                _ => fx_ready.1,
            };
            let tangents = world.cod2.as_ref().map(|c| &c.tangent_buf);
            let pipe = self
                .world_fx
                .as_ref()
                .filter(|_| ready && (sb.fx == WorldFx::Feather || tangents.is_some()))
                .and_then(|p| p.pipeline(sb.fx, sb.variant, sb.two_sided, sb.bias));
            match pipe {
                Some(p) => {
                    let key = Bound::Fx(sb.fx, sb.variant, sb.two_sided, sb.bias);
                    if *bound != Some(key) {
                        pass.pipeline(p);
                        pass.group(3, self.scene_copy.world_bind_group(), None);
                        if let (WorldFx::Distort, Some(t)) = (sb.fx, tangents) {
                            pass.vertices(1, t.slice(..));
                        }
                        *bound = Some(key);
                        *groups = WorldGroups::default();
                        tally(&self.census, |c| {
                            c.pipelines += 1;
                            c.bind_groups += 1;
                        });
                    }
                    groups.bind(pass, 1, sb.mat.bind_group(time), None, &self.census);
                    groups.bind(
                        pass,
                        2,
                        sb.stage.bind_group(time),
                        Some(world.stage_offset(sb.slot)),
                        &self.census,
                    );
                    pass.draw(draw.first..draw.first + draw.count);
                    tally(&self.census, |c| c.world += 1);
                    return;
                }
                None if sb.fx == WorldFx::Distort => return,
                None => {}
            }
        }
        match kind {
            DrawRef::Legacy(want) => {
                if *bound != Some(Bound::Legacy(want)) {
                    pass.pipeline(match want {
                        Pass::Opaque => &self.pipeline,
                        Pass::Solid => &self.solid_pipeline,
                        Pass::Prop { culled, solid } => {
                            &self.prop_pipelines[usize::from(culled)][usize::from(solid)]
                        }
                        Pass::Layer => &self.layer_pipeline,
                        Pass::Overlay => &self.overlay_pipeline,
                        Pass::Stage => return,
                    });
                    *bound = Some(Bound::Legacy(want));
                    *groups = WorldGroups::default();
                    tally(&self.census, |c| c.pipelines += 1);
                }
                let call = &world.batch_draws[draw.batch as usize];
                groups.bind(
                    pass,
                    1,
                    &world.bind_groups[call.bind_group],
                    None,
                    &self.census,
                );
            }
            DrawRef::Stage(sb_idx)
                if let (Some(lit), Some(c2)) =
                    (world.stage_batches[sb_idx as usize].cod2, &world.cod2) =>
            {
                let sb = &world.stage_batches[sb_idx as usize];
                let key = Bound::Lit(
                    sb.variant,
                    sb.two_sided,
                    lit.offset,
                    c2.fs_key(&lit),
                    lit.water,
                    depth,
                );
                let water = c2.water.as_ref().filter(|_| lit.water);
                if *bound != Some(key) {
                    // one lit pipeline after another (a split CoD4 path's,
                    // cod4-client-perf.md 29) shares its layout and buffers:
                    // the groups and tangents bound stay
                    let kept = !lit.water && matches!(*bound, Some(Bound::Lit(.., false, _)));
                    pass.pipeline(match (water, depth) {
                        (Some(w), _) => w.pipeline(sb.variant, sb.two_sided, lit.offset),
                        (None, Depth::Plain) => c2.pipeline(sb.variant, sb.two_sided, &lit),
                        (None, Depth::Prepass) => c2.prepass_pipeline(sb.two_sided, None),
                        (None, Depth::FloatZ) => c2.floatz_pipeline(
                            sb.two_sided,
                            c2.takes_floatz(sb.variant, &lit) == Some(true),
                        ),
                        (None, Depth::Equal) => c2.prepass_pipeline(sb.two_sided, Some(&lit)),
                    });
                    if !kept {
                        pass.vertices(1, c2.tangent_buf.slice(..));
                        *groups = WorldGroups::default();
                    }
                    *bound = Some(key);
                    tally(&self.census, |c| c.pipelines += 1);
                }
                let (mat, second) = match water {
                    Some(w) => (&w.material_bgs, &w.sims[lit.lightmap as usize].bg),
                    None => (&c2.material_bgs, &c2.lightmap_bgs[lit.lightmap as usize]),
                };
                groups.bind(pass, 1, &mat[lit.material as usize], None, &self.census);
                groups.bind(pass, 2, second, None, &self.census);
            }
            DrawRef::Stage(sb_idx) => {
                let sb = &world.stage_batches[sb_idx as usize];
                let key = Bound::Stage(sb.variant, sb.two_sided, sb.bias);
                if *bound != Some(key) {
                    pass.pipeline(self.stage_pipeline(sb.variant, sb.two_sided, sb.bias));
                    *bound = Some(key);
                    *groups = WorldGroups::default();
                    tally(&self.census, |c| c.pipelines += 1);
                }
                // animMap stages pick their frame per draw
                groups.bind(pass, 1, sb.mat.bind_group(time), None, &self.census);
                groups.bind(
                    pass,
                    2,
                    sb.stage.bind_group(time),
                    Some(world.stage_offset(sb.slot)),
                    &self.census,
                );
            }
        }
        pass.draw(draw.first..draw.first + draw.count);
        tally(&self.census, |c| match depth {
            Depth::Prepass | Depth::FloatZ => c.prepass_world += 1,
            _ => c.world += 1,
        });
    }

    /// The depth prepass's world draws joined. A pass that only writes the
    /// nearest depth (LessEqual, no colour) leaves every sample at its
    /// nearest fragment whatever the draw order, so per set, sidedness and
    /// flag wave (the one material switch `vs_main`'s position reads) the
    /// gathered ranges go in index order and ranges that touch draw as
    /// one; batches of one material sit side by side in the gathered
    /// buffer. With `floatz` the draws [`takes_floatz`] accepts write
    /// `floatZ` too; an alpha-tested one reads its material's colour map,
    /// so its ranges join only within the material. Returns the draws.
    pub(super) fn encode_prepass_world_joined(
        &self,
        pass: &mut impl DrawSink,
        world: &WorldGpu,
        world_list: &[WorldItem],
        floatz: bool,
    ) -> usize {
        let Some(c2) = &world.cod2 else {
            return 0;
        };
        // (set, two-sided, waves, the material when alpha tested), first,
        // count, material, lightmap
        type Range = ((usize, bool, bool, Option<u32>), u32, u32, u32, u32);
        let mut ranges: Vec<Range> = Vec::new();
        for &WorldItem { set, draw: i, .. } in world_list {
            let (draws, frame_draws, _) = self
                .world_sets(world)
                .nth(set)
                .expect("collected from these sets");
            let fd = &frame_draws[i as usize];
            let atest = match floatz {
                true => match takes_floatz(world, fd.kind) {
                    Some(atest) => atest,
                    None => continue,
                },
                false if takes_prepass(world, fd.kind) => false,
                false => continue,
            };
            let DrawRef::Stage(sb) = fd.kind else {
                continue;
            };
            let sb = &world.stage_batches[sb as usize];
            let Some(lit) = sb.cod2 else {
                continue;
            };
            let d = &draws[fd.src as usize];
            ranges.push((
                (
                    set,
                    sb.two_sided,
                    c2.waves(lit.material),
                    atest.then_some(lit.material),
                ),
                d.first,
                d.count,
                lit.material,
                lit.lightmap,
            ));
        }
        ranges.sort_unstable_by_key(|r| (r.0, r.1));
        // a batch's stages share its range
        ranges.dedup_by_key(|r| (r.0, r.1));
        let mut n = 0;
        let mut cam_set = None;
        let mut key = None;
        let mut k = 0;
        while k < ranges.len() {
            let (group, first, mut count, material, lightmap) = ranges[k];
            let (set, two_sided, waves, atest) = group;
            k += 1;
            while let Some(&(g2, f2, c, ..)) = ranges.get(k)
                && g2 == group
                && f2 == first + count
            {
                count += c;
                k += 1;
            }
            if cam_set != Some(set) {
                let (_, _, cam) = self
                    .world_sets(world)
                    .nth(set)
                    .expect("collected from these sets");
                pass.group(0, cam, None);
                cam_set = Some(set);
                tally(&self.census, |c| c.bind_groups += 1);
            }
            if key != Some((two_sided, waves, atest)) {
                pass.pipeline(match floatz {
                    true => c2.floatz_pipeline(two_sided, atest.is_some()),
                    false => c2.prepass_pipeline(two_sided, None),
                });
                pass.vertices(1, c2.tangent_buf.slice(..));
                pass.group(1, &c2.material_bgs[material as usize], None);
                pass.group(2, &c2.lightmap_bgs[lightmap as usize], None);
                key = Some((two_sided, waves, atest));
                tally(&self.census, |c| {
                    c.pipelines += 1;
                    c.bind_groups += 2;
                });
            }
            pass.draw(first..first + count);
            tally(&self.census, |c| c.prepass_world += 1);
            n += 1;
        }
        n
    }

    /// `r_vc_depthprepass`' depth-only draws: every world draw
    /// [`takes_prepass`] accepts across the world's sets and, with `ents`,
    /// every non-gun entity surface whose skin [`ent_takes_prepass`].
    /// Its own pass, with no colour target; the scene's first pass loads
    /// the depth it leaves. With `floatz` (a CoD4 frame a feather reads,
    /// cod4-client-perf.md 35) it takes 1.7's set instead, the draws whose
    /// material has a `build floatz` technique and prepass key 0 to 2
    /// ([`takes_floatz`], alpha-tested ones among them), and every draw
    /// writes the view depth into the feather's R32F target, as 1.7's
    /// prepass writes `floatZ` (`R_DepthPrepassCallback`, iw3mp.exe
    /// 0x658f40).
    pub(super) fn encode_depth_prepass(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        world_list: &[WorldItem],
        ent_draws: &[EntDraw],
        PrepassParts {
            world: world_draws,
            ents,
            floatz,
            load,
        }: PrepassParts,
        time: f32,
        bundles: Option<&mut WorldBundles>,
    ) -> usize {
        let Some(world) = &self.world else {
            return 0;
        };
        // the cleared far plane reads as the copy's `far`
        let floatz_target = floatz.then(|| wgpu::RenderPassColorAttachment {
            view: self.soft.linz(),
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations {
                load: match load {
                    true => wgpu::LoadOp::Load,
                    false => wgpu::LoadOp::Clear(wgpu::Color {
                        r: f64::from(crate::camera::Z_FAR),
                        g: 0.0,
                        b: 0.0,
                        a: 0.0,
                    }),
                },
                store: wgpu::StoreOp::Store,
            },
        });
        let depth = match floatz {
            true => Depth::FloatZ,
            false => Depth::Prepass,
        };
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("depth prepass"),
            color_attachments: match &floatz_target {
                Some(_) => std::slice::from_ref(&floatz_target),
                None => &[],
            },
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &self.depth_view,
                depth_ops: Some(wgpu::Operations {
                    load: match load {
                        true => wgpu::LoadOp::Load,
                        false => wgpu::LoadOp::Clear(1.0),
                    },
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        let pass = &mut pass;
        let mut n = 0;
        let mut bundles = bundles;
        pass.set_vertex_buffer(0, world.vertex_buf.slice(..));
        pass.set_index_buffer(world.index_buf.slice(..), wgpu::IndexFormat::Uint32);
        let mut bound = None;
        let mut cam_set = None;
        if !world_draws {
            // the entities' part alone
        } else if let Some(b) = bundles.as_deref_mut()
            && self.draw_joins
        {
            let mut rec = Recording::default();
            rec.vertices(0, world.vertex_buf.slice(..));
            rec.indices(world.index_buf.slice(..), wgpu::IndexFormat::Uint32);
            n += self.encode_prepass_world_joined(&mut rec, world, world_list, floatz);
            b.draw(
                pass,
                &self.device,
                rec,
                crate::frame_prof::Count::PreBundled,
            );
        } else if self.draw_joins {
            n += self.encode_prepass_world_joined(pass, world, world_list, floatz);
        } else {
            for &WorldItem { set, draw: i, .. } in world_list {
                let (draws, frame_draws, cam) = self
                    .world_sets(world)
                    .nth(set)
                    .expect("collected from these sets");
                let fd = &frame_draws[i as usize];
                let takes = match floatz {
                    true => takes_floatz(world, fd.kind).is_some(),
                    false => takes_prepass(world, fd.kind),
                };
                if !takes {
                    continue;
                }
                if cam_set != Some(set) {
                    pass.set_bind_group(0, cam, &[]);
                    cam_set = Some(set);
                    tally(&self.census, |c| c.bind_groups += 1);
                }
                let draw = &draws[fd.src as usize];
                self.emit_world_draw(
                    pass,
                    world,
                    draw,
                    fd.kind,
                    &mut bound,
                    &mut WorldGroups::default(),
                    time,
                    (false, false),
                    depth,
                );
                n += 1;
            }
        }
        if !ents {
            return n;
        }
        match bundles {
            Some(b) => {
                let mut rec = Recording::default();
                n += self.encode_prepass_ents(&mut rec, ent_draws, floatz, depth);
                b.draw(
                    pass,
                    &self.device,
                    rec,
                    crate::frame_prof::Count::EntBundled,
                );
            }
            None => n += self.encode_prepass_ents(pass, ent_draws, floatz, depth),
        }
        n
    }

    /// The depth prepass's entity surfaces ([`Renderer::encode_depth_prepass`])
    /// into `sink`, binding all they read. Returns the draws.
    pub(super) fn encode_prepass_ents(
        &self,
        sink: &mut impl DrawSink,
        ent_draws: &[EntDraw],
        floatz: bool,
        depth: Depth,
    ) -> usize {
        let mut n = 0;
        let dynamic = &self.dynamic;
        sink.group(0, &self.camera_bg, None);
        sink.group(2, &dynamic.bone_bg, None);
        tally(&self.census, |c| c.bind_groups += 2);
        // (instance, surface) of every entity surface the prepass takes
        let mut surfs: Vec<(u32, u32)> = ent_draws
            .iter()
            .filter(|d| !d.gun)
            .filter(|d| {
                let model = &dynamic.models[dynamic.instances[d.inst as usize].0];
                let skin = &model.skins[d.surf as usize];
                match floatz {
                    true => skin.stages.first().is_some_and(|st| st.floatz),
                    false => ent_takes_prepass(skin),
                }
            })
            .map(|d| (d.inst, d.surf))
            .collect();
        // the nearest depth wins in any order, so an instance's rigid
        // surfaces go in surface order and the touching ones join
        if self.draw_joins {
            surfs.sort_unstable();
        }
        let rigid = |skin: &EntSkin| skin.stages.first().is_some_and(|st| st.rigid);
        let mut bound = EntBound::default();
        let mut bound_model = None;
        sink.vertices(1, dynamic.instance_buf.slice(..));
        let mut k = 0;
        while k < surfs.len() {
            let (i, s) = surfs[k];
            k += 1;
            let model_at = dynamic.instances[i as usize].0;
            let model = &dynamic.models[model_at];
            let skin = &model.skins[s as usize];
            let vm = &model.vm;
            if bound_model != Some(model_at) {
                sink.vertices(0, vm.vertex_buf.slice(..));
                sink.indices(vm.index_buf.slice(..), wgpu::IndexFormat::Uint16);
                bound_model = Some(model_at);
            }
            let surf = &vm.surfaces[s as usize];
            let mut end = surf.first_index + surf.index_count;
            if self.draw_joins && rigid(skin) {
                while let Some(&(i2, s2)) = surfs.get(k)
                    && i2 == i
                    && let Some(next) = vm.surfaces.get(s2 as usize)
                    && rigid(&model.skins[s2 as usize])
                    && next.base_vertex == surf.base_vertex
                    && next.first_index == end
                {
                    end += next.index_count;
                    k += 1;
                }
            }
            draw_ent_stages(
                sink,
                &dynamic.materials,
                EntPass::World,
                &vm.bind_groups[surf.bind_group],
                skin,
                surf,
                (surf.first_index..end, i),
                &mut bound,
                depth,
                &self.census,
            );
            n += 1;
        }
        n
    }

    /// Each light over the opaque surfaces it reaches (`dlight.rs`).
    pub(super) fn emit_dlights(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        world: &WorldGpu,
        time: f32,
    ) {
        pass.set_vertex_buffer(0, world.vertex_buf.slice(..));
        pass.set_vertex_buffer(1, world.s_axis_buf.slice(..));
        pass.set_index_buffer(world.dlight_index_buf.slice(..), wgpu::IndexFormat::Uint32);
        let mut bound = None;
        for run in &world.dlight_runs {
            let cam = run
                .moved
                .map_or(&self.camera_bg, |mi| &self.moved_cams[mi].1);
            pass.set_bind_group(0, cam, &[]);
            pass.set_bind_group(
                3,
                &self.dlight.bg,
                &[(u64::from(run.light) * self.dlight.stride) as u32],
            );
            tally(&self.census, |c| c.bind_groups += 2);
            for &db in &world.dlights_of_batch[run.draw.batch as usize] {
                let d = &world.dlight_batches[db as usize];
                let key = (d.blend, d.two_sided, d.bias);
                if bound != Some(key) {
                    pass.set_pipeline(self.dlight.pipeline(d.blend, d.two_sided, d.bias));
                    bound = Some(key);
                    tally(&self.census, |c| c.pipelines += 1);
                }
                tally(&self.census, |c| {
                    c.bind_groups += 2;
                    c.dlights += 1;
                });
                pass.set_bind_group(1, d.mat.bind_group(time), &[]);
                pass.set_bind_group(2, d.stage.bind_group(time), &[world.stage_offset(d.slot)]);
                pass.draw_indexed(run.draw.first..run.draw.first + run.draw.count, 0, 0..1);
            }
        }
        pass.set_bind_group(0, &self.camera_bg, &[]);
    }
}
