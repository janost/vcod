//! Two clients on one server, each seeing the other. Stage 5's own gate for
//! the half no capture of a single client can reach: retail sends a client no
//! entity for itself, so the entity-list gates say nothing about what one
//! client is told about another.
//!
//! Needs `COD_DIR`; without the paks it returns early.

mod common;

use common::{Queues, ADDR, ADDR_B};
use std::cell::RefCell;
use std::collections::BTreeSet;
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
    let (mut ca, mut cb) = common::join_pair(
        &mut sv,
        &qa,
        &qb,
        &mut now,
        ("allies", "m1carbine_mp"),
        ("allies", "m1carbine_mp"),
    );

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
    let (mut ca, mut cb) = common::join_pair(
        &mut sv,
        &qa,
        &qb,
        &mut now,
        ("allies", "m1carbine_mp"),
        ("allies", "m1carbine_mp"),
    );
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

/// A field's family: `pos.trDelta[2]` and `pos.trDelta[0]` are one thing, and
/// which axes are nonzero is the player's velocity, not our coverage. The
/// comparison below is about whether we carry the field at all.
fn family(name: &str) -> &str {
    name.split_once('[').map(|(f, _)| f).unwrap_or(name)
}

/// Fields retail sets on a player entity that we do not, each with the reason.
/// Empty is the goal; the guard below fails on one that starts matching.
const PLAYER_GAPS: &[(&str, &str)] = &[
    (
        "eventSequence",
        "the entity event ring, which nothing raises until stage 6",
    ),
    ("events", "same ring"),
    ("eventParms", "the parms of that ring"),
];

/// Every field retail sets on a moving player, against ours. The capture is
/// one probe watching another on the retail server
/// (`--save-entities --capture-tag players`), so it is the only evidence of
/// what a player entity carries: a single client's capture cannot hold one,
/// since retail sends a client no entity for itself.
///
/// Field presence, not value: the captured player was walking somewhere else
/// entirely, so only which fields are set is comparable.
#[test]
fn a_player_entity_carries_the_fields_retail_sets() {
    let path = "tests/fixtures/entities/mp_carentan-dm-players.txt";
    let Ok(text) = std::fs::read_to_string(path) else {
        panic!("read {path}");
    };
    let p = &PROTOCOL_V1;
    // The union over the capture's player entities: a field set in any
    // sample. Whole blocks, because `eType` sits in the middle of one and the
    // fields above it -- the whole trajectory -- belong to the same entity.
    let mut retail_sets: BTreeSet<&str> = BTreeSet::new();
    for block in text.split("[ent ").skip(1) {
        let body = block.split_once(']').map(|(_, b)| b).unwrap_or(block);
        let body = body.split("[sample ").next().unwrap_or(body);
        if !body.lines().any(|l| l == "eType 1") {
            continue;
        }
        for line in body.lines() {
            if let Some((name, _)) = line.split_once(' ') {
                if EntityState::field_index(p, name).is_some() {
                    retail_sets.insert(name);
                }
            }
        }
    }
    assert!(
        retail_sets.contains("eType") && retail_sets.len() > 8,
        "{path}: no player entity in the capture ({} fields)",
        retail_sets.len()
    );

    let Some(ours) = a_moving_player() else {
        eprintln!("COD_DIR unset or has no main/: skipping");
        return;
    };
    // Which families we carry: any axis set covers the family.
    let ours_has: BTreeSet<&str> = p
        .entity_fields
        .iter()
        .zip(&ours.fields)
        .filter(|(_, v)| **v != 0)
        .map(|(f, _)| family(f.name))
        .collect();
    let mut missing: Vec<&str> = Vec::new();
    let mut gaps_hit: BTreeSet<&str> = BTreeSet::new();
    for name in retail_sets
        .iter()
        .map(|n| family(n))
        .collect::<BTreeSet<_>>()
    {
        if ours_has.contains(name) {
            continue;
        }
        match PLAYER_GAPS.iter().find(|(g, _)| *g == name) {
            Some((g, _)) => {
                gaps_hit.insert(g);
            }
            None => missing.push(name),
        }
    }
    for (g, why) in PLAYER_GAPS {
        assert!(
            gaps_hit.contains(g),
            "PLAYER_GAPS lists {g:?} ({why}) but we set it now; drop it"
        );
    }
    assert!(
        missing.is_empty(),
        "retail sets these on a player entity and we leave them zero: {missing:?}"
    );
}

