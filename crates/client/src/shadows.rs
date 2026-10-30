//! `vc_shadows`: real-time sun shadows of dynamic casters, a vcod-only
//! option (retail draws none; docs/research/cod11-light-grid-and-leaf-lights.md
//! section 12).
//!
//! The world's own sun shadows are baked into its lightmaps, so only what
//! moves casts here: snapshot entities (players, corpses, entity models)
//! and brush models off their spawn pose. Casters whose texture cuts
//! (foliage, fences, grilles) are alpha-tested as their main pass is, in
//! both layers. Two orthographic maps from the sun, fitted around the
//! view: the dynamic casters over a 2048-texel window, and the static
//! world (soups and props at spawn pose) over the window and a margin,
//! kept across moves and drawn ahead a slice a frame. A
//! full-screen pass after the opaque draws reads the scene depth and
//! darkens each pixel by the sun's share of its light where the dynamic
//! layer hides the sun and the static layer does not, so indoor and
//! already-shadowed points keep their baked light.

use glam::{Mat4, Vec3, Vec4};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use vcod_common::static_light::{RegionKeep, Sun};

/// shadow_cut.wgsl with the deforms its staged casters run.
const SHADOW_CUT_WGSL: &str = concat!(
    include_str!("shadow_cut.wgsl"),
    include_str!("noise.wgsl"),
    include_str!("deform.wgsl")
);

/// Shadow map side in texels.
const MAP_SIZE: u32 = 2048;
/// Half the map's side in world units: 1.5 units a texel.
const HALF_EXTENT: f32 = 1536.0;
const TEXEL: f32 = 2.0 * HALF_EXTENT / MAP_SIZE as f32;
/// How far ahead of the eye the map is centred, along the horizontal view.
const AHEAD: f32 = 768.0;
/// The centre snaps to this grid in light space: a whole number of texels,
/// so the texel grid holds still in the world.
const SNAP: f32 = 192.0;
/// Shadows fade out over the outer part of the map, in light NDC.
const FADE_START: f32 = 0.8;
/// Receiver offsets against acne: along its normal, and towards the sun.
const NORMAL_OFFSET: f32 = 3.0;
const DEPTH_BIAS: f32 = 1.0;
/// Uniform slots: the plain light matrix, then one per moved brush model,
/// the last [`STATIC_SLOTS`] for the static redraw's pieces.
const SLOT_STRIDE: u64 = 256;
const MAX_SLOTS: usize = 128;
/// A static redraw is one rectangle, cut in four at most by the map's wrap.
const STATIC_SLOTS: usize = 4;
const MASK_FLOATS: usize = 4 * 16 + 10 * 4;
const MAP_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;

/// One static caster: a soup or prop's `(first, count)` into the
/// world's indices and its bounds.
pub(crate) struct CasterRange {
    pub(crate) first: u32,
    pub(crate) count: u32,
    pub(crate) bounds: (Vec3, Vec3),
    /// The batch whose texture alpha-tests it (resolved through a
    /// [`CutBinding`] at draw time); `None` draws it solid.
    pub(crate) cut: Option<u32>,
}

/// One group of casters in [`Casters`]' index buffer: the solid ones, then
/// per alpha-tested batch its run.
#[derive(Default)]
struct Group {
    solid: std::ops::Range<u32>,
    cuts: Vec<(std::ops::Range<u32>, u32)>,
}

impl Group {
    fn indices(&self) -> u32 {
        self.solid.len() as u32 + self.cuts.iter().map(|(r, _)| r.len() as u32).sum::<u32>()
    }
}

/// How an alpha-tested batch samples its texture: its group 1 (the world's
/// material layout) and, for a scripted stage, its group 2 and params
/// offset. `stage: None` tests at half like the implicit path.
pub(crate) struct CutBinding<'a> {
    pub(crate) mat: &'a wgpu::BindGroup,
    pub(crate) stage: Option<(&'a wgpu::BindGroup, u32)>,
}

/// The layouts the alpha-tested world casters share with the world pass.
pub(crate) struct WorldLayouts<'a> {
    pub(crate) material: &'a wgpu::BindGroupLayout,
    pub(crate) stage: &'a wgpu::BindGroupLayout,
}

/// Side of the cells the static casters are grouped in, so a static
/// redraw skips the cells outside the light's box.
const CELL: f32 = 512.0;

/// The static casters of one loaded world: an index buffer over the
/// world's vertices, the world's soups and the props grouped by cell, then
/// each submodel's soups.
pub(crate) struct Casters {
    id: u64,
    index_buf: wgpu::Buffer,
    /// Per cell, its casters in `index_buf` and its bounds.
    cells: Vec<(Group, Vec3, Vec3)>,
    /// Per lump-27 model, its casters in `index_buf` and their bounds;
    /// model 0's is empty.
    model_ranges: Vec<(Group, Vec3, Vec3)>,
    /// The casters whose shader deforms them over time, wherever they
    /// stand: they cast in the dynamic layer, deformed, each frame.
    deformed: (Group, Vec3, Vec3),
    /// The world's bounds, which fix the map's depth range.
    bounds: (Vec3, Vec3),
    tris: u32,
    /// Of those, alpha-tested.
    cut_tris: u32,
}

impl Casters {
    /// `world` holds model 0's soups and the props, `models[m]` submodel
    /// `m`'s soups, `deformed` the casters that deform.
    pub(crate) fn build(
        device: &wgpu::Device,
        cpu_indices: &[u32],
        world: &[CasterRange],
        models: &[Vec<CasterRange>],
        deformed: &[CasterRange],
    ) -> Casters {
        static NEXT_ID: AtomicU64 = AtomicU64::new(1);
        let mut by_cell: std::collections::BTreeMap<(i32, i32), Vec<&CasterRange>> =
            Default::default();
        for r in world {
            let c = (r.bounds.0 + r.bounds.1) * 0.5;
            let key = ((c.x / CELL).floor() as i32, (c.y / CELL).floor() as i32);
            by_cell.entry(key).or_default().push(r);
        }
        let mut indices: Vec<u32> = Vec::new();
        let cells: Vec<_> = by_cell
            .into_values()
            .map(|rs| push_group(&mut indices, cpu_indices, rs))
            .collect();
        let model_ranges: Vec<_> = models
            .iter()
            .map(|rs| push_group(&mut indices, cpu_indices, rs.iter().collect()))
            .collect();
        let deformed = push_group(&mut indices, cpu_indices, deformed.iter().collect());
        let cut_indices: u32 = cells
            .iter()
            .chain(&model_ranges)
            .map(|c| &c.0)
            .map(|g| g.indices() - g.solid.len() as u32)
            .sum();
        let bounds = cells.iter().fold(
            (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)),
            |(a, b), c| (a.min(c.1), b.max(c.2)),
        );
        let tris = (indices.len() as u32 - deformed.0.indices()) / 3;
        log::info!(
            "shadow casters: {tris} tris, {} alpha-tested",
            cut_indices / 3
        );
        // a buffer can't be empty
        indices.resize(indices.len().max(3), 0);
        use wgpu::util::DeviceExt;
        let index_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("shadow casters"),
            contents: bytemuck::cast_slice(&indices),
            usage: wgpu::BufferUsages::INDEX,
        });
        Casters {
            id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
            index_buf,
            cells,
            model_ranges,
            deformed,
            bounds,
            tris,
            cut_tris: cut_indices / 3,
        }
    }
}

/// Appends `rs`' indices as one [`Group`]: the solid casters first, then
/// one run per cutting batch. Returns it with the group's bounds.
fn push_group(
    indices: &mut Vec<u32>,
    cpu: &[u32],
    mut rs: Vec<&CasterRange>,
) -> (Group, Vec3, Vec3) {
    rs.sort_by_key(|r| r.cut);
    let start = indices.len() as u32;
    let mut g = Group {
        solid: start..start,
        cuts: Vec::new(),
    };
    let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
    for r in rs {
        let at = indices.len() as u32;
        indices.extend_from_slice(&cpu[r.first as usize..(r.first + r.count) as usize]);
        let end = indices.len() as u32;
        match (r.cut, g.cuts.last_mut()) {
            (None, _) => g.solid.end = end,
            (Some(k), Some((run, last))) if *last == k => run.end = end,
            (Some(k), _) => g.cuts.push((at..end, k)),
        }
        lo = lo.min(r.bounds.0);
        hi = hi.max(r.bounds.1);
    }
    (g, lo, hi)
}

/// Whether a box reaches the light's box in x and y (rows 0 and 1 of the
/// orthographic `light_vp`).
fn in_light_box(light_vp: &Mat4, lo: Vec3, hi: Vec3) -> bool {
    let (c, e) = ((lo + hi) * 0.5, (hi - lo) * 0.5);
    [light_vp.row(0), light_vp.row(1)].iter().all(|r| {
        let at = r.truncate().dot(c) + r.w;
        at.abs() <= 1.0 + r.truncate().abs().dot(e)
    })
}

/// A rectangle of shadow-map texels on the light-space grid: texel `(x, y)`
/// spans `x..x + 1` texels along the light's right and `y..y + 1` along
/// its down, from the light-space origin. The grid holds still in the
/// world, so the static layer's texels stay valid across recentres.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct TexRect {
    x0: i32,
    y0: i32,
    x1: i32,
    y1: i32,
}

