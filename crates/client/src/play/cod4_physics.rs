//! `physicsLaunch`ed parts on the client: `CG_CalcEntityPhysicsPositions`
//! (iw3mp.exe 0x434e50). The server sends a launched part's launch once
//! and never moves it; each client flies it itself
//! ([`vcod_cod4::physics::Body`], cod4-gsc.md 3.6).

use std::collections::HashMap;

use glam::Vec3;
use vcod_cod4::net::delta::EntityState;
use vcod_cod4::physics::{Body, Dvars, PartModel, clip_trace};

use super::cod4::{ent_f32, ent_u32};
use super::cod4_predict::Cod4World;

/// `TR_PHYSICS`.
pub const TR_PHYSICS: u32 = 8;
/// A body is made only within this long of its launch (0x434ead): a
/// client that sees the launch later draws the part where it stood.
const LATE_MS: i32 = 1000;

/// One launched entity: its launch time and model, and its body, `None`
/// when it was seen too late or its model has no preset.
struct Part {
    launched: i32,
    model: String,
    body: Option<Body>,
}

/// Every launched part a session has seen, by entity number.
#[derive(Default)]
pub struct Cod4Physics {
    parts: HashMap<u32, Part>,
    pub dvars: Dvars,
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
    /// Where launched part `e`, drawn as `model`, is at `time`: its body
    /// made on the first frame within [`LATE_MS`] of the launch and pushed
    /// by `Phys_ObjBulletImpact` off `pos.trDelta` (the contact) and
    /// `apos.trDelta` (the force), stepped to `time`. `None` draws the
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
            let body = world
                .parts
                .get(&model.to_ascii_lowercase())
                .filter(|_| time.wrapping_sub(launched) <= LATE_MS)
                .map(|m| self.make(e, m, time));
            self.parts.insert(
                n,
                Part {
                    launched,
                    model: model.to_string(),
                    body,
                },
            );
        }
        let body = self.parts.get_mut(&n)?.body.as_mut()?;
        let mut trace = clip_trace(&world.clip);
        body.advance(time, &mut trace, &self.dvars);
        Some(body.pose(time))
    }

    fn make(&self, e: &EntityState, m: &PartModel, time: i32) -> Body {
        let mut b = Body::new(
            vec3(e, "lerp.pos.trBase"),
            vec3(e, "lerp.apos.trBase"),
            &m.shape,
            &m.preset,
            time,
        );
        let force = vec3(e, "lerp.apos.trDelta");
        b.launch(
            vec3(e, "lerp.pos.trDelta"),
            force,
            m.preset.bullet_force_scale,
            &self.dvars,
        );
        b
    }

    /// Drops the parts whose entities `keep` refuses.
    pub fn retain(&mut self, keep: impl Fn(u32) -> bool) {
        self.parts.retain(|n, _| keep(*n));
    }

    /// Part `n`'s body, once one flies.
    #[cfg(test)]
    pub fn body(&self, n: u32) -> Option<&Body> {
        self.parts.get(&n)?.body.as_ref()
    }

    /// How many parts are seen, and how many of them fly.
    pub fn counts(&self) -> (usize, usize) {
        let flying = self.parts.values().filter(|p| p.body.is_some()).count();
        (self.parts.len(), flying)
    }
}
