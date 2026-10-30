//! `bg_slidemove`: the clip, the slide and the step
//! (docs/research/cod2-movement.md 7).

use glam::Vec3;

use super::fpu::{self, d};
use super::{ENTITYNUM_NONE, Pml, Pmove, ev, pm_type, pmf};

/// rodata 0x8154ee0: the clip's 0.001.
const OVERCLIP: f32 = f32::from_bits(0x3a83_126f);
/// rodata 0x815531c: a plane this close to one already met is the same.
const SAME_PLANE: f32 = f32::from_bits(0x3f7f_be77);
/// rodata 0x8155320: a velocity meeting a plane under this is clipped.
const MEETS: f32 = f32::from_bits(0x3dcc_cccd);
/// rodata 0x8155324: the down pass's reach past the step on the ground.
const GROUND_SNAP: f32 = 9.0;
/// rodata 0x8155328: a down pass landing on a normal under this keeps the
/// plain slide.
const STEP_DOWN_MIN_NORMAL: f32 = f32::from_bits(0x3e99_999a);
/// rodata 0x815532c: the step's revert slack.
const STEP_REVERT_EPS: f32 = f32::from_bits(0x3a83_126f);
/// rodata 0x8155338 and 0x8155334: the step's velocity damp,
/// `0.19999999 + 0.8 * (1 - |dz| / step)`.
const STEP_SCALE_BASE: f32 = f32::from_bits(0x3e4c_cccc);
const STEP_SCALE_GAIN: f32 = f32::from_bits(0x3f4c_cccd);
const STEPSIZE: f32 = 18.0;
const STEPSIZE_PRONE: f32 = 10.0;
const MAX_CLIP_PLANES: usize = 8;

/// `PM_ClipVelocity` (0x80e14d4): `in - normal * (dot - |dot| * 0.001)`,
/// the dot stored first. Into a plane that is Q3's `dot * 1.001`; out of
/// one it is `dot * 0.999`.
pub fn clip_velocity(v: Vec3, normal: Vec3) -> Vec3 {
    let dot = fpu::dot(v, normal) as f32;
    let back = (-(d(dot) - d(OVERCLIP) * d(dot.abs()))) as f32;
    fpu::ma(v, back, normal)
}

/// `PM_PermuteRestrictiveClipPlanes` (0x80e9b7c): the planes' indices in
/// ascending order of `velocity . plane` (each dot stored), an insertion
/// sort that puts a tie ahead of the earlier plane; returns the order and
/// the smallest dot.
pub(crate) fn permute_planes(v: Vec3, planes: &[Vec3]) -> ([usize; MAX_CLIP_PLANES + 1], f32) {
    let mut dots = [0f32; MAX_CLIP_PLANES + 1];
    let mut order = [0usize; MAX_CLIP_PLANES + 1];
    for i in 0..planes.len() {
        dots[i] = fpu::dot(v, planes[i]) as f32;
        let mut j = i;
        while j != 0 && dots[i] <= dots[order[j - 1]] {
            order[j] = order[j - 1];
            j -= 1;
        }
        order[j] = i;
    }
    (order, dots[order[0]])
}

