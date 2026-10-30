//! `G_MoverPush` and `G_TryPushingEntity`: what a moving brush model does to
//! the players and items on it and in its way
//! (docs/research/cod11-movers.md, section 12).

use crate::area::Link;
use crate::game::host::GameHost;
use crate::game::mover::Step;
use crate::spectate::ClientSim;
use glam::Vec3;
use vcod_common::collision::{CollisionWorld, MASK_PLAYERSOLID, Pusher};
use vcod_common::movetrace::{Body, CONTENTS_BODY, MoveWorld};
use vcod_common::patch::Arith;
use vcod_gsc::Cx;

/// `G_TryPushingEntity`'s jitter step, and its reach is half the body's
/// width (`maxs.x * 0.5`, rodata 0x75bf8 and 0x75c08).
const JITTER_INC: f32 = 4.0;
/// `ANGLE2SHORT`'s factor (rodata 0x75bf4).
use vcod_common::pmove::cmd::ANGLE2SHORT;

/// One push to undo if a later entity blocks the mover: `G_MoverTeam` puts
/// every record back and relinks it (0x55744..0x557ef).
enum Pushed {
    Client {
        slot: usize,
        origin: Vec3,
        delta_yaw: i32,
    },
    Item {
        id: vcod_gsc::EntId,
        origin: Vec3,
    },
}

/// One entity `G_MoverPush` keeps off its list.
enum Kept {
    /// An index into `sims`.
    Client(usize),
    Item(vcod_gsc::EntId, ItemBody),
}

/// `G_MoverPush` (0x550f0) over `step.listed`, players and items in one
/// area-tree order: every kept entity is unlinked first (0x55561), each is
/// pushed and relinked on its turn (0x555e1), and the whole list is linked
/// again at the end (0x55661). Returns the slots a `TR_SINE` pusher crushed,
/// or `None` when a player fits nowhere: every entity pushed before it is
/// back where it was and relinked, it and the rest of the list stay
/// unlinked, and the caller stalls the mover
/// (docs/research/cod11-movers.md, section 12). A sine mover is never
/// stopped: a player it cannot place is crushed (0x55617) and the push goes
/// on, the caller owing it `G_Damage`'s 99999 (section 21).
pub fn push(
    host: &mut GameHost,
    cx: &mut Cx,
    step: &Step,
    sims: &mut [(usize, &mut ClientSim)],
    world: &CollisionWorld,
) -> Option<Vec<usize>> {
    let amove = step.to.1 - step.from.1;
    let yaw_short = (amove.y * ANGLE2SHORT) as i32 & 0xffff;

    // The list is filtered once, against the mover where it now is.
    let kept: Vec<Kept> = step
        .listed
        .iter()
        .filter_map(|&n| {
            let link = host.area.last_link(n);
            if let Some(i) = sims.iter().position(|(s, _)| *s as u32 == n) {
                return listed_client(host, step, n, sims[i].1, link, world)
                    .then_some(Kept::Client(i));
            }
            let id = host.ents.handle(n)?;
            let body = crate::game::item::push_body(host, cx, id)?;
            item_in_way(host, step, n, &body, link, world).then_some(Kept::Item(id, body))
        })
        .collect();
    let number = |k: &Kept, sims: &[(usize, &mut ClientSim)]| match k {
        Kept::Client(i) => sims[*i].0 as u32,
        Kept::Item(id, _) => id.0,
    };
    for k in &kept {
        host.area.unlink(number(k, sims));
    }
    // A kept player is out of the tree until its turn relinks it, so it
    // blocks nobody listed ahead of it.
    let mut out: Vec<usize> = kept
        .iter()
        .filter_map(|k| match k {
            Kept::Client(i) => Some(*i),
            Kept::Item(..) => None,
        })
        .collect();

    let mut pushed: Vec<Pushed> = Vec::new();
    let mut crushed = Vec::new();
    for k in &kept {
        match k {
            Kept::Item(id, body) => {
                let fit = fit_item(step, body, world);
                if let Some(ItemPush::At(_)) = fit {
                    pushed.push(Pushed::Item {
                        id: *id,
                        origin: body.origin,
                    });
                }
                // One that fits nowhere is relinked where it is (0x555d4).
                crate::game::item::place_pushed(host, cx, *id, step, fit);
            }
            Kept::Client(i) => {
                let i = *i;
                out.retain(|&o| o != i);
                let bodies: Vec<Body> = sims
                    .iter()
                    .enumerate()
                    .filter(|(o, _)| *o != i && !out.contains(o))
                    .filter_map(|(_, (s, o))| o.body(*s as u32))
                    .collect();
                let old = sims[i].1.ps.origin;
                let ps = &sims[i].1.ps;
                let shape = (ps.origin, (ps.mins(), ps.maxs()));
                let Some(fit) = try_push(step, sims[i].0, shape, &bodies, world) else {
                    if crate::game::mover::is_sine(host, step.ent) {
                        crushed.push(sims[i].0);
                        continue;
                    }
                    undo(host, cx, sims, &pushed);
                    return None;
                };
                let (slot, sim) = &mut sims[i];
                match fit {
                    Fit::Stays => sim.ps.on_ground = false,
                    Fit::At(at) => {
                        // Pushed off whatever else it stood on.
                        if sim.ps.ground_entity_num() != step.number {
                            sim.ps.on_ground = false;
                        }
                        pushed.push(Pushed::Client {
                            slot: *slot,
                            origin: old,
                            delta_yaw: sim.delta_angles()[1],
                        });
                        sim.pushed_to(at);
                        sim.turn_delta_yaw(yaw_short);
                    }
                }
                link_client(host, *slot, sim);
            }
        }
    }
    for k in &kept {
        match k {
            Kept::Client(i) => link_client(host, sims[*i].0, sims[*i].1),
            Kept::Item(id, _) => host.link_entity(cx, *id),
        }
    }
    Some(crushed)
}

