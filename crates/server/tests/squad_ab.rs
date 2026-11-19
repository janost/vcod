//! 1.5's `say_squad` between squad members against retail 1.5b
//! (`fixtures/chat/mp_carentan-squad-1.5.txt`, recipe in its header):
//! `client-probes/probe_squad` puts four clients on sides and in squads,
//! three of them say, and each client must get retail's `h`/`i` lines
//! (cod15-protocol-diff.md 13). The fixture's `!log` lines are the
//! `games_mp.log` format `game::say`'s unit tests pin. Needs `COD_DIR` at a
//! 1.5 install; returns early otherwise.

use crate::common;

use common::{ClientEnd, Queues};
use std::cell::RefCell;
use std::net::SocketAddr;
use std::rc::Rc;
use std::time::Duration;
use vcod_common::net::protocol::PROTOCOL_V6;
use vcod_common::net::{NetClient, NetEvent, Userinfo};

const PROBE: &str = "probe_squad";

/// A chat line as the probe prints it: the command letter (`i` a team
/// line) and the text, control bytes as `\xNN`.
fn escaped(letter: &str, text: &str) -> String {
    let body: String = text
        .chars()
        .map(|c| {
            if (c as u32) < 0x20 {
                format!("\\x{:02x}", c as u32)
            } else {
                c.to_string()
            }
        })
        .collect();
    format!("{letter} \"{body}\"")
}

#[test]
fn squad_says_reach_retail_s_listeners_on_15b() {
    let Some(fs) = vcod_common::testing::game_fs_15() else {
        return;
    };
    let text = include_str!("fixtures/chat/mp_carentan-squad-1.5.txt");
    let says: Vec<(usize, &str)> = text
        .lines()
        .filter_map(|l| l.strip_prefix("!say "))
        .map(|r| {
            let (n, cmd) = r.split_once(' ').unwrap();
            (n.parse().unwrap(), cmd)
        })
        .collect();
    let mut want: [Vec<String>; 4] = Default::default();
    for r in text.lines().filter_map(|l| l.strip_prefix("!chat ")) {
        let (n, line) = r.split_once(' ').unwrap();
        want[n.parse::<usize>().unwrap()].push(line.to_string());
    }

    let (mut sv, mut now) = common::Boot {
        proto: Some(&PROTOCOL_V6),
        script: Some(include_str!(
            "../../gsc/tests/fixtures/semantics/client-probes/probe_squad.gsc"
        )),
        ..common::Boot::new("mp_carentan", PROBE)
    }
    .start(fs);

    let addrs: [SocketAddr; 4] =
        std::array::from_fn(|i| SocketAddr::from(([127, 0, 0, 1], 31400 + i as u16)));
    let queues: [Rc<RefCell<Queues>>; 4] = Default::default();
    let mut cl: [NetClient<ClientEnd>; 4] = std::array::from_fn(|i| {
        let mut c =
            NetClient::start_with_qport(ClientEnd(queues[i].clone()), now, 0x2101 + i as u16);
        c.set_userinfo(Userinfo {
            name: "vcod".into(),
            ..Userinfo::default()
        });
        c
    });
    let mut got: [Vec<String>; 4] = Default::default();
    let mut placed_at = None;
    let mut says = says.iter();
    let mut next_say = None;
    for frame in 0..800 {
        now += Duration::from_millis(common::FRAME_MS as u64);
        // One client more every 30 frames, so each takes its slot in turn.
        let live = (frame / 30 + 1).min(4);
        if placed_at.is_none()
            && sv
                .script_log()
                .iter()
                .filter(|l| l.contains("PROBE squad "))
                .count()
                == 4
        {
            placed_at = Some(now);
            next_say = Some(now + Duration::from_secs(1));
        }
        // A second between says, clear of retail's 800 ms flood window.
        if next_say.is_some_and(|t| now >= t) {
            match says.next() {
                Some((n, cmd)) => {
                    cl[*n].send_reliable(cmd);
                    next_say = Some(now + Duration::from_secs(1));
                }
                None => next_say = None,
            }
        }
        for i in 0..live {
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
        for i in 0..live {
            for e in cl[i].pump_at(now) {
                if let NetEvent::Chat { text, team } = e {
                    got[i].push(escaped(if team { "i" } else { "h" }, &text));
                }
            }
        }
        if placed_at.is_some() && next_say.is_none() && frame % 20 == 0 {
            break;
        }
    }
    assert!(placed_at.is_some(), "the probe never placed four clients");
    for i in 0..4 {
        assert_eq!(got[i], want[i], "client {i}'s lines");
    }
}
