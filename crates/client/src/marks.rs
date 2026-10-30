//! Impact marks: bullet holes and scorches clipped onto the world.
//!
//! An efx `Decal` block calls back into the cgame's `CG_ImpactMark`
//! (cgame 0x30023560), which asks the renderer's `R_MarkFragments`
//! (CoDMP.exe 0x4e8db0) to clip a square onto the world's triangles and
//! keeps each clipped triangle as one pooled mark that `CG_AddMarks`
//! (0x300238f0) fades and draws. Facts and addresses:
//! docs/research/cod11-events-and-fx.md, section 9.

use std::collections::VecDeque;
use std::ops::Range;

use glam::Vec3;
use vcod_common::mapworld::MapWorld;

/// The cgame's mark pool (0x30023440 links 0x400 entries of 0x174 bytes).
pub const MAX_MARKS: usize = 1024;
/// A fragment's points past this are dropped (`0x300237f8`).
pub const MAX_MARK_VERTS: usize = 10;
/// `R_MarkFragments`' point and fragment budgets, as the cgame passes them.
const MAX_MARK_POINTS: usize = 0x400;
const MAX_MARK_FRAGMENTS: usize = 0x180;
/// The surface gather's list size (`0x4e9283`'s 0x1000).
const MAX_MARK_SURFACES: usize = 0x1000;
/// The chop's on-plane epsilon (0x4e8cf0 passes 0.5) and the facing test's
/// minimum `dot(triangle normal, mark normal)` (0x568e70, also 0.5).
const CLIP_EPSILON: f32 = 0.5;
const FACING_MIN: f32 = 0.5;
/// A clip polygon over this many points is dropped whole (0x4e86a0).
const MAX_CLIP_IN: usize = 0x3d;
/// `lifetime < 0` reads as this (0x30023597).
const DEFAULT_LIFE_MS: i32 = 20000;
/// `surfaceparm nomarks`, surface flag 0x20 (CoDMP.exe infoparm table
/// 0x571a60); the gather skips a surface whose shader carries it.
pub const SURF_NOMARKS: u32 = 0x20;

/// One `Decal` element's call into `CG_ImpactMark`, with the values the
/// effect spawner sampled (CoDMP.exe 0x496896..0x4969b1).
#[derive(Clone, Debug, PartialEq)]
pub struct MarkRequest {
    pub shader: String,
    pub origin: Vec3,
    /// The surface normal the mark faces out along.
    pub dir: Vec3,
    /// Degrees about `dir` (`rotation`).
    pub orientation: f32,
    pub rgba: [f32; 4],
    /// Effects always pass 1: the fade window writes alpha, not colour.
    pub alpha_fade: bool,
    /// Half the square's side (`size`).
    pub radius: f32,
    /// Milliseconds (`life`); negative reads as 20 s.
    pub life_ms: i32,
}

/// A clipped point: position, the mark's own texture coordinates and the
/// surface's lightmap coordinates.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct MarkVert {
    pub pos: [f32; 3],
    pub st: [f32; 2],
    pub lm: [f32; 2],
}

/// One clipped triangle, `R_MarkFragments`' `markFragment_t` with the
/// lightmap of the surface it came from (the renderer picks the shader's
/// lightmapped copy per fragment, 0x4e9369).
#[derive(Clone, Debug, PartialEq)]
pub struct Fragment {
    pub lightmap: u16,
    pub verts: Vec<MarkVert>,
}

#[derive(Copy, Clone, Debug)]
struct ClipVert {
    pos: Vec3,
    lm: [f32; 2],
}

struct MarkSurface {
    lightmap: u16,
    mins: Vec3,
    maxs: Vec3,
    tris: Range<usize>,
}

/// The world's triangles that take marks: model 0's drawn soups whose
/// material lacks `nomarks`, in soup order. Brush models and props take
/// none: the gather walks only the world's cells.
#[derive(Default)]
pub struct MarkWorld {
    surfaces: Vec<MarkSurface>,
    tris: Vec<[ClipVert; 3]>,
}

