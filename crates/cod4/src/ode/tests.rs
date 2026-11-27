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

use super::joint::amotor::{AMotor, Rel};
use super::joint::contact::{APPROX1_1, APPROX1_2, BOUNCE, Contact, SOFT_CFM, SOFT_ERP, Surface};
use super::joint::hinge::Hinge;
use super::mass::Mass;
use super::math::{V3, add, mul0_331, sub};
use super::world::Params;

fn close(got: f32, want: f64, rel: f64) {
    assert!(((got as f64 - want) / want).abs() < rel, "{got} != {want}");
}

/// A capsule of radius 2, 3 between its caps' centres, along body x:
/// mass `pi r^2 L + 4/3 pi r^3`, inertia across
/// `M1 (r^2/4 + L^2/12) + M2 (2r^2/5 + 3rL/8 + L^2/4)`, along
/// `(M1/2 + 2 M2/5) r^2`. Adjusted to 10 it keeps its shape, and the body's
/// inverse inertia is the diagonal's reciprocals.
#[test]
fn a_capsules_mass_matches_its_closed_form() {
    let m = Mass::capped_cylinder(1.0, 1, 2.0, 3.0);
    close(m.mass, 71.209_433_481_368_65, 1e-6);
    close(m.i[0][0], 129.014_738_307_420_8, 1e-6);
    close(m.i[1][1], 270.386_407_718_961_5, 1e-6);
    assert_eq!(m.i[1][1], m.i[2][2]);
    assert_eq!(m.i[0][1], 0.0);
    let mut adjusted = m;
    adjusted.adjust(10.0);
    assert_eq!(adjusted.mass, 10.0);
    close(adjusted.i[0][0], 18.117_647_058_823_525, 1e-6);
    close(adjusted.i[1][1], 37.970_588_235_294_11, 1e-6);
    let mut world = World::new();
    let b = world.create_body();
    world.body_mut(b).set_mass(adjusted);
    let inv = world.body(b).inv_i;
    close(inv[0][0], 0.055_194_805_194_805_206, 1e-6);
    close(inv[1][1], 0.026_336_173_508_907_83, 1e-6);
    assert!(inv[0][1].abs() < 1e-9 && inv[1][2].abs() < 1e-9);
    assert_eq!(world.body(b).inv_mass, 0.1);
}

/// A unit mass a metre under a world hinge (inertia 0.01 about its centre),
/// let go at 0.1 rad: its period is `2 pi sqrt(I / (m g l))` = 2.016 s.
#[test]
fn a_pendulum_on_a_hinge_keeps_its_period() {
    let mut world = World::new();
    world.params.gravity = [0.0, 0.0, -9.81];
    let b = world.create_body();
    let a = 0.1f32;
    world.body_mut(b).pos = [a.sin(), 0.0, -a.cos()];
    world.body_mut(b).set_mass(Mass {
        mass: 1.0,
        c: [0.0; 3],
        i: [[0.01, 0.0, 0.0], [0.0, 0.01, 0.0], [0.0, 0.0, 0.01]],
    });
    let j = world.create_joint(Joint::new(Kind::Hinge(Hinge::new(&world.params))));
    world.attach(j, Some(b), None);
    world.with_joint(j, |j, ends| {
        let Kind::Hinge(h) = &mut j.kind else {
            unreachable!()
        };
        h.set_anchor(ends, [0.0; 3]);
        h.set_axis(ends, [0.0, 1.0, 0.0]);
    });
    let h = 0.001;
    let mut crossings = Vec::new();
    let mut last = world.body(b).pos[0];
    let mut slip = 0.0f32;
    for step in 1..=6000 {
        world.quick_step(h);
        world.zero_forces();
        let x = world.body(b).pos[0];
        if (last > 0.0) != (x > 0.0) {
            crossings.push(step as f64 * h as f64);
        }
        last = x;
        let p = world.body(b).pos;
        let r = (p[0] * p[0] + p[1] * p[1] + p[2] * p[2]).sqrt();
        slip = slip.max((r - 1.0).abs());
        assert!(p[1].abs() < 1e-4, "off the hinge's plane: {p:?}");
    }
    eprintln!("slip {slip} crossings {crossings:?}");
    assert!(crossings.len() >= 5, "{crossings:?}");
    let period = (crossings[4] - crossings[0]) / 2.0;
    close(period as f32, 2.016_072_062_881_683_7, 5e-3);
}

