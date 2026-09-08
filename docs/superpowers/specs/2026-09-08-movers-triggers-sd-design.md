# Triggers, movers and the S&D plant

Design for the touch pass, the scriptent mover verbs and the Search &
Destroy plant/defuse path on `vcod-server`.

Evidence labels are per claim, as `AGENTS.md` requires. VERIFIED is read out
of a binary, an asset or a capture; INFERRED covers anything read off control
flow, instruction sequencing included.

## 1. What the census changed about the brief

The brief opened from "a retail client sees every door on every stock map
welded shut". That premise does not hold, and two measurements taken while
scoping reshaped the work.

**No stock MP map contains an engine mover.** VERIFIED: entity lump (29) of
all 13 shipped MP BSPs contains zero `func_door`, `func_door_rotating`,
`func_rotating`, `func_bobbing` and `func_pendulum`. Section 13 of
`docs/research/cod11-gsc-object-model.md` reaches the same result from the
retail server's own `getEntArray` counts. `SP_func_door` (0x56bd4),
`InitMover` (0x56454), `SetMoverState` (0x55904), `Reached_BinaryMover`
(0x55c9c) and `Think_SpawnNewDoorTrigger` (0x57d8c) are all present in
`game.mp.i386.so` (VERIFIED, symbol table), but nothing in stock MP reaches
them. Engine binary movers are therefore custom-map territory and out of
scope here.

**Nothing with brushes moves in stock MP either.** VERIFIED: the retrieval
objective that `maps/MP/gametypes/re.gsc` drives with `moveto` is a
`script_model` (`classname script_model`, `targetname retrieval_objective`),
which carries no brushes. The only MP caller of `moveGravity` and
`rotateVelocity` is `brush_throw` in `maps/MP/_utility.gsc`, which runs on
entities whose `targetname` is `exploderchunk`; no MP map's entity lump
contains one. Every `script_exploder` entity in the stock set is either
`targetname "exploder"` (reaching `brush_show`, which calls `show()` and
`solid()` and moves nothing) or untargeted (reaching `brush_delete`).

**What does exist, and is what this design is about**, VERIFIED from the same
census, summed over the 13 maps: `trigger_multiple` x119, `trigger_hurt` x25,
`trigger_use` x15, `trigger_lookat` x11, `script_brushmodel` x30,
`script_model` x187. The gameplay riding on them is `sd.gsc`'s plant and
defuse (both `waittill("trigger", other)`), `_minefields.gsc`, out-of-bounds
`trigger_hurt`, the MG42 mount pairs (`auto1`/`auto2`, one `trigger_multiple`
and one `trigger_use` each) and the `re` gametype's objective.

Per-entity moving-brush clip is kept in scope anyway, as a deliberate call
for custom-map support, on the promote-on-first-motion design in section 4.

## 2. Stages

Three stages, one design, one implementation plan each, each with its own
gate.

1. **Triggers and the touch pass.** Bounds on brush entities, the touch pass,
   the `"trigger"` notify, the six trigger kinds. Oracle: a gsc probe script
   plus a walking net probe.
2. **Scriptmover motion.** The ten `scriptent` verbs on one integrator, the
   completion notifies, the wire fields, the moving clip. Oracle: a gsc probe
   script logging positions per frame, paired with a net probe reading the
   wire.
3. **The S&D plant and defuse.** The missing builtins, `linkTo`, the
   objectives, the bomb and its countdown. Oracle: two cooperating net probes
   plus a retail client by eye.

Stage 2 depends on stage 1 only for entity bounds and linking. Stage 3
depends on both.

## 3. Stage 1: triggers and the touch pass

### 3.1 Entity bounds

A trigger is a brush entity: its block carries `"model" "*N"` and lump 27
holds model N's bounds (VERIFIED, `bsp.rs` parses it today). `spawn.rs`
currently builds these entities with no bounds at all, and `is_touching`
(`crates/server/src/game/builtins/entity.rs:489`) approximates containment
with a 32-unit origin comparison. Bounds on brush entities come first, and
`istouching`, the touch pass and `linkTo` then share one definition of
containment.

Bounds are derived from the entity's *current* origin on read, never cached
at spawn: `sd.gsc` relocates its defuse trigger with
`bombtrigger.origin = level.bombmodel.origin`, and `re.gsc` does the same to
its objective's trigger after a `moveto` (VERIFIED, both scripts).

### 3.2 No wire path

