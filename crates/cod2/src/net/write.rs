//! The server's half of CoD2's wire: the delta writers `parse` and `delta`
//! read back (docs/research/cod2-protocol.md 3.2, 3.3, 5.3), and the
//! gamestate and snapshot bodies built on them. A retail capture's messages
//! re-encode byte for byte through these (`tests::retail_*`).

use vcod_common::net::codec::{MsgWriter, write_delta_field, write_delta_struct, write_ps_value};

use super::delta::{ClientState2, EntityState2, HUD_FIELDS, HudElem2, PlayerState2, field_index};
use super::fields::{
    CLIENT_STATE_FIELDS, ENTITY_STATE_FIELDS, HUDELEM_FIELDS, OBJECTIVE_FIELDS, PLAYER_STATE_FIELDS,
};
use super::ops::{CLIENTNUM_BITS, ENTITYNUM_NONE, GENTITYNUM_BITS, Svc};
use super::parse::Gamestate2;

fn words(f: &[u32]) -> Vec<i32> {
    f.iter().map(|&v| v as i32).collect()
}

/// `MSG_WriteDeltaEntity`: the 10-bit number, then the struct delta of `to`
/// against `from`; `None` removes the entity. An unchanged entity writes
/// nothing unless `force` (a baseline, or one new to the frame).
pub fn write_delta_entity(
    w: &mut MsgWriter,
    from: &EntityState2,
    to: Option<&EntityState2>,
    force: bool,
) {
    let Some(to) = to else {
        w.write_bits(from.number as i32, GENTITYNUM_BITS);
        write_delta_struct(w, &ENTITY_STATE_FIELDS, &words(&from.f), None);
        return;
    };
    if !force && from.f == to.f {
        return;
    }
    w.write_bits(to.number as i32, GENTITYNUM_BITS);
    write_delta_struct(
        w,
        &ENTITY_STATE_FIELDS,
        &words(&from.f),
        Some(&words(&to.f)),
    );
}

/// `MSG_WriteDeltaClient` (0x80698e8): a set bit and the 6-bit number ahead
/// of the struct delta; [`write_delta_entity`]'s rules otherwise.
pub fn write_delta_client(
    w: &mut MsgWriter,
    from: &ClientState2,
    to: Option<&ClientState2>,
    force: bool,
) {
    let Some(to) = to else {
        w.write_bits(1, 1);
        w.write_bits(from.number as i32, CLIENTNUM_BITS);
        write_delta_struct(w, &CLIENT_STATE_FIELDS, &words(&from.f), None);
        return;
    };
    if !force && from.f == to.f {
        return;
    }
    w.write_bits(1, 1);
    w.write_bits(to.number as i32, CLIENTNUM_BITS);
    write_delta_struct(
        w,
        &CLIENT_STATE_FIELDS,
        &words(&from.f),
        Some(&words(&to.f)),
    );
}

/// `stats[]`'s widths in mask order: shorts but `stats[3]` 6 bits and
/// `stats[5]` a byte (5.3).
fn write_stat(w: &mut MsgWriter, i: usize, v: i32) {
    match i {
        3 => w.write_bits(v, 6),
        5 => w.write_byte(v as u8),
        _ => w.write_short(v as i16),
    }
}

/// Four sub-blocks of 16: a gate, a 16-bit changed mask, the changed shorts.
fn write_short_blocks(w: &mut MsgWriter, from: &[i32; 64], to: &[i32; 64]) {
    for block in 0..4 {
        let mut mask = 0u16;
        for i in 0..16 {
            if from[block * 16 + i] != to[block * 16 + i] {
                mask |= 1 << i;
            }
        }
        if mask == 0 {
            w.write_bits(0, 1);
            continue;
        }
        w.write_bits(1, 1);
        w.write_short(mask as i16);
        for i in 0..16 {
            if mask >> i & 1 != 0 {
                w.write_short(to[block * 16 + i] as i16);
            }
        }
    }
}

