//! A one-leaf clip map: a box brush and a floor triangle beside it.

use super::*;

const PLAYER_MINS: Vec3 = Vec3::new(-15.0, -15.0, 0.0);
const PLAYER_MAXS: Vec3 = Vec3::new(15.0, 15.0, 70.0);

/// Brush x 0..64, y -32..32, z 0..64; a floor triangle at z 0 over x
/// -200..-100, y -100..100 with `s` along x and `t` along y.
fn map() -> ClipMap {
    let brush = CBrush {
        mins: Vec3::new(0.0, -32.0, 0.0),
        maxs: Vec3::new(64.0, 32.0, 64.0),
        contents: CONTENTS_SOLID,
        sides: Vec::new(),
        axial_material: [[0; 3]; 2],
    };
    ClipMap {
        planes: vec![CPlane {
            normal: Vec3::Z,
            dist: -1000.0,
            kind: 2,
            signbits: 0,
        }],
        nodes: vec![CNode {
            plane: 0,
            children: [-1, -1],
        }],
        leafs: vec![CLeaf {
            first_aabb: 0,
            aabb_count: 1,
            brush_contents: CONTENTS_SOLID,
            terrain_contents: CONTENTS_SOLID,
            mins: brush.mins - Vec3::splat(0.125),
            maxs: brush.maxs + Vec3::splat(0.125),
            brush_node: 1,
        }],
        brush_nodes: vec![
            BrushNode::Leaf {
                first: 0,
                count: 0,
                contents: 0,
            },
            BrushNode::Leaf {
                first: 0,
                count: 1,
                contents: CONTENTS_SOLID,
            },
        ],
        leaf_brush_list: vec![0, 0],
        brushes: vec![brush],
        materials: vec![(0x50_0000, CONTENTS_SOLID)],
        verts: Vec::new(),
        edges: Vec::new(),
        tris: vec![CollTri {
            plane: [0.0, 0.0, 1.0, 0.0],
            svec: [0.01, 0.0, 0.0, -2.0],
            tvec: [0.0, 0.005, 0.0, -0.5],
            verts: [-1; 3],
            edges: [-1; 3],
        }],
        borders: Vec::new(),
        partitions: vec![CollPartition {
            unused: 0,
            tri_count: 1,
            border_count: 0,
            first_tri: 0,
            first_border: 0,
        }],
        aabbs: vec![CollAabbTree {
            origin: Vec3::new(-150.0, 0.0, 0.0),
            half_size: Vec3::new(50.0, 100.0, 1.0),
            material: 0,
            child_count: 0,
            child_or_partition: 0,
        }],
        indexed: None,
        reach: Default::default(),
    }
}

fn down(x: f32) -> (Vec3, Vec3) {
    (Vec3::new(x, 0.0, 100.0), Vec3::new(x, 0.0, -100.0))
}

#[test]
fn a_point_stops_short_of_the_brush_top() {
    let (s, e) = down(32.0);
    let t = map().trace(s, e, Vec3::ZERO, Vec3::ZERO, MASK_PLAYERSOLID);
    assert_eq!(t.fraction, (100.0 - 64.0 - 0.125) / 200.0);
    assert_eq!(t.normal, Vec3::Z);
    assert_eq!(t.contents, CONTENTS_SOLID);
}

#[test]
fn a_player_lands_on_the_brush_by_its_feet() {
    let (s, e) = down(32.0);
    let t = map().trace(s, e, PLAYER_MINS, PLAYER_MAXS, MASK_PLAYERSOLID);
    assert!(
        (t.fraction * 200.0 - (100.0 - 64.125)).abs() < 1e-4,
        "{t:?}"
    );
    assert_eq!(t.normal, Vec3::Z);
}

