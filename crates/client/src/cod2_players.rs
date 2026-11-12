//! CoD2 players on the renderer's dynamic model path: each client's
//! character assembled and uploaded once, its body stepped and posed every
//! frame by `vcod_cod2::player`. `cod2_scene::players` fills a
//! [`Cod2PlayerView`] per player entity from the snapshot (lerped) and the
//! client states; docs/research/cod2-player-anims.md.

use crate::fx::sim::TagPose;
use crate::play::cod2_fx::BodyTags;
use crate::renderer::{CookieEntity, DynamicModelInstance, ModelHandle, Renderer};
use glam::{Mat4, Quat, Vec3};
use std::collections::HashMap;
use std::rc::Rc;
use vcod_cod2::player::{
    BodyFrame, Character, GunFrame, PlayerAnimSet, PlayerBody, PlayerRig, PosedPlayer, RigParts,
    gunner_placement, posed,
};
use vcod_common::animclock::Times;
use vcod_common::asset_source::AssetSource;
use vcod_common::pk3::Pk3Fs;
use vcod_common::playerpose::BG_SWING_SPEED;

/// One player entity this frame.
#[derive(Clone)]
pub struct Cod2PlayerView {
    /// Entity number: the body's state is kept under it.
    pub number: u32,
    /// Interpolated by the caller ([`BodyFrame::from_entity`] per snapshot).
    pub frame: BodyFrame,
    /// [`Character::from_client_state`]; `None` draws the team's default.
    pub character: Option<Character>,
    /// The client state's `team`, for the default.
    pub team: i32,
    /// The CS 7 weapon held, whose `worldModel` hangs off
    /// `tag_weapon_right` (`CG_CreateDObj`, Mac 0x14420c); `None` empty
    /// handed or on a gun.
    pub weapon: Option<String>,
    /// The gun a gunner rides (`eFlags & 0x300`, `otherEntityNum`).
    pub gun: Option<GunFrame>,
}

/// The hand `CG_CreateDObj` hangs the weapon off (`cg_weaponrightbone`); the
/// left (`cg_weaponleftbone`) follows an `anim_gunhand = "left"` note,
/// which vcod does not read.
const WEAPON_TAG: &str = "tag_weapon_right";

/// `eType` 2 with this bit is a corpse just made off a live body
/// (`PlayerCmd_ClonePlayer` sets 0xa0000): `CG_ResetEntity` clones that
/// client's anim tree into it.
const FRESH_CORPSE: i32 = vcod_cod2::player::eflags::FRESH_CORPSE;

/// One body: the character it was posed for and its state.
struct Body {
    character: Character,
    body: PlayerBody,
    /// The render time it was last drawn; its clips hold still while away.
    seen_ms: i32,
}

type HeldRig = (
    Character,
    Option<String>,
    Option<(Rc<PlayerRig>, [Option<usize>; 3])>,
);

/// Per-connection CoD2 player state: the animtree, the rigs and GPU models
/// by name, and one [`Body`] per entity.
#[derive(Default)]
pub struct Cod2Players {
    /// `None` until first needed; `Some(None)` once the load failed, so it
    /// warns once and nothing poses.
    anims: Option<Option<PlayerAnimSet>>,
    /// `None` is a character whose body failed to load.
    rigs: HashMap<Character, Option<Rc<PlayerRig>>>,
    /// The rigs' meshes by part, shared across every weapon a body holds.
    parts: RigParts,
    defaults: HashMap<i32, Option<Character>>,
    handles: HashMap<String, Option<ModelHandle>>,
    bodies: HashMap<u32, Body>,
    /// A weapon's `worldModel` by its CS 7 name.
    weapon_models: HashMap<String, Option<String>>,
    /// A gun model's `tag_aim` and `tag_weapon`, and a turret weapon's
    /// `animHorRotateInc`.
    gun_tags: HashMap<String, Option<(Vec3, Vec3)>>,
    rotate_incs: HashMap<String, f32>,
    /// Each entity's rig as last drawn: the character and CS 7 weapon it
    /// was armed for, the rig (`None` when it did not build) and its
    /// `tag_flash`, `j_head` and `tag_brass` bones.
    held: HashMap<u32, HeldRig>,
    /// `bg_swingSpeed`; the caller keeps it in step with the console.
    pub swing_speed: f32,
    /// The last [`Self::posed`]'s world `tag_flash` per entity, where a
    /// body's tracers start (`CG_CalcMuzzlePoint`, Mac 0x1d7f72).
    pub muzzles: HashMap<u32, Vec3>,
    /// The last [`Self::posed`]'s world `j_head` per entity, where its head
    /// icon floats (`CG_PlayerFloatSprite`, Mac 0x1e64c6).
    pub heads: HashMap<u32, Vec3>,
    /// The last [`Self::posed`]'s `tag_flash` and `tag_brass` poses per
    /// entity, where its world flash and brass play.
    pub tags: HashMap<u32, BodyTags>,
}

