//! The map side of the server: collision for spectator flight and a spawn.

use std::collections::{BTreeMap, HashMap};
use vcod_common::mapworld;
use vcod_common::net::msg::EntityState;
use vcod_common::net::protocol::{ENTITYNUM_WORLD, Protocol};
use vcod_common::net::trajectory::{TR_LINEAR, Trajectory};
use vcod_common::{bsp, collision::CollisionWorld};

pub struct World {
    pub collision: CollisionWorld,
    /// The BSP tree and PVS table, for culling each client's entity list.
    pub vis: mapworld::Visibility,
    /// Feet origin and yaw degrees, from the ents lump's spawn classes.
    pub spawn: ([f32; 3], f32),
    /// Every gametype's spawn points, what the bots' navigation graph is
    /// flooded from (`crate::nav`).
    pub spawn_points: Vec<[f32; 3]>,
    /// The brushes of the triggers that kill whoever walks in: minefields
    /// and `trigger_hurt`. The navigation graph keeps out of them.
    pub hazards: Vec<crate::game::trigger::BrushHull>,
    /// The submodels of the brush entities marked `script_noteworthy
    /// rubble`: United Offensive's `bas.gsc` raises each 1000 units at the
    /// start (`BaseAssault_base_think`) and drops it back only over a
    /// destroyed base, and every other gametype deletes it, so the
    /// navigation graph is built without them.
    pub rubble: Vec<usize>,
    /// Where the navigation graph has to reach besides the spawns' box:
    /// every `script_gameobjectname bas` entity, its bases, charges and
    /// launchers. mp_italy's western axis bunker lay past the box's margin,
    /// its charge room off the graph (bot-objectives.md 7.2).
    pub nav_cover: Vec<[f32; 3]>,
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
/// The two areas must connect too, and an eye in a solid leaf is sent
/// nothing ([`Viewer::sees_nothing`]).
pub fn visible_entities(
    vis: &mapworld::Visibility,
    eye: [f32; 3],
    entities: &BTreeMap<u32, EntityState>,
    time: i32,
    p: &Protocol,
) -> BTreeMap<u32, EntityState> {
    let v = Viewer::at(vis, eye);
    if v.sees_nothing() {
        return BTreeMap::new();
    }
    entities
        .iter()
        .filter(|(_, e)| entity_seen(vis, &v, e, time, p, None))
        .map(|(n, e)| (*n, e.clone()))
        .collect()
}

/// Where a snapshot is built from: the eye, and the cluster and area of
/// its leaf.
#[derive(Clone, Copy, Debug)]
pub struct Viewer {
    pub eye: [f32; 3],
    pub cluster: i32,
    pub area: i32,
}

impl Viewer {
    pub fn at(vis: &mapworld::Visibility, eye: [f32; 3]) -> Self {
        Viewer {
            eye,
            cluster: vis.cluster_at(eye),
            area: vis.area_at(eye),
        }
    }

