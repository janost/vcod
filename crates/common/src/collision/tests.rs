use super::*;
use super::{build::*, clip::*, entities::*, masks::*, walk::*};
use crate::net::protocol::{ENTITYNUM_NONE, ENTITYNUM_WORLD};

/// A world clip that is one sky-only slab: only a mask carrying
/// `CONTENTS_SKY` (0x800) stops on it.
struct SkySlab;

impl WorldClip for SkySlab {
    fn trace(&self, _: Vec3, _: Vec3, _: Vec3, _: Vec3, mask: u32) -> WorldHit {
        let hit = mask & 0x800 != 0;
        WorldHit {
            fraction: if hit { 0.5 } else { 1.0 },
            normal: if hit { Vec3::Z } else { Vec3::ZERO },
            surface_flags: 0,
            contents: if hit { 0x800 } else { 0 },
            startsolid: false,
            allsolid: false,
        }
    }
    fn point_contents(&self, _: Vec3) -> u32 {
        0
    }
    fn shot_mask(&self) -> u32 {
        crate::clipmap::MASK_SHOT
    }
}

/// `shot_trace` on a world with a game's clip traces under that
/// game's bullet mask, which carries sky where CoD1's does not.
#[test]
fn shot_trace_takes_the_world_clips_mask() {
    let mut world = test_world(&[]);
    let (start, end) = (Vec3::new(0.0, 0.0, 300.0), Vec3::new(0.0, 0.0, 100.0));
    assert_eq!(MASK_SHOT & 0x800, 0);
    world.set_world_clip(Arc::new(SkySlab));
    assert!(world.shot_trace(start, end).fraction < 1.0);
}

/// mp_depot's `*1` is an exploder and a `bombzone`, so `sd` keeps the
/// gameobject and `_load.gsc` still takes it out; `*2` carries
/// `script_exploder` with no targetname and stays solid.
#[test]
fn mp_depots_exploder_leaves_the_clip_under_sd() {
    let Some(fs) = crate::testing::game_fs() else {
        return;
    };
    let bsp = crate::bsp::parse(&fs.read("maps/mp/mp_depot.bsp").unwrap()).unwrap();
    let world = CollisionWorld::build(&bsp, &[]);
    world.unlink_script_brushes(&bsp.entities, "sd");
    assert!(!world.model_linked(1));
    assert!(world.model_linked(2));
}

/// A world copy clips the world's own brushes where its entity stands,
/// names that entity, and leaves the submodels out: the retail probe
/// `probe_spawnclip` (movers doc 19) put a ground trace 64 units up
/// under a script_brushmodel spawned at (0, 0, 64).
#[test]
fn a_world_copy_clips_where_its_entity_stands() {
    let world = submodel_test_world("", &[([400.0, -8.0, 0.0], [416.0, 8.0, 32.0])]);
    let down = |x: f32| {
        let t = world.shot_trace(Vec3::new(x, 0.0, 200.0), Vec3::new(x, 0.0, -100.0));
        (t.endpos.z.round(), world.entity_num(&t))
    };
    assert_eq!(down(0.0), (0.0, ENTITYNUM_WORLD));
    world.set_world_copy(299, Some((Vec3::new(0.0, 0.0, 64.0), Vec3::ZERO, 1)));
    assert_eq!(down(0.0), (64.0, 299));
    assert_eq!(down(408.0), (64.0, 299), "the floor copy, not the submodel");
    assert_ne!(world.point_contents(Vec3::new(0.0, 0.0, 60.0)), 0);
    // `notSolid`: contents 0 meets no mask.
    world.set_world_copy(299, Some((Vec3::new(0.0, 0.0, 64.0), Vec3::ZERO, 0)));
    assert_eq!(down(0.0), (0.0, ENTITYNUM_WORLD));
    world.set_world_copy(299, Some((Vec3::new(0.0, 0.0, -8.0), Vec3::ZERO, 1)));
    assert_eq!(down(0.0), (0.0, ENTITYNUM_WORLD), "a copy below loses");
    world.set_world_copy(299, None);
    assert_eq!(down(0.0), (0.0, ENTITYNUM_WORLD));
}

/// `CM_TransformedBoxTrace` turns the capsule's sphere offset into a
/// turned copy's frame, so the capsule stays upright in the world.
#[test]
fn a_turned_copy_keeps_its_capsule_upright() {
    let copy = |angles: [f32; 3]| WorldCopy {
        entity: 299,
        origin: Vec3::ZERO,
        axis: Some(crate::pmove::aim::angles_to_axis(angles).map(Vec3::from)),
        contents: 1,
    };
    let up = Vec3::new(0.0, 0.0, 20.0);
    for angles in [[30.0, 0.0, 0.0], [5.0, 45.0, 0.0], [-60.0, 120.0, 0.0]] {
        let c = copy(angles);
        let world = c.to_world_dir(c.sphere(up));
        assert!((world - up).length() < 1e-4, "{angles:?}: {world:?}");
    }
    assert_eq!(copy([0.0, 90.0, 0.0]).sphere(up), up);
}

/// A capsule meets a brush's axial sides as its own box in the brush's
/// frame, `|sphere| + r` a side: a player falling onto a slab pitched 90
/// degrees rests on its top face, as retail's did (`probe_tiltslab`,
/// movers doc 22), not 20 units into it.
#[test]
fn a_turned_capsule_meets_axial_sides_with_its_turned_box() {
    // The slab in its own frame, 4 thick along x; pitched 90 its +x face
    // is the world's top and the world's down is its +x.
    let planes = [
        (Vec3::NEG_X, 2.0),
        (Vec3::X, 2.0),
        (Vec3::NEG_Y, 28.0),
        (Vec3::Y, 28.0),
        (Vec3::NEG_Z, 48.0),
        (Vec3::Z, 48.0),
    ];
    let (mins, maxs) = (Vec3::new(-15.0, -15.0, -35.0), Vec3::new(15.0, 15.0, 35.0));
    let (start, end) = (Vec3::new(100.0, 0.0, 0.0), Vec3::ZERO);
    let stop = |sphere: Vec3| {
        let mut trace = Trace::clear(end);
        let sweep = capsule_sweep(start, end, mins, maxs);
        clip_brush_capsule(
            &mut trace,
            &sweep,
            sphere,
            &planes,
            &[],
            Prim::Brush(0),
            Arith::Kept,
            false,
        );
        start.x + (end.x - start.x) * trace.fraction
    };
    // Upright: the radius off the face.
    assert!((stop(Vec3::new(0.0, 0.0, 20.0)) - 17.125).abs() < 1e-3);
    // Pitched 90: the half height off it.
    assert!((stop(Vec3::new(-20.0, 0.0, 0.0)) - 37.125).abs() < 1e-3);
}

/// The push's two questions about a world copy: whether a box is in
/// the copy alone, and whether a spot is clear of everything with the
/// copy in it; the push sweep passes through the copy it skips.
#[test]
fn a_pusher_copy_is_tested_on_its_own() {
    let world = submodel_test_world("", &[]);
    let (mins, maxs) = (Vec3::new(-15.0, -15.0, 0.0), Vec3::new(15.0, 15.0, 70.0));
    let at = Vec3::new(0.0, 0.0, 0.125);
    world.set_world_copy(171, Some((Vec3::new(0.0, 0.0, 8.0), Vec3::ZERO, 1)));
    let copy = Pusher::Copy(171);
    assert!(
        world
            .pusher_box_trace(copy, at, at, mins, maxs, MASK_PLAYERSOLID)
            .startsolid
    );
    assert!(
        !world
            .pusher_box_trace(Pusher::Copy(5), at, at, mins, maxs, MASK_PLAYERSOLID)
            .startsolid
    );
    assert!(world.box_position_test(at, mins, maxs, MASK_PLAYERSOLID));
    let up = at + Vec3::new(0.0, 0.0, 8.0);
    assert!(!world.box_position_test(up, mins, maxs, MASK_PLAYERSOLID));
    let t = world.box_trace_except(at, up, mins, maxs, MASK_PLAYERSOLID, copy);
    assert!(!t.startsolid && t.fraction == 1.0);
    world.set_world_copy(171, Some((Vec3::new(0.0, 0.0, 8.0), Vec3::ZERO, 0)));
    assert!(
        !world.box_position_test(at, mins, maxs, MASK_PLAYERSOLID),
        "notSolid"
    );
}

/// A prop wall at x = 5000 facing -x, two triangles wound outward.
fn prop_wall(contents: u32) -> [ModelTri; 2] {
    let (a, b, c, d) = (
        Vec3::new(5000.0, -50.0, 0.0),
        Vec3::new(5000.0, 50.0, 0.0),
        Vec3::new(5000.0, 50.0, 100.0),
        Vec3::new(5000.0, -50.0, 100.0),
    );
    let mt = |tri| ModelTri {
        tri,
        contents,
        surface_flags: 21 << 20,
    };
    [mt([a, c, b]), mt([a, d, c])]
}

/// A window pane (contents 0x8000010, no SOLID bit) is a brush every
/// mask meets, and the trace names its contents; a shot that starts
/// inside it, where a glass leg's nudge puts the next one, leaves it,
/// through a SOLID soup drawn on its far face.
#[test]
fn a_glass_brush_clips_and_reports_its_contents() {
    let glass = 0x8000010;
    let face = |x: f32| {
        let c = |y: f32, z: f32| Vec3::new(x, y, z);
        [
            [c(-64.0, 0.0), c(64.0, 0.0), c(64.0, 128.0)],
            [c(-64.0, 0.0), c(64.0, 128.0), c(-64.0, 128.0)],
        ]
    };
    let world = synthetic_world_tris(
        &[
            ("textures/test/solid", CONTENTS_SOLID, 0),
            ("textures/test/glass", glass, 0x900000),
        ],
        &[(1, [40.0, -64.0, 0.0], [42.0, 64.0, 128.0])],
        &face(42.0),
    );
    let (start, end) = (Vec3::new(0.0, 0.0, 60.0), Vec3::new(100.0, 0.0, 60.0));
    let t = world.shot_trace(start, end);
    assert!(
        (t.endpos.x - (40.0 - SURFACE_CLIP_EPSILON)).abs() < 1e-3,
        "{t:?}"
    );
    assert_eq!(world.hit_contents(&t), glass);
    assert_eq!(sound_material(t.surface_flags), 9);
    let mins = Vec3::new(-15.0, -15.0, 0.0);
    let maxs = Vec3::new(15.0, 15.0, 70.0);
    let moved = world.box_trace(start - Vec3::Z * 60.0, end - Vec3::Z * 60.0, mins, maxs);
    assert!(moved.fraction < 1.0, "a player stops at the pane");
    assert!(world.missile_trace(start, end).fraction < 1.0);

    let inside = world.shot_trace(Vec3::new(40.125, 0.0, 60.0), end);
    assert_eq!(inside.fraction, 1.0, "{inside:?}");
    assert_eq!(world.hit_contents(&inside), 0);
}

