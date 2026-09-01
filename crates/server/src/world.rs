//! The map side of the server: collision for spectator flight and a spawn.

use std::collections::{BTreeMap, HashMap};
use vcod_common::net::msg::EntityState;
use vcod_common::net::protocol::{Protocol, ENTITYNUM_WORLD};
use vcod_common::net::trajectory::TR_LINEAR;
use vcod_common::{bsp, collision::CollisionWorld};

pub struct World {
    pub collision: CollisionWorld,
    /// The BSP tree and PVS table, for culling each client's entity list.
    pub vis: bsp::Visibility,
    /// Feet origin and yaw degrees, from the ents lump's spawn classes.
    pub spawn: ([f32; 3], f32),
}

/// The unit the engine grows a linked entity's box by on each axis before
/// taking its clusters. VERIFIED from `SV_LinkEntity` (cod_lnxded 0x8090bbe):
/// after `absmin`/`absmax` are the origin plus `r.mins`/`r.maxs`, each low
/// component has 1 subtracted and each high component 1 added. It is what
/// gives a script model, whose own box is a point, any clusters at all.
const LINK_EPSILON: f32 = 1.0;

/// Which of `entities` a client standing at `eye` is sent. Retail's rule is
/// the entity's clusters against the client's PVS row
/// (`docs/protocol-1.1.md`, "Which entities a client is sent"), collected from
/// the box its spawn function links it with, and this does the same. A point
/// lookup is not enough: a turret's origin sits inside the geometry it stands
/// on and reads as a solid leaf, so it would be visible from everywhere.
///
/// Retail also checks that the two areas connect. Every stock map's leafs
/// carry area -1 or 0 (`docs/research/bsp-ibsp59-format.md`), so there is
/// nothing to disconnect and no area test here.
pub fn visible_entities(
    vis: &bsp::Visibility,
    eye: [f32; 3],
    entities: &BTreeMap<u32, EntityState>,
    p: &Protocol,
) -> BTreeMap<u32, EntityState> {
    let from = vis.cluster_at(eye);
    entities
        .iter()
        .filter(|(_, e)| {
            let o = e.origin(p);
            let (mins, maxs) = crate::game::wire::link_box(e.field_i32(p, "eType"));
            let at =
                |b: [f32; 3], pad: f32| [o[0] + b[0] + pad, o[1] + b[1] + pad, o[2] + b[2] + pad];
            let clusters = vis.clusters_in_box(at(mins, -LINK_EPSILON), at(maxs, LINK_EPSILON));
            // An entity whose box touches no cluster at all is skipped, the
            // way the module's loop skips one with `numClusters == 0`.
            clusters.iter().any(|&c| vis.visible(from, c))
        })
        .map(|(n, e)| (*n, e.clone()))
        .collect()
}

impl World {
    pub fn from_bsp(b: &bsp::Bsp) -> Self {
        let collision = CollisionWorld::build(b, &[]);
        let spawn = bsp::find_spawn(&b.entities)
            // No spawn class in the ents: hover above the origin.
            .unwrap_or(([0.0, 0.0, 64.0], 0.0));
        World {
            collision,
            vis: b.visibility(),
            spawn,
        }
    }
}

/// Scripted entities that exist only to drive the packet-entity wire path:
/// baselines, deltas, removals and trajectory evaluation. No gameplay meaning
/// (docs/design/2026-08-27-delta-compression-design.md).
pub struct TestEntities {
    count: usize,
    around: [f32; 3],
}

/// Entity numbers start at 32, clear of client slots for the default
/// `--max-clients` (8) and any run up to 32; CoD 1.1 allows up to 64
/// clients, so a server started with more than 32 collides a live slot
/// with a test entity number.
const FIRST_TEST_ENT: u32 = 32;
/// The last entity vanishes for 2 s out of every 8 s.
const CYCLE_MS: i32 = 8000;
const GONE_MS: i32 = 2000;
/// The highest `--test-entities` count that keeps every scripted entity
/// number below `ENTITYNUM_WORLD`; past this, the highest-numbered entity
/// collides with the reserved world slot or the packet-entities terminator.
pub const MAX_TEST_ENTITIES: usize = (ENTITYNUM_WORLD - FIRST_TEST_ENT) as usize;

impl TestEntities {
    pub fn new(count: usize, around: [f32; 3]) -> Self {
        TestEntities { count, around }
    }

