//! `PM_CheckDuck`, the eye's height, `BG_CheckProneValid`, the view angles
//! and the lean (docs/research/cod2-movement.md 10).

use glam::Vec3;

use super::anim::{self, Event};
use super::fpu::{self, d};
use super::{
    ENTITYNUM_NONE, PlayerState, Pml, Pmove, Stance, VIEW_CROUCH, VIEW_DEAD, VIEW_PRONE,
    VIEW_STAND, button, ef, ev, pm_type, pmf,
};

/// The prone body's length every fit but the swing passes (immediate
/// 0x42840000 at 0x80e4953, rodata 0x815511c), the swing's (0x8155118) and
/// the swing's divisor (0x8155114).
pub(crate) const PRONE_LENGTH: f32 = 66.0;
const PRONE_SWING_LENGTH: f32 = 45.0;
const PRONE_TURN_DIVISOR: f32 = 240.0;
pub(crate) const HEIGHT_PRONE: f32 = 30.0;
pub(crate) const HEIGHT_CROUCH: f32 = 50.0;
/// rodata 0x815512c, 0x8155130, 0x815513c: the yaw cap's soft edge, the
/// body's swing rate and the refused turn's ease back.
const PRONE_SOFT_EDGE: f32 = 5.0;
const PRONE_SWING_DEG_PER_SEC: f32 = 55.0;
const PRONE_REFUSED_EASE: f32 = f32::from_bits(0x3f7a_e148);
/// rodata 0x8155138: the torso's pitch either side of the ground's.
const PRONE_PITCHCAP: f32 = 45.0;
/// rodata 0x8155074, 0x8155078: the settle on going prone, up then down.
const PRONE_SETTLE_UP: f32 = 10.0;
const PRONE_GROUND_PROBE: f32 = 0.25;
/// rodata 0x8155060: the dead eye's rate.
const DEAD_VIEW_RATE: f32 = 180.0;
/// The lean's ramps out and back (rodata 0x8155110, 0x815510c).
const LEAN_TO_MS: f32 = 350.0;
const LEAN_FROM_MS: f32 = 280.0;
/// rodata 0x8155120: 360 / 65536.
const DEG_PER_SHORT: f32 = f32::from_bits(0x3bb4_0000);

/// The eye's curves (`.data` 0x8186780, 0x8186800, 0x8186880, 0x8186960):
/// `{percent, height, posadj}` rows; every posadj is 0 in 1.3, so the body
/// push off a changing posadj never runs and is not modelled.
const CURVE_STAND_CROUCH: &[(i32, f32)] = &[
    (0, 60.0),
    (1, 59.5),
    (4, 58.5),
    (30, 56.0),
    (80, 44.0),
    (90, 41.5),
    (95, 40.5),
    (100, 40.0),
];
const CURVE_CROUCH_STAND: &[(i32, f32)] = &[
    (0, 40.0),
    (5, 40.5),
    (10, 41.5),
    (20, 44.0),
    (70, 56.0),
    (96, 58.5),
    (99, 59.5),
    (100, 60.0),
];
const CURVE_CROUCH_PRONE: &[(i32, f32)] = &[
    (0, 40.0),
    (11, 38.0),
    (22, 33.0),
    (34, 25.0),
    (45, 16.0),
    (50, 15.0),
    (55, 16.0),
    (70, 18.0),
    (90, 17.0),
    (100, 11.0),
];
const CURVE_PRONE_CROUCH: &[(i32, f32)] = &[
    (0, 11.0),
    (5, 10.0),
    (30, 21.0),
    (50, 25.0),
    (67, 31.0),
    (83, 34.0),
    (100, 40.0),
];

/// `PM_GetViewHeightLerpTime` (0x80e3c46): 400 into prone and up out of it
/// to the crouch, 200 down to the crouch and up to standing.
pub(crate) fn lerp_duration(target: i32, down: bool) -> i32 {
    match target {
        VIEW_PRONE => 400,
        VIEW_CROUCH if !down => 400,
        _ => 200,
    }
}

fn curve(target: i32, down: bool) -> &'static [(i32, f32)] {
    match target {
        VIEW_PRONE => CURVE_CROUCH_PRONE,
        VIEW_CROUCH if !down => CURVE_PRONE_CROUCH,
        VIEW_CROUCH => CURVE_STAND_CROUCH,
        _ => CURVE_CROUCH_STAND,
    }
}

/// 0x80e3c86: the curve's height at `pct`, linear between rows, the
/// fraction stored.
fn curve_height(c: &[(i32, f32)], pct: i32) -> f32 {
    if pct == 0 {
        return c[0].1;
    }
    for w in c.windows(2) {
        let ((p0, h0), (p1, h1)) = (w[0], w[1]);
        if pct == p1 {
            return h1;
        }
        if pct < p1 {
            let f = ((pct - p0) as f32) / ((p1 - p0) as f32);
            return ((d(h1) - d(h0)) * d(f) + d(h0)) as f32;
        }
    }
    c[0].1
}

