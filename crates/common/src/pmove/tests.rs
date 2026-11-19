use super::*;
use crate::collision::{CollisionWorld, test_world};
use crate::movetrace::{Body, CONTENTS_BODY};

fn flat() -> CollisionWorld {
    test_world(&[])
}

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

/// The Windows cgames' walk scale (1.5 0x30007e40) stores the run
/// scale's product before the stance's, where 1.1d rounds once: a
/// crouched diagonal run wishes a float step less on every protocol's
/// client (cod15-movement.md 8.2, "The walk's cmd scale").
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

/// `PM_DropTimers` zeroes the timer once the frame's ms reach it, and
/// otherwise subtracts.
#[test]
fn knockback_runs_out() {
    let w = flat();
    let mut ps = PlayerState::spawn(Vec3::ZERO, 0.0);
    ps.knockback_ms = 20.0;
    pmove(
        &mut ps,
        &PmInput::default(),
        &MoveWorld::bare(&w),
        0.016,
        &[],
    );
    assert_eq!(ps.knockback_ms, 4.0);
    pmove(
        &mut ps,
        &PmInput::default(),
        &MoveWorld::bare(&w),
        0.016,
        &[],
    );
    assert_eq!(ps.knockback_ms, 0.0);
}

/// `PM_DropTimers` runs from `PmoveSingle` for every `pm_type`, so a
/// linked player (an S&D planter) still drops the timer even though
/// `linked_move` skips the move, ground and weapon dispatch.
#[test]
fn a_linked_player_still_drops_knockback() {
    let w = flat();
    let mut ps = PlayerState::spawn(Vec3::ZERO, 0.0);
    ps.linked = true;
    ps.knockback_ms = 20.0;
    pmove(
        &mut ps,
        &PmInput::default(),
        &MoveWorld::bare(&w),
        0.016,
        &[],
    );
    assert_eq!(ps.knockback_ms, 4.0);
    pmove(
        &mut ps,
        &PmInput::default(),
        &MoveWorld::bare(&w),
        0.016,
        &[],
    );
    assert_eq!(ps.knockback_ms, 0.0);
}

/// `PmoveSingle` clears `pm_flags` 0x8000 at its top whatever the
/// pm_type (0x33e5a): a dead, a dead linked and a spectator move each
/// end with the refused prone press gone (BUG-132).
#[test]
fn every_move_arm_clears_the_prone_blocked_flag() {
    let w = flat();
    let w = MoveWorld::bare(&w);
    for arm in 0..3 {
        let mut ps = PlayerState::spawn(Vec3::ZERO, 0.0);
        ps.prone_blocked = true;
        ps.prone_flip = true;
        match arm {
            0 => drop(dead_move(&mut ps, &w, 0.05)),
            1 => dead_linked_move(&mut ps, &w, 0.05),
            _ => drop(spectator_move(&mut ps, Some(&w), 0.0, 0.0, 0.0, 0, 0.05)),
        }
        assert!(!ps.prone_blocked, "arm {arm}");
        assert!(!ps.prone_flip, "arm {arm}");
    }
}

/// A death takes the sight down: `PmoveSingle`'s dead arm calls
/// `PM_ClearAimDownSightFlag` and nothing else about the weapon runs, so
/// the fraction stays where the death froze it
/// (docs/research/cod11-combat.md, 1.13).
#[test]
fn a_death_clears_the_ads_flag() {
    let w = flat();
    let w = MoveWorld::bare(&w);
    let mut ps = PlayerState::spawn(Vec3::ZERO, 0.0);
    ps.ads_active = true;
    ps.weapon_pos_frac = 1.0;
    dead_move(&mut ps, &w, 0.05);
    assert!(!ps.ads_active);
    assert_eq!(ps.weapon_pos_frac, 1.0);
}

/// `PM_DeadMove` takes 20 off a grounded corpse's speed every frame on
/// top of the friction, so the same knockback slides it a fraction of
/// what it slides a live player: the retail probe measured 25 units
/// against 60 for a flat 200 (docs/research/cod11-combat.md 5.7). The
/// drop is per frame, so halving the frame length shortens the slide.
#[test]
fn a_corpse_sheds_twenty_units_a_frame_on_the_ground() {
    let w = flat();
    let w = MoveWorld::bare(&w);
    let slide = |dt: f32| {
        let mut ps = PlayerState::spawn(Vec3::new(0.0, 0.0, 0.125), 0.0);
        ps.velocity = Vec3::new(240.0, 0.0, 0.0);
        ps.knockback_ms = 120.0;
        ps.knockback_flags = PMF_TIME_KNOCKBACK;
        for _ in 0..(2.0 / dt) as usize {
            dead_move(&mut ps, &w, dt);
        }
        assert_eq!(ps.velocity, Vec3::ZERO);
        ps.origin.x
    };
    let mut ps = PlayerState::spawn(Vec3::new(0.0, 0.0, 0.125), 0.0);
    ps.velocity = Vec3::new(10.0, 0.0, 0.0);
    dead_move(&mut ps, &w, 0.016);
    assert_eq!(ps.velocity, Vec3::ZERO, "10 is under one frame's drop");
    let (coarse, fine) = (slide(0.033), slide(0.016));
    assert!((20.0..35.0).contains(&coarse), "{coarse}");
    assert!(fine < coarse - 5.0, "{fine} against {coarse}");
}

/// The mounted arm (0x34274) never reaches the move dispatch or
/// `PM_Weapon`, so a mounted player's origin holds and no fire event
/// comes out of pmove (docs/research/cod11-turrets.md, section 5).
#[test]
fn a_mounted_player_does_not_move_or_fire() {
    let world = flat();
    let world = MoveWorld::bare(&world);
    let mut ps = PlayerState::spawn(Vec3::new(0.0, 0.0, 0.0), 0.0);
    ps.mounted = Some(Stance::Stand);
    let input = PmInput {
        forward: 127.0,
        attack: true,
        ..Default::default()
    };
    let before = ps.origin;
    let events = pmove(&mut ps, &input, &world, 0.05, &[]);
    assert_eq!(ps.origin, before);
    assert!(!ps.on_ground, "groundEntityNum NONE while mounted");
    assert!(
        events.iter().all(
            |e| e.event != weapon::EV_FIRE_WEAPON && e.event != weapon::EV_FIRE_WEAPON_LASTSHOT
        )
    );
}

/// The stance step (0x316f4) puts the player at the gun's stance
/// whatever the cmd asks, and `PM_UpdateLean` forces the lean input to 0
/// (docs/research/cod11-turrets.md, section 5).
#[test]
fn a_mounted_player_takes_the_gun_stance_whatever_the_cmd_asks() {
    let world = flat();
    let world = MoveWorld::bare(&world);
    let mut ps = PlayerState::spawn(Vec3::ZERO, 0.0);
    ps.mounted = Some(Stance::Prone);
    let input = PmInput {
        crouch: true,
        ..Default::default()
    };
    pmove(&mut ps, &input, &world, 0.05, &[]);
    assert_eq!(ps.stance, Stance::Prone);
    assert_eq!(ps.lean, 0.0);
}

/// 0x34274 calls no footstep routine, so a gunner mounted off a ladder
/// neither steps nor stays on it.
#[test]
fn a_mounted_player_takes_no_step_and_leaves_the_ladder() {
    let world = flat();
    let world = MoveWorld::bare(&world);
    let mut ps = PlayerState::spawn(Vec3::ZERO, 0.0);
    ps.mounted = Some(Stance::Stand);
    ps.on_ladder = true;
    ps.since_jump_ms = 10_000.0;
    ps.velocity = Vec3::new(0.0, 0.0, 200.0);
    let before = ps.bob_cycle;
    let events = pmove(&mut ps, &PmInput::default(), &world, 0.05, &[]);
    assert!(!ps.on_ladder);
    assert_eq!(ps.bob_cycle, before);
    assert!(events.is_empty(), "{events:?}");
}

/// Flat ground whose material carries the dirt sound surface (6).
fn dirt_flat() -> CollisionWorld {
    crate::collision::synthetic_world(
        &[(
            "textures/test/dirt",
            crate::collision::CONTENTS_SOLID,
            6 << 20,
        )],
        &[(0, [-2048.0, -2048.0, -16.0], [2048.0, 2048.0, 0.0])],
    )
}

