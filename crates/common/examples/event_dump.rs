//! Every event and server command in a committed capture, in the wire's own
//! numbering: one line per event with its server time, carrier, `eType`,
//! parm, weapon, `es.scale`, `surfType`, `eFlags` and `angles2` as sent.
//!
//! `cargo run -p vcod-common --example event_dump -- <protocol> <gamestate
//! fixture> <snapshot fixture>`, e.g. `22 net/gamestate-uo-quake.bin
//! net/snapshots-uo-quake.bin`.

use vcod_common::net::events::EventTracker;
use vcod_common::net::protocol::{Cod1Wire, PROTOCOL_V1, PROTOCOL_V6, PROTOCOL_V22};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [version, gs_name, snap_name] = &args[..] else {
        eprintln!("usage: event_dump <1|6|22> <gamestate fixture> <snapshot fixture>");
        std::process::exit(2);
    };
    let base = match version.as_str() {
        "6" => PROTOCOL_V6,
        "22" => PROTOCOL_V22,
        _ => PROTOCOL_V1,
    };
    // The wire's ids and flags, not vcod's 1.1 view of them.
    let p = Cod1Wire {
        event_to_v1: |e| e,
        eflags_to_v1: |f| f,
        ..base
    };
    let mut tracker = EventTracker::new();
    for frame in vcod_common::testing::fixture_frames(&p, gs_name, snap_name) {
        let snap = &frame.snapshot;
        for cmd in &frame.commands {
            println!("t {} cmd {cmd:?}", snap.server_time);
        }
        for ev in tracker.drain(snap, &p) {
            let carrier = match ev.entity_num {
                u32::MAX => "ps".to_string(),
                n => n.to_string(),
            };
            let angles2 = snap
                .entities
                .get(&ev.entity_num)
                .map(|e| [0, 1, 2].map(|i| e.field_f32(&p, &format!("angles2[{i}]"))))
                .filter(|a| a.iter().any(|v| *v != 0.0));
            println!(
                "t {} ev {} on {carrier} eType {} parm {} weapon {} scale {} surf {} eFlags {:#x} \
                 pos {:?} angles2 {angles2:?}",
                snap.server_time,
                ev.event,
                ev.e_type,
                ev.parm,
                ev.weapon,
                ev.scale,
                ev.surf_type,
                ev.eflags,
                ev.pos.map(|v| v.round() as i32),
            );
        }
    }
}
