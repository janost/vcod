//! A CoD2 server's snapshot as draws: what each non-player entity is
//! (`CG_ProcessEntity`, Mac 0x164bd4) and which xmodel or brush model it
//! draws, and the player bodies handed to [`Cod2Players`]
//! (docs/research/cod2-protocol.md 12), a turret's aim and pose (12.4).
//! Everything is placed by [`Frame::entity_place`], which snaps across a
//! teleport (12.3); [`Cod2Draw`] is what the CoD2 session runs each frame.

use std::collections::HashMap;
use std::rc::Rc;
use vcod_common::asset_source::AssetSource;

use glam::{Mat4, Vec3};
use vcod_cod2::net::delta::field_index;
use vcod_cod2::net::events::{
    ET_CORPSE, ET_GENERAL, ET_ITEM, ET_MISSILE, ET_PLAYER, ET_SCRIPTMOVER, ET_TURRET,
};
use vcod_cod2::net::fields::CLIENT_STATE_FIELDS;
use vcod_cod2::net::flags::{EF_FIRING, EF_TURRET_STANCE};
use vcod_cod2::net::protocol::PROTOCOL_118;
use vcod_cod2::net::{ConfigStrings, EntityView, Frame};
use vcod_cod2::player::{BodyFrame, Character};
use vcod_common::net::FogParams;
use vcod_common::pk3::Pk3Fs;
use vcod_common::pmove::aim::angle_subtract;
use vcod_common::skeleton::{AnimBinding, PoseBuffer, Skeleton};
use vcod_common::turretpose::angles_quat;
use vcod_common::weapon::WeaponDef;
use vcod_common::xanim::XAnim;

use crate::cod2_players::{Cod2PlayerView, Cod2Players, tag_pose};
use crate::entities::resolve_model;
use crate::fx::sim::TagPose;
use crate::renderer::{CookieEntity, DynamicModelInstance, ModelHandle, Renderer};
use crate::turret::{ANIM_BLEND_MS, GunAnim, apply_controller};
use vcod_cod2::player::GunFrame;

/// `eFlags` bit every model draw skips on (`CG_General`'s `& 0x20`).
const EF_NODRAW: i32 = 0x20;
/// A dead player's `eFlags` (and a corpse's, `PlayerCmd_ClonePlayer`).
const EF_DEAD: i32 = vcod_cod2::player::eflags::DEAD;
/// A brush model's packed `solid`.
const SOLID_BMODEL: i32 = 0xff_ffff;
/// `bg_itemlist` rows 1..=128 are the weapons by their CS 7 index.
const MAX_WEAPON_ITEMS: i32 = 128;
/// `bg_itemlist`'s rows past the weapons (`cod2_lnxded` .data 0x81850c0,
/// `bg_numItems` 131): their world model, `xmodel/` stripped.
const ITEM_MODELS: [(i32, &str); 2] = [(129, "health_small"), (130, "health_large")];

/// What a non-player entity draws as.
#[derive(Clone, Debug, PartialEq)]
pub enum Visual2 {
    /// A script model or a script mover's xmodel, `xmodel/` stripped.
    Model(String),
    /// A brush model: its inline submodel number (`*N`).
    Submodel(usize),
    /// A weapon lying on the ground: its CS 7 name; the weapon file's
    /// `worldModel` draws.
    Item(String),
    /// A flying missile: its weapon's CS 7 name; the `projectileModel` draws.
    Missile(String),
    /// A mounted gun's xmodel, posed by its [`TurretAim`].
    Turret(String),
}

/// What `CG_mg42_DoControllers` (Mac 0x1631ac) poses a mounted gun by.
#[derive(Clone, Debug, PartialEq)]
pub struct TurretAim {
    /// Its CS 7 weapon, whose `idleAnim` and `fireAnim` it plays.
    pub weapon: Option<String>,
    /// `tag_aim` and `tag_aim_animated`'s pitch and yaw, `tag_flash`'s pitch.
    pub barrel: [f32; 3],
    pub anim: GunAnim,
}

impl TurretAim {
    /// The newest state's `angles2`, unlerped; the gun the view rides
    /// (`eFlags & 0x300` and `viewlocked_entNum` on the playerstate) aims
    /// where the view looks off the gun's drawn `angles`, and idles. Any
    /// other plays its fire anim while `eFlags & 0x40`.
    pub fn of(e: &EntityView, frame: &Frame, angles: Vec3) -> TurretAim {
        let a2 = |i: usize| e.raw.f32(&format!("angles2[{i}]"));
        let ps = &frame.snap.ps;
        let ridden =
            ps.eflags & EF_TURRET_STANCE != 0 && ps.raw.i32("viewlocked_entNum") == e.number as i32;
        let (barrel, anim) = if ridden {
            let v = ps.viewangles;
            (
                [
                    angle_subtract(v[0], angles.x),
                    angle_subtract(v[1], angles.y),
                    a2(2),
                ],
                GunAnim::Idle,
            )
        } else if e.eflags & EF_FIRING != 0 {
            ([a2(0), a2(1), a2(2)], GunAnim::Fire)
        } else {
            ([a2(0), a2(1), a2(2)], GunAnim::Idle)
        };
        TurretAim {
            weapon: weapon_at(frame.configstrings, e.weapon),
            barrel,
            anim,
        }
    }
}

/// One entity this frame.
#[derive(Clone, Debug, PartialEq)]
pub struct Placed2 {
    pub number: u32,
    pub visual: Visual2,
    pub origin: Vec3,
    /// Pitch, yaw, roll in degrees, `AnglesToAxis` order.
    pub angles: Vec3,
    /// A [`Visual2::Turret`]'s aim.
    pub turret: Option<TurretAim>,
}

/// The model at 1-based `index` in the model range, `xmodel/` stripped.
fn model_at(cs: &ConfigStrings, index: i32) -> Option<String> {
    let s = cs.model(index)?.replace('\\', "/");
    let name = s.strip_prefix("xmodel/").unwrap_or(&s);
    (!name.is_empty()).then(|| name.to_string())
}

fn weapon_at(cs: &ConfigStrings, index: i32) -> Option<String> {
    cs.weapon_name(index).map(String::from)
}

