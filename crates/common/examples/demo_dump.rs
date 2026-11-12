//! A CoD1 demo file record by record (cod11-demos.md 2, 3): each record's
//! sequence, length and acknowledge, then its ops: server commands, the
//! gamestate's counts and serverinfo keys, a snapshot's time and delta byte.
//!
//! `cargo run -p vcod-common --example demo_dump -- <file.dm_N>`; the `N`
//! picks the protocol.

use vcod_common::net::demo::{self, DemoReader};
use vcod_common::net::gamestate;
use vcod_common::net::huffman::Huffman;
use vcod_common::net::info_value_for_key;
use vcod_common::net::msg::{
    MsgReader, SVC_EOF, SVC_GAMESTATE, SVC_NOP, SVC_SERVER_COMMAND, SVC_SNAPSHOT,
};

fn main() {
    let Some(path) = std::env::args().nth(1) else {
        eprintln!("usage: demo_dump <file.dm_N>");
        std::process::exit(2);
    };
    let p = demo::extension_of(&path)
        .and_then(demo::protocol_of_extension)
        .unwrap_or_else(|| panic!("{path}: not a .dm_1, .dm_2 or .dm_3"));
    let data = std::fs::read(&path).unwrap();
    println!(
        "{path}: {} bytes, protocol {}",
        data.len(),
        p.transport.version
    );
    let huff = Huffman::new();
    let mut r = DemoReader::new(data, p);
    let mut n = 0;
    loop {
        let (seq, msg) = match r.next_message() {
            Ok(m) => m,
            Err(end) => {
                println!("end after {n} records: {end:?}");
                break;
            }
        };
        n += 1;
        let ack = msg
            .get(..4)
            .map_or(-1, |b| i32::from_le_bytes(b.try_into().unwrap()));
        println!("record {n}: seq {seq} len {} ack {ack}", msg.len());
        let mut m = MsgReader::new(&msg[4.min(msg.len())..], &huff);
        while !m.is_overflowed() {
            match m.read_byte() {
                SVC_NOP => {}
                SVC_SERVER_COMMAND => {
                    let s = m.read_long();
                    let text = m.read_big_string();
                    println!("  cmd {s} {:?}", text.chars().take(100).collect::<String>());
                }
                SVC_GAMESTATE => {
                    match gamestate::parse_body(&mut m, p) {
                        Ok(gs) => {
                            let cs = &gs.configstrings;
                            println!(
                                "  gamestate: serverCommandSequence {}, {} configstrings, {} baselines, clientNum {}, checksumFeed {}",
                                gs.server_command_sequence,
                                cs.iter().filter(|s| !s.is_empty()).count(),
                                gs.baselines.len(),
                                gs.client_num,
                                gs.checksum_feed
                            );
                            for k in [
                                "sv_hostname",
                                "mapname",
                                "g_gametype",
                                "shortversion",
                                "protocol",
                            ] {
                                println!("    {k} {:?}", info_value_for_key(&cs[0], k));
                            }
                            println!("    systeminfo {:?}", cs[1]);
                            let mut types = std::collections::BTreeMap::new();
                            for b in gs.baselines.values() {
                                *types.entry(b.field_i32(p, "eType")).or_insert(0) += 1;
                            }
                            println!("    baseline eTypes {types:?}");
                        }
                        Err(e) => println!("  gamestate parse failed: {e}"),
                    }
                    let tail = m.read_byte();
                    println!("  then op {tail}");
                    break;
                }
                SVC_SNAPSHOT => {
                    let t = m.read_long();
                    let delta = m.read_byte();
                    println!("  snapshot serverTime {t} delta {delta}");
                    break;
                }
                SVC_EOF => break,
                op => {
                    println!("  op {op}");
                    break;
                }
            }
        }
    }
}
