//! Snapshot event extraction: entity event rings and one-shot event entities
//! to `GameEvent`s, each fired once. docs/research/cod11-events-and-fx.md section 1.

use std::collections::HashMap;

use crate::net::msg::EntityState;
use crate::net::protocol::Protocol;
use crate::net::snapshot::Snapshot;

/// Same as `entities::ET_EVENTS`; duplicated so net stays free of the client crate.
const ET_EVENTS: i32 = 12;
const EVENT_RING: i32 = 4;

pub struct GameEvent {
    /// EV_* id (doc section 1).
    pub event: i32,
    pub parm: i32,
    /// The shooter for fire events, the temp entity itself for
    /// eType >= ET_EVENTS, u32::MAX for playerState events.
    pub entity_num: u32,
    /// -1 when the entity has none.
    pub client_num: i32,
    pub weapon: i32,
    pub surf_type: i32,
    pub pos: [f32; 3],
    /// origin2. Zero on bullet-hit events, whose normal is in `parm`.
    pub dir: [f32; 3],
    /// `otherEntityNum`: the shooter on `EV_BULLET_HIT_*` (doc section 2),
    /// the victim on `EV_OBITUARY` (docs/research/cod11-hud-protocol.md
    /// section 1). u32::MAX on playerState events.
    pub other_entity_num: u32,
    /// `attackerEntityNum`: `EV_OBITUARY`'s attacker, 1022 (`ENTITYNUM_WORLD`)
    /// for a world kill (hud-protocol doc section 1). -1 on playerState events.
    pub attacker_entity_num: i32,
}

