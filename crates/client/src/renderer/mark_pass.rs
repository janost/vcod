//! Draws the cgame's impact marks (`crate::marks`) through shader.wgsl's
//! stage shader: each mark shader per lightmap page is its own material,
//! as the renderer copies the shader per surface lightmap (CoDMP.exe
//! 0x4e9369), and its draws take the shader's sort in the frame's list.
//! Unlike the world's stage pipelines, a mark's pipeline carries its
//! stage's exact blend pair and its shader's own polygon offset (the
//! stock decals' `polygonOffset2`, `markShadow`'s `GL_ZERO
//! GL_ONE_MINUS_SRC_COLOR`). The world's corona sprites (`crate::corona`)
//! ride the same buffers, pipelines and runs, rebuilt every view after the
//! marks.

use std::collections::HashMap;

use vcod_common::bsp::{self, DrawVert};
use vcod_common::material::{
    AlphaGen, BlendFactor, Bundle, Clamp, DepthFunc, ImageRef, Material, MaterialLib,
    PolygonOffset, RgbGen, Stage, TcGen, TexEnv,
};
use vcod_common::pk3::Pk3Fs;
use vcod_common::shader::Shader;

use super::{
    AnimFrames, BundleViews, STAGE_PARAMS_SIZE, depth_bias, scene_pipeline, stage_animated,
    stage_bind_groups, stage_params,
};
use crate::entity_material::wgpu_blend_factor;
use crate::lightmap_blocks::LightmapPage;
use crate::marks::{self, MAX_MARK_VERTS, MAX_MARKS, Mark, MarkPool, MarkRequest, MarkWorld};

/// What a mark stage's pipeline varies by: the blend pair, depth write,
/// two-sidedness and the polygon offset.
type PipeKey = (
    Option<(BlendFactor, BlendFactor)>,
    bool,
    bool,
    Option<PolygonOffset>,
);

/// One stage of a mark material: its params slot, pipeline and bind groups.
struct MarkBatch {
    slot: u32,
    pipeline: usize,
    /// group(1): bundle-0 image + lightmap page + samplers.
    mat: AnimFrames,
    /// group(2): params UBO slot + bundle-1 image.
    stage: AnimFrames,
}

/// Params slots for mark shader stages; the stock decals use a few dozen.
const MAX_MARK_SLOTS: u64 = 256;
/// The pool plus a frame's temporary marks (one `markShadow` a player).
const MAX_DRAWN: usize = MAX_MARKS + 256;
const MAX_SPRITES: usize = bsp::MAX_MAP_CORONAS;
const MAX_VERTS: usize = MAX_DRAWN * MAX_MARK_VERTS + MAX_SPRITES * 4;
const MAX_INDICES: usize = MAX_DRAWN * (MAX_MARK_VERTS - 2) * 3 + MAX_SPRITES * 6;

/// A mark shader's parse and its params slot per stage.
struct MarkShader {
    shader: Material,
    sort: f32,
    slots: Vec<u32>,
}

/// One run of marks sharing a shader and a lightmap page, in draw order.
#[derive(Clone)]
pub(super) struct MarkRun {
    key: (String, u16),
    first: u32,
    count: u32,
    pub(super) sort: f32,
}

/// What `MarkPass::update` borrows from the renderer.
pub(super) struct MarkCtx<'a> {
    pub device: &'a wgpu::Device,
    pub queue: &'a wgpu::Queue,
    pub fs: &'a Pk3Fs,
    pub shader_lib: &'a MaterialLib,
    pub material_layout: &'a wgpu::BindGroupLayout,
    pub stage_layout: &'a wgpu::BindGroupLayout,
    pub white_view: &'a wgpu::TextureView,
    pub white_lightmap: &'a LightmapPage,
    pub lightmap_sampler: &'a wgpu::Sampler,
    pub image_gamma: Option<&'a [u8; 256]>,
    pub textures: crate::texture_settings::TextureSettings,
}