impl MarkWorld {
    /// `drawn(material)` says whether the renderer draws the soup at all
    /// (sky and nodraw soups never reach the cells' surface lists).
    pub fn build(bsp: &MapWorld, drawn: impl Fn(u16) -> bool) -> MarkWorld {
        let mut out = MarkWorld::default();
        let world_soups = bsp.models.first().map_or(0..bsp.soups.len(), |m| {
            m.first_soup as usize..(m.first_soup + m.num_soups) as usize
        });
        for soup in bsp.soups.get(world_soups).unwrap_or_default() {
            let flags = bsp
                .materials
                .get(soup.material as usize)
                .map_or(0, |m| m.surface_flags);
            if flags & SURF_NOMARKS != 0 || !drawn(soup.material) {
                continue;
            }
            let fv = soup.first_vertex as usize;
            let fi = soup.first_index as usize;
            let Some(idx) = bsp.indices.get(fi..fi + soup.index_count as usize) else {
                continue;
            };
            let first = out.tris.len();
            let (mut mins, mut maxs) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
            for tri in idx.as_chunks::<3>().0 {
                let Some(v) = tri
                    .iter()
                    .map(|&i| bsp.verts.get(fv + i as usize))
                    .collect::<Option<Vec<_>>>()
                else {
                    continue;
                };
                let cv = [0, 1, 2].map(|k| ClipVert {
                    pos: Vec3::from_array(v[k].pos),
                    lm: v[k].lm_uv,
                });
                for c in &cv {
                    mins = mins.min(c.pos);
                    maxs = maxs.max(c.pos);
                }
                out.tris.push(cv);
            }
            if out.tris.len() > first {
                out.surfaces.push(MarkSurface {
                    lightmap: soup.lightmap,
                    mins,
                    maxs,
                    tris: first..out.tris.len(),
                });
            }
        }
        out
    }

    pub fn triangle_count(&self) -> usize {
        self.tris.len()
    }
}

/// `CG_ImpactMark`'s basis: the normal, then `axis[1] = axis[0] x axis[2]`
/// with `axis[2]` the perpendicular turned `orientation` degrees about the
/// normal. `None` for a zero normal.
pub fn mark_axes(dir: Vec3, orientation: f32) -> Option<[Vec3; 3]> {
    let a0 = dir.try_normalize()?;
    let a1 = perpendicular(a0);
    let a2 = rotate_about(a1, a0, orientation);
    Some([a0, a0.cross(a2), a2])
}

/// `PerpendicularVector` (cgame 0x3003a970): project the axis the vector
/// leans on least onto its plane.
fn perpendicular(v: Vec3) -> Vec3 {
    let a = v.to_array();
    let mut pos = 0;
    let mut min = 1.0f32;
    for (i, c) in a.iter().enumerate() {
        if c.abs() < min {
            pos = i;
            min = c.abs();
        }
    }
    let mut t = Vec3::ZERO;
    t[pos] = 1.0;
    (t - v * t.dot(v)).normalize_or_zero()
}

/// `RotatePointAroundVector`: `p` turned `degrees` about the unit `axis`.
fn rotate_about(p: Vec3, axis: Vec3, degrees: f32) -> Vec3 {
    let (s, c) = degrees.to_radians().sin_cos();
    p * c + axis.cross(p) * s + axis * axis.dot(p) * (1.0 - c)
}

/// The mark's square, wound as the cgame builds it.
fn mark_square(origin: Vec3, axis: &[Vec3; 3], r: f32) -> [Vec3; 4] {
    let (u, v) = (axis[1] * r, axis[2] * r);
    [
        origin - u - v,
        origin + u - v,
        origin + u + v,
        origin - u + v,
    ]
}

