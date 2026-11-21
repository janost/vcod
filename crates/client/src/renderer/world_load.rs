//! Loading a map onto the GPU: `WorldGpu`, the material library,
//! `Renderer::load_world` and its helpers, submodels, reopen and unload.

use super::*;

/// Everything built from one map: buffers, material bind groups, batches,
/// visibility. Dropped whole on a map change.
pub(super) struct WorldGpu {
    pub(super) vertex_buf: wgpu::Buffer,
    /// CoD2's lit draws; `None` on a CoD1 map.
    pub(super) cod2: Option<cod2::Cod2World>,
    pub(super) index_buf: wgpu::Buffer,
    pub(super) bind_groups: Vec<wgpu::BindGroup>,
    /// Merged CPU indices (world then props); `index_buf` holds the gathered visible subset.
    pub(super) cpu_indices: Vec<u32>,
    pub(super) soup_ranges: Vec<Option<IndexRange>>,
    /// (placement, LOD, range).
    pub(super) prop_ranges: Vec<(u32, u8, IndexRange)>,
    /// LOD 0's box per placement.
    pub(super) prop_bounds: Vec<(glam::Vec3, glam::Vec3)>,
    pub(super) prop_lods: Vec<props::PropLod>,
    /// Each placement's LOD at the last gather, `None` past its last.
    pub(super) prop_lod: Vec<Option<u8>>,
    /// Static per-batch draw descriptors; `draws` is rebuilt each frame from them.
    pub(super) batch_draws: Vec<DrawCall>,
    /// Per batch, its CoD4 primary light (`mesh::Batch::light`; 0 for a
    /// prop): what the frame's `draws` mark used for the spot shadows.
    pub(super) batch_lights: Vec<u8>,
    pub(super) draws: Vec<DrawRange>,
    /// Authored stages expanded from their parent batches.
    pub(super) stage_batches: Vec<StageBatch>,
    /// Parent batch id -> indices into `stage_batches`.
    pub(super) stages_of_batch: Vec<Vec<u32>>,
    /// Animated stages: (shader clone, stage idx, params slot); rewritten each frame.
    pub(super) animated: Vec<(Material, usize, u32)>,
    /// Cloud-dome stages: like `animated` but every entry also gets the view
    /// origin stamped into its params each frame.
    pub(super) sky_stages: Vec<(Material, usize, u32)>,
    /// Per batch, its material sort and its place in the sorted shader
    /// list (`sorted_places`), the draw list's key.
    pub(super) batch_keys: Vec<(f32, u32)>,
    /// This frame's emission-ordered draw list.
    pub(super) frame_draws: Vec<FrameDraw>,
    /// Stage draws emitted last frame, for the F3 overlay.
    pub(super) stage_draws_last: usize,
    /// Per-stage params UBO (one dynamic-offset slot per (material, stage)).
    /// Group(2) bind groups are per stage batch: bundle 1's image varies.
    pub(super) stage_params_buf: wgpu::Buffer,
    /// StageParams size rounded up to `min_uniform_buffer_offset_alignment`.
    pub(super) stage_params_stride: u64,
    pub(super) vis: WorldVis,
    pub(super) gathered: Vec<u32>,
    pub(super) gather_scratch: Vec<Vec<IndexRange>>,
    /// The ranges the world's part of `gathered` (its first
    /// `world_gathered` indices) was gathered from, so a frame whose
    /// visible set holds still neither gathers, orders nor uploads it again.
    pub(super) gathered_from: Vec<IndexRange>,
    pub(super) world_gathered: usize,
    /// The frozen set and the prop flags it was computed with, so `Locked`
    /// holds props still too.
    pub(super) locked: Option<(Visible, Vec<bool>)>,
    /// Last frame's cull and its eye, view and LOD ramp's bits: a frame
    /// that holds all three takes it again (cod4-client-perf.md 31).
    pub(super) culled: Option<(CullKey, (Visible, Vec<bool>))>,
    /// The lump-27 model each soup belongs to, 0 for the world's own.
    pub(super) soup_model: Vec<u32>,
    /// CoD4's static model placements, each drawn at one LOD a frame
    /// ([`vcod_common::mapworld::StaticLod`]).
    pub(super) static_lods: Vec<vcod_common::mapworld::StaticLod>,
    /// Per soup, whether it is a static model's LOD past 0: the cached
    /// casters and the unculled view draw LOD 0 alone.
    pub(super) coarse_lod: Vec<bool>,
    /// The world's sky soups and their boxes (`sun::sky_drawn`).
    pub(super) sky_soups: Vec<(usize, (glam::Vec3, glam::Vec3))>,
    /// Per model, whether its soups draw with the world this frame. A
    /// submodel draws only through the entity that carries it
    /// ([`Renderer::set_submodels`]); model 0 always draws.
    pub(super) static_models: Vec<bool>,
    /// Per model, its soups, `bsp.models[m]`'s `first_soup..+num_soups`.
    pub(super) model_soups: Vec<std::ops::Range<usize>>,
    /// The brush models drawn off their spawn pose this frame, with that
    /// pose: their soups gathered after the world's, under their own camera.
    pub(super) moved: Vec<(usize, glam::Mat4)>,
    /// Per entry of `moved`, its gathered draws and their emission order.
    pub(super) moved_draws: Vec<(Vec<DrawRange>, Vec<FrameDraw>)>,
    /// Per vertex, the tangent s axis the dlight projection reads
    /// (`dlight::soup_s_axes`; zero for props).
    pub(super) s_axis_buf: wgpu::Buffer,
    /// Per soup, its spawn-pose bounds, which a light must reach.
    pub(super) soup_bounds: Vec<(glam::Vec3, glam::Vec3)>,
    /// The dlight pass's stages and, per parent batch, its entries there.
    pub(super) dlight_batches: Vec<DlightBatch>,
    pub(super) dlights_of_batch: Vec<Vec<u32>>,
    /// This frame's per-light ranges, indices into `dlight_index_buf`.
    pub(super) dlight_runs: Vec<DlightRun>,
    pub(super) dlight_indices: Vec<u32>,
    pub(super) dlight_index_buf: wgpu::Buffer,
    /// `vc_shadows`' static casters, built the first frame it is on.
    pub(super) shadow_casters: Option<shadows::Casters>,
    /// Per batch, how it casts in `vc_shadows`.
    pub(super) shadow_cast: Vec<ShadowCast>,
    /// Per batch, a CoD2 BSP surface whose material takes the shadow
    /// cookies (`cod2_cookie`; static models never do).
    pub(super) cookie_receive: Vec<bool>,
    /// By a receiving soup's range (its first index), the soup's bounds:
    /// a receiver draw's box for its cookie mask.
    pub(super) cookie_bounds: HashMap<u32, (glam::Vec3, glam::Vec3)>,
    /// Every receiving soup of the last gather (`gathered_from`), in the
    /// gathered order: its range of `gathered` and its box (`None`: every
    /// cookie), what the receivers draw and mask soup by soup.
    pub(super) cookie_runs: Vec<CookieRun>,
    /// The lightmaps' measured shadow darkness for `vc_shadows`.
    pub(super) sun_keep: SunKeepState,
    /// CoD4's sun shadow map casters; `None` elsewhere.
    pub(super) sun_casters: Option<crate::cod4_sunshadow::Casters>,
}

/// `static_light::sun_keep_regions` for the loaded map: its samples until
/// `vc_shadows` first wants it, then the job, then the keep.
pub(super) enum SunKeepState {
    Samples(Vec<(glam::Vec3, f32)>, f32),
    Running(std::thread::JoinHandle<Option<vcod_common::static_light::RegionKeep>>),
    Done(Option<Arc<vcod_common::static_light::RegionKeep>>),
}

/// How a world batch casts in `vc_shadows`' layers.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(super) enum ShadowCast {
    /// Not at all: layers, decals, past opaque, or no depth-writing stage.
    None,
    Solid,
    /// Alpha-tested at half through the batch's own bind group, as
    /// `fs_main`, `fs_prop` and the overlays' coverage cut it.
    Cut,
    /// Alpha-tested by its first depth-writing stage, this stage batch.
    Stage(u32),
    /// Like `Stage`, but its shader deforms the vertices over time
    /// (`deformVertexes wave` / `flap`), so it casts in the dynamic layer
    /// each frame, deformed, and not in the static one.
    Deformed(u32),
}

