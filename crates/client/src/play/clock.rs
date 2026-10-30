//! The client's server time, retail's `cl.serverTime`: what every entity,
//! the HUD and the usercmds are stamped and drawn at. Local time plus a
//! delta that slews toward each new snapshot by a millisecond or two, so a
//! late, early or lost snapshot never steps it (docs/protocol-1.1.md, "The
//! client's clock").

/// `CL_AdjustTimeDelta`'s thresholds (CoDMP.exe 0x404bf0): past this the
/// delta is reset to the snapshot's.
const RESET_TIME: i32 = 500;
/// Past this the delta moves halfway to the snapshot's.
const FAST_ADJUST: i32 = 100;
/// `cl_timeNudge`'s clamp in `CL_SetCGameTime` (CoDMP.exe 0x404d60).
const MAX_NUDGE: i32 = 30;
/// `CL_FinishMove` (CoDMP.exe 0x40b690) stamps no cmd past this beyond the
/// snapshot.
const MAX_CMD_AHEAD: i32 = 5000;
/// `timedemo`'s step a frame (`* 0x32`, CoDMP.exe 0x404d60).
const TIMEDEMO_MS: i32 = 50;

/// How the delta has moved, for the F3 overlay.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ClockStats {
    pub resets: u32,
    pub fast: u32,
    /// Slow drifts forward (+1) and back (-2).
    pub ahead: u32,
    pub back: u32,
    /// Frames whose time reached the newest snapshot's minus 5.
    pub extrapolated: u32,
}

/// How a demo's clock runs this frame, off `cl_freezeDemo` and
/// `timedemo` (`CL_SetCGameTime`, CoDMP.exe 0x404d60).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DemoClock {
    /// `cl_freezeDemo`: the time holds where it was.
    pub freeze: bool,
    /// `timedemo`'s frame count (`clc.timeDemoFrames`, 0x15ef014), this
    /// frame counted: the time is the first snapshot's plus 50 ms a frame,
    /// frozen or not.
    pub timedemo_frames: Option<i32>,
}

#[derive(Default)]
pub struct ServerClock {
    /// CoD2's `CL_AdjustTimeDelta` (Mac 0x17b544) instead of 1.1's
    /// ([`Self::cod2`]).
    cod2: bool,
    /// CoD4 1.7 ([`Self::cod4`]): CoD2's adjust, but a reset aims at the
    /// newest snapshot less 5 ms.
    cod4: bool,
    /// CoD2: the snapshot time `cl.snap` held before the newest
    /// (`cl` +0x2700, stored by `CL_ParseSnapshot`, Mac 0x1d5cb2).
    prev_snap: Option<i32>,
    /// The snapshot message number seen last frame, to tell a new one.
    last_snap: Option<u32>,
    /// `cl.serverTimeDelta`; `None` until the first snapshot.
    delta: Option<i32>,
    /// `cl.oldServerTime`: the time never runs back below it.
    old_server_time: i32,
    /// `cl.oldFrameServerTime`: the snapshot time last frame.
    old_frame_snap: i32,
    /// `cl.extrapolatedSnapshot`.
    extrapolated: bool,
    /// `clc.timeDemoBaseTime` (0x15ef01c): the first snapshot's time.
    demo_base: i32,
    server_time: i32,
    pub stats: ClockStats,
}

impl ServerClock {
    /// A clock under CoD2's rules: its delta aims one snapshot behind the
    /// newest and slews either way (docs/research/cod2-protocol.md 18.1).
    pub fn cod2() -> Self {
        ServerClock {
            cod2: true,
            ..Default::default()
        }
    }

    /// A clock under CoD4 1.7's rules (`CL_AdjustTimeDelta`, iw3mp.exe
    /// 0x45c0e0): CoD2's, except that a reset puts the delta at the newest
    /// snapshot's time less 5 ms (docs/research/cod4-client-view.md 1).
    /// Run it through [`Self::update_cod2`].
    pub fn cod4() -> Self {
        ServerClock {
            cod2: true,
            cod4: true,
            ..Default::default()
        }
    }