/// `R_ChopPolyBehindPlane` as 0x4e86a0 runs it: keep the side the normal
/// faces, points within the epsilon count as on the plane.
fn chop(input: &[ClipVert], normal: Vec3, dist: f32, out: &mut Vec<ClipVert>) {
    out.clear();
    if input.len() > MAX_CLIP_IN {
        return;
    }
    const FRONT: u8 = 0;
    const BACK: u8 = 1;
    const ON: u8 = 2;
    // on the stack: a mark's gather chops every triangle near it
    let mut dists = [0.0f32; MAX_CLIP_IN];
    let mut sides = [ON; MAX_CLIP_IN];
    for (i, p) in input.iter().enumerate() {
        let d = p.pos.dot(normal) - dist;
        dists[i] = d;
        sides[i] = if d > CLIP_EPSILON {
            FRONT
        } else if d < -CLIP_EPSILON {
            BACK
        } else {
            ON
        };
    }
    let sides = &sides[..input.len()];
    if !sides.contains(&FRONT) {
        return;
    }
    if !sides.contains(&BACK) {
        out.extend_from_slice(input);
        return;
    }
    let n = input.len();
    for i in 0..n {
        let p1 = input[i];
        match sides[i] {
            ON => {
                out.push(p1);
                continue;
            }
            FRONT => out.push(p1),
            _ => {}
        }
        let next = (i + 1) % n;
        if sides[next] == ON || sides[next] == sides[i] {
            continue;
        }
        let p2 = input[next];
        let denom = dists[i] - dists[next];
        let d = if denom == 0.0 { 0.0 } else { dists[i] / denom };
        out.push(ClipVert {
            pos: p1.pos + (p2.pos - p1.pos) * d,
            lm: [
                p1.lm[0] + (p2.lm[0] - p1.lm[0]) * d,
                p1.lm[1] + (p2.lm[1] - p1.lm[1]) * d,
            ],
        });
    }
}

/// `R_MarkFragments` for a square of half-side `radius` about `origin` on
/// `axis` (normal first): every gathered triangle facing the normal within
/// 60 degrees, clipped to the square's edges and to `radius` either side of
/// its plane, one fragment each.
pub fn mark_fragments(
    world: &MarkWorld,
    origin: Vec3,
    axis: &[Vec3; 3],
    radius: f32,
) -> Vec<Fragment> {
    let points = mark_square(origin, axis, radius);
    let a0 = axis[0];
    // The gather box spans the square pushed radius out and 20 radii in.
    let (mut mins, mut maxs) = (Vec3::splat(262144.0), Vec3::splat(-262144.0));
    for p in points {
        for q in [p, p + a0 * radius, p + a0 * (radius * -20.0)] {
            mins = mins.min(q);
            maxs = maxs.max(q);
        }
    }
    let mut planes: Vec<(Vec3, f32)> = (0..4)
        .map(|i| {
            let n = (points[(i + 1) % 4] - points[i])
                .cross(a0)
                .normalize_or_zero();
            (n, n.dot(points[i]))
        })
        .collect();
    planes.push((a0, a0.dot(points[0]) - radius));
    planes.push((-a0, -a0.dot(points[0]) - radius));
    let st_scale = 0.5 / radius;

    let mut out: Vec<Fragment> = Vec::new();
    let mut used_points = 0usize;
    let (mut a, mut b) = (Vec::with_capacity(64), Vec::with_capacity(64));
    let surfaces = world
        .surfaces
        .iter()
        .filter(|s| s.mins.cmple(maxs).all() && s.maxs.cmpge(mins).all())
        .take(MAX_MARK_SURFACES);
    // A triangle wholly past one side of the gather box, grown by the
    // clip's epsilon, leaves nothing after the chops: skipped unchopped.
    let (lo, hi) = (mins - Vec3::splat(1.0), maxs + Vec3::splat(1.0));
    for surf in surfaces {
        for tri in &world.tris[surf.tris.clone()] {
            let [v0, v1, v2] = tri.map(|c| c.pos);
            if (v0.cmplt(lo) & v1.cmplt(lo) & v2.cmplt(lo)).any()
                || (v0.cmpgt(hi) & v1.cmpgt(hi) & v2.cmpgt(hi)).any()
            {
                continue;
            }
            let n = (v0 - v1).cross(v2 - v1).normalize_or_zero();
            if n.dot(a0) <= FACING_MIN {
                continue;
            }
            a.clear();
            a.extend_from_slice(tri);
            for &(pn, pd) in &planes {
                chop(&a, pn, pd, &mut b);
                std::mem::swap(&mut a, &mut b);
                if a.is_empty() {
                    break;
                }
            }
            if a.is_empty() || a.len() > MAX_MARK_POINTS - used_points {
                continue;
            }
            used_points += a.len();
            out.push(Fragment {
                lightmap: surf.lightmap,
                verts: a
                    .iter()
                    .map(|c| {
                        let d = c.pos - origin;
                        MarkVert {
                            pos: c.pos.to_array(),
                            st: [
                                d.dot(axis[1]) * st_scale + 0.5,
                                d.dot(axis[2]) * st_scale + 0.5,
                            ],
                            lm: c.lm,
                        }
                    })
                    .collect(),
            });
            if out.len() == MAX_MARK_FRAGMENTS || used_points > MAX_MARK_POINTS - 3 {
                return out;
            }
        }
    }
    out
}

