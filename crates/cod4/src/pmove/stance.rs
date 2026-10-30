//! `PM_CheckDuck`, the eye's height, `BG_CheckProneValid`, the view angles
//! and the lean (docs/research/cod4-pmove.md 8). Addresses are
//! `cod4_lnxded`'s.

use glam::Vec3;

use super::fpu::{self, d};
use super::{
    ENTITYNUM_NONE, PlayerState, Pml, Pmove, Stance, VIEW_CROUCH, VIEW_DEAD, VIEW_LASTSTAND,
    VIEW_PRONE, VIEW_STAND, button, ef, ev, pm_type, pmf,
};

/// How far behind the origin a prone body's feet lie: 50 for the stance
/// test, the prone pitch's and the step's (immediate 0x42480000 at
/// 0x8058f56, 0x80566ea), 45 for the swing's fits (0x42340000 at
/// 0x8057a54); a turning body blends the two (rodata 0x820dec0, 0x8209670
/// at 0x8057982) by its turn over 240 (0x820debc).
pub(crate) const PRONE_FEET_DIST: f32 = 50.0;
const PRONE_SWING_FEET_DIST: f32 = 45.0;
const PRONE_TURN_DIVISOR: f32 = 240.0;
/// The box heights `PM_CheckDuck` tests and sets (immediates at 0x8058fae,
/// 0x8058d19 and 0x805892a).
pub(crate) const HEIGHT_PRONE: f32 = 30.0;
pub(crate) const HEIGHT_CROUCH: f32 = 50.0;
const HEIGHT_STAND: f32 = 70.0;
/// The prone yaw cap's soft edge (rodata 0x820aae0), the body's swing rate
/// (0x820deb8) and the refused turn's ease back (0x820dec4).
const PRONE_SOFT_EDGE: f32 = 5.0;
const PRONE_SWING_DEG_PER_SEC: f32 = 55.0;
const PRONE_REFUSED_EASE: f32 = f32::from_bits(0x3f7a_e148);
/// The torso's pitch either side of the ground's (rodata 0x820dec0).
const PRONE_PITCHCAP: f32 = 45.0;
/// The settle on going prone, up then down (rodata 0x820aac8, 0x820966c).
const PRONE_SETTLE_UP: f32 = 10.0;
const PRONE_GROUND_PROBE: f32 = 0.25;
/// The eye's rate to a target off the stance curves, dead or in last stand
/// (rodata 0x8209668, and its negation at 0x820decc).
const DEAD_VIEW_RATE: f64 = 180.0;
/// The lean's ramps out and back (rodata 0x820dea0, 0x820de9c and their
/// negations) and its reach, prone and otherwise (0x820966c, 0x8209628).
const LEAN_TO_MS: f64 = 350.0;
const LEAN_FROM_MS: f64 = 280.0;
const LEAN_PRONE: f32 = 0.25;
const LEAN_FULL: f32 = 0.5;

/// `BG_CheckProneValid`'s trace mask for a player: the selector argument
/// at 0x8054fd0 picks 0x810011 for 0, 0x820011 otherwise, and every pmove
/// caller passes 0.
const PRONE_MASK: u32 = 0x810011;
/// The fit's numbers (rodata): the probe box's half size and the probes'
/// drop under the body's height (0x820aacc), the middle's distance out
/// (0x820a314), the retry's lift over a sill (0x820aad0), the sill's slope
/// (0x82096a4), the middle's drop per unit of width (0x820a31c), the legs'
/// least reach past the width (0x820964c), the middle's give against the
/// legs (0x820aad4), the bend's range (0x820aad8, 0x820aadc), the sight
/// lines' lift (0x820aae0) and the room test's lift (0x820aac8).
const PROBE: f32 = 6.0;
const MID_OUT: f32 = 18.0;
const SILL_LIFT: f32 = 22.0;
const SILL_SLOPE: f32 = f32::from_bits(0x3f33_3333);
const MID_DROP_PER_WIDTH: f32 = 2.5;
const LEGS_PAST_WIDTH: f32 = 2.0;
const MID_GIVE: f32 = -0.75;
const BEND: std::ops::RangeInclusive<f64> = -50.0..=70.0;
const SIGHT_LIFT: f32 = 5.0;
const ROOM_LIFT: f32 = 10.0;

/// The eye's curves (`.data` 0x826b000 down to the crouch, 0x826b0e0 up to
/// the stand, 0x826af60 down to prone, 0x826b080 up from it):
/// `{percent, height, posadj}` rows ending at percent -1; every posadj is
/// 0, so the body push off a changing posadj never runs and is not
/// modelled.
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

/// `PM_GetViewHeightLerpTime` (inlined at 0x8058263 and 0x805838c): 400
/// into prone and up out of it to the crouch, 200 down to the crouch and
/// up to standing.
pub fn lerp_duration(target: i32, down: bool) -> i32 {
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

/// The curve's height at `pct` (the lookup `PM_ViewHeightAdjust` inlines at
/// 0x80582c6): linear between rows, the fraction kept.
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
            let f = f64::from(pct - p0) / f64::from(p1 - p0);
            return ((d(h1) - d(h0)) * f + d(h0)) as f32;
        }
    }
    c[0].1
}

/// The stance the eye passes next from `stage` going down or up: 60, 40
/// and 11 in order (0x8058360..0x805836e, 0x8058614..0x8058624).
fn next_stage(stage: i32, down: bool) -> i32 {
    match (down, stage) {
        (true, VIEW_STAND) => VIEW_CROUCH,
        (true, VIEW_CROUCH) => VIEW_PRONE,
        (false, VIEW_PRONE) => VIEW_CROUCH,
        (false, VIEW_CROUCH) => VIEW_STAND,
        (_, s) => s,
    }
}

