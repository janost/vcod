//! Jumps, their takeoff events and 1.5's penalty.

use super::*;

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
