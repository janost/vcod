//! Playing a `.dm_1` and recording one (docs/research/cod2-demos.md).

use super::*;
use vcod_common::net::Msec;

/// `demo` / `timedemo` on a CoD2 install (docs/research/cod2-demos.md):
/// `CL_PlayDemo_f`'s `clc` fields (Mac 0x14c288) and `CL_SetCGameTime`'s
/// clock (0x17ba76).
pub(super) struct DemoPlay {
    reader: vcod_common::net::demo::DemoReader,
    /// `clc.firstDemoFrameSkipped` (+0x40194): only a new playback clears it.
    skipped: bool,
    /// `timedemo` (+0x4018c) and its count, kept for the whole demo.
    timedemo: Option<crate::play::demos::TimeDemo>,
    /// `timedemo`'s csv, when the caller asked for one
    /// ([`Cod2Session::write_timedemo_csv`]).
    csv: Option<TimedemoCsv>,
    /// `cl_freezeDemo`: the clock holds.
    freeze: bool,
    /// `CA_ACTIVE`: a snapshot since the last gamestate seeded the clock.
    pub(super) active: bool,
    /// `cl.serverTimeDelta`, `cl.serverTime`, `cl.oldServerTime`.
    delta: i32,
    pub(super) server_time: i32,
    old_server_time: i32,
    /// `clc.timeDemoBaseTime` (+0x401ac): the first snapshot's time.
    base: i32,
    /// The file ran out (or was cut short).
    ended: bool,
}

/// `timedemo`'s csv (`CL_SetCGameTime`, Mac 0x17ba76): `clc` +0x4019c, the
/// file, and +0x401a8, the last timed frame's `Sys_Milliseconds`.
struct TimedemoCsv {
    dir: PathBuf,
    r_mode: i32,
    out: Option<std::io::BufWriter<std::fs::File>>,
    last_ms: i32,
}

impl TimedemoCsv {
    /// One timed frame, before it counts: the file is opened while it is
    /// not (`FS_FOpenFileWrite`, so every frame until one opens), then each
    /// frame after the first writes `<frames counted>,<ms since the last>`.
    fn frame(&mut self, map: &str, td: &crate::play::demos::TimeDemo, now_ms: i32) {
        use std::io::Write;
        if self.out.is_none() {
            // the map is the demo's: a drive or stream colon opens nothing
            let name = timedemo_csv_name(map, self.r_mode);
            let file = vcod_common::game_dir::confined(&self.dir, &name)
                .ok_or_else(|| std::io::Error::other("not a file name"))
                .and_then(|path| {
                    std::fs::create_dir_all(&self.dir)?;
                    std::fs::File::create(path)
                });
            match file {
                Ok(f) => self.out = Some(std::io::BufWriter::new(f)),
                Err(e) => log::warn!("timedemo: {name}: {e}"),
            }
        }
        if td.started()
            && let Some(out) = self.out.as_mut()
        {
            let _ = writeln!(out, "{},{}", td.frames(), now_ms - self.last_ms);
        }
        self.last_ms = now_ms;
    }
}

/// `timedemo_<map>_mode_<r_mode>.csv`: the map is `cl.mapname`
/// (`maps/mp/<mapname>.d3dbsp`, `CL_InitCGame`, Mac 0x17b412) past its last
/// slash and short of its last dot (`CL_SetCGameTime`, 0x17ba76).
fn timedemo_csv_name(map: &str, r_mode: i32) -> String {
    let full = format!("maps/mp/{map}.d3dbsp");
    let base = full.rsplit(['/', '\\']).next().unwrap_or("");
    let base = base.rsplit_once('.').map_or(base, |(b, _)| b);
    format!("timedemo_{base}_mode_{r_mode}.csv")
}

