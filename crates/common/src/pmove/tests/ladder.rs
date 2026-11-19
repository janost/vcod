//! Ladders: the grab, the climb, the push-off and the yaw cap.

use super::*;

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

/// `check_jump` off a ladder plane, the way `ladder_move` calls it.
fn push_off(ps: &mut PlayerState, input: &PmInput, normal: Vec3) -> bool {
    let mut cmd = *input;
    check_jump(ps, &mut cmd, Some(normal), &mut Vec::new())
}

fn ladder_push_off_ran(ps: &PlayerState) -> bool {
    ps.jump_latched && ps.since_jump_ms == 0.0
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

/// Dry floor at z=0 plus a playerclip ladder wall (SURF_LADDER) spanning
/// x 50..54, y -200..200, z 0..220.
fn ladder_world() -> CollisionWorld {
    crate::collision::synthetic_world(
        &[
            ("textures/test/solid", crate::collision::CONTENTS_SOLID, 0),
            // CONTENTS_PLAYERCLIP is private to the collision module
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
/// collision/tests.rs does for harbor water, stand outside its thinnest axis and
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
