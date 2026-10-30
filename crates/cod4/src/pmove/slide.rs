//! `bg_slidemove`: the clip, the slide and the step
//! (docs/research/cod4-pmove.md 5).

use glam::Vec3;

use super::fpu::{self, d};
use super::{ENTITYNUM_NONE, MAX_CLIENTS, Pml, Pmove, pm_type, pmf};

/// `EQUAL_EPSILON`: the clip's 0.001.
const OVERCLIP: f32 = f32::from_bits(0x3a83_126f);
/// A plane this close to one already met is the same (a double 0.999).
const SAME_PLANE: f64 = 0.999;
/// A velocity meeting a plane under this is clipped.
const MEETS: f32 = f32::from_bits(0x3dcc_cccd);
/// The down pass's reach past the step on the ground.
const GROUND_SNAP: f32 = 9.0;
/// A down pass landing on an unwalkable normal under this keeps the plain
/// slide.
const STEP_DOWN_MIN_NORMAL: f32 = f32::from_bits(0x3e99_999a);
/// A ground plane flatter than this does not try the step without a bump.
const STEP_FLAT_NORMAL: f32 = f32::from_bits(0x3f66_6666);
/// The step's revert slack.
const STEP_REVERT_EPS: f32 = f32::from_bits(0x3a83_126f);
/// The step's velocity damp: what it keeps at a full step and the share of
/// the rest a smaller one gets back.
const STEP_SCALE_FLOOR: f32 = f32::from_bits(0x3e4c_cccc);
const STEP_SCALE_GAIN: f32 = f32::from_bits(0x3f4c_cccd);
const STEPSIZE: f32 = 18.0;
const STEPSIZE_PRONE: f32 = 10.0;
const MAX_CLIP_PLANES: usize = 8;

/// `PM_ClipVelocity`: `in - normal * (dot - |dot| * 0.001)`.
pub fn clip_velocity(v: Vec3, normal: Vec3) -> Vec3 {
    let dot = fpu::dot(v, normal);
    let back = -(dot - dot.abs() * d(OVERCLIP));
    fpu::ma_kept(v, back, normal)
}

/// `PM_ProjectVelocity` (0x8056242): the velocity turned onto the plane
/// without changing its length. The flat part stays and z becomes the
/// plane's at that flat move; the length's ratio, old over new, then
/// scales all three. A plane within 0.001 of vertical (0x8209640) or a
/// flat-still velocity passes through, and so does a projection that
/// would lengthen a falling or level velocity going up the plane. The new
/// z and the ratio stay on the x87 stack; each output is stored once.
pub(crate) fn project_velocity(v: Vec3, normal: Vec3) -> Vec3 {
    let (x, y, z) = (d(v.x), d(v.y), d(v.z));
    let flat_sq = x * x + y * y;
    if normal.z.abs() < OVERCLIP || flat_sq == 0.0 {
        return v;
    }
    let plane_z = -(d(normal.x) * x + d(normal.y) * y) / d(normal.z);
    let ratio = ((z * z + flat_sq) / (plane_z * plane_z + flat_sq)).sqrt();
    if ratio < 1.0 || plane_z < 0.0 || v.z > 0.0 {
        Vec3::new(
            (x * ratio) as f32,
            (y * ratio) as f32,
            (plane_z * ratio) as f32,
        )
    } else {
        v
    }
}

/// `PM_PermuteRestrictiveClipPlanes`: the planes' indices in ascending
/// order of `velocity . plane` (each dot stored), an insertion sort that
/// puts a tie ahead of the earlier plane; returns the order and the
/// smallest dot.
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

