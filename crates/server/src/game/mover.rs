//! The scriptent mover verbs' state and integrator.
//!
//! Everything here is measured, not guessed: `docs/research/cod11-movers.md`
//! is one paired capture against the retail 1.1d server, the per-frame
//! `getorigin()` on one side and the `pos`/`apos` trajectory groups on the
//! other. The three facts that shape this module are its sections 4 and 9.
//!
//! A verb builds a **plan**: the first [`Trajectory`] segment of the
//! trapezoidal velocity profile and the ends its setup stores for the rest,
//! plus the notify to raise when the last one settles. Retail sends exactly
//! those segments and nothing between them, so the plan is both the
//! simulation and the wire format; there is no second representation to
//! keep in sync.
//!
//! Each frame the integrator steps a segment whose duration has elapsed into
//! the next, which starts on that frame from the end the setup stored
//! (movers doc 24), and writes the evaluated origin and angles back into the
//! entity's own fields, which is what the capture shows retail doing:
//! `getorigin()` and `.angles` both read the interpolated value on every
//! frame of a move.
//!
//! A brush model's clip follows its trajectory one frame ahead of what
//! script reads, on retail's `G_MoverTeam` clock, and each move is handed to
//! the server as a [`Step`] whose riders and blocked players it pushes
//! (docs/research/cod11-movers.md, sections 11 and 12).

use crate::game::host::GameHost;
use glam::Vec3;
use vcod_common::collision::Pusher;
use vcod_common::net::trajectory::{
    TR_ACCELERATE, TR_DECCELERATE, TR_GRAVITY, TR_LINEAR_STOP, TR_SINE, TR_STATIONARY, Trajectory,
};
use vcod_common::x87;
use vcod_gsc::EntId;
use vcod_gsc::Host;

pub mod door;

/// `"movedone"`, what the four linear families and `movegravity` raise
/// (movers doc, section 7).
pub const MOVEDONE: &str = "movedone";
/// `"rotatedone"`, the angular half's.
pub const ROTATEDONE: &str = "rotatedone";

/// One group's motion: the trajectory on the wire now, the segment state
/// `Reached_ScriptMover` chains the next segment from, and whether finishing
/// owes a notify.
#[derive(Clone, Debug, Default, PartialEq)]
struct Plan {
    current: Trajectory,
    /// What the verb's setup left for the segments after the first.
    segs: Segs,
    /// `Some` while a verb's segments are still running.
    notify: Option<&'static str>,
    /// Whether a verb has ever run on this group. A rotate leaves `pos`
    /// untouched, and writing an unstarted group's zero trajectory back onto
    /// the entity would teleport it to the origin.
    started: bool,
    /// 1.5b's and UO's `BG_EvaluateTrajectory` ([`bg_evaluate`]).
    stored: bool,
}

/// The seven fields a verb's setup writes per group (game.mp `pos`: speed
/// `ent+0x1e0`, cruise seconds `+0x268`, decel seconds `+0x274`, the ends
/// `+0x1a4`, `+0x1b0`, `+0x1bc`; `apos`: `+0x1e4`, `+0x1d0`, `+0x26c`,
/// `+0x1e8`, `+0x280`, `+0x28c`, movers doc 24). A verb with no ramps
/// writes only the cruise, decel and destination.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Segs {
    speed: f32,
    cruise: f32,
    decel: f32,
    accel_end: Vec3,
    cruise_end: Vec3,
    dest: Vec3,
}

impl Plan {
    /// Where the running segment has the group at `t`: `BG_EvaluateTrajectory`
    /// on it alone, clamped at its end, as a verb on a moving mover and
    /// `G_MoverTeam` read it (0x5fb75, 0x5feca).
    fn at(&self, t: i32) -> Vec3 {
        bg_evaluate(self.stored, &self.current, t)
    }

    /// `G_MoverTeam`'s blocked arm: the running segment starts a frame later,
    /// so the group holds where it was for this frame. Retail shifts both
    /// groups whether or not they are moving: the stationary `apos` of the
    /// crush capture advances its `trTime` 50 a frame too (movers doc,
    /// section 12).
    fn stall(&mut self) {
        self.current.tr_time = self.current.tr_time.wrapping_add(crate::server::FRAME_MS);
    }

