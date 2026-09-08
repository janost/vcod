# Triggers and the Touch Pass Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give `vcod-server` a per-usercmd touch pass so a stock map's triggers fire their `"trigger"` notify, `trigger_hurt` hurts, `trigger_use` answers the use key and `trigger_lookat` sets a cursor hint.

**Architecture:** A host-side `Triggers` table keyed by `EntId`, registered from the entity lump at map load and dropped on entity free, shaped after `crates/server/src/game/missile.rs`. The pass runs inside `Server::replay_moves` after each pmove step, testing the client's grown abs box against each trigger's bounds and queueing a VM notify that parked `waittill("trigger", other)` threads wake on in the same tick's script frame. Damage goes through the existing `CodeCallback_PlayerDamage` entry point.

**Tech Stack:** Rust (workspace crates `vcod-server`, `vcod-common`, `vcod-gsc`), the retail 1.1d Linux dedicated server as oracle via `tools/run_probe.sh` and `--net-probe`.

**Spec:** `docs/superpowers/specs/2026-09-08-movers-triggers-sd-design.md` (this plan is stage 1 of three; sections 3 and 6.1 are the ones it implements)

## Global Constraints

- `cargo build`, `cargo test`, `cargo fmt`, `cargo clippy -D warnings` clean before every commit.
- `vcod-common` may not import wgpu, winit or kira; `vcod-gsc` may depend only on `anyhow` and `log`.
- Tests needing game data go through `vcod_common::testing::game_fs()` and return early when it is `None`; CI runs with no `COD_DIR`.
- Research facts live in `docs/research/*.md`, never pasted into comments; a comment is a pointer.
- Evidence labels are per claim: VERIFIED is read out of a binary, asset or capture; INFERRED covers control flow, instruction sequencing included.
- Conventional commit prefixes (`feat:`, `fix:`, `docs:`, `test:`, `perf:`, `style:`, `chore:`). Feature work on a branch; master is merge-only.
- Never attribute the work to an assistant in any commit message or comment.
- No absolute paths to anything outside the workspace in code, comments or docs.

---

### Task 1: Trigger table and registration

**Files:**
- Create: `crates/server/src/game/trigger.rs`
- Modify: `crates/server/src/game/mod.rs` (add `pub mod trigger;`)
- Modify: `crates/server/src/game/host.rs` (add the `triggers` field to `GameHost`, beside `missiles`)
- Modify: `crates/server/src/game/spawn.rs:63-106` (`spawn_entities_from_string`, register a trigger after the field loop)
- Test: inline `#[cfg(test)] mod tests` in `crates/server/src/game/trigger.rs`

**Interfaces:**
- Consumes: `GameHost` (`crates/server/src/game/host.rs:167`), `ObjectTable` (`crates/server/src/game/entity.rs:145`), `vcod_gsc::EntId`.
- Produces:
  - `pub enum TriggerKind { Multiple, Once, Use, LookAt, Hurt, Damage }`
  - `pub struct Trigger { pub kind: TriggerKind, pub mins: [f32; 3], pub maxs: [f32; 3], pub wait_ms: i32, pub random_ms: i32, pub next_fire_ms: i32 }`
  - `pub struct Triggers` with `pub fn register(&mut self, id: EntId, kind: TriggerKind, mins: [f32; 3], maxs: [f32; 3], wait_ms: i32, random_ms: i32)`, `pub fn remove(&mut self, id: EntId)`, `pub fn get(&self, id: EntId) -> Option<&Trigger>`, `pub fn iter(&self) -> impl Iterator<Item = (EntId, &Trigger)>`, `pub fn len(&self) -> usize`, `pub fn is_empty(&self) -> bool`
  - `pub fn kind_of(classname: &str) -> Option<TriggerKind>`
  - `pub fn abs_bounds(origin: [f32; 3], t: &Trigger) -> ([f32; 3], [f32; 3])`

- [ ] **Step 1: Write the failing test**

Create `crates/server/src/game/trigger.rs` containing only the test module for now:

```rust
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
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p vcod-server --lib game::trigger`
Expected: FAIL — the module is not declared in `game/mod.rs`, and `Trigger`, `Triggers`, `TriggerKind`, `kind_of` and `abs_bounds` do not exist.

- [ ] **Step 3: Write the implementation**

Add `pub mod trigger;` to `crates/server/src/game/mod.rs` beside the other `pub mod` lines, then put this above the test module in `crates/server/src/game/trigger.rs`:

```rust
//! The map's triggers as a host-side table, and the box test the touch pass
//! runs against them. Which classnames are triggers, and what each does, is
//! docs/superpowers/specs/2026-09-08-movers-triggers-sd-design.md section 3.

use std::collections::BTreeMap;
use vcod_gsc::EntId;

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

#[derive(Default)]
pub struct Triggers {
    rows: BTreeMap<EntId, Trigger>,
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
            id,
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
        self.rows.remove(&id);
    }

    pub fn get(&self, id: EntId) -> Option<&Trigger> {
        self.rows.get(&id)
    }

    pub fn get_mut(&mut self, id: EntId) -> Option<&mut Trigger> {
        self.rows.get_mut(&id)
    }

    pub fn iter(&self) -> impl Iterator<Item = (EntId, &Trigger)> {
        self.rows.iter().map(|(id, t)| (*id, t))
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

pub fn abs_bounds(origin: [f32; 3], t: &Trigger) -> ([f32; 3], [f32; 3]) {
    let lo = [
        origin[0] + t.mins[0],
        origin[1] + t.mins[1],
        origin[2] + t.mins[2],
    ];
    let hi = [
        origin[0] + t.maxs[0],
        origin[1] + t.maxs[1],
        origin[2] + t.maxs[2],
    ];
    (lo, hi)
}
```

Add the field to `GameHost` in `crates/server/src/game/host.rs`, beside the other host-side tables, with the initialiser in `GameHost::new`:

```rust
    /// The map's triggers. Host-side beside the object table for the reason
    /// `missiles` is: retail's own state lives on the `gentity_t`, ours in a
    /// table the object model does not have to carry.
    pub triggers: crate::game::trigger::Triggers,
```

```rust
            triggers: crate::game::trigger::Triggers::default(),
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p vcod-server --lib game::trigger`
Expected: PASS, 3 tests.

- [ ] **Step 5: Register triggers at map load — write the failing test**

Add to `crates/server/src/game/spawn.rs`'s test module:

