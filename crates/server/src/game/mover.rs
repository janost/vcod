//! The scriptent mover verbs' state and integrator.
//!
//! Everything here is measured, not guessed: `docs/research/cod11-movers.md`
//! is one paired capture against the retail 1.1d server, the per-frame
//! `getorigin()` on one side and the `pos`/`apos` trajectory groups on the
//! other. The three facts that shape this module are its sections 4 and 9.
//!
//! A verb builds a **plan**: a queue of [`Trajectory`] segments, one per
//! phase of the trapezoidal velocity profile, plus the notify to raise when
//! the last one ends. Retail sends exactly those segments and nothing
//! between them, so the plan is both the simulation and the wire format;
//! there is no second representation to keep in sync.
//!
//! Each frame the integrator retires the segment whose duration has elapsed,
//! starts the next from the position the last one reached, and writes the
//! evaluated origin and angles back into the entity's own fields, which is
//! what the capture shows retail doing: `getorigin()` and `.angles` both read
//! the interpolated value on every frame of a move.
//!
//! A brush model's clip follows its trajectory one frame ahead of what
//! script reads, on retail's `G_MoverTeam` clock, and each move is handed to
//! the server as a [`Step`] whose riders and blocked players it pushes
//! (docs/research/cod11-movers.md, sections 11 and 12).

use crate::game::host::GameHost;
use glam::Vec3;
use vcod_common::net::trajectory::{
    TR_ACCELERATE, TR_DECCELERATE, TR_GRAVITY, TR_LINEAR_STOP, TR_STATIONARY, Trajectory,
};
use vcod_gsc::EntId;
use vcod_gsc::Host;

/// `"movedone"`, what the four linear families and `movegravity` raise
/// (movers doc, section 7).
pub const MOVEDONE: &str = "movedone";
/// `"rotatedone"`, the angular half's.
pub const ROTATEDONE: &str = "rotatedone";

/// One group's motion: the trajectory on the wire now, the segments still to
/// come, and whether finishing them owes a notify.
#[derive(Clone, Debug, Default, PartialEq)]
struct Plan {
    current: Trajectory,
    queue: std::collections::VecDeque<Trajectory>,
    /// `Some` while a verb's segments are still running.
    notify: Option<&'static str>,
    /// Whether a verb has ever run on this group. A rotate leaves `pos`
    /// untouched, and writing an unstarted group's zero trajectory back onto
    /// the entity would teleport it to the origin.
    started: bool,
    /// Where a bounded verb settles, which is its destination rather than
    /// where its segments run out when the two differ (`Movers::move_to`).
    dest: Option<Vec3>,
}

impl Plan {
    /// Where the plan has the group at `t`, without retiring anything: the
    /// segments chained the way [`Plan::advance`] will chain them.
    fn at(&self, t: i32) -> Vec3 {
        let mut cur = self.current;
        let mut queue = self.queue.iter();
        if self.notify.is_none() {
            return cur.evaluate(t);
        }
        loop {
            let end = cur.tr_time + cur.tr_duration;
            if t < end {
                return cur.evaluate(t);
            }
            match queue.next() {
                Some(next) => {
                    let base = cur.evaluate(end);
                    cur = Trajectory {
                        tr_time: end,
                        base,
                        ..*next
                    };
                }
                None if cur.tr_type == TR_GRAVITY => return cur.evaluate(t),
                None => return cur.evaluate(end),
            }
        }
    }

    /// `G_MoverTeam`'s blocked arm: the running segment starts a frame later,
    /// so the group holds where it was for this frame. Retail shifts both
    /// groups whether or not they are moving: the stationary `apos` of the
    /// crush capture advances its `trTime` 50 a frame too (movers doc,
    /// section 12).
    fn stall(&mut self) {
        self.current.tr_time += crate::server::FRAME_MS;
    }
}

