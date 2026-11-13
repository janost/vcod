//! Client-side prediction: the newest snapshot's playerstate with every cmd
//! the server has not run yet replayed on top through the server's own step
//! (`vcod_common::pmove::predict`), and retail's `cg_errordecay` easing out
//! what a new snapshot corrects. The snapshot's brush models are clipped
//! where they stood at its time, and a mover the player stands on carries
//! the drawn origin on from there (`vcod_common::pmove::movers`).

use super::cmds::{CMD_MS, CmdRing};
use glam::Vec3;
use std::collections::{BTreeMap, HashMap, VecDeque};
use vcod_common::collision::CollisionWorld;
use vcod_common::movetrace::{Body, CONTENTS_BODY, MoveWorld};
use vcod_common::net::flags::EF_TELEPORT_BIT;
use vcod_common::net::msg;
use vcod_common::net::protocol::Protocol;
use vcod_common::pmove::movers::SnapshotMovers;
use vcod_common::pmove::predict::{self, Predicted};
use vcod_common::weapon::WeaponDef;

/// `cg_errordecay`: how long a correction takes to ease out.
const ERROR_DECAY_MS: f64 = 100.0;
/// A correction no longer than this is not eased (`CG_PredictPlayerState`'s
/// 0.1 at cgame 0x3006953c).
const MISS_EPSILON: f32 = 0.1;
/// A correction longer than this is drawn at once.
const SNAP_DISTANCE: f32 = 256.0;
/// `eFlags` capsule bit; the client clips only capsule entities.
const EF_CAPSULE: i32 = 0x10;
use vcod_common::net::flags::ET_ITEM;
use vcod_common::net::flags::ET_PLAYER;
/// A brush submodel's `solid`; `SnapshotMovers` clips those.
use vcod_common::net::flags::SOLID_BMODEL;

/// `ANGLE2SHORT` as `CG_PredictPlayerState` stores it: degrees times
/// 182.04445 (cgame 0x30069380) on the x87 stack, truncated by `_ftol`
/// and masked to 16 bits.
fn angle_to_short(deg: f32) -> i32 {
    (f64::from(deg) * f64::from(65536.0f32 / 360.0)) as i32 & 0xffff
}

/// What the camera draws for a predicted frame.
pub struct PredictedView {
    /// The replay at the render clock, between the two cmd results around
    /// it, minus what is left of the correction.
    pub origin: Vec3,
    /// Sampled the same way as `origin`.
    pub view_height: f32,
    /// Raw 16-bit wire values, the prone cone's push included.
    pub delta_angles: [i32; 3],
    /// The replayed state at the newest cmd, for the viewmodel.
    pub pred: Predicted,
}

/// The replay drawn last frame, and the snapshot playerstate, bodies and
/// brush models it started from.
struct Replay {
    snap: msg::PlayerState,
    bodies: Vec<Body>,
    /// The snapshot's brush models and its serverTime, where they are posed.
    movers: (SnapshotMovers, i32),
    pred: Predicted,
    /// `(commandTime, origin, view height)` after each of the last few cmds
    /// run, oldest first; the camera samples between them.
    results: VecDeque<(i32, Vec3, f32)>,
    /// A ground mover's turn went into `pred.delta_angles`, so the cmds run
    /// so far hold one frame's turn and the next frame replays them.
    turned: bool,
}

/// Last frame's `commandTime` and render clock: where retail's replay
/// turns the yaw with the ground mover, and the time it turns to.
type TurnAt = Option<(i32, i32)>;

/// Enough results to cover the render clock's window of two cmds behind the
/// newest, with room for a frame's worth of new ones.
const RESULTS: usize = 6;

impl Replay {
    fn new(
        snap: &msg::PlayerState,
        bodies: &[Body],
        movers: &(SnapshotMovers, i32),
        pred: Predicted,
    ) -> Self {
        let mut r = Replay {
            snap: snap.clone(),
            bodies: bodies.to_vec(),
            movers: movers.clone(),
            pred,
            results: VecDeque::with_capacity(RESULTS),
            turned: false,
        };
        r.record();
        r
    }

    fn record(&mut self) {
        let ps = &self.pred.ps;
        if self.results.len() == RESULTS {
            self.results.pop_front();
        }
        self.results
            .push_back((self.pred.command_time, ps.origin, ps.view_height()));
    }

    /// Puts `old`'s results from before this replay's `commandTime` ahead of
    /// its own, moved by the correction at that time, so the render clock
    /// can still sit behind a snapshot that acked the cmds it is drawing.
    fn carry_older(&mut self, old: &Replay) {
        let (t, origin, height) = self.results[0];
        let (old_origin, old_height) = old.sample(f64::from(t));
        let (d_origin, d_height) = (origin - old_origin, height - old_height);
        for &(rt, o, h) in old.results.iter().rev().filter(|e| e.0 < t) {
            if self.results.len() == RESULTS {
                break;
            }
            self.results.push_front((rt, o + d_origin, h + d_height));
        }
    }

    /// Origin and view height at cmd time `t`, linear between the two
    /// results around it and held at either end.
    fn sample(&self, t: f64) -> (Vec3, f32) {
        let at = |i: usize| {
            let (_, o, h) = self.results[i];
            (o, h)
        };
        let Some(i) = self
            .results
            .iter()
            .position(|&(rt, _, _)| f64::from(rt) >= t)
        else {
            return at(self.results.len() - 1);
        };
        if i == 0 {
            return at(0);
        }
        let ((t0, o0, h0), (t1, o1, h1)) = (self.results[i - 1], self.results[i]);
        let f = ((t - f64::from(t0)) / f64::from(t1 - t0)) as f32;
        (o0.lerp(o1, f), h0 + (h1 - h0) * f)
    }

