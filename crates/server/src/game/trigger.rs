//! The map's triggers as a host-side table, and the box test the touch pass
//! runs against them. Which classnames are triggers, and what each does, is
//! docs/superpowers/specs/2026-09-08-movers-triggers-sd-design.md section 3.

use crate::game::host::GameHost;
use std::collections::BTreeMap;
use vcod_gsc::{Atom, Cx, EntId, Host, Value};

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
    /// What a touch takes off the toucher, and the damage flags it takes it
    /// with. Both 0 on every kind but `Hurt`.
    pub damage: i32,
    pub dflags: i32,
}

// Keyed by `EntId::0` rather than `EntId` itself: `EntId` is a foreign type
// with no `Ord` impl, and the orphan rule refuses one added here. Entity
// numbers are handed out in spawn order, so this also keeps `iter()` in the
// order a map's triggers were loaded in.
#[derive(Default)]
pub struct Triggers {
    rows: BTreeMap<u32, Trigger>,
}

/// The `dmg` default, the two spawnflags and the two touch intervals, all
/// from docs/research/cod11-gsc-object-model.md 8.1.
pub const HURT_DEFAULT_DAMAGE: i32 = 5;
const HURT_NO_PROTECTION: i32 = 0x8;
const HURT_SLOW: i32 = 0x10;
const HURT_INTERVAL_MS: i32 = 100;
const HURT_SLOW_INTERVAL_MS: i32 = 1000;