/// `bytedirs`: 162 unit vectors indexed by one wire byte (doc section 3).
/// Read out of `cgame_mp_x86.dll` at VA 0x300740d0; identical to Q3's table.
#[rustfmt::skip]
pub const BYTE_DIRS: [[f32; 3]; 162] = [
    [-0.525731, 0.000000, 0.850651], [-0.442863, 0.238856, 0.864188], [-0.295242, 0.000000, 0.955423],
    [-0.309017, 0.500000, 0.809017], [-0.162460, 0.262866, 0.951056], [0.000000, 0.000000, 1.000000],
    [0.000000, 0.850651, 0.525731], [-0.147621, 0.716567, 0.681718], [0.147621, 0.716567, 0.681718],
    [0.000000, 0.525731, 0.850651], [0.309017, 0.500000, 0.809017], [0.525731, 0.000000, 0.850651],
    [0.295242, 0.000000, 0.955423], [0.442863, 0.238856, 0.864188], [0.162460, 0.262866, 0.951056],
    [-0.681718, 0.147621, 0.716567], [-0.809017, 0.309017, 0.500000], [-0.587785, 0.425325, 0.688191],
    [-0.850651, 0.525731, 0.000000], [-0.864188, 0.442863, 0.238856], [-0.716567, 0.681718, 0.147621],
    [-0.688191, 0.587785, 0.425325], [-0.500000, 0.809017, 0.309017], [-0.238856, 0.864188, 0.442863],
    [-0.425325, 0.688191, 0.587785], [-0.716567, 0.681718, -0.147621], [-0.500000, 0.809017, -0.309017],
    [-0.525731, 0.850651, 0.000000], [0.000000, 0.850651, -0.525731], [-0.238856, 0.864188, -0.442863],
    [0.000000, 0.955423, -0.295242], [-0.262866, 0.951056, -0.162460], [0.000000, 1.000000, 0.000000],
    [0.000000, 0.955423, 0.295242], [-0.262866, 0.951056, 0.162460], [0.238856, 0.864188, 0.442863],
    [0.262866, 0.951056, 0.162460], [0.500000, 0.809017, 0.309017], [0.238856, 0.864188, -0.442863],
    [0.262866, 0.951056, -0.162460], [0.500000, 0.809017, -0.309017], [0.850651, 0.525731, 0.000000],
    [0.716567, 0.681718, 0.147621], [0.716567, 0.681718, -0.147621], [0.525731, 0.850651, 0.000000],
    [0.425325, 0.688191, 0.587785], [0.864188, 0.442863, 0.238856], [0.688191, 0.587785, 0.425325],
    [0.809017, 0.309017, 0.500000], [0.681718, 0.147621, 0.716567], [0.587785, 0.425325, 0.688191],
    [0.955423, 0.295242, 0.000000], [1.000000, 0.000000, 0.000000], [0.951056, 0.162460, 0.262866],
    [0.850651, -0.525731, 0.000000], [0.955423, -0.295242, 0.000000], [0.864188, -0.442863, 0.238856],
    [0.951056, -0.162460, 0.262866], [0.809017, -0.309017, 0.500000], [0.681718, -0.147621, 0.716567],
    [0.850651, 0.000000, 0.525731], [0.864188, 0.442863, -0.238856], [0.809017, 0.309017, -0.500000],
    [0.951056, 0.162460, -0.262866], [0.525731, 0.000000, -0.850651], [0.681718, 0.147621, -0.716567],
    [0.681718, -0.147621, -0.716567], [0.850651, 0.000000, -0.525731], [0.809017, -0.309017, -0.500000],
    [0.864188, -0.442863, -0.238856], [0.951056, -0.162460, -0.262866], [0.147621, 0.716567, -0.681718],
    [0.309017, 0.500000, -0.809017], [0.425325, 0.688191, -0.587785], [0.442863, 0.238856, -0.864188],
    [0.587785, 0.425325, -0.688191], [0.688191, 0.587785, -0.425325], [-0.147621, 0.716567, -0.681718],
    [-0.309017, 0.500000, -0.809017], [0.000000, 0.525731, -0.850651], [-0.525731, 0.000000, -0.850651],
    [-0.442863, 0.238856, -0.864188], [-0.295242, 0.000000, -0.955423], [-0.162460, 0.262866, -0.951056],
    [0.000000, 0.000000, -1.000000], [0.295242, 0.000000, -0.955423], [0.162460, 0.262866, -0.951056],
    [-0.442863, -0.238856, -0.864188], [-0.309017, -0.500000, -0.809017], [-0.162460, -0.262866, -0.951056],
    [0.000000, -0.850651, -0.525731], [-0.147621, -0.716567, -0.681718], [0.147621, -0.716567, -0.681718],
    [0.000000, -0.525731, -0.850651], [0.309017, -0.500000, -0.809017], [0.442863, -0.238856, -0.864188],
    [0.162460, -0.262866, -0.951056], [0.238856, -0.864188, -0.442863], [0.500000, -0.809017, -0.309017],
    [0.425325, -0.688191, -0.587785], [0.716567, -0.681718, -0.147621], [0.688191, -0.587785, -0.425325],
    [0.587785, -0.425325, -0.688191], [0.000000, -0.955423, -0.295242], [0.000000, -1.000000, 0.000000],
    [0.262866, -0.951056, -0.162460], [0.000000, -0.850651, 0.525731], [0.000000, -0.955423, 0.295242],
    [0.238856, -0.864188, 0.442863], [0.262866, -0.951056, 0.162460], [0.500000, -0.809017, 0.309017],
    [0.716567, -0.681718, 0.147621], [0.525731, -0.850651, 0.000000], [-0.238856, -0.864188, -0.442863],
    [-0.500000, -0.809017, -0.309017], [-0.262866, -0.951056, -0.162460], [-0.850651, -0.525731, 0.000000],
    [-0.716567, -0.681718, -0.147621], [-0.716567, -0.681718, 0.147621], [-0.525731, -0.850651, 0.000000],
    [-0.500000, -0.809017, 0.309017], [-0.238856, -0.864188, 0.442863], [-0.262866, -0.951056, 0.162460],
    [-0.864188, -0.442863, 0.238856], [-0.809017, -0.309017, 0.500000], [-0.688191, -0.587785, 0.425325],
    [-0.681718, -0.147621, 0.716567], [-0.442863, -0.238856, 0.864188], [-0.587785, -0.425325, 0.688191],
    [-0.309017, -0.500000, 0.809017], [-0.147621, -0.716567, 0.681718], [-0.425325, -0.688191, 0.587785],
    [-0.162460, -0.262866, 0.951056], [0.442863, -0.238856, 0.864188], [0.162460, -0.262866, 0.951056],
    [0.309017, -0.500000, 0.809017], [0.147621, -0.716567, 0.681718], [0.000000, -0.525731, 0.850651],
    [0.425325, -0.688191, 0.587785], [0.587785, -0.425325, 0.688191], [0.688191, -0.587785, 0.425325],
    [-0.955423, 0.295242, 0.000000], [-0.951056, 0.162460, 0.262866], [-1.000000, 0.000000, 0.000000],
    [-0.850651, 0.000000, 0.525731], [-0.955423, -0.295242, 0.000000], [-0.951056, -0.162460, 0.262866],
    [-0.864188, 0.442863, -0.238856], [-0.951056, 0.162460, -0.262866], [-0.809017, 0.309017, -0.500000],
    [-0.864188, -0.442863, -0.238856], [-0.951056, -0.162460, -0.262866], [-0.809017, -0.309017, -0.500000],
    [-0.681718, 0.147621, -0.716567], [-0.681718, -0.147621, -0.716567], [-0.850651, 0.000000, -0.525731],
    [-0.688191, 0.587785, -0.425325], [-0.587785, 0.425325, -0.688191], [-0.425325, 0.688191, -0.587785],
    [-0.425325, -0.688191, -0.587785], [-0.587785, -0.425325, -0.688191], [-0.688191, -0.587785, -0.425325],
];

