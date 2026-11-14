//! Bot management end to end: `addbot`, `removebot` and `listbots` over
//! rcon, the name pool, team balance and the `vc_bot_minplayers` floor,
//! with vcod's own NetClient standing in for the humans.
//!
//! Needs `COD_DIR`; without the paks it returns early.

use crate::common;

use common::{ClientEnd, Join, Queues};
use std::cell::RefCell;
use std::net::SocketAddr;
use std::rc::Rc;
use std::time::{Duration, Instant};
use vcod_common::net::msg::NULL_USERCMD;
use vcod_common::net::{NetClient, NetEvent, NetState};
use vcod_server::Server;

const MAP: &str = "mp_carentan";
const RCON: SocketAddr =
    SocketAddr::new(std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST), 31999);

fn server(gametype: &str, max_clients: usize) -> Option<(Server, Instant)> {
    let fs = vcod_common::testing::game_fs_11()?;
    let bsp_bytes = fs.read(&fs.resolve_map(MAP)?)?;
    let bsp = vcod_common::bsp::parse(&bsp_bytes).expect("parse the bsp");
    let now = Instant::now();
    let mut cfg = common::cfg(MAP, gametype);
    cfg.max_clients = max_clients;
    let mut sv = Server::new(cfg, now);
    sv.set_cvar("rconPassword", "pw");
    // Peaceful bots: a fight would kill and move them on its own clock.
    sv.set_cvar("vc_bot_shoot", "0");
    sv.load_world(vcod_server::world::World::from_bsp(&bsp, Some(&fs)));
    sv.load_scripts(Rc::new(fs)).expect("load the scripts");
    Some((sv, now))
}

struct Human {
    addr: SocketAddr,
    q: Rc<RefCell<Queues>>,
    cl: NetClient<ClientEnd>,
    join: Join,
    gone: bool,
}

/// The `n`th human, answering the team menu with `team`.
fn human(n: u16, team: &str, now: Instant) -> Human {
    let q = Rc::new(RefCell::new(Queues::default()));
    let weapon = if team == "axis" {
        "kar98k_mp"
    } else {
        "m1garand_mp"
    };
    Human {
        addr: SocketAddr::new(
            std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST),
            32000 + n,
        ),
        cl: NetClient::start_with_qport(ClientEnd(q.clone()), now, 0x3000 + n),
        q,
        join: Join::new(team, weapon),
        gone: false,
    }
}

/// One 50 ms frame for the server and every human still connected.
fn frame(sv: &mut Server, humans: &mut [Human], now: &mut Instant) {
    *now += Duration::from_millis(50);
    for h in humans.iter_mut().filter(|h| !h.gone) {
        h.cl.send_frame(&NULL_USERCMD);
        let pending: Vec<Vec<u8>> = h.q.borrow_mut().to_server.drain(..).collect();
        for p in pending {
            sv.handle_packet(h.addr, &p, *now);
        }
    }
    sv.tick(*now);
    for (to, p) in sv.take_outgoing() {
        if let Some(h) = humans.iter().find(|h| h.addr == to && !h.gone) {
            h.q.borrow_mut().to_client.push_back(p);
        }
    }
    for h in humans.iter_mut().filter(|h| !h.gone) {
        for e in h.cl.pump_at(*now) {
            match e {
                NetEvent::ServerCommand(tokens) => {
                    h.join.on_server_command(&tokens, &mut h.cl, *now)
                }
                // A new map's menus are asked again.
                NetEvent::GamestateReady => h.join.reset_menus(),
                NetEvent::Dropped(r) => panic!("{} dropped: {r}", h.addr),
                _ => {}
            }
        }
    }
}

fn run(sv: &mut Server, humans: &mut [Human], now: &mut Instant, secs: u32) {
    for _ in 0..secs * 20 {
        frame(sv, humans, now);
    }
}

/// Disconnects a human the way a quitting client does.
fn leave(h: &mut Human, sv: &mut Server, now: Instant) {
    h.cl.disconnect();
    let pending: Vec<Vec<u8>> = h.q.borrow_mut().to_server.drain(..).collect();
    for p in pending {
        sv.handle_packet(h.addr, &p, now);
    }
    h.gone = true;
}