    /// CoD2's `CL_SetCGameTime` (Mac 0x17ba76) for one frame: [`Self::update`]
    /// with no `cl_timeNudge` (CoD2 registers none) and `prev`, the
    /// snapshot time before the newest, for the adjust.
    pub fn update_cod2(
        &mut self,
        realtime: i32,
        snap: i32,
        prev: Option<i32>,
        snap_num: u32,
    ) -> i32 {
        self.prev_snap = prev;
        self.update(realtime, snap, snap_num, 0)
    }

    /// `CL_SetCGameTime` for one frame: `realtime` is local ms, `snap` the
    /// newest snapshot's serverTime, `new_snapshot` whether one arrived since
    /// the last frame, `nudge` `cl_timeNudge`. Returns the frame's time.
    pub fn frame(&mut self, realtime: i32, snap: i32, new_snapshot: bool, nudge: i32) -> i32 {
        // `CL_FirstSnapshot` (CoDMP.exe 0x404d00), which consumes the new
        // snapshot flag; the same again, flag kept, when the snapshot clock
        // runs backwards.
        let mut new_snapshot = new_snapshot;
        if self.delta.is_none() {
            self.first_snapshot(realtime, snap);
            new_snapshot = false;
        } else if snap < self.old_frame_snap {
            self.first_snapshot(realtime, snap);
        }
        let delta = self.delta.expect("seeded above");
        self.old_frame_snap = snap;
        let nudge = nudge.clamp(-MAX_NUDGE, MAX_NUDGE);
        self.server_time = (realtime + delta - nudge).max(self.old_server_time);
        self.old_server_time = self.server_time;
        if realtime + delta >= snap - 5 {
            self.extrapolated = true;
            self.stats.extrapolated += 1;
        }
        if new_snapshot {
            self.adjust_delta(realtime, snap);
        }
        self.server_time
    }

    /// [`Self::frame`], telling a new snapshot by its message number, as
    /// every parsed snapshot sets `cl.newSnapshots`: a 1.5 match timeout
    /// sends snapshots whose time stands still, and the adjusts hold the
    /// clock there (docs/research/cod15-protocol-diff.md, "Match timeout").
    pub fn update(&mut self, realtime: i32, snap: i32, snap_num: u32, nudge: i32) -> i32 {
        let new_snapshot = self.last_snap != Some(snap_num);
        self.last_snap = Some(snap_num);
        self.frame(realtime, snap, new_snapshot, nudge)
    }

    /// [`Self::frame`] for a demo, whose delta never adjusts
    /// (`CL_AdjustTimeDelta` returns first while one plays, CoDMP.exe
    /// 0x404bf0). A frozen demo only seeds the delta; `timedemo` then
    /// overwrites the time.
    pub fn update_demo(&mut self, realtime: i32, snap: i32, nudge: i32, how: DemoClock) -> i32 {
        if how.freeze {
            if self.delta.is_none() || snap < self.old_frame_snap {
                self.first_snapshot(realtime, snap);
            }
            self.old_frame_snap = snap;
        } else {
            self.frame(realtime, snap, false, nudge);
        }
        if let Some(n) = how.timedemo_frames {
            self.server_time = self.demo_base + n * TIMEDEMO_MS;
        }
        self.server_time
    }

    /// Whether a snapshot has seeded the clock (`cls.state` 6, `CA_ACTIVE`).
    pub fn started(&self) -> bool {
        self.delta.is_some()
    }

