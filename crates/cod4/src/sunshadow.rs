//! CoD4 1.7's sun shadow map: where `R_SetupSunShadowMaps` (0x639900)
//! puts its two partitions for a view, the planes it culls each one's
//! casters by, and the constants the technique 9 shaders read
//! (`shadowLookupMatrix`, `shadowmapSwitchPartition`, `shadowmapScale`)
//! and the caster pass writes with (`shadowmapPolygonOffset`).
//! `docs/research/cod4-world.md` 3.6 has the addresses. The renderer is
//! the shader model 3 one (`r_rendererInUse` 1), so `rg.sunShadowFull` is
//! set: `sm_sunShadowScale` shrinks the far partition's square and the far
//! partition reaches past the near one's end to its square's edge.
//!
//! Matrices here take column vectors: `m * (p, 1)`.

use glam::{Mat4, Vec2, Vec3, Vec4};
use vcod_common::mapworld::MapWorld;

/// A partition's side in texels; the atlas stacks the near partition over
/// the far one, 1024 x 2048.
pub const PARTITION: u32 = 1024;
/// `rg.sunShadowPartitionRatio` at `sm_sunShadowScale` 1: the far
/// partition's sample size over the near one's.
pub const RATIO: f32 = 4.0;
/// `sm_sunSampleSizeNear`'s default: world units a near texel.
pub const SAMPLE_SIZE_NEAR: f32 = 0.25;
/// `sm_polygonOffsetBias` and `sm_polygonOffsetScale`'s defaults.
pub const POLYGON_OFFSET_BIAS: f32 = 0.5;
pub const POLYGON_OFFSET_SCALE: f32 = 2.0;
/// `sm_sunShadowScale`'s range (0x62c3da, 0x62c3e3).
pub const SHADOW_SCALE_MIN: f32 = 0.25;

/// The sun shadow dvars `R_SetupSunShadowMaps` reads.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Settings {
    /// `sm_sunSampleSizeNear`.
    pub sample_size_near: f32,
    /// `sm_sunShadowScale`, in `SHADOW_SCALE_MIN..=1`.
    pub scale: f32,
    /// `sm_sunShadowCenter`: the far partition's centre when not 0 0 0.
    pub center: Vec3,
}

impl Default for Settings {
    fn default() -> Settings {
        Settings {
            sample_size_near: SAMPLE_SIZE_NEAR,
            scale: 1.0,
            center: Vec3::ZERO,
        }
    }
}

/// What `R_GenerateSortedDrawSurfs` makes of `sm_sunShadowScale`
/// (0x5f9966-0x5f9afe): the nearest fraction `num / den` with `den` in
/// 1..=10, the far partition's used side `ceil(scale * 1024)` and the
/// partition ratio `4 / scale`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Scale {
    pub scale: f32,
    pub num: f32,
    pub size: u32,
    pub ratio: f32,
}

impl Scale {
    pub fn of(dvar: f32) -> Scale {
        // each denominator's nearest numerator, `floor(den * dvar + 0.5)`;
        // a later fraction wins only on a strictly smaller error, which
        // starts at 1 for 1/1
        let candidate = |den: u32| {
            let num = (den as f32 * dvar + 0.5).floor() as u32;
            (num, den, (num as f32 / den as f32 - dvar).abs())
        };
        let (num, den, _) =
            (1..=10).map(candidate).fold(
                (1, 1, 1.0f32),
                |best, c| {
                    if c.2 < best.2 { c } else { best }
                },
            );
        let scale = num as f32 / den as f32;
        Scale {
            scale,
            num: num as f32,
            size: (scale * 1024.0).ceil() as u32,
            ratio: 4.0 / scale,
        }
    }
}

/// The view the partitions are fitted around (`GfxViewParms`): its origin,
/// forward axis, near plane's distance and that plane's four corners, and
/// its four side planes (`dpvsGlob.sideFrustumPlanes`, each positive
/// inside).
#[derive(Clone, Copy, Debug)]
pub struct View {
    pub origin: Vec3,
    pub forward: Vec3,
    pub z_near: f32,
    pub near_corners: [Vec3; 4],
    pub side_planes: [Vec4; 4],
}

impl View {
    /// The view of a camera whose view-projection (depth 0 at the near
    /// plane) is `view_proj`: the corners are its inverse at clip depth 0
    /// (`R_GetFrustumNearClipPoints`), the side planes clip x and y at
    /// plus and minus w brought back to the world.
    pub fn of(view_proj: Mat4, origin: Vec3, forward: Vec3, z_near: f32) -> View {
        let inv = view_proj.inverse();
        let near_corners = [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)]
            .map(|(x, y)| inv.project_point3(Vec3::new(x, y, 0.0)));
        let side_planes = [
            Vec4::new(1.0, 0.0, 0.0, 1.0),
            Vec4::new(-1.0, 0.0, 0.0, 1.0),
            Vec4::new(0.0, 1.0, 0.0, 1.0),
            Vec4::new(0.0, -1.0, 0.0, 1.0),
        ]
        .map(|c| normalized(view_proj.transpose() * c));
        View {
            origin,
            forward,
            z_near,
            near_corners,
            side_planes,
        }
    }
}

