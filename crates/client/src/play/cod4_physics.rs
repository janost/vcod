//! `physicsLaunch`ed parts on the client: `CG_CalcEntityPhysicsPositions`
//! (iw3mp.exe 0x434e50). The server sends a launched part's launch once
//! and never moves it; each client flies it itself in physics world 1
//! (`vcod_cod4::ode::phys`, cod4-gsc.md 3.6).

use std::collections::HashMap;

use glam::Vec3;
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

/// Every launched part a session has seen, by entity number, and
/// physics world 1, which steps their bodies.
pub struct Cod4Physics {
    parts: HashMap<u32, Part>,
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