fn rcon(sv: &mut Server, now: &mut Instant, line: &str) -> String {
    // rcon answers one request per address every 500 ms.
    *now += Duration::from_millis(600);
    let mut pkt = b"\xff\xff\xff\xff".to_vec();
    pkt.extend_from_slice(format!("rcon pw {line}").as_bytes());
    sv.handle_packet(RCON, &pkt, *now);
    sv.take_outgoing()
        .into_iter()
        .filter(|(to, _)| *to == RCON)
        .map(|(_, p)| String::from_utf8_lossy(&p[4..]).into_owned())
        .collect()
}

/// `(allies, axis)` by `pers["team"]`, and the bots among each.
fn teams(sv: &mut Server) -> ((usize, usize), (usize, usize)) {
    let bots = sv.bot_slots();
    let mut out = ((0, 0), (0, 0));
    for slot in 0..16 {
        let bot = bots.contains(&slot);
        match sv.client_pers(slot, "team").as_deref() {
            Some("allies") => {
                out.0.0 += 1;
                out.1.0 += usize::from(bot);
            }
            Some("axis") => {
                out.0.1 += 1;
                out.1.1 += usize::from(bot);
            }
            _ => {}
        }
    }
    out
}

#[test]
fn addbot_removebot_and_listbots_over_rcon() {
    let Some((mut sv, mut now)) = server("tdm", 8) else {
        eprintln!("COD_DIR unset: skipping");
        return;
    };
    let mut none: Vec<Human> = Vec::new();
    rcon(&mut sv, &mut now, "addbot axis 2");
    rcon(&mut sv, &mut now, "addbot 2");
    run(&mut sv, &mut none, &mut now, 4);
    let slots = sv.bot_slots();
    assert_eq!(slots.len(), 4);
    let names: Vec<String> = slots
        .iter()
        .map(|&s| sv.bot_name(s).unwrap().to_string())
        .collect();
    for (slot, n) in slots.iter().zip(&names) {
        assert!(n.starts_with("[BOT] "), "{n}");
        // What the scoreboard reads.
        assert_eq!(sv.client_field(*slot, "name").as_deref(), Some(n.as_str()));
    }
    let mut unique = names.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(unique.len(), 4, "{names:?}");
    // The two pinned on axis stay; the auto pair balances against them.
    assert_eq!(teams(&mut sv), ((2, 2), (2, 2)));

    let list = rcon(&mut sv, &mut now, "listbots");
    for n in &names {
        assert!(list.contains(n.as_str()), "{list}");
    }
    assert!(list.contains("4 bot(s), 0 human player(s)"), "{list}");

    // By name, prefix optional and case folded.
    let bare = names[0]
        .strip_prefix("[BOT] ")
        .unwrap()
        .to_ascii_uppercase();
    let out = rcon(&mut sv, &mut now, &format!("removebot {bare}"));
    assert!(out.contains(&format!("removed {}", names[0])), "{out}");
    run(&mut sv, &mut none, &mut now, 1);
    assert_eq!(sv.bot_slots().len(), 3);
    assert!(rcon(&mut sv, &mut now, "removebot nobody").contains("no bot named nobody"));

    rcon(&mut sv, &mut now, "removebot 2");
    run(&mut sv, &mut none, &mut now, 1);
    assert_eq!(sv.bot_slots().len(), 1);
    rcon(&mut sv, &mut now, "removebot all");
    run(&mut sv, &mut none, &mut now, 1);
    assert!(sv.bot_slots().is_empty());
    assert!(rcon(&mut sv, &mut now, "addbot sideways").contains("usage: addbot"));
}

#[test]
fn removebot_takes_the_lowest_score_first() {
    let Some((mut sv, mut now)) = server("dm", 8) else {
        return;
    };
    let mut none: Vec<Human> = Vec::new();
    rcon(&mut sv, &mut now, "addbot 3");
    run(&mut sv, &mut none, &mut now, 4);
    let slots = sv.bot_slots();
    assert_eq!(slots.len(), 3);
    // A suicide costs a point in dm.
    sv.test_bot_command(slots[1], "kill");
    run(&mut sv, &mut none, &mut now, 1);
    assert_eq!(sv.client_field(slots[1], "score").as_deref(), Some("-1"));
    rcon(&mut sv, &mut now, "removebot");
    run(&mut sv, &mut none, &mut now, 1);
    assert_eq!(sv.bot_slots(), [slots[0], slots[2]]);
}

