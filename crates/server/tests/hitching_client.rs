//! How long one usercmd is simulated for when a client hitches.
//!
//! Retail does not clamp a long move away: `Pmove` walks `ps.commandTime` up
//! to the cmd's `serverTime` in steps of at most 66 ms, each its own
//! `PmoveSingle` (`game.mp.i386.so` 0x344b2 and 0x344d3), and `PmoveSingle`
//! leaves `commandTime` at the cmd's own clock (0x34074). So a client that
//! goes quiet for half a second and then sends one cmd gets the whole gap
//! simulated. `docs/protocol-1.1.md`, "How long a cmd is simulated for".

mod common;

use common::{connect, step, Queues};
use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};
use vcod_common::net::msg::NULL_USERCMD;
use vcod_common::net::protocol::PROTOCOL_V1;
use vcod_server::{Server, ServerConfig};

/// One server frame at `sv_fps` 20, which is also what the client's clock
/// advances by per step here, so a cmd's `serverTime` lands on the tick that
/// replays it.
const FRAME_MS: i32 = 50;
/// Frames the client sends nothing for. Long enough that a move clamped to
/// one `MAX_FRAME_MS` step would be an order of magnitude short.
const HITCH_FRAMES: i32 = 12;

/// No world is loaded, so a client flies as a spectator rather than walking
/// (`spectate.rs`, the `(Normal, None)` arm). That keeps the move to
/// `spectator_move` with no collision, which the test reproduces exactly, and
/// keeps the test off the game data CI does not have.
fn server() -> Server {
    Server::new(
        ServerConfig {
            map: "mp_carentan".into(),
            hostname: "loop".into(),
            max_clients: 2,
            gametype: "dm".into(),
            test_entities: 0,
            trace: false,
            bots: 0,
            bots_shoot: false,
        },
        Instant::now(),
    )
}

#[test]
fn a_hitching_clients_gap_is_chopped_into_66_ms_steps_not_clamped_away() {
    let q = Rc::new(RefCell::new(Queues::default()));
    let mut sv = server();
    let mut now = Instant::now();
    let mut cl = connect(&mut sv, &q, &mut now);
    let p = &PROTOCOL_V1;

    // Enter the world, then settle until the client's cmds are landing one
    // frame apart. The handshake stamps its first cmds off a clock with no
    // snapshot to anchor to yet, so the entering cmd can sit ahead of
    // `sv_time` and freeze the sim until the server catches up (`enter_world`).
    // An all-zero cmd moves a resting spectator nowhere, so once they do land
    // the sim is still exactly `PlayerState::spawn` at the fallback spawn.
    let mut command_time_before = 0;
    let mut settled = false;
    for _ in 0..80 {
        now += Duration::from_millis(FRAME_MS as u64);
        cl.send_frame(&NULL_USERCMD);
        step(&mut sv, &q, &mut cl, now);
        let ct = cl
            .snapshots()
            .newest()
            .expect("a snapshot")
            .ps
            .field_i32(p, "commandTime");
        settled = ct - command_time_before == FRAME_MS;
        command_time_before = ct;
        if settled {
            break;
        }
    }
    assert!(settled, "the client's cmds never caught the server's clock");
    let before = cl.snapshots().newest().expect("a snapshot");
    assert_eq!(before.ps.origin(p), [0.0, 0.0, 64.0], "not at the spawn");

    // The hitch: the server ticks on, the client sends nothing.
    for _ in 0..HITCH_FRAMES {
        now += Duration::from_millis(FRAME_MS as u64);
        step(&mut sv, &q, &mut cl, now);
    }

    // One cmd, holding forward, covering the whole gap.
    let mut fwd = NULL_USERCMD;
    fwd.forward = 127;
    now += Duration::from_millis(FRAME_MS as u64);
    cl.send_frame(&fwd);
    step(&mut sv, &q, &mut cl, now);

    let after = cl.snapshots().newest().expect("a snapshot");
    // `PmoveSingle` leaves `commandTime` at the cmd's own `serverTime`, so the
    // gap the client left shows up whole in the field the client predicts off.
    let gap = after.ps.field_i32(p, "commandTime") - command_time_before;
    assert_eq!(
        gap,
        (HITCH_FRAMES + 1) * FRAME_MS,
        "commandTime did not advance by the whole gap"
    );

    // What retail's chop loop integrates over that gap, from the same resting
    // state, in the same steps. A move clamped to one 66 ms step instead
    // leaves the spectator around 5 units out rather than several hundred.
    let mut ps = vcod_common::pmove::PlayerState::spawn(glam::Vec3::new(0.0, 0.0, 64.0), 0.0);
    let mut left = gap;
    while left > 0 {
        let msec = left.min(vcod_common::pmove::MAX_FRAME_MS as i32);
        left -= msec;
        vcod_common::pmove::spectator_move(&mut ps, 1.0, 0.0, 0.0, msec as f32 / 1000.0);
    }
    let origin = after.ps.origin(p);
    let expected = ps.origin.to_array();
    for axis in 0..3 {
        assert!(
            (origin[axis] - expected[axis]).abs() < 1.0,
            "origin {origin:?}, chopped over {gap} ms gives {expected:?}"
        );
    }
}