/// `MSG_WriteDeltaHudElems`: the count of leading elements with a `type`,
/// then per element the index of its last changed field (0 when none) and
/// fields `0..=last`.
fn write_hudelems(w: &mut MsgWriter, from: &[HudElem2], to: &[HudElem2]) {
    let ty = field_index(&HUDELEM_FIELDS, "type");
    let count = to.iter().take_while(|e| e[ty] != 0).count();
    w.write_bits(count as i32, 5);
    for (e, base) in to.iter().zip(from).take(count) {
        let last = (0..HUD_FIELDS).rposition(|i| e[i] != base[i]).unwrap_or(0);
        w.write_bits(last as i32, 5);
        for i in 0..=last {
            write_delta_field(w, base[i] as i32, e[i] as i32, HUDELEM_FIELDS[i].bits);
        }
    }
}

/// `MSG_WriteDeltaPlayerstate` (0x806a200): `to` against `from`, `None` the
/// zero state. The mirror of `delta::read_delta_playerstate`.
pub fn write_delta_playerstate(w: &mut MsgWriter, from: Option<&PlayerState2>, to: &PlayerState2) {
    let zero;
    let from = match from {
        Some(p) => p,
        None => {
            zero = PlayerState2::default();
            &zero
        }
    };
    let lc = (0..PLAYER_STATE_FIELDS.len())
        .rposition(|i| from.f[i] != to.f[i])
        .map_or(0, |i| i + 1);
    w.write_byte(lc as u8);
    for (i, field) in PLAYER_STATE_FIELDS.iter().enumerate().take(lc) {
        if from.f[i] == to.f[i] {
            w.write_bits(0, 1);
        } else {
            w.write_bits(1, 1);
            write_ps_value(w, to.f[i] as i32, field.bits);
        }
    }

    let mask = (0..6).fold(0, |m, i| m | i32::from(from.stats[i] != to.stats[i]) << i);
    if mask == 0 {
        w.write_bits(0, 1);
    } else {
        w.write_bits(1, 1);
        w.write_bits(mask, 6);
        for i in 0..6 {
            if mask >> i & 1 != 0 {
                write_stat(w, i, to.stats[i]);
            }
        }
    }
    if from.ammo == to.ammo {
        w.write_bits(0, 1);
    } else {
        w.write_bits(1, 1);
        write_short_blocks(w, &from.ammo, &to.ammo);
    }
    write_short_blocks(w, &from.ammoclip, &to.ammoclip);
    if from.objectives == to.objectives {
        w.write_bits(0, 1);
    } else {
        w.write_bits(1, 1);
        for (o, base) in to.objectives.iter().zip(&from.objectives) {
            w.write_bits(o.state as i32, 3);
            if o.f == base.f {
                w.write_bits(0, 1);
            } else {
                w.write_bits(1, 1);
                for (i, field) in OBJECTIVE_FIELDS.iter().enumerate() {
                    write_delta_field(w, base.f[i] as i32, o.f[i] as i32, field.bits);
                }
            }
        }
    }
    if from.hud_archival == to.hud_archival && from.hud_current == to.hud_current {
        w.write_bits(0, 1);
    } else {
        w.write_bits(1, 1);
        write_hudelems(w, &from.hud_archival, &to.hud_archival);
        write_hudelems(w, &from.hud_current, &to.hud_current);
    }
}

/// A Latin-1 string's bytes, one per char (the inverse of `super::latin1`).
pub fn latin1_bytes(s: &str) -> Vec<u8> {
    s.chars().map(|c| c as u32 as u8).collect()
}

/// `svc_serverCommand`: the sequence and the text as its bytes.
pub fn write_server_command(w: &mut MsgWriter, seq: i32, text: &[u8]) {
    w.write_byte(Svc::ServerCommand as u8);
    w.write_long(seq);
    w.write_string_bytes(text);
}