impl Cod2Session {
    /// `demo <name>` / `timedemo <name>` (`CL_PlayDemo_f`, Mac 0x14c288):
    /// a session over the file's bytes, read up to its gamestate. The
    /// caller loads the map and draws it as a live session's.
    pub fn play_demo(data: Vec<u8>, timedemo: bool) -> anyhow::Result<Self> {
        let mut s = Cod2Session::new(None, Client::new(0, 0));
        s.demo = Some(Box::new(DemoPlay {
            reader: vcod_cod2::net::demo::reader(data),
            skipped: false,
            timedemo: timedemo.then(Default::default),
            csv: None,
            freeze: false,
            active: false,
            delta: 0,
            server_time: 0,
            old_server_time: 0,
            base: 0,
            ended: false,
        }));
        while s.view.gamestates == 0 {
            if !s.read_demo_message() {
                if let Some(why) = s.dropped.take() {
                    bail!("{why}");
                }
                bail!("the demo ends before its gamestate");
            }
        }
        s.level_gamestates = s.view.gamestates;
        s.loaded_map = s.map_name().unwrap_or_default().to_string();
        Ok(s)
    }

    /// Under `timedemo`, write its csv into `dir` (the game's `demos/`)
    /// with `r_mode`'s index in its name.
    pub fn write_timedemo_csv(&mut self, dir: PathBuf, r_mode: i32) {
        if let Some(d) = self.demo.as_mut().filter(|d| d.timedemo.is_some()) {
            d.csv = Some(TimedemoCsv {
                dir,
                r_mode,
                out: None,
                last_ms: 0,
            });
        }
    }

    /// Whether this session plays a demo.
    pub fn is_demo(&self) -> bool {
        self.demo.is_some()
    }

    /// `cl_freezeDemo` for the next frames.
    pub fn set_freeze_demo(&mut self, freeze: bool) {
        if let Some(d) = self.demo.as_mut() {
            d.freeze = freeze;
        }
    }

    /// The demo ran out: the caller runs `CL_DemoCompleted` (Mac
    /// 0x14c096): [`Self::timedemo_report`], the disconnect, `nextdemo`.
    pub fn demo_over(&self) -> bool {
        self.demo.as_ref().is_some_and(|d| d.ended)
    }

    /// `CL_DemoCompleted`'s `%i frames, %3.1f seconds: %3.1f fps` under
    /// `timedemo`, when any time passed.
    pub fn timedemo_report(&self) -> Option<String> {
        let td = self.demo.as_ref()?.timedemo.as_ref()?;
        td.report(self.real_ms() as i32)
    }

    /// `CL_ReadDemoMessage` (Mac 0x14c132): one record parsed; false once
    /// the file has none, `Demo file was truncated.` printed for a cut
    /// record and a record past `MAX_MSGLEN` dropping the session.
    fn read_demo_message(&mut self) -> bool {
        use vcod_common::net::demo::DemoEnd;
        let Some(d) = self.demo.as_mut() else {
            return false;
        };
        if d.ended {
            return false;
        }
        match d.reader.next_message() {
            Ok((seq, msg)) => {
                match self.client.demo_message(seq as i32, &msg) {
                    Ok(Some(p)) => self.on_message(p),
                    Ok(None) => {}
                    Err(e) => {
                        self.stats.parse_errors += 1;
                        log::warn!("cod2: demo message parse failed: {e:#}");
                    }
                }
                true
            }
            Err(end) => {
                d.ended = true;
                // `CL_DemoCompleted` closes the csv (the drop flushes it).
                d.csv = None;
                match end {
                    DemoEnd::Completed => {}
                    DemoEnd::Truncated => crate::console::log::print("Demo file was truncated."),
                    DemoEnd::TooLong(_) => {
                        self.dropped = Some("CL_ReadDemoMessage: demoMsglen > MAX_MSGLEN".into())
                    }
                }
                false
            }
        }
    }