/// `PM_ViewHeightAdjust` (0x80e3eb0): the eye eases to its stance target
/// along the curves, one leg at a time, a leg reversed mid-way picking up
/// where it was; the dead eye moves at 180/s.
fn view_height_adjust(ps: &mut PlayerState, time: i32, frametime: f32) {
    let target = ps.view_height_target;
    if target == 0 || ps.view_height_current == 0.0 {
        ps.view_height_current = if ps.pm_type == pm_type::SPECTATOR {
            0.0
        } else {
            target as f32
        };
        return;
    }
    if ps.view_height_current == target as f32 && ps.view_height_lerp_time == 0 {
        return;
    }
    if !matches!(target, VIEW_PRONE | VIEW_CROUCH | VIEW_STAND) {
        ps.view_height_lerp_time = 0;
        let step = (d(DEAD_VIEW_RATE) * d(frametime)) as f32;
        let t = target as f32;
        if ps.view_height_current < t {
            ps.view_height_current = (ps.view_height_current + step).min(t);
        } else {
            ps.view_height_current = (ps.view_height_current - step).max(t);
        }
        return;
    }
    let mut pct = 0;
    if ps.view_height_lerp_time != 0 {
        let dur = lerp_duration(ps.view_height_lerp_target, ps.view_height_lerp_down);
        pct = ((time - ps.view_height_lerp_time) * 100 / dur).clamp(0, 100);
        if pct == 100 {
            ps.view_height_current = ps.view_height_lerp_target as f32;
            ps.view_height_lerp_time = 0;
        } else {
            let c = curve(ps.view_height_lerp_target, ps.view_height_lerp_down);
            ps.view_height_current = curve_height(c, pct);
        }
    }
    if ps.view_height_lerp_time == 0 {
        if ps.view_height_current != target as f32 {
            ps.view_height_lerp_time = time;
            let cur = ps.view_height_current;
            match target {
                VIEW_PRONE => {
                    ps.view_height_lerp_down = true;
                    ps.view_height_lerp_target = if cur > 40.0 { VIEW_CROUCH } else { VIEW_PRONE };
                }
                VIEW_CROUCH => {
                    ps.view_height_lerp_down = (target as f32) < cur;
                    ps.view_height_lerp_target = VIEW_CROUCH;
                }
                _ => {
                    ps.view_height_lerp_down = false;
                    ps.view_height_lerp_target = if cur < 40.0 { VIEW_CROUCH } else { VIEW_STAND };
                }
            }
        }
    } else {
        let lt = ps.view_height_lerp_target;
        let down = ps.view_height_lerp_down;
        if target != lt && ((target < lt && !down) || (lt < target && down)) {
            // Reversed mid-leg: the other way from the same height.
            pct = 100 - pct;
            ps.view_height_lerp_down = !down;
            ps.view_height_lerp_target = match (ps.view_height_lerp_down, lt) {
                (false, VIEW_PRONE) => VIEW_CROUCH,
                (false, VIEW_CROUCH) => VIEW_STAND,
                (true, VIEW_STAND) => VIEW_CROUCH,
                (true, VIEW_CROUCH) => VIEW_PRONE,
                (_, t) => t,
            };
            if pct == 100 {
                ps.view_height_current = ps.view_height_lerp_target as f32;
                ps.view_height_lerp_time = 0;
            } else {
                let dur = lerp_duration(ps.view_height_lerp_target, ps.view_height_lerp_down);
                ps.view_height_lerp_time =
                    time - (dur as f32 * f32::from_bits(0x3c23_d70a) * pct as f32) as i32;
            }
        }
    }
}

/// What [`check_prone`] is asked: `BG_CheckProneValid`'s arguments less the
/// entity, the half width (always the box's 15) and the outputs.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ProneCheck {
    pub origin: Vec3,
    pub height: f32,
    pub yaw: f32,
    pub already_prone: bool,
    pub on_ground: bool,
    pub ground_normal: Option<Vec3>,
    pub length: f32,
    /// Whether the caller passes the playerstate's three outputs; the
    /// swing's own fits pass none.
    pub outputs: bool,
}