pub(super) struct MarkPass {
    pool: MarkPool,
    world: MarkWorld,
    lightmaps: Vec<LightmapPage>,
    vertex_buf: wgpu::Buffer,
    index_buf: wgpu::Buffer,
    params_buf: wgpu::Buffer,
    params_stride: u64,
    slots_used: u32,
    /// `None`: no stage draws, or out of params slots; warned once.
    shaders: HashMap<String, Option<MarkShader>>,
    materials: HashMap<(String, u16), Vec<MarkBatch>>,
    module: wgpu::ShaderModule,
    layout: wgpu::PipelineLayout,
    /// The scene target's (`Renderer::world_format`).
    format: wgpu::TextureFormat,
    pipelines: Vec<(PipeKey, wgpu::RenderPipeline)>,
    views: HashMap<String, wgpu::TextureView>,
    /// The marks' own runs and the vertex and index counts they fill.
    mark_runs: Vec<MarkRun>,
    mark_fill: (u32, u32),
    /// The marks' and this view's sprites' runs, by sort.
    pub(super) runs: Vec<MarkRun>,
}

impl MarkPass {
    /// `module` is shader.wgsl and `layout` the stage pipeline layout.
    pub(super) fn new(
        device: &wgpu::Device,
        module: wgpu::ShaderModule,
        layout: wgpu::PipelineLayout,
        format: wgpu::TextureFormat,
    ) -> MarkPass {
        let align = device.limits().min_uniform_buffer_offset_alignment as u64;
        let params_stride = STAGE_PARAMS_SIZE.div_ceil(align) * align;
        let buf = |label, size, usage| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size,
                usage: usage | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        };
        MarkPass {
            pool: MarkPool::default(),
            world: MarkWorld::default(),
            lightmaps: Vec::new(),
            vertex_buf: buf(
                "mark vertices",
                (MAX_VERTS * std::mem::size_of::<DrawVert>()) as u64,
                wgpu::BufferUsages::VERTEX,
            ),
            index_buf: buf(
                "mark indices",
                (MAX_INDICES * 4) as u64,
                wgpu::BufferUsages::INDEX,
            ),
            params_buf: buf(
                "mark stage params",
                MAX_MARK_SLOTS * params_stride,
                wgpu::BufferUsages::UNIFORM,
            ),
            params_stride,
            slots_used: 0,
            shaders: HashMap::new(),
            materials: HashMap::new(),
            module,
            layout,
            format,
            pipelines: Vec::new(),
            views: HashMap::new(),
            mark_runs: Vec::new(),
            mark_fill: (0, 0),
            runs: Vec::new(),
        }
    }

    /// A new map: its triangles and lightmap pages; the old map's marks go.
    pub(super) fn load(&mut self, world: MarkWorld, lightmaps: Vec<LightmapPage>) {
        self.unload();
        log::info!(
            "marks: {} world triangles take marks",
            world.triangle_count()
        );
        self.world = world;
        self.lightmaps = lightmaps;
    }

    pub(super) fn unload(&mut self) {
        self.pool.clear();
        self.world = MarkWorld::default();
        self.lightmaps.clear();
        self.materials.clear();
        self.mark_runs.clear();
        self.mark_fill = (0, 0);
        self.runs.clear();
    }

    /// Search paths changed: shaders and images resolve again.
    pub(super) fn reopen(&mut self) {
        self.shaders.clear();
        self.materials.clear();
        self.views.clear();
        self.slots_used = 0;
    }

    pub(super) fn live(&self) -> usize {
        self.pool.len()
    }

    /// `CG_MapRestart`'s `CG_InitMarkPolys` (cgame 0x3002cabe).
    pub(super) fn clear(&mut self) {
        self.pool.clear();
        self.mark_runs.clear();
        self.mark_fill = (0, 0);
        self.runs.clear();
    }

    /// One frame of `CG_ImpactMark` and `CG_AddMarks`: place `requests`,
    /// drop the expired, fade the rest and upload them newest first after
    /// this frame's `temporary` marks, one run per (shader, lightmap)
    /// ordered by sort. With `cg_marks 0` the cgame neither places nor
    /// draws nor expires marks, temporary ones included.
    pub(super) fn update(
        &mut self,
        ctx: &MarkCtx,
        requests: &[MarkRequest],
        temporary: &[MarkRequest],
        now_ms: i32,
        time: f32,
        enabled: bool,
    ) {
        self.runs.clear();
        self.mark_runs.clear();
        self.mark_fill = (0, 0);
        if !enabled {
            return;
        }
        for req in requests {
            self.pool.impact(&self.world, req, now_ms);
        }
        self.pool.expire(now_ms);
        // added during the packet entities, before `CG_AddMarks`
        let temps: Vec<Mark> = temporary
            .iter()
            .flat_map(|req| marks::place(&self.world, req, now_ms))
            .take(MAX_DRAWN - MAX_MARKS)
            .collect();
        let mut groups: Vec<((String, u16), Vec<usize>)> = Vec::new();
        for (i, m) in temps.iter().chain(self.pool.newest_first()).enumerate() {
            match groups
                .iter_mut()
                .find(|(k, _)| k.0 == m.shader && k.1 == m.lightmap)
            {
                Some((_, v)) => v.push(i),
                None => groups.push(((m.shader.clone(), m.lightmap), vec![i])),
            }
        }
        groups.retain(|(key, _)| self.material(ctx, key).is_some());
        let marks: Vec<&Mark> = temps.iter().chain(self.pool.newest_first()).collect();
        let mut verts: Vec<DrawVert> = Vec::new();
        let mut indices: Vec<u32> = Vec::new();
        let mut runs = Vec::new();
        for (key, members) in groups {
            let Some(sort) = self.shaders[&key.0].as_ref().map(|s| s.sort) else {
                continue;
            };
            let first = indices.len() as u32;
            for i in members {
                let m = marks[i];
                let rgba = m.rgba_at(now_ms);
                let base = verts.len() as u32;
                verts.extend(m.verts.iter().map(|v| DrawVert {
                    pos: v.pos,
                    uv: v.st,
                    lm_uv: v.lm,
                    normal: [0.0; 3],
                    color: rgba,
                }));
                for k in 1..m.verts.len().saturating_sub(1) as u32 {
                    indices.extend_from_slice(&[base, base + k, base + k + 1]);
                }
            }
            runs.push(MarkRun {
                key,
                first,
                count: indices.len() as u32 - first,
                sort,
            });
        }
        // stable: newest first within a sort
        runs.sort_by(|a, b| a.sort.total_cmp(&b.sort));
        ctx.queue
            .write_buffer(&self.vertex_buf, 0, bytemuck::cast_slice(&verts));
        ctx.queue
            .write_buffer(&self.index_buf, 0, bytemuck::cast_slice(&indices));
        for ms in self.shaders.values().flatten() {
            for (si, &slot) in ms.slots.iter().enumerate() {
                if stage_animated(&ms.shader, si)
                    && let Some(p) = stage_params(&ms.shader, si, time)
                {
                    ctx.queue.write_buffer(
                        &self.params_buf,
                        u64::from(slot) * self.params_stride,
                        bytemuck::bytes_of(&p),
                    );
                }
            }
        }
        self.mark_fill = (verts.len() as u32, indices.len() as u32);
        self.mark_runs = runs.clone();
        self.runs = runs;
    }

    /// Builds `name`'s unlit material ahead of `set_sprites`, which has no
    /// file system to load it with. `false` when it cannot draw.
    pub(super) fn prepare_sprites(&mut self, ctx: &MarkCtx, name: &str) -> bool {
        self.material(ctx, &(name.to_string(), bsp::NO_LIGHTMAP))
            .is_some()
    }

    /// This view's sprites of the prepared shader `name`, after the marks
    /// in the buffers and merged into `runs` by sort (after the marks on a
    /// tie). Replaces the last view's sprites.
    pub(super) fn set_sprites(
        &mut self,
        queue: &wgpu::Queue,
        name: &str,
        sprites: &[crate::corona::Sprite],
    ) {
        self.runs.clone_from(&self.mark_runs);
        let key = (name.to_string(), bsp::NO_LIGHTMAP);
        let Some(sort) = self
            .shaders
            .get(name)
            .and_then(|s| s.as_ref())
            .map(|s| s.sort)
        else {
            return;
        };
        if sprites.is_empty() || !self.materials.contains_key(&key) {
            return;
        }
        let sprites = &sprites[..sprites.len().min(MAX_SPRITES)];
        let (v0, i0) = self.mark_fill;
        let mut verts = Vec::with_capacity(sprites.len() * 4);
        let mut indices = Vec::with_capacity(sprites.len() * 6);
        for (n, sp) in sprites.iter().enumerate() {
            let base = v0 + n as u32 * 4;
            for (pos, uv) in sp.corners.iter().zip(crate::corona::UVS) {
                verts.push(DrawVert {
                    pos: *pos,
                    uv,
                    lm_uv: [0.0; 2],
                    normal: [0.0; 3],
                    color: sp.rgba,
                });
            }
            indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
        }
        let vsize = std::mem::size_of::<DrawVert>() as u64;
        queue.write_buffer(
            &self.vertex_buf,
            u64::from(v0) * vsize,
            bytemuck::cast_slice(&verts),
        );
        queue.write_buffer(
            &self.index_buf,
            u64::from(i0) * 4,
            bytemuck::cast_slice(&indices),
        );
        let at = self.runs.partition_point(|r| r.sort <= sort);
        self.runs.insert(
            at,
            MarkRun {
                key,
                first: i0,
                count: indices.len() as u32,
                sort,
            },
        );
    }

    /// The stage draws for a (shader, lightmap), built on first use.
    fn material(&mut self, ctx: &MarkCtx, key: &(String, u16)) -> Option<&Vec<MarkBatch>> {
        if !self.shaders.contains_key(&key.0) {
            let ms = self.register(ctx, &key.0);
            self.shaders.insert(key.0.clone(), ms);
        }
        let ms = self.shaders[&key.0].as_ref()?;
        if !self.materials.contains_key(key) {
            let lm_view = match key.1 {
                bsp::NO_LIGHTMAP => ctx.white_lightmap,
                i => self.lightmaps.get(i as usize).unwrap_or(ctx.white_lightmap),
            };
            let mut bundle_views = BundleViews {
                views: std::mem::take(&mut self.views),
                table: ctx.image_gamma,
                picmip: ctx.textures.world_picmip(ms.shader.nopicmip),
                mipmap: !ms.shader.nomipmaps,
                store: ctx.textures.store(),
                prefetched: None,
            };
            let mut batches = Vec::new();
            for (si, st) in ms.shader.stages.iter().enumerate() {
                let Some(cs) = ms.shader.classify_stage(si) else {
                    continue;
                };
                let pipe_key = (
                    st.blend.clone(),
                    cs.depth_write,
                    cs.two_sided,
                    ms.shader.depth_bias,
                );
                let pipeline = match self.pipelines.iter().position(|(k, _)| *k == pipe_key) {
                    Some(i) => i,
                    None => {
                        let p = mark_pipeline(
                            ctx.device,
                            &self.module,
                            self.format,
                            &self.layout,
                            &pipe_key,
                        );
                        self.pipelines.push((pipe_key, p));
                        self.pipelines.len() - 1
                    }
                };
                let (mat, stage) = stage_bind_groups(
                    ctx.device,
                    ctx.queue,
                    ctx.fs,
                    ctx.material_layout,
                    ctx.stage_layout,
                    &self.params_buf,
                    STAGE_PARAMS_SIZE,
                    ctx.white_view,
                    lm_view,
                    ctx.lightmap_sampler,
                    &mut bundle_views,
                    st,
                );
                batches.push(MarkBatch {
                    slot: ms.slots[si],
                    pipeline,
                    mat,
                    stage,
                });
            }
            self.views = bundle_views.views;
            self.materials.insert(key.clone(), batches);
        }
        self.materials.get(key)
    }

    /// Mark shaders registered so far.
    #[cfg(test)]
    pub(super) fn registered(&self) -> usize {
        self.shaders.len()
    }

    /// Registers each shader of `names` and builds its stages unlit
    /// (`NO_LIGHTMAP`): the images, bind groups and pipelines a first mark
    /// would build in its frame.
    /// Their stages' pipelines are built first, all at once
    /// (`par_build`), registered in the same order a first mark would.
    pub(super) fn prewarm<'a>(&mut self, ctx: &MarkCtx, names: impl IntoIterator<Item = &'a str>) {
        let names: Vec<&str> = names.into_iter().collect();
        let mut fresh: Vec<PipeKey> = Vec::new();
        for name in &names {
            if !self.shaders.contains_key(*name) {
                let ms = self.register(ctx, name);
                self.shaders.insert(name.to_string(), ms);
            }
            let Some(ms) = &self.shaders[*name] else {
                continue;
            };
            for (si, st) in ms.shader.stages.iter().enumerate() {
                let Some(cs) = ms.shader.classify_stage(si) else {
                    continue;
                };
                let key = (
                    st.blend.clone(),
                    cs.depth_write,
                    cs.two_sided,
                    ms.shader.depth_bias,
                );
                if !fresh.contains(&key) && !self.pipelines.iter().any(|(k, _)| *k == key) {
                    fresh.push(key);
                }
            }
        }
        let (module, format, layout) = (&self.module, self.format, &self.layout);
        let built = super::par_build(fresh.len(), |i| {
            mark_pipeline(ctx.device, module, format, layout, &fresh[i])
        });
        self.pipelines.extend(fresh.into_iter().zip(built));
        for name in names {
            self.material(ctx, &(name.to_string(), bsp::NO_LIGHTMAP));
        }
    }

    /// Parses a mark shader and fills its params slots. A name with no
    /// script draws its image blended under vertex colour.
    fn register(&mut self, ctx: &MarkCtx, name: &str) -> Option<MarkShader> {
        let shader = match ctx.shader_lib.get(name) {
            Some(sh) if !sh.stages.is_empty() => sh.clone(),
            Some(_) => {
                log::warn!("mark shader {name}: no stages, not drawn");
                return None;
            }
            None => {
                log::warn!("mark shader {name}: no script, drawing its image blended");
                implicit_mark_shader(name)
            }
        };
        let n = shader.stages.len() as u32;
        if u64::from(self.slots_used + n) > MAX_MARK_SLOTS {
            log::warn!("mark shader {name}: out of params slots, not drawn");
            return None;
        }
        let slots: Vec<u32> = (self.slots_used..self.slots_used + n).collect();
        self.slots_used += n;
        for (si, &slot) in slots.iter().enumerate() {
            if let Some(p) = stage_params(&shader, si, 0.0) {
                ctx.queue.write_buffer(
                    &self.params_buf,
                    u64::from(slot) * self.params_stride,
                    bytemuck::bytes_of(&p),
                );
            }
        }
        Some(MarkShader {
            sort: shader.sort,
            shader,
            slots,
        })
    }

    /// Draws run `ri`'s stages back to back, the caller having bound the
    /// world camera at group 0; the number of draws.
    pub(super) fn draw(&self, pass: &mut wgpu::RenderPass<'_>, ri: usize, time: f32) -> u32 {
        let run = &self.runs[ri];
        let Some(batches) = self.materials.get(&run.key) else {
            return 0;
        };
        pass.set_vertex_buffer(0, self.vertex_buf.slice(..));
        pass.set_index_buffer(self.index_buf.slice(..), wgpu::IndexFormat::Uint32);
        for sb in batches {
            pass.set_pipeline(&self.pipelines[sb.pipeline].1);
            pass.set_bind_group(1, sb.mat.bind_group(time), &[]);
            pass.set_bind_group(
                2,
                sb.stage.bind_group(time),
                &[u32::try_from(u64::from(sb.slot) * self.params_stride)
                    .expect("mark params slot offset fits a dynamic offset")],
            );
            pass.draw_indexed(run.first..run.first + run.count, 0, 0..1);
        }
        batches.len() as u32
    }
}

