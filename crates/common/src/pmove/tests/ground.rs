//! The ground trace, the all-solid nudge and the water level.

use super::*;

/// `probe_blastloop`'s slot 3 on retail (mantle doc, "A start inside a
/// solid"): set down nine units inside a clip brush it stays exactly
/// where it was put, a second and more of cmds with no input later. A
/// box one unit deep is nudged out by `PM_CorrectAllSolid` instead.
#[test]
fn a_player_set_down_inside_a_brush_stays_there() {
    let w = test_world(&[(Vec3::new(-64.0, -64.0, -16.0), Vec3::new(64.0, 64.0, 9.125))]);
    let mw = MoveWorld::bare(&w);
    let mut deep = PlayerState::spawn(Vec3::new(0.0, 0.0, 0.0), 0.0);
    tick(&mut deep, &PmInput::default(), &mw, 150);
    assert_eq!(deep.origin, Vec3::ZERO);
    assert!(!deep.on_ground);

    let w = test_world(&[(Vec3::new(-64.0, -64.0, -16.0), Vec3::new(64.0, 64.0, 0.5))]);
    let mw = MoveWorld::bare(&w);
    let mut shallow = PlayerState::spawn(Vec3::new(0.0, 0.0, 0.0), 0.0);
    tick(&mut shallow, &PmInput::default(), &mw, 10);
    assert!((shallow.origin.z - 0.5).abs() < 0.2, "{:?}", shallow.origin);
    assert!(shallow.on_ground);
}

/// A pool: dry ground west, deep water (surface z=36) over a floor at
/// -74, a chest-deep shelf (top 0) and an ankle-deep shelf (top 30), an
/// exit wall at x=200 rising to 15, and landing ground east of it.
fn pool_world() -> CollisionWorld {
    crate::collision::synthetic_world(
        &[
            ("textures/test/solid", crate::collision::CONTENTS_SOLID, 0),
            ("textures/common/water", crate::collision::CONTENTS_WATER, 0),
        ],
        &[
            (0, [-600.0, -300.0, -16.0], [-200.0, 300.0, 0.0]),
            (0, [-200.0, -300.0, -90.0], [200.0, 300.0, -74.0]),
            (0, [-160.0, -100.0, -84.0], [185.0, 100.0, 0.0]),
            (0, [-200.0, 150.0, -84.0], [200.0, 260.0, 30.0]),
            (0, [200.0, -300.0, -90.0], [216.0, 300.0, 15.0]),
            (0, [240.0, -300.0, -16.0], [600.0, 300.0, 0.0]),
            (1, [-200.0, -300.0, -74.0], [200.0, 300.0, 36.0]),
        ],
    )
}

/// `groundEntityNum` is the ground trace's entity: a player on a
/// submodel carries the entity the server named for it, one on model 0
/// the world's number, and one in the air none.
#[test]
fn the_ground_entity_is_the_ground_traces_entity() {
    let w = crate::collision::submodel_test_world(
        "{\n\"classname\" \"script_brushmodel\"\n\"model\" \"*1\"\n\"origin\" \"200 0 0\"\n}",
        &[([-32.0, -32.0, 0.0], [32.0, 32.0, 8.0])],
    );
    w.set_model_entity(1, 177);
    let w = MoveWorld::bare(&w);
    let mut ps = PlayerState::spawn(Vec3::new(200.0, 0.0, 9.0), 0.0);
    tick(&mut ps, &PmInput::default(), &w, 10);
    assert_eq!(ps.ground_entity_num(), 177);
    let mut ps = PlayerState::spawn(Vec3::new(0.0, 0.0, 1.0), 0.0);
    tick(&mut ps, &PmInput::default(), &w, 10);
    assert_eq!(ps.ground_entity_num(), ENTITYNUM_WORLD);
    ps.on_ground = false;
    assert_eq!(ps.ground_entity_num(), ENTITYNUM_NONE);
}

#[test]
fn ankle_deep_water_wades_at_retails_scale() {
    // retail has no wading wish clamp: nothing references
    // pm_waterSwimScale/pm_waterWadeScale and the walk mover (0x2F03C)
    // never reads water level - only the water-friction term slows you
    let w = pool_world();
    let w = MoveWorld::bare(&w);
    let run = PmInput {
        forward: 1.0,
        ..Default::default()
    };
    let mut dry = PlayerState::spawn(Vec3::new(-400.0, 0.0, 1.0), 0.0);
    tick(&mut dry, &run, &w, 100);
    let mut wet = PlayerState::spawn(Vec3::new(-150.0, 200.0, 30.2), 0.0);
    tick(&mut wet, &run, &w, 100);
    assert_eq!(wet.water_level, 1);
    let dry_speed = dry.velocity.truncate().length();
    let wet_speed = wet.velocity.truncate().length();
    // Retail's cmd scale wades at `1 - waterlevel / 3 * 0.5`
    // (cod11-mantle.md, "The wish speed").
    assert!(
        wet_speed == 158.0,
        "ankle-deep run {wet_speed} vs dry {dry_speed}"
    );
}

