//! `iCompassFriendInfo` on the wire: the teammate a client's snapshot lacks,
//! at the spots `client-probes/probe_compass` measured on retail mp_harbor
//! (docs/research/cod11-hud-protocol.md, section 9, "Compass friendlies").
//!
//! Needs `COD_DIR`; without the paks it returns early.

mod common;

use common::Queues;
use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};
use vcod_common::net::msg::NULL_USERCMD;
use vcod_common::net::protocol::PROTOCOL_V1;

const MAP: &str = "mp_harbor";

fn server(now: Instant) -> Option<vcod_server::Server> {
    let fs = vcod_common::testing::game_fs()?;
    let bsp_path = fs.resolve_map(MAP).expect("map in the mounted paks");
    let bsp = vcod_common::bsp::parse(&fs.read(&bsp_path).unwrap()).unwrap();
    let mut sv = vcod_server::Server::new(common::cfg(MAP, "tdm"), now);
    sv.load_world(vcod_server::world::World::from_bsp(&bsp, Some(&fs)));
    sv.load_scripts(Rc::new(fs)).expect("load the scripts");
    Some(sv)
}

#[test]
fn an_out_of_view_teammate_is_packed_as_retail_packs_it() {
    let mut now = Instant::now();
    let Some(mut sv) = server(now) else {
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
