//! Votes against the retail captures in `fixtures/votes/`: three clients
//! (axis, allies, spectator) send the captures' `callvote` and `vote`
//! commands at the same seconds, and each must get the same `e` lines, `n`s
//! and configstring 15..18 changes in the same order
//! (docs/research/cod11-votes.md). The 1.1d run needs `COD_DIR` at a 1.1
//! install, the 1.5b run at a 1.5 one; each returns early otherwise.

use crate::common;

use common::{ADDR, ADDR_B, ADDR_C, ClientEnd, Join, Queues};
use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};
use vcod_common::net::protocol::{PROTOCOL_V1, PROTOCOL_V6, Protocol};
use vcod_common::net::{NetClient, NetEvent, NetState, Userinfo};

const NAMES: [&str; 3] = ["Able", "Baker", "Charlie"];
const PICKS: [(&str, &str); 3] = [
    ("axis", "kar98k_mp"),
    ("allies", "m1carbine_mp"),
    ("spectator", ""),
];

/// A client command, or a console line when `who` is `None`.
struct Say {
    secs: f32,
    who: Option<usize>,
    cmd: String,
}

struct Retail {
    says: Vec<Say>,
    lines: [Vec<String>; 3],
}

fn unescape(s: &str) -> String {
    s.replace("\\x14", "\u{14}").replace("\\x15", "\u{15}")
}

fn who(name: &str) -> usize {
    NAMES
        .iter()
        .position(|n| *n == name)
        .expect("a client name")
}

fn retail(fixture: &str) -> Retail {
    let path = format!(
        "{}/tests/fixtures/votes/{fixture}",
        env!("CARGO_MANIFEST_DIR")
    );
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
    let mut r = Retail {
        says: Vec::new(),
        lines: Default::default(),
    };
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("!say ") {
            let mut it = rest.splitn(3, ' ');
            let (name, secs, cmd) = (it.next().unwrap(), it.next().unwrap(), it.next().unwrap());
            r.says.push(Say {
                secs: secs.parse().unwrap(),
                who: Some(who(name)),
                cmd: cmd.to_string(),
            });
        } else if let Some(rest) = line.strip_prefix("!rcon ") {
            let (secs, cmd) = rest.split_once(' ').unwrap();
            r.says.push(Say {
                secs: secs.parse().unwrap(),
                who: None,
                cmd: cmd.to_string(),
            });
        } else if let Some(rest) = line.strip_prefix("!cmd ") {
            let (name, cmd) = rest.split_once(' ').unwrap();
            r.lines[who(name)].push(unescape(cmd));
        }
    }
    r.says.sort_by(|a, b| a.secs.total_cmp(&b.secs));
    r
}