/// A 0x2080 kerb brush stops a shot, a blast probe, a missile and a
/// falling item, and a player walks through it.
#[test]
fn a_kerb_brush_clips_every_mask_but_the_players() {
    let kerb = 0x2080;
    let world = synthetic_world(
        &[("textures/test/kerb", kerb, 0)],
        &[(0, [-64.0, -64.0, -16.0], [64.0, 64.0, 0.0])],
    );
    let (above, below) = (Vec3::new(0.0, 0.0, 32.0), Vec3::new(0.0, 0.0, -32.0));
    let shot = world.shot_trace(above, below);
    assert!(shot.fraction < 1.0, "{shot:?}");
    assert_eq!(world.hit_contents(&shot), kerb);
    assert!(world.point_trace(above, below, MASK_BLAST, true).fraction < 1.0);
    let item = world.item_trace(above, below, -Vec3::ONE, Vec3::ONE, 0x81);
    assert!(item.fraction < 1.0, "{item:?}");
    assert_eq!(world.point_contents(Vec3::new(0.0, 0.0, -8.0)), kerb);

    let (mins, maxs) = (Vec3::new(-15.0, -15.0, 0.0), Vec3::new(15.0, 15.0, 70.0));
    assert!(world.missile_trace(above, below).fraction < 1.0);
    assert_eq!(world.box_trace(above, below, mins, maxs).fraction, 1.0);
}

/// A prop's triangles stop a shot through them, front face only,
/// backing off the clip epsilon, and carry the surface's flags.
#[test]
fn model_triangles_clip_a_shot() {
    let (start, end) = (Vec3::new(4900.0, 0.0, 50.0), Vec3::new(5100.0, 0.0, 50.0));
    let bare = CollisionWorld::build(&tiny_world(), &[]);
    assert_eq!(bare.shot_trace(start, end).fraction, 1.0);
    let world = CollisionWorld::build(&tiny_world(), &prop_wall(CONTENTS_SOLID));
    let t = world.shot_trace(start, end);
    assert!(
        t.fraction < 1.0 && (t.endpos.x - (5000.0 - SURFACE_CLIP_EPSILON)).abs() < 1e-3,
        "{t:?}"
    );
    assert!(t.normal.abs_diff_eq(-Vec3::X, 1e-4), "{t:?}");
    assert_eq!(sound_material(t.surface_flags), 21);
    assert!(matches!(t.hit, Some(Prim::Model(_))), "{t:?}");
    // From behind, the same wall is open.
    assert_eq!(world.shot_trace(end, start).fraction, 1.0);
    // A segment past the triangle's edge misses by more than the epsilon.
    let over = world.shot_trace(start + Vec3::Z * 50.2, end + Vec3::Z * 50.2);
    assert_eq!(over.fraction, 1.0, "{over:?}");
    // Glass stops a shot too; a canopy (contents 0) stops nothing.
    let glass = CollisionWorld::build(&tiny_world(), &prop_wall(CONTENTS_GLASS));
    assert!(glass.shot_trace(start, end).fraction < 1.0);
    let canopy = CollisionWorld::build(&tiny_world(), &prop_wall(0));
    assert_eq!(canopy.shot_trace(start, end).fraction, 1.0);
}

/// A movement trace never meets a prop, whatever its shape: retail's
/// `trap_Trace` and `trap_TraceCapsule` leave the static models to the
/// locational trace, so a player walks through a chair a clip brush
/// does not wrap.
#[test]
fn model_triangles_never_meet_a_movement_trace() {
    let world = CollisionWorld::build(&tiny_world(), &prop_wall(CONTENTS_SOLID));
    let (start, end) = (Vec3::new(4900.0, 0.0, 10.0), Vec3::new(5100.0, 0.0, 10.0));
    assert_eq!(
        world.box_trace(start, end, Vec3::ZERO, Vec3::ZERO).fraction,
        1.0
    );
    let mins = Vec3::new(-15.0, -15.0, 0.0);
    let maxs = Vec3::new(15.0, 15.0, 70.0);
    assert_eq!(world.box_trace(start, end, mins, maxs).fraction, 1.0);
    assert!(world.shot_trace(start, end).fraction < 1.0);
}
use crate::mapworld::{self, MapWorld};
use glam::Vec3;

/// One axial brush (-64,-64,-16)..(64,64,0), one upward triangle patch
/// at z=10 over x,y in 100..200.
pub(crate) fn tiny_world() -> MapWorld {
    let dist = |v: f32| v.to_bits();
    let side = |v: f32| mapworld::BrushSide {
        plane_or_dist: dist(v),
        material: 0,
    };
    let mut bsp = MapWorld {
        materials: vec![mapworld::Material {
            name: "textures/test/solid".into(),
            surface_flags: 0,
            content_flags: 0x1,
        }],
        brush_sides: vec![
            side(-64.0),
            side(64.0), // xmin, xmax
            side(-64.0),
            side(64.0), // ymin, ymax
            side(-16.0),
            side(0.0), // zmin, zmax
        ],
        brushes: vec![mapworld::Brush {
            first_side: 0,
            num_sides: 6,
            material: 0,
        }],
        models: vec![mapworld::Model {
            mins: [-64.0, -64.0, -16.0],
            maxs: [64.0, 64.0, 0.0],
            first_soup: 0,
            num_soups: 0,
            first_brush: 0,
            partitions: (0, 0),
            num_brushes: 1,
        }],
        ..Default::default()
    };
    push_tri_patches(
        &mut bsp,
        &[[
            Vec3::new(100.0, 100.0, 10.0),
            Vec3::new(200.0, 100.0, 10.0),
            Vec3::new(100.0, 200.0, 10.0),
        ]],
        0,
    );
    bsp
}

fn vert(pos: [f32; 3]) -> mapworld::DrawVert {
    mapworld::DrawVert {
        pos,
        uv: [0.0; 2],
        lm_uv: [0.0; 2],
        normal: [0.0, 0.0, 1.0],
        color: [255; 4],
    }
}

#[test]
fn builds_brush_planes_from_axial_bounds() {
    let world = CollisionWorld::build(&tiny_world(), &[]);
    assert_eq!(world.brushes.len(), 1);
    let planes = &world.brushes[0].planes;
    assert_eq!(planes.len(), 6);
    // zmax face: normal +Z, dist 0
    assert!(
        planes
            .iter()
            .any(|(n, d)| n.abs_diff_eq(Vec3::Z, 1e-6) && *d == 0.0)
    );
    // xmin face: normal -X, dist 64 (inside test: -x <= 64  =>  x >= -64)
    assert!(
        planes
            .iter()
            .any(|(n, d)| n.abs_diff_eq(-Vec3::X, 1e-6) && *d == 64.0)
    );
}

#[test]
fn builds_patches_and_answers_aabb_queries() {
    let world = CollisionWorld::build(&tiny_world(), &[]);
    assert_eq!(world.patches.len(), 1);
    let mut out = Vec::new();
    world.candidates(
        Vec3::new(140.0, 140.0, 0.0),
        Vec3::new(150.0, 150.0, 20.0),
        &mut out,
    );
    assert!(out.iter().any(|p| matches!(p, Prim::Patch(_))));
    out.clear();
    world.candidates(
        Vec3::new(0.0, 0.0, -8.0),
        Vec3::new(1.0, 1.0, 8.0),
        &mut out,
    );
    assert!(out.iter().any(|p| matches!(p, Prim::Brush(_))));
    out.clear();
    world.candidates(
        Vec3::new(9000.0, 9000.0, 0.0),
        Vec3::new(9001.0, 9001.0, 1.0),
        &mut out,
    );
    assert!(out.is_empty());
}

#[test]
fn builds_mp_pavlov_world() {
    let Some(data) = crate::testing::real_bsp() else {
        return;
    };
    let bsp = crate::bsp::parse(&data).unwrap();
    let world = CollisionWorld::build(&bsp, &[]);
    // model 0's 7575 solid+playerclip brushes plus its two stray
    // non-trigger submodel clips; the 32 trigger brushes stay hollow
    assert_eq!(world.brushes.len(), 7577);
    // terrain triangles and lump 24's patches
    assert!(world.tris.len() > 4_000, "{}", world.tris.len());
    assert_eq!(world.patches.len(), bsp.patches.len());
    // a query around a known spawn; only holds if candidates() walks from the root
    let mut out = Vec::new();
    world.candidates(
        Vec3::new(-9100.0, 8950.0, -50.0),
        Vec3::new(-8800.0, 9250.0, 150.0),
        &mut out,
    );
    assert!(!out.is_empty());
}

/// Six triangle patches 1000 units apart along X: the median split
/// yields a 3-node BVH, so root (0) and last-pushed node (2) differ and
/// a walk that starts anywhere but the root misses the low-X leaf.
fn spread_tris_world() -> MapWorld {
    const N: usize = 6;
    let tris: Vec<[Vec3; 3]> = (0..N)
        .map(|i| {
            let ox = i as f32 * 1000.0;
            [
                Vec3::new(ox, 0.0, 10.0),
                Vec3::new(ox + 10.0, 0.0, 10.0),
                Vec3::new(ox, 10.0, 10.0),
            ]
        })
        .collect();
    let mut bsp = MapWorld {
        materials: vec![mapworld::Material {
            name: "textures/test/solid".into(),
            surface_flags: 0,
            content_flags: 0x1,
        }],
        models: vec![mapworld::Model {
            mins: [0.0, 0.0, 0.0],
            maxs: [0.0, 0.0, 0.0],
            first_soup: 0,
            num_soups: 0,
            first_brush: 0,
            partitions: (0, 0),
            num_brushes: 0,
        }],
        ..Default::default()
    };
    push_tri_patches(&mut bsp, &tris, 0);
    bsp
}

fn world() -> CollisionWorld {
    CollisionWorld::build(&tiny_world(), &[])
}

#[test]
fn ray_down_hits_brush_top() {
    let t = world().box_trace(
        Vec3::new(0.0, 0.0, 100.0),
        Vec3::new(0.0, 0.0, -100.0),
        Vec3::ZERO,
        Vec3::ZERO,
    );
    assert!(t.fraction < 1.0 && !t.startsolid);
    assert!(t.normal.abs_diff_eq(Vec3::Z, 1e-5));
    // stops SURFACE_CLIP_EPSILON-ish above the face at z=0
    assert!((t.endpos.z - 0.0).abs() < 0.2, "endpos {}", t.endpos);
}

#[test]
fn box_down_rests_on_brush_by_its_mins() {
    let t = world().box_trace(
        Vec3::new(0.0, 0.0, 100.0),
        Vec3::new(0.0, 0.0, -100.0),
        Vec3::new(-15.0, -15.0, 0.0),
        Vec3::new(15.0, 15.0, 70.0),
    );
    // bbox mins.z = 0 => origin comes to rest at the face, z ~ 0
    assert!(t.fraction < 1.0 && (t.endpos.z - 0.0).abs() < 0.2);
}

#[test]
fn miss_returns_full_fraction() {
    let t = world().box_trace(
        Vec3::new(500.0, 500.0, 100.0),
        Vec3::new(500.0, 500.0, 50.0),
        Vec3::ZERO,
        Vec3::ZERO,
    );
    assert_eq!(t.fraction, 1.0);
    assert_eq!(t.endpos, Vec3::new(500.0, 500.0, 50.0));
}

#[test]
fn start_inside_brush_is_startsolid() {
    let t = world().box_trace(
        Vec3::new(0.0, 0.0, -8.0),
        Vec3::new(0.0, 0.0, -8.0),
        Vec3::ZERO,
        Vec3::ZERO,
    );
    assert!(t.startsolid && t.allsolid && t.fraction == 0.0);
}

#[test]
fn ray_down_hits_triangle() {
    let t = world().box_trace(
        Vec3::new(120.0, 120.0, 100.0),
        Vec3::new(120.0, 120.0, -100.0),
        Vec3::ZERO,
        Vec3::ZERO,
    );
    assert!(t.fraction < 1.0);
    assert!(t.normal.abs_diff_eq(Vec3::Z, 1e-4));
    assert!((t.endpos.z - 10.0).abs() < 0.2);
}

