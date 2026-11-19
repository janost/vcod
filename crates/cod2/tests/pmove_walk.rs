//! CoD2 pmove over a converted map, a sanity walk, not a fidelity gate: from
//! every mp_toujane spawn a body runs, jumps, crouches, goes prone and turns,
//! and never falls through the world or ends stuck in it. Needs `COD2_DIR`.

use glam::Vec3;
use vcod_cod2::net::UserCmd2;
use vcod_cod2::pmove::{self, PlayerState, Rules, button};
use vcod_common::collision::CollisionWorld;
use vcod_common::movetrace::MoveWorld;
use vcod_common::patch::Arith;
use vcod_common::testing;

/// Spawn origins and yaws of the entity text.
fn spawns(entities: &str) -> Vec<(Vec3, f32)> {
    vcod_common::bsp::entity_blocks(entities)
        .into_iter()
        .filter(|b| {
            b.get("classname")
                .is_some_and(|c| c.starts_with("mp_") && c.ends_with("_spawn"))
        })
        .filter_map(|b| {
            let o = vcod_common::bsp::parse_vec3(b.get("origin")?)?;
            let yaw = b
                .get("angles")
                .and_then(|a| vcod_common::bsp::parse_vec3(a))
                .map_or(0.0, |a| a[1]);
            Some((Vec3::from_array(o), yaw))
        })
        .collect()
}

#[test]
fn a_body_walks_mp_toujane_from_every_spawn() {
    let Some(fs) = testing::cod2_fs() else {
        return;
    };
    let data = fs.read("maps/mp/mp_toujane.d3dbsp").expect("mp_toujane");
    let map = vcod_cod2::world::from_bytes(&data).unwrap();
    let col = CollisionWorld::build(&map, &[]);
    col.set_arith(Arith::Stored);
    let floor = map.models[0].mins[2] - 1.0;
    let spots = spawns(&map.entities);
    assert!(spots.len() > 10, "{} spawns", spots.len());

    // (cmds, buttons, forward, yaw turn in degrees)
    let script: &[(usize, i32, i8, f32)] = &[
        (10, 0, 0, 0.0),
        (20, 0, 127, 0.0),
        (1, button::JUMP, 127, 0.0),
        (15, 0, 127, 0.0),
        (15, button::CROUCH, 127, 0.0),
        (20, button::PRONE, 127, 0.0),
        (20, 0, 127, 90.0),
        (20, 0, -127, 0.0),
        (10, 0, 0, 0.0),
    ];
    let (mut moved, mut went_prone, mut grounded) = (0, 0, 0);
    for &(spot, yaw) in &spots {
        let mut ps = PlayerState::spawn(spot + Vec3::Z * 0.125, yaw, 1000);
        ps.delta_angles[1] = (yaw * 65536.0 / 360.0) as i32 & 0xffff;
        let mut old = UserCmd2 {
            server_time: 1000,
            ..Default::default()
        };
        let mut turn = 0.0f32;
        let mut prone = false;
        for &(n, buttons, forward, dyaw) in script {
            turn += dyaw;
            for _ in 0..n {
                let c = UserCmd2 {
                    server_time: ps.command_time + 50,
                    buttons,
                    forward_move: forward,
                    angles: [0, (turn * 65536.0 / 360.0) as i32 & 0xffff, 0],
                    ..Default::default()
                };
                pmove::player_step(
                    &mut ps,
                    &c,
                    &mut old,
                    MoveWorld::bare(&col),
                    Rules::default(),
                    1.0,
                );
                assert!(
                    ps.origin.is_finite() && ps.velocity.is_finite(),
                    "{spot}: {ps:?}"
                );
                assert!(
                    ps.origin.z > floor,
                    "{spot}: fell through the world: {ps:?}"
                );
                prone |= ps.pm_flags & pmove::pmf::PRONE != 0;
            }
        }
        // Not stuck: the stance's capsule fits where the walk ended.
        let (mins, maxs) = ps.stance_box();
        let t = MoveWorld::bare(&col).box_trace(
            ps.origin,
            ps.origin,
            mins,
            maxs,
            pmove::MASK_PLAYERSOLID,
        );
        assert!(!t.startsolid, "{spot}: ended in solid at {}", ps.origin);
        if (ps.origin - spot).truncate().length() > 50.0 {
            moved += 1;
        }
        went_prone += usize::from(prone);
        grounded += usize::from(ps.on_ground());
    }
    let n = spots.len();
    eprintln!(
        "mp_toujane: {n} spawns, {moved} moved, {went_prone} went prone, {grounded} on the ground, floor {floor}"
    );
    assert!(moved * 4 >= n * 3, "{moved} of {n} spawns moved");
    assert!(went_prone * 2 >= n, "{went_prone} of {n} spawns went prone");
    assert!(
        grounded * 4 >= n * 3,
        "{grounded} of {n} spawns ended on the ground"
    );
}

