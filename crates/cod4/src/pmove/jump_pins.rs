//! Pins on the jump, the step and the landing where no retail capture
//! reaches: a jump on a slope, steps at and past the step height, a jump
//! onto a ledge near its height, the landing's stun tiers and the jump
//! slowdown's two penalties. Each runs cmds in a world of convex brushes
//! and folds every cmd's moved fields into one digest; `cod4_moves_ab`
//! holds the covered arms to retail. The digests follow `PM_SlideMove`'s
//! stored time left (`cod4_lnxded` 0x805e6e9, cod4-pmove.md 5), which no
//! capture tells from a kept one.

use glam::Vec3;

use super::super::*;

/// The sweep's stand-off from a face, as the clip map keeps it.
const EPS: f32 = 0.125;
/// Asphalt, so the landings and steps carry a surface.
const ASPHALT: u32 = 22 << 20;

/// A convex brush: half-spaces `n . p <= dist` and the surface it reports.
struct Brush {
    planes: Vec<(Vec3, f32)>,
    surface_flags: u32,
}

impl Brush {
    /// An axial box.
    fn cuboid(lo: Vec3, hi: Vec3) -> Self {
        let planes = vec![
            (Vec3::X, hi.x),
            (Vec3::NEG_X, -lo.x),
            (Vec3::Y, hi.y),
            (Vec3::NEG_Y, -lo.y),
            (Vec3::Z, hi.z),
            (Vec3::NEG_Z, -lo.z),
        ];
        Brush {
            planes,
            surface_flags: ASPHALT,
        }
    }

    /// Everything under `z = slope * x + base`.
    fn ramp(slope: f32, base: f32) -> Self {
        let len = (slope * slope + 1.0).sqrt();
        let n = Vec3::new(-slope / len, 0.0, 1.0 / len);
        Brush {
            planes: vec![(n, base / len)],
            surface_flags: ASPHALT,
        }
    }

    /// Cut by one more half-space.
    fn and(mut self, n: Vec3, dist: f32) -> Self {
        self.planes.push((n, dist));
        self
    }

    fn flags(mut self, f: u32) -> Self {
        self.surface_flags = f;
        self
    }

    /// The box sweep's first contact as (fraction, normal), or the start
    /// inside the brush.
    fn sweep(&self, start: Vec3, end: Vec3, mins: Vec3, maxs: Vec3) -> Option<(f32, Vec3, bool)> {
        let (mut enter, mut leave) = (-1.0f32, 1.0f32);
        let mut face = Vec3::ZERO;
        let (mut starts_out, mut ends_out) = (false, false);
        for &(n, dist) in &self.planes {
            // The corner of the box deepest along -n meets the face first.
            let corner = Vec3::select(n.cmplt(Vec3::ZERO), maxs, mins);
            let dist = dist - n.dot(corner);
            let (a, b) = (n.dot(start) - dist, n.dot(end) - dist);
            starts_out |= a > 0.0;
            ends_out |= b > 0.0;
            if a > 0.0 && (b >= EPS || b >= a) {
                return None;
            }
            if a <= 0.0 && b <= 0.0 {
                continue;
            }
            if a > b {
                let f = ((a - EPS) / (a - b)).max(0.0);
                if f > enter {
                    enter = f;
                    face = n;
                }
            } else {
                leave = leave.min((a + EPS) / (a - b));
            }
        }
        if !starts_out {
            return Some((0.0, Vec3::ZERO, !ends_out));
        }
        (enter < leave && enter > -1.0).then_some((enter.max(0.0), face, false))
    }
}

struct World(Vec<Brush>);

