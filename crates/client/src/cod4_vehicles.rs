//! The hardpoint vehicles as the cgame draws them, and the tags of any
//! drawn model an effect is bolted to (cod4-client-view.md 11).
//!
//! A helicopter (`ET_HELICOPTER`) is posed from its rotor clip and its
//! body and gun controllers, puts its damage stage's smoke on its engines
//! and its rotor wash on the ground under it (`CG_VehProcessEntity`).
//! A jet (`ET_PLANE`) and every other model draw in their bind pose; an
//! `EV_PLAY_FX_ON_TAG` on any of them rides the named tag.

use glam::{Mat4, Quat, Vec3};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use vcod_common::model::Model;
use vcod_common::skeleton::{AnimBinding, PoseBuffer, Skeleton};
use vcod_common::turretpose::angles_quat;
use vcod_common::xanim::XAnim;

use crate::fx::sim::{SpawnAt, TagBolt, TagPose};
use crate::play::cod4::VehicleView;

/// A model tag's fx bolt slot is this plus the bone's index in its model
/// (`cod4_fx::slot` keeps the weapons' below it).
pub const TAG_SLOT_BASE: i32 = 16;

/// The rotor clip's goal weight, blend time and rate, set once on the
/// helicopter's anim tree (`CG_PreProcess_GetDObj`, iw3mp.exe 0x435288:
/// `eType` 12, then 1.0, 0.2 at 0x70b198 and 1.5 at 0x70b4e0).
const ROTOR_BLEND_S: f32 = 0.2;
const ROTOR_RATE: f32 = 1.5;

/// The damage stages' smoke (the vehicle effects step, iw3mp.exe 0x44f72d): stage
/// 0 fire and 1 black smoke on `tag_engine_left`, 2 white smoke on
/// `tag_engine_right`, 3 none; the names `CG_RegisterGraphics` registers
/// at 0x43f0ae..0x43f0f8.
fn stage_smoke(stage: u32) -> Option<(&'static str, &'static str)> {
    Some(match stage {
        0 => ("fire/fire_smoke_trail_L", "tag_engine_left"),
        1 => ("smoke/smoke_trail_black_heli", "tag_engine_left"),
        2 => ("smoke/smoke_trail_white_heli", "tag_engine_right"),
        _ => return None,
    })
}
/// A smoke puff every 50 ms (`add eax,0x32` at 0x44f7ba).
const SMOKE_MS: f64 = 50.0;
/// The rotor wash: dust, or spray over water (surface type 20,
/// `0x1400000` at 0x44f6d8), at 0x6cc024 and 0x6cc040.
const DUST: &str = "treadfx/heli_dust_default";
const WATER: &str = "treadfx/heli_water";
const WATER_SURFACE: u32 = 20;
/// The wash's trace: 1200 down (0x70bb50), a 0.5 box, mask 0x831.
const WASH_REACH: f32 = 1200.0;
const WASH_MASK: u32 = 0x831;

/// A model's bones and, for a helicopter, its rotor clip bound to them.
struct Rig {
    skeleton: Skeleton,
    rotors: Option<AnimBinding>,
    /// The bind pose's bone worlds, which any unposed model's tags read.
    bind: Vec<(Vec3, Quat)>,
    /// A gun's clips bound to its bones, by clip name.
    clips: HashMap<String, AnimBinding>,
}

/// The anim a mounted gun plays (`CG_mg42_PreControllers`): its goal,
/// 0 for the weapon's idle (anim 1) or 1 for its fire (anim 2), since
/// when, and the one it blends from over [`GUN_BLEND_MS`] with that
/// one's start.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct GunClock {
    pub goal: usize,
    pub since_ms: i32,
    pub from: Option<(usize, i32)>,
}

/// `XAnimSetGoalWeightKnobAll`'s blend time for a gun's anim, 0.1 s
/// (the float at 0x6b72dc, pushed at iw3mp.exe 0x432dc8).
const GUN_BLEND_MS: i32 = 100;

/// What a gun's pose takes from its weapon: its idle and fire clips, the
/// one it should play and the render time.
pub(crate) struct GunAnims<'a> {
    pub clips: &'a [Arc<XAnim>; 2],
    pub goal: usize,
    pub render_ms: i32,
}

/// One drawn model this frame: where it stands and its bones' poses in
/// its own space.
struct Placed {
    model: String,
    transform: Mat4,
    worlds: Vec<(Vec3, Quat)>,
}

/// A helicopter's effect clocks (`vehicleEffects`' `nextDustFx` and
/// `nextSmokeFx`), in ms on the fx clock.
#[derive(Default)]
struct HeliClock {
    dust: f64,
    smoke: f64,
}

