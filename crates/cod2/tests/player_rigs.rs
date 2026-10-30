//! Every stock MP character assembles into one skeleton and poses through
//! the shared pose code. Needs `COD2_DIR`; passes vacuously without it.
use glam::Vec3;
use vcod_cod2::player::{
    BodyFrame, Character, PlayerAnimSet, PlayerBody, PlayerRig, RigParts, character, posed,
};
use vcod_common::animclock::Times;
use vcod_common::playerpose::BG_SWING_SPEED;
use vcod_common::testing::cod2_fs;

/// The six controls `GScr_LoadConsts` (0x81224f8) names, and `tag_origin`.
const CONTROLS: [&str; 7] = [
    "tag_origin",
    "pelvis",
    "back_low",
    "back_mid",
    "back_up",
    "neck",
    "head",
];

#[test]
fn every_stock_mp_character_assembles() {
    let Some(fs) = cod2_fs() else {
        return;
    };
    vcod_cod2::model::register();
    let names: Vec<String> = fs
        .list_prefix("character/mp_")
        .iter()
        .filter_map(|p| p.strip_prefix("character/")?.strip_suffix(".gsc"))
        .map(str::to_string)
        .collect();
    assert_eq!(names.len(), 37, "{names:?}");
    // Each team's mptype picks among them.
    let picked = character::mptype_characters(&fs, "german_africa").unwrap();
    assert_eq!(picked, ["mp_german_africa"]);
    assert_eq!(
        character::mptype_characters(&fs, "american_normandy")
            .unwrap()
            .len(),
        10
    );
    for name in &names {
        let c = Character::load(&fs, name).unwrap_or_else(|e| panic!("{name}: {e:#}"));
        // The two leftovers name bodies no pak ships.
        if matches!(name.as_str(), "mp_allies_default" | "mp_axis_default") {
            assert!(PlayerRig::assemble(&fs, &c).is_err(), "{name}");
            continue;
        }
        let rig = PlayerRig::assemble(&fs, &c).unwrap_or_else(|e| panic!("{name}: {e:#}"));
        assert_eq!(rig.models.len(), 1 + c.attachments.len(), "{name}");
        let skel = &rig.skeleton;
        for bone in CONTROLS {
            assert!(skel.bone_index(bone).is_some(), "{name}: no {bone}");
        }
        // Every attachment grafted onto a body bone: one root.
        let roots = skel.bones().iter().filter(|b| b.parent < 0).count();
        assert_eq!(roots, 1, "{name}");
    }
    for team in [0, 1, 2] {
        let c = Character::default_for_team(&fs, team).unwrap();
        PlayerRig::assemble(&fs, &c).unwrap();
    }
}

/// One body under two weapons is two rigs (the client keys a rig by its
/// character, weapon included) over one set of meshes: a rig per weapon
/// baked its own copy, and the live client grew by them level after level
/// (cod2-client-perf.md 5.3).
#[test]
fn rigs_of_one_body_share_its_meshes() {
    let Some(fs) = cod2_fs() else {
        return;
    };
    vcod_cod2::model::register();
    let c = Character::load(&fs, "mp_german_africa").unwrap();
    let mut parts = RigParts::new();
    let armed = |gun: &str| {
        let mut a = c.clone();
        a.attachments
            .push((gun.to_string(), Some("tag_weapon_right".into())));
        a
    };
    let kar = PlayerRig::assemble_with(&fs, &armed("weapon_kar98"), &mut parts).unwrap();
    let mp44 = PlayerRig::assemble_with(&fs, &armed("weapon_mp44"), &mut parts).unwrap();
    let gun = kar.models.len() - 1;
    assert_eq!(mp44.models.len(), gun + 1);
    for (i, ((a, ma), (b, mb))) in kar.models.iter().zip(&mp44.models).enumerate() {
        // The gun is the last part; the body and the rest are one model each.
        assert_eq!(std::rc::Rc::ptr_eq(ma, mb), i != gun, "{a} / {b}");
    }
}

/// The retail capture's other client (`fixtures/mp_toujane-spectate.bin`)
/// ran forward with `legsAnim` 127 and stood with 153 (665 with the
/// restart toggle), each read here through 1.1's walk of CoD2's tree.
#[test]
fn the_wire_anims_run_and_idle_pose_to_finite_bones() {
    let Some(fs) = cod2_fs() else {
        return;
    };
    vcod_cod2::model::register();
    let mut set = PlayerAnimSet::load(&fs).unwrap();
    assert_eq!(set.anims.name(127), Some("pb_combatrun_forward_loop"));
    assert_eq!(set.anims.name(153), Some("pb_stand_alert"));
    assert_eq!(set.anims.name(665), Some("pb_stand_alert"));

    let c = Character::load(&fs, "mp_german_africa").unwrap();
    let rig = PlayerRig::assemble(&fs, &c).unwrap();
    let knee = rig.skeleton.bone_index("j_knee_le").unwrap();
    for legs in [127, 153] {
        let mut body = PlayerBody::default();
        let mut knees = Vec::new();
        for i in 0..20 {
            let now = 1000 + i * 50;
            let f = BodyFrame {
                origin: Vec3::new(if legs == 127 { i as f32 * 9.0 } else { 0.0 }, 0.0, 0.0),
                angles: [10.0, 0.0, 0.0],
                legs_anim: legs,
                torso_anim: 0,
                ..Default::default()
            };
            let at = Times {
                clock_ms: now,
                snapshot_ms: now,
            };
            body.advance(&set.anims, &f, at, 50, BG_SWING_SPEED);
            let pose = body.pose(&rig, &mut set, &fs, &f, now, None);
            let p = posed(&rig, &pose, &f);
            assert_eq!(p.models.len(), rig.models.len());
            for (name, skins) in &p.models {
                assert!(
                    skins.iter().all(|m| m.is_finite()),
                    "{legs} {name}: a skin matrix is not finite"
                );
            }
            knees.push(pose.bone_world(&rig.skeleton, knee).0);
        }
        let travel = knees
            .windows(2)
            .map(|w| w[0].distance(w[1]))
            .fold(0.0, f32::max);
        if legs == 127 {
            assert!(travel > 1.0, "the run moves no knee: {travel}");
        } else {
            assert!(travel < 0.5, "the idle walks: {travel}");
        }
    }
}