impl Plan {
    /// Starts a verb: the queue replaces whatever was running. A second verb
    /// on a moving entity taking over rather than queueing behind it is
    /// UNVERIFIED (movers doc, section 10); it is the reading that keeps one
    /// group to one trajectory, which is all the wire can carry. The caller
    /// sets `current` stationary where the verb starts first, and the
    /// destination after.
    fn start(&mut self, now_ms: i32, segments: Vec<Trajectory>, notify: &'static str) {
        self.dest = None;
        let mut q: std::collections::VecDeque<Trajectory> = segments.into();
        let Some(mut first) = q.pop_front() else {
            return;
        };
        first.tr_time = now_ms;
        first.base = self.current.evaluate(now_ms);
        self.current = first;
        self.queue = q;
        self.notify = Some(notify);
        self.started = true;
    }

    /// Retires every segment whose duration has elapsed by `now_ms`. Returns
    /// the notify owed, once, on the frame the last one ends.
    fn advance(&mut self, now_ms: i32) -> Option<&'static str> {
        while self.notify.is_some() {
            let end = self.current.tr_time + self.current.tr_duration;
            if now_ms < end {
                return None;
            }
            match self.queue.pop_front() {
                Some(mut next) => {
                    next.tr_time = end;
                    next.base = self.current.evaluate(end);
                    self.current = next;
                }
                None => return self.finish(end),
            }
        }
        None
    }

    /// The last segment ended at `end`. A bounded move settles to stationary
    /// at the point it reached, which is what the capture shows; a
    /// `movegravity` does not, because retail's own entity was still
    /// accelerating downward on the frame its notify came (movers doc,
    /// section 5).
    fn finish(&mut self, end: i32) -> Option<&'static str> {
        if self.current.tr_type != TR_GRAVITY {
            self.current = Trajectory {
                tr_type: TR_STATIONARY,
                tr_time: end,
                base: self.dest.unwrap_or_else(|| self.current.evaluate(end)),
                ..self.current
            };
        }
        self.notify.take()
    }
}

/// Every entity a mover verb has been called on, and what it is doing.
#[derive(Default)]
pub struct Movers {
    rows: std::collections::HashMap<EntId, Mover>,
}

#[derive(Clone, Debug, Default, PartialEq)]
struct Mover {
    pos: Plan,
    apos: Plan,
    /// Where the clip of a brush model mover was put last frame, origin and
    /// angles; `None` for anything else and before its first frame.
    clip: Option<(Vec3, Vec3)>,
}

/// One frame's move of a brush model mover, for the server to push players
/// with: `G_MoverPush`'s `move` and `amove` are `to - from`
/// (docs/research/cod11-movers.md, section 12).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Step {
    pub ent: EntId,
    /// The entity number a player standing on the brushes reads as ground.
    pub number: u32,
    /// The lump-27 model its brushes are.
    pub model: usize,
    pub from: (Vec3, Vec3),
    pub to: (Vec3, Vec3),
}

/// One notify the integrator owes script this frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Done {
    pub ent: EntId,
    pub event: &'static str,
}

impl Movers {
    /// The trajectory pair to put on the wire at level time `t`, or `None`
    /// for an entity no verb has ever touched. The plans retire segments on
    /// script's clock, a frame behind; the wire is on `G_MoverTeam`'s, so a
    /// move reads stationary on the snapshot of the frame it ends, as the
    /// ride capture's trajectory lines show (movers doc, section 14).
    pub fn wire(&self, id: EntId, t: i32) -> Option<(Trajectory, Trajectory)> {
        let at = |p: &Plan| {
            let mut p = p.clone();
            p.advance(t);
            p.current
        };
        self.rows.get(&id).map(|m| (at(&m.pos), at(&m.apos)))
    }

    pub fn forget(&mut self, id: EntId) {
        self.rows.remove(&id);
    }

    /// The origin and angles a mover's plans put it at `t`, ahead of what
    /// script reads, `None` for a group no verb has run on.
    pub fn pose_at(&self, id: EntId, t: i32) -> Option<(Option<Vec3>, Option<Vec3>)> {
        let m = self.rows.get(&id)?;
        let at = |p: &Plan| p.started.then(|| p.at(t));
        Some((at(&m.pos), at(&m.apos)))
    }

