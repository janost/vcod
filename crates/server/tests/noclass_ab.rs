//! A client between its connect and its first spawn, ours against retail's.
//! `client-probes/probe_noclass` logs the client's fields at the connect
//! callback, 3 s later and straight after a `spawn`, then what
//! `.headiconteam` reads after each of eight writes; the fixtures are its
//! `games_mp.log` and the probe client's snapshot origins on 1.1d, 1.5b and
//! UO 1.51b (cod11-gametypes-re-bel.md 8.3).
//!
//! The tests need `COD_DIR` at the install each names and return early
//! without.

use crate::common;

use common::{Queues, step};
use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;
use vcod_common::net::NetClient;
use vcod_common::net::msg::NULL_USERCMD;

const SCRIPT: &str =
    include_str!("../../gsc/tests/fixtures/semantics/client-probes/probe_noclass.gsc");

fn probe_lines<'a>(lines: impl Iterator<Item = &'a str>) -> Vec<String> {
    lines
        .map(str::trim_end)
        .filter(|l| l.starts_with("PROBE "))
        .map(str::to_string)
        .collect()
}

/// `developer` for the run. Each fixture ends on `.headiconteam`'s first
/// illegal string: retail 1.1d and UO died there (`run_probe.sh`'s
/// `developer 1`), while 1.5b ran on to the next line and dropped the
/// client after (gametypes doc 7.3), as ours runs on below 1.
fn check(fs: vcod_common::pk3::Pk3Fs, fixture: &str, developer: &str) {
    let (mut sv, mut now) = common::Boot {
        script: Some(SCRIPT),
        cvars: &[("developer", developer)],
        ..common::Boot::new("mp_carentan", "probe_noclass")
    }
    .start(fs);
    let q = Rc::new(RefCell::new(Queues::default()));
    let mut cl = NetClient::start_with_qport(common::ClientEnd(q.clone()), now, 0x2001);
    let frame = Duration::from_millis(common::FRAME_MS as u64);
    let spawned = |log: &[String]| log.iter().any(|l| l.starts_with("PROBE spawned"));
    // Retail's probe client reads `ps.origin` 0 on every snapshot until the
    // spawn (the fixture's SNAP lines).
    let mut before_spawn = 0;
    for _ in 0..200 {
        now += frame;
        cl.send_frame(&NULL_USERCMD);
        step(&mut sv, &q, &mut cl, now);
        if spawned(sv.script_log()) {
            break;
        }
        if let Some(s) = cl.snapshots().newest() {
            let f = |n: &str| f32::from_bits(s.ps.field_i32(cl.protocol(), n) as u32);
            let o = [f("origin[0]"), f("origin[1]"), f("origin[2]")];
            assert_eq!(
                o, [0.0; 3],
                "snapshot at {} before the spawn",
                s.server_time
            );
            before_spawn += 1;
        }
    }
    assert!(
        before_spawn > 20,
        "{before_spawn} snapshots before the spawn"
    );
    let retail = probe_lines(fixture.lines());
    let ours = probe_lines(sv.script_log().iter().map(String::as_str));
    assert_eq!(ours, retail);
}

#[test]
fn a_connected_client_reads_as_retail_11() {
    let Some(fs) = vcod_common::testing::game_fs_11() else {
        return;
    };
    check(fs, include_str!("fixtures/noclass/mp_carentan.txt"), "1");
}

#[test]
fn a_connected_client_reads_as_retail_15() {
    let Some(fs) = vcod_common::testing::game_fs_15() else {
        return;
    };
    check(
        fs,
        include_str!("fixtures/noclass/mp_carentan-1.5.txt"),
        "0",
    );
}

#[test]
fn a_connected_client_reads_as_retail_uo() {
    let Some(fs) = vcod_common::testing::uo_fs() else {
        return;
    };
    check(fs, include_str!("fixtures/noclass/mp_carentan-uo.txt"), "1");
}