/// Records stage batch `sb` (stage `si` of `sh`) as the batch's first
/// depth-writing stage if it is one and none came before. It cuts when it
/// alpha-tests a texture.
pub(super) fn note_first_depth(
    slot: &mut Option<(u32, bool)>,
    sh: &Material,
    si: usize,
    sb: usize,
) {
    if slot.is_some() || !sh.classify_stage(si).is_some_and(|c| c.depth_write) {
        return;
    }
    let st = &sh.stages[si];
    let cuts = st.alpha_func.is_some()
        && st
            .bundles
            .first()
            .is_some_and(|b| !b.takes_lightmap_coords());
    *slot = Some((sb as u32, cuts));
}

/// Encodes the soups of every sprite-deformed map material for `vs_stage`
/// (`vcod_common::deform`), their vertices in `vertices` and their index
/// runs in `indices` in place. Autosprite rewrites a soup's indices as
/// stamps, so a soup whose index count is not six per four vertices keeps
/// its geometry and is warned.
pub(super) fn encode_world_sprites(
    bsp: &MapWorld,
    lib: &MaterialLib,
    soup_ranges: &[Option<IndexRange>],
    vertices: &mut [DrawVert],
    indices: &mut [u32],
) {
    for (soup, range) in bsp.soups.iter().zip(soup_ranges) {
        let Some(r) = range else { continue };
        let name = &bsp.materials[soup.material as usize].name;
        let Some(sprite) = lib.get(name).and_then(|sh| sh.sprite) else {
            continue;
        };
        let first = soup.first_vertex as usize;
        let verts = &mut vertices[first..first + soup.vertex_count as usize];
        let idx = &mut indices[r.first as usize..(r.first + r.count) as usize];
        match sprite {
            Sprite::Auto if verts.len().is_multiple_of(4) && idx.len() == verts.len() / 4 * 6 => {
                idx.copy_from_slice(&deform::encode_autosprite(verts, soup.first_vertex));
            }
            Sprite::Auto => log::warn!("Autosprite shader {name} had odd vertex or index count"),
            Sprite::Auto2 => deform::encode_autosprite2(verts, idx, soup.first_vertex),
        }
    }
}

impl Renderer {
    /// Draws the next map with `lib` in place of the game's materials (a
    /// CoD4 map carries its own).
    pub fn set_material_lib(&mut self, lib: MaterialLib) {
        self.shader_lib = lib;
        self.lib_warn_count = self.shader_lib.warn_count();
        // The mark shaders resolved through the old lib and search path.
        self.marks.reopen();
    }

    /// Builds the map's GPU state; a previous map is dropped first.
    pub fn load_world(&mut self, bsp: &MapWorld, fs: &Pk3Fs) -> Result<()> {
        let mut laps = vcod_common::laps::Laps::new("world GPU state");
        self.unload_world();
        let device = &self.device;
        let queue = &self.queue;
        let material_layout = &self.material_layout;
        let lightmap_sampler = &self.lightmap_sampler;
        let white_view = &self.white_view;
        let white_lightmap = &self.white_lightmap;

        // The leaf lights' trace world builds beside the batches.
        let (kinds, active_sky, (mut indices, batches, soup_ranges), light_world) =
            std::thread::scope(|sc| {
                let light_world = sc.spawn(|| StaticLighting::collision_for(bsp));
                // Per-material draw decision: authored blocks classify from
                // their parsed surface bits, implicit names from the
                // gameplay prefixes.
                let kinds = vcod_common::sun::material_kinds(&self.shader_lib, bsp);
                let active_sky = vcod_common::sun::active_sky(&self.shader_lib, bsp, &kinds);
                // Props (misc_model entities) are baked to world space on the
                // CPU in the same vertex format and extend the map's buffers.
                let built = mesh::build_batches(bsp, &kinds);
                let light_world = vcod_common::laps::joined(light_world);
                (kinds, active_sky, built, light_world)
            });
        let mut soup_model = vec![0u32; bsp.soups.len()];
        for (mi, m) in bsp.models.iter().enumerate().skip(1) {
            let first = m.first_soup as usize;
            for s in soup_model.iter_mut().skip(first).take(m.num_soups as usize) {
                *s = mi as u32;
            }
        }
        if batches.is_empty() {
            bail!("map has no drawable surfaces");
        }
        laps.lap("batches");
        let mut lighting = StaticLighting::with_collision(bsp, light_world);
        let lib = &self.shader_lib;
        let cod2 = fs.game() == vcod_common::game::Game::Cod2;
        self.fog.radial = cod2;
        // The light grid's sight traces' world builds beside the props.
        let (mut props, light_clip) = std::thread::scope(|sc| {
            let clip = bsp
                .light_grid
                .is_some()
                .then(|| sc.spawn(|| vcod_common::collision::CollisionWorld::build(bsp, &[])));
            let props = props::build(fs, bsp, &mut lighting, &|skin| {
                let script = prop_material(lib, cod2, skin);
                props::SkinDraw {
                    sprite: script.and_then(|sh| sh.sprite),
                    reflects: match script.filter(|sh| !sh.stages.is_empty()) {
                        Some(sh) => sh.reflects(),
                        None => {
                            matches!(prop_skin_draw(fs, skin), PropSkinDraw::Stages(sh) if sh.reflects())
                        }
                    },
                    model_colour: cod2 && script.is_some_and(cod2_model_colour),
                    ambient: script.is_some_and(|m| m.model_light == ModelLight::Ambient),
                    smodel: cod2
                        && !bsp.lightmap_basis.is_empty()
                        && script.is_some_and(cod2_smodel),
                    skip: cod2 && script.is_some_and(|m| m.model_light == ModelLight::Objective),
                }
            });
            (props, clip.map(vcod_common::laps::joined))
        });
        laps.lap("props");
        self.dynamic.lighting = Some(lighting);
        self.dynamic.cod2_light =
            light_clip.and_then(|clip| crate::cod2_light::Cod2Lighting::with_clip(bsp, clip));
        self.dynamic.cod4_light = bsp
            .entity_lighting
            .clone()
            .zip(bsp.sun)
            .map(|(lighter, sun)| Cod4EntityLight {
                lighter,
                sun_dir: glam::Vec3::from(sun.dir),
                sun_colour: glam::Vec3::from(sun.colour),
                lights: bsp.model_lights.clone(),
            });
        // CoD4: the probes an entity's specular picks from (empty elsewhere)
        self.dynamic
            .materials
            .set_probes(&self.device, &self.queue, &bsp.reflection_probes);
        // `R_ResetShadowCookies` (Mac 0x000f99fc) at the map's load
        self.cookies.clock = Default::default();
        self.cookies.entities.clear();
        let smodel_cache = self
            .dynamic
            .cod2_light
            .as_ref()
            .and_then(|c2| c2.light_props(fs, &bsp.entities, &mut props));
        laps.lap("prop light");
        let prop_first_index = indices.len() as u32;
        let prop_first_vertex = bsp.verts.len() as u32;
        indices.extend(props.indices.iter().map(|i| i + prop_first_vertex));
        // prop batches follow the world's in `batch_draws`, prop indices follow
        // the world's in the merged buffer
        let prop_ranges: Vec<(u32, u8, IndexRange)> = props
            .ranges
            .iter()
            .map(|&(p, l, r)| {
                (
                    p,
                    l,
                    IndexRange {
                        batch: r.batch + batches.len() as u32,
                        first: r.first + prop_first_index,
                        count: r.count,
                    },
                )
            })
            .collect();
        let mut vertices = bsp.verts.clone();
        encode_world_sprites(
            bsp,
            &self.shader_lib,
            &soup_ranges,
            &mut vertices,
            &mut indices,
        );
        vertices.extend_from_slice(&props.verts);

        let vertex_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("map vertices"),
            contents: bytemuck::cast_slice(&vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let mut s_axes = dlight::soup_s_axes(&bsp.soups, &bsp.verts, &bsp.indices);
        s_axes.resize(vertices.len(), [0.0; 3]);
        let cod2_tangents = cod2.then(|| {
            let mut t = cod2::tangent_stream(bsp, vertices.len() - props.verts.len());
            t.extend_from_slice(&props.vert_tangent);
            t
        });
        let s_axis_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("map tangent s axes"),
            contents: bytemuck::cast_slice(&s_axes),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let index_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("map indices"),
            contents: bytemuck::cast_slice(&indices),
            usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
        });