```rust
    /// Every trigger classname in the lump gets a row, with the submodel's
    /// bounds and its `wait` key in milliseconds. A non-trigger block gets
    /// none.
    #[test]
    fn the_entity_lump_registers_its_triggers() {
        let (mut vm, mut host) = crate::game::testing_world_fixture();
        vm.with_cx(|cx| {
            super::spawn_entities_from_string(
                &mut host,
                cx,
                "{\n\"classname\" \"worldspawn\"\n}\n\
                 {\n\"classname\" \"trigger_multiple\"\n\"model\" \"*1\"\n\"wait\" \"0.5\"\n}\n\
                 {\n\"classname\" \"script_model\"\n\"model\" \"xmodel/barrels\"\n}\n\
                 {\n\"classname\" \"trigger_hurt\"\n\"model\" \"*2\"\n}\n",
            )
            .unwrap();
        });
        assert_eq!(host.triggers.len(), 2);
        let kinds: Vec<_> = host.triggers.iter().map(|(_, t)| t.kind).collect();
        assert_eq!(
            kinds,
            vec![
                crate::game::trigger::TriggerKind::Multiple,
                crate::game::trigger::TriggerKind::Hurt
            ]
        );
        let (_, first) = host.triggers.iter().next().unwrap();
        assert_eq!(first.wait_ms, 500, "the wait key is seconds on the wire");
        assert_eq!(first.mins, [-16.0, -16.0, 0.0], "submodel 1's box");
    }
```

This needs a fixture with a `World` carrying two submodels. Add it to `crates/server/src/game/mod.rs` beside `fixture`:

```rust
    /// `fixture` with a two-submodel world attached, for the paths that read
    /// brush bounds. Model 1 is a 32x32x72 box, model 2 a 64-unit cube; both
    /// are the shapes the trigger tests assert against.
    pub fn testing_world_fixture() -> (Vm, GameHost) {
        let (vm, mut host) = fixture();
        host.model_bounds = vec![
            ([0.0; 3], [0.0; 3]),
            ([-16.0, -16.0, 0.0], [16.0, 16.0, 72.0]),
            ([-32.0; 3], [32.0; 3]),
        ];
        (vm, host)
    }
```

- [ ] **Step 6: Run it to verify it fails**

Run: `cargo test -p vcod-server --lib game::spawn::tests::the_entity_lump_registers_its_triggers`
Expected: FAIL — `testing_world_fixture` and `GameHost::model_bounds` do not exist and no registration happens.

- [ ] **Step 7: Implement bounds plumbing and registration**

Add to `GameHost` in `crates/server/src/game/host.rs`:

```rust
    /// Per lump-27 model, the submodel's own box, for the brush entities that
    /// read one. Filled at map load from the BSP; empty in a test that
    /// mounts no map.
    pub model_bounds: Vec<([f32; 3], [f32; 3])>,
```

with `model_bounds: Vec::new()` in `GameHost::new`. Fill it where the server hands the host its world (search `host.world = ` in `crates/server/src/server.rs` and set both together) from `bsp.models`:

```rust
        host.model_bounds = bsp.models.iter().map(|m| (m.mins, m.maxs)).collect();
```

In `spawn_entities_from_string`, after the `parse_field` loop and beside the existing `trigger_hurt` sound-alias arm:

```rust
        if let Some(kind) = crate::game::trigger::kind_of(&classname) {
            register_trigger(host, &block, id, kind);
        }
```

and the helper, next to `trigger_hurt_sound`:

```rust
/// A trigger's box is its submodel's (`"model" "*N"`), and its `wait` and
/// `random` keys are seconds on the wire. A trigger with no brush model, or
/// one naming a model the BSP has no bounds for, is registered with a zero
/// box: it then touches nothing, which is what retail's unset `r.mins`/
/// `r.maxs` do.
fn register_trigger(
    host: &mut GameHost,
    block: &std::collections::HashMap<String, String>,
    id: EntId,
    kind: crate::game::trigger::TriggerKind,
) {
    let bounds = block
        .get("model")
        .and_then(|m| m.strip_prefix('*'))
        .and_then(|n| n.parse::<usize>().ok())
        .and_then(|n| host.model_bounds.get(n).copied())
        .unwrap_or(([0.0; 3], [0.0; 3]));
    let secs_ms = |key: &str| -> i32 {
        block
            .get(key)
            .and_then(|v| v.trim().parse::<f32>().ok())
            .map_or(0, |s| (s * 1000.0) as i32)
    };
    host.triggers.register(
        id,
        kind,
        bounds.0,
        bounds.1,
        secs_ms("wait"),
        secs_ms("random"),
    );
}
```

- [ ] **Step 8: Run the test to verify it passes**

Run: `cargo test -p vcod-server --lib game::spawn`
Expected: PASS, including the existing spawn tests.

- [ ] **Step 9: Drop a trigger's row when its entity is freed — write the failing test**

Add to `crates/server/src/game/trigger.rs`'s test module:

```rust
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
```

- [ ] **Step 10: Run it to verify it fails**

Run: `cargo test -p vcod-server --lib game::trigger::tests::freeing_an_entity_drops_its_trigger_row`
Expected: FAIL — `GameHost::free_entity` does not exist.

- [ ] **Step 11: Implement `free_entity` and route every delete through it**

In `crates/server/src/game/host.rs`, on `impl GameHost`:

```rust
    /// Free an entity and everything the host hangs off it. Retail's
    /// `G_FreeEntity` unlinks the entity, which is what takes a submodel's
    /// brushes out of the clip and a trigger out of the touch pass, so the
    /// three have one entry point here rather than three call sites each.
    pub fn free_entity(&mut self, id: EntId) {
        self.triggers.remove(id);
        self.ents.free(id);
    }
```

Then find every caller of `self.ents.free(` / `host.ents.free(` outside `ObjectTable` itself (`rg -n "ents\.free\(" crates/server/src`) and route each through `free_entity`, except `spawn_entities_from_string`'s `SPAWN_FREES` arm, which frees a classname that is never a trigger and may stay as it is.

- [ ] **Step 12: Run the tests to verify they pass**

Run: `cargo test -p vcod-server`
Expected: PASS.

- [ ] **Step 13: Commit**

```bash
git add crates/server/src/game/trigger.rs crates/server/src/game/mod.rs \
        crates/server/src/game/host.rs crates/server/src/game/spawn.rs \
        crates/server/src/server.rs
git commit -m "feat(server): the map's triggers as a host-side table"
```

---

### Task 2: `istouching` on real bounds

**Files:**
- Modify: `crates/server/src/game/builtins/entity.rs:489-509` (`is_touching`)
- Test: same file's `#[cfg(test)] mod tests`

**Interfaces:**
- Consumes: `Triggers`, `abs_bounds`, `GameHost::model_bounds` (Task 1).
- Produces: `pub fn entity_abs_bounds(host: &GameHost, cx: &mut Cx, id: EntId) -> ([f32; 3], [f32; 3])` in `crates/server/src/game/trigger.rs` — a brush entity's box around its current origin, a point box for anything else.

- [ ] **Step 1: Write the failing test**

