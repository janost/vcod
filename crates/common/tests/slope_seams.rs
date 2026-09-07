//! A walker on a terrain mesh at a high-fps client's cmd rate: the convex
//! seams and kerbs of a triangle soup, and a wall the slope rises toward.
//! What each of these used to do to the ground trace, and why the sight
//! ramp reversed with it, is in `docs/research/cod11-mantle.md`, "The
//! ground snap" and "What the collider does to a walker on a terrain seam".

use glam::Vec3;
use vcod_common::collision::{synthetic_world_tris, CollisionWorld, CONTENTS_SOLID};
use vcod_common::pmove::{pmove, PlayerState, PmInput};

const MINS: Vec3 = Vec3::new(-15.0, -15.0, 0.0);
const MAXS: Vec3 = Vec3::new(15.0, 15.0, 70.0);
/// A 125 fps client's cmd interval.
const DT: f32 = 0.008;

fn quad(a: Vec3, b: Vec3, c: Vec3, d: Vec3) -> [[Vec3; 3]; 2] {
    [[a, b, c], [a, c, d]]
}

/// `test_world`'s floor with a 15 degree ramp rising along +x from 0 to 400
/// and a wall brush across it at x 200..230, so a walker heading 60 degrees
/// off +x presses into the wall while it slides along it: the slope rises
/// toward the wall, the way a cambered street rises toward its kerb.
fn ramp_with_wall() -> CollisionWorld {
    let h = 400.0 * 15.0f32.to_radians().tan();
    let mut tris = Vec::new();
    tris.extend(quad(
        Vec3::new(0.0, -512.0, 0.0),
        Vec3::new(400.0, -512.0, h),
        Vec3::new(400.0, 512.0, h),
        Vec3::new(0.0, 512.0, 0.0),
    ));
    synthetic_world_tris(
        &[("textures/test/solid", CONTENTS_SOLID, 0)],
        &[
            (0, [-1024.0, -1024.0, -16.0], [1024.0, 1024.0, 0.0]),
            (0, [200.0, -512.0, -16.0], [230.0, 512.0, 300.0]),
        ],
        &tris,
    )
}

/// A convex seam between two rising facets, 20 degrees then 8, with the
/// seam running diagonally so the steeper facet's highest vertex is well
/// above the crossing: a box just past the seam is inside that facet's
/// box-expanded slab and under its top, which is what a terrain mesh does
/// at every ridge. The seam crosses y = 0 at x = 300.
fn convex_seam() -> CollisionWorld {
    let a = |x: f32| x * 20.0f32.to_radians().tan();
    let b = |x: f32, y: f32| 0.14 * x - 0.04375 * y + 67.2;
    let mut tris = Vec::new();
    tris.extend(quad(
        Vec3::new(0.0, -512.0, 0.0),
        Vec3::new(400.0, -512.0, a(400.0)),
        Vec3::new(200.0, 512.0, a(200.0)),
        Vec3::new(0.0, 512.0, 0.0),
    ));
    tris.extend(quad(
        Vec3::new(400.0, -512.0, b(400.0, -512.0)),
        Vec3::new(1024.0, -512.0, b(1024.0, -512.0)),
        Vec3::new(1024.0, 512.0, b(1024.0, 512.0)),
        Vec3::new(200.0, 512.0, b(200.0, 512.0)),
    ));
    synthetic_world_tris(
        &[("textures/test/solid", CONTENTS_SOLID, 0)],
        &[(0, [-1024.0, -1024.0, -16.0], [1024.0, 1024.0, 0.0])],
        &tris,
    )
}

/// The floor with an 8-unit kerb made of triangles from x = 50 on: a top
/// quad and a vertical face, the way a soup kerb arrives.
fn soup_kerb() -> CollisionWorld {
    let mut tris = Vec::new();
    tris.extend(quad(
        Vec3::new(50.0, -200.0, 8.0),
        Vec3::new(1024.0, -200.0, 8.0),
        Vec3::new(1024.0, 200.0, 8.0),
        Vec3::new(50.0, 200.0, 8.0),
    ));
    tris.extend(quad(
        Vec3::new(50.0, -200.0, 0.0),
        Vec3::new(50.0, -200.0, 8.0),
        Vec3::new(50.0, 200.0, 8.0),
        Vec3::new(50.0, 200.0, 0.0),
    ));
    synthetic_world_tris(
        &[("textures/test/solid", CONTENTS_SOLID, 0)],
        &[(0, [-1024.0, -1024.0, -16.0], [1024.0, 1024.0, 0.0])],
        &tris,
    )
}

