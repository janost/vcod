//! Demos: playing a recorded `.dm_1` back on its own clock, recording the
//! live session's messages to one, and the timedemo report.

use anyhow::{Result, bail};
use glam::Vec3;
use vcod_cod4::net::conn::angle2short;
use vcod_cod4::net::parse::{Client, Op};
use vcod_cod4::pmove as pm;

use super::scene::lerp_angles;
use super::{Cod4Session, Source, ps_f32};

/// `demo` / `timedemo`: `CL_PlayDemo_f`'s `clc` fields (`iw3mp.exe`
/// 0x469120) and `CL_SetCGameTime`'s demo arm (0x45c440).
pub(super) struct DemoPlay {
    reader: vcod_cod4::net::demo::DemoReader,
    pub(super) client: Client,
    /// The archive rows the demo carries (`cl.clientArchive`).
    pub(super) archive: vcod_cod4::net::demo::ArchiveRing,
    /// Every archive row in the file, by time: the recorder's view at
    /// each of his frames, read ahead of the messages (cod4-demos.md 5).
    pub(super) views: Vec<vcod_cod4::net::demo::Archive>,
    /// `clc.firstDemoFrameSkipped` (+0x401a0).
    first_frame_skipped: bool,
    /// `timedemo` (+0x40198) and its count.
    timedemo: Option<crate::play::demos::TimeDemo>,
    /// `clc.timeDemoBaseTime` (0x934e98): the first snapshot's time.
    base_time: i32,
    /// `cl_freezeDemo`: the clock holds.
    freeze: bool,
    /// `cl.serverTimeDelta` once the first snapshot set it; `None` primed.
    pub(super) delta: Option<i32>,
    /// `cl.oldServerTime` and `cl.serverTime`.
    old_time: i32,
    pub(super) time: i32,
    /// The file has no record left.
    pub(super) ended: bool,
    /// Gamestates read (a map change inside the demo).
    pub(super) gamestates: u32,
    /// The newest server command applied to the table (`d`).
    pub(super) applied: i32,
    /// Records read, messages parsed and parse errors.
    pub(super) messages: usize,
    pub(super) errors: usize,
}

impl DemoPlay {
    /// `CL_ReadDemoMessage` (0x4690c0): one record; false once there is
    /// none. A message that fails to parse is counted and skipped, and so
    /// is one acknowledging more than 128 behind `reliableSequence`, which
    /// stays 0 on playback (0x469076..0x469099, cod4-demos.md 2).
    pub(super) fn read(&mut self) -> bool {
        use vcod_cod4::net::demo::{DemoEnd, Record};
        if self.ended {
            return false;
        }
        match self.reader.next_record() {
            Ok(Record::Archive(i, a)) => {
                self.archive.rows[i] = a;
                self.archive.index = (i + 1) % vcod_cod4::net::demo::ARCHIVE_ROWS;
                true
            }
            Ok(Record::Message(seq, msg)) => {
                self.messages += 1;
                if msg.len() < 4 {
                    return true;
                }
                let ack = i32::from_le_bytes([msg[0], msg[1], msg[2], msg[3]]);
                if ack < -0x80 {
                    return true;
                }
                match self.client.parse(seq, &msg[4..]) {
                    Ok(p) => {
                        let n = p.ops.iter().filter(|o| matches!(o, Op::Gamestate)).count();
                        self.gamestates += n as u32;
                        vcod_cod4::net::conn::apply_configstrings(
                            &mut self.client,
                            &mut self.applied,
                            &p,
                        );
                    }
                    Err(e) => {
                        self.errors += 1;
                        log::warn!("cod4: demo message {seq}: {e:#}");
                    }
                }
                true
            }
            Err(end) => {
                match end {
                    DemoEnd::Completed => {}
                    DemoEnd::Truncated => crate::console::log::print("Demo file truncated."),
                    DemoEnd::Corrupt => crate::console::log::print("Demo file corrupt."),
                    DemoEnd::TooLong(_) => {
                        crate::console::log::print("CL_ReadDemoMessage: demoMsglen > MAX_MSGLEN")
                    }
                }
                self.ended = true;
                false
            }
        }
    }

    /// The recorder's view at client time `time`, lerped between his
    /// rows either side of it, which his snapshots leave out
    /// (cod4-demos.md 5).
    pub(super) fn archived_view(&self, time: i32) -> Option<Vec3> {
        let after = self.views.partition_point(|r| r.server_time <= time);
        let before = self.views.get(after.checked_sub(1)?)?;
        let Some(next) = self.views.get(after) else {
            return Some(Vec3::from(before.view_angles));
        };
        let f = (time - before.server_time) as f32 / (next.server_time - before.server_time) as f32;
        Some(lerp_angles(
            Vec3::from(before.view_angles),
            Vec3::from(next.view_angles),
            f,
        ))
    }