#[test]
fn a_triangle_is_one_sided() {
    let m = map();
    let (s, e) = down(-170.0);
    let t = m.trace(s, e, Vec3::ZERO, Vec3::ZERO, MASK_SHOT);
    assert_eq!(t.fraction, (100.0 - 0.125) / 200.0);
    assert_eq!(t.surface_flags, 0x50_0000);
    assert_eq!(
        m.trace(e, s, Vec3::ZERO, Vec3::ZERO, MASK_SHOT).fraction,
        1.0
    );
}

#[test]
fn a_capsule_meets_a_triangle_with_its_lower_sphere() {
    let (s, e) = down(-170.0);
    let t = map().trace(s, e, PLAYER_MINS, PLAYER_MAXS, MASK_PLAYERSOLID);
    // The lower sphere's centre is 15 over the feet; it stops 15.125 off.
    assert!(
        (t.fraction - (115.0 - 15.125) / 200.0).abs() < 1e-6,
        "{t:?}"
    );
    assert!(!t.startsolid);
}

#[test]
fn a_box_inside_the_brush_is_all_solid() {
    let m = map();
    let p = Vec3::new(32.0, 0.0, 10.0);
    let t = m.trace(p, p, PLAYER_MINS, PLAYER_MAXS, MASK_PLAYERSOLID);
    assert!(t.allsolid && t.startsolid && t.fraction == 0.0);
    let clear = Vec3::new(32.0, 0.0, 64.5);
    assert!(
        !m.trace(clear, clear, PLAYER_MINS, PLAYER_MAXS, MASK_PLAYERSOLID)
            .startsolid
    );
    assert_eq!(m.point_contents(p), CONTENTS_SOLID);
    assert_eq!(m.point_contents(clear), 0);
}

#[test]
fn a_mask_without_solid_passes_everything() {
    let (s, e) = down(32.0);
    let t = map().trace(s, e, PLAYER_MINS, PLAYER_MAXS, CONTENTS_WATER);
    assert_eq!(t, ClipTrace::clear());
}

/// One test map per shape `check_trees` refuses: the kernels would spin,
/// recurse without end, or index past an array on each.
#[test]
fn check_trees_refuses_what_the_kernels_cannot_walk() {
    assert!(map().check_trees().is_ok());
    let refused = |edit: &dyn Fn(&mut ClipMap)| {
        let mut m = map();
        edit(&mut m);
        m.check_trees().is_err()
    };
    let node = |children| CNode { plane: 0, children };
    assert!(refused(&|m| m.nodes.clear()));
    assert!(refused(&|m| m.nodes[0].children = [0, -1]));
    assert!(refused(&|m| m.nodes = vec![node([1, 1]), node([-1, -1])]));
    let aabb = |child_count, child_or_partition| CollAabbTree {
        child_count,
        child_or_partition,
        ..map().aabbs[0]
    };
    assert!(refused(&|m| m.aabbs[0] = aabb(1, 0)));
    assert!(refused(
        &|m| m.aabbs = vec![aabb(2, 1), aabb(1, 2), aabb(0, 0)]
    ));
    let split = |children, straddlers| BrushNode::Split {
        axis: 0,
        dist: 0.0,
        clearance: 0.0,
        straddlers,
        children,
        contents: CONTENTS_SOLID,
    };
    assert!(refused(&|m| m.brush_nodes[1] = split([0, 1], false)));
    let leaf = map().brush_nodes[1];
    assert!(refused(&|m| {
        m.brush_nodes = vec![leaf, split([1, 2], true), leaf, leaf];
    }));
    assert!(!refused(&|m| {
        m.brush_nodes = vec![leaf, split([2, 3], true), leaf, leaf, leaf];
    }));
}

/// A map with every record kind the loaders fill, in both triangle forms:
/// a BSP split at x 0 and one on `x - y = 0` under it; west of 0 a creased
/// two-triangle terrain patch (x -256..-64, the diagonal from v0 to v2 a
/// ridge) under a two-child AABB tree, with edges, vertices and a border
/// along its east edge; east of 0 an axial box A (x 32..96), a wedge B
/// (x 128..256) cut by a slope rising 0.5 a unit along x and by the plane
/// `x + y = 256`, and a pillar C (x 104..120) across a k-d split at 112.
/// `kd` puts the brushes under that split, else in one flat leaf.
mod rich {
    use super::*;

