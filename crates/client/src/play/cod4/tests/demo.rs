//! Demos: a live session recorded and played back, retail's framed demos
//! and old snapshots shed on playback.

use vcod_cod4::net::conn::Cp;

use super::*;
use crate::play::cod4_menus::Join as MenuJoin;

/// `record` against vcod-server's `war` with four shooting bots,
/// watched as a spectator, then `demo` of the file in process
/// (cod4-demos.md 6): the demo opens on the live table, holds every
/// snapshot the session saw from its first full frame on, state for
/// state, fires the same events off them, and ends on the trailer.
#[test]
fn cod4_live_recorded_demo_plays_back_what_the_session_saw() {
    use vcod_server::cod4::{Cod4Config, Cod4Server};
    let Some(install) = vcod_cod4::testing::cod4_install() else {
        return;
    };
    let _live = live_slot();
    let mut sv = Cod4Server::new(
        Cod4Config {
            map: "mp_crash".into(),
            gametype: "war".into(),
            hostname: "vcod".into(),
            max_clients: 8,
        },
        7,
    );
    sv.set_cvar("scr_game_playerwaittime", "1");
    sv.set_cvar("scr_game_matchstarttime", "1");
    sv.load(Some(&install)).unwrap();
    sv.add_bots(4, true);
    let sock = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    sock.set_nonblocking(true).unwrap();
    let addr = sock.local_addr().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let demos = dir.path().to_path_buf();
    let clock = sim::Clock::new();
    let client = sim::spawn(&clock, move || {
        let join = MenuJoin {
            team: Some("spectator".into()),
            class: None,
        };
        let mut s = Cod4Session::connect(addr, "recorder", Cp::Bare, join).unwrap();
        let t0 = clk_now();
        let mut started = None;
        let mut fired = Vec::new();
        let mut lines = Vec::new();
        while since(t0) < Duration::from_secs(16) {
            s.step(since(t0).as_secs_f64() * 1000.0).unwrap();
            s.take_commands();
            // Past the match start's `d`s (the `*_1mc_boost` slots at
            // 3 s): one landing while the record waits for its full
            // frame never reaches the file (cod4-demos.md 4).
            if started.is_none() && since(t0) > Duration::from_secs(4) {
                // A name that leaves `demos/` opens nothing.
                for bad in ["../escape", "/escape", "C:escape"] {
                    let line = s.start_recording(&demos, Some(bad));
                    if line != "You must be in level to record." {
                        assert_eq!(line, "ERROR: couldn't open.", "{bad}");
                    }
                }
                let line = s.start_recording(&demos, Some("live"));
                if line.starts_with("recording to") {
                    assert_eq!(line, "recording to demos/live.dm_1.");
                    assert_eq!(s.start_recording(&demos, None), "Already recording.");
                    started = Some(s.client().snapshots.len());
                    s.take_fired();
                }
            }
            if started.is_some() {
                fired.extend(s.take_fired());
            }
            lines.extend(s.recording_status().map(|(n, b)| (n.to_string(), b)));
            clk_sleep(Duration::from_millis(16));
        }
        assert_eq!(s.stop_recording(), "Stopped demo.");
        assert_eq!(s.stop_recording(), "Not recording demo.");
        let snaps = s.client().snapshots[started.unwrap()..].to_vec();
        let table: Vec<Vec<u8>> = s
            .gamestate()
            .unwrap()
            .all_configstrings()
            .into_iter()
            .map(<[u8]>::to_vec)
            .collect();
        s.disconnect();
        (snaps, fired, table, lines)
    });
    serve(&mut sv, &sock, &clock, &client, |_, _, _| true);
    let (live, live_fired, live_table, status) = client.join().unwrap();
    let (name, bytes) = status.last().cloned().unwrap();
    assert_eq!(name, "live");
    assert!(!dir.path().join("../escape.dm_1").exists());
    let data = std::fs::read(dir.path().join("live.dm_1")).unwrap();
    // The trailer: a type 0 record of -1 and -1.
    assert_eq!(
        &data[data.len() - 9..],
        &[0, 255, 255, 255, 255, 255, 255, 255, 255]
    );
    assert!(data.len() as u64 >= bytes);

    // `timedemo`: 50 ms of the demo a frame, one snapshot each
    // (vcod-server's 20 a second).
    let mut t = Cod4Session::play_demo(data.clone(), true).unwrap();
    let mut steps = 0;
    while !t.demo_over() && steps < 10_000 {
        t.step(f64::from(steps) * 3.0).unwrap();
        steps += 1;
    }
    let report = t.timedemo_report(steps * 3).unwrap();
    let frames: usize = report.split(' ').next().unwrap().parse().unwrap();
    let snaps = t.client().snapshots.len();
    assert!(frames.abs_diff(snaps) <= 2, "{report}: {snaps} snapshots");
    assert!(
        report.contains(" frames, ") && report.ends_with(" fps"),
        "{report}"
    );

    let mut d = Cod4Session::play_demo(data, false).unwrap();
    assert!(d.is_demo() && !d.is_live());
    assert_eq!(d.map_name().as_deref(), Some("mp_crash"));
    let mut fired = Vec::new();
    let mut now = 0.0;
    while !d.demo_over() && now < 120_000.0 {
        d.step(now).unwrap();
        d.take_commands();
        fired.extend(d.take_fired());
        now += 16.0;
    }
    assert!(d.demo_over(), "the demo never ended");
    let (messages, errors) = d.demo_counts().unwrap();
    assert_eq!(errors, 0);
    let demo = &d.client().snapshots;
    assert!(demo.len() > 100, "{} snapshots", demo.len());
    assert_eq!(
        messages,
        demo.len() + 1,
        "the gamestate, then a snapshot a message"
    );
    // The first is the full frame `demowaiting` asked for.
    assert_eq!(demo[0].delta_num, 0);
    let first = live
        .iter()
        .position(|s| s.server_time == demo[0].server_time)
        .expect("the demo's first frame is one the session saw");
    assert_eq!(live.len() - first, demo.len());
    for (l, m) in live[first..].iter().zip(demo) {
        assert_eq!(l.server_time, m.server_time);
        assert_eq!(l.ps, m.ps, "{}", l.server_time);
        assert_eq!(l.entities, m.entities, "{}", l.server_time);
        assert_eq!(l.clients, m.clients, "{}", l.server_time);
    }
    let table: Vec<Vec<u8>> = d
        .gamestate()
        .unwrap()
        .all_configstrings()
        .into_iter()
        .map(<[u8]>::to_vec)
        .collect();
    assert_eq!(table, live_table);
    // Events off the snapshots past the first, which both walked from
    // the same state.
    let key = |f: &Fired| (f.name, f.entity, f.server_time, f.parm);
    let after = |v: &[Fired]| -> Vec<_> {
        v.iter()
            .filter(|f| f.server_time > demo[0].server_time)
            .map(key)
            .collect()
    };
    let (a, b) = (after(&live_fired), after(&fired));
    eprintln!(
        "demo: {bytes} bytes, {messages} messages, {} snapshots, {} events",
        demo.len(),
        a.len()
    );
    assert!(!a.is_empty(), "no event in the session");
    assert_eq!(a, b);
}