/// `SV_SendClientGameState`'s body (3.2) from `svc_gamestate` to the
/// `checksumFeed`: every non-empty configstring ascending, every baseline
/// from the null state. The message's own `svc_EOF` is the netchan's.
pub fn write_gamestate(w: &mut MsgWriter, gs: &Gamestate2) {
    w.write_byte(Svc::Gamestate as u8);
    w.write_long(gs.server_command_sequence);
    for (&i, s) in gs.configstrings.iter().filter(|(_, s)| !s.is_empty()) {
        w.write_byte(Svc::Configstring as u8);
        w.write_short(i as i16);
        w.write_string_bytes(&latin1_bytes(s));
    }
    for (&n, es) in &gs.baselines {
        w.write_byte(Svc::Baseline as u8);
        write_delta_entity(w, &EntityState2::null(n), Some(es), true);
    }
    w.write_byte(Svc::Eof as u8);
    w.write_long(gs.client_num);
    w.write_long(gs.checksum_feed);
}

/// One snapshot frame as the server keeps it for deltas.
#[derive(Clone, Debug, PartialEq)]
pub struct Frame2 {
    pub server_time: i32,
    pub ps: PlayerState2,
    /// Ascending by number.
    pub entities: Vec<EntityState2>,
    /// Ascending by number.
    pub clients: Vec<ClientState2>,
}

