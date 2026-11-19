//! `PM_SlideMove` and `PM_StepSlideMove`.

use super::footsteps::{footsteps_audible, ground_step_event};
use super::ground::pm_trace;
use super::jump::JUMP_HEIGHT;
use super::prone::{PRONE_CHECK_LENGTH, ProneQuery, check_prone};
use super::state::{MoveRules, PlayerState, PmEvent, Stance};
use super::walk::clip_velocity;
use super::{MAX_CLIENTS, MAX_CLIP_PLANES, STEP_REVERT_EPS, STEPSIZE, STEPSIZE_PRONE};
use crate::movetrace::MoveWorld;
/// `EV_STEP_VIEW`: the vertical jump the step machinery added this frame,
/// which the client smooths the eye over
/// (docs/research/cod11-mantle.md, "The step event and the velocity scale").
use crate::net::event_ids::EV_STEP_VIEW;
use crate::x87;
use glam::Vec3;

/// Step below which retail raises nothing (double @0x70f08).
const STEP_VIEW_EPS: f32 = 0.5;
/// Clamp and bias the rounded step takes before it becomes the parm
/// (@0x35727-0x35742); the parm is 8 bits on the wire.
const STEP_VIEW_MIN: i32 = -16;
const STEP_VIEW_MAX: i32 = 24;
pub(super) const STEP_VIEW_BIAS: i32 = 128;
/// Post-step velocity scale `0.2 + 0.8 * (1 - |dz| / stepSize)`
/// (rodata 0x70f14/0x70f10, applied @0x35770). The 0.2 is 0x3e4ccccc, a
/// bit under `0.2f32`; 1.5b and UO reach the same bits as `1.0 - 0.8f`
/// (1.5b 0x2a4fb, 0x2a5d9).
const STEP_SCALE_BASE: f32 = f32::from_bits(0x3e4c_cccc);
const STEP_SCALE_GAIN: f32 = 0.8;

/// What [`slide_move`] ran into.
#[derive(Clone, Copy)]
pub(super) struct Slide {
    pub(super) blocked: bool,
}

