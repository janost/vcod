//! The bob cycle and the step events.

use super::*;

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