    /// A push the step asked for was blocked: the mover holds this frame
    /// where it was, every trajectory a frame later, and its clip goes back
    /// to `step.from`.
    pub fn stall(&mut self, step: &Step) {
        if let Some(m) = self.rows.get_mut(&step.ent) {
            m.pos.stall();
            m.apos.stall();
            m.clip = Some(step.from);
        }
    }

    /// `moveto` and the three axis verbs, which differ only in how the
    /// caller builds `dest` (movers doc, section 3: the axis verbs take a
    /// delta, `moveto` a destination).
    ///
    /// The segments' velocities come from `from`, the origin script reads,
    /// but a verb on a moving entity starts its `trBase` where the running
    /// trajectory has it at `now_ms`, a frame further on: the ride
    /// capture's `moveto` on the descending slab sent `trBase` z 28 with a
    /// velocity of -30 from the 30 script read, ran out 2 units short and
    /// settled on `dest` (movers doc, section 14).
    pub fn move_to(&mut self, id: EntId, now_ms: i32, from: Vec3, dest: Vec3, m: Ramp) {
        let row = self.rows.entry(id).or_default();
        let base = if row.pos.started {
            row.pos.at(now_ms)
        } else {
            from
        };
        row.pos.current = stationary(base);
        row.pos.start(now_ms, m.segments(dest - from), MOVEDONE);
        row.pos.dest = Some(dest);
    }

    /// `rotateto` and the three axis verbs. The delta is taken per component
    /// rather than along the shortest arc: every measured case is a single
    /// axis, where the two agree, and a multi-axis `rotateto` is UNVERIFIED.
    /// Started like [`Self::move_to`], which is INFERRED for the angles: no
    /// capture holds a rotate called on an entity already turning.
    pub fn rotate_to(&mut self, id: EntId, now_ms: i32, from: Vec3, dest: Vec3, m: Ramp) {
        let row = self.rows.entry(id).or_default();
        let base = if row.apos.started {
            row.apos.at(now_ms)
        } else {
            from
        };
        row.apos.current = stationary(base);
        row.apos.start(now_ms, m.segments(dest - from), ROTATEDONE);
        row.apos.dest = Some(dest);
    }

    /// `movegravity(velocity, seconds)`: one unbounded-shape segment on a
    /// gravity trajectory, bounded only in when the notify comes.
    pub fn move_gravity(&mut self, id: EntId, now_ms: i32, from: Vec3, velocity: Vec3, secs: f32) {
        let row = self.rows.entry(id).or_default();
        row.pos.current = stationary(from);
        row.pos.start(
            now_ms,
            vec![Trajectory {
                tr_type: TR_GRAVITY,
                tr_time: now_ms,
                tr_duration: ms(secs),
                base: from,
                delta: velocity,
            }],
            MOVEDONE,
        );
    }

    /// `rotatevelocity(degreesPerSecond, seconds, accel, decel)`. Retail's
    /// own capture ended a `(0,180,0)` over 2 s at 360 degrees, so the verb
    /// is `rotateto` with the target the velocity times the duration; that
    /// the ramps then preserve the total angle rather than the peak rate is
    /// INFERRED from the two sharing one handler shape.
    pub fn rotate_velocity(&mut self, id: EntId, now_ms: i32, from: Vec3, v: Vec3, m: Ramp) {
        let dest = from + v * m.secs;
        self.rotate_to(id, now_ms, from, dest, m);
    }
}