/// One of the cgame's own effects: the name, where, and the tag it
/// rides.
pub(crate) type VehicleFx = (&'static str, SpawnAt, Option<TagBolt>);

#[derive(Default)]
pub(crate) struct Cod4Vehicles {
    rotors: Option<Arc<XAnim>>,
    rigs: HashMap<String, Rig>,
    /// When each helicopter was first drawn (render ms): its rotor clip's
    /// start.
    born: HashMap<u32, i32>,
    placed: HashMap<u32, Placed>,
    /// Helicopters drawn this frame: number, origin, damage stage.
    helis: Vec<(u32, Vec3, u32)>,
    clocks: HashMap<u32, HeliClock>,
    /// Each drawn gun's anim.
    pub(crate) guns: HashMap<u32, GunClock>,
    /// Tags an effect rides, posed while their entity draws.
    bolts: HashSet<TagBolt>,
    /// Helicopter frames posed and effects the helicopters started, for
    /// F3 and the gates.
    pub(crate) posed: usize,
    pub(crate) smoke: usize,
    pub(crate) wash: usize,
}

impl Cod4Vehicles {
    /// `rotors` is the helicopter clip (`anim::ENTITY_CLIPS`).
    pub(crate) fn new(rotors: Option<XAnim>) -> Self {
        Cod4Vehicles {
            rotors: rotors.map(Arc::new),
            ..Default::default()
        }
    }

    /// A new frame: last frame's placements go.
    pub(crate) fn begin(&mut self) {
        self.placed.clear();
        self.helis.clear();
    }

    /// Places entity `number`'s `model` at `transform` (origin and
    /// angles) and returns its skin matrices when it is posed: a
    /// helicopter's rotors turn from its first frame on `render_ms` and
    /// its controllers take `vehicle`; anything else draws its bind pose
    /// (`None`).
    pub(crate) fn place(
        &mut self,
        number: u32,
        model: &Model,
        transform: Mat4,
        vehicle: Option<&VehicleView>,
        render_ms: i32,
    ) -> Option<Vec<Mat4>> {
        let rotors = self.rotors.clone();
        let rig = self
            .rigs
            .entry(model.name.clone())
            .or_insert_with(|| rig(model, rotors.as_deref()));
        let Some(v) = vehicle else {
            self.placed.insert(
                number,
                Placed {
                    model: model.name.clone(),
                    transform,
                    worlds: rig.bind.clone(),
                },
            );
            return None;
        };
        let born = *self.born.entry(number).or_insert(render_ms);
        let mut pose = PoseBuffer::new(&rig.skeleton);
        if let (Some(c), Some(b)) = (&self.rotors, &rig.rotors) {
            // The tree's weight eases in over the blend; the clip runs at
            // its rate from the entity's first frame.
            let t = (render_ms - born).max(0) as f32 / 1000.0;
            let w = (t / ROTOR_BLEND_S).min(1.0);
            let at = c.frame_pos(t * ROTOR_RATE, true);
            if w >= 1.0 {
                pose.apply(c, b, at);
            } else {
                pose.apply_weighted(c, b, at, w);
            }
        }
        // `CG_VehPoseControllers`: the body pitches and rolls, the turret
        // yaws and the barrel pitches, each as a local tag.
        let turn = |pose: &mut PoseBuffer, name: &str, angles: [f32; 3]| {
            if let Some(b) = rig.skeleton.bone_index(name) {
                let (p, q) = pose.local(b);
                pose.set_local(b, p, q * angles_quat(angles));
            }
        };
        turn(&mut pose, "tag_body", [v.body_pitch, 0.0, v.body_roll]);
        turn(&mut pose, "tag_turret", [0.0, v.gun_yaw, 0.0]);
        turn(&mut pose, "tag_barrel", [v.gun_pitch, 0.0, 0.0]);
        let skins = pose.skin_matrices(&rig.skeleton, 0);
        self.posed += 1;
        self.placed.insert(
            number,
            Placed {
                model: model.name.clone(),
                transform,
                worlds: pose.bone_worlds(&rig.skeleton),
            },
        );
        self.helis
            .push((number, transform.w_axis.truncate(), v.stage));
        Some(skins)
    }

    /// A mounted gun posed by its weapon's anim (`anims`, retail's
    /// `CG_mg42_PreControllers`: the goal set at full weight over 0.1 s)
    /// and then `turret_controller`:
    /// `tag_aim` and `tag_aim_animated` turned by the barrel's pitch and
    /// yaw, `tag_flash` by the third angle, each as a local tag; its tags
    /// kept for the effects and the gunner's eye.
    pub(crate) fn place_turret(
        &mut self,
        number: u32,
        model: &Model,
        transform: Mat4,
        gun: [f32; 3],
        anims: Option<GunAnims>,
    ) -> Option<Vec<Mat4>> {
        let rotors = self.rotors.clone();
        let rig = self
            .rigs
            .entry(model.name.clone())
            .or_insert_with(|| rig(model, rotors.as_deref()));
        let mut pose = PoseBuffer::new(&rig.skeleton);
        if let Some(a) = anims {
            let now = a.render_ms;
            let c = self.guns.entry(number).or_insert(GunClock {
                goal: a.goal,
                since_ms: now,
                from: None,
            });
            if c.goal != a.goal {
                *c = GunClock {
                    goal: a.goal,
                    since_ms: now,
                    from: Some((c.goal, c.since_ms)),
                };
            }
            let c = *c;
            let mut play = |pose: &mut PoseBuffer, n: usize, start: i32, w: f32| {
                let clip = &a.clips[n];
                let b = rig
                    .clips
                    .entry(clip.name.clone())
                    .or_insert_with(|| rig.skeleton.bind(clip));
                let at = clip.frame_pos((now - start).max(0) as f32 / 1000.0, clip.looping);
                pose.apply_weighted(clip, b, at, w);
            };
            let w = ((now - c.since_ms) as f32 / GUN_BLEND_MS as f32).clamp(0.0, 1.0);
            if let Some((n, start)) = c.from.filter(|_| w < 1.0) {
                play(&mut pose, n, start, 1.0);
            }
            play(
                &mut pose,
                c.goal,
                c.since_ms,
                if c.from.is_some() { w } else { 1.0 },
            );
        }
        let turn = |pose: &mut PoseBuffer, name: &str, angles: [f32; 3]| {
            if let Some(b) = rig.skeleton.bone_index(name) {
                let (p, q) = pose.local(b);
                pose.set_local(b, p, q * angles_quat(angles));
            }
        };
        turn(&mut pose, "tag_aim", [gun[0], gun[1], 0.0]);
        turn(&mut pose, "tag_aim_animated", [gun[0], gun[1], 0.0]);
        turn(&mut pose, "tag_flash", [gun[2], 0.0, 0.0]);
        let skins = pose.skin_matrices(&rig.skeleton, 0);
        self.posed += 1;
        self.placed.insert(
            number,
            Placed {
                model: model.name.clone(),
                transform,
                worlds: pose.bone_worlds(&rig.skeleton),
            },
        );
        Some(skins)
    }

    /// The frame's placements done: a helicopter, a clock or a bolt whose
    /// entity no longer draws is forgotten.
    pub(crate) fn end(&mut self) {
        let placed = &self.placed;
        self.born.retain(|n, _| placed.contains_key(n));
        self.clocks.retain(|n, _| placed.contains_key(n));
        self.guns.retain(|n, _| placed.contains_key(n));
        self.bolts.retain(|b| placed.contains_key(&b.entity));
    }

    /// Entity `number`'s `tag` this frame, by name (case-blind, as the
    /// engine's script strings are), as an fx bolt and its world pose.
    pub(crate) fn tag(&self, number: u32, tag: &str) -> Option<(TagBolt, TagPose)> {
        let p = self.placed.get(&number)?;
        let rig = self.rigs.get(&p.model)?;
        let bone = rig
            .skeleton
            .bones()
            .iter()
            .position(|b| b.name.eq_ignore_ascii_case(tag))?;
        let bolt = TagBolt {
            entity: number,
            slot: TAG_SLOT_BASE + bone as i32,
        };
        Some((bolt, pose_of(p, bone)))
    }

    /// [`Self::tag`], kept posing for the effects on it while its entity
    /// draws (`FX_PlayBoltedEffect`).
    pub(crate) fn bolt(&mut self, number: u32, tag: &str) -> Option<(TagBolt, TagPose)> {
        let (b, pose) = self.tag(number, tag)?;
        self.bolts.insert(b);
        Some((b, pose))
    }

    /// The bolted tags' poses this frame.
    pub(crate) fn tags(&self) -> HashMap<TagBolt, TagPose> {
        self.bolts
            .iter()
            .filter_map(|b| {
                let p = self.placed.get(&b.entity)?;
                let bone = usize::try_from(b.slot - TAG_SLOT_BASE).ok()?;
                (bone < p.worlds.len()).then(|| (*b, pose_of(p, bone)))
            })
            .collect()
    }

    /// The vehicle effects step's helicopter arm (iw3mp.exe 0x44f568) at `now_ms`
    /// on the fx clock: the wash where a trace 1200 down from the body
    /// meets the clip map, again 50 to 150 ms later as it climbs from 350
    /// to 1200 above it (1 s when it meets nothing), and the stage's
    /// smoke on its engine tag every 50 ms.
    pub(crate) fn heli_fx(
        &mut self,
        now_ms: f64,
        clip: Option<&vcod_cod4::collision::ClipMap>,
    ) -> Vec<VehicleFx> {
        let mut out = Vec::new();
        for (n, origin, stage) in self.helis.clone() {
            let clock = self.clocks.entry(n).or_default();
            if clock.dust <= now_ms {
                let mut next = 1000.0;
                if let Some(clip) = clip {
                    let end = origin - Vec3::Z * WASH_REACH;
                    let half = Vec3::splat(0.5);
                    let tr = clip.trace(origin, end, -half, half, WASH_MASK);
                    if tr.fraction < 1.0 {
                        let ground = origin.lerp(end, tr.fraction);
                        let dist = ground.distance(origin);
                        next = (((dist - 350.0) / 850.0 * 0.1 + 0.05) * 1000.0).trunc();
                        let surf = (tr.surface_flags & 0x1f0_0000) >> 20;
                        let name = if surf == WATER_SURFACE { WATER } else { DUST };
                        out.push((
                            name,
                            SpawnAt::Directed {
                                pos: ground,
                                dir: Vec3::Z,
                            },
                            None,
                        ));
                        self.wash += 1;
                    }
                }
                self.clocks.entry(n).or_default().dust = now_ms + f64::from(next);
            }
            let clock = self.clocks.entry(n).or_default();
            let Some((name, tag)) = stage_smoke(stage) else {
                continue;
            };
            if clock.smoke > now_ms {
                continue;
            }
            clock.smoke = now_ms + SMOKE_MS;
            if let Some((b, pose)) = self.bolt(n, tag) {
                out.push((
                    name,
                    SpawnAt::Oriented {
                        pos: pose.origin,
                        axis: pose.axis,
                    },
                    Some(b),
                ));
                self.smoke += 1;
            }
        }
        out
    }

    /// The models drawn this frame, by entity.
    #[cfg(test)]
    pub(crate) fn models(&self) -> impl Iterator<Item = (u32, &str)> + '_ {
        self.placed.iter().map(|(&n, p)| (n, p.model.as_str()))
    }

    /// The entities drawn this frame and where, for the sounds riding
    /// them.
    pub(crate) fn origins(&self) -> impl Iterator<Item = (u32, Vec3)> + '_ {
        self.placed
            .iter()
            .map(|(&n, p)| (n, p.transform.w_axis.truncate()))
    }
}

