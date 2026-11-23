//! Live entities and the view weapon: dynamic model upload, instance packing,
//! placement, LOD and light picks, the entity draw order and the viewmodel
//! pass.

use super::*;
use crate::renderer::gpu_util;

/// The viewmodel shares the world's fov but has its own near plane, retail's
/// `r_znear_depthhack` default (docs/research/xmodel-v14-format.md, "The
/// view fov").
pub const VM_NEAR: f32 = 0.1;
pub(crate) const VM_FAR: f32 = 500.0;
/// The depth range's front slice the viewmodel is squeezed into, retail's
/// 0..0.2 for a depth-hack entity (CoDMP.exe 0x4d6a7e).
pub(crate) const VM_DEPTH_RANGE: f32 = 0.2;
/// `proj` (64) + `model` (64) + the light set (656) + `misc` (16) +
/// `view_to_world` (64), std140-compatible as is.
pub(super) const VM_UNIFORM_SIZE: u64 = VM_VIEW_TO_WORLD_OFFSET + 64;
/// The light set sits after the projection and motion transform.
pub(super) const VM_LIGHTS_OFFSET: u64 = 128;
/// `misc` (x identityLight, y 1 on CoD2, z the clock) follows the light set.
pub(super) const VM_MISC_OFFSET: u64 = VM_LIGHTS_OFFSET + std::mem::size_of::<GpuLightSet>() as u64;
/// The view's rotation back to the world, which a CoD2 probe set reads.
pub(super) const VM_VIEW_TO_WORLD_OFFSET: u64 = VM_MISC_OFFSET + 16;
/// Matches `array<mat4x4<f32>, 128>` in viewmodel.wgsl.
pub(crate) const VM_BONE_COUNT: usize = 128;
pub(super) const VM_BONE_BUF_SIZE: u64 = (VM_BONE_COUNT * 64) as u64;

/// One entity or viewmodel surface in draw order.
#[derive(Clone, Copy)]
pub(super) struct EntDraw {
    /// Instance (or viewmodel part) and its surface.
    pub(super) inst: u32,
    pub(super) surf: u32,
    pub(super) sort: f32,
    pub(super) order: u32,
    /// `EntSkin::anchor`: where it draws among the world's on a sort tie.
    pub(super) anchor: u32,
    /// A viewmodel surface: depth-hack range and the view camera.
    pub(super) gun: bool,
}

/// The live entities' and the viewmodel's `(instance, surface, material)`
/// in instance order, in one list sorted as retail's key does: the
/// material's sort, its place among the model shaders (`EntSkin::order`,
/// the sorted index), then the entity number. The gun is added after the packet entities, so on one material
/// it draws after them (docs/research/cod11-shader-scripts.md, §5).
pub(super) fn sorted_ent_draws<'a>(
    ents: impl Iterator<Item = (u32, u32, &'a EntSkin)>,
    gun: impl Iterator<Item = (u32, u32, &'a EntSkin)>,
) -> Vec<EntDraw> {
    let draw = |is_gun| {
        move |(inst, surf, sk): (u32, u32, &EntSkin)| EntDraw {
            inst,
            surf,
            sort: sk.sort,
            order: sk.order.get(),
            anchor: sk.anchor.get(),
            gun: is_gun,
        }
    };
    let mut out: Vec<EntDraw> = ents.map(draw(false)).chain(gun.map(draw(true))).collect();
    // stable: equal keys keep instance then surface order
    out.sort_by(draw_order);
    out
}

/// [`sorted_ent_draws`]' key.
fn draw_order(a: &EntDraw, b: &EntDraw) -> std::cmp::Ordering {
    a.sort
        .total_cmp(&b.sort)
        .then(a.order.cmp(&b.order))
        .then(a.gun.cmp(&b.gun))
}

/// Two lists sorted by [`draw_order`] as one, `a`'s first on a tie: what a
/// stable sort of `a` then `b` gives.
pub(super) fn merge_draws(
    a: impl IntoIterator<Item = EntDraw>,
    b: impl IntoIterator<Item = EntDraw>,
) -> Vec<EntDraw> {
    let (mut a, mut b) = (a.into_iter().peekable(), b.into_iter().peekable());
    let mut out = Vec::with_capacity(a.size_hint().0 + b.size_hint().0);
    loop {
        let next = match (a.peek(), b.peek()) {
            (Some(x), Some(y)) if draw_order(x, y).is_gt() => b.next(),
            (Some(_), _) => a.next(),
            (None, _) => b.next(),
        };
        match next {
            Some(d) => out.push(d),
            None => return out,
        }
    }
}

/// Every instance's entity draws sorted as [`sorted_ent_draws`] sorts
/// them, kept from frame to frame: the instances that lead in last frame's
/// models keep their draws, and only those after them sort again. A map's
/// held dynamic entities lead the list (cod4-client-perf.md 41).
#[derive(Default)]
pub(super) struct EntSorted {
    /// Per instance, its `DynamicPass::models` index.
    models: Vec<usize>,
    pub(super) draws: Vec<EntDraw>,
}

impl EntSorted {
    pub(super) fn update(&mut self, instances: &[(usize, InstanceRaw)], models: &[DynModel]) {
        self.update_by(instances.iter().map(|(m, _)| *m), |m| &models[m].skins);
    }

    /// [`Self::update`] over the instances' model indices, each model's
    /// skins by surface from `skins`.
    pub(super) fn update_by<'a>(
        &mut self,
        instances: impl ExactSizeIterator<Item = usize> + Clone,
        skins: impl Fn(usize) -> &'a [Rc<EntSkin>],
    ) {
        let n = instances.len();
        let held = self
            .models
            .iter()
            .zip(instances.clone())
            .take_while(|(a, b)| **a == *b)
            .count();
        if held == n && held == self.models.len() {
            return;
        }
        self.models.truncate(held);
        self.models.extend(instances.skip(held));
        let rest = sorted_ent_draws(
            self.models[held..].iter().enumerate().flat_map(|(j, &m)| {
                skins(m)
                    .iter()
                    .enumerate()
                    .map(move |(s, sk)| ((held + j) as u32, s as u32, &**sk))
            }),
            std::iter::empty(),
        );
        let kept = std::mem::take(&mut self.draws);
        self.draws = merge_draws(kept.into_iter().filter(|e| (e.inst as usize) < held), rest);
    }
}

/// What a run of entity draws left bound: the pipeline (a stage's index,
/// or a prepass one's `usize::MAX - n`), and since that pipeline the image
/// group's address and the stage parameters' group and offset, so a draw
/// rebinds only what changed (cod4-client-perf.md 38).
#[derive(Clone, Copy, Default, PartialEq)]
pub(super) struct EntBound {
    pub(super) pipeline: Option<usize>,
    pub(super) image: Option<usize>,
    pub(super) params: Option<(usize, u32)>,
}

/// Draws one entity surface's stages back to back, as retail's stage
/// iterator does, over `indices` (the surface's own, or a run of its
/// neighbours' from [`ent_run`]) and `instances` (one, or a run of one
/// model's from [`ent_instances`]). `bound` tracks the bound pipeline
/// across calls.
#[expect(clippy::too_many_arguments)]
pub(super) fn draw_ent_stages(
    pass: &mut impl DrawSink,
    mats: &EntityMaterials,
    which: EntPass,
    skin_bg: &wgpu::BindGroup,
    skin: &EntSkin,
    surf: &VmSurface,
    (indices, instances): (std::ops::Range<u32>, std::ops::Range<u32>),
    bound: &mut EntBound,
    depth: Depth,
    census: &std::cell::Cell<PassCensus>,
) {
    for (si, st) in skin.stages.iter().enumerate() {
        // the prepass draws the first stage alone
        if matches!(depth, Depth::Prepass | Depth::FloatZ) && si > 0 {
            break;
        }
        // the prepass's own pipelines bind as `usize::MAX - n`, culled as
        // the stage's own pipeline is
        let cull = mats.pipelines[st.pipeline].0.3;
        let c = usize::from(cull);
        let (key, pipeline) = match depth {
            Depth::Prepass => (usize::MAX - 4 * c, &mats.prepass[c][0]),
            Depth::FloatZ => (usize::MAX - 2 - 4 * c, &mats.floatz[c]),
            Depth::Equal if si == 0 => (usize::MAX - 1 - 4 * c, &mats.prepass[c][1]),
            _ => (st.pipeline, &mats.pipelines[st.pipeline].1[which as usize]),
        };
        if bound.pipeline != Some(key) {
            pass.pipeline(pipeline);
            *bound = EntBound {
                pipeline: Some(key),
                ..EntBound::default()
            };
            tally(census, |c| c.pipelines += 1);
        }
        let image = match st.image {
            StageImage::Skin => skin_bg,
            StageImage::Own(i) => &mats.images[i],
        };
        let image_key = Some(std::ptr::from_ref(image) as usize);
        if bound.image != image_key {
            pass.group(1, image, None);
            bound.image = image_key;
            tally(census, |c| c.bind_groups += 1);
        }
        let params = (st.params, (u64::from(st.slot) * ENT_STAGE_STRIDE) as u32);
        if bound.params != Some(params) {
            pass.group(3, &mats.param_groups[params.0], Some(params.1));
            bound.params = Some(params);
            tally(census, |c| c.bind_groups += 1);
        }
        pass.draw_inst(indices.clone(), surf.base_vertex, instances.clone());
        tally(census, |c| match (depth, which) {
            (Depth::Prepass | Depth::FloatZ, _) => c.prepass_ents += 1,
            (_, EntPass::View) => c.gun += 1,
            _ => c.ents += 1,
        });
    }
}

/// How many of `items` from `k` on draw as one: the entity surface at `k`
/// and each next item that is the same instance's next surface on the
/// same one-stage skin and bind group, its indices following on. One draw
/// over their joined range rasterizes the same triangles in the same
/// order. `stops` are the item indices something else happens before
/// (a split, the dlights), which a run never crosses.
/// `surfaces` and `skins` are the model's, by surface.
pub(super) fn ent_run(
    items: &[SortedItem],
    k: usize,
    draws: &[EntDraw],
    (surfaces, skins): (&[VmSurface], &[Rc<EntSkin>]),
    stops: &[Option<usize>],
) -> usize {
    let SortedItem::Model(first) = items[k] else {
        return 1;
    };
    let d = &draws[first];
    let skin = &skins[d.surf as usize];
    if skin.stages.len() != 1 {
        return 1;
    }
    let mut end = &surfaces[d.surf as usize];
    let mut n = 1;
    while let Some(&SortedItem::Model(next)) = items.get(k + n) {
        let e = &draws[next];
        let Some(surf) = surfaces.get(e.surf as usize) else {
            break;
        };
        if e.gun
            || e.inst != d.inst
            || e.surf != d.surf + n as u32
            || stops.contains(&Some(k + n))
            || !Rc::ptr_eq(&skins[e.surf as usize], skin)
            || surf.bind_group != end.bind_group
            || surf.base_vertex != end.base_vertex
            || surf.first_index != end.first_index + end.index_count
        {
            break;
        }
        end = surf;
        n += 1;
    }
    n
}

