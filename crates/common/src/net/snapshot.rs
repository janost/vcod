//! Contains routines ported from the RTCW-MP GPL source, Copyright (C) 1999-2010 id Software LLC, a ZeniMax Media company.
//! See NOTICE.
//!
//! `CL_ParseSnapshot` + `CL_ParsePacketEntities` (RTCW-MP cl_parse.c), and the
//! ring of recent snapshots a delta frame resolves its base from.

use super::msg::{
    read_delta_client, read_delta_entity, read_delta_playerstate, write_delta_entity, ClientState,
    EntityState, MsgReader, MsgWriter, PlayerState,
};
use super::protocol::{Protocol, ENTITYNUM_NONE, GENTITYNUM_BITS};
use std::collections::{BTreeMap, BTreeSet, HashMap};

/// `svc_ops_e` (confirmed against cod_lnxded 1.1d).
pub const SVC_BAD: u8 = 0;
pub const SVC_NOP: u8 = 1;
pub const SVC_GAMESTATE: u8 = 2;
pub const SVC_CONFIGSTRING: u8 = 3;
pub const SVC_BASELINE: u8 = 4;
pub const SVC_SERVER_COMMAND: u8 = 5;
pub const SVC_DOWNLOAD: u8 = 6;
pub const SVC_SNAPSHOT: u8 = 7;
pub const SVC_EOF: u8 = 8;

/// `PACKET_BACKUP`.
const RING_CAP: usize = 32;

#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    pub server_time: i32,
    pub message_num: u32,
    /// Base message, or -1 when uncompressed.
    pub delta_num: i32,
    pub snap_flags: u32,
    pub ps: PlayerState,
    pub entities: BTreeMap<u32, EntityState>,
    pub clients: BTreeMap<u32, ClientState>,
    /// False when the delta base was missing; such a frame is not kept as a
    /// base.
    pub valid: bool,
}

/// Slots indexed by `message_num % RING_CAP`, plus the gamestate baselines for
/// entities with no base in the previous frame.
pub struct SnapshotRing {
    slots: Vec<Option<Snapshot>>,
    newest_num: Option<u32>,
    dropped: Option<Snapshot>,
    baselines: HashMap<u32, EntityState>,
}

impl Default for SnapshotRing {
    fn default() -> Self {
        Self::new()
    }
}

impl SnapshotRing {
    pub fn new() -> Self {
        SnapshotRing {
            slots: vec![None; RING_CAP],
            newest_num: None,
            dropped: None,
            baselines: HashMap::new(),
        }
    }

    /// Set once from the parsed gamestate.
    pub fn set_baselines(&mut self, baselines: HashMap<u32, EntityState>) {
        self.baselines = baselines;
    }

    /// A new gamestate: every frame deltas against dead baselines.
    pub fn clear(&mut self) {
        self.slots.iter_mut().for_each(|s| *s = None);
        self.newest_num = None;
        self.dropped = None;
    }

    fn slot(num: u32) -> usize {
        num as usize % RING_CAP
    }

    /// `None` for an evicted or invalid base.
    pub fn get(&self, num: u32) -> Option<&Snapshot> {
        self.slots[Self::slot(num)]
            .as_ref()
            .filter(|s| s.valid && s.message_num == num)
    }

    pub fn insert(&mut self, snap: Snapshot) {
        let num = snap.message_num;
        if self.newest_num.is_none_or(|n| num >= n) {
            self.newest_num = Some(num);
        }
        self.slots[Self::slot(num)] = Some(snap);
    }

    pub fn newest(&self) -> Option<&Snapshot> {
        self.newest_num
            .and_then(|n| self.slots[Self::slot(n)].as_ref())
            .filter(|s| s.valid)
    }

    /// Parse from after the `svc_snapshot` op. A frame whose base is missing
    /// parses to `valid = false` and is not kept.
    pub fn parse_into(
        &mut self,
        r: &mut MsgReader,
        p: &Protocol,
        message_num: u32,
    ) -> anyhow::Result<&Snapshot> {
        let server_time = r.read_long();
        let delta_byte = r.read_byte();
        // 0 means uncompressed; otherwise the base is this many messages back.
        let delta_num = if delta_byte == 0 {
            -1
        } else {
            message_num as i32 - delta_byte as i32
        };
        let snap_flags = r.read_byte() as u32;
        // No areamask on CoD1 (docs/protocol-1.1.md, divergence 6); reading one
        // desyncs the playerState.

        // Clone the base out of the ring so the borrow ends before re-insert.
        let (base_ps, base_entities, base_clients, valid) = if delta_num <= 0 {
            (PlayerState::null(p), None, None, true)
        } else if let Some(base) = self.get(delta_num as u32) {
            (
                base.ps.clone(),
                Some(base.entities.clone()),
                Some(base.clients.clone()),
                true,
            )
        } else {
            (PlayerState::null(p), None, None, false)
        };

        let ps = read_delta_playerstate(r, p, &base_ps);
        let entities = parse_packet_entities(r, p, base_entities.as_ref(), &self.baselines);
        let clients = parse_clients(r, p, base_clients);

        anyhow::ensure!(
            !r.is_overflowed(),
            "snapshot {message_num} overran the message"
        );

        let snap = Snapshot {
            server_time,
            message_num,
            delta_num,
            snap_flags,
            ps,
            entities,
            clients,
            valid,
        };

        if valid {
            self.insert(snap);
            Ok(self.slots[Self::slot(message_num)].as_ref().unwrap())
        } else {
            // Dropped: keep it only long enough to hand back a reference.
            self.dropped = Some(snap);
            Ok(self.dropped.as_ref().unwrap())
        }
    }