/// One pooled fragment (`markPoly_t`, 0x174 bytes).
#[derive(Clone, Debug)]
pub struct Mark {
    pub time_ms: i32,
    pub life_ms: i32,
    pub shader: String,
    pub lightmap: u16,
    pub alpha_fade: bool,
    pub color: [f32; 4],
    pub verts: Vec<MarkVert>,
}

impl Mark {
    /// The vertex colour bytes `CG_AddMarks` leaves at `now_ms`: the
    /// creation colour, and past half the life `remaining * 255 / (life /
    /// 2)` in the alpha (alpha fade) or scaling the colour (otherwise).
    pub fn rgba_at(&self, now_ms: i32) -> [u8; 4] {
        let byte = |f: f32| f as i32 as u8;
        let mut out = self.color.map(|c| byte(c * 255.0));
        let remaining = self.time_ms.wrapping_add(self.life_ms).wrapping_sub(now_ms);
        let half = f64::from(self.life_ms) * 0.5;
        if f64::from(remaining) < half {
            let fade = (f64::from(remaining) * 255.0 / half) as i32;
            if self.alpha_fade {
                out[3] = fade as u8;
            } else {
                for (o, c) in out.iter_mut().zip(self.color).take(3) {
                    *o = byte(fade as f32 * c);
                }
            }
        }
        out
    }

    fn expired(&self, now_ms: i32) -> bool {
        self.time_ms.wrapping_add(self.life_ms) < now_ms
    }
}

/// `CG_ImpactMark`'s clip: one mark per fragment, its points capped at
/// [`MAX_MARK_VERTS`]. A zero normal or radius places nothing. A
/// temporary mark (`markShadow`) is these, drawn this frame only and never
/// pooled (0x300237f8, syscall 0x40).
pub fn place(world: &MarkWorld, req: &MarkRequest, now_ms: i32) -> Vec<Mark> {
    if req.radius <= 0.0 {
        return Vec::new();
    }
    let Some(axis) = mark_axes(req.dir, req.orientation) else {
        return Vec::new();
    };
    let life_ms = if req.life_ms < 0 {
        DEFAULT_LIFE_MS
    } else {
        req.life_ms
    };
    mark_fragments(world, req.origin, &axis, req.radius)
        .into_iter()
        .map(|mut frag| {
            frag.verts.truncate(MAX_MARK_VERTS);
            Mark {
                time_ms: now_ms,
                life_ms,
                shader: req.shader.clone(),
                lightmap: frag.lightmap,
                alpha_fade: req.alpha_fade,
                color: req.rgba,
                verts: frag.verts,
            }
        })
        .collect()
}