/// How many instances the run of `n` items at `k` ([`ent_run`]) draws
/// over as one instanced draw: it and each next `n` items that repeat it
/// on the next instance of the same model, no stop among them. The draw
/// rasterizes instance by instance, so in the same order as one draw
/// each. Only a one-stage skin repeats: a second stage would draw after
/// every instance's first. `model_of` is an instance's model.
pub(super) fn ent_instances(
    items: &[SortedItem],
    (k, n): (usize, usize),
    draws: &[EntDraw],
    model_of: impl Fn(u32) -> usize,
    one_stage: bool,
    stops: &[Option<usize>],
) -> usize {
    let SortedItem::Model(first) = items[k] else {
        return 1;
    };
    let d = &draws[first];
    if !one_stage || d.gun {
        return 1;
    }
    let model = model_of(d.inst);
    let repeats = |r: usize| {
        (0..n).all(|t| {
            let at = k + r * n + t;
            match items.get(at) {
                Some(&SortedItem::Model(m)) => {
                    let e = &draws[m];
                    !e.gun
                        && e.inst as usize == d.inst as usize + r
                        && e.surf as usize == d.surf as usize + t
                        && model_of(e.inst) == model
                        && !stops.contains(&Some(at))
                }
                _ => false,
            }
        })
    };
    let mut r = 1;
    while repeats(r) {
        r += 1;
    }
    r
}

/// The depth prepass's [`ent_instances`]: how many instances the draw
/// of `surfs[k..k + n]` (one instance's surfaces, `(instance, surface)`)
/// takes, it and each next `n` entries that repeat it on the next
/// instance of the same model.
pub(super) fn prepass_instances(
    surfs: &[(u32, u32)],
    (k, n): (usize, usize),
    model_of: impl Fn(u32) -> usize,
) -> usize {
    let (inst, _) = surfs[k];
    let model = model_of(inst);
    let repeats = |r: usize| {
        (0..n).all(|t| {
            surfs.get(k + r * n + t).is_some_and(|&(i, s)| {
                i as usize == inst as usize + r && s == surfs[k + t].1 && model_of(i) == model
            })
        })
    };
    let mut r = 1;
    while repeats(r) {
        r += 1;
    }
    r
}

/// The uploaded surfaces [`Renderer::dynamic_model_hiding`] draws, by
/// index: those whose source surface (`VmSurface::src`) `keep` does not set
/// false. A surface past `keep`'s end stays.
pub(super) fn kept_surfaces(surfaces: &[VmSurface], keep: &[bool]) -> Vec<usize> {
    (0..surfaces.len())
        .filter(|&i| keep.get(surfaces[i].src as usize).copied().unwrap_or(true))
        .collect()
}

#[derive(Clone)]
pub(super) struct VmSurface {
    pub(super) first_index: u32,
    pub(super) index_count: u32,
    /// Surfaces share one vertex buffer. A model whose vertices all fit
    /// u16 indices has its indices rebased and every `base_vertex` 0, so
    /// neighbouring surfaces on one skin draw as one range
    /// ([`ent_run`]); a bigger one keeps each surface's indices unrebased.
    pub(super) base_vertex: i32,
    /// One bind group per distinct skin of the model.
    pub(super) bind_group: usize,
    /// Its index in the `model::Surface`s it was uploaded from (an empty
    /// one uploads nothing).
    pub(super) src: u32,
}

#[derive(Clone)]
pub(super) struct VmModel {
    pub(super) vertex_buf: wgpu::Buffer,
    pub(super) index_buf: wgpu::Buffer,
    pub(super) bind_groups: Vec<wgpu::BindGroup>,
    pub(super) surfaces: Vec<VmSurface>,
    /// Per surface, its skin's name: the key of its material.
    pub(super) skins: Vec<String>,
    /// The view model's own bone matrices; a dynamic model's LODs pose
    /// through the shared `DynamicPass::bone_buf` and carry none.
    pub(super) bones: Option<VmBones>,
}

#[derive(Clone)]
pub(super) struct VmBones {
    pub(super) buf: wgpu::Buffer,
    pub(super) bg: wgpu::BindGroup,
}

/// Built up front so the shader is validated at startup even with no viewmodel set.
pub(super) struct VmPass {
    pub(super) uniform_layout: wgpu::BindGroupLayout,
    pub(super) uniform_buf: wgpu::Buffer,
    pub(super) uniform_bg: wgpu::BindGroup,
    pub(super) skin_layout: wgpu::BindGroupLayout,
    pub(super) bone_layout: wgpu::BindGroupLayout,
    pub(super) models: Vec<Rc<VmModel>>,
    /// Per model, per surface, its material; empty until resolved against
    /// this map's entity materials.
    pub(super) skins: Vec<Vec<Rc<EntSkin>>>,
    /// Every viewmodel part uploaded so far, keyed by [`vm_cache_key`], so a
    /// weapon switch back to a gun (and every gun's shared hands) skips the
    /// upload. Only one viewmodel draws at a time, so parts can share their
    /// bone buffer.
    pub(super) cache: HashMap<String, Rc<VmModel>>,
    /// Parts uploaded inside `set_viewmodel`, the draw's own first uses:
    /// none once a level's view weapons are preloaded (cod2-client-perf.md,
    /// "The view weapon's first draw").
    pub(super) first_uses: usize,
}

/// LOD name plus skins: the hands files share one mesh and differ only in
/// their sleeve skins.
pub(super) fn vm_cache_key(m: &model::Model) -> String {
    format!("{}|{}", m.name, m.materials().join("|"))
}

/// Each player is 4-7 part instances, so 32 players plus corpses and dropped
/// weapons land at 200-280; a CoD4 map's dynents add up to 1901 unskinned
/// ones (cod4-client-perf.md 41).
pub(super) const MAX_DYNAMIC_INSTANCES: usize = 4096;
/// Skinned instances the bone buffer holds, each up to [`MAX_INSTANCE_BONES`].
pub(super) const MAX_SKINNED_INSTANCES: usize = 512;
// mp_shipment's dynents must leave room for every entity besides them.
const _: () = assert!(MAX_DYNAMIC_INSTANCES >= 1901 + MAX_SKINNED_INSTANCES);
pub(crate) const DYNAMIC_INSTANCE_STRIDE: u64 = 80;
/// Unskinned instances point at a shared identity block of this many matrices.
pub const MAX_INSTANCE_BONES: usize = 128;

/// Index into the renderer's dynamic model table, valid until the next
/// [`Renderer::unload_world`]: a handle from an older map draws nothing.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct ModelHandle {
    pub(crate) index: usize,
    /// The table's [`DynamicPass::epoch`] when the handle was made.
    epoch: u32,
}

impl ModelHandle {
    /// A handle of the first map's table, for tests without a GPU.
    #[cfg(test)]
    pub(crate) fn first_map(index: usize) -> Self {
        ModelHandle { index, epoch: 0 }
    }

    /// Its index into a table of `len` handles at `epoch`, if it is one of them.
    pub(super) fn live(self, epoch: u32, len: usize) -> Option<usize> {
        (self.epoch == epoch && self.index < len).then_some(self.index)
    }
}

/// `bones: None` draws the baked bind pose; `Some` is truncated at [`MAX_INSTANCE_BONES`].
pub struct DynamicModelInstance {
    pub model: ModelHandle,
    pub transform: glam::Mat4,
    pub bones: Option<Vec<glam::Mat4>>,
    /// Where the light grid is sampled: the refEntity's lighting origin.
    pub light_origin: glam::Vec3,
    /// The material clock in seconds when the refEntity carries its own
    /// `shaderTime` (a UO vehicle's treads and tyres); `None` runs the
    /// frame's.
    pub shader_time: Option<f32>,
}

/// Per-instance vertex data. `bone_base` 0 is the shared identity block.
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
pub struct InstanceRaw {
    pub transform: [f32; 16],
    pub bone_base: u32,
    /// Index into the frame's light sets.
    pub light_set: u32,
    /// Seconds this instance's material clock runs ahead of the frame's.
    pub time_offset: f32,
    pub _pad: u32,
}

// The instance vertex layout below hardcodes this stride.
const _: () = assert!(std::mem::size_of::<InstanceRaw>() == 80);

/// Bone buffer slot 0 is a [`MAX_INSTANCE_BONES`]-matrix identity block and
/// `None` bone sets point there. The `usize` per tuple is the model index, unused here.
#[allow(clippy::type_complexity)]
pub fn pack_instances(
    instances: &[(usize, [f32; 16], Option<&[glam::Mat4]>)],
) -> (Vec<InstanceRaw>, Vec<[f32; 16]>) {
    static BONE_CAP_WARNED: std::sync::Once = std::sync::Once::new();

    let mut mats: Vec<[f32; 16]> = vec![glam::Mat4::IDENTITY.to_cols_array(); MAX_INSTANCE_BONES];
    let mut raw = Vec::with_capacity(instances.len());
    for &(_, transform, bones) in instances {
        let bone_base = match bones {
            None => 0u32,
            Some(bones) => {
                let base = mats.len() as u32;
                let bones = if bones.len() > MAX_INSTANCE_BONES {
                    BONE_CAP_WARNED.call_once(|| {
                        log::warn!(
                            "dynamic instance bone set exceeds {MAX_INSTANCE_BONES} bones; truncating"
                        );
                    });
                    &bones[..MAX_INSTANCE_BONES]
                } else {
                    bones
                };
                mats.extend(bones.iter().map(glam::Mat4::to_cols_array));
                base
            }
        };
        raw.push(InstanceRaw {
            transform,
            bone_base,
            light_set: 0,
            time_offset: 0.0,
            _pad: 0,
        });
    }
    (raw, mats)
}

/// `bone_sets[i]` belongs to `set_viewmodel` model i. Absent or empty leaves
/// that model's GPU buffer as it was.
pub struct VmDraw {
    pub transform: glam::Mat4,
    /// World-space lighting origin: the eye, `ps.origin` plus
    /// `viewHeightCurrent` (cgame 0x30036cf0).
    pub light_origin: glam::Vec3,
    /// Horizontal degrees, the world view's own.
    pub fov_x: f32,
    pub bone_sets: Vec<Vec<glam::Mat4>>,
}

/// A CoD4 map's entity lighting: the grid lookup, the sun and the spot
/// and omni lights.
pub(super) struct Cod4EntityLight {
    pub(super) lighter: vcod_common::mapworld::EntityLighter,
    pub(super) sun_dir: glam::Vec3,
    pub(super) sun_colour: glam::Vec3,
    pub(super) lights: Vec<Option<vcod_common::mapworld::ModelLightSource>>,
    /// Last frame's lookups in set order, kept as 1.7 keeps a model's block
    /// while its lighting origin holds (cod4-world.md 3.3).
    kept: Vec<(LightKey, vcod_common::mapworld::EntityLight)>,
    /// Last frame's sets and blocks, by set: set `i` is `kept[i]` packed
    /// for its block at `identity`'s bits.
    sets: Vec<GpuLightSet>,
    blocks: Vec<[[u8; 4]; 64]>,
    identity: u32,
}