/// What a prone client flicking its view costs (cod2-movement.md 10):
/// from every mp_toujane spawn a body crawls into whatever is ahead, then
/// sends 32 cmds whose yaw swings 180 degrees each, a cmd a millisecond,
/// one packet's worth. Prints the slowest packet. A measurement, not a
/// gate: `--ignored --nocapture`.
#[test]
#[ignore = "a measurement"]
fn a_prone_view_flick_costs() {
    let Some(fs) = testing::cod2_fs() else {
        return;
    };
    let data = fs.read("maps/mp/mp_toujane.d3dbsp").expect("mp_toujane");
    let map = vcod_cod2::world::from_bytes(&data).unwrap();
    let col = CollisionWorld::build(&map, &[]);
    col.set_arith(Arith::Stored);
    let world = || MoveWorld::bare(&col);
    let short = |deg: f32| (deg * 65536.0 / 360.0) as i32 & 0xffff;
    let (mut worst, mut total, mut packets) = (std::time::Duration::ZERO, 0.0, 0);
    let (mut still_worst, mut still_total) = (std::time::Duration::ZERO, 0.0);
    for (spot, yaw) in spawns(&map.entities) {
        let mut ps = PlayerState::spawn(spot + Vec3::Z * 0.125, yaw, 1000);
        ps.delta_angles[1] = short(yaw);
        let mut old = UserCmd2 {
            server_time: 1000,
            ..Default::default()
        };
        let cmd = |ps: &PlayerState, dt: i32, forward: i8, turn: f32| UserCmd2 {
            server_time: ps.command_time + dt,
            buttons: button::PRONE,
            forward_move: forward,
            angles: [0, short(turn), 0],
            ..Default::default()
        };
        for forward in [0, 127] {
            for _ in 0..200 {
                let c = cmd(&ps, 50, forward, 0.0);
                pmove::player_step(&mut ps, &c, &mut old, world(), Rules::default(), 1.0);
            }
        }
        let t0 = std::time::Instant::now();
        for _ in 0..32 {
            let c = cmd(&ps, 1, 0, 0.0);
            pmove::player_step(&mut ps, &c, &mut old, world(), Rules::default(), 1.0);
        }
        still_worst = still_worst.max(t0.elapsed());
        still_total += t0.elapsed().as_secs_f64();
        for round in 0..4 {
            let t0 = std::time::Instant::now();
            for i in 0..32 {
                let c = cmd(
                    &ps,
                    1,
                    0,
                    if (round * 32 + i) % 2 == 0 {
                        180.0
                    } else {
                        0.0
                    },
                );
                pmove::player_step(&mut ps, &c, &mut old, world(), Rules::default(), 1.0);
            }
            let dt = t0.elapsed();
            worst = worst.max(dt);
            total += dt.as_secs_f64();
            packets += 1;
        }
    }
    eprintln!(
        "mp_toujane: {packets} flick packets, mean {:.2} ms, worst {:.2} ms; \
         still: mean {:.2} ms, worst {:.2} ms",
        total * 1e3 / f64::from(packets),
        worst.as_secs_f64() * 1e3,
        still_total * 4e3 / f64::from(packets),
        still_worst.as_secs_f64() * 1e3
    );
}