    /// `origin` carried by the ground mover from the snapshot's time to `t`,
    /// retail's `CG_AdjustPositionForMover` (cgame 0x3001baa0).
    fn carry(&self, origin: Vec3, ground: u32, t: i32) -> Vec3 {
        let (movers, at) = &self.movers;
        movers.carry(origin, ground as i32, *at, t)
    }

    /// The ground mover's yaw turn from the snapshot to `to`, as a 16-bit
    /// `delta_angles` step: 0 off a turning mover.
    fn turn(&self, to: i32) -> i32 {
        let (movers, at) = &self.movers;
        let ground = self.pred.ps.ground_entity_num() as i32;
        angle_to_short(movers.carry_yaw(ground, *at, to))
    }

    /// Whether running this frame's cmds on the cached state would skip
    /// what retail's full replay does: the cached cmds hold a turn, or the
    /// next one would take one.
    fn needs_replay(&self, ring: &CmdRing, turn_at: TurnAt) -> bool {
        let Some((old_cmd, old_t)) = turn_at else {
            return self.turned;
        };
        let new_cmd = ring.since(self.pred.command_time).next().is_some();
        new_cmd && (self.turned || (self.pred.command_time == old_cmd && self.turn(old_t) != 0))
    }

    /// Runs every cmd past `pred.command_time`, oldest first, calling
    /// `after` after each; returns how many ran. Before the first cmd past
    /// `turn_at`'s `commandTime`, reached exactly, the yaw turns with the
    /// ground mover there (cgame 0x3002970d..0x30029757).
    fn run(
        &mut self,
        ring: &CmdRing,
        world: &MoveWorld,
        weapons: &[Option<WeaponDef>],
        turn_at: TurnAt,
        mut after: impl FnMut(&Predicted),
    ) -> usize {
        let mut n = 0;
        let mut turn_at = turn_at;
        for cmd in ring.since(self.pred.command_time) {
            if let Some((old_cmd, old_t)) = turn_at
                && self.pred.command_time == old_cmd
            {
                let turn = self.turn(old_t);
                self.pred.delta_angles[1] = self.pred.delta_angles[1].wrapping_add(turn);
                self.turned |= turn != 0;
                turn_at = None;
            }
            let before = self.pred.command_time;
            predict::run_cmd(&mut self.pred, cmd, world, weapons);
            if self.pred.command_time != before {
                self.record();
            }
            after(&self.pred);
            n += 1;
        }
        n
    }
}

#[derive(Default)]
pub struct Predictor {
    /// Frames drawn unpredicted because the cmd history did not reach back
    /// to the snapshot's `commandTime`.
    pub misses: u64,
    /// Last frame's predicted `commandTime` and origin, uncorrected and
    /// carried to that `commandTime` by its ground mover.
    last: Option<(i32, Vec3)>,
    /// The teleport bit of the snapshot last predicted from.
    teleport: Option<bool>,
    error: Vec3,
    /// Local ms the error eases from: the frame before the one it was last
    /// added on, retail's `cg.oldTime`.
    error_ms: f64,
    /// The last predicted frame's local ms.
    frame_ms: Option<f64>,
    /// The error still drawn on the last frame, `None` when it was not
    /// predicted.
    drawn_error: Option<f32>,
    /// The longest correction since `log_ms`, logged once a second.
    max_correction: f32,
    log_ms: f64,
    /// Kept while the snapshot and the bodies are unchanged, so a frame runs
    /// only the cmds built since the last one. vcod's own shortcut: retail
    /// replays every cmd every frame, and this is exact only while the clip
    /// is static.
    replay: Option<Replay>,
    /// The render clock in cmd-time ms: advanced by local frame time only,
    /// so a snapshot or the server clock's re-anchor never moves it, and
    /// held within two cmds behind the newest. With the local ms it was
    /// last advanced at.
    drawn: Option<(f64, f64)>,
    /// The systeminfo fall bounds every replay lands with; the caller keeps
    /// them current.
    pub fall_heights: vcod_common::pmove::FallHeights,
    /// The cgame's movement cvars every replay runs under; the caller keeps
    /// them current.
    pub bg: vcod_common::pmove::BgCvars,
    /// In a UO vehicle seat, the vehicle's angles the view's arcs are
    /// measured from (`pm` +0x3c); the caller keeps them current.
    pub seat_angles: [f32; 3],
    #[cfg(test)]
    cmds_run: usize,
    /// Snapshots whose correction was not zero, and those past
    /// `MISS_EPSILON`: the replays' misprediction counts.
    #[cfg(test)]
    pub(crate) corrections: (usize, usize),
}