    pub const TERRAIN: u32 = 0x50_0000;
    pub const WEDGE: u32 = 0x60_0000;
    pub const AXIAL: u32 = 0x70_0000;

    pub fn verts() -> [Vec3; 4] {
        [
            Vec3::new(-256.0, -128.0, 0.0),
            Vec3::new(-64.0, -128.0, 0.0),
            Vec3::new(-64.0, 128.0, 32.0),
            Vec3::new(-256.0, 128.0, 0.0),
        ]
    }

    pub const TRIS: [[usize; 3]; 2] = [[0, 1, 2], [0, 2, 3]];

    pub fn slope() -> CPlane {
        let n = Vec3::new(-0.5, 0.0, 1.0).normalize();
        plane(n, n.dot(Vec3::new(128.0, 0.0, 0.0)))
    }

    pub fn corner() -> CPlane {
        let n = Vec3::new(1.0, 1.0, 0.0).normalize();
        plane(n, n.dot(Vec3::new(256.0, 0.0, 0.0)))
    }

    fn plane(normal: Vec3, dist: f32) -> CPlane {
        let kind = (0..3).find(|&k| normal[k] == 1.0).unwrap_or(3) as u8;
        let signbits = (0..3).fold(0u8, |s, k| s | (u8::from(normal[k] < 0.0) << k));
        CPlane {
            normal,
            dist,
            kind,
            signbits,
        }
    }

    /// Lump 31's record for `a, b, c`: `s` is `b`'s barycentric weight and
    /// `t` is `c`'s, both flat along the normal.
    fn tri(a: Vec3, b: Vec3, c: Vec3, verts: [i32; 3], edges: [i32; 3]) -> CollTri {
        let (e1, e2) = (b - a, c - a);
        let n = e1.cross(e2).normalize();
        let gs = e2.cross(n) / e1.dot(e2.cross(n));
        let gt = n.cross(e1) / e2.dot(n.cross(e1));
        CollTri {
            plane: [n.x, n.y, n.z, n.dot(a)],
            svec: [gs.x, gs.y, gs.z, gs.dot(a)],
            tvec: [gt.x, gt.y, gt.z, gt.dot(a)],
            verts,
            edges,
        }
    }

    fn edge(a: Vec3, b: Vec3) -> CEdge {
        let len = (b - a).length();
        let dir = (b - a) / len;
        let side = dir.any_orthonormal_vector();
        CEdge {
            origin: a,
            axis: [side, dir.cross(side)],
            dir_scaled: dir / len,
        }
    }

    fn brush(mins: Vec3, maxs: Vec3, sides: Vec<(u32, u16)>) -> CBrush {
        CBrush {
            mins,
            maxs,
            contents: CONTENTS_SOLID,
            sides,
            axial_material: [[2; 3]; 2],
        }
    }