/// A plane scaled to a unit normal.
fn normalized(p: Vec4) -> Vec4 {
    p / p.truncate().length()
}

/// The world planes a partition culls its casters by
/// (`R_SetupShadowSurfacesDpvs`, 0x60b850), each positive inside: the
/// bounding polygon's edges, then the near and far region planes and the
/// view's side planes that face against the sun.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CullPlanes {
    pub count: usize,
    pub planes: [Vec4; 20],
}

impl CullPlanes {
    /// Past the last slot a plane is dropped, which only culls less.
    fn push(&mut self, p: Vec4) {
        if let Some(slot) = self.planes.get_mut(self.count) {
            *slot = p;
            self.count += 1;
        }
    }

    pub fn planes(&self) -> &[Vec4] {
        &self.planes[..self.count]
    }

    /// Whether the box `lo..hi` lies wholly on the outside of a plane, at
    /// or past it (the cell test in `R_AddWorldSurfacesFrustumOnly`): its
    /// casters are not drawn.
    pub fn culls(&self, lo: Vec3, hi: Vec3) -> bool {
        self.planes().iter().any(|p| {
            let far = Vec3::select(p.truncate().cmpge(Vec3::ZERO), hi, lo);
            p.truncate().dot(far) + p.w <= 0.0
        })
    }
}

/// One partition: the matrix from the world to its clip space (z in 0..1),
/// the rectangle of its 1024 square the casters are drawn in (x, y, width,
/// height), its `shadowmapPolygonOffset` and its casters' cull planes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Partition {
    pub view_proj: Mat4,
    pub viewport: [u32; 4],
    pub polygon_offset: Vec4,
    pub cull: CullPlanes,
}

/// Both partitions and the receivers' constants.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SunShadow {
    pub partitions: [Partition; 2],
    /// `shadowLookupMatrix`: the world to the near partition's atlas
    /// coordinates (x, y, depth) and, in w, the distance along the view
    /// over the near partition's reach (1 at its end).
    pub lookup: Mat4,
    /// `shadowmapSwitchPartition`: the far partition's coordinates are the
    /// near one's times w plus xy.
    pub switch_partition: Vec4,
    /// `shadowmapScale`: how fast the far partition's edge fades out.
    pub scale: Vec4,
}

/// `R_GetSunAxes` (0x63dc10): forward away from the sun, then right and up
/// off world up, or off x when the sun stands within 0.1 of overhead in
/// its squared horizontal length.
pub fn sun_axes(to_sun: Vec3) -> [Vec3; 3] {
    let f = -to_sun;
    let seed = if f.x * f.x + f.y * f.y >= 0.1 {
        Vec3::Z
    } else {
        Vec3::X
    };
    let right = seed.cross(f).normalize();
    [f, right, f.cross(right)]
}

/// The convex hull of `points` (`Com_ConvexHull`, 0x55ad30), each kept
/// with its index; fewer than three in a line give what is left.
fn convex_hull(points: &[Vec2]) -> Vec<(Vec2, usize)> {
    let mut p: Vec<(Vec2, usize)> = points.iter().copied().zip(0..).collect();
    p.sort_by(|a, b| a.0.x.total_cmp(&b.0.x).then(a.0.y.total_cmp(&b.0.y)));
    p.dedup_by(|a, b| a.0 == b.0);
    if p.len() < 3 {
        return p;
    }
    let cross = |o: Vec2, a: Vec2, b: Vec2| (a - o).perp_dot(b - o);
    let mut hull: Vec<(Vec2, usize)> = Vec::new();
    for pass in 0..2 {
        let start = hull.len();
        let iter: Box<dyn Iterator<Item = &(Vec2, usize)>> = if pass == 0 {
            Box::new(p.iter())
        } else {
            Box::new(p.iter().rev())
        };
        for &q in iter {
            while hull.len() >= start + 2
                && cross(hull[hull.len() - 2].0, hull[hull.len() - 1].0, q.0) <= 0.0
            {
                hull.pop();
            }
            hull.push(q);
        }
        hull.pop();
    }
    hull
}

/// A partition's bounding polygon in its clip space and which of its
/// points are the far partition's inner ones (`R_SetupSunShadowBoundingPoly`,
/// 0x637e80): the hull of `points` in the sun's plane, scaled about the
/// view by `2 / max_size`; `R_SunShadowMapBoundingPoly` (0x637fc0) then
/// moves it onto the partition's view.
fn bounding_poly(points: &[Vec2], org: Vec2, max_size: f32) -> Vec<(Vec2, bool)> {
    let to_clip = 2.0 / max_size;
    convex_hull(points)
        .into_iter()
        .map(|(p, i)| ((p - org) * to_clip, points.len() == 8 && i >= 4))
        .collect()
}

