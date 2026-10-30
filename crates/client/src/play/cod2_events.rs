//! A CoD2 connection's events: `CG_CheckEvents` and
//! `CG_TransitionPlayerState` over each snapshot as it becomes the cgame's
//! current one (docs/research/cod2-sound.md 5, cod2-efx.md 6). Each event
//! comes out once, in CoD2's numbering ([`vcod_cod2::net::events`]), and
//! every reaction reads the same list: the sounds
//! (`play::cod2_sound::Cod2Sounds::cues`) and the effects
//! (`play::cod2_fx::Cod2Fx::frame`).

use std::collections::HashMap;

use glam::Vec3;
use vcod_cod2::net::events::{self as ev, ET_CORPSE, ET_EVENTS, ET_GENERAL, ET_MISSILE};
use vcod_cod2::net::flags::EF_TELEPORT_BIT;
use vcod_cod2::net::{EntityState2, SnapshotView};

/// A corpse whose `eFlags` carry this replays its ring on entering
/// (`CG_ResetEntity`, Mac 0x1deb10: the `cent[0xfa] & 8` test).
const EF_CORPSE_REPLAY: i32 = 0x80000;

/// One event as `CG_EntityEvent` receives it: the entity's state at the
/// snapshot that carried it, and the event's parm in `eventParm`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Event2 {
    pub entity: u32,
    pub event: i32,
    pub parm: i32,
    pub etype: i32,
    pub eflags: i32,
    /// 1-based into configstring 7.
    pub weapon: i32,
    /// `es.surfType`, the surface an impact or explosion struck.
    pub surf: i32,
    pub other_entity: i32,
    /// `es.time`.
    pub time: i32,
    /// `es.scale`: a bullet hit's reflected direction as a `ByteToDir`
    /// byte.
    pub scale: i32,
    /// `es.angles2[0..2]`: an earthquake's scale and radius.
    pub angles2: [f32; 2],
    /// Where the entity is at the snapshot's time.
    pub pos: Vec3,
    /// `apos.trBase` (pitch, yaw, roll degrees); the view angles for the
    /// playerstate's ring.
    pub angles: Vec3,
    /// From the playerstate's ring, not an entity's.
    pub from_ps: bool,
}

/// What [`EventTracker`] keeps of an entity between snapshots.
#[derive(Clone, Copy)]
struct Seen {
    /// `cent->previousEvent` (`cent + 0x1e4`); for a temp entity, whether
    /// its event already fired.
    prev: i32,
    /// `eFlags`, for the teleport bit's flip.
    eflags: i32,
}

/// `CG_CheckEvents` (Mac 0x1e614a) over each snapshot's entities, then
/// `CG_TransitionPlayerState` (Mac 0x1e4088) over its playerstate.
#[derive(Default)]
pub struct EventTracker {
    /// Every entity of the last snapshot walked.
    seen: HashMap<u32, Seen>,
    /// The last playerstate's client, sequence and ring.
    ps: Option<(i32, i32, [i32; 4])>,
    /// The snapshot last walked.
    last: Option<i32>,
    /// The local body is predicted: the playerstate ring's move events
    /// (`play::cod2_predict::predicted_event`) came off the prediction and
    /// are not raised again here.
    pub predicting: bool,
}

/// An entity state's `events[i & 3]` and `eventParms[i & 3]`.
fn ring(es: &EntityState2, i: i32) -> (i32, i32) {
    let k = i & 3;
    (
        es.i32(&format!("events[{k}]")),
        es.i32(&format!("eventParms[{k}]")),
    )
}