    pub fn map(indexed: bool, kd: bool) -> ClipMap {
        let v = verts();
        // Brushes: A, the wedge B, the pillar C.
        let brushes = vec![
            brush(
                Vec3::new(32.0, -32.0, 0.0),
                Vec3::new(96.0, 32.0, 64.0),
                vec![],
            ),
            brush(
                Vec3::new(128.0, -64.0, 0.0),
                Vec3::new(256.0, 64.0, 64.0),
                vec![(2, 1), (3, 1)],
            ),
            brush(
                Vec3::new(104.0, -16.0, 0.0),
                Vec3::new(120.0, 16.0, 200.0),
                vec![],
            ),
        ];
        let placeholder = BrushNode::Leaf {
            first: 0,
            count: 0,
            contents: 0,
        };
        let leaf = |first, count| BrushNode::Leaf {
            first,
            count,
            contents: CONTENTS_SOLID,
        };
        let (brush_nodes, leaf_brush_list) = if kd {
            (
                vec![
                    placeholder,
                    BrushNode::Split {
                        axis: 0,
                        dist: 112.0,
                        clearance: 0.0,
                        straddlers: true,
                        children: [2, 3],
                        contents: CONTENTS_SOLID,
                    },
                    leaf(0, 1),
                    leaf(1, 1),
                    leaf(2, 1),
                ],
                vec![2, 1, 0, 0],
            )
        } else {
            (vec![placeholder, leaf(0, 3)], vec![0, 1, 2, 0])
        };
        let (lo, hi) = brushes.iter().fold(
            (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)),
            |(lo, hi), b| (lo.min(b.mins), hi.max(b.maxs)),
        );
        let east = CLeaf {
            first_aabb: 0,
            aabb_count: 0,
            brush_contents: CONTENTS_SOLID,
            terrain_contents: 0,
            mins: lo - Vec3::splat(0.125),
            maxs: hi + Vec3::splat(0.125),
            brush_node: 1,
        };
        let west = CLeaf {
            first_aabb: 0,
            aabb_count: 1,
            brush_contents: 0,
            terrain_contents: CONTENTS_SOLID,
            mins: Vec3::ZERO,
            maxs: Vec3::ZERO,
            brush_node: 0,
        };
        let patch = |child_count, child_or_partition| CollAabbTree {
            origin: Vec3::new(-160.0, 0.0, 16.0),
            half_size: Vec3::new(96.0, 128.0, 16.0),
            material: 0,
            child_count,
            child_or_partition,
        };
        // Edges: the ridge v0-v2, then v0-v1, v1-v2, v2-v3, v3-v0.
        let edges = [(0, 2), (0, 1), (1, 2), (2, 3), (3, 0)].map(|(a, b)| edge(v[a], v[b]));
        let mut m = ClipMap {
            planes: vec![
                plane(Vec3::X, 0.0),
                plane(Vec3::new(1.0, -1.0, 0.0).normalize(), 0.0),
                slope(),
                corner(),
            ],
            nodes: vec![
                CNode {
                    plane: 0,
                    children: [1, -1],
                },
                CNode {
                    plane: 1,
                    children: [-2, -3],
                },
            ],
            leafs: vec![west, east, east],
            brush_nodes,
            leaf_brush_list,
            brushes,
            materials: vec![
                (TERRAIN, CONTENTS_SOLID),
                (WEDGE, CONTENTS_SOLID),
                (AXIAL, CONTENTS_SOLID),
            ],
            verts: v.to_vec(),
            edges: edges.to_vec(),
            tris: vec![
                tri(v[0], v[1], v[2], [0, 1, 2], [2, 0, 1]),
                tri(v[0], v[2], v[3], [0, 2, 3], [3, 4, 0]),
            ],
            // The patch's east edge, x -64, outward +x: along runs from v2
            // (y 128, z 32) to v1 (y -128, z 0).
            borders: vec![CollBorder {
                dist_eq: [1.0, 0.0, -64.0],
                z_base: 32.0,
                z_slope: -0.125,
                start: -128.0,
                length: 256.0,
            }],
            partitions: vec![
                CollPartition {
                    unused: 0,
                    tri_count: 1,
                    border_count: 1,
                    first_tri: 0,
                    first_border: 0,
                },
                CollPartition {
                    unused: 0,
                    tri_count: 1,
                    border_count: 0,
                    first_tri: 1,
                    first_border: 0,
                },
            ],
            aabbs: vec![patch(2, 1), patch(0, 0), patch(0, 1)],
            indexed: None,
            reach: Default::default(),
        };
        if indexed {
            m.tris.clear();
            m.indexed = Some(IndexedMesh {
                verts: v.to_vec(),
                // Wound the other way: CoD4's front is `e2 x e1`.
                tris: TRIS
                    .map(|[a, b, c]| [a as u16, c as u16, b as u16])
                    .to_vec(),
                // Every edge walkable.
                walkable: vec![0x3f],
            });
        }
        m
    }

    /// `m` with every node's reach unbounded: the walk without the prune.
    pub fn unpruned(m: &ClipMap) -> ClipMap {
        let m = m.clone();
        let all = [Vec3::splat(f32::NEG_INFINITY), Vec3::splat(f32::INFINITY)];
        m.reach.set(vec![all; m.nodes.len()]).unwrap();
        m
    }
}

