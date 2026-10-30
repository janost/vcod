//! Which soups a camera sees, decided the way the retail client does it:
//! docs/research/bsp-ibsp59-format.md, "How the retail client draws the world".

use crate::mapworld;
use crate::mapworld::MapWorld;
use glam::{Mat4, Vec3};

/// Planes as `(n, d)`, inside where `n·p + d >= 0`.
#[derive(Clone, Debug)]
pub struct Frustum {
    pub planes: Vec<(Vec3, f32)>,
}

impl Frustum {
    /// Gribb-Hartmann on a column-major clip matrix with 0..1 depth.
    pub fn from_view_proj(m: Mat4) -> Self {
        let r = |i| m.row(i);
        let rows = [
            r(3) + r(0),
            r(3) - r(0),
            r(3) + r(1),
            r(3) - r(1),
            r(2),
            r(3) - r(2),
        ];
        let planes = rows
            .iter()
            .map(|v| {
                let n = v.truncate();
                let len = n.length();
                (n / len, v.w / len)
            })
            .collect();
        Frustum { planes }
    }

    pub fn contains_point(&self, p: Vec3) -> bool {
        self.planes.iter().all(|(n, d)| n.dot(p) + d >= 0.0)
    }

    /// Conservative: tests the corner furthest along each plane normal.
    pub fn intersects_aabb(&self, lo: Vec3, hi: Vec3) -> bool {
        self.planes.iter().all(|(n, d)| {
            let p = Vec3::select(n.cmpge(Vec3::ZERO), hi, lo);
            n.dot(p) + d >= 0.0
        })
    }

    /// One plane per polygon edge through `eye`, oriented so the polygon's
    /// centroid is inside; the child frustum of a portal.
    pub fn from_polygon(eye: Vec3, poly: &[Vec3]) -> Self {
        let centroid = poly.iter().sum::<Vec3>() / poly.len() as f32;
        let planes = (0..poly.len())
            .filter_map(|i| {
                let a = poly[i] - eye;
                let b = poly[(i + 1) % poly.len()] - eye;
                let n = a.cross(b).try_normalize()?;
                let n = if n.dot(centroid - eye) >= 0.0 { n } else { -n };
                Some((n, -n.dot(eye)))
            })
            .collect();
        Frustum { planes }
    }

    /// [`Self::from_polygon`] plus the four screen-space bevel planes retail
    /// adds when `r_portalbevels > 0` (default 0.7, so always on): the portal
    /// polygon's projection is clamped to the screen box `[-1,1]` and one
    /// plane is built through the eye and each edge of that box. Hugging the
    /// silhouette keeps a frustum narrowed too far by near-plane clipping
    /// from admitting cells beside the portal (CoDMP.exe `0x4e19b0`).
    pub fn from_polygon_beveled(eye: Vec3, poly: &[Vec3], view_proj: Mat4) -> Self {
        let mut f = Self::from_polygon(eye, poly);
        let (mut s0, mut s1, mut t0, mut t1) = (1.0f32, -1.0f32, 1.0f32, -1.0f32);
        for &v in poly {
            let c = view_proj * glam::Vec4::new(v.x, v.y, v.z, 1.0);
            let (s, t) = (c.x / c.w, c.y / c.w);
            s0 = s0.min(s);
            s1 = s1.max(s);
            t0 = t0.min(t);
            t1 = t1.max(t);
        }
        let (s0, s1, t0, t1) = (
            s0.clamp(-1.0, 1.0),
            s1.clamp(-1.0, 1.0),
            t0.clamp(-1.0, 1.0),
            t1.clamp(-1.0, 1.0),
        );
        let inv = view_proj.inverse();
        // unproject the clip-space point (mind the perspective divide), then
        // aim the ray from the eye
        let ray = |s: f32, t: f32| {
            let p = inv.mul_vec4(glam::Vec4::new(s, t, 1.0, 1.0));
            ((p.truncate() / p.w) - eye).normalize()
        };
        let centroid = poly.iter().sum::<Vec3>() / poly.len() as f32;
        // clamping can collapse the box along an axis; merge corners that landed
        // on the same spot modulo float dust (a segment yields one plane
        // through its two surviving rays)
        let mut uniq: Vec<(f32, f32)> = Vec::new();
        for c in [(s0, t0), (s1, t0), (s1, t1), (s0, t1)] {
            if !uniq
                .iter()
                .any(|p| (p.0 - c.0).abs() < 1e-4 && (p.1 - c.1).abs() < 1e-4)
            {
                uniq.push(c);
            }
        }
        if uniq.len() > 1 {
            for k in 0..uniq.len() {
                let (sa, ta) = uniq[k];
                let (sb, tb) = uniq[(k + 1) % uniq.len()];
                let (ra, rb) = (ray(sa, ta), ray(sb, tb));
                // near-collinear rays mean a collapsed edge: skip it
                if ra.dot(rb) > 0.999_999 {
                    continue;
                }
                if let Some(mut n) = ra.cross(rb).try_normalize() {
                    // contains the eye by construction; point it so the portal
                    // polygon stays on the inside, like the edge planes above
                    if n.dot(centroid - eye) < 0.0 {
                        n = -n;
                    }
                    f.planes.push((n, -n.dot(eye)));
                }
            }
        }
        f
    }
}

/// [`WorldVis::walk_queued`]'s portal state: the chopped points each queued
/// portal gathered, the single portal that queued it (`None` once two did),
/// the chain the current cell was entered through, the shallowest visit
/// that queued it, and the min-heap.
struct PortalQueue {
    points: Vec<Vec<Vec3>>,
    queued: Vec<bool>,
    depth: Vec<u8>,
    parent: Vec<Option<usize>>,
    ancestor: Vec<bool>,
    heap: std::collections::BinaryHeap<std::cmp::Reverse<(OrdF32, usize)>>,
}

#[derive(Clone, Copy, PartialEq)]
struct OrdF32(f32);
impl Eq for OrdF32 {}
impl PartialOrd for OrdF32 {
    fn partial_cmp(&self, o: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(o))
    }
}
impl Ord for OrdF32 {
    fn cmp(&self, o: &Self) -> std::cmp::Ordering {
        self.0.total_cmp(&o.0)
    }
}

impl PortalQueue {
    fn push(&mut self, dist: f32, pi: usize) {
        self.queued[pi] = true;
        self.heap.push(std::cmp::Reverse((OrdF32(dist), pi)));
    }

    fn pop(&mut self) -> Option<(f32, usize)> {
        let std::cmp::Reverse((d, pi)) = self.heap.pop()?;
        self.queued[pi] = false;
        Some((d.0, pi))
    }

    /// `R_SetAncestorListStatus`: the portal and the parents it was queued
    /// by, while they have one.
    fn set_ancestors(&mut self, mut p: Option<usize>, on: bool) {
        let mut steps = self.ancestor.len();
        while let Some(pi) = p {
            if steps == 0 || self.ancestor[pi] == on {
                break;
            }
            steps -= 1;
            self.ancestor[pi] = on;
            p = self.parent[pi];
        }
    }
}

/// The convex hull of points on a plane with normal `n`, wound around it
/// (`Com_ConvexHull` on the portal's hull axes).
fn convex_hull_on_plane(points: &[Vec3], n: Vec3) -> Vec<Vec3> {
    if points.len() < 3 {
        return Vec::new();
    }
    let u = n.any_orthonormal_vector();
    let v = n.cross(u);
    let mut pts: Vec<(f32, f32, Vec3)> = points.iter().map(|&p| (p.dot(u), p.dot(v), p)).collect();
    pts.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.total_cmp(&b.1)));
    pts.dedup_by(|a, b| (a.0 - b.0).abs() < 1e-3 && (a.1 - b.1).abs() < 1e-3);
    if pts.len() < 3 {
        return Vec::new();
    }
    let cross = |o: &(f32, f32, Vec3), a: &(f32, f32, Vec3), b: &(f32, f32, Vec3)| {
        (a.0 - o.0) * (b.1 - o.1) - (a.1 - o.1) * (b.0 - o.0)
    };
    let mut hull: Vec<(f32, f32, Vec3)> = Vec::with_capacity(pts.len() * 2);
    for pass in 0..2 {
        let start = hull.len();
        let iter: Box<dyn Iterator<Item = &(f32, f32, Vec3)>> = if pass == 0 {
            Box::new(pts.iter())
        } else {
            Box::new(pts.iter().rev())
        };
        for p in iter {
            while hull.len() >= start + 2
                && cross(&hull[hull.len() - 2], &hull[hull.len() - 1], p) <= 0.0
            {
                hull.pop();
            }
            hull.push(*p);
        }
        hull.pop();
    }
    hull.into_iter().map(|p| p.2).collect()
}

/// Whether `p`, on the winding's plane, lies inside it (`ProjectedWinding
/// ContainsCoplanarPoint`): on the inner side of every edge.
/// Whether a popped portal's hull covers `min_area` of the screen, so its
/// cell queues its own portals (`R_VisitPortals`, CoD2 Mac 0xf14c0): the
/// share of the screen's 2 x 2 its points' NDC rectangle covers, then,
/// past `min_area`, the polygon's own. A point within 0.125 of the eye
/// plane (clip w) always clips.
fn clips_children(hull: &[Vec3], view_proj: Mat4, min_area: f32) -> bool {
    let mut ndc = Vec::with_capacity(hull.len());
    for &p in hull {
        let c = view_proj * p.extend(1.0);
        if c.w < 0.125 {
            return true;
        }
        ndc.push(glam::Vec2::new(c.x / c.w, c.y / c.w));
    }
    let (lo, hi) = ndc
        .iter()
        .fold((glam::Vec2::ONE, glam::Vec2::NEG_ONE), |(lo, hi), &v| {
            (lo.min(v), hi.max(v))
        });
    if (hi.x - lo.x) * (hi.y - lo.y) * 0.25 < min_area {
        return false;
    }
    // the shoelace sum, twice the area; retail's winding makes it
    // positive, vcod's hull may wind either way
    let n = ndc.len();
    let twice: f32 = (0..n)
        .map(|i| ndc[i].x * (ndc[(i + 1) % n].y - ndc[(i + n - 1) % n].y))
        .sum();
    (twice * 0.125).abs() >= min_area
}

fn point_in_winding(p: Vec3, verts: &[Vec3], n: Vec3) -> bool {
    let mut sign = 0.0f32;
    for i in 0..verts.len() {
        let e = verts[(i + 1) % verts.len()] - verts[i];
        let s = n.dot(e.cross(p - verts[i]));
        if s != 0.0 {
            if sign != 0.0 && s.signum() != sign {
                return false;
            }
            sign = s.signum();
        }
    }
    true
}

/// Below this angular width (radians) a clipped portal is a numerical sliver
/// at a shared edge, not an opening: its cone loses planes or wobbles.
const SLIVER_EPS: f32 = 1e-4;
/// Occluder box planes are pulled in by this (units) at build time, like
/// retail's `d - eps` at load, so a portal brushing a wall is not occluded.
const OCC_D_EPS: f32 = 1e-3;