#[test]
fn box_sideways_into_triangle_edge_stops_outside() {
    let t = world().box_trace(
        Vec3::new(50.0, 120.0, 10.0),
        Vec3::new(150.0, 120.0, 10.0),
        Vec3::splat(-5.0),
        Vec3::splat(5.0),
    );
    assert!(t.fraction < 1.0);
    assert!(
        t.endpos.x <= 95.5,
        "box center should stop before the edge, got {}",
        t.endpos.x
    );
}

#[test]
fn answers_aabb_queries_across_a_multi_node_bvh() {
    let world = CollisionWorld::build(&spread_tris_world(), &[]);
    assert_eq!(world.patches.len(), 6);

    // low-X triangle: only reachable from the true root
    let mut out = Vec::new();
    world.candidates(
        Vec3::new(-5.0, -5.0, 0.0),
        Vec3::new(15.0, 15.0, 20.0),
        &mut out,
    );
    assert!(out.iter().any(|p| matches!(p, Prim::Patch(0))));

    // high-X triangle (index 5)
    out.clear();
    let ox = 5.0 * 1000.0;
    world.candidates(
        Vec3::new(ox - 5.0, -5.0, 0.0),
        Vec3::new(ox + 15.0, 15.0, 20.0),
        &mut out,
    );
    assert!(out.iter().any(|p| matches!(p, Prim::Patch(5))));

    // nowhere near any triangle
    out.clear();
    world.candidates(
        Vec3::new(50_000.0, 50_000.0, 0.0),
        Vec3::new(50_001.0, 50_001.0, 1.0),
        &mut out,
    );
    assert!(out.is_empty());
}

/// Every prim clipped, no traversal: the oracle for the ordered walk.
fn brute_trace(world: &CollisionWorld, start: Vec3, end: Vec3, mins: Vec3, maxs: Vec3) -> Trace {
    let mut trace = Trace::clear(end);
    let capsule = Capsule::of(mins, maxs);
    let sweep = capsule_sweep(start, end, mins, maxs);
    for brushes in [true, false] {
        for (prim, _, _) in &world.prims {
            if trace.fraction <= 0.0 || brushes != matches!(prim, Prim::Brush(_)) {
                continue;
            }
            match *prim {
                Prim::Brush(i) => {
                    let brush = &world.brushes[i as usize];
                    clip_brush_capsule(
                        &mut trace,
                        &sweep,
                        capsule.offset,
                        &brush.planes,
                        &brush.side_surface_flags,
                        *prim,
                        Arith::Kept,
                        false,
                    );
                }
                Prim::Terrain(i) => {
                    world.clip_terrain(i, start, end, mins, maxs, capsule, u32::MAX, &mut trace);
                }
                Prim::Tri(_) => unreachable!(),
                Prim::Patch(i) => {
                    clip_patch(
                        &mut trace,
                        start,
                        end,
                        capsule,
                        &world.patches[i as usize],
                        Arith::Kept,
                        *prim,
                    );
                }
                Prim::Model(i) => {
                    clip_segment_model(
                        &mut trace,
                        start,
                        end,
                        &world.model_tris[i as usize],
                        *prim,
                        false,
                    );
                }
                // Never stored in world.prims.
                Prim::Body(_) | Prim::Clip(_) => unreachable!(),
            }
        }
    }
    trace.endpos = cm_endpos(start, end, mins, maxs, trace.fraction, Arith::Kept);
    trace
}

/// Deterministic xorshift, so a failure reproduces.
fn rng(seed: &mut u64) -> f32 {
    *seed ^= *seed << 13;
    *seed ^= *seed >> 7;
    *seed ^= *seed << 17;
    (*seed >> 40) as f32 / (1u64 << 24) as f32
}

fn check_against_brute(world: &CollisionWorld, lo: Vec3, hi: Vec3, sweeps: usize, seed: u64) {
    let mut s = seed;
    let mut hits = 0;
    for _ in 0..sweeps {
        let r = |s: &mut u64| Vec3::new(rng(s), rng(s), rng(s));
        let start = lo + (hi - lo) * r(&mut s);
        let end = lo + (hi - lo) * r(&mut s);
        let (mins, maxs) = if rng(&mut s) < 0.5 {
            (Vec3::ZERO, Vec3::ZERO)
        } else {
            (Vec3::new(-15.0, -15.0, 0.0), Vec3::new(15.0, 15.0, 60.0))
        };
        let a = world.box_trace(start, end, mins, maxs);
        let b = brute_trace(world, start, end, mins, maxs);
        assert!(
            (a.fraction - b.fraction).abs() <= 1e-5,
            "{start} -> {end}: {a:?} vs {b:?}"
        );
        assert_eq!(
            (a.startsolid, a.allsolid),
            (b.startsolid, b.allsolid),
            "{start} -> {end}"
        );
        if a.fraction < 1.0 {
            hits += 1;
            assert!(
                a.normal.abs_diff_eq(b.normal, 1e-4),
                "{start} -> {end}: {a:?} vs {b:?}"
            );
        }
    }
    assert!(hits > 20, "too few hits to trust the oracle: {hits}");
}

#[test]
fn ordered_walk_matches_brute_force_on_synthetic_worlds() {
    let w = spread_tris_world();
    // bounds hug triangle 0 so 300 sweeps land plenty of hits
    check_against_brute(
        &CollisionWorld::build(&w, &[]),
        Vec3::new(-20.0, -20.0, -20.0),
        Vec3::new(30.0, 30.0, 30.0),
        300,
        0x9E3779B97F4A7C15,
    );
    let w = test_world(&[(Vec3::new(50.0, -400.0, 0.0), Vec3::new(100.0, 400.0, 100.0))]);
    check_against_brute(
        &w,
        Vec3::new(-300.0, -300.0, -50.0),
        Vec3::new(300.0, 300.0, 200.0),
        300,
        0xD1B54A32D192ED03,
    );
}

#[test]
fn the_leaf_replay_matches_a_second_walk_on_mp_pavlov() {
    let Some(fs) = crate::testing::game_fs() else {
        return;
    };
    let bsp = crate::bsp::parse(&fs.read("maps/mp/mp_pavlov.bsp").unwrap()).unwrap();
    // With the props, so a shot walks the static models' subtree.
    let world = CollisionWorld::build(&bsp, &crate::props::collision_tris(&fs, &bsp.entities));
    assert_ne!(world.models_root, u32::MAX);
    let (lo, hi) = crate::mesh::map_bounds(&bsp);
    let mut s = 0x9E3779B97F4A7C15u64;
    let mut hits = 0;
    for i in 0..3000 {
        let r = |s: &mut u64| Vec3::new(rng(s), rng(s), rng(s));
        let start = Vec3::from(lo) + (Vec3::from(hi) - Vec3::from(lo)) * r(&mut s);
        let end = start + (r(&mut s) - Vec3::splat(0.5)) * if i % 2 == 0 { 100.0 } else { 2000.0 };
        let (mins, maxs) = if i % 3 == 0 {
            (Vec3::ZERO, Vec3::ZERO)
        } else {
            (Vec3::new(-15.0, -15.0, 0.0), Vec3::new(15.0, 15.0, 70.0))
        };
        let both = |f: &dyn Fn() -> Trace| {
            TWO_WALKS.set(false);
            let a = f();
            TWO_WALKS.set(true);
            let b = f();
            TWO_WALKS.set(false);
            (a, b)
        };
        for (a, b) in [
            both(&|| world.box_trace(start, end, mins, maxs)),
            both(&|| world.shot_trace(start, end)),
        ] {
            assert_eq!(
                a.fraction.to_bits(),
                b.fraction.to_bits(),
                "{start} -> {end}"
            );
            assert_eq!(a.endpos, b.endpos);
            assert_eq!(a.normal, b.normal);
            assert_eq!(
                (a.startsolid, a.allsolid, format!("{:?}", a.hit)),
                (b.startsolid, b.allsolid, format!("{:?}", b.hit))
            );
            assert_eq!(a.surface_flags, b.surface_flags);
            hits += usize::from(a.fraction < 1.0);
        }
    }
    assert!(hits > 1000, "{hits}");
}

/// A trace reports the side it stopped on, not the brush's material:
/// the floor brush at x 232..552, y 1096..1408 on mp_carentan is
/// `rock@cobbl_patrn1a` (17) with a wood top side (21), and retail
/// 1.1d's footsteps across it are `EV_FOOTSTEP_RUN_WOOD` (the
/// probe_surfside capture, docs/research/bsp-ibsp59-format.md).
#[test]
fn a_trace_reports_the_side_it_stops_on() {
    let Some(data) = crate::testing::game_fs().and_then(|fs| fs.read("maps/mp/mp_carentan.bsp"))
    else {
        return;
    };
    let bsp = crate::bsp::parse(&data).unwrap();
    let world = CollisionWorld::build(&bsp, &[]);
    let t = world.point_trace(
        Vec3::new(450.0, 1252.0, 40.0),
        Vec3::new(450.0, 1252.0, 0.0),
        MASK_PLAYERSOLID,
        false,
    );
    let Some(Prim::Brush(b)) = t.hit else {
        panic!("{:?}", t.hit);
    };
    assert_eq!(
        world.brushes[b as usize].material,
        "textures/normandy/floors/rock@cobbl_patrn1a"
    );
    assert_eq!(sound_material(t.surface_flags), 21);
}

/// A capsule standing across two floor brushes that meet at one
/// height enters both at the same fraction; retail reports the one its
/// leaf walk reaches first. On mp_carentan at (817.4, 694.6) that is
/// brush 3882's material 4, where the BVH met brush 3880's 21 first:
/// retail 1.1d, 1.5b and UO all step there with `EV_FOOTSTEP_RUN` 4
/// (the patchwalk pillar captures, bsp-ibsp59-format.md "The leaf
/// walk").
#[test]
fn a_capsule_on_two_floor_brushes_reads_the_first_in_leaf_order() {
    let Some(data) = crate::testing::game_fs().and_then(|fs| fs.read("maps/mp/mp_carentan.bsp"))
    else {
        return;
    };
    let bsp = crate::bsp::parse(&data).unwrap();
    let world = CollisionWorld::build(&bsp, &[]);
    let at = Vec3::new(817.44934, 694.5726, 8.125);
    let (mins, maxs) = (Vec3::new(-15.0, -15.0, 0.0), Vec3::new(15.0, 15.0, 70.0));
    for arith in [Arith::Kept, Arith::Stored] {
        world.set_arith(arith);
        let t = world.move_trace(
            at + Vec3::Z * 0.25,
            at - Vec3::Z * 0.25,
            mins,
            maxs,
            MASK_PLAYERSOLID,
        );
        assert_eq!(t.fraction, 0.5, "{arith:?}");
        assert_eq!(sound_material(t.surface_flags), 4, "{arith:?} {:?}", t.hit);
    }
    world.set_arith(Arith::Kept);
}