const POINT: (Vec3, Vec3) = (Vec3::ZERO, Vec3::ZERO);
const PLAYER: (Vec3, Vec3) = (PLAYER_MINS, PLAYER_MAXS);

fn near(a: f32, b: f32) -> bool {
    (a - b).abs() < 1e-4
}

fn near3(a: Vec3, b: Vec3) -> bool {
    (a - b).abs().max_element() < 1e-4
}

#[test]
fn the_rich_map_is_a_tree_in_both_forms() {
    for indexed in [false, true] {
        for kd in [false, true] {
            rich::map(indexed, kd).check_trees().unwrap();
        }
    }
}

/// A point and a capsule onto the wedge's slope, and a capsule into its
/// cut corner: the non-axial sides lead the contact, the capsule's sides
/// pushed out by its radius and, on the slope, by the end sphere's height.
#[test]
fn a_wedge_meets_a_sweep_on_its_non_axial_sides() {
    let slope = rich::slope();
    let n = slope.normal;
    for indexed in [false, true] {
        for kd in [false, true] {
            let m = rich::map(indexed, kd);
            let (s, e) = (Vec3::new(192.0, 0.0, 200.0), Vec3::new(192.0, 0.0, -100.0));
            let t = m.trace(s, e, POINT.0, POINT.1, MASK_PLAYERSOLID);
            let (d1, d2) = (n.dot(s) - slope.dist, n.dot(e) - slope.dist);
            assert!(near(t.fraction, (d1 - 0.125) / (d1 - d2)), "{t:?}");
            assert!(
                near3(t.normal, n) && t.surface_flags == rich::WEDGE,
                "{t:?}"
            );

            // The player's centre is 35 up, its sphere 20 off it, radius 15.
            let t = m.trace(s, e, PLAYER.0, PLAYER.1, MASK_PLAYERSOLID);
            let lift = slope.dist + 15.0 + n.z * 20.0;
            let c = Vec3::new(0.0, 0.0, 35.0);
            let (d1, d2) = (n.dot(s + c) - lift, n.dot(e + c) - lift);
            assert!(near(t.fraction, (d1 - 0.125) / (d1 - d2)), "{t:?}");
            assert!(near3(t.normal, n), "{t:?}");

            let corner = rich::corner();
            let (s, e) = (Vec3::new(330.0, 30.0, 10.0), Vec3::new(230.0, 30.0, 10.0));
            let t = m.trace(s, e, PLAYER.0, PLAYER.1, MASK_PLAYERSOLID);
            let at = |p: Vec3| corner.normal.dot(p) - corner.dist - 15.0;
            assert!(near(t.fraction, (at(s) - 0.125) / (at(s) - at(e))), "{t:?}");
            assert!(near3(t.normal, corner.normal), "{t:?}");
        }
    }
}