        laps.lap("buffers");
        // CoD2's images, decoded on every core before the loops below take
        // them one by one in their own order.
        let prefetched = assets::DecodedImages::default();
        if cod2 {
            let mats = bsp
                .materials
                .iter()
                .filter_map(|m| self.shader_lib.get(&m.name))
                .chain(
                    props
                        .batches
                        .iter()
                        .filter_map(|b| prop_material(&self.shader_lib, cod2, &b.skin)),
                );
            prefetched.prefetch(fs, prefetch_paths(fs, mats));
        }
        laps.lap("prefetch");
        let fallback_px = fallback_pixels();
        let shaders = assets::load_shaders(fs);
        let mut material_views: HashMap<u16, wgpu::TextureView> = HashMap::new();
        // materials whose image has texels the 0.5 alpha test cuts
        let mut cut_materials: HashSet<u16> = HashSet::new();
        // CoD1 materials no level of whose image the cut drops a texel of
        let cod1 = fs.game() == vcod_common::game::Game::Cod1;
        let mut solid_materials: HashSet<u16> = HashSet::new();
        let (mut loaded, mut fallbacks) = (0usize, 0usize);
        let load_level = texture_settings::path_levels(fs);
        let lib = &self.shader_lib;
        let (textures, gamma) = (self.textures, self.image_gamma.as_ref());
        // A material that draws through its stages never binds its base
        // image (`Pass::Stage`): white, with the reads its load would mark.
        let mut seen = HashSet::new();
        let firsts: Vec<u16> = batches
            .iter()
            .map(|b| b.material)
            .filter(|&m| seen.insert(m))
            .collect();
        let (staged, based): (Vec<u16>, Vec<u16>) = firsts.into_iter().partition(|&m| {
            kinds[m as usize] == MaterialKind::Draw
                && lib
                    .get(&bsp.materials[m as usize].name)
                    .is_some_and(|sh| !sh.stages.is_empty())
        });
        for m in staged {
            let name = &bsp.materials[m as usize].name;
            if cod2 {
                mark_colour_map(fs, lib.get(name));
            } else {
                assets::mark_material_image(fs, &shaders, name);
            }
            material_views.insert(m, white_view.clone());
        }
        // decoded and mipped on every core a chunk at a time, uploaded in
        // first-use order
        for chunk in based.chunks(par_chunk()) {
            let prepared = vcod_common::laps::par_map(chunk, |&m| {
                let mat = &bsp.materials[m as usize];
                let name = &mat.name;
                let sh = lib.get(name);
                let mut img = if cod2 {
                    colour_map_image(fs, sh, &prefetched)
                } else {
                    assets::load_material_image(fs, &shaders, name)
                };
                let picmip = textures.world_picmip(sh.is_some_and(|sh| sh.nopicmip));
                let mips = if sh.is_some_and(|sh| sh.nomipmaps) {
                    texture_settings::Mips::single(name, &load_level)
                } else {
                    texture_settings::Mips::chain(name, &load_level)
                };
                let fallback = is_fallback(&img, &fallback_px);
                if sh.is_none() && !mesh::is_overlay(mat) && world_type_draws_opaque(fs, name) {
                    assets::force_opaque(&mut img);
                }
                let cut = assets::has_cutout_alpha(&img);
                let levels =
                    texture_settings::prepare(&mut img, picmip, gamma, textures.store(), mips);
                let solid = cod1 && !uploads_cutout(&img, &levels);
                (img, levels, fallback, cut, solid)
            });
            for (&m, (img, levels, fallback, cut, solid)) in chunk.iter().zip(prepared) {
                if fallback {
                    fallbacks += 1;
                } else {
                    loaded += 1;
                }
                if cut {
                    cut_materials.insert(m);
                }
                if solid {
                    solid_materials.insert(m);
                }
                let name = &bsp.materials[m as usize].name;
                material_views.insert(m, upload_levels(device, queue, name, &img, &levels));
            }
        }

        laps.lap("material images");
        // One RGBA lightmap page per BSP page, plus a white 1x1 for unlit soups.
        let block_edges = lightmap_blocks::edges(bsp);
        let pages_rgba = vcod_common::laps::par_map(&bsp.lightmaps, |page| {
            let mut rgba = Vec::with_capacity(page.len() / 3 * 4);
            for px in page.as_chunks::<3>().0 {
                rgba.extend_from_slice(&[px[0], px[1], px[2], 255]);
            }
            rgba
        });
        let lightmap_views: Vec<LightmapPage> = pages_rgba
            .iter()
            .zip(&block_edges)
            .enumerate()
            .map(|(i, (rgba, edges))| {
                let size = bsp::LIGHTMAP_SIZE as u32;
                LightmapPage {
                    view: upload_rgba(device, queue, &format!("lightmap {i}"), size, size, rgba),
                    edges: lightmap_blocks::upload_edges(
                        device,
                        queue,
                        &format!("lightmap {i} blocks"),
                        size,
                        edges,
                    ),
                }
            })
            .collect();

        laps.lap("lightmaps");
        self.marks.load(
            crate::marks::MarkWorld::build(bsp, |m| kinds[m as usize] == MaterialKind::Draw),
            lightmap_views.clone(),
        );
        self.coronas = bsp::coronas(&bsp.entities);
        if !self.coronas.is_empty() {
            log::info!("coronas: {}", self.coronas.len());
        }