/// A lookup's lighting origin and world box, by their bits: the lighter is
/// a function of the two alone (the world's clip map, no movers).
pub(super) type LightKey = ([u32; 3], [u32; 6]);

fn light_key(origin: glam::Vec3, [lo, hi]: [glam::Vec3; 2]) -> LightKey {
    let bits = |v: glam::Vec3| v.to_array().map(f32::to_bits);
    let (lo, hi) = (bits(lo), bits(hi));
    (bits(origin), [lo[0], lo[1], lo[2], hi[0], hi[1], hi[2]])
}

impl Cod4EntityLight {
    pub(super) fn new(
        lighter: vcod_common::mapworld::EntityLighter,
        sun_dir: glam::Vec3,
        sun_colour: glam::Vec3,
        lights: Vec<Option<vcod_common::mapworld::ModelLightSource>>,
    ) -> Self {
        Cod4EntityLight {
            lighter,
            sun_dir,
            sun_colour,
            lights,
            kept: Vec::new(),
            sets: Vec::new(),
            blocks: Vec::new(),
            identity: 0,
        }
    }

    /// The light set of a model lit from `origin` whose world box is
    /// `bounds` (mins, maxs), naming entity volume block `block`, and the
    /// texels that block takes.
    pub(super) fn set(
        &self,
        origin: glam::Vec3,
        bounds: [glam::Vec3; 2],
        block: u32,
        identity_light: f32,
    ) -> (GpuLightSet, [[u8; 4]; 64]) {
        let light = (self.lighter.0)(origin.to_array(), bounds.map(|v| v.to_array()));
        (self.pack(&light, block, identity_light), light.texels)
    }

    fn pack(
        &self,
        light: &vcod_common::mapworld::EntityLight,
        block: u32,
        identity_light: f32,
    ) -> GpuLightSet {
        let source = self
            .lights
            .get(usize::from(light.primary))
            .and_then(Option::as_ref);
        entity_light::pack_cod4(
            light,
            block,
            self.sun_dir,
            self.sun_colour,
            source,
            identity_light,
        )
    }

    /// The frame's set and block per `(origin, box)` ([`Self::sets`],
    /// [`Self::blocks`]), set `i` naming block `i` and past the volume's
    /// blocks the last entity block (the very last is the view model's).
    /// A pair looked up last frame reuses that lookup, and one in the same
    /// place its set and block, so a held scene runs and packs none.
    /// Returns the sets that may differ from last frame's: those outside
    /// it, up to the shorter list's end, hold the same bytes.
    pub(super) fn frame_sets(
        &mut self,
        origins: &[(glam::Vec3, [glam::Vec3; 2])],
        identity_light: f32,
    ) -> std::ops::Range<usize> {
        let last = entity_light::COD4_VOLUME_BLOCKS - 2;
        let repack = identity_light.to_bits() != self.identity;
        self.identity = identity_light.to_bits();
        // by key, built at the first set whose place in the list moved
        let mut by_key: Option<BitsMap<LightKey, usize>> = None;
        let mut fresh = Vec::new();
        for (i, &(o, b)) in origins.iter().enumerate() {
            let key = light_key(o, b);
            let at = match self.kept.get(i) {
                Some((k, _)) if *k == key => {
                    if !repack {
                        continue;
                    }
                    Some(i)
                }
                _ => by_key
                    .get_or_insert_with(|| {
                        self.kept
                            .iter()
                            .enumerate()
                            .map(|(j, (k, _))| (*k, j))
                            .collect()
                    })
                    .get(&key)
                    .copied(),
            };
            let light = match at {
                Some(j) => self.kept[j].1,
                None => (self.lighter.0)(o.to_array(), b.map(|v| v.to_array())),
            };
            fresh.push((i, key, light));
        }
        let n = origins.len();
        self.kept.truncate(n);
        self.sets.truncate(n);
        self.blocks.truncate(n);
        let changed = fresh.first().map_or(n, |f| f.0)..fresh.last().map_or(n, |f| f.0 + 1);
        for (i, key, light) in fresh {
            let set = self.pack(&light, (i as u32).min(last), identity_light);
            if i < self.kept.len() {
                self.kept[i] = (key, light);
                self.sets[i] = set;
                self.blocks[i] = light.texels;
            } else {
                // every set past the old end is fresh, in order
                debug_assert_eq!(i, self.kept.len());
                self.kept.push((key, light));
                self.sets.push(set);
                self.blocks.push(light.texels);
            }
        }
        changed
    }

    /// The last [`Self::frame_sets`]' sets.
    pub(super) fn sets(&self) -> &[GpuLightSet] {
        &self.sets
    }

    /// The last [`Self::frame_sets`]' blocks, by set.
    pub(super) fn blocks(&self) -> &[[[u8; 4]; 64]] {
        &self.blocks
    }
}

/// A buffer's contents as last written, so a frame writes only the span
/// that changed since.
pub(super) struct Written<T>(Vec<T>);

impl<T> Default for Written<T> {
    fn default() -> Self {
        Written(Vec::new())
    }
}

impl<T: bytemuck::Pod> Written<T> {
    /// Writes `new` from offset 0 where it differs from the last write;
    /// returns the bytes written.
    pub(super) fn write(&mut self, queue: &wgpu::Queue, buf: &wgpu::Buffer, new: &[T]) -> usize {
        self.write_within(queue, buf, new, 0..new.len())
    }

    /// [`Self::write`] when only the elements in `maybe` (and any past the
    /// last write's end) can differ from the last write.
    pub(super) fn write_within(
        &mut self,
        queue: &wgpu::Queue,
        buf: &wgpu::Buffer,
        new: &[T],
        maybe: std::ops::Range<usize>,
    ) -> usize {
        let Some(span) = write_span(&self.0, new, maybe) else {
            return 0;
        };
        let bytes: &[u8] = bytemuck::cast_slice(&new[span.clone()]);
        queue.write_buffer(buf, (span.start * size_of::<T>()) as u64, bytes);
        // the span holds every element past the old end
        self.0.resize(new.len(), T::zeroed());
        self.0[span.clone()].copy_from_slice(&new[span]);
        bytes.len()
    }
}

/// `VCOD_WRITE_ALL=1`: the entity lists (light sets, instances, bones,
/// volume blocks) are written whole each frame, not their changed span
/// (cod4-client-perf.md 42.1's A/B).
fn write_all() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("VCOD_WRITE_ALL").is_ok_and(|v| v == "1"))
}

/// What a write of `new` over `old` covers: [`changed_span_within`], or
/// all of `new` under [`write_all`].
pub(crate) fn write_span<T: bytemuck::Pod>(
    old: &[T],
    new: &[T],
    maybe: std::ops::Range<usize>,
) -> Option<std::ops::Range<usize>> {
    if write_all() {
        return (!new.is_empty()).then_some(0..new.len());
    }
    let span = changed_span_within(old, new, maybe);
    #[cfg(test)]
    assert_eq!(
        span,
        changed_span(old, new),
        "an element outside the hint changed"
    );
    span
}

/// The least range of `new` holding every element whose bytes differ from
/// `old`'s or that `old` lacks; `None` when there is none.
pub(crate) fn changed_span<T: bytemuck::Pod>(
    old: &[T],
    new: &[T],
) -> Option<std::ops::Range<usize>> {
    let same = |a: &T, b: &T| bytemuck::bytes_of(a) == bytemuck::bytes_of(b);
    let common = old.len().min(new.len());
    let start = (0..common)
        .find(|&i| !same(&old[i], &new[i]))
        .unwrap_or(common);
    if start == new.len() {
        return None;
    }
    let end = if new.len() > old.len() {
        new.len()
    } else {
        (start..new.len())
            .rev()
            .find(|&i| !same(&old[i], &new[i]))
            .expect("start differs")
            + 1
    };
    Some(start..end)
}

/// [`changed_span`] when the elements both hold outside `maybe` are known
/// to be the same: only `maybe` is compared.
pub(crate) fn changed_span_within<T: bytemuck::Pod>(
    old: &[T],
    new: &[T],
    maybe: std::ops::Range<usize>,
) -> Option<std::ops::Range<usize>> {
    let a = maybe.start.min(old.len()).min(new.len());
    let b = if new.len() > old.len() {
        new.len()
    } else {
        maybe.end.clamp(a, new.len())
    };
    let span = changed_span(&old[a..b.min(old.len())], &new[a..b])?;
    Some(span.start + a..span.end + a)
}

/// FxHash's step over a key's 32-bit words: the light picks key a few
/// hundred float triples a frame, where SipHash's cost shows.
#[derive(Default)]
pub(super) struct BitsHasher(u64);

impl BitsHasher {
    fn add(&mut self, w: u64) {
        self.0 = (self.0.rotate_left(5) ^ w).wrapping_mul(0x517c_c1b7_2722_0a95);
    }
}

impl std::hash::Hasher for BitsHasher {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        for c in bytes.chunks(4) {
            let mut w = [0u8; 4];
            w[..c.len()].copy_from_slice(c);
            self.add(u64::from(u32::from_le_bytes(w)));
        }
    }

    fn write_u32(&mut self, i: u32) {
        self.add(u64::from(i));
    }

    fn write_usize(&mut self, i: usize) {
        self.add(i as u64);
    }
}

/// A map keyed by float bits, hashed by [`BitsHasher`].
pub(super) type BitsMap<K, V> = HashMap<K, V, std::hash::BuildHasherDefault<BitsHasher>>;