/// The k-d tree's split and straddler: a sweep along x meets A, then the
/// pillar across the split, then the wedge, each from its near side.
#[test]
fn the_brush_tree_walks_both_sides_and_the_straddler() {
    for indexed in [false, true] {
        let m = rich::map(indexed, true);
        let y = Vec3::new(0.0, 0.0, 20.0);
        let along = |x0: f32, x1: f32| {
            let (s, e) = (Vec3::new(x0, 0.0, 0.0) + y, Vec3::new(x1, 0.0, 0.0) + y);
            let t = m.trace(s, e, POINT.0, POINT.1, MASK_PLAYERSOLID);
            (x0 + (x1 - x0) * t.fraction, t.normal)
        };
        let hit = along(10.0, 300.0);
        assert!(near(hit.0, 32.0 - 0.125) && hit.1 == Vec3::NEG_X, "{hit:?}");
        let hit = along(100.0, 300.0);
        assert!(
            near(hit.0, 104.0 - 0.125) && hit.1 == Vec3::NEG_X,
            "{hit:?}"
        );
        // At z 20 the wedge starts where the slope reaches it, x 168.
        let hit = along(122.0, 300.0);
        let n = rich::slope().normal;
        assert!(
            near(hit.0, 168.0 + 0.125 / n.x) && near3(hit.1, n),
            "{hit:?}"
        );
        let hit = along(126.0, 100.0);
        assert!(near(hit.0, 120.0 + 0.125) && hit.1 == Vec3::X, "{hit:?}");
    }
}

/// The CoD2 and CoD4 triangle kernels on the terrain patch: a point onto a
/// face, a capsule onto the ridge edge and onto the high corner's vertex,
/// and a capsule moving west into the border along the patch's east edge.
#[test]
fn a_capsule_meets_the_terrain_faces_edges_vertex_and_border() {
    let v = rich::verts();
    let n0 = (v[1] - v[0]).cross(v[2] - v[0]).normalize();
    for indexed in [false, true] {
        let m = rich::map(indexed, false);
        // A point down through the middle of triangle 0 (x -100, y -80).
        let p = Vec3::new(-100.0, -80.0, 0.0);
        let z = (n0.dot(v[0]) - n0.x * p.x - n0.y * p.y) / n0.z;
        let (s, e) = (Vec3::new(p.x, p.y, 100.0), Vec3::new(p.x, p.y, -100.0));
        let t = m.trace(s, e, POINT.0, POINT.1, MASK_PLAYERSOLID);
        // CoD2 stops 0.125 short along the normal; CoD4 has no epsilon.
        let (d1, d2) = (n0.dot(s) - n0.dot(v[0]), n0.dot(e) - n0.dot(v[0]));
        let want = if indexed {
            (100.0 - z) / 200.0
        } else {
            (d1 - 0.125) / (d1 - d2)
        };
        assert!(near(t.fraction, want), "{indexed} {t:?}");
        assert!(
            near3(t.normal, n0) && t.surface_flags == rich::TERRAIN,
            "{t:?}"
        );

        // Over the ridge, both faces' contacts fall on the other face: the
        // lower sphere (15 over the feet) meets the edge when it is the
        // reach off the edge's line. 0.85 along, near enough an end that
        // the edge's own bounds count.
        let mid = v[0] + (v[2] - v[0]) * 0.85;
        let d = (v[2] - v[0]).normalize();
        let reach = 15.125;
        let horiz = (d.x * d.x + d.y * d.y).sqrt();
        let zc = mid.z + reach / horiz;
        let (s, e) = (
            Vec3::new(mid.x, mid.y, 100.0),
            Vec3::new(mid.x, mid.y, -100.0),
        );
        let t = m.trace(s, e, PLAYER.0, PLAYER.1, MASK_PLAYERSOLID);
        let want = (115.0 - zc) / 200.0;
        let up = (Vec3::Z - d * d.z).normalize();
        assert!(
            (t.fraction - want).abs() < 1e-3,
            "{indexed} ridge {t:?} want {want}"
        );
        assert!(
            (t.normal - up).abs().max_element() < 1e-3,
            "{indexed} ridge {t:?}"
        );
        // Standing on it: the lower sphere's centre 10 off the edge holds
        // the capsule, 16 off does not.
        let solid = |c: Vec3| {
            let feet = c - Vec3::new(0.0, 0.0, 15.0);
            m.trace(feet, feet, PLAYER.0, PLAYER.1, MASK_PLAYERSOLID)
                .allsolid
        };
        assert!(solid(mid + up * 10.0), "{indexed} on the ridge");
        assert!(!solid(mid + up * 16.0), "{indexed} over the ridge");

        // Beyond the high corner on both edges' lines: its vertex.
        let off = Vec3::new(8.0, 6.0, 0.0);
        let zc = v[2].z + (reach * reach - off.length_squared()).sqrt();
        let (s, e) = (
            v[2] + off + Vec3::new(0.0, 0.0, 100.0),
            v[2] + off - Vec3::new(0.0, 0.0, 100.0),
        );
        let t = m.trace(s, e, PLAYER.0, PLAYER.1, MASK_PLAYERSOLID);
        let want = (s.z + 15.0 - zc) / 200.0;
        let normal = (Vec3::new(off.x, off.y, zc - v[2].z)) / reach;
        assert!(
            (t.fraction - want).abs() < 1e-3,
            "{indexed} vertex {t:?} want {want}"
        );
        assert!(
            (t.normal - normal).abs().max_element() < 1e-3,
            "{indexed} vertex {t:?}"
        );
        assert!(
            solid(v[2] + off + Vec3::new(0.0, 0.0, 5.0)),
            "{indexed} on the vertex"
        );
        assert!(
            !solid(v[2] + off + Vec3::new(0.0, 0.0, 12.0)),
            "{indexed} over the vertex"
        );

        // West at y 0, where the wall's top is 16: the straight part
        // (centre 25 +- 20) spans it, and the capsule stops a reach off
        // x -64.
        let (s, e) = (Vec3::new(-20.0, 0.0, -10.0), Vec3::new(-120.0, 0.0, -10.0));
        let t = m.trace(s, e, PLAYER.0, PLAYER.1, MASK_PLAYERSOLID);
        assert!(
            near(t.fraction, (44.0 - reach) / 100.0),
            "{indexed} border {t:?}"
        );
        assert_eq!(t.normal, Vec3::X, "{indexed} border");
        // Wholly over the wall's top it passes on to the faces.
        let (s, e) = (Vec3::new(-20.0, 0.0, 40.0), Vec3::new(-120.0, 0.0, 40.0));
        assert_eq!(
            m.trace(s, e, PLAYER.0, PLAYER.1, MASK_PLAYERSOLID).fraction,
            1.0
        );
    }
}