impl Predictor {
    /// `ps` is the newest snapshot's playerstate, ours, `bodies` what it
    /// clips against besides the map ([`solid_bodies`]), and `movers` its
    /// brush models with its serverTime; `None` when its `pm_type` is not one
    /// the client predicts, or when the cmd history no longer reaches back
    /// to its `commandTime` (a miss).
    #[allow(clippy::too_many_arguments)]
    pub fn predict(
        &mut self,
        p: &Protocol,
        ps: &msg::PlayerState,
        ring: &CmdRing,
        world: &CollisionWorld,
        bodies: &[Body],
        movers: &(SnapshotMovers, i32),
        weapons: &[Option<WeaponDef>],
        now_ms: f64,
    ) -> Option<PredictedView> {
        if !predict::predictable(ps.field_i32(p, "pm_type")) {
            self.reset();
            return None;
        }
        // Retail clips each brush model at the snapshot's time, not per cmd
        // (`CG_ClipMoveToEntities`, cgame 0x30028e58).
        movers.0.place(world, movers.1);
        let world = &MoveWorld::new(world, bodies, ps.field_i32(p, "clientNum") as u32);
        if now_ms - self.log_ms >= 1000.0 {
            log::debug!("predict: max correction {:.2}u", self.max_correction);
            self.max_correction = 0.0;
            self.log_ms = now_ms;
        }
        let turn_at = self.turn_at();
        // Brought up to this frame's cmds first even when a new snapshot
        // replaces it: that one carries its older results over. Retail
        // replays from the snapshot every frame, the cmds up to last
        // frame's `commandTime` with the snapshot's yaw and the newer ones
        // turned by the ground mover; on a turning mover the cache cannot
        // stand in for that, so the frame replays.
        let mut stale = false;
        if let Some(r) = &mut self.replay {
            stale = r.needs_replay(ring, turn_at);
            if !stale {
                let n = r.run(ring, world, weapons, turn_at, |_| {});
                self.count(n);
            }
        }
        let unchanged = !stale
            && self
                .replay
                .as_ref()
                .is_some_and(|r| r.snap == *ps && r.bodies == bodies && r.movers == *movers);
        if !unchanged && !self.replay_snapshot(p, ps, ring, world, movers, weapons, now_ms) {
            return None;
        }
        let r = self.replay.as_ref().expect("replayed above");
        let pred = r.pred;
        let ground = pred.ps.ground_entity_num();
        self.last = Some((
            pred.command_time,
            r.carry(pred.ps.origin, ground, pred.command_time),
        ));
        let error = self.error * self.decay(now_ms);
        self.drawn_error = Some(error.length());
        self.frame_ms = Some(now_ms);
        let newest = f64::from(pred.command_time);
        let t = match self.drawn {
            Some((t, at)) => t + (now_ms - at),
            None => newest - f64::from(CMD_MS),
        }
        .clamp(newest - f64::from(2 * CMD_MS), newest);
        self.drawn = Some((t, now_ms));
        let (origin, view_height) = r.sample(t);
        let origin = r.carry(origin, ground, t as i32);
        Some(PredictedView {
            origin: origin - error,
            view_height,
            delta_angles: pred.delta_angles,
            pred,
        })
    }

    /// Last frame's `commandTime` and render clock, while there was a last
    /// frame.
    fn turn_at(&self) -> TurnAt {
        Some((self.last?.0, self.drawn?.0 as i32))
    }

    /// A new snapshot or moved bodies: rebuild from the snapshot, replay every
    /// cmd past its `commandTime`, and ease out what it corrected. False on a
    /// miss.
    #[allow(clippy::too_many_arguments)]
    fn replay_snapshot(
        &mut self,
        p: &Protocol,
        ps: &msg::PlayerState,
        ring: &CmdRing,
        world: &MoveWorld,
        movers: &(SnapshotMovers, i32),
        weapons: &[Option<WeaponDef>],
        now_ms: f64,
    ) -> bool {
        let command_time = ps.field_i32(p, "commandTime");
        let teleport = ps.field_i32(p, "eFlags") & EF_TELEPORT_BIT != 0;
        if self.teleport.is_some_and(|t| t != teleport) {
            self.snap();
        }
        self.teleport = Some(teleport);

        // Losing only the cmd at `commandTime` costs its seeded fields, not
        // the base; anything older than that is a gap.
        let reaches = ring
            .since(i32::MIN)
            .next()
            .is_some_and(|oldest| oldest.server_time <= command_time + CMD_MS);
        if !reaches {
            self.misses += 1;
            self.snap();
            return false;
        }
        let last_cmd = ring
            .since(command_time.wrapping_sub(1))
            .next()
            .filter(|c| c.server_time == command_time);
        let mut pred = predict::from_wire(p, ps, last_cmd);
        pred.ps.fall_heights = self.fall_heights;
        pred.ps.bg = self.bg;
        if let Some(seat) = &mut pred.ps.seat {
            seat.angles = self.seat_angles;
        }
        let mut r = Replay::new(ps, world.bodies, movers, pred);
        if let Some(old) = &self.replay {
            r.carry_older(old);
        }

        // Last frame's prediction against this one's at the same cmd, both
        // carried to that cmd by their own snapshot's mover: the correction
        // the snapshot brought, and nothing for the ride itself (Q3's miss
        // test in `CG_PredictPlayerState`, cgame 0x3002972d).
        let last = self.last;
        let carried = |pred: &Predicted| {
            let (m, at) = movers;
            m.carry(
                pred.ps.origin,
                pred.ps.ground_entity_num() as i32,
                *at,
                pred.command_time,
            )
        };
        let mut at_last = last
            .filter(|&(t, _)| t == r.pred.command_time)
            .map(|_| carried(&r.pred));
        let n = r.run(ring, world, weapons, self.turn_at(), |pred| {
            if last.is_some_and(|(t, _)| t == pred.command_time) {
                at_last = Some(carried(pred));
            }
        });
        self.count(n);
        if let (Some((_, before)), Some(after)) = (last, at_last) {
            let delta = after - before;
            self.max_correction = self.max_correction.max(delta.length());
            #[cfg(test)]
            {
                self.corrections.0 += usize::from(delta != Vec3::ZERO);
                self.corrections.1 += usize::from(delta.length() > MISS_EPSILON);
            }
            if delta.length() > SNAP_DISTANCE {
                self.error = Vec3::ZERO;
            } else if delta.length() > MISS_EPSILON {
                self.error = self.error * self.decay(now_ms) + delta;
                self.error_ms = self.frame_ms.unwrap_or(now_ms);
            }
        }
        self.replay = Some(r);
        true
    }

