//! The frame's draw list on the CPU: draw kinds, the sorted shader order and
//! the merge of world, mark, entity and fx draws.

use super::*;

/// The world batches' pipelines. Layers (polygonOffset terrain blends) and
/// overlays (decals, masked cutouts) are coplanar with the world and draw
/// depth-biased on top. `Stage` marks batches expanded into per-stage draws
/// instead. Placement comes from the batch's sort key (`order_draws`).
#[derive(Copy, Clone, PartialEq)]
pub(super) enum Pass {
    /// `fs_main`: the image's alpha cut at 0.5.
    Opaque,
    /// A CoD1 `Opaque` batch whose image has no texel the cut drops, drawn
    /// without the discard (`fs_main_solid`) so early depth writes hold.
    Solid,
    /// Static models (`fs_prop`): `culled` as retail culls a one-sided
    /// skin, `solid` as [`Pass::Solid`] (cod11-client-perf.md 5).
    Prop {
        culled: bool,
        solid: bool,
    },
    Layer,
    Overlay,
    Stage,
}

pub(super) struct DrawCall {
    pub(super) bind_group: usize,
    pub(super) pass: Pass,
}

/// A per-frame draw: `batch` picks the bind group and pass, the range is in
/// the gathered index buffer.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct DrawRange {
    pub batch: u32,
    pub first: u32,
    pub count: u32,
}

/// One entry of a frame's emission-ordered list.
#[derive(Copy, Clone)]
pub(super) enum DrawRef {
    /// A legacy batch on its fixed pipeline.
    Legacy(Pass),
    /// An authored stage draw; index into `WorldGpu::stage_batches`.
    Stage(u32),
}

pub(super) struct FrameDraw {
    pub(super) kind: DrawRef,
    /// The batch's material sort and its place in the sorted shader list
    /// (`WorldGpu::batch_keys`).
    pub(super) sort: f32,
    pub(super) rank: u32,
    /// Index into `WorldGpu::draws`, the gathered visible range.
    pub(super) src: u32,
}

/// One entry of the frame's world list: a [`FrameDraw`]'s sort and rank,
/// the draw set it is in ([`Renderer::world_sets`]) and its index in that
/// set's frame draws.
#[derive(Clone, Copy)]
pub(super) struct WorldItem {
    pub(super) sort: f32,
    pub(super) rank: u32,
    pub(super) set: usize,
    pub(super) draw: u32,
}

/// The pipeline a run of world draws has bound.
#[derive(Copy, Clone, PartialEq)]
pub(super) enum Bound {
    Legacy(Pass),
    Stage(StageVariant, bool, bool),
    /// CoD2's lit pipeline (`cod2::Cod2World`), then a static prop's and
    /// a water's, and which of the depth prepass's pipelines.
    Lit(
        StageVariant,
        bool,
        Option<vcod_common::material::PolygonOffset>,
        cod2::FsKey,
        bool,
        Depth,
    ),
    /// A CoD4 feathered or distortion stage (`cod4_worldfx`).
    Fx(cod4_worldfx::WorldFx, StageVariant, bool, bool),
}

/// Where a draw stands to `r_vc_depthprepass`: its own pipeline, the
/// prepass's depth-only draw, or its colour draw under EQUAL after it.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub(super) enum Depth {
    Plain,
    Prepass,
    /// The prepass writing `floatZ` too (cod4-client-perf.md 35).
    FloatZ,
    Equal,
}

/// Where the dlight pass goes in the merged list: after the last
/// world draw that sorts up to opaque, as retail lights a batch right
/// after its base stages (0x500d3a) and only those sorts take it.
pub(super) fn dlight_slot(items: &[SortedItem], world_sorts: &[f32]) -> usize {
    items
        .iter()
        .rposition(|it| matches!(it, SortedItem::World(wi) if world_sorts[*wi] <= SORT_OPAQUE))
        .map_or(0, |at| at + 1)
}

/// Where the depth copy a CoD2 feather reads goes: before the first
/// entity or view weapon surface, so the copy holds the world (props
/// included) and not the models, as retail's `floatZ` under
/// `r_depthPrepassModels 0` does (cod2-materials.md 9.5); never past the
/// frame's `split`.
pub(super) fn feather_copy_at(items: &[SortedItem], split: usize) -> usize {
    items
        .iter()
        .position(|it| matches!(it, SortedItem::Model(_)))
        .map_or(split, |m| m.min(split))
}