/// `BG_CheckProneValid` (0x80e01f4): whether a body may lie at `origin`
/// facing `yaw`. The sweep runs behind the facing; the hips sample 48 out
/// (immediate 0x42400000) with a sill limit of `48 + 0.7 h`; the torso
/// pitches off the origin itself. Writes `prone_valid` on a fit.
pub(crate) fn check_prone(pm: &mut Pmove, q: ProneCheck) -> bool {
    let r = 15.0f32;
    let mask = super::MASK_DEADSOLID;
    let o = q.origin;
    let ret = |pm: &mut Pmove| -> bool {
        if q.on_ground {
            return false;
        }
        if q.outputs {
            pm.ps.prone_valid = [0.0; 3];
        }
        true
    };
    if !q.already_prone {
        let t = pm.raw_trace(
            o,
            Vec3::new(o.x, o.y, o.z + 10.0),
            Vec3::new(-r, -r, 0.0),
            Vec3::new(r, r, q.height),
            mask,
        );
        if t.allsolid {
            return false;
        }
    }
    if q.on_ground && q.ground_normal.is_some_and(|n| n.z < 0.7) {
        return false;
    }
    let half = Vec3::splat(6.0);
    let tr = |pm: &Pmove, a: Vec3, b: Vec3| pm.raw_trace(a, b, -half, half, mask);
    let mut fwd = vcod_common::x87::angle_vectors(Vec3::new(0.0, q.yaw - 180.0, 0.0))[0];
    let h = q.height - 6.0;
    let start = Vec3::new(o.x, o.y, o.z + h);
    let mut end = fpu::ma(start, q.length - 6.0, fwd);
    let mut t = tr(pm, start, end);
    let mut blocked = false;
    let mut reach = q.length;
    let sill = (48.0 + 0.7 * d(h)) as f32;
    if t.fraction < 1.0 {
        if !q.on_ground {
            return false;
        }
        blocked = true;
        reach = (6.0 + (d(q.length) - 6.0) * d(t.fraction)) as f32;
        if reach < 2.0 + r {
            return false;
        }
        if reach < sill {
            // Again 22 units higher: over a kerb or a sill. The direction
            // the later samples take is this sweep's.
            blocked = false;
            end.z += 22.0;
            let (dir, len) = fpu::normalize_to(end - start);
            fwd = dir;
            t = tr(pm, start, end);
            if t.fraction < 1.0 {
                blocked = true;
                reach = (6.0 + d(t.fraction) * d(len)) as f32;
                if reach < sill {
                    return false;
                }
            } else {
                reach = q.length;
            }
        }
    }
    let mut feet = fpu::lerp(start, end, t.fraction);
    // The hips, 48 out, dropped onto the ground.
    let hips_from = {
        let p = fpu::ma(o, 48.0, fwd);
        Vec3::new(p.x, p.y, p.z + h)
    };
    let drop = (2.5 * d(r) + d(h) - 6.0) as f32;
    let hips_to = Vec3::new(hips_from.x, hips_from.y, hips_from.z - drop);
    let t = tr(pm, hips_from, hips_to);
    if t.fraction == 1.0 {
        return ret(pm);
    }
    if t.normal.z < 0.7 {
        return false;
    }
    let hips_drop = (6.0 + d(drop) * d(t.fraction)) as f32;
    let mut hips = fpu::lerp(hips_from, hips_to, t.fraction);
    hips.z -= 6.0;
    if blocked {
        if reach - hips_drop < -0.75 * hips_drop {
            return ret(pm);
        }
        let mut v = fpu::ma(feet - hips, 6.0, fwd);
        v.z += 6.0;
        let v = fpu::normalize(v).0;
        let mut to = fpu::ma(hips_from, q.length - 6.0 - 48.0, v);
        to.x = (0.5 * ((d(q.length) - 6.0) * d(fwd.x) + d(o.x) + d(to.x))) as f32;
        to.y = (0.5 * ((d(q.length) - 6.0) * d(fwd.y) + d(o.y) + d(to.y))) as f32;
        let mut from = hips_from;
        let mut t = tr(pm, from, to);
        if t.fraction < 1.0 {
            from = fpu::lerp(from, to, t.fraction);
            from.z += 18.0;
            to.z += 18.0;
            t = tr(pm, from, to);
            if t.fraction < 1.0 {
                return ret(pm);
            }
        }
        feet = fpu::lerp(from, to, t.fraction);
    }
    // The feet dropped onto the ground.
    let feet_to = Vec3::new(
        feet.x,
        feet.y,
        (d(feet.z) - ((d(feet.z) - d(hips.z)) * 2.0 + d(r))) as f32,
    );
    let t = tr(pm, feet, feet_to);
    if t.fraction == 1.0 {
        return ret(pm);
    }
    if t.normal.z < 0.7 {
        return false;
    }
    let mut feet = fpu::lerp(feet, feet_to, t.fraction);
    feet.z -= 6.0;
    let bend = fpu::angle_subtract(fpu::vectopitch(feet - hips), fpu::vectopitch(hips - o));
    let mut ok = (-50.0..=70.0).contains(&bend);
    let up = Vec3::Z * 5.0;
    if pm
        .raw_trace(o + up, hips + up, Vec3::ZERO, Vec3::ZERO, mask)
        .fraction
        < 1.0
    {
        ok = false;
    }
    if pm
        .raw_trace(hips + up, feet + up, Vec3::ZERO, Vec3::ZERO, mask)
        .fraction
        < 1.0
    {
        ok = false;
    }
    let torso = fpu::angle_normalize180(fpu::vectopitch(o - hips));
    let legs = fpu::angle_normalize180(fpu::vectopitch(hips - feet));
    if q.outputs {
        pm.ps.prone_valid = [0.0, torso, legs];
    }
    if ok { true } else { ret(pm) }
}