/// Live snapshot entities: xmodels uploaded once with bind pose baked in,
/// drawn with per-instance transforms streamed through one instance-step
/// vertex buffer.
pub(super) struct DynamicPass {
    pub(super) instance_buf: wgpu::Buffer,
    pub(super) bone_layout: wgpu::BindGroupLayout,
    /// Identity block first, then each skinned instance's bone set. Sized
    /// for every instance at the cap.
    pub(super) bone_buf: wgpu::Buffer,
    pub(super) bone_bg: wgpu::BindGroup,
    /// Indexed by [`ModelHandle`]: each model's LOD table; grows until
    /// [`Renderer::unload_world`] clears it.
    pub(super) handles: Vec<DynHandle>,
    /// Bumped by each [`Renderer::unload_world`], so the old map's handles
    /// no longer index `handles`.
    pub(super) epoch: u32,
    /// One per uploaded LOD and per hidden-part view of one; never shrinks.
    pub(super) models: Vec<DynModel>,
    /// LODs whose buffers and skins went to the GPU.
    pub(super) uploads: usize,
    /// This frame's instances before the LOD pick, from `set_dynamic_models`.
    pub(super) placed: Vec<Placed>,
    /// `(models index, packed instance)` after the LOD pick; position here
    /// is the slot in `instance_buf`.
    pub(super) instances: Vec<(usize, InstanceRaw)>,
    /// Per entry of `instances`, its `Placed::src`.
    pub(super) instance_src: Vec<usize>,
    /// Last pick: instances per LOD, instances past their last LOD, and the
    /// triangles drawn.
    pub(super) lod_counts: LodCounts,
    /// Written to `bone_buf` in [`Renderer::render`].
    pub(super) bone_mats: Vec<[f32; 16]>,
    /// Per instance, its index in `placed`.
    pub(super) instance_placed: Vec<usize>,
    /// Per entry of `placed`, its posed boxes, kept while its model and
    /// transform hold.
    pub(super) placed_boxes: PlacedBoxes,
    /// The instances' entity draws in draw order, before the view cull.
    pub(super) ent_sorted: EntSorted,
    /// `EntityMaterials::order_epoch` `ent_sorted` was sorted under.
    pub(super) ent_sorted_epoch: u32,
    /// Per instance, where its lights are picked.
    pub(super) light_origins: Vec<glam::Vec3>,
    /// CoD4: each instance's posed world box (mins, maxs), as
    /// `light_origins`.
    pub(super) light_boxes: Vec<(glam::Vec3, glam::Vec3)>,
    /// One `GpuLightSet` per distinct lighting origin this frame.
    pub(super) light_buf: wgpu::Buffer,
    /// What `light_buf`, `instance_buf` and `bone_buf` hold.
    pub(super) light_written: Written<GpuLightSet>,
    pub(super) instance_written: Written<InstanceRaw>,
    pub(super) bone_written: Written<[f32; 16]>,
    /// The map's light grid; `None` with no map.
    pub(super) lighting: Option<StaticLighting>,
    /// A CoD2 map's light grid, which lights its models in place of
    /// `lighting`.
    pub(super) cod2_light: Option<crate::cod2_light::Cod2Lighting>,
    /// A CoD4 map's light grid lookup and sun, which light its entities
    /// and view model through their own model lighting blocks
    /// (cod4-world.md 3.3).
    pub(super) cod4_light: Option<Cod4EntityLight>,
    /// This frame's fx lights, for the entity pick.
    pub(super) scene_lights: Vec<SceneLight>,
    /// The cgame's own scene lights this frame (`constantLight`,
    /// `projectileDLight`), queued after the fx lights.
    pub(super) entity_scene_lights: Vec<SceneLight>,
    pub(super) materials: EntityMaterials,
    /// Each skin's image bind group, uploaded once a map for every LOD and
    /// model that binds it; [`Renderer::unload_world`] clears it.
    pub(super) skin_groups: HashMap<String, wgpu::BindGroup>,
}

/// A dynamic model and, per drawn surface, its material.
pub(super) struct DynModel {
    pub(super) vm: VmModel,
    pub(super) skins: Vec<Rc<EntSkin>>,
    /// Some surface takes the light grid; an unlit model skips the pick.
    pub(super) lit: bool,
    pub(super) tris: usize,
}

/// An uploaded xmodel: `Model::lod_dists`, and per LOD its `DynModel`
/// (`None` when that LOD has nothing to draw).
pub(super) struct DynHandle {
    pub(super) dists: Vec<f32>,
    pub(super) lods: Vec<Option<usize>>,
    /// Per bone LOD 0 weights any vertex to, the box of those vertices in
    /// model space: posed by the bone's skin matrix, their union bounds
    /// the posed mesh (`cod2_cookie::Candidate`).
    pub(super) bone_boxes: Vec<(u8, glam::Vec3, glam::Vec3)>,
    /// The same over every LOD's vertices: what the view cull poses
    /// ([`instance_seen`]), whichever LOD draws.
    pub(super) view_boxes: Vec<(u8, glam::Vec3, glam::Vec3)>,
}

/// One `DynamicModelInstance`, packed, before its LOD is picked.
pub(super) struct Placed {
    pub(super) handle: usize,
    /// Its index in the slice `set_dynamic_models` was handed.
    pub(super) src: usize,
    pub(super) raw: InstanceRaw,
    pub(super) lod_origin: glam::Vec3,
    pub(super) light_origin: glam::Vec3,
    pub(super) shader_time: Option<f32>,
}

/// Per entry of `DynamicPass::placed`, its posed world box ([`posed_box`],
/// CoD4 only) and its view box ([`instance_seen`]), with the model and
/// transform they were posed from. An unskinned entry whose two hold
/// keeps its boxes: they read no other instance's bones.
#[derive(Default)]
pub(super) struct PlacedBoxes {
    /// Whether `light` was posed (a CoD4 map).
    cod4: bool,
    boxes: Vec<PlacedBox>,
}

struct PlacedBox {
    handle: usize,
    transform: [u32; 16],
    light: Option<(glam::Vec3, glam::Vec3)>,
    /// Grown as [`instance_seen`] grows it.
    view: Option<(glam::Vec3, glam::Vec3)>,
}

impl PlacedBoxes {
    /// Poses each of `d.placed` that did not hold since the last call.
    fn update(d: &mut DynamicPass) {
        let cod4 = d.cod4_light.is_some();
        let mut boxes = std::mem::take(&mut d.placed_boxes);
        if boxes.cod4 != cod4 {
            boxes = PlacedBoxes {
                cod4,
                boxes: Vec::new(),
            };
        }
        boxes.boxes.truncate(d.placed.len());
        for (i, p) in d.placed.iter().enumerate() {
            let transform = p.raw.transform.map(f32::to_bits);
            let h = &d.handles[p.handle];
            let rigid = p.raw.bone_base == 0
                && [&h.bone_boxes, &h.view_boxes].iter().all(|b| {
                    b.last()
                        .is_none_or(|b| usize::from(b.0) < MAX_INSTANCE_BONES)
                });
            let held = boxes
                .boxes
                .get(i)
                .filter(|b| rigid && b.handle == p.handle && b.transform == transform);
            if held.is_some() && !cfg!(test) {
                continue;
            }
            let view = posed_box_of(d, p, &h.view_boxes).map(|(lo, hi)| {
                let origin = glam::Vec3::from_slice(&p.raw.transform[12..15]);
                let reach = (lo - origin).abs().max((hi - origin).abs()).max_element();
                let grow = glam::Vec3::splat(1.0 + reach * 0.01);
                (lo - grow, hi + grow)
            });
            let b = PlacedBox {
                handle: p.handle,
                transform,
                light: if cod4 { posed_box(d, p) } else { None },
                view,
            };
            #[cfg(test)]
            if let Some(h) = held {
                let bits = |b: Option<(glam::Vec3, glam::Vec3)>| {
                    b.map(|(lo, hi)| [lo.to_array(), hi.to_array()].map(|v| v.map(f32::to_bits)))
                };
                assert!(
                    bits(h.light) == bits(b.light) && bits(h.view) == bits(b.view),
                    "kept box {i}"
                );
            }
            match boxes.boxes.get_mut(i) {
                Some(old) => *old = b,
                None => boxes.boxes.push(b),
            }
        }
        d.placed_boxes = boxes;
    }
}

/// F3's `lod` counts for the dynamic models.
#[derive(Clone, Copy, Default)]
pub struct LodCounts {
    pub per_lod: [usize; 3],
    pub gone: usize,
    pub tris: usize,
}

impl Renderer {
    /// Dynamic model LODs uploaded to the GPU so far.
    #[cfg(test)]
    pub(crate) fn dynamic_uploads(&self) -> usize {
        self.dynamic.uploads
    }

    /// The cgame's scene lights for this frame (`entities::BuiltScene`);
    /// entity picks see them after the fx lights.
    pub fn set_entity_lights(&mut self, lights: Vec<SceneLight>) {
        self.dynamic.entity_scene_lights = lights;
    }

    /// Slice order is draw order (hands, then the gun). Replaces anything set before.
    pub fn set_viewmodel(&mut self, fs: &Pk3Fs, models: &[model::Model]) {
        let before = self.vm_pass.cache.len();
        self.prefetch_skins(fs, models.iter().flat_map(|m| m.materials()));
        let uploaded: Vec<_> = models
            .iter()
            .filter_map(|m| self.vm_upload(fs, m))
            .collect();

        let surfaces: usize = uploaded.iter().map(|m| m.surfaces.len()).sum();
        log::debug!(
            "viewmodel: {} models, {surfaces} drawn surfaces",
            uploaded.len()
        );
        self.vm_pass.models = uploaded;
        self.resolve_viewmodel_skins(fs);
        let after = self.vm_pass.cache.len();
        self.vm_pass.first_uses += after - before;
    }

    /// Parts uploaded by `set_viewmodel` itself so far.
    #[cfg(test)]
    pub(crate) fn viewmodel_first_uses(&self) -> usize {
        self.vm_pass.first_uses
    }

    /// The drawn viewmodel's materials, from this map's entity materials.
    pub(super) fn resolve_viewmodel_skins(&mut self, fs: &Pk3Fs) {
        let gamma = self.image_gamma;
        self.vm_pass.skins = self
            .vm_pass
            .models
            .iter()
            .map(|m| {
                m.skins
                    .iter()
                    .map(|skin| {
                        self.dynamic.materials.skin(
                            &self.device,
                            &self.queue,
                            (fs, &self.model_images),
                            &self.shader_lib,
                            gamma.as_ref(),
                            skin,
                        )
                    })
                    .collect()
            })
            .collect();
    }