/// Model 0 in retail's leaf order alone, then the rest as `walk_tree`
/// clips it after its leaf walk: retail's answer to a trace.
fn leaf_order_trace(
    world: &CollisionWorld,
    start: Vec3,
    end: Vec3,
    mins: Vec3,
    maxs: Vec3,
    boxed: bool,
) -> Trace {
    let capsule = if boxed {
        Capsule::wide(mins, maxs)
    } else {
        Capsule::of(mins, maxs)
    };
    let sweep = TraceInput {
        start,
        end,
        mins,
        maxs,
        capsule,
        mask: MASK_PLAYERSOLID,
        statics: false,
        skip: None,
        world_only: false,
        boxed,
        sphere: capsule.offset,
    };
    let mut trace = Trace::clear(end);
    world.clip_leaf_walk(world.leaf_walk.as_ref().unwrap(), &sweep, &mut trace);
    let mut rest = Walk {
        sweep,
        leaves: [0; LEAF_LOG],
        logged: usize::MAX,
        patch_met: std::cell::Cell::new(false),
        rest: true,
    };
    world.trace_node(0, &mut rest, true, &mut trace);
    world.trace_node(0, &mut rest, false, &mut trace);
    trace
}

/// Ground traces, short runs and step-downs of a standing player, as a
/// capsule and as `trap_Trace`'s box, from random spots on the floors of
/// two stock maps: the tree walk reports what retail's leaf order does
/// wherever two contacts nearly tie (two brushes a float step apart, a
/// brush or a partition within the terrain's back-off of another) and,
/// wherever it stops at fraction 0 or starts all in solid, and wherever
/// it reaches terrain no leaf on its walk lists. Before the near-tie
/// replay mp_depot alone gave about one answer in eighty from the
/// BVH's order (bsp-ibsp59-format.md, "The leaf walk").
#[test]
fn near_ties_read_as_retails_leaf_order() {
    let (mins, maxs) = (Vec3::new(-15.0, -15.0, 0.0), Vec3::new(15.0, 15.0, 70.0));
    for map in ["mp_depot", "mp_brecourt"] {
        let Some(data) =
            crate::testing::game_fs_11().and_then(|fs| fs.read(&format!("maps/mp/{map}.bsp")))
        else {
            return;
        };
        let bsp = crate::bsp::parse(&data).unwrap();
        let world = CollisionWorld::build(&bsp, &[]);
        let (lo, hi) = crate::mesh::map_bounds(&bsp);
        let (lo, hi) = (Vec3::from(lo), Vec3::from(hi));
        let mut s = 0x2545F4914F6CDD1Du64;
        let (mut traces, mut wrong) = (0, Vec::new());
        for k in 0..6000 {
            let r = |s: &mut u64| Vec3::new(rng(s), rng(s), rng(s));
            let p = lo + (hi - lo) * r(&mut s);
            let down = world.move_trace(p, p - Vec3::Z * 2000.0, mins, maxs, MASK_PLAYERSOLID);
            if down.fraction >= 1.0 || down.startsolid {
                continue;
            }
            let o = down.endpos;
            let (a, b) = match k % 3 {
                0 => (o + Vec3::Z * 0.25, o - Vec3::Z * 0.25),
                1 => {
                    let run = (r(&mut s) - Vec3::splat(0.5)).with_z(0.0) * 40.0;
                    (o + Vec3::Z * 0.125, o + run + Vec3::Z * 0.125)
                }
                _ => (o + Vec3::Z * 18.0, o - Vec3::Z * 9.0),
            };
            let boxed = k % 2 == 1;
            let ours = world.trace_shape(a, b, mins, maxs, MASK_PLAYERSOLID, false, None, boxed);
            let retail = leaf_order_trace(&world, a, b, mins, maxs, boxed);
            traces += 1;
            let key = |t: &Trace| {
                (
                    t.fraction.to_bits(),
                    t.normal.to_array().map(f32::to_bits),
                    t.surface_flags,
                    t.startsolid,
                    t.allsolid,
                )
            };
            if key(&ours) != key(&retail) {
                wrong.push(format!(
                    "{map} {a} -> {b} boxed {boxed}: {ours:?} vs {retail:?}"
                ));
            }
        }
        assert!(traces > 2000, "{map}: {traces}");
        assert!(
            wrong.is_empty(),
            "{} of {traces}:\n{}",
            wrong.len(),
            wrong.join("\n")
        );
    }
}

/// A capsule the BVH stops at fraction 0 or starts all in solid ends
/// on whatever it clipped first, so it reads the contact retail's leaf
/// order meets first: on mp_depot a run into the wall where brushes 182
/// and 193 both stop it at 0 (leaf order: 193) and a step-down that
/// starts in brush 5285 after the floor's contact, whose normal it keeps;
/// on mp_railyard a run into two terrain triangles at 0. The
/// `leaf_order_census` sampler found them (bsp-ibsp59-format.md, "The
/// leaf walk").
#[test]
fn a_capsule_stopped_at_0_reads_the_first_in_leaf_order() {
    let Some(fs) = crate::testing::game_fs_11() else {
        return;
    };
    let (mins, maxs) = (Vec3::new(-15.0, -15.0, 0.0), Vec3::new(15.0, 15.0, 70.0));
    for (map, a, b) in [
        (
            "mp_depot",
            [-2682.3232, -1800.0657, -31.750011],
            [-2674.6855, -1785.2223, -31.750011],
        ),
        (
            "mp_depot",
            [978.2998, 1853.0566, 42.067444],
            [978.2998, 1853.0566, 15.067444],
        ),
        (
            "mp_railyard",
            [2404.5742, 4883.2344, -95.75002],
            [2411.0657, 4897.0728, -95.75002],
        ),
    ] {
        let bsp = crate::bsp::parse(&fs.read(&format!("maps/mp/{map}.bsp")).unwrap()).unwrap();
        let world = CollisionWorld::build(&bsp, &[]);
        let (a, b) = (Vec3::from(a), Vec3::from(b));
        let ours = world.trace_shape(a, b, mins, maxs, MASK_PLAYERSOLID, false, None, false);
        let retail = leaf_order_trace(&world, a, b, mins, maxs, false);
        assert_eq!(ours.fraction, 0.0, "{map} {a}");
        assert_eq!(
            (ours.hit, ours.normal, ours.surface_flags, ours.allsolid),
            (
                retail.hit,
                retail.normal,
                retail.surface_flags,
                retail.allsolid
            ),
            "{map} {a}"
        );
    }
}