/// The cgame's mark list: oldest first. A full pool frees the oldest
/// marks, every one placed at the same time as the oldest (0x300234d0),
/// so a whole impact goes at once.
#[derive(Default)]
pub struct MarkPool {
    marks: VecDeque<Mark>,
}

impl MarkPool {
    /// `CG_ImpactMark` for a permanent mark: one pooled mark per fragment.
    pub fn impact(&mut self, world: &MarkWorld, req: &MarkRequest, now_ms: i32) {
        for mark in place(world, req, now_ms) {
            self.alloc(mark);
        }
    }

    fn alloc(&mut self, mark: Mark) {
        if self.marks.len() >= MAX_MARKS
            && let Some(t) = self.marks.front().map(|m| m.time_ms)
        {
            while self.marks.front().is_some_and(|m| m.time_ms == t) {
                self.marks.pop_front();
            }
        }
        self.marks.push_back(mark);
    }

    /// Frees the marks whose life ran out (`CG_AddMarks`' first test).
    pub fn expire(&mut self, now_ms: i32) {
        self.marks.retain(|m| !m.expired(now_ms));
    }

    /// Live marks, newest first: `CG_AddMarks` walks the list backwards.
    pub fn newest_first(&self) -> impl Iterator<Item = &Mark> {
        self.marks.iter().rev()
    }

    pub fn len(&self) -> usize {
        self.marks.len()
    }