/// The polygon's narrowest extent over its distance from `eye`: the angle
/// it subtends across its thin direction.
fn angular_width(eye: Vec3, poly: &[Vec3]) -> f32 {
    let mut area2 = 0.0;
    for i in 1..poly.len().saturating_sub(1) {
        area2 += (poly[i] - poly[0]).cross(poly[i + 1] - poly[0]).length();
    }
    let mut longest = 0.0f32;
    for i in 0..poly.len() {
        for j in i + 1..poly.len() {
            longest = longest.max((poly[i] - poly[j]).length());
        }
    }
    let dist = poly
        .iter()
        .map(|v| (*v - eye).length())
        .fold(f32::INFINITY, f32::min);
    if longest <= 0.0 || dist <= 0.0 {
        return 0.0;
    }
    area2 / longest / dist
}

/// Sutherland-Hodgman against one plane, keeping `n·p + d >= 0`.
pub fn clip_polygon(poly: &[Vec3], (n, d): (Vec3, f32)) -> Vec<Vec3> {
    let mut out = Vec::with_capacity(poly.len() + 1);
    for i in 0..poly.len() {
        let a = poly[i];
        let b = poly[(i + 1) % poly.len()];
        let da = n.dot(a) + d;
        let db = n.dot(b) + d;
        if da >= 0.0 {
            out.push(a);
        }
        if (da >= 0.0) != (db >= 0.0) {
            let t = da / (da - db);
            out.push(a + (b - a) * t);
        }
    }
    out
}

#[derive(Default, Clone, Debug)]
pub struct VisStats {
    pub cells_visited: usize,
    pub soups: usize,
    pub nodes_tested: usize,
    /// Occluder volumes built for the visited cells.
    pub occluders: usize,
    /// Portals hidden because they sat behind an occluder volume.
    pub portals_occluded: usize,
    /// No camera cell: every tree and cull group was frustum-tested.
    pub fallback: bool,
}

pub struct Visible {
    pub soups: Vec<bool>,
    pub cells: Vec<bool>,
    pub stats: VisStats,
}

/// One occluder's geometry, resolved from lumps 12-15 at build time. Hidden
/// here (outside `bsp`) because it is per-frame eye work, not a lump parse.
struct Occ {
    /// Box planes, `d` pulled in by a hair: a portal brushing the wall is
    /// not occluded by it.
    planes: Vec<(Vec3, f32)>,
    verts: Vec<Vec3>,
    /// `(plane_a, plane_b, vert_a, vert_b)`; the two planes the edge shares
    /// and the two verts it spans.
    edges: Vec<[usize; 4]>,
}

pub struct WorldVis {
    soup_bounds: Vec<(Vec3, Vec3)>,
    node_bounds: Vec<(Vec3, Vec3)>,
    /// Preorder end of each node's subtree; 0 for nodes no cell reaches.
    tree_end: Vec<u32>,
    aabb_nodes: Vec<mapworld::AabbNode>,
    cull_groups: Vec<mapworld::CullGroup>,
    cells: Vec<mapworld::Cell>,
    nodes: Vec<mapworld::Node>,
    leafs: Vec<mapworld::Leaf>,
    /// `(n, dist)`, front where `n·p - dist > 0`.
    planes: Vec<(Vec3, f32)>,
    cull_indices: Vec<u32>,
    /// Per cell, its range of `group_nodes`: the cell's cull groups as a
    /// box tree, so a cell whose static models lie out of view tests a few
    /// boxes, not one a placement ([`WorldVis::mark_cell`]).
    cell_groups: Vec<std::ops::Range<u32>>,
    group_nodes: Vec<GroupNode>,
    /// The groups the tree's leaves hold, into `cull_groups`.
    group_order: Vec<u32>,
    portals: Vec<mapworld::Portal>,
    portal_verts: Vec<Vec3>,
    occluders: Vec<Occ>,
    occluder_indices: Vec<u16>,
    /// Per brush model past the world, its bounds and its soups. No cell
    /// lists them; they draw only through their entity.
    brush_models: Vec<((Vec3, Vec3), std::ops::Range<usize>)>,
    /// [`MapWorld::portals_only`].
    portals_only: bool,
    /// [`MapWorld::portal_min_clip_area`].
    min_clip_area: f32,
    /// [`MapWorld::portal_min_recurse_depth`].
    min_recurse_depth: u8,
}

/// State threaded through the portal recursion: the eye and the unnarrowed
/// frustum are fixed for the walk, the marks and the portal stack accumulate.
struct Walk<'a> {
    eye: Vec3,
    camera: &'a Frustum,
    /// Enables the bevel planes of child frusta; `None` for synthetic tests.
    view_proj: Option<Mat4>,
    vis: Visible,
    on_stack: Vec<bool>,
    /// Cells whose boundary portal an occluder volume hid; the post-walk
    /// frustum expansion must not reach them (an_occluder_hides_the_portal).
    sealed: Vec<bool>,
    /// Each cell's occluder volumes, built on its first visit: they depend
    /// on the cell and the eye alone, and the walk revisits cells.
    volumes: Vec<Option<std::rc::Rc<Volumes>>>,
}

/// A cell's occluder volumes, each a set of planes.
type Volumes = Vec<Vec<(Vec3, f32)>>;

const EMPTY: (Vec3, Vec3) = (Vec3::INFINITY, Vec3::NEG_INFINITY);

/// One node of a cell's cull group tree: its box holds every group under
/// it, `end` is its subtree's preorder end, and a leaf lists its groups
/// (`first..first + count` of `WorldVis::group_order`).
#[derive(Clone, Copy)]
struct GroupNode {
    lo: Vec3,
    hi: Vec3,
    end: u32,
    first: u32,
    count: u32,
}

/// Groups a leaf of the tree holds at most.
const GROUP_LEAF: usize = 8;

/// The tree over `idx`'s groups, appended in preorder: split at the
/// median of the box centres along the longest axis down to
/// [`GROUP_LEAF`] groups.
fn build_group_tree(
    idx: &mut [u32],
    groups: &[mapworld::CullGroup],
    nodes: &mut Vec<GroupNode>,
    order: &mut Vec<u32>,
) {
    let bounds = |g: u32| {
        let g = &groups[g as usize];
        (Vec3::from(g.mins), Vec3::from(g.maxs))
    };
    let (lo, hi) = idx.iter().fold(EMPTY, |b, &g| union(b, bounds(g)));
    let at = nodes.len();
    nodes.push(GroupNode {
        lo,
        hi,
        end: 0,
        first: order.len() as u32,
        count: 0,
    });
    if idx.len() <= GROUP_LEAF {
        order.extend_from_slice(idx);
        nodes[at].count = idx.len() as u32;
    } else {
        let axis = (hi - lo).max_position();
        let centre = |g: u32| {
            let (a, b) = bounds(g);
            (a[axis] + b[axis]) * 0.5
        };
        idx.sort_by(|&a, &b| centre(a).total_cmp(&centre(b)));
        let (a, b) = idx.split_at_mut(idx.len() / 2);
        build_group_tree(a, groups, nodes, order);
        build_group_tree(b, groups, nodes, order);
    }
    nodes[at].end = nodes.len() as u32;
}

/// A soup's bounds over the vertices its indices reach, as CoD2's
/// `R_LoadSurfaces` (Mac 0xe4672) bounds a surface: a d3dbsp surface's
/// vertex run is its whole material batch, so bounding the run made every
/// CoD2 soup map-sized and the frustum culled none
/// (cod2-d3dbsp-format.md, "Surface bounds"). A CoD1 soup's indices reach
/// its whole run, so its bounds are the run's.
pub fn soup_bounds(bsp: &MapWorld, soup: usize) -> (Vec3, Vec3) {
    let s = &bsp.soups[soup];
    let (fv, fi) = (s.first_vertex as usize, s.first_index as usize);
    bsp.indices[fi..fi + s.index_count as usize]
        .iter()
        .filter_map(|&i| bsp.verts.get(fv + i as usize))
        .fold(EMPTY, |b, v| {
            union(b, (Vec3::from(v.pos), Vec3::from(v.pos)))
        })
}

fn union(a: (Vec3, Vec3), b: (Vec3, Vec3)) -> (Vec3, Vec3) {
    (a.0.min(b.0), a.1.max(b.1))
}

fn overlaps(a: (Vec3, Vec3), lo: Vec3, hi: Vec3) -> bool {
    lo.cmple(a.1).all() && hi.cmpge(a.0).all()
}

impl WorldVis {
    pub fn build(bsp: &MapWorld) -> Self {
        let soup_bounds: Vec<(Vec3, Vec3)> =
            (0..bsp.soups.len()).map(|s| soup_bounds(bsp, s)).collect();
        let mut node_bounds = vec![EMPTY; bsp.aabb_nodes.len()];
        let mut tree_end = vec![0u32; bsp.aabb_nodes.len()];
        for cell in &bsp.cells {
            let root = cell.first_aabb as usize;
            if root < bsp.aabb_nodes.len() && tree_end[root] == 0 {
                walk_bounds(
                    root,
                    0,
                    &bsp.aabb_nodes,
                    &soup_bounds,
                    &mut node_bounds,
                    &mut tree_end,
                );
            }
        }
        let (mut group_nodes, mut group_order) = (Vec::new(), Vec::new());
        let cell_groups = bsp
            .cells
            .iter()
            .map(|c| {
                let start = group_nodes.len() as u32;
                let mut idx: Vec<u32> = bsp
                    .cull_indices
                    .get(c.first_cull_index as usize..(c.first_cull_index + c.cull_count) as usize)
                    .unwrap_or_default()
                    .iter()
                    .copied()
                    .filter(|&g| (g as usize) < bsp.cull_groups.len())
                    .collect();
                if !idx.is_empty() {
                    build_group_tree(
                        &mut idx,
                        &bsp.cull_groups,
                        &mut group_nodes,
                        &mut group_order,
                    );
                }
                start..group_nodes.len() as u32
            })
            .collect();
        let brush_models = bsp
            .models
            .iter()
            .skip(1)
            .map(|m| m.first_soup as usize..(m.first_soup + m.num_soups) as usize)
            .filter(|r| !r.is_empty() && r.end <= soup_bounds.len())
            .map(|r| (r.clone().fold(EMPTY, |b, s| union(b, soup_bounds[s])), r))
            .collect();
        WorldVis {
            portals_only: bsp.portals_only,
            min_clip_area: bsp.portal_min_clip_area,
            min_recurse_depth: bsp.portal_min_recurse_depth,
            brush_models,
            soup_bounds,
            node_bounds,
            tree_end,
            aabb_nodes: bsp.aabb_nodes.clone(),
            cull_groups: bsp.cull_groups.clone(),
            cells: bsp.cells.clone(),
            nodes: bsp.nodes.clone(),
            leafs: bsp.leafs.clone(),
            planes: bsp
                .planes
                .iter()
                .map(|p| (Vec3::from(p.normal), p.dist))
                .collect(),
            cull_indices: bsp.cull_indices.clone(),
            cell_groups,
            group_nodes,
            group_order,
            portals: bsp.portals.clone(),
            portal_verts: bsp.portal_verts.iter().map(|&v| Vec3::from(v)).collect(),
            occluders: bsp
                .occluders
                .iter()
                .map(|oc| {
                    let pb = oc.first_plane as usize;
                    let planes = bsp.occluder_plane_indices[pb..pb + oc.num_planes as usize]
                        .iter()
                        .map(|&pi| {
                            let p = &bsp.planes[pi as usize];
                            (Vec3::from(p.normal), p.dist - OCC_D_EPS)
                        })
                        .collect();
                    let vb = oc.first_vert as usize;
                    let verts = bsp.portal_verts[vb..vb + oc.vert_count as usize]
                        .iter()
                        .map(|&v| Vec3::from(v))
                        .collect();
                    let eb = oc.first_edge as usize;
                    // edge vertex bytes are `first_vert + local (mod 256)`
                    let r = (oc.first_vert % 256) as usize;
                    let local = |b: u8| ((b as usize) + 256 - r) % 256;
                    let edges = bsp.occluder_edges[eb..eb + oc.num_edges as usize]
                        .iter()
                        .map(|e| [e[0] as usize, e[1] as usize, local(e[2]), local(e[3])])
                        .collect();
                    Occ {
                        planes,
                        verts,
                        edges,
                    }
                })
                .collect(),
            occluder_indices: bsp.occluder_indices.clone(),
        }
    }