impl PmWorld for World {
    fn trace(&self, start: Vec3, end: Vec3, mins: Vec3, maxs: Vec3, _pass: u32, _mask: u32) -> Tr {
        let mut t = Tr::default();
        for b in &self.0 {
            let Some((f, n, all)) = b.sweep(start, end, mins, maxs) else {
                continue;
            };
            if f == 0.0 && n == Vec3::ZERO {
                t.startsolid = true;
                t.allsolid |= all;
                t.fraction = 0.0;
                t.entity_num = ENTITYNUM_WORLD as u32;
                t.contents = 1;
                continue;
            }
            if f < t.fraction {
                t.fraction = f;
                t.normal = n;
                t.walkable = n.z >= 0.7;
                t.surface_flags = b.surface_flags;
                t.entity_num = ENTITYNUM_WORLD as u32;
                t.contents = 1;
            }
        }
        t
    }
}

fn floor() -> Brush {
    Brush::cuboid(
        Vec3::new(-4096.0, -4096.0, -64.0),
        Vec3::new(4096.0, 4096.0, 0.0),
    )
}

/// FNV-1a over 32-bit words.
#[derive(Default)]
struct Digest(u64, f32);

impl Digest {
    fn word(&mut self, w: u32) {
        let h = if self.0 == 0 {
            0xcbf2_9ce4_8422_2325
        } else {
            self.0
        };
        self.0 = (h ^ u64::from(w)).wrapping_mul(0x0100_0000_01b3);
    }

    fn vec(&mut self, v: Vec3) {
        for c in v.to_array() {
            self.word(c.to_bits());
        }
    }

    fn state(&mut self, ps: &PlayerState) {
        self.1 = self.1.max(ps.origin.z);
        self.vec(ps.origin);
        self.vec(ps.velocity);
        for w in [
            ps.pm_flags,
            ps.pm_time as u32,
            ps.jump_time as u32,
            ps.jump_origin_z.to_bits(),
            ps.ground_entity_num as u32,
            ps.bob_cycle as u32,
            ps.event_sequence as u32,
            ps.aim_spread_scale.to_bits(),
        ] {
            self.word(w);
        }
        for i in 0..4 {
            self.word(ps.events[i]);
            self.word(ps.event_parms[i]);
        }
    }
}

/// `n` 50 ms cmds of `buttons` and `forward` keys, the yaw short `yaw`.
#[derive(Clone, Copy)]
struct Key {
    n: usize,
    buttons: u32,
    forward: i8,
    right: i8,
}

const fn key(n: usize, buttons: u32, forward: i8, right: i8) -> Key {
    Key {
        n,
        buttons,
        forward,
        right,
    }
}

fn play(ps: &mut PlayerState, keys: &[Key], world: &World, rules: Rules, d: &mut Digest) {
    let mut old = Cmd {
        server_time: ps.command_time,
        ..Default::default()
    };
    for k in keys {
        for _ in 0..k.n {
            let c = Cmd {
                server_time: ps.command_time + 50,
                buttons: k.buttons,
                forward_move: k.forward,
                right_move: k.right,
                ..Default::default()
            };
            player_step(ps, &c, &mut old, world, rules, WeaponMove::NONE);
            d.state(ps);
        }
    }
}

fn standing(at: Vec3) -> PlayerState {
    let mut ps = PlayerState::spawn(at, 0.0, 1000);
    ps.ground_entity_num = ENTITYNUM_WORLD;
    ps
}

/// Jump, release, jump: the key's edge every `gap` cmds, `times` times.
fn hops(gap: usize, times: usize, forward: i8) -> Vec<Key> {
    (0..times)
        .flat_map(|_| {
            [
                key(1, button::JUMP, forward, 0),
                key(gap - 1, 0, forward, 0),
            ]
        })
        .collect()
}

#[test]
fn a_jump_up_and_down_a_slope_pins() {
    let mut d = Digest::default();
    // A walkable 0.4 grade and a steep 0.78 one, each climbed and descended.
    for slope in [0.4f32, 0.8] {
        let world = World(vec![floor(), Brush::ramp(slope, 0.0)]);
        let mut ps = standing(Vec3::new(20.0, 0.0, slope * 20.0 + 16.0));
        let mut keys = vec![key(6, 0, 0, 0), key(4, 0, 127, 0)];
        keys.extend(hops(8, 4, 127));
        keys.extend(hops(12, 2, 127));
        keys.push(key(6, 0, -127, 40));
        keys.extend(hops(6, 3, -127));
        play(&mut ps, &keys, &world, Rules::default(), &mut d);
        assert!(ps.origin.x > 0.0, "still on the ramp: {}", ps.origin);
    }
    assert_eq!(d.0, 0x1cc2_dfc5_c968_65a3);
}

