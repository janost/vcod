# CoD 1.1 MP: the scriptent mover verbs

What `moveto`, `movex`/`movey`/`movez`, `movegravity`, `rotateto`,
`rotatepitch`/`rotateyaw`/`rotateroll` and `rotatevelocity` do, their units,
and what they put on the wire.

Evidence for sections 1-10: one paired capture against the retail 1.1d Linux
dedicated server, mp_pavlov under `dm`, 2026-09-09.

| File | Half |
|---|---|
| `crates/server/tests/fixtures/movers/mp_pavlov-dm-movers.txt` | server: `getorigin()` and `.angles` once per server frame through each verb, logged by `crates/gsc/tests/fixtures/semantics/client-probes/probe_mover.gsc` under `tools/run_probe.sh` |
| `crates/server/tests/fixtures/movers/mp_pavlov-dm-movers-wire.txt` | client: every change to an entity's `pos`/`apos` group, from a `--net-probe` on the same server |

The verb-to-handler map is the `scriptent` method table at `game.mp.i386.so`
`0x78d40`, walked by `ScriptEnt_GetMethod` (`0x60f60`);
`docs/research/cod11-gsc-object-model.md` section 9 lists it. The trajectory
enumeration is `docs/protocol-1.1.md`, divergence 8.

## 1. Which entities accept a mover verb

VERIFIED: only three classnames. A `movez` on a `mpweapon_panzerfaust` is the
fatal script runtime error

```
entity 258 is not a script_brushmodel, script_model, or script_origin
```

not a silent no-op.

## 2. The time argument is seconds

VERIFIED. `moveto((600,0,100), 1)` from `(100,0,100)` took 1000 ms of level
clock and `moveto((600,0,100), 2)` took 2000, from the first moved frame to
the `movedone` frame.

## 3. The axis verbs are deltas; the target verbs are absolute

VERIFIED. `movex(500, 2)` from `x = 100` and `moveto((600,0,100), 2)` from the
same start both ended at `x = 600`.

VERIFIED: two `rotateyaw(90, 2)` calls from yaw 0 ran the second from 90 to
180. `rotatepitch`/`rotateyaw`/`rotateroll` are deltas; `rotateto` takes
absolute angles.

## 4. The motion law: a trapezoidal velocity profile

`moveto`/`movex`/`movey`/`movez` and `rotateto`/`rotatepitch`/`rotateyaw`/
`rotateroll` take optional `accel` and `decel` after the duration, both
**seconds of ramp**, default 0.

VERIFIED from the per-frame origins. For distance `D` over `T` seconds with
ramps `ta` and `td`:

```
v = D / (T - ta/2 - td/2)
```

Constant acceleration 0 to `v` for `ta`, cruise at `v`, constant deceleration
to 0 for `td`.

| verb | D | T | ta | td | v predicted | v measured |
|---|---|---|---|---|---|---|
| `moveto` | 500 | 1 | 0 | 0 | 500 u/s | 500 u/s (25.00 per 50 ms frame) |
| `moveto` | 500 | 2 | 0 | 0 | 250 u/s | 250 u/s (12.50 per frame) |
| `moveto` | 500 | 2 | 0.5 | 0.5 | 333.33 u/s | 333.33 u/s (16.67 per frame) |
| `moveto` | 500 | 2 | 0.5 | 0 | 285.71 u/s | 285.71 u/s (14.29 per frame) |
| `rotateto` | 90 deg | 2 | 0.5 | 0.5 | 60 deg/s | 60 deg/s (3.00 per frame) |

The accel-only row shows the two ends are independent. The ramp is exact
constant acceleration, not a smoothstep: with `ta = 0.5`, `v = 333.33` the
displacement at 50 ms is 0.83, `½ (v/ta) t²`. With no ramps the motion is
linear and the entity is at its destination on the `movedone` frame.

## 5. `movegravity`

VERIFIED. `movegravity((0,0,300), 3)` from `z = 100` traced
`z(t) = 100 + 300t - 400t²`: peak 156.25 at t = 0.375, -2600.00 at t = 3. The
vector is an initial velocity in u/s; gravity is 800 u/s², the
`DEFAULT_GRAVITY` in `crates/common/src/net/trajectory.rs`.

The time argument is when `movedone` fires and the motion stops; the entity
holds where it was, still falling, on that frame. VERIFIED, `probe_moverreads`
(section 23): `movegravity((0, -10, 0), 1)` started at 2000 read z -336 at
3000 and every frame after. VERIFIED, game.mp `0x5f93b`: the segment setup
`Reached_ScriptMover` calls (0x5f7b0) evaluates a `TR_GRAVITY` (5) `pos` at
`level.time` into its own `trBase` and makes it stationary; every other type
takes the target.

## 6. `rotatevelocity`

VERIFIED. `rotatevelocity((0,180,0), 2, 0, 0)` turned yaw 9 degrees per 50 ms
frame (180 deg/s) for exactly 2 s, ending at 0 after a full turn, and raised
`rotatedone`. The vector is deg/s, the second argument a duration, accel and
decel as section 4.

## 7. The completion notifies

VERIFIED. The four linear families and `movegravity` raise `"movedone"`; the
four angular families and `rotatevelocity` raise `"rotatedone"`, on the entity,
on the frame the duration elapses.

## 8. Script-visible state while a mover runs

VERIFIED. `getorigin()` and `.angles` read the interpolated value every frame;
the field is written back. After `rotatevelocity` turned a full 360 `.angles`
read `(0, 0, 0)` while the wire carried 360 (section 9). INFERRED, off section
12's "The residual yaw": that is `Reached_ScriptMover`'s `AngleNormalize360`
at the end of a rotate; while it runs `.angles` is raw. VERIFIED,
`probe_moverreads` (section 23): `rotateyaw(-90, 1)` from 0 read -4.5, -9, ...
-85.5, then 270 once it ended; `rotateroll(-400, 1)` read down to -380, then
-39.9957.

VERIFIED, `probe_moverreads`: `.origin` moved every frame of a `movez` and a
`movegravity`, `r.currentOrigin` plus the frame's stored `move`.

**Script sees the trajectory one server frame late.** VERIFIED, both halves of
the capture: `moveto((600,0,100), 1)` called at 1050 put `trTime` 1050 on the
wire, yet `getorigin()` read the start at 1100, the first moved value (125) at
1150, and `movedone` came at 2100. `rotateyaw(90, 2)` called at 13450 raised
`rotatedone` at 15500. Script at level time `T` reads the trajectory at
`T - 50` (one frame at `sv_fps` 20), and the notify comes on the first frame
whose lagged clock has passed the end. UNVERIFIED: one frame or a fixed 50 ms
(one frame rate captured). INFERRED, off the entity pass running after the
threads (combat doc 14.7, "The pass runs after the threads"): `G_RunMover`
evaluates at `level.time` in the pass, script reads that next frame, and the
notify wakes its waiter on the next frame's threads. vcod runs the mover on
its turn in the pass (`mover::run_one`) and queues the notify for the next
frame's script (`GameHost::engine_notifies`).

## 9. What goes on the wire

VERIFIED: retail sends **a trajectory the client extrapolates**, not
per-frame origins. A two-second move is two snapshot updates, one at each end.

`trType_t` values from `crates/common/src/net/trajectory.rs`:

| verb | group | at the call | at the end |
|---|---|---|---|
| `moveto`/`movex`/`movey`/`movez`, no ramps | `pos` | `trType` 3 (`TR_LINEAR_STOP`), `trTime` = call time, `trDuration` = ms, `trBase` = start, `trDelta` = velocity u/s | `trType` 0, `trBase` = end, `trTime` = completion time; `trDelta`, `trDuration` stale |
| the same with ramps | `pos` | one trajectory per segment: `trType` 9 (`TR_ACCELERATE`) for `ta` ms, 3 for the cruise, 10 (`TR_DECCELERATE`) for `td` ms; each with `trDelta` = cruise `v` and `trBase` at its own start | `trType` 0 at the end origin |
| `movegravity` | `pos` | `trType` 5 (`TR_GRAVITY`), `trDuration` = ms, `trDelta` = initial velocity | not observed (probe deleted the entity first) |
| `rotateto`/`rotatepitch`/`rotateyaw`/`rotateroll`/`rotatevelocity` | `apos` | `trType` 3, `trDelta` = deg/s | `trType` 0, `trBase` = angle reached, unnormalized: a full turn read `(0, 360, 0)` |

Measured: `movez(96, 2, 0.5, 0.5)` from `z = 592` sent

| trType | trDuration | base z | delta z |
|---|---|---|---|
| 9 | 500 | 592 | 64 |
| 3 | 1000 | 608 | 64 |
| 10 | 500 | 672 | 64 |
| 0 | | 688 | |

The segments cover 16, 64 and 16 units; 64 = `96 / (2 - 0.25 - 0.25)`.

A mover is `eType` 8 (`ET_SCRIPTMOVER`) throughout; starting or stopping
changes nothing about the entity kind.

## 10. What is not measured here

- A rotation on more than one axis at once (section 23 reads a three-axis
  `rotateto` and `rotatevelocity` through `.angles` only).
- A brush model with an origin brush, so a pivot other than the world origin.

A second verb on a moving entity replaces the trajectory (section 12).

## 11. A brush model mover's clip follows it

Evidence for sections 11-14: a second paired capture against retail 1.1d,
mp_carentan under a `dm`-shaped probe, recaptured 2026-10-20 (port 33801).

| File | Contents |
|---|---|
| `crates/server/tests/fixtures/movers/mp_carentan-dm-ride.txt` | server: one `PROBE f` line per frame of a phase, from `crates/gsc/tests/fixtures/semantics/client-probes/probe_ride.gsc`, then the client's `CMDS` lines |
| `mp_carentan-dm-ride-wire.txt` | client: the `--probe-ride` playerstate per snapshot, the slab's `RIDE_ENT` and `RIDE_GONE` lines, every trajectory change |

The probe renames the two bombzone brush models' `script_gameobjectname` to
`dm` so `_gameobjects` keeps them, stands the player on brush 4281 of model
`*5` (entity 177, a plank slab, top z -22) and runs one verb per phase.

VERIFIED, off this capture and the 2026-10-05 one it replaced: where a
`setOrigin` a unit over the courtyard terrain comes to rest depends on the
client's cmds.

| Phase | 2026-10-05 | 2026-10-20 |
|---|---|---|
| before `push_y` | -30, -30.445, -31.819 | -30, -30.683, -31.875 |
| under `lift` | rests -31.84 | rests -31.84 |
| before `bm_hide`, after the unlink | | -30, -31.156, -31.836 |

`ride_ab.rs` replays the capture's cmds (`ProbeHold::framed`), linked frames
included, and every snapshot from the first placement matches retail's to
three decimals; on one cmd a frame 602 of 981 do not (VERIFIED, vcod
measurement). The old run logged no cmds; its -31.819 is why `REST_GAPS`
existed.

VERIFIED, off the capture (`PROBE bm 177`): a script_brushmodel with no
`origin`, `angles` or `model` key reads `getorigin()` and `.angles`
`(0, 0, 0)` and `.model` `""`. Its brushes are in model space, so the
entity's origin (here the world origin) is the pivot of a rotate.

VERIFIED, `Scr_GetGenericField` (game.mp `0x6248c`): the type-8 arm (the
`model` field) reads the model byte at `ent+0x175` through `G_ModelName`
(`0x66f8c`), configstring `0x10c` plus that index; a byte of 0 is the model
block's slot 0. INFERRED, off the `""` above and `G_ParseField` sending a
`*N` to `ent+0x8c` (`docs/research/cod11-gsc-object-model.md`): any entity
whose model byte was never set reads `""`, a brush model included. vcod keeps
a BSP `*N` apart from the field; an unset `model` reads `""`.

VERIFIED, off the capture: the brushes move with the entity. A player on the
slab rides `movez(48, 2)` up and down and `movex(48, 2)` across and back, and
is pushed aside when it moves sideways into him (section 12). INFERRED: the
inline model's clip sits at the entity's `r.currentOrigin` and
`r.currentAngles` every trace, which `G_MoverPush` writes (section 12) and is
the only state the push and the traces share.

VERIFIED, `G_RunFrame` (game.mp `0x50478`): the entity loop at
`0x50912`-`0x50975` calls the per-entity runner `0x502bc` for an in-use entity
(byte `ent+0x160`), and before it, at `0x50949`, for the entity the first
dword of the link record `ent+0x2e4` points at. The runner's arms:

| Call | Condition | Address |
|---|---|---|
| `G_RunMissile` | `eType` 4 | `0x50375` |
| `G_RunItem` | byte `ent+0x161` set | `0x503f5` |
| `G_RunMover` | `eType` 5 or 8 | `0x5040e` |
| `G_RunClient` | has a client | `0x50422` |

INFERRED, off those branches: a linked entity's parent runs ahead of it in
the frame, and `ent+0x194` against the level time (`0x502cb`) keeps either
from running twice.

VERIFIED, `G_RunMover` (`0x5760c`): it calls `G_GeneralLink` when the entity
has a link record (`0x57623`), `G_MoverTeam` when `pos.trType` (`ent+0xc`) or
`apos.trType` (`ent+0x30`) is non-zero (`0x57676`), then `G_RunThink`
(`0x57682`). VERIFIED, `G_MoverTeam` (`0x55684`): it evaluates `pos` and
`apos` at `level.time` (`0x556c1`, `0x556d7`) and passes the push function
`0x550f0` the differences from `r.currentOrigin` (`ent+0x134`) and
`r.currentAngles` (`ent+0x140`) as `move` and `amove`. VERIFIED, off the
`Scr_RunCurrentThreads` calls at `0x50653` and `0x506d0` and the entity loop
at `0x50912`: script threads run before the entity pass in `G_RunFrame`.
INFERRED: the clip is on the trajectory at the level time, one frame ahead of
section 8's `getorigin()`, and on the playerstate's clock: `movez(48, 2)`
called at 11250 put the rider at z -20.675 on the 11300 snapshot (one frame
of 24 u/s) while script read the slab at z 0.

vcod: `CollisionWorld::set_model_pose` places a submodel's brushes by origin
and angles; traces skip a posed model in the BVH and clip its moved planes
after. `crate::game::mover` poses a mover's model at the level time each
frame.

## 12. `G_MoverPush`: riders, pushes and a blocked mover

VERIFIED, the push function `0x550f0` (`G_MoverPush` by its shape; no symbol):

| Step | Address |
|---|---|
| Bound the move: `r.absmin`/`r.absmax` plus `move` when the pusher's angles and `amove` are all zero, else `RadiusFromBounds` about the origin | `0x5518c` |
| Unlink the pusher, `trap_EntitiesInBox` with mask `0x2000180` | `0x5533c` |
| Add `move`/`amove` to `r.currentOrigin`/`r.currentAngles`, relink the pusher | `0x553ae` |
| Per listed entity: `eType` 1, 3 or 4 and byte `ent+0x161` | `0x553e0`-`0x553fa` |
| `groundEntityNum` (`ent+0x7c`) against the pusher's number | `0x55405` |
| `absmin`/`absmax` against the bounds | `0x5540e`-`0x5548f` |
| Skip the trace when the clipmask is set and `r.contents` is `0x4000000` | `0x554a5` |
| `trap_Trace` with the entity's `mins`/`maxs` at its `r.currentOrigin`, skip = the entity (its `r.ownerNum` for `eType` 4), mask = `clipmask` (`ent+0x190`) or 0x11 when 0 | `0x554ee` |
| Trace's entity against the pusher | `0x55515` |

INFERRED: a player is pushed when he stands on the pusher or his box overlaps
the pusher where it now is; a corpse never is (its contents are outside the
box mask).

VERIFIED, `cod_lnxded`: the area-node walk at `0x08059590` (the function
printing `CM_AreaEntities: MAXCOUNT`) lists an entity only when its
`r.contents` (`+0x118`) shares a bit with the mask, and descends a node only
when the node's contents word does. INFERRED: that mask is
`trap_EntitiesInBox`'s fifth argument, so `0x2000180` filters by contents
before any `eType` test.

VERIFIED, the contents writers in game.mp:

| Writer | Contents | Address |
|---|---|---|
| playing client | `0x2000000` | |
| `player_die` (dying player) | `0x4000000` | `0x49c28` |
| `G_SpawnItem`, `LaunchItem` | `0x407c0108` | `0x4e778`, `0x4dc97` |
| `RespawnItem` | `0x407c0008` | `0x4ece9` |
| `Touch_Item` | 0 | `0x4d90c`, `0x4d9ce` |
| `fire_grenade`, `fire_rocket` | none (clipmask only) | `0x544a0`, `0x546ef` |
| body-queue clone | 0 (`docs/research/cod11-combat.md` 5.2) | |

INFERRED: the list holds live players and items lying in the world; never a
grenade, rocket, corpse, dead player, taken item, or one `RespawnItem`
brought back (`0x407c0008` lacks `0x100`). The `eType` 4 arm and the
`0x4000000` test are unreachable through this query.

VERIFIED, mp_carentan's BSP: all ten brushes of model `*5` are
`textures/common/clip_metal`, contents `0x280306c0` (`0x80` and `0x10000` set,
`0x1` and `0x10` clear). A player's clipmask `0x2810011` sees them through
`0x10000`; an item's push trace mask `0x11` sees neither.

VERIFIED, paired capture against retail 1.1d, mp_carentan, `dm`-shaped probe,
2026-10-06: `crates/server/tests/fixtures/movers/mp_carentan-dm-push.txt`
(`PROBE` lines from `client-probes/probe_push.gsc`, `ITEM` lines from the
`--probe-items` client). The probe drops carbine `a` on the slab, health pack
`b` on the ground south of it, hangs carbine `c` there (`spawnflags 1`), then
runs probe_ride's verbs.

- **Carried.** `a` rests on the slab with ground 177 and follows it to the
  hundredth through `movez`, `movex` and both `rotateyaw`s, ground 177 every
  snapshot, the slab's position in `pos.trBase`, `trType` 0. From `push_y` on
  it creeps +x 0.021 a frame (the residual yaw, below).
- **Not shoved.** `b` and `c` never move; the slab sweeps through both in
  `ride_yaw` and `push_y`, `b`'s ground stays 1022 and `c`'s 0, and the slab
  lowered onto `b` (`crush`) never stalls.

INFERRED, off the listing, masks and capture: an item resting on a mover is
carried because the ground test keeps it before any trace. An item in the way
is shoved only when its push mask sees the mover's brushes: a script-spawned
or placed item's `0x11` misses a clip brush, a dropped item's `0x81` would
see it through `0x80` (not captured). An item is 2 wide, so `maxs.x * 0.5`
never passes the jitter's 4, and an item that fits nowhere is relinked in
place without stalling the mover. `G_TryPushingEntity` writes `0x3ff` to the
ground of anything it moves off another ground or leaves in place, and
`G_RunItem` drops from that.

VERIFIED, the list box (`0x550f0`..`0x5530a`): `r.absmin`/`r.absmax` when
`r.currentAngles` and `amove` are all zero, else a cube of
`RadiusFromBounds(r.mins, r.maxs)` about `r.currentOrigin`, both before the
move, then stretched along `move` per axis (max side when positive, min side
otherwise; sign test at `0x552e8`).

VERIFIED, then, in order:

1. The filter loop (`0x553d4`..`0x5553c`) copies each kept entity number into
   a second array.
2. A loop unlinks every kept entity (`0x55550`..`0x55570`, call `0x55561`).
3. A loop hands each, in list order, to `G_TryPushingEntity` (`0x555c8`)
   after storing it and its `r.currentOrigin` at `pushed_p`
   (`0x55598`..`0x555bb`), and relinks it on a 1 (`0x555e1`). On a 0: an
   `eType` 3 entity is relinked and skipped (`0x555d4`); else a pusher whose
   `pos.trType` or `apos.trType` is 4 (`TR_SINE`) calls `G_Damage(check,
   pusher, pusher, 0, 0, 99999, 0, 0x13, 0)` (`0x55617`) and goes on with the
   entity unlinked; any other stores the entity through its fourth argument
   and returns 0 (`0x55621`) straight to the epilogue.
4. When the list runs out, a last loop links every kept entity again in list
   order (`0x55650`..`0x55670`, call `0x55661`) and returns 1.

VERIFIED: nothing in `0x550f0`..`0x553ae` stores through `pushed`; the pusher
is not on the undo list. INFERRED: a push reverses the kept entities in their
node's list (each relink prepends, combat doc 14.7); the last loop finds each
already in its node and keeps its place; a blocked push leaves the blocker and
every kept entity listed after it out of the tree until something links them
(a player's next cmd).

VERIFIED, paired capture against retail 1.1d, mp_carentan, two clients,
2026-10-09: `crates/server/tests/fixtures/movers/mp_carentan-dm-pushorder.txt`
(`PROBE` lines from `client-probes/probe_pushorder.gsc`). Two players set down
in the slab's path, slot 0 then 1, are walked 1 then 0 by a flat
`radiusDamage`. Each `movey` that pushes both reverses the walk (0,1; then
1,0; then 0,1); a `movey` away from them leaves it.

VERIFIED, `G_TryPushingEntity` (`0x54930`):

- Returns 0 when the pusher's `eFlags` carry 0x04000000 (byte `+0xb` bit 4,
  `0x54942`) and the entity's ground is not the pusher.
- Adds `move` to `r.currentOrigin` (`0x54956`-`0x54982`), calls
  `AngleVectors(amove)` and `VectorInverse` on the right vector (`0x5498c`,
  `0x54995`), turns the result about the pusher's moved `r.currentOrigin`
  (`0x549d5`-`0x54aae`), and traces there with the entity's box (`0x54b3b`).
- Clear trace: writes `0x3ff` to `groundEntityNum` unless it was the pusher
  (`0x54b77`), the spot into `r.currentOrigin` and `s.pos.trBase`; for a
  client adds `(int)(amove[1] * 182.044) & 0xffff` to `ps.delta_angles[1]`
  (`0x54bb5`-`0x54bf7`, 182.044 at rodata `0x75bf4`) and writes `ps.origin`
  (`0x54c00`-`0x54c1e`); steps `pushed_p` (`0x54f41`), returns 1.
- When `maxs.x * 0.5` exceeds 4 (`0x54c23`, rodata `0x75bf8`, `0x75c00`) it
  tries offsets in steps of 4 (rodata `0x75c08`) with the same trace and
  success path (`0x54db0`, `0x54e21`, `0x54e8e`).
- Last it traces the unmoved `r.currentOrigin` (`0x55084`, `0x550a5`); clear,
  it writes `0x3ff` to `groundEntityNum` and returns 1 without stepping
  `pushed_p` (`0x550cd`-`0x550d7`). Otherwise 0.

INFERRED: the offsets are RTCW's jitter, `z` 0 then ±4, `x` ±4, `y` ±4, in
that nesting; a player is 15 wide, so he reaches 4 and no further.

VERIFIED, `G_MoverTeam` on a 0 from the push: it walks the `pushed` records
newest first (`0x55744`-`0x557ef`), restoring `r.currentOrigin`,
`s.pos.trBase`, a client's `ps.origin`, subtracting `ANGLE2SHORT(record yaw)`
from `ps.delta_angles[1]` (`0x557b3`), and links each (`0x557de`). Then per
team part it adds `level.time - level.previousTime` to `pos.trTime` and
`apos.trTime` (`0x5580f`, `0x55821`), re-evaluates both (`0x55838`,
`0x55851`) and links it (`0x5585d`); then calls `ent+0x208` if set
(`0x55881`). `InitScriptMover` (`0x60214`) writes `Reached_ScriptMover` to
`ent+0x204` (`0x60364`) and nothing to `ent+0x208`. INFERRED: a blocked
script mover holds one frame per blocked frame, no callback, and its
`movedone` comes that much later.

VERIFIED, off the capture:

| Case | Observed |
|---|---|
| Riding | Rider follows the slab to the hundredth: z -21.875 to 26.125 and back on `movez`, x -215 to -167 and back on `movex`; `groundEntityNum` 177 throughout |
| Rotation | `rotateyaw(2, 1)` about the world origin carried the rider from `(-215, 2463)` to `(-300.83, 2453.99)`; `delta_angles[1]` rose 18 a frame (0.1 degrees, `ANGLE2SHORT` truncated) |
| Pushing | Player south of the slab at `(-215, 2430)` first moved on the 28550 snapshot (slab trajectory 6 to 7.2), then 1.2 a frame; the slab moving back left him there |
| Blocked | Slab lowered from z 80 onto a standing player stopped at z 72 for the four seconds he stood there, wire `trTime` +50 a frame, player unmoved; once he was teleported it went on down |
| Verb on a moving entity | `moveto((0, 0, 0), 1)` at 42250 during the stalled descent replaced it: new `trBase` z 30 is the old trajectory at the call's level time, `trDelta` z -32 is what `getorigin()` read a frame earlier. Section 14 and "A blocked mover" tell the two apart |
| Residual yaw | After `rotateyaw(2, 1)` and `rotateyaw(-2, 1)`, `apos.trBase` reads `-0.0` and a pushed player drifts +x 0.021 a frame: the push turns about the world origin by about 0.0005 degrees a frame |

vcod: `mover::run_one` unlinks a moving mover, lists `G_MoverPush`'s box with
`AreaTree::entities_in_box` into `Step::listed` and relinks it.
`crate::push::push` runs on the mover's turn in the entity pass (combat doc
14.7), after the frame's script: it keeps listed players and items in list
order, unlinks them all, pushes and relinks each in turn (the jitter and
keep-in-place fallback for a player, linked unsnapped at the spot), links
them all again, and on a player that fits nowhere walks its records back,
relinks those, leaves the rest unlinked and stalls the mover. A player listed
after the one being pushed is out of the tree and blocks nothing, as retail.

- A player moves from `ps.origin`. VERIFIED, vcod measurement against the
  ride fixture: pushing from the truncated link origin lost retail's fraction
  (`push_y` frame 8, y 2427.6 against 2428.8).