Triggers never reach a client: the retail traces put a stock map's static
entity set at items, scriptmovers and turrets (`docs/protocol-1.1.md`,
"Which entities a client is sent"). Nothing in `wire.rs` changes for them.

### 3.3 The trigger table

New module `crates/server/src/game/trigger.rs`, shaped after `missile.rs`: a
`Triggers` table keyed by `EntId` holding the kind, the `wait`/`random`
keys, the next-fire time and the per-entity gate state. Host-side, beside the
object table rather than inside it, for the reason `Missiles` is.

Kinds served, all from the `spawns` table in object-model section 8:
`trigger_multiple`, `trigger_once`, `trigger_use`, `trigger_lookat`,
`trigger_hurt`, `trigger_damage`.

`delete()` drops a trigger's row. `sd.gsc` deletes both bombzones the instant
a plant completes, and a stale row would keep firing. Deletion shares the
`World::set_model_linked` hook, so one path covers the clip and the table.

### 3.4 The pass itself

VERIFIED: `G_TouchTriggers` is 0x3f88c and `ClientThink_real` calls it at
0x405b3, so it runs once per usercmd, not once per server frame. VERIFIED as
call targets, INFERRED as sequencing: inside `ClientThink_real` the order is
`Pmove` (0x40466), `BG_PlayerStateToEntityState`, `trap_LinkEntity`
(0x40595), `G_TouchTriggers` (0x405b3), `Cmd_Activate_f` (0x4064e).

In ours the pass runs inside `Server::replay_moves`, after each pmove step
and the entity-state mirror, with the use-key handling after it. It is part
of the existing "each client's queued usercmds" step of the documented tick
order; nothing else in that order moves.

One pass builds the client's abs box from `ps.origin` and the pmove bounds,
grown by the three `.rodata` floats `G_TouchTriggers` loads at 0x7dcdc,
0x7dce0 and 0x7dce4 (VERIFIED). Retail then calls `trap_EntitiesInBox` and
`trap_EntityContact` per candidate (VERIFIED, relocations in that function).
Ours runs the same two-stage test against the trigger table directly: a
linear scan, at 38 triggers on the worst map (`mp_brecourt`), which keeps
triggers out of the cluster-linking path that exists for the wire.

### 3.5 The notify

`G_TouchTriggers` raises it itself: `Scr_Notify` with `Scr_AddEntity`
supplying `other` and `scr_const` supplying the event atom (VERIFIED,
relocations). Ours queues `Vm::notify(Target::Entity(trigger), "trigger",
[toucher])` during `replay_moves`; parked threads wake in the script frame
later in the same tick, which is the existing rule that work found during the
move pass queues rather than reentering the VM mid-frame.

### 3.6 Per kind

- `trigger_multiple`: gated by its `wait`/`random` keys through a per-entity
  next-fire time.
- `trigger_once`: frees itself after one fire.
  ANNOTATION 2026-09-08: ours latches the refire window shut instead, so the
  entity survives in the object table and a `getEntArray` would see a
  difference. Whether retail really frees it is unmeasured — this line was
  written off Q3's `trigger_once`, not off the module — so the divergence is
  recorded rather than closed. Measuring it is a follow-up.
- `trigger_hurt`: damage through `finishPlayerDamage`, with the sound alias
  `spawn.rs` already registers (`SP_trigger_hurt` 0x64ef8, VERIFIED).
- `trigger_use`: fires from the use key rather than from contact.
- `trigger_lookat`: sets the cursor hint (`setCursorHint` 0x5a0e4; the
  `serverCursorHintString` 255 sentinel is object-model section 20). Not
  cosmetic: `bombtrigger`, S&D's defuse trigger, is a `trigger_lookat` on
  every map that has one (VERIFIED, census).
  ANNOTATION 2026-09-08: the cursor-hint claim is disproven — `SP_trigger_lookat`
  touches no hint field and no touch path writes one (object-model section 20).
  So is the assumption that the kind fires on contact: the broad phase's
  contents mask excludes its `r.contents` bit (object-model 22.1), and the
  module's only reader of the classname is an aim trace. Ours registers the
  kind and notifies it on nothing; the aim trace is unmodelled.
- `trigger_damage`: kind recognised, no missile touch path. Retail gives
  missiles their own (`G_GrenadeTouchTriggerDamage` 0x655a0), and no stock MP
  map places a `trigger_damage`, so it stays out.

### 3.7 A correctness fix that falls out