/// `order` is the connect order, which fixes the slots: retail's 1.5b run
/// put the spectator in slot 0.
fn run(fixture: &str, proto: &'static Protocol, cvars: &[(&str, &str)], order: [usize; 3]) {
    let Some(fs) = vcod_common::testing::game_fs() else {
        return;
    };
    if vcod_server::server::install_protocol(&fs).transport.version != proto.transport.version {
        return;
    }
    let retail = retail(fixture);
    let bsp_path = fs
        .resolve_map("mp_carentan")
        .expect("map in the mounted paks");
    let bsp = vcod_common::bsp::parse(&fs.read(&bsp_path).unwrap()).unwrap();
    let fs = Rc::new(fs);
    let mut now = Instant::now();
    let mut sv = vcod_server::Server::new(common::cfg("mp_carentan", "tdm"), now);
    sv.set_protocol(proto);
    for (name, value) in cvars {
        sv.set_cvar(name, value);
    }
    sv.load_world(vcod_server::world::World::from_bsp(&bsp, Some(&fs)));
    sv.load_scripts(fs).expect("load the scripts");

    let addrs = [ADDR, ADDR_B, ADDR_C];
    let queues: [Rc<RefCell<Queues>>; 3] = Default::default();
    let mut cl: [NetClient<ClientEnd>; 3] = std::array::from_fn(|i| {
        let mut c =
            NetClient::start_with_qport(ClientEnd(queues[i].clone()), now, 0x2001 + i as u16);
        c.set_userinfo(Userinfo {
            name: NAMES[i].into(),
            ..Userinfo::default()
        });
        c
    });
    let mut joins: [Join; 3] = std::array::from_fn(|i| Join::new(PICKS[i].0, PICKS[i].1));
    let mut got: [Vec<String>; 3] = Default::default();
    let mut joined = None;
    let mut says = retail.says.iter().peekable();
    for frame in 0..2400 {
        now += Duration::from_millis(common::FRAME_MS as u64);
        // Join in `order`, one client a frame, so each takes its slot.
        let live = (frame + 1).min(3);
        if let Some(start) = joined {
            let secs = now.duration_since(start).as_secs_f32();
            while let Some(s) = says.next_if(|s| secs >= s.secs) {
                match s.who {
                    Some(i) => cl[i].send_reliable(&s.cmd),
                    None => sv.push_console(&s.cmd),
                }
            }
        }
        // Once all three are in and until the pair has joined, the packets
        // go in the reverse of `order`, so the clients answer and spawn in
        // `order` (`common::join_pair_stamped`). After that the packets
        // keep `order`, so two clients' votes in one frame arrive in it, and
        // the clients a vote's restart sends back through the menus spawn
        // in the reverse, which no compared line reads (with every frame
        // reversed the lines are the same).
        let mut packets: Vec<usize> = order[..live].to_vec();
        if joined.is_none() && cl.iter().all(|c| c.state() == NetState::Active) {
            packets.reverse();
        }
        for i in packets {
            cl[i].send_frame(&vcod_common::net::msg::NULL_USERCMD);
            let pending: Vec<Vec<u8>> = queues[i].borrow_mut().to_server.drain(..).collect();
            for p in pending {
                sv.handle_packet(addrs[i], &p, now);
            }
        }
        sv.tick(now);
        for (to, p) in sv.take_outgoing() {
            let i = addrs.iter().position(|a| *a == to).unwrap();
            queues[i].borrow_mut().to_client.push_back(p);
        }
        for i in 0..3 {
            for e in cl[i].pump_at(now) {
                match e {
                    NetEvent::ServerCommand(tokens) => {
                        if tokens.first().is_some_and(|t| t == "n") {
                            joins[i].reset_menus();
                        }
                        joins[i].on_server_command(&tokens, &mut cl[i], now);
                        if joined.is_none() {
                            continue;
                        }
                        match tokens.first().map(String::as_str) {
                            Some("e") => got[i].push(format!(
                                "e \"{}\"",
                                tokens.get(1).map_or("", String::as_str)
                            )),
                            Some("n") => got[i].push("n".into()),
                            _ => {}
                        }
                    }
                    NetEvent::ConfigstringChanged(cs @ 15..=18) if joined.is_some() => {
                        let mut v = cl[i].configstring(cs).to_string();
                        if cs == 15
                            && proto.transport.version == PROTOCOL_V1.transport.version
                            && !v.is_empty()
                        {
                            let t = cl[i].snapshots().newest().map_or(0, |s| s.server_time);
                            v = format!("+{}", v.parse::<i32>().unwrap() - t);
                        }
                        got[i].push(format!("d {cs} {v}"));
                    }
                    NetEvent::Dropped(r) => panic!("{} dropped: {r}", NAMES[i]),
                    _ => {}
                }
            }
        }
        if joined.is_none() && joins[0].settled(now) && joins[1].settled(now) {
            joined = Some(now);
        }
        if says.peek().is_none() && (0..3).all(|i| got[i].len() >= retail.lines[i].len()) {
            break;
        }
    }
    assert!(joined.is_some(), "{}", joins[0].summary());
    for i in 0..3 {
        assert_eq!(got[i], retail.lines[i], "{}'s lines", NAMES[i]);
    }
}

#[test]
fn votes_run_as_11d_ran_them() {
    run(
        "mp_carentan-tdm-votes.txt",
        &PROTOCOL_V1,
        &[("rconPassword", "pw")],
        [0, 1, 2],
    );
}

#[test]
fn votes_run_as_15b_ran_them() {
    run(
        "mp_carentan-tdm-votes-1.5.txt",
        &PROTOCOL_V6,
        &[
            ("g_allowVoteKick", "1"),
            ("g_allowVoteKillCam", "1"),
            ("g_allowVoteFriendlyFire", "1"),
            ("g_allowVoteDrawFriend", "1"),
            ("g_allowVoteTempBanClient", "1"),
        ],
        [2, 0, 1],
    );
}