/// `PM_ViewHeightAdjust` (0x805817e): the eye eases to its stance target
/// along the curves one stance at a time; a target the other way turns the
/// running leg round at the height it reached. A target off the curves
/// (dead, last stand) is met at 180 a second.
fn view_height_adjust(ps: &mut PlayerState, time: i32, frametime: f32) {
    let target = ps.view_height_target;
    let goal = target as f32;
    if target == 0 || ps.view_height_current == 0.0 {
        ps.view_height_current = if ps.pm_type == pm_type::SPECTATOR {
            0.0
        } else {
            goal
        };
        return;
    }
    if ps.view_height_current == goal && ps.view_height_lerp_time == 0 {
        return;
    }
    if !matches!(target, VIEW_PRONE | VIEW_CROUCH | VIEW_STAND) {
        // The sum stays on the stack for the overshoot test (0x8058204).
        ps.view_height_lerp_time = 0;
        let step = DEAD_VIEW_RATE * d(frametime);
        let now = d(ps.view_height_current);
        let rising = d(goal) > now;
        let next = if rising { now + step } else { now - step };
        let reached = if rising {
            next >= d(goal)
        } else {
            d(goal) >= next
        };
        ps.view_height_current = if reached { goal } else { next as f32 };
        return;
    }
    if ps.view_height_lerp_time != 0 {
        let (stage, down) = (ps.view_height_lerp_target, ps.view_height_lerp_down);
        let pct = (100 * (time - ps.view_height_lerp_time) / lerp_duration(stage, down)).max(0);
        if pct >= 100 {
            ps.view_height_current = stage as f32;
            ps.view_height_lerp_time = 0;
        } else {
            ps.view_height_current = curve_height(curve(stage, down), pct);
            let reversed = if down { target > stage } else { target < stage };
            if reversed {
                turn_leg(ps, time, 100 - pct);
            }
            return;
        }
    }
    if ps.view_height_current != goal {
        start_leg(ps, time);
    }
}

/// A new leg toward the target from the height the eye rests at
/// (0x8058443..0x8058508): prone goes down by the crouch from above 40,
/// standing up by it from below 40, the crouch whichever way it lies.
fn start_leg(ps: &mut PlayerState, time: i32) {
    let now = ps.view_height_current;
    let (down, stage) = match ps.view_height_target {
        VIEW_PRONE if now <= 40.0 => (true, VIEW_PRONE),
        VIEW_PRONE => (true, VIEW_CROUCH),
        VIEW_CROUCH => (now > 40.0, VIEW_CROUCH),
        _ if now >= 40.0 => (false, VIEW_STAND),
        _ => (false, VIEW_CROUCH),
    };
    ps.view_height_lerp_time = time;
    ps.view_height_lerp_down = down;
    ps.view_height_lerp_target = stage;
}

/// The running leg turned round with `done` percent of the new leg behind
/// it (0x8058340..0x80583c5): the new leg's start is put back by that
/// share of its length, stored as a float and truncated (`cvttss2si`).
fn turn_leg(ps: &mut PlayerState, time: i32, done: i32) {
    let down = !ps.view_height_lerp_down;
    let stage = next_stage(ps.view_height_lerp_target, down);
    ps.view_height_lerp_down = down;
    ps.view_height_lerp_target = stage;
    if done == 100 {
        ps.view_height_current = stage as f32;
        ps.view_height_lerp_time = 0;
        return;
    }
    let percent = d(f32::from_bits(0x3c23_d70a)) * f64::from(done);
    let behind = (f64::from(lerp_duration(stage, down)) * percent) as f32;
    ps.view_height_lerp_time = time - behind as i32;
}

/// What [`check_prone`] is asked: `BG_CheckProneValid`'s arguments, in the
/// order its wrapper at 0x8055ad8 forwards them, less the entity, the trace
/// handler and the two pitch pointers, which `outputs` stands for.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ProneCheck {
    pub origin: Vec3,
    pub size: f32,
    pub height: f32,
    pub yaw: f32,
    pub already_prone: bool,
    pub on_ground: bool,
    pub walkable: bool,
    pub feet_dist: f32,
    /// Whether the caller passes `fTorsoPitch` and `fWaistPitch`.
    pub outputs: bool,
}

/// Why a body may not lie down: never, or only while on the ground; one in
/// the air lies down anyway with its pitches zeroed (0x8055307).
enum Misfit {
    Never,
    OnGround,
}

/// How the body would lie: its torso's and legs' pitches, and whether the
/// bend and the sight lines along the body held.
struct Lie {
    torso: f32,
    legs: f32,
    clear: bool,
}

/// `BG_CheckProneValid` (0x8054fa2): whether a body may lie at `origin`
/// facing `yaw` with its feet `feet_dist` behind; the pitches go out with a
/// lie, even one the bend or a sight line spoils.
pub(crate) fn check_prone(pm: &mut Pmove, q: ProneCheck) -> bool {
    let lie = body_lie(pm, &q);
    if let (Ok(lie), true) = (&lie, q.outputs) {
        pm.ps.torso_pitch = lie.torso;
        pm.ps.waist_pitch = lie.legs;
    }
    match lie {
        Ok(Lie { clear: true, .. }) => true,
        Err(Misfit::Never) => false,
        _ if q.on_ground => false,
        _ => {
            if q.outputs {
                pm.ps.torso_pitch = 0.0;
                pm.ps.waist_pitch = 0.0;
            }
            true
        }
    }
}

/// `(s - floor(s + 0.5)) * 360` of the turns `s`, all on the stack: the
/// `AngleNormalize180` 1.7 inlines (0x8055562, 0x8057dd2).
fn turns180(deg: f64) -> f64 {
    let s = deg * INV_360;
    (s - (s + 0.5).floor()) * 360.0
}

/// A body not yet prone needs its box clear 10 up (0x80553b2..0x8055429).
fn room_to_lie(pm: &Pmove, q: &ProneCheck) -> bool {
    let (o, r) = (q.origin, q.size);
    let up = o.with_z((d(o.z) + d(ROOM_LIFT)) as f32);
    let (lo, hi) = (Vec3::new(-r, -r, 0.0), Vec3::new(r, r, q.height));
    !pm.raw_trace(o, up, lo, hi, PRONE_MASK).allsolid
}

/// The fit's traces: six-unit boxes and point lines.
struct Probes<'p, 'a> {
    pm: &'p Pmove<'a>,
}

/// A drop's contact: the stored top it fell from, the contact's height
/// kept and the trace's fraction.
struct Contact {
    top: Vec3,
    z: f64,
    frac: f32,
}

impl Probes<'_, '_> {
    fn sweep(&self, from: Vec3, to: Vec3) -> super::Tr {
        let half = Vec3::splat(PROBE);
        self.pm.raw_trace(from, to, -half, half, PRONE_MASK)
    }

