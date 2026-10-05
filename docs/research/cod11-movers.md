# CoD 1.1 MP: the scriptent mover verbs

What `moveto`, `movex`/`movey`/`movez`, `movegravity`, `rotateto`,
`rotatepitch`/`rotateyaw`/`rotateroll` and `rotatevelocity` do, in what units,
and what they put on the wire.

The evidence is one paired capture against the retail 1.1d Linux dedicated
server, mp_pavlov under `dm`, 2026-09-09:

- `crates/server/tests/fixtures/movers/mp_pavlov-dm-movers.txt`, the server
  half: `getorigin()` and `.angles` once per server frame through each verb,
  logged by `crates/gsc/tests/fixtures/semantics/client-probes/probe_mover.gsc`
  under `tools/run_probe.sh`;
- `crates/server/tests/fixtures/movers/mp_pavlov-dm-movers-wire.txt`, the
  client half: every change to an entity's `pos`/`apos` trajectory group, from
  a `--net-probe` attached to that same server.

The verb-to-handler mapping is the `scriptent` method table at
`game.mp.i386.so` `0x78d40`, walked by `ScriptEnt_GetMethod` (`0x60f60`), and
is in `docs/research/cod11-gsc-object-model.md` section 9. The trajectory
enumeration is `docs/protocol-1.1.md`, divergence 8.

## 1. Which entities accept a mover verb

Only three classnames. VERIFIED: a `movez` on a `mpweapon_panzerfaust` is the
fatal

```
entity 258 is not a script_brushmodel, script_model, or script_origin
```

so the mover verbs are gated on the entity's spawn class and the gate is a
script runtime error, not a silent no-op. That cost the probe one run.

## 2. The time argument is seconds

VERIFIED. `moveto((600,0,100), 1)` from `(100,0,100)` took 1000 ms of level
clock and `moveto((600,0,100), 2)` took 2000, measured between the frame the
origin first moved and the frame `movedone` fired. A frame or millisecond
reading of the same argument would have finished inside one frame.

## 3. The axis verbs are deltas; the target verbs are absolute

VERIFIED. `movex(500, 2)` on an entity at `x = 100` ended at `x = 600`, and
`moveto((600,0,100), 2)` from the same start ended at the same place, so one
argument is a displacement and the other a destination.

VERIFIED for the angular half too, and it needed a second call to see: a
single `rotateyaw(90, 2)` on an entity at yaw 0 is ambiguous, so the probe
calls it twice. The second call ran the yaw from 90 to 180, so
`rotatepitch`/`rotateyaw`/`rotateroll` are deltas and `rotateto` takes an
absolute angle set.

## 4. The motion law: a trapezoidal velocity profile

`moveto`/`movex`/`movey`/`movez` and `rotateto`/`rotatepitch`/`rotateyaw`/
`rotateroll` take an optional `accel` and `decel` after the duration, both in
**seconds of ramp**, and both default to 0.

VERIFIED from the per-frame origins. For a move of distance `D` over `T`
seconds with ramps `ta` and `td`, the cruise speed is

```
v = D / (T - ta/2 - td/2)
```

the first `ta` seconds are constant acceleration from 0 to `v`, the last `td`
seconds constant deceleration from `v` to 0, and the middle is `v`. Three
measured cases agree to the sampled digit:

| verb | D | T | ta | td | v predicted | v measured |
|---|---|---|---|---|---|---|
| `moveto` | 500 | 1 | 0 | 0 | 500 u/s | 500 u/s (25.00 per 50 ms frame) |
| `moveto` | 500 | 2 | 0 | 0 | 250 u/s | 250 u/s (12.50 per frame) |
| `moveto` | 500 | 2 | 0.5 | 0.5 | 333.33 u/s | 333.33 u/s (16.67 per frame) |
| `moveto` | 500 | 2 | 0.5 | 0 | 285.71 u/s | 285.71 u/s (14.29 per frame) |
| `rotateto` | 90 deg | 2 | 0.5 | 0.5 | 60 deg/s | 60 deg/s (3.00 per frame) |