/// What `e` draws as at `render_time`, `None` for what draws no model:
/// players and corpses (the player path's), invisible, sound blend, looping
/// fx and event entities, and anything flagged no-draw.
pub fn resolve(e: &EntityView, cs: &ConfigStrings, render_time: i32) -> Option<Visual2> {
    if e.eflags & EF_NODRAW != 0 {
        return None;
    }
    let index = e.model_index;
    match e.etype {
        ET_GENERAL => model_at(cs, index).map(Visual2::Model),
        // `CG_ScriptMover` (Mac 0x164972): a brush model's `index` is its
        // inline model, anything else an xmodel.
        ET_SCRIPTMOVER if e.raw.i32("solid") == SOLID_BMODEL => {
            (index > 0).then_some(Visual2::Submodel(index as usize))
        }
        ET_SCRIPTMOVER => model_at(cs, index).map(Visual2::Model),
        // `CG_Item` (Mac 0x164550): `index` into `bg_itemlist`.
        ET_ITEM if index <= MAX_WEAPON_ITEMS => weapon_at(cs, index).map(Visual2::Item),
        ET_ITEM => ITEM_MODELS
            .iter()
            .find(|(row, _)| *row == index)
            .map(|(_, m)| Visual2::Model(m.to_string())),
        // `CG_Missile` (Mac 0x1646da) waits out `time`.
        ET_MISSILE if e.raw.i32("time") <= render_time => {
            weapon_at(cs, e.weapon).map(Visual2::Missile)
        }
        ET_TURRET => model_at(cs, index).map(Visual2::Turret),
        _ => None,
    }
}

/// Every non-player entity of `frame`'s newest snapshot that draws, placed
/// at the render time ([`Frame::entity_place`]).
pub fn place(frame: &Frame) -> Vec<Placed2> {
    frame
        .snap
        .entities
        .iter()
        .filter_map(|e| {
            let visual = resolve(e, frame.configstrings, frame.time_ms)?;
            let (origin, angles) = frame.entity_place(e);
            let turret =
                matches!(visual, Visual2::Turret(_)).then(|| TurretAim::of(e, frame, angles));
            Some(Placed2 {
                number: e.number,
                visual,
                origin,
                angles,
                turret,
            })
        })
        .collect()
}

/// The bodies of `frame`'s newest snapshot: every live player entity but
/// the view's own (free or following, retail draws no body around the eye)
/// and every corpse, lerped, each with its client's character, team and
/// weapon in hand.
pub fn players(frame: &Frame) -> Vec<Cod2PlayerView> {
    let snap = frame.snap;
    let team_field = field_index(&CLIENT_STATE_FIELDS, "team");
    snap.entities
        .iter()
        .filter(|e| match e.etype {
            // `CG_Player` (Mac 0x1e6834) draws no dead body; the corpse
            // (`CG_Corpse`, 0x1e78ea) does, whatever its flags.
            ET_PLAYER => e.number as i32 != snap.ps.client_num && e.eflags & EF_DEAD == 0,
            ET_CORPSE => true,
            _ => false,
        })
        .filter(|e| e.eflags & EF_NODRAW == 0)
        .map(|e| {
            let (origin, angles) = frame.entity_place(e);
            let mut body = BodyFrame::from_entity(&e.raw);
            body.origin = origin;
            body.angles = angles.to_array();
            let client = snap
                .clients
                .iter()
                .find(|c| c.number as i32 == e.client_num);
            Cod2PlayerView {
                number: e.number,
                frame: body,
                character: client.and_then(|c| {
                    Character::from_client_state(c, |i| Some(frame.configstrings.get(i.into())))
                }),
                team: client.map_or(0, |c| c.f[team_field] as i32),
                // `BG_UpdatePlayerDObj` (Mac 0x182312) holds no weapon on a
                // gun's mount.
                weapon: (e.eflags & EF_TURRET_STANCE == 0)
                    .then(|| weapon_at(frame.configstrings, e.weapon))
                    .flatten(),
                gun: gun_ridden(frame, e),
            }
        })
        .collect()
}

/// The gun `e` mans: `CG_Player` (Mac 0x1e6834) places a body with
/// `eFlags & 0x300` whose `otherEntityNum` is past the clients, not 1023,
/// and a turret in the snapshot. Its barrel is the newest state's.
fn gun_ridden(frame: &Frame, e: &EntityView) -> Option<GunFrame> {
    if e.eflags & EF_TURRET_STANCE == 0 {
        return None;
    }
    let n = e.raw.i32("otherEntityNum");
    if !(64..1023).contains(&n) {
        return None;
    }
    let g = frame
        .snap
        .entity(n as u32)
        .filter(|g| g.etype == ET_TURRET)?;
    let (origin, angles) = frame.entity_place(g);
    let a2 = |i: usize| g.raw.f32(&format!("angles2[{i}]"));
    Some(GunFrame {
        model: model_at(frame.configstrings, g.model_index)?,
        weapon: weapon_at(frame.configstrings, g.weapon)?,
        origin,
        angles: angles.to_array(),
        barrel: [a2(0), a2(1), a2(2)],
    })
}

/// This frame's draws: the dynamic model instances and the brush models
/// with their poses, for `Renderer::set_dynamic_models` and
/// `Renderer::set_submodels`.
#[derive(Default)]
pub struct Built2 {
    pub instances: Vec<DynamicModelInstance>,
    pub submodels: Vec<(usize, Mat4)>,
    /// Each posed gun's world `tag_flash`, by entity.
    pub muzzles: Vec<(u32, TagPose)>,
}

/// A mounted gun's skeleton and its clips' bindings.
struct GunRig {
    skeleton: Skeleton,
    bindings: HashMap<String, AnimBinding>,
}

/// One gun's anim across frames: the slot it blends to and the one it
/// leaves.
#[derive(Clone, Copy)]
struct GunClock {
    anim: GunAnim,
    start_ms: i32,
    prev: Option<(GunAnim, i32)>,
}