    /// The time [`Self::update_demo`] will draw at `realtime`, before the
    /// frame's snapshot is known: what a demo is read up to. `None`
    /// before the first snapshot.
    pub fn time_at(&self, realtime: i32, nudge: i32, how: DemoClock) -> Option<i32> {
        let delta = self.delta?;
        if let Some(n) = how.timedemo_frames {
            return Some(self.demo_base + n * TIMEDEMO_MS);
        }
        if how.freeze {
            return Some(self.server_time);
        }
        let nudge = nudge.clamp(-MAX_NUDGE, MAX_NUDGE);
        Some((realtime + delta - nudge).max(self.old_server_time))
    }

    /// `CL_FirstSnapshot` (CoDMP.exe 0x404d00).
    fn first_snapshot(&mut self, realtime: i32, snap: i32) {
        self.delta = Some(snap - realtime);
        self.old_server_time = snap;
        self.demo_base = snap;
        self.extrapolated = false;
    }

    /// `CL_AdjustTimeDelta` (CoDMP.exe 0x404bf0).
    fn adjust_delta(&mut self, realtime: i32, snap: i32) {
        if self.cod2 {
            return self.adjust_delta_cod2(realtime, snap);
        }
        let Some(delta) = self.delta else { return };
        let new_delta = snap - realtime;
        let off = (new_delta - delta).abs();
        let delta = if off > RESET_TIME {
            self.stats.resets += 1;
            self.server_time = snap;
            self.old_server_time = snap;
            new_delta
        } else if off > FAST_ADJUST {
            self.stats.fast += 1;
            (delta + new_delta) >> 1
        } else if self.extrapolated {
            self.extrapolated = false;
            self.stats.back += 1;
            delta - 2
        } else {
            self.stats.ahead += 1;
            delta + 1
        };
        self.delta = Some(delta);
    }

    /// CoD2's `CL_AdjustTimeDelta` (Mac 0x17b544): the target is the
    /// previous snapshot's time less 5 ms, and a delta short of it by less
    /// than one snapshot interval only slews. A reset or a fast adjust
    /// measures the shortfall past that interval (all of it once the
    /// interval passes 500 ms). The slew is -2 after an extrapolated frame,
    /// else one ms toward the target (`com_timescale` 1).
    fn adjust_delta_cod2(&mut self, realtime: i32, snap: i32) {
        let Some(delta) = self.delta else { return };
        let prev = self.prev_snap.unwrap_or(snap);
        let interval = snap - prev;
        let new_delta = prev - realtime - 5;
        let short = new_delta - delta;
        let off = if short < 1 {
            Some(-short)
        } else if interval > 500 {
            Some(short)
        } else if short - interval >= 0 {
            Some(short - interval)
        } else {
            None
        };
        let delta = match off {
            Some(off) if off > RESET_TIME => {
                self.stats.resets += 1;
                self.server_time = snap;
                self.old_server_time = snap;
                if self.cod4 {
                    snap - realtime - 5
                } else {
                    new_delta
                }
            }
            Some(off) if off > FAST_ADJUST => {
                self.stats.fast += 1;
                (delta + new_delta) >> 1
            }
            _ if self.extrapolated => {
                self.extrapolated = false;
                self.stats.back += 1;
                delta - 2
            }
            _ => {
                self.stats.ahead += u32::from(delta < new_delta);
                self.stats.back += u32::from(delta > new_delta);
                delta + (new_delta - delta).signum()
            }
        };
        self.delta = Some(delta);
    }

    /// The last frame's time.
    pub fn server_time(&self) -> i32 {
        self.server_time
    }

    /// The time a cmd built now carries, held within 5 s of the snapshot.
    pub fn cmd_time(&self, snap: i32) -> i32 {
        self.server_time.min(snap + MAX_CMD_AHEAD)
    }