    /// Whether the line between two points, both lifted 5 (each sum
    /// stored, 0x80556fe, 0x805571b, 0x805579b), is open.
    fn sight(&self, from: Vec3, to: Vec3) -> bool {
        let lift = |p: Vec3| p.with_z((d(p.z) + d(SIGHT_LIFT)) as f32);
        let line = self
            .pm
            .raw_trace(lift(from), lift(to), Vec3::ZERO, Vec3::ZERO, PRONE_MASK);
        line.fraction >= 1.0
    }

    /// A straight drop from `top` (kept, stored for the trace) by `depth`
    /// onto walkable ground: a miss fails only a body on the ground, a
    /// steep contact any. The contact keeps the top's x and y.
    fn floor_under(&self, top: [f64; 3], depth: f64) -> Result<Contact, Misfit> {
        let from = Vec3::new(top[0] as f32, top[1] as f32, top[2] as f32);
        let to = from.with_z((top[2] - depth) as f32);
        let tr = self.sweep(from, to);
        if tr.fraction == 1.0 {
            return Err(Misfit::OnGround);
        }
        if !tr.walkable {
            return Err(Misfit::Never);
        }
        let z = part_way(from, to, tr.fraction)[2];
        Ok(Contact {
            top: from,
            z,
            frac: tr.fraction,
        })
    }
}

/// The point `f` of the way from `a` to `b`, each component kept
/// (0x8055225..0x805525d, 0x80554cc..0x80554fa, 0x80559f3..0x8055a15).
fn part_way(a: Vec3, b: Vec3, f: f32) -> [f64; 3] {
    let (a, b, f) = (a.to_array(), b.to_array(), d(f));
    std::array::from_fn(|i| d(a[i]) + (d(b[i]) - d(a[i])) * f)
}

/// The legs' run behind the hip: its two ends and the fraction it got, and
/// for legs stopped short the length they lie over, stored across the
/// middle's trace (0x80551d9..0x80551e7).
struct Legs {
    hip: Vec3,
    heel: Vec3,
    reached: f32,
    short: Option<f32>,
}

/// `BG_CheckProneValid`'s layout (cod4-pmove.md 8): the legs' run, the
/// middle dropped 18 back, a short run's legs bent, the feet dropped, then
/// the pitches and the lines along the body.
fn body_lie(pm: &Pmove, q: &ProneCheck) -> Result<Lie, Misfit> {
    if !q.already_prone && !room_to_lie(pm, q) {
        return Err(Misfit::Never);
    }
    if q.on_ground && !q.walkable {
        return Err(Misfit::Never);
    }
    let probes = Probes { pm };
    let o = q.origin;
    // The probes' height over the origin and their reach behind it, both
    // stored (0x805505c, 0x8055092); the heading back too (0x805502b).
    let rise = (d(q.height) - d(PROBE)) as f32;
    let reach = (d(q.feet_dist) - d(PROBE)) as f32;
    let heading = Vec3::new(0.0, (d(q.yaw) - 180.0) as f32, 0.0);
    let mut back = vcod_common::x87::angle_vectors(heading)[0];
    let legs = legs_run(&probes, q, rise, reach, &mut back)?;

    // 18 back at the probes' height, `(18 back.z + o.z) + rise` kept
    // (0x805517c), by `(2.5 width + rise) - 6` (0x805519c).
    let mid_top = [
        d(MID_OUT) * d(back.x) + d(o.x),
        d(MID_OUT) * d(back.y) + d(o.y),
        d(MID_OUT) * d(back.z) + d(o.z) + d(rise),
    ];
    let mid_depth = d(q.size) * d(MID_DROP_PER_WIDTH) + d(rise) - d(PROBE);
    let waist = probes.floor_under(mid_top, mid_depth)?;
    // The contact stored, then 6 lower stored again (0x80552bb, 0x80552cd).
    let mid = waist.top.with_z((d(waist.z as f32) - d(PROBE)) as f32);

    let mut feet = part_way(legs.hip, legs.heel, legs.reached);
    if let Some(len) = legs.short {
        // The depth crosses the trace stored (0x80551d9).
        let fell = d(mid_depth as f32) * d(waist.frac) + d(PROBE);
        if fell * d(MID_GIVE) > d(len) - fell {
            return Err(Misfit::OnGround);
        }
        feet = bent_feet(&probes, o, back, reach, mid, waist.top, feet)?;
    }

    // Twice the feet's height over the middle, plus the width
    // (0x8055464..0x8055471); the contact 6 lower in one store (0x8055504).
    let feet_depth = (feet[2] - d(mid.z)) * 2.0 + d(q.size);
    let sole = probes.floor_under(feet, feet_depth)?;
    let ground = sole.top.with_z((sole.z - d(PROBE)) as f32);
    Ok(lie_along(&probes, o, mid, ground))
}

/// The run from the hip, `rise` over the origin, to the heel `reach`
/// behind (0x8055022..0x80550f0). One stopped short fails off the ground
/// and under `width + 2`; under the sill `0.7 rise + 18` (stored,
/// 0x8055395) it runs once more to a heel 22 higher (0x8055a29), whose
/// line becomes `back`.
fn legs_run(
    probes: &Probes,
    q: &ProneCheck,
    rise: f32,
    reach: f32,
    back: &mut Vec3,
) -> Result<Legs, Misfit> {
    let o = q.origin;
    // The hip's height kept for the heel (0x8055082).
    let hip_z = d(o.z) + d(rise);
    let hip = o.with_z(hip_z as f32);
    let mut heel = Vec3::new(
        (d(o.x) + d(reach) * d(back.x)) as f32,
        (d(o.y) + d(reach) * d(back.y)) as f32,
        (hip_z + d(reach) * d(back.z)) as f32,
    );
    let legs = |heel: Vec3, reached: f32, short: Option<f64>| Legs {
        hip,
        heel,
        reached,
        short: short.map(|len| len as f32),
    };
    let first = probes.sweep(hip, heel);
    if first.fraction >= 1.0 {
        return Ok(legs(heel, first.fraction, None));
    }
    if !q.on_ground {
        return Err(Misfit::Never);
    }
    let sill = (d(rise) * d(SILL_SLOPE) + d(MID_OUT)) as f32;
    let got = d(reach) * d(first.fraction) + d(PROBE);
    if d(q.size) + d(LEGS_PAST_WIDTH) > got {
        return Err(Misfit::Never);
    }
    if got >= d(sill) {
        return Ok(legs(heel, first.fraction, Some(got)));
    }
    heel.z = (d(heel.z) + d(SILL_LIFT)) as f32;
    let (line, span) = fpu::normalize_to(heel - hip);
    *back = line;
    let second = probes.sweep(hip, heel);
    if second.fraction >= 1.0 {
        return Ok(legs(heel, second.fraction, None));
    }
    let got = d(span) * d(second.fraction) + d(PROBE);
    if d(sill) > got {
        return Err(Misfit::Never);
    }
    Ok(legs(heel, second.fraction, Some(got)))
}