/// Uploaded models and loaded weapon files across frames. Its handles die
/// with the map: make a new one after `Renderer::unload_world`.
#[derive(Default)]
pub struct Cod2Scene {
    models: HashMap<String, Option<ModelHandle>>,
    weapons: HashMap<String, Option<WeaponDef>>,
    gun_rigs: HashMap<String, Option<GunRig>>,
    clips: HashMap<String, Option<Rc<XAnim>>>,
    guns: HashMap<u32, GunClock>,
}

impl Cod2Scene {
    /// The xmodel `visual` draws, `None` for a brush model or a weapon file
    /// that names none.
    pub fn model_name(&mut self, visual: &Visual2, fs: &dyn AssetSource) -> Option<String> {
        let name = match visual {
            Visual2::Model(m) | Visual2::Turret(m) => Some(m.clone()),
            Visual2::Item(w) => self.weapon(fs, w)?.world_model.clone(),
            Visual2::Missile(w) => self.weapon(fs, w)?.projectile_model.clone(),
            Visual2::Submodel(_) => None,
        };
        name.filter(|n| !n.is_empty())
    }

    fn weapon(&mut self, fs: &dyn AssetSource, name: &str) -> Option<&WeaponDef> {
        self.weapons
            .entry(name.to_string())
            .or_insert_with(|| match vcod_common::weapon::load(fs, name) {
                Ok(d) => Some(d),
                Err(e) => {
                    log::warn!("CoD2 weapon '{name}': {e:#}");
                    None
                }
            })
            .as_ref()
    }

    /// The gun `p` places, posed at `now_ms`: its weapon's idle or fire
    /// clip, blended in over 0.1 s, then the controllers
    /// ([`crate::turret::apply_controller`]; the tags and the order are
    /// `CG_mg42_DoControllers`'). `None` for a model that does not load.
    /// Guns not in `placed` this frame are forgotten by [`Self::forget_guns`].
    pub fn gun_pose(
        &mut self,
        fs: &dyn AssetSource,
        p: &Placed2,
        now_ms: i32,
    ) -> Option<(PoseBuffer, &Skeleton)> {
        let (Visual2::Turret(model), Some(aim)) = (&p.visual, &p.turret) else {
            return None;
        };
        let (idle, fire) = match aim.weapon.as_deref() {
            Some(w) => self
                .weapon(fs, w)
                .map_or((None, None), |d| (d.idle_anim.clone(), d.fire_anim.clone())),
            None => (None, None),
        };
        let clock = self.guns.entry(p.number).or_insert(GunClock {
            anim: aim.anim,
            start_ms: now_ms,
            prev: None,
        });
        if clock.anim != aim.anim {
            clock.prev = Some((clock.anim, clock.start_ms));
            clock.anim = aim.anim;
            clock.start_ms = now_ms;
        }
        let fade = (now_ms - clock.start_ms) as f32 / ANIM_BLEND_MS as f32;
        if fade >= 1.0 {
            clock.prev = None;
        }
        let clock = *clock;
        let rig = self
            .gun_rigs
            .entry(model.clone())
            .or_insert_with(|| match vcod_common::model::load(fs, model) {
                Ok(m) => Some(GunRig {
                    skeleton: Skeleton::build(&[&m]),
                    bindings: HashMap::new(),
                }),
                Err(e) => {
                    log::warn!("CoD2 turret '{model}': {e:#}");
                    None
                }
            })
            .as_mut()?;
        let mut pose = PoseBuffer::new(&rig.skeleton);
        let layers = clock
            .prev
            .map(|(a, start)| (a, start, 1.0))
            .into_iter()
            .chain([(
                clock.anim,
                clock.start_ms,
                if clock.prev.is_some() {
                    fade.max(0.0)
                } else {
                    1.0
                },
            )]);
        for (anim, start, w) in layers {
            let name = match anim {
                GunAnim::Idle => idle.as_deref(),
                GunAnim::Fire => fire.as_deref(),
            };
            let Some(name) = name else { continue };
            let clip = self
                .clips
                .entry(name.to_string())
                .or_insert_with(|| {
                    vcod_common::xanim::load(fs, name)
                        .inspect_err(|e| log::warn!("CoD2 turret anim '{name}': {e:#}"))
                        .ok()
                        .map(Rc::new)
                })
                .clone();
            let Some(clip) = clip else { continue };
            let binding = rig
                .bindings
                .entry(name.to_string())
                .or_insert_with(|| rig.skeleton.bind(&clip));
            let t = (now_ms - start).max(0) as f32 / 1000.0;
            pose.apply_weighted(&clip, binding, clip.frame_pos(t, clip.looping), w);
        }
        apply_controller(&mut pose, &rig.skeleton, aim.barrel);
        Some((pose, &rig.skeleton))
    }

    /// Drops the anim clocks of guns `placed` no longer holds.
    fn forget_guns(&mut self, placed: &[Placed2]) {
        self.guns
            .retain(|n, _| placed.iter().any(|p| p.number == *n && p.turret.is_some()));
    }

    pub fn build(
        &mut self,
        placed: &[Placed2],
        fs: &Pk3Fs,
        renderer: &mut Renderer,
        now_ms: i32,
    ) -> Built2 {
        self.forget_guns(placed);
        let mut out = Built2::default();
        for p in placed {
            let transform =
                Mat4::from_rotation_translation(angles_quat(p.angles.to_array()), p.origin);
            if let Visual2::Submodel(n) = p.visual {
                // At rest where the map put it is the identity: the world
                // draws it with its own surfaces.
                let at_rest =
                    p.origin.abs().max_element() < 0.01 && p.angles.abs().max_element() < 0.01;
                out.submodels
                    .push((n, if at_rest { Mat4::IDENTITY } else { transform }));
                continue;
            }
            let Some(name) = self.model_name(&p.visual, fs) else {
                continue;
            };
            let Some(model) = resolve_model(&mut self.models, renderer, fs, &name) else {
                continue;
            };
            // `CG_ProcessEntity`'s turret arm sets a lighting origin 32 up
            // off the origin; the rest sample the grid 4 over theirs
            // (`RB_SetupEntityLighting`, Mac 0xec126).
            let light = match p.visual {
                Visual2::Turret(_) => p.origin + Vec3::Z * 32.0,
                _ => vcod_cod2::lightgrid::entity_point(p.origin),
            };
            let bones = self.gun_pose(fs, p, now_ms).map(|(pose, skel)| {
                if let Some(at) = gun_muzzle(&pose, skel) {
                    out.muzzles.push((p.number, tag_pose(transform, at)));
                }
                pose.skin_matrices(skel, 0)
            });
            out.instances.push(DynamicModelInstance {
                model,
                transform,
                bones,
                light_origin: light,
                shader_time: None,
            });
        }
        out
    }
}

