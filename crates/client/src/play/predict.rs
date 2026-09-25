//! Client-side prediction: the newest snapshot's playerstate with every cmd
//! the server has not run yet replayed on top through the server's own step
//! (`vcod_common::pmove::predict`), and retail's `cg_errordecay` easing out
//! what a new snapshot corrects.

use super::cmds::{CmdRing, CMD_MS};
use glam::Vec3;
use std::collections::VecDeque;
use vcod_common::collision::CollisionWorld;
use vcod_common::net::msg;
use vcod_common::net::protocol::Protocol;
use vcod_common::pmove::predict::{self, Predicted};
use vcod_common::weapon::WeaponDef;

/// `cg_errordecay`: how long a correction takes to ease out.
const ERROR_DECAY_MS: f64 = 100.0;
/// A correction longer than this is drawn at once.
const SNAP_DISTANCE: f32 = 256.0;
/// `eFlags` teleport bit; it flips on every spawn (AGENTS.md, Gotchas).
const EF_TELEPORT: i32 = 0x8;

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

/// The replay drawn last frame, and the snapshot playerstate it started from.
struct Replay {
    snap: msg::PlayerState,
    pred: Predicted,
    /// `(commandTime, origin, view height)` after each of the last few cmds
    /// run, oldest first; the camera samples between them.
    results: VecDeque<(i32, Vec3, f32)>,
}

/// Enough results to cover the render clock's window of two cmds behind the
/// newest, with room for a frame's worth of new ones.
const RESULTS: usize = 6;