/// Q3 `bg_slidemove.c` `PM_SlideMove`.
pub(super) fn slide_move(
    ps: &mut PlayerState,
    world: &MoveWorld,
    dt: f32,
    gravity: bool,
    mask: u32,
) -> Slide {
    const NUM_BUMPS: usize = 4;
    let (mins, maxs) = (ps.mins(), ps.maxs());

    // `pml.groundPlane`, which a steep plane sets too (0x3483b reads it, not
    // `pml.walking`); `on_ground` covers a state no ground trace has run on.
    let ground = ps.ground_plane.or(ps.on_ground.then_some(ps.ground_normal));
    // Q3's `primal_velocity`: what a running timer hands back at the end.
    let mut primal = ps.velocity;
    let mut end_velocity = ps.velocity;
    if gravity {
        // The average of start and end velocity, matching the analytic
        // parabola. The x87 keeps `v - g * frametime` unrounded into it and
        // rounds each store once (0x34811-0x34838).
        let mut end_z = f64::from(ps.velocity.z) - f64::from(ps.gravity) * f64::from(dt);
        end_velocity.z = end_z as f32;
        if ps.rules.v15() || world.world.arith() == crate::patch::Arith::Msvc {
            // 1.5b stores it first (0x28e34) and averages the float
            // (cod15-movement.md 7), and so does every Windows cgame
            // (8).
            end_z = f64::from(end_velocity.z);
        }
        primal.z = end_velocity.z;
        ps.velocity.z = ((end_z + f64::from(ps.velocity.z)) * 0.5) as f32;
        if let Some(n) = ground {
            ps.velocity = clip_velocity(ps.velocity, n, ps.rules.v15());
        }
    }

    let mut planes: Vec<Vec3> = Vec::with_capacity(MAX_CLIP_PLANES);
    // never turn against the ground plane or back against the original move
    if let Some(n) = ground {
        planes.push(n);
    }
    // 1.5b's `VectorNormalize2` (0x43216) stores its steps too.
    planes.push(if ps.rules.v15() {
        x87::normalize_stored(ps.velocity)
    } else {
        x87::normalize(ps.velocity)
    });

    let v15 = ps.rules.v15();
    let mut time_left = dt;
    let mut bumps = 0;
    for _ in 0..NUM_BUMPS {
        // `VectorMA` on the x87 rounds once, into the float store.
        let end = (ps.origin.as_dvec3() + ps.velocity.as_dvec3() * f64::from(time_left)).as_vec3();
        let t = pm_trace(ps, world, ps.origin, end, mins, maxs, mask);
        if t.allsolid {
            // trapped in solid: keep the horizontal control, kill the fall
            ps.velocity.z = 0.0;
            return Slide { blocked: true };
        }
        if t.fraction > 0.0 {
            ps.origin = t.endpos;
        }
        if t.fraction == 1.0 {
            break;
        }
        world.touch(&t);
        bumps += 1;
        time_left = (f64::from(time_left) - f64::from(time_left) * f64::from(t.fraction)) as f32;

        if planes.len() >= MAX_CLIP_PLANES {
            ps.velocity = Vec3::ZERO;
            return Slide { blocked: true };
        }
        // same plane again: nudge out along it (`> 0.999`, 1.1d 0x34ad9,
        // 1.5b 0x291e4, UO 0x2ead8)
        if planes
            .iter()
            .any(|p| x87::dot(t.normal, *p) > f64::from(0.999f32))
        {
            ps.velocity += t.normal;
            continue;
        }
        planes.push(t.normal);

        for (i, &plane_i) in planes.iter().enumerate() {
            // 1.5b stores this dot before the compare (0x293b5).
            let into = x87::dot(ps.velocity, plane_i);
            let into = if v15 { f64::from(into as f32) } else { into };
            if into >= 0.1 {
                continue;
            }
            let mut clipped = clip_velocity(ps.velocity, plane_i, v15);
            let mut end_clipped = clip_velocity(end_velocity, plane_i, v15);

            for (j, &plane_j) in planes.iter().enumerate() {
                if j == i || x87::dot(clipped, plane_j) >= 0.1 {
                    continue;
                }
                clipped = clip_velocity(clipped, plane_j, v15);
                end_clipped = clip_velocity(end_clipped, plane_j, v15);
                if x87::dot(clipped, plane_i) >= 0.0 {
                    continue;
                }
                // two planes: slide along their crease
                // Two opposed planes cross to a zero vector, which
                // `VectorNormalize` leaves signed: head-on into a wall the
                // velocity comes out (0, 0, -0) (protocol doc, "Spectator").
                // 1.5b stores the normalize's steps and each dot (0x29667-0x29714).
                let cross = x87::cross(plane_i, plane_j);
                let crease = if v15 {
                    x87::normalize_stored(cross)
                } else {
                    x87::normalize_in_place(cross)
                };
                let along = |v: Vec3| {
                    let d = x87::dot(crease, v);
                    if v15 { f64::from(d as f32) } else { d }
                };
                clipped = x87::scale(crease, along(ps.velocity));
                end_clipped = x87::scale(crease, along(end_velocity));

                // three planes: nowhere left to go
                if planes
                    .iter()
                    .enumerate()
                    .any(|(k, &p)| k != i && k != j && x87::dot(clipped, p) < 0.1)
                {
                    ps.velocity = Vec3::ZERO;
                    return Slide { blocked: true };
                }
            }

            ps.velocity = clipped;
            end_velocity = end_clipped;
            break;
        }
    }

    if gravity {
        ps.velocity = end_velocity;
    }
    // Under a running `pm_time` the clips do not reach the velocity
    // (0x34f83-0x34f9c; docs/research/cod11-player-clip.md 8.5).
    if ps.knockback_ms != 0.0 {
        ps.velocity = primal;
    }
    Slide {
        blocked: bumps != 0,
    }
}