#[test]
fn steps_up_and_down_at_the_step_height_pin() {
    let mut d = Digest::default();
    // 18 steps, 18.5 does not; 10 is the prone's and 10.5 not; then the
    // walk off each top.
    for (top, buttons) in [
        (18.0f32, 0),
        (18.5, 0),
        (17.0, button::CROUCH),
        (10.0, button::PRONE),
        (10.5, button::PRONE),
    ] {
        let world = World(vec![
            floor(),
            Brush::cuboid(Vec3::new(40.0, -512.0, 0.0), Vec3::new(120.0, 512.0, top)),
        ]);
        let mut ps = standing(Vec3::new(0.0, 0.0, 0.125));
        let keys = [
            key(8, buttons, 0, 0),
            key(30, buttons, 127, 0),
            key(4, buttons, 127, 70),
            key(30, buttons, -127, 0),
        ];
        d.1 = 0.0;
        play(&mut ps, &keys, &world, Rules::default(), &mut d);
        let climbed = d.1 >= top;
        let limit = if buttons == button::PRONE { 10.0 } else { 18.0 };
        assert_eq!(climbed, top <= limit, "top {top}, peak {}", d.1);
    }
    // Off a ledge onto a face too steep to land on.
    let world = World(vec![
        floor(),
        Brush::cuboid(Vec3::new(-512.0, -512.0, 0.0), Vec3::new(40.0, 512.0, 30.0)),
        Brush::ramp(-4.0, 190.0)
            .and(Vec3::NEG_X, -40.0)
            .and(Vec3::Z, 30.0),
    ]);
    let mut ps = standing(Vec3::new(0.0, 0.0, 30.125));
    play(
        &mut ps,
        &[key(4, 0, 0, 0), key(40, 0, 30, 0)],
        &world,
        Rules::default(),
        &mut d,
    );
    assert_eq!(d.0, 0x5d84_51ad_97c5_9b31);
}

#[test]
fn a_jump_onto_a_ledge_near_the_jump_height_pins() {
    let mut d = Digest::default();
    // Ledges the jump's own step reaches, one at its height and one past.
    for top in [20.0f32, 30.0, 36.0, 38.9, 39.0, 41.0, 56.0] {
        let world = World(vec![
            floor(),
            Brush::cuboid(Vec3::new(30.0, -512.0, 0.0), Vec3::new(200.0, 512.0, top)),
        ]);
        // A run-up of none (at the wall) to four cmds.
        for (from, run) in [(14.0f32, 0), (0.0, 1), (0.0, 3), (-40.0, 4)] {
            let mut ps = standing(Vec3::new(from, 0.0, 0.125));
            let keys = [
                key(4, 0, 0, 0),
                key(run, 0, 127, 0),
                key(1, button::JUMP, 127, 0),
                key(30, 0, 127, 0),
            ];
            play(&mut ps, &keys, &world, Rules::default(), &mut d);
        }
    }
    // A sloped ledge: the landing's projection rises faster than what is
    // left of the jump.
    for base in [20.0f32, 30.0, 34.0, 37.0, 38.0] {
        let world = World(vec![
            floor(),
            Brush::ramp(0.6, base - 18.0)
                .and(Vec3::NEG_X, -30.0)
                .and(Vec3::X, 200.0)
                .and(Vec3::NEG_Z, 0.0),
        ]);
        let mut ps = standing(Vec3::new(0.0, 0.0, 0.125));
        let keys = [
            key(4, 0, 0, 0),
            key(3, 0, 127, 0),
            key(1, button::JUMP, 127, 0),
            key(20, 0, 127, 0),
        ];
        play(&mut ps, &keys, &world, Rules::default(), &mut d);
    }
    assert_eq!(d.0, 0x79d7_0c81_f997_1f04);
}