/// Past the seam the box is inside the steeper facet's slab, 2.7 units under
/// its plane at 12 units along. That is a contact with the facet it stands
/// on, not a solid start.
#[test]
fn a_box_past_a_convex_seam_rests_on_the_facet_under_it() {
    let w = convex_seam();
    let start = Vec3::new(312.0, 0.0, 0.14 * 312.0 + 67.2 + 0.125);
    let down = w.box_trace(start, start - Vec3::Z * 9.0, MINS, MAXS);
    assert!(
        !down.startsolid && !down.allsolid,
        "the steeper facet's slab read as solid: {down:?}"
    );
    assert!(down.fraction < 0.05, "no floor under the box: {down:?}");
    assert!(down.normal.z > 0.98, "{down:?}");
    let along = w.box_trace(start, start + Vec3::new(2.0, 0.0, 0.28), MINS, MAXS);
    assert_eq!(along.fraction, 1.0, "the facet is free to walk: {along:?}");
}

/// Beside a kerb the box is 8 units under the kerb top's slab and inside
/// its bevels. The floor under the box is what the down trace reports.
#[test]
fn a_box_beside_a_soup_kerb_finds_the_floor_under_it() {
    let w = soup_kerb();
    let start = Vec3::new(40.0, 0.0, 0.125);
    let down = w.box_trace(start, start - Vec3::Z * 0.25, MINS, MAXS);
    assert!(
        !down.startsolid && !down.allsolid,
        "the kerb top's slab read as solid: {down:?}"
    );
    assert!(
        down.fraction < 0.05 && down.normal.abs_diff_eq(Vec3::Z, 1e-3),
        "{down:?}"
    );
    // And the kerb face still stops a walk into it.
    let into = w.box_trace(start, start + Vec3::X * 2.0, MINS, MAXS);
    assert!(
        into.fraction < 1.0 && into.normal.abs_diff_eq(-Vec3::X, 1e-3),
        "{into:?}"
    );
}

fn walk(w: &CollisionWorld, start: Vec3, yaw: f32, frames: usize) -> (PlayerState, usize) {
    let mut ps = PlayerState::spawn(start, yaw);
    for _ in 0..80 {
        pmove(&mut ps, &PmInput::default(), w, 0.016, &[]);
    }
    assert!(ps.on_ground, "the walker never settled at {}", ps.origin);
    let forward = PmInput {
        forward: 1.0,
        ..Default::default()
    };
    let mut airborne = 0;
    for _ in 0..frames {
        pmove(&mut ps, &forward, w, DT, &[]);
        airborne += usize::from(!ps.on_ground);
    }
    (ps, airborne)
}

/// A walker pressing into a wall the slope rises toward: every slide keeps
/// the climb's upward component and loses the forward one, which is upward
/// velocity into the ground plane. Retail's snap runs regardless and clips
/// it out; a snap gated on that velocity refused every frame and the ground
/// trace dropped the walker.
#[test]
fn a_wall_rubbing_walk_stays_on_the_ground_at_8_ms() {
    let w = ramp_with_wall();
    let (ps, airborne) = walk(&w, Vec3::new(120.0, -400.0, 40.0), 60.0, 550);
    assert!(ps.origin.y > 250.0, "the walk stalled at {}", ps.origin);
    assert_eq!(
        airborne, 0,
        "left the ground {airborne} frames, ending at {}",
        ps.origin
    );
}

/// The ridge at a high-fps client's cmd rate: the seam is crossed in small
/// steps, each of which used to read the steeper facet's slab as solid.
#[test]
fn a_convex_seam_keeps_the_ground_at_8_ms() {
    let w = convex_seam();
    let (ps, airborne) = walk(&w, Vec3::new(-20.0, 0.0, 1.0), 0.0, 500);
    assert!(
        ps.origin.x > 340.0,
        "the walk never crossed the seam, at {}",
        ps.origin
    );
    assert_eq!(
        airborne, 0,
        "left the ground {airborne} frames, ending at {}",
        ps.origin
    );
}
