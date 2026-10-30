//! Prints a CoD4 1.7 capture as the client lived it: each client command,
//! each server command, each configstring the gamestate carried (with
//! `--cs`), and per snapshot the playerstate fields that changed (with
//! `--ps`) and the entities that entered or left; with `--hud` the
//! playerstate's hudelems each time they change.
//!
//!     cargo run -p vcod-cod4 --example play_dump -- CAPTURE.txt [--cs] [--ps] [--hud] [--ent N] [--times]
//!
//! `--ent N` prints entity N's non-zero fields in every snapshot carrying it.
//! `--times` prints every snapshot's `serverTime` (the `sv_fps` captures,
//! cod4-server-frame.md 8).

use anyhow::{Context, Result};
use std::collections::BTreeSet;
use vcod_cod4::net::capture::{self, Item, Session};
use vcod_cod4::net::clmsg::ClOp;
use vcod_cod4::net::download::DownloadMsg;
use vcod_cod4::net::fields::PLAYER_STATE;
use vcod_cod4::net::parse::Op;
use vcod_cod4::net::pure::block_checksum_key32;

/// An entity's non-zero fields by its type's table.
fn ent_fields(e: &vcod_cod4::net::delta::EntityState) -> String {
    vcod_cod4::net::delta::EntityState::table(e.etype())
        .iter()
        .filter_map(|f| {
            let v = e.0[f.word()];
            (v != 0).then(|| format!("{}={v:#x}", f.name))
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let path = args.get(1).context("usage: CAPTURE.txt [--cs] [--ps]")?;
    let cs = args.iter().any(|a| a == "--cs");
    let show_ps = args.iter().any(|a| a == "--ps");
    let show_hud = args.iter().any(|a| a == "--hud");
    let times = args.iter().any(|a| a == "--times");
    let mut last_hud: Vec<vcod_cod4::hud::HudElem> = Vec::new();
    // `--full N`: every non-zero playerstate field of message N.
    let full: Option<u32> = args
        .iter()
        .position(|a| a == "--full")
        .and_then(|i| args.get(i + 1)?.parse().ok());
    let ent: Option<u32> = args
        .iter()
        .position(|a| a == "--ent")
        .and_then(|i| args.get(i + 1)?.parse().ok());
    let lines = capture::parse(&std::fs::read_to_string(path)?)?;
    let mut session = Session::new();
    let mut last_ps: Option<Vec<u32>> = None;
    let mut last_stats = [0i32; 5];
    let mut last_ents: BTreeSet<(u32, u32)> = BTreeSet::new();
    for l in &lines {
        if l.data.starts_with(&[0xff; 4]) {
            let text = String::from_utf8_lossy(&l.data[4..]);
            if !text.starts_with("stats") {
                println!(
                    "{} {} oob {:?}",
                    l.ms,
                    l.dir,
                    text.chars().take(120).collect::<String>()
                );
            }
        }
        match session.feed(l)? {
            Some(Item::Client { msg, .. }) => {
                for op in &msg.ops {
                    if let ClOp::Command { sequence, text } = op {
                        println!(
                            "{} c2s cmd {sequence} {:?}",
                            l.ms,
                            String::from_utf8_lossy(text)
                        );
                    }
                }
            }
            Some(Item::Server { msg, parsed }) => {
                for op in &parsed.ops {
                    match op {
                        Op::ServerCommand { sequence, text } => {
                            let t = String::from_utf8_lossy(text);
                            println!(
                                "{} s2c msg {} scmd {sequence} {:?}",
                                l.ms,
                                msg.sequence,
                                t.chars().take(160).collect::<String>()
                            );
                        }
                        Op::Download(d) => {
                            let d = match d {
                                DownloadMsg::Block { block, size, data } => format!(
                                    "block {block} size {size:?} len {} crc32 {:08x}",
                                    data.len(),
                                    block_checksum_key32(data, 0)
                                ),
                                other => format!("{other:?}"),
                            };
                            println!("{} s2c msg {} download {d}", l.ms, msg.sequence);
                        }
                        Op::Gamestate => {
                            let gs = session.client.gamestate.as_ref().unwrap();
                            println!(
                                "{} s2c msg {} gamestate seq {} client {} baselines {:?}",
                                l.ms,
                                msg.sequence,
                                gs.server_command_sequence,
                                gs.client_num,
                                gs.baselines.keys().collect::<Vec<_>>()
                            );
                            if args.iter().any(|a| a == "--baselines") {
                                for e in gs.baselines.values() {
                                    println!("  baseline {} {}", e.number(), ent_fields(e));
                                }
                            }
                            if cs {
                                for (i, s) in &gs.configstrings {
                                    println!("  cs {i} {:?}", String::from_utf8_lossy(s));
                                }
                            }
                        }
                        Op::Snapshot(i) => {
                            let s = &session.client.snapshots[*i];
                            if times {
                                println!(
                                    "{} s2c msg {} t {} snap",
                                    l.ms, msg.sequence, s.server_time
                                );
                            }
                            let ents: BTreeSet<(u32, u32)> =
                                s.entities.iter().map(|e| (e.number(), e.etype())).collect();
                            let came: Vec<_> = ents.difference(&last_ents).collect();
                            let went: Vec<_> = last_ents.difference(&ents).collect();
                            if !came.is_empty() || !went.is_empty() {
                                println!(
                                    "{} s2c msg {} t {} ents +{:?} -{:?}",
                                    l.ms, msg.sequence, s.server_time, came, went
                                );
                            }
                            last_ents = ents;
                            if let Some(e) =
                                ent.and_then(|n| s.entities.iter().find(|e| e.number() == n))
                            {
                                println!(
                                    "{} s2c msg {} t {} ent {} {}",
                                    l.ms,
                                    msg.sequence,
                                    s.server_time,
                                    e.number(),
                                    ent_fields(e)
                                );
                            }
                            if full == Some(msg.sequence) {
                                for f in PLAYER_STATE {
                                    let v = s.ps.words[f.word()];
                                    if v != 0 {
                                        println!("  {} = {v:#x} ({})", f.name, f32::from_bits(v));
                                    }
                                }
                                println!("  stats {:?}", s.ps.stats);
                                let nz = |a: &[i32]| -> Vec<(usize, i32)> {
                                    a.iter()
                                        .enumerate()
                                        .filter(|(_, v)| **v != 0)
                                        .map(|(i, v)| (i, *v))
                                        .collect()
                                };
                                println!("  ammo {:?}", nz(&s.ps.ammo));
                                println!("  ammoclip {:?}", nz(&s.ps.ammoclip));
                                println!(
                                    "  weapon_models {:?}",
                                    s.ps.weapon_models
                                        .iter()
                                        .enumerate()
                                        .filter(|(_, v)| **v != 0)
                                        .collect::<Vec<_>>()
                                );
                                for e in &s.entities {
                                    println!("  ent {} {}", e.number(), ent_fields(e));
                                }
                                for c in &s.clients {
                                    let f: Vec<String> = vcod_cod4::net::fields::CLIENT_STATE
                                        .iter()
                                        .filter(|f| c.0[f.word()] != 0)
                                        .map(|f| format!("{}={:#x}", f.name, c.0[f.word()]))
                                        .collect();
                                    println!("  client {} {}", c.0[0], f.join(" "));
                                }
                            }
                            let hud = vcod_cod4::hud::hud_elems(&s.ps);
                            if show_hud && hud != last_hud {
                                println!(
                                    "{} s2c msg {} t {} hud",
                                    l.ms, msg.sequence, s.server_time
                                );
                                for e in &hud {
                                    println!("  {e:?}");
                                }
                                last_hud = hud;
                            }
                            if show_ps {
                                let w = &s.ps.words;
                                let mut diff = Vec::new();
                                for f in PLAYER_STATE {
                                    let v = w[f.word()];
                                    let was = last_ps.as_ref().map(|p| p[f.word()]);
                                    if was != Some(v) && f.name != "commandTime" {
                                        diff.push(format!("{}={:#x}", f.name, v));
                                    }
                                }
                                if s.ps.stats != last_stats {
                                    diff.push(format!("stats={:?}", s.ps.stats));
                                    last_stats = s.ps.stats;
                                }
                                if !diff.is_empty() {
                                    println!(
                                        "{} s2c msg {} t {} ps {}",
                                        l.ms,
                                        msg.sequence,
                                        s.server_time,
                                        diff.join(" ")
                                    );
                                }
                                last_ps = Some(w.clone());
                            }
                        }
                        Op::Nop => {}
                    }
                }
            }
            None => {}
        }
    }
    Ok(())
}