#[test]
fn landings_stun_by_fall_height() {
    let mut d = Digest::default();
    let mut stuns = Vec::new();
    // Below a step, each footstep tier, the land, the damage's linear
    // stretch on both sides of 500 and 1500 ms of stun, and past the cap.
    let heights = [
        3.0f32, 6.0, 10.0, 20.0, 129.0, 140.0, 160.0, 200.0, 240.0, 280.0, 299.0, 340.0,
    ];
    for (flags, dead) in [
        (ASPHALT, false),
        (ASPHALT | 1, false),
        (ASPHALT | 2, false),
        (0, false),
        (ASPHALT, true),
    ] {
        for h in heights {
            let world = World(vec![floor().flags(flags)]);
            let mut ps = PlayerState::spawn(Vec3::new(0.0, 0.0, h), 0.0, 1000);
            ps.ground_entity_num = ENTITYNUM_NONE;
            ps.velocity = Vec3::new(120.0, 0.0, 0.0);
            if dead {
                ps.pm_type = pm_type::DEAD;
            }
            let mut landed = None;
            let mut old = Cmd {
                server_time: ps.command_time,
                ..Default::default()
            };
            for _ in 0..30 {
                let c = Cmd {
                    server_time: ps.command_time + 50,
                    forward_move: 127,
                    ..Default::default()
                };
                let air = !ps.on_ground();
                player_step(
                    &mut ps,
                    &c,
                    &mut old,
                    &world,
                    Rules::default(),
                    WeaponMove::NONE,
                );
                if air && ps.on_ground() && landed.is_none() {
                    landed = Some(ps.pm_time);
                }
                d.state(&ps);
            }
            if flags == ASPHALT && !dead {
                stuns.push(landed.unwrap_or(-1));
            }
        }
    }
    // No stun under the damage height, a longer one each step up, capped.
    assert!(stuns[..5].iter().all(|&t| t == 0), "{stuns:?}");
    // A fatal fall damps the speed but stuns nothing.
    assert!(stuns[5..9].windows(2).all(|w| w[0] < w[1]), "{stuns:?}");
    assert_eq!(stuns[8..], [2000, 2000, 2000, 0], "{stuns:?}");
    assert_eq!(d.0, 0x200a_4adf_1942_f295);
}

#[test]
fn the_jump_slowdown_takes_its_two_penalties() {
    let mut d = Digest::default();
    let mut landed = Vec::new();
    // On the flat (1800 ms at 0.65), onto a ledge 18 up or more (1200 ms
    // at 0.5), with the slowdown off, and a second jump inside the first's
    // penalty.
    for (top, enable) in [
        (0.0f32, true),
        (17.0, true),
        (18.0, true),
        (24.0, true),
        (0.0, false),
        (24.0, false),
    ] {
        let mut brushes = vec![floor()];
        if top > 0.0 {
            brushes.push(Brush::cuboid(
                Vec3::new(20.0, -512.0, 0.0),
                Vec3::new(400.0, 512.0, top),
            ));
        }
        let world = World(brushes);
        let rules = Rules {
            jump_slowdown_enable: enable,
            ..Rules::default()
        };
        let mut ps = standing(Vec3::new(0.0, 0.0, 0.125));
        play(
            &mut ps,
            &[
                key(4, 0, 0, 0),
                key(2, 0, 127, 0),
                key(1, button::JUMP, 127, 0),
            ],
            &world,
            rules,
            &mut d,
        );
        let mut old = Cmd {
            server_time: ps.command_time,
            buttons: button::JUMP,
            ..Default::default()
        };
        let mut first = None;
        for i in 0..50 {
            let c = Cmd {
                server_time: ps.command_time + 50,
                forward_move: 127,
                // A second jump 300 ms after the first landing.
                buttons: if first.is_some_and(|f| i == f + 6) {
                    button::JUMP
                } else {
                    0
                },
                ..Default::default()
            };
            let air = !ps.on_ground();
            player_step(&mut ps, &c, &mut old, &world, rules, WeaponMove::NONE);
            if air && ps.on_ground() && first.is_none() {
                first = Some(i);
            }
            // The penalty starts on the first cmd walked after the landing.
            if first.is_some_and(|f| i == f + 1) {
                landed.push((ps.pm_time, ps.velocity.x));
            }
            d.state(&ps);
        }
    }
    // A jump off a 200 ledge lands hard with the jump still flagged.
    let world = World(vec![
        floor(),
        Brush::cuboid(Vec3::new(-512.0, -512.0, 0.0), Vec3::new(0.0, 512.0, 200.0)),
    ]);
    let mut ps = standing(Vec3::new(-20.0, 0.0, 200.125));
    let keys = [
        key(4, 0, 0, 0),
        key(1, button::JUMP, 127, 0),
        key(40, 0, 127, 0),
    ];
    play(&mut ps, &keys, &world, Rules::default(), &mut d);
    assert!(ps.pm_flags & pmf::JUMPING == 0);
    let penalties: Vec<i32> = landed.iter().map(|l| l.0).collect();
    assert_eq!(penalties[..4], [1800, 1800, 1800, 1200], "{landed:?}");
    assert_eq!(d.0, 0xe676_0b8d_c5fc_610c);
}