/// `BG_CheckProneTurned` (0x80e6dac): a body turned toward `yaw` fits at a
/// heading and a length between the body's and the view's.
fn check_prone_turned(pm: &mut Pmove, yaw: f32) -> bool {
    let delta = fpu::angle_delta(yaw, pm.ps.view_angles[1]);
    let f = (d(delta.abs()) / d(PRONE_TURN_DIVISOR)) as f32;
    let heading = fpu::normalize360_accurate((d(yaw) - (1.0 - d(f)) * d(delta)) as f32);
    let length = (d(PRONE_LENGTH) * (1.0 - d(f)) + d(PRONE_SWING_LENGTH) * d(f)) as f32;
    check_prone(
        pm,
        ProneCheck {
            origin: pm.ps.origin,
            height: HEIGHT_PRONE,
            yaw: heading,
            already_prone: true,
            on_ground: pm.ps.ground_entity_num != ENTITYNUM_NONE,
            ground_normal: None,
            length,
            outputs: true,
        },
    )
}

fn swing_fits(pm: &mut Pmove, yaw: f32) -> bool {
    check_prone(
        pm,
        ProneCheck {
            origin: pm.ps.origin,
            height: HEIGHT_PRONE,
            yaw,
            already_prone: true,
            on_ground: pm.ps.ground_entity_num != ENTITYNUM_NONE,
            ground_normal: None,
            length: PRONE_SWING_LENGTH,
            outputs: false,
        },
    )
}

fn push_yaw(ps: &mut PlayerState, deg: f32) {
    ps.delta_angles[1] = ps.delta_angles[1].wrapping_add(fpu::shorts(deg));
}

/// `PM_UpdateViewAngles` (0x80e6eb8): the cmd's angles over `delta_angles`,
/// the pitch held at `player_view_pitch_up` / `_down` (85: 15473 shorts),
/// then a prone body's yaw and torso caps, then the lean.
pub(crate) fn update_view_angles(pm: &mut Pmove, msec: f32) {
    let ps = &mut *pm.ps;
    if ps.pm_type == pm_type::INTERMISSION {
        return;
    }
    if ps.pm_type >= pm_type::DEAD {
        update_lean(pm, msec);
        return;
    }
    let old_yaw = ps.view_angles[1];
    let up = (d(fpu::SHORT_PER_DEG) * d(pm.rules.view_pitch_up)) as i32 & 0xffff;
    let down = (d(fpu::SHORT_PER_DEG) * d(pm.rules.view_pitch_down)) as i32 & 0xffff;
    for i in 0..3 {
        let mut a = (ps.delta_angles[i] as i16).wrapping_add(pm.cmd.angles[i] as i16);
        if i == 0 {
            if i32::from(a) > down {
                ps.delta_angles[0] = down - pm.cmd.angles[0];
                a = down as i16;
            } else if i32::from(a) < -up {
                ps.delta_angles[0] = -pm.cmd.angles[0] - up;
                a = -(up as i16);
            }
        }
        ps.view_angles[i] = DEG_PER_SHORT * f32::from(a);
    }
    let new_yaw = ps.view_angles[1];
    if ps.e_flags & ef::TURRET != 0 {
        turret_clamp(ps);
        return;
    }
    if ps.pm_flags & pmf::MANTLE != 0 {
        super::mantle::cap_view(pm);
        return;
    }
    super::ladder::cap_yaw(pm);
    if pm.ps.pm_flags & pmf::PRONE != 0 {
        prone_view(pm, msec, old_yaw, new_yaw);
    }
    if !matches!(
        pm.ps.pm_type,
        pm_type::NOCLIP | pm_type::UFO | pm_type::SPECTATOR
    ) {
        update_lean(pm, msec);
    }
}

/// The gunner's arm of `PM_UpdateViewAngles` (0x80e6eb8, its `eFlags &
/// 0x300` branch): pitch then yaw, a view more than
/// `viewAngleClampRange` off `viewAngleClampBase` is put back on the edge,
/// the excess pushed into `delta_angles` unmasked (cod2-turrets.md 5).
fn turret_clamp(ps: &mut PlayerState) {
    for i in 0..2 {
        let (base, range) = (ps.view_angle_clamp_base[i], ps.view_angle_clamp_range[i]);
        let delta = fpu::angle_delta(base, ps.view_angles[i]);
        if delta > range || delta < -range {
            let excess = if delta > range {
                delta - range
            } else {
                delta + range
            };
            ps.delta_angles[i] = ps.delta_angles[i].wrapping_add(fpu::shorts(excess));
            ps.view_angles[i] = fpu::normalize360_accurate(if excess > 0.0 {
                base - range
            } else {
                base + range
            });
        }
    }
}