/// Q3 `bg_slidemove.c` `PM_StepSlideMove`. Step height 18, or 10 while prone
/// (retail picks the height off pm_flags bit 0x1 @0x35045, not ladder state).
/// A prone move ends in `PM_VerifyPronePosition` (0x346e0): a body that no
/// longer fits where the move left it goes back to where it started, with
/// no step event (docs/research/cod11-mantle.md, "The ground samples").
pub(super) fn step_slide_move(
    ps: &mut PlayerState,
    world: &MoveWorld,
    dt: f32,
    gravity: bool,
    mask: u32,
    mut events: Option<&mut Vec<PmEvent>>,
) {
    let (start_o, start_v) = (ps.origin, ps.velocity);
    let raised = events.as_ref().map_or(0, |e| e.len());
    // The entry gate's and the down pass's early outs skip the tail, and
    // the check with it (0x350ce..0x3510c, 0x35377).
    if !step_slide(ps, world, dt, gravity, mask, events.as_deref_mut())
        || ps.stance != Stance::Prone
    {
        return;
    }
    let q = ProneQuery {
        origin: ps.origin,
        yaw_deg: ps.prone_direction,
        entry: false,
        ground: true,
        length: PRONE_CHECK_LENGTH,
        rules: ps.rules,
    };
    match check_prone(world, q) {
        Some(body) => ps.prone_body = body,
        None => {
            ps.origin = start_o;
            ps.velocity = start_v;
            if let Some(e) = events {
                e.truncate(raised);
            }
        }
    }
}

