//! Committed captures' bodies: aim, lean, control bones, shared meshes
//! and a gunner on his gun.

use std::collections::BTreeMap;

use vcod_cod2::player::eflags as cod2_eflags;

use super::*;

/// A watched body shows its aim and its lean (cod4-player-anims.md 8):
/// the spectator capture's walker, posed with its entity's pitch and
/// `leanf` overridden, drops its head aiming down and swings it back
/// aiming up, and moves it to the side it leans to.
#[test]
fn a_watched_body_aims_and_leans() {
    use crate::play::cod4_players::Cod4Players;
    let Some(install) = vcod_cod4::testing::cod4_install() else {
        return;
    };
    let text = std::fs::read_to_string(format!("{FIXTURES}/play-1.7-spec.txt")).unwrap();
    let mut s = Cod4Session::open_capture(&text).unwrap();
    let (_, weapons, src) =
        crate::cod4_fly::load_bodies(install.root(), "mp_crash", &s.entity_assets()).unwrap();
    let mut ms = 0.0;
    let mut walker = None;
    while walker.is_none() && !s.finished() {
        ms += 1000.0 / 60.0;
        s.step(ms).unwrap();
        walker = s.scene().and_then(|scene| {
            let d = scene.entities.iter().find(|d| d.number == 2)?;
            Some((d.body.clone()?, scene.render_time, scene.server_time))
        });
    }
    let (view, t, snap) = walker.unwrap();
    // The head's model-space spot after a second of frames at `pitch`
    // and `lean` (the controls ease at 0.36 degrees a ms).
    let head = |pitch: f32, lean: f32| {
        let mut v = view.clone();
        v.frame.angles[0] = pitch;
        v.frame.lean = lean;
        let mut players = Cod4Players::default();
        let mut out = Vec::new();
        for k in 0..60 {
            out = players.posed(&src, &weapons, &[(2, &v)], &[], t + k * 16, snap);
        }
        let (_, rig, p) = &out[0];
        let body = &rig.models[0].1;
        let b = body.bones.iter().position(|b| b.name == "j_head").unwrap();
        // Model space: x forward, y left.
        let at = p.models[0].1[b].transform_point3(body.bones[b].pos);
        (at.z, at.x, -at.y)
    };
    let level = head(0.0, 0.0);
    let down = head(45.0, 0.0);
    let up = head(-45.0, 0.0);
    let right = head(0.0, 0.5);
    let left = head(0.0, -0.5);
    println!("head level {level:?} down {down:?} up {up:?} right {right:?} left {left:?}");
    // The torso pitches toward twice the view, half of it on the
    // spine with no `back_up`: the head swings 9 units each way (3.6
    // at CoD2's 0.6 of the view).
    assert!(
        down.0 < level.0 - 6.0 && down.1 > level.1 + 8.0,
        "{down:?} {level:?}"
    );
    assert!(
        up.0 > level.0 - 0.5 && up.1 < level.1 - 8.0,
        "{up:?} {level:?}"
    );
    assert!(right.2 > level.2 + 3.0, "{right:?} {level:?}");
    assert!(left.2 < level.2 - 3.0, "{left:?} {level:?}");
}