/// Ankle-deep water over a floor at -6; both materials carry no sound
/// surface, so any footstep id can only come from the fixed water ids.
fn shallow_water() -> CollisionWorld {
    crate::collision::synthetic_world(
        &[
            ("textures/test/solid", crate::collision::CONTENTS_SOLID, 0),
            ("textures/common/water", crate::collision::CONTENTS_WATER, 0),
        ],
        &[
            (0, [-1024.0, -1024.0, -22.0], [1024.0, 1024.0, -6.0]),
            (1, [-1024.0, -1024.0, -74.0], [1024.0, 1024.0, 10.0]),
        ],
    )
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

fn tick(ps: &mut PlayerState, input: &PmInput, w: &MoveWorld, n: usize) {
    for _ in 0..n {
        pmove(ps, input, w, 1.0 / 125.0, &[]);
    }
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

/// Standing to prone is two legs of the eye, 150 ms down to the crouch
/// height and 400 ms on along a curve that dips to 15 at half way, and the
/// walk scale is prone's through the first and blends from crouch's
/// through the second (docs/research/cod11-mantle.md, "The eye through a
/// stance change"; the street capture's sideways press is the evidence).
#[test]
fn a_prone_press_walks_the_eye_through_the_crouch_height() {
    let w = flat();
    let w = MoveWorld::bare(&w);
    let mut ps = PlayerState::spawn(Vec3::ZERO, 0.0);
    tick(&mut ps, &PmInput::default(), &w, 50); // settle on the ground
    assert_eq!(ps.view_height(), VIEW_STAND);

    let prone = PmInput {
        prone: true,
        ..PmInput::default()
    };
    tick(&mut ps, &prone, &w, 1);
    assert_eq!(
        (ps.view_lerp_target, ps.view_lerp_down),
        (VIEW_CROUCH, true)
    );
    assert_eq!(stance_speed_scale(&ps), SCALE_PRONE);
    tick(&mut ps, &prone, &w, 19); // 152 ms: the crouch leg is done
    assert_eq!(
        (ps.view_lerp_target, ps.view_lerp_ms),
        (VIEW_PRONE, Some(0))
    );
    assert_eq!(ps.view_height(), VIEW_CROUCH);
    tick(&mut ps, &prone, &w, 25); // 200 ms into the prone leg
    assert_eq!(ps.view_height(), 15.0);
    let half = SCALE_PRONE * 0.5 + SCALE_CROUCH * 0.5;
    assert!((stance_speed_scale(&ps) - half).abs() < 1e-6);
    tick(&mut ps, &prone, &w, 25);
    assert_eq!(ps.view_height(), VIEW_PRONE);
    assert_eq!(ps.view_lerp_ms, None);
    assert_eq!(stance_speed_scale(&ps), SCALE_PRONE);
}

/// The hip cone's minimum follows the same two legs, linearly in time
/// from one end's minimum to the other's (combat doc 2.1).
#[test]
fn a_prone_press_blends_the_hip_spread_minimum_across_both_legs() {
    use weapon::{SpreadStance, hip_spread_min};
    let def = WeaponDef {
        hip_spread_stand_min: 3.0,
        hip_spread_ducked_min: 2.0,
        hip_spread_prone_min: 1.0,
        ..WeaponDef::default()
    };
    let w = flat();
    let w = MoveWorld::bare(&w);
    let mut ps = PlayerState::spawn(Vec3::ZERO, 0.0);
    tick(&mut ps, &PmInput::default(), &w, 50);
    let min = |ps: &PlayerState, time: i32| hip_spread_min(&def, &SpreadStance::of(ps, 0, time));
    assert_eq!(min(&ps, 0), 3.0);

    let prone = PmInput {
        prone: true,
        ..PmInput::default()
    };
    tick(&mut ps, &prone, &w, 1);
    assert_eq!(ps.view_lerp_ms, Some(0));
    assert_eq!(min(&ps, 0), 3.0);
    assert_eq!(min(&ps, 75), 2.5, "half the 150 ms leg down to crouch");
    assert_eq!(min(&ps, -10), 3.0, "a clock behind the leg's stamp");
    tick(&mut ps, &prone, &w, 19);
    assert_eq!(min(&ps, 0), 2.0, "the prone leg's start");
    tick(&mut ps, &prone, &w, 25);
    assert_eq!(min(&ps, 0), 1.5, "half the 400 ms leg to prone");
    assert_eq!(min(&ps, 1000), 1.0, "a clock past the leg's end");
    tick(&mut ps, &prone, &w, 25);
    assert_eq!(min(&ps, 0), 1.0);
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
fn falls_and_lands_on_floor() {
    let w = flat();
    let w = MoveWorld::bare(&w);
    let mut ps = PlayerState::spawn(Vec3::new(0.0, 0.0, 50.0), 0.0);
    tick(&mut ps, &PmInput::default(), &w, 250);
    assert!(ps.on_ground);
    assert!(ps.origin.z.abs() < 0.5, "settled at {}", ps.origin.z);
    assert!(ps.velocity.length() < 1.0);
}

/// Full run on dirt: rate 0.335 ticks/ms, truncated per frame
/// (rodata 0x70c44, PM_Footsteps @0x327f8).
#[test]
fn running_steps_fire_on_the_retail_cadence() {
    let w = dirt_flat();
    let w = MoveWorld::bare(&w);
    let mut ps = PlayerState::spawn(Vec3::ZERO, 0.0);
    let run = PmInput {
        forward: 1.0,
        ..Default::default()
    };
    for _ in 0..125 {
        pmove(&mut ps, &run, &w, 1.0 / 125.0, &[]);
    }
    assert!(ps.velocity.truncate().length() > 180.0);
    let mut steps = Vec::new();
    for _ in 0..375 {
        steps.extend(
            pmove(&mut ps, &run, &w, 8.0 / 1000.0, &[])
                .into_iter()
                .map(|e| e.event),
        );
    }
    assert!(
        steps.iter().all(|&e| e == EV_FOOTSTEP_RUN_BASE + 6),
        "{steps:?}"
    );
    // Retail truncates the cycle per frame (fistp under control word
    // 0xc00 @0x32819), so at fixed 8 ms frames the 2.68-tick advance
    // drops to 2 and the period stretches to 512 ms: 5 or 6 in 3 s.
    assert!(
        steps.len() >= 5 && steps.len() <= 6,
        "{} events",
        steps.len()
    );
}

/// The enable gate: walk key and the crouched/prone gaits tick the cycle
/// but never emit (PM_ShouldMakeFootsteps @0x3221c).
#[test]
fn walk_key_and_stances_are_silent() {
    let w = dirt_flat();
    let w = MoveWorld::bare(&w);
    for (name, input) in [
        (
            "walk key",
            PmInput {
                forward: 1.0,
                walk_slow: true,
                ..Default::default()
            },
        ),
        (
            "crouch",
            PmInput {
                forward: 1.0,
                crouch: true,
                ..Default::default()
            },
        ),
        (
            "prone",
            PmInput {
                forward: 1.0,
                prone: true,
                ..Default::default()
            },
        ),
    ] {
        let mut ps = PlayerState::spawn(Vec3::ZERO, 0.0);
        for _ in 0..125 {
            pmove(&mut ps, &input, &w, 1.0 / 125.0, &[]);
        }
        let speed = ps.velocity.truncate().length();
        assert!(speed > 10.0, "{name} must still move, at {speed}");
        let events: Vec<_> = (0..250)
            .flat_map(|_| pmove(&mut ps, &input, &w, 8.0 / 1000.0, &[]))
            .collect();
        assert!(events.is_empty(), "{name} played {events:?}");
    }
}

/// Keys released: the timer advances but no event fires (@0x32831 path).
#[test]
fn glide_suppresses_step_events() {
    let w = dirt_flat();
    let w = MoveWorld::bare(&w);
    let mut ps = PlayerState::spawn(Vec3::ZERO, 0.0);
    let run = PmInput {
        forward: 1.0,
        ..Default::default()
    };
    for _ in 0..125 {
        pmove(&mut ps, &run, &w, 1.0 / 125.0, &[]);
    }
    let idle = PmInput::default();
    let mut events = Vec::new();
    while ps.velocity.truncate().length() > 10.0 {
        events.extend(pmove(&mut ps, &idle, &w, 8.0 / 1000.0, &[]));
    }
    assert!(events.is_empty(), "{events:?}");
}

#[test]
fn landing_plays_one_event_from_the_impact_bands() {
    let w = dirt_flat();
    let w = MoveWorld::bare(&w);
    let mut ps = PlayerState::spawn(Vec3::new(0.0, 0.0, 200.0), 0.0);
    let idle = PmInput::default();
    let mut events = Vec::new();
    for _ in 0..500 {
        events.extend(pmove(&mut ps, &idle, &w, 8.0 / 1000.0, &[]));
    }
    assert!(ps.on_ground);
    let ids: Vec<i32> = events.into_iter().map(|e| e.event).collect();
    assert_eq!(ids, vec![EV_LANDING_BASE + 6], "a long fall lands once");
}

/// The ladder reads the fall height, not the speed: the one-unit drop a
/// turret release ends in lands in silence, as the retail turret capture
/// reads it (`crates/server/tests/turret_ab.rs`).
#[test]
fn a_one_unit_drop_lands_silently() {
    let w = dirt_flat();
    let w = MoveWorld::bare(&w);
    let mut ps = PlayerState::spawn(Vec3::new(0.0, 0.0, 1.0), 0.0);
    ps.on_ground = false;
    let idle = PmInput::default();
    let mut events = Vec::new();
    for _ in 0..20 {
        events.extend(pmove(&mut ps, &idle, &w, 8.0 / 1000.0, &[]));
    }
    assert!(ps.on_ground);
    assert!(events.is_empty(), "{events:?}");
}

/// The land anim's speed half: a 200-unit fall lands faster than
/// `LAND_ANIM_SPEED` and reports it on the landing move only; a
/// one-unit drop never does.
#[test]
fn only_a_fast_landing_reports_the_land_anim() {
    let w = dirt_flat();
    let w = MoveWorld::bare(&w);
    let idle = PmInput::default();
    let mut ps = PlayerState::spawn(Vec3::new(0.0, 0.0, 200.0), 0.0);
    ps.on_ground = false;
    let mut reported = Vec::new();
    for i in 0..500 {
        let was = ps.on_ground;
        pmove(&mut ps, &idle, &w, 8.0 / 1000.0, &[]);
        if ps.land_anim {
            reported.push((i, was, ps.on_ground));
        }
    }
    assert_eq!(reported.len(), 1, "{reported:?}");
    assert_eq!((reported[0].1, reported[0].2), (false, true));

    let mut ps = PlayerState::spawn(Vec3::new(0.0, 0.0, 1.0), 0.0);
    ps.on_ground = false;
    for _ in 0..20 {
        pmove(&mut ps, &idle, &w, 8.0 / 1000.0, &[]);
        assert!(!ps.land_anim);
    }
    assert!(ps.on_ground);
}

#[test]
fn landing_sound_bands_follow_fall_height() {
    let band = |sf: u32, height: f32| {
        let mut ps = PlayerState::spawn(Vec3::ZERO, 0.0);
        ps.ground_surface_flags = sf;
        landing_event(height, &ps).map(|e| (e.event, e.parm))
    };
    let dirt = 6 << 20;
    assert_eq!(band(dirt, 4.0), None);
    assert_eq!(band(dirt, 4.5), Some((EV_FOOTSTEP_WALK_BASE + 6, 0)));
    assert_eq!(band(dirt, 11.5), Some((EV_FOOTSTEP_RUN_BASE + 6, 0)));
    assert_eq!(band(dirt, 12.0), Some((EV_LANDING_BASE + 6, 0)));
    // the land event's parm is the view bob, 4 at 12 up to 24
    assert_eq!(band(dirt, 38.0), Some((EV_LANDING_BASE + 6, 8)));
    assert_eq!(band(dirt, 200.0), Some((EV_LANDING_BASE + 6, 24)));
    // material 0 silences the steps but lands on the default surface
    let quiet = SURF_NO_SOUND | dirt;
    assert_eq!(band(quiet, 11.5), None);
    assert_eq!(band(quiet, 38.0), Some((EV_LANDING_BASE, 8)));
}

/// Drop a player from `height` onto flat dirt at 8 ms frames and return
/// the landing frame's state and events.
fn land_from(height: f32, sf: u32) -> (PlayerState, Vec<PmEvent>) {
    let w = crate::collision::synthetic_world(
        &[("textures/test/floor", crate::collision::CONTENTS_SOLID, sf)],
        &[(0, [-2048.0, -2048.0, -16.0], [2048.0, 2048.0, 0.0])],
    );
    let w = MoveWorld::bare(&w);
    let mut ps = PlayerState::spawn(Vec3::new(0.0, 0.0, height), 0.0);
    ps.on_ground = false;
    ps.velocity.x = 100.0;
    for _ in 0..1000 {
        let events = pmove(&mut ps, &PmInput::default(), &w, 8.0 / 1000.0, &[]);
        if ps.on_ground {
            return (ps, events);
        }
    }
    panic!("never landed");
}

/// A fall past `bg_fallDamageMinHeight` (256) lands with the damage in
/// the pain event's parm, `pm_time` 35 * damage + 500 under 0x100 and
/// the velocity scaled by the stun's multiplier.
#[test]
fn a_damaging_landing_starts_the_stun() {
    let dirt = 6 << 20;
    let (ps, events) = land_from(300.0, dirt);
    let damage = events
        .iter()
        .find(|e| e.event == EV_LANDING_PAIN_BASE + 6)
        .map(|e| e.parm)
        .expect("pain event");
    // The velocity snap drops 0.4 a frame off gravity's 6.4, so the 300
    // units read as 281 by the impact speed: 11 percent.
    assert_eq!(damage, 11);
    let stun = 35 * damage + 500;
    assert_eq!(ps.knockback_ms, stun as f32);
    assert_eq!(ps.knockback_flags, PMF_TIME_KNOCKBACK);
    let scale = 0.5 - (stun - 500) as f32 / 1000.0 * 0.3;
    // 38.45, snapped
    assert_eq!(ps.velocity.x, (100.0 * scale).round(), "scale {scale}");

    // past 28 percent the scale pins at 0.2, past 42 the timer at 2000
    let (ps, events) = land_from(420.0, dirt);
    let damage = events
        .iter()
        .find(|e| e.event >= EV_LANDING_PAIN_BASE)
        .unwrap()
        .parm;
    assert!(damage > 42, "damage {damage}");
    assert_eq!(ps.knockback_ms, 2000.0);
    assert!((ps.velocity.x - 20.0).abs() < 0.01, "{}", ps.velocity.x);
}

/// Under the min height, on a no-damage or slick surface, and at 100
/// percent there is no stun; the last three damp by 0.67.
#[test]
fn a_soft_landing_starts_no_stun() {
    let dirt = 6 << 20;
    let (ps, events) = land_from(200.0, dirt);
    assert_eq!((ps.knockback_ms, ps.knockback_flags), (0.0, 0));
    assert_eq!(events[0].event, EV_LANDING_BASE + 6);
    assert!((ps.velocity.x - 67.0).abs() < 0.01);

    let (ps, events) = land_from(300.0, dirt | SURF_NODAMAGE);
    assert_eq!((ps.knockback_ms, ps.knockback_flags), (0.0, 0));
    assert_eq!(events[0].event, EV_LANDING_BASE + 6);

    for (sf, height, what) in [
        (dirt | crate::collision::SURF_SLICK, 300.0, "slick"),
        (dirt, 600.0, "fatal"),
    ] {
        let (ps, events) = land_from(height, sf);
        assert_eq!((ps.knockback_ms, ps.knockback_flags), (0.0, 0), "{what}");
        assert_eq!(events[0].event, EV_LANDING_PAIN_BASE + 6, "{what}");
        assert!(
            (ps.velocity.x - 67.0).abs() < 0.01,
            "{what} {}",
            ps.velocity.x
        );
    }
}

/// The two bounds are the cvars' values, and bounds out of order or a
/// negative min take no damage at all (0x2fe6c).
#[test]
fn the_fall_damage_bounds_are_the_cvars() {
    let at = |min, max| FallHeights { min, max };
    assert_eq!(fall_damage(300.0, 0, 0, FallHeights::default()), 19);
    assert_eq!(fall_damage(300.0, 0, 0, at(200.0, 1000.0)), 12);
    assert_eq!(fall_damage(300.0, 0, 0, at(100.0, 300.0)), 100);
    assert_eq!(fall_damage(250.0, 0, 2, at(200.0, 300.0)), 25);
    assert_eq!(fall_damage(900.0, 0, 0, at(480.0, 256.0)), 0);
    assert_eq!(fall_damage(900.0, 0, 0, at(-1.0, 256.0)), 0);

    // The retail run's systeminfo under `+set`s (player-clip doc 8.10).
    let info = r"\bg_fallDamageMaxHeight\1000\bg_fallDamageMinHeight\200\sv_cheats\0";
    assert_eq!(FallHeights::from_systeminfo(info), at(200.0, 1000.0));
    assert_eq!(
        FallHeights::from_systeminfo(r"\sv_cheats\0"),
        FallHeights::default()
    );
}

/// A corpse dropped onto the floor lands like a live player on a soft
/// fall, whatever the height: the damp and the land event, no pain and
/// no stun (`pm_type > 5` at 0x2feba).
#[test]
fn a_corpse_lands_with_no_damage() {
    let w = crate::collision::synthetic_world(
        &[(
            "textures/test/floor",
            crate::collision::CONTENTS_SOLID,
            6 << 20,
        )],
        &[(0, [-2048.0, -2048.0, -16.0], [2048.0, 2048.0, 0.0])],
    );
    let w = MoveWorld::bare(&w);
    let mut ps = PlayerState::spawn(Vec3::new(0.0, 0.0, 600.0), 0.0);
    ps.on_ground = false;
    let mut landed = None;
    for _ in 0..1000 {
        let events = dead_move(&mut ps, &w, 0.016);
        if ps.on_ground {
            landed = Some(events);
            break;
        }
    }
    let events = landed.expect("never landed");
    assert_eq!(events.len(), 1, "{events:?}");
    assert_eq!(events[0].event, EV_LANDING_BASE + 6);
    assert_eq!((ps.knockback_ms, ps.knockback_flags), (0.0, 0));
}

/// Water level 1-2 replaces the ground material with the fixed water ids
/// and ignores the enable gate (PM_FootstepEvent @0x321b0).
#[test]
fn water_steps_use_the_fixed_water_ids() {
    let w = shallow_water();
    let w = MoveWorld::bare(&w);
    let mut ps = PlayerState::spawn(Vec3::new(0.0, 0.0, -6.0), 0.0);
    let run = PmInput {
        forward: 1.0,
        ..Default::default()
    };
    for _ in 0..125 {
        pmove(&mut ps, &run, &w, 1.0 / 125.0, &[]);
    }
    assert_eq!(ps.water_level, 1);
    let mut ids = Vec::new();
    for _ in 0..250 {
        ids.extend(
            pmove(&mut ps, &run, &w, 8.0 / 1000.0, &[])
                .into_iter()
                .map(|e| e.event),
        );
    }
    assert!(!ids.is_empty());
    assert!(ids.iter().all(|&e| e == EV_FOOTSTEP_RUN_WATER), "{ids:?}");
}

/// 1.1 runs `PM_Weapon` ahead of `PM_Footsteps` (0x34331, 0x34336), 1.5
/// and UO behind it: a step that both fires and steps rings the shot
/// first on 1.1 only.
#[test]
fn the_shot_and_the_footstep_ring_in_each_versions_order() {
    let def = WeaponDef {
        clip_size: 200,
        fire_time: 0.001,
        clip_index: 1,
        ammo_index: 1,
        ..WeaponDef::default()
    };
    let weapons = [None, Some(def)];
    let w = dirt_flat();
    let w = MoveWorld::bare(&w);
    let run = PmInput {
        forward: 1.0,
        attack: true,
        weapon: 1,
        ..PmInput::default()
    };
    for (rules, shot_first) in [(MoveRules::V1_1, true), (MoveRules::V1_5, false)] {
        let mut ps = PlayerState::spawn(Vec3::new(0.0, 0.0, 0.125), 0.0);
        ps.rules = rules;
        weapon::give(&mut ps, 1, 1);
        ps.weapon = 1;
        ps.ammoclip[1] = 200;
        let both = (0..125)
            .map(|_| pmove(&mut ps, &run, &w, 0.008, &weapons))
            .find(|ev| {
                ev.iter().any(|e| e.event == weapon::EV_FIRE_WEAPON)
                    && ev.iter().any(|e| e.event == EV_FOOTSTEP_RUN_BASE + 6)
            })
            .expect("a step that fires and steps");
        let first = both
            .iter()
            .find(|e| e.event == weapon::EV_FIRE_WEAPON || e.event == EV_FOOTSTEP_RUN_BASE + 6)
            .unwrap();
        assert_eq!(
            first.event == weapon::EV_FIRE_WEAPON,
            shot_first,
            "{rules:?} {both:?}"
        );
    }
}

/// The airborne ladder branch probes into the wall face; a miss or a
/// material-less hit defaults to metal (@0x32039-0x32135).
#[test]
fn ladder_steps_probe_into_the_face_and_default_to_metal() {
    let input = PmInput::default();
    let dt = 0.05;

    let mut ps = PlayerState::spawn(Vec3::ZERO, 0.0);
    ps.on_ground = false;
    ps.on_ladder = true;
    // normal points away from the wall, so the probe reaches back into it
    ps.ladder_normal = -Vec3::X;
    ps.velocity.z = 80.0;
    let mut events = Vec::new();
    for _ in 0..20 {
        footsteps(
            &mut ps,
            &input,
            &MoveWorld::bare(&dirt_flat()),
            dt,
            &mut events,
        );
    }
    assert!(!events.is_empty(), "climbing at vz=80 must step");
    assert!(
        events
            .iter()
            .all(|e| e.event == EV_FOOTSTEP_RUN_BASE + DEFAULT_MATERIAL)
    );

    // same climb against a wooden wall: the probe reports wood
    let wall = crate::collision::synthetic_world(
        &[(
            "textures/test/wood",
            crate::collision::CONTENTS_SOLID,
            21 << 20,
        )],
        &[(0, [16.0, -1024.0, -16.0], [32.0, 1024.0, 512.0])],
    );
    let wall = MoveWorld::bare(&wall);
    let mut ps = PlayerState::spawn(Vec3::new(1.0, 0.0, 40.0), 0.0);
    ps.on_ground = false;
    ps.on_ladder = true;
    ps.ladder_normal = -Vec3::X;
    ps.velocity.z = 80.0;
    let mut events = Vec::new();
    for _ in 0..20 {
        footsteps(&mut ps, &input, &wall, dt, &mut events);
    }
    assert!(!events.is_empty());
    assert!(events.iter().all(|e| e.event == EV_FOOTSTEP_RUN_BASE + 21));
}

/// PM_CheckJump @0x2eda8: the push-off event carries the last ground trace's
/// material; from mid-wall (no ground under the climb) it stays silent.
#[test]
fn push_off_jump_event_reads_the_last_ground() {
    let n = Vec3::new(-1.0, 0.0, 0.0);
    let jump = PmInput {
        jump: true,
        ..Default::default()
    };
    let wall = crate::collision::synthetic_world(
        &[
            (
                "textures/test/wood",
                crate::collision::CONTENTS_SOLID,
                21 << 20,
            ),
            (
                "textures/common/ladder",
                crate::collision::CONTENTS_SOLID,
                0x8,
            ),
        ],
        &[
            (0, [-1024.0, -1024.0, -16.0], [1024.0, 1024.0, 0.0]),
            (1, [16.0, -1024.0, 0.0], [32.0, 1024.0, 512.0]),
        ],
    );
    let wall = MoveWorld::bare(&wall);
    let mut ps = PlayerState::spawn(Vec3::new(0.0, 0.0, 0.0), 0.0);
    ps.since_jump_ms = 10_000.0;
    pmove(&mut ps, &PmInput::default(), &wall, 1.0 / 125.0, &[]);
    assert_eq!(ps.ground_surface_flags, 21 << 20);
    let mut events = Vec::new();
    ladder_move(&mut ps, &jump, n, &wall, 1.0 / 125.0, &mut events);
    assert_eq!(
        events,
        vec![PmEvent {
            event: EV_JUMP_BASE + 21,
            parm: 0
        }]
    );

    // mid-wall: the last ground trace hit nothing, so no event
    let mut ps = PlayerState::spawn(Vec3::new(1.0, 0.0, 200.0), 0.0);
    ps.on_ground = false;
    ps.since_jump_ms = 10_000.0;
    let mut events = Vec::new();
    ladder_move(&mut ps, &jump, n, &wall, 1.0 / 125.0, &mut events);
    assert!(ladder_push_off_ran(&ps));
    assert!(events.is_empty());
}

/// The 299 ms post-push-off quiet window (@0x323b9).
#[test]
fn ladder_steps_stay_quiet_after_a_push_off() {
    let input = PmInput::default();
    let mut ps = PlayerState::spawn(Vec3::ZERO, 0.0);
    ps.on_ground = false;
    ps.on_ladder = true;
    ps.ladder_normal = -Vec3::X;
    ps.velocity.z = 80.0;
    ps.since_jump_ms = 0.0;
    let mut events = Vec::new();
    for _ in 0..5 {
        ps.since_jump_ms += 50.0;
        footsteps(
            &mut ps,
            &input,
            &MoveWorld::bare(&dirt_flat()),
            0.05,
            &mut events,
        );
    }
    assert!(events.is_empty(), "quiet for 299 ms, got {events:?}");
    for _ in 0..20 {
        ps.since_jump_ms += 50.0;
        footsteps(
            &mut ps,
            &input,
            &MoveWorld::bare(&dirt_flat()),
            0.05,
            &mut events,
        );
    }
    assert!(!events.is_empty(), "climbing must step once quiet");
}

/// `check_jump` off a ladder plane, the way `ladder_move` calls it.
fn push_off(ps: &mut PlayerState, input: &PmInput, normal: Vec3) -> bool {
    let mut cmd = *input;
    check_jump(ps, &mut cmd, Some(normal), &mut Vec::new())
}

fn ladder_push_off_ran(ps: &PlayerState) -> bool {
    ps.jump_latched && ps.since_jump_ms == 0.0
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
fn ground_jump_apex_is_the_jump_height() {
    let w = flat();
    let w = MoveWorld::bare(&w);
    let run = PmInput {
        forward: 1.0,
        ..Default::default()
    };
    // retail: vz = sqrt(2 * 39 * g) standing (0x708c8)
    let mut ps = PlayerState::spawn(Vec3::ZERO, 0.0);
    tick(&mut ps, &PmInput::default(), &w, 50); // settle
    let mut apex = 0.0f32;
    let launch = PmInput {
        forward: 1.0,
        jump: true,
        ..Default::default()
    };
    // 20 ms steps take a whole 16 off the velocity per frame, so the
    // snap does not bend the arc (`a_125_fps_jump_goes_higher`).
    pmove(&mut ps, &launch, &w, 0.02, &[]);
    for _ in 0..80 {
        pmove(&mut ps, &run, &w, 0.02, &[]);
        apex = apex.max(ps.origin.z);
    }
    assert!(
        apex == 39.035995,
        "standing apex {apex}, expected ~{JUMP_HEIGHT}"
    );
    assert!(ps.on_ground);
}

/// `PM_CheckJump` refuses `pm_flags` 0x1 and 0x2 (0x2ebc8, 0x2ebf5): a
/// crouched or prone player holding jump stays on the ground.
#[test]
fn neither_a_crouch_nor_a_prone_jumps() {
    let w = flat();
    let w = MoveWorld::bare(&w);
    for (crouch, prone) in [(true, false), (false, true)] {
        let hold = PmInput {
            crouch,
            prone,
            ..Default::default()
        };
        let mut ps = PlayerState::spawn(Vec3::ZERO, 0.0);
        tick(&mut ps, &hold, &w, 80); // settle into the stance
        assert_ne!(ps.stance, Stance::Stand);
        let hop = PmInput { jump: true, ..hold };
        for _ in 0..60 {
            pmove(&mut ps, &hop, &w, 1.0 / 125.0, &[]);
            assert!(ps.on_ground && !ps.jumped, "crouch {crouch} prone {prone}");
        }
    }
}

/// Retail sweeps a 12-unit box 54 units behind the facing before it lets a
/// body lie down (docs/research/cod11-mantle.md, "Prone").
#[test]
fn prone_is_refused_when_the_body_has_no_room_behind_it() {
    // A wall 20 units behind the origin, well inside the 54 the body needs.
    let w = crate::collision::test_world(&[(
        Vec3::new(-40.0, -30.0, -8.0),
        Vec3::new(-20.0, 30.0, 72.0),
    )]);
    let w = MoveWorld::bare(&w);
    let mut ps = PlayerState::spawn(Vec3::ZERO, 0.0);
    tick(&mut ps, &PmInput::default(), &w, 20);
    let prone = PmInput {
        prone: true,
        ..Default::default()
    };
    // Facing +x puts the body in the wall behind; facing -x puts it in the
    // open, and the same input is then taken.
    ps.yaw = 0f32.to_radians();
    tick(&mut ps, &prone, &w, 20);
    assert_eq!(
        ps.stance,
        Stance::Stand,
        "prone into a wall must be refused"
    );
    // Every refused cmd raises `pm_flags` 0x8000; the next cmd starts
    // clear.
    assert!(ps.prone_blocked, "the refusal is announced");
    tick(&mut ps, &PmInput::default(), &w, 1);
    assert!(!ps.prone_blocked, "cleared once the press is gone");

    ps.yaw = 180f32.to_radians();
    tick(&mut ps, &prone, &w, 20);
    assert_eq!(
        ps.stance,
        Stance::Prone,
        "prone with room behind must be taken"
    );
    assert!(!ps.prone_blocked);
    assert!(
        normalize180_accurate(ps.prone_direction - 180.0).abs() < 0.01,
        "the body faces the view, at {}",
        ps.prone_direction
    );
}

fn stance_events(ps: &mut PlayerState, input: &PmInput, w: &MoveWorld, n: usize) -> Vec<i32> {
    let mut out = Vec::new();
    for _ in 0..n {
        for e in pmove(ps, input, w, 1.0 / 125.0, &[]) {
            if (EV_STANCE_FORCE_STAND..=EV_STANCE_FORCE_PRONE).contains(&e.event) {
                out.push(e.event);
            }
        }
    }
    out
}

/// `PM_CheckDuck` 0x3196b-0x31996: every refused prone cmd tells the
/// client to stand, or to crouch when it came from a crouch, unless the
/// stance rides a held key (`wbuttons` 0x2).
#[test]
fn a_refused_prone_forces_the_stance_it_came_from() {
    let w = crate::collision::test_world(&[(
        Vec3::new(-40.0, -30.0, -8.0),
        Vec3::new(-20.0, 30.0, 72.0),
    )]);
    let w = MoveWorld::bare(&w);
    let mut ps = PlayerState::spawn(Vec3::ZERO, 0.0);
    tick(&mut ps, &PmInput::default(), &w, 20);
    let prone = PmInput {
        prone: true,
        ..Default::default()
    };
    let got = stance_events(&mut ps, &prone, &w, 3);
    assert_eq!(got, [EV_STANCE_FORCE_STAND; 3], "one per refused cmd");
    assert_eq!(ps.stance, Stance::Stand);

    let held = PmInput {
        stance_held: true,
        ..prone
    };
    assert!(stance_events(&mut ps, &held, &w, 3).is_empty());
    assert!(ps.prone_blocked, "a held key is refused all the same");

    let crouch = PmInput {
        crouch: true,
        ..Default::default()
    };
    tick(&mut ps, &crouch, &w, 5);
    let got = stance_events(&mut ps, &prone, &w, 1);
    assert_eq!(got, [EV_STANCE_FORCE_CROUCH]);
    assert_eq!(ps.stance, Stance::Crouch);
}

/// `PM_CheckDuck`'s rising arms (0x319a4-0x31b78): a prone player let go
/// under a crouch-high ceiling crouches; with no room for that either it
/// stays prone and is told so (142); a crouch under a ceiling it cannot
/// stand in stays and is told so (141).
#[test]
fn a_rise_without_headroom_stays_down_and_says_so() {
    let w = crate::collision::test_world(&[
        // Room to crouch, not to stand.
        (
            Vec3::new(200.0, -100.0, 55.0),
            Vec3::new(400.0, 100.0, 120.0),
        ),
        // Room to lie only.
        (
            Vec3::new(600.0, -100.0, 40.0),
            Vec3::new(800.0, 100.0, 120.0),
        ),
    ]);
    let w = MoveWorld::bare(&w);
    let prone = PmInput {
        prone: true,
        ..Default::default()
    };
    let crouch = PmInput {
        crouch: true,
        ..Default::default()
    };
    let lie_down_at = |x: f32| {
        let mut ps = PlayerState::spawn(Vec3::ZERO, 0.0);
        tick(&mut ps, &PmInput::default(), &w, 20);
        tick(&mut ps, &prone, &w, 20);
        assert_eq!(ps.stance, Stance::Prone);
        // Off the floor's face, which a zero-length trace reads as solid.
        ps.origin = Vec3::new(x, 0.0, 0.125);
        ps
    };

    let mut ps = lie_down_at(700.0);
    assert_eq!(
        stance_events(&mut ps, &crouch, &w, 1),
        [EV_STANCE_FORCE_PRONE]
    );
    assert_eq!(
        stance_events(&mut ps, &PmInput::default(), &w, 1),
        [EV_STANCE_FORCE_PRONE]
    );
    assert_eq!(ps.stance, Stance::Prone);

    let mut ps = lie_down_at(300.0);
    assert!(stance_events(&mut ps, &PmInput::default(), &w, 1).is_empty());
    assert_eq!(ps.stance, Stance::Crouch, "no room to stand: crouch");
    assert_eq!(
        stance_events(&mut ps, &PmInput::default(), &w, 1),
        [EV_STANCE_FORCE_CROUCH]
    );
    assert_eq!(ps.stance, Stance::Crouch);
}

/// `PM_UpdatePronePitch`'s ease (0x334e2-0x33585): 1.1 truncates both
/// pitches to whole shorts after each step, 1.5 keeps the float.
#[test]
fn the_prone_pitches_ease_in_whole_shorts_on_1_1() {
    let w = crate::collision::test_world(&[]);
    let w = MoveWorld::bare(&w);
    let eased = |rules| {
        let mut ps = PlayerState::spawn(Vec3::ZERO, 0.0);
        ps.rules = rules;
        ps.stance = Stance::Prone;
        ps.on_ground = true;
        ps.ground_plane = None;
        // -8673 shorts, the wire's value at the speedgrav capture's ct 48146.
        ps.prone_torso_pitch = -47.642_21;
        update_prone_pitch(&mut ps, &w, 0.008, &mut Vec::new());
        ps.prone_torso_pitch
    };
    assert_eq!(eased(MoveRules::V1_1), -8571.0 * (360.0 / 65536.0));
    assert!((eased(MoveRules::V1_5) - (-47.642_21 + 0.56)).abs() < 1e-5);
}

/// Every build's ease measures through the short-truncating
/// `AngleDelta` (1.1 0x334e3, 1.5 0x26fc8): a pitch under a short off
/// its target reads a difference of 0 and stays where it is.
#[test]
fn a_prone_pitch_a_fraction_of_a_short_off_level_stays() {
    let w = crate::collision::test_world(&[]);
    let w = MoveWorld::bare(&w);
    for rules in [MoveRules::V1_1, MoveRules::V1_5, MoveRules::Uo] {
        let mut ps = PlayerState::spawn(Vec3::ZERO, 0.0);
        ps.rules = rules;
        ps.stance = Stance::Prone;
        ps.on_ground = true;
        ps.ground_plane = None;
        ps.prone_torso_pitch = -0.003;
        update_prone_pitch(&mut ps, &w, 0.008, &mut Vec::new());
        assert_eq!(ps.prone_torso_pitch, -0.003, "{rules:?}");
    }
}

/// The pitch cap measures through `AngleDelta` on every build (1.1
/// 0x33295, 1.5 0x26ba0..): a torso pitch 45.003 degrees off the view
/// reads 45 and holds nothing.
#[test]
fn the_prone_pitch_cap_measures_in_whole_shorts() {
    let w = flat();
    let w = MoveWorld::bare(&w);
    for rules in [MoveRules::V1_1, MoveRules::V1_5, MoveRules::Uo] {
        let mut ps = PlayerState::spawn(Vec3::ZERO, 0.0);
        ps.rules = rules;
        ps.stance = Stance::Prone;
        ps.prone_torso_pitch = -0.003;
        // 45 down on the wire, 8192 shorts.
        ps.pitch = -view_rad(45.0);
        cap_prone_pitch(&mut ps, &w);
        assert_eq!(ps.view_pitch_correction, 0.0, "{rules:?}");
        assert_eq!(ps.view_capped[0], None, "{rules:?}");
    }
}

/// 1.1's caps place the view through the short-truncating
/// `AngleNormalize360` (0x33266) and `AngleNormalize180` (0x33346):
/// the yaw on the grid in 0..360, the pitch on the grid. 1.5 places the
/// pitch through `AngleNormalize180Accurate`, off the grid.
#[test]
fn a_capped_prone_view_lands_on_the_shorts_grid_on_1_1() {
    let w = flat();
    let w = MoveWorld::bare(&w);
    let on_grid = |deg: f32| {
        let s = f64::from(deg) * 65536.0 / 360.0;
        (s - s.round()).abs() < 1e-6
    };
    let mut ps = PlayerState::spawn(Vec3::ZERO, 0.0);
    ps.stance = Stance::Prone;
    ps.on_ground = true;
    ps.prone_direction = 10.3;
    ps.prone_torso_pitch = -4.1;
    // A view 147 degrees off the body and 60 down, both on the grid.
    ps.yaw = view_rad(-25000.0 * (360.0 / 65536.0));
    ps.pitch = -view_rad(10923.0 * (360.0 / 65536.0));
    update_prone_view(&mut ps, &PmInput::default(), &w, 0.008);
    let yaw = view_deg(ps.yaw);
    assert!((0.0..360.0).contains(&yaw) && on_grid(yaw), "yaw {yaw}");
    assert_eq!(
        yaw,
        short_normalize360(f64::from(ps.prone_direction - PRONE_YAWCAP))
    );
    assert_eq!(ps.view_capped[1], Some(yaw));
    let pitch = ps.view_capped[0].expect("the pitch is capped");
    assert!(on_grid(pitch), "pitch {pitch}");
    assert_eq!(pitch, short_normalize180(f64::from(-4.1f32 + 45.0)));

    ps.rules = MoveRules::V1_5;
    ps.pitch = -view_rad(10923.0 * (360.0 / 65536.0));
    cap_prone_pitch(&mut ps, &w);
    assert_eq!(ps.view_capped[0], Some(-4.1f32 + 45.0));
}

/// The 1.1 cgame passes `AngleNormalize180` the difference as a float
/// argument (cgame_mp_x86.dll 0x30007925-0x3000792b); 1.1d's
/// `AngleDelta` keeps it on the x87 stack (0x3ec9c). Here the kept
/// difference truncates to 1295 shorts and the float one rounds up to
/// 1296, so the Windows predictor reads a short more than the server.
#[test]
fn the_windows_1_1_predictor_stores_the_angle_delta() {
    let w = flat();
    let (a, b) = (-3.867_187_7_f32, -10.986_328_f32);
    let short = |n: f32| n * (360.0 / 65536.0);
    let delta =
        |w: &CollisionWorld| angle_delta(a, b, delta_stored(MoveRules::V1_1, &MoveWorld::bare(w)));
    assert_eq!(delta(&w), short(1295.0), "1.1d");
    w.set_arith(crate::patch::Arith::Msvc);
    assert_eq!(delta(&w), short(1296.0), "the 1.1 cgame");
}

/// `PM_LadderMove` (0x33daa) takes the legs' angle off `AngleDelta`
/// before the `(int)`: a ladder 30.001 degrees round from the view
/// reads 5461 shorts, 29.998 degrees, and the legs 29.
#[test]
fn the_ladder_legs_angle_truncates_through_the_shorts() {
    let w = crate::collision::test_world(&[]);
    let w = MoveWorld::bare(&w);
    let mut ps = PlayerState::spawn(Vec3::new(0.0, 0.0, 200.0), 0.0);
    ps.on_ground = false;
    let n = {
        let a = 210.001f64.to_radians();
        Vec3::new(a.cos() as f32, a.sin() as f32, 0.0)
    };
    ladder_move(&mut ps, &PmInput::default(), n, &w, 0.008, &mut Vec::new());
    assert_eq!(ps.movement_dir, 29);
}

/// `bg_ladder_yawcap` (1.1 0x32e7d, 1.5 0x26270): off the ground on
/// the rungs facing 90, a view at 200 is pushed back 10 degrees and
/// placed at 190, on the shorts grid on 1.1 and exactly on 1.5; on the
/// ground, or inside the cap, it stays (mp_ship-dm-ladder-turn*.txt).
#[test]
fn a_climber_looking_away_is_held_100_off_the_rungs() {
    let w = crate::collision::test_world(&[]);
    let w = MoveWorld::bare(&w);
    let at = |yaw: f32, ground: bool, rules: MoveRules| {
        let mut ps = PlayerState::spawn(Vec3::ZERO, yaw);
        ps.rules = rules;
        ps.on_ladder = true;
        ps.on_ground = ground;
        ps.ladder_normal = -Vec3::Y;
        cap_ladder_yaw(&mut ps, &w);
        (ps.view_yaw_correction, ps.view_capped[1])
    };
    let (c, yaw) = at(200.0, false, MoveRules::V1_1);
    assert!((c + 10.0).abs() < 0.01, "{c}");
    assert_eq!(yaw, Some(short_normalize360(190.0)));
    assert_eq!(at(200.0, false, MoveRules::V1_5).1, Some(190.0));
    assert_eq!(at(-30.0, false, MoveRules::Uo).1, Some(350.0));
    assert_eq!(at(200.0, true, MoveRules::V1_1), (0.0, None));
    assert_eq!(at(189.0, false, MoveRules::V1_1), (0.0, None));
    // The cap is the server's `bg_ladder_yawcap.integer`; 0 lifts it.
    let capped = |cap: i32| {
        let mut ps = PlayerState::spawn(Vec3::ZERO, 200.0);
        ps.rules = MoveRules::V1_5;
        ps.on_ladder = true;
        ps.ladder_normal = -Vec3::Y;
        ps.bg.ladder_yawcap = cap;
        cap_ladder_yaw(&mut ps, &w);
        ps.view_capped[1]
    };
    assert_eq!(capped(90), Some(180.0));
    assert_eq!(capped(0), None);
}

/// `SetClientViewAngle`'s prone arm (1.1 0x41e30, 1.5 0x3bf08): the
/// yaw held 45 either side of the body, the pitch from 45 down to 15 up
/// off the torso, both placed on the shorts grid.
#[test]
fn a_prone_player_given_a_view_is_held_in_the_cone() {
    let mut ps = PlayerState::spawn(Vec3::ZERO, 0.0);
    let free = prone_view_angle(&ps, [60.0, 100.0, 0.0]);
    assert_eq!(free, [60.0, 100.0, 0.0]);
    ps.stance = Stance::Prone;
    ps.prone_direction = 10.3;
    ps.prone_torso_pitch = -4.0;
    let held = prone_view_angle(&ps, [60.0, 100.0, 0.0]);
    assert_eq!(held[1], short_normalize360(f64::from(10.3f32 + 45.0)));
    assert_eq!(held[0], short_normalize180(f64::from(-4.0f32 + 15.0)));
    let held = prone_view_angle(&ps, [-60.0, -50.0, 0.0]);
    assert_eq!(held[1], short_normalize360(f64::from(10.3f32 - 45.0)));
    assert!(held[1] > 300.0);
    assert_eq!(held[0], short_normalize180(f64::from(-4.0f32 - 45.0)));
}

/// `PM_UpdatePronePitch`'s airborne arm (0x33434-0x33461): a prone body
/// in the air that no longer fits raises `pm_flags` 0x8000 and event
/// 141, and stays prone.
#[test]
fn an_airborne_prone_body_that_does_not_fit_forces_a_crouch() {
    let w = crate::collision::test_world(&[(
        Vec3::new(-40.0, 470.0, 50.0),
        Vec3::new(-20.0, 530.0, 300.0),
    )]);
    let w = MoveWorld::bare(&w);
    let mut ps = PlayerState::spawn(Vec3::ZERO, 0.0);
    let prone = PmInput {
        prone: true,
        ..Default::default()
    };
    tick(&mut ps, &PmInput::default(), &w, 20);
    tick(&mut ps, &prone, &w, 20);
    assert_eq!(ps.stance, Stance::Prone);
    assert!(stance_events(&mut ps, &prone, &w, 1).is_empty());

    ps.origin = Vec3::new(0.0, 500.0, 100.0);
    ps.on_ground = false;
    assert_eq!(
        stance_events(&mut ps, &prone, &w, 1),
        [EV_STANCE_FORCE_CROUCH]
    );
    assert!(ps.prone_blocked);
    assert_eq!(ps.stance, Stance::Prone);
}

/// `PM_CheckDuck` 0x3189c-0x318be: on a ladder the stance keys are taken
/// off the cmd and the client is told to stand.
#[test]
fn a_climber_s_stance_keys_force_a_stand() {
    let w = flat();
    let w = MoveWorld::bare(&w);
    let mut ps = PlayerState::spawn(Vec3::ZERO, 0.0);
    tick(&mut ps, &PmInput::default(), &w, 20);
    ps.on_ladder = true;
    let crouch = PmInput {
        crouch: true,
        stance_held: true,
        ..Default::default()
    };
    assert_eq!(
        stance_events(&mut ps, &crouch, &w, 1),
        [EV_STANCE_FORCE_STAND]
    );
    assert_eq!(ps.stance, Stance::Stand);
}

/// `PM_StepSlideMove`'s last block (0x3579c-0x3585c): a grounded step of
/// more than 3 units pushes the bob cycle on by 7 + 1.25 * min(step / 2,
/// 4), and a step across a quarter of the cycle lands a footstep. A
/// crouched body is not audible and keeps its cycle.
#[test]
fn a_step_up_pushes_the_bob_cycle() {
    let mat = 5;
    let stepped = |ducked: bool, bob: u8| {
        let mut ps = PlayerState::spawn(Vec3::new(0.0, 0.0, 8.0), 0.0);
        ps.on_ground = true;
        ps.ducked = ducked;
        ps.bob_cycle = bob;
        ps.ground_surface_flags = mat << 20;
        let mut events = Vec::new();
        step_view(&mut ps, &mut events, 0.0, 0.0, STEPSIZE);
        (ps.bob_cycle, events)
    };
    let (bob, events) = stepped(false, 60);
    assert_eq!(bob, 60 + 12);
    assert_eq!(events.len(), 2);
    assert_eq!(events[1].event, EV_FOOTSTEP_RUN_BASE + mat as i32);
    let (bob, events) = stepped(false, 0);
    assert_eq!((bob, events.len()), (12, 1), "no quarter crossed");
    let (bob, _) = stepped(true, 60);
    assert_eq!(bob, 60);
}

/// `BG_CheckProneValid`'s ground samples: a chest on a 4-unit slab and
/// hips and feet on the floor behind it pitch the torso up by
/// atan(4 / 24) and leave the legs flat.
#[test]
fn a_prone_body_bends_over_a_step_under_the_chest() {
    let w = crate::collision::test_world(&[
        (
            Vec3::new(-200.0, -200.0, -16.0),
            Vec3::new(200.0, 200.0, 0.0),
        ),
        (Vec3::new(-10.0, -200.0, 0.0), Vec3::new(200.0, 200.0, 4.0)),
    ]);
    let w = MoveWorld::bare(&w);
    let q = ProneQuery {
        origin: Vec3::new(0.0, 0.0, 4.125),
        yaw_deg: 0.0,
        entry: false,
        ground: true,
        length: PRONE_CHECK_LENGTH,
        rules: MoveRules::V1_1,
    };
    let body = check_prone(&w, q).expect("room to lie down");
    assert!(body.torso_height.abs() < 0.2, "{body:?}");
    let up = -(4.0f32 / 24.0).atan().to_degrees();
    assert!((body.torso_pitch - up).abs() < 0.2, "{body:?}");
    assert!(body.waist_pitch.abs() < 0.01, "{body:?}");
    // Without the ground samples the fit writes a flat body.
    let flat = check_prone(&w, ProneQuery { ground: false, ..q });
    assert_eq!(flat, Some(ProneBody::default()));
}

/// The entity's copy is whole once the eye has settled, follows the eye's
/// leg into prone, and is flat for a standing body.
#[test]
fn the_entity_scales_the_prone_body_by_the_eye_leg() {
    let mut ps = PlayerState::spawn(Vec3::ZERO, 0.0);
    ps.prone_body = ProneBody {
        torso_height: 2.0,
        torso_pitch: 45.0,
        waist_pitch: -315.0,
    };
    assert_eq!(ps.entity_prone_body(0, 1000), ProneBody::default());
    ps.stance = Stance::Prone;
    let whole = ProneBody {
        torso_height: 2.0,
        torso_pitch: 45.0,
        waist_pitch: 45.0,
    };
    assert_eq!(ps.entity_prone_body(0, 1000), whole);
    ps.view_lerp_target = VIEW_PRONE;
    ps.view_lerp_down = true;
    let len = view_lerp_length(VIEW_PRONE, true, false);
    let half = ps.entity_prone_body(1000, 1000 + len / 2);
    assert!((half.torso_pitch - 22.5).abs() < 0.1, "{half:?}");
}

/// Past the soft edge the body turns toward the view at 55 deg/s, and the
/// view is held inside 85 degrees of the body by a correction the caller
/// applies to `delta_angles`.
#[test]
fn a_prone_view_swings_the_body_and_is_capped() {
    let w = flat();
    let w = MoveWorld::bare(&w);
    let mut ps = PlayerState::spawn(Vec3::ZERO, 0.0);
    tick(&mut ps, &PmInput::default(), &w, 20);
    let prone = PmInput {
        prone: true,
        ..Default::default()
    };
    tick(&mut ps, &prone, &w, 20);
    assert_eq!(ps.stance, Stance::Prone);
    assert!((ps.prone_direction).abs() < 0.01);

    // Inside the soft edge the body does not move and nothing is clamped.
    ps.yaw = 60f32.to_radians();
    pmove(&mut ps, &prone, &w, 0.05, &[]);
    assert!(
        ps.prone_direction.abs() < 0.01,
        "the body holds under 80 degrees"
    );
    assert_eq!(ps.view_yaw_correction, 0.0, "60 degrees is inside the cap");

    // Past it, the body swings at the measured rate.
    ps.yaw = 150f32.to_radians();
    // A frame inside `MAX_FRAME_MS`, which pmove clamps dt to.
    pmove(&mut ps, &prone, &w, 0.05, &[]);
    // The body turns toward the view, which is at +150.
    let swung = PRONE_SWING_DEG_PER_SEC * 0.05;
    assert!(
        (ps.prone_direction - swung).abs() < 0.01,
        "body at {}, expected {swung}",
        ps.prone_direction
    );
    // 150 degrees off the body is past the 85 cap: the push is measured
    // off the body before the swing, the view placed on the cap after it.
    assert!(
        (ps.view_yaw_correction - (-150.0 + PRONE_YAWCAP)).abs() < 0.01,
        "correction {}",
        ps.view_yaw_correction
    );
    assert!(
        ((ps.yaw.to_degrees() - ps.prone_direction).abs() - PRONE_YAWCAP).abs() < 0.01,
        "the view ends exactly on the cap"
    );
}

/// A swing the body does not fit in is refused, and announced through
/// `pm_flags` 0x8000 while the view sits past the cap (0x331c4).
#[test]
fn a_refused_swing_past_the_cap_raises_prone_blocked() {
    // Lying facing +x, the body runs along -x; a wall 20 units off on -y
    // stops it from turning toward +y.
    let w = crate::collision::test_world(&[(
        Vec3::new(-80.0, -60.0, -8.0),
        Vec3::new(80.0, -20.0, 72.0),
    )]);
    let w = MoveWorld::bare(&w);
    let mut ps = PlayerState::spawn(Vec3::new(0.0, 0.0, 0.125), 0.0);
    let prone = PmInput {
        prone: true,
        ..Default::default()
    };
    tick(&mut ps, &prone, &w, 20);
    assert_eq!(ps.stance, Stance::Prone);
    assert!(!ps.prone_blocked);
    let mut blocked = false;
    for _ in 0..200 {
        // The cmd keeps asking for a view 100 degrees off the body.
        ps.yaw = (ps.prone_direction + 100.0).to_radians();
        pmove(&mut ps, &prone, &w, 0.05, &[]);
        blocked |= ps.prone_blocked;
    }
    assert!(blocked, "the body stopped at {}", ps.prone_direction);
    assert!(ps.prone_direction < 90.0, "the wall held the body");
}

/// A crawling player's body follows the view inside the soft edge, which
/// a still one's does not (`PM_UpdateViewAngles` 0x330c0).
#[test]
fn a_crawl_swings_the_body_inside_the_soft_edge() {
    let w = flat();
    let w = MoveWorld::bare(&w);
    let mut ps = PlayerState::spawn(Vec3::ZERO, 0.0);
    tick(&mut ps, &PmInput::default(), &w, 20);
    let prone = PmInput {
        prone: true,
        ..Default::default()
    };
    tick(&mut ps, &prone, &w, 60);
    ps.yaw = 30f32.to_radians();
    let crawl = PmInput {
        right: 1.0,
        ..prone
    };
    pmove(&mut ps, &crawl, &w, 0.05, &[]);
    let swung = PRONE_SWING_DEG_PER_SEC * 0.05;
    assert!(
        (ps.prone_direction - swung).abs() < 0.01,
        "body at {}, expected {swung}",
        ps.prone_direction
    );
}

/// The prone view pitches at most 45 degrees off the ground's pitch along
/// it, level ground here, and the excess goes to `delta_angles[0]`.
#[test]
fn a_prone_view_pitch_is_capped_off_the_ground() {
    let w = flat();
    let w = MoveWorld::bare(&w);
    let mut ps = PlayerState::spawn(Vec3::ZERO, 0.0);
    tick(&mut ps, &PmInput::default(), &w, 20);
    let prone = PmInput {
        prone: true,
        ..Default::default()
    };
    tick(&mut ps, &prone, &w, 60);
    assert_eq!(ps.prone_torso_pitch, 0.0);
    // Camera convention: positive up. 60 down is past the cap by 15.
    ps.pitch = (-60f32).to_radians();
    pmove(&mut ps, &prone, &w, 0.05, &[]);
    assert!((ps.pitch.to_degrees() + PRONE_PITCHCAP).abs() < 0.01);
    assert!(
        (ps.view_pitch_correction + 15.0).abs() < 0.01,
        "correction {}",
        ps.view_pitch_correction
    );
}

/// The takeoff anim each arm raises (`Takeoff`): the jump forced and
/// after the move's timers drop, the dive's kickoff unforced and ahead
/// of them, a step off a drop past 64 forced, a shorter drop none.
#[test]
fn each_takeoff_raises_its_own_anim_event() {
    let w = flat();
    let w = MoveWorld::bare(&w);
    let run = PmInput {
        forward: 1.0,
        ..Default::default()
    };
    let mut ps = PlayerState::spawn(Vec3::ZERO, 0.0);
    tick(&mut ps, &run, &w, 30);
    pmove(&mut ps, &PmInput { jump: true, ..run }, &w, 0.008, &[]);
    assert_eq!(
        ps.takeoff,
        Some(Takeoff {
            forced: true,
            back: false,
            before_timers: false
        })
    );
    pmove(&mut ps, &run, &w, 0.008, &[]);
    assert_eq!(ps.takeoff, None, "cleared by the next move");

    let mut ps = PlayerState::spawn(Vec3::ZERO, 0.0);
    let back = PmInput {
        forward: -1.0,
        ..Default::default()
    };
    tick(&mut ps, &back, &w, 30);
    pmove(
        &mut ps,
        &PmInput {
            prone: true,
            ..back
        },
        &w,
        0.008,
        &[],
    );
    assert_eq!(
        ps.takeoff,
        Some(Takeoff {
            forced: false,
            back: true,
            before_timers: true
        })
    );

    for (height, lift) in [(100.0, true), (40.0, false)] {
        let shelf = test_world(&[(
            Vec3::new(-1024.0, -1024.0, 0.0),
            Vec3::new(0.0, 1024.0, height),
        )]);
        let shelf = MoveWorld::bare(&shelf);
        let mut ps = PlayerState::spawn(Vec3::new(-40.0, 0.0, height + 0.125), 0.0);
        let mut raised = None;
        for _ in 0..60 {
            pmove(&mut ps, &run, &shelf, 0.008, &[]);
            raised = raised.or(ps.takeoff);
        }
        assert!(ps.origin.x > 30.0 && !ps.on_ground, "{:?}", ps.origin);
        let want = lift.then_some(Takeoff {
            forced: true,
            back: false,
            before_timers: false,
        });
        assert_eq!(raised, want, "a {height} unit drop");
    }
}

/// A prone press on a player moving forward throws it into the air at the
/// standing dive's speed, and holds `pm_flags` 0x4 while the key is down.
#[test]
fn a_prone_press_while_running_dives() {
    let w = flat();
    let w = MoveWorld::bare(&w);
    let mut ps = PlayerState::spawn(Vec3::ZERO, 0.0);
    let run = PmInput {
        forward: 1.0,
        ..Default::default()
    };
    tick(&mut ps, &run, &w, 30);
    assert!(ps.on_ground);
    let dive = PmInput { prone: true, ..run };
    pmove(&mut ps, &dive, &w, 0.008, &[]);
    assert_eq!(ps.stance, Stance::Prone);
    assert!(ps.prone_dive);
    assert!(!ps.on_ground);
    // The takeoff, sqrt(2 * DIVE_HEIGHT_STAND * GRAVITY), less one
    // 8 ms tick of gravity, snapped.
    assert!(ps.velocity.z == 227.0, "vz {}", ps.velocity.z);
    // A sideways press does not dive.
    let mut ps = PlayerState::spawn(Vec3::ZERO, 0.0);
    let strafe = PmInput {
        right: 1.0,
        ..Default::default()
    };
    tick(&mut ps, &strafe, &w, 30);
    pmove(
        &mut ps,
        &PmInput {
            prone: true,
            ..strafe
        },
        &w,
        0.008,
        &[],
    );
    assert!(!ps.prone_dive);
    assert!(ps.on_ground);
}

#[test]
fn a_standing_jump_needs_no_forward_input() {
    // The forwardmove gate once read off fn 0x316F4 @0x31CC0 is not there:
    // a retail server jumps a probe holding upmove alone
    // (docs/research/cod11-mantle.md, "Jumps").
    let w = flat();
    let w = MoveWorld::bare(&w);
    let mut ps = PlayerState::spawn(Vec3::ZERO, 0.0);
    tick(&mut ps, &PmInput::default(), &w, 50); // settle
    let mut apex = ps.origin.z;
    for _ in 0..80 {
        pmove(
            &mut ps,
            &PmInput {
                jump: true,
                ..Default::default()
            },
            &w,
            0.02,
            &[],
        );
        apex = apex.max(ps.origin.z);
    }
    assert!(
        apex == 39.035995,
        "standing apex {apex}, expected ~{JUMP_HEIGHT}"
    );
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
fn the_velocity_snap_rounds_to_nearest_even_and_stores_plus_zero() {
    let mut ps = PlayerState::spawn(Vec3::ZERO, 0.0);
    ps.velocity = Vec3::new(183.5, 26.5, -0.4);
    snap_velocity(&mut ps);
    assert_eq!(ps.velocity, Vec3::new(184.0, 26.0, 0.0));
    assert_eq!(ps.velocity.z.to_bits(), 0, "a snapped -0.4 is +0.0");
    ps.velocity = Vec3::new(-2.5, 137.6, -196.2);
    snap_velocity(&mut ps);
    assert_eq!(ps.velocity, Vec3::new(-2.0, 138.0, -196.0));
}

#[test]
fn a_frame_that_moved_under_half_its_velocity_takes_the_move_as_velocity() {
    let world = crate::collision::synthetic_world(
        &[("textures/test/solid", crate::collision::CONTENTS_SOLID, 0)],
        &[(0, [-64.0, -64.0, -1100.0], [64.0, 64.0, -1000.0])],
    );
    let w = MoveWorld::bare(&world);
    let mut ps = PlayerState::spawn(Vec3::new(1.0, 0.0, 0.0), 0.0);
    ps.move_start = Vec3::ZERO;
    // 1 unit in 8 ms is 125 u/s: 200 is under twice that and stays.
    ps.velocity = Vec3::new(200.0, 0.0, 0.0);
    clamp_velocity_to_move(&mut ps, &w, 0.008);
    assert_eq!(ps.velocity.x, 200.0);
    ps.velocity = Vec3::new(300.0, 0.0, 40.0);
    clamp_velocity_to_move(&mut ps, &w, 0.008);
    assert!(ps.velocity.abs_diff_eq(Vec3::new(125.0, 0.0, 0.0), 1e-3));
}

/// A fall from rest moves exactly half its end velocity, and 1.1d's
/// stack keeps the tie or takes the move by the origin's last bits:
/// `probe_spawnview`'s 66 + 34 ms spawn thinks on mp_carentan
/// (cod11-gametypes-re-bel.md 8.5) fell 3.0888 from these heights and
/// 4.0068 from the rest, and 1.5b's stored floats took it only at -2
/// and ended the fall from 4 at -0.00680161 (cod15-movement.md 7); the
/// Windows clients fall as 1.1d on every patch (8).
#[test]
fn a_fall_from_rest_ties_the_tail_by_the_start_height() {
    let world = crate::collision::synthetic_world(
        &[("textures/test/solid", crate::collision::CONTENTS_SOLID, 0)],
        &[(0, [-64.0, -64.0, -1100.0], [64.0, 64.0, -1000.0])],
    );
    let w = MoveWorld::bare(&world);
    let end_as = |z: f32, rules: MoveRules, arith: crate::patch::Arith| {
        world.set_arith(arith);
        let mut ps = PlayerState::spawn(Vec3::new(0.0, 0.0, z), 0.0);
        ps.rules = rules;
        pmove(&mut ps, &PmInput::default(), &w, 0.066, &[]);
        pmove(&mut ps, &PmInput::default(), &w, 0.034, &[]);
        ps.origin.z
    };
    let end = |z: f32, rules: MoveRules| {
        let linux = if rules.v15() {
            crate::patch::Arith::Stored
        } else {
            crate::patch::Arith::Kept
        };
        end_as(z, rules, linux)
    };
    let fall = |z: f32, rules: MoveRules| ((z - end(z, rules)) * 10000.0).round() / 10000.0;
    // The Windows client stores only y and z of the tail and keeps the
    // trace's end: on every patch it falls as 1.1d does
    // (cod15-movement.md 8).
    for rules in [MoveRules::V1_1, MoveRules::V1_5, MoveRules::Uo] {
        let msvc = |z: f32| {
            let e = end_as(z, rules, crate::patch::Arith::Msvc);
            ((z - e) * 10000.0).round() / 10000.0
        };
        for z in [-2.0, -1.0, 0.0, 1.0, 2.0, 4.0] {
            assert_eq!(msvc(z), 3.0888, "{rules:?} on Windows from {z}");
        }
        for z in [-4.0, 8.0, 16.0, 100.0] {
            assert_eq!(msvc(z), 4.0068, "{rules:?} on Windows from {z}");
        }
    }
    for z in [-2.0, -1.0, 0.0, 1.0, 2.0, 4.0] {
        assert_eq!(fall(z, MoveRules::V1_1), 3.0888, "1.1 from {z}");
    }
    for z in [-4.0, 8.0, 16.0, 100.0] {
        assert_eq!(fall(z, MoveRules::V1_1), 4.0068, "1.1 from {z}");
    }
    for z in [-4.0, -1.0, 0.0, 1.0, 2.0, 4.0, 8.0, 16.0, 100.0] {
        assert_eq!(fall(z, MoveRules::V1_5), 4.0068, "1.5 from {z}");
    }
    assert_eq!(fall(-2.0, MoveRules::V1_5), 3.0888);
    assert_eq!((end(4.0, MoveRules::V1_5) * 1e8).round(), -680161.0);
}

/// At 8 ms a frame's gravity is 6.4 and the snap keeps 6 of it, so a
/// 125 fps jump tops out over two units above the 39 a 20 ms one reaches.
#[test]
fn a_125_fps_jump_goes_higher() {
    let apex = |dt: f32| {
        let w = flat();
        let w = MoveWorld::bare(&w);
        let mut ps = PlayerState::spawn(Vec3::ZERO, 0.0);
        tick(&mut ps, &PmInput::default(), &w, 50);
        let jump = PmInput {
            jump: true,
            ..Default::default()
        };
        pmove(&mut ps, &jump, &w, dt, &[]);
        let mut apex = ps.origin.z;
        for _ in 0..(1.6 / dt) as usize {
            pmove(&mut ps, &PmInput::default(), &w, dt, &[]);
            apex = apex.max(ps.origin.z);
        }
        apex
    };
    let (fast, slow) = (apex(0.008), apex(0.02));
    assert!(slow == 39.035995, "20 ms apex {slow}");
    assert!(fast - slow > 1.5, "8 ms apex {fast}, 20 ms {slow}");
}

/// A held key jumps once: the latch (`pm_flags` 0x8) holds until the key
/// is released, and a press inside 500 ms of the last jump is refused
/// (`PM_CheckJump` 0x2ebb3, 0x2ec0d).
#[test]
fn a_held_jump_does_not_chain_and_a_repress_waits_out_the_cooldown() {
    let w = flat();
    let w = MoveWorld::bare(&w);
    let dt = 1.0 / 125.0;
    let hop = PmInput {
        forward: 1.0,
        jump: true,
        ..Default::default()
    };
    let run = PmInput { jump: false, ..hop };
    let mut ps = PlayerState::spawn(Vec3::ZERO, 0.0);
    tick(&mut ps, &PmInput::default(), &w, 50); // settle
    let mut launches = 0;
    for _ in 0..300 {
        pmove(&mut ps, &hop, &w, dt, &[]);
        launches += usize::from(ps.jumped);
    }
    assert_eq!(launches, 1, "a held key jumps once");
    assert!(ps.on_ground && ps.jump_latched);

    // Released for a frame and pressed again, well past 500 ms: jumps.
    pmove(&mut ps, &run, &w, dt, &[]);
    assert!(!ps.jump_latched);
    pmove(&mut ps, &hop, &w, dt, &[]);
    assert!(ps.jumped);

    // Land, then re-press inside the cooldown: a full flight outlasts the
    // 500 ms, so the jump is cut short by taking its vertical speed away.
    let mut ps = PlayerState::spawn(Vec3::ZERO, 0.0);
    tick(&mut ps, &PmInput::default(), &w, 50);
    pmove(&mut ps, &hop, &w, dt, &[]);
    assert!(ps.jumped);
    ps.velocity.z = 0.0;
    tick(&mut ps, &run, &w, 20);
    assert!(ps.on_ground, "landed");
    assert!(ps.since_jump_ms < JUMP_COOLDOWN_MS);
    pmove(&mut ps, &hop, &w, dt, &[]);
    assert!(!ps.jumped, "a jump inside 500 ms of the last is refused");
    while ps.since_jump_ms <= JUMP_COOLDOWN_MS - 1.0 {
        pmove(&mut ps, &run, &w, dt, &[]);
    }
    pmove(&mut ps, &hop, &w, dt, &[]);
    assert!(ps.jumped, "and taken once the 500 ms are up");
}

/// The step tail: the vertical jump the step added rides `EV_STEP_VIEW`
/// with a +128 bias, and the frame's velocity is scaled by how much of
/// the step height it used (docs/research/cod11-mantle.md, "The step
/// event and the velocity scale").
#[test]
fn a_step_announces_itself_and_costs_speed() {
    let w = test_world(&[(Vec3::new(50.0, -200.0, 0.0), Vec3::new(1024.0, 200.0, 6.0))]);
    let w = MoveWorld::bare(&w);
    let mut ps = PlayerState::spawn(Vec3::new(30.0, 0.0, 0.0), 0.0);
    ps.on_ground = true;
    ps.ground_normal = Vec3::Z;
    let mut events = Vec::new();
    for _ in 0..20 {
        ps.velocity = Vec3::new(200.0, 0.0, 0.0);
        step_slide_move(
            &mut ps,
            &w,
            1.0 / 125.0,
            false,
            MASK_PLAYERSOLID,
            Some(&mut events),
        );
        if !events.is_empty() {
            break;
        }
    }
    assert_eq!(
        events,
        vec![PmEvent {
            event: EV_STEP_VIEW,
            parm: 128 + 6,
        }],
        "a 6-unit step, biased by 128"
    );
    // 0.2 + 0.8 * (1 - 6/18)
    assert!(
        (ps.velocity.x - 200.0 * (0.2 + 0.8 * (1.0 - 6.0 / STEPSIZE))).abs() < 0.5,
        "the step cost a third of the frame's speed, got {}",
        ps.velocity.x
    );
}

/// The tail is out of reach in the air: retail only gets there on a
/// grounded frame (@0x350ec).
#[test]
fn an_airborne_step_announces_nothing() {
    let w = test_world(&[(Vec3::new(50.0, -200.0, 0.0), Vec3::new(1024.0, 200.0, 6.0))]);
    let w = MoveWorld::bare(&w);
    let mut ps = PlayerState::spawn(Vec3::new(30.0, 0.0, 0.0), 0.0);
    let mut events = Vec::new();
    for _ in 0..20 {
        ps.velocity = Vec3::new(200.0, 0.0, 0.0);
        step_slide_move(
            &mut ps,
            &w,
            1.0 / 125.0,
            false,
            MASK_PLAYERSOLID,
            Some(&mut events),
        );
    }
    assert!(events.is_empty(), "{events:?}");
}

/// In the air only a jump steps, and never past its origin
/// (`PM_StepSlideMove` 0x35057-0x35112, 0x35450; docs/research/cod11-mantle.md,
/// "The jump's step").
#[test]
fn an_airborne_step_needs_a_jump_and_stops_at_its_origin() {
    // A ledge 14 high 20 units ahead of an airborne player at 10.
    let w = test_world(&[(Vec3::new(50.0, -200.0, 0.0), Vec3::new(1024.0, 200.0, 14.0))]);
    let w = MoveWorld::bare(&w);
    let fly = |jump_origin_z: f32| {
        let mut ps = PlayerState::spawn(Vec3::new(30.0, 0.0, 10.0), 0.0);
        ps.jump_origin_z = jump_origin_z;
        let mut events = Vec::new();
        ps.velocity = Vec3::new(200.0, 0.0, 0.0);
        step_slide_move(
            &mut ps,
            &w,
            0.05,
            false,
            MASK_PLAYERSOLID,
            Some(&mut events),
        );
        (ps, events)
    };
    // A fall, no jump: held at the face.
    let (ps, events) = fly(0.0);
    assert!(
        ps.origin.z == 10.0 && ps.origin.x < 36.0,
        "at {}",
        ps.origin
    );
    assert!(events.is_empty());
    // Under a jump's origin: steps onto the ledge and announces it.
    let (ps, events) = fly(39.0);
    assert!(
        (ps.origin.z - 14.0).abs() < 0.2 && ps.origin.x > 36.0,
        "at {}",
        ps.origin
    );
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].event, EV_STEP_VIEW);
    // An origin the ledge top is past: the step is reverted.
    let (ps, _) = fly(13.0);
    assert!(
        ps.origin.z == 10.0 && ps.origin.x < 36.0,
        "at {}",
        ps.origin
    );
    // Already above the origin: no step at all.
    let (ps, _) = fly(9.0);
    assert!(
        ps.origin.z == 10.0 && ps.origin.x < 36.0,
        "at {}",
        ps.origin
    );
}

#[test]
fn prone_steps_lower_than_standing() {
    // retail picks the 10-unit step height off pm_flags bit 0x1
    // (PM_StepSlideMove @0x35045), 18 otherwise (@0x35034)
    let w = test_world(&[(Vec3::new(50.0, -200.0, 0.0), Vec3::new(1024.0, 200.0, 14.0))]);
    let w = MoveWorld::bare(&w);
    let mut stand = PlayerState::spawn(Vec3::new(30.0, 0.0, 0.0), 0.0);
    stand.on_ground = true;
    stand.velocity = Vec3::new(200.0, 0.0, 0.0);
    for _ in 0..20 {
        stand.velocity.x = 200.0;
        step_slide_move(&mut stand, &w, 1.0 / 125.0, false, MASK_PLAYERSOLID, None);
    }
    assert!(
        (stand.origin.z - 14.0).abs() < 0.5,
        "standing should step 14, at {}",
        stand.origin
    );

    let mut prone = PlayerState::spawn(Vec3::new(30.0, 0.0, 0.0), 0.0);
    prone.on_ground = true;
    prone.stance = Stance::Prone;
    for _ in 0..20 {
        prone.velocity.x = 200.0;
        step_slide_move(&mut prone, &w, 1.0 / 125.0, false, MASK_PLAYERSOLID, None);
    }
    assert!(
        prone.origin.z < 1.0 && prone.origin.x < 36.0,
        "prone must not step 14, at {}",
        prone.origin
    );
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

/// `PM_SlideMove`'s same-plane test is `> 0.999` (1.1d rodata 0x70d7c,
/// 1.5b 0x75b80, UO 0x9ba40): a 5 degree ramp met from the flat floor
/// (0.9962 against the ground plane) is a new plane and clips the
/// velocity up the ramp, where a 0.99 test nudged the body off it.
#[test]
fn a_shallow_ramp_met_from_the_floor_is_a_new_plane() {
    let w = crate::collision::ramp_test_world(5.0, 0.0, 300.0);
    let w = MoveWorld::bare(&w);
    let mut ps = PlayerState::spawn(Vec3::new(-60.0, 0.0, 1.0), 0.0);
    tick(&mut ps, &PmInput::default(), &w, 40);
    assert!(ps.on_ground && ps.ground_plane.is_some(), "{}", ps.origin);
    ps.velocity = Vec3::new(320.0, 0.0, 0.0);
    let slide = slide_move(&mut ps, &w, 0.25, false, MASK_PLAYERSOLID);
    assert!(slide.blocked);
    assert!(ps.velocity.z > 20.0, "velocity {}", ps.velocity);
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
fn steps_up_low_ledge_but_not_high_one() {
    // z=16 ledge ahead (+X), z=40 ledge behind (-X), both reaching the
    // floor edge: 300 ticks covers ~440 units
    let w = test_world(&[
        (Vec3::new(50.0, -200.0, 0.0), Vec3::new(1024.0, 200.0, 16.0)),
        (
            Vec3::new(-1024.0, -200.0, 0.0),
            Vec3::new(-50.0, 200.0, 40.0),
        ),
    ]);
    let w = MoveWorld::bare(&w);
    let mut ps = PlayerState::spawn(Vec3::ZERO, 0.0);
    tick(
        &mut ps,
        &PmInput {
            forward: 1.0,
            ..Default::default()
        },
        &w,
        300,
    );
    assert!(
        ps.origin.x > 60.0 && (ps.origin.z - 16.0).abs() < 0.5,
        "should stand on the ledge, at {}",
        ps.origin
    );

    let mut ps = PlayerState::spawn(Vec3::ZERO, 180.0);
    tick(
        &mut ps,
        &PmInput {
            forward: 1.0,
            ..Default::default()
        },
        &w,
        300,
    );
    assert!(
        ps.origin.x > -50.0 - HALF_WIDTH - 1.0 && ps.origin.z < 1.0,
        "blocked by the wall, at {}",
        ps.origin
    );
}

#[test]
fn slides_along_wall() {
    let w = test_world(&[(Vec3::new(50.0, -400.0, 0.0), Vec3::new(100.0, 400.0, 100.0))]);
    let w = MoveWorld::bare(&w);
    let mut ps = PlayerState::spawn(Vec3::ZERO, 0.0);
    tick(
        &mut ps,
        &PmInput {
            forward: 1.0,
            right: 0.3,
            ..Default::default()
        },
        &w,
        400,
    );
    assert!(ps.origin.x < 50.0 - HALF_WIDTH + 1.0);
    // retail constants (accel 9, stopspeed floor 100) give a lower
    // tangential plateau than the old Q3-derived ones
    assert!(
        ps.origin.y.abs() > 30.0,
        "should have slid along the wall, at {}",
        ps.origin
    );
}

#[test]
fn a_running_timer_keeps_the_velocity_the_slide_clipped() {
    let w = test_world(&[(Vec3::new(50.0, -400.0, 0.0), Vec3::new(100.0, 400.0, 100.0))]);
    let w = MoveWorld::bare(&w);
    let slide = |timer: f32| {
        let mut ps = PlayerState::spawn(Vec3::new(30.0, 0.0, 1.0), 0.0);
        ps.velocity = Vec3::new(300.0, 300.0, 0.0);
        ps.knockback_ms = timer;
        slide_move(&mut ps, &w, 0.05, false, MASK_PLAYERSOLID);
        ps
    };
    let free = slide(0.0);
    assert!(
        free.velocity.x <= 0.0,
        "the wall took the x: {}",
        free.velocity
    );
    let held = slide(500.0);
    assert_eq!(held.origin, free.origin, "the move itself is the same");
    assert_eq!(held.velocity, Vec3::new(300.0, 300.0, 0.0));
}

#[test]
fn crouch_lowers_speed_and_ceiling_blocks_standing() {
    // z=60 ceiling: crouch fits, standing does not; wide enough for the
    // ~240-unit run
    let w = test_world(&[(
        Vec3::new(-1024.0, -1024.0, 60.0),
        Vec3::new(1024.0, 1024.0, 100.0),
    )]);
    let w = MoveWorld::bare(&w);
    let mut ps = PlayerState::spawn(Vec3::ZERO, 0.0);
    ps.stance = Stance::Crouch;
    ps.ducked = true;
    let crouched = PmInput {
        forward: 1.0,
        crouch: true,
        ..Default::default()
    };
    tick(&mut ps, &crouched, &w, 250);
    let h = ps.velocity.truncate().length();
    assert!(h == 123.0, "crouch speed {h}");
    tick(&mut ps, &PmInput::default(), &w, 10);
    assert_eq!(ps.stance, Stance::Crouch);
}

#[test]
fn prone_is_slowest() {
    let w = flat();
    let w = MoveWorld::bare(&w);
    let mut ps = PlayerState::spawn(Vec3::ZERO, 0.0);
    let prone = PmInput {
        forward: 1.0,
        prone: true,
        ..Default::default()
    };
    tick(&mut ps, &prone, &w, 400);
    let h = ps.velocity.truncate().length();
    assert!(h == 29.0, "prone speed {h}");
}

/// `PM_UpdateLean`'s ramps: a full lean in 350 ms, back in 280.
#[test]
fn lean_ramps_up_clamps_and_returns() {
    let w = flat();
    let w = MoveWorld::bare(&w);
    let mut ps = PlayerState::spawn(Vec3::ZERO, 0.0);
    tick(&mut ps, &PmInput::default(), &w, 50); // settle
    let lean_r = PmInput {
        lean_right: true,
        ..Default::default()
    };
    tick(&mut ps, &lean_r, &w, 43);
    assert!(
        (ps.lean - 43.0 * 8.0 / 350.0).abs() < 1e-4,
        "lean {}",
        ps.lean
    );
    tick(&mut ps, &lean_r, &w, 20);
    assert_eq!(ps.lean, 1.0);
    tick(&mut ps, &PmInput::default(), &w, 20);
    assert!(
        (ps.lean - (1.0 - 160.0 / 280.0)).abs() < 1e-4,
        "lean {}",
        ps.lean
    );
    tick(&mut ps, &PmInput::default(), &w, 20);
    assert_eq!(ps.lean, 0.0);
}

/// Off the ground the lean keys do nothing and a lean eases back.
#[test]
fn an_airborne_player_does_not_lean() {
    let w = flat();
    let w = MoveWorld::bare(&w);
    let mut ps = PlayerState::spawn(Vec3::new(0.0, 0.0, 200.0), 0.0);
    ps.lean = 0.5;
    let lean_r = PmInput {
        lean_right: true,
        ..Default::default()
    };
    tick(&mut ps, &lean_r, &w, 5);
    assert!(!ps.on_ground);
    assert!(
        (ps.lean - (0.5 - 40.0 / 280.0)).abs() < 1e-4,
        "lean {}",
        ps.lean
    );
}

#[test]
fn lean_left_is_negative_and_wall_limits_lean() {
    // Wall on +Y (the left side at yaw 0), past y=12 so the lean box does
    // not start inside it, which would pin fraction at 0.
    let w = test_world(&[(Vec3::new(-200.0, 20.0, 0.0), Vec3::new(200.0, 70.0, 200.0))]);
    let w = MoveWorld::bare(&w);
    let mut ps = PlayerState::spawn(Vec3::ZERO, 0.0);
    tick(&mut ps, &PmInput::default(), &w, 50);
    let lean_l = PmInput {
        lean_left: true,
        ..Default::default()
    };
    tick(&mut ps, &lean_l, &w, 125);
    assert!(ps.lean < 0.0);
    assert!(ps.lean > -0.9, "wall should limit lean, got {}", ps.lean);
}

/// Prone tops the lean out at 0.25 (`PM_UpdateLean`'s stance scale).
#[test]
fn prone_caps_lean_at_a_quarter() {
    let w = flat();
    let w = MoveWorld::bare(&w);
    let mut ps = PlayerState::spawn(Vec3::ZERO, 0.0);
    let input = PmInput {
        prone: true,
        lean_right: true,
        ..Default::default()
    };
    tick(&mut ps, &input, &w, 125);
    assert_eq!(ps.stance, Stance::Prone);
    assert_eq!(ps.lean, 0.25);
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

/// A full right lean at yaw 0 moves the eye `20 * right(0, 0, 16)`:
/// 19.2 to -Y and 5.5 down (`AddLeanToPosition`).
#[test]
fn view_reflects_stance_and_lean() {
    let w = flat();
    let w = MoveWorld::bare(&w);
    let mut ps = PlayerState::spawn(Vec3::ZERO, 0.0);
    tick(&mut ps, &PmInput::default(), &w, 50);
    let v = ps.view();
    assert!(v.eye.z == 60.0);
    let lean_r = PmInput {
        lean_right: true,
        ..Default::default()
    };
    tick(&mut ps, &lean_r, &w, 125);
    assert_eq!(ps.lean, 1.0);
    let d = ps.view().eye - (ps.origin + Vec3::Z * ps.view_height());
    let (s, c) = 16f32.to_radians().sin_cos();
    assert!(
        (d - Vec3::new(0.0, -20.0 * c, -20.0 * s)).length() < 1e-4,
        "{d}"
    );
}

// --- water ---

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

/// Dry floor at z=0 plus a playerclip ladder wall (SURF_LADDER) spanning
/// x 50..54, y -200..200, z 0..220.
fn ladder_world() -> CollisionWorld {
    crate::collision::synthetic_world(
        &[
            ("textures/test/solid", crate::collision::CONTENTS_SOLID, 0),
            // CONTENTS_PLAYERCLIP is private to collision.rs
            ("textures/common/ladder", 0x10000, 0x8),
        ],
        &[
            (0, [-600.0, -300.0, -16.0], [600.0, 300.0, 0.0]),
            (1, [50.0, -200.0, 0.0], [54.0, 200.0, 220.0]),
        ],
    )
}

#[test]
fn grabs_a_ladder_and_climbs_it_holding_forward() {
    let w = ladder_world();
    let w = MoveWorld::bare(&w);
    let mut ps = PlayerState::spawn(Vec3::new(20.0, 0.0, 0.2), 0.0);
    tick(
        &mut ps,
        &PmInput {
            forward: 1.0,
            ..Default::default()
        },
        &w,
        150,
    );
    assert!(ps.on_ladder, "should be on the ladder at {}", ps.origin);
    // hugging the face: center stops a half-width short of x=50
    assert!(
        (30.0..40.0).contains(&ps.origin.x),
        "should press against the face, at {}",
        ps.origin
    );
    assert!(ps.origin.z > 30.0, "should have climbed, at {}", ps.origin);
    assert!(ps.velocity.z > 20.0, "climb vz {}", ps.velocity.z);
}

#[test]
fn no_ladder_grab_when_idle_or_backing_off() {
    let w = ladder_world();
    let w = MoveWorld::bare(&w);
    let mut ps = PlayerState::spawn(Vec3::new(33.0, 0.0, 0.2), 0.0);
    tick(&mut ps, &PmInput::default(), &w, 30);
    assert!(!ps.on_ladder, "idle at the base must not grab");
    tick(
        &mut ps,
        &PmInput {
            forward: -1.0,
            ..Default::default()
        },
        &w,
        30,
    );
    assert!(!ps.on_ladder, "backing away must not grab");
    assert!(ps.origin.x < 30.0, "moved away, at {}", ps.origin);
}

#[test]
fn climb_rate_follows_pitch() {
    let w = ladder_world();
    let w = MoveWorld::bare(&w);
    let input = PmInput {
        forward: 1.0,
        ..Default::default()
    };
    // two units off the face; settled climb speeds, no friction on the
    // rungs: 0.5 * upscale * 190, 0.625 level, clamped to 1 at 30 up
    // (snapped to whole units at the frame's end)
    let mut level = PlayerState::spawn(Vec3::new(33.0, 0.0, 5.0), 0.0);
    tick(&mut level, &input, &w, 100);
    let mut up = PlayerState::spawn(Vec3::new(33.0, 0.0, 5.0), 0.0);
    up.pitch = 30.0f32.to_radians();
    tick(&mut up, &input, &w, 100);
    assert!(level.on_ladder && up.on_ladder);
    assert_eq!(level.velocity.z, 59.0, "level climb");
    assert_eq!(up.velocity.z, 95.0, "climb looking up");
    assert!(up.origin.z > level.origin.z);
}

#[test]
fn diagonal_input_climbs_slower_than_pure_forward() {
    // retail runs the wish through PM_CmdScale, so W+D must not beat the
    // vertical rate of W alone (up 30 deg for a climb-dominated wish)
    let w = ladder_world();
    let w = MoveWorld::bare(&w);
    let climb = |right: f32| {
        let mut ps = PlayerState::spawn(Vec3::new(33.0, 0.0, 5.0), 0.0);
        ps.pitch = 30.0f32.to_radians();
        tick(
            &mut ps,
            &PmInput {
                forward: 1.0,
                right,
                ..Default::default()
            },
            &w,
            100,
        );
        assert!(ps.on_ladder);
        ps.velocity.z
    };
    let solo = climb(0.0);
    let diag = climb(1.0);
    let ratio = diag / solo;
    // Unsnapped the ratio is the cmd scale's 1/sqrt(2); at 8 ms steps the
    // velocity snap takes a rounding share off both terminal speeds,
    // 53.4 -> 50 and 37.8 -> 34.
    assert!(
        diag < solo && (0.6..0.95).contains(&ratio),
        "W+D must climb slower than W alone, {solo} -> {diag} (x{ratio})"
    );
}

#[test]
fn releasing_input_mid_climb_hangs_without_sliding() {
    let w = ladder_world();
    let w = MoveWorld::bare(&w);
    let mut ps = PlayerState::spawn(Vec3::new(20.0, 0.0, 0.2), 0.0);
    tick(
        &mut ps,
        &PmInput {
            forward: 1.0,
            ..Default::default()
        },
        &w,
        100,
    );
    let hang_z = ps.origin.z;
    tick(&mut ps, &PmInput::default(), &w, 50);
    assert!(ps.on_ladder, "must stay on the ladder while hanging");
    assert!(
        ps.velocity.z == 0.0,
        "vertical speed should damp to zero, vz {}",
        ps.velocity.z
    );
    assert!(
        ps.origin.z > hang_z - 8.0,
        "must not slide down, {} -> {}",
        hang_z,
        ps.origin.z
    );
}

#[test]
fn ladder_probe_box_is_shrunk() {
    // retail shrinks the probe bbox horizontally (@0x70cb0), so a
    // sideways hover this close to the wall must NOT grab
    let w = ladder_world();
    let w = MoveWorld::bare(&w);
    let input = PmInput {
        forward: 1.0,
        ..Default::default()
    };
    let mut ps = PlayerState::spawn(Vec3::new(8.0, 0.0, 40.0), 0.0);
    pmove(&mut ps, &input, &w, 1.0 / 125.0, &[]);
    assert!(!ps.on_ladder, "full-box probe would grab from here");

    // just past the shrunken reach it still grabs
    let mut ps = PlayerState::spawn(Vec3::new(13.0, 0.0, 40.0), 0.0);
    pmove(&mut ps, &input, &w, 1.0 / 125.0, &[]);
    assert!(ps.on_ladder, "should grab within shrunk reach");
}

#[test]
fn facing_away_mid_climb_keeps_the_grab() {
    let w = ladder_world();
    let w = MoveWorld::bare(&w);
    let run = PmInput {
        forward: 1.0,
        ..Default::default()
    };
    let mut ps = PlayerState::spawn(Vec3::new(20.0, 0.0, 0.2), 0.0);
    for _ in 0..200 {
        pmove(&mut ps, &run, &w, 1.0 / 125.0, &[]);
        if ps.on_ladder && ps.origin.z > 25.0 {
            break;
        }
    }
    assert!(ps.on_ladder && ps.origin.z > 25.0, "set up: {}", ps.origin);
    assert!(
        (ps.ladder_normal + Vec3::X).length() < 0.01,
        "stored normal {:?}",
        ps.ladder_normal
    );

    // turn around mid-climb: retail probes along -vLadderVec, so the
    // grab survives facing away while airborne
    ps.yaw += std::f32::consts::PI;
    let hang_z = ps.origin.z;
    tick(&mut ps, &run, &w, 10);
    assert!(ps.on_ladder, "must keep the grab facing away");
    assert!(
        ps.origin.z > hang_z - 5.0,
        "must not fall off, {} -> {}",
        hang_z,
        ps.origin.z
    );
}

#[test]
fn dropping_onto_the_ladder_from_above_catches() {
    let w = ladder_world();
    let w = MoveWorld::bare(&w);
    // past the face plane, above the wall top region, falling; facing the
    // wall so the first trace hits it more than a unit away and the
    // grab-from-above guard pushes back into it
    let mut ps = PlayerState::spawn(Vec3::new(75.0, 0.0, 80.0), 180.0);
    let input = PmInput {
        forward: 1.0,
        ..Default::default()
    };
    let mut grabbed = false;
    for _ in 0..150 {
        pmove(&mut ps, &input, &w, 1.0 / 125.0, &[]);
        grabbed |= ps.on_ladder;
    }
    assert!(grabbed, "should catch the ladder, at {}", ps.origin);
    // pressed against the face (54 + half-width ~ 69) and climbing, not
    // fallen to the floor
    assert!(
        ps.origin.x < 72.0 && ps.origin.z > 60.0,
        "pushed into the wall instead of falling, at {}",
        ps.origin
    );
}

#[test]
fn jump_gate_matrix() {
    // PM_CheckJump gates @0x2ebb3-0x2ec13: 500 ms since the last jump,
    // standing, jump key released since the previous one
    let n = Vec3::new(-1.0, 0.0, 0.0);
    let mk = |stance: Stance| {
        let mut ps = PlayerState::spawn(Vec3::ZERO, 0.0);
        ps.stance = stance;
        ps.ducked = stance == Stance::Crouch;
        ps.since_jump_ms = 10000.0;
        ps
    };
    let jump = PmInput {
        jump: true,
        ..Default::default()
    };
    let try_jump = |ps: &mut PlayerState, input: &PmInput, ladder: Option<Vec3>| {
        let mut cmd = *input;
        check_jump(ps, &mut cmd, ladder, &mut Vec::new())
    };

    // Off the ground: vz = sqrt(78 * g) = 249.8, the horizontal kept and
    // the jump origin 39 up.
    let mut ps = mk(Stance::Stand);
    ps.velocity = Vec3::new(100.0, 20.0, 0.0);
    assert!(try_jump(&mut ps, &jump, None));
    assert!((ps.velocity.z - 249.8).abs() < 0.05, "vz {}", ps.velocity.z);
    assert_eq!((ps.velocity.x, ps.velocity.y), (100.0, 20.0));
    assert_eq!(ps.jump_origin_z, JUMP_HEIGHT);
    assert!(ps.jump_latched && !ps.on_ground);
    assert_eq!(ps.aim_spread_scale, JUMP_SPREAD_ADD);

    // Off a ladder, facing straight into the wall (yaw 0, normal -1):
    // reflection sends the push straight back; vz = 249.8 * 0.75 = 187.35
    let mut ps = mk(Stance::Stand);
    ps.on_ladder = true;
    assert!(try_jump(&mut ps, &jump, Some(n)));
    assert!((ps.velocity.z - 187.35).abs() < 0.1, "vz {}", ps.velocity.z);
    assert!((ps.velocity.x + 128.0).abs() < 0.1, "vx {}", ps.velocity.x);
    assert!(ps.velocity.y.abs() < 0.1);
    assert!(!ps.on_ladder);

    for ladder in [None, Some(n)] {
        let mut ps = mk(Stance::Stand);
        ps.since_jump_ms = JUMP_COOLDOWN_MS - 1.0;
        assert!(!try_jump(&mut ps, &jump, ladder), "inside the cooldown");

        // boundary: retail allows at delta > 499, so exactly 500 passes
        let mut ps = mk(Stance::Stand);
        ps.since_jump_ms = JUMP_COOLDOWN_MS;
        assert!(try_jump(&mut ps, &jump, ladder), "500 ms allows");

        // a held key is refused and taken off the cmd
        let mut ps = mk(Stance::Stand);
        ps.jump_latched = true;
        let mut cmd = jump;
        assert!(
            !check_jump(&mut ps, &mut cmd, ladder, &mut Vec::new()),
            "held key must release first"
        );
        assert!(!cmd.jump);

        let mut ps = mk(Stance::Crouch);
        assert!(!try_jump(&mut ps, &jump, ladder), "crouch refuses");
        let mut ps = mk(Stance::Prone);
        assert!(!try_jump(&mut ps, &jump, ladder), "prone refuses");

        let mut ps = mk(Stance::Stand);
        assert!(
            !try_jump(&mut ps, &PmInput::default(), ladder),
            "no jump key"
        );
    }
}

#[test]
fn ladder_push_off_leaves_the_wall_with_reflected_forward() {
    let w = ladder_world();
    let w = MoveWorld::bare(&w);
    let climb = PmInput {
        forward: 1.0,
        ..Default::default()
    };
    let mut ps = PlayerState::spawn(Vec3::new(20.0, 0.0, 0.2), 0.0);
    for _ in 0..200 {
        pmove(&mut ps, &climb, &w, 1.0 / 125.0, &[]);
        if ps.on_ladder && ps.origin.z > 25.0 {
            break;
        }
    }
    assert!(ps.on_ladder && ps.origin.z > 25.0, "set up: {}", ps.origin);

    // facing straight into the wall (yaw 0, wall face at x=50): the
    // reflection (@0x2eca2-0x2ed36) sends the push straight back out
    pmove(
        &mut ps,
        &PmInput {
            forward: 1.0,
            jump: true,
            ..Default::default()
        },
        &w,
        1.0 / 125.0,
        &[],
    );
    assert!(!ps.on_ladder, "push-off must clear the ladder");
    // impulse values pinned in the gate-matrix unit test; here the normal
    // mover also ran this frame (air accel + gravity), so ranges only
    assert!(
        (170.0..190.0).contains(&ps.velocity.z),
        "push-off vz {}",
        ps.velocity.z
    );
    assert!(
        (-130.0..-124.0).contains(&ps.velocity.x),
        "push-off vx {}",
        ps.velocity.x
    );
    assert!(ps.since_jump_ms < 16.0, "push-off stamps the timer");
}

#[test]
fn ladder_regrab_locks_for_300ms_after_a_push_off() {
    // detection is skipped while within pm_ladderJumpTime of a push-off
    // (@0x33822, pm_ladderJumpTime = 300 @0x70830)
    let w = ladder_world();
    let w = MoveWorld::bare(&w);
    let climb = PmInput {
        forward: 1.0,
        ..Default::default()
    };

    let mut locked = PlayerState::spawn(Vec3::new(33.0, 0.0, 0.2), 0.0);
    locked.since_jump_ms = 100.0;
    tick(&mut locked, &climb, &w, 10);
    assert!(
        !locked.on_ladder,
        "must not regrab inside the lock, at {}",
        locked.origin
    );

    let mut free = PlayerState::spawn(Vec3::new(33.0, 0.0, 0.2), 0.0);
    free.since_jump_ms = LADDER_REGRAB_LOCK_MS - 40.0;
    tick(&mut free, &climb, &w, 10);
    assert!(
        free.on_ladder,
        "must regrab once the lock elapses, at {}",
        free.origin
    );
}

#[test]
fn airborne_ladder_glue_presses_back_into_the_wall() {
    // PM_LadderMove tail @0x33cf1: while airborne on a ladder, strip
    // velocity along the ladder normal and press into the wall at 250,
    // or 500 holding forward (rodata 0x70cd4/0x70cd8)
    let w = flat();
    let w = MoveWorld::bare(&w);
    let n = Vec3::new(-1.0, 0.0, 0.0); // wall to the east, normal west

    let mut ps = PlayerState::spawn(Vec3::new(0.0, 0.0, 40.0), 0.0);
    ps.on_ground = false;
    ladder_move(
        &mut ps,
        &PmInput::default(),
        n,
        &w,
        1.0 / 125.0,
        &mut Vec::new(),
    );
    assert!(ps.velocity.x == 250.0, "neutral vx {}", ps.velocity.x);

    let mut ps = PlayerState::spawn(Vec3::new(0.0, 0.0, 40.0), 0.0);
    ps.on_ground = false;
    ladder_move(
        &mut ps,
        &PmInput {
            forward: 1.0,
            ..Default::default()
        },
        n,
        &w,
        1.0 / 125.0,
        &mut Vec::new(),
    );
    assert!(ps.velocity.x == 500.0, "forward vx {}", ps.velocity.x);

    // tangential motion survives the glue (minus ladder friction)
    let mut ps = PlayerState::spawn(Vec3::new(0.0, 0.0, 40.0), 0.0);
    ps.on_ground = false;
    ps.velocity = Vec3::new(0.0, 100.0, 0.0);
    ladder_move(
        &mut ps,
        &PmInput::default(),
        n,
        &w,
        1.0 / 125.0,
        &mut Vec::new(),
    );
    assert!(
        (72.0..92.0).contains(&ps.velocity.y),
        "tangential vy survives, {}",
        ps.velocity.y
    );
    assert!(ps.velocity.x == 250.0);
}

#[test]
fn glue_strength_follows_the_climb_rate_sign() {
    // selector @0x33d2e reads the climb-rate slot's sign: forward *
    // 0.5 * upscale * cmdScale (@0x33a50), so W past the fz=-0.25 pitch
    // falls back to 250 and S below it strengthens to 500
    let w = flat();
    let w = MoveWorld::bare(&w);
    let n = Vec3::new(-1.0, 0.0, 0.0);
    let vx = |pitch_deg: f32, forward: f32| {
        let mut ps = PlayerState::spawn(Vec3::new(0.0, 0.0, 40.0), 0.0);
        ps.on_ground = false;
        ps.pitch = pitch_deg.to_radians();
        ladder_move(
            &mut ps,
            &PmInput {
                forward,
                ..Default::default()
            },
            n,
            &w,
            1.0 / 125.0,
            &mut Vec::new(),
        );
        ps.velocity.x
    };
    assert!(vx(0.0, 1.0) == 500.0, "W level: 500");
    // -30 deg: fz = -0.5, u = -0.625 -> sign flips
    assert!(vx(-30.0, 1.0) == 250.0, "W down: 250");
    assert!(vx(-30.0, -1.0) == 500.0, "S down: 500");
    assert!(vx(30.0, -1.0) == 250.0, "S up: 250");
}

#[test]
fn push_off_is_pitch_invariant_on_a_vertical_wall() {
    // the composition dots the FLAT normalized forward against the ladder
    // vec (locals built @0x2ec7e-0x2ec90 with a literal-zero z lane; the
    // pitched forward feeds only the facing gate @0x2eca2-0x2ecd3), so a
    // vertical wall pushes 128 horizontally at any look pitch
    let n = Vec3::new(-1.0, 0.0, 0.0);
    let mut ps = PlayerState::spawn(Vec3::ZERO, 0.0);
    ps.pitch = 45f32.to_radians();
    assert!(push_off(
        &mut ps,
        &PmInput {
            jump: true,
            ..Default::default()
        },
        n
    ));
    assert!((ps.velocity.x + LADDER_PUSHOFF_SPEED).abs() < 0.1);
    assert!(ps.velocity.y.abs() < 0.1);
    assert!((ps.velocity.z - 187.35).abs() < 0.1);
}

#[test]
fn push_off_normalizes_the_3d_reflection_before_scaling_xy() {
    // tilted normal: retail normalizes the full 3D reflected vector
    // (@0x2ed36) and only then scales x/y by 128 (@0x2ed5a)
    let n = Vec3::new(-2.0, 0.0, 1.0).normalize();
    let mut ps = PlayerState::spawn(Vec3::ZERO, 0.0);
    assert!(push_off(
        &mut ps,
        &PmInput {
            jump: true,
            ..Default::default()
        },
        n
    ));
    // yaw 0 -> f = (1,0,0); d2 = f.n = n.x; R = f - 2*d2*n
    let d2 = n.x;
    let r = Vec3::new(1.0 - 2.0 * d2 * n.x, 0.0, -2.0 * d2 * n.z);
    let expect_x = r.x / r.length() * LADDER_PUSHOFF_SPEED;
    assert!(
        (ps.velocity.x - expect_x).abs() < 0.1,
        "vx {} vs expected {expect_x}",
        ps.velocity.x
    );
    assert!(ps.velocity.y.abs() < 0.1);
}

/// Stock maps carry real SURF_LADDER brushes; find one from the BSP like
/// collision.rs does for harbor water, stand outside its thinnest axis and
/// climb.
#[test]
fn climbs_a_real_map_ladder() {
    let Some(fs) = crate::testing::game_fs() else {
        return;
    };
    for map in ["maps/mp/mp_powcamp.bsp", "maps/mp/mp_harbor.bsp"] {
        let Some(data) = fs.read(map) else {
            continue;
        };
        let Ok(bsp) = crate::bsp::parse(&data) else {
            continue;
        };
        let world = CollisionWorld::build(&bsp, &[]);
        let world = MoveWorld::bare(&world);
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
        for b in &bsp.brushes {
            if bsp.materials[b.material as usize].surface_flags & 0x8 == 0 {
                continue;
            }
            let (lo, hi) = axial_bounds(b);
            let ex = hi[0] - lo[0];
            let ey = hi[1] - lo[1];
            if ex <= 0.0 || ey <= 0.0 || hi[2] - lo[2] < 40.0 {
                continue;
            }
            // thinnest horizontal axis is the climb direction
            let (dir, span) = if ex < ey {
                (Vec3::X, ex * 0.5)
            } else {
                (Vec3::Y, ey * 0.5)
            };
            let mid_other = if ex < ey {
                (lo[1] + hi[1]) * 0.5
            } else {
                (lo[0] + hi[0]) * 0.5
            };
            for sign in [-1.0f32, 1.0] {
                let yaw = if dir == Vec3::X {
                    if sign > 0.0 { 180.0f32 } else { 0.0 }
                } else if sign > 0.0 {
                    90.0
                } else {
                    -90.0
                };
                let base = Vec3::new(
                    if dir == Vec3::X {
                        (lo[0] + hi[0]) * 0.5 + sign * (span + 16.0)
                    } else {
                        mid_other
                    },
                    if dir == Vec3::X {
                        mid_other
                    } else {
                        (lo[1] + hi[1]) * 0.5 + sign * (span + 16.0)
                    },
                    0.0,
                );
                for dz in [24.0f32, 48.0] {
                    let start = base + Vec3::Z * (lo[2] + dz);
                    let mut ps = PlayerState::spawn(start, yaw);
                    let before = ps.origin.z;
                    tick(
                        &mut ps,
                        &PmInput {
                            forward: 1.0,
                            ..Default::default()
                        },
                        &world,
                        90,
                    );
                    if ps.on_ladder && ps.origin.z - before > 10.0 {
                        return; // found a working ladder on this map
                    }
                }
            }
        }
        panic!("no approach to any {map} ladder grabbed and climbed");
    }
    panic!("powcamp/harbor not found under $COD_DIR");
}

const FULL: f32 = 127.0;

#[test]
fn spectator_flies_forward_at_spectator_speed() {
    let mut ps = PlayerState::spawn(Vec3::ZERO, 90.0); // yaw 90 deg faces +Y
    ps.speed = SPEED_SPECTATOR;
    for _ in 0..250 {
        spectator_move(&mut ps, None, FULL, 0.0, 0.0, 0, 1.0 / 125.0);
    }
    let h = ps.velocity.truncate().length();
    assert!(h == 400.0, "speed {h}");
    // x is 400 * cos(pi / 2) in f32 (-1.7e-5 here): libm's, so a bound.
    assert!(ps.velocity.y > 350.0 && ps.velocity.x.abs() < 1e-3);
}

/// `PM_FlyMove` wishes along the full view and scales by
/// `PM_CmdScale`: looking down flies down, half a key flies at half
/// speed, and no key leaves the velocity alone, a -0 included
/// (docs/protocol-1.1.md, "Spectator").
#[test]
fn spectator_flies_along_the_view_at_the_key_s_speed() {
    let mut ps = PlayerState::spawn(Vec3::ZERO, 0.0);
    ps.speed = SPEED_SPECTATOR;
    ps.pitch = -45f32.to_radians();
    for _ in 0..250 {
        spectator_move(&mut ps, None, FULL, 0.0, 0.0, 0, 1.0 / 125.0);
    }
    assert!(
        ps.velocity.z < -250.0 && ps.velocity.x > 250.0,
        "{}",
        ps.velocity
    );

    let mut ps = PlayerState::spawn(Vec3::ZERO, 0.0);
    ps.speed = SPEED_SPECTATOR;
    for _ in 0..250 {
        spectator_move(&mut ps, None, 63.5, 0.0, 0.0, 0, 1.0 / 125.0);
    }
    assert!(ps.velocity.x == 200.0, "{}", ps.velocity);

    let mut ps = PlayerState::spawn(Vec3::ZERO, 0.0);
    ps.speed = SPEED_SPECTATOR;
    ps.velocity = Vec3::new(0.0, 0.0, -0.0);
    spectator_move(&mut ps, None, 0.0, 0.0, 0.0, 0, 1.0 / 125.0);
    assert_eq!(ps.velocity.z.to_bits(), 0x8000_0000);
}

/// The lean bits fly a spectator with no key down 512 (right) and up
/// 256 (left), and down 256 with both. Retail's ramp under both, from
/// `--probe-edge lean-fly` on mp_carentan (docs/protocol-1.1.md,
/// "Spectator"): the cmd times it sent and the `velocity[2]` bits of the
/// three snapshots that followed, from 0.9565565 at rest.
#[test]
fn spectator_lean_bits_fly_down_512_and_up_256() {
    use crate::net::msg::{WBUTTON_LEAN_LEFT, WBUTTON_LEAN_RIGHT};
    let both = WBUTTON_LEAN_LEFT | WBUTTON_LEAN_RIGHT;
    let mut ps = PlayerState::spawn(Vec3::ZERO, 0.0);
    ps.velocity = Vec3::new(0.0, 0.0, f32::from_bits(0x3f74_e0e3));
    let mut ct = 22441;
    for (cmds, bits) in [
        ([22461, 22478, 22495], 0xc2c8_543e_u32),
        ([22512, 22529, 22546], 0xc32c_8db8),
        ([22561, 22577, 22594], 0xc361_0530),
    ] {
        for t in cmds {
            let dt = cmd::frametime(t - ct);
            spectator_move(&mut ps, None, 0.0, 0.0, 0.0, both, dt);
            ct = t;
        }
        assert_eq!(
            ps.velocity.z.to_bits(),
            bits,
            "{} {:08x}",
            ps.velocity.z,
            ps.velocity.z.to_bits()
        );
    }

    for (wbuttons, vz) in [(WBUTTON_LEAN_RIGHT, -512.0), (WBUTTON_LEAN_LEFT, 256.0)] {
        let mut ps = PlayerState::spawn(Vec3::ZERO, 0.0);
        for _ in 0..125 {
            spectator_move(&mut ps, None, 0.0, 0.0, 0.0, wbuttons, 1.0 / 125.0);
        }
        assert_eq!(ps.velocity, Vec3::new(0.0, 0.0, vz));
    }
}

#[test]
fn spectator_rises_with_upmove_and_stops_on_friction() {
    let mut ps = PlayerState::spawn(Vec3::ZERO, 0.0);
    for _ in 0..63 {
        spectator_move(&mut ps, None, 0.0, 0.0, FULL, 0, 1.0 / 125.0);
    }
    assert!(ps.velocity.z > 100.0, "climbing, vz {}", ps.velocity.z);
    for _ in 0..250 {
        spectator_move(&mut ps, None, 0.0, 0.0, 0.0, 0, 1.0 / 125.0);
    }
    assert!(ps.velocity.length() < 1.0, "friction must stop the fly");
}

/// A spectator flies in its own 16-unit hull and stops on a wall, with
/// its centre 8.125 short of the face; a ledge under its hull it steps
/// onto, in the air, and announces the step (docs/protocol-1.1.md,
/// "Spectator").
#[test]
fn a_spectator_stops_on_a_wall_and_steps_onto_a_ledge() {
    let wall = (
        Vec3::new(64.0, -512.0, -16.0),
        Vec3::new(128.0, 512.0, 512.0),
    );
    let ledge = (
        Vec3::new(32.0, -512.0, -16.0),
        Vec3::new(128.0, 512.0, 100.0),
    );
    let cw = test_world(&[wall, ledge]);
    let w = MoveWorld::bare(&cw);
    let mut ps = PlayerState::spawn(Vec3::new(-200.0, 0.0, 96.0), 0.0);
    let mut events = Vec::new();
    for _ in 0..250 {
        events.extend(spectator_move(
            &mut ps,
            Some(&w),
            FULL,
            0.0,
            0.0,
            0,
            1.0 / 125.0,
        ));
    }
    assert_eq!(ps.origin.x, 64.0 - 8.125, "origin {}", ps.origin);
    assert_eq!(ps.origin.z, 108.125, "origin {}", ps.origin);
    assert!(!ps.spectator, "the flag outlived the move");
    let step = events
        .iter()
        .find(|e| e.event == EV_STEP_VIEW)
        .expect("a step event");
    assert_eq!(step.parm, 12 + STEP_VIEW_BIAS);
}

fn body(x: f32, y: f32, z: f32) -> Body {
    Body {
        entity: 7,
        origin: Vec3::new(x, y, z),
        mins: Vec3::new(-15.0, -15.0, 0.0),
        maxs: Vec3::new(15.0, 15.0, 70.0),
        contents: CONTENTS_BODY,
    }
}

#[test]
fn a_run_into_a_player_stops_at_its_capsule() {
    let w = flat();
    let bodies = [body(60.0, 0.0, 0.0)];
    let mw = MoveWorld::new(&w, &bodies, 0);
    let mut ps = PlayerState::spawn(Vec3::ZERO, 0.0);
    let run = PmInput {
        forward: 1.0,
        ..Default::default()
    };
    for _ in 0..40 {
        pmove(&mut ps, &run, &mw, 0.008, &[]);
    }
    assert!(
        ps.origin.x <= 60.0 - 30.0 && ps.origin.x > 60.0 - 31.0,
        "{}",
        ps.origin.x
    );
}

#[test]
fn a_run_into_a_prone_player_does_not_step_onto_it() {
    let w = flat();
    // Both resting where a floor holds a player, 0.125 up.
    let prone = Body {
        maxs: Vec3::new(15.0, 15.0, 30.0),
        ..body(60.0, 0.0, 0.125)
    };
    let bodies = [prone];
    let mw = MoveWorld::new(&w, &bodies, 0);
    let mut ps = PlayerState::spawn(Vec3::Z * 0.125, 0.0);
    let run = PmInput {
        forward: 1.0,
        ..Default::default()
    };
    for _ in 0..60 {
        pmove(&mut ps, &run, &mw, 0.008, &[]);
    }
    // A push this slow creeps inside the backoff through the step's down
    // pass, which misses the bare radius; nothing gets past that. The
    // down pass's float fraction leaves the floor a few millionths under
    // 0.125, as retail's does.
    assert!(
        (ps.origin.z - 0.125).abs() < 1e-5 && (30.0..30.2).contains(&(60.0 - ps.origin.x)),
        "{:?}",
        ps.origin
    );
}

#[test]
fn dead_mask_ignores_bodies() {
    let w = flat();
    let bodies = [body(20.0, 0.0, 0.0)];
    let mw = MoveWorld::new(&w, &bodies, 0);
    let mut ps = PlayerState::spawn(Vec3::new(0.0, 0.0, 0.125), 0.0);
    ps.velocity = Vec3::new(200.0, 0.0, 0.0);
    for _ in 0..10 {
        dead_move(&mut ps, &mw, 0.05);
    }
    assert!(
        ps.origin.x > 5.0,
        "a corpse slides through players: {}",
        ps.origin.x
    );
}

#[test]
fn lands_on_a_body() {
    let w = flat();
    let bodies = [body(0.0, 0.0, 0.0)];
    let mw = MoveWorld::new(&w, &bodies, 0);
    let mut ps = PlayerState::spawn(Vec3::new(0.0, 0.0, 120.0), 0.0);
    ps.on_ground = false;
    for _ in 0..60 {
        pmove(&mut ps, &PmInput::default(), &mw, 0.016, &[]);
    }
    assert!(
        ps.on_ground && ps.ground_entity == 7,
        "{:?} {}",
        ps.origin,
        ps.ground_entity
    );
    assert!(ps.origin.z == 70.125, "{}", ps.origin.z);
}

#[test]
fn lean_is_blocked_by_a_body() {
    // yaw 0's right vector is -Y, where lean_right swings the eye; a body
    // there should cut the lean short the way a wall does (INFERRED,
    // PM_UpdateLean's 0x2810011 == MASK_PLAYERSOLID, docs/research/cod11-player-clip.md 1.1).
    let w = flat();
    let bodies = [body(0.0, -35.0, 0.0)];
    let mw = MoveWorld::new(&w, &bodies, 0);
    let mut ps = PlayerState::spawn(Vec3::ZERO, 0.0);
    tick(&mut ps, &PmInput::default(), &mw, 50);
    let lean_r = PmInput {
        lean_right: true,
        ..Default::default()
    };
    tick(&mut ps, &lean_r, &mw, 125);
    assert!(ps.lean > 0.0);
    assert!(ps.lean < 0.9, "a body should limit lean, got {}", ps.lean);
}

fn v15(origin: Vec3) -> PlayerState {
    PlayerState {
        rules: MoveRules::V1_5,
        ..PlayerState::spawn(origin, 0.0)
    }
}

/// The apex of one standing jump at 125 fps from where `ps` stands.
fn jump_apex(ps: &mut PlayerState, w: &MoveWorld) -> f32 {
    let start = ps.origin.z;
    let jump = PmInput {
        jump: true,
        ..Default::default()
    };
    pmove(ps, &jump, w, 0.008, &[]);
    let mut apex = ps.origin.z;
    while !ps.on_ground {
        pmove(ps, &PmInput::default(), w, 0.008, &[]);
        apex = apex.max(ps.origin.z);
    }
    apex - start
}

/// 1.5's jump penalty (docs/research/cod15-movement.md, "The jump
/// penalty"): a landing starts an 1800 ms timer, a jump inside it takes
/// off at the squared speed over the curve, and 1.1 has none of it.
#[test]
fn a_1_5_jump_inside_the_penalty_is_lower() {
    let w = flat();
    let w = MoveWorld::bare(&w);
    let mut ps = v15(Vec3::ZERO);
    tick(&mut ps, &PmInput::default(), &w, 30);
    // 125 fps jumps a little over the 39 (`a_125_fps_jump_goes_higher`).
    let first = jump_apex(&mut ps, &w);
    assert!(first == 41.26878, "{first}");
    assert!(ps.jump_penalty);
    assert_eq!(ps.jump_origin_z, 0.0, "the takeoff height, kept");
    // The landing frame armed the timer; a jump 500 ms on pays the curve
    // at ~1300 ms left: 39 / (1 + 1300 * 1.5 / 1700) = 18.2.
    tick(&mut ps, &PmInput::default(), &w, 62);
    assert!(ps.knockback_ms > 1200.0 && ps.knockback_ms < 1350.0);
    let second = jump_apex(&mut ps, &w);
    assert!(second > 16.0 && second < 20.0, "{second}");
    // Past the timer the flag drops and the full jump is back.
    tick(&mut ps, &PmInput::default(), &w, 250);
    assert!(!ps.jump_penalty && ps.knockback_ms == 0.0);
    let third = jump_apex(&mut ps, &w);
    assert!((third - first).abs() < 0.5, "{third}");

    let mut ps = PlayerState::spawn(Vec3::ZERO, 0.0);
    tick(&mut ps, &PmInput::default(), &w, 30);
    jump_apex(&mut ps, &w);
    tick(&mut ps, &PmInput::default(), &w, 62);
    let again = jump_apex(&mut ps, &w);
    assert!((again - first).abs() < 0.5, "1.1: {again}");
}

/// UO's sprint (docs/research/coduo-movement.md, 2-4): 1.6 times the
/// run, a full bar spent in 3 s with the wind under half, nothing back
/// for a second after, then the whole bar in 7 s and the wind gone.
#[test]
fn a_uo_sprint_runs_fast_and_tires() {
    let w = flat();
    let w = MoveWorld::bare(&w);
    let mut ps = PlayerState {
        rules: MoveRules::Uo,
        ..PlayerState::spawn(Vec3::ZERO, 0.0)
    };
    let mut t = 0;
    let mut run = |ps: &mut PlayerState, input: PmInput, ms: i32| {
        for _ in 0..ms / 8 {
            t += 8;
            pmove(
                ps,
                &PmInput {
                    server_time: t,
                    ..input
                },
                &w,
                0.008,
                &[],
            );
        }
    };
    let sprint = PmInput {
        forward: 1.0,
        sprint: true,
        ..Default::default()
    };
    run(&mut ps, PmInput::default(), 240);
    run(&mut ps, sprint, 1000);
    assert!(ps.sprinting);
    let speed = ps.velocity.truncate().length();
    assert!(speed == 304.0, "{speed}");
    run(&mut ps, sprint, 600);
    assert!(ps.winded && ps.fatigue < 0.5);
    run(&mut ps, sprint, 1500);
    assert!(!ps.sprinting && ps.fatigue == 0.0);
    let mut rested = ps;
    run(&mut rested, PmInput::default(), 900);
    assert_eq!(rested.fatigue, 0.0, "the regeneration delay");
    // Held on an empty bar: no sprint and no refill, though the delay
    // runs from the last sprinting cmd.
    run(&mut ps, sprint, 2000);
    assert_eq!(ps.fatigue, 0.0);
    run(&mut ps, PmInput::default(), 3500);
    assert!(ps.winded && (ps.fatigue - 0.5).abs() < 0.01);
    run(&mut ps, PmInput::default(), 3600);
    assert!(!ps.winded && ps.fatigue == 1.0);
    // `bg_nofatigue` 1: the sprint runs on a full bar (0x2aa63).
    let mut fresh = PlayerState {
        rules: MoveRules::Uo,
        ..PlayerState::spawn(Vec3::ZERO, 0.0)
    };
    fresh.bg.nofatigue = true;
    run(&mut fresh, PmInput::default(), 240);
    run(&mut fresh, sprint, 1000);
    assert!(fresh.sprinting && fresh.fatigue == 1.0);
}

/// Every patch drops the moves of a sighted prone body before the
/// move (1.1 0x33f48 on `pm_flags` 0x21, 1.5 0x281ae on the effective
/// stance); a crouched or unsighted one keeps them.
#[test]
fn a_sighted_prone_body_holds_still_on_every_patch() {
    let moving = PmInput {
        forward: 1.0,
        right: -1.0,
        ..Default::default()
    };
    for rules in [MoveRules::V1_1, MoveRules::V1_5, MoveRules::Uo] {
        let mut ps = PlayerState::spawn(Vec3::ZERO, 0.0);
        ps.rules = rules;
        ps.stance = Stance::Prone;
        ps.view_target = VIEW_PRONE;
        ps.ads_active = true;
        let held = hold_still_sighted(&ps, &moving);
        assert_eq!((held.forward, held.right), (0.0, 0.0), "{rules:?}");
        ps.ads_active = false;
        assert_eq!(hold_still_sighted(&ps, &moving).forward, 1.0, "{rules:?}");
        ps.ads_active = true;
        ps.stance = Stance::Crouch;
        ps.view_target = VIEW_CROUCH;
        assert_eq!(hold_still_sighted(&ps, &moving).forward, 1.0, "{rules:?}");
    }
}

/// The sight off the ground (cod15-movement.md 5): 1.5 and UO raise it
/// on a body falling within a unit of the floor, `PM_GroundTraceMissed`'s
/// `almostGroundPlane`; 1.1 and a body higher up wait for the landing.
#[test]
fn a_15_sight_starts_a_unit_over_the_floor() {
    let w = flat();
    let w = MoveWorld::bare(&w);
    let rifle = WeaponDef {
        aim_down_sight: true,
        ads_trans_in: 0.3,
        ads_trans_out: 0.3,
        clip_size: 5,
        ammo_index: 1,
        clip_index: 1,
        ..WeaponDef::default()
    };
    let weapons = [None, Some(rifle)];
    let sight = PmInput {
        ads: true,
        weapon: 1,
        ..Default::default()
    };
    let first_cmd = |rules: MoveRules, z: f32| {
        let mut ps = PlayerState {
            rules,
            ..PlayerState::spawn(Vec3::new(0.0, 0.0, z), 0.0)
        };
        weapon::give(&mut ps, 1, 1);
        ps.weapon = 1;
        ps.ammoclip[1] = 5;
        ps.on_ground = false;
        pmove(&mut ps, &sight, &w, 0.016, &weapons);
        ps.ads_active
    };
    for rules in [MoveRules::V1_5, MoveRules::Uo] {
        assert!(first_cmd(rules, 0.63), "{rules:?}");
        assert!(!first_cmd(rules, 1.5), "{rules:?}");
    }
    assert!(!first_cmd(MoveRules::V1_1, 0.63));
}

/// UO's bipod from prone (coduo-movement.md 7): the sight goes down in
/// `adsTransInTime`, the body takes no move and its view no more than
/// 55 degrees off `proneDirection`, and letting go breaks it down in
/// `adsTransOutTime` at the walk.
#[test]
fn a_uo_bipod_deploys_prone_and_holds_the_view() {
    let w = flat();
    let w = MoveWorld::bare(&w);
    let lmg = WeaponDef {
        weapon_class: "lmg".into(),
        aim_down_sight: true,
        ads_trans_in: 1.8,
        ads_trans_out: 1.9,
        move_speed_scale: 1.1,
        clip_size: 100,
        ammo_index: 1,
        clip_index: 1,
        ..WeaponDef::default()
    };
    let weapons = [None, Some(lmg)];
    let mut ps = PlayerState {
        rules: MoveRules::Uo,
        ..PlayerState::spawn(Vec3::ZERO, 0.0)
    };
    weapon::give(&mut ps, 1, 1);
    ps.weapon = 1;
    ps.ammoclip[1] = 100;
    let tick = |ps: &mut PlayerState, input: &PmInput, n: usize| {
        for _ in 0..n {
            pmove(ps, input, &w, 0.008, &weapons);
        }
    };
    let prone = PmInput {
        prone: true,
        weapon: 1,
        ..Default::default()
    };
    tick(&mut ps, &prone, 125);
    assert_eq!(ps.stance, Stance::Prone);
    let sight = PmInput { ads: true, ..prone };
    tick(&mut ps, &sight, 1);
    assert!(ps.deployed());
    assert_eq!(ps.weaponstate, weapon::WEAPON_DEPLOY);
    assert_eq!(ps.weapon_time_ms, 1799);
    tick(&mut ps, &sight, 230);
    assert_eq!(ps.weaponstate, weapon::WEAPON_READY);
    assert_eq!(ps.weapon_pos_frac, 1.0);
    let at = ps.origin;
    tick(
        &mut ps,
        &PmInput {
            forward: 1.0,
            ..sight
        },
        60,
    );
    assert_eq!(ps.origin.truncate(), at.truncate());
    ps.yaw = 80f32.to_radians();
    tick(&mut ps, &sight, 1);
    assert!(
        (ps.yaw.to_degrees() - LMG_YAWCAP).abs() < 0.1,
        "{}",
        ps.yaw.to_degrees()
    );
    assert_eq!(ps.prone_direction, 0.0, "a deployed body does not swing");
    tick(&mut ps, &prone, 1);
    assert_eq!(ps.weaponstate, weapon::WEAPON_BREAKDOWN);
    assert_eq!(ps.weapon_time_ms, 1899);
}

/// 1.5 has no prone dive, and its eye target steps one stance a leg:
/// stand to prone is a 200 ms leg to the crouch height, the target's
/// step to prone on the next cmd, which arms the jump penalty, and a
/// 400 ms leg on. The box and the effective stance follow the target.
#[test]
fn a_1_5_prone_press_steps_the_eye_and_never_dives() {
    let w = flat();
    let w = MoveWorld::bare(&w);
    let mut ps = v15(Vec3::ZERO);
    let run = PmInput {
        forward: 1.0,
        ..Default::default()
    };
    tick(&mut ps, &run, &w, 30);
    let prone = PmInput { prone: true, ..run };
    pmove(&mut ps, &prone, &w, 0.008, &[]);
    assert_eq!(ps.stance, Stance::Prone);
    assert!(!ps.prone_dive && ps.on_ground);
    assert_eq!(ps.view_target, VIEW_CROUCH);
    assert_eq!(ps.maxs().z, HEIGHT_CROUCH);
    let mut first_leg = 8;
    while ps.view_target == VIEW_CROUCH {
        pmove(&mut ps, &prone, &w, 0.008, &[]);
        first_leg += 8;
    }
    // The press starts the 200 ms leg at 0, the 26th cmd ends it and the
    // 27th steps the target.
    assert_eq!(first_leg, 27 * 8);
    assert_eq!(ps.view_target, VIEW_PRONE);
    assert_eq!(ps.maxs().z, HEIGHT_PRONE);
    // Less the cmd's own 8 ms: `PM_DropTimers` runs after the stance.
    assert!(ps.jump_penalty && ps.knockback_ms == JUMP_PENALTY_MS - 8.0);
    assert_eq!(ps.effective_stance(), Stance::Prone);
}

/// UO's `PM_CheckDuck` (0x28d66..0x28ef0) sizes the box off the flags
/// where they stand higher than the eye's target: a crouch let go
/// inside the 200 ms leg down stands the box at once, where 1.5's waits
/// for the target. So a UO body that crouch-walks under a 60-unit
/// ceiling after the release is stopped at its edge, and a 1.5 one
/// walks under it and stands up into it, held there all solid
/// (coduo-movement.md 18; mp_cassino held bots so until they typed
/// `kill`).
#[test]
fn a_uo_rise_sizes_the_box_off_the_flags() {
    let w = test_world(&[(Vec3::new(0.0, -64.0, 60.0), Vec3::new(256.0, 64.0, 80.0))]);
    let w = MoveWorld::bare(&w);
    let crouch = PmInput {
        crouch: true,
        ..Default::default()
    };
    let walk = PmInput {
        forward: 1.0,
        ..Default::default()
    };
    let wedged = |rules: MoveRules| {
        let mut ps = PlayerState {
            rules,
            ..PlayerState::spawn(Vec3::new(-16.0, 0.0, 0.125), 0.0)
        };
        tick(&mut ps, &PmInput::default(), &w, 10);
        tick(&mut ps, &crouch, &w, 5);
        assert_eq!(ps.view_target, VIEW_CROUCH);
        assert_eq!(ps.maxs().z, HEIGHT_CROUCH);
        tick(&mut ps, &walk, &w, 1);
        assert_eq!(ps.stance, Stance::Stand);
        assert!(ps.view_lerp_ms.is_some(), "still inside the leg down");
        let box_at_release = ps.maxs().z;
        tick(&mut ps, &walk, &w, 125);
        let x = ps.origin.x;
        tick(&mut ps, &walk, &w, 25);
        (box_at_release, x, ps.origin.x, ps.maxs().z)
    };
    let (uo_box, uo_x, uo_end, uo_h) = wedged(MoveRules::Uo);
    assert_eq!(uo_box, HEIGHT_STAND);
    assert!(uo_x <= -15.0 && uo_end <= -15.0, "{uo_x} {uo_end}");
    assert_eq!(uo_h, HEIGHT_STAND);
    let (box15, x15, end15, h15) = wedged(MoveRules::V1_5);
    assert_eq!(box15, HEIGHT_CROUCH);
    assert!(
        x15 > -15.0 && x15 == end15,
        "held under the ceiling: {x15} {end15}"
    );
    assert_eq!(h15, HEIGHT_STAND);
}

/// `PM_Footsteps`: a lean is the walk under 1.5, in the rate and the
/// ids, and audible; under 1.1 it is a slow run (0x24bf7, 0x3105c).
#[test]
fn a_1_5_lean_run_steps_like_a_walk() {
    let w = dirt_flat();
    let w = MoveWorld::bare(&w);
    let lean_run = PmInput {
        forward: 1.0,
        lean_right: true,
        ..Default::default()
    };
    let ids: Vec<Vec<i32>> = [MoveRules::V1_1, MoveRules::V1_5]
        .into_iter()
        .map(|rules| {
            let mut ps = PlayerState {
                rules,
                ..PlayerState::spawn(Vec3::ZERO, 0.0)
            };
            (0..500)
                .flat_map(|_| pmove(&mut ps, &lean_run, &w, 0.008, &[]))
                .map(|e| e.event)
                .collect()
        })
        .collect();
    assert!(
        ids[0].iter().all(|&e| e == EV_FOOTSTEP_RUN_BASE + 6),
        "{ids:?}"
    );
    assert!(
        ids[1].iter().all(|&e| e == EV_FOOTSTEP_WALK_BASE + 6),
        "{ids:?}"
    );
    assert!(ids[1].len() > ids[0].len(), "{ids:?}");
}

/// The cycle ticks by the share of full speed the body has, truncated:
/// a start from rest moves it nothing on the first cmd. Under 10 u/s a
/// creep keeps the phase and a stop (under 1) zeroes it (0x324af).
#[test]
fn the_bob_cycle_runs_on_the_share_of_full_speed() {
    let w = dirt_flat();
    let w = MoveWorld::bare(&w);
    let mut ps = PlayerState::spawn(Vec3::ZERO, 0.0);
    let run = PmInput {
        forward: 1.0,
        ..Default::default()
    };
    pmove(&mut ps, &run, &w, 0.008, &[]);
    assert!(ps.velocity.length() > 10.0);
    assert_eq!(ps.bob_cycle, 0);
    tick(&mut ps, &run, &w, 40);
    let phase = ps.bob_cycle;
    assert_ne!(phase, 0);
    let still = PmInput::default();
    ps.velocity = Vec3::new(5.0, 0.0, 0.0);
    footsteps(&mut ps, &still, &w, 0.008, &mut Vec::new());
    assert_eq!(ps.bob_cycle, phase);
    ps.velocity = Vec3::new(0.5, 0.0, 0.0);
    footsteps(&mut ps, &still, &w, 0.008, &mut Vec::new());
    assert_eq!(ps.bob_cycle, 0);
}

/// A leaning body's bob ticks at the walk's rate on 1.5 and the run's on
/// UO (`game.mp.uo` 0x297e4): the same strafe at the same speed advances
/// UO's cycle by less (moveshot_ab.rs, the lean-strafe rounds).
#[test]
fn a_uo_lean_bobs_at_the_run_s_rate() {
    let w = dirt_flat();
    let w = MoveWorld::bare(&w);
    let strafe = PmInput {
        right: 1.0,
        ..Default::default()
    };
    let tick_of = |rules: MoveRules| {
        let mut ps = PlayerState::spawn(Vec3::ZERO, 0.0);
        ps.rules = rules;
        ps.on_ground = true;
        ps.lean = -0.5;
        ps.velocity = Vec3::new(0.0, -71.0, 0.0);
        footsteps(&mut ps, &strafe, &w, 0.050, &mut Vec::new());
        ps.bob_cycle
    };
    let (v15, uo) = (tick_of(MoveRules::V1_5), tick_of(MoveRules::Uo));
    assert!(uo > 0 && uo < v15, "1.5 {v15} UO {uo}");
}

fn prone_on(w: &MoveWorld, origin: Vec3, ground: bool, rules: MoveRules) -> Option<ProneBody> {
    check_prone(
        w,
        ProneQuery {
            origin,
            yaw_deg: 0.0,
            entry: false,
            ground,
            length: PRONE_CHECK_LENGTH,
            rules,
        },
    )
}

/// UO's `BG_CheckProneValid` lets the legs bend up 45 degrees past the
/// hips (0x9a9e8), where 1.1 and 1.5 allow 50 (coduo-movement.md 17):
/// the feet on a 30-unit step behind bend them about 48.
#[test]
fn a_uo_body_refuses_legs_bent_up_past_45_degrees() {
    let step = |height: f32| {
        test_world(&[
            (
                Vec3::new(-200.0, -200.0, -16.0),
                Vec3::new(200.0, 200.0, 0.0),
            ),
            (
                Vec3::new(-200.0, -200.0, 0.0),
                Vec3::new(-54.0, 200.0, height),
            ),
        ])
    };
    let o = Vec3::new(0.0, 0.0, 0.125);
    let high = step(30.0);
    let w = MoveWorld::bare(&high);
    for rules in [MoveRules::V1_1, MoveRules::V1_5] {
        let body = prone_on(&w, o, true, rules).expect("a fit");
        assert!((45.0..50.0).contains(&body.waist_pitch), "{body:?}");
    }
    assert!(prone_on(&w, o, true, MoveRules::Uo).is_none());
    let low = step(20.0);
    assert!(prone_on(&MoveWorld::bare(&low), o, true, MoveRules::Uo).is_some());
}

/// 1.5's `BG_CheckProneValid` refuses a body whose samples land on a
/// plane steeper than 0.7 (0x1d842, 0x1dd86, 0x1def4); 1.1 lays it down.
#[test]
fn a_1_5_body_does_not_lie_on_a_steep_plane() {
    let w = crate::collision::ramp_test_world(50.0, -200.0, 200.0);
    let w = MoveWorld::bare(&w);
    let o = Vec3::new(0.0, 0.0, 200.0 * 50f32.to_radians().tan() + 1.0);
    assert!(prone_on(&w, o, true, MoveRules::V1_1).is_some());
    assert!(prone_on(&w, o, true, MoveRules::V1_5).is_none());
}

/// Off the ground 1.1 stops at the sweep and lays the body flat; 1.5
/// samples the ground under it anyway (cod15-movement.md 4).
#[test]
fn a_1_5_airborne_body_still_samples_the_ground() {
    let w = crate::collision::ramp_test_world(20.0, -200.0, 200.0);
    let w = MoveWorld::bare(&w);
    let o = Vec3::new(0.0, 0.0, 200.0 * 20f32.to_radians().tan() + 1.0);
    assert_eq!(
        prone_on(&w, o, false, MoveRules::V1_1),
        Some(ProneBody::default())
    );
    let body = prone_on(&w, o, false, MoveRules::V1_5).expect("a fit");
    assert!(body.torso_pitch.abs() > 10.0, "{body:?}");
}

/// 1.5's prone view (0x26118): turned toward a wall the legs cannot
/// sweep, the body refuses the swing, the refusal sets 0x8000, and the
/// view is pulled back toward the body further than 1.1's cap leaves it.
#[test]
fn a_1_5_prone_turn_into_a_wall_pulls_the_view_back() {
    let w = test_world(&[(
        Vec3::new(-200.0, -60.0, 0.0),
        Vec3::new(200.0, -40.0, 100.0),
    )]);
    let w = MoveWorld::bare(&w);
    let mut off = Vec::new();
    for rules in [MoveRules::V1_1, MoveRules::V1_5] {
        let mut ps = PlayerState {
            rules,
            ..PlayerState::spawn(Vec3::new(0.0, 0.0, 0.125), 0.0)
        };
        let prone = PmInput {
            prone: true,
            ..Default::default()
        };
        tick(&mut ps, &prone, &w, 100);
        assert_eq!(ps.stance, Stance::Prone);
        let mut blocked = false;
        for i in 0..250 {
            ps.prev_yaw = ps.yaw;
            ps.yaw = (120.0 * i as f32 / 250.0).to_radians();
            pmove(&mut ps, &prone, &w, 0.008, &[]);
            blocked |= ps.prone_blocked;
        }
        off.push((
            ps.yaw.to_degrees() - ps.prone_direction,
            blocked,
            ps.view_yaw_shorts,
        ));
    }
    assert!(off[1].1, "{off:?}");
    assert!(off[1].0 < off[0].0 - 1.0, "{off:?}");
}