/// The polygon's edge planes in clip x and y, positive inside
/// (`R_SunShadowMapClipSpaceClipPlanes`, 0x638040): every edge for the near
/// partition; for the far one only the edges that touch an inner point,
/// then its square's four sides at plus and minus `scale`.
fn clip_planes(poly: &[(Vec2, bool)], far: bool, scale: f32) -> Vec<Vec4> {
    let mut out = Vec::new();
    let n = poly.len();
    // the hull winds counter-clockwise, so the inside is to the left
    for i in 0..n {
        let (a, an) = poly[i];
        let (b, bn) = poly[(i + 1) % n];
        if far && !an && !bn {
            continue;
        }
        let normal = Vec2::new(a.y - b.y, b.x - a.x);
        out.push(Vec4::new(normal.x, normal.y, 0.0, -normal.dot(a)));
    }
    if far {
        for (x, y) in [(1.0, 0.0), (-1.0, 0.0), (0.0, 1.0), (0.0, -1.0)] {
            out.push(Vec4::new(x, y, 0.0, scale));
        }
    }
    out
}

/// A box on the sun's plane.
#[derive(Clone, Copy, Debug)]
struct Rect {
    lo: Vec2,
    hi: Vec2,
}

impl Rect {
    fn around(points: &[Vec2]) -> Rect {
        let empty = Rect {
            lo: Vec2::splat(f32::MAX),
            hi: Vec2::splat(-f32::MAX),
        };
        empty.with(points)
    }

    fn with(self, points: &[Vec2]) -> Rect {
        points.iter().fold(self, |r, &p| Rect {
            lo: r.lo.min(p),
            hi: r.hi.max(p),
        })
    }
}

/// Where one axis of the atlas puts the view: its texel in each
/// partition and each partition's viewport span (start, length) on the
/// axis.
struct Span {
    texel: [f32; 2],
    start: [u32; 2],
    len: [u32; 2],
}

/// One axis's distances, in world units on the sun's plane, all measured
/// from the side the atlas axis starts at (x: the low side, y: the high
/// side, the texture's y running down the sun's up).
struct AxisGaps {
    /// The view sits in the near box's half the axis starts at.
    view_leads: bool,
    /// From the near box's start edge to the view, and from the view to
    /// its end edge.
    lead: f32,
    trail: f32,
    /// The far box's start edge past the near box's, and the near box's
    /// end edge past the far box's.
    far_lead: f32,
    far_trail: f32,
    /// The near box's extent and the view's offset from
    /// `sm_sunShadowCenter`, when that is set.
    extent: f32,
    from_center: Option<f32>,
}

/// The atlas constants a placement reads: the near fit (1023 texels over
/// the near box's longer side), the far square's first and past-last
/// texel, its share of the 1024 and its sample size.
struct Fit {
    texels_per_unit: f32,
    max_size: f32,
    far_begin: u32,
    far_end: u32,
    far_share: f32,
    far_sample: f32,
    far_size: u32,
}

impl Fit {
    /// 0x638e5b-0x639491, one axis: the near partition hugs the atlas
    /// edge on the view's side, the far one the far square's edge there
    /// (or, under `sm_sunShadowCenter`, puts the centre at texel 512).
    fn place(&self, g: &AxisGaps) -> Span {
        let side = PARTITION as f32;
        let (begin, end) = (self.far_begin as f32, self.far_end as f32);
        let near_len = (side / self.max_size * g.extent - 0.001).ceil() as u32;
        let near_start = if g.view_leads {
            0
        } else {
            PARTITION - near_len
        };
        let near_texel = if g.view_leads {
            g.lead * self.texels_per_unit + 1.0
        } else {
            1023.0 - g.trail * self.texels_per_unit
        };
        let (far_texel, far_start, far_len) = match (g.from_center, g.view_leads) {
            (Some(off), true) => (512.0 + off / self.far_sample, self.far_begin, None),
            (Some(off), false) => (
                512.0 - off / self.far_sample,
                self.far_begin,
                Some(self.far_size),
            ),
            (None, true) => {
                let texel = begin + 1.0 + g.lead * self.texels_per_unit * self.far_share;
                let start = (self.texels_per_unit * g.far_lead + begin)
                    .floor()
                    .max(begin) as u32;
                (texel, start, None)
            }
            (None, false) => {
                let texel = end - 1.0 - g.trail * self.texels_per_unit * self.far_share;
                let reach = (end - self.texels_per_unit * g.far_trail).ceil() as i32;
                // vcod's: a span 1.7 would store negative is empty
                let len = (reach - self.far_begin as i32).clamp(0, self.far_size as i32);
                (texel, self.far_begin, Some(len as u32))
            }
        };
        // a span starting at the view's edge runs to the far square's end
        let far_len = far_len.unwrap_or_else(|| self.far_end.saturating_sub(far_start));
        Span {
            texel: [near_texel, far_texel],
            start: [near_start, far_start],
            len: [near_len, far_len],
        }
    }
}

