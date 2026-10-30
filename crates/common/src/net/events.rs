//! Snapshot event extraction: entity event rings and one-shot event entities
//! to `GameEvent`s, each fired once. docs/research/cod11-events-and-fx.md section 1.

use std::collections::HashMap;

use crate::net::flags::{EF_CORPSE_FRESH, EF_TELEPORT_BIT, ET_CORPSE, ET_GENERAL, ET_MISSILE};
use crate::net::msg::EntityState;
use crate::net::protocol::Cod1Wire;
use crate::net::snapshot::Snapshot;

/// `ET_EVENTS`, the base an event entity's `eType` is offset from. 12 on
/// CoD 1.1 MP, not Q3's 13 (CoDExtended shared.h:445; section 1); 16 on
/// United Offensive ([`Cod1Wire::et_events`]).
pub const ET_EVENTS: i32 = 12;
/// Slots in an entity's or the playerstate's event ring.
pub const EVENT_RING: i32 = 4;

pub struct GameEvent {
    /// EV_* id (doc section 1), in 1.1's numbering whatever the protocol
    /// ([`Cod1Wire::event_to_v1`]).
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
    /// `es.scale` (offset 216, 8 bits): `EV_BULLET_HIT_*`'s reflected
    /// direction and `EV_PLAY_FX_DIR`'s forward, each a byte dir (doc
    /// sections 2 and 3). 0 on playerState events.
    pub scale: i32,
    /// `otherEntityNum`: the shooter on `EV_BULLET_HIT_*` (doc section 2),
    /// the victim on `EV_OBITUARY` (docs/research/cod11-hud-protocol.md
    /// section 1). u32::MAX on playerState events.
    pub other_entity_num: u32,
    /// `attackerEntityNum`: `EV_OBITUARY`'s attacker, 1022 (`ENTITYNUM_WORLD`)
    /// for a world kill (hud-protocol doc section 1). -1 on playerState events.
    pub attacker_entity_num: i32,
    /// The carrier's `eType` as the wire has it (an event entity's is
    /// `ET_EVENTS + id`); `ET_PLAYER` on playerState events.
    pub e_type: i32,
    /// The carrier's `eFlags`, in 1.1's layout ([`Cod1Wire::eflags_to_v1`]).
    pub eflags: i32,
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
    /// Per-entity eventSequence and `eFlags` teleport bit at the last drain.
    seq: HashMap<u32, (i32, i32)>,
    /// Fired event entities: number to (eType, eventParm), so a reused slot
    /// with new content refires and a lingering entity does not.
    fired: HashMap<u32, (i32, i32)>,
    /// The playerstate's `eventSequence` and `events[]` at the last drain.
    ps_prev: Option<(i32, [i32; 4])>,
    last_message: Option<u32>,
}

/// New events between `prev` and `cur`. `eventSequence` is 8 bits on the wire
/// and wraps; a plain `cur > prev` would stop replaying after the first wrap.
/// The ring size divides 256, so `seq & 3` stays aligned across a wrap.
///
/// The counter is incremented *after* the write (`G_AddEvent`,
/// docs/research/cod11-combat.md section 7), so the events new in this frame
/// sit at sequences `prev ..= cur - 1`, not `prev + 1 ..= cur`. Reading the
/// latter is one slot high and drops the oldest of them: a death frame
/// arriving as `eventSequence` 2 with `events` `[189, 155]` read as 155 and an
/// empty slot, and `EV_DEATH` never reached a consumer.
fn seq_diff(cur: i32, prev: i32) -> i32 {
    (cur - prev) & 0xff
}

/// Whether `CG_ResetEntity` starts this entity's ring over from 0, so an
/// entity first seen with events on it plays them: a general entity, a
/// missile, and a corpse while its fresh bit is up. Every other type starts
/// from the sequence it arrives with (cod11-events-and-fx.md 1, "An entity
/// that enters the snapshot").
fn replays_on_reset(etype: i32, eflags: i32) -> bool {
    match etype {
        ET_GENERAL | ET_MISSILE => true,
        ET_CORPSE => eflags & EF_CORPSE_FRESH != 0,
        _ => false,
    }
}