- The candidate test is retail's box and whole trace ("The candidate trace").
  VERIFIED, vcod measurement: a capsule candidate test met the plank 4.8
  units of slab travel later than retail's box.
- The position test is the capsule plus a sweep from where the body stood
  that passes through the pusher (a zero-length capsule is never `startsolid`
  under terrain; without the sweep the lowered slab pushed the player through
  the ground), plus `CollisionWorld::box_position_test` (retail's box against
  every brush, the capsule against terrain and patches, section 20).
- The residual yaw is modelled (below); the `TR_SINE` crush too (section 21).

Gates: `crates/server/tests/ride_ab.rs` (players), `push_ab.rs` (items),
`push_order_ab.rs` (tree order).

### The residual yaw

VERIFIED, `Reached_ScriptMover` (game.mp `0x5f98c`), angular half
(`0x5fa57`..`0x5fb13`): the settle (`0x5f7b0`) writes the stored target
(`ent+0x28c`) into `apos.trBase` with `trType` 0; `BG_EvaluateTrajectory`
writes it into `r.currentAngles` (`+0x140`); `trap_LinkEntity` links; then
pitch and roll go through `AngleNormalize180` (`0x3eb70`) and yaw through
`AngleNormalize360` (`0x3eb30`), stored back with no link after. Both take
angle times 182.044 (rodata `0x72ba4`), truncated (`fldcw` `0xc00`),
`& 0xffff`, times 360/65536 (`0x72ba8`). INFERRED: after a rotate
`r.currentAngles` is the target truncated to 65536ths of a turn;
`apos.trBase` keeps the target.

VERIFIED, `SV_LinkEntity` (`cod_lnxded` `0x080908b0`, game syscall 50):
before setting the link box, per component `a` of `r.currentAngles`, with `n`
the truncation of `a + 0.5` (rodata `0x80d664c`), when
`(n - a)^2 < 1.0000001e-6` (`0x80d6650`) it writes `n`. INFERRED: every link
snaps a component within a thousandth of an integer to it.

VERIFIED, `ScriptEnt_RotateAxis` (game.mp `0x60b5c`): target =
`r.currentAngles` plus the delta (`0x60cd4`..`0x60cfe`), the move set up from
`r.currentAngles` (`0x5fb38`), ending in `trap_LinkEntity` (`0x60d5e`).
`G_MoverTeam` (`0x55684`) takes `amove` as evaluated `apos` less
`r.currentAngles`; `G_MoverPush` adds it back (`0x55379`..`0x553a7`) before
the link.

VERIFIED, capture against retail 1.1d, mp_carentan,
`client-probes/probe_yawres.gsc`, no client, 2026-10-17:
`crates/server/tests/fixtures/movers/mp_carentan-dm-yawres.txt`, the brush
model's `.angles` times 1e9 each frame (`.angles` reads `r.currentAngles`,
script field 16, offset `0x140`):

| Verb | `.angles` yaw |
|---|---|
| `rotateyaw(2, 1)` from 0 | 0.1 a frame up to 1.9, then 1.99951171875 (`2 - 2^-11`, 364/65536) from the end frame |
| `rotateyaw(-2, 1)` | exactly 2 on its first two frames (the verb's link snapped 1.99951), 1.89951 down, exactly 1.0 at the half (0.99951 snapped), 0 at the end (target `-2^-11`, snapped) |
| `movey(-48, 1)` | 0 every frame |
| `rotateyaw(10, 1)` | ends 9.99756 (1820/65536) |
| `rotateyaw(-10, 1)` | ends 0 (target -0.00244 truncated) |
| `movey(48, 1)` | -0.00244 from its first frame: `amove` is target less 0, too far from 0 to snap |

INFERRED: whenever the slab moves after a rotate, `G_MoverTeam` turns it and
everything it pushes by the 65536th the quantization dropped. A target within
a thousandth of an integer (the -2 pair's `-2^-11`) is snapped back by the
link each frame and the same `amove` recurs every moving frame: the ride
capture's 0.021 a frame. Otherwise (the -10 pair's -0.00244) the first moving
frame takes it all: `linkto_ab`'s 0.1 units at radius 2470.

vcod: `mover::run_one` keeps `.angles` as `r.currentAngles`, takes the push's
`amove` off it, snaps it (`mover::snap_angles`) and quantizes it when a
rotate ends. The verbs start a stationary rotate from it and snap it in their
link; a link reads it, not the trajectory. Linear trajectories evaluate as the
x87 does, `delta * dt + base` rounded once (`mover::bg_evaluate`). VERIFIED,
vcod measurement: `ride_ab.rs`'s
`a_rotated_brush_model_keeps_retails_quantized_angles` matches every capture
row to the digit; the ride push and the linkto children match to 0.05 and
0.02 with no gap.

VERIFIED, the 1.5b and UO game modules: `Reached_ScriptMover` calls
`AngleNormalize180`, `AngleNormalize360`, `AngleNormalize180` as 1.1d's does.
VERIFIED, 1.5b's `BG_EvaluateTrajectory` stores a linear trajectory's `dt` to
a float (`0x1b8f5`) before the mul-add; vcod follows on a 1.5 or UO server
(`Movers::stored`). The 1.5b and UO engines snap as 1.1d's (next subsection).

### The snap on every link

VERIFIED, the snap in each engine's `SV_LinkEntity`:

| Engine | Snap | Angles at | Rounding | Constant |
|---|---|---|---|---|
| 1.1d `cod_lnxded` | `0x08090a0a`..`0x08090aad`, every call, after `s.solid` packing, before any contents or shape test | `ent+0x140` | `+ 0.5` truncated | `0x80d6650` |
| 1.5b | loop `0x8098d7f` called at `0x8098f72` from `SV_LinkEntity` (`0x8098df6`); stores the difference to a float (`0x8098dc2`) | `ent+0x144` | `0x809ac27` (`+ 0.5`, rodata `0x80e0b5c`, truncated under `0xc00`) | `1.0000001e-6` at `0x80e0b00` |
| UO 1.51b | loop `0x8099d77` called at `0x8099f6a` | `ent+0x148` | `0x809bc1f` | `0x80e6a40` |

INFERRED: `ent+0x144` and `ent+0x148` are those builds' `r.currentAngles`
(their contents tests read `ent+0x11c` and `ent+0x120`, 1.1d's `+0x118` moved
the same amount). The stored difference changes nothing: a float minus a
nearby integer is exact.

VERIFIED, `client-probes/probe_anglesnap.gsc` against retail 1.1d, 1.5b and
UO 1.51b, mp_carentan, no client, 2026-10-17, identical lines on all three:
`crates/server/tests/fixtures/movers/mp_carentan-dm-anglesnap.txt`. A script
model, a script origin, a suspended `item_health` and model `*5` read
`.angles` as written, (0.0004, 89.9996, 0.0011) and (-0.9995, 180.0008,
359.9991), until an `.origin` write links them; then (0, 90, 0.0011) and
(-0.9995, 180, 360), the same a frame later. -0.9995 stays: `n` truncates
-0.4995 to 0.

VERIFIED, 1.1d `ClientThink_real`'s tail writes `ent+0x140..0x148` after its
link (cod11-gametypes-re-bel.md 8.6). INFERRED: a playing client's `.angles`
reads the unsnapped write after every cmd.

vcod: `GameHost::link_entity_at` snaps the `angles` field of every non-client
entity it links or unlinks; `ride_ab.rs`'s
`every_link_snaps_angles_like_retail` matches every line of the probe. A
client's link does not snap.

### A blocked mover

VERIFIED, both earlier ride captures (2026-10-05 and -06, since replaced;
section 11): the slab lowered onto the standing player moved 2 units more on
retail than on ours once, for two different reasons. In the `-ents` run its
trajectory, stalled from 36700, logs no change at 36950 and shifts again from
37000: it moved 72 to 70 and stalled at 70. In the first run every `crush`
sample reads 72, and the resetting `moveto` starts from 30 where ours started
from 32.

VERIFIED, game.mp `ScriptEnt_MoveAxis` (`0x606c0`) and `ScriptEntCmd_MoveTo`
(`0x603a0`) hand `r.currentOrigin` (`ent+0x134`; `0x608a1`, `0x60554`) to the
move setup (`0x5fb38`, called at `0x608bf`, `0x60575`) and end in
`trap_LinkEntity` (`0x608cb`, `0x60581`). INFERRED, off the setup's control
flow: it takes the delta to the target off `r.currentOrigin`; then, when
`pos.trType` is not 0, `BG_EvaluateTrajectory` writes the running trajectory
at `level.time` into `r.currentOrigin`; `trBase` is copied from it; a last
evaluation of the new trajectory at `level.time` writes it back.

VERIFIED, the other verbs:

| Verb | Setup | Link |
|---|---|---|
| `ScriptEnt_RotateAxis` | `0x5fb38` on `r.currentAngles` (`0x60d52`) | `0x60d5e` |
| `ScriptEntCmd_RotateVelocity` | velocity twin `0x5feb0` on `ent+0x140` (`0x60f24`, `0x60f45`) | `0x60f51` |
| `ScriptEntCmd_RotateTo` | setup at `0x60b3c` | `0x60b4b` |
| `ScriptEntCmd_GravityMove` (`0x60590`) | copies `r.currentOrigin` into `trBase` (`0x60667`), no evaluation before | |

INFERRED: `0x5feb0` evaluates a running trajectory as `0x5fb38` does. After a
move verb on a moving entity `getorigin()` reads the new `trBase`, one frame
further on than before the call (a stalled trajectory runs a frame ahead of
where the mover holds), and the verb's link puts the clip there. That is the
first run's 2 units: its reset is called a second after the teleport, while
the freed slab still descends.

VERIFIED, `client-probes/probe_crush.gsc` against retail 1.1d, mp_carentan,
2026-10-17, client on a jittered link (`VCOD_NETSIM="ping=40,jitter=70"`),
since replaced: the slab held on the player for 8 s slipped 2 units five times
(72 to 62), each on the frame after a snapshot whose `commandTime` repeats the
one before, and on no other frame; those five frames ran with no cmd from the
player. The `moveto` called in the teleport frame, slab stalled at 62, read
`getorigin()` 60 straight after. INFERRED, off the push listing above: the
blocked push leaves the player unlinked until his next `ClientThink`, so a
frame that runs before his next cmd arrives finds nobody under the slab. The
`-ents` run logged no `commandTime`; its 36950 pass is read as such a frame,
not measured.

VERIFIED, same probe and link, 2026-10-20, the `--probe-ride` client logging
its cmds and every parsed snapshot's `commandTime`
(`crates/server/tests/fixtures/movers/mp_carentan-dm-crush.txt`): nine slips
in 8 s (72 to 54); the `moveto` at the teleport reads 52. The client logs
every snapshot, including two landing in one loop pass: logging only the newer
lost 25 of 240 between lift and done, and a replay reads a lost snapshot's
frame as one with no cmd.

vcod: the move verbs write the start into `origin` and link
(`builtins::mover::linked`, `mover::verb_linked`); a blocked push leaves the
blocker unlinked. `ride_ab.rs`'s
`a_blocked_slab_moves_on_a_frame_without_a_cmd_like_retail` replays the crush
capture's cmds (`ProbeHold::framed`, cmd-less frames included) on a client
that joins so its lift starts on retail's frame, and holds every frame of
every phase, the fall after the teleport too, to two decimals. VERIFIED, vcod
measurement 2026-10-20: all 240 frames match. In the 2026-10-20 ride capture
(section 11) every `crush` frame had a cmd and the slab holds at 72 for all 75
stalled samples, as on ours; `ride_ab.rs`'s wire gate matches the slab's
trajectory with no tolerance on `trTime`.

### The box test

VERIFIED, `0x550f0`..`0x5530a`: a second box beside the list box, the
pusher's box where the move takes it, both taken before the pusher moves:

| Pusher | Moved box | Address (stored at `ebp-0xc`, `ebp-0x18`) |
|---|---|---|
| unrotated | `r.absmin + move`, `r.absmax + move` | `0x55260`..`0x5527c` |
| rotated | `r.currentOrigin + move` less and plus the `RadiusFromBounds` radius | `0x551d0`..`0x551f0` |

VERIFIED, `0x5540e`..`0x5548f`, after the ground test, before the trace: a
listed entity is dropped when on any axis its `r.absmin` (`+0x11c`) is not
below the moved box's max (`fcomp`, `and ah,0x45`, `dec ah`, `cmp ah,0x40`,
`jb` to the loop's `continue` at `0x55532`) or its `r.absmax` (`+0x128`) is
not above the moved box's min (`and ah,0x5`, `je` to the same place).
INFERRED: Q3's `G_MoverPush` test; boxes that only touch miss. INFERRED: both
are link boxes grown a unit a side (`SV_LinkEntity`, `Link::boxed` in
`crates/server/src/area.rs`), so the test never drops an entity the pusher's
brushes overlap; it only saves traces.

vcod: `mover::push_boxes` keeps the moved box on `Step::moved_box`;
`crate::push` runs the test off each listed entity's last link box between the
ground test and the trace, players and items alike.
`push::tests::the_box_test_drops_a_box_that_only_touches` pins the
comparisons; forcing the test to drop everything fails `push_order_ab`.

### The candidate trace

VERIFIED, game.mp: the trace at `0x554ee` passes the listed entity's number
and box at its `r.currentOrigin` as start and end, and keeps the entity only
when the trace's entity is the pusher (`0x55515`). It is a whole `trap_Trace`,
so `cod_lnxded` decides what it names:

- VERIFIED, `SV_Trace` (0x80916f4): the world clip runs first; a fraction 0
  result jumps to the copy-out (0x8091a64) and walks no entity. Its entity
  number is `0x3fe` (`ENTITYNUM_WORLD`) for any fraction below 1.
- VERIFIED, the walk (0x80599c8): a node whose contents word misses the mask,
  or whose start fraction is not below the trace's, returns at once. Otherwise
  it recurses into the side above (`+0x20`) or below (`+0x24`) the split when
  the box (half extents grown a unit, clip `+0x18`) lies wholly on it, into
  both, above first, when not, and only then runs the per-entity clip
  (0x8090f18) on each entry of the node's own list from its head
  (0x8059bfa..0x8059c16).
- VERIFIED, the per-entity clip (0x8090f18): skips an entity whose
  `r.contents` misses the mask, the pass entity, and anything the pass entity
  or its owner owns. A brush model clips through `CM_InlineModel` at its
  angles, anything else through `CM_TempBoxModel` over `r.mins`/`r.maxs`. The
  result replaces the trace only on a strictly smaller fraction (`fcomps`,
  `and ah,0x45`, `dec ah`, `cmp ah,0x40`, `jae`, 0x809100a..0x809101a) and
  takes the entity's `s.number` (`movzwl (%esi)` at 0x809103c into trace
  `+0x28`); a tie only ORs in `startsolid` and `allsolid`.
- VERIFIED, the pass tests (0x8090f52..0x8090f75), all skipped when the
  pass entity (moveclip `+0x6c`) is 0x3ff: the entity is the pass entity,
  or its `r.ownerNum` (`+0x14c`) is the pass entity or the moveclip's pass
  owner (`+0x70`). VERIFIED, `SV_Trace` (0x8091847..0x809185f): the
  pass owner is the pass entity's `r.ownerNum`, -1 when that is 0x3ff, so
  two unowned entities never pass each other. The pass entity's owner
  itself still clips.

vcod: `push::first_named` walks under `combat::Pass`'s rule with the
owners vcod links under the push masks, a seated client's vehicle and a
driven UO vehicle's driver (`push::Owners`); a missile's and a turret's
contents miss the masks. `push::tests::the_candidate_trace_passes_what_the_pass_owner_owns`
pins it.

INFERRED: a position test is named by the world when the box starts in it,
else by the first entity in walk order whose clip contains the box; later ones
tie at 0. A listed player sunk into the floor, or overlapped by a player the
walk meets before the pusher, is not kept, and the mover passes through him.

VERIFIED, capture against retail 1.1d, mp_carentan, `dm`-shaped probe, a
spectator then two `allies` clients, 2026-10-20:
`crates/server/tests/fixtures/movers/mp_carentan-dm-pushcand.txt` (`PROBE`
lines from `client-probes/probe_pushcand.gsc`, then both allied clients'
`--probe-ride` `CMDS` lines, each cmd's send stamped with the wall clock,
`w=`). The spectator keeps `StuckInClient` off (`cod11-player-clip.md` 6.2),
so two players set down on one spot stay stacked at z -30. Each phase is a
one-frame `movey(-8, 0.05)` of `*5` into players at y 2430:

| Phase | Result |
|---|---|
| Alone | Player moved 8 on the slab's moving frame |
| Sunk | Player `setOrigin`ed to z -33 (1.1 into the street) stays at (-215, 2430, -33.00) while the slab moves 8 through him |
| Stacked; half (second player 15 to the side) | Slab holds at 0 for all four logged frames and the half second after, its move never ending; neither player moves in x or y. Both sink: stacked one cmd's fall (0.13) at a time, slot 2 at 20200, slot 1 at 20250, to -30.78 and -30.72 half a second on; half pair slot 2 drops 0.46 at 23400 and is on the street (-31.81) by the end. An earlier run with unlogged cmds sank them by other amounts (-30.10 to -30.46) |

INFERRED, off the walk and the capture: in the last two the slab, relinked at
its node's head ahead of the filter (0x553ae), is met before the partner, so
at least one player is kept; he fits nowhere with the other still linked, and
the slab stalls.

VERIFIED, same capture: a stall on a move's last frame holds the move.
`G_MoverTeam`'s `reached` call (0x558a0..0x558e9) follows the push and a
blocked push returns before it, so the end is retried every frame.