/// `PM_SlideMove`: four bumps along the velocity, each hit placed by the
/// lerp; returns whether anything was met.
pub(crate) fn slide_move(pm: &mut Pmove, pml: &mut Pml, gravity: bool) -> bool {
    let mut primal = pm.ps.velocity;
    let mut end_velocity = pm.ps.velocity;
    if gravity {
        end_velocity.z = (d(end_velocity.z) - f64::from(pm.ps.gravity) * d(pml.frametime)) as f32;
        pm.ps.velocity.z = ((d(pm.ps.velocity.z) + d(end_velocity.z)) * 0.5) as f32;
        primal.z = end_velocity.z;
        if pml.ground_plane {
            pm.ps.velocity = clip_velocity(pm.ps.velocity, pml.ground_trace.normal);
        }
    }
    let mut time_left = d(pml.frametime);
    let mut planes: Vec<Vec3> = Vec::with_capacity(MAX_CLIP_PLANES + 1);
    if pml.ground_plane {
        planes.push(pml.ground_trace.normal);
    }
    planes.push(fpu::normalize_to(pm.ps.velocity).0);

    let mut bumps = 0;
    while bumps < 4 {
        let end = fpu::ma_kept(pm.ps.origin, time_left, pm.ps.velocity);
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
        time_left -= time_left * d(t.fraction);
        if planes.len() >= MAX_CLIP_PLANES {
            pm.ps.velocity = Vec3::ZERO;
            return true;
        }
        if planes.iter().any(|p| fpu::dot(t.normal, *p) > SAME_PLANE) {
            // A plane met before: clip against it and nudge off it.
            let v = clip_velocity(pm.ps.velocity, t.normal);
            pm.ps.velocity = t.normal + v;
        } else {
            planes.push(t.normal);
            let (order, least) = permute_planes(pm.ps.velocity, &planes);
            if f64::from(least) < d(MEETS) {
                if pml.impact_speed < -least {
                    pml.impact_speed = -least;
                }
                let first = planes[order[0]];
                let mut clip = clip_velocity(pm.ps.velocity, first);
                let mut end_clip = clip_velocity(end_velocity, first);
                for j in 1..planes.len() {
                    let pj = planes[order[j]];
                    if fpu::dot(clip, pj) >= f64::from(MEETS) {
                        continue;
                    }
                    clip = clip_velocity(clip, pj);
                    end_clip = clip_velocity(end_clip, pj);
                    if fpu::dot(clip, first) >= 0.0 {
                        continue;
                    }
                    // The crease of the two planes.
                    let (dir, _) = fpu::normalize(vcod_common::x87::cross(first, pj));
                    clip = fpu::scale_kept(dir, fpu::dot(dir, pm.ps.velocity));
                    end_clip = fpu::scale_kept(dir, fpu::dot(dir, end_velocity));
                    // A third plane met too: stop dead.
                    if (1..planes.len())
                        .any(|k| k != j && fpu::dot(clip, planes[order[k]]) < f64::from(MEETS))
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
    if pm.ps.pm_time != 0 {
        pm.ps.velocity = primal;
    }
    bumps != 0
}

/// `Jump_ClearState`.
pub(crate) fn jump_clear(ps: &mut super::PlayerState) {
    ps.pm_flags &= !pmf::JUMPING;
    ps.jump_origin_z = 0.0;
}

/// `Jump_GetStepHeight` (0x805072a): how far a jumping body that met
/// something may step from `from_z`, its slide's start. None once it is at
/// or over its jump's top (`jumpOriginZ + jump_height`, kept); else
/// `jump_stepSize`, or the room left to the top when that is less.
fn jump_step(ps: &super::PlayerState, from_z: f32, rules: &super::Rules) -> Option<f32> {
    let top = d(ps.jump_origin_z) + d(rules.jump_height);
    let from = d(from_z);
    if top <= from {
        return None;
    }
    let full = rules.jump_step_size;
    Some(if d(full) + from > top {
        (top - from) as f32
    } else {
        full
    })
}

/// `Jump_ClampVelocity`: a rising step holds vz to what reaches the jump's
/// height, none within 0.1 of it.
fn jump_clamp_velocity(ps: &mut super::PlayerState, from_z: f32, rules: &super::Rules) {
    if d(ps.origin.z) - d(from_z) <= 0.0 {
        return;
    }
    let left = (d(ps.jump_origin_z) + d(rules.jump_height) - d(ps.origin.z)) as f32;
    if f64::from(left) < d(f32::from_bits(0x3dcc_cccd)) {
        ps.velocity.z = 0.0;
    } else {
        let v = fpu::sqrt(f64::from(ps.gravity) * (d(left) + d(left)));
        if v < ps.velocity.z {
            ps.velocity.z = v;
        }
    }
}

/// `Jump_IsPlayerAboveMax`.
fn jump_above_max(ps: &super::PlayerState, rules: &super::Rules) -> bool {
    d(ps.origin.z) >= d(ps.jump_origin_z) + d(rules.jump_height)
}

/// Where a move stood at one point of the step: origin and velocity.
#[derive(Clone, Copy)]
struct Spot {
    origin: Vec3,
    velocity: Vec3,
}

impl Spot {
    fn of(ps: &super::PlayerState) -> Self {
        Spot {
            origin: ps.origin,
            velocity: ps.velocity,
        }
    }

    fn restore(self, ps: &mut super::PlayerState) {
        ps.origin = self.origin;
        ps.velocity = self.velocity;
    }
}

/// `PM_StepSlideMove` (0x805e930): the slide, then the same move tried
/// again from up to a step higher and set back down, kept only when it
/// went further along the entry velocity (cod4-pmove.md 5).
///
/// - Grounded means on a ground plane and not on a ladder. A climber's
///   jump is cleared, and so is a jump whose penalty runs once airborne.
/// - In the air the step is tried only by a jump that bumped below its top
///   ([`jump_step`]; under a unit of room nothing more happens) or by a
///   climber rising; otherwise the slide stands.
/// - The lift is tried when the slide bumped or the ground is steeper than
///   0.9 (0x820df28): a trace up the step plus a unit, the lift what it
///   cleared less that unit, kept on the stack; under a unit there is none.
/// - The down pass drops the lift, and 9 more on the ground (0x820dee4),
///   in one store. It keeps the plain slide on a client or on a face
///   neither walkable nor steeper than 0.3 (0x8209684); on a miss the
///   lift alone comes off; on a hit the body lands there with its
///   velocity projected onto the face.
/// - A step that gained no more than 0.001 over the slide along the entry
///   velocity, or a jump's step over its top, falls back to the slide;
///   grounded, that slide is then snapped up to 9 down and clipped. The
///   progress test's float model is cod4-pmove.md 1.
/// - A jump's step clamps its rise; a grounded live body that may lie
///   where it ended gets the eye's step ([`step_view`]).
pub(crate) fn step_slide_move(pm: &mut Pmove, pml: &mut Pml, gravity: bool) {
    let jumping_flag = |ps: &super::PlayerState| ps.pm_flags & pmf::JUMPING != 0;
    let grounded = pm.ps.pm_flags & pmf::LADDER == 0 && pml.ground_plane;
    if pm.ps.pm_flags & pmf::LADDER != 0
        || (!pml.ground_plane && jumping_flag(pm.ps) && pm.ps.pm_time != 0)
    {
        jump_clear(pm.ps);
    }
    let start = Spot::of(pm.ps);
    let bumped = slide_move(pm, pml, gravity);

    let mut reach = if pm.ps.pm_flags & pmf::PRONE != 0 {
        STEPSIZE_PRONE
    } else {
        STEPSIZE
    };
    let mut jump_stepping = false;
    if pm.ps.ground_entity_num == ENTITYNUM_NONE {
        if jumping_flag(pm.ps) && pm.ps.pm_time != 0 {
            jump_clear(pm.ps);
        }
        let jump_room = (bumped && jumping_flag(pm.ps))
            .then(|| jump_step(pm.ps, start.origin.z, &pm.rules))
            .flatten();
        match jump_room {
            Some(room) if room < 1.0 => return,
            Some(room) => {
                reach = room;
                jump_stepping = true;
            }
            None if pm.ps.pm_flags & pmf::LADDER != 0 && pm.ps.velocity.z > 0.0 => {}
            None => return,
        }
    }
    let slid = Spot::of(pm.ps);

    let steep = pml.ground_plane && pml.ground_trace.normal.z < STEP_FLAT_NORMAL;
    let lift = if bumped || steep {
        lift_and_slide(pm, pml, gravity, start, reach)
    } else {
        0.0
    };
    if (grounded || lift != 0.0) && !set_down(pm, slid, lift, grounded) {
        return;
    }

    let no_gain = slide_did_as_well(pm.ps.origin, start, slid.origin);
    if no_gain || (jump_stepping && jump_above_max(pm.ps, &pm.rules)) {
        slid.restore(pm.ps);
        if grounded {
            snap_to_ground(pm);
        }
    }
    if jump_stepping {
        jump_clamp_velocity(pm.ps, slid.origin.z, &pm.rules);
    }
    if grounded && pm.ps.pm_type < pm_type::DEAD && verify_prone(pm, start.origin, start.velocity) {
        step_view(pm, pml, slid.origin.z, start.origin.z, reach);
    }
}

/// The step's lift: up from the slide's start by at most `reach`, then the
/// slide again from there with the start's velocity. Returns the lift, 0
/// when it is under a unit (the body is then left where the slide put it).
fn lift_and_slide(pm: &mut Pmove, pml: &mut Pml, gravity: bool, start: Spot, reach: f32) -> f64 {
    let span = d(reach) + 1.0;
    let o = start.origin;
    let top = Vec3::new(o.x, o.y, (span + d(o.z)) as f32);
    let lift = span * d(pm.trace(o, top).fraction) - 1.0;
    if lift < 1.0 {
        return 0.0;
    }
    pm.ps.origin = Vec3::new(o.x, o.y, (d(o.z) + lift) as f32);
    pm.ps.velocity = start.velocity;
    slide_move(pm, pml, gravity);
    lift
}

/// The step's down pass. Returns false when the move ends at the slide
/// (`slid` restored, nothing more to do).
fn set_down(pm: &mut Pmove, slid: Spot, lift: f64, grounded: bool) -> bool {
    let o = pm.ps.origin;
    let snap = if grounded { d(GROUND_SNAP) } else { 0.0 };
    let below = Vec3::new(o.x, o.y, (d(o.z) - lift - snap) as f32);
    let t = pm.trace(o, below);
    let lands_on_client = (t.entity_num as i32) < MAX_CLIENTS;
    let too_steep = t.fraction < 1.0 && !t.walkable && t.normal.z < STEP_DOWN_MIN_NORMAL;
    if lands_on_client || too_steep {
        slid.restore(pm.ps);
        return false;
    }
    if t.fraction < 1.0 {
        pm.ps.origin = fpu::lerp(o, below, t.fraction);
        pm.ps.velocity = project_velocity(pm.ps.velocity, t.normal);
    } else if lift != 0.0 {
        pm.ps.origin.z = (d(o.z) - lift) as f32;
    }
    true
}

/// Whether the plain slide to `slid` went, within 0.001, as far along the
/// entry velocity as the move from `start` to `now`. The step's dot stays
/// on the stack; the slide's deltas and products are stored
/// (0x805eb63..0x805ebe5).
fn slide_did_as_well(now: Vec3, start: Spot, slid: Vec3) -> bool {
    let v = start.velocity;
    let s = start.origin;
    let stepped = (d(now.x) - d(s.x)) * d(v.x) + (d(now.y) - d(s.y)) * d(v.y);
    let flat = slid - s;
    let along = |a: f32, b: f32| d((d(a) * d(b)) as f32);
    let plain = along(v.x, flat.x) + along(v.y, flat.y);
    stepped <= plain + d(STEP_REVERT_EPS)
}

/// The fallback slide on the ground: dropped up to 9 onto what is below
/// and clipped against it.
fn snap_to_ground(pm: &mut Pmove) {
    let o = pm.ps.origin;
    let below = Vec3::new(o.x, o.y, (d(o.z) - d(GROUND_SNAP)) as f32);
    let t = pm.trace(o, below);
    if t.fraction < 1.0 {
        pm.ps.origin = fpu::lerp(o, below, t.fraction);
        pm.ps.velocity = clip_velocity(pm.ps.velocity, t.normal);
    }
}

/// `PM_VerifyPronePosition`: a prone body the step put where it cannot lie
/// goes back to where the step began.
fn verify_prone(pm: &mut Pmove, start_o: Vec3, start_v: Vec3) -> bool {
    if pm.ps.pm_flags & pmf::PRONE == 0 {
        return true;
    }
    let ok = super::stance::check_prone(
        pm,
        super::stance::ProneCheck {
            origin: pm.ps.origin,
            size: 15.0,
            height: 30.0,
            yaw: pm.ps.prone_direction,
            already_prone: true,
            on_ground: true,
            walkable: true,
            feet_dist: super::stance::PRONE_FEET_DIST,
            outputs: true,
        },
    );
    if !ok {
        pm.ps.origin = start_o;
        pm.ps.velocity = start_v;
    }
    ok
}

/// The step's tail (0x805ec20..0x805edc3). The eye's rise over the
/// slide (kept) counts past half a unit and rounded to the nearest whole
/// one as `floor(rise + 0.5)`; nothing happens when that is 0. Then the
/// eye's step goes to [`Pmove::view_steps`] (1.7 sends no view event, the
/// client reads `pmove_t.viewChange`), the velocity keeps
/// `0.2 + 0.8 * (1 - |climb| / reach)` of itself (0x820e254, 0x8209694;
/// the factor kept, the climb from the start stored), `xyspeed` is
/// measured again, and a whole rise past 3 on the ground plays a footstep
/// with the bob pushed 7 + 1.25 per two units, at most four
/// (0x820e25c, 0x820e258).
fn step_view(pm: &mut Pmove, pml: &Pml, slid_z: f32, start_z: f32, reach: f32) {
    let rise = d(pm.ps.origin.z) - d(slid_z);
    if (rise.abs() as f32) <= 0.5 {
        return;
    }
    let whole = (rise + 0.5).floor() as i32;
    if whole == 0 {
        return;
    }
    pm.view_steps.push((pm.ps.command_time, rise as f32));
    let climb = (d(pm.ps.origin.z) - d(start_z)).abs() as f32;
    let left = 1.0 - d(climb) / d(reach);
    let keep = left * d(STEP_SCALE_GAIN) + d(STEP_SCALE_FLOOR);
    pm.ps.velocity = fpu::scale_kept(pm.ps.velocity, keep);
    let (vx, vy) = (d(pm.ps.velocity.x), d(pm.ps.velocity.y));
    pm.xyspeed = (vx * vx + vy * vy).sqrt() as f32;
    let steps = whole.abs();
    if steps <= 3
        || pm.ps.ground_entity_num == ENTITYNUM_NONE
        || !super::footsteps::should_make_footsteps(pm)
    {
        return;
    }
    let push = 7.0 + 1.25 * f64::from((steps / 2).min(4));
    let old = pm.ps.bob_cycle;
    pm.ps.bob_cycle = ((f64::from(old) + push) as f32 as i32) & 0xff;
    super::footsteps::footstep(pm, pml, old, pm.ps.bob_cycle, true);
}

#[cfg(test)]
#[path = "jump_pins.rs"]
mod pins;
