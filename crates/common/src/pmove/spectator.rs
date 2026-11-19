//! `PM_FlyMove`: the spectator's free flight.

use super::frame::begin_move;
use super::slide::step_slide_move;
use super::state::{PlayerState, PmEvent};
use super::walk::{accelerate_stored, friction_stored, view_vectors};
use super::{MAX_FRAME_MS, PM_SPECTATOR_FRICTION};
use crate::movetrace::{MASK_SPECTATOR, MoveWorld};
use crate::x87;
use glam::Vec3;

/// A CoD 1.1 spectator, `PM_FlyMove` (0x2ee6c): friction always, then the
/// fly accelerate, then `PM_StepSlideMove` without gravity in the
/// spectator's own hull under [`MASK_SPECTATOR`] (docs/protocol-1.1.md,
/// "Spectator"). The axes are the usercmd's, -127..127, and `wbuttons` its
/// button byte, whose lean bits fly a spectator down and up. With no world
/// it integrates straight off the velocity: a host with no map loaded.
pub fn spectator_move(
    ps: &mut PlayerState,
    world: Option<&MoveWorld>,
    forward: f32,
    right: f32,
    up: f32,
    wbuttons: u8,
    dt: f32,
) -> Vec<PmEvent> {
    let dt = dt.min(MAX_FRAME_MS / 1000.0);
    begin_move(ps);
    ps.on_ground = false;
    let arith = world.map_or(crate::patch::Arith::Kept, |w| w.world.arith());
    if ps.rules.v15() {
        spectator_friction_stored(ps, dt);
        fly_accelerate_stored(ps, forward, right, up, wbuttons, dt, arith);
        return spectator_slide(ps, world, dt);
    }
    // `PM_Friction` (0x2e460): a spectator is never walking, so only its
    // `pm_type` 4 arm (0x2e56d) drops speed, by the speed itself times 5
    // (0x70874), with no stop-speed floor.
    let speed = x87::dot(ps.velocity, ps.velocity).sqrt() as f32;
    if speed < 1.0 {
        ps.velocity.x = 0.0;
        ps.velocity.y = 0.0;
    } else {
        let speed = f64::from(speed);
        let drop = speed * f64::from(PM_SPECTATOR_FRICTION) * f64::from(dt);
        let scale = (speed - drop).max(0.0) / speed;
        ps.velocity = (ps.velocity.as_dvec3() * scale).as_vec3();
    }
    fly_accelerate(ps, forward, right, up, wbuttons, dt, arith);
    spectator_slide(ps, world, dt)
}

/// The fly move's `PM_StepSlideMove` in the spectator's hull, gravity off.
fn spectator_slide(ps: &mut PlayerState, world: Option<&MoveWorld>, dt: f32) -> Vec<PmEvent> {
    let mut events = Vec::new();
    match world {
        Some(world) => {
            ps.spectator = true;
            step_slide_move(ps, world, dt, false, MASK_SPECTATOR, Some(&mut events));
            ps.spectator = false;
        }
        None => ps.origin += ps.velocity * dt,
    }
    events
}

/// `PM_FlyMove`'s wish and accelerate (0x2ee8b..0x2f026). The wish runs
/// along the full view, pitch included (`PmoveSingle`'s `AngleVectors` of
/// `ps.viewangles` at 0x34120), plus `upmove` straight up, scaled by
/// `PM_CmdScale` (0x2e5bc), so a partial key flies slower. Lean right then
/// takes 512 off the wish's z and lean left adds 256, outside the scale
/// (0x2ef19..0x2ef4a). No wish leaves the velocity alone, signed zeros
/// included. The accelerate is 8 (0x708e4) times a wish speed of at least
/// 100 (0x708e8).
fn fly_accelerate(
    ps: &mut PlayerState,
    f: f32,
    r: f32,
    u: f32,
    wbuttons: u8,
    dt: f32,
    arith: crate::patch::Arith,
) {
    let max = f.abs().max(r.abs()).max(u.abs());
    let lean_down = if wbuttons & crate::net::msg::WBUTTON_LEAN_RIGHT != 0 {
        512.0
    } else {
        0.0
    };
    let lean_up = if wbuttons & crate::net::msg::WBUTTON_LEAN_LEFT != 0 {
        256.0
    } else {
        0.0
    };
    if max == 0.0 && lean_down == 0.0 && lean_up == 0.0 {
        return;
    }
    let mut wishvel = Vec3::ZERO;
    if max != 0.0 {
        let (f, r, u) = (f64::from(f), f64::from(r), f64::from(u));
        let total = (f * f + r * r + u * u).sqrt();
        let scale = (f64::from(ps.speed) * f64::from(max) / (127.0 * total)) as f32;
        let scale = f64::from(scale);
        let (fwd, right) = view_vectors(ps, arith);
        let (fwd, right) = (fwd.as_dvec3(), right.as_dvec3());
        wishvel = (fwd * scale * f + right * scale * r).as_vec3();
        wishvel.z = (f64::from(wishvel.z) + scale * u) as f32;
    }
    // Both lean terms ride one x87 chain and round once, at the store.
    wishvel.z = ((f64::from(wishvel.z) - lean_down) + lean_up) as f32;
    let wishdir = x87::normalize(wishvel);
    let wishspeed = x87::dot(wishvel, wishvel).sqrt() as f32;
    let add = f64::from(wishspeed) - x87::dot(ps.velocity, wishdir);
    if add <= 0.0 {
        return;
    }
    let accel = (8.0 * f64::from(wishspeed.max(100.0)) * f64::from(dt)).min(add);
    ps.velocity = (ps.velocity.as_dvec3() + wishdir.as_dvec3() * accel).as_vec3();
}