    /// CoD2 model `skins`' images and their stage images decoded on every
    /// core into [`Self::model_images`], ahead of the uploads that take them.
    pub(super) fn prefetch_skins<'s>(&self, fs: &Pk3Fs, skins: impl Iterator<Item = &'s String>) {
        if fs.game() != vcod_common::game::Game::Cod2 {
            return;
        }
        let lib = &self.shader_lib;
        let mats = skins.filter_map(|skin| lib.get(skin));
        self.model_images.prefetch(fs, prefetch_paths(fs, mats));
    }

    /// Builds the entity and view model pipelines of `keys` now, so no
    /// model's first upload builds one (cod2-client-perf.md, "The sd
    /// level's first frame").
    pub(crate) fn prewarm_entity_pipelines(&mut self, keys: &[EntPipelineKey]) {
        self.dynamic.materials.prewarm(&self.device, keys);
    }

    /// Entity and view model pipeline pairs built so far.
    #[cfg(test)]
    pub(crate) fn entity_pipelines(&self) -> usize {
        self.dynamic.materials.pipeline_count()
    }

    /// Uploads viewmodel parts into the cache without drawing them, so a
    /// later `set_viewmodel` of the same parts uploads nothing.
    pub fn preload_viewmodel(&mut self, fs: &Pk3Fs, models: &[model::Model]) {
        let fresh: Vec<&model::Model> = models
            .iter()
            .filter(|m| !self.vm_pass.cache.contains_key(&vm_cache_key(m)))
            .collect();
        self.prepare_skins(
            fs,
            &drawn_skins(fresh.iter().map(|m| (m.surfaces(), m.materials()))),
        );
        for m in models {
            self.vm_upload(fs, m);
        }
        self.prepared_skins.clear();
        self.model_images.clear();
    }

    /// `models` uploaded as [`Self::upload_dynamic_model`] uploads each,
    /// in order, with their skins' entity pipelines built and their images
    /// decoded (and on CoD1 mipped) on every core first: a level's
    /// prewarm (cod4-client-perf.md 32).
    pub fn upload_dynamic_models(
        &mut self,
        fs: &Pk3Fs,
        models: &[&model::Model],
    ) -> Vec<Option<ModelHandle>> {
        // the skins the uploads will read: every LOD of a model whose LOD 0
        // draws, as `upload_dynamic_model` stops at an empty LOD 0
        let meshes = models
            .iter()
            .filter(|m| {
                m.lod_mesh(0).is_some_and(|(s, _)| {
                    s.iter()
                        .any(|s| !s.verts.is_empty() && !s.indices.is_empty())
                })
            })
            .flat_map(|m| (0..m.lods.len()).filter_map(|i| m.lod_mesh(i)));
        let skins = drawn_skins(meshes);
        let keys = self
            .dynamic
            .materials
            .skin_pipeline_keys(fs, &self.shader_lib, &skins);
        self.dynamic.materials.prewarm(&self.device, &keys);
        self.prepare_skins(fs, &skins);
        let out = models
            .iter()
            .map(|m| self.upload_dynamic_model(fs, m))
            .collect();
        self.prepared_skins.clear();
        self.model_images.clear();
        out
    }

    /// `skins` decoded on every core ahead of a batch of uploads: CoD2's
    /// and CoD4's colour, normal and specular maps into
    /// [`Self::model_images`], CoD1's skin images decoded and mipped into
    /// [`Self::prepared_skins`].
    pub(super) fn prepare_skins(&mut self, fs: &Pk3Fs, skins: &[String]) {
        if fs.game() == vcod_common::game::Game::Cod2 {
            self.prefetch_skins(fs, skins.iter());
            return;
        }
        // a skin no stage of reads its own image is left to its draw's
        // white group (`upload_dyn_lod`)
        let mats = &mut self.dynamic.materials;
        let fresh: Vec<&String> = skins
            .iter()
            .filter(|s| !self.prepared_skins.contains_key(s.as_str()))
            .filter(|s| mats.may_read_skin_image(fs, &self.shader_lib, s))
            .collect();
        let (lib, images) = (&self.shader_lib, &self.model_images);
        let (textures, gamma) = (self.textures, self.image_gamma);
        let prepared = vcod_common::laps::par_map(&fresh, |skin| {
            prepare_skin(fs, lib, images, &textures, gamma.as_ref(), skin)
        });
        for (skin, p) in fresh.into_iter().zip(prepared) {
            self.prepared_skins.insert(skin.clone(), p);
        }
    }

    /// A model skin's image as an upload binds it: the batch's when
    /// [`Self::prepare_skins`] made it, else decoded and mipped now.
    pub(super) fn skin_image(&self, fs: &Pk3Fs, skin: &str) -> PreparedImage {
        match self.prepared_skins.get(skin) {
            Some(p) => p.clone(),
            None => prepare_skin(
                fs,
                &self.shader_lib,
                &self.model_images,
                &self.textures,
                self.image_gamma.as_ref(),
                skin,
            ),
        }
    }

    /// A viewmodel part's upload, out of the cache after the first time.
    pub(super) fn vm_upload(&mut self, fs: &Pk3Fs, m: &model::Model) -> Option<Rc<VmModel>> {
        let key = vm_cache_key(m);
        if let Some(hit) = self.vm_pass.cache.get(&key) {
            return Some(hit.clone());
        }
        let Some(model) = upload_vm_model(
            &self.device,
            &self.vm_pass,
            m.surfaces(),
            m.materials(),
            true,
            &|_| None,
            &mut |skin| {
                let (img, levels) = self.skin_image(fs, skin);
                let view = upload_levels(&self.device, &self.queue, skin, &img, &levels);
                vm_skin_group(&self.device, &self.vm_pass, &view)
            },
        ) else {
            log::warn!("viewmodel {}: no drawable surfaces, skipping it", m.name);
            return None;
        };
        let model = Rc::new(model);
        self.vm_pass.cache.insert(key, model.clone());
        Some(model)
    }

    /// Same upload as the viewmodel (bind pose baked into the mesh), every
    /// LOD; a model whose LOD 0 is empty returns `None`.
    pub fn upload_dynamic_model(
        &mut self,
        fs: &Pk3Fs,
        model: &model::Model,
    ) -> Option<ModelHandle> {
        // every LOD's skins and their stage images, decoded on every core
        self.prefetch_skins(
            fs,
            (0..model.lods.len())
                .filter_map(|i| model.lod_mesh(i))
                .flat_map(|(_, materials)| materials.iter()),
        );
        let mut lods = Vec::with_capacity(model.lods.len());
        for i in 0..model.lods.len() {
            let uploaded = model
                .lod_mesh(i)
                .and_then(|(surfaces, materials)| self.upload_dyn_lod(fs, surfaces, materials));
            if i == 0 && uploaded.is_none() {
                return None;
            }
            lods.push(uploaded);
        }
        self.dynamic.handles.push(DynHandle {
            dists: model.lod_dists(),
            lods,
            bone_boxes: bone_boxes(model),
            view_boxes: bone_boxes_of(model.lods.iter().flat_map(|l| &l.surfaces)),
        });
        Some(ModelHandle {
            index: self.dynamic.handles.len() - 1,
            epoch: self.dynamic.epoch,
        })
    }

    /// `h`, uploaded from `model`, drawn without the LOD 0 surfaces whose
    /// `keep` entry (by index into `model`'s LOD 0 surfaces) is false: a
    /// new handle onto the same buffers, skins and materials, so nothing
    /// is uploaded (cod4-client-perf.md 18). Its other LODs are `h`'s.
    /// `None` when `h` is stale or nothing is kept.
    pub fn dynamic_model_hiding(
        &mut self,
        h: ModelHandle,
        model: &model::Model,
        keep: &[bool],
    ) -> Option<ModelHandle> {
        let d = &mut self.dynamic;
        let full = &d.handles[h.live(d.epoch, d.handles.len())?];
        let lod0 = &d.models[(*full.lods.first()?)?];
        let shown = kept_surfaces(&lod0.vm.surfaces, keep);
        if shown.is_empty() {
            return None;
        }
        let skins: Vec<_> = shown.iter().map(|&i| lod0.skins[i].clone()).collect();
        let vm = VmModel {
            skins: shown.iter().map(|&i| lod0.vm.skins[i].clone()).collect(),
            surfaces: shown.iter().map(|&i| lod0.vm.surfaces[i].clone()).collect(),
            ..lod0.vm.clone()
        };
        let lit = skins.iter().any(|s| s.lit);
        let tris = vm.surfaces.iter().map(|s| s.index_count as usize / 3).sum();
        let mut lods = full.lods.clone();
        let dists = full.dists.clone();
        let lod0_surfaces = model.lods.first().map_or(&[][..], |l| &l.surfaces[..]);
        let bone_boxes = bone_boxes_of(
            lod0_surfaces
                .iter()
                .enumerate()
                .filter(|(i, _)| keep.get(*i).copied().unwrap_or(true))
                .map(|(_, s)| s),
        );
        let view_boxes = full.view_boxes.clone();
        d.models.push(DynModel {
            vm,
            skins,
            lit,
            tris,
        });
        lods[0] = Some(d.models.len() - 1);
        d.handles.push(DynHandle {
            dists,
            lods,
            bone_boxes,
            view_boxes,
        });
        Some(ModelHandle {
            index: d.handles.len() - 1,
            epoch: d.epoch,
        })
    }

    pub(super) fn upload_dyn_lod(
        &mut self,
        fs: &Pk3Fs,
        surfaces: &[model::Surface],
        materials: &[String],
    ) -> Option<usize> {
        let gamma = self.image_gamma;
        let cod2 = fs.game() == vcod_common::game::Game::Cod2;
        // Stages bind their images by path: a skin image only a
        // `$texturename` stage or the fallback reads uploads, the others
        // bind white. Built in surface order, as the upload's loop
        // below reads them, so each skin's load order holds.
        for skin in drawn_skins(std::iter::once((surfaces, materials))) {
            if self.dynamic.skin_groups.contains_key(&skin) {
                continue;
            }
            let ent = self.dynamic.materials.skin(
                &self.device,
                &self.queue,
                (fs, &self.model_images),
                &self.shader_lib,
                gamma.as_ref(),
                &skin,
            );
            let g = match ent.reads_skin_image() {
                true => {
                    let (img, levels) = self.skin_image(fs, &skin);
                    let view = upload_levels(&self.device, &self.queue, &skin, &img, &levels);
                    vm_skin_group(&self.device, &self.vm_pass, &view)
                }
                false => {
                    // CoD2's pak read of the colour map is marked; CoD1's
                    // retail never opens a scripted skin's own image
                    if cod2 {
                        super::prop_skin::mark_colour_map(fs, self.shader_lib.get(&skin));
                    }
                    self.dynamic.materials.images[0].clone()
                }
            };
            self.dynamic.skin_groups.insert(skin, g);
        }
        let sprites: HashMap<String, Sprite> = drawn_skins(std::iter::once((surfaces, materials)))
            .into_iter()
            .filter_map(|skin| {
                let sprite = self.dynamic.materials.sprite(&skin)?;
                Some((skin, sprite))
            })
            .collect();
        let mut groups = std::mem::take(&mut self.dynamic.skin_groups);
        let uploaded = upload_vm_model(
            &self.device,
            &self.vm_pass,
            surfaces,
            materials,
            false,
            &|skin| sprites.get(skin).copied(),
            &mut |skin| {
                if let Some(g) = groups.get(skin) {
                    return g.clone();
                }
                let (img, levels) = self.skin_image(fs, skin);
                let view = upload_levels(&self.device, &self.queue, skin, &img, &levels);
                let g = vm_skin_group(&self.device, &self.vm_pass, &view);
                groups.insert(skin.to_string(), g.clone());
                g
            },
        );
        self.dynamic.skin_groups = groups;
        let uploaded = uploaded?;
        let skins: Vec<_> = uploaded
            .skins
            .iter()
            .map(|skin| {
                self.dynamic.materials.skin(
                    &self.device,
                    &self.queue,
                    (fs, &self.model_images),
                    &self.shader_lib,
                    gamma.as_ref(),
                    skin,
                )
            })
            .collect();
        let lit = skins.iter().any(|s| s.lit);
        let tris = uploaded
            .surfaces
            .iter()
            .map(|s| s.index_count as usize / 3)
            .sum();
        self.dynamic.uploads += 1;
        self.dynamic.models.push(DynModel {
            vm: uploaded,
            skins,
            lit,
            tris,
        });
        Some(self.dynamic.models.len() - 1)
    }

    /// Instances past `MAX_DYNAMIC_INSTANCES`, and skinned ones past
    /// `MAX_SKINNED_INSTANCES`, are dropped (warned once); a stale handle is
    /// skipped. Written to the GPU in [`Self::render`].
    pub fn set_dynamic_models(&mut self, instances: &[DynamicModelInstance]) {
        static OVERFLOW_WARNED: std::sync::Once = std::sync::Once::new();
        static SKINNED_WARNED: std::sync::Once = std::sync::Once::new();

        let mut handles = Vec::with_capacity(instances.len());
        let mut items = Vec::with_capacity(instances.len());
        let mut skinned = 0;
        for (src, inst) in instances.iter().enumerate() {
            if handles.len() == MAX_DYNAMIC_INSTANCES {
                OVERFLOW_WARNED.call_once(|| {
                    log::warn!(
                        "more than {MAX_DYNAMIC_INSTANCES} live entities this frame; dropping the rest"
                    );
                });
                break;
            }
            let Some(idx) = inst
                .model
                .live(self.dynamic.epoch, self.dynamic.handles.len())
            else {
                continue;
            };
            if inst.bones.is_some() {
                if skinned == MAX_SKINNED_INSTANCES {
                    SKINNED_WARNED.call_once(|| {
                        log::warn!(
                            "more than {MAX_SKINNED_INSTANCES} skinned entities this frame; dropping the rest"
                        );
                    });
                    continue;
                }
                skinned += 1;
            }
            handles.push((src, inst));
            items.push((idx, inst.transform.to_cols_array(), inst.bones.as_deref()));
        }
        let (raw, bone_mats) = pack_instances(&items);
        self.dynamic.placed = handles
            .into_iter()
            .zip(raw)
            .map(|((src, inst), raw)| Placed {
                handle: inst.model.index,
                src,
                raw,
                lod_origin: inst.transform.w_axis.truncate(),
                light_origin: inst.light_origin,
                shader_time: inst.shader_time,
            })
            .collect();
        self.dynamic.bone_mats = bone_mats;
    }

    /// Picks each placed instance's LOD from the view; one past its last LOD
    /// is not drawn (docs/research/xmodel-v14-format.md, "LOD selection").
    /// Entities carry no refEntity scale. `time` is the frame's unwrapped
    /// clock, which an instance's own `shader_time` is taken against: the
    /// offset slides a stage uv whose phase is the unwrapped time's, and
    /// [`gpu_time`]'s wrap is a whole number of every shader cycle.
    pub(super) fn pick_dynamic_lods(&mut self, eye: glam::Vec3, view: &model::LodView, time: f32) {
        let d = &mut self.dynamic;
        d.instances.clear();
        d.instance_src.clear();
        d.instance_placed.clear();
        d.light_origins.clear();
        d.light_boxes.clear();
        // CoD4: each instance's posed world box picks its spot or omni light
        PlacedBoxes::update(d);
        let boxes = &d.placed_boxes;
        let mut counts = LodCounts::default();
        for (i, p) in d.placed.iter().enumerate() {
            let h = &d.handles[p.handle];
            let picked = model::select_lod(&h.dists, view.distance(eye, p.lod_origin, 0.0));
            let Some(lod) = picked else {
                counts.gone += 1;
                continue;
            };
            if let Some(n) = counts.per_lod.get_mut(lod) {
                *n += 1;
            }
            let Some(m) = h.lods[lod] else {
                continue;
            };
            counts.tris += d.models[m].tris;
            let mut raw = p.raw;
            raw.time_offset = p.shader_time.map_or(0.0, |t| t - time);
            d.instances.push((m, raw));
            d.instance_src.push(p.src);
            d.instance_placed.push(i);
            d.light_origins.push(p.light_origin);
            if boxes.cod4 {
                d.light_boxes.push(
                    boxes.boxes[i]
                        .light
                        .unwrap_or((p.light_origin, p.light_origin)),
                );
            }
        }
        d.lod_counts = counts;
    }

    /// Picks each instance's lights (one pick per distinct origin, so a
    /// player's parts share one), points its `light_set` at them and writes
    /// the sets that changed; returns the bytes written.
    pub(super) fn write_entity_lights(&mut self) -> usize {
        let d = &mut self.dynamic;
        if let Some(c4) = &mut d.cod4_light {
            // CoD4: a lit model's set is its own block, one per distinct
            // lighting origin as `R_AllocModelLighting`'s cache keeps, over
            // the box its instances span (a scene entity's)
            let n = d.instances.len();
            let mut origins: Vec<(glam::Vec3, [glam::Vec3; 2])> = Vec::with_capacity(n);
            let mut seen: BitsMap<[u32; 3], u32> =
                BitsMap::with_capacity_and_hasher(n, Default::default());
            let ents = d
                .instances
                .iter_mut()
                .zip(&d.light_origins)
                .zip(&d.light_boxes);
            for (((model, raw), origin), &(lo, hi)) in ents {
                if !d.models[*model].lit {
                    raw.light_set = 0;
                    continue;
                }
                let key = origin.to_array().map(f32::to_bits);
                raw.light_set = *seen.entry(key).or_insert_with(|| {
                    origins.push((*origin, [lo, hi]));
                    origins.len() as u32 - 1
                });
                let b = &mut origins[raw.light_set as usize].1;
                *b = [b[0].min(lo), b[1].max(hi)];
            }
            let changed = c4.frame_sets(&origins, self.identity_light);
            d.materials
                .cod4_volume
                .write_blocks(&self.queue, c4.blocks(), changed.clone());
            return match c4.sets() {
                [] => d
                    .light_written
                    .write(&self.queue, &d.light_buf, &[GpuLightSet::default()]),
                sets => d
                    .light_written
                    .write_within(&self.queue, &d.light_buf, sets, changed),
            };
        }
        let mut sets = Vec::new();
        let mut seen: BitsMap<[u32; 3], u32> =
            BitsMap::with_capacity_and_hasher(d.instances.len(), Default::default());
        if let Some(c2) = &d.cod2_light {
            // CoD2: a lit model's light set is its grid sample
            for ((model, raw), origin) in d.instances.iter_mut().zip(&d.light_origins) {
                if !d.models[*model].lit {
                    raw.light_set = 0;
                    continue;
                }
                let key = origin.to_array().map(f32::to_bits);
                raw.light_set = *seen.entry(key).or_insert_with(|| {
                    sets.push(c2.entity_set(*origin, self.identity_light));
                    sets.len() as u32 - 1
                });
            }
        } else if let Some(lighting) = d.lighting.as_mut() {
            let scene =
                model_scene_lights(&d.scene_lights, &d.entity_scene_lights, self.dynamic_light);
            for ((model, raw), origin) in d.instances.iter_mut().zip(&d.light_origins) {
                // retail picks only for a model with a lit surface (0x50e3e0)
                if !d.models[*model].lit {
                    raw.light_set = 0;
                    continue;
                }
                let key = origin.to_array().map(f32::to_bits);
                raw.light_set = *seen.entry(key).or_insert_with(|| {
                    let l = lighting.entity_lights(*origin, &scene);
                    sets.push(entity_light::pack(&l, None, self.identity_light));
                    sets.len() as u32 - 1
                });
            }
        }
        if sets.is_empty() {
            sets.push(GpuLightSet::default());
        }
        d.light_written.write(&self.queue, &d.light_buf, &sets)
    }
}