fn rig(model: &Model, rotors: Option<&XAnim>) -> Rig {
    let skeleton = Skeleton::build(&[model]);
    let bind = PoseBuffer::new(&skeleton).bone_worlds(&skeleton);
    Rig {
        rotors: rotors.map(|c| skeleton.bind(c)),
        skeleton,
        bind,
        clips: HashMap::new(),
    }
}

fn pose_of(p: &Placed, bone: usize) -> TagPose {
    let (pos, q) = p.worlds[bone];
    let (_, turn, _) = p.transform.to_scale_rotation_translation();
    let q = turn * q;
    TagPose {
        origin: p.transform.transform_point3(pos),
        axis: [q * Vec3::X, q * Vec3::Y, q * Vec3::Z],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::play::cod4::tests::{clk_now, clk_sleep, run_held, since as clk_since};
    use crate::play::cod4::{Cod4Session, ET_HELICOPTER, ET_PLANE};
    use std::time::Duration;

    /// What the live test asks of the server's loop.
    enum Call {
        /// `triggerHardpoint("helicopter_mp")` for our player.
        Heli,
        /// The airstrike on a point.
        Airstrike([f32; 3]),
    }

    /// What the live client saw.
    #[derive(Debug, Default)]
    struct Seen {
        heli_model: Option<String>,
        plane_models: HashSet<String>,
        rotor_turned: bool,
        heli_pixels: usize,
        /// The helicopter's damage stages as drawn, in order.
        stages: Vec<u32>,
        heli_gone: bool,
        smoke: usize,
        wash: usize,
        effects: HashSet<String>,
        /// The entities' loop sounds the client asked the mixer for, and
        /// the aliases the mixer held.
        loops: HashSet<String>,
        sounds: HashSet<String>,
        /// The looping voices but the map's ambient the mixer still played
        /// 25 s after the strike, the helicopter and the jets long gone.
        stuck: Vec<String>,
        errors: Vec<String>,
        /// The most trail quads one frame drew while our launcher's grenade
        /// flew.
        gl_trail_quads: usize,
        /// The crash debris's emissions (`fire/fire_smoke_trail_m`).
        emits: usize,
    }

    /// The angle about the body's up between `main_rotor_jnt` and
    /// `tag_body`, degrees.
    fn rotor_angle(v: &Cod4Vehicles, n: u32) -> Option<f32> {
        let (_, rotor) = v.tag(n, "main_rotor_jnt")?;
        let (_, body) = v.tag(n, "tag_body")?;
        let a = rotor.axis[0].dot(body.axis[0]).clamp(-1.0, 1.0).acos();
        Some(a.to_degrees())
    }

    /// Retail 1.7's helicopter and planes (`play-1.7-hardpoint.txt`, a
    /// probe gametype flying two planes and a helicopter, cod4-gsc.md
    /// 3.2) replayed through the session: the helicopter draws the model
    /// its `un2` names and the planes theirs, and posed through
    /// [`Cod4Vehicles`] with the zones' rotor clip the helicopter's rotors
    /// turn and its engine tags pose. Needs `COD4_DIR`.
    #[test]
    fn retail_hardpoint_capture_draws_and_poses_the_vehicles() {
        use crate::play::cod4::Visual;
        let Some(install) = vcod_cod4::testing::cod4_install() else {
            return;
        };
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../cod4/tests/fixtures/net/play-1.7-hardpoint.txt"
        );
        let mut s = Cod4Session::open_capture(&std::fs::read_to_string(path).unwrap()).unwrap();
        let map = s.map_name().unwrap();
        let mut loaded = None;
        let mut v = Cod4Vehicles::default();
        let (mut helis, mut planes) = (HashSet::new(), HashSet::new());
        let (mut rotor, mut turned, mut engines) = (None::<f32>, false, false);
        let mut flash = None;
        let mut ms = 0.0;
        while !s.finished() {
            ms += 1000.0 / 60.0;
            s.step(ms).unwrap();
            s.take_fired();
            let Some(scene) = s.scene() else { continue };
            let models = loaded.get_or_insert_with(|| {
                let m =
                    crate::cod4_fly::load(install.root(), None, &map, &s.entity_assets()).unwrap();
                let clip = m.bodies.as_ref().and_then(|b| {
                    use vcod_common::asset_source::AssetSource;
                    b.decoded_anim(vcod_cod4::anim::ENTITY_CLIPS[0])?.ok()
                });
                v = Cod4Vehicles::new(clip);
                m.models
            });
            v.begin();
            for d in &scene.entities {
                let Visual::Model(name) = &d.visual else {
                    continue;
                };
                let pose =
                    Mat4::from_rotation_translation(angles_quat(d.angles.to_array()), d.origin);
                match d.etype {
                    ET_PLANE => {
                        planes.insert(name.clone());
                    }
                    ET_HELICOPTER => {
                        helis.insert(name.clone());
                        let m = models.get(name).expect("the zones hold the helicopter");
                        let skins =
                            v.place(d.number, m, pose, d.vehicle.as_ref(), scene.render_time);
                        assert_eq!(skins.map(|k| k.len()), Some(m.bones.len()));
                        engines |= v.tag(d.number, "tag_engine_left").is_some();
                        if let Some((_, f)) = v.tag(d.number, "tag_flash") {
                            let (fwd, up) = (pose.x_axis.truncate(), pose.z_axis.truncate());
                            let off = f.origin - d.origin;
                            flash = Some((off.dot(fwd), off.dot(up), f.axis[0].dot(fwd)));
                        }
                        let a = rotor_angle(&v, d.number);
                        match rotor {
                            None => rotor = a,
                            Some(r0) => turned |= a.is_some_and(|a| (a - r0).abs() > 5.0),
                        }
                    }
                    _ => {}
                }
            }
            v.end();
        }
        assert_eq!(
            helis,
            HashSet::from(["vehicle_cobra_helicopter_fly".to_string()])
        );
        assert!(!planes.is_empty(), "no plane drew");
        assert!(turned, "the rotors stood still");
        assert!(engines);
        // The gun's flash (`CG_FireWeapon`'s helicopter arm): ahead of the
        // body and under it, pointing forward, not up at the rotor.
        let (ahead, up, along) = flash.expect("the helicopter's tag_flash");
        println!("tag_flash {ahead} ahead, {up} up, forward {along}");
        assert!(ahead > 50.0 && up < 0.0 && along > 0.7, "{flash:?}");
    }

    /// vcod-server's war on mp_crash with us joined on allies, played
    /// through `Cod4Client::pump` as `main` runs a frame (the session's
    /// map, models, clips, sounds and effects off `cod4_fly::load`, a
    /// headless renderer, a silent mixer, the fx system). The stock
    /// helicopter we call, cvar'd to fall apart on its own, draws its
    /// model posed with turning rotors (an offscreen frame aimed at it
    /// differs from one without the models; `VCOD_C4_SHOTS=<dir>` writes
    /// both), asks for its loop sound, washes the ground under it, smokes
    /// white, black and on fire through stages 2, 1 and 0, and its crash
    /// plays the script's explosions, trails and sounds. Then the
    /// airstrike's jets draw their model, their loops, the sonic boom, the
    /// bombs' `playFxOnTag` and the impacts play, and 25 s on the mixer,
    /// its loops kept as a device keeps them, plays no loop but the map's
    /// ambient (the dying loop's secondary once rang forever). Needs `COD4_DIR` and an
    /// adapter with BC textures (lavapipe will do).
    #[test]
    fn cod4_live_hardpoints_draw_smoke_and_sound() {
        use vcod_server::cod4::{Cod4Config, Cod4Server};
        let Some(install) = vcod_cod4::testing::cod4_install() else {
            return;
        };
        let root = install.root().to_path_buf();
        let _slot = crate::renderer::pipeline_check::live_session_slot();
        let mut sv = Cod4Server::new(
            Cod4Config {
                map: "mp_crash".into(),
                gametype: "war".into(),
                hostname: "vcod".into(),
                max_clients: 4,
            },
            7,
        );
        sv.set_cvar("scr_game_playerwaittime", "1");
        sv.set_cvar("scr_game_matchstarttime", "1");
        // The stock helicopter falls apart on its own: no armour, light
        // smoke at once, heavy at 8 of 10, 5 more each second in heavy
        // smoke (2.5 in light) until past 10 it crashes (`heli_health`).
        for (k, v) in [
            ("scr_heli_maxhealth", "10"),
            ("scr_heli_armor", "0"),
            ("scr_heli_health_degrade", "5"),
        ] {
            sv.set_cvar(k, v);
        }
        sv.load(Some(&install)).unwrap();
        let (tx, rx) = std::sync::mpsc::channel::<Call>();

        let client = run_held(
            &mut sv,
            move |addr| play(&root, addr, tx),
            |sv, _, _| {
                if let Ok(call) = rx.try_recv() {
                    let l = sv.game.level.as_mut().unwrap();
                    match call {
                        Call::Heli => l
                            .start_player_script(
                                0,
                                "maps/mp/gametypes/_hardpoints",
                                "triggerHardpoint",
                                &["helicopter_mp"],
                            )
                            .unwrap(),
                        Call::Airstrike(at) => l.start_airstrike(0, at).unwrap(),
                    }
                }
                true
            },
        );
        let Some(seen) = client else {
            return;
        };
        println!("{seen:#?}");
        assert!(seen.errors.is_empty(), "{:?}", seen.errors);
        assert_eq!(
            seen.heli_model.as_deref(),
            Some("vehicle_cobra_helicopter_fly")
        );
        assert!(seen.rotor_turned, "the rotors stood still");
        assert!(seen.heli_pixels > 400, "{} pixels", seen.heli_pixels);
        // Whether the mixer takes the loop rides its distance (the crash
        // can come before it nears us).
        assert!(
            seen.loops.contains("mp_cobra_helicopter"),
            "{:?}",
            seen.loops
        );
        // `heli_health` sets 3 and, with no armour, 2 in its first pass.
        assert!(seen.stages.ends_with(&[2, 1, 0]), "{:?}", seen.stages);
        assert!(seen.heli_gone, "it never crashed");
        // The trails (cod4-effects.md 16) and the crash debris's emissions
        // (15).
        assert!(
            seen.gl_trail_quads > 0,
            "the launcher's grenade drew no trail"
        );
        assert!(seen.emits > 0, "the crash debris emitted nothing");
        assert!(seen.wash > 0);
        assert!(seen.smoke > 0);
        for fx in [
            // the cgame's stages
            "smoke/smoke_trail_white_heli",
            "smoke/smoke_trail_black_heli",
            "fire/fire_smoke_trail_L",
            // `heli_crash`'s and `heli_explode`'s
            "explosions/aerial_explosion_large",
            "explosions/aerial_explosion",
            "explosions/helicopter_explosion_cobra",
            // the airstrike's bomb
            "explosions/clusterbomb",
        ] {
            assert!(seen.effects.contains(fx), "{fx}: {:?}", seen.effects);
        }
        for alias in [
            "cobra_helicopter_hit",
            "cobra_helicopter_secondary_exp",
            "cobra_helicopter_dying_loop",
            "cobra_helicopter_crash",
            "veh_mig29_dist_loop",
            "veh_mig29_close_loop",
            "veh_mig29_sonic_boom",
            "artillery_impact",
        ] {
            assert!(seen.sounds.contains(alias), "{alias}: {:?}", seen.sounds);
        }
        assert!(seen.effects.iter().any(|e| e.starts_with("treadfx/")));
        assert!(seen.stuck.is_empty(), "loops left on: {:?}", seen.stuck);
        assert!(
            seen.plane_models.contains("vehicle_mig29_desert"),
            "{:?}",
            seen.plane_models
        );
    }

    /// The client's half of [`cod4_live_hardpoints_draw_smoke_and_sound`];
    /// `None` without an adapter.
    fn play(
        root: &std::path::Path,
        addr: std::net::SocketAddr,
        tx: std::sync::mpsc::Sender<Call>,
    ) -> Option<Seen> {
        use crate::audio::{AudioOpts, AudioSystem, bank};
        use crate::cod2_client::Cod2Frame;
        use crate::renderer::{CullMode, Frame, Renderer};
        let join = crate::play::cod4_menus::Join {
            team: Some("allies".into()),
            class: Some("assault_mp,0".into()),
        };
        let mut s =
            Cod4Session::connect(addr, "joiner", vcod_cod4::net::conn::Cp::Bare, join).unwrap();
        let t0 = clk_now();
        while s.spawn().is_none() && clk_since(t0) < Duration::from_secs(60) {
            s.step(clk_since(t0).as_secs_f64() * 1000.0).unwrap();
            s.take_commands();
            clk_sleep(Duration::from_millis(16));
        }
        assert!(s.spawn().is_some(), "never spawned");
        let map = crate::cod4_fly::load(root, None, "mp_crash", &s.entity_assets()).unwrap();
        let mut fs = crate::cod4_fly::open_fs(root, None, None).unwrap();
        for (p, w) in &map.sound_files {
            fs.overlay(p, w.clone());
        }
        let Some((mut r, errors)) = Renderer::headless_sized(&fs, false, 1, (640, 384)) else {
            s.disconnect();
            return None;
        };
        r.set_gamma(1.0, crate::gamma::overbright_bits(fs.game(), false, true));
        r.set_material_lib(map.materials.clone());
        r.load_world(&map.bsp, &fs).unwrap();
        let shell = crate::console::shell::Shell::new();
        let localized = vcod_common::localize::Localized::load(&fs);
        let ui = crate::frontend::Ui::without_menus(&fs);
        let mut audio = AudioSystem::with_aliases(
            &fs,
            AudioOpts {
                enabled: false,
                volume: shell.cvar_f32("mss_volume"),
                rate: bank::mixer_rate(shell.cvar_f32("mss_khz") as i32),
            },
            Default::default(),
        );
        audio.on_gamestate("mp_crash");
        // Its loops ring on until stopped, as a device's would.
        audio.keep_silent_loops();
        let mut fx = crate::fx::sim::FxSystem::new();
        let mut c = crate::cod4_client::Cod4Client::new(
            s,
            map.models,
            map.parts,
            map.weapons,
            map.bodies,
            map.av,
            map.world,
            map.look,
        );
        let mut cam = crate::camera::FlyCamera::new(Vec3::ZERO, 0.0);
        let mut seen = Seen::default();
        let start = clk_now();
        let mut last = start;
        // When the helicopter first drew, its rotor angle then, and when
        // the airstrike went to the server.
        let (mut heli_at, mut rotor0, mut struck) = (None, None, None);
        // The launcher first: its grenade's `projTrailEffect` on its
        // `tag_fx`; then the helicopter.
        let (mut gl_weapon, mut gl_fired, mut heli_called) = (None::<u32>, None::<f32>, false);
        let trail_quads = |fx: &crate::fx::sim::FxSystem, time: f32| {
            fx.build_quads(Vec3::ZERO, Vec3::Y, Vec3::Z, time)
                .iter()
                .filter(|q| q.shader.contains("smoke_trail"))
                .count()
        };
        let emitted = crate::cod4_fx::key("fire/fire_smoke_trail_m");
        while clk_since(start) < Duration::from_secs(110) {
            // the frame first: a pass that skips on must still move the clock
            clk_sleep(Duration::from_millis(16));
            let now = clk_now();
            let dt = now.duration_since(last).as_secs_f32();
            last = now;
            let time = clk_since(start).as_secs_f32();
            let mut cx = Cod2Frame {
                fs: &fs,
                shell: &shell,
                localized: &localized,
                audio: &mut audio,
                fx: &mut fx,
                ui: &ui,
                r: &mut r,
                console_open: false,
                time,
                dt,
            };
            c.pump(&mut cx, &mut cam);
            // `ViewTail`'s listener on the camera.
            let (f, rt, u) = crate::camera::basis(cam.yaw, cam.pitch);
            audio.set_listener(cam.pos, f, rt, u, u32::MAX);
            fx.step(dt, time, None);
            seen.emits += fx
                .pending_chains()
                .iter()
                .filter(|p| **p == emitted)
                .count();
            // `main`'s frame starts them after the step.
            fx.spawn_chained(&fs, time);
            if !heli_called {
                let ps = c.session.client().snapshots.last().map(|s| s.ps.clone());
                let u = |n: &str| ps.as_ref().and_then(|p| p.get(n)).unwrap_or(0);
                match (gl_weapon, gl_fired) {
                    (None, _) => {
                        if let Some(k) = (0..4).find(|k| u(&format!("actionSlotType[{k}]")) == 2)
                            && c.session.action_slot(k + 1)
                        {
                            gl_weapon = Some(u("weapon"));
                        }
                    }
                    (Some(w), None) => {
                        if u("weapon") != w && u("weaponstate") == 0 {
                            c.session
                                .input
                                .key(crate::play::input::Action::Attack, true);
                            gl_fired = Some(time);
                        }
                    }
                    (Some(_), Some(t)) => {
                        if time > t + 0.3 {
                            c.session
                                .input
                                .key(crate::play::input::Action::Attack, false);
                        }
                        seen.gl_trail_quads = seen.gl_trail_quads.max(trail_quads(&fx, time));
                        if time > t + 4.0 {
                            heli_called = true;
                            tx.send(Call::Heli).ok()?;
                        }
                    }
                }
                if !heli_called && time > 20.0 {
                    heli_called = true;
                    tx.send(Call::Heli).ok()?;
                }
            }
            let positions = c.entity_positions();
            // What the mixer holds before the step drops this silent
            // system's voices.
            seen.sounds
                .extend(audio.mix().into_iter().map(|(row, _)| row.name));
            audio.step(&positions, &HashMap::new(), None);
            let snap = c.session.client().snapshots.last().cloned();
            if let (Some(sn), Some(gs)) = (&snap, c.session.gamestate()) {
                let cs = |n: u16| String::from_utf8_lossy(gs.configstring(n)).into_owned();
                let loops = crate::cod4_audio::loop_sounds(
                    &sn.entities,
                    &cs,
                    &[],
                    &crate::play::cod4::origin_of,
                );
                seen.loops
                    .extend(loops.into_values().filter_map(|(_, a, _)| a));
            }
            let of = |t: u32| {
                snap.iter()
                    .flat_map(|s| s.entities.iter())
                    .filter(move |e| e.etype() == t)
                    .map(|e| e.number())
                    .collect::<Vec<_>>()
            };
            let drawn: HashMap<u32, String> = c
                .vehicles
                .models()
                .map(|(n, m)| (n, m.to_string()))
                .collect();
            for n in of(ET_PLANE) {
                seen.plane_models.extend(drawn.get(&n).cloned());
            }
            if let Some(&n) = of(ET_HELICOPTER).first()
                && let Some(model) = drawn.get(&n)
            {
                seen.heli_model = Some(model.clone());
                let a = rotor_angle(&c.vehicles, n);
                match rotor0 {
                    None => rotor0 = a,
                    Some(r0) => {
                        seen.rotor_turned |= a.is_some_and(|a| (a - r0).abs() > 5.0);
                    }
                }
                let first = *heli_at.get_or_insert(clk_since(start));
                // An offscreen frame aimed at it, with and without the
                // models this frame drew.
                if seen.heli_pixels == 0 && clk_since(start) > first + Duration::from_secs(2) {
                    let h = c.vehicles.origins().find(|o| o.0 == n).unwrap().1;
                    let eye = h + Vec3::new(-260.0, -260.0, -80.0);
                    let d = h - eye;
                    let yaw = d.y.atan2(d.x);
                    let pitch = d.z.atan2(d.truncate().length());
                    let (fwd, _, up) = crate::camera::basis(yaw, pitch);
                    const FOV: f32 = 80.0;
                    let frame = |r: &Renderer| Frame {
                        view_proj: crate::camera::view_proj_from(
                            eye,
                            yaw,
                            pitch,
                            0.0,
                            FOV,
                            r.aspect(),
                        ),
                        eye,
                        fwd,
                        up,
                        time,
                        fov_x: FOV,
                        cull: CullMode::On,
                        hud_lines: Vec::new(),
                    };
                    r.render(frame(&r), None);
                    let (_, _, with) = r.read_offscreen().unwrap();
                    r.set_dynamic_models(&[]);
                    r.render(frame(&r), None);
                    let (_, _, without) = r.read_offscreen().unwrap();
                    if let Some(dir) = std::env::var_os("VCOD_C4_SHOTS") {
                        for (name, px) in [("with", &with), ("without", &without)] {
                            let rgba: Vec<u8> =
                                px.chunks(4).flat_map(|p| [p[2], p[1], p[0], 255]).collect();
                            let path = std::path::Path::new(&dir).join(format!("heli_{name}.tga"));
                            image::save_buffer(
                                path,
                                &rgba,
                                640,
                                384,
                                image::ExtendedColorType::Rgba8,
                            )
                            .unwrap();
                        }
                    }
                    // The middle third, where the helicopter is.
                    seen.heli_pixels = with
                        .chunks(4)
                        .zip(without.chunks(4))
                        .enumerate()
                        .filter(|(i, _)| {
                            let (x, y) = (i % 640, i / 640);
                            (213..427).contains(&x) && (128..256).contains(&y)
                        })
                        .filter(|(_, (p, q))| (0..3).any(|i| p[i].abs_diff(q[i]) > 8))
                        .count();
                }
                let stage = snap
                    .iter()
                    .flat_map(|s| s.entities.iter())
                    .find(|e| e.number() == n)
                    .map(|e| crate::play::cod4::ent_u32(e, "un1.helicopterStage"));
                if let Some(st) = stage
                    && seen.stages.last() != Some(&st)
                {
                    seen.stages.push(st);
                }
            } else if heli_at.is_some() && !seen.heli_gone {
                // It crashed and blew up: the airstrike next, 400 from us.
                seen.heli_gone = true;
                let ps = &snap.as_ref()?.ps;
                let o = |k: usize| ps.get_f32(&format!("origin[{k}]")).unwrap_or(0.0);
                tx.send(Call::Airstrike([o(0) + 400.0, o(1), o(2)])).ok()?;
                struck = Some(clk_since(start));
            }
            if struck.is_some_and(|t| clk_since(start) > t + Duration::from_secs(25)) {
                seen.stuck = audio
                    .mix()
                    .into_iter()
                    // The map's ambient plays on for good.
                    .filter(|(row, _)| row.looping && !row.name.starts_with("ambient_"))
                    .map(|(row, _)| row.name)
                    .collect();
                break;
            }
        }
        if let Some(e) = c.effects_named() {
            seen.effects = e;
        }
        seen.smoke = c.vehicles.smoke;
        seen.wash = c.vehicles.wash;
        seen.errors = errors.lock().unwrap().clone();
        c.session.disconnect();
        Some(seen)
    }

    #[test]
    fn the_stages_smoke_as_the_cgame_picks() {
        assert_eq!(stage_smoke(3), None);
        assert_eq!(stage_smoke(2).unwrap().1, "tag_engine_right");
        assert_eq!(stage_smoke(1).unwrap().0, "smoke/smoke_trail_black_heli");
        assert_eq!(stage_smoke(0).unwrap().0, "fire/fire_smoke_trail_L");
    }
}