/// A posed gun's `tag_flash` in its model space, where a round it fires
/// draws its tracer from (`CG_CalcMuzzlePoint`, Mac 0x1d7f72, reads the
/// impact's entity's own tag: the gun's for a turret round;
/// cod2-turrets.md 12).
fn gun_muzzle(pose: &PoseBuffer, skel: &Skeleton) -> Option<(Vec3, glam::Quat)> {
    Some(pose.bone_world(skel, skel.bone_index("tag_flash")?))
}

/// What a CoD2 session draws across frames: the entity models and the
/// player bodies. Its handles die with the map, as [`Cod2Scene`]'s do.
#[derive(Default)]
pub struct Cod2Draw {
    scene: Cod2Scene,
    players: Cod2Players,
    /// The last frame's render time, for the bodies' frame time.
    last_ms: Option<i32>,
    /// The fog configstring last handed to the renderer.
    fog_cs: Option<String>,
}

/// What one frame drew.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DrawCounts {
    /// Entity xmodels (weapons on the ground, missiles, script models,
    /// turrets).
    pub models: usize,
    pub submodels: usize,
    /// Bodies: posed ([`Cod2Draw::count`]) or handed to the poser
    /// ([`Cod2Draw::draw`]).
    pub players: usize,
    /// Player part instances (body, head, helmet, gear, weapon).
    pub player_parts: usize,
    /// Turrets posed ([`Cod2Draw::count`] only).
    pub guns_posed: usize,
}

impl Cod2Draw {
    pub fn new() -> Self {
        Cod2Draw {
            players: Cod2Players::new(),
            ..Default::default()
        }
    }

    /// The draw for the next map on the same search path: the renderer
    /// dropped every model handle with the old map, so those go, with the
    /// bodies and the guns' clocks; the parsed weapon files, player rigs,
    /// anims and gun skeletons stay, as the next map draws the same ones.
    pub fn next_map(&mut self) {
        let Cod2Draw {
            scene,
            players,
            last_ms,
            fog_cs,
        } = self;
        scene.models.clear();
        scene.guns.clear();
        players.next_map();
        *last_ms = None;
        *fog_cs = None;
    }

    /// The last frame's drawn bodies' and guns' world `tag_flash` by
    /// entity, where their tracers start.
    pub fn muzzles(&self) -> &HashMap<u32, Vec3> {
        &self.players.muzzles
    }

    /// The last frame's drawn bodies' world `j_head` by entity.
    pub fn heads(&self) -> &HashMap<u32, Vec3> {
        &self.players.heads
    }

    /// The last frame's drawn bodies' `tag_flash` and `tag_brass` poses by
    /// entity.
    pub fn tags(&self) -> &HashMap<u32, crate::play::cod2_fx::BodyTags> {
        &self.players.tags
    }

