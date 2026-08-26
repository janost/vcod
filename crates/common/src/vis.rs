//! Which soups a camera sees, decided the way the retail client does it:
//! docs/research/bsp-ibsp59-format.md, "How the retail client draws the world".

use crate::bsp::{self, Bsp};
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
    aabb_nodes: Vec<bsp::AabbNode>,
    cull_groups: Vec<bsp::CullGroup>,
    cells: Vec<bsp::Cell>,
    nodes: Vec<bsp::Node>,
    leafs: Vec<bsp::Leaf>,
    /// `(n, dist)`, front where `n·p - dist > 0`.
    planes: Vec<(Vec3, f32)>,
    cull_indices: Vec<u32>,
    portals: Vec<bsp::Portal>,
    portal_verts: Vec<Vec3>,
    occluders: Vec<Occ>,
    occluder_indices: Vec<u16>,
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
}

const EMPTY: (Vec3, Vec3) = (Vec3::INFINITY, Vec3::NEG_INFINITY);

fn union(a: (Vec3, Vec3), b: (Vec3, Vec3)) -> (Vec3, Vec3) {
    (a.0.min(b.0), a.1.max(b.1))
}

fn overlaps(a: (Vec3, Vec3), lo: Vec3, hi: Vec3) -> bool {
    lo.cmple(a.1).all() && hi.cmpge(a.0).all()
}

impl WorldVis {
    pub fn build(bsp: &Bsp) -> Self {
        let soup_bounds: Vec<(Vec3, Vec3)> = bsp
            .soups
            .iter()
            .map(|s| {
                let fv = s.first_vertex as usize;
                bsp.verts[fv..fv + s.vertex_count as usize]
                    .iter()
                    .fold(EMPTY, |b, v| {
                        union(b, (Vec3::from(v.pos), Vec3::from(v.pos)))
                    })
            })
            .collect();
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
        WorldVis {
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
        };
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
        let volumes = self.occluder_volumes(cell, w.eye);
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

    /// The cell's occluder volumes against `eye` (CoDMP 0x4e2860): each
    /// front-facing box plane stays, and every silhouette edge whose two
    /// planes straddle the eye contributes one plane through the eye. No
    /// volume (no occluders, or the call had none) is an empty slice.
    fn occluder_volumes(&self, cell: usize, eye: Vec3) -> Vec<Vec<(Vec3, f32)>> {
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
    fn portal_in_occluders(&self, p: &bsp::Portal, volumes: &[Vec<(Vec3, f32)>]) -> bool {
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
            for s in node.first_soup as usize..(node.first_soup + node.soup_count) as usize {
                let (lo, hi) = self.soup_bounds[s];
                if frustum.intersects_aabb(lo, hi) {
                    vis.soups[s] = true;
                }
            }
            i += 1;
        }
        root_hit
    }

    /// Group bounds only; retail marks every surface of a hit group.
    fn mark_group(&self, g: &bsp::CullGroup, frustum: &Frustum, vis: &mut Visible) {
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
    nodes: &[bsp::AabbNode],
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
pub fn two_cell_world() -> Bsp {
    use crate::bsp::{
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
        lightmap: bsp::NO_LIGHTMAP,
        first_vertex: i * 3,
        vertex_count: 3,
        index_count: 3,
        first_index: 0,
    };
    Bsp {
        materials: vec![Material {
            name: "textures/test/wall".into(),
            surface_flags: 0,
            content_flags: 1,
        }],
        lightmaps: vec![],
        soups: vec![soup(2), soup(0), soup(1)],
        verts,
        indices: vec![0, 1, 2],
        entities: String::new(),
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
        brush_sides: vec![],
        brushes: vec![],
        models: vec![],
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
        occluders: vec![],
        occluder_plane_indices: vec![],
        occluder_edges: vec![],
        occluder_indices: vec![],
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
                cell: 0,
            },
            Leaf {
                cluster: 1,
                cell: 1,
            },
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[allow(unused_imports)]
    use crate::bsp::{AabbNode, Cell, DrawVert, Leaf, Material, Node, Plane, Portal, TriangleSoup};

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
        let b = bsp::parse(&data).unwrap();
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
    fn occluder_world() -> Bsp {
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
            lightmap: bsp::NO_LIGHTMAP,
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
        Bsp {
            materials: vec![Material {
                name: "textures/test/wall".into(),
                surface_flags: 0,
                content_flags: 1,
            }],
            lightmaps: vec![],
            soups: vec![soup(0), soup(1), soup(2)],
            verts,
            indices: vec![0, 1, 2],
            entities: String::new(),
            planes,
            brush_sides: vec![],
            brushes: vec![],
            models: vec![],
            cull_groups: vec![],
            cull_indices: vec![],
            portal_verts,
            occluders: vec![bsp::Occluder {
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
                    cell: 0,
                },
                Leaf {
                    cluster: 1,
                    cell: 1,
                },
                Leaf {
                    cluster: 2,
                    cell: 2,
                },
            ],
        }
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
        use crate::bsp::AabbNode;
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
        let bsp = bsp::parse(&data).unwrap();
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
        use crate::bsp::AabbNode;
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
        let no_tree = WorldVis::build(&Bsp {
            nodes: vec![],
            ..two_cell_world()
        });
        assert_eq!(no_tree.cell_for_point(Vec3::ZERO), None);

        let solid = WorldVis::build(&Bsp {
            leafs: vec![
                bsp::Leaf {
                    cluster: 0,
                    cell: -1,
                },
                bsp::Leaf {
                    cluster: 1,
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
        let bsp = bsp::parse(&data).unwrap();
        let vis = WorldVis::build(&bsp);
        let (origin, yaw) = bsp::find_spawn(&bsp.entities).unwrap();
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
        let solid = WorldVis::build(&Bsp {
            leafs: vec![
                bsp::Leaf {
                    cluster: 0,
                    cell: -1,
                },
                bsp::Leaf {
                    cluster: 1,
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
        let bsp = bsp::parse(&data).unwrap();
        let vis = WorldVis::build(&bsp);
        let mut spawns = 0;
        let mut tighter = 0;
        for e in bsp::entity_blocks(&bsp.entities) {
            let Some(origin) = e.get("origin").and_then(|s| bsp::parse_vec3(s)) else {
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