/// A plane times `sign` over its normal's length, as 1.7 scales one:
/// by the reciprocal (0x605f3c, 0x6383e9, 0x6385de).
fn unit_plane(p: Vec4, sign: f32) -> Vec4 {
    p * (sign / p.truncate().length())
}

/// The partitions for `view` under a sun toward `to_sun` over a world in
/// `bounds` at the dvars `settings` (`R_SetupSunShadowMaps`, 0x639900;
/// the placement is 0x6386c0, cod4-world.md 3.6).
pub fn setup(view: &View, to_sun: Vec3, bounds: (Vec3, Vec3), settings: &Settings) -> SunShadow {
    let sc = Scale::of(settings.scale);
    let axes = sun_axes(to_sun);
    let [forward, right, up] = axes;
    // the sun's plane: right negated, up
    let on_plane = |p: Vec3| Vec2::new(-p.dot(right), p.dot(up));
    let eye = on_plane(view.origin);
    let corners = view.near_corners.map(on_plane);
    let toward_eye = 0.75 / sc.ratio;
    let pulled = corners.map(|c| (c - eye) * toward_eye + eye);
    let far_box = Rect::around(&corners).with(&pulled);
    let near_box = Rect::around(&corners).with(&[eye]);
    let extent = near_box.hi - near_box.lo;
    let max_size = extent.x.max(extent.y);

    let near_sample = settings.sample_size_near;
    let samples = [near_sample, near_sample * sc.ratio];
    let snap = samples[1] * sc.num;
    let snapped = (eye / snap).floor() * snap;
    let far_begin = (PARTITION - sc.size) / 2;
    let fit = Fit {
        texels_per_unit: 1023.0 / max_size,
        max_size,
        far_begin,
        far_end: PARTITION - far_begin,
        far_share: sc.size as f32 / PARTITION as f32,
        far_sample: samples[1],
        far_size: sc.size,
    };
    let from_center = (settings.center != Vec3::ZERO).then(|| eye - on_plane(settings.center));
    let mid = (near_box.lo + near_box.hi) * 0.5;
    let x = fit.place(&AxisGaps {
        view_leads: eye.x < mid.x,
        lead: eye.x - near_box.lo.x,
        trail: near_box.hi.x - eye.x,
        far_lead: far_box.lo.x - near_box.lo.x,
        far_trail: near_box.hi.x - far_box.hi.x,
        extent: extent.x,
        from_center: from_center.map(|d| d.x),
    });
    let y = fit.place(&AxisGaps {
        view_leads: eye.y > mid.y,
        lead: near_box.hi.y - eye.y,
        trail: eye.y - near_box.lo.y,
        far_lead: near_box.hi.y - far_box.hi.y,
        far_trail: far_box.lo.y - near_box.lo.y,
        extent: extent.y,
        from_center: from_center.map(|d| d.y),
    });

    // the view's texel in each partition from the snapped origin, over
    // the 1024 (0x6394ca-0x6395d3)
    let side = PARTITION as f32;
    let uv: [Vec2; 2] = std::array::from_fn(|p| {
        let shift = (snapped - eye) / samples[p];
        Vec2::new(
            (shift.x + x.texel[p]).floor(),
            (y.texel[p] - shift.y).floor(),
        ) / side
    });
    let step = 1.0 / sc.ratio;
    let switch_partition = Vec4::new(
        uv[1].x - uv[0].x * step,
        (uv[1].y - uv[0].y * step + 1.0) * 0.5,
        0.0,
        step,
    );

    // the sun's view, less the snapped origin
    let sun_view = Mat4::from_cols(
        Vec4::new(-right.x, up.x, forward.x, 0.0),
        Vec4::new(-right.y, up.y, forward.y, 0.0),
        Vec4::new(-right.z, up.z, forward.z, 0.0),
        Vec4::new(-snapped.x, -snapped.y, 0.0, 1.0),
    );
    // the world box's reach along the sun
    let (depth_lo, depth_hi) = (0..8)
        .map(|i| {
            Vec3::select(
                glam::BVec3::new(i & 1 != 0, i & 2 != 0, i & 4 != 0),
                bounds.1,
                bounds.0,
            )
            .dot(forward)
        })
        .fold((f32::MAX, -f32::MAX), |(lo, hi), d| (lo.min(d), hi.max(d)));

    // 0x6382f0: 1 where the near partition's reach along the view ends
    let reach = view.z_near * near_sample * fit.texels_per_unit;
    let along = view.forward * (1.0 / reach);
    let along = along.extend(-view.origin.dot(along));
    // 0x638390: each partition's region along the view, the near one
    // from 0 to 1 reach, the far one from 0.75 to the ratio
    let starts = [0.0, 0.75].map(|d| unit_plane(along - Vec4::W * d, 1.0));
    let ends = [1.0, sc.ratio].map(|d| unit_plane(along - Vec4::W * d, -1.0));

    let outlines = [
        bounding_poly(
            &[corners[0], corners[1], corners[2], corners[3], eye],
            eye,
            max_size,
        ),
        bounding_poly(&[corners, pulled].concat(), eye, max_size),
    ];
    let depth_scale = 1.0 / (depth_hi - depth_lo + 2.0);
    let partitions: [Partition; 2] = std::array::from_fn(|p| {
        // 0x638240: texels of `samples[p]` units, the view's texel at
        // its clip position, depth 0..1 over the world's reach plus one
        // each side
        let texel = 2.0 / (samples[p] * side);
        let origin = Vec2::new(uv[p].x * 2.0 - 1.0, 1.0 - uv[p].y * 2.0);
        let ortho = Mat4::from_cols(
            Vec4::new(texel, 0.0, 0.0, 0.0),
            Vec4::new(0.0, texel, 0.0, 0.0),
            Vec4::new(0.0, 0.0, depth_scale, 0.0),
            Vec4::new(origin.x, origin.y, -(depth_lo - 1.0) * depth_scale, 1.0),
        );
        let view_proj = ortho * sun_view;
        let mut cull = CullPlanes {
            count: 0,
            planes: [Vec4::ZERO; 20],
        };
        // 0x637fc0: the outline onto the partition's clip origin, shifted
        // by the snap
        let shift = (eye - snapped) * texel;
        let outline: Vec<(Vec2, bool)> = outlines[p]
            .iter()
            .map(|&(q, inner)| (q + origin + shift, inner))
            .collect();
        for c in clip_planes(&outline, p == 1, sc.scale) {
            cull.push(unit_plane(view_proj.transpose() * c, 1.0));
        }
        // 0x60b850: the region planes and the view's sides that face
        // against the sun; the far partition takes no end plane
        let region = [Some(starts[p]), (p == 0).then_some(ends[p])];
        for plane in region.into_iter().flatten().chain(view.side_planes) {
            if forward.dot(plane.truncate()) < 0.0 {
                cull.push(plane);
            }
        }
        Partition {
            view_proj,
            viewport: [x.start[p], y.start[p], x.len[p], y.len[p]],
            polygon_offset: Vec4::new(
                POLYGON_OFFSET_BIAS * 0.25 * depth_scale,
                POLYGON_OFFSET_SCALE,
                0.0,
                0.0,
            ),
            cull,
        }
    });

    // 0x6397a0: clip x onto 0..1 and y onto 0.5..0 (the atlas's top
    // half), each plus half a texel; z as the near partition's; w the
    // reach fraction
    let near_vp = partitions[0].view_proj;
    let lookup = Mat4::from_cols(
        near_vp.row(0) * 0.5 + near_vp.row(3) * (0.5 + 1.0 / 2048.0),
        near_vp.row(1) * -0.25 + near_vp.row(3) * (0.25 + 1.0 / 4096.0),
        near_vp.row(2),
        along,
    )
    .transpose();
    SunShadow {
        partitions,
        lookup,
        switch_partition,
        scale: Vec4::new(16.0 / sc.scale, 32.0 / sc.scale, 0.0, 0.0),
    }
}

