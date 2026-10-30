//! A 1.7 install's player clips and models in vcod-common's types
//! (`vcod_cod4::anim`, docs/research/cod4-player-anims.md): every xanim of
//! `common_mp` decodes with each data stream read to its last byte, every
//! xmodel of `mp_crash` composes its local bind into the zone's own
//! `baseMat`, and the player tree's clips all resolve. Vacuous without
//! `COD4_DIR`.

use glam::{Quat, Vec3};
use vcod_cod4::anim::{BodySource, bones, clip, model};
use vcod_cod4::assets::Assets;
use vcod_cod4::testing::cod4_install;
use vcod_cod4::walk::{WalkOptions, walk};
use vcod_cod4::zone::Zone;
use vcod_common::asset_source::AssetSource;

fn zone(name: &str) -> Option<Assets> {
    let install = cod4_install()?;
    let z = Zone::open_path(&install.zone("english", name)).unwrap();
    Some(walk(z, WalkOptions { keep_bulk: true }).unwrap().assets)
}

#[test]
fn common_mp_clips_decode_to_their_last_byte() {
    let Some(a) = zone("common_mp") else {
        return;
    };
    let mut n = 0;
    for x in a.xanims.iter().filter(|x| !x.name.starts_with(',')) {
        let c = clip(&a, x).unwrap_or_else(|e| panic!("{e:#}"));
        assert_eq!(c.tracks.len(), usize::from(x.bone_count[9]), "{}", x.name);
        n += 1;
    }
    assert_eq!(n, 1125);
    // The run's root travels; its forward pace is the clip's.
    let run = a
        .xanims
        .iter()
        .find(|x| x.name == "pb_combatrun_forward_loop")
        .unwrap();
    let m = clip(&a, run).unwrap().motion();
    println!("pb_combatrun_forward_loop: {m:?}");
    assert!(m.looping && m.distance / m.duration > 100.0, "{m:?}");
}

#[test]
fn mp_crash_models_compose_their_base_matrices() {
    let Some(a) = zone("mp_crash") else {
        return;
    };
    let mut n = 0;
    let mut off = Vec::new();
    for x in a.xmodels.iter().filter(|x| !x.name.starts_with(',')) {
        let Some(m) = model(&a, x, |id| a.materials[id.index()].name.clone()).unwrap() else {
            continue;
        };
        let matches = bones(&a, x).unwrap().iter().enumerate().all(|(i, b)| {
            let f = |o: usize| {
                f32::from_le_bytes(x.base_mat[32 * i + o..32 * i + o + 4].try_into().unwrap())
            };
            let q = Quat::from_xyzw(f(0), f(4), f(8), f(12)).normalize();
            let t = Vec3::new(f(16), f(20), f(24));
            b.pos.distance(t) < 0.01 && b.rot.dot(q).abs() > 0.9999
        });
        if !matches {
            off.push(m.name.clone());
        }
        n += 1;
    }
    println!("{n} models, base off the composition: {off:?}");
    assert_eq!(n, 297);
    // Only the first-person hands' bind differs from their own bones.
    assert!(off.iter().all(|m| m.starts_with("viewhands_")), "{off:?}");
}

#[test]
fn the_player_tree_resolves_its_clips() {
    let (Some(map), Some(common)) = (zone("mp_crash"), zone("common_mp")) else {
        return;
    };
    let src = BodySource::from_zones(&[&map, &common], Default::default()).unwrap();
    // The tree's 393 and the helicopter's `bh_rotors` (`anim::ENTITY_CLIPS`).
    assert_eq!(src.clip_count(), 394);
    let anims = vcod_common::animtree::PlayerAnims::load(&src).unwrap();
    // The spectator capture's walker (cod4-player-anims.md 3).
    for (wire, name) in [
        (185, "pb_stand_alert"),
        (152, "pb_combatrun_forward_loop"),
        (155, "pb_standjump_takeoff"),
        (154, "pb_standjump_land"),
        (178, "pb_crouch_alert"),
        (316, "pt_stand_shoot"),
    ] {
        assert_eq!(anims.name(wire), Some(name));
        assert!(src.decoded_anim(name).unwrap().is_ok());
        assert!(anims.motion(wire).is_some(), "{name}");
    }
}

/// The kill capture's corpse (`legsAnim` 36, cod4-combat.md 5) is one of
/// the standing `death` clause's eight commands, whichever `rand()` drew.
#[test]
fn the_kill_captures_death_anim_is_a_standing_death() {
    let (Some(map), Some(common)) = (zone("mp_crash"), zone("common_mp")) else {
        return;
    };
    let src = BodySource::from_zones(&[&map, &common], Default::default()).unwrap();
    let anims = vcod_common::animtree::PlayerAnims::load(&src).unwrap();
    let ps = vcod_cod4::pmove::PlayerState {
        anim_movetype: vcod_cod4::pmove::anim::movetype::IDLE,
        pm_type: vcod_cod4::pmove::pm_type::DEAD,
        ..Default::default()
    };
    let input = vcod_cod4::pmove::anim::AnimInput {
        anims: &anims,
        player_anim_type: 4,
        weapon_class: 0,
    };
    let choices = vcod_cod4::pmove::anim::event_choices(&ps, 0, &input, "death");
    let legs: Vec<u32> = choices.iter().map(|c| c.legs_anim & 0x1ff).collect();
    println!(
        "{:?}",
        legs.iter()
            .map(|&w| anims.name(w as i32))
            .collect::<Vec<_>>()
    );
    assert_eq!(legs.len(), 8);
    assert!(legs.contains(&36), "{legs:?}");
}

/// `player_die`'s death flips the toggle of whatever torso anim the live
/// body holds: retail's 24 kills in the corpse run (cod4-combat.md
/// 5, the torso at the call and the corpse's `torsoAnim` after it, one
/// pair per state) hold for every command the clause offers.
#[test]
fn a_death_flips_the_torso_toggle_it_finds() {
    let (Some(map), Some(common)) = (zone("mp_crash"), zone("common_mp")) else {
        return;
    };
    let src = BodySource::from_zones(&[&map, &common], Default::default()).unwrap();
    let anims = vcod_common::animtree::PlayerAnims::load(&src).unwrap();
    let input = vcod_cod4::pmove::anim::AnimInput {
        anims: &anims,
        player_anim_type: 4,
        weapon_class: 0,
    };
    // Flinch, crouched, fire, reload, jump, switch, the fire's other bit.
    for (torso, corpse) in [
        (0x308, 0),
        (0x200, 0),
        (0x13c, 0x200),
        (0x321, 0),
        (0x105, 0x200),
        (0x30b, 0),
        (0x305, 0),
        (0x33c, 0),
    ] {
        let ps = vcod_cod4::pmove::PlayerState {
            anim_movetype: vcod_cod4::pmove::anim::movetype::IDLE,
            pm_type: vcod_cod4::pmove::pm_type::DEAD,
            torso_anim: torso,
            torso_timer: 484,
            ..Default::default()
        };
        let choices = vcod_cod4::pmove::anim::event_choices(&ps, 0, &input, "death");
        assert!(!choices.is_empty());
        for c in &choices {
            assert_eq!(c.torso_anim, corpse, "torso {torso:#x}");
        }
    }
}
