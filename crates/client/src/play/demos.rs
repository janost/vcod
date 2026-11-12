//! Where `record` writes and `demo` reads, and the 1.5 and UO automatic
//! demo (docs/research/cod11-demos.md). The file format is
//! `vcod_common::net::demo`'s.

use super::clock::{DemoClock, ServerClock};
use std::path::{Path, PathBuf};
use std::time::Instant;
use vcod_common::net::demo;
use vcod_common::net::protocol::Protocol;
use vcod_common::net::{NetClient, NetEvent, Transport};
use vcod_common::text::atoi_msvc;

/// `record`'s file under `dir` (the game directory's `demos/`) and the
/// name it keeps (`clc.demoName`): `name` with the protocol's `.dm_<n>`,
/// or the first free `demoNNNN`.
pub fn record_path(dir: &Path, name: Option<&str>, p: &Protocol) -> (PathBuf, String) {
    let ext = demo::extension_number(p);
    let file = |n: &str| dir.join(format!("{n}.dm_{ext}"));
    let name = match name {
        Some(n) => n.to_string(),
        None => demo::auto_name(|n| file(n).exists()),
    };
    (file(&name), name)
}

/// `timedemo`'s count (`clc.timeDemoFrames` 0x15ef014 and
/// `clc.timeDemoStart` 0x15ef018): kept for the whole demo, across its
/// gamestates, and reset by the next.
#[derive(Default)]
pub struct TimeDemo {
    frames: i32,
    start_ms: i32,
}

impl TimeDemo {
    /// One drawn frame under `timedemo`, `now_ms` the local clock; the
    /// count this frame draws at.
    pub fn step(&mut self, now_ms: i32) -> i32 {
        if self.start_ms == 0 {
            self.start_ms = now_ms;
        }
        self.frames += 1;
        self.frames
    }

    /// The frames counted so far.
    pub fn frames(&self) -> i32 {
        self.frames
    }

    /// Whether a frame has stamped the start (a start of 0 reads as none).
    pub fn started(&self) -> bool {
        self.start_ms != 0
    }

    /// `CL_DemoCompleted`'s line (CoDMP.exe 0x40e920) with `timedemo`
    /// set as the demo ends; nothing when no time has passed.
    pub fn report(&self, now_ms: i32) -> Option<String> {
        let ms = now_ms - self.start_ms;
        (ms > 0).then(|| {
            format!(
                "{} frames, {:3.1} seconds: {:3.1} fps",
                self.frames,
                ms as f64 * 0.001,
                self.frames as f64 * 1000.0 / ms as f64
            )
        })
    }
}

/// The demo knobs a frame reads: `cl_freezeDemo`, `timedemo` and
/// `cl_timeNudge`.
#[derive(Clone, Copy, Debug, Default)]
pub struct DemoKnobs {
    pub freeze: bool,
    pub timedemo: bool,
    pub nudge: i32,
}

/// One frame of `CL_SetCGameTime`'s demo read (CoDMP.exe 0x404d60) once
/// the map is up, `realtime` the local clock in ms. Before the first
/// snapshot it is `CA_PRIMED`: [`NetClient::pump_demo_primed`], and a
/// snapshot that message brings seeds the clock (`CL_FirstSnapshot`,
/// 0x404d00) and falls through to `CA_ACTIVE` in the same frame, which
/// counts the `timedemo` frame and reads up to the clock. Returns the
/// messages' events and the clock the frame draws with; the caller runs
/// [`ServerClock::update_demo`] with it once the events are handled.
pub fn read_frame<T: Transport>(
    net: &mut NetClient<T>,
    clock: &mut ServerClock,
    td: &mut TimeDemo,
    now: Instant,
    realtime: i32,
    knobs: DemoKnobs,
) -> (Vec<NetEvent>, DemoClock) {
    let held = DemoClock {
        freeze: knobs.freeze,
        timedemo_frames: None,
    };
    let mut events = Vec::new();
    if !clock.started() {
        events = net.pump_demo_primed(now);
        let stop = events
            .iter()
            .any(|e| matches!(e, NetEvent::GamestateReady | NetEvent::DemoEnded(_)));
        let Some(snap) = net.snapshots().newest().map(|s| s.server_time) else {
            return (events, held);
        };
        if stop {
            return (events, held);
        }
        clock.update_demo(realtime, snap, knobs.nudge, held);
    }
    let how = DemoClock {
        freeze: knobs.freeze,
        timedemo_frames: knobs.timedemo.then(|| td.step(realtime)),
    };
    let until = clock.time_at(realtime, knobs.nudge, how);
    events.extend(net.pump_demo(now, until));
    (events, how)
}

