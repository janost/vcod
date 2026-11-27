//! `physicsLaunch`ed parts on the client: `CG_CalcEntityPhysicsPositions`
//! (iw3mp.exe 0x434e50). The server sends a launched part's launch once
//! and never moves it; each client flies it itself in physics world 1
//! (`vcod_cod4::ode::phys`, cod4-gsc.md 3.6). The FX system's physics
//! model elements fly in the same world (`vcod_cod4::fx_phys`,
//! cod4-effects.md 9.1.1).

use std::collections::HashMap;
use std::sync::Arc;

use glam::Vec3;
use vcod_cod4::fx_phys::{self, EffectFrame, PhysEffect};
use vcod_cod4::net::delta::EntityState;
use vcod_cod4::ode::collide::{self, Impact};
use vcod_cod4::ode::obj::PhysModel;
use vcod_cod4::ode::phys::{self, PhysWorld, WorldId};
use vcod_cod4::ode::world::{BodyId, PhysDvars};
use vcod_common::rng::MsvcRand;
use vcod_common::turretpose::quat_angles;

use super::cod4::{ent_f32, ent_u32};
use super::cod4_predict::Cod4World;

/// `TR_PHYSICS`.
pub const TR_PHYSICS: u32 = 8;
/// A body is made only within this long of its launch (0x434ead): a
/// client that sees the launch later draws the part where it stood.
const LATE_MS: i32 = 1000;

/// One launched entity: its launch time and model, and its body in world
/// 1 with the preset's `bulletForceScale`, `None` when it was seen too
/// late, its model has no preset or the body pool was spent.
struct Part {
    launched: i32,
    model: String,
    body: Option<(BodyId, f32)>,
}

/// A physics model element in flight: its body, its visual's model, its
/// def (element `elem` of `effect`), its seed, start and life, ms.
struct FxBody {
    body: BodyId,
    model: String,
    effect: Arc<PhysEffect>,
    elem: usize,
    seed: u16,
    begin: i32,
    life: i32,
}

/// Every launched part a session has seen, by entity number, the FX
/// system's physics model elements, and physics world 1, which steps
/// their bodies.
pub struct Cod4Physics {
    parts: HashMap<u32, Part>,
    fx: Vec<FxBody>,
    pub phys: PhysDvars,
    pub contact: collide::Dvars,
    pub pw: PhysWorld,
    /// Whether world 1's clock runs ([`PhysWorld::set_time`]).
    started: bool,
    /// The impact sounds world 1's steps played since the owner last took
    /// them.
    pub impacts: Vec<Impact>,
}

impl Default for Cod4Physics {
    fn default() -> Self {
        Cod4Physics {
            parts: HashMap::new(),
            fx: Vec::new(),
            phys: PhysDvars::default(),
            contact: collide::Dvars::default(),
            pw: PhysWorld::new(WorldId::Fx),
            started: false,
            impacts: Vec::new(),
        }
    }
}

fn vec3(e: &EntityState, name: &str) -> Vec3 {
    Vec3::from_array(std::array::from_fn(|k| ent_f32(e, &format!("{name}[{k}]"))))
}

/// Whether `e` is a part `CG_CalcEntityPhysicsPositions` flies: its
/// position `TR_PHYSICS`, drawn (`eFlags` 0x20 clear, 0x434e60), and not a
/// brush model (0x434e6d).
pub fn launched(e: &EntityState) -> bool {
    ent_u32(e, "lerp.pos.trType") == TR_PHYSICS
        && ent_u32(e, "lerp.eFlags") & 0x20 == 0
        && ent_u32(e, "solid") != 0xff_ffff
}

impl Cod4Physics {
    fn start(&mut self, now: i32) {
        if !self.started {
            self.pw.set_time(now);
            self.started = true;
        }
    }

    /// World 1 run to `time` (`Phys_RunToTime` at 0x5fa373), the CRT's
    /// `rand` stream lent to it. Run once a frame before the parts are
    /// placed.
    pub fn advance(&mut self, world: &Cod4World, time: i32, rand: &mut MsvcRand) {
        self.start(time);
        std::mem::swap(&mut self.pw.rand, rand);
        let (phys, contact) = (self.phys, self.contact);
        self.pw
            .run_to_time(time, &phys, &contact, &*world.clip, &mut |_| {});
        std::mem::swap(&mut self.pw.rand, rand);
        self.impacts.append(&mut self.pw.impacts);
        // An element past its life frees its body (0x4a2909..0x4a291d).
        let pw = &mut self.pw;
        self.fx.retain(|b| {
            let live = time.wrapping_sub(b.begin) < b.life;
            if !live {
                pw.destroy_body(b.body);
            }
            live
        });
    }

