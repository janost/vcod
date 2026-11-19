//! What a CoD4 1.7 level's pmove and script traces clip: the map's
//! `vcod_cod4::collision::ClipMap`, then the other clients' bodies and the
//! linked brush entities, as `SV_Trace` walks the world then the entities
//! (docs/research/cod4-pmove.md 15).

use std::rc::Rc;
use std::sync::Arc;

use glam::Vec3;
use vcod_cod4::collision::ClipMap;
use vcod_cod4::pmove::{self, PmWorld, Tr};

/// A client's body as `SV_LinkEntity` links it: feet origin and the box
/// its last move left (`r.mins`, `r.maxs`); or a mantle's blockage, a box
/// of `CONTENTS_PLAYERCLIP` its owner passes (`SV_Trace` skips an entity
/// whose `r.ownerNum` is the pass entity).
#[derive(Clone, Copy, Debug)]
pub struct Body4 {
    pub entity: u32,
    pub origin: Vec3,
    pub mins: Vec3,
    pub maxs: Vec3,
    pub contents: u32,
    pub owner: Option<u32>,
}

/// A brush entity linked with an inline model, where it stands.
pub type Brush4 = pmove::Brush;

/// One trace's world: the clip map, then each body (a temp box model of
/// `CONTENTS_BODY`) and brush entity but `pass` that cuts it shorter.
pub struct Cod4Clip<'a> {
    pub clip: &'a ClipMap,
    pub bodies: &'a [Body4],
    pub brushes: &'a [Brush4],
}

impl PmWorld for Cod4Clip<'_> {
    fn trace(&self, start: Vec3, end: Vec3, mins: Vec3, maxs: Vec3, pass: u32, mask: u32) -> Tr {
        let mut t = pmove::world_tr(&self.clip.trace(start, end, mins, maxs, mask));
        if t.fraction == 0.0 {
            return t;
        }
        let clips =
            |b: &&Body4| b.entity != pass && b.owner != Some(pass) && mask & b.contents != 0;
        for b in self.bodies.iter().filter(clips) {
            // A client is a capsule; the mantle blocks stay boxes.
            if b.entity < pmove::MAX_CLIENTS as u32 {
                let body = vcod_common::movetrace::Body {
                    entity: b.entity,
                    origin: b.origin,
                    mins: b.mins,
                    maxs: b.maxs,
                    contents: b.contents,
                };
                t.fold_capsule(start, end, mins, maxs, &body);
            } else {
                let mut c = vcod_common::collision::Trace::clear(end);
                c.fraction = t.fraction;
                vcod_common::collision::clip_entity_box(
                    &mut c,
                    start,
                    end,
                    mins,
                    maxs,
                    b.origin + b.mins,
                    b.origin + b.maxs,
                    b.contents,
                    b.entity,
                );
                t.fold_body(&c, b.contents, b.entity);
            }
            if t.fraction == 0.0 {
                return t;
            }
        }
        for b in self.brushes.iter().filter(|b| b.entity != pass) {
            if b.model == 0 || b.model >= self.clip.submodels.len() {
                continue;
            }
            t.fold_brush(self.clip, b, start, end, mins, maxs, mask);
            if t.fraction == 0.0 {
                break;
            }
        }
        t
    }
}

