//! `G_MoverPush` and `G_TryPushingEntity` over the players: what a moving
//! brush model does to the bodies on it and in its way
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
const ANGLE2SHORT: f32 = 65536.0 / 360.0;

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
    let mv = step.to.0 - step.from.0;
    let amove = step.to.1 - step.from.1;
    let axis = vcod_common::pmove::aim::angles_to_axis(amove.to_array()).map(Vec3::from);
    let rotate = |v: Vec3| axis[0] * v.x + axis[1] * v.y + axis[2] * v.z;
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
                    .model_box_trace(step.model, o, o, sim.ps.mins(), sim.ps.maxs())
                    .startsolid
        })
        .collect();

    let mut pushed: Vec<Pushed> = Vec::new();
    for i in list {
        let old = sims[i].1.ps.origin;
        let moved = old + mv;
        let target = if amove == Vec3::ZERO {
            moved
        } else {
            step.to.0 + rotate(moved - step.to.0)
        };
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
        let sweep = world.box_trace_except(here, p, mins, maxs, pusher);
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
