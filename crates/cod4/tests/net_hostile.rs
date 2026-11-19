//! Hostile input on the 1.7 wire: committed captures' server and client
//! messages, mutated, through the client's `CL_ParseServerMessage` walk and
//! the server's op reader. Nothing may panic. No install needed.

use vcod_cod4::net::capture::{self, Item, Session};
use vcod_cod4::net::clmsg::{self, UserCmd};
use vcod_cod4::net::huffman::Huffman;
use vcod_cod4::net::parse::Client;
use vcod_common::testing::fuzz;

const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/net");

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
            None => {}
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