/// One frame of every mover: retire the segments whose duration has elapsed,
/// write the evaluated origin and angles back onto each entity, and hand back
/// the notifies owed.
///
/// It runs one server frame behind the trajectory, which is measured rather
/// than chosen: retail's `moveto((600,0,100), 1)` called at level time 1050
/// put `trTime` 1050 on the wire and yet `getorigin()` still read the start
/// origin at 1100 and the first moved value at 1150, and `movedone` came at
/// 2100 rather than 2050. So what script sees is the trajectory evaluated at
/// the previous frame, and the wire carries the frame script has not caught
/// up to. Both halves of the capture agree on it
/// (docs/research/cod11-movers.md, section 8).
///
/// The write-back is what the capture shows retail doing -- `getorigin()` and
/// `.angles` both read the interpolated value on every frame of a move -- and
/// it is also what keeps this to one representation: `istouching`, the touch
/// pass, `getorigin` and the wire build all go on reading the entity's own
/// fields. Angles are normalized into 0..360 the way the script side reads
/// them; the wire keeps the raw value (movers doc, section 8).
pub fn run(host: &mut GameHost, cx: &mut vcod_gsc::Cx) -> Vec<Done> {
    let level_ms = host.level_time_ms;
    let now_ms = level_ms - crate::server::FRAME_MS;
    let mut movers = std::mem::take(&mut host.movers);
    movers.rows.retain(|id, _| host.ents.get(*id).is_some());

    let mut done = Vec::new();
    let mut ids: Vec<EntId> = movers.rows.keys().copied().collect();
    ids.sort_by_key(|i| i.0);
    for id in ids {
        let m = movers.rows.get_mut(&id).expect("just listed");
        for plan in [&mut m.pos, &mut m.apos] {
            if let Some(e) = plan.advance(now_ms) {
                done.push(Done { ent: id, event: e });
            }
        }
        let norm = |a: f32| a.rem_euclid(360.0);
        let origin = m.pos.current.evaluate(now_ms);
        let angles = m.apos.current.evaluate(now_ms);
        let fields = [
            (m.pos.started, "origin", [origin.x, origin.y, origin.z]),
            (
                m.apos.started,
                "angles",
                [norm(angles.x), norm(angles.y), norm(angles.z)],
            ),
        ];
        for (started, name, v) in fields {
            if !started {
                continue;
            }
            let atom = cx.intern_folded(name);
            let _ = host.set_field(cx, id, atom, vcod_gsc::Value::Vector(v));
        }
        clip_step(host, cx, id, m, level_ms);
    }

    host.movers = movers;
    done
}

/// A brush model mover's clip, moved to where its plans have it at the
/// level time and handed on as a [`Step`] when it moved. That is
/// `G_MoverTeam`'s clock: the entity pass runs after the script frame, so
/// the clip is a frame ahead of the `getorigin()` above (movers doc,
/// sections 8 and 11).
fn clip_step(host: &mut GameHost, cx: &mut vcod_gsc::Cx, id: EntId, m: &mut Mover, level_ms: i32) {
    let Some(model) = submodel(host, id) else {
        return;
    };
    let Some(world) = host.world.clone() else {
        return;
    };
    let field = |host: &mut GameHost, cx: &mut vcod_gsc::Cx, name: &str| {
        let atom = cx.intern_folded(name);
        match host.get_field(cx, id, atom) {
            vcod_gsc::Value::Vector(v) => Vec3::from(v),
            _ => Vec3::ZERO,
        }
    };
    let origin = field(host, cx, "origin");
    let angles = field(host, cx, "angles");
    let at = |m: &Mover, t: i32| {
        (
            if m.pos.started { m.pos.at(t) } else { origin },
            if m.apos.started { m.apos.at(t) } else { angles },
        )
    };
    let to = at(m, level_ms);
    let from = m
        .clip
        .unwrap_or_else(|| at(m, level_ms - crate::server::FRAME_MS));
    m.clip = Some(to);
    if from == to {
        return;
    }
    world.collision.set_model_pose(model, to.0, to.1);
    if !world.collision.model_linked(model) {
        return;
    }
    host.mover_steps.push(Step {
        ent: id,
        number: id.0,
        model,
        from,
        to,
    });
}

/// The `N` of an entity whose BSP `model` key is `"*N"`, N > 0.
fn submodel(host: &GameHost, id: EntId) -> Option<usize> {
    host.ents
        .get(id)?
        .brush_model
        .map(|n| n as usize)
        .filter(|&n| n > 0)
}