/// Legs stopped short lie bent (0x80558aa..0x8055a1b): from the middle's
/// top along the line from the middle to where the run stopped, out to
/// half way between that line's end and the full reach behind the origin.
/// A wall on the way is climbed once by 18; a second fails a body on the
/// ground.
fn bent_feet(
    probes: &Probes,
    o: Vec3,
    back: Vec3,
    reach: f32,
    mid: Vec3,
    mid_top: Vec3,
    stop: [f64; 3],
) -> Result<[f64; 3], Misfit> {
    let six = d(PROBE);
    let [x, y, z] = [
        stop[0] - d(mid.x) + six * d(back.x),
        stop[1] - d(mid.y) + six * d(back.y),
        stop[2] - d(mid.z) + six * d(back.z) + six,
    ];
    // The length off the kept line, the scale onto its stored components
    // (0x80558cd..0x805593b): x and y scaled are stored, z kept.
    let len = (x * x + y * y + z * z).sqrt();
    let scale = if len > 0.0 { 1.0 / len } else { 1.0 };
    let [ux, uy, uz] = [x, y, z].map(|c| d(c as f32) * scale);
    let span = d(reach) - d(MID_OUT);
    let end_x = (d(ux as f32) * span + d(mid_top.x)) as f32;
    let end_y = (d(uy as f32) * span + d(mid_top.y)) as f32;
    // Each half way stored (0x8055985, 0x80559aa).
    let halve = |o: f32, back: f32, end: f32| ((d(reach) * d(back) + d(o) + d(end)) * 0.5) as f32;
    let mut to = Vec3::new(
        halve(o.x, back.x, end_x),
        halve(o.y, back.y, end_y),
        (uz * span + d(mid_top.z)) as f32,
    );
    let mut from = mid_top;
    for climbs_left in [1, 0] {
        let sweep = probes.sweep(from, to);
        let at = part_way(from, to, sweep.fraction);
        if sweep.fraction >= 1.0 {
            return Ok(at);
        }
        if climbs_left == 0 {
            break;
        }
        from = Vec3::new(at[0] as f32, at[1] as f32, (at[2] + d(MID_OUT)) as f32);
        to.z = (d(to.z) + d(MID_OUT)) as f32;
    }
    Err(Misfit::OnGround)
}

/// The pitches off the origin to the middle and the middle to the feet,
/// each stored (0x80555b1, 0x8055641), the bend between them in -50..70,
/// and both lines along the body open.
fn lie_along(probes: &Probes, o: Vec3, mid: Vec3, ground: Vec3) -> Lie {
    let pitch = |from: Vec3, to: Vec3| turns180(d(fpu::vectopitch(from - to))) as f32;
    let torso = pitch(o, mid);
    let legs = pitch(mid, ground);
    let bend_ok = BEND.contains(&turns180(d(torso) - d(legs)));
    let lines_open = probes.sight(o, mid) & probes.sight(mid, ground);
    Lie {
        torso,
        legs,
        clear: bend_ok && lines_open,
    }
}

/// The swing's fit (0x8057a54, 0x8057bb4, 0x8058017): the body's box at
/// `yaw`, the feet 45 behind, no pitches out.
fn swing_fits(pm: &mut Pmove, yaw: f32) -> bool {
    check_prone(
        pm,
        ProneCheck {
            origin: pm.ps.origin,
            size: 15.0,
            height: HEIGHT_PRONE,
            yaw,
            already_prone: true,
            on_ground: pm.ps.ground_entity_num != ENTITYNUM_NONE,
            walkable: true,
            feet_dist: PRONE_SWING_FEET_DIST,
            outputs: false,
        },
    )
}

/// `BG_CheckProneTurned`, inlined at 0x8057944 and 0x8057c28: a body turned
/// to `yaw` is tested at a heading and a feet distance part way back
/// toward the view, by the turn's share of 240 degrees.
fn turned_fits(pm: &mut Pmove, yaw: f32) -> bool {
    let turn = fpu::angle_delta(yaw, pm.ps.view_angles[1]);
    let share = d(turn.abs()) / d(PRONE_TURN_DIVISOR);
    let rest = 1.0 - share;
    let heading = fpu::angle_normalize360((d(yaw) - d(turn) * rest) as f32);
    // The weights cross the `AngleNormalize360` call stored (0x8057971,
    // 0x8057977); their sum is kept.
    let feet =
        (d(rest as f32) * d(PRONE_FEET_DIST) + d(share as f32) * d(PRONE_SWING_FEET_DIST)) as f32;
    check_prone(
        pm,
        ProneCheck {
            origin: pm.ps.origin,
            size: 15.0,
            height: HEIGHT_PRONE,
            yaw: heading,
            already_prone: true,
            on_ground: pm.ps.ground_entity_num != ENTITYNUM_NONE,
            walkable: true,
            feet_dist: feet,
            outputs: true,
        },
    )
}

/// `PM_UpdateViewAngles`: the cmd's angles over the float `delta_angles`,
/// the pitch held at `player_view_pitch_up` / `_down`, then the gunner's,
/// the ladder's or a prone body's caps, then the lean.
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
    clamp_angles(pm);
    let ps = &mut *pm.ps;
    if ps.e_flags & ef::TURRET != 0 {
        gunner_arcs(ps, old_yaw);
        return;
    }
    if ps.pm_flags & pmf::MANTLE != 0 {
        super::mantle::cap_view(pm);
        return;
    }
    super::ladder::cap_yaw(pm);
    if pm.ps.pm_flags & pmf::PRONE != 0 {
        prone_caps(pm, msec, old_yaw);
    }
    if !matches!(
        pm.ps.pm_type,
        pm_type::NOCLIP | pm_type::UFO | pm_type::SPECTATOR
    ) {
        update_lean(pm, msec);
    }
}