    /// `G_MoverTeam`'s tail for one group at level time `now_ms`: when the
    /// running segment's time is up, `Reached_ScriptMover` steps to the
    /// next one, at most one a frame (0x5f9a2..0x5f9f8). Returns the notify
    /// owed, once, on the frame the last one settles.
    fn advance(&mut self, now_ms: i32) -> Option<&'static str> {
        if self.notify.is_none()
            || self.current.tr_type == TR_STATIONARY
            || self.current.tr_time.wrapping_add(self.current.tr_duration) > now_ms
        {
            return None;
        }
        if self.step(now_ms) {
            self.notify.take()
        } else {
            None
        }
    }

    /// The segment step `Reached_ScriptMover` calls (0x5f7b0): an ended
    /// accelerating segment goes on into the cruise when it lasts, an ended
    /// cruise (or an accelerating segment with none) into the decelerating
    /// one when there is one, each starting at `now_ms`, the frame that saw
    /// the end, not at the end itself. Anything else settles stationary on
    /// the destination, a `movegravity` where its arc is at `now_ms`
    /// (0x5f93b). Returns whether the group settled.
    fn step(&mut self, now_ms: i32) -> bool {
        let s = self.segs;
        let tr = &mut self.current;
        if tr.tr_type == TR_ACCELERATE && s.cruise > 0.0 {
            let dur = trunc_ms(s.cruise);
            // `(cruiseEnd - accelEnd) * (1000 / dur)` on the stack; 1.5b
            // stores the difference and the factor (0x630ea, 0x63121).
            let k = per_ms(self.stored, dur);
            let stored = self.stored;
            let d = |a: f32, b: f32| scaled(stored, f64::from(a) - f64::from(b), k);
            *tr = Trajectory {
                tr_type: TR_LINEAR_STOP,
                tr_time: now_ms,
                tr_duration: dur,
                base: s.accel_end,
                delta: Vec3::new(
                    d(s.cruise_end.x, s.accel_end.x),
                    d(s.cruise_end.y, s.accel_end.y),
                    d(s.cruise_end.z, s.accel_end.z),
                ),
            };
            return false;
        }
        if matches!(tr.tr_type, TR_ACCELERATE | TR_LINEAR_STOP) && s.decel > 0.0 {
            let dir = normalize_in_place(self.stored, s.dest - s.cruise_end);
            let v = |c: f32| (f64::from(s.speed) * f64::from(c)) as f32;
            *tr = Trajectory {
                tr_type: TR_DECCELERATE,
                tr_time: now_ms,
                tr_duration: trunc_ms(s.decel),
                base: s.cruise_end,
                delta: Vec3::new(v(dir.x), v(dir.y), v(dir.z)),
            };
            return false;
        }
        let base = if tr.tr_type == TR_GRAVITY {
            bg_evaluate(self.stored, tr, now_ms)
        } else {
            s.dest
        };
        *tr = Trajectory {
            tr_type: TR_STATIONARY,
            tr_time: now_ms,
            base,
            ..*tr
        };
        true
    }

    /// The move and rotate verbs' setup (0x5fb38) toward `dest` from `cur`,
    /// `r.currentOrigin` or `r.currentAngles`: the displacement is taken off
    /// `cur` as script left it, then a running trajectory is evaluated at
    /// `now_ms` into `cur`, where the first segment starts. Returns the
    /// start, which the verb writes back and links.
    fn start_toward(&mut self, now_ms: i32, cur: Vec3, dest: Vec3, m: Ramp) -> Vec3 {
        let d = dest - cur;
        let cur = self.running_start(now_ms, cur);
        let stored = self.stored;
        if m.accel == 0.0 && m.decel == 0.0 {
            let dur = trunc_ms(m.secs);
            self.segs.cruise = m.secs;
            self.segs.decel = 0.0;
            self.segs.dest = dest;
            let k = per_ms(stored, dur);
            let v = |c: f32| scaled(stored, f64::from(c), k);
            self.current = Trajectory {
                tr_type: TR_LINEAR_STOP,
                tr_time: now_ms,
                tr_duration: dur,
                base: cur,
                delta: Vec3::new(v(d.x), v(d.y), v(d.z)),
            };
            return self.begin(now_ms);
        }
        let (secs, accel, decel) = (f64::from(m.secs), f64::from(m.accel), f64::from(m.decel));
        let cruise = (secs - accel - decel) as f32;
        let dist = x87::dot(d, d).sqrt() as f32;
        let speed = ((f64::from(dist) * 2.0) / (secs * 2.0 - accel - decel)) as f32;
        let dir = normalize2(self.stored, d);
        // The velocity stays on the stack for the cruise's delta; its
        // stored copy feeds the accelerating delta and the cruise's end.
        // 1.5b stores it at once (0x63753).
        let vf = (dir.as_dvec3() * f64::from(speed)).as_vec3();
        let v = if stored {
            vf.as_dvec3()
        } else {
            dir.as_dvec3() * f64::from(speed)
        };
        self.segs.cruise = cruise;
        self.segs.decel = m.decel;
        self.segs.speed = speed;
        self.segs.dest = dest;
        if m.accel != 0.0 {
            self.current = Trajectory {
                tr_type: TR_ACCELERATE,
                tr_time: now_ms,
                tr_duration: trunc_ms(m.accel),
                base: cur,
                delta: vf,
            };
            self.segs.accel_end = self.at(now_ms.wrapping_add(self.current.tr_duration));
        } else {
            self.segs.accel_end = cur;
            self.current = if cruise != 0.0 {
                let dur = trunc_ms(cruise);
                let k = per_ms(stored, dur);
                let c = f64::from(cruise);
                let x = |v: f64| {
                    let d = v * c;
                    scaled(stored, if stored { f64::from(d as f32) } else { d }, k)
                };
                Trajectory {
                    tr_type: TR_LINEAR_STOP,
                    tr_time: now_ms,
                    tr_duration: dur,
                    base: cur,
                    delta: Vec3::new(x(v.x), x(v.y), x(v.z)),
                }
            } else {
                Trajectory {
                    tr_type: TR_DECCELERATE,
                    tr_time: now_ms,
                    tr_duration: trunc_ms(m.decel),
                    base: cur,
                    delta: vf,
                }
            };
        }
        self.segs.cruise_end = mul_add(vf, cruise, self.segs.accel_end);
        self.begin(now_ms)
    }

    /// `rotateVelocity`'s setup (0x5feb0): the delta is the velocity on
    /// every segment, the cruise lasts what the ramps leave, and the
    /// destination is where the segments run out, so ramps shorten the turn
    /// to `v * (T - ta/2 - td/2)`.
    fn start_velocity(&mut self, now_ms: i32, cur: Vec3, v: Vec3, m: Ramp) -> Vec3 {
        let cur = self.running_start(now_ms, cur);
        let segment = |tr_type: i32, secs: f32, base: Vec3| Trajectory {
            tr_type,
            tr_time: now_ms,
            tr_duration: trunc_ms(secs),
            base,
            delta: v,
        };
        if m.accel == 0.0 && m.decel == 0.0 {
            self.segs.cruise = m.secs;
            self.segs.decel = 0.0;
            self.current = segment(TR_LINEAR_STOP, m.secs, cur);
            self.segs.dest = self.at(now_ms.wrapping_add(self.current.tr_duration));
            return self.begin(now_ms);
        }
        let (secs, accel, decel) = (f64::from(m.secs), f64::from(m.accel), f64::from(m.decel));
        let cruise = (secs - accel - decel) as f32;
        self.segs.cruise = cruise;
        self.segs.decel = m.decel;
        self.segs.speed = x87::dot(v, v).sqrt() as f32;
        if m.accel != 0.0 {
            self.current = segment(TR_ACCELERATE, m.accel, cur);
            self.segs.accel_end = self.at(now_ms.wrapping_add(self.current.tr_duration));
        } else {
            self.segs.accel_end = cur;
            self.current = if cruise != 0.0 {
                segment(TR_LINEAR_STOP, cruise, cur)
            } else {
                segment(TR_DECCELERATE, m.decel, cur)
            };
        }
        let cruise_end = mul_add(v, cruise, self.segs.accel_end);
        self.segs.cruise_end = cruise_end;
        self.segs.dest = if m.decel != 0.0 {
            let tail = segment(TR_DECCELERATE, m.decel, cruise_end);
            bg_evaluate(self.stored, &tail, now_ms.wrapping_add(tail.tr_duration))
        } else {
            cruise_end
        };
        self.begin(now_ms)
    }

    /// `cur` as a verb's setup starts from it: a group whose `trType` is not
    /// 0 is evaluated at `now_ms` first (0x5fb70, 0x5fec5).
    fn running_start(&self, now_ms: i32, cur: Vec3) -> Vec3 {
        if self.current.tr_type != TR_STATIONARY {
            self.at(now_ms)
        } else {
            cur
        }
    }

    /// The setup's last evaluation, of the first segment at `now_ms`
    /// (0x5fea0, 0x60204): where the verb leaves `cur`.
    fn begin(&mut self, now_ms: i32) -> Vec3 {
        self.started = true;
        self.at(now_ms)
    }
}

