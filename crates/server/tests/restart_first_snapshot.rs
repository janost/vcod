//! The first snapshot after a `map_restart`, against retail's live runs
//! (docs/research/cod11-map-cycle.md, 4.7): the settle frames move the
//! clock 300 ms with no snapshot, the frames the restart's own run time
//! owes then run back to back in one `SV_Frame`, and only the last of them
//! is sent. Retail's first snapshot read the last one before plus 300 plus
//! 50 a frame owed, and its `commandTime` the first frame after the
//! settle, plus 350, in every run.
//!
//! Needs `COD_DIR`; without the paks it returns early.

use crate::common;

use common::{ADDR, ClientEnd, Queues};
use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};
use vcod_common::net::NetClient;
use vcod_common::net::msg::NULL_USERCMD;
use vcod_common::net::protocol::PROTOCOL_V1;

const MAP: &str = "mp_carentan";

/// The frame loop's shape: the packets, the console, then one `SV_Frame`
/// running `frames` game frames.
fn step_frames(
    sv: &mut vcod_server::Server,
    q: &Rc<RefCell<Queues>>,
    cl: &mut NetClient<ClientEnd>,
    now: Instant,
    frames: u32,
) {
    let pending: Vec<Vec<u8>> = q.borrow_mut().to_server.drain(..).collect();
    for p in pending {
        sv.handle_packet(ADDR, &p, now);
    }
    sv.drain_console(now);
    sv.tick_frames(now, frames);
    for (_, p) in sv.take_outgoing() {
        q.borrow_mut().to_client.push_back(p);
    }
    cl.pump_at(now);
}

/// (serverTime, commandTime, snapFlags) of every snapshot newer than `after`.
fn snaps_after(cl: &NetClient<ClientEnd>, after: i32) -> Vec<(i32, i32, u32)> {
    let p = &PROTOCOL_V1;
    let Some(newest) = cl.snapshots().newest().map(|s| s.message_num) else {
        return Vec::new();
    };
    (newest.saturating_sub(31)..=newest)
        .filter_map(|n| cl.snapshots().get(n))
        .filter(|s| s.server_time.raw() > after)
        .map(|s| {
            (
                s.server_time.raw(),
                s.ps.field_i32(p, "commandTime"),
                s.snap_flags,
            )
        })
        .collect()
}

#[test]
fn the_first_snapshot_after_a_restart_is_the_last_frame_it_owed() {
    let Some(fs) = vcod_common::testing::game_fs_11() else {
        eprintln!("COD_DIR unset or has no main/: skipping");
        return;
    };
    let (mut sv, mut now) = common::Boot::new(MAP, "dm").start(fs);
    let q = Rc::new(RefCell::new(Queues::default()));
    let mut cl = common::connect(&mut sv, &q, &mut now);
    for _ in 0..40 {
        now += Duration::from_millis(50);
        cl.send_frame(&NULL_USERCMD);
        step_frames(&mut sv, &q, &mut cl, now, 1);
    }

    // The frames owed by the restart's own run, as `main.rs` counts them.
    for owed in [1u32, 5] {
        let last = cl
            .snapshots()
            .newest()
            .expect("snapshots before the restart");
        let (t, flags) = (last.server_time.raw(), last.snap_flags);
        sv.push_console("map_restart");
        now += Duration::from_millis(50) * owed;
        cl.send_frame(&NULL_USERCMD);
        step_frames(&mut sv, &q, &mut cl, now, owed);
        assert_eq!(
            snaps_after(&cl, t),
            vec![(t + 300 + 50 * owed as i32, t + 350, flags ^ 4)],
            "owing {owed} frames: one snapshot, the last frame's, after {t}"
        );
        for _ in 0..40 {
            now += Duration::from_millis(50);
            cl.send_frame(&NULL_USERCMD);
            step_frames(&mut sv, &q, &mut cl, now, 1);
        }
    }
}