    /// `(a, b)` with `a.server_time <= t < b.server_time` and nothing valid
    /// between. `None` outside the buffered range.
    pub fn two_for_time(&self, t: i32) -> Option<(&Snapshot, &Snapshot)> {
        let mut valid: Vec<&Snapshot> = self
            .slots
            .iter()
            .filter_map(|s| s.as_ref())
            .filter(|s| s.valid)
            .collect();
        valid.sort_by_key(|s| s.server_time);
        valid
            .windows(2)
            .find(|w| w[0].server_time <= t && t < w[1].server_time)
            .map(|w| (w[0], w[1]))
    }
}

/// `CL_ParsePacketEntities`. Ascending numbers, `ENTITYNUM_NONE` terminates.
/// An entity absent from the base frame deltas from its baseline; base-frame
/// entities not mentioned carry forward.
fn parse_packet_entities(
    r: &mut MsgReader,
    p: &Protocol,
    oldframe: Option<&BTreeMap<u32, EntityState>>,
    baselines: &HashMap<u32, EntityState>,
) -> BTreeMap<u32, EntityState> {
    let mut new = BTreeMap::new();
    let empty = BTreeMap::new();
    let mut old = oldframe.unwrap_or(&empty).iter().peekable();
    let null = EntityState::null(p);

    loop {
        let newnum = r.read_bits(GENTITYNUM_BITS as i32) as u32;
        if newnum == ENTITYNUM_NONE || r.is_overflowed() {
            break;
        }

        // Base-frame entities below newnum carry forward.
        while old.peek().is_some_and(|(&oldnum, _)| oldnum < newnum) {
            let (&oldnum, oldstate) = old.next().unwrap();
            new.insert(oldnum, oldstate.clone());
        }

        // Base frame, else baseline, else null.
        let base = match old.peek() {
            Some(&(&oldnum, oldstate)) if oldnum == newnum => {
                old.next();
                oldstate
            }
            _ => baselines.get(&newnum).unwrap_or(&null),
        };

        if let Some(es) = read_delta_entity(r, p, base, newnum) {
            new.insert(newnum, es);
        }
    }

    // The rest of the base frame carries forward.
    for (&oldnum, oldstate) in old {
        new.insert(oldnum, oldstate.clone());
    }
    new
}

/// The clientState stream: repeated `[1 bit][6-bit index][delta]`, a 0 bit
/// terminates (`SV_WriteSnapshotToClient` 0x808e1fd;
/// docs/research/clientstate-wire-format.md). Unmentioned clients carry
/// forward; a removed delta drops the client. On an uncompressed frame the
/// caller passes no oldframe, so the roster restarts from null.
fn parse_clients(
    r: &mut MsgReader,
    p: &Protocol,
    oldframe: Option<BTreeMap<u32, ClientState>>,
) -> BTreeMap<u32, ClientState> {
    let mut new = oldframe.unwrap_or_default();
    let null = ClientState::null(p);
    while r.read_bits(1) == 1 {
        if r.is_overflowed() {
            break;
        }
        let num = r.read_bits(6) as u32;
        let base = new.get(&num).unwrap_or(&null).clone();
        match read_delta_client(r, p, &base, num) {
            Some(cs) => {
                new.insert(num, cs);
            }
            None => {
                new.remove(&num);
            }
        }
    }
    new
}

/// `SV_WriteSnapshotToClient`. `from` is the base frame the client acked, or
/// `None` for an uncompressed frame. No areamask on CoD 1.1: the header goes
/// straight into the playerstate delta (docs/protocol-1.1.md, divergence 6).
pub fn write(
    w: &mut MsgWriter,
    p: &Protocol,
    message_num: u32,
    from: Option<&Snapshot>,
    to: &Snapshot,
    baselines: &HashMap<u32, EntityState>,
) {
    use super::msg::{write_delta_client, write_delta_playerstate};

    w.write_long(to.server_time);
    // deltaNum rides the wire as the offset back to the base, not its number.
    let delta_byte = match from {
        Some(base) => (message_num - base.message_num) as u8,
        None => 0,
    };
    w.write_byte(delta_byte);
    w.write_byte(to.snap_flags as u8);

    let null_ps = PlayerState::null(p);
    let base_ps = from.map_or(&null_ps, |b| &b.ps);
    write_delta_playerstate(w, p, base_ps, &to.ps);

    write_packet_entities(w, p, from.map(|b| &b.entities), &to.entities, baselines);

    let null_cs = ClientState::null(p);
    match from {
        // Every present client full from null; the reader restarts its roster
        // on an uncompressed frame, so departures need no removal bit.
        None => {
            for (&num, cs) in &to.clients {
                w.write_bits(1, 1);
                w.write_bits(num as i32, 6);
                write_delta_client(w, p, &null_cs, Some(cs));
            }
        }
        Some(base) => {
            let nums: BTreeSet<u32> = base
                .clients
                .keys()
                .chain(to.clients.keys())
                .copied()
                .collect();
            for num in nums {
                let old = base.clients.get(&num);
                let new = to.clients.get(&num);
                if old == new {
                    continue;
                }
                w.write_bits(1, 1);
                w.write_bits(num as i32, 6);
                write_delta_client(w, p, old.unwrap_or(&null_cs), new);
            }
        }
    }
    w.write_bits(0, 1);
}