/// The drawn body turns its control bones in `tag_origin`'s frame as
/// the server does (cod4-player-anims.md 8): the spectator capture's
/// walker strafing 45 degrees off its view and aiming 30 down, so
/// `tag_origin` yaws off the view. For the same pose and controls every
/// bone of the drawn body sits within a thousandth of a unit of
/// `cod4_lnxded`'s arithmetic, where CoD1's model-frame turn moves the
/// head by more than a unit. The drawn body's locals are 1.7's
/// sampler's (cod4-player-anims.md 11), word for word.
#[test]
fn a_drawn_bodys_control_bones_turn_as_the_servers() {
    use crate::play::cod4_players::Cod4Players;
    use vcod_cod4::skel::{player_mats_sampled, worlds_of};
    use vcod_common::playerpose::Cod4Arith;
    let Some(install) = vcod_cod4::testing::cod4_install() else {
        return;
    };
    let text = std::fs::read_to_string(format!("{FIXTURES}/play-1.7-spec.txt")).unwrap();
    let mut s = Cod4Session::open_capture(&text).unwrap();
    let (_, weapons, src) =
        crate::cod4_fly::load_bodies(install.root(), "mp_crash", &s.entity_assets()).unwrap();
    let mut ms = 0.0;
    let mut walker = None;
    while walker.is_none() && !s.finished() {
        ms += 1000.0 / 60.0;
        s.step(ms).unwrap();
        walker = s.scene().and_then(|scene| {
            let d = scene.entities.iter().find(|d| d.number == 2)?;
            Some((d.body.clone()?, scene.render_time, scene.server_time))
        });
    }
    let (mut v, t, snap) = walker.unwrap();
    v.frame.angles[0] = 30.0;
    v.frame.movement_dir = 45.0;
    let mut players = Cod4Players::default();
    let mut out = Vec::new();
    for k in 0..60 {
        out = players.posed(&src, &weapons, &[(2, &v)], &[], t + k * 16, snap);
    }
    let (_, rig, drawn) = &out[0];
    let bp = players.body_pose(&src, 2, &v.frame, t + 59 * 16).unwrap();
    let controls = bp.controls;
    assert!(std::rc::Rc::ptr_eq(rig, &bp.rig));
    assert!(controls.origin[1].abs() > 20.0, "{controls:?}");
    assert!(controls.back_low[0].abs() > 5.0, "{controls:?}");
    let server = worlds_of(&player_mats_sampled(
        &rig.skeleton,
        &bp.pose,
        &bp.sampled,
        Some(&controls),
        Cod4Arith::Server,
    ));
    // The drawn body takes `iw3mp.exe`'s sampler's locals (cod4-player-anims.md
    // 11) on every bone a clip reaches, word for word.
    assert!(bp.sampled.iter().filter(|l| l.is_some()).count() > 40);
    let client = worlds_of(&player_mats_sampled(
        &rig.skeleton,
        &bp.pose,
        &bp.sampled,
        Some(&controls),
        Cod4Arith::Client,
    ));
    assert_eq!(drawn.models[0].1, rig.skeleton.skin_matrices(&client, 0));
    let body = &rig.models[0].1;
    let at = |skins: &[glam::Mat4], b: usize| skins[b].transform_point3(body.bones[b].pos);
    let theirs = rig.skeleton.skin_matrices(&server, 0);
    let cod1 = bp.shared.skin_matrices(&rig.skeleton, 0);
    let worst = (0..body.bones.len())
        .map(|b| at(&drawn.models[0].1, b).distance(at(&theirs, b)))
        .fold(0.0f32, f32::max);
    assert!(worst < 1e-3, "{worst}");
    let head = body.bones.iter().position(|b| b.name == "j_head").unwrap();
    let off = at(&cod1, head).distance(at(&theirs, head));
    println!("worst bone {worst:e}, CoD1's head {off:.2} off");
    assert!(off > 1.0, "{off}");
}

/// One body under two weapons is two rigs over one copy of its body
/// and head: each rig baked its own before (cod4-client-perf.md 23).
#[test]
fn a_body_under_two_weapons_shares_its_meshes() {
    use crate::play::cod4_players::Cod4Players;
    let Some(install) = vcod_cod4::testing::cod4_install() else {
        return;
    };
    let text = std::fs::read_to_string(format!("{FIXTURES}/play-1.7-spec.txt")).unwrap();
    let mut s = Cod4Session::open_capture(&text).unwrap();
    let mut ents = s.entity_assets();
    ents.weapons.extend(["ak47_mp".into(), "m16_mp".into()]);
    let (_, weapons, src) =
        crate::cod4_fly::load_bodies(install.root(), "mp_crash", &ents).unwrap();
    let mut ms = 0.0;
    let mut walker = None;
    while walker.is_none() && !s.finished() {
        ms += 1000.0 / 60.0;
        s.step(ms).unwrap();
        walker = s.scene().and_then(|scene| {
            let d = scene.entities.iter().find(|d| d.number == 2)?;
            Some((d.body.clone()?, scene.render_time, scene.server_time))
        });
    }
    let (view, t, snap) = walker.unwrap();
    let posed = |players: &mut Cod4Players, weapon: &str, at: i32| {
        let mut v = view.clone();
        v.weapon = Some(weapon.into());
        let mut out = players.posed(&src, &weapons, &[(2, &v)], &[], at, snap);
        let (_, rig, p) = out.remove(0);
        (rig, p.transform, p.models)
    };
    let mut players = Cod4Players::default();
    let (ak, ..) = posed(&mut players, "ak47_mp", t);
    let (m16, transform, drawn) = posed(&mut players, "m16_mp", t + 16);
    assert!(!std::rc::Rc::ptr_eq(&ak, &m16));
    // What is drawn is what a rig loaded on its own draws.
    let mut alone = Cod4Players::default();
    posed(&mut alone, "m16_mp", t);
    let (_, transform_alone, drawn_alone) = posed(&mut alone, "m16_mp", t + 16);
    assert_eq!((transform, &drawn), (transform_alone, &drawn_alone));
    assert_eq!(ak.models.len(), m16.models.len());
    for (i, ((a, ma), (b, mb))) in ak.models.iter().zip(&m16.models).enumerate() {
        // The gun is the first attachment; the body and the rest are
        // one model each.
        assert_eq!(std::rc::Rc::ptr_eq(ma, mb), i != 1, "{a} / {b}");
    }
}