    fn ent(&self, p: &Protocol, i: usize, server_time: i32) -> EntityState {
        let mut e = EntityState::null(p);
        // `number` takes part in EntityState equality; stamp it so a built
        // entity compares equal to the same entity read back off the wire.
        e.number = FIRST_TEST_ENT + i as u32;
        let seti = |e: &mut EntityState, name: &str, v: i32| {
            if let Some(idx) = EntityState::field_index(p, name) {
                e.fields[idx] = v;
            }
        };
        // pos.trBase and pos.trDelta are `bits: 0` fields: the wire value is
        // an f32 bit pattern, not a raw integer (fields_v1.rs).
        let setf = |e: &mut EntityState, name: &str, v: f32| {
            if let Some(idx) = EntityState::field_index(p, name) {
                e.fields[idx] = v.to_bits() as i32;
            }
        };
        // Entities sit 64 units apart along x and drift along y at 100
        // units/s; the trajectory restarts every lap so pos.trTime changes
        // on the wire without pos.trBase itself needing to move.
        let lap = 4000;
        let phase = server_time.rem_euclid(lap);
        seti(&mut e, "pos.trType", TR_LINEAR);
        seti(&mut e, "pos.trTime", server_time - phase);
        setf(&mut e, "pos.trBase[0]", self.around[0] + i as f32 * 64.0);
        setf(&mut e, "pos.trBase[1]", self.around[1]);
        setf(&mut e, "pos.trBase[2]", self.around[2]);
        setf(&mut e, "pos.trDelta[1]", 100.0);
        e
    }

    pub fn baselines(&self, p: &Protocol) -> HashMap<u32, EntityState> {
        (0..self.count)
            .map(|i| (FIRST_TEST_ENT + i as u32, self.ent(p, i, 0)))
            .collect()
    }

    pub fn at(&self, p: &Protocol, server_time: i32) -> BTreeMap<u32, EntityState> {
        let cycled_out = server_time.rem_euclid(CYCLE_MS) >= CYCLE_MS - GONE_MS;
        let last = self.count.saturating_sub(1);
        (0..self.count)
            .filter(|i| !(cycled_out && *i == last))
            .map(|i| (FIRST_TEST_ENT + i as u32, self.ent(p, i, server_time)))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spawns_at_a_map_spot_on_real_maps() {
        let Some(data) = vcod_common::testing::real_bsp() else {
            return;
        };
        let parsed = bsp::parse(&data).unwrap();
        let w = World::from_bsp(&parsed);
        // find_spawn picked a real spot, not bedrock.
        assert!(w.spawn.0[2] > -500.0, "spawn {:?}", w.spawn);
    }

    #[test]
    fn test_entities_move_and_one_cycles_out_and_back() {
        let p = &vcod_common::net::protocol::PROTOCOL_V1;
        let te = TestEntities::new(3, [0.0, 0.0, 64.0]);
        let time_idx = EntityState::field_index(p, "pos.trTime").unwrap();

        let a = te.at(p, 0);
        let b = te.at(p, 4000);
        assert_eq!(a.len(), 3, "all three present at t=0");
        // Motion is carried by trTime (the trajectory's start), not trBase,
        // which is fixed per entity; the client evaluates the interpolation.
        assert_ne!(
            a[&32].fields[time_idx], b[&32].fields[time_idx],
            "entity 32's trajectory restarted between t=0 and t=4000"
        );

        // The cycling entity is gone for a stretch and comes back.
        // 6500 % 8000 == 6500, inside the last 2s (>= 6000) of the 8s cycle.
        let gone = te.at(p, 6500);
        assert_eq!(gone.len(), 2, "one entity cycles out at t=6500");
        let back = te.at(p, 11000);
        assert_eq!(back.len(), 3, "and returns by t=11000");

        // Every entity has a baseline for the gamestate.
        let bl = te.baselines(p);
        assert_eq!(bl.len(), 3);
        assert!(bl.contains_key(&32));
    }

    #[test]
    fn max_test_entities_is_the_last_count_below_entitynum_world() {
        let p = &vcod_common::net::protocol::PROTOCOL_V1;

        let at_max = TestEntities::new(MAX_TEST_ENTITIES, [0.0, 0.0, 64.0]);
        let highest = *at_max.baselines(p).keys().max().unwrap();
        assert_eq!(
            highest,
            ENTITYNUM_WORLD - 1,
            "MAX_TEST_ENTITIES should use every number up to ENTITYNUM_WORLD - 1"
        );

        let one_over = TestEntities::new(MAX_TEST_ENTITIES + 1, [0.0, 0.0, 64.0]);
        let highest_over = *one_over.baselines(p).keys().max().unwrap();
        assert_eq!(
            highest_over, ENTITYNUM_WORLD,
            "one past MAX_TEST_ENTITIES reaches ENTITYNUM_WORLD, which is what \
             the --test-entities startup check rejects"
        );
    }
}