/// The map's side in texels, as a grid coordinate.
const SIDE: i32 = MAP_SIZE as i32;
/// Texels the static layer holds past the window on each side, drawn
/// ahead of the view a slice at a time.
const MARGIN: i32 = 256;
/// The static layer's side.
const STATIC_SIDE: i32 = SIDE + 2 * MARGIN;
/// Texels a static redraw draws in a frame past a start-over: one
/// 16-texel strip along the layer's whole side. Its cost is mostly the
/// alpha-tested casters' fragments, so it scales with the area.
const BUDGET: i32 = 16 * STATIC_SIDE;
/// The side of the square around the eye a start-over draws at once. The
/// layer starts over when it no longer holds the half of that around the
/// eye, so the strips have time to follow a moving eye.
const CORE: i32 = 512;

impl TexRect {
    /// The map's window with its corner at `o`.
    fn window(o: (i32, i32)) -> TexRect {
        TexRect {
            x0: o.0,
            y0: o.1,
            x1: o.0 + SIDE,
            y1: o.1 + SIDE,
        }
    }

    fn grown(&self, k: i32) -> TexRect {
        TexRect {
            x0: self.x0 - k,
            y0: self.y0 - k,
            x1: self.x1 + k,
            y1: self.y1 + k,
        }
    }

    fn intersect(&self, r: &TexRect) -> TexRect {
        TexRect {
            x0: self.x0.max(r.x0),
            y0: self.y0.max(r.y0),
            x1: self.x1.min(r.x1),
            y1: self.y1.min(r.y1),
        }
    }

    fn contains(&self, r: &TexRect) -> bool {
        self.x0 <= r.x0 && r.x1 <= self.x1 && self.y0 <= r.y0 && r.y1 <= self.y1
    }

    #[cfg(test)]
    fn area(&self) -> i64 {
        i64::from(self.x1 - self.x0) * i64::from(self.y1 - self.y0)
    }
}

/// What the static layer holds: the light basis (as its matrix at the
/// origin) and casters it was drawn for, the texels it holds, and the
/// corner it wraps around (see [`on_torus`]).
#[derive(Clone, Copy, Debug, PartialEq)]
struct StaticHeld {
    key: ([f32; 16], u64),
    valid: TexRect,
    base: (i32, i32),
}

/// One side of a [`TexRect`]: `x` or `y`, and the far (`x1`/`y1`) one.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Side {
    y: bool,
    far: bool,
}

const SIDES: [Side; 4] = [
    Side {
        y: false,
        far: true,
    },
    Side {
        y: false,
        far: false,
    },
    Side { y: true, far: true },
    Side {
        y: true,
        far: false,
    },
];

/// Grows `v` on `side` towards `t` by up to `k` texels, dropping as many
/// from the opposite side once it is wider than the layer. Returns the
/// texels to draw.
fn grow(v: &mut TexRect, t: &TexRect, side: Side, k: i32) -> TexRect {
    if side.y {
        let mut tv = transpose(*v);
        let s = grow(&mut tv, &transpose(*t), Side { y: false, ..side }, k);
        *v = transpose(tv);
        return transpose(s);
    }
    if side.far {
        let x1 = t.x1.min(v.x1 + k);
        let s = TexRect { x0: v.x1, x1, ..*v };
        v.x1 = x1;
        v.x0 = v.x0.max(x1 - STATIC_SIDE);
        s
    } else {
        let x0 = t.x0.max(v.x0 - k);
        let s = TexRect { x0, x1: v.x0, ..*v };
        v.x0 = x0;
        v.x1 = v.x1.min(x0 + STATIC_SIDE);
        s
    }
}

fn transpose(r: TexRect) -> TexRect {
    TexRect {
        x0: r.y0,
        y0: r.x0,
        x1: r.y1,
        y1: r.x1,
    }
}

/// How far `v`'s `side` falls short of `t`'s, and how far it lies from
/// texel `at`.
fn short_by(v: &TexRect, t: &TexRect, side: Side, at: (i32, i32)) -> (i32, i32) {
    let (v, t, at) = if side.y {
        (transpose(*v), transpose(*t), (at.1, at.0))
    } else {
        (*v, *t, at)
    };
    if side.far {
        (t.x1 - v.x1, v.x1 - at.0)
    } else {
        (v.x0 - t.x0, at.0 - v.x0)
    }
}

/// This frame's static redraw for the window `view` and the eye's texel
/// `eye`: what the layer then holds, whether it starts over from a cleared
/// layer, and the texels to draw. The layer aims to hold the window and
/// [`MARGIN`] around it. It starts over with the [`CORE`] around the eye
/// when the light changed or the layer no longer holds that, and otherwise
/// draws one strip of about [`BUDGET`] texels a frame: on the side nearest
/// the eye of those the window still lacks, else of those short of the
/// margin. The mask leaves what the layer does not hold unshadowed.
fn plan_static(
    held: Option<StaticHeld>,
    key: ([f32; 16], u64),
    view: TexRect,
    eye: (i32, i32),
) -> (StaticHeld, bool, Vec<TexRect>) {
    let target = view.grown(MARGIN);
    let around = |side: i32| {
        TexRect {
            x0: eye.0 - side / 2,
            y0: eye.1 - side / 2,
            x1: eye.0 + side / 2,
            y1: eye.1 + side / 2,
        }
        .intersect(&view)
    };
    let (core, keep) = (around(CORE), around(CORE / 2));
    let Some(mut h) = held.filter(|h| h.key == key && h.valid.contains(&keep)) else {
        let h = StaticHeld {
            key,
            valid: core,
            base: (core.x0, core.y0),
        };
        return (h, true, vec![core]);
    };
    let pick = |goal: &TexRect| {
        SIDES
            .into_iter()
            .map(|s| (s, short_by(&h.valid, goal, s, eye)))
            .filter(|(_, (short, _))| *short > 0)
            .min_by_key(|(_, (_, dist))| *dist)
            .map(|(s, _)| s)
    };
    let Some(side) = pick(&view).or_else(|| pick(&target)) else {
        return (h, false, Vec::new());
    };
    let along = if side.y {
        h.valid.x1 - h.valid.x0
    } else {
        h.valid.y1 - h.valid.y0
    };
    let strip = grow(&mut h.valid, &target, side, (BUDGET / along.max(1)).max(1));
    (h, false, vec![strip])
}

/// `r` cut where the static layer wraps: each piece with its corner in the
/// layer. Texel `(x, y)` is stored at `(x - base.0, y - base.1)` mod the
/// layer's side, `base` the corner its last start-over drew, so that draw
/// is a single piece.
fn on_torus(r: TexRect, base: (i32, i32)) -> Vec<(TexRect, (u32, u32))> {
    let n = STATIC_SIDE;
    let cuts = |a: i32, b: i32, o: i32| {
        ((a - o).div_euclid(n)..=(b - o - 1).div_euclid(n))
            .map(move |k| (a.max(o + k * n), b.min(o + (k + 1) * n)))
    };
    let mut out = Vec::new();
    for (y0, y1) in cuts(r.y0, r.y1, base.1) {
        for (x0, x1) in cuts(r.x0, r.x1, base.0) {
            let at = (
                (x0 - base.0).rem_euclid(n) as u32,
                (y0 - base.1).rem_euclid(n) as u32,
            );
            out.push((TexRect { x0, y0, x1, y1 }, at));
        }
    }
    out
}

/// The light's axes and depth range for one sun and world: everything in
/// its matrix but the window.
struct LightBasis {
    right: Vec3,
    up: Vec3,
    dir: Vec3,
    zmax: f32,
    span: f32,
}

/// An orthographic box along `-sun` whose depth spans `bounds`, depth 0
/// the side nearest the sun.
fn light_basis(sun: Vec3, bounds: (Vec3, Vec3)) -> LightBasis {
    let dir = sun.normalize();
    let f = -dir;
    let hint = if f.z.abs() > 0.99 { Vec3::X } else { Vec3::Z };
    let right = f.cross(hint).normalize();
    let up = right.cross(f);
    let (lo, hi) = bounds;
    let (mut zmin, mut zmax) = (f32::MAX, f32::MIN);
    for i in 0..8 {
        let c = Vec3::new(
            if i & 1 == 0 { lo.x } else { hi.x },
            if i & 2 == 0 { lo.y } else { hi.y },
            if i & 4 == 0 { lo.z } else { hi.z },
        );
        let z = c.dot(dir);
        zmin = zmin.min(z);
        zmax = zmax.max(z);
    }
    zmin -= 16.0;
    zmax += 16.0;
    LightBasis {
        right,
        up,
        dir,
        zmax,
        span: zmax - zmin,
    }
}

impl LightBasis {
    /// The window's corner for a view at `eye` looking along `fwd`: centred
    /// [`AHEAD`] in front of the eye, snapped to [`SNAP`] in light space.
    fn window(&self, eye: Vec3, fwd: Vec3) -> (i32, i32) {
        let flat = Vec3::new(fwd.x, fwd.y, 0.0).normalize_or_zero();
        let focus = eye + flat * AHEAD;
        let step = (SNAP / TEXEL) as i32;
        let kx = (focus.dot(self.right) / SNAP).round() as i32;
        let ky = (focus.dot(self.up) / SNAP).round() as i32;
        (kx * step - SIDE / 2, -ky * step - SIDE / 2)
    }