    /// `cl.serverTimeDelta`, for the overlay.
    pub fn delta(&self) -> Option<i32> {
        self.delta
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_snapshot_pegs_the_delta() {
        let mut c = ServerClock::default();
        assert_eq!(c.frame(1000, 50_000, true, 0), 50_000);
        assert_eq!(c.delta(), Some(49_000));
        assert_eq!(c.frame(1016, 50_000, false, 0), 50_016);
    }

    #[test]
    fn slow_drift_is_plus_one_or_minus_two() {
        let mut c = ServerClock::default();
        c.frame(0, 10_000, true, 0);
        c.extrapolated = false;
        // Snapshot 50 ms later arriving on time, drawn time well behind it:
        // the delta creeps forward.
        c.delta = Some(10_000 - 60);
        c.frame(50, 10_050, true, 0);
        assert_eq!(c.delta(), Some(10_000 - 59));
        assert_eq!(c.stats.ahead, 1);
        // A frame at or past snap - 5 marks the interval extrapolated; the
        // next snapshot pulls the delta back two.
        c.frame(105, 10_050, false, 0);
        assert!(c.extrapolated);
        c.frame(110, 10_100, true, 0);
        assert_eq!(c.delta(), Some(10_000 - 61));
        assert_eq!(c.stats.back, 1);
    }

    #[test]
    fn fast_adjust_halves_and_reset_snaps() {
        let mut c = ServerClock::default();
        c.frame(0, 10_000, true, 0);
        // 200 ms off: half the way.
        c.frame(50, 10_250, true, 0);
        assert_eq!(c.delta(), Some((10_000 + 10_200) >> 1));
        assert_eq!(c.stats.fast, 1);
        // 2 s off: reset, and the frame draws the snapshot's time.
        assert_eq!(c.frame(100, 12_300, true, 0), 12_300);
        assert_eq!(c.delta(), Some(12_200));
        assert_eq!(c.stats.resets, 1);
    }

    #[test]
    fn never_runs_backwards() {
        let mut c = ServerClock::default();
        c.frame(0, 10_000, true, 0);
        let a = c.frame(40, 10_000, false, 0);
        // A slow-down adjust and a nudge both want an earlier time; held.
        c.delta = Some(c.delta().unwrap() - 20);
        let b = c.frame(41, 10_000, false, 30);
        assert_eq!(a, b);
    }

    #[test]
    fn nudge_is_clamped_to_30() {
        let mut c = ServerClock::default();
        c.frame(0, 10_000, true, 0);
        c.old_server_time = 0;
        assert_eq!(c.frame(100, 10_000, false, 500), 10_100 - 30);
    }

    #[test]
    fn a_snapshot_clock_that_goes_back_repegs() {
        let mut c = ServerClock::default();
        c.frame(0, 50_000, true, 0);
        c.frame(16, 50_000, false, 0);
        assert_eq!(c.frame(32, 100, true, 0), 100);
        // The kept flag runs a slow adjust: the interval read extrapolated.
        assert_eq!(c.frame(48, 100, false, 0), 114);
    }

    #[test]
    fn cmd_time_holds_within_five_seconds() {
        let mut c = ServerClock::default();
        c.frame(0, 10_000, true, 0);
        c.frame(9000, 10_000, false, 0);
        assert_eq!(c.cmd_time(10_000), 15_000);
    }

    /// 20 Hz snapshots over a jittery, lossy link, 125 fps: the drawn time
    /// never steps back, never jumps more than a frame plus the slew, and
    /// settles between the two newest snapshots on all but a few percent of
    /// frames.
    #[test]
    fn settles_behind_the_newest_snapshot_under_jitter() {
        let mut c = ServerClock::default();
        let mut rng = 12345u64;
        let mut rand = || {
            rng ^= rng << 13;
            rng ^= rng >> 7;
            rng ^= rng << 17;
            (rng % 1000) as f64 / 1000.0
        };
        // (arrival local ms, snapshot time)
        let mut arrivals = Vec::new();
        for i in 0..1200 {
            if rand() < 0.02 {
                continue;
            }
            let sent = 1000 + i * 50;
            let at = sent as f64 + 50.0 + (rand() - 0.5) * 20.0;
            arrivals.push((at as i32, 10_000 + i * 50));
        }
        let mut next = 0;
        let mut snap = None;
        let mut last = None;
        let mut behind = Vec::new();
        for realtime in (1000..61_000).step_by(8) {
            let mut new = false;
            while next < arrivals.len() && arrivals[next].0 <= realtime {
                snap = Some(arrivals[next].1);
                next += 1;
                new = true;
            }
            let Some(s) = snap else { continue };
            let t = c.frame(realtime, s, new, 0);
            if let Some(l) = last {
                assert!(t >= l, "ran back");
                assert!(t - l <= 8 + 2, "jumped {} at {realtime}", t - l);
            }
            last = Some(t);
            if realtime > 11_000 {
                behind.push(s - t);
            }
        }
        assert_eq!(c.stats.resets, 0);
        let past = behind.iter().filter(|&&b| b <= 0).count();
        assert!(
            past * 20 < behind.len(),
            "past the newest snapshot on {past} of {} frames",
            behind.len()
        );
        let mean = behind.iter().sum::<i32>() as f64 / behind.len() as f64;
        eprintln!(
            "past {past} of {} frames, mean lag {mean:.1} ms, {:?}",
            behind.len(),
            c.stats
        );
        assert!((5.0..60.0).contains(&mean), "mean lag {mean}");
    }

    #[test]
    fn a_frozen_snapshot_clock_holds_the_drawn_time() {
        // The 1.5b server during a match timeout, 2026-10-13: 20 snapshots
        // a second, all at 43600. Frames every 8 ms for 15 s.
        let mut c = ServerClock::default();
        let mut num = 100;
        let mut drawn = 0;
        for frame in 0..15_000 / 8 {
            let realtime = frame * 8;
            if realtime % 50 < 8 {
                num += 1;
            }
            drawn = c.update(realtime, 43_600, num, 0);
        }
        assert!((43_600..43_800).contains(&drawn), "{drawn}");
    }

    #[test]
    fn a_frozen_demo_holds_and_resumes_from_the_held_time() {
        let run = DemoClock::default();
        let freeze = DemoClock {
            freeze: true,
            ..run
        };
        let mut c = ServerClock::default();
        assert_eq!(c.update_demo(1000, 50_000, 0, run), 50_000);
        assert_eq!(c.update_demo(1100, 50_000, 0, run), 50_100);
        assert_eq!(c.time_at(1500, 0, freeze), Some(50_100));
        assert_eq!(c.update_demo(1500, 50_200, 0, freeze), 50_100);
        // The delta never moved, so thawed the time jumps to the wall's.
        assert_eq!(c.update_demo(1600, 50_200, 0, run), 50_600);
    }

    #[test]
    fn timedemo_steps_50_ms_a_frame_off_the_first_snapshot() {
        let mut c = ServerClock::default();
        let td = |n| DemoClock {
            timedemo_frames: Some(n),
            ..DemoClock::default()
        };
        assert_eq!(c.time_at(1000, 0, td(1)), None);
        c.update_demo(1000, 50_000, 0, DemoClock::default());
        assert_eq!(c.time_at(1001, 0, td(1)), Some(50_050));
        assert_eq!(c.update_demo(1001, 50_050, 0, td(1)), 50_050);
        // Frozen too: timedemo wins.
        let frozen = DemoClock {
            freeze: true,
            timedemo_frames: Some(2),
        };
        assert_eq!(c.update_demo(1002, 50_100, 0, frozen), 50_100);
    }

    /// CoD2's adjust (Mac 0x17b544) arm by arm: the target is the previous
    /// snapshot less 5, a shortfall inside one interval slews by one either
    /// way, past it the excess is what the reset and the fast adjust read.
    #[test]
    fn cod2_aims_one_snapshot_behind() {
        let mut c = ServerClock::cod2();
        c.update_cod2(0, 10_000, None, 1);
        assert_eq!(c.delta(), Some(10_000));
        c.extrapolated = false;
        // Next at 10_050 arrives at 50: target 10_000 - 50 - 5 = 9_945,
        // the delta 55 ahead of it: slewed back one.
        c.update_cod2(50, 10_050, Some(10_000), 2);
        assert_eq!(c.delta(), Some(10_000 - 2), "extrapolated frame: -2");
        // Drawn at 10_098, inside 5 of the newest: extrapolated, -2 again.
        c.update_cod2(100, 10_100, Some(10_050), 3);
        assert_eq!(c.delta(), Some(10_000 - 4));
        // Drawn well short of it: one toward the target.
        c.update_cod2(120, 10_150, Some(10_100), 4);
        assert_eq!(c.delta(), Some(10_000 - 5));
        // Next at 10_200 (prev 10_150) at 300: target 9_845. 40 short of
        // it, inside the 50 ms interval: +1.
        c.delta = Some(9_845 - 40);
        c.extrapolated = false;
        c.update_cod2(300, 10_200, Some(10_150), 5);
        assert_eq!(c.delta(), Some(9_845 - 39));
        // 200 short: 150 past the interval, fast.
        c.delta = Some(9_845 - 200);
        c.update_cod2(300, 10_200, Some(10_150), 6);
        assert_eq!(c.delta(), Some((9_845 - 200 + 9_845) >> 1));
        assert_eq!(c.stats.fast, 1);
        // 600 ahead: reset to the target, the time held at the snapshot.
        c.delta = Some(9_845 + 600);
        assert_eq!(c.update_cod2(300, 10_200, Some(10_150), 7), 10_200);
        assert_eq!(c.delta(), Some(9_845));
        assert_eq!(c.stats.resets, 1);
    }

    /// 20 Hz snapshots over a 30 ms jitter, 5 % loss, 125 fps: the CoD2
    /// clock never steps back and draws short of the newest snapshot on
    /// nearly every frame, nearer one interval behind it than 1.1's.
    #[test]
    fn cod2_settles_a_snapshot_behind_under_jitter() {
        let mut c = ServerClock::cod2();
        let mut rng = 999u64;
        let mut rand = || {
            rng ^= rng << 13;
            rng ^= rng >> 7;
            rng ^= rng << 17;
            (rng % 1000) as f64 / 1000.0
        };
        let mut arrivals = Vec::new();
        for i in 0..1200 {
            if rand() < 0.05 {
                continue;
            }
            let at = (1000 + i * 50) as f64 + 50.0 + (rand() - 0.5) * 30.0;
            arrivals.push((at as i32, 10_000 + i * 50));
        }
        let (mut next, mut snap, mut prev, mut num) = (0, None, None, 0);
        let mut last = None;
        let mut behind = Vec::new();
        for realtime in (1000..61_000).step_by(8) {
            while next < arrivals.len() && arrivals[next].0 <= realtime {
                // In order (the netchan drops a stale one).
                if snap.is_none_or(|s| arrivals[next].1 > s) {
                    prev = snap;
                    snap = Some(arrivals[next].1);
                    num += 1;
                }
                next += 1;
            }
            let Some(s) = snap else { continue };
            let t = c.update_cod2(realtime, s, prev, num);
            if let Some(l) = last {
                assert!(t >= l, "ran back");
            }
            last = Some(t);
            if realtime > 11_000 {
                behind.push(s - t);
            }
        }
        assert_eq!(c.stats.resets, 0);
        let past = behind.iter().filter(|&&b| b <= 0).count();
        let mean = behind.iter().sum::<i32>() as f64 / behind.len() as f64;
        eprintln!(
            "past {past} of {}, mean lag {mean:.1}, {:?}",
            behind.len(),
            c.stats
        );
        assert!(past * 20 < behind.len(), "past the newest on {past} frames");
        assert!((20.0..80.0).contains(&mean), "mean lag {mean}");
    }
}
