//! `G_MoverPush` and `G_TryPushingEntity`: what a moving brush model does to
//! the players and items on it and in its way
//! (docs/research/cod11-movers.md, section 12).

use crate::game::host::GameHost;
use crate::game::mover::Step;
use crate::spectate::ClientSim;
use glam::Vec3;
use vcod_common::collision::{CollisionWorld, MASK_PLAYERSOLID};
use vcod_common::movetrace::{Body, CONTENTS_BODY, MoveWorld};
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
/// again at the end (0x55661). `false` when a player fits nowhere: every
/// entity pushed before it is back where it was and relinked, it and the
/// rest of the list stay unlinked, and the caller stalls the mover
/// (docs/research/cod11-movers.md, section 12).
pub fn push(
    host: &mut GameHost,
    cx: &mut Cx,
    step: &Step,
    sims: &mut [(usize, &mut ClientSim)],
    world: &CollisionWorld,
) -> bool {
    let amove = step.to.1 - step.from.1;
    let yaw_short = (amove.y * ANGLE2SHORT) as i32 & 0xffff;

    // The list is filtered once, against the mover where it now is.
    let kept: Vec<Kept> = step
        .listed
        .iter()
        .filter_map(|&n| {
            if let Some(i) = sims.iter().position(|(s, _)| *s as u32 == n) {
                return listed_client(step, sims[i].1, world).then_some(Kept::Client(i));
            }
            let id = host.ents.handle(n)?;
            let body = crate::game::item::push_body(host, cx, id)?;
            item_in_way(step, &body, world).then_some(Kept::Item(id, body))
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
                let Some(fit) = try_push(step, sims[i].0, sims[i].1, &bodies, world) else {
                    undo(host, cx, sims, &pushed);
                    return false;
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
    true
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

/// Whether `G_MoverPush` keeps a listed player: one standing on the mover,
/// or one whose box the mover's brushes now overlap (0x55405, 0x554ee).
fn listed_client(step: &Step, sim: &ClientSim, world: &CollisionWorld) -> bool {
    if !sim.linked() || sim.contents & CONTENTS_BODY == 0 || sim.link_to.is_some() {
        return false;
    }
    let o = sim.ps.origin;
    sim.ps.ground_entity_num() == step.number
        || world
            .model_box_trace(
                step.model,
                o,
                o,
                sim.ps.mins(),
                sim.ps.maxs(),
                MASK_PLAYERSOLID,
            )
            .startsolid
}

/// Where the mover's move and turn take a point: moved, then turned about the
/// mover's moved origin (`G_TryPushingEntity`, 0x54956-0x54aae).
fn carried(step: &Step, at: Vec3) -> Vec3 {
    let moved = at + (step.to.0 - step.from.0);
    let amove = step.to.1 - step.from.1;
    if amove == Vec3::ZERO {
        return moved;
    }
    let axis = vcod_common::pmove::aim::angles_to_axis(amove.to_array()).map(Vec3::from);
    let v = moved - step.to.0;
    step.to.0 + axis[0] * v.x + axis[1] * v.y + axis[2] * v.z
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
/// box at its origin meets the mover's brushes under its own push mask.
pub fn item_in_way(step: &Step, item: &ItemBody, world: &CollisionWorld) -> bool {
    let here = item.origin;
    item.ground == step.number as i32
        || world
            .model_box_trace(step.model, here, here, item.mins, item.maxs, item.mask)
            .startsolid
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
        if t.startsolid || t.allsolid {
            return false;
        }
        let sweep = world.box_trace_except(here, p, item.mins, item.maxs, item.mask, step.model);
        !sweep.startsolid && sweep.fraction >= 1.0
    };
    let target = carried(step, here);
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

/// Where a player can go: the carried spot, the first clear jitter around it in
/// `G_TryPushingEntity`'s order, or its own spot. `None` when the mover has
/// nowhere to put it.
fn try_push(
    step: &Step,
    slot: usize,
    sim: &ClientSim,
    bodies: &[Body],
    world: &CollisionWorld,
) -> Option<Fit> {
    let (mins, maxs) = (sim.ps.mins(), sim.ps.maxs());
    let mw = MoveWorld::new(world, bodies, slot as u32);
    // `G_TryPushingEntity` moves `r.currentOrigin` (0x54956); the ride
    // capture keeps `ps.origin`'s fraction through every push, so that is
    // what it reads.
    let here = sim.ps.origin;
    let target = carried(step, here);
    let pusher = step.model;
    // Retail's test is one `trap_Trace` at the spot (game.mp 0x54b3b). Ours
    // also sweeps there from where the body stood, past the pusher it may
    // start inside: a zero-length capsule is never `startsolid` under
    // terrain, and without the sweep a slab lowered onto a head pushes the
    // body through the ground (movers doc, section 12).
    let clear = |p: Vec3| {
        let t = mw.box_trace(p, p, mins, maxs, MASK_PLAYERSOLID);
        if t.startsolid || t.allsolid {
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
