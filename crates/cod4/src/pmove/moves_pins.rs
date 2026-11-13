//! Pins on the walk's scales, the fly, the movement direction and the
//! footstep cycle where no retail capture reaches: a damage slowdown, a
//! back strafe, the prone blend, a sprint, the sighted walk, the inertia
//! clamp, a flying spectator and the ladder's bob. Each runs cmds on a flat
//! floor and folds every cmd's moved fields into one digest; `cod4_moves_ab`
//! holds the covered arms to retail.

use glam::Vec3;

use super::super::*;

/// A floor at z 0, solid below.
struct Floor;

impl PmWorld for Floor {
    fn trace(&self, start: Vec3, end: Vec3, mins: Vec3, _maxs: Vec3, _pass: u32, _mask: u32) -> Tr {
        let (a, b) = (start.z + mins.z, end.z + mins.z);
        let mut t = Tr::default();
        if a < 0.0 {
            t.fraction = 0.0;
            t.startsolid = true;
            t.allsolid = b < 0.0;
        } else if b < 0.125 && b < a {
            t.fraction = ((a - 0.125) / (a - b)).max(0.0);
        } else {
            return t;
        }
        t.normal = Vec3::Z;
        t.walkable = true;
        t.entity_num = ENTITYNUM_WORLD as u32;
        t.contents = 1;
        // Asphalt, so the footsteps carry a surface.
        t.surface_flags = 22 << 20;
        t
    }
}

const RIFLE: WeaponMove = WeaponMove {
    move_speed_scale: 0.95,
    ads_move_speed_scale: 0.5,
    sprint_duration_scale: 1.0,
};

/// FNV-1a over 32-bit words.
#[derive(Default)]
struct Digest(u64);

impl Digest {
    fn word(&mut self, w: u32) {
        let h = if self.0 == 0 {
            0xcbf2_9ce4_8422_2325
        } else {
            self.0
        };
        self.0 = (h ^ u64::from(w)).wrapping_mul(0x0100_0000_01b3);
    }

    fn state(&mut self, ps: &PlayerState) {
        for v in [ps.origin, ps.velocity] {
            for c in v.to_array() {
                self.word(c.to_bits());
            }
        }
        for w in [
            ps.bob_cycle as u32,
            ps.movement_dir as u32,
            ps.damage_timer as u32,
            ps.pm_flags,
            ps.event_sequence as u32,
        ] {
            self.word(w);
        }
        for i in 0..4 {
            self.word(ps.events[i]);
            self.word(ps.event_parms[i]);
        }
    }
}

/// One cmd: keys, buttons and the view's yaw short.
#[derive(Clone, Copy)]
struct Key {
    n: usize,
    buttons: u32,
    forward: i8,
    right: i8,
    yaw: u16,
}

const fn key(n: usize, buttons: u32, forward: i8, right: i8) -> Key {
    Key {
        n,
        buttons,
        forward,
        right,
        yaw: 0,
    }
}

/// Runs `keys` as 50 ms cmds and digests the state after each.
fn play(ps: &mut PlayerState, keys: &[Key], rules: Rules, weapon: WeaponMove) -> u64 {
    let mut d = Digest::default();
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
                angles: [0, k.yaw, 0],
                ..Default::default()
            };
            player_step(ps, &c, &mut old, &Floor, rules, weapon);
            d.state(ps);
        }
    }
    d.0
}

fn armed() -> PlayerState {
    let mut ps = PlayerState::spawn(Vec3::new(0.0, 0.0, 0.125), 0.0, 1000);
    ps.ground_entity_num = ENTITYNUM_WORLD;
    ps.weapon = 1;
    ps.move_speed_scale_multiplier = 0.95;
    ps
}

#[test]
fn a_damage_timer_slows_the_walk_until_it_runs_out() {
    let rules = Rules {
        dmgtimer_min_scale: 0.5,
        ..Rules::default()
    };
    let mut hurt = armed();
    hurt.damage_timer = 700;
    let mut whole = armed();
    let keys = [key(20, 0, 127, 50)];
    let pin = play(&mut hurt, &keys[..], rules, RIFLE);
    play(&mut whole, &keys[..], rules, RIFLE);
    assert_eq!(hurt.damage_timer, 0);
    assert!(hurt.origin.x < whole.origin.x);
    assert_eq!(pin, 0x34f7_0ba1_be41_c5a0);
}

#[test]
fn a_crouched_back_strafe_pins() {
    let mut ps = armed();
    let keys = [
        key(6, button::CROUCH, 0, 0),
        key(20, button::CROUCH, -127, 127),
        key(10, button::CROUCH, -127, 0),
        key(10, 0, 0, -127),
    ];
    let pin = play(&mut ps, &keys, Rules::default(), RIFLE);
    assert_eq!(pin, 0xd6cc_84c1_7b4d_b7bf);
}

#[test]
fn going_prone_and_back_up_blends_the_stance_scale() {
    let mut ps = armed();
    let keys = [
        key(4, 0, 127, 0),
        key(16, button::PRONE, 127, 0),
        key(16, button::CROUCH, 127, 0),
        key(8, 0, 127, 0),
    ];
    let mut prone = false;
    let mut pin = 0;
    for (i, k) in keys.iter().enumerate() {
        pin ^= play(&mut ps, &keys[i..=i], Rules::default(), RIFLE).rotate_left(i as u32);
        prone |= k.buttons == button::PRONE && ps.pm_flags & pmf::PRONE != 0;
    }
    assert!(prone);
    assert_eq!(pin, 0x28a1_12fe_b1a8_991d);
}

