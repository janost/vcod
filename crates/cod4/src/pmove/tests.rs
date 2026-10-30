//! The moves on an endless flat floor at z 0: what each arm of
//! `PmoveSingle` must do whatever the map. `cod4_moves_ab` holds them to
//! retail.

use glam::Vec3;

use super::*;

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
        t
    }
}

fn cmd(time: i32, buttons: u32, forward: i8) -> Cmd {
    Cmd {
        server_time: time,
        buttons,
        forward_move: forward,
        ..Default::default()
    }
}

/// `n` 50 ms cmds of `buttons` and `forward` from `ps`'s clock.
fn run(ps: &mut PlayerState, n: usize, buttons: u32, forward: i8, weapon: WeaponMove) {
    let mut old = cmd(ps.command_time, 0, 0);
    for _ in 0..n {
        let c = cmd(ps.command_time + 50, buttons, forward);
        player_step(ps, &c, &mut old, &Floor, Rules::default(), weapon);
    }
}

fn standing() -> PlayerState {
    let mut ps = PlayerState::spawn(Vec3::new(0.0, 0.0, 0.125), 0.0, 1000);
    ps.ground_entity_num = ENTITYNUM_WORLD;
    ps
}

const RIFLE: WeaponMove = WeaponMove {
    move_speed_scale: 0.95,
    ads_move_speed_scale: 0.5,
    sprint_duration_scale: 1.0,
};

#[test]
fn a_run_settles_at_the_speed_times_the_weapons_and_the_players_scales() {
    let mut ps = standing();
    ps.weapon = 1;
    ps.move_speed_scale_multiplier = 0.95;
    run(&mut ps, 40, 0, 127, RIFLE);
    // 190 * 0.95 * 0.95, snapped.
    assert_eq!(ps.velocity, Vec3::new(171.0, 0.0, 0.0));
    assert!(ps.on_ground());
}

/// A shellshock whose record slows the walk (the concussion's) runs at
/// 0.4 of the speed; one that does not (the flashbang's) at the full speed.
#[test]
fn a_slowing_shellshock_walks_at_four_tenths() {
    let rules = Rules {
        shock_slows: 1 << 2,
        ..Rules::default()
    };
    let speed = |index: i32| {
        let mut ps = standing();
        ps.pm_flags |= pmf::SHELLSHOCKED;
        ps.shellshock_index = index;
        let mut old = cmd(ps.command_time, 0, 0);
        for _ in 0..40 {
            let c = cmd(ps.command_time + 50, 0, 127);
            player_step(&mut ps, &c, &mut old, &Floor, rules, WeaponMove::NONE);
        }
        ps.velocity.x
    };
    // 190 and 190 * 0.4, snapped.
    assert_eq!(speed(1), 190.0);
    assert_eq!(speed(2), 76.0);
}

#[test]
fn a_jump_leaves_at_the_speed_that_reaches_jump_height() {
    let mut ps = standing();
    run(&mut ps, 1, button::JUMP, 0, WeaponMove::NONE);
    assert!(ps.pm_flags & pmf::JUMPING != 0);
    assert_eq!(ps.jump_time, ps.command_time);
    assert!(!ps.on_ground());
    // sqrt(2 g h) less the frame's gravity, snapped.
    let v0 = (2.0f32 * 800.0 * 39.0).sqrt();
    assert_eq!(ps.velocity.z, (v0 - 800.0 * 0.05).round());
    // Held, the key does not jump again on landing.
    run(&mut ps, 30, button::JUMP, 0, WeaponMove::NONE);
    assert!(ps.on_ground());
    assert_eq!(ps.velocity.z, 0.0);
}

#[test]
fn a_crouch_lowers_the_eye_to_40_over_200_ms() {
    let mut ps = standing();
    run(&mut ps, 2, button::CROUCH, 0, WeaponMove::NONE);
    assert_eq!(ps.view_height_target, VIEW_CROUCH);
    assert!(ps.view_height_current < 60.0 && ps.view_height_current > 40.0);
    run(&mut ps, 3, button::CROUCH, 0, WeaponMove::NONE);
    assert_eq!(ps.view_height_current, 40.0);
    assert_eq!(ps.pm_flags & (pmf::DUCKED | pmf::PRONE), pmf::DUCKED);
    assert_eq!(ps.e_flags & (ef::CROUCH | ef::PRONE), ef::CROUCH);
}

#[test]
fn a_spectator_flies_level_whatever_its_pitch() {
    let mut ps = PlayerState::spawn(Vec3::new(0.0, 0.0, 100.0), 0.0, 1000);
    ps.pm_type = pm_type::SPECTATOR;
    ps.speed = 400;
    // Pitched 45 down: 1.7's fly takes the forward off the right vector.
    ps.delta_angles[0] = 45.0;
    run(&mut ps, 10, 0, 127, WeaponMove::NONE);
    assert_eq!(ps.velocity.z, 0.0);
    assert!(ps.velocity.x > 300.0);
    assert_eq!(ps.origin.z, 100.0);
    // The lean-right key rises.
    run(&mut ps, 5, button::LEAN_RIGHT, 0, WeaponMove::NONE);
    assert!(ps.origin.z > 100.0);
}

#[test]
fn a_held_sprint_forces_the_stand_on_every_cmd() {
    // 1.7's `PM_CheckDuck` sprint arm (0x8058dc4) adds the event each cmd,
    // not on the sprint's edge (cod4-pmove.md 8).
    let mut ps = standing();
    ps.weapon = 1;
    run(&mut ps, 4, 0, 127, RIFLE);
    let mut old = cmd(ps.command_time, 0, 0);
    let mut seq = None;
    for _ in 0..40 {
        let c = cmd(ps.command_time + 50, button::SPRINT, 127);
        player_step(&mut ps, &c, &mut old, &Floor, Rules::default(), RIFLE);
        assert!(ps.pm_flags & pmf::SPRINTING != 0);
        let s = seq.get_or_insert(ps.event_sequence - 1);
        *s += 1;
        assert_eq!(ps.event_sequence, *s);
        assert_eq!(ps.events[(*s as usize - 1) & 3], ev::STANCE_FORCE_STAND);
    }
}

/// 1.7 refuses a sprint to a sighted body (`PM_UpdateSprint` 0x80571b0
/// tests 0x10 among the refusing flags) and leaves the sight alone.
#[test]
fn a_sighted_body_does_not_start_a_sprint() {
    let mut ps = standing();
    ps.pm_flags |= pmf::SIGHT;
    run(&mut ps, 4, button::SPRINT, 127, RIFLE);
    assert_eq!(ps.pm_flags & (pmf::SPRINTING | pmf::SIGHT), pmf::SIGHT);
    ps.pm_flags &= !pmf::SIGHT;
    run(&mut ps, 1, button::SPRINT, 127, RIFLE);
    assert_ne!(ps.pm_flags & pmf::SPRINTING, 0);
}

/// A 96-unit lunge sideways: `sqrt(2 * 96 * 1200)` = 480 along its yaw,
/// 1200 a second of friction, 400 ms; the forced forward key walks on.
#[test]
fn a_melee_charge_throws_the_body_along_its_yaw_until_friction_stops_it() {
    let mut ps = standing();
    ps.pm_flags |= pmf::MELEE_CHARGE;
    ps.melee_charge_yaw = 90.0;
    ps.melee_charge_dist = 96;
    let y0 = ps.origin.y;
    run(&mut ps, 1, 0, 0, WeaponMove::NONE);
    assert_eq!(ps.velocity.y, 420.0);
    assert_eq!(ps.melee_charge_time, 350);
    assert!(ps.velocity.x > 0.0, "the forced forward key");
    run(&mut ps, 6, 0, 0, WeaponMove::NONE);
    assert_ne!(ps.pm_flags & pmf::MELEE_CHARGE, 0);
    run(&mut ps, 1, 0, 0, WeaponMove::NONE);
    assert_eq!(ps.pm_flags & pmf::MELEE_CHARGE, 0);
    assert_eq!((ps.melee_charge_dist, ps.melee_charge_time), (0, 0));
    // About the distance the pick asked for.
    let dy = ps.origin.y - y0;
    assert!((dy - 96.0).abs() < 1.0, "{dy}");
}

/// The weapon state machine (`weapon::pm_weapon`) driven on its own, one
/// line a cmd: state, `weaponTime`, `weaponDelay`, shot count, anim,
/// `weapFlags`, clip, reserve, the prone override, the kick clock and the
/// events the cmd raised. These pin the cases no capture fires: a bolt
/// action's rechamber, a burst's cooldown and fresh pull, the perks'
/// steps, an empty clip, a prone shot and the held grenade.
mod weapon_pins {
    use super::*;
    use crate::pmove::weapon::{self, WeaponDef};

    const SLOT: usize = 1;

    fn def(fire_type: i32) -> WeaponDef {
        WeaponDef {
            name: "pin_mp".into(),
            fire_type,
            ammo_index: SLOT,
            clip_index: SLOT,
            clip_size: 5,
            max_ammo: 30,
            fire_time: 80,
            reload_time: 300,
            reload_empty_time: 400,
            reload_add_time: 200,
            raise_time: 100,
            drop_time: 100,
            sprint_in_time: 100,
            sprint_out_time: 100,
            oo_pos_anim_length: [1.0 / 200.0, 1.0 / 200.0],
            hip_spread_fire_add: 0.25,
            hip_gun_kick_reduced_kick_bullets: 2,
            ..WeaponDef::default()
        }
    }

    fn armed(clip: i32, reserve: i32) -> PlayerState {
        let mut ps = standing();
        ps.pm_flags &= !pmf::RESPAWNED;
        ps.weapon = 1;
        ps.weapons[0] |= 1 << 1;
        ps.ammoclip[SLOT] = clip;
        ps.ammo[SLOT] = reserve;
        ps
    }

    /// Runs `steps` of (msec, buttons, count) on `ps` and prints a line a
    /// cmd.
    fn trace(defs: &[WeaponDef], ps: &mut PlayerState, steps: &[(i32, u32, usize)]) -> String {
        let steps: Vec<_> = steps.iter().map(|&(m, b, n)| (m, b, None, n)).collect();
        trace_cmds(defs, ps, &steps, false)
    }

    /// [`trace`] with each step's cmd weapon byte (`None` echoes
    /// `ps.weapon`); `wide` adds the held weapon, the `weapons`,
    /// `weaponold` and `weaponrechamber` words and slot 2's clip and reserve.
    fn trace_cmds(
        defs: &[WeaponDef],
        ps: &mut PlayerState,
        steps: &[(i32, u32, Option<u8>, usize)],
        wide: bool,
    ) -> String {
        let defs = defs.to_vec();
        let mut out = String::new();
        let mut old = Cmd {
            server_time: ps.command_time,
            weapon: ps.weapon as u8,
            ..Default::default()
        };
        for &(msec, buttons, weapon, n) in steps {
            for _ in 0..n {
                let cmd = Cmd {
                    server_time: old.server_time + msec,
                    buttons,
                    weapon: weapon.unwrap_or(ps.weapon as u8),
                    ..Default::default()
                };
                let seq = ps.event_sequence;
                let mut pm = Pmove::new(ps, cmd, old, &Floor, Rules::default());
                pm.defs = Some(&defs);
                let pml = Pml {
                    msec,
                    frametime: msec as f32 * 0.001,
                    ..Pml::default()
                };
                weapon::pm_weapon(&mut pm, &pml);
                let evs: Vec<String> = (seq..ps.event_sequence)
                    .map(|k| {
                        format!(
                            "{}:{}",
                            ps.events[(k & 3) as usize],
                            ps.event_parms[(k & 3) as usize]
                        )
                    })
                    .collect();
                if wide {
                    out += &format!(
                        "w{} m{:x} o{:x} b{:x} C{} R{} ",
                        ps.weapon,
                        ps.weapons[0],
                        ps.weaponold[0],
                        ps.weaponrechamber[0],
                        ps.ammoclip[2],
                        ps.ammo[2]
                    );
                }
                out += &format!(
                    "s{} t{} d{} n{} a{:x} f{:x} c{} r{} p{} k{} g{} [{}]\n",
                    ps.weapon_state,
                    ps.weapon_time,
                    ps.weapon_delay,
                    ps.weapon_shot_count,
                    ps.weap_anim,
                    ps.weap_flags,
                    ps.ammoclip[SLOT],
                    ps.ammo[SLOT],
                    u8::from(ps.pm_flags & pmf::PRONE_STILL != 0),
                    ps.weapon_restrict_kick_time,
                    ps.grenade_time_left,
                    evs.join(" ")
                );
                old = cmd;
            }
        }
        out
    }

    fn defs_with(d: WeaponDef) -> Vec<WeaponDef> {
        vec![WeaponDef::default(), d]
    }

    const A: u32 = button::ATTACK;

    #[test]
    fn a_bolt_action_rechambers_after_its_one_shot() {
        let d = WeaponDef {
            bolt_action: true,
            rechamber_time: 400,
            rechamber_bolt_time: 150,
            fire_delay: 0,
            ..def(1)
        };
        let mut ps = armed(5, 10);
        let got = trace(
            &defs_with(d),
            &mut ps,
            &[(50, A, 4), (50, 0, 12), (50, A, 3)],
        );
        assert_eq!(got, BOLT, "{got}");
    }

    #[test]
    fn a_burst_cools_down_and_waits_for_a_fresh_pull() {
        let mut ps = armed(5, 10);
        let got = trace(
            &defs_with(def(3)),
            &mut ps,
            &[(30, A, 14), (30, 0, 3), (30, A, 10)],
        );
        assert_eq!(got, BURST, "{got}");
    }

    #[test]
    fn the_perks_step_the_fire_and_reload_clocks_faster() {
        let mut ps = armed(2, 3);
        ps.perks = 0x4 | 0x8;
        let got = trace(&defs_with(def(0)), &mut ps, &[(16, A, 30)]);
        assert_eq!(got, PERKS, "{got}");
    }

    #[test]
    fn an_empty_gun_clicks_and_waits_half_a_second() {
        let mut ps = armed(1, 0);
        let got = trace(&defs_with(def(0)), &mut ps, &[(50, A, 14)]);
        assert_eq!(got, EMPTY, "{got}");
    }

    #[test]
    fn a_prone_shot_overrides_the_prone_move_until_the_gun_idles() {
        let mut ps = armed(5, 10);
        ps.pm_flags |= pmf::PRONE;
        let mut got = trace(&defs_with(def(1)), &mut ps, &[(50, A, 3)]);
        ps.weapon_state = weapon::state::SPRINT_OUT;
        ps.weapon_time = 30;
        got += &trace(&defs_with(def(1)), &mut ps, &[(50, 0, 2)]);
        assert_eq!(got, PRONE, "{got}");
    }

    #[test]
    fn a_sighted_only_weapon_delays_its_shot_by_the_sight() {
        let d = WeaponDef {
            ads_fire_only: true,
            fire_delay: 40,
            ..def(0)
        };
        let mut ps = armed(5, 10);
        ps.weapon_pos_frac = 0.5;
        let got = trace(&defs_with(d), &mut ps, &[(50, A, 6)]);
        assert_eq!(got, SIGHTED, "{got}");
    }

    #[test]
    fn a_held_grenade_pulls_its_pin_and_goes_on_release() {
        let d = WeaponDef {
            weap_type: weapon::WEAPTYPE_GRENADE,
            hold_button_to_throw: true,
            fuse_time: 3000,
            hold_fire_time: 150,
            fire_time: 300,
            ..def(1)
        };
        let mut ps = armed(3, 0);
        let got = trace(&defs_with(d), &mut ps, &[(50, A, 6), (50, 0, 10)]);
        assert_eq!(got, GRENADE, "{got}");
    }

    #[test]
    fn a_body_in_the_air_never_fires_a_freezing_weapon() {
        let d = WeaponDef {
            freeze_movement_when_firing: true,
            ..def(0)
        };
        let mut ps = armed(5, 10);
        ps.ground_entity_num = ENTITYNUM_NONE;
        let mut got = trace(&defs_with(d.clone()), &mut ps, &[(50, A, 2)]);
        ps.ground_entity_num = ENTITYNUM_WORLD;
        got += &trace(&defs_with(d), &mut ps, &[(50, A, 2)]);
        assert_eq!(got, FREEZE, "{got}");
    }

    const R: u32 = weapon::RELOAD_BUTTON;

    /// (msec, buttons, count), as [`trace`] takes them.
    type Step = (i32, u32, usize);

    /// A shotgun-style def: a start, a round a reload, an end.
    fn segmented() -> WeaponDef {
        WeaponDef {
            segmented_reload: true,
            clip_size: 4,
            reload_start_time: 200,
            reload_start_add_time: 100,
            reload_start_add: 1,
            reload_time: 150,
            reload_empty_time: 250,
            reload_add_time: 100,
            reload_ammo_add: 1,
            reload_end_time: 120,
            ..def(1)
        }
    }

    /// A second weapon in slot 2, every change time its own.
    fn second() -> WeaponDef {
        WeaponDef {
            name: "pin2_mp".into(),
            ammo_index: 2,
            clip_index: 2,
            drop_time: 110,
            raise_time: 120,
            quick_drop_time: 60,
            quick_raise_time: 70,
            first_raise_time: 300,
            empty_raise_time: 130,
            empty_drop_time: 140,
            alt_drop_time: 40,
            alt_raise_time: 45,
            ..def(1)
        }
    }

    fn two_armed(clip: i32, reserve: i32, clip2: i32) -> PlayerState {
        let mut ps = armed(clip, reserve);
        ps.weapons[0] |= 1 << 2;
        ps.weaponold[0] |= 1 << 1;
        ps.ammoclip[2] = clip2;
        ps.ammo[2] = 7;
        ps
    }

    #[test]
    fn a_segmented_reload_adds_a_round_at_a_time_until_the_reserve_or_the_clip_runs_out() {
        let mut got = String::new();
        for (clip, reserve) in [(1, 2), (1, 10), (0, 10)] {
            let mut ps = armed(clip, reserve);
            got += &trace(&defs_with(segmented()), &mut ps, &[(50, R, 1), (50, 0, 22)]);
            got += "--\n";
        }
        let d = WeaponDef {
            reload_start_add: 0,
            reload_start_add_time: 0,
            ..segmented()
        };
        let mut ps = armed(2, 10);
        got += &trace(&defs_with(d), &mut ps, &[(50, R, 1), (50, 0, 16)]);
        assert_eq!(got, SEGMENTED, "{got}");
    }

    #[test]
    fn the_trigger_interrupts_a_segmented_reload_past_its_starts_first_two_fifths() {
        let mut got = String::new();
        let runs: [(i32, &[Step]); 4] = [
            (
                1,
                &[(50, R, 1), (50, A, 1), (50, 0, 1), (50, A, 1), (50, 0, 6)],
            ),
            (1, &[(50, R, 1), (50, 0, 6), (50, A, 1), (50, 0, 8)]),
            (1, &[(50, R, 1), (50, 0, 4), (50, A, 12)]),
            (0, &[(50, 0, 1), (50, 0, 2), (50, A, 1), (50, 0, 10)]),
        ];
        for (clip, steps) in runs {
            let mut ps = armed(clip, 10);
            got += &trace(&defs_with(segmented()), &mut ps, steps);
            got += "--\n";
        }
        // 40 ms cmds put the start's clock on exactly 0.4, which does not
        // interrupt; an empty start that adds nothing goes on interrupted.
        let mut ps = armed(1, 10);
        let steps = [
            (40, R, 1),
            (40, 0, 1),
            (40, A, 1),
            (40, 0, 1),
            (40, A, 1),
            (40, 0, 5),
        ];
        got += &trace(&defs_with(segmented()), &mut ps, &steps);
        got += "--\n";
        let d = WeaponDef {
            reload_start_add: 0,
            ..segmented()
        };
        let mut ps = armed(0, 10);
        got += &trace(
            &defs_with(d),
            &mut ps,
            &[(50, 0, 3), (50, A, 1), (50, 0, 10)],
        );
        assert_eq!(got, INTERRUPT, "{got}");
    }

    #[test]
    fn an_empty_clip_reloads_on_its_own_outside_a_shot_and_the_sprint() {
        let mut got = String::new();
        let mut ps = armed(0, 10);
        got += &trace(&defs_with(def(0)), &mut ps, &[(50, 0, 3)]);
        got += "--\n";
        let mut ps = armed(0, 10);
        ps.pm_flags |= pmf::SPRINTING;
        ps.weapon_state = weapon::state::SPRINT_HOLD;
        got += &trace(&defs_with(def(0)), &mut ps, &[(50, 0, 2)]);
        got += "--\n";
        let mut ps = armed(0, 10);
        ps.weapon_state = weapon::state::FIRE;
        ps.weapon_time = 20;
        got += &trace(&defs_with(def(0)), &mut ps, &[(50, 0, 2)]);
        got += "--\n";
        // The script's reload flag on a full clip, then on a part-empty one.
        let mut ps = armed(5, 10);
        ps.weap_flags |= weapon::wf::RELOAD;
        got += &trace(&defs_with(def(0)), &mut ps, &[(50, 0, 1)]);
        ps.ammoclip[SLOT] = 3;
        ps.weap_flags |= weapon::wf::RELOAD;
        got += &trace(&defs_with(def(0)), &mut ps, &[(50, 0, 2)]);
        assert_eq!(got, AUTO, "{got}");
    }

    #[test]
    fn a_no_partial_reload_waits_for_room_for_its_whole_add() {
        let mut got = String::new();
        for (add, clip) in [(3, 3), (3, 2), (0, 1), (0, 0), (5, 1)] {
            let d = WeaponDef {
                no_partial_reload: true,
                reload_ammo_add: add,
                ..def(0)
            };
            let mut ps = armed(clip, 10);
            got += &trace(&defs_with(d), &mut ps, &[(50, R, 1), (50, 0, 1)]);
            got += "--\n";
        }
        // A reserve shorter than the room tops up what it has.
        let mut ps = armed(1, 2);
        got += &trace(&defs_with(def(0)), &mut ps, &[(50, R, 1), (50, 0, 8)]);
        assert_eq!(got, PARTIAL, "{got}");
    }

    #[test]
    fn a_bolt_action_reloaded_unchambered_ejects_its_brass_first() {
        let d = WeaponDef {
            bolt_action: true,
            rechamber_time: 400,
            rechamber_bolt_time: 150,
            reload_time: 600,
            reload_add_time: 400,
            ..def(1)
        };
        let mut got = String::new();
        let mut ps = armed(3, 10);
        got += &trace(
            &defs_with(d.clone()),
            &mut ps,
            &[(50, A, 3), (50, R, 1), (50, 0, 14)],
        );
        got += "--\n";
        let seg = WeaponDef {
            bolt_action: true,
            rechamber_time: 400,
            rechamber_bolt_time: 150,
            ..segmented()
        };
        let mut ps = armed(3, 10);
        got += &trace(
            &defs_with(seg.clone()),
            &mut ps,
            &[(50, A, 3), (50, R, 1), (50, 0, 14)],
        );
        got += "--\n";
        let quick_bolt = WeaponDef {
            rechamber_bolt_time: 300,
            ..seg
        };
        let mut ps = armed(3, 10);
        got += &trace(
            &defs_with(quick_bolt),
            &mut ps,
            &[(50, A, 3), (50, R, 1), (50, 0, 14)],
        );
        assert_eq!(got, BOLTRELOAD, "{got}");
    }

    #[test]
    fn a_switch_mid_reload_stops_its_sound_and_raises_the_other_weapon() {
        let defs = vec![WeaponDef::default(), def(1), second()];
        let mut got = String::new();
        let mut ps = two_armed(2, 10, 3);
        let steps = [
            (50, R, Some(1), 1),
            (50, 0, Some(1), 2),
            (50, 0, Some(2), 10),
            (50, 0, Some(1), 6),
            (50, 0, Some(2), 2),
            (50, 0, Some(1), 4),
        ];
        got += &trace_cmds(&defs, &mut ps, &steps, true);
        got += "--\n";
        // Empty, then an unowned and an out-of-range byte.
        let mut ps = two_armed(0, 0, 0);
        let steps = [
            (50, 0, Some(2), 8),
            (50, 0, Some(3), 2),
            (50, 0, Some(9), 2),
            (50, 0, Some(1), 8),
        ];
        got += &trace_cmds(&defs, &mut ps, &steps, true);
        assert_eq!(got, SWITCH, "{got}");
    }