/// The prone arm of `PM_UpdateViewAngles` (0x80e70c0..0x80e7a6c): the body
/// swings toward the view at 55 deg/s, stepping back a degree at a time
/// while the swing does not fit; a body still off the view that does not
/// fit turns the view toward it; the view is capped `bg_prone_yawcap` off
/// the body, and the torso 45 off its ground pitch. A refusal sets 0x10000
/// and eases a turn of at most a degree 98% back.
fn prone_view(pm: &mut Pmove, msec: f32, old_yaw: f32, new_yaw: f32) {
    let cap = pm.rules.prone_yawcap;
    let mut refused = false;
    let mut delta = fpu::angle_delta(pm.ps.prone_direction, pm.ps.view_angles[1]);
    let moving = pm.cmd.forward_move != 0 || pm.cmd.right_move != 0;
    'swing: {
        if !(delta > cap - PRONE_SOFT_EDGE
            || delta < -(cap - PRONE_SOFT_EDGE)
            || moving && delta != 0.0)
        {
            break 'swing;
        }
        let step = (0.001 * d(PRONE_SWING_DEG_PER_SEC) * d(msec)) as f32;
        let mut target = if (delta.abs() as f64) < f64::from(step) {
            pm.ps.view_angles[1]
        } else if delta > 0.0 {
            pm.ps.prone_direction - step
        } else {
            pm.ps.prone_direction + step
        };
        let mut more = true;
        while !check_prone_turned(pm, target) {
            if !more {
                break 'swing;
            }
            let mut dd = fpu::angle_delta(pm.ps.prone_direction, target);
            more = dd.abs() > 1.0;
            if more {
                dd = if dd > 0.0 { 1.0 } else { -1.0 };
            } else {
                refused = true;
            }
            target = fpu::normalize360_accurate(target + dd);
        }
        let view = pm.ps.view_angles[1];
        let fits = swing_fits(pm, view) && swing_fits(pm, target);
        if fits {
            pm.ps.prone_direction = target;
        } else {
            refused = true;
        }
    }
    delta = fpu::angle_delta(pm.ps.prone_direction, pm.ps.view_angles[1]);
    'turn: {
        if delta == 0.0 {
            break 'turn;
        }
        let mut target = pm.ps.prone_direction;
        let mut more = true;
        loop {
            let fits = swing_fits(pm, target);
            if fits && check_prone_turned(pm, target) {
                break;
            }
            if !more {
                break 'turn;
            }
            more = delta.abs() > 1.0;
            if more {
                delta = if delta > 0.0 { 1.0 } else { -1.0 };
            }
            refused = true;
            push_yaw(pm.ps, delta);
            pm.ps.view_angles[1] = fpu::normalize360_accurate(pm.ps.view_angles[1] + delta);
            delta = fpu::angle_delta(pm.ps.prone_direction, pm.ps.view_angles[1]);
            if !fits {
                target = fpu::normalize360_accurate(target + delta);
            }
        }
        pm.ps.prone_direction = target;
    }
    let ps = &mut *pm.ps;
    if delta > cap || delta < -cap {
        let excess = if delta > cap {
            delta - cap
        } else {
            delta + cap
        };
        push_yaw(ps, excess);
        ps.view_angles[1] = fpu::normalize360_accurate(if excess > 0.0 {
            ps.prone_direction - cap
        } else {
            ps.prone_direction + cap
        });
    }
    if refused {
        ps.pm_flags |= pmf::REFUSED;
        let back = fpu::angle_delta(old_yaw, ps.view_angles[1]);
        if back.abs() <= 1.0 && back * fpu::angle_delta(new_yaw, ps.view_angles[1]) > 0.0 {
            let ease = PRONE_REFUSED_EASE * back;
            ps.view_angles[1] = fpu::normalize360_accurate(ps.view_angles[1] + ease);
            push_yaw(ps, ease);
        }
    }
    let tilt = fpu::angle_delta(ps.prone_torso_pitch, ps.view_angles[0]);
    if tilt > PRONE_PITCHCAP || tilt < -PRONE_PITCHCAP {
        let excess = if tilt > PRONE_PITCHCAP {
            tilt - PRONE_PITCHCAP
        } else {
            PRONE_PITCHCAP + tilt
        };
        ps.delta_angles[0] = ps.delta_angles[0].wrapping_add(fpu::shorts(excess));
        ps.view_angles[0] = fpu::normalize180_accurate(if excess > 0.0 {
            ps.prone_torso_pitch - PRONE_PITCHCAP
        } else {
            PRONE_PITCHCAP + ps.prone_torso_pitch
        });
    }
}