`solid`/`notsolid` exist as builtins (`builtins/entity.rs:287`) but only set
`e.solid` on the host entity; they never reach `World::set_model_linked`.
Retail's `SP_script_brushmodel` spawns a `script_exploder` entity hidden and
non-solid until `brush_show` calls `solid()` (INFERRED from the script and
the spawn function). We link those brushes at load, so `mp_depot`,
`mp_powcamp` and `mp_rocket` currently carry collision retail does not.
Wiring the existing flag to the clip, plus the spawn-time non-solid state,
closes it.

Correction, from implementing it: the sentence above is wrong about the
spawn function and there is no spawn-time state to add. VERIFIED:
`SP_script_brushmodel` (game.mp 0x60fb8) is `trap_SetBrushModel`,
`InitScriptMover`, a store of 1 to `r.contents` and `trap_LinkEntity`, with
no other instruction. INFERRED, from that being the whole body: it reads no
`script_exploder` key and gives an exploder brush model no state of its
own. `maps/MP/_load.gsc` is what calls `notsolid()` on the four that have
one, so wiring the builtin to the clip is the whole fix. The evidence and
the census are in `docs/research/cod11-mantle.md`, "A submodel's brushes
are its entity's".

## 4. Stage 2: the mover integrator, the wire and the moving clip

### 4.1 State

`builtins/mover.rs`'s stub grows into `game/mover.rs`: a `Movers` table keyed
by `EntId` beside `Missiles` and `Triggers`, each row holding a `pos` and an
`apos` trajectory plus the pending completion notify for each. The trajectory
is the state, so `getorigin`, the wire and the clip all read one thing.

### 4.2 Ten verbs, four shapes

Linear-to-target (`moveto`, `movex`, `movey`, `movez`) and angular-to-target
(`rotateto`, `rotatepitch`, `rotateyaw`, `rotateroll`) each set a bounded
trajectory with a duration; `movegravity` sets a gravity trajectory on `pos`;
`rotatevelocity` sets an unbounded one on `apos`. The axis verbs are the
target verb with two components held, so there is one implementation per
family.

### 4.3 What the probe decides

Three things the brief lists as unmeasured, and this design deliberately does
not guess at, following `mover.rs`'s own precedent:

1. The unit of the time argument. `re.gsc` computes
   `speed = distance(loc, end_loc) / 250` and passes it as that argument
   (VERIFIED), which reads as seconds at 250 units/s, but that is INFERRED
   until a probe measures it.
2. What `accel` and `decel` mean numerically (an ease fraction, seconds of
   ramp, or units/s^2).
3. Whether retail puts per-frame origins on the wire or a trajectory the
   client extrapolates. This one is load-bearing rather than cosmetic: a
   retail client predicts its own movement against a mover it learns from
   those fields, so the wrong choice shows up as client prediction
   disagreement, not as a server-side error.

The integrator is written against a `MoveCurve` mapping (duration, accel,
decel) onto a `trType` and a delta; the probe fills the mapping in before the
integrator is written.

### 4.4 Wire

Nothing new is needed. `wire.rs` already builds `ET_SCRIPTMOVER` (8) for
static script models, and `missile.rs` already emits non-stationary
`pos`/`apos` groups evaluated by `crates/common/src/net/trajectory.rs` under
the divergence-#8 enumeration in `docs/protocol-1.1.md`. A moving
scriptmover is an existing entity kind carrying an existing field group with
a non-zero `trDelta`.

### 4.5 The moving clip

Promote on first motion. A submodel is baked into the world BVH at load, as
today. The first mover verb on its entity clears its existing `model_linked`
bit and appends the model to a `DynamicModels` list holding its brush set and
a current transform. `box_trace`, `shot_trace` and `missile_trace` each gain
one pass over that list, transforming the ray into model space
(inverse-translate, and inverse-rotate when `apos` is non-trivial) and
sweeping the shapes they sweep today.

The list is empty on all 13 stock maps, so the stock trace path is unchanged
and `playerstate_slope_ab.rs` stays valid. The alternative — pulling every
submodel out of the baked BVH and clipping retail's way, entity by entity, on
every trace — was rejected for making every map pay for a feature no stock
map uses, in the one subsystem measured against retail to the unit.

## 5. Stage 3: the S&D plant and defuse

### 5.1 Missing builtins