    #[test]
    fn an_alt_switch_and_a_pistol_change_quick() {
        let mut got = String::new();
        let alt = WeaponDef {
            alt_weapon: 2,
            ..def(1)
        };
        let defs = vec![WeaponDef::default(), alt, second()];
        let mut ps = two_armed(2, 10, 3);
        let steps = [(50, 0, Some(2), 3), (50, 0, Some(1), 6)];
        got += &trace_cmds(&defs, &mut ps, &steps, true);
        got += "--\n";
        let pistol = WeaponDef {
            weap_class: weapon::WEAPCLASS_PISTOL,
            ..second()
        };
        let defs = vec![WeaponDef::default(), def(1), pistol];
        let mut ps = two_armed(2, 10, 3);
        got += &trace_cmds(&defs, &mut ps, &[(50, 0, Some(2), 6)], true);
        got += "--\n";
        // Sprinting, an alt switch is a plain one with no anim.
        let defs = vec![
            WeaponDef::default(),
            WeaponDef {
                alt_weapon: 2,
                ..def(1)
            },
            second(),
        ];
        let mut ps = two_armed(2, 10, 3);
        ps.pm_flags |= pmf::SPRINTING;
        ps.weapon_state = weapon::state::SPRINT_HOLD;
        got += &trace_cmds(&defs, &mut ps, &[(50, 0, Some(2), 7)], true);
        assert_eq!(got, ALT, "{got}");
    }

    #[test]
    fn an_empty_clip_only_weapon_is_taken_once_it_is_put_away() {
        let only = WeaponDef {
            clip_only: true,
            ..def(1)
        };
        let defs = vec![WeaponDef::default(), only, second()];
        let mut ps = two_armed(0, 0, 3);
        let got = trace_cmds(&defs, &mut ps, &[(50, 0, Some(2), 8)], true);
        assert_eq!(got, CLIPONLY, "{got}");
    }

    #[test]
    fn the_sprint_lowers_the_gun_and_its_end_raises_it() {
        let mut got = String::new();
        let mut ps = armed(5, 10);
        ps.pm_flags |= pmf::SPRINTING;
        got += &trace(&defs_with(def(0)), &mut ps, &[(50, 0, 4)]);
        ps.pm_flags &= !pmf::SPRINTING;
        got += &trace(&defs_with(def(0)), &mut ps, &[(50, 0, 4)]);
        got += "--\n";
        // Mid-raise the sprint waits; a reload holds it off too.
        let mut ps = armed(2, 10);
        got += &trace(&defs_with(def(0)), &mut ps, &[(50, R, 1)]);
        ps.pm_flags |= pmf::SPRINTING;
        got += &trace(&defs_with(def(0)), &mut ps, &[(50, 0, 10)]);
        assert_eq!(got, SPRINT, "{got}");
    }

    #[test]
    fn the_ladder_and_disabled_weapons_put_the_gun_away() {
        let defs = vec![WeaponDef::default(), def(1), second()];
        let mut got = String::new();
        let mut ps = two_armed(2, 10, 3);
        ps.pm_flags |= pmf::LADDER;
        got += &trace_cmds(&defs, &mut ps, &[(50, 0, Some(1), 4)], true);
        ps.pm_flags &= !pmf::LADDER;
        got += &trace_cmds(&defs, &mut ps, &[(50, 0, Some(1), 4)], true);
        got += "--\n";
        let mut ps = two_armed(2, 10, 3);
        ps.weap_flags |= weapon::wf::DISABLED;
        got += &trace_cmds(&defs, &mut ps, &[(50, 0, Some(1), 4)], true);
        ps.weap_flags &= !weapon::wf::DISABLED;
        got += &trace_cmds(&defs, &mut ps, &[(50, 0, Some(2), 8)], true);
        got += "--\n";
        // A weapon taken from the hand goes away on its own.
        let mut ps = two_armed(2, 10, 3);
        ps.weapons[0] &= !(1 << 1);
        got += &trace_cmds(&defs, &mut ps, &[(50, 0, Some(1), 4)], true);
        got += "--\n";
        // Frozen, the cmd's change waits.
        let mut ps = two_armed(2, 10, 3);
        ps.pm_flags |= pmf::FROZEN;
        got += &trace_cmds(&defs, &mut ps, &[(50, 0, Some(2), 2)], true);
        assert_eq!(got, LADDER, "{got}");
    }

