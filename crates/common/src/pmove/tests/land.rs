//! Landings: the events, the damage and the stun.

use super::*;

#[test]
fn falls_and_lands_on_floor() {
    let w = flat();
    let w = MoveWorld::bare(&w);
    let mut ps = PlayerState::spawn(Vec3::new(0.0, 0.0, 50.0), 0.0);
    tick(&mut ps, &PmInput::default(), &w, 250);
    assert!(ps.on_ground);
    assert!(ps.origin.z.abs() < 0.5, "settled at {}", ps.origin.z);
    assert!(ps.velocity.length() < 1.0);
}

#[test]
fn landing_plays_one_event_from_the_impact_bands() {
    let w = dirt_flat();
    let w = MoveWorld::bare(&w);
    let mut ps = PlayerState::spawn(Vec3::new(0.0, 0.0, 200.0), 0.0);
    let idle = PmInput::default();
    let mut events = Vec::new();
    for _ in 0..500 {
        events.extend(pmove(&mut ps, &idle, &w, 8.0 / 1000.0, &[]));
    }
    assert!(ps.on_ground);
    let ids: Vec<i32> = events.into_iter().map(|e| e.event).collect();
    assert_eq!(ids, vec![EV_LANDING_BASE + 6], "a long fall lands once");
}

/// The ladder reads the fall height, not the speed: the one-unit drop a
/// turret release ends in lands in silence, as the retail turret capture
/// reads it (`crates/server/tests/turret_ab.rs`).
#[test]
fn a_one_unit_drop_lands_silently() {
    let w = dirt_flat();
    let w = MoveWorld::bare(&w);
    let mut ps = PlayerState::spawn(Vec3::new(0.0, 0.0, 1.0), 0.0);
    ps.on_ground = false;
    let idle = PmInput::default();
    let mut events = Vec::new();
    for _ in 0..20 {
        events.extend(pmove(&mut ps, &idle, &w, 8.0 / 1000.0, &[]));
    }
    assert!(ps.on_ground);
    assert!(events.is_empty(), "{events:?}");
}

/// The land anim's speed half: a 200-unit fall lands faster than
/// `LAND_ANIM_SPEED` and reports it on the landing move only; a
/// one-unit drop never does.
#[test]
fn only_a_fast_landing_reports_the_land_anim() {
    let w = dirt_flat();
    let w = MoveWorld::bare(&w);
    let idle = PmInput::default();
    let mut ps = PlayerState::spawn(Vec3::new(0.0, 0.0, 200.0), 0.0);
    ps.on_ground = false;
    let mut reported = Vec::new();
    for i in 0..500 {
        let was = ps.on_ground;
        pmove(&mut ps, &idle, &w, 8.0 / 1000.0, &[]);
        if ps.land_anim {
            reported.push((i, was, ps.on_ground));
        }
    }
    assert_eq!(reported.len(), 1, "{reported:?}");
    assert_eq!((reported[0].1, reported[0].2), (false, true));

    let mut ps = PlayerState::spawn(Vec3::new(0.0, 0.0, 1.0), 0.0);
    ps.on_ground = false;
    for _ in 0..20 {
        pmove(&mut ps, &idle, &w, 8.0 / 1000.0, &[]);
        assert!(!ps.land_anim);
    }
    assert!(ps.on_ground);
}

#[test]
fn landing_sound_bands_follow_fall_height() {
    let band = |sf: u32, height: f32| {
        let mut ps = PlayerState::spawn(Vec3::ZERO, 0.0);
        ps.ground_surface_flags = sf;
        landing_event(height, &ps).map(|e| (e.event, e.parm))
    };
    let dirt = 6 << 20;
    assert_eq!(band(dirt, 4.0), None);
    assert_eq!(band(dirt, 4.5), Some((EV_FOOTSTEP_WALK_BASE + 6, 0)));
    assert_eq!(band(dirt, 11.5), Some((EV_FOOTSTEP_RUN_BASE + 6, 0)));
    assert_eq!(band(dirt, 12.0), Some((EV_LANDING_BASE + 6, 0)));
    // the land event's parm is the view bob, 4 at 12 up to 24
    assert_eq!(band(dirt, 38.0), Some((EV_LANDING_BASE + 6, 8)));
    assert_eq!(band(dirt, 200.0), Some((EV_LANDING_BASE + 6, 24)));
    // material 0 silences the steps but lands on the default surface
    let quiet = SURF_NO_SOUND | dirt;
    assert_eq!(band(quiet, 11.5), None);
    assert_eq!(band(quiet, 38.0), Some((EV_LANDING_BASE, 8)));
}

/// Drop a player from `height` onto flat dirt at 8 ms frames and return
/// the landing frame's state and events.
fn land_from(height: f32, sf: u32) -> (PlayerState, Vec<PmEvent>) {
    let w = crate::collision::synthetic_world(
        &[("textures/test/floor", crate::collision::CONTENTS_SOLID, sf)],
        &[(0, [-2048.0, -2048.0, -16.0], [2048.0, 2048.0, 0.0])],
    );
    let w = MoveWorld::bare(&w);
    let mut ps = PlayerState::spawn(Vec3::new(0.0, 0.0, height), 0.0);
    ps.on_ground = false;
    ps.velocity.x = 100.0;
    for _ in 0..1000 {
        let events = pmove(&mut ps, &PmInput::default(), &w, 8.0 / 1000.0, &[]);
        if ps.on_ground {
            return (ps, events);
        }
    }
    panic!("never landed");
}

