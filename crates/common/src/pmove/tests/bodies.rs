//! Other players' bodies in the move.

use super::*;

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
