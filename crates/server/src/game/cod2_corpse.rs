//! CoD2's side of a clone's move (`game::corpse`): `PlayerCmd_ClonePlayer`
//! (Mac 0x1c3574) launches the body queue's clone and `G_RunCorpse` (Mac
//! 0x1b0882) moves it each frame over the map's `CollisionWorld`.
//! `BodyEnd` (Mac 0x1c14d4) is the body queue's own fresh-bit think.
//! docs/research/cod2-combat.md 6.2.

use glam::Vec3;
use vcod_common::collision::CollisionWorld;
use vcod_common::net::protocol::Protocol;

use super::bodies::Body;
use super::corpse::{CLIPMASK, CONTENTS_NODROP, CorpseAnim, CorpseWorld, Kinds, MAXS, MINS};
pub use super::corpse::{MAX_VELOCITY, Moved};

/// CoD2's clone falls with `TR_GRAVITY` and rests with `TR_INTERPOLATE`
/// in its own numbering.
const KINDS: Kinds = Kinds {
    rest: vcod_cod2::trajectory::TR_INTERPOLATE,
    fall: vcod_cod2::trajectory::TR_GRAVITY,
};

/// [`super::corpse::launch`] in CoD2's numbering.
pub fn launch(
    body: &mut Body,
    origin: Vec3,
    velocity: Vec3,
    now_ms: i32,
    max_velocity: f32,
    p: &Protocol,
) {
    super::corpse::launch(body, origin, velocity, now_ms, max_velocity, KINDS.fall, p);
}

/// CoD2's trace for the clone: `allsolid` reads it stuck, `startsolid`
/// rests it where it is, the fall ends at the trace's own end point and
/// a drag at the lerp; the ground is the entity the trace names. INFERRED:
/// the trace fields' names (startsolid, allsolid, entity) off the trace
/// layout.
impl CorpseWorld for CollisionWorld {
    fn kinds(&self, _: i32) -> Kinds {
        KINDS
    }

    fn sweep(&self, from: Vec3, to: Vec3, fall: bool) -> super::corpse::Sweep {
        let tr = self.move_trace(from, to, MINS, MAXS, CLIPMASK);
        super::corpse::Sweep {
            fraction: if tr.allsolid { 0.0 } else { tr.fraction },
            end: if fall {
                tr.endpos
            } else {
                super::corpse::lerp(from, to, tr.fraction)
            },
            normal: tr.normal,
            embedded: tr.startsolid,
            ground: self.entity_num(&tr) as i32,
        }
    }

    /// `SV_TracePassed` (lnxded 0x810a57e).
    fn clear(&self, from: Vec3, to: Vec3) -> bool {
        let tr = self.move_trace(from, to, MINS, MAXS, CLIPMASK);
        tr.fraction >= 1.0 && !tr.startsolid
    }

    fn no_drop(&self, at: Vec3) -> bool {
        self.point_contents(at) & CONTENTS_NODROP != 0
    }
}

/// `G_RunCorpseMove` (lnxded 0x8107846, Mac 0x1b00ae) for one CoD2 body,
/// `delta` its death anim's root motion this frame
/// ([`vcod_cod2::rootdelta::rel_delta`]).
pub fn run(
    world: &CollisionWorld,
    body: &mut Body,
    now_ms: i32,
    delta: Vec3,
    p: &Protocol,
) -> Moved {
    super::corpse::step(world, body, now_ms, delta, p)
}