    /// `effect` played at `frame` at `now` (`FX_SpawnEffect` into
    /// `FX_SpawnElem`, cod4-effects.md 9.1.1): its seed off the CRT's
    /// `rand()`, or its start under flag 0x1000, and each one-shot physics
    /// element's instances made as world 1 bodies with their visual's
    /// model and preset (0x4a1ff0). One the pool has no room for, or whose
    /// model has no preset, is dropped, as retail frees the element.
    pub fn play_fx(
        &mut self,
        world: &Cod4World,
        effect: &Arc<PhysEffect>,
        frame: &EffectFrame,
        now: i32,
        rand: &mut MsvcRand,
    ) {
        self.start(now);
        let seed = if effect.timed_seed {
            fx_phys::effect_seed_at(now)
        } else {
            fx_phys::effect_seed(rand.next() as i32)
        };
        for (elem, e) in effect.elems.iter().enumerate() {
            // the sequence is a byte (0x4a263a)
            for k in 0..fx_phys::one_shot_count(&e.def, seed).clamp(0, 256) {
                let (begin, s) = fx_phys::elem_seed(&e.def, seed, now, k as u8);
                let spawn = fx_phys::spawn(&e.def, e.models.len(), frame, s);
                let Some(model) = e.models.get(spawn.visual) else {
                    continue;
                };
                let Some(m) = world.parts.get(&model.to_ascii_lowercase()) else {
                    continue;
                };
                let Some(body) = self.pw.fx_body(&spawn, &m.model, &m.preset.obj, &self.phys)
                else {
                    continue;
                };
                self.fx.push(FxBody {
                    body,
                    model: model.clone(),
                    effect: effect.clone(),
                    elem,
                    seed: s,
                    begin,
                    life: spawn.life,
                });
            }
        }
    }

    /// The physics model elements drawn at `time`: each at its body's
    /// interpolated pose (0x498340), its axes scaled by the element's
    /// scale then (0x498399).
    pub fn fx_models(&self, time: i32) -> Vec<crate::fx::sim::FxModel> {
        self.fx
            .iter()
            .filter_map(|b| {
                let (origin, rot) = self.pw.pose(b.body)?;
                let def = &b.effect.elems[b.elem].def;
                let scale = fx_phys::scale_at(def, b.seed, time.wrapping_sub(b.begin), b.life);
                let m = glam::Mat3::from_quat(rot) * scale;
                Some(crate::fx::sim::FxModel {
                    model: b.model.clone(),
                    origin,
                    axis: [m.x_axis, m.y_axis, m.z_axis],
                })
            })
            .collect()
    }

    /// Where launched part `e`, drawn as `model`, is at `time`: its body
    /// made on the first frame within [`LATE_MS`] of the launch and pushed
    /// by `Phys_ObjBulletImpact` off `pos.trDelta` (the contact) and
    /// `apos.trDelta` (the force), drawn at its interpolated state as
    /// world 1 last stepped it ([`Self::advance`]). A new launch or model
    /// on the entity destroys the old body (0x4351c7). `None` draws the
    /// part at its `trBase`s.
    pub fn place(
        &mut self,
        e: &EntityState,
        model: &str,
        world: &Cod4World,
        time: i32,
    ) -> Option<(Vec3, Vec3)> {
        let launched = ent_u32(e, "lerp.pos.trTime") as i32;
        let n = e.number();
        let fresh = self
            .parts
            .get(&n)
            .is_none_or(|p| p.launched != launched || p.model != model);
        if fresh {
            if let Some((id, _)) = self.parts.remove(&n).and_then(|p| p.body) {
                self.pw.destroy_body(id);
            }
            let body = world
                .parts
                .get(&model.to_ascii_lowercase())
                .filter(|_| time.wrapping_sub(launched) <= LATE_MS)
                .and_then(|m| Some((self.make(e, m, time)?, m.preset.bullet_force_scale)));
            self.parts.insert(
                n,
                Part {
                    launched,
                    model: model.to_string(),
                    body,
                },
            );
        }
        let (id, _) = self.parts.get(&n)?.body?;
        let (origin, rot) = self.pw.pose(id)?;
        Some((origin, Vec3::from(quat_angles(rot))))
    }

