//! Hostile input on the 1.7 and CoD4x 21 wires: committed captures' server
//! and client messages and side channel packets, mutated, through the
//! client's `CL_ParseServerMessage` walk, the side channel's readers and the
//! server's op reader. Nothing may panic. No install needed.

use vcod_cod4::net::capture::{self, Item, Session};
use vcod_cod4::net::clmsg::{self, UserCmd};
use vcod_cod4::net::huffman::Huffman;
use vcod_cod4::net::parse::Client;
use vcod_cod4::net::reliable::{Inbound, Packet, StatsMsg};
use vcod_cod4::net::wire::Dialect;
use vcod_common::testing::fuzz;

static FIXTURES: vcod_common::manifest::ManifestPath =
    vcod_common::manifest_const!("/tests/fixtures/net");

/// A capture's server messages (sequence and plain block) and client
/// messages (plain ops and usercmd key), in order.
#[allow(clippy::type_complexity)]
fn messages(name: &str) -> (Vec<(u32, Vec<u8>)>, Vec<(Vec<u8>, i32)>) {
    let text = std::fs::read_to_string(format!("{FIXTURES}/{name}")).unwrap();
    let huff = Huffman::new();
    let mut session = Session::new();
    let (mut server, mut client) = (Vec::new(), Vec::new());
    for l in &capture::parse(&text).unwrap() {
        match session.feed(l).unwrap() {
            Some(Item::Server { msg, .. }) => {
                server.push((msg.sequence, huff.decompress(&msg.block)));
            }
            Some(Item::Client { block, key, .. }) => client.push((huff.decompress(&block), key)),
            _ => {}
        }
    }
    (server, client)
}

/// The gamestate and the first snapshots of two fights, each message
/// mutated over a client that parsed the ones before it.
#[test]
fn hostile_server_messages_never_panic() {
    let huff = Huffman::new();
    for name in ["play-1.7-kill-shooter.txt", "play-1.7-rocket-target.txt"] {
        let (server, _) = messages(name);
        let server = &server[..server.len().min(24)];
        for (i, (seq, plain)) in server.iter().enumerate() {
            let mut client = Client::new();
            for (s, p) in &server[..i] {
                let _ = client.parse(*s, &huff.compress(p));
            }
            fuzz(&format!("{name} message {seq}"), plain, 60, |m| {
                let _ = client.parse(*seq, &huff.compress(m));
            });
        }
    }
}

/// A join's and a fight's client messages, mutated, through `read_ops`.
#[test]
fn hostile_client_messages_never_panic() {
    for name in ["handshake-1.7-cmds.txt", "play-1.7-kill-shooter.txt"] {
        let (_, client) = messages(name);
        for (i, (plain, key)) in client.iter().take(40).enumerate() {
            fuzz(&format!("{name} client message {i}"), plain, 50, |m| {
                let _ = clmsg::read_ops(m.to_vec(), *key, &UserCmd::default());
            });
        }
    }
}

/// CoD4x 21's side channel packets, its gamestate and stats messages, and
/// the first snapshots after the gamestate, each mutated.
#[test]
fn hostile_x21_messages_never_panic() {
    let name = "handshake-x21.txt";
    let text = std::fs::read_to_string(format!("{FIXTURES}/{name}")).unwrap();
    let huff = Huffman::new();
    let mut session = Session::with_dialect(Dialect::X21);
    let (mut packets, mut bodies, mut server) = (Vec::new(), Vec::new(), Vec::new());
    for l in &capture::parse(&text).unwrap() {
        match session.feed(l).unwrap() {
            Some(Item::Side {
                packet, messages, ..
            }) => {
                packets.push(packet.write());
                bodies.extend(messages);
            }
            Some(Item::Server { msg, .. }) => {
                server.push((msg.sequence, huff.decompress(&msg.block)));
            }
            _ => {}
        }
    }
    for (i, p) in packets.iter().take(12).enumerate() {
        fuzz(&format!("{name} side packet {i}"), p, 50, |m| {
            let _ = Packet::read(m).and_then(|p| Inbound::default().take(&p));
        });
    }
    let gamestate = bodies.iter().find(|b| b[..4] == [1, 0, 0, 0]).unwrap();
    fuzz(&format!("{name} gamestate"), gamestate, 60, |m| {
        let _ = Client::with_dialect(Dialect::X21).take_side_gamestate(m);
    });
    for (i, b) in bodies.iter().filter(|b| b[..4] == [9, 0, 0, 0]).enumerate() {
        fuzz(&format!("{name} stats {i}"), b, 50, |m| {
            let _ = StatsMsg::read(m);
        });
    }
    let mut client = Client::with_dialect(Dialect::X21);
    client.take_side_gamestate(gamestate).unwrap();
    let after: Vec<_> = server.iter().filter(|(s, _)| *s > 60).take(16).collect();
    for (i, (seq, plain)) in after.iter().enumerate() {
        for (s, p) in &after[..i] {
            let _ = client.parse(*s, &huff.compress(p));
        }
        fuzz(&format!("{name} message {seq}"), plain, 40, |m| {
            let _ = client.parse(*seq, &huff.compress(m));
        });
    }
}