/// `SV_EmitPacketEntities`. Ascending numbers, `ENTITYNUM_NONE` terminates.
/// An entity unchanged from the base frame is omitted and the reader carries
/// it forward; one absent from the base is always written, even unchanged
/// from its baseline, or the reader never learns it exists.
fn write_packet_entities(
    w: &mut MsgWriter,
    p: &Protocol,
    from: Option<&BTreeMap<u32, EntityState>>,
    to: &BTreeMap<u32, EntityState>,
    baselines: &HashMap<u32, EntityState>,
) {
    let empty = BTreeMap::new();
    let base = from.unwrap_or(&empty);
    let null = EntityState::null(p);

    let nums: BTreeSet<u32> = base.keys().chain(to.keys()).copied().collect();
    for num in nums {
        let old = base.get(&num);
        let new = to.get(&num);
        match (old, new) {
            // Compare fields only, not the whole EntityState: `number` is not
            // meaningful in `o` unless it came off the same construction path
            // as `n`, and retail's own unchanged test is `lc == 0` over fields.
            (Some(o), Some(n)) if o.fields == n.fields => continue,
            (Some(o), Some(n)) => {
                w.write_bits(num as i32, GENTITYNUM_BITS as i32);
                write_delta_entity(w, p, o, Some(n));
            }
            (None, Some(n)) => {
                w.write_bits(num as i32, GENTITYNUM_BITS as i32);
                write_delta_entity(w, p, baselines.get(&num).unwrap_or(&null), Some(n));
            }
            (Some(o), None) => {
                w.write_bits(num as i32, GENTITYNUM_BITS as i32);
                write_delta_entity(w, p, o, None);
            }
            (None, None) => unreachable!("number came from one of the two maps"),
        }
    }
    w.write_bits(ENTITYNUM_NONE as i32, GENTITYNUM_BITS as i32);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::huffman::Huffman;
    use crate::net::msg::MsgWriter;
    use crate::net::protocol::PROTOCOL_V1;
    use std::collections::HashMap;

    /// Write a changed integral-float entity/player field.
    fn wfloat(w: &mut MsgWriter, v: f32) {
        w.write_bits(1, 1); // changed
        w.write_bits(1, 1); // non-zero
        w.write_bits(0, 1); // integral
        w.write_packed_bits(v as i32 + 4096, 13);
    }

    /// playerState delta from null: origin, then empty arrays.
    fn write_playerstate(w: &mut MsgWriter, origin: [f32; 3]) {
        let p = &PROTOCOL_V1;
        let o0 = PlayerState::field_index(p, "origin[0]").unwrap();
        let o1 = PlayerState::field_index(p, "origin[1]").unwrap();
        let o2 = PlayerState::field_index(p, "origin[2]").unwrap();
        let lc = o0.max(o1).max(o2) + 1;
        w.write_byte(lc as u8);
        for i in 0..lc {
            if i == o0 {
                w.write_bits(1, 1);
                w.write_bits(0, 1);
                w.write_packed_bits(origin[0] as i32 + 4096, 13);
            } else if i == o1 {
                w.write_bits(1, 1);
                w.write_bits(0, 1);
                w.write_packed_bits(origin[1] as i32 + 4096, 13);
            } else if i == o2 {
                w.write_bits(1, 1);
                w.write_bits(0, 1);
                w.write_packed_bits(origin[2] as i32 + 4096, 13);
            } else {
                w.write_bits(0, 1);
            }
        }
        for _ in 0..8 {
            w.write_bits(0, 1); // empty array section
        }
    }

    /// Snapshot body after the op. `entities`: (number, Some(pos.trBase[0]) or
    /// None to keep the base). `clients`: (number, Some(modelindex) or None).
    fn write_snapshot(
        w: &mut MsgWriter,
        server_time: i32,
        delta_byte: u8,
        ps_origin: [f32; 3],
        entities: &[(u32, Option<f32>)],
        clients: &[(u32, Option<i32>)],
    ) {
        w.write_long(server_time);
        w.write_byte(delta_byte);
        w.write_byte(0); // snapFlags; no areamask on CoD1
        write_playerstate(w, ps_origin);
        for &(num, change) in entities {
            w.write_bits(num as i32, GENTITYNUM_BITS as i32);
            w.write_bits(0, 1); // not removed
            match change {
                Some(x) => {
                    w.write_bits(1, 1); // delta follows
                    let p = &PROTOCOL_V1;
                    let f0 = EntityState::field_index(p, "pos.trBase[0]").unwrap();
                    w.write_byte((f0 + 1) as u8); // lc
                    for i in 0..=f0 {
                        if i == f0 {
                            wfloat(w, x);
                        } else {
                            w.write_bits(0, 1);
                        }
                    }
                }
                None => {
                    w.write_bits(0, 1); // no delta: unchanged from base
                }
            }
        }
        w.write_bits(ENTITYNUM_NONE as i32, GENTITYNUM_BITS as i32);
        write_clients(w, clients);
    }

    /// Client section plus its terminating 0 bit.
    fn write_clients(w: &mut MsgWriter, clients: &[(u32, Option<i32>)]) {
        let p = &PROTOCOL_V1;
        for &(num, change) in clients {
            w.write_bits(1, 1); // another client follows
            w.write_bits(num as i32, 6);
            w.write_bits(0, 1); // not removed
            match change {
                Some(mi) => {
                    w.write_bits(1, 1); // delta follows
                    let f = crate::net::msg::ClientState::field_index(p, "modelindex").unwrap();
                    w.write_byte((f + 1) as u8);
                    for i in 0..=f {
                        if i == f {
                            w.write_bits(1, 1);
                            w.write_bits(1, 1);
                            w.write_packed_bits(mi, p.client_fields[f].bits);
                        } else {
                            w.write_bits(0, 1);
                        }
                    }
                }
                None => w.write_bits(0, 1), // no delta
            }
        }
        w.write_bits(0, 1); // no more clients
    }

    fn baselines() -> HashMap<u32, EntityState> {
        let p = &PROTOCOL_V1;
        let mut b = HashMap::new();
        b.insert(5, EntityState::null(p));
        b.insert(9, EntityState::null(p));
        b
    }

    #[test]
    fn parses_client_section_and_carries_forward() {
        let p = &PROTOCOL_V1;
        let h = Huffman::new();
        let mut ring = SnapshotRing::new();
        ring.set_baselines(baselines());

        let mut w = MsgWriter::new(&h);
        write_snapshot(
            &mut w,
            48_000,
            0,
            [0.0; 3],
            &[],
            &[(0, Some(12)), (3, Some(37))],
        );
        let d = w.finish();
        let mut r = MsgReader::new(&d, &h);
        let s = ring.parse_into(&mut r, p, 10).unwrap();
        assert_eq!(s.clients.len(), 2);
        assert_eq!(s.clients[&3].field_i32(p, "modelindex"), 37);
        assert!(!r.is_overflowed());

        // delta frame touching only client 0
        let mut w = MsgWriter::new(&h);
        write_snapshot(&mut w, 48_050, 1, [0.0; 3], &[], &[(0, Some(13))]);
        let d = w.finish();
        let mut r = MsgReader::new(&d, &h);
        let s = ring.parse_into(&mut r, p, 11).unwrap();
        assert_eq!(s.clients[&0].field_i32(p, "modelindex"), 13);
        assert_eq!(s.clients[&3].field_i32(p, "modelindex"), 37);
    }

    #[test]
    fn parse_into_decodes_uncompressed_frame() {
        let p = &PROTOCOL_V1;
        let h = Huffman::new();
        let mut ring = SnapshotRing::new();
        ring.set_baselines(baselines());

        let mut w = MsgWriter::new(&h);
        write_snapshot(
            &mut w,
            48_000,
            0,
            [200.0, 100.0, 50.0],
            &[(5, Some(300.0)), (9, None)],
            &[],
        );
        let data = w.finish();

        let mut r = MsgReader::new(&data, &h);
        let s = ring.parse_into(&mut r, p, 10).unwrap();
        assert!(s.valid);
        assert_eq!(s.server_time, 48_000);
        assert_eq!(s.message_num, 10);
        assert_eq!(s.delta_num, -1); // uncompressed
        assert_eq!(s.ps.origin(p), [200.0, 100.0, 50.0]);
        assert_eq!(s.entities.keys().copied().collect::<Vec<_>>(), vec![5, 9]);
        assert_eq!(s.entities[&5].field_f32(p, "pos.trBase[0]"), 300.0);
        assert_eq!(s.entities[&9].field_f32(p, "pos.trBase[0]"), 0.0);
        assert!(!r.is_overflowed());
    }

    #[test]
    fn parse_into_resolves_delta_base() {
        let p = &PROTOCOL_V1;
        let h = Huffman::new();
        let mut ring = SnapshotRing::new();
        ring.set_baselines(baselines());

        // frame 10: uncompressed, entity 5 at x=300
        let mut w = MsgWriter::new(&h);
        write_snapshot(&mut w, 48_000, 0, [0.0, 0.0, 0.0], &[(5, Some(300.0))], &[]);
        let d = w.finish();
        let mut r = MsgReader::new(&d, &h);
        ring.parse_into(&mut r, p, 10).unwrap();

        // frame 11: delta from 10 (deltaByte 1), touching nothing
        let mut w = MsgWriter::new(&h);
        write_snapshot(&mut w, 48_050, 1, [0.0, 0.0, 0.0], &[], &[]);
        let d = w.finish();
        let mut r = MsgReader::new(&d, &h);
        let s = ring.parse_into(&mut r, p, 11).unwrap();
        assert!(s.valid);
        assert_eq!(s.delta_num, 10);
        assert_eq!(s.entities[&5].field_f32(p, "pos.trBase[0]"), 300.0);
        assert_eq!(ring.newest().unwrap().message_num, 11);
    }

    #[test]
    fn parse_into_drops_frame_with_missing_base() {
        let p = &PROTOCOL_V1;
        let h = Huffman::new();
        let mut ring = SnapshotRing::new();
        ring.set_baselines(baselines());

        // delta from 10, which was never parsed
        let mut w = MsgWriter::new(&h);
        write_snapshot(&mut w, 48_050, 1, [0.0, 0.0, 0.0], &[(5, Some(7.0))], &[]);
        let d = w.finish();
        let mut r = MsgReader::new(&d, &h);
        let s = ring.parse_into(&mut r, p, 11).unwrap();
        assert!(!s.valid, "missing base must invalidate the frame");
        assert!(ring.newest().is_none(), "a dropped frame is not kept");
        assert!(
            !r.is_overflowed(),
            "an invalid frame is still fully consumed"
        );
    }

    fn snap(message_num: u32, delta_num: i32, server_time: i32, valid: bool) -> Snapshot {
        Snapshot {
            server_time,
            message_num,
            delta_num,
            valid,
            ..Default::default()
        }
    }

    #[test]
    fn delta_base_too_old_invalidates() {
        let mut ring = SnapshotRing::new();
        for n in 0..=40u32 {
            ring.insert(snap(n, n as i32 - 1, n as i32 * 50, true));
        }
        // slot 1 now holds 33
        assert!(ring.get(40).is_some());
        assert!(ring.get(1).is_none(), "evicted base must not resolve");
        assert_eq!(ring.get(33).map(|s| s.message_num), Some(33));
        // 9 == 40 - 31 is the oldest still in its own slot; 40 reused 8's
        assert!(ring.get(9).is_some(), "oldest still-live base resolves");
        assert!(ring.get(8).is_none(), "slot reused by a newer frame");
    }

    #[test]
    fn evicted_slot_rejects_mismatched_number() {
        let mut ring = SnapshotRing::new();
        ring.insert(snap(1, -1, 100, true));
        // 33 % 32 == 1
        ring.insert(snap(33, 32, 200, true));
        assert!(ring.get(1).is_none());
        assert_eq!(ring.get(33).map(|s| s.server_time), Some(200));
    }

    #[test]
    fn two_for_time_straddles() {
        let mut ring = SnapshotRing::new();
        ring.insert(snap(1, -1, 100, true));
        ring.insert(snap(2, 1, 150, true));
        ring.insert(snap(3, 2, 200, true));

        let (a, b) = ring.two_for_time(160).unwrap();
        assert_eq!((a.server_time, b.server_time), (150, 200));
        // on a boundary that frame is the earlier of the pair
        let (a, b) = ring.two_for_time(150).unwrap();
        assert_eq!((a.server_time, b.server_time), (150, 200));
        // outside the buffered range
        assert!(ring.two_for_time(50).is_none());
        assert!(ring.two_for_time(200).is_none());
        assert!(ring.two_for_time(999).is_none());
    }

    #[test]
    fn invalid_frames_are_skipped() {
        let mut ring = SnapshotRing::new();
        ring.insert(snap(1, -1, 100, true));
        ring.insert(snap(2, 1, 150, true));
        // a dropped frame never reaches the ring via insert; plant one directly
        let mut bad = snap(3, 2, 200, false);
        bad.valid = false;
        ring.slots[super::SnapshotRing::slot(3)] = Some(bad);
        ring.newest_num = Some(3);

        assert!(ring.newest().is_none(), "newest must skip an invalid frame");
        assert!(
            ring.two_for_time(175).is_none(),
            "an invalid upper bound is not a straddle"
        );
    }

    #[test]
    fn newest_tracks_highest_message() {
        let mut ring = SnapshotRing::new();
        assert!(ring.newest().is_none());
        ring.insert(snap(5, -1, 500, true));
        ring.insert(snap(6, 5, 600, true));
        assert_eq!(ring.newest().map(|s| s.message_num), Some(6));
    }

    /// Replay the committed snapshot capture through a `SnapshotRing` the way
    /// `NetClient::handle_message` does. snapshots.bin is a run of
    /// `[u32 message_num][u32 len][len bytes]`, each `[u32 reliableAck][huffman
    /// block]` like gamestate.bin; baselines come from the gamestate fixture of
    /// the same map. A wrong header framing desyncs the delta and overruns the
    /// reader.
    #[test]
    fn parses_captured_snapshot_run() {
        use crate::net::gamestate;
        use crate::net::protocol::MAX_GENTITIES;
        let p = &PROTOCOL_V1;
        let h = Huffman::new();

        let gs_data = crate::testing::fixture("net/gamestate.bin");
        let mut gr = MsgReader::new(&gs_data[4..], &h);
        let gs = gamestate::parse(&mut gr, p).unwrap();
        let mut ring = SnapshotRing::new();
        ring.set_baselines(gs.baselines.clone());

        let data = crate::testing::fixture("net/snapshots.bin");
        let mut off = 0usize;
        let mut messages = 0usize;
        let mut valid_count = 0usize;
        let mut times: Vec<i32> = Vec::new();
        let mut max_entities = 0usize;
        let mut max_clients = 0usize;

        while off + 8 <= data.len() {
            let message_num = u32::from_le_bytes(data[off..off + 4].try_into().unwrap());
            let len = u32::from_le_bytes(data[off + 4..off + 8].try_into().unwrap()) as usize;
            off += 8;
            let msg = &data[off..off + len];
            off += len;
            messages += 1;

            // four plain reliableAcknowledge bytes, then the huffman block
            assert!(msg.len() >= 4, "message {message_num} shorter than the ack");
            let mut r = MsgReader::new(&msg[4..], &h);

            let mut saw_snapshot = false;
            loop {
                if r.is_overflowed() {
                    break;
                }
                match r.read_byte() {
                    SVC_NOP => {}
                    SVC_SERVER_COMMAND => {
                        r.read_long();
                        r.read_big_string();
                    }
                    SVC_SNAPSHOT => {
                        let s = ring.parse_into(&mut r, p, message_num).unwrap();
                        assert!(s.server_time > 0, "serverTime {}", s.server_time);
                        assert!(s.snap_flags <= 0xff, "snapFlags {}", s.snap_flags);
                        assert!(
                            s.delta_num == -1
                                || (s.delta_num > 0 && (s.delta_num as u32) < message_num),
                            "deltaNum {} for message {message_num}",
                            s.delta_num
                        );
                        assert!(
                            !r.is_overflowed(),
                            "snapshot {message_num} overran the message"
                        );

                        if s.valid {
                            for &num in s.entities.keys() {
                                assert!(
                                    num < ENTITYNUM_NONE,
                                    "entity {num} not below ENTITYNUM_NONE"
                                );
                            }
                            assert!(
                                s.entities.len() < MAX_GENTITIES,
                                "{} entities in snapshot {message_num}",
                                s.entities.len()
                            );
                            max_entities = max_entities.max(s.entities.len());
                            for &cn in s.clients.keys() {
                                assert!(cn < 64, "client index {cn}");
                            }
                            max_clients = max_clients.max(s.clients.len());
                            let o = s.ps.origin(p);
                            assert!(o.iter().all(|c| c.abs() < 65536.0), "origin {o:?}");
                            valid_count += 1;
                            times.push(s.server_time);
                        }
                        // The snapshot is the message's last op; huffman padding follows.
                        saw_snapshot = true;
                        break;
                    }
                    SVC_EOF => break,
                    // trailing huffman padding reads back as a stray op
                    _ => break,
                }
            }
            assert!(
                saw_snapshot,
                "captured message {message_num} had no svc_snapshot"
            );
        }

        assert!(messages >= 15, "only {messages} captured messages");
        assert!(
            valid_count >= 15,
            "only {valid_count} valid snapshots in the run"
        );
        for w in times.windows(2) {
            assert!(
                w[1] > w[0],
                "serverTime not strictly ascending: {} then {}",
                w[0],
                w[1]
            );
        }
        // guards against an empty list passing the checks above vacuously
        assert!(
            max_entities > 0,
            "no packet entities decoded across the run"
        );
        assert!(max_clients > 0, "no clientStates decoded across the run");
    }

    #[test]
    fn clear_forgets_every_frame_but_keeps_nothing_stale() {
        use crate::net::gamestate;
        let p = &PROTOCOL_V1;
        let h = Huffman::new();

        let gs_data = crate::testing::fixture("net/gamestate.bin");
        let mut gr = MsgReader::new(&gs_data[4..], &h);
        let gs = gamestate::parse(&mut gr, p).unwrap();
        let mut ring = SnapshotRing::new();
        ring.set_baselines(gs.baselines.clone());

        let data = crate::testing::fixture("net/snapshots.bin");
        let mut off = 0usize;

        while off + 8 <= data.len() {
            let message_num = u32::from_le_bytes(data[off..off + 4].try_into().unwrap());
            let len = u32::from_le_bytes(data[off + 4..off + 8].try_into().unwrap()) as usize;
            off += 8;
            let msg = &data[off..off + len];
            off += len;

            // four plain reliableAcknowledge bytes, then the huffman block
            assert!(msg.len() >= 4, "message {message_num} shorter than the ack");
            let mut r = MsgReader::new(&msg[4..], &h);

            loop {
                if r.is_overflowed() {
                    break;
                }
                match r.read_byte() {
                    SVC_NOP => {}
                    SVC_SERVER_COMMAND => {
                        r.read_long();
                        r.read_big_string();
                    }
                    SVC_SNAPSHOT => {
                        ring.parse_into(&mut r, p, message_num).unwrap();
                        break;
                    }
                    SVC_EOF => break,
                    _ => break,
                }
            }
        }

        let newest = ring.newest().expect("fixture ring has frames").message_num;
        ring.clear();
        assert!(ring.newest().is_none());
        assert!(ring.get(newest).is_none());
        assert!(ring.two_for_time(0).is_none());
    }

    /// Three frames through one ring: a join, a shortening rename plus a
    /// second join, then a disconnect with a surviving client whose team
    /// clears to zero. Entries go out as full states from null, so every
    /// frame decodes standalone.
    #[test]
    fn written_snapshot_client_lifecycle_round_trips() {
        let p = &PROTOCOL_V1;
        let h = Huffman::new();
        let mut ring = SnapshotRing::new();

        let mut ps = PlayerState::null(p);
        ps.fields[PlayerState::field_index(p, "pm_type").unwrap()] = 4;
        ps.fields[PlayerState::field_index(p, "origin[0]").unwrap()] = 384f32.to_bits() as i32;

        // Frame A: client 0 appears out of nothing.
        let mut a = Snapshot {
            message_num: 10,
            server_time: 48_000,
            valid: true,
            ps: ps.clone(),
            ..Default::default()
        };
        a.clients.insert(0, ClientState::named(p, 0, 3, "vcod"));
        let mut w = MsgWriter::new(&h);
        write(&mut w, p, 10, None, &a, &HashMap::new());
        let bits = w.bits_written();
        let d = w.finish();
        let mut r = MsgReader::new(&d, &h);
        let s = ring.parse_into(&mut r, p, 10).unwrap();
        assert!(s.valid);
        assert_eq!(s.delta_num, -1);
        assert_eq!(s.server_time, 48_000);
        assert_eq!(s.ps.origin(p)[0], 384.0);
        assert_eq!(s.ps.field_i32(p, "pm_type"), 4);
        assert_eq!(s.clients[&0].name(p), "vcod");
        assert!(s.entities.is_empty());
        assert!(!r.is_overflowed());
        assert_eq!(r.bits_read(), bits);

        // Frame B: client 0 renamed shorter, client 3 joins. Slot indexes are
        // client numbers, so the gap stays absent.
        let mut b = Snapshot {
            message_num: 11,
            server_time: 48_050,
            valid: true,
            ps: ps.clone(),
            ..Default::default()
        };
        b.clients.insert(0, ClientState::named(p, 0, 3, "bob"));
        b.clients.insert(3, ClientState::named(p, 3, 1, "eve"));
        let mut w = MsgWriter::new(&h);
        write(&mut w, p, 11, None, &b, &HashMap::new());
        let bits = w.bits_written();
        let d = w.finish();
        let mut r = MsgReader::new(&d, &h);
        let s = ring.parse_into(&mut r, p, 11).unwrap();
        assert!(s.valid);
        assert_eq!(s.clients[&0].name(p), "bob");
        assert_eq!(s.clients[&3].name(p), "eve");
        assert!(!r.is_overflowed());
        assert_eq!(r.bits_read(), bits);

        // Frame C: client 0 leaves by omission. An uncompressed frame
        // restarts the reader's roster, so no removal bit exists; the
        // survivor is re-sent as a full state from null with its team cleared
        // back to zero, and that field must decode as zero rather than the
        // roster carry-forward a non-null resolve would produce.
        let mut eve_cleared = ClientState::named(p, 3, 1, "eve");
        let ti = ClientState::field_index(p, "team").unwrap();
        eve_cleared.fields[ti] = 0;
        let mut c = Snapshot {
            message_num: 12,
            server_time: 48_100,
            valid: true,
            ps: ps.clone(),
            ..Default::default()
        };
        c.clients.insert(3, eve_cleared);
        let mut w = MsgWriter::new(&h);
        write(&mut w, p, 12, None, &c, &HashMap::new());
        let bits = w.bits_written();
        let d = w.finish();
        let mut r = MsgReader::new(&d, &h);
        let s = ring.parse_into(&mut r, p, 12).unwrap();
        assert!(s.valid);
        assert_eq!(s.clients.keys().copied().collect::<Vec<_>>(), vec![3]);
        assert_eq!(s.clients[&3].name(p), "eve");
        assert_eq!(s.clients[&3].field_i32(p, "team"), 0);
        assert!(!r.is_overflowed());
        assert_eq!(r.bits_read(), bits);
    }

    /// A delta frame's roster: an unchanged client is omitted, a changed one
    /// deltas against the base entry, a departed one gets the removal bit.
    #[test]
    fn delta_roster_omits_unchanged_and_removes_departed() {
        let p = &PROTOCOL_V1;
        let h = Huffman::new();

        let mut base = Snapshot {
            message_num: 10,
            server_time: 1000,
            valid: true,
            ps: PlayerState::null(p),
            ..Default::default()
        };
        base.clients.insert(0, ClientState::named(p, 0, 3, "alice"));
        base.clients.insert(1, ClientState::named(p, 1, 3, "bob"));
        base.clients.insert(2, ClientState::named(p, 2, 3, "carol"));

        // alice unchanged, bob renamed, carol gone, dave new.
        let mut to = Snapshot {
            message_num: 11,
            server_time: 1050,
            valid: true,
            ps: PlayerState::null(p),
            ..Default::default()
        };
        to.clients.insert(0, ClientState::named(p, 0, 3, "alice"));
        to.clients.insert(1, ClientState::named(p, 1, 3, "robert"));
        to.clients.insert(3, ClientState::named(p, 3, 3, "dave"));

        let mut w = MsgWriter::new(&h);
        write(&mut w, p, 11, Some(&base), &to, &HashMap::new());

        let mut ring = SnapshotRing::new();
        ring.insert(base.clone());
        // finish(), not into_ops(): MsgReader::new always block-decompresses,
        // so the reader needs huffman-coded bytes, not the plain wire bits.
        let bytes = w.finish();
        let mut r = MsgReader::new(&bytes, &h);
        let got = ring.parse_into(&mut r, p, 11).unwrap().clone();

        assert!(got.valid, "delta frame resolved its base");
        assert_eq!(got.delta_num, 10);
        assert_eq!(
            got.clients.keys().copied().collect::<Vec<_>>(),
            vec![0, 1, 3]
        );
        assert_eq!(got.clients[&1], to.clients[&1], "renamed client");
        assert_eq!(
            got.clients[&0], base.clients[&0],
            "unchanged carries forward"
        );
        assert_eq!(got.clients[&3], to.clients[&3], "new client from null");
    }

    /// The four packet-entity cases: new-from-baseline, changed, unchanged
    /// (omitted, carried forward), removed.
    #[test]
    fn delta_entities_cover_new_changed_unchanged_and_removed() {
        let p = &PROTOCOL_V1;
        let h = Huffman::new();
        let o0 = EntityState::field_index(p, "pos.trBase[0]").unwrap();
        let o1 = EntityState::field_index(p, "pos.trBase[1]").unwrap();

        let ent = |x: i32| {
            let mut e = EntityState::null(p);
            e.fields[o0] = x;
            e
        };

        // Entity 7 has a baseline and is new to this frame at its baseline value.
        let mut baselines = HashMap::new();
        baselines.insert(7u32, ent(700));

        let mut base = Snapshot {
            message_num: 20,
            server_time: 2000,
            valid: true,
            ps: PlayerState::null(p),
            ..Default::default()
        };
        base.entities.insert(4, ent(400)); // unchanged
                                           // Changed, with a second field (o1: 42 -> 0) that only the correct
                                           // base (the base-frame entity, not the unrelated baseline/null) makes
                                           // `lc` reach far enough to write.
        let mut e5 = ent(500);
        e5.fields[o1] = 42;
        base.entities.insert(5, e5);
        base.entities.insert(6, ent(600)); // removed

        let mut to = Snapshot {
            message_num: 21,
            server_time: 2050,
            valid: true,
            ps: PlayerState::null(p),
            ..Default::default()
        };
        to.entities.insert(4, ent(400));
        to.entities.insert(5, ent(555));
        to.entities.insert(7, ent(700));

        let mut w = MsgWriter::new(&h);
        write(&mut w, p, 21, Some(&base), &to, &baselines);
        let bits = w.bits_written();

        let mut ring = SnapshotRing::new();
        ring.set_baselines(baselines.clone());
        ring.insert(base.clone());
        // finish(), not into_ops(): MsgReader::new always block-decompresses,
        // so the reader needs huffman-coded bytes, not the plain wire bits.
        let bytes = w.finish();
        let mut r = MsgReader::new(&bytes, &h);
        let got = ring.parse_into(&mut r, p, 21).unwrap().clone();

        assert!(got.valid);
        assert_eq!(
            got.entities.keys().copied().collect::<Vec<_>>(),
            vec![4, 5, 7]
        );
        assert_eq!(
            got.entities[&4].fields[o0], 400,
            "unchanged carried forward"
        );
        assert_eq!(got.entities[&5].fields[o0], 555, "changed deltas from base");
        assert_eq!(
            got.entities[&5].fields[o1], 0,
            "changed deltas from the base-frame entity, not the baseline or null \
             (a wrong base would stop lc short of this field and leave 42)"
        );
        assert_eq!(
            got.entities[&7].fields[o0], 700,
            "new entity from its baseline"
        );
        assert!(!r.is_overflowed());
        assert_eq!(r.bits_read(), bits);

        // Identical frames must emit nothing but the terminator: an entity
        // unchanged from the base is omitted, not re-sent as a no-delta entry.
        // Compared against a reference writer that only writes the
        // terminator, not a literal bit count: a fresh MsgWriter rounds
        // bits_written() up to the next byte on a pure-bits write (out.len()
        // pre-allocates a byte at each 8-bit boundary), so GENTITYNUM_BITS
        // itself is not what a correct call reports.
        let mut w2 = MsgWriter::new(&h);
        write_packet_entities(&mut w2, p, Some(&base.entities), &base.entities, &baselines);
        let mut w_ref = MsgWriter::new(&h);
        w_ref.write_bits(ENTITYNUM_NONE as i32, GENTITYNUM_BITS as i32);
        assert_eq!(
            w2.into_ops(),
            w_ref.into_ops(),
            "identical frames emit only the terminator"
        );
    }

    /// Testing layer 2 from the design doc: parse a committed capture
    /// snapshot to full state, rebuild it uncompressed with our writers from
    /// null, parse again, and hold the result against retail's bytes.
    #[test]
    fn reencoded_capture_snapshot_round_trips() {
        use crate::net::gamestate;
        let p = &PROTOCOL_V1;
        let h = Huffman::new();

        let gs_data = crate::testing::fixture("net/gamestate.bin");
        let mut gr = MsgReader::new(&gs_data[4..], &h);
        let gs = gamestate::parse(&mut gr, p).unwrap();
        let mut ring = SnapshotRing::new();
        ring.set_baselines(gs.baselines.clone());

        let data = crate::testing::fixture("net/snapshots.bin");
        let mut off = 0usize;
        let mut found: Option<Snapshot> = None;
        while found.is_none() && off + 8 <= data.len() {
            let message_num = u32::from_le_bytes(data[off..off + 4].try_into().unwrap());
            let len = u32::from_le_bytes(data[off + 4..off + 8].try_into().unwrap()) as usize;
            off += 8;
            let msg = &data[off..off + len];
            off += len;

            let mut r = MsgReader::new(&msg[4..], &h);
            while !r.is_overflowed() {
                match r.read_byte() {
                    SVC_NOP => {}
                    SVC_SERVER_COMMAND => {
                        r.read_long();
                        r.read_big_string();
                    }
                    SVC_SNAPSHOT => {
                        let s = ring.parse_into(&mut r, p, message_num).unwrap();
                        if s.valid {
                            found = Some(s.clone());
                        }
                        break;
                    }
                    _ => break,
                }
            }
        }
        let cap = found.expect("capture holds no valid snapshot");
        assert!(!cap.clients.is_empty(), "no clientStates in the snapshot");

        let mut w = MsgWriter::new(&h);
        write(&mut w, p, cap.message_num, None, &cap, &HashMap::new());
        let d = w.finish();
        let mut out_ring = SnapshotRing::new();
        let mut r = MsgReader::new(&d, &h);
        let back = out_ring.parse_into(&mut r, p, cap.message_num).unwrap();
        assert!(back.valid);
        assert!(!r.is_overflowed());

        assert_eq!(back.server_time, cap.server_time);
        assert_eq!(back.ps, cap.ps);
        assert_eq!(
            back.clients.keys().copied().collect::<Vec<_>>(),
            cap.clients.keys().copied().collect::<Vec<_>>()
        );
        for (&num, cs) in &cap.clients {
            assert_eq!(back.clients[&num].fields, cs.fields, "client {num}");
        }
    }

    /// Re-encodes every snapshot in one captured run against the base the
    /// parse resolved, and returns `(frames, delta frames)`.
    fn regate(gs_fixture: &str, snap_fixture: &str) -> (usize, usize) {
        use crate::net::gamestate;
        let p = &PROTOCOL_V1;
        let h = Huffman::new();

        let gs_data = crate::testing::fixture(gs_fixture);
        let mut gr = MsgReader::new(&gs_data[4..], &h);
        let gs = gamestate::parse(&mut gr, p).unwrap();

        let mut ring = SnapshotRing::new();
        ring.set_baselines(gs.baselines.clone());

        let data = crate::testing::fixture(snap_fixture);
        let mut off = 0usize;
        let mut checked = 0usize;
        let mut delta_frames = 0usize;

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
                        let start_bits = r.bits_read();
                        assert_eq!(start_bits % 8, 0, "frame body starts byte-aligned");

                        let snap = ring.parse_into(&mut r, p, message_num).unwrap().clone();
                        let end_bits = r.bits_read();

                        // Both cursors interleave bits and bytes the same way,
                        // and the terminator's unused high bits are zero on
                        // both sides, so the span compares as whole bytes.
                        let original = r.plain()[start_bits / 8..end_bits.div_ceil(8)].to_vec();

                        let base = if snap.delta_num > 0 {
                            ring.get(snap.delta_num as u32).cloned()
                        } else {
                            None
                        };

                        let mut w = MsgWriter::new(&h);
                        write(&mut w, p, message_num, base.as_ref(), &snap, &gs.baselines);
                        let re = w.into_ops();

                        assert_eq!(
                            re, original,
                            "frame {message_num} (delta_num {}) re-encoded differently",
                            snap.delta_num
                        );
                        checked += 1;
                        if snap.delta_num > 0 {
                            delta_frames += 1;
                        }
                    }
                    _ => break,
                }
            }
        }
        (checked, delta_frames)
    }

    /// Every captured retail frame re-encodes to the identical byte string.
    /// The gamestate writer is pinned the same way
    /// (`writer_reproduces_the_captured_gamestate_byte_for_byte`); this is the
    /// only evidence that our encoder makes retail's choices rather than merely
    /// self-consistent ones.
    ///
    /// Two captured runs. `snapshots.bin` is the first 24 messages of a
    /// connection, every frame uncompressed: the server has no acked frame to
    /// delta against yet, so that run pins nothing about deltas.
    /// `snapshots-delta.bin` is 400 messages off the retail 1.1d dedicated
    /// server on mp_carentan, past the point where the client's acks start
    /// arriving, so every frame but the first is a delta.
    #[test]
    fn writer_reproduces_the_captured_snapshots_byte_for_byte() {
        let (connect, _) = regate("net/gamestate.bin", "net/snapshots.bin");
        assert!(connect > 0, "no snapshots in the connect-time capture");

        let (steady, delta_frames) = regate("net/gamestate-delta.bin", "net/snapshots-delta.bin");
        assert!(
            steady > connect,
            "the steady-state capture shrank: {steady} frames"
        );
        assert!(delta_frames > 0, "capture carried no delta frames to pin");
    }
}