/// `G_TryPushingEntity`'s `trap_LinkEntity` of a pushed player, at
/// `r.currentOrigin`, unsnapped.
fn link_client(host: &mut GameHost, slot: usize, sim: &ClientSim) {
    host.link_client(
        slot,
        sim.link_origin().into(),
        (sim.ps.mins().into(), sim.ps.maxs().into()),
        sim.contents as i32,
        true,
    );
}

/// `G_MoverTeam`'s walk back over the records, newest first: each entity
/// back where it was pushed from, a player's yaw delta
/// back, and a relink.
fn undo(host: &mut GameHost, cx: &mut Cx, sims: &mut [(usize, &mut ClientSim)], pushed: &[Pushed]) {
    for p in pushed.iter().rev() {
        match p {
            Pushed::Client {
                slot,
                origin,
                delta_yaw,
            } => {
                if let Some((_, sim)) = sims.iter_mut().find(|(s, _)| s == slot) {
                    sim.pushed_to(*origin);
                    sim.set_delta_yaw(*delta_yaw);
                    link_client(host, *slot, sim);
                }
            }
            // Its ground stays `ENTITYNUM_NONE` (0x557b6 restores no ground).
            Pushed::Item { id, origin } => {
                crate::game::item::restore_pushed(host, cx, *id, *origin);
            }
        }
    }
}

/// `G_MoverPush`'s box test (0x5540e..0x5548f): a listed entity whose link
/// box misses the pusher's moved box is dropped before its trace. Boxes that
/// only touch miss.
fn overlaps_moved(step: &Step, link: Option<Link>) -> bool {
    let (Some((mins, maxs)), Some(l)) = (step.moved_box, link) else {
        return false;
    };
    (0..3).all(|i| l.absmin[i] < maxs[i] && l.absmax[i] > mins[i])
}

