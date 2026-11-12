//! Prints a CoD4 1.7 `.dm_1` record by record (docs/research/cod4-demos.md
//! 2): each message's sequence, length and ops, each archive row with
//! `--rows`, the opening gamestate's host, map and table size, and how the
//! file ends.
//!
//!     cargo run -p vcod-cod4 --example demo_dump -- FILE.dm_1 [--rows]

use anyhow::{Context, Result};
use vcod_cod4::net::demo::{DemoReader, Record};
use vcod_cod4::net::parse::{Client, Op};

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let path = args.get(1).context("usage: FILE.dm_1 [--rows]")?;
    let rows = args.iter().any(|a| a == "--rows");
    let data = std::fs::read(path)?;
    println!("{path}: {} bytes", data.len());
    let mut rd = DemoReader::new(data);
    let mut client = Client::new();
    let (mut messages, mut archive) = (0, 0);
    let end = loop {
        match rd.next_record() {
            Ok(Record::Archive(i, a)) => {
                archive += 1;
                if rows {
                    println!("row {i}: {a:?}");
                }
            }
            Ok(Record::Message(seq, msg)) => {
                messages += 1;
                let ack = msg
                    .get(..4)
                    .map(|b| i32::from_le_bytes(b.try_into().unwrap()));
                let ops = match msg.get(4..).map(|b| client.parse(seq, b)) {
                    Some(Ok(p)) => {
                        let names: Vec<&str> = p
                            .ops
                            .iter()
                            .map(|o| match o {
                                Op::Gamestate => "gamestate",
                                Op::Snapshot(_) => "snapshot",
                                Op::ServerCommand { .. } => "command",
                                _ => "other",
                            })
                            .collect();
                        format!("{names:?} exact {}", p.exact)
                    }
                    Some(Err(e)) => format!("parse error: {e:#}"),
                    None => "short".into(),
                };
                println!("message {seq}: {} bytes, ack {ack:?}, {ops}", msg.len());
                if let (1, Some(gs)) = (messages, &client.gamestate) {
                    let s = |k: &str| gs.systeminfo(k).unwrap_or_default();
                    println!(
                        "  gamestate: {} slots held, {} baselines, clientNum {}, checksumFeed {}, sv_serverid {}",
                        gs.configstrings.len(),
                        gs.baselines.len(),
                        gs.client_num,
                        gs.checksum_feed,
                        s("sv_serverid"),
                    );
                    for i in 0..4 {
                        let cs = String::from_utf8_lossy(gs.configstring(i));
                        println!("  cs {i}: {}", cs.chars().take(300).collect::<String>());
                    }
                }
            }
            Err(e) => break e,
        }
    };
    println!(
        "{messages} messages, {archive} archive rows, {} snapshots, end {end:?}",
        client.snapshots.len()
    );
    Ok(())
}