/// A demo file as `iw3mp.exe` writes one (cod4-demos.md 2, 3, 8),
/// byte by byte off the binary's writes and apart from
/// [`vcod_cod4::net::demo::Recording`].
#[derive(Default)]
struct RetailDemo(Vec<u8>);

impl RetailDemo {
    /// `CL_WriteDemoMessage` (0x468850): byte 0, the sequence, the
    /// length, the bytes.
    pub(super) fn message(&mut self, sequence: i32, msg: &[u8]) {
        self.0.push(0);
        self.0.extend(sequence.to_le_bytes());
        self.0.extend((msg.len() as i32).to_le_bytes());
        self.0.extend(msg);
    }

    /// 0x468750: byte 1, the index, then the row's +0x04, +0x10,
    /// +0x20, +0x1c, +0x00 and +0x24.
    fn row(&mut self, index: usize, a: &vcod_cod4::net::demo::Archive) {
        self.0.push(1);
        self.0.extend((index as i32).to_le_bytes());
        for v in a.origin.iter().chain(&a.velocity) {
            self.0.extend(v.to_le_bytes());
        }
        self.0.extend(a.movement_dir.to_le_bytes());
        self.0.extend(a.bob_cycle.to_le_bytes());
        self.0.extend(a.server_time.to_le_bytes());
        for v in &a.view_angles {
            self.0.extend(v.to_le_bytes());
        }
    }

    /// `CL_StopRecord_f` (0x4688c0): byte 0 and two -1 longs.
    fn stop(&mut self) {
        self.0.push(0);
        self.0.extend((-1i32).to_le_bytes());
        self.0.extend((-1i32).to_le_bytes());
    }
}