vcod: `push::names_pusher` is the trace:
`CollisionWorld::world_box_position_test` (model 0's brushes against the box,
its terrain and patches by the capsule position tests), then
`AreaTree::trace_order` (retail's walk), with the pusher, a submodel, a world
copy or a vehicle's collmap clipped through `pusher_box_trace` and anything
else as its link box less the unit. A box touching another's face counts as
inside (INFERRED, Q3's `CM_TestBoxInBrush`). It reads `ps.origin`, which
`ClientThink_real` writes back to `r.currentOrigin` past the touch pass
(0x405c7). `mover::run_one` keeps the plans as they stood before a frame
retired a segment; `mover::stall` restores them and takes back the notifies
they owed.

INFERRED, off the listing (section 12, "A blocked mover") and the capture: the
sinking is the blocked push's unlink. The player it could not place stays out
of the tree until his next `ClientThink`, so a partner's cmd read first falls
free of him for one `Pmove`, and the next cmd of either is stuck inside the
other again. Which one sinks, and how often, depends on the order the two
clients' packets reach the server.

vcod: the packet pass clips each `Pmove` against the clients linked in the
area tree (`Server::replay_moves`), so a player a blocked push left unlinked
is out of every other client's move until his own cmd relinks him.
`crates/server/tests/push_candidate_ab.rs` replays the capture: both clients
join so their first snapshots fall on retail's, send the captured cmds on
retail's side of each frame (`ProbeHold::framed`), one packet a cmd, ordered
by their `w=` stamps. Every row matches: level times, slab, all three
coordinates of every player, the street's `setOrigin` rests (section 21) and
the stalled pairs' sinking. Clipping against every playing client instead
keeps both pairs at -30 and fails eight rows.

VERIFIED, the same recipe on retail 1.5b and UO 1.51b, 2026-10-20:
`mp_carentan-dm-pushcand-1.5.txt` and `-uo.txt` (PROBE lines, both clients'
`CMDS`, and the `RIDE` lines of the snapshots a `StuckInClient` push left on
them). Alone and sunk match 1.1 (moved 8; left at z -33.00). Stacked and half
do not: the spectator does not keep `StuckInClient` off there
(`cod11-player-clip.md` 6.2), so on the first end frame after the
`setOrigin`s the pair is shoved apart, both velocities 190 long, and comes to
rest 95 to 107 units apart before the slab moves.

| Pair | Shove | Rest |
|---|---|---|
| 1.5 stacked | slot 1 (-170.53, -83.78) at 17750, again at 17800 | (-274.58, 2397.78), (-190.04, 2449.83) |
| UO stacked | slot 1 (179.16, 63.25) at 18100 and 18150 | |
| 1.5 half | one push, mostly along the 15 units between them: slot 1 (-189.88, -6.76) at 20950 | |

In both phases the slab still holds at 0 through the four logged frames and
the half second after, and neither player moves.

VERIFIED, the 1.5b and UO game modules keep 1.1's shape:

| Function | 1.5b | UO |
|---|---|---|
| `G_MoverPush` | 0x560d7 | 0x60c1b |
| candidate `G_TestEntityPosition` (must return the pusher) | 0x56590 | 0x610d4 |
| `G_TryPushingEntity` call per kept entity | 0x566ab | 0x611ef |
| `G_TryPushingEntity` | 0x55aa1 | 0x605e5 |
| its carried-spot test | 0x55bff | 0x60743 |
| its jitter (z 0 then -4, 4; x and y -4, 4 while under `maxs.x * 0.5`) | 0x55eb7 | 0x609fb |
| its own-spot test | 0x5609a | 0x60bde |
| `G_TestEntityPosition` | 0x55834 | 0x60378 |
| its `trap_Trace` over `r.mins`/`r.maxs` | 0x558cb, 0x5590e | 0x6040f, 0x60452 |

A player it cannot place returns the blocker unless the pusher is `TR_SINE`.
The tests use `trap_Trace`, not `trap_TraceCapsule`, which both modules also
import.

INFERRED, off ours replaying both captures and retail's logged rest spots: the
partner does not block the slab. One of each pair rests under `*5`'s
clip_metal brush 4277 (lump 4), whose underside slopes to z 7.5 over the
street. His capsule, slid there by the shove's `Pmove`, clears it by 0.1 to
1.9; his box overlaps it by 2.9 to 3.5. The box candidate trace names the
slab and keeps him, and every spot `G_TryPushingEntity` tries is inside 4277,
plate 4281 under it, neighbour 4279 or the street's terrain (the z -4 row).
The push fails and the slab stalls every frame, as 1.1's stacked pair does.
Rest spots: 1.5b stacked slot 2 (-190.04, 2449.83, -31.88), half slot 2
(-182.08, 2430.91); UO slot 1 (-181.90, 2437.40, -32.00), slot 2 (-182.08,
2431.90).

vcod: ours stalls on the same frames for the same reason.
`push::tests::on_15_and_uo_a_body_whose_box_is_under_the_slab_stalls_it`
checks each fact above off both captures' rest spots: one of each pair has his
box in the slab and his capsule clear, `try_push` with no other body linked
finds him no spot, and the lone player is carried 8. `push_candidate_ab`
replays both captures and draws each push's jitter from glibc `rand()` under
the seed recovered off the `RIDE` lines (`Server::pin_stuck_rand`, player-clip
doc 13.4). Every row matches. Before `StuckInClient` took 1.5's scan the pairs
stayed stacked on ours and 48 rows failed.

### `TR_SINE` and the crush arm

VERIFIED, an `objdump` scan of game.mp's `.text` for immediate stores
`mov DWORD PTR [reg+0xc],0x4` or `[reg+0x30],0x4`: two, `SP_func_pendulum`
(`apos.trType`, `0x57110`) and `SP_func_bobbing` (`pos.trType`, `0x584c4`).
VERIFIED, a scan of every `.bsp` and `.gsc` in the 1.1 install's `pak*.pk3`,
single-player maps included: none contains `func_bobbing` or `func_pendulum`.
VERIFIED, script reaches both: `spawn(classname, origin)` (`0x5d268`) calls
`G_CallSpawnEntity` (`0x5d2f1`), which walks the `spawns` table (`0x61861`)
holding both (`docs/research/cod11-gsc-object-model.md` 8).

VERIFIED, two retail runs of `client-probes/probe_sine.gsc` (mp_carentan, no
client, 2026-10-10): `spawn("func_bobbing", (0, 0, 100))` returns a live
entity with `.model` `""` whose `origin` swings `32 sin(2 pi t / 4000)` about
z 0 (31.90 at 1100, -31.90 at 3100), the spawn origin unused; the
`func_pendulum` beside it swings its roll (18.85 at 1100). Spawned at 1050 in
one run and 2050 in the other, both times the server went down on the frame
after the 4000 line with `ERROR: Reached_BinaryMover: bad moverState`.

VERIFIED: `InitMover` stores `Reached_BinaryMover` at `ent+0x204`
(`0x565da`); `G_MoverTeam` calls `ent+0x204` per team part when `pos.trType`
is not 0 and `level.time >= pos.trTime + pos.trDuration`
(`0x558a0`..`0x558c2`), likewise for `apos` (`0x558c7`..`0x558e9`). INFERRED:
a bobbing's `pos.trTime` is its phase times its period, 0 by default, so
`Reached_BinaryMover` sees a mover state `SP_func_bobbing` never set and drops
the map once the level clock passes one period; one spawned later drops it on
its first entity pass.

VERIFIED, `cod_lnxded` syscall 0x21 (`trap_SetBrushModel`, game.mp
`0x63724`), case `0x8088360` off the switch table at `0x80d4eac`: reads the
inline model number at `ent+0x8c`, writes `r.mins`/`r.maxs` from its bounds,
`r.bmodel` 1 (`+0xfc`), `r.contents` -1 (`+0x118`), then links. INFERRED: a
script-spawned bobbing has no `*N` and carries inline model 0, a moving copy
of the world's brushes until the drop; whether it pushes or crushes in that
window is not measured here.

INFERRED: the crush arm (`0x555f0`..`0x55617`) is reachable only through an
entity that drops the server within one period of its `trTime`, from a
classname no stock map or script uses. Section 18 has the spawn functions,
wire and drop; section 21 measures the crush inside that window.

## 13. A player linked to a mover

VERIFIED, off the capture: a player linked to a `script_origin` 32 units east
of him follows the parent's `movez(48, 2)` and `movex(48, 2)` exactly, and on
the parent's `rotateyaw(90, 2)` swings round it to `(-480, 2656)`, the offset
turned 90 degrees, `delta_angles[1]` unchanged. VERIFIED: the snapshot after
the call carries the parent's first frame of motion (z -22.675 at 44300 for a
move called at 44250), a frame ahead of the player's own script `.origin`.

INFERRED, off section 11's parent-first loop and `G_SetFixedLink`'s mode-2 arm
(`docs/research/cod11-gsc-object-model.md` 23.2): the re-anchor reads the
parent where it is this frame and turns the offset, held in the parent's
frame, by the parent's axis. The capture's parent had zero angles at the link,
so it cannot tell a parent-frame offset from a world one.

vcod: the re-anchor runs on the client's turn in the entity pass, after its
parent's (`ScriptRuntime::run_entity_pass`, `PassStop::Client`), reads
`ScriptRuntime::link_anchor` (the mover's plan at the level time) and relinks
the client, as `G_RunClient` (0x40660) does through `G_SetFixedLink`
(0x406d9) and `trap_LinkEntity` (0x40713). `linkTo` stores the offset in the
parent's frame.

## 14. The brush model on the wire, and the client's half

The wire evidence was a third capture from the same probe with five phases
added (`hide`, `show`, `notsolid`, `solid`, `delete` on the slab), 2026-10-06.
The 2026-10-20 ride capture (section 11) runs the same probe and replaces it:
its wire file holds `RIDE_ENT` (the slab's `solid`, `index`, `eFlags` on
change), `RIDE_GONE` and trajectory lines; the server file the phase starts.

VERIFIED, off that capture: both bombzone brush models arrive on the first
snapshot as `eType` 8, `solid` `0xffffff`, `index` 5 and 6 (their inline model
numbers), `eFlags` 0. Model configstring 5 on that load is
`xmodel/barrel_black1`, so a `0xffffff` entity's `index` names an inline
model, not a configstring slot. VERIFIED: `SV_LinkEntity` (cod_lnxded
`0x80908b0`) stores `0xffffff` into `s.solid` when `r.bmodel` is set
(`0x80908da`), before reading `r.contents`.

VERIFIED, off the capture:

| Call | Wire |
|---|---|
| `hide()` at 53450 | `eFlags` `0x100` on the 53450 snapshot |
| `show()` at 54450 | `0x100` off on the 54450 snapshot |
| `notsolid()`, `solid()` | nothing; entity in every snapshot, `solid` `0xffffff` |
| `delete()` at 57450 | entity gone from the 57450 snapshot |

VERIFIED, game.mp: the per-entity runner `0x602bc`, for an entity with no
client (`ent+0x158` zero), sets byte `ent+9` bit 1 (`s.eFlags` `0x100`) when
byte `ent+0x17d` bit `0x10` (the `flags` `0x1000` `ScrCmd_Hide` writes) is set,
and clears it otherwise. INFERRED: a `notSolid()`ed brush model is still sent
as one, and the entity leaves the snapshot on the `delete()` frame, a tenth of
a second before the free.

VERIFIED, off the trajectory lines: a move reads stationary on the snapshot of
the frame its last segment ends (`ride_up`, called at 10450 for 2 s, reads
`trType` 0 `trTime` 12450 at 12450), a frame before script's `movedone`
(section 8). Every blocked frame, `trTime` of `pos` and of the stationary
`apos` both advance 50 (section 12).

VERIFIED: the `moveto((0, 0, 0), 1)` at 41450 on the descending slab sent
`trBase` z 28 and `trDelta` z -30 where script read z 30 at the call; the move
ended at 42450 on `trBase` `(0, 0, 0)`. INFERRED: `trBase` is the old
trajectory at the call's level time, the velocity is taken from
`r.currentOrigin` (a frame behind), and the stationary end is the destination,
not where the segment ran out (z -2). The first run's moveto (section 12) is
the same rule with the slab 2 units higher.

vcod: `crate::game::wire` sends a `script_brushmodel` as `eType` 8, `solid`
`0xffffff`, `index` its `*N`, `eFlags` `0x100` while hidden, and drops it on
`delete()`. `Movers::wire` hands the plans over advanced to the level time.
`crate::world` culls a brush model by its inline model's bounds (a cube of
their radius once its angles are non-zero, `SV_LinkEntity`'s `r.bmodel` arm).
`ride_ab.rs` diffs the slab's entity per snapshot against the capture.

VERIFIED, a fourth capture, 2026-10-06:
`crates/server/tests/fixtures/movers/mp_carentan-dm-cull.txt`, from
`client-probes/probe_cull.gsc` and a `--probe-ride` client on the attackers'
spawn. `movez(20000, 8)` at 12350 took entity 177 out of the 13300 snapshot,
the first frame its trajectory stood 2375 up (2250 the frame before);
`movez(-20000, 8)` at 21350 brought it back on the 28450 snapshot, the first
frame down to 2250. A cull at `trBase` (the segment's start) would have kept
it the whole lift and dropped it the whole descent. INFERRED, off that and
`G_MoverPush` relinking the pusher at its moved pose every moving frame
(section 12, `0x553ae`): the snapshot cull reads the clusters of the box the
mover was last linked with, at its trajectory at the level time.

vcod evaluates an `eType` 8 entity's `pos` and `apos` at the frame's level
time for the cull (`crate::world::entity_visible`). VERIFIED, vcod
measurement, same probe and client against ours: the slab left the snapshot
950 ms into the lift and came back 7100 ms into the descent, as on retail.

### The client

VERIFIED, `cgame_mp_x86.dll`: case 8 of the entity-type switch at
`0x3001d5f0` is `0x3001b710`. It returns without drawing when `eFlags` carries
`0x100` (`test ah,0x1` at `0x3001b732`); when `solid` is `0xffffff`
(`0x3001b76d`, `0x3001b7de`) it adds a type 0 ref entity whose model is the
table at `0x301d31c0` indexed by `es.index`, at the interpolated origin
(`cent+0x1f8`..`+0x200`); any other `eType` 8 is a type 1 ref entity from the
xmodel table at `0x301d24fc`. INFERRED: a brush model draws only through its
snapshot entity, at its interpolated pose; a hidden one draws nothing.

VERIFIED, a census of the 16 BSPs in the mounted paks: 30
`script_brushmodel`s, none with an `origin` key, so all brushes and surfaces
are in world space. The only submodels with draw surfaces belong to
`script_brushmodel`s (18); carentan's bombzones `*5` and `*6` carry none.
INFERRED: the ride capture's slab is clip only, drawn by nothing on retail
either.

INFERRED, off the ref entity at `0x3001b76d` naming the inline model: the
renderer draws a moved brush model's own BSP surfaces, lightmap indices and
all, so it keeps the lighting baked where it was compiled. Not read in
`CoDMP.exe`.

vcod: `entities::resolve_visual` takes a `0xffffff` entity's `index` as its
inline model and skips one with `eFlags` `0x100`. A brush model at the zero
pose draws with the world; one that has moved draws its own soups through the
world's pipelines and lightmaps under a `model` matrix of its interpolated
pose; one with no entity in the snapshot draws nowhere
(`Renderer::set_submodels`). With no server, every one draws at spawn.

VERIFIED, a census of mp_powcamp, mp_chateau, mp_depot and mp_harbor (lump 27
against the cells' lists): no submodel soup sits in any cell's AABB tree
(lump 16) or cull group (lump 9), so the cell walk never marks one. INFERRED:
retail draws them only through the ref entity, culled on its own bounds after
the snapshot's PVS cull chose the entity. vcod: the walk and its frustum
fallback mark every soup of a brush model whose soup bounds meet the frustum
(`WorldVis::mark_brush_models`); the renderer keeps those whose model an
entity carries. Without that, an at-rest brush model drew only with culling
off: mp_powcamp's bombzone sheds (`*2`, `*8`: front wall, roof, crates) and
mp_chateau's fence gate (`*6`, `*7`) were invisible yet solid (GitHub issue
6).

VERIFIED, `cgame_mp_x86.dll`: `CG_ClipMoveToEntities` (`0x30028df0`) takes a
`solid` `0xffffff` entity down an arm that calls syscall `0x21` with
`s.modelindex` and `BG_EvaluateTrajectory` (`0x30005470`) twice before the
trace syscall. INFERRED: Q3's `trap_CM_InlineModel` with `pos` and `apos` at
the prediction time, so prediction clips a moving brush model where its
trajectory has it.

VERIFIED: the function at `0x3001baa0` reads an entity number between 1 and
`0x3fd`, tests `eType` against 5 and 8, calls `BG_EvaluateTrajectory` four
times and writes `in + (a - b)` and an angle delta; `CG_PredictPlayerState`
(`0x300294f0`) calls it at two sites. INFERRED: Q3's
`CG_AdjustPositionForMover`, carrying the predicted origin with the ground
entity's motion from the snapshot time to the render time.

VERIFIED, cgame: `CG_BuildSolidList` (`0x30028d50`) walks the snapshot's
entities and skips one whose next state has `solid` `0xffffff` (`0x30028d93`)
and `eFlags` bit `0x2` (`test byte [eax+0xf8],0x2` at `0x30028d9b`, a byte at
`centity+0xf8`). `SP_trigger_multiple`, `SP_trigger_damage` and
`SP_trigger_once` (game.mp `0x74c50`, `0x75278`, `0x75c0c`) are the writers
of that bit found so far.

VERIFIED: the brush model arm of `CG_ClipMoveToEntities` evaluates `apos` and
`pos` (`0x30028e67`, `0x30028e78`) at the time in `0x30207150`, which
`CG_PredictPlayerState` loads from the snapshot's `serverTime` (`snap+8`).
VERIFIED: the carry after the cmd loop (`0x30029a2e`) passes the dword at
`0x302071b0` as the entity, that time, the dword at `0x30207148` as the target
time and the predicted origin at `0x30207170` as in and out; the angle delta
it writes to `[esp+0x34]` is not read again. The second call site is
`0x3002972d`, inside the cmd loop. INFERRED: the entity is the predicted
`groundEntityNum`, the target time `cg.time`. Retail clips a brush model where
it stood at the snapshot, not per cmd, and carries the predicted origin by the
ground mover's translation only, so a rider of a turning mover is predicted
standing still between snapshots. The second site is Q3's miss test, carrying
the new replay's origin at the old prediction's `commandTime` before
comparing, so a ride is no correction.

VERIFIED, cgame 1.1 `0x3002970d..0x30029757` (1.5 `0x3002aa7d..0x3002aaba`,
same code): the second site runs when the predicted `commandTime`
(`0x3020715c`) equals last frame's (`[esp+0x70]`). It passes `0x30207150` as
the from time, `0x3020714c` as the to time, an angle out at `[esp+0x34]`; then
loads the out's yaw (`[esp+0x38]`), multiplies by 182.04445 (float at
`0x30069380`), truncates through `_ftol` (`0x3005a890`), masks `0xffff` and
adds it to `0x302071a8`, the predicted `delta_angles[1]` (playerstate base
`0x3020715c`, `delta_angles` at `+0x48`, netfield offset 76 for `[1]`). The
add comes before the `cg_showmiss` test and the error arithmetic. INFERRED:
`0x3020714c` is `cg.oldTime`, so this is RTCW's "add the deltaAngles (fixes
jittery view while riding trains)": the cmds past last frame's `commandTime`
run, and the view is drawn, with the yaw turned by what the ground mover
turned from the snapshot to last frame's time. Only the yaw turns; the origin
is never turned about the mover.

How the turn meets the cmds, cgame 1.1 `CG_PredictPlayerState`
(`0x300294f0`; RTCW-MP `cg_predict.c` makes the same add unconditionally):

1. VERIFIED: last frame's prediction is copied aside, the playerstate rebuilt
   from the snapshot's (`delta_angles` included) and `0x30207150` set to the
   snapshot's `serverTime`.
2. VERIFIED: the loop takes cmds `current - 63` to `current` and skips one
   unless its `serverTime` is past the playerstate's `commandTime` and no
   later than the newest cmd's (`0x300296c6..0x300296f7`).
3. VERIFIED (`0x30029703`, the operands): before a cmd runs, when the replayed
   `commandTime` equals the copy's, the add runs with the replayed
   `groundEntityNum` (`0x302071b0`): the ground after the cmds up to last
   frame's `commandTime`.
4. INFERRED, off the loop's branches: cmds up to last frame's `commandTime`
   run with the snapshot's yaw, every later cmd (this frame's) with the
   snapshot's yaw plus the turn to `cg.oldTime`; the drawn view is the last
   cmd's. The turn is measured from the snapshot every frame and never piles
   up (the rebuild resets `delta_angles`); it fires at most once a frame; with
   no cmd past last frame's `commandTime`, or a replay that never lands on it
   (a newer snapshot past it), nothing is added.
5. VERIFIED: the carry after the loop (`0x30029a2e`) takes the mover's
   translation to `cg.time`; its angle out is never read, so the turn from
   `cg.oldTime` to `cg.time` never reaches the view.

VERIFIED, asm: United Offensive's cgame (`uo_cgame_mp_x86.dll`) has the same
code at `0x30035b0a..0x30035b67`: `commandTime` test against the copy
(`0x304831c4` against `[esp+0xd8]`), adjust `0x3001f5c0` with the ground at
`0x3048321c`, from `0x304831b8` to `0x304831b4`, yaw times 182.04445
(`0x3007bd60`), `_ftol2` (`0x3006be3c`), `& 0xffff`, added to
`delta_angles[1]` at `0x30483214`. Only the layout moved: ground entity and
`delta_angles` sit 4 bytes past 1.1's. VERIFIED: `0x3001f5c0` zeroes the
angle out and moves only entities 1 to `0x3fd` of `eType` 5 or 8, as 1.1's
`0x3001baa0`.

vcod (`play::predict::Predictor::predict`, `Replay::run`) runs the same add
inside the replay, before the first cmd past last frame's `commandTime`,
reached exactly, with the ground at that point and last frame's render clock
as `cg.oldTime`. vcod keeps its replay across frames and runs only new cmds,
which on a turning ground would keep each cmd's own frame's turn; so a frame
with a new cmd whose cached state holds or would take a turn replays from the
snapshot, as retail does every frame. A frame with no new cmd keeps last
frame's state and turn; retail builds a cmd every frame and never meets one.
INFERRED, off the `eFlags` test and section 14's wire: a `notSolid()`ed brush
model still in the snapshot is clipped by retail's prediction, as are the four
exploder brush models `_load.gsc` hides and `notSolid()`s (mp_depot `*1`,
mp_powcamp `*3` and `*9`, mp_rocket `*3`; cod11-mantle.md).

VERIFIED, cgame `0x3001d210` (`CG_CalcEntityLerpPositions` by shape): reads
`currentState.pos.trType` (`cent+0xc`, `0x3001d216`) and calls the snapshot
lerp `0x3001d090` when it is 1 (`TR_INTERPOLATE`, `0x3001d219`), or 3
(`TR_LINEAR_STOP`) with entity number below `0x40` (`0x3001d22d`,
`0x3001d232`). Otherwise it calls `BG_EvaluateTrajectory` twice at `cg.time`
(`0x30207148`, `0x3001d246`, `0x3001d25b`) into `cent+0x1f8` and `cent+0x204`,
and, unless `cent` is `0x3020922c`, calls `0x3001baa0` at `0x3001d2ee` with
`currentState.groundEntityNum` (`cent+0x7c`), `cg.snap->serverTime`
(`[0x301e2160]+8`), `cg.time`, a null angle out (`ecx` 0 from `0x3001d263`,
pushed at `0x3001d2dd`) and `cent+0x1f8` as in and out. VERIFIED:
`0x3001baa0` copies in to out unchanged unless the number is above 0 and below
`0x3fe` (`0x3001bab5`, `0x3001babd`) and that entity's `eType` is 5 or 8
(`0x3001bad6`, `0x3001badf`). INFERRED: `0x3020922c` is
`cg.predictedPlayerEntity`. Any unlerped entity is drawn off the older
snapshot's trajectories at the drawn time and carried by its ground mover's
translation since that snapshot: an item resting on a moving brush model
(section 12: `trType` 0, ground the mover) rides it smoothly; one with no
mover under it holds its older `trBase` until the next snapshot.

VERIFIED, game.mp `G_GeneralLink` (`0x68530`), called by `G_RunMover` for a
linked entity (section 11): `G_SetFixedLink(ent, 0)`, `G_SetOrigin` and
`G_SetAngle` at the re-anchored `r.currentOrigin`/`r.currentAngles`, 1 to
`pos.trType` and `apos.trType` (`0x68568`, `0x6856f`), `trap_LinkEntity`.
INFERRED: a script model linked to a mover goes out `TR_INTERPOLATE` at its
re-anchored pose every frame and the client lerps it; the ground carry never
reaches it.

vcod: `entities::lerp_pos_angles` makes the same choice on the older
snapshot's state and carries through `SnapshotMovers::carry`. Section 15
measures the link itself.

vcod: `vcod_common::pmove::movers::SnapshotMovers` is that solid list and
carry. The client unlinks every submodel at map load; each prediction links
and poses the snapshot's brush models at its `serverTime`, naming each by its
entity so a ground trace reads it. The replay's newest origin and the drawn
origin are carried by the ground mover, and the correction compares both
sides carried to the old `commandTime`. `predict_ab.rs`'s
`predictor_rides_retail_movers` predicts the ride capture's rider from each
snapshot to the next against retail's next playerstate: exact to 0.05 through
every carried phase, 4.3 units out through the two yaw phases, as retail's own
prediction is.

## 15. `linkTo` on script entities

Evidence: one paired capture against retail 1.1d, mp_carentan, `dm`-shaped
probe, 2026-10-08.

| File | Half |
|---|---|
| `crates/server/tests/fixtures/movers/mp_carentan-dm-linkto.txt` | server: one `PROBE` line per frame of a phase, from `crates/gsc/tests/fixtures/semantics/client-probes/probe_linkto.gsc` |
| `mp_carentan-dm-linkto-wire.txt` | client: every snapshot's state of the linked script_model, entity 171 |

The probe keeps model `*5` (entity 177, origin and angles zero) as
`probe_ride` does, links a script_origin `a` (170, yaw 30) and a script_model
`b` (171) to it and moves it, runs two three-entity chains, deletes a moving
parent, links four script_origins to the player, and ends on a link cycle.

### Which receivers

VERIFIED: `InitScriptMover` (`0x60214`) writes `eType` 8 (`0x60378`) and sets
the `ent+0x17d` bit 0x20 `linkTo` tests (`0x6038d`); object-model doc 23.2
lists the other writers (`G_SpawnItem`, `G_SpawnTurret`, `ClientSpawn`,
`enableLinkTo`). INFERRED, off the mover verbs' gate (section 1) naming the
same three classes: every `script_model`, `script_origin` and
`script_brushmodel` takes a link, and so do items and turrets.

### The record

VERIFIED, the helper both link builtins call (`0x662e0`, unnamed, between
`G_EntAttach` and `G_CalcTagAxis`):

| Step | Address |
|---|---|
| `G_EntUnlink` on the child | `0x662f3` |
| `trap_DObjExists` on the parent, `trap_DObjGetBoneIndex` | `0x66307`, `0x6631b` |
| -1 as the bone | `0x66330` |
| parent against child | `0x66335` |
| each record's first dword along the parent's chain | `0x66344`..`0x66353` |
| allocate `0x70` bytes | `0x6635c` |
| store parent `+0x0`, parent's old first child `+0x4`, lowercased tag `+0x8` (`SL_GetLowercaseString` `0x66376`), bone `+0xc`; zero `+0x10` and `+0x40`, 0x30 bytes each; `parent+0x2e8 = child`, `child+0x2e4 = record` | `0x66386`..`0x663bf` |

INFERRED, off the branches around those sites and to `0x66380` (returns 0):
the DObj calls run for a non-empty tag and -1 is an empty tag's bone. A
missing DObj or bone, the parent being the child, or a parent chain that
reaches the child refuses the link, after the unlink at the top has run.

VERIFIED, `G_EntLinkTo` (`0x68034`): calls the helper (`0x6804a`) and
`G_CalcTagAxis(child, 0)` (`0x6805c`). VERIFIED, `G_CalcTagAxis` (`0x663d4`):
reads the parent's `AnglesToAxis(r.currentAngles)` and `r.currentOrigin`,
calls `G_DObjCalcBone` and `DObjSkelMatrixMultiply43`
(`0x6643c`..`0x66463`), the child's own `AnglesToAxis` (`0x664b4`),
`MatrixInverseOrthogonal43` (`0x66502`) and `MatrixMultiply43` into
`record+0x10` (`0x6652e`). INFERRED, off the bone test at `0x663f8` and the
mode test at `0x664c9`: the bone matrix enters only for a non-negative bone,
and mode 0 leaves the record holding the child's pose in the parent's (or
tag's) frame, so the link moves nothing on its frame. VERIFIED, off the
capture's `PROBE linked` and `PROBE linked_pl` lines: every child reads its
pre-call pose.

VERIFIED, `G_EntLinkToWithOffset` (`0x68074`): calls the helper, writes
`AnglesToAxis` of the fourth argument into `record+0x10` and the third into
`record+0x34`. VERIFIED, off the capture: `k4 linkto(player, "", (16, 0, 8),
(0, 90, 0))` reads the player's origin plus `(16, 0, 8)` and angles
`(0, 90, 0)`.

VERIFIED, the builtin's failure arm (`0x59df5`..`0x59ea8`): calls
`trap_DObjExists` on the parent (`0x59e01`), reads the model byte `ent+0x175`
(`0x59e0d`), `trap_DObjGetBoneIndex` (`0x59e5d`), and carries:

| Error | String at |
|---|---|
| `"failed to link entity since parent has no model"` | `0x76bc0` |
| `"failed to link entity since parent model '%s' is invalid"` | `0x76c00` |
| `"failed to link entity since tag '%s' does not exist in parent model '%s'"` | `0x76c40` |
| `"failed to link entity due to link cycle"` | `0x76ca0` |

INFERRED, off its branches: a parent with no DObj takes the first error when
its model byte is 0, the second otherwise; a missing bone the third; only a
parent with a DObj and a good tag reaches the cycle's. VERIFIED, off the
capture: `y linkto(x)` with `x` already linked to `y`, both script_origins, is
the fatal `failed to link entity since parent has no model`.

### The per-frame re-anchor

VERIFIED, the per-entity runner `0x502bc` (section 11): compares `eType` with 3
(`0x50380`), tests the link record (`0x50385`), calls `G_GeneralLink`
(`0x50392`) and the think pointer (`0x503e1`) on that path. `G_RunMover` tests
the record (`0x57616`), calls `G_GeneralLink` (`0x57623`) and jumps to
`G_RunThink` (`0x57628`, `0x5767e`). INFERRED: a linked item runs the link and
its think instead of `G_RunItem`, and a linked mover never reaches
`G_MoverTeam`. VERIFIED, `G_GeneralLink` (`0x68530`): `G_SetFixedLink(ent, 0)`
(`0x68540`), `G_SetOrigin`, `G_SetAngle`, 1 into both `trType`s (`0x68568`,
`0x6856f`), `trap_LinkEntity`. VERIFIED, `G_SetFixedLink` (`0x66540`): builds
the parent's frame from the same reads as `G_CalcTagAxis`; its mode-0 arm
(`0x66630`) calls `MatrixMultiply43` on `record+0x10` and `AxisToAngles` into
the child's `r.currentAngles`.

VERIFIED, off the capture: `a` and `b` move on the same script frame as the
brush model's own `getorigin()`: at 12600 the model reads z 1.20, `a` 41.20,
`b` -20.80. INFERRED, off section 11's clocks: the pass runs after the threads
and reads the parent at the level time; script reads the result a frame later,
as it reads the parent's.

VERIFIED, off the wire half: entity 171 goes out `trType` 1, `trTime` 0,
`trDuration` 0 on both groups every snapshot it is linked; its `trBase` z at
`serverTime` 12550 is -20.8, what script reads at 12600.

VERIFIED, off the capture: a turning parent swings the child round its own
origin and adds its yaw to the child's: at 15600 the model reads yaw 0.50, `a`
30.50, `b` 0.50. `b.angles` reads `(0, 360.00, 0)` once the model's yaw is
back to `-0.00`: `AxisToAngles` wraps a yaw a hair below 0 into `[0, 360)`.

VERIFIED, off the `verb_linked` phase: `a movez(100, 1)` on the linked `a`
moves nothing, and no `movedone` comes in the next 2 s or after the unlink.
INFERRED, off `G_RunMover` skipping `G_MoverTeam` and `G_SetOrigin` rewriting
the trajectory each link frame: a verb on a linked entity is lost.

VERIFIED, off the `unlink` phase: `a unlink()` at 25500 leaves `a` at z 65.20,
what script read that frame, while `b` rides on. VERIFIED, `G_EntUnlink`
(object-model doc 23.2): `G_SetOrigin` and `G_SetAngle` at the entity's own
current pose.

### Order: a parent runs first, a grandparent does not

VERIFIED, `G_RunFrame` (`0x50912`..`0x50975`, section 11): the loop counts
entity numbers up, runs the first dword of the link record (`0x50949`) then
the entity (`0x50955`), and never reads the parent's own record. INFERRED, off
that and the runner's level-time stamp (`0x502cb`): a parent runs ahead of its
child and never twice; nothing runs the parent's parent first.

VERIFIED, off the capture: the forward chain `c0` 172 (moved), `c1` 173, `c2`
174 reads in step, all z 102.40 at 30600. The reversed chain `r2` 175, `r1`
176, `r0` 179 (moved) does not: at 33800 `r0` reads 102.40, `r1` and `r2`
100.00, and through the yaw `r1` reads `r0`'s previous frame plus 45 (35350:
9.00 and 49.50). INFERRED: at 175 the loop runs 176 first, which reads 179
before 179 has run; 175 then reads 176 current.

### A deleted parent

VERIFIED, `G_FreeEntity` (`0x66948`): `0x66954`..`0x669ee` is `G_EntUnlink`'s
body inlined on the entity, and `0x669f1`..`0x66a9e` the same body looped over
the list at `ent+0x2e8`, `G_SetOrigin` and `G_SetAngle` at each child's own
pose. INFERRED, off the loop's exit test at `0x66a9c`: every child is unlinked
where it stands before the slot is cleared.

