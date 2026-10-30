//! CoD4 1.7's side of a clone's move (`game::corpse`): `PlayerCmd_ClonePlayer`
//! (`cod4_lnxded` 0x80acf50) launches the clone and `G_RunCorpseMove`
//! (0x80b97e0) moves it each frame of the entity pass over the clip map,
//! in 1.7's trajectory numbering, ragdoll types included
//! (docs/research/cod4-combat.md 5.1, 5.2).

use glam::Vec3;
use vcod_cod4::collision::ClipMap;
use vcod_common::net::protocol::Protocol;

use crate::game::bodies::Body;
use crate::game::corpse::{
    CLIPMASK, CONTENTS_NODROP, CorpseAnim, CorpseWorld, Kinds, MAX_VELOCITY, MAXS, MINS, Moved,
    Sweep,
};

/// 1.7's `TR_GRAVITY` (0x80ad004) and `TR_INTERPOLATE`, and the ragdoll
/// pair `startRagdoll` switches a clone to (9..11; cod4-combat.md 5.1):
/// its rest is 11, its fall 10 (0x80b9a74..0x80b9a96, 0x80b9b66).
const PLAIN: Kinds = Kinds { rest: 1, fall: 5 };
const RAGDOLL: Kinds = Kinds { rest: 11, fall: 10 };
#[cfg(test)]
const TR_GRAVITY: i32 = PLAIN.fall;
/// The world's entity number: what a stopped fall names as its ground.
const ENTITYNUM_WORLD: i32 = 1022;

/// [`crate::game::corpse::launch`] at 1.7's defaults: `TR_GRAVITY`, the
/// cap `g_clonePlayerMaxVelocity`'s 80 (`tools/re/cod4_dvars.py`).
pub fn launch(body: &mut Body, origin: Vec3, velocity: Vec3, now_ms: i32, p: &Protocol) {
    crate::game::corpse::launch(body, origin, velocity, now_ms, MAX_VELOCITY, PLAIN.fall, p);
}

/// The clip map's trace for the clone: the lerp by the fraction both ways
/// (0x80b9983..0x80b99fd), `startsolid` read as stuck, `allsolid` as
/// embedded, the world as the ground. The bounce's branches are INFERRED
/// on 1.7 (`G_BounceCorpse`, CoD2's measured shape).
impl CorpseWorld for ClipMap {
    fn kinds(&self, now: i32) -> Kinds {
        if (9..=11).contains(&now) {
            RAGDOLL
        } else {
            PLAIN
        }
    }

    fn sweep(&self, from: Vec3, to: Vec3, _fall: bool) -> Sweep {
        let tr = self.trace(from, to, MINS, MAXS, CLIPMASK);
        Sweep {
            fraction: if tr.startsolid { 0.0 } else { tr.fraction },
            end: crate::game::corpse::lerp(from, to, tr.fraction),
            normal: tr.normal,
            embedded: tr.allsolid,
            ground: ENTITYNUM_WORLD,
        }
    }

    fn clear(&self, from: Vec3, to: Vec3) -> bool {
        let tr = self.trace(from, to, MINS, MAXS, CLIPMASK);
        tr.fraction >= 1.0 && !tr.startsolid
    }

    fn no_drop(&self, at: Vec3) -> bool {
        self.point_contents(at) & CONTENTS_NODROP != 0
    }
}