Checked against the corpus, not guessed. Missing today: `linkto` (0x59cc4),
`unlink`, `enablelinkto` (0x5d5d0), `scaleovertime` (0x4bd34) with its
`fadeovertime` and `moveovertime` siblings in the same hudelem table,
`isonground` (`PlayerCmd_isOnGround`), `isalive`, `stoploopsound`,
`logprint`, and the objective family (`objective_add` 0x5a2d0 through
`objective_team` 0x5e2c8). All addresses VERIFIED from
`tools/re/dump_builtins.py`.

Already present, and enough for the rest of the path: `istouching`,
`usebuttonpressed`, `newclienthudelem`, `setshader`, `playsound`,
`playloopsound`, `playfx`, `radiusdamage`, `spawn`, `setmodel`, `hide`,
`show`, `destroy`, `precachemodel`, `getentitynumber`, `vectortoangles`.

### 5.2 `linkTo`

The one genuinely unmeasured piece. Stock S&D links the planting player to a
static bombzone, so its visible effect is a player pinned for the plant's
duration. Whether retail freezes that player's pmove or only re-anchors
`ps.origin` each frame is not answerable from the corpus, and the two differ
the moment a linked player presses a movement key. The plant capture settles
it: the attacker probe sends a walk input mid-plant, and whether its origin
moves is the measurement. `enableLinkTo`, which retail gates linking on, and
`G_SetFixedLink` (called from `G_RunClient`) are the addresses to read
alongside it.

### 5.3 Objectives

A configstring range, covered by machinery that exists: `--net-probe
--save-configstrings` against retail during a live plant captures the
objective slots as they change, and `crates/server/tests/configstrings_ab.rs`
compares ours. No new harness.

### 5.4 The round path

After the plant it is ordinary script on existing builtins: the bomb model
spawned and given a loop sound, `bomb_think` waiting on the defuse trigger,
`bomb_countdown` waiting 60 s, `radiusDamage` at the end, and `endRound`
reaching the `exitLevel` path the map-cycle work already built. The only new
engine behaviour past the builtins above is `logPrint`, writing the `A;` line
into `games_mp.log` beside the `D;` and `K;` records already there.

## 6. Testing and gates

### 6.1 Stage 1

Unit tests: bounds derived from a submodel and following an origin write;
`multi_trigger`'s `wait`/`random` gating; `trigger_once` freeing itself; a
deleted trigger going quiet; `trigger_hurt` damage reaching
`finishPlayerDamage`.

A/B evidence pairs two harnesses that already exist. A `probe_trigger.gsc`
gametype script under `tools/run_probe.sh` logs a `PROBE` line per
`"trigger"` notify with the trigger's entity number and the toucher; a
`--net-probe` client walks a fixed route across `mp_pavlov`'s minefield belt
(26 `trigger_multiple`s named `minefield`; brecourt 32, hurtgen 14,
rocket 10). That is notify-level evidence rather than a health drop inferred
back to a notify, and the script and route replay against ours. Triggers are
not on the wire, so the fixture is the probe log, not a netchan capture.

### 6.2 Stage 2

`probe_mover.gsc` calls each verb family on a `script_origin` and logs
`getorigin()` and `self.angles` every frame: a pure script oracle, no client,
and the thing that settles the time unit, the accel/decel mapping and the
completion notify names. Paired with one `--net-probe` run recording the same
entity's fields, it also answers the per-frame-origins versus trajectory
question. Both replay against ours.

### 6.3 Stage 3

Two probes against `tools/run_server.sh mp_carentan +set g_gametype sd`: an
attacker walking to `bombzone_A` and holding use through a full plant, a
defender walking to the planted bomb and holding use through a defuse.
`--save-configstrings` covers the objectives. `linkTo` is measured inside the
same capture (section 5.2). Then a retail client by eye: plant, defuse, and
the countdown explosion.

### 6.4 Housekeeping

Every new probe mode writes committed retail evidence and carries the warning
the existing ones do: a run against ours overwrites the fixture, so move it
to `tmp/` and `git checkout` the directory afterwards. All fixtures replay
without `COD_DIR` so CI stays green. `cargo fmt`, `cargo clippy -D warnings`
and `cargo test` clean per stage.

## 7. Out of scope

- Engine binary movers (`func_door` and the rest of the `SP_func_*` family).
  No stock MP map places one.
- Missile-versus-trigger touch (`G_GrenadeTouchTriggerDamage`). No stock MP
  map places a `trigger_damage`.
- The killcam, and item pickup, which remain unmodelled from earlier work.
