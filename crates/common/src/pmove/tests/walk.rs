//! The walk: friction, the cmd scale, the accelerates and the legs' direction.

use super::*;

/// A shellshocked client's `wbuttons` 0x4 takes the walk's wish speed to
/// 0.4: retail's shocked carbine runs at 89.6 and backs at 62.72
/// (`server/tests/shellshock_ab.rs`).
#[test]
fn shellshock_bit_slows_the_walk_wish() {
    let ps = PlayerState::spawn(Vec3::new(0.0, 0.0, 0.1), 0.0);
    let run = PmInput {
        forward: 1.0,
        ..Default::default()
    };
    let shocked = PmInput {
        shellshock: true,
        ..run
    };
    let (_, free) = wish(&ps, &run, None, WishArith::Kept);
    let (_, slow) = wish(&ps, &shocked, None, WishArith::Kept);
    assert!((slow - free * 0.4).abs() < 1e-4, "{slow} {free}");
}

/// 1.5b's walk scale (0x1f08b) stores the length, the base scale and
/// each factor, where 1.1d rounds the whole product once: a diagonal
/// run at 190 wishes a float step more (cod15-movement.md 8.2, "The
/// walk's cmd scale").
#[test]
fn the_1_5_walk_scale_rounds_each_step() {
    let mut ps = PlayerState::spawn(Vec3::new(0.0, 0.0, 0.1), 0.0);
    ps.rules = MoveRules::V1_5;
    assert_eq!(ps.speed, 190.0);
    let run = PmInput {
        forward: 1.0,
        right: 1.0,
        ..Default::default()
    };
    let len = f64::from(32258f32.sqrt());
    let (_, kept) = wish(&ps, &run, None, WishArith::Kept);
    let (_, stored) = wish(&ps, &run, None, WishArith::Stored);
    assert_eq!(kept / len, f64::from(f32::from_bits(0x3f87687d)));
    assert_eq!(stored / len, f64::from(f32::from_bits(0x3f87687e)));
}

/// The Windows cgames' `PM_Accelerate` (1.1 0x30007560) caps the rate
/// at the lack stored as a float, where 1.1d's inline one keeps it.
#[test]
fn the_windows_accelerate_stores_the_lack() {
    let dir = x87::normalize_stored(Vec3::new(1.0, 1.0, 0.0));
    let mut ps = PlayerState::spawn(Vec3::ZERO, 0.0);
    ps.velocity = Vec3::new(150.0, 0.0, 0.0);
    let mut kept = ps;
    accelerate_msvc(&mut ps, dir, 110.0, PM_ACCELERATE, 0.05);
    // 1.1d's walk (0x2f4e4-0x2f57c): the lack is under the rate of 49.5.
    let add = 110.0 - x87::dot(kept.velocity, dir);
    kept.velocity = (dir.as_dvec3() * add + kept.velocity.as_dvec3()).as_vec3();
    assert_eq!(ps.velocity, Vec3::new(152.78175, 2.7817473, 0.0));
    assert_eq!(kept.velocity, Vec3::new(152.78175, 2.781747, 0.0));
}

/// The Windows walk's inline clip multiplies a positive dot by the float
/// `1 / 1.001` (1.1 rodata 0x30069750), a last bit off the division.
#[test]
fn the_windows_walk_clip_takes_the_reciprocal_overclip() {
    let n = x87::normalize_stored(Vec3::new(0.3, 0.1, 1.0));
    let v = Vec3::new(101.76, 30.0, 0.0);
    assert_eq!(
        clip_velocity_msvc(v, n),
        Vec3::new(92.62514, 26.955044, -30.44955)
    );
    assert_eq!(
        clip_velocity(v, n, false),
        Vec3::new(92.62514, 26.955046, -30.44955)
    );
}

/// The Windows cgames' walk scale (1.5 0x30007e40) stores the run
/// scale's product before the stance's, where 1.1d rounds once: a
/// crouched diagonal run wishes a float step less on every protocol's
/// client (cod15-movement.md 8.2, "The walk's cmd scale").
#[test]
fn the_windows_walk_scale_stores_the_run_product() {
    let run = PmInput {
        forward: 1.0,
        right: 1.0,
        ..Default::default()
    };
    let len = f64::from(32258f32.sqrt());
    let base = 190.0 * 127.0 / (32258f64.sqrt() * 127.0);
    let crouch = f64::from(SCALE_CROUCH);
    for rules in [MoveRules::V1_1, MoveRules::V1_5, MoveRules::Uo] {
        let mut ps = PlayerState::spawn(Vec3::new(0.0, 0.0, 0.1), 0.0);
        ps.rules = rules;
        ps.settle_stance(Stance::Crouch);
        let (_, kept) = wish(&ps, &run, None, WishArith::Kept);
        let (_, msvc) = wish(&ps, &run, None, WishArith::Msvc);
        assert_eq!(kept / len, f64::from((base * crouch) as f32), "{rules:?}");
        let windows = (f64::from(base as f32) * crouch) as f32;
        assert_eq!(msvc / len, f64::from(windows), "{rules:?}");
        assert_ne!(kept, msvc, "{rules:?}");
    }
}