/// The mod `hurt_touch` damages with (the same doc section).
pub const MOD_TRIGGER_HURT: &str = "MOD_TRIGGER_HURT";

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
                damage: 0,
                dflags: 0,
            },
        );
    }

    /// A `trigger_hurt`, whose damage, flags and cadence come from
    /// `SP_trigger_hurt` (0x64ef8) and `hurt_touch` (0x64dc4) rather than from
    /// the `wait`/`random` keys the other kinds take; the cadence rides
    /// `wait_ms` because retail's timestamp gates its notify too
    /// (docs/research/cod11-gsc-object-model.md 8.1).
    ///
    /// Not modelled, and the same section says why: `hurt_touch` tests a byte
    /// on the *toucher* before its timestamp, so ours hurts a dead player
    /// where retail may not.
    pub fn register_hurt(
        &mut self,
        id: EntId,
        mins: [f32; 3],
        maxs: [f32; 3],
        damage: i32,
        spawnflags: i32,
    ) {
        let wait_ms = if spawnflags & HURT_SLOW == 0 {
            HURT_INTERVAL_MS
        } else {
            HURT_SLOW_INTERVAL_MS
        };
        self.register(id, TriggerKind::Hurt, mins, maxs, wait_ms, 0);
        if let Some(t) = self.rows.get_mut(&id.0) {
            t.damage = damage;
            t.dflags = if spawnflags & HURT_NO_PROTECTION == 0 {
                0
            } else {
                crate::game::combat::DFLAG_NO_PROTECTION
            };
        }
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

    /// Whether a touch fires, arming the next window from `wait_ms` and
    /// `random_ms`. An ungated trigger (both 0) fires on every touch; a
    /// gated one refuses until the window elapses; a `trigger_once` arms a
    /// window that never elapses, so it fires exactly once.
    pub fn fire(&mut self, id: EntId, now_ms: i32, rng: &mut impl FnMut(i32) -> i32) -> bool {
        let Some(t) = self.rows.get_mut(&id.0) else {
            return false;
        };
        if now_ms < t.next_fire_ms {
            return false;
        }
        t.next_fire_ms = match t.kind {
            TriggerKind::Once => i32::MAX,
            _ if t.wait_ms == 0 && t.random_ms == 0 => now_ms,
            _ => now_ms + t.wait_ms + rng(t.random_ms),
        };
        true
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

/// The player's own clip box, which is what a touch's exact
/// `trap_EntityContact` test measures against and not what the broad phase
/// queries (docs/research/cod11-gsc-object-model.md section 22). Half-width 15
/// and 0..72 standing, from the movement constants table in
/// docs/research/cod11-mantle.md.
pub const PLAYER_MINS: [f32; 3] = [-15.0, -15.0, 0.0];
pub const PLAYER_MAXS: [f32; 3] = [15.0, 15.0, 72.0];

/// `origin` is taken as an already-interned atom rather than folded here:
/// the touch pass calls this once per client plus once per trigger row per
/// cmd, and folding lowercases and allocates every time.
fn entity_origin(host: &mut GameHost, cx: &mut Cx, id: EntId, origin: Atom) -> [f32; 3] {
    match host.get_field(cx, id, origin) {
        Value::Vector(v) => v,
        _ => [0.0; 3],
    }
}

/// An entity's absolute box: a registered trigger's submodel box around its
/// current origin, a client's player box, and a point box for everything
/// else, which is what an unset `r.mins`/`r.maxs` gives retail.
pub fn entity_abs_bounds(host: &mut GameHost, cx: &mut Cx, id: EntId) -> ([f32; 3], [f32; 3]) {
    let origin = cx.intern_folded("origin");
    abs_bounds_with_atom(host, cx, id, origin)
}

fn abs_bounds_with_atom(
    host: &mut GameHost,
    cx: &mut Cx,
    id: EntId,
    origin_atom: Atom,
) -> ([f32; 3], [f32; 3]) {
    let origin = entity_origin(host, cx, id, origin_atom);
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
    let origin_atom = cx.intern_folded("origin");
    let origin = entity_origin(host, cx, client, origin_atom);
    let candidate = offset_bounds(
        origin,
        [-TOUCH_BOX[0], -TOUCH_BOX[1], -TOUCH_BOX[2]],
        TOUCH_BOX,
    );
    let exact = abs_bounds_with_atom(host, cx, client, origin_atom);
    let ids: Vec<EntId> = host.triggers.iter().map(|(id, _)| id).collect();
    ids.into_iter()
        .filter(|id| {
            // A `trigger_lookat`'s contents bit is not in the mask retail's
            // broad phase queries with, so no touch ever returns one
            // (docs/research/cod11-gsc-object-model.md 22.1).
            if host.triggers.get(*id).map(|t| t.kind) == Some(TriggerKind::LookAt) {
                return false;
            }
            let b = abs_bounds_with_atom(host, cx, *id, origin_atom);
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
            damage: 0,
            dflags: 0,
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

    /// A `trigger_lookat` sharing a box with a `trigger_multiple` is not
    /// touched: retail's broad-phase contents mask has its bit clear
    /// (docs/research/cod11-gsc-object-model.md 22.1).
    #[test]
    fn a_lookat_trigger_is_not_touched_where_a_multiple_is() {
        let (mut vm, mut host) = crate::game::testing::fixture();
        vm.with_cx(|cx| {
            let player = host.ents.spawn_client(cx, 0, None).unwrap();
            let origin = cx.intern_folded("origin");
            let place = |host: &mut GameHost, cx: &mut Cx, kind| {
                let id = host.ents.spawn(cx).unwrap();
                host.set_field(cx, id, origin, Value::Vector([0.0, 0.0, 4.0]))
                    .unwrap();
                host.triggers.register(id, kind, [-8.0; 3], [8.0; 3], 0, 0);
                id
            };
            let multiple = place(&mut host, cx, TriggerKind::Multiple);
            place(&mut host, cx, TriggerKind::LookAt);
            assert_eq!(touched(&mut host, cx, player), vec![multiple]);
        });
    }

    /// `wait` gates a `trigger_multiple`: the first touch fires, touches
    /// inside the window do not, and the one after it does. `random` widens
    /// the window by up to its own value.
    #[test]
    fn wait_gates_a_multiple_and_random_widens_it() {
        let mut ts = Triggers::default();
        let id = EntId(72);
        ts.register(id, TriggerKind::Multiple, [-8.0; 3], [8.0; 3], 500, 0);
        let mut zero = |_: i32| 0;
        assert!(ts.fire(id, 1000, &mut zero), "first touch fires");
        assert!(!ts.fire(id, 1400, &mut zero), "inside the 500 ms window");
        assert!(ts.fire(id, 1500, &mut zero), "the window has passed");

        let mut half = |n: i32| n / 2;
        ts.register(id, TriggerKind::Multiple, [-8.0; 3], [8.0; 3], 500, 400);
        assert!(ts.fire(id, 0, &mut half));
        assert!(!ts.fire(id, 690, &mut half), "500 + 400/2 is 700");
        assert!(ts.fire(id, 700, &mut half));
    }

    /// A `trigger_once` fires once and then never again, however long the
    /// toucher stands in it.
    #[test]
    fn a_once_trigger_fires_once() {
        let mut ts = Triggers::default();
        let id = EntId(73);
        ts.register(id, TriggerKind::Once, [-8.0; 3], [8.0; 3], 0, 0);
        let mut zero = |_: i32| 0;
        assert!(ts.fire(id, 0, &mut zero));
        assert!(!ts.fire(id, 1, &mut zero));
        assert!(!ts.fire(id, 100_000, &mut zero));
    }

    /// With neither key set, every touch fires: that is what a bombzone does,
    /// and `bombzone_think` relies on being notified every pass while the
    /// player stands in it.
    #[test]
    fn no_wait_key_fires_every_touch() {
        let mut ts = Triggers::default();
        let id = EntId(74);
        ts.register(id, TriggerKind::Multiple, [-8.0; 3], [8.0; 3], 0, 0);
        let mut zero = |_: i32| 0;
        for t in [0, 50, 100, 150] {
            assert!(ts.fire(id, t, &mut zero), "touch at {t}");
        }
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