fn step_slide(
    ps: &mut PlayerState,
    world: &MoveWorld,
    dt: f32,
    gravity: bool,
    mask: u32,
    events: Option<&mut Vec<PmEvent>>,
) -> bool {
    let mut step_size = if ps.stance == Stance::Prone {
        STEPSIZE_PRONE
    } else {
        STEPSIZE
    };
    let start_o = ps.origin;
    let start_v = ps.velocity;
    let v15 = ps.rules.v15();
    // 1.5's top (0x2997f-0x299b2): a ladder ends the jump penalty, and so
    // does a penalty timer running in the air.
    let penalty_timed = ps.jump_penalty && ps.knockback_ms != 0.0;
    if v15 && (ps.on_ladder || ps.ground_plane.is_none() && penalty_timed) {
        ps.jump_penalty = false;
        ps.jump_origin_z = 0.0;
    }
    let slide = slide_move(ps, world, dt, gravity, mask);
    let blocked = slide.blocked;
    // And again for any airborne body, steep plane or not (0x299dd).
    if v15 && !ps.on_ground && ps.jump_penalty && ps.knockback_ms != 0.0 {
        ps.jump_penalty = false;
        ps.jump_origin_z = 0.0;
    }
    // The height the jump's step may reach: 1.1 stores it, 1.5 stores the
    // takeoff and adds the jump height while the penalty flag stands.
    let peak = match ps.rules {
        MoveRules::V1_1 => (ps.jump_origin_z.abs() > 0.001).then_some(ps.jump_origin_z),
        MoveRules::V1_5 | MoveRules::Uo => {
            ps.jump_penalty.then_some(ps.jump_origin_z + JUMP_HEIGHT)
        }
    };
    // The entry gate (0x35057-0x35112). Retail takes the down pass on every
    // grounded frame, not only a blocked one, where Q3 returns as soon as the
    // slide succeeded (docs/research/cod11-mantle.md, "The ground snap"). In
    // the air it steps only a blocked move: one still under its jump's origin,
    // with the step cut to reach no higher than that origin, or a climb up a
    // ladder ("The jump's step").
    let mut jump_step = false;
    if let Some(peak) = peak.filter(|&p| blocked && !ps.on_ground && start_o.z < p) {
        let reach = peak - start_o.z;
        if reach < 1.0 {
            return false;
        }
        step_size = reach.min(STEPSIZE);
        jump_step = true;
    }
    // A spectator's `groundEntityNum` is never ground traced and reads 0,
    // not `ENTITYNUM_NONE`, so the gate (0x3507f) always lets it through.
    // 1.5 takes a rising climber through blocked or not (0x29a70).
    let climbing = ps.on_ladder && ps.velocity.z > 0.0 && (blocked || v15);
    let through = ps.on_ground || ps.spectator || jump_step || climbing;
    if !through {
        return false;
    }
    let (mins, maxs) = (ps.mins(), ps.maxs());
    let (down_o, down_v) = (ps.origin, ps.velocity);
    // Retail's `ebx` (0x34fdf-0x34ff1): on a ground plane and not on a
    // ladder. 1.5 reads `pml.groundPlane`, a steep plane's too.
    let ground_plane = !ps.on_ladder
        && match ps.rules {
            MoveRules::V1_1 => ps.on_ground,
            MoveRules::V1_5 | MoveRules::Uo => ps.ground_plane.is_some(),
        };
    // The ground snap's reach: half a step in 1.1, 9 whatever the step in
    // 1.5 (rodata 0x75cb4).
    let snap = if v15 { 9.0 } else { step_size * 0.5 };

    // The step-up runs on a blocked slide only (0x35166): the up trace over
    // `stepSize + 1`, `stepUp` a unit under what it reached and nothing under
    // 1 (0x351ca-0x351e8), the slide again from up there
    // (docs/research/cod11-mantle.md, "The step-up stops a unit short").
    // The Linux builds keep `stepSize + 1` and its product with the
    // fraction on the stack (1.1d 0x35173-0x351d8, 1.5b 0x29ccf, 0x29d46).
    let linux = world.world.arith() != crate::patch::Arith::Msvc;
    let lift = f64::from(step_size) + 1.0;
    let mut step = 0.0;
    if blocked {
        let top = if linux {
            start_o.with_z((f64::from(start_o.z) + lift) as f32)
        } else {
            start_o + Vec3::Z * (step_size + 1.0)
        };
        let up = pm_trace(ps, world, start_o, top, mins, maxs, mask);
        let room = if linux {
            (lift * f64::from(up.fraction) - 1.0) as f32
        } else {
            (step_size + 1.0) * up.fraction - 1.0
        };
        if !up.allsolid && room >= 1.0 {
            step = room;
            ps.origin = start_o + Vec3::Z * room;
            ps.velocity = start_v;
            slide_move(ps, world, dt, gravity, mask);
        }
    }

    // The down pass: past the step by half a step size more on a ground
    // plane (0x352e8), and taken on an unblocked grounded frame too, where
    // it reaches 9 units under the plain slide (docs/research/cod11-mantle.md,
    // "The ground snap").
    if ground_plane || step != 0.0 {
        // `end[2] -= stepUp`, then the snap: 1.1d keeps the difference on
        // the stack (0x352d8-0x352f6), 1.5b stores it (0x29e89, 0x29eaa).
        let z = f64::from(ps.origin.z) - f64::from(step);
        let z = match world.world.arith() {
            crate::patch::Arith::Kept if ground_plane => (z - f64::from(snap)) as f32,
            crate::patch::Arith::Stored if ground_plane => {
                (f64::from(z as f32) - f64::from(snap)) as f32
            }
            crate::patch::Arith::Msvc => {
                ps.origin.z - (step + if ground_plane { snap } else { 0.0 })
            }
            _ => z as f32,
        };
        let down = pm_trace(ps, world, ps.origin, ps.origin.with_z(z), mins, maxs, mask);
        // A down pass that meets a player drops the step and the snap and
        // keeps the plain slide (0x3533a; docs/research/cod11-player-clip.md 2.3).
        if world.entity_num(&down) < MAX_CLIENTS {
            ps.origin = down_o;
            ps.velocity = down_v;
            return false;
        }
        if down.fraction < 1.0 {
            ps.origin = down.endpos;
            ps.velocity = clip_velocity(ps.velocity, down.normal, ps.rules.v15());
        } else {
            // Nothing within reach: the step is undone and the snap is not
            // a fall (0x353d0).
            ps.origin.z -= step;
        }
    }

    // Retail's revert (0x353f7-0x3546e): the plain slide's displacement and
    // the stepped one's, both projected on the velocity the down pass left,
    // and the plain slide's state comes back whenever it got as far, so a
    // step that cleared nothing is undone together with its down pass
    // (docs/research/cod11-mantle.md, "The ground snap").
    // A jump's step that ends at or above the jump's origin is reverted too
    // (0x35450-0x35468).
    // The Linux builds take both dots on the stack; 1.1d's stepped x stays
    // there too, unrounded (0x353ff-0x35447), 1.5b stores it (0x2a04e).
    let v = ps.velocity.truncate();
    let flat_d = (down_o - start_o).truncate();
    let reverted = match world.world.arith() {
        crate::patch::Arith::Msvc => {
            let flat = v.dot(flat_d);
            let stepped = v.dot((ps.origin - start_o).truncate());
            flat + STEP_REVERT_EPS > stepped
        }
        arith => {
            let (vx, vy) = (f64::from(v.x), f64::from(v.y));
            let flat =
                (vx * f64::from(flat_d.x) + vy * f64::from(flat_d.y)) + f64::from(STEP_REVERT_EPS);
            let sx = f64::from(ps.origin.x) - f64::from(start_o.x);
            let sx = if arith == crate::patch::Arith::Stored {
                f64::from(sx as f32)
            } else {
                sx
            };
            let sy = f64::from(ps.origin.y - start_o.y);
            flat > vy * sy + vx * sx
        }
    };
    let peak = peak.unwrap_or(0.0);
    if reverted || jump_step && ps.origin.z >= peak {
        ps.origin = down_o;
        ps.velocity = down_v;
        // The ground snap proper (0x354cc-0x3557b): a reverted move on a
        // ground plane is pulled down by half a step size from the plain
        // slide's end, which is what walks a player down a kerb without a
        // fall and raises the negative step events the captures carry.
        if ground_plane {
            let down = pm_trace(
                ps,
                world,
                ps.origin,
                ps.origin - Vec3::Z * snap,
                mins,
                maxs,
                mask,
            );
            if down.fraction < 1.0 {
                ps.origin = down.endpos;
                ps.velocity = clip_velocity(ps.velocity, down.normal, ps.rules.v15());
            }
        }
    }
    // A jump's step that rose is held to the speed that just reaches the
    // jump's origin, and to none within 0.1 of it (0x355a4-0x35655).
    // 1.1d keeps `left` and the root's argument on the stack
    // (0x355cd-0x355f9), 1.5b stores `left` (0x2a3a3) and takes the root of
    // the double product (0x2a3d4-0x2a3f5).
    if jump_step && ps.origin.z > down_o.z {
        let left = f64::from(peak) - f64::from(ps.origin.z);
        let left = match world.world.arith() {
            crate::patch::Arith::Kept => left,
            _ => f64::from(left as f32),
        };
        ps.velocity.z = if left < f64::from(0.1f32) {
            0.0
        } else if linux {
            ps.velocity
                .z
                .min(((left + left) * f64::from(ps.gravity)).sqrt() as f32)
        } else {
            let left = left as f32;
            ps.velocity.z.min((2.0 * left * ps.gravity).sqrt())
        };
    }
    // The step-up and the snap both reach this, and so does a reverted step:
    // retail's tail is past every arm of the move (@0x35659), so everything
    // the entry gate let through announces its step.
    // 1.5 gates the tail, and the prone check with it, on the ground plane
    // the move started on (0x2a46f).
    let tail = if v15 { ground_plane } else { through };
    if !tail {
        return false;
    }
    if let Some(events) = events {
        step_view(ps, events, start_o.z, down_o.z, step_size);
    }
    true
}