impl Cod2Players {
    pub fn new() -> Self {
        Cod2Players {
            swing_speed: BG_SWING_SPEED,
            ..Default::default()
        }
    }

    /// Forgets the old map's model handles and bodies; the parsed rigs,
    /// anims and weapon files stay (`Cod2Draw::next_map`).
    pub fn next_map(&mut self) {
        self.handles.clear();
        self.bodies.clear();
        self.held.clear();
        self.muzzles.clear();
        self.tags.clear();
    }

    /// Steps and poses `players` at `render_time`, `frametime_ms` after the
    /// last call; `snapshot_ms` is the newest snapshot's `serverTime`, the
    /// clock a body's anim rate is measured on. Each drawn player comes
    /// back with its index in `players`; one with no loadable body is left
    /// out. Bodies not in `players` are forgotten.
    pub fn posed(
        &mut self,
        fs: &dyn AssetSource,
        players: &[Cod2PlayerView],
        render_time: i32,
        frametime_ms: i32,
        snapshot_ms: i32,
    ) -> Vec<(usize, Rc<PlayerRig>, PosedPlayer)> {
        let anims = self.anims.get_or_insert_with(|| {
            vcod_cod2::model::register();
            PlayerAnimSet::load(fs)
                .inspect_err(|e| log::warn!("cod2 player anims: {e:#}, players stay unposed"))
                .ok()
        });
        let Some(set) = anims else {
            return Vec::new();
        };
        // A fresh corpse new this frame takes over its client's live body,
        // the clips mid-play, before the body is forgotten.
        for p in players {
            if p.frame.eflags & FRESH_CORPSE == 0 || self.bodies.contains_key(&p.number) {
                continue;
            }
            let live = u32::try_from(p.frame.client_num)
                .ok()
                .and_then(|n| self.bodies.get(&n))
                .map(|b| (b.character.clone(), b.body.clone(), b.seen_ms));
            if let Some((character, body, seen_ms)) = live {
                self.bodies.insert(
                    p.number,
                    Body {
                        character,
                        body,
                        seen_ms,
                    },
                );
            }
        }
        self.bodies
            .retain(|n, _| players.iter().any(|p| p.number == *n));
        self.held
            .retain(|n, _| players.iter().any(|p| p.number == *n));
        let mut out = Vec::new();
        self.muzzles.clear();
        self.heads.clear();
        self.tags.clear();
        for (i, p) in players.iter().enumerate() {
            let character = match &p.character {
                Some(c) => c,
                None => {
                    let default = self.defaults.entry(p.team).or_insert_with(|| {
                        Character::default_for_team(fs, p.team)
                            .inspect_err(|e| log::warn!("cod2 default character: {e:#}"))
                            .ok()
                    });
                    match default {
                        Some(c) => &*c,
                        None => continue,
                    }
                }
            };
            // The rig drawn last frame for the same character and weapon,
            // else the one they key.
            let rig = match self.held.get(&p.number) {
                Some((c, w, rig)) if c == character && *w == p.weapon => rig.clone(),
                _ => {
                    // The rig carries the weapon; the body's state is the
                    // character's alone, so a switch keeps the clips.
                    let mut armed = character.clone();
                    if let Some(w) = &p.weapon {
                        let model = self.weapon_models.entry(w.clone()).or_insert_with(|| {
                            match vcod_common::weapon::load(fs, w) {
                                Ok(d) => d.world_model.filter(|m| !m.is_empty()),
                                Err(e) => {
                                    log::warn!("CoD2 weapon '{w}': {e:#}");
                                    None
                                }
                            }
                        });
                        if let Some(m) = model {
                            armed
                                .attachments
                                .push((m.clone(), Some(WEAPON_TAG.to_string())));
                        }
                    }
                    let rig = self
                        .rigs
                        .entry(armed.clone())
                        .or_insert_with(|| {
                            PlayerRig::assemble_with(fs, &armed, &mut self.parts)
                                .inspect_err(|e| log::warn!("cod2 player {}: {e:#}", armed.body))
                                .ok()
                                .map(Rc::new)
                        })
                        .clone()
                        .map(|rig| {
                            let bone = |name: &str| {
                                rig.skeleton
                                    .bones()
                                    .iter()
                                    .position(|b| b.name.eq_ignore_ascii_case(name))
                            };
                            let tags = [bone("tag_flash"), bone("j_head"), bone("tag_brass")];
                            (rig, tags)
                        });
                    self.held
                        .insert(p.number, (character.clone(), p.weapon.clone(), rig.clone()));
                    rig
                }
            };
            let Some((rig, [flash, head, brass])) = rig else {
                continue;
            };
            let body = self.bodies.entry(p.number).or_insert_with(|| Body {
                character: character.clone(),
                body: PlayerBody::default(),
                seen_ms: render_time,
            });
            if body.character != *character {
                body.character = character.clone();
                body.body = PlayerBody::default();
            }
            let unseen = render_time
                .wrapping_sub(frametime_ms)
                .wrapping_sub(body.seen_ms);
            if unseen > 0 {
                body.body.pause(unseen);
            }
            body.seen_ms = render_time;
            let at = Times {
                clock_ms: render_time,
                snapshot_ms,
            };
            body.body
                .advance(&set.anims, &p.frame, at, frametime_ms, self.swing_speed);
            // A gunner stands where his gun's placement puts him, turned
            // with it; the height stays the server's trace.
            let mut frame = p.frame;
            let placed = p.gun.as_ref().and_then(|gun| {
                let tags = *self.gun_tags.entry(gun.model.clone()).or_insert_with(|| {
                    let bones = vcod_common::model::load_bones(fs, &gun.model).ok()?;
                    let tag = |n: &str| bones.iter().find(|b| b.name == n).map(|b| b.pos);
                    Some((tag("tag_aim")?, tag("tag_weapon")?))
                });
                let inc = *self
                    .rotate_incs
                    .entry(gun.weapon.clone())
                    .or_insert_with(|| {
                        vcod_common::weapon::load(fs, &gun.weapon)
                            .map_or(0.0, |w| w.anim_hor_rotate_inc)
                    });
                gunner_placement(
                    vcod_common::turretpose::GunnerRule::Cod1,
                    set,
                    fs,
                    &p.frame,
                    gun,
                    tags?,
                    inc,
                )
            });
            if let Some(g) = placed.as_ref().filter(|g| g.origin.is_finite()) {
                frame.origin = Vec3::new(g.origin.x, g.origin.y, frame.origin.z);
                frame.angles = [0.0, g.yaw(), 0.0];
            }
            let turret = placed.as_ref().map(|g| (g.node, g.rows.as_slice()));
            let pose = body.body.pose(&rig, set, fs, &frame, render_time, turret);
            let drawn = posed(&rig, &pose, &frame);
            if let Some(b) = flash {
                let (at, _) = pose.bone_world(&rig.skeleton, b);
                self.muzzles
                    .insert(p.number, drawn.transform.transform_point3(at));
            }
            if let Some(b) = head {
                let (at, _) = pose.bone_world(&rig.skeleton, b);
                self.heads
                    .insert(p.number, drawn.transform.transform_point3(at));
            }
            let tag = |b: Option<usize>| {
                Some(tag_pose(
                    drawn.transform,
                    pose.bone_world(&rig.skeleton, b?),
                ))
            };
            self.tags.insert(
                p.number,
                BodyTags {
                    flash: tag(flash),
                    brass: tag(brass),
                },
            );
            out.push((i, rig, drawn));
        }
        out
    }