/// The clamp of `PM_UpdateViewAngles` (0x805742e..0x805768b): each angle
/// `delta + short * 360 / 65536`
/// wrapped to -180..180, the pitch held in `-up..down` with the excess put
/// back into `delta_angles[0]`.
fn clamp_angles(pm: &mut Pmove) {
    let up = d(pm.rules.view_pitch_up);
    let down = d(pm.rules.view_pitch_down);
    let ps = &mut *pm.ps;
    for i in 0..3 {
        let short = f64::from(pm.cmd.angles[i]) * DEG_PER_SHORT_F64;
        let mut t = turns180(d(ps.delta_angles[i]) + short);
        if i == 0 {
            if t > down {
                ps.delta_angles[0] = (down - short) as f32;
                t = down;
            } else if t < -up {
                ps.delta_angles[0] = (-up - short) as f32;
                t = -up;
            }
        }
        ps.view_angles[i] = turns180(t) as f32;
    }
}

/// 1 / 360 as the double constant the clamp multiplies by.
const INV_360: f64 = 0.002777777845039964;

/// 360 / 65536 as the double the clamp multiplies a short by.
const DEG_PER_SHORT_F64: f64 = 0.0054931640625;

/// How far `off` lies past `±limit`, or `None` inside: the arc test the
/// gunner's and the prone caps share (0x80576cd, 0x8057d12, 0x8057d8c).
fn overshoot(off: f64, limit: f64) -> Option<f64> {
    if off > limit {
        Some(off - limit)
    } else if -limit > off {
        Some(off + limit)
    } else {
        None
    }
}

/// The edge an overshoot puts the view back on: the near one, `centre`
/// less the limit for an overshoot past `+limit`.
fn arc_edge(centre: f32, limit: f32, excess: f64) -> f32 {
    if excess > 0.0 {
        (d(centre) - d(limit)) as f32
    } else {
        (d(centre) + d(limit)) as f32
    }
}

/// The gunner's arm of `PM_UpdateViewAngles` (0x8057691..0x8057796),
/// pitch then yaw: an arc under 180 holds the view to
/// `viewAngleClampBase ± viewAngleClampRange`, the overshoot pushed into
/// `delta_angles`. The yaw's offset is the base's from last cmd's view plus
/// the turn since, so it reads past the wrap.
fn gunner_arcs(ps: &mut PlayerState, old_yaw: f32) {
    for axis in 0..2 {
        let (base, range) = (
            ps.view_angle_clamp_base[axis],
            ps.view_angle_clamp_range[axis],
        );
        if range >= 180.0 {
            continue;
        }
        let off = if axis == 1 {
            d(fpu::angle_delta(base, old_yaw)) + d(fpu::angle_delta(old_yaw, ps.view_angles[1]))
        } else {
            d(fpu::angle_delta(base, ps.view_angles[0]))
        };
        if let Some(excess) = overshoot(off, d(range)) {
            ps.delta_angles[axis] = (d(ps.delta_angles[axis]) + excess) as f32;
            ps.view_angles[axis] = fpu::angle_normalize360(arc_edge(base, range, excess));
        }
    }
}

/// The prone arm of `PM_UpdateViewAngles` (0x8057858..0x8057e2a): the
/// body swings after the view, the view is dragged back to a body that
/// cannot swing, the yaw is capped `bg_prone_yawcap` off the body and the
/// pitch 45 off the torso's. A refusal sets `PMF_PRONE_REFUSED` and eases back
/// a turn of a degree or less.
fn prone_caps(pm: &mut Pmove, msec: f32, old_yaw: f32) {
    let new_yaw = pm.ps.view_angles[1];
    let swing_refused = swing_body(pm, msec);
    let (off, drag_refused) = drag_view(pm);
    let cap = pm.rules.prone_yawcap;
    let ps = &mut *pm.ps;
    if let Some(excess) = overshoot(d(off), d(cap)) {
        ps.delta_angles[1] = (d(ps.delta_angles[1]) + excess) as f32;
        ps.view_angles[1] = fpu::angle_normalize360(arc_edge(ps.prone_direction, cap, excess));
    }
    if swing_refused || drag_refused {
        ease_refused(ps, old_yaw, new_yaw);
    }
    let tilt = fpu::angle_delta(ps.prone_torso_pitch, ps.view_angles[0]);
    if let Some(excess) = overshoot(d(tilt), d(PRONE_PITCHCAP)) {
        ps.delta_angles[0] = (d(ps.delta_angles[0]) + excess) as f32;
        let edge = d(ps.prone_torso_pitch) - d(PRONE_PITCHCAP) * excess.signum();
        ps.view_angles[0] = turns180(edge) as f32;
    }
}

/// The body's swing toward the view (0x8057858..0x8057ad2), when the view
/// is near the cap's edge or the body crawls: 55 degrees a second, or onto
/// the view when that is nearer. A heading that does not fit is walked back
/// toward the body a degree at a time; the swing is then taken only if the
/// body also fits at the view and at the heading. Returns whether it was
/// refused.
fn swing_body(pm: &mut Pmove, msec: f32) -> bool {
    let view = pm.ps.view_angles[1];
    let body = pm.ps.prone_direction;
    let off = fpu::angle_delta(body, view);
    let soft = d(pm.rules.prone_yawcap) - d(PRONE_SOFT_EDGE);
    let near_edge = d(off) > soft || -soft > d(off);
    let crawling = (pm.cmd.forward_move != 0 || pm.cmd.right_move != 0) && off != 0.0;
    if !(near_edge || crawling) {
        return false;
    }
    let step = d(msec) * d(PRONE_SWING_DEG_PER_SEC) * d(super::MSEC_TO_SEC);
    let mut heading = if step > d(off.abs()) {
        view
    } else if off > 0.0 {
        (d(body) - step) as f32
    } else {
        (d(body) + step) as f32
    };
    let mut refused = false;
    let mut walking_back = true;
    loop {
        if turned_fits(pm, heading) {
            if swing_fits(pm, view) && swing_fits(pm, heading) {
                pm.ps.prone_direction = heading;
                return refused;
            }
            return true;
        }
        if !walking_back {
            return refused;
        }
        let gap = fpu::angle_delta(pm.ps.prone_direction, heading);
        walking_back = gap.abs() > 1.0;
        let back = if walking_back {
            if gap > 0.0 { 1.0 } else { -1.0 }
        } else {
            refused = true;
            gap
        };
        heading = fpu::angle_normalize360((d(back) + d(heading)) as f32);
    }
}