The accel-only row is what says the two ends are independent rather than one
symmetric ease. The ramp itself is exact constant acceleration: with
`ta = 0.5` and `v = 333.33` the displacement at 50 ms is 0.83 units, which is
`½ (v/ta) t²` to the sampled digit, not a smoothstep.

With no ramps the motion is linear, and the entity is at its destination on
the frame `movedone` fires.

## 5. `movegravity`

VERIFIED. `movegravity((0,0,300), 3)` from `z = 100` traced
`z(t) = 100 + 300t - 400t²` exactly, peaking at 156.25 at t = 0.375 and
reading -2600.00 at t = 3. So the vector is an initial velocity in units per
second and gravity is 800 u/s², the `DEFAULT_GRAVITY` the trajectory code
already carries (`crates/common/src/net/trajectory.rs`).

The time argument is when `movedone` fires, not when the motion stops: the
entity was still accelerating downward on the frame the notify came. Whether
it stops there or keeps falling is UNVERIFIED; the probe's sampler ran out on
the same frame.

## 6. `rotatevelocity`

VERIFIED. `rotatevelocity((0,180,0), 2, 0, 0)` turned the yaw at 9 degrees per
50 ms frame, 180 deg/s, for exactly 2 seconds, ending back at 0 after a full
turn, and raised `rotatedone`. So the vector is an angular velocity in degrees
per second and the second argument is a duration; the accel and decel
arguments are the same pair section 4 measures.

## 7. The completion notifies

VERIFIED. The four linear families and `movegravity` raise `"movedone"`; the
four angular families and `rotatevelocity` raise `"rotatedone"`. Each fires on
the frame the duration elapses, on the entity itself.

## 8. Script-visible state while a mover runs

VERIFIED. `getorigin()` returns the interpolated position on every frame of a
move, and `.angles` reads the interpolated angles on every frame of a rotate
— the field is written back, not left at the value it had when the verb was
called. `.angles` is normalized: after `rotatevelocity` turned a full 360 it
read `(0, 0, 0)` while the wire carried 360 (section 9).

Whether `.origin` is written back the same way is UNVERIFIED; the probe read
`getorigin()`.

**Script sees the trajectory one server frame late.** VERIFIED, from both
halves of the capture at once. `moveto((600,0,100), 1)` called at level time
1050 put `trTime` 1050 on the wire, and yet `getorigin()` still read the start
origin at 1100 and the first moved value, 125, at 1150; `movedone` came at
2100 rather than at 1050 + 1000. Every phase agrees: `rotateyaw(90, 2)` called
at 13450 raised `rotatedone` at 15500. So the script-visible origin and angles
at level time `T` are the trajectory evaluated at `T - 50`, one frame at the
default `sv_fps` 20, and the completion notify comes on the first frame whose
lagged clock has passed the end. Whether the lag is one frame or a fixed 50 ms
is UNVERIFIED; the capture ran at one frame rate.

## 9. What goes on the wire

VERIFIED, and this is the load-bearing one: retail sends **a trajectory the
client extrapolates**, not per-frame origins. A two-second move is two
snapshot updates, one at each end, and nothing in between, whatever the frame
rate.

Per verb, with the `trType_t` enumeration from
`crates/common/src/net/trajectory.rs`:

| verb | group | at the call | at the end |
|---|---|---|---|
| `moveto`/`movex`/`movey`/`movez`, no ramps | `pos` | `trType` 3 (`TR_LINEAR_STOP`), `trTime` = the level time of the call, `trDuration` = the duration in ms, `trBase` = the start origin, `trDelta` = the velocity in units/s | `trType` 0, `trBase` = the end origin, `trTime` = the completion time. `trDelta` and `trDuration` are left stale |
| the same with ramps | `pos` | three consecutive trajectories, one per segment: `trType` 9 (`TR_ACCELERATE`) for `trDuration` = `ta` ms, then `trType` 3 for the cruise, then `trType` 10 (`TR_DECCELERATE`) for `td` ms. Every one carries `trDelta` = the cruise velocity `v` and a `trBase` at that segment's own start | `trType` 0 at the end origin |
| `movegravity` | `pos` | `trType` 5 (`TR_GRAVITY`), `trDuration` = the duration in ms, `trDelta` = the initial velocity | not observed; the probe deleted the entity first |
| `rotateto`/`rotatepitch`/`rotateyaw`/`rotateroll`/`rotatevelocity` | `apos` | `trType` 3, `trDelta` = the angular velocity in deg/s | `trType` 0 with `trBase` = the angle reached, unnormalized: a full turn read `(0, 360, 0)` |

The measured example: `movez(96, 2, 0.5, 0.5)` from `z = 592` sent
`trType 9, trDuration 500, base z 592, delta z 64`, then
`trType 3, trDuration 1000, base z 608, delta z 64`, then
`trType 10, trDuration 500, base z 672, delta z 64`, then
`trType 0, base z 688`. The three segments cover 16, 64 and 16 units, and 64
is the cruise speed section 4's formula gives for `96 / (2 - 0.25 - 0.25)`.

A mover is an `eType` 8 (`ET_SCRIPTMOVER`) entity throughout; nothing about
the entity kind changes when it starts or stops moving.

## 10. What is not measured here

- What `movegravity` leaves the entity doing after its `movedone`.
- Whether `.origin` is written back per frame.
- A rotation of more than one axis at once, and a brush model with an
  origin brush, so a pivot other than the world origin.

A second verb on an entity already moving was measured in section 12: it
replaces the trajectory.

## 11. A brush model mover's clip follows it

The evidence for sections 11 to 13 is a second paired capture against the
retail 1.1d Linux server, mp_carentan under a `dm`-shaped probe, 2026-10-05:
`crates/server/tests/fixtures/movers/mp_carentan-dm-ride.txt` (the server
half, one `PROBE f` line per server frame of a phase, from
`crates/gsc/tests/fixtures/semantics/client-probes/probe_ride.gsc`) and
`mp_carentan-dm-ride-wire.txt` (the client half, the `--probe-ride`
playerstate per snapshot and every trajectory change). The probe renames the
two bombzone brush models' `script_gameobjectname` to `dm` so `_gameobjects`
keeps them, stands the player on brush 4281 of model `*5` (entity 177, a
plank slab whose top is z -22) and runs one verb per phase on the entity.

VERIFIED, off the capture: a script_brushmodel with no `origin`, `angles` or
`model` key reads `getorigin()` and `.angles` `(0, 0, 0)` and `.model` `""`
(`PROBE bm 177`). Its brushes are in model space, so the entity's origin is
the pivot of a rotate, here the world origin.

VERIFIED, off the capture: the brushes move with the entity. A player
standing on the slab rides `movez(48, 2)` up and back down, `movex(48, 2)`
across and back, and is pushed out of its way when it moves sideways into
him (section 12). INFERRED: the server's clip of the inline model is placed
at the entity's `r.currentOrigin` and `r.currentAngles` every trace, which is
what `G_MoverPush` writes (section 12) and the only state the push and the
traces share.

VERIFIED, `G_RunFrame` (game.mp `0x50478`): the entity loop at
`0x50912`-`0x50975` calls the per-entity runner `0x502bc` for an in-use
entity (byte `ent+0x160`), and before it, at `0x50949`, for the entity the
first dword of the link record `ent+0x2e4` points at. The runner calls
`G_RunMissile` for `eType` 4 (`0x50375`), `G_RunItem` when byte `ent+0x161`
is set (`0x503f5`), `G_RunMover` for `eType` 5 or 8 (`0x5040e`) and
`G_RunClient` for an entity with a client (`0x50422`). INFERRED, off those
branches: a linked entity's parent runs ahead of it in the same frame, and
`ent+0x194` against the level time (`0x502cb`) keeps either from running
twice.