impl super::level::Cod4Level {
    /// Every corpse's `G_RunCorpse` on the level's clock, in the entity
    /// pass: its death anim's node time stepped by `G_XAnimUpdate`
    /// (`0.001 * level.frametime` at rate 1), the move with that step's
    /// root motion, then `G_RunCorpseAnimate`, which starts a legs anim
    /// the corpse's tree does not play yet at 0.
    pub(super) fn run_corpses(&mut self) {
        let now = self.level_time();
        let p = &super::COD4_HOST_PROTO;
        let Some(clip) = self.rt.host.cod4().clip.clone() else {
            return;
        };
        let dtime = (f64::from(0.001f32) * f64::from(self.frame_time() as f32)) as f32;
        let numbers: Vec<(u32, i32)> = self
            .rt
            .host
            .bodies
            .bodies_mut()
            .filter(|b| b.cod2.is_some())
            .map(|b| {
                (
                    b.state.number,
                    crate::game::corpse::field(&b.state, p, "legsAnim"),
                )
            })
            .collect();
        let mut freed = Vec::new();
        for (n, legs) in numbers {
            let c4 = self.rt.host.cod4();
            let root = c4
                .anim_names
                .get((legs & 0x1ff) as usize)
                .and_then(|name| c4.anim_clips.get(&name.to_ascii_lowercase()))
                .cloned();
            let Some(body) = self.rt.host.bodies.body_mut(n) else {
                continue;
            };
            let Some(c) = body.cod2.as_mut() else {
                continue;
            };
            let mut delta = Vec3::ZERO;
            if let (Some(a), Some(r)) =
                (c.anim.as_mut(), root.as_ref().and_then(|a| a.root.as_ref()))
            {
                let old = a.time;
                a.time = r.step(old, dtime);
                delta = r.delta(old, a.time);
            }
            if crate::game::corpse::step(&*clip, body, now, delta, p) == Moved::Freed {
                freed.push(n);
                continue;
            }
            if let Some(c) = &mut body.cod2
                && c.anim.is_none_or(|a| a.wire != legs)
            {
                c.anim = Some(CorpseAnim {
                    wire: legs,
                    time: 0.0,
                });
            }
        }
        for n in freed {
            self.rt.host.bodies.free(n);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::bodies::BodyQueue;
    use crate::game::corpse::{field as get, field_vec as get_vec, put as set};
    use vcod_common::net::msg::EntityState;

    /// The body queue's slot a ninth death reuses starts a fresh clone:
    /// falling from the new origin at the capped velocity, no ground, no
    /// death anim yet, whatever the old corpse was doing.
    #[test]
    fn a_reused_body_slot_launches_a_fresh_corpse() {
        let p = &super::super::COD4_HOST_PROTO;
        let mut q = BodyQueue::new(1);
        let first = q.push_raw(EntityState::null(p), 1000, None, p, None);
        let body = q.body_mut(first).unwrap();
        launch(
            body,
            Vec3::new(1.0, 2.0, 3.0),
            Vec3::new(120.0, -300.0, 90.0),
            1000,
            p,
        );
        assert_eq!(
            get_vec(&body.state, p, "pos.trDelta"),
            Vec3::new(80.0, -300.0, 90.0)
        );
        // At rest on its death anim.
        set(&mut body.state, p, "pos.trType", RAGDOLL.rest);
        set(&mut body.state, p, "groundEntityNum", ENTITYNUM_WORLD);
        body.cod2.as_mut().unwrap().anim = Some(CorpseAnim { wire: 7, time: 0.5 });
        let again = q.push_raw(EntityState::null(p), 9000, None, p, None);
        assert_eq!(again, first);
        let body = q.body_mut(again).unwrap();
        assert!(body.cod2.is_none());
        launch(
            body,
            Vec3::new(10.0, 20.0, 30.0),
            Vec3::new(5.0, 6.0, 7.0),
            9000,
            p,
        );
        let e = &body.state;
        assert_eq!(get(e, p, "pos.trType"), TR_GRAVITY);
        assert_eq!(get(e, p, "pos.trTime"), 9000);
        assert_eq!(get(e, p, "groundEntityNum"), 1023);
        assert_eq!(get_vec(e, p, "pos.trBase"), Vec3::new(10.0, 20.0, 30.0));
        assert_eq!(get_vec(e, p, "pos.trDelta"), Vec3::new(5.0, 6.0, 7.0));
        let c = body.cod2.unwrap();
        assert_eq!((c.origin, c.anim), (Vec3::new(10.0, 20.0, 30.0), None));
    }
}