/// The anchor in the world, from each end.
fn ball_error(world: &mut World, j: super::world::JointId) -> f32 {
    world.with_joint(j, |j, ends| {
        let Kind::Ball(ball) = &j.kind else {
            unreachable!()
        };
        let b1 = ends[0].unwrap();
        let p1 = add(b1.pos, mul0_331(&b1.r, ball.anchor1));
        let p2 = match ends[1] {
            Some(b2) => add(b2.pos, mul0_331(&b2.r, ball.anchor2)),
            None => ball.anchor2,
        };
        let d = sub(p1, p2);
        (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt()
    })
}

fn capsule_body(world: &mut World, pos: V3) -> BodyId {
    let b = world.create_body();
    let mut m = Mass::capped_cylinder(1.0, 1, 2.0, 5.0);
    m.adjust(10.0);
    world.body_mut(b).pos = pos;
    world.body_mut(b).set_mass(m);
    world.body_mut(b).set_finite_rotation(true);
    b
}

/// Two 10-unit capsules hanging from a world ball and each other, let go
/// 45 degrees off the vertical under the ragdoll world's -800 at 17 ms
/// steps: both balls hold within 3% of a link while the chain swings for
/// 3 s (the soft rows give most at the whip's bottom). Let go level, the
/// lower link whips past `CLAMP_ANGULAR` and the clamp, cutting its speed
/// after the solve, opens its ball by 1.4.
#[test]
fn ball_joints_hold_a_swinging_chain() {
    let mut world = World::new();
    world.params = Params::ragdoll();
    let d = std::f32::consts::FRAC_1_SQRT_2;
    let at = |k: f32| [k * d, 0.0, -k * d];
    let b1 = capsule_body(&mut world, at(5.0));
    let b2 = capsule_body(&mut world, at(15.0));
    let ball = |world: &mut World, a: BodyId, b: Option<BodyId>, p: V3| {
        let j = world.create_joint(Joint::new(Kind::Ball(Ball::default())));
        world.attach(j, Some(a), b);
        world.with_joint(j, |j, ends| {
            let Kind::Ball(ball) = &mut j.kind else {
                unreachable!()
            };
            ball.set_anchor(ends, p);
        });
        j
    };
    let j1 = ball(&mut world, b1, None, [0.0; 3]);
    let j2 = ball(&mut world, b2, Some(b1), at(10.0));
    let mut lowest = 0.0f32;
    let mut worst = 0.0f32;
    for _ in 0..180 {
        world.quick_step(0.017);
        world.zero_forces();
        worst = worst
            .max(ball_error(&mut world, j1))
            .max(ball_error(&mut world, j2));
        lowest = lowest.min(world.body(b2).pos[2]);
    }
    assert!(worst < 0.3, "{worst}");
    assert!(lowest < -15.0, "never swung down: {lowest}");
}

/// A hinge turned by a steady torque with no gravity stops at its high
/// stop (0.5 rad) and rests on it.
#[test]
fn a_hinge_stops_at_its_stop() {
    let mut world = World::new();
    let b = capsule_body(&mut world, [5.0, 0.0, 0.0]);
    let j = world.create_joint(Joint::new(Kind::Hinge(Hinge::new(&world.params))));
    world.attach(j, Some(b), None);
    world.with_joint(j, |j, ends| {
        let Kind::Hinge(h) = &mut j.kind else {
            unreachable!()
        };
        h.set_anchor(ends, [0.0; 3]);
        h.set_axis(ends, [0.0, 0.0, 1.0]);
        h.limot.lostop = -0.5;
        h.limot.histop = 0.5;
    });
    let angle = |world: &mut World| {
        world.with_joint(j, |j, ends| {
            let Kind::Hinge(h) = &j.kind else {
                unreachable!()
            };
            h.angle(ends)
        })
    };
    let mut peak = 0.0f32;
    for _ in 0..90 {
        world.body_mut(b).add_torque([0.0, 0.0, 2000.0]);
        world.quick_step(0.017);
        world.zero_forces();
        peak = peak.max(angle(&mut world));
    }
    assert!(peak < 0.55, "past the stop: {peak}");
    assert!(
        (angle(&mut world) - 0.5).abs() < 0.02,
        "{}",
        angle(&mut world)
    );
}

/// The swivel's Euler motor between two bodies holds each angle to its
/// stops: the two twisted against each other about their common x by a
/// pair of torques, free of gravity, stop at 0.3 rad.
#[test]
fn an_euler_motor_holds_its_stops() {
    let mut world = World::new();
    let top = capsule_body(&mut world, [-5.0, 0.0, 0.0]);
    let low = capsule_body(&mut world, [5.0, 0.0, 0.0]);
    let ball = world.create_joint(Joint::new(Kind::Ball(Ball::default())));
    world.attach(ball, Some(low), Some(top));
    world.with_joint(ball, |j, ends| {
        let Kind::Ball(b) = &mut j.kind else {
            unreachable!()
        };
        b.set_anchor(ends, [0.0; 3]);
    });
    let m = world.create_joint(Joint::new(Kind::AMotor(AMotor::euler(&world.params))));
    world.attach(m, Some(low), Some(top));
    world.with_joint(m, |j, ends| {
        let Kind::AMotor(a) = &mut j.kind else {
            unreachable!()
        };
        a.set_axis(ends, 2, Rel::Body2, [0.0, 0.0, 1.0]);
        a.set_axis(ends, 0, Rel::Body1, [1.0, 0.0, 0.0]);
        for l in &mut a.limot {
            l.lostop = -0.3;
            l.histop = 0.3;
        }
    });
    let angles = |world: &mut World| {
        world.with_joint(m, |j, ends| {
            let Kind::AMotor(a) = &j.kind else {
                unreachable!()
            };
            a.euler_angles(ends)
        })
    };
    assert_eq!(angles(&mut world), [0.0; 3]);
    let mut peak = 0.0f32;
    for _ in 0..90 {
        world.body_mut(low).add_torque([200.0, 0.0, 0.0]);
        world.body_mut(top).add_torque([-200.0, 0.0, 0.0]);
        world.quick_step(0.017);
        world.zero_forces();
        let a = angles(&mut world);
        peak = peak.max(a[0].abs());
        assert!(a[1].abs() < 0.05 && a[2].abs() < 0.05, "{a:?}");
    }
    assert!(peak < 0.35, "past the stop: {peak}");
    assert!(
        (angles(&mut world)[0].abs() - 0.3).abs() < 0.03,
        "{:?}",
        angles(&mut world)
    );
}

/// A ball of radius 5 that cannot turn, sliding at 100 units/s on a floor
/// contact the way the ragdoll's near callback makes them (mode 0x301c, mu 0.5) under the
/// ragdoll world: friction stops it in about `v / (mu g)` = 0.25 s. Under
/// the surface layer (1) the contact pushes back nothing but its soft CFM's
/// give, so the ball sinks into it and rests a little past.
#[test]
fn a_contact_holds_a_body_up_and_friction_stops_it() {
    let mut world = World::new();
    world.params = Params::ragdoll();
    let b = world.create_body();
    world.body_mut(b).pos = [0.0, 0.0, 5.0];
    world.body_mut(b).lvel = [100.0, 0.0, 0.0];
    // Too stiff to roll, so it slides.
    let big = 1e6;
    world.body_mut(b).set_mass(Mass {
        mass: 1.0,
        c: [0.0; 3],
        i: [[big, 0.0, 0.0], [0.0, big, 0.0], [0.0, 0.0, big]],
    });
    let surface = Surface {
        mode: APPROX1_1 | APPROX1_2 | SOFT_CFM | SOFT_ERP | BOUNCE,
        mu: 0.5,
        soft_erp: 0.3,
        soft_cfm: 0.001,
        bounce_vel: 0.1,
        ..Surface::default()
    };
    let mut stopped = None;
    for step in 1..=120 {
        let p = world.body(b).pos;
        let depth = 5.0 - p[2];
        if depth > 0.0 {
            world.create_contact(
                Contact::new(surface, [p[0], p[1], 0.0], [0.0, 0.0, 1.0], depth),
                Some(b),
                None,
            );
        }
        world.quick_step(0.017);
        world.zero_forces();
        world.empty_contacts();
        if stopped.is_none() && world.body(b).lvel[0].abs() < 1.0 {
            stopped = Some(step as f32 * 0.017);
        }
    }
    let t = stopped.expect("never stopped");
    assert!((0.2..0.35).contains(&t), "stopped at {t}");
    let z = world.body(b).pos[2];
    assert!((3.9..4.0).contains(&z), "rests at {z}");
}

/// IW's `addLimot` (0x5b3403..0x5b3420): an unmoving motor (`vel` 0, the
/// ragdoll's joint friction) at the high stop presses into it, where
/// upstream 0.5's pushed body 1 away; at the low stop both press into it.
#[test]
fn friction_at_either_stop_presses_into_it() {
    use super::joint::Info2;
    use super::joint::limot::LimitMotor;
    let world = World::new();
    let body = super::body::Body::new(world.params.adis, false);
    let torque_on_body1 = |limit: i32| {
        let mut l = LimitMotor::new(&world.params);
        l.fmax = 10.0;
        l.lostop = -1.0;
        l.histop = 1.0;
        l.limit = limit;
        let (mut rows, mut c, mut cfm, mut findex) = ([Row::default()], [0.0], [0.0], [-1]);
        let mut info = Info2 {
            fps: 1.0 / 0.017,
            erp: 0.8,
            rows: &mut rows,
            c: &mut c,
            cfm: &mut cfm,
            findex: &mut findex,
            torque: Vec::new(),
        };
        assert!(l.add_limot([Some(&body), None], &mut info, 0, [0.0, 0.0, 1.0]));
        assert_eq!(info.torque.len(), 1);
        info.torque[0].1
    };
    // A torque on body 1 along the axis raises the hinge's angle.
    assert_eq!(torque_on_body1(2), [0.0, 0.0, 10.0]);
    assert_eq!(torque_on_body1(1), [0.0, 0.0, -10.0]);
}

/// `msecStep` off the awake count: 17 up to 32, then `(awake - 32) / 18`
/// of the way to the world's ceiling, truncated: 67 for worlds 0 and 1,
/// 34 for world 2 (0x71e7a4, 0x71e7b0).
#[test]
fn each_worlds_msec_step_follows_its_awake_count() {
    use super::phys::{WorldId, msec_step};
    for w in [WorldId::Dynent, WorldId::Fx] {
        let got: Vec<i32> = [0, 32, 33, 41, 49, 50, 512]
            .iter()
            .map(|&n| msec_step(w, n))
            .collect();
        assert_eq!(got, [17, 17, 19, 42, 64, 67, 67], "{w:?}");
    }
    let got: Vec<i32> = [0, 32, 33, 41, 49, 50, 512]
        .iter()
        .map(|&n| msec_step(WorldId::Ragdoll, n))
        .collect();
    assert_eq!(got, [17, 17, 17, 25, 33, 34, 34]);
}

/// 0x5a62f0: worlds 0 and 1 scale both velocities by `1 - drag dt`
/// (floored at 0), world 2 only damps the spin by a hundredth, and a
/// sleeping body is neither touched nor counted.
#[test]
fn the_drag_arm_is_worlds_zero_and_ones_and_the_damping_world_twos() {
    use super::body::DISABLED;
    use super::phys::{WorldId, drag};
    use super::world::PhysDvars;
    let dv = PhysDvars::default();
    let moving = || {
        let mut w = World::new();
        let id = w.create_body();
        let b = w.body_mut(id);
        b.lvel = [100.0, 0.0, -50.0];
        b.avel = [0.0, 100.0, 0.0];
        b.clone()
    };
    let dt = 0.017;
    for w in [WorldId::Dynent, WorldId::Fx] {
        let mut b = moving();
        assert!(drag(w, &mut b, dt, &dv));
        let kl = (1.0 - 0.03f32 as f64 * dt as f64) as f32;
        let ka = (1.0 - 0.5 * dt as f64) as f32;
        assert_eq!(b.lvel, [100.0 * kl, 0.0, -50.0 * kl]);
        assert_eq!(b.avel, [0.0, 100.0 * ka, 0.0]);
    }
    let mut b = moving();
    assert!(drag(WorldId::Ragdoll, &mut b, dt, &dv));
    assert_eq!(b.lvel, [100.0, 0.0, -50.0]);
    // 100 less its stored hundredth, -1
    assert_eq!(b.avel, [0.0, 99.0, 0.0]);
    // a step past 1 / drag stops the body dead, never reverses it
    let mut b = moving();
    drag(WorldId::Fx, &mut b, 40.0, &dv);
    assert_eq!((b.lvel, b.avel), ([0.0, 0.0, -0.0], [0.0; 3]));
    let mut b = moving();
    b.flags |= DISABLED;
    assert!(!drag(WorldId::Dynent, &mut b, dt, &dv));
    assert_eq!(b.lvel, [100.0, 0.0, -50.0]);
}

/// `Phys_ObjAddForce` (0x5a5900): the push times `1000 / msecStep` goes on
/// the force, its moment about the centre on the torque
/// (`dBodyAddForceAtPos` 0x5b7000), and the body wakes.
#[test]
fn a_push_lands_as_a_force_over_one_step_at_its_point() {
    use super::body::DISABLED;
    use super::phys::obj_add_force;
    let mut w = World::new();
    let id = w.create_body();
    let b = w.body_mut(id);
    b.pos = [10.0, 0.0, 0.0];
    b.flags |= DISABLED;
    b.adis_timeleft = 0.0;
    obj_add_force(b, 20, [10.0, 2.0, 0.0], [0.0, 0.0, 3.0]);
    assert_eq!(b.facc, [0.0, 0.0, 150.0]);
    // (0, 2, 0) x (0, 0, 150)
    assert_eq!(b.tacc, [300.0, 0.0, 0.0]);
    assert!(b.enabled());
    assert_eq!(b.adis_timeleft, b.adis.idle_time);
    // world 0's 67 ms step at its most bodies spreads the same push thinner
    let mut b = b.clone();
    b.facc = [0.0; 3];
    let at = b.pos;
    obj_add_force(&mut b, 67, at, [0.0, 0.0, 3.0]);
    assert_eq!(
        b.facc,
        [0.0, 0.0, (3.0 * (1000.0f64 / 67.0) as f32 as f64) as f32]
    );
}

/// `Phys_ObjBulletImpact` (0x5a5a20) through the centre of a resting body:
/// no lever, so `J = speed m scale / (m + 0.5)` along the direction tipped
/// up by `phys_bulletUpBias`, pushed over the named world's step.
#[test]
fn a_bullet_through_the_centre_pushes_without_spin() {
    use super::phys::obj_bullet_impact;
    use super::world::PhysDvars;
    let mut w = World::new();
    let id = w.create_body();
    let b = w.body_mut(id);
    let mut m = Mass::unit();
    m.adjust(2.0);
    b.set_mass(m);
    let dv = PhysDvars {
        bullet_up_bias: 0.0,
        ..PhysDvars::default()
    };
    obj_bullet_impact(b, 20, [0.0; 3], [1.0, 0.0, 0.0], 100.0, 0.5, &dv);
    let j = (100.0f64 * (2.0 * 0.5) / 2.5) as f32;
    assert_eq!(b.facc, [(j as f64 * 50.0) as f32, 0.0, 0.0]);
    assert_eq!(b.tacc, [0.0; 3]);
    // a body already closing faster than the round is left alone
    let mut b2 = b.clone();
    b2.facc = [0.0; 3];
    b2.lvel = [150.0, 0.0, 0.0];
    obj_bullet_impact(&mut b2, 20, [0.0; 3], [1.0, 0.0, 0.0], 100.0, 0.5, &dv);
    assert_eq!(b2.facc, [0.0; 3]);
}

/// A model flat on an axis with no `physGeoms`: launched (0x5a5710) it
/// takes retail's -50..50 box (0x5a587a), as a dynamic entity (0x490240)
/// its bounds as they are, or with the preset's +0x28 set a cylinder on
/// z of half the larger level size and half the height (0x5a54a0). Each
/// sits at its bounds' midpoint at the preset's mass.
#[test]
fn a_flat_model_launches_as_a_cube_and_lies_as_its_bounds() {
    use super::geom::Geom;
    use super::obj::ObjPreset;
    use super::phys::{PhysWorld, WorldId};
    use super::world::PhysDvars;
    use crate::assets::model::XModel;
    let flat = XModel {
        mins: [-4.0, -2.0, 1.0],
        maxs: [4.0, 2.0, 1.0],
        ..XModel::default()
    };
    let p = ObjPreset {
        sound: 3,
        mass: 6.0,
        bounce: 0.1,
        friction: 0.5,
        cylinder: false,
    };
    let dv = PhysDvars::default();
    let at = [10.0, 20.0, 30.0];
    let build = |world, f: &dyn Fn(&mut PhysWorld, BodyId)| {
        let mut pw = PhysWorld::new(world);
        let id = pw.obj_create(at, &IDENTITY, [0.0; 3], &p, &dv).unwrap();
        f(&mut pw, id);
        let (b, o) = (pw.world.body(id).clone(), pw.obj(id).unwrap().clone());
        (b, o)
    };
    let (b, o) = build(WorldId::Fx, &|pw, id| {
        pw.set_collision_from_xmodel(id, &flat)
    });
    assert_eq!(o.geoms, [Geom::Box { sides: [100.0; 3] }]);
    assert_eq!(b.mass, Mass::box_total(6.0, 100.0, 100.0, 100.0));
    assert_eq!((b.pos, o.offset, o.sound), (at, [0.0; 3], 3));
    let (b, o) = build(WorldId::Dynent, &|pw, id| {
        pw.dynent_collision(id, &flat, false)
    });
    assert_eq!(
        o.geoms,
        [Geom::Box {
            sides: [8.0, 4.0, 0.0]
        }]
    );
    assert_eq!((b.pos, o.offset), ([10.0, 20.0, 31.0], [0.0, 0.0, -1.0]));
    let tall = XModel {
        mins: [-2.0, -4.0, 0.0],
        maxs: [2.0, 4.0, 6.0],
        ..XModel::default()
    };
    let (b, o) = build(WorldId::Dynent, &|pw, id| {
        pw.dynent_collision(id, &tall, true)
    });
    let cyl = Geom::Cylinder {
        direction: 3,
        radius: 4.0,
        half_height: 3.0,
    };
    assert_eq!(o.geoms, [cyl]);
    assert_eq!(b.mass, Mass::cylinder_total(6.0, 3, 4.0, 3.0));
    assert_eq!(b.pos, [10.0, 20.0, 33.0]);
}

/// A tyre (`physGeoms` type 4 in a transform turning its axis onto z,
/// radius 11.95, `+0x54` 3.5, mass 18) let go 10 over a floor brush in
/// world 0 steps through its cylinder collider and comes to rest on its
/// cap a unit under its half height (`phys_csl`'s surface layer), asleep.
#[test]
fn a_tyre_dropped_on_a_floor_rests_on_its_cap() {
    use super::collide::tests::{Brushes, cuboid};
    use super::obj::{ObjPreset, Part, PartKind};
    use super::phys::{PhysWorld, WorldId};
    use super::world::PhysDvars;
    let floor = Brushes(vec![cuboid([-256.0, -256.0, -32.0], [256.0, 256.0, 0.0])]);
    let dv = PhysDvars::default();
    let p = ObjPreset {
        sound: 0,
        mass: 18.0,
        bounce: 0.1,
        friction: 0.8,
        cylinder: false,
    };
    let mut pw = PhysWorld::new(WorldId::Dynent);
    let id = pw
        .obj_create([0.0, 0.0, 10.0], &IDENTITY, [0.0; 3], &p, &dv)
        .unwrap();
    let part = Part {
        kind: PartKind::Cylinder {
            direction: 1,
            radius: 11.95,
            length: 3.5,
        },
        transform: Some([[0.0, 0.0, 1.0], [0.0, 1.0, 0.0], [-1.0, 0.0, 0.0]]),
    };
    pw.add_part(id, 18.0, &part, [0.0; 3]);
    let contact = super::collide::Dvars::default();
    let mut asleep = None;
    for frame in 1..=300 {
        pw.run_to_time(frame * 17, &dv, &contact, &floor, &mut |_| {});
        if pw.world.body(id).flags & super::body::DISABLED != 0 {
            asleep = Some(frame);
            break;
        }
    }
    let z = pw.world.body(id).pos[2];
    assert!(asleep.is_some(), "still awake at z {z}");
    assert!((2.4..2.6).contains(&z), "rests at {z}");
}

/// 0x5a684f on the sedan hood's brush (-3.5..41.5, -24..24, -8..0): a
/// hundredth of its bounds, x cut to z's 0.08 about its middle, y too;
/// its top x, 0.23, sizes the post-frame sweep, a point under 2 `phys_csl`.
#[test]
fn a_brush_geoms_sweep_size_is_its_bounds_cut_to_its_height() {
    let brush = std::sync::Arc::new(crate::assets::model::PhysBrush {
        mins: [-3.5, -24.0, -8.0],
        maxs: [41.5, 24.0, 0.0],
        ..Default::default()
    });
    let g = super::geom::Geom::Brush {
        brush,
        offset: [0.0; 3],
    };
    let (lo, hi) = g.bounds();
    let close = |a: [f32; 3], b: [f32; 3]| (0..3).all(|k| (a[k] - b[k]).abs() < 1e-6);
    assert!(close(lo, [0.15, -0.04, -0.08]), "{lo:?}");
    assert!(close(hi, [0.23, 0.04, 0.0]), "{hi:?}");
}

/// 0x43de60's handles: a surface with its own alias takes it, the rest
/// the prefix's `_default`, a row with no prefix `collision_default` in
/// every slot, and an alias not loaded none.
#[test]
fn impact_rows_take_the_surfaces_alias_or_the_prefixs_default() {
    use super::collide::SURFACE_TYPES;
    use super::obj::impact_aliases;
    let have = [
        "physics_tire_default",
        "physics_tire_metal",
        "collision_default",
    ];
    let rows = [
        "physics_tire".to_string(),
        String::new(),
        "nothing".to_string(),
    ];
    let a = impact_aliases(&rows, |n| have.contains(&n));
    assert_eq!(a.len(), 3 * SURFACE_TYPES);
    let name = |k: usize| a[k].as_deref();
    assert_eq!(name(13), Some("physics_tire_metal"));
    assert_eq!(name(0), Some("physics_tire_default"));
    assert_eq!(name(21), Some("physics_tire_default"));
    assert!((SURFACE_TYPES..2 * SURFACE_TYPES).all(|k| name(k) == Some("collision_default")));
    assert!((2 * SURFACE_TYPES..3 * SURFACE_TYPES).all(|k| name(k).is_none()));
}

/// The body pool is one across the worlds: a world whose others hold all
/// but one slot makes one body and then none.
#[test]
fn the_body_pool_is_shared_by_the_worlds() {
    use super::phys::{MAX_BODIES, PhysWorld, WorldId};
    let mut pw = PhysWorld::new(WorldId::Fx);
    pw.others = MAX_BODIES - 1;
    assert!(pw.create_body().is_some());
    assert!(pw.create_body().is_none());
}