/// `G_MoverPush`'s candidate trace (game.mp 0x554ee): a `trap_Trace` of
/// the entity's box at its `r.currentOrigin` that does not move, passing the
/// entity itself, kept only when it names the pusher (0x55515). The world
/// clips first and a box inside it ends the trace there (`cod_lnxded`
/// 0x80916f4); past it the first entity in the walk's order whose clip the
/// box is inside names the trace (movers doc 12, "The candidate trace").
fn names_pusher(
    host: &GameHost,
    step: &Step,
    me: u32,
    at: Vec3,
    (mins, maxs): (Vec3, Vec3),
    mask: u32,
    world: &CollisionWorld,
) -> bool {
    if world.world_box_position_test(at, mins, maxs, mask) {
        return false;
    }
    let (lo, hi) = (at + mins, at + maxs);
    for n in host.area.trace_order(lo.into(), hi.into(), mask as i32) {
        if n == me {
            continue;
        }
        let inside = match entity_clip(host, step, n) {
            Some(pusher) => {
                world
                    .pusher_box_trace(pusher, at, at, mins, maxs, mask)
                    .startsolid
            }
            // `CM_TempBoxModel` over `r.mins`/`r.maxs` at the entity's
            // origin, the link box less its unit; a position test counts a
            // face it touches (INFERRED, Q3's `CM_TestBoxInBrush`).
            None => host.area.last_link(n).is_some_and(|l| {
                (0..3).all(|k| lo[k] <= l.absmax[k] - 1.0 && hi[k] >= l.absmin[k] + 1.0)
            }),
        };
        if inside {
            return n == step.number;
        }
    }
    false
}

/// What an entity clips a trace with when it is a brush model (`r.bmodel`,
/// `SV_ClipMoveToEntity` 0x8090f18): the pusher, a submodel, a world copy
/// or a UO vehicle's collmap. `None` for a box.
fn entity_clip(host: &GameHost, step: &Step, n: u32) -> Option<Pusher> {
    if n == step.number {
        return Some(step.pusher);
    }
    let id = host.ents.handle(n)?;
    match host.ents.get(id).and_then(|e| e.brush_model) {
        Some(0) => Some(Pusher::Copy(n)),
        Some(m) => Some(Pusher::Model(m as usize)),
        None => host
            .vehicles
            .live
            .get(&id)
            .and_then(|v| v.collmap)
            .map(|_| Pusher::Copy(n)),
    }
}

/// Whether `G_MoverPush` keeps a listed player: one standing on the mover,
/// or one whose box the mover's box now overlaps and whose candidate trace
/// names the mover (0x55405, 0x5540e, 0x554ee).
fn listed_client(
    host: &GameHost,
    step: &Step,
    me: u32,
    sim: &ClientSim,
    link: Option<Link>,
    world: &CollisionWorld,
) -> bool {
    if !sim.linked() || sim.contents & CONTENTS_BODY == 0 || sim.link_to.is_some() {
        return false;
    }
    // `r.currentOrigin` is `ps.origin` again past the cmd's touch pass
    // (0x405c7), unsnapped.
    sim.ps.ground_entity_num() == step.number
        || overlaps_moved(step, link)
            && names_pusher(
                host,
                step,
                me,
                sim.ps.origin,
                (sim.ps.mins(), sim.ps.maxs()),
                MASK_PLAYERSOLID,
                world,
            )
}

/// Where the mover's move and turn take a point: moved, then turned about the
/// mover's moved origin (`G_TryPushingEntity`, 0x54956-0x54aae). 1.1d keeps
/// the turn on the x87 stack and rounds once, adding `org2 - org` to the
/// stored moved point; 1.5b and UO store `org`, `org2` and `org2 - org`
/// (cod11-movers.md 21, "The carry's rounding").
fn carried(step: &Step, at: Vec3, arith: Arith) -> Vec3 {
    let moved = at + (step.to.0 - step.from.0);
    let amove = step.to.1 - step.from.1;
    if amove == Vec3::ZERO {
        return moved;
    }
    let axis = vcod_common::x87::rotation_matrix(amove);
    if arith != Arith::Kept {
        // `G_RotatePoint` sums each row on the stack and stores it.
        let org = moved - step.to.0;
        let o = org.as_dvec3();
        let org2 = (axis[0].as_dvec3() * o.x + axis[1].as_dvec3() * o.y + axis[2].as_dvec3() * o.z)
            .as_vec3();
        return moved + (org2 - org);
    }
    let org = (moved - step.to.0).as_dvec3();
    let org2 = axis[0].as_dvec3() * org.x + axis[1].as_dvec3() * org.y + axis[2].as_dvec3() * org.z;
    (moved.as_dvec3() + (org2 - org)).as_vec3()
}