VERIFIED, off the `del_parent` phase: the parent deleted at 37700 mid-move
carries its child two more frames (124.00 at 37750, 125.20 at 37800); the
child then holds 125.20. INFERRED: `delete()` frees a tenth of a second later
off the think (object-model doc 14), and the mover runs its trajectory until
then.

### A player parent

VERIFIED, `ClientThink_real` (`0x405bb`..`0x405f6`, just past its
`G_TouchTriggers` call): copies `ps.origin` into `r.currentOrigin` and writes 0
to all three `r.currentAngles`. VERIFIED, off the capture: the player's
`.angles` read `(0, 0, 0)` every frame but the one
`setPlayerAngles((0, 180, 0))` ran on, and `k1 linkto(player)` with no tag
keeps its `(32, 0, 40)` offset and angles 0, swinging to `(-32, 0, 40)` and yaw
180 for that one frame (39850). INFERRED: a link to a player's entity ignores
where he looks.

VERIFIED, off the capture: `k2 linkto(player, "bip01 head")` and
`k3 linkto(player, "tag_weapon_right", (0,0,0), (0,0,0))` follow the posed
bones: `k3` sits on the right hand, `(6.3, -5.2, 48.1)` from the feet, and both
drift a few degrees a frame with the idle. From the `setPlayerAngles` frame
`k3`'s yaw reads 179.47, 269.54, 44.62, 21.99, then settles near 0. INFERRED:
the bone carries the legs' yaw off the view (`tag_origin`'s local tag, combat
doc 16.4), and the legs swing after the view and back. VERIFIED, `linkto_ab`
with that swing modelled: ours reads 182.16, 272.0, 46.6 and 23.75 on the same
frames; the gate holds yaw to 4 degrees.

vcod: `crate::game::link` holds the records; `link::run_linked` is the link
arm of the entity pass (`ScriptRuntime::run_entity_pass`), ascending by
number, parent first, a mover parent read at the level time once it has run.
It writes the child's `origin` and `angles`, relinks it, poses a brush model's
clip and forgets any mover plan; the wire sends a linked entity
`TR_INTERPOLATE`. A player's bone is the hit rig's pose (`link::client_bone`),
a script model's its bind pose. The bone is held in model space and placed by
the player entity's `angles` once.

Trap: turning the bone by the body's yaw and then by `angles` again passes
every probe whose player faces world yaw 0. UO's `vehicle_ab`
`the_jeep_driver_s_bones_follow_retail_s` (a driver facing 165) catches it:
every bone lands the view yaw round the feet from retail's (VERIFIED,
replay).

`crates/server/tests/linkto_ab.rs` replays the probe. Every non-player row
matches to 0.02 (section 12's yaw residual included), the player's
entity-frame links to 0.02, and the bone-linked children follow the body's
swing after `setPlayerAngles` within 0.01 units and 0.01 degrees of retail's
every frame, link frame included. Before the bone cache below (2026-10-12)
the residual was 0.08 units, the head 0.75 degrees and the hand's first frame
0.37. `enableLinkTo`, turrets, a tag parent's model change and an item
unlinked in mid-air are section 16.

VERIFIED, vcod measurement 2026-10-12, `linkto_ab` with the bone pose's anim
clock shifted by a test-only offset, ours' client joined on the server's first
frame; worst tag error over every player phase (head and hand, still, turn and
move):

| Offset (ms) | -400 | -300 | -250 | -225 | -200 | -100 | 0 | +100 |
|---|---|---|---|---|---|---|---|---|
| Error | 1.02 | 0.30 | 0.08 | 0.23 | 0.38 | 0.94 | 1.44 | 1.81 |

**Where the 250 ms came from: the join, not the clock.** VERIFIED, vcod
measurement 2026-10-12: ours' legs channel started `pb_stand_alert` (index
122) at level time 800, the spawn, and the probe's watcher (sleeps 1 s, then
polls every 50 ms for a playing client) first saw the player at 1050, so the
schedule ran 250 ms behind the idle. VERIFIED, the fixture: retail's `linked`
line reads 12500, so its watcher saw the player at 8500; the client joined
seconds into the run (header recipe). VERIFIED, vcod measurement: with ours'
server run alone for 1000, 2000, 2050 or 7500 ms before the client connects,
the idle starts 4000 ms before `linked` every time and the worst tag error is
0.08 at each. INFERRED: on retail the idle starts on the frame the watcher
first sees the player playing, as on ours, and the server's anim clock for a
standing body is ours. `common::PROBE_JOIN_MS` holds ours' client back
1500 ms; `linkto_ab` and `itemdrop_ab` (items doc 14.1) use it.

**Retail's server anim clock.** VERIFIED, `game.mp.i386.so`: `G_RunFrame`
(0x50478) writes `level.time - level.previousTime` to `level+0x1f0`
(0x504a2) and, before anything else, calls `trap_DObjInitServerTime` (syscall
0x6d) for every entity with byte `ent+0x160` set, then
`G_DObjUpdateServerTime` (0x670fc) for each without bit 0x40 at `ent+0x17d`,
re-running `Scr_RunCurrentThreads` while it returns non-zero
(0x506c5..0x506f7). `G_DObjUpdateServerTime` passes `level+0x1f0` times 0.001
(0x79bf4) to `trap_DObjUpdateServerTime` (syscall 0x6c). VERIFIED,
`cod_lnxded`: syscall 0x6c reaches 0x080bf008, then 0x080bbab0, which adds
`dtime * rate (node +0x40) * frequency (parts +0x8)` to the node's time and,
for a looping clip, wraps it by 1. The loader 0x080b7160 stores
`numframes = header frames + looping - 1` at `parts +0x0` and
`frequency = framerate / numframes` at `+0x8`, so a looping clip's period is
header frames over frame rate, as `XAnim::duration` reads it (188 / 30 s for
`pb_stand_alert`). VERIFIED, `game.mp.i386.so`: the per-channel anim setter
0x2a600, called by 0x2aba0 from `BG_PlayerAnimation` (0x2c1f4) in
`ClientEndFrame` when the wire index changes, calls
`trap_XAnimSetCompleteGoalWeight` (syscall 0x5f) with restart 1 (0x2aa13);
`cod_lnxded` 0x080bdf64 hands it to 0x080bdca4, which zeroes the node's time
when restart is set. INFERRED: an anim's time is 0 at the end frame that set
it and grows by each later frame's `frametime` at that frame's start, ours'
`now - start_ms`.

### A moving body's clip

**The rate.** VERIFIED, `game.mp.i386.so`, the per-channel updater 0x2aba0
(`BG_PlayerAnimation` calls it for the legs lerp frame `ci+0x37c` with
`es->legsAnim` at 0x2c30f and the torso's `ci+0x3ac` at 0x2c327):

- It calls the setter 0x2a600 when the wire value, toggle included, differs
  from `lf+0x10` (0x2abfd..0x2ac22).
- For an anim with a `moveSpeed` (`animation_t +0x44`) and non-zero
  `lf+0x2c`, rate = distance `es->pos.trBase` moved from `lf+0x1c`
  (`VectorDistance` 0x2ac73, or vertical only under the anim's flag 2,
  0x2ac5e) over `(bg.time - lf+0x2c) * 0.001` (0x6ef24) over `moveSpeed`,
  into `lf+0x28` (0x2ac9d); then `bg.time` and `trBase` are stored back
  (0x2aca0..0x2acb7).
- Clamps: under 0.1 (0x6ef28) becomes 0.1, or 0 when under 0.01 (0x6ef2c)
  with flag 2. Over 2.0 (0x6ef30) caps at 4.0 (0x6ef3c) with flag 2; else 2.0
  for `moveSpeed` over 150, 3.0 (0x6ef34) under 20, `3 - (moveSpeed - 20) /
  130` (0x6ef38) between.
- No `moveSpeed`, or `lf+0x2c` 0: rate 1.0, time and position stored
  (0x2ada5).
- `trap_XAnimSetAnimRate` (0x2adef) gets the rate for `lf+0x10`'s anim only.

VERIFIED: `G_RunFrame` writes `level.time` into `bg+0x0` and `bg+0x4` and the
frametime into `bg+0x8` (0x504a8..0x504b2). VERIFIED: `ClientEndFrame`
rewrites the entity from the playerstate with snap 1 (0x41179, 0x4118a) before
`BG_PlayerAnimation` (0x41486), so `trBase` is the truncated origin (combat
doc 14.3). INFERRED: the rate is the snapped ground speed since the last end
frame over the clip's own speed, and the phase a clip advances is the distance
covered over the clip's root travel.

**`moveSpeed`.** VERIFIED, `BG_FinalizePlayerAnims` (0x27d70): stores
`trap_XAnimGetLength` at `+0x48` (0x27f34), takes the length of
`trap_XAnimGetRelDelta`'s translation from time 0 to 1.0 (0x27f41..0x27f73),
truncates it (control word `| 0xc00`, 0x27f82) and stores `travel * 1000 /
length` at `+0x44` (0x27f9b..0x27fa5), 0 for no travel or no length; a length
under 500 ms is then raised to 500 (0x27fb1), after the division. VERIFIED,
`cod_lnxded`: syscall 0x5d's case (0x8088b32) calls 0x80bed5c, which divides
`numframes` by the frame rate and multiplies by 1000.0 (0x80d4ea8) under a
truncating control word. VERIFIED, `BG_ParseCommands` sets bit
`1 << movetype` of `animation_t +0x54` per state anim (0x287f1) and bit 2 of
`+0x50` for a `climbup` or `climbdown` (movetypes 16, 17) anim with a
`moveSpeed` (0x287f5..0x28811). vcod: `vcod_common::animtree::AnimMotion`;
`pb_combatrun_forward_loop` reads 232, `pb_combatrun_back_loop` 128, the
ladder clips 72 and vertical.

**The phase.** VERIFIED, the setter 0x2a600: for a new looping anim with a
`moveSpeed` (`trap_XAnimIsLooped`, 0x2a858..0x2a881), the start time is the
old anim's `trap_XAnimGetTime` when the old one had a `moveSpeed` and loops
(0x2a890..0x2a8bc), else `((bg.time / 100 + clientNum * 360) % 1000) * 0.001`
(0x2a8c4..0x2a919, `clientNum` at `ci+0x8`). `trap_XAnimSetTime` (0x2aa5a)
applies it after the goal weight call whenever the new anim has a
`moveSpeed`. 1.5b's and UO's setters set it only on a node whose weight reads
0 (cod11-combat.md 3.1, "The setter on a weighted node").

**The phase on 1.5 and UO.** VERIFIED, disassembly of 1.5b's `game.mp` setter
(file address 0x18ced; Ghidra's `FUN_00028ced`, image 0x10000 higher) and
UO's (0x1ce5f), the same instructions:

- The spread arm (1.5b 0x18fa2..0x19036, UO 0x1d114..0x1d1a8) takes the
  period from the OLD anim (`ci+0x10` before the store, toggle off): its
  `trap_XAnimGetLength` + 200 when `trap_XAnimIsPrimitive` says it is a clip,
  else 1000.
- Phase = `clientNum * 0.36f + (bg.time % period) / period`, the remainder an
  `idiv`, quotient and product summed on the x87 stack and stored as a float,
  then that float less its `fistp` under 0xc00 (truncated), stored again.
  0.36f at 1.5b 0x73cc4, UO 0x995e4.
- `XAnimGetLength` (syscall 0x61; 1.5b `cod_lnxded` 0x808f584, UO 0x8090396):
  frames over the float framerate on the x87 stack (1.5b 0x80c9b5e), times
  1000.0f (0x80df1f8, UO 0x80e50f8), stored truncated, as 1.1d.

VERIFIED, 1.5's cgame (`cgame_mp15.dll` 0x30003a82..0x30003af6): the same
period and sum, but MSVC keeps the sum unstored and truncates through `_ftol`
(0x3005cf7c), so the client's phase is the unrounded sum's fraction rounded
once; vcod's client takes the server's (under 1e-6 apart). vcod:
`animclock::spread_phase_15`, used by the windowed setter
(`ChannelClock::update_windowed`), with `AnimMotion::length_ms`.

VERIFIED, live (retail 1.5b and UO 1.51b, 2026-10-20, `probe_legs` with a
`--save-legs` client, fixtures `mp_carentan-dm-legs-{1.5,uo}.txt` and their
`-legs-client-` halves; recipe in each header): the run out of the settle's
idle and every later movement clip pose like retail's to 0.01 units and 0.08
degrees once the run's blend out of the idle ends (`legs_ab`'s `..._on_15`
and `..._on_uo`). Under 1.1's spread the same rows read 20 units off, 243
failing.

VERIFIED, same captures: the placement's one-unit drop stops at z -151.64139
(vz -33) on 1.5 and reads -151.35341 (vz -28) for one UO frame, because the
probe sent a cmd at the placement's level time, 25000, that the client half
does not list (its `!cmd` lines start a cmd later). 1.5 falls 18, 16 and 16 ms
from commandTime 24982; UO 2, 16 and 16 from 24998. The fall is pmove's: under
1.5's stored tail (cod15-movement.md 7) the 18 ms step from rest takes the
move as its velocity (-7), then -20 and -33, ending 0.234 over the floor,
where the next ground trace (0.25 down) stands it; UO's 2 ms step does not
take the move and ends -2, -15, -28. Both match to every printed digit.
INFERRED: the cmd at 25000 is the probe's, which ends each frame's cmds on the
frame's time (25050, 25100, ... in both logs). `legs_ab` replays retail's
commandTime on the placement frame and that cmd before the log's first; the
1.5 walk is then exact throughout, and the UO walk once the spawn idle is
dated as "The spawn idle on 1.5 and UO" has it. VERIFIED, same captures: the
crouch walk back that returns at 42000 while its node still blends out keeps
its time ("The setter on a weighted node", cod11-combat.md 3.1), to 0.03
units.

**The cmd at the placement.** VERIFIED (`crates/client/src/probe.rs`'s loop):
the net probe builds, sends and logs a frame's cmd before reading the snapshot
that shows the placement, so cmds sent past that snapshot's commandTime
predate the placement its log is timed from. Retail runs them through the
placement's fall: the legs captures' 25000 above, the shellshock capture's
15000 (cod11-shellshock.md, "Measured"). `--save-legs` and `--save-shock` keep
the last cmds sent before the placement and log those past the placement
snapshot's commandTime at `ms=0`, ahead of the first `!snap`'s successors.
VERIFIED (a 1.1d `--save-legs` run on 2026-10-21, not committed): the
placement snapshot (t 25000) read ct 24986 and the next line is
`!cmd ms=0 st=25000`.

Older captures lack it:

| Gate | Handling |
|---|---|
| `legs_ab`, `shellshock_ab` | add a holding cmd at the placement's level time when the log's first cmd is later |
| `moves_ab`, `predict_ab` | rebuild each pair from its first snapshot; skip a pair whose last cmd is missing |
| `ladder_ab` | start at the first snapshot past the log's first cmd |
| slope gates | captures log every cmd from the first spawned snapshot; gates rebase on snapshots |
| `bump_ab` | walker log starts 51 ms past the placement snapshot's commandTime (26282 to 26333): first pair runs one holding step where retail ran three, on a body at rest on the floor. INFERRED (the gate passes with no gaps): that step moves it nowhere |

**The blend.** VERIFIED, the setter: `lf+0x18` starts at the new anim's
`+0x40` (0x2a6ab; `BG_FinalizePlayerAnims` writes -1 there for an unset 0,
0x27f1d), or 200 for no anim (0x2a6c7). Legs (`lf == ci+0x37c`, 0x2a62a)
with no old anim: 0 (0x2a6db). Otherwise, unless already positive:

| Case | Blend ms | Address |
|---|---|---|
| legs, the two anims' `+0x54` differ in `& 0xc4` (`idlecr`, `walkcr`, `walkcrbk`) or `& 0x308` (`idleprone`, `walkprone`, `walkpronebk`); new anim has `moveSpeed` or a turn bit (`+0x55 & 0xc0`) | 200 | 0x2a7f8..0x2a81f |
| same, otherwise | 250 | |
| any other, new anim has `moveSpeed` | 120 | 0x2a827 |
| any other, old anim had `moveSpeed` | 250 | 0x2a835..0x2a849 |
| any other, neither | 170 | |

Never below what it held (0x2a84e). The old anim then gets
`trap_XAnimClearGoalWeight(lf+0x18 * 0.001)` (0x2a945), the new one
`trap_XAnimSetCompleteGoalWeight(weight 1.0, lf+0x18 * 0.001, rate 1.0,
notify, restart 1)` (0x2aa3e).

VERIFIED, `cod_lnxded`'s sampler 0x080b8370: per bone each clip with a track
adds its key rotation's four components (identity, weight on `w`, with no
keys) times its weight into one sum, its translation times its weight into
another (nothing with no keys), and its weight into a third. INFERRED: the
bone takes the normalized rotation sum and the translation sum over the weight
sum, so a clip with no translation keys for a bone pulls that bone's offset to
0 by its weight; vcod's `PoseBuffer::blend` does that (xanim-v14-format.md,
"Rotation keys as the engine rebuilds them").