/// One client, joined, walking, and the entity another client would be sent
/// about it. Walking matters: a standing player's velocity and trajectory
/// fields are legitimately zero and would read as missing.
fn a_moving_player() -> Option<EntityState> {
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
    let (mut ca, mut cb) = common::join_pair(
        &mut sv,
        &qa,
        &qb,
        &mut now,
        ("allies", "m1carbine_mp"),
        ("allies", "m1carbine_mp"),
    );

    let p = &PROTOCOL_V1;
    let spot = ca.snapshots().newest()?.ps.origin(p);
    let nb = cb.snapshots().newest()?.ps.field_i32(p, "clientNum") as usize;
    let na = ca.snapshots().newest()?.ps.field_i32(p, "clientNum") as usize;
    sv.place_client(na, spot, 0.0);
    sv.place_client(nb, [spot[0] + 40.0, spot[1], spot[2]], 180.0);

    // Looking somewhere as well as moving: the entity carries the body yaw,
    // and a client that never turns leaves it at the spawn's. The strafe is
    // what makes `angles2[1]` nonzero: a player running straight ahead has
    // its legs on the view yaw, and retail sends 0 there too.
    //
    // The spawn is random and `place_client` does not check the box fits, so
    // a client can end up pinned and stand still whatever it presses. Turn
    // an eighth at a time until the sim actually carries it somewhere.
    let mut latest = None;
    for turn in 0..8 {
        let walking = vcod_common::net::msg::UserCmd {
            forward: 127,
            right: 127,
            // Odd multiples of 22.5 degrees, so no turn leaves the body
            // yaw at zero and reads as a field we never set.
            angles: [0, (turn * 2 + 1) * (65536 / 16), 0],
            ..vcod_common::net::msg::NULL_USERCMD
        };
        for _ in 0..20 {
            now += Duration::from_millis(50);
            ca.send_frame(&vcod_common::net::msg::NULL_USERCMD);
            cb.send_frame(&walking);
            common::step_pair(&mut sv, (&qa, &mut ca), (&qb, &mut cb), now);
        }
        let Some(e) = ca.snapshots().newest()?.entities.get(&(nb as u32)).cloned() else {
            continue;
        };
        let speed = (0..2)
            .map(|axis| f32::from_bits(e.field_i32(p, &format!("pos.trDelta[{axis}]")) as u32))
            .map(|v| v * v)
            .sum::<f32>()
            .sqrt();
        latest = Some(e);
        if speed > 10.0 {
            break;
        }
    }
    latest
}

/// The body a client is drawn with rides the roster, not the entity: without
/// it another client is sent a player it can name but cannot see, which is
/// exactly what a retail client showed before this landed.
#[test]
fn a_joined_client_carries_a_body_model_in_the_roster() {
    let Some((sa, nb)) = a_roster_view() else {
        eprintln!("COD_DIR unset or has no main/: skipping");
        return;
    };
    let p = &PROTOCOL_V1;
    let cs = sa
        .clients
        .get(&nb)
        .unwrap_or_else(|| panic!("no roster entry for client {nb}"));
    let model = cs.field_i32(p, "modelindex");
    assert!(
        model > 0,
        "client {nb} has no body model, so another client can name it and not draw it"
    );
}

/// A spectator is not a thing in the world. Retail links no entity for one,
/// and a player's crosshair naming a spectator flying overhead is what the
/// missing check looked like.
#[test]
fn a_spectator_is_sent_to_nobody() {
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
    let (mut ca, mut cb) = common::join_pair(
        &mut sv,
        &qa,
        &qb,
        &mut now,
        ("allies", "m1carbine_mp"),
        ("allies", "m1carbine_mp"),
    );

    let p = &PROTOCOL_V1;
    let na = ca
        .snapshots()
        .newest()
        .expect("A")
        .ps
        .field_i32(p, "clientNum") as usize;
    let nb = cb
        .snapshots()
        .newest()
        .expect("B")
        .ps
        .field_i32(p, "clientNum") as usize;
    let spot = ca.snapshots().newest().expect("A").ps.origin(p);
    sv.place_client(na, spot, 0.0);
    sv.place_client(nb, [spot[0] + 40.0, spot[1], spot[2]], 180.0);
    for _ in 0..20 {
        now += Duration::from_millis(50);
        ca.send_frame(&vcod_common::net::msg::NULL_USERCMD);
        cb.send_frame(&vcod_common::net::msg::NULL_USERCMD);
        common::step_pair(&mut sv, (&qa, &mut ca), (&qb, &mut cb), now);
    }
    assert!(
        ca.snapshots()
            .newest()
            .expect("A")
            .entities
            .contains_key(&(nb as u32)),
        "the players cannot see each other, so this proves nothing about spectators"
    );

    // B goes back to spectating, where every client starts and where a dead
    // one waits.
    sv.spectate_client(nb, [spot[0] + 40.0, spot[1], spot[2] + 200.0]);
    for _ in 0..20 {
        now += Duration::from_millis(50);
        ca.send_frame(&vcod_common::net::msg::NULL_USERCMD);
        cb.send_frame(&vcod_common::net::msg::NULL_USERCMD);
        common::step_pair(&mut sv, (&qa, &mut ca), (&qb, &mut cb), now);
    }
    let sa = ca.snapshots().newest().expect("A has no snapshot");
    assert!(
        !sa.entities.contains_key(&(nb as u32)),
        "client A is still sent an entity for spectator {nb}"
    );
}

