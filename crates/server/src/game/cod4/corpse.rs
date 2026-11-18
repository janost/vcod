//! CoD4 1.7's side of a clone's move (`game::corpse`): `PlayerCmd_ClonePlayer`
//! (`cod4_lnxded` 0x80acf50) launches the clone and `G_RunCorpseMove`
//! (0x80b97e0) moves it each frame of the entity pass over the clip map,
//! in 1.7's trajectory numbering, ragdoll types included
//! (docs/research/cod4-combat.md 5.1, 5.2).

use glam::Vec3;
use vcod_cod4::collision::ClipMap;
use vcod_common::net::protocol::Cod1Wire;

use vcod_cod2::player::BodyFrame;

use crate::game::bodies::Body;
use crate::game::corpse::{
    CLIPMASK, CONTENTS_NODROP, CorpseAnim, CorpseWorld, Kinds, MAX_VELOCITY, MINS, Moved, Sweep,
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
/// How far above a clone stuck in solid its second sweep starts
/// (0x821b094, read at 0x80b9e31).
const UNSTICK_RAISE: f32 = 32.0;
/// `CONTENTS_PLAYERCLIP`, which the second sweep leaves out (0x80b9e55).
const CONTENTS_PLAYERCLIP: u32 = 0x1_0000;

/// [`crate::game::corpse::launch`] at 1.7's defaults: `TR_GRAVITY`, the
/// cap `g_clonePlayerMaxVelocity`'s 80 (`tools/re/cod4_dvars.py`). `height`
/// is the live body's `r.maxs[2]`: the killed callback clones it before
/// `player_die` lowers it to 30 (0x80b1c75, 0x80b1da0; cod4-combat.md 5.2).
pub fn launch(
    body: &mut Body,
    origin: Vec3,
    velocity: Vec3,
    height: f32,
    now_ms: i32,
    p: &Cod1Wire,
) {
    let maxs = Vec3::new(15.0, 15.0, height);
    crate::game::corpse::launch(
        body,
        origin,
        velocity,
        now_ms,
        MAX_VELOCITY,
        PLAIN.fall,
        maxs,
        p,
    );
}

/// The clip map's trace for the clone: the lerp by the fraction both ways
/// (0x80b9983..0x80b99fd), `startsolid` read as stuck, `allsolid` as
/// embedded, the world as the ground (cod4-combat.md 5.2).
impl CorpseWorld for ClipMap {
    fn kinds(&self, now: i32) -> Kinds {
        if (9..=11).contains(&now) {
            RAGDOLL
        } else {
            PLAIN
        }
    }

    fn sweep(&self, from: Vec3, to: Vec3, maxs: Vec3, _fall: bool) -> Sweep {
        let tr = self.trace(from, to, MINS, maxs, CLIPMASK);
        Sweep {
            fraction: if tr.startsolid { 0.0 } else { tr.fraction },
            end: crate::game::corpse::lerp(from, to, tr.fraction),
            normal: tr.normal,
            embedded: tr.allsolid,
            ground: ENTITYNUM_WORLD,
        }
    }

    fn clear(&self, from: Vec3, to: Vec3, maxs: Vec3) -> bool {
        let tr = self.trace(from, to, MINS, maxs, CLIPMASK);
        tr.fraction >= 1.0 && !tr.startsolid
    }

    /// A fall embedded all along sweeps again from 32 over `at` to the
    /// gravity point, playerclip left out, and lerps from that start back
    /// toward `at` by its fraction; still embedded, it stays at `at`
    /// (0x80b9e1b..0x80b9efe).
    fn unstick(&self, at: Vec3, to: Vec3, maxs: Vec3) -> Option<Sweep> {
        let from = Vec3::new(
            at.x,
            at.y,
            (f64::from(UNSTICK_RAISE) + f64::from(at.z)) as f32,
        );
        let tr = self.trace(from, to, MINS, maxs, CLIPMASK & !CONTENTS_PLAYERCLIP);
        Some(Sweep {
            fraction: tr.fraction,
            end: if tr.allsolid {
                at
            } else {
                crate::game::corpse::lerp(from, at, tr.fraction)
            },
            normal: tr.normal,
            embedded: tr.allsolid,
            ground: ENTITYNUM_WORLD,
        })
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
    /// the corpse's tree does not play yet at 0 and steps the anims the
    /// rounds' traces pose ([`Self::animate_corpse`]).
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
            let born = body.born_ms;
            if let Some(f) = corpse_frame(body, p) {
                self.animate_corpse(n, born, &f);
            }
        }
        for n in freed {
            self.rt.host.bodies.free(n);
        }
    }

    /// The corpses a round meets: each with its fresh bit, whose contents
    /// (0x4004000 from `clonePlayer`, 0x80ad1d8) carry the shot mask's
    /// 0x4000 until `BodyEnd` leaves 0x4000000 (0x80a8630).
    pub(super) fn shot_corpses(&mut self) -> Vec<ShotCorpse> {
        let p = &super::COD4_HOST_PROTO;
        self.rt
            .host
            .bodies
            .bodies_mut()
            .filter(|b| crate::game::corpse::field(&b.state, p, "eFlags") & EF_FRESH != 0)
            .filter_map(|b| {
                Some(ShotCorpse {
                    number: b.state.number,
                    born_ms: b.born_ms,
                    frame: corpse_frame(b, p)?,
                    maxs: b.cod2?.maxs,
                })
            })
            .collect()
    }
}

/// A corpse a round can meet ([`Cod4Level::shot_corpses`]): its number,
/// birth, frame and box.
pub(super) struct ShotCorpse {
    pub number: u32,
    pub born_ms: i32,
    pub frame: BodyFrame,
    pub maxs: Vec3,
}

/// `clonePlayer`'s fresh corpse bit, which `BodyEnd` clears.
const EF_FRESH: i32 = 0x8_0000;

/// What a corpse's anims and hit boxes read off it: `r.currentOrigin`, its
/// angles, its two anims, `eFlags` and the client it was.
fn corpse_frame(b: &Body, p: &Cod1Wire) -> Option<BodyFrame> {
    let c = b.cod2.as_ref()?;
    let get = |n: &str| crate::game::corpse::field(&b.state, p, n);
    Some(BodyFrame {
        origin: c.origin,
        angles: crate::game::corpse::field_vec(&b.state, p, "apos.trBase").to_array(),
        legs_anim: get("legsAnim"),
        torso_anim: get("torsoAnim"),
        eflags: get("eFlags"),
        client_num: get("clientNum"),
        ..Default::default()
    })
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
            70.0,
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
            70.0,
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
        assert_eq!(c.maxs, Vec3::new(15.0, 15.0, 70.0));
    }
}