/// `demo <name>`'s file and the protocol it was recorded under. Retail
/// reads only its own `.dm_<n>` and appends it to a name without one; vcod
/// plays all three, so a bare name takes the first of `.dm_1`, `.dm_2`
/// and `.dm_3` that exists, searched through `dirs` in order.
pub fn find(dirs: &[PathBuf], name: &str) -> Option<(PathBuf, &'static Protocol)> {
    let candidates: Vec<(String, &'static Protocol)> = match demo::extension_of(name) {
        Some(n) => vec![(name.to_string(), demo::protocol_of_extension(n)?)],
        None => (1..=3)
            .filter_map(|n| Some((format!("{name}.dm_{n}"), demo::protocol_of_extension(n)?)))
            .collect(),
    };
    for dir in dirs {
        for (file, p) in &candidates {
            let path = dir.join(file);
            if path.is_file() {
                return Some((path, p));
            }
        }
    }
    None
}

/// What 1.5's and UO's `CG_ServerCommand` runs for the server's `C` and
/// `D` (cgame_mp15.dll 0x3002f660, cases 0x43 and 0x44) while
/// `cg_autodemo` is 1: `record <name>-<yyyyMMdd-HHmmss>`, the player's
/// name kept to letters and digits (0x3002f470) and the local date and
/// time off engine trap 0xf4 (CoDMP15.exe 0x46af40), and `stoprecord`.
/// 1.1's cgame knows neither command.
pub fn auto_command(
    tokens: &[String],
    p: &Protocol,
    autodemo: &str,
    name: &str,
    stamp: &str,
) -> Option<String> {
    if p.is_v1() || atoi_msvc(autodemo) != 1 {
        return None;
    }
    match tokens.first().map(String::as_str)? {
        "C" => {
            let clean: String = name.chars().filter(char::is_ascii_alphanumeric).collect();
            Some(format!("record {clean}-{stamp}"))
        }
        "D" => Some("stoprecord".to_string()),
        _ => None,
    }
}

