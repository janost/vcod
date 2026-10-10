//! Temp entities: an entity that carries one event to clients. Retail's
//! `G_TempEntity` (0x67938) takes a number from `G_Spawn`, sets
//! `s.eType = ET_EVENTS + event`, and `G_RunEntity` frees it once
//! `level.time` is more than [`EVENT_VALID_MS`] past the event; it rides
//! every snapshot until then (`docs/research/cod11-combat.md` 14.7,
//! "Entity numbers"; `docs/research/cod11-hud-protocol.md` section 1 for the
//! obituary, which is the first of them vcod raised).

use vcod_common::net::msg::EntityState;
use vcod_common::net::protocol::Protocol;
use vcod_gsc::EntId;

pub use vcod_common::net::events::ET_EVENTS;

/// `G_RunEntity`'s 300 (0x50309): a `freeAfterEvent` entity is freed on the
/// first turn more than this past its `eventTime`.
pub const EVENT_VALID_MS: i32 = 300;

/// A temp entity between its `G_TempEntity` and its free: the number the
/// object table gave it and the level time it was raised at.
pub struct LiveTemp {
    pub id: EntId,
    pub born_ms: i32,
    pub te: TempEntity,
}

/// Who a temp entity is sent to. Retail spells this in `r.svFlags`:
/// `SVF_BROADCAST` (8) sends to everyone regardless of PVS, and the
/// single-client flags send to or withhold from one
/// (`docs/protocol-1.1.md`, "Which entities a client is sent"). The two
/// single-client arms name a client number, which the snapshot compares with
/// its own `ps.clientNum`, not with the slot it is written for
/// (`docs/research/cod11-events-and-fx.md` section 2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    Broadcast,
    /// Everyone, culled by PVS: a bare `G_TempEntity` sets no `svFlags`.
    Pvs,
    /// `svFlags` 0x2000 with `r.singleClient`: every snapshot but this
    /// client's, culled by PVS.
    AllBut(usize),
    /// `svFlags` 0x800 with `r.singleClient`: this client's snapshot alone,
    /// culled by PVS.
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
    /// `weapon`. `G_TempEntity` zeroes the state, so only the events whose
    /// caller fills it in carry one: the melee hit and miss
    /// (`docs/research/cod11-combat.md` 2.5).
    pub weapon: i32,
    /// `clientNum`, which only the caller that fills it in carries.
    pub client_num: i32,
    /// `scale`, which `EV_PLAY_FX_DIR` spends on its `DirToByte` direction.
    pub scale: i32,
    pub origin: [f32; 3],
    pub scope: Scope,
    /// `EV_EARTHQUAKE`'s own three fields.
    pub quake: Option<Quake>,
}

/// What the `earthquake` builtin (0x5f3d8) writes past `G_TempEntity`:
/// `angles2[0]` the scale, `time` the duration in ms, `angles2[1]` the
/// radius.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Quake {
    pub scale: f32,
    pub duration_ms: i32,
    pub radius: f32,
}

/// The entity state one temp entity puts on the wire at `number`. The origin
/// is truncated toward zero, as `G_TempEntity` (0x67938) stores it
/// (`docs/research/cod11-turrets.md` 8).
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
    set("weapon", te.weapon);
    set("clientNum", te.client_num);
    set("_union.scale", te.scale);
    if let Some(q) = te.quake {
        set("angles2[0]", q.scale.to_bits() as i32);
        set("time", q.duration_ms);
        set("angles2[1]", q.radius.to_bits() as i32);
    }
    for (axis, v) in te.origin.iter().enumerate() {
        set(
            &format!("pos.trBase[{axis}]"),
            (v.trunc() + 0.0).to_bits() as i32,
        );
    }
    e
}

/// Whether a snapshot whose `ps.clientNum` is `client_num` may carry this
/// temp entity at all. A `Broadcast` one still skips the PVS cull; the other
/// three are culled like any other entity once this says yes
/// (`crate::server`).
pub fn visible_to(te: &TempEntity, client_num: usize) -> bool {
    te.scope.admits(client_num)
}

impl Scope {
    /// [`visible_to`] for the scope alone, which is what an archived frame
    /// keeps of a temp entity.
    pub fn admits(self, client_num: usize) -> bool {
        match self {
            Scope::Broadcast | Scope::Pvs => true,
            Scope::AllBut(s) => s != client_num,
            Scope::Only(s) => s == client_num,
        }
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
            weapon: 0,
            origin: [0.0; 3],
            client_num: 0,
            scale: 0,
            scope: Scope::Broadcast,
            quake: None,
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
            weapon: 0,
            origin: [1.0, 2.0, 3.0],
            client_num: 0,
            scale: 0,
            scope: Scope::Broadcast,
            quake: None,
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

    /// A bullet's impact point reaches the wire whole, each axis toward
    /// zero: the turret capture's rounds read (1648, 1492, -31).
    #[test]
    fn the_origin_is_truncated_toward_zero() {
        let te = TempEntity {
            origin: [1648.0751, 1492.962, -31.875],
            ..flesh_hit()
        };
        let p = &PROTOCOL_V1;
        assert_eq!(build(&te, 900, p).origin(p), [1648.0, 1492.0, -31.0]);
    }

    #[test]
    fn scope_all_but_hides_from_one_client() {
        let te = TempEntity {
            scope: Scope::AllBut(2),
            quake: None,
            ..flesh_hit()
        };
        assert!(visible_to(&te, 0));
        assert!(!visible_to(&te, 2));
    }

    #[test]
    fn scope_only_reaches_one_client() {
        let te = TempEntity {
            scope: Scope::Only(2),
            quake: None,
            ..flesh_hit()
        };
        assert!(!visible_to(&te, 0));
        assert!(visible_to(&te, 2));
        assert!(visible_to(&flesh_hit(), 0));
    }
}