    const BOLT: &str = concat!(
        "s5 t80 d0 n1 a202 f0 c4 r10 p0 k160 g0 [26:0]\n",
        "s5 t30 d0 n1 a202 f0 c4 r10 p0 k110 g0 []\n",
        "s6 t400 d150 n1 a204 f0 c4 r10 p0 k60 g0 [28:0]\n",
        "s6 t350 d100 n1 a204 f0 c4 r10 p0 k10 g0 []\n",
        "s6 t300 d50 n1 a204 f0 c4 r10 p0 k0 g0 []\n",
        "s6 t250 d0 n1 a204 f0 c4 r10 p0 k0 g0 [29:0]\n",
        "s6 t200 d0 n1 a204 f0 c4 r10 p0 k0 g0 []\n",
        "s6 t150 d0 n1 a204 f0 c4 r10 p0 k0 g0 []\n",
        "s6 t100 d0 n1 a204 f0 c4 r10 p0 k0 g0 []\n",
        "s6 t50 d0 n1 a204 f0 c4 r10 p0 k0 g0 []\n",
        "s0 t0 d0 n0 a204 f0 c4 r10 p0 k0 g0 []\n",
        "s0 t0 d0 n0 a204 f0 c4 r10 p0 k0 g0 []\n",
        "s0 t0 d0 n0 a204 f0 c4 r10 p0 k0 g0 []\n",
        "s0 t0 d0 n0 a204 f0 c4 r10 p0 k0 g0 []\n",
        "s0 t0 d0 n0 a204 f0 c4 r10 p0 k0 g0 []\n",
        "s0 t0 d0 n0 a204 f0 c4 r10 p0 k0 g0 []\n",
        "s5 t80 d0 n1 a2 f0 c3 r10 p0 k160 g0 [26:0]\n",
        "s5 t30 d0 n1 a2 f0 c3 r10 p0 k110 g0 []\n",
        "s6 t400 d150 n1 a4 f0 c3 r10 p0 k60 g0 [28:0]\n",
    );
    const BURST: &str = concat!(
        "s5 t80 d0 n1 a202 f0 c4 r10 p0 k160 g0 [26:0]\n",
        "s5 t50 d0 n1 a202 f0 c4 r10 p0 k130 g0 []\n",
        "s5 t20 d0 n1 a202 f0 c4 r10 p0 k100 g0 []\n",
        "s5 t80 d0 n2 a2 f0 c3 r10 p0 k70 g0 [26:0]\n",
        "s5 t50 d0 n2 a2 f0 c3 r10 p0 k40 g0 []\n",
        "s5 t20 d0 n2 a2 f0 c3 r10 p0 k10 g0 []\n",
        "s5 t80 d0 n3 a202 f0 c2 r10 p0 k0 g0 [26:0]\n",
        "s5 t50 d0 n3 a202 f0 c2 r10 p0 k0 g0 []\n",
        "s5 t20 d0 n3 a202 f0 c2 r10 p0 k0 g0 []\n",
        "s0 t200 d0 n3 a0 f0 c2 r10 p0 k0 g0 []\n",
        "s0 t170 d0 n3 a0 f0 c2 r10 p0 k0 g0 []\n",
        "s0 t140 d0 n3 a0 f0 c2 r10 p0 k0 g0 []\n",
        "s0 t110 d0 n3 a0 f0 c2 r10 p0 k0 g0 []\n",
        "s0 t80 d0 n3 a0 f0 c2 r10 p0 k0 g0 []\n",
        "s0 t50 d0 n3 a0 f0 c2 r10 p0 k0 g0 []\n",
        "s0 t20 d0 n3 a0 f0 c2 r10 p0 k0 g0 []\n",
        "s0 t0 d0 n0 a0 f0 c2 r10 p0 k0 g0 []\n",
        "s5 t80 d0 n1 a202 f0 c1 r10 p0 k160 g0 [26:0]\n",
        "s5 t50 d0 n1 a202 f0 c1 r10 p0 k130 g0 []\n",
        "s5 t20 d0 n1 a202 f0 c1 r10 p0 k100 g0 []\n",
        "s5 t80 d0 n2 a3 f0 c0 r10 p0 k70 g0 [27:0]\n",
        "s5 t50 d0 n2 a3 f0 c0 r10 p0 k40 g0 []\n",
        "s5 t20 d0 n2 a3 f0 c0 r10 p0 k10 g0 []\n",
        "s7 t400 d200 n0 a20e f0 c0 r10 p0 k0 g0 [14:0 19:0 16:0]\n",
        "s7 t370 d170 n0 a20e f0 c0 r10 p0 k0 g0 []\n",
        "s7 t340 d140 n0 a20e f0 c0 r10 p0 k0 g0 []\n",
        "s7 t310 d110 n0 a20e f0 c0 r10 p0 k0 g0 []\n",
    );
    const PERKS: &str = concat!(
        "s5 t80 d0 n0 a202 f0 c1 r3 p0 k160 g0 [26:0]\n",
        "s5 t59 d0 n0 a202 f0 c1 r3 p0 k144 g0 []\n",
        "s5 t38 d0 n0 a202 f0 c1 r3 p0 k128 g0 []\n",
        "s5 t17 d0 n0 a202 f0 c1 r3 p0 k112 g0 []\n",
        "s5 t80 d0 n0 a3 f0 c0 r3 p0 k96 g0 [27:0]\n",
        "s5 t59 d0 n0 a3 f0 c0 r3 p0 k80 g0 []\n",
        "s5 t38 d0 n0 a3 f0 c0 r3 p0 k64 g0 []\n",
        "s5 t17 d0 n0 a3 f0 c0 r3 p0 k48 g0 []\n",
        "s7 t400 d200 n0 a20e f0 c0 r3 p0 k32 g0 [14:0 19:0 16:0]\n",
        "s7 t368 d168 n0 a20e f0 c0 r3 p0 k16 g0 []\n",
        "s7 t336 d136 n0 a20e f0 c0 r3 p0 k0 g0 []\n",
        "s7 t304 d104 n0 a20e f0 c0 r3 p0 k0 g0 []\n",
        "s7 t272 d72 n0 a20e f0 c0 r3 p0 k0 g0 []\n",
        "s7 t240 d40 n0 a20e f0 c0 r3 p0 k0 g0 []\n",
        "s7 t208 d8 n0 a20e f0 c0 r3 p0 k0 g0 []\n",
        "s7 t176 d0 n0 a20e f0 c3 r0 p0 k0 g0 [20:0]\n",
        "s7 t144 d0 n0 a20e f0 c3 r0 p0 k0 g0 []\n",
        "s7 t112 d0 n0 a20e f0 c3 r0 p0 k0 g0 []\n",
        "s7 t80 d0 n0 a20e f0 c3 r0 p0 k0 g0 []\n",
        "s7 t48 d0 n0 a20e f0 c3 r0 p0 k0 g0 []\n",
        "s7 t16 d0 n0 a20e f0 c3 r0 p0 k0 g0 []\n",
        "s0 t0 d0 n0 a0 f0 c3 r0 p0 k0 g0 []\n",
        "s5 t80 d0 n0 a202 f0 c2 r0 p0 k160 g0 [26:0]\n",
        "s5 t59 d0 n0 a202 f0 c2 r0 p0 k144 g0 []\n",
        "s5 t38 d0 n0 a202 f0 c2 r0 p0 k128 g0 []\n",
        "s5 t17 d0 n0 a202 f0 c2 r0 p0 k112 g0 []\n",
        "s5 t80 d0 n0 a2 f0 c1 r0 p0 k96 g0 [26:0]\n",
        "s5 t59 d0 n0 a2 f0 c1 r0 p0 k80 g0 []\n",
        "s5 t38 d0 n0 a2 f0 c1 r0 p0 k64 g0 []\n",
        "s5 t17 d0 n0 a2 f0 c1 r0 p0 k48 g0 []\n",
    );
    const EMPTY: &str = concat!(
        "s5 t80 d0 n0 a203 f0 c0 r0 p0 k160 g0 [27:0 11:0]\n",
        "s5 t30 d0 n0 a203 f0 c0 r0 p0 k110 g0 []\n",
        "s5 t500 d0 n0 a0 f0 c0 r0 p0 k60 g0 [11:0]\n",
        "s5 t450 d0 n0 a0 f0 c0 r0 p0 k10 g0 []\n",
        "s5 t400 d0 n0 a0 f0 c0 r0 p0 k0 g0 []\n",
        "s5 t350 d0 n0 a0 f0 c0 r0 p0 k0 g0 []\n",
        "s5 t300 d0 n0 a0 f0 c0 r0 p0 k0 g0 []\n",
        "s5 t250 d0 n0 a0 f0 c0 r0 p0 k0 g0 []\n",
        "s5 t200 d0 n0 a0 f0 c0 r0 p0 k0 g0 []\n",
        "s5 t150 d0 n0 a0 f0 c0 r0 p0 k0 g0 []\n",
        "s5 t100 d0 n0 a0 f0 c0 r0 p0 k0 g0 []\n",
        "s5 t50 d0 n0 a0 f0 c0 r0 p0 k0 g0 []\n",
        "s5 t500 d0 n0 a0 f0 c0 r0 p0 k0 g0 [11:0]\n",
        "s5 t450 d0 n0 a0 f0 c0 r0 p0 k0 g0 []\n",
    );
    const PRONE: &str = concat!(
        "s5 t80 d0 n1 a202 f0 c4 r10 p1 k160 g0 [26:0]\n",
        "s5 t30 d0 n1 a202 f0 c4 r10 p1 k110 g0 []\n",
        "s0 t1 d0 n1 a0 f0 c4 r10 p1 k60 g0 []\n",
        "s0 t0 d0 n0 a200 f0 c4 r10 p0 k10 g0 []\n",
        "s0 t0 d0 n0 a200 f0 c4 r10 p0 k0 g0 []\n",
    );
    const SIGHTED: &str = concat!(
        "s5 t80 d200 n0 a0 f0 c5 r10 p0 k200 g0 []\n",
        "s5 t30 d150 n0 a0 f0 c5 r10 p0 k150 g0 []\n",
        "s5 t0 d100 n0 a0 f0 c5 r10 p0 k100 g0 []\n",
        "s5 t0 d50 n0 a0 f0 c5 r10 p0 k50 g0 []\n",
        "s5 t80 d200 n0 a0 f0 c5 r10 p0 k0 g0 []\n",
        "s5 t30 d150 n0 a0 f0 c5 r10 p0 k0 g0 []\n",
    );
    const GRENADE: &str = concat!(
        "s5 t0 d150 n1 a21a f0 c3 r0 p0 k0 g3000 [25:1]\n",
        "s5 t0 d100 n1 a21a f0 c3 r0 p0 k0 g3000 []\n",
        "s5 t0 d50 n1 a21a f0 c3 r0 p0 k0 g3000 []\n",
        "s5 t300 d0 n2 a2 f0 c2 r0 p0 k0 g3000 [26:0]\n",
        "s5 t250 d0 n2 a2 f0 c2 r0 p0 k0 g3000 []\n",
        "s5 t200 d0 n2 a2 f0 c2 r0 p0 k0 g3000 []\n",
        "s0 t0 d0 n2 a1 f0 c2 r0 p0 k0 g3000 []\n",
        "s0 t0 d0 n2 a1 f0 c2 r0 p0 k0 g3000 []\n",
        "s0 t0 d0 n2 a1 f0 c2 r0 p0 k0 g3000 []\n",
        "s0 t0 d0 n2 a1 f0 c2 r0 p0 k0 g3000 []\n",
        "s0 t0 d0 n2 a1 f0 c2 r0 p0 k0 g3000 []\n",
        "s0 t0 d0 n2 a1 f0 c2 r0 p0 k0 g3000 []\n",
        "s0 t0 d0 n2 a1 f0 c2 r0 p0 k0 g3000 []\n",
        "s0 t0 d0 n2 a1 f0 c2 r0 p0 k0 g3000 []\n",
        "s0 t0 d0 n2 a1 f0 c2 r0 p0 k0 g3000 []\n",
        "s0 t0 d0 n2 a1 f0 c2 r0 p0 k0 g3000 []\n",
    );
    const FREEZE: &str = concat!(
        "s0 t0 d0 n0 a0 f0 c5 r10 p0 k0 g0 []\n",
        "s0 t0 d0 n0 a0 f0 c5 r10 p0 k0 g0 []\n",
        "s5 t80 d0 n0 a202 f0 c4 r10 p0 k160 g0 [26:0]\n",
        "s5 t30 d0 n0 a202 f0 c4 r10 p0 k110 g0 []\n",
    );
    const BOLTRELOAD: &str = concat!(
        "s5 t80 d0 n1 a202 f0 c2 r10 p0 k160 g0 [26:0]\n",
        "s5 t30 d0 n1 a202 f0 c2 r10 p0 k110 g0 []\n",
        "s6 t400 d150 n1 a204 f0 c2 r10 p0 k60 g0 [28:0]\n",
        "s7 t600 d150 n0 ad f0 c2 r10 p0 k10 g0 [14:0 19:0 15:0]\n",
        "s7 t550 d100 n0 ad f0 c2 r10 p0 k0 g0 []\n",
        "s7 t500 d50 n0 ad f0 c2 r10 p0 k0 g0 []\n",
        "s7 t450 d250 n0 ad f0 c2 r10 p0 k0 g0 [29:0]\n",
        "s7 t400 d200 n0 ad f0 c2 r10 p0 k0 g0 []\n",
        "s7 t350 d150 n0 ad f0 c2 r10 p0 k0 g0 []\n",
        "s7 t300 d100 n0 ad f0 c2 r10 p0 k0 g0 []\n",
        "s7 t250 d50 n0 ad f0 c2 r10 p0 k0 g0 []\n",
        "s7 t200 d0 n0 ad f0 c5 r7 p0 k0 g0 [20:0]\n",
        "s7 t150 d0 n0 ad f0 c5 r7 p0 k0 g0 []\n",
        "s7 t100 d0 n0 ad f0 c5 r7 p0 k0 g0 []\n",
        "s7 t50 d0 n0 ad f0 c5 r7 p0 k0 g0 []\n",
        "s0 t0 d0 n0 a200 f0 c5 r7 p0 k0 g0 []\n",
        "s0 t0 d0 n0 a200 f0 c5 r7 p0 k0 g0 []\n",
        "s0 t0 d0 n0 a200 f0 c5 r7 p0 k0 g0 []\n",
        "--\n",
        "s5 t80 d0 n1 a202 f0 c2 r10 p0 k160 g0 [26:0]\n",
        "s5 t30 d0 n1 a202 f0 c2 r10 p0 k110 g0 []\n",
        "s6 t400 d150 n1 a204 f0 c2 r10 p0 k60 g0 [28:0]\n",
        "s9 t200 d100 n0 af f0 c2 r10 p0 k10 g0 [14:0 19:0 17:0]\n",
        "s9 t150 d50 n0 af f0 c2 r10 p0 k0 g0 []\n",
        "s9 t100 d99 n0 af f0 c2 r10 p0 k0 g0 [29:0]\n",
        "s9 t50 d49 n0 af f0 c2 r10 p0 k0 g0 []\n",
        "s7 t150 d100 n0 a20d f0 c3 r9 p0 k0 g0 [20:0 15:0]\n",
        "s7 t100 d50 n0 a20d f0 c3 r9 p0 k0 g0 []\n",
        "s7 t50 d0 n0 a20d f0 c4 r8 p0 k0 g0 [20:0]\n",
        "s11 t120 d0 n0 a10 f0 c4 r8 p0 k0 g0 [18:0]\n",
        "s11 t70 d0 n0 a10 f0 c4 r8 p0 k0 g0 []\n",
        "s11 t20 d0 n0 a10 f0 c4 r8 p0 k0 g0 []\n",
        "s0 t0 d0 n0 a200 f0 c4 r8 p0 k0 g0 []\n",
        "s0 t0 d0 n0 a200 f0 c4 r8 p0 k0 g0 []\n",
        "s0 t0 d0 n0 a200 f0 c4 r8 p0 k0 g0 []\n",
        "s0 t0 d0 n0 a200 f0 c4 r8 p0 k0 g0 []\n",
        "s0 t0 d0 n0 a200 f0 c4 r8 p0 k0 g0 []\n",
        "--\n",
        "s5 t80 d0 n1 a202 f0 c2 r10 p0 k160 g0 [26:0]\n",
        "s5 t30 d0 n1 a202 f0 c2 r10 p0 k110 g0 []\n",
        "s6 t400 d300 n1 a204 f0 c2 r10 p0 k60 g0 [28:0]\n",
        "s9 t200 d100 n0 af f0 c2 r10 p0 k10 g0 [14:0 19:0 17:0]\n",
        "s9 t150 d50 n0 af f0 c2 r10 p0 k0 g0 []\n",
        "s9 t100 d99 n0 af f0 c2 r10 p0 k0 g0 [29:0]\n",
        "s9 t50 d49 n0 af f0 c2 r10 p0 k0 g0 []\n",
        "s7 t150 d100 n0 a20d f0 c3 r9 p0 k0 g0 [20:0 15:0]\n",
        "s7 t100 d50 n0 a20d f0 c3 r9 p0 k0 g0 []\n",
        "s7 t50 d0 n0 a20d f0 c4 r8 p0 k0 g0 [20:0]\n",
        "s11 t120 d0 n0 a10 f0 c4 r8 p0 k0 g0 [18:0]\n",
        "s11 t70 d0 n0 a10 f0 c4 r8 p0 k0 g0 []\n",
        "s11 t20 d0 n0 a10 f0 c4 r8 p0 k0 g0 []\n",
        "s0 t0 d0 n0 a200 f0 c4 r8 p0 k0 g0 []\n",
        "s0 t0 d0 n0 a200 f0 c4 r8 p0 k0 g0 []\n",
        "s0 t0 d0 n0 a200 f0 c4 r8 p0 k0 g0 []\n",
        "s0 t0 d0 n0 a200 f0 c4 r8 p0 k0 g0 []\n",
        "s0 t0 d0 n0 a200 f0 c4 r8 p0 k0 g0 []\n",
    );
    const PARTIAL: &str = concat!(
        "s0 t0 d0 n0 a0 f0 c3 r10 p0 k0 g0 []\n",
        "s0 t0 d0 n0 a0 f0 c3 r10 p0 k0 g0 []\n",
        "--\n",
        "s7 t300 d200 n0 a20d f0 c2 r10 p0 k0 g0 [14:0 19:0 15:0]\n",
        "s7 t250 d150 n0 a20d f0 c2 r10 p0 k0 g0 []\n",
        "--\n",
        "s0 t0 d0 n0 a0 f0 c1 r10 p0 k0 g0 []\n",
        "s0 t0 d0 n0 a0 f0 c1 r10 p0 k0 g0 []\n",
        "--\n",
        "s7 t400 d200 n0 a20e f0 c0 r10 p0 k0 g0 [14:0 19:0 16:0]\n",
        "s7 t350 d150 n0 a20e f0 c0 r10 p0 k0 g0 []\n",
        "--\n",
        "s0 t0 d0 n0 a0 f0 c1 r10 p0 k0 g0 []\n",
        "s0 t0 d0 n0 a0 f0 c1 r10 p0 k0 g0 []\n",
        "--\n",
        "s7 t300 d200 n0 a20d f0 c1 r2 p0 k0 g0 [14:0 19:0 15:0]\n",
        "s7 t250 d150 n0 a20d f0 c1 r2 p0 k0 g0 []\n",
        "s7 t200 d100 n0 a20d f0 c1 r2 p0 k0 g0 []\n",
        "s7 t150 d50 n0 a20d f0 c1 r2 p0 k0 g0 []\n",
        "s7 t100 d0 n0 a20d f0 c3 r0 p0 k0 g0 [20:0]\n",
        "s7 t50 d0 n0 a20d f0 c3 r0 p0 k0 g0 []\n",
        "s0 t0 d0 n0 a0 f0 c3 r0 p0 k0 g0 []\n",
        "s0 t0 d0 n0 a0 f0 c3 r0 p0 k0 g0 []\n",
        "s0 t0 d0 n0 a0 f0 c3 r0 p0 k0 g0 []\n",
    );
    const SEGMENTED: &str = concat!(
        "s9 t200 d100 n0 a20f f0 c1 r2 p0 k0 g0 [14:0 19:0 17:0]\n",
        "s9 t150 d50 n0 a20f f0 c1 r2 p0 k0 g0 []\n",
        "s9 t100 d0 n0 a20f f0 c2 r1 p0 k0 g0 [20:0]\n",
        "s9 t50 d0 n0 a20f f0 c2 r1 p0 k0 g0 []\n",
        "s7 t150 d100 n0 ad f0 c2 r1 p0 k0 g0 [15:0]\n",
        "s7 t100 d50 n0 ad f0 c2 r1 p0 k0 g0 []\n",
        "s7 t50 d0 n0 ad f0 c3 r0 p0 k0 g0 [20:0]\n",
        "s11 t120 d0 n0 a210 f0 c3 r0 p0 k0 g0 [18:0]\n",
        "s11 t70 d0 n0 a210 f0 c3 r0 p0 k0 g0 []\n",
        "s11 t20 d0 n0 a210 f0 c3 r0 p0 k0 g0 []\n",
        "s0 t0 d0 n0 a0 f0 c3 r0 p0 k0 g0 []\n",
        "s0 t0 d0 n0 a0 f0 c3 r0 p0 k0 g0 []\n",
        "s0 t0 d0 n0 a0 f0 c3 r0 p0 k0 g0 []\n",
        "s0 t0 d0 n0 a0 f0 c3 r0 p0 k0 g0 []\n",
        "s0 t0 d0 n0 a0 f0 c3 r0 p0 k0 g0 []\n",
        "s0 t0 d0 n0 a0 f0 c3 r0 p0 k0 g0 []\n",
        "s0 t0 d0 n0 a0 f0 c3 r0 p0 k0 g0 []\n",
        "s0 t0 d0 n0 a0 f0 c3 r0 p0 k0 g0 []\n",
        "s0 t0 d0 n0 a0 f0 c3 r0 p0 k0 g0 []\n",
        "s0 t0 d0 n0 a0 f0 c3 r0 p0 k0 g0 []\n",
        "s0 t0 d0 n0 a0 f0 c3 r0 p0 k0 g0 []\n",
        "s0 t0 d0 n0 a0 f0 c3 r0 p0 k0 g0 []\n",
        "s0 t0 d0 n0 a0 f0 c3 r0 p0 k0 g0 []\n",
        "--\n",
        "s9 t200 d100 n0 a20f f0 c1 r10 p0 k0 g0 [14:0 19:0 17:0]\n",
        "s9 t150 d50 n0 a20f f0 c1 r10 p0 k0 g0 []\n",
        "s9 t100 d0 n0 a20f f0 c2 r9 p0 k0 g0 [20:0]\n",
        "s9 t50 d0 n0 a20f f0 c2 r9 p0 k0 g0 []\n",
        "s7 t150 d100 n0 ad f0 c2 r9 p0 k0 g0 [15:0]\n",
        "s7 t100 d50 n0 ad f0 c2 r9 p0 k0 g0 []\n",
        "s7 t50 d0 n0 ad f0 c3 r8 p0 k0 g0 [20:0]\n",
        "s7 t150 d100 n0 a20d f0 c3 r8 p0 k0 g0 [15:0]\n",
        "s7 t100 d50 n0 a20d f0 c3 r8 p0 k0 g0 []\n",
        "s7 t50 d0 n0 a20d f0 c4 r7 p0 k0 g0 [20:0]\n",
        "s11 t120 d0 n0 a10 f0 c4 r7 p0 k0 g0 [18:0]\n",
        "s11 t70 d0 n0 a10 f0 c4 r7 p0 k0 g0 []\n",
        "s11 t20 d0 n0 a10 f0 c4 r7 p0 k0 g0 []\n",
        "s0 t0 d0 n0 a200 f0 c4 r7 p0 k0 g0 []\n",
        "s0 t0 d0 n0 a200 f0 c4 r7 p0 k0 g0 []\n",
        "s0 t0 d0 n0 a200 f0 c4 r7 p0 k0 g0 []\n",
        "s0 t0 d0 n0 a200 f0 c4 r7 p0 k0 g0 []\n",
        "s0 t0 d0 n0 a200 f0 c4 r7 p0 k0 g0 []\n",
        "s0 t0 d0 n0 a200 f0 c4 r7 p0 k0 g0 []\n",
        "s0 t0 d0 n0 a200 f0 c4 r7 p0 k0 g0 []\n",
        "s0 t0 d0 n0 a200 f0 c4 r7 p0 k0 g0 []\n",
        "s0 t0 d0 n0 a200 f0 c4 r7 p0 k0 g0 []\n",
        "s0 t0 d0 n0 a200 f0 c4 r7 p0 k0 g0 []\n",
        "--\n",
        "s9 t200 d100 n0 a20f f0 c0 r10 p0 k0 g0 [14:0 19:0 17:0]\n",
        "s9 t150 d50 n0 a20f f0 c0 r10 p0 k0 g0 []\n",
        "s9 t100 d0 n0 a20f f0 c1 r9 p0 k0 g0 [20:0]\n",
        "s9 t50 d0 n0 a20f f0 c1 r9 p0 k0 g0 []\n",
        "s7 t150 d100 n0 ad f0 c1 r9 p0 k0 g0 [15:0]\n",
        "s7 t100 d50 n0 ad f0 c1 r9 p0 k0 g0 []\n",
        "s7 t50 d0 n0 ad f0 c2 r8 p0 k0 g0 [20:0]\n",
        "s7 t150 d100 n0 a20d f0 c2 r8 p0 k0 g0 [15:0]\n",
        "s7 t100 d50 n0 a20d f0 c2 r8 p0 k0 g0 []\n",
        "s7 t50 d0 n0 a20d f0 c3 r7 p0 k0 g0 [20:0]\n",
        "s7 t150 d100 n0 ad f0 c3 r7 p0 k0 g0 [15:0]\n",
        "s7 t100 d50 n0 ad f0 c3 r7 p0 k0 g0 []\n",
        "s7 t50 d0 n0 ad f0 c4 r6 p0 k0 g0 [20:0]\n",
        "s11 t120 d0 n0 a210 f0 c4 r6 p0 k0 g0 [18:0]\n",
        "s11 t70 d0 n0 a210 f0 c4 r6 p0 k0 g0 []\n",
        "s11 t20 d0 n0 a210 f0 c4 r6 p0 k0 g0 []\n",
        "s0 t0 d0 n0 a0 f0 c4 r6 p0 k0 g0 []\n",
        "s0 t0 d0 n0 a0 f0 c4 r6 p0 k0 g0 []\n",
        "s0 t0 d0 n0 a0 f0 c4 r6 p0 k0 g0 []\n",
        "s0 t0 d0 n0 a0 f0 c4 r6 p0 k0 g0 []\n",
        "s0 t0 d0 n0 a0 f0 c4 r6 p0 k0 g0 []\n",
        "s0 t0 d0 n0 a0 f0 c4 r6 p0 k0 g0 []\n",
        "s0 t0 d0 n0 a0 f0 c4 r6 p0 k0 g0 []\n",
        "--\n",
        "s9 t200 d0 n0 a20f f0 c2 r10 p0 k0 g0 [14:0 19:0 17:0]\n",
        "s9 t150 d0 n0 a20f f0 c2 r10 p0 k0 g0 []\n",
        "s9 t100 d0 n0 a20f f0 c2 r10 p0 k0 g0 []\n",
        "s9 t50 d0 n0 a20f f0 c2 r10 p0 k0 g0 []\n",
        "s7 t150 d100 n0 ad f0 c2 r10 p0 k0 g0 [15:0]\n",
        "s7 t100 d50 n0 ad f0 c2 r10 p0 k0 g0 []\n",
        "s7 t50 d0 n0 ad f0 c3 r9 p0 k0 g0 [20:0]\n",
        "s7 t150 d100 n0 a20d f0 c3 r9 p0 k0 g0 [15:0]\n",
        "s7 t100 d50 n0 a20d f0 c3 r9 p0 k0 g0 []\n",
        "s7 t50 d0 n0 a20d f0 c4 r8 p0 k0 g0 [20:0]\n",
        "s11 t120 d0 n0 a10 f0 c4 r8 p0 k0 g0 [18:0]\n",
        "s11 t70 d0 n0 a10 f0 c4 r8 p0 k0 g0 []\n",
        "s11 t20 d0 n0 a10 f0 c4 r8 p0 k0 g0 []\n",
        "s0 t0 d0 n0 a200 f0 c4 r8 p0 k0 g0 []\n",
        "s0 t0 d0 n0 a200 f0 c4 r8 p0 k0 g0 []\n",
        "s0 t0 d0 n0 a200 f0 c4 r8 p0 k0 g0 []\n",
        "s0 t0 d0 n0 a200 f0 c4 r8 p0 k0 g0 []\n",
    );
    const SWITCH: &str = concat!(
        "w1 m6 o2 b0 C3 R7 s7 t300 d200 n0 a20d f0 c2 r10 p0 k0 g0 [14:0 19:0 15:0]\n",
        "w1 m6 o2 b0 C3 R7 s7 t250 d150 n0 a20d f0 c2 r10 p0 k0 g0 []\n",
        "w1 m6 o2 b0 C3 R7 s7 t200 d100 n0 a20d f0 c2 r10 p0 k0 g0 []\n",
        "w1 m6 o2 b0 C3 R7 s3 t100 d0 n0 aa f0 c2 r10 p0 k0 g0 [2:7 23:0]\n",
        "w1 m6 o2 b0 C3 R7 s3 t50 d0 n0 aa f0 c2 r10 p0 k0 g0 []\n",
        "w2 m6 o6 b0 C3 R7 s1 t300 d0 n0 a20c f0 c2 r10 p0 k0 g0 [22:0]\n",
        "w2 m6 o6 b0 C3 R7 s1 t250 d0 n0 a20c f0 c2 r10 p0 k0 g0 []\n",
        "w2 m6 o6 b0 C3 R7 s1 t200 d0 n0 a20c f0 c2 r10 p0 k0 g0 []\n",
        "w2 m6 o6 b0 C3 R7 s1 t150 d0 n0 a20c f0 c2 r10 p0 k0 g0 []\n",
        "w2 m6 o6 b0 C3 R7 s1 t100 d0 n0 a20c f0 c2 r10 p0 k0 g0 []\n",
        "w2 m6 o6 b0 C3 R7 s1 t50 d0 n0 a20c f0 c2 r10 p0 k0 g0 []\n",
        "w2 m6 o6 b0 C3 R7 s0 t0 d0 n0 a0 f0 c2 r10 p0 k0 g0 []\n",
        "w2 m6 o6 b0 C3 R7 s0 t0 d0 n0 a0 f0 c2 r10 p0 k0 g0 []\n",
        "w2 m6 o6 b0 C3 R7 s3 t110 d0 n0 a20a f0 c2 r10 p0 k0 g0 [23:0]\n",
        "w2 m6 o6 b0 C3 R7 s3 t60 d0 n0 a20a f0 c2 r10 p0 k0 g0 []\n",
        "w2 m6 o6 b0 C3 R7 s3 t10 d0 n0 a20a f0 c2 r10 p0 k0 g0 []\n",
        "w1 m6 o6 b0 C3 R7 s1 t100 d0 n0 ab f0 c2 r10 p0 k0 g0 [21:0]\n",
        "w1 m6 o6 b0 C3 R7 s1 t50 d0 n0 ab f0 c2 r10 p0 k0 g0 []\n",
        "w1 m6 o6 b0 C3 R7 s0 t0 d0 n0 a200 f0 c2 r10 p0 k0 g0 []\n",
        "w1 m6 o6 b0 C3 R7 s3 t100 d0 n0 aa f0 c2 r10 p0 k0 g0 [23:0]\n",
        "w1 m6 o6 b0 C3 R7 s3 t50 d0 n0 aa f0 c2 r10 p0 k0 g0 []\n",
        "w1 m6 o6 b0 C3 R7 s0 t0 d0 n0 a1 f0 c2 r10 p0 k0 g0 []\n",
        "w1 m6 o6 b0 C3 R7 s0 t0 d0 n0 a1 f0 c2 r10 p0 k0 g0 []\n",
        "w1 m6 o6 b0 C3 R7 s0 t0 d0 n0 a1 f0 c2 r10 p0 k0 g0 []\n",
        "w1 m6 o6 b0 C3 R7 s0 t0 d0 n0 a1 f0 c2 r10 p0 k0 g0 []\n",
        "--\n",
        "w2 m6 o6 b0 C0 R7 s1 t130 d0 n0 a16 f0 c0 r0 p0 k0 g0 [23:0 22:0]\n",
        "w2 m6 o6 b0 C0 R7 s1 t80 d0 n0 a16 f0 c0 r0 p0 k0 g0 []\n",
        "w2 m6 o6 b0 C0 R7 s1 t30 d0 n0 a16 f0 c0 r0 p0 k0 g0 []\n",
        "w2 m6 o6 b0 C0 R7 s0 t0 d0 n0 a200 f0 c0 r0 p0 k0 g0 []\n",
        "w2 m6 o6 b0 C0 R7 s7 t400 d200 n0 ae f0 c0 r0 p0 k0 g0 [14:0 19:0 16:0]\n",
        "w2 m6 o6 b0 C0 R7 s7 t350 d150 n0 ae f0 c0 r0 p0 k0 g0 []\n",
        "w2 m6 o6 b0 C0 R7 s7 t300 d100 n0 ae f0 c0 r0 p0 k0 g0 []\n",
        "w2 m6 o6 b0 C0 R7 s7 t250 d50 n0 ae f0 c0 r0 p0 k0 g0 []\n",
        "w2 m6 o6 b0 C5 R2 s7 t200 d0 n0 ae f0 c0 r0 p0 k0 g0 [20:0]\n",
        "w2 m6 o6 b0 C5 R2 s7 t150 d0 n0 ae f0 c0 r0 p0 k0 g0 []\n",
        "w2 m6 o6 b0 C5 R2 s7 t100 d0 n0 ae f0 c0 r0 p0 k0 g0 []\n",
        "w2 m6 o6 b0 C5 R2 s7 t50 d0 n0 ae f0 c0 r0 p0 k0 g0 []\n",
        "w2 m6 o6 b0 C5 R2 s3 t110 d0 n0 a20a f0 c0 r0 p0 k0 g0 [2:7 23:0]\n",
        "w2 m6 o6 b0 C5 R2 s3 t60 d0 n0 a20a f0 c0 r0 p0 k0 g0 []\n",
        "w2 m6 o6 b0 C5 R2 s3 t10 d0 n0 a20a f0 c0 r0 p0 k0 g0 []\n",
        "w1 m6 o6 b0 C5 R2 s1 t0 d0 n0 a16 f0 c0 r0 p0 k0 g0 [21:0]\n",
        "w1 m6 o6 b0 C5 R2 s0 t0 d0 n0 a200 f0 c0 r0 p0 k0 g0 []\n",
        "w1 m6 o6 b0 C5 R2 s0 t0 d0 n0 a200 f0 c0 r0 p0 k0 g0 []\n",
        "w1 m6 o6 b0 C5 R2 s0 t0 d0 n0 a200 f0 c0 r0 p0 k0 g0 []\n",
        "w1 m6 o6 b0 C5 R2 s0 t0 d0 n0 a200 f0 c0 r0 p0 k0 g0 []\n",
    );
    const ALT: &str = concat!(
        "w2 m6 o6 b0 C3 R7 s2 t45 d0 n0 a12 f0 c2 r10 p0 k0 g0 [24:0]\n",
        "w2 m6 o6 b0 C3 R7 s0 t0 d0 n0 a200 f0 c2 r10 p0 k0 g0 []\n",
        "w2 m6 o6 b0 C3 R7 s0 t0 d0 n0 a200 f0 c2 r10 p0 k0 g0 []\n",
        "w2 m6 o6 b0 C3 R7 s3 t110 d0 n0 aa f0 c2 r10 p0 k0 g0 [23:0]\n",
        "w2 m6 o6 b0 C3 R7 s3 t60 d0 n0 aa f0 c2 r10 p0 k0 g0 []\n",
        "w2 m6 o6 b0 C3 R7 s3 t10 d0 n0 aa f0 c2 r10 p0 k0 g0 []\n",
        "w1 m6 o6 b0 C3 R7 s1 t100 d0 n0 a20b f0 c2 r10 p0 k0 g0 [21:0]\n",
        "w1 m6 o6 b0 C3 R7 s1 t50 d0 n0 a20b f0 c2 r10 p0 k0 g0 []\n",
        "w1 m6 o6 b0 C3 R7 s0 t0 d0 n0 a0 f0 c2 r10 p0 k0 g0 []\n",
        "--\n",
        "w2 m6 o6 b0 C3 R7 s1 t300 d0 n0 ac f0 c2 r10 p0 k0 g0 [23:0 22:0]\n",
        "w2 m6 o6 b0 C3 R7 s1 t250 d0 n0 ac f0 c2 r10 p0 k0 g0 []\n",
        "w2 m6 o6 b0 C3 R7 s1 t200 d0 n0 ac f0 c2 r10 p0 k0 g0 []\n",
        "w2 m6 o6 b0 C3 R7 s1 t150 d0 n0 ac f0 c2 r10 p0 k0 g0 []\n",
        "w2 m6 o6 b0 C3 R7 s1 t100 d0 n0 ac f0 c2 r10 p0 k0 g0 []\n",
        "w2 m6 o6 b0 C3 R7 s1 t50 d0 n0 ac f0 c2 r10 p0 k0 g0 []\n",
        "--\n",
        "w1 m6 o2 b0 C3 R7 s3 t100 d0 n0 a0 f0 c2 r10 p0 k0 g0 [23:0]\n",
        "w1 m6 o2 b0 C3 R7 s3 t50 d0 n0 a0 f0 c2 r10 p0 k0 g0 []\n",
        "w2 m6 o6 b0 C3 R7 s1 t300 d0 n0 a20c f0 c2 r10 p0 k0 g0 [22:0]\n",
        "w2 m6 o6 b0 C3 R7 s1 t250 d0 n0 a20c f0 c2 r10 p0 k0 g0 []\n",
        "w2 m6 o6 b0 C3 R7 s1 t200 d0 n0 a20c f0 c2 r10 p0 k0 g0 []\n",
        "w2 m6 o6 b0 C3 R7 s1 t150 d0 n0 a20c f0 c2 r10 p0 k0 g0 []\n",
        "w2 m6 o6 b0 C3 R7 s1 t100 d0 n0 a20c f0 c2 r10 p0 k0 g0 []\n",
    );
    const CLIPONLY: &str = concat!(
        "w2 m4 o6 b0 C3 R7 s1 t300 d0 n0 ac f0 c0 r0 p0 k0 g0 [23:0 22:0]\n",
        "w2 m4 o6 b0 C3 R7 s1 t250 d0 n0 ac f0 c0 r0 p0 k0 g0 []\n",
        "w2 m4 o6 b0 C3 R7 s1 t200 d0 n0 ac f0 c0 r0 p0 k0 g0 []\n",
        "w2 m4 o6 b0 C3 R7 s1 t150 d0 n0 ac f0 c0 r0 p0 k0 g0 []\n",
        "w2 m4 o6 b0 C3 R7 s1 t100 d0 n0 ac f0 c0 r0 p0 k0 g0 []\n",
        "w2 m4 o6 b0 C3 R7 s1 t50 d0 n0 ac f0 c0 r0 p0 k0 g0 []\n",
        "w2 m4 o6 b0 C3 R7 s0 t0 d0 n0 a200 f0 c0 r0 p0 k0 g0 []\n",
        "w2 m4 o6 b0 C3 R7 s0 t0 d0 n0 a200 f0 c0 r0 p0 k0 g0 []\n",
    );
    const AUTO: &str = concat!(
        "s7 t400 d200 n0 a20e f0 c0 r10 p0 k0 g0 [14:0 19:0 16:0]\n",
        "s7 t350 d150 n0 a20e f0 c0 r10 p0 k0 g0 []\n",
        "s7 t300 d100 n0 a20e f0 c0 r10 p0 k0 g0 []\n",
        "--\n",
        "s23 t0 d0 n0 a0 f0 c0 r10 p0 k0 g0 []\n",
        "s23 t0 d0 n0 a0 f0 c0 r10 p0 k0 g0 []\n",
        "--\n",
        "s0 t0 d0 n0 a0 f0 c0 r10 p0 k0 g0 []\n",
        "s7 t400 d200 n0 a20e f0 c0 r10 p0 k0 g0 [14:0 19:0 16:0]\n",
        "--\n",
        "s0 t0 d0 n0 a0 f0 c5 r10 p0 k0 g0 []\n",
        "s7 t300 d200 n0 a20d f0 c3 r10 p0 k0 g0 [14:0 19:0 15:0]\n",
        "s7 t250 d150 n0 a20d f0 c3 r10 p0 k0 g0 []\n",
    );
    const LADDER: &str = concat!(
        "w0 m6 o3 b0 C3 R7 s1 t0 d0 n0 a16 f0 c2 r10 p0 k0 g0 [23:0 22:0]\n",
        "w0 m6 o3 b0 C3 R7 s0 t0 d0 n0 a200 f0 c2 r10 p0 k0 g0 []\n",
        "w0 m6 o3 b0 C3 R7 s0 t0 d0 n0 a200 f0 c2 r10 p0 k0 g0 []\n",
        "w0 m6 o3 b0 C3 R7 s0 t0 d0 n0 a200 f0 c2 r10 p0 k0 g0 []\n",
        "w1 m6 o3 b0 C3 R7 s1 t100 d0 n0 ab f0 c2 r10 p0 k0 g0 []\n",
        "w1 m6 o3 b0 C3 R7 s1 t50 d0 n0 ab f0 c2 r10 p0 k0 g0 []\n",
        "w1 m6 o3 b0 C3 R7 s0 t0 d0 n0 a200 f0 c2 r10 p0 k0 g0 []\n",
        "w1 m6 o3 b0 C3 R7 s0 t0 d0 n0 a200 f0 c2 r10 p0 k0 g0 []\n",
        "--\n",
        "w1 m6 o2 b0 C3 R7 s3 t100 d0 n0 a20a f80 c2 r10 p0 k0 g0 [23:0]\n",
        "w1 m6 o2 b0 C3 R7 s3 t50 d0 n0 a20a f80 c2 r10 p0 k0 g0 []\n",
        "w0 m6 o3 b0 C3 R7 s1 t0 d0 n0 a16 f80 c2 r10 p0 k0 g0 [22:0]\n",
        "w0 m6 o3 b0 C3 R7 s0 t0 d0 n0 a200 f80 c2 r10 p0 k0 g0 []\n",
        "w2 m6 o7 b0 C3 R7 s1 t300 d0 n0 ac f0 c2 r10 p0 k0 g0 []\n",
        "w2 m6 o7 b0 C3 R7 s1 t250 d0 n0 ac f0 c2 r10 p0 k0 g0 []\n",
        "w2 m6 o7 b0 C3 R7 s1 t200 d0 n0 ac f0 c2 r10 p0 k0 g0 []\n",
        "w2 m6 o7 b0 C3 R7 s1 t150 d0 n0 ac f0 c2 r10 p0 k0 g0 []\n",
        "w2 m6 o7 b0 C3 R7 s1 t100 d0 n0 ac f0 c2 r10 p0 k0 g0 []\n",
        "w2 m6 o7 b0 C3 R7 s1 t50 d0 n0 ac f0 c2 r10 p0 k0 g0 []\n",
        "w2 m6 o7 b0 C3 R7 s0 t0 d0 n0 a200 f0 c2 r10 p0 k0 g0 []\n",
        "w2 m6 o7 b0 C3 R7 s0 t0 d0 n0 a200 f0 c2 r10 p0 k0 g0 []\n",
        "--\n",
        "w0 m4 o3 b0 C3 R7 s1 t0 d0 n0 a216 f0 c2 r10 p0 k0 g0 [22:0]\n",
        "w0 m4 o3 b0 C3 R7 s0 t0 d0 n0 a0 f0 c2 r10 p0 k0 g0 []\n",
        "w0 m4 o3 b0 C3 R7 s0 t0 d0 n0 a0 f0 c2 r10 p0 k0 g0 []\n",
        "w0 m4 o3 b0 C3 R7 s0 t0 d0 n0 a0 f0 c2 r10 p0 k0 g0 []\n",
        "--\n",
        "w1 m6 o2 b0 C3 R7 s0 t0 d0 n0 a0 f0 c2 r10 p0 k0 g0 []\n",
        "w1 m6 o2 b0 C3 R7 s0 t0 d0 n0 a0 f0 c2 r10 p0 k0 g0 []\n",
    );
    const SPRINT: &str = concat!(
        "s22 t100 d0 n0 a217 f0 c5 r10 p0 k0 g0 []\n",
        "s22 t50 d0 n0 a217 f0 c5 r10 p0 k0 g0 []\n",
        "s23 t0 d0 n0 a18 f0 c5 r10 p0 k0 g0 []\n",
        "s23 t0 d0 n0 a18 f0 c5 r10 p0 k0 g0 []\n",
        "s24 t100 d0 n0 a219 f0 c5 r10 p0 k0 g0 []\n",
        "s24 t50 d0 n0 a219 f0 c5 r10 p0 k0 g0 []\n",
        "s0 t0 d0 n0 a0 f0 c5 r10 p0 k0 g0 []\n",
        "s0 t0 d0 n0 a0 f0 c5 r10 p0 k0 g0 []\n",
        "--\n",
        "s7 t300 d200 n0 a20d f0 c2 r10 p0 k0 g0 [14:0 19:0 15:0]\n",
        "s22 t100 d0 n0 a17 f0 c2 r10 p0 k0 g0 []\n",
        "s22 t50 d0 n0 a17 f0 c2 r10 p0 k0 g0 []\n",
        "s23 t0 d0 n0 a218 f0 c2 r10 p0 k0 g0 []\n",
        "s23 t0 d0 n0 a218 f0 c2 r10 p0 k0 g0 []\n",
        "s23 t0 d0 n0 a218 f0 c2 r10 p0 k0 g0 []\n",
        "s23 t0 d0 n0 a218 f0 c2 r10 p0 k0 g0 []\n",
        "s23 t0 d0 n0 a218 f0 c2 r10 p0 k0 g0 []\n",
        "s23 t0 d0 n0 a218 f0 c2 r10 p0 k0 g0 []\n",
        "s23 t0 d0 n0 a218 f0 c2 r10 p0 k0 g0 []\n",
        "s23 t0 d0 n0 a218 f0 c2 r10 p0 k0 g0 []\n",
    );
    const INTERRUPT: &str = concat!(
        "s9 t200 d100 n0 a20f f0 c1 r10 p0 k0 g0 [14:0 19:0 17:0]\n",
        "s9 t150 d50 n0 a20f f0 c1 r10 p0 k0 g0 []\n",
        "s9 t100 d0 n0 a20f f0 c2 r9 p0 k0 g0 [20:0]\n",
        "s10 t50 d0 n0 a20f f0 c2 r9 p0 k0 g0 []\n",
        "s11 t120 d0 n0 a10 f0 c2 r9 p0 k0 g0 [18:0]\n",
        "s11 t70 d0 n0 a10 f0 c2 r9 p0 k0 g0 []\n",
        "s11 t20 d0 n0 a10 f0 c2 r9 p0 k0 g0 []\n",
        "s0 t0 d0 n0 a200 f0 c2 r9 p0 k0 g0 []\n",
        "s0 t0 d0 n0 a200 f0 c2 r9 p0 k0 g0 []\n",
        "s0 t0 d0 n0 a200 f0 c2 r9 p0 k0 g0 []\n",
        "--\n",
        "s9 t200 d100 n0 a20f f0 c1 r10 p0 k0 g0 [14:0 19:0 17:0]\n",
        "s9 t150 d50 n0 a20f f0 c1 r10 p0 k0 g0 []\n",
        "s9 t100 d0 n0 a20f f0 c2 r9 p0 k0 g0 [20:0]\n",
        "s9 t50 d0 n0 a20f f0 c2 r9 p0 k0 g0 []\n",
        "s7 t150 d100 n0 ad f0 c2 r9 p0 k0 g0 [15:0]\n",
        "s7 t100 d50 n0 ad f0 c2 r9 p0 k0 g0 []\n",
        "s7 t50 d0 n0 ad f0 c3 r8 p0 k0 g0 [20:0]\n",
        "s11 t120 d0 n0 a210 f0 c3 r8 p0 k0 g0 [18:0]\n",
        "s11 t70 d0 n0 a210 f0 c3 r8 p0 k0 g0 []\n",
        "s11 t20 d0 n0 a210 f0 c3 r8 p0 k0 g0 []\n",
        "s0 t0 d0 n0 a0 f0 c3 r8 p0 k0 g0 []\n",
        "s0 t0 d0 n0 a0 f0 c3 r8 p0 k0 g0 []\n",
        "s0 t0 d0 n0 a0 f0 c3 r8 p0 k0 g0 []\n",
        "s0 t0 d0 n0 a0 f0 c3 r8 p0 k0 g0 []\n",
        "s0 t0 d0 n0 a0 f0 c3 r8 p0 k0 g0 []\n",
        "s0 t0 d0 n0 a0 f0 c3 r8 p0 k0 g0 []\n",
        "--\n",
        "s9 t200 d100 n0 a20f f0 c1 r10 p0 k0 g0 [14:0 19:0 17:0]\n",
        "s9 t150 d50 n0 a20f f0 c1 r10 p0 k0 g0 []\n",
        "s9 t100 d0 n0 a20f f0 c2 r9 p0 k0 g0 [20:0]\n",
        "s9 t50 d0 n0 a20f f0 c2 r9 p0 k0 g0 []\n",
        "s7 t150 d100 n0 ad f0 c2 r9 p0 k0 g0 [15:0]\n",
        "s8 t100 d50 n0 ad f0 c2 r9 p0 k0 g0 []\n",
        "s8 t50 d0 n0 ad f0 c3 r8 p0 k0 g0 [20:0]\n",
        "s11 t120 d0 n0 a210 f0 c3 r8 p0 k0 g0 [18:0]\n",
        "s11 t70 d0 n0 a210 f0 c3 r8 p0 k0 g0 []\n",
        "s11 t20 d0 n0 a210 f0 c3 r8 p0 k0 g0 []\n",
        "s0 t0 d0 n0 a0 f0 c3 r8 p0 k0 g0 []\n",
        "s5 t80 d0 n1 a202 f0 c2 r8 p0 k160 g0 [26:0]\n",
        "s5 t30 d0 n1 a202 f0 c2 r8 p0 k110 g0 []\n",
        "s0 t1 d0 n1 a0 f0 c2 r8 p0 k60 g0 []\n",
        "s0 t1 d0 n1 a0 f0 c2 r8 p0 k10 g0 []\n",
        "s0 t1 d0 n1 a0 f0 c2 r8 p0 k0 g0 []\n",
        "s0 t1 d0 n1 a0 f0 c2 r8 p0 k0 g0 []\n",
        "--\n",
        "s9 t200 d100 n0 a20f f0 c0 r10 p0 k0 g0 [14:0 19:0 17:0]\n",
        "s9 t150 d50 n0 a20f f0 c0 r10 p0 k0 g0 []\n",
        "s9 t100 d0 n0 a20f f0 c1 r9 p0 k0 g0 [20:0]\n",
        "s10 t50 d0 n0 a20f f0 c1 r9 p0 k0 g0 []\n",
        "s11 t120 d0 n0 a10 f0 c1 r9 p0 k0 g0 [18:0]\n",
        "s11 t70 d0 n0 a10 f0 c1 r9 p0 k0 g0 []\n",
        "s11 t20 d0 n0 a10 f0 c1 r9 p0 k0 g0 []\n",
        "s0 t0 d0 n0 a200 f0 c1 r9 p0 k0 g0 []\n",
        "s0 t0 d0 n0 a200 f0 c1 r9 p0 k0 g0 []\n",
        "s0 t0 d0 n0 a200 f0 c1 r9 p0 k0 g0 []\n",
        "s0 t0 d0 n0 a200 f0 c1 r9 p0 k0 g0 []\n",
        "s0 t0 d0 n0 a200 f0 c1 r9 p0 k0 g0 []\n",
        "s0 t0 d0 n0 a200 f0 c1 r9 p0 k0 g0 []\n",
        "s0 t0 d0 n0 a200 f0 c1 r9 p0 k0 g0 []\n",
        "--\n",
        "s9 t200 d100 n0 a20f f0 c1 r10 p0 k0 g0 [14:0 19:0 17:0]\n",
        "s9 t160 d60 n0 a20f f0 c1 r10 p0 k0 g0 []\n",
        "s9 t120 d20 n0 a20f f0 c1 r10 p0 k0 g0 []\n",
        "s9 t80 d0 n0 a20f f0 c2 r9 p0 k0 g0 [20:0]\n",
        "s10 t40 d0 n0 a20f f0 c2 r9 p0 k0 g0 []\n",
        "s11 t120 d0 n0 a10 f0 c2 r9 p0 k0 g0 [18:0]\n",
        "s11 t80 d0 n0 a10 f0 c2 r9 p0 k0 g0 []\n",
        "s11 t40 d0 n0 a10 f0 c2 r9 p0 k0 g0 []\n",
        "s0 t0 d0 n0 a200 f0 c2 r9 p0 k0 g0 []\n",
        "s0 t0 d0 n0 a200 f0 c2 r9 p0 k0 g0 []\n",
        "--\n",
        "s9 t200 d100 n0 a20f f0 c0 r10 p0 k0 g0 [14:0 19:0 17:0]\n",
        "s9 t150 d50 n0 a20f f0 c0 r10 p0 k0 g0 []\n",
        "s9 t100 d0 n0 a20f f0 c0 r10 p0 k0 g0 []\n",
        "s10 t50 d0 n0 a20f f0 c0 r10 p0 k0 g0 []\n",
        "s8 t250 d100 n0 ae f0 c0 r10 p0 k0 g0 [16:0]\n",
        "s8 t200 d50 n0 ae f0 c0 r10 p0 k0 g0 []\n",
        "s8 t150 d0 n0 ae f0 c1 r9 p0 k0 g0 [20:0]\n",
        "s8 t100 d0 n0 ae f0 c1 r9 p0 k0 g0 []\n",
        "s8 t50 d0 n0 ae f0 c1 r9 p0 k0 g0 []\n",
        "s11 t120 d0 n0 a210 f0 c1 r9 p0 k0 g0 [18:0]\n",
        "s11 t70 d0 n0 a210 f0 c1 r9 p0 k0 g0 []\n",
        "s11 t20 d0 n0 a210 f0 c1 r9 p0 k0 g0 []\n",
        "s0 t0 d0 n0 a0 f0 c1 r9 p0 k0 g0 []\n",
        "s0 t0 d0 n0 a0 f0 c1 r9 p0 k0 g0 []\n",
    );

