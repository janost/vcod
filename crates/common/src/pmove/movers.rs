//! The snapshot's brush models as client prediction meets them: retail's
//! `CG_BuildSolidList` and the brush model arm of `CG_ClipMoveToEntities`,
//! which clip each one where its trajectory has it at the snapshot's time,
//! and `CG_AdjustPositionForMover`, which carries the predicted origin with
//! the ground entity from there to the drawn time
//! (docs/research/cod11-movers.md, section 14).

use crate::collision::CollisionWorld;
use crate::net::msg::EntityState;
use crate::net::protocol::Cod1Wire;
use crate::net::trajectory::Trajectory;
use glam::Vec3;

/// `solid` of an entity the server linked as a brush model.
use crate::net::flags::SOLID_BMODEL;
/// The `eFlags` bit that keeps a brush model out of the solid list
/// (cgame 0x30028d9b); `SP_trigger_*` set it.
const EF_NONSOLID_BMODEL: i32 = 0x2;
use crate::net::flags::ET_MOVER;
use crate::net::flags::ET_SCRIPTMOVER;
use crate::net::flags::ET_VEHICLE_V22;
/// The entity numbers the carry reads (cgame 0x3001baa0): not the world,
/// not `ENTITYNUM_NONE`.
const CARRIERS: std::ops::RangeInclusive<i32> = 1..=0x3fd;

/// One snapshot's brush models and the movers a player can stand on.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SnapshotMovers {
    /// `(entity, inline model, pos, apos, own copy)` of each brush model
    /// clipped. Index 0 is the world's model: `CM_InlineModel(0)` clips the
    /// whole world where the entity stands (movers doc 20). A UO vehicle
    /// clips its own copy of its collmap, which every vehicle of its model
    /// shares (coduo-vehicles.md 10.3).
    clipped: Vec<(u32, usize, Trajectory, Trajectory, bool)>,
    /// `(entity, pos, apos)` of each `eType` 5 or 8 entity, what the carry
    /// reads.
    carriers: Vec<(u32, Trajectory, Trajectory)>,
}