/// The view dragged onto a body that cannot swing to it (0x8057ad8..
/// 0x8057d07): a degree a step, the last step what is left, until the
/// body fits both swung and turned at its heading; a heading the swing
/// refuses follows the view's turn. Returns the view's last offset from
/// the body and whether the view was moved.
fn drag_view(pm: &mut Pmove) -> (f32, bool) {
    let mut off = fpu::angle_delta(pm.ps.prone_direction, pm.ps.view_angles[1]);
    if off == 0.0 {
        return (off, false);
    }
    let mut heading = pm.ps.prone_direction;
    let mut dragged = false;
    let mut more = true;
    loop {
        let swings = swing_fits(pm, heading);
        if swings && turned_fits(pm, heading) {
            pm.ps.prone_direction = heading;
            return (off, dragged);
        }
        if !more {
            return (off, dragged);
        }
        more = off.abs() > 1.0;
        if more {
            off = if off > 0.0 { 1.0 } else { -1.0 };
        }
        let ps = &mut *pm.ps;
        ps.delta_angles[1] = (d(off) + d(ps.delta_angles[1])) as f32;
        ps.view_angles[1] = fpu::angle_normalize360((d(off) + d(ps.view_angles[1])) as f32);
        off = fpu::angle_delta(ps.prone_direction, ps.view_angles[1]);
        if !swings {
            heading = fpu::angle_normalize360((d(heading) + d(off)) as f32);
        }
        dragged = true;
    }
}

/// A refused prone turn (0x8057f88): `PMF_PRONE_REFUSED`, and a view still
/// within a degree of last cmd's, turned the way the cmd turned it, eases
/// 98% of the way back.
fn ease_refused(ps: &mut PlayerState, old_yaw: f32, new_yaw: f32) {
    ps.pm_flags |= pmf::PRONE_REFUSED;
    let back = fpu::angle_delta(old_yaw, ps.view_angles[1]);
    if back.abs() > 1.0 {
        return;
    }
    let turned = fpu::angle_delta(new_yaw, ps.view_angles[1]);
    if d(turned) * d(back) <= 0.0 {
        return;
    }
    // The view takes the kept product, `delta_angles` the stored one.
    let ease = d(back) * d(PRONE_REFUSED_EASE);
    ps.view_angles[1] = fpu::angle_normalize360((d(ps.view_angles[1]) + ease) as f32);
    ps.delta_angles[1] = (d(ease as f32) + d(ps.delta_angles[1])) as f32;
}

/// `PM_UpdateLean` (0x80567f4): the keys count on the ground or linked and
/// not on a turret, the lean steps out over 350 ms and back over 280 to
/// 0.5 (0.25 at the prone target), and a ±8 box from the eye to a full
/// lean holds it to what a wall leaves.
fn update_lean(pm: &mut Pmove, msec: f32) {
    let ps = &mut *pm.ps;
    let keys = pm.cmd.buttons;
    let counted = keys & (button::LEAN_LEFT | button::LEAN_RIGHT) != 0
        && ps.pm_flags & pmf::FROZEN == 0
        && ps.pm_type < pm_type::DEAD
        && (ps.ground_entity_num != ENTITYNUM_NONE || ps.pm_type == pm_type::NORMAL_LINKED)
        && ps.e_flags & ef::TURRET == 0;
    let side = if counted {
        i32::from(keys & button::LEAN_RIGHT != 0) - i32::from(keys & button::LEAN_LEFT != 0)
    } else {
        0
    };
    let full = d(if ps.view_height_target == VIEW_PRONE {
        LEAN_PRONE
    } else {
        LEAN_FULL
    });
    let lean = d(ps.leanf);
    let out = d(msec) / LEAN_TO_MS * full;
    let back = d(msec) / LEAN_FROM_MS * full;
    // Outward the step stops at the reach and a lean past it is put on it;
    // back, the step stops at 0.
    let next = match side {
        1 => (if lean < full { lean + out } else { lean }).min(full),
        -1 => (if lean > -full { lean - out } else { lean }).max(-full),
        _ if lean > 0.0 => (lean - back).max(0.0),
        _ if lean < 0.0 => (lean + back).min(0.0),
        _ => lean,
    };
    ps.leanf = next as f32;
    if ps.leanf == 0.0 {
        return;
    }
    let sign = if ps.leanf < 0.0 { -1.0 } else { 1.0 };
    let eye = Vec3::new(
        ps.origin.x,
        ps.origin.y,
        (d(ps.origin.z) + d(ps.view_height_current)) as f32,
    );
    let yaw = vcod_common::x87::radians(ps.view_angles[1]);
    let leaned =
        vcod_common::pmove::aim::add_lean(eye, yaw, sign, vcod_common::patch::Arith::Stored);
    let wall = pm.raw_trace(
        eye,
        leaned,
        Vec3::splat(-8.0),
        Vec3::splat(8.0),
        super::MASK_PLAYERSOLID,
    );
    let room = vcod_common::pmove::aim::unget_lean_fraction(wall.fraction);
    if pm.ps.leanf.abs() > room {
        pm.ps.leanf = room * sign;
    }
}

/// `PM_CheckDuck`'s prone test (0x8058f20, also built alone at 0x8056a7e):
/// a body already prone may stay; one on the ground may lie down where it
/// fits at the view's yaw. 1.7 also refuses a weapon whose def has +0x54c
/// set (0x80609de), which the model does not read.
fn may_lie_down(pm: &mut Pmove) -> bool {
    if pm.ps.pm_flags & pmf::PRONE != 0 {
        return true;
    }
    if pm.ps.ground_entity_num == ENTITYNUM_NONE {
        return false;
    }
    check_prone(
        pm,
        ProneCheck {
            origin: pm.ps.origin,
            size: pm.maxs.x,
            height: HEIGHT_PRONE,
            yaw: pm.ps.view_angles[1],
            already_prone: false,
            on_ground: true,
            walkable: true,
            feet_dist: PRONE_FEET_DIST,
            outputs: true,
        },
    )
}