/// Whether the playerstate ring's slot at sequence `i` plays: the ring half
/// of `CG_TransitionPlayerState` (cgame 1.1 0x30028be0, 1.5 0x30029f50, UO
/// 0x30034ec0) plays a slot at or past the old `eventSequence`, or one
/// within the old ring whose event differs from the old playerstate's.
/// Compared raw on the 8-bit wire values, so a follow switch onto a lower
/// sequence plays nothing old (cod11-spectator-follow.md 11.1).
fn ps_slot_plays(i: i32, prev: i32, event: i32, old_event: i32) -> bool {
    i >= prev || (i > prev - EVENT_RING && event != old_event)
}

impl EventTracker {
    pub fn new() -> EventTracker {
        EventTracker {
            seq: HashMap::new(),
            fired: HashMap::new(),
            ps_prev: None,
            last_message: None,
        }
    }

    /// Events fired since the last drained snapshot. Idempotent per
    /// message_num.
    pub fn drain(&mut self, snap: &Snapshot, p: &Cod1Wire) -> Vec<GameEvent> {
        self.drain_seq(snap, p)
            .into_iter()
            .map(|(_, ev)| ev)
            .collect()
    }

    /// The playerstate ring's `eventSequence` at the last drain.
    pub fn ps_sequence(&self) -> Option<i32> {
        self.ps_prev.map(|(seq, _)| seq)
    }

    /// [`Self::drain`], each playerstate-ring event with the 8-bit sequence
    /// it was written at; `None` for entity events.
    pub fn drain_seq(&mut self, snap: &Snapshot, p: &Cod1Wire) -> Vec<(Option<i32>, GameEvent)> {
        if self.last_message == Some(snap.message_num) {
            return Vec::new();
        }
        self.last_message = Some(snap.message_num);
        let mut out = Vec::new();

        for (&num, es) in &snap.entities {
            let etype = es.field_i32(p, "eType");
            if let Some(event) = p.event_of(etype) {
                let key = (etype, es.field_i32(p, "eventParm"));
                if self.fired.get(&num) != Some(&key) {
                    self.fired.insert(num, key);
                    out.push((None, Self::event_from(es, p, num, event, key.1)));
                }
                continue;
            }
            let cur = es.field_i32(p, "eventSequence");
            let eflags = es.field_i32(p, "eFlags");
            let teleport = eflags & EF_TELEPORT_BIT;
            let prev = match self.seq.insert(num, (cur, teleport)) {
                Some((prev, t)) if t == teleport => prev,
                // `CG_ResetEntity`: first sight or a toggled teleport bit.
                _ if replays_on_reset(etype, eflags) => 0,
                _ => cur,
            };
            let diff = seq_diff(cur, prev).min(EVENT_RING);
            for i in (cur - diff)..cur {
                let slot = (i & 3) as usize;
                let ev = (p.event_to_v1)(es.field_i32(p, &format!("events[{slot}]")));
                let parm = es.field_i32(p, &format!("eventParms[{slot}]"));
                out.push((None, Self::event_from(es, p, num, ev, parm)));
            }
        }
        // Forget entities gone from this snapshot so slot reuse refires.
        self.seq.retain(|n, _| snap.entities.contains_key(n));
        self.fired.retain(|n, _| snap.entities.contains_key(n));

        // playerState ring; entity_num u32::MAX marks the view.
        let cur = snap.ps.field_i32(p, "eventSequence");
        let events: [i32; 4] =
            std::array::from_fn(|slot| snap.ps.field_i32(p, &format!("events[{slot}]")));
        if let Some((prev, old)) = self.ps_prev.replace((cur, events)) {
            for i in (cur - EVENT_RING)..cur {
                let slot = (i & 3) as usize;
                if !ps_slot_plays(i, prev, events[slot], old[slot]) {
                    continue;
                }
                out.push((
                    Some(i & 0xff),
                    GameEvent {
                        event: (p.event_to_v1)(events[slot]),
                        parm: snap.ps.field_i32(p, &format!("eventParms[{slot}]")),
                        entity_num: u32::MAX,
                        client_num: snap.ps.field_i32(p, "clientNum"),
                        weapon: snap.ps.field_i32(p, "weapon"),
                        surf_type: 0,
                        pos: snap.ps.origin(p),
                        dir: [0.0; 3],
                        scale: 0,
                        other_entity_num: u32::MAX,
                        attacker_entity_num: -1,
                        e_type: crate::net::flags::ET_PLAYER,
                        eflags: snap.ps.field_i32(p, "eFlags"),
                    },
                ));
            }
        }
        out
    }