/// `PM_SlideMove` (0x80e9c42): four bumps along the velocity, each hit
/// placed by the lerp; returns whether anything was met.
pub(crate) fn slide_move(pm: &mut Pmove, pml: &mut Pml, gravity: bool) -> bool {
    let mut primal = pm.ps.velocity;
    let mut end_velocity = pm.ps.velocity;
    if gravity {
        // The gravity averaged over the frame, the end velocity kept.
        end_velocity.z = (d(end_velocity.z) - f64::from(pm.ps.gravity) * d(pml.frametime)) as f32;
        pm.ps.velocity.z = (0.5 * (d(pm.ps.velocity.z) + d(end_velocity.z))) as f32;
        primal.z = end_velocity.z;
        if pml.ground_plane {
            pm.ps.velocity = clip_velocity(pm.ps.velocity, pml.ground_trace.normal);
        }
    }
    let mut time_left = pml.frametime;
    let mut planes: Vec<Vec3> = Vec::with_capacity(MAX_CLIP_PLANES + 1);
    if pml.ground_plane {
        planes.push(pml.ground_trace.normal);
    }
    planes.push(fpu::normalize_to(pm.ps.velocity).0);

    let mut bumps = 0;
    while bumps < 4 {
        let end = fpu::ma(pm.ps.origin, time_left, pm.ps.velocity);
        let t = pm.trace(pm.ps.origin, end);
        if t.allsolid {
            pm.ps.velocity.z = 0.0;
            return true;
        }
        if t.fraction > 0.0 {
            pm.ps.origin = fpu::lerp(pm.ps.origin, end, t.fraction);
        }
        if t.fraction == 1.0 {
            break;
        }
        pm.add_touch(t.entity_num);
        time_left = (d(time_left) - d(time_left) * d(t.fraction)) as f32;
        if planes.len() > MAX_CLIP_PLANES - 1 {
            pm.ps.velocity = Vec3::ZERO;
            return true;
        }
        // A plane met before: clip against it and nudge off it.
        if planes
            .iter()
            .any(|p| fpu::dot(t.normal, *p) > d(SAME_PLANE))
        {
            let v = clip_velocity(pm.ps.velocity, t.normal);
            pm.ps.velocity = t.normal + v;
        } else {
            planes.push(t.normal);
            let (order, least) = permute_planes(pm.ps.velocity, &planes);
            if least < MEETS {
                if pml.impact_speed < -least {
                    pml.impact_speed = -least;
                }
                let first = planes[order[0]];
                let mut clip = clip_velocity(pm.ps.velocity, first);
                let mut end_clip = clip_velocity(end_velocity, first);
                for j in 1..planes.len() {
                    let pj = planes[order[j]];
                    if fpu::dot(clip, pj) >= d(MEETS) {
                        continue;
                    }
                    clip = clip_velocity(clip, pj);
                    end_clip = clip_velocity(end_clip, pj);
                    if fpu::dot(clip, first) >= 0.0 {
                        continue;
                    }
                    // The crease of the two planes.
                    let (dir, _) = fpu::normalize(vcod_common::x87::cross(first, pj));
                    clip = fpu::scale(dir, fpu::dot(dir, pm.ps.velocity) as f32);
                    end_clip = fpu::scale(dir, fpu::dot(dir, end_velocity) as f32);
                    // A third plane met too: stop dead.
                    if (1..planes.len())
                        .any(|k| k != j && fpu::dot(clip, planes[order[k]]) < d(MEETS))
                    {
                        pm.ps.velocity = Vec3::ZERO;
                        return true;
                    }
                }
                pm.ps.velocity = clip;
                end_velocity = end_clip;
            }
        }
        bumps += 1;
    }
    if gravity {
        pm.ps.velocity = end_velocity;
    }
    // A running `pm_time` hands back the entry velocity, its z the
    // gravity's end when gravity ran.
    if pm.ps.pm_time != 0 {
        pm.ps.velocity = primal;
    }
    bumps != 0
}

/// `Jump_ClearState` (0x80dc718).
pub(crate) fn jump_clear(ps: &mut super::PlayerState) {
    ps.pm_flags &= !pmf::JUMPING;
    ps.jump_origin_z = 0.0;
}

/// `Jump_GetStepHeight` (0x80dc73a): a jump below its height steps at most
/// `jump_stepSize`, and never past the height.
fn jump_step_height(ps: &super::PlayerState, start_z: f32, rules: &super::Rules) -> Option<f32> {
    let top = d(ps.jump_origin_z) + d(rules.jump_height);
    if d(start_z) >= top {
        return None;
    }
    let mut step = rules.jump_step_size;
    if top < d(start_z) + d(step) {
        step = (top - d(start_z)) as f32;
    }
    Some(step)
}