#[test]
fn bots_balance_against_humans_and_move_only_when_dead() {
    let Some((mut sv, mut now)) = server("tdm", 10) else {
        return;
    };
    let mut humans = vec![human(0, "allies", now), human(1, "allies", now)];
    run(&mut sv, &mut humans, &mut now, 6);
    assert_eq!(teams(&mut sv), ((2, 0), (0, 0)));
    rcon(&mut sv, &mut now, "addbot 4");
    run(&mut sv, &mut humans, &mut now, 4);
    // Two humans on allies: the first two bots go axis, then one each.
    assert_eq!(teams(&mut sv), ((3, 3), (1, 3)));

    // A third human on allies tips it to 4 against 3: within one.
    humans.push(human(2, "allies", now));
    run(&mut sv, &mut humans, &mut now, 6);
    assert_eq!(teams(&mut sv), ((4, 3), (1, 3)));
    // A fourth makes it 5 against 3. The allied bot is alive, so it stays.
    humans.push(human(3, "allies", now));
    run(&mut sv, &mut humans, &mut now, 6);
    assert_eq!(teams(&mut sv), ((5, 3), (1, 3)));
    // Once it dies it moves.
    let allied_bot = sv
        .bot_slots()
        .into_iter()
        .find(|&s| sv.client_pers(s, "team").as_deref() == Some("allies"))
        .unwrap();
    sv.test_bot_command(allied_bot, "kill");
    run(&mut sv, &mut humans, &mut now, 2);
    assert_eq!(teams(&mut sv), ((4, 4), (0, 4)));
    assert_eq!(sv.client_pers(allied_bot, "team").as_deref(), Some("axis"));
    // And spawns there through the weapon menu.
    run(&mut sv, &mut humans, &mut now, 6);
    assert_eq!(
        sv.client_field(allied_bot, "sessionteam").as_deref(),
        Some("axis")
    );
    assert_eq!(
        sv.client_field(allied_bot, "sessionstate").as_deref(),
        Some("playing")
    );
}

#[test]
fn the_floor_follows_humans_in_and_out() {
    let Some((mut sv, mut now)) = server("tdm", 8) else {
        return;
    };
    let mut humans: Vec<Human> = Vec::new();
    run(&mut sv, &mut humans, &mut now, 1);
    assert!(sv.bot_slots().is_empty());
    // Live, within a second of the write.
    rcon(&mut sv, &mut now, "set vc_bot_minplayers 4");
    run(&mut sv, &mut humans, &mut now, 1);
    assert_eq!(sv.bot_slots().len(), 4);
    run(&mut sv, &mut humans, &mut now, 3);
    assert_eq!(teams(&mut sv), ((2, 2), (2, 2)));

    // A human joining axis takes one bot's place, off the larger side.
    humans.push(human(0, "axis", now));
    run(&mut sv, &mut humans, &mut now, 6);
    assert_eq!(sv.bot_slots().len(), 3);
    let ((allies, axis), _) = teams(&mut sv);
    assert_eq!(allies + axis, 4);
    assert!(allies.abs_diff(axis) <= 1, "{allies} {axis}");

    // Leaving brings it back.
    leave(&mut humans[0], &mut sv, now);
    run(&mut sv, &mut humans, &mut now, 2);
    assert_eq!(sv.bot_slots().len(), 4);

    // The floor going down drops bots; 0 is off.
    rcon(&mut sv, &mut now, "set vc_bot_minplayers 2");
    run(&mut sv, &mut humans, &mut now, 1);
    assert_eq!(sv.bot_slots().len(), 2);
    rcon(&mut sv, &mut now, "set vc_bot_minplayers 0");
    run(&mut sv, &mut humans, &mut now, 1);
    assert!(sv.bot_slots().is_empty());
}

#[test]
fn the_floor_leaves_a_public_slot_free() {
    let Some((mut sv, mut now)) = server("tdm", 6) else {
        return;
    };
    sv.set_cvar("sv_privateClients", "2");
    // Without a password of its own a human may take a private slot too.
    sv.set_cvar("sv_privatePassword", "secret");
    let mut humans: Vec<Human> = Vec::new();
    rcon(&mut sv, &mut now, "set vc_bot_minplayers 20");
    run(&mut sv, &mut humans, &mut now, 2);
    // Six slots, two private, one kept for a joining human.
    let slots = sv.bot_slots();
    assert_eq!(slots, [2, 3, 4]);
    humans.push(human(0, "allies", now));
    run(&mut sv, &mut humans, &mut now, 4);
    assert_eq!(sv.bot_slots().len(), 2);
    // Manual bots may fill the public slots, never the private ones.
    rcon(&mut sv, &mut now, "set vc_bot_minplayers 0");
    rcon(&mut sv, &mut now, "addbot 10");
    run(&mut sv, &mut humans, &mut now, 2);
    assert_eq!(sv.bot_slots().len(), 3);
    assert!(sv.bot_slots().iter().all(|&s| s >= 2));
}