/// A [`std::io::Write`] whose bytes the test reads back.
#[derive(Clone, Default)]
struct Sink(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

impl std::io::Write for Sink {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// A retail capture's server messages written as a demo twice, by
/// [`RetailDemo`] and by vcod's `Recording`. The record starts as the
/// gamestate lands: retail's `record` waits for its first snapshot,
/// but the capture's client never asked for the full frame a later
/// start waits for. A row from each snapshot's playerstate goes in
/// after each message, as `CL_WritePacket` adds one between them.
/// Returns both files and the table the capture's client ends on, its
/// `d` commands applied as a live client applies them.
fn capture_demo(name: &str) -> (Vec<u8>, Vec<u8>, Vec<Vec<u8>>) {
    use vcod_cod4::net::capture::{self, Item, Session};
    use vcod_cod4::net::demo::{ARCHIVE_ROWS, Archive, ArchiveRing, Recording};
    let text = std::fs::read_to_string(format!("{FIXTURES}/{name}")).unwrap();
    let mut session = Session::new();
    let mut retail = RetailDemo::default();
    let sink = Sink::default();
    let mut rec: Option<Recording> = None;
    let mut ring = ArchiveRing::default();
    // `clc.demowaiting` and the last row written (+0x4014c).
    let mut waiting = true;
    let mut written = 0;
    let mut applied = 0;
    for l in capture::parse(&text).unwrap() {
        let Some(Item::Server { msg, parsed }) = session.feed(&l).unwrap() else {
            continue;
        };
        vcod_cod4::net::conn::apply_configstrings(&mut session.client, &mut applied, &parsed);
        let c = &session.client;
        let Some(r) = rec.as_mut() else {
            let Some(gs) = &c.gamestate else { continue };
            let reliable = session.client_commands.keys().max().copied();
            let opening = vcod_cod4::net::demo::gamestate_message(
                &c.huff,
                reliable.unwrap_or(0),
                gs.server_command_sequence,
                gs,
            );
            retail.message(msg.sequence as i32, &opening);
            for (i, a) in ring.rows.iter().enumerate() {
                retail.row(i, a);
            }
            written = ring.index;
            rec = Some(
                Recording::start(
                    Box::new(sink.clone()),
                    "capture",
                    msg.sequence as i32,
                    &opening,
                    &ring,
                )
                .unwrap(),
            );
            continue;
        };
        // Cleared inside the snapshot parse, ahead of the write
        // (0x4737d5).
        let full = parsed
            .ops
            .iter()
            .any(|o| matches!(o, Op::Snapshot(i) if c.snapshots[*i].delta_num == 0));
        if full {
            waiting = false;
            r.waiting = false;
        }
        let mut bytes = msg.reliable_ack.to_le_bytes().to_vec();
        bytes.extend(&msg.block);
        if !waiting {
            while written != ring.index {
                retail.row(written, &ring.rows[written]);
                written = (written + 1) % ARCHIVE_ROWS;
            }
            retail.message(msg.sequence as i32, &bytes);
        }
        r.write(msg.sequence as i32, &bytes, &ring).unwrap();
        if let Some(s) = c.snapshots.last() {
            let ps = pm::PlayerState::from_wire(&s.ps);
            ring.add(Archive {
                server_time: s.server_time,
                origin: ps.origin.to_array(),
                velocity: ps.velocity.to_array(),
                bob_cycle: ps.bob_cycle,
                movement_dir: ps.movement_dir,
                view_angles: ps.view_angles,
            });
        }
    }
    retail.stop();
    rec.unwrap().finish().unwrap();
    let vcod = sink.0.lock().unwrap().clone();
    let table = session
        .client
        .gamestate
        .as_ref()
        .unwrap()
        .all_configstrings();
    let table = table.into_iter().map(<[u8]>::to_vec).collect();
    (retail.0, vcod, table)
}

/// Retail to vcod and back (cod4-demos.md 8): a retail spectator
/// capture written in retail's framing plays in vcod's `demo`, every
/// record read, every message parsed, the capture's snapshots state for
/// state from the first on, its events and its closing table, `d`s
/// applied; and
/// vcod's recorder, fed the same messages and rows, writes the same
/// file byte for byte.
#[test]
fn cod4_retail_framed_demo_plays_and_vcod_writes_it_alike() {
    let name = "play-1.7-spec.txt";
    let (retail, vcod, capture_table) = capture_demo(name);
    assert_eq!(retail.len(), vcod.len(), "file length");
    if let Some(at) = (0..retail.len()).find(|&i| retail[i] != vcod[i]) {
        panic!("vcod's file differs from retail's framing at byte {at}");
    }

    let text = std::fs::read_to_string(format!("{FIXTURES}/{name}")).unwrap();
    let mut live = Cod4Session::open_capture(&text).unwrap();
    let mut live_fired = Vec::new();
    let mut ms = 0.0;
    while !live.finished() {
        ms += 1000.0 / 60.0;
        live.step(ms).unwrap();
        live_fired.extend(live.take_fired());
    }

    let mut d = Cod4Session::play_demo(retail, false).unwrap();
    assert_eq!(d.map_name(), live.map_name());
    let mut fired = Vec::new();
    let mut now = 0.0;
    while !d.demo_over() && now < 600_000.0 {
        d.step(now).unwrap();
        d.take_commands();
        fired.extend(d.take_fired());
        now += 16.0;
    }
    assert!(d.demo_over(), "the demo never ended");
    let (messages, errors) = d.demo_counts().unwrap();
    assert_eq!(errors, 0);
    let (demo, cap) = (&d.client().snapshots, &live.client().snapshots);
    assert_eq!(demo.len(), cap.len(), "a snapshot a message");
    assert_eq!(messages, demo.len() + 1, "the gamestate, then the rest");
    assert_eq!(demo[0].delta_num, 0);
    for (l, m) in cap.iter().zip(demo) {
        assert_eq!(l.server_time, m.server_time);
        assert_eq!(l.ps, m.ps, "{}", l.server_time);
        assert_eq!(l.entities, m.entities, "{}", l.server_time);
        assert_eq!(l.clients, m.clients, "{}", l.server_time);
    }
    let table = d.gamestate().unwrap().all_configstrings();
    assert_eq!(table.len(), capture_table.len());
    for (i, (m, l)) in table.iter().zip(&capture_table).enumerate() {
        assert_eq!(
            String::from_utf8_lossy(m),
            String::from_utf8_lossy(l),
            "slot {i}"
        );
    }
    let key = |f: &Fired| (f.name, f.entity, f.server_time, f.parm);
    let after = |v: &[Fired]| -> Vec<_> {
        v.iter()
            .filter(|f| f.server_time > demo[0].server_time)
            .map(key)
            .collect()
    };
    let (a, b) = (after(&live_fired), after(&fired));
    eprintln!(
        "{name}: {} bytes, {messages} messages, {} snapshots, {} events",
        vcod.len(),
        demo.len(),
        a.len()
    );
    assert!(!a.is_empty(), "no event in the capture");
    assert_eq!(a, b);
}

/// A demo sheds its old snapshots as a live session does and plays the
/// same events to its end.
#[test]
fn cod4_demo_sheds_old_snapshots_and_plays_on() {
    let (retail, _, _) = capture_demo("play-1.7-spec.txt");
    let play = |keep: Option<usize>| {
        let mut d = Cod4Session::play_demo(retail.clone(), false).unwrap();
        if let Some(k) = keep {
            d.keep_snapshots = k;
        }
        let (mut fired, mut most, mut now) = (Vec::new(), 0, 0.0);
        while !d.demo_over() && now < 600_000.0 {
            d.step(now).unwrap();
            d.take_commands();
            fired.extend(
                d.take_fired()
                    .iter()
                    .map(|f| (f.name, f.entity, f.server_time)),
            );
            most = most.max(d.client().snapshots.len());
            now += 16.0;
        }
        assert!(d.demo_over());
        (
            fired,
            most,
            d.client().shed_count,
            d.client().total_snapshots(),
        )
    };
    let (all, _, none_shed, total) = play(None);
    let (shed, most, shed_count, total_shed) = play(Some(40));
    assert_eq!(none_shed, 0);
    assert!(total > 2 * 40, "{total} snapshots");
    assert!(shed_count > 0 && most < 2 * 40, "held {most}");
    assert_eq!(total_shed, total);
    assert_eq!(shed, all);
}

/// A player records himself running back and forth on mp_crash and
/// turning, predicted as the app plays, and the demo plays back at
/// 144 frames a second: his eye moves and his view turns nearly every
/// frame, where they stood still between snapshots (cod4-demos.md 5).
#[test]
fn cod4_live_recorded_run_plays_back_smoothly() {
    use crate::play::input::Action;
    use vcod_server::cod4::{Cod4Config, Cod4Server};
    let Some(install) = vcod_cod4::testing::cod4_install() else {
        return;
    };
    let Some(world) = crash_world() else {
        return;
    };
    let _live = live_slot();
    let mut sv = Cod4Server::new(
        Cod4Config {
            map: "mp_crash".into(),
            gametype: "war".into(),
            hostname: "vcod".into(),
            max_clients: 4,
        },
        7,
    );
    sv.set_cvar("scr_game_playerwaittime", "1");
    sv.set_cvar("scr_game_matchstarttime", "1");
    sv.load(Some(&install)).unwrap();
    let sock = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    sock.set_nonblocking(true).unwrap();
    let addr = sock.local_addr().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let demos = dir.path().to_path_buf();
    let clock = sim::Clock::new();
    let client = sim::spawn(&clock, move || {
        let join = MenuJoin {
            team: Some("allies".into()),
            class: Some("assault_mp,0".into()),
        };
        let mut s = Cod4Session::connect(addr, "runner", Cp::Bare, join).unwrap();
        s.set_world(world);
        let t0 = clk_now();
        let mut started = None;
        while since(t0) < Duration::from_secs(20) {
            let now = since(t0).as_secs_f64() * 1000.0;
            s.step(now).unwrap();
            s.take_commands();
            // The app draws a scene each frame, which predicts.
            s.scene();
            s.take_fired();
            if s.spawn().is_some() && started.is_none() {
                started = Some(now);
            }
            if let Some(at) = started {
                // A steady turn, 30 degrees a second.
                s.view[1] = (s.view[1] + 0.21).rem_euclid(360.0);
                // Back and forth, so a wall stops no run for long.
                let back = ((now - at) / 1500.0) as i32 % 2 == 1;
                s.input.key(Action::Forward, !back && now - at > 500.0);
                s.input.key(Action::Back, back);
                if now - at > 1000.0 && s.recording_status().is_none() {
                    s.start_recording(&demos, Some("run"));
                }
                if now - at > 7000.0 {
                    break;
                }
            }
            clk_sleep(Duration::from_millis(7));
        }
        s.stop_recording();
        s.disconnect();
    });
    serve(&mut sv, &sock, &clock, &client, |_, _, _| true);
    client.join().unwrap();
    let data = std::fs::read(dir.path().join("run.dm_1")).unwrap();
    let mut d = Cod4Session::play_demo(data, false).unwrap();
    let mut eyes = Vec::new();
    let mut now = 0.0;
    while !d.demo_over() && now < 60_000.0 {
        d.step(now).unwrap();
        d.take_commands();
        d.take_fired();
        if let Some(sc) = d.scene() {
            eyes.push((sc.eye, sc.angles[1]));
        }
        now += 1000.0 / 144.0;
    }
    // Per frame: how far the eye went and the view turned.
    let steps: Vec<(f32, f32)> = eyes
        .windows(2)
        .map(|w| {
            let turn = (w[1].1 - w[0].1 + 540.0).rem_euclid(360.0) - 180.0;
            ((w[1].0 - w[0].0).truncate().length(), turn.abs())
        })
        .collect();
    // Frames past the recording's first second, where both run.
    let steps = &steps[steps.len().min(144)..];
    let still = |k: fn(&(f32, f32)) -> f32| {
        let mut v: Vec<f32> = steps.iter().map(k).collect();
        v.sort_by(f32::total_cmp);
        let median = v[v.len() / 2];
        let still = v.iter().filter(|&&x| x < median * 0.25).count();
        (median, still)
    };
    let (run, run_still) = still(|s| s.0);
    let (turn, turn_still) = still(|s| s.1);
    eprintln!(
        "demo run: {} frames, eye median {run:.3} still {run_still}, turn median {turn:.3} still {turn_still}",
        steps.len()
    );
    assert!(steps.len() > 500, "{} frames", steps.len());
    assert!(turn > 0.1, "the view never turned: {turn}");
    // A frame or two a second stand still at a wall or where the run
    // turns back; a stepped view stood still six frames in seven.
    assert!(turn_still * 20 < steps.len(), "{turn_still} still turns");
    assert!(run_still * 5 < steps.len(), "{run_still} still eyes");
}