/// `SV_WriteSnapshotToClient` (0x80985c8) from `svc_snapshot` on: `to`
/// deltaed from `from`, which went out `delta_num` messages back (`None`
/// and 0 for a full frame); an entity new to the frame deltas from its
/// `baseline`, a new client state from zero. `nops` is `sv_padPackets`.
pub fn write_snapshot(
    w: &mut MsgWriter,
    from: Option<&Frame2>,
    delta_num: u8,
    snap_flags: u8,
    to: &Frame2,
    baseline: impl Fn(u32) -> Option<EntityState2>,
    nops: usize,
) {
    w.write_byte(Svc::Snapshot as u8);
    w.write_long(to.server_time);
    w.write_byte(if from.is_some() { delta_num } else { 0 });
    w.write_byte(snap_flags);
    write_delta_playerstate(w, from.map(|f| &f.ps), &to.ps);

    let old: &[EntityState2] = from.map_or(&[], |f| &f.entities);
    let (mut oi, mut ni) = (0, 0);
    while oi < old.len() || ni < to.entities.len() {
        let o = old.get(oi);
        let n = to.entities.get(ni);
        match (o, n) {
            (Some(o), Some(n)) if o.number == n.number => {
                write_delta_entity(w, o, Some(n), false);
                oi += 1;
                ni += 1;
            }
            (Some(o), Some(n)) if n.number < o.number => {
                let base = baseline(n.number).unwrap_or(EntityState2::null(n.number));
                write_delta_entity(w, &base, Some(n), true);
                ni += 1;
            }
            (None, Some(n)) => {
                let base = baseline(n.number).unwrap_or(EntityState2::null(n.number));
                write_delta_entity(w, &base, Some(n), true);
                ni += 1;
            }
            (Some(o), _) => {
                write_delta_entity(w, o, None, true);
                oi += 1;
            }
            (None, None) => unreachable!(),
        }
    }
    w.write_bits(ENTITYNUM_NONE as i32, GENTITYNUM_BITS);

    let old: &[ClientState2] = from.map_or(&[], |f| &f.clients);
    let (mut oi, mut ni) = (0, 0);
    while oi < old.len() || ni < to.clients.len() {
        match (old.get(oi), to.clients.get(ni)) {
            (Some(o), Some(n)) if o.number == n.number => {
                write_delta_client(w, o, Some(n), false);
                oi += 1;
                ni += 1;
            }
            (Some(o), Some(n)) if n.number < o.number => {
                write_delta_client(w, &ClientState2::null(n.number), Some(n), true);
                ni += 1;
            }
            (None, Some(n)) => {
                write_delta_client(w, &ClientState2::null(n.number), Some(n), true);
                ni += 1;
            }
            (Some(o), _) => {
                write_delta_client(w, o, None, true);
                oi += 1;
            }
            (None, None) => unreachable!(),
        }
    }
    w.write_bits(0, 1);
    for _ in 0..nops {
        w.write_byte(Svc::Nop as u8);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::Parser;
    use crate::net::capture::{Capture, Dir};
    use crate::net::oob::{self, Reply};
    use vcod_common::net::huffman::Huffman;
    use vcod_common::net::netchan::{Netchan, NetchanParams};

    const FIXTURE: &[u8] = include_bytes!("../../tests/fixtures/mp_toujane-spectate.bin");

    /// Every server message of the retail session, decompressed, with the
    /// parser's state after it.
    fn retail_messages() -> Vec<(i32, Vec<u8>, usize)> {
        let cap = Capture::decode(FIXTURE).unwrap();
        let huff = Huffman::new();
        let mut challenge = None;
        let mut nc: Option<Netchan> = None;
        let mut parser = Parser::new();
        let mut out = Vec::new();
        for (dir, p) in &cap.packets {
            if let Some(pl) = oob::payload(p) {
                match dir {
                    Dir::FromServer => {
                        if let Reply::Challenge(c) = oob::parse_reply(pl) {
                            challenge = Some(c);
                        }
                    }
                    Dir::ToServer => {
                        let t = String::from_utf8_lossy(pl);
                        if let Some(ui) = t.strip_prefix("connect \"") {
                            let q = oob::info_value_for_key(ui.trim_end_matches('"'), "qport")
                                .unwrap()
                                .parse()
                                .unwrap();
                            nc = Some(Netchan::with_params(
                                q,
                                challenge.unwrap(),
                                NetchanParams::COD2,
                            ));
                        }
                    }
                }
                continue;
            }
            if *dir != Dir::FromServer {
                continue;
            }
            let nc = nc.as_mut().unwrap();
            let Some((seq, mut body)) = nc.frame(p).unwrap() else {
                continue;
            };
            nc.decode(seq, b"", &mut body);
            let plain = huff.decompress_block(&body[4..]);
            let consumed = parser.parse(seq as i32, &plain).unwrap().consumed;
            out.push((seq as i32, plain, consumed));
        }
        out
    }

    /// Re-encode one retail message from what the parser decoded: the
    /// reliable acknowledge is the netchan's, so the ops start at byte 0.
    #[test]
    fn retail_gamestate_and_snapshots_re_encode_byte_for_byte() {
        let huff = Huffman::new();
        let msgs = retail_messages();
        let mut parser = Parser::new();
        let mut frames: std::collections::BTreeMap<i32, Frame2> = Default::default();
        let (mut gamestates, mut snapshots) = (0, 0);
        for (seq, plain, consumed) in &msgs {
            let p = parser.parse(*seq, plain).unwrap();
            let mut w = MsgWriter::new(&huff);
            for (s, text) in &p.commands {
                write_server_command(&mut w, *s, text);
            }
            if p.gamestate {
                gamestates += 1;
                write_gamestate(&mut w, parser.gamestate.as_ref().unwrap());
            }
            if let Some(n) = p.snapshot {
                snapshots += 1;
                let s = parser.frame(n).unwrap();
                let to = Frame2 {
                    server_time: s.server_time,
                    ps: s.ps.clone(),
                    entities: s.entities.clone(),
                    clients: s.clients.clone(),
                };
                let from = (s.delta_num >= 0).then(|| &frames[&s.delta_num]);
                let gs = parser.gamestate.as_ref().unwrap();
                write_snapshot(
                    &mut w,
                    from,
                    (n - s.delta_num) as u8,
                    s.snap_flags,
                    &to,
                    |num| gs.baselines.get(&num).copied(),
                    p.nops,
                );
                frames.insert(n, to);
            }
            w.write_byte(Svc::Eof as u8);
            let ours = w.into_ops();
            // The parse reads one Huffman symbol of the pad past `svc_EOF`.
            assert_eq!(
                &ours[..],
                &plain[..ours.len()],
                "message {seq} differs (retail consumed {consumed})"
            );
            assert!(*consumed <= ours.len() + 1, "message {seq}");
        }
        assert_eq!((gamestates, snapshots), (1, 23));
    }

    #[test]
    fn a_frame_round_trips_through_the_parser() {
        let huff = Huffman::new();
        let mut gs = Gamestate2 {
            server_command_sequence: 3,
            configstrings: Default::default(),
            baselines: Default::default(),
            client_num: 2,
            checksum_feed: -5,
        };
        gs.configstrings.insert(0, "\\mapname\\mp_toujane".into());
        gs.configstrings
            .insert(1, "\\sv_serverid\\16\\x\\\u{e9}".into());
        let mut b = EntityState2::null(70);
        b.f[0] = 3;
        gs.baselines.insert(70, b);
        let mut ps = PlayerState2::default();
        ps.f[field_index(&PLAYER_STATE_FIELDS, "pm_type")] = 4;
        ps.f[field_index(&PLAYER_STATE_FIELDS, "origin[2]")] = 456f32.to_bits();
        ps.stats[3] = 63;
        ps.stats[5] = 1;
        ps.ammo[17] = 9;
        ps.hud_current[0][field_index(&HUDELEM_FIELDS, "type")] = 1;
        ps.hud_current[0][field_index(&HUDELEM_FIELDS, "y")] = 28f32.to_bits();
        let mut cs = ClientState2::null(2);
        cs.f[0] = 3;
        let mut e = b;
        e.f[1] = 7;
        let f1 = Frame2 {
            server_time: 1000,
            ps: ps.clone(),
            entities: vec![e],
            clients: vec![cs],
        };
        let mut ps2 = ps.clone();
        ps2.f[field_index(&PLAYER_STATE_FIELDS, "origin[0]")] = (-12.5f32).to_bits();
        ps2.hud_current[0] = [0; HUD_FIELDS];
        let f2 = Frame2 {
            server_time: 1050,
            ps: ps2,
            entities: vec![],
            clients: vec![cs],
        };

        let mut parser = Parser::new();
        let mut w = MsgWriter::new(&huff);
        write_server_command(&mut w, 3, b"v x \"1\"");
        write_gamestate(&mut w, &gs);
        w.write_byte(Svc::Eof as u8);
        parser.parse(1, &w.into_ops()).unwrap();
        assert_eq!(parser.gamestate.as_ref().unwrap(), &gs);

        let mut w = MsgWriter::new(&huff);
        write_snapshot(
            &mut w,
            None,
            0,
            4,
            &f1,
            |n| gs.baselines.get(&n).copied(),
            0,
        );
        w.write_byte(Svc::Eof as u8);
        parser.parse(2, &w.into_ops()).unwrap();
        let s = parser.frame(2).unwrap();
        assert_eq!(
            (&s.ps, &s.entities, &s.clients),
            (&f1.ps, &f1.entities, &f1.clients)
        );

        let mut w = MsgWriter::new(&huff);
        write_snapshot(&mut w, Some(&f1), 1, 4, &f2, |_| None, 2);
        w.write_byte(Svc::Eof as u8);
        let p = parser.parse(3, &w.into_ops()).unwrap();
        assert_eq!(p.nops, 2);
        let s = parser.frame(3).unwrap();
        assert_eq!(s.delta_num, 2);
        assert_eq!(
            (&s.ps, &s.entities, &s.clients),
            (&f2.ps, &f2.entities, &f2.clients)
        );
    }
}