VERIFIED, `G_RunMover` (`0x5760c`): it calls `G_GeneralLink` when the entity
has a link record (`0x57623`), `G_MoverTeam` when `pos.trType` (`ent+0xc`) or
`apos.trType` (`ent+0x30`) is non-zero (`0x57676`), and `G_RunThink`
(`0x57682`). VERIFIED, `G_MoverTeam` (`0x55684`): it evaluates `pos` and
`apos` at `level.time` (`0x556c1`, `0x556d7`) and hands the push function
`0x550f0` the differences from `r.currentOrigin` (`ent+0x134`) and
`r.currentAngles` (`ent+0x140`) as `move` and `amove`. VERIFIED, off the
`0x50653` and `0x506d0` calls of `Scr_RunCurrentThreads` and the entity loop
at `0x50912`: the script threads run before the entity pass inside
`G_RunFrame`. INFERRED: the clip is on the trajectory at the level time, one
frame ahead of the `getorigin()` section 8 measured, and that is the clock
the wire's playerstate carries: `movez(48, 2)` called at 11250 put the rider
at z -20.675 on the 11300 snapshot (one frame of 24 u/s) while script read
the slab's z 0 at 11300.

vcod: `CollisionWorld::set_model_pose` places a submodel's brushes by an
origin and angles; traces skip a posed model in the BVH and clip its moved
planes after it. `crate::game::mover` poses a mover's model at the level
time each frame.

## 12. `G_MoverPush`: riders, pushes and a blocked mover

VERIFIED, the push function `0x550f0` (`G_MoverPush` by its shape; the module
has no symbol for it): it bounds the move by `r.absmin`/`r.absmax` plus
`move` when the pusher's angles and `amove` are all zero, by
`RadiusFromBounds` (`0x5518c`) about the origin otherwise, unlinks the pusher,
calls `trap_EntitiesInBox` with mask `0x2000180` (`0x5533c`), adds `move` and
`amove` to `r.currentOrigin` and `r.currentAngles` and relinks the pusher
(`0x553ae`). For each listed entity it tests `eType` 1, 3 or 4 and byte
`ent+0x161` (`0x553e0`-`0x553fa`), compares `groundEntityNum` (`ent+0x7c`)
against the pusher's number (`0x55405`), compares the entity's
`absmin`/`absmax` against the bounds (`0x5540e`-`0x5548f`) and calls
`trap_Trace` (`0x554ee`) with the entity's own `mins`/`maxs` at its own
`r.currentOrigin`, the entity as the skip (its `r.ownerNum` for `eType` 4)
and its `clipmask` (`ent+0x190`), or 0x11 when that is 0; it compares the
trace's entity against the pusher (`0x55515`). The trace is skipped when the
clipmask is set and `r.contents` is `0x4000000` (`0x554a5`). INFERRED, off
those branches: a player is pushed when it stands on the pusher, or when its
box overlaps the pusher where the pusher now is; a corpse never is, its
contents being outside the box mask.

VERIFIED, then: every kept entity is unlinked (`0x55561`), handed to
`G_TryPushingEntity` in list order (`0x555c8`) and relinked when that returns
1 (`0x555e1`). On a 0, an `eType` 3 entity is relinked and skipped
(`0x555d4`); otherwise a pusher whose `pos.trType` or `apos.trType` is 4,
`TR_SINE`, calls `G_Damage(check, pusher, pusher, 0, 0, 99999, 0, 0x13, 0)`
(`0x55617`) and goes on, and any other stores the entity through its fourth
argument and returns 0 (`0x55621`). INFERRED: no scriptent verb writes
`TR_SINE` (section 9), so a blocked script mover never crushes.