impl SnapshotMovers {
    pub fn from_entities<'a>(
        p: &Cod1Wire,
        entities: impl IntoIterator<Item = (&'a u32, &'a EntityState)>,
    ) -> Self {
        let mut out = SnapshotMovers::default();
        for (&n, e) in entities {
            let etype = e.field_i32(p, "eType");
            if etype == ET_MOVER || etype == ET_SCRIPTMOVER {
                out.carriers.push((
                    n,
                    Trajectory::read(e, p, "pos"),
                    Trajectory::read(e, p, "apos"),
                ));
            }
            let index = e.field_i32(p, "index");
            if e.field_i32(p, "solid") == SOLID_BMODEL
                && e.field_i32(p, "eFlags") & EF_NONSOLID_BMODEL == 0
                && index >= 0
            {
                out.clipped.push((
                    n,
                    index as usize,
                    Trajectory::read(e, p, "pos"),
                    Trajectory::read(e, p, "apos"),
                    !p.vehicle_fields.is_empty() && etype == ET_VEHICLE_V22,
                ));
            }
        }
        out
    }

    /// Puts every clipped brush model where its trajectory has it at `t`,
    /// named by its entity, and takes every other submodel and world copy
    /// out of the clip: the client's world trace holds model 0 alone and
    /// meets a brush model only through the snapshot's entity. The brush
    /// model arm of `CG_ClipMoveToEntities` tests no contents, so a copy
    /// clips under every mask.
    pub fn place(&self, world: &CollisionWorld, t: i32) {
        for model in 1..world.model_count() {
            world.set_model_linked(model, false);
        }
        world.clear_world_copies();
        for &(n, model, pos, apos, own) in &self.clipped {
            if model == 0 {
                world.set_world_copy(n, Some((pos.evaluate_f32(t), apos.evaluate_f32(t), -1)));
                continue;
            }
            if own {
                world.set_model_instance(
                    n,
                    model,
                    Some((pos.evaluate_f32(t), apos.evaluate_f32(t), !0)),
                );
                continue;
            }
            world.set_model_linked(model, true);
            world.set_model_entity(model, n);
            world.set_model_pose(model, pos.evaluate_f32(t), apos.evaluate_f32(t));
        }
    }

    /// `origin` moved by what the `ground` entity's `pos` trajectory does
    /// between `from` and `to`. The carry never turns the origin about a
    /// rotating mover (movers doc 14).
    pub fn carry(&self, origin: Vec3, ground: i32, from: i32, to: i32) -> Vec3 {
        match self.carrier(ground) {
            Some((pos, _)) => origin + pos.evaluate_f32(to) - pos.evaluate_f32(from),
            None => origin,
        }
    }

    /// The carry's angle change: what the `ground` entity's `apos` yaw does
    /// between `from` and `to`, in degrees, 0 off a mover.
    pub fn carry_yaw(&self, ground: i32, from: i32, to: i32) -> f32 {
        match self.carrier(ground) {
            Some((_, apos)) => apos.evaluate_f32(to).y - apos.evaluate_f32(from).y,
            None => 0.0,
        }
    }

    fn carrier(&self, ground: i32) -> Option<(&Trajectory, &Trajectory)> {
        if !CARRIERS.contains(&ground) {
            return None;
        }
        self.carriers
            .iter()
            .find(|c| c.0 as i32 == ground)
            .map(|(_, pos, apos)| (pos, apos))
    }

    pub fn is_empty(&self) -> bool {
        self.clipped.is_empty() && self.carriers.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::protocol::PROTOCOL_V1;
    use crate::net::trajectory::{TR_LINEAR_STOP, TR_STATIONARY};

    const P: &Cod1Wire = &PROTOCOL_V1;

    fn ent(num: u32, fields: &[(&str, i32)], pos: Trajectory) -> (u32, EntityState) {
        let mut e = EntityState::null(P);
        e.number = num;
        let mut put = |name: &str, v: i32| {
            e.fields[EntityState::field_index(P, name).unwrap()] = v;
        };
        for &(name, v) in fields {
            put(name, v);
        }
        put("pos.trType", pos.tr_type);
        put("pos.trTime", pos.tr_time);
        put("pos.trDuration", pos.tr_duration);
        for i in 0..3 {
            put(&format!("pos.trBase[{i}]"), pos.base[i].to_bits() as i32);
            put(&format!("pos.trDelta[{i}]"), pos.delta[i].to_bits() as i32);
        }
        (num, e)
    }

    /// `movez(48, 2)` from z 0 at 1000.
    fn rising() -> Trajectory {
        Trajectory {
            tr_type: TR_LINEAR_STOP,
            tr_time: 1000,
            tr_duration: 2000,
            base: Vec3::ZERO,
            delta: Vec3::new(0.0, 0.0, 24.0),
        }
    }

    #[test]
    fn a_brush_model_is_clipped_unless_flagged_nonsolid() {
        let ents: std::collections::BTreeMap<u32, EntityState> = [
            ent(
                177,
                &[("eType", 8), ("solid", SOLID_BMODEL), ("index", 5)],
                rising(),
            ),
            ent(
                178,
                &[
                    ("eType", 8),
                    ("solid", SOLID_BMODEL),
                    ("index", 6),
                    ("eFlags", 2),
                ],
                rising(),
            ),
            ent(251, &[("eType", 8), ("index", 56)], Trajectory::default()),
        ]
        .into_iter()
        .collect();
        let m = SnapshotMovers::from_entities(P, &ents);
        assert_eq!(
            m.clipped.iter().map(|c| (c.0, c.1)).collect::<Vec<_>>(),
            [(177, 5)]
        );
        assert_eq!(
            m.carriers.iter().map(|c| c.0).collect::<Vec<_>>(),
            [177, 178, 251]
        );
    }

    /// UO vehicles sharing a collmap each clip their own copy where they
    /// stand and name themselves on a hit (coduo-vehicles.md 10.3); with
    /// one posed model the second tank took the first one's brushes away.
    #[test]
    fn uo_vehicles_clip_their_own_copies_of_one_collmap() {
        use crate::collision::{Pusher, submodel_test_world};
        use crate::net::flags::ET_VEHICLE_V22;
        use crate::net::protocol::PROTOCOL_V22;
        let p = &PROTOCOL_V22;
        let world = submodel_test_world(
            "{\n\"classname\" \"script_vehicle_collmap\"\n\"model\" \"*1\"\n}",
            &[([-32.0, -32.0, 0.0], [32.0, 32.0, 48.0])],
        );
        let tank = |num: u32, x: f32| {
            let mut e = EntityState::null(p);
            e.number = num;
            let mut put = |name: &str, v: i32| {
                e.fields[EntityState::field_index(p, name).unwrap()] = v;
            };
            put("eType", ET_VEHICLE_V22);
            put("solid", SOLID_BMODEL);
            put("index", 1);
            put("pos.trBase[0]", x.to_bits() as i32);
            (num, e)
        };
        let ents: std::collections::BTreeMap<u32, EntityState> =
            [tank(186, 0.0), tank(187, 200.0)].into_iter().collect();
        SnapshotMovers::from_entities(p, &ents).place(&world, 1000);
        let down = |x: f32, skip: Option<u32>| {
            let t = world.point_trace_except(
                Vec3::new(x, 0.0, 100.0),
                Vec3::new(x, 0.0, -8.0),
                0x211,
                false,
                skip.map(Pusher::Copy),
            );
            (t.endpos.z.round(), world.entity_num(&t))
        };
        assert_eq!(down(0.0, None), (48.0, 186));
        assert_eq!(down(200.0, None), (48.0, 187));
        assert_eq!(down(200.0, Some(187)).0, 0.0, "its own copy passed");
    }

    /// A brush model whose index is 0 clips as a copy of the world where
    /// its trajectory has it at the prediction time, under any mask and
    /// named by its entity, and the next placement without it takes it out.
    #[test]
    fn index_zero_places_a_world_copy() {
        use crate::collision::submodel_test_world;
        use crate::net::protocol::ENTITYNUM_WORLD;
        let world = submodel_test_world("", &[]);
        let ents: std::collections::BTreeMap<u32, EntityState> = [ent(
            171,
            &[("eType", 8), ("solid", SOLID_BMODEL), ("index", 0)],
            Trajectory {
                tr_type: TR_LINEAR_STOP,
                tr_time: 1000,
                tr_duration: 2000,
                base: Vec3::new(0.0, 0.0, -64.0),
                delta: Vec3::new(0.0, 0.0, 48.0),
            },
        )]
        .into_iter()
        .collect();
        let down = || {
            let t = world.box_trace(
                Vec3::new(0.0, 0.0, 200.0),
                Vec3::new(0.0, 0.0, -100.0),
                Vec3::ZERO,
                Vec3::ZERO,
            );
            (t.endpos.z.round(), world.entity_num(&t))
        };
        let m = SnapshotMovers::from_entities(P, &ents);
        m.place(&world, 3000);
        assert_eq!(down(), (32.0, 171));
        SnapshotMovers::default().place(&world, 3000);
        assert_eq!(down(), (0.0, ENTITYNUM_WORLD));
    }

    /// The carry is the ground mover's travel between the two times, and
    /// nothing for the world, an unknown entity or a stationary one.
    #[test]
    fn the_carry_follows_the_ground_movers_trajectory() {
        let still = Trajectory {
            tr_type: TR_STATIONARY,
            base: Vec3::new(0.0, 0.0, 48.0),
            ..Trajectory::default()
        };
        let ents: std::collections::BTreeMap<u32, EntityState> = [
            ent(
                177,
                &[("eType", 8), ("solid", SOLID_BMODEL), ("index", 5)],
                rising(),
            ),
            ent(
                178,
                &[("eType", 8), ("solid", SOLID_BMODEL), ("index", 6)],
                still,
            ),
            ent(3, &[("eType", 1)], rising()),
        ]
        .into_iter()
        .collect();
        let m = SnapshotMovers::from_entities(P, &ents);
        let o = Vec3::new(-215.0, 2463.0, -20.675);
        let up = m.carry(o, 177, 1050, 1100);
        assert!((up.z - (o.z + 1.2)).abs() < 1e-4, "{up}");
        assert_eq!(m.carry(o, 178, 1050, 1100), o);
        assert_eq!(m.carry(o, 3, 1050, 1100), o, "a player is no carrier");
        assert_eq!(m.carry(o, 1022, 1050, 1100), o);
        assert_eq!(m.carry(o, 99, 1050, 1100), o);
    }
}