/// Where the SSAO multiply goes in the merged list: after the last draw
/// that sorts up to decal, so it darkens the opaque surfaces and the
/// polygonOffset layers and impact marks blended onto them and none of the
/// blends after.
pub(super) fn ssao_split(
    items: &[SortedItem],
    world: &[f32],
    marks: &[f32],
    models: &[f32],
    fx: &[f32],
) -> usize {
    items
        .iter()
        .rposition(|it| {
            let sort = match *it {
                SortedItem::World(i) => world[i],
                SortedItem::Mark(i) => marks[i],
                SortedItem::Model(i) => models[i],
                SortedItem::Fx(i) => fx[i],
            };
            sort <= SORT_DECAL
        })
        .map_or(0, |at| at + 1)
}

/// The emission order of `draws`, into `out`: by the batch's place in the
/// sorted shader list (`keys`, material sort first), each batch's stages
/// back to back in stage order, as retail's draw loop iterates a shader's
/// stages over its surfaces (shader doc 5). Sky soups are never in `draws`.
pub(super) fn order_draws(
    keys: &[(f32, u32)],
    calls: &[DrawCall],
    stages_of_batch: &[Vec<u32>],
    draws: &[DrawRange],
    out: &mut Vec<FrameDraw>,
) {
    out.clear();
    for (di, d) in draws.iter().enumerate() {
        let b = d.batch as usize;
        let (sort, rank) = keys[b];
        let at = |kind| FrameDraw {
            kind,
            sort,
            rank,
            src: di as u32,
        };
        match calls[b].pass {
            Pass::Stage => out.extend(stages_of_batch[b].iter().map(|&sb| at(DrawRef::Stage(sb)))),
            p => out.push(at(DrawRef::Legacy(p))),
        }
    }
    // stable: a batch keeps its stage order
    out.sort_by_key(|fd| fd.rank);
}

/// A shader as R_SortNewShader (CoDMP.exe 0x4fb790) reads it: its sort and
/// its first stage's bundle 0 and 1 image numbers (0 for none), `None` for
/// a shader without stages.
#[derive(Copy, Clone, Debug)]
pub(super) struct SortEntry {
    pub(super) sort: f32,
    pub(super) images: Option<[u32; 2]>,
}

/// A scripted shader's entry: its stages' images numbered in stage order,
/// then its sort and its sorting stage's two bundle images.
pub(super) fn staged_sort_entry(
    sh: &Material,
    image: &mut dyn FnMut(&ImageRef) -> u32,
) -> SortEntry {
    for st in &sh.stages {
        for bu in &st.bundles {
            image(&bu.image);
            for p in bu.anim.iter().flat_map(|a| &a.paths) {
                image(&ImageRef::Path(p.clone()));
            }
        }
    }
    SortEntry {
        sort: sh.sort,
        images: sh.sort_images.as_ref().map(|imgs| {
            let mut at = |i: usize| imgs.get(i).map_or(0, &mut *image);
            [at(0), at(1)]
        }),
    }
}

/// Each entry's place in retail's sorted shader list when registered in
/// this order. R_SortNewShader walks back from the end and puts the new
/// shader after the first one that sorts lower, or sorts the same with a
/// lower bundle 0 image, or the same bundle 0 image and a bundle 1 image
/// no higher; it passes everything else, a stageless shader and a missing
/// bundle 1 included.
pub(super) fn sorted_places(entries: &[SortEntry]) -> Vec<u32> {
    let mut list: Vec<usize> = Vec::with_capacity(entries.len());
    for n in 0..entries.len() {
        insert_sorted(entries, &mut list, n);
    }
    let mut place = vec![0; entries.len()];
    for (p, &e) in list.iter().enumerate() {
        place[e] = p as u32;
    }
    place
}

/// R_SortNewShader's walk for `entries[n]` into `list` (indices into
/// `entries` in sorted order).
fn insert_sorted(entries: &[SortEntry], list: &mut Vec<usize>, n: usize) {
    let new = &entries[n];
    let [b0, b1] = new.images.unwrap_or([0, 0]);
    let mut at = list.len();
    while at > 0 {
        let e = &entries[list[at - 1]];
        if e.sort < new.sort {
            break;
        }
        if e.sort == new.sort
            && let Some([e0, e1]) = e.images
        {
            let b0_holds = e0 == 0 || e0 == b0;
            if e0 != 0 && e0 < b0 {
                break;
            }
            if b0_holds && e1 != 0 && e1 <= b1 {
                break;
            }
        }
        at -= 1;
    }
    list.insert(at, n);
}

/// No world shader of a model's sort follows it in the sorted list
/// ([`ShaderList::model_keys`]): on a tie it draws after the world's.
pub(crate) const AFTER_THE_WORLD: u32 = u32::MAX;

/// A CoD1 map's sorted shader list kept past the load, so the cgame's
/// model shaders register into it as R_SortNewShader (CoDMP.exe 0x4fb790)
/// puts them, their images numbered on from the world's (shader doc 5).
pub(crate) struct ShaderList {
    entries: Vec<SortEntry>,
    /// `entries` in sorted order.
    list: Vec<usize>,
    /// The world's entries lead `entries`.
    world: usize,
    images: ImageNumbers,
}