/// The side of the grid cells the world's casters are grouped by, so a
/// partition skips the cells its planes cull.
pub const CASTER_CELL: f32 = 128.0;

/// One run of casting triangles: the world vertices it indexes, their
/// bounds and the alpha-tested material it reads (`None` when solid).
#[derive(Clone, Debug, PartialEq)]
pub struct CasterGroup {
    pub cut: Option<u16>,
    pub indices: Vec<u32>,
    pub lo: Vec3,
    pub hi: Vec3,
}

/// The casting soups of `w`'s world surfaces (`soup_model` 0, no static
/// model), whose material has a caster entry, grouped by the `cell`-unit
/// square their bounds' centre falls in: solid ones first, then by
/// alpha-tested material. Retail culls each surface on its own
/// (`R_AddCellSurfacesAndCullGroupsInFrustumDelayed`); the groups trade
/// that for fewer draws. The static models cast through
/// [`smodel_casters`], a LOD picked each frame.
pub fn caster_groups(w: &MapWorld, soup_model: &[u32], cell: f32) -> Vec<CasterGroup> {
    let mut groups: std::collections::BTreeMap<(Option<u16>, i32, i32), CasterGroup> =
        Default::default();
    let mut smodel = vec![false; w.soups.len()];
    for &(_, first, count) in w.static_lods.iter().flat_map(|l| l.lods.iter()) {
        smodel[first as usize..(first + count) as usize].fill(true);
    }
    for (si, s) in w.soups.iter().enumerate() {
        if soup_model.get(si) != Some(&0) || s.index_count == 0 || smodel[si] {
            continue;
        }
        let Some((cut, idx, lo, hi)) = caster_soup(w, si) else {
            continue;
        };
        let mid = (lo + hi) * 0.5 / cell;
        let key = (cut, mid.x.floor() as i32, mid.y.floor() as i32);
        let g = groups.entry(key).or_insert_with(|| CasterGroup {
            cut,
            indices: Vec::new(),
            lo,
            hi,
        });
        g.lo = g.lo.min(lo);
        g.hi = g.hi.max(hi);
        g.indices.extend(idx);
    }
    groups.into_values().collect()
}