    pub fn soup_count(&self) -> usize {
        self.soup_bounds.len()
    }

    pub fn cell_count(&self) -> usize {
        self.cells.len()
    }

    /// The leaf's cell after the BSP walk; `None` without a tree or in a
    /// solid leaf. Back side (`n·p - dist <= 0`) is `children[1]`, as retail.
    /// Bounded to `nodes.len()` steps: a hostile file with a child cycle
    /// must not spin forever.
    pub fn cell_for_point(&self, p: Vec3) -> Option<usize> {
        if self.nodes.is_empty() {
            return None;
        }
        let mut i = 0usize;
        for _ in 0..self.nodes.len() {
            let node = &self.nodes[i];
            let (n, dist) = self.planes[node.plane as usize];
            let c = node.children[usize::from(n.dot(p) - dist <= 0.0)];
            if c < 0 {
                let leaf = &self.leafs[(-(c as i64) - 1) as usize];
                return usize::try_from(leaf.cell)
                    .ok()
                    .filter(|&c| c < self.cells.len());
            }
            i = c as usize;
        }
        None
    }

    /// Frustum-only, as retail does when the eye is in no cell.
    pub fn visible_fallback(&self, frustum: &Frustum) -> Visible {
        let mut vis = self.empty_visible();
        vis.stats.fallback = true;
        for ci in 0..self.cells.len() {
            if self.mark_tree(self.cells[ci].first_aabb, frustum, &mut vis) {
                vis.cells[ci] = true;
                vis.stats.cells_visited += 1;
            }
        }
        for g in &self.cull_groups {
            self.mark_group(g, frustum, &mut vis);
        }
        self.mark_brush_models(frustum, &mut vis);
        vis.stats.soups = vis.soups.iter().filter(|&&s| s).count();
        vis
    }

    /// Retail's walk when the eye is in a cell, the frustum fallback otherwise.
    /// A `view_proj` enables the bevel planes of child frusta (retail's
    /// `r_portalbevels`, default on); synthetic test frusta pass `None`.
    pub fn visible(&self, eye: Vec3, frustum: &Frustum, view_proj: Option<Mat4>) -> Visible {
        let Some(cell) = self.cell_for_point(eye) else {
            return self.visible_fallback(frustum);
        };
        let mut w = Walk {
            eye,
            camera: frustum,
            view_proj,
            vis: self.empty_visible(),
            on_stack: vec![false; self.portals.len()],
            sealed: vec![false; self.cells.len()],
            volumes: vec![None; self.cells.len()],
        };
        if self.portals_only {
            self.walk_queued(cell, frustum, &mut w);
            self.mark_brush_models(frustum, &mut w.vis);
            w.vis.stats.soups = w.vis.soups.iter().filter(|&&s| s).count();
            return w.vis;
        }
        self.walk(cell, frustum, &mut w, 0);
        // above a cell's top the eye looks over its portal-less walls, which
        // the graph treats as full height: such cells are open from above
        for (ci, c) in self.cells.iter().enumerate() {
            if ci == cell || c.maxs[2] >= eye.z {
                continue;
            }
            if self.mark_tree(c.first_aabb, frustum, &mut w.vis) {
                w.vis.cells[ci] = true;
            }
            for &gi in &self.cull_indices
                [c.first_cull_index as usize..(c.first_cull_index + c.cull_count) as usize]
            {
                self.mark_group(&self.cull_groups[gi as usize], frustum, &mut w.vis);
            }
        }
        // Sightlines that leave the graph over low geometry — bulwarks, rails
        // (mp_ship's deck over the ocean) — under-mark: the walk treats those
        // walls as sealed. Any cell sharing a portal with a visited cell is
        // frustum-tested directly, to a fixpoint; the camera frustum is the
        // conservative bound, same as the fallback path.
        let mut grew = true;
        while grew {
            grew = false;
            for (ci, c) in self.cells.iter().enumerate() {
                if w.vis.cells[ci] || w.sealed[ci] {
                    continue;
                }
                let touches_visited = (c.first_portal as usize
                    ..(c.first_portal + c.portal_count) as usize)
                    .any(|pi| w.vis.cells[self.portals[pi].cell as usize]);
                if !touches_visited
                    || !frustum.intersects_aabb(Vec3::from(c.mins), Vec3::from(c.maxs))
                {
                    continue;
                }
                if self.mark_tree(c.first_aabb, frustum, &mut w.vis) {
                    w.vis.cells[ci] = true;
                    w.vis.stats.cells_visited += 1;
                    grew = true;
                }
                for &gi in &self.cull_indices
                    [c.first_cull_index as usize..(c.first_cull_index + c.cull_count) as usize]
                {
                    self.mark_group(&self.cull_groups[gi as usize], frustum, &mut w.vis);
                }
            }
        }
        self.mark_brush_models(frustum, &mut w.vis);
        w.vis.stats.soups = w.vis.soups.iter().filter(|&&s| s).count();
        w.vis
    }

    /// Marks the cell's geometry, then recurses through every portal whose
    /// polygon still has area inside `frustum`, narrowed to that polygon.
    fn walk(&self, cell: usize, frustum: &Frustum, w: &mut Walk, depth: usize) {
        // retail has no depth limit; this only guards hostile data
        if depth > 64 {
            return;
        }
        // a portal graph that revisits cells cannot cost more than this
        if w.vis.stats.cells_visited > 8 * self.cells.len() {
            return;
        }
        w.vis.cells[cell] = true;
        w.vis.stats.cells_visited += 1;
        let c = self.cells[cell];
        self.mark_tree(c.first_aabb, frustum, &mut w.vis);
        for &gi in &self.cull_indices
            [c.first_cull_index as usize..(c.first_cull_index + c.cull_count) as usize]
        {
            self.mark_group(&self.cull_groups[gi as usize], frustum, &mut w.vis);
        }
        // the cell's occluder volumes, built once against the fixed eye; each
        // volume is a set of planes a portal is hidden behind when every one
        // of its vertices is on the negative side of them all (CoDMP 0x4e2860)
        let volumes = w.volumes[cell]
            .get_or_insert_with(|| std::rc::Rc::new(self.occluder_volumes(cell, w.eye)))
            .clone();
        w.vis.stats.occluders += volumes.len();
        let near = w.camera.planes.get(4).copied();
        for pi in c.first_portal as usize..(c.first_portal + c.portal_count) as usize {
            if w.on_stack[pi] {
                continue;
            }
            let p = self.portals[pi];
            let (n, dist) = self.planes[p.plane as usize];
            // the plane faces out of this cell; past it the portal shows nothing
            let side = n.dot(w.eye) - dist;
            if side > 1.0 {
                continue;
            }
            let verts =
                &self.portal_verts[p.first_vert as usize..(p.first_vert + p.vert_count) as usize];
            if !volumes.is_empty() && self.portal_in_occluders(&p, &volumes) {
                w.vis.stats.portals_occluded += 1;
                w.sealed[p.cell as usize] = true;
                continue;
            }
            let mut poly = verts.to_vec();
            // a portal closer than the near plane still shows its cell, so that
            // one plane is left out; every frustum here carries the camera's
            // copy of it, the parent's own or the one appended to the cone
            for &plane in frustum.planes.iter().filter(|&&p| Some(p) != near) {
                poly = clip_polygon(&poly, plane);
                if poly.len() < 3 {
                    break;
                }
            }
            if poly.len() < 3 || angular_width(w.eye, &poly) < SLIVER_EPS {
                continue;
            }
            let mut child;
            // within a unit of the plane the polygon degenerates: keep the parent
            let next = if side >= -1.0 {
                frustum
            } else {
                child = match w.view_proj {
                    Some(m) => Frustum::from_polygon_beveled(w.eye, &poly, m),
                    None => Frustum::from_polygon(w.eye, &poly),
                };
                // every edge plane runs through the eye, so the cone reaches
                // backwards and past the far plane; the camera's near and far
                // planes cap it, its side planes already contain it
                child
                    .planes
                    .extend_from_slice(w.camera.planes.get(4..6).unwrap_or_default());
                &child
            };
            w.on_stack[pi] = true;
            self.walk(p.cell as usize, next, w, depth + 1);
            w.on_stack[pi] = false;
        }
    }

    /// CoD4 1.7's walk (`R_VisitPortals`, docs/research/cod4-world.md
    /// 9.1, "The portal walk") and CoD2's, the same queue
    /// (cod2-client-perf.md 9): a cell's portals are chopped to its
    /// frustum and queued, nearest first; a portal reached again before
    /// its turn gathers the new points, and on its turn it is opened once,
    /// through the convex hull of everything gathered. The recursive
    /// [`Self::walk`] opens a portal once per path, which on mp_overgrown's
    /// sky cell ran past its visit cap and dropped whole rooms.
    fn walk_queued(&self, cell: usize, frustum: &Frustum, w: &mut Walk) {
        let n = self.portals.len();
        let mut q = PortalQueue {
            points: vec![Vec::new(); n],
            queued: vec![false; n],
            depth: vec![0; n],
            parent: vec![None; n],
            ancestor: vec![false; n],
            heap: std::collections::BinaryHeap::new(),
        };
        self.visit_queued(cell, None, 0, frustum, w, &mut q, true);
        // retail stops at 256 queued portals (`Com_Error`); a hostile graph
        // that requeues forever stops here instead
        let mut budget = 64 * n.max(1);
        while let Some((_, pi)) = q.pop() {
            let points = std::mem::take(&mut q.points[pi]);
            budget -= 1;
            if budget == 0 {
                break;
            }
            let p = self.portals[pi];
            let (pn, _) = self.planes[p.plane as usize];
            let hull = convex_hull_on_plane(&points, pn);
            if hull.len() < 3 || angular_width(w.eye, &hull) < SLIVER_EPS {
                continue;
            }
            let mut child = match w.view_proj {
                Some(m) => Frustum::from_polygon_beveled(w.eye, &hull, m),
                None => Frustum::from_polygon(w.eye, &hull),
            };
            child
                .planes
                .extend_from_slice(w.camera.planes.get(4..6).unwrap_or_default());
            // CoD4's `r_portalMinRecurseDepth`: shallow portals always
            // queue their cell's portals (iw3mp.exe 0x60a6ca..0x60a6e0)
            let depth = q.depth[pi];
            let clip_children = match w.view_proj {
                _ if depth < self.min_recurse_depth => true,
                Some(m) if self.min_clip_area > 0.0 => clips_children(&hull, m, self.min_clip_area),
                _ => true,
            };
            self.visit_queued(
                p.cell as usize,
                Some(pi),
                depth.saturating_add(1),
                &child,
                w,
                &mut q,
                clip_children,
            );
        }
    }

