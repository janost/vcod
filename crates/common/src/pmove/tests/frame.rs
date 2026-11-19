//! `PmoveSingle`'s arms, the knockback timer and the default arm's tail.

use super::*;

/// `PM_DropTimers` zeroes the timer once the frame's ms reach it, and
/// otherwise subtracts.
#[test]
fn knockback_runs_out() {
    let w = flat();
    let mut ps = PlayerState::spawn(Vec3::ZERO, 0.0);
    ps.knockback_ms = 20.0;
    pmove(
        &mut ps,
        &PmInput::default(),
        &MoveWorld::bare(&w),
        0.016,
        &[],
    );
    assert_eq!(ps.knockback_ms, 4.0);
    pmove(
        &mut ps,
        &PmInput::default(),
        &MoveWorld::bare(&w),
        0.016,
        &[],
    );
    assert_eq!(ps.knockback_ms, 0.0);
}

/// `PM_DropTimers` runs from `PmoveSingle` for every `pm_type`, so a
/// linked player (an S&D planter) still drops the timer even though
/// `linked_move` skips the move, ground and weapon dispatch.
#[test]
fn a_linked_player_still_drops_knockback() {
    let w = flat();
    let mut ps = PlayerState::spawn(Vec3::ZERO, 0.0);
    ps.linked = true;
    ps.knockback_ms = 20.0;
    pmove(
        &mut ps,
        &PmInput::default(),
        &MoveWorld::bare(&w),
        0.016,
        &[],
    );
    assert_eq!(ps.knockback_ms, 4.0);
    pmove(
        &mut ps,
        &PmInput::default(),
        &MoveWorld::bare(&w),
        0.016,
        &[],
    );
    assert_eq!(ps.knockback_ms, 0.0);
}

/// `PmoveSingle` clears `pm_flags` 0x8000 at its top whatever the
/// pm_type (0x33e5a): a dead, a dead linked and a spectator move each
/// end with the refused prone press gone (BUG-132).
#[test]
fn every_move_arm_clears_the_prone_blocked_flag() {
    let w = flat();
    let w = MoveWorld::bare(&w);
    for arm in 0..3 {
        let mut ps = PlayerState::spawn(Vec3::ZERO, 0.0);
        ps.prone_blocked = true;
        ps.prone_flip = true;
        match arm {
            0 => drop(dead_move(&mut ps, &w, 0.05)),
            1 => dead_linked_move(&mut ps, &w, 0.05),
            _ => drop(spectator_move(&mut ps, Some(&w), 0.0, 0.0, 0.0, 0, 0.05)),
        }
        assert!(!ps.prone_blocked, "arm {arm}");
        assert!(!ps.prone_flip, "arm {arm}");
    }
}

/// A death takes the sight down: `PmoveSingle`'s dead arm calls
/// `PM_ClearAimDownSightFlag` and nothing else about the weapon runs, so
/// the fraction stays where the death froze it
/// (docs/research/cod11-combat.md, 1.13).
#[test]
fn a_death_clears_the_ads_flag() {
    let w = flat();
    let w = MoveWorld::bare(&w);
    let mut ps = PlayerState::spawn(Vec3::ZERO, 0.0);
    ps.ads_active = true;
    ps.weapon_pos_frac = 1.0;
    dead_move(&mut ps, &w, 0.05);
    assert!(!ps.ads_active);
    assert_eq!(ps.weapon_pos_frac, 1.0);
}

/// `PM_DeadMove` takes 20 off a grounded corpse's speed every frame on
/// top of the friction, so the same knockback slides it a fraction of
/// what it slides a live player: the retail probe measured 25 units
/// against 60 for a flat 200 (docs/research/cod11-combat.md 5.7). The
/// drop is per frame, so halving the frame length shortens the slide.
#[test]
fn a_corpse_sheds_twenty_units_a_frame_on_the_ground() {
    let w = flat();
    let w = MoveWorld::bare(&w);
    let slide = |dt: f32| {
        let mut ps = PlayerState::spawn(Vec3::new(0.0, 0.0, 0.125), 0.0);
        ps.velocity = Vec3::new(240.0, 0.0, 0.0);
        ps.knockback_ms = 120.0;
        ps.knockback_flags = PMF_TIME_KNOCKBACK;
        for _ in 0..(2.0 / dt) as usize {
            dead_move(&mut ps, &w, dt);
        }
        assert_eq!(ps.velocity, Vec3::ZERO);
        ps.origin.x
    };
    let mut ps = PlayerState::spawn(Vec3::new(0.0, 0.0, 0.125), 0.0);
    ps.velocity = Vec3::new(10.0, 0.0, 0.0);
    dead_move(&mut ps, &w, 0.016);
    assert_eq!(ps.velocity, Vec3::ZERO, "10 is under one frame's drop");
    let (coarse, fine) = (slide(0.033), slide(0.016));
    assert!((20.0..35.0).contains(&coarse), "{coarse}");
    assert!(fine < coarse - 5.0, "{fine} against {coarse}");
}