    /// The texel `p` falls in.
    fn texel(&self, p: Vec3) -> (i32, i32) {
        (
            (p.dot(self.right) / TEXEL).floor() as i32,
            (-p.dot(self.up) / TEXEL).floor() as i32,
        )
    }

    /// The view-projection that puts `r` on the whole viewport.
    fn matrix(&self, r: TexRect) -> Mat4 {
        let (hx, hy) = (
            (r.x1 - r.x0) as f32 * 0.5 * TEXEL,
            (r.y1 - r.y0) as f32 * 0.5 * TEXEL,
        );
        let (cx, cy) = (
            (r.x0 + r.x1) as f32 * 0.5 * TEXEL,
            -(r.y0 + r.y1) as f32 * 0.5 * TEXEL,
        );
        Mat4::from_cols(
            (self.right / hx).extend(-cx / hx),
            (self.up / hy).extend(-cy / hy),
            (-self.dir / self.span).extend(self.zmax / self.span),
            Vec4::W,
        )
        .transpose()
    }
}

/// The light's view-projection for a view at `eye` looking along `fwd`
/// (see [`light_basis`], [`LightBasis::window`]).
#[cfg(test)]
fn light_matrix(sun: Vec3, eye: Vec3, fwd: Vec3, bounds: (Vec3, Vec3)) -> Mat4 {
    let b = light_basis(sun, bounds);
    b.matrix(TexRect::window(b.window(eye, fwd)))
}

/// One piece of a static redraw: its matrix and where it sits in the map.
struct Piece {
    vp: Mat4,
    at: (u32, u32),
    size: (u32, u32),
}

/// A snapshot entity surface that casts: its buffers, instance slot,
/// index range, and the image and params of its first depth-writing stage.
pub(crate) struct EntCaster<'a> {
    pub(crate) vertex_buf: &'a wgpu::Buffer,
    pub(crate) index_buf: &'a wgpu::Buffer,
    pub(crate) instance: u32,
    pub(crate) first_index: u32,
    pub(crate) index_count: u32,
    pub(crate) base_vertex: i32,
    pub(crate) image: &'a wgpu::BindGroup,
    pub(crate) params_offset: u32,
}

/// The entity draws' shared bindings.
pub(crate) struct EntBindings<'a> {
    pub(crate) instance_buf: &'a wgpu::Buffer,
    pub(crate) instance_stride: u64,
    pub(crate) bones: &'a wgpu::BindGroup,
    pub(crate) params: &'a wgpu::BindGroup,
}

/// The layouts the entity caster pipeline shares with the entity pass.
pub(crate) struct EntLayouts<'a> {
    pub(crate) skin: &'a wgpu::BindGroupLayout,
    pub(crate) bones: &'a wgpu::BindGroupLayout,
    pub(crate) params: &'a wgpu::BindGroupLayout,
    pub(crate) vertex: wgpu::VertexBufferLayout<'a>,
    pub(crate) instance: wgpu::VertexBufferLayout<'a>,
}

/// GPU timestamps around the caster passes and the mask: ms each.
struct Timer {
    queries: wgpu::QuerySet,
    resolve: wgpu::Buffer,
    readback: wgpu::Buffer,
    /// A readback is in flight; set again by its callback when mapped.
    pending: bool,
    mapped: Arc<AtomicBool>,
    period_ns: f32,
    /// The in-flight frame's static redraw: its pieces and triangles, and
    /// whether it started over.
    static_ran: Option<(usize, u32, bool)>,
    reads: u64,
}

const QUERIES: u32 = 6;

impl Timer {
    fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Option<Timer> {
        if !device.features().contains(wgpu::Features::TIMESTAMP_QUERY) {
            return None;
        }
        let size = u64::from(QUERIES) * 8;
        Some(Timer {
            queries: device.create_query_set(&wgpu::QuerySetDescriptor {
                label: Some("shadow timestamps"),
                ty: wgpu::QueryType::Timestamp,
                count: QUERIES,
            }),
            resolve: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("shadow timestamp resolve"),
                size,
                usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            }),
            readback: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("shadow timestamp readback"),
                size,
                usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }),
            pending: false,
            mapped: Arc::new(AtomicBool::new(false)),
            period_ns: queue.get_timestamp_period(),
            static_ran: None,
            reads: 0,
        })
    }

    fn writes(&self, pair: u32) -> Option<wgpu::RenderPassTimestampWrites<'_>> {
        (!self.pending).then_some(wgpu::RenderPassTimestampWrites {
            query_set: &self.queries,
            beginning_of_pass_write_index: Some(pair * 2),
            end_of_pass_write_index: Some(pair * 2 + 1),
        })
    }
}

/// The least part of a texel's light a shadow keeps, whatever a map's
/// lightmaps measure.
const KEEP_MIN: f32 = 0.25;

/// The sun's luminance over sky plus ambient, which the mask weighs by
/// `n.L`: a shadowed point keeps `1 / (ratio n.L + 1)`. With the map's
/// measured keep on flat ground (`static_light::sun_keep`) the ratio is the
/// one that keeps that part at the sun's elevation; without, the worldspawn
/// split's.
pub(crate) fn sun_ratio(sun: &Sun, keep: Option<f32>) -> f32 {
    let up = sun.dir.normalize_or_zero().z;
    match keep {
        Some(keep) if up > 0.0 => {
            let keep = keep.clamp(KEEP_MIN, 1.0);
            (1.0 - keep) / (keep * up)
        }
        _ => sun.sun / (sun.sky + sun.ambient).max(1e-4),
    }
}

/// The side of the texture that holds a map's [`RegionKeep`], a texel a
/// cell.
const KEEP_SIDE: usize = 128;

/// `k`'s cells as [`KEEP_SIDE`]-square texels, each its keep clamped as
/// [`sun_ratio`] clamps it (so never 0), the rest the whole map's; and the
/// scale and offset that take a world `xy` to the texture's uv. `None`
/// when no cell has a keep of its own or they span more than the texture.
/// The mask falls back to the whole map's ratio where it reads 0.
fn keep_texels(k: &RegionKeep) -> Option<(Vec<u8>, [f32; 4])> {
    let known = |i: usize| k.keeps[i].is_some();
    let cells = (0..k.keeps.len()).filter(|&i| known(i));
    let (mut lo, mut hi) = ((usize::MAX, usize::MAX), (0, 0));
    for i in cells {
        let (x, y) = (i % k.size.0, i / k.size.0);
        lo = (lo.0.min(x), lo.1.min(y));
        hi = (hi.0.max(x), hi.1.max(y));
    }
    if lo.0 == usize::MAX || hi.0 - lo.0 >= KEEP_SIDE || hi.1 - lo.1 >= KEEP_SIDE {
        return None;
    }
    let byte = |keep: f32| (keep.clamp(KEEP_MIN, 1.0) * 255.0).round() as u8;
    let mut texels = vec![byte(k.whole.keep()); KEEP_SIDE * KEEP_SIDE];
    for y in lo.1..=hi.1 {
        for x in lo.0..=hi.0 {
            if let Some(keep) = k.keeps[y * k.size.0 + x] {
                texels[(y - lo.1) * KEEP_SIDE + (x - lo.0)] = byte(keep);
            }
        }
    }
    // texel (0, 0) is cell (origin + lo); its centre that cell's
    let n = KEEP_SIDE as f32;
    let o = (k.origin.0 + lo.0 as i32, k.origin.1 + lo.1 as i32);
    let at = [
        1.0 / (RegionKeep::CELL * n),
        -o.0 as f32 / n,
        -o.1 as f32 / n,
        0.0,
    ];
    Some((texels, at))
}

/// What a surface drawn after the mask reads to darken itself
/// (`shadow_receive.wgsl`): the mask's uniform, the dynamic map and its
/// sampler, the static one and its, the regional keep and its.
pub(crate) struct Receive<'a> {
    pub(crate) uniform: &'a wgpu::Buffer,
    pub(crate) map: &'a wgpu::TextureView,
    pub(crate) sampler: &'a wgpu::Sampler,
    pub(crate) static_map: &'a wgpu::TextureView,
    pub(crate) static_sampler: &'a wgpu::Sampler,
    pub(crate) keep: &'a wgpu::TextureView,
    pub(crate) keep_sampler: &'a wgpu::Sampler,
}

/// The receiver inputs bound while `vc_shadows` is off: a zero uniform,
/// whose sun share of 0 leaves every colour as it is.
pub(crate) struct ReceiveOff {
    uniform: wgpu::Buffer,
    _map: wgpu::Texture,
    map: wgpu::TextureView,
    sampler: wgpu::Sampler,
    keep: Keep,
}