/// The position tests and the point contents across both BSP levels, the
/// wedge's non-axial sides and the straddler; and the leaves a box
/// touches across the diagonal split.
#[test]
fn positions_and_contents_follow_the_sides_and_the_tree() {
    for indexed in [false, true] {
        for kd in [false, true] {
            let m = rich::map(indexed, kd);
            let solid = |p: Vec3| m.trace(p, p, PLAYER.0, PLAYER.1, MASK_PLAYERSOLID).allsolid;
            // Under the slope, over it, and in the cut corner.
            assert_eq!(
                m.point_contents(Vec3::new(200.0, 0.0, 10.0)),
                CONTENTS_SOLID
            );
            assert_eq!(m.point_contents(Vec3::new(200.0, 0.0, 50.0)), 0);
            assert_eq!(m.point_contents(Vec3::new(250.0, 60.0, 5.0)), 0);
            assert_eq!(
                m.point_contents(Vec3::new(112.0, 0.0, 150.0)),
                CONTENTS_SOLID
            );
            assert!(solid(Vec3::new(112.0, 0.0, 100.0)));
            assert!(solid(Vec3::new(200.0, 0.0, 0.0)));
            // The lower sphere 10 and 17 off the slope at x 160: in, then
            // clear. The side is pushed out by the sphere's height too, or
            // the first would pass.
            let n = rich::slope().normal;
            let feet = |off: f32| Vec3::new(160.0, 0.0, 16.0) + n * off - Vec3::new(0.0, 0.0, 15.0);
            assert!(solid(feet(10.0)));
            assert!(!solid(feet(17.0)));
            // Sunk into the terrain, and clear over it.
            assert!(solid(Vec3::new(-160.0, -60.0, 0.0)), "{indexed}");
            assert!(!solid(Vec3::new(-160.0, -60.0, 40.0)), "{indexed}");
            assert_eq!(
                m.box_leafs(Vec3::new(40.0, -50.0, 0.0), Vec3::new(45.0, -30.0, 10.0)),
                [1]
            );
            assert_eq!(
                m.box_leafs(Vec3::new(40.0, 60.0, 0.0), Vec3::new(45.0, 80.0, 10.0)),
                [2]
            );
            assert_eq!(
                m.box_leafs(Vec3::new(40.0, 40.0, 0.0), Vec3::new(50.0, 60.0, 10.0)),
                [1, 2]
            );
            assert_eq!(
                m.box_leafs(Vec3::new(-5.0, -50.0, 0.0), Vec3::new(5.0, -40.0, 10.0)),
                [1, 0]
            );
        }
    }
}