/// `Jump_ClampVelocity` (0x80dc95c): a rising step holds vz to what reaches
/// the jump's height, none within 0.1 of it.
fn jump_clamp_velocity(ps: &mut super::PlayerState, from_z: f32, rules: &super::Rules) {
    if (d(ps.origin.z) - d(from_z)) as f32 <= 0.0 {
        return;
    }
    let left = (d(ps.jump_origin_z) + d(rules.jump_height) - d(ps.origin.z)) as f32;
    if left < f32::from_bits(0x3dcc_cccd) {
        ps.velocity.z = 0.0;
    } else {
        let v = fpu::sqrt(f64::from(ps.gravity) * (d(left) + d(left)));
        if v < ps.velocity.z {
            ps.velocity.z = v;
        }
    }
}

/// `PM_StepSlideMove` (0x80ea3f4).
pub(crate) fn step_slide_move(pm: &mut Pmove, pml: &mut Pml, gravity: bool) {
    let mut step_up = 0f32;
    let mut jump_step = false;
    let grounded = if pm.ps.pm_flags & pmf::LADDER != 0 {
        jump_clear(pm.ps);
        false
    } else if pml.ground_plane {
        true
    } else {
        if pm.ps.pm_flags & pmf::JUMPING != 0 && pm.ps.pm_time != 0 {
            jump_clear(pm.ps);
        }
        false
    };
    let start_o = pm.ps.origin;
    let start_v = pm.ps.velocity;
    let slid = slide_move(pm, pml, gravity);
    let mut step = if pm.ps.pm_flags & pmf::PRONE != 0 {
        STEPSIZE_PRONE
    } else {
        STEPSIZE
    };
    if pm.ps.ground_entity_num == ENTITYNUM_NONE {
        if pm.ps.pm_flags & pmf::JUMPING != 0 && pm.ps.pm_time != 0 {
            jump_clear(pm.ps);
        }
        if slid
            && pm.ps.pm_flags & pmf::JUMPING != 0
            && let Some(s) = jump_step_height(pm.ps, start_o.z, &pm.rules)
        {
            step = s;
            if step < 1.0 {
                return;
            }
            jump_step = true;
        }
        if !jump_step && (pm.ps.pm_flags & pmf::LADDER == 0 || pm.ps.velocity.z <= 0.0) {
            return;
        }
    }
    let down_o = pm.ps.origin;
    let down_v = pm.ps.velocity;
    let slide_flat = down_o - start_o;
    if slid {
        let mut up = start_o;
        up.z = (d(up.z) + d(step) + 1.0) as f32;
        let t = pm.trace(start_o, up);
        step_up = ((d(step) + 1.0) * d(t.fraction) - 1.0) as f32;
        if step_up < 1.0 {
            step_up = 0.0;
        } else {
            pm.ps.origin = Vec3::new(up.x, up.y, start_o.z + step_up);
            pm.ps.velocity = start_v;
            slide_move(pm, pml, gravity);
        }
    }
    if grounded || step_up != 0.0 {
        let mut down = pm.ps.origin;
        down.z -= step_up;
        if grounded {
            down.z -= GROUND_SNAP;
        }
        let t = pm.trace(pm.ps.origin, down);
        if t.entity_num < 64 {
            pm.ps.origin = down_o;
            pm.ps.velocity = down_v;
            return;
        }
        if t.fraction < 1.0 {
            if t.normal.z < STEP_DOWN_MIN_NORMAL {
                pm.ps.origin = down_o;
                pm.ps.velocity = down_v;
                return;
            }
            pm.ps.origin = fpu::lerp(pm.ps.origin, down, t.fraction);
            pm.ps.velocity = clip_velocity(pm.ps.velocity, t.normal);
        } else if step_up != 0.0 {
            pm.ps.origin.z -= step_up;
        }
    }
    // The step only stands when it went further along the velocity.
    let step_flat = pm.ps.origin - start_o;
    let along = fpu::dot2(step_flat, pm.ps.velocity) as f32;
    let plain = fpu::dot2(slide_flat, pm.ps.velocity);
    let reverted = d(along) <= d(STEP_REVERT_EPS) + plain;
    if reverted || (jump_step && jump_above_max(pm.ps, &pm.rules)) {
        pm.ps.origin = down_o;
        pm.ps.velocity = down_v;
        if grounded {
            let mut down = pm.ps.origin;
            down.z -= GROUND_SNAP;
            let t = pm.trace(pm.ps.origin, down);
            if t.fraction < 1.0 {
                pm.ps.origin = fpu::lerp(pm.ps.origin, down, t.fraction);
                pm.ps.velocity = clip_velocity(pm.ps.velocity, t.normal);
            }
        }
    }
    if jump_step {
        jump_clamp_velocity(pm.ps, down_o.z, &pm.rules);
    }
    if grounded && pm.ps.pm_type < pm_type::DEAD && verify_prone(pm, start_o, start_v) {
        step_view(pm, pml, down_o.z, start_o.z, step);
    }
}