**The weight ramp.** VERIFIED, `cod_lnxded`: the game's
`trap_XAnimClearGoalWeight` is syscall 0x53 (`game.mp.i386.so` 0x63e5b) and
`trap_XAnimSetCompleteGoalWeight` 0x5f (0x64038); the dispatcher sends them to
0x080bf53c and 0x080bdf64. A node half is 0x1c bytes from node `+0xc`, the
server's the second (`0x083179e0` selects it): time, old time, cycle, old
cycle, then `+0x18` goal time, `+0x1c` goal weight, `+0x20` weight, `+0x24`
rate (0x080bd674's stores).

| Function | Behaviour |
|---|---|
| goal weight setter 0x080bd674 | stores the goal weight and `|goal - weight| * time` as goal time (the `ABS` product); snaps the weight to the goal when that is under 0.001 (0x080db490); raises a zero weight to `0.001 * goal` |
| 0x080bdf64 | calls it for the node, then each ancestor whose goal weight is 0 (goal 1), then restarts the node (0x080bdca4); touches no sibling |
| clear 0x080bf53c | zeroes all three at once when the node's or an ancestor's weight reads 0 (0x080bd0ec); else goal weight 0, goal time the whole `time` unscaled (or the lesser of the two when the goal was already 0) |
| per-frame step 0x080bb130 (from syscall 0x6d, `trap_DObjInitServerTime`, frametime in 0x083179f0) | moves the weight by `(goal - weight) * dt / goalTime` and takes `dt` off the goal time while `dt + 0.001` (0x080db218) fits, else lands on the goal |

INFERRED: each weight runs in a straight line from where the setter found it;
a clip coming in from weight `w` takes `(1 - w)` of the blend time, the clip it
replaces fades over the whole blend time from its weight, and clips already
fading keep their own ramps. VERIFIED, vcod measurement, `legs_ab`: runbk,
idle, runbk on three end frames in a row read 4.79 units and 14.5 degrees off
retail's with every ramp over the whole blend time and every fading clip
re-ramped at each change, and 0.23 and 0.54 with the rule above.

INFERRED, off the setter's raise: a node set from 0 runs from `0.001 * goal`
over the whole distance's time. vcod: `animclock::Ramp::raised`. VERIFIED,
vcod measurement, 2026-10-21: the MG42 mount and the first frame of each aim
and fire group moved from 0.008-0.039 off retail's gunner grid to within
0.005, and the knockback kills of `cod11-turrets.md` 13.6 onto or within 2e-4
of retail's points (turrets doc 13.9, "The weights across frames").

VERIFIED, retail capture 2026-10-12:
`crates/server/tests/fixtures/movers/mp_carentan-dm-legs.txt` (server half,
`client-probes/probe_legs.gsc`: script_origins linked at zero offset to
`bip01 l foot`, `bip01 r foot`, `tag_weapon_right` and `bip01 head`, logged
every frame) and `-legs-client.txt` (the `--save-legs` walk: run, stop,
backpedal, strafes, diagonals, crouch walk, every cmd and snapshot). A legs
change at an end frame shows in the bone log two rows later: the row at `t`
logs the pose the entity pass of `t - 50` took.

`crates/server/tests/legs_ab.rs` replays the cmds on ours at the same level
times. VERIFIED, vcod measurement:

| Model | Bones against retail |
|---|---|
| rate, phase, weights and sum above, keys rebuilt as the engine does | running, strafing, backing, crouch-walking within 0.03 units, 0.13 degrees; through a legs change's blend 0.03 and 0.21 |
| each clip slerped over the last at its share of the weight | move-to-idle blends up to 0.73 units off on the hand (`pb_combatrun_forward_loop` keys the clavicles' translations, `pb_stand_alert` does not); a stance change 2.1 degrees |
| no blend | a change up to 24 units off over its first frames |
| also rate held at 1, every clip from time 0 | strafe 34 units off, run 14, steady state |

Nothing stays open; the spawn idle's phase is the join time ("Where the
250 ms came from"). A crouched stop picks `idlecr` at 84 u/s with no move key
held, the 120 u/s rule of player-model-anim-system.md, "The stop".

**Torso over legs.** VERIFIED, `game.mp.i386.so`: `BG_FindAnims` (0x2c0e0)
finds `root`, `torso`, `legs` and `turning` in the `multiplayer` tree
(`animtrees/multiplayer.atr`: `main { torso { pt_* } legs { pb_* ... } }` and
`turning`, under an implicit `root`); `BG_FindAnimTrees` (0x2c14c) copies the
torso and legs nodes to `bgs` 0x9b6c4 and 0x9b6c8. VERIFIED, the setter
0x2a600, torso lerp frame only (the legs one jumps past at 0x2aa62's test),
each call with the torso's blend `lf+0x18 * 0.001`:

| Torso anim | Torso node goal | Legs node goal |
|---|---|---|
| set | 1.0 (0x2aaa5) | 0.01 (float at 0x6eee4, loaded 0x2aad1; call 0x2ab5b) |
| none | 0 (0x2ab26) | 1.0 (0x2ab5b) |

VERIFIED, `cod_lnxded`: `XAnimCalc` (0x080bcef8) walks the tree once from the
root with weight 1 (0x080bbea8, arguments fresh 1, top 0) into one per-bone
buffer of rotation, weight and offset, skipping bones in 0x083179d0 (not asked
for, or already calculated). The walk (0x080bbea8) counts a node's children
with non-zero weight. With one it walks that child at the node's own weight,
ignoring the child's. With two or more it walks each at its own weight into a
buffer (the parent's when the node is fresh, else a scratch one the first
child starts), then per bone normalizes the rotation sum to length `weight`
and scales the offset sum to `weight` over the weight sum, adding into the
parent's buffer when it used a scratch one; the root divides the whole by the
weight sum. A leaf samples through 0x080b8370 (above).

INFERRED: the torso and legs nodes blend per bone as two units, each its own
normalized blend at its node's weight, and a bone only one of them keys takes
that one's pose whatever its weight. With a torso anim, bones a `pt_*` clip
keys (spine up, arms, `tag_weapon_*`) take the torso clip with the legs clip at
0.01 over 1.01; the rest (pelvis, legs) the legs clip alone. Between two torso
anims the torso node stays at 1 and the two clips blend inside it, so no legs
pose shows through. vcod: `PoseBuffer::blend_nodes`, the node weights in
`ChannelClock::node_weights`, `pose_player` for the server's hit and tag poses
and the client's drawn bodies.

VERIFIED, live (retail 1.1d, 2026-10-17, `client-probes/probe_torso`,
`movers/mp_carentan-dm-torso.txt` beside `-legs-client-torso.txt`): fourteen
tags from the pelvis through both arms to `tag_weapon_right` and the feet,
logged every frame while the body fires standing, running, strafing, crouched
and aimed, and reloads backing, strafing and crouched. Ours replays the cmds
(`legs_ab::a_body_firing_and_reloading_on_the_move_is_posed_like_retail`):
every tag within 0.04 units and 0.13 degrees of retail's on all 559 frames,
blends included. Beyond the anim selection (player-model-anim-system.md, "Walk
against run" and "The weapon event's movetype") that took two things:

- Signed sums. VERIFIED, `cod_lnxded` 0x080b7160 (the xanim loader): a
  per-track bit in the bitset ahead of the z-only one negates the track's
  first rotation key (the `uVar26 & 1` arms after the key rebuilds, returning
  to 0x080b7a96 and 0x080b7dc5); then each key is put on its predecessor's
  hemisphere (xanim-v14-format.md). The sampler and node walk add those raw.
  Where a torso clip's hand and the legs clip's sit on opposite hemispheres,
  the reload's blend-out swings the hand the long way: retail's `bip01 l hand`
  turned 178 degrees off the short way's at 32000. Trap: aligning every sum to
  its first contributor, as vcod once did, takes the short way. vcod:
  `xanim::parse`, `skeleton::clip_sums`, `PoseBuffer::blend_nodes`.
- The idle's phase: the capture's placement is message 352; ours' idle set at
  7600 fits it (below).

**The spawn idle's phase.** INFERRED, off the setter: the idle
(`pb_stand_alert`, 6.267 s, no `moveSpeed`) takes no phase, restarts at 0 on
the end frame that sets it, and nothing resets it while the wire value holds
(634 from the capture's first snapshot to the run), so its phase at the
placement is the time since the spawn. VERIFIED, vcod measurement, `legs_ab`,
sweeping ours' idle start: bones from 25050 to the run read 24.6 degrees off
at worst with ours' own spawn (level time 800), 0.11 at worst with the idle at
5750 (mod 6267 ms, the one candidate on a frame); 5700 and 5800 read over 2.8
off. VERIFIED, the client capture's first row: the placement snapshot is
netchan message 389 at 25000, a message a frame since the gamestate, which
puts the join near 5600 and the spawn a few frames after. INFERRED: retail's
spawn set the idle at 5750; nothing on the server picks the idle's phase.
`legs_ab` starts ours' client so its spawn lands on 5750 and asserts it.
VERIFIED, the torso capture: placement message 352 and the same rule (idle 4
messages after the join, 50 ms a message) give 7600, which fits its settle to
0.08 degrees where 7500 and 7700 read 11 off.

**The spawn idle on 1.5 and UO.** VERIFIED, live (retail UO 1.51b,
2026-10-21, `probe_legs` with a `--save-legs` client that also logs the join's
snapshots and cmds as `!psnap` and `!pcmd`; recipe in each header):

| Fixture | Spawn point |
|---|---|
| `movers/mp_carentan-dm-legs{,-client}-uo-settle.txt` | deathmatch point (426, 2158) |
| `-uo-spawn720.txt`, `+set probe_legs_spawn 720_-32` (the probe deletes every other `mp_deathmatch_spawn`) | the point `-legs-uo.txt`'s probe took |

The first playing snapshot (`pm_type` 0) is netchan message 5 in both (level
time 9100 and 9550), already carries `legsAnim` 697 (`pb_stand_alert`) and
stands at rest: z 160.125, `placeSpawnpoint`'s height for that point in
`spawnpoints/placebits-retail.txt`; and -7.8749866 at (720, -32), where the
placed point reads -5.25356 and the spawn's think lands the body on the floor
under it. No snapshot shows a fall from the spawn point. INFERRED: the spawn's
end frame sets the idle and nothing restarts it until the run, as on 1.1.

VERIFIED, vcod measurement, `legs_ab`
(`a_body_settling_from_a_logged_spawn_is_posed_like_retail_on_uo`): pinned at
the placed height with the idle set on retail's spawn snapshot, both logged
walks match every row, settle included. Pinned at the lump's origin (z 176 at
(426, 2158), 0 at (720, -32)) the body falls and sets the idle up to 150 ms
late. Captures starting at the placement are dated from the placement's
message: message 5 gives 8500 for the 1.5b walk, 8800 for the 1.5b stop and
go, 10100 for the UO walk, each the one frame on which the walk matches
throughout (a frame either side misses 31 to 34 rows); the UO stop and go
matches only on message 6 (11750). INFERRED: the spawn's message depends on
when the client's team menu answer arrives. The UO walk's old residual (0.11
units, 2.9 degrees at 26300) was its idle fit at 3850, 6250 ms off retail's
10100: with a 6267 ms idle, that phase sat 17 ms off.

**`movementDir` folds like retail's.** VERIFIED, `PM_SetMovementDir`
(0x2e970): yaw from `vectoangles` (0x2eac4, 0x3de94: `atan2`, plus 360 under
0) and `AngleDelta` (0x2eaeb, 0x3ec9c: the difference through a 16-bit short
and back, over 180 less 360), truncated; a back key adds 180.0 (0x70898)
through `AngleNormalize180` (0x2eb2e), truncates again, caps at 90 (0x2eb56).
VERIFIED, the capture: a crouched body still sliding forward on the first back
cmd reads `movementDir` 90 (a float fold reads -90 and swings the legs 90
degrees the wrong way); a diagonal reads 211 (-45), not 212.

**Two events set their own anims.** VERIFIED, `BG_ParseCommands`
(0x28868..0x288bb): the current event index (0x91024, set at 0x2959e from
`BG_IndexForString` over the event names at 0x7b478, read through
`.rel.data`: 0 `PAIN`, 1 `DEATH`, 2 `FIREWEAPON`) decides:

| Event | Anim line gets | Address |
|---|---|---|
| 2 `FIREWEAPON` | `+0x50 \|= 0x8`, `+0x40 = 30` | 0x2888d |
| 1 `DEATH` | `+0x44 = 0`, `+0x50 \|= 0x40` | 0x288a9, 0x288bb |

VERIFIED: `BG_FinalizePlayerAnims` calls `BG_AnimParseAnimScript` after its
`moveSpeed` loop, so the zero stands. INFERRED, off the setter reading `+0x40`
at 0x2a6ab ahead of the "already positive" test: every change into a
`FIREWEAPON` anim blends over 30 ms, and a death anim always plays at rate 1.
VERIFIED, the setter (0x2a97b; cgame 0x3000395d): a new anim with
`+0x50 & 0x40` loads at full weight, blend 0
(`trap_XAnimSetCompleteGoalWeightKnobAll`, 0x2a9fe), and
`trap_XAnimSetTime(1.0)` (0x2aa5a), its last frame, unless the setter's fourth
argument is set. Both callers take it from `es->eFlags >> 11 & 1`
(0x2ac12..0x2ac18; cgame 0x30003c3b..0x30003c44), the `cloneplayer` bit
combat doc 5.2 measured clearing 250 ms after the body is made; with it the
anim blends in over `lf+0x18` from wherever its node stood. INFERRED: a body
seen within its first 250 ms falls; one seen later (a late joiner, a body
coming into view) is already down.

vcod: `vcod_common::animclock::ChannelClock` per channel, stepped in
`ClientSim::commit_pose` with the level time and slot; the server's hit rig and
tag pose read its layers.

### The drawn body's clip

Addresses are `cgame_mp_x86.dll` 1.1.

VERIFIED: `CG_Player` (0x30028210, `ET_PLAYER`) and the corpse draw
(0x30028400, `ET_CORPSE`) both pass `cent+0xf0` to the per-frame anim routine
0x30004e40, which swings the body angles (0x300040e0), then calls the channel
updater 0x30003be0 for the legs lerp frame `ci+0x37c` with `es+0xcc`
(`legsAnim`, 0x30004e86) and the torso's `ci+0x3ac` with `es+0xd0`
(0x30004e96), `EDI` the entity state both times. A player's `ci` is its
client's; a corpse's is `(es.number - 64) * 0x448 + 0x301d64dc`
(0x30028435..0x30028445), one per body queue slot. `CG_SetNextSnap`
(0x3002fc80) copies each new snapshot entity into `cent+0xf0` (the 0x3c-dword
copy at 0x30030103) and `CG_TransitionSnapshot` (0x3002faa0) copies it down to
`cent+0` (0x3002fb9a), so `cent+0xf0` is the newer snapshot's state, vcod's
`b`. The draw skips a player with `eFlags & 0x100` (`EF_NODRAW`) or `& 0x1`
(`EF_DEAD`, 0x30028228..0x3002824c); what a death leaves lying is the corpse.

VERIFIED, 0x30003be0 against `game.mp.i386.so` 0x2aba0: same tests, same
order, same constants (0x300693c0 0.001, 0x3006953c 0.1, 0x30069308 2.0,
0x3006944c 3.0, 0x30069754 1/130, 0x300693dc 4.0, 0x300693f4 0.01); the setter
0x300036b0 is a copy of 0x2a600 (the spread phase at 0x300038be reads
`bg.time`, 0x300f0310). VERIFIED: the measuring clock is `bg+0x4`
(0x300f0314), set by `CG_ProcessSnapshots` (0x300302c0) at 0x3003030c from
the second out-argument of `trap_GetCurrentSnapshotNumber` (syscall 0x50,
0x300302ce): the newest received snapshot's `serverTime`,
`cg.latestSnapshotTime`. `bg.time` (0x300f0310) is `cg.time`: the frame
routine 0x30033980 writes its `serverTime` argument into it and into `cg.time`
(0x30207148) at 0x300339c9. VERIFIED: the rate goes to the DObj with syscall
0x8e at 0x30003e2e on every call with a non-zero anim. VERIFIED: 0x3001bd00
advances the DObj of each newest-snapshot entity by `cg.frametime * 0.001`
(syscall 0x95) in a loop ahead of the draw loop (0x3001d5f0 per entity).

INFERRED: the drawn body's clips run on `cg.time`, each frame advancing at the
rate the frame before left. A legs or torso change starts its blend on the
first drawn frame that sees its snapshot, about a snapshot interval before the
drawn clock reaches that `serverTime`. The rate is measured once per snapshot,
off the distance `pos.trBase` moved between the two newest snapshots over
their time difference: the server's end-frame measure at snapshot cadence.

VERIFIED, 0x30003e50, called for both lerp frames before the updates
(0x30004e64, 0x30004e70): when the anim in `lf+0x10` reads weight 0 (syscall
0x93) it clears `lf+0x10`, `+0x14` and sets `+0x18` to 150, so the next update
restarts the clip. VERIFIED, the same test in:

| Module | Address | Weight read |
|---|---|---|
| 1.5 cgame | 0x300040b0 | syscall 0x94 |
| UO cgame | 0x300042c0 | syscall 0x96 |
| server `BG_PlayerAnimation` (`game.mp.i386.so` 0x2c1f4), legs first | `fucompp` against 0 at 0x2c267, stores 0x2c273..0x2c287 | `trap_XAnimGetWeight` at 0x2c25d, 0x2c2c8 |

INFERRED, off the goal weight setter raising a zero weight to `0.001 * goal`
(0x080bd674, "The weight ramp"): a node its own lerp frame's setter started
never reads 0, so only the other channel's setter takes it there. That is a
`both` line's node, shared by the two lerp frames and cleared by the channel
that leaves it (its clip restarts on the channel that kept it, from 0, or a
movement clip at the spread phase), or a node beside a shown death's path
(`KnobAll`, below). vcod: `animclock::update_body` steps a body's two lerp
frames as this routine and the server's do: the two clears, then the legs and
torso updates, each setter's clear, goal weight and `KnobAll` handed to the
other lerp frame (`ChannelClock::touched`). The server's end frame and the
client's draw both step through it.

VERIFIED, `cod_lnxded` 0x080bf8ac, whose arguments (DObj, anim, root, weight,
time, rate) match syscall 0x8a's as the setter passes them (cgame 0x300039e1
through 0x300317b0: the anim, `root`, 1.0, 0, 1.0): it clears the goal weight
of every sibling of the node (0x080bd35c, goal time the largest weight change
times `time`, under 0.001 snapped to 0), sets the node's own, then walks up to
the given root, clearing each ancestor's siblings and raising the ancestor to
1. VERIFIED: the setter's root is `0x3018bbfc`, looked up from the name at
0x30068f64, `root` (0x30005090). INFERRED: a death shown at its last frame on
the legs takes `torso` (a sibling of `legs` under `main`) and `turning` to 0
and `legs` to 1 at the next step, and every legs clip beside it with them; the
torso's own leaves keep their weights under a node at 0. vcod:
`ChannelClock::knob_nodes` sets the torso clock's two node weights at once and
`touched` drops the other lerp frame's clips beside the path. Trap: dropping
only the death channel's own fades lets a torso holding a fire anim fade over
the torso's own blend. No stock death shows it on the client (a corpse's torso
carries no anim, combat doc 8, so its torso node already stands at 0); the
server's dying player, whose torso can still be firing, does.

VERIFIED, vcod measurement, `crates/server/tests/legs_ab.rs`
(`a_drawn_body_runs_the_end_frame_clips_a_snapshot_early`): retail's snapshots
of the legs walk fed to the drawn clock, each arriving 50 ms ahead and drawn
every 10 ms, put both feet within 0.01 units of where the end-frame clock puts
them 50 ms later on every compared frame. Drawn every 7 ms, a change lands up
to 7 ms late and the feet read up to 1.9 units off in a steady clip, 2.9
through a strafe-to-strafe blend.

vcod: `crates/client/src/entities.rs` keeps a `ChannelClock` pair per entity
number and updates it each drawn frame with `Times { clock_ms: render_time,
snapshot_ms: newest snapshot }`, the newer snapshot's `pos.trBase` and
`eFlags`. INFERRED: the `ci` lerp frames sit in the cgame's static client and
body queue arrays and nothing on the draw path clears them, so they outlive any
unseen stretch; vcod keeps the pair for the cgame's life too. INFERRED, off
0x3001bd00 stepping only the newest snapshot's entities and the per-`ci` XAnim
tree (0x30005180 allocates one per client and body queue slot with syscall
0x83; the DObj create at 0x30004eb0 hands it the slot's tree): a body's clips
and weights stand still while its entity is out of the snapshot and go on from
there when it returns; the rate, on snapshot times, measures across the gap.
vcod: `EntityAnim::present_ms` notes the last frame the entity was in the
snapshot, and the first frame back shifts both clocks by the frames between
(`ChannelClock::pause`). 1.5's and UO's advance was not read.

### A bone asked for before the anim update

VERIFIED, `game.mp.i386.so`: `G_DObjCalcBone` (0x6737c) calls
`trap_DObjCreateSkelForBone` (syscall 0x6a) and returns at once when the bone
is ready; otherwise it calls `trap_DObjGetHierarchyBits` for the bone,
`trap_DObjCalcAnim` with those bits, the entity's controller callback
(`ent+0x220`) and `trap_DObjCalcSkel`. VERIFIED, `cod_lnxded`: syscall 0x6a
runs 0x080c4eb8 with the global stamp 0x0833df04 (clears the skeleton's ready
bits and stores the stamp when the skeleton's differs), then 0x080c4d58 on the
bone. `SV_Frame` (0x0808cdf8) bumps the stamp once before its per-client pass
(0x0808cccc) and, in its frame loop, between game calls 9 and 10, the second
carrying `svs.time`. INFERRED: call 10 is `G_RunFrame`, so a frame runs under
one stamp. VERIFIED, `G_RunFrame`: the first `Scr_RunCurrentThreads`
(0x50653) comes before `trap_DObjInitServerTime` and `G_DObjUpdateServerTime`
(0x5069f, 0x506ed). INFERRED: within one `G_RunFrame` a bone and every bone
above it keep the pose of the first time the frame asked for them. A script
thread due on the last frame's clock (section 11's `wait 0.05` loops, a `wait`
landing on a frame time) that asks for a player's bone poses and fixes it at
the last frame's anim times; a later bone on another branch takes this frame's
times below the shared bones.

VERIFIED, the capture: `k2 linkto(player, "bip01 head")` and
`k3 linkto(player, "tag_weapon_right", (0,0,0), (0,0,0))` run in such a
thread; `k2` reads `(0, 0, 0)` on its first two rows and the head's swing
after, and `k3`'s first re-anchored row reads yaw 7.21 where the next reads
8.19. VERIFIED, vcod measurement, `linkto_ab`:

| Pose rule | `k2` | `k3` first row |
|---|---|---|
| this frame's times | constant 0.36 degrees pitch, 0.71 yaw off | 7.58 |
| every bone of the link frame at last frame's times | matches | 6.62 |
| per-bone rule above (head's chain fixed at the link, the hand's branch posed in the entity pass) | every row within 0.01 units, 0.01 degrees | same |

VERIFIED, `G_EntLinkToWithOffset` (0x68074, above): the four-argument form
calls only the helper, whose bone work is `trap_DObjGetBoneIndex`, so it fixes
nothing; vcod's `link::link` poses the tag only for the two-argument form.
vcod: `GameHost::dobj_time_ms` (last frame's time until
`ScriptRuntime::run_threads` has run `Vm::run_current`) and
`GameHost::dobj_posed` (the time each bone was posed this frame, cleared at
the frame's start and past the entity loop); `link::client_bone` and
`dropItem`'s tag go through it. VERIFIED, live and vcod measurement
(cod11-combat.md 3.6, `probe_stamp`): a script `bulletTrace` falls under the
same stamp and poses every bone of the body, `tag_weapon_right` included;
marks linked to the body read the trace's last-frame pose in that frame's
entity pass on 1.1d, 1.5b and UO. `bulletTrace` and the script `radiusDamage`
walk take and stamp each bone through `dobj_posed`, as do the engine's traces
(below).

VERIFIED, `cod_lnxded`: 0x0808c870 bumps the same stamp (0x0833df04) on every
packet that is not out-of-band (`cursize < 4` or a first word other than -1)
before matching it to a client by address and qport and running that client's
message. INFERRED, off the matching: it is `SV_PacketEvent`, so each client
packet's cmds, and the rounds they fire, pose under their own stamp, and on 1.5
and UO each such build runs the controller hook again (`coduo-vehicles.md` 13,
"The glide back"). VERIFIED, `cod_lnxded`: `SV_Frame` bumps the stamp once
ahead of its bot pass (0x0808cccc, one call per client with flag `+0x10a40`
set: random buttons off `rand()`, then `SV_ClientThink` at 0x0808789c), so
every bot's cmds run under one stamp.

vcod: every engine trace (rounds in a cmd, turret rounds, missiles, blasts,
the lookat trace) stamps its bodies through `GameHost::dobj_posed`;
`replay_moves` starts a stamp at each new packet; the end frames' traces stay
under `G_RunFrame`'s. The build eases the controls, not the end frame.
VERIFIED, vcod measurement (cod11-combat.md 3.7): only so does a round that is
the first build after a turn land where retail's does. vcod queues a tick's
bot cmds as one packet after every real one (`Server::step_bots`), so they run
in slot order under one stamp, as retail's bot pass.

The use key needs no stamp. VERIFIED, items doc 2.1: `G_GetActivateEnt`'s
candidates come off an area walk under contents 0x200000, and its visibility
test is a `trap_Trace` under mask 0x11 with the player as pass entity.
INFERRED: neither meets a body's contents (0x2000000), and `trap_Trace` clips
entities by link box, so the pick poses no DObj; vcod's `item::activate_ent`
and UO's `vehicle::seat::aimed_vehicle` (the vehicle's bounds) trace no bones
either.

## 16. `enableLinkTo`, turrets, model changes and unlinked items

Evidence: a second paired capture against retail 1.1d, mp_carentan,
`dm`-shaped probe, 2026-10-08:
`crates/server/tests/fixtures/movers/mp_carentan-dm-linkto2.txt` (server half,
from `crates/gsc/tests/fixtures/semantics/client-probes/probe_linkto2.gsc`) and
`mp_carentan-dm-linkto2-wire.txt` (client half). The player stands at
`(-512, 2688, -15.91)` throughout. Addresses are `game.mp.i386.so`.

### `enableLinkTo` and `Think_GeneralLink`

VERIFIED, `enableLinkTo` (0x5d5d0): object-model doc 23.2 has its tests and
stores; the think it installs is `Think_GeneralLink` (relocation at 0x5d69c).
VERIFIED, `Think_GeneralLink` (0x68588): writes `ent+0x1fc = level.time + 50`
(0x6859a), tests the link record at `ent+0x2e4` (0x685a0), calls
`G_SetFixedLink(ent, 0)` (0x685af), `G_SetOrigin` (0x685bf), `G_SetAngle`
(0x685d2), writes 1 to `ent+0xc` and `ent+0x30` (0x685d7, 0x685de), calls
`trap_LinkEntity` (0x685e9). INFERRED, off the store at 0x6859a ahead of the
record test: the think re-arms every frame, linked or not, and on a linked
frame does what `G_GeneralLink` does (section 15).

VERIFIED: `Touch_Multi` (0x65a18) raises its `"trigger"` notify (0x65a5a)
before comparing the think with `Think_GeneralLink` (0x65aa2) and `nextthink`
with 0 (0x65ab2); `multi_trigger` (0x65897) and `Use_Multi` (0x6595f) carry
the same compare, `Activate_trigger_damage` at 0x650db and 0x651b8. INFERRED:
a `trigger_multiple` whose think is `Think_GeneralLink` skips the `wait` arm
(`multi_wait`, or the 100 ms `G_FreeEntity` for a `wait` not above 0) and
keeps its think. VERIFIED: `SP_trigger_multiple` (0x64c50) reads `wait` with
default `"0.5"` (0x79940, `G_SpawnFloat` at 0x64c6e).

VERIFIED, off the capture:

- The bombzone_A `trigger_multiple` (entity 176, model `*4`, no `wait` key),
  moved onto the player by an origin write and not linked, notified on every
  touch, two to four a frame with the client's three cmds a frame, from 10700
  to 12150. INFERRED: the notify does not wait out `wait`; only the think arm
  behind it does.
- After `t enableLinkTo()` and `t linkTo(p)`, `p moveto(at, 1)` carries the
  trigger at the parent's pace (`-164.30` at 12750, the player's spot at
  13700); notifies run from 13450 to 15450, the frames its box overlaps the
  player.
- First run: `spawn("trigger_radius", origin, 0, 32, 64)` dies with
  `unable to spawn "trigger_radius" entity`. A trigger to link must come from
  the map.
- `enableLinkTo` on a script_origin is the fatal
  `entity already has linkTo enabled`.

### A linked turret

VERIFIED: `turret_think` (0x5328c) writes `ent+0x1fc = level.time + 50`
(0x5329e), tests the record (0x532a4) and calls `G_GeneralLink` (0x532b2)
before reading the gunner. VERIFIED, off the capture: `mg linkTo(mp)` on the
misc_mg42 297 with `mp` at its origin, then `mp movez(32, 1)` and
`mp rotateyaw(45, 1)`, carries the gun from z 175 to 207 and its yaw from 295
to 340 in step. VERIFIED, off the wire half: entity 297 goes out
`apos.trType` 3 from its spawn, both groups `trType` 1 with `trTime` 0 on every
linked snapshot (16650..19150), both 0 from the unlink on (19650). VERIFIED,
`G_EntUnlink` (0x680d4) calls `G_SetOrigin` (0x680f9) and `G_SetAngle`
(0x68109), and `G_SetAngle` writes 0 to `ent+0x30..0x38` (0x67dcd..0x67ddb).
INFERRED: an unlinked turret never goes back to the 3.

INFERRED, off `turret_think` running in `G_RunFrame`'s entity loop and
`turret_think_client` in `ClientEndFrame` (turrets doc 6): a gunner reads the
gun where this frame's link put it. The capture had no gunner.

### A tag parent's model change

VERIFIED: `G_DObjUpdate` (0x66054) returns at once on a client (`ent+0x158`,
0x66060); otherwise it frees the DObj and, with model byte `ent+0x175` 0,
calls `G_UpdateTagInfoOfChildren(ent, 0)` (0x6608d); with a model it builds
the DObj and calls it with 1 (0x6617e). Callers: the `setModel` method
(0x5dabc, after `G_SetModel`, at 0x5db0d), `G_EntAttach`, `G_EntDetach`,
`G_EntDetachAll` (0x662bc, 0x67fb0, 0x68022). VERIFIED, `G_SetModel`
(0x67020): an empty name stores model byte 0 and returns (0x6702c..0x6703b).
VERIFIED, `G_UpdateTagInfoOfChildren` (0x68294): walks the list at
`ent+0x2e8`; for a record with a tag (`record+0x8`, 0x682bc) it calls
`trap_DObjGetBoneIndex` (0x682e1) only when its second argument is non-zero
(0x682c7) and stores the result at `record+0xc`; the unlink body at
0x682f4..0x6837b (`G_SetOrigin`, `G_SetAngle`, list splice, record freed)
runs past both tests; a record with no tag gets -1 (0x68385). INFERRED: a
child on a tag is unlinked where it stands when the parent has no model or
the new model lacks the bone, and keeps its link with the bone looked up
again otherwise; a child on the entity's own frame is never touched.
VERIFIED: no call or relocation in the module targets `G_UpdateTagInfo`
(0x6819c), the same body for one entity.

VERIFIED, off the capture: k1 linked to a script_model on `bip01 head` with
zero offsets sits `(-2.18, 0, 62.68)` from the model's origin with angles
`(282.50, 0, -90)`, the body's bind pose. After `setModel` to
`playerbody_german_wehrmacht` it rides on at the same offset; after
`setModel("xmodel/weapon_thompson")` it stays at z 38.77, the pose script read
that frame, while k0 (no tag) rides on; setting the body back does not relink
it. Linked again and followed by `setModel("")`, k1 does not move.

### A tag on an attached model

Evidence: one capture against retail 1.1d, mp_carentan,
`client-probes/probe_linkattach.gsc`, no client, 2026-10-16:
`crates/server/tests/fixtures/movers/mp_carentan-dm-linkattach.txt`. A
script_model (170) wears `playerbody_german_wehrmacht` with `weapon_thompson`
attached on `tag_weapon_right`; k1 (171) links to `tag_flash`, a bone only the
thompson has, with zero offsets, and k2 (172) to the same tag keeping its
pose.

VERIFIED, off the capture:

| Phase | Result |
|---|---|
| `att_move` | both links take; both children ride the thompson's muzzle through `movez(16, 1)` and `rotateyaw(45, 1)`. k1 reads `(31.24, 351.37, -58.44)` at yaw 30, the hand's bind pose: a script model's DObj is not animated |
| `att_detach` | `detach` of the thompson unlinks both where they stand (the model moves 16 down, k1 and k2 hold) |
| `att_second` | attaching `weapon_mp40` (own `tag_flash`) on `tag_weapon_left` after the thompson keeps k1 on the thompson's (k1 holds x and y through the lift) |
| `att_detachall` | `detachAll` unlinks both |
| `att_root` | a link to the thompson's root bone `tag_weapon` takes and gives the same pose as `tag_weapon_right` (rows identical) |
| nothing attached | `linkto(m, "tag_flash", ...)` is the fatal `failed to link entity since tag 'tag_flash' does not exist in parent model 'xmodel/playerbody_german_wehrmacht'`: the message names the entity's own model |

INFERRED, off `G_EntDetach` calling `G_DObjUpdate` (0x67fb0) and "A tag
parent's model change": the detach is `G_UpdateTagInfoOfChildren` finding no
bone, and `attach` and `detachAll` (0x662bc, 0x68022) run the same pass.
INFERRED: in `att_second` the pass looks the bone up again and finds the first
model's.

vcod: a non-client parent's tag resolves in the grafted rig of its model and
attachments (`link::model_bone`, `hitrig::HitRigs::bone`), which also finds an
attachment's root bone under its own name; `attach`, `detach` and `detachAll`
run `link::model_changed`. `linkto_ab.rs`'s
`a_tag_on_an_attached_model_takes_a_link_like_retail`: every row matches
retail's print (next section).

### The tag on 1.5 and UO

Evidence: `probe_linkattach` with `probe_exact 1` (every vector printed as
integer parts and exact remainders) on retail 1.5b and UO 1.51b, 2026-11-13:
`mp_carentan-dm-linkattach-1.5.txt` and `-uo.txt` beside the 1.1d capture.
VERIFIED, off the two captures: every row is the same float on both
servers. VERIFIED, the 1.5b capture: the closing link to `tag_flash` with
nothing attached is no fatal there; the console prints `WARNING: Server
should have crashed here!` twice and the probe's next line runs. On UO it
aborts the server after printing the part's bone list.

The chain a link to a non-client's tag runs. Each function named below
has the same instruction sequence on both modules; addresses are UO's,
then 1.5b's.

- VERIFIED: `G_GeneralLink` (0x78bc8, 0x6c4c5) calls `G_SetFixedLink(ent,
  0)` (0x78a69, 0x6c366). That takes the parent's tag matrix from
  `G_CalcTagParentAxis` (0x7882b, 0x6c128), multiplies the link record's
  matrix (`record+0x10`) by it with `MatrixMultiply43` (0x3bc1c, 0x349cc)
  into the origin, and writes `AxisToAngles` (0x3e350, 0x37100) of the
  product into `r.currentAngles`.
- VERIFIED: the three-argument link's record is `G_CalcTagAxis` (0x78982,
  0x6c27f) with its second argument 0. `MatrixInverseOrthogonal43`
  (0x3c6ec, 0x3549c) transposes the parent's matrix and turns the negated
  origin (`vec3_origin - origin`, each component stored) through the
  transpose with `MatrixTransformVector` (0x3cd97, 0x35b47). Then
  `MatrixMultiply43` multiplies the child's `AnglesToAxis` and origin by
  that inverse.
- VERIFIED: every element of either multiply and of the transform is its
  three products and the add on the x87 stack, rounded to a float once.
- VERIFIED: `AxisToAngles` takes pitch and yaw from `vectoangles` (0x3aded,
  0x33b9d) of the forward axis. The roll turns the left axis back by
  `-yaw` and then `-pitch`. Each angle's radians are `(float)(-a * pi /
  180)` (the doubles at UO 0x9dba0 and 0x9dba8), its sine and cosine come
  from `fsincos` (UO 0x3f464) and are stored, and each turned component is
  stored.
- VERIFIED: `vectosignedpitch` (0x3ad1d, 0x33acd) of the turned left is
  `(float)(atan2(z, (float)sqrt(x*x + y*y)) * -180 / pi)`, or -90 / 90
  for a vertical vector. With the turned left's y below 0 the roll is
  that plus 180 when it is negative and minus 180 otherwise; else it is
  the negation.

VERIFIED, vcod measurement (`linkto_ab`'s `_on_15` and `_on_uo`): with
the parent's tag matrix built from the bind records a DObj with no anim
tree holds (`skeleton::Record::bind`, each bone's bind rotation's shorts
over 32767, unnormalized) through the skeleton build
(`PoseBuffer::stored_worlds`, cod11-combat.md 3.1, "The pose on 1.5 and
UO"), and the chain above, every float of all 114 rows is retail's bit on
both servers. Before, the tag came off the normalized bind pose in `f32`:
k1 sat 0.011 units and 0.018 degrees off (`-478.94553` against
`-478.9361`), and the pose-keeping k2 sat 1 to 2 float steps off.

VERIFIED, vcod measurement (`a_tag_on_an_attached_model_takes_a_link_like_retail`,
the 1.1d capture): the same bind records bring every row of 1.1d's to its
print, where the normalized bind pose left 113 rows a print step off (up to
0.02 units and 0.03 degrees). INFERRED: 1.1d's DObj holds the same
quantized bind rotation. Its build's arithmetic is not read; on 1.1 vcod
composes the tag with the parent in `f32` as before.

vcod: `link::model_bone_stored` (CoD1 hosts), and on 1.5b and UO
`Frame::within_stored`, `Frame::relative_to_stored` and
`Frame::angles_stored` in the link and `run_one`. A client parent's tag
takes the same chain (`link::client_bone_stored`). Not ported: vehicle
seat and gun tags (`vehicle::seat::posed`) and a turret's tags
(`turretpose`) still pose through `glam`. No exact capture covers them.

### An item unlinked in mid-air

VERIFIED, `G_RunItem` (0x4eb18): with `groundEntityNum` (`ent+0x7c`) 0x3ff
and `pos.trType` not 5 (0x4eb24, 0x4eb2d) it writes 5 and the level time
(0x4eb33, 0x4eb3f); a type of 0 or 8 then runs only `G_RunThink`
(0x4eb42..0x4eb52).

VERIFIED, off the capture: an `item_health` linked on its spawn frame at the
player's origin plus 100 (it reads 8 lower, 76.09), carried up 20 and
unlinked, stays at 96.09 for the 2 s sampled; one that had landed at -22.91,
lifted 60 and unlinked, stays at 37.09. VERIFIED, off the wire half: from the
unlink both go out `pos.trType` 5 with `trTime` the snapshot's own time and
the same `trBase`, as the landed one went out before its link (30650..30750).
UNVERIFIED: which store puts the type back each frame.
`crate::game::item::run_items` produces the same rows and wire.

vcod: `crate::game::link::enable` is `enableLinkTo` (link bit
`link::has_link_bit`); the touch pass never spends an enabled
`trigger_multiple` (section 17). `linkTo` takes a turret; the record reads the
gun's own pose, so barrel and gunner follow, and `TurretRecord::angle_set`
keeps `apos.trType` 0 after the unlink. `link::model_changed`, from the
`setModel` builtin on a non-client, is `G_UpdateTagInfoOfChildren`.
`crates/server/tests/linkto_ab.rs` replays the probe: every row matches to
0.06 (the player stood 0.04 higher on ours), the notify frames match, and the
turret's and mid-air item's wire types and bases match per snapshot.

## 17. `Touch_Multi`'s `wait`: a free, never a gate

Evidence: paired capture against retail 1.1d, 2026-10-09:
`crates/server/tests/fixtures/movers/mp_carentan-dm-trigwait.txt` from
`crates/gsc/tests/fixtures/semantics/client-probes/probe_trigwait.gsc`. No
stock MP map gives a trigger a `wait` key and `wait` is a gsc keyword, so a
script cannot write the field; the capture ran on mp_carentan with four
same-length entity lump edits (fixture header), served from a pk3 in the
server's homepath. Addresses are `game.mp.i386.so`.

VERIFIED: `SP_trigger_multiple` (0x64c50) reads `wait` into `ent+0x268`
(default `"0.5"`, strings 0x79944 and 0x79940, `G_SpawnFloat` at 0x64c6e) and
`random` into `ent+0x26c` (default `"0"`, 0x7994b, 0x79949), and installs
`Touch_Multi` and `Use_Multi` (0x64cdd, 0x64ce7). `SP_trigger_once`
(0x65c0c) stores -1.0 (`.rodata` 0x79ad8) into `ent+0x268` (0x65c16) and
installs the same two.

VERIFIED, `Touch_Multi` (0x65a18), with the script system active:

1. Raises `"trigger"` before any `wait` test: at once through `Scr_Notify`
   (0x65a5a) when the pending queue at `level+0x29ec` holds 256 entries, else
   queued there (0x65a68..0x65a96).
2. Stores the toucher at `ent+0x25c` (0x65a9c); returns when the think is
   `Think_GeneralLink` (0x65aa2) or `nextthink` is non-zero (0x65ab2).
3. `wait` above 0 (compare 0x65abf..0x65acf): installs `multi_wait`,
   `nextthink = level.time + (wait + random * crandom) * 1000`
   (0x65ad1..0x65b28).
4. `wait` not above 0: clears the touch function `ent+0x20c` (0x65b30),
   `nextthink = level.time + 100` (0x65b3f), installs `G_FreeEntity`
   (0x65b48).

VERIFIED: `multi_wait` (0x65870) only zeroes `nextthink`; `multi_trigger`
(0x65884) and `Use_Multi` (0x6594c) carry the same arm without the notify.
`G_TouchTriggers` (0x3fa22..0x3fa8b) raises the two `"touch"` notifies on
every contact and calls the touch function only when `ent+0x20c` is non-null.
INFERRED: a positive `wait` gates nothing a script sees (`nextthink` gates
only the arm); a `wait` not above 0, as every `trigger_once` has, lets the
first touch notify and frees the trigger two frames on; a trigger whose think
is `Think_GeneralLink` is never spent.

VERIFIED, off the capture (about three cmds a frame):

| Trigger | Result |
|---|---|
| bombzone_A, `"wait" "5"` | three notifies a frame (two to four on a few) for all 20 frames it sat on the player |
| bombzone_B, `"wait" "0"` | one notify, frame after the origin write (offset 50); `isdefined` 1 at offsets 0, 50, 100 and 0 from 150; the `trigger_multiple` + `trigger_once` count drops 4 to 3 the same frame |
| auto1, `"wait" "-1"`, `enableLinkTo` with no parent | notifies every frame, never freed |
| auto2, `trigger_once` | one notify at offset 50, freed at 150 like bombzone_B |

INFERRED, off the notify at offset 50: the touch ran in the cmds between
offsets 0 and 50 at `level.time` = offset 0, so `nextthink` is offset 100 and
`G_FreeEntity` ran in that frame's entity pass (`G_RunEntity`, 0x502bc, called
from `G_RunFrame` at 0x50955), after the frame's timed waits had read the
trigger as defined: `G_RunFrame` calls `Scr_SetTime` (0x5070c) ahead of the
entity loop.

vcod: `crate::game::trigger::Triggers::fire` returns `Fire::Spent` for the
touch that spends a `trigger_multiple` with `wait` not above 0 or any
`trigger_once`; the touch pass schedules the free `SPENT_FREE_MS` out.
`crate::game::spawn` stores 500 for a `trigger_multiple` with no `wait` key
and -1000 for every `trigger_once`. Nothing reads `random`.
`crates/server/tests/linkto_ab.rs` (`touch_multi_spends_a_trigger_like_retail`)
replays the capture on the same patched lump: entity numbers, every frame's
`isdefined` and count, and the notify frames match.

## 18. `func_bobbing` and `func_pendulum`

The two classnames section 12 found storing `TR_SINE`. Three retail runs of
`crates/gsc/tests/fixtures/semantics/client-probes/probe_bobbing.gsc` on
mp_carentan, 2026-10-11; the probe directory's README has the recipes and
`crates/server/tests/sine_ab.rs` replays all three:

| Fixture (`crates/server/tests/fixtures/movers/`) | Contents |
|---|---|
| `mp_carentan-dm-sine.txt` | five script-spawned movers, no client, up to the drop |
| `-sine-map.txt` | a map-placed pair: bombzone brush models `*5` and `*6` rewritten into a bobbing and a pendulum (keys in the header) |
| `-sine-wire.txt` | what a `--net-probe --probe-ride` client was sent of a spawned pair and the map-placed pair |

### The spawn functions

VERIFIED, `SP_func_bobbing` (game.mp `0x583d0`): `G_SpawnFloat("speed", "4")`
into `ent->speed` (`+0x1e0`), `"height"` default `"32"`,
`G_SpawnInt("dmg", "2")` into `+0x238`, `"phase"` default `"0"`, then
`trap_SetBrushModel` and `InitMover`. Then it copies `r.currentOrigin` into
`pos.trBase`, stores `pos.trDuration = speed * 1000` and
`pos.trTime = trDuration * phase`, both truncated (control word `| 0xc00`),
`pos.trType` 4 (`0x584c4`), and the height into `trDelta`'s x for spawnflags 1,
y for spawnflags 2, z otherwise.

VERIFIED, `SP_func_pendulum` (`0x56fe0`): `"speed"` default `"30"` into a
local, `dmg`, `phase`, `trap_SetBrushModel`, then the period:
`length = |r.mins[2]|`, at least 8 (`0x75dc4`);
`freq = 0.15915494309189535 * sqrt(g_gravity / (length * 3))` (`0x75dd0`,
`0x75dc8`); period `1000 / freq` truncated (`0x75dd8`, `0x5709a`). It stores
the period in `pos.trDuration` ahead of `InitMover` (`0x570a3`), copies
`r.currentOrigin` to `pos.trBase` and `r.currentAngles` to `apos.trBase`, and
writes the period to `apos.trDuration` (`0x570e2`), `trDuration * phase`
truncated to `apos.trTime`, `apos.trType` 4 (`0x57110`) and the speed to
`apos.trDelta[2]` (`0x5711a`): a roll swing.

VERIFIED, `InitMover` (`0x56454`), past the link where Ghidra's export cuts:
`noise` and `light`/`color` keys, `Reached_BinaryMover` into `ent+0x204`
(`0x565da`), `r.currentOrigin = pos1` (`+0x1a4`, `0x565e4`), `moverState` 0,
`r.svFlags` 0x80, `s.eType` 5 (`0x56619`), the link, then `pos.trBase = pos1`,
`pos.trType` 0, a zero `speed` made 100 (`0x75cb8`, `0x566a4`) and
`pos.trDuration` the `pos1`..`pos2` distance over the speed, at least 1
(`0x566dd`). VERIFIED: the only stores to `+0x1a4` in `.text` are
`SP_func_door`'s (`0x56d83`, `0x56eaf`), so for both classnames `pos1` is zero
and the mover swings about the world origin. `InitMover`'s 1 overwrites the
pendulum's period in `pos.trDuration`; the bobbing writes its period after.

VERIFIED, off the runs:

- **Spawn origin and `origin` key unused.** `spawn("func_bobbing", (10, 20,
  100))` reads `origin` (0, 0, 0) on the next line; the map-placed bobbing
  with `"origin" "16 32 8"` swings about x 0.
- **Spawnflags.** `spawn(..., 1)` swings along x, `spawn(..., 2)` along y (the
  third argument lands in `spawnflags`, `0x5d2e7`); the pendulum ignores them.
  A script spawn reads no key (`G_SpawnFloat` sees the spawn vars
  `G_ParseSpawnVars` cleared after the last map block), so it takes every
  default: 32 units over 4 s, and 30 degrees.
- **Keys.** The map-placed bobbing (`speed` 3, `height` 40, `phase` 100,
  spawnflags 1) went out `trTime` 300000, `trDuration` 3000, `trDelta`
  (40, 0, 0); the pendulum (`speed` 20, `phase` 100) `apos` `trTime` 246300,
  `trDuration` 2463, `trDelta` (0, 0, 20).
- **The pendulum's length.** Periods are 9711 for a script-spawned pendulum
  (model 0, the world) and 2463 for `*6`. The lump-27 floors (-636.12, -40)
  give 9704 and 2433; floors one unit lower give both retail periods.
  VERIFIED, cod_lnxded: `trap_SetBrushModel` (0x8089544; syscall arm
  0x8088360) takes `CM_InlineModel(s.index)` (0x804bac8) and copies
  `CM_ModelBounds` (0x804bbac) into `r.mins`/`r.maxs`, and the submodel loader
  stores each lump-27 bound a unit out (`fld1`, `fsubr` / `fadd` at
  0x804a8e5..0x804a8fa). `SV_LinkEntity` grows the abs box one more unit
  (`docs/protocol-1.1.md`, "The box an entity links with"), so a brush
  entity's `r.absmin` sits two units outside its lump bounds. vcod:
  `bsp::Model::clip_bounds` is the loader's box, behind
  `GameHost::model_bounds` and `Visibility::model_bounds` (the lump's box is
  the trap).
- **Script reads.** `origin` and `angles` read the trajectory at the last
  frame's level time (section 8); the roll reads negative through half the
  swing: a sine mover's angles are not normalized.
- **The wire.** `eType` 5, `solid` 0xffffff, `eFlags` 0, `index` the inline
  model number (0 for a script-spawned one). The bobbing's `apos` and the
  pendulum's `pos` are stationary at the world origin, the pendulum's `pos`
  with `trDuration` 1. A client at the world origin was sent the spawned pair.

### The drop

VERIFIED: `G_RunMover` reaches `G_MoverTeam` off a non-zero `trType`
(`0x57666`); `G_MoverTeam`, once the push went through, calls `ent+0x204` for
a group whose `trTime + trDuration` the level time has reached
(`0x558a0`..`0x558e9`). `Reached_BinaryMover` (`0x55c9c`) has arms for mover
states 3, 4, 9 and 10 only and `G_Error`s `"Reached_BinaryMover: bad
moverState"` on `InitMover`'s 0. VERIFIED: the spawned run's last frame is
`f 4000`, one period after `trTime` 0, and the console prints that error; a
map-placed one with `phase` 100 ran 40 s untouched. A bobbing spawned past one
period drops the level on its first entity pass.