    /// The offhand, the melee, the sight, the hip spread and the breath:
    /// one line a cmd as [`trace`] prints it, then the offhand, slot 2's
    /// clip, the sight flag and the prone override, `fWeaponPosFrac`,
    /// `adsDelayTime`, `aimSpreadScale`, the breath's scale and timer and
    /// the charge's flag, distance and yaw. Each cmd runs
    /// `PM_AdjustAimSpreadScale`, `PM_UpdateAimDownSightFlag` and
    /// `PM_Weapon` in `PmoveSingle`'s order.
    mod hands {
        use super::*;

        /// (msec, cmd, count); the cmd's clock is filled in and a weapon
        /// byte of 0 echoes `ps.weapon`.
        type Step = (i32, Cmd, usize);

        #[derive(Default)]
        struct Arm {
            rules: Rules,
            almost_ground_plane: bool,
        }

        fn keys(buttons: u32) -> Cmd {
            Cmd {
                buttons,
                ..Default::default()
            }
        }

        fn drive(arm: &Arm, defs: &[WeaponDef], ps: &mut PlayerState, steps: &[Step]) -> String {
            let defs = defs.to_vec();
            let mut out = String::new();
            let mut old = Cmd {
                server_time: ps.command_time,
                weapon: ps.weapon as u8,
                ..Default::default()
            };
            for &(msec, tmpl, n) in steps {
                for _ in 0..n {
                    let cmd = Cmd {
                        server_time: old.server_time + msec,
                        weapon: if tmpl.weapon == 0 {
                            ps.weapon as u8
                        } else {
                            tmpl.weapon
                        },
                        ..tmpl
                    };
                    let seq = ps.event_sequence;
                    let mut pm = Pmove::new(ps, cmd, old, &Floor, arm.rules);
                    pm.defs = Some(&defs);
                    let pml = Pml {
                        msec,
                        frametime: msec as f32 * 0.001,
                        almost_ground_plane: arm.almost_ground_plane,
                        ..Pml::default()
                    };
                    weapon::adjust_aim_spread(&mut pm, &pml);
                    weapon::update_ads_flag(&mut pm, &pml);
                    weapon::pm_weapon(&mut pm, &pml);
                    let evs: Vec<String> = (seq..ps.event_sequence)
                        .map(|k| {
                            let i = (k & 3) as usize;
                            format!("{}:{}", ps.events[i], ps.event_parms[i])
                        })
                        .collect();
                    out += &format!(
                        "s{} t{} d{} n{} a{:x} f{:x} c{} r{} p{} k{} g{} o{} C{} v{}{} q{} y{} sp{} b{}/{} m{}/{}/{} [{}]\n",
                        ps.weapon_state,
                        ps.weapon_time,
                        ps.weapon_delay,
                        ps.weapon_shot_count,
                        ps.weap_anim,
                        ps.weap_flags,
                        ps.ammoclip[SLOT],
                        ps.ammo[SLOT],
                        u8::from(ps.pm_flags & pmf::PRONE_STILL != 0),
                        ps.weapon_restrict_kick_time,
                        ps.grenade_time_left,
                        ps.off_hand_index,
                        ps.ammoclip[2],
                        u8::from(ps.pm_flags & pmf::SIGHT != 0),
                        u8::from(ps.pm_flags & pmf::PRONE_STILL != 0),
                        ps.weapon_pos_frac,
                        ps.ads_delay_time,
                        ps.aim_spread_scale,
                        ps.hold_breath_scale,
                        ps.hold_breath_timer,
                        u8::from(ps.pm_flags & pmf::MELEE_CHARGE != 0),
                        ps.melee_charge_dist,
                        ps.melee_charge_yaw,
                        evs.join(" ")
                    );
                    old = cmd;
                }
            }
            out
        }

        fn run(defs: &[WeaponDef], ps: &mut PlayerState, steps: &[Step]) -> String {
            drive(&Arm::default(), defs, ps, steps)
        }

        /// A rifle in slot 1 that sights and swings, a cooking frag in
        /// slot 2, a smoke in 3 and a flash in 4, each its own ammo slot.
        fn rifle() -> WeaponDef {
            WeaponDef {
                aim_down_sight: true,
                quick_drop_time: 60,
                quick_raise_time: 70,
                melee_damage: 50,
                melee_time: 300,
                melee_delay: 100,
                melee_charge_time: 500,
                melee_charge_delay: 150,
                position_reload_trans_time: 100,
                oo_pos_anim_length: [1.0 / 150.0, 1.0 / 100.0],
                hip_spread_stand_min: 2.0,
                hip_spread_stand_max: 8.0,
                hip_spread_ducked_min: 1.0,
                hip_spread_ducked_max: 6.0,
                hip_spread_prone_min: 0.5,
                hip_spread_prone_max: 4.0,
                hip_spread_decay_rate: 2.0,
                hip_spread_ducked_decay: 1.5,
                hip_spread_prone_decay: 3.0,
                hip_spread_turn_add: 0.6,
                hip_spread_move_add: 0.8,
                ..def(1)
            }
        }

        fn offhand(slot: usize, class: i32) -> WeaponDef {
            WeaponDef {
                name: format!("offhand{slot}_mp"),
                weap_type: weapon::WEAPTYPE_GRENADE,
                offhand_class: class,
                ammo_index: slot,
                clip_index: slot,
                clip_size: 2,
                clip_only: true,
                hold_fire_time: 150,
                fire_time: 120,
                fire_delay: 50,
                fuse_time: 1000,
                ..WeaponDef::default()
            }
        }

        fn frag() -> WeaponDef {
            WeaponDef {
                cook_off_hold: true,
                ..offhand(2, weapon::OFFHAND_CLASS_FRAG)
            }
        }

        fn kit(rifle: WeaponDef, frag: WeaponDef) -> Vec<WeaponDef> {
            vec![
                WeaponDef::default(),
                rifle,
                frag,
                offhand(3, weapon::OFFHAND_CLASS_SMOKE),
                offhand(4, weapon::OFFHAND_CLASS_FLASH),
            ]
        }

        fn stock() -> Vec<WeaponDef> {
            kit(rifle(), frag())
        }

        /// A player with the rifle up, two frags, a smoke and a flash.
        fn kitted() -> PlayerState {
            let mut ps = armed(5, 10);
            ps.other_flags |= POF_PLAYER;
            ps.weapons[0] |= 0b11100;
            ps.ammoclip[2] = 2;
            ps.ammoclip[3] = 1;
            ps.ammoclip[4] = 1;
            ps
        }

        const F: u32 = button::FRAG;
        const S: u32 = button::SMOKE;
        const M: u32 = button::MELEE;
        const ADS: u32 = button::ADS_MODE;

        #[test]
        fn a_cooked_frag_runs_its_fuse_down_while_the_key_holds() {
            let mut got = run(
                &stock(),
                &mut kitted(),
                &[(50, keys(F), 12), (50, keys(0), 10)],
            );
            got += "--\n";
            // Not cooked, the fuse waits for the release.
            let d = kit(rifle(), offhand(2, weapon::OFFHAND_CLASS_FRAG));
            got += &run(&d, &mut kitted(), &[(50, keys(F), 10), (50, keys(0), 8)]);
            assert_eq!(got, COOK, "{got}");
        }

        #[test]
        fn a_frag_held_past_its_fuse_goes_off_in_the_hand() {
            let d = kit(
                rifle(),
                WeaponDef {
                    fuse_time: 300,
                    ..frag()
                },
            );
            let mut got = run(&d, &mut kitted(), &[(50, keys(F), 14), (50, keys(0), 4)]);
            got += "--\n";
            // A grenade thrown back costs no round.
            let mut ps = kitted();
            ps.throw_back_grenade_owner = 5;
            ps.throw_back_grenade_time_left = 300;
            ps.ammoclip[2] = 0;
            got += &run(&d, &mut ps, &[(50, keys(F), 14), (50, keys(0), 4)]);
            assert_eq!(got, SUICIDE, "{got}");
        }

        #[test]
        fn the_smoke_key_throws_the_smoke_or_under_offhand_secondary_the_flash() {
            let mut got = run(
                &stock(),
                &mut kitted(),
                &[(50, keys(S), 1), (50, keys(0), 12)],
            );
            got += "--\n";
            let mut ps = kitted();
            ps.offhand_secondary = 1;
            got += &run(&stock(), &mut ps, &[(50, keys(S), 1), (50, keys(0), 12)]);
            assert_eq!(got, SMOKE, "{got}");
        }

        #[test]
        fn the_offhand_keys_wait_out_the_sprint_and_click_on_an_empty_class() {
            let mut got = String::new();
            let mut ps = kitted();
            ps.pm_flags |= pmf::SPRINTING;
            ps.weapon_state = weapon::state::SPRINT_HOLD;
            got += &run(&stock(), &mut ps, &[(50, keys(F), 2)]);
            got += "--\n";
            // No frag left but one held: the click and the frag hint.
            let mut ps = kitted();
            ps.ammoclip[2] = 0;
            got += &run(&stock(), &mut ps, &[(50, keys(F), 2)]);
            got += "--\n";
            // No flash held at all: the click alone.
            let mut ps = kitted();
            ps.offhand_secondary = 1;
            ps.weapons[0] &= !(1 << 4);
            got += &run(&stock(), &mut ps, &[(50, keys(S), 1)]);
            got += "--\n";
            // A special grenade held but spent: the special hint.
            let mut ps = kitted();
            ps.ammoclip[3] = 0;
            got += &run(&stock(), &mut ps, &[(50, keys(S), 1)]);
            got += "--\n";
            // The cmd's offhand byte is taken where held, not where not.
            let mut ps = kitted();
            let pick = |o| Cmd {
                offhand: o,
                ..keys(0)
            };
            got += &run(&stock(), &mut ps, &[(50, pick(3), 1), (50, pick(6), 1)]);
            got += "--\n";
            // Weapons off, on a turret, or mid-throw the keys do nothing.
            let mut ps = kitted();
            ps.weap_flags |= weapon::wf::DISABLED;
            got += &run(&stock(), &mut ps, &[(50, keys(F), 1)]);
            let mut ps = kitted();
            ps.weapon_state = weapon::state::GRENADE_HELD;
            ps.off_hand_index = 3;
            got += &run(&stock(), &mut ps, &[(50, keys(F), 1)]);
            assert_eq!(got, BARRED, "{got}");
        }

        #[test]
        fn empty_hands_go_straight_to_the_pin_unless_a_use_hint_shows() {
            let mut got = String::new();
            for hint in [ENTITYNUM_NONE, 0] {
                let mut ps = kitted();
                ps.weapon = 0;
                ps.cursor_hint_ent_index = hint;
                got += &run(&stock(), &mut ps, &[(50, keys(F), 1), (50, keys(0), 9)]);
                got += "--\n";
            }
            assert_eq!(got, EMPTYHANDS, "{got}");
        }

        #[test]
        fn a_hold_to_throw_grenade_let_go_early_is_put_back() {
            let d = kit(
                rifle(),
                WeaponDef {
                    hold_button_to_throw: true,
                    ..frag()
                },
            );
            let mut got = run(&d, &mut kitted(), &[(50, keys(F), 3), (50, keys(0), 3)]);
            got += "--\n";
            // Held past the pin it throws as soon as it can.
            got += &run(&d, &mut kitted(), &[(50, keys(F), 6), (50, keys(0), 6)]);
            got += "--\n";
            // A main-hand one let go in its shot goes back to ready.
            let main = WeaponDef {
                weap_type: weapon::WEAPTYPE_GRENADE,
                hold_button_to_throw: true,
                fire_time: 300,
                ..def(1)
            };
            let mut ps = kitted();
            got += &run(
                &kit(main, frag()),
                &mut ps,
                &[(50, keys(A), 2), (50, keys(0), 2)],
            );
            got += "--\n";
            // A main-hand grenade held by its key keeps the offhand off;
            // a detonator's key is 0x80000.
            let main = WeaponDef {
                weap_type: weapon::WEAPTYPE_GRENADE,
                fire_time: 300,
                fire_delay: 100,
                ..def(1)
            };
            got += &run(
                &kit(main.clone(), frag()),
                &mut kitted(),
                &[(50, keys(A | F), 3)],
            );
            let det = WeaponDef {
                has_detonator: true,
                ..main
            };
            got += "--\n";
            got += &run(
                &kit(det, frag()),
                &mut kitted(),
                &[(50, keys(0x8_0000 | F), 3)],
            );
            assert_eq!(got, CANCEL, "{got}");
        }

        #[test]
        fn the_melee_key_swings_hits_and_raises_a_knife() {
            let mut got = run(
                &stock(),
                &mut kitted(),
                &[(50, keys(M), 1), (50, keys(0), 7)],
            );
            got += "--\n";
            let knife = WeaponDef {
                knife_model: true,
                ..rifle()
            };
            got += &run(
                &kit(knife, frag()),
                &mut kitted(),
                &[(50, keys(M), 1), (50, keys(0), 9)],
            );
            got += "--\n";
            // Prone, the swing overrides the prone move.
            let mut ps = kitted();
            ps.pm_flags |= pmf::PRONE;
            got += &run(&stock(), &mut ps, &[(50, keys(M), 1), (50, keys(0), 2)]);
            assert_eq!(got, MELEE, "{got}");
        }

        #[test]
        fn a_melee_cmd_with_a_distance_starts_the_charge_swing() {
            let lunge = Cmd {
                melee_charge_dist: 90,
                melee_charge_yaw: 0x4000,
                ..keys(M)
            };
            let charged = WeaponDef {
                melee_charge_anim: true,
                knife_model: true,
                ..rifle()
            };
            let mut got = run(
                &kit(charged, frag()),
                &mut kitted(),
                &[(50, lunge, 1), (50, keys(0), 13)],
            );
            got += "--\n";
            // No charge anim: the plain swing, the charge still set.
            got += &run(&stock(), &mut kitted(), &[(50, lunge, 1), (50, keys(0), 1)]);
            got += "--\n";
            // A press with no distance clears a charge left over.
            let mut ps = kitted();
            ps.pm_flags |= pmf::MELEE_CHARGE;
            ps.melee_charge_dist = 40;
            ps.melee_charge_yaw = 10.0;
            got += &run(&stock(), &mut ps, &[(50, keys(M), 1)]);
            assert_eq!(got, CHARGE, "{got}");
        }