    /// `R_VisitPortalsForCell`: marks the cell under `frustum`, then queues
    /// each of its portals still in view, chopped to `frustum`. A portal the
    /// eye stands in (within 1/8 unit of its plane) is entered at once
    /// under the same frustum. `depth` counts the portals opened on the
    /// way here (0 at the eye's cell); each portal queued keeps the
    /// shallowest it was queued from (iw3mp.exe 0x60a289, 0x60a2aa).
    #[allow(clippy::too_many_arguments)]
    fn visit_queued(
        &self,
        cell: usize,
        parent: Option<usize>,
        depth: u8,
        frustum: &Frustum,
        w: &mut Walk,
        q: &mut PortalQueue,
        clip_children: bool,
    ) {
        w.vis.cells[cell] = true;
        w.vis.stats.cells_visited += 1;
        let c = self.cells[cell];
        self.mark_cell(cell, frustum, &mut w.vis);
        q.set_ancestors(parent, true);
        let volumes = self.occluder_volumes(cell, w.eye);
        w.vis.stats.occluders += volumes.len();
        let near = w.camera.planes.get(4).copied();
        // a portal too small on screen to queue its cell's portals: the
        // cells beyond under this frustum instead (`DpvsClipChildren` 0)
        if let (false, Some(pp)) = (clip_children, parent) {
            let (pn, pd) = self.planes[self.portals[pp].plane as usize];
            let mut list = Vec::new();
            self.further_cells(
                cell,
                (pn, -pd),
                frustum,
                near,
                &volumes,
                w.eye,
                q,
                &mut list,
            );
            for c in list {
                w.vis.cells[c] = true;
                w.vis.stats.cells_visited += 1;
                self.mark_cell(c, frustum, &mut w.vis);
            }
            q.set_ancestors(parent, false);
            return;
        }
        for pi in c.first_portal as usize..(c.first_portal + c.portal_count) as usize {
            if q.ancestor[pi] {
                continue;
            }
            let p = self.portals[pi];
            let (n, dist) = self.planes[p.plane as usize];
            let side = n.dot(w.eye) - dist;
            if side > 0.0 {
                continue;
            }
            let verts =
                &self.portal_verts[p.first_vert as usize..(p.first_vert + p.vert_count) as usize];
            if !volumes.is_empty() && self.portal_in_occluders(&p, &volumes) {
                w.vis.stats.portals_occluded += 1;
                continue;
            }
            if side > -0.125 {
                // the eye stands in the portal: enter it unnarrowed when the
                // eye lies inside its winding
                if point_in_winding(w.eye, verts, n) && !w.on_stack[pi] {
                    w.on_stack[pi] = true;
                    self.visit_queued(
                        p.cell as usize,
                        Some(pi),
                        depth.saturating_add(1),
                        frustum,
                        w,
                        q,
                        clip_children,
                    );
                    w.on_stack[pi] = false;
                }
                continue;
            }
            let mut poly = verts.to_vec();
            // the entering portal's plane, then the frustum (its near plane
            // left out, as in `walk`)
            if let Some(pp) = parent {
                let (pn, pd) = self.planes[self.portals[pp].plane as usize];
                poly = clip_polygon(&poly, (pn, -pd));
            }
            for &plane in frustum.planes.iter().filter(|&&p| Some(p) != near) {
                if poly.len() < 3 {
                    break;
                }
                poly = clip_polygon(&poly, plane);
            }
            if poly.len() < 3 {
                continue;
            }
            q.points[pi].extend_from_slice(&poly);
            if q.queued[pi] {
                if q.parent[pi] != parent {
                    q.parent[pi] = None;
                }
                q.depth[pi] = q.depth[pi].min(depth);
            } else {
                q.parent[pi] = parent;
                q.depth[pi] = depth;
                // nearest first: the winding's furthest point along the view
                let fwd = w.camera.planes.get(4).map_or(Vec3::ZERO, |p| p.0);
                let far = verts
                    .iter()
                    .map(|&v| fwd.dot(v - w.eye))
                    .fold(f32::MIN, f32::max);
                q.push(far, pi);
            }
        }
        q.set_ancestors(parent, false);
    }

    /// `R_GetFurtherCellList_r` (CoD2 Mac 0xef68a): through each portal of
    /// `cell` not on the walk's path, facing away from the eye, clear of
    /// the visit's occluder `volumes` and left with area after the
    /// entering portal's plane `enter` and `frustum` (its near plane
    /// left out, as in `walk`), the cell beyond goes on `list` once and
    /// its own portals are tried the same way, under the same planes.
    /// Retail's list holds 384 (its stack array).
    #[allow(clippy::too_many_arguments)]
    fn further_cells(
        &self,
        cell: usize,
        enter: (Vec3, f32),
        frustum: &Frustum,
        near: Option<(Vec3, f32)>,
        volumes: &Volumes,
        eye: Vec3,
        q: &PortalQueue,
        list: &mut Vec<usize>,
    ) {
        let c = &self.cells[cell];
        for pi in c.first_portal as usize..(c.first_portal + c.portal_count) as usize {
            let p = self.portals[pi];
            let next = p.cell as usize;
            if list.contains(&next) || q.ancestor[pi] {
                continue;
            }
            let (n, dist) = self.planes[p.plane as usize];
            if n.dot(eye) - dist > 0.0 {
                continue;
            }
            if !volumes.is_empty() && self.portal_in_occluders(&p, volumes) {
                continue;
            }
            let verts =
                &self.portal_verts[p.first_vert as usize..(p.first_vert + p.vert_count) as usize];
            let mut poly = clip_polygon(verts, enter);
            for &plane in frustum.planes.iter().filter(|&&p| Some(p) != near) {
                if poly.len() < 3 {
                    break;
                }
                poly = clip_polygon(&poly, plane);
            }
            if poly.len() < 3 || list.len() >= 384 {
                continue;
            }
            list.push(next);
            self.further_cells(next, enter, frustum, near, volumes, eye, q, list);
        }
    }

    /// The cell's AABB tree and cull groups under `frustum`
    /// (`R_AddVisibleSurfacesInCell`).
    /// Every soup the listed cells' trees and cull groups hold, unculled:
    /// the most a walk that reaches those cells can mark.
    pub fn cell_soups(&self, cells: &[bool]) -> Vec<bool> {
        let mut vis = self.empty_visible();
        let all = Frustum { planes: Vec::new() };
        for (ci, _) in cells.iter().enumerate().filter(|&(_, &on)| on) {
            self.mark_cell(ci, &all, &mut vis);
        }
        vis.soups
    }

    fn mark_cell(&self, cell: usize, frustum: &Frustum, vis: &mut Visible) {
        let c = &self.cells[cell];
        self.mark_tree(c.first_aabb, frustum, vis);
        // the groups' tree: a box out of view holds groups out of view, so
        // the marks are the flat list's
        let range = self.cell_groups[cell].clone();
        let mut i = range.start;
        while i < range.end {
            let n = self.group_nodes[i as usize];
            if !frustum.intersects_aabb(n.lo, n.hi) {
                i = n.end;
                continue;
            }
            for &gi in &self.group_order[n.first as usize..(n.first + n.count) as usize] {
                self.mark_group(&self.cull_groups[gi as usize], frustum, vis);
            }
            i += 1;
        }
    }

    /// The cell's occluder volumes against `eye` (CoDMP 0x4e2860): each
    /// front-facing box plane stays, and every silhouette edge whose two
    /// planes straddle the eye contributes one plane through the eye. No
    /// volume (no occluders, or the call had none) is an empty slice.
    fn occluder_volumes(&self, cell: usize, eye: Vec3) -> Volumes {
        let c = &self.cells[cell];
        let list = &self.occluder_indices
            [c.first_occluder as usize..(c.first_occluder + c.occluder_count) as usize];
        list.iter()
            .filter_map(|&oi| {
                let occ = &self.occluders[oi as usize];
                let front: Vec<bool> = occ.planes.iter().map(|&(n, d)| n.dot(eye) > d).collect();
                if front.iter().all(|&f| !f) {
                    // every wall points away: the eye is past the occluder
                    return None;
                }
                let mut planes: Vec<(Vec3, f32)> = occ
                    .planes
                    .iter()
                    .zip(&front)
                    .filter_map(|(&(n, d), &f)| if f { Some((n, d)) } else { None })
                    .collect();
                for e in &occ.edges {
                    if front[e[0]] != front[e[1]] {
                        let a = occ.verts[e[2]] - eye;
                        let b = occ.verts[e[3]] - eye;
                        if let Some(mut n) = a.cross(b).try_normalize() {
                            // orient toward the occluder (its centroid is the
                            // negative side), so what lies beyond is under it
                            let centroid = occ.verts.iter().sum::<Vec3>() / occ.verts.len() as f32;
                            if n.dot(centroid - eye) > 0.0 {
                                n = -n;
                            }
                            // hidden side reads `n·v <= d`; the eye sits on the
                            // plane by construction, eps behind it hides just
                            planes.push((n, n.dot(eye) - OCC_D_EPS));
                        }
                    }
                }
                if planes.is_empty() {
                    None
                } else {
                    Some(planes)
                }
            })
            .collect()
    }

    /// Portal hidden when every vertex is on the negative side of every
    /// plane of one volume (CoDMP 0x4e2cc0).
    fn portal_in_occluders(&self, p: &mapworld::Portal, volumes: &[Vec<(Vec3, f32)>]) -> bool {
        let verts =
            &self.portal_verts[p.first_vert as usize..(p.first_vert + p.vert_count) as usize];
        volumes.iter().any(|volume| {
            volume
                .iter()
                .all(|&(n, d)| verts.iter().all(|&v| n.dot(v) <= d))
        })
    }

