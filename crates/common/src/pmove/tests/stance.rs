//! Stance changes and the eye through them.

use super::*;

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