        #[test]
        fn the_melee_waits_for_its_edge_a_damage_and_no_scope() {
            let mut got = String::new();
            // Held from the last cmd: no edge.
            got += &run(&stock(), &mut kitted(), &[(50, keys(M), 3)]);
            got += "--\n";
            let mut ps = kitted();
            ps.weapon_pos_frac = 0.5;
            let scoped = WeaponDef {
                overlay_reticle: 1,
                ..rifle()
            };
            got += &run(
                &kit(scoped.clone(), frag()),
                &mut ps,
                &[(50, keys(0), 1), (50, keys(M), 1)],
            );
            got += "--\n";
            // Down out of the sight the scope swings.
            let mut ps = kitted();
            got += &run(
                &kit(scoped, frag()),
                &mut ps,
                &[(50, keys(0), 1), (50, keys(M), 1)],
            );
            got += "--\n";
            let blunt = WeaponDef {
                melee_damage: 0,
                ..rifle()
            };
            got += &run(
                &kit(blunt, frag()),
                &mut kitted(),
                &[(50, keys(0), 1), (50, keys(M), 1)],
            );
            got += "--\n";
            // A shot's delay holds it; a reload's does not; a raise does.
            let delayed = WeaponDef {
                fire_delay: 120,
                ..rifle()
            };
            got += &run(
                &kit(delayed, frag()),
                &mut kitted(),
                &[(50, keys(A), 1), (50, keys(M), 1)],
            );
            got += "--\n";
            got += &run(
                &stock(),
                &mut kitted(),
                &[(50, keys(R), 1), (50, keys(M), 1)],
            );
            got += "--\n";
            let mut ps = kitted();
            ps.weapon_state = weapon::state::RAISE;
            ps.weapon_time = 200;
            got += &run(&stock(), &mut ps, &[(50, keys(0), 1), (50, keys(M), 1)]);
            assert_eq!(got, NOMELEE, "{got}");
        }

        #[test]
        fn the_sight_comes_up_on_its_key_and_goes_down_without_it() {
            let got = run(
                &stock(),
                &mut kitted(),
                &[(50, keys(ADS), 5), (50, keys(0), 3)],
            );
            assert_eq!(got, SIGHT, "{got}");
        }

        #[test]
        fn the_sprint_key_drops_the_sight_but_a_scope_holding_its_breath() {
            let mut got = run(
                &stock(),
                &mut kitted(),
                &[(50, keys(ADS), 4), (50, keys(ADS | button::SPRINT), 2)],
            );
            got += "--\n";
            let scoped = WeaponDef {
                overlay_reticle: 1,
                ..rifle()
            };
            got += &run(
                &kit(scoped, frag()),
                &mut kitted(),
                &[
                    (50, keys(ADS), 4),
                    (50, keys(ADS | button::SPRINT | 0x2000), 2),
                    (50, keys(ADS | button::SPRINT), 1),
                ],
            );
            assert_eq!(got, SPRINTSIGHT, "{got}");
        }

        #[test]
        fn prone_the_sight_takes_the_keys_edge_or_a_still_body() {
            let mut got = String::new();
            let moving = |b| Cmd {
                forward_move: 127,
                ..keys(b)
            };
            let mut ps = kitted();
            ps.pm_flags |= pmf::PRONE;
            ps.e_flags |= ef::PRONE;
            got += &run(
                &stock(),
                &mut ps,
                &[(50, moving(ADS), 2), (50, moving(0), 1), (50, keys(ADS), 1)],
            );
            got += "--\n";
            // Held into prone while crawling, the sight waits.
            let mut ps = kitted();
            ps.pm_flags |= pmf::PRONE;
            ps.e_flags |= ef::PRONE;
            got += &run(
                &stock(),
                &mut ps,
                &[(50, moving(0), 1), (50, moving(ADS), 1)],
            );
            ps.pm_flags &= !pmf::SIGHT;
            got += &run(&stock(), &mut ps, &[(50, keys(ADS), 1)]);
            got += "--\n";
            // A scope part way into its sight sights whatever the move.
            let scoped = WeaponDef {
                overlay_reticle: 1,
                ..rifle()
            };
            let mut ps = kitted();
            ps.pm_flags |= pmf::PRONE;
            ps.e_flags |= ef::PRONE;
            ps.weapon_pos_frac = 0.25;
            got += &run(
                &kit(scoped, frag()),
                &mut ps,
                &[(50, moving(ADS), 1), (50, moving(ADS), 2)],
            );
            assert_eq!(got, PRONESIGHT, "{got}");
        }

        #[test]
        fn the_sight_drops_on_a_ladder_a_reload_and_a_rechamber() {
            let mut got = String::new();
            let mut defs = stock();
            defs.push(WeaponDef {
                ammo_index: 5,
                clip_index: 5,
                ..rifle()
            });
            let mut ps = kitted();
            ps.weapons[0] |= 1 << 5;
            ps.weapon_pos_frac = 1.0;
            ps.pm_flags |= pmf::LADDER;
            got += &run(&defs, &mut ps, &[(50, keys(ADS), 4)]);
            got += "--\n";
            let mut ps = kitted();
            ps.ammoclip[SLOT] = 2;
            got += &run(
                &stock(),
                &mut ps,
                &[
                    (50, keys(ADS), 4),
                    (50, keys(ADS | R), 1),
                    (50, keys(ADS), 8),
                ],
            );
            got += "--\n";
            let seg = WeaponDef {
                segmented_reload: true,
                reload_start_time: 200,
                reload_start_add: 1,
                reload_ammo_add: 1,
                reload_add_time: 100,
                reload_time: 150,
                reload_end_time: 150,
                ..rifle()
            };
            let mut ps = kitted();
            ps.ammoclip[SLOT] = 4;
            got += &run(
                &kit(seg, frag()),
                &mut ps,
                &[
                    (50, keys(ADS), 4),
                    (50, keys(ADS | R), 1),
                    (50, keys(ADS), 10),
                ],
            );
            got += "--\n";
            for keep in [false, true] {
                let bolt = WeaponDef {
                    bolt_action: true,
                    rechamber_time: 200,
                    rechamber_while_ads: keep,
                    ..rifle()
                };
                let mut ps = kitted();
                got += &run(
                    &kit(bolt, frag()),
                    &mut ps,
                    &[
                        (50, keys(ADS), 4),
                        (50, keys(ADS | A), 1),
                        (50, keys(ADS), 5),
                    ],
                );
                got += "--\n";
            }
            assert_eq!(got, LOWERED, "{got}");
        }

        #[test]
        fn the_sight_is_barred_off_the_ground_on_a_turret_and_on_an_empty_mag() {
            let mut got = String::new();
            let mut ps = kitted();
            ps.pm_type = pm_type::NORMAL_LINKED;
            let arm = Arm {
                almost_ground_plane: true,
                ..Arm::default()
            };
            got += &drive(&arm, &stock(), &mut ps, &[(50, keys(ADS), 2)]);
            got += "--\n";
            let mut ps = kitted();
            ps.other_flags = 0;
            got += &run(&stock(), &mut ps, &[(50, keys(ADS), 2)]);
            got += "--\n";
            let mut ps = kitted();
            ps.weap_flags |= weapon::wf::NO_ADS;
            got += &run(&stock(), &mut ps, &[(50, keys(ADS), 2)]);
            got += "--\n";
            let dry = WeaponDef {
                no_ads_when_mag_empty: true,
                ..rifle()
            };
            let mut ps = kitted();
            ps.ammoclip[SLOT] = 0;
            ps.ammo[SLOT] = 0;
            got += &run(&kit(dry, frag()), &mut ps, &[(50, keys(ADS), 2)]);
            got += "--\n";
            let mut ps = kitted();
            ps.weapon_pos_frac = 0.5;
            ps.e_flags |= 0x100;
            got += &run(&stock(), &mut ps, &[(50, keys(ADS), 1)]);
            got += "--\n";
            let flat = WeaponDef {
                aim_down_sight: false,
                ..rifle()
            };
            let mut ps = kitted();
            ps.weapon_pos_frac = 0.5;
            got += &run(&kit(flat, frag()), &mut ps, &[(50, keys(ADS), 1)]);
            got += "--\n";
            // The empty hand drops the sight at once.
            let mut ps = kitted();
            ps.weapon = 0;
            ps.weapon_pos_frac = 0.5;
            ps.ads_delay_time = 9;
            got += &run(&stock(), &mut ps, &[(50, keys(ADS), 1)]);
            assert_eq!(got, BARREDSIGHT, "{got}");
        }

        #[test]
        fn the_hip_spread_decays_by_stance_and_grows_with_turns_runs_and_air() {
            let mut got = String::new();
            for (pm, e) in [(0, 0), (pmf::DUCKED, ef::CROUCH), (pmf::PRONE, ef::PRONE)] {
                let mut ps = kitted();
                ps.pm_flags |= pm;
                ps.e_flags |= e;
                ps.aim_spread_scale = 200.0;
                ps.spread_override = 4;
                got += &run(&stock(), &mut ps, &[(50, keys(0), 2)]);
            }
            got += "--\n";
            // In the air: half the decay, then the air's add.
            let mut ps = kitted();
            ps.ground_entity_num = ENTITYNUM_NONE;
            ps.aim_spread_scale = 100.0;
            got += &run(&stock(), &mut ps, &[(50, keys(0), 2)]);
            got += "--\n";
            // A turn of the view: the cmd angles move 2 degrees a cmd.
            let mut ps = kitted();
            ps.aim_spread_scale = 50.0;
            let mut steps = Vec::new();
            for i in 1..4u16 {
                steps.push((
                    50,
                    Cmd {
                        angles: [i * 364, i * 182, 0],
                        ..keys(0)
                    },
                    1,
                ));
            }
            got += &run(&stock(), &mut ps, &steps);
            got += "--\n";
            // A run past the threshold, then a crawl's pin at 255.
            let mut ps = kitted();
            ps.aim_spread_scale = 50.0;
            ps.velocity = Vec3::new(150.0, 40.0, 0.0);
            ps.speed = 190;
            let run_cmd = Cmd {
                forward_move: 127,
                ..keys(0)
            };
            got += &run(&stock(), &mut ps, &[(50, run_cmd, 2)]);
            ps.velocity = Vec3::new(8.0, 0.0, 0.0);
            got += &run(&stock(), &mut ps, &[(50, run_cmd, 1)]);
            ps.pm_flags |= pmf::PRONE;
            got += &run(&stock(), &mut ps, &[(50, run_cmd, 1)]);
            got += "--\n";
            // Sighted, no add; under the override state the decay is
            // scaled until the scale comes under 255.
            let mut ps = kitted();
            ps.aim_spread_scale = 120.0;
            ps.weapon_pos_frac = 1.0;
            ps.ground_entity_num = ENTITYNUM_NONE;
            got += &run(&stock(), &mut ps, &[(50, keys(ADS), 1)]);
            let mut ps = kitted();
            ps.aim_spread_scale = 250.0;
            ps.spread_override = 6;
            ps.spread_override_state = 1;
            got += &run(&stock(), &mut ps, &[(50, keys(0), 8)]);
            got += "--\n";
            // No decay rate: a whole 255 off a cmd; empty hands likewise.
            let still = WeaponDef {
                hip_spread_decay_rate: 0.0,
                ..rifle()
            };
            let mut ps = kitted();
            ps.aim_spread_scale = 200.0;
            got += &run(&kit(still, frag()), &mut ps, &[(50, keys(0), 1)]);
            let mut ps = kitted();
            ps.weapon = 0;
            ps.aim_spread_scale = 200.0;
            got += &run(&stock(), &mut ps, &[(50, keys(0), 1)]);
            assert_eq!(got, SPREAD, "{got}");
        }

        #[test]
        fn a_sighted_scope_holds_its_breath_until_it_gasps() {
            let scoped = WeaponDef {
                overlay_reticle: 1,
                oo_pos_anim_length: [1.0 / 50.0, 1.0 / 50.0],
                ..rifle()
            };
            let rules = Rules {
                breath_hold_time: 0.4,
                breath_gasp_time: 0.3,
                ..Rules::default()
            };
            let arm = Arm {
                rules,
                ..Arm::default()
            };
            let hold = keys(ADS | 0x2000);
            let mut got = drive(
                &arm,
                &kit(scoped.clone(), frag()),
                &mut kitted(),
                &[(50, keys(ADS), 1), (50, hold, 12), (50, keys(ADS), 8)],
            );
            got += "--\n";
            // Let go mid-hold it recovers; a fresh press waits for 0.
            let mut ps = kitted();
            got += &drive(
                &arm,
                &kit(scoped.clone(), frag()),
                &mut ps,
                &[
                    (50, keys(ADS), 1),
                    (50, hold, 3),
                    (50, keys(ADS), 1),
                    (50, hold, 4),
                ],
            );
            got += "--\n";
            // The perk adds its 5 s.
            let mut ps = kitted();
            ps.perks |= 0x10;
            got += &drive(
                &arm,
                &kit(scoped.clone(), frag()),
                &mut ps,
                &[(50, keys(ADS), 1), (50, hold, 10)],
            );
            got += "--\n";
            // A plain sight, an item and a hold time of 0 never hold.
            got += &drive(
                &arm,
                &stock(),
                &mut kitted(),
                &[(50, keys(ADS), 4), (50, hold, 2)],
            );
            let item = WeaponDef {
                weap_class: weapon::WEAPCLASS_ITEM,
                ..scoped.clone()
            };
            got += &drive(
                &arm,
                &kit(item, frag()),
                &mut kitted(),
                &[(50, keys(ADS), 1), (50, hold, 2)],
            );
            let none = Arm {
                rules: Rules {
                    breath_hold_time: 0.0,
                    ..rules
                },
                ..Arm::default()
            };
            let mut ps = kitted();
            ps.hold_breath_scale = 3.0;
            ps.hold_breath_timer = 200;
            got += &drive(&none, &kit(scoped, frag()), &mut ps, &[(50, hold, 2)]);
            assert_eq!(got, BREATH, "{got}");
        }

        /// 1.7 tests the hold time with the perk's added (0x8062ea1): at
        /// `player_breath_hold_time` 0 the perk alone holds the breath.
        #[test]
        fn the_perk_holds_the_breath_with_no_hold_time() {
            let scoped = WeaponDef {
                overlay_reticle: 1,
                oo_pos_anim_length: [1.0 / 50.0, 1.0 / 50.0],
                ..rifle()
            };
            let arm = Arm {
                rules: Rules {
                    breath_hold_time: 0.0,
                    breath_gasp_time: 0.3,
                    ..Rules::default()
                },
                ..Arm::default()
            };
            let mut ps = kitted();
            ps.perks |= 0x10;
            let hold = keys(ADS | 0x2000);
            drive(
                &arm,
                &kit(scoped, frag()),
                &mut ps,
                &[(50, keys(ADS), 1), (50, hold, 3)],
            );
            assert_eq!((ps.weap_flags & 4, ps.hold_breath_timer), (4, 150));
        }

        /// The breath reads the view weapon's def (0x8062e42): a sighted
        /// scope with the offhand showing (`weapFlags` 2) holds nothing.
        #[test]
        fn the_breath_reads_the_offhand_while_it_shows() {
            let scoped = WeaponDef {
                overlay_reticle: 1,
                oo_pos_anim_length: [1.0 / 50.0, 1.0 / 50.0],
                ..rifle()
            };
            let arm = Arm {
                rules: Rules {
                    breath_hold_time: 0.4,
                    breath_gasp_time: 0.3,
                    ..Rules::default()
                },
                ..Arm::default()
            };
            let defs = kit(scoped, frag());
            let hold = keys(ADS | 0x2000);
            let held = |showing: bool| {
                let mut ps = kitted();
                drive(&arm, &defs, &mut ps, &[(50, keys(ADS), 1)]);
                assert_eq!(ps.weapon_pos_frac, 1.0);
                if showing {
                    ps.weap_flags |= 2;
                    ps.off_hand_index = 2;
                }
                drive(&arm, &defs, &mut ps, &[(50, hold, 3)]);
                (ps.weap_flags & 4, ps.hold_breath_timer)
            };
            assert_eq!(held(false), (4, 150));
            assert_eq!(held(true), (0, 0));
        }

        /// `PM_MeleeChargeUpdate` through the whole step: the charge
        /// stops on a ladder, a turret and a dead or linked body.
        #[test]
        fn a_melee_charge_is_cleared_off_the_plain_walk() {
            let mut got = String::new();
            let charged = |f: &dyn Fn(&mut PlayerState)| {
                let mut ps = standing();
                ps.pm_flags |= pmf::MELEE_CHARGE;
                ps.melee_charge_yaw = 30.0;
                ps.melee_charge_dist = 70;
                f(&mut ps);
                run_steps(&mut ps, 2);
                format!(
                    "{} {} {} {:?}\n",
                    u8::from(ps.pm_flags & pmf::MELEE_CHARGE != 0),
                    ps.melee_charge_dist,
                    ps.melee_charge_time,
                    ps.velocity
                )
            };
            got += &charged(&|_| {});
            got += &charged(&|ps| ps.pm_flags |= pmf::LADDER);
            got += &charged(&|ps| ps.e_flags |= 0x200);
            got += &charged(&|ps| ps.pm_type = pm_type::NORMAL_LINKED);
            got += &charged(&|ps| ps.pm_type = pm_type::DEAD);
            assert_eq!(got, CHARGEOFF, "{got}");
        }

        fn run_steps(ps: &mut PlayerState, n: usize) {
            super::super::run(ps, n, 0, 0, WeaponMove::NONE);
        }