impl EventTracker {
    /// The events of `snap` once it is the current snapshot; nothing when
    /// it was already walked. A gamestate starts the tracker over.
    pub fn transition(&mut self, snap: &SnapshotView) -> Vec<Event2> {
        if self.last == Some(snap.message_num) {
            return Vec::new();
        }
        self.last = Some(snap.message_num);
        let mut out = Vec::new();
        let mut seen = HashMap::with_capacity(snap.entities.len());
        for e in &snap.entities {
            let es = &e.raw;
            let at = |event: i32, parm: i32| Event2 {
                entity: e.number,
                event,
                parm,
                etype: e.etype,
                eflags: e.eflags,
                weapon: e.weapon,
                surf: es.i32("surfType"),
                other_entity: es.i32("otherEntityNum"),
                time: es.i32("time"),
                scale: es.i32("scale"),
                angles2: [es.f32("angles2[0]"), es.f32("angles2[1]")],
                pos: e.pos.evaluate(snap.server_time),
                angles: e.apos.base,
                from_ps: false,
            };
            let now = es.i32("eventSequence");
            // `CG_SetNextSnap` (Mac 0x1dee08) resets an entity not in the
            // last snapshot, or whose teleport bit flipped.
            let prev = match self.seen.get(&e.number) {
                Some(s) if (s.eflags ^ e.eflags) & EF_TELEPORT_BIT == 0 => s.prev,
                // `CG_ResetEntity`: a general entity, a missile, or a
                // corpse flagged 0x80000 replays its ring; anything else,
                // temp entities included, starts at its sequence.
                _ => match e.etype {
                    ET_GENERAL | ET_MISSILE => 0,
                    ET_CORPSE if e.eflags & EF_CORPSE_REPLAY != 0 => 0,
                    _ => now,
                },
            };
            let prev = if e.etype > ET_EVENTS {
                // A temp entity fires once, while `previousEvent` is 0.
                if prev == 0 {
                    out.push(at(e.etype - ET_EVENTS, es.i32("eventParm")));
                }
                1
            } else if now == 0 {
                0
            } else {
                let mut prev = if now < prev { prev - 256 } else { prev };
                prev = prev.max(now - 4);
                for i in prev..now {
                    let (event, parm) = ring(es, i);
                    out.push(at(event, parm));
                }
                now
            };
            seen.insert(
                e.number,
                Seen {
                    prev,
                    eflags: e.eflags,
                },
            );
        }
        self.seen = seen;
        out.extend(self.ps_events(snap));
        out
    }

    /// `CG_TransitionPlayerState`: the playerstate ring's slots past the
    /// last sequence, or changed within its window. A follow switch copies
    /// the new playerstate over the old first (`CG_SetNextSnap`'s clientNum
    /// test), so it raises nothing.
    fn ps_events(&mut self, snap: &SnapshotView) -> Vec<Event2> {
        let ps = &snap.ps;
        let now = ps.raw.i32("eventSequence");
        let events = [0, 1, 2, 3].map(|k| ps.raw.i32(&format!("events[{k}]")));
        let old = self.ps.replace((ps.client_num, now, events));
        let Some((client, before, old_events)) = old else {
            return Vec::new();
        };
        if client != ps.client_num {
            return Vec::new();
        }
        let mut out = Vec::new();
        for i in now - 4..now {
            let k = (i & 3) as usize;
            if self.predicting && crate::play::cod2_predict::predicted_event(events[k]) {
                continue;
            }
            if i >= before || (i > before - 4 && events[k] != old_events[k]) {
                out.push(Event2 {
                    entity: ps.client_num as u32,
                    event: events[k],
                    parm: ps.raw.i32(&format!("eventParms[{k}]")),
                    etype: ev::ET_PLAYER,
                    eflags: ps.eflags,
                    weapon: ps.weapon,
                    surf: 0,
                    other_entity: 0,
                    time: snap.server_time,
                    scale: 0,
                    angles2: [0.0; 2],
                    pos: Vec3::from(ps.origin),
                    angles: Vec3::from(ps.viewangles),
                    from_ps: true,
                });
            }
        }
        out
    }
}

/// `Com_SurfaceTypeToName`: the surface names the cgame's per-surface
/// aliases and impact rows end in, in the footstep events' order.
pub fn surface_name(surf: i32) -> &'static str {
    usize::try_from(surf)
        .ok()
        .and_then(|i| SURFACES.get(i))
        .copied()
        .unwrap_or("default")
}

/// The surface types in `Com_SurfaceTypeToName` order; `surfType` indexes
/// it.
pub const SURFACES: [&str; 23] = [
    "default", "bark", "brick", "carpet", "cloth", "concrete", "dirt", "flesh", "foliage", "glass",
    "grass", "gravel", "ice", "metal", "mud", "paper", "plaster", "rock", "sand", "snow", "water",
    "wood", "asphalt",
];

#[cfg(test)]
mod tests {
    use super::*;
    use vcod_cod2::net::{EntityView, PlayerView};

    fn set(es: &mut EntityState2, k: &str, v: i32) {
        let i = vcod_cod2::net::delta::field_index(&vcod_cod2::net::fields::ENTITY_STATE_FIELDS, k);
        es.f[i] = v as u32;
    }