/// `ByteToDir`. Out of range yields zero, like the client's
/// (`cgame_mp_x86.dll 0x300399b0`), not the server's entry 0.
pub fn byte_to_dir(b: i32) -> [f32; 3] {
    if (0..BYTE_DIRS.len() as i32).contains(&b) {
        BYTE_DIRS[b as usize]
    } else {
        [0.0, 0.0, 0.0]
    }
}

/// Nearest table entry by dot product: what a server writes into `eventParm`
/// when it emits a wall hit from its own trace normal.
pub fn dir_to_byte(dir: [f32; 3]) -> i32 {
    let mut best = 0;
    let mut best_dot = f32::MIN;
    for (i, v) in BYTE_DIRS.iter().enumerate() {
        let d = v[0] * dir[0] + v[1] * dir[1] + v[2] * dir[2];
        if d > best_dot {
            best_dot = d;
            best = i as i32;
        }
    }
    best
}

pub struct EventTracker {
    /// Per-entity eventSequence at the last drain.
    seq: HashMap<u32, i32>,
    /// Fired event entities: number to (eType, eventParm), so a reused slot
    /// with new content refires and a lingering entity does not.
    fired: HashMap<u32, (i32, i32)>,
    ps_seq: Option<i32>,
    last_message: Option<u32>,
}

/// New events between `prev` and `cur`. `eventSequence` is 8 bits on the wire
/// and wraps; a plain `cur > prev` would stop replaying after the first wrap.
/// The ring size divides 256, so `seq & 3` stays aligned across a wrap.
fn seq_diff(cur: i32, prev: i32) -> i32 {
    (cur - prev) & 0xff
}

impl EventTracker {
    pub fn new() -> EventTracker {
        EventTracker {
            seq: HashMap::new(),
            fired: HashMap::new(),
            ps_seq: None,
            last_message: None,
        }
    }