impl ShaderList {
    /// The list over the world's `entries` as `sorted_places` sorts them,
    /// numbering on through `images`.
    pub(super) fn new(entries: Vec<SortEntry>, images: ImageNumbers) -> Self {
        let mut list = Vec::with_capacity(entries.len());
        for n in 0..entries.len() {
            insert_sorted(&entries, &mut list, n);
        }
        let world = entries.len();
        ShaderList {
            entries,
            list,
            world,
            images,
        }
    }

    /// Registers a model's shader: its stages' images numbered in stage
    /// order, `skin_image` standing for `$texturename`, `Lightmap` and
    /// `White` for `$white`.
    pub(crate) fn register(&mut self, sh: &Material, skin_image: &str) {
        let images = &mut self.images;
        let entry = staged_sort_entry(sh, &mut |r: &ImageRef| match r {
            ImageRef::Path(p) if p.eq_ignore_ascii_case(crate::entity_material::TEXTURE_NAME) => {
                images.number(skin_image)
            }
            ImageRef::Path(p) => images.number(p),
            ImageRef::Lightmap | ImageRef::White => images.number("$white"),
        });
        self.entries.push(entry);
        insert_sorted(&self.entries, &mut self.list, self.entries.len() - 1);
    }

    /// Each registered model shader's `(order, anchor)`, in registration
    /// order: its place among the model shaders, and the world rank
    /// (`sorted_places`) it draws before on a sort tie, or
    /// [`AFTER_THE_WORLD`].
    pub(crate) fn model_keys(&self) -> Vec<(u32, u32)> {
        let mut keys = vec![(0, AFTER_THE_WORLD); self.entries.len() - self.world];
        let (mut world_seen, mut models_seen) = (0u32, 0u32);
        for (at, &e) in self.list.iter().enumerate() {
            if e < self.world {
                world_seen += 1;
                continue;
            }
            let sort = self.entries[e].sort;
            let next_world = self.list[at + 1..].iter().find(|&&n| n < self.world);
            let anchor = match next_world {
                Some(&w) if self.entries[w].sort == sort => world_seen,
                _ => AFTER_THE_WORLD,
            };
            keys[e - self.world] = (models_seen, anchor);
            models_seen += 1;
        }
        keys
    }
}

/// Each entry's place in a CoD4 frame's order: by sort, then the first
/// place in 1.7's lit draw order (`MapWorld::soup_order`) any of the
/// batch's soups holds, so a batch draws where retail's list reaches its
/// first surface and the primary light groups the materials as 1.7's key
/// does (cod4-world.md 12). Entries past the world's batches keep their
/// registration order after them.
pub(super) fn cod4_places(
    entries: &[SortEntry],
    ranges: &[Option<IndexRange>],
    order: &[u64],
) -> Vec<u32> {
    let mut first = vec![u64::MAX; entries.len()];
    for (r, &o) in ranges.iter().zip(order) {
        if let Some(f) = r.and_then(|r| first.get_mut(r.batch as usize)) {
            *f = (*f).min(o);
        }
    }
    let mut by: Vec<usize> = (0..entries.len()).collect();
    by.sort_by(|&a, &b| {
        let (ea, eb) = (&entries[a], &entries[b]);
        ea.sort
            .total_cmp(&eb.sort)
            .then(first[a].cmp(&first[b]))
            .then(a.cmp(&b))
    });
    let mut place = vec![0; entries.len()];
    for (p, &e) in by.iter().enumerate() {
        place[e] = p as u32;
    }
    place
}

/// Retail's image numbers in load order (`image+0x58`, CoDMP.exe
/// 0x4ec310: the creation count plus 0x400); 0 stays "no image".
#[derive(Default)]
pub(super) struct ImageNumbers(HashMap<String, u32>);

impl ImageNumbers {
    pub(super) fn number(&mut self, name: &str) -> u32 {
        let mut key = name.to_ascii_lowercase();
        for ext in [".tga", ".jpg", ".dds"] {
            if let Some(stem) = key.strip_suffix(ext) {
                key.truncate(stem.len());
                break;
            }
        }
        let next = self.0.len() as u32 + 1;
        *self.0.entry(key).or_insert(next)
    }
}

/// One entry of the frame's merged draw list.
#[derive(Copy, Clone, Debug, PartialEq)]
pub(super) enum SortedItem {
    World(usize),
    /// A run of impact marks (`mark_pass::MarkPass::runs`).
    Mark(usize),
    /// An entity or viewmodel surface.
    Model(usize),
    Fx(usize),
}