    /// `SV_AddEntitiesVisibleFromPoint` returns before its loop when the
    /// eye's leaf has area -1 (cod_lnxded 0x808e298, 1.5b 0x8095c30,
    /// coduo_lnxded 0x8096b97): an eye in a solid leaf is sent no entity,
    /// broadcast and temp entities included.
    pub fn sees_nothing(&self) -> bool {
        self.area < 0
    }
}

/// [`visible_entities`]' cluster test for one entity, from the cluster
/// `from`, in the frame whose level time is `time`.
pub fn entity_visible(
    vis: &mapworld::Visibility,
    from: i32,
    e: &EntityState,
    time: i32,
    p: &Protocol,
) -> bool {
    vis.link_visible(from, &entity_link(vis, e, time, p).0)
}

/// The snapshot builder's whole test for an entity the viewer is not sent
/// unconditionally: the areas, the clusters, then on 1.5 and UO the fog.
/// `fog` is [`fog_cull`]'s squared distance.
pub fn entity_seen(
    vis: &mapworld::Visibility,
    v: &Viewer,
    e: &EntityState,
    time: i32,
    p: &Protocol,
    fog: Option<f32>,
) -> bool {
    let (link, lo, hi) = entity_link(vis, e, time, p);
    link.in_area(v.area)
        && vis.link_visible(v.cluster, &link)
        && fog.is_none_or(|d| box_dist_sq(lo, hi, v.eye) <= d)
}

/// What 1.5's and United Offensive's snapshot builders cull past
/// (`vmMain` 0x17, `level.fogOpaqueDistSqrd`, game.mp.uo 0x56ac0 and
/// game.mp 1.5 0x4d888): `G_setfog` reads the fog configstring with
/// `"%f %f %f %f %f %f %f"` and, given all seven and a density of at least
/// 1 (`setCullFog`'s), keeps `((far - near) + near)` squared. `None` for
/// exponential fog, no fog or anything else, which the engine reads as
/// no cull (coduo_lnxded 0x8096bfe). 1.1's engine never asks.
pub fn fog_cull(cs: &str) -> Option<f32> {
    let v: Vec<f32> = cs
        .split_whitespace()
        .map_while(|w| w.parse().ok())
        .take(7)
        .collect();
    if v.len() != 7 || v[2] < 1.0 {
        return None;
    }
    let d = (v[1] - v[0]) + v[0];
    Some(d * d).filter(|&d| d != 0.0)
}

/// The squared distance from `p` to the box, counting only the axes on
/// which the box lies wholly to one side (coduo_lnxded 0x806ab7e).
fn box_dist_sq(lo: [f32; 3], hi: [f32; 3], p: [f32; 3]) -> f32 {
    (0..3)
        .map(|i| {
            let (a, b) = (lo[i] - p[i], hi[i] - p[i]);
            if a * b > 0.0 { (a * a).min(b * b) } else { 0.0 }
        })
        .sum()
}

/// `SV_inSnapshot`, what the game's `trap_InSnapshot` asks of an entity
/// from a point (cod_lnxded 0x8087b90, 1.5b 0x808d5d2, coduo_lnxded
/// 0x808e3a9; docs/research/cod11-hud-protocol.md 9, "Compass
/// friendlies"). `broadcast` is the entity's `SVF_BROADCAST` (1.1) or a
/// `r.broadcastTime` no snapshot has cleared yet (1.5, UO), either of which
/// reads as seen. 1.1 then counts an entity touching no cluster as seen;
/// 1.5 and UO test the areas first and count it as not seen, and add the
/// fog.
pub fn in_snapshot(
    vis: &mapworld::Visibility,
    eye: [f32; 3],
    e: &EntityState,
    broadcast: bool,
    time: i32,
    p: &Protocol,
    fog: Option<f32>,
) -> bool {
    broadcast || link_in_snapshot(vis, eye, &entity_link(vis, e, time, p), p.is_v1(), fog)
}

/// [`in_snapshot`]'s test past the broadcast flag, on a link the caller
/// placed ([`link_at`]). `v1` is 1.1's rule for an entity touching no
/// cluster.
pub fn link_in_snapshot(
    vis: &mapworld::Visibility,
    eye: [f32; 3],
    (link, lo, hi): &(mapworld::LinkClusters, [f32; 3], [f32; 3]),
    v1: bool,
    fog: Option<f32>,
) -> bool {
    let (lo, hi) = (*lo, *hi);
    if v1 && link.clusters.is_empty() {
        return true;
    }
    let v = Viewer::at(vis, eye);
    link.in_area(v.area)
        && !link.clusters.is_empty()
        && vis.link_visible(v.cluster, link)
        && fog.is_none_or(|d| box_dist_sq(lo, hi, eye) <= d)
}

/// The clusters an entity's link box leaves it with, and that box
/// (`r.absmin`, `r.absmax`).
fn entity_link(
    vis: &mapworld::Visibility,
    e: &EntityState,
    time: i32,
    p: &Protocol,
) -> (mapworld::LinkClusters, [f32; 3], [f32; 3]) {
    let (o, angles) = link_pose(e, time, p);
    let bmodel = (e.field_i32(p, "solid") == vcod_common::net::flags::SOLID_BMODEL)
        .then(|| e.field_i32(p, "index") as usize);
    let own = crate::game::wire::link_box(e.field_i32(p, "eType"));
    link_at(vis, o, angles, bmodel, own)
}

/// The clusters an entity linked at `o` turned by `angles` leaves, and its
/// box (`r.absmin`, `r.absmax`): a brush model's (`bmodel`, its inline
/// model number) or else `own`, about the origin.
pub fn link_at(
    vis: &mapworld::Visibility,
    o: [f32; 3],
    angles: [f32; 3],
    bmodel: Option<usize>,
    own: ([f32; 3], [f32; 3]),
) -> (mapworld::LinkClusters, [f32; 3], [f32; 3]) {
    let (mins, maxs) = link_bounds(vis, bmodel, angles, own);
    let at = |b: [f32; 3], pad: f32| [o[0] + b[0] + pad, o[1] + b[1] + pad, o[2] + b[2] + pad];
    let (lo, hi) = (at(mins, -LINK_EPSILON), at(maxs, LINK_EPSILON));
    (vis.link_clusters(lo, hi), lo, hi)
}

/// Where an entity was last linked, origin and angles. A mover is relinked
/// each frame it moves at its trajectories evaluated at the level time
/// (`G_MoverTeam` and the push, docs/research/cod11-movers.md sections 11
/// and 14); anything else at its `trBase`.
fn link_pose(e: &EntityState, time: i32, p: &Protocol) -> ([f32; 3], [f32; 3]) {
    use vcod_common::net::flags::{ET_MOVER, ET_SCRIPTMOVER};
    if matches!(e.field_i32(p, "eType"), ET_MOVER | ET_SCRIPTMOVER) {
        let at = |group| Trajectory::read(e, p, group).evaluate(time).to_array();
        return (at("pos"), at("apos"));
    }
    (e.origin(p), e.angles(p))
}

/// The box about the origin an entity links with. A brush model's is its
/// inline model's bounds, or with any angle set a cube of the radius of those
/// bounds: VERIFIED, `SV_LinkEntity`'s `r.bmodel` arm (cod_lnxded 0x80908b0)
/// takes `RadiusFromBounds` when `r.currentAngles` is not zero.
fn link_bounds(
    vis: &mapworld::Visibility,
    bmodel: Option<usize>,
    angles: [f32; 3],
    own: ([f32; 3], [f32; 3]),
) -> ([f32; 3], [f32; 3]) {
    let Some((mins, maxs)) = bmodel.and_then(|i| vis.model_bounds(i)) else {
        return own;
    };
    if angles == [0.0; 3] {
        return (mins, maxs);
    }
    let r = (0..3)
        .map(|i| mins[i].abs().max(maxs[i].abs()).powi(2))
        .sum::<f32>()
        .sqrt();
    ([-r; 3], [r; 3])
}

impl World {
    /// `fs` is the mounted paks, for the static props: a map's carts, crates
    /// and hay bales are xmodels rather than brushes, and retail collides
    /// against them. Without them a grenade rolls through a cart and a
    /// bullet passes through a barrel (`crates/server/tests/missile_ab.rs`,
    /// where the capture's own grenade comes to rest on one). `None` is the
    /// bare world every test that mounts no paks builds.
    pub fn from_bsp(b: &mapworld::MapWorld, fs: Option<&vcod_common::pk3::Pk3Fs>) -> Self {
        let props = fs.map_or_else(Vec::new, |fs| {
            vcod_common::props::collision_tris(fs, &b.entities)
        });
        let collision = CollisionWorld::build(b, &props);
        let spawn = bsp::find_spawn(&b.entities)
            // No spawn class in the ents: hover above the origin.
            .unwrap_or(([0.0, 0.0, 64.0], 0.0));
        World {
            collision,
            vis: b.visibility(),
            spawn,
            spawn_points: crate::nav::spawn_points(&b.entities),
            hazards: hazards(b),
            rubble: bsp::entity_blocks(&b.entities)
                .into_iter()
                .filter(|e| e.get("script_noteworthy").is_some_and(|n| n == "rubble"))
                .filter_map(|e| e.get("model")?.strip_prefix('*')?.parse().ok())
                .collect(),
            nav_cover: bsp::entity_blocks(&b.entities)
                .into_iter()
                .filter(|e| {
                    e.get("script_gameobjectname")
                        .is_some_and(|n| n.split_whitespace().any(|n| n == "bas"))
                })
                .filter_map(|e| bsp::parse_vec3(e.get("origin")?))
                .collect(),
        }
    }
}

/// The brushes of every `trigger_hurt` and every `trigger_multiple` named
/// `minefield` (`maps/mp/_minefields.gsc` kills whoever stays in one).
fn hazards(b: &mapworld::MapWorld) -> Vec<crate::game::trigger::BrushHull> {
    let mut hulls = crate::game::trigger::model_brush_hulls(b);
    let mut out = Vec::new();
    for e in bsp::entity_blocks(&b.entities) {
        let class = e.get("classname").map(String::as_str);
        let minefield = class == Some("trigger_multiple")
            && e.get("targetname").is_some_and(|t| t == "minefield");
        if !(minefield || class == Some("trigger_hurt")) {
            continue;
        }
        let model = e
            .get("model")
            .and_then(|m| m.strip_prefix('*')?.parse::<usize>().ok());
        if let Some(m) = model.filter(|&m| m < hulls.len()) {
            out.append(&mut hulls[m]);
        }
    }
    out
}

/// Scripted entities that exist only to drive the packet-entity wire path:
/// baselines, deltas, removals and trajectory evaluation. No gameplay meaning.
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
        let w = World::from_bsp(&parsed, None);
        // find_spawn picked a real spot, not bedrock.
        assert!(w.spawn.0[2] > -500.0, "spawn {:?}", w.spawn);
    }

    /// A moving brush model is culled where its trajectory has it at the
    /// frame's time, not at its `trBase`: one sliding from a spot the eye
    /// cannot see to one it can is sent once it gets there.
    #[test]
    fn a_moving_brush_model_is_culled_where_it_is_now() {
        let Some(data) = vcod_common::testing::real_bsp() else {
            return;
        };
        let parsed = bsp::parse(&data).unwrap();
        let w = World::from_bsp(&parsed, None);
        let p = &vcod_common::net::protocol::PROTOCOL_V1;
        // The smallest submodel, so a translation can take it out of view.
        let extent = |n: usize| {
            w.vis
                .model_bounds(n)
                .map(|(lo, hi)| (0..3).map(|i| hi[i] - lo[i]).sum::<f32>())
        };
        let model = (1..parsed.models.len())
            .filter_map(|n| Some((n, extent(n)?)))
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .expect("a submodel")
            .0;
        let mut e = EntityState::null(p);
        let put = |e: &mut EntityState, name: &str, v: i32| {
            e.fields[EntityState::field_index(p, name).unwrap()] = v;
        };
        put(&mut e, "eType", vcod_common::net::flags::ET_SCRIPTMOVER);
        put(&mut e, "solid", vcod_common::net::flags::SOLID_BMODEL);
        put(&mut e, "index", model as i32);
        let seen_at = |from: i32, off: [f32; 3]| {
            let mut e = e.clone();
            for (axis, v) in off.iter().enumerate() {
                put(&mut e, &format!("pos.trBase[{axis}]"), v.to_bits() as i32);
            }
            entity_visible(&w.vis, from, &e, 0, p)
        };
        // An eye and two translations of the model, one it sees and one not.
        let grid: Vec<[f32; 3]> = (-8..=8)
            .flat_map(|x| (-8..=8).map(move |y| [x as f32 * 256.0, y as f32 * 256.0, 0.0]))
            .collect();
        let (from, seen, unseen) = w
            .spawn_points
            .iter()
            .find_map(|&eye| {
                let from = w.vis.cluster_at(eye);
                let seen = grid.iter().find(|&&o| seen_at(from, o))?;
                let unseen = grid.iter().find(|&&o| !seen_at(from, o))?;
                Some((from, *seen, *unseen))
            })
            .expect("a spawn that sees the model from one spot and not another");
        for (axis, v) in unseen.iter().enumerate() {
            put(&mut e, &format!("pos.trBase[{axis}]"), v.to_bits() as i32);
            put(
                &mut e,
                &format!("pos.trDelta[{axis}]"),
                (seen[axis] - v).to_bits() as i32,
            );
        }
        put(
            &mut e,
            "pos.trType",
            vcod_common::net::trajectory::TR_LINEAR_STOP,
        );
        put(&mut e, "pos.trTime", 1000);
        put(&mut e, "pos.trDuration", 1000);
        assert!(!entity_visible(&w.vis, from, &e, 1000, p), "at the start");
        assert!(entity_visible(&w.vis, from, &e, 2000, p), "at the end");
    }

    /// `G_setfog`'s cull distance: linear fog's far distance squared,
    /// nothing for exponential fog or a short string.
    #[test]
    fn the_fog_cull_is_the_linear_far_distance_squared() {
        assert_eq!(
            fog_cull("500 15000 1 0.0273 0.082 0.1601 0"),
            Some(15000.0 * 15000.0)
        );
        assert_eq!(fog_cull("0 1 0.0002 0.5 0.5 0.5 0"), None);
        assert_eq!(fog_cull("0"), None);
        assert_eq!(fog_cull(""), None);
    }

    /// The fog test's distance counts an axis only where the box lies wholly
    /// to one side of the eye.
    #[test]
    fn the_fog_distance_skips_the_axes_the_box_straddles() {
        let (lo, hi) = ([10.0, -5.0, -5.0], [20.0, 5.0, 5.0]);
        assert_eq!(box_dist_sq(lo, hi, [0.0; 3]), 100.0);
        assert_eq!(box_dist_sq(lo, hi, [15.0, 0.0, 0.0]), 0.0);
        assert_eq!(box_dist_sq(lo, hi, [0.0, 8.0, -9.0]), 100.0 + 9.0 + 16.0);
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