/// `PM_UpdateLean` (0x80e69f4): the keys count on the ground or linked, out
/// at 350 ms and back at 280 to 0.5 (0.25 prone); a sphere from the eye to a
/// full lean holds it to what the wall leaves.
fn update_lean(pm: &mut Pmove, msec: f32) {
    let ps = &mut *pm.ps;
    let mut dir = 0;
    if pm.cmd.buttons & (button::LEAN_LEFT | button::LEAN_RIGHT) != 0
        && ps.pm_flags & pmf::FROZEN == 0
        && ps.pm_type < pm_type::DEAD
        && (ps.ground_entity_num != ENTITYNUM_NONE || ps.pm_type == pm_type::NORMAL_LINKED)
    {
        if pm.cmd.buttons & button::LEAN_LEFT != 0 {
            dir = -1;
        }
        if pm.cmd.buttons & button::LEAN_RIGHT != 0 {
            dir += 1;
        }
    }
    if ps.e_flags & ef::TURRET != 0 {
        dir = 0;
    }
    let scale = if ps.effective_stance() == Stance::Prone {
        0.25
    } else {
        0.5
    };
    let mut l = ps.leanf;
    // `l -+ msec / C * scale` on the x87 stack, one store (0x80e6adf).
    let out = d(msec) / d(LEAN_TO_MS) * d(scale);
    let back = d(msec) / d(LEAN_FROM_MS) * d(scale);
    match dir {
        0 => {
            if l > 0.0 {
                l = ((d(l) - back) as f32).max(0.0);
            } else if l < 0.0 {
                l = ((d(l) + back) as f32).min(0.0);
            }
        }
        d if d < 0 => {
            if -scale < l {
                l = (f64::from(l) - out) as f32;
            }
            if l < -scale {
                l = -scale;
            }
        }
        _ => {
            if l < scale {
                l = (f64::from(l) + out) as f32;
            }
            if scale < l {
                l = scale;
            }
        }
    }
    ps.leanf = l;
    if l == 0.0 {
        return;
    }
    let side = if l.is_sign_negative() { -1.0 } else { 1.0 };
    let eye = Vec3::new(
        ps.origin.x,
        ps.origin.y,
        ps.origin.z + ps.view_height_current,
    );
    let yaw = vcod_common::x87::radians(ps.view_angles[1]);
    let to = vcod_common::pmove::aim::add_lean(eye, yaw, side, vcod_common::patch::Arith::Stored);
    let t = pm.raw_trace(
        eye,
        to,
        Vec3::splat(-8.0),
        Vec3::splat(8.0),
        super::MASK_PLAYERSOLID,
    );
    let max = vcod_common::pmove::aim::unget_lean_fraction(t.fraction);
    if max < pm.ps.leanf.abs() {
        pm.ps.leanf = max * side;
    }
}

