//! Client-side prediction: the newest snapshot's playerstate with every cmd
//! the server has not run yet replayed on top through the server's own step
//! (`vcod_common::pmove::predict`), and retail's `cg_errordecay` easing out
//! what a new snapshot corrects.

use super::cmds::{CmdRing, CMD_MS};
use glam::Vec3;
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
    /// Eased between the results before and after the newest cmd, minus
    /// what is left of the correction.
    pub origin: Vec3,
    /// Eased the same way as `origin`.
    pub view_height: f32,
    /// Raw 16-bit wire values, the prone cone's push included.
    pub delta_angles: [i32; 3],
    /// Read by nothing yet; the predicted events are stage 4's.
    #[allow(dead_code)]
    pub pred: Predicted,
}

/// The frame's two clocks.
#[derive(Clone, Copy)]
pub struct Clock {
    /// Monotonic wall ms; the error decays on it.
    pub local_ms: f64,
    /// `NetClient::server_clock_ms`, the clock cmds are stamped with.
    pub server_ms: i32,
}

/// The replay drawn last frame, and the snapshot playerstate it started from.
struct Replay {
    snap: msg::PlayerState,
    pred: Predicted,
    /// Origin and view height before the newest cmd ran; the camera eases
    /// from here to `pred` over that cmd's [`CMD_MS`].
    prev: (Vec3, f32),
}