/// `PM_WalkMove`'s accel and `PM_Friction`'s ground control both scale
/// down while the knockback timer runs.
#[test]
fn knockback_quarters_accel_and_softens_friction() {
    let w = flat();
    let mw = MoveWorld::bare(&w);
    let run = PmInput {
        forward: 1.0,
        ..Default::default()
    };
    let mut free = PlayerState::spawn(Vec3::new(0.0, 0.0, 0.1), 0.0);
    let mut kb = free;
    kb.knockback_ms = 300.0;
    kb.knockback_flags = PMF_TIME_KNOCKBACK;
    // One frame can't separate a 0.25 accel scale from snap_velocity's
    // rounding, which alone moves either side by up to 0.5; ten frames
    // of walk accel put both well clear of that noise floor.
    for _ in 0..10 {
        pmove(&mut free, &run, &mw, 0.016, &[]);
        pmove(&mut kb, &run, &mw, 0.016, &[]);
    }
    assert!(
        kb.velocity.x == 43.0,
        "{} vs {}",
        kb.velocity.x,
        free.velocity.x
    );

    let mut slide = PlayerState::spawn(Vec3::new(0.0, 0.0, 0.1), 0.0);
    slide.velocity = Vec3::new(190.0, 0.0, 0.0);
    let mut slide_kb = slide;
    slide_kb.knockback_ms = 300.0;
    slide_kb.knockback_flags = PMF_TIME_KNOCKBACK;
    pmove(&mut slide, &PmInput::default(), &mw, 0.008, &[]);
    pmove(&mut slide_kb, &PmInput::default(), &mw, 0.008, &[]);
    assert!(slide_kb.velocity.x > slide.velocity.x);
}

/// A hit's knockback timer (`pm_flags` 0x200) takes the ground friction
/// away, and the gravity `PM_WalkMove` adds under it is folded into
/// ground speed by the clip; the drop ends it ahead of the walk.
#[test]
fn a_damage_knockback_slides_free_of_friction() {
    let w = flat();
    let mw = MoveWorld::bare(&w);
    let mut ps = PlayerState::spawn(Vec3::new(0.0, 0.0, 0.1), 0.0);
    pmove(&mut ps, &PmInput::default(), &mw, 0.008, &[]);
    ps.velocity = Vec3::new(80.0, 0.0, 0.0);
    ps.knockback_ms = 50.0;
    ps.knockback_flags = PMF_TIME_DAMAGE;
    let mut free = ps;
    free.knockback_ms = 0.0;
    free.knockback_flags = 0;
    for _ in 0..6 {
        pmove(&mut ps, &PmInput::default(), &mw, 0.008, &[]);
        pmove(&mut free, &PmInput::default(), &mw, 0.008, &[]);
    }
    assert!(ps.velocity.x >= 80.0, "{}", ps.velocity.x);
    assert!(free.velocity.x < 70.0, "{}", free.velocity.x);
    assert_eq!(ps.knockback_ms, 2.0);
    pmove(&mut ps, &PmInput::default(), &mw, 0.008, &[]);
    assert_eq!((ps.knockback_ms, ps.knockback_flags), (0.0, 0));
    assert!(ps.velocity.x < 80.0, "friction is back, {}", ps.velocity.x);
}

