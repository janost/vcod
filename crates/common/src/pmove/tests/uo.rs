//! United Offensive's sprint, sight and bipod.

use super::*;

/// UO's sprint (docs/research/coduo-movement.md, 2-4): 1.6 times the
/// run, a full bar spent in 3 s with the wind under half, nothing back
/// for a second after, then the whole bar in 7 s and the wind gone.
#[test]
fn a_uo_sprint_runs_fast_and_tires() {
    let w = flat();
    let w = MoveWorld::bare(&w);
    let mut ps = PlayerState {
        rules: MoveRules::Uo,
        ..PlayerState::spawn(Vec3::ZERO, 0.0)
    };
    let mut t = 0;
    let mut run = |ps: &mut PlayerState, input: PmInput, ms: i32| {
        for _ in 0..ms / 8 {
            t += 8;
            pmove(
                ps,
                &PmInput {
                    server_time: t,
                    ..input
                },
                &w,
                0.008,
                &[],
            );
        }
    };
    let sprint = PmInput {
        forward: 1.0,
        sprint: true,
        ..Default::default()
    };
    run(&mut ps, PmInput::default(), 240);
    run(&mut ps, sprint, 1000);
    assert!(ps.sprinting);
    let speed = ps.velocity.truncate().length();
    assert!(speed == 304.0, "{speed}");
    run(&mut ps, sprint, 600);
    assert!(ps.winded && ps.fatigue < 0.5);
    run(&mut ps, sprint, 1500);
    assert!(!ps.sprinting && ps.fatigue == 0.0);
    let mut rested = ps;
    run(&mut rested, PmInput::default(), 900);
    assert_eq!(rested.fatigue, 0.0, "the regeneration delay");
    // Held on an empty bar: no sprint and no refill, though the delay
    // runs from the last sprinting cmd.
    run(&mut ps, sprint, 2000);
    assert_eq!(ps.fatigue, 0.0);
    run(&mut ps, PmInput::default(), 3500);
    assert!(ps.winded && (ps.fatigue - 0.5).abs() < 0.01);
    run(&mut ps, PmInput::default(), 3600);
    assert!(!ps.winded && ps.fatigue == 1.0);
    // `bg_nofatigue` 1: the sprint runs on a full bar (0x2aa63).
    let mut fresh = PlayerState {
        rules: MoveRules::Uo,
        ..PlayerState::spawn(Vec3::ZERO, 0.0)
    };
    fresh.bg.nofatigue = true;
    run(&mut fresh, PmInput::default(), 240);
    run(&mut fresh, sprint, 1000);
    assert!(fresh.sprinting && fresh.fatigue == 1.0);
}

/// Every patch drops the moves of a sighted prone body before the
/// move (1.1 0x33f48 on `pm_flags` 0x21, 1.5 0x281ae on the effective
/// stance); a crouched or unsighted one keeps them.
#[test]
fn a_sighted_prone_body_holds_still_on_every_patch() {
    let moving = PmInput {
        forward: 1.0,
        right: -1.0,
        ..Default::default()
    };
    for rules in [MoveRules::V1_1, MoveRules::V1_5, MoveRules::Uo] {
        let mut ps = PlayerState::spawn(Vec3::ZERO, 0.0);
        ps.rules = rules;
        ps.stance = Stance::Prone;
        ps.view_target = VIEW_PRONE;
        ps.ads_active = true;
        let held = hold_still_sighted(&ps, &moving);
        assert_eq!((held.forward, held.right), (0.0, 0.0), "{rules:?}");
        ps.ads_active = false;
        assert_eq!(hold_still_sighted(&ps, &moving).forward, 1.0, "{rules:?}");
        ps.ads_active = true;
        ps.stance = Stance::Crouch;
        ps.view_target = VIEW_CROUCH;
        assert_eq!(hold_still_sighted(&ps, &moving).forward, 1.0, "{rules:?}");
    }
}

