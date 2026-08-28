//! What a retail server puts in `serverTime` and `ps.commandTime`, frame by
//! frame, out of the committed captures. The two fields are the contract that
//! drives a client's prediction: it replays every usercmd whose serverTime is
//! past `commandTime`, so their relationship decides whether prediction runs
//! at all.
//!
//! `cargo run -p vcod-common --example snapshot_timing`

use std::collections::HashMap;

use vcod_common::net::gamestate;
use vcod_common::net::huffman::Huffman;
use vcod_common::net::msg::MsgReader;
use vcod_common::net::protocol::{Protocol, PROTOCOL_V1};
use vcod_common::net::snapshot::{
    SnapshotRing, SVC_EOF, SVC_NOP, SVC_SERVER_COMMAND, SVC_SNAPSHOT,
};

fn main() {
    for (gs_name, snap_name) in [
        ("net/gamestate.bin", "net/snapshots.bin"),
        ("net/gamestate-delta.bin", "net/snapshots-delta.bin"),
    ] {
        println!("== {snap_name}");
        report(gs_name, snap_name, &PROTOCOL_V1);
        println!();
    }
}

fn report(gs_name: &str, snap_name: &str, p: &Protocol) {
    let h = Huffman::new();

    let gs_data = vcod_common::testing::fixture(gs_name);
    let mut gr = MsgReader::new(&gs_data[4..], &h);
    let gs = gamestate::parse(&mut gr, p).unwrap();

    let mut ring = SnapshotRing::new();
    ring.set_baselines(gs.baselines.clone());

    let data = vcod_common::testing::fixture(snap_name);
    let mut off = 0usize;
    let mut rows: Vec<(u32, i32, i32, i32)> = Vec::new();
    let mut last_ps: Option<vcod_common::net::msg::PlayerState> = None;
    let mut angles: Vec<(u32, f32, f32, i32, i32)> = Vec::new();

    while off + 8 <= data.len() {
        let message_num = u32::from_le_bytes(data[off..off + 4].try_into().unwrap());
        let len = u32::from_le_bytes(data[off + 4..off + 8].try_into().unwrap()) as usize;
        off += 8;
        let msg = &data[off..off + len];
        off += len;

        let mut r = MsgReader::new(&msg[4..], &h);
        loop {
            if r.is_overflowed() {
                break;
            }
            let op = r.read_byte();
            if op == SVC_EOF {
                break;
            }
            match op {
                SVC_NOP => {}
                SVC_SERVER_COMMAND => {
                    r.read_long();
                    r.read_big_string();
                }
                SVC_SNAPSHOT => {
                    let snap = ring.parse_into(&mut r, p, message_num).unwrap().clone();
                    last_ps = Some(snap.ps.clone());
                    let ct = snap.ps.field_i32(p, "commandTime");
                    if angles.len() < 8 {
                        angles.push((
                            snap.message_num,
                            snap.ps.field_f32(p, "viewangles[0]"),
                            snap.ps.field_f32(p, "viewangles[1]"),
                            snap.ps.field_i32(p, "delta_angles[0]"),
                            snap.ps.field_i32(p, "delta_angles[1]"),
                        ));
                    }
                    rows.push((
                        snap.message_num,
                        snap.server_time,
                        ct,
                        snap.server_time - ct,
                    ));
                    break;
                }
                _ => break,
            }
        }
    }

    if rows.is_empty() {
        println!("  no snapshots");
        return;
    }

    println!("  {} frames", rows.len());
    println!(
        "  {:>8} {:>12} {:>12} {:>8}",
        "msg", "serverTime", "commandTime", "lead"
    );
    for row in rows.iter().take(6) {
        println!("  {:>8} {:>12} {:>12} {:>8}", row.0, row.1, row.2, row.3);
    }
    if rows.len() > 12 {
        println!("  {:>8} {:>12} {:>12} {:>8}", "...", "...", "...", "...");
        for row in rows.iter().skip(rows.len() - 6) {
            println!("  {:>8} {:>12} {:>12} {:>8}", row.0, row.1, row.2, row.3);
        }
    }

    // The lead is what decides prediction: serverTime - commandTime. A client
    // replays cmds newer than commandTime, so a lead at or below zero means
    // the server claims to have simulated up to (or past) its own frame clock.
    let leads: Vec<i32> = rows.iter().map(|r| r.3).collect();
    let lo = *leads.iter().min().unwrap();
    let hi = *leads.iter().max().unwrap();
    let mean = leads.iter().map(|&v| i64::from(v)).sum::<i64>() / leads.len() as i64;
    println!("  lead (serverTime - commandTime): min {lo}, max {hi}, mean {mean}");

    let mut hist: HashMap<i32, usize> = HashMap::new();
    for &l in &leads {
        *hist.entry(l).or_default() += 1;
    }
    let mut counts: Vec<(i32, usize)> = hist.into_iter().collect();
    counts.sort_by_key(|&(_, n)| std::cmp::Reverse(n));
    let top: Vec<String> = counts
        .iter()
        .take(6)
        .map(|(l, n)| format!("{l}:{n}"))
        .collect();
    println!("  most common leads: {}", top.join(" "));

    println!("  view/delta angles, first frames:");
    println!(
        "  {:>8} {:>12} {:>12} {:>14} {:>14}",
        "msg", "viewang[0]", "viewang[1]", "delta_ang[0]", "delta_ang[1]"
    );
    for a in &angles {
        println!(
            "  {:>8} {:>12} {:>12} {:>14} {:>14}",
            a.0, a.1, a.2, a.3, a.4
        );
    }

    // Every non-zero playerstate field of the last frame: this capture is a
    // lone spectator, so it is the oracle for what our own to_wire should
    // produce (crates/server/src/spectate.rs).
    if let Some(ps) = &last_ps {
        println!("  non-zero playerstate fields, last frame:");
        for (i, f) in p.player_fields.iter().enumerate() {
            let v = ps.fields[i];
            if v == 0 {
                continue;
            }
            if f.bits == 0 {
                println!(
                    "    {:<22} {:>12}  (f32 {})",
                    f.name,
                    v,
                    f32::from_bits(v as u32)
                );
            } else {
                println!("    {:<22} {:>12}", f.name, v);
            }
        }
    }

    // Does commandTime advance in step with serverTime, or lag it unevenly?
    let ct_steps: Vec<i32> = rows.windows(2).map(|w| w[1].2 - w[0].2).collect();
    let st_steps: Vec<i32> = rows.windows(2).map(|w| w[1].1 - w[0].1).collect();
    if !ct_steps.is_empty() {
        let ct_zero = ct_steps.iter().filter(|&&s| s == 0).count();
        println!(
            "  serverTime step: min {} max {}   commandTime step: min {} max {}, {} frames with no advance",
            st_steps.iter().min().unwrap(),
            st_steps.iter().max().unwrap(),
            ct_steps.iter().min().unwrap(),
            ct_steps.iter().max().unwrap(),
            ct_zero,
        );
    }
}