impl Replay {
    fn new(snap: &msg::PlayerState, pred: Predicted) -> Self {
        Replay {
            snap: snap.clone(),
            prev: (pred.ps.origin, pred.ps.view_height()),
            pred,
        }
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
            self.prev = (self.pred.ps.origin, self.pred.ps.view_height());
            predict::run_cmd(&mut self.pred, cmd, world, weapons);
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
        clock: Clock,
    ) -> Option<PredictedView> {
        if !predict::predictable(ps.field_i32(p, "pm_type")) {
            self.reset();
            return None;
        }
        if clock.local_ms - self.log_ms >= 1000.0 {
            log::debug!("predict: max correction {:.2}u", self.max_correction);
            self.max_correction = 0.0;
            self.log_ms = clock.local_ms;
        }
        if matches!(&self.replay, Some(r) if r.snap == *ps) {
            let r = self.replay.as_mut().expect("matched above");
            let n = r.run(ring, world, weapons, |_| {});
            self.count(n);
        } else if !self.replay_snapshot(p, ps, ring, world, weapons, clock.local_ms) {
            return None;
        }
        let r = self.replay.as_ref().expect("replayed above");
        let pred = r.pred;
        self.last = Some((pred.command_time, pred.ps.origin));
        let error = self.error * self.decay(clock.local_ms);
        self.drawn_error = Some(error.length());
        let f = cmd_fraction(clock.server_ms, pred.command_time);
        let (prev_origin, prev_height) = r.prev;
        Some(PredictedView {
            origin: prev_origin.lerp(pred.ps.origin, f) - error,
            view_height: prev_height + (pred.ps.view_height() - prev_height) * f,
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

/// How far the camera is from the result before the newest cmd to the
/// newest's: `server_ms` runs between the [`CMD_MS`] ticks cmds are built on.
fn cmd_fraction(server_ms: i32, newest_cmd_ms: i32) -> f32 {
    (server_ms.wrapping_sub(newest_cmd_ms) as f32 / CMD_MS as f32).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use vcod_common::collision::test_world;
    use vcod_common::net::msg::UserCmd;
    use vcod_common::net::protocol::{ENTITYNUM_WORLD, PROTOCOL_V1};

    const P: &Protocol = &PROTOCOL_V1;

    /// A frame at `local_ms` whose server clock is past every cmd, so the
    /// camera sits on the newest cmd's result.
    fn clock(local_ms: f64) -> Clock {
        Clock {
            local_ms,
            server_ms: 1_000_000,
        }
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
            (pr.predict(P, &snap, &r, &world, &[], clock(0.0)), pr.misses)
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
            .predict(P, &snap, &CmdRing::default(), &world, &[], clock(0.0))
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
        pr.predict(P, &snap, &r, &world, &[], clock(0.0)).unwrap();
        assert_eq!(pr.cmds_run, 5);
        r.push(UserCmd {
            server_time: 5048,
            forward: 127,
            ..Default::default()
        });
        let v = pr.predict(P, &snap, &r, &world, &[], clock(8.0)).unwrap();
        assert_eq!(pr.cmds_run, 6);
        let full = Predictor::default()
            .predict(P, &snap, &r, &world, &[], clock(8.0))
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
            .predict(P, &snap, &r, &world, &[], clock(0.0))
            .is_none());
    }

    /// Two frames on idle cmds: the first on a snapshot at x 0, the second
    /// on a newer one that puts the player at `x`, with the teleport bit
    /// flipped or not. Returns the second frame's drawn and predicted x.
    fn corrected(x: f32, flip: bool) -> (f32, f32) {
        let world = test_world(&[]);
        let r = ring((5008..=5040).step_by(8), false);
        let mut pr = Predictor::default();
        pr.predict(P, &standing(5000, 0.0), &r, &world, &[], clock(0.0))
            .unwrap();
        let mut snap = standing(5016, x);
        if flip {
            set(&mut snap, "eFlags", 16 | 0x8);
        }
        let v = pr.predict(P, &snap, &r, &world, &[], clock(16.0)).unwrap();
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
            .predict(P, &standing(5000, 0.0), &r, &world, &[], clock(0.0))
            .unwrap();
        assert_eq!(first.origin.x, 0.0);
        let snap = standing(5016, 10.0);
        let at = |pr: &mut Predictor, ms: f64| {
            pr.predict(P, &snap, &r, &world, &[], clock(ms))
                .unwrap()
                .origin
                .x
        };
        assert_eq!(at(&mut pr, 16.0), 0.0, "the correction starts fully eased");
        assert!((at(&mut pr, 66.0) - 5.0).abs() < 1e-4);
        assert!((at(&mut pr, 116.0) - 10.0).abs() < 1e-4);
        assert_eq!(at(&mut pr, 500.0), 10.0);
    }

    #[test]
    fn cmd_fraction_runs_over_one_cmd() {
        assert_eq!(cmd_fraction(5040, 5040), 0.0);
        assert_eq!(cmd_fraction(5042, 5040), 0.25);
        assert_eq!(cmd_fraction(5046, 5040), 0.75);
        assert_eq!(cmd_fraction(5048, 5040), 1.0);
        assert_eq!(cmd_fraction(5100, 5040), 1.0, "a stalled cmd clock holds");
        assert_eq!(
            cmd_fraction(5030, 5040),
            0.0,
            "a clock re-anchored back holds"
        );
    }

    /// Between two cmd ticks the camera eases from the result before the
    /// newest cmd to the newest's instead of stepping.
    #[test]
    fn the_camera_moves_between_cmds() {
        let world = test_world(&[]);
        let snap = standing(5000, 0.0);
        let r = ring((5008..=5040).step_by(8), true);
        let mut pr = Predictor::default();
        let at = |pr: &mut Predictor, server_ms: i32| {
            let c = Clock {
                local_ms: 0.0,
                server_ms,
            };
            pr.predict(P, &snap, &r, &world, &[], c).unwrap().origin.x
        };
        let before_newest = {
            let mut short = CmdRing::default();
            for c in r.since(0).take(4) {
                short.push(*c);
            }
            Predictor::default()
                .predict(P, &snap, &short, &world, &[], clock(0.0))
                .unwrap()
                .origin
                .x
        };
        let newest = at(&mut pr, 5048);
        assert!(newest > before_newest);
        assert_eq!(at(&mut pr, 5040), before_newest);
        let half = at(&mut pr, 5044);
        assert!(
            (half - (before_newest + newest) / 2.0).abs() < 1e-4,
            "{half}"
        );
    }
}
