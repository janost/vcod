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
use vcod_common::collision::Pusher;
use vcod_common::net::trajectory::{
    TR_ACCELERATE, TR_DECCELERATE, TR_GRAVITY, TR_LINEAR, TR_LINEAR_STOP, TR_SINE, TR_STATIONARY,
    Trajectory,
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
    /// 1.5b's and UO's `BG_EvaluateTrajectory` ([`bg_evaluate`]).
    stored: bool,
}

impl Plan {
    /// Where the plan has the group at `t`, without retiring anything: the
    /// segments chained the way [`Plan::advance`] will chain them.
    fn at(&self, t: i32) -> Vec3 {
        let mut cur = self.current;
        let mut queue = self.queue.iter();
        if self.notify.is_none() {
            return bg_evaluate(self.stored, &cur, t);
        }
        loop {
            let end = cur.tr_time + cur.tr_duration;
            if t < end {
                return bg_evaluate(self.stored, &cur, t);
            }
            match queue.next() {
                Some(next) => {
                    let base = bg_evaluate(self.stored, &cur, end);
                    cur = Trajectory {
                        tr_time: end,
                        base,
                        ..*next
                    };
                }
                None if cur.tr_type == TR_GRAVITY => {
                    return bg_evaluate(self.stored, &cur, t.min(gravity_stop(end)));
                }
                None => return bg_evaluate(self.stored, &cur, end),
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
        first.base = bg_evaluate(self.stored, &self.current, now_ms);
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
                    next.base = bg_evaluate(self.stored, &self.current, end);
                    self.current = next;
                }
                None => return self.finish(end, now_ms),
            }
        }
        None
    }

    /// The last segment ended at `end`, seen on the frame at `now_ms`. A
    /// bounded move settles to stationary at the point it reached, which is
    /// what the capture shows; a `movegravity` settles where it is at
    /// `now_ms`, the first frame past its time (0x5f93b, movers doc 5).
    fn finish(&mut self, end: i32, now_ms: i32) -> Option<&'static str> {
        let base = if self.current.tr_type == TR_GRAVITY {
            bg_evaluate(self.stored, &self.current, now_ms)
        } else {
            self.dest
                .unwrap_or_else(|| bg_evaluate(self.stored, &self.current, end))
        };
        self.current = Trajectory {
            tr_type: TR_STATIONARY,
            tr_time: end,
            base,
            ..self.current
        };
        self.notify.take()
    }
}

/// The level time a `movegravity` ending at `end` stops on: the first
/// frame at or past it, whose `Reached_ScriptMover` evaluates the arc there
/// (0x5f93b).
fn gravity_stop(end: i32) -> i32 {
    let f = crate::server::FRAME_MS;
    end.div_euclid(f) * f + if end.rem_euclid(f) == 0 { 0 } else { f }
}

/// `BG_EvaluateTrajectory` (game.mp 0x2c600) as the 1.1d build runs it:
/// a linear trajectory's `delta * dt + base` stays on the x87 stack until
/// the store, `dt` the milliseconds times `0.001f`, so the turn a frame
/// lands a float step off an `f32` evaluation (movers doc 12, "The
/// residual yaw"). A gravity arc is `Trajectory::evaluate_kept` (0x2c6c0),
/// every other type `Trajectory::evaluate`.
///
/// 1.5b and UO store `dt` to a float before the mul-add (1.5b
/// `0x1b8f5`), `stored`.
fn bg_evaluate(stored: bool, tr: &Trajectory, t: i32) -> Vec3 {
    if tr.tr_type == TR_GRAVITY {
        return if stored {
            tr.evaluate_stored(t)
        } else {
            tr.evaluate_kept(t)
        };
    }
    let dt = |t: i32| {
        let dt = f64::from(t - tr.tr_time) * f64::from(0.001_f32);
        if stored { f64::from(dt as f32) } else { dt }
    };
    let dt = match tr.tr_type {
        TR_LINEAR => dt(t),
        // Clamped to the end and to no less than 0 (0x2c673..0x2c69e).
        TR_LINEAR_STOP => dt(t.min(tr.tr_time + tr.tr_duration)).max(0.0),
        _ => return tr.evaluate(t),
    };
    let f = |base: f32, delta: f32| (f64::from(delta) * dt + f64::from(base)) as f32;
    Vec3::new(
        f(tr.base.x, tr.delta.x),
        f(tr.base.y, tr.delta.y),
        f(tr.base.z, tr.delta.z),
    )
}