impl Replay {
    fn new(snap: &msg::PlayerState, pred: Predicted) -> Self {
        let mut r = Replay {
            snap: snap.clone(),
            pred,
            results: VecDeque::with_capacity(RESULTS),
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

    /// Runs every cmd past `pred.command_time`, oldest first, calling
    /// `after` after each; returns how many ran.
    fn run(
        &mut self,
        ring: &CmdRing,
        world: &CollisionWorld,
        weapons: &[Option<WeaponDef>],
        mut after: impl FnMut(&Predicted),
    ) -> usize {
        let mut n = 0;
        for cmd in ring.since(self.pred.command_time) {
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
    /// Last frame's predicted `commandTime` and origin, uncorrected.
    last: Option<(i32, Vec3)>,
    /// The teleport bit of the snapshot last predicted from.
    teleport: Option<bool>,
    error: Vec3,
    /// Local ms the error was last added to.
    error_ms: f64,
    /// The error still drawn on the last frame, `None` when it was not
    /// predicted.
    drawn_error: Option<f32>,
    /// The longest correction since `log_ms`, logged once a second.
    max_correction: f32,
    log_ms: f64,
    /// Kept while the snapshot is unchanged, so a frame runs only the cmds
    /// built since the last one (ioq3's `cg_optimizePrediction`).
    replay: Option<Replay>,
    /// The render clock in cmd-time ms: advanced by local frame time only,
    /// so a snapshot or the server clock's re-anchor never moves it, and
    /// held within two cmds behind the newest. With the local ms it was
    /// last advanced at.
    drawn: Option<(f64, f64)>,
    #[cfg(test)]
    cmds_run: usize,
}

impl Predictor {
    /// `ps` is the newest snapshot's playerstate, ours; `None` when its
    /// `pm_type` is not one the client predicts, or when the cmd history no
    /// longer reaches back to its `commandTime` (a miss).
    pub fn predict(
        &mut self,
        p: &Protocol,
        ps: &msg::PlayerState,
        ring: &CmdRing,
        world: &CollisionWorld,
        weapons: &[Option<WeaponDef>],
        now_ms: f64,
    ) -> Option<PredictedView> {
        if !predict::predictable(ps.field_i32(p, "pm_type")) {
            self.reset();
            return None;
        }
        if now_ms - self.log_ms >= 1000.0 {
            log::debug!("predict: max correction {:.2}u", self.max_correction);
            self.max_correction = 0.0;
            self.log_ms = now_ms;
        }
        // Brought up to this frame's cmds first even when a new snapshot
        // replaces it: that one carries its older results over.
        if let Some(r) = &mut self.replay {
            let n = r.run(ring, world, weapons, |_| {});
            self.count(n);
        }
        let unchanged = self.replay.as_ref().is_some_and(|r| r.snap == *ps);
        if !unchanged && !self.replay_snapshot(p, ps, ring, world, weapons, now_ms) {
            return None;
        }
        let r = self.replay.as_ref().expect("replayed above");
        let pred = r.pred;
        self.last = Some((pred.command_time, pred.ps.origin));
        let error = self.error * self.decay(now_ms);
        self.drawn_error = Some(error.length());
        let newest = f64::from(pred.command_time);
        let t = match self.drawn {
            Some((t, at)) => t + (now_ms - at),
            None => newest - f64::from(CMD_MS),
        }
        .clamp(newest - f64::from(2 * CMD_MS), newest);
        self.drawn = Some((t, now_ms));
        let (origin, view_height) = r.sample(t);
        Some(PredictedView {
            origin: origin - error,
            view_height,
            delta_angles: pred.delta_angles,
            pred,
        })
    }

    /// A new snapshot: rebuild from it, replay every cmd past its
    /// `commandTime`, and ease out what it corrected. False on a miss.
    fn replay_snapshot(
        &mut self,
        p: &Protocol,
        ps: &msg::PlayerState,
        ring: &CmdRing,
        world: &CollisionWorld,
        weapons: &[Option<WeaponDef>],
        now_ms: f64,
    ) -> bool {
        let command_time = ps.field_i32(p, "commandTime");
        let teleport = ps.field_i32(p, "eFlags") & EF_TELEPORT != 0;
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
        let mut r = Replay::new(ps, predict::from_wire(p, ps, last_cmd));
        if let Some(old) = &self.replay {
            r.carry_older(old);
        }

        // Last frame's prediction against this one's at the same cmd: the
        // correction the snapshot brought.
        let last = self.last;
        let mut at_last = last
            .filter(|&(t, _)| t == r.pred.command_time)
            .map(|_| r.pred.ps.origin);
        let n = r.run(ring, world, weapons, |pred| {
            if last.is_some_and(|(t, _)| t == pred.command_time) {
                at_last = Some(pred.ps.origin);
            }
        });
        self.count(n);
        if let (Some((_, before)), Some(after)) = (last, at_last) {
            let delta = after - before;
            self.max_correction = self.max_correction.max(delta.length());
            if delta.length() > SNAP_DISTANCE {
                self.error = Vec3::ZERO;
            } else if delta != Vec3::ZERO {
                self.error = self.error * self.decay(now_ms) + delta;
                self.error_ms = now_ms;
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
    }

    /// The share of the error still drawn at `now_ms`, 1 down to 0.
    fn decay(&self, now_ms: f64) -> f32 {
        ((ERROR_DECAY_MS - (now_ms - self.error_ms)) / ERROR_DECAY_MS).clamp(0.0, 1.0) as f32
    }
}

/// The brush models the stock map-load scripts take out of the clip (AGENTS.md,
/// "A submodel's brushes are in the clip only while its entity is linked").
pub fn unlink_script_brushes(world: &CollisionWorld, entities: &str, gametype: &str) {
    let allowed: &[&str] = match gametype {
        "sd" => &["sd", "bombzone", "blocker"],
        "re" => &["re", "retrieval"],
        g => &[g][..],
    };
    for block in vcod_common::bsp::entity_blocks(entities) {
        let deleted = block
            .get("script_gameobjectname")
            .is_some_and(|name| !allowed.contains(&name.as_str()));
        let exploder = block
            .get("classname")
            .is_some_and(|c| c == "script_brushmodel")
            && block.contains_key("script_exploder")
            && block
                .get("targetname")
                .is_some_and(|t| t == "exploder" || t == "exploderchunk");
        if !deleted && !exploder {
            continue;
        }
        if let Some(n) = block
            .get("model")
            .and_then(|m| m.strip_prefix('*'))
            .and_then(|n| n.parse::<usize>().ok())
        {
            world.set_model_linked(n, false);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vcod_common::collision::test_world;
    use vcod_common::net::msg::UserCmd;
    use vcod_common::net::protocol::{ENTITYNUM_WORLD, PROTOCOL_V1};

    const P: &Protocol = &PROTOCOL_V1;

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
            (pr.predict(P, &snap, &r, &world, &[], 0.0), pr.misses)
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
        assert!(pr
            .predict(P, &snap, &CmdRing::default(), &world, &[], 0.0)
            .is_none());
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
        pr.predict(P, &snap, &r, &world, &[], 0.0).unwrap();
        assert_eq!(pr.cmds_run, 5);
        r.push(UserCmd {
            server_time: 5048,
            forward: 127,
            ..Default::default()
        });
        let v = pr.predict(P, &snap, &r, &world, &[], 8.0).unwrap();
        assert_eq!(pr.cmds_run, 6);
        let full = Predictor::default()
            .predict(P, &snap, &r, &world, &[], 8.0)
            .unwrap();
        assert_eq!(v.origin, full.origin);
        assert_eq!(v.pred.command_time, 5048);
    }

    /// mp_depot's `*1` is an exploder and a `bombzone`, so `sd` keeps the
    /// gameobject and `_load.gsc` still takes it out; `*2` carries
    /// `script_exploder` with no targetname and stays solid.
    #[test]
    fn mp_depots_exploder_leaves_the_clip_under_sd() {
        let Some(fs) = vcod_common::testing::game_fs() else {
            return;
        };
        let bsp = vcod_common::bsp::parse(&fs.read("maps/mp/mp_depot.bsp").unwrap()).unwrap();
        let world = CollisionWorld::build(&bsp, &[]);
        unlink_script_brushes(&world, &bsp.entities, "sd");
        assert!(!world.model_linked(1));
        assert!(world.model_linked(2));
    }

    #[test]
    fn not_predictable_is_none() {
        let world = test_world(&[]);
        let mut snap = standing(5000, 0.0);
        set(&mut snap, "pm_type", 6);
        let r = ring((5008..=5040).step_by(8), false);
        assert!(Predictor::default()
            .predict(P, &snap, &r, &world, &[], 0.0)
            .is_none());
    }

    /// Two frames on idle cmds: the first on a snapshot at x 0, the second
    /// on a newer one that puts the player at `x`, with the teleport bit
    /// flipped or not. Returns the second frame's drawn and predicted x.
    fn corrected(x: f32, flip: bool) -> (f32, f32) {
        let world = test_world(&[]);
        let r = ring((5008..=5040).step_by(8), false);
        let mut pr = Predictor::default();
        pr.predict(P, &standing(5000, 0.0), &r, &world, &[], 0.0)
            .unwrap();
        let mut snap = standing(5016, x);
        if flip {
            set(&mut snap, "eFlags", 16 | 0x8);
        }
        let v = pr.predict(P, &snap, &r, &world, &[], 16.0).unwrap();
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
        assert_eq!(drawn, 0.0, "under 256 units without a flip eases");
    }

    #[test]
    fn error_decays_over_100_ms() {
        let world = test_world(&[]);
        let r = ring((5008..=5040).step_by(8), false);
        let mut pr = Predictor::default();
        let first = pr
            .predict(P, &standing(5000, 0.0), &r, &world, &[], 0.0)
            .unwrap();
        assert_eq!(first.origin.x, 0.0);
        let snap = standing(5016, 10.0);
        let at = |pr: &mut Predictor, ms: f64| {
            pr.predict(P, &snap, &r, &world, &[], ms).unwrap().origin.x
        };
        assert_eq!(at(&mut pr, 16.0), 0.0, "the correction starts fully eased");
        assert!((at(&mut pr, 66.0) - 5.0).abs() < 1e-4);
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
                pr.predict(P, s, &r, &world, &[], local).unwrap().origin.x
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
                    predict::run_cmd(&mut next, &cmd, &world, &[]);
                    truth.push(next);
                }
                let prev_local = (k as f64 - 1.0) * 1000.0 / hz;
                if k > 0 && (local / 50.0).floor() != (prev_local / 50.0).floor() {
                    snap = wire(&truth[truth.len().saturating_sub(1 + unacked)]);
                }
                pr.predict(P, &snap, &r, &world, &[], local)
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