/// `Jump_IsPlayerAboveMax` (0x80dc7c8).
fn jump_above_max(ps: &super::PlayerState, rules: &super::Rules) -> bool {
    d(ps.jump_origin_z) + d(rules.jump_height) <= d(ps.origin.z)
}

/// `PM_VerifyPronePosition` (0x80e9a7c): a prone body the step put where it
/// cannot lie goes back to where the step began.
fn verify_prone(pm: &mut Pmove, start_o: Vec3, start_v: Vec3) -> bool {
    if pm.ps.pm_flags & pmf::PRONE == 0 {
        return true;
    }
    let ok = super::stance::check_prone(
        pm,
        super::stance::ProneCheck {
            origin: pm.ps.origin,
            height: 30.0,
            yaw: pm.ps.prone_direction,
            already_prone: true,
            on_ground: true,
            ground_normal: None,
            length: super::stance::PRONE_LENGTH,
            outputs: true,
        },
    );
    if !ok {
        pm.ps.origin = start_o;
        pm.ps.velocity = start_v;
    }
    ok
}

/// The step's tail: over half a unit of eye movement, `EV_STEP_VIEW` with the
/// rounded step clamped to -16..24 plus 128, the velocity damped by how much
/// of the step was taken, and a footstep for a step over 3 on the ground.
fn step_view(pm: &mut Pmove, pml: &Pml, down_z: f32, start_z: f32, step: f32) {
    let dz = pm.ps.origin.z - down_z;
    if dz.abs() <= 0.5 {
        return;
    }
    // 0x80eae0c: `floor(0.5 + x)`.
    let delta = (0.5 + d(dz)).floor() as i32;
    if delta == 0 {
        return;
    }
    let delta = delta.clamp(-16, 24);
    pm.ps.add_event(ev::STEP_VIEW, delta + 128);
    let taken = (pm.ps.origin.z - start_z).abs();
    let k = (d(STEP_SCALE_BASE) + d(STEP_SCALE_GAIN) * (1.0 - d(taken) / d(step))) as f32;
    pm.ps.velocity = fpu::scale(pm.ps.velocity, k);
    if delta.abs() > 3
        && pm.ps.ground_entity_num != ENTITYNUM_NONE
        && super::footsteps::should_make_footsteps(pm)
    {
        let n = (delta.abs() / 2).min(4);
        let old = pm.ps.bob_cycle;
        // rodata 0x8155340 and 0x815533c, truncated under control word 0xc00.
        pm.ps.bob_cycle = ((f64::from(old) + 7.0 + 1.25 * f64::from(n)) as i32) & 0xff;
        super::footsteps::footstep(pm, pml, old, pm.ps.bob_cycle, true);
    }
}
