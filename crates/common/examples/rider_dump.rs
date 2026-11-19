//! Every seated player entity (`eType` 1 with `otherEntityNum` set and a
//! seat in `surfType`) in a UO capture, with its trajectory fields.
use vcod_common::net::protocol::{Cod1Wire, PROTOCOL_V22};
fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let p = Cod1Wire {
        event_to_v1: |e| e,
        eflags_to_v1: |f| f,
        ..PROTOCOL_V22
    };
    let mut n = 0;
    for frame in vcod_common::testing::fixture_frames(&p, &args[0], &args[1]) {
        let snap = &frame.snapshot;
        for (num, e) in &snap.entities {
            if *num >= 64 || e.field_i32(&p, "surfType") & 7 == 0 {
                continue;
            }
            let f = |k: &str| e.field_i32(&p, k);
            let g = |k: &str| e.field_f32(&p, k);
            if n < 12 {
                println!(
                    "t {} ent {num} eType {} surf {:#x} other {} pos.trType {} trTime {} trDur {} base ({}, {}, {}) delta ({}, {}, {}) apos.trType {} abase ({}, {}, {})",
                    snap.server_time,
                    f("eType"),
                    f("surfType"),
                    f("otherEntityNum"),
                    f("pos.trType"),
                    f("pos.trTime"),
                    f("pos.trDuration"),
                    g("pos.trBase[0]"),
                    g("pos.trBase[1]"),
                    g("pos.trBase[2]"),
                    g("pos.trDelta[0]"),
                    g("pos.trDelta[1]"),
                    g("pos.trDelta[2]"),
                    f("apos.trType"),
                    g("apos.trBase[0]"),
                    g("apos.trBase[1]"),
                    g("apos.trBase[2]")
                );
            }
            n += 1;
        }
    }
    println!("{n} seated entity samples");
}