/// A gunner drawn by another client stands where the server put him
/// (`CG_PlayerTurretPositionAndBlend`, cod4-gsc.md 9.2): the spectator
/// capture's walker given mp_backlot's crouch SAW 393 at (-720 68 290)
/// and its gunner's legs anim (0x35) and wire origin, drawn at retail's
/// placement for the barrel at 0 and -45 (`play-1.7-saw.txt`).
#[test]
fn a_watched_gunner_stands_on_his_gun() {
    use crate::play::cod4_players::{Cod4Players, GunInfo};
    let Some(install) = vcod_cod4::testing::cod4_install() else {
        return;
    };
    let text = std::fs::read_to_string(format!("{FIXTURES}/play-1.7-spec.txt")).unwrap();
    let mut s = Cod4Session::open_capture(&text).unwrap();
    let mut ms = 0.0;
    let mut walker = None;
    while walker.is_none() && !s.finished() {
        ms += 1000.0 / 60.0;
        s.step(ms).unwrap();
        walker = s.scene().and_then(|scene| {
            let d = scene.entities.iter().find(|d| d.number == 2)?;
            Some((d.body.clone()?, scene.render_time, scene.server_time))
        });
    }
    let (view, t, snap) = walker.unwrap();
    let mut ents = s.entity_assets();
    ents.models.push("weapon_saw_mg_setup".into());
    ents.weapons.push("saw_bipod_crouch_mp".into());
    let (models, weapons, src) =
        crate::cod4_fly::load_bodies(install.root(), "mp_backlot", &ents).unwrap();
    let gun_model = models
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("weapon_saw_mg_setup"))
        .map(|(_, m)| m)
        .unwrap_or_else(|| {
            panic!(
                "{:?}",
                models
                    .keys()
                    .filter(|k| k.contains("saw"))
                    .collect::<Vec<_>>()
            )
        });
    let bones = &gun_model.bones;
    let tag = |n: &str| bones.iter().find(|b| b.name == n).unwrap().pos;
    let drawn = |barrel: f32, origin: [f32; 3]| {
        let mut v = view.clone();
        v.frame.legs_anim = 0x35;
        v.frame.eflags |= 0x200;
        v.frame.origin = Vec3::from(origin);
        let gun = GunInfo {
            frame: vcod_cod2::player::GunFrame {
                model: "weapon_saw_mg_setup".into(),
                weapon: "saw_bipod_crouch_mp".into(),
                origin: Vec3::new(-720.0, 68.0, 290.0),
                angles: [0.0; 3],
                barrel: [0.0, barrel, 0.0],
            },
            tags: (tag("tag_aim"), tag("tag_weapon")),
            rotate_inc: weapons["saw_bipod_crouch_mp"].rotate_inc,
        };
        let mut players = Cod4Players::default();
        let out = players.posed(&src, &weapons, &[(2, &v)], &[Some(gun)], t, snap);
        out[0].2.transform.w_axis.truncate()
    };
    // The wire's truncated origins of retail's placed gunner: the
    // height off the truncated z blends the rows a little apart from
    // the server's, as the cgame's does off the same origin.
    for (barrel, wire, placed) in [
        (0.0, [-754.0, 75.0, 258.0], [-754.039, 75.398]),
        (-45.0, [-738.0, 99.0, 258.0], [-738.634, 99.883]),
    ] {
        let at = drawn(barrel, wire);
        assert!(
            (at.x - placed[0]).abs() < 0.15 && (at.y - placed[1]).abs() < 0.15,
            "barrel {barrel}: drawn at {at:?}, retail placed {placed:?}"
        );
    }
}