Add to `crates/server/src/game/builtins/entity.rs`'s test module:

```rust
    /// `istouching` is a box overlap, not a distance: a player standing in a
    /// bombzone 200 units wide is touching it, and one 4 units outside is
    /// not. The 32-unit origin comparison this replaces answered both wrong.
    #[test]
    fn is_touching_overlaps_boxes() {
        let (mut vm, mut host) = crate::game::testing_world_fixture();
        vm.with_cx(|cx| {
            let zone = host.ents.spawn(cx).unwrap();
            let origin = cx.intern_folded("origin");
            host.set_field(cx, zone, origin, Value::Vector([0.0, 0.0, 0.0]))
                .unwrap();
            host.triggers.register(
                zone,
                crate::game::trigger::TriggerKind::Multiple,
                [-100.0, -100.0, 0.0],
                [100.0, 100.0, 64.0],
                0,
                0,
            );

            let player = host.ents.spawn_client(cx, 0, None).unwrap();
            let inside = Some(Target::Entity(player));
            host.set_field(cx, player, origin, Value::Vector([90.0, 0.0, 0.0]))
                .unwrap();
            assert_eq!(
                is_touching(&mut host, cx, inside, &[Value::Entity(zone)]),
                Ok(Value::Int(1)),
                "90 is inside a box that reaches 100"
            );

            host.set_field(cx, player, origin, Value::Vector([104.0, 0.0, 0.0]))
                .unwrap();
            assert_eq!(
                is_touching(&mut host, cx, inside, &[Value::Entity(zone)]),
                Ok(Value::Int(0)),
                "104 is outside a box that reaches 100 plus the player's 15"
            );
        });
    }
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p vcod-server --lib builtins::entity::tests::is_touching_overlaps_boxes`
Expected: FAIL — the current `is_touching` compares origins with a 32-unit box, so the 104 case returns 0 only by accident and the 90 case returns 0 wrongly.

- [ ] **Step 3: Implement**

In `crates/server/src/game/trigger.rs`:

```rust
/// The player's own clip box, which retail hands `trap_EntitiesInBox` around
/// the client's origin. Half-width 15 and 0..72 standing, from the movement
/// constants table in docs/research/cod11-mantle.md.
pub const PLAYER_MINS: [f32; 3] = [-15.0, -15.0, 0.0];
pub const PLAYER_MAXS: [f32; 3] = [15.0, 15.0, 72.0];

/// An entity's absolute box: a registered trigger's submodel box around its
/// current origin, a client's player box, and a point box for everything
/// else, which is what an unset `r.mins`/`r.maxs` gives retail.
pub fn entity_abs_bounds(
    host: &GameHost,
    cx: &mut Cx,
    id: EntId,
) -> ([f32; 3], [f32; 3]) {
    let origin = match host.get_field(cx, id, cx.intern_folded("origin")) {
        Value::Vector(v) => v,
        _ => [0.0; 3],
    };
    if let Some(t) = host.triggers.get(id) {
        return abs_bounds(origin, t);
    }
    let is_client = host.ents.get(id).is_some_and(|e| e.client.is_some());
    let (mins, maxs) = if is_client {
        (PLAYER_MINS, PLAYER_MAXS)
    } else {
        ([0.0; 3], [0.0; 3])
    };
    abs_bounds(
        origin,
        &Trigger {
            kind: TriggerKind::Multiple,
            mins,
            maxs,
            wait_ms: 0,
            random_ms: 0,
            next_fire_ms: 0,
        },
    )
}

/// Do two absolute boxes overlap, the test `trap_EntitiesInBox` performs.
pub fn boxes_overlap(a: ([f32; 3], [f32; 3]), b: ([f32; 3], [f32; 3])) -> bool {
    (0..3).all(|i| a.0[i] <= b.1[i] && a.1[i] >= b.0[i])
}
```

`GameHost::get_field` takes `&mut self`; if borrow-checking bites, take `host: &mut GameHost` in `entity_abs_bounds` and adjust the callers, which all have a `&mut GameHost` in hand.

Then replace the body of `is_touching` (`builtins/entity.rs:489`):

```rust
    let a = entity_receiver(recv)?;
    let Some(Value::Entity(b)) = args.first() else {
        return Err(ErrorKind::BadType("isTouching takes an entity"));
    };
    let b = *b;
    let ba = crate::game::trigger::entity_abs_bounds(host, cx, a);
    let bb = crate::game::trigger::entity_abs_bounds(host, cx, b);
    Ok(Value::Int(crate::game::trigger::boxes_overlap(ba, bb) as i32))
```

Update the doc comment above it: it currently describes the 32-unit approximation.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p vcod-server`
Expected: PASS. If an existing test asserted the 32-unit behaviour, it is now wrong and should be rewritten to the box semantics rather than kept.

- [ ] **Step 5: Commit**

```bash
git add crates/server/src/game/trigger.rs crates/server/src/game/builtins/entity.rs
git commit -m "feat(server): istouching is a box overlap, not an origin distance"
```

---

### Task 3: The touch pass and the `"trigger"` notify

**Files:**
- Modify: `crates/server/src/game/trigger.rs` (the pass itself)
- Modify: `crates/server/src/game/script.rs` (a `ScriptRuntime` entry point, beside `deliver_hits` at :298)
- Modify: `crates/server/src/server.rs:2789-2940` (`replay_moves`, call it per cmd)
- Test: `crates/server/src/game/script.rs` test module

**Interfaces:**
- Consumes: Task 1's table, Task 2's `boxes_overlap`/`entity_abs_bounds`.
- Produces:
  - `pub fn touched(host: &mut GameHost, cx: &mut Cx, client: EntId) -> Vec<EntId>` in `trigger.rs` — every trigger whose box overlaps the client's grown box, ascending entity number.
  - `pub fn touch_triggers(&mut self, slot: usize, now_ms: i32)` on `ScriptRuntime`.

- [ ] **Step 1: Write the failing test**

Add to `crates/server/src/game/script.rs`'s test module:

```rust
    /// A client standing in a trigger wakes the thread parked on it, with the
    /// toucher as the notify's argument. Retail raises this inside
    /// `G_TouchTriggers` (0x3f88c) with `Scr_AddEntity` supplying `other`.
    #[test]
    fn touching_a_trigger_notifies_the_parked_thread() {
        let mut rt = ScriptRuntime::for_test(
            "main() { level.hits = 0; level thread watch(); }\n\
             watch() { for(;;) { level waittill(\"go\"); } }\n",
        );
        // The trigger's own thread, started on the entity the test registers.
        let src = "trigger_think() { for(;;) { self waittill(\"trigger\", other); \
                   level.hits = level.hits + 1; level.who = other; } }";
        rt.install_for_test(src);

        let zone = rt.spawn_map_entity_for_test([0.0, 0.0, 0.0]);
        rt.host.triggers.register(
            zone,
            crate::game::trigger::TriggerKind::Multiple,
            [-64.0, -64.0, 0.0],
            [64.0, 64.0, 64.0],
            0,
            0,
        );
        rt.start_thread_for_test(zone, "trigger_think", 0);
        rt.run_frame(0);

        let player = rt.spawn_client_for_test(0, [10.0, 0.0, 0.0]);
        assert_eq!(rt.client_entity(0), Some(player));

        rt.touch_triggers(0, 50);
        rt.run_frame(50);
        assert_eq!(rt.level_field("hits"), Value::Int(1), "one notify");
        assert_eq!(rt.level_field("who"), Value::Entity(player));

        // Out of the box: no second notify.
        rt.set_client_origin(0, [500.0, 0.0, 0.0]);
        rt.touch_triggers(0, 100);
        rt.run_frame(100);
        assert_eq!(rt.level_field("hits"), Value::Int(1));
    }