/// The sight off the ground (cod15-movement.md 5): 1.5 and UO raise it
/// on a body falling within a unit of the floor, `PM_GroundTraceMissed`'s
/// `almostGroundPlane`; 1.1 and a body higher up wait for the landing.
#[test]
fn a_15_sight_starts_a_unit_over_the_floor() {
    let w = flat();
    let w = MoveWorld::bare(&w);
    let rifle = WeaponDef {
        aim_down_sight: true,
        ads_trans_in: 0.3,
        ads_trans_out: 0.3,
        clip_size: 5,
        ammo_index: 1,
        clip_index: 1,
        ..WeaponDef::default()
    };
    let weapons = [None, Some(rifle)];
    let sight = PmInput {
        ads: true,
        weapon: 1,
        ..Default::default()
    };
    let first_cmd = |rules: MoveRules, z: f32| {
        let mut ps = PlayerState {
            rules,
            ..PlayerState::spawn(Vec3::new(0.0, 0.0, z), 0.0)
        };
        weapon::give(&mut ps, 1, 1);
        ps.weapon = 1;
        ps.ammoclip[1] = 5;
        ps.on_ground = false;
        pmove(&mut ps, &sight, &w, 0.016, &weapons);
        ps.ads_active
    };
    for rules in [MoveRules::V1_5, MoveRules::Uo] {
        assert!(first_cmd(rules, 0.63), "{rules:?}");
        assert!(!first_cmd(rules, 1.5), "{rules:?}");
    }
    assert!(!first_cmd(MoveRules::V1_1, 0.63));
}

/// UO's bipod from prone (coduo-movement.md 7): the sight goes down in
/// `adsTransInTime`, the body takes no move and its view no more than
/// 55 degrees off `proneDirection`, and letting go breaks it down in
/// `adsTransOutTime` at the walk.
#[test]
fn a_uo_bipod_deploys_prone_and_holds_the_view() {
    let w = flat();
    let w = MoveWorld::bare(&w);
    let lmg = WeaponDef {
        weapon_class: "lmg".into(),
        aim_down_sight: true,
        ads_trans_in: 1.8,
        ads_trans_out: 1.9,
        move_speed_scale: 1.1,
        clip_size: 100,
        ammo_index: 1,
        clip_index: 1,
        ..WeaponDef::default()
    };
    let weapons = [None, Some(lmg)];
    let mut ps = PlayerState {
        rules: MoveRules::Uo,
        ..PlayerState::spawn(Vec3::ZERO, 0.0)
    };
    weapon::give(&mut ps, 1, 1);
    ps.weapon = 1;
    ps.ammoclip[1] = 100;
    let tick = |ps: &mut PlayerState, input: &PmInput, n: usize| {
        for _ in 0..n {
            pmove(ps, input, &w, 0.008, &weapons);
        }
    };
    let prone = PmInput {
        prone: true,
        weapon: 1,
        ..Default::default()
    };
    tick(&mut ps, &prone, 125);
    assert_eq!(ps.stance, Stance::Prone);
    let sight = PmInput { ads: true, ..prone };
    tick(&mut ps, &sight, 1);
    assert!(ps.deployed());
    assert_eq!(ps.weaponstate, weapon::WEAPON_DEPLOY);
    assert_eq!(ps.weapon_time_ms, 1799);
    tick(&mut ps, &sight, 230);
    assert_eq!(ps.weaponstate, weapon::WEAPON_READY);
    assert_eq!(ps.weapon_pos_frac, 1.0);
    let at = ps.origin;
    tick(
        &mut ps,
        &PmInput {
            forward: 1.0,
            ..sight
        },
        60,
    );
    assert_eq!(ps.origin.truncate(), at.truncate());
    ps.yaw = 80f32.to_radians();
    tick(&mut ps, &sight, 1);
    assert!(
        (ps.yaw.to_degrees() - LMG_YAWCAP).abs() < 0.1,
        "{}",
        ps.yaw.to_degrees()
    );
    assert_eq!(ps.prone_direction, 0.0, "a deployed body does not swing");
    tick(&mut ps, &prone, 1);
    assert_eq!(ps.weaponstate, weapon::WEAPON_BREAKDOWN);
    assert_eq!(ps.weapon_time_ms, 1899);
}