    /// In the fallback the frustum decides alone; otherwise the prop must
    /// also touch a visited cell, retail's per-cell static model list.
    pub fn prop_visible(&self, vis: &Visible, frustum: &Frustum, lo: Vec3, hi: Vec3) -> bool {
        if !frustum.intersects_aabb(lo, hi) {
            return false;
        }
        vis.stats.fallback
            || self
                .cells
                .iter()
                .zip(&vis.cells)
                .any(|(c, &v)| v && overlaps((Vec3::from(c.mins), Vec3::from(c.maxs)), lo, hi))
    }

    fn empty_visible(&self) -> Visible {
        Visible {
            soups: vec![false; self.soup_bounds.len()],
            cells: vec![false; self.cells.len()],
            stats: VisStats::default(),
        }
    }

    /// Preorder walk with `tree_end` skipping culled subtrees. Returns
    /// whether the root itself intersected the frustum.
    fn mark_tree(&self, root: u32, frustum: &Frustum, vis: &mut Visible) -> bool {
        let root = root as usize;
        if root >= self.aabb_nodes.len() {
            return false;
        }
        let end = self.tree_end[root] as usize;
        let mut i = root;
        let mut root_hit = false;
        while i < end {
            vis.stats.nodes_tested += 1;
            let (lo, hi) = self.node_bounds[i];
            if !frustum.intersects_aabb(lo, hi) {
                i = self.tree_end[i] as usize;
                continue;
            }
            if i == root {
                root_hit = true;
            }
            let node = &self.aabb_nodes[i];
            // a soup an earlier visit marked stays marked: no test
            for s in node.first_soup as usize..(node.first_soup + node.soup_count) as usize {
                if !vis.soups[s] {
                    let (lo, hi) = self.soup_bounds[s];
                    vis.soups[s] = frustum.intersects_aabb(lo, hi);
                }
            }
            i += 1;
        }
        root_hit
    }

    /// Every brush model whose bounds the frustum meets. Its soups are in no
    /// cell's tree or cull group: retail draws a brush model as its entity's
    /// ref entity, which the server's PVS cull already chose, so the caller
    /// drops the soups of a model no snapshot entity carries
    /// (docs/research/cod11-movers.md, "The client").
    fn mark_brush_models(&self, frustum: &Frustum, vis: &mut Visible) {
        for ((lo, hi), soups) in &self.brush_models {
            if frustum.intersects_aabb(*lo, *hi) {
                vis.soups[soups.clone()].fill(true);
            }
        }
    }

    /// Group bounds only; retail marks every surface of a hit group.
    fn mark_group(&self, g: &mapworld::CullGroup, frustum: &Frustum, vis: &mut Visible) {
        if frustum.intersects_aabb(Vec3::from(g.mins), Vec3::from(g.maxs)) {
            for s in g.first_soup as usize..(g.first_soup + g.soup_count) as usize {
                vis.soups[s] = true;
            }
        }
    }
}

/// Stock trees are a few levels deep; past this the file is hostile.
const MAX_TREE_DEPTH: usize = 128;

/// Bottom-up bounds and preorder ends; returns the index after the subtree.
/// Guards against a hostile forest two ways: an out-of-range or repeated
/// (`tree_end[i] != 0`) child is skipped, and `depth` stops the recursion at
/// `MAX_TREE_DEPTH` so a `child_count: 1` chain cannot overflow the stack.
fn walk_bounds(
    i: usize,
    depth: usize,
    nodes: &[mapworld::AabbNode],
    soup_bounds: &[(Vec3, Vec3)],
    node_bounds: &mut [(Vec3, Vec3)],
    tree_end: &mut [u32],
) -> usize {
    if depth > MAX_TREE_DEPTH {
        return i;
    }
    let node = nodes[i];
    let mut b = (node.first_soup as usize..(node.first_soup + node.soup_count) as usize)
        .fold(EMPTY, |b, s| union(b, soup_bounds[s]));
    let mut j = i + 1;
    for _ in 0..node.child_count {
        if j >= nodes.len() || tree_end[j] != 0 {
            break;
        }
        let child = j;
        j = walk_bounds(child, depth + 1, nodes, soup_bounds, node_bounds, tree_end);
        b = union(b, node_bounds[child]);
    }
    node_bounds[i] = b;
    tree_end[i] = j as u32;
    j
}