/// 1.5b's and UO's `PM_Friction` for a spectator (1.5 0x1eab3, UO
/// 0x234ef): the speed sums `(vz^2 + vy^2) + vx^2` and is stored, under one
/// unit x, y and z stop, and otherwise the `pm_type` 4 term `speed *
/// pm_spectatorfriction (5) * frametime` joins the stored drop of
/// [`friction_stored`].
fn spectator_friction_stored(ps: &mut PlayerState, dt: f32) {
    let v = ps.velocity.as_dvec3();
    let speed = ((v.z * v.z + v.y * v.y) + v.x * v.x).sqrt() as f32;
    if speed < 1.0 {
        ps.velocity = Vec3::ZERO;
        return;
    }
    friction_stored(ps, speed, dt, true);
}

/// 1.5b's and UO's `PM_FlyMove` past the friction (1.5 0x1fcf9, UO
/// 0x24838): `PM_CmdScale` (UO 0x23905) stored, the wish
/// `right * scale * pml.right + forward * scale * pml.forward` plus `up *
/// scale` on z, each component stored; the lean's 512 down and 256 up only
/// while `ps.speed` is not 0 (UO 0x2492e, `ps+0x48`); then the stored
/// `VectorNormalize` and `PM_Accelerate` (UO 0x2379e) at
/// `pm_flyaccelerate` 8. The run scale `PM_CmdScale` multiplies in is 1 on
/// a spectator.
fn fly_accelerate_stored(
    ps: &mut PlayerState,
    f: f32,
    r: f32,
    u: f32,
    wbuttons: u8,
    dt: f32,
    arith: crate::patch::Arith,
) {
    let max = f.abs().max(r.abs()).max(u.abs());
    let mut wishvel = Vec3::ZERO;
    if max != 0.0 && ps.speed != 0.0 {
        let (fd, rd, ud) = (f64::from(f), f64::from(r), f64::from(u));
        let total = (ud * ud + fd * fd + rd * rd).sqrt();
        let scale = f64::from(((f64::from(max) * f64::from(ps.speed)) / (127.0 * total)) as f32);
        let (fwd, right) = view_vectors(ps, arith);
        let (fwd, right) = (fwd.as_dvec3(), right.as_dvec3());
        wishvel = (right * (rd * scale) + fwd * (fd * scale)).as_vec3();
        wishvel.z = (f64::from(wishvel.z) + ud * scale) as f32;
    }
    if ps.speed != 0.0 {
        let down = if wbuttons & crate::net::msg::WBUTTON_LEAN_RIGHT != 0 {
            512.0
        } else {
            0.0
        };
        let up = if wbuttons & crate::net::msg::WBUTTON_LEAN_LEFT != 0 {
            256.0
        } else {
            0.0
        };
        wishvel.z = ((f64::from(wishvel.z) - down) + up) as f32;
    }
    let wishspeed = f64::from(x87::dot(wishvel, wishvel) as f32).sqrt() as f32;
    let wishdir = x87::normalize_stored(wishvel);
    accelerate_stored(ps, wishdir, wishspeed, 8.0, dt);
}