/// The mounted arm (0x34274) never reaches the move dispatch or
/// `PM_Weapon`, so a mounted player's origin holds and no fire event
/// comes out of pmove (docs/research/cod11-turrets.md, section 5).
#[test]
fn a_mounted_player_does_not_move_or_fire() {
    let world = flat();
    let world = MoveWorld::bare(&world);
    let mut ps = PlayerState::spawn(Vec3::new(0.0, 0.0, 0.0), 0.0);
    ps.mounted = Some(Stance::Stand);
    let input = PmInput {
        forward: 127.0,
        attack: true,
        ..Default::default()
    };
    let before = ps.origin;
    let events = pmove(&mut ps, &input, &world, 0.05, &[]);
    assert_eq!(ps.origin, before);
    assert!(!ps.on_ground, "groundEntityNum NONE while mounted");
    assert!(
        events.iter().all(
            |e| e.event != weapon::EV_FIRE_WEAPON && e.event != weapon::EV_FIRE_WEAPON_LASTSHOT
        )
    );
}

/// The stance step (0x316f4) puts the player at the gun's stance
/// whatever the cmd asks, and `PM_UpdateLean` forces the lean input to 0
/// (docs/research/cod11-turrets.md, section 5).
#[test]
fn a_mounted_player_takes_the_gun_stance_whatever_the_cmd_asks() {
    let world = flat();
    let world = MoveWorld::bare(&world);
    let mut ps = PlayerState::spawn(Vec3::ZERO, 0.0);
    ps.mounted = Some(Stance::Prone);
    let input = PmInput {
        crouch: true,
        ..Default::default()
    };
    pmove(&mut ps, &input, &world, 0.05, &[]);
    assert_eq!(ps.stance, Stance::Prone);
    assert_eq!(ps.lean, 0.0);
}

/// 0x34274 calls no footstep routine, so a gunner mounted off a ladder
/// neither steps nor stays on it.
#[test]
fn a_mounted_player_takes_no_step_and_leaves_the_ladder() {
    let world = flat();
    let world = MoveWorld::bare(&world);
    let mut ps = PlayerState::spawn(Vec3::ZERO, 0.0);
    ps.mounted = Some(Stance::Stand);
    ps.on_ladder = true;
    ps.since_jump_ms = 10_000.0;
    ps.velocity = Vec3::new(0.0, 0.0, 200.0);
    let before = ps.bob_cycle;
    let events = pmove(&mut ps, &PmInput::default(), &world, 0.05, &[]);
    assert!(!ps.on_ladder);
    assert_eq!(ps.bob_cycle, before);
    assert!(events.is_empty(), "{events:?}");
}

/// The mounted arm runs no ground trace (turrets doc 5), so a gunner who
/// mounted off a steep face keeps no plane: it does not read as sliding,
/// and 1.5's near flag does not pass the sight.
#[test]
fn a_mounted_player_keeps_no_ground_plane() {
    let world = flat();
    let world = MoveWorld::bare(&world);
    for rules in [MoveRules::V1_1, MoveRules::V1_5] {
        let mut ps = PlayerState::spawn(Vec3::ZERO, 0.0);
        ps.rules = rules;
        ps.mounted = Some(Stance::Stand);
        ps.on_ground = false;
        ps.ground_plane = Some(Vec3::new(0.8, 0.0, 0.6));
        ps.near_ground = true;
        pmove(&mut ps, &PmInput::default(), &world, 0.05, &[]);
        assert!(!ps.sliding(), "{rules:?}");
        assert!(!ps.sight_ground(), "{rules:?}");
    }
}

#[test]
fn the_velocity_snap_rounds_to_nearest_even_and_stores_plus_zero() {
    let mut ps = PlayerState::spawn(Vec3::ZERO, 0.0);
    ps.velocity = Vec3::new(183.5, 26.5, -0.4);
    snap_velocity(&mut ps);
    assert_eq!(ps.velocity, Vec3::new(184.0, 26.0, 0.0));
    assert_eq!(ps.velocity.z.to_bits(), 0, "a snapped -0.4 is +0.0");
    ps.velocity = Vec3::new(-2.5, 137.6, -196.2);
    snap_velocity(&mut ps);
    assert_eq!(ps.velocity, Vec3::new(-2.0, 138.0, -196.0));
}