    fn snap(ents: &[EntityState2], t: i32) -> SnapshotView {
        SnapshotView {
            message_num: t,
            server_time: t,
            snap_flags: 0,
            ps: PlayerView::new(&Default::default(), 0),
            entities: ents.iter().map(EntityView::new).collect(),
            clients: Vec::new(),
        }
    }

    /// The surface names are the footstep events' own, in their order.
    #[test]
    fn surfaces_follow_the_footstep_events() {
        for (i, s) in SURFACES.iter().enumerate() {
            let name = ev::EV_NAMES[ev::EV_FOOTSTEP_RUN as usize + i];
            assert_eq!(
                name.strip_prefix("EV_FOOTSTEP_RUN_"),
                Some(&*s.to_uppercase())
            );
        }
        assert_eq!(surface_name(99), "default");
    }

    #[test]
    fn a_ring_replays_only_new_slots() {
        let mut es = EntityState2::null(5);
        set(&mut es, "eType", ev::ET_PLAYER);
        set(&mut es, "eventSequence", 3);
        set(&mut es, "events[2]", ev::EV_FIRE_WEAPON);
        let mut t = EventTracker::default();
        // A player entering starts at its sequence: nothing replays.
        assert!(t.transition(&snap(&[es], 50)).is_empty());
        set(&mut es, "eventSequence", 4);
        set(&mut es, "events[3]", ev::EV_RELOAD);
        let got = t.transition(&snap(&[es], 100));
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].event, ev::EV_RELOAD);
        // The same snapshot again raises nothing.
        assert!(t.transition(&snap(&[es], 100)).is_empty());
        // A teleport resets him: his sequence again, nothing replays.
        set(&mut es, "eventSequence", 5);
        set(&mut es, "eFlags", EF_TELEPORT_BIT);
        assert!(t.transition(&snap(&[es], 150)).is_empty());
    }

    /// A predicted body's shot kicks off the prediction's own fire event
    /// (`play::cod2_recoil`); the snapshot's copy in the playerstate ring
    /// is not raised again, so the kick never runs twice. An unpredicted
    /// body's comes off the ring.
    #[test]
    fn a_predicted_shot_is_not_raised_again_off_the_snapshot() {
        use vcod_cod2::net::PlayerState2;
        use vcod_cod2::net::delta::field_index;
        use vcod_cod2::net::fields::PLAYER_STATE_FIELDS;
        let ps_snap = |seq: u32, t: i32| {
            let mut raw = PlayerState2::default();
            let mut put = |n: &str, v: u32| raw.f[field_index(&PLAYER_STATE_FIELDS, n)] = v;
            put("eventSequence", seq);
            put("events[0]", ev::EV_FIRE_WEAPON as u32);
            SnapshotView {
                ps: PlayerView::new(&raw, 0),
                ..snap(&[], t)
            }
        };
        for predicting in [true, false] {
            let mut t = EventTracker {
                predicting,
                ..Default::default()
            };
            assert!(t.transition(&ps_snap(0, 50)).is_empty());
            let got: Vec<_> = t
                .transition(&ps_snap(1, 100))
                .iter()
                .map(|e| e.event)
                .collect();
            let want: &[i32] = if predicting {
                &[]
            } else {
                &[ev::EV_FIRE_WEAPON]
            };
            assert_eq!(got, want, "predicting {predicting}");
        }
    }

    /// A missile entering replays its ring from 0; a temp entity fires
    /// once however long it stays.
    #[test]
    fn missiles_replay_and_temp_entities_fire_once() {
        let mut m = EntityState2::null(80);
        set(&mut m, "eType", ET_MISSILE);
        set(&mut m, "eventSequence", 2);
        set(&mut m, "events[0]", ev::EV_GRENADE_BOUNCE);
        set(&mut m, "events[1]", ev::EV_GRENADE_EXPLODE);
        let mut hit = EntityState2::null(81);
        set(&mut hit, "eType", ET_EVENTS + ev::EV_BULLET_HIT_SMALL);
        set(&mut hit, "eventParm", 7);
        let mut t = EventTracker::default();
        let got = t.transition(&snap(&[m, hit], 50));
        let names: Vec<_> = got.iter().map(|e| (e.event, e.parm)).collect();
        assert_eq!(
            names,
            [
                (ev::EV_GRENADE_BOUNCE, 0),
                (ev::EV_GRENADE_EXPLODE, 0),
                (ev::EV_BULLET_HIT_SMALL, 7)
            ]
        );
        assert!(t.transition(&snap(&[m, hit], 100)).is_empty());
    }
}