VERIFIED, cod_lnxded `Com_Error` (`0x806b93c`) with `ERR_DROP` on a dedicated
server: `com_errorEntered` set, the longjmp; `0x806b7ec` prints `ERROR: %s`
and runs `SV_Shutdown` (`0x808ad8c`), whose `SV_FinalMessage` loop is gated on
`com_errorEntered` being 0, so no client is told. VERIFIED, the wire run: the
client's last snapshot was serverTime 3950, no server command after. INFERRED:
the master flatline still goes out and the process idles as after
`killserver` (handshake doc, "Shutdown"). The message reaches the console with
a 0x15 byte ahead of it.

vcod: `crate::game::mover::spawn_sine` is both spawn functions plus
`InitMover`, called from the map load and the `spawn` builtin; it links the
entity with the model's bounds and contents -1 and gives it a mover row whose
plans are the two trajectories. `mover::run_one` writes `origin` and the raw
`angles` back each frame like any mover; `mover::reached` sets
`GameHost::game_error` when a group is due at mover state 0, on the mover's
turn or after its push. The entity pass stops there and `Server::drop_level`
logs the error and shuts the level down with no final message. A `useBy` runs
the mover through `Use_BinaryMover`'s legs instead, which stops a bobbing at
the world origin (`cod11-combat.md` 5.9). A script-spawned mover's clip is a
moving copy of the world's (section 19) that pushes like any brush model
(section 20) and crushes what it cannot place (section 21); a map-placed one
moves and pushes like a script brush model.

VERIFIED, the same probe: a script-spawned entity's unwritten `spawnflags`
reads 0, as `Scr_GetGenericField` reads an int straight off the zeroed
`gentity_t` (`cod11-gsc-object-model.md` 4). vcod reads an unwritten engine
int, float or vector entity field as 0.

## 19. A script-spawned brush entity clips as a copy of the world

Evidence: `crates/gsc/tests/fixtures/semantics/client-probes/probe_spawnclip.gsc`
on retail 1.1d, mp_carentan, no client, 2026-10-12:
`crates/server/tests/fixtures/movers/mp_carentan-dm-spawnclip.txt`.

VERIFIED, game.mp: the `spawn` builtin (0x5d268) runs `G_Spawn`, writes the
classname, `r.currentOrigin` and `spawnflags`, and calls `G_CallSpawnEntity`;
nothing writes `s.index`, so it stays 0. `SP_script_brushmodel` (0x60fb8)
calls `trap_SetBrushModel`, `InitScriptMover`, writes `r.contents` 1
(0x60fd4) and links. VERIFIED, cod_lnxded: the brush-model setter (0x8089544)
takes `CM_InlineModel(s.index)` (the world's model for 0), copies its bounds
(section 18) and sets `r.bmodel`. The entity clip at 0x8091390 takes
`CM_InlineModel(s.index)` (0x80913a3) for an entity with `r.bmodel`, else a
temporary box model of `r.mins`/`r.maxs` (0x804baf8), and traces it with
`CM_TransformedBoxTrace` (0x80568d8) at `r.currentOrigin` (`+0x134`) and, for
a bmodel, `r.currentAngles` (`+0x140`).

VERIFIED, off the capture (`bulletTrace` straight down from 100 above the
first dm spawn point, floor at z -9.46):

| State | Hit |
|---|---|
| nothing spawned | floor, no entity |
| `script_brushmodel` spawned at (0, 0, 64) (reads `.model` `""`) | 64 up (z 54.54), same frame, entity 299 |
| after `movez(32, 0.5)` | 96 up (86.54) |
| `notSolid()` / `solid()` / `delete()` | floor at once / the copy / floor the same frame |
| spawned `func_bobbing` swinging down from the world origin (section 18) | floor all period (the copy sits below it) |

INFERRED: a brush entity whose `s.index` is 0 carries a copy of the world's
whole clip (brushes, terrain, patches; not the submodels or static models)
wherever its entity stands, moving with it. A tie with the world keeps the
world's hit (Q3's `SV_ClipMoveToEntities` replaces only on a nearer fraction
or allsolid). No stock map holds a brush entity without a `*N` model or with
`*0`.