/// Seeded sweeps and position tests of a point, a small box and the
/// player over the rich map, in both triangle forms and both brush
/// layouts: the node-reach prune (vcod's, not retail's) gives the unpruned
/// walk's trace, and the k-d tree gives the flat leaf's.
#[test]
fn the_prune_and_the_brush_tree_change_no_trace() {
    let mut seed = 0x9e37_79b9_7f4a_7c15u64;
    let mut r = |lo: f32, hi: f32| {
        let u = (crate::rng::xorshift(&mut seed) >> 40) as f32 / (1u64 << 24) as f32;
        lo + (hi - lo) * u
    };
    let boxes = [
        POINT,
        (Vec3::splat(-4.0), Vec3::splat(4.0)),
        PLAYER,
        (Vec3::new(-15.0, -15.0, 0.0), Vec3::new(15.0, 15.0, 50.0)),
    ];
    for indexed in [false, true] {
        let flat = rich::map(indexed, false);
        let kd = rich::map(indexed, true);
        let free = rich::unpruned(&kd);
        let mut hits = 0;
        for i in 0..4000 {
            let s = Vec3::new(r(-300.0, 300.0), r(-200.0, 200.0), r(-20.0, 220.0));
            // Most end among the brushes and over the terrain.
            let e = if i % 5 == 0 {
                s
            } else {
                Vec3::new(r(-260.0, 260.0), r(-130.0, 130.0), r(-10.0, 80.0))
            };
            let (mins, maxs) = boxes[i % boxes.len()];
            let t = kd.trace(s, e, mins, maxs, MASK_PLAYERSOLID);
            hits += usize::from(t.fraction < 1.0);
            let (u, f) = (
                free.trace(s, e, mins, maxs, MASK_PLAYERSOLID),
                flat.trace(s, e, mins, maxs, MASK_PLAYERSOLID),
            );
            assert_eq!(t, u, "{indexed} pruned vs not: {s} -> {e} box {maxs}");
            // Inside one brush and touching another, the order the walk
            // meets them in picks whether the normal is kept, in either.
            let all = |t: ClipTrace| (t.fraction, t.allsolid, t.startsolid, t.contents);
            if t.allsolid {
                assert_eq!(
                    all(t),
                    all(f),
                    "{indexed} tree vs flat: {s} -> {e} box {maxs}"
                );
            } else {
                assert_eq!(t, f, "{indexed} tree vs flat: {s} -> {e} box {maxs}");
            }
        }
        // Enough of them meet something for the comparison to mean much.
        assert!(hits > 1000, "{indexed}: {hits} hits");
    }
}