/// Slick ground takes the ground friction away, walks at an accel of 1
/// where standing on dirt takes 9, and adds the gravity the clip folds
/// back into speed, so a slide keeps going where dirt stops it.
#[test]
fn slick_ground_has_no_friction_and_an_accel_of_one() {
    let floor = |flags| {
        crate::collision::synthetic_world(
            &[(
                "textures/test/floor",
                crate::collision::CONTENTS_SOLID,
                flags,
            )],
            &[(0, [-2048.0, -2048.0, -16.0], [2048.0, 2048.0, 0.0])],
        )
    };
    let (ice, dirt) = (floor(crate::collision::SURF_SLICK), floor(0));
    let (ice, dirt) = (MoveWorld::bare(&ice), MoveWorld::bare(&dirt));
    let mut on_ice = PlayerState::spawn(Vec3::new(0.0, 0.0, 0.1), 0.0);
    pmove(&mut on_ice, &PmInput::default(), &ice, 0.008, &[]);
    assert!(on_slick(&on_ice));
    let mut on_dirt = PlayerState::spawn(Vec3::new(0.0, 0.0, 0.1), 0.0);
    pmove(&mut on_dirt, &PmInput::default(), &dirt, 0.008, &[]);
    assert!(!on_slick(&on_dirt));

    let (mut slide_ice, mut slide_dirt) = (on_ice, on_dirt);
    slide_ice.velocity = Vec3::new(80.0, 0.0, 0.0);
    slide_dirt.velocity = slide_ice.velocity;
    for _ in 0..6 {
        pmove(&mut slide_ice, &PmInput::default(), &ice, 0.008, &[]);
        pmove(&mut slide_dirt, &PmInput::default(), &dirt, 0.008, &[]);
    }
    assert!(slide_ice.velocity.x >= 80.0, "{}", slide_ice.velocity.x);
    assert!(slide_dirt.velocity.x < 70.0, "{}", slide_dirt.velocity.x);
    assert!(slide_ice.on_ground);

    let run = PmInput {
        forward: 1.0,
        ..Default::default()
    };
    let (mut run_ice, mut run_dirt) = (on_ice, on_dirt);
    pmove(&mut run_ice, &run, &ice, 0.016, &[]);
    pmove(&mut run_dirt, &run, &dirt, 0.016, &[]);
    // Ice: the accel's 1 * 0.016 * 190 = 3.04 plus the gravity's 12.8
    // down, which the clip's rescale turns into |(3.04, 12.8)| = 13.16
    // of ground speed. An accel of 9 would read 30, no gravity 3.
    assert_eq!(run_ice.velocity, Vec3::new(13.0, 0.0, 0.0));
    // Dirt: 9 * 0.016 * 190 = 27.36.
    assert_eq!(run_dirt.velocity, Vec3::new(27.0, 0.0, 0.0));
}

/// The air accelerate floors its rate at 100 as the walk's does: 1.1d's
/// inline one at 0x2f159 (rodata 0x708ec), 1.5b's and UO's
/// `PM_Accelerate` (0x1ed62, `pm_stopspeed`). 1.5's air wish takes a
/// lean as the walk (0x1eec9): a jump while leaning wishes 76 until
/// the lean has eased back, and gains 100's 5 units in a 50 ms frame,
/// not Q3's 3.8 (the snap rounds that to 4). 1.1's walk key does the
/// same.
#[test]
fn a_wish_under_100_in_the_air_accelerates_at_100s_rate() {
    let world = test_world(&[]);
    let w = MoveWorld::bare(&world);
    for rules in [MoveRules::V1_1, MoveRules::V1_5, MoveRules::Uo] {
        let input = PmInput {
            forward: 1.0,
            walk_slow: rules == MoveRules::V1_1,
            lean_left: rules.v15(),
            ..Default::default()
        };
        let mut ps = PlayerState::spawn(Vec3::new(0.0, 0.0, 4000.0), 0.0);
        ps.rules = rules;
        // A lean carried off the ground, easing back in the air.
        ps.lean = 0.25;
        pmove(&mut ps, &input, &w, 0.05, &[]);
        assert!(!ps.on_ground, "{rules:?}");
        assert_eq!(
            ps.velocity.truncate(),
            glam::Vec2::new(5.0, 0.0),
            "{rules:?} lean {}",
            ps.lean
        );
    }
}