/// The `pm_flags` stance bits of `stance`.
fn stance_flags(stance: Stance) -> u32 {
    match stance {
        Stance::Prone => pmf::PRONE,
        Stance::Crouch => pmf::DUCKED,
        Stance::Stand => 0,
    }
}

fn set_stance(ps: &mut PlayerState, stance: Stance) {
    ps.pm_flags = (ps.pm_flags & !(pmf::PRONE | pmf::DUCKED)) | stance_flags(stance);
}

/// `PM_CheckDuck` (0x80588c6): the stance keys against the room each
/// stance needs, the eye's target, its lerp, then the box, `eFlags` and
/// `pm_flags` off the stance the eye reached.
pub(crate) fn check_duck(pm: &mut Pmove, pml: &mut Pml) {
    pm.prone_change = false;
    let mask = pm.tracemask & !super::CONTENTS_BODY;
    if pm.ps.pm_type == pm_type::SPECTATOR {
        pm.mins = Vec3::splat(-8.0);
        pm.maxs = Vec3::new(8.0, 8.0, 16.0);
        set_stance(pm.ps, Stance::Stand);
        if pm.cmd.buttons & button::PRONE != 0 {
            pm.cmd.buttons &= !button::PRONE;
            pm.ps.add_event(ev::STANCE_FORCE_STAND, 0);
        }
        pm.ps.view_height_target = 0;
        pm.ps.view_height_current = 0.0;
        return;
    }
    let was_prone = pm.ps.pm_flags & pmf::PRONE != 0;
    pm.mins = super::PLAYER_MINS;
    pm.maxs = super::PLAYER_MAXS;
    if pm.ps.pm_type == pm_type::DEAD {
        pm.ps.view_height_target = VIEW_DEAD;
        view_height_adjust(pm.ps, pm.cmd.server_time, pml.frametime);
        return;
    }
    // A sprint stands the body up on every cmd (0x805894c, 0x8058dc4). 1.7
    // does the same, `eFlags` left alone, for `pm_flags` 0x100000
    // (0x805893e), which the model never sets.
    if pm.ps.pm_flags & pmf::SPRINTING != 0 {
        let ps = &mut *pm.ps;
        ps.view_height_target = VIEW_STAND;
        ps.e_flags &= !(ef::CROUCH | ef::PRONE);
        set_stance(ps, Stance::Stand);
        ps.add_event(ev::STANCE_FORCE_STAND, 0);
        view_height_adjust(pm.ps, pm.cmd.server_time, pml.frametime);
        return;
    }
    stance_keys(pm, mask);
    if pm.ps.view_height_lerp_time == 0 {
        pick_eye_target(pm);
    }
    view_height_adjust(pm.ps, pm.cmd.server_time, pml.frametime);
    let ps = &mut *pm.ps;
    let stance = ps.effective_stance();
    let (height, flags) = match stance {
        Stance::Prone => (HEIGHT_PRONE, ef::PRONE),
        Stance::Crouch => (HEIGHT_CROUCH, ef::CROUCH),
        Stance::Stand => (HEIGHT_STAND, 0),
    };
    pm.maxs.z = height;
    ps.e_flags = (ps.e_flags & !(ef::PRONE | ef::CROUCH)) | flags;
    set_stance(ps, stance);
    if pm.ps.pm_flags & pmf::PRONE != 0 && !was_prone {
        enter_prone(pm, mask);
    }
}

/// The stance the keys and the room leave in `pm_flags`
/// (0x8058952..0x80592e3). A gunner takes the turret's stance; a frozen or
/// respawned body keeps its own; last stand crouches. Otherwise the prone
/// key lies down where it may, the crouch key crouches (rising out of
/// prone where a crouched box fits), and no key rises as far as the room
/// lets. A refusal raises a forced-stance event unless the cmd holds the
/// stance key still. 1.7 also plays the stance's anim script event on each
/// change and keeps the stance for a weapon in state 5 whose def has
/// +0x5b8 set (0x805907e); the model does neither.
fn stance_keys(pm: &mut Pmove, mask: u32) {
    let ps = &mut *pm.ps;
    match ps.e_flags & ef::TURRET {
        0 => {}
        0x100 => return set_stance(ps, Stance::Prone),
        0x200 => return set_stance(ps, Stance::Crouch),
        _ => return set_stance(ps, Stance::Stand),
    }
    if ps.pm_flags & (pmf::RESPAWNED | pmf::FROZEN) != 0 {
        return;
    }
    if ps.pm_type == pm_type::LASTSTAND {
        return set_stance(ps, Stance::Crouch);
    }
    if ps.pm_flags & pmf::LADDER != 0 && pm.cmd.buttons & (button::PRONE | button::CROUCH) != 0 {
        pm.cmd.buttons &= !(button::PRONE | button::CROUCH);
        ps.add_event(ev::STANCE_FORCE_STAND, 0);
    }
    let keys = pm.cmd.buttons;
    let held = keys & button::STANCE_HELD != 0;
    let prone = ps.pm_flags & pmf::PRONE != 0;
    let ducked = ps.pm_flags & pmf::DUCKED != 0;
    if keys & button::PRONE != 0 {
        if may_lie_down(pm) {
            set_stance(pm.ps, Stance::Prone);
        } else if pm.ps.ground_entity_num != ENTITYNUM_NONE {
            pm.ps.pm_flags |= pmf::PRONE_REFUSED;
            if !held {
                let lower = prone || ducked;
                let e = if lower {
                    ev::STANCE_FORCE_CROUCH
                } else {
                    ev::STANCE_FORCE_STAND
                };
                pm.ps.add_event(e, 0);
            }
        }
        return;
    }
    // Each rise: the stances tried in turn, then the refusal's event.
    let (tries, refusal): (&[Stance], _) = match (keys & button::CROUCH != 0, prone, ducked) {
        (true, false, _) => return set_stance(pm.ps, Stance::Crouch),
        (true, true, _) => (&[Stance::Crouch], (ev::STANCE_FORCE_PRONE, 2)),
        (false, true, _) => (
            &[Stance::Stand, Stance::Crouch],
            (ev::STANCE_FORCE_PRONE, 1),
        ),
        (false, false, true) => (&[Stance::Stand], (ev::STANCE_FORCE_CROUCH, 1)),
        (false, false, false) => return,
    };
    for &stance in tries {
        pm.maxs.z = if stance == Stance::Stand {
            HEIGHT_STAND
        } else {
            HEIGHT_CROUCH
        };
        let o = pm.ps.origin;
        if !pm.raw_trace(o, o, pm.mins, pm.maxs, mask).allsolid {
            return set_stance(pm.ps, stance);
        }
    }
    if !held {
        pm.ps.add_event(refusal.0, refusal.1);
    }
}