impl ReceiveOff {
    pub(crate) fn new(device: &wgpu::Device) -> ReceiveOff {
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("shadow receive off"),
            size: MASK_FLOATS as u64 * 4,
            usage: wgpu::BufferUsages::UNIFORM,
            mapped_at_creation: false,
        });
        let map = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("shadow receive off"),
            size: wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: MAP_FORMAT,
            usage: wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let view = map.create_view(&Default::default());
        ReceiveOff {
            uniform,
            _map: map,
            map: view,
            sampler: compare_sampler(device),
            keep: Keep::new(device, 1),
        }
    }

    pub(crate) fn receive(&self) -> Receive<'_> {
        Receive {
            uniform: &self.uniform,
            map: &self.map,
            sampler: &self.sampler,
            static_map: &self.map,
            static_sampler: &self.sampler,
            keep: &self.keep.view,
            keep_sampler: &self.keep.sampler,
        }
    }
}

/// The regional keep's texture ([`keep_texels`]), zero until a map's
/// measurement fills it.
struct Keep {
    tex: wgpu::Texture,
    view: wgpu::TextureView,
    sampler: wgpu::Sampler,
}

impl Keep {
    fn new(device: &wgpu::Device, side: u32) -> Keep {
        let tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("shadow keep"),
            size: wgpu::Extent3d {
                width: side,
                height: side,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        Keep {
            view: tex.create_view(&Default::default()),
            tex,
            sampler: device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some("shadow keep sampler"),
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                ..Default::default()
            }),
        }
    }

    fn write(&self, queue: &wgpu::Queue, texels: &[u8]) {
        let side = KEEP_SIDE as u32;
        queue.write_texture(
            self.tex.as_image_copy(),
            texels,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(side),
                rows_per_image: Some(side),
            },
            wgpu::Extent3d {
                width: side,
                height: side,
                depth_or_array_layers: 1,
            },
        );
    }
}

fn compare_sampler(device: &wgpu::Device) -> wgpu::Sampler {
    device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("shadow compare sampler"),
        address_mode_u: wgpu::AddressMode::ClampToEdge,
        address_mode_v: wgpu::AddressMode::ClampToEdge,
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        compare: Some(wgpu::CompareFunction::LessEqual),
        ..Default::default()
    })
}

/// The camera group's receiver entries, bindings 2..=8
/// (`shadow_receive.wgsl`).
pub(crate) fn receive_layout_entries() -> [wgpu::BindGroupLayoutEntry; 7] {
    let map = |binding| wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Depth,
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    };
    let sampler = |binding| wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Comparison),
        count: None,
    };
    [
        wgpu::BindGroupLayoutEntry {
            binding: 2,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: wgpu::BufferSize::new(MASK_FLOATS as u64 * 4),
            },
            count: None,
        },
        map(3),
        sampler(4),
        map(5),
        sampler(6),
        keep_layout_entry(7),
        keep_sampler_entry(8),
    ]
}

fn keep_layout_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: true },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    }
}

fn keep_sampler_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
        count: None,
    }
}

impl Receive<'_> {
    pub(crate) fn entries(&self) -> [wgpu::BindGroupEntry<'_>; 7] {
        [
            wgpu::BindGroupEntry {
                binding: 2,
                resource: self.uniform.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: wgpu::BindingResource::TextureView(self.map),
            },
            wgpu::BindGroupEntry {
                binding: 4,
                resource: wgpu::BindingResource::Sampler(self.sampler),
            },
            wgpu::BindGroupEntry {
                binding: 5,
                resource: wgpu::BindingResource::TextureView(self.static_map),
            },
            wgpu::BindGroupEntry {
                binding: 6,
                resource: wgpu::BindingResource::Sampler(self.static_sampler),
            },
            wgpu::BindGroupEntry {
                binding: 7,
                resource: wgpu::BindingResource::TextureView(self.keep),
            },
            wgpu::BindGroupEntry {
                binding: 8,
                resource: wgpu::BindingResource::Sampler(self.keep_sampler),
            },
        ]
    }
}

/// F3 readout: GPU ms of the last measured frame, smoothed.
#[derive(Default, Clone, Copy)]
pub(crate) struct ShadowStats {
    pub(crate) static_ms: f32,
    pub(crate) dynamic_ms: f32,
    pub(crate) mask_ms: f32,
    pub(crate) static_draws: u64,
    pub(crate) casters: usize,
    pub(crate) static_tris: u32,
    /// Of those, alpha-tested.
    pub(crate) static_cut: u32,
    /// Of those, drawn at the last static redraw, and of those alpha-tested.
    pub(crate) static_drawn: u32,
    pub(crate) static_cut_drawn: u32,
    /// CPU ms of `prepare` and `encode_maps`, smoothed.
    pub(crate) cpu_ms: f32,
}

pub(crate) struct ShadowPass {
    /// The static layer, [`STATIC_SIDE`] square and wrapping, then the
    /// dynamic one, the map's window.
    views: [wgpu::TextureView; 2],
    compare_sampler: wgpu::Sampler,
    /// The static layer's: it wraps (see [`on_torus`]).
    static_sampler: wgpu::Sampler,
    slots_buf: wgpu::Buffer,
    slots_bg: wgpu::BindGroup,
    world_pipeline: wgpu::RenderPipeline,
    /// Fills a static piece's viewport with the far depth.
    clear_pipeline: wgpu::RenderPipeline,
    /// Alpha-tested world casters: `[implicit, scripted stage]`.
    cut_pipelines: [wgpu::RenderPipeline; 2],
    entity_pipeline: wgpu::RenderPipeline,
    mask_layout: wgpu::BindGroupLayout,
    mask_buf: wgpu::Buffer,
    mask_pipeline: wgpu::RenderPipeline,
    static_held: Option<StaticHeld>,
    keep: Keep,
    /// The measurement `keep` holds, and where it sits in the world.
    keep_src: Option<Arc<RegionKeep>>,
    keep_at: [f32; 4],
    /// This frame's static redraw, set by [`ShadowPass::prepare`]: whether
    /// it starts from a cleared layer, and its pieces.
    static_full: bool,
    pieces: Vec<Piece>,
    moved: usize,
    timer: Option<Timer>,
    cpu_start: std::time::Instant,
    pub(crate) stats: ShadowStats,
}