    #[cfg_attr(not(test), allow(unused_variables))]
    fn count(&mut self, cmds: usize) {
        #[cfg(test)]
        {
            self.cmds_run += cmds;
        }
    }

    /// The correction drawn on the last frame, in units; `None` when that
    /// frame was not predicted.
    pub fn drawn_error(&self) -> Option<f32> {
        self.drawn_error
    }

    /// Forgets the last frame's prediction and any correction in flight.
    pub fn reset(&mut self) {
        self.teleport = None;
        self.snap();
    }

    fn snap(&mut self) {
        self.replay = None;
        self.drawn = None;
        self.last = None;
        self.error = Vec3::ZERO;
        self.drawn_error = None;
        self.frame_ms = None;
    }

    /// The share of the error still drawn at `now_ms`, 1 down to 0.
    fn decay(&self, now_ms: f64) -> f32 {
        ((ERROR_DECAY_MS - (now_ms - self.error_ms)) / ERROR_DECAY_MS).clamp(0.0, 1.0) as f32
    }
}

/// The snapshot's solid entities as the predictor clips them, retail's
/// `CG_BuildSolidList` and `CG_ClipMoveToEntities`
/// (`docs/research/cod11-player-clip.md`). `lerped` is where each was drawn,
/// else its `trBase` stands in.
pub fn solid_bodies(
    p: &Protocol,
    entities: &BTreeMap<u32, msg::EntityState>,
    own: u32,
    lerped: &HashMap<u32, Vec3>,
) -> Vec<Body> {
    let mut out = Vec::new();
    for (&n, e) in entities {
        let solid = e.field_i32(p, "solid");
        let etype = e.field_i32(p, "eType");
        if n == own || solid == 0 || solid == SOLID_BMODEL || etype == ET_ITEM {
            continue;
        }
        if e.field_i32(p, "eFlags") & EF_CAPSULE == 0 {
            log::debug!("predict: solid entity {n} (eType {etype}) is no capsule, skipped");
            continue;
        }
        let origin = lerped
            .get(&n)
            .copied()
            .unwrap_or_else(|| Vec3::from(e.origin(p)));
        let contents = if etype == ET_PLAYER { CONTENTS_BODY } else { 1 };
        out.push(Body::from_solid(n, origin, solid, contents));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{BTreeMap, HashMap};
    use vcod_common::collision::test_world;
    use vcod_common::movetrace::{Body, CONTENTS_BODY};
    use vcod_common::net::msg::UserCmd;
    use vcod_common::net::protocol::{ENTITYNUM_NONE, ENTITYNUM_WORLD, PROTOCOL_V1};

    const P: &Protocol = &PROTOCOL_V1;

    /// A snapshot with no brush models.
    fn still() -> (SnapshotMovers, i32) {
        Default::default()
    }

    fn set(w: &mut msg::PlayerState, name: &str, v: i32) {
        w.fields[msg::PlayerState::field_index(P, name).unwrap()] = v;
    }

    /// A player standing on `test_world`'s floor at `x` at `time`.
    fn standing(time: i32, x: f32) -> msg::PlayerState {
        let mut w = msg::PlayerState::null(P);
        set(&mut w, "eFlags", 16);
        set(&mut w, "groundEntityNum", ENTITYNUM_WORLD as i32);
        set(&mut w, "viewHeightCurrent", 60.0f32.to_bits() as i32);
        set(&mut w, "origin[0]", x.to_bits() as i32);
        set(&mut w, "commandTime", time);
        set(&mut w, "speed", 190);
        set(&mut w, "gravity", 800);
        w
    }

    /// Cmds every 8 ms over `times`, running forward when `forward` is set.
    fn ring(times: impl Iterator<Item = i32>, forward: bool) -> CmdRing {
        let mut r = CmdRing::default();
        for t in times {
            r.push(UserCmd {
                server_time: t,
                forward: if forward { 127 } else { 0 },
                ..Default::default()
            });
        }
        r
    }

    #[test]
    /// A miss is `None`, so the caller draws stage 2's interpolated snapshots.
    fn history_gap_draws_the_snapshot() {
        let world = test_world(&[]);
        let snap = standing(5000, 0.0);
        let run = |oldest: i32| {
            let mut pr = Predictor::default();
            let r = ring((oldest..=5200).step_by(8), true);
            (
                pr.predict(P, &snap, &r, &world, &[], &still(), &[], 0.0),
                pr.misses,
            )
        };

        let (v, misses) = run(5008);
        assert!(
            v.unwrap().origin.x > 10.0,
            "a history from commandTime + 8 replays"
        );
        assert_eq!(misses, 0);
        let (v, misses) = run(5016);
        assert!(v.is_none(), "oldest at commandTime + 16 is a gap");
        assert_eq!(misses, 1);
        let (v, misses) = run(5100);
        assert!(v.is_none());
        assert_eq!(misses, 1);

        let mut pr = Predictor::default();
        assert!(
            pr.predict(
                P,
                &snap,
                &CmdRing::default(),
                &world,
                &[],
                &still(),
                &[],
                0.0
            )
            .is_none()
        );
        assert_eq!(pr.misses, 1);
    }

    /// An unchanged snapshot runs only the cmd built since the last frame,
    /// and lands where a full replay does.
    #[test]
    fn an_unchanged_snapshot_runs_only_the_new_cmd() {
        let world = test_world(&[]);
        let snap = standing(5000, 0.0);
        let mut r = ring((5008..=5040).step_by(8), true);
        let mut pr = Predictor::default();
        pr.predict(P, &snap, &r, &world, &[], &still(), &[], 0.0)
            .unwrap();
        assert_eq!(pr.cmds_run, 5);
        r.push(UserCmd {
            server_time: 5048,
            forward: 127,
            ..Default::default()
        });
        let v = pr
            .predict(P, &snap, &r, &world, &[], &still(), &[], 8.0)
            .unwrap();
        assert_eq!(pr.cmds_run, 6);
        let full = Predictor::default()
            .predict(P, &snap, &r, &world, &[], &still(), &[], 8.0)
            .unwrap();
        assert_eq!(v.origin, full.origin);
        assert_eq!(v.pred.command_time, 5048);
    }

    fn entity(num: u32, fields: &[(&str, i32)], origin: Vec3) -> msg::EntityState {
        let mut e = msg::EntityState::null(P);
        e.number = num;
        let mut put = |name: &str, v: i32| {
            e.fields[msg::EntityState::field_index(P, name).unwrap()] = v;
        };
        for &(name, v) in fields {
            put(name, v);
        }
        for i in 0..3 {
            put(&format!("pos.trBase[{i}]"), origin[i].to_bits() as i32);
        }
        e
    }

    #[test]
    fn solid_bodies_keeps_capsule_players_only() {
        let standing = 6684943;
        let at = Vec3::new(40.0, 0.0, 0.0);
        let ents: BTreeMap<u32, msg::EntityState> = [
            entity(1, &[("eType", 1), ("solid", standing), ("eFlags", 16)], at),
            entity(2, &[("eType", 3), ("solid", standing), ("eFlags", 16)], at),
            entity(3, &[("eType", 1), ("solid", 0), ("eFlags", 16)], at),
            entity(4, &[("solid", 0xffffff), ("eFlags", 16)], at),
            entity(5, &[("eType", 1), ("solid", standing), ("eFlags", 16)], at),
            entity(6, &[("eType", 1), ("solid", standing)], at),
            entity(7, &[("solid", standing), ("eFlags", 16)], at),
        ]
        .into_iter()
        .map(|e| (e.number, e))
        .collect();
        let lerped = HashMap::from([(1, Vec3::new(100.0, 0.0, 0.0))]);

        let bodies = solid_bodies(P, &ents, 5, &lerped);

        let body = |entity, origin, contents| Body {
            entity,
            origin,
            mins: Vec3::new(-15.0, -15.0, -1.0),
            maxs: Vec3::new(15.0, 15.0, 70.0),
            contents,
        };
        assert_eq!(
            bodies,
            [
                body(1, Vec3::new(100.0, 0.0, 0.0), CONTENTS_BODY),
                body(7, at, 1),
            ]
        );
    }

    #[test]
    fn prediction_stops_at_a_snapshot_player() {
        let world = test_world(&[]);
        let body = Body::from_solid(9, Vec3::new(60.0, 0.0, 0.0), 6684943, CONTENTS_BODY);
        // 16 ms apart so the capped history still reaches the snapshot.
        let r = ring((5008..=6000).step_by(16), true);
        let v = Predictor::default()
            .predict(
                P,
                &standing(5000, 0.0),
                &r,
                &world,
                &[body],
                &still(),
                &[],
                0.0,
            )
            .unwrap();
        let short = 60.0 - v.pred.ps.origin.x;
        assert!((30.0..31.0).contains(&short), "stopped {short} short");

        let open = Predictor::default()
            .predict(P, &standing(5000, 0.0), &r, &world, &[], &still(), &[], 0.0)
            .unwrap();
        assert!(
            open.pred.ps.origin.x > 100.0,
            "the run reaches past it bare"
        );
    }

    /// A body that moved since the last frame replays from the snapshot, as
    /// a fresh predictor would, rather than keeping cmds run against the old
    /// pose.
    #[test]
    fn a_moved_body_replays_the_kept_cmds() {
        let world = test_world(&[]);
        let snap = standing(5000, 0.0);
        let r = ring((5008..=6000).step_by(16), true);
        let at = |x| {
            [Body::from_solid(
                9,
                Vec3::new(x, 0.0, 0.0),
                6684943,
                CONTENTS_BODY,
            )]
        };
        let mut pr = Predictor::default();
        pr.predict(P, &snap, &r, &world, &at(60.0), &still(), &[], 0.0)
            .unwrap();
        let moved = pr
            .predict(P, &snap, &r, &world, &at(80.0), &still(), &[], 8.0)
            .unwrap();
        let fresh = Predictor::default()
            .predict(P, &snap, &r, &world, &at(80.0), &still(), &[], 8.0)
            .unwrap();
        assert_eq!(moved.pred.ps.origin, fresh.pred.ps.origin);
        assert!(moved.pred.ps.origin.x > 45.0, "{}", moved.pred.ps.origin.x);
    }

    /// A player standing on a brush model that `movez(48, 2)` lifts from
    /// 1000, resting 0.125 over its top as pmove leaves him: the drawn
    /// origin rises with it between snapshots, and the next snapshot, which
    /// the server carried 1.2 units up, corrects nothing.
    #[test]
    fn a_rider_is_carried_and_the_next_snapshot_corrects_nothing() {
        let world = vcod_common::collision::submodel_test_world(
            "{\n\"classname\" \"script_brushmodel\"\n\"model\" \"*1\"\n}\n",
            &[([-64.0, -64.0, 0.0], [64.0, 64.0, 16.0])],
        );
        let slab = entity(
            177,
            &[
                ("eType", 8),
                ("solid", 0xffffff),
                ("index", 1),
                ("pos.trType", 3),
                ("pos.trTime", 1000),
                ("pos.trDuration", 2000),
                ("pos.trDelta[2]", 24.0f32.to_bits() as i32),
            ],
            Vec3::ZERO,
        );
        let ents = BTreeMap::from([(177, slab)]);
        let movers = |t| (SnapshotMovers::from_entities(P, &ents), t);
        let on_slab = |ct: i32, z: f32| {
            let mut w = standing(ct, 0.0);
            set(&mut w, "origin[2]", z.to_bits() as i32);
            set(&mut w, "groundEntityNum", 177);
            w
        };
        let mut pr = Predictor::default();
        let r = ring((1008..=1048).step_by(8), false);
        let first = pr
            .predict(
                P,
                &on_slab(1000, 16.125),
                &r,
                &world,
                &[],
                &movers(1000),
                &[],
                0.0,
            )
            .unwrap();
        assert_eq!(first.pred.ps.ground_entity_num(), 177);
        assert!((first.origin.z - 17.085).abs() < 1e-3, "{}", first.origin.z);

        let r = ring((1008..=1056).step_by(8), false);
        let next = pr
            .predict(
                P,
                &on_slab(1048, 17.325),
                &r,
                &world,
                &[],
                &movers(1050),
                &[],
                8.0,
            )
            .unwrap();
        assert!(pr.drawn_error().unwrap() < 1e-3, "{:?}", pr.drawn_error());
        assert!((next.origin.z - 17.277).abs() < 1e-3, "{}", next.origin.z);
    }

    /// On a brush model turning 90 degrees a second, the second frame's
    /// new cmd runs with the yaw turned by what the mover turned from the
    /// snapshot to the first frame's time, after a replay from the
    /// snapshot; the first frame, with no frame before it, does not turn
    /// (cgame 0x3002972d).
    #[test]
    fn a_turning_mover_turns_the_predicted_yaw() {
        let world = vcod_common::collision::submodel_test_world(
            "{\n\"classname\" \"script_brushmodel\"\n\"model\" \"*1\"\n}\n",
            &[([-64.0, -64.0, 0.0], [64.0, 64.0, 16.0])],
        );
        let wheel = entity(
            177,
            &[
                ("eType", 8),
                ("solid", 0xffffff),
                ("index", 1),
                ("apos.trType", 2),
                ("apos.trTime", 1000),
                ("apos.trDelta[1]", 90.0f32.to_bits() as i32),
            ],
            Vec3::ZERO,
        );
        let ents = BTreeMap::from([(177, wheel)]);
        let movers = (SnapshotMovers::from_entities(P, &ents), 1000);
        let mut snap = standing(1000, 0.0);
        set(&mut snap, "origin[2]", 16.125f32.to_bits() as i32);
        set(&mut snap, "groundEntityNum", 177);
        let mut pr = Predictor::default();
        let r = ring((1008..=1048).step_by(8), false);
        let first = pr
            .predict(P, &snap, &r, &world, &[], &movers, &[], 0.0)
            .unwrap();
        let base = first.delta_angles;
        let (old_t, _) = pr.drawn.unwrap();
        // The new cmd replays every cmd from the snapshot, the newest turned
        // by what the wheel turned up to last frame's render clock.
        let runs = pr.cmds_run;
        let r = ring((1008..=1056).step_by(8), false);
        let next = pr
            .predict(P, &snap, &r, &world, &[], &movers, &[], 8.0)
            .unwrap();
        let turned = angle_to_short(90.0 * (old_t as i32 - 1000) as f32 / 1000.0);
        assert!(turned > 0);
        assert_eq!(next.delta_angles[1], base[1].wrapping_add(turned));
        assert_eq!(pr.cmds_run - runs, 7);
        // No new cmd: nothing runs and the turn stays.
        let runs = pr.cmds_run;
        let same = pr
            .predict(P, &snap, &r, &world, &[], &movers, &[], 12.0)
            .unwrap();
        assert_eq!(same.delta_angles, next.delta_angles);
        assert_eq!(pr.cmds_run, runs);
    }

    /// A free-flying spectator is predicted as retail's cgame predicts one:
    /// it flies off its cmds with no gravity and through a player's body.
    #[test]
    fn spectator_flies_through_bodies() {
        let world = test_world(&[]);
        let mut snap = msg::PlayerState::null(P);
        set(&mut snap, "pm_type", 4);
        set(&mut snap, "origin[2]", 100.0f32.to_bits() as i32);
        set(&mut snap, "commandTime", 5000);
        set(&mut snap, "speed", 400);
        set(&mut snap, "groundEntityNum", ENTITYNUM_NONE as i32);
        let body = Body {
            entity: 1,
            origin: Vec3::new(40.0, 0.0, 70.0),
            mins: Vec3::new(-15.0, -15.0, -1.0),
            maxs: Vec3::new(15.0, 15.0, 70.0),
            contents: CONTENTS_BODY,
        };
        let r = ring((5008..=5400).step_by(8), true);
        let v = Predictor::default()
            .predict(P, &snap, &r, &world, &[body], &still(), &[], 0.0)
            .expect("a spectator is predicted");
        assert!(v.pred.ps.origin.x > 60.0, "{}", v.pred.ps.origin);
        assert_eq!(v.pred.ps.origin.z, 100.0);
    }

    /// A linked corpse (`pm_type` 7) is predicted through the linked arm:
    /// no fall, no slide, the eye sinking (cod11-player-clip.md 1.1).
    #[test]
    fn a_linked_corpse_is_predicted_and_holds_still() {
        let world = test_world(&[]);
        let mut snap = standing(5000, 0.0);
        set(&mut snap, "pm_type", 7);
        set(&mut snap, "velocity[0]", 150.0f32.to_bits() as i32);
        set(&mut snap, "origin[2]", 0.125f32.to_bits() as i32);
        let r = ring((5008..=5200).step_by(8), true);
        let v = Predictor::default()
            .predict(P, &snap, &r, &world, &[], &still(), &[], 0.0)
            .expect("a linked corpse is predicted");
        assert_eq!(v.pred.ps.origin, glam::Vec3::new(0.0, 0.0, 0.125));
        assert_eq!(v.pred.ps.velocity.x, 150.0);
        assert!(!v.pred.ps.on_ground);
        assert!(v.pred.ps.view_height() < 60.0);
    }

    /// Retail's cgame predicts its own corpse: one killed running slides on
    /// to a stop and its eye sinks, whatever the cmds hold.
    #[test]
    fn a_corpse_is_predicted() {
        let world = test_world(&[]);
        let mut snap = standing(5000, 0.0);
        set(&mut snap, "pm_type", 6);
        set(&mut snap, "velocity[0]", 150.0f32.to_bits() as i32);
        set(&mut snap, "origin[2]", 0.125f32.to_bits() as i32);
        let r = ring((5008..=5200).step_by(8), true);
        let v = Predictor::default()
            .predict(P, &snap, &r, &world, &[], &still(), &[], 0.0)
            .expect("a corpse is predicted");
        assert!(v.pred.ps.origin.x > 0.0, "{}", v.pred.ps.origin);
        assert_eq!(v.pred.ps.velocity.x, 0.0);
        assert!(v.pred.ps.view_height() < 60.0);
    }

    /// Two frames on idle cmds: the first on a snapshot at x 0, the second
    /// on a newer one that puts the player at `x`, with the teleport bit
    /// flipped or not. Returns the second frame's drawn and predicted x.
    fn corrected(x: f32, flip: bool) -> (f32, f32) {
        let world = test_world(&[]);
        let r = ring((5008..=5040).step_by(8), false);
        let mut pr = Predictor::default();
        pr.predict(P, &standing(5000, 0.0), &r, &world, &[], &still(), &[], 0.0)
            .unwrap();
        let mut snap = standing(5016, x);
        if flip {
            set(&mut snap, "eFlags", 16 | 0x8);
        }
        let v = pr
            .predict(P, &snap, &r, &world, &[], &still(), &[], 16.0)
            .unwrap();
        (v.origin.x, v.pred.ps.origin.x)
    }

    #[test]
    fn teleport_snaps() {
        let (drawn, predicted) = corrected(100.0, true);
        assert_eq!(predicted, 100.0);
        assert_eq!(drawn, predicted, "a teleport-bit flip snaps");
        let (drawn, predicted) = corrected(300.0, false);
        assert_eq!(drawn, predicted, "past 256 units snaps");
        let (drawn, _) = corrected(100.0, false);
        assert!(
            (drawn - 16.0).abs() < 1e-3,
            "under 256 units without a flip eases: {drawn}"
        );
    }

    #[test]
    fn error_decays_over_100_ms() {
        let world = test_world(&[]);
        let r = ring((5008..=5040).step_by(8), false);
        let mut pr = Predictor::default();
        let first = pr
            .predict(P, &standing(5000, 0.0), &r, &world, &[], &still(), &[], 0.0)
            .unwrap();
        assert_eq!(first.origin.x, 0.0);
        let snap = standing(5016, 10.0);
        let at = |pr: &mut Predictor, ms: f64| {
            pr.predict(P, &snap, &r, &world, &[], &still(), &[], ms)
                .unwrap()
                .origin
                .x
        };
        // The ease runs from the frame before the correction, retail's
        // `cg.oldTime`, so its first frame is 16 ms into it.
        assert!((at(&mut pr, 16.0) - 1.6).abs() < 1e-4);
        assert!((at(&mut pr, 66.0) - 6.6).abs() < 1e-4);
        assert!((at(&mut pr, 116.0) - 10.0).abs() < 1e-4);
        assert_eq!(at(&mut pr, 500.0), 10.0);
    }

    /// Frames at `hz` local fps, `CmdClock` building running cmds off a
    /// server clock that tracks local time minus `server_lag(frame)`, one
    /// unchanged snapshot, a resent copy of it from `resend_at` on. Returns
    /// the drawn x per frame.
    fn walk(
        hz: f64,
        frames: usize,
        server_lag: impl Fn(usize) -> i32,
        resend_at: usize,
    ) -> Vec<f32> {
        let world = test_world(&[]);
        let snap = standing(5000, 0.0);
        let mut resent = snap.clone();
        set(&mut resent, "damageEvent", 1);
        let mut clock = super::super::cmds::CmdClock::default();
        let mut r = CmdRing::default();
        let mut pr = Predictor::default();
        (0..frames)
            .map(|k| {
                let local = k as f64 * 1000.0 / hz;
                for t in clock.due(5000 + local as i32 - server_lag(k)) {
                    r.push(UserCmd {
                        server_time: t,
                        forward: 127,
                        ..Default::default()
                    });
                }
                let s = if k >= resend_at { &resent } else { &snap };
                pr.predict(P, s, &r, &world, &[], &still(), &[], local)
                    .unwrap()
                    .origin
                    .x
            })
            .collect()
    }

    /// At 60 Hz the cmd clock lands 2 or 3 cmds a frame; the render clock
    /// still moves the camera the same distance every frame.
    #[test]
    fn sixty_hz_moves_evenly() {
        let xs = walk(60.0, 120, |_| 0, usize::MAX);
        let steps: Vec<f32> = xs[60..].windows(2).map(|w| w[1] - w[0]).collect();
        let (lo, hi) = steps
            .iter()
            .fold((f32::MAX, f32::MIN), |(lo, hi), &d| (lo.min(d), hi.max(d)));
        assert!(lo > 1.0, "moving at speed: {steps:?}");
        assert!(hi - lo < 0.01, "{lo}..{hi}: {steps:?}");
    }

    /// Above the cmd rate the camera still moves every frame.
    #[test]
    fn one_forty_four_hz_never_repeats_or_goes_back() {
        let xs = walk(144.0, 300, |_| 0, usize::MAX);
        assert!(xs[20..].windows(2).all(|w| w[1] > w[0]), "{xs:?}");
    }

    /// The server clock steps back 30 ms at frame 20, as it does when a
    /// snapshot re-anchors it, and a new snapshot arrives there too. No cmd
    /// is built for a few frames, so the camera holds at the newest result,
    /// and the render clock, which runs on local time only, never steps it
    /// back or restarts the ease.
    #[test]
    fn a_server_clock_re_anchor_does_not_move_the_render_clock() {
        let lag = |k: usize| if k >= 20 { 30 } else { 0 };
        let xs = walk(60.0, 60, lag, 20);
        let even = walk(60.0, 60, |_| 0, usize::MAX);
        assert_eq!(xs[..20], even[..20]);
        assert!(xs[1..].windows(2).all(|w| w[1] >= w[0]), "{xs:?}");
        assert!(xs[30..].windows(2).all(|w| w[1] > w[0]), "{xs:?}");
    }

    /// `pred` as the server would send it, for the fields a flat run reads.
    fn wire(pred: &Predicted) -> msg::PlayerState {
        let ps = &pred.ps;
        let mut w = standing(pred.command_time, ps.origin.x);
        for i in 0..3 {
            set(
                &mut w,
                &format!("origin[{i}]"),
                ps.origin[i].to_bits() as i32,
            );
            set(
                &mut w,
                &format!("velocity[{i}]"),
                ps.velocity[i].to_bits() as i32,
            );
        }
        set(&mut w, "groundEntityNum", ps.ground_entity_num() as i32);
        set(
            &mut w,
            "viewHeightCurrent",
            ps.view_height().to_bits() as i32,
        );
        set(&mut w, "bobCycle", i32::from(ps.bob_cycle));
        set(&mut w, "movementDir", ps.movement_dir & 0xff);
        w
    }

    /// A forward run at `hz` local fps with a snapshot every 50 ms that
    /// acks all but the newest `unacked` cmds, taken off a reference run of
    /// the same cmds. Returns the drawn x per frame.
    fn walk_acked(hz: f64, frames: usize, unacked: usize) -> Vec<f32> {
        let world = test_world(&[]);
        let first = standing(5000, 0.0);
        let mut truth = vec![predict::from_wire(P, &first, None)];
        let mut snap = first;
        let mut clock = super::super::cmds::CmdClock::default();
        let mut r = CmdRing::default();
        let mut pr = Predictor::default();
        (0..frames)
            .map(|k| {
                let local = k as f64 * 1000.0 / hz;
                for t in clock.due(5000 + local as i32) {
                    let cmd = UserCmd {
                        server_time: t,
                        forward: 127,
                        ..Default::default()
                    };
                    r.push(cmd);
                    let mut next = *truth.last().unwrap();
                    predict::run_cmd(&mut next, &cmd, &MoveWorld::bare(&world), &[]);
                    truth.push(next);
                }
                let prev_local = (k as f64 - 1.0) * 1000.0 / hz;
                if k > 0 && (local / 50.0).floor() != (prev_local / 50.0).floor() {
                    snap = wire(&truth[truth.len().saturating_sub(1 + unacked)]);
                }
                pr.predict(P, &snap, &r, &world, &[], &still(), &[], local)
                    .unwrap()
                    .origin
                    .x
            })
            .collect()
    }

    fn step_range(xs: &[f32]) -> (f32, f32) {
        xs.windows(2)
            .map(|w| w[1] - w[0])
            .fold((f32::MAX, f32::MIN), |(lo, hi), d| (lo.min(d), hi.max(d)))
    }

    /// A snapshot that acks the newest cmd, or all but one, puts its
    /// `commandTime` inside the render clock's window; the camera still
    /// moves evenly across it.
    #[test]
    fn a_snapshot_inside_the_render_window_does_not_jump() {
        for unacked in [0, 1, 3] {
            let xs = walk_acked(60.0, 120, unacked);
            let (lo, hi) = step_range(&xs[60..]);
            assert!(lo > 1.0 && hi - lo < 0.01, "unacked {unacked}: {lo}..{hi}");
            let xs = walk_acked(144.0, 300, unacked);
            assert!(
                xs[20..].windows(2).all(|w| w[1] > w[0]),
                "unacked {unacked}: {xs:?}"
            );
        }
    }
}