#[test]
fn vc_bot_names_off_numbers_the_bots() {
    let Some((mut sv, mut now)) = server("dm", 8) else {
        return;
    };
    let mut none: Vec<Human> = Vec::new();
    rcon(&mut sv, &mut now, "set vc_bot_names 0");
    rcon(&mut sv, &mut now, "addbot 1");
    run(&mut sv, &mut none, &mut now, 1);
    let slot = sv.bot_slots()[0];
    assert_eq!(sv.bot_name(slot), Some(format!("bot{}", slot + 1).as_str()));
}

/// `addbot ... skill N` pins a bot's level, auto-side bots included;
/// `vc_bot_skill` moves every other bot live, and clamps.
#[test]
fn vc_bot_skill_and_addbot_skill_set_the_level() {
    let Some((mut sv, mut now)) = server("tdm", 8) else {
        return;
    };
    let mut none: Vec<Human> = Vec::new();
    rcon(&mut sv, &mut now, "addbot axis 1 skill 3");
    rcon(&mut sv, &mut now, "addbot skill 0 1");
    rcon(&mut sv, &mut now, "addbot 1");
    run(&mut sv, &mut none, &mut now, 2);
    let levels = |sv: &Server| -> Vec<Option<u8>> {
        sv.bot_slots().iter().map(|&s| sv.bot_skill(s)).collect()
    };
    assert_eq!(levels(&sv), [Some(3), Some(0), Some(1)]);
    rcon(&mut sv, &mut now, "set vc_bot_skill 9");
    run(&mut sv, &mut none, &mut now, 1);
    assert_eq!(levels(&sv), [Some(3), Some(0), Some(3)]);
    rcon(&mut sv, &mut now, "set vc_bot_skill 2");
    run(&mut sv, &mut none, &mut now, 1);
    assert_eq!(levels(&sv), [Some(3), Some(0), Some(2)]);
    let list = rcon(&mut sv, &mut now, "listbots");
    assert!(list.contains("vc_bot_skill 2"), "{list}");
    assert!(rcon(&mut sv, &mut now, "addbot skill").contains("usage: addbot"));
}

/// A passed `callvote map` on a server of 46 bots while both humans take
/// 15 s to load: a loading client acknowledges nothing, and the bots'
/// reconnect lines are prints, which a client not active is never queued
/// (docs/protocol-1.1.md, "The server command queue"), so neither human
/// overflows its 64 slots and both play on the new map.
#[test]
fn a_voted_map_change_keeps_a_slow_loading_client() {
    let Some((mut sv, mut now)) = server("tdm", 48) else {
        eprintln!("COD_DIR unset: skipping");
        return;
    };
    let mut humans = vec![human(0, "allies", now), human(1, "axis", now)];
    run(&mut sv, &mut humans, &mut now, 4);
    rcon(&mut sv, &mut now, "addbot 46");
    run(&mut sv, &mut humans, &mut now, 4);
    assert_eq!(sv.bot_slots().len(), 46);
    let id = sv.server_id();
    humans[0].cl.send_reliable("callvote map mp_brecourt");
    run(&mut sv, &mut humans, &mut now, 1);
    humans[1].cl.send_reliable("vote yes");
    for _ in 0..400 {
        if sv.server_id() != id {
            break;
        }
        frame(&mut sv, &mut humans, &mut now);
    }
    assert_ne!(sv.server_id(), id, "the vote never changed the map");
    // The load: no packet either way.
    for _ in 0..15 * 20 {
        now += Duration::from_millis(50);
        sv.tick(now);
        sv.take_outgoing();
    }
    run(&mut sv, &mut humans, &mut now, 8);
    for (slot, h) in humans.iter().enumerate() {
        assert_eq!(h.cl.state(), NetState::Active, "{}", h.join.summary());
        assert_eq!(h.cl.server_id(), i32::from(sv.server_id()));
        assert_eq!(
            sv.client_field(slot, "sessionstate").as_deref(),
            Some("playing"),
            "{}",
            h.join.summary()
        );
    }
}