/// The spectator capture's bodies posed through `play::cod4_players`
/// at 60 frames a second, with the zones' clips and models
/// (`COD4_DIR`; vacuous without it): every anim the walker's entity
/// names resolves to a clip, every frame it is drawn it is posed, and
/// the pose moves it (cod4-player-anims.md 6).
#[test]
fn spectator_capture_poses_its_bodies() {
    use crate::play::cod4_players::Cod4Players;
    use vcod_common::asset_source::AssetSource;
    let Some(install) = vcod_cod4::testing::cod4_install() else {
        return;
    };
    let text = std::fs::read_to_string(format!("{FIXTURES}/play-1.7-spec.txt")).unwrap();
    let mut s = Cod4Session::open_capture(&text).unwrap();
    let (_, weapons, src) =
        crate::cod4_fly::load_bodies(install.root(), "mp_crash", &s.entity_assets()).unwrap();
    let mut players = Cod4Players::default();
    assert!(players.anim_count(&src) > 300);
    let (mut frames, mut posed, mut drawn) = (0, 0, 0);
    let mut names: BTreeMap<String, bool> = BTreeMap::new();
    // The walker's neck, model space, standing and crouched.
    let mut necks: Vec<(i32, f32)> = Vec::new();
    let mut moved = 0;
    let mut last: Option<Vec<glam::Mat4>> = None;
    let mut wrists: Vec<glam::Vec3> = Vec::new();
    let mut ms = 0.0;
    while !s.finished() {
        ms += 1000.0 / 60.0;
        s.step(ms).unwrap();
        let Some(scene) = s.scene() else { continue };
        frames += 1;
        let views: Vec<(u32, &BodyView)> = scene
            .entities
            .iter()
            .filter_map(|d| Some((d.number, d.body.as_ref()?)))
            .collect();
        drawn += views.len();
        for (_, v) in &views {
            for wire in [v.frame.legs_anim, v.frame.torso_anim] {
                if wire & 511 == 0 {
                    continue;
                }
                let name = players.anim_name(&src, wire).unwrap_or_default();
                let ok = src.decoded_anim(&name).is_some_and(|c| c.is_ok());
                names.insert(name, ok);
            }
        }
        let out = players.posed(
            &src,
            &weapons,
            &views,
            &[],
            scene.render_time,
            scene.server_time,
        );
        posed += out.len();
        for (i, rig, p) in &out {
            assert!(
                p.models
                    .iter()
                    .all(|(_, m)| m.iter().all(|m| m.is_finite()))
            );
            // The held weapon (model 1) in the right hand: the wrist
            // behind the gun's `tag_weapon`, along its barrel.
            if views[*i].1.weapon.is_some() {
                let body = &rig.models[0].1;
                let b = body
                    .bones
                    .iter()
                    .position(|b| b.name == "j_wrist_ri")
                    .unwrap();
                let wrist = p.models[0].1[b].transform_point3(body.bones[b].pos);
                let at = p.models[1].1[0].inverse().transform_point3(wrist);
                assert!(
                    (-25.0..-8.0).contains(&at.x),
                    "right wrist at {at} off the gun"
                );
                wrists.push(at);
            }
            if views[*i].0 != 2 {
                continue;
            }
            let body = &rig.models[0].1;
            let neck = body.bones.iter().position(|b| b.name == "neck").unwrap();
            let at = p.models[0].1[neck].transform_point3(body.bones[neck].pos);
            necks.push((views[*i].1.frame.eflags, at.z));
            if last.as_ref().is_some_and(|l| *l != p.models[0].1) {
                moved += 1;
            }
            last = Some(p.models[0].1.clone());
        }
    }
    assert!(frames > 2000, "{frames}");
    assert_eq!(posed, drawn, "every body drawn is posed");
    let missing: Vec<&String> = names
        .iter()
        .filter(|(_, ok)| !**ok)
        .map(|(n, _)| n)
        .collect();
    assert!(missing.is_empty(), "no clip for {missing:?}");
    assert!(
        names.contains_key("pb_combatrun_forward_loop") && names.contains_key("pt_stand_shoot"),
        "{names:?}"
    );
    assert!(
        moved > frames / 2,
        "the walker moved in {moved} of {frames} frames"
    );
    let mean = |crouched: bool| {
        let v: Vec<f32> = necks
            .iter()
            .filter(|(f, _)| (f & cod2_eflags::CROUCH != 0) == crouched)
            .map(|(_, z)| *z)
            .collect();
        v.iter().sum::<f32>() / v.len().max(1) as f32
    };
    let (stand, crouch) = (mean(false), mean(true));
    let wrist = wrists.iter().sum::<glam::Vec3>() / wrists.len().max(1) as f32;
    println!(
        "right wrist off the gun's tag_weapon: mean {wrist} over {}",
        wrists.len()
    );
    assert!(wrists.len() > 2000, "{}", wrists.len());
    assert!(
        wrist.distance(glam::Vec3::new(-14.1, -1.0, -4.8)) < 1.0,
        "right wrist at {wrist} on average"
    );
    println!(
        "clips {}  posed {posed} of {frames} frames  neck stand {stand:.1} crouch {crouch:.1}",
        names.len()
    );
    assert!((50.0..70.0).contains(&stand), "standing neck at {stand}");
    assert!(
        crouch < stand - 10.0,
        "crouched neck at {crouch}, standing {stand}"
    );
}