    /// What the last frame's draw hands the effects, `brass` being
    /// `cg_brass`.
    pub fn fx_scene(&self, brass: bool) -> crate::play::cod2_fx::FxScene<'_> {
        crate::play::cod2_fx::FxScene {
            muzzles: &self.players.muzzles,
            tags: &self.players.tags,
            brass,
        }
    }

    /// The bodies' clock: (render time, frame time, newest snapshot time).
    fn clock(&mut self, frame: &Frame) -> (i32, i32, i32) {
        let now = frame.time_ms;
        let dt = self.last_ms.map_or(0, |l| (now - l).max(0));
        self.last_ms = Some(now);
        (now, dt, frame.snap.server_time)
    }

    /// The CPU half of [`Self::draw`], for the headless probe: what the
    /// frame places and poses, each model resolved to a name (not
    /// uploaded) and each body posed.
    pub fn count(&mut self, frame: &Frame, fs: &dyn AssetSource) -> DrawCounts {
        let placed = place(frame);
        let mut c = DrawCounts::default();
        let mut guns = Vec::new();
        self.scene.forget_guns(&placed);
        for p in &placed {
            match p.visual {
                Visual2::Submodel(_) => c.submodels += 1,
                _ if self.scene.model_name(&p.visual, fs).is_some() => c.models += 1,
                _ => {}
            }
            if let Some((pose, skel)) = self.scene.gun_pose(fs, p, frame.time_ms) {
                c.guns_posed += 1;
                if let Some(at) = gun_muzzle(&pose, skel) {
                    let transform =
                        Mat4::from_rotation_translation(angles_quat(p.angles.to_array()), p.origin);
                    guns.push((p.number, tag_pose(transform, at)));
                }
            }
        }
        let views = players(frame);
        let (now, dt, snap_ms) = self.clock(frame);
        let posed = self.players.posed(fs, &views, now, dt, snap_ms);
        c.players = posed.len();
        c.player_parts = posed.iter().map(|(_, rig, _)| rig.models.len()).sum();
        self.players.add_guns(&guns);
        c
    }

    /// Hands `frame`'s entities and bodies to the renderer: the brush
    /// models' poses and every model instance.
    /// The scene fog once configstring 12 changes (`CG_ParseFog`, Mac
    /// 0x1e273c): seven numbers set the fog and fade to it over the last,
    /// in ms; anything shorter switches it off. The colour lands as the
    /// bytes `R_SetFog` keeps (cod2-materials.md 9.1).
    fn fog_change(&mut self, cs: &ConfigStrings) -> Option<Option<FogParams>> {
        let s = cs.get(PROTOCOL_118.cs.fog);
        if self.fog_cs.as_deref() == Some(s) {
            return None;
        }
        self.fog_cs = Some(s.to_string());
        Some(FogParams::parse(s).map(|mut f| {
            f.color = f.color.map(|c| (c * 255.0 + 0.5).floor() / 255.0);
            f
        }))
    }

    /// Loads and uploads every model a level registers at its gamestate,
    /// so none loads in the frame its entity first shows (an sd bomb, a
    /// ctf flag): `CG_RegisterGraphics` (Mac 0x14374a) takes configstring
    /// models 1 to 255 up to the first empty one, and `CG_RegisterWeapon`
    /// (0x1d85cc) each CS 7 weapon's world and projectile models
    /// (cod2-client-perf.md, "The soak"). Every pipeline a stock model
    /// draws with is built first, so a later level's new model builds none
    /// (`COD2_MODEL_PIPELINES`). Returns the models loaded.
    pub fn preload(&mut self, cs: &ConfigStrings, fs: &Pk3Fs, r: &mut Renderer) -> usize {
        r.prewarm_entity_pipelines(&crate::entity_material::COD2_MODEL_PIPELINES);
        let scene = &mut self.scene;
        let mut names: Vec<String> = (1..256)
            .take_while(|&i| cs.model(i).is_some())
            .filter_map(|i| model_at(cs, i))
            .collect();
        for w in (1..).map_while(|i| cs.weapon_name(i)) {
            if let Some(d) = scene.weapon(fs, w) {
                names.extend(d.world_model.iter().chain(&d.projectile_model).cloned());
            }
        }
        names.retain(|n| !n.is_empty());
        crate::entities::resolve_models(&mut scene.models, r, fs, &names);
        names
            .iter()
            .filter(|n| resolve_model(&mut scene.models, r, fs, n).is_some())
            .count()
    }

    /// Whether `name` (the configstring's, `xmodel/` and all) is loaded.
    #[cfg(test)]
    pub(crate) fn model_loaded(&self, name: &str) -> bool {
        let name = name.strip_prefix("xmodel/").unwrap_or(name);
        self.scene.models.get(name).is_some_and(Option::is_some)
    }

    pub fn draw(&mut self, frame: &Frame, fs: &Pk3Fs, r: &mut Renderer) -> DrawCounts {
        use crate::frame_prof::{Count, Stage, span};
        let mut prof = span(Stage::Pose);
        let built = self.scene.build(&place(frame), fs, r, frame.time_ms);
        let views = players(frame);
        let clock = self.clock(frame);
        prof.next(Stage::Bodies);
        let (bodies, body_casters) = self.players.build_instances(r, fs, &views, clock);
        prof.next(Stage::Pose);
        self.players.add_guns(&built.muzzles);
        let counts = DrawCounts {
            models: built.instances.len(),
            submodels: built.submodels.len(),
            players: views.len(),
            player_parts: bodies.len(),
            guns_posed: 0,
        };
        // every model entity may cast a shadow cookie, the bodies over
        // their parts (cod2-materials.md 9.8)
        let mut casters: Vec<CookieEntity> = built
            .instances
            .iter()
            .enumerate()
            .map(|(i, inst)| CookieEntity {
                origin: inst.transform.w_axis.truncate(),
                parts: i..i + 1,
            })
            .collect();
        let at = built.instances.len();
        casters.extend(body_casters.into_iter().map(|c| CookieEntity {
            parts: c.parts.start + at..c.parts.end + at,
            ..c
        }));
        let mut instances = built.instances;
        instances.extend(bodies);
        crate::frame_prof::count(Count::Models, instances.len());
        crate::frame_digest::submodels(&built.submodels);
        crate::frame_digest::models(&instances);
        r.set_submodels(&built.submodels);
        r.set_dynamic_models(&instances);
        r.set_cookie_entities(casters);
        if let Some(fog) = self.fog_change(frame.configstrings) {
            r.set_fog_now(fog);
        }
        counts
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use vcod_cod2::net::capture::{Capture, Dir};
    use vcod_cod2::net::events::ET_FX;
    use vcod_cod2::net::oob::{self, Reply};
    use vcod_cod2::net::protocol::CS_WEAPONS;
    use vcod_cod2::net::{Client, NetView};

    const WEAPON_TAG_FOR_TEST: &str = "tag_weapon_right";

    const FIXTURE: &[u8] = include_bytes!("../../cod2/tests/fixtures/mp_toujane-spectate.bin");

    /// The fixture's session through a `Client` into the snapshot view the
    /// session draws from.
    pub(crate) fn replay() -> NetView {
        let cap = Capture::decode(FIXTURE).unwrap();
        let mut challenge = None;
        let mut client: Option<Client> = None;
        let mut view = NetView::default();
        for (dir, p) in &cap.packets {
            if let Some(pl) = oob::payload(p) {
                match dir {
                    Dir::FromServer => {
                        if let Reply::Challenge(c) = oob::parse_reply(pl) {
                            challenge = Some(c);
                        }
                    }
                    Dir::ToServer => {
                        let text = String::from_utf8_lossy(pl);
                        if let Some(ui) = text.strip_prefix("connect \"") {
                            let q = oob::info_value_for_key(ui.trim_end_matches('"'), "qport");
                            client =
                                Some(Client::new(q.unwrap().parse().unwrap(), challenge.unwrap()));
                        }
                    }
                }
            } else if *dir == Dir::FromServer {
                let cl = client.as_mut().unwrap();
                if let Some(parsed) = cl.receive(p).unwrap() {
                    view.update(&cl.parser, &parsed);
                }
            }
        }
        view
    }

    /// Half way between the last two snapshots, as the session draws.
    pub(crate) fn mid(view: &NetView) -> Frame<'_> {
        let (a, b) = (view.prev.as_ref().unwrap(), view.snap.as_ref().unwrap());
        view.frame((a.server_time + b.server_time) / 2).unwrap()
    }

    fn weapons(view: &NetView) -> Vec<String> {
        view.configstrings
            .get(CS_WEAPONS)
            .split(' ')
            .filter(|w| !w.is_empty())
            .map(String::from)
            .collect()
    }

    /// The retail capture's last frame as the session hands it over: the
    /// brush models, guns and flak wrecks placed, and the other client's
    /// body (entity 0; the probe, client 1, spectates free) given to the
    /// player poser with its client's character. With an install the body
    /// poses its body, head and helmet to finite bones.
    #[test]
    fn retail_toujane_frame_draws_entities_and_the_other_player() {
        let view = replay();
        let f = mid(&view);
        let placed = place(&f);
        let got: Vec<_> = placed
            .iter()
            .map(|p| (p.number, p.visual.clone()))
            .collect();
        let mg42 = || Visual2::Turret("weapon_mg42".into());
        let flak = || Visual2::Model("german_artillery_flak88_destroy".into());
        assert_eq!(
            got,
            vec![
                (84, Visual2::Submodel(1)),
                (308, mg42()),
                (311, mg42()),
                (368, flak()),
                (472, flak()),
                (510, Visual2::Submodel(16)),
                (512, Visual2::Submodel(18)),
                (517, mg42()),
            ]
        );
        // The rest: the other client's body and the looping effects.
        let snap = f.snap;
        let types: Vec<_> = snap
            .entities
            .iter()
            .filter(|e| !placed.iter().any(|p| p.number == e.number))
            .map(|e| e.etype)
            .collect();
        assert_eq!(types.iter().filter(|&&t| t == ET_FX).count(), 19);
        assert_eq!(types.iter().filter(|&&t| t == ET_PLAYER).count(), 1);
        assert_eq!(types.len(), 20);
        // Stationary: the gun stands on its spot turned by its yaw.
        let gun = placed.iter().find(|p| p.number == 308).unwrap();
        assert_eq!(gun.origin, Vec3::new(662.0, 24.0, 197.0));
        assert!((gun.angles.y - 82.89734).abs() < 1e-3);

        let bodies = players(&f);
        assert_eq!(bodies.len(), 1);
        let body = &bodies[0];
        assert_eq!(body.number, 0);
        let character = body.character.as_ref().expect("client 0 names a model");
        assert!(!character.body.is_empty());
        // Lerped half way, as `Frame::entity_place` puts it.
        let e = snap.entity(0).unwrap();
        assert_eq!(body.frame.origin, f.entity_origin(e));
        assert!(body.frame.origin.is_finite());

        let Some(fs) = vcod_common::testing::cod2_fs() else {
            return;
        };
        let mut poser = Cod2Players::new();
        let t = f.time_ms;
        let posed = poser.posed(&fs, &bodies, t, 0, snap.server_time);
        assert_eq!(posed.len(), 1);
        let (_, rig, drawn) = &posed[0];
        let parts: Vec<&str> = rig.models.iter().map(|(n, _)| n.as_str()).collect();
        assert!(parts.len() >= 3, "{parts:?}");
        assert!(parts.iter().any(|n| n.contains("head")), "{parts:?}");
        assert!(parts.iter().any(|n| n.contains("helmet")), "{parts:?}");
        assert!(drawn.transform.is_finite());
        for (_, bones) in &drawn.models {
            assert!(!bones.is_empty());
            assert!(bones.iter().all(|m| m.is_finite()));
        }
        // The CPU count the headless probe logs agrees.
        let mut draw = Cod2Draw::new();
        let c = draw.count(&f, &fs);
        assert_eq!(
            (c.models, c.submodels, c.players, c.player_parts),
            (5, 3, 1, parts.len())
        );
    }

    /// The fixture's three MG42s, unmanned: each aims by its newest
    /// state's `angles2`, idles, and names its CS 7 weapon. With an install
    /// each poses off its bind (the idle clip and the controllers) to finite
    /// bones, and the barrel turns `tag_flash` with the yaw.
    #[test]
    fn retail_toujane_turrets_aim_and_pose() {
        let view = replay();
        let f = mid(&view);
        let placed = place(&f);
        let guns: Vec<_> = placed.iter().filter(|p| p.turret.is_some()).collect();
        assert_eq!(
            guns.iter().map(|p| p.number).collect::<Vec<_>>(),
            [308, 311, 517]
        );
        // The capture's unmanned guns rest pitched up, where `turret_think`
        // steers them.
        let pitches: Vec<_> = guns
            .iter()
            .map(|g| g.turret.as_ref().unwrap().barrel)
            .collect();
        assert_eq!(
            pitches,
            [[-81.0, 0.0, 0.0], [-57.0, 0.0, 0.0], [-57.0, 0.0, 0.0]]
        );
        for g in &guns {
            let aim = g.turret.as_ref().unwrap();
            assert_eq!(aim.anim, GunAnim::Idle);
            assert!(aim.weapon.is_some());
        }
        assert!(
            placed
                .iter()
                .all(|p| p.turret.is_some() == matches!(p.visual, Visual2::Turret(_)))
        );

        let Some(fs) = vcod_common::testing::cod2_fs() else {
            return;
        };
        vcod_cod2::model::register();
        let mut scene = Cod2Scene::default();
        for g in &guns {
            let (pose, skel) = scene.gun_pose(&fs, g, f.time_ms).expect("the gun poses");
            let worlds = pose.bone_worlds(skel);
            let bind = PoseBuffer::new(skel).bone_worlds(skel);
            assert!(worlds.iter().all(|(p, q)| p.is_finite() && q.is_finite()));
            let flash = skel.bone_index("tag_flash").expect("tag_flash");
            // The barrel's pitch swings the muzzle 10 to 15 units.
            let moved = (worlds[flash].0 - bind[flash].0).length();
            assert!(moved > 5.0, "gun {} flash moved {moved}", g.number);
        }
        let mut draw = Cod2Draw::new();
        assert_eq!(draw.count(&f, &fs).guns_posed, 3);
        // Each gun's muzzle is where its rounds' tracers start.
        for g in &guns {
            let m = draw.muzzles()[&g.number];
            let d = m.distance(g.origin);
            assert!(d > 10.0 && d < 80.0, "gun {} muzzle {d} out", g.number);
            // And its flash's tag, pointing out of the barrel.
            let flash = draw.tags()[&g.number].flash.expect("the gun's flash tag");
            assert_eq!(flash.origin, m);
            assert!(flash.axis[0].dot((m - g.origin).normalize()) > 0.5);
        }
    }

    /// `bg_itemlist`'s two rows past the weapons resolve to their health
    /// models, the rows past `bg_numItems` to nothing; with an install both
    /// models load.
    #[test]
    fn health_item_rows_resolve_and_load() {
        let view = replay();
        let item = |index: u32| {
            let mut es = vcod_cod2::net::EntityState2::null(200);
            let mut set = |name: &str, v: u32| {
                es.f[field_index(&vcod_cod2::net::fields::ENTITY_STATE_FIELDS, name)] = v
            };
            set("eType", ET_ITEM as u32);
            set("index", index);
            resolve(&EntityView::new(&es), &view.configstrings, 0)
        };
        assert_eq!(item(129), Some(Visual2::Model("health_small".into())));
        assert_eq!(item(130), Some(Visual2::Model("health_large".into())));
        assert_eq!(item(131), None);
        let Some(fs) = vcod_common::testing::cod2_fs() else {
            return;
        };
        vcod_cod2::model::register();
        for (_, m) in ITEM_MODELS {
            if let Err(e) = vcod_common::model::load(&fs, m) {
                panic!("{m}: {e:#}");
            }
        }
    }

    /// The fixture's other client draws holding its CS 7 weapon: the
    /// weapon's `worldModel` is a part of its rig on `tag_weapon_right`, and
    /// every CS 7 weapon with a world model arms a body that poses finite.
    #[test]
    fn bodies_hold_their_weapon_world_model() {
        let view = replay();
        let f = mid(&view);
        let bodies = players(&f);
        let held = bodies[0].weapon.clone().expect("client 0 holds a weapon");
        assert!(weapons(&view).contains(&held), "{held}");
        let Some(fs) = vcod_common::testing::cod2_fs() else {
            return;
        };
        let mut scene = Cod2Scene::default();
        let mut poser = Cod2Players::new();
        let t = f.time_ms;
        let mut armed_with = 0;
        for w in weapons(&view) {
            let Some(world) = scene.model_name(&Visual2::Item(w.clone()), &fs) else {
                continue;
            };
            armed_with += 1;
            let mut armed = bodies.clone();
            armed[0].weapon = Some(w.clone());
            let posed = poser.posed(&fs, &armed, t, 0, f.snap.server_time);
            let (_, rig, drawn) = &posed[0];
            let part = rig.models.iter().position(|(n, _)| *n == world);
            let part = part.unwrap_or_else(|| panic!("{w}: no {world} in the rig"));
            let bones = &drawn.models[part].1;
            assert!(
                !bones.is_empty() && bones.iter().all(|m| m.is_finite()),
                "{w}"
            );
            assert!(
                rig.skeleton.bone_index(WEAPON_TAG_FOR_TEST).is_some(),
                "{w}"
            );
        }
        assert!(armed_with > 5, "{armed_with}");
    }

    /// Every model the fixture's entities resolve to, and every CS 7
    /// weapon's world and projectile model, loads from the install.
    #[test]
    fn resolved_models_load() {
        let Some(fs) = vcod_common::testing::cod2_fs() else {
            return;
        };
        vcod_cod2::model::register();
        let view = replay();
        let f = view.frame(view.snap.as_ref().unwrap().server_time).unwrap();
        let mut visuals: Vec<_> = place(&f).into_iter().map(|p| p.visual).collect();
        for w in weapons(&view) {
            visuals.push(Visual2::Item(w.clone()));
            visuals.push(Visual2::Missile(w));
        }
        let mut scene = Cod2Scene::default();
        let mut names: Vec<_> = visuals
            .iter()
            .filter_map(|v| scene.model_name(v, &fs))
            .collect();
        names.sort();
        names.dedup();
        assert!(names.iter().any(|n| n == "weapon_mg42"));
        assert!(names.iter().any(|n| n.starts_with("projectile_")));
        for n in &names {
            if let Err(e) = vcod_common::model::load(&fs, n) {
                panic!("{n}: {e:#}");
            }
        }
        // Every weapon file loaded; only the turret's gun and the binoculars
        // name no world model.
        let bare: Vec<_> = weapons(&view)
            .into_iter()
            .filter(|w| scene.model_name(&Visual2::Item(w.clone()), &fs).is_none())
            .collect();
        assert_eq!(bare, ["mg42_bipod_stand_mp", "binoculars_mp"]);
        assert!(scene.weapons.values().all(Option::is_some));
    }

    /// `turret_think_init` (Mac 0x1bc1f2): the first of the pitches 0, -3,
    /// ... -90 at which the segment from the gun's `tag_aim` to its
    /// `tag_butt`, the butt turned about `tag_aim` by that pitch, meets the
    /// world under mask 0x811; `None` leaves `G_SpawnTurret`'s -90
    /// (docs/research/cod2-protocol.md 12.4). The world only: retail's
    /// `G_LocationalTrace` sees entities too.
    fn drop_pitch(
        clip: &vcod_cod2::collision::ClipMap,
        origin: Vec3,
        angles: Vec3,
        aim: Vec3,
        butt: Vec3,
    ) -> Option<f32> {
        let axis = angles_quat(angles.to_array());
        let start = origin + axis * aim;
        (0..31).map(|i| i as f32 * -3.0).find(|&p| {
            let end = origin + axis * (aim + angles_quat([p, 0.0, 0.0]) * (butt - aim));
            clip.trace(start, end, Vec3::ZERO, Vec3::ZERO, 0x811)
                .fraction
                < 1.0
        })
    }

    /// Model-space `tag_aim` and `tag_butt` of `model` at its bind, where
    /// `turret_think_init` reads them.
    fn aim_and_butt(fs: &dyn AssetSource, model: &str) -> (Vec3, Vec3) {
        let m = vcod_common::model::load(fs, model).unwrap();
        let skel = Skeleton::build(&[&m]);
        let bind = PoseBuffer::new(&skel).bone_worlds(&skel);
        let at = |t: &str| bind[skel.bone_index(t).unwrap()].0;
        (at("tag_aim"), at("tag_butt"))
    }

    /// Pitch and yaw in degrees of a world forward, `AnglesToAxis`' sense.
    fn pitch_yaw(fwd: Vec3) -> (f32, f32) {
        (-fwd.z.asin().to_degrees(), fwd.y.atan2(fwd.x).to_degrees())
    }

    /// Poses `gun` as the scene draws it and checks the pose against
    /// `CG_mg42_DoControllers`: the bipod (`tag_pivot`, the bone the base
    /// mesh skins to) stands level, within the idle clip's 1 degree, the
    /// gun's body (`tag_aim_animated`) points along the entity's yaw at
    /// `angles2`' pitch, and the butt, swung about `tag_aim` with it,
    /// touches the world.
    fn check_resting_gun(
        scene: &mut Cod2Scene,
        fs: &dyn AssetSource,
        clip: &vcod_cod2::collision::ClipMap,
        gun: &Placed2,
        time_ms: i32,
    ) {
        let barrel = gun.turret.as_ref().unwrap().barrel;
        let (pose, skel) = scene.gun_pose(fs, gun, time_ms).expect("the gun poses");
        let worlds = pose.bone_worlds(skel);
        let axis = angles_quat(gun.angles.to_array());
        let bone = |t: &str| {
            let (p, q) = worlds[skel.bone_index(t).unwrap()];
            (gun.origin + axis * p, axis * q)
        };
        let n = gun.number;
        let (_, origin) = bone("tag_origin");
        assert!((origin * Vec3::Z).angle_between(Vec3::Z) < 1e-4, "gun {n}");
        let (_, pivot) = bone("tag_pivot");
        let tilt = (pivot * Vec3::Z).angle_between(Vec3::Z).to_degrees();
        assert!(tilt < 1.1, "gun {n}: the bipod tilts {tilt} degrees");
        let (pitch, yaw) = pitch_yaw(bone("tag_aim_animated").1 * Vec3::X);
        assert!((pitch - barrel[0]).abs() < 0.01, "gun {n}: pitch {pitch}");
        let yaw_off = angle_subtract(yaw, gun.angles.y + barrel[1]);
        assert!(yaw_off.abs() < 0.01, "gun {n}: yaw {yaw}");
        let (aim, _) = bone("tag_aim");
        let (butt, _) = bone("tag_butt");
        let t = clip.trace(aim, butt, Vec3::ZERO, Vec3::ZERO, 0x811);
        assert!(t.fraction < 1.0, "gun {n}: the butt hangs in the air");
    }

    /// An unmanned gun's `angles2` pitch is its `dropPitch`: the fixture's
    /// three MG42s carry what `turret_think_init` finds from their map
    /// spots, and the scene draws each that far tipped back on a level
    /// bipod, its butt on the world.
    #[test]
    fn retail_toujane_guns_rest_on_their_butts() {
        let Some(fs) = vcod_common::testing::cod2_fs() else {
            return;
        };
        vcod_cod2::model::register();
        let view = replay();
        let f = mid(&view);
        let placed = place(&f);
        let data = fs.read("maps/mp/mp_toujane.d3dbsp").unwrap();
        let bsp = vcod_cod2::d3dbsp::D3dBsp::parse(&data).unwrap();
        let clip = vcod_cod2::map::clip_map(&bsp).unwrap();
        let (aim, butt) = aim_and_butt(&fs, "weapon_mg42");
        // One gun per spot, entered once per game type.
        let mut spots: Vec<(Vec3, Vec3)> = vcod_common::bsp::entity_blocks(&bsp.entities)
            .into_iter()
            .filter(|e| e.get("classname").map(String::as_str) == Some("misc_mg42"))
            .map(|e| {
                let v = |k: &str| Vec3::from_array(vcod_common::bsp::parse_vec3(&e[k]).unwrap());
                (v("origin"), v("angles"))
            })
            .collect();
        spots.sort_by(|a, b| a.0.x.total_cmp(&b.0.x));
        spots.dedup_by(|a, b| a.0 == b.0);
        assert_eq!(spots.len(), 3);
        let mut scene = Cod2Scene::default();
        for gun in placed.iter().filter(|p| p.turret.is_some()) {
            let &(origin, angles) = spots
                .iter()
                .find(|(o, _)| o.distance(gun.origin) < 0.5)
                .expect("the gun stands on a map spot");
            let wire = gun.turret.as_ref().unwrap().barrel[0];
            assert_eq!(
                drop_pitch(&clip, origin, angles, aim, butt),
                Some(wire),
                "gun {}",
                gun.number
            );
            check_resting_gun(&mut scene, &fs, &clip, gun, f.time_ms);
        }
    }

    /// mp_carentan's MG42s (the house windows of a screenshot)
    /// tip back to the pitch `turret_think_init` finds, on a level bipod,
    /// the butt on the floor or the sill.
    #[test]
    fn carentan_mg42s_rest_on_their_butts() {
        let Some(fs) = vcod_common::testing::cod2_fs() else {
            return;
        };
        vcod_cod2::model::register();
        let data = fs.read("maps/mp/mp_carentan.d3dbsp").unwrap();
        let bsp = vcod_cod2::d3dbsp::D3dBsp::parse(&data).unwrap();
        let clip = vcod_cod2::map::clip_map(&bsp).unwrap();
        let (aim, butt) = aim_and_butt(&fs, "weapon_mg42");
        let mut scene = Cod2Scene::default();
        let mut pitches = Vec::new();
        for (i, e) in vcod_common::bsp::entity_blocks(&bsp.entities)
            .iter()
            .filter(|e| {
                e.get("classname").map(String::as_str) == Some("misc_turret")
                    && e.get("weaponinfo").map(String::as_str) == Some("mg42_bipod_stand_mp")
            })
            .enumerate()
        {
            let v = |k: &str| Vec3::from_array(vcod_common::bsp::parse_vec3(&e[k]).unwrap());
            let (origin, angles) = (v("origin"), v("angles"));
            let pitch = drop_pitch(&clip, origin, angles, aim, butt).expect("the butt lands");
            pitches.push(pitch);
            let gun = Placed2 {
                number: i as u32,
                visual: Visual2::Turret("weapon_mg42".into()),
                origin,
                angles,
                turret: Some(TurretAim {
                    weapon: Some("mg42_bipod_stand_mp".into()),
                    barrel: [pitch, 0.0, 0.0],
                    anim: GunAnim::Idle,
                }),
            };
            check_resting_gun(&mut scene, &fs, &clip, &gun, 0);
        }
        pitches.sort_by(f32::total_cmp);
        pitches.dedup();
        assert_eq!(pitches, [-69.0, -63.0]);
    }
}