impl ShadowPass {
    pub(crate) fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        scene_format: wgpu::TextureFormat,
        samples: u32,
        ent: EntLayouts<'_>,
        world: WorldLayouts<'_>,
    ) -> ShadowPass {
        let map = |label, side| {
            device
                .create_texture(&wgpu::TextureDescriptor {
                    label: Some(label),
                    size: wgpu::Extent3d {
                        width: side,
                        height: side,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: MAP_FORMAT,
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                        | wgpu::TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                })
                .create_view(&Default::default())
        };
        let views = [
            map("shadow map static", STATIC_SIDE as u32),
            map("shadow map dynamic", MAP_SIZE),
        ];
        let compare_sampler = compare_sampler(device);
        let static_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("shadow static compare sampler"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            compare: Some(wgpu::CompareFunction::LessEqual),
            ..Default::default()
        });

        let slots_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("shadow light slots layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: true,
                        min_binding_size: wgpu::BufferSize::new(64),
                    },
                    count: None,
                },
                // the cutout casters' alpha test reads images through it
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let cut_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("shadow cutout sampler"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            ..Default::default()
        });
        let slots_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("shadow light slots"),
            size: MAX_SLOTS as u64 * SLOT_STRIDE,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let slots_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("shadow light slots"),
            layout: &slots_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                        buffer: &slots_buf,
                        offset: 0,
                        size: wgpu::BufferSize::new(64),
                    }),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&cut_sampler),
                },
            ],
        });

        let cast = device.create_shader_module(wgpu::include_wgsl!("shadow_cast.wgsl"));
        // Casters draw two-sided; the receiver's offsets handle acne.
        let depth = Some(wgpu::DepthStencilState {
            format: MAP_FORMAT,
            depth_write_enabled: Some(true),
            depth_compare: Some(wgpu::CompareFunction::LessEqual),
            stencil: Default::default(),
            bias: wgpu::DepthBiasState {
                constant: 0,
                slope_scale: 1.0,
                clamp: 0.0,
            },
        });
        let world_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("shadow world layout"),
            bind_group_layouts: &[Some(&slots_layout)],
            immediate_size: 0,
        });
        let world_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("shadow world casters"),
            layout: Some(&world_layout),
            vertex: wgpu::VertexState {
                module: &cast,
                entry_point: Some("vs_world"),
                compilation_options: Default::default(),
                // the map's 44-byte vertex; only the position is read
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: 44,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![0 => Float32x3],
                })],
            },
            fragment: None,
            primitive: Default::default(),
            depth_stencil: depth.clone(),
            multisample: Default::default(),
            multiview_mask: None,
            cache: None,
        });
        let clear_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("shadow clear layout"),
            bind_group_layouts: &[],
            immediate_size: 0,
        });
        let clear_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("shadow static clear"),
            layout: Some(&clear_layout),
            vertex: wgpu::VertexState {
                module: &cast,
                entry_point: Some("vs_clear"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: None,
            primitive: Default::default(),
            depth_stencil: Some(wgpu::DepthStencilState {
                format: MAP_FORMAT,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::Always),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: Default::default(),
            multiview_mask: None,
            cache: None,
        });
        let cut = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("shadow_cut.wgsl"),
            source: wgpu::ShaderSource::Wgsl(SHADOW_CUT_WGSL.into()),
        });
        let cut_pipeline = |label, groups: &[Option<&wgpu::BindGroupLayout>], vs, fs| {
            let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some(label),
                bind_group_layouts: groups,
                immediate_size: 0,
            });
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &cut,
                    entry_point: Some(vs),
                    compilation_options: Default::default(),
                    // the map's 44-byte vertex: position, uv, normal, colour
                    buffers: &[Some(wgpu::VertexBufferLayout {
                        array_stride: 44,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &[
                            wgpu::VertexAttribute {
                                format: wgpu::VertexFormat::Float32x3,
                                offset: 0,
                                shader_location: 0,
                            },
                            wgpu::VertexAttribute {
                                format: wgpu::VertexFormat::Float32x2,
                                offset: 12,
                                shader_location: 1,
                            },
                            wgpu::VertexAttribute {
                                format: wgpu::VertexFormat::Float32x3,
                                offset: 28,
                                shader_location: 3,
                            },
                            wgpu::VertexAttribute {
                                format: wgpu::VertexFormat::Unorm8x4,
                                offset: 40,
                                shader_location: 4,
                            },
                        ],
                    })],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &cut,
                    entry_point: Some(fs),
                    compilation_options: Default::default(),
                    targets: &[],
                }),
                primitive: Default::default(),
                depth_stencil: depth.clone(),
                multisample: Default::default(),
                multiview_mask: None,
                cache: None,
            })
        };
        let cut_pipelines = [
            cut_pipeline(
                "shadow cutout casters",
                &[Some(&slots_layout), Some(world.material)],
                "vs_cut",
                "fs_cut",
            ),
            cut_pipeline(
                "shadow cutout stage casters",
                &[Some(&slots_layout), Some(world.material), Some(world.stage)],
                "vs_cut_stage",
                "fs_cut_stage",
            ),
        ];
        let entity_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("shadow entity layout"),
            bind_group_layouts: &[
                Some(&slots_layout),
                Some(ent.skin),
                Some(ent.bones),
                Some(ent.params),
            ],
            immediate_size: 0,
        });
        let entity_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("shadow entity casters"),
            layout: Some(&entity_layout),
            vertex: wgpu::VertexState {
                module: &cast,
                entry_point: Some("vs_entity"),
                compilation_options: Default::default(),
                buffers: &[Some(ent.vertex), Some(ent.instance)],
            },
            fragment: Some(wgpu::FragmentState {
                module: &cast,
                entry_point: Some("fs_entity"),
                compilation_options: Default::default(),
                targets: &[],
            }),
            primitive: Default::default(),
            depth_stencil: depth,
            multisample: Default::default(),
            multiview_mask: None,
            cache: None,
        });

        let mask_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("shadow mask layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: wgpu::BufferSize::new(MASK_FLOATS as u64 * 4),
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Depth,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: samples > 1,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Depth,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Comparison),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 4,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Comparison),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 5,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Depth,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                keep_layout_entry(6),
                keep_sampler_entry(7),
            ],
        });
        let mask_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("shadow mask uniform"),
            size: MASK_FLOATS as u64 * 4,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mask = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("shadow_mask.wgsl"),
            source: wgpu::ShaderSource::Wgsl(crate::renderer::depth_wgsl(include_str!(
                "shadow_mask.wgsl"
            ))),
        });
        let mask_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("shadow mask pipeline layout"),
            bind_group_layouts: &[Some(&mask_layout)],
            immediate_size: 0,
        });
        // dst * src on rgb, the frame's alpha kept
        let multiply = wgpu::BlendState {
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::Zero,
                dst_factor: wgpu::BlendFactor::Src,
                operation: wgpu::BlendOperation::Add,
            },
            alpha: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::Zero,
                dst_factor: wgpu::BlendFactor::One,
                operation: wgpu::BlendOperation::Add,
            },
        };
        let mask_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("shadow mask"),
            layout: Some(&mask_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &mask,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &mask,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: scene_format,
                    blend: Some(multiply),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: Default::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState {
                count: samples,
                ..Default::default()
            },
            multiview_mask: None,
            cache: None,
        });

        ShadowPass {
            views,
            compare_sampler,
            static_sampler,
            slots_buf,
            slots_bg,
            world_pipeline,
            clear_pipeline,
            cut_pipelines,
            entity_pipeline,
            mask_layout,
            mask_buf,
            mask_pipeline,
            static_held: None,
            keep: Keep::new(device, KEEP_SIDE as u32),
            keep_src: None,
            keep_at: [0.0; 4],
            static_full: false,
            pieces: Vec::new(),
            moved: 0,
            timer: Timer::new(device, queue),
            cpu_start: std::time::Instant::now(),
            stats: ShadowStats::default(),
        }
    }

    /// This frame's uniforms. `camera` is the frame's Camera block
    /// (`renderer::CAMERA_FLOATS`), whose fog tail the mask copies; `moved`
    /// the poses of the brush models off their spawn pose; `keep` the map's
    /// measured [`RegionKeep`] once known.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn prepare(
        &mut self,
        queue: &wgpu::Queue,
        sun: &Sun,
        casters: &Casters,
        view_proj: Mat4,
        eye: Vec3,
        fwd: Vec3,
        camera: &[f32],
        viewport: (f32, f32),
        moved: &[Mat4],
        keep: Option<&Arc<RegionKeep>>,
    ) {
        self.read_timer();
        self.set_keep(queue, keep);
        let keep = keep.map(|k| k.whole.keep());
        self.cpu_start = std::time::Instant::now();
        self.stats.static_tris = casters.tris;
        self.stats.static_cut = casters.cut_tris;
        let basis = light_basis(sun.dir, casters.bounds);
        let window = basis.window(eye, fwd);
        let light_vp = basis.matrix(TexRect::window(window));
        let key = (
            basis.matrix(TexRect::window((0, 0))).to_cols_array(),
            casters.id,
        );
        let view = TexRect::window(window);
        let (held, full, rects) = plan_static(self.static_held, key, view, basis.texel(eye));
        self.static_full = full;
        self.pieces = rects
            .into_iter()
            .flat_map(|r| on_torus(r, held.base))
            .map(|(r, at)| Piece {
                vp: basis.matrix(r),
                at,
                size: ((r.x1 - r.x0) as u32, (r.y1 - r.y0) as u32),
            })
            .collect();
        self.static_held = Some(held);
        self.moved = moved.len().min(MAX_SLOTS - 1 - STATIC_SLOTS);
        let slots = std::iter::once(light_vp).chain(moved.iter().map(|&pose| light_vp * pose));
        let write = |i: usize, m: Mat4| {
            queue.write_buffer(
                &self.slots_buf,
                i as u64 * SLOT_STRIDE,
                bytemuck::cast_slice(&m.to_cols_array()),
            )
        };
        for (i, m) in slots.take(self.moved + 1).enumerate() {
            write(i, m);
        }
        for (i, p) in self.pieces.iter().enumerate() {
            write(MAX_SLOTS - STATIC_SLOTS + i, p.vp);
        }
        let (lo, hi) = casters.bounds;
        let span = {
            let d = sun.dir.normalize();
            let ext = (hi - lo).abs();
            ext.x * d.x.abs() + ext.y * d.y.abs() + ext.z * d.z.abs() + 32.0
        };
        let mut u = [0.0f32; MASK_FLOATS];
        u[..16].copy_from_slice(&view_proj.inverse().to_cols_array());
        u[16..32].copy_from_slice(&light_vp.to_cols_array());
        u[32..36].copy_from_slice(&sun.dir.normalize().extend(0.0).to_array());
        u[36..40].copy_from_slice(&[sun_ratio(sun, keep), 1.0, DEPTH_BIAS / span, NORMAL_OFFSET]);
        u[40..44].copy_from_slice(&[1.0 / MAP_SIZE as f32, FADE_START, viewport.0, viewport.1]);
        // eye_fog_mode, fog_color_density, fog_range, view_fwd
        u[44..60].copy_from_slice(&camera[20..36]);
        // where the window sits in the wrapping static layer, in its uv
        let n = STATIC_SIDE as f32;
        let at = |v: i32, b: i32| (v - b).rem_euclid(STATIC_SIDE) as f32 / n;
        u[60..64].copy_from_slice(&[
            at(window.0, held.base.0),
            at(window.1, held.base.1),
            MAP_SIZE as f32 / n,
            1.0 / n,
        ]);
        // what the static layer holds, in the window's uv
        let v = held.valid;
        let w = |a: i32, b: i32| (a - b) as f32 / MAP_SIZE as f32;
        u[64..68].copy_from_slice(&[
            w(v.x0, view.x0),
            w(v.y0, view.y0),
            w(v.x1, view.x0),
            w(v.y1, view.y0),
        ]);
        u[68..72].copy_from_slice(&self.keep_at);
        queue.write_buffer(&self.mask_buf, 0, bytemuck::cast_slice(&u));
    }

    /// Leaves the surfaces after the mask undarkened this frame: no world
    /// or no sun, so [`ShadowPass::prepare`] did not run.
    pub(crate) fn idle(&self, queue: &wgpu::Queue) {
        queue.write_buffer(&self.mask_buf, 36 * 4, bytemuck::cast_slice(&[0.0f32]));
    }

    pub(crate) fn receive(&self) -> Receive<'_> {
        Receive {
            uniform: &self.mask_buf,
            map: &self.views[1],
            sampler: &self.compare_sampler,
            static_map: &self.views[0],
            static_sampler: &self.static_sampler,
            keep: &self.keep.view,
            keep_sampler: &self.keep.sampler,
        }
    }

    /// Fills the regional keep's texture when the map's measurement
    /// changed; zero (the whole map's ratio everywhere) without one.
    fn set_keep(&mut self, queue: &wgpu::Queue, keep: Option<&Arc<RegionKeep>>) {
        let same = match (keep, &self.keep_src) {
            (Some(a), Some(b)) => Arc::ptr_eq(a, b),
            (None, None) => true,
            _ => false,
        };
        if same {
            return;
        }
        self.keep_src = keep.cloned();
        let (texels, at) = keep
            .and_then(|k| keep_texels(k))
            .unwrap_or_else(|| (vec![0; KEEP_SIDE * KEEP_SIDE], [0.0; 4]));
        self.keep.write(queue, &texels);
        self.keep_at = at;
    }

    /// Draws `g`'s alpha-tested runs under the light slot at `slot`;
    /// `bound` tracks which cut pipeline is set. Returns the indices drawn.
    fn draw_cuts<'a>(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        g: &Group,
        slot: u32,
        cut: &dyn Fn(u32) -> Option<CutBinding<'a>>,
        bound: &mut Option<bool>,
    ) -> u32 {
        let mut drawn = 0;
        for (r, k) in &g.cuts {
            let Some(b) = cut(*k) else {
                continue;
            };
            let staged = b.stage.is_some();
            if *bound != Some(staged) {
                pass.set_pipeline(&self.cut_pipelines[usize::from(staged)]);
                *bound = Some(staged);
            }
            pass.set_bind_group(0, &self.slots_bg, &[slot]);
            pass.set_bind_group(1, b.mat, &[]);
            if let Some((stage, offset)) = b.stage {
                pass.set_bind_group(2, stage, &[offset]);
            }
            pass.draw_indexed(r.clone(), 0, 0..1);
            drawn += r.len() as u32;
        }
        drawn
    }

    /// The caster passes, before the frame's main pass: the static layer's
    /// pieces when the window or the world changed, the dynamic layer every
    /// frame.
    /// `cut` resolves an alpha-tested caster's batch to its bindings.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn encode_maps<'a>(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        world_vertices: &wgpu::Buffer,
        casters: &Casters,
        moved_models: &[usize],
        ents: &[EntCaster<'_>],
        eb: &EntBindings<'_>,
        cut: &dyn Fn(u32) -> Option<CutBinding<'a>>,
    ) {
        let pass_desc = |view, load, tw| wgpu::RenderPassDescriptor {
            label: Some("shadow casters"),
            color_attachments: &[],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view,
                depth_ops: Some(wgpu::Operations {
                    load,
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            timestamp_writes: tw,
            occlusion_query_set: None,
            multiview_mask: None,
        };
        let timer = self.timer.as_ref();
        if !self.pieces.is_empty() {
            self.stats.static_draws += 1;
            let load = if self.static_full {
                wgpu::LoadOp::Clear(1.0)
            } else {
                wgpu::LoadOp::Load
            };
            let mut pass = encoder.begin_render_pass(&pass_desc(
                &self.views[0],
                load,
                timer.and_then(|t| t.writes(0)),
            ));
            pass.set_vertex_buffer(0, world_vertices.slice(..));
            pass.set_index_buffer(casters.index_buf.slice(..), wgpu::IndexFormat::Uint32);
            let (mut drawn, mut cut_drawn) = (0, 0);
            for (i, p) in self.pieces.iter().enumerate() {
                let slot = ((MAX_SLOTS - STATIC_SLOTS + i) as u64 * SLOT_STRIDE) as u32;
                let (x, y) = (p.at.0 as f32, p.at.1 as f32);
                pass.set_viewport(x, y, p.size.0 as f32, p.size.1 as f32, 0.0, 1.0);
                if !self.static_full {
                    pass.set_pipeline(&self.clear_pipeline);
                    pass.draw(0..3, 0..1);
                }
                pass.set_pipeline(&self.world_pipeline);
                pass.set_bind_group(0, &self.slots_bg, &[slot]);
                // submodels at their spawn pose, where the bake saw them
                let groups: Vec<&Group> = casters
                    .cells
                    .iter()
                    .chain(&casters.model_ranges)
                    .filter(|(g, lo, hi)| g.indices() > 0 && in_light_box(&p.vp, *lo, *hi))
                    .map(|(g, _, _)| g)
                    .collect();
                for g in &groups {
                    if !g.solid.is_empty() {
                        drawn += g.solid.len() as u32;
                        pass.draw_indexed(g.solid.clone(), 0, 0..1);
                    }
                }
                let mut bound = None;
                for g in &groups {
                    cut_drawn += self.draw_cuts(&mut pass, g, slot, cut, &mut bound);
                }
            }
            self.stats.static_drawn = (drawn + cut_drawn) / 3;
            self.stats.static_cut_drawn = cut_drawn / 3;
        }
        let mut pass = encoder.begin_render_pass(&pass_desc(
            &self.views[1],
            wgpu::LoadOp::Clear(1.0),
            timer.and_then(|t| t.writes(1)),
        ));
        pass.set_pipeline(&self.world_pipeline);
        pass.set_vertex_buffer(0, world_vertices.slice(..));
        pass.set_index_buffer(casters.index_buf.slice(..), wgpu::IndexFormat::Uint32);
        for (i, &m) in moved_models.iter().take(self.moved).enumerate() {
            let Some((g, _, _)) = casters.model_ranges.get(m) else {
                continue;
            };
            if g.indices() == 0 {
                continue;
            }
            let slot = ((i + 1) as u64 * SLOT_STRIDE) as u32;
            pass.set_pipeline(&self.world_pipeline);
            pass.set_bind_group(0, &self.slots_bg, &[slot]);
            if !g.solid.is_empty() {
                pass.draw_indexed(g.solid.clone(), 0, 0..1);
            }
            self.draw_cuts(&mut pass, g, slot, cut, &mut None);
        }
        // every deformed caster is a stage cutout: the stage deforms it
        self.draw_cuts(&mut pass, &casters.deformed.0, 0, cut, &mut None);
        if !ents.is_empty() {
            pass.set_pipeline(&self.entity_pipeline);
            pass.set_bind_group(0, &self.slots_bg, &[0]);
            pass.set_bind_group(2, eb.bones, &[]);
            for c in ents {
                let off = u64::from(c.instance) * eb.instance_stride;
                pass.set_vertex_buffer(0, c.vertex_buf.slice(..));
                pass.set_vertex_buffer(1, eb.instance_buf.slice(off..off + eb.instance_stride));
                pass.set_index_buffer(c.index_buf.slice(..), wgpu::IndexFormat::Uint16);
                pass.set_bind_group(1, c.image, &[]);
                pass.set_bind_group(3, eb.params, &[c.params_offset]);
                pass.draw_indexed(
                    c.first_index..c.first_index + c.index_count,
                    c.base_vertex,
                    0..1,
                );
            }
        }
        self.stats.casters = ents.len();
        let ms = self.cpu_start.elapsed().as_secs_f32() * 1000.0;
        self.stats.cpu_ms = self.stats.cpu_ms * 0.9 + ms * 0.1;
    }

    /// The mask over the multisampled scene; its own pass between the
    /// frame's opaque draws and the rest.
    pub(crate) fn encode_mask(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        depth: &wgpu::TextureView,
    ) {
        let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("shadow mask"),
            layout: &self.mask_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: self.mask_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(depth),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&self.views[1]),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::Sampler(&self.compare_sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::Sampler(&self.static_sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: wgpu::BindingResource::TextureView(&self.views[0]),
                },
                wgpu::BindGroupEntry {
                    binding: 6,
                    resource: wgpu::BindingResource::TextureView(&self.keep.view),
                },
                wgpu::BindGroupEntry {
                    binding: 7,
                    resource: wgpu::BindingResource::Sampler(&self.keep.sampler),
                },
            ],
        });
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("shadow mask"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: self.timer.as_ref().and_then(|t| t.writes(2)),
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(&self.mask_pipeline);
        pass.set_bind_group(0, &bg, &[]);
        pass.draw(0..3, 0..1);
    }

    /// Resolves this frame's timestamps; call after the mask, before submit.
    pub(crate) fn resolve_timer(&mut self, encoder: &mut wgpu::CommandEncoder) {
        let static_due = (!self.pieces.is_empty()).then_some((
            self.pieces.len(),
            self.stats.static_drawn,
            self.static_full,
        ));
        let Some(t) = self.timer.as_mut().filter(|t| !t.pending) else {
            return;
        };
        encoder.resolve_query_set(&t.queries, 0..QUERIES, &t.resolve, 0);
        encoder.copy_buffer_to_buffer(&t.resolve, 0, &t.readback, 0, None);
        t.static_ran = static_due;
        t.pending = true;
        t.mapped.store(false, Ordering::Release);
    }

    /// Starts the readback; call after the frame's submit.
    pub(crate) fn after_submit(&mut self) {
        let Some(t) = self.timer.as_mut().filter(|t| t.pending) else {
            return;
        };
        if t.mapped.load(Ordering::Acquire) {
            return;
        }
        let flag = t.mapped.clone();
        t.readback.map_async(wgpu::MapMode::Read, .., move |r| {
            if r.is_ok() {
                flag.store(true, Ordering::Release);
            }
        });
    }

    fn read_timer(&mut self) {
        let Some(t) = self.timer.as_mut().filter(|t| t.pending) else {
            return;
        };
        if !t.mapped.load(Ordering::Acquire) {
            return;
        }
        let ticks: Vec<u64> = match t.readback.get_mapped_range(..) {
            Ok(view) => bytemuck::cast_slice(&view).to_vec(),
            Err(_) => return,
        };
        t.readback.unmap();
        t.mapped.store(false, Ordering::Release);
        t.pending = false;
        let ms = |i: usize| ticks[i + 1].wrapping_sub(ticks[i]) as f32 * t.period_ns / 1e6;
        let smooth = |old: &mut f32, new: f32| {
            *old = if *old == 0.0 {
                new
            } else {
                *old * 0.9 + new * 0.1
            }
        };
        if let Some((pieces, tris, full)) = t.static_ran {
            log::debug!(
                "shadow static redraw: {:.2} ms gpu, {pieces} pieces, {tris} tris{}",
                ms(0),
                if full { ", start-over" } else { "" }
            );
            smooth(&mut self.stats.static_ms, ms(0));
        }
        smooth(&mut self.stats.dynamic_ms, ms(2));
        smooth(&mut self.stats.mask_ms, ms(4));
        t.reads += 1;
        if t.reads.is_multiple_of(120) {
            let s = &self.stats;
            log::debug!(
                "shadow gpu (smoothed): dyn {:.2} mask {:.2} ms, cpu {:.2} ms, {} casters",
                s.dynamic_ms,
                s.mask_ms,
                s.cpu_ms,
                s.casters
            );
        }
    }

    /// The F3 line.
    pub(crate) fn debug_line(&self) -> String {
        let s = &self.stats;
        let head = format!(
            "shadow: ents {} static {}k/{}k tris cut {}k/{}k x{} cpu {:.2}ms",
            s.casters,
            s.static_drawn / 1000,
            s.static_tris / 1000,
            s.static_cut_drawn / 1000,
            s.static_cut / 1000,
            s.static_draws,
            s.cpu_ms
        );
        if self.timer.is_none() {
            return head;
        }
        format!(
            "{head} gpu static {:.2} dyn {:.2} mask {:.2}ms",
            s.static_ms, s.dynamic_ms, s.mask_ms
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ndc(m: Mat4, p: Vec3) -> Vec3 {
        let c = m * p.extend(1.0);
        c.truncate() / c.w
    }

    #[test]
    fn light_matrix_frames_the_view_and_the_world_depth() {
        let sun = Vec3::new(-0.6, -0.35, 0.7).normalize();
        let bounds = (
            Vec3::new(-3000.0, -2000.0, -500.0),
            Vec3::new(2500.0, 3000.0, 1200.0),
        );
        let eye = Vec3::new(100.0, 200.0, 50.0);
        let m = light_matrix(sun, eye, Vec3::X, bounds);
        // the eye sits inside the map, short of its centre by about AHEAD
        let e = ndc(m, eye);
        assert!(e.x.abs() < 1.0 && e.y.abs() < 1.0, "{e}");
        let ahead = ndc(m, eye + Vec3::X * AHEAD);
        let snap_ndc = SNAP / HALF_EXTENT;
        assert!(
            ahead.x.abs() <= snap_ndc && ahead.y.abs() <= snap_ndc,
            "{ahead}"
        );
        // depth: the world's corners land in 0..1, nearer the sun is smaller
        for i in 0..8 {
            let c = Vec3::new(
                if i & 1 == 0 { bounds.0.x } else { bounds.1.x },
                if i & 2 == 0 { bounds.0.y } else { bounds.1.y },
                if i & 4 == 0 { bounds.0.z } else { bounds.1.z },
            );
            let z = ndc(m, c).z;
            assert!((0.0..=1.0).contains(&z), "{c} -> {z}");
        }
        assert!(ndc(m, eye + sun * 100.0).z < e.z);
        // a point straight along the sun ray shares the eye's texel
        let up = ndc(m, eye + sun * 500.0);
        assert!((up.truncate() - e.truncate()).length() < 1e-4);
    }

    #[test]
    fn a_group_puts_solid_casters_first_and_one_run_per_cutting_batch() {
        let cpu: Vec<u32> = (0..12).collect();
        let r = |first, cut| CasterRange {
            first,
            count: 3,
            bounds: (Vec3::splat(first as f32), Vec3::splat(first as f32 + 1.0)),
            cut,
        };
        let rs = [r(0, Some(7)), r(3, None), r(6, Some(7)), r(9, Some(2))];
        let mut out = vec![99];
        let (g, lo, hi) = push_group(&mut out, &cpu, rs.iter().collect());
        assert_eq!(g.solid, 1..4);
        assert_eq!(g.cuts, vec![(4..7, 2), (7..13, 7)]);
        assert_eq!(&out[1..4], &[3, 4, 5]);
        assert_eq!(&out[4..7], &[9, 10, 11]);
        assert_eq!(g.indices(), 12);
        assert_eq!((lo, hi), (Vec3::ZERO, Vec3::splat(10.0)));
    }

    #[test]
    fn light_box_test_keeps_what_it_reaches() {
        let sun = Vec3::new(0.0, 0.0, 1.0);
        let bounds = (Vec3::splat(-8000.0), Vec3::splat(8000.0));
        let m = light_matrix(sun, Vec3::ZERO, Vec3::X, bounds);
        let at = |x: f32| {
            in_light_box(
                &m,
                Vec3::new(x, 0.0, 0.0),
                Vec3::new(x + 100.0, 100.0, 50.0),
            )
        };
        assert!(at(0.0));
        // the box spans AHEAD +- HALF_EXTENT along x, give or take a snap
        assert!(at(AHEAD + HALF_EXTENT - SNAP - 50.0));
        assert!(!at(AHEAD + HALF_EXTENT + SNAP + 10.0));
        assert!(!at(AHEAD - HALF_EXTENT - SNAP - 200.0));
    }

    #[test]
    fn light_matrix_holds_still_within_a_snap_step() {
        let sun = Vec3::new(0.3, 0.4, 0.8).normalize();
        let bounds = (Vec3::splat(-4000.0), Vec3::splat(4000.0));
        let a = light_matrix(sun, Vec3::new(10.0, 10.0, 0.0), Vec3::Y, bounds);
        let b = light_matrix(sun, Vec3::new(14.0, 12.0, 0.0), Vec3::Y, bounds);
        assert_eq!(a, b);
    }

    #[test]
    fn the_static_layer_holds_the_window_as_it_moves_and_fills_in_by_budget() {
        // the layer on a lattice of its texels: which global texel each holds
        const LATTICE: usize = 7;
        let n = STATIC_SIDE as usize;
        let mut layer: Vec<Option<(i32, i32)>> = vec![None; n * n];
        let step = (SNAP / TEXEL) as i32;
        let key_a = ([1.0; 16], 1);
        let key_b = ([1.0; 16], 2);
        let mut held: Option<StaticHeld> = None;
        let mut corner = (5 * step - SIDE / 2, -3 * step - SIDE / 2);
        // per frame: the window's move in snap steps, and the light; the
        // eye sits AHEAD behind the window's centre
        let mut frames = vec![((0, 0), key_a); 3];
        for d in [(1, 0), (1, 1), (-2, 0), (0, -3), (-1, 2), (6, 0), (40, 0)] {
            frames.push((d, key_a));
            frames.extend([((0, 0), key_a); 20]);
        }
        frames.push(((0, 0), key_b));
        // long enough to fill the whole layer from the core
        let fill = (STATIC_SIDE * STATIC_SIDE / BUDGET) as usize + 64;
        frames.extend(std::iter::repeat_n(((0, 0), key_b), fill));
        let mut window_whole_after = None;
        for (i, (d, key)) in frames.into_iter().enumerate() {
            corner = (corner.0 + d.0 * step, corner.1 + d.1 * step);
            let view = TexRect::window(corner);
            let eye = (
                corner.0 + SIDE / 2,
                corner.1 + SIDE / 2 + (AHEAD / TEXEL) as i32,
            );
            let around = |side: i32| TexRect {
                x0: eye.0 - side / 2,
                y0: eye.1 - side / 2,
                x1: eye.0 + side / 2,
                y1: eye.1 + side / 2,
            };
            let (core, keep) = (around(CORE), around(CORE / 2));
            let was = held;
            let (h, full, rects) = plan_static(held, key, view, eye);
            let fresh = was.is_none_or(|w| w.key != key || !w.valid.contains(&keep));
            assert_eq!(full, fresh, "frame {i}");
            if full {
                layer.fill(None);
                assert_eq!(rects, vec![core], "a start-over draws the core");
                assert_eq!(on_torus(rects[0], h.base).len(), 1, "in one piece");
                // the quiet spell after the light change
                window_whole_after = (key == key_b).then_some(i);
            } else {
                // never more than the budget a frame, in one strip
                assert!(rects.len() <= 1, "frame {i}");
                let drawn: i64 = rects.iter().map(TexRect::area).sum();
                assert!(drawn <= i64::from(BUDGET), "frame {i}: {drawn}");
                // the window comes before the margin
                if let Some(r) = rects.first()
                    && !was.unwrap().valid.contains(&view)
                {
                    assert!(r.intersect(&view).area() > 0, "frame {i}: {r:?}");
                }
            }
            for r in &rects {
                let pieces = on_torus(*r, h.base);
                assert!(pieces.len() <= STATIC_SLOTS);
                for (p, at) in pieces {
                    assert!(at.0 as i32 + p.x1 - p.x0 <= STATIC_SIDE);
                    assert!(at.1 as i32 + p.y1 - p.y0 <= STATIC_SIDE);
                    for y in p.y0..p.y1 {
                        for x in p.x0..p.x1 {
                            let u = at.0 as usize + (x - p.x0) as usize;
                            let v = at.1 as usize + (y - p.y0) as usize;
                            if u.is_multiple_of(LATTICE) && v.is_multiple_of(LATTICE) {
                                layer[v * n + u] = Some((x, y));
                            }
                        }
                    }
                }
            }
            assert!(h.valid.contains(&keep), "frame {i}");
            assert!(h.valid.x1 - h.valid.x0 <= STATIC_SIDE);
            assert!(h.valid.y1 - h.valid.y0 <= STATIC_SIDE);
            // the one texel of `valid` stored at each lattice spot
            let held_at = |u: usize, lo: i32, hi: i32, base: i32| {
                let x = lo + (u as i32 - (lo - base)).rem_euclid(STATIC_SIDE);
                (x < hi).then_some(x)
            };
            for v in (0..n).step_by(LATTICE) {
                let Some(y) = held_at(v, h.valid.y0, h.valid.y1, h.base.1) else {
                    continue;
                };
                for u in (0..n).step_by(LATTICE) {
                    if let Some(x) = held_at(u, h.valid.x0, h.valid.x1, h.base.0) {
                        assert_eq!(layer[v * n + u], Some((x, y)), "frame {i}");
                    }
                }
            }
            if h.valid.contains(&view)
                && let Some(start) = window_whole_after.take()
            {
                // the window fills within about its area's worth of budgets
                let frames = i - start;
                let most = (SIDE * SIDE / BUDGET) as usize + 16;
                assert!(
                    frames <= most,
                    "window whole {frames} frames after a start-over"
                );
            }
            held = Some(h);
        }
        assert_eq!(window_whole_after, None, "the window filled in");
        // after a quiet spell the layer holds the margin all round
        assert_eq!(held.unwrap().valid, TexRect::window(corner).grown(MARGIN));
    }

    #[test]
    fn a_running_eye_is_followed_without_starting_over() {
        // 400 units/s at 60 frames/s on a diagonal, the window ahead of it
        let key = ([1.0; 16], 1);
        let step = (SNAP / TEXEL) as i32;
        let (mut held, mut whole_at) = (None, None);
        for i in 0..900 {
            let t = i as f32 * 400.0 / 60.0 / TEXEL * std::f32::consts::FRAC_1_SQRT_2;
            let eye = (t as i32, t as i32);
            let snap = |v: i32| (v as f32 / step as f32).round() as i32 * step;
            let view = TexRect::window((snap(eye.0) - SIDE / 2, snap(eye.1 - 512) - SIDE / 2));
            let (h, full, _) = plan_static(held, key, view, eye);
            assert_eq!(full, i == 0, "frame {i}");
            if whole_at.is_none() && h.valid.contains(&view) {
                whole_at = Some(i);
            }
            held = Some(h);
        }
        assert!(whole_at.is_some_and(|i| i < 200), "{whole_at:?}");
    }

    #[test]
    fn a_piece_matrix_lands_its_texels_where_the_window_does() {
        let sun = Vec3::new(0.3, 0.4, 0.8).normalize();
        let b = light_basis(sun, (Vec3::splat(-4000.0), Vec3::splat(4000.0)));
        let w = TexRect::window(b.window(Vec3::new(300.0, -700.0, 40.0), Vec3::Y));
        let full = b.matrix(w);
        let piece = TexRect {
            x0: w.x0 + 1000,
            y0: w.y0 + 300,
            x1: w.x0 + 1128,
            y1: w.y1,
        };
        let pm = b.matrix(piece);
        // a world point's texel, through the window and through the piece
        let p = b.right * ((w.x0 + 1050) as f32 + 0.25) * TEXEL
            - b.up * ((w.y0 + 400) as f32 + 0.75) * TEXEL
            + b.dir * 37.0;
        let texel = |m: Mat4, r: TexRect| {
            let n = ndc(m, p);
            (
                r.x0 as f32 + (n.x * 0.5 + 0.5) * (r.x1 - r.x0) as f32,
                r.y0 as f32 + (0.5 - n.y * 0.5) * (r.y1 - r.y0) as f32,
                n.z,
            )
        };
        let (a, c) = (texel(full, w), texel(pm, piece));
        assert!(
            (a.0 - c.0).abs() < 1e-2 && (a.1 - c.1).abs() < 1e-2,
            "{a:?} {c:?}"
        );
        assert!((a.0 - (w.x0 + 1050) as f32 - 0.25).abs() < 1e-2, "{a:?}");
        assert_eq!(a.2, c.2);
    }

    #[test]
    fn a_measured_keep_sets_the_flat_ground_darkness() {
        let sun = Sun {
            dir: Vec3::new(0.6, 0.0, 0.8),
            sun: 0.2,
            sky: 0.15,
            ambient: 0.05,
        };
        let kept = |ratio: f32, ndl: f32| 1.0 / (ratio * ndl + 1.0);
        assert!((kept(sun_ratio(&sun, None), 1.0) - 0.5).abs() < 1e-6);
        let r = sun_ratio(&sun, Some(0.4));
        assert!((kept(r, 0.8) - 0.4).abs() < 1e-6);
        // a map that measured no darker in the shade casts nothing
        assert_eq!(sun_ratio(&sun, Some(1.2)), 0.0);
        assert!((kept(sun_ratio(&sun, Some(0.0)), 0.8) - KEEP_MIN).abs() < 1e-6);
    }

    #[test]
    fn the_keep_texture_puts_each_known_cell_at_its_spot() {
        use vcod_common::static_light::SunKeep;
        let whole = SunKeep {
            lit: 0.5,
            shadowed: 0.3,
            lit_n: 100,
            shadowed_n: 100,
        };
        // 4 x 3 cells from cell (-2, 5); known: (-1, 5) 0.4, (1, 7) 0.1
        let mut keeps = vec![None; 12];
        keeps[1] = Some(0.4);
        keeps[2 * 4 + 3] = Some(0.1);
        let k = RegionKeep {
            whole,
            origin: (-2, 5),
            size: (4, 3),
            keeps,
        };
        let (texels, at) = keep_texels(&k).unwrap();
        let byte = |v: f32| (v * 255.0).round() as u8;
        // the known cells' box starts at cell (-1, 5)
        assert_eq!(texels[0], byte(0.4));
        assert_eq!(texels[2 * KEEP_SIDE + 2], byte(KEEP_MIN), "clamped");
        assert_eq!(texels[1], byte(0.6), "unknown: the whole map's");
        assert_eq!(texels[KEEP_SIDE * KEEP_SIDE - 1], byte(0.6));
        assert!(texels.iter().all(|&t| t > 0));
        // a world point at the centre of cell (1, 7) lands on its texel's centre
        let p = glam::Vec2::new(1.5, 7.5) * RegionKeep::CELL;
        let uv = p * at[0] + glam::Vec2::new(at[1], at[2]);
        let n = KEEP_SIDE as f32;
        assert!((uv - glam::Vec2::new(2.5, 2.5) / n).length() < 1e-6, "{uv}");
        // nothing known, or more than the texture holds: the whole map's ratio
        let none = RegionKeep {
            keeps: vec![None; 12],
            ..k.clone()
        };
        assert!(keep_texels(&none).is_none());
        let mut wide = vec![None; KEEP_SIDE + 1];
        wide[0] = Some(0.5);
        wide[KEEP_SIDE] = Some(0.5);
        let wide = RegionKeep {
            size: (KEEP_SIDE + 1, 1),
            keeps: wide,
            ..k
        };
        assert!(keep_texels(&wide).is_none());
    }

    #[test]
    fn mask_and_cast_shaders_validate() {
        use wgpu::naga;
        for src in [
            include_str!("shadow_cast.wgsl"),
            SHADOW_CUT_WGSL,
            include_str!("shadow_mask.wgsl"),
        ] {
            let module = naga::front::wgsl::parse_str(src)
                .unwrap_or_else(|e| panic!("{}", e.emit_to_string(src)));
            naga::valid::Validator::new(
                naga::valid::ValidationFlags::all(),
                naga::valid::Capabilities::default(),
            )
            .validate(&module)
            .unwrap_or_else(|e| panic!("{}", e.emit_to_string(src)));
        }
    }
}