    /// `CG_CreatePhysicsObject` (0x434c70): `Phys_ObjCreate` at the
    /// entity's `trBase`s with the model's collision, then the launch's
    /// push at world 1's `msecStep` (the `push 0x1` into 0x5a5a20 at
    /// 0x434dda).
    fn make(&mut self, e: &EntityState, m: &PhysModel, time: i32) -> Option<BodyId> {
        self.start(time);
        let axes =
            vcod_common::x87::angles_to_axis_msvc(vec3(e, "lerp.apos.trBase")).map(|v| v.into());
        let pos = vec3(e, "lerp.pos.trBase").into();
        let id = self.pw.create_from(m, pos, &axes, false, &self.phys)?;
        let (dir, speed) = phys::normalize(vec3(e, "lerp.apos.trDelta").into());
        let msec = self.pw.clock.msec_step;
        let b = self.pw.world.body_mut(id);
        let at = vec3(e, "lerp.pos.trDelta").into();
        phys::obj_bullet_impact(
            b,
            msec,
            at,
            dir,
            speed,
            m.preset.bullet_force_scale,
            &self.phys,
        );
        Some(id)
    }

    /// A client-side round's first contact `at` on entity `n`, along
    /// `dir`: a part that flies takes `Phys_ObjBulletImpact` at `force`
    /// (`dynEnt_bulletForce`) and its preset's `bulletForceScale`
    /// (0x48df10, from the round's walk at 0x455b92). Retail names world
    /// 0 for the part's world 1 (0x48e043): the push is a force at world
    /// 0's `msecStep`, `msec0`, spent over world 1's next step. Whether one
    /// took it.
    pub fn shoot(&mut self, n: u32, at: Vec3, dir: Vec3, force: f32, msec0: i32) -> bool {
        let Some((id, scale)) = self.parts.get(&n).and_then(|p| p.body) else {
            return false;
        };
        let dir = phys::normalize(dir.into()).0;
        let b = self.pw.world.body_mut(id);
        phys::obj_bullet_impact(b, msec0, at.into(), dir, force, scale, &self.phys);
        true
    }

    /// Drops the parts whose entities `keep` refuses, and their bodies.
    pub fn retain(&mut self, keep: impl Fn(u32) -> bool) {
        let pw = &mut self.pw;
        self.parts.retain(|n, p| {
            let kept = keep(*n);
            if !kept && let Some((id, _)) = p.body {
                pw.destroy_body(id);
            }
            kept
        });
    }

    /// Part `n`'s body, once one flies.
    #[cfg(test)]
    pub fn body(&self, n: u32) -> Option<&vcod_cod4::ode::Body> {
        let (id, _) = self.parts.get(&n)?.body?;
        Some(self.pw.world.body(id))
    }

    /// How many parts are seen, and how many of them fly.
    pub fn counts(&self) -> (usize, usize) {
        let flying = self.parts.values().filter(|p| p.body.is_some()).count();
        (self.parts.len(), flying)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// mp_bog's watermelon's `destroyFx` played at its pose: its six
    /// chunks fly as world 1 bodies drawn at their poses, fall, and free
    /// their bodies when their lives (10 to 15 s) run out; a pool with
    /// room for two makes two. Returns at once without an install.
    #[test]
    fn a_watermelons_chunks_fly_as_world_1_bodies_until_their_lives_end() {
        let Some(install) = vcod_cod4::testing::cod4_install() else {
            return;
        };
        let world = crate::cod4_fly::load_world(install.root(), "mp_bog").unwrap();
        let effect = world.fx_phys[&crate::cod4_fx::key("props/watermelon")].clone();
        let melon = world
            .dynents
            .iter()
            .find(|d| d.destroy_fx.as_deref() == Some("props/watermelon"))
            .unwrap();
        let m = glam::Mat3::from_quat(melon.rot);
        let frame = EffectFrame {
            origin: melon.origin.into(),
            axes: [m.x_axis.into(), m.y_axis.into(), m.z_axis.into()],
        };
        let mut rand = MsvcRand::default();
        let mut p = Cod4Physics::default();
        p.play_fx(&world, &effect, &frame, 1000, &mut rand);
        assert_eq!(p.pw.bodies(), 6);
        let start = p.fx_models(1000);
        assert_eq!(start.len(), 6);
        for t in (1017..=1500).step_by(17) {
            p.advance(&world, t, &mut rand);
        }
        let later = p.fx_models(1500);
        assert_eq!(later.len(), 6);
        for (a, b) in start.iter().zip(&later) {
            assert_eq!(a.model, b.model);
            assert!(b.origin.distance(a.origin) > 1.0, "{} held still", a.model);
        }
        p.advance(&world, 16_500, &mut rand);
        assert_eq!(p.pw.bodies(), 0, "a body outlived its element");
        assert!(p.fx_models(16_500).is_empty());

        let mut full = Cod4Physics::default();
        full.pw.others = vcod_cod4::ode::phys::MAX_BODIES - 2;
        full.play_fx(&world, &effect, &frame, 1000, &mut rand);
        assert_eq!(full.fx_models(1000).len(), 2);
    }
}