#[test]
fn a_sprint_bobs_at_the_camera_bob_and_steps_as_a_sprint() {
    let mut ps = armed();
    let keys = [
        key(4, 0, 127, 0),
        key(30, button::SPRINT, 127, 0),
        key(10, button::SPRINT, 127, 90),
    ];
    let pin = play(&mut ps, &keys, Rules::default(), RIFLE);
    assert!(ps.events.contains(&ev::FOOTSTEP_SPRINT));
    assert_eq!(pin, 0x271d_96a1_8b74_f364);
}

#[test]
fn the_sighted_walk_and_the_lean_walk_pin() {
    let mut ps = armed();
    // Raised, as the weapon half (not run here) would leave it.
    ps.pm_flags |= pmf::SIGHT;
    ps.weapon_pos_frac = 1.0;
    let keys = [
        key(12, button::ADS_MODE, 127, 0),
        key(12, button::LEAN_LEFT, 127, -60),
        key(8, 0, 127, 0),
    ];
    let pin = play(&mut ps, &keys[..1], Rules::default(), RIFLE);
    assert!(ps.pm_flags & pmf::SIGHT_WALK != 0);
    let pin = pin ^ play(&mut ps, &keys[1..2], Rules::default(), RIFLE).rotate_left(1);
    assert!(ps.leanf != 0.0);
    ps.pm_flags &= !pmf::SIGHT;
    let pin = pin ^ play(&mut ps, &keys[2..], Rules::default(), RIFLE).rotate_left(2);
    assert_eq!(pin, 0xf5cf_440d_5a00_34de);
}

#[test]
fn a_reversal_past_inertia_angle_is_clamped() {
    let rules = Rules {
        inertia_angle: 0.5,
        ..Rules::default()
    };
    let mut ps = armed();
    let mut keys = vec![key(16, 0, 127, 0), key(10, 0, -127, 0)];
    keys.push(Key {
        yaw: 0x4000,
        ..key(10, 0, 127, 40)
    });
    let mut free = armed();
    let unclamped = play(&mut free, &keys, Rules::default(), RIFLE);
    let pin = play(&mut ps, &keys, rules, RIFLE);
    assert_ne!(pin, unclamped);
    assert_eq!(pin, 0x5a3f_4e7d_cfd5_ca63);
}

#[test]
fn a_spectator_flies_on_its_keys_and_leans() {
    let mut ps = PlayerState::spawn(Vec3::new(0.0, 0.0, 100.0), 0.0, 1000);
    ps.pm_type = pm_type::SPECTATOR;
    ps.speed = 400;
    ps.delta_angles[0] = 30.0;
    let keys = [
        key(10, 0, 127, 40),
        Key {
            yaw: 0x2000,
            ..key(8, button::LEAN_RIGHT, 60, -127)
        },
        key(8, button::LEAN_LEFT, 0, 0),
        key(6, button::LEAN_LEFT | button::LEAN_RIGHT, -127, 0),
    ];
    let pin = play(&mut ps, &keys, Rules::default(), WeaponMove::NONE);
    assert_eq!(pin, 0xaecb_2678_7ea8_5b36);
}

/// The ladder's bob, run straight through `PM_Footsteps` with a climber
/// off the ground: a rung footstep each quarter of the cycle.
#[test]
fn a_climber_bobs_off_his_vertical_speed() {
    let mut d = Digest::default();
    for (walking, vz) in [(false, 100.0), (true, 100.0), (false, -63.0), (true, 37.5)] {
        let mut ps = armed();
        ps.ground_entity_num = ENTITYNUM_NONE;
        ps.pm_flags |= pmf::LADDER;
        if walking {
            ps.pm_flags |= pmf::SIGHT_WALK;
        }
        ps.velocity = Vec3::new(0.0, 0.0, vz);
        ps.ladder_vec = Vec3::new(0.0, -1.0, 0.0);
        ps.jump_time = 0;
        for i in 0..24 {
            let c = Cmd {
                server_time: 2000 + 33 * i,
                ..Default::default()
            };
            let mut pm = Pmove::new(&mut ps, c, c, &Floor, Rules::default());
            let pml = Pml {
                msec: 33,
                ..Default::default()
            };
            super::super::footsteps::footsteps(&mut pm, &pml);
            d.state(&ps);
        }
        assert!(ps.events.contains(&ev::FOOTSTEP_RUN));
    }
    assert_eq!(d.0, 0xea0f_ad92_aa9f_c8cf);
}

/// `PM_SetMovementDir`'s prone and ladder arms, which no walk reaches here.
#[test]
fn the_movement_dir_follows_the_prone_heading_and_the_ladder() {
    let mut d = Digest::default();
    for yaw in [-170.0f32, -95.0, -30.0, 0.0, 45.0, 89.5, 135.0, 179.0] {
        for arm in [pmf::PRONE, pmf::LADDER] {
            let mut ps = armed();
            ps.pm_flags |= arm;
            ps.view_angles[1] = yaw;
            ps.prone_direction = 20.0;
            ps.ladder_vec = Vec3::new(0.6, -0.8, 0.0);
            let c = Cmd::default();
            let mut pm = Pmove::new(&mut ps, c, c, &Floor, Rules::default());
            super::update_movement_dir(&mut pm, &Pml::default());
            d.word(ps.movement_dir as u32);
        }
    }
    assert_eq!(d.0, 0x7242_e734_9db5_bc4a);
}