/// The viewmodel's uniform, skin and bone layouts and buffers; its
/// pipelines are the entity materials' `EntPass::View` ones, built with the
/// dynamic pass.
pub(super) fn create_vm_pass(device: &wgpu::Device, tex: &TexSamplers) -> VmPass {
    let [s0, s1, s2, s3, s4, s5, s6, s7] = TexSamplers::layout_entries();
    let uniform_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("viewmodel uniform layout"),
        entries: &[
            gpu_util::uniform_sized(
                0,
                wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                false,
                VM_UNIFORM_SIZE,
            ),
            s0,
            s1,
            s2,
            s3,
            s4,
            s5,
            s6,
            s7,
        ],
    });
    let skin_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("viewmodel skin layout"),
        entries: &[gpu_util::texture_2d(0)],
    });
    let bone_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("viewmodel bone layout"),
        entries: &[gpu_util::uniform_sized(
            0,
            wgpu::ShaderStages::VERTEX,
            false,
            VM_BONE_BUF_SIZE,
        )],
    });
    let uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("viewmodel uniform"),
        size: VM_UNIFORM_SIZE,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let uniform_bg = vm_uniform_group(device, &uniform_layout, &uniform_buf, tex);

    VmPass {
        uniform_layout,
        uniform_buf,
        uniform_bg,
        skin_layout,
        bone_layout,
        models: Vec::new(),
        skins: Vec::new(),
        cache: HashMap::new(),
        first_uses: 0,
    }
}

/// The viewmodel's group 0: its uniform and `r_textureMode`'s samplers.
pub(super) fn vm_uniform_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    buf: &wgpu::Buffer,
    tex: &TexSamplers,
) -> wgpu::BindGroup {
    let [s0, s1, s2, s3, s4, s5, s6, s7] = tex.entries();
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("viewmodel uniform bind group"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: buf.as_entire_binding(),
            },
            s0,
            s1,
            s2,
            s3,
            s4,
            s5,
            s6,
            s7,
        ],
    })
}

/// [`DynHandle::bone_boxes`] of `model`'s LOD 0.
pub(super) fn bone_boxes(model: &model::Model) -> Vec<(u8, glam::Vec3, glam::Vec3)> {
    bone_boxes_of(model.lods.first().into_iter().flat_map(|l| &l.surfaces))
}

/// [`bone_boxes`] over `surfaces`' vertices.
pub(super) fn bone_boxes_of<'a>(
    surfaces: impl Iterator<Item = &'a model::Surface>,
) -> Vec<(u8, glam::Vec3, glam::Vec3)> {
    let mut boxes: HashMap<u8, (glam::Vec3, glam::Vec3)> = HashMap::new();
    for v in surfaces.flat_map(|s| &s.verts) {
        let p = glam::Vec3::from(v.pos);
        for (&b, &w) in v.bone_indices.iter().zip(&v.bone_weights) {
            if w > 0.0 {
                let e = boxes.entry(b).or_insert((p, p));
                *e = (e.0.min(p), e.1.max(p));
            }
        }
    }
    let mut out: Vec<_> = boxes.into_iter().map(|(b, (lo, hi))| (b, lo, hi)).collect();
    out.sort_by_key(|e| e.0);
    out
}

/// The world box of placed instance `p` posed by its bone matrices
/// (`DynamicPass::bone_mats` from its `bone_base`), `None` for a mesh
/// with no vertex.
pub(super) fn posed_box(d: &DynamicPass, p: &Placed) -> Option<(glam::Vec3, glam::Vec3)> {
    posed_box_of(d, p, &d.handles[p.handle].bone_boxes)
}