/// Every entity a mover verb has been called on, and what it is doing.
#[derive(Default)]
pub struct Movers {
    rows: std::collections::HashMap<EntId, Mover>,
    /// A 1.5b or UO server's evaluation ([`bg_evaluate`]).
    pub stored: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
struct Mover {
    pos: Plan,
    apos: Plan,
    /// Where the clip of a brush model mover was put last frame, origin and
    /// angles; `None` for anything else and before its first frame.
    clip: Option<(Vec3, Vec3)>,
    /// A `func_bobbing` or `func_pendulum` ([`spawn_sine`]): its angles
    /// read raw, and the end of a period is `Reached_BinaryMover`'s
    /// `G_Error` unless a `useBy` moved its state ([`use_binary`]).
    sine: bool,
    /// A sine mover's `moverState` (`ent+0x174`), which only a `useBy`
    /// moves off `InitMover`'s 0 (cod11-combat.md 5.9, "Use_BinaryMover").
    binary: Binary,
    /// `ReturnToPos1`'s `nextthink`, armed when a 1-to-2 leg ends.
    return_at: Option<i32>,
    /// Spawned through `InitMover` (0x56454), which writes `eType` 5: the
    /// sine movers and a `func_static`.
    init_mover: bool,
    /// A `func_static` with `takedamage` (0x5828a, 0x582d0): `G_Damage`
    /// calls its `use`, `Use_Static`, in place of any damage.
    use_static: bool,
    /// The groups whose running segment ended this frame: one
    /// `Reached_ScriptMover` link each (0x5fa17, 0x5faba).
    reach_links: u8,
    /// Both plans as they stood before this frame retired a segment, and
    /// the notifies that owed: `G_MoverTeam` calls `reached` only after a
    /// push that went through (0x558a0..0x558e9), so a blocked push on the
    /// last frame of a move puts them back and the move runs on.
    unreached: Option<(Plan, Plan, Vec<&'static str>)>,
}

/// `SetMoverState`'s four door states (`MOVER_POS1`, `POS2`, `1TO2`,
/// `2TO1`: 0, 1, 3, 4), the ones a sine mover can reach.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Binary {
    #[default]
    Pos1,
    Pos2,
    OneToTwo,
    TwoToOne,
}

/// One frame's move of a brush model mover, for the server to push players
/// with: `G_MoverPush`'s `move` and `amove` are `to - from`
/// (docs/research/cod11-movers.md, section 12).
#[derive(Clone, Debug, PartialEq)]
pub struct Step {
    pub ent: EntId,
    /// The entity number a player standing on the brushes reads as ground.
    pub number: u32,
    /// Its clip: a lump-27 submodel's brushes, or the world copy a brush
    /// entity with no `*N` carries (movers doc 20).
    pub pusher: Pusher,
    pub from: (Vec3, Vec3),
    pub to: (Vec3, Vec3),
    /// `trap_EntitiesInBox(.., 0x2000180)` over the swept move, taken with
    /// the pusher unlinked: the push's candidates in area-tree order.
    pub listed: Vec<u32>,
    /// The pusher's box where it now is, which a listed entity's own link
    /// box must overlap before its trace runs (`G_MoverPush`, 0x5540e).
    pub moved_box: Option<(Vec3, Vec3)>,
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

