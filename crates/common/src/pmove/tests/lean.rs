//! The lean.

use super::*;

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
