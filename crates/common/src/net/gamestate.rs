//! Contains routines ported from the RTCW-MP GPL source, Copyright (C) 1999-2010 id Software LLC, a ZeniMax Media company.
//! See NOTICE.
//!
//! `CL_ParseGamestate` (RTCW-MP/src/client/cl_parse.c): the configstring table
//! and entity baselines the server hands over right after `connectResponse`.

use super::msg::{
    read_delta_entity, write_delta_entity, EntityState, MsgReader, MsgWriter, SVC_BASELINE,
    SVC_CONFIGSTRING, SVC_EOF, SVC_GAMESTATE, SVC_SERVER_COMMAND,
};
use super::protocol::{Protocol, GENTITYNUM_BITS, MAX_GENTITIES};
use std::collections::HashMap;

#[derive(Debug, Default)]
pub struct Gamestate {
    /// Indexed by configstring number; unset slots are empty.
    pub configstrings: Vec<String>,
    /// Sparse: only entities the server sent a baseline for.
    pub baselines: HashMap<u32, EntityState>,
    pub client_num: i32,
    pub checksum_feed: i32,
    pub server_command_sequence: i32,
}

/// Parse from the `svc_gamestate` op byte. The caller has stripped the plain
/// `reliableAcknowledge` (docs/protocol-1.1.md, message headers); leading
/// `svc_serverCommand` ops are skipped here. [`parse_body`] takes over once
/// the op byte is consumed.
pub fn parse(r: &mut MsgReader, p: &Protocol) -> anyhow::Result<Gamestate> {
    loop {
        match r.read_byte() {
            SVC_GAMESTATE => return parse_body(r, p),
            SVC_SERVER_COMMAND => {
                r.read_long();
                r.read_string();
            }
            cmd => anyhow::bail!("expected svc_gamestate, got op {cmd}"),
        }
        anyhow::ensure!(!r.is_overflowed(), "message ended before svc_gamestate");
    }
}

/// `CL_ParseGamestate`, with the op byte already consumed.
pub fn parse_body(r: &mut MsgReader, p: &Protocol) -> anyhow::Result<Gamestate> {
    let mut gs = Gamestate {
        configstrings: vec![String::new(); p.max_configstrings],
        server_command_sequence: r.read_long(),
        ..Default::default()
    };
    let null = EntityState::null(p);

    loop {
        anyhow::ensure!(!r.is_overflowed(), "gamestate truncated");
        match r.read_byte() {
            SVC_EOF => break,
            SVC_CONFIGSTRING => {
                let i = r.read_short();
                let i = usize::try_from(i)
                    .ok()
                    .filter(|&i| i < p.max_configstrings)
                    .ok_or_else(|| anyhow::anyhow!("configstring index {i} out of range"))?;
                gs.configstrings[i] = r.read_big_string();
            }
            SVC_BASELINE => {
                let num = r.read_bits(GENTITYNUM_BITS as i32) as u32;
                anyhow::ensure!(
                    (num as usize) < MAX_GENTITIES,
                    "baseline number {num} out of range"
                );
                if let Some(es) = read_delta_entity(r, p, &null, num) {
                    gs.baselines.insert(num, es);
                }
            }
            cmd => anyhow::bail!("bad command byte {cmd} at bit {}", r.bits_read()),
        }
    }

    gs.client_num = r.read_long();
    gs.checksum_feed = r.read_long();
    anyhow::ensure!(
        !r.is_overflowed(),
        "gamestate truncated at the trailing longs"
    );
    Ok(gs)
}