    /// `id`'s row, a fresh one on this server's evaluation.
    fn row(&mut self, id: EntId) -> &mut Mover {
        let stored = self.stored;
        self.rows.entry(id).or_insert_with(|| Mover {
            pos: Plan {
                stored,
                ..Plan::default()
            },
            apos: Plan {
                stored,
                ..Plan::default()
            },
            ..Mover::default()
        })
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

    /// `moveto` and the three axis verbs, which differ only in how the
    /// caller builds `dest` (movers doc, section 3: the axis verbs take a
    /// delta, `moveto` a destination).
    ///
    /// The segments' velocities come from `from`, the origin script reads,
    /// but a verb on a moving entity starts its `trBase` where the running
    /// trajectory has it at `now_ms`, a frame further on: the ride
    /// capture's `moveto` on the descending slab sent `trBase` z 28 with a
    /// velocity of -30 from the 30 script read, ran out 2 units short and
    /// settled on `dest` (movers doc, section 14). Returns the start, which
    /// the verb writes into `r.currentOrigin` and links (0x5fb38): `getorigin()`
    /// reads it straight after the call (movers doc 12, "A blocked mover").
    pub fn move_to(&mut self, id: EntId, now_ms: i32, from: Vec3, dest: Vec3, m: Ramp) -> Vec3 {
        let row = self.row(id);
        let base = if row.pos.started {
            row.pos.at(now_ms)
        } else {
            from
        };
        row.pos.current = stationary(base);
        row.pos.start(now_ms, m.segments(dest - from), MOVEDONE);
        row.pos.dest = Some(dest);
        base
    }

    /// `rotateto` and the three axis verbs, per component from `from` to
    /// `dest` (`rotateto`'s caller folds `dest` within 180 of `from`, movers
    /// doc 23). Started like [`Self::move_to`], which is INFERRED for the angles: no
    /// capture holds a rotate called on an entity already turning.
    ///
    /// A stationary group starts from `from`, `r.currentAngles`, not from
    /// its settled `trBase`: the two differ by the quantization
    /// `Reached_ScriptMover` leaves (movers doc 12, "The residual yaw").
    /// Returns the start, which the verb's own link snaps into
    /// `r.currentAngles` ([`snap_angles`]).
    pub fn rotate_to(&mut self, id: EntId, now_ms: i32, from: Vec3, dest: Vec3, m: Ramp) -> Vec3 {
        let row = self.row(id);
        let base = if row.apos.notify.is_some() {
            row.apos.at(now_ms)
        } else {
            from
        };
        row.apos.current = stationary(base);
        row.apos.start(now_ms, m.segments(dest - from), ROTATEDONE);
        row.apos.dest = Some(dest);
        base
    }

    /// `movegravity(velocity, seconds)`: one unbounded-shape segment on a
    /// gravity trajectory, bounded only in when the notify comes.
    pub fn move_gravity(&mut self, id: EntId, now_ms: i32, from: Vec3, velocity: Vec3, secs: f32) {
        let row = self.row(id);
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
    pub fn rotate_velocity(
        &mut self,
        id: EntId,
        now_ms: i32,
        from: Vec3,
        v: Vec3,
        m: Ramp,
    ) -> Vec3 {
        let dest = from + v * m.secs;
        self.rotate_to(id, now_ms, from, dest, m)
    }
}

/// `G_RunMover` for one entity on its turn in the entity pass: retire the
/// segments whose duration has elapsed, write the origin and angles at the
/// level time back onto the entity, move a brush model's clip, and hand
/// back the notifies owed and the push the clip's move asks for.
///
/// The pass runs after the frame's threads, so script reads this frame's
/// result on the next one: retail's `moveto((600,0,100), 1)` called at level
/// time 1050 put `trTime` 1050 on the wire and yet `getorigin()` still read
/// the start origin at 1100 and the first moved value at 1150, and
/// `movedone` came at 2100 rather than 2050, raised on the 2050 pass and
/// run by the next frame's threads (docs/research/cod11-movers.md, section
/// 8; combat doc 14.7, "Entity numbers").
///
/// The write-back is what the capture shows retail doing -- `getorigin()` and
/// `.angles` both read the interpolated value on every frame of a move -- and
/// it is also what keeps this to one representation: `istouching`, the touch
/// pass, `getorigin` and the wire build all go on reading the entity's own
/// fields. `.angles` is `r.currentAngles`: raw while a rotate runs, through
/// `AngleNormalize360`/`180` once it ends, while the wire keeps the
/// trajectory (movers doc 8, and 12 "The residual yaw").
pub fn run_one(host: &mut GameHost, cx: &mut vcod_gsc::Cx, id: EntId) -> (Vec<Done>, Option<Step>) {
    let level_ms = host.level_time_ms;
    let mut done = Vec::new();
    let Some(mut m) = host.movers.rows.remove(&id) else {
        return (done, None);
    };
    if host.ents.get(id).is_none() {
        return (done, None);
    }
    // `G_RunMover` reaches `G_MoverTeam` only off a moving trajectory
    // (0x57666..0x57670).
    let moving = [&m.pos, &m.apos]
        .iter()
        .any(|p| p.started && p.current.tr_type != TR_STATIONARY);
    m.reach_links = [&m.pos, &m.apos]
        .iter()
        .filter(|p| p.notify.is_some() && level_ms >= p.current.tr_time + p.current.tr_duration)
        .count() as u8;
    let unadvanced = (m.pos.clone(), m.apos.clone());
    for plan in [&mut m.pos, &mut m.apos] {
        if let Some(e) = plan.advance(level_ms) {
            done.push(Done { ent: id, event: e });
        }
    }
    // Groups whose running segment ended this frame.
    let (pos_reached, apos_reached) = (
        m.pos.current != unadvanced.0.current,
        m.apos.current != unadvanced.1.current,
    );
    m.unreached = (m.pos != unadvanced.0 || m.apos != unadvanced.1).then(|| {
        let events = done.iter().map(|d| d.event).collect();
        (unadvanced.0, unadvanced.1, events)
    });
    let angles_atom = cx.intern_folded("angles");
    // `.angles` is `r.currentAngles` itself (script field 16, offset 0x140).
    let before = match host.get_field(cx, id, angles_atom) {
        vcod_gsc::Value::Vector(v) => Vec3::from(v),
        _ => Vec3::ZERO,
    };
    let origin_atom = cx.intern_folded("origin");
    let before_origin = match host.get_field(cx, id, origin_atom) {
        vcod_gsc::Value::Vector(v) => Vec3::from(v),
        _ => Vec3::ZERO,
    };
    // `G_MoverTeam`: `move` and `amove` are the trajectories less
    // `r.currentOrigin` and `r.currentAngles`, each stored as a float, which
    // `G_MoverPush` adds back (0x650f0) and the link snaps (movers doc 12,
    // "The residual yaw"; 23). A group whose segment ended this frame is
    // then evaluated into place by `Reached_ScriptMover` (0x5fa0e, 0x5fab1).
    let origin = bg_evaluate(m.pos.stored, &m.pos.current, level_ms);
    let origin = if moving && !pos_reached {
        before_origin + (origin - before_origin)
    } else {
        origin
    };
    let (turned, angles) = if moving {
        let target = if m.apos.started {
            bg_evaluate(m.apos.stored, &m.apos.current, level_ms)
        } else {
            before
        };
        let turned = if apos_reached {
            target
        } else {
            before + (target - before)
        };
        (turned, snap_angles(turned))
    } else {
        (before, before)
    };
    // `Reached_ScriptMover` quantizes after the links, so no link snaps it.
    let reached_at = (!m.sine && done.iter().any(|d| d.event == ROTATEDONE))
        .then(|| reached_angles(m.apos.current.base));
    let fields = [
        (m.pos.started, "origin", origin),
        (m.apos.started || moving, "angles", angles),
    ];
    for (started, name, v) in fields {
        if !started {
            continue;
        }
        let atom = cx.intern_folded(name);
        let _ = host.write_field(cx, id, atom, vcod_gsc::Value::Vector(v.into()));
    }
    let mut step = clip_step(host, cx, id, &mut m, level_ms, (before, turned));
    // `G_MoverPush` unlinks the pusher (0x55315), lists what the move
    // sweeps (0x5533c) and links it where it now is (0x553ae), so a moving
    // mover goes to the head of its node's list every frame.
    if moving {
        let boxes = step.as_ref().and_then(|s| push_boxes(host, s));
        host.area.unlink(id.0);
        if let (Some(s), Some((swept, moved))) = (step.as_mut(), boxes) {
            s.listed =
                host.area
                    .entities_in_box(swept.0.to_array(), swept.1.to_array(), PUSH_LIST_MASK);
            s.moved_box = Some(moved);
        }
        let (origin, angles) = pose(host, cx, id);
        host.link_entity_at(cx, id, Some((origin.into(), angles.into())));
    }
    if let Some(a) = reached_at {
        let _ = host.write_field(cx, id, angles_atom, vcod_gsc::Value::Vector(a.into()));
        // A trace turns a brush model by `r.currentAngles` as it finds them
        // (0x8090f18 hands 0x80568d8 the entity's +0x140), so the quantized
        // angles clip from here on with no link (movers doc 22).
        let model = host
            .ents
            .get(id)
            .and_then(|e| e.brush_model)
            .filter(|&n| n > 0);
        if let Some(model) = model {
            let (origin, _) = pose(host, cx, id);
            if let Some(world) = &host.world {
                world.collision.set_model_pose(model as usize, origin, a);
            }
        }
    }
    host.movers.rows.insert(id, m);
    // A push runs first and a blocked one never reaches this
    // (`crate::game::script::ScriptRuntime::push_mover`).
    if step.is_none() {
        reached(host, cx, id);
    }
    (done, step)
}

/// A verb's closing `trap_LinkEntity`, at the `origin` and `angles` it left:
/// a brush model's clip is put there, and its next move pushes from there.
pub fn verb_linked(host: &mut GameHost, cx: &mut vcod_gsc::Cx, id: EntId) {
    host.link_entity(cx, id);
    let pose = pose(host, cx, id);
    if let Some(m) = host.movers.rows.get_mut(&id)
        && m.clip.is_some()
    {
        m.clip = Some(pose);
    }
}

/// `G_MoverTeam`'s tail (0x558a0..0x558e9), after the move went through:
/// `pos`, then `apos` as the first call left it, each calls the entity's
/// `reached` when its `trType` is not 0 and the level time has reached its
/// `trTime + trDuration`. Only a sine mover's is `Reached_BinaryMover`
/// (0x55c9c), which ends a door leg and `G_Error`s on any other state, so
/// an unused sine mover drops the level at its first period's end
/// (docs/research/cod11-movers.md 18); the verbs' plans retire their own
/// segments. Then `G_RunThink`'s `ReturnToPos1` when it is due.
///
/// A verb's `Reached_ScriptMover` links again where `G_MoverPush` just
/// linked, once per group whose segment ended: the entity keeps its place
/// in the area tree, but each link runs the node's push-down.
pub fn reached(host: &mut GameHost, cx: &mut vcod_gsc::Cx, id: EntId) {
    let links = host.movers.rows.get(&id).map_or(0, |m| m.reach_links);
    for _ in 0..links {
        if let Some(link) = host.area.last_link(id.0) {
            host.area.link(id.0, &link);
        }
    }
    if !host.movers.rows.get(&id).is_some_and(|m| m.sine) {
        return;
    }
    let t = host.level_time_ms;
    for apos in [false, true] {
        let Some(m) = host.movers.rows.get(&id) else {
            return;
        };
        let p = if apos { &m.apos } else { &m.pos };
        if p.current.tr_type != TR_STATIONARY
            && t >= p.current.tr_time + p.current.tr_duration
            && host.game_error.is_none()
        {
            reached_binary(host, cx, id);
        }
    }
    // `G_RunThink` (`nextthink` of 0 or less never runs).
    if let Some(m) = host.movers.rows.get_mut(&id)
        && m.return_at.is_some_and(|at| at > 0 && at <= t)
    {
        m.return_at = None;
        return_to_pos1(host, cx, id);
    }
}

/// `Reached_BinaryMover` (0x55c9c) on a sine mover: a 1-to-2 leg settles
/// at `pos2` and arms `ReturnToPos1` `wait` on (never at a `wait` of
/// -1000, 0x75c84), a 2-to-1 leg settles at `pos1`, and any other state
/// drops the level. Its sounds are the mover's alias bytes, 0 on a sine
/// mover, which `G_PlaySoundAlias` (0x67b30) skips.
fn reached_binary(host: &mut GameHost, cx: &mut vcod_gsc::Cx, id: EntId) {
    let t = host.level_time_ms;
    match host.movers.rows.get(&id).map(|m| m.binary) {
        Some(Binary::OneToTwo) => {
            set_mover_state(host, cx, id, Binary::Pos2, t);
            if let Some(wait) = mover_wait(host, cx, id)
                && let Some(m) = host.movers.rows.get_mut(&id)
            {
                m.return_at = Some(t + wait);
            }
        }
        Some(Binary::TwoToOne) => set_mover_state(host, cx, id, Binary::Pos1, t),
        _ => {
            host.game_error = Some("Reached_BinaryMover: bad moverState".to_string());
        }
    }
}

/// The milliseconds `ReturnToPos1` waits at `pos2`: the `wait` field
/// (`ent+0x268`, a float the script may write) truncated, `None` for
/// -1000, which holds it there.
fn mover_wait(host: &mut GameHost, cx: &mut vcod_gsc::Cx, id: EntId) -> Option<i32> {
    let atom = cx.intern_folded("wait");
    let wait = match host.get_field(cx, id, atom) {
        vcod_gsc::Value::Float(f) => f,
        vcod_gsc::Value::Int(i) => i as f32,
        _ => 0.0,
    };
    (wait != -1000.0).then_some(wait as i32)
}

/// `ReturnToPos1` (0x57b28): the 2-to-1 leg from the level time.
fn return_to_pos1(host: &mut GameHost, cx: &mut vcod_gsc::Cx, id: EntId) {
    let t = host.level_time_ms;
    set_mover_state(host, cx, id, Binary::TwoToOne, t);
}

/// `SetMoverState` (0x55904) on a sine mover, whose `pos1` and `pos2` are
/// both the world origin and whose leg time (`ent+0x1f4`, `+0x1f8`) is
/// `InitMover`'s 1 ms: `pos` stationary at the origin for `POS1` and
/// `POS2`, a `TR_LINEAR_STOP` of no distance for a leg; `trTime` of both
/// groups set to `t`, which restarts a pendulum's swing; then
/// `r.currentOrigin` off `pos` at the level time and a link (0x55c5d).
/// `r.currentAngles` keeps what the last frame wrote.
fn set_mover_state(host: &mut GameHost, cx: &mut vcod_gsc::Cx, id: EntId, state: Binary, t: i32) {
    let level = host.level_time_ms;
    let Some(m) = host.movers.rows.get_mut(&id) else {
        return;
    };
    m.binary = state;
    m.pos.started = true;
    m.pos.current = match state {
        Binary::Pos1 | Binary::Pos2 => Trajectory {
            tr_type: TR_STATIONARY,
            tr_time: t,
            base: Vec3::ZERO,
            ..m.pos.current
        },
        Binary::OneToTwo | Binary::TwoToOne => Trajectory {
            tr_type: TR_LINEAR_STOP,
            tr_time: t,
            tr_duration: 1,
            base: Vec3::ZERO,
            delta: Vec3::ZERO,
        },
    };
    m.apos.current.tr_time = t;
    let origin = bg_evaluate(m.pos.stored, &m.pos.current, level);
    let atom = cx.intern_folded("origin");
    let _ = host.write_field(cx, id, atom, vcod_gsc::Value::Vector(origin.into()));
    verb_linked(host, cx, id);
}

/// `Use_BinaryMover` (0x560ec), every sine mover's `use` (`InitMover`,
/// 0x565d0), on `useBy`: at `pos1` the 1-to-2 leg starts 50 ms on; at
/// `pos2` `ReturnToPos1` is put `wait` on. A use during a leg runs
/// `Blocked_Door`'s reversal, which vcod does not model: a script sees a
/// leg only for 100 ms after a use. Returns false for anything not a sine
/// mover (cod11-combat.md 5.9, "Use_BinaryMover").
pub fn use_binary(host: &mut GameHost, cx: &mut vcod_gsc::Cx, id: EntId) -> bool {
    let Some(state) = host
        .movers
        .rows
        .get(&id)
        .filter(|m| m.sine)
        .map(|m| m.binary)
    else {
        return false;
    };
    let t = host.level_time_ms;
    match state {
        Binary::Pos1 => set_mover_state(host, cx, id, Binary::OneToTwo, t + 50),
        Binary::Pos2 => {
            let at = mover_wait(host, cx, id).map(|w| t + w);
            if let (Some(at), Some(m)) = (at, host.movers.rows.get_mut(&id)) {
                m.return_at = Some(at);
            }
        }
        Binary::OneToTwo | Binary::TwoToOne => {}
    }
    true
}

/// `SV_LinkEntity`'s snap (cod_lnxded `0x080908b0`): each component of
/// `r.currentAngles` within a thousandth of an integer becomes that
/// integer, `(n - a)^2 < 1e-6` with `n` the truncation of `a + 0.5`
/// (movers doc 12, "The residual yaw").
pub fn snap_angles(a: Vec3) -> Vec3 {
    let snap = |c: f32| {
        let n = (f64::from(c) + 0.5).trunc();
        let d = n - f64::from(c);
        if d * d < f64::from(1.000_000_1e-6_f32) {
            n as f32
        } else {
            c
        }
    };
    Vec3::new(snap(a.x), snap(a.y), snap(a.z))
}

/// `Reached_ScriptMover`'s angular half (0x5fa57..0x5fb13): the settled
/// trajectory's base, linked (snapped), then pitch and roll through
/// `AngleNormalize180` and yaw through `AngleNormalize360`, both truncated
/// to 65536ths. Nothing links the entity after, so `r.currentAngles`
/// keeps the quantized value while `apos.trBase` keeps the target.
fn reached_angles(base: Vec3) -> Vec3 {
    use vcod_common::pmove::aim::{angle_normalize_180, angle_normalize_360};
    let a = snap_angles(base);
    Vec3::new(
        angle_normalize_180(a.x),
        angle_normalize_360(a.y),
        angle_normalize_180(a.z),
    )
}

/// `SP_func_bobbing` (0x583d0) and `SP_func_pendulum` (0x56fe0), from a map
/// block's keys or, for a script `spawn`, none (`G_SpawnFloat` reads the
/// spawn vars the last map block left, which `G_ParseSpawnVars` clears).
/// Both run `InitMover` (0x56454), which moves the entity to `pos1`: no
/// `SP_` function but `SP_func_door`'s writes it, so both sit at the world
/// origin whatever their `origin` key, and both link with the model's
/// bounds and `trap_SetBrushModel`'s contents -1. A script-spawned one has
/// no `*N` and takes model 0's, the world's. Returns false for any other
/// classname (docs/research/cod11-movers.md 18).
pub fn spawn_sine(
    host: &mut GameHost,
    cx: &mut vcod_gsc::Cx,
    id: EntId,
    classname: &str,
    keys: Option<&[(String, String)]>,
) -> bool {
    use crate::game::builtins::cvar::{atof, atoi};
    let bobbing = match classname {
        "func_bobbing" => true,
        "func_pendulum" => false,
        "func_static" => return spawn_static(host, cx, id, keys),
        _ => return false,
    };
    // `G_SpawnString` matches a key with `strcmp`, the first copy (0x613c9).
    let key = |name: &str| keys.and_then(|b| vcod_common::bsp::spawn_string(b, name));
    let float = |name: &str, default: f32| key(name).map_or(default, atof);
    let model = host.ents.get(id).and_then(|e| e.brush_model).unwrap_or(0) as usize;
    let (mins, maxs) = host
        .model_bounds
        .get(model)
        .copied()
        .unwrap_or(([0.0; 3], [0.0; 3]));
    let vector = |host: &mut GameHost, cx: &mut vcod_gsc::Cx, name: &str| {
        let atom = cx.intern_folded(name);
        match host.get_field(cx, id, atom) {
            vcod_gsc::Value::Vector(v) => Vec3::from(v),
            _ => Vec3::ZERO,
        }
    };
    let set = |host: &mut GameHost, cx: &mut vcod_gsc::Cx, name: &str, v: vcod_gsc::Value| {
        let atom = cx.intern_folded(name);
        let _ = host.write_field(cx, id, atom, v);
    };
    let speed_key = float("speed", if bobbing { 4.0 } else { 30.0 });
    if bobbing {
        set(host, cx, "speed", vcod_gsc::Value::Float(speed_key));
    }
    let height = float("height", 32.0);
    set(
        host,
        cx,
        "dmg",
        vcod_gsc::Value::Int(key("dmg").map_or(2, atoi)),
    );
    let phase = float("phase", 0.0);
    // The pendulum's period, off `r.mins[2]` (0x5703c..0x570a3).
    let pendulum_period = {
        let length = mins[2].abs().max(8.0);
        let gravity = atof(host.cvars.get("g_gravity"));
        let root = (f64::from(gravity) / (f64::from(length) * 3.0)).sqrt() as f32;
        let freq = (f64::from(root) * 0.159_154_943_091_895_35) as f32;
        (1000.0 / f64::from(freq)) as i32
    };
    // `InitMover`: `r.currentOrigin = pos1`, a zero speed made 100, the link.
    set(host, cx, "origin", vcod_gsc::Value::Vector([0.0; 3]));
    let speed_atom = cx.intern_folded("speed");
    let speed = match host.get_field(cx, id, speed_atom) {
        vcod_gsc::Value::Float(f) => f,
        vcod_gsc::Value::Int(i) => i as f32,
        _ => 0.0,
    };
    let speed = if speed == 0.0 {
        set(host, cx, "speed", vcod_gsc::Value::Float(100.0));
        100.0
    } else {
        speed
    };
    let shape = crate::game::entity::LinkShape {
        kind: crate::game::entity::LinkKind::Brush,
        contents: -1,
        mins,
        maxs,
    };
    host.link_shaped(cx, id, shape);
    let angles = vector(host, cx, "angles");
    let (pos, apos) = if bobbing {
        let period = (speed * 1000.0) as i32;
        let spawnflags = cx.intern_folded("spawnflags");
        let flags = match host.get_field(cx, id, spawnflags) {
            vcod_gsc::Value::Int(i) => i,
            _ => 0,
        };
        let axis = if flags & 1 != 0 {
            Vec3::X
        } else if flags & 2 != 0 {
            Vec3::Y
        } else {
            Vec3::Z
        };
        let pos = Trajectory {
            tr_type: TR_SINE,
            tr_time: (f64::from(period) * f64::from(phase)) as i32,
            tr_duration: period,
            base: Vec3::ZERO,
            delta: axis * height,
        };
        (pos, stationary(angles))
    } else {
        let period = pendulum_period;
        // `InitMover`'s own `trDuration`, its distance over its speed and
        // at least 1, which only the bobbing writes over.
        let pos = Trajectory {
            tr_duration: 1,
            ..stationary(Vec3::ZERO)
        };
        let apos = Trajectory {
            tr_type: TR_SINE,
            tr_time: (f64::from(period) * f64::from(phase)) as i32,
            tr_duration: period,
            base: angles,
            delta: Vec3::new(0.0, 0.0, speed_key),
        };
        (pos, apos)
    };
    let stored = host.movers.stored;
    let plan = |current: Trajectory| Plan {
        current,
        stored,
        started: current.tr_type == TR_SINE,
        ..Plan::default()
    };
    host.movers.rows.insert(
        id,
        Mover {
            pos: plan(pos),
            apos: plan(apos),
            clip: None,
            sine: true,
            init_mover: true,
            use_static: false,
            reach_links: 0,
            binary: Binary::Pos1,
            return_at: None,
            unreached: None,
        },
    );
    true
}

/// `SP_func_static` (0x5820c): `trap_SetBrushModel`, `InitMover` (the
/// world origin, `eType` 5, contents -1, a link), `s.pos.trBase` from
/// `r.currentOrigin`, and an unlink under spawnflags 1. No stock map places
/// one; a script `spawn` makes a world copy at the world origin, which
/// clips exactly where the world does (movers doc 20). Spawnflags 2 or 4,
/// or a `health` key, give it `takedamage` and `Use_Static`
/// ([`use_static`]); `Static_Pain` never runs, since `G_Damage` takes the
/// `use` arm for an `eType` 5 entity first (movers doc 21).
fn spawn_static(
    host: &mut GameHost,
    cx: &mut vcod_gsc::Cx,
    id: EntId,
    keys: Option<&[(String, String)]>,
) -> bool {
    let model = host.ents.get(id).and_then(|e| e.brush_model).unwrap_or(0) as usize;
    let (mins, maxs) = host
        .model_bounds
        .get(model)
        .copied()
        .unwrap_or(([0.0; 3], [0.0; 3]));
    let set = |host: &mut GameHost, cx: &mut vcod_gsc::Cx, name: &str, v: vcod_gsc::Value| {
        let atom = cx.intern_folded(name);
        let _ = host.write_field(cx, id, atom, v);
    };
    set(host, cx, "origin", vcod_gsc::Value::Vector([0.0; 3]));
    let shape = crate::game::entity::LinkShape {
        kind: crate::game::entity::LinkKind::Brush,
        contents: -1,
        mins,
        maxs,
    };
    host.link_shaped(cx, id, shape);
    let spawnflags = match keys {
        Some(b) => vcod_common::bsp::field_value(b, "spawnflags")
            .and_then(|v| v.trim().parse::<i32>().ok())
            .unwrap_or(0),
        None => {
            let atom = cx.intern_folded("spawnflags");
            match host.get_field(cx, id, atom) {
                vcod_gsc::Value::Int(i) => i,
                _ => 0,
            }
        }
    };
    if spawnflags & 1 != 0 {
        host.area.unlink(id.0);
        host.unlink_world_copy(id);
    }
    // The `health` key, then the pain arm: either sets `takedamage`, and the
    // pain arm writes `health` 9999 (0x5828a..0x582d7). A script spawn reads
    // no key.
    let health_key = keys
        .and_then(|b| vcod_common::bsp::spawn_string(b, "health"))
        .and_then(|v| v.trim().parse::<i32>().ok())
        .unwrap_or(0);
    let use_static = health_key != 0 || spawnflags & 6 != 0;
    if spawnflags & 6 != 0 {
        set(host, cx, "health", vcod_gsc::Value::Int(STATIC_HEALTH));
    }
    let angles = {
        let atom = cx.intern_folded("angles");
        match host.get_field(cx, id, atom) {
            vcod_gsc::Value::Vector(v) => Vec3::from(v),
            _ => Vec3::ZERO,
        }
    };
    // `InitMover`'s `trDuration`: no distance over its speed, at least 1.
    let pos = Trajectory {
        tr_duration: 1,
        ..stationary(Vec3::ZERO)
    };
    host.movers.rows.insert(
        id,
        Mover {
            pos: Plan {
                current: pos,
                stored: host.movers.stored,
                ..Plan::default()
            },
            apos: Plan {
                current: stationary(angles),
                stored: host.movers.stored,
                ..Plan::default()
            },
            clip: None,
            sine: false,
            init_mover: true,
            use_static,
            reach_links: 0,
            binary: Binary::Pos1,
            return_at: None,
            unreached: None,
        },
    );
    true
}

/// Whether `id` is a [`spawn_sine`] mover, which the wire sends as `eType`
/// 5 (`InitMover`, 0x56619).
pub fn is_sine(host: &GameHost, id: EntId) -> bool {
    host.movers.rows.get(&id).is_some_and(|m| m.sine)
}

/// The `health` `SP_func_static`'s pain arm writes (0x582d7).
pub const STATIC_HEALTH: i32 = 9999;

/// Every linked `func_static` a blast can reach, by entity number:
/// `takedamage`, `Use_Static` as its `use`, and still in the area tree.
pub fn damageable_statics(host: &GameHost) -> Vec<EntId> {
    let mut ids: Vec<EntId> = host
        .movers
        .rows
        .iter()
        .filter(|(id, m)| m.use_static && host.area.last_link(id.0).is_some())
        .map(|(id, _)| *id)
        .collect();
    ids.sort_by_key(|id| id.0);
    ids
}

/// `G_Damage`'s arm for an `eType` 5 entity (game.mp `0x49e93`): with a
/// `use` and `moverState` 0 it notifies `"trigger"` with the attacker and
/// calls the `use`, and touches no `health`. A `func_static`'s is
/// `Use_Static`, which unlinks a linked entity and links an unlinked one
/// (movers doc 21); a blast only reaches a linked one. Returns whether `id`
/// took that arm.
pub fn use_static(host: &mut GameHost, cx: &mut vcod_gsc::Cx, id: EntId) -> bool {
    host.movers.rows.get(&id).is_some_and(|m| m.use_static) && toggle_static(host, cx, id)
}

/// `Use_Static` (1.1d 0x58070, 1.5b 0x697a3, UO 0x742e7), which
/// `SP_func_static` installs as every `func_static`'s `use`, takedamage or
/// not, so a script `useBy` reaches it too (cod11-combat.md 5.9, "useBy").
/// Returns false for anything not a `func_static`.
pub fn toggle_static(host: &mut GameHost, cx: &mut vcod_gsc::Cx, id: EntId) -> bool {
    if !host
        .movers
        .rows
        .get(&id)
        .is_some_and(|m| m.init_mover && !m.sine)
    {
        return false;
    }
    if host.area.last_link(id.0).is_some() {
        host.area.unlink(id.0);
        host.unlink_world_copy(id);
    } else {
        host.link_entity(cx, id);
    }
    true
}

/// Whether `id` went through `InitMover`, which the wire sends as `eType`
/// 5 (0x56619): a [`spawn_sine`] mover or a `func_static`.
pub fn is_init_mover(host: &GameHost, id: EntId) -> bool {
    host.movers.rows.get(&id).is_some_and(|m| m.init_mover)
}

/// [`run_one`] for every mover by entity number, the pushes dropped: the
/// tests' stand-in for the entity pass.
pub fn run(host: &mut GameHost, cx: &mut vcod_gsc::Cx) -> Vec<Done> {
    let mut ids: Vec<EntId> = host.movers.rows.keys().copied().collect();
    ids.sort_by_key(|i| i.0);
    ids.into_iter()
        .flat_map(|id| run_one(host, cx, id).0)
        .collect()
}

/// Whether `id` has a mover row, which is what puts it on `G_RunMover`'s arm.
pub fn is_mover(host: &GameHost, id: EntId) -> bool {
    host.movers.rows.contains_key(&id)
}

/// The push `step` asked for was blocked: `G_MoverTeam` puts the mover back
/// where it was, its fields and its clip, and every trajectory runs a frame
/// later (movers doc, section 12).
pub fn stall(host: &mut GameHost, cx: &mut vcod_gsc::Cx, step: &Step) {
    let Some(m) = host.movers.rows.get_mut(&step.ent) else {
        return;
    };
    if let Some((pos, apos, events)) = m.unreached.take() {
        m.pos = pos;
        m.apos = apos;
        for e in events {
            if let Some(i) = host
                .engine_notifies
                .iter()
                .rposition(|(ent, ev, _)| *ent == step.ent && *ev == e)
            {
                host.engine_notifies.remove(i);
            }
        }
    }
    let Some(m) = host.movers.rows.get_mut(&step.ent) else {
        return;
    };
    m.pos.stall();
    m.apos.stall();
    // `r.currentAngles` re-evaluated off the shifted trajectory and linked
    // (0x55838..0x5585d), so a quantized yaw goes back to the target.
    let a = snap_angles(if m.apos.started {
        bg_evaluate(m.apos.stored, &m.apos.current, host.level_time_ms)
    } else {
        step.from.1
    });
    let o = step.from.0;
    m.clip = Some((o, a));
    let (pos, apos) = (m.pos.started, m.apos.started);
    // A world copy goes back with the link below.
    if let (Some(world), Pusher::Model(model)) = (&host.world, step.pusher) {
        world.collision.set_model_pose(model, o, a);
    }
    for (started, name, v) in [(pos, "origin", o), (apos || a != step.from.1, "angles", a)] {
        if started {
            let atom = cx.intern_folded(name);
            let _ = host.write_field(cx, step.ent, atom, vcod_gsc::Value::Vector(v.into()));
        }
    }
    let (origin, angles) = pose(host, cx, step.ent);
    host.link_entity_at(cx, step.ent, Some((origin.into(), angles.into())));
}

/// `G_MoverPush`'s `trap_EntitiesInBox` mask (0x5531d): a live player's
/// `0x2000000`, an item's `0x100` and `0x80`.
const PUSH_LIST_MASK: i32 = 0x2000180;

/// `G_MoverPush`'s two boxes (0x550f0..0x5530a), `(swept, moved)`: the
/// pusher's last link box, or a cube of `RadiusFromBounds` about its origin
/// when its angles or `amove` are not all zero; `moved` is that box plus
/// `move`, `swept` the box stretched along `move` on each axis.
fn push_boxes(host: &GameHost, step: &Step) -> Option<((Vec3, Vec3), (Vec3, Vec3))> {
    let amove = step.to.1 - step.from.1;
    let here = if step.from.1 == Vec3::ZERO && amove == Vec3::ZERO {
        let link = host.area.last_link(step.number)?;
        (Vec3::from(link.absmin), Vec3::from(link.absmax))
    } else {
        let shape = host.ents.get(step.ent)?.link?;
        let (lo, hi) = (Vec3::from(shape.mins), Vec3::from(shape.maxs));
        let r = lo.abs().max(hi.abs()).length();
        (step.from.0 - Vec3::splat(r), step.from.0 + Vec3::splat(r))
    };
    Some(boxes_from(here, step.to.0 - step.from.0))
}

/// [`push_boxes`] off the pusher's box before the move.
fn boxes_from((mins, maxs): (Vec3, Vec3), mv: Vec3) -> ((Vec3, Vec3), (Vec3, Vec3)) {
    let (mut lo, mut hi) = (mins, maxs);
    // The sign test at 0x552e8: a positive move grows the max side.
    for i in 0..3 {
        if mv[i] > 0.0 {
            hi[i] += mv[i];
        } else {
            lo[i] += mv[i];
        }
    }
    ((lo, hi), (mins + mv, maxs + mv))
}

/// The entity's `origin` and `angles` fields.
fn pose(host: &mut GameHost, cx: &mut vcod_gsc::Cx, id: EntId) -> (Vec3, Vec3) {
    let mut field = |name: &str| {
        let atom = cx.intern_folded(name);
        match host.get_field(cx, id, atom) {
            vcod_gsc::Value::Vector(v) => Vec3::from(v),
            _ => Vec3::ZERO,
        }
    };
    (field("origin"), field("angles"))
}

/// A brush model mover's clip, moved to where its plans have it at the
/// level time and handed on as a [`Step`] when it moved. That is
/// `G_MoverTeam`'s clock: the entity pass runs after the script frame, so
/// the clip is a frame ahead of the `getorigin()` above (movers doc,
/// sections 8 and 11).
fn clip_step(
    host: &mut GameHost,
    cx: &mut vcod_gsc::Cx,
    id: EntId,
    m: &mut Mover,
    level_ms: i32,
    (before, turned): (Vec3, Vec3),
) -> Option<Step> {
    let pusher = match submodel(host, id) {
        Some(model) => Pusher::Model(model),
        None if host.is_world_copy(id) => Pusher::Copy(id.0),
        None => return None,
    };
    let world = host.world.clone()?;
    let field = |host: &mut GameHost, cx: &mut vcod_gsc::Cx, name: &str| {
        let atom = cx.intern_folded(name);
        match host.get_field(cx, id, atom) {
            vcod_gsc::Value::Vector(v) => Vec3::from(v),
            _ => Vec3::ZERO,
        }
    };
    let origin = field(host, cx, "origin");
    // `r.currentAngles` as linked this frame; the push turns by
    // `turned - before`, `G_MoverTeam`'s `amove`.
    let angles = field(host, cx, "angles");
    let at = |m: &Mover, t: i32| if m.pos.started { m.pos.at(t) } else { origin };
    let to = (at(m, level_ms), turned);
    let from = (
        m.clip
            .map_or_else(|| at(m, level_ms - crate::server::FRAME_MS), |c| c.0),
        before,
    );
    m.clip = Some((to.0, angles));
    if from == to {
        return None;
    }
    // A world copy is placed by the link `run_one` makes after the listing.
    match pusher {
        Pusher::Model(model) => {
            world.collision.set_model_pose(model, to.0, angles);
            if !world.collision.model_linked(model) {
                return None;
            }
        }
        Pusher::Copy(_) => {
            let solid = host.ents.get(id)?.link.is_some_and(|l| l.contents != 0);
            if !solid {
                return None;
            }
        }
    }
    Some(Step {
        ent: id,
        number: id.0,
        pusher,
        from,
        to,
        listed: Vec::new(),
        moved_box: None,
    })
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

    /// `movegravity` notifies on the frame its time runs out and stops there,
    /// the arc evaluated at that frame (`probe_moverreads`, movers doc 5).
    #[test]
    fn movegravity_stops_where_its_time_runs_out() {
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

        // It stops where it is on the frame its time runs out (0x5f93b).
        let row = movers.rows.get_mut(&id).unwrap();
        assert_eq!(row.pos.advance(3000), Some(MOVEDONE));
        assert_eq!(row.pos.current.tr_type, TR_STATIONARY);
        assert!((row.pos.current.evaluate(3500).z + 2600.0).abs() < 0.01);
    }

    /// `G_MoverPush`'s boxes for an unrotated pusher: the list box grows
    /// only on the side `move` points to, the test box is shifted whole
    /// (0x55260..0x5530a).
    #[test]
    fn push_boxes_stretch_and_shift_along_the_move() {
        let here = (Vec3::new(-1.0, -1.0, -1.0), Vec3::new(11.0, 11.0, 11.0));
        let ((lo, hi), (mlo, mhi)) = boxes_from(here, Vec3::new(5.0, -3.0, 0.0));
        assert_eq!(
            (lo, hi),
            (Vec3::new(-1.0, -4.0, -1.0), Vec3::new(16.0, 11.0, 11.0))
        );
        assert_eq!(
            (mlo, mhi),
            (Vec3::new(4.0, -4.0, -1.0), Vec3::new(16.0, 8.0, 11.0))
        );
    }
}