        const COOK: &str = concat!(
            "s15 t60 d0 n0 a213 f0 c5 r10 p0 k0 g0 o2 C2 v00 q0 y0 sp0 b1/0 m0/0/0 [34:2 14:0]\n",
            "s15 t10 d0 n0 a213 f0 c5 r10 p0 k0 g0 o2 C2 v00 q0 y0 sp0 b1/0 m0/0/0 []\n",
            "s16 t150 d0 n0 a1a f2 c5 r10 p0 k0 g0 o2 C2 v00 q0 y0 sp0 b1/0 m0/0/0 [32:2]\n",
            "s16 t100 d0 n0 a1a f2 c5 r10 p0 k0 g0 o2 C2 v00 q0 y0 sp0 b1/0 m0/0/0 []\n",
            "s16 t50 d0 n0 a1a f2 c5 r10 p0 k0 g0 o2 C2 v00 q0 y0 sp0 b1/0 m0/0/0 []\n",
            "s18 t0 d0 n0 a1a f2 c5 r10 p0 k0 g1000 o2 C2 v00 q0 y0 sp0 b1/0 m0/0/0 []\n",
            "s18 t0 d1 n0 a1a f2 c5 r10 p0 k0 g950 o2 C2 v00 q0 y0 sp0 b1/0 m0/0/0 []\n",
            "s18 t0 d1 n0 a1a f2 c5 r10 p0 k0 g900 o2 C2 v00 q0 y0 sp0 b1/0 m0/0/0 []\n",
            "s18 t0 d1 n0 a1a f2 c5 r10 p0 k0 g850 o2 C2 v00 q0 y0 sp0 b1/0 m0/0/0 []\n",
            "s18 t0 d1 n0 a1a f2 c5 r10 p0 k0 g800 o2 C2 v00 q0 y0 sp0 b1/0 m0/0/0 []\n",
            "s18 t0 d1 n0 a1a f2 c5 r10 p0 k0 g750 o2 C2 v00 q0 y0 sp0 b1/0 m0/0/0 []\n",
            "s18 t0 d1 n0 a1a f2 c5 r10 p0 k0 g700 o2 C2 v00 q0 y0 sp0 b1/0 m0/0/0 []\n",
            "s17 t120 d50 n0 a202 f2 c5 r10 p0 k0 g650 o2 C2 v00 q0 y0 sp0 b1/0 m0/0/0 []\n",
            "s19 t70 d0 n0 a202 f2 c5 r10 p0 k0 g600 o2 C1 v00 q0 y0 sp0 b1/0 m0/0/0 [33:2]\n",
            "s19 t20 d0 n0 a202 f2 c5 r10 p0 k0 g550 o2 C1 v00 q0 y0 sp0 b1/0 m0/0/0 []\n",
            "s20 t70 d0 n0 a14 f0 c5 r10 p0 k0 g500 o2 C1 v00 q0 y0 sp0 b1/0 m0/0/0 []\n",
            "s20 t20 d0 n0 a14 f0 c5 r10 p0 k0 g500 o2 C1 v00 q0 y0 sp0 b1/0 m0/0/0 []\n",
            "s0 t0 d0 n0 a200 f0 c5 r10 p0 k0 g500 o2 C1 v00 q0 y0 sp0 b1/0 m0/0/0 []\n",
            "s0 t0 d0 n0 a200 f0 c5 r10 p0 k0 g500 o2 C1 v00 q0 y0 sp0 b1/0 m0/0/0 []\n",
            "s0 t0 d0 n0 a200 f0 c5 r10 p0 k0 g500 o2 C1 v00 q0 y0 sp0 b1/0 m0/0/0 []\n",
            "s0 t0 d0 n0 a200 f0 c5 r10 p0 k0 g500 o2 C1 v00 q0 y0 sp0 b1/0 m0/0/0 []\n",
            "s0 t0 d0 n0 a200 f0 c5 r10 p0 k0 g500 o2 C1 v00 q0 y0 sp0 b1/0 m0/0/0 []\n",
            "--\n",
            "s15 t60 d0 n0 a213 f0 c5 r10 p0 k0 g0 o2 C2 v00 q0 y0 sp0 b1/0 m0/0/0 [34:2 14:0]\n",
            "s15 t10 d0 n0 a213 f0 c5 r10 p0 k0 g0 o2 C2 v00 q0 y0 sp0 b1/0 m0/0/0 []\n",
            "s16 t150 d0 n0 a1a f2 c5 r10 p0 k0 g0 o2 C2 v00 q0 y0 sp0 b1/0 m0/0/0 [32:2]\n",
            "s16 t100 d0 n0 a1a f2 c5 r10 p0 k0 g0 o2 C2 v00 q0 y0 sp0 b1/0 m0/0/0 []\n",
            "s16 t50 d0 n0 a1a f2 c5 r10 p0 k0 g0 o2 C2 v00 q0 y0 sp0 b1/0 m0/0/0 []\n",
            "s18 t0 d0 n0 a1a f2 c5 r10 p0 k0 g1000 o2 C2 v00 q0 y0 sp0 b1/0 m0/0/0 []\n",
            "s18 t0 d1 n0 a1a f2 c5 r10 p0 k0 g1000 o2 C2 v00 q0 y0 sp0 b1/0 m0/0/0 []\n",
            "s18 t0 d1 n0 a1a f2 c5 r10 p0 k0 g1000 o2 C2 v00 q0 y0 sp0 b1/0 m0/0/0 []\n",
            "s18 t0 d1 n0 a1a f2 c5 r10 p0 k0 g1000 o2 C2 v00 q0 y0 sp0 b1/0 m0/0/0 []\n",
            "s18 t0 d1 n0 a1a f2 c5 r10 p0 k0 g1000 o2 C2 v00 q0 y0 sp0 b1/0 m0/0/0 []\n",
            "s17 t120 d50 n0 a202 f2 c5 r10 p0 k0 g1000 o2 C2 v00 q0 y0 sp0 b1/0 m0/0/0 []\n",
            "s19 t70 d0 n0 a202 f2 c5 r10 p0 k0 g1000 o2 C1 v00 q0 y0 sp0 b1/0 m0/0/0 [33:2]\n",
            "s19 t20 d0 n0 a202 f2 c5 r10 p0 k0 g1000 o2 C1 v00 q0 y0 sp0 b1/0 m0/0/0 []\n",
            "s20 t70 d0 n0 a14 f0 c5 r10 p0 k0 g1000 o2 C1 v00 q0 y0 sp0 b1/0 m0/0/0 []\n",
            "s20 t20 d0 n0 a14 f0 c5 r10 p0 k0 g1000 o2 C1 v00 q0 y0 sp0 b1/0 m0/0/0 []\n",
            "s0 t0 d0 n0 a200 f0 c5 r10 p0 k0 g1000 o2 C1 v00 q0 y0 sp0 b1/0 m0/0/0 []\n",
            "s0 t0 d0 n0 a200 f0 c5 r10 p0 k0 g1000 o2 C1 v00 q0 y0 sp0 b1/0 m0/0/0 []\n",
            "s0 t0 d0 n0 a200 f0 c5 r10 p0 k0 g1000 o2 C1 v00 q0 y0 sp0 b1/0 m0/0/0 []\n",
        );
        const SUICIDE: &str = concat!(
            "s15 t60 d0 n0 a213 f0 c5 r10 p0 k0 g0 o2 C2 v00 q0 y0 sp0 b1/0 m0/0/0 [34:2 14:0]\n",
            "s15 t10 d0 n0 a213 f0 c5 r10 p0 k0 g0 o2 C2 v00 q0 y0 sp0 b1/0 m0/0/0 []\n",
            "s16 t150 d0 n0 a1a f2 c5 r10 p0 k0 g0 o2 C2 v00 q0 y0 sp0 b1/0 m0/0/0 [32:2]\n",
            "s16 t100 d0 n0 a1a f2 c5 r10 p0 k0 g0 o2 C2 v00 q0 y0 sp0 b1/0 m0/0/0 []\n",
            "s16 t50 d0 n0 a1a f2 c5 r10 p0 k0 g0 o2 C2 v00 q0 y0 sp0 b1/0 m0/0/0 []\n",
            "s18 t0 d0 n0 a1a f2 c5 r10 p0 k0 g300 o2 C2 v00 q0 y0 sp0 b1/0 m0/0/0 []\n",
            "s18 t0 d1 n0 a1a f2 c5 r10 p0 k0 g250 o2 C2 v00 q0 y0 sp0 b1/0 m0/0/0 []\n",
            "s18 t0 d1 n0 a1a f2 c5 r10 p0 k0 g200 o2 C2 v00 q0 y0 sp0 b1/0 m0/0/0 []\n",
            "s18 t0 d1 n0 a1a f2 c5 r10 p0 k0 g150 o2 C2 v00 q0 y0 sp0 b1/0 m0/0/0 []\n",
            "s18 t0 d1 n0 a1a f2 c5 r10 p0 k0 g100 o2 C2 v00 q0 y0 sp0 b1/0 m0/0/0 []\n",
            "s18 t0 d1 n0 a1a f2 c5 r10 p0 k0 g50 o2 C2 v00 q0 y0 sp0 b1/0 m0/0/0 []\n",
            "s18 t0 d1 n0 a1a f2 c5 r10 p0 k0 g-1 o2 C1 v00 q0 y0 sp0 b1/0 m0/0/0 [62:2]\n",
            "s18 t0 d1 n0 a1a f2 c5 r10 p0 k0 g-1 o2 C1 v00 q0 y0 sp0 b1/0 m0/0/0 []\n",
            "s18 t0 d1 n0 a1a f2 c5 r10 p0 k0 g-1 o2 C1 v00 q0 y0 sp0 b1/0 m0/0/0 []\n",
            "s17 t120 d50 n0 a202 f2 c5 r10 p0 k0 g-1 o2 C1 v00 q0 y0 sp0 b1/0 m0/0/0 []\n",
            "s19 t70 d0 n0 a202 f2 c5 r10 p0 k0 g-1 o2 C0 v00 q0 y0 sp0 b1/0 m0/0/0 [33:2]\n",
            "s19 t20 d0 n0 a202 f2 c5 r10 p0 k0 g-1 o2 C0 v00 q0 y0 sp0 b1/0 m0/0/0 []\n",
            "s20 t70 d0 n0 a14 f0 c5 r10 p0 k0 g-1 o2 C0 v00 q0 y0 sp0 b1/0 m0/0/0 []\n",
            "--\n",
            "s15 t60 d0 n0 a213 f0 c5 r10 p0 k0 g0 o2 C0 v00 q0 y0 sp0 b1/0 m0/0/0 [34:2 14:0]\n",
            "s15 t10 d0 n0 a213 f0 c5 r10 p0 k0 g0 o2 C0 v00 q0 y0 sp0 b1/0 m0/0/0 []\n",
            "s16 t150 d0 n0 a1a f2 c5 r10 p0 k0 g0 o2 C0 v00 q0 y0 sp0 b1/0 m0/0/0 [32:2]\n",
            "s16 t100 d0 n0 a1a f2 c5 r10 p0 k0 g0 o2 C0 v00 q0 y0 sp0 b1/0 m0/0/0 []\n",
            "s16 t50 d0 n0 a1a f2 c5 r10 p0 k0 g0 o2 C0 v00 q0 y0 sp0 b1/0 m0/0/0 []\n",
            "s18 t0 d0 n0 a1a f2 c5 r10 p0 k0 g300 o2 C0 v00 q0 y0 sp0 b1/0 m0/0/0 []\n",
            "s18 t0 d1 n0 a1a f2 c5 r10 p0 k0 g250 o2 C0 v00 q0 y0 sp0 b1/0 m0/0/0 []\n",
            "s18 t0 d1 n0 a1a f2 c5 r10 p0 k0 g200 o2 C0 v00 q0 y0 sp0 b1/0 m0/0/0 []\n",
            "s18 t0 d1 n0 a1a f2 c5 r10 p0 k0 g150 o2 C0 v00 q0 y0 sp0 b1/0 m0/0/0 []\n",
            "s18 t0 d1 n0 a1a f2 c5 r10 p0 k0 g100 o2 C0 v00 q0 y0 sp0 b1/0 m0/0/0 []\n",
            "s18 t0 d1 n0 a1a f2 c5 r10 p0 k0 g50 o2 C0 v00 q0 y0 sp0 b1/0 m0/0/0 []\n",
            "s18 t0 d1 n0 a1a f2 c5 r10 p0 k0 g-1 o2 C0 v00 q0 y0 sp0 b1/0 m0/0/0 [62:2]\n",
            "s18 t0 d1 n0 a1a f2 c5 r10 p0 k0 g-1 o2 C0 v00 q0 y0 sp0 b1/0 m0/0/0 []\n",
            "s18 t0 d1 n0 a1a f2 c5 r10 p0 k0 g-1 o2 C0 v00 q0 y0 sp0 b1/0 m0/0/0 []\n",
            "s17 t120 d50 n0 a202 f2 c5 r10 p0 k0 g-1 o2 C0 v00 q0 y0 sp0 b1/0 m0/0/0 []\n",
            "s19 t70 d0 n0 a202 f2 c5 r10 p0 k0 g-1 o2 C0 v00 q0 y0 sp0 b1/0 m0/0/0 [33:2 13:0]\n",
            "s19 t20 d0 n0 a202 f2 c5 r10 p0 k0 g-1 o2 C0 v00 q0 y0 sp0 b1/0 m0/0/0 []\n",
            "s20 t70 d0 n0 a14 f0 c5 r10 p0 k0 g-1 o2 C0 v00 q0 y0 sp0 b1/0 m0/0/0 []\n",
        );
        const SMOKE: &str = concat!(
            "s15 t60 d0 n0 a213 f0 c5 r10 p0 k0 g0 o3 C2 v00 q0 y0 sp0 b1/0 m0/0/0 [34:3 14:0]\n",
            "s15 t10 d0 n0 a213 f0 c5 r10 p0 k0 g0 o3 C2 v00 q0 y0 sp0 b1/0 m0/0/0 []\n",
            "s16 t150 d0 n0 a1a f2 c5 r10 p0 k0 g0 o3 C2 v00 q0 y0 sp0 b1/0 m0/0/0 [32:3]\n",
            "s16 t100 d0 n0 a1a f2 c5 r10 p0 k0 g0 o3 C2 v00 q0 y0 sp0 b1/0 m0/0/0 []\n",
            "s16 t50 d0 n0 a1a f2 c5 r10 p0 k0 g0 o3 C2 v00 q0 y0 sp0 b1/0 m0/0/0 []\n",
            "s18 t0 d0 n0 a1a f2 c5 r10 p0 k0 g1000 o3 C2 v00 q0 y0 sp0 b1/0 m0/0/0 []\n",
            "s17 t120 d50 n0 a202 f2 c5 r10 p0 k0 g1000 o3 C2 v00 q0 y0 sp0 b1/0 m0/0/0 []\n",
            "s19 t70 d0 n0 a202 f2 c5 r10 p0 k0 g1000 o3 C2 v00 q0 y0 sp0 b1/0 m0/0/0 [33:3]\n",
            "s19 t20 d0 n0 a202 f2 c5 r10 p0 k0 g1000 o3 C2 v00 q0 y0 sp0 b1/0 m0/0/0 []\n",
            "s20 t70 d0 n0 a14 f0 c5 r10 p0 k0 g1000 o3 C2 v00 q0 y0 sp0 b1/0 m0/0/0 []\n",
            "s20 t20 d0 n0 a14 f0 c5 r10 p0 k0 g1000 o3 C2 v00 q0 y0 sp0 b1/0 m0/0/0 []\n",
            "s0 t0 d0 n0 a200 f0 c5 r10 p0 k0 g1000 o3 C2 v00 q0 y0 sp0 b1/0 m0/0/0 []\n",
            "s0 t0 d0 n0 a200 f0 c5 r10 p0 k0 g1000 o3 C2 v00 q0 y0 sp0 b1/0 m0/0/0 []\n",
            "--\n",
            "s15 t60 d0 n0 a213 f0 c5 r10 p0 k0 g0 o4 C2 v00 q0 y0 sp0 b1/0 m0/0/0 [34:4 14:0]\n",
            "s15 t10 d0 n0 a213 f0 c5 r10 p0 k0 g0 o4 C2 v00 q0 y0 sp0 b1/0 m0/0/0 []\n",
            "s16 t150 d0 n0 a1a f2 c5 r10 p0 k0 g0 o4 C2 v00 q0 y0 sp0 b1/0 m0/0/0 [32:4]\n",
            "s16 t100 d0 n0 a1a f2 c5 r10 p0 k0 g0 o4 C2 v00 q0 y0 sp0 b1/0 m0/0/0 []\n",
            "s16 t50 d0 n0 a1a f2 c5 r10 p0 k0 g0 o4 C2 v00 q0 y0 sp0 b1/0 m0/0/0 []\n",
            "s18 t0 d0 n0 a1a f2 c5 r10 p0 k0 g1000 o4 C2 v00 q0 y0 sp0 b1/0 m0/0/0 []\n",
            "s17 t120 d50 n0 a202 f2 c5 r10 p0 k0 g1000 o4 C2 v00 q0 y0 sp0 b1/0 m0/0/0 []\n",
            "s19 t70 d0 n0 a202 f2 c5 r10 p0 k0 g1000 o4 C2 v00 q0 y0 sp0 b1/0 m0/0/0 [33:4]\n",
            "s19 t20 d0 n0 a202 f2 c5 r10 p0 k0 g1000 o4 C2 v00 q0 y0 sp0 b1/0 m0/0/0 []\n",
            "s20 t70 d0 n0 a14 f0 c5 r10 p0 k0 g1000 o4 C2 v00 q0 y0 sp0 b1/0 m0/0/0 []\n",
            "s20 t20 d0 n0 a14 f0 c5 r10 p0 k0 g1000 o4 C2 v00 q0 y0 sp0 b1/0 m0/0/0 []\n",
            "s0 t0 d0 n0 a200 f0 c5 r10 p0 k0 g1000 o4 C2 v00 q0 y0 sp0 b1/0 m0/0/0 []\n",
            "s0 t0 d0 n0 a200 f0 c5 r10 p0 k0 g1000 o4 C2 v00 q0 y0 sp0 b1/0 m0/0/0 []\n",
        );
        const BARRED: &str = concat!(
            "s23 t0 d0 n0 a0 f0 c5 r10 p0 k0 g0 o0 C2 v00 q0 y0 sp0 b1/0 m0/0/0 []\n",
            "s23 t0 d0 n0 a0 f0 c5 r10 p0 k0 g0 o0 C2 v00 q0 y0 sp0 b1/0 m0/0/0 []\n",
            "--\n",
            "s0 t0 d0 n0 a0 f0 c5 r10 p0 k0 g0 o0 C0 v00 q0 y0 sp0 b1/0 m0/0/0 [13:0 67:0]\n",
            "s0 t0 d0 n0 a0 f0 c5 r10 p0 k0 g0 o0 C0 v00 q0 y0 sp0 b1/0 m0/0/0 [13:0 67:0]\n",
            "--\n",
            "s0 t0 d0 n0 a0 f0 c5 r10 p0 k0 g0 o0 C2 v00 q0 y0 sp0 b1/0 m0/0/0 [13:0]\n",
            "--\n",
            "s0 t0 d0 n0 a0 f0 c5 r10 p0 k0 g0 o0 C2 v00 q0 y0 sp0 b1/0 m0/0/0 [13:0 68:0]\n",
            "--\n",
            "s0 t0 d0 n0 a0 f0 c5 r10 p0 k0 g0 o3 C2 v00 q0 y0 sp0 b1/0 m0/0/0 []\n",
            "s0 t0 d0 n0 a0 f0 c5 r10 p0 k0 g0 o3 C2 v00 q0 y0 sp0 b1/0 m0/0/0 []\n",
            "--\n",
            "s3 t100 d0 n0 a20a f80 c5 r10 p0 k0 g0 o0 C2 v00 q0 y0 sp0 b1/0 m0/0/0 [23:0]\n",
            "s17 t120 d50 n0 a202 f2 c5 r10 p0 k0 g0 o3 C2 v00 q0 y0 sp0 b1/0 m0/0/0 []\n",
        );
        const EMPTYHANDS: &str = concat!(
            "s16 t150 d0 n0 a21a f2 c5 r10 p0 k0 g0 o2 C2 v00 q0 y0 sp0 b1/0 m0/0/0 [34:2 32:2]\n",
            "s16 t100 d0 n0 a21a f2 c5 r10 p0 k0 g0 o2 C2 v00 q0 y0 sp0 b1/0 m0/0/0 []\n",
            "s16 t50 d0 n0 a21a f2 c5 r10 p0 k0 g0 o2 C2 v00 q0 y0 sp0 b1/0 m0/0/0 []\n",
            "s18 t0 d0 n0 a21a f2 c5 r10 p0 k0 g1000 o2 C2 v00 q0 y0 sp0 b1/0 m0/0/0 []\n",
            "s17 t120 d50 n0 a2 f2 c5 r10 p0 k0 g950 o2 C2 v00 q0 y0 sp0 b1/0 m0/0/0 []\n",
            "s19 t70 d0 n0 a2 f2 c5 r10 p0 k0 g900 o2 C1 v00 q0 y0 sp0 b1/0 m0/0/0 [33:2]\n",
            "s19 t20 d0 n0 a2 f2 c5 r10 p0 k0 g850 o2 C1 v00 q0 y0 sp0 b1/0 m0/0/0 []\n",
            "s20 t0 d1 n0 a2 f0 c5 r10 p0 k0 g800 o2 C1 v00 q0 y0 sp0 b1/0 m0/0/0 []\n",
            "s0 t0 d0 n0 a200 f0 c5 r10 p0 k0 g800 o2 C1 v00 q0 y0 sp0 b1/0 m0/0/0 []\n",
            "s0 t0 d0 n0 a200 f0 c5 r10 p0 k0 g800 o2 C1 v00 q0 y0 sp0 b1/0 m0/0/0 []\n",
            "--\n",
            "s15 t100 d0 n0 a0 f0 c5 r10 p0 k0 g0 o2 C2 v00 q0 y0 sp0 b1/0 m0/0/0 [34:2 14:0]\n",
            "s15 t50 d0 n0 a0 f0 c5 r10 p0 k0 g0 o2 C2 v00 q0 y0 sp0 b1/0 m0/0/0 []\n",
            "s16 t150 d0 n0 a21a f2 c5 r10 p0 k0 g0 o2 C2 v00 q0 y0 sp0 b1/0 m0/0/0 [32:2]\n",
            "s16 t100 d0 n0 a21a f2 c5 r10 p0 k0 g0 o2 C2 v00 q0 y0 sp0 b1/0 m0/0/0 []\n",
            "s16 t50 d0 n0 a21a f2 c5 r10 p0 k0 g0 o2 C2 v00 q0 y0 sp0 b1/0 m0/0/0 []\n",
            "s18 t0 d0 n0 a21a f2 c5 r10 p0 k0 g1000 o2 C2 v00 q0 y0 sp0 b1/0 m0/0/0 []\n",
            "s17 t120 d50 n0 a2 f2 c5 r10 p0 k0 g950 o2 C2 v00 q0 y0 sp0 b1/0 m0/0/0 []\n",
            "s19 t70 d0 n0 a2 f2 c5 r10 p0 k0 g900 o2 C1 v00 q0 y0 sp0 b1/0 m0/0/0 [33:2]\n",
            "s19 t20 d0 n0 a2 f2 c5 r10 p0 k0 g850 o2 C1 v00 q0 y0 sp0 b1/0 m0/0/0 []\n",
            "s20 t0 d1 n0 a2 f0 c5 r10 p0 k0 g800 o2 C1 v00 q0 y0 sp0 b1/0 m0/0/0 []\n",
            "--\n",
        );
        const CANCEL: &str = concat!(
            "s15 t60 d0 n0 a213 f0 c5 r10 p0 k0 g0 o2 C2 v00 q0 y0 sp0 b1/0 m0/0/0 [34:2 14:0]\n",
            "s15 t10 d0 n0 a213 f0 c5 r10 p0 k0 g0 o2 C2 v00 q0 y0 sp0 b1/0 m0/0/0 []\n",
            "s16 t150 d0 n0 a1a f2 c5 r10 p0 k0 g0 o2 C2 v00 q0 y0 sp0 b1/0 m0/0/0 [32:2]\n",
            "s20 t70 d0 n0 a214 f0 c5 r10 p0 k0 g0 o2 C2 v00 q0 y0 sp0 b1/0 m0/0/0 []\n",
            "s20 t20 d0 n0 a214 f0 c5 r10 p0 k0 g0 o2 C2 v00 q0 y0 sp0 b1/0 m0/0/0 []\n",
            "s0 t0 d0 n0 a0 f0 c5 r10 p0 k0 g0 o2 C2 v00 q0 y0 sp0 b1/0 m0/0/0 []\n",
            "--\n",
            "s15 t60 d0 n0 a213 f0 c5 r10 p0 k0 g0 o2 C2 v00 q0 y0 sp0 b1/0 m0/0/0 [34:2 14:0]\n",
            "s15 t10 d0 n0 a213 f0 c5 r10 p0 k0 g0 o2 C2 v00 q0 y0 sp0 b1/0 m0/0/0 []\n",
            "s16 t150 d0 n0 a1a f2 c5 r10 p0 k0 g0 o2 C2 v00 q0 y0 sp0 b1/0 m0/0/0 [32:2]\n",
            "s16 t100 d0 n0 a1a f2 c5 r10 p0 k0 g0 o2 C2 v00 q0 y0 sp0 b1/0 m0/0/0 []\n",
            "s16 t50 d0 n0 a1a f2 c5 r10 p0 k0 g0 o2 C2 v00 q0 y0 sp0 b1/0 m0/0/0 []\n",
            "s18 t0 d0 n0 a1a f2 c5 r10 p0 k0 g1000 o2 C2 v00 q0 y0 sp0 b1/0 m0/0/0 []\n",
            "s17 t120 d50 n0 a202 f2 c5 r10 p0 k0 g950 o2 C2 v00 q0 y0 sp0 b1/0 m0/0/0 []\n",
            "s19 t70 d0 n0 a202 f2 c5 r10 p0 k0 g900 o2 C1 v00 q0 y0 sp0 b1/0 m0/0/0 [33:2]\n",
            "s19 t20 d0 n0 a202 f2 c5 r10 p0 k0 g850 o2 C1 v00 q0 y0 sp0 b1/0 m0/0/0 []\n",
            "s20 t70 d0 n0 a14 f0 c5 r10 p0 k0 g800 o2 C1 v00 q0 y0 sp0 b1/0 m0/0/0 []\n",
            "s20 t20 d0 n0 a14 f0 c5 r10 p0 k0 g800 o2 C1 v00 q0 y0 sp0 b1/0 m0/0/0 []\n",
            "s0 t0 d0 n0 a200 f0 c5 r10 p0 k0 g800 o2 C1 v00 q0 y0 sp0 b1/0 m0/0/0 []\n",
            "--\n",
            "s5 t300 d0 n1 a2 f0 c4 r10 p0 k0 g0 o0 C2 v00 q0 y0 sp63.75 b1/0 m0/0/0 [25:1 26:0]\n",
            "s5 t250 d0 n1 a2 f0 c4 r10 p0 k0 g0 o0 C2 v00 q0 y0 sp0 b1/0 m0/0/0 []\n",
            "s0 t0 d0 n1 a1 f0 c4 r10 p0 k0 g0 o0 C2 v00 q0 y0 sp0 b1/0 m0/0/0 []\n",
            "s0 t0 d0 n1 a1 f0 c4 r10 p0 k0 g0 o0 C2 v00 q0 y0 sp0 b1/0 m0/0/0 []\n",
            "--\n",
            "s5 t300 d0 n1 a2 f0 c4 r10 p0 k0 g0 o0 C2 v00 q0 y0 sp63.75 b1/0 m0/0/0 [25:1 26:0]\n",
            "s5 t250 d0 n1 a2 f0 c4 r10 p0 k0 g0 o0 C2 v00 q0 y0 sp0 b1/0 m0/0/0 []\n",
            "s5 t200 d0 n1 a2 f0 c4 r10 p0 k0 g0 o0 C2 v00 q0 y0 sp0 b1/0 m0/0/0 []\n",
            "--\n",
            "s5 t300 d0 n1 a2 f0 c4 r10 p0 k0 g0 o0 C2 v00 q0 y0 sp63.75 b1/0 m0/0/0 [25:1 26:0]\n",
            "s5 t250 d0 n1 a2 f0 c4 r10 p0 k0 g0 o0 C2 v00 q0 y0 sp0 b1/0 m0/0/0 []\n",
            "s5 t200 d0 n1 a2 f0 c4 r10 p0 k0 g0 o0 C2 v00 q0 y0 sp0 b1/0 m0/0/0 []\n",
        );
        const MELEE: &str = concat!(
            "s12 t300 d100 n0 a208 f0 c5 r10 p0 k0 g0 o0 C2 v00 q0 y0 sp255 b1/0 m0/0/0 [30:0]\n",
            "s12 t250 d50 n0 a208 f0 c5 r10 p0 k0 g0 o0 C2 v00 q0 y0 sp255 b1/0 m0/0/0 []\n",
            "s13 t200 d0 n0 a208 f0 c5 r10 p0 k0 g0 o0 C2 v00 q0 y0 sp255 b1/0 m0/0/0 [31:0]\n",
            "s13 t150 d0 n0 a208 f0 c5 r10 p0 k0 g0 o0 C2 v00 q0 y0 sp255 b1/0 m0/0/0 []\n",
            "s13 t100 d0 n0 a208 f0 c5 r10 p0 k0 g0 o0 C2 v00 q0 y0 sp255 b1/0 m0/0/0 []\n",
            "s13 t50 d0 n0 a208 f0 c5 r10 p0 k0 g0 o0 C2 v00 q0 y0 sp255 b1/0 m0/0/0 []\n",
            "s0 t0 d0 n0 a0 f0 c5 r10 p0 k0 g0 o0 C2 v00 q0 y0 sp255 b1/0 m0/0/0 []\n",
            "s0 t0 d0 n0 a0 f0 c5 r10 p0 k0 g0 o0 C2 v00 q0 y0 sp229.5 b1/0 m0/0/0 []\n",
            "--\n",
            "s12 t300 d100 n0 a208 f0 c5 r10 p0 k0 g0 o0 C2 v00 q0 y0 sp255 b1/0 m0/0/0 [30:0]\n",
            "s12 t250 d50 n0 a208 f0 c5 r10 p0 k0 g0 o0 C2 v00 q0 y0 sp255 b1/0 m0/0/0 []\n",
            "s13 t200 d0 n0 a208 f0 c5 r10 p0 k0 g0 o0 C2 v00 q0 y0 sp255 b1/0 m0/0/0 [31:0]\n",
            "s13 t150 d0 n0 a208 f0 c5 r10 p0 k0 g0 o0 C2 v00 q0 y0 sp255 b1/0 m0/0/0 []\n",
            "s13 t100 d0 n0 a208 f0 c5 r10 p0 k0 g0 o0 C2 v00 q0 y0 sp255 b1/0 m0/0/0 []\n",
            "s13 t50 d0 n0 a208 f0 c5 r10 p0 k0 g0 o0 C2 v00 q0 y0 sp255 b1/0 m0/0/0 []\n",
            "s14 t70 d0 n0 a14 f0 c5 r10 p0 k0 g0 o0 C2 v00 q0 y0 sp255 b1/0 m0/0/0 []\n",
            "s14 t20 d0 n0 a14 f0 c5 r10 p0 k0 g0 o0 C2 v00 q0 y0 sp255 b1/0 m0/0/0 []\n",
            "s0 t0 d0 n0 a200 f0 c5 r10 p0 k0 g0 o0 C2 v00 q0 y0 sp255 b1/0 m0/0/0 []\n",
            "s0 t0 d0 n0 a200 f0 c5 r10 p0 k0 g0 o0 C2 v00 q0 y0 sp229.5 b1/0 m0/0/0 []\n",
            "--\n",
            "s12 t300 d100 n0 a208 f0 c5 r10 p1 k0 g0 o0 C2 v01 q0 y0 sp255 b1/0 m0/0/0 [30:0]\n",
            "s12 t250 d50 n0 a208 f0 c5 r10 p1 k0 g0 o0 C2 v01 q0 y0 sp255 b1/0 m0/0/0 []\n",
            "s13 t200 d0 n0 a208 f0 c5 r10 p1 k0 g0 o0 C2 v01 q0 y0 sp255 b1/0 m0/0/0 [31:0]\n",
        );
        const CHARGE: &str = concat!(
            "s12 t500 d150 n0 a209 f0 c5 r10 p0 k0 g0 o0 C2 v00 q0 y0 sp255 b1/0 m1/90/90 [30:0]\n",
            "s12 t450 d100 n0 a209 f0 c5 r10 p0 k0 g0 o0 C2 v00 q0 y0 sp255 b1/0 m1/90/90 []\n",
            "s12 t400 d50 n0 a209 f0 c5 r10 p0 k0 g0 o0 C2 v00 q0 y0 sp255 b1/0 m1/90/90 []\n",
            "s13 t350 d0 n0 a209 f0 c5 r10 p0 k0 g0 o0 C2 v00 q0 y0 sp255 b1/0 m1/90/90 [31:0]\n",
            "s13 t300 d0 n0 a209 f0 c5 r10 p0 k0 g0 o0 C2 v00 q0 y0 sp255 b1/0 m1/90/90 []\n",
            "s13 t250 d0 n0 a209 f0 c5 r10 p0 k0 g0 o0 C2 v00 q0 y0 sp255 b1/0 m1/90/90 []\n",
            "s13 t200 d0 n0 a209 f0 c5 r10 p0 k0 g0 o0 C2 v00 q0 y0 sp255 b1/0 m1/90/90 []\n",
            "s13 t150 d0 n0 a209 f0 c5 r10 p0 k0 g0 o0 C2 v00 q0 y0 sp255 b1/0 m1/90/90 []\n",
            "s13 t100 d0 n0 a209 f0 c5 r10 p0 k0 g0 o0 C2 v00 q0 y0 sp255 b1/0 m1/90/90 []\n",
            "s13 t50 d0 n0 a209 f0 c5 r10 p0 k0 g0 o0 C2 v00 q0 y0 sp255 b1/0 m1/90/90 []\n",
            "s14 t70 d0 n0 a14 f0 c5 r10 p0 k0 g0 o0 C2 v00 q0 y0 sp255 b1/0 m1/90/90 []\n",
            "s14 t20 d0 n0 a14 f0 c5 r10 p0 k0 g0 o0 C2 v00 q0 y0 sp255 b1/0 m1/90/90 []\n",
            "s0 t0 d0 n0 a200 f0 c5 r10 p0 k0 g0 o0 C2 v00 q0 y0 sp255 b1/0 m1/90/90 []\n",
            "s0 t0 d0 n0 a200 f0 c5 r10 p0 k0 g0 o0 C2 v00 q0 y0 sp229.5 b1/0 m1/90/90 []\n",
            "--\n",
            "s12 t300 d100 n0 a208 f0 c5 r10 p0 k0 g0 o0 C2 v00 q0 y0 sp255 b1/0 m1/90/90 [30:0]\n",
            "s12 t250 d50 n0 a208 f0 c5 r10 p0 k0 g0 o0 C2 v00 q0 y0 sp255 b1/0 m1/90/90 []\n",
            "--\n",
            "s12 t300 d100 n0 a208 f0 c5 r10 p0 k0 g0 o0 C2 v00 q0 y0 sp255 b1/0 m0/0/0 [30:0]\n",
        );
        const NOMELEE: &str = concat!(
            "s12 t300 d100 n0 a208 f0 c5 r10 p0 k0 g0 o0 C2 v00 q0 y0 sp255 b1/0 m0/0/0 [30:0]\n",
            "s12 t250 d50 n0 a208 f0 c5 r10 p0 k0 g0 o0 C2 v00 q0 y0 sp255 b1/0 m0/0/0 []\n",
            "s13 t200 d0 n0 a208 f0 c5 r10 p0 k0 g0 o0 C2 v00 q0 y0 sp255 b1/0 m0/0/0 [31:0]\n",
            "--\n",
            "s0 t0 d0 n0 a0 f0 c5 r10 p0 k0 g0 o0 C2 v00 q0.000000011175871 y0 sp0 b1/0 m0/0/0 []\n",
            "s12 t300 d100 n0 a208 f0 c5 r10 p0 k0 g0 o0 C2 v00 q0 y0 sp255 b1/0 m0/0/0 [30:0]\n",
            "--\n",
            "s0 t0 d0 n0 a0 f0 c5 r10 p0 k0 g0 o0 C2 v00 q0 y0 sp0 b1/0 m0/0/0 []\n",
            "s12 t300 d100 n0 a208 f0 c5 r10 p0 k0 g0 o0 C2 v00 q0 y0 sp255 b1/0 m0/0/0 [30:0]\n",
            "--\n",
            "s0 t0 d0 n0 a0 f0 c5 r10 p0 k0 g0 o0 C2 v00 q0 y0 sp0 b1/0 m0/0/0 []\n",
            "s0 t0 d0 n0 a0 f0 c5 r10 p0 k0 g0 o0 C2 v00 q0 y0 sp0 b1/0 m0/0/0 []\n",
            "--\n",
            "s5 t80 d120 n1 a0 f0 c5 r10 p0 k280 g0 o0 C2 v00 q0 y0 sp0 b1/0 m0/0/0 []\n",
            "s5 t30 d70 n1 a0 f0 c5 r10 p0 k230 g0 o0 C2 v00 q0 y0 sp0 b1/0 m0/0/0 []\n",
            "--\n",
            "s0 t0 d0 n0 a0 f0 c5 r10 p0 k0 g0 o0 C2 v00 q0 y0 sp0 b1/0 m0/0/0 []\n",
            "s12 t300 d100 n0 a208 f0 c5 r10 p0 k0 g0 o0 C2 v00 q0 y0 sp255 b1/0 m0/0/0 [30:0]\n",
            "--\n",
            "s1 t150 d0 n0 a0 f0 c5 r10 p0 k0 g0 o0 C2 v00 q0 y0 sp0 b1/0 m0/0/0 []\n",
            "s1 t100 d0 n0 a0 f0 c5 r10 p0 k0 g0 o0 C2 v00 q0 y0 sp0 b1/0 m0/0/0 []\n",
        );
        const SIGHT: &str = concat!(
            "s0 t0 d0 n0 a0 f0 c5 r10 p0 k0 g0 o0 C2 v10 q0.33333334 y0 sp0 b1/0 m0/0/0 []\n",
            "s0 t0 d0 n0 a0 f0 c5 r10 p0 k0 g0 o0 C2 v10 q0.6666667 y0 sp0 b1/0 m0/0/0 []\n",
            "s0 t0 d0 n0 a0 f0 c5 r10 p0 k0 g0 o0 C2 v10 q1 y0 sp0 b1/0 m0/0/0 []\n",
            "s0 t0 d0 n0 a0 f0 c5 r10 p0 k0 g0 o0 C2 v10 q1 y0 sp0 b1/0 m0/0/0 []\n",
            "s0 t0 d0 n0 a0 f0 c5 r10 p0 k0 g0 o0 C2 v10 q1 y0 sp0 b1/0 m0/0/0 []\n",
            "s0 t0 d0 n0 a0 f0 c5 r10 p0 k0 g0 o0 C2 v00 q0.5 y0 sp0 b1/0 m0/0/0 []\n",
            "s0 t0 d0 n0 a0 f0 c5 r10 p0 k0 g0 o0 C2 v00 q0.000000011175871 y0 sp0 b1/0 m0/0/0 []\n",
            "s0 t0 d0 n0 a0 f0 c5 r10 p0 k0 g0 o0 C2 v00 q0 y0 sp0 b1/0 m0/0/0 []\n",
        );
        const SPRINTSIGHT: &str = concat!(
            "s0 t0 d0 n0 a0 f0 c5 r10 p0 k0 g0 o0 C2 v10 q0.33333334 y0 sp0 b1/0 m0/0/0 []\n",
            "s0 t0 d0 n0 a0 f0 c5 r10 p0 k0 g0 o0 C2 v10 q0.6666667 y0 sp0 b1/0 m0/0/0 []\n",
            "s0 t0 d0 n0 a0 f0 c5 r10 p0 k0 g0 o0 C2 v10 q1 y0 sp0 b1/0 m0/0/0 []\n",
            "s0 t0 d0 n0 a0 f0 c5 r10 p0 k0 g0 o0 C2 v10 q1 y0 sp0 b1/0 m0/0/0 []\n",
            "s0 t0 d0 n0 a0 f0 c5 r10 p0 k0 g0 o0 C2 v00 q0.5 y0 sp0 b1/0 m0/0/0 [14:0]\n",
            "s0 t0 d0 n0 a0 f0 c5 r10 p0 k0 g0 o0 C2 v00 q0.000000011175871 y0 sp0 b1/0 m0/0/0 [14:0]\n",
            "--\n",
            "s0 t0 d0 n0 a0 f0 c5 r10 p0 k0 g0 o0 C2 v10 q0.33333334 y0 sp0 b1/0 m0/0/0 []\n",
            "s0 t0 d0 n0 a0 f0 c5 r10 p0 k0 g0 o0 C2 v10 q0.6666667 y0 sp0 b1/0 m0/0/0 []\n",
            "s0 t0 d0 n0 a0 f0 c5 r10 p0 k0 g0 o0 C2 v10 q1 y0 sp0 b1/0 m0/0/0 []\n",
            "s0 t0 d0 n0 a0 f0 c5 r10 p0 k0 g0 o0 C2 v10 q1 y0 sp0 b1/0 m0/0/0 []\n",
            "s0 t0 d0 n0 a0 f4 c5 r10 p0 k0 g0 o0 C2 v10 q1 y0 sp0 b0.95/50 m0/0/0 []\n",
            "s0 t0 d0 n0 a0 f4 c5 r10 p0 k0 g0 o0 C2 v10 q1 y0 sp0 b0.9025/100 m0/0/0 []\n",
            "s0 t0 d0 n0 a0 f0 c5 r10 p0 k0 g0 o0 C2 v00 q0.5 y0 sp0 b0.9365227/50 m0/0/0 [14:0]\n",
        );
        const PRONESIGHT: &str = concat!(
            "s0 t0 d0 n0 a0 f0 c5 r10 p1 k0 g0 o0 C2 v11 q0.33333334 y0 sp255 b1/0 m0/0/0 []\n",
            "s0 t0 d0 n0 a0 f0 c5 r10 p1 k0 g0 o0 C2 v01 q0 y0 sp255 b1/0 m0/0/0 []\n",
            "s0 t0 d0 n0 a0 f0 c5 r10 p1 k0 g0 o0 C2 v01 q0 y0 sp255 b1/0 m0/0/0 []\n",
            "s0 t0 d0 n0 a0 f0 c5 r10 p1 k0 g0 o0 C2 v11 q0.33333334 y0 sp178.5 b1/0 m0/0/0 []\n",
            "--\n",
            "s0 t0 d0 n0 a0 f0 c5 r10 p0 k0 g0 o0 C2 v00 q0 y0 sp255 b1/0 m0/0/0 []\n",
            "s0 t0 d0 n0 a0 f0 c5 r10 p1 k0 g0 o0 C2 v11 q0.33333334 y0 sp255 b1/0 m0/0/0 []\n",
            "s0 t0 d0 n0 a0 f0 c5 r10 p1 k0 g0 o0 C2 v11 q0.6666667 y0 sp178.5 b1/0 m0/0/0 []\n",
            "--\n",
            "s0 t0 d0 n0 a0 f0 c5 r10 p0 k0 g0 o0 C2 v10 q0.5833333 y0 sp255 b1/0 m0/0/0 []\n",
            "s0 t0 d0 n0 a0 f0 c5 r10 p0 k0 g0 o0 C2 v10 q0.9166666 y0 sp255 b1/0 m0/0/0 []\n",
            "s0 t0 d0 n0 a0 f0 c5 r10 p0 k0 g0 o0 C2 v10 q1 y0 sp178.5 b1/0 m0/0/0 []\n",
        );
        const LOWERED: &str = concat!(
            "s4 t60 d0 n0 a213 f0 c5 r10 p0 k0 g0 o0 C2 v10 q1 y0 sp0 b1/0 m0/0/0 [23:0]\n",
            "s4 t10 d0 n0 a213 f0 c5 r10 p0 k0 g0 o0 C2 v00 q0.5 y0 sp0 b1/0 m0/0/0 []\n",
            "s1 t0 d0 n0 a16 f0 c5 r10 p0 k0 g0 o0 C2 v00 q0.000000011175871 y0 sp255 b1/0 m0/0/0 [22:0]\n",
            "s0 t0 d0 n0 a200 f0 c5 r10 p0 k0 g0 o0 C2 v00 q0 y0 sp0 b1/0 m0/0/0 []\n",
            "--\n",
            "s0 t0 d0 n0 a0 f0 c2 r10 p0 k0 g0 o0 C2 v10 q0.33333334 y0 sp0 b1/0 m0/0/0 []\n",
            "s0 t0 d0 n0 a0 f0 c2 r10 p0 k0 g0 o0 C2 v10 q0.6666667 y0 sp0 b1/0 m0/0/0 []\n",
            "s0 t0 d0 n0 a0 f0 c2 r10 p0 k0 g0 o0 C2 v10 q1 y0 sp0 b1/0 m0/0/0 []\n",
            "s0 t0 d0 n0 a0 f0 c2 r10 p0 k0 g0 o0 C2 v10 q1 y0 sp0 b1/0 m0/0/0 []\n",
            "s7 t300 d200 n0 a20d f0 c2 r10 p0 k0 g0 o0 C2 v10 q1 y0 sp0 b1/0 m0/0/0 [14:0 19:0 15:0]\n",
            "s7 t250 d150 n0 a20d f0 c2 r10 p0 k0 g0 o0 C2 v10 q0.5 y0 sp0 b1/0 m0/0/0 []\n",
            "s7 t200 d100 n0 a20d f0 c2 r10 p0 k0 g0 o0 C2 v10 q0.000000011175871 y0 sp0 b1/0 m0/0/0 []\n",
            "s7 t150 d50 n0 a20d f0 c2 r10 p0 k0 g0 o0 C2 v10 q0 y0 sp0 b1/0 m0/0/0 []\n",
            "s7 t100 d0 n0 a20d f0 c5 r7 p0 k0 g0 o0 C2 v10 q0 y0 sp0 b1/0 m0/0/0 [20:0]\n",
            "s7 t50 d0 n0 a20d f0 c5 r7 p0 k0 g0 o0 C2 v10 q0.33333334 y0 sp0 b1/0 m0/0/0 []\n",
            "s0 t0 d0 n0 a0 f0 c5 r7 p0 k0 g0 o0 C2 v10 q0.6666667 y0 sp0 b1/0 m0/0/0 []\n",
            "s0 t0 d0 n0 a0 f0 c5 r7 p0 k0 g0 o0 C2 v10 q1 y0 sp0 b1/0 m0/0/0 []\n",
            "s0 t0 d0 n0 a0 f0 c5 r7 p0 k0 g0 o0 C2 v10 q1 y0 sp0 b1/0 m0/0/0 []\n",
            "--\n",
            "s0 t0 d0 n0 a0 f0 c4 r10 p0 k0 g0 o0 C2 v10 q0.33333334 y0 sp0 b1/0 m0/0/0 []\n",
            "s0 t0 d0 n0 a0 f0 c4 r10 p0 k0 g0 o0 C2 v10 q0.6666667 y0 sp0 b1/0 m0/0/0 []\n",
            "s0 t0 d0 n0 a0 f0 c4 r10 p0 k0 g0 o0 C2 v10 q1 y0 sp0 b1/0 m0/0/0 []\n",
            "s0 t0 d0 n0 a0 f0 c4 r10 p0 k0 g0 o0 C2 v10 q1 y0 sp0 b1/0 m0/0/0 []\n",
            "s9 t200 d0 n0 a20f f0 c4 r10 p0 k0 g0 o0 C2 v10 q1 y0 sp0 b1/0 m0/0/0 [14:0 19:0 17:0]\n",
            "s9 t150 d0 n0 a20f f0 c4 r10 p0 k0 g0 o0 C2 v10 q0.5 y0 sp0 b1/0 m0/0/0 []\n",
            "s9 t100 d0 n0 a20f f0 c4 r10 p0 k0 g0 o0 C2 v10 q0.000000011175871 y0 sp0 b1/0 m0/0/0 []\n",
            "s9 t50 d0 n0 a20f f0 c4 r10 p0 k0 g0 o0 C2 v10 q0 y0 sp0 b1/0 m0/0/0 []\n",
            "s7 t150 d100 n0 ad f0 c4 r10 p0 k0 g0 o0 C2 v10 q0 y0 sp0 b1/0 m0/0/0 [15:0]\n",
            "s7 t100 d50 n0 ad f0 c4 r10 p0 k0 g0 o0 C2 v10 q0 y0 sp0 b1/0 m0/0/0 []\n",
            "s7 t50 d0 n0 ad f0 c5 r9 p0 k0 g0 o0 C2 v10 q0 y0 sp0 b1/0 m0/0/0 [20:0]\n",
            "s11 t150 d0 n0 a210 f0 c5 r9 p0 k0 g0 o0 C2 v10 q0 y0 sp0 b1/0 m0/0/0 [18:0]\n",
            "s11 t100 d0 n0 a210 f0 c5 r9 p0 k0 g0 o0 C2 v10 q0 y0 sp0 b1/0 m0/0/0 []\n",
            "s11 t50 d0 n0 a210 f0 c5 r9 p0 k0 g0 o0 C2 v10 q0.33333334 y0 sp0 b1/0 m0/0/0 []\n",
            "s0 t0 d0 n0 a0 f0 c5 r9 p0 k0 g0 o0 C2 v10 q0.6666667 y0 sp0 b1/0 m0/0/0 []\n",
            "--\n",
            "s0 t0 d0 n0 a0 f0 c5 r10 p0 k0 g0 o0 C2 v10 q0.33333334 y0 sp0 b1/0 m0/0/0 []\n",
            "s0 t0 d0 n0 a0 f0 c5 r10 p0 k0 g0 o0 C2 v10 q0.6666667 y0 sp0 b1/0 m0/0/0 []\n",
            "s0 t0 d0 n0 a0 f0 c5 r10 p0 k0 g0 o0 C2 v10 q1 y0 sp0 b1/0 m0/0/0 []\n",
            "s0 t0 d0 n0 a0 f0 c5 r10 p0 k0 g0 o0 C2 v10 q1 y0 sp0 b1/0 m0/0/0 []\n",
            "s5 t80 d0 n1 a205 f0 c4 r10 p0 k0 g0 o0 C2 v10 q1 y0 sp0 b1/0 m0/0/0 [26:0]\n",
            "s5 t30 d0 n1 a205 f0 c4 r10 p0 k0 g0 o0 C2 v10 q1 y0 sp0 b1/0 m0/0/0 []\n",
            "s0 t0 d0 n0 a0 f0 c4 r10 p0 k0 g0 o0 C2 v10 q1 y0 sp0 b1/0 m0/0/0 []\n",
            "s6 t200 d1 n0 a207 f0 c4 r10 p0 k0 g0 o0 C2 v10 q1 y0 sp0 b1/0 m0/0/0 [28:0]\n",
            "s6 t150 d0 n0 a207 f0 c4 r10 p0 k0 g0 o0 C2 v10 q0.5 y0 sp0 b1/0 m0/0/0 [29:0]\n",
            "s6 t100 d0 n0 a207 f0 c4 r10 p0 k0 g0 o0 C2 v10 q0.000000011175871 y0 sp0 b1/0 m0/0/0 []\n",
            "--\n",
            "s0 t0 d0 n0 a0 f0 c5 r10 p0 k0 g0 o0 C2 v10 q0.33333334 y0 sp0 b1/0 m0/0/0 []\n",
            "s0 t0 d0 n0 a0 f0 c5 r10 p0 k0 g0 o0 C2 v10 q0.6666667 y0 sp0 b1/0 m0/0/0 []\n",
            "s0 t0 d0 n0 a0 f0 c5 r10 p0 k0 g0 o0 C2 v10 q1 y0 sp0 b1/0 m0/0/0 []\n",
            "s0 t0 d0 n0 a0 f0 c5 r10 p0 k0 g0 o0 C2 v10 q1 y0 sp0 b1/0 m0/0/0 []\n",
            "s5 t80 d0 n1 a205 f0 c4 r10 p0 k0 g0 o0 C2 v10 q1 y0 sp0 b1/0 m0/0/0 [26:0]\n",
            "s5 t30 d0 n1 a205 f0 c4 r10 p0 k0 g0 o0 C2 v10 q1 y0 sp0 b1/0 m0/0/0 []\n",
            "s0 t0 d0 n0 a0 f0 c4 r10 p0 k0 g0 o0 C2 v10 q1 y0 sp0 b1/0 m0/0/0 []\n",
            "s6 t200 d1 n0 a207 f0 c4 r10 p0 k0 g0 o0 C2 v10 q1 y0 sp0 b1/0 m0/0/0 [28:0]\n",
            "s6 t150 d0 n0 a207 f0 c4 r10 p0 k0 g0 o0 C2 v10 q1 y0 sp0 b1/0 m0/0/0 [29:0]\n",
            "s6 t100 d0 n0 a207 f0 c4 r10 p0 k0 g0 o0 C2 v10 q1 y0 sp0 b1/0 m0/0/0 []\n",
            "--\n",
        );
        const BARREDSIGHT: &str = concat!(
            "s0 t0 d0 n0 a0 f0 c5 r10 p0 k0 g0 o0 C2 v00 q0 y0 sp0 b1/0 m0/0/0 []\n",
            "s0 t0 d0 n0 a0 f0 c5 r10 p0 k0 g0 o0 C2 v00 q0 y0 sp0 b1/0 m0/0/0 []\n",
            "--\n",
            "s0 t0 d0 n0 a0 f0 c5 r10 p0 k0 g0 o0 C2 v00 q0 y0 sp0 b1/0 m0/0/0 []\n",
            "s0 t0 d0 n0 a0 f0 c5 r10 p0 k0 g0 o0 C2 v00 q0 y0 sp0 b1/0 m0/0/0 []\n",
            "--\n",
            "s0 t0 d0 n0 a0 f20 c5 r10 p0 k0 g0 o0 C2 v00 q0 y0 sp0 b1/0 m0/0/0 []\n",
            "s0 t0 d0 n0 a0 f20 c5 r10 p0 k0 g0 o0 C2 v00 q0 y0 sp0 b1/0 m0/0/0 []\n",
            "--\n",
            "s0 t0 d0 n0 a0 f0 c0 r0 p0 k0 g0 o0 C2 v00 q0 y0 sp0 b1/0 m0/0/0 []\n",
            "s0 t0 d0 n0 a0 f0 c0 r0 p0 k0 g0 o0 C2 v00 q0 y0 sp0 b1/0 m0/0/0 []\n",
            "--\n",
            "s0 t0 d0 n0 a0 f0 c5 r10 p0 k0 g0 o0 C2 v00 q0.5 y0 sp0 b1/0 m0/0/0 []\n",
            "--\n",
            "s0 t0 d0 n0 a0 f0 c5 r10 p0 k0 g0 o0 C2 v00 q0 y0 sp0 b1/0 m0/0/0 []\n",
            "--\n",
            "s0 t0 d0 n0 a0 f0 c5 r10 p0 k0 g0 o0 C2 v00 q0 y0 sp0 b1/0 m0/0/0 []\n",
        );
        const SPREAD: &str = concat!(
            "s0 t0 d0 n0 a0 f0 c5 r10 p0 k0 g0 o0 C2 v00 q0 y0 sp174.5 b1/0 m0/0/0 []\n",
            "s0 t0 d0 n0 a0 f0 c5 r10 p0 k0 g0 o0 C2 v00 q0 y0 sp149 b1/0 m0/0/0 []\n",
            "s0 t0 d0 n0 a0 f0 c5 r10 p0 k0 g0 o0 C2 v00 q0 y0 sp161.75 b1/0 m0/0/0 []\n",
            "s0 t0 d0 n0 a0 f0 c5 r10 p0 k0 g0 o0 C2 v00 q0 y0 sp123.5 b1/0 m0/0/0 []\n",
            "s0 t0 d0 n0 a0 f0 c5 r10 p0 k0 g0 o0 C2 v00 q0 y0 sp123.5 b1/0 m0/0/0 []\n",
            "s0 t0 d0 n0 a0 f0 c5 r10 p0 k0 g0 o0 C2 v00 q0 y0 sp47 b1/0 m0/0/0 []\n",
            "--\n",
            "s0 t0 d0 n0 a0 f0 c5 r10 p0 k0 g0 o0 C2 v00 q0 y0 sp132.64 b1/0 m0/0/0 []\n",
            "s0 t0 d0 n0 a0 f0 c5 r10 p0 k0 g0 o0 C2 v00 q0 y0 sp165.28 b1/0 m0/0/0 []\n",
            "--\n",
            "s0 t0 d0 n0 a0 f0 c5 r10 p0 k0 g0 o0 C2 v00 q0 y0 sp54.58888 b1/0 m0/0/0 []\n",
            "s0 t0 d0 n0 a0 f0 c5 r10 p0 k0 g0 o0 C2 v00 q0 y0 sp59.177757 b1/0 m0/0/0 []\n",
            "s0 t0 d0 n0 a0 f0 c5 r10 p0 k0 g0 o0 C2 v00 q0 y0 sp63.766636 b1/0 m0/0/0 []\n",
            "--\n",
            "s0 t0 d0 n0 a0 f0 c5 r10 p0 k0 g0 o0 C2 v00 q0 y0 sp58.33403 b1/0 m0/0/0 []\n",
            "s0 t0 d0 n0 a0 f0 c5 r10 p0 k0 g0 o0 C2 v00 q0 y0 sp66.66806 b1/0 m0/0/0 []\n",
            "s0 t0 d0 n0 a0 f0 c5 r10 p0 k0 g0 o0 C2 v00 q0 y0 sp41.16806 b1/0 m0/0/0 []\n",
            "s0 t0 d0 n0 a0 f0 c5 r10 p0 k0 g0 o0 C2 v00 q0 y0 sp255 b1/0 m0/0/0 []\n",
            "--\n",
            "s0 t0 d0 n0 a0 f0 c5 r10 p0 k0 g0 o0 C2 v10 q1 y0 sp107.25 b1/0 m0/0/0 []\n",
            "s0 t0 d0 n0 a0 f0 c5 r10 p0 k0 g0 o0 C2 v00 q0 y0 sp141.16667 b1/0 m0/0/0 []\n",
            "s0 t0 d0 n0 a0 f0 c5 r10 p0 k0 g0 o0 C2 v00 q0 y0 sp115.66667 b1/0 m0/0/0 []\n",
            "s0 t0 d0 n0 a0 f0 c5 r10 p0 k0 g0 o0 C2 v00 q0 y0 sp90.16667 b1/0 m0/0/0 []\n",
            "s0 t0 d0 n0 a0 f0 c5 r10 p0 k0 g0 o0 C2 v00 q0 y0 sp64.66667 b1/0 m0/0/0 []\n",
            "s0 t0 d0 n0 a0 f0 c5 r10 p0 k0 g0 o0 C2 v00 q0 y0 sp39.16667 b1/0 m0/0/0 []\n",
            "s0 t0 d0 n0 a0 f0 c5 r10 p0 k0 g0 o0 C2 v00 q0 y0 sp13.666672 b1/0 m0/0/0 []\n",
            "s0 t0 d0 n0 a0 f0 c5 r10 p0 k0 g0 o0 C2 v00 q0 y0 sp0 b1/0 m0/0/0 []\n",
            "s0 t0 d0 n0 a0 f0 c5 r10 p0 k0 g0 o0 C2 v00 q0 y0 sp0 b1/0 m0/0/0 []\n",
            "--\n",
            "s0 t0 d0 n0 a0 f0 c5 r10 p0 k0 g0 o0 C2 v00 q0 y0 sp0 b1/0 m0/0/0 []\n",
            "s0 t0 d0 n0 a0 f0 c5 r10 p0 k0 g0 o0 C2 v00 q0 y0 sp0 b1/0 m0/0/0 []\n",
        );
        const BREATH: &str = concat!(
            "s0 t0 d0 n0 a0 f0 c5 r10 p0 k0 g0 o0 C2 v10 q1 y0 sp0 b1/0 m0/0/0 []\n",
            "s0 t0 d0 n0 a0 f4 c5 r10 p0 k0 g0 o0 C2 v10 q1 y0 sp0 b0.95/50 m0/0/0 []\n",
            "s0 t0 d0 n0 a0 f4 c5 r10 p0 k0 g0 o0 C2 v10 q1 y0 sp0 b0.9025/100 m0/0/0 []\n",
            "s0 t0 d0 n0 a0 f4 c5 r10 p0 k0 g0 o0 C2 v10 q1 y0 sp0 b0.85737497/150 m0/0/0 []\n",
            "s0 t0 d0 n0 a0 f4 c5 r10 p0 k0 g0 o0 C2 v10 q1 y0 sp0 b0.81450623/200 m0/0/0 []\n",
            "s0 t0 d0 n0 a0 f4 c5 r10 p0 k0 g0 o0 C2 v10 q1 y0 sp0 b0.77378094/250 m0/0/0 []\n",
            "s0 t0 d0 n0 a0 f4 c5 r10 p0 k0 g0 o0 C2 v10 q1 y0 sp0 b0.73509187/300 m0/0/0 []\n",
            "s0 t0 d0 n0 a0 f4 c5 r10 p0 k0 g0 o0 C2 v10 q1 y0 sp0 b0.69833726/350 m0/0/0 []\n",
            "s0 t0 d0 n0 a0 f4 c5 r10 p0 k0 g0 o0 C2 v10 q1 y0 sp0 b0.6634204/400 m0/0/0 []\n",
            "s0 t0 d0 n0 a0 f0 c5 r10 p0 k0 g0 o0 C2 v10 q1 y0 sp0 b1.8143942/700 m0/0/0 []\n",
            "s0 t0 d0 n0 a0 f0 c5 r10 p0 k0 g0 o0 C2 v10 q1 y0 sp0 b2.545076/650 m0/0/0 []\n",
            "s0 t0 d0 n0 a0 f0 c5 r10 p0 k0 g0 o0 C2 v10 q1 y0 sp0 b2.981553/600 m0/0/0 []\n",
            "s0 t0 d0 n0 a0 f0 c5 r10 p0 k0 g0 o0 C2 v10 q1 y0 sp0 b3.2120872/550 m0/0/0 []\n",
            "s0 t0 d0 n0 a0 f0 c5 r10 p0 k0 g0 o0 C2 v10 q1 y0 sp0 b3.298461/500 m0/0/0 []\n",
            "s0 t0 d0 n0 a0 f0 c5 r10 p0 k0 g0 o0 C2 v10 q1 y0 sp0 b3.2839227/450 m0/0/0 []\n",
            "s0 t0 d0 n0 a0 f0 c5 r10 p0 k0 g0 o0 C2 v10 q1 y0 sp0 b3.198746/400 m0/0/0 []\n",
            "s0 t0 d0 n0 a0 f0 c5 r10 p0 k0 g0 o0 C2 v10 q1 y0 sp0 b3.0641222/350 m0/0/0 []\n",
            "s0 t0 d0 n0 a0 f0 c5 r10 p0 k0 g0 o0 C2 v10 q1 y0 sp0 b2.8948855/300 m0/0/0 []\n",
            "s0 t0 d0 n0 a0 f0 c5 r10 p0 k0 g0 o0 C2 v10 q1 y0 sp0 b2.7014198/250 m0/0/0 []\n",
            "s0 t0 d0 n0 a0 f0 c5 r10 p0 k0 g0 o0 C2 v10 q1 y0 sp0 b2.490994/200 m0/0/0 []\n",
            "s0 t0 d0 n0 a0 f0 c5 r10 p0 k0 g0 o0 C2 v10 q1 y0 sp0 b2.2686958/150 m0/0/0 []\n",
            "--\n",
            "s0 t0 d0 n0 a0 f0 c5 r10 p0 k0 g0 o0 C2 v10 q1 y0 sp0 b1/0 m0/0/0 []\n",
            "s0 t0 d0 n0 a0 f4 c5 r10 p0 k0 g0 o0 C2 v10 q1 y0 sp0 b0.95/50 m0/0/0 []\n",
            "s0 t0 d0 n0 a0 f4 c5 r10 p0 k0 g0 o0 C2 v10 q1 y0 sp0 b0.9025/100 m0/0/0 []\n",
            "s0 t0 d0 n0 a0 f4 c5 r10 p0 k0 g0 o0 C2 v10 q1 y0 sp0 b0.85737497/150 m0/0/0 []\n",
            "s0 t0 d0 n0 a0 f0 c5 r10 p0 k0 g0 o0 C2 v10 q1 y0 sp0 b1.0501624/100 m0/0/0 []\n",
            "s0 t0 d0 n0 a0 f0 c5 r10 p0 k0 g0 o0 C2 v10 q1 y0 sp0 b1.1101137/50 m0/0/0 []\n",
            "s0 t0 d0 n0 a0 f0 c5 r10 p0 k0 g0 o0 C2 v10 q1 y0 sp0 b1.0770797/0 m0/0/0 []\n",
            "s0 t0 d0 n0 a0 f4 c5 r10 p0 k0 g0 o0 C2 v10 q1 y0 sp0 b1.0232257/50 m0/0/0 []\n",
            "s0 t0 d0 n0 a0 f4 c5 r10 p0 k0 g0 o0 C2 v10 q1 y0 sp0 b0.9720644/100 m0/0/0 []\n",
            "--\n",
            "s0 t0 d0 n0 a0 f0 c5 r10 p0 k0 g0 o0 C2 v10 q1 y0 sp0 b1/0 m0/0/0 []\n",
            "s0 t0 d0 n0 a0 f4 c5 r10 p0 k0 g0 o0 C2 v10 q1 y0 sp0 b0.95/50 m0/0/0 []\n",
            "s0 t0 d0 n0 a0 f4 c5 r10 p0 k0 g0 o0 C2 v10 q1 y0 sp0 b0.9025/100 m0/0/0 []\n",
            "s0 t0 d0 n0 a0 f4 c5 r10 p0 k0 g0 o0 C2 v10 q1 y0 sp0 b0.85737497/150 m0/0/0 []\n",
            "s0 t0 d0 n0 a0 f4 c5 r10 p0 k0 g0 o0 C2 v10 q1 y0 sp0 b0.81450623/200 m0/0/0 []\n",
            "s0 t0 d0 n0 a0 f4 c5 r10 p0 k0 g0 o0 C2 v10 q1 y0 sp0 b0.77378094/250 m0/0/0 []\n",
            "s0 t0 d0 n0 a0 f4 c5 r10 p0 k0 g0 o0 C2 v10 q1 y0 sp0 b0.73509187/300 m0/0/0 []\n",
            "s0 t0 d0 n0 a0 f4 c5 r10 p0 k0 g0 o0 C2 v10 q1 y0 sp0 b0.69833726/350 m0/0/0 []\n",
            "s0 t0 d0 n0 a0 f4 c5 r10 p0 k0 g0 o0 C2 v10 q1 y0 sp0 b0.6634204/400 m0/0/0 []\n",
            "s0 t0 d0 n0 a0 f4 c5 r10 p0 k0 g0 o0 C2 v10 q1 y0 sp0 b0.6302494/450 m0/0/0 []\n",
            "s0 t0 d0 n0 a0 f4 c5 r10 p0 k0 g0 o0 C2 v10 q1 y0 sp0 b0.5987369/500 m0/0/0 []\n",
            "--\n",
            "s0 t0 d0 n0 a0 f0 c5 r10 p0 k0 g0 o0 C2 v10 q0.33333334 y0 sp0 b1/0 m0/0/0 []\n",
            "s0 t0 d0 n0 a0 f0 c5 r10 p0 k0 g0 o0 C2 v10 q0.6666667 y0 sp0 b1/0 m0/0/0 []\n",
            "s0 t0 d0 n0 a0 f0 c5 r10 p0 k0 g0 o0 C2 v10 q1 y0 sp0 b1/0 m0/0/0 []\n",
            "s0 t0 d0 n0 a0 f0 c5 r10 p0 k0 g0 o0 C2 v10 q1 y0 sp0 b1/0 m0/0/0 []\n",
            "s0 t0 d0 n0 a0 f0 c5 r10 p0 k0 g0 o0 C2 v10 q1 y0 sp0 b1/0 m0/0/0 []\n",
            "s0 t0 d0 n0 a0 f0 c5 r10 p0 k0 g0 o0 C2 v10 q1 y0 sp0 b1/0 m0/0/0 []\n",
            "s0 t0 d0 n0 a0 f0 c5 r10 p0 k0 g0 o0 C2 v10 q1 y0 sp0 b1/0 m0/0/0 []\n",
            "s0 t0 d0 n0 a0 f0 c5 r10 p0 k0 g0 o0 C2 v10 q1 y0 sp0 b1/0 m0/0/0 []\n",
            "s0 t0 d0 n0 a0 f0 c5 r10 p0 k0 g0 o0 C2 v10 q1 y0 sp0 b1/0 m0/0/0 []\n",
            "s0 t0 d0 n0 a0 f0 c5 r10 p0 k0 g0 o0 C2 v10 q1 y0 sp0 b1/0 m0/0/0 []\n",
            "s0 t0 d0 n0 a0 f0 c5 r10 p0 k0 g0 o0 C2 v10 q1 y0 sp0 b1/0 m0/0/0 []\n",
        );
        const CHARGEOFF: &str = concat!(
            "1 70 241 Vec3(251.0, 145.0, 0.0)\n",
            "0 0 0 Vec3(58.0, 0.0, 0.0)\n",
            "0 0 0 Vec3(0.0, 0.0, 0.0)\n",
            "0 0 0 Vec3(0.0, 0.0, 0.0)\n",
            "0 0 0 Vec3(-0.0, 0.0, 0.0)\n",
        );
    }
}