impl super::cod2::Cod2Level {
    /// Every CoD2 corpse's frame (`G_RunCorpse`), on the level's clock:
    /// its death anim's node time stepped (`G_RunFrame`'s DObj pass), the
    /// move with that step's root motion, then the anim update, which
    /// starts a death anim the corpse's tree does not play yet at 0.
    pub fn run_corpses(&mut self) {
        let now = self.level_time();
        let p = vcod_cod2::net::PROTOCOL_118.as_cod1_shim();
        let world = self.world.clone();
        let fs = self.rt.host.fs.clone();
        // `0.001 * level.frametime`, stored (lnxded 0x811e394).
        let dtime = (f64::from(0.001f32) * f64::from(self.frametime_ms() as f32)) as f32;
        let mut freed = Vec::new();
        let numbers: Vec<u32> = self
            .rt
            .host
            .bodies
            .bodies_mut()
            .map(|b| b.state.number)
            .collect();
        for n in numbers {
            let Some(body) = self.rt.host.bodies.body_mut(n) else {
                continue;
            };
            let Some(c) = body.cod2 else { continue };
            let legs = super::corpse::field(&body.state, p, "legsAnim");
            let clip = |wire: i32, combat: &mut super::cod2_combat::Combat| {
                fs.as_ref().and_then(|fs| combat.anim_clip(&**fs, wire))
            };
            let mut delta = Vec3::ZERO;
            let mut anim = c.anim;
            if let Some(a) = &mut anim
                && let Some(x) = clip(a.wire, &mut self.combat)
            {
                let old = a.time;
                let f = vcod_cod2::rootdelta::frequency(&x);
                a.time = vcod_cod2::rootdelta::step_time(old, dtime, 1.0, f, x.looping);
                delta = vcod_cod2::rootdelta::rel_delta(&x, old, a.time);
            }
            let Some(body) = self.rt.host.bodies.body_mut(n) else {
                continue;
            };
            if let Some(c) = &mut body.cod2 {
                c.anim = anim;
            }
            if run(&world.collision, body, now, delta, p) == Moved::Freed {
                freed.push(n);
                continue;
            }
            // The lerp frame's setter: a new legs anim restarts at 0.
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
    use crate::game::corpse::{field, field_vec};
    use vcod_common::collision::test_world;
    use vcod_common::net::msg::EntityState;

    fn proto() -> &'static Protocol {
        vcod_cod2::net::PROTOCOL_118.as_cod1_shim()
    }

    /// One corpse a frame at a time on `world` from `at` at `velocity`,
    /// `delta` its root motion each frame: per frame its trajectory type,
    /// ground, `pos.trBase`, `pos.trDelta` and yaw, as bits.
    fn frames(
        world: &CollisionWorld,
        at: Vec3,
        velocity: Vec3,
        yaw: f32,
        delta: Vec3,
    ) -> Vec<String> {
        let p = proto();
        let mut q = BodyQueue::new(2);
        let n = q.push_raw(EntityState::null(p), 1000, None, p, None);
        let body = q.body_mut(n).unwrap();
        crate::game::corpse::put_vec(&mut body.state, p, "apos.trBase", Vec3::new(0.0, yaw, 0.0));
        launch(body, at, velocity, 1000, MAX_VELOCITY, p);
        (1..=16)
            .map(|k| {
                let moved = run(world, body, 1000 + 50 * k, delta, p);
                let e = &body.state;
                let bits = |f: &str| field_vec(e, p, f).to_array().map(f32::to_bits);
                let origin = body.cod2.map(|c| c.origin.to_array().map(f32::to_bits));
                format!(
                    "{moved:?} {origin:x?} {} {} {:x?} {:x?} {:x}",
                    field(e, p, "pos.trType"),
                    field(e, p, "groundEntityNum"),
                    bits("pos.trBase"),
                    bits("pos.trDelta"),
                    bits("apos.trBase")[1],
                )
            })
            .collect()
    }

    /// [`frames`]' lines for the five runs below, as the shipped mover left
    /// them.
    const EXPECTED: [&str; 80] = [
        "Kept Some([40800000, bfc00000, 427c0000]) 5 1023 [0, 0, 42800000] [42a00000, c1f00000, 0] 41f00000",
        "Kept Some([41000000, c0400000, 42700000]) 5 1023 [0, 0, 42800000] [42a00000, c1f00000, 0] 41f00000",
        "Kept Some([41400000, c0900000, 425c0000]) 5 1023 [0, 0, 42800000] [42a00000, c1f00000, 0] 41f00000",
        "Kept Some([41800000, c0c00000, 42400000]) 5 1023 [0, 0, 42800000] [42a00000, c1f00000, 0] 41f00000",
        "Kept Some([41a00000, c0f00000, 421c0000]) 5 1023 [0, 0, 42800000] [42a00000, c1f00000, 0] 41f00000",
        "Kept Some([41c00000, c1100000, 41dffffe]) 5 1023 [0, 0, 42800000] [42a00000, c1f00000, 0] 41f00000",
        "Kept Some([41e00001, c1280001, 416ffff8]) 5 1023 [0, 0, 42800000] [42a00000, c1f00000, 0] 41f00000",
        "Kept Some([41ffbbbc, c13fcccd, 3dffffe7]) 1 1022 [41ffbbbc, c13fcccd, 3dffffe7] [0, 0, 0] 41f00000",
        "Kept Some([41ffbbbc, c13fcccd, 3dffffe7]) 1 1022 [41ffbbbc, c13fcccd, 3dffffe7] [0, 0, 0] 41f00000",
        "Kept Some([41ffbbbc, c13fcccd, 3dffffe7]) 1 1022 [41ffbbbc, c13fcccd, 3dffffe7] [0, 0, 0] 41f00000",
        "Kept Some([41ffbbbc, c13fcccd, 3dffffe7]) 1 1022 [41ffbbbc, c13fcccd, 3dffffe7] [0, 0, 0] 41f00000",
        "Kept Some([41ffbbbc, c13fcccd, 3dffffe7]) 1 1022 [41ffbbbc, c13fcccd, 3dffffe7] [0, 0, 0] 41f00000",
        "Kept Some([41ffbbbc, c13fcccd, 3dffffe7]) 1 1022 [41ffbbbc, c13fcccd, 3dffffe7] [0, 0, 0] 41f00000",
        "Kept Some([41ffbbbc, c13fcccd, 3dffffe7]) 1 1022 [41ffbbbc, c13fcccd, 3dffffe7] [0, 0, 0] 41f00000",
        "Kept Some([41ffbbbc, c13fcccd, 3dffffe7]) 1 1022 [41ffbbbc, c13fcccd, 3dffffe7] [0, 0, 0] 41f00000",
        "Kept Some([41ffbbbc, c13fcccd, 3dffffe7]) 1 1022 [41ffbbbc, c13fcccd, 3dffffe7] [0, 0, 0] 41f00000",
        "Kept Some([40800000, 0, 42830000]) 5 1023 [0, 0, 42800000] [42a00000, 0, 42480000] 41f00000",
        "Kept Some([41000000, 0, 42820000]) 5 1023 [0, 0, 42800000] [42a00000, 0, 42480000] 41f00000",
        "Kept Some([41400000, 0, 427a0000]) 5 1023 [0, 0, 42800000] [42a00000, 0, 42480000] 41f00000",
        "Kept Some([41800000, 0, 42680000]) 5 1023 [0, 0, 42800000] [42a00000, 0, 42480000] 41f00000",
        "Kept Some([41a00000, 0, 424e0000]) 5 1023 [0, 0, 42800000] [42a00000, 0, 42480000] 41f00000",
        "Kept Some([41c00000, 0, 422bffff]) 5 1023 [0, 0, 42800000] [42a00000, 0, 42480000] 41f00000",
        "Kept Some([41bf0000, 0, 4222cfff]) 5 1023 [41bf0000, 0, 4222cfff] [0, 0, 0] 41f00000",
        "Kept Some([41bf0000, 0, 421ecfff]) 5 1023 [41bf0000, 0, 4222cfff] [0, 0, 0] 41f00000",
        "Kept Some([41bf0000, 0, 4212cfff]) 5 1023 [41bf0000, 0, 4222cfff] [0, 0, 0] 41f00000",
        "Kept Some([41bf0000, 0, 41fd9ffe]) 5 1023 [41bf0000, 0, 4222cfff] [0, 0, 0] 41f00000",
        "Kept Some([41bf0000, 0, 41c59ffe]) 5 1023 [41bf0000, 0, 4222cfff] [0, 0, 0] 41f00000",
        "Kept Some([41bf0000, 0, 417b3ffc]) 5 1023 [41bf0000, 0, 4222cfff] [0, 0, 0] 41f00000",
        "Kept Some([41bf0000, 0, 40967ff0]) 5 1023 [41bf0000, 0, 4222cfff] [0, 0, 0] 41f00000",
        "Kept Some([41bf0000, 0, 3dfffff4]) 1 1022 [41bf0000, 0, 3dfffff4] [0, 0, 0] 41f00000",
        "Kept Some([41bf0000, 0, 3dfffff4]) 1 1022 [41bf0000, 0, 3dfffff4] [0, 0, 0] 41f00000",
        "Kept Some([41bf0000, 0, 3dfffff4]) 1 1022 [41bf0000, 0, 3dfffff4] [0, 0, 0] 41f00000",
        "Kept Some([0, 0, 3e000000]) 1 1022 [0, 0, 3e000000] [0, 0, 0] 41f00000",
        "Kept Some([405646e1, 3e4dc8f6, 3e000000]) 1 1022 [405646e1, 3e4dc8f6, 3e000000] [0, 0, 0] 41f00000",
        "Kept Some([40d646e1, 3ecdc8f7, 3e000000]) 1 1022 [40d646e1, 3ecdc8f7, 3e000000] [0, 0, 0] 41f00000",
        "Kept Some([4120b529, 3f1a56ba, 3e000000]) 1 1022 [4120b529, 3f1a56ba, 3e000000] [0, 0, 0] 41f00000",
        "Kept Some([415646e1, 3f4dc8f6, 3e000000]) 1 1022 [415646e1, 3f4dc8f6, 3e000000] [0, 0, 0] 41f00000",
        "Kept Some([4185ec4c, 3f809d9b, 3e000000]) 1 1022 [4185ec4c, 3f809d9b, 3e000000] [0, 0, 0] 41f00000",
        "Kept Some([41a0b528, 3f9a56bb, 3e000000]) 1 1022 [41a0b528, 3f9a56bb, 3e000000] [0, 0, 0] 41f00000",
        "Kept Some([41bb7e04, 3fb40fdb, 3e000000]) 1 1022 [41bb7e04, 3fb40fdb, 3e000000] [0, 0, 0] 41f00000",
        "Kept Some([41d646e0, 3fcdc8fb, 3e000000]) 1 1022 [41d646e0, 3fcdc8fb, 3e000000] [0, 0, 0] 41f00000",
        "Kept Some([41f10fbc, 3fe7821b, 3e000000]) 1 1022 [41f10fbc, 3fe7821b, 3e000000] [0, 0, 0] 41f00000",
        "Kept Some([4205ec4c, 40009d9d, 3e000000]) 1 1022 [4205ec4c, 40009d9d, 3e000000] [0, 0, 0] 41f00000",
        "Kept Some([421350ba, 400d7a2c, 3e000000]) 1 1022 [421350ba, 400d7a2c, 3e000000] [0, 0, 0] 41f00000",
        "Kept Some([4220b528, 401a56bb, 3e000000]) 1 1022 [4220b528, 401a56bb, 3e000000] [0, 0, 0] 41f00000",
        "Kept Some([422e1996, 4027334a, 3e000000]) 1 1022 [422e1996, 4027334a, 3e000000] [0, 0, 0] 41f00000",
        "Kept Some([423b7e04, 40340fd9, 3e000000]) 1 1022 [423b7e04, 40340fd9, 3e000000] [0, 0, 0] 41f00000",
        "Kept Some([4248e272, 4040ec67, 3e000000]) 1 1022 [4248e272, 4040ec67, 3e000000] [0, 0, 0] 41f00000",
        "Kept Some([447a0000, 0, 3e000000]) 1 1022 [447a0000, 0, 3e000000] [0, 0, 0] 0",
        "Kept Some([447b0000, 0, 3e000000]) 1 1022 [447b0000, 0, 3e000000] [0, 0, 0] 0",
        "Kept Some([447c0000, 0, 3e000000]) 1 1022 [447c0000, 0, 3e000000] [0, 0, 0] 0",
        "Kept Some([447d0000, 0, 3e000000]) 1 1022 [447d0000, 0, 3e000000] [0, 0, 0] 0",
        "Kept Some([447e0000, 0, 3e000000]) 1 1022 [447e0000, 0, 3e000000] [0, 0, 0] 0",
        "Kept Some([447f0000, 0, 3e000000]) 1 1022 [447f0000, 0, 3e000000] [0, 0, 0] 0",
        "Kept Some([44800000, 0, 3e000000]) 1 1022 [44800000, 0, 3e000000] [0, 0, 0] 0",
        "Kept Some([44808000, 0, 3e000000]) 1 1022 [44808000, 0, 3e000000] [0, 0, 0] 0",
        "Kept Some([44810000, 0, 3e000000]) 1 1022 [44810000, 0, 3e000000] [0, 0, 0] 0",
        "Kept Some([44818000, 0, 3e000000]) 1 1022 [44818000, 0, 3e000000] [0, 0, 0] 0",
        "Kept Some([44820000, 0, 3e000000]) 5 1022 [44820000, 0, 3e000000] [40800000, 0, 0] 0",
        "Kept Some([44820666, 0, bf600000]) 5 1022 [44820000, 0, 3e000000] [40800000, 0, 0] 0",
        "Kept Some([44820ccd, 0, c0780000]) 5 1022 [44820000, 0, 3e000000] [40800000, 0, 0] 0",
        "Kept Some([44821333, 0, c10e0001]) 5 1022 [44820000, 0, 3e000000] [40800000, 0, 0] 0",
        "Kept Some([4482199a, 0, c17e0000]) 5 1022 [44820000, 0, 3e000000] [40800000, 0, 0] 0",
        "Kept Some([44822000, 0, c1c70000]) 5 1022 [44820000, 0, 3e000000] [40800000, 0, 0] 0",
        "Kept Some([0, 0, 3e000000]) 1 1022 [0, 0, 3e000000] [0, 0, 0] 0",
        "Kept Some([0, 0, 3e000000]) 1 1022 [0, 0, 3e000000] [0, 0, 0] 0",
        "Kept Some([0, 0, 3e000000]) 1 1022 [0, 0, 3e000000] [0, 0, 0] 0",
        "Kept Some([0, 0, 3e000000]) 1 1022 [0, 0, 3e000000] [0, 0, 0] 0",
        "Kept Some([0, 0, 3e000000]) 1 1022 [0, 0, 3e000000] [0, 0, 0] 0",
        "Kept Some([0, 0, 3e000000]) 1 1022 [0, 0, 3e000000] [0, 0, 0] 0",
        "Kept Some([0, 0, 3e000000]) 1 1022 [0, 0, 3e000000] [0, 0, 0] 0",
        "Kept Some([0, 0, 3e000000]) 1 1022 [0, 0, 3e000000] [0, 0, 0] 0",
        "Kept Some([0, 0, 3e000000]) 1 1022 [0, 0, 3e000000] [0, 0, 0] 0",
        "Kept Some([0, 0, 3e000000]) 1 1022 [0, 0, 3e000000] [0, 0, 0] 0",
        "Kept Some([0, 0, 3e000000]) 1 1022 [0, 0, 3e000000] [0, 0, 0] 0",
        "Kept Some([0, 0, 3e000000]) 1 1022 [0, 0, 3e000000] [0, 0, 0] 0",
        "Kept Some([0, 0, 3e000000]) 1 1022 [0, 0, 3e000000] [0, 0, 0] 0",
        "Kept Some([0, 0, 3e000000]) 1 1022 [0, 0, 3e000000] [0, 0, 0] 0",
        "Kept Some([0, 0, 3e000000]) 1 1022 [0, 0, 3e000000] [0, 0, 0] 0",
        "Kept Some([0, 0, 3e000000]) 1 1022 [0, 0, 3e000000] [0, 0, 0] 0",
    ];

    /// The fall, the landing, the wall's nudge and the drag along the
    /// floor, frame by frame, held to the bit.
    #[test]
    fn a_corpse_falls_lands_and_drags_frame_by_frame() {
        let floor = test_world(&[]);
        let wall = test_world(&[(Vec3::new(40.0, -64.0, 0.0), Vec3::new(56.0, 64.0, 128.0))]);
        let runs = [
            frames(
                &floor,
                Vec3::new(0.0, 0.0, 64.0),
                Vec3::new(200.0, -30.0, 0.0),
                30.0,
                Vec3::ZERO,
            ),
            frames(
                &wall,
                Vec3::new(0.0, 0.0, 64.0),
                Vec3::new(400.0, 0.0, 50.0),
                30.0,
                Vec3::ZERO,
            ),
            frames(
                &floor,
                Vec3::new(0.0, 0.0, 0.125),
                Vec3::ZERO,
                30.0,
                Vec3::new(3.0, -1.5, 0.0),
            ),
            // Dragged off the floor's edge at 1024: it drops at the drag's pace.
            frames(
                &floor,
                Vec3::new(1000.0, 0.0, 0.125),
                Vec3::ZERO,
                0.0,
                Vec3::new(4.0, 0.0, 0.0),
            ),
            // A root motion of a unit or less moves nothing.
            frames(
                &floor,
                Vec3::new(0.0, 0.0, 0.125),
                Vec3::ZERO,
                0.0,
                Vec3::new(0.6, 0.7, 0.0),
            ),
        ];
        assert_eq!(runs.concat(), EXPECTED);
    }
}