/// `movementDir` is the legs' heading off the view, which the player
/// entity carries as `angles2[1]`. Straight ahead is 0, a pure strafe is
/// the 90-degree cap, and a backpedal folds by 180.
#[test]
fn movement_dir_is_the_legs_yaw_off_the_view() {
    let w = flat();
    let w = MoveWorld::bare(&w);
    // From a standstill each time: velocity left over from the previous
    // input keeps the displacement off the key being held.
    let held = |input: &PmInput| {
        let mut ps = PlayerState::spawn(Vec3::ZERO, 0.0);
        tick(&mut ps, &PmInput::default(), &w, 50);
        tick(&mut ps, input, &w, 50);
        ps.movement_dir
    };
    assert_eq!(held(&PmInput::default()), 0, "standing still");
    assert_eq!(
        held(&PmInput {
            forward: 1.0,
            ..PmInput::default()
        }),
        0,
        "running along the view"
    );
    // Right is -y at yaw 0, so a right strafe reads negative.
    assert_eq!(
        held(&PmInput {
            right: 1.0,
            ..PmInput::default()
        }),
        -90,
        "strafing right"
    );
    let diagonal = held(&PmInput {
        forward: 1.0,
        right: 1.0,
        ..PmInput::default()
    });
    assert!(
        (diagonal + 45).abs() <= 1,
        "running forward-right: {diagonal}"
    );
    // Backpedalling points the legs the way the body faces, not the way
    // it travels: retail folds the angle by 180 when forwardmove < 0.
    assert_eq!(
        held(&PmInput {
            forward: -1.0,
            ..PmInput::default()
        }),
        0,
        "backpedalling"
    );

    // Prone reads the body's own yaw instead of the move, so turning the
    // view inside the prone cone turns the legs away from it. Retail
    // settles at -59 for a 60-degree turn the other way: `AngleDelta`
    // folds 60 through the shorts to 59.996 and the cast truncates.
    let mut ps = PlayerState::spawn(Vec3::ZERO, 0.0);
    tick(&mut ps, &PmInput::default(), &w, 50);
    let prone = PmInput {
        prone: true,
        ..PmInput::default()
    };
    tick(&mut ps, &prone, &w, 60);
    assert_eq!(ps.stance, Stance::Prone);
    ps.yaw = (-60f32).to_radians();
    tick(&mut ps, &prone, &w, 20);
    assert_eq!(ps.movement_dir, 59, "prone, view turned 60 degrees");
}

#[test]
fn accelerates_to_run_speed_cap() {
    let w = flat();
    let w = MoveWorld::bare(&w);
    let mut ps = PlayerState::spawn(Vec3::ZERO, 0.0);
    tick(
        &mut ps,
        &PmInput {
            forward: 1.0,
            ..Default::default()
        },
        &w,
        250,
    );
    let h = ps.velocity.truncate().length();
    assert!(h == 190.0, "run speed {h}");
    // yaw 0 => moving along +X
    assert!(ps.velocity.x > 100.0 && ps.velocity.y == 0.0);
}

/// The walk wishes `ps.speed`, not a fixed 190: a `g_speed` 250
/// server runs its bodies at 250, and a speed of 0 stands still.
#[test]
fn the_run_takes_the_playerstate_s_speed() {
    let w = flat();
    let w = MoveWorld::bare(&w);
    let run = |speed: f32| {
        let mut ps = PlayerState::spawn(Vec3::ZERO, 0.0);
        ps.speed = speed;
        let input = PmInput {
            forward: 1.0,
            ..Default::default()
        };
        tick(&mut ps, &input, &w, 250);
        ps.velocity.truncate().length()
    };
    let h = run(250.0);
    assert!(h == 250.0, "run speed {h}");
    assert_eq!(run(0.0), 0.0);
}

#[test]
fn friction_stops_the_player() {
    let w = flat();
    let w = MoveWorld::bare(&w);
    let mut ps = PlayerState::spawn(Vec3::ZERO, 0.0);
    tick(
        &mut ps,
        &PmInput {
            forward: 1.0,
            ..Default::default()
        },
        &w,
        250,
    );
    tick(&mut ps, &PmInput::default(), &w, 200);
    assert!(
        ps.velocity.length() < 1.0,
        "still moving at {}",
        ps.velocity.length()
    );
}

#[test]
fn walks_on_mp_pavlov_terrain() {
    let Some(data) = crate::testing::real_bsp() else {
        return;
    };
    let bsp = crate::bsp::parse(&data).unwrap();
    let world = CollisionWorld::build(&bsp, &[]);
    let world = MoveWorld::bare(&world);
    let (origin, yaw) = crate::bsp::find_spawn(&bsp.entities).unwrap();
    let mut ps = PlayerState::spawn(Vec3::from(origin) + Vec3::Z * 2.0, yaw);
    // must land on terrain triangles near the spawn, not fall to bedrock
    tick(&mut ps, &PmInput::default(), &world, 250);
    assert!(ps.on_ground, "player should land");
    assert!(
        (ps.origin.z - origin[2]).abs() < 40.0,
        "landed at z={}, spawn z={}",
        ps.origin.z,
        origin[2]
    );
    let start = ps.origin;
    tick(
        &mut ps,
        &PmInput {
            forward: 1.0,
            ..Default::default()
        },
        &world,
        500,
    );
    assert!(
        ps.origin.truncate().distance(start.truncate()) > 100.0,
        "should cover ground"
    );
    assert!(
        ps.origin.z > -100.0,
        "must not fall through the map, z={}",
        ps.origin.z
    );
}