/// The vertical jump the step machinery added, told to the client and paid
/// for in speed. Retail measures the event against the position the plain
/// slide left (@0x3568d) and the velocity scale against the frame's start
/// (@0x35761), and skips both when the step moved the eye by half a unit or
/// less (docs/research/cod11-mantle.md, "The step event and the velocity
/// scale").
pub(super) fn step_view(
    ps: &mut PlayerState,
    events: &mut Vec<PmEvent>,
    start_z: f32,
    down_z: f32,
    step_size: f32,
) {
    let stepped = ps.origin.z - down_z;
    if stepped.abs() <= STEP_VIEW_EPS {
        return;
    }
    // rounded by a +0.5 bias and an x87 truncate toward zero (@0x356af)
    let units = (stepped + 0.5) as i32;
    if units == 0 {
        return;
    }
    let units = units.clamp(STEP_VIEW_MIN, STEP_VIEW_MAX);
    events.push(PmEvent {
        event: EV_STEP_VIEW,
        parm: units + STEP_VIEW_BIAS,
    });
    ps.velocity *=
        STEP_SCALE_BASE + STEP_SCALE_GAIN * (1.0 - (ps.origin.z - start_z).abs() / step_size);
    // 0x3579c-0x3585c: a step of more than 3 on the ground pushes the bob
    // cycle on by 7 plus 1.25 per two units (at most 4), truncated, and
    // may land a footstep.
    if units.abs() > 3 && (ps.on_ground || ps.spectator) && footsteps_audible(ps) {
        let old = ps.bob_cycle;
        let push = (units.abs() / 2).min(4) as f32 * 1.25 + 7.0;
        ps.bob_cycle = ((f32::from(old) + push) as i32 & 0xff) as u8;
        ground_step_event(ps, old, ps.bob_cycle, false, true, events);
    }
}
