//! Temp entities: one entity that exists for a single frame to carry an
//! event to clients. Retail's `G_TempEntity` allocates a gentity, sets
//! `s.eType = ET_EVENTS + event` and frees it the frame after, so nothing
//! ever compares one frame's number against the next's
//! (`docs/research/cod11-hud-protocol.md` section 1 for the obituary, which
//! is the first of them vcod raises).

use vcod_common::net::msg::EntityState;
use vcod_common::net::protocol::{Protocol, ENTITYNUM_WORLD};

/// `ET_EVENTS`, the base an event entity's `eType` is offset from. 12 on
/// CoD 1.1 MP, not Q3's 13 (`docs/research/cod11-events-and-fx.md` section 1).
pub const ET_EVENTS: i32 = 12;

/// The first entity number a temp entity takes. The 64 numbers below
/// `ENTITYNUM_WORLD` are reused every frame; the split of the whole range is
/// documented in `crate::game::wire`.
pub const TEMP_FIRST: u32 = ENTITYNUM_WORLD - TEMP_COUNT;
/// How many temp entities one frame can carry.
pub const TEMP_COUNT: u32 = 64;

/// The block sits below `ENTITYNUM_WORLD` and clear of both the body queue
/// and the map entities, so a temp entity's number can never be read as one
/// of those. `crate::game::wire` documents the whole split.
const _: () = {
    assert!(TEMP_FIRST == 958);
    assert!(TEMP_FIRST + TEMP_COUNT <= ENTITYNUM_WORLD);
    assert!(
        TEMP_FIRST > crate::game::bodies::BODY_FIRST + crate::game::bodies::BODY_QUEUE_SIZE as u32
    );
    assert!(TEMP_FIRST > crate::game::entity::FIRST_MAP_ENTITY);
};

/// Who a temp entity is sent to. Retail spells this in `r.svFlags`:
/// `SVF_BROADCAST` (8) sends to everyone regardless of PVS, and the
/// single-client flags send to or withhold from one
/// (`docs/protocol-1.1.md`, "Which entities a client is sent").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    Broadcast,
    AllBut(usize),
    Only(usize),
}

/// One event to put on the wire this frame.
pub struct TempEntity {
    /// The `EV_*` number, not the `eType`: `build` adds `ET_EVENTS`.
    pub event: i32,
    pub parm: i32,
    pub surf_type: i32,
    /// `otherEntityNum`, the victim for an obituary.
    pub other: u32,
    /// `attackerEntityNum`, `ENTITYNUM_WORLD` when there is no player.
    pub attacker: i32,
    pub origin: [f32; 3],
    pub scope: Scope,
}

/// The entity state one temp entity puts on the wire at `number`.
pub fn build(te: &TempEntity, number: u32, p: &Protocol) -> EntityState {
    let mut e = EntityState::null(p);
    e.number = number;
    let mut set = |name: &str, v: i32| {
        if let Some(i) = EntityState::field_index(p, name) {
            e.fields[i] = v;
        }
    };
    set("eType", ET_EVENTS + te.event);
    set("eventParm", te.parm);
    set("surfType", te.surf_type);
    set("otherEntityNum", te.other as i32);
    set("attackerEntityNum", te.attacker);
    for (axis, v) in te.origin.iter().enumerate() {
        set(&format!("pos.trBase[{axis}]"), v.to_bits() as i32);
    }
    e
}

/// Whether one client's snapshot may carry this temp entity at all. A
/// `Broadcast` one still skips the PVS cull; the two scoped ones are culled
/// like any other entity once this says yes (`crate::server`).
pub fn visible_to(te: &TempEntity, slot: usize) -> bool {
    match te.scope {
        Scope::Broadcast => true,
        Scope::AllBut(s) => s != slot,
        Scope::Only(s) => s == slot,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vcod_common::net::protocol::PROTOCOL_V1;

    fn flesh_hit() -> TempEntity {
        TempEntity {
            event: 175,
            parm: 0,
            surf_type: 1,
            other: 0,
            attacker: 0,
            origin: [0.0; 3],
            scope: Scope::Broadcast,
        }
    }

    #[test]
    fn an_obituary_temp_entity_carries_victim_attacker_and_parm() {
        let te = TempEntity {
            event: 201,
            parm: 12,
            surf_type: 0,
            other: 3,
            attacker: 5,
            origin: [1.0, 2.0, 3.0],
            scope: Scope::Broadcast,
        };
        let p = &PROTOCOL_V1;
        let e = build(&te, 900, p);
        assert_eq!(e.number, 900);
        assert_eq!(e.field_i32(p, "eType"), 12 + 201);
        assert_eq!(e.field_i32(p, "eventParm"), 12);
        assert_eq!(e.field_i32(p, "otherEntityNum"), 3);
        assert_eq!(e.field_i32(p, "attackerEntityNum"), 5);
        assert_eq!(e.origin(p), [1.0, 2.0, 3.0]);
    }

    #[test]
    fn scope_all_but_hides_from_one_client() {
        let te = TempEntity {
            scope: Scope::AllBut(2),
            ..flesh_hit()
        };
        assert!(visible_to(&te, 0));
        assert!(!visible_to(&te, 2));
    }

    #[test]
    fn scope_only_reaches_one_client() {
        let te = TempEntity {
            scope: Scope::Only(2),
            ..flesh_hit()
        };
        assert!(!visible_to(&te, 0));
        assert!(visible_to(&te, 2));
        assert!(visible_to(&flesh_hit(), 0));
    }
}
