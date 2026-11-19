//! The slide and the step.

use super::*;

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