    /// The posed turrets' `tag_flash` poses by entity, beside the bodies'
    /// muzzles and tags: where a turret's flash and tracers start.
    pub fn add_guns(&mut self, guns: &[(u32, TagPose)]) {
        for &(n, flash) in guns {
            self.muzzles.insert(n, flash.origin);
            self.tags.insert(
                n,
                BodyTags {
                    flash: Some(flash),
                    brass: None,
                },
            );
        }
    }

    /// [`Self::posed`] as the renderer's dynamic model instances, each part
    /// uploaded on first use, and each body as a shadow cookie caster over
    /// its parts (`CG_Player` adds one refEntity at the body's origin).
    pub fn build_instances(
        &mut self,
        renderer: &mut Renderer,
        fs: &Pk3Fs,
        players: &[Cod2PlayerView],
        (render_time, frametime_ms, snapshot_ms): (i32, i32, i32),
    ) -> (Vec<DynamicModelInstance>, Vec<CookieEntity>) {
        let posed = self.posed(fs, players, render_time, frametime_ms, snapshot_ms);
        let mut out = Vec::new();
        let mut casters = Vec::new();
        for (i, rig, drawn) in posed {
            let first = out.len();
            let light_origin = players[i].frame.light_origin();
            for ((name, model), (_, skins)) in rig.models.iter().zip(drawn.models) {
                let handle = *self.handles.entry(name.clone()).or_insert_with(|| {
                    let h = renderer.upload_dynamic_model(fs, model);
                    if h.is_none() {
                        log::warn!("cod2 player part '{name}' uploaded no geometry");
                    }
                    h
                });
                let Some(model) = handle else {
                    continue;
                };
                out.push(DynamicModelInstance {
                    model,
                    transform: drawn.transform,
                    bones: Some(skins),
                    light_origin,
                    shader_time: None,
                });
            }
            casters.push(CookieEntity {
                origin: players[i].frame.origin,
                parts: first..out.len(),
            });
        }
        (out, casters)
    }
}

