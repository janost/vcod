//! Spectator flight.

use super::*;

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