/// The floor, with foliage filling `x` 100..300 for a contents-2 test.
struct Brush;

impl PmWorld for Brush {
    fn trace(&self, start: Vec3, end: Vec3, mins: Vec3, maxs: Vec3, pass: u32, mask: u32) -> Tr {
        if mask == 2 {
            let mut t = Tr::default();
            if (100.0..300.0).contains(&start.x) {
                t.fraction = 0.0;
                t.startsolid = true;
                t.contents = 2;
            }
            return t;
        }
        Floor.trace(start, end, mins, maxs, pass, mask)
    }
}

/// One scripted run: per cmd its buttons and forward key, 50 ms apart.
type Script = Vec<(u32, i8)>;

fn hold(n: usize, buttons: u32, forward: i8) -> Script {
    vec![(buttons, forward); n]
}

/// An FNV-1a digest of every field the frame's head, the sprint, the
/// foliage and the tail write, after each cmd of `script`.
fn digest(mut ps: PlayerState, script: &Script, rules: Rules, weapon: WeaponMove) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut eat = |v: u32| {
        for b in v.to_le_bytes() {
            h = (h ^ u64::from(b)).wrapping_mul(0x100_0000_01b3);
        }
    };
    let mut old = cmd(ps.command_time, 0, 0);
    for &(buttons, forward) in script {
        let mut c = cmd(ps.command_time + 50, buttons, forward);
        c.right_move = if buttons & 0x8_0000 != 0 { 64 } else { 0 };
        player_step(&mut ps, &c, &mut old, &Brush, rules, weapon);
        let s = ps.sprint;
        for v in [
            ps.pm_flags,
            ps.e_flags,
            ps.pm_time as u32,
            ps.event_sequence as u32,
            ps.events[0],
            ps.events[1],
            ps.events[2],
            ps.events[3],
            ps.foliage_sound_time as u32,
            s.last_start as u32,
            s.last_end as u32,
            s.start_max_length as u32,
            u32::from(s.delay) | u32::from(s.button_up_required) << 1,
            ps.ground_entity_num as u32,
            ps.view_height_target as u32,
        ] {
            eat(v);
        }
        for v in [ps.origin, ps.velocity] {
            for x in v.to_array() {
                eat(x.to_bits());
            }
        }
        eat(ps.old_velocity[0].to_bits());
        eat(ps.old_velocity[1].to_bits());
    }
    h
}