/// Soup `si` as a caster: its alpha-tested material (`None` solid), its
/// world vertex indices and bounds; `None` when its material casts not.
fn caster_soup(w: &MapWorld, si: usize) -> Option<(Option<u16>, Vec<u32>, Vec3, Vec3)> {
    let s = &w.soups[si];
    let c = w.sun_casters.get(usize::from(s.material))?.as_ref()?;
    let first = s.first_index as usize;
    let idx = &w.indices[first..first + s.index_count as usize];
    let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(-f32::MAX));
    let out: Vec<u32> = idx
        .iter()
        .map(|&i| {
            let v = s.first_vertex + u32::from(i);
            let p = Vec3::from(w.verts[v as usize].pos);
            lo = lo.min(p);
            hi = hi.max(p);
            v
        })
        .collect();
    Some((c.alpha_test.then_some(s.material), out, lo, hi))
}

/// One static model as a caster: per LOD its casting runs (alpha-tested
/// material, world vertex indices), and the bounds of them all.
#[derive(Clone, Debug, PartialEq)]
pub struct SmodelCaster {
    /// Its placement's LOD pick (`StaticLod::pick`).
    pub lod: vcod_common::mapworld::StaticLod,
    pub lods: Vec<Vec<(Option<u16>, Vec<u32>)>>,
    pub lo: Vec3,
    pub hi: Vec3,
}

/// Every static model of `w` with a casting surface on some LOD, in
/// `MapWorld::static_lods`' order. Retail's sun and spot shadow passes
/// draw each at the LOD the camera's distance picks through the rigid
/// ramp, none past its `cullDist` (cod4-world.md 9.10, 3.6).
pub fn smodel_casters(w: &MapWorld) -> Vec<SmodelCaster> {
    w.static_lods
        .iter()
        .filter_map(|sl| {
            let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(-f32::MAX));
            let lods: Vec<Vec<(Option<u16>, Vec<u32>)>> = sl
                .lods
                .iter()
                .map(|&(_, first, count)| {
                    (first as usize..(first + count) as usize)
                        .filter(|&si| w.soups[si].index_count > 0)
                        .filter_map(|si| caster_soup(w, si))
                        .map(|(cut, idx, a, b)| {
                            lo = lo.min(a);
                            hi = hi.max(b);
                            (cut, idx)
                        })
                        .collect()
                })
                .collect();
            lods.iter().any(|l| !l.is_empty()).then(|| SmodelCaster {
                lod: sl.clone(),
                lods,
                lo,
                hi,
            })
        })
        .collect()
}

/// The 4-tap test of `lm_sm_sun_*_sm3.hlsl` (cod4-world.md 3.6) at atlas
/// coordinates `at` and depth `z`, `depth` reading the atlas at a texel:
/// the share of four taps whose stored depth is at or past `z`.
pub fn pcf(at: Vec2, z: f32, depth: impl Fn(u32, u32) -> f32) -> f32 {
    let size = Vec2::new(PARTITION as f32, 2.0 * PARTITION as f32);
    let taps = [
        Vec2::new(-1.0 / 2048.0, 1.0 / 8192.0),
        Vec2::new(1.0 / 4096.0, 1.0 / 4096.0),
        Vec2::new(-1.0 / 4096.0, -1.0 / 4096.0),
        Vec2::new(1.0 / 2048.0, -1.0 / 8192.0),
    ];
    let lit = taps.iter().filter(|&&t| {
        let tx = ((at + t) * size).floor().clamp(Vec2::ZERO, size - 1.0);
        depth(tx.x as u32, tx.y as u32) >= z
    });
    lit.count() as f32 * 0.25
}

