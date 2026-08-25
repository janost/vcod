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
    // Unread until Task 4's portal walk.
    #[allow(dead_code)]
    cull_indices: Vec<u32>,
    #[allow(dead_code)]
    portals: Vec<bsp::Portal>,
    #[allow(dead_code)]
    portal_verts: Vec<Vec3>,
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

    /// Stub: Task 4 adds the portal walk from the camera cell. Until then,
    /// falls back to frustum-only visibility regardless of `eye`.
    pub fn visible(&self, eye: Vec3, frustum: &Frustum) -> Visible {
        let _ = eye;
        self.visible_fallback(frustum)
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

/// Bottom-up bounds and preorder ends; returns the index after the subtree.
/// Guards against a hostile forest two ways: an out-of-range or repeated
/// (`tree_end[i] != 0`) child is skipped, and `depth` caps recursion at
/// `nodes.len()` so a `child_count: 1` chain cannot overflow the stack.
fn walk_bounds(
    i: usize,
    depth: usize,
    nodes: &[bsp::AabbNode],
    soup_bounds: &[(Vec3, Vec3)],
    node_bounds: &mut [(Vec3, Vec3)],
    tree_end: &mut [u32],
) -> usize {
    if depth > nodes.len() {
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
    // soup 0: A, soup 1: B, soup 2: A's cull group, up on the +Y wall
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
    fn nested_tree_unions_bounds_and_tests_every_sibling_once() {
        use crate::bsp::AabbNode;
        let mut bsp = two_cell_world();
        // A 3-node tree: root (no soups of its own) over two leaf children,
        // where the fixture's aabb_nodes previously only ever had child_count
        // 0, never exercising walk_bounds's child loop or mark_tree's skip.
        bsp.aabb_nodes = vec![
            AabbNode {
                first_soup: 0,
                soup_count: 0,
                child_count: 2,
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
        ];
        bsp.cells[0].first_aabb = 0;
        bsp.cells[1].first_aabb = 0;
        let vis = WorldVis::build(&bsp);
        assert_eq!(vis.tree_end, vec![3, 2, 3]);
        let (a_lo, a_hi) = vis.node_bounds[1];
        let (b_lo, b_hi) = vis.node_bounds[2];
        assert_eq!(
            vis.node_bounds[0],
            (a_lo.min(b_lo), a_hi.max(b_hi)),
            "root's bounds are the union of its children's"
        );

        // near=4, far=100 from x=-90 reaches child A (closest point x=-50,
        // distance 40) but not child B (closest point x=50, distance 140);
        // the root's own union bounds still touch A, so it is hit too.
        let eye = Vec3::new(-90.0, 0.0, 0.0);
        let view = glam::camera::rh::view::look_to_mat4(eye, Vec3::X, Vec3::Z);
        let proj =
            glam::camera::rh::proj::directx::perspective(90f32.to_radians(), 1.0, 4.0, 100.0);
        let f = Frustum::from_view_proj(proj * view);

        let mut v = Visible {
            soups: vec![false; vis.soup_count()],
            cells: vec![false; vis.cell_count()],
            stats: VisStats::default(),
        };
        assert!(vis.mark_tree(0, &f, &mut v), "root itself is hit");
        assert!(v.soups[1], "child A is inside the shortened far plane");
        assert!(!v.soups[2], "child B is past it");
        // Root, A and B are each visited once: the preorder range is
        // contiguous and a failing leaf's own tree_end is just itself + 1,
        // so it can't skip past its sibling. The skip only pays off for a
        // failing internal node with descendants beyond it (mp_pavlov's
        // real trees have those; this fixture keeps the two-node case).
        assert_eq!(v.stats.nodes_tested, 3);
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
        assert_eq!(
            vis.visible(eye, &f).soups,
            v.soups,
            "visible() is still the fallback stub"
        );
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
}
