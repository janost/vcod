//! Pins on the walk's scales, the fly, the movement direction and the
//! footstep cycle where no retail capture reaches: a damage slowdown, a
//! back strafe, the prone blend, a sprint, the sighted walk, the inertia
//! clamp, a flying spectator and the ladder's bob. Each runs cmds on a flat
//! floor and folds every cmd's moved fields into one digest; `cod4_moves_ab`
//! holds the covered arms to retail.

use glam::Vec3;

use super::super::pin::Pin;
use super::super::*;

use super::super::testing::ASPHALT;

const RIFLE: WeaponMove = WeaponMove {
    move_speed_scale: 0.95,
    ads_move_speed_scale: 0.5,
    sprint_duration_scale: 1.0,
};

/// The state after one cmd, onto `p`'s next step.
fn state(p: &mut Pin, ps: &PlayerState) {
    p.step();
    p.vec("origin", ps.origin);
    p.vec("velocity", ps.velocity);
    p.i32("bob_cycle", ps.bob_cycle);
    p.i32("movement_dir", ps.movement_dir);
    p.i32("damage_timer", ps.damage_timer);
    p.u32("pm_flags", ps.pm_flags);
    p.i32("event_sequence", ps.event_sequence);
    for i in 0..4 {
        p.u32("event", ps.events[i]);
        p.u32("parm", ps.event_parms[i]);
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
fn play(ps: &mut PlayerState, keys: &[Key], rules: Rules, weapon: WeaponMove) -> Pin {
    let mut d = Pin::words();
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
            player_step(ps, &c, &mut old, &ASPHALT, rules, weapon);
            state(&mut d, ps);
        }
    }
    d
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
    pin.check("moves_damage_timer", 0x34f7_0ba1_be41_c5a0);
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
    pin.check("moves_back_strafe", 0xd6cc_84c1_7b4d_b7bf);
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
    let mut pin = Pin::words();
    for (i, k) in keys.iter().enumerate() {
        pin.xor(
            play(&mut ps, &keys[i..=i], Rules::default(), RIFLE),
            i as u32,
        );
        prone |= k.buttons == button::PRONE && ps.pm_flags & pmf::PRONE != 0;
    }
    assert!(prone);
    pin.check("moves_prone_blend", 0x28a1_12fe_b1a8_991d);
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
    pin.check("moves_sprint", 0x271d_96a1_8b74_f364);
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
    let mut pin = play(&mut ps, &keys[..1], Rules::default(), RIFLE);
    assert!(ps.pm_flags & pmf::SIGHT_WALK != 0);
    pin.xor(play(&mut ps, &keys[1..2], Rules::default(), RIFLE), 1);
    assert!(ps.leanf != 0.0);
    ps.pm_flags &= !pmf::SIGHT;
    pin.xor(play(&mut ps, &keys[2..], Rules::default(), RIFLE), 2);
    pin.check("moves_sighted_lean", 0xf5cf_440d_5a00_34de);
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
    assert_ne!(pin.h, unclamped.h);
    pin.check("moves_inertia", 0x5a3f_4e7d_cfd5_ca63);
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
    pin.check("moves_spectator", 0xaecb_2678_7ea8_5b36);
}

/// The ladder's bob, run straight through `PM_Footsteps` with a climber
/// off the ground: a rung footstep each quarter of the cycle.
#[test]
fn a_climber_bobs_off_his_vertical_speed() {
    let mut d = Pin::words();
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
            let mut pm = Pmove::new(&mut ps, c, c, &ASPHALT, Rules::default());
            let pml = Pml {
                msec: 33,
                ..Default::default()
            };
            super::super::footsteps::footsteps(&mut pm, &pml);
            state(&mut d, &ps);
        }
        assert!(ps.events.contains(&ev::FOOTSTEP_RUN));
    }
    d.check("moves_ladder_bob", 0xea0f_ad92_aa9f_c8cf);
}

/// `PM_SetMovementDir`'s prone and ladder arms, which no walk reaches here.
#[test]
fn the_movement_dir_follows_the_prone_heading_and_the_ladder() {
    let mut d = Pin::words();
    for yaw in [-170.0f32, -95.0, -30.0, 0.0, 45.0, 89.5, 135.0, 179.0] {
        for arm in [pmf::PRONE, pmf::LADDER] {
            let mut ps = armed();
            ps.pm_flags |= arm;
            ps.view_angles[1] = yaw;
            ps.prone_direction = 20.0;
            ps.ladder_vec = Vec3::new(0.6, -0.8, 0.0);
            let c = Cmd::default();
            let mut pm = Pmove::new(&mut ps, c, c, &ASPHALT, Rules::default());
            super::update_movement_dir(&mut pm, &Pml::default());
            d.step();
            d.i32("movement_dir", ps.movement_dir);
        }
    }
    d.check("moves_movement_dir", 0x7242_e734_9db5_bc4a);
}