/// Trap 0xf4's string, `yyyyMMdd-HHmmss` in local time.
pub fn stamp_now() -> String {
    jiff::Zoned::now().strftime("%Y%m%d-%H%M%S").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use vcod_common::net::protocol::PROTOCOL_V1;
    use vcod_common::net::protocol::{PROTOCOL_V6, PROTOCOL_V22};
    use vcod_common::net::{NetEvent, UdpTransport};

    fn toks(s: &str) -> Vec<String> {
        s.split(' ').map(str::to_string).collect()
    }

    #[test]
    fn auto_demo_runs_on_1_5_and_uo_only_and_only_at_1() {
        let stamp = "20261017-120000";
        let c = |p, v| auto_command(&toks("C"), p, v, "^1Big Joe!", stamp);
        assert_eq!(
            c(&PROTOCOL_V6, "1").as_deref(),
            Some("record 1BigJoe-20261017-120000")
        );
        // UO's killcam sends `C <ms>`, which reads the same.
        assert_eq!(
            auto_command(&toks("C 5000"), &PROTOCOL_V22, "1", "a", stamp).as_deref(),
            Some("record a-20261017-120000")
        );
        assert_eq!(c(&PROTOCOL_V1, "1"), None);
        assert_eq!(c(&PROTOCOL_V6, "0"), None);
        assert_eq!(c(&PROTOCOL_V6, "2"), None);
        assert_eq!(
            auto_command(&toks("D"), &PROTOCOL_V6, "1", "a", stamp).as_deref(),
            Some("stoprecord")
        );
    }

    #[test]
    fn timedemo_reports_frames_seconds_and_rate() {
        let mut t = TimeDemo::default();
        assert_eq!(t.step(2000), 1);
        for _ in 0..99 {
            t.step(2500);
        }
        assert_eq!(
            t.report(4000).as_deref(),
            Some("100 frames, 2.0 seconds: 50.0 fps")
        );
        assert_eq!(t.report(2000), None);
    }

    #[test]
    fn record_names_count_up_and_demo_finds_any_protocol() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path().to_path_buf();
        let path = |name, p| record_path(&d, name, p).0;
        assert_eq!(
            record_path(&d, None, &PROTOCOL_V1),
            (d.join("demo0000.dm_1"), "demo0000".to_string())
        );
        std::fs::write(d.join("demo0000.dm_1"), b"").unwrap();
        assert_eq!(path(None, &PROTOCOL_V1), d.join("demo0001.dm_1"));
        // The count is per extension, as the file name is.
        assert_eq!(path(None, &PROTOCOL_V22), d.join("demo0000.dm_3"));
        assert_eq!(
            record_path(&d, Some("x"), &PROTOCOL_V6),
            (d.join("x.dm_2"), "x".to_string())
        );

        std::fs::write(d.join("x.dm_3"), b"").unwrap();
        let found = find(std::slice::from_ref(&d), "x").unwrap();
        assert_eq!((found.0, found.1.transport.version), (d.join("x.dm_3"), 22));
        let found = find(std::slice::from_ref(&d), "demo0000.dm_1").unwrap();
        assert_eq!(found.1.transport.version, 1);
        assert!(find(&[d], "nope").is_none());
    }

    /// A retail capture played as a demo at 60 fps through the clock the
    /// online frame runs: every frame read, each in its turn, and the drawn
    /// time always between two snapshots, never past the newest read.
    /// The retail 400-frame capture as a demo, its gamestate read.
    fn retail_demo(now: std::time::Instant) -> vcod_common::net::NetClient<UdpTransport> {
        let gs = vcod_common::testing::fixture("net/gamestate-delta.bin");
        let snaps = vcod_common::testing::fixture("net/snapshots-delta.bin");
        let first = u32::from_le_bytes(snaps[..4].try_into().unwrap());
        let mut w = demo::DemoWriter::new(Vec::new());
        w.write_message(first - 1, &gs).unwrap();
        let mut data = w.finish().unwrap();
        data.truncate(data.len() - 8);
        data.extend_from_slice(&snaps);
        data.extend_from_slice(&[0xff; 8]);
        let mut net =
            vcod_common::net::NetClient::play_demo(UdpTransport::none(), now, &PROTOCOL_V1, data);
        assert!(net.pump_demo(now, None).contains(&NetEvent::GamestateReady));
        net
    }

    /// Only the playback's first `CA_PRIMED` frame is skipped: after a
    /// gamestate inside the demo the first such frame reads.
    #[test]
    fn a_gamestate_inside_a_demo_skips_no_frame() {
        let now = Instant::now();
        let gs = vcod_common::testing::fixture("net/gamestate-delta.bin");
        let snaps = vcod_common::testing::fixture("net/snapshots-delta.bin");
        let first = u32::from_le_bytes(snaps[..4].try_into().unwrap());
        let len = u32::from_le_bytes(snaps[4..8].try_into().unwrap()) as usize;
        let mut w = demo::DemoWriter::new(Vec::new());
        w.write_message(first - 1, &gs).unwrap();
        w.write_message(first, &snaps[8..8 + len]).unwrap();
        w.write_message(first + 1, &gs).unwrap();
        let data = w.finish().unwrap();
        let mut net = NetClient::play_demo(UdpTransport::none(), now, &PROTOCOL_V1, data);
        assert!(net.pump_demo(now, None).contains(&NetEvent::GamestateReady));
        assert!(net.pump_demo_primed(now).is_empty());
        assert!(net.snapshots().newest().is_none());
        net.pump_demo_primed(now);
        assert!(net.snapshots().newest().is_some());
        assert!(
            net.pump_demo(now, Some(i32::MAX))
                .contains(&NetEvent::GamestateReady)
        );
        // The next record is the trailer, read on the first frame.
        let events = net.pump_demo_primed(now);
        assert!(events.iter().any(|e| matches!(e, NetEvent::DemoEnded(_))));
    }

    const TIMEDEMO: DemoKnobs = DemoKnobs {
        freeze: false,
        timedemo: true,
        nudge: 0,
    };

    /// `timedemo` reads a 20 Hz demo one snapshot a frame, however fast
    /// the frames come: the first map-up frame reads nothing, the next
    /// reads the first snapshot and draws 50 ms past it as frame 1.
    #[test]
    fn timedemo_reads_a_snapshot_a_frame() {
        let now = Instant::now();
        let mut net = retail_demo(now);
        let mut clock = ServerClock::default();
        let mut td = TimeDemo::default();
        // The demo's first snapshot: what `CL_FirstSnapshot` bases on.
        let base = {
            let mut n = retail_demo(now);
            n.pump_demo_primed(now);
            assert!(n.snapshots().newest().is_none());
            n.pump_demo_primed(now);
            n.snapshots().newest().unwrap().server_time
        };
        let mut frames = 0;
        for frame in 0.. {
            let realtime = 1000 + frame;
            let (events, how) = read_frame(&mut net, &mut clock, &mut td, now, realtime, TIMEDEMO);
            if events.iter().any(|e| matches!(e, NetEvent::DemoEnded(_))) {
                assert_eq!(how.timedemo_frames, Some(frames + 1));
                break;
            }
            let Some(newest) = net.snapshots().newest().map(|s| s.server_time) else {
                // `clc.firstDemoFrameSkipped`: nothing read, nothing counted.
                assert_eq!((frame, how.timedemo_frames), (0, None));
                continue;
            };
            let t = clock.update_demo(realtime, newest, 0, how);
            frames += 1;
            assert_eq!(how.timedemo_frames, Some(frames));
            assert_eq!(t, base + 50 * frames, "frame {frame}");
            // Read one snapshot past the frame's time.
            assert_eq!(newest, t + 50, "frame {frame}");
        }
        let span = net.snapshots().newest().unwrap().server_time - base;
        // The frame that finds the trailer is counted but not drawn.
        assert_eq!(frames + 1, span / 50);
        assert!(
            td.report(2000)
                .unwrap()
                .starts_with(&format!("{} frames", frames + 1))
        );
    }

    #[test]
    fn a_demo_reads_one_snapshot_ahead_of_the_clock() {
        let now = Instant::now();
        let mut net = retail_demo(now);
        let mut clock = ServerClock::default();
        let mut td = TimeDemo::default();
        let mut seen = std::collections::BTreeSet::new();
        let mut ended = false;
        for frame in 0..10_000 {
            let realtime = 1000 + frame * 16;
            let knobs = DemoKnobs::default();
            let (events, run) = read_frame(&mut net, &mut clock, &mut td, now, realtime, knobs);
            if events.iter().any(|e| matches!(e, NetEvent::DemoEnded(_))) {
                ended = true;
                break;
            }
            let Some(newest) = net.snapshots().newest() else {
                assert_eq!(frame, 0, "only the first frame is skipped");
                continue;
            };
            seen.insert(newest.message_num);
            let t = clock.update_demo(realtime, newest.server_time, 0, run);
            assert!(
                newest.server_time > t,
                "frame {frame}: drawn past the newest"
            );
            let (a, b) = net.snapshots().two_for_time(t).unwrap();
            assert!(a.server_time <= t && t <= b.server_time, "frame {frame}");
        }
        assert!(ended, "the demo never ended");
        // Every snapshot is newest on some frame but the first: the frame
        // that seeds the clock on it reads on to the next.
        assert_eq!(seen.len(), 399);
        assert_eq!(clock.stats.resets + clock.stats.fast, 0);
    }
}