    /// `CL_SetCGameTime`'s demo arm (Mac 0x17ba76). Primed, the first
    /// frame is skipped, then one message a frame until a snapshot not
    /// flagged 2 seeds the clock and the base; active, the clock runs on
    /// the wall clock (held under `cl_freezeDemo`, never adjusted:
    /// `CL_AdjustTimeDelta` returns, 0x17b544) or, under `timedemo`, 50 ms
    /// a frame past the base, and messages are read while the newest
    /// snapshot is no later than it. A gamestate primes it again.
    fn demo_frame(&mut self) {
        let real = self.real_ms() as i32;
        let gamestates = self.view.gamestates;
        let Some(d) = self.demo.as_mut() else {
            return;
        };
        if d.ended {
            return;
        }
        if !d.active {
            if !d.skipped {
                d.skipped = true;
                return;
            }
            let snaps = self.stats.snapshots;
            if !self.read_demo_message() {
                return;
            }
            let fresh = self.stats.snapshots > snaps;
            let Some((time, flags)) = self
                .client
                .parser
                .snapshot()
                .map(|s| (s.server_time, s.snap_flags))
            else {
                return;
            };
            if !fresh || flags & 2 != 0 || self.view.gamestates != gamestates {
                return;
            }
            let d = self.demo.as_mut().expect("playing");
            d.active = true;
            d.delta = (time - real).raw();
            d.old_server_time = time.raw();
            d.server_time = time.raw();
            d.base = time.raw();
        }
        let d = self.demo.as_mut().expect("playing");
        if !d.freeze {
            d.server_time = (real + d.delta).max(d.old_server_time);
            d.old_server_time = d.server_time;
        }
        if let Some(td) = d.timedemo.as_mut() {
            if let Some(csv) = d.csv.as_mut() {
                let map = self.view.configstrings.map_name().unwrap_or_default();
                csv.frame(map, td, real);
            }
            d.server_time = d.base + td.step(real) * 50;
        }
        let until = d.server_time;
        while self
            .client
            .parser
            .snapshot()
            .is_some_and(|s| s.server_time <= Msec::from_raw(until))
        {
            if !self.read_demo_message() {
                return;
            }
            if self.view.gamestates != gamestates {
                if let Some(d) = self.demo.as_mut() {
                    d.active = false;
                }
                return;
            }
        }
    }

    /// A demo's frame: read on its clock, the camera on the recorder's
    /// eye, which [`Self::frame`] lerps between the snapshots. Nothing is
    /// predicted or sent (`cg.demoPlayback`, Mac 0x1ddafa).
    pub(super) fn demo_fly(&mut self, cam: &mut FlyCamera) {
        self.demo_frame();
        if let Some(f) = self.frame() {
            cam.pos = self.snapshot_eye(&f);
            let [pitch, yaw, _] = f.snap.ps.viewangles;
            cam.pitch = -pitch.to_radians();
            cam.yaw = yaw.to_radians();
        }
    }

    /// `record [name]` (`CL_Record_f`, Mac 0x149856) into `dir` (the
    /// game's `demos/`): `<name>.dm_1`, or the first free `demoNNNN`.
    /// Returns the console line.
    pub fn start_recording(&mut self, dir: &Path, name: Option<&str>) -> String {
        use vcod_cod2::net::demo::EXTENSION;
        if self.is_recording() {
            return "Already recording.".into();
        }
        // `cls.state` 8, `CA_ACTIVE`: a snapshot is in.
        if self.view.snap.is_none() {
            return "You must be in a level to record.".into();
        }
        use crate::play::demos::{auto_name, demo_file};
        // `clc.demoName` holds 63 characters.
        let name = match name {
            Some(n) => n.chars().take(63).collect(),
            None => auto_name(dir, EXTENSION),
        };
        let qpath = format!("demos/{name}.dm_{EXTENSION}");
        let Some(file) = demo_file(dir, &name, EXTENSION) else {
            return "ERROR: couldn't open.".into();
        };
        let out = std::fs::create_dir_all(dir).and_then(|_| std::fs::File::create(file));
        let Ok(out) = out else {
            return "ERROR: couldn't open.".into();
        };
        match self.client.start_recording(
            Box::new(std::io::BufWriter::new(out)),
            &name,
            self.view.configstrings.all(),
        ) {
            Ok(()) => format!("recording to {qpath}."),
            Err(e) => format!("{e:#}"),
        }
    }

    /// `stoprecord` (`CL_StopRecord_f`, Mac 0x14724e); the console line.
    pub fn stop_recording(&mut self) -> String {
        match self.client.stop_recording() {
            None => "Not recording a demo.".into(),
            Some(Ok(_)) => "Stopped demo.".into(),
            Some(Err(e)) => format!("ERROR: {e}"),
        }
    }

    pub fn is_recording(&self) -> bool {
        self.client.recording_status().is_some()
    }

