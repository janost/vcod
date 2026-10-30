//! `record` over `fixtures/mp_toujane-spectate.bin`'s retail messages (the
//! session `replay.rs` describes), then the file played back through a
//! fresh client: the opening gamestate is the server's own ops, and every
//! snapshot comes back as the live client parsed it.

use std::io::Write;
use std::sync::{Arc, Mutex};

use vcod_cod2::net::capture::{Capture, replay};
use vcod_cod2::net::{Client, NetView, Parser, demo};
use vcod_common::net::demo::DemoEnd;
use vcod_common::net::huffman::Huffman;

const FIXTURE: &[u8] = include_bytes!("fixtures/mp_toujane-spectate.bin");

#[derive(Clone, Default)]
struct Shared(Arc<Mutex<Vec<u8>>>);

impl Write for Shared {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[test]
fn a_recorded_retail_session_plays_back_snapshot_for_snapshot() {
    let rep = replay(&Capture::decode(FIXTURE).unwrap()).unwrap();
    let huff = Huffman::new();
    let file = Shared::default();
    let mut live = Client::new(0, 0);
    let mut view = NetView::default();
    let mut gamestate_msg = None;
    let mut live_snaps = Vec::new();
    for (seq, msg) in &rep.messages {
        // What the netchan's frame sets on a live client.
        live.netchan.incoming_sequence = *seq as u32;
        let p = live.server_message(*seq, msg).unwrap();
        view.update(&live.parser, &p);
        if p.gamestate {
            gamestate_msg = Some(msg.clone());
            live.start_recording(Box::new(file.clone()), "t", view.configstrings.all())
                .unwrap();
            assert!(!live.can_delta(), "a waiting record asks for a full frame");
            continue;
        }
        if let Some(n) = p.snapshot {
            live_snaps.push(live.parser.frame(n).unwrap().clone());
        }
    }
    assert!(live_snaps.len() > 10);
    let written = live.stop_recording().unwrap().unwrap();
    let bytes = file.0.lock().unwrap().clone();
    assert_eq!(written as usize, bytes.len());
    assert_eq!(&bytes[bytes.len() - 8..], &[0xff; 8]);

    // The first record: the gamestate message's sequence, its length, the
    // client's reliableSequence (0) and the server's gamestate ops.
    let gs_seq = rep
        .messages
        .iter()
        .find(|(_, m)| Some(m) == gamestate_msg.as_ref());
    let gs_seq = gs_seq.unwrap().0;
    assert_eq!(i32::from_le_bytes(bytes[0..4].try_into().unwrap()), gs_seq);
    let len = u32::from_le_bytes(bytes[4..8].try_into().unwrap()) as usize;
    let first = &bytes[8..8 + len];
    assert_eq!(&first[..4], &[0; 4]);
    let consumed = |block: &[u8]| {
        let plain = huff.decompress_block(block);
        let n = Parser::new().parse(gs_seq, &plain).unwrap().consumed;
        plain[..n].to_vec()
    };
    let ours = consumed(&first[4..]);
    let server = consumed(&gamestate_msg.unwrap()[4..]);
    assert_eq!(ours[0], 1, "svc_gamestate first");
    assert!(
        server.ends_with(&ours),
        "the recorded gamestate is the server's past its leading commands"
    );

    let mut r = demo::reader(bytes);
    let mut back = Client::new(0, 0);
    let mut snaps = Vec::new();
    let end = loop {
        match r.next_message() {
            Ok((seq, msg)) => {
                if let Some(p) = back.demo_message(seq as i32, &msg).unwrap()
                    && let Some(n) = p.snapshot
                {
                    snaps.push(back.parser.frame(n).unwrap().clone());
                }
            }
            Err(e) => break e,
        }
    };
    assert_eq!(end, DemoEnd::Completed);
    assert_eq!(snaps, live_snaps);
    assert_eq!(
        back.parser.gamestate.as_ref().unwrap().configstrings,
        live.parser.gamestate.as_ref().unwrap().configstrings
    );
}

/// `fixtures/demos/mp_toujane-retail.dm_1`: retail `CoD2MP_s.exe` 1.3 under
/// Wine, `set activeAction "record wine1"` then `devmap mp_toujane`, killed
/// after about a minute without `stoprecord`, so the file ends where its
/// buffer was last flushed (docs/research/cod2-demos.md 6).
#[test]
fn a_retail_demo_parses_every_record() {
    let data = include_bytes!("fixtures/demos/mp_toujane-retail.dm_1").to_vec();
    let mut r = demo::reader(data);
    let mut c = Client::new(0, 0);
    let (mut records, mut snaps, mut deltas) = (0, 0, 0);
    let mut first = None;
    let mut times = Vec::new();
    let end = loop {
        match r.next_message() {
            Ok((seq, msg)) => {
                records += 1;
                if first.is_none() {
                    // The opening gamestate: reliableSequence 2, then ops.
                    assert_eq!(&msg[..4], &[2, 0, 0, 0]);
                    first = Some(seq);
                }
                let p = c.demo_message(seq as i32, &msg).unwrap().unwrap();
                if records == 1 {
                    assert!(p.gamestate);
                    // vcod's `record` writes retail's opening byte for byte
                    // from the same table.
                    let gs = c.parser.gamestate.as_ref().unwrap();
                    let cs = vcod_cod2::net::ConfigStrings::from_gamestate(gs);
                    let ours = demo::gamestate_message(&Huffman::new(), 2, gs, cs.all());
                    assert_eq!(ours, msg, "the opening record re-encodes");
                }
                if let Some(n) = p.snapshot {
                    let f = c.parser.frame(n).unwrap();
                    snaps += 1;
                    deltas += usize::from(f.delta_num >= 0);
                    times.push(f.server_time);
                }
            }
            Err(e) => break e,
        }
    };
    assert_eq!(first, Some(3));
    assert_eq!(end, DemoEnd::Truncated);
    let gs = c.parser.gamestate.as_ref().unwrap();
    assert_eq!(gs.configstrings.len(), 282);
    assert_eq!(gs.baselines.len(), 38);
    assert_eq!(records, 858);
    assert_eq!(snaps, 857);
    // The first record after the gamestate is the full frame the record
    // waited for.
    assert_eq!(deltas, snaps - 1);
    assert!(times.windows(2).all(|w| w[0] < w[1]));
}