vcod: `CollisionWorld::set_world_copy` holds one copy per entity (origin,
angles, `r.contents`), clipped after the world and the posed submodels in
every trace, the sweep turned into the copy's frame and the box kept axial
there (Q3's approximation for a rotated box), merged by the rule above;
`entity_num` names the copy's entity and `point_contents` reads the copies. A
capsule's sphere offset is turned into the copy's frame for its brushes
(section 21). `GameHost::link_entity_at` places the copy of a brush entity
with no `*N` (or `*0`) at every link, `set_link_contents` follows
`solid`/`notSolid`, `delete` and `free_entity` take it out, and a map load
clears them all. `bulletTrace` returns the brush entity a trace stopped on.
`spawnclip_ab.rs` replays the capture line for line.

Section 20 measures the copy on the wire, as a pusher and in the client's
prediction.

## 20. A world copy on the wire, as a pusher, and on the client

Evidence: `crates/gsc/tests/fixtures/semantics/client-probes/probe_worldcopy.gsc`
on retail 1.1d, mp_carentan, one `--probe-ride` client, 2026-10-12, retaken
2026-10-20 with the client's cmds logged.

| File | Contents |
|---|---|
| `crates/server/tests/fixtures/movers/mp_carentan-dm-worldcopy.txt` | server `PROBE f` line per frame (player, health pack, copy) and the client's `CMDS` lines (section 21, "The slide down the slope") |
| `-worldcopy-wire.txt` | client `RIDE` line per snapshot, `RIDE_NEW` per entity that entered, trajectory lines |

The player stands at (-512, 2688) on the street's terrain (z -23.875) with a
pack dropped 40 east; a `script_brushmodel` spawned at (0, 0, -64) rises 96 in
2 s, slides 32 east in 1 s, sinks 96 in 2 s and is deleted; then a
`func_static` is spawned at (0, 0, -20000) and deleted.

### On the wire

VERIFIED, off the capture:

- The `script_brushmodel` enters the snapshot of its spawn frame as `eType` 8,
  `solid` 0xffffff, `index` 0, `eFlags` 0, at (0, 0, -64), and leaves on its
  `delete()` frame, as a `*N` brush model does (section 14). Its moves go out
  as any script mover's.
- The `func_static` enters as `eType` 5, `solid` 0xffffff, `index` 0, origin
  (0, 0, 0) whatever the spawn origin: `SP_func_static` (game.mp 0x5820c) runs
  `trap_SetBrushModel` and `InitMover` (section 18), copies `r.currentOrigin`
  into `s.pos.trBase`, and unlinks under spawnflags 1. VERIFIED, a scan of
  every `.bsp` in the 1.1 paks: none holds a `func_static`.

VERIFIED, cgame: the inline-model loop at 0x300218b0 registers handles from 1
(`ebp` 1 at 0x3002104e, count from syscall 0x20) into the table at 0x301d31c0,
and nothing else writes it. INFERRED: an `index` 0 brush model draws through
handle 0, which draws no world; vcod draws nothing for one
(`entities::resolve_visual`).

vcod: `crate::game::wire` sends a `script_brushmodel` with no `*N` with
`index` 0, and a `func_static` (`mover::spawn_static`) as `eType` 5 like the
sine movers. `worldcopy_ab.rs`'s
`a_spawned_brush_entity_goes_on_the_wire_like_retail` compares both entry
lines.

### The push

VERIFIED, off the capture:

- **Lifted.** Player and pack, both on the street (ground 1022), leave it the
  frame the copy's floor passes their feet: the copy at script z 0.8, its clip
  a frame ahead at 3.2 (section 8), the player a frame later at -22.58, then
  2.4 a frame (the copy's speed) to 8.125, 32 over the street. The player's
  ground then reads the copy (171).
- **Carried.** Riding east, both follow the copy 1.6 a frame until the real
  world is in the way: the pack stops at x -467.2 and from then is left in
  place each frame and falls back onto the copy (`trType` 5 every other
  snapshot); the player is jittered (-4, -4) off his carried spot at x -492.8
  and again at -488.8 (section 12's jitter).
- **Lowered.** Both ride the copy down to the street (z -23.07 on the frame
  the copy's floor sinks through the street's), the player is jittered once
  more, and both rest on the street.

INFERRED: `G_MoverPush` treats the copy as any brush model. Its list box is
the copy's link box, the world's bounds about its origin, so every player and
item is listed; the candidate trace and `G_TryPushingEntity`'s trace see the
copy, and the real world blocks a carried spot.

VERIFIED, vcod measurement against the capture: two things decide retail's
frames. The candidate and spot tests are a trace that does not move, which
against terrain is the capsule position test (`cod11-mantle.md`, "A trace that
does not move"); without it the push came a frame late and left the player 0.8
inside the copy's floor. And `G_TryPushingEntity`'s trace is `trap_Trace`'s box
against brushes: with a capsule the player met the obstacle 8 units of travel
later and jittered once where retail jittered twice.

vcod: `mover::clip_step` hands a world copy's move to the push as
`Pusher::Copy`; `CollisionWorld::pusher_box_trace` tests a body against the
copy alone, `box_trace_except` passes through it, and `box_position_test` is
the spot test (boxes against brushes, capsule position tests against terrain
and patches), asked by both the player's and the item's fit. A copy whose
entity is `notSolid` pushes nothing, as a non-solid submodel.
`worldcopy_ab.rs`'s `a_moving_world_copy_pushes_like_retail` replays the
capture frame for frame. VERIFIED, the capture: every landing of the pack rests
it a fresh `G_BounceItem` lift (`docs/research/cod11-items.md` 14.2), up to
half a unit, over a floor ours finds to the hundredth (-23.875 on the street,
8.125 on the copy); ours cannot replay retail's `rand()`, so the gate lands the
pack with retail's lift and holds each lift to the range. Replayed on the
client's own cmds every frame matches to the hundredth; one 50 ms cmd a frame
left the player 0.11 higher the frame after the first lift (a frame in the
air), the cmd split of section 21.

### The client

VERIFIED, cgame: the brush model arm of `CG_ClipMoveToEntities` (0x30028df0)
calls syscall 0x21 with `es.index` untested, and tests no contents mask on
that arm. INFERRED: `CM_InlineModel(0)` is the world's model, so retail's
prediction clips an `index` 0 brush model as a copy of the whole world at its
trajectory, whatever the mask, as the server does.

vcod: `pmove::movers::SnapshotMovers` keeps an `index` 0 brush model and
places it as a world copy (contents -1) at the prediction time.
`predict_ab.rs`'s `predictor_rides_a_retail_world_copy` predicts the capture's
rider from each snapshot to the next against retail's next playerstate: exact
to 0.05 on every snapshot both stand on the copy with no jitter between (82);
all 82 miss without the copy.

Section 21 measures a turned copy and the `TR_SINE` crush.

## 21. A turned world copy, `func_static` damage and the sine crush

Three captures on retail 1.1d, mp_carentan, each with one
`--probe-team allies` client, 2026-10-12 (the first two retaken 2026-10-20 by
a `--probe-ride` client, which logs its cmds); `crates/server/tests/worldcopy_ab.rs`
replays all three:

| Fixture (`crates/server/tests/fixtures/movers/`) | Probe | Setup |
|---|---|---|
| `mp_carentan-dm-worldcopy2.txt` | `client-probes/probe_worldcopy2.gsc` | player at (-512, 2688) on the street, health pack 40 east; a `script_brushmodel` spawned at (-576, 2688, -75.875), so the player stands 64 out along the copy's x on the copy's terrain hill (the world's ground about its origin), 8 under the street; it rises 16, `rotateyaw` 45 and 45 more, back 90, `rotatepitch` 5 and back, sinks 16, is deleted. A `PROBE f` line per frame (player, pack, copy origin and angles) and a 3 x 3 grid of `bulletTrace`s round the player at each rest. Then three `func_static`s at the world origin, spawnflags 0, 2 and 4, the second moved 64 up by an `origin` write, and three `radiusDamage`s |
| `-worldcopy3.txt` | `client-probes/probe_worldcopy3.gsc` | same copy and verbs to the end of the unpitch, a `PROBE g` line per frame with the player's offset from the copy's spawn origin to the ten-thousandth |
| `-sinecrush.txt` | `client-probes/probe_sinecrush.gsc` | a `func_bobbing` spawned on the first frame; the player stood at (752, -40), under a sloped ceiling 89 to 96 over its floor, at 2400 |

### Clip

VERIFIED, the grid: every trace at yaw 0, 45, 90 and pitch 5 names the copy
(171) or the world where vcod's copy, turned through `AnglesToAxis`, does, at
the same height to the hundredth.

VERIFIED, `cod_lnxded` `CM_TransformedBoxTrace` (0x80568d8): a model is turned
when any angle is non-zero and it is not the box handle (0x1ff). Start and end
go into the model's frame through `AngleVectors` with the right vector
inverted (0x8063388, 0x8065c38); the trace's box stays axial in that frame
(half extents not turned); the capsule's sphere offset (`half height - radius`
up) is turned as `(forward.z, -left.z, up.z) * offset`, Quake III's
`sphere.offset[1] = -matrix[1][2] * t`. The trace work struct carries the
offset at `+0x100..+0x108` (copied from the sphere argument by 0x8056310).
VERIFIED: the brush arm (0x8054e90) reads `+0x100..+0x108` to pick the sphere
nearer each non-axial side (0x805506d..0x8055080); the terrain sweep
(0x8052a58) and position test (0x8053280) read only radius and half height
(`+0xf8`, `+0xfc`) and shift z. INFERRED: against a turned model's brushes the
capsule stays upright in the world, give or take the `-left.z` sign only a
roll reaches; against its terrain it tilts with the model. VERIFIED, section 22
("A rolled submodel's non-axial side"): against a rolled model's non-axial
side the capsule sits where the mirrored offset puts it, not upright.

vcod: `WorldCopy::sphere` turns the offset and `clip_brush_capsule` takes it;
terrain and patches keep the upright one.

### Push

VERIFIED, vcod measurement against the 2026-10-20 captures, replayed on the
client's own cmds (below): through the lift, both 45-degree turns, the turn
back, the pitch and unpitch and the sink, every frame of `-worldcopy2` matches
to the hundredth (the (-4, -4) jitters the real world forces, the falls after
them and the slides down the copy's slope included), and the pack rests and is
carried the same; every `PROBE g` row of `-worldcopy3` matches to the
ten-thousandth.

The 2026-10-12 captures logged no cmds. Replayed at an even three cmds a
frame, with the player reset to retail's rest after each fall, four rows
differed, each a player in the air: the first lift's slide down the slope
(0.86 on retail, 0.54 on ours), the fall after a jitter off a blocked turn and
one off the lowered copy, and the pitch's blocked frame and the jitter's fall
after it. The pitch's frame was the resync, which put ours' player at
retail's printed hundredths, up to 0.005 off the real spot, where the turned
centre's margin at the blocked frame is 0.002.

### The slide down the slope

VERIFIED, vcod measurement against `-worldcopy3`, replayed at the probe
client's rate (a cmd every 16 or 17 ms, three a frame): the first lift's row
matches retail's `PROBE g` to the ten-thousandth through frame 11,
(64 - 0.006, 0.3328, 52.2786) off the copy's origin, and holds 0.007 off from
frame 12. Sent one 50 ms cmd a frame, ours slid 0.11 and missed by 0.43, with
pmove unchanged.

INFERRED, off ours' per-cmd trace: the lift leaves the player 0.47 over the
copy's slope (normal z 0.978), out of the ground trace's reach. At three cmds a
frame the second cmd's fall ends 0.2 over the slope with velocity -20, inside
the quarter unit the closing ground trace reaches, so the third cmd starts on
the ground with that fall in its velocity. `PM_WalkMove`'s clip onto the plane
keeps the speed (0x2f5b8-0x2f6b3, `cod11-mantle.md`), so the fall becomes 20
u/s down the slope and the next cmds' friction takes it off: 0.33 that frame,
0.21 the next. A 50 ms cmd falls the whole 0.47 in one air move, whose slide
clips the velocity on the slope and keeps only 8.

So how far a fall onto a slope slides depends on where the cmd boundaries fall.
VERIFIED, vcod measurement in the `-worldcopy2` replay, the first lift's slide
by cmd pattern:

| Cmds a frame | 1 x 50 ms | 2 x 25 | 3 x 16-17 | 4 x 12-13 | 5 x 10 | retail | `-worldcopy3` |
|---|---|---|---|---|---|---|---|
| Slide | 0.11 | 0.24 | 0.54 | 0.48 | 0.64 | 0.86 | 0.55 |

INFERRED: the probe client sleeps `--probe-cmd-ms` (16) between cmds, each in
its own packet (`client/src/probe.rs`), so the lengths and how they straddle a
server frame differ run to run; the 2026-10-12 captures did not record them.
The pitch jitter's fall is the same case: in `-worldcopy3`'s first take
retail's frame 15 sample sat 0.30 under the jittered spot with x and y
untouched, a fall no cmd had landed yet, where ours' three even cmds had landed
it and slid it 0.32.

VERIFIED, vcod measurement against the 2026-10-20 captures: `--probe-ride`
prints `--probe-fall`'s `CMDS` lines (each cmd's `serverTime`, each snapshot's
`commandTime`), and `worldcopy_ab.rs` replays them through
`ProbeHold::framed`, each cmd on retail's side of a frame, the client joined so
its first snapshot falls on retail's. Every fall and slide in `-worldcopy`,
`-worldcopy2`, `-worldcopy3` and `mp_harbor-dm-turnedslab` then matches: frames
to the hundredth, `PROBE g` rows to the ten-thousandth, no resync.

### The carry's rounding

VERIFIED, `game.mp.i386.so` `G_TryPushingEntity` 0x54956-0x54aa1: the moved
point (`r.currentOrigin + move`) is stored to a float (0x54961, 0x54970,
0x54982). `org` (that point less the pusher's `r.currentOrigin`,
`fsub [eax+0x134]` at 0x549eb, 0x54a01, 0x54a1b), the three dot products with
the stored matrix rows and `org2 - org` (`fsubrp` at 0x54a64, 0x54a7e,
0x54a94) stay on the x87 stack, and the sum with the stored moved point
(`fadd [ebp-0xcc]` at 0x54a84, `[ebp-0xd0]` at 0x54a96) is rounded once by
the `fstp` to the trace start (0x54a9e). INFERRED: Quake III's
`trBase + (org2 - org)` at extended precision.

VERIFIED, vcod measurement in the `-worldcopy3` replay: turning the point in
floats as `pusher + R * (point - pusher)` left the rider 0.0002 a frame off
retail's through each turn, 0.0017 by the pitch's rest, and every `PROBE g`
row of the pitch (182) missed the ten-thousandth. With 1.1d's order and one
rounding (`push::carried`, in `f64`) all match.

VERIFIED, the 1.5b and UO modules store every step:

| Step | 1.5b `game.mp.i386.so` (`G_TryPushingEntity` 0x55aa1) | UO 1.51b `game.mp.uo.i386.so` (0x605e5) |
|---|---|---|
| moved point stored | `fstps [ebp-0xd8..-0xd0]`, 0x55aef, 0x55b06, 0x55b1d | same sequence |
| matrix | `G_CreateRotationMatrix` (0x55943: `AngleVectors`, `VectorInverse` of the right row), `G_TransposeMatrix` (0x5598f), called 0x55b33, 0x55b48 | `G_CreateRotationMatrix` 0x60487 |
| `org` (point less `r.currentOrigin`) stored, copied to `org2` | `fsubs [eax+0x138]`; 0x55b5c, 0x55b6e, 0x55b80 | pusher origin at `+0x13c` |
| each row's three products summed on the stack and stored | `G_RotatePoint` (0x559fa), `fstps` 0x55a43, 0x55a70, 0x55a9d | `G_RotatePoint` 0x6053e, same three `fstps` |
| `org2 - org` stored | 0x55bad, 0x55bb6, 0x55bbf | same |
| moved point plus it stored (trace start) | 0x55bcb, 0x55bda, 0x55be9 | same |

So 1.5b and UO round `org`, each turned row, `org2 - org` and the sum to
floats (`push::carried` under `Arith::Stored`). VERIFIED, vcod measurement:
`probe_worldcopy3` on 1.5b and UO (`-worldcopy3-1.5.txt`, `-worldcopy3-uo.txt`,
2026-10-21, one `--probe-ride` client each), replayed by `worldcopy_ab`: with
the stored order every player row (`PROBE f` column 0 to 0.05, every
`PROBE g` row to the ten-thousandth) matches on both. The float order
`pusher + R * (point - pusher)` missed 169 rows on 1.5b and 127 on UO.

VERIFIED, the same UO capture: the health pack on the turning copy lands a
second time at yaw 10.5 (sample 16000 at -15.69, 16100 at -14.83, a lift of
0.86 off a floor at -15.69).

INFERRED, UO `G_RunItem` (0x56876) and 1.5b's (0x4d63d), off the decompiled
control flow: the same as 1.1d's. A ground of `ENTITYNUM_NONE` (`+0x7c` 0x3ff)
turns `s.pos` to `TR_GRAVITY` at `level.time`, a stationary `s.pos` only
thinks, and the sweep's mask is `clipmask` or 0x491. UO's `G_TryPushingEntity`
(0x605e5) tests the carried spot, the jitter, then the item's own spot
(`G_TestEntityPosition` 0x60378 at `r.currentOrigin`, one zero-length
`trap_Trace`); a clear own spot sets the ground to 0x3ff. `G_MoverPush`
(static, 0x60c1b) relinks an `eType` 3 that fits nowhere and goes on, ground
untouched. vcod's `fit_item` and `place_pushed` follow that order.

VERIFIED, vcod measurement: a harness trap, not a server one. Lifting ours'
pack to retail's rest after the landing frame made that frame's push (copy
171, after the pack's 170) test the pack's own spot 0.93 low, inside the
turned slope, leaving its ground on the copy. `worldcopy_ab` draws retail's
lift inside the landing (a second pass carries a lift a mover's step corrected
after the frame); ours' own spot is then clear on that frame as retail's was:
the pack drops, lands again at 16050 with 0.86 and leaves the slope on
retail's frame. Every row of `-worldcopy3-uo.txt` matches with no gap, as do
1.5b's and 1.1d's.

### The turned centre

VERIFIED, `cod_lnxded` `CM_TransformedBoxTrace` (0x80568d8): before turning
anything it takes the box's centre `(mins + maxs) * 0.5` (0.5 at `.rodata`
0x80cd440, loaded 0x80568fc), the half extents about it, and start and end
each plus that centre (loop 0x8056940..0x8056974), less the model's origin
(0x805698b on). The turn (`AngleVectors` at 0x8056a6f, `VectorInverse` at
0x8056a78) applies to those points, and `CM_BoxTrace` (called 0x8056c2d) runs
on the half extents about the turned centre. INFERRED: in the copy's frame the
box, and the capsule its terrain test builds, stand about the turned centre
(35 up a standing player), not the turned origin. On a pitch or roll the
capsule moves by `35 * (R^T up - up)` against a body turned rigidly about its
feet: about 2 units along the copy's x and 0.06 toward its terrain at 3.5
degrees of pitch. A yaw leaves `R^T up` as `up`.

VERIFIED, the `-worldcopy3` capture: from the rest before the pitch the player
is carried to the ten-thousandth for 14 frames, and the carried spot of frame
15 (3.5 degrees) is blocked: jitter (-4, +4), as in `-worldcopy2`. VERIFIED,
vcod measurement in the `-worldcopy2` replay: with the centre turned, the spot
test against the copy's terrain triangle 1603 leaves a margin falling from
0.059 at the first pitch frame to 0.011 at frame 14, 0.002 at 15, none at 16;
turning the origin, the margin holds 0.059 every frame and the push carries
on. VERIFIED, vcod measurement in the `-worldcopy3` replay from retail's rest:
ours is blocked at frame 15 and jitters (-4, +4).

vcod: `CollisionWorld::clip_copy` turns `start + mid` and `end + mid` into the
copy's frame and traces from there less `mid` (`mid` the box's centre); every
trace against a copy goes through it, pmove's and the spot test's alike.
`worldcopy_ab.rs`'s `a_pitched_copy_blocks_its_rider_like_retail` resyncs
ours' player to the `PROBE g` rest before the pitch and holds the carry to
0.0003 and the block to its frame. The jitter's fall that follows is the slide
above.

VERIFIED, vcod measurement against the crush capture: `G_MoverPush`'s
candidate test against a world copy is a box against its brushes, as against a
submodel (section 12). With a capsule ours listed the player under the sloped
ceiling two frames late, where a box corner meets the slope 5 units lower than
the capsule's dome.

### `func_static`

VERIFIED, `SP_func_static` (game.mp 0x5820c), after `InitMover`: stores
`Use_Static` as `use` (`+0x210`), copies `r.currentOrigin` into
`s.pos.trBase`, unlinks under spawnflags 1. A non-zero `health` key, read
unless bit 4 of the byte at `+0x17c` is set, sets `takedamage` (`+0x171`).
Spawnflags 2 or 4 store `Static_Pain` as `pain` (`+0x214`), scale `+0x274` by
a constant, set `takedamage`, write `health` 9999 (`+0x230`) and default
`+0x250` to 4. `Use_Static` (0x58070) unlinks a linked entity and links an
unlinked one.

VERIFIED, `G_Damage` (0x49dac): an entity with no client and `s.eType` 5 takes
its own arm: with a `use` and the byte at `+0x174` zero it calls
`Scr_AddEntity(attacker)`, `Scr_Notify(ent, scr_const+0x92, 1)` (`"trigger"`)
and the `use`, and returns, touching no `health`, `pain` or `die`. INFERRED:
`+0x174` is `moverState`, which `InitMover` zeroes; Quake III's "shootable
doors" rule.

VERIFIED, the capture:

- `health` reads 0, 9999 and 9999 for spawnflags 0, 2 and 4.
- An `origin` write moves the second static's copy at once: a trace 64 west of
  the player lands on it at z 40.12, naming it.
- A `radiusDamage` in the sky over the world's middle, inside every copy's
  link box: neither health moves; the two statics with `takedamage` are
  notified `trigger` with the world (1022), after the caller moved on, the
  second spawned first; the moved one's copy is gone from the next trace.
- A second blast there and one on the street reach neither, unlinked.

vcod: `spawn_static` gives spawnflags 2 or 4 `health` 9999, and either or a
`health` key `takedamage`, marked on the mover row.
`GameHost::blast_entities` lists a linked one with its brush model's box,
`Blast::entity_damage` measures it by box distance (`r.bmodel`, combat doc
14.1) with its own clip passed through by `CanDamage`'s traces, and
`GameHost::damage_nonclient` takes the `use` arm: `"trigger"` and
`mover::use_static`. `Static_Pain` is never reached.

### The sine crush

VERIFIED, the crush capture (2026-10-12, since replaced): the player stood at
2400 (the `setOrigin` reads z one up, -6.75) is crushed in that frame's entity
pass, the copy at -18.81 pressing a box corner of its sloped ceiling into the
head. The damage callback reads inflictor and attacker the bobbing (299),
damage 99999, `dflags` 0, `MOD_CRUSH`, weapon and hit location `none`, health
100; then the killed callback with the same. The bobbing goes on and drops the
level at 4000 (section 18). The corpse settled at -7.16, then -7.99, 0.12 under
a live player's rest.

VERIFIED, the 2026-10-20 recapture with a `--probe-ride` client
(`crates/server/tests/fixtures/movers/mp_carentan-dm-sinecrush.txt`, its
`CMDS` lines the client's cmds): the same callbacks; the corpse reads -7.78 at
2450 and -7.98 from 2500 on. The cmds in the frame after the death are 2400,
2416 and 2433 (`ct` 2433 at 2450), then 2450, 2468 and 2484.

INFERRED: the sink under the 0.125 pad is the flat-terrain fall of
`cod11-player-clip.md` 8.12. A corpse falls on every cmd and lands again; on a
flat partition a fall whose end stays over the face is not stopped at the pad.
From -7.78 (at 41 down), the 17 ms cmd at 2450 ends under the face and stops on
the pad at -7.875; the 18 ms one at 2468 would end under the face again and
stops where it starts; the 16 ms one at 2484 ends 0.1024 down, over the face,
and falls in full to -7.977, velocity cut back to the move (`-6` after the
snap, the `RIDE` line's `vel`).

VERIFIED, the gsc and the binary, why no landing timer holds the corpse:
`G_TryPushingEntity`'s crush calls `G_Damage` with a null direction and point
(the two `push 0` at `0x5560d`, `0x5560f`, ahead of the call at `0x55617`);
`Scr_PlayerDamage` (`0x5ca18`) passes a null pointer to the callback as
undefined (`cod11-combat.md` 4.4); stock `dm.gsc`'s `Callback_PlayerDamage`
adds `level.iDFLAGS_NO_KNOCKBACK` for an undefined `vDir` before
`finishPlayerDamage`, whose knockback block, the `pm_time` store included, is
skipped on that flag (`test $0x4,%esi` at `0x43952`). With the knockback's
120 ms `pm_time` running, `PM_CheckLadderMove` returns before it drops the
ground (`0x336f6`) and the corpse would stay at -7.78 for those cmds.

vcod: `crate::push::push` returns the players a sine mover could not place
instead of stalling it; `ScriptRuntime::push_mover` hands each `G_Damage`
through `deliver_world_hit` before the end-of-period test, and the mover moves
on. `deliver_world_hit` passes the callback point and direction undefined.
Trap: a defined zero pair makes the gametype keep the knockback, and the corpse
holds at -7.78 for one frame and sinks 0.13 on an 18 ms cmd a frame late.
`a_sine_mover_crushes_what_it_cannot_push_like_retail` replays the capture's
cmds (dead included) and holds every line exact.

## 22. A turned submodel is traced in its own frame

VERIFIED, `cod_lnxded` 0x8090f18 (the per-entity arm of
`SV_ClipMoveToEntities`): an entity with `r.bmodel` (`+0xfc`) takes its inline
model through `CM_InlineModel` (0x804bac8, on `s.index` at `+0x8c`) and passes
`r.currentAngles` (`+0x140`) and `r.currentOrigin` (`+0x134`) to
`CM_TransformedBoxTrace` (0x80568d8); any other takes `CM_TempBoxModel`
(0x804baf8) and the zero angles at 0x80cdfc8. The other per-entity arm,
0x809105c, picks model and angles the same way (`+0xfc`, `+0x8c`, `+0x140`). A
submodel goes through the same turn as a world copy (section 21): the box's
centre into the model's frame, the box axial there, the capsule's sphere offset
turned, the brushes clipped as built.

INFERRED: against a turned submodel a box trace is a box turned with the
model. A 30-wide box beside a face turned 45 degrees reaches 15 from it in
retail's frame; a box kept axial in the world reaches 21.2 with its corner.

Evidence: one capture on retail 1.1d, mp_harbor, one `--probe-team allies`
client, 2026-10-12, retaken 2026-10-20 by a `--probe-ride` client logging its
cmds: `crates/server/tests/fixtures/movers/mp_harbor-dm-turnedslab.txt`, off
`client-probes/probe_turnedslab.gsc`. The `bel` gametype's
`script_brushmodel` `*9`, one brush 4 x 56 x 96 at the world origin, kept from
`_gameobjects` by clearing its `script_gameobjectname`, is moved and turned 45
degrees in one frame so its centre lands on an open floor at (-11000, -7656).
The player is stood 25, 22, 20 and 18 from that centre along the turned face's
normal; at each the slab lifts 2 over 0.1 s and comes back.

VERIFIED, the capture: at every distance the slab rises and falls on schedule
and the player is never pushed or jittered. A box axial in the world would
meet the face from 23.2 in, so `G_MoverPush`'s candidate test (game.mp
0x554ed) lists none of the three nearer stands.

VERIFIED, vcod measurement in the replay: with the brushes turned into the
world and the box axial there, ours lists the player at 22, 20 and 18, jitters
him (+4, +4) and the slab stays down; with the sweep turned into the model's
frame every row matches. The landing height after each `setOrigin` (0.125 to
0.36 over the floor) is where the cmd lengths leave the player within the
ground trace's quarter unit: the 2026-10-12 capture logged no cmds and missed
by up to 0.25; the 2026-10-20 recapture, replayed on its cmds (section 21),
matches every `PROBE g` row to six digits.

vcod: `CollisionWorld::clip_turned_model` takes every trace against a posed
model whose angles are not all zero: `start + mid` and `end + mid` into the
model's frame, a box clipped axial there, a capsule through the brush arm
`clip_brush_capsule` with sphere offset `(f.z, -l.z, u.z) * offset`, the
normal turned back. A pose that only moved keeps the brushes' planes moved into
the world. `worldcopy_ab.rs`'s
`a_turned_submodel_tests_its_neighbour_like_retail` replays the capture.

### A capsule against a pitched or rolled submodel

VERIFIED, `cod_lnxded` `CM_BoxTrace` (0x8056310): with a capsule (sphere flag
at `local_28` non-zero) and a moving trace, each axis of the work struct's
`+0x10c..+0x114` is `|sphere offset[i]| + radius` (the loop at the
`local_1c + 0xc` stores), after the sphere is copied in from
`CM_TransformedBoxTrace`. VERIFIED: the brush arm (0x8054e90) with the capsule
flag (`+0xf4`) tests the six axial sides against `+0x10c..+0x114`
(`param_1 + 0x10c` in its second loop), where the box arm reads the half
extents at `+0x34`. INFERRED: a capsule meets a brush's axial sides as its own
bounding box in the brush's frame: `(r, r, half height)` upright, and with the
sphere offset turned into a pitched or rolled model's frame, the box of the
capsule as it stands in the world.

Evidence: one capture on retail 1.1d, mp_harbor, one `--probe-team allies`
client, 2026-10-12, retaken 2026-10-20 by a `--probe-ride` client logging its
cmds: `crates/server/tests/fixtures/movers/mp_harbor-dm-tiltslab.txt`, off
`client-probes/probe_tiltslab.gsc`. The same slab `*9` is posed over
(-11000, -7656) four ways and the player dropped onto it from z 110:

| Pose | Angles | Shape |
|---|---|---|
| `flat` | (90, 0, 0) | lying, top at 32 |
| `edge` | (0, 0, 90) | on its side, 4-wide top at 57 |
| `slope` | (60, 0, 0) | -x face 30 degrees off level |
| `roll` | (0, 0, -60) | -y face the same |

VERIFIED, the capture: the player rests at z 32.1251 on `flat` and 57.1251 on
`edge` (face plus the ground trace's 0.125); on `slope` and `roll` he slides
13.3 and 5.7 down the face and rests with his lower sphere's centre 15.127 off
it. INFERRED: the capsule meets a turned slab as it meets an upright one.

VERIFIED, vcod measurement in the replay: with the axial sides taking
`(r, r, half height)` in the model's frame whatever the turn, ours sank 20 into
`flat` and `edge` (12.12, 37.13) and the 30-degree faces let the capsule
through to the floor or 20 into the face. With `|sphere| + r` every rest on
`flat` and `edge` matches to 0.0002, and on the tilted faces the lower sphere
sits within 0.003 of retail's distance. The slide down a face is the cmd
split's (section 21): the first capture logged no cmds, and ours slid 10.5 and
5.9 to retail's 13.3 and 5.7, its falls starting about 0.15 s off. The
2026-10-20 recapture logs its cmds.

VERIFIED, a third capture, 2026-10-20, a `--probe-ride` client on port 34321:
`crates/server/tests/fixtures/movers/mp_harbor-dm-tiltslab-ride.txt`. The probe
adds a `PROBE r` line per pose: the slab's `.origin` less the `moveto` target
in ten-thousandths and `.angles` less the `rotateto` target in millionths of a
degree. `flat` and `edge` read all 0. `slope` reads pitch -3662.11 and `roll`
roll +3662.11: `r.currentAngles` is 59.99634 and -59.99634, 60 truncated to
65536ths (10922/65536) by `Reached_ScriptMover`'s normalize (section 12, "The
residual yaw"). 90 is a whole number of 65536ths, so `flat` and `edge` keep it.

VERIFIED: the per-entity clip reads that value live; 0x8090f18 passes
`gentity + 0x140` (`r.currentAngles`) to `CM_TransformedBoxTrace` on each trace
(above), and nothing links the entity after the normalize. INFERRED: every
trace after a rotate ends turns the slab by the truncated angle, not the target
the link saw. The brush sits about 12500 units from the entity's origin in the
model's frame, so 0.00366 degrees move the 30-degree faces by thousandths.
Trap: posing the clip at the link with 60 put every rest on `slope` and `roll`
0.002-0.004 nearer the face than retail's, the slide up to 0.011 further.

VERIFIED, `cod_lnxded` 0x8056a7d..0x8056bc6: `CM_TransformedBoxTrace` turns
the start and end (already moved by the box's centre, less the origin, each
stored as a float) with three dot products that stay on the x87 stack and are
stored once. A product rounded to a float moves a point ten thousand units out
by up to half a thousandth. VERIFIED, vcod measurement: with the clip at
`r.currentAngles` alone, the second capture's `slope` slid 0.0078 short from
its second frame on ground; with the dots kept as well it matched exactly.

vcod: `clip_brush_capsule` takes the axial sides' half extents off the sphere
offset it is handed. `ModelPose::to_local` and `WorldCopy::to_local` keep each
dot unrounded until the store. `mover::run_one` poses a brush model's clip at
the angles `Reached_ScriptMover` writes back. `worldcopy_ab.rs`'s
`a_capsule_lands_on_a_pitched_and_a_rolled_submodel_like_retail` (second
capture) and `a_capsule_lands_on_a_tilted_submodel_like_retail_again` (third,
`PROBE r` lines included) replay each with its cmds: every row matches to the
ten-thousandth, no separate tolerance for the tilted faces.

### A rolled submodel's non-axial side

The slab above has only axial sides, which take `|sphere offset| + radius` and
never read the offset's sign. A non-axial side does: the brush arm
(0x8054e90, 0x805506d..0x8055080) shifts the start by minus or plus the
offset, whichever sphere the side's normal faces, so the capsule meets a side
`radius + |n . offset|` off its centre, `n` the side's normal in the model's
frame. INFERRED: with `(f.z, -l.z, u.z)` that is
`|n . (f.z, -l.z, u.z)| * (half height - radius)`, where an upright capsule
would take `|n . (f.z, l.z, u.z)|`; the two differ only when `n.y` and `l.z`
are both non-zero, so only a roll reaches it. Under a pure roll `rho` the
capsule's axis is the world's up turned by `2 rho` about x.

VERIFIED, a capture on retail 1.1d, mp_depot, one `--probe-ride` client on
port 34621, 2026-10-21:
`crates/server/tests/fixtures/movers/mp_depot-dm-rollramp.txt`, off
`client-probes/probe_rollramp.gsc`. mp_depot's script_brushmodel `*3` (BSP
entity 232, the last script_brushmodel, which no script moves) has a ramp: its
brushes 3, 4 and 5 share the non-axial side `n = (0, -0.19611613, 0.9805807)`,
`d = -38.438763` (bsp lump read). The probe turns and moves the model so the
ramp point `(-1310, y, on the side)` lands at (-2624, -264, 16), over an open
floor at z -24, and drops the player onto it from z 120. He slides a few units
and rests on the ramp in every pose. Rest of his centre off the ramp's plane:

| Pose | Angles | Ramp off level | Truncated angles (`PROBE r`) | Retail rest | Upright capsule would rest |
|---|---|---|---|---|---|
| `down` | (0, 0, -25) | 13.7 | -24.99939 | 31.2416 | 34.5569 |
| `steep` | (0, 0, -40) | 28.7 | -39.99573 | 27.6283 | 32.6703 |
| `turned` | (-10, 90, -30) | 21.1 | (-9.99756, 90, -29.99817) | 29.9203 | 33.7828 |

Retail's rests are `radius + 0.125 + |n . (f.z, -l.z, u.z)| * 20` to the fourth
decimal in all three (hand computation off the `PROBE g` rest rows).

VERIFIED, `cod_lnxded` `AngleVectors` (0x8063388), which
`CM_TransformedBoxTrace` calls at 0x8056a6f and whose right 0x8065c38 negates:
each angle times the double at 0x80cdfe8 (`M_PI * 2 / 360`) is stored as a
float (`fstps -0x1c(%ebp)`); `fsincos` runs on that float and both results are
stored as floats; each component of right and up (`-sr*sp*cy + cr*sy` and the
rest) is summed on the x87 stack and stored once. VERIFIED, the `turned` pose:
with every product rounded to a float, ours landed on the same frame and then
slid 0.0017, 0.0024 and 0.0059 short of retail over the next three rows; with
the engine's rounding every row matches. `down` and `steep` (a roll alone)
matched either way. Trap: `pmove::aim::angles_to_axis` is not the game
module's rounding to borrow here ("The game module's turn" below). VERIFIED,
the normal back into the world (0x8056c4d..0x8056d10) is the turn's
transpose, not Quake III's `CreateRotationMatrix` of the negated angles;
keeping its dots unrounded moved no row.

vcod: `x87::rotation_matrix` builds the turn for a posed submodel
(`CollisionWorld::model_pose`) and a world copy (`set_world_copy`).
`worldcopy_ab.rs`'s
`a_capsule_lands_on_a_rolled_submodels_non_axial_side_like_retail` replays the
capture with its cmds; every row matches to the ten-thousandth. With `+l.z` in
the sphere offset the poses rest 3.2, 6.2 and 3.6 units higher than retail's.

#### On 1.5b and United Offensive

The same recipe ran on retail 1.5b and UO 1.51b on 2026-10-21 (ports 34611
and 34612, a `--probe-ride` client each): `mp_harbor-dm-tiltslab-1.5.txt` and
`mp_harbor-dm-tiltslab-uo.txt` in `crates/server/tests/fixtures/movers/`.

VERIFIED, both captures' `PROBE r` lines match 1.1d's: `flat` and `edge` read
0, `slope` pitch -3662.11, `roll` roll +3662.11. INFERRED: both servers'
`Reached_ScriptMover` truncates `r.currentAngles` to 65536ths as 1.1d's does.
The addresses in `cod_lnxded` 1.5b and `coduo_lnxded` were not looked up.

VERIFIED, both captures:

| Pose | 1.5b | UO | 1.1d |
|---|---|---|---|
| `flat` rest z | 32.1251 | 32.1254 | 32.1251 |
| `edge` rest z | 57.1251 | 57.1249 | 57.1251 |
| `slope` | stays at x -15, z 23.1204 | z 23.3404 | slides 10.1 down the face |
| `roll` | slides 5.6 to y -25.5962, z 70.0223 | -25.5879, 70.0272 | |

VERIFIED, vcod measurement: `worldcopy_ab.rs`'s
`a_capsule_lands_on_a_tilted_submodel_like_retail_on_1_5` and `_on_uo` replay
each capture with its cmds on the same clip as 1.1. Every row matches to the
ten-thousandth and the `PROBE r` lines exactly. When `ModelPose::to_local`
rounds each product to a float, 37 rows of each fail. VERIFIED, the
disassembly: the 1.5b and UO builds keep `CM_TransformedBoxTrace`'s three
products on the x87 stack until the store, as 1.1d's does, though they store
every float variable (`cod15-movement.md` 7): `RotatePoint` (UO
`coduo_lnxded` 0x8057b18, 1.5b 0x8057ce4) sums each dot on the stack and stores
it once.

**The turn's axis.** VERIFIED, the disassembly: `CM_TransformedBoxTrace`
(1.5b 0x805b366, UO 0x805b19a) builds its matrix through
`CreateRotationMatrix` (1.5b 0x8057df6, UO 0x8057c2a: `AngleVectors`, then
`VectorInverse` on the right), and that `AngleVectors` (1.5b 0x806759d, UO
0x80674ad) rounds as 1.1d's 0x8063388, instruction for instruction: each angle
times the double at 1.5b 0x80d7fb8 / UO 0x80dd7f8 (bytes
`39 9d 52 a2 46 df 91 3f`, the double both `M_PI * 2 / 360` and `M_PI / 180`
give) stored as a float; `fsincos` on it with both results stored (1.5b
0x806b570, UO 0x806b4de); the right's -1.0 a float constant (1.5b 0x80d7fc0,
UO 0x80dd800); each component's products and sum on the stack, stored once.
`x87::angle_vectors` is that arithmetic on all three servers.

**The game module's turn.** VERIFIED, the disassembly: the game modules'
`AngleVectors` round as the engine's.

| Module | `AngleVectors` | Radians stored | sin/cos stored | Components stored once |
|---|---|---|---|---|
| 1.1d `game.mp.i386.so` | 0x3b228 (double at 0x72930) | `fstps` 0x3b250, 0x3b268, 0x3b2a9 | yes | right 0x3b2ed..0x3b2f5, up 0x3b336..0x3b33e |
| 1.5b (unoptimized) | 0x33e18 | 0x33e3a | helper 0x381a6 | 0x33f1f, 0x33f48, 0x33f7a, 0x33f9b |
| UO | 0x3b068 | `cod11-combat.md` 2.2 | | |

Two wrappers build an axis from it:

- `AnglesToAxis` (game.mp 0x3ef3c, 1.5b 0x36ffa, UO 0x3e24a; engine UO
  0x806a46e) writes the left as `vec3_origin - right` (`fld` of
  `vec3_origin`, `fsub` of the right, at 0x3ef58..0x3ef79; 1.5b
  0x3703b..0x37065; UO engine 0x806a49d..0x806a4c8), so a zero there is
  always `+0.0`.
- `G_CreateRotationMatrix` (game.mp 0x579fc, 1.5b 0x55943, UO 0x60487) and
  1.1d's `G_TryPushingEntity`, which inlines it (0x5498c `AngleVectors`,
  0x54995 `VectorInverse`), negate the right in place, as
  `CM_TransformedBoxTrace` does.

INFERRED: the two differ only in a zero's sign. VERIFIED, `G_SetFixedLink`
(game.mp 0x66540) turns a mode 2 link's offset by `AnglesToAxis` of the
parent's angles (0x66577), and `StopFollowing` (0x46a28) calls `AngleVectors`
(0x46a84).

vcod: `x87::angle_vectors`, `x87::angles_to_axis` (the `vec3_origin - right`
left) and `x87::rotation_matrix` (the negated right) are the only server-side
turn: a posed submodel and a world copy, `push::carried`, a linked client's
offset (`run_client_link`, the `linkTo` offset builtin),
`link::Frame::from_pose`, the turret muzzle and rest sweep, the stored body
frame, the damage kick, the follow stop spot and the UO vehicle code. The
game-module callers (`pmove::aim::angles_to_axis`, `spawn::angles_to_axis`)
take the stored rounding too. VERIFIED, vcod measurement: rounding every
product to a float instead moves no row of `movers_ab`, `push_ab`,
`push_candidate_ab`, `push_order_ab`, `worldcopy_ab`, `ride_ab`, `sine_ab`,
`linkto_ab`, `turret_ab`, `scriptmodel_ab` or the vehicle gates at the 1.1 and
UO installs. pmove's own turns are below.

**pmove's turns.** VERIFIED, the relocations against `AngleVectors` in 1.1d
`game.mp.i386.so` and the PLT calls in 1.5b's and UO's (0x13b00, 0x175b4): the
player move turns through the game module's `AngleVectors` in:

| Caller | 1.1d | 1.5b | UO | Angles, vectors |
|---|---|---|---|---|
| `PmoveSingle` | 0x34120 | 0x284ec | 0x2dbdd | `ps.viewangles`; forward, right, up |
| `BG_CheckProneValid` | 0x2d5e0 | 0x1d1d4 | 0x21955 | `(0, yaw - 180, 0)`; forward |
| `AddLeanToPosition` | 0x6bae5 | 0x71ae8 | 0x946f4 | `(0, yaw, f * rollscale)`; right |
| `PM_UpdateAimDownSightFlag` | | | 0x32e3c | `(0, viewangles[YAW], 0)`; forward, the LMG mount probe 15 ahead and 1 up |

VERIFIED, 1.1d's `PM_UpdateViewAngles` call (0x32fc5) sits behind
`g_debugProneCheck`'s `integer` (0x32f94, the relocation names the cvar) and
only feeds `G_DebugLine`. INFERRED: 1.5b's (0x26459) and UO's (0x2b75e) are the
same, behind the same test of a cvar's `+0xc` (0x26401, 0x2b703). VERIFIED,
`ClientThink_real`'s aim block builds both axes through `AnglesToAxis`
(0x4037e, 0x40394), and 1.5b's `CalcMuzzlePoint` calls `AngleVectors`
(0x6ea7b). INFERRED: every one rounds as the engine's, since each calls its
module's one `AngleVectors`.

VERIFIED, `AddLeanToPosition` 1.1d 0x6ba9c: `f = (2 - |leanf|) * leanf` kept on
the stack (2.0 at 0x7a344) and saved as a ten-byte value (`fstp TBYTE`
0x6bae2), the roll `f * rollscale` stored as a float (0x6bad6), then each
`pos[i] + right[i] * (f * dist)` summed on the stack and stored once
(0x6baea..0x6bb0a). 1.5b 0x71a7c and UO 0x94688 store `GetLeanFraction`'s
result (0x71ab2, 0x946be; 1.5b's `GetLeanFraction` 0x71a17 returns
`(2 - |f|) * f` off the stack), the roll (0x71ac8, 0x946d4) and `f * dist`
(0x71af3, 0x946ff), then sum each component on the stack and store it once.

VERIFIED, the Windows cgames' `AngleVectors` (1.1 `cgame_mp_x86.dll`
0x3003a790, 1.5 `cgame_mp15.dll` 0x3003bea0, UO `uo_cgame_mp_x86.dll`
0x3004a200; fastcall, angles in `edx`, forward, right, up in `esi`, `edi`,
`ebx`) are one body:

- Each angle times the float at 0x30069390 / 0x3006b608 / 0x3007bd70 (bytes
  `35 fa 8e 3c`, `pi / 180` rounded to a float once, not the servers' double),
  stored as a float; `fsincos` on it, both results stored.
- Forward `cp * cy`, `cp * sy`, `-sp`, each one product.
- `sr * sp` stored as a float (0x3003a865) before right's `cr * sy - t * cy`,
  `cr * cy * -1 - t * sy`, `sr * cp * -1`.
- `cr * sp` stored (0x3003a8b5) before up's `t * cy + sr * sy`,
  `t * sy - sr * cy`, `cr * cp`.
- Each remaining sum kept at the CRT's 53 bits until its one store.

VERIFIED, the 1.1 cgame's `PmoveSingle` (0x3000c440) and `BG_CheckProneValid`
(0x300062a0, mask 0x810011) call it (0x3000c718, 0x3000648c). INFERRED from
the call order: 1.5's 0x3000d4ad and 0x3000699f and UO's 0x3000e39e and
0x30007074 are the same two calls. VERIFIED, the cgames' `AddLeanToPosition`
(1.1 0x3003f4e0, 1.5 0x30040df0) inlines that body: the fraction
`(2 - |leanf|) * leanf` stored (0x3003f523), the roll and radians stored,
`sr * sp` stored, then `f * dist` stored (0x3003f611) and each
`right[i] * d + pos[i]` stored once.

INFERRED: on a shorts-grid angle the server's radians and the cgame's part in
the last bit on 6052 of the 65536 shorts
(`x87::the_windows_radians_are_not_the_servers`); with no roll the two builds'
products then agree. A roll (the lean's, the gun's tilt in the aim block) also
parts them by the stored `sr * sp`.

vcod: `x87::angle_vectors_of_radians` takes the sim's radians (the Linux
builds' own, `cmd::apply_view`), `x87::angle_vectors_msvc` is the cgames'
body, and `x87::angle_vectors_in` picks by `Arith`. `pmove::view_vectors`
(`PmoveSingle`'s, through the fly, walk and ladder moves), `check_prone`, the
UO mount probe and `aim::add_lean` turn by the move world's `Arith`, so the
client predictor (`Arith::Msvc`) takes the cgame's rounding;
`PlayerState::view` and `combat::muzzle_point_in` take the rules' server.
`aim::angles_to_axis` (the aim block) and `aim::add_muzzle_bob` are the
servers'; the client draws through `aim::angles_to_axis_msvc` and
`aim::eye_bob`, the cgame's.

VERIFIED, vcod measurement at the 1.1 and UO installs, against float products,
Rust's `to_radians` and float sines of float radians (the lean, the prone
sweep, the mount probe): of `predict_ab`, `moves_ab`, `moveshot_ab`,
`legs_ab`, `turret_ab`, `leanhit_ab` and the fourteen `vehicle*_ab` gates
only `leanhit_ab`'s round directions moved. Its leaning rounds (`t_left`,
`t_crouch_left`, `t_prone_left`, `t_back`) sat 4 to 9 ten-millionths off
retail's print on one axis under the float turn and match to the print with
this one, so `DIR_NEAR` is 3e-7 (from 2e-5); at 3e-7 the float turn misses 7
rounds at the 1.1 install and 8 at UO. `nav_census` printed the same lines at
both installs. The client's `the_windows_build_draws_no_new_correction`
counts did not move (1.5 moves 44, turns 14, UO autofire 0, sprint 126 with
98 past 0.1, 1.1 moves 4, turns 3, springfield 0). INFERRED: the slope
captures neither lean nor lie down, so of the turns above only
`PmoveSingle`'s reaches them, and its radians part from the server's on few of
their views.

**The rolled ramp.** The `probe_rollramp` recipe ran on retail 1.5b and UO
1.51b on 2026-10-21 (ports 34661 and 34662, a `--probe-ride` client each,
`COD_DIR` the UO install whose `main/` is 1.5's):
`mp_depot-dm-rollramp-1.5.txt` and `mp_depot-dm-rollramp-uo.txt`. VERIFIED,
both captures' `PROBE r` lines match 1.1d's. VERIFIED, the rests (how far each
slides is its cmd split's):

| Pose | 1.5b | UO |
|---|---|---|
| `down` | (0, -2.32526, 13.7258) | (0, -2.63382, 13.8009) |
| `steep` | (0, 4.6915, 9.96) | (0, 4.71423, 9.94756) |
| `turned` | (-1.24976, -0.795105, 12.5732) | (-0.898438, -0.59256, 12.7295) |

**The end point of a trace an entity cut short.** VERIFIED, vcod measurement:
without the rule below ours missed 63 rows of the 1.5b capture (`down` from
its third frame on ground, 0.0043 short, and `turned`, 0.005) and 31 of UO's
(`steep`, 0.014). VERIFIED, gdb on two more 1.5b runs of the probe
(2026-10-21, ports 34663 and 34664; dprintfs on `PmoveSingle`,
`PM_ClipVelocity` and the return of 0x805b366): every `CM_TransformedBoxTrace`
returned ours' fraction and normal to the bit, but the origin after the
landing cmd sat a few ulps off retail's, and a few cmds later one of ours'
slide traces met the ramp again where retail's ran clear, so ours nudged the
velocity off the face (`PM_SlideMove`'s same-plane nudge) and slid
0.004-0.011 further.

VERIFIED, `SV_Trace` (1.5b 0x8099fd1, UO the same body at 0x809b445 on, 1.1d
0x80916f4): the world trace runs first (`CM_BoxTrace` on the trace's own
start, end and bounds), then the entities' clip on the box centred
(`start + mid`, `end + mid`, the half extents either side). When the entities'
fraction is below the world's (1.5b 0x809a44d `ja`, 1.1d 0x80919f9 `fcoms`
with `ah & 0x45 == 1`), the end point is set again off the unshifted points,
`start + (end - start) * fraction`, each component rounded once: each delta
stored as a float on 1.5b (0x809a469, 0x809a47c, 0x809a48f) and UO (0x809b461
on), kept on the stack on 1.1d (`fsub` then `fsts`, the stack copy used,
0x8091a10..0x8091a28). The world trace's end point is `CM_BoxTrace`'s
(`collision::cm_endpos`), whose delta runs between the shifted points.

vcod: `CollisionWorld::trace` takes `sv_trace_endpos` when a posed model, a
world copy or a model instance lowered the world's fraction. VERIFIED, vcod
measurement: with it every row of both new captures and both gdb runs
matches, and the 1.1d gates in `worldcopy_ab.rs` still match.
`worldcopy_ab.rs`'s
`a_capsule_lands_on_a_rolled_submodels_non_axial_side_like_retail_on_1_5` and
`_on_uo` replay the captures.

## 23. Mover script reads: `.speed`, `.dmg`, `movegravity`, `.angles`

Evidence: one capture on retail 1.1d, mp_carentan, no client, 2026-10-20:
`crates/server/tests/fixtures/movers/mp_carentan-dm-moverreads.txt`, off
`client-probes/probe_moverreads.gsc`. `ride_ab.rs`'s
`mover_script_reads_match_retail` replays it and holds every line exact.

**`.speed` and `.dmg` after a spawn.** VERIFIED, the capture:

| Entity | `.speed` | `.dmg` |
|---|---|---|
| script-spawned `func_bobbing` | 4 | 2 |
| `func_pendulum` (speed key goes to a local, so `InitMover` finds `ent->speed` 0 and makes it 100, section 18) | 100 | 2 |
| `script_model` | 0 | 0 |

**`movegravity` on a moving mover.** VERIFIED, the capture: called on a
`movez(100, 1)` on the frame whose `.origin` read z 85, it starts from 85
(`getorigin` reads 85 straight after and a frame on), not the 90 the running
move had reached at `level.time` as `moveto` would (section 12, "A blocked
mover"). VERIFIED, game.mp 0x60667..0x6067f: the verb loads `r.currentOrigin`
into `trBase` without evaluating the old trajectory, then evaluates the new
arc into `r.currentOrigin` (0x606a9) and links (0x606b2). A second
`movegravity` on a running arc starts the same way.

**`r.currentOrigin` while a mover moves.** VERIFIED, the capture: the second
arc crossed z 0 at 2400 and read -9.53674e-06, where the arc evaluates to
-9.89437e-06 (base 63.9999962, `400 * dt * dt` on the x87 stack). VERIFIED,
game.mp: `G_MoverTeam` (0x65684) stores `move` as the trajectory at
`level.time` less `r.currentOrigin`, a float per component, and `G_MoverPush`
(0x650f0) adds it back (`r.currentOrigin += move`), so a frame's origin is
`prev + (eval - prev)` with two float roundings, as `amove` and
`r.currentAngles` (section 12, "The residual yaw"). On the frame a segment
ends, `Reached_ScriptMover` evaluates the new trajectory straight into
`r.currentOrigin` (0x5fa0e) and `r.currentAngles` (0x5fab1). VERIFIED: the
gravity arm of `BG_EvaluateTrajectory` (0x2c6c0) keeps
`base + delta * dt - 400 * dt * dt` on the stack and rounds once.

**`.angles` mid-rotate.** VERIFIED, the capture: raw while a rotate runs,
negative below 0 and past 360 above it (`rotateyaw(500, 1)` from 270 read up to
745 before settling at 49.9988), then `Reached_ScriptMover`'s normalize
(section 12). A `rotatevelocity((-40, -120, 0), 1)` likewise.

**`rotateto` takes the near way round.** VERIFIED, the capture: from
(-69.9994, 290.001, -39.9957), `rotateto((10, -170, -20), 1)` turned yaw down
100 to 190, not 460 to -170. VERIFIED, game.mp 0x60ad0..0x60afb: each target
component is `AngleSubtract(target, r.currentAngles) + r.currentAngles` on the
x87 stack. The axis verbs and `rotatevelocity` take their delta as given.

**`rotatevelocity`'s ramps are optional.** VERIFIED, the capture (a
two-argument call ran) and game.mp 0x60e4d: accel and decel are read only when
there are more than two and three arguments, as on every other verb.

vcod: `builtins::mover::rotate_to` folds the target; `rotate_velocity` takes
two arguments; `mover::run_one` writes a moving group's `prev + (eval - prev)`
unless its segment ended that frame; the plan's `finish` stops a
`movegravity` at the frame's evaluation; `bg_evaluate` takes a gravity arc
through `Trajectory::evaluate_kept` (`evaluate_stored` on 1.5 and UO).

**1.5b and UO.** VERIFIED, two captures off the same probe on mp_carentan,
2026-10-20: `mp_carentan-dm-moverreads-1.5.txt` (retail 1.5b) and
`mp_carentan-dm-moverreads-uo.txt` (retail UO 1.51b) are identical line for
line to each other, and to 1.1d's but for one row: the second arc's z at 2400
reads -5.72205e-06 where 1.1d's reads -9.53674e-06. Every `.speed` and `.dmg`
read, the `movegravity` start off `r.currentOrigin`, the gravity stop, the raw
angles mid-rotate, the normalize and the `rotateto` fold read as on 1.1d.
INFERRED: the one row is the stored `dt` and stored `base + delta * dt` of
1.5b's and UO's `BG_EvaluateTrajectory` (1.5b `.so` 0x1b877, UO 0x1fe63,
`Trajectory::evaluate_stored`), the arc's evaluation feeding `move` in
`G_MoverTeam`; with 1.1d's `evaluate_kept` ours reads 1.1d's value there.
`ride_ab.rs`'s `mover_script_reads_match_retail_on_1_5` and `_on_uo` replay
both, every line exact.