/// An item as `G_MoverPush` sees it. Only a placed or dropped item still on
/// the ground is listed: a taken one has contents 0, and the box query's
/// mask 0x2000180 takes an item by its contents' 0x100.
pub struct ItemBody {
    pub origin: Vec3,
    pub mins: Vec3,
    pub maxs: Vec3,
    /// `clipmask`, or 0x11 when it is 0 (0x554b3).
    pub mask: u32,
    /// `s.groundEntityNum`.
    pub ground: i32,
}

/// The `G_TryPushingEntity` mask of an entity whose `clipmask` is 0.
pub const PUSH_DEFAULT_MASK: u32 = 0x11;

/// What a push does to an item. A mover never stalls on one: `G_MoverPush`
/// relinks an `eType` 3 entity that fits nowhere and goes on (0x555d4).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ItemPush {
    /// Moved to the spot. Its ground is `ENTITYNUM_NONE` unless it stood on
    /// the mover.
    At(Vec3),
    /// Left where it is with its ground `ENTITYNUM_NONE`, so `G_RunItem`
    /// drops it.
    Dropped,
}

/// `G_MoverPush`'s test for one listed item: it stands on the mover, or its
/// link box overlaps the mover's moved box and its candidate trace under
/// its own push mask names the mover.
fn item_in_way(
    host: &GameHost,
    step: &Step,
    me: u32,
    item: &ItemBody,
    link: Option<Link>,
    world: &CollisionWorld,
) -> bool {
    item.ground == step.number as i32
        || overlaps_moved(step, link)
            && names_pusher(
                host,
                step,
                me,
                item.origin,
                (item.mins, item.maxs),
                item.mask,
                world,
            )
}

/// `G_TryPushingEntity` for an item in the way: `None` when it fits
/// nowhere. An item is 2 units wide, under the jitter's reach, so the only
/// fallback is its own spot.
pub fn fit_item(step: &Step, item: &ItemBody, world: &CollisionWorld) -> Option<ItemPush> {
    let here = item.origin;
    // The sweep is the players' (see `try_push`): a zero-length box is
    // never inside terrain.
    let clear = |p: Vec3| {
        let t = world.item_trace(p, p, item.mins, item.maxs, item.mask);
        if t.startsolid || t.allsolid || world.box_position_test(p, item.mins, item.maxs, item.mask)
        {
            return false;
        }
        let sweep = world.box_trace_except(here, p, item.mins, item.maxs, item.mask, step.pusher);
        !sweep.startsolid && sweep.fraction >= 1.0
    };
    let target = carried(step, here, world.arith());
    if clear(target) {
        Some(ItemPush::At(target))
    } else if clear(here) {
        Some(ItemPush::Dropped)
    } else {
        None
    }
}

/// Where a push leaves a body.
enum Fit {
    At(Vec3),
    /// Its own spot is still clear: a rider the mover slid out from under,
    /// left in the air.
    Stays,
}