/// A verb's duration and its two ramps, all in seconds, and the segment
/// builder they share (movers doc, section 4).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ramp {
    pub secs: f32,
    pub accel: f32,
    pub decel: f32,
}

impl Ramp {
    pub fn new(secs: f32, accel: f32, decel: f32) -> Self {
        let secs = secs.max(0.0);
        // Retail's own clamp is unmeasured; ramps that do not fit inside the
        // duration would give a negative cruise time and a segment that runs
        // backwards, so they are trimmed rather than trusted.
        let (mut accel, mut decel) = (accel.max(0.0), decel.max(0.0));
        if accel + decel > secs {
            let scale = if accel + decel > 0.0 {
                secs / (accel + decel)
            } else {
                0.0
            };
            accel *= scale;
            decel *= scale;
        }
        Ramp { secs, accel, decel }
    }

    /// The trapezoid for a total displacement `d`: cruise velocity
    /// `d / (T - ta/2 - td/2)`, an accelerating segment of `ta`, a linear one
    /// for the rest, and a decelerating one of `td`. `tr_time` and `base` are
    /// filled in by [`Plan`] as each segment starts.
    fn segments(&self, d: Vec3) -> Vec<Trajectory> {
        let effective = self.secs - self.accel / 2.0 - self.decel / 2.0;
        let delta = if effective > 0.0 {
            d / effective
        } else {
            Vec3::ZERO
        };
        let cruise = self.secs - self.accel - self.decel;
        let mut out = Vec::new();
        let mut push = |tr_type: i32, secs: f32| {
            if secs > 0.0 {
                out.push(Trajectory {
                    tr_type,
                    tr_time: 0,
                    tr_duration: ms(secs),
                    base: Vec3::ZERO,
                    delta,
                });
            }
        };
        push(TR_ACCELERATE, self.accel);
        push(TR_LINEAR_STOP, cruise);
        push(TR_DECCELERATE, self.decel);
        if out.is_empty() {
            // A zero-duration verb still has to settle at the destination
            // and raise its notify, which one empty segment does.
            out.push(Trajectory {
                tr_type: TR_LINEAR_STOP,
                ..Trajectory::default()
            });
        }
        out
    }
}

/// A group at rest at `at`, what a verb starts from.
fn stationary(at: Vec3) -> Trajectory {
    Trajectory {
        tr_type: TR_STATIONARY,
        base: at,
        ..Trajectory::default()
    }
}