/// `near_ties_read_as_retails_leaf_order`'s sampler over every stock
/// 1.1 map (`LEAF_MAPS` narrows it, `LEAF_SAMPLES` spots a map): each
/// difference from the leaf walk, as `in` (the walk enters an open
/// leaf) or `out` (only solid ones). Run in release.
#[test]
#[ignore]
fn leaf_order_census() {
    let Some(fs) = crate::testing::game_fs_11() else {
        return;
    };
    let maps = std::env::var("LEAF_MAPS").unwrap_or_else(|_| {
        "mp_brecourt mp_carentan mp_chateau mp_dawnville mp_depot mp_harbor mp_hurtgen \
         mp_pavlov mp_powcamp mp_railyard mp_rocket mp_ship"
            .into()
    });
    let samples: usize = std::env::var("LEAF_SAMPLES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(30000);
    let (mins, maxs) = (Vec3::new(-15.0, -15.0, 0.0), Vec3::new(15.0, 15.0, 70.0));
    let (mut total, mut zeros, mut wrong) = (0usize, 0usize, Vec::new());
    for map in maps.split_whitespace() {
        let bsp = crate::bsp::parse(&fs.read(&format!("maps/mp/{map}.bsp")).unwrap()).unwrap();
        let world = CollisionWorld::build(&bsp, &[]);
        let walk = world.leaf_walk.as_ref().unwrap();
        let (lo, hi) = crate::mesh::map_bounds(&bsp);
        let (lo, hi) = (Vec3::from(lo), Vec3::from(hi));
        let mut s = 0x2545F4914F6CDD1Du64;
        for k in 0..samples {
            let r = |s: &mut u64| Vec3::new(rng(s), rng(s), rng(s));
            let p = lo + (hi - lo) * r(&mut s);
            let down = world.move_trace(p, p - Vec3::Z * 2000.0, mins, maxs, MASK_PLAYERSOLID);
            if down.fraction >= 1.0 || down.startsolid {
                continue;
            }
            let o = down.endpos;
            let (a, b) = match k % 3 {
                0 => (o + Vec3::Z * 0.25, o - Vec3::Z * 0.25),
                1 => {
                    let run = (r(&mut s) - Vec3::splat(0.5)).with_z(0.0) * 40.0;
                    (o + Vec3::Z * 0.125, o + run + Vec3::Z * 0.125)
                }
                _ => (o + Vec3::Z * 18.0, o - Vec3::Z * 9.0),
            };
            let mid = (mins + maxs) * 0.5;
            let mut open = false;
            walk.visit(
                (a + mid).to_array(),
                (b + mid).to_array(),
                (maxs - mid).to_array(),
                false,
                1.0,
                |leaf, _| {
                    open |= bsp.leafs[leaf as usize].cluster >= 0;
                    1.0
                },
            );
            for boxed in [false, true] {
                let ours =
                    world.trace_shape(a, b, mins, maxs, MASK_PLAYERSOLID, false, None, boxed);
                let retail = leaf_order_trace(&world, a, b, mins, maxs, boxed);
                total += 1;
                zeros += usize::from(!boxed && ours.fraction <= 0.0);
                let key = |t: &Trace| {
                    (
                        t.fraction.to_bits(),
                        t.normal.to_array().map(f32::to_bits),
                        t.surface_flags,
                        t.startsolid,
                        t.allsolid,
                    )
                };
                if key(&ours) != key(&retail) {
                    wrong.push(format!(
                        "{} {map} {a} -> {b} boxed {boxed}: {:?} {:?} {} vs {:?} {:?} {}",
                        if open { "in" } else { "out" },
                        ours.hit,
                        ours.normal,
                        ours.fraction,
                        retail.hit,
                        retail.normal,
                        retail.fraction
                    ));
                }
            }
        }
    }
    println!(
        "{total} traces, {zeros} capsules stopped at 0, {} differ",
        wrong.len()
    );
    for w in &wrong {
        println!("  {w}");
    }
}

#[test]
fn ordered_walk_matches_brute_force_on_mp_pavlov() {
    let Some(data) = crate::testing::real_bsp() else {
        return;
    };
    let bsp = crate::bsp::parse(&data).unwrap();
    let world = CollisionWorld::build(&bsp, &[]);
    let (lo, hi) = crate::mesh::map_bounds(&bsp);
    // short sweeps so most of them hit something
    let mut s = 0x2545F4914F6CDD1Du64;
    let mut hits = 0;
    for _ in 0..200 {
        let r = |s: &mut u64| Vec3::new(rng(s), rng(s), rng(s));
        let start = Vec3::from(lo) + (Vec3::from(hi) - Vec3::from(lo)) * r(&mut s);
        let end = start + (r(&mut s) - Vec3::splat(0.5)) * 600.0;
        let a = world.box_trace(start, end, Vec3::ZERO, Vec3::ZERO);
        let b = brute_trace(&world, start, end, Vec3::ZERO, Vec3::ZERO);
        assert!(
            (a.fraction - b.fraction).abs() <= 1e-5,
            "{start} -> {end}: {a:?} vs {b:?}"
        );
        assert_eq!((a.startsolid, a.allsolid), (b.startsolid, b.allsolid));
        hits += (a.fraction < 1.0) as usize;
    }
    assert!(hits > 20, "{hits}");
}

/// Model 0: floor z -16..0 over +-1024. Model 1: a door brush local
/// (-8..8)^2 x 0..64 plus a stray render triangle at local x -60..-20,
/// placed by an entity at (200, 0, 0).
fn two_model_world(classname: &str) -> MapWorld {
    let dist = |v: f32| v.to_bits();
    let side = |v: f32| mapworld::BrushSide {
        plane_or_dist: dist(v),
        material: 0,
    };
    let mut brush_sides = Vec::new();
    let mut push_box = |lo: [f32; 3], hi: [f32; 3]| {
        for axis in 0..3 {
            brush_sides.push(side(lo[axis]));
            brush_sides.push(side(hi[axis]));
        }
    };
    push_box([-1024.0, -1024.0, -16.0], [1024.0, 1024.0, 0.0]);
    push_box([-8.0, -8.0, 0.0], [8.0, 8.0, 64.0]);
    MapWorld {
        materials: vec![mapworld::Material {
            name: "textures/test/solid".into(),
            surface_flags: 0,
            content_flags: 0x1,
        }],
        soups: vec![mapworld::TriangleSoup {
            material: 0,
            lightmap: mapworld::NO_LIGHTMAP,
            first_vertex: 0,
            vertex_count: 3,
            index_count: 3,
            first_index: 0,
        }],
        verts: vec![
            vert([-60.0, 0.0, 30.0]),
            vert([-20.0, 0.0, 30.0]),
            vert([-60.0, 40.0, 30.0]),
        ],
        indices: vec![0, 1, 2],
        entities: format!(
            "{{\n\"classname\" \"{classname}\"\n\"model\" \"*1\"\n\"origin\" \"200 0 0\"\n}}"
        ),
        brush_sides,
        brushes: vec![
            mapworld::Brush {
                first_side: 0,
                num_sides: 6,
                material: 0,
            },
            mapworld::Brush {
                first_side: 6,
                num_sides: 6,
                material: 0,
            },
        ],
        models: vec![
            mapworld::Model {
                mins: [-1024.0; 3],
                maxs: [1024.0; 3],
                first_soup: 1,
                num_soups: 0,
                first_brush: 0,
                partitions: (0, 0),
                num_brushes: 1,
            },
            mapworld::Model {
                mins: [-8.0, -8.0, 0.0],
                maxs: [8.0, 8.0, 64.0],
                first_soup: 0,
                num_soups: 1,
                first_brush: 1,
                partitions: (0, 0),
                num_brushes: 1,
            },
        ],
        ..Default::default()
    }
}

#[test]
fn submodel_brush_collides_at_its_entity_origin() {
    let world = CollisionWorld::build(&two_model_world("script_brushmodel"), &[]);
    let t = world.box_trace(
        Vec3::new(150.0, 0.0, 32.0),
        Vec3::new(250.0, 0.0, 32.0),
        Vec3::ZERO,
        Vec3::ZERO,
    );
    assert!(t.fraction < 1.0, "ray should hit the placed door: {t:?}");
    assert!(
        (t.endpos.x - 192.0).abs() < 1.0,
        "should stop at the door face, got {t:?}"
    );
}

/// `SV_Trace` clips the static models after the world and before the
/// entities (`cod_lnxded` 0x80916f4), each pass replacing only a nearer
/// fraction, so a prop face flush with a door's face names the prop.
#[test]
fn a_static_model_wins_a_tie_with_a_submodel() {
    let face = |x: f32| {
        let c = |y: f32, z: f32| Vec3::new(x, y, z);
        let mt = |tri| ModelTri {
            tri,
            contents: CONTENTS_SOLID,
            surface_flags: 21 << 20,
        };
        [
            mt([c(-8.0, 0.0), c(8.0, 64.0), c(8.0, 0.0)]),
            mt([c(-8.0, 0.0), c(-8.0, 64.0), c(8.0, 64.0)]),
        ]
    };
    let (start, end) = (Vec3::new(150.0, 0.0, 32.0), Vec3::new(250.0, 0.0, 32.0));
    let door = CollisionWorld::build(&two_model_world("script_brushmodel"), &[]);
    let alone = door.shot_trace(start, end);
    assert!(matches!(alone.hit, Some(Prim::Brush(_))), "{alone:?}");
    let world = CollisionWorld::build(&two_model_world("script_brushmodel"), &face(192.0));
    let t = world.shot_trace(start, end);
    assert_eq!(t.fraction, alone.fraction);
    assert!(matches!(t.hit, Some(Prim::Model(_))), "{t:?}");
    assert_eq!(sound_material(t.surface_flags), 21);
}

/// A pose moves a submodel's brushes with its entity: the door that
/// stood at x 192 stops the ray at 292 once its entity sits at 300, a
/// 45-degree yaw stands it on a corner, and the spawn pose puts it back.
#[test]
fn a_posed_submodel_clips_where_its_entity_is() {
    let world = CollisionWorld::build(&two_model_world("script_brushmodel"), &[]);
    let ray = |w: &CollisionWorld| {
        w.box_trace(
            Vec3::new(150.0, 0.0, 32.0),
            Vec3::new(350.0, 0.0, 32.0),
            Vec3::ZERO,
            Vec3::ZERO,
        )
        .endpos
        .x
    };
    world.set_model_pose(1, Vec3::new(300.0, 0.0, 0.0), Vec3::ZERO);
    assert!((ray(&world) - 292.0).abs() < 0.5, "{}", ray(&world));
    assert_ne!(world.point_contents(Vec3::new(300.0, 0.0, 32.0)), 0);
    assert_eq!(world.point_contents(Vec3::new(200.0, 0.0, 32.0)), 0);

    world.set_model_pose(1, Vec3::new(200.0, 0.0, 0.0), Vec3::new(0.0, 45.0, 0.0));
    let corner = 200.0 - 8.0 * std::f32::consts::SQRT_2;
    assert!((ray(&world) - corner).abs() < 0.5, "{}", ray(&world));

    world.set_model_pose(1, Vec3::new(200.0, 0.0, 0.0), Vec3::ZERO);
    assert!((ray(&world) - 192.0).abs() < 0.5, "{}", ray(&world));
}

/// Two entities' instances of one submodel each clip where they stand
/// and name their own entity; a trace passing one still meets the
/// other, and an instance whose contents miss the mask is not clipped.
#[test]
fn model_instances_clip_where_their_entities_stand() {
    let world = CollisionWorld::build(&two_model_world("script_brushmodel"), &[]);
    world.set_model_linked(1, false);
    let ray = |w: &CollisionWorld, skip: Option<Pusher>| {
        let t = w.trace_with_mask(
            Vec3::new(150.0, 0.0, 32.0),
            Vec3::new(450.0, 0.0, 32.0),
            Vec3::ZERO,
            Vec3::ZERO,
            TRACE_MASK_MOVE,
            false,
            skip,
        );
        (t.endpos.x, w.entity_num(&t))
    };
    assert_eq!(ray(&world, None).1, ENTITYNUM_NONE);
    world.set_model_instance(300, 1, Some((Vec3::new(250.0, 0.0, 0.0), Vec3::ZERO, 1)));
    world.set_model_instance(301, 1, Some((Vec3::new(400.0, 0.0, 0.0), Vec3::ZERO, 1)));
    let (x, e) = ray(&world, None);
    assert!((x - 242.0).abs() < 0.5 && e == 300, "{x} {e}");
    let (x, e) = ray(&world, Some(Pusher::Copy(300)));
    assert!((x - 392.0).abs() < 0.5 && e == 301, "{x} {e}");
    world.set_model_instance(300, 1, Some((Vec3::new(250.0, 0.0, 0.0), Vec3::ZERO, 0x4)));
    assert_eq!(ray(&world, None).1, 301, "contents off the mask");
    world.set_model_instance(301, 1, None);
    world.set_model_instance(300, 1, None);
    assert_eq!(ray(&world, None).1, ENTITYNUM_NONE);
}

/// A turned instance's brushes clip in lump order and a contact at
/// fraction 0 ends the model, as the leaf trace's loop does (UO
/// 0x8059875): a later brush holding the whole sweep never reads it all
/// solid. The jeep's collmap under a corpse its hull drove into
/// (coduo-vehicles.md 11.7).
#[test]
fn a_contact_at_zero_ends_a_turned_instance_s_brushes() {
    let mut bsp = two_model_world("script_brushmodel");
    for (lo, hi) in [(-6.0f32, 6.0), (-6.0, 6.0), (60.0, 90.0)] {
        for v in [lo, hi] {
            bsp.brush_sides.push(mapworld::BrushSide {
                plane_or_dist: v.to_bits(),
                material: 0,
            });
        }
    }
    bsp.brushes.push(mapworld::Brush {
        first_side: 12,
        num_sides: 6,
        material: 0,
    });
    bsp.models[1].num_brushes = 2;
    let world = CollisionWorld::build(&bsp, &[]);
    world.set_model_linked(1, false);
    let at = Vec3::new(500.0, 0.0, 0.0);
    world.set_model_instance(300, 1, Some((at, Vec3::new(0.0, 90.0, 0.0), 1)));
    // A capsule resting a twentieth over the first brush's top, inside
    // the second from start to end.
    let (mins, maxs) = (Vec3::new(-4.0, -4.0, 0.0), Vec3::new(4.0, 4.0, 16.0));
    let start = at + Vec3::new(0.0, 0.0, 64.05);
    let t = world.trace_with_mask(
        start,
        start - Vec3::Z * 0.5,
        mins,
        maxs,
        TRACE_MASK_MOVE,
        false,
        None,
    );
    assert!(!t.allsolid && !t.startsolid, "{t:?}");
    assert_eq!(t.fraction, 0.0);
    assert!((t.normal - Vec3::Z).length() < 1e-5, "{:?}", t.normal);
    assert_eq!(world.entity_num(&t), 300);
}

/// An unlinked instance clips nothing until it is put back where it
/// stood.
#[test]
fn an_unlinked_model_instance_clips_again_once_relinked() {
    let world = CollisionWorld::build(&two_model_world("script_brushmodel"), &[]);
    world.set_model_linked(1, false);
    let ray = |w: &CollisionWorld| {
        let t = w.trace_with_mask(
            Vec3::new(150.0, 0.0, 32.0),
            Vec3::new(450.0, 0.0, 32.0),
            Vec3::ZERO,
            Vec3::ZERO,
            TRACE_MASK_MOVE,
            false,
            None,
        );
        w.entity_num(&t)
    };
    world.set_model_instance(300, 1, Some((Vec3::new(250.0, 0.0, 0.0), Vec3::ZERO, 1)));
    assert!(world.unlink_model_instance(301).is_none());
    let unlinked = world.unlink_model_instance(300).unwrap();
    assert_eq!(ray(&world), ENTITYNUM_NONE);
    world.relink_model_instance(unlinked);
    assert_eq!(ray(&world), 300);
}

/// `model_box_trace` sees one model: the floor under the door is not in
/// it, the door is.
#[test]
fn a_model_trace_sees_only_that_model() {
    let world = CollisionWorld::build(&two_model_world("script_brushmodel"), &[]);
    let floor = Vec3::new(0.0, 0.0, -8.0);
    assert!(
        !world
            .model_box_trace(1, floor, floor, Vec3::ZERO, Vec3::ZERO, MASK_PLAYERSOLID)
            .startsolid
    );
    let door = Vec3::new(200.0, 0.0, 32.0);
    assert!(
        world
            .model_box_trace(1, door, door, Vec3::ZERO, Vec3::ZERO, MASK_PLAYERSOLID)
            .startsolid
    );
}

#[test]
fn trigger_submodels_do_not_collide() {
    let world = CollisionWorld::build(&two_model_world("trigger_multiple"), &[]);
    let t = world.box_trace(
        Vec3::new(150.0, 0.0, 32.0),
        Vec3::new(250.0, 0.0, 32.0),
        Vec3::ZERO,
        Vec3::ZERO,
    );
    assert_eq!(t.fraction, 1.0, "trigger brushes must stay hollow: {t:?}");
}

#[test]
fn submodel_render_triangles_leave_collision() {
    let world = CollisionWorld::build(&two_model_world("script_brushmodel"), &[]);
    // through the triangle's untranslated local spot: only the floor may stop this
    let t = world.box_trace(
        Vec3::new(-40.0, 20.0, 30.0),
        Vec3::new(-40.0, 20.0, -100.0),
        Vec3::ZERO,
        Vec3::ZERO,
    );
    assert!(
        (t.endpos.z - 0.0).abs() < 0.5,
        "should land on the floor, got {t:?}"
    );
}

fn water_world() -> MapWorld {
    let dist = |v: f32| v.to_bits();
    let side = |m: u32, v: f32| mapworld::BrushSide {
        plane_or_dist: dist(v),
        material: m,
    };
    let mut brush_sides = Vec::new();
    let mut brushes = Vec::new();
    for (m, lo, hi) in [
        (0u32, [-256.0, -256.0, -16.0], [256.0, 256.0, 0.0]),
        (1u32, [100.0, -100.0, -16.0], [300.0, 100.0, 36.0]),
    ] {
        for axis in 0..3 {
            brush_sides.push(side(m, lo[axis]));
            brush_sides.push(side(m, hi[axis]));
        }
        brushes.push(mapworld::Brush {
            first_side: (brushes.len() * 6) as u32,
            num_sides: 6,
            material: m as u16,
        });
    }
    MapWorld {
        materials: vec![
            mapworld::Material {
                name: "textures/test/solid".into(),
                surface_flags: 0,
                content_flags: 0x1,
            },
            mapworld::Material {
                name: "textures/common/water".into(),
                surface_flags: 0,
                content_flags: 0x20,
            },
        ],
        brush_sides,
        brushes,
        models: vec![mapworld::Model {
            mins: [-256.0; 3],
            maxs: [256.0; 3],
            first_soup: 0,
            num_soups: 0,
            first_brush: 0,
            partitions: (0, 0),
            num_brushes: 2,
        }],
        ..Default::default()
    }
}

#[test]
fn point_contents_reports_water() {
    let world = CollisionWorld::build(&water_world(), &[]);
    assert_ne!(
        world.point_contents(Vec3::new(200.0, 0.0, 10.0)) & CONTENTS_WATER,
        0,
        "inside the pool"
    );
    assert_eq!(
        world.point_contents(Vec3::new(200.0, 0.0, 60.0)) & CONTENTS_WATER,
        0,
        "above the surface"
    );
    assert_eq!(
        world.point_contents(Vec3::new(0.0, 0.0, -8.0)) & CONTENTS_WATER,
        0,
        "dry ground next to the pool"
    );
}

/// One flagged playerclip wall at x 50..54 spanning y -200..200, z 0..100.
fn ladder_wall_world(surface_flags: u32) -> MapWorld {
    let dist = |v: f32| v.to_bits();
    let side = |v: f32| mapworld::BrushSide {
        plane_or_dist: dist(v),
        material: 0,
    };
    MapWorld {
        materials: vec![mapworld::Material {
            name: "textures/common/ladder".into(),
            surface_flags,
            content_flags: 0x10000,
        }],
        brush_sides: vec![
            side(50.0),
            side(54.0),
            side(-200.0),
            side(200.0),
            side(0.0),
            side(100.0),
        ],
        brushes: vec![mapworld::Brush {
            first_side: 0,
            num_sides: 6,
            material: 0,
        }],
        models: vec![mapworld::Model {
            mins: [50.0, -200.0, 0.0],
            maxs: [54.0, 200.0, 100.0],
            first_soup: 0,
            num_soups: 0,
            first_brush: 0,
            partitions: (0, 0),
            num_brushes: 1,
        }],
        ..Default::default()
    }
}

#[test]
fn trace_reports_ladder_surface_flag() {
    let world = CollisionWorld::build(&ladder_wall_world(0x8), &[]);
    let t = world.box_trace(
        Vec3::new(0.0, 0.0, 50.0),
        Vec3::new(100.0, 0.0, 50.0),
        Vec3::new(-15.0, -15.0, 0.0),
        Vec3::new(15.0, 15.0, 60.0),
    );
    assert!(t.fraction < 1.0 && t.surface_flags & 0x8 != 0, "{t:?}");
    let plain = CollisionWorld::build(&ladder_wall_world(0x0), &[]);
    let t = plain.box_trace(
        Vec3::new(0.0, 0.0, 50.0),
        Vec3::new(100.0, 0.0, 50.0),
        Vec3::new(-15.0, -15.0, 0.0),
        Vec3::new(15.0, 15.0, 60.0),
    );
    assert!(t.fraction < 1.0 && t.surface_flags == 0, "{t:?}");
}

/// The sound surface rides the soup material's `surface_flags` bits 20-24,
/// which a trace must carry through triangle hits too.
#[test]
fn triangle_hits_carry_the_sound_material() {
    let mut bsp = tiny_world();
    bsp.materials[0].surface_flags = 6 << 20; // dirt
    let world = CollisionWorld::build(&bsp, &[]);
    // straight down through the z=10 triangle
    let t = world.box_trace(
        Vec3::new(150.0, 150.0, 30.0),
        Vec3::new(150.0, 150.0, -8.0),
        Vec3::ZERO,
        Vec3::ZERO,
    );
    assert!(t.fraction < 1.0 && t.normal.z > 0.9, "{t:?}");
    assert_eq!(super::sound_material(t.surface_flags), 6);
}

/// A brush trace reports the struck side's own material, not the
/// brush's (`cod_lnxded` 0x8055565, bsp-ibsp59-format.md lump 3): a
/// caulk brush with a plaster +y face reads plaster off that face and
/// nothing off its caulk top, for a point and a capsule.
#[test]
fn a_brush_hit_reports_the_struck_sides_material() {
    let mut bsp = synthetic_bsp(
        &[
            ("textures/common/caulk", CONTENTS_SOLID, 0x400a0),
            ("textures/test/plaster", CONTENTS_SOLID, 16 << 20),
        ],
        &[(0, [0.0, 0.0, 0.0], [64.0, 64.0, 64.0])],
    );
    bsp.brush_sides[3].material = 1;
    let world = CollisionWorld::build(&bsp, &[]);
    let face = |t: Trace| (t.fraction < 1.0).then(|| super::sound_material(t.surface_flags));
    let side = |mins: Vec3, maxs: Vec3| {
        world.box_trace(
            Vec3::new(32.0, 120.0, 32.0),
            Vec3::new(32.0, 32.0, 32.0),
            mins,
            maxs,
        )
    };
    assert_eq!(face(side(Vec3::ZERO, Vec3::ZERO)), Some(16));
    assert_eq!(face(side(Vec3::splat(-4.0), Vec3::splat(4.0))), Some(16));
    let capsule = world.box_trace(
        Vec3::new(32.0, 140.0, 0.0),
        Vec3::new(32.0, 32.0, 0.0),
        Vec3::new(-15.0, -15.0, 0.0),
        Vec3::new(15.0, 15.0, 70.0),
    );
    assert_eq!(face(capsule), Some(16));
    let top = world.box_trace(
        Vec3::new(32.0, 32.0, 120.0),
        Vec3::new(32.0, 32.0, 32.0),
        Vec3::ZERO,
        Vec3::ZERO,
    );
    assert_eq!(face(top), Some(0));
}

/// The BVH walk reports what a scan of every brush does, on and off
/// brushes: each prim's centre and corners, where the walk's node bounds
/// are tightest.
#[test]
fn point_contents_matches_a_scan_of_every_brush() {
    let Some(fs) = crate::testing::game_fs() else {
        return;
    };
    let bsp = crate::bsp::parse(&fs.read("maps/mp/mp_carentan.bsp").unwrap()).unwrap();
    let world = CollisionWorld::build(&bsp, &[]);
    let scan = |p: Vec3| {
        let water = world.water.iter().any(|v| {
            p.cmple(v.hi).all()
                && p.cmpge(v.lo).all()
                && v.planes.iter().all(|&(n, d)| n.dot(p) <= d)
        });
        world
            .prims
            .iter()
            .filter_map(|(prim, _, _)| match prim {
                Prim::Brush(b) => Some(&world.brushes[*b as usize]),
                _ => None,
            })
            .filter(|b| world.brush_linked(b) && b.planes.iter().all(|&(n, d)| n.dot(p) <= d))
            .fold(if water { CONTENTS_WATER } else { 0 }, |c, b| {
                c | b.content_flags
            })
    };
    let mut hits = 0;
    for &(_, lo, hi) in world.prims.iter().step_by(7) {
        for p in [(lo + hi) * 0.5, lo, hi, hi + Vec3::ONE] {
            let want = scan(p);
            hits += usize::from(want != 0);
            assert_eq!(world.point_contents(p), want, "at {p}");
        }
    }
    assert!(hits > 100, "only {hits} samples inside a brush");
}

/// mp_harbor carries both stock-MP water and ladders; both must survive
/// into the built world. Derived from the BSP itself, no hardcoded spots.
#[test]
fn harbor_water_volumes_and_ladder_flags_survive_build() {
    let Some(fs) = crate::testing::game_fs() else {
        return;
    };
    let data = crate::testing::stock_file(&fs, "maps/mp/mp_harbor.bsp");
    let bsp = crate::bsp::parse(&data).unwrap();

    let axial_bounds = |b: &crate::mapworld::Brush| -> ([f32; 3], [f32; 3]) {
        let sides = &bsp.brush_sides[b.first_side as usize..][..b.num_sides as usize];
        let mut lo = [0.0f32; 3];
        let mut hi = [0.0f32; 3];
        for axis in 0..3 {
            lo[axis] = f32::from_bits(sides[axis * 2].plane_or_dist);
            hi[axis] = f32::from_bits(sides[axis * 2 + 1].plane_or_dist);
        }
        (lo, hi)
    };

    let water = bsp
        .brushes
        .iter()
        .find(|b| bsp.materials[b.material as usize].content_flags & 0x20 != 0);
    let ladders: Vec<&crate::mapworld::Brush> = bsp
        .brushes
        .iter()
        .filter(|b| bsp.materials[b.material as usize].surface_flags & 0x8 != 0)
        .collect();
    let (Some(water), Some(_)) = (water, ladders.first()) else {
        panic!("census says harbor has water and ladders");
    };

    let world = CollisionWorld::build(&bsp, &[]);

    let (wlo, whi) = axial_bounds(water);
    let mid = Vec3::new(
        (wlo[0] + whi[0]) * 0.5,
        (wlo[1] + whi[1]) * 0.5,
        (wlo[2] + whi[2]) * 0.5,
    );
    assert_ne!(
        world.point_contents(mid) & CONTENTS_WATER,
        0,
        "water brush interior at {mid}"
    );
    assert_eq!(
        world.point_contents(mid + Vec3::Z * (whi[2] - wlo[2] + 8.0)) & CONTENTS_WATER,
        0,
        "above the water surface"
    );

    // Ladder volumes sit inside staircases and walls; probe each from a
    // few offsets along its thinnest axis until one reports the flag.
    let player_box = (Vec3::new(-15.0, -15.0, 0.0), Vec3::new(15.0, 15.0, 60.0));
    let mut flagged = false;
    for ladder in &ladders {
        let (llo, lhi) = axial_bounds(ladder);
        let ex = lhi[0] - llo[0];
        let ey = lhi[1] - llo[1];
        let center = Vec3::new(
            (llo[0] + lhi[0]) * 0.5,
            (llo[1] + lhi[1]) * 0.5,
            ((llo[2] + lhi[2]) * 0.5).min(llo[2] + 40.0),
        );
        let dir = if ex < ey { Vec3::X } else { Vec3::Y };
        let span = (if ex < ey { ex } else { ey }) * 0.5;
        for back in [24.0, 40.0, 64.0] {
            for sign in [-1.0, 1.0] {
                let t = world.box_trace(
                    center - dir * sign * (span + back),
                    center,
                    player_box.0,
                    player_box.1,
                );
                if !t.startsolid && t.fraction < 1.0 && t.surface_flags & 0x8 != 0 {
                    flagged = true;
                }
            }
        }
    }
    assert!(flagged, "no approach reported the ladder flag");
}

/// Axial-free test helper: one triangle patch per entry against named
/// `(name, contents, surface)` materials. Model 0 only, no brushes.
fn synthetic_patch_world(
    materials: &[(&str, u32, u32)],
    tris: &[(usize, [[f32; 3]; 3])],
) -> CollisionWorld {
    let mut bsp = MapWorld {
        materials: materials
            .iter()
            .map(|(name, content, surface)| crate::mapworld::Material {
                name: name.to_string(),
                surface_flags: *surface,
                content_flags: *content,
            })
            .collect(),
        models: vec![crate::mapworld::Model {
            mins: [0.0; 3],
            maxs: [0.0; 3],
            first_soup: 0,
            num_soups: 0,
            first_brush: 0,
            partitions: (0, 0),
            num_brushes: 0,
        }],
        ..Default::default()
    };
    for (mat, tri) in tris {
        push_tri_patches(&mut bsp, &[tri.map(Vec3::from_array)], *mat as u16);
    }
    CollisionWorld::build(&bsp, &[])
}

/// Patch content words from the stock maps' lump 24: foliage-like
/// TRANSLUCENT|WINDOW (a terrain word; no patch carries it, so it stands
/// for any word outside every mask) and a masked fence's
/// TRANSLUCENT|PLAYERCLIP|MONSTERCLIP.
const BUSH_WALL: (&str, u32, u32) = (
    "textures/global_use/foliage_masked@bushwall1",
    0x2000_0002,
    8_454_176,
);
const IRON_FENCE: (&str, u32, u32) = (
    "textures/austria/transparents/metal_masked@ironfence1a",
    0x2003_0000,
    13_713_440,
);

/// A wall facing -x, toward a trace from the origin.
fn wall_tri(x: f32) -> [[f32; 3]; 3] {
    [[x, -50.0, 0.0], [x, -50.0, 100.0], [x, 50.0, 0.0]]
}

#[test]
fn a_patch_outside_every_mask_clips_nothing() {
    let world = synthetic_patch_world(&[BUSH_WALL], &[(0, wall_tri(100.0))]);
    assert_eq!(world.patches.len(), 1);
    let (start, end) = (Vec3::new(0.0, 0.0, 50.0), Vec3::new(200.0, 0.0, 50.0));
    assert_eq!(
        world.box_trace(start, end, Vec3::ZERO, Vec3::ZERO).fraction,
        1.0
    );
    assert_eq!(world.shot_trace(start, end).fraction, 1.0);
}

#[test]
fn a_playerclip_fence_patch_stops_movement_but_not_shots() {
    let world = synthetic_patch_world(&[IRON_FENCE], &[(0, wall_tri(100.0))]);
    let (start, end) = (Vec3::new(0.0, 0.0, 50.0), Vec3::new(200.0, 0.0, 50.0));
    let box_mins = Vec3::new(-15.0, -15.0, 0.0);
    let box_maxs = Vec3::new(15.0, 15.0, 60.0);
    let movement = world.box_trace(start, end, box_mins, box_maxs);
    assert!(movement.fraction < 1.0, "players are stopped: {movement:?}");
    let shot = world.shot_trace(start, end);
    assert_eq!(shot.fraction, 1.0, "bullets pass playerclip-only geometry");
}

/// A patch collides by its curve, not its control points: a 64 x 64
/// arch whose middle control row stands 64 up peaks at 32, and its box
/// (what the leaf walk culls by) is the subdivided grid's grown by a
/// unit, not the control points' (`cod_lnxded` 0x804e26d-0x804e314).
#[test]
fn a_curved_patch_collides_and_is_culled_by_its_curve() {
    let mut bsp = synthetic_bsp(&[("textures/test/solid", CONTENTS_SOLID, 0)], &[]);
    bsp.collision_verts = (0..3)
        .flat_map(|j| {
            (0..3).map(move |i| [i as f32 * 32.0, j as f32 * 32.0, (i % 2) as f32 * 64.0])
        })
        .collect();
    bsp.patches.push(crate::mapworld::PatchPart {
        material: 0,
        width: 3,
        height: 3,
        tolerance: 4,
        first_vert: 0,
    });
    let world = CollisionWorld::build(&bsp, &[]);
    let box_ = &world.patches[0].collide;
    assert_eq!((box_.mins, box_.maxs), ([-1.0; 3], [65.0, 65.0, 33.0]));

    // A shot down the crown stops on it, one that ends 1.5 over it never
    // reaches the clip, and a player walks over it with his feet above.
    let shot = world.shot_trace(Vec3::new(32.0, 32.0, 60.0), Vec3::new(32.0, 32.0, 20.0));
    assert!((shot.endpos.z - 32.125).abs() < 0.01, "{shot:?}");
    let short = world.shot_trace(Vec3::new(32.0, 32.0, 60.0), Vec3::new(32.0, 32.0, 33.5));
    assert_eq!(short.fraction, 1.0);
    let (mins, maxs) = (Vec3::new(-15.0, -15.0, 0.0), Vec3::new(15.0, 15.0, 70.0));
    let over = world.box_trace(
        Vec3::new(-40.0, 32.0, 34.0),
        Vec3::new(104.0, 32.0, 34.0),
        mins,
        maxs,
    );
    assert_eq!(over.fraction, 1.0, "{over:?}");
    let into = world.box_trace(
        Vec3::new(-40.0, 32.0, 20.0),
        Vec3::new(104.0, 32.0, 20.0),
        mins,
        maxs,
    );
    assert!(into.fraction < 1.0, "{into:?}");
}

#[test]
fn a_solid_patch_stops_movement_and_shots_from_its_front() {
    let world = synthetic_patch_world(
        &[("textures/test/solid", 0x1, 5 << 20)],
        &[(0, wall_tri(100.0))],
    );
    let (start, end) = (Vec3::new(0.0, 0.0, 50.0), Vec3::new(200.0, 0.0, 50.0));
    let t = world.box_trace(start, end, Vec3::ZERO, Vec3::ZERO);
    assert!(t.fraction < 1.0, "movement stops");
    assert_eq!(world.hit_contents(&t), 0x1);
    assert_eq!(sound_material(t.surface_flags), 5);
    assert!(world.shot_trace(start, end).fraction < 1.0, "shots stop");
    assert_eq!(
        world.shot_trace(end, start).fraction,
        1.0,
        "the back is open"
    );
}

/// The 1.5b turret round `turret_ab` replays (`mp_carentan-dm-turret-15.txt`
/// at 30000) and a ray beside it, under retail's own `bulletTrace`
/// (`client-probes/probe_patchshot`, 2026-10-16): 1.1d stops the round
/// on patch 136, where it meets patch 133, and lets the other through;
/// 1.5b and UO 1.51b let the round through a crack along that facet
/// seam to the wall behind and stop the other.
#[test]
fn the_15_turret_round_slips_a_seam_1_1_closes() {
    let Some(fs) = crate::testing::game_fs() else {
        return;
    };
    let data = crate::testing::stock_file(&fs, "maps/mp/mp_carentan.bsp");
    let bsp = crate::bsp::parse(&data).unwrap();
    let world = CollisionWorld::build(&bsp, &[]);
    let muzzle = Vec3::new(1706.721, 1822.8395, 28.679152);
    let end = Vec3::new(-3318.5461, -4644.5234, -140.95471);
    let up = Vec3::new(0.0, 0.0, 0.1);
    let shot = |s, e| world.shot_trace(s, e).fraction;
    assert!((shot(muzzle, end) - 0.0633128).abs() < 1e-6);
    assert!((shot(muzzle + up, end + up) - 0.111953).abs() < 1e-6);
    world.set_arith(Arith::Stored);
    assert!((shot(muzzle, end) - 0.111953).abs() < 1e-6);
    assert!((shot(muzzle + up, end + up) - 0.0633132).abs() < 1e-6);
}

/// Retail's own `bulletTrace` along 790 segments through mp_carentan's
/// patches (`tests/fixtures/patch/mp_carentan-seams.txt`): 1.1d's
/// answers off the 1.1 build, 1.5b's off the stored one. The fraction
/// and its distance from the line's base, as the gsc's floats work them
/// out and print them to six digits, tell two fractions a bit apart.
#[test]
fn patch_seams_clip_as_each_retail_build() {
    let Some(fs) = crate::testing::game_fs() else {
        return;
    };
    let data = crate::testing::stock_file(&fs, "maps/mp/mp_carentan.bsp");
    let bsp = crate::bsp::parse(&data).unwrap();
    // The probe ran as a dm gametype, and a round meets the props too.
    let world = CollisionWorld::build(&bsp, &crate::props::collision_tris(&fs, &bsp.entities));
    world.unlink_script_brushes(&bsp.entities, "dm");
    let text = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/patch/mp_carentan-seams.txt"
    ))
    .unwrap();
    // Six significant digits, as `%g` prints them.
    let close = |ours: f32, printed: f32| {
        (ours - printed).abs() <= 6e-6 * printed.abs().max(ours.abs()) + 1e-6
    };
    let mut bad = Vec::new();
    let mut lines = 0;
    for line in text.lines().filter(|l| !l.starts_with('#')) {
        let v: Vec<f32> = line.split(' ').map(|w| w.parse().unwrap()).collect();
        let (start, end) = (Vec3::new(v[1], v[2], v[3]), Vec3::new(v[4], v[5], v[6]));
        for (arith, f, d) in [(Arith::Kept, v[8], v[9]), (Arith::Stored, v[10], v[11])] {
            world.set_arith(arith);
            let ours = world.shot_trace(start, end).fraction;
            let delta = (ours - v[7]) * 1_000_000.0;
            if !close(ours, f) || !close(delta, d) {
                bad.push(format!(
                    "{} {arith:?}: ours {ours} ({delta}), retail {f} ({d})",
                    v[0]
                ));
            }
        }
        lines += 1;
    }
    assert_eq!(lines, 790);
    assert!(
        bad.is_empty(),
        "{} of 1580 off retail:\n{}",
        bad.len(),
        bad.join("\n")
    );
}