/// The sun's visibility the technique 9 pixel shaders take, at the
/// lookup's output `s` (`lookup * (p, 1)`), with `baked` the visibility the
/// surface stores (the primary lightmap's red, a model's lighting block
/// alpha): near the view the near partition's test alone, blended into the
/// far partition's over the last quarter of its reach; past it the far
/// partition's, faded into `baked` toward its edge.
pub fn visibility(sh: &SunShadow, s: Vec4, baked: f32, depth: impl Fn(u32, u32) -> f32) -> f32 {
    let far_at =
        s.truncate().truncate() * sh.switch_partition.w + sh.switch_partition.truncate().truncate();
    if s.w <= 1.0 {
        let near = pcf(s.truncate().truncate(), s.z, &depth);
        if s.w <= 0.75 {
            return near;
        }
        let far = pcf(far_at, s.z, &depth);
        let t = s.w * 4.0 - 3.0;
        return t * far + (1.0 - t) * near;
    }
    let edge = (far_at - Vec2::new(0.5, 0.75)).abs() * sh.scale.truncate().truncate();
    let edge = edge.x.max(edge.y);
    if edge - 8.0 >= 0.0 {
        return baked;
    }
    let far = pcf(far_at, s.z, &depth);
    let t = (edge - 7.0).clamp(0.0, 1.0);
    t * baked + (1.0 - t) * far
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Looking along x from (100, 200, 60), 80 by 64 degrees.
    fn view() -> View {
        let origin = Vec3::new(100.0, 200.0, 60.0);
        let proj =
            glam::camera::rh::proj::directx::perspective(64f32.to_radians(), 1.3333, 4.0, 60000.0);
        let look = glam::camera::rh::view::look_to_mat4(origin, Vec3::X, Vec3::Z);
        View::of(proj * look, origin, Vec3::X, 4.0)
    }

    #[test]
    fn the_near_corners_lie_on_the_near_plane() {
        let v = view();
        for c in v.near_corners {
            assert!(((c - v.origin).dot(Vec3::X) - 4.0).abs() < 1e-3, "{c}");
        }
    }

    fn bounds() -> (Vec3, Vec3) {
        (Vec3::splat(-4096.0), Vec3::splat(4096.0))
    }

    /// A sun low in the west.
    fn to_sun() -> Vec3 {
        Vec3::new(-0.6, 0.3, 0.74).normalize()
    }

    #[test]
    fn the_axes_are_orthonormal_and_face_away_from_the_sun() {
        for sun in [to_sun(), Vec3::Z, Vec3::new(0.1, 0.0, 1.0).normalize()] {
            let [f, r, u] = sun_axes(sun);
            assert!((f + sun).length() < 1e-6);
            for (a, b) in [(f, r), (f, u), (r, u)] {
                assert!(a.dot(b).abs() < 1e-5);
            }
            assert!((r.length() - 1.0).abs() < 1e-5 && (u.length() - 1.0).abs() < 1e-5);
        }
    }

    /// The near partition takes a quarter unit a texel and the far one a
    /// unit, both draw the view's own spot, and the lookup lands it where
    /// the near partition drew it.
    #[test]
    fn the_lookup_finds_the_view_where_the_partitions_drew_it() {
        let sh = setup(&view(), to_sun(), bounds(), &Settings::default());
        let p = view().origin + Vec3::new(30.0, 3.0, -5.0);
        let near = sh.partitions[0].view_proj * p.extend(1.0);
        let far = sh.partitions[1].view_proj * p.extend(1.0);
        let one_unit =
            sh.partitions[0].view_proj * (p + to_sun().cross(Vec3::Z).normalize()).extend(1.0);
        // a unit across the sun is four near texels (8 / 1024 of clip)
        assert!(((one_unit - near).truncate().truncate().length() - 8.0 / 1024.0).abs() < 1e-4);
        for c in [near, far] {
            assert!(
                c.x.abs() < 1.0 && c.y.abs() < 1.0 && c.z > 0.0 && c.z < 1.0,
                "{c}"
            );
        }
        let s = sh.lookup * p.extend(1.0);
        // atlas x is clip x half scaled, y the top half's
        assert!((s.x - (near.x * 0.5 + 0.5 + 1.0 / 2048.0)).abs() < 1e-5);
        assert!((s.y - (near.y * -0.25 + 0.25 + 1.0 / 4096.0)).abs() < 1e-5);
        assert!((s.z - near.z).abs() < 1e-6);
        // the far partition's spot is the near one's switched
        let f = s.truncate().truncate() * sh.switch_partition.w
            + sh.switch_partition.truncate().truncate();
        assert!(
            (f.x - (far.x * 0.5 + 0.5 + 1.0 / 2048.0 / RATIO)).abs() < 2.0 / 1024.0,
            "{f} {far}"
        );
        assert!(f.y > 0.5 && f.y < 1.0);
        // w counts the distance along the view in near partition reaches
        assert!(s.w > 0.0 && s.w < 1.0, "{}", s.w);
    }

    /// The far partition fades into the baked visibility over its last
    /// texel of `shadowmapScale` and reads it past.
    #[test]
    fn far_past_the_partitions_the_baked_visibility_reads() {
        let sh = setup(&view(), to_sun(), bounds(), &Settings::default());
        let s = sh.lookup * (view().origin + Vec3::new(3000.0, 0.0, 0.0)).extend(1.0);
        assert!(s.w > 1.0);
        assert_eq!(visibility(&sh, s, 0.3, |_, _| 0.0), 0.3);
        // in the near partition only the map counts
        let s = sh.lookup * (view().origin + Vec3::new(20.0, 0.0, -40.0)).extend(1.0);
        assert!(s.w < 0.75);
        assert_eq!(visibility(&sh, s, 0.3, |_, _| 1.0), 1.0);
        assert_eq!(visibility(&sh, s, 0.3, |_, _| 0.0), 0.0);
    }

    #[test]
    fn the_scale_takes_the_nearest_tenth_fraction() {
        let one = Scale::of(1.0);
        assert_eq!(
            (one.scale, one.num, one.size, one.ratio),
            (1.0, 1.0, 1024, 4.0)
        );
        let half = Scale::of(0.5);
        assert_eq!(
            (half.scale, half.num, half.size, half.ratio),
            (0.5, 1.0, 512, 8.0)
        );
        let s = Scale::of(0.3);
        assert_eq!((s.num, s.size), (3.0, 308));
        // 0.33 is 1/3 (error 0.0033) before 3/10 (0.03)
        assert_eq!(Scale::of(0.33).num, 1.0);
    }

    fn cube(at: Vec3, half: f32) -> (Vec3, Vec3) {
        (at - half, at + half)
    }

    /// The view's own spot casts into the near partition and the ground
    /// past its reach into the far one, which leaves out what the near one
    /// covers alone; with the sun ahead a box behind the view casts into
    /// neither, and one far ahead not into the near partition.
    #[test]
    fn the_partitions_cull_casters_outside_their_regions() {
        let v = view();
        let ahead = Vec3::new(0.6, 0.0, 0.8);
        let sh = setup(&v, ahead, bounds(), &Settings::default());
        let [near, far] = &sh.partitions;
        let (lo, hi) = cube(v.origin, 8.0);
        assert!(!near.cull.culls(lo, hi));
        assert!(far.cull.culls(lo, hi));
        let (lo, hi) = cube(v.origin + Vec3::new(60.0, 0.0, -40.0), 8.0);
        assert!(!near.cull.culls(lo, hi), "the floor ahead");
        let (lo, hi) = cube(v.origin + Vec3::new(400.0, 0.0, -40.0), 8.0);
        assert!(!far.cull.culls(lo, hi), "the floor past the near reach");
        for p in &sh.partitions {
            let (lo, hi) = cube(v.origin - Vec3::new(600.0, 0.0, 0.0), 8.0);
            assert!(p.cull.culls(lo, hi), "behind the view");
        }
        let (lo, hi) = cube(v.origin + Vec3::new(3000.0, 0.0, 0.0), 8.0);
        assert!(sh.partitions[0].cull.culls(lo, hi));
        // a sun behind: past the near partition's reach nothing casts into it
        let behind = Vec3::new(-0.6, 0.0, 0.8);
        let sh = setup(&v, behind, bounds(), &Settings::default());
        let (lo, hi) = cube(v.origin + Vec3::new(1000.0, 0.0, -100.0), 8.0);
        assert!(sh.partitions[0].cull.culls(lo, hi));
        assert!(
            !sh.partitions[1].cull.culls(lo, hi),
            "the far one reaches it"
        );
    }

    /// At half scale the far partition draws in the middle 512 square and
    /// its edge fades twice as fast in the lookup's units.
    #[test]
    fn a_half_scale_shrinks_the_far_square() {
        let st = Settings {
            scale: 0.5,
            ..Settings::default()
        };
        let sh = setup(&view(), to_sun(), bounds(), &st);
        let [x, y, w, h] = sh.partitions[1].viewport;
        assert!(
            x >= 256 && y >= 256 && x + w <= 768 && y + h <= 768,
            "{x} {y} {w} {h}"
        );
        assert_eq!(sh.scale.x, 32.0);
        assert_eq!(sh.switch_partition.w, 1.0 / 8.0);
        // a centre pins the far square on it
        let st = Settings {
            center: Vec3::new(500.0, 200.0, 0.0),
            ..Settings::default()
        };
        let sh = setup(&view(), to_sun(), bounds(), &st);
        assert_eq!(sh.partitions[1].viewport, [0, 0, 1024, 1024]);
    }
}