/// Two box cells, A at x < 0 and B at x > 0, joined by a square portal in
/// the x = 0 plane; one soup in each cell and one cull-group soup in A.
/// Always compiled: the client's gather tests use it.
#[doc(hidden)]
pub fn two_cell_world() -> MapWorld {
    use crate::mapworld::{
        AabbNode, Cell, CullGroup, DrawVert, Leaf, Material, Node, Plane, Portal, TriangleSoup,
    };
    let vert = |x: f32, y: f32, z: f32| DrawVert {
        pos: [x, y, z],
        uv: [0.0; 2],
        lm_uv: [0.0; 2],
        normal: [0.0, 0.0, 1.0],
        color: [255; 4],
    };
    // three triangles: A's, B's, and A's cull group up on the +Y wall. `soups`
    // reorders them, so soup 0 is the cull group, soup 1 is A, soup 2 is B
    let tri = |x: f32, y: f32| {
        [
            vert(x, y, -10.0),
            vert(x + 10.0, y, -10.0),
            vert(x, y, 10.0),
        ]
    };
    let verts: Vec<DrawVert> = [tri(-50.0, 0.0), tri(50.0, 0.0), tri(-50.0, 80.0)].concat();
    let soup = |i: u32| TriangleSoup {
        material: 0,
        lightmap: mapworld::NO_LIGHTMAP,
        first_vertex: i * 3,
        vertex_count: 3,
        index_count: 3,
        first_index: 0,
    };
    MapWorld {
        materials: vec![Material {
            name: "textures/test/wall".into(),
            surface_flags: 0,
            content_flags: 1,
        }],
        soups: vec![soup(2), soup(0), soup(1)],
        verts,
        indices: vec![0, 1, 2],
        planes: vec![
            Plane {
                normal: [1.0, 0.0, 0.0],
                dist: 0.0,
            },
            Plane {
                normal: [-1.0, 0.0, 0.0],
                dist: 0.0,
            },
        ],
        cull_groups: vec![CullGroup {
            mins: [-50.0, 80.0, -10.0],
            maxs: [-40.0, 80.0, 10.0],
            first_soup: 0,
            soup_count: 1,
        }],
        cull_indices: vec![0],
        portal_verts: vec![
            [0.0, -50.0, -50.0],
            [0.0, 50.0, -50.0],
            [0.0, 50.0, 50.0],
            [0.0, -50.0, 50.0],
            [0.0, -50.0, -50.0],
            [0.0, -50.0, 50.0],
            [0.0, 50.0, 50.0],
            [0.0, 50.0, -50.0],
        ],
        aabb_nodes: vec![
            AabbNode {
                first_soup: 1,
                soup_count: 1,
                child_count: 0,
            },
            AabbNode {
                first_soup: 2,
                soup_count: 1,
                child_count: 0,
            },
        ],
        cells: vec![
            Cell {
                mins: [-100.0, -100.0, -100.0],
                maxs: [0.0, 100.0, 100.0],
                first_aabb: 0,
                first_portal: 0,
                portal_count: 1,
                first_cull_index: 0,
                cull_count: 1,
                first_occluder: 0,
                occluder_count: 0,
            },
            Cell {
                mins: [0.0, -100.0, -100.0],
                maxs: [100.0, 100.0, 100.0],
                first_aabb: 1,
                first_portal: 1,
                portal_count: 1,
                first_cull_index: 1,
                cull_count: 0,
                first_occluder: 0,
                occluder_count: 0,
            },
        ],
        // a portal's plane faces out of its cell: A's points +X, B's points -X
        portals: vec![
            Portal {
                plane: 0,
                cell: 1,
                first_vert: 0,
                vert_count: 4,
            },
            Portal {
                plane: 1,
                cell: 0,
                first_vert: 4,
                vert_count: 4,
            },
        ],
        // front of plane 0 (x > 0) is B = leaf 1, back is A = leaf 0
        nodes: vec![Node {
            plane: 0,
            children: [-2, -1],
            mins: [-100; 3],
            maxs: [100; 3],
        }],
        leafs: vec![
            Leaf {
                cluster: 0,
                area: 0,
                cell: 0,
            },
            Leaf {
                cluster: 1,
                area: 0,
                cell: 1,
            },
        ],
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[allow(unused_imports)]
    use crate::mapworld::{
        AabbNode, Cell, DrawVert, Leaf, Material, Node, Plane, Portal, TriangleSoup,
    };

    /// Nearest corner along each normal: true only when the whole box is inside.
    fn fully_inside(f: &Frustum, lo: Vec3, hi: Vec3) -> bool {
        f.planes.iter().all(|(n, d)| {
            let p = Vec3::select(n.cmpge(Vec3::ZERO), lo, hi);
            n.dot(p) + d >= 0.0
        })
    }

    #[test]
    fn slivers_are_narrower_than_doorways() {
        let eye = Vec3::new(-1000.0, 0.0, 0.0);
        let door = [
            Vec3::new(0.0, -30.0, -60.0),
            Vec3::new(0.0, 30.0, -60.0),
            Vec3::new(0.0, 30.0, 60.0),
            Vec3::new(0.0, -30.0, 60.0),
        ];
        assert!(angular_width(eye, &door) > 0.05);
        let sliver = [
            Vec3::new(0.0, -30.0, 0.0),
            Vec3::new(0.0, 30.0, 0.0),
            Vec3::new(0.0, 30.0, 0.01),
            Vec3::new(0.0, -30.0, 0.01),
        ];
        assert!(angular_width(eye, &sliver) < SLIVER_EPS);
    }

    /// An eye above a cell's top looks over its portal-less walls; the cell
    /// is marked with the camera frustum, not only through its portals.
    #[test]
    fn a_cell_below_the_eye_is_seen_from_above() {
        let mut b = two_cell_world();
        b.cells[0].maxs[2] = 300.0;
        let vis = WorldVis::build(&b);
        let eye = Vec3::new(-50.0, 0.0, 150.0);
        assert_eq!(vis.cell_for_point(eye), Some(0));
        // the ray to B's soup crosses x = 0 at z ~ 79, above the portal's top edge
        let v = vis.visible(eye, &frustum_at(eye, Vec3::new(0.6, 0.0, -0.8)), None);
        assert!(!v.stats.fallback);
        assert!(v.soups[2], "B's soup is seen over the portal's top edge");
        // at eye height inside the cells the portal alone decides
        let eye = Vec3::new(-50.0, 0.0, 0.0);
        let v = vis.visible(eye, &frustum_at(eye, Vec3::new(0.6, 0.0, -0.8)), None);
        assert!(!v.soups[2], "below B's top the ray misses the portal");
        // a CoD4 world walks its portals alone, over the top or not
        b.portals_only = true;
        let vis = WorldVis::build(&b);
        let eye = Vec3::new(-50.0, 0.0, 150.0);
        let v = vis.visible(eye, &frustum_at(eye, Vec3::new(0.6, 0.0, -0.8)), None);
        assert!(!v.soups[2], "portals only: B stays hidden");
    }

    /// Flying above the map, no soup that is fully on screen in two
    /// consecutive frames may switch between drawn and culled.
    #[test]
    fn high_flight_over_mp_brecourt_does_not_pop() {
        let Some(fs) = crate::testing::game_fs() else {
            return;
        };
        let Some(data) = fs.read("maps/mp/mp_brecourt.bsp") else {
            return;
        };
        let b = crate::bsp::parse(&data).unwrap();
        let vis = WorldVis::build(&b);
        let (lo, hi) = crate::mesh::map_bounds(&b);
        let (lo, hi) = (Vec3::from(lo), Vec3::from(hi));
        let z = hi.z - 300.0;
        let y = (lo.y + hi.y) * 0.5;
        let dir = Vec3::new(
            std::f32::consts::FRAC_1_SQRT_2,
            0.0,
            -std::f32::consts::FRAC_1_SQRT_2,
        );
        let mut prev: Option<(Visible, Frustum)> = None;
        let mut x = lo.x;
        while x <= hi.x {
            let eye = Vec3::new(x, y, z);
            let f = frustum_at(eye, dir);
            let v = vis.visible(eye, &f, None);
            // a sample inside solid falls back to the frustum, which cannot pop
            if v.stats.fallback {
                prev = None;
                x += 48.0;
                continue;
            }
            if let Some((pv, pf)) = &prev {
                for (s, (a, b_)) in pv.soups.iter().zip(&v.soups).enumerate() {
                    if a == b_ {
                        continue;
                    }
                    let (slo, shi) = vis.soup_bounds[s];
                    assert!(
                        !(fully_inside(&f, slo, shi) && fully_inside(pf, slo, shi)),
                        "soup {s} popped at x = {x}: {a} -> {b_}"
                    );
                }
            }
            prev = Some((v, f));
            x += 48.0;
        }
    }

    /// 90 degree vertical FOV, square aspect, looking along `dir` from `eye`.
    pub(crate) fn frustum_at(eye: Vec3, dir: Vec3) -> Frustum {
        let view = glam::camera::rh::view::look_to_mat4(eye, dir, Vec3::Z);
        let proj =
            glam::camera::rh::proj::directx::perspective(90f32.to_radians(), 1.0, 4.0, 10000.0);
        Frustum::from_view_proj(proj * view)
    }

    #[test]
    fn frustum_classifies_points() {
        let f = frustum_at(Vec3::ZERO, Vec3::X);
        assert!(f.contains_point(Vec3::new(100.0, 0.0, 0.0)));
        assert!(
            f.contains_point(Vec3::new(100.0, 90.0, 0.0)),
            "inside the 45 degree half angle"
        );
        assert!(
            !f.contains_point(Vec3::new(100.0, 110.0, 0.0)),
            "outside it"
        );
        assert!(!f.contains_point(Vec3::new(-100.0, 0.0, 0.0)), "behind");
        assert!(
            !f.contains_point(Vec3::new(1.0, 0.0, 0.0)),
            "before the near plane"
        );
        assert!(
            f.intersects_aabb(Vec3::new(50.0, 200.0, -1.0), Vec3::new(400.0, 300.0, 1.0)),
            "box crossing the edge"
        );
        assert!(
            !f.intersects_aabb(Vec3::new(50.0, 200.0, -1.0), Vec3::new(60.0, 300.0, 1.0)),
            "box fully outside"
        );
    }

    #[test]
    fn bevel_planes_add_the_screen_box_without_ejecting_the_portal() {
        // a portal quad hanging above the screen centre: its projection is
        // partly clamped away, yet the edge planes still carry it and the
        // four extra planes must not reject it (they only ever trim regions
        // the near-plane-clipped cone would leak into, which needs the full
        // walk to show up in the counters)
        let eye = Vec3::new(-200.0, 0.0, 0.0);
        let poly = [
            Vec3::new(50.0, -20.0, 140.0),
            Vec3::new(50.0, 20.0, 140.0),
            Vec3::new(50.0, 20.0, 160.0),
            Vec3::new(50.0, -20.0, 160.0),
        ];
        let view = glam::camera::rh::view::look_to_mat4(eye, Vec3::X, Vec3::Z);
        let proj =
            glam::camera::rh::proj::directx::perspective(90f32.to_radians(), 1.0, 4.0, 10000.0);
        let view_proj = proj * view;
        let beveled = Frustum::from_polygon_beveled(eye, &poly, view_proj);
        let plain = Frustum::from_polygon(eye, &poly).planes.len();
        assert!(beveled.planes.len() > plain && beveled.planes.len() <= plain + 4);
        // verts sit on their own edge planes; float noise demands an epsilon
        let inside = |p: Vec3| beveled.planes.iter().all(|(n, d)| n.dot(p) + d > -1e-2);
        for v in &poly {
            assert!(inside(*v), "the portal itself stays in");
        }
        let midpoint = eye.lerp(poly[0], 0.5) + Vec3::Y;
        assert!(inside(midpoint), "the cone interior too");
    }

    /// A fully off-screen quad exercises the clamp/degenerate paths: corner
    /// rays may collapse, and whatever planes survive must not eject it.
    #[test]
    fn bevel_planes_survive_a_fully_offscreen_portal() {
        let eye = Vec3::ZERO;
        let poly = [
            Vec3::new(50.0, -5.0, 100.0),
            Vec3::new(50.0, 5.0, 100.0),
            Vec3::new(50.0, 5.0, 110.0),
            Vec3::new(50.0, -5.0, 110.0),
        ];
        let view = glam::camera::rh::view::look_to_mat4(eye, Vec3::X, Vec3::Z);
        let proj =
            glam::camera::rh::proj::directx::perspective(90f32.to_radians(), 1.0, 4.0, 10000.0);
        let beveled = Frustum::from_polygon_beveled(eye, &poly, proj * view);
        for v in &poly {
            assert!(
                beveled.planes.iter().all(|(n, d)| n.dot(*v) + d > -1e-2),
                "verts survive their own edge planes plus the bevels"
            );
        }
    }

    /// A | B | C corridor with an occluder slab in B covering the far portal:
    /// walking B skips the C-portal (retail 0x4e2860/0x4e2cc0) and never
    /// marks C; without the occluder it does.
    #[test]
    fn an_occluder_hides_the_portal_behind_it() {
        let mut b = occluder_world();
        b.cells[1].occluder_count = 0;
        let bare = WorldVis::build(&b);
        let eye = Vec3::new(-250.0, 0.0, 0.0);
        let f = frustum_at(eye, Vec3::X);
        let v = bare.visible(eye, &f, None);
        assert!(!v.stats.fallback);
        assert!(v.soups[2], "C's soup seen without the occluder");
        assert_eq!(v.stats.portals_occluded, 0);

        b.cells[1].occluder_count = 1;
        let vis = WorldVis::build(&b);
        let v = vis.visible(eye, &f, None);
        assert!(!v.stats.fallback);
        assert_eq!(v.stats.occluders, 1, "B built its volume");
        assert_eq!(v.stats.portals_occluded, 1, "the C-portal was hidden");
        assert!(!v.soups[2], "C is unreachable behind the slab");
        assert!(v.soups[1], "B itself is still drawn");
    }

    /// Every store keeps its corners in a run of lump 11 and hides behind
    /// real slab geometry with portal walks intact.
    fn occluder_world() -> MapWorld {
        let vert = |x: f32, y: f32, z: f32| DrawVert {
            pos: [x, y, z],
            uv: [0.0; 2],
            lm_uv: [0.0; 2],
            normal: [0.0, 0.0, 1.0],
            color: [255; 4],
        };
        // one soup per cell at -150 / 0 / +150
        let tri = |x: f32| {
            [
                vert(x, 0.0, -10.0),
                vert(x + 10.0, 0.0, -10.0),
                vert(x, 0.0, 10.0),
            ]
        };
        let verts: Vec<DrawVert> = [tri(-150.0), tri(0.0), tri(150.0)].concat();
        let soup = |i: u32| TriangleSoup {
            material: 0,
            lightmap: mapworld::NO_LIGHTMAP,
            first_vertex: i * 3,
            vertex_count: 3,
            index_count: 3,
            first_index: 0,
        };
        let p = |normal: [f32; 3], dist: f32| Plane { normal, dist };
        let plane = |normal: [f32; 3], dist: f32| p(normal, dist);
        let plane_idx = |normal: [f32; 3], dist: f32, planes: &mut Vec<Plane>| {
            planes.push(plane(normal, dist));
            planes.len() as u32 - 1
        };
        let mut planes: Vec<Plane> = vec![plane([1.0, 0.0, 0.0], 0.0)]; // placeholder
        planes.clear();
        // portal planes: A/B walls at x=-100, B/C walls at x=+100
        let ab_front = plane_idx([1.0, 0.0, 0.0], -100.0, &mut planes);
        let ab_back = plane_idx([-1.0, 0.0, 0.0], 100.0, &mut planes);
        let bc_front = plane_idx([1.0, 0.0, 0.0], 100.0, &mut planes);
        let bc_back = plane_idx([-1.0, 0.0, 0.0], -100.0, &mut planes);
        // occluder slab x [0,10], y/z [-60,60]: six outward walls
        let s_xp = plane_idx([1.0, 0.0, 0.0], 10.0, &mut planes);
        let s_xn = plane_idx([-1.0, 0.0, 0.0], 0.0, &mut planes);
        let s_yp = plane_idx([0.0, 1.0, 0.0], 60.0, &mut planes);
        let s_yn = plane_idx([0.0, -1.0, 0.0], 60.0, &mut planes);
        let s_zp = plane_idx([0.0, 0.0, 1.0], 60.0, &mut planes);
        let s_zn = plane_idx([0.0, 0.0, -1.0], 60.0, &mut planes);

        // pool: 8 slab corners first, then two portal quads
        let corner = |x: u8, y: u8, z: u8| {
            [
                if x == 1 { 10.0 } else { 0.0 },
                if y == 1 { 60.0 } else { -60.0 },
                if z == 1 { 60.0 } else { -60.0 },
            ]
        };
        let mut portal_verts: Vec<[f32; 3]> = (0u8..8)
            .map(|c| corner(c >> 2 & 1, c >> 1 & 1, c & 1))
            .collect();
        let quad = |x: f32, base: usize, out: &mut Vec<[f32; 3]>| {
            out.extend_from_slice(&[
                [x, -50.0, -50.0],
                [x, 50.0, -50.0],
                [x, 50.0, 50.0],
                [x, -50.0, 50.0],
            ]);
            base as u32
        };
        let _ = quad(-100.0, 8, &mut portal_verts); // slots 8..12
        let bc_base = quad(100.0, 12, &mut portal_verts); // slots 12..16

        // cube corners indexed by bit: x*4 + y*2 + z; edges connect corners
        // differing in one bit and are adjacent to the two walls perpendicular
        // to the OTHER two axes
        // cube corners indexed by bit x*4+y*2+z: 0=(0,-60,-60) .. 7=(10,60,60);
        // relative plane ids 0..6 = [+x,-x,+y,-y,+z,-z]. Each edge carries the
        // two walls it borders and its endpoint slots.
        let edges: Vec<[u8; 4]> = vec![
            // along x, faces +-y / +-z
            [3, 5, 0, 4],
            [3, 4, 1, 5],
            [2, 5, 2, 6],
            [2, 4, 3, 7],
            // along y, faces +-x / +-z
            [1, 5, 0, 2],
            [1, 4, 1, 3],
            [0, 5, 4, 6],
            [0, 4, 5, 7],
            // along z, faces +-x / +-y
            [1, 3, 0, 1],
            [1, 2, 2, 3],
            [0, 3, 4, 5],
            [0, 2, 6, 7],
        ];
        MapWorld {
            materials: vec![Material {
                name: "textures/test/wall".into(),
                surface_flags: 0,
                content_flags: 1,
            }],
            soups: vec![soup(0), soup(1), soup(2)],
            verts,
            indices: vec![0, 1, 2],
            planes,
            portal_verts,
            occluders: vec![mapworld::Occluder {
                first_plane: 0,
                num_planes: 6,
                num_edges: edges.len() as u16,
                first_edge: 0,
                first_vert: 0,
                vert_count: 8,
            }],
            occluder_plane_indices: vec![s_xp, s_xn, s_yp, s_yn, s_zp, s_zn],
            occluder_edges: edges,
            occluder_indices: vec![0],
            aabb_nodes: vec![
                AabbNode {
                    first_soup: 0,
                    soup_count: 1,
                    child_count: 0,
                },
                AabbNode {
                    first_soup: 1,
                    soup_count: 1,
                    child_count: 0,
                },
                AabbNode {
                    first_soup: 2,
                    soup_count: 1,
                    child_count: 0,
                },
            ],
            cells: vec![
                Cell {
                    mins: [-300.0, -300.0, -300.0],
                    maxs: [-100.0, 300.0, 300.0],
                    first_aabb: 0,
                    first_portal: 0,
                    portal_count: 1,
                    first_cull_index: 0,
                    cull_count: 0,
                    first_occluder: 0,
                    occluder_count: 0,
                },
                Cell {
                    mins: [-100.0, -300.0, -300.0],
                    maxs: [100.0, 300.0, 300.0],
                    first_aabb: 1,
                    first_portal: 1,
                    portal_count: 2,
                    first_cull_index: 0,
                    cull_count: 0,
                    first_occluder: 0,
                    occluder_count: 1,
                },
                Cell {
                    mins: [100.0, -300.0, -300.0],
                    maxs: [300.0, 300.0, 300.0],
                    first_aabb: 2,
                    first_portal: 3,
                    portal_count: 1,
                    first_cull_index: 0,
                    cull_count: 0,
                    first_occluder: 0,
                    occluder_count: 0,
                },
            ],
            portals: vec![
                Portal {
                    plane: ab_front,
                    cell: 1,
                    first_vert: 8,
                    vert_count: 4,
                },
                Portal {
                    plane: ab_back,
                    cell: 0,
                    first_vert: 8,
                    vert_count: 4,
                },
                Portal {
                    plane: bc_front,
                    cell: 2,
                    first_vert: bc_base,
                    vert_count: 4,
                },
                Portal {
                    plane: bc_back,
                    cell: 1,
                    first_vert: bc_base,
                    vert_count: 4,
                },
            ],
            // BSP route: front of the A/B wall is B or C, back of it A; the B/C wall
            // then splits B (back) from C (front)
            nodes: vec![
                Node {
                    plane: ab_front,
                    children: [1, -1],
                    mins: [-300; 3],
                    maxs: [300; 3],
                },
                Node {
                    plane: bc_front,
                    children: [-3, -2],
                    mins: [-100; 3],
                    maxs: [300; 3],
                },
            ],
            leafs: vec![
                Leaf {
                    cluster: 0,
                    area: 0,
                    cell: 0,
                },
                Leaf {
                    cluster: 1,
                    area: 0,
                    cell: 1,
                },
                Leaf {
                    cluster: 2,
                    area: 0,
                    cell: 2,
                },
            ],
            ..Default::default()
        }
    }

    /// CoD4's queued walk opens B through the portal once, and the hull of
    /// points two paths gathered spans both.
    #[test]
    fn queued_walk_opens_each_portal_once() {
        let mut b = two_cell_world();
        b.portals_only = true;
        let vis = WorldVis::build(&b);
        let eye = Vec3::new(-50.0, 0.0, 0.0);
        let v = vis.visible(eye, &frustum_at(eye, Vec3::X), None);
        assert!(v.soups[2], "B's soup through the portal");
        assert_eq!(v.stats.cells_visited, 2);
        let v = vis.visible(eye, &frustum_at(eye, -Vec3::X), None);
        assert!(!v.soups[2], "B behind the eye");

        let sq = |x0: f32, y0: f32| {
            [(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)]
                .map(|(x, y)| Vec3::new(x0 + x, y0 + y, 5.0))
        };
        let mut pts = sq(0.0, 0.0).to_vec();
        pts.extend(sq(5.0, 5.0));
        let hull = convex_hull_on_plane(&pts, Vec3::Z);
        assert_eq!(hull.len(), 6, "{hull:?}");
        let area: f32 = (1..hull.len() - 1)
            .map(|i| (hull[i] - hull[0]).cross(hull[i + 1] - hull[0]).z * 0.5)
            .sum();
        assert!((area.abs() - 200.0).abs() < 1e-3, "{area}");
    }

    #[test]
    fn builds_node_bounds_from_soups() {
        let vis = WorldVis::build(&two_cell_world());
        assert_eq!(vis.soup_count(), 3);
        assert_eq!(vis.cell_count(), 2);
        let (lo, hi) = vis.node_bounds[0];
        assert_eq!(
            (lo, hi),
            (Vec3::new(-50.0, 0.0, -10.0), Vec3::new(-40.0, 0.0, 10.0))
        );
        let (lo, hi) = vis.node_bounds[1];
        assert_eq!(
            (lo, hi),
            (Vec3::new(50.0, 0.0, -10.0), Vec3::new(60.0, 0.0, 10.0))
        );
        assert_eq!(vis.tree_end, vec![1, 2]);
    }

    #[test]
    fn nested_tree_unions_bounds_and_skips_a_culled_grandchild() {
        use crate::mapworld::AabbNode;
        let mut bsp = two_cell_world();
        // A 4-node tree in preorder: root (child_count: 2, no soups of its
        // own) over child A (child_count: 1, no soups of its own) and leaf
        // child B; A's single child is a leaf grandchild carrying A's soup.
        // a flat tree can't skip a sibling without testing it; the grandchild shows the skip
        bsp.aabb_nodes = vec![
            AabbNode {
                first_soup: 0,
                soup_count: 0,
                child_count: 2,
            }, // 0: root
            AabbNode {
                first_soup: 0,
                soup_count: 0,
                child_count: 1,
            }, // 1: A
            AabbNode {
                first_soup: 1,
                soup_count: 1,
                child_count: 0,
            }, // 2: A's grandchild
            AabbNode {
                first_soup: 2,
                soup_count: 1,
                child_count: 0,
            }, // 3: B
        ];
        bsp.cells[0].first_aabb = 0;
        bsp.cells[1].first_aabb = 0;
        let vis = WorldVis::build(&bsp);
        assert_eq!(vis.tree_end, vec![4, 3, 3, 4]);
        assert_eq!(
            vis.node_bounds[0],
            union(vis.node_bounds[1], vis.node_bounds[3]),
            "root's bounds are the union of A's and B's"
        );
        let (a_lo, a_hi) = vis.node_bounds[1];
        let (g_lo, g_hi) = vis.node_bounds[2];
        assert!(
            a_lo.cmple(g_lo).all() && a_hi.cmpge(g_hi).all(),
            "A's bounds include its grandchild's"
        );

        // eye between A and B, facing B: A (x -50..-40) is behind the
        // camera and fails outright, B (x 50..60) is ahead and passes.
        let f = frustum_at(Vec3::new(40.0, 0.0, 0.0), Vec3::X);
        let mut v = Visible {
            soups: vec![false; vis.soup_count()],
            cells: vec![false; vis.cell_count()],
            stats: VisStats::default(),
        };
        assert!(vis.mark_tree(0, &f, &mut v), "root is hit through B");
        assert!(!v.soups[1], "A's soup, behind the eye, is not visible");
        assert!(v.soups[2], "B's soup is visible");
        // root(1) + A(2, fails) + B(3) = 3: A's failure jumps straight to
        // its tree_end (3), so its grandchild at index 2 is never tested.
        assert_eq!(v.stats.nodes_tested, 3);
    }

    /// mp_powcamp's bombzone sheds keep their front wall, roof and crates in
    /// `script_brushmodel` `*2` and `*8`, whose soups no cell lists: from
    /// the yard in front of the A shed they are marked, and looking away
    /// they are not (GitHub issue 6: the walls never drew).
    #[test]
    fn mp_powcamp_shed_brush_model_is_visible_from_the_yard() {
        let Some(fs) = crate::testing::game_fs() else {
            return;
        };
        let Some(data) = fs.read("maps/MP/mp_powcamp.bsp") else {
            return;
        };
        let bsp = crate::bsp::parse(&data).unwrap();
        let vis = WorldVis::build(&bsp);
        let m = &bsp.models[2];
        let shed = m.first_soup as usize..(m.first_soup + m.num_soups) as usize;
        assert!(!shed.is_empty(), "*2 carries the shed's surfaces");
        let eye = Vec3::new(416.0, 1000.0, 60.0);
        let v = vis.visible(eye, &frustum_at(eye, Vec3::Y), None);
        assert!(shed.clone().all(|s| v.soups[s]), "the shed faces the eye");
        let v = vis.visible(eye, &frustum_at(eye, -Vec3::Y), None);
        assert!(
            shed.clone().all(|s| !v.soups[s]),
            "the shed is behind the eye"
        );
    }

    /// Spectating from mp_ship's deck looks down over the bulwark at ocean
    /// cells the portal walk seals off; the frustum expansion must surface
    /// the ocean soup anyway (found live 2026-08-26: cells 4/40 from the
    /// deck, flat farbox grey where the water should be).
    #[test]
    fn mp_ship_ocean_visible_from_the_deck() {
        let Some(fs) = crate::testing::game_fs() else {
            return;
        };
        let Some(data) = fs.read("maps/MP/mp_ship.bsp") else {
            return;
        };
        let bsp = crate::bsp::parse(&data).unwrap();
        let vis = WorldVis::build(&bsp);
        let ocean_soup = bsp
            .soups
            .iter()
            .position(|s| {
                bsp.materials[s.material as usize]
                    .name
                    .contains("mp_ship_ocean")
            })
            .expect("mp_ship carries the ocean");
        // the spectator spot from the live comparison, aimed over the rail
        let eye = Vec3::new(1354.0, 845.0, -282.0);
        let frustum = frustum_at(eye, Vec3::new(1.0, 0.12, 0.0).normalize());
        let v = vis.visible(eye, &frustum, None);
        assert!(
            v.soups[ocean_soup],
            "the ocean soup must be visible from the deck"
        );
    }

    #[test]
    fn a_deep_child_chain_stops_at_the_depth_cap() {
        use crate::mapworld::AabbNode;
        let mut bsp = two_cell_world();
        let mut chain = vec![
            AabbNode {
                first_soup: 1,
                soup_count: 0,
                child_count: 1,
            };
            200
        ];
        chain[199].child_count = 0;
        bsp.aabb_nodes = chain;
        bsp.cells[0].first_aabb = 0;
        bsp.cells[1].first_aabb = 0;
        let vis = WorldVis::build(&bsp);
        assert_ne!(vis.tree_end[0], 0, "the root was walked");
        assert_eq!(
            vis.tree_end[MAX_TREE_DEPTH + 1],
            0,
            "the walk stopped at the cap"
        );
    }

    #[test]
    fn finds_the_cell_of_a_point() {
        let vis = WorldVis::build(&two_cell_world());
        assert_eq!(vis.cell_for_point(Vec3::new(-50.0, 0.0, 0.0)), Some(0));
        assert_eq!(vis.cell_for_point(Vec3::new(50.0, 0.0, 0.0)), Some(1));
        let no_tree = WorldVis::build(&MapWorld {
            nodes: vec![],
            ..two_cell_world()
        });
        assert_eq!(no_tree.cell_for_point(Vec3::ZERO), None);

        let solid = WorldVis::build(&MapWorld {
            leafs: vec![
                mapworld::Leaf {
                    cluster: 0,
                    area: 0,
                    cell: -1,
                },
                mapworld::Leaf {
                    cluster: 1,
                    area: 0,
                    cell: 1,
                },
            ],
            ..two_cell_world()
        });
        assert_eq!(
            solid.cell_for_point(Vec3::new(-50.0, 0.0, 0.0)),
            None,
            "a solid leaf (cell -1) has no cell"
        );
    }

    #[test]
    fn frustum_fallback_marks_whatever_the_frustum_contains() {
        let vis = WorldVis::build(&two_cell_world());
        let eye = Vec3::new(-90.0, 0.0, 0.0);
        let f = frustum_at(eye, Vec3::X);
        let v = vis.visible_fallback(&f);
        assert!(v.stats.fallback);
        assert_eq!(
            v.soups,
            vec![false, true, true],
            "both cells' soups are ahead, the cull group is off to the side"
        );
        assert_eq!(v.cells, vec![true, true]);
        let v = vis.visible_fallback(&frustum_at(eye, -Vec3::X));
        assert_eq!(v.soups, vec![false, false, false]);
        let v = vis.visible_fallback(&frustum_at(Vec3::new(-45.0, 0.0, 0.0), Vec3::Y));
        assert_eq!(
            v.soups,
            vec![true, false, false],
            "the cull group is straight up +Y"
        );
    }

    #[test]
    fn props_need_the_frustum_and_in_portal_mode_a_visited_cell() {
        let vis = WorldVis::build(&two_cell_world());
        let f = frustum_at(Vec3::new(-90.0, 0.0, 0.0), Vec3::X);
        let mut v = vis.visible_fallback(&f);
        assert!(vis.prop_visible(
            &v,
            &f,
            Vec3::new(40.0, -5.0, -5.0),
            Vec3::new(60.0, 5.0, 5.0)
        ));
        assert!(
            !vis.prop_visible(
                &v,
                &f,
                Vec3::new(-95.0, 90.0, -5.0),
                Vec3::new(-94.0, 95.0, 5.0)
            ),
            "outside the frustum"
        );
        v.stats.fallback = false;
        v.cells = vec![true, false];
        assert!(
            !vis.prop_visible(
                &v,
                &f,
                Vec3::new(40.0, -5.0, -5.0),
                Vec3::new(60.0, 5.0, 5.0)
            ),
            "in B, which was not visited"
        );
        assert!(vis.prop_visible(
            &v,
            &f,
            Vec3::new(-60.0, -5.0, -5.0),
            Vec3::new(-40.0, 5.0, 5.0)
        ));
    }

    #[test]
    fn mp_pavlov_fallback_draws_less_than_everything() {
        let Some(data) = crate::testing::real_bsp() else {
            return;
        };
        let bsp = crate::bsp::parse(&data).unwrap();
        let vis = WorldVis::build(&bsp);
        let (origin, yaw) = crate::bsp::find_spawn(&bsp.entities).unwrap();
        let eye = Vec3::from(origin) + Vec3::Z * 60.0;
        let dir = Vec3::new(yaw.to_radians().cos(), yaw.to_radians().sin(), 0.0);
        let v = vis.visible_fallback(&frustum_at(eye, dir));
        let n = v.soups.iter().filter(|&&s| s).count();
        assert!(n > 0 && n < bsp.soups.len(), "{n} of {}", bsp.soups.len());
        assert_eq!(v.stats.soups, n);
    }

    #[test]
    fn clips_a_polygon_against_a_plane() {
        let square = [
            Vec3::new(-1.0, -1.0, 0.0),
            Vec3::new(1.0, -1.0, 0.0),
            Vec3::new(1.0, 1.0, 0.0),
            Vec3::new(-1.0, 1.0, 0.0),
        ];
        // keep x >= 0
        let half = clip_polygon(&square, (Vec3::X, 0.0));
        assert_eq!(half.len(), 4);
        assert!(half.iter().all(|p| p.x >= -1e-6));
        assert!(half.iter().any(|p| (p.x - 1.0).abs() < 1e-6));
        // keep x >= 2: nothing survives
        assert!(clip_polygon(&square, (Vec3::X, -2.0)).is_empty());
        // keep x >= -5: untouched
        assert_eq!(clip_polygon(&square, (Vec3::X, 5.0)).len(), 4);
    }

    #[test]
    fn frustum_from_polygon_contains_what_the_portal_shows() {
        let eye = Vec3::new(-100.0, 0.0, 0.0);
        let portal = [
            Vec3::new(0.0, -50.0, -50.0),
            Vec3::new(0.0, 50.0, -50.0),
            Vec3::new(0.0, 50.0, 50.0),
            Vec3::new(0.0, -50.0, 50.0),
        ];
        let f = Frustum::from_polygon(eye, &portal);
        assert_eq!(f.planes.len(), 4);
        assert!(f.contains_point(Vec3::new(100.0, 0.0, 0.0)));
        assert!(
            f.contains_point(Vec3::new(100.0, 90.0, 0.0)),
            "just inside the cone through the edge"
        );
        assert!(!f.contains_point(Vec3::new(100.0, 110.0, 0.0)));
        assert!(!f.contains_point(Vec3::new(100.0, 0.0, 120.0)));
        // reversed winding gives the same volume
        let rev: Vec<Vec3> = portal.iter().rev().copied().collect();
        let g = Frustum::from_polygon(eye, &rev);
        assert!(g.contains_point(Vec3::new(100.0, 90.0, 0.0)));
        assert!(!g.contains_point(Vec3::new(100.0, 110.0, 0.0)));
    }

    #[test]
    fn portal_walk_sees_the_neighbour_only_through_the_portal() {
        let vis = WorldVis::build(&two_cell_world());
        let eye = Vec3::new(-90.0, 0.0, 0.0);
        let v = vis.visible(eye, &frustum_at(eye, Vec3::X), None);
        assert!(!v.stats.fallback);
        assert_eq!(v.cells, vec![true, true]);
        assert_eq!(v.soups, vec![false, true, true]);
        // facing away: B is not visited, its soup stays hidden
        let v = vis.visible(eye, &frustum_at(eye, -Vec3::X), None);
        assert_eq!(v.cells, vec![true, false]);
        assert_eq!(v.soups, vec![false, false, false]);
        // looking +X but from high up in A so the portal square is below the frustum
        let eye = Vec3::new(-10.0, 0.0, 90.0);
        let v = vis.visible(eye, &frustum_at(eye, Vec3::X), None);
        assert_eq!(v.cells, vec![true, false], "portal outside the frustum");
        // eye in no cell: fallback. The fixture's single-plane tree puts every
        // point in a cell, so this needs the solid-leaf (cell -1) variant.
        let solid = WorldVis::build(&MapWorld {
            leafs: vec![
                mapworld::Leaf {
                    cluster: 0,
                    area: 0,
                    cell: -1,
                },
                mapworld::Leaf {
                    cluster: 1,
                    area: 0,
                    cell: 1,
                },
            ],
            ..two_cell_world()
        });
        let eye = Vec3::new(-90.0, 0.0, 0.0);
        let v = solid.visible(eye, &frustum_at(eye, Vec3::X), None);
        assert!(v.stats.fallback);
    }

    #[test]
    fn a_portal_closer_than_the_near_plane_still_opens_its_cell() {
        let vis = WorldVis::build(&two_cell_world());
        let eye = Vec3::new(-0.5, 0.0, 0.0);
        let v = vis.visible(eye, &frustum_at(eye, Vec3::X), None);
        assert_eq!(
            v.cells,
            vec![true, true],
            "the 4-unit near plane must not clip the portal away"
        );
    }

    #[test]
    fn mp_pavlov_portal_walk_is_a_subset_of_the_fallback() {
        let Some(data) = crate::testing::real_bsp() else {
            return;
        };
        let bsp = crate::bsp::parse(&data).unwrap();
        let vis = WorldVis::build(&bsp);
        let mut spawns = 0;
        let mut tighter = 0;
        for e in crate::bsp::entity_blocks(&bsp.entities) {
            let Some(origin) = e.get("origin").and_then(|s| crate::bsp::parse_vec3(s)) else {
                continue;
            };
            if !e.get("classname").is_some_and(|c| c.starts_with("mp_")) {
                continue;
            }
            let yaw: f32 = e
                .get("angle")
                .and_then(|a| a.trim().parse().ok())
                .unwrap_or(0.0);
            let eye = Vec3::from(origin) + Vec3::Z * 60.0;
            let dir = Vec3::new(yaw.to_radians().cos(), yaw.to_radians().sin(), 0.0);
            let f = frustum_at(eye, dir);
            let walk = vis.visible(eye, &f, None);
            if walk.stats.fallback {
                continue;
            }
            let all = vis.visible_fallback(&f);
            for (w, a) in walk.soups.iter().zip(&all.soups) {
                assert!(
                    !w || *a,
                    "portal walk drew a soup the frustum alone would not"
                );
            }
            spawns += 1;
            if walk.stats.soups < all.stats.soups {
                tighter += 1;
            }
            if spawns == 5 {
                break;
            }
        }
        assert!(spawns >= 3, "found {spawns} spawns inside cells");
        assert!(
            tighter >= 1,
            "portals never culled anything the frustum kept"
        );
    }
}
