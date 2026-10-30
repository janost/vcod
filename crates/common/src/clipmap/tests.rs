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