```

The three `*_for_test` helpers do not exist yet; write them in the same test-support block as `for_test_at` (`crates/server/src/game/script.rs:1020`), each a thin wrapper over what the runtime already does:

```rust
    /// Compile and install another file's worth of functions into a test
    /// runtime, so a test can add a thread body after `for_test`.
    pub fn install_for_test(&mut self, src: &str) {
        let ast = vcod_gsc::parse::parse_file(src).expect("test script parses");
        let fns = vcod_gsc::compile::compile_file(&ast, &self.entry, self.vm.interner_mut())
            .expect("test script compiles");
        self.vm.install(fns).expect("test script installs");
    }

    /// A map entity at `origin`, the way the entity lump makes one.
    pub fn spawn_map_entity_for_test(&mut self, origin: [f32; 3]) -> EntId {
        let host = &mut self.host;
        self.vm.with_cx(|cx| {
            let id = host.ents.spawn(cx).unwrap();
            let at = cx.intern_folded("origin");
            host.set_field(cx, id, at, Value::Vector(origin)).unwrap();
            id
        })
    }

    /// A client entity in `slot` at `origin`.
    pub fn spawn_client_for_test(&mut self, slot: usize, origin: [f32; 3]) -> EntId {
        let host = &mut self.host;
        let id = self
            .vm
            .with_cx(|cx| host.ents.spawn_client(cx, slot, None).unwrap());
        self.set_client_origin(slot, origin);
        id
    }

    /// Start `name` as a thread on `ent`, the way a script's `thread` does.
    pub fn start_thread_for_test(&mut self, ent: EntId, name: &str, now_ms: i32) {
        let f = self.vm.func_ref(&self.entry, name);
        self.vm
            .start_thread(&mut self.host, now_ms, f, Some(Target::Entity(ent)), vec![]);
    }
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p vcod-server --lib game::script::tests::touching_a_trigger_notifies_the_parked_thread`
Expected: FAIL — `touch_triggers` and the test helpers do not exist.

- [ ] **Step 3: Implement the pass**

In `crates/server/src/game/trigger.rs`:

```rust
/// The candidate box retail hands `trap_EntitiesInBox`: the client's
/// `ps.origin` plus and minus these three, not the player's own clip box.
/// VERIFIED as values, read from `.data` 0x7dcdc, 0x7dce0 and 0x7dce4 (file
/// offsets 0x7ccdc..0x7cce4; `.data` is mapped at VA 0x7b3a0 from file
/// 0x7a3a0, so the raw dword at the VA is the wrong bytes). INFERRED as
/// role, from `G_TouchTriggers`'s two box constructions: the first, from
/// these constants, is the broad-phase query; the second, from the entity's
/// own `r.mins`/`r.maxs` at +0x100..+0x114, is the exact test.
const TOUCH_BOX: [f32; 3] = [40.0, 40.0, 52.0];

/// Every trigger the client's candidate box overlaps, ascending entity
/// number — retail's `trap_EntitiesInBox` broad phase followed by the exact
/// `trap_EntityContact` test against the client's own box.
pub fn touched(host: &mut GameHost, cx: &mut Cx, client: EntId) -> Vec<EntId> {
    let (lo, hi) = entity_abs_bounds(host, cx, client);
    let grown = (
        [
            lo[0] - TOUCH_BOX[0],
            lo[1] - TOUCH_BOX[1],
            lo[2] - TOUCH_BOX[2],
        ],
        [
            hi[0] + TOUCH_BOX[0],
            hi[1] + TOUCH_BOX[1],
            hi[2] + TOUCH_BOX[2],
        ],
    );
    let ids: Vec<EntId> = host.triggers.iter().map(|(id, _)| id).collect();
    ids.into_iter()
        .filter(|id| {
            let b = entity_abs_bounds(host, cx, *id);
            boxes_overlap(grown, b)
        })
        .collect()
}
```

`Triggers` is a `BTreeMap`, so `iter` is already ascending entity number, which is the order `getEntArray` and the entity walk use (object-model section 10).

`TOUCH_BOX`'s three values are already read out of the module (40, 40, 52).
Re-derive them if you want the check; note the `.data` mapping, since the raw
dword at the virtual address is the wrong bytes:

```bash
python3 tools/re/annotate_func.py private/server/main/game.mp.i386.so 0x3f88c | head -40
python3 - <<'PY'
import struct
d = open('private/server/main/game.mp.i386.so', 'rb').read()
for va in (0x7dcdc, 0x7dce0, 0x7dce4):
    print(hex(va), struct.unpack_from('<f', d, va - 0x7b3a0 + 0x7a3a0)[0])
PY
```

What the two boxes are for is INFERRED from control flow, so if the broad
phase turns out to matter (a trigger whose box is more than 40 units outside
the client's own would be missed), the A/B gate in Task 9 is what catches it.

In `crates/server/src/game/script.rs`, beside `deliver_hits`:

```rust
    /// Retail's `G_TouchTriggers` for one client, called once per usercmd
    /// from `ClientThink_real` (0x405b3). The notify wakes threads parked in
    /// `waittill("trigger", other)` when the scheduler next runs, which is
    /// this tick's script frame.
    pub fn touch_triggers(&mut self, slot: usize, now_ms: i32) {
        let Some(client) = self.client_entity(slot) else {
            return;
        };
        let host = &mut self.host;
        let hits = self
            .vm
            .with_cx(|cx| crate::game::trigger::touched(host, cx, client));
        for id in hits {
            let event = self.vm.with_cx(|cx| cx.intern_folded("trigger"));
            self.vm.notify(
                Target::Entity(id),
                event,
                vec![Value::Entity(client)],
            );
        }
        let _ = now_ms;
    }