/// [`posed_box`] over `boxes`, a handle's bone boxes.
pub(super) fn posed_box_of(
    d: &DynamicPass,
    p: &Placed,
    boxes: &[(u8, glam::Vec3, glam::Vec3)],
) -> Option<(glam::Vec3, glam::Vec3)> {
    let transform = glam::Mat4::from_cols_array(&p.raw.transform);
    let mut out: Option<(glam::Vec3, glam::Vec3)> = None;
    for &(b, lo, hi) in boxes {
        let skin = d
            .bone_mats
            .get(p.raw.bone_base as usize + usize::from(b))
            .map_or(glam::Mat4::IDENTITY, glam::Mat4::from_cols_array);
        let m = transform * skin;
        for c in 0..8 {
            let corner = glam::Vec3::new(
                if c & 1 == 0 { lo.x } else { hi.x },
                if c & 2 == 0 { lo.y } else { hi.y },
                if c & 4 == 0 { lo.z } else { hi.z },
            );
            let w = m.transform_point3(corner);
            out = Some(out.map_or((w, w), |(a, z)| (a.min(w), z.max(w))));
        }
    }
    out
}

/// Whether instance `k` can reach the view `frustum` cuts: its posed box
/// over every LOD (`DynHandle::view_boxes`), grown by a unit and a
/// hundredth of its reach from the instance's origin (a skinned vertex
/// whose weights sum short of 1 is pulled toward it), meets the frustum.
/// An instance with a stage whose vertex shader moves its vertices (a
/// deform, a flag) or without a box is always seen.
pub(super) fn instance_seen(
    d: &DynamicPass,
    k: usize,
    frustum: &vcod_common::vis::Frustum,
) -> bool {
    let (m, _) = d.instances[k];
    if !d.models[m]
        .skins
        .iter()
        .all(|sk| sk.stages.iter().all(|st| st.rigid))
    {
        return true;
    }
    match d.placed_boxes.boxes[d.instance_placed[k]].view {
        Some((lo, hi)) => frustum.intersects_aabb(lo, hi),
        None => true,
    }
}

/// Group 0 is the camera, group 1 the viewmodel skin layout (the uploaded
/// models' bind groups are built against it), group 2 the bone storage
/// buffer, group 3 the stage's `StageParams`. Vertex buffer 1 is
/// instance-step `InstanceRaw`. The entity materials also build the
/// viewmodel's pipelines over `vm`'s layouts.
pub(super) fn create_dynamic_pass(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    target: (wgpu::TextureFormat, u32),
    camera_layout: &wgpu::BindGroupLayout,
    vm: &VmPass,
    white_view: &wgpu::TextureView,
    textures: TextureSettings,
) -> DynamicPass {
    let instance_buf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("dynamic instance transforms"),
        size: MAX_DYNAMIC_INSTANCES as u64 * DYNAMIC_INSTANCE_STRIDE,
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });

    // Worst case: every skinned instance at the bone cap, plus the identity block.
    let storage = gpu_util::storage_ro;
    // Binding 1 holds the frame's light sets; a CoD2 probe-lit surface
    // reads its set in the fragment stage.
    let bone_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("dynamic bone layout"),
        entries: &[
            storage(0, wgpu::ShaderStages::VERTEX),
            storage(1, wgpu::ShaderStages::VERTEX_FRAGMENT),
        ],
    });
    let bone_buf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("dynamic bone matrices"),
        size: (MAX_SKINNED_INSTANCES + 1) as u64 * MAX_INSTANCE_BONES as u64 * 64,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let light_buf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("dynamic light sets"),
        size: MAX_DYNAMIC_INSTANCES as u64 * std::mem::size_of::<GpuLightSet>() as u64,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let bone_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("dynamic bone bind group"),
        layout: &bone_layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: bone_buf.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: light_buf.as_entire_binding(),
            },
        ],
    });

    let materials = EntityMaterials::new(
        device,
        queue,
        target,
        PassLayouts {
            camera: camera_layout,
            bones: &bone_layout,
            view_uniform: &vm.uniform_layout,
            view_bones: &vm.bone_layout,
        },
        &vm.skin_layout,
        textures,
        white_view,
    );

    DynamicPass {
        instance_buf,
        bone_layout,
        bone_buf,
        bone_bg,
        handles: Vec::new(),
        epoch: 0,
        models: Vec::new(),
        uploads: 0,
        placed: Vec::new(),
        instances: Vec::new(),
        instance_src: Vec::new(),
        lod_counts: LodCounts::default(),
        bone_mats: Vec::new(),
        light_origins: Vec::new(),
        light_boxes: Vec::new(),
        instance_placed: Vec::new(),
        light_buf,
        placed_boxes: PlacedBoxes::default(),
        ent_sorted: EntSorted::default(),
        ent_sorted_epoch: 0,
        light_written: Written::default(),
        instance_written: Written::default(),
        bone_written: Written::default(),
        lighting: None,
        cod2_light: None,
        cod4_light: None,
        scene_lights: Vec::new(),
        entity_scene_lights: Vec::new(),
        materials,
        skin_groups: HashMap::new(),
    }
}

/// The projection and motion transform; the light set follows at 128.
pub(super) fn vm_uniform(model: glam::Mat4, fov_x: f32, aspect: f32) -> [f32; 32] {
    let proj = crate::camera::perspective(fov_x, aspect, VM_NEAR, VM_FAR);
    let mut out = [0.0f32; 32];
    out[..16].copy_from_slice(&proj.to_cols_array());
    out[16..32].copy_from_slice(&model.to_cols_array());
    out
}

/// A skin image's group for `VmPass::skin_layout`.
pub(super) fn vm_skin_group(
    device: &wgpu::Device,
    vm: &VmPass,
    view: &wgpu::TextureView,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("viewmodel skin bind group"),
        layout: &vm.skin_layout,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: wgpu::BindingResource::TextureView(view),
        }],
    })
}

/// `None` when nothing is drawable (empty buffers are invalid to create).
/// `bind_skin` gives a skin name's image group, called once per skin the
/// upload binds, in surface order; `bones` makes the view model's own
/// bone buffer. A surface whose skin `sprite` names goes up encoded for
/// `dynamic_model.wgsl`'s sprite step ([`encode_entity_sprite`]).
pub(super) fn upload_vm_model(
    device: &wgpu::Device,
    vm: &VmPass,
    model_surfaces: &[model::Surface],
    materials: &[String],
    bones: bool,
    sprite: &dyn Fn(&str) -> Option<Sprite>,
    bind_skin: &mut dyn FnMut(&str) -> wgpu::BindGroup,
) -> Option<VmModel> {
    let mut verts: Vec<VmVert> = Vec::new();
    let mut indices: Vec<u16> = Vec::new();
    let mut bind_groups = Vec::with_capacity(model_surfaces.len());
    let mut bound_skins: HashMap<&str, usize> = HashMap::new();
    let mut surfaces = Vec::with_capacity(model_surfaces.len());
    let mut skins = Vec::with_capacity(model_surfaces.len());
    let drawn = model_surfaces
        .iter()
        .enumerate()
        .filter(|(_, s)| !s.verts.is_empty() && !s.indices.is_empty());
    let rebase = drawn.clone().map(|(_, s)| s.verts.len()).sum::<usize>() <= 1 << 16;
    for (src, surf) in drawn {
        let skin = materials
            .get(surf.material)
            .map(String::as_str)
            .unwrap_or_default();
        let bind_group = *bound_skins.entry(skin).or_insert_with(|| {
            bind_groups.push(bind_skin(skin));
            bind_groups.len() - 1
        });
        skins.push(skin.to_string());
        let base = verts.len();
        let mut surf_verts = surf.verts.clone();
        let mut surf_indices = surf.indices.clone();
        if let Some(k) = sprite(skin) {
            encode_entity_sprite(skin, k, &mut surf_verts, &mut surf_indices);
        }
        surfaces.push(VmSurface {
            first_index: indices.len() as u32,
            index_count: surf_indices.len() as u32,
            base_vertex: if rebase { 0 } else { base as i32 },
            bind_group,
            src: src as u32,
        });
        verts.extend_from_slice(&surf_verts);
        match rebase {
            // `base + i` stays under 1 << 16: the total is.
            true => indices.extend(surf_indices.iter().map(|&i| (base + usize::from(i)) as u16)),
            false => indices.extend_from_slice(&surf_indices),
        }
    }
    if surfaces.is_empty() {
        return None;
    }
    let bones = bones.then(|| {
        // Identity bones: skinning is a no-op until an animation is sampled.
        let identity: Vec<[f32; 16]> = vec![glam::Mat4::IDENTITY.to_cols_array(); VM_BONE_COUNT];
        let buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("viewmodel bone matrices"),
            contents: bytemuck::cast_slice(&identity),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("viewmodel bone bind group"),
            layout: &vm.bone_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: buf.as_entire_binding(),
            }],
        });
        VmBones { buf, bg }
    });
    Some(VmModel {
        vertex_buf: device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("viewmodel vertices"),
            contents: bytemuck::cast_slice(&verts),
            usage: wgpu::BufferUsages::VERTEX,
        }),
        index_buf: device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("viewmodel indices"),
            contents: bytemuck::cast_slice(&indices),
            usage: wgpu::BufferUsages::INDEX,
        }),
        bind_groups,
        surfaces,
        skins,
        bones,
    })
}

/// An entity surface under `autosprite` or `autosprite2`, encoded in model
/// space as the world's soups are (`vcod_common::deform`): the centre or
/// end midpoint in `pos`, the stamp corner in `uv`, autosprite2's axis in
/// `normal`, the half-width or signed half end-length in `tangent[0]`.
/// Retail runs the deform on the entity's posed tess with the view's axes
/// brought into the entity's frame (CoDMP.exe 0x514080, 0x514330 through
/// 0x514010); `dynamic_model.wgsl` adds the world-space view axes after the
/// pose, the same corners. Autosprite's quad takes its first vertex's bones,
/// as its centre is that vertex's quad.
pub(super) fn encode_entity_sprite(
    skin: &str,
    sprite: Sprite,
    verts: &mut [VmVert],
    indices: &mut Vec<u16>,
) {
    let mut dv: Vec<vcod_common::mapworld::DrawVert> = verts
        .iter()
        .map(|v| vcod_common::mapworld::DrawVert {
            pos: v.pos,
            uv: v.uv,
            lm_uv: [0.0; 2],
            normal: v.normal,
            color: v.color,
        })
        .collect();
    match sprite {
        Sprite::Auto if dv.len().is_multiple_of(4) && indices.len() == dv.len() / 4 * 6 => {
            *indices = vcod_common::deform::encode_autosprite(&mut dv, 0)
                .into_iter()
                .map(|i| i as u16)
                .collect();
            for quad in verts.as_chunks_mut::<4>().0 {
                let (bi, bw) = (quad[0].bone_indices, quad[0].bone_weights);
                for v in quad {
                    (v.bone_indices, v.bone_weights) = (bi, bw);
                }
            }
        }
        Sprite::Auto => {
            log::warn!("Autosprite shader {skin} had odd vertex or index count");
            return;
        }
        Sprite::Auto2 => {
            let idx: Vec<u32> = indices.iter().map(|&i| u32::from(i)).collect();
            vcod_common::deform::encode_autosprite2(&mut dv, &idx, 0);
        }
    }
    for (v, d) in verts.iter_mut().zip(&dv) {
        v.pos = d.pos;
        v.uv = d.uv;
        v.normal = d.normal;
        v.color = d.color;
        v.tangent = [d.lm_uv[0], 0.0, 0.0];
    }
}