        laps.lap("marks");
        let mut bind_groups: Vec<wgpu::BindGroup> = Vec::new();
        let mut cache: HashMap<(u16, u16), usize> = HashMap::new();
        // each batch's CoD4 primary light, the props' none, in
        // `batch_draws`' order
        let batch_lights: Vec<u8> = batches
            .iter()
            .map(|b| b.light)
            .chain(props.batches.iter().map(|_| 0))
            .collect();
        let mut batch_draws = Vec::with_capacity(batches.len());
        for Batch {
            material, lightmap, ..
        } in &batches
        {
            let idx = *cache.entry((*material, *lightmap)).or_insert_with(|| {
                let lm = match *lightmap {
                    bsp::NO_LIGHTMAP => white_lightmap,
                    i => lightmap_views.get(i as usize).unwrap_or(white_lightmap),
                };
                bind_groups.push(device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("material bind group"),
                    layout: material_layout,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: wgpu::BindingResource::TextureView(&material_views[material]),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::TextureView(&lm.view),
                        },
                        wgpu::BindGroupEntry {
                            binding: 3,
                            resource: wgpu::BindingResource::Sampler(lightmap_sampler),
                        },
                        wgpu::BindGroupEntry {
                            binding: 4,
                            resource: wgpu::BindingResource::TextureView(&lm.edges),
                        },
                    ],
                }));
                bind_groups.len() - 1
            });
            let mat = &bsp.materials[*material as usize];
            let pass = if mesh::is_overlay(mat) {
                Pass::Overlay
            } else if shaders.uses_polygon_offset(&mat.name) {
                Pass::Layer
            } else if solid_materials.contains(material) {
                Pass::Solid
            } else {
                Pass::Opaque
            };
            batch_draws.push(DrawCall {
                bind_group: idx,
                pass,
            });
        }

        laps.lap("bind groups");
        // Prop materials are skin filenames, not `textures/...` names, so
        // `load_skin_image` resolves them.
        let skin_level = texture_settings::skin_levels(fs);
        let mut prop_cut: Vec<bool> = Vec::with_capacity(props.batches.len());
        // `type@` templates past one unblended stage (metal_env, the
        // blends), bound to their skin; they draw through the stage path
        let mut prop_templates: Vec<Material> = Vec::new();
        let mut prop_template_of: Vec<Option<usize>> = Vec::with_capacity(props.batches.len());
        // each skin once, in first-use order: its script, its template and
        // whether it draws through stages, which bind no base image
        struct SkinPlan<'s> {
            skin: &'s str,
            scripted: bool,
            template: Option<usize>,
            opaque: bool,
            staged: bool,
        }
        let mut plan_of: HashMap<&str, usize> = HashMap::new();
        let mut plans: Vec<SkinPlan> = Vec::new();
        for batch in &props.batches {
            let skin = batch.skin.as_str();
            if plan_of.contains_key(skin) {
                continue;
            }
            let scripted =
                prop_material(lib, cod2, skin).is_some_and(|sh| !sh.stages.is_empty());
            let (mut template, mut opaque) = (None, false);
            if !scripted {
                match prop_skin_draw(fs, skin) {
                    PropSkinDraw::Stages(sh) => {
                        prop_templates.push(*sh);
                        template = Some(prop_templates.len() - 1);
                    }
                    PropSkinDraw::Opaque => opaque = true,
                    PropSkinDraw::Cut => {}
                }
            }
            let staged =
                scripted || template.is_some_and(|i| !prop_templates[i].stages.is_empty());
            plan_of.insert(skin, plans.len());
            plans.push(SkinPlan {
                skin,
                scripted,
                template,
                opaque,
                staged,
            });
        }
        let skin_bind_group = |view: &wgpu::TextureView| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("prop skin bind group"),
                layout: material_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(white_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: wgpu::BindingResource::Sampler(lightmap_sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 4,
                        resource: wgpu::BindingResource::TextureView(&white_lightmap.edges),
                    },
                ],
            })
        };
        // per plan: its bind group, cut and pass (`Pass::Stage` replaces a
        // staged one's below)
        let mut skin_draws: Vec<(usize, bool, Pass)> = Vec::with_capacity(plans.len());
        for chunk in plans.chunks(par_chunk()) {
            let prepared = vcod_common::laps::par_map(chunk, |p| {
                if p.staged {
                    if cod2 {
                        mark_colour_map(fs, prop_material(lib, cod2, p.skin));
                    } else {
                        assets::mark_skin_image(fs, p.skin);
                    }
                    return None;
                }
                let mut img = if cod2 {
                    colour_map_image(fs, prop_material(lib, cod2, p.skin), &prefetched)
                } else {
                    assets::load_skin_image(fs, p.skin)
                };
                if p.opaque {
                    assets::force_opaque(&mut img);
                }
                let cut = assets::has_cutout_alpha(&img);
                let levels = texture_settings::prepare(
                    &mut img,
                    textures.picmip2,
                    gamma,
                    textures.store(),
                    texture_settings::Mips::chain(p.skin, &skin_level),
                );
                let pass = Pass::Prop {
                    culled: cod1 && !p.scripted && !prop_skin_two_sided(fs, p.skin),
                    solid: cod1 && !uploads_cutout(&img, &levels),
                };
                Some((img, levels, cut, pass))
            });
            for (p, prepared) in chunk.iter().zip(prepared) {
                let (view, cut, pass) = match prepared {
                    Some((img, levels, cut, pass)) => {
                        (upload_levels(device, queue, p.skin, &img, &levels), cut, pass)
                    }
                    None => {
                        let pass = Pass::Prop {
                            culled: false,
                            solid: false,
                        };
                        (white_view.clone(), false, pass)
                    }
                };
                bind_groups.push(skin_bind_group(&view));
                skin_draws.push((bind_groups.len() - 1, cut, pass));
            }
        }
        for batch in &props.batches {
            let pi = plan_of[batch.skin.as_str()];
            let (idx, cut, pass) = skin_draws[pi];
            prop_cut.push(cut);
            prop_template_of.push(plans[pi].template);
            batch_draws.push(DrawCall {
                bind_group: idx,
                pass,
            });
        }

        laps.lap("prop skins");
        let count = |p: fn(Pass) -> bool| batch_draws.iter().filter(|d| p(d.pass)).count();
        log::info!(
            "{} batches ({} opaque, {} prop, {} layer, {} overlay), {} draw indices, {} vertices",
            batch_draws.len(),
            count(|p| matches!(p, Pass::Opaque | Pass::Solid)),
            count(|p| matches!(p, Pass::Prop { .. })),
            count(|p| p == Pass::Layer),
            count(|p| p == Pass::Overlay),
            indices.len(),
            vertices.len()
        );
        log::info!(
            "{} textures loaded, {} missing, {} lightmap pages",
            loaded,
            fallbacks,
            lightmap_views.len()
        );

        // One dynamic-offset slot per authored stage of every map material
        // that resolves in the shader lib; batches of one material share the
        // slot per stage. Implicit materials use no slots.
        let mut stage_slots: Vec<(&Material, usize)> = Vec::new();
        let mut slot_of: HashMap<(u16, usize), u32> = HashMap::new();
        for (mi, mat) in bsp.materials.iter().enumerate() {
            if let Some(sh) = self.shader_lib.get(&mat.name) {
                for idx in 0..sh.stages.len() {
                    slot_of.insert((mi as u16, idx), stage_slots.len() as u32);
                    stage_slots.push((sh, idx));
                }
            }
        }
        // A prop skin with a `skins/<name>` block, or a template the prop
        // path cannot draw, goes through its stages like a map material,
        // slotted after them (shader doc 8.2).
        // A lit CoD2 prop's stages read its baked light as `lightingDiffuse`,
        // rescaled from identityLight 0.5 as `vs_prop` does, never raw.
        let cod2_lit: HashMap<&str, Material> = props
            .batches
            .iter()
            .filter(|_| cod2)
            .filter_map(|b| {
                let m = self.shader_lib.get(&b.skin)?;
                let lit = matches!(m.model_light, ModelLight::Probe | ModelLight::Ambient);
                (lit && !m.stages.is_empty())
                    .then(|| (b.skin.as_str(), crate::entity_material::lit_stages(m)))
            })
            .collect();
        let prop_script = |b: &props::Batch, template: Option<usize>| {
            cod2_lit.get(b.skin.as_str()).or_else(|| {
                prop_material(&self.shader_lib, cod2, &b.skin)
                    .filter(|sh| !sh.stages.is_empty())
                    .or_else(|| template.map(|i| &prop_templates[i]))
            })
        };
        // a reflecting placement's own copy, in its model's frame
        let prop_framed: Vec<Option<Material>> = props
            .batches
            .iter()
            .zip(&prop_template_of)
            .map(|(b, &template)| {
                let axes = b.model_axes?;
                let mut sh = prop_script(b, template)?.clone();
                sh.model_axes = Some(axes);
                Some(sh)
            })
            .collect();
        let prop_scripts: Vec<Option<(&Material, u32)>> = props
            .batches
            .iter()
            .zip(&prop_template_of)
            .zip(&prop_framed)
            .map(|((b, &template), framed)| {
                let sh = framed.as_ref().or_else(|| prop_script(b, template))?;
                let first = stage_slots.len() as u32;
                stage_slots.extend((0..sh.stages.len()).map(|idx| (sh, idx)));
                (!sh.stages.is_empty()).then_some((sh, first))
            })
            .collect();
        // The dlight pass's stages (shader doc 3), slotted after the base
        // stages: per scripted material its own as single-stage blocks that
        // keep the material's deform and culling, then the implicit
        // surfaces' copy (texture times the dlight image, rgbGen identity),
        // the same alpha-tested for masked overlays, and `dlightshader`.
        let blob = self.shader_lib.get("dlightshader");
        let mut dl_shaders: Vec<Material> = Vec::new();
        let mut dl_of_material: HashMap<u16, Vec<u32>> = HashMap::new();
        for (mi, mat) in bsp.materials.iter().enumerate() {
            let Some(sh) = self.shader_lib.get(&mat.name) else {
                continue;
            };
            if sh.stages.is_empty() {
                continue;
            }
            for syn in dlight::stage_shaders(sh, blob) {
                dl_shaders.push(syn);
                let i = dl_shaders.len() as u32 - 1;
                dl_of_material.entry(mi as u16).or_default().push(i);
            }
        }
        let implicit_copy = |alpha_func| Stage {
            bundles: [ImageRef::White, ImageRef::Path(DLIGHT_IMAGE.into())]
                .map(|image| Bundle {
                    image,
                    anim: None,
                    clamp: Clamp::NONE,
                    tcmods: Vec::new(),
                    tc_gen: TcGen::Base,
                    env: TexEnv::Modulate,
                })
                .into(),
            blend: Some((BlendFactor::One, BlendFactor::One)),
            depth_write: None,
            alpha_func,
            depth_func: DepthFunc::LessEqual,
            rgb_gen: RgbGen::Identity,
            alpha_gen: AlphaGen::Identity,
        };
        // the copies fog as the implicit surface does; the blob as
        // `dlightshader` (`nofog`)
        let implicit = |st: Stage, nofog: bool| {
            Material::from_shader(&Shader {
                stages: vec![st],
                nofog,
                ..Shader::default()
            })
        };
        let dl_implicit = [
            Some(implicit(implicit_copy(None), false)),
            Some(implicit(implicit_copy(Some(AlphaFunc::Ge128)), false)),
            blob.and_then(|b| Some(implicit(b.stages.first()?.clone(), b.nofog))),
        ]
        .map(|syn| {
            syn.map(|syn| {
                dl_shaders.push(syn);
                dl_shaders.len() as u32 - 1
            })
        });
        let dl_first_slot = stage_slots.len() as u32;

        let align = device.limits().min_uniform_buffer_offset_alignment as u64;
        let stage_params_stride = STAGE_PARAMS_SIZE.div_ceil(align) * align;
        let stage_params_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("stage params"),
            size: ((stage_slots.len() + dl_shaders.len()) as u64 * stage_params_stride)
                .max(stage_params_stride),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        // Provisional fill at t = 0 so every slot starts valid (static stages
        // keep these values; animated ones are rewritten each frame).
        let all_slots = stage_slots
            .iter()
            .copied()
            .chain(dl_shaders.iter().map(|sh| (sh, 0)));
        for (i, (sh, idx)) in all_slots.enumerate() {
            if let Some(p) = stage_params(sh, idx, 0.0) {
                queue.write_buffer(
                    &stage_params_buf,
                    i as u64 * stage_params_stride,
                    bytemuck::bytes_of(&p),
                );
            }
        }
        log::info!(
            "stage params: {} slots x {STAGE_PARAMS_SIZE} B (stride {stage_params_stride})",
            stage_slots.len()
        );

        laps.lap("stage slots");
        // Expand authored materials: one StageBatch per parsed stage sharing
        // the parent batch's index range. Implicit materials and blocks with
        // no effective stages keep the legacy fast path.
        let mut stage_batches: Vec<StageBatch> = Vec::new();
        let mut stages_of_batch: Vec<Vec<u32>> = vec![Vec::new(); batch_draws.len()];
        // per batch, its first depth-writing stage batch and whether it cuts
        let mut first_depth: Vec<Option<(u32, bool)>> = vec![None; batch_draws.len()];
        let mut animated: Vec<(Material, usize, u32)> = Vec::new();
        let mut bundle_views = BundleViews {
            views: HashMap::new(),
            table: self.image_gamma.as_ref(),
            picmip: 0,
            mipmap: true,
            store: self.textures.store(),
            prefetched: Some(&prefetched),
        };
        // once per material, not per batch: a material can hold several batches
        let dropped_stages: usize = bsp
            .materials
            .iter()
            .zip(&kinds)
            .filter(|(_, k)| **k == MaterialKind::Draw)
            .filter_map(|(m, _)| self.shader_lib.get(&m.name))
            .map(|sh| sh.dropped_stages)
            .sum();
        // CoD2's lit draws, over the map's basis pages
        let mut lit = (cod2 && !bsp.lightmap_basis.is_empty()).then(|| {
            let mut b = cod2::Cod2Builder::with_shared(
                device,
                queue,
                self.world_format,
                &self.camera_layout,
                bsp,
                std::mem::take(&mut self.cod2_lit),
            );
            if let Some(c) = &smodel_cache {
                b.set_smodel_cache(c);
            }
            b.set_picmip(self.textures.picmip_bump, self.textures.picmip_spec);
            b
        });
        for (bi, batch) in batches.iter().enumerate() {
            if kinds[batch.material as usize] != MaterialKind::Draw {
                continue;
            }
            let name = &bsp.materials[batch.material as usize].name;
            let Some(sh) = self.shader_lib.get(name) else {
                continue;
            };
            if sh.stages.is_empty() {
                continue;
            }
            bundle_views.picmip = self.textures.world_picmip(sh.nopicmip);
            bundle_views.mipmap = !sh.nomipmaps;
            let lm_view = match batch.lightmap {
                bsp::NO_LIGHTMAP => white_lightmap,
                i => lightmap_views.get(i as usize).unwrap_or(white_lightmap),
            };
            for si in 0..sh.stages.len() {
                let Some((variant, two_sided, bias)) = stage_draw_facts(sh, si) else {
                    continue;
                };
                let slot = slot_of[&(batch.material, si)];
                let (mat, stage) = stage_bind_groups(
                    device,
                    queue,
                    fs,
                    material_layout,
                    &self.stage_layout,
                    &stage_params_buf,
                    STAGE_PARAMS_SIZE,
                    white_view,
                    lm_view,
                    lightmap_sampler,
                    &mut bundle_views,
                    &sh.stages[si],
                );
                if stage_animated(sh, si) {
                    animated.push((sh.clone(), si, slot));
                }
                stages_of_batch[bi].push(stage_batches.len() as u32);
                note_first_depth(&mut first_depth[bi], sh, si, stage_batches.len());
                let lit_bind = lit.as_mut().and_then(|b| {
                    if cod2_water::draws_water(sh, variant) {
                        return Some(b.bind_water(fs, sh, variant, two_sided, bias));
                    }
                    if cod2::draws_cod4_water(sh, variant) {
                        let probe = batch.probe;
                        return Some(b.bind_cod4_water(
                            fs,
                            &mut bundle_views,
                            sh,
                            probe,
                            variant,
                            two_sided,
                            bias,
                        ));
                    }
                    // a CoD4 model set off the model lighting volume
                    if cod2::draws_cod4_model(sh, variant) {
                        return b.has_model_volume().then(|| {
                            b.bind_cod4_model(
                                fs,
                                &mut bundle_views,
                                sh,
                                (batch.probe, batch.light),
                                variant,
                                two_sided,
                                bias,
                            )
                        });
                    }
                    cod2::draws_lit(sh, batch.lightmap, b.page_count(), variant).then(|| {
                        b.bind_in(
                            fs,
                            &mut bundle_views,
                            sh,
                            (batch.lightmap, batch.probe, batch.light),
                            variant,
                            two_sided,
                            bias,
                        )
                    })
                });
                stage_batches.push(StageBatch {
                    slot,
                    variant,
                    two_sided,
                    bias,
                    mat,
                    stage,
                    cod2: lit_bind,
                    fx: cod4_worldfx::world_fx(sh, variant),
                });
            }
        }
        // Scripted prop skins keep the prop path's two-sidedness (xmodel
        // winding) and take no lightmap.
        for (pi, script) in prop_scripts.iter().enumerate() {
            let Some((sh, first)) = *script else { continue };
            bundle_views.picmip = self.textures.model_picmip(sh.nopicmip);
            bundle_views.mipmap = !sh.nomipmaps;
            for si in 0..sh.stages.len() {
                let Some((variant, _, bias)) = stage_draw_facts(sh, si) else {
                    continue;
                };
                let slot = first + si as u32;
                let (mat, stage) = stage_bind_groups(
                    device,
                    queue,
                    fs,
                    material_layout,
                    &self.stage_layout,
                    &stage_params_buf,
                    STAGE_PARAMS_SIZE,
                    white_view,
                    white_lightmap,
                    lightmap_sampler,
                    &mut bundle_views,
                    &sh.stages[si],
                );
                if stage_animated(sh, si) {
                    animated.push((sh.clone(), si, slot));
                }
                stages_of_batch[batches.len() + pi].push(stage_batches.len() as u32);
                note_first_depth(
                    &mut first_depth[batches.len() + pi],
                    sh,
                    si,
                    stage_batches.len(),
                );
                // a per pixel static model (`SkinDraw::smodel`) through its
                // `lprobe_smc*` technique
                let smodel = smodel_cache.as_ref().and_then(|_| {
                    let m = self.shader_lib.get(&props.batches[pi].skin)?;
                    let b = lit.as_mut()?;
                    cod2_smodel(m).then(|| b.bind_smodel(fs, &mut bundle_views, m, variant, bias))
                });
                stage_batches.push(StageBatch {
                    slot,
                    variant,
                    two_sided: true,
                    bias,
                    mat,
                    stage,
                    cod2: smodel,
                    fx: cod4_worldfx::world_fx(sh, variant),
                });
            }
        }
        for (bi, sbs) in stages_of_batch.iter().enumerate() {
            if !sbs.is_empty() {
                batch_draws[bi].pass = Pass::Stage;
            }
        }

        // The dlight pass per world batch: a scripted material's stages
        // through their own bind groups, an implicit one through its base
        // bind group and a neutral bundle 1.
        laps.lap("stage images");
        let implicit_stage_bg =
            AnimFrames::static_(device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("implicit dlight stage bind group"),
                layout: &self.stage_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                            buffer: &stage_params_buf,
                            offset: 0,
                            size: wgpu::BufferSize::new(STAGE_PARAMS_SIZE),
                        }),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(white_view),
                    },
                ],
            }));
        let mut dlight_batches: Vec<DlightBatch> = Vec::new();
        let mut dlights_of_batch: Vec<Vec<u32>> = vec![Vec::new(); batch_draws.len()];
        for (bi, batch) in batches.iter().enumerate() {
            if kinds[batch.material as usize] != MaterialKind::Draw {
                continue;
            }
            let sh = self
                .shader_lib
                .get(&bsp.materials[batch.material as usize].name);
            if batch_draws[bi].pass == Pass::Stage {
                let Some(list) = dl_of_material.get(&batch.material) else {
                    continue;
                };
                let lm_view = match batch.lightmap {
                    bsp::NO_LIGHTMAP => white_lightmap,
                    i => lightmap_views.get(i as usize).unwrap_or(white_lightmap),
                };
                for &di in list {
                    let syn = &dl_shaders[di as usize];
                    bundle_views.picmip = self.textures.world_picmip(syn.nopicmip);
                    bundle_views.mipmap = !syn.nomipmaps;
                    let st = &syn.stages[0];
                    let (Some(blend), Some((_, two_sided, bias))) =
                        (DlightBlend::of(st), stage_draw_facts(syn, 0))
                    else {
                        log::warn!("{}: dlight stage blend {:?} not drawn", syn.name, st.blend);
                        continue;
                    };
                    let slot = dl_first_slot + di;
                    let (mat, stage) = stage_bind_groups(
                        device,
                        queue,
                        fs,
                        material_layout,
                        &self.stage_layout,
                        &stage_params_buf,
                        STAGE_PARAMS_SIZE,
                        white_view,
                        lm_view,
                        lightmap_sampler,
                        &mut bundle_views,
                        st,
                    );
                    if stage_animated(syn, 0) {
                        animated.push((syn.clone(), 0, slot));
                    }
                    dlights_of_batch[bi].push(dlight_batches.len() as u32);
                    dlight_batches.push(DlightBatch {
                        blend,
                        two_sided,
                        bias,
                        slot,
                        mat,
                        stage,
                    });
                }
                continue;
            }
            // retail's implicit shader multiplies the texture by the
            // lightmap (0x4fc440), which the copy takes; a vertex-lit one
            // has no lightmap bundle and gets `dlightshader`
            if sh.is_some_and(|sh| sh.dlight.is_none()) {
                continue;
            }
            let call = &batch_draws[bi];
            let which = match (call.pass, batch.lightmap) {
                (Pass::Opaque | Pass::Solid | Pass::Overlay, bsp::NO_LIGHTMAP) => 2,
                (Pass::Opaque | Pass::Solid, _) => 0,
                (Pass::Overlay, _) => 1,
                _ => continue,
            };
            let Some(di) = dl_implicit[which] else {
                continue;
            };
            let blend = DlightBlend::of(&dl_shaders[di as usize].stages[0])
                .expect("the implicit dlight stages blend onto GL_ONE");
            dlights_of_batch[bi].push(dlight_batches.len() as u32);
            dlight_batches.push(DlightBatch {
                blend,
                two_sided: false,
                bias: call.pass == Pass::Overlay,
                slot: dl_first_slot + di,
                mat: AnimFrames::static_(bind_groups[call.bind_group].clone()),
                stage: AnimFrames::static_(implicit_stage_bg.groups[0].clone()),
            });
        }
        log::info!(
            "dlight pass: {} stage draws over {} batches",
            dlight_batches.len(),
            dlights_of_batch.iter().filter(|d| !d.is_empty()).count()
        );
        self.shader_dropped = dropped_stages;
        log::info!(
            "shader scripts: {} stage batches, {} dropped stages, {} lib warnings",
            stage_batches.len(),
            dropped_stages,
            self.shader_lib.warn_count()
        );

        // ---- sky pass: farbox now, cloud dome through the stage path ----
        self.sky = None;
        laps.lap("dlight stages");
        let mut sky_stages: Vec<(Material, usize, u32)> = Vec::new();
        if let Some((name, mat_idx)) = active_sky {
            let sh = self
                .shader_lib
                .get(&name)
                .expect("sky candidate comes from the lib");
            let parms = sh.sky.as_ref().expect("sky_blocks only yields skyparms");
            bundle_views.picmip = self.textures.world_picmip(sh.nopicmip);
            bundle_views.mipmap = !sh.nomipmaps;
            log::info!(
                "sky: {name}  env {}  cloud height {}  stages {}",
                parms.env,
                parms.cloud_height,
                sh.stages.len()
            );
            // no box when outerbox[0] fails to load (`SkyBox`)
            let mut farbox = None;
            if cod2 {
                farbox = cod2::build_sky_cube(
                    device,
                    queue,
                    self.world_format,
                    fs,
                    &self.camera_layout,
                    &parms.env,
                );
            } else if parms.env.is_empty() {
                log::info!("sky {name}: skyParms '-' outerbox, drawing no farbox");
            } else {
                let rt = format!("{}_rt", parms.env);
                if assets::resolve_bundle_image(fs, &rt).is_some() {
                    // skyParms loads its sides with flags 0x33 whatever the
                    // block says: mipmapped, `r_picmip`, clamped (0x4fa3cd)
                    bundle_views.picmip = self.textures.world_picmip(false);
                    bundle_views.mipmap = true;
                    farbox = Some(build_sky_box(
                        device,
                        queue,
                        self.world_format,
                        fs,
                        &self.camera_layout,
                        &mut bundle_views,
                        white_view,
                        &parms.env,
                        &name,
                    ));
                } else {
                    log::warn!("sky {name}: env image {rt} not found, drawing no farbox");
                }
                bundle_views.picmip = self.textures.world_picmip(sh.nopicmip);
                bundle_views.mipmap = !sh.nomipmaps;
            }
            let mut dome = None;
            if !sh.stages.is_empty() {
                let size = sky::box_size(Z_FAR, Z_NEAR);
                let (verts, idx) = sky::dome_mesh(size, parms.cloud_height);
                let vertex_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("sky dome vertices"),
                    contents: bytemuck::cast_slice(&verts),
                    usage: wgpu::BufferUsages::VERTEX,
                });
                let index_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("sky dome indices"),
                    contents: bytemuck::cast_slice(&idx),
                    usage: wgpu::BufferUsages::INDEX,
                });
                let mut dome_stages = Vec::new();
                for si in 0..sh.stages.len() {
                    if sh.stages[si].bundles.is_empty() {
                        continue;
                    }
                    let Some((variant, two_sided, _)) = stage_draw_facts(sh, si) else {
                        continue;
                    };
                    // a sky stage may carry explicit depthWrite; the dome must
                    // still never write depth, it draws first at far depth
                    let variant = match variant {
                        StageVariant::BlendDepthWrite => StageVariant::Blend,
                        StageVariant::AdditiveFixedDepthWrite => StageVariant::AdditiveFixed,
                        StageVariant::AdditiveFadedDepthWrite => StageVariant::AdditiveFaded,
                        other => other,
                    };
                    // the block's slots were allocated with every other material's
                    let slot = slot_of[&(mat_idx, si)];
                    let (mat, stage) = stage_bind_groups(
                        device,
                        queue,
                        fs,
                        material_layout,
                        &self.stage_layout,
                        &stage_params_buf,
                        STAGE_PARAMS_SIZE,
                        white_view,
                        white_lightmap,
                        lightmap_sampler,
                        &mut bundle_views,
                        &sh.stages[si],
                    );
                    // dome params ride the per-frame rewrite with the eye offset
                    sky_stages.push((sh.clone(), si, slot));
                    dome_stages.push(DomeStage {
                        variant,
                        two_sided,
                        slot,
                        mat,
                        stage,
                    });
                }
                if !dome_stages.is_empty() {
                    log::info!(
                        "cloud dome: {} stage(s), {} verts",
                        dome_stages.len(),
                        verts.len()
                    );
                    dome = Some(DomeLayer {
                        vertex_buf,
                        index_buf,
                        index_count: idx.len() as u32,
                        stages: dome_stages,
                    });
                }
            }
            // The disc and the flare's angle test follow `sundirection`, the
            // flare sits on the lump-30 light's own direction (0x4d9250).
            let mut sun = None;
            let mut flare = None;
            if let Some(vcod_common::sun::MapSun {
                file: sf,
                light_dir,
                dir,
            }) = &vcod_common::sun::load(fs, sh, bsp)
            {
                let (light_dir, dir) = (*light_dir, *dir);
                if let Some(sprite) = &sf.sprite {
                    log::info!("sky {name}: sun sprite {sprite} size {}", sf.sprite_size);
                    // a shader of its own (0x4e7930); stock ones say
                    // `nomipmaps` and `nopicmip`
                    let sh = self.shader_lib.get(sprite);
                    bundle_views.picmip =
                        self.textures.world_picmip(sh.is_some_and(|sh| sh.nopicmip));
                    bundle_views.mipmap = !sh.is_some_and(|sh| sh.nomipmaps);
                    sun = build_sun_sprite(
                        device,
                        queue,
                        self.world_format,
                        fs,
                        &self.camera_layout,
                        &mut bundle_views,
                        sprite,
                        dir,
                        sf.sprite_size,
                    );
                }
                if let Some(f) = &sf.flare {
                    let sh = self.shader_lib.get(f);
                    bundle_views.picmip =
                        self.textures.world_picmip(sh.is_some_and(|sh| sh.nopicmip));
                    bundle_views.mipmap = !sh.is_some_and(|sh| sh.nomipmaps);
                }
                // the flare's image: its block's first stage, or the name
                let image = sf.flare.as_ref().and_then(|f| {
                    let path = self.shader_lib.image(f).unwrap_or(f).to_string();
                    assets::resolve_bundle_image(fs, &path).map(|_| path)
                });
                match image {
                    Some(path) => {
                        log::info!("sky {name}: sun flare {path}");
                        let view = upload_bundle_view(device, queue, fs, &mut bundle_views, &path);
                        flare = Some(SunFlareGpu::new(
                            device,
                            self.world_format,
                            FlareParams::of(sf),
                            light_dir,
                            dir,
                            &view,
                            &self.tex_samplers,
                        ));
                    }
                    None => log::info!("sky {name}: no sun flare image, drawing no flare"),
                }
            }
            self.sky = Some(SkyRender {
                name,
                farbox,
                dome,
                sun,
                flare,
            });
        }

        // Retail's sorted shader list over the map's shaders (shader doc 5):
        // one shader per (material, lightmap), registered in batch order,
        // which follows the shader lump's first-use order; images number
        // as they first load, white and the lightmap pages before the
        // textures. vcod's own: props and the implicit overlays keep their
        // slot after every other opaque.
        laps.lap("sky and sun");
        let batch_keys: Vec<(f32, u32)> = {
            let mut images = ImageNumbers::default();
            images.number("$white");
            let lightmap = |i: u16| format!("$lightmap{i}");
            for i in 0..lightmap_views.len() {
                images.number(&lightmap(i as u16));
            }
            let mut entries: Vec<SortEntry> = Vec::with_capacity(batch_draws.len());
            for (bi, batch) in batches.iter().enumerate() {
                let mat = &bsp.materials[batch.material as usize];
                let sh = self.shader_lib.get(&mat.name);
                let mut image = |r: &ImageRef| match r {
                    ImageRef::Path(p) => images.number(p),
                    ImageRef::Lightmap if batch.lightmap != bsp::NO_LIGHTMAP => {
                        images.number(&lightmap(batch.lightmap))
                    }
                    ImageRef::Lightmap | ImageRef::White => images.number("$white"),
                };
                entries.push(match (batch_draws[bi].pass, sh) {
                    (Pass::Overlay, _) => SortEntry {
                        sort: SORT_OPAQUE,
                        images: Some([u32::MAX, 0]),
                    },
                    (Pass::Stage, Some(sh)) => staged_sort_entry(sh, &mut image),
                    // implicit: the texture, then the lightmap in bundle 1
                    (_, sh) => {
                        let tex = images.number(&mat.name);
                        let lm = match batch.lightmap {
                            bsp::NO_LIGHTMAP => 0,
                            i => images.number(&lightmap(i)),
                        };
                        SortEntry {
                            sort: sh.map_or(SORT_OPAQUE, |sh| sh.sort),
                            images: Some([tex, lm]),
                        }
                    }
                });
            }
            for script in &prop_scripts {
                entries.push(match script {
                    Some((sh, _)) => staged_sort_entry(sh, &mut |r: &ImageRef| match r {
                        ImageRef::Path(p) => images.number(p),
                        ImageRef::Lightmap | ImageRef::White => images.number("$white"),
                    }),
                    None => SortEntry {
                        sort: SORT_OPAQUE,
                        images: Some([u32::MAX - 1, 0]),
                    },
                });
            }
            // `VCOD_C4_DRAW_ORDER=0` keeps the CoD1 sorted shader list's
            // order, to measure against
            let old_order = std::env::var_os("VCOD_C4_DRAW_ORDER").is_some_and(|v| v == "0");
            let places = match bsp.soup_order.is_empty() || old_order {
                true => sorted_places(&entries),
                false => cod4_places(&entries, &soup_ranges, &bsp.soup_order),
            };
            entries
                .iter()
                .zip(places)
                .map(|(e, p)| (e.sort, p))
                .collect()
        };

        let shadow_cast: Vec<ShadowCast> = batch_draws
            .iter()
            .enumerate()
            .map(|(bi, call)| {
                if batch_keys.get(bi).is_some_and(|k| k.0 > SORT_OPAQUE) {
                    return ShadowCast::None;
                }
                let world = batches.get(bi).map(|b| b.material);
                // water lets the sun through; mp_ship's ocean also waves
                // away from the pose the static layer would hold
                if world.is_some_and(|m| {
                    self.shader_lib
                        .get(&bsp.materials[m as usize].name)
                        .is_some_and(|sh| sh.surface.water)
                }) {
                    return ShadowCast::None;
                }
                let image_cut = match world {
                    Some(m) => cut_materials.contains(&m),
                    None => prop_cut[bi - batches.len()],
                };
                let script = match world {
                    Some(m) => self.shader_lib.get(&bsp.materials[m as usize].name),
                    None => prop_scripts[bi - batches.len()].map(|(sh, _)| sh),
                };
                let deformed = script.is_some_and(|sh| sh.deforms.iter().any(Deform::moves));
                let solid_or_cut = |cut| {
                    if cut {
                        ShadowCast::Cut
                    } else {
                        ShadowCast::Solid
                    }
                };
                match call.pass {
                    Pass::Layer => ShadowCast::None,
                    // decals lie on other surfaces; masked overlays are cutouts
                    Pass::Overlay
                        if world.is_some_and(|m| !mesh::is_masked(&bsp.materials[m as usize])) =>
                    {
                        ShadowCast::None
                    }
                    Pass::Overlay | Pass::Opaque | Pass::Solid | Pass::Prop { .. } => {
                        solid_or_cut(image_cut)
                    }
                    Pass::Stage => match first_depth[bi] {
                        None => ShadowCast::None,
                        Some((sb, _)) if deformed => ShadowCast::Deformed(sb),
                        Some((sb, true)) => ShadowCast::Stage(sb),
                        Some((_, false)) => ShadowCast::Solid,
                    },
                }
            })
            .collect();
        let cookie_receive: Vec<bool> = (0..batch_draws.len())
            .map(|bi| {
                batches.get(bi).is_some_and(|b| {
                    kinds[b.material as usize] == MaterialKind::Draw
                        && self
                            .shader_lib
                            .get(&bsp.materials[b.material as usize].name)
                            .is_some_and(|sh| sh.cookie.receiver)
                })
            })
            .collect();
        laps.lap("sort and shadows");
        let cod2_world = lit.zip(cod2_tangents.as_deref()).map(|(b, t)| {
            let (world, shared) = b.finish_keeping(t);
            self.cod2_lit = shared;
            world
        });
        // the feathered and distortion stages' pipelines, when the map has any
        let fx_keys: Vec<_> = stage_batches
            .iter()
            .filter(|sb| sb.fx != cod4_worldfx::WorldFx::None)
            .map(|sb| (sb.fx, sb.variant, sb.two_sided, sb.bias))
            .collect();
        self.world_fx = (!fx_keys.is_empty()).then(|| {
            cod4_worldfx::WorldFxPass::new(
                device,
                self.world_format,
                [
                    &self.camera_layout,
                    material_layout,
                    &self.stage_layout,
                    &self.scene_copy.world_layout,
                ],
                depth_bias(PolygonOffset::Single),
                fx_keys,
            )
        });
        let sun_casters = self.sun_shadow.casters(device, bsp, &soup_model, |p| {
            upload_bundle_view(device, queue, fs, &mut bundle_views, p)
        });
        let cookie_bounds = soup_ranges
            .iter()
            .enumerate()
            .filter_map(|(si, r)| {
                let r = (*r)?;
                cookie_receive
                    .get(r.batch as usize)
                    .is_some_and(|&on| on)
                    .then(|| (r.first, vcod_common::vis::soup_bounds(bsp, si)))
            })
            .collect();
        // A material over several lightmaps or lights pushed its slot once
        // per batch; the frame writes each slot once, with the last push's
        // params as the repeated writes left it.
        let mut written = HashSet::new();
        animated.reverse();
        animated.retain(|(_, _, slot)| written.insert(*slot));
        animated.reverse();
        self.world = Some(WorldGpu {
            sun_casters,
            vertex_buf,
            cod2: cod2_world,
            index_buf,
            bind_groups,
            cpu_indices: indices,
            soup_ranges,
            prop_ranges,
            prop_bounds: props.bounds,
            prop_lods: props.lods,
            prop_lod: Vec::new(),
            batch_draws,
            batch_lights,
            draws: Vec::new(),
            stage_batches,
            stages_of_batch,
            animated,
            sky_stages,
            batch_keys,
            shadow_cast,
            cookie_bounds,
            cookie_runs: Vec::new(),
            cookie_receive,
            sun_keep: SunKeepState::Samples(
                vcod_common::static_light::sun_keep_samples(bsp, 16_384),
                vcod_common::static_light::world_reach(bsp),
            ),
            frame_draws: Vec::new(),
            stage_draws_last: 0,
            stage_params_buf,
            stage_params_stride,
            vis: WorldVis::build(bsp),
            gathered: Vec::new(),
            gathered_from: Vec::new(),
            world_gathered: 0,
            gather_scratch: Vec::new(),
            locked: None,
            culled: None,
            soup_model,
            static_lods: bsp.static_lods.clone(),
            coarse_lod: {
                let mut c = vec![false; bsp.soups.len()];
                for &(_, first, n) in bsp.static_lods.iter().flat_map(|l| l.lods.iter().skip(1)) {
                    c[first as usize..(first + n) as usize].fill(true);
                }
                c
            },
            sky_soups: bsp
                .models
                .first()
                .map_or(0..0, |m| {
                    m.first_soup as usize..(m.first_soup + m.num_soups) as usize
                })
                .filter(|&si| {
                    bsp.soups
                        .get(si)
                        .is_some_and(|s| kinds[s.material as usize] == MaterialKind::Sky)
                })
                .map(|si| (si, vcod_common::vis::soup_bounds(bsp, si)))
                .collect(),
            static_models: (0..bsp.models.len()).map(|m| m == 0).collect(),
            model_soups: bsp
                .models
                .iter()
                .map(|m| m.first_soup as usize..(m.first_soup + m.num_soups) as usize)
                .collect(),
            moved: Vec::new(),
            moved_draws: Vec::new(),
            s_axis_buf,
            soup_bounds: dlight::soup_bounds(bsp),
            dlight_batches,
            dlights_of_batch,
            dlight_runs: Vec::new(),
            dlight_indices: Vec::new(),
            dlight_index_buf: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("dlight indices"),
                size: 4,
                usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }),
            shadow_casters: None,
        });
        // the materials were dropped with the old map
        self.resolve_viewmodel_skins(fs);
        laps.lap("finish");
        laps.finish();
        Ok(())
    }

    /// The submodels drawn this frame, each with its entity's pose. One at
    /// the identity stands where the map put it and draws with the world;
    /// any other draws its own soups through a camera carrying its pose, so
    /// both keep the baked lightmap, as retail's brush model ref entity
    /// does. A submodel not listed, one whose entity is not in the snapshot,
    /// draws nowhere: retail's world draw holds model 0's surfaces only.
    pub fn set_submodels(&mut self, models: &[(usize, glam::Mat4)]) {
        let Some(world) = &mut self.world else {
            return;
        };
        let mut want: Vec<bool> = (0..world.static_models.len()).map(|m| m == 0).collect();
        world.moved.clear();
        for &(m, pose) in models {
            if m == 0 || m >= want.len() {
                continue;
            }
            if pose == glam::Mat4::IDENTITY {
                want[m] = true;
            } else {
                world.moved.push((m, pose));
            }
        }
        if want != world.static_models {
            world.static_models = want;
        }
    }

    /// A download reopened the search path: reparse the shader scripts the
    /// next `load_world` classifies against, and drop the fx and HUD textures
    /// resolved off the old paks, misses included.
    pub fn reopen(&mut self, fs: &dyn AssetSource) {
        self.shaders = assets::load_shaders(fs);
        self.shader_lib = material_lib(fs);
        self.lib_warn_count = self.shader_lib.warn_count();
        self.fx.textures.clear();
        self.marks.reopen();
        self.hud_pass.textures.clear();
        self.vm_pass.cache.clear();
        self.model_images.clear();
        self.dynamic.skin_groups.clear();
    }

    /// Drops the map and every model uploaded for it; the old map's
    /// `ModelHandle`s go stale and draw nothing.
    pub fn unload_world(&mut self) {
        self.world = None;
        // The recorded runs hold the old map's buffers and bind groups.
        self.world_bundles.runs.clear();
        self.prepass_bundles.runs.clear();
        self.marks.unload();
        self.coronas.clear();
        self.sky = None;
        self.dynamic.handles.clear();
        self.dynamic.epoch = self.dynamic.epoch.wrapping_add(1);
        self.dynamic.models.clear();
        self.dynamic.placed.clear();
        self.dynamic.instances.clear();
        self.dynamic.bone_mats.clear();
        self.dynamic.light_origins.clear();
        self.dynamic.light_boxes.clear();
        self.dynamic.lighting = None;
        self.dynamic.cod2_light = None;
        self.dynamic.cod4_light = None;
        self.dynamic.materials.clear();
        self.dynamic.skin_groups.clear();
        self.vm_pass.skins.clear();
        // decoded under the old map's search path, which a CoD4 map change
        // swaps without a `reopen`
        self.model_images.clear();
        self.dynamic.entity_scene_lights.clear();
        self.vis_counts = VisCounts::default();
    }
}