    fn event_from(es: &EntityState, p: &Cod1Wire, num: u32, event: i32, parm: i32) -> GameEvent {
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
            scale: es.field_i32(p, "_union.scale"),
            other_entity_num: es.field_i32(p, "otherEntityNum") as u32,
            attacker_entity_num: es.field_i32(p, "attackerEntityNum"),
            e_type: es.field_i32(p, "eType"),
            eflags: es.field_i32(p, "eFlags"),
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
    use crate::net::flags::ET_PLAYER;
    use crate::net::msg::{EntityState, PlayerState};
    use crate::net::protocol::PROTOCOL_V1;
    use crate::net::snapshot::Snapshot;
    use std::collections::BTreeMap;

    fn ent(p: &Cod1Wire, num: u32, fields: &[(&str, i32)]) -> EntityState {
        let mut e = EntityState::null(p);
        e.number = num;
        for (name, v) in fields {
            e.fields[EntityState::field_index(p, name).unwrap()] = *v;
        }
        e
    }

    fn snap(p: &Cod1Wire, msg: u32, time: i32, ents: Vec<EntityState>) -> Snapshot {
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
        // First sight of a player: seq 2 with stale events must NOT fire
        // (PVS enter).
        let s1 = snap(
            p,
            1,
            1000,
            vec![ent(
                p,
                5,
                &[
                    ("eType", ET_PLAYER),
                    ("eventSequence", 2),
                    ("events[0]", 9),
                    ("events[1]", 9),
                ],
            )],
        );
        assert!(t.drain(&s1, p).is_empty());
        // seq 2 -> 3: the write went to slot 2 & 3 and the counter followed it.
        let s2 = snap(
            p,
            2,
            1050,
            vec![ent(
                p,
                5,
                &[
                    ("eType", ET_PLAYER),
                    ("eventSequence", 3),
                    ("events[2]", 7),
                    ("eventParms[2]", 4),
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

    /// A missile first seen already exploded (a tank shell striking within
    /// its first step) plays the explode; a general entity's ring replays
    /// from 0 too, capped at the ring; a corpse only while fresh.
    #[test]
    fn an_entering_missile_general_or_fresh_corpse_replays_its_ring() {
        let p = &PROTOCOL_V1;
        let mut t = EventTracker::new();
        let s1 = snap(
            p,
            1,
            1000,
            vec![
                ent(
                    p,
                    5,
                    &[
                        ("eType", ET_GENERAL),
                        ("eventSequence", 1),
                        ("events[0]", 9),
                        ("eventParms[0]", 32),
                    ],
                ),
                ent(
                    p,
                    6,
                    &[
                        ("eType", ET_MISSILE),
                        ("eventSequence", 6),
                        ("events[0]", 11),
                        ("events[1]", 12),
                        ("events[2]", 13),
                        ("events[3]", 14),
                    ],
                ),
                ent(
                    p,
                    7,
                    &[
                        ("eType", ET_CORPSE),
                        ("eFlags", EF_CORPSE_FRESH),
                        ("eventSequence", 1),
                        ("events[0]", 15),
                    ],
                ),
                ent(
                    p,
                    8,
                    &[
                        ("eType", ET_CORPSE),
                        ("eventSequence", 1),
                        ("events[0]", 16),
                    ],
                ),
            ],
        );
        let evs: Vec<_> = t
            .drain(&s1, p)
            .iter()
            .map(|e| (e.entity_num, e.event, e.parm))
            .collect();
        assert_eq!(
            evs,
            [
                (5, 9, 32),
                (6, 13, 0),
                (6, 14, 0),
                (6, 11, 0),
                (6, 12, 0),
                (7, 15, 0)
            ]
        );
    }

    /// A toggled teleport bit resets the ring as a first sight does: a
    /// respawned player's stale slots stay quiet.
    #[test]
    fn a_toggled_teleport_bit_resets_the_ring() {
        let p = &PROTOCOL_V1;
        let mut t = EventTracker::new();
        let player = |msg, seq, eflags| {
            snap(
                p,
                msg,
                1000 + 50 * msg as i32,
                vec![ent(
                    p,
                    5,
                    &[
                        ("eType", ET_PLAYER),
                        ("eFlags", eflags),
                        ("eventSequence", seq),
                        ("events[0]", 9),
                        ("events[1]", 9),
                    ],
                )],
            )
        };
        assert!(t.drain(&player(1, 0, 0), p).is_empty());
        assert!(t.drain(&player(2, 2, EF_TELEPORT_BIT), p).is_empty());
        assert_eq!(t.drain(&player(3, 3, EF_TELEPORT_BIT), p).len(), 1);
    }

    #[test]
    fn dropped_snapshot_catches_up_at_most_ring_size() {
        let p = &PROTOCOL_V1;
        let mut t = EventTracker::new();
        let s1 = snap(p, 1, 1000, vec![ent(p, 5, &[("eventSequence", 0)])]);
        t.drain(&s1, p);
        // seq jumps 0 -> 6: only the last 4 (ring size) can be recovered, the
        // writes at sequences 2, 3, 4 and 5, so slots 2, 3, 0 and 1.
        let s2 = snap(
            p,
            4,
            1200,
            vec![ent(
                p,
                5,
                &[
                    ("eventSequence", 6),
                    ("events[2]", 11),
                    ("events[3]", 12),
                    ("events[0]", 13),
                    ("events[1]", 14),
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
        s2.ps.fields[fi("events[0]")] = 9;
        s2.ps.fields[fi("events[2]")] = 5;
        s2.ps.fields[fi("eventParms[2]")] = 1;
        let evs = t.drain(&s2, p);
        assert_eq!(evs.len(), 1);
        assert_eq!(
            (evs[0].event, evs[0].parm, evs[0].entity_num),
            (5, 1, u32::MAX)
        );
    }

    /// The retail death frame, verbatim off the committed hit fixture:
    /// `eventSequence` 2 with `events` `[189, 155, 0, 0]` after a frame that
    /// read 0. Both are new, in that order. Reading the ring one slot high
    /// dropped `EV_DEATH` and handed the consumer an empty slot instead.
    #[test]
    fn playerstate_death_frame_drains_both_events_in_order() {
        let p = &PROTOCOL_V1;
        let mut t = EventTracker::new();
        let fi = |n| PlayerState::field_index(p, n).unwrap();
        let s1 = snap(p, 1, 1000, vec![]);
        assert!(t.drain(&s1, p).is_empty()); // seq 0, first sight
        let mut s2 = snap(p, 2, 1050, vec![]);
        s2.ps.fields[fi("eventSequence")] = 2;
        s2.ps.fields[fi("events[0]")] = 189; // EV_DEATH
        s2.ps.fields[fi("events[1]")] = 155; // EV_RAISE_WEAPON
        let evs = t.drain(&s2, p);
        assert_eq!(evs.iter().map(|e| e.event).collect::<Vec<_>>(), [189, 155]);
    }

    /// The sequence each playerstate-ring event was written at; entity
    /// events carry none.
    #[test]
    fn drain_seq_names_the_ring_sequence() {
        let p = &PROTOCOL_V1;
        let mut t = EventTracker::new();
        let fi = |n| PlayerState::field_index(p, n).unwrap();
        let mut s1 = snap(p, 1, 1000, vec![ent(p, 5, &[("eventSequence", 0)])]);
        s1.ps.fields[fi("eventSequence")] = 253;
        t.drain_seq(&s1, p);
        assert_eq!(t.ps_sequence(), Some(253));
        let mut s2 = snap(
            p,
            2,
            1050,
            vec![ent(p, 5, &[("eventSequence", 1), ("events[0]", 7)])],
        );
        s2.ps.fields[fi("eventSequence")] = 255;
        let seqs: Vec<_> = t.drain_seq(&s2, p).into_iter().map(|(s, _)| s).collect();
        assert_eq!(seqs, [None, Some(253), Some(254)]);
    }

    /// Retail compares the 8-bit wire sequences raw (cgame 0x30028be0), so
    /// the frame that wraps the playerstate ring from 254 to 1 plays nothing.
    #[test]
    fn the_playerstate_ring_plays_nothing_across_the_wire_wrap() {
        let p = &PROTOCOL_V1;
        let mut t = EventTracker::new();
        let fi = |n| PlayerState::field_index(p, n).unwrap();
        let mut s1 = snap(p, 1, 1000, vec![]);
        s1.ps.fields[fi("eventSequence")] = 254;
        t.drain(&s1, p);
        let mut s2 = snap(p, 2, 1050, vec![]);
        s2.ps.fields[fi("eventSequence")] = 1;
        s2.ps.fields[fi("events[2]")] = 159;
        s2.ps.fields[fi("events[0]")] = 159;
        assert!(t.drain(&s2, p).is_empty());
    }

    /// A follow switch onto a player whose sequence is lower plays none of
    /// his old events: the retail 1.1d follow capture's switch from client
    /// 2's ring (sequence 19) onto client 0's (cod11-spectator-follow.md
    /// 11.1), here with shots left in the new ring. A slot of the old window
    /// that changed still plays, as `CG_TransitionPlayerState` plays it.
    #[test]
    fn a_switch_onto_a_lower_sequence_plays_nothing_old() {
        let p = &PROTOCOL_V1;
        let mut t = EventTracker::new();
        let fi = |n| PlayerState::field_index(p, n).unwrap();
        let ring = |s: &mut Snapshot, seq: i32, events: [i32; 4]| {
            s.ps.fields[fi("eventSequence")] = seq;
            for (i, e) in events.into_iter().enumerate() {
                let name = format!("events[{i}]");
                s.ps.fields[PlayerState::field_index(p, &name).unwrap()] = e;
            }
        };
        let mut s1 = snap(p, 1, 1000, vec![]);
        ring(&mut s1, 19, [159, 159, 152, 159]);
        t.drain(&s1, p);
        let mut s2 = snap(p, 2, 1050, vec![]);
        ring(&mut s2, 2, [159, 159, 0, 0]);
        assert!(t.drain(&s2, p).is_empty(), "his shots at 0 and 1");
        let mut s3 = snap(p, 3, 1100, vec![]);
        ring(&mut s3, 0, [0; 4]);
        assert!(t.drain(&s3, p).is_empty(), "a respawn's empty ring");

        // Same sequence, one slot inside the old window changed.
        let mut s4 = snap(p, 4, 1150, vec![]);
        ring(&mut s4, 0, [0, 0, 0, 152]);
        let evs = t.drain_seq(&s4, p);
        assert_eq!(
            evs.iter().map(|(s, e)| (*s, e.event)).collect::<Vec<_>>(),
            [(Some(255), 152)]
        );
    }

    /// The same shape on an entity ring, which shares the walk.
    #[test]
    fn entity_death_frame_drains_both_events_in_order() {
        let p = &PROTOCOL_V1;
        let mut t = EventTracker::new();
        let s1 = snap(p, 1, 1000, vec![ent(p, 5, &[("eventSequence", 0)])]);
        assert!(t.drain(&s1, p).is_empty());
        let s2 = snap(
            p,
            2,
            1050,
            vec![ent(
                p,
                5,
                &[("eventSequence", 2), ("events[0]", 189), ("events[1]", 155)],
            )],
        );
        let evs = t.drain(&s2, p);
        assert_eq!(evs.iter().map(|e| e.event).collect::<Vec<_>>(), [189, 155]);
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
        // 254 -> 1 (wrapped through 255/0): three new events, written at
        // sequences 254, 255 and 0, so slots 2, 3 and 0.
        let s2 = snap(
            p,
            2,
            1050,
            vec![ent(
                p,
                5,
                &[
                    ("eventSequence", 1),
                    ("events[2]", 21),
                    ("events[3]", 22),
                    ("events[0]", 23),
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