/// `PM_CheckDuck` (0x80e4644): the stance keys against the room each
/// stance needs, the eye's target, its lerp, then the box, `eFlags` and
/// `pm_flags` off the stance the eye reached.
pub(crate) fn check_duck(pm: &mut Pmove, pml: &mut Pml) {
    pm.prone_change = false;
    let mask = pm.tracemask & !vcod_common::movetrace::CONTENTS_BODY;
    if pm.ps.pm_type == pm_type::SPECTATOR {
        pm.mins = Vec3::splat(-8.0);
        pm.maxs = Vec3::new(8.0, 8.0, 16.0);
        pm.ps.pm_flags &= !(pmf::PRONE | pmf::DUCKED);
        if pm.cmd.buttons & button::PRONE != 0 {
            pm.cmd.buttons &= !button::PRONE;
            pm.ps.add_event(ev::STANCE_FORCE_STAND, 0);
        }
        pm.ps.view_height_target = 0;
        pm.ps.view_height_current = 0.0;
        return;
    }
    let was_prone = pm.ps.pm_flags & pmf::PRONE != 0;
    pm.mins = pm.ps.mins;
    pm.maxs = pm.ps.maxs;
    if pm.ps.pm_type >= pm_type::DEAD {
        pm.ps.view_height_target = VIEW_DEAD;
        view_height_adjust(pm.ps, pm.cmd.server_time, pml.frametime);
        return;
    }
    let fits = |pm: &mut Pmove, height: f32| {
        pm.maxs.z = height;
        let o = pm.ps.origin;
        !pm.raw_trace(o, o, pm.mins, pm.maxs, mask).allsolid
    };
    let quiet = pm.cmd.buttons & button::STANCE_HELD != 0;
    let stand = pm.ps.maxs.z;
    if pm.ps.e_flags & ef::TURRET != 0 {
        let e = pm.ps.e_flags & ef::TURRET;
        pm.ps.pm_flags &= !(pmf::PRONE | pmf::DUCKED);
        if e == 0x100 {
            pm.ps.pm_flags |= pmf::PRONE;
        } else if e == 0x200 {
            pm.ps.pm_flags |= pmf::DUCKED;
        }
    } else if pm.ps.pm_flags & pmf::FROZEN == 0 {
        if pm.ps.pm_flags & pmf::LADDER != 0
            && pm.cmd.buttons & (button::PRONE | button::CROUCH) != 0
        {
            pm.cmd.buttons &= !(button::PRONE | button::CROUCH);
            pm.ps.add_event(ev::STANCE_FORCE_STAND, 0);
        }
        let b = pm.cmd.buttons;
        let flags = pm.ps.pm_flags;
        if b & button::PRONE != 0 {
            let lie = flags & pmf::PRONE != 0
                || (pm.ps.ground_entity_num != ENTITYNUM_NONE
                    && check_prone(
                        pm,
                        ProneCheck {
                            origin: pm.ps.origin,
                            height: HEIGHT_PRONE,
                            yaw: pm.ps.view_angles[1],
                            already_prone: false,
                            on_ground: true,
                            ground_normal: None,
                            length: PRONE_LENGTH,
                            outputs: true,
                        },
                    ));
            if lie {
                pm.ps.pm_flags = (pm.ps.pm_flags | pmf::PRONE) & !pmf::DUCKED;
            } else if pm.ps.ground_entity_num != ENTITYNUM_NONE {
                pm.ps.pm_flags |= pmf::REFUSED;
                if !quiet {
                    let e = if flags & pmf::DUCKED == 0 {
                        ev::STANCE_FORCE_STAND
                    } else {
                        ev::STANCE_FORCE_CROUCH
                    };
                    pm.ps.add_event(e, 0);
                }
            }
        } else if b & button::CROUCH != 0 {
            // The stance anim events (0x80e4644), unforced; the stock script
            // has blocks for `prone_to_crouch` and `crouch_to_prone` only.
            if flags & pmf::PRONE == 0 {
                anim::event(pm, Event::StandToCrouch, false, false);
                pm.ps.pm_flags |= pmf::DUCKED;
            } else if fits(pm, HEIGHT_CROUCH) {
                anim::event(pm, Event::ProneToCrouch, false, false);
                pm.ps.pm_flags = (pm.ps.pm_flags & !pmf::PRONE) | pmf::DUCKED;
            } else if !quiet {
                pm.ps.add_event(ev::STANCE_FORCE_PRONE, 2);
            }
        } else if flags & pmf::PRONE != 0 {
            if fits(pm, stand) {
                anim::event(pm, Event::ProneToStand, false, false);
                pm.ps.pm_flags &= !(pmf::PRONE | pmf::DUCKED);
            } else if fits(pm, HEIGHT_CROUCH) {
                pm.ps.pm_flags = (pm.ps.pm_flags & !pmf::PRONE) | pmf::DUCKED;
            } else if !quiet {
                pm.ps.add_event(ev::STANCE_FORCE_PRONE, 1);
            }
        } else if flags & pmf::DUCKED != 0 {
            if fits(pm, stand) {
                anim::event(pm, Event::CrouchToStand, false, false);
                pm.ps.pm_flags &= !pmf::DUCKED;
            } else if !quiet {
                pm.ps.add_event(ev::STANCE_FORCE_CROUCH, 1);
            }
        }
    }
    // The eye's target, one stance at a time while no lerp runs.
    if pm.ps.view_height_lerp_time == 0 {
        let ps = &mut *pm.ps;
        if ps.pm_flags & pmf::PRONE == 0 {
            if ps.view_height_target == VIEW_PRONE {
                ps.view_height_target = VIEW_CROUCH;
                pm.prone_change = true;
            } else {
                ps.view_height_target = if ps.pm_flags & pmf::DUCKED != 0 {
                    VIEW_CROUCH
                } else {
                    VIEW_STAND
                };
            }
        } else if ps.view_height_target == VIEW_STAND {
            ps.view_height_target = VIEW_CROUCH;
        } else if ps.view_height_target != VIEW_PRONE {
            ps.view_height_target = VIEW_PRONE;
            pm.prone_change = true;
            // `Jump_ActivateSlowdown` (0x80dc7ee).
            if ps.pm_time == 0 {
                ps.pm_flags |= pmf::JUMPING;
                ps.pm_time = 1800;
            }
        }
    }
    view_height_adjust(pm.ps, pm.cmd.server_time, pml.frametime);
    let ps = &mut *pm.ps;
    match ps.effective_stance() {
        Stance::Prone => {
            pm.maxs.z = HEIGHT_PRONE;
            ps.e_flags = (ps.e_flags | ef::PRONE) & !ef::CROUCH;
            ps.pm_flags = (ps.pm_flags | pmf::PRONE) & !pmf::DUCKED;
        }
        Stance::Crouch => {
            pm.maxs.z = HEIGHT_CROUCH;
            ps.e_flags = (ps.e_flags | ef::CROUCH) & !ef::PRONE;
            ps.pm_flags = (ps.pm_flags | pmf::DUCKED) & !pmf::PRONE;
        }
        Stance::Stand => {
            pm.maxs.z = stand;
            ps.e_flags &= !(ef::PRONE | ef::CROUCH);
            ps.pm_flags &= !(pmf::PRONE | pmf::DUCKED);
        }
    }
    if pm.ps.pm_flags & pmf::PRONE != 0 && !was_prone {
        enter_prone(pm, mask);
    }
}