/// Pins `PmoveSingle`'s head, every arm's call order, the sprint and the
/// foliage sounds to the behaviour `cod4_moves_ab` left them at; no
/// capture sprints yet (cod4-pmove.md 11).
#[test]
fn pmove_single_and_the_sprint_hold_their_pinned_traces() {
    const SPRINT: u32 = button::SPRINT;
    let cat = |parts: &[Script]| parts.concat();
    let flagged = |flags: u32| {
        let mut ps = standing();
        ps.pm_flags |= flags;
        ps
    };
    let typed = |t: i32| {
        let mut ps = standing();
        ps.pm_type = t;
        ps
    };
    let state = |w: i32| {
        let mut ps = standing();
        ps.weapon_state = w;
        ps
    };
    let longer = {
        let mut ps = standing();
        ps.perks = 0x400;
        ps
    };
    let ducked = {
        let mut ps = standing();
        ps.pm_flags |= pmf::DUCKED;
        ps.view_height_target = VIEW_CROUCH;
        ps
    };
    let turret = {
        let mut ps = standing();
        ps.e_flags |= ef::TURRET;
        ps
    };
    let pause = Rules {
        sprint_recharge_pause: 1.5,
        sprint_min_time: 0.5,
        ..Rules::default()
    };
    let half = WeaponMove {
        sprint_duration_scale: 0.5,
        ..RIFLE
    };
    let runout = cat(&[
        hold(100, SPRINT, 127),
        hold(4, 0, 127),
        hold(60, SPRINT, 127),
    ]);
    let repress = cat(&[
        hold(10, SPRINT, 127),
        hold(3, 0, 127),
        hold(5, SPRINT, 127),
        hold(2, 0, 127),
        hold(10, SPRINT, 127),
    ]);
    let enders = cat(&[
        hold(6, SPRINT, 127),
        hold(4, SPRINT, 100),
        hold(2, 0, 127),
        hold(6, SPRINT, 127),
        hold(4, SPRINT | button::LEAN_LEFT, 127),
        hold(2, 0, 127),
        hold(6, SPRINT, 127),
        hold(4, SPRINT | button::CROUCH, 127),
        hold(2, 0, 127),
        hold(6, SPRINT, 127),
        hold(4, SPRINT | button::ADS_MODE, 127),
        hold(4, SPRINT | button::JUMP, 127),
        hold(20, SPRINT, 127),
    ]);
    let heads = cat(&[
        hold(4, button::TALK | SPRINT | button::ATTACK, 127),
        hold(4, button::ATTACK | 0x8_0000, -127),
        hold(4, 0x101, 0),
        hold(4, 0, 127),
        hold(4, 0x8_0000, 0),
    ]);
    let walk = cat(&[hold(30, 0, 127), hold(30, 0, 40), hold(30, SPRINT, 127)]);
    let cases: Vec<(&str, PlayerState, Script, Rules, WeaponMove, u64)> = vec![
        (
            "runout",
            standing(),
            runout.clone(),
            Rules::default(),
            RIFLE,
            0xb49a3bb4b6b82e26,
        ),
        (
            "pause",
            standing(),
            runout.clone(),
            pause,
            RIFLE,
            0x1372f82bddb58844,
        ),
        (
            "perk",
            longer,
            runout.clone(),
            Rules::default(),
            RIFLE,
            0x1d3074ba34b90b5c,
        ),
        (
            "half",
            standing(),
            runout.clone(),
            Rules::default(),
            half,
            0xfaf70c1b424b2229,
        ),
        (
            "unarmed",
            standing(),
            runout,
            Rules::default(),
            WeaponMove::NONE,
            0x5d4b7ea3a4f3a6d3,
        ),
        (
            "repress",
            standing(),
            repress,
            Rules::default(),
            RIFLE,
            0xe7f1431cf4087ace,
        ),
        (
            "enders",
            standing(),
            enders.clone(),
            Rules::default(),
            RIFLE,
            0x9f7492951d85040c,
        ),
        (
            "ducked",
            ducked,
            cat(&[hold(6, SPRINT | button::CROUCH, 127), enders.clone()]),
            Rules::default(),
            RIFLE,
            0xdd9fe14069cc0108,
        ),
        (
            "offhand",
            state(15),
            enders.clone(),
            Rules::default(),
            RIFLE,
            0xe3fd63f74f93e46f,
        ),
        (
            "state25",
            state(25),
            enders.clone(),
            Rules::default(),
            RIFLE,
            0x7abe3054bc7f4443,
        ),
        (
            "nosprint",
            flagged(pmf::NO_SPRINT),
            enders.clone(),
            Rules::default(),
            RIFLE,
            0x8783e031a2a26903,
        ),
        (
            "shocked",
            flagged(pmf::SHELLSHOCKED),
            enders.clone(),
            Rules::default(),
            RIFLE,
            0xdbbb762a79e4533f,
        ),
        (
            "frozen",
            flagged(pmf::FROZEN),
            heads.clone(),
            Rules::default(),
            RIFLE,
            0xf3a0114114a260bd,
        ),
        (
            "respawned",
            flagged(pmf::RESPAWNED),
            heads.clone(),
            Rules::default(),
            RIFLE,
            0x6f52e5bf45c460ae,
        ),
        (
            "sighted",
            flagged(pmf::SIGHT),
            heads.clone(),
            Rules::default(),
            RIFLE,
            0x71740e6e360af1ce,
        ),
        (
            "prone",
            flagged(pmf::PRONE | pmf::PRONE_STILL),
            heads.clone(),
            Rules::default(),
            RIFLE,
            0x948afac463ce8e6e,
        ),
        (
            "mantle",
            flagged(pmf::MANTLE),
            heads.clone(),
            Rules::default(),
            RIFLE,
            0x5fab39f02b012815,
        ),
        (
            "turret",
            turret,
            heads.clone(),
            Rules::default(),
            RIFLE,
            0xaba2811e36c000fd,
        ),
        (
            "linked",
            typed(pm_type::NORMAL_LINKED),
            heads.clone(),
            Rules::default(),
            RIFLE,
            0x2c3c0110f1406ca5,
        ),
        (
            "noclip",
            typed(pm_type::NOCLIP),
            heads.clone(),
            Rules::default(),
            RIFLE,
            0x27741bc92c33ee77,
        ),
        (
            "ufo",
            typed(pm_type::UFO),
            heads.clone(),
            Rules::default(),
            RIFLE,
            0xda4cbda58097b422,
        ),
        (
            "spectator",
            typed(pm_type::SPECTATOR),
            heads.clone(),
            Rules::default(),
            RIFLE,
            0x08bc99b4437c11bf,
        ),
        (
            "intermission",
            typed(pm_type::INTERMISSION),
            heads.clone(),
            Rules::default(),
            RIFLE,
            0xca2b79b70b6f52bd,
        ),
        (
            "laststand",
            typed(pm_type::LASTSTAND),
            heads.clone(),
            Rules::default(),
            RIFLE,
            0x2b78d6efe60fadfd,
        ),
        (
            "dead",
            typed(pm_type::DEAD),
            heads.clone(),
            Rules::default(),
            RIFLE,
            0xf54f452db9bae39d,
        ),
        (
            "deadlinked",
            typed(pm_type::DEAD_LINKED),
            heads,
            Rules::default(),
            RIFLE,
            0x2c3c0110f1406ca5,
        ),
        (
            "foliage",
            standing(),
            walk,
            Rules::default(),
            RIFLE,
            0xb0dea394c7e90443,
        ),
    ];
    let mut wrong = Vec::new();
    for (name, ps, script, rules, weapon, want) in cases {
        let got = digest(ps, &script, rules, weapon);
        if got != want {
            wrong.push(format!("(\"{name}\", {got:#018x})"));
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}