#[test]
fn a_frame_that_moved_under_half_its_velocity_takes_the_move_as_velocity() {
    let world = crate::collision::synthetic_world(
        &[("textures/test/solid", crate::collision::CONTENTS_SOLID, 0)],
        &[(0, [-64.0, -64.0, -1100.0], [64.0, 64.0, -1000.0])],
    );
    let w = MoveWorld::bare(&world);
    let mut ps = PlayerState::spawn(Vec3::new(1.0, 0.0, 0.0), 0.0);
    ps.move_start = Vec3::ZERO;
    // 1 unit in 8 ms is 125 u/s: 200 is under twice that and stays.
    ps.velocity = Vec3::new(200.0, 0.0, 0.0);
    clamp_velocity_to_move(&mut ps, &w, 0.008);
    assert_eq!(ps.velocity.x, 200.0);
    ps.velocity = Vec3::new(300.0, 0.0, 40.0);
    clamp_velocity_to_move(&mut ps, &w, 0.008);
    assert!(ps.velocity.abs_diff_eq(Vec3::new(125.0, 0.0, 0.0), 1e-3));
}

/// A fall from rest moves exactly half its end velocity, and 1.1d's
/// stack keeps the tie or takes the move by the origin's last bits:
/// `probe_spawnview`'s 66 + 34 ms spawn thinks on mp_carentan
/// (cod11-gametypes-re-bel.md 8.5) fell 3.0888 from these heights and
/// 4.0068 from the rest, and 1.5b's stored floats took it only at -2
/// and ended the fall from 4 at -0.00680161 (cod15-movement.md 7); the
/// Windows clients fall as 1.1d on every patch (8).
#[test]
fn a_fall_from_rest_ties_the_tail_by_the_start_height() {
    let world = crate::collision::synthetic_world(
        &[("textures/test/solid", crate::collision::CONTENTS_SOLID, 0)],
        &[(0, [-64.0, -64.0, -1100.0], [64.0, 64.0, -1000.0])],
    );
    let w = MoveWorld::bare(&world);
    let end_as = |z: f32, rules: MoveRules, arith: crate::patch::Arith| {
        world.set_arith(arith);
        let mut ps = PlayerState::spawn(Vec3::new(0.0, 0.0, z), 0.0);
        ps.rules = rules;
        pmove(&mut ps, &PmInput::default(), &w, 0.066, &[]);
        pmove(&mut ps, &PmInput::default(), &w, 0.034, &[]);
        ps.origin.z
    };
    let end = |z: f32, rules: MoveRules| {
        let linux = if rules.v15() {
            crate::patch::Arith::Stored
        } else {
            crate::patch::Arith::Kept
        };
        end_as(z, rules, linux)
    };
    let fall = |z: f32, rules: MoveRules| ((z - end(z, rules)) * 10000.0).round() / 10000.0;
    // The Windows client stores only y and z of the tail and keeps the
    // trace's end: on every patch it falls as 1.1d does
    // (cod15-movement.md 8).
    for rules in [MoveRules::V1_1, MoveRules::V1_5, MoveRules::Uo] {
        let msvc = |z: f32| {
            let e = end_as(z, rules, crate::patch::Arith::Msvc);
            ((z - e) * 10000.0).round() / 10000.0
        };
        for z in [-2.0, -1.0, 0.0, 1.0, 2.0, 4.0] {
            assert_eq!(msvc(z), 3.0888, "{rules:?} on Windows from {z}");
        }
        for z in [-4.0, 8.0, 16.0, 100.0] {
            assert_eq!(msvc(z), 4.0068, "{rules:?} on Windows from {z}");
        }
    }
    for z in [-2.0, -1.0, 0.0, 1.0, 2.0, 4.0] {
        assert_eq!(fall(z, MoveRules::V1_1), 3.0888, "1.1 from {z}");
    }
    for z in [-4.0, 8.0, 16.0, 100.0] {
        assert_eq!(fall(z, MoveRules::V1_1), 4.0068, "1.1 from {z}");
    }
    for z in [-4.0, -1.0, 0.0, 1.0, 2.0, 4.0, 8.0, 16.0, 100.0] {
        assert_eq!(fall(z, MoveRules::V1_5), 4.0068, "1.5 from {z}");
    }
    assert_eq!(fall(-2.0, MoveRules::V1_5), 3.0888);
    assert_eq!((end(4.0, MoveRules::V1_5) * 1e8).round(), -680161.0);
}
