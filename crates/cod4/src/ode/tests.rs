use super::body::{AUTO_DISABLE, AutoDisable};
use super::joint::{Joint, Kind, ball::Ball};
use super::math::IDENTITY;
use super::quickstep::{INFINITY, Row, draw, reorder, sor_lcp};
use super::world::{BodyId, World};

/// The draws off `Phys_Init`'s seed, worked by hand from 0x5b7b1c..0x5b7b39:
/// `s = s * 0x343fd + 0x269ec3`, index `((s >> 17) * 5) >> 15`.
#[test]
fn the_reorder_draws_run_msvcs_lcg_from_the_init_seed() {
    let mut s = super::world::SEED;
    let mut got = Vec::new();
    for _ in 0..6 {
        let (i, next) = draw(s, 5);
        got.push((next, i));
        s = next;
    }
    assert_eq!(
        got,
        [
            (0x6fa3_b0f6, 2),
            (0x3f1e_e3e1, 1),
            (0x22f4_b720, 0),
            (0xbd4c_f963, 3),
            (0x26a6_fe9a, 0),
            (0xe3a0_8af5, 4)
        ]
    );
    // Five swaps of two draws each over rows 0..5.
    let mut order: Vec<usize> = (0..5).collect();
    let mut seed = super::world::SEED;
    reorder(&mut order, &mut seed);
    assert_eq!(order, [4, 2, 1, 3, 0]);
    assert_eq!(seed, 0x25c8_a849);
}

fn row(j1l: [f32; 3], rhs: f32, lo: f32, hi: f32) -> Row {
    Row {
        j1l,
        rhs,
        lo,
        hi,
        b1: 0,
        b2: -1,
        ..Row::default()
    }
}

/// One unit body, two rows coupled through it: `A = [[1, 1], [1, 2]]`,
/// `b = [1, 3]`, row 0 bounded below by 0. The unbounded solution has
/// `lambda0 = -1`, so the LCP's is `lambda = [0, 1.5]` with row 0's
/// slack 0.5.
#[test]
fn sor_solves_a_two_row_lcp_with_a_bound() {
    let mut rows = vec![
        row([1.0, 0.0, 0.0], 1.0, 0.0, INFINITY),
        row([1.0, 1.0, 0.0], 3.0, -INFINITY, INFINITY),
    ];
    let mut fc = vec![[[0.0; 3]; 2]];
    let mut seed = super::world::SEED;
    sor_lcp(
        &mut rows,
        &[1.0],
        &[IDENTITY],
        &[0.0, 0.0],
        &[-1, -1],
        60,
        1.3,
        &mut seed,
        &mut fc,
    );
    assert_eq!(rows[0].lambda, 0.0);
    assert!((rows[1].lambda - 1.5).abs() < 1e-5, "{}", rows[1].lambda);
    // fc = inv(M) J' lambda.
    for (got, want) in fc[0][0].iter().zip([1.5, 1.5, 0.0]) {
        assert!((got - want).abs() < 1e-5, "{:?}", fc[0][0]);
    }
}

/// A friction row (findex 0, hi the friction coefficient) is bounded by
/// `mu * lambda[normal]` each pass: the normal row's 2 holds the tangent
/// push that wants 5 at 0.5 * 2.
#[test]
fn a_friction_row_is_held_to_mu_times_its_normal() {
    let mut rows = vec![
        row([0.0, 0.0, 1.0], 2.0, 0.0, INFINITY),
        row([1.0, 0.0, 0.0], -5.0, 0.0, 0.5),
    ];
    let mut fc = vec![[[0.0; 3]; 2]];
    let mut seed = super::world::SEED;
    sor_lcp(
        &mut rows,
        &[1.0],
        &[IDENTITY],
        &[0.0, 0.0],
        &[-1, 0],
        15,
        1.3,
        &mut seed,
        &mut fc,
    );
    assert!((rows[0].lambda - 2.0).abs() < 1e-5, "{}", rows[0].lambda);
    assert!((rows[1].lambda + 1.0).abs() < 1e-5, "{}", rows[1].lambda);
}

/// IW's sleep: `phys_autoDisableTime` 0.9 with the steps zeroed. A body
/// drifting under `phys_autoDisableLinear` sleeps on the step its time runs
/// out: 0.9 / 0.017 = 52.9, so the 53rd step's test disables it before it
/// moves. One past the threshold restarts the clock.
#[test]
fn a_body_sleeps_once_idle_for_the_time_alone() {
    let mut world = World::new();
    world.params.adis = AutoDisable {
        linear_threshold: 400.0,
        angular_threshold: 1.0,
        idle_time: 0.9,
        idle_steps: 0,
    };
    world.params.adis_flag = true;
    let b = world.create_body();
    assert_eq!(world.body(b).flags & AUTO_DISABLE, AUTO_DISABLE);
    world.body_mut(b).lvel = [10.0, 0.0, 0.0];
    let mut steps = 0;
    while world.body(b).enabled() {
        world.quick_step(0.017);
        steps += 1;
        assert!(steps < 100, "never slept");
    }
    assert_eq!(steps, 53);
    // The 52 steps it moved before.
    assert!(
        (world.body(b).pos[0] - 52.0 * 0.17).abs() < 1e-3,
        "{:?}",
        world.body(b).pos
    );

    world.body_mut(b).enable();
    world.body_mut(b).lvel = [10.0, 0.0, 0.0];
    for _ in 0..30 {
        world.quick_step(0.017);
    }
    world.body_mut(b).avel = [0.0, 0.0, 1.5];
    world.quick_step(0.017);
    world.body_mut(b).avel = [0.0; 3];
    for _ in 0..52 {
        world.quick_step(0.017);
        assert!(world.body(b).enabled());
    }
    world.quick_step(0.017);
    assert!(!world.body(b).enabled());
}

/// IW's split on a chain of 16 balls: the island keeps its first 14
/// bodies; the joint between the two dropped goes back for a later island,
/// and the one from the cut to a kept body stays, its dropped end read as
/// index 0 (0x5b9c60).
#[test]
fn an_island_past_14_bodies_drops_its_last() {
    let mut world = World::new();
    let bodies: Vec<BodyId> = (0..16).map(|_| world.create_body()).collect();
    for (i, &b) in bodies.iter().enumerate() {
        world.body_mut(b).pos = [i as f32, 0.0, 0.0];
    }
    let joints: Vec<_> = bodies
        .windows(2)
        .map(|w| {
            let j = world.create_joint(Joint::new(Kind::Ball(Ball::default())));
            world.attach(j, Some(w[0]), Some(w[1]));
            j
        })
        .collect();
    // The walk starts at the list's head, the last body made, and runs
    // down the chain.
    let (island, jlist) = world.split_island_for_test(bodies[15]);
    assert_eq!(
        island,
        bodies[2..].iter().rev().copied().collect::<Vec<_>>()
    );
    assert!(!jlist.contains(&joints[0]));
    assert!(jlist.contains(&joints[1]));
    assert_eq!(jlist.len(), 14);
    assert_eq!(world.joint(joints[1]).index, [0, 13]);
    assert_eq!(world.joint(joints[0]).tag, 0);
    // A full step reaches the dropped pair as their own island.
    world.params.gravity = [0.0, 0.0, -800.0];
    world.quick_step(0.017);
    for &b in &bodies {
        assert!(world.body(b).lvel[2] < 0.0, "{b:?} not stepped");
    }
}