/// A fall past `bg_fallDamageMinHeight` (256) lands with the damage in
/// the pain event's parm, `pm_time` 35 * damage + 500 under 0x100 and
/// the velocity scaled by the stun's multiplier.
#[test]
fn a_damaging_landing_starts_the_stun() {
    let dirt = 6 << 20;
    let (ps, events) = land_from(300.0, dirt);
    let damage = events
        .iter()
        .find(|e| e.event == EV_LANDING_PAIN_BASE + 6)
        .map(|e| e.parm)
        .expect("pain event");
    // The velocity snap drops 0.4 a frame off gravity's 6.4, so the 300
    // units read as 281 by the impact speed: 11 percent.
    assert_eq!(damage, 11);
    let stun = 35 * damage + 500;
    assert_eq!(ps.knockback_ms, stun as f32);
    assert_eq!(ps.knockback_flags, PMF_TIME_KNOCKBACK);
    let scale = 0.5 - (stun - 500) as f32 / 1000.0 * 0.3;
    // 38.45, snapped
    assert_eq!(ps.velocity.x, (100.0 * scale).round(), "scale {scale}");

    // past 28 percent the scale pins at 0.2, past 42 the timer at 2000
    let (ps, events) = land_from(420.0, dirt);
    let damage = events
        .iter()
        .find(|e| e.event >= EV_LANDING_PAIN_BASE)
        .unwrap()
        .parm;
    assert!(damage > 42, "damage {damage}");
    assert_eq!(ps.knockback_ms, 2000.0);
    assert!((ps.velocity.x - 20.0).abs() < 0.01, "{}", ps.velocity.x);
}

/// Under the min height, on a no-damage or slick surface, and at 100
/// percent there is no stun; the last three damp by 0.67.
#[test]
fn a_soft_landing_starts_no_stun() {
    let dirt = 6 << 20;
    let (ps, events) = land_from(200.0, dirt);
    assert_eq!((ps.knockback_ms, ps.knockback_flags), (0.0, 0));
    assert_eq!(events[0].event, EV_LANDING_BASE + 6);
    assert!((ps.velocity.x - 67.0).abs() < 0.01);

    let (ps, events) = land_from(300.0, dirt | SURF_NODAMAGE);
    assert_eq!((ps.knockback_ms, ps.knockback_flags), (0.0, 0));
    assert_eq!(events[0].event, EV_LANDING_BASE + 6);

    for (sf, height, what) in [
        (dirt | crate::collision::SURF_SLICK, 300.0, "slick"),
        (dirt, 600.0, "fatal"),
    ] {
        let (ps, events) = land_from(height, sf);
        assert_eq!((ps.knockback_ms, ps.knockback_flags), (0.0, 0), "{what}");
        assert_eq!(events[0].event, EV_LANDING_PAIN_BASE + 6, "{what}");
        assert!(
            (ps.velocity.x - 67.0).abs() < 0.01,
            "{what} {}",
            ps.velocity.x
        );
    }
}

/// The two bounds are the cvars' values, and bounds out of order or a
/// negative min take no damage at all (0x2fe6c).
#[test]
fn the_fall_damage_bounds_are_the_cvars() {
    let at = |min, max| FallHeights { min, max };
    assert_eq!(fall_damage(300.0, 0, 0, FallHeights::default()), 19);
    assert_eq!(fall_damage(300.0, 0, 0, at(200.0, 1000.0)), 12);
    assert_eq!(fall_damage(300.0, 0, 0, at(100.0, 300.0)), 100);
    assert_eq!(fall_damage(250.0, 0, 2, at(200.0, 300.0)), 25);
    assert_eq!(fall_damage(900.0, 0, 0, at(480.0, 256.0)), 0);
    assert_eq!(fall_damage(900.0, 0, 0, at(-1.0, 256.0)), 0);

    // The retail run's systeminfo under `+set`s (player-clip doc 8.10).
    let info = r"\bg_fallDamageMaxHeight\1000\bg_fallDamageMinHeight\200\sv_cheats\0";
    assert_eq!(FallHeights::from_systeminfo(info), at(200.0, 1000.0));
    assert_eq!(
        FallHeights::from_systeminfo(r"\sv_cheats\0"),
        FallHeights::default()
    );
}

/// A corpse dropped onto the floor lands like a live player on a soft
/// fall, whatever the height: the damp and the land event, no pain and
/// no stun (`pm_type > 5` at 0x2feba).
#[test]
fn a_corpse_lands_with_no_damage() {
    let w = crate::collision::synthetic_world(
        &[(
            "textures/test/floor",
            crate::collision::CONTENTS_SOLID,
            6 << 20,
        )],
        &[(0, [-2048.0, -2048.0, -16.0], [2048.0, 2048.0, 0.0])],
    );
    let w = MoveWorld::bare(&w);
    let mut ps = PlayerState::spawn(Vec3::new(0.0, 0.0, 600.0), 0.0);
    ps.on_ground = false;
    let mut landed = None;
    for _ in 0..1000 {
        let events = dead_move(&mut ps, &w, 0.016);
        if ps.on_ground {
            landed = Some(events);
            break;
        }
    }
    let events = landed.expect("never landed");
    assert_eq!(events.len(), 1, "{events:?}");
    assert_eq!(events[0].event, EV_LANDING_BASE + 6);
    assert_eq!((ps.knockback_ms, ps.knockback_flags), (0.0, 0));
}
