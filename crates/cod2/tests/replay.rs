//! Replays `fixtures/mp_toujane-spectate.bin`, a session against the retail
//! CoD2 1.3 `cod2_lnxded` on loopback (`examples/spectate.rs --fly --save
//! ... --save-messages 40`, with a second `--join --fly` client on the
//! server so a player entity moves): challenge, connect, a seven-fragment
//! gamestate, then `clc_move` snapshots deltaed from acknowledged frames.
//! Every server message must decode to its last bit: the block re-compressed
//! from the bytes the parse consumed equals what the server sent.

use vcod_cod2::net::Parser;
use vcod_cod2::net::capture::{Capture, Dir};
use vcod_cod2::net::oob::{self, Reply};
use vcod_cod2::net::ops::{CLC_BITS, Clc};
use vcod_cod2::net::usercmd::{UserCmd2, default_usercmd, read_delta_usercmd};
use vcod_common::net::codec::MsgReader;
use vcod_common::net::com_hash_key_bytes;
use vcod_common::net::huffman::Huffman;
use vcod_common::net::netchan::{Netchan, NetchanParams, scramble_client_ops};

const FIXTURE: &[u8] = include_bytes!("fixtures/mp_toujane-spectate.bin");

#[test]
fn retail_session_decodes_to_the_last_bit() {
    let cap = Capture::decode(FIXTURE).unwrap();
    let huff = Huffman::new();
    let mut challenge = None;
    let mut netchan: Option<Netchan> = None;
    let mut parser = Parser::new();
    let mut messages = 0;
    let mut snapshots = 0;
    let mut deltas = 0;
    let mut moves = 0;
    let mut player_origins = Vec::new();
    let mut own_origins = Vec::new();
    let mut cmd_times = Vec::new();

    for (dir, p) in &cap.packets {
        if let Some(pl) = oob::payload(p) {
            match dir {
                Dir::FromServer => match oob::parse_reply(pl) {
                    Reply::Challenge(c) => challenge = Some(c),
                    Reply::Connect => {}
                    r => panic!("unexpected reply {r:?}"),
                },
                Dir::ToServer => {
                    let text = String::from_utf8_lossy(pl);
                    let Some(ui) = text.strip_prefix("connect \"") else {
                        continue;
                    };
                    let ui = ui.trim_end_matches('"');
                    if let Some(q) = oob::info_value_for_key(ui, "qport") {
                        netchan = Some(Netchan::with_params(
                            q.parse().unwrap(),
                            challenge.unwrap(),
                            NetchanParams::COD2,
                        ));
                    }
                }
            }
            continue;
        }
        match dir {
            Dir::ToServer => {
                let nc = netchan.as_ref().unwrap();
                assert_eq!(u16::from_le_bytes([p[4], p[5]]), nc.qport);
                // SV_Netchan_Decode: the server's key over our block.
                let server_id = p[6];
                let msg_ack = i32::from_le_bytes(p[7..11].try_into().unwrap());
                let rel_ack = i32::from_le_bytes(p[11..15].try_into().unwrap());
                let command = parser.server_command(rel_ack).to_vec();
                let mut block = p[15..].to_vec();
                scramble_client_ops(
                    (nc.challenge ^ i32::from(server_id) ^ msg_ack) as u8,
                    &command,
                    &mut block,
                );
                let plain = huff.decompress_block(&block);
                let mut r = MsgReader::from_plain(plain);
                let op = r.read_bits(CLC_BITS) as u32;
                if op == Clc::Move as u32 || op == Clc::MoveNoDelta as u32 {
                    moves += 1;
                    let n = r.read_byte();
                    let gs = parser.gamestate.as_ref().unwrap();
                    let key =
                        (gs.checksum_feed ^ msg_ack ^ com_hash_key_bytes(&command, 32)) as u32;
                    let mut from = parser
                        .snapshot()
                        .map(|s| default_usercmd(&s.ps))
                        .unwrap_or_default();
                    for _ in 0..n {
                        let c: UserCmd2 = read_delta_usercmd(&mut r, key, &from);
                        cmd_times.push(c.server_time);
                        assert_eq!(c.forward_move, 127);
                        from = c;
                    }
                    assert_eq!(r.read_bits(CLC_BITS), Clc::Eof as i32);
                } else {
                    assert_eq!(op, Clc::Eof as u32);
                }
                assert!(!r.is_overflowed());
            }
            Dir::FromServer => {
                let nc = netchan.as_mut().unwrap();
                let Some((sequence, mut body)) = nc.frame(p).unwrap() else {
                    continue;
                };
                // The spectator sends no client commands, so the key string
                // is empty.
                nc.decode(sequence, b"", &mut body);
                let block = &body[4..];
                let plain = huff.decompress_block(block);
                let parsed = parser.parse(sequence as i32, &plain).unwrap();
                assert_eq!(
                    huff.compress_block(&plain[..parsed.consumed]),
                    block,
                    "message {} left bits over",
                    sequence
                );
                messages += 1;
                if let Some(n) = parsed.snapshot {
                    snapshots += 1;
                    let f = parser.frame(n).unwrap();
                    assert!(f.valid);
                    if f.delta_num >= 0 {
                        deltas += 1;
                    }
                    own_origins.push(f.ps.origin());
                    if let Some(e) = f.entities.iter().find(|e| e.number == 0) {
                        assert_eq!(e.i32("eType"), 1);
                        player_origins.push(e.origin());
                    }
                }
            }
        }
    }

    let gs = parser.gamestate.as_ref().unwrap();
    assert_eq!(gs.configstrings.len(), 282);
    assert_eq!(gs.baselines.len(), 38);
    assert_eq!(gs.configstring(2), "cod");
    assert_eq!(gs.server_id(), Some(16));
    assert_eq!(gs.client_num, 1);
    assert_eq!(messages, 40);
    // 40 messages: 16 bare ones before the gamestate, the gamestate, 23
    // snapshots of which only the first is not a delta.
    assert_eq!(snapshots, 23);
    assert_eq!(deltas, 22);
    assert_eq!(moves, 30);
    assert!(cmd_times.windows(2).all(|w| w[0] <= w[1]));

    let last = parser.snapshot().unwrap();
    assert_eq!(last.snap_flags, 4);
    assert_eq!(last.entities.len(), 28);
    assert_eq!(last.clients.len(), 2);
    assert_eq!(last.ps.i32("pm_type"), 4);
    assert_eq!(last.ps.i32("speed"), 400);
    // Both bodies moved across the delta frames: the spectator's own
    // playerstate and the other client's entity.
    assert_eq!(player_origins.len(), snapshots);
    assert_ne!(player_origins.first(), player_origins.last());
    assert_ne!(own_origins.first(), own_origins.last());
}
