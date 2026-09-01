//! Two clients on one server, each seeing the other. Stage 5's own gate for
//! the half no capture of a single client can reach: retail sends a client no
//! entity for itself, so the entity-list gates say nothing about what one
//! client is told about another.
//!
//! Needs `COD_DIR`; without the paks it returns early.

mod common;

use common::{Queues, ADDR, ADDR_B};
use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};
use vcod_common::net::msg::EntityState;
use vcod_common::net::protocol::PROTOCOL_V1;

const MAP: &str = "mp_carentan";

fn cfg() -> vcod_server::ServerConfig {
    vcod_server::ServerConfig {
        map: MAP.into(),
        hostname: "vcod test".into(),
        max_clients: 8,
        gametype: "dm".into(),
        test_entities: 0,
        trace: false,
    }
}

/// Both clients joined, spawned and stepped far enough that each has a
/// snapshot describing the other.
fn two_joined() -> Option<(EntityState, EntityState, [f32; 3], [f32; 3])> {
    let fs = vcod_common::testing::game_fs()?;
    let bsp_path = fs.resolve_map(MAP).expect("map in the mounted paks");
    let bsp_bytes = fs.read(&bsp_path).expect("read the bsp");
    let bsp = vcod_common::bsp::parse(&bsp_bytes).expect("parse the bsp");

    let mut now = Instant::now();
    let mut sv = vcod_server::Server::new(cfg(), now);
    sv.load_world(vcod_server::world::World::from_bsp(&bsp));
    sv.load_scripts(Rc::new(fs)).expect("load the scripts");

    let qa = Rc::new(RefCell::new(Queues::default()));
    let qb = Rc::new(RefCell::new(Queues::default()));
    let (mut ca, mut cb) = common::join_pair(&mut sv, &qa, &qb, &mut now, "allies", "m1carbine_mp");

    // Both spawn weighted-random, which on carentan is often two ends of the
    // map and two clusters: whether they can see each other is then the PVS's
    // answer, not this gate's subject. So put them in one place first.
    let p = &PROTOCOL_V1;
    let (na, nb) = (
        ca.snapshots()
            .newest()
            .expect("A has no snapshot")
            .ps
            .field_i32(p, "clientNum") as usize,
        cb.snapshots()
            .newest()
            .expect("B has no snapshot")
            .ps
            .field_i32(p, "clientNum") as usize,
    );
    let spot = ca.snapshots().newest().unwrap().ps.origin(p);
    sv.place_client(na, spot, 0.0);
    sv.place_client(nb, [spot[0] + 40.0, spot[1], spot[2]], 180.0);

    // A few more frames so both spawns have settled and each client has a
    // snapshot built after the other existed.
    for _ in 0..40 {
        now += Duration::from_millis(50);
        ca.send_frame(&vcod_common::net::msg::NULL_USERCMD);
        cb.send_frame(&vcod_common::net::msg::NULL_USERCMD);
        common::step_pair(&mut sv, (&qa, &mut ca), (&qb, &mut cb), now);
    }

    let sa = ca.snapshots().newest().expect("client A got no snapshot");
    let sb = cb.snapshots().newest().expect("client B got no snapshot");
    let (na, nb) = (na as u32, nb as u32);
    assert_ne!(na, nb, "both clients took the same slot");
    assert_eq!(
        (ADDR.port() != ADDR_B.port()) as u8,
        1,
        "the harness gave both clients one address"
    );

    // What each is told about the other, and where the other actually is.
    let of_b = sa
        .entities
        .get(&nb)
        .unwrap_or_else(|| panic!("client A was sent no entity for client B ({nb})"))
        .clone();
    let of_a = sb
        .entities
        .get(&na)
        .unwrap_or_else(|| panic!("client B was sent no entity for client A ({na})"))
        .clone();
    let origin_a = sa.ps.origin(p);
    let origin_b = sb.ps.origin(p);
    assert!(
        !sa.entities.contains_key(&na) && !sb.entities.contains_key(&nb),
        "a client was sent an entity for itself; retail sends none"
    );
    Some((of_a, of_b, origin_a, origin_b))
}

#[test]
fn two_clients_are_sent_each_other_as_players() {
    let Some((of_a, of_b, origin_a, origin_b)) = two_joined() else {
        eprintln!("COD_DIR unset or has no main/: skipping");
        return;
    };
    let p = &PROTOCOL_V1;
    for (who, e, origin) in [("A", &of_a, origin_a), ("B", &of_b, origin_b)] {
        assert_eq!(e.field_i32(p, "eType"), 1, "{who} is not ET_PLAYER");
        assert_eq!(
            e.field_i32(p, "clientNum") as u32,
            e.number,
            "{who}'s clientNum is not its slot"
        );
        // The position the other client is shown is where the player is, not
        // where it spawned: the entity is built from the sim every frame.
        let at = e.origin(p);
        for axis in 0..3 {
            assert!(
                (at[axis] - origin[axis]).abs() < 0.01,
                "{who} is at {origin:?} but is shown at {at:?}"
            );
        }
        // A moving player travels as a trajectory, which is what keeps the
        // other client's view of it smooth between snapshots.
        assert_eq!(e.field_i32(p, "pos.trType"), 3, "{who} is sent as a point");
        assert_eq!(e.field_i32(p, "weapon"), 12, "{who} carries no weapon");
    }
}

/// The script half: `getentarray("player", "classname")` is how every stock
/// script reaches the players, and the entity-list gates cannot see it.
#[test]
fn the_scripts_can_find_both_players_by_classname() {
    let Some(fs) = vcod_common::testing::game_fs() else {
        eprintln!("COD_DIR unset or has no main/: skipping");
        return;
    };
    let bsp_path = fs.resolve_map(MAP).expect("map in the mounted paks");
    let bsp_bytes = fs.read(&bsp_path).expect("read the bsp");
    let bsp = vcod_common::bsp::parse(&bsp_bytes).expect("parse the bsp");

    let mut now = Instant::now();
    let mut sv = vcod_server::Server::new(cfg(), now);
    sv.load_world(vcod_server::world::World::from_bsp(&bsp));
    sv.load_scripts(Rc::new(fs)).expect("load the scripts");
    let qa = Rc::new(RefCell::new(Queues::default()));
    let qb = Rc::new(RefCell::new(Queues::default()));
    let (mut ca, mut cb) = common::join_pair(&mut sv, &qa, &qb, &mut now, "allies", "m1carbine_mp");
    for _ in 0..20 {
        now += Duration::from_millis(50);
        ca.send_frame(&vcod_common::net::msg::NULL_USERCMD);
        cb.send_frame(&vcod_common::net::msg::NULL_USERCMD);
        common::step_pair(&mut sv, (&qa, &mut ca), (&qb, &mut cb), now);
    }

    let found = sv.script_players();
    assert_eq!(
        found.len(),
        2,
        "getentarray(\"player\", \"classname\") found {} of 2 clients",
        found.len()
    );
    // And each carries where its sim put it, not where it spawned: with only
    // `spawn` writing the field, `positionWouldTelefrag` tested a stale spot.
    for (slot, origin) in &found {
        assert!(
            origin.iter().any(|v| *v != 0.0),
            "client {slot} is at the origin, so nothing is syncing it"
        );
    }
}
