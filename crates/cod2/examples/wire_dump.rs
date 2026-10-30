//! A capture (`spectate.rs --save`) as text: the gamestate's configstrings
//! and baselines, the server commands in order, and each snapshot's
//! playerstate and entities with every non-zero field, for diffing two
//! servers' wire by eye (cod2-protocol.md 15).
//!
//!     cargo run -p vcod-cod2 --example wire_dump -- capture.bin [--snaps N]

use anyhow::Context;
use vcod_cod2::net::capture::{Capture, replay};
use vcod_cod2::net::fields::{ENTITY_STATE_FIELDS, NetField, PLAYER_STATE_FIELDS};

fn fields(table: &[NetField], words: &[u32]) -> String {
    table
        .iter()
        .zip(words)
        .filter(|(_, w)| **w != 0)
        .map(|(f, w)| {
            if matches!(f.bits, 0 | -99 | -100) {
                format!("{}={}", f.name, f32::from_bits(*w))
            } else {
                format!("{}={}", f.name, *w as i32)
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let path = args.first().context("usage: wire_dump <capture.bin>")?;
    let snaps: usize = args
        .iter()
        .position(|a| a == "--snaps")
        .and_then(|i| args.get(i + 1))
        .map_or(Ok(5), |s| s.parse())?;
    let cap = Capture::decode(&std::fs::read(path)?)?;
    let r = replay(&cap)?;
    let gs = r.parser.gamestate.as_ref().context("no gamestate")?;
    println!("clientNum {}", gs.client_num);
    for (i, s) in &gs.configstrings {
        println!("cs {i} {s:?}");
    }
    for (n, b) in &gs.baselines {
        println!("baseline {n} {}", fields(&ENTITY_STATE_FIELDS, &b.f));
    }
    for c in &r.commands {
        println!("cmd {c:?}");
    }
    for s in r.snapshots.iter().take(snaps) {
        println!(
            "snap {} time {} flags {} ents {}",
            s.message_num,
            s.server_time,
            s.snap_flags,
            s.entities.len()
        );
        println!("  ps {}", fields(&PLAYER_STATE_FIELDS, &s.ps.f));
        println!("  stats {:?}", s.ps.stats);
        let slots = |a: &[i32; 64]| {
            a.iter()
                .enumerate()
                .filter(|(_, v)| **v != 0)
                .map(|(i, v)| format!("{i}:{v}"))
                .collect::<Vec<_>>()
                .join(" ")
        };
        println!("  ammo {}", slots(&s.ps.ammo));
        println!("  ammoclip {}", slots(&s.ps.ammoclip));
        for e in &s.entities {
            println!("  ent {} {}", e.number, fields(&ENTITY_STATE_FIELDS, &e.f));
        }
        for c in &s.clients {
            println!("  client {} {:?}", c.number, c.name());
        }
    }
    Ok(())
}