/// A bone's model-space pose under `model` as a world [`TagPose`]: its
/// origin and its axes `[forward, left, up]`.
pub fn tag_pose(model: Mat4, (at, rot): (Vec3, Quat)) -> TagPose {
    let (_, turn, _) = model.to_scale_rotation_translation();
    let q = turn * rot;
    TagPose {
        origin: model.transform_point3(at),
        axis: [q * Vec3::X, q * Vec3::Y, q * Vec3::Z],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Another client on mp_carentan's mg42 (the retail turret capture,
    /// cod2-turrets.md 10) is drawn where retail's server put him: each
    /// mounted snapshot's gunner origin, placed off the gun's barrel and
    /// the body's height a snapshot before, matches in the plane.
    #[test]
    fn a_remote_gunner_stands_where_retail_placed_him() {
        let Some(fs) = vcod_common::testing::cod2_fs() else {
            return;
        };
        let text = include_str!("../../cod2/tests/fixtures/turret/carentan-mg42.txt");
        let hex = |v: &str| f32::from_bits(u32::from_str_radix(v, 16).unwrap());
        // (eFlags, legsAnim, origin, barrel) per snapshot.
        let mut rows: Vec<(i32, i32, Vec3, [f32; 3])> = Vec::new();
        let (mut ef, mut legs, mut o, mut b) = (0, 0, Vec3::ZERO, [0.0; 3]);
        for line in text.lines() {
            let mut w = line.split_whitespace();
            let tag = w.next();
            let gun = tag == Some("e") && w.next() == Some("295");
            if tag != Some("s") && !gun {
                continue;
            }
            for kv in w {
                let Some((k, v)) = kv.split_once('=') else {
                    continue;
                };
                match (gun, k) {
                    (false, "eFlags") => ef = i32::from_str_radix(v, 16).unwrap(),
                    (false, "legsAnim") => legs = i32::from_str_radix(v, 16).unwrap(),
                    (false, "origin[0]") => o.x = hex(v),
                    (false, "origin[1]") => o.y = hex(v),
                    (false, "origin[2]") => o.z = hex(v),
                    (true, "angles2[0]") => b[0] = hex(v),
                    (true, "angles2[1]") => b[1] = hex(v),
                    _ => {}
                }
            }
            match gun {
                false => rows.push((ef, legs, o, b)),
                true => rows.last_mut().unwrap().3 = b,
            }
        }
        let mut players = Cod2Players::new();
        let (mut checked, mut moved) = (0, 0);
        for pair in rows.windows(2) {
            let (before, (ef, legs, at, _)) = (pair[0], pair[1]);
            if ef & 0x300 == 0 || before.0 & 0x300 == 0 {
                continue;
            }
            let view = Cod2PlayerView {
                number: 3,
                frame: BodyFrame {
                    legs_anim: legs,
                    eflags: ef,
                    origin: before.2,
                    ..Default::default()
                },
                character: None,
                team: 1,
                weapon: None,
                gun: Some(GunFrame {
                    model: "weapon_mg42".into(),
                    weapon: "mg42_bipod_stand_mp".into(),
                    origin: Vec3::new(1718.0, 1824.0, 5.0),
                    angles: [0.0, -126.9, 0.0],
                    barrel: before.3,
                }),
            };
            let posed = players.posed(&fs, &[view], 1000, 50, 1000);
            let drawn = posed[0].2.transform.w_axis.truncate();
            let off = (drawn - at).truncate().length();
            assert!(off < 0.05, "{drawn} vs retail {at}, off {off}");
            checked += 1;
            moved += usize::from((before.2 - at).truncate().length() > 0.5);
        }
        assert!(checked > 100 && moved >= 3, "{checked} {moved}");
    }

    /// An armed body's `tag_flash` is its muzzle: ahead of it at about
    /// chest height, turned with its yaw.
    #[test]
    fn an_armed_body_reports_its_muzzle() {
        let Some(fs) = vcod_common::testing::cod2_fs() else {
            return;
        };
        let mut scene = Cod2Players::new();
        let view = |yaw: f32| Cod2PlayerView {
            number: 4,
            frame: BodyFrame {
                legs_anim: 127,
                origin: Vec3::new(100.0, 200.0, 0.0),
                angles: [0.0, yaw, 0.0],
                ..Default::default()
            },
            character: None,
            team: 1,
            weapon: Some("kar98k_mp".into()),
            gun: None,
        };
        for (t, yaw) in [(0, 0.0), (1, 90.0)] {
            scene.posed(&fs, &[view(yaw)], 1000 + t * 50, 50, 1000 + t * 50);
            let m = scene.muzzles[&4] - Vec3::new(100.0, 200.0, 0.0);
            println!("yaw {yaw}: muzzle {m}");
            assert!((20.0..70.0).contains(&m.z), "{m}");
            let ahead = if yaw == 0.0 { m.x } else { m.y };
            assert!(ahead > 10.0, "{m}");
        }
        scene.posed(&fs, &[], 1200, 50, 1200);
        assert!(scene.muzzles.is_empty());
        assert!(scene.tags.is_empty());
    }

    /// An armed body's `tag_flash` pose is its muzzle, pointing the way he
    /// faces, and its `tag_brass` sits back along the gun; the stock world
    /// flash played on that pose draws at the muzzle.
    #[test]
    fn an_armed_body_poses_its_flash_and_brass_tags() {
        let Some(fs) = vcod_common::testing::cod2_fs() else {
            return;
        };
        let mut scene = Cod2Players::new();
        for (weapon, yaw) in [("kar98k_mp", 0.0f32), ("mp40_mp", 90.0), ("bar_mp", 200.0)] {
            let view = Cod2PlayerView {
                number: 4,
                frame: BodyFrame {
                    legs_anim: 127,
                    origin: Vec3::new(100.0, 200.0, 0.0),
                    angles: [0.0, yaw, 0.0],
                    ..Default::default()
                },
                character: None,
                team: 1,
                weapon: Some(weapon.into()),
                gun: None,
            };
            scene.posed(&fs, &[view], 1000, 50, 1000);
            let t = scene.tags[&4];
            let (flash, brass) = (t.flash.unwrap(), t.brass.expect(weapon));
            assert_eq!(flash.origin, scene.muzzles[&4]);
            let ahead = Vec3::new(yaw.to_radians().cos(), yaw.to_radians().sin(), 0.0);
            assert!(flash.axis[0].dot(ahead) > 0.7, "{weapon}: {:?}", flash.axis);
            let back = (flash.origin - brass.origin).dot(ahead);
            assert!((5.0..40.0).contains(&back), "{weapon}: brass {back} behind");
            let mut fx = crate::fx::sim::FxSystem::new();
            let path = vcod_cod2::weapon::load(&fs, weapon)
                .unwrap()
                .def
                .str("worldFlashEffect")
                .to_string();
            let bolt = crate::fx::sim::TagBolt {
                entity: 4,
                slot: -1,
            };
            fx.spawn_on_tag(&fs, &path, bolt, flash, 0.0);
            fx.step(0.01, 0.01, None);
            let quads = fx.build_quads(Vec3::new(0.0, 0.0, 60.0), Vec3::X, Vec3::Z, 0.01);
            assert!(!quads.is_empty(), "{weapon}: {path} drew nothing");
            for q in &quads {
                for v in q.verts {
                    let d = Vec3::from(v).distance(flash.origin);
                    assert!(d < 40.0, "{weapon}: {path} vertex {d} from the muzzle");
                }
            }
        }
    }

    /// A client whose state names no model draws its team's default; both
    /// pose to finite bones, and a body that leaves is forgotten.
    #[test]
    fn players_pose_with_and_without_a_named_character() {
        let Some(fs) = vcod_common::testing::cod2_fs() else {
            return;
        };
        let mut scene = Cod2Players::new();
        let frame = |legs| BodyFrame {
            legs_anim: legs,
            ..Default::default()
        };
        let named = Character::load(&fs, "mp_russian_coat").unwrap();
        let players = [
            Cod2PlayerView {
                number: 0,
                frame: frame(127),
                character: Some(named),
                team: 2,
                weapon: None,
                gun: None,
            },
            Cod2PlayerView {
                number: 3,
                frame: frame(153),
                character: None,
                team: 1,
                weapon: None,
                gun: None,
            },
        ];
        for t in 0..5 {
            let posed = scene.posed(&fs, &players, 1000 + t * 50, 50, 1000 + t * 50);
            assert_eq!(posed.iter().map(|p| p.0).collect::<Vec<_>>(), [0, 1]);
            for (_, _, p) in &posed {
                assert!(
                    p.models
                        .iter()
                        .all(|(_, m)| m.iter().all(|m| m.is_finite()))
                );
            }
        }
        assert_eq!(scene.bodies.len(), 2);
        scene.posed(&fs, &players[..1], 1300, 50, 1300);
        assert_eq!(scene.bodies.len(), 1);
    }

    /// A fresh corpse new in the frame its client dies takes over the live
    /// body (`CG_ResetEntity`'s tree clone), so it blends out of the live
    /// clip; one whose client has no body drawn starts cold.
    #[test]
    fn fresh_corpse_takes_over_its_clients_body() {
        let Some(fs) = vcod_common::testing::cod2_fs() else {
            return;
        };
        let fresh = vcod_cod2::player::eflags::FRESH_CORPSE | vcod_cod2::player::eflags::DEAD;
        let view = |number, legs_anim, eflags, client_num| Cod2PlayerView {
            number,
            frame: BodyFrame {
                legs_anim,
                eflags,
                client_num,
                ..Default::default()
            },
            character: None,
            team: 1,
            weapon: None,
            gun: None,
        };
        let mut scene = Cod2Players::new();
        for t in 0..4 {
            scene.posed(&fs, &[view(2, 127, 0, 2)], 1000 + t * 50, 50, 1000 + t * 50);
        }
        // The client's body leaves as its corpse arrives; a second corpse
        // names a client nobody draws.
        let bones = |posed: &[(usize, Rc<PlayerRig>, PosedPlayer)], i: usize| {
            posed.iter().find(|p| p.0 == i).unwrap().2.models[0]
                .1
                .clone()
        };
        let posed = scene.posed(
            &fs,
            &[view(70, 153, fresh, 2), view(71, 153, fresh, 5)],
            1200,
            50,
            1200,
        );
        assert_eq!(scene.bodies.len(), 2);
        assert_ne!(bones(&posed, 0), bones(&posed, 1));
        let mut cold = Cod2Players::new();
        let alone = cold.posed(&fs, &[view(71, 153, fresh, 5)], 1200, 50, 1200);
        assert_eq!(bones(&alone, 0), bones(&posed, 1));
    }
}