/// Retail's own `bulletTrace` and a falling weapon item straight down
/// over patches the leaves under them do not list, and over patches a
/// submodel owns (`tests/fixtures/patch/leafgate.txt`): a capsule meets
/// model 0's patches only from the leaves that list them, as a point
/// does, and a submodel's only while its entity is linked.
#[test]
fn lump_24_clips_from_its_leaves_or_as_its_submodel() {
    let Some(fs) = crate::testing::game_fs() else {
        return;
    };
    assert_eq!(check_leaf_fixture(&fs, Some("dm"), "leafgate.txt"), 23);
}

/// mp_arnhem's dom props are submodels whose clip is terrain alone: a
/// retail UO bulletTrace stops on their tops
/// (`tests/fixtures/patch/domprops-uo.txt`, bsp-ibsp59-format.md "The
/// leaf walk").
#[test]
fn a_submodel_of_terrain_clips_at_its_entity() {
    let Some(fs) = crate::testing::uo_fs() else {
        return;
    };
    assert_eq!(check_leaf_fixture(&fs, None, "domprops-uo.txt"), 4);
}

/// Retail UO's `bulletTrace` and a falling weapon item straight down
/// over terrain partitions no leaf lists
/// (`tests/fixtures/patch/terrainleaf-uo.txt`): neither clips them, as
/// neither clips an unlisted patch. No stock 1.1 map has such a
/// partition with a triangle (`unlisted_lump_24_on_stock_1_1_maps`).
#[test]
fn unlisted_terrain_never_clips() {
    let Some(fs) = crate::testing::uo_fs() else {
        return;
    };
    assert_eq!(
        check_leaf_fixture(&fs, Some("dm"), "terrainleaf-uo.txt"),
        12
    );
}