/// Seconds as a `trDuration`: times `1000.0f` on the stack and truncated
/// (the setups' `fistp` under a chop control word, 0x5fbe6).
fn trunc_ms(secs: f32) -> i32 {
    (f64::from(secs) * 1000.0) as i32
}

/// `1000 / dur`, the factor a linear segment's delta takes: on the stack,
/// or stored on 1.5b and UO.
fn per_ms(stored: bool, dur: i32) -> f64 {
    let k = 1000.0 / f64::from(dur);
    if stored { f64::from(k as f32) } else { k }
}

/// `d * k`, rounded once to a float. 1.5b loads `d` from a float first.
fn scaled(stored: bool, d: f64, k: f64) -> f32 {
    let d = if stored { f64::from(d as f32) } else { d };
    (d * k) as f32
}

/// `v * s + a` per component, one rounding (0x5fe5a, 0x6012f).
fn mul_add(v: Vec3, s: f32, a: Vec3) -> Vec3 {
    let f = |v: f32, a: f32| (f64::from(v) * f64::from(s) + f64::from(a)) as f32;
    Vec3::new(f(v.x, a.x), f(v.y, a.y), f(v.z, a.z))
}

/// The module's `VectorNormalize2`, 1.1d's or 1.5b's and UO's.
fn normalize2(stored: bool, v: Vec3) -> Vec3 {
    if stored {
        x87::normalize2_stored(v)
    } else {
        x87::normalize(v)
    }
}

