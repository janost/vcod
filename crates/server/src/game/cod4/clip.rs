//! What a CoD4 1.7 level's pmove and script traces clip: the map's
//! `vcod_cod4::collision::ClipMap`, then the other clients' bodies and the
//! linked brush entities, as `SV_Trace` walks the world then the entities
//! (docs/research/cod4-pmove.md 15).

use std::rc::Rc;
use std::sync::Arc;

use glam::Vec3;
use vcod_cod4::collision::ClipMap;
use vcod_cod4::pmove::{self, PmWorld, Tr};
use vcod_common::turretpose::angles_quat;

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

/// A brush entity linked with inline model `model`, where it stands.
#[derive(Clone, Copy, Debug)]
pub struct Brush4 {
    pub entity: u32,
    pub model: usize,
    pub origin: Vec3,
    pub angles: Vec3,
}

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
        {
            let clips =
                |b: &&Body4| b.entity != pass && b.owner != Some(pass) && mask & b.contents != 0;
            for b in self.bodies.iter().filter(clips) {
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
                if c.fraction < t.fraction || (c.startsolid && !t.startsolid) {
                    t = Tr {
                        fraction: c.fraction,
                        normal: c.normal,
                        surface_flags: 0,
                        contents: b.contents,
                        entity_num: b.entity,
                        startsolid: c.startsolid,
                        allsolid: c.allsolid,
                        walkable: c.normal.z >= vcod_cod4::collision::WALKABLE_NORMAL_Z,
                    };
                }
                if t.fraction == 0.0 {
                    return t;
                }
            }
        }
        for b in self.brushes.iter().filter(|b| b.entity != pass) {
            if b.model == 0 || b.model >= self.clip.submodels.len() {
                continue;
            }
            // `CM_TransformedBoxTrace`: in the model's frame.
            let q = angles_quat(b.angles.to_array());
            let local = |p: Vec3| q.inverse() * (p - b.origin);
            let c = self
                .clip
                .trace_model(b.model, local(start), local(end), mins, maxs, mask);
            if c.fraction < t.fraction || (c.startsolid && !t.startsolid) {
                let mut w = pmove::world_tr(&c);
                w.normal = q * c.normal;
                w.entity_num = b.entity;
                t = w;
            }
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