/// The engine's `CollisionWorld` over a CoD4 clip map, what the shared
/// script traces (`placeSpawnPoint`, `bulletTrace` and kin) read: no BSP of
/// its own, every query through the clip map
/// (`CollisionWorld::set_world_clip`).
pub fn level_world(clip: Arc<ClipMap>) -> Rc<crate::world::World> {
    let map = vcod_common::mapworld::MapWorld::default();
    let mut collision = vcod_common::collision::CollisionWorld::build(&map, &[]);
    collision.set_world_clip(clip);
    Rc::new(crate::world::World {
        collision,
        vis: map.visibility(),
        spawn: ([0.0; 3], 0.0),
        spawn_points: Vec::new(),
        hazards: Vec::new(),
        rubble: Vec::new(),
        nav_cover: Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use vcod_cod4::collision::{CONTENTS_SOLID, MASK_PLAYERSOLID, Submodel};
    use vcod_common::clipmap::{self, BrushNode, CBrush, CLeaf, CNode, CPlane};

    fn leaf(b: &CBrush, brush_node: u32) -> CLeaf {
        CLeaf {
            first_aabb: 0,
            aabb_count: 0,
            brush_contents: CONTENTS_SOLID,
            terrain_contents: 0,
            mins: b.mins - Vec3::splat(0.125),
            maxs: b.maxs + Vec3::splat(0.125),
            brush_node,
        }
    }

    fn brush(mins: Vec3, maxs: Vec3) -> CBrush {
        CBrush {
            mins,
            maxs,
            contents: CONTENTS_SOLID,
            sides: Vec::new(),
            axial_material: [[0; 3]; 2],
        }
    }

    /// A world wall at x 64..96 and submodel 1, a block round the origin.
    fn wall_and_block() -> ClipMap {
        let wall = brush(
            Vec3::new(64.0, -100.0, -100.0),
            Vec3::new(96.0, 100.0, 100.0),
        );
        let block = brush(Vec3::splat(-32.0), Vec3::splat(32.0));
        let (world_leaf, block_leaf) = (leaf(&wall, 1), leaf(&block, 2));
        let world = clipmap::ClipMap {
            planes: vec![CPlane {
                normal: Vec3::Z,
                dist: -1000.0,
                kind: 2,
                signbits: 0,
            }],
            nodes: vec![CNode {
                plane: 0,
                children: [-1, -1],
            }],
            leafs: vec![world_leaf],
            brush_nodes: vec![
                BrushNode::Leaf {
                    first: 0,
                    count: 0,
                    contents: 0,
                },
                BrushNode::Leaf {
                    first: 0,
                    count: 1,
                    contents: CONTENTS_SOLID,
                },
                BrushNode::Leaf {
                    first: 1,
                    count: 1,
                    contents: CONTENTS_SOLID,
                },
            ],
            leaf_brush_list: vec![0, 1],
            brushes: vec![wall, block],
            materials: vec![(0, CONTENTS_SOLID)],
            verts: Vec::new(),
            edges: Vec::new(),
            tris: Vec::new(),
            borders: Vec::new(),
            partitions: Vec::new(),
            aabbs: Vec::new(),
            indexed: None,
            reach: Default::default(),
        };
        let sub = |leaf| Submodel {
            mins: Vec3::splat(-32.0),
            maxs: Vec3::splat(32.0),
            radius: 56.0,
            leaf,
        };
        ClipMap {
            name: "wall_and_block".into(),
            checksum: 0,
            world,
            submodels: vec![sub(world_leaf), sub(block_leaf)],
            static_models: Vec::new(),
        }
    }

    /// A sweep that starts inside a brush entity and leaves it still stops
    /// on the world wall behind it, and keeps the start in solid.
    #[test]
    fn leaving_a_brush_entity_still_stops_on_the_wall_behind_it() {
        let clip = wall_and_block();
        let brushes = [Brush4 {
            entity: 70,
            model: 1,
            origin: Vec3::ZERO,
            angles: Vec3::ZERO,
        }];
        let world = Cod4Clip {
            clip: &clip,
            bodies: &[],
            brushes: &brushes,
        };
        let (start, end) = (Vec3::ZERO, Vec3::new(128.0, 0.0, 0.0));
        let alone =
            pmove::world_tr(&clip.trace(start, end, Vec3::ZERO, Vec3::ZERO, MASK_PLAYERSOLID));
        assert!(alone.fraction < 0.5 && !alone.startsolid, "{alone:?}");
        let t = world.trace(start, end, Vec3::ZERO, Vec3::ZERO, 1023, MASK_PLAYERSOLID);
        assert_eq!(t.fraction, alone.fraction, "{t:?}");
        assert_eq!(t.entity_num, alone.entity_num);
        assert!(t.startsolid);
    }
}