```

Drop the `let _ = now_ms;` in Task 4, which is where the parameter starts being read.

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p vcod-server --lib game::script`
Expected: PASS.

- [ ] **Step 5: Call it from the move pass**

In `crates/server/src/server.rs`, inside `replay_moves`'s per-cmd loop, after the `while base != cmd.server_time` chop loop finishes and the events are drained for that cmd — i.e. immediately before `last_cmd = Some(cmd);` — record that this cmd wants a touch pass, and run the passes after the client loop, since the script runtime and the client borrow cannot be held at once:

```rust
                touched_slots.push(slot);
```

with `let mut touched_slots: Vec<usize> = Vec::new();` declared beside `let mut moved = ...`, and after the per-client loop, before the entity-state mirror block:

```rust
        // Retail runs the touch pass per usercmd inside `ClientThink_real`
        // (0x405b3), between the link and the use key; ours runs one pass per
        // cmd here, where the script runtime is borrowable.
        if let Some(rt) = self.script.as_mut() {
            for slot in touched_slots {
                rt.touch_triggers(slot, now_ms);
            }
        }
```

- [ ] **Step 6: Verify the whole suite still passes**

Run: `cargo test -p vcod-server && cargo clippy -p vcod-server --all-targets -- -D warnings`
Expected: PASS, no warnings.

- [ ] **Step 7: Commit**

```bash
git add crates/server/src/game/trigger.rs crates/server/src/game/script.rs crates/server/src/server.rs
git commit -m "feat(server): the touch pass raises the trigger notify"
```

---

### Task 4: `wait`/`random` gating and `trigger_once`

**Files:**
- Modify: `crates/server/src/game/trigger.rs` (the gate)
- Modify: `crates/server/src/game/script.rs` (`touch_triggers` applies it)
- Test: `crates/server/src/game/trigger.rs` test module

**Interfaces:**
- Consumes: Task 3's `touched`.
- Produces: `pub fn fire(&mut self, id: EntId, now_ms: i32, rng: &mut impl FnMut(i32) -> i32) -> bool` on `Triggers` — whether this touch fires, arming the next window.

- [ ] **Step 1: Write the failing test**

```rust
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
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p vcod-server --lib game::trigger`
Expected: FAIL — `Triggers::fire` does not exist.

- [ ] **Step 3: Implement**

```rust
impl Triggers {
    /// Whether a touch fires, arming the next window. Retail's `multi_wait`
    /// arms `nextthink` from the `wait` and `random` keys; a `trigger_once`
    /// is the same gate with an infinite window.
    pub fn fire(&mut self, id: EntId, now_ms: i32, rng: &mut impl FnMut(i32) -> i32) -> bool {
        let Some(t) = self.rows.get_mut(&id) else {
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
```

A `next_fire_ms` equal to `now_ms` still fires, which is what makes the ungated case fire every pass.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p vcod-server --lib game::trigger`
Expected: PASS, 6 tests.

- [ ] **Step 5: Apply the gate in `touch_triggers`**

Replace the notify loop body in `ScriptRuntime::touch_triggers` (Task 3) with:

```rust
        let rng = &mut self.rng;
        for id in hits {
            if !self.host.triggers.fire(id, now_ms, &mut |n| {
                if n <= 0 {
                    0
                } else {
                    ((vcod_common::rng::xorshift(rng) >> 33) as i32 & 0x7fff_ffff) % n
                }
            }) {
                continue;
            }
            let event = self.vm.with_cx(|cx| cx.intern_folded("trigger"));
            self.vm
                .notify(Target::Entity(id), event, vec![Value::Entity(client)]);
        }
```

`ScriptRuntime` has no rng today; give it `rng: u64` seeded at construction, the same `u64` state `Server` carries (`crates/server/src/server.rs:310`, drawn through `vcod_common::rng::xorshift`). Threading `Server`'s own rng through the call instead would put a `&mut` parameter on `touch_triggers`, which every test in Tasks 5 through 7 calls with two arguments — keep the signature at `(slot, now_ms)`.

- [ ] **Step 6: Run the suite**

Run: `cargo test -p vcod-server`
Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add crates/server/src/game/trigger.rs crates/server/src/game/script.rs crates/server/src/server.rs
git commit -m "feat(server): wait, random and once gate a trigger's notify"
```

---

### Task 5: `trigger_hurt` damage

**Files:**
- Modify: `crates/server/src/game/script.rs` (a world-damage entry point beside `deliver_hits`)
- Modify: `crates/server/src/game/trigger.rs` (the hurt keys)
- Test: `crates/server/tests/triggers.rs` (create)

**Interfaces:**
- Consumes: Task 3's pass, `CodeCallback_PlayerDamage` (`script.rs:298-334` shows the call shape).
- Produces: `pub fn deliver_world_hit(&mut self, victim_slot: usize, inflictor: EntId, damage: i32, mod_: &str, now_ms: i32)` on `ScriptRuntime`; `Trigger::damage: i32` and `Trigger::dflags: i32` fields.

- [ ] **Step 1: Write the failing test**

Create `crates/server/tests/triggers.rs`:

```rust
//! The touch pass against a script that answers it, end to end through
//! `ScriptRuntime`: what a stock map's triggers do to a player standing in
//! one.

use vcod_server::game::script::ScriptRuntime;
use vcod_server::game::trigger::TriggerKind;

/// A `trigger_hurt` damages the player standing in it through the stock
/// damage callback, so the victim's health drops and the kill path is the
/// same one a bullet takes.
#[test]
fn a_trigger_hurt_damages_the_player_in_it() {
    let mut rt = ScriptRuntime::for_test_at(
        "maps/mp/gametypes/_callbacksetup",
        "main() {}\n\
         CodeCallback_PlayerDamage(inflictor, attacker, damage, flags, mod, weapon, point, dir, hitloc) \
         { level.damage = damage; level.mod = mod; self.health = self.health - damage; }\n",
    );
    let player = rt.spawn_client_for_test(0, [0.0, 0.0, 0.0]);
    rt.set_client_health_for_test(0, 100);

    let hurt = rt.spawn_map_entity_for_test([0.0, 0.0, 0.0]);
    rt.host.triggers.register_hurt(
        hurt,
        [-64.0, -64.0, 0.0],
        [64.0, 64.0, 64.0],
        5,
    );

    rt.touch_triggers(0, 100);
    rt.run_frame(100);

    assert_eq!(rt.level_field("damage"), vcod_gsc::Value::Int(5));
    assert_eq!(rt.client_vitals(0).health, 95);
    let _ = (player, TriggerKind::Hurt);
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p vcod-server --test triggers`
Expected: FAIL — `register_hurt`, `set_client_health_for_test` and the damage path do not exist.

- [ ] **Step 3: Implement**

