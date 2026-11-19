//! Hostile input on CoD2's client wire: `fixtures/mp_toujane-spectate.bin`'s
//! retail server messages, mutated, through `Client::server_message` and
//! `NetView::update` over a client that took the messages before them, and
//! its raw packets through `Client::receive`. Nothing may panic, and the
//! view's lists stay within the wire's bounds. No install needed.

use vcod_cod2::net::capture::{Capture, Dir, replay};
use vcod_cod2::net::{Client, NetView};
use vcod_common::net::huffman::Huffman;
use vcod_common::testing::fuzz;

const FIXTURE: &[u8] = include_bytes!("fixtures/mp_toujane-spectate.bin");

#[test]
fn hostile_server_messages_never_panic() {
    let rep = replay(&Capture::decode(FIXTURE).unwrap()).unwrap();
    let huff = Huffman::new();
    let messages = &rep.messages[..rep.messages.len().min(24)];
    for (i, (seq, msg)) in messages.iter().enumerate() {
        let mut c = Client::new(0, 0);
        let mut view = NetView::default();
        for (s, m) in &messages[..i] {
            c.netchan.incoming_sequence = *s as u32;
            if let Ok(p) = c.server_message(*s, m) {
                view.update(&c.parser, &p);
            }
        }
        let plain = huff.decompress_block(&msg[4..]);
        fuzz(&format!("cod2 message {seq}"), &plain, 60, |m| {
            let mut whole = msg[..4].to_vec();
            whole.extend(huff.compress_block(m));
            c.netchan.incoming_sequence = *seq as u32;
            if let Ok(p) = c.server_message(*seq, &whole) {
                view.update(&c.parser, &p);
            }
            // Entity and client numbers rise through a snapshot (10 and 6 bits).
            if let Some(s) = &view.snap {
                assert!(s.entities.len() <= 1024 && s.clients.len() <= 64);
            }
        });
    }
}

#[test]
fn hostile_server_packets_never_panic() {
    let cap = Capture::decode(FIXTURE).unwrap();
    let mut c = Client::new(0, 0);
    let packets = cap
        .packets
        .iter()
        .filter(|(d, p)| matches!(d, Dir::FromServer) && !p.starts_with(&[0xff; 4]));
    for (i, (_, p)) in packets.take(40).enumerate() {
        fuzz(&format!("cod2 packet {i}"), p, 50, |m| {
            let _ = c.receive(m);
        });
    }
}