VERIFIED, `G_TryPushingEntity` (`0x54930`): it returns 0 when the pusher's
`eFlags` carry 0x04000000 (byte `+0xb` bit 4, `0x54942`) and the entity's
ground is not the pusher; it adds `move` to `r.currentOrigin`
(`0x54956`-`0x54982`), calls `AngleVectors(amove)` and `VectorInverse` on the
right vector (`0x5498c`, `0x54995`), and turns the result about the pusher's
moved `r.currentOrigin` by that matrix (`0x549d5`-`0x54aae`). It calls
`trap_Trace` there (`0x54b3b`) with the entity's box. On a clear trace it
writes `0x3ff` to `groundEntityNum` when that was not the pusher
(`0x54b77`), the spot into `r.currentOrigin` and `s.pos.trBase`, and for a
client adds `(int)(amove[1] * 182.044) & 0xffff` to `ps.delta_angles[1]`
(`0x54bb5`-`0x54bf7`, the 182.044 at rodata `0x75bf4`) and writes `ps.origin`
(`0x54c00`-`0x54c1e`), steps `pushed_p` (`0x54f41`) and returns 1. When
`maxs.x * 0.5` exceeds 4 (`0x54c23`, rodata `0x75bf8` and `0x75c00`) it tries
offsets in steps of 4 (rodata `0x75c08`) with the same trace and the same
success path (`0x54db0`, `0x54e21`, `0x54e8e`); last it traces the entity's
unmoved `r.currentOrigin` (`0x55084`, `0x550a5`) and, clear, writes `0x3ff`
to `groundEntityNum` and returns 1 without stepping `pushed_p`
(`0x550cd`-`0x550d7`); otherwise it returns 0. INFERRED: the offsets are
RTCW's jitter, `z` 0 then ±4, `x` ±4, `y` ±4, in that nesting; a player is
15 wide, so it reaches 4 and no further.

VERIFIED, `G_MoverTeam` again: on a 0 from the push it walks the `pushed`
records back (`0x55744`-`0x557ef`), restoring `r.currentOrigin`,
`s.pos.trBase`, a client's `ps.origin` and subtracting
`ANGLE2SHORT(record yaw)` from `ps.delta_angles[1]` (`0x557b3`); then for
each team part adds `level.time - level.previousTime` to `pos.trTime` and
`apos.trTime` (`0x5580f`, `0x55821`) and re-evaluates both (`0x55838`,
`0x55851`); then calls `ent+0x208` if set (`0x55881`). `InitScriptMover`
(`0x60214`) writes `Reached_ScriptMover` to `ent+0x204` (`0x60364`) and
nothing to `ent+0x208`. INFERRED: a blocked script mover holds where it was,
one frame per blocked frame, with no callback, and its `movedone` comes that
much later.

VERIFIED, off the capture:

- **Riding.** The rider's origin follows the slab to the hundredth through
  every ride phase, z -21.875 to 26.125 and back on `movez`, x -215 to -167
  and back on `movex`; `groundEntityNum` reads 177 throughout.
- **Rotation.** `rotateyaw(2, 1)` about the world origin carried the rider
  from `(-215, 2463)` to `(-300.83, 2453.99)`, the point turned 2 degrees
  about the origin, and `delta_angles[1]` rose 18 a frame (0.1 degrees,
  `ANGLE2SHORT` truncated).
- **Pushing.** The player standing south of the slab at `(-215, 2430)` was
  first moved on the 28550 snapshot, the frame the slab's trajectory went
  from 6 to 7.2 units, and from there by the slab's 1.2 units a frame. The
  slab moving back left him where it pushed him.
- **Blocked.** The slab lowered from z 80 onto the standing player stopped
  at z 72 and held for the four seconds he stood there, its wire `trTime`
  advancing 50 every frame, and he was not moved. Once he was teleported
  away it went on down.