/// The module's `VectorNormalize`, in place.
fn normalize_in_place(stored: bool, v: Vec3) -> Vec3 {
    if stored {
        x87::normalize_stored(v)
    } else {
        x87::normalize_in_place(v)
    }
}

/// `BG_EvaluateTrajectory` as the build runs it: 1.1d's on the x87 stack
/// (`Trajectory::evaluate_kept`), 1.5b's and UO's stored
/// (`Trajectory::evaluate_stored`). A linear turn lands a float step off an
/// `f32` evaluation (movers doc 12, "The residual yaw").
fn bg_evaluate(stored: bool, tr: &Trajectory, t: i32) -> Vec3 {
    if stored {
        tr.evaluate_stored(t)
    } else {
        tr.evaluate_kept(t)
    }
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
    /// moves off `InitMover`'s 0 (cod11-combat.md 5.9, "useBy on a gun, a sine mover").
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
    /// A `func_door`, `func_door_rotating` or `func_rotating` ([`door`]).
    door: Option<door::Door>,
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

    /// A script `.origin` write (`Scr_SetOrigin`'s `G_SetOrigin`) on a
    /// group that has settled: the plan stands at `at`, so the next entity
    /// pass and the wire keep it there, and the clip's pose moves with the
    /// write's link. A group still moving is left to its plan: what retail
    /// does to a running move is unmeasured.
    pub fn set_origin(&mut self, id: EntId, at: Vec3) {
        let Some(m) = self.rows.get_mut(&id) else {
            return;
        };
        if m.pos.started && m.pos.notify.is_none() {
            m.pos.current = stationary(at);
            if let Some(c) = m.clip.as_mut() {
                c.0 = at;
            }
        }
    }

    /// [`Self::set_origin`]'s twin for `.angles` (`Scr_SetAngles`).
    pub fn set_angles(&mut self, id: EntId, at: Vec3) {
        let Some(m) = self.rows.get_mut(&id) else {
            return;
        };
        if m.apos.started && m.apos.notify.is_none() {
            m.apos.current = stationary(at);
            if let Some(c) = m.clip.as_mut() {
                c.1 = at;
            }
        }
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
    /// delta, `moveto` a destination), from `from`, the origin script
    /// reads ([`Plan::start_toward`]). A verb on a moving entity starts its
    /// `trBase` where the running trajectory has it at `now_ms`, a frame
    /// further on: the ride capture's `moveto` on the descending slab sent
    /// `trBase` z 28 with a velocity of -30 from the 30 script read, ran out
    /// 2 units short and settled on `dest` (movers doc, section 14). Returns
    /// the start, which the verb writes into `r.currentOrigin` and links:
    /// `getorigin()` reads it straight after the call (movers doc 12, "A
    /// blocked mover").
    pub fn move_to(&mut self, id: EntId, now_ms: i32, from: Vec3, dest: Vec3, m: Ramp) -> Vec3 {
        let row = self.row(id);
        let start = row.pos.start_toward(now_ms, from, dest, m);
        row.pos.notify = Some(MOVEDONE);
        start
    }

    /// `rotateto` and the three axis verbs, per component from `from`,
    /// `r.currentAngles`, to `dest` (`rotateto`'s caller folds `dest` within
    /// 180 of `from`, movers doc 23), on the moves' setup. A stationary group
    /// starts from `from`, not from its settled `trBase`: the two differ by
    /// the quantization `Reached_ScriptMover` leaves (movers doc 12, "The
    /// residual yaw"). Returns the start, which the verb's own link snaps
    /// into `r.currentAngles` ([`snap_angles`]).
    pub fn rotate_to(&mut self, id: EntId, now_ms: i32, from: Vec3, dest: Vec3, m: Ramp) -> Vec3 {
        let row = self.row(id);
        let start = row.apos.start_toward(now_ms, from, dest, m);
        row.apos.notify = Some(ROTATEDONE);
        start
    }

    /// `movegravity(velocity, seconds)`: one gravity segment from `from`,
    /// `r.currentOrigin` with no evaluation of what ran before (0x60667),
    /// bounded only in when the notify comes.
    pub fn move_gravity(&mut self, id: EntId, now_ms: i32, from: Vec3, velocity: Vec3, secs: f32) {
        let row = self.row(id);
        row.pos.current = Trajectory {
            tr_type: TR_GRAVITY,
            tr_time: now_ms,
            tr_duration: trunc_ms(secs),
            base: from,
            delta: velocity,
        };
        row.pos.notify = Some(MOVEDONE);
        row.pos.started = true;
    }

    /// `rotatevelocity(degreesPerSecond, seconds, accel, decel)`
    /// ([`Plan::start_velocity`]): retail's own capture ended a `(0,180,0)`
    /// over 2 s at 360 degrees.
    pub fn rotate_velocity(
        &mut self,
        id: EntId,
        now_ms: i32,
        from: Vec3,
        v: Vec3,
        m: Ramp,
    ) -> Vec3 {
        let row = self.row(id);
        let start = row.apos.start_velocity(now_ms, from, v, m);
        row.apos.notify = Some(ROTATEDONE);
        start
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
/// The write-back is what the capture shows retail doing (`getorigin()` and
/// `.angles` both read the interpolated value on every frame of a move), and
/// it is also what keeps this to one representation: `istouching`, the touch
/// pass, `getorigin` and the wire build all go on reading the entity's own
/// fields. `.angles` is `r.currentAngles`: raw while a rotate runs, through
/// `AngleNormalize360`/`180` once it ends, while the wire keeps the
/// trajectory (movers doc 8, and 12 "The residual yaw").
pub fn run_one(host: &mut GameHost, cx: &mut vcod_gsc::Cx, id: EntId) -> (Vec<Done>, Option<Step>) {
    match run_turn(host, cx, id) {
        (done, Turn::Push(step)) => (done, Some(step)),
        (done, Turn::Team(step)) => (done, step),
        (done, Turn::Done) => (done, None),
    }
}

/// What a mover's turn in the entity pass leaves for the server.
pub enum Turn {
    /// Nothing to push; `reached` and the think have run.
    Done,
    /// One brush model's push ([`run_one`]).
    Push(Step),
    /// A moving team master's `G_MoverTeam` (0x55684): its own push, if
    /// any, then each part's in chain order, all put back on a block
    /// (`crate::game::script::ScriptRuntime::team_move`).
    Team(Option<Step>),
}

/// [`run_one`] that hands a moving team master's turn back whole.
pub fn run_turn(host: &mut GameHost, cx: &mut vcod_gsc::Cx, id: EntId) -> (Vec<Done>, Turn) {
    let team = is_moving(host, id)
        && host
            .ents
            .get(id)
            .is_some_and(|e| !e.team.slave && e.team.chain.is_some());
    let (done, step) = evaluate(host, cx, id);
    if team {
        return (done, Turn::Team(step));
    }
    match step {
        Some(step) => (done, Turn::Push(step)),
        None => {
            reached(host, cx, id);
            (done, Turn::Done)
        }
    }
}

/// Whether either group's trajectory is moving, `G_RunMover`'s gate on
/// `G_MoverTeam` (0x57666..0x57670).
fn is_moving(host: &GameHost, id: EntId) -> bool {
    host.movers.rows.get(&id).is_some_and(|m| {
        [&m.pos, &m.apos]
            .iter()
            .any(|p| p.started && p.current.tr_type != TR_STATIONARY)
    })
}

/// One part's evaluation inside `G_MoverTeam`: [`run_one`] with no
/// `reached`, which the team's walk calls once every part has pushed.
pub fn evaluate_part(
    host: &mut GameHost,
    cx: &mut vcod_gsc::Cx,
    id: EntId,
) -> (Vec<Done>, Option<Step>) {
    evaluate(host, cx, id)
}

fn evaluate(host: &mut GameHost, cx: &mut vcod_gsc::Cx, id: EntId) -> (Vec<Done>, Option<Step>) {
    let level_ms = host.level_time_ms;
    let link_snap = host.profile.link_snap;
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
        .filter(|p| {
            p.notify.is_some() && level_ms >= p.current.tr_time.wrapping_add(p.current.tr_duration)
        })
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
    let before = host.vec_field_at(cx, id, angles_atom);
    let origin_atom = cx.intern_folded("origin");
    let before_origin = host.vec_field_at(cx, id, origin_atom);
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
        (turned, link_snap(turned))
    } else {
        (before, before)
    };
    // `Reached_ScriptMover` quantizes after the links, so no link snaps it.
    let reached_at = (!m.sine && done.iter().any(|d| d.event == ROTATEDONE))
        .then(|| reached_angles(link_snap(m.apos.current.base)));
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
    // A push runs first and a blocked one never reaches `reached`
    // (`crate::game::script::ScriptRuntime::push_mover`).
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
    reached_part(host, cx, id);
    run_think(host, cx, id);
}

/// [`reached`] for one part of a team, with no think: `G_MoverTeam`'s
/// tail runs every part's `reached`, `G_RunMover` the master's think.
pub fn reached_part(host: &mut GameHost, cx: &mut vcod_gsc::Cx, id: EntId) {
    let links = host.movers.rows.get(&id).map_or(0, |m| m.reach_links);
    for _ in 0..links {
        if let Some(link) = host.area.last_link(id.0) {
            host.area.link(id.0, &link);
        }
    }
    let binary = host
        .movers
        .rows
        .get(&id)
        .is_some_and(|m| m.sine || m.door.is_some());
    if !binary {
        return;
    }
    let t = host.level_time_ms;
    for apos in [false, true] {
        let Some(m) = host.movers.rows.get(&id) else {
            return;
        };
        let p = if apos { &m.apos } else { &m.pos };
        if p.current.tr_type != TR_STATIONARY
            && t >= p.current.tr_time.wrapping_add(p.current.tr_duration)
            && host.game_error.is_none()
        {
            if m.door.is_some() {
                door::reached(host, cx, id);
            } else {
                reached_binary(host, cx, id);
            }
        }
    }
}

/// `G_RunThink` on a mover's turn (`nextthink` of 0 or less never runs).
pub fn run_think(host: &mut GameHost, cx: &mut vcod_gsc::Cx, id: EntId) {
    door::run_think(host, cx, id);
    let t = host.level_time_ms;
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
/// mover (cod11-combat.md 5.9, "useBy on a gun, a sine mover").
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

/// 1.7's `SV_LinkEntity` snap (`cod4_lnxded` 0x817e426..0x817e5ba,
/// cod4-gsc.md 3.2.6): as [`snap_angles`], but `n =
/// floor(a + 0.5)` (`frndint` rounding down, then `fistp`) against the
/// float 0x358637be (0x8212470), so a negative angle snaps too.
pub fn snap_angles4(a: Vec3) -> Vec3 {
    let snap = |c: f32| {
        let n = (f64::from(c) + 0.5).floor();
        let d = n - f64::from(c);
        if d * d < f64::from(f32::from_bits(0x3586_37be)) {
            n as f32
        } else {
            c
        }
    };
    Vec3::new(snap(a.x), snap(a.y), snap(a.z))
}

/// `Reached_ScriptMover`'s angular half (0x5fa57..0x5fb13): the settled
/// trajectory's base, linked (`a`, snapped by the caller), then pitch and roll through
/// `AngleNormalize180` and yaw through `AngleNormalize360`, both truncated
/// to 65536ths. Nothing links the entity after, so `r.currentAngles`
/// keeps the quantized value while `apos.trBase` keeps the target.
fn reached_angles(a: Vec3) -> Vec3 {
    use vcod_common::pmove::aim::{angle_normalize_180, angle_normalize_360};
    Vec3::new(
        angle_normalize_180(a.x),
        angle_normalize_360(a.y),
        angle_normalize_180(a.z),
    )
}

/// `InitMover`'s and `InitMoverRotate`'s `light` and `color` keys (1.1d
/// 0x56495..0x565a0 and 0x56769..0x56863): with either key in the block,
/// each `color` component times 255 and the `light` (default 100) times
/// 0.25, each truncated and capped at 255, go to `s.constantLight` as
/// `r | g << 8 | b << 16 | i << 24`. A script spawn reads no key
/// (docs/research/cod11-movers.md 26.7).
pub fn constant_light(keys: Option<&[(String, String)]>) -> i32 {
    use crate::game::builtins::cvar::atof;
    let key = |name: &str| keys.and_then(|b| vcod_common::bsp::spawn_string(b, name));
    let (light, color) = (key("light"), key("color"));
    if light.is_none() && color.is_none() {
        return 0;
    }
    let light = atof(light.unwrap_or("100"));
    // `G_SpawnVector`'s `sscanf("%f %f %f")`.
    let mut c = [0.0f32; 3];
    for (slot, tok) in c
        .iter_mut()
        .zip(color.unwrap_or("1 1 1").split_whitespace())
    {
        *slot = atof(tok);
    }
    let q = |v: f32, k: f32| ((f64::from(v) * f64::from(k)) as i32).min(255);
    q(c[0], 255.0) | q(c[1], 255.0) << 8 | q(c[2], 255.0) << 16 | q(light, 0.25) << 24
}

/// [`constant_light`] onto the entity.
pub fn set_constant_light(host: &mut GameHost, id: EntId, keys: Option<&[(String, String)]>) {
    let light = constant_light(keys);
    if let Some(e) = host.ents.get_mut(id) {
        e.constant_light = light;
    }
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
    let vector =
        |host: &mut GameHost, cx: &mut vcod_gsc::Cx, name: &str| host.vec_field(cx, id, name);
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
    set_constant_light(host, id, keys);
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
            door: None,
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
    set_constant_light(host, id, keys);
    host.link_shaped(cx, id, shape);
    let spawnflags = match keys {
        Some(b) => vcod_common::bsp::field_value(b, "spawnflags")
            .map_or(0, crate::game::builtins::cvar::atoi),
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
        .map_or(0, crate::game::builtins::cvar::atoi);
    let use_static = health_key != 0 || spawnflags & 6 != 0;
    if spawnflags & 6 != 0 {
        set(host, cx, "health", vcod_gsc::Value::Int(STATIC_HEALTH));
    }
    let angles = { host.vec_field(cx, id, "angles") };
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
            door: None,
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
        .keys()
        .copied()
        .filter(|&id| is_damageable_static(host, id))
        .collect();
    ids.sort_by_key(|id| id.0);
    ids
}

/// Whether `id` is one of [`damageable_statics`].
pub fn is_damageable_static(host: &GameHost, id: EntId) -> bool {
    host.movers.rows.get(&id).is_some_and(|m| m.use_static) && host.area.last_link(id.0).is_some()
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
        .is_some_and(|m| m.init_mover && !m.sine && m.door.is_none())
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
    let a = (host.profile.link_snap)(if m.apos.started {
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

/// `G_MoverTeam`'s blocked arm over a whole team (0x557f4..0x5585d): every
/// part from the master runs a frame later, re-evaluated and linked. A part
/// that moved this frame (`steps`) goes back to where it was as [`stall`]
/// puts it; one the walk never reached is shifted where it stands.
pub fn stall_team(host: &mut GameHost, cx: &mut vcod_gsc::Cx, parts: &[EntId], steps: &[Step]) {
    for &part in parts {
        if let Some(step) = steps.iter().find(|s| s.ent == part) {
            stall(host, cx, step);
            continue;
        }
        let level = host.level_time_ms;
        let link_snap = host.profile.link_snap;
        let Some(m) = host.movers.rows.get_mut(&part) else {
            continue;
        };
        m.pos.stall();
        m.apos.stall();
        let origin = m.pos.started.then(|| m.pos.at(level));
        let angles = m.apos.started.then(|| link_snap(m.apos.at(level)));
        for (name, v) in [("origin", origin), ("angles", angles)] {
            if let Some(v) = v {
                let atom = cx.intern_folded(name);
                let _ = host.write_field(cx, part, atom, vcod_gsc::Value::Vector(v.into()));
            }
        }
        host.link_entity(cx, part);
    }
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
    let mut field = |name: &str| host.vec_field(cx, id, name);
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
    let field =
        |host: &mut GameHost, cx: &mut vcod_gsc::Cx, name: &str| host.vec_field(cx, id, name);
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

/// A verb's duration and its two ramps, all in seconds (movers doc 4).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ramp {
    pub secs: f32,
    pub accel: f32,
    pub decel: f32,
}

impl Ramp {
    pub fn new(secs: f32, accel: f32, decel: f32) -> Self {
        Ramp { secs, accel, decel }
    }

    /// The verbs' argument checks (game.mp 0x60467..0x60514 on `moveTo`,
    /// the same on every verb): the error each raises, in retail's order.
    /// The sum of the ramps is taken on the stack against the float time,
    /// so ramps that fill the time exactly pass.
    pub fn check(&self) -> Result<(), &'static str> {
        if self.secs <= 0.0 {
            return Err("total time must be positive");
        }
        if self.accel < 0.0 {
            return Err("accel time must be nonnegative");
        }
        if self.decel < 0.0 {
            return Err("decel time must be nonnegative");
        }
        if f64::from(self.accel) + f64::from(self.decel) > f64::from(self.secs) {
            return Err("accel time plus decel time is greater than total time");
        }
        Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;

    /// `movez(96, 2, 0.5, 0.5)` from z 592: the three segments retail sends
    /// (movers doc 9), 500/1000/500 ms, each carrying the cruise velocity
    /// 64 and starting where the last ran out, then the destination and the
    /// notify once.
    #[test]
    fn a_ramped_move_is_the_three_segments_retail_sends() {
        let mut plan = Plan::default();
        let from = Vec3::new(0.0, 0.0, 592.0);
        let start = plan.start_toward(
            1000,
            from,
            Vec3::new(0.0, 0.0, 688.0),
            Ramp::new(2.0, 0.5, 0.5),
        );
        plan.notify = Some(MOVEDONE);
        assert_eq!(start, from);
        for (at, z, kind) in [
            (1000, 592.0, TR_ACCELERATE),
            (1500, 608.0, TR_LINEAR_STOP),
            (2500, 672.0, TR_DECCELERATE),
        ] {
            assert_eq!(plan.advance(at), None, "not done at {at}");
            assert_eq!(plan.current.tr_type, kind, "at {at}");
            assert_eq!(plan.current.tr_time, at);
            assert_eq!(plan.current.base.z, z, "at {at}");
            assert_eq!(plan.current.delta.z, 64.0, "at {at}");
        }
        assert_eq!(plan.advance(3000), Some(MOVEDONE));
        assert_eq!(plan.current.tr_type, TR_STATIONARY);
        assert_eq!(plan.current.base.z, 688.0);
        assert_eq!(plan.advance(4000), None, "the notify comes once");
    }

    /// A script time past i32 milliseconds saturates the duration and
    /// the segment ends wrap as retail's C ints do, with no overflow
    /// panic (BUG-098).
    #[test]
    fn a_huge_move_time_wraps_without_a_panic() {
        for ramp in [Ramp::new(3e6, 1e6, 1e6), Ramp::new(3e6, 0.0, 0.0)] {
            let mut plan = Plan::default();
            plan.start_toward(1000, Vec3::ZERO, Vec3::new(0.0, 0.0, 50.0), ramp);
            plan.notify = Some(MOVEDONE);
            for at in [1050, 2_000_000_000, i32::MAX] {
                let _ = plan.at(at);
                let _ = plan.advance(at);
            }
        }
    }

    /// The accel-only case says the two ends are independent:
    /// `500 / (2 - 0.25)` is 285.71, not `500 / 1.5`.
    #[test]
    fn the_cruise_speed_is_the_measured_formula() {
        let mut plan = Plan::default();
        plan.start_toward(
            0,
            Vec3::ZERO,
            Vec3::new(500.0, 0.0, 0.0),
            Ramp::new(2.0, 0.5, 0.0),
        );
        assert_eq!(plan.current.tr_type, TR_ACCELERATE);
        assert!((plan.current.delta.x - 285.714).abs() < 0.001);
        assert_eq!(plan.segs.decel, 0.0);
    }

    /// A segment that ends between frames hands over on the frame that sees
    /// it end, from where it ran out: `movex(100, 1.7, 0.33, 0.27)`'s accel
    /// lasts 330 ms and its cruise starts at 350 (movers doc 24).
    #[test]
    fn a_segment_hands_over_on_the_frame_that_sees_it_end() {
        let mut plan = Plan::default();
        plan.start_toward(
            0,
            Vec3::ZERO,
            Vec3::new(100.0, 0.0, 0.0),
            Ramp::new(1.7, 0.33, 0.27),
        );
        plan.notify = Some(MOVEDONE);
        assert_eq!(plan.current.tr_duration, 330);
        assert_eq!(plan.advance(300), None);
        assert_eq!(plan.current.tr_type, TR_ACCELERATE);
        let end = plan.at(330);
        assert_eq!(plan.at(350), end, "clamped at its end");
        plan.advance(350);
        assert_eq!(plan.current.tr_type, TR_LINEAR_STOP);
        assert_eq!((plan.current.tr_time, plan.current.base), (350, end));
    }

    /// A stall holds the running segment a frame, and the notify comes that
    /// much later (movers doc, section 12).
    #[test]
    fn a_stall_holds_the_plan_a_frame() {
        let mut plan = Plan::default();
        plan.start_toward(
            1000,
            Vec3::ZERO,
            Vec3::new(0.0, 0.0, 50.0),
            Ramp::new(1.0, 0.0, 0.0),
        );
        plan.notify = Some(MOVEDONE);
        let before = plan.at(1200);
        plan.stall();
        assert_eq!(plan.at(1250), before);
        assert_eq!(plan.advance(2000), None);
        assert_eq!(plan.advance(2050), Some(MOVEDONE));
    }

    /// Ramps that overrun the time, a time that is not positive and a
    /// negative ramp are retail's script errors, not trimmed.
    #[test]
    fn the_verbs_argument_checks_are_retails() {
        assert_eq!(Ramp::new(1.0, 0.5, 0.5).check(), Ok(()));
        assert!(Ramp::new(1.0, 0.6, 0.5).check().is_err());
        assert!(Ramp::new(0.0, 0.0, 0.0).check().is_err());
        assert!(Ramp::new(1.0, -0.1, 0.0).check().is_err());
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
        assert!((pos.evaluate_f32(375).z - 156.25).abs() < 0.01);
        assert!((pos.evaluate_f32(3000).z + 2600.0).abs() < 0.01);

        // It stops where it is on the frame its time runs out (0x5f93b).
        let row = movers.rows.get_mut(&id).unwrap();
        assert_eq!(row.pos.advance(3000), Some(MOVEDONE));
        assert_eq!(row.pos.current.tr_type, TR_STATIONARY);
        assert!((row.pos.current.evaluate_f32(3500).z + 2600.0).abs() < 0.01);
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