    /// Events fired since the last drained snapshot. Idempotent per
    /// message_num.
    pub fn drain(&mut self, snap: &Snapshot, p: &Protocol) -> Vec<GameEvent> {
        if self.last_message == Some(snap.message_num) {
            return Vec::new();
        }
        self.last_message = Some(snap.message_num);
        let mut out = Vec::new();

        for (&num, es) in &snap.entities {
            let etype = es.field_i32(p, "eType");
            if etype >= ET_EVENTS {
                let key = (etype, es.field_i32(p, "eventParm"));
                if self.fired.get(&num) != Some(&key) {
                    self.fired.insert(num, key);
                    out.push(Self::event_from(es, p, num, etype - ET_EVENTS, key.1));
                }
                continue;
            }
            let cur = es.field_i32(p, "eventSequence");
            match self.seq.insert(num, cur) {
                None => {} // first sight: record, don't replay the ring
                Some(prev) => {
                    let diff = seq_diff(cur, prev).min(EVENT_RING);
                    for i in (cur - diff + 1)..=cur {
                        let slot = (i & 3) as usize;
                        let ev = es.field_i32(p, &format!("events[{slot}]"));
                        let parm = es.field_i32(p, &format!("eventParms[{slot}]"));
                        out.push(Self::event_from(es, p, num, ev, parm));
                    }
                }
            }
        }
        // Forget entities gone from this snapshot so slot reuse refires.
        self.seq.retain(|n, _| snap.entities.contains_key(n));
        self.fired.retain(|n, _| snap.entities.contains_key(n));

        // playerState ring; entity_num u32::MAX marks the view.
        let cur = snap.ps.field_i32(p, "eventSequence");
        match self.ps_seq.replace(cur) {
            None => {}
            Some(prev) => {
                let diff = seq_diff(cur, prev).min(EVENT_RING);
                for i in (cur - diff + 1)..=cur {
                    let slot = (i & 3) as usize;
                    out.push(GameEvent {
                        event: snap.ps.field_i32(p, &format!("events[{slot}]")),
                        parm: snap.ps.field_i32(p, &format!("eventParms[{slot}]")),
                        entity_num: u32::MAX,
                        client_num: snap.ps.field_i32(p, "clientNum"),
                        weapon: snap.ps.field_i32(p, "weapon"),
                        surf_type: 0,
                        pos: snap.ps.origin(p),
                        dir: [0.0; 3],
                        other_entity_num: u32::MAX,
                        attacker_entity_num: -1,
                    });
                }
            }
        }
        out
    }

    fn event_from(es: &EntityState, p: &Protocol, num: u32, event: i32, parm: i32) -> GameEvent {
        GameEvent {
            event,
            parm,
            entity_num: num,
            client_num: es.field_i32(p, "clientNum"),
            weapon: es.field_i32(p, "weapon"),
            surf_type: es.field_i32(p, "surfType"),
            pos: es.origin(p),
            dir: [
                es.field_f32(p, "origin2[0]"),
                es.field_f32(p, "origin2[1]"),
                es.field_f32(p, "origin2[2]"),
            ],
            other_entity_num: es.field_i32(p, "otherEntityNum") as u32,
            attacker_entity_num: es.field_i32(p, "attackerEntityNum"),
        }
    }
}