/// `SV_SendClientGameState` from the op on, inverse of [`parse_body`]. The
/// caller writes the plain `reliableAcknowledge` and any leading server
/// commands.
pub fn write(w: &mut MsgWriter, p: &Protocol, gs: &Gamestate) {
    w.write_byte(SVC_GAMESTATE);
    w.write_long(gs.server_command_sequence);
    for (i, cs) in gs.configstrings.iter().enumerate() {
        if cs.is_empty() {
            continue;
        }
        w.write_byte(SVC_CONFIGSTRING);
        w.write_short(i as i16);
        w.write_big_string(cs);
    }
    let null = EntityState::null(p);
    let mut nums: Vec<u32> = gs.baselines.keys().copied().collect();
    nums.sort_unstable();
    for n in nums {
        w.write_byte(SVC_BASELINE);
        w.write_bits(n as i32, GENTITYNUM_BITS as i32);
        write_delta_entity(w, p, &null, Some(&gs.baselines[&n]));
    }
    w.write_byte(SVC_EOF);
    w.write_long(gs.client_num);
    w.write_long(gs.checksum_feed);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::{
        huffman::Huffman,
        msg::{MsgReader, MsgWriter},
        protocol::PROTOCOL_V1,
    };

    #[test]
    fn parses_captured_gamestate() {
        let data = crate::testing::fixture("net/gamestate.bin");
        let h = Huffman::new();
        // Plain LE reliableAcknowledge, then the huffman block (protocol doc,
        // divergence 4).
        let reliable_ack = i32::from_le_bytes(data[..4].try_into().unwrap());
        assert_eq!(reliable_ack, 0);
        let mut r = MsgReader::new(&data[4..], &h);
        let gs = parse(&mut r, &PROTOCOL_V1).unwrap();
        // cs[0] serverinfo, cs[1] systeminfo (codextended sv_client.c:319).
        assert!(gs.configstrings[0].contains("mapname"));
        assert!(gs.configstrings[1].contains("sv_serverid"));
        let joined = gs.configstrings.join("\n");
        assert!(joined.contains("mp_carentan"));
        assert!(gs.client_num >= 0 && gs.client_num < 64);
        assert!(!gs.baselines.is_empty());
        for e in gs.baselines.values() {
            let o = e.origin(&PROTOCOL_V1);
            assert!(o.iter().all(|c| c.abs() < 65536.0), "origin {o:?}");
        }
        assert!(!r.is_overflowed());

        // Exact counts and the finishing position prove the reader stayed in
        // step; a desynced delta reader still lands somewhere plausible.
        assert_eq!(
            gs.configstrings.iter().filter(|s| !s.is_empty()).count(),
            246
        );
        assert_eq!(gs.baselines.len(), 19);
        assert_eq!(gs.client_num, 0);
        assert_eq!(gs.checksum_feed, 0x2b07_d805);
        assert_eq!(gs.server_command_sequence, 0);
        // Up to the svc_EOF that SV_Netchan_Transmit appends; one decompressor
        // pad symbol follows it.
        assert_eq!(r.bits_read(), 57640);
        // one of the map's own entities
        let e = &gs.baselines[&73];
        assert_eq!(e.origin(&PROTOCOL_V1), [360.0, -928.0, 128.0]);
        assert_eq!(e.field_i32(&PROTOCOL_V1, "eType"), 8);
    }

    /// Round trip of the retail capture. The body is `svc_serverCommand 0 ""`
    /// then the gamestate; the capture then carries the netchan `svc_EOF` and
    /// one decompressor pad byte (protocol doc, svc_gamestate).
    #[test]
    fn writer_reproduces_the_captured_gamestate_byte_for_byte() {
        let data = crate::testing::fixture("net/gamestate.bin");
        let h = Huffman::new();
        let plain = h.decompress_block(&data[4..]);
        let mut r = MsgReader::new(&data[4..], &h);
        let gs = parse(&mut r, &PROTOCOL_V1).unwrap();

        let mut w = MsgWriter::new(&h);
        crate::net::msg::write_server_command(&mut w, 0, "");
        write(&mut w, &PROTOCOL_V1, &gs);
        let ops = w.into_ops();
        assert_eq!(
            ops.len() * 8,
            r.bits_read(),
            "wrote {} bytes, the capture's message is {}",
            ops.len(),
            r.bits_read() / 8
        );
        if let Some(i) = ops.iter().zip(&plain).position(|(a, b)| a != b) {
            panic!(
                "first difference at byte {i}: wrote {:#04x}, capture {:#04x}",
                ops[i], plain[i]
            );
        }
        // netchan terminator, then the decompressor's pad symbol
        assert_eq!(plain[ops.len()], SVC_EOF, "svc_EOF after the gamestate");
        assert_eq!(plain.len(), ops.len() + 2);

        // the recompressed message parses back to the same thing
        let mut r2 = MsgReader::new(&h.compress_block(&ops), &h);
        let gs2 = parse(&mut r2, &PROTOCOL_V1).unwrap();
        assert_eq!(gs2.configstrings, gs.configstrings);
        assert_eq!(gs2.baselines, gs.baselines);
        assert_eq!(
            (gs2.client_num, gs2.checksum_feed),
            (gs.client_num, gs.checksum_feed)
        );
    }
}
