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
    /// No camera cell: every tree and cull group was frustum-tested.
    pub fallback: bool,
}

pub struct Visible {
    pub soups: Vec<bool>,
    pub cells: Vec<bool>,
    pub stats: VisStats,
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
}

/// State threaded through the portal recursion: the eye and the unnarrowed
/// frustum are fixed for the walk, the marks and the portal stack accumulate.
struct Walk<'a> {
    eye: Vec3,
    camera: &'a Frustum,
    vis: Visible,
    on_stack: Vec<bool>,
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
    pub fn visible(&self, eye: Vec3, frustum: &Frustum) -> Visible {
        let Some(cell) = self.cell_for_point(eye) else {
            return self.visible_fallback(frustum);
        };
        let mut w = Walk {
            eye,
            camera: frustum,
            vis: self.empty_visible(),
            on_stack: vec![false; self.portals.len()],
        };
        self.walk(cell, frustum, &mut w, 0);
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
            if poly.len() < 3 {
                continue;
            }
            let mut child;
            // within a unit of the plane the polygon degenerates: keep the parent
            let next = if side >= -1.0 {
                frustum
            } else {
                child = Frustum::from_polygon(w.eye, &poly);
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
            },
            Cell {
                mins: [0.0, -100.0, -100.0],
                maxs: [100.0, 100.0, 100.0],
                first_aabb: 1,
                first_portal: 1,
                portal_count: 1,
                first_cull_index: 1,
                cull_count: 0,
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
        let v = vis.visible(eye, &frustum_at(eye, Vec3::X));
        assert!(!v.stats.fallback);
        assert_eq!(v.cells, vec![true, true]);
        assert_eq!(v.soups, vec![false, true, true]);
        // facing away: B is not visited, its soup stays hidden
        let v = vis.visible(eye, &frustum_at(eye, -Vec3::X));
        assert_eq!(v.cells, vec![true, false]);
        assert_eq!(v.soups, vec![false, false, false]);
        // looking +X but from high up in A so the portal square is below the frustum
        let eye = Vec3::new(-10.0, 0.0, 90.0);
        let v = vis.visible(eye, &frustum_at(eye, Vec3::X));
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
        let v = solid.visible(eye, &frustum_at(eye, Vec3::X));
        assert!(v.stats.fallback);
    }

    #[test]
    fn a_portal_closer_than_the_near_plane_still_opens_its_cell() {
        let vis = WorldVis::build(&two_cell_world());
        let eye = Vec3::new(-0.5, 0.0, 0.0);
        let v = vis.visible(eye, &frustum_at(eye, Vec3::X));
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
            let walk = vis.visible(eye, &f);
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