/// How many images a load decodes on every core before uploading them:
/// a few a core, so the decoded copies held at once stay bounded.
fn par_chunk() -> usize {
    4 * std::thread::available_parallelism().map_or(1, |n| n.get())
}

/// Whether an image as uploaded, `img` after `texture_settings::prepare`
/// and the levels it returned, has a texel under alpha 128 at any level:
/// only then can a 0.5 alpha test drop a filtered sample.
pub(super) fn uploads_cutout(img: &Image, levels: &[Vec<u8>]) -> bool {
    assets::has_cutout_alpha_any_level(img)
        || levels
            .iter()
            .any(|l| l.iter().skip(3).step_by(4).any(|&a| a < 128))
}

/// Whether a script-less `<type>@<image>` world material's
/// `shadertypes/world/<type>.stype` draws opaque: no stage alpha-tests and
/// the first one does not blend. Its image's alpha is then read by nothing
/// (wood@chateau_exteriordoor's is a gloss mask; docs/research/
/// cod11-shader-scripts.md, "World shader types").
pub(super) fn world_type_draws_opaque(fs: &dyn AssetSource, name: &str) -> bool {
    crate::entity_material::skin_type(name)
        .and_then(|kind| {
            fs.load::<RawFile>(&format!("shadertypes/world/{kind}.stype"))
                .ok()
        })
        .is_some_and(|file| stype_draws_opaque(&file.text()))
}

pub(super) fn stype_draws_opaque(text: &str) -> bool {
    let Some((_, body)) = vcod_common::shader::split_blocks(text).into_iter().next() else {
        return false;
    };
    let sh =
        vcod_common::shader::parse_shader("stype", &body, &mut vcod_common::shader::WarnSet::new());
    sh.stages.first().is_some_and(|st| st.blend.is_none())
        && sh.stages.iter().all(|st| st.alpha_func.is_none())
}

/// The materials `fs` draws with: CoD1's shader scripts, or CoD2's
/// compiled binary materials (empty, logged, when they fail).
pub(super) fn material_lib(fs: &dyn AssetSource) -> MaterialLib {
    match fs.game() {
        // `compile_all` fails only on a search path with no `materials/`:
        // a CoD4 install's, whose zones hand theirs over (`set_material_lib`).
        vcod_common::game::Game::Cod2 => vcod_cod2::materials::load_lib(fs).unwrap_or_else(|e| {
            log::debug!("cod2 materials: {e:#}");
            MaterialLib::default()
        }),
        _ => ShaderLib::load(fs).materials(),
    }
}