/// The first frame prone (0x80e4b6e..0x80e4e1a): a move key leaves the
/// sight; the body settles up 10 and back down; the heading takes the view's
/// yaw, the ground's pitch along it the plane under, and the torso that
/// pitch held within 45 of the view.
fn enter_prone(pm: &mut Pmove, mask: u32) {
    if pm.cmd.forward_move != 0 || pm.cmd.right_move != 0 {
        pm.ps.pm_flags &= !(pmf::PRONE_SIGHT | pmf::SIGHT);
    }
    let o = pm.ps.origin;
    let up = Vec3::new(o.x, o.y, o.z + PRONE_SETTLE_UP);
    let t = pm.raw_trace(o, up, pm.mins, pm.maxs, mask);
    let top = fpu::lerp(o, up, t.fraction);
    let t = pm.raw_trace(top, o, pm.mins, pm.maxs, mask);
    pm.ps.origin = fpu::lerp(top, o, t.fraction);
    pm.ps.prone_direction = pm.ps.view_angles[1];
    let o = pm.ps.origin;
    let probe = Vec3::new(o.x, o.y, o.z - PRONE_GROUND_PROBE);
    let t = pm.raw_trace(o, probe, pm.mins, pm.maxs, mask);
    pm.ps.prone_direction_pitch = if t.startsolid || t.fraction >= 1.0 {
        0.0
    } else {
        vcod_common::pmove::pitch_for_yaw_on_normal(pm.ps.prone_direction, t.normal)
    };
    let ps = &mut *pm.ps;
    let tilt = fpu::angle_delta(ps.prone_direction_pitch, ps.view_angles[0]);
    ps.prone_torso_pitch = if tilt < -PRONE_PITCHCAP {
        ps.view_angles[0] - PRONE_PITCHCAP
    } else if tilt > PRONE_PITCHCAP {
        PRONE_PITCHCAP + ps.view_angles[0]
    } else {
        ps.prone_direction_pitch
    };
}

/// rodata 0x8155144: the ground normal's z under which a prone body is
/// forced to crouch; 0x8155148: the prone pitches' rate in degrees a second.
const PRONE_STEEP_Z: f32 = f32::from_bits(0x3f33_3333);
const PRONE_PITCH_DEG_PER_SEC: f32 = 70.0;

/// `PM_UpdatePronePitch` (0x80e7c00), every prone frame after the sight
/// walk: off the ground the body must still fit (else `EV_STANCE_FORCE_CROUCH`
/// and the refused flag), on a plane steeper than 0.7 it is forced to crouch;
/// then the body's and the torso's pitch ease at 70 a second toward the
/// ground's pitch along the body's heading and the view's yaw (0 with no
/// ground plane).
pub(crate) fn update_prone_pitch(pm: &mut Pmove, pml: &Pml) {
    if pm.ps.pm_flags & pmf::PRONE == 0 {
        return;
    }
    let normal = pml.ground_plane.then_some(pml.ground_trace.normal);
    if pm.ps.ground_entity_num == ENTITYNUM_NONE {
        let fits = check_prone(
            pm,
            ProneCheck {
                origin: pm.ps.origin,
                height: HEIGHT_PRONE,
                yaw: pm.ps.prone_direction,
                already_prone: true,
                on_ground: false,
                ground_normal: normal,
                length: PRONE_LENGTH,
                outputs: true,
            },
        );
        if !fits {
            pm.ps.add_event(ev::STANCE_FORCE_CROUCH, 0);
            pm.ps.pm_flags |= pmf::REFUSED;
        }
    } else if normal.is_some_and(|n| n.z < PRONE_STEEP_Z) {
        pm.ps.add_event(ev::STANCE_FORCE_CROUCH, 0);
    }
    // The x87 keeps the product unrounded for the compare and the sum.
    let step = d(PRONE_PITCH_DEG_PER_SEC) * d(pml.frametime);
    let ease = |cur: f32, yaw: f32| -> f32 {
        let target = normal.map_or(0.0, |n| vcod_common::pmove::pitch_for_yaw_on_normal(yaw, n));
        let delta = fpu::angle_delta(target, cur);
        if delta == 0.0 {
            return cur;
        }
        let next = if step < d(delta.abs()) {
            (d(cur) + step * d(delta.signum())) as f32
        } else {
            cur + delta
        };
        fpu::normalize180_accurate(next)
    };
    let ps = &mut *pm.ps;
    ps.prone_direction_pitch = ease(ps.prone_direction_pitch, ps.prone_direction);
    ps.prone_torso_pitch = ease(ps.prone_torso_pitch, ps.view_angles[1]);
}