Add the two fields to `Trigger` (`damage: i32`, `dflags: i32`), defaulting to 0 in `register`, and a convenience constructor:

```rust
    /// `SP_trigger_hurt` (0x64ef8) reads a `dmg` key, defaulting to 5, and
    /// damages with `MOD_TRIGGER_HURT`. The default is INFERRED from the
    /// spawn function's control flow; the key name is VERIFIED from the
    /// shipped BSPs' `trigger_hurt` blocks.
    pub fn register_hurt(&mut self, id: EntId, mins: [f32; 3], maxs: [f32; 3], damage: i32) {
        self.register(id, TriggerKind::Hurt, mins, maxs, 0, 0);
        if let Some(t) = self.rows.get_mut(&id) {
            t.damage = damage;
        }
    }
```

Read the `dmg` key in `spawn.rs`'s `register_trigger` for the `Hurt` kind, defaulting to 5, and confirm both the key name and the default against `SP_trigger_hurt` before committing the constant:

```bash
python3 tools/re/annotate_func.py private/server/main/game.mp.i386.so 0x64ef8 | head -60
```

In `script.rs`, beside `deliver_hits`:

```rust
    /// Damage whose attacker is not a player: a `trigger_hurt`'s own entity
    /// is both inflictor and attacker, the way retail hands `G_Damage` the
    /// trigger for both. The stock callback tests `isPlayer(attacker)`, so a
    /// non-player attacker is a shape the corpus already handles.
    pub fn deliver_world_hit(
        &mut self,
        victim_slot: usize,
        inflictor: EntId,
        damage: i32,
        mod_: &str,
        now_ms: i32,
    ) {
        let Some(victim) = self.client_entity(victim_slot) else {
            return;
        };
        let (mod_, weapon, hitloc) = self.vm.with_cx(|cx| {
            (
                cx.intern_exact(mod_),
                cx.intern_exact("none"),
                cx.intern_exact("none"),
            )
        });
        let args = vec![
            Value::Entity(inflictor),
            Value::Entity(inflictor),
            Value::Int(damage),
            Value::Int(0),
            Value::String(mod_),
            Value::String(weapon),
            Value::Vector([0.0; 3]),
            Value::Vector([0.0; 3]),
            Value::String(hitloc),
        ];
        if let Err(e) = self.start_with_args(
            CALLBACK_SETUP,
            "CodeCallback_PlayerDamage",
            Some(Target::Entity(victim)),
            args,
            now_ms,
        ) {
            log::error!("gsc: {e:#}");
        }
    }
```

In `touch_triggers`, after the gate passes, branch on the kind: a `Hurt` calls `deliver_world_hit(slot, id, t.damage, "MOD_TRIGGER_HURT", now_ms)` **as well as** raising the notify — `_minefields.gsc` waits on the notify of a `trigger_multiple`, and a `trigger_hurt` raises its notify too.

Add the test helper next to the others in `script.rs`:

