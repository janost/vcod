//! The map side of the server: collision for spectator flight and a spawn.

use vcod_common::{bsp, collision::CollisionWorld};

pub struct World {
    pub collision: CollisionWorld,
    /// Feet origin and yaw degrees, from the ents lump's spawn classes.
    pub spawn: ([f32; 3], f32),
}

impl World {
    pub fn from_bsp(b: &bsp::Bsp) -> Self {
        let collision = CollisionWorld::build(b, &[]);
        let spawn = bsp::find_spawn(&b.entities)
            // No spawn class in the ents: hover above the origin.
            .unwrap_or(([0.0, 0.0, 64.0], 0.0));
        World { collision, spawn }
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
}