/// Seconds as the milliseconds a `trDuration` is in.
fn ms(secs: f32) -> i32 {
    (secs.max(0.0) * 1000.0).round() as i32
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The trapezoid retail measured: `movez(96, 2, 0.5, 0.5)` is three
    /// segments of 500/1000/500 ms, all carrying the cruise velocity 64, and
    /// they cover 16, 64 and 16 units (movers doc, section 9).
    #[test]
    fn a_ramped_move_is_the_three_segments_retail_sends() {
        let segs = Ramp::new(2.0, 0.5, 0.5).segments(Vec3::new(0.0, 0.0, 96.0));
        let kinds: Vec<(i32, i32, f32)> = segs
            .iter()
            .map(|t| (t.tr_type, t.tr_duration, t.delta.z))
            .collect();
        assert_eq!(
            kinds,
            vec![
                (TR_ACCELERATE, 500, 64.0),
                (TR_LINEAR_STOP, 1000, 64.0),
                (TR_DECCELERATE, 500, 64.0),
            ]
        );
    }

    /// A move with no ramps is one linear segment at `distance / duration`,
    /// and the accel-only case is what says the two ends are independent:
    /// `500 / (2 - 0.25)` is 285.71, not `500 / 1.5`.
    #[test]
    fn the_cruise_speed_is_the_measured_formula() {
        let plain = Ramp::new(2.0, 0.0, 0.0).segments(Vec3::new(500.0, 0.0, 0.0));
        assert_eq!(plain.len(), 1);
        assert_eq!(plain[0].tr_type, TR_LINEAR_STOP);
        assert!((plain[0].delta.x - 250.0).abs() < 0.01);

        let accel_only = Ramp::new(2.0, 0.5, 0.0).segments(Vec3::new(500.0, 0.0, 0.0));
        assert_eq!(accel_only.len(), 2);
        assert!(
            (accel_only[0].delta.x - 285.714).abs() < 0.01,
            "{}",
            accel_only[0].delta.x
        );
    }

    /// The segments are chained through their own evaluation, so a ramped
    /// move lands exactly on its destination and settles stationary there,
    /// and the notify comes on the frame the last one ends.
    #[test]
    fn a_ramped_move_lands_on_its_destination_and_notifies_once() {
        let mut plan = Plan::default();
        plan.current.base = Vec3::new(0.0, 0.0, 592.0);
        plan.start(
            1000,
            Ramp::new(2.0, 0.5, 0.5).segments(Vec3::new(0.0, 0.0, 96.0)),
            MOVEDONE,
        );

        // The three boundaries retail's own capture carries.
        for (at, z, kind) in [
            (1000, 592.0, TR_ACCELERATE),
            (1500, 608.0, TR_LINEAR_STOP),
            (2500, 672.0, TR_DECCELERATE),
        ] {
            assert_eq!(plan.advance(at), None, "not done at {at}");
            assert_eq!(plan.current.tr_type, kind, "at {at}");
            assert!(
                (plan.current.base.z - z).abs() < 0.01,
                "at {at}: base {}",
                plan.current.base.z
            );
        }
        assert_eq!(plan.advance(3000), Some(MOVEDONE));
        assert_eq!(plan.current.tr_type, TR_STATIONARY);
        assert!((plan.current.base.z - 688.0).abs() < 0.01);
        assert_eq!(plan.advance(4000), None, "the notify comes once");
    }

    /// `at` reads ahead without retiring anything, chaining the segments the
    /// way `advance` will; a stall holds the running segment a frame, and
    /// the notify comes that much later (movers doc, section 12).
    #[test]
    fn a_stall_holds_the_plan_a_frame() {
        let mut plan = Plan::default();
        plan.current.base = Vec3::new(0.0, 0.0, 592.0);
        plan.start(
            1000,
            Ramp::new(2.0, 0.5, 0.5).segments(Vec3::new(0.0, 0.0, 96.0)),
            MOVEDONE,
        );
        assert!(
            (plan.at(2500).z - 672.0).abs() < 0.01,
            "{}",
            plan.at(2500).z
        );
        assert!((plan.at(5000).z - 688.0).abs() < 0.01);
        assert_eq!(plan.current.tr_type, TR_ACCELERATE, "at retired nothing");

        let before = plan.at(1200);
        plan.stall();
        assert_eq!(plan.at(1250), before);
        assert_eq!(plan.advance(3000), None);
        assert_eq!(plan.advance(3050), Some(MOVEDONE));
    }

    /// `movegravity` is the one verb that does not settle: retail's entity
    /// was still accelerating downward on the frame its notify came, so the
    /// trajectory stays gravity and the position keeps running.
    #[test]
    fn movegravity_notifies_without_stopping() {
        let mut movers = Movers::default();
        let id = EntId(72, 0);
        movers.move_gravity(
            id,
            0,
            Vec3::new(0.0, 0.0, 100.0),
            Vec3::new(0.0, 0.0, 300.0),
            3.0,
        );
        let (pos, _) = movers.wire(id, 0).unwrap();
        // z(t) = 100 + 300t - 400t^2, the retail trace to the digit.
        assert!((pos.evaluate(375).z - 156.25).abs() < 0.01);
        assert!((pos.evaluate(3000).z + 2600.0).abs() < 0.01);

        let row = movers.rows.get_mut(&id).unwrap();
        assert_eq!(row.pos.advance(3000), Some(MOVEDONE));
        assert_eq!(row.pos.current.tr_type, TR_GRAVITY);
        assert!((row.pos.current.evaluate(3500).z + 3750.0).abs() < 0.01);
    }
}
