//! The prone body check, the prone view and pitch.

use super::*;

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
        angle_normalize360_short(ps.prone_direction - PRONE_YAWCAP)
    );
    assert_eq!(ps.view_capped[1], Some(yaw));
    let pitch = ps.view_capped[0].expect("the pitch is capped");
    assert!(on_grid(pitch), "pitch {pitch}");
    assert_eq!(pitch, angle_normalize180_short(-4.1f32 + 45.0));

    ps.rules = MoveRules::V1_5;
    ps.pitch = -view_rad(10923.0 * (360.0 / 65536.0));
    cap_prone_pitch(&mut ps, &w);
    assert_eq!(ps.view_capped[0], Some(-4.1f32 + 45.0));
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
    assert_eq!(held[1], angle_normalize360_short(10.3f32 + 45.0));
    assert_eq!(held[0], angle_normalize180_short(-4.0f32 + 15.0));
    let held = prone_view_angle(&ps, [-60.0, -50.0, 0.0]);
    assert_eq!(held[1], angle_normalize360_short(10.3f32 - 45.0));
    assert!(held[1] > 300.0);
    assert_eq!(held[0], angle_normalize180_short(-4.0f32 - 45.0));
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

/// UO's chest sample landing more than 24 under the origin plus 6 misses
/// (game.mp.uo 0x22841..0x22866): a lie-down 26 over the floor refuses,
/// an airborne check lays the body flat, and 1.5 fits it.
#[test]
fn a_uo_chest_sample_far_under_the_origin_misses() {
    let floor = test_world(&[(
        Vec3::new(-200.0, -200.0, -16.0),
        Vec3::new(200.0, 200.0, 0.0),
    )]);
    let w = MoveWorld::bare(&floor);
    let high = Vec3::new(0.0, 0.0, 26.0);
    let body = prone_on(&w, high, true, MoveRules::V1_5).expect("a fit");
    assert!(body.torso_height < -24.0, "{body:?}");
    assert!(prone_on(&w, high, true, MoveRules::Uo).is_none());
    assert_eq!(
        prone_on(&w, high, false, MoveRules::Uo),
        Some(ProneBody::default())
    );
    let low = Vec3::new(0.0, 0.0, 20.0);
    assert!(prone_on(&w, low, true, MoveRules::Uo).is_some());
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

/// The floor and a pool of water `lo..hi` on it, its top 4 over the floor.
fn pool_world(lo: [f32; 2], hi: [f32; 2]) -> crate::collision::CollisionWorld {
    crate::collision::synthetic_world(
        &[
            ("textures/test/solid", crate::collision::CONTENTS_SOLID, 0),
            ("textures/common/water", crate::collision::CONTENTS_WATER, 0),
        ],
        &[
            (0, [-1024.0, -1024.0, -16.0], [1024.0, 1024.0, 0.0]),
            (1, [lo[0], lo[1], -16.0], [hi[0], hi[1], 4.0]),
        ],
    )
}

/// UO's prone traces meet water (mask 0x810031), and a feet or chest
/// sample that lands on it misses (game.mp.uo 0x22615..0x22631,
/// 0x227a4..0x227c0): refused on the ground, a flat body off it. 1.5's
/// mask has no water bit, so it lies down in the same pools.
#[test]
fn a_uo_chest_or_feet_sample_on_water_misses() {
    for (pool_lo, pool_hi) in [
        ([-10.0, -10.0], [10.0, 10.0]),
        ([-60.0, -10.0], [-45.0, 10.0]),
    ] {
        let pool = pool_world(pool_lo, pool_hi);
        let w = MoveWorld::bare(&pool);
        assert!(prone_on(&w, Vec3::ZERO, true, MoveRules::V1_5).is_some());
        assert!(prone_on(&w, Vec3::ZERO, true, MoveRules::Uo).is_none());
        assert_eq!(
            prone_on(&w, Vec3::ZERO, false, MoveRules::Uo),
            Some(ProneBody::default())
        );
    }
    let dry = pool_world([300.0, 300.0], [310.0, 310.0]);
    assert!(prone_on(&MoveWorld::bare(&dry), Vec3::ZERO, true, MoveRules::Uo).is_some());
}

/// UO's lie-down refuses a prone box that starts in water (0x217f1..0x2180b,
/// the entry trace's contents and `startsolid`); the same pool off the
/// samples' path lets the move's own check through.
#[test]
fn a_uo_lie_down_refuses_a_body_starting_in_water() {
    let pool = pool_world([8.0, -4.0], [14.0, 4.0]);
    let w = MoveWorld::bare(&pool);
    let query = |entry, rules| ProneQuery {
        origin: Vec3::ZERO,
        yaw_deg: 0.0,
        entry,
        ground: true,
        length: PRONE_CHECK_LENGTH,
        rules,
    };
    assert!(check_prone(&w, query(true, MoveRules::Uo)).is_none());
    assert!(check_prone(&w, query(false, MoveRules::Uo)).is_some());
    assert!(check_prone(&w, query(true, MoveRules::V1_5)).is_some());
}