/// A slope is not a launch ramp. Retail's `PM_StepSlideMove` (0x34fbc)
/// takes its down pass on every grounded frame and pushes half a step
/// size past the step it took, so a walker is pulled back onto the ground
/// at a crest instead of floating off it.
#[test]
fn walking_a_slope_never_leaves_the_ground() {
    // 5 degrees over 300 units, then level ground at the top: the crest
    // is what a walker used to launch off.
    let w = crate::collision::ramp_test_world(5.0, 0.0, 300.0);
    let w = MoveWorld::bare(&w);
    let mut ps = PlayerState::spawn(Vec3::new(-200.0, 0.0, 1.0), 0.0);
    tick(&mut ps, &PmInput::default(), &w, 40);
    assert!(ps.on_ground, "the walker never settled on the floor");
    let mut airborne = 0;
    let forward = PmInput {
        forward: 1.0,
        ..Default::default()
    };
    for _ in 0..500 {
        pmove(&mut ps, &forward, &w, 1.0 / 125.0, &[]);
        if !ps.on_ground {
            airborne += 1;
        }
    }
    assert!(
        ps.origin.x > 320.0,
        "the walk never crossed the crest, at {}",
        ps.origin
    );
    assert_eq!(
        airborne, 0,
        "the walk left the ground {airborne} frames, ending at {}",
        ps.origin
    );
}

/// Standing on a slope holds its height: the ground trace, the friction
/// and the gravity must settle rather than trade the player up and down.
#[test]
fn standing_on_a_slope_holds_its_height() {
    let w = crate::collision::ramp_test_world(5.0, 0.0, 300.0);
    let w = MoveWorld::bare(&w);
    let mut ps = PlayerState::spawn(Vec3::new(150.0, 0.0, 20.0), 0.0);
    tick(&mut ps, &PmInput::default(), &w, 60);
    let (z, ground) = (ps.origin.z, ps.on_ground);
    assert!(ground, "the stander is not on the slope at {}", ps.origin);
    for _ in 0..100 {
        pmove(&mut ps, &PmInput::default(), &w, 1.0 / 125.0, &[]);
        assert_eq!(ps.on_ground, ground, "the ground flipped under a stander");
        assert!(
            (ps.origin.z - z).abs() < 0.01,
            "a stander drifted from {z} to {}",
            ps.origin.z
        );
    }
}

#[test]
fn water_level_tracks_depth() {
    let w = pool_world();
    let w = MoveWorld::bare(&w);
    // deep floor: only the eye stays above the surface? no - fully under
    let mut ps = PlayerState::spawn(Vec3::new(0.0, -200.0, -73.0), 0.0);
    tick(&mut ps, &PmInput::default(), &w, 5);
    assert_eq!(ps.water_level, 3, "on the pool bottom at {}", ps.origin);

    // chest-deep shelf: feet+1 and waist wet, eyes dry
    let mut ps = PlayerState::spawn(Vec3::new(150.0, 0.0, 0.2), 0.0);
    tick(&mut ps, &PmInput::default(), &w, 5);
    assert_eq!(ps.water_level, 2, "on the shelf at {}", ps.origin);

    // ankle-deep shelf
    let mut ps = PlayerState::spawn(Vec3::new(0.0, 200.0, 30.2), 0.0);
    tick(&mut ps, &PmInput::default(), &w, 5);
    assert_eq!(ps.water_level, 1, "ankle-deep at {}", ps.origin);

    // dry ground west of the pool
    let mut ps = PlayerState::spawn(Vec3::new(-400.0, 0.0, 1.0), 0.0);
    tick(&mut ps, &PmInput::default(), &w, 5);
    assert_eq!(ps.water_level, 0);
}

#[test]
fn deep_water_has_no_swim() {
    // Retail 1.1 MP walks and falls through water: no swim, no sink
    // wish, no water jump (cod11-mantle.md, "Water").
    let w = pool_world();
    let w = MoveWorld::bare(&w);
    let mut ps = PlayerState::spawn(Vec3::new(0.0, -200.0, 20.0), 0.0);
    let mut fastest = 0.0f32;
    for _ in 0..60 {
        pmove(&mut ps, &PmInput::default(), &w, 0.016, &[]);
        fastest = fastest.max(-ps.velocity.z);
    }
    assert!(fastest > 200.0, "a free fall through the water: {fastest}");
    assert!(ps.on_ground, "on the pool floor at {}", ps.origin);
    assert_eq!(ps.water_level, 3);
}

// --- ladders ---