```rust
    /// Set a test client's health and max health, the way a spawn does.
    pub fn set_client_health_for_test(&mut self, slot: usize, health: i32) {
        if let Some(v) = self.host.client_vitals.get_mut(slot) {
            v.health = health;
            v.max_health = health;
        }
    }
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p vcod-server --test triggers`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/server/src/game/trigger.rs crates/server/src/game/script.rs crates/server/tests/triggers.rs
git commit -m "feat(server): trigger_hurt damages through the stock damage callback"
```

---

### Task 6: `trigger_use` and the use key

**Files:**
- Modify: `crates/server/src/game/script.rs` (`touch_triggers` reads the use bit)
- Modify: `crates/server/src/server.rs` (pass the cmd's buttons into the pass)
- Test: `crates/server/tests/triggers.rs`

**Interfaces:**
- Consumes: Task 3's pass, `GameHost::client_buttons` (`host.rs:203`).
- Produces: `touch_triggers` gains a `buttons: u8` parameter. The use bit is `vcod_common::net::msg::BUTTON_USE`, the same constant `use_button_pressed` reads (`crates/server/src/game/builtins/client.rs:69`); no new constant.

- [ ] **Step 1: Write the failing test**

```rust
/// A `trigger_use` notifies only while the use key is down; standing in it
/// with no key raises nothing. The MG42 mounts (`auto1`/`auto2` on every
/// stock map) are what this serves.
#[test]
fn a_trigger_use_needs_the_use_key() {
    let mut rt = ScriptRuntime::for_test(
        "main() { level.hits = 0; }\n\
         watch() { for(;;) { self waittill(\"trigger\", other); level.hits = level.hits + 1; } }\n",
    );
    rt.spawn_client_for_test(0, [0.0, 0.0, 0.0]);
    let mount = rt.spawn_map_entity_for_test([0.0, 0.0, 0.0]);
    rt.host.triggers.register(
        mount,
        TriggerKind::Use,
        [-64.0, -64.0, 0.0],
        [64.0, 64.0, 64.0],
        0,
        0,
    );
    rt.start_thread_for_test(mount, "watch", 0);
    rt.run_frame(0);

    rt.touch_triggers_with_buttons(0, 100, 0);
    rt.run_frame(100);
    assert_eq!(rt.level_field("hits"), vcod_gsc::Value::Int(0), "no use key");

    rt.touch_triggers_with_buttons(0, 200, vcod_common::net::msg::BUTTON_USE);
    rt.run_frame(200);
    assert_eq!(rt.level_field("hits"), vcod_gsc::Value::Int(1));
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p vcod-server --test triggers::a_trigger_use_needs_the_use_key`
Expected: FAIL — `touch_triggers_with_buttons` and `BUTTON_USE` do not exist.

- [ ] **Step 3: Implement**

The bit already exists as `vcod_common::net::msg::BUTTON_USE`; use it directly and define nothing new. Update the test above to name it that way rather than `vcod_server::game::trigger::BUTTON_USE`.

Rename `touch_triggers` to `touch_triggers_with_buttons(&mut self, slot: usize, now_ms: i32, buttons: u8)` and keep a `touch_triggers(slot, now_ms)` that passes `self.host.client_buttons[slot]`, which is what `Server` already mirrors in each frame. In the kind branch:

```rust
            // Retail answers a `trigger_use` from `Cmd_Activate_f`
            // (`ClientThink_real` 0x4064e), after the touch pass, so contact
            // alone raises nothing.
            if t.kind == TriggerKind::Use && buttons & BUTTON_USE == 0 {
                continue;
            }
```

Take that `continue` **before** `Triggers::fire`, so a use trigger touched without the key does not consume its `wait` window.

In `replay_moves`, push `(slot, cmd.buttons)` into `touched_slots` instead of the bare slot and pass the buttons through.

- [ ] **Step 4: Run the tests**

Run: `cargo test -p vcod-server`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/server/src/game/trigger.rs crates/server/src/game/script.rs \
        crates/server/src/server.rs crates/server/tests/triggers.rs
git commit -m "feat(server): a trigger_use answers the use key"
```

---

### Task 7: `trigger_lookat` and the cursor hint

**Files:**
- Modify: `crates/server/src/game/trigger.rs`
- Modify: `crates/server/src/game/script.rs`
- Test: `crates/server/tests/triggers.rs`

**Interfaces:**
- Consumes: Task 6's kind branch; the `serverCursorHintString` sentinel (object-model doc section 20, "`serverCursorHintString` 255 is a no-hint sentinel").
- Produces: `pub fn cursor_hint(&self, slot: usize) -> i32` on `ScriptRuntime`, and the per-client hint state on `GameHost`.

- [ ] **Step 1: Read the retail side first**

```bash
python3 tools/re/annotate_func.py private/server/main/game.mp.i386.so 0x5a0e4 | head -40
rg -n "serverCursorHint" docs/research/cod11-gsc-object-model.md
```

Record in the task's commit message what `setCursorHint` writes and where the playerstate carries it. `bombtrigger`, S&D's defuse trigger, is a `trigger_lookat` on every stock map that has one (VERIFIED, entity-lump census), so this kind is load-bearing for stage 3, not decoration.

- [ ] **Step 2: Write the failing test**

```rust
/// Standing in a `trigger_lookat` sets the client's cursor hint; leaving it
/// clears it back to the 255 no-hint sentinel.
#[test]
fn a_trigger_lookat_sets_and_clears_the_cursor_hint() {
    let mut rt = ScriptRuntime::for_test("main() {}\n");
    rt.spawn_client_for_test(0, [0.0, 0.0, 0.0]);
    let look = rt.spawn_map_entity_for_test([0.0, 0.0, 0.0]);
    rt.host.triggers.register(
        look,
        TriggerKind::LookAt,
        [-64.0, -64.0, 0.0],
        [64.0, 64.0, 64.0],
        0,
        0,
    );

    assert_eq!(rt.cursor_hint(0), 255, "no hint before any touch");
    rt.touch_triggers(0, 100);
    assert_ne!(rt.cursor_hint(0), 255, "standing in it sets a hint");

    rt.set_client_origin(0, [1000.0, 0.0, 0.0]);
    rt.touch_triggers(0, 200);
    assert_eq!(rt.cursor_hint(0), 255, "leaving clears it");
}
```

- [ ] **Step 3: Run it to verify it fails**

Run: `cargo test -p vcod-server --test triggers::a_trigger_lookat_sets_and_clears_the_cursor_hint`
Expected: FAIL — `cursor_hint` does not exist.

- [ ] **Step 4: Implement**

Add `pub client_cursor_hint: Vec<i32>` to `GameHost`, sized like `client_buttons` and initialised to 255 (`NO_CURSOR_HINT`). In `touch_triggers`, before the per-trigger loop, reset the touching client's hint to `NO_CURSOR_HINT`; in the `LookAt` arm, set it to the hint the entity carries (its `cursorhint` script field if present, otherwise the generic "use" hint value read in Step 1). Add:

```rust
    pub fn cursor_hint(&self, slot: usize) -> i32 {
        self.host
            .client_cursor_hint
            .get(slot)
            .copied()
            .unwrap_or(crate::game::trigger::NO_CURSOR_HINT)
    }
```

and mirror it into the playerstate where the other per-client host state is mirrored in `Server` (`rt.client_vitals(slot)` is the pattern to follow).

- [ ] **Step 5: Run the tests**

Run: `cargo test -p vcod-server`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/server/src/game/trigger.rs crates/server/src/game/script.rs \
        crates/server/src/game/host.rs crates/server/src/server.rs crates/server/tests/triggers.rs
git commit -m "feat(server): a trigger_lookat sets the cursor hint"
```

---

### Task 8: `solid`/`notsolid` reach the clip, and exploder brushmodels spawn non-solid

**Files:**
- Modify: `crates/server/src/game/builtins/entity.rs:287-295` (`set_solid`)
- Modify: `crates/server/src/game/spawn.rs` (the `script_brushmodel` spawn state)
- Test: `crates/server/tests/triggers.rs`

**Interfaces:**
- Consumes: `World::set_model_linked` (`crates/common/src/collision.rs:872`), `GameHost::world`.
- Produces: nothing new; this closes the section 3.7 defect.

- [ ] **Step 1: Write the failing test**

```rust
/// `notSolid()` takes a brush model's brushes out of the clip and `solid()`
/// puts them back, which is what `_utility.gsc`'s `brush_show` relies on.
/// Retail holds a submodel's brushes only through the linked entity
/// (docs/research/cod11-mantle.md, "A submodel's brushes").
#[test]
fn notsolid_unlinks_a_brush_models_brushes() {
    let Some(fs) = vcod_common::testing::game_fs() else {
        return;
    };
    // A map with a `script_brushmodel`: mp_depot places three.
    let (mut rt, ent) = load_map_and_find_brushmodel(&fs, "mp_depot");
    let world = rt.host.world.clone().expect("map loaded");

    let before = trace_hits_the_brushmodel(&world, ent);
    assert!(before, "the brushmodel clips while linked");

    rt.call_entity_builtin_for_test(ent, "notsolid");
    assert!(!trace_hits_the_brushmodel(&world, ent), "unlinked");

    rt.call_entity_builtin_for_test(ent, "solid");
    assert!(trace_hits_the_brushmodel(&world, ent), "relinked");
}
```

Write `load_map_and_find_brushmodel`, `trace_hits_the_brushmodel` and `call_entity_builtin_for_test` as helpers in the same test file. For the trace, use the same `box_trace` entry the slope gate uses (`rg -n "fn box_trace" crates/common/src/collision.rs` for the signature) with a short segment through the brushmodel's own bounds, taken from `host.model_bounds` for that entity's `*N`.

- [ ] **Step 2: Run it to verify it fails**

Run: `COD_DIR=<install> cargo test -p vcod-server --test triggers::notsolid_unlinks_a_brush_models_brushes`
Expected: FAIL — `set_solid` writes `e.solid` and never touches the world.

- [ ] **Step 3: Implement**

In `set_solid` (`builtins/entity.rs:287`), after writing `e.solid`, resolve the entity's `model` field; when it is `*N`, call `host.world`'s `collision.set_model_linked(n, solid)`. Keep the flag write: the wire build reads it.

Then in `spawn.rs`, give a `script_brushmodel` carrying a `script_exploder` key the spawn state retail gives it — hidden and non-solid, its brushes unlinked — so `mp_depot`, `mp_powcamp` and `mp_rocket` stop carrying collision retail does not. Confirm the spawn function's behaviour before writing it:

```bash
python3 tools/re/dump_builtins.py private/server/main/game.mp.i386.so spawns | rg script_brushmodel
python3 tools/re/annotate_func.py private/server/main/game.mp.i386.so <SP_script_brushmodel addr> | head -60
```

Label the resulting comment INFERRED where it rests on control flow.

- [ ] **Step 4: Run the tests**

Run: `COD_DIR=<install> cargo test -p vcod-server && cargo test -p vcod-server`
Expected: PASS both with and without `COD_DIR` (the second run skips the map test through `game_fs()`).

- [ ] **Step 5: Commit**

```bash
git add crates/server/src/game/builtins/entity.rs crates/server/src/game/spawn.rs crates/server/tests/triggers.rs
git commit -m "fix(server): solid and notSolid reach the clip, exploder brushmodels spawn non-solid"
```

---

### Task 9: The retail A/B gate

**Files:**
- Create: `crates/gsc/tests/fixtures/semantics/probe_trigger.gsc`
- Create: `crates/server/tests/fixtures/triggers/mp_pavlov-dm-triggers.txt` (the captured retail log)
- Create: `crates/server/tests/triggers_ab.rs`
- Modify: `crates/client/src/probe.rs` (a `--probe-triggers` walk mode)
- Modify: `crates/client/src/main.rs` (the flag and its doc comment)
- Modify: `AGENTS.md` (the probe-mode paragraph, as every other mode has one)

**Interfaces:**
- Consumes: everything above; the probe conventions in `crates/gsc/tests/fixtures/semantics/README.md` (read it first — six engine behaviours dictate a probe's shape).
- Produces: a committed retail fixture and a test that replays the same route against ours.

- [ ] **Step 1: Write the probe script**

`crates/gsc/tests/fixtures/semantics/probe_trigger.gsc`, which logs one line per notify. `logPrint` is the only output channel on a dedicated server:

```c
main()
{
	thread watch_triggers();
	maps\mp\gametypes\dm::main();
}

watch_triggers()
{
	triggers = getentarray("trigger_multiple", "classname");
	for (i = 0; i < triggers.size; i++)
		triggers[i] thread watch_one();

	hurts = getentarray("trigger_hurt", "classname");
	for (i = 0; i < hurts.size; i++)
		hurts[i] thread watch_one();

	uses = getentarray("trigger_use", "classname");
	for (i = 0; i < uses.size; i++)
		uses[i] thread watch_one();

	logPrint("PROBE triggers " + (triggers.size + hurts.size + uses.size) + "\n");
}

watch_one()
{
	self endon("death");
	num = self getEntityNumber();
	for (;;)
	{
		self waittill("trigger", other);
		logPrint("PROBE fire " + num + " " + other getEntityNumber() + " " + other.origin + "\n");
	}
}
```

Read the README's rules before running it: a runtime error kills the server, a compile error silently empties the whole section, and the homepath's loose `probe_*.gsc` count is capped at 31.

- [ ] **Step 2: Capture retail**

```bash
COD_LNXDED_HOME=<an absolute path with no '+' in it> \
PROBE_SECS=120 tools/run_probe.sh probe_trigger mp_pavlov
```

in one shell, and in another, once the server is up:

```bash
cargo run -p vcod -- --net-probe 127.0.0.1:28960 --probe-triggers --probe-secs 110
```

Save the `PROBE fire` lines, in order, to `crates/server/tests/fixtures/triggers/mp_pavlov-dm-triggers.txt`, with a header naming the map, the gametype, the date, the exact two commands and the route the probe walked. Every other fixture carries that header; match their format (`head -20 crates/server/tests/fixtures/playerstate/*-combat.txt`).

- [ ] **Step 3: Write the probe walk mode**

In `crates/client/src/probe.rs`, add `--probe-triggers`: join, then walk a fixed route across `mp_pavlov`'s minefield belt (26 `trigger_multiple`s named `minefield`; the belt's extent comes from the map's entity lump), printing one line per second with the origin and the health, and stopping at `--probe-secs`. Reuse `PvsProbe`'s station walk (`crates/client/src/probe.rs:5594`) rather than writing a second walker: the route is a station list and the existing `walkable` check handles geometry. The mode writes no fixture — the retail evidence is the server-side probe log.

Document the flag in `crates/client/src/main.rs` beside the others, and add a paragraph to `AGENTS.md`'s probe list saying what it does and that the fixture is the *server's* log, not a client capture.

- [ ] **Step 4: Write the A/B test**

`crates/server/tests/triggers_ab.rs`: load `mp_pavlov` into a `ScriptRuntime` with the same `dm` gametype, replay the route's origins through `touch_triggers` at the same cadence, collect our own `PROBE fire` equivalents, and diff against the fixture — entity numbers and firing order, not wall-clock times. Follow `crates/server/tests/configstrings_ab.rs` for the fixture-reading and diff-reporting shape, and gate the whole test on `vcod_common::testing::game_fs()` returning `Some`, so CI without `COD_DIR` skips it.

- [ ] **Step 5: Run it**

Run: `COD_DIR=<install> cargo test -p vcod-server --test triggers_ab`
Expected: PASS. A mismatch is a finding about ours, not about the fixture: retail is right, and the difference goes in a research doc with the evidence before any code changes to paper over it.

- [ ] **Step 6: Full check**

Run: `cargo fmt --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace && COD_DIR=<install> cargo test --workspace`
Expected: all clean.

- [ ] **Step 7: Commit**

```bash
git add crates/gsc/tests/fixtures/semantics/probe_trigger.gsc \
        crates/server/tests/fixtures/triggers/ crates/server/tests/triggers_ab.rs \
        crates/client/src/probe.rs crates/client/src/main.rs AGENTS.md
git commit -m "test(server): retail A/B gate for the touch pass"
```

---

## Notes for the executor

- One constant is still to be read out of the module rather than guessed:
  `trigger_hurt`'s `dmg` default (Task 5). The step says which command prints
  it. `TOUCH_BOX` (Task 3) is already measured at (40, 40, 52), with the
  `.data` mapping written down beside it.
- The broad-phase box being (40, 40, 52) around `ps.origin` rather than the
  player's own clip box is a fact `docs/research/` does not yet carry. Add it
  to the object-model doc's trigger material as part of Task 3's commit, with
  the address and the VERIFIED/INFERRED split the comment uses.
- The retail server and both probes need `COD_LNXDED_HOME` pointed at an
  absolute path with no `+` in it when run from a worktree — the engine splits
  its own command line on `+`.
- A run of `--probe-triggers` against our own server is fine and writes
  nothing, but any probe mode that *does* write a fixture overwrites committed
  retail evidence: move the file to `tmp/` and `git checkout` the fixture
  directory afterwards.