/// The eye's next target while no leg runs (0x8058982..0x8058a04,
/// 0x8058e3a..0x8058f1b): one stance at a time, last stand at 22. Going
/// prone or leaving it marks the cmd's `proneChange`; going prone also
/// starts `Jump_ActivateSlowdown` (0x80507a8) when no timer runs.
fn pick_eye_target(pm: &mut Pmove) {
    let ps = &mut *pm.ps;
    let now = ps.view_height_target;
    let next = if ps.pm_type == pm_type::LASTSTAND {
        VIEW_LASTSTAND
    } else if ps.pm_flags & pmf::PRONE != 0 {
        if now == VIEW_STAND {
            VIEW_CROUCH
        } else {
            VIEW_PRONE
        }
    } else if now == VIEW_PRONE || ps.pm_flags & pmf::DUCKED != 0 {
        VIEW_CROUCH
    } else {
        VIEW_STAND
    };
    if (now == VIEW_PRONE) != (next == VIEW_PRONE) {
        pm.prone_change = true;
        if next == VIEW_PRONE && ps.pm_time == 0 {
            ps.pm_flags |= pmf::JUMPING;
            ps.pm_time = 1800;
        }
    }
    ps.view_height_target = next;
}

/// The first frame prone: a move key leaves the sight; the body settles up
/// 10 and back down; the heading takes the view's yaw, the ground's pitch
/// along it the plane under, and the torso that pitch held within 45 of the
/// view.
fn enter_prone(pm: &mut Pmove, mask: u32) {
    if pm.cmd.forward_move != 0 || pm.cmd.right_move != 0 {
        pm.ps.pm_flags &= !(pmf::PRONE_STILL | pmf::SIGHT);
    }
    let o = pm.ps.origin;
    let up = Vec3::new(o.x, o.y, (d(o.z) + d(PRONE_SETTLE_UP)) as f32);
    let t = pm.raw_trace(o, up, pm.mins, pm.maxs, mask);
    let top = fpu::lerp(o, up, t.fraction);
    let t = pm.raw_trace(top, o, pm.mins, pm.maxs, mask);
    pm.ps.origin = fpu::lerp(top, o, t.fraction);
    pm.ps.prone_direction = pm.ps.view_angles[1];
    let o = pm.ps.origin;
    let probe = Vec3::new(o.x, o.y, (d(o.z) - d(PRONE_GROUND_PROBE)) as f32);
    let t = pm.raw_trace(o, probe, pm.mins, pm.maxs, mask);
    pm.ps.prone_direction_pitch = if t.startsolid || t.fraction >= 1.0 {
        0.0
    } else {
        vcod_common::pmove::pitch_for_yaw_on_normal(pm.ps.prone_direction, t.normal)
    };
    let ps = &mut *pm.ps;
    let tilt = fpu::angle_delta(ps.prone_direction_pitch, ps.view_angles[0]);
    ps.prone_torso_pitch = if tilt < -PRONE_PITCHCAP {
        (d(ps.view_angles[0]) - d(PRONE_PITCHCAP)) as f32
    } else if tilt > PRONE_PITCHCAP {
        (d(ps.view_angles[0]) + d(PRONE_PITCHCAP)) as f32
    } else {
        ps.prone_direction_pitch
    };
}

/// The prone pitches' rate in degrees a second.
const PRONE_PITCH_DEG_PER_SEC: f64 = 70.0;

/// `PM_UpdatePronePitch`, every prone frame: off the ground the body must
/// still fit (else `EV_STANCE_FORCE_CROUCH` and `PMF_PRONE_REFUSED`), on an
/// unwalkable plane it is forced to crouch; then the body's and the
/// torso's pitch ease at 70 a second toward the ground's pitch along the
/// body's heading and the view's yaw (0 with no ground plane).
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
                size: 15.0,
                height: HEIGHT_PRONE,
                yaw: pm.ps.prone_direction,
                already_prone: true,
                on_ground: false,
                walkable: !pml.ground_plane || pml.ground_trace.walkable,
                feet_dist: PRONE_FEET_DIST,
                outputs: true,
            },
        );
        if !fits {
            pm.ps.add_event(ev::STANCE_FORCE_CROUCH, 0);
            pm.ps.pm_flags |= pmf::PRONE_REFUSED;
        }
    } else if pml.ground_plane && !pml.ground_trace.walkable {
        pm.ps.add_event(ev::STANCE_FORCE_CROUCH, 0);
    }
    let step = d(pml.frametime) * PRONE_PITCH_DEG_PER_SEC;
    let ease = |cur: f32, yaw: f32| -> f32 {
        let target = normal.map_or(0.0, |n| vcod_common::pmove::pitch_for_yaw_on_normal(yaw, n));
        let delta = fpu::angle_delta(target, cur);
        if delta == 0.0 {
            return cur;
        }
        let next = if d(delta.abs()) <= step {
            (d(cur) + d(delta)) as f32
        } else {
            let s = if delta < 0.0 { -1.0 } else { 1.0 };
            (step * s + d(cur)) as f32
        };
        fpu::angle_normalize180(next)
    };
    let ps = &mut *pm.ps;
    ps.prone_direction_pitch = ease(ps.prone_direction_pitch, ps.prone_direction);
    ps.prone_torso_pitch = ease(ps.prone_torso_pitch, ps.view_angles[1]);
}

#[cfg(test)]
mod tests;