/// Merges the world's draws, the mark runs, the entity and viewmodel
/// surfaces and the fx runs, each list already in sort order, by material
/// sort. On a tie the world goes first, then the marks, the models and the
/// fx: the world's materials and images register at map load, before the
/// cgame's (R_SortNewShader breaks ties on image numbers, 0x4fb790). The
/// marks' place among the cgame's lists is vcod's guess.
#[cfg(test)]
pub(super) fn merge_by_sort(
    world: &[f32],
    marks: &[f32],
    models: &[f32],
    fx: &[f32],
) -> Vec<SortedItem> {
    let world: Vec<(f32, u32)> = world.iter().map(|&s| (s, 0)).collect();
    let models: Vec<(f32, u32)> = models.iter().map(|&s| (s, AFTER_THE_WORLD)).collect();
    merge_by_place(&world, marks, &models, fx)
}

/// [`merge_by_sort`] with the world's draws' ranks and the models'
/// anchors ([`ShaderList::model_keys`]): on a tie a model whose shader
/// sorts before a world shader of its sort (one whose image numbers are
/// lower, as a reused image's are) draws before that shader's draws.
pub(super) fn merge_by_place(
    world: &[(f32, u32)],
    marks: &[f32],
    models: &[(f32, u32)],
    fx: &[f32],
) -> Vec<SortedItem> {
    // (sort, place, class): the world by rank, an anchored model before
    // the rank it anchors to, the rest after the world in list order
    let key = |l: usize, i: usize| -> (f32, u32, u8) {
        match l {
            0 => (world[i].0, world[i].1, 1),
            1 => (marks[i], u32::MAX, 2),
            2 if models[i].1 == AFTER_THE_WORLD => (models[i].0, u32::MAX, 3),
            2 => (models[i].0, models[i].1, 0),
            _ => (fx[i], u32::MAX, 4),
        }
    };
    let less =
        |a: (f32, u32, u8), b: (f32, u32, u8)| a.0 < b.0 || (a.0 == b.0 && (a.1, a.2) < (b.1, b.2));
    let lens = [world.len(), marks.len(), models.len(), fx.len()];
    let mut at = [0usize; 4];
    let mut out = Vec::with_capacity(lens.iter().sum());
    loop {
        let mut best: Option<usize> = None;
        for l in 0..4 {
            if at[l] < lens[l] && best.is_none_or(|b| less(key(l, at[l]), key(b, at[b]))) {
                best = Some(l);
            }
        }
        let Some(l) = best else {
            break;
        };
        out.push(match l {
            0 => SortedItem::World(at[l]),
            1 => SortedItem::Mark(at[l]),
            2 => SortedItem::Model(at[l]),
            _ => SortedItem::Fx(at[l]),
        });
        at[l] += 1;
    }
    out
}

/// What a run of world draws left bound: the draw set whose camera group
/// is bound ([`Renderer::world_sets`]), the pipeline and groups 1 and 2.
#[derive(Clone, Copy, Default)]
pub(super) struct WorldRunState {
    pub(super) set: Option<usize>,
    pub(super) bound: Option<Bound>,
    pub(super) groups: WorldGroups,
}

/// What a run of entity draws left bound: the `DynamicPass::models` index
/// whose buffers are bound, and the stage state.
#[derive(Clone, Copy, Default)]
pub(super) struct EntRunState {
    pub(super) model: Option<usize>,
    pub(super) bound: EntBound,
}

/// What a merged list's last draw left bound.
pub(super) enum Emit {
    None,
    World(WorldRunState),
    Ent(EntRunState),
    Gun { part: u32, bound: EntBound },
    Fx { blend: FxBlend, soft: bool },
}

/// Copies the visible `ranges` out of `src` batch by batch into `out`, one
/// `DrawRange` per batch that got anything. `per_batch` is scratch, kept
/// between frames.
pub(crate) fn gather(
    src: &[u32],
    ranges: impl IntoIterator<Item = IndexRange>,
    batch_count: usize,
    out: &mut Vec<u32>,
    per_batch: &mut Vec<Vec<IndexRange>>,
) -> Vec<DrawRange> {
    per_batch.resize_with(batch_count, Vec::new);
    for b in per_batch.iter_mut() {
        b.clear();
    }
    for r in ranges {
        per_batch[r.batch as usize].push(r);
    }
    out.clear();
    let mut draws = Vec::new();
    for (batch, rs) in per_batch.iter().enumerate() {
        if rs.is_empty() {
            continue;
        }
        let first = out.len() as u32;
        for r in rs {
            out.extend_from_slice(&src[r.first as usize..(r.first + r.count) as usize]);
        }
        draws.push(DrawRange {
            batch: batch as u32,
            first,
            count: out.len() as u32 - first,
        });
    }
    draws
}
