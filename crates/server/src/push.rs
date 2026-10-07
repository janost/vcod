//! `G_MoverPush` and `G_TryPushingEntity`: what a moving brush model does to
//! the players and items on it and in its way
//! (docs/research/cod11-movers.md, section 12).

use crate::game::mover::Step;
use crate::spectate::ClientSim;
use glam::Vec3;
use vcod_common::collision::{CollisionWorld, MASK_PLAYERSOLID};
use vcod_common::movetrace::{Body, CONTENTS_BODY, MoveWorld};

/// `G_TryPushingEntity`'s jitter step, and its reach is half the body's
/// width (`maxs.x * 0.5`, rodata 0x75bf8 and 0x75c08).
const JITTER_INC: f32 = 4.0;
/// `ANGLE2SHORT`'s factor (rodata 0x75bf4).
use vcod_common::pmove::cmd::ANGLE2SHORT;

/// One push to undo if a later body blocks the mover: `G_MoverTeam` puts
/// every pushed body back before it stalls.
struct Pushed {
    slot: usize,
    origin: Vec3,
    delta_yaw: i32,
}

/// Moves every player `step` carries or shoves, in slot order. `false` when
/// one fits nowhere; every body pushed before it is back where it was, and
/// the caller stalls the mover.
pub fn push(step: &Step, sims: &mut [(usize, &mut ClientSim)], world: &CollisionWorld) -> bool {
    let amove = step.to.1 - step.from.1;
    let yaw_short = (amove.y * ANGLE2SHORT) as i32 & 0xffff;

    // The list is taken once, against the mover where it now is: a body
    // standing on it, or one its brushes now overlap.
    let list: Vec<usize> = (0..sims.len())
        .filter(|&i| {
            let sim = &*sims[i].1;
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
        })
        .collect();

    let mut pushed: Vec<Pushed> = Vec::new();
    for i in list {
        let old = sims[i].1.ps.origin;
        let target = carried(step, old);
        match try_push(i, sims, world, step.model, target) {
            Some(Fit::Stays) => sims[i].1.ps.on_ground = false,
            Some(Fit::At(at)) => {
                let (slot, sim) = &mut sims[i];
                // Pushed off whatever else it stood on.
                if sim.ps.ground_entity_num() != step.number {
                    sim.ps.on_ground = false;
                }
                pushed.push(Pushed {
                    slot: *slot,
                    origin: old,
                    delta_yaw: sim.delta_angles()[1],
                });
                sim.ps.origin = at;
                sim.turn_delta_yaw(yaw_short);
            }
            None => {
                for p in pushed.iter().rev() {
                    if let Some((_, sim)) = sims.iter_mut().find(|(s, _)| *s == p.slot) {
                        sim.ps.origin = p.origin;
                        sim.set_delta_yaw(p.delta_yaw);
                    }
                }
                return false;
            }
        }
    }
    true
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

/// `G_MoverPush`'s test and `G_TryPushingEntity` for one item: `None` when
/// the item is not in the mover's way, or is and fits nowhere, and stays put.
/// An item is 2 units wide, under the jitter's reach, so the only fallback is
/// its own spot.
pub fn push_item(step: &Step, item: &ItemBody, world: &CollisionWorld) -> Option<ItemPush> {
    let here = item.origin;
    let on = item.ground == step.number as i32;
    if !on
        && !world
            .model_box_trace(step.model, here, here, item.mins, item.maxs, item.mask)
            .startsolid
    {
        return None;
    }
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

/// Where `sims[i]` can go: `target`, the first clear jitter around it in
/// `G_TryPushingEntity`'s order, or its own spot. `None` when the mover has
/// nowhere to put it.
fn try_push(
    i: usize,
    sims: &[(usize, &mut ClientSim)],
    world: &CollisionWorld,
    pusher: usize,
    target: Vec3,
) -> Option<Fit> {
    let (slot, sim) = (sims[i].0, &*sims[i].1);
    let (mins, maxs) = (sim.ps.mins(), sim.ps.maxs());
    let bodies: Vec<Body> = sims
        .iter()
        .filter(|(s, _)| *s != slot)
        .filter_map(|(s, o)| o.body(*s as u32))
        .collect();
    let mw = MoveWorld::new(world, &bodies, slot as u32);
    let here = sim.ps.origin;
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