/// A terrain partition the leaves list clips only a trace whose walk
/// enters one of them: retail 1.1d's bulletTrace and falling item from
/// over mp_powcamp's terrain past its playable walls, where the walk
/// enters only solid leaves, pass it, where the BVH stopped both on it
/// (`tests/fixtures/patch/outterrain.txt`, bsp-ibsp59-format.md "The
/// leaf walk").
#[test]
fn terrain_out_of_the_world_never_clips() {
    let Some(fs) = crate::testing::game_fs_11() else {
        return;
    };
    assert_eq!(check_leaf_fixture(&fs, Some("dm"), "outterrain.txt"), 12);
}

/// A patch facet stops a trace only from in front of its surface:
/// retail UO's bulletTrace through mp_streets' arch corner 391 (x 292,
/// facing +x) from behind passes it and stops on its mirror 393 (x 284,
/// facing -x), as ours does (`tests/fixtures/patch/patchside-uo.txt`,
/// bsp-ibsp59-format.md "The leaf walk").
#[test]
fn a_patch_stops_traces_from_its_front_only() {
    let Some(fs) = crate::testing::uo_fs() else {
        return;
    };
    let text = std::fs::read_to_string(format!(
        "{}/tests/fixtures/patch/patchside-uo.txt",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap();
    let bsp = crate::bsp::parse(&fs.read("maps/mp/mp_streets.bsp").unwrap()).unwrap();
    let world = CollisionWorld::build(&bsp, &crate::props::collision_tris(&fs, &bsp.entities));
    world.set_arith(Arith::Stored);
    let mut checked = 0;
    for line in text.lines().filter(|l| !l.starts_with('#')) {
        let w: Vec<&str> = line.split(' ').collect();
        let v = |i: usize| w[i].parse::<f32>().unwrap();
        let start = Vec3::new(v(1), v(2), v(3));
        let end = start + Vec3::X * if w[4] == "front" { -40.0 } else { 40.0 };
        let t = world.shot_trace(start, end);
        assert!(
            t.fraction == v(5) && (t.endpos.x - v(6)).abs() <= 0.01 && t.normal.x == v(7),
            "{line}: ours {} at {} normal {}",
            t.fraction,
            t.endpos.x,
            t.normal.x
        );
        checked += 1;
    }
    assert_eq!(checked, 6);
}

/// 1.5's mp_neuville: its patches 68 and 69, which no leaf lists, stand
/// in open air, and retail 1.5b's bulletTrace passes them
/// (`tests/fixtures/patch/neuvilleleaf-15.txt`).
#[test]
fn neuvilles_unlisted_patches_never_clip() {
    let Some(fs) = crate::testing::game_fs() else {
        return;
    };
    if fs.read("maps/mp/mp_neuville.bsp").is_none() {
        return;
    }
    assert_eq!(
        check_leaf_fixture(&fs, Some("dm"), "neuvilleleaf-15.txt"),
        6
    );
}

/// 1.5's mp_neuville: `placeSpawnpoint`'s ceiling check over entity 90
/// meets caulk brush 2647's pushed-out underside past its roof slope's
/// unbevelled edge, 40 units off the brush, where no leaf lists it.
/// Retail 1.5b's drop starts the full 128 up
/// (`tests/fixtures/spawnpoints/placebits-retail.txt`).
#[test]
fn a_brush_no_leaf_on_the_walk_lists_never_clips() {
    let Some(fs) = crate::testing::game_fs() else {
        return;
    };
    let Some(data) = fs.read("maps/mp/mp_neuville.bsp") else {
        return;
    };
    let world = CollisionWorld::build(&crate::bsp::parse(&data).unwrap(), &[]);
    let start = Vec3::new(-14688.0, 5072.0, 8.0);
    let (mins, maxs) = (Vec3::new(-15.0, -15.0, 0.0), Vec3::new(15.0, 15.0, 70.0));
    let up = world.box_trace(start, start + Vec3::Z * 128.0, mins, maxs);
    assert_eq!((up.fraction, up.hit), (1.0, None));
}

/// Every model-0 lump-24 partition no leaf lists on the stock 1.1
/// maps: mp_brecourt's terrain record 385 (five points, no triangle)
/// and mp_carentan's four patches.
#[test]
fn unlisted_lump_24_on_stock_1_1_maps() {
    let Some(fs) = crate::testing::game_fs_11() else {
        return;
    };
    let mut found = Vec::new();
    for map in [
        "mp_brecourt",
        "mp_carentan",
        "mp_chateau",
        "mp_dawnville",
        "mp_depot",
        "mp_harbor",
        "mp_hurtgen",
        "mp_pavlov",
        "mp_powcamp",
        "mp_railyard",
        "mp_rocket",
        "mp_ship",
    ] {
        let bsp = crate::bsp::parse(&fs.read(&format!("maps/mp/{map}.bsp")).unwrap()).unwrap();
        let listed = LeafWalk::listed(&bsp).unwrap();
        let (terrain, patches) = partition_owners(&bsp);
        for (i, t) in bsp.terrain.iter().enumerate() {
            if terrain[i] == 0 && !listed.terrain[i] {
                found.push(format!("{map} terrain {i} indices {}", t.index_count));
            }
        }
        for (i, (&owner, &on)) in patches.iter().zip(&listed.patches).enumerate() {
            if owner == 0 && !on {
                found.push(format!("{map} patch {i}"));
            }
        }
    }
    assert_eq!(
        found,
        [
            "mp_brecourt terrain 385 indices 0",
            "mp_carentan patch 69",
            "mp_carentan patch 109",
            "mp_carentan patch 283",
            "mp_carentan patch 284",
        ]
    );
}

/// Replays a `tests/fixtures/patch/` file of retail point and item
/// traces straight down on ours; the number of lines checked.
/// `gametype` names the `_gameobjects` pass the run had (`None`: none,
/// every brush model linked).
fn check_leaf_fixture(fs: &crate::pk3::Pk3Fs, gametype: Option<&str>, file: &str) -> usize {
    let text = std::fs::read_to_string(format!(
        "{}/tests/fixtures/patch/{file}",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap();
    let mut worlds = std::collections::HashMap::new();
    let mut checked = 0;
    for line in text.lines().filter(|l| !l.starts_with('#')) {
        let w: Vec<&str> = line.split(' ').collect();
        let data = fs.read(&format!("maps/mp/{}.bsp", w[0])).unwrap();
        let world = worlds.entry(w[0]).or_insert_with(|| {
            let bsp = crate::bsp::parse(&data).unwrap();
            let world =
                CollisionWorld::build(&bsp, &crate::props::collision_tris(fs, &bsp.entities));
            if let Some(g) = gametype {
                world.unlink_script_brushes(&bsp.entities, g);
            }
            // mp_ship.gsc deletes the docks outside `re`.
            for block in crate::bsp::entity_blocks(&bsp.entities) {
                if block.get("targetname").is_some_and(|t| t == "dock")
                    && let Some(n) = block
                        .get("model")
                        .and_then(|m| m.strip_prefix('*'))
                        .and_then(|n| n.parse().ok())
                {
                    world.set_model_linked(n, false);
                }
            }
            world
        });
        let v = |i: usize| w[i].parse::<f32>().unwrap();
        let start = Vec3::new(v(1), v(2), v(3));
        let end = start - Vec3::Z * 400.0;
        if w[4] == "west" {
            let t = world.shot_trace(start, start - Vec3::X * 80.0);
            assert!(
                (t.fraction - v(5)).abs() <= 1e-6 && (t.endpos.x - v(6)).abs() <= 0.01,
                "{line}: ours {} at {}",
                t.fraction,
                t.endpos.x
            );
        } else if w[4] == "point" {
            let t = world.shot_trace(start, end);
            assert!(
                (t.fraction - v(5)).abs() <= 1e-6 && (t.endpos.z - v(6)).abs() <= 0.01,
                "{line}: ours {} at {}",
                t.fraction,
                t.endpos.z
            );
        } else {
            let t = world.item_trace(start, end, Vec3::splat(-1.0), Vec3::splat(1.0), 0x491);
            // A landing lifts the item by up to a unit; one that falls
            // through keeps falling past the sweep's end.
            let z = v(5);
            let ok = if z < end.z {
                t.fraction == 1.0
            } else {
                (t.endpos.z..=t.endpos.z + 1.0).contains(&z)
            };
            assert!(ok, "{line}: ours {} at {}", t.fraction, t.endpos.z);
        }
        checked += 1;
    }
    checked
}

/// A point sliding down a hull's side onto its deck: the side's
/// backed-off fraction is under the deck's, but the deck's surface comes
/// first and the side's bounds start past it, so the posed clip stops on
/// the deck (coduo-vehicles.md 13.3, "A blast on a hull").
#[test]
fn a_later_surface_past_the_best_fraction_is_not_clipped() {
    let tri = |a: [f32; 3], b: [f32; 3], c: [f32; 3]| ModelTri {
        tri: [Vec3::from(a), Vec3::from(b), Vec3::from(c)],
        contents: 1,
        surface_flags: 0,
    };
    // The deck faces +z at z 0, the side +y at y 0 below it; each
    // surface's bounds a unit deep behind its face.
    let tris = [
        tri([-10.0, -10.0, 0.0], [10.0, -10.0, 0.0], [0.0, 10.0, 0.0]),
        tri([-10.0, 0.0, -10.0], [0.0, 0.0, 10.0], [10.0, 0.0, -10.0]),
    ];
    let surf = |i: usize, mins: [f32; 3], maxs: [f32; 3]| ModelSurf {
        tris: i..i + 1,
        rot: glam::Quat::IDENTITY,
        pos: Vec3::ZERO,
        mins: Vec3::from(mins),
        maxs: Vec3::from(maxs),
    };
    let surfs = [
        surf(0, [-10.0, -10.0, -1.0], [10.0, 10.0, 0.0]),
        surf(1, [-10.0, -1.0, -10.0], [10.0, 0.0, 10.0]),
    ];
    let (start, end) = (Vec3::new(0.0, 0.1, 1.0), Vec3::new(0.0, -0.4, -5.0));
    let whole = clip_model_tris(start, end, &tris, 1, 1.0).unwrap();
    assert_eq!(whole.1.y, 1.0, "every triangle at once: the side");
    let posed = clip_model_surfs(start, end, &tris, &surfs, 1, 1.0).unwrap();
    assert_eq!(posed.1.z, 1.0, "a surface at a time: the deck");
    assert!((posed.0 - (1.0 - 0.125) / 6.0).abs() < 1e-6, "{}", posed.0);
}