    /// The demo's name and its bytes so far, for the `RECORDING` line.
    pub fn recording_status(&self) -> Option<(&str, u64)> {
        self.client.recording_status()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `timedemo_<map>_mode_<r_mode>.csv`, the map short of its directory
    /// and last extension.
    #[test]
    fn the_timedemo_csv_is_named_for_the_map_and_mode() {
        assert_eq!(
            timedemo_csv_name("mp_toujane", 3),
            "timedemo_mp_toujane_mode_3.csv"
        );
        assert_eq!(timedemo_csv_name("a.b", -1), "timedemo_a.b_mode_-1.csv");
        assert_eq!(timedemo_csv_name("x/y", 0), "timedemo_y_mode_0.csv");
        assert_eq!(timedemo_csv_name("", 0), "timedemo__mode_0.csv");
    }

    /// Retail's own demo under `timedemo`: the csv opens on the first timed
    /// frame and takes `<frame>,<ms>` from the second, the frames counted
    /// before it, one up a row, and is whole once the demo ends; a plain
    /// `demo` writes none.
    #[test]
    fn timedemo_writes_a_row_a_frame_after_the_first() {
        let data = include_bytes!("../../../../cod2/tests/fixtures/demos/mp_toujane-retail.dm_1");
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().to_path_buf();
        let fs = Pk3Fs::empty();
        let mut cam = FlyCamera::new(Vec3::ZERO, 0.0);
        let mut plain = Cod2Session::play_demo(data.to_vec(), false).unwrap();
        plain.write_timedemo_csv(dir.clone(), 3);
        assert!(plain.demo.as_ref().unwrap().csv.is_none());
        let mut s = Cod2Session::play_demo(data.to_vec(), true).unwrap();
        s.write_timedemo_csv(dir.clone(), 3);
        // A fixed 16 ms frame on the frame clock (`cl_avidemo`'s step), so
        // no run packs several frames into the first millisecond.
        let step = Some(std::time::Duration::from_millis(16));
        let mut frames = 0;
        while !s.demo_over() && frames < 5000 {
            crate::frame_clock::tick(std::time::Instant::now(), step);
            s.fly(&mut cam, &fs);
            frames += 1;
        }
        crate::frame_clock::tick(std::time::Instant::now(), None);
        assert!(s.demo_over(), "the demo never ended");
        let counted = s.demo.as_ref().unwrap().timedemo.as_ref().unwrap().frames();
        let text = std::fs::read_to_string(dir.join("timedemo_mp_toujane_mode_3.csv")).unwrap();
        let rows: Vec<(i32, i32)> = text
            .lines()
            .map(|l| {
                let (f, ms) = l.split_once(',').unwrap();
                (f.parse().unwrap(), ms.parse().unwrap())
            })
            .collect();
        // A start stamped at 0 ms reads as none, so a frame in the
        // session's first millisecond writes no row.
        assert!(counted > 800, "{counted} frames");
        assert!(rows.len() as i32 >= counted - 3, "{} rows", rows.len());
        assert_eq!(rows.last().map(|r| r.0), Some(counted - 1));
        for w in rows.windows(2) {
            assert_eq!(w[1].0, w[0].0 + 1);
        }
        assert!(rows.iter().all(|r| r.1 >= 0));
    }

    /// A demo's eye is `CG_CalcViewValues`' first-person arm over the lerped
    /// playerstate (Mac 0x1d3b28 has no demo test): retail's recorder, a
    /// free spectator standing with `viewHeightCurrent` 0, sees from the
    /// arm's floor 8 over its origin, where the snapshots' plain eye is the
    /// origin itself.
    #[test]
    fn a_demo_eye_is_the_first_person_arms() {
        let data = include_bytes!("../../../../cod2/tests/fixtures/demos/mp_toujane-retail.dm_1");
        let fs = Pk3Fs::empty();
        let mut cam = FlyCamera::new(Vec3::ZERO, 0.0);
        let mut s = Cod2Session::play_demo(data.to_vec(), true).unwrap();
        let mut frames = 0;
        while !s.demo_over() && frames < 5000 {
            s.fly(&mut cam, &fs);
            let Some(f) = s.frame() else { continue };
            frames += 1;
            let ps = crate::play::cod2_predict::interpolated(&f);
            assert_eq!(ps.view_height_current, 0.0);
            assert_eq!(cam.pos, ps.origin + Vec3::Z * 8.0, "at {}", f.time_ms);
            assert_eq!(Vec3::from(f.eye()), ps.origin);
        }
        assert!(frames > 800, "{frames} frames");
    }
}