/// Where a player standing at `here` (`ps.origin`) with his box can go:
/// the carried spot, the first clear jitter around it in
/// `G_TryPushingEntity`'s order, or its own spot. `None` when the mover has
/// nowhere to put it.
fn try_push(
    step: &Step,
    slot: usize,
    (here, (mins, maxs)): (Vec3, (Vec3, Vec3)),
    bodies: &[Body],
    world: &CollisionWorld,
) -> Option<Fit> {
    let mw = MoveWorld::new(world, bodies, slot as u32);
    // `G_TryPushingEntity` moves `r.currentOrigin` (0x54956); the ride
    // capture keeps `ps.origin`'s fraction through every push, so that is
    // what it reads.
    let target = carried(step, here, world.arith());
    let pusher = step.pusher;
    // Retail's test is one `trap_Trace` at the spot (game.mp 0x54b3b). Ours
    // also sweeps there from where the body stood, past the pusher it may
    // start inside: a zero-length capsule is never `startsolid` under
    // terrain, and without the sweep a slab lowered onto a head pushes the
    // body through the ground (movers doc, section 12).
    let clear = |p: Vec3| {
        let t = mw.box_trace(p, p, mins, maxs, MASK_PLAYERSOLID);
        if t.startsolid || t.allsolid || world.box_position_test(p, mins, maxs, MASK_PLAYERSOLID) {
            return false;
        }
        let sweep = world.box_trace_except(here, p, mins, maxs, MASK_PLAYERSOLID, pusher);
        !sweep.startsolid && sweep.fraction >= 1.0
    };
    if clear(target) {
        return Some(Fit::At(target));
    }
    let reach = maxs.x * 0.5;
    if reach > JITTER_INC {
        let mut z = 0.0;
        while z < reach {
            for fz in if z == 0.0 { vec![0.0] } else { vec![-z, z] } {
                let mut x = JITTER_INC;
                while x < reach {
                    for fx in [-x, x] {
                        let mut y = JITTER_INC;
                        while y < reach {
                            for fy in [-y, y] {
                                let p = target + Vec3::new(fx, fy, fz);
                                if clear(p) {
                                    return Some(Fit::At(p));
                                }
                            }
                            y += JITTER_INC;
                        }
                    }
                    x += JITTER_INC;
                }
            }
            z += JITTER_INC;
        }
    }
    clear(here).then_some(Fit::Stays)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn step(moved: (Vec3, Vec3)) -> Step {
        Step {
            ent: vcod_gsc::EntId(100, 0),
            number: 100,
            pusher: vcod_common::collision::Pusher::Model(1),
            from: (Vec3::ZERO, Vec3::ZERO),
            to: (Vec3::X, Vec3::ZERO),
            listed: vec![1],
            moved_box: Some(moved),
        }
    }

    /// `G_MoverPush` skips a listed entity whose link box only touches the
    /// pusher's moved box (`absmin >= maxs` or `absmax <= mins` on any axis,
    /// 0x5540e..0x5548f), and keeps one that overlaps it by any amount.
    #[test]
    fn the_box_test_drops_a_box_that_only_touches() {
        let s = step((Vec3::new(0.0, 0.0, 0.0), Vec3::new(64.0, 64.0, 8.0)));
        let player = |x: f32| {
            Some(Link::boxed(
                [x, 32.0, 8.0],
                [-15.0, -15.0, 0.0],
                [15.0, 15.0, 70.0],
                0x2000000,
            ))
        };
        // absmin.x = 80 - 15 - 1 = 64: touching the max face.
        assert!(!overlaps_moved(&s, player(80.0)));
        assert!(overlaps_moved(&s, player(79.9)));
        // absmax.x = -16 + 15 + 1 = 0: touching the min face.
        assert!(!overlaps_moved(&s, player(-16.0)));
        assert!(overlaps_moved(&s, player(-15.9)));
        // Standing on the top face: absmin.z = 8 + 0 - 1 = 7 overlaps.
        assert!(overlaps_moved(&s, player(32.0)));
        assert!(!overlaps_moved(&s, None));
    }

    /// The retail rest spots `tag` logged for slots 1 and 2 in a pushcand
    /// capture (`PROBE <tag> <slot> (x, y, z)`).
    fn rest_spots(fixture: &str, tag: &str) -> [Vec3; 2] {
        [1, 2].map(|slot| {
            let head = format!("PROBE {tag} {slot} (");
            let l = fixture
                .lines()
                .find_map(|l| l.strip_prefix(&head))
                .expect("the tag in the fixture");
            let v: Vec<f32> = l
                .trim_end_matches(')')
                .split(", ")
                .map(|c| c.parse().unwrap())
                .collect();
            Vec3::new(v[0], v[1], v[2])
        })
    }

    /// On 1.5b and UO `StuckInClient` shoves the stacked and half pairs
    /// ~100 apart before the slab moves, and the slab still stalls (movers
    /// doc 12, "The candidate trace"). The partner is not what blocks it:
    /// one of each pair comes to rest with his capsule clear of the slab's
    /// clip_metal brush 4277 (its underside slopes), but his box inside it.
    /// The candidate trace (a box) names the slab, and every spot
    /// `G_TryPushingEntity` tries with its box (1.5b 0x55bff, 0x55eb7,
    /// 0x5609a) is in 4277, the plate 4281, the next brush or the street,
    /// so the push fails on every frame. The lone player's spot is the
    /// control: carried 8.
    #[test]
    fn on_15_and_uo_a_body_whose_box_is_under_the_slab_stalls_it() {
        // A UO install's `main/` is 1.5's, so both run there.
        let fs_15 = || {
            let fs = vcod_common::testing::game_fs()?;
            let p = crate::server::install_protocol(&fs);
            (p.transport.version == vcod_common::net::protocol::PROTOCOL_V6.transport.version)
                .then_some(fs)
        };
        let cases = [
            (
                fs_15(),
                include_str!("../tests/fixtures/movers/mp_carentan-dm-pushcand-1.5.txt"),
            ),
            (
                vcod_common::testing::uo_fs(),
                include_str!("../tests/fixtures/movers/mp_carentan-dm-pushcand-uo.txt"),
            ),
        ];
        let mut ran = 0;
        for (fs, fixture) in cases {
            let Some(fs) = fs else { continue };
            ran += 1;
            let p = crate::server::install_protocol(&fs);
            let bsp_path = fs.resolve_map("mp_carentan").unwrap();
            let bsp = vcod_common::bsp::parse(&fs.read(&bsp_path).unwrap()).unwrap();
            let world = crate::world::World::from_bsp(&bsp, Some(&fs)).collision;
            world.set_arith(Arith::for_protocol(p));
            let (mins, maxs) = (Vec3::new(-15.0, -15.0, 0.0), Vec3::new(15.0, 15.0, 70.0));
            let slab = Pusher::Model(5);
            let push = Step {
                number: 177,
                pusher: slab,
                from: (Vec3::ZERO, Vec3::ZERO),
                to: (Vec3::new(0.0, -8.0, 0.0), Vec3::ZERO),
                ..step((Vec3::ZERO, Vec3::ZERO))
            };
            for tag in ["after_stack", "after_half"] {
                let spots = rest_spots(fixture, tag);
                // The slab at rest: where pmove left them.
                world.set_model_pose(5, Vec3::ZERO, Vec3::ZERO);
                let under: Vec<Vec3> = spots
                    .into_iter()
                    .filter(|&at| {
                        world
                            .pusher_box_trace(slab, at, at, mins, maxs, MASK_PLAYERSOLID)
                            .startsolid
                    })
                    .collect();
                assert_eq!(under.len(), 1, "{tag}: one of {spots:?} under the slab");
                let at = under[0];
                let capsule = world.move_trace(at, at, mins, maxs, MASK_PLAYERSOLID);
                assert!(!capsule.startsolid, "{tag}: the capsule at {at} is clear");
                world.set_model_pose(5, push.to.0, Vec3::ZERO);
                assert!(
                    try_push(&push, 2, (at, (mins, maxs)), &[], &world).is_none(),
                    "{tag}: the slab finds {at} a spot"
                );
            }
            let alone = rest_spots(fixture, "set_alone")[0];
            assert!(matches!(
                try_push(&push, 1, (alone, (mins, maxs)), &[], &world),
                Some(Fit::At(at)) if at == alone - Vec3::Y * 8.0
            ));
            world.set_model_pose(5, Vec3::ZERO, Vec3::ZERO);
        }
        if ran == 0 {
            eprintln!("no 1.5 or UO install under COD_DIR: skipping");
        }
    }
}