- **A verb on a moving entity.** A `moveto((0, 0, 0), 1)` called at 42250
  while the stalled descent was still running replaced it: the wire's new
  `trBase` z 30 is what `getorigin()` read at the call.
- **The residual yaw.** After `rotateyaw(2, 1)` and `rotateyaw(-2, 1)` the
  slab's `apos.trBase` reads `-0.0` and the pushed player drifts +x by 0.021
  a frame while it is pushed, the slab's whole push turning about the world
  origin by about 0.0005 degrees a frame. INFERRED: `amove` is never quite 0
  for an entity whose `r.currentAngles` came back off a rotate.

vcod: `crate::push` runs the push over the players after each frame's
script, in slot order, with `G_TryPushingEntity`'s jitter and its keep-in-place
fallback, and stalls the mover when one fits nowhere. Its candidate test is
a box, as retail's; its position test is our capsule plus a sweep from where
the body stood that passes through the pusher, because a zero-length capsule
is never `startsolid` under terrain and without the sweep the lowered slab
pushed the player through the ground. VERIFIED, vcod measurement against
the same fixture: a capsule candidate test met the plank 4.8 units of slab
travel later than retail's box did. Items, missiles and corpses are not
pushed, the residual yaw is not modelled, and the `TR_SINE` crush has no
verb that reaches it. `crates/server/tests/ride_ab.rs` is the gate.

## 13. A player linked to a mover

VERIFIED, off the capture: a player linked to a `script_origin` 32 units to
its east follows the parent's `movez(48, 2)` and `movex(48, 2)` exactly, and
on the parent's `rotateyaw(90, 2)` swings round it to `(-480, 2656)`, the
offset turned 90 degrees, with `delta_angles[1]` unchanged. VERIFIED: the
snapshot after the call carries the parent's first frame of motion, z
-22.675 at 44300 for a move called at 44250, a frame ahead of the player's
own script `.origin`.

INFERRED, off section 11's parent-first loop and `G_SetFixedLink`'s mode-2 arm
(`docs/research/cod11-gsc-object-model.md` 23.2): the re-anchor reads the
parent where it is this frame and turns the link offset by the parent's
axis; the offset is held in the parent's frame. The capture's parent had zero
angles at the link, so it cannot tell a parent-frame offset from a world one.

vcod: the re-anchor reads `ScriptRuntime::link_anchor`, the mover's plan at
the level time, and `linkTo` stores the offset in the parent's frame.

## 14. The client's half

Not implemented in vcod; read here for the follow-up.

VERIFIED, `cgame_mp_x86.dll`: `CG_ClipMoveToEntities` (`0x30028df0`) takes
an entity whose `solid` is `0xffffff` down a separate arm that calls syscall
`0x21` with `s.modelindex` and `BG_EvaluateTrajectory` (`0x30005470`) twice
before the trace syscall. INFERRED: that is Q3's `trap_CM_InlineModel` and the
`pos` and `apos` evaluated at the prediction time, so prediction clips a
moving brush model where its trajectory has it. `CG_BuildSolidList`
(`0x30028d50`) leaves out a `0xffffff` entity only on a flag bit 2 of a byte
at `centity+0xf8`.

VERIFIED: the function at `0x3001baa0` reads an entity number between 1 and
`0x3fd`, tests its `eType` against 5 and 8, calls `BG_EvaluateTrajectory`
four times and writes `in + (a - b)` and an angle delta;
`CG_PredictPlayerState` (`0x300294f0`) calls it at two sites. INFERRED: it is
Q3's `CG_AdjustPositionForMover`, carrying the predicted origin with the
ground entity's motion between the snapshot time and the render time.

vcod's server does not put a `script_brushmodel` on the wire yet
(`crate::game::wire`, `kind_of`), so its `modelindex` and `solid` there are
unread; the retail capture carries the entity as `eType` 8.