/// One client's snapshot and the other's slot, both joined and spawned.
fn a_roster_view() -> Option<(vcod_common::net::snapshot::Snapshot, u32)> {
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
    let (mut ca, mut cb) = common::join_pair(
        &mut sv,
        &qa,
        &qb,
        &mut now,
        ("allies", "m1carbine_mp"),
        ("allies", "m1carbine_mp"),
    );
    for _ in 0..20 {
        now += Duration::from_millis(50);
        ca.send_frame(&vcod_common::net::msg::NULL_USERCMD);
        cb.send_frame(&vcod_common::net::msg::NULL_USERCMD);
        common::step_pair(&mut sv, (&qa, &mut ca), (&qb, &mut cb), now);
    }
    let p = &PROTOCOL_V1;
    let nb = cb.snapshots().newest()?.ps.field_i32(p, "clientNum") as u32;
    Some((ca.snapshots().newest()?.clone(), nb))
}

/// A head and a helmet are attachments, not part of the body model: the stock
/// character script attaches them. A client sent none renders headless, which
/// is what a retail client showed.
#[test]
fn a_joined_client_carries_its_head_and_helmet_as_attachments() {
    let Some((sa, nb)) = a_roster_view() else {
        eprintln!("COD_DIR unset or has no main/: skipping");
        return;
    };
    let p = &PROTOCOL_V1;
    let cs = sa.clients.get(&nb).expect("no roster entry");
    let attached: Vec<i32> = (0..6)
        .map(|i| cs.field_i32(p, &format!("attachModelIndex[{i}]")))
        .filter(|v| *v != 0)
        .collect();
    assert!(
        attached.len() >= 2,
        "client {nb} carries {} attachments; the stock character script attaches \
         a head and a helmet at least",
        attached.len()
    );
}

/// `clientState.team` is what a client colours names and picks friend from
/// foe with, so two players on opposite teams have to read different values,
/// and each has to read the same pair whichever of them is looking. The four
/// values (`none` 0, `axis` 1, `allies` 2, `spectator` 3) are measured
/// against retail in `docs/research/clientstate-wire-format.md`. The gate
/// runs tdm: dm's `spawnPlayer` sets `.sessionteam` back to `"none"` on every
/// spawn, so both clients would read 0 there whatever the menu answered.
#[test]
fn opposite_teams_carry_different_roster_team_values() {
    const TEAM_AXIS: i32 = 1;
    const TEAM_ALLIES: i32 = 2;

    let Some(fs) = vcod_common::testing::game_fs() else {
        eprintln!("COD_DIR unset or has no main/: skipping");
        return;
    };
    let bsp_path = fs.resolve_map(MAP).expect("map in the mounted paks");
    let bsp_bytes = fs.read(&bsp_path).expect("read the bsp");
    let bsp = vcod_common::bsp::parse(&bsp_bytes).expect("parse the bsp");

    let mut now = Instant::now();
    let mut cfg = cfg();
    cfg.gametype = "tdm".into();
    let mut sv = vcod_server::Server::new(cfg, now);
    sv.load_world(vcod_server::world::World::from_bsp(&bsp));
    sv.load_scripts(Rc::new(fs)).expect("load the scripts");

    let qa = Rc::new(RefCell::new(Queues::default()));
    let qb = Rc::new(RefCell::new(Queues::default()));
    // The weapon menu is per nationality, so the two answers differ too.
    let (mut ca, mut cb) = common::join_pair(
        &mut sv,
        &qa,
        &qb,
        &mut now,
        ("allies", "m1carbine_mp"),
        ("axis", "kar98k_mp"),
    );
    for _ in 0..20 {
        now += Duration::from_millis(50);
        ca.send_frame(&vcod_common::net::msg::NULL_USERCMD);
        cb.send_frame(&vcod_common::net::msg::NULL_USERCMD);
        common::step_pair(&mut sv, (&qa, &mut ca), (&qb, &mut cb), now);
    }

    let p = &PROTOCOL_V1;
    let sa = ca.snapshots().newest().expect("A has no snapshot");
    let sb = cb.snapshots().newest().expect("B has no snapshot");
    let na = sa.ps.field_i32(p, "clientNum") as u32;
    let nb = sb.ps.field_i32(p, "clientNum") as u32;
    assert_ne!(na, nb, "both clients took the same slot");

    for (who, s) in [("A", sa), ("B", sb)] {
        let team = |n: u32| {
            s.clients
                .get(&n)
                .unwrap_or_else(|| panic!("{who}'s roster has no entry for client {n}"))
                .field_i32(p, "team")
        };
        assert_eq!(
            (team(na), team(nb)),
            (TEAM_ALLIES, TEAM_AXIS),
            "{who}'s roster reads allies {na} as {} and axis {nb} as {}",
            team(na),
            team(nb)
        );
    }
}
