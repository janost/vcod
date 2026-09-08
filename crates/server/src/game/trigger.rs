//! The map's triggers as a host-side table, and the box test the touch pass
//! runs against them. Which classnames are triggers, and what each does, is
//! docs/superpowers/specs/2026-09-08-movers-triggers-sd-design.md section 3.

use crate::game::host::GameHost;
use std::collections::BTreeMap;
use vcod_gsc::{Cx, EntId, Host, Value};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TriggerKind {
    Multiple,
    Once,
    Use,
    LookAt,
    Hurt,
    Damage,
}

/// One trigger. `mins`/`maxs` are the submodel's own box, so the absolute
/// one is taken around the entity's current origin rather than cached.
#[derive(Clone, Copy, Debug)]
pub struct Trigger {
    pub kind: TriggerKind,
    pub mins: [f32; 3],
    pub maxs: [f32; 3],
    /// The `wait` and `random` keys as milliseconds; both 0 means no gate.
    pub wait_ms: i32,
    pub random_ms: i32,
    /// Level-clock time this may fire again.
    pub next_fire_ms: i32,
}

// Keyed by `EntId::0` rather than `EntId` itself: `EntId` is a foreign type
// with no `Ord` impl, and the orphan rule refuses one added here. Entity
// numbers are handed out in spawn order, so this also keeps `iter()` in the
// order a map's triggers were loaded in.
#[derive(Default)]
pub struct Triggers {
    rows: BTreeMap<u32, Trigger>,
}

impl Triggers {
    pub fn register(
        &mut self,
        id: EntId,
        kind: TriggerKind,
        mins: [f32; 3],
        maxs: [f32; 3],
        wait_ms: i32,
        random_ms: i32,
    ) {
        self.rows.insert(
            id.0,
            Trigger {
                kind,
                mins,
                maxs,
                wait_ms,
                random_ms,
                next_fire_ms: 0,
            },
        );
    }

    pub fn remove(&mut self, id: EntId) {
        self.rows.remove(&id.0);
    }

    pub fn get(&self, id: EntId) -> Option<&Trigger> {
        self.rows.get(&id.0)
    }

    pub fn get_mut(&mut self, id: EntId) -> Option<&mut Trigger> {
        self.rows.get_mut(&id.0)
    }

    pub fn iter(&self) -> impl Iterator<Item = (EntId, &Trigger)> {
        self.rows.iter().map(|(id, t)| (EntId(*id), t))
    }

    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }
}

pub fn kind_of(classname: &str) -> Option<TriggerKind> {
    Some(match classname {
        "trigger_multiple" => TriggerKind::Multiple,
        "trigger_once" => TriggerKind::Once,
        "trigger_use" => TriggerKind::Use,
        "trigger_lookat" => TriggerKind::LookAt,
        "trigger_hurt" => TriggerKind::Hurt,
        "trigger_damage" => TriggerKind::Damage,
        _ => return None,
    })
}

/// A box offset to an origin: the shape `abs_bounds` and the point/player
/// cases share.
fn offset_bounds(origin: [f32; 3], mins: [f32; 3], maxs: [f32; 3]) -> ([f32; 3], [f32; 3]) {
    let lo = [
        origin[0] + mins[0],
        origin[1] + mins[1],
        origin[2] + mins[2],
    ];
    let hi = [
        origin[0] + maxs[0],
        origin[1] + maxs[1],
        origin[2] + maxs[2],
    ];
    (lo, hi)
}

pub fn abs_bounds(origin: [f32; 3], t: &Trigger) -> ([f32; 3], [f32; 3]) {
    offset_bounds(origin, t.mins, t.maxs)
}

/// The player's own clip box, which retail hands `trap_EntitiesInBox` around
/// the client's origin. Half-width 15 and 0..72 standing, from the movement
/// constants table in docs/research/cod11-mantle.md.
pub const PLAYER_MINS: [f32; 3] = [-15.0, -15.0, 0.0];
pub const PLAYER_MAXS: [f32; 3] = [15.0, 15.0, 72.0];

/// An entity's absolute box: a registered trigger's submodel box around its
/// current origin, a client's player box, and a point box for everything
/// else, which is what an unset `r.mins`/`r.maxs` gives retail.
fn entity_origin(host: &mut GameHost, cx: &mut Cx, id: EntId) -> [f32; 3] {
    let origin_atom = cx.intern_folded("origin");
    match host.get_field(cx, id, origin_atom) {
        Value::Vector(v) => v,
        _ => [0.0; 3],
    }
}

pub fn entity_abs_bounds(host: &mut GameHost, cx: &mut Cx, id: EntId) -> ([f32; 3], [f32; 3]) {
    let origin = entity_origin(host, cx, id);
    if let Some(t) = host.triggers.get(id) {
        return abs_bounds(origin, t);
    }
    let is_client = host.ents.get(id).is_some_and(|e| e.client.is_some());
    let (mins, maxs) = if is_client {
        (PLAYER_MINS, PLAYER_MAXS)
    } else {
        ([0.0; 3], [0.0; 3])
    };
    offset_bounds(origin, mins, maxs)
}

/// The candidate box retail hands `trap_EntitiesInBox`, taken around the
/// client's origin rather than around its clip box
/// (docs/research/cod11-gsc-object-model.md section 22).
const TOUCH_BOX: [f32; 3] = [40.0, 40.0, 52.0];