/// `PM_ProjectVelocity` over a handful of velocities and planes. The new z
/// is kept unrounded into its product (0x8056316).
#[test]
fn the_projection_keeps_the_flat_length_on_its_planes() {
    let mut d = Digest::default();
    let planes = [
        Vec3::Z,
        Vec3::new(0.0, 0.0, -1.0),
        Vec3::new(0.3714, 0.0, 0.9285),
        Vec3::new(-0.6, 0.0, 0.8),
        Vec3::new(0.2, -0.3, 0.9327),
        Vec3::new(1.0, 0.0, 0.0005),
        Vec3::new(0.6, 0.8, 0.0),
    ];
    let velocities = [
        Vec3::new(190.0, 0.0, 0.0),
        Vec3::new(-150.0, 60.0, 0.0),
        Vec3::new(100.0, 0.0, 80.0),
        Vec3::new(100.0, 20.0, -240.0),
        Vec3::new(0.0, 0.0, -300.0),
        Vec3::new(33.3, -77.7, 12.5),
    ];
    for n in planes {
        for v in velocities {
            d.vec(super::project_velocity(v, n));
        }
    }
    assert_eq!(
        super::project_velocity(Vec3::new(0.0, 0.0, -9.0), Vec3::Z),
        Vec3::new(0.0, 0.0, -9.0)
    );
    assert_eq!(d.0, 0xc561_f777_d177_cc41);
}

/// `PM_Friction` on a jumping body at each point of the slowdown curve.
#[test]
fn friction_follows_the_jump_curve() {
    let mut d = Digest::default();
    for enable in [true, false] {
        let rules = Rules {
            jump_slowdown_enable: enable,
            ..Rules::default()
        };
        for t in [0, 1, 50, 850, 1699, 1700, 1750, 1800, 1801, 2500] {
            for land in [false, true] {
                let mut ps = standing(Vec3::new(0.0, 0.0, 0.125));
                ps.velocity = Vec3::new(180.0, -40.0, 0.0);
                ps.pm_flags |= pmf::JUMPING;
                if land {
                    ps.pm_flags |= pmf::TIME_LAND;
                }
                ps.pm_time = t;
                ps.jump_origin_z = 12.0;
                let pml = Pml {
                    walking: true,
                    frametime: 0.05,
                    msec: 50,
                    ..Default::default()
                };
                super::super::moves::friction(&mut ps, &pml, &rules);
                d.state(&ps);
            }
        }
    }
    assert_eq!(d.0, 0xcbcd_b771_90f0_d0f5);
}