/// The skins `upload_vm_model` binds for these meshes, each once, in
/// order: those of the surfaces it draws.
pub(super) fn drawn_skins<'m>(
    meshes: impl Iterator<Item = (&'m [model::Surface], &'m [String])>,
) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for (surfaces, materials) in meshes {
        for s in surfaces
            .iter()
            .filter(|s| !s.verts.is_empty() && !s.indices.is_empty())
        {
            let skin = materials
                .get(s.material)
                .map(String::as_str)
                .unwrap_or_default();
            if seen.insert(skin) {
                out.push(skin.to_string());
            }
        }
    }
    out
}

/// `r_dynamiclight` 1 keeps the world's dlight pass; any other value
/// zeroes its light count (0x4e9cb0).
pub(super) fn lights_world(mode: i32) -> bool {
    mode == 1
}

/// Only `r_dynamiclight` 0 zeroes the light count the model light pick
/// reads, so 2 ("Models Only") still lights entities (0x4e9cb0; the
/// split between the two counts is INFERRED, light grid doc).
pub(super) fn lights_models(mode: i32) -> bool {
    mode != 0
}

/// The scene lights the model light pick reads, the view weapon's
/// included: the cgame's then the entities' (`constantLight`, the
/// projectiles'), capped at the scene's count, and none under
/// `r_dynamiclight 0` (light grid doc, the model pick count).
pub(super) fn model_scene_lights(
    scene: &[SceneLight],
    entity: &[SceneLight],
    mode: i32,
) -> Vec<SceneLight> {
    if !lights_models(mode) {
        return Vec::new();
    }
    let mut out = scene.to_vec();
    out.extend_from_slice(entity);
    out.truncate(crate::fx::sim::MAX_SCENE_LIGHTS);
    out
}

#[cfg(test)]
mod light_tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use vcod_common::mapworld::{EntityLight, EntityLighter};

    fn sprite_vert(p: [f32; 3], bone: u8) -> VmVert {
        VmVert {
            pos: p,
            normal: [0.0, -1.0, 0.0],
            uv: [0.3, 0.7],
            bone_indices: [bone, 0, 0, 0],
            bone_weights: [1.0, 0.0, 0.0, 0.0],
            color: [255; 4],
            tangent: [0.0; 3],
            binormal: [0.0; 3],
        }
    }

    /// An entity's autosprite quad uploads as the world's: its centre, the
    /// stamp corners and indices, the half-width in `tangent[0]` (0.707 of
    /// the corner distance, 0x5691cc), and one bone set, so the posed
    /// corners face the view as `autosprite_corner` puts them.
    #[test]
    fn an_entity_autosprite_quad_uploads_encoded() {
        let mut v: Vec<VmVert> = [[-8., 0., -8.], [8., 0., -8.], [8., 0., 8.], [-8., 0., 8.]]
            .iter()
            .enumerate()
            .map(|(i, p)| sprite_vert(*p, i as u8))
            .collect();
        let mut idx = vec![0, 1, 2, 0, 2, 3];
        encode_entity_sprite("light_sprite", Sprite::Auto, &mut v, &mut idx);
        assert_eq!(idx, vec![0, 1, 3, 3, 1, 2]);
        let r = 128f32.sqrt() * vcod_common::deform::AUTOSPRITE_RADIUS_SCALE;
        for (k, x) in v.iter().enumerate() {
            assert_eq!(x.pos, [0.0; 3]);
            assert!((x.tangent[0] - r).abs() < 1e-5);
            assert_eq!(x.bone_indices, [0; 4], "vertex {k}");
        }
        assert_eq!(
            v.iter().map(|x| x.uv).collect::<Vec<_>>(),
            [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]]
        );
        // seen down +y: left -x, up +z; the first corner is top left
        let d = vcod_common::mapworld::DrawVert {
            pos: v[0].pos,
            uv: v[0].uv,
            lm_uv: [v[0].tangent[0], 0.0],
            normal: v[0].normal,
            color: v[0].color,
        };
        let c = vcod_common::deform::autosprite_corner(&d, -glam::Vec3::X, glam::Vec3::Z);
        assert!((c - glam::Vec3::new(-r, 0.0, r)).length() < 1e-4);
    }

    /// autosprite2 keeps the ribbon's indices and moves each end onto its
    /// midpoint, the axis in the normal and the signed half end in
    /// `tangent[0]`; an odd autosprite surface stays as it is.
    #[test]
    fn an_entity_autosprite2_ribbon_uploads_encoded() {
        let mut v: Vec<VmVert> = [[0., 0., -2.], [100., 0., -2.], [100., 0., 2.], [0., 0., 2.]]
            .map(|p| sprite_vert(p, 0))
            .to_vec();
        let mut idx = vec![0, 1, 2, 0, 2, 3];
        encode_entity_sprite("beam", Sprite::Auto2, &mut v, &mut idx);
        assert_eq!(idx, vec![0, 1, 2, 0, 2, 3]);
        assert_eq!([v[0].pos, v[3].pos], [[0.0; 3]; 2]);
        assert_eq!([v[1].pos, v[2].pos], [[100.0, 0.0, 0.0]; 2]);
        assert!(v.iter().all(|x| x.normal[0].abs() == 100.0));
        assert!(v.iter().all(|x| x.tangent[0].abs() == 2.0));
        assert_eq!(v[0].tangent[0], -v[3].tangent[0]);

        let mut odd: Vec<VmVert> = (0..3).map(|i| sprite_vert([i as f32, 0., 0.], 0)).collect();
        let mut idx = vec![0, 1, 2];
        encode_entity_sprite("odd", Sprite::Auto, &mut odd, &mut idx);
        assert_eq!(odd[1].pos, [1.0, 0.0, 0.0]);
        assert_eq!(idx, vec![0, 1, 2]);
    }

    /// A lighter whose texels read the origin's x and the box's low z,
    /// counting its lookups.
    fn counting() -> (Cod4EntityLight, Arc<AtomicUsize>) {
        let n = Arc::new(AtomicUsize::new(0));
        let seen = n.clone();
        let lighter = EntityLighter(Arc::new(move |o: [f32; 3], b: [[f32; 3]; 2]| {
            seen.fetch_add(1, Ordering::Relaxed);
            EntityLight {
                texels: [[o[0] as u8, b[0][2] as u8, 0, 255]; 64],
                probe: 1,
                primary: 0,
            }
        }));
        let c4 = Cod4EntityLight::new(lighter, glam::Vec3::Z, glam::Vec3::ONE, Vec::new());
        (c4, n)
    }

    fn at(x: f32, z: f32) -> (glam::Vec3, [glam::Vec3; 2]) {
        let o = glam::Vec3::new(x, 0.0, 4.0);
        (o, [o - glam::Vec3::Z * z, o + glam::Vec3::ONE])
    }

    /// A held scene looks nothing up; an origin or box that moved, or one
    /// new, is looked up alone, and every set and block is the one a fresh
    /// lookup gives in its place. The changed range covers each set that
    /// moved.
    #[test]
    fn cod4_lookups_hold_while_their_origin_and_box_do() {
        let (mut c4, n) = counting();
        let first: Vec<_> = (0..5).map(|i| at(i as f32, 2.0)).collect();
        assert_eq!(c4.frame_sets(&first, 0.5), 0..5);
        assert_eq!(n.swap(0, Ordering::Relaxed), 5);
        let a = (c4.sets().to_vec(), c4.blocks().to_vec());
        assert_eq!(c4.frame_sets(&first, 0.5), 5..5, "held");
        assert!((c4.sets(), c4.blocks()) == (&a.0[..], &a.1[..]));
        assert_eq!(n.swap(0, Ordering::Relaxed), 0, "held");
        // one new in front shifts every set's block; a box grew
        let mut next = vec![at(9.0, 2.0)];
        next.extend_from_slice(&first);
        next[3].1[0].z -= 1.0;
        assert_eq!(c4.frame_sets(&next, 0.5), 0..6);
        assert_eq!(n.swap(0, Ordering::Relaxed), 2);
        let (fresh, _) = counting();
        let check = |c4: &Cod4EntityLight, list: &[(glam::Vec3, [glam::Vec3; 2])], identity| {
            assert_eq!(c4.sets().len(), list.len());
            for (i, &(o, bx)) in list.iter().enumerate() {
                let (set, texels) = fresh.set(o, bx, i as u32, identity);
                assert!(c4.sets()[i] == set && c4.blocks()[i] == texels, "set {i}");
            }
        };
        check(&c4, &next, 0.5);
        // one moved in place: that set alone; the list shrank: none
        next[2].0.x += 1.0;
        assert_eq!(c4.frame_sets(&next, 0.5), 2..3);
        check(&c4, &next, 0.5);
        assert_eq!(c4.frame_sets(&next[..4], 0.5), 4..4);
        check(&c4, &next[..4], 0.5);
        // a new identityLight repacks every set, looking none up
        n.swap(0, Ordering::Relaxed);
        assert_eq!(c4.frame_sets(&next[..4], 1.0), 0..4);
        assert_eq!(n.swap(0, Ordering::Relaxed), 0);
        check(&c4, &next[..4], 1.0);
    }

    /// Only the hint is compared; what lies past the old end is new.
    #[test]
    fn changed_span_within_compares_the_hint_alone() {
        let old = [1u32, 2, 3, 4];
        assert_eq!(changed_span_within(&old, &[1, 9, 3, 8], 1..4), Some(1..4));
        assert_eq!(changed_span_within(&old, &[1, 9, 3, 4], 1..2), Some(1..2));
        assert_eq!(changed_span_within(&old, &old, 4..4), None);
        assert_eq!(changed_span_within(&old, &[1, 2], 2..2), None);
        assert_eq!(
            changed_span_within(&old, &[1, 2, 3, 4, 5, 6], 6..6),
            Some(4..6)
        );
        assert_eq!(
            changed_span_within(&old, &[1, 7, 3, 4, 5], 1..2),
            Some(1..5)
        );
        assert_eq!(changed_span_within(&[], &[5u32], 0..0), Some(0..1));
    }

    #[test]
    fn changed_span_covers_what_differs_and_what_is_new() {
        let old = [1u32, 2, 3, 4];
        assert_eq!(changed_span(&old, &old), None);
        assert_eq!(changed_span(&old, &old[..2]), None);
        assert_eq!(changed_span(&old, &[1, 9, 3, 8]), Some(1..4));
        assert_eq!(changed_span(&old, &[1, 2, 7]), Some(2..3));
        assert_eq!(changed_span(&old, &[1, 2, 3, 4, 5]), Some(4..5));
        assert_eq!(changed_span(&[], &[5u32]), Some(0..1));
        // by bytes: -0 is not 0
        assert_eq!(changed_span(&[0.0f32], &[-0.0]), Some(0..1));
    }
}