    pub fn clear(&mut self) {
        self.marks.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A world of one floor quad at z = 0 spanning +-64, normal +z.
    fn floor() -> MarkWorld {
        let c = |x: f32, y: f32| ClipVert {
            pos: Vec3::new(x, y, 0.0),
            lm: [(x + 64.0) / 128.0, (y + 64.0) / 128.0],
        };
        let (a, b, cc, d) = (
            c(-64.0, -64.0),
            c(64.0, -64.0),
            c(64.0, 64.0),
            c(-64.0, 64.0),
        );
        // wound so (v0 - v1) x (v2 - v1) is +z
        let tris = vec![[a, d, cc], [a, cc, b]];
        MarkWorld {
            surfaces: vec![MarkSurface {
                lightmap: 3,
                mins: Vec3::new(-64.0, -64.0, 0.0),
                maxs: Vec3::new(64.0, 64.0, 0.0),
                tris: 0..2,
            }],
            tris,
        }
    }

    fn request(origin: Vec3, radius: f32) -> MarkRequest {
        MarkRequest {
            shader: "gfx/impact/bullethole1".into(),
            origin,
            dir: Vec3::Z,
            orientation: 0.0,
            rgba: [1.0; 4],
            alpha_fade: true,
            radius,
            life_ms: 8000,
        }
    }

    fn area(verts: &[MarkVert]) -> f32 {
        let p: Vec<Vec3> = verts.iter().map(|v| Vec3::from_array(v.pos)).collect();
        (1..p.len() - 1)
            .map(|i| (p[i] - p[0]).cross(p[i + 1] - p[0]).length() * 0.5)
            .sum()
    }

    #[test]
    fn basis_is_orthonormal_with_the_normal_first() {
        for (dir, rot) in [(Vec3::Z, 0.0), (Vec3::new(1.0, 2.0, -0.5), 137.0)] {
            let [a0, a1, a2] = mark_axes(dir, rot).unwrap();
            assert!((a0 - dir.normalize()).length() < 1e-6);
            for (x, y) in [(a0, a1), (a0, a2), (a1, a2)] {
                assert!(x.dot(y).abs() < 1e-5);
            }
            assert!((a1.length() - 1.0).abs() < 1e-5 && (a2.length() - 1.0).abs() < 1e-5);
        }
        assert!(mark_axes(Vec3::ZERO, 0.0).is_none());
    }

    #[test]
    fn a_mark_inside_one_face_clips_to_its_square() {
        let world = floor();
        let axis = mark_axes(Vec3::Z, 30.0).unwrap();
        let frags = mark_fragments(&world, Vec3::new(10.0, 5.0, 0.0), &axis, 4.0);
        // the square straddles the quad's diagonal on this placement or
        // not; either way the pieces add up to the 8x8 square
        let total: f32 = frags.iter().map(|f| area(&f.verts)).sum();
        assert!((total - 64.0).abs() < 1e-3, "area {total}");
        for f in &frags {
            assert_eq!(f.lightmap, 3);
            for v in &f.verts {
                assert!(v.st.iter().all(|&s| (-1e-4..=1.0 + 1e-4).contains(&s)));
                assert!(v.pos[2].abs() < 1e-6);
                // lightmap coordinates interpolate with the position
                assert!((v.lm[0] - (v.pos[0] + 64.0) / 128.0).abs() < 1e-5);
            }
        }
    }

    #[test]
    fn a_mark_over_the_edge_keeps_only_the_part_on_the_face() {
        let world = floor();
        let axis = mark_axes(Vec3::Z, 0.0).unwrap();
        let frags = mark_fragments(&world, Vec3::new(62.0, 0.0, 0.0), &axis, 4.0);
        let total: f32 = frags.iter().map(|f| area(&f.verts)).sum();
        assert!((total - 6.0 * 8.0).abs() < 1e-3, "area {total}");
    }

    #[test]
    fn faces_turned_away_or_out_of_depth_take_nothing() {
        let world = floor();
        let down = mark_axes(-Vec3::Z, 0.0).unwrap();
        assert!(mark_fragments(&world, Vec3::ZERO, &down, 4.0).is_empty());
        // 61 degrees off the face normal fails the 0.5 facing test
        let tilt = Vec3::new(61f32.to_radians().sin(), 0.0, 61f32.to_radians().cos());
        let tilted = mark_axes(tilt, 0.0).unwrap();
        assert!(mark_fragments(&world, Vec3::ZERO, &tilted, 4.0).is_empty());
        // a mark more than its radius above the face misses it
        let up = mark_axes(Vec3::Z, 0.0).unwrap();
        assert!(mark_fragments(&world, Vec3::new(0.0, 0.0, 5.0), &up, 4.0).is_empty());
        assert!(!mark_fragments(&world, Vec3::new(0.0, 0.0, 3.0), &up, 4.0).is_empty());
    }

    #[test]
    fn chop_keeps_the_front_and_counts_near_points_as_on() {
        let tri = [
            ClipVert {
                pos: Vec3::new(0.0, 0.0, 0.0),
                lm: [0.0, 0.0],
            },
            ClipVert {
                pos: Vec3::new(10.0, 0.0, 0.0),
                lm: [1.0, 0.0],
            },
            ClipVert {
                pos: Vec3::new(0.0, 10.0, 0.0),
                lm: [0.0, 1.0],
            },
        ];
        let mut out = Vec::new();
        chop(&tri, Vec3::X, 5.0, &mut out);
        assert_eq!(out.len(), 3);
        assert!(out.iter().all(|v| v.pos.x >= 5.0 - 1e-5));
        // the cut points carry interpolated lightmap coordinates
        assert!(out.iter().all(|v| (v.lm[0] - v.pos.x / 10.0).abs() < 1e-6));
        // every point within 0.5 of the plane counts as on it, and a
        // polygon with nothing in front is dropped
        chop(&tri, Vec3::Z, -0.4, &mut out);
        assert!(out.is_empty(), "no point in front");
        chop(&tri, Vec3::Z, -0.6, &mut out);
        assert_eq!(out.len(), 3);
    }

    #[test]
    fn a_full_pool_frees_every_mark_of_the_oldest_impact() {
        let world = floor();
        let mut pool = MarkPool::default();
        // straddling the diagonal: two fragments per impact
        let req = request(Vec3::ZERO, 4.0);
        pool.impact(&world, &req, 0);
        let per = pool.len();
        assert_eq!(per, 2);
        let mut t = 1;
        while pool.len() + per <= MAX_MARKS {
            pool.impact(&world, &req, t);
            t += 1;
        }
        assert_eq!(pool.len(), MAX_MARKS);
        pool.impact(&world, &req, t);
        assert_eq!(pool.len(), MAX_MARKS);
        assert!(pool.newest_first().all(|m| m.time_ms != 0));
        assert_eq!(pool.newest_first().next().unwrap().time_ms, t);
    }

    #[test]
    fn marks_fade_over_the_second_half_of_their_life_then_expire() {
        let world = floor();
        let mut pool = MarkPool::default();
        let mut req = request(Vec3::new(30.0, -20.0, 0.0), 2.0);
        req.rgba = [0.5, 0.25, 1.0, 0.75];
        pool.impact(&world, &req, 1000);
        let m = pool.newest_first().next().unwrap().clone();
        assert_eq!(m.rgba_at(1000), [127, 63, 255, 191]);
        assert_eq!(m.rgba_at(5000), [127, 63, 255, 191]);
        // 2000 ms left of 8000: 2000 * 255 / 4000
        assert_eq!(m.rgba_at(7000), [127, 63, 255, 127]);
        assert_eq!(m.rgba_at(9000), [127, 63, 255, 0]);
        let mut dim = m.clone();
        dim.alpha_fade = false;
        assert_eq!(dim.rgba_at(7000), [63, 31, 127, 191]);

        pool.expire(9000);
        assert_eq!(pool.len(), 1);
        pool.expire(9001);
        assert_eq!(pool.len(), 0);
    }

    #[test]
    fn negative_life_reads_as_twenty_seconds() {
        let world = floor();
        let mut pool = MarkPool::default();
        let mut req = request(Vec3::new(30.0, -20.0, 0.0), 2.0);
        req.life_ms = -1;
        pool.impact(&world, &req, 0);
        assert_eq!(pool.newest_first().next().unwrap().life_ms, 20000);
    }

    #[test]
    fn nomarks_and_undrawn_soups_take_no_marks() {
        let Some(fs) = vcod_common::testing::game_fs() else {
            return;
        };
        let Some(raw) = fs.read("maps/mp/mp_carentan.bsp") else {
            return;
        };
        let bsp = vcod_common::bsp::parse(&raw).unwrap();
        let all = MarkWorld::build(&bsp, |_| true);
        let none = MarkWorld::build(&bsp, |_| false);
        assert!(all.triangle_count() > 0);
        assert_eq!(none.triangle_count(), 0);
        let world_soups = bsp.models[0].num_soups as usize;
        assert!(all.surfaces.len() < world_soups, "nomarks soups dropped");
        // the retail facing normal agrees with the stored vertex normals,
        // so a floor's triangles face up
        let mut agree = 0;
        let mut total = 0;
        for soup in &bsp.soups[..world_soups] {
            let fv = soup.first_vertex as usize;
            let fi = soup.first_index as usize;
            for t in bsp.indices[fi..fi + soup.index_count as usize]
                .as_chunks::<3>()
                .0
            {
                let v = t.map(|i| bsp.verts[fv + i as usize]);
                let p = v.map(|v| Vec3::from_array(v.pos));
                let n = (p[0] - p[1]).cross(p[2] - p[1]);
                let stored = Vec3::from_array(v[0].normal);
                if n.length() > 1e-3 && stored.length() > 0.5 {
                    total += 1;
                    if n.dot(stored) > 0.0 {
                        agree += 1;
                    }
                }
            }
        }
        assert!(agree * 100 > total * 99, "{agree} of {total}");
    }
}