impl Default for EventTracker {
    fn default() -> EventTracker {
        EventTracker::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::msg::{EntityState, PlayerState};
    use crate::net::protocol::PROTOCOL_V1;
    use crate::net::snapshot::Snapshot;
    use std::collections::BTreeMap;

    fn ent(p: &Protocol, num: u32, fields: &[(&str, i32)]) -> EntityState {
        let mut e = EntityState::null(p);
        e.number = num;
        for (name, v) in fields {
            e.fields[EntityState::field_index(p, name).unwrap()] = *v;
        }
        e
    }

    fn snap(p: &Protocol, msg: u32, time: i32, ents: Vec<EntityState>) -> Snapshot {
        Snapshot {
            server_time: time,
            message_num: msg,
            delta_num: -1,
            snap_flags: 0,
            ps: PlayerState::null(p),
            entities: ents.into_iter().map(|e| (e.number, e)).collect(),
            clients: BTreeMap::new(),
            valid: true,
        }
    }

    #[test]
    fn entity_event_ring_fires_new_events_once() {
        let p = &PROTOCOL_V1;
        let mut t = EventTracker::new();
        // First sight: seq 2 with stale events must NOT fire (PVS enter).
        let s1 = snap(
            p,
            1,
            1000,
            vec![ent(
                p,
                5,
                &[("eventSequence", 2), ("events[0]", 9), ("events[1]", 9)],
            )],
        );
        assert!(t.drain(&s1, p).is_empty());
        // seq 2 -> 3: exactly events[3 & 3] fires.
        let s2 = snap(
            p,
            2,
            1050,
            vec![ent(
                p,
                5,
                &[
                    ("eventSequence", 3),
                    ("events[3]", 7),
                    ("eventParms[3]", 4),
                    ("weapon", 2),
                ],
            )],
        );
        let evs = t.drain(&s2, p);
        assert_eq!(evs.len(), 1);
        assert_eq!(
            (evs[0].event, evs[0].parm, evs[0].weapon, evs[0].entity_num),
            (7, 4, 2, 5)
        );
        // Same snapshot again: idempotent.
        assert!(t.drain(&s2, p).is_empty());
    }

    #[test]
    fn dropped_snapshot_catches_up_at_most_ring_size() {
        let p = &PROTOCOL_V1;
        let mut t = EventTracker::new();
        let s1 = snap(p, 1, 1000, vec![ent(p, 5, &[("eventSequence", 0)])]);
        t.drain(&s1, p);
        // seq jumps 0 -> 6: only the last 4 (ring size) can be recovered.
        let s2 = snap(
            p,
            4,
            1200,
            vec![ent(
                p,
                5,
                &[
                    ("eventSequence", 6),
                    ("events[3]", 11),
                    ("events[0]", 12),
                    ("events[1]", 13),
                    ("events[2]", 14),
                ],
            )],
        );
        let evs = t.drain(&s2, p);
        assert_eq!(
            evs.iter().map(|e| e.event).collect::<Vec<_>>(),
            vec![11, 12, 13, 14]
        );
    }

    #[test]
    fn event_entity_fires_on_appearance_only() {
        let p = &PROTOCOL_V1;
        let mut t = EventTracker::new();
        let ev_ent = |num, etype, parm| ent(p, num, &[("eType", etype), ("eventParm", parm)]);
        // ET_EVENTS(12) + 3 = eType 15, event id 3.
        let s1 = snap(p, 1, 1000, vec![ev_ent(30, 15, 8)]);
        let evs = t.drain(&s1, p);
        assert_eq!(evs.len(), 1);
        assert_eq!((evs[0].event, evs[0].parm, evs[0].entity_num), (3, 8, 30));
        // Still present next snapshot (EVENT_VALID_MSEC lingering): no refire.
        let s2 = snap(p, 2, 1050, vec![ev_ent(30, 15, 8)]);
        assert!(t.drain(&s2, p).is_empty());
        // Gone, then the slot is reused for a new event: fires again.
        let s3 = snap(p, 3, 1100, vec![]);
        t.drain(&s3, p);
        let s4 = snap(p, 4, 1150, vec![ev_ent(30, 14, 2)]);
        let evs = t.drain(&s4, p);
        assert_eq!(evs.len(), 1);
        assert_eq!((evs[0].event, evs[0].parm), (2, 2));
    }

    #[test]
    fn event_entity_carries_other_entity_num_for_the_shooter() {
        let p = &PROTOCOL_V1;
        let mut t = EventTracker::new();
        // ET_EVENTS(12) + 173 (EV_BULLET_HIT_SMALL) = eType 185.
        let s1 = snap(
            p,
            1,
            1000,
            vec![ent(
                p,
                40,
                &[("eType", 185), ("eventParm", 3), ("otherEntityNum", 7)],
            )],
        );
        let evs = t.drain(&s1, p);
        assert_eq!(evs.len(), 1);
        assert_eq!(
            (evs[0].event, evs[0].entity_num, evs[0].other_entity_num),
            (173, 40, 7)
        );
    }

    #[test]
    fn event_entity_carries_attacker_entity_num_for_obituaries() {
        let p = &PROTOCOL_V1;
        let mut t = EventTracker::new();
        // ET_EVENTS(12) + 201 (EV_OBITUARY) = eType 213.
        let s1 = snap(
            p,
            1,
            1000,
            vec![ent(
                p,
                50,
                &[
                    ("eType", 213),
                    ("eventParm", 3),
                    ("otherEntityNum", 5),
                    ("attackerEntityNum", 9),
                ],
            )],
        );
        let evs = t.drain(&s1, p);
        assert_eq!(evs.len(), 1);
        assert_eq!(
            (
                evs[0].event,
                evs[0].other_entity_num,
                evs[0].attacker_entity_num
            ),
            (201, 5, 9)
        );
    }

    #[test]
    fn playerstate_events_fire_and_reset_without_replay() {
        let p = &PROTOCOL_V1;
        let mut t = EventTracker::new();
        let mut s1 = snap(p, 1, 1000, vec![]);
        let fi = |n| PlayerState::field_index(p, n).unwrap();
        s1.ps.fields[fi("eventSequence")] = 2; // first sight: no replay
        s1.ps.fields[fi("events[0]")] = 9;
        assert!(t.drain(&s1, p).is_empty());
        let mut s2 = snap(p, 2, 1050, vec![]);
        s2.ps.fields[fi("eventSequence")] = 3;
        s2.ps.fields[fi("events[3]")] = 5;
        s2.ps.fields[fi("eventParms[3]")] = 1;
        let evs = t.drain(&s2, p);
        assert_eq!(evs.len(), 1);
        assert_eq!(
            (evs[0].event, evs[0].parm, evs[0].entity_num),
            (5, 1, u32::MAX)
        );
    }

    #[test]
    fn byte_to_dir_decodes_the_table_and_zeroes_out_of_range() {
        assert_eq!(byte_to_dir(0), [-0.525731, 0.0, 0.850651]);
        assert_eq!(byte_to_dir(161), [-0.688191, -0.587785, -0.425325]);
        assert_eq!(byte_to_dir(-1), [0.0, 0.0, 0.0]);
        assert_eq!(byte_to_dir(162), [0.0, 0.0, 0.0]);
    }

    #[test]
    fn byte_dirs_table_is_162_unit_vectors() {
        assert_eq!(BYTE_DIRS.len(), 162);
        for v in BYTE_DIRS {
            let len_sq: f32 = v.iter().map(|x| x * x).sum();
            assert!((len_sq - 1.0).abs() < 1e-4, "{v:?} len_sq={len_sq}");
        }
    }

    #[test]
    fn dir_to_byte_round_trips_every_entry() {
        for b in 0..162 {
            assert_eq!(dir_to_byte(byte_to_dir(b)), b, "entry {b}");
        }
    }

    #[test]
    fn dir_to_byte_snaps_a_trace_normal_to_the_nearest_entry() {
        let decoded = byte_to_dir(dir_to_byte([0.0, 0.0, 1.0]));
        assert!(decoded[2] > 0.85, "{decoded:?}");
    }

    #[test]
    fn entity_event_sequence_wraps_at_256() {
        let p = &PROTOCOL_V1;
        let mut t = EventTracker::new();
        let s1 = snap(p, 1, 1000, vec![ent(p, 5, &[("eventSequence", 254)])]);
        t.drain(&s1, p);
        // 254 -> 1 (wrapped through 255/0): three new events, seq 255, 0, 1.
        let s2 = snap(
            p,
            2,
            1050,
            vec![ent(
                p,
                5,
                &[
                    ("eventSequence", 1),
                    ("events[3]", 21),
                    ("events[0]", 22),
                    ("events[1]", 23),
                ],
            )],
        );
        let evs = t.drain(&s2, p);
        assert_eq!(
            evs.iter().map(|e| e.event).collect::<Vec<_>>(),
            vec![21, 22, 23]
        );
    }
}