/// A mark stage's pipeline: its blend pair on colour and alpha alike, as GL
/// blends, the back faces culled unless two-sided.
fn mark_pipeline(
    device: &wgpu::Device,
    module: &wgpu::ShaderModule,
    format: wgpu::TextureFormat,
    layout: &wgpu::PipelineLayout,
    (blend, depth_write, two_sided, offset): &PipeKey,
) -> wgpu::RenderPipeline {
    let blend = blend.as_ref().map(|(src, dst)| {
        let c = wgpu::BlendComponent {
            src_factor: wgpu_blend_factor(src),
            dst_factor: wgpu_blend_factor(dst),
            operation: wgpu::BlendOperation::Add,
        };
        wgpu::BlendState { color: c, alpha: c }
    });
    scene_pipeline(
        device,
        module,
        format,
        &format!("mark stage {blend:?} depthwrite={depth_write} twosided={two_sided} {offset:?}"),
        layout,
        ("vs_stage", "fs_stage"),
        offset.map(depth_bias).unwrap_or_default(),
        false,
        blend,
        *depth_write,
        (!two_sided).then_some(wgpu::Face::Back),
    )
}

fn implicit_mark_shader(name: &str) -> Material {
    let bundle = |image| Bundle {
        image,
        anim: None,
        clamp: Clamp::NONE,
        tcmods: Vec::new(),
        tc_gen: TcGen::Base,
        env: TexEnv::Modulate,
    };
    Material::from_shader(&Shader {
        name: name.to_string(),
        stages: vec![Stage {
            bundles: vec![
                bundle(ImageRef::Path(name.to_string())),
                bundle(ImageRef::Lightmap),
            ],
            blend: Some((BlendFactor::SrcAlpha, BlendFactor::OneMinusSrcAlpha)),
            depth_write: None,
            alpha_func: None,
            depth_func: DepthFunc::LessEqual,
            rgb_gen: RgbGen::Vertex,
            alpha_gen: AlphaGen::Vertex,
        }],
        polygon_offset: Some(PolygonOffset::Single),
        ..Shader::default()
    })
}