    /// One frame of `CL_SetCGameTime` at local ms `realtime`: primed, the
    /// first call is skipped, then a record a frame until a snapshot
    /// without `SNAPFLAG_NOT_ACTIVE` (2) seeds the clock (`CL_FirstSnapshot`,
    /// 0x45c250); active, `serverTime = realtime + delta`, never back
    /// (held under `cl_freezeDemo`; `CL_AdjustTimeDelta` is never reached),
    /// or under `timedemo` 50 ms a frame off the first snapshot
    /// (0x45c350), then records while the newest snapshot is not past it.
    pub(super) fn frame(&mut self, realtime: i32) -> Option<i32> {
        if self.delta.is_none() {
            if !self.first_frame_skipped {
                self.first_frame_skipped = true;
                return None;
            }
            let had = self.client.snapshots.len();
            self.read();
            let snap = self.client.snapshots.last()?;
            if self.client.snapshots.len() == had || snap.snap_flags & 2 != 0 {
                return None;
            }
            let t = snap.server_time.raw();
            self.delta = Some(t - realtime);
            (self.old_time, self.time, self.base_time) = (t, t, t);
        }
        let delta = self.delta?;
        if !self.freeze {
            self.time = (realtime + delta).max(self.old_time);
        }
        if let Some(td) = self.timedemo.as_mut() {
            self.time = self.base_time + td.step(realtime) * 50;
        }
        self.old_time = self.time;
        while self
            .client
            .snapshots
            .last()
            .is_none_or(|s| self.time >= s.server_time.raw())
        {
            if !self.read() {
                break;
            }
        }
        Some(self.time)
    }
}

impl Cod4Session {
    /// What `CL_SetUserCmdOrigin` (0x45ad30) last handed the client: the
    /// predicted body, else the newest snapshot's, at `cl.serverTime`.
    pub(super) fn archive_row(&self) -> Option<vcod_cod4::net::demo::Archive> {
        let time = self.cg_time?;
        let built;
        let ps = match &self.last_predicted {
            Some(p) => p,
            None => {
                // Unpredicted, the view is the last cmd's over the newest
                // `delta_angles`, as the scene draws it.
                let snap = &self.client().snapshots.last()?.ps;
                let mut ps = pm::PlayerState::from_wire(snap);
                if let Some(a) = self.cmd_angles {
                    for (k, (v, a)) in ps.view_angles.iter_mut().zip(a).enumerate() {
                        let d = angle2short(ps_f32(snap, &format!("delta_angles[{k}]")));
                        *v = f32::from(a.wrapping_add(d) as i16) * (360.0 / 65536.0);
                    }
                }
                built = ps;
                &built
            }
        };
        Some(vcod_cod4::net::demo::Archive {
            server_time: time,
            origin: ps.origin.to_array(),
            velocity: ps.velocity.to_array(),
            bob_cycle: ps.bob_cycle,
            movement_dir: ps.movement_dir,
            view_angles: ps.view_angles,
        })
    }

    /// `demo <name>` / `timedemo <name>` (`CL_PlayDemo_f`, `iw3mp.exe`
    /// 0x469120): the file's records read up to its gamestate; the caller
    /// loads the map it names, then steps the session on the demo's clock
    /// with nothing predicted or sent (cod4-demos.md 5).
    pub fn play_demo(data: Vec<u8>, timedemo: bool) -> Result<Cod4Session> {
        let mut views = Vec::new();
        let mut scan = vcod_cod4::net::demo::DemoReader::new(data.clone());
        while let Ok(r) = scan.next_record() {
            if let vcod_cod4::net::demo::Record::Archive(_, a) = r {
                views.push(a);
            }
        }
        views.retain(|a| a.server_time != 0);
        views.sort_by_key(|a| a.server_time);
        views.dedup_by_key(|a| a.server_time);
        let mut d = DemoPlay {
            views,
            reader: vcod_cod4::net::demo::DemoReader::new(data),
            client: Client::new(),
            archive: Default::default(),
            first_frame_skipped: false,
            timedemo: timedemo.then(Default::default),
            base_time: 0,
            freeze: false,
            delta: None,
            old_time: 0,
            time: 0,
            ended: false,
            gamestates: 0,
            applied: 0,
            messages: 0,
            errors: 0,
        };
        // `CL_PlayDemo_f`'s loop: records while the state is 5 to 7, so
        // up to the gamestate's (`CL_ParseGamestate` moves it on).
        while d.client.gamestate.is_none() {
            if !d.read() {
                bail!("the demo ends before its gamestate");
            }
        }
        let n = d.gamestates;
        let mut s = Self::new(Source::Demo(Box::new(d)));
        s.gamestates = n;
        s.level_gamestates = n;
        s.loaded_map = s.map_name();
        Ok(s)
    }