/// Every trigger this client touches, ascending entity number: the
/// `trap_EntitiesInBox` broad phase around the origin, then the exact
/// `trap_EntityContact` test against the client's own clip box. Neither box
/// contains the other -- the candidate reaches 52 below the feet and the clip
/// box 72 above them -- so both have to hold.
pub fn touched(host: &mut GameHost, cx: &mut Cx, client: EntId) -> Vec<EntId> {
    let origin = entity_origin(host, cx, client);
    let candidate = offset_bounds(
        origin,
        [-TOUCH_BOX[0], -TOUCH_BOX[1], -TOUCH_BOX[2]],
        TOUCH_BOX,
    );
    let exact = entity_abs_bounds(host, cx, client);
    let ids: Vec<EntId> = host.triggers.iter().map(|(id, _)| id).collect();
    ids.into_iter()
        .filter(|id| {
            let b = entity_abs_bounds(host, cx, *id);
            boxes_overlap(candidate, b) && boxes_overlap(exact, b)
        })
        .collect()
}

/// Do two absolute boxes overlap, the test `trap_EntitiesInBox` performs.
pub fn boxes_overlap(a: ([f32; 3], [f32; 3]), b: ([f32; 3], [f32; 3])) -> bool {
    (0..3).all(|i| a.0[i] <= b.1[i] && a.1[i] >= b.0[i])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The six trigger classnames in the `spawns` table map to kinds; nothing
    /// else does (docs/research/cod11-gsc-object-model.md section 8).
    #[test]
    fn the_spawns_tables_trigger_classnames_map_to_kinds() {
        assert_eq!(kind_of("trigger_multiple"), Some(TriggerKind::Multiple));
        assert_eq!(kind_of("trigger_once"), Some(TriggerKind::Once));
        assert_eq!(kind_of("trigger_use"), Some(TriggerKind::Use));
        assert_eq!(kind_of("trigger_lookat"), Some(TriggerKind::LookAt));
        assert_eq!(kind_of("trigger_hurt"), Some(TriggerKind::Hurt));
        assert_eq!(kind_of("trigger_damage"), Some(TriggerKind::Damage));
        assert_eq!(kind_of("script_brushmodel"), None);
        assert_eq!(kind_of("misc_mg42"), None);
    }

    /// Bounds are the submodel's, taken around the entity's *current* origin:
    /// `sd.gsc` relocates its defuse trigger with `bombtrigger.origin =
    /// level.bombmodel.origin`, so a cached absolute box would be wrong from
    /// the plant onward.
    #[test]
    fn abs_bounds_follow_the_origin() {
        let t = Trigger {
            kind: TriggerKind::Multiple,
            mins: [-16.0, -16.0, 0.0],
            maxs: [16.0, 16.0, 72.0],
            wait_ms: 0,
            random_ms: 0,
            next_fire_ms: 0,
        };
        assert_eq!(
            abs_bounds([100.0, -50.0, 8.0], &t),
            ([84.0, -66.0, 8.0], [116.0, -34.0, 80.0])
        );
    }

    /// A registered trigger is found by id and a removed one is gone: the
    /// plant deletes both bombzones and a stale row would keep firing.
    #[test]
    fn register_and_remove() {
        let mut ts = Triggers::default();
        let id = EntId(72);
        assert!(ts.is_empty());
        ts.register(id, TriggerKind::Hurt, [-8.0; 3], [8.0; 3], 0, 0);
        assert_eq!(ts.len(), 1);
        assert_eq!(ts.get(id).map(|t| t.kind), Some(TriggerKind::Hurt));
        ts.remove(id);
        assert!(ts.get(id).is_none());
        assert!(ts.is_empty());
    }

    /// A trigger has to clear both boxes: one 65 units up is inside the
    /// client's clip box and outside the candidate box, one 30 down is the
    /// other way round, and retail touches neither.
    #[test]
    fn a_touch_needs_both_the_candidate_box_and_the_clip_box() {
        let (mut vm, mut host) = crate::game::testing::fixture();
        vm.with_cx(|cx| {
            let player = host.ents.spawn_client(cx, 0, None).unwrap();
            let origin = cx.intern_folded("origin");
            let place = |host: &mut GameHost, cx: &mut Cx, z: f32| {
                let id = host.ents.spawn(cx).unwrap();
                host.set_field(cx, id, origin, Value::Vector([0.0, 0.0, z]))
                    .unwrap();
                host.triggers
                    .register(id, TriggerKind::Multiple, [-8.0; 3], [8.0; 3], 0, 0);
                id
            };
            let at_feet = place(&mut host, cx, 4.0);
            place(&mut host, cx, 65.0);
            place(&mut host, cx, -30.0);
            assert_eq!(touched(&mut host, cx, player), vec![at_feet]);
        });
    }

    /// `delete()` takes the row with the entity. `sd.gsc` deletes both
    /// bombzones the instant a plant completes.
    #[test]
    fn freeing_an_entity_drops_its_trigger_row() {
        let (mut vm, mut host) = crate::game::testing::fixture();
        vm.with_cx(|cx| {
            let id = host.ents.spawn(cx).unwrap();
            host.triggers
                .register(id, TriggerKind::Multiple, [-8.0; 3], [8.0; 3], 0, 0);
            host.free_entity(id);
            assert!(host.triggers.get(id).is_none());
            assert!(host.ents.get(id).is_none());
        });
    }
}
