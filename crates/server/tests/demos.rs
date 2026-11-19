//! `record` and `demo` end to end against vcod-server in process: a joined
//! client records mid-level, across a `map_restart`, with two bots
//! shooting around it; the file holds the gamestate the client built and
//! then every message it parsed, byte for byte, and played back it parses
//! to the same frames, configstrings and server commands
//! (docs/research/cod11-demos.md). Needs `COD_DIR` (UO's install runs UO,
//! else 1.1); without the paks it returns early.

use crate::common;

use common::{ClientEnd, Queues};
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::io::Write;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use vcod_common::net::demo::{DemoEnd, DemoReader};
use vcod_common::net::protocol::{Cod1Wire, PROTOCOL_V1, PROTOCOL_V22};
use vcod_common::net::{NetClient, NetEvent, NetState};

/// A `Write` the test reads back after the client is done with it.
#[derive(Clone, Default)]
struct Shared(Arc<Mutex<Vec<u8>>>);

impl Write for Shared {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn setup() -> Option<(vcod_common::pk3::Pk3Fs, &'static Cod1Wire)> {
    match vcod_common::testing::uo_fs() {
        Some(fs) => Some((fs, &PROTOCOL_V22)),
        None => Some((vcod_common::testing::game_fs()?, &PROTOCOL_V1)),
    }
}

/// What a client saw: each snapshot by message number, the server
/// commands in order, the configstrings at the end.
#[derive(Default)]
struct Seen {
    frames: BTreeMap<u32, String>,
    commands: Vec<String>,
}

impl Seen {
    fn take(&mut self, cl: &NetClient<impl vcod_common::net::Transport>, events: &[NetEvent]) {
        for e in events {
            if let NetEvent::ServerCommand(t) = e {
                self.commands.push(t.join(" "));
            }
        }
        if let Some(s) = cl.snapshots().newest() {
            self.frames
                .entry(s.message_num)
                .or_insert_with(|| format!("{s:?}"));
        }
    }
}

/// `record` mid-level, no restart: the client's no-delta moves get a full
/// frame back at once, and the file opens on it (cod11-demos.md 4). Needs
/// no paks.
#[test]
fn record_mid_level_gets_a_full_frame_without_a_restart() {
    let mut now = Instant::now();
    let mut sv = vcod_server::Server::new(common::cfg("mp_carentan", "dm"), now);
    let q = Rc::new(RefCell::new(Queues::default()));
    let mut cl = common::connect(&mut sv, &q, &mut now);
    for _ in 0..8 {
        now += Duration::from_millis(50);
        cl.send_frame(&vcod_common::net::msg::NULL_USERCMD);
        common::step(&mut sv, &q, &mut cl, now);
    }
    assert!(cl.snapshots().newest().unwrap().delta_num > 0);
    let sink = Shared::default();
    cl.start_recording(Box::new(sink.clone()), "mid")
        .expect("record in a level");
    let mut full = None;
    for _ in 0..3 {
        now += Duration::from_millis(50);
        cl.send_frame(&vcod_common::net::msg::NULL_USERCMD);
        common::step(&mut sv, &q, &mut cl, now);
        if let Some(s) = cl.snapshots().newest()
            && s.message_num == cl.incoming_sequence()
            && s.delta_num <= 0
        {
            full.get_or_insert(s.message_num);
        }
    }
    let full = full.expect("no full frame after record");
    assert!(cl.stop_recording().unwrap());
    let mut r = DemoReader::new(sink.0.lock().unwrap().clone(), &PROTOCOL_V1);
    r.next_message().unwrap();
    assert_eq!(r.next_message().unwrap().0, full);
}

#[test]
fn a_recorded_demo_plays_back_the_frames_the_client_parsed() {
    let Some((fs, p)) = setup() else {
        eprintln!("COD_DIR unset: skipping");
        return;
    };
    let fs = Rc::new(fs);
    let mut cfg = common::cfg("mp_carentan", "dm");
    cfg.bots = 2;
    cfg.bots_shoot = true;
    let (mut sv, mut now) = common::Boot {
        proto: Some(p),
        ..common::Boot::on(cfg)
    }
    .start(fs.clone());
    let q = Rc::new(RefCell::new(Queues::default()));
    let (mut cl, _) = common::join(&mut sv, &q, &mut now, "axis", "mp44_mp");
    cl.enable_capture();

    let sink = Shared::default();
    cl.start_recording(Box::new(sink.clone()), "ab")
        .expect("record in a level");
    assert!(
        cl.start_recording(Box::new(Shared::default()), "x")
            .is_err()
    );

    // Walk and fire; a restart halfway sends `map_restart`'s commands.
    let mut live = Seen::default();
    let mut first_recorded = None;
    for i in 0..240 {
        now += Duration::from_millis(50);
        if i == 120 {
            sv.push_console("map_restart");
        }
        let mut cmd = common::holding(&cl);
        cmd.forward = 127;
        cmd.buttons = if i % 10 < 3 { 1 } else { 0 };
        cl.send_frame(&cmd);
        let events = common::step(&mut sv, &q, &mut cl, now);
        assert!(
            !events.iter().any(|e| matches!(e, NetEvent::Dropped(_))),
            "{events:?}"
        );
        // The recording opens on the first frame that deltas from nothing.
        if first_recorded.is_none()
            && let Some(s) = cl.snapshots().newest()
            && s.delta_num <= 0
        {
            first_recorded = Some(s.message_num);
        }
        if first_recorded.is_some() {
            live.take(&cl, &events);
        }
    }
    let first_recorded = first_recorded.expect("no full frame after record");
    // The `RECORDING` line's size is the file's, trailer aside.
    assert_eq!(
        cl.recording_status(),
        Some(("ab", sink.0.lock().unwrap().len() as u64))
    );
    assert!(cl.stop_recording().unwrap());
    assert!(!cl.stop_recording().unwrap());
    let bytes = sink.0.lock().unwrap().clone();
    assert_eq!(&bytes[bytes.len() - 8..], &[0xff; 8], "no trailer");

    // Byte for byte: past the opening gamestate, the records are the
    // snapshot messages the client parsed, under their sequences.
    let capture = cl.take_capture().unwrap().triples;
    let mut want = BTreeMap::new();
    let mut r = DemoReader::new(capture, p);
    while let Ok((seq, msg)) = r.next_message() {
        if seq >= first_recorded {
            want.insert(seq, msg);
        }
    }
    let mut r = DemoReader::new(bytes.clone(), p);
    let (gs_seq, _) = r.next_message().unwrap();
    assert!(gs_seq < first_recorded);
    let mut got = BTreeMap::new();
    let mut first = None;
    loop {
        match r.next_message() {
            Ok((seq, msg)) => {
                first.get_or_insert(seq);
                got.insert(seq, msg);
            }
            Err(e) => {
                assert_eq!(e, DemoEnd::Completed);
                break;
            }
        }
    }
    assert_eq!(first, Some(first_recorded), "the wait for a full frame");
    // Every message the server sent in the window carried a snapshot.
    assert_eq!(got.len(), want.len());
    assert!(got == want, "a record differs from the message parsed live");

    // Played back: the same frames, commands and configstrings.
    let mut play = NetClient::play_demo(ClientEnd(Rc::default()), now, p, bytes);
    let events = play.pump_demo(now, None);
    assert!(events.contains(&NetEvent::GamestateReady));
    let mut seen = Seen::default();
    loop {
        let until = play
            .snapshots()
            .newest()
            .map_or(i32::MIN, |s| s.server_time);
        let events = play.pump_demo(now, Some(until));
        seen.take(&play, &events);
        if events.iter().any(|e| matches!(e, NetEvent::DemoEnded(_))) {
            break;
        }
    }
    assert_eq!(play.state(), NetState::Disconnected);
    // 12 s of a 10 Hz snapshot stream.
    assert!(live.frames.len() > 100, "{} frames", live.frames.len());
    assert_eq!(seen.frames.len(), live.frames.len());
    for (num, frame) in &live.frames {
        assert_eq!(seen.frames.get(num), Some(frame), "frame {num}");
    }
    assert!(
        live.commands.iter().any(|c| c == "n" || c == "B"),
        "the restart's command never came: {:?}",
        live.commands
    );
    assert_eq!(seen.commands, live.commands);
    assert_eq!(play.configstrings(), cl.configstrings());
}