    /// Whether this session plays a demo.
    pub fn is_demo(&self) -> bool {
        matches!(self.source, Source::Demo(_))
    }

    /// `cl_freezeDemo` for the next frames.
    pub fn set_freeze_demo(&mut self, freeze: bool) {
        if let Source::Demo(d) = &mut self.source {
            d.freeze = freeze;
        }
    }

    /// The demo ran out: the caller runs `CL_DemoCompleted` (0x468df0):
    /// [`Self::timedemo_report`], the disconnect, `nextdemo`.
    pub fn demo_over(&self) -> bool {
        matches!(&self.source, Source::Demo(d) if d.ended)
    }

    /// The records' messages read and how many failed to parse.
    #[cfg(test)]
    pub fn demo_counts(&self) -> Option<(usize, usize)> {
        match &self.source {
            Source::Demo(d) => Some((d.messages, d.errors)),
            _ => None,
        }
    }

    /// `CL_DemoCompleted`'s `%i frames, %3.1f seconds: %3.1f fps` under
    /// `timedemo`, when any time passed (0x468e2b).
    pub fn timedemo_report(&self, now_ms: i32) -> Option<String> {
        match &self.source {
            Source::Demo(d) => d.timedemo.as_ref()?.report(now_ms),
            _ => None,
        }
    }

    /// `record [name]` (`CL_Record_f`, 0x468960) into `dir` (the game's
    /// `demos/`): `<name>.dm_1`, or the first free `demoNNNN`. The console
    /// line it prints.
    pub fn start_recording(&mut self, dir: &std::path::Path, name: Option<&str>) -> String {
        use vcod_cod4::net::demo::{EXTENSION, Recording, gamestate_message};
        if self.recording.is_some() {
            return "Already recording.".into();
        }
        let Source::Live { conn, .. } = &mut self.source else {
            return "You must be in level to record.".into();
        };
        // `cls.state` 9 (`CA_ACTIVE`): a snapshot is in.
        let Some(gs) = conn
            .client
            .gamestate
            .as_ref()
            .filter(|_| !conn.client.snapshots.is_empty())
        else {
            return "You must be in level to record.".into();
        };
        use crate::play::demos::{auto_name, demo_file};
        let name = match name {
            // `clc.demoName` keeps 63 characters.
            Some(n) => n.chars().take(63).collect(),
            None => auto_name(dir, EXTENSION),
        };
        let qpath = format!("demos/{name}.dm_{EXTENSION}");
        let Some(file) = demo_file(dir, &name, EXTENSION) else {
            return "ERROR: couldn't open.".into();
        };
        let opened = std::fs::create_dir_all(dir).and_then(|_| std::fs::File::create(file));
        let Ok(f) = opened else {
            return "ERROR: couldn't open.".into();
        };
        let msg = gamestate_message(
            &conn.client.huff,
            conn.reliable_sequence(),
            conn.client.server_command_sequence,
            gs,
        );
        match Recording::start(
            Box::new(std::io::BufWriter::new(f)),
            &name,
            conn.message_sequence(),
            &msg,
            &self.archive,
        ) {
            Ok(r) => {
                self.recording = Some(r);
                conn.no_delta = true;
                format!("recording to {qpath}.")
            }
            Err(_) => "ERROR: couldn't open.".into(),
        }
    }

    /// `stoprecord` (`CL_StopRecord_f`, 0x4688c0); the console line.
    pub fn stop_recording(&mut self) -> String {
        let Some(r) = self.recording.take() else {
            return "Not recording demo.".into();
        };
        if let Source::Live { conn, .. } = &mut self.source {
            conn.no_delta = false;
        }
        match r.finish() {
            Ok(_) => "Stopped demo.".into(),
            Err(e) => format!("ERROR: {e}"),
        }
    }

    /// The demo's name and its bytes so far, for the `RECORDING` line.
    pub fn recording_status(&self) -> Option<(&str, u64)> {
        self.recording
            .as_ref()
            .map(|r| (r.name.as_str(), r.written()))
    }
}
