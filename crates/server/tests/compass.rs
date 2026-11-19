//! `iCompassFriendInfo` on the wire: the teammate a client's snapshot lacks,
//! at the spots `client-probes/probe_compass` measured on retail mp_harbor
//! (docs/research/cod11-hud-protocol.md, section 9, "Compass friendlies").
//!
//! Needs `COD_DIR`; without the paks it returns early.

use crate::common;

use common::Queues;
use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};
use vcod_common::net::msg::NULL_USERCMD;
use vcod_common::net::protocol::PROTOCOL_V1;

const MAP: &str = "mp_harbor";

fn server() -> Option<vcod_server::Server> {
    let fs = vcod_common::testing::game_fs_11()?;
    let (sv, _) = common::Boot::new(MAP, "tdm").start(fs);
    Some(sv)
}

#[test]
fn an_out_of_view_teammate_is_packed_as_retail_packs_it() {
    let mut now = Instant::now();
    let Some(mut sv) = server() else {
        eprintln!("COD_DIR unset or has no main/: skipping");
        return;
    };
    let qa = Rc::new(RefCell::new(Queues::default()));
    let qb = Rc::new(RefCell::new(Queues::default()));
    let (mut ca, mut cb) = common::join_pair(
        &mut sv,
        &qa,
        &qb,
        &mut now,
        ("allies", "mosin_nagant_mp"),
        ("allies", "mosin_nagant_mp"),
    );
    let p = &PROTOCOL_V1;
    let na = ca
        .snapshots()
        .newest()
        .unwrap()
        .ps
        .field_i32(p, "clientNum");
    let nb = cb
        .snapshots()
        .newest()
        .unwrap()
        .ps
        .field_i32(p, "clientNum");
    // Viewer slot and teammate placed, three frames stepped: the viewer's
    // `iCompassFriendInfo` and `eFlags`.
    let mut after = |sv: &mut vcod_server::Server, at: Option<[f32; 3]>| {
        if let Some(at) = at {
            sv.place_client(na as usize, [-7352.0, -7976.0, 0.125], 0.0);
            sv.place_client(nb as usize, at, 0.0);
        }
        for _ in 0..3 {
            now += Duration::from_millis(50);
            ca.send_frame(&NULL_USERCMD);
            cb.send_frame(&NULL_USERCMD);
            common::step_pair(sv, (&qa, &mut ca), (&qb, &mut cb), now);
        }
        let ps = &ca.snapshots().newest().unwrap().ps;
        (
            ps.field_i32(p, "iCompassFriendInfo") as u32,
            ps.field_i32(p, "eFlags"),
        )
    };
    // Out of view, in range: retail sent 0x00a08f01 with the teammate in
    // slot 1.
    let (info, ef) = after(&mut sv, Some([-8136.0, -7712.0, 0.125]));
    assert_eq!(info, 0x00a08f00 | nb as u32);
    assert_eq!(ef & 0x100000, 0);
    // Its `pingPlayer` bit reaches the viewer as `eFlags` 0x100000.
    sv.test_ping_player(nb as usize);
    assert_eq!(after(&mut sv, None).1 & 0x100000, 0x100000);
    // Past -1022 on x: y scaled with it, x clamped (retail 0x004a8001).
    let (info, _) = after(&mut sv, Some([-9104.0, -8712.0, 0.125]));
    assert_eq!(info, 0x004a8000 | nb as u32);
    // In view: nothing.
    assert_eq!(after(&mut sv, Some([-6784.0, -7416.0, 0.125])).0, 0);
}

/// Each slot's end frame packs its compass in slot order, reading a higher
/// slot's entity as its last end frame left it: a lower viewer loses a
/// teammate who left `playing` a frame late, a higher viewer at once
/// (retail 1.1d, `client-probes/probe_compass` either way round,
/// cod11-hud-protocol.md, "Compass friendlies").
#[test]
fn a_lower_slot_reads_a_higher_ones_end_frame_a_frame_late() {
    for dies_high in [true, false] {
        let mut now = Instant::now();
        let Some(mut sv) = server() else {
            return;
        };
        let qa = Rc::new(RefCell::new(Queues::default()));
        let qb = Rc::new(RefCell::new(Queues::default()));
        let (mut ca, mut cb) = common::join_pair(
            &mut sv,
            &qa,
            &qb,
            &mut now,
            ("allies", "mosin_nagant_mp"),
            ("allies", "mosin_nagant_mp"),
        );
        let p = &PROTOCOL_V1;
        let num = |c: &vcod_common::net::NetClient<common::ClientEnd>| {
            c.snapshots().newest().unwrap().ps.field_i32(p, "clientNum") as usize
        };
        let (na, nb) = (num(&ca), num(&cb));
        assert!(na < nb);
        sv.place_client(na, [-7352.0, -7976.0, 0.125], 0.0);
        sv.place_client(nb, [-8136.0, -7712.0, 0.125], 0.0);
        let mut step = |sv: &mut vcod_server::Server, frames: usize| {
            for _ in 0..frames {
                now += Duration::from_millis(50);
                ca.send_frame(&NULL_USERCMD);
                cb.send_frame(&NULL_USERCMD);
                common::step_pair(sv, (&qa, &mut ca), (&qb, &mut cb), now);
            }
            let info = |c: &vcod_common::net::NetClient<common::ClientEnd>| {
                c.snapshots()
                    .newest()
                    .unwrap()
                    .ps
                    .field_i32(p, "iCompassFriendInfo")
            };
            (info(&ca), info(&cb))
        };
        let (a, b) = step(&mut sv, 3);
        assert!(a != 0 && b != 0, "both out of view: {a:#x} {b:#x}");
        sv.test_set_client_session(if dies_high { nb } else { na }, "dead");
        let (a, b) = step(&mut sv, 1);
        if dies_high {
            assert_ne!(a, 0, "the lower viewer still reads the higher slot");
        } else {
            assert_eq!(b, 0, "the higher viewer reads the lower slot's end frame");
        }
        let (a, _) = step(&mut sv, 1);
        if dies_high {
            assert_eq!(a, 0, "a frame later");
        }
    }
}
